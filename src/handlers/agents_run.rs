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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::flow::{AgentEntry, AgentsYaml};

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
        libc::signal(libc::SIGTERM, handle_sigterm as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, handle_sigterm as *const () as libc::sighandler_t);
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
    if policies_path.exists() {
        let bytes = std::fs::read_to_string(&policies_path)
            .with_context(|| format!("reading {}", policies_path.display()))?;
        crate::flow::policies_yaml::PoliciesYaml::from_yaml(&bytes)
            .context("parsing .stores/policies.yaml")?;
    }

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
            eprintln!("[daemon] shutdown received, exiting after {} iterations", iter);
            break;
        }
        match poll_once(&conn, &agents, &claimer) {
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

pub fn run_backfill_placeholder() -> Result<()> {
    eprintln!("agents backfill: not yet implemented (Phase 7).");
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
/// agent's subscriptions, claim atomically, and dispatch. Returns the number
/// of dispatches performed.
pub fn poll_once(conn: &Connection, agents: &AgentsYaml, claimer: &str) -> Result<usize> {
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
                    agent,
                    &sub.store,
                    row_id,
                    &display_id,
                    &sub.transition.from,
                    &sub.transition.to,
                );
                let (status_str, code) = match exit_code {
                    Ok(c) => (
                        if c == 0 { "ok".to_string() } else { format!("exit={}", c) },
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
fn run_dispatch(
    agent: &AgentEntry,
    store: &str,
    row_id: i64,
    display_id: &str,
    from: &str,
    to: &str,
) -> Result<i32> {
    if agent.is_builtin() {
        eprintln!(
            "[daemon] builtin '{}' (stub) for {}/{} ({}->{})",
            agent.command, store, display_id, from, to
        );
        return Ok(0);
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
        .status()
        .with_context(|| format!("spawning agent '{}'", agent.name))?;
    Ok(status.code().unwrap_or(-1))
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
    use crate::flow::{AgentEntry, BackoffKind, RetryPolicy, Subscription};
    use crate::flow::agents_yaml::TransitionEdge;

    fn fresh_db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SUBSTRATE_DDL).unwrap();
        c
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
        }
    }

    /// AC4.2 test (b): a tasks row freshly transitioned to in_review is
    /// dispatched once to a registered noop subscriber within one poll
    /// iteration.
    #[test]
    fn poll_dispatches_matching_row_once() {
        let conn = fresh_db();
        insert_history(&conn, "tasks", 42, "T042", "ready", "in_review");
        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };

        let n = poll_once(&conn, &agents, "test-claimer").unwrap();
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
        let n2 = poll_once(&conn, &agents, "test-claimer").unwrap();
        assert_eq!(n2, 0, "second poll does not re-dispatch an already-claimed row");
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
