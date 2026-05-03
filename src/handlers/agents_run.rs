//! `stores agents run` daemon — Phase 4 of T014.
//!
//! Polls `transition_history` at a fixed interval, looks for entries that
//! match an agent's declared `subscribes_to` triple, atomically claims the
//! pair `(store, row_id, agent_name)` via INSERT into `dispatch_locks` (the
//! UNIQUE constraint is what gives us idempotency against parallel daemons),
//! and dispatches either a shell `command` or a `builtin:*` keyword.
//!
//! Builtins are stubbed in this phase (Phase 6 wires them).

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::codegen::ddl::quote_ident;
use crate::flow::{decide, AgentEntry, AgentsYaml, Decision, NotifyEvent, PoliciesYaml};

/// Args parsed from the CLI.
pub struct RunArgs {
    pub poll_interval_ms: u64,
    pub detach: bool,
    pub log_file: Option<String>,
    /// Test/debug knob: stop the loop after this many poll iterations.
    pub max_iters: Option<usize>,
}

/// Process-wide shutdown flag; flipped by the SIGTERM handler. Public so
/// tests can flip it directly without sending a signal.
pub static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigterm(_: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn install_sigterm_handler() {
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_sigterm as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_sigterm as *const () as libc::sighandler_t,
        );
    }
}

pub fn run_daemon(args: RunArgs) -> Result<()> {
    let stores_dir = crate::paths::stores_dir()?;

    // Load agents.yaml — fail-loud on parse error; missing file → empty registry.
    let agents_path = stores_dir.join("agents.yaml");
    let agents = if agents_path.exists() {
        crate::flow::agents_yaml::load_from_path(&agents_path)
            .context("loading .stores/agents.yaml")?
    } else {
        AgentsYaml::default_empty()
    };

    // Load policies.yaml — fail-loud on parse error; missing file → empty.
    let policies_path = stores_dir.join("policies.yaml");
    let policies = if policies_path.exists() {
        let bytes = std::fs::read_to_string(&policies_path)
            .with_context(|| format!("reading {}", policies_path.display()))?;
        crate::flow::policies_yaml::PoliciesYaml::from_yaml(&bytes)
            .context("parsing .stores/policies.yaml")?
    } else {
        PoliciesYaml {
            hash: String::new(),
            policies: vec![],
        }
    };

    let config_path = stores_dir.join("config.yaml");

    if args.detach {
        detach_process(&args.log_file)?;
    }

    install_sigterm_handler();

    let db_path = crate::paths::db_path()?;
    let conn = crate::db::open(&db_path)?;
    let claimer = format!("daemon-{}", std::process::id());

    let mut iter = 0usize;
    loop {
        if SHUTDOWN.load(Ordering::SeqCst) {
            eprintln!(
                "[daemon] shutdown received, exiting after {} iterations",
                iter
            );
            break;
        }
        match poll_once(&conn, &agents, &policies, &config_path, &claimer) {
            Ok(n) if n > 0 => eprintln!("[daemon] dispatched {} job(s) in iteration {}", n, iter),
            Ok(_) => {}
            Err(e) => eprintln!("[daemon] poll error: {}", e),
        }
        iter += 1;
        if let Some(max) = args.max_iters {
            if iter >= max {
                break;
            }
        }
        sleep_interruptible(args.poll_interval_ms);
    }
    Ok(())
}

/// Sleep `ms` milliseconds in 50ms slices, returning early if SHUTDOWN is set.
fn sleep_interruptible(ms: u64) {
    let mut remaining = ms;
    while remaining > 0 && !SHUTDOWN.load(Ordering::SeqCst) {
        let chunk = remaining.min(50);
        std::thread::sleep(Duration::from_millis(chunk));
        remaining = remaining.saturating_sub(chunk);
    }
}

/// One poll iteration: scan `transition_history` for entries that match each
/// agent's subscriptions, gate via the policy layer, claim atomically, and
/// dispatch. Returns the number of dispatches performed (Halt-policied rows
/// do NOT count).
pub fn poll_once(
    conn: &Connection,
    agents: &AgentsYaml,
    policies: &PoliciesYaml,
    config_path: &Path,
    claimer: &str,
) -> Result<usize> {
    let mut dispatched = 0;
    for agent in &agents.agents {
        for sub in &agent.subscribes_to {
            let mut stmt = conn.prepare(
                "SELECT id, row_id, display_id FROM transition_history \
                 WHERE store = ?1 AND from_status = ?2 AND to_status = ?3 \
                 ORDER BY id ASC",
            )?;
            let rows: Vec<(i64, i64, String)> = stmt
                .query_map(
                    rusqlite::params![&sub.store, &sub.transition.from, &sub.transition.to],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?
                .filter_map(|r| r.ok())
                .collect();
            for (transition_id, row_id, display_id) in rows {
                // Policy gate: read the row as JSON, run decide().
                // On Halt: ntfy + skip (do NOT claim or retry).
                let row_json = read_row_as_json(conn, &sub.store, row_id)
                    .unwrap_or(Value::Object(serde_json::Map::new()));
                let decision = decide(
                    policies,
                    &sub.store,
                    &sub.transition.from,
                    &sub.transition.to,
                    &row_json,
                )
                .unwrap_or(Decision::Allow {
                    policy_id: "default-allow".into(),
                });
                let policy_id = match &decision {
                    Decision::Allow { policy_id } => policy_id.clone(),
                    Decision::Halt { policy_id } => {
                        let event = NotifyEvent {
                            row_id: display_id.clone(),
                            transition_attempted: format!(
                                "{}: {}→{}",
                                sub.store, sub.transition.from, sub.transition.to
                            ),
                            policy_id_or_actor_halt: policy_id.clone(),
                            summary: format!(
                                "policy '{}' halted dispatch to agent '{}'",
                                policy_id, agent.name
                            ),
                        };
                        let _ = crate::flow::notify_with_path(config_path, event);
                        continue;
                    }
                };

                let claimed = try_claim(
                    conn,
                    &sub.store,
                    row_id,
                    &display_id,
                    &agent.name,
                    transition_id,
                    claimer,
                )?;
                if !claimed {
                    continue;
                }
                let exit_code = run_dispatch(
                    conn,
                    agents,
                    config_path,
                    agent,
                    &sub.store,
                    row_id,
                    &display_id,
                    &sub.transition.from,
                    &sub.transition.to,
                    &policy_id,
                    &policies.hash,
                    &row_json,
                );
                let (status_str, code) = match exit_code {
                    Ok(c) => (
                        if c == 0 {
                            "ok".to_string()
                        } else {
                            format!("exit={}", c)
                        },
                        c,
                    ),
                    Err(e) => (format!("error: {}", e), -1),
                };
                let _ = mark_claim_finished(conn, &sub.store, row_id, &agent.name, &status_str);
                let _ = code;
                dispatched += 1;
            }
        }
    }
    Ok(dispatched)
}

/// Atomically claim `(store, row_id, agent_name)` by inserting a
/// `dispatch_locks` row. Returns `Ok(true)` if we won the claim,
/// `Ok(false)` if another claimer won (UNIQUE conflict).
pub fn try_claim(
    conn: &Connection,
    store: &str,
    row_id: i64,
    display_id: &str,
    agent_name: &str,
    transition_id: i64,
    claimer: &str,
) -> Result<bool> {
    let now = crate::handlers::row::now_iso8601();
    let res = conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            store,
            row_id,
            display_id,
            agent_name,
            transition_id,
            now,
            claimer,
        ],
    );
    match res {
        Ok(_) => Ok(true),
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            Ok(false)
        }
        Err(e) => Err(anyhow!("try_claim insert failed: {}", e)),
    }
}

fn mark_claim_finished(
    conn: &Connection,
    store: &str,
    row_id: i64,
    agent_name: &str,
    last_status: &str,
) -> Result<()> {
    let now = crate::handlers::row::now_iso8601();
    conn.execute(
        "UPDATE dispatch_locks SET last_status = ?1, finished_at = ?2 \
         WHERE store = ?3 AND row_id = ?4 AND agent_name = ?5",
        rusqlite::params![last_status, now, store, row_id, agent_name],
    )?;
    Ok(())
}

/// Run an agent's command. For builtins this is a stub until Phase 6.
///
/// `policy_ref` and `policies_hash` are forwarded as env vars
/// `STORES_POLICY_REF` / `STORES_POLICIES_HASH` so any follow-on substrate
/// transition the dispatched subprocess performs can record them on
/// `transition_history` (see `transition.rs::run_in_tx`). This is the
/// daemon→subscriber→substrate plumbing for AC5.3 / Task 5.2.
#[allow(clippy::too_many_arguments)]
fn run_dispatch(
    conn: &Connection,
    agents: &AgentsYaml,
    config_path: &Path,
    agent: &AgentEntry,
    store: &str,
    row_id: i64,
    display_id: &str,
    from: &str,
    to: &str,
    policy_ref: &str,
    policies_hash: &str,
    row_json: &Value,
) -> Result<i32> {
    if agent.is_builtin() {
        let kw = agent.command.trim_start_matches("builtin:");
        let ctx = crate::flow::builtins::DispatchCtx {
            conn,
            agents,
            config_path,
            policies_hash,
        };
        match crate::flow::builtins::dispatch_builtin(kw, row_json, &ctx) {
            Some(Ok(code)) => return Ok(code),
            Some(Err(e)) => {
                eprintln!(
                    "[daemon] builtin '{}' for {}/{} ({}->{}) failed: {}",
                    agent.command, store, display_id, from, to, e
                );
                return Ok(-1);
            }
            None => {
                eprintln!(
                    "[daemon] unknown builtin '{}' for {}/{} ({}->{}) policy_ref='{}'",
                    agent.command, store, display_id, from, to, policy_ref
                );
                return Ok(0);
            }
        }
    }
    use std::process::Command;
    let status = Command::new("sh")
        .arg("-c")
        .arg(&agent.command)
        .env("STORES_ROW_ID", row_id.to_string())
        .env("STORES_DISPLAY_ID", display_id)
        .env("STORES_TRANSITION_FROM", from)
        .env("STORES_TRANSITION_TO", to)
        .env("STORES_STORE", store)
        .env("STORES_POLICY_REF", policy_ref)
        .env("STORES_POLICIES_HASH", policies_hash)
        .status()
        .with_context(|| format!("spawning agent '{}'", agent.name))?;
    Ok(status.code().unwrap_or(-1))
}

/// Read a single row from `<store>` as a flat JSON object. JSON-typed columns
/// (TEXT-encoded) are best-effort parsed back into structured Values so
/// nested predicates work (`$linked_observations[0]`, etc).
fn read_row_as_json(conn: &Connection, store: &str, row_id: i64) -> Result<Value> {
    let sql = format!("SELECT * FROM {} WHERE id = ?1", quote_ident(store));
    let mut stmt = conn.prepare(&sql)?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(rusqlite::params![row_id])?;
    let row = rows
        .next()?
        .ok_or_else(|| anyhow!("row id={} not found in {}", row_id, store))?;
    let mut obj = serde_json::Map::new();
    for (i, name) in cols.iter().enumerate() {
        let v: rusqlite::types::Value = row.get(i)?;
        let jv = match v {
            rusqlite::types::Value::Null => Value::Null,
            rusqlite::types::Value::Integer(i) => Value::from(i),
            rusqlite::types::Value::Real(f) => {
                Value::from(serde_json::Number::from_f64(f).unwrap_or(0.into()))
            }
            rusqlite::types::Value::Text(s) => match serde_json::from_str::<Value>(&s) {
                Ok(parsed @ (Value::Object(_) | Value::Array(_))) => parsed,
                _ => Value::String(s),
            },
            rusqlite::types::Value::Blob(b) => {
                Value::String(String::from_utf8_lossy(&b).to_string())
            }
        };
        obj.insert(name.clone(), jv);
    }
    Ok(Value::Object(obj))
}

/// Public, just so a caller in lib.rs can resolve `.stores/config.yaml` for
/// tests without re-implementing path logic.
#[allow(dead_code)]
pub(crate) fn default_config_path() -> Result<PathBuf> {
    Ok(crate::paths::stores_dir()?.join("config.yaml"))
}

fn detach_process(log_file: &Option<String>) -> Result<()> {
    use std::os::unix::io::AsRawFd;
    let log_path = log_file
        .as_deref()
        .ok_or_else(|| anyhow!("--detach requires --log-file"))?;
    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            bail!("fork failed");
        }
        if pid > 0 {
            println!("{}", pid);
            std::process::exit(0);
        }
        if libc::setsid() < 0 {
            bail!("setsid failed");
        }
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("opening log file {}", log_path))?;
        let fd = f.as_raw_fd();
        libc::dup2(fd, libc::STDOUT_FILENO);
        libc::dup2(fd, libc::STDERR_FILENO);
        libc::close(libc::STDIN_FILENO);
        std::mem::forget(f);
    }
    Ok(())
}

// AgentsYaml::default_empty helper lives next to AgentsYaml itself but we
// keep a tiny adapter here so the daemon's empty-config path is one call.
impl AgentsYaml {
    pub fn default_empty() -> Self {
        Self {
            agents: vec![],
            deployment_specialist: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::SUBSTRATE_DDL;
    use crate::flow::agents_yaml::TransitionEdge;
    use crate::flow::{AgentEntry, BackoffKind, RetryPolicy, Subscription};

    fn fresh_db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SUBSTRATE_DDL).unwrap();
        // Minimal `tasks` table the policy/dispatch tests rely on. Fields
        // mirror what the production schema would expose to predicates.
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                display_id TEXT UNIQUE NOT NULL,
                status TEXT NOT NULL,
                tier_hint TEXT,
                branch TEXT
            );",
        )
        .unwrap();
        c
    }

    fn empty_policies() -> PoliciesYaml {
        PoliciesYaml {
            hash: String::new(),
            policies: vec![],
        }
    }

    fn cfg_path() -> std::path::PathBuf {
        // Pointing at a non-existent file is fine: notify_with_path falls
        // through to the env var (also unset in tests) → stderr-only.
        std::path::PathBuf::from("/tmp/stores-test-nonexistent-config.yaml")
    }

    fn insert_history(
        conn: &Connection,
        store: &str,
        row_id: i64,
        display_id: &str,
        from: &str,
        to: &str,
    ) {
        conn.execute(
            "INSERT INTO transition_history \
             (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'submit', 'ai_autonomous', '2026-05-03T00:00:00Z')",
            rusqlite::params![store, row_id, display_id, from, to],
        )
        .unwrap();
    }

    fn insert_task_row(
        conn: &Connection,
        row_id: i64,
        display_id: &str,
        status: &str,
        tier: &str,
        branch: &str,
    ) {
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![row_id, display_id, status, tier, branch],
        )
        .unwrap();
    }

    fn noop_agent(name: &str, store: &str, from: &str, to: &str) -> AgentEntry {
        AgentEntry {
            name: name.to_string(),
            subscribes_to: vec![Subscription {
                store: store.to_string(),
                transition: TransitionEdge {
                    from: from.to_string(),
                    to: to.to_string(),
                },
            }],
            command: "/bin/true".to_string(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy {
                max_attempts: 3,
                backoff: BackoffKind::Linear,
            },
            command_args: None,
        }
    }

    /// AC4.2 test (b): a tasks row freshly transitioned to in_review is
    /// dispatched once to a registered noop subscriber within one poll
    /// iteration.
    #[test]
    fn poll_dispatches_matching_row_once() {
        let conn = fresh_db();
        insert_task_row(&conn, 42, "T042", "in_review", "T2", "feat/x");
        insert_history(&conn, "tasks", 42, "T042", "ready", "in_review");
        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };
        let policies = empty_policies();
        let cfg = cfg_path();

        let n = poll_once(&conn, &agents, &policies, &cfg, "test-claimer").unwrap();
        assert_eq!(n, 1, "first poll dispatches the matching row exactly once");

        // Lock recorded.
        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 42 AND agent_name = 'noop'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);

        // Second poll on same db is a no-op (already claimed).
        let n2 = poll_once(&conn, &agents, &policies, &cfg, "test-claimer").unwrap();
        assert_eq!(
            n2, 0,
            "second poll does not re-dispatch an already-claimed row"
        );
    }

    /// AC4.3 test (c): two concurrent dispatch invocations against the same
    /// row result in exactly one row in dispatch_locks.
    #[test]
    fn concurrent_try_claim_yields_exactly_one_winner() {
        // Use a shared on-disk SQLite to allow two threads with their own
        // connections (in-memory DBs are not shared across handles).
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("t.sqlite");
        let setup = Connection::open(&db).unwrap();
        setup.execute_batch(SUBSTRATE_DDL).unwrap();
        drop(setup);

        let db1 = db.clone();
        let db2 = db.clone();
        let h1 = std::thread::spawn(move || {
            let c = Connection::open(&db1).unwrap();
            try_claim(&c, "tasks", 7, "T007", "noop", 1, "claimer-1").unwrap()
        });
        let h2 = std::thread::spawn(move || {
            let c = Connection::open(&db2).unwrap();
            try_claim(&c, "tasks", 7, "T007", "noop", 1, "claimer-2").unwrap()
        });

        let r1 = h1.join().unwrap();
        let r2 = h2.join().unwrap();
        assert!(
            r1 ^ r2,
            "exactly one of the two concurrent claims must succeed; got r1={r1} r2={r2}"
        );

        let c = Connection::open(&db).unwrap();
        let cnt: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 7 AND agent_name = 'noop'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1, "exactly one dispatch_locks row exists post-race");
    }

    /// AC4.5: malformed agents.yaml refuses to parse; the error names the
    /// failing field. The daemon's `run_daemon` would surface this via
    /// context; we exercise the underlying loader here.
    #[test]
    fn malformed_agents_yaml_is_refused_with_field_path() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agents.yaml");
        // `command` missing on the only entry.
        std::fs::write(
            &path,
            "agents:\n  - name: a\n    subscribes_to:\n      - store: tasks\n        transition: { from: a, to: b }\n",
        )
        .unwrap();
        let err = crate::flow::agents_yaml::load_from_path(&path)
            .unwrap_err()
            .to_string();
        assert!(err.contains("command"), "expected field path; got: {err}");
    }

    // ---- Phase 5: policy integration tests (AC5.1 cases d/e/f/g/h) ----
    pub(super) mod policy {
        use super::*;
        use crate::flow::policies_yaml::PoliciesYaml;
        use crate::flow::{install_notifier, MockNotifier, NotifierBackend, NotifyEvent};
        use std::sync::Mutex;

        /// Helper: build a PoliciesYaml from inline YAML and seed agents with
        /// a single noop subscriber on tasks: ready→in_review.
        fn fixture(policies_yaml: &str) -> (Connection, AgentsYaml, PoliciesYaml) {
            let conn = fresh_db();
            let agents = AgentsYaml {
                agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
                deployment_specialist: None,
            };
            let policies = if policies_yaml.is_empty() {
                empty_policies()
            } else {
                PoliciesYaml::from_yaml(policies_yaml).unwrap()
            };
            (conn, agents, policies)
        }

        /// Capture-and-forward shim so the test can assert on events after
        /// the boxed backend is installed into the OnceLock.
        struct Shim {
            inner: &'static MockNotifier,
        }
        impl NotifierBackend for Shim {
            fn send(&self, url: &str, event: &NotifyEvent) -> Result<()> {
                self.inner.send(url, event)
            }
        }

        /// Install a fresh global mock notifier and return its handle.
        fn install_mock() -> &'static MockNotifier {
            let mock: &'static MockNotifier = Box::leak(Box::new(MockNotifier::new()));
            install_notifier(Box::new(Shim { inner: mock }));
            mock
        }

        /// All policy tests share the global notifier + STORES_NTFY_URL env.
        /// Serialize them to keep the captured events scoped.
        fn lock() -> &'static Mutex<()> {
            use std::sync::OnceLock;
            static L: OnceLock<Mutex<()>> = OnceLock::new();
            L.get_or_init(|| Mutex::new(()))
        }

        /// AC5.1 case (d): integration — policy match drives daemon dispatch.
        /// An Allow policy with a matching predicate lets the row through;
        /// the same policy with a non-matching predicate falls through to
        /// default-allow.
        #[test]
        fn d_policy_match_drives_dispatch() {
            let _g = lock().lock().unwrap();
            let yaml = r#"
policies:
  - id: allow-T2-fast-path
    transition: { store: tasks, from: ready, to: in_review }
    predicate: { op: "==", left: "$tier_hint", right: "T2" }
    action: allow
"#;
            let (conn, agents, policies) = fixture(yaml);
            insert_task_row(&conn, 11, "T011", "in_review", "T2", "feat/x");
            insert_history(&conn, "tasks", 11, "T011", "ready", "in_review");

            let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer").unwrap();
            assert_eq!(n, 1, "T2 row matches allow policy and is dispatched");
        }

        /// AC5.1 case (e): default-allow — no rule matches, row still flows.
        #[test]
        fn e_default_allow_when_no_rule_matches() {
            let _g = lock().lock().unwrap();
            let (conn, agents, policies) = fixture("");
            insert_task_row(&conn, 21, "T021", "in_review", "T2", "feat/x");
            insert_history(&conn, "tasks", 21, "T021", "ready", "in_review");

            let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer").unwrap();
            assert_eq!(n, 1, "default-allow lets the row flow");
        }

        /// AC5.1 case (f) + AC5.2: NEVER overrides Allow → halt + ntfy fired.
        #[test]
        fn f_never_overrides_allow_and_skips_dispatch() {
            let _g = lock().lock().unwrap();
            std::env::set_var("STORES_NTFY_URL", "https://test.local");
            let mock = install_mock();
            let yaml = r#"
policies:
  - id: never-T3-fast-path
    transition: { store: tasks, from: ready, to: in_review }
    predicate: { op: "==", left: "$tier_hint", right: "T3" }
    action: never
  - id: allow-all
    transition: { store: tasks, from: ready, to: in_review }
    predicate: { op: "!=", left: "$tier_hint", right: "" }
    action: allow
"#;
            let (conn, agents, policies) = fixture(yaml);
            insert_task_row(&conn, 31, "T031", "in_review", "T3", "feat/x");
            insert_history(&conn, "tasks", 31, "T031", "ready", "in_review");

            let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer").unwrap();
            assert_eq!(n, 0, "NEVER halts dispatch (overrides Allow)");

            // No claim recorded.
            let cnt: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 31",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(cnt, 0, "halted row must not be claimed");

            // AC5.2: a single MockNotifier event with the halting policy id.
            let evs = mock.events();
            assert_eq!(evs.len(), 1, "exactly one ntfy event recorded");
            assert_eq!(evs[0].1.policy_id_or_actor_halt, "never-T3-fast-path");
            assert_eq!(evs[0].1.row_id, "T031");
            std::env::remove_var("STORES_NTFY_URL");
        }

        /// AC5.1 case (g) + AC5.3: when the dispatched subscriber writes a
        /// substrate transition, transition_history captures the policy_ref
        /// (matched id or 'default-allow') AND policies_hash. Manual writes
        /// (no env) record NULL.
        #[test]
        fn g_policy_ref_recording_on_auto_path_and_null_on_manual() {
            let _g = lock().lock().unwrap();
            // Auto path: env vars set → write into transition_history.
            std::env::set_var("STORES_POLICY_REF", "allow-T1-fast-path");
            std::env::set_var("STORES_POLICIES_HASH", "deadbeef");
            let conn = fresh_db();
            // Need a real schema-driven write for this; use the same minimal
            // observations schema the transition.rs tests use.
            let schema_yaml = r#"
name: observations
id_format: "L{:03d}"
default_actor: ai_with_human
lifecycle:
  states: [open, triaged]
  transitions:
    - {from: open, to: triaged, verb: triage, actor: ai_with_human}
fields:
  - name: summary
    type: text
    required: true
"#;
            let schema = crate::schema::Schema::from_yaml(schema_yaml).unwrap();
            // Create the per-store observations table on the same conn.
            conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
                .unwrap();
            conn.execute(
                "INSERT INTO observations (display_id, status, summary) VALUES ('L001', 'open', 'x')",
                [],
            )
            .unwrap();
            let cmd = clap::Command::new("triage")
                .arg(clap::Arg::new("display_id").required(true).index(1))
                .arg(clap::Arg::new("summary").long("summary"));
            let m = cmd.get_matches_from(["triage", "L001"]);
            crate::handlers::transition::run(
                &schema,
                &conn,
                &m,
                crate::schema::actor::Actor::Human.into(),
                "triage",
            )
            .unwrap();

            let (pref, phash): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT policy_ref, policies_hash FROM transition_history \
                     WHERE store='observations' AND display_id='L001'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(pref.as_deref(), Some("allow-T1-fast-path"));
            assert_eq!(phash.as_deref(), Some("deadbeef"));

            // Manual path: clear envs → next write must record NULL.
            std::env::remove_var("STORES_POLICY_REF");
            std::env::remove_var("STORES_POLICIES_HASH");
            let schema2_yaml = r#"
name: tasks2
id_format: "T{:03d}"
default_actor: ai_with_human
lifecycle:
  states: [open, triaged]
  transitions:
    - {from: open, to: triaged, verb: triage, actor: ai_with_human}
fields:
  - name: summary
    type: text
    required: true
"#;
            let schema2 = crate::schema::Schema::from_yaml(schema2_yaml).unwrap();
            conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema2))
                .unwrap();
            conn.execute(
                "INSERT INTO tasks2 (display_id, status, summary) VALUES ('T001', 'open', 'x')",
                [],
            )
            .unwrap();
            let cmd2 = clap::Command::new("triage")
                .arg(clap::Arg::new("display_id").required(true).index(1))
                .arg(clap::Arg::new("summary").long("summary"));
            let m2 = cmd2.get_matches_from(["triage", "T001"]);
            crate::handlers::transition::run(
                &schema2,
                &conn,
                &m2,
                crate::schema::actor::Actor::Human.into(),
                "triage",
            )
            .unwrap();
            let (pref2, phash2): (Option<String>, Option<String>) = conn
                .query_row(
                    "SELECT policy_ref, policies_hash FROM transition_history \
                     WHERE store='tasks2' AND display_id='T001'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert!(pref2.is_none(), "manual path: policy_ref must be NULL");
            assert!(phash2.is_none(), "manual path: policies_hash must be NULL");
        }

        /// AC5.1 case (h): ntfy mock — halt event body contains the row id
        /// and the halting policy id.
        #[test]
        fn h_ntfy_halt_event_body() {
            let _g = lock().lock().unwrap();
            std::env::set_var("STORES_NTFY_URL", "https://test.local");
            let mock = install_mock();
            let yaml = r#"
policies:
  - id: halt-on-empty-branch
    transition: { store: tasks, from: ready, to: in_review }
    predicate: { op: "==", left: "$branch", right: "" }
    action: halt
"#;
            let (conn, agents, policies) = fixture(yaml);
            insert_task_row(&conn, 99, "T099", "in_review", "T2", "");
            insert_history(&conn, "tasks", 99, "T099", "ready", "in_review");

            let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer").unwrap();
            assert_eq!(n, 0, "halt policy must skip dispatch");

            let evs = mock.events();
            assert_eq!(evs.len(), 1);
            assert_eq!(evs[0].1.row_id, "T099");
            assert_eq!(evs[0].1.policy_id_or_actor_halt, "halt-on-empty-branch");
            assert!(
                evs[0].1.transition_attempted.contains("ready"),
                "transition descriptor must mention from-state; got: {}",
                evs[0].1.transition_attempted
            );
            std::env::remove_var("STORES_NTFY_URL");
        }
    }

    /// SHUTDOWN flag is observed by sleep_interruptible.
    #[test]
    fn sleep_interruptible_exits_when_shutdown_set() {
        // Reset flag regardless of test order.
        SHUTDOWN.store(false, Ordering::SeqCst);
        let t = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            SHUTDOWN.store(true, Ordering::SeqCst);
        });
        let start = std::time::Instant::now();
        sleep_interruptible(5_000);
        let elapsed = start.elapsed();
        t.join().unwrap();
        SHUTDOWN.store(false, Ordering::SeqCst);
        assert!(
            elapsed < Duration::from_millis(500),
            "sleep_interruptible should return promptly when SHUTDOWN flips; elapsed={:?}",
            elapsed
        );
    }
}
