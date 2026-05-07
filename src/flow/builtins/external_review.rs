//! `builtin:external-review` — daemon lane for typed external_reviews rows.
//!
//! Runs one pending external review attempt through the configured review
//! runner, persists the verdict/status on the attempt row, and routes REVISE
//! back to the task executor lane via the framework transition.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
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

    // ── Atomic CAS gate ──────────────────────────────────────────────────────
    // Open ONE BEGIN IMMEDIATE transaction that wraps load + cap-check +
    // mark_running.  This eliminates the TOCTOU window between the status
    // check and the UPDATE that existed when two concurrent callers (Layer 2
    // state-driven dispatch + action_loop) could both pass the pending guard
    // and both call mark_running on the same row.
    //
    // Pattern mirrors promote_elapsed_tooling_held (T083 r3 precedent).
    // Transaction::new_unchecked accepts &Connection (no &mut required).

    // Test-only synchronization hook: STORES_TEST_RUN_CAS_DELAY_MS introduces a
    // sleep BEFORE opening the BEGIN IMMEDIATE transaction, after signalling
    // RUN_CAS_DELAY_HOOK_ENTERED.  This lets the concurrent-race test coordinate
    // two threads so both reach the BEGIN IMMEDIATE open at nearly the same time,
    // exposing any TOCTOU window.  Gated on debug_assertions so release builds
    // compile this out entirely.
    #[cfg(debug_assertions)]
    {
        if let Ok(ms) = std::env::var("STORES_TEST_RUN_CAS_DELAY_MS") {
            if let Ok(n) = ms.parse::<u64>() {
                RUN_CAS_DELAY_HOOK_ENTERED.store(true, std::sync::atomic::Ordering::Release);
                eprintln!("[external-review::run] pre-tx CAS delay start ({n}ms)");
                if n > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(n));
                }
                eprintln!("[external-review::run] pre-tx CAS delay end");
            }
        }
    }

    let tx = rusqlite::Transaction::new_unchecked(ctx.conn, TransactionBehavior::Immediate)?;

    let review_row = match load_review_row_tx(&tx, display_id)? {
        None => {
            // Row disappeared between the outer check and the TX — abort cleanly.
            tx.rollback()?;
            eprintln!("[external-review] {display_id}: row disappeared; skipping");
            return Ok(1);
        }
        Some(r) => r,
    };

    if review_row.status != "pending" {
        tx.rollback()?;
        eprintln!(
            "[external-review] {}: status={} not pending; skipping",
            review_row.display_id, review_row.status
        );
        return Ok(0);
    }

    let cap = resolve_review_config(ctx.config_path).max_parallel.max(1);
    let active = count_active_reviews_tx(&tx, &review_row.display_id)?;
    if active >= cap as usize {
        // Mark cap-held inside the TX so the update is visible atomically.
        mark_cap_held_tx(&tx, &review_row)?;
        tx.commit()?;
        eprintln!(
            "[external-review cap held] task_id={} review_attempt_id={} runner={} status=pending held_reason=cap-held active={} cap={} liveness=cap-held retry=next-poll",
            review_row.task_id,
            review_row.display_id,
            resolve_review_config(ctx.config_path).runner,
            active,
            cap
        );
        return Ok(0);
    }

    // CAS UPDATE: WHERE status='pending' re-checks atomically under the write lock.
    // Returns rows_affected; 0 means a concurrent caller already claimed this row.
    let affected = mark_running_tx(&tx, &review_row)?;
    if affected == 0 {
        // Race loser: another caller won the CAS; abort silently.
        tx.rollback()?;
        eprintln!(
            "[external-review] {}: CAS lost (concurrent mark_running); skipping",
            review_row.display_id
        );
        return Ok(0);
    }

    // CAS winner: insert transition history inside the same TX, then commit.
    crate::db::insert_transition_history(
        &tx,
        "external_reviews",
        review_row.row_id,
        &review_row.display_id,
        "pending",
        "running",
        "start-review",
        "framework",
        None,
        None,
        Some(REVIEW_AGENT_NAME),
    )?;
    tx.commit()?;
    // ── End atomic CAS gate ──────────────────────────────────────────────────

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
    let active = count_active_reviews(conn)?;
    if active < cap as usize {
        return Ok(true);
    }
    mark_cap_held(conn, &row)?;
    eprintln!(
        "[external-review cap held] task_id={} review_attempt_id={} runner={} status=pending held_reason=cap-held active={} cap={} liveness=cap-held retry=next-poll",
        row.task_id, row.display_id, cfg.runner, active, cap
    );
    Ok(false)
}

/// Count reviews in states that consume lane capacity.
///
/// Pi msg_ccfb6b59 requires "pending/tooling_held-eligible" accounting.
/// Both `pending` and `running` rows occupy a lane slot — `pending` rows are
/// about to be dispatched (or are cap-held candidates), and `running` rows are
/// actively executing.  Counting only `running` lets N pending + M running
/// exceed the cap on a single tick.
fn count_active_reviews(conn: &Connection) -> Result<usize> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM external_reviews WHERE status IN ('pending','running')",
        [],
        |r| r.get(0),
    )?;
    Ok(n.max(0) as usize)
}

/// Transaction-aware variant of `count_active_reviews` for use inside the
/// `run()` CAS BEGIN IMMEDIATE transaction.
///
/// Excludes the row currently being dispatched (`exclude_display_id`) from the
/// count, because that row is already locked in this tx and is about to become
/// running.  Including it would make every single-row dispatch appear to consume
/// a slot before the UPDATE fires, causing false cap-holds.
fn count_active_reviews_tx(
    tx: &rusqlite::Transaction<'_>,
    exclude_display_id: &str,
) -> Result<usize> {
    let n: i64 = tx.query_row(
        "SELECT COUNT(*) FROM external_reviews \
         WHERE status IN ('pending','running') AND display_id != ?1",
        params![exclude_display_id],
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

/// Transaction-aware variant of `mark_cap_held` for use inside the `run()` CAS
/// BEGIN IMMEDIATE transaction.
fn mark_cap_held_tx(tx: &rusqlite::Transaction<'_>, row: &ReviewRow) -> Result<()> {
    let now = now_iso8601();
    tx.execute(
        "UPDATE external_reviews SET held_reason='cap-held', updated_at=?2 WHERE display_id=?1",
        params![row.display_id, now],
    )?;
    Ok(())
}

/// Transaction-aware variant of `load_review_row` for use inside the `run()`
/// CAS BEGIN IMMEDIATE transaction.  Reads from the TX's snapshot so the load
/// and the subsequent CAS UPDATE are under the same write lock.
fn load_review_row_tx(
    tx: &rusqlite::Transaction<'_>,
    display_id: &str,
) -> Result<Option<ReviewRow>> {
    tx.query_row(
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

/// CAS UPDATE: transitions `pending → running` and returns the number of rows
/// affected.  A return value of 0 means a concurrent caller already claimed
/// this row (the WHERE status='pending' predicate failed); the caller must
/// abort silently.  A return value of 1 means this caller won the race.
///
/// Called inside the `run()` BEGIN IMMEDIATE transaction via `mark_running_tx`.
fn mark_running_tx(tx: &rusqlite::Transaction<'_>, row: &ReviewRow) -> Result<usize> {
    let now = now_iso8601();
    let affected = tx.execute(
        "UPDATE external_reviews SET status='running', started_at=COALESCE(started_at, ?2), updated_at=?2, held_reason=NULL, attempts=COALESCE(attempts,0)+1 WHERE display_id=?1 AND status='pending'",
        params![row.display_id, now],
    )?;
    Ok(affected)
}

/// Scan for `tooling_held` rows whose `next_retry_at` has elapsed and
/// transition them back to `pending` so they are picked up on the next
/// daemon iteration.  Called at the top of `run()` before the pending guard.
///
/// `pub` so that integration tests can call it directly to assert
/// atomicity (concurrent promote → single transition history record).
///
/// Atomicity: ONE `BEGIN IMMEDIATE` transaction wraps both the candidate SELECT
/// and all per-row CAS UPDATEs.  This eliminates the TOCTOU window between
/// candidate enumeration and the first per-row lock that the previous per-row
/// BEGIN IMMEDIATE pattern had.  Holding the write lock across the entire
/// scan-and-promote pass is acceptable because the candidate set is bounded —
/// only a small number of `tooling_held` rows exist at any time.
///
/// Per T079's pattern, `Transaction::new_unchecked` is used to open the
/// transaction without requiring `&mut Connection`.
pub fn promote_elapsed_tooling_held(conn: &Connection) -> Result<()> {
    let now = now_iso8601();

    // Test-only synchronization hook: STORES_TEST_PROMOTE_DELAY_MS introduces a
    // sleep AFTER signalling PROMOTE_DELAY_HOOK_ENTERED but BEFORE opening the
    // BEGIN IMMEDIATE transaction.  This lets the concurrent-race test inject a
    // second thread that attempts its own BEGIN IMMEDIATE after the first thread
    // enters the delay window, exposing any remaining TOCTOU window.
    // Gated on debug_assertions so release builds compile this out entirely.
    #[cfg(debug_assertions)]
    {
        if let Ok(ms) = std::env::var("STORES_TEST_PROMOTE_DELAY_MS") {
            if let Ok(n) = ms.parse::<u64>() {
                PROMOTE_DELAY_HOOK_ENTERED.store(true, std::sync::atomic::Ordering::Release);
                eprintln!("[external-review::promote] pre-tx delay start ({n}ms)");
                if n > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(n));
                }
                eprintln!("[external-review::promote] pre-tx delay end");
            }
        }
    }

    // Open ONE BEGIN IMMEDIATE transaction before the SELECT so that the
    // candidate read and all per-row CAS UPDATEs happen under the same write
    // lock.  Concurrent callers will block on this lock until we commit.
    //
    // Transaction::new_unchecked accepts &Connection (no &mut required),
    // matching the immutable-shared &Connection signature of this function.
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;

    // Read candidates inside the transaction.
    let candidates: Vec<(i64, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, display_id FROM external_reviews \
             WHERE status='tooling_held' AND next_retry_at IS NOT NULL AND next_retry_at <= ?1",
        )?;
        let rows = stmt
            .query_map(params![now], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };

    for (row_id, did) in candidates {
        // CAS UPDATE: the WHERE clause re-checks status='tooling_held' so
        // concurrent callers that somehow reach this point (e.g. via a
        // different lock scope) will no-op cleanly.
        let affected = tx.execute(
            "UPDATE external_reviews \
             SET status='pending', held_reason=NULL, next_retry_at=NULL, updated_at=?2 \
             WHERE display_id=?1 AND status='tooling_held' AND next_retry_at <= ?2",
            params![did, now],
        )?;
        if affected == 1 {
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
            eprintln!(
                "[external-review] {did}: tooling_held → pending (retry window elapsed)"
            );
        }
        // rows_affected == 0: row was not in tooling_held (raced or already
        // promoted); no history insert needed.
    }

    tx.commit()?;
    Ok(())
}

/// Test-only sentinel: set to `true` immediately when the delay hook inside
/// `promote_elapsed_tooling_held` is entered (before the sleep begins).
/// Tests busy-wait on this flag before opening their own BEGIN IMMEDIATE.
/// Only compiled in debug builds; absent from release binaries.
#[cfg(debug_assertions)]
pub static PROMOTE_DELAY_HOOK_ENTERED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Test-only sentinel: set to `true` immediately when the delay hook inside
/// `run()` (STORES_TEST_RUN_CAS_DELAY_MS) is entered, before the sleep.
/// Race tests busy-wait on this flag so Thread B reaches the BEGIN IMMEDIATE
/// open at nearly the same time as Thread A.
/// Only compiled in debug builds; absent from release binaries.
#[cfg(debug_assertions)]
pub static RUN_CAS_DELAY_HOOK_ENTERED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

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
