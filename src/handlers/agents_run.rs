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

    let seeded = seed_starting_line(&conn, &agents).context("seeding starting-line dispatch_locks")?;
    eprintln!("[daemon] seeded {} starting-line dispatch_locks", seeded);

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

                // Per-subscription predicate gate (T022 P2). Runs AFTER the
                // policy decide() halt-check so existing halt+ntfy semantics
                // are preserved; runs BEFORE try_claim so a false predicate
                // costs no claim and no ntfy.
                if let Some(pred) = &sub.predicate {
                    match crate::flow::predicate::eval(pred, &row_json) {
                        Ok(true) => {}
                        Ok(false) => continue,
                        Err(e) => {
                            eprintln!(
                                "[daemon] predicate eval error for agent '{}' on {}/{}: {}",
                                agent.name, sub.store, display_id, e
                            );
                            continue;
                        }
                    }
                }

                // Pre-claim cap check for builtin:auto-drive (T022 P4 / Task
                // 4.5). The `drive.max_parallel` config gates concurrent
                // drives BEFORE we burn a claim; otherwise a row would be
                // claimed-and-skipped, which would prevent retry on the next
                // poll. Only the auto-drive builtin is special-cased.
                if agent.command == "builtin:auto-drive" {
                    let cap = crate::flow::config::resolve_drive_max_parallel(config_path);
                    let live = count_live_drive_pids(conn).unwrap_or(0);
                    if live >= cap as usize {
                        continue;
                    }
                }

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
    // T022 P5: drive watchdog sweep — reconcile dispatch_locks for `auto-drive`
    // whose grandchild PID is no longer alive. Errors are logged, not fatal.
    if let Err(e) = crate::flow::builtins::auto_drive::sweep_drive_watchdog(
        conn,
        agents,
        config_path,
        &policies.hash,
    ) {
        eprintln!("[daemon] drive watchdog sweep error: {}", e);
    }
    Ok(dispatched)
}

/// Starting-line seeder (T026 P1). For each subscription declared in
/// `agents.yaml`, insert a `dispatch_locks` row marked
/// `claimed_by='starting-line-marker'` / `last_status='skip-historical'` for
/// every existing `transition_history` row that matches the subscription's
/// `(store, from, to)` edge. Uses `INSERT OR IGNORE` so any pre-existing real
/// claim is left untouched (the UNIQUE(store, row_id, agent_name) constraint
/// is what gives us the silent skip).
///
/// Returns the count of newly-inserted skip-historical rows.
pub fn seed_starting_line(conn: &Connection, agents: &AgentsYaml) -> Result<usize> {
    let now = crate::handlers::row::now_iso8601();
    let mut total = 0usize;
    for agent in &agents.agents {
        for sub in &agent.subscribes_to {
            let n = conn.execute(
                "INSERT OR IGNORE INTO dispatch_locks \
                 (store, row_id, display_id, agent_name, transition_id, \
                  claimed_at, claimed_by, last_status, finished_at) \
                 SELECT th.store, th.row_id, th.display_id, ?1, th.id, ?2, \
                        'starting-line-marker', 'skip-historical', ?2 \
                 FROM transition_history th \
                 WHERE th.store = ?3 AND th.from_status = ?4 AND th.to_status = ?5",
                rusqlite::params![
                    &agent.name,
                    &now,
                    &sub.store,
                    &sub.transition.from,
                    &sub.transition.to,
                ],
            )?;
            total += n;
        }
    }
    Ok(total)
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

pub(crate) fn mark_claim_finished(
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

/// True when `pid > 0` and `kill(pid, 0)` succeeds (signal-0 is the standard
/// liveness probe — sends nothing, errors EPERM/ESRCH on dead/foreign).
pub(crate) fn pid_is_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Count tasks rows whose `drive_pid` is set to a still-running process.
/// Used by the daemon's `poll_once` cap-check (Task 4.5).
pub(crate) fn count_live_drive_pids(conn: &Connection) -> Result<usize> {
    let mut stmt = conn.prepare("SELECT drive_pid FROM tasks WHERE drive_pid IS NOT NULL")?;
    let pids: Vec<i64> = stmt
        .query_map([], |r| r.get::<_, i64>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(pids.into_iter().filter(|p| pid_is_alive(*p as i32)).count())
}

/// Spawn `argv` as an orphaned grandchild detached from the daemon. Returns
/// the grandchild PID. Stdout/stderr go to `log_path` (created/appended).
/// `cwd` becomes the grandchild's working directory.
///
/// Uses double-fork + a pipe so the parent can read the grandchild PID and
/// reap the intermediate child without leaving a zombie. The grandchild is
/// reparented to PID 1 once the intermediate child exits.
pub(crate) fn spawn_detached_drive(
    argv: &[String],
    cwd: &Path,
    log_path: &Path,
) -> Result<i32> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;

    if argv.is_empty() {
        bail!("spawn_detached_drive: empty argv");
    }
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating log dir {}", parent.display()))?;
    }

    // Pipe for grandchild→parent PID communication.
    let mut fds: [libc::c_int; 2] = [-1, -1];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        bail!("pipe() failed");
    }
    let read_fd = fds[0];
    let write_fd = fds[1];

    let argv_owned: Vec<std::ffi::CString> = argv
        .iter()
        .map(|s| std::ffi::CString::new(s.as_bytes()).unwrap_or_else(|_| std::ffi::CString::new("").unwrap()))
        .collect();
    let cwd_c = std::ffi::CString::new(cwd.as_os_str().as_bytes())
        .map_err(|_| anyhow!("cwd contains NUL"))?;
    let log_c = std::ffi::CString::new(log_path.as_os_str().as_bytes())
        .map_err(|_| anyhow!("log_path contains NUL"))?;

    unsafe {
        let pid1 = libc::fork();
        if pid1 < 0 {
            libc::close(read_fd);
            libc::close(write_fd);
            bail!("first fork failed");
        }
        if pid1 == 0 {
            // ---- intermediate child ----
            libc::close(read_fd);
            if libc::setsid() < 0 {
                libc::_exit(11);
            }
            let pid2 = libc::fork();
            if pid2 < 0 {
                libc::_exit(12);
            }
            if pid2 == 0 {
                // ---- grandchild ----
                libc::close(write_fd);
                // Open log file (create | append). Mode 0644.
                let log_fd = libc::open(
                    log_c.as_ptr(),
                    libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
                    0o644,
                );
                if log_fd >= 0 {
                    libc::dup2(log_fd, libc::STDOUT_FILENO);
                    libc::dup2(log_fd, libc::STDERR_FILENO);
                    if log_fd > libc::STDERR_FILENO {
                        libc::close(log_fd);
                    }
                }
                // Close stdin (drive subprocess does not read it).
                libc::close(libc::STDIN_FILENO);

                if libc::chdir(cwd_c.as_ptr()) != 0 {
                    libc::_exit(13);
                }

                // Build argv ptr array (NULL-terminated).
                let mut argv_ptrs: Vec<*const libc::c_char> =
                    argv_owned.iter().map(|c| c.as_ptr()).collect();
                argv_ptrs.push(std::ptr::null());
                libc::execvp(argv_ptrs[0], argv_ptrs.as_ptr());
                // Only reached on exec failure.
                libc::_exit(127);
            }
            // ---- intermediate writes pid2 then exits ----
            let bytes = (pid2 as i32).to_le_bytes();
            let _ = libc::write(write_fd, bytes.as_ptr() as *const _, bytes.len());
            libc::close(write_fd);
            libc::_exit(0);
        }

        // ---- parent ----
        libc::close(write_fd);
        let mut buf = [0u8; 4];
        let n = libc::read(read_fd, buf.as_mut_ptr() as *mut _, buf.len());
        libc::close(read_fd);
        // Reap the intermediate child.
        let mut status: libc::c_int = 0;
        libc::waitpid(pid1, &mut status as *mut _, 0);
        if n != 4 {
            bail!("spawn_detached_drive: short read from pid pipe ({} bytes)", n);
        }
        let pid2 = i32::from_le_bytes(buf);
        if pid2 <= 0 {
            bail!("spawn_detached_drive: invalid grandchild pid {}", pid2);
        }
        // Touch fd vars so the AsRawFd import isn't flagged unused.
        let _ = std::io::stdout().as_raw_fd();
        Ok(pid2)
    }
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
                predicate: None,
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

    /// T022 P2 / AC2.2: when a subscription's predicate evaluates false on
    /// the row, poll_once skips the claim and dispatch entirely — no
    /// dispatch_locks row, no ntfy event, no return-count bump.
    #[test]
    fn predicate_false_skips_claim() {
        let conn = fresh_db();
        // workspace_path column is what auto-drive will gate on; add it.
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN workspace_path TEXT")
            .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path) \
             VALUES (?1, ?2, 'planning', 'T2', 'feat/x', '')",
            rusqlite::params![55, "T055"],
        )
        .unwrap();
        insert_history(&conn, "tasks", 55, "T055", "", "planning");

        let mut agent = noop_agent("auto-drive", "tasks", "", "planning");
        agent.subscribes_to[0].predicate =
            Some(crate::flow::predicate::PredicateExpr::Neq {
                left: serde_json::json!("$workspace_path"),
                right: serde_json::json!(""),
            });
        let agents = AgentsYaml {
            agents: vec![agent],
            deployment_specialist: None,
        };
        let policies = empty_policies();

        let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer").unwrap();
        assert_eq!(n, 0, "predicate-false rows must not dispatch");

        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 55",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 0, "predicate-false rows must not be claimed");
    }

    /// T022 P2 / AC2.2: predicate-true → claim+dispatch fires exactly once.
    #[test]
    fn predicate_true_claims_and_dispatches() {
        let conn = fresh_db();
        conn.execute_batch("ALTER TABLE tasks ADD COLUMN workspace_path TEXT")
            .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path) \
             VALUES (?1, ?2, 'planning', 'T2', 'feat/x', '/tmp/wt')",
            rusqlite::params![56, "T056"],
        )
        .unwrap();
        insert_history(&conn, "tasks", 56, "T056", "", "planning");

        let mut agent = noop_agent("auto-drive", "tasks", "", "planning");
        agent.subscribes_to[0].predicate =
            Some(crate::flow::predicate::PredicateExpr::Neq {
                left: serde_json::json!("$workspace_path"),
                right: serde_json::json!(""),
            });
        let agents = AgentsYaml {
            agents: vec![agent],
            deployment_specialist: None,
        };
        let policies = empty_policies();

        let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer").unwrap();
        assert_eq!(n, 1, "predicate-true row must dispatch once");

        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id = 56 AND agent_name='auto-drive'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 1);
    }

    /// T022 P4 / AC4.2: with `drive.max_parallel: 1` (default) and one drive
    /// already running, a second auto-drive dispatch is skipped pre-claim.
    /// `dispatch_locks` count for `auto-drive` must remain unchanged.
    #[test]
    fn auto_drive_cap_skips_when_full() {
        let conn = fresh_db();
        // Extend the minimal tasks table to carry workspace_path + drive_pid
        // (auto-drive's gating columns).
        conn.execute_batch(
            "ALTER TABLE tasks ADD COLUMN workspace_path TEXT;
             ALTER TABLE tasks ADD COLUMN drive_pid INTEGER;",
        )
        .unwrap();

        // Row already mid-drive: drive_pid = our own pid (alive).
        let our_pid = std::process::id() as i64;
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path, drive_pid) \
             VALUES (?1, ?2, 'executing', 'T2', 'feat/x', '/tmp/wt', ?3)",
            rusqlite::params![70, "T070", our_pid],
        )
        .unwrap();

        // Candidate row at planning awaiting auto-drive.
        conn.execute(
            "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path) \
             VALUES (?1, ?2, 'planning', 'T2', 'feat/y', '/tmp/wt2')",
            rusqlite::params![71, "T071"],
        )
        .unwrap();
        insert_history(&conn, "tasks", 71, "T071", "", "planning");

        let mut agent = noop_agent("auto-drive", "tasks", "", "planning");
        agent.command = "builtin:auto-drive".to_string();
        let agents = AgentsYaml {
            agents: vec![agent],
            deployment_specialist: None,
        };
        let policies = empty_policies();

        let n = poll_once(&conn, &agents, &policies, &cfg_path(), "test-claimer").unwrap();
        assert_eq!(n, 0, "cap is full → no dispatch");

        let cnt: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='auto-drive'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cnt, 0, "no claim must be taken when cap is full");
    }

    // ---- T026 P1: starting-line seeder tests ----

    /// Seeder inserts exactly one starting-line row per matching
    /// transition_history row across all subscriptions.
    #[test]
    fn seed_starting_line_inserts_one_per_history_row() {
        let conn = fresh_db();
        // Need an observations table for the second subscription's history rows.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS observations (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                display_id TEXT UNIQUE NOT NULL,
                status TEXT NOT NULL
            );",
        )
        .unwrap();

        // 3 tasks ready→in_review, 2 observations confirmed→ready.
        insert_history(&conn, "tasks", 1, "T001", "ready", "in_review");
        insert_history(&conn, "tasks", 2, "T002", "ready", "in_review");
        insert_history(&conn, "tasks", 3, "T003", "ready", "in_review");
        insert_history(&conn, "observations", 1, "L001", "confirmed", "ready");
        insert_history(&conn, "observations", 2, "L002", "confirmed", "ready");

        let agents = AgentsYaml {
            agents: vec![
                noop_agent("task-watcher", "tasks", "ready", "in_review"),
                noop_agent("obs-watcher", "observations", "confirmed", "ready"),
            ],
            deployment_specialist: None,
        };

        let n = seed_starting_line(&conn, &agents).unwrap();
        assert_eq!(n, 5, "should insert one row per matching history row");

        // Every newly-inserted row must carry the starting-line marker.
        let bad: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks \
                 WHERE NOT (claimed_by = 'starting-line-marker' \
                            AND last_status = 'skip-historical' \
                            AND finished_at IS NOT NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bad, 0, "every inserted row must be a starting-line marker");

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 5);
    }

    /// Re-running the seeder is a no-op: UNIQUE(store, row_id, agent_name)
    /// gives us idempotency via INSERT OR IGNORE.
    #[test]
    fn seed_starting_line_is_idempotent() {
        let conn = fresh_db();
        insert_history(&conn, "tasks", 1, "T001", "ready", "in_review");
        insert_history(&conn, "tasks", 2, "T002", "ready", "in_review");
        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };

        let n1 = seed_starting_line(&conn, &agents).unwrap();
        assert_eq!(n1, 2);
        let count1: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
            .unwrap();

        let n2 = seed_starting_line(&conn, &agents).unwrap();
        assert_eq!(n2, 0, "second run inserts zero rows");
        let count2: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count1, count2, "dispatch_locks count unchanged");
    }

    /// Pre-existing real locks are NEVER overwritten by the seeder.
    #[test]
    fn seed_starting_line_never_overwrites_real_locks() {
        let conn = fresh_db();
        insert_history(&conn, "tasks", 7, "T007", "ready", "in_review");
        // Pre-insert a real claim for (tasks, row 7, 'noop').
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, \
              claimed_at, claimed_by, last_status, finished_at) \
             VALUES ('tasks', 7, 'T007', 'noop', 1, '2026-01-01T00:00:00Z', \
                     'daemon-1', 'ok', '2026-01-01T00:00:01Z')",
            [],
        )
        .unwrap();

        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };

        let n = seed_starting_line(&conn, &agents).unwrap();
        assert_eq!(n, 0, "INSERT OR IGNORE skips conflicting row");

        let (claimed_by, last_status): (String, String) = conn
            .query_row(
                "SELECT claimed_by, last_status FROM dispatch_locks \
                 WHERE store='tasks' AND row_id=7 AND agent_name='noop'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(claimed_by, "daemon-1", "real lock untouched");
        assert_eq!(last_status, "ok", "real lock untouched");
    }

    /// Empty transition_history with subscribers configured → 0 rows inserted.
    #[test]
    fn seed_starting_line_no_history_no_op() {
        let conn = fresh_db();
        let agents = AgentsYaml {
            agents: vec![noop_agent("noop", "tasks", "ready", "in_review")],
            deployment_specialist: None,
        };
        let n = seed_starting_line(&conn, &agents).unwrap();
        assert_eq!(n, 0);
        let cnt: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 0);
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
