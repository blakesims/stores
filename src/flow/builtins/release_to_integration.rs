use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::flow::builtins::{fire_framework_transition, DispatchCtx};
use crate::handlers::row::now_iso8601;

fn row_str<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn row_matches(row: &Value) -> bool {
    let lifecycle = row_str(row, "lifecycle");
    let active_step = row_str(row, "active_step");
    let status = row_str(row, "status").unwrap_or("");
    let acceptance_policy = row_str(row, "human_acceptance_policy").unwrap_or("optional");
    let decided = row_str(row, "acceptance_decided_by").is_some();
    let in_source = (lifecycle == Some("active") && active_step == Some("wrapping"))
        || matches!(status, "complete" | "in_review" | "accepted");
    let acceptance_possible = matches!(acceptance_policy, "required" | "optional" | "delegated_by_policy") || decided;
    in_source && acceptance_possible
}

fn has_passing_authoritative_task_review(ctx: &DispatchCtx, display_id: &str) -> Result<bool> {
    let exists: Option<i64> = ctx
        .conn
        .query_row(
            "SELECT 1 FROM external_reviews WHERE task_id=?1 AND status='passed' AND verdict='PASS' LIMIT 1",
            params![display_id],
            |r| r.get(0),
        )
        .optional()
        .or_else(|e| {
            if matches!(e, rusqlite::Error::SqliteFailure(_, _)) {
                Ok(None)
            } else {
                Err(e)
            }
        })?;
    Ok(exists.is_some())
}

fn write_policy_delegate(ctx: &DispatchCtx, display_id: &str) -> Result<()> {
    ctx.conn.execute(
        "UPDATE tasks SET acceptance_decided_by='policy_delegate', acceptance_decided_at=?1 WHERE display_id=?2",
        params![now_iso8601(), display_id],
    )?;
    Ok(())
}

pub fn run(row: &Value, ctx: &DispatchCtx) -> Result<i32> {
    if !row_matches(row) {
        return Ok(0);
    }
    let display_id = row_str(row, "display_id").context("release-to-integration row missing display_id")?;
    let policy = row_str(row, "human_acceptance_policy").unwrap_or("optional");
    let task_review_policy = row_str(row, "task_review_policy").unwrap_or("none");
    if policy == "delegated_by_policy"
        && row_str(row, "acceptance_decided_by").is_none()
        && matches!(task_review_policy, "authoritative" | "both")
        && has_passing_authoritative_task_review(ctx, display_id)?
    {
        write_policy_delegate(ctx, display_id)?;
    }
    fire_framework_transition(
        ctx.conn,
        display_id,
        "release-to-integration",
        BTreeMap::new(),
        ctx.policies_hash,
    )?;
    Ok(0)
}
