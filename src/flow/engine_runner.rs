//! Engine-runner observability substrate.
//!
//! Records per-iteration heartbeats and per-row actionability state, and
//! reconciles state invariants owned by the engine runner. Layer 1 mints
//! pending external_review rows for T2/T3 in_review tasks. Layer 2 dispatches
//! pending external_review rows whose runner is configured and no live dispatch
//! exists, closing the gap where a subscriber added after an ER row was minted
//! never fires (L055 seeder marks historical, daemon restart loses the event).

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;

use crate::codegen::ddl::quote_ident;
use crate::flow::AgentsYaml;
use crate::handlers::agents_run::pid_is_alive;
use crate::handlers::next_action::find_next_agent;
use crate::schema::Schema;
use crate::validate::EntryMap;

/// Counters persisted once per engine-runner poll iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatSummary {
    pub iteration: i64,
    pub saw_tasks: i64,
    pub saw_intake: i64,
    pub saw_observations: i64,
    pub actionable: i64,
    pub held: i64,
    pub dispatched: i64,
}

/// Latest actionability state for one substrate-visible row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionabilityRecord<'a> {
    pub store: &'a str,
    pub row_id: i64,
    pub classification: &'a str,
    pub action: Option<&'a str>,
    pub held_reason: Option<&'a str>,
    pub dispatched: bool,
    pub last_logged_at: Option<&'a str>,
}

const ACTION_LOG_THROTTLE_SECS: u64 = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExistingActionability {
    classification: String,
    action: Option<String>,
    held_reason: Option<String>,
    dispatched: bool,
    last_logged_at: Option<String>,
}

/// Schemas scanned by one engine-runner poll.
pub struct ScannerSchemas<'a> {
    pub tasks: &'a Schema,
    pub intake: &'a Schema,
    pub observations: &'a Schema,
}

/// Per-row scanner decision persisted to `engine_runner_actions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedRow {
    pub store: String,
    pub row_id: i64,
    pub classification: String,
    pub held_reason: Option<String>,
}

/// Result of one scanner pass before dispatch execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannerResult {
    pub summary: HeartbeatSummary,
    pub rows: Vec<ClassifiedRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalReviewBackfill {
    pub task_row_id: i64,
    pub task_display_id: String,
    pub review_row_id: i64,
    pub review_display_id: String,
    pub reason: String,
}

/// Outcome of a single Layer 2 external-review dispatch attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalReviewDispatchOutcome {
    /// `external_review::run()` was called; ER row transitioned out of pending.
    Dispatched,
    /// Cap full: another review is running; row held for next tick.
    CapHeld,
    /// Runner not configured in config.yaml; row skipped.
    NoRunner,
}

/// Record produced by the Layer 2 reconciler for each candidate ER row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalReviewDispatch {
    pub review_row_id: i64,
    pub review_display_id: String,
    pub task_display_id: String,
    pub outcome: ExternalReviewDispatchOutcome,
}

/// Insert one durable heartbeat row for a poll iteration.
pub fn record_heartbeat(
    conn: &Connection,
    summary: HeartbeatSummary,
    started_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO engine_runner_heartbeats \
         (iteration, started_at, saw_tasks, saw_intake, saw_observations, actionable, held, dispatched) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            summary.iteration,
            started_at,
            summary.saw_tasks,
            summary.saw_intake,
            summary.saw_observations,
            summary.actionable,
            summary.held,
            summary.dispatched,
        ],
    )
    .context("record engine_runner_heartbeats")?;
    Ok(())
}

/// Upsert the latest actionability state for one row.
pub fn upsert_actionability(
    conn: &Connection,
    record: ActionabilityRecord<'_>,
    updated_at: &str,
) -> Result<()> {
    write_actionability(conn, record, updated_at)
}

fn existing_actionability(
    conn: &Connection,
    store: &str,
    row_id: i64,
) -> Result<Option<ExistingActionability>> {
    let mut stmt = conn
        .prepare(
            "SELECT classification, action, held_reason, dispatched, last_logged_at \
             FROM engine_runner_actions WHERE store=?1 AND row_id=?2",
        )
        .context("prepare existing engine_runner_actions lookup")?;
    let mut rows = stmt.query(rusqlite::params![store, row_id])?;
    if let Some(r) = rows.next()? {
        Ok(Some(ExistingActionability {
            classification: r.get(0)?,
            action: r.get(1)?,
            held_reason: r.get(2)?,
            dispatched: r.get::<_, i64>(3)? != 0,
            last_logged_at: r.get(4)?,
        }))
    } else {
        Ok(None)
    }
}

fn write_actionability(
    conn: &Connection,
    record: ActionabilityRecord<'_>,
    updated_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO engine_runner_actions \
         (store, row_id, classification, action, held_reason, dispatched, last_logged_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(store, row_id) DO UPDATE SET \
         classification=excluded.classification, \
         action=excluded.action, \
         held_reason=excluded.held_reason, \
         dispatched=excluded.dispatched, \
         last_logged_at=excluded.last_logged_at, \
         updated_at=excluded.updated_at",
        rusqlite::params![
            record.store,
            record.row_id,
            record.classification,
            record.action,
            record.held_reason,
            if record.dispatched { 1 } else { 0 },
            record.last_logged_at,
            updated_at,
        ],
    )
    .context("upsert engine_runner_actions")?;
    Ok(())
}

fn should_log_actionability(
    existing: Option<&ExistingActionability>,
    record: &ActionabilityRecord<'_>,
    updated_at: &str,
) -> bool {
    let Some(existing) = existing else {
        return true;
    };
    if existing.classification != record.classification
        || existing.action.as_deref() != record.action
        || existing.held_reason.as_deref() != record.held_reason
        || existing.dispatched != record.dispatched
    {
        return true;
    }
    existing
        .last_logged_at
        .as_deref()
        .and_then(parse_iso8601_to_epoch)
        .zip(parse_iso8601_to_epoch(updated_at))
        .is_none_or(|(last, now)| now.saturating_sub(last) >= ACTION_LOG_THROTTLE_SECS as i64)
}

fn display_id_for(conn: &Connection, store: &str, row_id: i64) -> Option<String> {
    let table = quote_ident(store);
    let sql = format!("SELECT display_id FROM {table} WHERE id=?1");
    conn.query_row(&sql, rusqlite::params![row_id], |r| r.get(0))
        .ok()
}

fn log_actionability(conn: &Connection, record: &ActionabilityRecord<'_>) {
    let display_id = display_id_for(conn, record.store, record.row_id)
        .unwrap_or_else(|| record.row_id.to_string());
    eprintln!(
        "[engine-runner] row store={} display_id={} row_id={} classification={} action={} reason={} dispatched={}",
        record.store,
        display_id,
        record.row_id,
        record.classification,
        record.action.unwrap_or("none"),
        record.held_reason.unwrap_or("none"),
        if record.dispatched { 1 } else { 0 }
    );
}

fn upsert_actionability_throttled_log(
    conn: &Connection,
    record: ActionabilityRecord<'_>,
    updated_at: &str,
) -> Result<bool> {
    let existing = existing_actionability(conn, record.store, record.row_id)?;
    let log = should_log_actionability(existing.as_ref(), &record, updated_at);
    let last_logged_at = if log {
        Some(updated_at)
    } else {
        existing.as_ref().and_then(|e| e.last_logged_at.as_deref())
    };
    write_actionability(
        conn,
        ActionabilityRecord {
            last_logged_at,
            ..record
        },
        updated_at,
    )?;
    if log {
        log_actionability(conn, &record);
    }
    Ok(log)
}

/// Scan active rows, classify actionability, and persist one heartbeat plus
/// per-row latest actionability records. This function is lifecycle read-only:
/// it writes only `engine_runner_heartbeats` and `engine_runner_actions`.
pub fn scan_and_record_actionability(
    conn: &Connection,
    schemas: ScannerSchemas<'_>,
    iteration: i64,
    started_at: &str,
) -> Result<ScannerResult> {
    let rows = scan_rows(conn, schemas, started_at, 300)?;
    let summary = summarize_rows(iteration, &rows, 0);

    record_heartbeat(conn, summary, started_at)?;
    for row in &rows {
        upsert_actionability_throttled_log(
            conn,
            ActionabilityRecord {
                store: &row.store,
                row_id: row.row_id,
                classification: &row.classification,
                action: None,
                held_reason: row.held_reason.as_deref(),
                dispatched: false,
                last_logged_at: Some(started_at),
            },
            started_at,
        )?;
    }

    Ok(ScannerResult { summary, rows })
}

/// Reconcile the external_reviews lane invariant for T2/T3 in_review tasks.
pub fn reconcile_external_reviews_for_in_review_tasks(
    conn: &Connection,
) -> Result<Vec<ExternalReviewBackfill>> {
    if !table_exists(conn, "tasks")? || !table_exists(conn, "external_reviews")? {
        return Ok(Vec::new());
    }

    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let candidates: Vec<(i64, String, Option<String>)> = {
        let mut stmt = tx.prepare(
            "SELECT id, display_id, wrap_log FROM tasks \
             WHERE status='in_review' AND tier_hint IN ('T2','T3') \
             ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows
    };

    let mut minted = Vec::new();
    for (task_row_id, task_display_id, wrap_log) in candidates {
        let active: i64 = tx.query_row(
            "SELECT COUNT(*) FROM external_reviews \
             WHERE task_id=?1 \
               AND status IN ('pending','running','passed','revise','tooling_held')",
            rusqlite::params![task_display_id],
            |r| r.get(0),
        )?;
        if active > 0 {
            continue;
        }

        let next_attempt: i64 = tx.query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM external_reviews WHERE task_id=?1",
            rusqlite::params![task_display_id],
            |r| r.get(0),
        )?;
        let next_id: i64 = tx.query_row(
            "SELECT COALESCE(MAX(id), 0) + 1 FROM external_reviews",
            [],
            |r| r.get(0),
        )?;
        let review_display_id = format!("ER{next_id:03}");
        let now = crate::handlers::row::now_iso8601();
        let wrap_len = wrap_log
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.as_array().map(Vec::len))
            .unwrap_or(0);
        let wrap_ref = if wrap_len == 0 {
            format!("tasks:{task_display_id}:wrap_log")
        } else {
            format!("tasks:{task_display_id}:wrap_log[{}]", wrap_len - 1)
        };
        tx.execute(
            "INSERT INTO external_reviews \
             (display_id, status, created_at, updated_at, created_by, updated_by, \
              task_id, attempt, adapter, contract_ref, plan_ref, wrap_log_ref, diff_ref, prior_review_ref) \
             VALUES (?1, 'pending', ?2, ?2, 'framework', 'framework', \
                     ?3, ?4, 'external_review', ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                review_display_id,
                now,
                task_display_id,
                next_attempt,
                format!("tasks:{task_display_id}:contract"),
                format!("tasks:{task_display_id}:plan"),
                wrap_ref,
                format!("tasks:{task_display_id}:diff"),
                format!("tasks:{task_display_id}:cycles"),
            ],
        )
        .context("engine-runner: create pending external_review backfill")?;
        let review_row_id = tx.last_insert_rowid();
        let reason = format!("external_review backfilled for {task_display_id} (wrap pre-deploy)");
        crate::db::insert_transition_history(
            &tx,
            "external_reviews",
            review_row_id,
            &review_display_id,
            "",
            "pending",
            "create-external-review",
            "framework",
            None,
            None,
            Some(&format!(
                "source=engine_runner_reconcile task_row_id={task_row_id} task_display_id={task_display_id} reason={reason}"
            )),
        )?;
        minted.push(ExternalReviewBackfill {
            task_row_id,
            task_display_id,
            review_row_id,
            review_display_id,
            reason,
        });
    }
    tx.commit()?;
    for row in &minted {
        eprintln!("[engine-runner] {}", row.reason);
    }
    Ok(minted)
}

/// Layer 2 reconciler: state-driven dispatch of pending external_reviews rows.
///
/// Invariant: every `external_reviews` row in status=`pending` whose runner is
/// configured AND whose dispatch_lock (if any) has `finished_at IS NOT NULL`
/// (i.e., no live claim) MUST eventually be dispatched.  This is NOT guaranteed
/// by the transition subscriber alone: the L055/L116 seeder marks pre-existing
/// `""→pending` TH rows as `skip-historical` when the `external-review`
/// subscriber is first added, so rows minted before the subscriber existed
/// (e.g. ER001 in the ER001-backfill scenario) never get dispatched via the
/// transition path.  Daemon restarts after a missed transition share the same
/// gap.
///
/// This function fills that gap by scanning `external_reviews` directly each
/// engine-runner tick and calling `external_review::run()` for unserviced rows.
///
/// Idempotency:
///   - Status filter: only `status='pending'` rows are candidates; `run()`
///     transitions `pending→running` atomically, so a row won't be re-dispatched
///     once it leaves pending.
///   - dispatch_locks guard: rows with a live unfinished dispatch_lock
///     (`finished_at IS NULL`) are skipped — the action_loop is already
///     handling them.
///   - `run()` CAS: `UPDATE … WHERE status='pending'` is the final gate; a
///     concurrent `run()` from the action_loop path will no-op if it sees
///     status≠pending.
///
/// Cap: `cap_allows_or_log` is called per row; CapHeld rows are returned with
/// the held outcome so callers can see the lane is full.
///
/// BEGIN IMMEDIATE scope: the candidates SELECT is done OUTSIDE a transaction
/// (read-only snapshot is sufficient); each `run()` call internally uses its
/// own BEGIN IMMEDIATE where needed (promote_elapsed_tooling_held + mark_running).
/// No wrapping transaction is needed here because each `run()` is an independent
/// unit; holding a writer lock across all rows would starve the action_loop
/// unnecessarily.
pub fn reconcile_pending_external_review_dispatch(
    conn: &Connection,
    agents: &AgentsYaml,
    config_path: &Path,
    policies_hash: &str,
) -> Result<Vec<ExternalReviewDispatch>> {
    if !table_exists(conn, "external_reviews")? || !table_exists(conn, "dispatch_locks")? {
        return Ok(Vec::new());
    }

    // Gate: only run if the `external-review` agent is present in agents.yaml.
    // Without an agent entry, `cap_allows_or_log` would still work but there is
    // no configured subscriber to hand-off to; skip cleanly.
    if !agents
        .agents
        .iter()
        .any(|a| a.name == "external-review")
    {
        return Ok(Vec::new());
    }

    // Skip immediately if the runner is explicitly empty (mis-configuration).
    let review_cfg = crate::flow::config::resolve_review_config(config_path);
    if review_cfg.runner.is_empty() {
        return Ok(Vec::new());
    }

    // Ensure runtime columns exist (held_reason, next_retry_at, attempts).
    // safe to call repeatedly; external_review::ensure_runtime_columns is private
    // so we call visible_status_rows which calls it internally, OR inline the
    // columns check.  Use cap_allows_or_log which calls ensure_runtime_columns.
    // Actually: call visible_status_rows which invokes ensure_runtime_columns.
    // Cheapest path: just call ensure_runtime_columns via an exported helper.
    // Since it's private we inline the required column additions here.
    {
        let expected = [
            ("held_reason", "TEXT"),
            ("next_retry_at", "TEXT"),
            ("attempts", "INTEGER DEFAULT 0"),
        ];
        let mut cols_stmt = conn.prepare("PRAGMA table_info(external_reviews)")?;
        let existing: std::collections::BTreeSet<String> = cols_stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .collect::<std::result::Result<_, _>>()?;
        for (name, ty) in expected {
            if !existing.contains(name) {
                conn.execute(
                    &format!("ALTER TABLE external_reviews ADD COLUMN {name} {ty}"),
                    [],
                )
                .context("Layer2: add external_reviews runtime column")?;
            }
        }
    }

    // Promote elapsed tooling_held rows before candidate enumeration so retry-review
    // rows re-enter the same Layer 2 pending-dispatch path on this tick.
    crate::flow::builtins::external_review::promote_elapsed_tooling_held(conn)
        .context("Layer2: promote elapsed tooling_held external_reviews")?;

    // Candidates: pending ER rows with no live (unfinished) dispatch_lock.
    // "Live" = dispatch_lock row exists AND finished_at IS NULL.
    // Rows with finished locks (skip-historical or completed) are re-eligible.
    let mut stmt = conn.prepare(
        "SELECT er.id, er.display_id, er.task_id \
         FROM external_reviews er \
         WHERE er.status = 'pending' \
           AND NOT EXISTS ( \
               SELECT 1 FROM dispatch_locks dl \
               WHERE dl.store = 'external_reviews' \
                 AND dl.row_id = er.id \
                 AND dl.agent_name = 'external-review' \
                 AND dl.finished_at IS NULL \
           ) \
         ORDER BY er.id",
    )?;
    let candidates: Vec<(i64, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    // Pi msg_577e80a3: per-tick dispatch budget = cap - running_at_tick_start.
    // Pending rows are queue backlog; they do not consume running capacity.
    // We compute the budget once at tick start so that synchronous dispatches
    // that complete before the next candidate check don't inflate the budget.
    let cap = review_cfg.max_parallel.max(1) as usize;
    let running_at_start =
        crate::flow::builtins::external_review::count_running_reviews(conn).unwrap_or(0);
    let budget = cap.saturating_sub(running_at_start);
    let mut dispatched_this_tick: usize = 0;

    let mut results = Vec::new();
    for (review_row_id, review_display_id, task_display_id) in candidates {
        // Per-tick budget gate: stop dispatching once we've reached cap.
        // budget = cap - running_at_tick_start; once exhausted, mark remaining
        // candidates cap-held so they surface in visible_status_rows.
        if dispatched_this_tick >= budget {
            if let Err(e) = crate::flow::builtins::external_review::mark_cap_held_by_display_id(
                conn,
                &review_display_id,
            ) {
                eprintln!(
                    "[engine-runner Layer2] mark_cap_held error for {review_display_id}: {e}"
                );
            }
            eprintln!(
                "[engine-runner Layer2 cap held] review_attempt_id={review_display_id} budget_exhausted dispatched_this_tick={dispatched_this_tick} cap={cap} running_at_start={running_at_start}"
            );
            results.push(ExternalReviewDispatch {
                review_row_id,
                review_display_id,
                task_display_id,
                outcome: ExternalReviewDispatchOutcome::CapHeld,
            });
            continue;
        }

        // Cap check via the exported helper (marks the row cap-held if needed).
        // This is the per-row in-database safety check (guards against concurrent
        // callers from the action_loop path racing with Layer 2).
        match crate::flow::builtins::external_review::cap_allows_or_log(
            conn,
            config_path,
            &review_display_id,
        ) {
            Ok(true) => {}
            Ok(false) => {
                results.push(ExternalReviewDispatch {
                    review_row_id,
                    review_display_id,
                    task_display_id,
                    outcome: ExternalReviewDispatchOutcome::CapHeld,
                });
                continue;
            }
            Err(e) => {
                eprintln!(
                    "[engine-runner Layer2] cap_allows_or_log error for {review_display_id}: {e}"
                );
                continue;
            }
        }

        let row_json = serde_json::json!({"display_id": review_display_id});
        let ctx = crate::flow::builtins::DispatchCtx {
            conn,
            agents,
            config_path,
            policies_hash,
        };
        eprintln!(
            "[engine-runner Layer2] dispatching pending external_review {review_display_id} (task={task_display_id})"
        );
        match crate::flow::builtins::external_review::run(&row_json, &ctx) {
            Ok(crate::flow::builtins::external_review::DispatchOutcome::Dispatched) => {
                // CAS winner: budget consumed.
                dispatched_this_tick += 1;
                results.push(ExternalReviewDispatch {
                    review_row_id,
                    review_display_id,
                    task_display_id,
                    outcome: ExternalReviewDispatchOutcome::Dispatched,
                });
            }
            Ok(crate::flow::builtins::external_review::DispatchOutcome::CapHeld) => {
                // In-TX cap held: row marked held inside run(); no budget consumed.
                eprintln!(
                    "[engine-runner Layer2] {review_display_id}: in-TX cap held (no budget consumed)"
                );
                results.push(ExternalReviewDispatch {
                    review_row_id,
                    review_display_id,
                    task_display_id,
                    outcome: ExternalReviewDispatchOutcome::CapHeld,
                });
            }
            Ok(crate::flow::builtins::external_review::DispatchOutcome::RaceLost) => {
                // CAS loser or row disappeared: no budget consumed; log only.
                eprintln!(
                    "[engine-runner Layer2] {review_display_id}: CAS lost or row gone (no budget consumed)"
                );
            }
            Err(e) => {
                eprintln!(
                    "[engine-runner Layer2] external_review::run error for {review_display_id}: {e}"
                );
            }
        }
    }
    Ok(results)
}

/// Scan and execute only existing autonomous task re-drive edges. Non-task
/// actionable rows remain observation-only for this phase.
pub fn scan_record_and_redrive_tasks(
    conn: &Connection,
    schemas: ScannerSchemas<'_>,
    iteration: i64,
    started_at: &str,
    agents: &AgentsYaml,
    config_path: &Path,
    policies_hash: &str,
    // Dispatches already issued by the daemon's base poll loop this iteration.
    // Folded into the heartbeat row so the persisted record matches the log line.
    base_dispatched: i64,
) -> Result<ScannerResult> {
    let claim_window_secs = auto_drive_claim_window_secs(agents);
    // Layer 1: mint pending external_review rows for T2/T3 in_review tasks.
    let backfilled = reconcile_external_reviews_for_in_review_tasks(conn)?;
    // Layer 2: dispatch pending external_review rows that have no live claim.
    // Runs after Layer 1 so freshly-minted rows from THIS tick are picked up
    // immediately rather than waiting for the next daemon iteration.
    let l2_dispatches =
        reconcile_pending_external_review_dispatch(conn, agents, config_path, policies_hash)
            .unwrap_or_else(|e| {
                eprintln!("[engine-runner Layer2] reconcile error: {e}");
                Vec::new()
            });
    let l2_dispatched = l2_dispatches
        .iter()
        .filter(|d| d.outcome == ExternalReviewDispatchOutcome::Dispatched)
        .count() as i64;
    let mut rows = scan_rows(conn, schemas, started_at, claim_window_secs)?;
    for backfill in &backfilled {
        if let Some(row) = rows
            .iter_mut()
            .find(|r| r.store == "tasks" && r.row_id == backfill.task_row_id)
        {
            row.classification = "held".to_string();
            row.held_reason = Some(backfill.reason.clone());
        }
    }
    let cap = crate::flow::config::resolve_drive_max_parallel(config_path) as usize;
    let mut dispatched = l2_dispatched;

    let mut processed_task_rows = std::collections::BTreeSet::new();
    for row in rows.iter_mut().filter(|r| {
        r.store == "tasks"
            && r.classification == "actionable_task_redrive"
            && r.held_reason.is_none()
    }) {
        processed_task_rows.insert(row.row_id);
        let occupied =
            count_active_auto_drive_capacity(conn, started_at, claim_window_secs).unwrap_or(0);
        if occupied >= cap {
            row.classification = "held".to_string();
            row.held_reason = Some("lane_cap_full".to_string());
            upsert_actionability_throttled_log(
                conn,
                ActionabilityRecord {
                    store: &row.store,
                    row_id: row.row_id,
                    classification: &row.classification,
                    action: None,
                    held_reason: row.held_reason.as_deref(),
                    dispatched: false,
                    last_logged_at: Some(started_at),
                },
                started_at,
            )?;
            continue;
        }

        match crate::flow::builtins::auto_drive::redispatch_orphaned_next_agent(
            conn,
            row.row_id,
            agents,
            config_path,
            policies_hash,
        )? {
            Some(_) => {
                row.classification = "dispatched_task_redrive".to_string();
                dispatched += 1;
                upsert_actionability_throttled_log(
                    conn,
                    ActionabilityRecord {
                        store: &row.store,
                        row_id: row.row_id,
                        classification: &row.classification,
                        action: Some("redispatched"),
                        held_reason: None,
                        dispatched: true,
                        last_logged_at: Some(started_at),
                    },
                    started_at,
                )?;
            }
            None => {
                row.classification = "held".to_string();
                row.held_reason = Some("redrive_noop".to_string());
                upsert_actionability_throttled_log(
                    conn,
                    ActionabilityRecord {
                        store: &row.store,
                        row_id: row.row_id,
                        classification: &row.classification,
                        action: None,
                        held_reason: row.held_reason.as_deref(),
                        dispatched: false,
                        last_logged_at: Some(started_at),
                    },
                    started_at,
                )?;
            }
        }
    }

    for row in rows
        .iter()
        .filter(|r| !(r.store == "tasks" && processed_task_rows.contains(&r.row_id)))
    {
        upsert_actionability_throttled_log(
            conn,
            ActionabilityRecord {
                store: &row.store,
                row_id: row.row_id,
                classification: &row.classification,
                action: None,
                held_reason: row.held_reason.as_deref(),
                dispatched: false,
                last_logged_at: Some(started_at),
            },
            started_at,
        )?;
    }

    // Fold base_dispatched into the summary BEFORE recording the heartbeat so
    // the persisted row and the caller's log line both see the same union count.
    let summary = summarize_rows(iteration, &rows, dispatched + base_dispatched);
    record_heartbeat(conn, summary, started_at)?;
    Ok(ScannerResult { summary, rows })
}

fn scan_rows(
    conn: &Connection,
    schemas: ScannerSchemas<'_>,
    started_at: &str,
    claim_window_secs: u64,
) -> Result<Vec<ClassifiedRow>> {
    let mut rows = Vec::new();
    rows.extend(scan_tasks(
        conn,
        schemas.tasks,
        started_at,
        claim_window_secs,
    )?);
    rows.extend(scan_intake(conn, schemas.intake)?);
    rows.extend(scan_observations(conn, schemas.observations)?);
    Ok(rows)
}

fn summarize_rows(iteration: i64, rows: &[ClassifiedRow], dispatched: i64) -> HeartbeatSummary {
    HeartbeatSummary {
        iteration,
        saw_tasks: rows.iter().filter(|r| r.store == "tasks").count() as i64,
        saw_intake: rows.iter().filter(|r| r.store == "intake").count() as i64,
        saw_observations: rows.iter().filter(|r| r.store == "observations").count() as i64,
        actionable: rows
            .iter()
            .filter(|r| r.held_reason.is_none() && r.classification.starts_with("actionable_"))
            .count() as i64,
        held: rows.iter().filter(|r| r.held_reason.is_some()).count() as i64,
        dispatched,
    }
}

fn scan_tasks(
    conn: &Connection,
    schema: &Schema,
    started_at: &str,
    claim_window_secs: u64,
) -> Result<Vec<ClassifiedRow>> {
    let table = quote_ident(&schema.name);
    let sql = format!(
        "SELECT id, status, current_phase, current_cycle, tier_hint, plan, blocked_reason, drive_pid \
         FROM {table} WHERE status IN ('planning','plan_review','ready','executing','code_review','complete','in_review','blocked')"
    );
    let mut stmt = conn.prepare(&sql).context("prepare task scanner")?;
    let mut out = Vec::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<i64>>(7)?,
        ))
    })?;

    let workflow = schema.workflow.as_ref();
    for row in rows {
        let (
            row_id,
            status,
            current_phase,
            current_cycle,
            tier_hint,
            plan,
            blocked_reason,
            drive_pid,
        ) = row?;
        let mut entry: EntryMap = BTreeMap::new();
        entry.insert("status".into(), json!(status));
        entry.insert("current_phase".into(), opt_i64_value(current_phase));
        entry.insert("current_cycle".into(), opt_i64_value(current_cycle));
        entry.insert("tier_hint".into(), opt_string_value(tier_hint.clone()));
        entry.insert("blocked_reason".into(), opt_string_value(blocked_reason.clone()));
        entry.insert("plan".into(), parse_json_text(plan));

        let rate_limit_until = blocked_reason.as_deref().and_then(rate_limit_blocked_until);
        let next_agent = workflow.and_then(|wf| find_next_agent(wf, &status, &entry));
        // Known limitation: drive_pid liveness uses kill(pid, 0) which does not
        // protect against PID reuse. If the OS reuses the PID for an unrelated
        // process between drive exit and engine-runner observation, the row will
        // appear live and be held as `live_drive_owner` indefinitely — until the
        // stale process exits or an operator intervenes. Owner-identity hardening
        // (pid + start_time / dispatch_lock owner identity) is a future
        // scheduler/dispatch ownership task — not in T079/L186 phase-1 scope.
        let live_drive_owner = drive_pid
            .and_then(|pid| i32::try_from(pid).ok())
            .is_some_and(pid_is_alive);
        let (classification, held_reason) = if let Some(until) = rate_limit_until {
            (
                "held".to_string(),
                Some(format!("rate_limit_cooldown_until:{until}")),
            )
        } else if status == "blocked" {
            ("held".to_string(), Some("blocked".to_string()))
        } else if let Some(_agent) = next_agent {
            if status == "in_review" {
                (
                    "held".to_string(),
                    Some("no_autonomous_reviewer_runner".to_string()),
                )
            } else if live_drive_owner {
                ("held".to_string(), Some("live_drive_owner".to_string()))
            } else if has_live_dispatch_lock(
                conn,
                &schema.name,
                row_id,
                "auto-drive",
                started_at,
                claim_window_secs,
            )? {
                ("held".to_string(), Some("live_dispatch_lock".to_string()))
            } else {
                ("actionable_task_redrive".to_string(), None)
            }
        } else {
            ("held".to_string(), Some("no_next_agent".to_string()))
        };
        let held_reason =
            if status == "in_review" && matches!(tier_hint.as_deref(), Some("T2") | Some("T3")) {
                external_review_backfill_reason(conn, row_id)?.or(held_reason)
            } else {
                held_reason
            };
        out.push(ClassifiedRow {
            store: schema.name.clone(),
            row_id,
            classification,
            held_reason,
        });
    }
    Ok(out)
}

fn rate_limit_blocked_until(reason: &str) -> Option<&str> {
    let rest = reason.strip_prefix("rate_limit:")?;
    let (_provider, until) = rest.split_once(':')?;
    if is_iso8601_cooldown(until) { Some(until) } else { None }
}

fn is_iso8601_cooldown(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20
        || b.get(4) != Some(&b'-')
        || b.get(7) != Some(&b'-')
        || b.get(10) != Some(&b'T')
        || b.get(13) != Some(&b':')
        || b.get(16) != Some(&b':')
    {
        return false;
    }
    b[0..4].iter().all(u8::is_ascii_digit)
        && b[5..7].iter().all(u8::is_ascii_digit)
        && b[8..10].iter().all(u8::is_ascii_digit)
        && b[11..13].iter().all(u8::is_ascii_digit)
        && b[14..16].iter().all(u8::is_ascii_digit)
        && b[17..19].iter().all(u8::is_ascii_digit)
        && (s.ends_with('Z') || s[19..].contains('+') || s[19..].contains('-'))
}

fn external_review_backfill_reason(conn: &Connection, task_row_id: i64) -> Result<Option<String>> {
    if !table_exists(conn, "external_reviews")? {
        return Ok(None);
    }
    conn.query_row(
        "SELECT 'external_review backfilled for ' || t.display_id || ' (wrap pre-deploy)' \
         FROM tasks t \
         JOIN external_reviews er ON er.task_id=t.display_id \
         JOIN transition_history th ON th.store='external_reviews' \
          AND th.row_id=er.id \
          AND th.verb='create-external-review' \
          AND th.invoker='framework' \
         WHERE t.id=?1 AND er.status='pending' \
         ORDER BY er.attempt DESC, er.id DESC LIMIT 1",
        rusqlite::params![task_row_id],
        |r| r.get(0),
    )
    .optional()
    .context("lookup external_review backfill actionability reason")
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        rusqlite::params![table],
        |r| r.get(0),
    )?;
    Ok(exists > 0)
}

fn scan_intake(conn: &Connection, schema: &Schema) -> Result<Vec<ClassifiedRow>> {
    let table = quote_ident(&schema.name);
    let sql = format!("SELECT id, status FROM {table} WHERE status IN ('triaging','needs_info')");
    let mut stmt = conn.prepare(&sql).context("prepare intake scanner")?;
    let workflow = schema.workflow.as_ref();
    let mut out = Vec::new();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (row_id, status) = row?;
        let entry: EntryMap = BTreeMap::new();
        let next_agent = workflow.and_then(|wf| find_next_agent(wf, &status, &entry));
        let (classification, held_reason) = match next_agent.as_deref() {
            Some(_) => (
                "held".to_string(),
                Some("no_built_in_entrypoint".to_string()),
            ),
            None => ("held".to_string(), Some("no_next_agent".to_string())),
        };
        out.push(ClassifiedRow {
            store: schema.name.clone(),
            row_id,
            classification,
            held_reason,
        });
    }
    Ok(out)
}

fn scan_observations(conn: &Connection, schema: &Schema) -> Result<Vec<ClassifiedRow>> {
    let table = quote_ident(&schema.name);
    let sql = format!(
        "SELECT id, status, intent_contract, risk_class, approval_policy \
         FROM {table} WHERE status IN ('open','needs_investigation','investigating','investigated','confirmed','ready','needs_info','in_progress')"
    );
    let mut stmt = conn.prepare(&sql).context("prepare observations scanner")?;
    let mut out = Vec::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    })?;
    for row in rows {
        let (row_id, status, contract_text, risk_class, approval_policy) = row?;
        let contract = parse_json_text(contract_text);
        let contract_state = contract
            .get("contract_state")
            .and_then(Value::as_str)
            .unwrap_or("");
        let approved_by = contract.get("approved_by").and_then(Value::as_str);
        let approved_at = contract.get("approved_at").and_then(Value::as_str);
        let arch_surface = risk_class.as_deref() == Some("architecture")
            || approval_policy.as_deref() == Some("architecture");
        let awaiting_human_contract =
            contract_state == "draft" || approved_by.is_none() || approved_at.is_none();
        let (classification, held_reason) = if arch_surface {
            ("held".to_string(), Some("needs_architect".to_string()))
        } else if awaiting_human_contract {
            ("held".to_string(), Some("needs_human".to_string()))
        } else if matches!(status.as_str(), "investigated" | "confirmed" | "ready") {
            ("held".to_string(), Some("needs_human".to_string()))
        } else if status == "needs_investigation" {
            (
                "held".to_string(),
                Some("no_built_in_entrypoint".to_string()),
            )
        } else {
            (
                "held".to_string(),
                Some("no_built_in_entrypoint".to_string()),
            )
        };
        out.push(ClassifiedRow {
            store: schema.name.clone(),
            row_id,
            classification,
            held_reason,
        });
    }
    Ok(out)
}

fn has_live_dispatch_lock(
    conn: &Connection,
    store: &str,
    row_id: i64,
    agent: &str,
    started_at: &str,
    claim_window_secs: u64,
) -> Result<bool> {
    let cutoff = iso8601_sub_secs(started_at, claim_window_secs).unwrap_or_default();
    let mut stmt = conn.prepare(
        "SELECT COALESCE(pid, 0) FROM dispatch_locks \
         WHERE store=?1 AND row_id=?2 AND agent_name=?3 AND finished_at IS NULL \
           AND COALESCE(claimed_at, '') >= ?4",
    )?;
    let pids: Vec<i64> = stmt
        .query_map(rusqlite::params![store, row_id, agent, cutoff], |r| {
            r.get(0)
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(pids
        .into_iter()
        .any(|pid| pid <= 0 || pid_is_alive(pid as i32)))
}

/// Count occupied auto-drive capacity as the union of active task owners and
/// in-window unfinished auto-drive locks. Same task row counted once.
pub(crate) fn count_active_auto_drive_capacity(
    conn: &Connection,
    started_at: &str,
    claim_window_secs: u64,
) -> Result<usize> {
    let mut occupied = std::collections::BTreeSet::new();

    let mut task_stmt =
        conn.prepare("SELECT id, drive_pid FROM tasks WHERE drive_pid IS NOT NULL")?;
    let task_rows = task_stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    for row in task_rows.filter_map(|r| r.ok()) {
        let (row_id, pid) = row;
        if pid_is_alive(pid as i32) {
            occupied.insert(row_id);
        }
    }

    let cutoff = iso8601_sub_secs(started_at, claim_window_secs).unwrap_or_default();
    let mut lock_stmt = conn.prepare(
        "SELECT row_id, COALESCE(pid, 0) FROM dispatch_locks \
         WHERE store='tasks' AND agent_name='auto-drive' AND finished_at IS NULL \
           AND COALESCE(claimed_at, '') >= ?1",
    )?;
    let lock_rows = lock_stmt.query_map(rusqlite::params![cutoff], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
    })?;
    for row in lock_rows.filter_map(|r| r.ok()) {
        let (row_id, pid) = row;
        if pid <= 0 || pid_is_alive(pid as i32) {
            occupied.insert(row_id);
        }
    }

    Ok(occupied.len())
}

fn auto_drive_claim_window_secs(agents: &AgentsYaml) -> u64 {
    agents
        .agents
        .iter()
        .find(|a| a.name == "auto-drive")
        .map(|a| a.claim_window_secs)
        .unwrap_or(300)
}

fn iso8601_sub_secs(base: &str, secs: u64) -> Option<String> {
    let epoch = parse_iso8601_to_epoch(base)?;
    let shifted = epoch.saturating_sub(secs as i64).max(0) as u64;
    let (y, mo, d, h, mi, se) = unix_to_ymd_hms(shifted);
    Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{se:02}Z"))
}

fn parse_iso8601_to_epoch(s: &str) -> Option<i64> {
    if s.len() < 19 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y: u32 = std::str::from_utf8(&b[0..4]).ok()?.parse().ok()?;
    let mo: u32 = std::str::from_utf8(&b[5..7]).ok()?.parse().ok()?;
    let d: u32 = std::str::from_utf8(&b[8..10]).ok()?.parse().ok()?;
    let h: u32 = std::str::from_utf8(&b[11..13]).ok()?.parse().ok()?;
    let mi: u32 = std::str::from_utf8(&b[14..16]).ok()?.parse().ok()?;
    let se: u32 = std::str::from_utf8(&b[17..19]).ok()?.parse().ok()?;
    Some(ymd_hms_to_epoch(y, mo, d, h, mi, se))
}

fn ymd_hms_to_epoch(y: u32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    fn days_in_month(y: u32, m: u32) -> u32 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if is_leap(y) {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        }
    }
    let mut days = 0_i64;
    for yy in 1970..y {
        days += if is_leap(yy) { 366 } else { 365 };
    }
    for mm in 1..mo {
        days += i64::from(days_in_month(y, mm));
    }
    days += i64::from(d.saturating_sub(1));
    days * 86_400 + i64::from(h) * 3_600 + i64::from(mi) * 60 + i64::from(s)
}

fn unix_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let s = secs % 60;
    let total_min = secs / 60;
    let mi = total_min % 60;
    let total_hr = total_min / 60;
    let h = total_hr % 24;
    let mut days = total_hr / 24;
    let mut year = 1970_u32;
    loop {
        let dy = if is_leap(year) { 366 } else { 365 } as u64;
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let dim = [
        31_u32,
        if is_leap(year) { 29 } else { 28 },
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
    let mut month = 0_usize;
    let mut d = days as u32;
    while month < dim.len() && d >= dim[month] {
        d -= dim[month];
        month += 1;
    }
    (year, month as u32 + 1, d + 1, h as u32, mi as u32, s as u32)
}

fn is_leap(y: u32) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

fn opt_i64_value(v: Option<i64>) -> Value {
    v.map(Value::from).unwrap_or(Value::Null)
}

fn opt_string_value(v: Option<String>) -> Value {
    v.map(Value::from).unwrap_or(Value::Null)
}

fn parse_json_text(v: Option<String>) -> Value {
    v.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::flow::AgentsYaml;
    use crate::schema::Schema;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn actionability_upsert_replaces_latest_state_for_row() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();

        upsert_actionability(
            &conn,
            ActionabilityRecord {
                store: "tasks",
                row_id: 7,
                classification: "held",
                action: None,
                held_reason: Some("needs_human"),
                dispatched: false,
                last_logged_at: Some("2026-05-07T00:00:00Z"),
            },
            "2026-05-07T00:00:01Z",
        )
        .unwrap();
        upsert_actionability(
            &conn,
            ActionabilityRecord {
                store: "tasks",
                row_id: 7,
                classification: "actionable",
                action: Some("redispatched"),
                held_reason: Some("orphaned_next_agent"),
                dispatched: true,
                last_logged_at: Some("2026-05-07T00:01:00Z"),
            },
            "2026-05-07T00:01:01Z",
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM engine_runner_actions WHERE store='tasks' AND row_id=7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let row: (String, Option<String>, i64, String) = conn
            .query_row(
                "SELECT classification, held_reason, dispatched, updated_at \
                 FROM engine_runner_actions WHERE store='tasks' AND row_id=7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "actionable".to_string(),
                Some("orphaned_next_agent".to_string()),
                1,
                "2026-05-07T00:01:01Z".to_string(),
            )
        );
    }

    #[test]
    fn throttled_actionability_log_preserves_last_logged_before_window() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();

        let first_logged = upsert_actionability_throttled_log(
            &conn,
            ActionabilityRecord {
                store: "tasks",
                row_id: 8,
                classification: "held",
                action: None,
                held_reason: Some("needs_human"),
                dispatched: false,
                last_logged_at: None,
            },
            "2026-05-07T00:00:00Z",
        )
        .unwrap();
        let second_logged = upsert_actionability_throttled_log(
            &conn,
            ActionabilityRecord {
                store: "tasks",
                row_id: 8,
                classification: "held",
                action: None,
                held_reason: Some("needs_human"),
                dispatched: false,
                last_logged_at: None,
            },
            "2026-05-07T00:04:59Z",
        )
        .unwrap();

        assert!(first_logged);
        assert!(!second_logged);
        let row: (Option<String>, String) = conn
            .query_row(
                "SELECT last_logged_at, updated_at FROM engine_runner_actions WHERE store='tasks' AND row_id=8",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0.as_deref(), Some("2026-05-07T00:00:00Z"));
        assert_eq!(row.1, "2026-05-07T00:04:59Z");
    }

    #[test]
    fn actionability_state_change_logs_and_updates_last_logged() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();

        upsert_actionability_throttled_log(
            &conn,
            ActionabilityRecord {
                store: "tasks",
                row_id: 9,
                classification: "held",
                action: None,
                held_reason: Some("lane_cap_full"),
                dispatched: false,
                last_logged_at: None,
            },
            "2026-05-07T00:00:00Z",
        )
        .unwrap();
        let changed_logged = upsert_actionability_throttled_log(
            &conn,
            ActionabilityRecord {
                store: "tasks",
                row_id: 9,
                classification: "dispatched_task_redrive",
                action: Some("redispatched"),
                held_reason: None,
                dispatched: true,
                last_logged_at: None,
            },
            "2026-05-07T00:00:01Z",
        )
        .unwrap();

        assert!(changed_logged);
        let row: (String, Option<String>, Option<String>, i64, Option<String>) = conn
            .query_row(
                "SELECT classification, action, held_reason, dispatched, last_logged_at FROM engine_runner_actions WHERE store='tasks' AND row_id=9",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(row.0, "dispatched_task_redrive");
        assert_eq!(row.1.as_deref(), Some("redispatched"));
        assert_eq!(row.2, None);
        assert_eq!(row.3, 1);
        assert_eq!(row.4.as_deref(), Some("2026-05-07T00:00:01Z"));
    }

    fn scanner_schemas() -> (Schema, Schema, Schema) {
        (
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap(),
            Schema::from_yaml(include_str!("../../stores/intake_items/schema.yaml")).unwrap(),
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap(),
        )
    }

    fn open_scanner_db() -> (Connection, Schema, Schema, Schema) {
        let (tasks, intake, observations) = scanner_schemas();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&ddl_for(&tasks)).unwrap();
        conn.execute_batch(&ddl_for(&intake)).unwrap();
        conn.execute_batch(&ddl_for(&observations)).unwrap();
        (conn, tasks, intake, observations)
    }

    fn insert_task(conn: &Connection, display_id: &str, status: &str) -> i64 {
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, title, slug, current_phase, current_cycle, tier_hint, plan) \
             VALUES (?1, ?2, '2026-05-07T00:00:00Z', '2026-05-07T00:00:00Z', 'Task', 'task', 1, 1, 'T2', ?3)",
            rusqlite::params![display_id, status, r#"{"phases":[{"name":"p1"}]}"#],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn rate_limit_blocked_task_is_held_with_cooldown_reason() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let row_id = insert_task(&conn, "T429", "blocked");
        conn.execute(
            "UPDATE tasks SET blocked_reason=?1 WHERE id=?2",
            rusqlite::params!["rate_limit:anthropic:2099-01-01T00:00:00Z", row_id],
        )
        .unwrap();
        let rows = scan_rows(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            "2026-05-07T00:00:00Z",
            300,
        )
        .unwrap();
        let row = rows.iter().find(|r| r.store == "tasks" && r.row_id == row_id).unwrap();
        assert_eq!(row.classification, "held");
        assert_eq!(
            row.held_reason.as_deref(),
            Some("rate_limit_cooldown_until:2099-01-01T00:00:00Z")
        );
    }

    fn insert_intake(conn: &Connection, display_id: &str, status: &str) -> i64 {
        conn.execute(
            "INSERT INTO intake (display_id, status, created_at, updated_at, summary, source_agent, captured_at, captured_week) \
             VALUES (?1, ?2, '2026-05-07T00:00:00Z', '2026-05-07T00:00:00Z', 'Intake', 'tester', '2026-05-07T00:00:00Z', 'w18-d4')",
            rusqlite::params![display_id, status],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_observation(
        conn: &Connection,
        display_id: &str,
        status: &str,
        contract: &str,
        risk_class: &str,
        approval_policy: &str,
    ) -> i64 {
        conn.execute(
            "INSERT INTO observations (display_id, status, created_at, updated_at, summary, source, priority, captured_at, captured_week, intent_contract, risk_class, approval_policy) \
             VALUES (?1, ?2, '2026-05-07T00:00:00Z', '2026-05-07T00:00:00Z', 'Observation', 'qa', 'normal', '2026-05-07T00:00:00Z', 'w18-d4', ?3, ?4, ?5)",
            rusqlite::params![display_id, status, contract, risk_class, approval_policy],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn scanner_counts_tasks_intake_observations_and_actionable_task_redrive() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T901", "executing");
        insert_intake(&conn, "I901", "triaging");
        insert_observation(
            &conn,
            "L901",
            "needs_investigation",
            "{}",
            "normal",
            "human",
        );

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            1,
            "2026-05-07T00:00:00Z",
        )
        .unwrap();

        assert_eq!(result.summary.saw_tasks, 1);
        assert_eq!(result.summary.saw_intake, 1);
        assert_eq!(result.summary.saw_observations, 1);
        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.classification == "actionable_task_redrive"
            && r.held_reason.is_none()));

        let persisted: String = conn
            .query_row(
                "SELECT classification FROM engine_runner_actions WHERE store='tasks' AND row_id=?1",
                rusqlite::params![task_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(persisted, "actionable_task_redrive");
    }

    #[test]
    fn scanner_treats_stale_drive_pid_as_orphaned_task_redrive() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T905", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![999_999_999_i64, task_id],
        )
        .unwrap();

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            5,
            "2026-05-07T00:04:00Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.classification == "actionable_task_redrive"
            && r.held_reason.is_none()));
    }

    #[test]
    fn scanner_holds_live_drive_pid_as_owner() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T906", "code_review");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![std::process::id() as i64, task_id],
        )
        .unwrap();

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            6,
            "2026-05-07T00:05:00Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.classification == "held"
            && r.held_reason.as_deref() == Some("live_drive_owner")));
    }

    #[test]
    fn action_loop_redispatches_dead_pid_orphan() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("STORES_DRIVE_CMD", "sleep 5 #");
        let (conn, tasks, intake, observations) = open_scanner_db();
        let tmp = tempfile::tempdir().unwrap();
        let task_id = insert_task(&conn, "T907", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1, workspace_path=?2 WHERE id=?3",
            rusqlite::params![999_999_999_i64, tmp.path().to_str().unwrap(), task_id],
        )
        .unwrap();
        let cfg = tmp.path().join("config.yaml");
        std::fs::write(&cfg, "drive:\n  max_parallel: 5\n").unwrap();

        scan_record_and_redrive_tasks(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            8,
            "2026-05-07T00:07:00Z",
            &AgentsYaml::default_empty(),
            &cfg,
            "",
            0,
        )
        .unwrap();

        let (pid, action, agent): (i64, Option<String>, String) = conn
            .query_row(
                "SELECT t.drive_pid, a.action, dl.agent_name \
                 FROM tasks t \
                 JOIN engine_runner_actions a ON a.row_id=t.id AND a.store='tasks' \
                 JOIN dispatch_locks dl ON dl.row_id=t.id AND dl.store='tasks' \
                 WHERE t.id=?1",
                rusqlite::params![task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(pid > 0 && pid != 999_999_999_i64);
        assert!(pid_is_alive(pid as i32));
        assert_eq!(action.as_deref(), Some("redispatched"));
        assert_eq!(agent, "auto-drive");
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        std::env::remove_var("STORES_DRIVE_CMD");
    }

    #[test]
    fn action_loop_live_pid_is_held_noop() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T908", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![std::process::id() as i64, task_id],
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.yaml");

        scan_record_and_redrive_tasks(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            9,
            "2026-05-07T00:08:00Z",
            &AgentsYaml::default_empty(),
            &cfg,
            "",
            0,
        )
        .unwrap();

        let (held, dispatched, locks): (Option<String>, i64, i64) = conn
            .query_row(
                "SELECT a.held_reason, a.dispatched, \
                        (SELECT COUNT(*) FROM dispatch_locks WHERE row_id=?1 AND agent_name='auto-drive') \
                 FROM engine_runner_actions a WHERE a.store='tasks' AND a.row_id=?1",
                rusqlite::params![task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(held.as_deref(), Some("live_drive_owner"));
        assert_eq!(dispatched, 0);
        assert_eq!(locks, 0);
    }

    #[test]
    fn action_loop_respects_drive_lane_cap() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let live_id = insert_task(&conn, "T909", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![std::process::id() as i64, live_id],
        )
        .unwrap();
        let orphan_id = insert_task(&conn, "T910", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![999_999_998_i64, orphan_id],
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.yaml");
        std::fs::write(&cfg, "drive:\n  max_parallel: 1\n").unwrap();

        scan_record_and_redrive_tasks(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            10,
            "2026-05-07T00:09:00Z",
            &AgentsYaml::default_empty(),
            &cfg,
            "",
            0,
        )
        .unwrap();

        let held: Option<String> = conn
            .query_row(
                "SELECT held_reason FROM engine_runner_actions WHERE store='tasks' AND row_id=?1",
                rusqlite::params![orphan_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(held.as_deref(), Some("lane_cap_full"));
        let locks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id=?1 AND agent_name='auto-drive'",
                rusqlite::params![orphan_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(locks, 0);
    }

    #[test]
    fn scanner_ignores_stale_dispatch_lock_outside_claim_window() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T911", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![999_999_997_i64, task_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, claimed_at, claimed_by) \
             VALUES ('tasks', ?1, 'T911', 'auto-drive', '2026-05-07T00:00:00Z', 'daemon')",
            rusqlite::params![task_id],
        )
        .unwrap();

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            11,
            "2026-05-07T00:10:01Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.classification == "actionable_task_redrive"
            && r.held_reason.is_none()));
    }

    #[test]
    fn scanner_holds_recent_dispatch_lock_within_claim_window() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T912", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![999_999_996_i64, task_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, claimed_at, claimed_by, pid) \
             VALUES ('tasks', ?1, 'T912', 'auto-drive', '2026-05-07T00:08:00Z', 'daemon', ?2)",
            rusqlite::params![task_id, std::process::id() as i64],
        )
        .unwrap();

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            12,
            "2026-05-07T00:10:00Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.classification == "held"
            && r.held_reason.as_deref() == Some("live_dispatch_lock")));
    }

    #[test]
    fn action_loop_lane_cap_counts_recent_dispatch_locks() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let occupied_id = insert_task(&conn, "T913", "executing");
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, claimed_at, claimed_by, pid) \
             VALUES ('tasks', ?1, 'T913', 'auto-drive', '2026-05-07T00:09:00Z', 'daemon', ?2)",
            rusqlite::params![occupied_id, std::process::id() as i64],
        )
        .unwrap();
        let orphan_id = insert_task(&conn, "T914", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![999_999_995_i64, orphan_id],
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.yaml");
        std::fs::write(&cfg, "drive:\n  max_parallel: 1\n").unwrap();

        scan_record_and_redrive_tasks(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            13,
            "2026-05-07T00:10:00Z",
            &AgentsYaml::default_empty(),
            &cfg,
            "",
            0,
        )
        .unwrap();

        let (held, locks): (Option<String>, i64) = conn
            .query_row(
                "SELECT a.held_reason, \
                        (SELECT COUNT(*) FROM dispatch_locks WHERE row_id=?1 AND agent_name='auto-drive') \
                 FROM engine_runner_actions a WHERE a.store='tasks' AND a.row_id=?1",
                rusqlite::params![orphan_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(held.as_deref(), Some("lane_cap_full"));
        assert_eq!(locks, 0);
    }

    #[test]
    fn action_loop_lane_cap_counts_mixed_live_pid_and_lock_capacity() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let live_id = insert_task(&conn, "T915", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![std::process::id() as i64, live_id],
        )
        .unwrap();
        let locked_id = insert_task(&conn, "T916", "executing");
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, claimed_at, claimed_by, pid) \
             VALUES ('tasks', ?1, 'T916', 'auto-drive', '2026-05-07T00:09:00Z', 'daemon', 0)",
            rusqlite::params![locked_id],
        )
        .unwrap();
        let orphan_id = insert_task(&conn, "T917", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![999_999_994_i64, orphan_id],
        )
        .unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.yaml");
        std::fs::write(&cfg, "drive:\n  max_parallel: 2\n").unwrap();

        scan_record_and_redrive_tasks(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            14,
            "2026-05-07T00:10:00Z",
            &AgentsYaml::default_empty(),
            &cfg,
            "",
            0,
        )
        .unwrap();

        let (held, orphan_locks): (Option<String>, i64) = conn
            .query_row(
                "SELECT a.held_reason, \
                        (SELECT COUNT(*) FROM dispatch_locks WHERE row_id=?1 AND agent_name='auto-drive') \
                 FROM engine_runner_actions a WHERE a.store='tasks' AND a.row_id=?1",
                rusqlite::params![orphan_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(held.as_deref(), Some("lane_cap_full"));
        assert_eq!(orphan_locks, 0);
    }

    #[test]
    fn scanner_holds_u_moment_observation_without_lifecycle_writes() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let original_contract = r#"{"contract_state":"draft","approved_by":null,"approved_at":null,"objective":"draft"}"#;
        let obs_id = insert_observation(
            &conn,
            "L902",
            "investigating",
            original_contract,
            "normal",
            "human",
        );

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            2,
            "2026-05-07T00:01:00Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "observations"
            && r.row_id == obs_id
            && r.classification == "held"
            && r.held_reason.as_deref() == Some("needs_human")));
        let row: (String, String, String, String) = conn
            .query_row(
                "SELECT status, intent_contract, risk_class, approval_policy FROM observations WHERE id=?1",
                rusqlite::params![obs_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, "investigating");
        assert_eq!(row.1, original_contract);
        assert_eq!(row.2, "normal");
        assert_eq!(row.3, "human");

        let forbidden: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE verb IN ('accept','reject','resume','amend','abandon','confirm','ratify') OR verb LIKE 'architecture%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(forbidden, 0);
    }

    #[test]
    fn scanner_holds_non_investigating_draft_contract_as_needs_human() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let original_contract = r#"{"contract_state":"draft","approved_by":null,"approved_at":null,"objective":"draft"}"#;
        let obs_id =
            insert_observation(&conn, "L904", "open", original_contract, "normal", "human");

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            7,
            "2026-05-07T00:06:00Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "observations"
            && r.row_id == obs_id
            && r.classification == "held"
            && r.held_reason.as_deref() == Some("needs_human")));
        let row: (String, String) = conn
            .query_row(
                "SELECT status, intent_contract FROM observations WHERE id=?1",
                rusqlite::params![obs_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "open");
        assert_eq!(row.1, original_contract);

        let forbidden: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE verb IN ('accept','reject','resume','amend','abandon','confirm','ratify') OR verb LIKE 'architecture%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(forbidden, 0);
    }

    #[test]
    fn scanner_holds_architecture_surface_for_architect() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let obs_id = insert_observation(
            &conn,
            "L903",
            "confirmed",
            "{}",
            "architecture",
            "architecture",
        );

        scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            3,
            "2026-05-07T00:02:00Z",
        )
        .unwrap();

        let held: Option<String> = conn
            .query_row(
                "SELECT held_reason FROM engine_runner_actions WHERE store='observations' AND row_id=?1",
                rusqlite::params![obs_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(held.as_deref(), Some("needs_architect"));
    }

    #[test]
    fn scanner_respects_live_task_dispatch_lock() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T904", "code_review");
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, claimed_at, claimed_by) \
             VALUES ('tasks', ?1, 'T904', 'auto-drive', '2026-05-07T00:00:00Z', 'daemon')",
            rusqlite::params![task_id],
        )
        .unwrap();

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            4,
            "2026-05-07T00:03:00Z",
        )
        .unwrap();

        assert_eq!(result.summary.actionable, 0);
        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.held_reason.as_deref() == Some("live_dispatch_lock")));
    }

    #[test]
    fn heartbeat_helper_inserts_only_heartbeat_row() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        record_heartbeat(
            &conn,
            HeartbeatSummary {
                iteration: 1,
                saw_tasks: 2,
                saw_intake: 3,
                saw_observations: 4,
                actionable: 5,
                held: 6,
                dispatched: 7,
            },
            "2026-05-07T00:00:00Z",
        )
        .unwrap();
        let row: (i64, i64, i64) = conn
            .query_row(
                "SELECT saw_tasks, held, dispatched FROM engine_runner_heartbeats \
                 WHERE iteration=1 AND started_at='2026-05-07T00:00:00Z'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, (2, 6, 7));
    }
}
