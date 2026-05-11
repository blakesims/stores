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
#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde_json::Value;

/// Test-only counter: incremented each time the CAS abort branch fires
/// (drive_pid alive at re-read).  Only compiled in debug builds; unavailable
/// in release.  Tests assert this counter to prove the race path executed.
#[cfg(debug_assertions)]
pub static CAS_ABORT_DRIVE_PID_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Test-only synchronization signal: set to `true` immediately when the
/// CAS pre-spawn delay hook is entered (before the sleep begins).  Tests
/// busy-wait on this flag before injecting a live drive_pid, guaranteeing
/// the scanner is past its fast-path read and inside the delay window.
/// Only compiled in debug builds; absent from release binaries.
#[cfg(debug_assertions)]
pub static CAS_DELAY_HOOK_ENTERED: AtomicBool = AtomicBool::new(false);

use crate::flow::builtins::{
    dispatch_to_specialist, fire_mark_drive_failed, load_tasks_schema, refresh_task_row,
    BuiltinResult, DispatchCtx,
};
use crate::flow::AgentsYaml;
use crate::handlers::agents_run::{
    drive_pid_exe_is_stale, mark_claim_finished, mark_claim_silent_zombie, pid_is_alive,
    spawn_detached_drive,
};
use crate::handlers::row::now_iso8601;
use crate::runner::liveness::{classify, LivenessClass, LivenessThresholds};

/// Grace window (seconds) before the silent-zombie scan flips a row whose
/// drive_pid is NULL (post-spawn UPDATE not yet committed). Without this
/// window, a freshly-claimed auto-drive that has not yet run the
/// `UPDATE tasks SET drive_pid` statement could be flipped on the very next
/// 2s daemon poll. 10s is comfortably larger than the typical spawn→UPDATE
/// gap (sub-second) but small enough that real silent zombies surface fast.
pub(crate) const ZOMBIE_GRACE_SECS: i64 = 10;

/// Grace window (seconds) after a task's latest `external_reviews` row has
/// been touched before the drive watchdog will mark the task `blocked`. Inside
/// this window the review reconciler (external_reviews subscriber +
/// submit-external-review verb) and the auto-drive respawn subscriber are
/// still propagating the verdict transition; the watchdog must not race them.
/// Without this gate, a dead drive_pid from the prior cycle can trip the
/// watchdog in the same poll tick that the substrate transitions
/// in_review → executing on a REVISE verdict.
pub(crate) const EXTERNAL_REVIEW_RACE_GRACE_SECS: i64 = 30;

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
    "in_review",
];

fn is_watchdog_actionable_status(status: &str) -> bool {
    IN_CYCLE_STATUSES.contains(&status)
}

/// Returns true when `verb` is reachable from `from_status` in the tasks
/// schema. Used by the watchdog to pre-check reachability before attempting
/// a transition that would fail and log spurious errors. Reachability is
/// derived from the schema's transition table, not duplicated here.
fn verb_reachable_from(schema: &crate::schema::Schema, from_status: &str, verb: &str) -> bool {
    schema
        .lifecycle
        .transitions
        .iter()
        .any(|t| t.verb == verb && t.from == from_status)
}

/// True if the task has an `external_reviews` control-plane row that the
/// drive watchdog must defer to. Returns true when the most-recent ER row
/// for `display_id` is either:
///   - in non-terminal status (`pending`, `running`, `tooling_held`) — the
///     review run is in flight, or
///   - within `EXTERNAL_REVIEW_RACE_GRACE_SECS` of its last update — the
///     review reconciler is still propagating a fresh verdict and a new
///     drive subprocess may not yet be spawned.
///
/// Returns false when the task has no ER rows or the latest row's last
/// update predates the grace window.
fn task_has_active_external_review_lane(
    conn: &Connection,
    display_id: &str,
    now_iso: &str,
) -> bool {
    let row = conn.query_row(
        "SELECT status, COALESCE(updated_at, completed_at, created_at, '') \
         FROM external_reviews WHERE task_id = ?1 ORDER BY id DESC LIMIT 1",
        rusqlite::params![display_id],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
    );
    let (status, updated_at) = match row {
        Ok(x) => x,
        Err(_) => return false,
    };
    if matches!(status.as_str(), "pending" | "running" | "tooling_held") {
        return true;
    }
    let now_epoch = match parse_iso8601_to_epoch_local(now_iso) {
        Some(v) => v,
        None => return false,
    };
    let upd_epoch = match parse_iso8601_to_epoch_local(&updated_at) {
        Some(v) => v,
        None => return false,
    };
    now_epoch.saturating_sub(upd_epoch) < EXTERNAL_REVIEW_RACE_GRACE_SECS
}

/// Parse `YYYY-MM-DDTHH:MM:SSZ` into a unix epoch. Local copy that mirrors
/// the helpers in `agents_run.rs` / `drive.rs` to avoid a wider re-export
/// churn for this narrow watchdog gate.
fn parse_iso8601_to_epoch_local(s: &str) -> Option<i64> {
    if s.len() < 20 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y: i64 = std::str::from_utf8(&b[0..4]).ok()?.parse().ok()?;
    let mo: i64 = std::str::from_utf8(&b[5..7]).ok()?.parse().ok()?;
    let d: i64 = std::str::from_utf8(&b[8..10]).ok()?.parse().ok()?;
    let h: i64 = std::str::from_utf8(&b[11..13]).ok()?.parse().ok()?;
    let mi: i64 = std::str::from_utf8(&b[14..16]).ok()?.parse().ok()?;
    let se: i64 = std::str::from_utf8(&b[17..19]).ok()?.parse().ok()?;
    if !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || !(0..=23).contains(&h)
        || !(0..=59).contains(&mi)
        || !(0..=60).contains(&se)
    {
        return None;
    }
    let mut days: i64 = 0;
    for yy in 1970..y {
        let leap = (yy % 4 == 0 && yy % 100 != 0) || yy % 400 == 0;
        days += if leap { 366 } else { 365 };
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    for m in 1..mo {
        days += dim[(m - 1) as usize] as i64;
    }
    days += d - 1;
    Some(days * 86_400 + h * 3600 + mi * 60 + se)
}

/// Returns true when the task still has auto-drive work to do.
///
/// Delegates entirely to `next_agent` from `compute()`. Pending work means
/// `next_agent IS NOT NULL`; no further work means `next_agent IS NULL`.
///
/// A1-strict (pi ruling): wrap_log is durable history, NOT a completion
/// sentinel. The daemon MUST NOT consult wrap_log to decide whether to
/// re-spawn a drive process. For in_review rows the schema always yields
/// `next_agent=Some("wrap")` (no `when:` guard); once wrap runs and the task
/// transitions to accepted/rejected the status leaves in_review and
/// `next_agent` becomes None. That is the sole completion signal.
pub(crate) fn has_pending_auto_drive_work(conn: &Connection, display_id: &str) -> Result<bool> {
    let schema = crate::flow::builtins::load_tasks_schema()?;
    let out = crate::handlers::next_action::compute(&schema, conn, display_id)?;
    Ok(out.next_agent.is_some())
}

fn mark_pending_handoff_lock(
    conn: &Connection,
    row_id: i64,
    display_id: &str,
    pid: i32,
) -> Result<()> {
    let now = now_iso8601();
    conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, attempts, \
          last_status, finished_at, daemon_epoch, claim_source, attempt, pid, terminal_reason, next_retry_at) \
         VALUES ('tasks', ?1, ?2, 'auto-drive', NULL, ?3, 'engine-runner', 0, \
                 'in_flight:pending_next', NULL, '', 'try_claim', 0, ?4, NULL, NULL) \
         ON CONFLICT(store, row_id, agent_name) DO UPDATE SET \
             claimed_at=excluded.claimed_at, claimed_by=excluded.claimed_by, \
             last_status=excluded.last_status, finished_at=NULL, \
             terminal_reason=NULL, next_retry_at=NULL, pid=excluded.pid, \
             claim_source=excluded.claim_source",
        rusqlite::params![row_id, display_id, now, pid as i64],
    )?;
    Ok(())
}

fn redispatch_pending_drive(
    conn: &Connection,
    row_id: i64,
    display_id: &str,
    agents: &AgentsYaml,
    config_path: &Path,
    policies_hash: &str,
) -> Result<bool> {
    let Some(row) = refresh_task_row(conn, display_id) else {
        return Ok(false);
    };
    if !has_pending_auto_drive_work(conn, display_id)? {
        return Ok(false);
    }
    let before = row.get("drive_pid").and_then(|v| v.as_i64()).unwrap_or(0);
    let ctx = DispatchCtx {
        conn,
        agents,
        config_path,
        policies_hash,
    };
    let code = run(&row, &ctx)?;
    if code != 0 {
        return Ok(false);
    }
    let after: i64 = conn.query_row(
        "SELECT COALESCE(drive_pid, 0) FROM tasks WHERE display_id = ?1",
        rusqlite::params![display_id],
        |r| r.get(0),
    )?;
    if after > 0 && after != before {
        mark_pending_handoff_lock(conn, row_id, display_id, after as i32)?;
        eprintln!(
            "[auto-drive-watchdog] {display_id}: re-dispatched pending auto-drive work pid={after}"
        );
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn redispatch_orphaned_next_agent(
    conn: &Connection,
    row_id: i64,
    agents: &AgentsYaml,
    config_path: &Path,
    policies_hash: &str,
) -> Result<Option<i32>> {
    // Fast-path read: row existence + pending work check before taking a write lock.
    let row_info: Option<(String, i64)> = conn
        .query_row(
            "SELECT display_id, COALESCE(drive_pid, 0) FROM tasks WHERE id = ?1",
            rusqlite::params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((display_id, _before_fast)) = row_info else {
        return Ok(None);
    };
    if !has_pending_auto_drive_work(conn, &display_id)? {
        return Ok(None);
    }

    // Test-only synchronization hook: STORES_TEST_CAS_PRE_SPAWN_DELAY_MS
    // introduces a sleep between the fast-path scanner read and the BEGIN
    // IMMEDIATE transaction so a test can inject a live drive_pid (or a live
    // dispatch_locks owner row) inside the gap — simulating the race the CAS
    // is built to defend against.  Gated on debug_assertions so release builds
    // compile it out entirely; the env-var cannot leak into production binaries.
    #[cfg(debug_assertions)]
    {
        if let Ok(ms) = std::env::var("STORES_TEST_CAS_PRE_SPAWN_DELAY_MS") {
            if let Ok(n) = ms.parse::<u64>() {
                // Signal BEFORE the sleep so the injector thread can observe
                // that the scanner is past its fast-path read and inside the
                // delay window.  The test busy-waits on this flag before
                // writing the live drive_pid, making the sync deterministic.
                CAS_DELAY_HOOK_ENTERED.store(true, Ordering::Release);
                eprintln!("[engine-runner::cas] {display_id}: pre-spawn delay start ({n}ms)");
                if n > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(n));
                }
                eprintln!("[engine-runner::cas] {display_id}: pre-spawn delay end");
            }
        }
    }

    // Atomic CAS: BEGIN IMMEDIATE acquires a write-intent lock so no concurrent
    // writer (another daemon iteration, an external `stores tasks drive`, or a
    // race-reused OS PID) can interleave between the re-read and the spawn+UPDATE.
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    // Re-read inside the transaction — the authoritative check.
    let reread: Option<(String, i64)> = tx
        .query_row(
            "SELECT display_id, COALESCE(drive_pid, 0) FROM tasks WHERE id = ?1",
            rusqlite::params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let Some((display_id_tx, before)) = reread else {
        // Row vanished between fast-path and lock.
        tx.rollback()?;
        return Ok(None);
    };

    // Verify drive_pid is still absent or dead.
    if before > 0 && pid_is_alive(before as i32) {
        eprintln!(
            "[engine-runner] {display_id_tx}: raced; drive_pid={before} alive since scan; skipping redispatch"
        );
        // Test-only sentinel: increments the global counter and emits a
        // greppable line so tests can assert the race path was actually
        // exercised (not just that no spawn happened sequentially).
        // Compiled out in release builds.
        #[cfg(debug_assertions)]
        if std::env::var_os("STORES_TEST_CAS_PRE_SPAWN_DELAY_MS").is_some() {
            CAS_ABORT_DRIVE_PID_COUNT.fetch_add(1, Ordering::Relaxed);
            eprintln!("[engine-runner::cas-abort] {display_id_tx}: orphan no longer applicable; drive_pid={before} now alive");
        }
        tx.rollback()?;
        return Ok(None);
    }

    // Verify no live auto-drive dispatch_lock appeared within the claim window
    // since the scanner pass (another engine-runner instance could have claimed it).
    let live_lock: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM dispatch_locks \
             WHERE store='tasks' AND row_id=?1 AND agent_name='auto-drive' \
               AND finished_at IS NULL AND COALESCE(pid, 0) > 0",
            rusqlite::params![row_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if live_lock > 0 {
        // Check whether that lock's PID is actually alive before aborting.
        let lock_pid: i64 = tx
            .query_row(
                "SELECT COALESCE(pid, 0) FROM dispatch_locks \
                 WHERE store='tasks' AND row_id=?1 AND agent_name='auto-drive' \
                   AND finished_at IS NULL AND COALESCE(pid, 0) > 0 \
                 LIMIT 1",
                rusqlite::params![row_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if lock_pid > 0 && pid_is_alive(lock_pid as i32) {
            eprintln!(
                "[engine-runner] {display_id_tx}: raced; live auto-drive lock pid={lock_pid} appeared since scan; skipping redispatch"
            );
            tx.rollback()?;
            return Ok(None);
        }
    }

    if crate::flow::engine_runner::has_fresh_running_current_run_marker(
        &tx,
        row_id,
        &now_iso8601(),
    )
    .unwrap_or(false)
    {
        eprintln!(
            "[engine-runner] {display_id_tx}: raced; fresh running current-run marker exists; skipping redispatch"
        );
        tx.rollback()?;
        return Ok(None);
    }

    // Still an orphan — perform the refresh and mutate drive_pid to Null so
    // `run()` treats this as a fresh spawn (not an already-running drive).
    let Some(mut row) = refresh_task_row(&tx, &display_id_tx) else {
        tx.rollback()?;
        return Ok(None);
    };
    if let Some(obj) = row.as_object_mut() {
        obj.insert("drive_pid".to_string(), Value::Null);
    }
    let ctx = DispatchCtx {
        conn: &tx,
        agents,
        config_path,
        policies_hash,
    };
    if run(&row, &ctx)? != 0 {
        tx.rollback()?;
        return Ok(None);
    }
    let after: i64 = tx.query_row(
        "SELECT COALESCE(drive_pid, 0) FROM tasks WHERE display_id = ?1",
        rusqlite::params![&display_id_tx],
        |r| r.get(0),
    )?;
    if after > 0 && after != before {
        mark_pending_handoff_lock(&tx, row_id, &display_id_tx, after as i32)?;
        tx.commit()?;
        return Ok(Some(after as i32));
    }
    tx.rollback()?;
    Ok(None)
}

fn drive_runner_configured() -> bool {
    let Ok(path) = crate::flow::config::default_config_path() else {
        return false;
    };
    let Ok(Some(cfg)) = crate::flow::config::load(&path) else {
        return false;
    };
    let Some(drive) = cfg.drive else {
        return false;
    };
    drive
        .default_runner
        .as_deref()
        .is_some_and(|s| !s.is_empty())
        || !drive.roles.is_empty()
}

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
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
    if row.get("activation").and_then(|v| v.as_str()) == Some("inactive") {
        eprintln!("[auto-drive] {}: activation inactive; skipping", display_id);
        return Ok(0);
    }

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

    if let Some(row_id) = row.get("id").and_then(|v| v.as_i64()) {
        if crate::flow::engine_runner::has_fresh_running_current_run_marker(
            ctx.conn,
            row_id,
            &now_iso8601(),
        )
        .unwrap_or(false)
        {
            eprintln!(
                "[auto-drive] {}: fresh running current-run marker exists; skipping",
                display_id
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
        let mut argv = vec![
            exe,
            "tasks".to_string(),
            "drive".to_string(),
            display_id.to_string(),
        ];
        // Phase A per-role runner config: when `.stores/config.yaml` declares
        // drive.default_runner or drive.roles, let `tasks drive` resolve the
        // runner per role. Preserve the historical Claude Code default when no
        // drive runner config exists, so older projects do not break.
        if !drive_runner_configured() {
            argv.push("--claude-code".to_string());
        }
        argv.extend(["--invoker".to_string(), "ai_autonomous".to_string()]);
        argv
    };

    let cwd = PathBuf::from(workspace_path);
    let logs_dir = cwd.join(".stores").join("logs");
    let ts = now_iso8601().replace(':', "-");
    let log_path = logs_dir.join(format!("drive-{}-{}.log", display_id, ts));

    let canonical_stores_dir = match crate::paths::stores_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "[auto-drive] {}: resolving stores_dir failed: {:#}",
                display_id, e
            );
            return Ok(1);
        }
    };
    let Some(canonical_root) = canonical_stores_dir.parent().map(|p| p.to_path_buf()) else {
        eprintln!(
            "[auto-drive] {}: stores_dir has no parent: {}",
            display_id,
            canonical_stores_dir.display()
        );
        return Ok(1);
    };
    let canonical_root_string = canonical_root.to_string_lossy().to_string();
    let env_overrides = [
        ("STORES_AUTO_DRIVE_HANDOFF_DISPLAY_ID", display_id),
        ("STORES_ROOT", canonical_root_string.as_str()),
    ];

    // Contract: cwd stays at the worktree for the runner; STORES_ROOT routes the child to the canonical substratum store.
    let pid = match spawn_detached_drive(&argv, &cwd, &log_path, &env_overrides) {
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
///   * `status` is in {planning, plan_review, ready, executing, code_review, in_review}
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
                                  AND COALESCE(dl.terminal_reason, '') != 'silent_zombie' \
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
    let dim = [
        31u32,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
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
        if let Err(e) = conn.execute(
            "ALTER TABLE transition_history ADD COLUMN actor_note TEXT",
            [],
        ) {
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

/// Build the structured log line emitted when the watchdog detects a live
/// drive subprocess whose exe inode was replaced on disk. Isolated here so
/// tests can assert its contents directly without stderr capture.
fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_epoch(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(v) = s.parse::<i64>() {
        return Some(v);
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

fn dead_pid_still_within_heartbeat_grace(heartbeat_at: Option<&str>) -> bool {
    let Some(heartbeat_epoch) = heartbeat_at.and_then(parse_epoch) else {
        return false;
    };
    let idle_secs = now_epoch().saturating_sub(heartbeat_epoch);
    idle_secs < LivenessThresholds::from_env().no_output_secs
}

pub(crate) fn stale_exe_log_line(display_id: &str, pid: i64) -> String {
    format!(
        "[auto-drive-watchdog] {display_id}: drive_pid={pid} stale_binary_inode \
         (/proc/{pid}/exe -> deleted); advisory only for already-running drive"
    )
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
    let now_iso = now_iso8601();
    let tasks_schema = load_tasks_schema()?;
    let locks: Vec<(i64, String, Option<String>, Option<String>)> = {
        let mut stmt = conn.prepare(
            "SELECT row_id, display_id, claimed_at, heartbeat_at FROM dispatch_locks \
             WHERE agent_name = 'auto-drive' AND finished_at IS NULL",
        )?;
        let it = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2).ok().flatten(),
                r.get::<_, Option<String>>(3).ok().flatten(),
            ))
        })?;
        it.filter_map(|r| r.ok()).collect()
    };

    let mut handled: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (row_id, display_id, claimed_at, heartbeat_at) in locks {
        let row = match refresh_task_row(conn, &display_id) {
            Some(r) => r,
            None => continue,
        };
        let pid = row.get("drive_pid").and_then(|v| v.as_i64()).unwrap_or(0);
        if pid <= 0 {
            // Spawn UPDATE not yet committed; defer until next sweep.
            continue;
        }
        let pid_alive = pid_is_alive(pid as i32);
        if pid_alive && drive_pid_exe_is_stale(pid as i32) {
            // Boundary split: daemon/control-plane stale-exe checks remain
            // fail-loud before spawning work, but an already-running drive is
            // pinned to the executable image it started with. Linux permits
            // that image to continue after the install path is replaced; this
            // watchdog must not convert safe post-spawn inode drift into a
            // task-level drive failure. Stale-exe drift is advisory metadata;
            // continue into the normal alive-PID liveness classifier below so
            // it cannot mask unrelated stalled/no-output failures.
            eprintln!("{}", stale_exe_log_line(&display_id, pid));
        }
        if pid_alive {
            let status = row.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if !is_watchdog_actionable_status(status)
                || task_has_active_external_review_lane(conn, &display_id, &now_iso)
            {
                continue;
            }
            let liveness = classify(
                claimed_at.as_deref().and_then(parse_epoch),
                heartbeat_at.as_deref().and_then(parse_epoch),
                now_epoch(),
                &LivenessThresholds::from_env(),
            );
            let detail = match liveness {
                LivenessClass::StalledNoOutput {
                    idle_secs,
                    threshold_secs,
                } => {
                    format!("no_output_idle_{idle_secs}s_threshold_{threshold_secs}s")
                }
                LivenessClass::WallClockElapsed { .. }
                | LivenessClass::Active { .. }
                | LivenessClass::Unknown => continue,
            };
            match fire_mark_drive_failed(
                conn,
                &display_id,
                "drive_failed",
                policies_hash,
                Some(&detail),
            ) {
                Ok(()) => {
                    annotate_drive_failed_history(conn, &display_id, &detail);
                    let ctx = DispatchCtx {
                        conn,
                        agents,
                        config_path,
                        policies_hash,
                    };
                    dispatch_to_specialist(&row, &ctx, &display_id, "auto-drive-watchdog-liveness");
                    let agent = agents.agents.iter().find(|a| a.name == "auto-drive");
                    let _ = mark_claim_silent_zombie(
                        conn,
                        "tasks",
                        row_id,
                        agent,
                        "auto-drive",
                        &detail,
                    );
                    acted += 1;
                    handled.insert(display_id.clone());
                }
                Err(e) => {
                    eprintln!(
                        "[auto-drive-watchdog] {}: mark_drive_failed liveness failed: {:#}",
                        display_id, e
                    );
                }
            }
            continue;
        }
        if dead_pid_still_within_heartbeat_grace(heartbeat_at.as_deref()) {
            eprintln!(
                "[auto-drive-watchdog] {}: drive_pid={} is gone but heartbeat is recent; \
                 deferring silent-zombie classification",
                display_id, pid
            );
            handled.insert(display_id.clone());
            continue;
        }
        let status = row.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if !is_watchdog_actionable_status(status) {
            continue;
        }
        if status == "in_review" {
            if redispatch_pending_drive(
                conn,
                row_id,
                &display_id,
                agents,
                config_path,
                policies_hash,
            )? {
                acted += 1;
                handled.insert(display_id.clone());
                continue;
            }
            let _ = mark_claim_finished(conn, "tasks", row_id, "auto-drive", "ok");
            acted += 1;
            handled.insert(display_id.clone());
            continue;
        }
        if task_has_active_external_review_lane(conn, &display_id, &now_iso) {
            eprintln!(
                "[auto-drive-watchdog] {}: deferring mark_drive_failed (status={status}); \
                 external_review control-plane row is in flight or within race grace",
                display_id
            );
            continue;
        }
        match fire_mark_drive_failed(
            conn,
            &display_id,
            "drive_failed",
            policies_hash,
            Some("silent_zombie_pid_dead"),
        ) {
            Ok(()) => {
                annotate_drive_failed_history(conn, &display_id, "silent_zombie_pid_dead");
                let ctx = DispatchCtx {
                    conn,
                    agents,
                    config_path,
                    policies_hash,
                };
                dispatch_to_specialist(&row, &ctx, &display_id, "auto-drive-watchdog");
                let agent = agents.agents.iter().find(|a| a.name == "auto-drive");
                let _ = mark_claim_silent_zombie(
                    conn,
                    "tasks",
                    row_id,
                    agent,
                    "auto-drive",
                    "silent_zombie_pid_dead",
                );
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

    // Pending-next handoff pass: terminal-ok closed locks whose task is still
    // in_review with pending work (next_agent IS NOT NULL) and whose drive
    // subprocess is dead are re-dispatched so wrap fires without manual
    // intervention.
    //
    // NARROW predicate (pi ruling, MAJOR 3): only sweep locks where
    // terminal_reason='ok' AND t.status='in_review'. Non-ok locks (error,
    // retry, silent_zombie) belong to the L134/L135 retry/error lifecycle and
    // must NOT be swept here. next_agent is computed dynamically by
    // has_pending_auto_drive_work below; the SQL gate narrows to the
    // structural condition observable in the DB.
    //
    // T067 r7 HIGH fix: exclude locks written by force_close_auto_drive_lock_ok
    // (which sets last_status='ok:wrap_completed'). Those locks represent
    // current-cycle wrap dispatch that already completed; re-dispatching them
    // would fire wrap again, defeating the r5/r6 lifecycle-closure design.
    // Plain last_status='ok' (old handoff that died before force-close) remains
    // eligible for re-dispatch (correct amend/re-entry semantics).
    //
    // Why last_status rather than a typed terminal_reason variant: the
    // terminal_reason column carries a CHECK constraint
    // (ok|exit_nonzero|error|silent_zombie|timeout|halted|legacy_unknown) that
    // cannot be broadened on existing DBs without table recreation. last_status
    // is free-text and is the designated typed discriminator for this case.
    let pending_locks: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT dl.row_id, dl.display_id \
             FROM dispatch_locks dl JOIN tasks t ON t.id = dl.row_id \
             WHERE dl.store = 'tasks' AND dl.agent_name = 'auto-drive' \
               AND dl.terminal_reason = 'ok' \
               AND dl.last_status != 'ok:wrap_completed' \
               AND t.status = 'in_review' \
               AND COALESCE(t.drive_pid, 0) > 0",
        )?;
        let it = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        it.filter_map(|r| r.ok()).collect()
    };
    for (row_id, display_id) in pending_locks {
        if handled.contains(&display_id) {
            continue;
        }
        let pid: i64 = conn
            .query_row(
                "SELECT COALESCE(drive_pid, 0) FROM tasks WHERE display_id = ?1",
                rusqlite::params![&display_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if pid <= 0 || pid_is_alive(pid as i32) {
            continue;
        }
        if redispatch_pending_drive(
            conn,
            row_id,
            &display_id,
            agents,
            config_path,
            policies_hash,
        )? {
            acted += 1;
            handled.insert(display_id);
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
        if task_has_active_external_review_lane(conn, &display_id, &now_iso) {
            eprintln!(
                "[auto-drive-watchdog-zombie] {}: deferring mark_drive_failed (status={cur_status}); \
                 external_review control-plane row is in flight or within race grace",
                display_id
            );
            continue;
        }
        // Post-defer-window reachability gate: this is the sister check to
        // the I023 ER-in-flight defer gate above. Once the defer window has
        // lifted, the row may have already advanced to a terminal state
        // (in_review, accepted, etc.) where mark_drive_failed is not schema-
        // reachable. Skip silently — this is not an error, it is the normal
        // outcome of a drive that completed successfully after the PID died.
        // Reachability is derived from the schema, not duplicated here.
        if !verb_reachable_from(&tasks_schema, cur_status, "mark_drive_failed") {
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
                let agent = agents.agents.iter().find(|a| a.name == "auto-drive");
                let _ =
                    mark_claim_silent_zombie(conn, "tasks", row_id, agent, "auto-drive", reason);
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

    struct EnvRestore {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match self.old.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
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
            "INSERT INTO tasks (display_id, status, title, slug, branch, tier_hint, workspace_path, contract, activation, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'planning', 'test', 'tslug', 'feat/tslug', 'T2', ?2, ?3, 'active', ?4, ?4, 'ai_autonomous', 'ai_autonomous')",
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

    #[test]
    fn vii_spawn_passes_canonical_stores_root_for_adapter_cwd() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        crate::paths::clear_stores_dir_override_for_tests();
        let conn = fresh_db();
        let worktree = temp_cwd();
        let canonical = tempfile::tempdir().unwrap();
        let canonical_stores = canonical.path().join(".stores");
        std::fs::create_dir(&canonical_stores).unwrap();
        crate::paths::set_stores_dir_override(canonical_stores).unwrap();

        let marker = worktree.path().join("stores-root-marker.txt");
        assert!(
            !worktree.path().join(".stores").exists(),
            "adapter cwd must not start with .stores/"
        );
        let cmd = r#"printf '%s' "$STORES_ROOT" > stores-root-marker.txt; sleep 30 #"#;
        std::env::set_var("STORES_DRIVE_CMD", cmd);

        insert_planning_task(&conn, "T706", worktree.path().to_str().unwrap());
        let row = task_row_json(&conn, "T706");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");

        let res = run(&row, &ctx_for(&conn, &agents, &cfg)).unwrap();
        assert_eq!(res, 0);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline && !marker.exists() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(marker.exists(), "marker must land under workspace_path cwd");
        let got = std::fs::read_to_string(&marker).unwrap();
        assert_eq!(got, canonical.path().to_string_lossy());

        let pid: i64 = conn
            .query_row(
                "SELECT COALESCE(drive_pid, 0) FROM tasks WHERE display_id='T706'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if pid > 0 {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        std::env::remove_var("STORES_DRIVE_CMD");
        crate::paths::clear_stores_dir_override_for_tests();
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
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, drive_pid, activation, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'test', 't', 'feat/x', '/tmp/no-such', ?3, ?4, 'active', ?5, ?5, 'framework', 'framework')",
            rusqlite::params![display_id, status, contract, drive_pid, now],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn dead_pid() -> i64 {
        // Pick a high-numbered PID overwhelmingly likely to be unallocated.
        0x7fff_fffe
    }

    #[test]
    fn watchdog_classifies_alive_but_stalled_runner() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _no_output = EnvRestore::set("STORES_RUNNER_NO_OUTPUT_SECS", "300");
        let _wall_clock = EnvRestore::set("STORES_RUNNER_WALL_CLOCK_MAX_SECS", "1800");

        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T143W", "planning", Some(std::process::id() as i64));
        insert_lock(&conn, row_id, "T143W");
        let stale = (super::now_epoch() - 600).to_string();
        conn.execute(
            "UPDATE dispatch_locks SET claimed_at=?1, heartbeat_at=?1 WHERE row_id=?2",
            rusqlite::params![stale, row_id],
        )
        .unwrap();
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
        let (terminal_reason, last_status): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT terminal_reason, last_status FROM dispatch_locks WHERE row_id=?1",
                rusqlite::params![row_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(terminal_reason.as_deref(), Some("silent_zombie"));
        assert!(last_status
            .unwrap_or_default()
            .starts_with("drive_failed:no_output_idle_"));
        let note: String = conn
            .query_row(
                "SELECT COALESCE(actor_note,'') FROM transition_history \
                 WHERE display_id='T143W' AND verb='mark_drive_failed' ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(note.starts_with("no_output_idle_"), "actor_note={note}");
    }

    /// AC5.1 (i): live PID + status='planning' → no flip, lock left open.
    #[test]
    fn watchdog_live_pid_no_flip() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let our_pid = std::process::id() as i64;
        let row_id = insert_task_full(&conn, "T720", "planning", Some(our_pid));
        insert_lock(&conn, row_id, "T720");
        conn.execute(
            "UPDATE dispatch_locks SET claimed_at=?1, heartbeat_at=?1 WHERE row_id=?2",
            rusqlite::params![super::now_epoch().to_string(), row_id],
        )
        .unwrap();

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
        assert_eq!(
            reason.as_deref(),
            Some("drive_failed:silent_zombie_pid_dead")
        );

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

    #[test]
    fn watchdog_mixed_state_open_locks_marks_only_actionable() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let rows = [
            ("T890", "planning"),
            ("T891", "executing"),
            ("T892", "code_review"),
            ("T893", "abandoned"),
            ("T894", "closed_out_of_band"),
            ("T895", "schema_migrated"),
            ("T896", "accepted"),
            ("T897", "cargo_installed"),
            ("T898", "deploy_blocked"),
            ("T899", "rejected"),
        ];
        for (display_id, status) in rows {
            let row_id = insert_task_full(&conn, display_id, status, Some(dead_pid()));
            insert_lock(&conn, row_id, display_id);
        }

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
        assert_eq!(acted, 3, "only actionable rows should be watchdog-actioned");

        let marked: Vec<String> = conn
            .prepare(
                "SELECT display_id FROM transition_history \
                 WHERE verb = 'mark_drive_failed' ORDER BY display_id",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(marked, vec!["T890", "T891", "T892"]);

        for display_id in ["T893", "T894", "T895", "T896", "T897", "T898", "T899"] {
            let status: String = conn
                .query_row(
                    "SELECT status FROM tasks WHERE display_id = ?1",
                    rusqlite::params![display_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(
                matches!(
                    status.as_str(),
                    "abandoned"
                        | "closed_out_of_band"
                        | "schema_migrated"
                        | "accepted"
                        | "cargo_installed"
                        | "deploy_blocked"
                        | "rejected"
                ),
                "terminal row {display_id} must not be mutated; got {status}"
            );
        }
    }

    /// I023 regression: dead PID + active `external_reviews` control-plane
    /// row → watchdog defers, does NOT flip to blocked. Concrete shape: the
    /// T098 ER333 race where `submit-external-review` (REVISE) and
    /// `mark_drive_failed` (silent_zombie) both fired in the same daemon
    /// poll tick at 2026-05-08T12:02:23Z, racing the auto-drive subscriber's
    /// new spawn. With the gate, the watchdog observes the active ER row
    /// and stands down so the review reconciler can drive the next transition.
    fn fresh_db_with_external_reviews() -> Connection {
        let conn = fresh_db_with_obs();
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "external_reviews")
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        conn
    }

    fn insert_external_review(
        conn: &Connection,
        display_id: &str,
        task_id: &str,
        status: &str,
        updated_at: &str,
    ) {
        conn.execute(
            "INSERT INTO external_reviews \
             (display_id, status, task_id, attempt, runner, created_at, updated_at, \
              created_by, updated_by) \
             VALUES (?1, ?2, ?3, 1, 'codex', ?4, ?4, 'framework', 'framework')",
            rusqlite::params![display_id, status, task_id, updated_at],
        )
        .unwrap();
    }

    #[test]
    fn watchdog_defers_mark_drive_failed_when_external_review_lane_is_active() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_external_reviews();
        let row_id = insert_task_full(&conn, "T720R", "executing", Some(dead_pid()));
        insert_lock(&conn, row_id, "T720R");
        // ER row in `running` — review run in flight.
        let now = crate::handlers::row::now_iso8601();
        insert_external_review(&conn, "ER720R", "T720R", "running", &now);

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T720R'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            status, "executing",
            "watchdog must defer when ER is running"
        );
        assert!(
            reason.as_deref().unwrap_or("").is_empty(),
            "no blocked_reason must be set: {:?}",
            reason
        );
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE display_id='T720R' AND verb='mark_drive_failed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "mark_drive_failed must not have fired");
    }

    #[test]
    fn watchdog_defers_when_external_review_just_returned_terminal_verdict() {
        // Race shape: ER returned REVISE within the grace window; the
        // submit-external-review verb has already advanced the task back to
        // `executing`, but the auto-drive subscriber has not yet spawned a
        // new drive. The dead drive_pid from the prior cycle is still on the
        // row. Watchdog must defer, not race.
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_external_reviews();
        let row_id = insert_task_full(&conn, "T720T", "executing", Some(dead_pid()));
        insert_lock(&conn, row_id, "T720T");
        let now = crate::handlers::row::now_iso8601();
        // Terminal verdict, just-now updated → within race grace window.
        insert_external_review(&conn, "ER720T", "T720T", "revise", &now);

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T720T'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "executing",
            "watchdog must defer within ER race grace window"
        );
    }

    #[test]
    fn watchdog_flips_when_external_review_terminal_is_stale() {
        // Companion to the deferral tests: confirm the deferral is BOUNDED.
        // An old terminal verdict (well outside the grace window) does NOT
        // shield the row from mark_drive_failed; the row flips as expected.
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
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

        let conn = fresh_db_with_external_reviews();
        let row_id = insert_task_full(&conn, "T720S", "executing", Some(dead_pid()));
        insert_lock(&conn, row_id, "T720S");
        // Stale terminal verdict (far in the past).
        insert_external_review(&conn, "ER720S", "T720S", "revise", "2024-01-01T00:00:00Z");

        let agents = AgentsYaml::default_empty();
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg_file, "", "").unwrap();

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T720S'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "blocked", "stale ER must not shield the row");
        assert_eq!(
            reason.as_deref(),
            Some("drive_failed:silent_zombie_pid_dead")
        );
    }

    /// A1-strict AC5.1 (iii): dead PID + status='in_review' + open lock →
    /// wrap_log is IGNORED; next_agent IS NOT NULL means work is still pending,
    /// so the watchdog REDISPATCHES rather than marking ok.
    ///
    /// wrap_log content (even non-empty) must not suppress redispatch.
    #[test]
    fn watchdog_dead_pid_in_review_redispatches_regardless_of_wrap_log() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("STORES_DRIVE_CMD", "sleep 5 #");
        let conn = fresh_db_with_obs();
        let tmp = temp_cwd();
        let row_id = insert_task_full(&conn, "T722", "in_review", Some(dead_pid()));
        // Set workspace_path so redispatch can succeed, and set non-empty wrap_log
        // to confirm it does NOT suppress the redispatch.
        conn.execute(
            "UPDATE tasks SET workspace_path=?1, drive_started_at='2026-01-01T00:00:00Z', \
             wrap_log=?2 WHERE display_id='T722'",
            rusqlite::params![
                tmp.path().to_str().unwrap(),
                r#"[{"executive_summary":"prior-run"}]"#
            ],
        )
        .unwrap();
        insert_lock(&conn, row_id, "T722");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
        assert_eq!(
            acted, 1,
            "watchdog must act on dead-PID in_review open lock"
        );

        // Row stays in_review (redispatch, not transition).
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T722'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "in_review");

        // Lock is in-flight (pending_next), NOT closed as 'ok'.
        let last: String = conn
            .query_row(
                "SELECT last_status FROM dispatch_locks WHERE row_id=?1",
                rusqlite::params![row_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            last, "in_flight:pending_next",
            "non-empty wrap_log must not suppress redispatch; next_agent IS NOT NULL is the signal"
        );

        // Clean up spawned process.
        let pid: i64 = conn
            .query_row(
                "SELECT COALESCE(drive_pid, 0) FROM tasks WHERE display_id='T722'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if pid > 0 {
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        std::env::remove_var("STORES_DRIVE_CMD");
    }

    /// A1-strict: in_review + dead PID + closed lock (terminal_reason='ok') +
    /// next_agent IS NOT NULL → redispatches. wrap_log state is irrelevant.
    #[test]
    fn watchdog_dead_pid_in_review_pending_wrap_redispatches() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("STORES_DRIVE_CMD", "sleep 5 #");
        let conn = fresh_db_with_obs();
        let tmp = temp_cwd();
        let row_id = insert_task_full(&conn, "T723W", "in_review", Some(dead_pid()));
        conn.execute("UPDATE tasks SET workspace_path=?1, drive_started_at='2026-01-01T00:00:00Z' WHERE display_id='T723W'", rusqlite::params![tmp.path().to_str().unwrap()]).unwrap();
        insert_lock_closed(&conn, row_id, "T723W");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
        assert_eq!(acted, 1);
        let (status, terminal_reason, finished_at, pid): (String, Option<String>, Option<String>, i64) = conn.query_row(
            "SELECT t.status, dl.terminal_reason, dl.finished_at, t.drive_pid FROM tasks t JOIN dispatch_locks dl ON dl.row_id=t.id WHERE t.display_id='T723W'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
        assert_eq!(status, "in_review");
        assert!(terminal_reason.is_none());
        assert!(finished_at.is_none());
        assert!(pid > 0 && pid != dead_pid());
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        std::env::remove_var("STORES_DRIVE_CMD");
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
        assert_eq!(
            reason.as_deref(),
            Some("drive_failed:silent_zombie_pid_dead")
        );
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
    // T067 r7 HIGH: force_close discriminator — watchdog must NOT re-dispatch
    // -----------------------------------------------------------------

    /// Insert a lock in the state written by `force_close_auto_drive_lock_ok`:
    /// `terminal_reason='ok'`, `last_status='ok:wrap_completed'`, `finished_at` SET.
    /// This simulates the real `tasks drive` binary calling force-close after wrap dispatch.
    fn insert_lock_force_closed(conn: &Connection, row_id: i64, display_id: &str) {
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, \
              last_status, finished_at, terminal_reason) \
             VALUES ('tasks', ?1, ?2, 'auto-drive', 1, '2026-05-03T00:00:00Z', 'test-claimer', \
                     'ok:wrap_completed', '2026-05-03T00:00:01Z', 'ok')",
            rusqlite::params![row_id, display_id],
        )
        .unwrap();
    }

    /// T067 r7 HIGH: after `force_close_auto_drive_lock_ok` fires (lock closed with
    /// `last_status='ok:wrap_completed'`), the watchdog pending-handoff sweep must
    /// NOT re-dispatch wrap. The closed lock proves the drive subprocess already handed
    /// wrap off; re-dispatch would fire wrap a second time.
    ///
    /// Contrast with `watchdog_dead_pid_in_review_pending_wrap_redispatches` which
    /// uses `last_status='ok'` (old handoff, drive died before force-close) and
    /// MUST re-dispatch.
    #[test]
    fn watchdog_force_closed_wrap_lock_no_redispatch() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("STORES_DRIVE_CMD", "sleep 5 #");
        let conn = fresh_db_with_obs();
        let tmp = temp_cwd();
        let row_id = insert_task_full(&conn, "T723F", "in_review", Some(dead_pid()));
        conn.execute(
            "UPDATE tasks SET workspace_path=?1, drive_started_at='2026-01-01T00:00:00Z' \
             WHERE display_id='T723F'",
            rusqlite::params![tmp.path().to_str().unwrap()],
        )
        .unwrap();
        // Insert a force-closed lock (last_status='ok:wrap_completed') — this is the
        // state produced by force_close_auto_drive_lock_ok after wrap dispatch.
        insert_lock_force_closed(&conn, row_id, "T723F");
        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
        // Watchdog must not act on force-closed locks.
        assert_eq!(
            acted, 0,
            "watchdog must NOT re-dispatch a force-closed (ok:wrap_completed) lock; \
             re-dispatch would fire wrap a second time"
        );
        // Lock must remain closed (finished_at still set, last_status unchanged).
        let (finished_at, last_status, terminal_reason): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT finished_at, last_status, terminal_reason FROM dispatch_locks \
                 WHERE display_id='T723F' AND agent_name='auto-drive'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(
            finished_at.is_some(),
            "force-closed lock must remain closed (finished_at non-null); got None"
        );
        assert_eq!(
            last_status.as_deref(),
            Some("ok:wrap_completed"),
            "last_status must stay 'ok:wrap_completed'; got {last_status:?}"
        );
        assert_eq!(
            terminal_reason.as_deref(),
            Some("ok"),
            "terminal_reason must stay 'ok'; got {terminal_reason:?}"
        );
        std::env::remove_var("STORES_DRIVE_CMD");
    }

    // -----------------------------------------------------------------
    // T030 Phase 1: silent-zombie reproductions (these MUST FAIL on main)
    // -----------------------------------------------------------------

    /// Insert a closed lock — `finished_at` is SET and `terminal_reason='ok'`,
    /// simulating `mark_claim_finished` already ran post-spawn (the L062 shape).
    fn insert_lock_closed(conn: &Connection, row_id: i64, display_id: &str) {
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, \
              last_status, finished_at, terminal_reason) \
             VALUES ('tasks', ?1, ?2, 'auto-drive', 1, '2026-05-03T00:00:00Z', 'test-claimer', \
                     'ok', '2026-05-03T00:00:01Z', 'ok')",
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
        assert_eq!(
            reason.as_deref(),
            Some("drive_failed:silent_zombie_pid_dead")
        );

        let (terminal_reason, last_status): (String, String) = conn
            .query_row(
                "SELECT terminal_reason, last_status FROM dispatch_locks WHERE row_id=?1",
                rusqlite::params![row_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(terminal_reason, "silent_zombie");
        assert_eq!(last_status, "drive_failed:silent_zombie_pid_dead");
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
    fn watchdog_dead_drive_pid_with_recent_heartbeat_is_deferred() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("STORES_RUNNER_NO_OUTPUT_SECS");
        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T732", "code_review", Some(dead_pid()));
        let now = now_iso8601();
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, claimed_at, heartbeat_at, claimed_by) \
             VALUES ('tasks', ?1, 'T732', 'auto-drive', 1, ?2, ?2, 'test-claimer')",
            rusqlite::params![row_id, now],
        )
        .unwrap();

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T732'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(acted, 0);
        assert_eq!(status, "code_review");
        assert_eq!(reason, None);
    }

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

        let zombie_re =
            regex::Regex::new(r"^drive_failed:(silent_zombie_pid_dead|pid_never_recorded)$")
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
            assert_eq!(
                note, want_suffix,
                "{id}: actor_note must match bare variant"
            );
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
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-04T00:00:00Z").unwrap();

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

    /// T050 P4 Task 4.3: scan_zombie_tasks excludes rows already marked as
    /// terminal_reason='silent_zombie' so the watchdog does not re-fire.
    #[test]
    fn t050_scan_zombie_tasks_skips_already_marked_silent_zombie() {
        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T754", "executing", Some(dead_pid()));
        insert_lock_closed(&conn, row_id, "T754");
        conn.execute(
            "UPDATE dispatch_locks SET terminal_reason = 'silent_zombie' WHERE row_id = ?1",
            rusqlite::params![row_id],
        )
        .unwrap();

        let rows = scan_zombie_tasks(&conn, "2026-05-02T00:00:00Z");
        assert!(
            rows.iter()
                .all(|(_, display_id, _, _, _)| display_id != "T754"),
            "already-marked silent_zombie locks must be excluded from watchdog scan"
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
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-02T00:00:00Z").unwrap();

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
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-04T00:00:00Z").unwrap();

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
        let _ = sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-02T00:00:00Z").unwrap();

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
            let mut stmt = conn
                .prepare("PRAGMA table_info(transition_history)")
                .unwrap();
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
            let mut stmt = conn
                .prepare("PRAGMA table_info(transition_history)")
                .unwrap();
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

    // -----------------------------------------------------------------
    // L141 -> L134: auto-drive's post-spawn lock-close is now gated by the
    // typed `drive_pid_recorded_or_terminal` postcondition. T049 originally
    // detected pre-submit zombies by leaving the lock OPEN and relying on a
    // watchdog grace window; T050's typed lifecycle replaces that with a
    // postcondition check inside `mark_claim_finished_typed`. On a healthy
    // spawn (drive_pid recorded on the tasks row) the lock closes typed-clean
    // (terminal_reason='ok'); on a pre-record death the postcondition fails
    // and the lock closes with terminal_reason='error', retry-eligible.
    // -----------------------------------------------------------------

    /// poll_once-driven test: after auto-drive successfully spawns its drive
    /// subprocess and the `drive_pid_recorded_or_terminal` postcondition
    /// passes, the dispatch_lock closes cleanly (finished_at set, terminal
    /// reason 'ok', postcondition_id stamped on the row).
    #[test]
    fn auto_drive_run_closes_lock_after_drive_pid_postcondition_passes() {
        use crate::flow::agents_yaml::TransitionEdge;
        use crate::flow::policies_yaml::PoliciesYaml;
        use crate::flow::{AgentEntry, BackoffKind, RetryPolicy, Subscription};
        use crate::handlers::agents_run::poll_once;

        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        // Long-running stub so the spawned drive is still alive when we check.
        std::env::set_var("STORES_DRIVE_CMD", "sleep 30 #");

        let conn = fresh_db_with_obs();
        let tmp = temp_cwd();
        insert_planning_task(&conn, "T780", tmp.path().to_str().unwrap());
        let row_id: i64 = conn
            .query_row("SELECT id FROM tasks WHERE display_id='T780'", [], |r| {
                r.get(0)
            })
            .unwrap();
        // History row that fires the ''→'planning' subscription.
        conn.execute(
            "INSERT INTO transition_history \
             (store, row_id, display_id, from_status, to_status, verb, \
              invoker, occurred_at) \
             VALUES ('tasks', ?1, 'T780', '', 'planning', 'submit', \
                     'ai_autonomous', '2026-05-03T00:00:00Z')",
            rusqlite::params![row_id],
        )
        .unwrap();

        let agent = AgentEntry {
            name: "auto-drive".to_string(),
            subscribes_to: vec![Subscription {
                store: "tasks".to_string(),
                transition: TransitionEdge {
                    from: "".to_string(),
                    to: "planning".to_string(),
                },
                integration_step: None,
                predicate: None,
            }],
            command: "builtin:auto-drive".to_string(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy {
                max_attempts: 3,
                backoff: BackoffKind::Linear,
            },
            command_args: None,
        };
        let agents = AgentsYaml {
            agents: vec![agent],
            deployment_specialist: None,
        };
        let policies = PoliciesYaml {
            hash: String::new(),
            policies: vec![],
        };
        let cfg = std::path::PathBuf::from("/tmp/stores-test-no-config.yaml");

        let n = poll_once(&conn, &agents, &policies, &cfg, "test-claimer", "")
            .expect("poll_once must succeed");
        assert_eq!(n, 1, "exactly one auto-drive dispatch must fire");

        // T067/L134 invariant: initial auto-drive spawn leaves a pending
        // next_agent (planner) and therefore the lock stays in-flight rather
        // than terminal_reason='ok'.
        type LockRow = (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        );
        let (finished_at, last_status, terminal_reason, postcondition_id, drive_pid): LockRow = conn
            .query_row(
                "SELECT dl.finished_at, dl.last_status, dl.terminal_reason, dl.postcondition_id, t.drive_pid \
                 FROM dispatch_locks dl JOIN tasks t ON t.id = dl.row_id \
                 WHERE t.display_id='T780' AND dl.agent_name='auto-drive'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert!(
            finished_at.is_none(),
            "T067: pending next_agent must keep auto-drive lock in-flight (got finished_at={:?})",
            finished_at
        );
        assert_eq!(
            last_status.as_deref(),
            Some("in_flight:pending_next"),
            "T067: last_status records pending handoff",
        );
        assert!(
            terminal_reason.is_none(),
            "T067: terminal_reason must remain NULL while next_agent is pending",
        );
        assert_eq!(
            postcondition_id.as_deref(),
            Some("drive_pid_recorded_or_terminal"),
            "L134: postcondition_id stamped on the lock row",
        );

        // Reap the stub.
        if let Some(p) = drive_pid {
            unsafe {
                libc::kill(p as i32, libc::SIGTERM);
            }
        }
        std::env::remove_var("STORES_DRIVE_CMD");
    }

    // -----------------------------------------------------------------
    // L511: verb-reachability gate — zombie pass must not attempt
    // mark_drive_failed on rows where the verb is not schema-reachable.
    // -----------------------------------------------------------------

    /// (a) Row at status=in_review with a dead drive_pid and no in-flight ER:
    /// the reachability gate must fire, leaving status unchanged and writing
    /// zero transition_history entries for mark_drive_failed.
    #[test]
    fn watchdog_zombie_skips_mark_drive_failed_when_not_reachable_from_in_review() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T790", "in_review", Some(dead_pid()));
        // Use last_status='ok:wrap_completed' to exclude this lock from the
        // pending-locks re-dispatch pass, which filters out completed wrap
        // locks. We only want to exercise the zombie pass reachability gate.
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, \
              last_status, finished_at, terminal_reason) \
             VALUES ('tasks', ?1, ?2, 'auto-drive', 1, '2026-05-03T00:00:00Z', 'test-claimer', \
                     'ok:wrap_completed', '2026-05-03T00:00:01Z', 'ok')",
            rusqlite::params![row_id, "T790"],
        )
        .unwrap();

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        // daemon_epoch predates the lock so the epoch gate allows the row through.
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-02T00:00:00Z").unwrap();
        assert_eq!(acted, 0, "no action must be taken for in_review row");

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T790'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "in_review", "status must remain in_review");

        let th_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE display_id='T790' AND verb='mark_drive_failed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            th_count, 0,
            "no mark_drive_failed transition must land for in_review row; got {th_count}"
        );
    }

    /// (b) Row at status=executing with a dead drive_pid: mark_drive_failed
    /// must still fire, preserving the existing L186 detection behavior.
    #[test]
    fn watchdog_zombie_still_fires_mark_drive_failed_from_executing() {
        let _g = crate::flow::builtins::tests::lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T791", "executing", Some(dead_pid()));
        insert_lock_closed(&conn, row_id, "T791");

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "2026-05-02T00:00:00Z").unwrap();
        assert_eq!(acted, 1, "executing zombie must be actioned");

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T791'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "blocked", "executing zombie must flip to blocked");

        let th_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE display_id='T791' AND verb='mark_drive_failed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(th_count, 1, "exactly one mark_drive_failed must land");
    }

    /// (c) `verb_reachable_from` handles unknown statuses gracefully: returns
    /// false (does not panic), ensuring the gate never blocks legitimate
    /// dispatch even if a future status appears in a zombie scan row.
    #[test]
    fn verb_reachable_from_graceful_on_unknown_status() {
        let schema = load_tasks_schema().unwrap();
        // Unknown status → not reachable; must not panic.
        assert!(
            !verb_reachable_from(&schema, "future_unknown_status", "mark_drive_failed"),
            "unknown status must not be mark_drive_failed-reachable"
        );
        // Known in-cycle status → reachable (ensures the helper returns true
        // for legitimate cases, not just silently false for everything).
        assert!(
            verb_reachable_from(&schema, "executing", "mark_drive_failed"),
            "executing must be mark_drive_failed-reachable"
        );
        assert!(
            verb_reachable_from(&schema, "planning", "mark_drive_failed"),
            "planning must be mark_drive_failed-reachable"
        );
    }

    // -----------------------------------------------------------------
    // Stale-binary-inode watchdog tests (Tasks 1.6, 1.7, 1.8)
    // -----------------------------------------------------------------

    /// Pure-string test: stale_exe_log_line emits drive_pid, proc path, and
    /// stale_binary_inode on a single line. No cfg gate — pure function.
    #[test]
    fn stale_exe_log_line_carries_pid_and_proc_path() {
        let s = stale_exe_log_line("T999", 12345);
        assert!(
            s.contains("stale_binary_inode"),
            "log line must contain stale_binary_inode; got: {s}"
        );
        assert!(
            s.contains("drive_pid=12345"),
            "log line must contain drive_pid=12345; got: {s}"
        );
        assert!(
            s.contains("/proc/12345/exe"),
            "log line must contain /proc/12345/exe; got: {s}"
        );
    }

    /// Teardown guard: sends SIGKILL to the wrapped PID on drop so spawned
    /// children never leak past a test.
    #[cfg(target_os = "linux")]
    struct KillOnDrop(u32);

    #[cfg(target_os = "linux")]
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            unsafe {
                libc::kill(self.0 as i32, libc::SIGKILL);
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn spawn_sleep_copy(path: &std::path::Path) -> std::process::Child {
        let mut last_err = None;
        for _ in 0..20 {
            match std::process::Command::new(path).arg("30").spawn() {
                Ok(child) => return child,
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(e) => panic!("spawn sleep copy: {e}"),
            }
        }
        panic!("spawn sleep copy: {}", last_err.unwrap());
    }

    /// Alive drive PID whose exe inode is deleted is post-spawn binary drift,
    /// not a drive failure. The daemon/control-plane still fail-louds before
    /// spawning stale work, but the watchdog must not block an already-running
    /// task solely because its launch-path inode was replaced by install.
    #[cfg(target_os = "linux")]
    #[test]
    fn watchdog_alive_pid_with_deleted_exe_is_advisory_no_block() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());

        let tmp = tempfile::tempdir().unwrap();
        let sleep_copy = tmp.path().join("sleep_copy");
        std::fs::copy("/bin/sleep", &sleep_copy).expect("copy /bin/sleep");

        let mut child = spawn_sleep_copy(&sleep_copy);
        let child_pid = child.id();
        let _guard = KillOnDrop(child_pid);

        std::fs::remove_file(&sleep_copy).expect("remove sleep copy");

        // Poll until the kernel marks /proc/<pid>/exe with " (deleted)".
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            if crate::handlers::agents_run::drive_pid_exe_is_stale(child_pid as i32) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "drive_pid_exe_is_stale never returned true within 1s"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T795", "executing", Some(child_pid as i64));
        insert_lock(&conn, row_id, "T795");
        conn.execute(
            "UPDATE dispatch_locks SET claimed_at=?1, heartbeat_at=?1 WHERE row_id=?2",
            rusqlite::params![super::now_epoch().to_string(), row_id],
        )
        .unwrap();

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
        assert_eq!(
            acted, 0,
            "watchdog must not block an already-running stale-exe PID"
        );

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T795'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "executing");
        assert_eq!(
            reason.as_deref(),
            None,
            "stale installed binary must not write drive_failed:stale_binary_inode"
        );

        let (finished_at, terminal_reason): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT finished_at, terminal_reason FROM dispatch_locks WHERE row_id=?1",
                rusqlite::params![row_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(
            finished_at.is_none(),
            "lock must remain open while the drive PID is alive"
        );
        assert_eq!(
            terminal_reason.as_deref(),
            None,
            "alive stale-exe drift must not close as silent_zombie"
        );

        let transition_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE display_id='T795' AND verb='mark_drive_failed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            transition_count, 0,
            "alive stale-exe drift must not write mark_drive_failed transition"
        );

        // Reap: kill explicitly so wait() returns immediately; guard is backup.
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Alive stale-exe drift is advisory only, but it must not make the drive
    /// immune to the normal alive-PID liveness watchdog. If heartbeat/claimed_at
    /// are stale beyond the no-output threshold, the row should fail with the
    /// normal no_output reason, never stale_binary_inode.
    #[cfg(target_os = "linux")]
    #[test]
    fn watchdog_alive_deleted_exe_with_stale_heartbeat_fails_normal_liveness() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let _no_output = EnvRestore::set("STORES_RUNNER_NO_OUTPUT_SECS", "300");
        let _wall_clock = EnvRestore::set("STORES_RUNNER_WALL_CLOCK_MAX_SECS", "1800");

        let tmp = tempfile::tempdir().unwrap();
        let sleep_copy = tmp.path().join("sleep_copy_stalled");
        std::fs::copy("/bin/sleep", &sleep_copy).expect("copy /bin/sleep");

        let mut child = spawn_sleep_copy(&sleep_copy);
        let child_pid = child.id();
        let _guard = KillOnDrop(child_pid);

        std::fs::remove_file(&sleep_copy).expect("remove sleep copy");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            if crate::handlers::agents_run::drive_pid_exe_is_stale(child_pid as i32) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "drive_pid_exe_is_stale never returned true within 1s"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T797", "executing", Some(child_pid as i64));
        insert_lock(&conn, row_id, "T797");
        let stale = (super::now_epoch() - 600).to_string();
        conn.execute(
            "UPDATE dispatch_locks SET claimed_at=?1, heartbeat_at=?1 WHERE row_id=?2",
            rusqlite::params![stale, row_id],
        )
        .unwrap();

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
        assert_eq!(
            acted, 1,
            "stale exe must not mask normal stalled/no-output liveness failure"
        );

        let (status, reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T797'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "blocked");
        let reason = reason.expect("blocked_reason must be written");
        assert!(
            reason.starts_with("drive_failed:no_output_idle_"),
            "expected normal liveness reason, got {reason}"
        );
        assert!(
            !reason.contains("stale_binary_inode"),
            "stale exe must not be the failure reason: {reason}"
        );

        let (finished_at, terminal_reason, last_status): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT finished_at, terminal_reason, last_status FROM dispatch_locks WHERE row_id=?1",
                rusqlite::params![row_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(finished_at.is_some(), "stalled lock should be closed");
        assert_eq!(terminal_reason.as_deref(), Some("silent_zombie"));
        assert!(
            last_status.unwrap_or_default().starts_with("drive_failed:no_output_idle_"),
            "last_status should record normal liveness failure"
        );

        let note: String = conn
            .query_row(
                "SELECT COALESCE(actor_note,'') FROM transition_history \
                 WHERE display_id='T797' AND verb='mark_drive_failed' ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            note.starts_with("no_output_idle_"),
            "actor_note must be normal liveness detail, got {note}"
        );
        assert!(
            !note.contains("stale_binary_inode"),
            "actor_note must not use stale_binary_inode: {note}"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    /// AC1.3: alive drive PID whose exe is NOT deleted → watchdog leaves the
    /// row untouched (acted == 0, status still 'executing').
    #[cfg(target_os = "linux")]
    #[test]
    fn watchdog_alive_pid_with_fresh_exe_no_flip() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());

        let tmp = tempfile::tempdir().unwrap();
        let sleep_copy = tmp.path().join("sleep_fresh");
        std::fs::copy("/bin/sleep", &sleep_copy).expect("copy /bin/sleep");

        let mut child = spawn_sleep_copy(&sleep_copy);
        let child_pid = child.id();
        let _guard = KillOnDrop(child_pid);

        // Deliberately do NOT delete the copy.

        let conn = fresh_db_with_obs();
        let row_id = insert_task_full(&conn, "T796", "executing", Some(child_pid as i64));
        insert_lock(&conn, row_id, "T796");
        conn.execute(
            "UPDATE dispatch_locks SET claimed_at=?1, heartbeat_at=?1 WHERE row_id=?2",
            rusqlite::params![super::now_epoch().to_string(), row_id],
        )
        .unwrap();

        let agents = AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted = sweep_drive_watchdog(&conn, &agents, &cfg, "", "").unwrap();
        assert_eq!(acted, 0, "fresh-exe alive PID must not be touched");

        let (status, blocked_reason): (String, Option<String>) = conn
            .query_row(
                "SELECT status, blocked_reason FROM tasks WHERE display_id='T796'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "executing", "row must remain executing");
        assert!(blocked_reason.is_none(), "blocked_reason must be NULL");

        let finished_at: Option<String> = conn
            .query_row(
                "SELECT finished_at FROM dispatch_locks WHERE row_id=?1",
                rusqlite::params![row_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(finished_at.is_none(), "lock must remain open");

        let _ = child.kill();
        let _ = child.wait();
    }
}
