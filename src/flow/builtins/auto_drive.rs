//! `builtin:auto-drive` — spawn `stores tasks drive <id>` as a detached
//! subprocess when a task lands at `planning` with `workspace_path` set.
//!
//! Subscribes (via agents.yaml) to `tasks: ''→planning` with the predicate
//! `workspace_path != ""`. Records the grandchild PID + start timestamp on
//! the row so the watchdog (Phase 5) can reconcile drives that crash.
//!
//! Idempotency: if `drive_pid` is already set AND alive (kill -0), the call
//! is a no-op `Ok(0)`. If a stored PID is dead and the row's status is not
//! `in_review`, also a no-op — recovery belongs to the watchdog, not the
//! spawn path. The concurrency cap (`drive.max_parallel`) is enforced
//! pre-claim by `agents_run::poll_once`, not here.
//!
//! Test override: when `STORES_DRIVE_CMD` is set, the value is invoked via
//! `sh -c "<cmd>" <display_id>` instead of the `stores` binary. This lets
//! tests substitute a stub (`sleep 30`, etc.) without touching PATH.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use crate::flow::builtins::{
    dispatch_to_specialist, fire_mark_drive_failed, refresh_task_row, BuiltinResult, DispatchCtx,
};
use crate::flow::AgentsYaml;
use crate::handlers::agents_run::{mark_claim_finished, pid_is_alive, spawn_detached_drive};
use crate::handlers::row::now_iso8601;

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row
        .get("display_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if display_id.is_empty() {
        eprintln!("[auto-drive] tasks row missing display_id; skipping");
        return Ok(1);
    }
    let workspace_path = row
        .get("workspace_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if workspace_path.is_empty() {
        // Phase 2's predicate gate should have caught this; defend anyway.
        eprintln!(
            "[auto-drive] {}: workspace_path empty; skipping",
            display_id
        );
        return Ok(1);
    }
    let status = row.get("status").and_then(|v| v.as_str()).unwrap_or("");

    // Idempotency: if a PID is recorded and alive, no-op.
    if let Some(pid) = row.get("drive_pid").and_then(|v| v.as_i64()) {
        if pid > 0 && pid_is_alive(pid as i32) {
            eprintln!(
                "[auto-drive] {}: drive already running pid={}; skipping",
                display_id, pid
            );
            return Ok(0);
        }
        // Stored PID is dead. If the row hasn't moved past code_review (i.e.
        // status != in_review), let the watchdog (Phase 5) reconcile rather
        // than re-spawn here. Without a watchdog yet, we still no-op so we
        // don't silently re-spawn drives mid-cycle.
        if status != "in_review" {
            eprintln!(
                "[auto-drive] {}: stored drive_pid={} is dead; deferring to watchdog",
                display_id, pid
            );
            return Ok(0);
        }
    }

    // Build argv. Test override via STORES_DRIVE_CMD.
    let argv: Vec<String> = if let Ok(cmd) = std::env::var("STORES_DRIVE_CMD") {
        if cmd.is_empty() {
            return Ok(0);
        }
        vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("{} \"$@\"", cmd),
            "auto-drive-stub".to_string(),
            display_id.to_string(),
        ]
    } else {
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "stores".to_string());
        vec![
            exe,
            "tasks".to_string(),
            "drive".to_string(),
            display_id.to_string(),
            "--claude-code".to_string(),
            "--invoker".to_string(),
            "ai_autonomous".to_string(),
        ]
    };

    let cwd = PathBuf::from(workspace_path);
    let logs_dir = cwd.join(".stores").join("logs");
    let ts = now_iso8601().replace(':', "-");
    let log_path = logs_dir.join(format!("drive-{}-{}.log", display_id, ts));

    let pid = match spawn_detached_drive(&argv, &cwd, &log_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[auto-drive] {}: spawn failed: {:#}", display_id, e);
            return Ok(1);
        }
    };

    let now = now_iso8601();
    if let Err(e) = ctx.conn.execute(
        "UPDATE tasks SET drive_pid = ?1, drive_started_at = ?2, updated_at = ?3 \
         WHERE display_id = ?4",
        rusqlite::params![pid as i64, now, now, display_id],
    ) {
        eprintln!(
            "[auto-drive] {}: UPDATE drive_pid={} failed: {}",
            display_id, pid, e
        );
        return Ok(1);
    }

    eprintln!("[auto-drive] {}: spawned drive pid={}", display_id, pid);
    Ok(0)
}

/// Sweep open `dispatch_locks` for `agent_name='auto-drive'`. For each lock
/// whose grandchild PID is no longer alive, reconcile based on the task row's
/// status:
///
/// * `status == 'in_review'` — drive succeeded after wrap landed; just close
///   the lock (`mark_claim_finished` with `ok`).
/// * any other status — drive died mid-cycle. Fire `mark_drive_failed`
///   (framework actor) with `blocked_reason='drive_failed'`, dispatch to the
///   configured `deployment_specialist` (default `builtin:user-escalation`),
///   and close the lock (`drive_failed`).
///
/// Returns the number of locks the sweep took action on (closed or flipped).
/// Locks whose PID is still alive, or whose drive_pid is NULL (spawn UPDATE
/// not yet committed), are left untouched and counted zero.
pub fn sweep_drive_watchdog(
    conn: &Connection,
    agents: &AgentsYaml,
    config_path: &Path,
    policies_hash: &str,
) -> Result<usize> {
    let mut acted = 0usize;
    let locks: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT row_id, display_id FROM dispatch_locks \
             WHERE agent_name = 'auto-drive' AND finished_at IS NULL",
        )?;
        let it = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        it.filter_map(|r| r.ok()).collect()
    };

    for (row_id, display_id) in locks {
        let row = match refresh_task_row(conn, &display_id) {
            Some(r) => r,
            None => continue,
        };
        let pid = row.get("drive_pid").and_then(|v| v.as_i64()).unwrap_or(0);
        if pid <= 0 {
            // Spawn UPDATE not yet committed; defer until next sweep.
            continue;
        }
        if pid_is_alive(pid as i32) {
            continue;
        }
        let status = row.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if status == "in_review" {
            let _ = mark_claim_finished(conn, "tasks", row_id, "auto-drive", "ok");
            acted += 1;
            continue;
        }
        match fire_mark_drive_failed(conn, &display_id, "drive_failed", policies_hash) {
            Ok(()) => {
                let ctx = DispatchCtx {
                    conn,
                    agents,
                    config_path,
                    policies_hash,
                };
                dispatch_to_specialist(&row, &ctx, &display_id, "auto-drive-watchdog");
                let _ = mark_claim_finished(conn, "tasks", row_id, "auto-drive", "drive_failed");
                acted += 1;
            }
            Err(e) => {
                eprintln!(
                    "[auto-drive-watchdog] {}: mark_drive_failed failed: {:#}",
                    display_id, e
                );
            }
        }
    }
    Ok(acted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::flow::AgentsYaml;
    use crate::schema::Schema;
    use rusqlite::Connection;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
    }

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(tasks_yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        conn
    }

    fn insert_planning_task(conn: &Connection, display_id: &str, workspace_path: &str) {
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'planning', 'test', 'tslug', 'feat/tslug', ?2, ?3, ?4, ?4, 'ai_autonomous', 'ai_autonomous')",
            rusqlite::params![display_id, workspace_path, contract, now],
        ).unwrap();
    }

    fn task_row_json(conn: &Connection, display_id: &str) -> Value {
        let mut stmt = conn
            .prepare("SELECT * FROM tasks WHERE display_id = ?1")
            .unwrap();
        let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let mut rows = stmt.query(rusqlite::params![display_id]).unwrap();
        let row = rows.next().unwrap().unwrap();
        let mut obj = serde_json::Map::new();
        for (i, name) in cols.iter().enumerate() {
            let v: rusqlite::types::Value = row.get(i).unwrap();
            let jv = match v {
                rusqlite::types::Value::Null => Value::Null,
                rusqlite::types::Value::Integer(n) => Value::from(n),
                rusqlite::types::Value::Real(f) => {
                    Value::from(serde_json::Number::from_f64(f).unwrap_or(0.into()))
                }
                rusqlite::types::Value::Text(s) => Value::String(s),
                rusqlite::types::Value::Blob(b) => {
                    Value::String(String::from_utf8_lossy(&b).to_string())
                }
            };
            obj.insert(name.clone(), jv);
        }
        Value::Object(obj)
    }

    fn ctx_for<'a>(
        conn: &'a Connection,
        agents: &'a AgentsYaml,
        cfg: &'a std::path::Path,
    ) -> DispatchCtx<'a> {
        DispatchCtx {
            conn,
            agents,
            config_path: cfg,
            policies_hash: "",
        }
    }

    /// Make a cwd that exists (any tmp dir works for the spawn target).
    fn temp_cwd() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    /// AC4.1 test (i): spawn happens, tasks.drive_pid > 0 is recorded.
    #[test]
    fn i_spawn_records_pid() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("STORES_DRIVE_CMD", "sleep 5 #");
        let conn = fresh_db();
        let tmp = temp_cwd();
        insert_planning_task(&conn, "T700", tmp.path().to_str().unwrap());
        let row = task_row_json(&conn, "T700");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");

        let res = run(&row, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0);

        let (pid, started): (i64, Option<String>) = conn
            .query_row(
                "SELECT drive_pid, drive_started_at FROM tasks WHERE display_id='T700'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(pid > 0, "drive_pid must be > 0; got {pid}");
        assert!(started.is_some(), "drive_started_at must be set");
        assert!(pid_is_alive(pid as i32), "spawned process must be alive");

        // Reap: send SIGTERM so we don't leak the stub.
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        std::env::remove_var("STORES_DRIVE_CMD");
    }

    /// AC4.1 test (ii): re-run with live PID is a no-op (no second spawn).
    #[test]
    fn ii_rerun_with_live_pid_is_noop() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db();
        let tmp = temp_cwd();
        insert_planning_task(&conn, "T701", tmp.path().to_str().unwrap());
        // Inject our own pid (alive by definition).
        let our_pid = std::process::id() as i64;
        conn.execute(
            "UPDATE tasks SET drive_pid = ?1 WHERE display_id='T701'",
            rusqlite::params![our_pid],
        )
        .unwrap();
        let row = task_row_json(&conn, "T701");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");

        // Should NOT touch STORES_DRIVE_CMD because we never reach spawn.
        std::env::remove_var("STORES_DRIVE_CMD");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0);

        // drive_pid must remain == our_pid (no spawn, no overwrite).
        let pid: i64 = conn
            .query_row(
                "SELECT drive_pid FROM tasks WHERE display_id='T701'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pid, our_pid, "drive_pid must not be re-written");
    }

    /// AC4.1 test (iii): re-run with dead PID + status != in_review does NOT
    /// re-spawn (returns Ok(0); leaves drive_pid intact for the watchdog).
    #[test]
    fn iii_rerun_with_dead_pid_does_not_respawn() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("STORES_DRIVE_CMD");
        let conn = fresh_db();
        let tmp = temp_cwd();
        insert_planning_task(&conn, "T702", tmp.path().to_str().unwrap());
        // PID 0x7fffffff is overwhelmingly likely to be dead/unallocated.
        let dead_pid: i64 = 0x7fff_fffe;
        conn.execute(
            "UPDATE tasks SET drive_pid = ?1 WHERE display_id='T702'",
            rusqlite::params![dead_pid],
        )
        .unwrap();
        let row = task_row_json(&conn, "T702");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");

        let res = run(&row, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0);

        // drive_pid still the dead one (no overwrite, no spawn).
        let pid: i64 = conn
            .query_row(
                "SELECT drive_pid FROM tasks WHERE display_id='T702'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pid, dead_pid, "drive_pid must be left for watchdog");
    }

    /// AC4.3: spawned grandchild is reparented to PID 1 (orphaned from
    /// daemon). The stub writes its `getppid()` to a file; we poll for the
    /// file then assert the value.
    #[test]
    fn spawn_orphans_grandchild_to_pid_one() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = temp_cwd();
        let ppid_file = tmp.path().join("ppid.txt");
        // Sleep briefly so the intermediate child has exited and we've been
        // reparented to PID 1 before recording getppid.
        // sh's own $PPID is the grandchild's parent. After our intermediate
        // child has exited (sleep 0.2 wins), the grandchild has been
        // reparented to PID 1.
        let cmd = format!("sleep 0.2 && echo \"$PPID\" > {} #", ppid_file.display());
        std::env::set_var("STORES_DRIVE_CMD", &cmd);

        let conn = fresh_db();
        insert_planning_task(&conn, "T703", tmp.path().to_str().unwrap());
        let row = task_row_json(&conn, "T703");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let res = run(&row, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0);

        // Wait up to ~3s for the stub to finish.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline && !ppid_file.exists() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        std::env::remove_var("STORES_DRIVE_CMD");
        assert!(ppid_file.exists(), "stub must have written ppid.txt");
        let s = std::fs::read_to_string(&ppid_file).unwrap();
        let ppid: i32 = s.trim().parse().expect("ppid must parse");
        assert_eq!(ppid, 1, "grandchild's parent must be PID 1; got {ppid}");
    }

    // -----------------------------------------------------------------
    // Phase 5: drive watchdog sweep
    // -----------------------------------------------------------------

    /// Extended fresh-db: tasks + observations DDL, so user-escalation can
    /// file an observation when the watchdog flips a row.
    fn fresh_db_with_obs() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        for store in &["tasks", "observations"] {
            let yaml = BUNDLED_STORE_SCHEMAS
                .iter()
                .find(|(n, _)| n == store)
                .map(|(_, y)| *y)
                .unwrap();
            let schema = Schema::from_yaml(yaml).unwrap();
            conn.execute_batch(&ddl_for(&schema)).unwrap();
        }
        conn
    }

    fn insert_lock(conn: &Connection, row_id: i64, display_id: &str) {
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by) \
             VALUES ('tasks', ?1, ?2, 'auto-drive', 1, '2026-05-03T00:00:00Z', 'test-claimer')",
            rusqlite::params![row_id, display_id],
        )
        .unwrap();
    }

    fn insert_task_full(
        conn: &Connection,
        display_id: &str,
        status: &str,
        drive_pid: Option<i64>,
    ) -> i64 {
        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, drive_pid, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'test', 't', 'feat/x', '/tmp/no-such', ?3, ?4, ?5, ?5, 'framework', 'framework')",
            rusqlite::params![display_id, status, contract, drive_pid, now],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn dead_pid() -> i64 {
        // Pick a high-numbered PID overwhelmingly likely to be unallocated.
        0x7fff_fffe
    }

    /// AC5.1 (i): live PID + status='planning' → no flip, lock left open.
    #[test]
    fn watchdog_live_pid_no_flip() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let our_pid = std::process::id() as i64;
        let row_id = insert_task_full(&conn, "T720", "planning", Some(our_pid));
        insert_lock(&conn, row_id, "T720");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "").unwrap();
        assert_eq!(acted, 0, "live PID must not be touched");

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T720'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "planning");
        let finished: Option<String> = conn
            .query_row(
                "SELECT finished_at FROM dispatch_locks WHERE row_id=?1",
                rusqlite::params![row_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(finished.is_none(), "lock must remain open");
    }

    /// AC5.1 (ii) + AC5.2 + AC5.3: dead PID + status='executing' → row flips
    /// to blocked with blocked_reason='drive_failed'; one observation filed
    /// (task_id back-pointer); MockNotifier captures one event whose
    /// transition_attempted contains 'blocked'.
    #[test]
    fn watchdog_dead_pid_flips_blocked() {
        // Share the builtins-tests mutex so we don't race with other tests
        // that install_notifier on the global backend.
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Use a config-file path (immune to STORES_NTFY_URL env races across
        // parallel tests).
        let tmp = tempfile::tempdir().unwrap();
        let cfg_file = tmp.path().join("config.yaml");
        std::fs::write(&cfg_file, "ntfy:\n  url: https://test.local\n").unwrap();
        let mock: &'static crate::flow::MockNotifier =
            Box::leak(Box::new(crate::flow::MockNotifier::new()));
        struct Shim {
            inner: &'static crate::flow::MockNotifier,
        }
        impl crate::flow::NotifierBackend for Shim {
            fn send(&self, url: &str, ev: &crate::flow::NotifyEvent) -> anyhow::Result<()> {
                self.inner.send(url, ev)
            }
        }
        crate::flow::install_notifier(Box::new(Shim { inner: mock }));

        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T721", "executing", Some(dead_pid()));
        insert_lock(&conn, row_id, "T721");

        let agents = AgentsYaml::default_empty();
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg_file, "").unwrap();

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T721'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "blocked");
        assert_eq!(reason.as_deref(), Some("drive_failed"));

        let (obs_count, obs_task_id): (i64, String) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(task_id), '') FROM observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(obs_count, 1, "exactly one observation must be filed");
        assert_eq!(obs_task_id, "T721");

        let finished: Option<String> = conn
            .query_row(
                "SELECT finished_at FROM dispatch_locks WHERE row_id=?1",
                rusqlite::params![row_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(finished.is_some(), "lock must be closed");

        let evs = mock.events();
        let blocked_evs: Vec<_> = evs
            .iter()
            .filter(|(_, e)| e.row_id == "T721" && e.transition_attempted.contains("blocked"))
            .collect();
        assert_eq!(
            blocked_evs.len(),
            1,
            "exactly one ntfy event with 'blocked' for T721; got events: {:?}",
            evs
        );
    }

    /// AC5.1 (iii): dead PID + status='in_review' → drive succeeded; row not
    /// flipped, lock marked finished='ok'.
    #[test]
    fn watchdog_dead_pid_in_review_marks_ok() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T722", "in_review", Some(dead_pid()));
        insert_lock(&conn, row_id, "T722");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "").unwrap();
        assert_eq!(acted, 1);

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T722'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "in_review");

        let last: String = conn
            .query_row(
                "SELECT last_status FROM dispatch_locks WHERE row_id=?1",
                rusqlite::params![row_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last, "ok");
    }

    /// AC5.4: daemon-restart simulation — set up state on disk, drop the
    /// connection, reopen, and run sweep. The flip must still fire because
    /// the persisted lock + drive_pid are the only inputs the sweep needs.
    #[test]
    fn watchdog_survives_daemon_restart() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("t.sqlite");

        // ---- session 1: set up tables, insert task + lock ----
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(SUBSTRATE_DDL).unwrap();
            for store in &["tasks", "observations"] {
                let yaml = BUNDLED_STORE_SCHEMAS
                    .iter()
                    .find(|(n, _)| n == store)
                    .map(|(_, y)| *y)
                    .unwrap();
                let schema = Schema::from_yaml(yaml).unwrap();
                conn.execute_batch(&ddl_for(&schema)).unwrap();
            }
            let row_id = insert_task_full(&conn, "T723", "executing", Some(dead_pid()));
            insert_lock(&conn, row_id, "T723");
        }

        // ---- session 2: fresh connection, run sweep ----
        let conn2 = Connection::open(&db).unwrap();
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let _ = sweep_drive_watchdog(&conn2, &agents, &cfg, "").unwrap();

        let status: String = conn2
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T723'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "blocked", "watchdog must flip across restart");
        let reason: Option<String> = conn2
            .query_row(
                "SELECT blocked_reason FROM tasks WHERE display_id='T723'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason.as_deref(), Some("drive_failed"));
        let finished: Option<String> = conn2
            .query_row(
                "SELECT finished_at FROM dispatch_locks WHERE display_id='T723'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(finished.is_some(), "lock must be closed after sweep");
    }

    // -----------------------------------------------------------------
    // T030 Phase 1: silent-zombie reproductions (these MUST FAIL on main)
    // -----------------------------------------------------------------

    /// Insert a closed lock — `finished_at` is SET, simulating
    /// `mark_claim_finished` already ran post-spawn (the L062 shape).
    fn insert_lock_closed(conn: &Connection, row_id: i64, display_id: &str) {
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, last_status, finished_at) \
             VALUES ('tasks', ?1, ?2, 'auto-drive', 1, '2026-05-03T00:00:00Z', 'test-claimer', 'ok', '2026-05-03T00:00:01Z')",
            rusqlite::params![row_id, display_id],
        )
        .unwrap();
    }

    /// L062 silent-zombie shape #1: row stuck at `executing` with a dead
    /// `drive_pid`, but the dispatch_lock has already been closed by the
    /// post-spawn `mark_claim_finished` (so the current sweep's
    /// `WHERE finished_at IS NULL` filter skips it). Watchdog must still
    /// detect the zombie and flip the row to `blocked`.
    ///
    /// MUST FAIL on current main: sweep_drive_watchdog only inspects locks
    /// with `finished_at IS NULL`, so the row is left in `executing`.
    #[test]
    fn watchdog_silent_zombie_lock_already_closed() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T730", "executing", Some(dead_pid()));
        insert_lock_closed(&conn, row_id, "T730");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "").unwrap();

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T730'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            status, "blocked",
            "L062 silent zombie: row stays '{}' instead of flipping to 'blocked' (closed-lock path)",
            status
        );
        assert_eq!(reason.as_deref(), Some("drive_failed"));
    }

    /// L062 silent-zombie shape #2: drive subprocess died before recording
    /// its PID (the `UPDATE tasks SET drive_pid = ?` after spawn never
    /// committed). Row is stuck at `planning` with `drive_pid` NULL; the
    /// lock has been closed; the row's `updated_at` is far past the grace
    /// window. Watchdog must detect and flip to `blocked` with a
    /// `pid_never_recorded` annotation.
    ///
    /// MUST FAIL on current main: sweep_drive_watchdog skips locks with
    /// `finished_at IS NOT NULL` AND skips rows with `drive_pid <= 0`.
    #[test]
    fn watchdog_silent_zombie_pid_not_yet_recorded() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        // drive_pid=NULL ⇒ post-spawn UPDATE never landed.
        let row_id = insert_task_full(&conn, "T731", "planning", None);
        // updated_at on the row is hard-coded "2026-05-03T00:00:00Z" by
        // insert_task_full — > grace_window seconds ago for any sane window.
        insert_lock_closed(&conn, row_id, "T731");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "").unwrap();

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T731'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            status, "blocked",
            "L062 silent zombie: row stays '{}' instead of flipping to 'blocked' (pid_never_recorded path)",
            status
        );
        assert_eq!(reason.as_deref(), Some("drive_failed"));

        // The reason note `pid_never_recorded` must appear in the most
        // recent transition_history row's annotation/details for T731.
        let note: String = conn
            .query_row(
                "SELECT COALESCE(actor_note, '') FROM transition_history \
                 WHERE display_id='T731' ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap_or_default();
        assert!(
            note.contains("pid_never_recorded"),
            "expected reason note 'pid_never_recorded' in last transition_history; got {:?}",
            note
        );
    }

    /// AC4.4: dispatch_builtin("auto-drive", ...) resolves to this module.
    #[test]
    fn dispatch_builtin_returns_some_for_auto_drive() {
        let conn = fresh_db();
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let ctx = ctx_for(&conn, &agents, &cfg);
        let row = serde_json::json!({"display_id": ""});
        let res = crate::flow::builtins::dispatch_builtin("auto-drive", &row, &ctx);
        assert!(res.is_some(), "auto-drive keyword must resolve");
    }
}
