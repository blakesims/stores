//! `builtin:external-review` — daemon lane for typed external_reviews rows.
//!
//! Runs one pending external review attempt through the configured review
//! runner, persists the verdict/status on the attempt row, and routes REVISE
//! back to the task executor lane via the framework transition.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::flow::config::{resolve_codex_config, resolve_review_config};
use crate::handlers::external_reviews::{
    run_external_review_attempt, ExternalReviewVerdict, ParsedReviewOutput, ToolingError,
};
use crate::handlers::row::now_iso8601;
use crate::handlers::transition::execute_transition_write;
use crate::schema::actor::Actor;
use crate::schema::lifecycle::select_transition;
use crate::validate::{self, EntryMap, Op};

const REVIEW_AGENT_NAME: &str = "external-review";
const TOOLING_RETRY_SECS: i64 = 300;

#[derive(Debug, Clone)]
struct ReviewRow {
    row_id: i64,
    display_id: String,
    task_id: String,
    status: String,
    attempt: i64,
}

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row.get("display_id").and_then(Value::as_str).unwrap_or("");
    if display_id.is_empty() {
        eprintln!("[external-review] row missing display_id; skipping");
        return Ok(1);
    }

    ensure_runtime_columns(ctx.conn).with_context(|| "external-review runtime columns")?;

    // Before the pending guard: promote any tooling_held rows whose retry
    // window has elapsed back to pending so they are re-tried on this or a
    // subsequent daemon iteration.
    promote_elapsed_tooling_held(ctx.conn)?;

    let Some(review_row) = load_review_row(ctx.conn, display_id)? else {
        eprintln!("[external-review] {display_id}: row disappeared; skipping");
        return Ok(1);
    };
    if review_row.status != "pending" {
        eprintln!(
            "[external-review] {}: status={} not pending; skipping",
            review_row.display_id, review_row.status
        );
        return Ok(0);
    }

    let cap = resolve_review_config(ctx.config_path).max_parallel.max(1);
    let running = count_running_reviews(ctx.conn)?;
    if running >= cap as usize {
        mark_cap_held(ctx.conn, &review_row)?;
        eprintln!(
            "[external-review cap held] task_id={} review_attempt_id={} runner={} status=pending held_reason=cap-held running={} cap={} liveness=cap-held retry=next-poll",
            review_row.task_id,
            review_row.display_id,
            resolve_review_config(ctx.config_path).runner,
            running,
            cap
        );
        return Ok(0);
    }

    mark_running(ctx.conn, &review_row)?;
    let review_cfg = resolve_review_config(ctx.config_path);
    let codex_cfg = resolve_codex_config(ctx.config_path);
    eprintln!(
        "[external-review] task_id={} review_attempt_id={} attempt={} runner={} status=running held_reason=none liveness=live retry=none",
        review_row.task_id, review_row.display_id, review_row.attempt, review_cfg.runner
    );

    match run_external_review_attempt(
        ctx.conn,
        &review_row.display_id,
        &review_row.task_id,
        &review_cfg,
        &codex_cfg,
        None,
        None,
        None,
    ) {
        Ok(parsed) => record_terminal(ctx, &review_row, &review_cfg.runner, parsed)?,
        Err(err) => record_tooling_held(ctx.conn, &review_row, &review_cfg.runner, &err)?,
    }

    Ok(0)
}

fn ensure_runtime_columns(conn: &Connection) -> Result<()> {
    let cols = external_review_columns(conn)?;
    let expected = [
        ("held_reason", "TEXT"),
        ("next_retry_at", "TEXT"),
        ("attempts", "INTEGER DEFAULT 0"),
    ];
    for (name, ty) in expected {
        if !cols.contains(name) {
            conn.execute(
                &format!("ALTER TABLE external_reviews ADD COLUMN {name} {ty}"),
                [],
            )?;
        }
    }
    Ok(())
}

fn external_review_columns(conn: &Connection) -> Result<std::collections::BTreeSet<String>> {
    let mut stmt = conn.prepare("PRAGMA table_info(external_reviews)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect::<std::result::Result<std::collections::BTreeSet<_>, _>>()
        .map_err(Into::into)
}

fn load_review_row(conn: &Connection, display_id: &str) -> Result<Option<ReviewRow>> {
    conn.query_row(
        "SELECT id, display_id, task_id, status, attempt FROM external_reviews WHERE display_id=?1",
        params![display_id],
        |r| {
            Ok(ReviewRow {
                row_id: r.get(0)?,
                display_id: r.get(1)?,
                task_id: r.get(2)?,
                status: r.get(3)?,
                attempt: r.get(4)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn cap_allows_or_log(
    conn: &Connection,
    config_path: &std::path::Path,
    display_id: &str,
) -> Result<bool> {
    ensure_runtime_columns(conn)?;
    let Some(row) = load_review_row(conn, display_id)? else {
        return Ok(false);
    };
    let cfg = resolve_review_config(config_path);
    let cap = cfg.max_parallel.max(1);
    let running = count_running_reviews(conn)?;
    if running < cap as usize {
        return Ok(true);
    }
    mark_cap_held(conn, &row)?;
    eprintln!(
        "[external-review cap held] task_id={} review_attempt_id={} runner={} status=pending held_reason=cap-held running={} cap={} liveness=cap-held retry=next-poll",
        row.task_id, row.display_id, cfg.runner, running, cap
    );
    Ok(false)
}

fn count_running_reviews(conn: &Connection) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM external_reviews WHERE status='running'",
        [],
        |r| r.get(0),
    )?;
    Ok(n.max(0) as usize)
}

fn mark_cap_held(conn: &Connection, row: &ReviewRow) -> Result<()> {
    let now = now_iso8601();
    conn.execute(
        "UPDATE external_reviews SET held_reason='cap-held', updated_at=?2 WHERE display_id=?1",
        params![row.display_id, now],
    )?;
    Ok(())
}

fn mark_running(conn: &Connection, row: &ReviewRow) -> Result<()> {
    let now = now_iso8601();
    conn.execute(
        "UPDATE external_reviews SET status='running', started_at=COALESCE(started_at, ?2), updated_at=?2, held_reason=NULL, attempts=COALESCE(attempts,0)+1 WHERE display_id=?1 AND status='pending'",
        params![row.display_id, now],
    )?;
    let tx = conn.unchecked_transaction()?;
    crate::db::insert_transition_history(
        &tx,
        "external_reviews",
        row.row_id,
        &row.display_id,
        "pending",
        "running",
        "start-review",
        "framework",
        None,
        None,
        Some(REVIEW_AGENT_NAME),
    )?;
    tx.commit()?;
    Ok(())
}

/// Scan for `tooling_held` rows whose `next_retry_at` has elapsed and
/// transition them back to `pending` so they are picked up on the next
/// daemon iteration.  Called at the top of `run()` before the pending guard.
fn promote_elapsed_tooling_held(conn: &Connection) -> Result<()> {
    let now = now_iso8601();
    // Collect rows that are past their retry window.
    let candidates: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, display_id FROM external_reviews \
             WHERE status='tooling_held' AND next_retry_at IS NOT NULL AND next_retry_at <= ?1",
        )?;
        let rows = stmt
            .query_map(params![now], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };
    for (row_id, did) in candidates {
        conn.execute(
            "UPDATE external_reviews SET status='pending', held_reason=NULL, next_retry_at=NULL, updated_at=?2 WHERE display_id=?1 AND status='tooling_held'",
            params![did, now],
        )?;
        let tx = conn.unchecked_transaction()?;
        crate::db::insert_transition_history(
            &tx,
            "external_reviews",
            row_id,
            &did,
            "tooling_held",
            "pending",
            "retry-after-tooling-held",
            "framework",
            None,
            None,
            Some(REVIEW_AGENT_NAME),
        )?;
        tx.commit()?;
        eprintln!(
            "[external-review] {did}: tooling_held → pending (retry window elapsed)"
        );
    }
    Ok(())
}

fn record_terminal(
    ctx: &DispatchCtx,
    row: &ReviewRow,
    runner: &str,
    parsed: ParsedReviewOutput,
) -> Result<()> {
    let (status, gate) = match parsed.verdict {
        ExternalReviewVerdict::Pass => ("passed", "PASS"),
        ExternalReviewVerdict::Revise => ("revise", "REVISE"),
        ExternalReviewVerdict::ToolingFailure => {
            let err = ToolingError {
                verdict: ExternalReviewVerdict::ToolingFailure,
                message: "runner returned TOOLING_FAILURE".to_string(),
            };
            return record_tooling_held(ctx.conn, row, runner, &err);
        }
    };
    let now = now_iso8601();
    ctx.conn.execute(
        "UPDATE external_reviews SET status=?2, verdict=?3, completed_at=COALESCE(completed_at, ?4), updated_at=?4, held_reason=NULL, next_retry_at=NULL WHERE display_id=?1",
        params![row.display_id, status, gate, now],
    )?;
    insert_review_transition(
        ctx.conn,
        row,
        "running",
        status,
        "record-verdict",
        Some(gate),
    )?;
    eprintln!(
        "[external-review] task_id={} review_attempt_id={} runner={} status={} verdict={} held_reason=none liveness=complete retry=none",
        row.task_id, row.display_id, runner, status, gate
    );
    if gate == "REVISE" {
        fire_task_external_review_revise(ctx.conn, &row.task_id, ctx.policies_hash)?;
    }
    Ok(())
}

fn record_tooling_held(
    conn: &Connection,
    row: &ReviewRow,
    runner: &str,
    err: &ToolingError,
) -> Result<()> {
    let now = now_iso8601();
    let next_retry_at = add_secs(&now, TOOLING_RETRY_SECS).unwrap_or_else(|| now.clone());
    let reason = err.message.clone();
    let log_ref = format!("external_review://{}/tooling_failure", row.display_id);
    let findings =
        serde_json::json!({"verdict":"TOOLING_FAILURE","error":reason,"log_ref":log_ref})
            .to_string();
    conn.execute(
        "UPDATE external_reviews SET status='tooling_held', verdict='TOOLING_FAILURE', held_reason=?2, next_retry_at=?3, completed_at=?4, updated_at=?4, log_path=COALESCE(NULLIF(log_path,''), ?5), transcript_path=COALESCE(NULLIF(transcript_path,''), ?5), findings=?6 WHERE display_id=?1",
        params![row.display_id, reason, next_retry_at, now, log_ref, findings],
    )?;
    insert_review_transition(
        conn,
        row,
        "running",
        "tooling_held",
        "record-tooling-failure",
        None,
    )?;
    eprintln!(
        "[external-review] task_id={} review_attempt_id={} runner={} status=tooling_held held_reason={} liveness=held retry={}",
        row.task_id, row.display_id, runner, err.message, next_retry_at
    );
    Ok(())
}

fn insert_review_transition(
    conn: &Connection,
    row: &ReviewRow,
    from: &str,
    to: &str,
    verb: &str,
    gate: Option<&str>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    crate::db::insert_transition_history(
        &tx,
        "external_reviews",
        row.row_id,
        &row.display_id,
        from,
        to,
        verb,
        "framework",
        gate,
        None,
        Some(REVIEW_AGENT_NAME),
    )?;
    tx.commit()?;
    Ok(())
}

fn fire_task_external_review_revise(
    conn: &Connection,
    task_id: &str,
    policies_hash: &str,
) -> Result<()> {
    let schema = crate::flow::builtins::load_tasks_schema()?;
    let tx = conn.unchecked_transaction()?;
    let (row_id, existing) = crate::handlers::row::read_row(&schema, &tx, task_id)?;
    let current_status = existing
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let current_cycle = existing
        .get("current_cycle")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let bumped_cycle = current_cycle + 1;
    let mut diff: EntryMap = BTreeMap::new();
    let mut merged = existing.clone();
    merged.insert("current_cycle".to_string(), Value::from(bumped_cycle));
    diff.insert("current_cycle".to_string(), Value::from(bumped_cycle));
    let transition = select_transition(
        &schema.lifecycle.transitions,
        &current_status,
        "submit-external-review",
        Some("REVISE"),
        &merged,
    )?;
    validate::validate(
        &schema,
        &merged,
        Op::Transition("submit-external-review".to_string(), diff.clone()),
        Actor::Framework.into(),
    )
    .map_err(|errs| {
        anyhow::anyhow!(
            "submit-external-review validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;
    let phash_opt = if policies_hash.is_empty() {
        None
    } else {
        Some(policies_hash)
    };
    execute_transition_write(
        &tx,
        &schema,
        row_id,
        task_id,
        &current_status,
        &transition.to,
        "submit-external-review",
        &diff,
        &merged,
        Actor::Framework,
        Some("REVISE"),
        phash_opt,
        Some(REVIEW_AGENT_NAME),
    )?;
    tx.commit()?;
    Ok(())
}

fn add_secs(base: &str, secs: i64) -> Option<String> {
    let epoch = parse_iso8601_to_epoch(base)?.saturating_add(secs).max(0) as u64;
    let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(epoch as i64, 0)?;
    Some(dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

fn parse_iso8601_to_epoch(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp())
}

/// Visible status rows for engine-runner/watch surfaces.
pub fn visible_status_rows(conn: &Connection) -> Result<Vec<String>> {
    if !table_exists(conn, "external_reviews")? {
        return Ok(Vec::new());
    }
    ensure_runtime_columns(conn)?;
    let mut stmt = conn.prepare(
        "SELECT er.display_id, er.task_id, er.status, COALESCE(er.runner,''), COALESCE(er.held_reason,''), COALESCE(er.next_retry_at,''), COALESCE(er.attempts,0) FROM external_reviews er WHERE er.status IN ('pending','running','tooling_held') ORDER BY er.id",
    )?;
    let rows = stmt.query_map([], |r| {
        let id: String = r.get(0)?;
        let task: String = r.get(1)?;
        let status: String = r.get(2)?;
        let runner: String = r.get(3)?;
        let held: String = r.get(4)?;
        let retry: String = r.get(5)?;
        let attempts: i64 = r.get(6)?;
        let liveness = match status.as_str() {
            "running" => "live",
            "tooling_held" => "held",
            _ if held == "cap-held" => "cap-held",
            _ => "pending",
        };
        Ok(format!(
            "external-review task_id={task} review_attempt_id={id} runner={} status={status} held_reason={} attempts={attempts} next_retry_at={} liveness={liveness}",
            if runner.is_empty() { "unknown" } else { &runner },
            if held.is_empty() { "none" } else { &held },
            if retry.is_empty() { "none" } else { &retry },
        ))
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![table],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}
