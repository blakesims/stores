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

/// Grace window (seconds) before the silent-zombie scan flips a row whose
/// drive_pid is NULL (post-spawn UPDATE not yet committed). Without this
/// window, a freshly-claimed auto-drive that has not yet run the
/// `UPDATE tasks SET drive_pid` statement could be flipped on the very next
/// 2s daemon poll. 10s is comfortably larger than the typical spawn→UPDATE
/// gap (sub-second) but small enough that real silent zombies surface fast.
pub(crate) const ZOMBIE_GRACE_SECS: i64 = 10;

/// In-cycle statuses we monitor for silent zombies. A row in any of these
/// states whose owning auto-drive subprocess has died (or never recorded its
/// PID) past the grace window is a silent zombie that the watchdog must
/// recover to `blocked`.
const IN_CYCLE_STATUSES: &[&str] = &[
    "planning",
    "plan_review",
    "ready",
    "executing",
    "code_review",
];

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

/// One row produced by `scan_zombie_tasks`: the row id (in `tasks`), the
/// display_id, the current status, the recorded `drive_pid` (or 0 when
/// NULL/missing), and the silent-zombie reason ("drive_pid_dead" or
/// "pid_never_recorded").
pub(crate) type ZombieRow = (i64, String, String, i64, &'static str);

/// Scan the `tasks` table for rows stuck in an in-cycle status whose owning
/// auto-drive subprocess is no longer alive (the L062 silent-zombie shape).
///
/// A row qualifies as a silent zombie when:
///   * `status` is in {planning, plan_review, ready, executing, code_review}
///   * an auto-drive `dispatch_locks` row exists for it (any state, including
///     already-closed via `mark_claim_finished`), AND
///   * either `drive_pid` is set but the PID is dead, OR `drive_pid` is NULL
///     and the lock was claimed more than `ZOMBIE_GRACE_SECS` seconds ago
///     (giving a freshly-spawned drive a window to commit its PID UPDATE).
///
/// The closed-lock case is the L062 shape: T022's spawn path closes the lock
/// immediately after spawn (`mark_claim_finished("ok")`), so the existing
/// open-lock sweep (`WHERE finished_at IS NULL`) cannot see it. This scan
/// inspects the `tasks` table directly and does not filter on `finished_at`.
///
/// `daemon_epoch` is an ISO-8601 Z-timestamp marking the current daemon
/// process's start time. When non-empty, rows whose `MIN(dispatch_locks.
/// claimed_at) < daemon_epoch` are skipped — these are pre-existing zombies
/// from a prior daemon lifetime and recovery is not the new daemon's
/// responsibility to assert. An empty string disables the gate (used by
/// tests that pre-date the gate's introduction and by callers that want
/// the legacy behavior).
pub(crate) fn scan_zombie_tasks(conn: &Connection, daemon_epoch: &str) -> Vec<ZombieRow> {
    // Cutoff timestamp string for grace-window comparison. Lexicographic
    // comparison on ISO-8601 Z-strings matches chronological order.
    let cutoff = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let cutoff_secs = now.saturating_sub(ZOMBIE_GRACE_SECS).max(0) as u64;
        // Re-use the project's iso formatter via a temporary now string then
        // recompute from secs to keep the format identical.
        let (y, mo, d, h, mi, s) = unix_to_ymd_hms(cutoff_secs);
        format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
    };

    // Placeholders for IN-clause.
    let in_placeholders = (1..=IN_CYCLE_STATUSES.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let cutoff_idx = IN_CYCLE_STATUSES.len() + 1;

    let epoch_idx = IN_CYCLE_STATUSES.len() + 2;
    // When daemon_epoch is empty the gate is disabled — the `?epoch_idx`
    // bind is the empty string, which is lexicographically <= every
    // ISO-8601 Z timestamp, so `MIN(dl.claimed_at) >= ''` always holds.
    let sql = format!(
        "SELECT t.id, t.display_id, t.status, COALESCE(t.drive_pid, 0), \
                COALESCE(MIN(dl.claimed_at), ''), COALESCE(t.drive_started_at, '') \
         FROM tasks t \
         JOIN dispatch_locks dl ON dl.store = 'tasks' AND dl.row_id = t.id \
                                  AND dl.agent_name = 'auto-drive' \
         WHERE t.status IN ({in_placeholders}) \
         GROUP BY t.id \
         HAVING ( \
                  ( \
                    COALESCE(t.drive_pid, 0) > 0 \
                    AND COALESCE(t.drive_started_at, '') < ?{cutoff_idx} \
                  ) \
                  OR ( \
                    COALESCE(t.drive_pid, 0) = 0 \
                    AND COALESCE(MIN(dl.claimed_at), '') < ?{cutoff_idx} \
                    AND COALESCE(MIN(dl.claimed_at), '') != '' \
                  ) \
                ) \
                AND COALESCE(MIN(dl.claimed_at), '') >= ?{epoch_idx}"
    );

    let mut params: Vec<rusqlite::types::Value> = IN_CYCLE_STATUSES
        .iter()
        .map(|s| rusqlite::types::Value::Text(s.to_string()))
        .collect();
    params.push(rusqlite::types::Value::Text(cutoff.clone()));
    params.push(rusqlite::types::Value::Text(daemon_epoch.to_string()));

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[auto-drive-watchdog] scan_zombie_tasks prepare failed: {e}");
            return Vec::new();
        }
    };
    let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
        ))
    });
    let rows = match rows {
        Ok(it) => it.filter_map(|r| r.ok()).collect::<Vec<_>>(),
        Err(e) => {
            eprintln!("[auto-drive-watchdog] scan_zombie_tasks query failed: {e}");
            return Vec::new();
        }
    };

    let mut out: Vec<ZombieRow> = Vec::new();
    for (row_id, display_id, status, pid, _claimed_at, _started) in rows {
        let reason: &'static str = if pid > 0 {
            if pid_is_alive(pid as i32) {
                continue;
            }
            "silent_zombie_pid_dead"
        } else {
            // PID NULL/zero — grace already enforced by the SQL HAVING; the
            // claimed_at < cutoff predicate guarantees we only land here
            // outside the grace window.
            "pid_never_recorded"
        };
        out.push((row_id, display_id, status, pid, reason));
    }
    out
}

// Local mirror of unix_to_ymd_hms (private in handlers::row); used for the
// grace-cutoff timestamp in scan_zombie_tasks. Matches now_iso8601's format.
fn unix_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let s = secs % 60;
    let total_min = secs / 60;
    let mi = total_min % 60;
    let total_hr = total_min / 60;
    let h = total_hr % 24;
    let days = total_hr / 24;
    let (y, mo, d) = days_to_ymd(days);
    (y, mo, d, h as u32, mi as u32, s as u32)
}

fn days_to_ymd(mut days: u64) -> (u32, u32, u32) {
    let mut year = 1970u32;
    loop {
        let dy = days_in_year(year) as u64;
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let leap = is_leap(year);
    let dim = [31u32, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 0u32;
    let mut d = days as u32;
    while month < 12 && d >= dim[month as usize] {
        d -= dim[month as usize];
        month += 1;
    }
    (year, month + 1, d + 1)
}

fn days_in_year(y: u32) -> u32 {
    if is_leap(y) {
        366
    } else {
        365
    }
}

fn is_leap(y: u32) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

/// Ensure the `transition_history` table has the `actor_note` column. SQLite's
/// `CREATE TABLE IF NOT EXISTS` does not migrate pre-existing tables, so a DB
/// that predates the actor_note DDL addition is missing the column and the
/// watchdog UPDATE in `annotate_drive_failed_history` would error
/// `no such column: actor_note`. T047: best-effort online ALTER preserves the
/// L062 silent-zombie audit trail.
fn ensure_actor_note_column(conn: &Connection) {
    let mut have_column = false;
    if let Ok(mut stmt) = conn.prepare("PRAGMA table_info(transition_history)") {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) {
            for name in rows.flatten() {
                if name == "actor_note" {
                    have_column = true;
                    break;
                }
            }
        }
    }
    if !have_column {
        if let Err(e) =
            conn.execute("ALTER TABLE transition_history ADD COLUMN actor_note TEXT", [])
        {
            eprintln!(
                "[auto-drive-watchdog] ensure_actor_note_column: ALTER TABLE failed: {}",
                e
            );
        }
    }
}

/// Annotate the most recent `mark_drive_failed` transition_history row for a
/// display_id with a structured reason note (e.g. `drive_pid_dead`,
/// `pid_never_recorded`). Best-effort; failures log but do not propagate.
fn annotate_drive_failed_history(conn: &Connection, display_id: &str, note: &str) {
    ensure_actor_note_column(conn);
    if let Err(e) = conn.execute(
        "UPDATE transition_history SET actor_note = ?1 \
         WHERE id = ( \
             SELECT id FROM transition_history \
             WHERE display_id = ?2 AND verb = 'mark_drive_failed' \
             ORDER BY id DESC LIMIT 1 \
         )",
        rusqlite::params![note, display_id],
    ) {
        eprintln!(
            "[auto-drive-watchdog] {}: actor_note annotate failed: {}",
            display_id, e
        );
    }
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
///
/// `daemon_epoch` is an ISO-8601 Z-timestamp captured once at daemon start.
/// It is forwarded to `scan_zombie_tasks` to gate the silent-zombie
/// (closed-lock) pass against pre-existing dead drive_pids whose lock
/// predates this daemon process. Pass `""` to disable the gate (legacy
/// semantics; preserved for tests).
pub fn sweep_drive_watchdog(
    conn: &Connection,
    agents: &AgentsYaml,
    config_path: &Path,
    policies_hash: &str,
    daemon_epoch: &str,
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

    let mut handled: std::collections::HashSet<String> = std::collections::HashSet::new();

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
            handled.insert(display_id.clone());
            continue;
        }
        match fire_mark_drive_failed(conn, &display_id, "drive_failed", policies_hash, None) {
            Ok(()) => {
                annotate_drive_failed_history(conn, &display_id, "drive_pid_dead");
                let ctx = DispatchCtx {
                    conn,
                    agents,
                    config_path,
                    policies_hash,
                };
                dispatch_to_specialist(&row, &ctx, &display_id, "auto-drive-watchdog");
                let _ = mark_claim_finished(conn, "tasks", row_id, "auto-drive", "drive_failed");
                acted += 1;
                handled.insert(display_id.clone());
            }
            Err(e) => {
                eprintln!(
                    "[auto-drive-watchdog] {}: mark_drive_failed failed: {:#}",
                    display_id, e
                );
            }
        }
    }

    // Silent-zombie pass: scan the tasks table directly for in-cycle rows
    // whose owning auto-drive subprocess is dead but whose dispatch_lock has
    // already been closed (the L062 shape). The open-lock scan above can
    // never see these because it filters on `finished_at IS NULL`.
    // The open-lock pass above intentionally does NOT take a daemon_epoch
    // gate: a dead-PID open lock from a prior lifetime is the same recovery
    // target regardless of which daemon process owned it. Only the
    // closed-lock (silent-zombie) path needs the epoch gate, because the
    // closed lock cannot be distinguished from a healthy mid-cycle row
    // without it (drive_pid stays set across daemon restarts).
    let zombies = scan_zombie_tasks(conn, daemon_epoch);
    for (row_id, display_id, _status, _pid, reason) in zombies {
        if handled.contains(&display_id) {
            // Already actioned by the open-lock pass this sweep.
            continue;
        }
        let row = match refresh_task_row(conn, &display_id) {
            Some(r) => r,
            None => continue,
        };
        // Idempotency guard: row already blocked → nothing to do.
        let cur_status = row.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if cur_status == "blocked" {
            continue;
        }
        match fire_mark_drive_failed(
            conn,
            &display_id,
            "drive_failed",
            policies_hash,
            Some(reason),
        ) {
            Ok(()) => {
                annotate_drive_failed_history(conn, &display_id, reason);
                let ctx = DispatchCtx {
                    conn,
                    agents,
                    config_path,
                    policies_hash,
                };
                dispatch_to_specialist(&row, &ctx, &display_id, "auto-drive-watchdog-zombie");
                // mark_claim_finished is idempotent: re-closing an already
                // closed lock is a no-op UPDATE matching zero rows.
                let _ = mark_claim_finished(conn, "tasks", row_id, "auto-drive", "drive_failed");
                acted += 1;
            }
            Err(e) => {
                eprintln!(
                    "[auto-drive-watchdog-zombie] {}: mark_drive_failed failed: {:#}",
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
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
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
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg_file, "", "").unwrap();

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
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
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
        let _ = sweep_drive_watchdog(&conn2, &agents, &cfg, "", "").unwrap();

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
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();

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
        assert_eq!(reason.as_deref(), Some("drive_failed:silent_zombie_pid_dead"));
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
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();

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
        assert_eq!(reason.as_deref(), Some("drive_failed:pid_never_recorded"));

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

    /// AC2.2: a row with drive_pid=NULL whose lock was claimed within
    /// `ZOMBIE_GRACE_SECS` of now is left untouched — protects freshly
    /// spawned drives that have not yet committed their PID UPDATE.
    #[test]
    fn watchdog_skips_within_grace_window() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T732", "planning", None);
        // Lock claimed_at = NOW (well within the grace window).
        let now = crate::handlers::row::now_iso8601();
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, last_status, finished_at) \
             VALUES ('tasks', ?1, ?2, 'auto-drive', 1, ?3, 'test-claimer', 'ok', ?3)",
            rusqlite::params![row_id, "T732", now],
        )
        .unwrap();

        // Confirm the constant exists and is sane.
        const { assert!(ZOMBIE_GRACE_SECS >= 1, "grace window must be positive") };

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T732'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "planning",
            "row within grace window must not be flipped; got status={status}"
        );
    }

    /// AC2.3: running sweep twice on the same zombie produces exactly one
    /// transition_history entry (mark_drive_failed) and one observation —
    /// the second sweep must be a no-op (idempotent guard).
    #[test]
    fn watchdog_idempotent_on_already_blocked() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // user-escalation reads a config_path; provide a tmp file so the
        // observation insert path completes cleanly.
        let tmp = tempfile::tempdir().unwrap();
        let cfg_file = tmp.path().join("config.yaml");
        std::fs::write(&cfg_file, "ntfy:\n  url: https://test.local\n").unwrap();

        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T733", "executing", Some(dead_pid()));
        insert_lock_closed(&conn, row_id, "T733");

        let agents = AgentsYaml::default_empty();
        // Sweep #1 — flips row to blocked, files one observation.
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg_file, "", "").unwrap();
        // Sweep #2 — must be a no-op.
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg_file, "", "").unwrap();

        let th_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE display_id='T733' AND verb='mark_drive_failed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            th_count, 1,
            "exactly one mark_drive_failed transition for T733 across two sweeps; got {th_count}"
        );

        let obs_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE task_id='T733'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            obs_count, 1,
            "exactly one observation for T733 across two sweeps; got {obs_count}"
        );
    }

    /// AC3.1 / AC3.2: the silent-zombie watchdog flip writes a suffix-tagged
    /// `blocked_reason` in `tasks` matching the audit regex
    /// `^drive_failed:(silent_zombie_pid_dead|pid_never_recorded)$`, and the
    /// corresponding `transition_history` row is mechanically distinguishable
    /// from a generic `drive_failed` flip (its `actor_note` carries the bare
    /// detection variant).
    #[test]
    fn transition_history_records_silent_zombie_reason() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();

        // Case A: dead-PID closed-lock zombie → drive_failed:silent_zombie_pid_dead
        let row_a = insert_task_full(&conn, "T740", "executing", Some(dead_pid()));
        insert_lock_closed(&conn, row_a, "T740");

        // Case B: pid-never-recorded closed-lock zombie → drive_failed:pid_never_recorded
        let row_b = insert_task_full(&conn, "T741", "planning", None);
        insert_lock_closed(&conn, row_b, "T741");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();

        let zombie_re = regex::Regex::new(
            r"^drive_failed:(silent_zombie_pid_dead|pid_never_recorded)$",
        )
        .unwrap();

        for (id, want_suffix) in [
            ("T740", "silent_zombie_pid_dead"),
            ("T741", "pid_never_recorded"),
        ] {
            let reason: String = conn
                .query_row(
                    "SELECT COALESCE(blocked_reason, '') FROM tasks WHERE display_id=?1",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                zombie_re.is_match(&reason),
                "{id}: blocked_reason {reason:?} must match silent-zombie audit regex",
            );
            assert_eq!(
                reason,
                format!("drive_failed:{want_suffix}"),
                "{id}: expected suffix {want_suffix}",
            );

            // transition_history mark_drive_failed row exists and carries the
            // bare detection variant in actor_note.
            let (verb, note): (String, String) = conn
                .query_row(
                    "SELECT verb, COALESCE(actor_note, '') FROM transition_history \
                     WHERE display_id=?1 ORDER BY id DESC LIMIT 1",
                    rusqlite::params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(verb, "mark_drive_failed");
            assert_eq!(note, want_suffix, "{id}: actor_note must match bare variant");
        }
    }

    // -----------------------------------------------------------------
    // T040: daemon-epoch gate against pre-existing zombies from a prior
    // daemon lifetime (false-positive scenario closed)
    // -----------------------------------------------------------------

    /// T040 regression: a row with a dead drive_pid + closed lock whose
    /// `claimed_at` predates the current daemon's start MUST NOT be flipped
    /// to `blocked`. The lock belongs to a prior daemon lifetime; recovery
    /// is not this daemon's responsibility to assert. No observation should
    /// be filed and no `mark_drive_failed` transition should land.
    #[test]
    fn watchdog_skips_pre_existing_zombie_from_prior_lifetime() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        // Lock claimed_at hard-coded by insert_lock_closed = 2026-05-03T00:00:00Z.
        let row_id = insert_task_full(&conn, "T750", "executing", Some(dead_pid()));
        insert_lock_closed(&conn, row_id, "T750");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        // daemon_epoch is FUTURE relative to the lock's claimed_at.
        let _ =
            sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-04T00:00:00Z").unwrap();

        // Row must remain at executing — not flipped.
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T750'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "executing",
            "pre-existing zombie must NOT be flipped under the daemon-epoch gate; got {status}"
        );

        // No observation filed for this display_id.
        let obs_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM observations WHERE task_id='T750'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(obs_count, 0, "no observation must be filed for skipped row");

        // No mark_drive_failed transition row landed for T750.
        let th_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE display_id='T750' AND verb='mark_drive_failed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            th_count, 0,
            "no mark_drive_failed transition must land for skipped row; got {th_count}"
        );
    }

    /// T040 corollary: same closed-lock + dead-PID setup, but with a
    /// daemon_epoch that PREDATES the lock's `claimed_at` — the row IS an
    /// in-lifetime zombie and the gate must allow the flip. Proves the gate
    /// does not over-block legitimate silent zombies.
    #[test]
    fn watchdog_flips_in_lifetime_zombie() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T751", "executing", Some(dead_pid()));
        insert_lock_closed(&conn, row_id, "T751");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        // daemon_epoch PREDATES the lock's claimed_at (2026-05-03T00:00:00Z).
        let _ =
            sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-02T00:00:00Z").unwrap();

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T751'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            status, "blocked",
            "in-lifetime zombie must still flip when daemon_epoch predates the lock"
        );
        assert_eq!(
            reason.as_deref(),
            Some("drive_failed:silent_zombie_pid_dead"),
            "blocked_reason must carry the silent-zombie suffix"
        );
    }

    /// T040 codex-revise: the `pid_never_recorded` branch (drive_pid IS NULL,
    /// stale executing row + lock) must also honor the daemon-epoch gate.
    /// Pre-existing zombie shape: lock claimed before this daemon's epoch ⇒
    /// SKIP; in-lifetime shape: lock claimed before epoch is FALSE ⇒ FLIP.
    #[test]
    fn watchdog_skips_pre_existing_pid_never_recorded() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        // drive_pid IS NULL → exercises the pid_never_recorded branch.
        let row_id = insert_task_full(&conn, "T752", "executing", None);
        insert_lock_closed(&conn, row_id, "T752");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        // daemon_epoch is FUTURE relative to the lock's claimed_at.
        let _ =
            sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-04T00:00:00Z").unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T752'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "executing",
            "pid_never_recorded pre-existing zombie must NOT flip under the gate; got {status}"
        );

        let th_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE display_id='T752' AND verb='mark_drive_failed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(th_count, 0, "no mark_drive_failed must land");
    }

    #[test]
    fn watchdog_flips_in_lifetime_pid_never_recorded() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T753", "executing", None);
        insert_lock_closed(&conn, row_id, "T753");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        // daemon_epoch PREDATES the lock's claimed_at.
        let _ =
            sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-02T00:00:00Z").unwrap();

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T753'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "blocked");
        assert_eq!(
            reason.as_deref(),
            Some("drive_failed:pid_never_recorded"),
            "blocked_reason must carry the pid_never_recorded suffix"
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

    /// T047 AC1.3: a transition_history table that predates the actor_note
    /// DDL addition (i.e. lacks the column) must be auto-migrated by the
    /// watchdog so the UPDATE no longer hits "no such column: actor_note".
    #[test]
    fn t047_annotate_drive_failed_history_adds_missing_column() {
        let conn = Connection::open_in_memory().unwrap();
        // Create a transition_history table WITHOUT the actor_note column,
        // mirroring a pre-T030 DB on disk.
        conn.execute_batch(
            "CREATE TABLE transition_history (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 store TEXT NOT NULL,
                 row_id INTEGER NOT NULL,
                 display_id TEXT NOT NULL,
                 from_status TEXT,
                 to_status TEXT NOT NULL,
                 verb TEXT NOT NULL,
                 invoker TEXT NOT NULL,
                 policy_ref TEXT,
                 policies_hash TEXT,
                 occurred_at TEXT NOT NULL
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO transition_history \
             (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
             VALUES ('tasks', 1, 'T999', 'planning', 'blocked', 'mark_drive_failed', \
                     'framework', '2026-05-06T00:00:00Z')",
            [],
        )
        .unwrap();

        // Sanity: column must NOT exist before the call.
        let pre_cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(transition_history)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        assert!(
            !pre_cols.iter().any(|c| c == "actor_note"),
            "precondition: actor_note must be absent before annotate"
        );

        // Call: must not panic, must not surface "no such column".
        annotate_drive_failed_history(&conn, "T999", "drive_pid_dead");

        // Post: column exists, row is annotated.
        let post_cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(transition_history)").unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, String>(1)).unwrap();
            rows.filter_map(|r| r.ok()).collect()
        };
        assert!(
            post_cols.iter().any(|c| c == "actor_note"),
            "actor_note column must be added by ensure_actor_note_column"
        );

        let note: String = conn
            .query_row(
                "SELECT COALESCE(actor_note, '') FROM transition_history \
                 WHERE display_id = 'T999' AND verb = 'mark_drive_failed' \
                 ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            note, "drive_pid_dead",
            "actor_note must carry the supplied note value"
        );
    }
}
