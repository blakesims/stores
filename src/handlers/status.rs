/// `status` handler — workflow telemetry frames for live tailing.
///
/// # `status` vs `show` distinction
///
/// `stores tasks show <id>` (existing v0.2 verb) prints the **full task row** —
/// every column, as a debug dump.  `stores tasks status <id>` prints a
/// **compact workflow telemetry frame**: a one-liner summarising
/// `current_phase / current_cycle / status / next-action / blocked`.  They are
/// not redundant: `show` is a debug dump; `status --follow` is a live tail.
///
/// # Frame format (single-task)
/// ```text
/// [HH:MM:SS] T001 status=executing phase=2/3 cycle=1 next=executor blocked=false
/// ```
///
/// # Frame format (multi-task, no id)
/// ```text
/// [HH:MM:SS]
///   T001 status=executing phase=2/3 cycle=1 next=executor blocked=false
///   T002 status=plan_review phase=-/- cycle=- next=plan-reviewer blocked=false
/// ```
///
/// # AC5.5 dedup
/// A state key `(status, current_phase, current_cycle, blocked_reason)` is
/// hashed per task.  If unchanged from the prior frame, the line is suppressed.
/// The first frame is always printed.
///
/// # AC5.4 Ctrl-C
/// SIGINT is caught via a static `AtomicBool`.  The last printed frame stays on
/// screen; the loop exits with code 130.
///
/// # AC5.6 bounded testing
/// `--max-iters` (hidden) caps loop iterations; tests pass a small value to
/// avoid sleeping.
use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::db;
use crate::handlers::disposition::{operator_disposition, GitBranchStateSource};
use crate::paths::db_path;

// ---------------------------------------------------------------------------
// SIGINT flag — set by signal handler, polled by the loop
// ---------------------------------------------------------------------------

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Install a SIGINT handler that sets `INTERRUPTED`.  Called once on entry.
/// Safe to call multiple times (idempotent for tests).
fn install_sigint_handler() {
    // Use the `signal` syscall via libc.  This is the only direct `unsafe` block.
    unsafe {
        libc::signal(
            libc::SIGINT,
            sigint_handler as *const () as libc::sighandler_t,
        );
    }
}

extern "C" fn sigint_handler(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// Public args struct
// ---------------------------------------------------------------------------

pub struct StatusArgs {
    /// Optional display ID.  If None → multi-task mode.
    pub display_id: Option<String>,
    /// If true, poll in a loop until terminal.
    pub follow: bool,
    /// Interval between frames in milliseconds.
    pub interval_ms: u64,
    /// Maximum iterations (for testing; default usize::MAX).
    pub max_iters: usize,
}

// ---------------------------------------------------------------------------
// Task state row (lightweight — no EntryMap needed)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct TaskState {
    pub display_id: String,
    pub status: String,
    pub current_phase: Option<i64>,
    pub total_phases: Option<i64>,
    pub current_cycle: Option<i64>,
    pub blocked_reason: Option<String>,
    pub lifecycle: Option<String>,
    pub active_step: Option<String>,
    pub integration_step: Option<String>,
}

/// Dedup key: hash the fields that define "same state".
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StateKey {
    pub status: String,
    pub current_phase: Option<i64>,
    pub current_cycle: Option<i64>,
    pub blocked_reason: Option<String>,
}

impl TaskState {
    pub fn state_key(&self) -> StateKey {
        StateKey {
            status: self.status.clone(),
            current_phase: self.current_phase,
            current_cycle: self.current_cycle,
            blocked_reason: self.blocked_reason.clone(),
        }
    }
}

/// Determine `next` agent label from status string (best-effort, no schema needed).
fn next_from_status(status: &str) -> &'static str {
    match status {
        "planning" => "planner",
        "plan_review" => "plan-reviewer",
        "ready" | "executing" => "executor",
        "code_review" => "code-reviewer",
        "complete" => "wrap",
        "in_review" => "wrap",
        "integration_queued" | "integrating" => "integrate",
        "integrated" => "post-integrated",
        "integration_blocked" => "-",
        "accepted" => "-",
        "rejected" => "-",
        "blocked" => "-",
        _ => "?",
    }
}

/// Is this status a truly-terminal state (no further progress without human action)?
///
/// `accepted`, `rejected`, and `abandoned` are terminal/history end-states.
/// `in_review` and `blocked` are NOT terminal but ARE "awaiting human" — `status follow`
/// can safely stop on them via `is_awaiting_human`. `complete` is transient (the
/// `complete → in_review` follow-on fires in the same tx; it should never be observable
/// as a resting state).
fn is_terminal(status: &str) -> bool {
    // accepted: human signed off — nothing more to do.
    // rejected: human said no — requires amend, which is a human decision.
    // abandoned: intentionally retired — no further workflow action.
    matches!(
        status,
        "accepted" | "rejected" | "abandoned" | "closed_out_of_band" | "schema_migrated"
    )
}

/// Active integration-lane states (queued/in-progress). These are still in
/// flight from a monitoring standpoint and should NOT cause `status follow`
/// to exit.
pub fn is_integration_active(status: &str) -> bool {
    matches!(status, "integration_queued" | "integrating")
}

/// Is this status one where drive (or `status follow`) should pause for human input?
/// Superset of `is_terminal`; includes states that are awaiting a human action but
/// could theoretically continue automatically (e.g. after `stores tasks accept`).
///
/// `integration_blocked` mirrors `blocked` / `deploy_blocked` — it requires a
/// human-authorized retry (`tasks retry-integration`) so the follow loop pauses.
fn is_awaiting_human(status: &str) -> bool {
    status == "blocked"
        || status == "in_review"
        || status == "integration_blocked"
        || is_terminal(status)
}

/// Returns true when `integrated` should be treated as terminal for the
/// bounded-follow loop — i.e. when no agents.yaml subscriber is wired for the
/// `integrated` handoff. This includes subscribers on the edge into integrated
/// (`integrating → integrated`, e.g. cargo-install) as well as subscribers wired
/// off `from: integrated` (e.g. schema-migrate). Used by `run_follow_loop` to
/// decide whether to exit when a single-task follow lands on `integrated`.
///
/// Looks for `agents.yaml` next to the supplied DB path (stores_dir layout).
fn integrated_is_terminal_no_post_subscriber(db: &Path) -> bool {
    let dir = match db.parent() {
        Some(d) => d,
        None => return true,
    };
    let path = dir.join("agents.yaml");
    if !path.exists() {
        return true;
    }
    match crate::flow::agents_yaml::load_from_path(&path) {
        Ok(cfg) => !cfg
            .agents
            .iter()
            .flat_map(|a| a.subscribes_to.iter())
            .any(|s| {
                s.store == "tasks"
                    && (s.transition.from == "integrated" || s.transition.to == "integrated")
            }),
        Err(_) => true,
    }
}

// ---------------------------------------------------------------------------
// Frame formatting
// ---------------------------------------------------------------------------

/// Current wall-clock time as `HH:MM:SS` (UTC).
fn hms_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Format a single task line (no timestamp prefix).
pub fn format_task_line(task: &TaskState) -> String {
    let phase_str = match (task.current_phase, task.total_phases) {
        (Some(p), Some(t)) => format!("{p}/{t}"),
        (Some(p), None) => format!("{p}/-"),
        _ => "-/-".to_string(),
    };
    let cycle_str = match task.current_cycle {
        Some(c) => c.to_string(),
        None => "-".to_string(),
    };
    let next = next_from_status(&task.status);
    let blocked = crate::handlers::is_blocked(&task.status, task.blocked_reason.as_deref());
    format!(
        "{id} status={status} lifecycle={lifecycle} active_step={active_step} integration_step={integration_step} phase={phase} cycle={cycle} next={next} blocked={blocked}",
        id = task.display_id,
        status = task.status,
        lifecycle = task.lifecycle.as_deref().unwrap_or("-"),
        active_step = task.active_step.as_deref().unwrap_or("-"),
        integration_step = task.integration_step.as_deref().unwrap_or("-"),
        phase = phase_str,
        cycle = cycle_str,
        next = next,
        blocked = blocked,
    )
}

/// Format a single-task frame.
pub fn compute_status_frame(task: &TaskState) -> String {
    format!("[{}] {}", hms_now(), format_task_line(task))
}

/// Format a multi-task frame.
pub fn compute_multi_frame(tasks: &[TaskState]) -> String {
    let ts = hms_now();
    if tasks.is_empty() {
        return format!("[{ts}] (no non-terminal tasks)");
    }
    let mut lines = vec![format!("[{ts}]")];
    for t in tasks {
        lines.push(format!("  {}", format_task_line(t)));
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// DB fetchers
// ---------------------------------------------------------------------------

fn task_projection_exprs(conn: &Connection) -> Result<(String, String, String)> {
    let cols: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(tasks)")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let expr = |name: &str| {
        if cols.iter().any(|c| c == name) {
            name.to_string()
        } else {
            "NULL".to_string()
        }
    };
    Ok((expr("lifecycle"), expr("active_step"), expr("integration_step")))
}

/// Fetch a single task row by display_id from an open connection.
pub fn fetch_task(conn: &Connection, display_id: &str) -> Result<TaskState> {
    let (lifecycle_expr, active_step_expr, integration_step_expr) = task_projection_exprs(conn)?;
    let sql = format!(
        "SELECT display_id, status, current_phase, current_cycle, blocked_reason, plan, {lifecycle_expr}, {active_step_expr}, {integration_step_expr} \
         FROM tasks WHERE display_id = ?1"
    );
    let row = conn.query_row(
        &sql,
        rusqlite::params![display_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
            ))
        },
    );
    match row {
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            bail!("no task with display_id '{display_id}'")
        }
        Err(e) => bail!("db error: {e}"),
        Ok((
            id,
            status,
            current_phase,
            current_cycle,
            blocked_reason,
            plan_json,
            lifecycle,
            active_step,
            integration_step,
        )) => {
            let total_phases = plan_json
                .as_deref()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
                .and_then(|v| {
                    v.get("phases")
                        .and_then(|p| p.as_array())
                        .map(|a| a.len() as i64)
                });
            Ok(TaskState {
                display_id: id,
                status,
                current_phase,
                total_phases,
                current_cycle,
                blocked_reason,
                lifecycle,
                active_step,
                integration_step,
            })
        }
    }
}

/// Fetch all non-terminal task rows ordered by created_at.
///
/// Excludes truly-terminal states (`accepted`, `rejected`, `abandoned`) from the active view.
/// `complete` is transient and appears here if a row is somehow stuck mid-follow-on.
/// `blocked` and `in_review` ARE included — they are awaiting human input but are
/// still "active" from a monitoring standpoint.
pub fn fetch_all_tasks(conn: &Connection) -> Result<Vec<TaskState>> {
    let (lifecycle_expr, active_step_expr, integration_step_expr) = task_projection_exprs(conn)?;
    let sql = format!(
        "SELECT display_id, status, current_phase, current_cycle, blocked_reason, plan, {lifecycle_expr}, {active_step_expr}, {integration_step_expr} \
         FROM tasks \
         WHERE status NOT IN ('accepted', 'rejected', 'abandoned', 'closed_out_of_band', 'schema_migrated') \
         ORDER BY created_at ASC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
            row.get::<_, Option<String>>(8)?,
        ))
    })?;
    let mut result = Vec::new();
    for row in rows {
        let (
            id,
            status,
            current_phase,
            current_cycle,
            blocked_reason,
            plan_json,
            lifecycle,
            active_step,
            integration_step,
        ) = row?;
        let total_phases = plan_json
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| {
                v.get("phases")
                    .and_then(|p| p.as_array())
                    .map(|a| a.len() as i64)
            });
        result.push(TaskState {
            display_id: id,
            status,
            current_phase,
            total_phases,
            current_cycle,
            blocked_reason,
            lifecycle,
            active_step,
            integration_step,
        });
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// AC5.5: change detection predicate
// ---------------------------------------------------------------------------

/// Returns true if the frame should be printed for this task.
/// Always true on first frame (prev_key is None); true when key changed.
pub fn should_print(prev_key: Option<&StateKey>, new_key: &StateKey) -> bool {
    match prev_key {
        None => true,
        Some(prev) => prev != new_key,
    }
}

// ---------------------------------------------------------------------------
// T140 P5: disposition surfacing
// ---------------------------------------------------------------------------

/// Fetch the row JSON shape `operator_disposition` consumes for a single
/// task, tolerant to legacy schemas that omit the `activation` /
/// `accepted_at` columns. The returned JSON always contains
/// `display_id`, `status`, `activation`, `branch`, and `linked_observations`
/// keys; `accepted_at` is included when discoverable from
/// `transition_history`.
fn fetch_disposition_row_json(conn: &Connection, display_id: &str) -> Result<Value> {
    let cols: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(tasks)")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let activation_expr = if cols.iter().any(|c| c == "activation") {
        "COALESCE(activation,'inactive')"
    } else {
        "'inactive'"
    };
    let branch_expr = if cols.iter().any(|c| c == "branch") {
        "COALESCE(branch,'')"
    } else {
        "''"
    };
    let sql =
        format!("SELECT id, {activation_expr}, {branch_expr} FROM tasks WHERE display_id = ?1");
    let (row_id, activation, branch): (i64, String, String) =
        conn.query_row(&sql, rusqlite::params![display_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;

    let mut row_json = json!({
        "display_id": display_id,
        "activation": activation,
        "branch": branch,
        "linked_observations": [],
    });

    // accepted_at is recovered from transition_history (matches engine.rs
    // load_accepted_at_map). Absent / missing transition_history → leave unset.
    if let Ok(true) = table_exists(conn, "transition_history") {
        let at: Option<String> = conn
            .query_row(
                "SELECT MAX(occurred_at) FROM transition_history \
                 WHERE store='tasks' AND row_id=?1 AND to_status='accepted'",
                rusqlite::params![row_id],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap_or(None);
        if let Some(at) = at {
            row_json["accepted_at"] = json!(at);
        }
    }

    Ok(row_json)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        rusqlite::params![table],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

fn wall_clock_today() -> DateTime<Utc> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or_else(|| {
        DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is a valid DateTime")
    })
}

/// Compute and print the `Activation:` and `Disposition:` lines that
/// surface `handlers::disposition::operator_disposition` to the operator.
/// Single source of truth — no disposition-keyword strings appear here;
/// the rendered label is always sourced from `Disposition::display_label`.
fn print_task_disposition(conn: &Connection, task: &TaskState, status: &str) {
    let row_json = match fetch_disposition_row_json(conn, &task.display_id) {
        Ok(mut v) => {
            // Prefer the live status from TaskState (the row we just fetched)
            // over re-querying.
            v["status"] = json!(status);
            v
        }
        Err(_) => return,
    };
    let activation = row_json
        .get("activation")
        .and_then(|v| v.as_str())
        .unwrap_or("inactive")
        .to_string();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let branch_state = GitBranchStateSource::new(cwd, "main");
    let disposition = operator_disposition(&row_json, wall_clock_today(), &branch_state);
    println!("Activation: {activation}");
    println!("Disposition: {}", disposition.display_label());
}

fn format_age_from_rfc3339(updated_at: &str, now: DateTime<Utc>) -> Option<String> {
    let ts = DateTime::parse_from_rfc3339(updated_at)
        .ok()?
        .with_timezone(&Utc);
    let delta = now.signed_duration_since(ts);
    let std_delta = if delta.num_seconds() < 0 {
        Duration::from_secs(0)
    } else {
        delta.to_std().ok()?
    };
    let secs = std_delta.as_secs();
    if secs < 60 {
        Some(format!("{secs}s ago"))
    } else if secs < 60 * 60 {
        Some(format!("{}m ago", secs / 60))
    } else if secs < 24 * 60 * 60 {
        Some(format!("{}h ago", secs / 3600))
    } else {
        Some(format!("{}d ago", secs / 86_400))
    }
}

fn live_runner_lines(
    stores_dir: &Path,
    current: &crate::cli::runs::CurrentRun,
    now: DateTime<Utc>,
) -> Vec<String> {
    let marker = &current.marker;
    let runner = marker.runner.as_deref().unwrap_or("?");
    let status = marker.status.as_deref().unwrap_or("?");
    let updated = marker
        .updated_at
        .as_deref()
        .and_then(|u| format_age_from_rfc3339(u, now).map(|age| format!(" updated={age}")));
    let semantic = crate::cli::runs::read_current_status(stores_dir, current)
        .ok()
        .flatten();
    let semantic_age = semantic
        .as_ref()
        .and_then(|s| s.last_event_at.as_deref())
        .and_then(|u| format_age_from_rfc3339(u, now).map(|age| format!(" last_event={age}")));
    let activity = semantic
        .as_ref()
        .and_then(|s| s.current_activity.as_deref())
        .map(|a| format!(" activity={a}"));
    let event_type = semantic
        .as_ref()
        .and_then(|s| s.last_event_type.as_deref())
        .map(|t| format!(" event={t}"));
    let mut lines = vec![format!(
        "Live runner: role={} runner={} status={}{}{}{}{}",
        marker.role,
        runner,
        status,
        updated.unwrap_or_default(),
        semantic_age.unwrap_or_default(),
        activity.unwrap_or_default(),
        event_type.unwrap_or_default()
    )];
    if let Some(path) = &marker.transcript_path {
        lines.push(format!(
            "Live stdout: {}",
            crate::cli::runs::resolve_marker_path(stores_dir, &current.marker_path, path).display()
        ));
    }
    if let Some(path) = &marker.stderr_log_path {
        lines.push(format!(
            "Live stderr: {}",
            crate::cli::runs::resolve_marker_path(stores_dir, &current.marker_path, path).display()
        ));
    }
    lines
}

fn print_live_runner_status(db: &Path, task: &TaskState) {
    let Some(stores_dir) = db.parent() else {
        return;
    };
    let Ok(current) = crate::cli::runs::find_current_run(stores_dir, &task.display_id, None) else {
        return;
    };
    for line in live_runner_lines(stores_dir, &current, wall_clock_today()) {
        println!("{line}");
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Entry point called from dispatch.rs.
pub fn run_status(args: StatusArgs) -> Result<()> {
    install_sigint_handler();

    let db = db_path()?;

    if args.follow {
        run_follow_loop(&db, args)
    } else {
        // Single-frame mode (AC5.1 / AC5.3 one-shot)
        let conn = db::open(&db)?;
        match &args.display_id {
            Some(id) => {
                let task = fetch_task(&conn, id)?;
                println!("{}", compute_status_frame(&task));
                // T140 P5: surface the operator_disposition label so the
                // reader does not need to mentally re-derive it from raw
                // status. Single source of truth — display_label() lives
                // in handlers::disposition.
                print_task_disposition(&conn, &task, &task.status);
                print_live_runner_status(&db, &task);
            }
            None => {
                let tasks = fetch_all_tasks(&conn)?;
                println!("{}", compute_multi_frame(&tasks));
            }
        }
        Ok(())
    }
}

/// Polling follow loop (AC5.2 / AC5.3 / AC5.4 / AC5.5 / AC5.6).
fn run_follow_loop(db: &Path, args: StatusArgs) -> Result<()> {
    // Per-task dedup map: display_id → last printed StateKey
    let mut prev_keys: HashMap<String, StateKey> = HashMap::new();

    let interval = std::time::Duration::from_millis(args.interval_ms);

    for _iter in 0..args.max_iters {
        // Check Ctrl-C (AC5.4)
        if INTERRUPTED.load(Ordering::SeqCst) {
            std::process::exit(130);
        }

        let conn = db::open(db)?;

        match &args.display_id {
            Some(id) => {
                let task = fetch_task(&conn, id)?;
                let key = task.state_key();
                if should_print(prev_keys.get(id), &key) {
                    println!("{}", compute_status_frame(&task));
                    print_task_disposition(&conn, &task, &task.status);
                    print_live_runner_status(db, &task);
                    prev_keys.insert(id.clone(), key.clone());
                }
                if is_awaiting_human(&task.status) {
                    return Ok(());
                }
                // `integrated` is the framework-terminal lane state; only exit
                // here when no post-integrated subscriber is wired (otherwise
                // the row will advance via mark_cargo_installed soon).
                if task.status == "integrated" && integrated_is_terminal_no_post_subscriber(db) {
                    return Ok(());
                }
            }
            None => {
                let tasks = fetch_all_tasks(&conn)?;
                // Print multi-frame if any task state changed
                let any_changed = tasks.iter().any(|t| {
                    let key = t.state_key();
                    should_print(prev_keys.get(&t.display_id), &key)
                });
                if any_changed {
                    println!("{}", compute_multi_frame(&tasks));
                    for t in &tasks {
                        prev_keys.insert(t.display_id.clone(), t.state_key());
                    }
                }
                // `integrated` is framework-terminal when no post-integrated
                // subscriber is wired; otherwise it's awaiting post-land and
                // remains active. Compute active_tasks accordingly so the
                // multi-task follow loop exits in the no-subscriber case.
                let integrated_terminal = integrated_is_terminal_no_post_subscriber(db);
                let active_tasks_remaining = tasks
                    .iter()
                    .any(|t| !(t.status == "integrated" && integrated_terminal));
                if !active_tasks_remaining {
                    // All remaining tasks are framework-terminal
                    return Ok(());
                }
            }
        }

        // Sleep between frames (skipped in tests with interval_ms=0)
        if interval.as_millis() > 0 {
            // Poll SIGINT during sleep by sleeping in small increments
            let chunk = std::time::Duration::from_millis(50);
            let mut remaining = interval;
            while remaining > std::time::Duration::ZERO {
                if INTERRUPTED.load(Ordering::SeqCst) {
                    std::process::exit(130);
                }
                let sleep_for = remaining.min(chunk);
                std::thread::sleep(sleep_for);
                remaining = remaining.saturating_sub(chunk);
            }
        }
    }

    // Exhausted max_iters — exit 0 (for tests; in production max_iters is usize::MAX)
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (AC5.6)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use tempfile::tempdir;

    // Minimal tasks schema DDL for tests (matches real schema shape).
    const TEST_DDL: &str = "
        CREATE TABLE tasks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            display_id TEXT NOT NULL UNIQUE,
            status TEXT NOT NULL DEFAULT 'planning',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            created_by TEXT NOT NULL DEFAULT 'human',
            updated_by TEXT NOT NULL DEFAULT 'human',
            title TEXT,
            current_phase INTEGER,
            current_cycle INTEGER,
            blocked_reason TEXT,
            plan TEXT,
            cycles TEXT,
            depends_on TEXT,
            claimed_by TEXT,
            claimed_at TEXT
        );
    ";

    fn open_test_conn() -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.db");
        let conn = db::open(&db_file).unwrap();
        conn.execute_batch(TEST_DDL).unwrap();
        (dir, conn)
    }

    fn insert_task(
        conn: &Connection,
        display_id: &str,
        status: &str,
        phase: Option<i64>,
        cycle: Option<i64>,
        blocked_reason: Option<&str>,
        total_phases: Option<usize>,
    ) {
        let plan_json = total_phases.map(|n| {
            let phases: Vec<serde_json::Value> = (0..n)
                .map(|i| serde_json::json!({"name": format!("Phase {}", i + 1)}))
                .collect();
            serde_json::to_string(&serde_json::json!({"phases": phases})).unwrap()
        });
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, \
             title, current_phase, current_cycle, blocked_reason, plan) \
             VALUES (?1, ?2, '2026-01-01', '2026-01-01', 'human', 'human', \
             'Test Task', ?3, ?4, ?5, ?6)",
            rusqlite::params![
                display_id,
                status,
                phase,
                cycle,
                blocked_reason,
                plan_json,
            ],
        )
        .unwrap();
    }

    #[test]
    fn live_runner_lines_include_marker_paths_and_updated_age() {
        let dir = tempdir().unwrap();
        let stores_dir = dir.path().join(".stores");
        let runs_dir = stores_dir.join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        let marker_path = runs_dir.join("current-T123-executor.json");
        std::fs::create_dir_all(runs_dir.join("abc")).unwrap();
        std::fs::write(
            runs_dir.join("abc/status.json"),
            r#"{
  "last_event_at": "2026-05-11T00:01:00Z",
  "last_event_type": "tool_start",
  "current_activity": "tool:bash"
}"#,
        )
        .unwrap();
        let current = crate::cli::runs::CurrentRun {
            marker_path,
            marker: crate::cli::runs::CurrentRunMarker {
                display_id: "T123".to_string(),
                phase: Some(2),
                cycle: Some(1),
                role: "executor".to_string(),
                runner: Some("pi".to_string()),
                session_id: Some("abc".to_string()),
                status: Some("running".to_string()),
                transcript_path: Some(std::path::PathBuf::from(".stores/runs/abc.jsonl")),
                stderr_log_path: Some(std::path::PathBuf::from(".stores/runs/abc.stderr.log")),
                events_path: None,
                status_path: None,
                updated_at: Some("2026-05-11T00:00:00Z".to_string()),
            },
        };
        let now = DateTime::parse_from_rfc3339("2026-05-11T00:01:05Z")
            .unwrap()
            .with_timezone(&Utc);

        let lines = live_runner_lines(&stores_dir, &current, now);

        assert_eq!(
            lines[0],
            "Live runner: role=executor runner=pi status=running updated=1m ago last_event=5s ago activity=tool:bash event=tool_start"
        );
        assert_eq!(
            lines[1],
            format!("Live stdout: {}", runs_dir.join("abc.jsonl").display())
        );
        assert_eq!(
            lines[2],
            format!("Live stderr: {}", runs_dir.join("abc.stderr.log").display())
        );
    }

    #[test]
    fn live_runner_lines_tolerate_minimal_marker() {
        let dir = tempdir().unwrap();
        let stores_dir = dir.path().join(".stores");
        let marker_path = stores_dir.join("runs/current-T123-executor.json");
        let current = crate::cli::runs::CurrentRun {
            marker_path,
            marker: crate::cli::runs::CurrentRunMarker {
                display_id: "T123".to_string(),
                phase: None,
                cycle: None,
                role: "executor".to_string(),
                runner: None,
                session_id: None,
                status: None,
                transcript_path: None,
                stderr_log_path: None,
                events_path: None,
                status_path: None,
                updated_at: None,
            },
        };

        let now = DateTime::parse_from_rfc3339("2026-05-11T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let lines = live_runner_lines(&stores_dir, &current, now);

        assert_eq!(lines, vec!["Live runner: role=executor runner=? status=?"]);
    }

    // -----------------------------------------------------------------------
    // AC5.1: single-frame mode
    // -----------------------------------------------------------------------

    #[test]
    fn single_frame_contains_required_fields() {
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T001", "executing", Some(2), Some(1), None, Some(3));

        let task = fetch_task(&conn, "T001").unwrap();
        let frame = compute_status_frame(&task);

        // Timestamp prefix
        assert!(
            frame.contains('[') && frame.contains(']'),
            "frame should have timestamp: {frame}"
        );
        assert!(frame.contains("T001"), "frame should contain id: {frame}");
        assert!(
            frame.contains("status=executing"),
            "frame should contain status: {frame}"
        );
        assert!(
            frame.contains("phase=2/3"),
            "frame should contain phase: {frame}"
        );
        assert!(
            frame.contains("cycle=1"),
            "frame should contain cycle: {frame}"
        );
        assert!(
            frame.contains("next=executor"),
            "frame should contain next: {frame}"
        );
        assert!(
            frame.contains("blocked=false"),
            "frame should contain blocked: {frame}"
        );
    }

    #[test]
    fn single_frame_blocked_task() {
        let (_dir, conn) = open_test_conn();
        insert_task(
            &conn,
            "T002",
            "blocked",
            None,
            None,
            Some("Needs human input"),
            None,
        );

        let task = fetch_task(&conn, "T002").unwrap();
        let frame = compute_status_frame(&task);

        assert!(
            frame.contains("status=blocked"),
            "frame should contain status=blocked: {frame}"
        );
        assert!(
            frame.contains("blocked=true"),
            "frame should contain blocked=true: {frame}"
        );
    }

    // -----------------------------------------------------------------------
    // Bug 1 / T005-P1: is_blocked() helper — status drives the predicate
    // -----------------------------------------------------------------------

    /// status="executing", blocked_reason=None → blocked=false
    #[test]
    fn blocked_helper_null_reason() {
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T010", "executing", Some(1), Some(1), None, Some(2));
        let task = fetch_task(&conn, "T010").unwrap();
        let line = format_task_line(&task);
        assert!(
            line.contains("blocked=false"),
            "null reason + status=executing → blocked=false: {line}"
        );
    }

    /// status="executing", blocked_reason=Some("") → blocked=false
    #[test]
    fn blocked_helper_empty_reason() {
        let (_dir, conn) = open_test_conn();
        insert_task(
            &conn,
            "T011",
            "executing",
            Some(1),
            Some(1),
            Some(""),
            Some(2),
        );
        let task = fetch_task(&conn, "T011").unwrap();
        let line = format_task_line(&task);
        assert!(
            line.contains("blocked=false"),
            "empty-string reason + status=executing → blocked=false: {line}"
        );
    }

    /// status="executing", blocked_reason=Some("real reason") → blocked=false
    /// (status-only predicate: reason is a description, not the gate)
    #[test]
    fn blocked_helper_real_reason() {
        let (_dir, conn) = open_test_conn();
        insert_task(
            &conn,
            "T012",
            "executing",
            Some(1),
            Some(1),
            Some("real reason"),
            Some(2),
        );
        let task = fetch_task(&conn, "T012").unwrap();
        let line = format_task_line(&task);
        assert!(
            line.contains("blocked=false"),
            "real reason + status=executing → blocked=false (status drives): {line}"
        );
    }

    /// status="blocked" × all three reason shapes → blocked=true for all three
    #[test]
    fn blocked_helper_status_blocked_all_reasons() {
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T013", "blocked", None, None, None, None);
        insert_task(&conn, "T014", "blocked", None, None, Some(""), None);
        insert_task(
            &conn,
            "T015",
            "blocked",
            None,
            None,
            Some("real reason"),
            None,
        );

        for id in &["T013", "T014", "T015"] {
            let task = fetch_task(&conn, id).unwrap();
            let line = format_task_line(&task);
            assert!(
                line.contains("blocked=true"),
                "status=blocked → blocked=true regardless of reason (id={id}): {line}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // AC5.3: multi-task frame
    // -----------------------------------------------------------------------

    #[test]
    fn multi_frame_contains_both_tasks() {
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T001", "executing", Some(2), Some(1), None, Some(3));
        insert_task(&conn, "T002", "plan_review", None, None, None, None);

        let tasks = fetch_all_tasks(&conn).unwrap();
        assert_eq!(tasks.len(), 2, "should fetch 2 non-terminal tasks");

        let frame = compute_multi_frame(&tasks);
        assert!(
            frame.contains("T001"),
            "multi-frame should contain T001: {frame}"
        );
        assert!(
            frame.contains("T002"),
            "multi-frame should contain T002: {frame}"
        );
        assert!(
            frame.contains("status=executing"),
            "multi-frame should contain executing: {frame}"
        );
        assert!(
            frame.contains("status=plan_review"),
            "multi-frame should contain plan_review: {frame}"
        );
        // Both lines indented
        assert!(
            frame.contains("  T001"),
            "T001 line should be indented: {frame}"
        );
        assert!(
            frame.contains("  T002"),
            "T002 line should be indented: {frame}"
        );
    }

    #[test]
    fn multi_frame_excludes_terminal_tasks() {
        let (_dir, conn) = open_test_conn();
        // accepted, rejected, and abandoned are terminal/history; executing is active.
        insert_task(&conn, "T001", "accepted", None, None, None, None);
        insert_task(&conn, "T002", "rejected", None, None, None, None);
        insert_task(&conn, "T004", "abandoned", None, None, None, None);
        insert_task(&conn, "T003", "executing", Some(1), Some(1), None, Some(2));

        let tasks = fetch_all_tasks(&conn).unwrap();
        assert_eq!(
            tasks.len(),
            1,
            "should only fetch non-terminal tasks: {tasks:?}"
        );
        assert_eq!(tasks[0].display_id, "T003");
    }

    // -----------------------------------------------------------------------
    // AC5.5: change detection
    // -----------------------------------------------------------------------

    #[test]
    fn should_print_first_frame_always() {
        let key = StateKey {
            status: "executing".into(),
            current_phase: Some(1),
            current_cycle: Some(1),
            blocked_reason: None,
        };
        assert!(
            should_print(None, &key),
            "first frame (None prev) should always print"
        );
    }

    #[test]
    fn should_print_same_state_suppressed() {
        let key = StateKey {
            status: "executing".into(),
            current_phase: Some(1),
            current_cycle: Some(1),
            blocked_reason: None,
        };
        assert!(
            !should_print(Some(&key), &key),
            "identical state should be suppressed"
        );
    }

    #[test]
    fn should_print_on_status_change() {
        let prev = StateKey {
            status: "executing".into(),
            current_phase: Some(1),
            current_cycle: Some(1),
            blocked_reason: None,
        };
        let next = StateKey {
            status: "code_review".into(),
            current_phase: Some(1),
            current_cycle: Some(1),
            blocked_reason: None,
        };
        assert!(
            should_print(Some(&prev), &next),
            "status change should print"
        );
    }

    #[test]
    fn should_print_on_phase_change() {
        let prev = StateKey {
            status: "executing".into(),
            current_phase: Some(1),
            current_cycle: Some(1),
            blocked_reason: None,
        };
        let next = StateKey {
            status: "executing".into(),
            current_phase: Some(2),
            current_cycle: Some(1),
            blocked_reason: None,
        };
        assert!(
            should_print(Some(&prev), &next),
            "phase change should print"
        );
    }

    #[test]
    fn should_print_on_cycle_change() {
        let prev = StateKey {
            status: "executing".into(),
            current_phase: Some(1),
            current_cycle: Some(1),
            blocked_reason: None,
        };
        let next = StateKey {
            status: "executing".into(),
            current_phase: Some(1),
            current_cycle: Some(2),
            blocked_reason: None,
        };
        assert!(
            should_print(Some(&prev), &next),
            "cycle change should print"
        );
    }

    // -----------------------------------------------------------------------
    // AC5.6: bounded follow loop
    // -----------------------------------------------------------------------

    #[test]
    fn bounded_follow_loop_runs_max_iters() {
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T001", "executing", Some(1), Some(1), None, Some(3));
        let db_path_val = _dir.path().join("test.db");

        // Patch: we need to set the db_path context. Instead of routing through
        // run_status (which calls db_path()), we test the loop logic directly
        // by calling run_follow_loop with a fixed path.
        let args = StatusArgs {
            display_id: Some("T001".to_string()),
            follow: true,
            interval_ms: 0,
            max_iters: 3,
        };
        // Use run_follow_loop directly (it takes a &Path)
        let result = run_follow_loop(&db_path_val, args);
        assert!(
            result.is_ok(),
            "bounded follow loop should succeed: {result:?}"
        );
    }

    #[test]
    fn bounded_follow_loop_exits_on_accepted() {
        // `accepted` is now the terminal state that causes single-task follow to exit.
        // `complete` is transient and is_awaiting_human returns false for it (it's mid-flow).
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T001", "accepted", None, None, None, None);
        let db_path_val = _dir.path().join("test.db");

        let args = StatusArgs {
            display_id: Some("T001".to_string()),
            follow: true,
            interval_ms: 0,
            max_iters: 100, // high limit, should exit early
        };
        let result = run_follow_loop(&db_path_val, args);
        // Should exit 0 immediately (terminal state on first iter)
        assert!(
            result.is_ok(),
            "follow loop should exit 0 on accepted task: {result:?}"
        );
    }

    #[test]
    fn bounded_follow_loop_exits_on_in_review() {
        // `in_review` triggers is_awaiting_human — the single-task follow loop should
        // exit 0 at this state (human needs to act; further polling is pointless).
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T001", "in_review", None, None, None, None);
        let db_path_val = _dir.path().join("test.db");

        let args = StatusArgs {
            display_id: Some("T001".to_string()),
            follow: true,
            interval_ms: 0,
            max_iters: 100,
        };
        let result = run_follow_loop(&db_path_val, args);
        assert!(
            result.is_ok(),
            "follow loop should exit 0 on in_review task: {result:?}"
        );
    }

    #[test]
    fn bounded_follow_loop_exits_on_integration_blocked() {
        // `integration_blocked` mirrors `blocked` / `deploy_blocked` — awaiting
        // a human-authorized retry-integration. is_awaiting_human returns true
        // and the single-task follow loop exits 0.
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T100", "integration_blocked", None, None, None, None);
        let db_path_val = _dir.path().join("test.db");

        let args = StatusArgs {
            display_id: Some("T100".to_string()),
            follow: true,
            interval_ms: 0,
            max_iters: 100,
        };
        let result = run_follow_loop(&db_path_val, args);
        assert!(
            result.is_ok(),
            "follow loop should exit 0 on integration_blocked: {result:?}"
        );
    }

    #[test]
    fn bounded_follow_loop_exits_on_integrated_when_no_post_subscriber() {
        // No agents.yaml in the tempdir → no post-integrated subscriber wired
        // → `integrated` is framework-terminal and the follow loop exits 0.
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T101", "integrated", None, None, None, None);
        let db_path_val = _dir.path().join("test.db");

        let args = StatusArgs {
            display_id: Some("T101".to_string()),
            follow: true,
            interval_ms: 0,
            max_iters: 100,
        };
        let result = run_follow_loop(&db_path_val, args);
        assert!(
            result.is_ok(),
            "follow loop should exit 0 on integrated with no post-subscriber: {result:?}"
        );
    }

    #[test]
    fn bounded_follow_loop_runs_max_iters_on_integrated_with_post_subscriber() {
        // agents.yaml wires schema-migrate on (integrated → cargo_installed)
        // → `integrated` is awaiting post-land, NOT terminal. The single-task
        // follow loop must keep polling until max_iters.
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T102", "integrated", None, None, None, None);
        let db_path_val = _dir.path().join("test.db");
        // Drop an agents.yaml fixture next to the db.
        let agents_yaml = r#"
agents:
  - name: schema-migrate
    subscribes_to:
      - store: tasks
        transition: { from: integrated, to: cargo_installed }
    command: "builtin:schema-migrate"
"#;
        std::fs::write(_dir.path().join("agents.yaml"), agents_yaml).unwrap();

        let args = StatusArgs {
            display_id: Some("T102".to_string()),
            follow: true,
            interval_ms: 0,
            max_iters: 3,
        };
        let result = run_follow_loop(&db_path_val, args);
        assert!(
            result.is_ok(),
            "follow loop should run max_iters when post-integrated subscriber wired: {result:?}"
        );
    }

    #[test]
    fn bounded_follow_loop_runs_max_iters_on_integrated_with_to_integrated_subscriber() {
        // cargo-install subscribes to the transition into integrated
        // (integrating → integrated). An integrated row may still be awaiting
        // that post-land handoff, so follow must not treat integrated as
        // terminal merely because no subscriber has from: integrated.
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T103", "integrated", None, None, None, None);
        let db_path_val = _dir.path().join("test.db");
        let agents_yaml = r#"
agents:
  - name: cargo-install
    subscribes_to:
      - store: tasks
        transition: { from: integrating, to: integrated }
    command: "builtin:cargo-install"
"#;
        std::fs::write(_dir.path().join("agents.yaml"), agents_yaml).unwrap();

        let args = StatusArgs {
            display_id: Some("T103".to_string()),
            follow: true,
            interval_ms: 0,
            max_iters: 3,
        };
        let result = run_follow_loop(&db_path_val, args);
        assert!(
            result.is_ok(),
            "follow loop should run max_iters when to-integrated subscriber wired: {result:?}"
        );
    }

    #[test]
    fn bounded_follow_loop_multi_task_exits_on_integrated_when_no_post_subscriber() {
        // Multi-task follow (display_id=None): only an `integrated` row remains
        // and no agents.yaml subscriber is wired off `integrated` → the row is
        // framework-terminal and the loop must exit BEFORE max_iters.
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T150", "integrated", None, None, None, None);
        let db_path_val = _dir.path().join("test.db");

        let args = StatusArgs {
            display_id: None,
            follow: true,
            interval_ms: 0,
            // If the loop fails to treat integrated as terminal it will exhaust
            // max_iters; we use a small bound so the test still finishes, and
            // assert exit timing via wall-clock proxy below.
            max_iters: 1_000_000,
        };
        let start = std::time::Instant::now();
        let result = run_follow_loop(&db_path_val, args);
        let elapsed = start.elapsed();
        assert!(
            result.is_ok(),
            "multi-task follow loop should exit 0 on integrated-only with no post-subscriber: {result:?}"
        );
        // With interval_ms=0 a one-million-iter exhaustion takes seconds; an
        // early exit returns in milliseconds. 500ms is a generous ceiling that
        // still distinguishes the two regimes on slow CI hardware.
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "loop should exit early, not exhaust max_iters; elapsed={elapsed:?}"
        );
    }

    #[test]
    fn bounded_follow_loop_multi_task_keeps_running_on_integrated_with_post_subscriber() {
        // Multi-task follow (display_id=None): an `integrated` row plus an
        // agents.yaml subscriber wired off `integrated` → the row is awaiting
        // post-land and the loop must keep polling until max_iters.
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T151", "integrated", None, None, None, None);
        let db_path_val = _dir.path().join("test.db");
        let agents_yaml = r#"
agents:
  - name: schema-migrate
    subscribes_to:
      - store: tasks
        transition: { from: integrated, to: cargo_installed }
    command: "builtin:schema-migrate"
"#;
        std::fs::write(_dir.path().join("agents.yaml"), agents_yaml).unwrap();

        let args = StatusArgs {
            display_id: None,
            follow: true,
            interval_ms: 0,
            max_iters: 3,
        };
        let result = run_follow_loop(&db_path_val, args);
        assert!(
            result.is_ok(),
            "multi-task follow loop should run max_iters when post-integrated subscriber wired: {result:?}"
        );
    }

    #[test]
    fn integration_active_states_keep_multi_task_loop_alive() {
        // integration_queued and integrating are NOT terminal — multi-task
        // follow must include them and not exit early.
        let (_dir, conn) = open_test_conn();
        insert_task(&conn, "T200", "integration_queued", None, None, None, None);
        insert_task(&conn, "T201", "integrating", None, None, None, None);
        let tasks = fetch_all_tasks(&conn).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|t| t.status == "integration_queued"));
        assert!(tasks.iter().any(|t| t.status == "integrating"));
    }

    #[test]
    fn next_from_status_covers_integration_lane_states() {
        assert_eq!(next_from_status("integration_queued"), "integrate");
        assert_eq!(next_from_status("integrating"), "integrate");
        assert_eq!(next_from_status("integrated"), "post-integrated");
        assert_eq!(next_from_status("integration_blocked"), "-");
    }

    #[test]
    fn bounded_follow_loop_multi_task_exits_when_all_terminal() {
        let (_dir, conn) = open_test_conn();
        // All tasks are terminal — multi-task loop should exit immediately.
        // `accepted`, `rejected`, and `abandoned` are terminal; `blocked` is still active.
        insert_task(&conn, "T001", "accepted", None, None, None, None);
        insert_task(&conn, "T002", "rejected", None, None, None, None);
        insert_task(&conn, "T003", "abandoned", None, None, None, None);
        let db_path_val = _dir.path().join("test.db");

        let args = StatusArgs {
            display_id: None,
            follow: true,
            interval_ms: 0,
            max_iters: 100,
        };
        let result = run_follow_loop(&db_path_val, args);
        assert!(
            result.is_ok(),
            "multi follow loop should exit 0 when all terminal: {result:?}"
        );
    }
}
