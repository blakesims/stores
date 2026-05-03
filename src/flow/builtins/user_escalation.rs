//! `builtin:user-escalation` — default deploy_blocked handler.
//!
//! Files a substrate observation pointing at the blocked task with conflict
//! context, then fires `ntfy`. Pure side-effect; the row stays `deploy_blocked`
//! awaiting human `resume`.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::Value;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::flow::NotifyEvent;
use crate::handlers::row::now_iso8601;

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
    let branch = row.get("branch").and_then(|v| v.as_str()).unwrap_or("");
    let blocked_reason = row
        .get("blocked_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let new_obs_id = file_observation(ctx.conn, display_id, branch, blocked_reason)
        .context("filing user-escalation observation")?;

    let event = NotifyEvent {
        row_id: display_id.to_string(),
        transition_attempted: "tasks: deploy_blocked (user-escalation)".to_string(),
        policy_id_or_actor_halt: "user-escalation".to_string(),
        summary: format!(
            "filed observation {} for deploy_blocked task {} (branch '{}')",
            new_obs_id, display_id, branch
        ),
    };
    let _ = crate::flow::notify_with_path(ctx.config_path, event);

    Ok(0)
}

/// Direct INSERT into `observations` (bypasses the normal add handler so we
/// don't need a clap matches struct in-process). Returns the minted L-id.
fn file_observation(
    conn: &Connection,
    task_display_id: &str,
    branch: &str,
    blocked_reason: &str,
) -> Result<String> {
    let max_id: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM observations", [], |r| {
            r.get(0)
        })
        .unwrap_or(0);
    let next_num = max_id + 1;
    let new_display_id = format!("L{:03}", next_num);

    let summary = format!(
        "deploy-blocked: task {} merge conflict on branch '{}'",
        task_display_id, branch
    );
    let body = format!(
        "Task {task_display_id} is in deploy_blocked after accept-merge \
         hit a conflict on branch '{branch}'.\n\nDetails:\n{blocked_reason}\n\n\
         Resume after specialist intervention with: \
         stores tasks resume {task_display_id}"
    );
    let now = now_iso8601();
    let week = ops_week_label(&now);

    conn.execute(
        "INSERT INTO observations \
         (display_id, status, summary, body, source, priority, captured_at, captured_week, task_id, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'open', ?2, ?3, 'dev', 'normal', ?4, ?5, ?6, ?4, ?4, 'framework', 'framework')",
        rusqlite::params![
            new_display_id,
            summary,
            body,
            now,
            week,
            task_display_id,
        ],
    )
    .context("INSERT INTO observations")?;

    Ok(new_display_id)
}

/// Cheap ops-week label `wWW` from an ISO-ish timestamp prefix. Best-effort —
/// observation captured_week is operator-hygiene per-schema with no pattern.
fn ops_week_label(_now_iso: &str) -> String {
    // Simple stamp; the schema doesn't enforce a pattern (per stores/observations/CLAUDE.md).
    "w-flow-engine".to_string()
}
