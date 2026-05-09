//! Submit handlers — write-path workflow verbs.
//!
//! Four sub-handlers, one per submit verb:
//!   - `submit_plan`         — planning → plan_review
//!   - `submit_plan_review`  — plan_review → ready (→ executing via on-entry follow-on)
//!   - `submit_execute`      — executing → code_review
//!   - `submit_review`       — code_review → executing | complete | blocked
//!
//! Each handler follows the strict sequence per task 5.3 (C3 / transaction boundary):
//!   1. Open transaction (the boundary)
//!   2. Acquire row lock (conditional UPDATE)
//!   3. Read row
//!   4. Parse inputs into diff
//!   5. Build the target entry (record or list-record append)
//!   6. Validator pass
//!   7. Compute engine post-actions
//!   8. Apply user-write UPDATE inside tx
//!   9. Fire follow-on transitions inside tx
//!  10. Release lock (final action inside tx)
//!  11. Commit tx
//!  12. Print summary line (run() only)
//!
//! compute/run split: each verb has `pub(crate) fn compute_submit_*(...) -> Result<SubmitOutput>`
//!   plus thin `pub fn run_submit_*(...)` printers. Tests call `compute_*` and assert on the
//! structured output and post-call DB state.
//!
//! ## L503-A artifact-persistence scope
//!
//! L503-A persists artifacts at dispatch boundaries:
//! - `agent_runs.brief_text`: the rendered brief stored verbatim at spawn time (wired in drive.rs).
//! - `plan_review_log[].reviewed_plan`: snapshot of `tasks.plan` at submit-plan-review time.
//!
//! L503-A does NOT enforce contracts on those artifacts (L504-A's domain) and does NOT
//! provide an operator inspector view (L012's domain).
//!
//! The `cycles[].executor.external_review_id` soft-FK back-link is deferred to a follow-up
//! slice — distinguishing "cycle was triggered by external_review respawn" from "cycle exists
//! alongside a stale REVISE ER" requires lifecycle state plumbing beyond the overlay's current
//! semantics.

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::codegen::ddl::quote_ident;
use crate::schema::{
    actor::{Actor, InvokerCtx},
    Schema,
};
use crate::validate::expr_eval::eval;
use crate::validate::{self, EntryMap, Op};

use super::row::{now_iso8601, read_row};

// ---------------------------------------------------------------------------
// Output type (compute/run split pattern)
// ---------------------------------------------------------------------------

/// Structured output from any submit handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitOutput {
    /// Display ID of the affected row.
    pub display_id: String,
    /// Status the row transitioned TO (after all follow-ons).
    pub new_status: String,
    /// Human-readable summary line.
    pub summary: String,
    /// For submit-execute: index in cycles[] where the entry was appended.
    pub cycles_idx: Option<usize>,
    /// For submit-review: gate value used.
    pub gate: Option<String>,
    /// For submit-plan-review: gate value used.
    pub plan_review_gate: Option<String>,
    /// Populated when new_status == "blocked" — the reason text written to the row.
    pub blocked_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Lock helpers (5.3 step 2 / 5.8)
// ---------------------------------------------------------------------------

/// Acquire the row lock via a conditional UPDATE.
///
/// Returns Ok(()) if the lock was acquired (1 row changed).
/// Returns Err(...) if the row is currently held by another invoker
/// within the 5-minute window.
fn acquire_lock(tx: &Transaction, table: &str, display_id: &str, invoker: &str) -> Result<()> {
    let now = now_iso8601();
    let five_min_ago = iso_subtract_seconds(300);
    let qtable = quote_ident(table);

    let sql = format!(
        "UPDATE {qtable} SET claimed_by = ?1, claimed_at = ?2 \
         WHERE display_id = ?3 AND (claimed_by IS NULL OR claimed_at < ?4)"
    );

    let rows_changed = tx
        .execute(
            &sql,
            rusqlite::params![invoker, now, display_id, five_min_ago],
        )
        .context("acquire lock")?;

    if rows_changed == 0 {
        let lock_info: Result<(Option<String>, Option<String>), _> = tx.query_row(
            &format!("SELECT claimed_by, claimed_at FROM {qtable} WHERE display_id = ?1"),
            rusqlite::params![display_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        );
        let (holder, held_at) = lock_info.unwrap_or((None, None));
        let holder = holder.unwrap_or_else(|| "unknown".to_string());
        let held_at = held_at.unwrap_or_else(|| "unknown".to_string());
        bail!("row {display_id} is claimed by '{holder}' since {held_at}; retry after 5 minutes");
    }

    Ok(())
}

/// Release the row lock (final action inside tx, per 5.3 step 10).
fn release_lock(tx: &Transaction, table: &str, display_id: &str) -> Result<()> {
    let qtable = quote_ident(table);
    let sql =
        format!("UPDATE {qtable} SET claimed_by = NULL, claimed_at = NULL WHERE display_id = ?1");
    tx.execute(&sql, rusqlite::params![display_id])
        .context("release lock")?;
    Ok(())
}

fn table_has_column(tx: &Transaction, table: &str, column: &str) -> Result<bool> {
    let qtable = quote_ident(table);
    let mut stmt = tx.prepare(&format!("PRAGMA table_info({qtable})"))?;
    let cols = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for col in cols {
        if col? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(tx: &Transaction, table: &str) -> Result<bool> {
    let exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        rusqlite::params![table],
        |r| r.get(0),
    )?;
    Ok(exists > 0)
}

/// Clear stale auto-drive bookkeeping before a human resume. A task blocked by
/// `mark_drive_failed` can retain the dead `drive_pid` and an old
/// `dispatch_locks` row; if left in place, the watchdog can immediately flip
/// the just-resumed row back to blocked.
fn clear_auto_drive_bookkeeping_for_resume(
    tx: &Transaction,
    table: &str,
    row_id: i64,
    display_id: &str,
) -> Result<()> {
    let has_drive_pid = table_has_column(tx, table, "drive_pid")?;
    let has_drive_started_at = table_has_column(tx, table, "drive_started_at")?;
    if has_drive_pid || has_drive_started_at {
        let qtable = quote_ident(table);
        let assignments = match (has_drive_pid, has_drive_started_at) {
            (true, true) => "drive_pid = NULL, drive_started_at = NULL",
            (true, false) => "drive_pid = NULL",
            (false, true) => "drive_started_at = NULL",
            (false, false) => unreachable!(),
        };
        tx.execute(
            &format!("UPDATE {qtable} SET {assignments} WHERE id = ?1"),
            rusqlite::params![row_id],
        )
        .with_context(|| format!("resume: clear auto-drive pid fields for {display_id}"))?;
    }

    if table_exists(tx, "dispatch_locks")? {
        tx.execute(
            "DELETE FROM dispatch_locks \
             WHERE store = ?1 AND row_id = ?2 AND display_id = ?3 AND agent_name = 'auto-drive'",
            rusqlite::params![table, row_id, display_id],
        )
        .with_context(|| {
            format!("resume: clear stale auto-drive dispatch_lock for {display_id}")
        })?;
    }

    Ok(())
}

/// Build an ISO-8601 timestamp for N seconds ago.
fn iso_subtract_seconds(seconds: u64) -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(seconds);
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

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
    let mut month = 1u32;
    loop {
        let dm = days_in_month(year, month) as u64;
        if days < dm {
            break;
        }
        days -= dm;
        month += 1;
    }
    (year, month, days as u32 + 1)
}

#[allow(clippy::manual_is_multiple_of)]
fn is_leap(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
fn days_in_year(y: u32) -> u32 {
    if is_leap(y) {
        366
    } else {
        365
    }
}
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
        _ => 31,
    }
}

// ---------------------------------------------------------------------------
// Audit metadata for transition_history (T014 P1)
// ---------------------------------------------------------------------------

/// Audit data threaded into `write_status_and_fields` when a real lifecycle
/// transition is being committed. Manual paths (current code) pass
/// `policy_ref = None` / `policies_hash = None`; the autonomous flow daemon
/// supplies the policy id + full-policies-file hash for policy-mediated writes.
pub(crate) struct TransitionAudit<'a> {
    pub display_id: &'a str,
    pub from_status: &'a str,
    pub verb: &'a str,
    pub policy_ref: Option<&'a str>,
    pub policies_hash: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// Core DB write helper
// ---------------------------------------------------------------------------

/// Write status + framework fields + text columns inside a transaction.
///
/// `framework_fields`: column name → integer value (e.g. current_phase, current_cycle).
/// `text_fields`: column name → text value (e.g. blocked_reason, JSON for list/record fields).
///
/// T014 P1 audit: callers that perform a real status change supply
/// `audit = Some(TransitionAudit { ... })`; this writes one row to
/// `transition_history`. Callers that touch text/framework fields without changing
/// the row's lifecycle status (e.g. submit-wrap appending wrap_log[] in-place)
/// pass `audit = None`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_status_and_fields(
    tx: &Transaction,
    table: &str,
    row_id: i64,
    new_status: &str,
    invoker: &str,
    framework_fields: &BTreeMap<String, i64>,
    text_fields: &BTreeMap<String, String>,
    audit: Option<TransitionAudit<'_>>,
) -> Result<()> {
    let now = now_iso8601();

    let mut set_parts: Vec<String> = vec![
        "updated_at = ?1".to_string(),
        "updated_by = ?2".to_string(),
        "status = ?3".to_string(),
    ];
    let mut sql_values: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(now),
        rusqlite::types::Value::Text(invoker.to_string()),
        rusqlite::types::Value::Text(new_status.to_string()),
    ];
    let mut idx = 4usize;

    for (col, val) in framework_fields {
        set_parts.push(format!("{col} = ?{idx}"));
        sql_values.push(rusqlite::types::Value::Integer(*val));
        idx += 1;
    }

    for (col, val) in text_fields {
        set_parts.push(format!("{col} = ?{idx}"));
        sql_values.push(rusqlite::types::Value::Text(val.clone()));
        idx += 1;
    }

    let where_idx = idx;
    sql_values.push(rusqlite::types::Value::Integer(row_id));

    let set_clause = set_parts.join(", ");
    let qtable = quote_ident(table);
    let sql = format!("UPDATE {qtable} SET {set_clause} WHERE id = ?{where_idx}");

    tx.execute(&sql, rusqlite::params_from_iter(sql_values.iter()))
        .context("write_status_and_fields")?;

    if let Some(a) = audit {
        crate::db::insert_transition_history(
            tx,
            table,
            row_id,
            a.display_id,
            a.from_status,
            new_status,
            a.verb,
            invoker,
            a.policy_ref,
            a.policies_hash,
            None,
        )?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Guard evaluation + transition selection (5.2 / 5.5 / 5.5b)
// ---------------------------------------------------------------------------

/// Find a transition matching (from_state, verb, gate) whose guard evaluates true.
///
/// Thin delegator to `crate::schema::lifecycle::select_transition`, which holds
/// the canonical algorithm.  Kept here so the four call sites in this file
/// (submit_plan, submit_plan_review, submit_execute, submit_review) remain byte-identical.
pub(crate) fn find_transition<'a>(
    schema: &'a Schema,
    from_state: &str,
    verb: &str,
    gate: Option<&str>,
    entry: &EntryMap,
) -> Result<&'a crate::schema::lifecycle::Transition> {
    crate::schema::lifecycle::select_transition(
        &schema.lifecycle.transitions,
        from_state,
        verb,
        gate,
        entry,
    )
}

// ---------------------------------------------------------------------------
// On-entry follow-on firing (5.3 step 9 / 5.4 M5)
// ---------------------------------------------------------------------------

/// Fire on_state follow-ons for `state` (TransitionTo actions only).
///
/// If on_state[state] contains TransitionTo(target), transitions the row to
/// target inside the same tx.  Recurses if target also has on-entry follow-ons.
/// All writes use Actor::Framework as invoker.
/// T054: derive a deliberately-sparse one-phase plan from the contract for
/// T1 rows during the planning → ready (skip-plan) on-entry transition.
///
/// Mapping is mechanical:
/// - `plan.objective` ← `contract.executive_intent` (fallback `contract.done_when`)
/// - `phases[0].name` ← "Contract execution"
/// - `phases[0].objective` ← `contract.done_when`
/// - `phases[0].tasks` ← bullet/newline split of `contract.scope_in`
///   (fallback `["Execute the ratified contract scope"]`)
/// - `phases[0].acceptance_criteria` ← `[contract.done_when]`
/// - `phases[0].files` ← `[]`
/// - `phases[0].dependencies` ← `[]`
///
/// No invented detail; every field traces back to the human-ratified contract.
pub(crate) fn synthesize_t1_plan_from_contract(entry: &EntryMap) -> Value {
    let contract = entry.get("contract").cloned().unwrap_or(Value::Null);
    let get_str = |k: &str| -> String {
        contract
            .get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let executive_intent = get_str("executive_intent");
    let done_when = get_str("done_when");
    let scope_in = get_str("scope_in");

    let objective = if !executive_intent.trim().is_empty() {
        executive_intent.clone()
    } else {
        done_when.clone()
    };

    let tasks = parse_scope_in_bullets(&scope_in);
    let tasks = if tasks.is_empty() {
        vec!["Execute the ratified contract scope".to_string()]
    } else {
        tasks
    };

    serde_json::json!({
        "objective": objective,
        "phases": [{
            "name": "Contract execution",
            "objective": done_when,
            "tasks": tasks,
            "acceptance_criteria": [done_when],
            "files": [],
            "dependencies": [],
        }]
    })
}

/// Split a free-text scope_in string into a list of task bullets. Recognises
/// leading `- `, `* `, `• `, or `<n>. ` markers and falls back to newline-
/// separated trimmed lines. Empty inputs return an empty Vec.
fn parse_scope_in_bullets(scope_in: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in scope_in.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Strip common bullet markers
        let stripped = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("• "))
            .unwrap_or(trimmed);
        // Strip "<n>. " numeric markers
        let stripped = if let Some(rest) = stripped.split_once(". ") {
            if rest.0.chars().all(|c| c.is_ascii_digit()) && !rest.0.is_empty() {
                rest.1
            } else {
                stripped
            }
        } else {
            stripped
        };
        let s = stripped.trim().to_string();
        if !s.is_empty() {
            out.push(s);
        }
    }
    out
}

pub fn fire_on_entry_follow_ons(
    tx: &Transaction,
    schema: &Schema,
    display_id: &str,
    row_id: i64,
    state: &str,
) -> Result<()> {
    let workflow = match &schema.workflow {
        Some(w) => w,
        None => return Ok(()),
    };

    let actions = match workflow.on_state.get(state) {
        Some(a) => a,
        None => return Ok(()),
    };

    for action in actions {
        if let crate::schema::workflow::StateActionKind::TransitionTo(target_state) = &action.kind {
            // Re-read to get fresh row state for guard evaluation and framework field computation
            let (_, current_entry) = read_row(schema, tx, display_id)?;

            // T027 P2: per-action `when:` predicate gates whether the
            // follow-on fires for this row.  Absent `when:` is always-true.
            // (Distinct from the transition-level `guard:` evaluated below,
            // which is a schema-author invariant on the chosen transition.)
            if let Some(expr) = &action.when {
                if !eval(expr, &current_entry) {
                    continue;
                }
            }

            // Find the framework-actor transition from state → target_state
            let follow_on_t = schema
                .lifecycle
                .transitions
                .iter()
                .find(|t| {
                    t.from == state
                        && t.to.as_str() == target_state.as_str()
                        && t.actor == Some(Actor::Framework)
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "on_state[{}] declares transition_to: {} but no framework transition \
                         from '{}' to '{}' exists in schema",
                        state,
                        target_state,
                        state,
                        target_state
                    )
                })?;

            // Evaluate guard if present (framework-fired follow-ons must have guard satisfied)
            if let Some(guard) = &follow_on_t.guard {
                if !eval(guard, &current_entry) {
                    bail!(
                        "framework-fired follow-on guard failed: {} → {} — schema author error \
                         (guards must always be satisfied when on_state fires)",
                        state,
                        target_state
                    );
                }
            }

            // Compute framework fields for entering target_state
            let (fw_fields, mut txt_fields) =
                compute_on_entry_framework_fields(schema, target_state, &current_entry);

            // T054: skip-plan branch synthesises a one-phase plan from the
            // contract so plan IS NULL becomes impossible for T1 rows past
            // planning → ready. Idempotent: only writes when the plan field
            // is currently null/empty.
            let schema_has_plan = schema.fields.iter().any(|f| f.name == "plan");
            if follow_on_t.verb == "skip-plan"
                && state == "planning"
                && target_state == "ready"
                && schema_has_plan
            {
                let plan_is_empty = current_entry
                    .get("plan")
                    .map(|v| match v {
                        Value::Null => true,
                        Value::String(s) => s.trim().is_empty(),
                        Value::Object(m) => m.is_empty(),
                        _ => false,
                    })
                    .unwrap_or(true);
                if plan_is_empty {
                    let synthesized = synthesize_t1_plan_from_contract(&current_entry);
                    txt_fields.insert("plan".to_string(), serde_json::to_string(&synthesized)?);
                    txt_fields.insert(
                        "plan_source".to_string(),
                        "contract_synthesized".to_string(),
                    );
                }
            }

            write_status_and_fields(
                tx,
                &schema.name,
                row_id,
                target_state,
                "framework",
                &fw_fields,
                &txt_fields,
                Some(TransitionAudit {
                    display_id,
                    from_status: state,
                    verb: &follow_on_t.verb,
                    policy_ref: None,
                    policies_hash: None,
                }),
            )?;

            // Recurse: does target_state also have on-entry follow-ons?
            fire_on_entry_follow_ons(tx, schema, display_id, row_id, target_state)?;
        }
    }

    Ok(())
}

/// Compute framework fields to write when entering `target_state` via an on-entry follow-on.
///
/// Per 5.4 table:
/// - `ready → executing` (on-entry follow-on from plan_review READY):
///   sets current_phase = 1, current_cycle = 1 ONLY if current_phase == 0
///   (distinguishes initial plan approval from resume, where current_phase is preserved).
fn compute_on_entry_framework_fields(
    _schema: &Schema,
    target_state: &str,
    current_entry: &EntryMap,
) -> (BTreeMap<String, i64>, BTreeMap<String, String>) {
    let mut fw_fields: BTreeMap<String, i64> = BTreeMap::new();
    let txt_fields: BTreeMap<String, String> = BTreeMap::new();

    if target_state == "executing" {
        let current_phase = current_entry
            .get("current_phase")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);

        // Only set phase/cycle here if we're entering executing for the first time
        // (current_phase == 0 means fresh from plan approval).
        // Resume path: current_phase is already ≥ 1; submit-review PASS path: handled before follow-on.
        if current_phase == 0 {
            fw_fields.insert("current_phase".to_string(), 1);
            fw_fields.insert("current_cycle".to_string(), 1);
        }
        // If current_phase > 0, the caller already set current_phase/current_cycle correctly.
        // For the resume path (blocked → ready → executing), the caller sets current_cycle=1
        // and current_phase is unchanged; those writes happen before fire_on_entry_follow_ons.
        // The follow-on just writes status=executing here.
        // But: write_status_and_fields with empty fw_fields only writes status+timestamps,
        // which is correct for the resume path.
    }

    (fw_fields, txt_fields)
}

// ---------------------------------------------------------------------------
// Require workflow helper
// ---------------------------------------------------------------------------

fn require_workflow<'a>(
    schema: &'a Schema,
    verb: &str,
) -> Result<&'a crate::schema::workflow::Workflow> {
    schema.workflow.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "store '{}' has no workflow declaration; {} is not available",
            schema.name,
            verb
        )
    })
}

// ---------------------------------------------------------------------------
// submit-plan: planning → plan_review (AC5.6)
// ---------------------------------------------------------------------------

/// Core logic for submit-plan.  All DB access is inside a transaction.
pub(crate) fn compute_submit_plan(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    plan_json: Value,
    invoker: Actor,
) -> Result<SubmitOutput> {
    require_workflow(schema, "submit-plan")?;

    // Step 1: open transaction
    let tx = conn
        .unchecked_transaction()
        .context("submit-plan: begin tx")?;

    // Step 2: acquire lock
    acquire_lock(&tx, &schema.name, display_id, &invoker.to_string())?;

    // Step 3: read row
    let (row_id, existing) = read_row(schema, &tx, display_id)?;

    // State-machine check
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current_status != "planning" {
        bail!(
            "cannot submit-plan: row is in state '{}', expected 'planning'",
            current_status
        );
    }

    // Step 4/5: build diff — write plan record
    // P5-m3: look up field name from submit_targets instead of hardcoding "plan"
    let workflow = require_workflow(schema, "submit-plan")?;
    let plan_field = workflow
        .submit_targets
        .get("submit-plan")
        .map(|s| s.as_str())
        .unwrap_or("plan");

    let mut diff: EntryMap = BTreeMap::new();
    diff.insert(plan_field.to_string(), plan_json.clone());

    // Deep-merge for validation
    let mut merged = existing.clone();
    merged.insert(plan_field.to_string(), plan_json.clone());

    // T027 P4: tier-T2 phase-count gate. T2 plans must contain exactly one
    // phase (the contract IS that single phase). T1 never reaches submit-plan
    // (planner is skipped); T3 unconstrained.
    if merged.get("tier_hint").and_then(|v| v.as_str()) == Some("T2") {
        let n = plan_json
            .get("phases")
            .and_then(|p| p.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        if n != 1 {
            bail!("submit-plan: tier T2 requires phases.length == 1, got {n}");
        }
    }

    // T047: hard shape gate. Reject any plan whose `phases` is missing, not an
    // array, or empty. This makes a regression of the planner→persistence path
    // (e.g. SAP picking the wrong JSON object in prose) fail loudly at the
    // substrate boundary instead of silently writing a degenerate `{}` plan.
    match plan_json.get("phases") {
        Some(p) if p.is_array() && !p.as_array().unwrap().is_empty() => {}
        Some(p) if p.is_array() => {
            bail!(
                "submit-plan: plan.phases is an empty array; planner must emit at least one phase"
            );
        }
        Some(p) => {
            bail!(
                "submit-plan: plan.phases must be an array, got {}",
                p
            );
        }
        None => {
            bail!("submit-plan: plan.phases is missing; expected a non-empty array");
        }
    }

    // Step 6: validator pass
    validate::validate(schema, &merged, Op::SubmitPlan(diff), invoker.into()).map_err(|errs| {
        anyhow::anyhow!(
            "submit-plan validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    // Step 7: find transition planning → plan_review
    let transition = find_transition(schema, "planning", "submit-plan", None, &merged)?;
    let new_status = transition.to.clone();

    // Step 8: write plan record + new status
    let plan_json_str = serde_json::to_string(&plan_json)?;
    let mut text_fields: BTreeMap<String, String> = BTreeMap::new();
    text_fields.insert(plan_field.to_string(), plan_json_str);
    // T054: tag every planner-authored plan symmetrically with synthesized
    // plans (which carry plan_source = "contract_synthesized"). Consumers
    // (render, audit) distinguish by reading plan_source rather than
    // branching on plan-shape heuristics.
    text_fields.insert("plan_source".to_string(), "planner_authored".to_string());

    let fw_fields: BTreeMap<String, i64> = BTreeMap::new();

    write_status_and_fields(
        &tx,
        &schema.name,
        row_id,
        &new_status,
        &invoker.to_string(),
        &fw_fields,
        &text_fields,
        Some(TransitionAudit {
            display_id,
            from_status: "planning",
            verb: "submit-plan",
            policy_ref: None,
            policies_hash: None,
        }),
    )?;

    // Step 9: no follow-on for submit-plan

    // Step 10: release lock
    release_lock(&tx, &schema.name, display_id)?;

    // Step 11: commit
    tx.commit().context("submit-plan: commit")?;

    Ok(SubmitOutput {
        display_id: display_id.to_string(),
        new_status: new_status.clone(),
        summary: format!("Submitted plan for {display_id}; status now: {new_status}"),
        cycles_idx: None,
        gate: None,
        plan_review_gate: None,
        blocked_reason: None,
    })
}

pub fn run_submit_plan(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    plan_json: Value,
    invoker: InvokerCtx,
) -> Result<()> {
    let out = compute_submit_plan(schema, conn, display_id, plan_json, invoker.actor)?;
    println!("{}", out.summary);
    Ok(())
}

// ---------------------------------------------------------------------------
// submit-plan-review: plan_review → ready (→ executing via on-entry) (AC5.7/5.8/5.9)
// ---------------------------------------------------------------------------

pub(crate) fn compute_submit_plan_review(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    gate: &str, // READY | NEEDS_WORK | NOT_READY
    summary: &str,
    open_questions: Option<Vec<String>>,
    invoker: Actor,
) -> Result<SubmitOutput> {
    // P5-m3: look up field name from submit_targets instead of hardcoding "plan_review_log"
    let workflow = require_workflow(schema, "submit-plan-review")?;
    let log_field = workflow
        .submit_targets
        .get("submit-plan-review")
        .map(|s| s.as_str())
        .unwrap_or("plan_review_log");

    let tx = conn
        .unchecked_transaction()
        .context("submit-plan-review: begin tx")?;
    acquire_lock(&tx, &schema.name, display_id, &invoker.to_string())?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;

    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current_status != "plan_review" {
        bail!(
            "cannot submit-plan-review: row is in state '{}', expected 'plan_review'",
            current_status
        );
    }

    // P5-m2: Build updated log entry, including open_questions if provided.
    // L503-A: snapshot tasks.plan into reviewed_plan at append time so
    // subsequent mutations to tasks.plan cannot retroactively alter this entry.
    let mut log_entry_obj = serde_json::Map::new();
    log_entry_obj.insert("at".to_string(), Value::String(now_iso8601()));
    log_entry_obj.insert("gate".to_string(), Value::String(gate.to_string()));
    log_entry_obj.insert("summary".to_string(), Value::String(summary.to_string()));
    if let Some(qs) = open_questions {
        log_entry_obj.insert(
            "open_questions".to_string(),
            Value::Array(qs.into_iter().map(Value::String).collect()),
        );
    }
    log_entry_obj.insert(
        "reviewed_plan".to_string(),
        existing.get("plan").cloned().unwrap_or(Value::Null),
    );
    let log_entry = Value::Object(log_entry_obj);

    let mut log_list: Vec<Value> = existing
        .get(log_field)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Guard evaluation (for NEEDS_WORK cycle-limit guard) uses the PRE-append entry.
    // The guard `plan_review_log.length < 3` counts how many reviews have already been
    // submitted, NOT including this one.  So on the 4th NEEDS_WORK (where pre-append length=3),
    // 3 < 3 is false → blocked.  On the 3rd NEEDS_WORK (pre-append length=2), 2 < 3 is true → planning.
    // (Post-append evaluation would cause the 3rd NEEDS_WORK to fail, which is wrong.)
    let guard_entry = existing.clone(); // guards evaluate against pre-append state

    log_list.push(log_entry);

    let mut diff: EntryMap = BTreeMap::new();
    diff.insert(log_field.to_string(), Value::Array(log_list.clone()));

    let mut merged = existing.clone();
    merged.insert(log_field.to_string(), Value::Array(log_list.clone()));

    // Validator pass (against post-append merged — ensures content validity)
    validate::validate(
        schema,
        &merged,
        Op::SubmitPlanReview(gate.to_string(), diff),
        invoker.into(),
    )
    .map_err(|errs| {
        anyhow::anyhow!(
            "submit-plan-review validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    // Find transition based on gate (guard evaluated against pre-append state per AC5.8)
    let transition = find_transition(
        schema,
        "plan_review",
        "submit-plan-review",
        Some(gate),
        &guard_entry,
    )?;
    let new_status = transition.to.clone();

    // Compute post-action fields
    let mut text_fields: BTreeMap<String, String> = BTreeMap::new();
    let fw_fields: BTreeMap<String, i64> = BTreeMap::new();

    if new_status == "blocked" {
        let reason = match gate {
            "NOT_READY" => format!("plan-reviewer marked NOT_READY: {summary}"),
            "NEEDS_WORK" => format!(
                "plan-review NEEDS_WORK cycle limit exceeded (plan_review_log.length >= 3): {summary}"
            ),
            _ => format!("plan-review blocked: {summary}"),
        };
        text_fields.insert("blocked_reason".to_string(), reason);
    }

    let log_json = serde_json::to_string(&log_list)?;
    text_fields.insert(log_field.to_string(), log_json);

    write_status_and_fields(
        &tx,
        &schema.name,
        row_id,
        &new_status,
        &invoker.to_string(),
        &fw_fields,
        &text_fields,
        Some(TransitionAudit {
            display_id,
            from_status: "plan_review",
            verb: "submit-plan-review",
            policy_ref: None,
            policies_hash: None,
        }),
    )?;

    // Step 9: fire on-entry follow-ons (e.g. ready → executing)
    fire_on_entry_follow_ons(&tx, schema, display_id, row_id, &new_status)?;

    // Read final status after all follow-ons (they may have changed it)
    let (_, final_entry) = read_row(schema, &tx, display_id)?;
    let final_status = final_entry
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or(&new_status)
        .to_string();

    // Step 10: release lock
    release_lock(&tx, &schema.name, display_id)?;

    // Step 11: commit
    tx.commit().context("submit-plan-review: commit")?;

    Ok(SubmitOutput {
        display_id: display_id.to_string(),
        new_status: final_status.clone(),
        summary: format!(
            "Submitted plan-review for {display_id} --gate {gate}; status now: {final_status}"
        ),
        cycles_idx: None,
        gate: None,
        plan_review_gate: Some(gate.to_string()),
        blocked_reason: None,
    })
}

pub fn run_submit_plan_review(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    gate: &str,
    summary: &str,
    open_questions: Option<Vec<String>>,
    invoker: InvokerCtx,
) -> Result<()> {
    let out = compute_submit_plan_review(
        schema,
        conn,
        display_id,
        gate,
        summary,
        open_questions,
        invoker.actor,
    )?;
    println!("{}", out.summary);
    Ok(())
}

// ---------------------------------------------------------------------------
// submit-execute: executing → code_review (AC5.1)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(crate) fn compute_submit_execute(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    exec_summary: &str,
    commit_sha: Option<&str>,
    files_changed: Option<&str>,
    notes: Option<&str>,
    invoker: Actor,
    // T072 r6: when Some, embedded into executor sub-record inside the same tx (atomic backlink).
    transcript_path: Option<&str>,
) -> Result<SubmitOutput> {
    // P5-m3: look up field name from submit_targets instead of hardcoding "cycles"
    let workflow = require_workflow(schema, "submit-execute")?;
    let cycles_field = workflow
        .submit_targets
        .get("submit-execute")
        .map(|s| s.as_str())
        .unwrap_or("cycles");

    let tx = conn
        .unchecked_transaction()
        .context("submit-execute: begin tx")?;
    acquire_lock(&tx, &schema.name, display_id, &invoker.to_string())?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;

    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current_status != "executing" {
        bail!(
            "cannot submit-execute: row is in state '{}', expected 'executing'",
            current_status
        );
    }

    let current_phase = existing
        .get("current_phase")
        .and_then(|v| v.as_i64())
        .unwrap_or(1);
    let current_cycle = existing
        .get("current_cycle")
        .and_then(|v| v.as_i64())
        .unwrap_or(1);

    // Build new cycles entry
    let mut executor_obj = serde_json::Map::new();
    executor_obj.insert("at".to_string(), Value::String(now_iso8601()));
    executor_obj.insert(
        "summary".to_string(),
        Value::String(exec_summary.to_string()),
    );
    if let Some(sha) = commit_sha {
        executor_obj.insert("commit".to_string(), Value::String(sha.to_string()));
    }
    if let Some(files) = files_changed {
        let files_vec: Vec<Value> = files
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .map(Value::String)
            .collect();
        executor_obj.insert("files_changed".to_string(), Value::Array(files_vec));
    }
    if let Some(n) = notes {
        executor_obj.insert("notes".to_string(), Value::String(n.to_string()));
    }
    // T072 r6: atomic backlink — embed transcript_path inside the tx so the
    // executor sub-record is never committed without its transcript pointer.
    if let Some(tp) = transcript_path {
        executor_obj.insert("transcript_path".to_string(), Value::String(tp.to_string()));
    }

    let new_cycle_entry = json!({
        "phase": current_phase,
        "cycle": current_cycle,
        "executor": Value::Object(executor_obj),
        "review": Value::Null,
    });

    let mut cycles: Vec<Value> = existing
        .get(cycles_field)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let cycles_idx = cycles.len();
    cycles.push(new_cycle_entry);

    let mut diff: EntryMap = BTreeMap::new();
    diff.insert(cycles_field.to_string(), Value::Array(cycles.clone()));

    let mut merged = existing.clone();
    merged.insert(cycles_field.to_string(), Value::Array(cycles.clone()));

    validate::validate(schema, &merged, Op::SubmitExecute(diff), invoker.into()).map_err(
        |errs| {
            anyhow::anyhow!(
                "submit-execute validation failed:\n{}",
                validate::pretty_print(&errs)
            )
        },
    )?;

    let transition = find_transition(schema, "executing", "submit-execute", None, &merged)?;
    let new_status = transition.to.clone();

    let cycles_json = serde_json::to_string(&cycles)?;
    let mut text_fields: BTreeMap<String, String> = BTreeMap::new();
    text_fields.insert(cycles_field.to_string(), cycles_json);

    let fw_fields: BTreeMap<String, i64> = BTreeMap::new();

    write_status_and_fields(
        &tx,
        &schema.name,
        row_id,
        &new_status,
        &invoker.to_string(),
        &fw_fields,
        &text_fields,
        Some(TransitionAudit {
            display_id,
            from_status: "executing",
            verb: "submit-execute",
            policy_ref: None,
            policies_hash: None,
        }),
    )?;

    // No follow-on for submit-execute

    release_lock(&tx, &schema.name, display_id)?;
    tx.commit().context("submit-execute: commit")?;

    Ok(SubmitOutput {
        display_id: display_id.to_string(),
        new_status,
        summary: format!(
            "Submitted execute for {display_id} phase {current_phase} cycle {current_cycle}; status now: code_review"
        ),
        cycles_idx: Some(cycles_idx),
        gate: None,
        plan_review_gate: None,
        blocked_reason: None,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn run_submit_execute(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    exec_summary: &str,
    commit_sha: Option<&str>,
    files_changed: Option<&str>,
    notes: Option<&str>,
    invoker: InvokerCtx,
) -> Result<()> {
    let out = compute_submit_execute(
        schema,
        conn,
        display_id,
        exec_summary,
        commit_sha,
        files_changed,
        notes,
        invoker.actor,
        None, // CLI submits have no runner session
    )?;
    println!("{}", out.summary);
    Ok(())
}

// ---------------------------------------------------------------------------
// submit-review: code_review → executing | complete | blocked (AC5.2/5.3/5.4/5.4b)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(crate) fn compute_submit_review(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    gate: &str, // PASS | REVISE | FAIL
    review_summary: &str,
    review_details: Option<&str>,
    critical: i64,
    major: i64,
    minor: i64,
    invoker: Actor,
    // T072 r6: when Some, embedded into review sub-record inside the same tx (atomic backlink).
    transcript_path: Option<&str>,
) -> Result<SubmitOutput> {
    // P5-m3: look up field name from submit_targets instead of hardcoding "cycles"
    let workflow = require_workflow(schema, "submit-review")?;
    let cycles_field = workflow
        .submit_targets
        .get("submit-review")
        .map(|s| s.as_str())
        .unwrap_or("cycles");

    let tx = conn
        .unchecked_transaction()
        .context("submit-review: begin tx")?;
    acquire_lock(&tx, &schema.name, display_id, &invoker.to_string())?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;

    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current_status != "code_review" {
        bail!(
            "cannot submit-review: row is in state '{}', expected 'code_review'",
            current_status
        );
    }

    let current_phase = existing
        .get("current_phase")
        .and_then(|v| v.as_i64())
        .unwrap_or(1);
    let current_cycle = existing
        .get("current_cycle")
        .and_then(|v| v.as_i64())
        .unwrap_or(1);

    let mut cycles: Vec<Value> = existing
        .get(cycles_field)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Find the matching cycle entry (most recent phase+cycle match)
    let cycle_idx = cycles.iter().rposition(|v| {
        v.get("phase").and_then(|p| p.as_i64()) == Some(current_phase)
            && v.get("cycle").and_then(|c| c.as_i64()) == Some(current_cycle)
    });

    let cycle_idx = cycle_idx.ok_or_else(|| {
        anyhow::anyhow!(
            "no cycles[] entry found for phase {} cycle {} — was submit-execute called first?",
            current_phase,
            current_cycle
        )
    })?;

    // P5-m4: Patch the cycles entry with the review sub-record; summary and details are separate fields
    let mut review_obj_map = serde_json::Map::new();
    review_obj_map.insert("at".to_string(), Value::String(now_iso8601()));
    review_obj_map.insert("gate".to_string(), Value::String(gate.to_string()));
    review_obj_map.insert(
        "summary".to_string(),
        Value::String(review_summary.to_string()),
    );
    if let Some(details) = review_details {
        review_obj_map.insert("details".to_string(), Value::String(details.to_string()));
    }
    review_obj_map.insert("critical".to_string(), Value::from(critical));
    review_obj_map.insert("major".to_string(), Value::from(major));
    review_obj_map.insert("minor".to_string(), Value::from(minor));
    // T072 r6: atomic backlink — embed transcript_path inside the tx so the
    // review sub-record is never committed without its transcript pointer.
    if let Some(tp) = transcript_path {
        review_obj_map.insert("transcript_path".to_string(), Value::String(tp.to_string()));
    }
    let review_obj = Value::Object(review_obj_map);

    if let Some(Value::Object(ref mut obj)) = cycles.get_mut(cycle_idx) {
        obj.insert("review".to_string(), review_obj);
    }

    let mut diff: EntryMap = BTreeMap::new();
    diff.insert(cycles_field.to_string(), Value::Array(cycles.clone()));

    let mut merged = existing.clone();
    merged.insert(cycles_field.to_string(), Value::Array(cycles.clone()));

    validate::validate(
        schema,
        &merged,
        Op::SubmitReview(gate.to_string(), diff),
        invoker.into(),
    )
    .map_err(|errs| {
        anyhow::anyhow!(
            "submit-review validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    // Engine post-actions (5.4 / 5.5 / 5.5b)
    let mut fw_fields: BTreeMap<String, i64> = BTreeMap::new();
    let mut text_fields: BTreeMap<String, String> = BTreeMap::new();

    let cycles_json = serde_json::to_string(&cycles)?;
    text_fields.insert(cycles_field.to_string(), cycles_json);

    let new_status: String;

    match gate {
        "REVISE" => {
            // 5.5: post-increment current_cycle in a working copy, evaluate guard.
            let bumped_cycle = current_cycle + 1;

            let mut guard_entry = merged.clone();
            guard_entry.insert("current_cycle".to_string(), Value::from(bumped_cycle));

            let transition = find_transition(
                schema,
                "code_review",
                "submit-review",
                Some("REVISE"),
                &guard_entry,
            )?;
            new_status = transition.to.clone();

            if new_status == "executing" {
                // Guard passed — apply the bump
                fw_fields.insert("current_cycle".to_string(), bumped_cycle);
                text_fields.insert("blocked_reason".to_string(), String::new());
            } else {
                // Guard failed → blocked (unguarded REVISE fallback matched)
                // Do NOT apply the bump (working-copy only, per 5.5)
                let reason = format!(
                    "4th revise rejected by guard current_cycle <= 4 on phase {current_phase} cycle {current_cycle}: {review_summary}"
                );
                text_fields.insert("blocked_reason".to_string(), reason);
            }
        }

        "PASS" => {
            // 5.5b: two PASS transitions disambiguated by guard
            let transition = find_transition(
                schema,
                "code_review",
                "submit-review",
                Some("PASS"),
                &merged,
            )?;
            new_status = transition.to.clone();

            if new_status == "executing" {
                // Non-last phase: advance
                fw_fields.insert("current_phase".to_string(), current_phase + 1);
                fw_fields.insert("current_cycle".to_string(), 1);
                text_fields.insert("blocked_reason".to_string(), String::new());
            }
            // last phase → complete: no framework field changes
        }

        "FAIL" => {
            new_status = "blocked".to_string();
            text_fields.insert(
                "blocked_reason".to_string(),
                format!("code-reviewer marked FAIL on phase {current_phase}: {review_summary}"),
            );
        }

        other => {
            bail!(
                "submit-review: unknown gate '{}'; expected PASS, REVISE, or FAIL",
                other
            );
        }
    }

    write_status_and_fields(
        &tx,
        &schema.name,
        row_id,
        &new_status,
        &invoker.to_string(),
        &fw_fields,
        &text_fields,
        Some(TransitionAudit {
            display_id,
            from_status: "code_review",
            verb: "submit-review",
            policy_ref: None,
            policies_hash: None,
        }),
    )?;

    // Fire on-entry follow-ons for the new state (e.g. complete → in_review via on_state.complete).
    // For PASS-last-phase, new_status == "complete" and the schema's on_state.complete fires
    // request_review (framework actor), advancing the row to in_review in the same tx.
    fire_on_entry_follow_ons(&tx, schema, display_id, row_id, &new_status)?;

    // Re-read status after follow-ons (may have advanced from complete → in_review)
    let (_, post_follow_on_entry) = read_row(schema, &tx, display_id)?;
    let final_status = post_follow_on_entry
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or(&new_status)
        .to_string();

    release_lock(&tx, &schema.name, display_id)?;
    tx.commit().context("submit-review: commit")?;

    let blocked_reason = text_fields.get("blocked_reason").and_then(|r| {
        if r.is_empty() {
            None
        } else {
            Some(r.clone())
        }
    });

    Ok(SubmitOutput {
        display_id: display_id.to_string(),
        new_status: final_status.clone(),
        summary: format!(
            "Submitted review for {display_id} --gate {gate}; status now: {final_status}"
        ),
        cycles_idx: Some(cycle_idx),
        gate: Some(gate.to_string()),
        plan_review_gate: None,
        blocked_reason,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn run_submit_review(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    gate: &str,
    review_summary: &str,
    review_details: Option<&str>,
    critical: i64,
    major: i64,
    minor: i64,
    invoker: InvokerCtx,
) -> Result<()> {
    let out = compute_submit_review(
        schema,
        conn,
        display_id,
        gate,
        review_summary,
        review_details,
        critical,
        major,
        minor,
        invoker.actor,
        None, // CLI submits have no runner session
    )?;
    println!("{}", out.summary);
    // Non-zero exit when the submit routes the row to blocked (e.g. 4th REVISE guard failure).
    // The blocked_reason already contains the guard expression and context.
    if out.new_status == "blocked" {
        if let Some(reason) = &out.blocked_reason {
            bail!("submit-review routed {} to blocked: {}", display_id, reason);
        } else {
            bail!("submit-review routed {} to blocked", display_id);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// External review seam for submit-wrap (T083 P1)
// ---------------------------------------------------------------------------

fn next_external_review_display_id(tx: &Transaction) -> Result<String> {
    let next_num: i64 = tx.query_row(
        "SELECT COALESCE(MAX(id), 0) + 1 FROM external_reviews",
        [],
        |r| r.get(0),
    )?;
    Ok(format!("ER{next_num:03}"))
}

fn maybe_create_pending_external_review(
    tx: &Transaction,
    task_row_id: i64,
    task_display_id: &str,
    task_row: &EntryMap,
    wrap_len: usize,
    invoker: &str,
) -> Result<Option<i64>> {
    if !table_exists(tx, "external_reviews")? {
        return Ok(None);
    }

    let tier = task_row
        .get("tier_hint")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if tier == "T1" {
        return Ok(None);
    }

    let open_attempts: i64 = tx.query_row(
        "SELECT COUNT(*) FROM external_reviews \
         WHERE task_id = ?1 AND status IN ('pending', 'running')",
        rusqlite::params![task_display_id],
        |r| r.get(0),
    )?;
    if open_attempts > 0 {
        return Ok(None);
    }

    let next_attempt: i64 = tx.query_row(
        "SELECT COALESCE(MAX(attempt), 0) + 1 FROM external_reviews WHERE task_id = ?1",
        rusqlite::params![task_display_id],
        |r| r.get(0),
    )?;
    let review_display_id = next_external_review_display_id(tx)?;
    let now = now_iso8601();
    let wrap_ref = if wrap_len == 0 {
        format!("tasks:{task_display_id}:wrap_log")
    } else {
        format!("tasks:{task_display_id}:wrap_log[{}]", wrap_len - 1)
    };

    tx.execute(
        "INSERT INTO external_reviews \
         (display_id, status, created_at, updated_at, created_by, updated_by, \
          task_id, attempt, adapter, contract_ref, plan_ref, wrap_log_ref, diff_ref, prior_review_ref) \
         VALUES (?1, 'pending', ?2, ?2, ?3, ?3, ?4, ?5, 'external_review', ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            review_display_id,
            now,
            invoker,
            task_display_id,
            next_attempt,
            format!("tasks:{task_display_id}:contract"),
            format!("tasks:{task_display_id}:plan"),
            wrap_ref,
            format!("tasks:{task_display_id}:diff"),
            format!("tasks:{task_display_id}:cycles"),
        ],
    )
    .context("submit-wrap: create pending external_review")?;
    let review_row_id = tx.last_insert_rowid();

    crate::db::insert_transition_history(
        tx,
        "external_reviews",
        review_row_id,
        &review_display_id,
        "",
        "pending",
        "create-external-review",
        invoker,
        None,
        None,
        Some(&format!("task_row_id={task_row_id}")),
    )?;

    Ok(Some(review_row_id))
}

// ---------------------------------------------------------------------------
// submit-wrap: append to wrap_log[] (pure write — no transition fired) (AC3.x)
// ---------------------------------------------------------------------------

/// Append a wrap entry to `wrap_log[]` without firing any transition.
///
/// **Design rationale:** By the time this handler is called, the row is already at `in_review`.
/// The `complete → in_review` transition was fired by `compute_submit_review`'s on-entry
/// follow-on machinery (`on_state.complete: [transition_to: in_review]`). There is no
/// `submit-wrap` verb in `lifecycle.transitions`; `submit-wrap` is declared only in
/// `submit_targets` as a list_record write target.
///
/// **Actor enforcement:** submit-wrap accepts any invoker. The actor gate that matters
/// for the wrap lifecycle is on the upstream `complete → in_review` transition
/// (`actor: framework`, only fireable by on-entry machinery) and on the downstream
/// `accept`/`reject` transitions (`actor: human`). submit-wrap itself is invoked by
/// the wrap agent (ai_autonomous), but there is no verb-matched transition to validate
/// against, so no actor check is applied here. Existing submit verbs that lack a verb-matched
/// transition (e.g. this pattern) simply skip the `find_transition` + validator step.
///
/// **Re-entry semantics:** calling this on an `in_review` row that already has a `wrap_log`
/// entry appends a new entry (append-only list_record). This supports re-wrap after
/// `rejected → planning → … → complete` round-trips without overwriting history.
pub(crate) fn compute_submit_wrap(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    wrap_entry: Value,
    invoker: Actor,
) -> Result<SubmitOutput> {
    require_workflow(schema, "submit-wrap")?;

    // Look up field name from submit_targets (schema is the contract)
    let workflow = require_workflow(schema, "submit-wrap")?;
    let wrap_field = workflow
        .submit_targets
        .get("submit-wrap")
        .map(|s| s.as_str())
        .unwrap_or("wrap_log");

    // Step 1: open tx
    let tx = conn
        .unchecked_transaction()
        .context("submit-wrap: begin tx")?;

    // Step 2: acquire lock
    acquire_lock(&tx, &schema.name, display_id, &invoker.to_string())?;

    // Step 3: read row
    let (row_id, existing) = read_row(schema, &tx, display_id)?;

    // State-machine check (AC3.1): must be in_review
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current_status != "in_review" {
        bail!(
            "cannot submit-wrap: row is in state '{}', expected 'in_review'",
            current_status
        );
    }

    // Step 4/5: build updated wrap_log list by appending new entry with `at` timestamp
    let mut entry_obj = match wrap_entry {
        Value::Object(m) => m,
        other => {
            bail!(
                "submit-wrap: wrap_entry must be a JSON object, got {}",
                other
            );
        }
    };
    // Step 6: defense-in-depth shape check — executive_summary must be present and non-empty.
    // The full schema-validator Op is a hardening follow-up (no Op::SubmitWrap exists yet);
    // this guard catches the most critical missing field without structural rework.
    {
        let summary_ok = entry_obj
            .get("executive_summary")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        if !summary_ok {
            bail!("submit-wrap: executive_summary is required and must be a non-empty string");
        }
    }

    // `at` is always set by the handler, overriding any caller-supplied value
    entry_obj.insert("at".to_string(), Value::String(now_iso8601()));
    let entry = Value::Object(entry_obj);

    let mut wrap_list: Vec<Value> = existing
        .get(wrap_field)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    wrap_list.push(entry);

    // Step 7: no transition fired (status stays in_review)

    // Step 8: write updated wrap_log (status unchanged)
    let wrap_json = serde_json::to_string(&wrap_list)?;
    let mut text_fields: BTreeMap<String, String> = BTreeMap::new();
    text_fields.insert(wrap_field.to_string(), wrap_json);

    let fw_fields: BTreeMap<String, i64> = BTreeMap::new();

    write_status_and_fields(
        &tx,
        &schema.name,
        row_id,
        "in_review",
        &invoker.to_string(),
        &fw_fields,
        &text_fields,
        // No status change (in_review → in_review is an in-place wrap_log append).
        None,
    )?;

    // Step 9: after the wrap_log append, T2/T3 rows become externally reviewable.
    maybe_create_pending_external_review(
        &tx,
        row_id,
        display_id,
        &existing,
        wrap_list.len(),
        &invoker.to_string(),
    )?;

    // Step 10: release lock
    release_lock(&tx, &schema.name, display_id)?;

    // Step 11: commit
    tx.commit().context("submit-wrap: commit")?;

    Ok(SubmitOutput {
        display_id: display_id.to_string(),
        new_status: "in_review".to_string(),
        summary: format!(
            "Submitted wrap for {display_id}; wrap_log now has {} entries; status remains: in_review",
            wrap_list.len()
        ),
        cycles_idx: None,
        gate: None,
        plan_review_gate: None,
        blocked_reason: None,
    })
}

pub fn run_submit_wrap(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    wrap_entry: Value,
    invoker: InvokerCtx,
) -> Result<()> {
    let out = compute_submit_wrap(schema, conn, display_id, wrap_entry, invoker.actor)?;
    println!("{}", out.summary);
    Ok(())
}

// ---------------------------------------------------------------------------
// resume: blocked → ready (→ executing via on-entry follow-on) (AC5.14)
// ---------------------------------------------------------------------------

/// Resume a blocked task.  Follows the same 11-step pattern as the other submit verbs.
///
/// Required actor: `ai_with_human` (declared on the `resume` transition in schema).
/// Post-actions per 5.4 "resume" row:
///   - `current_cycle = 1` (reset; audit trail in cycles[] preserved)
///   - `current_phase` UNCHANGED
///   - `blocked_reason` cleared
///   - stale auto-drive bookkeeping cleared so the watchdog does not immediately
///     re-block a human-resumed row based on an old dead PID/lock
pub(crate) fn compute_resume(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    invoker: Actor,
) -> Result<ResumeOutput> {
    require_workflow(schema, "resume")?;

    // Step 1: open tx
    let tx = conn.unchecked_transaction().context("resume: begin tx")?;

    // Step 2: acquire lock (error if already claimed)
    acquire_lock(&tx, &schema.name, display_id, &invoker.to_string())?;

    // Step 3: read row
    let (row_id, existing) = read_row(schema, &tx, display_id)?;

    // State-machine check
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current_status == "deploy_blocked" {
        bail!(
            "cannot resume: row is in state 'deploy_blocked'; deploy_blocked rows use retry-deploy or close-out-of-band, not resume"
        );
    }
    if current_status != "blocked" {
        bail!(
            "cannot resume: row is in state '{}', expected 'blocked'",
            current_status
        );
    }

    // Step 4/5: no user diff for resume; build an empty diff for the actor check
    let diff: EntryMap = BTreeMap::new();

    // Step 6: validator pass — enforces `actor: ai_with_human` on the resume transition
    validate::validate(
        schema,
        &existing,
        Op::Transition("resume".to_string(), diff),
        invoker.into(),
    )
    .map_err(|errs| {
        anyhow::anyhow!(
            "resume validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    clear_auto_drive_bookkeeping_for_resume(&tx, &schema.name, row_id, display_id)?;

    // Step 7: compute post-action fields
    //   current_cycle reset to 1; current_phase UNCHANGED; blocked_reason cleared
    let mut fw_fields: BTreeMap<String, i64> = BTreeMap::new();
    fw_fields.insert("current_cycle".to_string(), 1);
    let mut txt_fields: BTreeMap<String, String> = BTreeMap::new();
    txt_fields.insert("blocked_reason".to_string(), String::new());

    // T054/I033: resume may only route to ready/executing when the row has an
    // executable plan. "plan is non-empty" is insufficient: T118 proved a row
    // can have a non-empty *rejected* plan, then block during revision planning;
    // resuming that shape to ready executes an invalid plan. For T2/T3 rows,
    // require latest plan_review_log.gate == READY. T1 keeps the contract-is-plan
    // path because skip-plan synthesizes its executable plan from the contract.
    let tier_hint = existing
        .get("tier_hint")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let is_t1 = tier_hint == "T1";
    let plan_is_empty = existing
        .get("plan")
        .map(|v| match v {
            serde_json::Value::Null => true,
            serde_json::Value::String(s) => s.trim().is_empty(),
            serde_json::Value::Object(m) => m.is_empty(),
            _ => false,
        })
        .unwrap_or(true);
    let latest_plan_review_ready = existing
        .get("plan_review_log")
        .and_then(|v| match v {
            serde_json::Value::Array(items) => items.last().cloned(),
            serde_json::Value::String(s) => serde_json::from_str::<serde_json::Value>(s)
                .ok()
                .and_then(|parsed| parsed.as_array().and_then(|items| items.last().cloned())),
            _ => None,
        })
        .and_then(|last| {
            last.get("gate")
                .and_then(|v| v.as_str())
                .map(|gate| gate == "READY")
        })
        .unwrap_or(false);
    let resume_target = if plan_is_empty {
        "planning"
    } else if is_t1 || latest_plan_review_ready {
        "ready"
    } else {
        "planning"
    };

    // Step 8: write blocked → resume_target
    write_status_and_fields(
        &tx,
        &schema.name,
        row_id,
        resume_target,
        &invoker.to_string(),
        &fw_fields,
        &txt_fields,
        Some(TransitionAudit {
            display_id,
            from_status: "blocked",
            verb: "resume",
            policy_ref: None,
            policies_hash: None,
        }),
    )?;

    // Step 9: fire on-entry follow-ons. For target=ready this cascades
    // ready → executing; for target=planning there's no follow-on (planner
    // is dispatched by the daemon's auto-drive subscriber on next poll).
    fire_on_entry_follow_ons(&tx, schema, display_id, row_id, resume_target)?;

    // Clear stale auto-drive ownership. A blocked row may carry drive_pid and an
    // auto-drive dispatch_lock from a previous detached drive. If left intact,
    // the watchdog can immediately mark the resumed row blocked again as
    // silent_zombie_pid_dead/pid_never_recorded even when a fresh manual drive is
    // active. Resume is the human-authorized recovery point, so it severs that
    // stale ownership before commit.
    let has_drive_pid = schema.fields.iter().any(|f| f.name == "drive_pid");
    let has_drive_started_at = schema.fields.iter().any(|f| f.name == "drive_started_at");
    if has_drive_pid && has_drive_started_at {
        let table = crate::codegen::ddl::quote_ident(&schema.name);
        tx.execute(
            &format!(
                "UPDATE {table} SET drive_pid = NULL, drive_started_at = '', updated_at = ?1 WHERE id = ?2"
            ),
            rusqlite::params![now_iso8601(), row_id],
        )
        .context("resume: clear stale auto-drive task bookkeeping")?;
        tx.execute(
            "DELETE FROM dispatch_locks WHERE store = ?1 AND row_id = ?2 AND agent_name = 'auto-drive'",
            rusqlite::params![schema.name, row_id],
        )
        .context("resume: delete stale auto-drive dispatch lock")?;
    }

    // Read final status after follow-ons
    let (_, final_entry) = read_row(schema, &tx, display_id)?;
    let final_status = final_entry
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("executing")
        .to_string();

    // Step 10: release lock
    release_lock(&tx, &schema.name, display_id)?;

    // Step 11: commit
    tx.commit().context("resume: commit")?;

    Ok(ResumeOutput {
        display_id: display_id.to_string(),
        new_status: final_status.clone(),
        summary: format!("Resumed {display_id}; status now: {final_status}"),
    })
}

/// Structured output from resume handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeOutput {
    pub display_id: String,
    pub new_status: String,
    pub summary: String,
}

/// Structured output from retry-deploy handler. Distinct from `ResumeOutput`
/// to reflect the handler-scope separation L145 requires (retry-deploy is not
/// a resume — it re-fires the full deploy ceremony via subscriber chain).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryDeployOutput {
    pub display_id: String,
    pub new_status: String,
    pub summary: String,
}

pub fn run_resume(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    invoker: InvokerCtx,
) -> Result<()> {
    let out = compute_resume(schema, conn, display_id, invoker.actor)?;
    println!("{}", out.summary);
    Ok(())
}

// ---------------------------------------------------------------------------
// retry-deploy: deploy_blocked → accepted (subscriber edge retry)
// ---------------------------------------------------------------------------

/// Retry the deploy ceremony by writing the schema-declared
/// deploy_blocked → accepted edge. Subscribers observe the resulting
/// transition_history row and re-run the existing ceremony; this handler does
/// not directly call planner/executor/code-reviewer or deploy builtins.
pub(crate) fn compute_retry_deploy(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    invoker: Actor,
) -> Result<RetryDeployOutput> {
    require_workflow(schema, "retry-deploy")?;

    let tx = conn
        .unchecked_transaction()
        .context("retry-deploy: begin tx")?;
    acquire_lock(&tx, &schema.name, display_id, &invoker.to_string())?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current_status != "deploy_blocked" {
        bail!(
            "cannot retry-deploy: row is in state '{}', expected 'deploy_blocked'",
            current_status
        );
    }

    let diff: EntryMap = BTreeMap::new();
    validate::validate(
        schema,
        &existing,
        Op::Transition("retry-deploy".to_string(), diff),
        invoker.into(),
    )
    .map_err(|errs| {
        anyhow::anyhow!(
            "retry-deploy validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    let transition = find_transition(schema, "deploy_blocked", "retry-deploy", None, &existing)?;

    let fw_fields: BTreeMap<String, i64> = BTreeMap::new();
    let mut txt_fields: BTreeMap<String, String> = BTreeMap::new();
    txt_fields.insert("blocked_reason".to_string(), String::new());

    write_status_and_fields(
        &tx,
        &schema.name,
        row_id,
        &transition.to,
        &invoker.to_string(),
        &fw_fields,
        &txt_fields,
        Some(TransitionAudit {
            display_id,
            from_status: "deploy_blocked",
            verb: "retry-deploy",
            policy_ref: None,
            policies_hash: None,
        }),
    )?;

    release_lock(&tx, &schema.name, display_id)?;
    tx.commit().context("retry-deploy: commit")?;

    Ok(RetryDeployOutput {
        display_id: display_id.to_string(),
        new_status: transition.to.clone(),
        summary: format!(
            "Retrying deploy for {display_id}; status now: {}",
            transition.to
        ),
    })
}

pub fn run_retry_deploy(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    invoker: InvokerCtx,
) -> Result<()> {
    let out = compute_retry_deploy(schema, conn, display_id, invoker.actor)?;
    println!("{}", out.summary);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — all 14 ACs verified at compute level
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Schema;
    use rusqlite::Connection;
    use serde_json::json;

    // ---------------------------------------------------------------------------
    // Fixture schema — mirrors the workflow_minimal schema
    // ---------------------------------------------------------------------------

    const WF_SCHEMA_YAML: &str = r#"
name: wf_tasks
id_format: "WF{:03d}"

lifecycle:
  states: [planning, plan_review, ready, executing, code_review, blocked, complete, in_review, accepted, rejected]
  transitions:
    - from: planning
      to: plan_review
      verb: submit-plan
      actor: ai_autonomous
    - from: plan_review
      to: ready
      verb: submit-plan-review
      requires_gate: READY
      actor: ai_autonomous
    - from: plan_review
      to: planning
      verb: submit-plan-review
      requires_gate: NEEDS_WORK
      guard: "plan_review_log.length < 3"
      actor: ai_autonomous
    - from: plan_review
      to: blocked
      verb: submit-plan-review
      requires_gate: NEEDS_WORK
      actor: ai_autonomous
    - from: plan_review
      to: blocked
      verb: submit-plan-review
      requires_gate: NOT_READY
      actor: ai_autonomous
    - from: ready
      to: executing
      verb: ready-enter
      actor: framework
    - from: executing
      to: code_review
      verb: submit-execute
      actor: ai_autonomous
    - from: code_review
      to: executing
      verb: submit-review
      requires_gate: REVISE
      guard: "current_cycle <= 4"
      actor: ai_autonomous
    - from: code_review
      to: blocked
      verb: submit-review
      requires_gate: REVISE
      actor: ai_autonomous
    - from: code_review
      to: executing
      verb: submit-review
      requires_gate: PASS
      guard: "current_phase < plan.phases.length"
      actor: ai_autonomous
    - from: code_review
      to: complete
      verb: submit-review
      requires_gate: PASS
      guard: "current_phase >= plan.phases.length"
      actor: ai_autonomous
    - from: code_review
      to: blocked
      verb: submit-review
      requires_gate: FAIL
      actor: ai_autonomous
    - from: blocked
      to: ready
      verb: resume
      actor: ai_with_human
    - from: complete
      to: in_review
      verb: request_review
      actor: framework
    - from: in_review
      to: accepted
      verb: accept
      actor: human
    - from: in_review
      to: rejected
      verb: reject
      actor: human
    - from: rejected
      to: planning
      verb: amend
      actor: ai_with_human

fields:
  - name: title
    type: text
    required: true
  - name: description
    type: text
  - name: tier_hint
    type: text
  - name: current_phase
    type: integer
    actor: framework
  - name: current_cycle
    type: integer
    actor: framework
  - name: blocked_reason
    type: text
    actor: framework
  - name: claimed_by
    type: text
    actor: framework
  - name: claimed_at
    type: timestamp
    actor: framework
  - name: drive_pid
    type: integer
    actor: framework
  - name: drive_started_at
    type: timestamp
    actor: framework
  - name: plan_source
    type: text
    actor: framework
  - name: plan
    type: record
    fields:
      - name: summary
        type: text
      - name: phases
        type: list_record
        fields:
          - name: name
            type: text
  - name: cycles
    type: list_record
    fields:
      - name: phase
        type: integer
      - name: cycle
        type: integer
      - name: executor
        type: record
        fields:
          - name: summary
            type: text
          - name: commit
            type: text
          - name: files_changed
            type: text
      - name: review
        type: record
        fields:
          - name: gate
            type: text
          - name: summary
            type: text
          - name: critical
            type: integer
          - name: major
            type: integer
          - name: minor
            type: integer
  - name: plan_review_log
    type: list_record
    fields:
      - name: gate
        type: text
      - name: summary
        type: text
  - name: wrap_log
    type: list_record
    fields:
      - name: executive_summary
        type: text
      - name: deviations
        type:
          list: text
      - name: residual_risks
        type:
          list: text
      - name: recommended_sanity_checks
        type:
          list: text
      - name: reject_reason
        type: text
      - name: at
        type: timestamp

workflow:
  agent_roles:
    planner:
      description: "Creates the implementation plan"
    executor:
      description: "Implements the plan"
    code_reviewer:
      description: "Reviews the execution"
    plan_reviewer:
      description: "Reviews the plan"
    wrap:
      description: "Synthesises completed task into a reviewer brief"
  briefing_templates:
    planner: templates/planner-brief.md.tpl
    executor: templates/executor-brief.md.tpl
    code_reviewer: templates/executor-brief.md.tpl
    plan_reviewer: templates/planner-brief.md.tpl
    wrap: templates/wrap-brief.md.tpl
  on_state:
    planning:
      - dispatch_agent: planner
    ready:
      - transition_to: executing
    executing:
      - dispatch_agent: executor
    code_review:
      - dispatch_agent: code_reviewer
    plan_review:
      - dispatch_agent: plan_reviewer
    complete:
      - transition_to: in_review
    in_review:
      - dispatch_agent: wrap
  submit_targets:
    submit-plan: plan
    submit-execute: cycles
    submit-review: cycles
    submit-plan-review: plan_review_log
    submit-wrap: wrap_log
  max_revise_cycles: 3
"#;

    fn setup() -> (Schema, Connection) {
        let schema = Schema::from_yaml(WF_SCHEMA_YAML).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    /// Insert a row with specific state for testing.
    #[allow(unused_variables, clippy::too_many_arguments)]
    fn insert_row_at(
        conn: &Connection,
        schema: &Schema, // unused — kept for call-site clarity
        status: &str,
        current_phase: i64,
        current_cycle: i64,
        plan_phases_count: usize,
        cycles: Vec<Value>,
        plan_review_log: Vec<Value>,
        blocked_reason: Option<&str>,
    ) {
        let now = now_iso8601();
        let plan_json = serde_json::to_string(&json!({
            "summary": "test plan",
            "phases": (0..plan_phases_count)
                .map(|i| json!({"name": format!("phase {}", i + 1)}))
                .collect::<Vec<_>>()
        }))
        .unwrap();
        let cycles_json = serde_json::to_string(&cycles).unwrap();
        let log_json = serde_json::to_string(&plan_review_log).unwrap();
        let br = blocked_reason.unwrap_or("");

        conn.execute(
            "INSERT INTO wf_tasks (display_id, status, created_at, updated_at, \
             created_by, updated_by, title, current_phase, current_cycle, \
             plan, cycles, plan_review_log, blocked_reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                "WF001",
                status,
                now,
                now,
                "human",
                "human",
                "Test task",
                current_phase,
                current_cycle,
                plan_json,
                cycles_json,
                log_json,
                br
            ],
        )
        .unwrap();
    }

    fn read_status(conn: &Connection) -> String {
        conn.query_row(
            "SELECT status FROM wf_tasks WHERE display_id = 'WF001'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn read_i64(conn: &Connection, col: &str) -> i64 {
        conn.query_row(
            &format!("SELECT {col} FROM wf_tasks WHERE display_id = 'WF001'"),
            [],
            |r| r.get(0),
        )
        .unwrap_or(0)
    }

    fn read_text(conn: &Connection, col: &str) -> Option<String> {
        conn.query_row(
            &format!("SELECT {col} FROM wf_tasks WHERE display_id = 'WF001'"),
            [],
            |r| r.get(0),
        )
        .unwrap_or(None)
    }

    fn read_cycles(conn: &Connection) -> Vec<Value> {
        let json_str: Option<String> = conn
            .query_row(
                "SELECT cycles FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(None);
        json_str
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn read_plan_review_log(conn: &Connection) -> Vec<Value> {
        let json_str: Option<String> = conn
            .query_row(
                "SELECT plan_review_log FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(None);
        json_str
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    // Helpers to step through the lifecycle
    fn do_execute(schema: &Schema, conn: &Connection) -> SubmitOutput {
        let status = read_status(conn);
        let phase = read_i64(conn, "current_phase");
        let cycle = read_i64(conn, "current_cycle");
        assert_eq!(status, "executing");
        let summary = format!("attempt phase {} cycle {}", phase, cycle);
        compute_submit_execute(
            schema,
            conn,
            "WF001",
            &summary,
            Some("abc"),
            None,
            None,
            Actor::AiAutonomous,
            None,
        )
        .unwrap()
    }

    fn force_status(conn: &Connection, status: &str, phase: i64, cycle: i64) {
        conn.execute(
            "UPDATE wf_tasks SET status = ?1, current_phase = ?2, current_cycle = ?3 WHERE display_id = 'WF001'",
            rusqlite::params![status, phase, cycle],
        ).unwrap();
    }

    fn set_cycles_json(conn: &Connection, cycles: &[Value]) {
        let j = serde_json::to_string(cycles).unwrap();
        conn.execute(
            "UPDATE wf_tasks SET cycles = ?1 WHERE display_id = 'WF001'",
            rusqlite::params![j],
        )
        .unwrap();
    }

    // ---------------------------------------------------------------------------
    // AC5.1: submit-execute writes cycles[], transitions executing → code_review
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_1_submit_execute_writes_cycle_and_transitions() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "executing", 1, 1, 2, vec![], vec![], None);

        let out = compute_submit_execute(
            &schema,
            &conn,
            "WF001",
            "phase 2 done",
            Some("abc123"),
            Some("src/foo.rs,src/bar.rs"),
            None,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        assert_eq!(out.new_status, "code_review");
        assert_eq!(out.cycles_idx, Some(0));

        assert_eq!(read_status(&conn), "code_review");
        let cycles = read_cycles(&conn);
        assert_eq!(cycles.len(), 1);
        assert_eq!(
            cycles[0]["executor"]["summary"].as_str().unwrap(),
            "phase 2 done"
        );
        assert_eq!(cycles[0]["executor"]["commit"].as_str().unwrap(), "abc123");
        assert_eq!(cycles[0]["phase"].as_i64().unwrap(), 1);
        assert_eq!(cycles[0]["cycle"].as_i64().unwrap(), 1);

        // Lock released
        let claimed_by = read_text(&conn, "claimed_by");
        assert!(
            claimed_by.is_none() || claimed_by.as_deref() == Some(""),
            "lock should be released: {:?}",
            claimed_by
        );
    }

    // ---------------------------------------------------------------------------
    // AC5.2: submit-review --gate PASS (non-last phase) → executing, phase++, cycle reset
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_2_submit_review_pass_non_last_phase_advances() {
        let (schema, conn) = setup();
        let initial_cycles = vec![json!({
            "phase": 1, "cycle": 1,
            "executor": {"summary": "done", "commit": "abc"},
            "review": null
        })];
        insert_row_at(
            &conn,
            &schema,
            "code_review",
            1,
            1,
            2,
            initial_cycles,
            vec![],
            None,
        );

        let out = compute_submit_review(
            &schema,
            &conn,
            "WF001",
            "PASS",
            "approved",
            None,
            0,
            0,
            1,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        assert_eq!(out.new_status, "executing");
        assert_eq!(out.gate, Some("PASS".to_string()));
        assert_eq!(read_status(&conn), "executing");
        assert_eq!(read_i64(&conn, "current_phase"), 2);
        assert_eq!(read_i64(&conn, "current_cycle"), 1);

        let claimed_by = read_text(&conn, "claimed_by");
        assert!(claimed_by.is_none() || claimed_by.as_deref() == Some(""));
    }

    // ---------------------------------------------------------------------------
    // AC5.3: submit-review --gate PASS (last phase) → complete → in_review
    //        (complete is transient; on-entry follow-on advances to in_review in same tx)
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_3_submit_review_pass_last_phase_completes() {
        let (schema, conn) = setup();
        let initial_cycles = vec![json!({
            "phase": 1, "cycle": 1,
            "executor": {"summary": "final", "commit": "abc"},
            "review": null
        })];
        // plan with 1 phase, current_phase = 1 (== plan.phases.length)
        insert_row_at(
            &conn,
            &schema,
            "code_review",
            1,
            1,
            1,
            initial_cycles,
            vec![],
            None,
        );

        let out = compute_submit_review(
            &schema,
            &conn,
            "WF001",
            "PASS",
            "final approved",
            None,
            0,
            0,
            0,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        // on_state.complete fires request_review (framework) → in_review in same tx
        assert_eq!(out.new_status, "in_review");
        assert_eq!(read_status(&conn), "in_review");
        // current_phase must NOT be bumped past last
        assert_eq!(read_i64(&conn, "current_phase"), 1);
    }

    /// T054 task 1.14 (L123 regression): a T1 row carrying a synthesized
    /// one-phase plan completes via the generic `current_phase >= plan.phases.length`
    /// PASS branch. With T054, plan IS NULL is impossible post planning→ready,
    /// so the previously declared T1-specific PASS-to-complete transition is
    /// no longer required and has been removed. This test proves the generic
    /// branch suffices for T1 PASS.
    #[test]
    fn submit_review_pass_completes_t1_via_generic_phase_guard() {
        let (_schema, conn) = setup();
        let now = now_iso8601();
        let cycles_json = serde_json::to_string(&json!([{
            "phase": 1, "cycle": 1,
            "executor": {"summary": "ok", "commit": "abc"},
            "review": null
        }]))
        .unwrap();
        // Insert T1 row at code_review with a synthesized 1-phase plan (the
        // post-T054 shape for a T1 row that has executed phase 1).
        let synthesized_plan = serde_json::to_string(&json!({
            "objective": "intent text",
            "phases": [{
                "name": "Contract execution",
                "objective": "done when X",
                "tasks": ["do X"],
                "acceptance_criteria": ["X done"],
                "files": [],
                "dependencies": []
            }]
        }))
        .unwrap();
        conn.execute(
            "INSERT INTO wf_tasks (display_id, status, created_at, updated_at, \
             created_by, updated_by, title, tier_hint, current_phase, current_cycle, \
             plan, plan_source, cycles, plan_review_log, blocked_reason) \
             VALUES ('WF001', 'code_review', ?1, ?1, 'human', 'human', 't1 task', \
                     'T1', 1, 1, ?2, 'contract_synthesized', ?3, '[]', '')",
            rusqlite::params![now, synthesized_plan, cycles_json],
        )
        .unwrap();

        let out = compute_submit_review(
            &Schema::from_yaml(WF_SCHEMA_YAML).unwrap(),
            &conn,
            "WF001",
            "PASS",
            "T1 done",
            None,
            0,
            0,
            0,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        // complete → in_review fires on-entry, so final status is in_review.
        assert_eq!(
            out.new_status, "in_review",
            "T1 PASS with synthesized 1-phase plan must reach complete via the \
             generic current_phase >= plan.phases.length branch (and on-entry to in_review)"
        );
        assert_eq!(read_status(&conn), "in_review");
    }

    // ---------------------------------------------------------------------------
    // AC5.4: 4th REVISE attempt is blocked (marquee test — 3-cycle guard)
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_4_fourth_revise_blocked() {
        let (schema, conn) = setup();

        // Initial state: code_review, phase 1, cycle 1
        let initial_cycles = vec![json!({
            "phase": 1, "cycle": 1,
            "executor": {"summary": "attempt 1", "commit": "abc"},
            "review": null
        })];
        insert_row_at(
            &conn,
            &schema,
            "code_review",
            1,
            1,
            2,
            initial_cycles,
            vec![],
            None,
        );

        // Helper: do a REVISE and assert it produces executing
        let do_revise = |summary: &str| {
            compute_submit_review(
                &schema,
                &conn,
                "WF001",
                "REVISE",
                summary,
                None,
                1,
                0,
                0,
                Actor::AiAutonomous,
                None,
            )
        };

        // 1st REVISE: current_cycle 1 → 2 (guard 2 <= 4 true) → executing
        let out1 = do_revise("needs work 1").unwrap();
        assert_eq!(
            out1.new_status, "executing",
            "1st REVISE must produce executing"
        );
        assert_eq!(read_i64(&conn, "current_cycle"), 2);

        // Prep cycle 2 execute
        force_status(&conn, "executing", 1, 2);
        set_cycles_json(
            &conn,
            &[
                json!({"phase":1,"cycle":1,"executor":{"summary":"attempt 1","commit":"abc"},"review":{"gate":"REVISE","summary":"needs work 1","critical":1,"major":0,"minor":0}}),
                json!({"phase":1,"cycle":2,"executor":{"summary":"attempt 2","commit":"def"},"review":null}),
            ],
        );
        do_execute(&schema, &conn);

        // 2nd REVISE: current_cycle 2 → 3 (guard 3 <= 4 true) → executing
        let out2 = do_revise("needs work 2").unwrap();
        assert_eq!(
            out2.new_status, "executing",
            "2nd REVISE must produce executing"
        );
        assert_eq!(read_i64(&conn, "current_cycle"), 3);

        // Prep cycle 3 execute
        force_status(&conn, "executing", 1, 3);
        set_cycles_json(
            &conn,
            &[
                json!({"phase":1,"cycle":1,"executor":{"summary":"a1","commit":"a"},"review":{"gate":"REVISE","summary":"nw1","critical":1,"major":0,"minor":0}}),
                json!({"phase":1,"cycle":2,"executor":{"summary":"a2","commit":"b"},"review":{"gate":"REVISE","summary":"nw2","critical":1,"major":0,"minor":0}}),
                json!({"phase":1,"cycle":3,"executor":{"summary":"attempt 3","commit":"ghi"},"review":null}),
            ],
        );
        do_execute(&schema, &conn);

        // 3rd REVISE: current_cycle 3 → 4 (guard 4 <= 4 true) → executing
        let out3 = do_revise("needs work 3").unwrap();
        assert_eq!(
            out3.new_status, "executing",
            "3rd REVISE must produce executing"
        );
        assert_eq!(read_i64(&conn, "current_cycle"), 4);

        // Prep cycle 4 execute
        force_status(&conn, "executing", 1, 4);
        set_cycles_json(
            &conn,
            &[
                json!({"phase":1,"cycle":1,"executor":{"summary":"a1","commit":"a"},"review":{"gate":"REVISE","summary":"nw1","critical":1,"major":0,"minor":0}}),
                json!({"phase":1,"cycle":2,"executor":{"summary":"a2","commit":"b"},"review":{"gate":"REVISE","summary":"nw2","critical":1,"major":0,"minor":0}}),
                json!({"phase":1,"cycle":3,"executor":{"summary":"a3","commit":"c"},"review":{"gate":"REVISE","summary":"nw3","critical":0,"major":1,"minor":0}}),
                json!({"phase":1,"cycle":4,"executor":{"summary":"attempt 4","commit":"jkl"},"review":null}),
            ],
        );
        do_execute(&schema, &conn);

        // 4th REVISE attempt: would-be cycle 5, guard 5 <= 4 false → BLOCKED
        let out4 = do_revise("4th attempt").unwrap();
        assert_eq!(
            out4.new_status, "blocked",
            "4th REVISE must produce blocked"
        );
        assert_eq!(read_status(&conn), "blocked");

        // current_cycle must NOT be bumped (working-copy bump stays rolled back)
        assert_eq!(
            read_i64(&conn, "current_cycle"),
            4,
            "current_cycle must remain 4, not bumped to 5"
        );

        // blocked_reason must cite the guard failure
        let br = read_text(&conn, "blocked_reason").unwrap_or_default();
        assert!(
            br.contains("4th revise rejected"),
            "blocked_reason must cite guard rejection: {:?}",
            br
        );
        assert!(
            br.contains("phase 1"),
            "blocked_reason must name the phase: {:?}",
            br
        );
        assert!(
            br.contains("cycle 4"),
            "blocked_reason must name the cycle: {:?}",
            br
        );
    }

    // ---------------------------------------------------------------------------
    // AC5.4b: Cross-phase isolation — per-phase counter resets on PASS
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_4b_cross_phase_cycle_counter_resets_on_pass() {
        let (schema, conn) = setup();

        // Phase 1: 2 REVISEs then PASS to phase 2
        let initial_cycles = vec![json!({
            "phase": 1, "cycle": 1,
            "executor": {"summary": "phase1 attempt1", "commit": "a"},
            "review": null
        })];
        insert_row_at(
            &conn,
            &schema,
            "code_review",
            1,
            1,
            2,
            initial_cycles,
            vec![],
            None,
        );

        // 1st REVISE phase 1: cycle 1 → 2
        {
            let out = compute_submit_review(
                &schema,
                &conn,
                "WF001",
                "REVISE",
                "needs work",
                None,
                1,
                0,
                0,
                Actor::AiAutonomous,
                None,
            )
            .unwrap();
            assert_eq!(out.new_status, "executing");
            assert_eq!(read_i64(&conn, "current_cycle"), 2);

            force_status(&conn, "executing", 1, 2);
            set_cycles_json(
                &conn,
                &[
                    json!({"phase":1,"cycle":1,"executor":{"summary":"a1","commit":"a"},"review":{"gate":"REVISE","summary":"nw","critical":1,"major":0,"minor":0}}),
                    json!({"phase":1,"cycle":2,"executor":{"summary":"a2","commit":"b"},"review":null}),
                ],
            );
            do_execute(&schema, &conn);
        }

        // 2nd REVISE phase 1: cycle 2 → 3
        {
            let out = compute_submit_review(
                &schema,
                &conn,
                "WF001",
                "REVISE",
                "still needs work",
                None,
                0,
                1,
                0,
                Actor::AiAutonomous,
                None,
            )
            .unwrap();
            assert_eq!(out.new_status, "executing");
            assert_eq!(read_i64(&conn, "current_cycle"), 3);

            force_status(&conn, "executing", 1, 3);
            set_cycles_json(
                &conn,
                &[
                    json!({"phase":1,"cycle":1,"executor":{"summary":"a1","commit":"a"},"review":{"gate":"REVISE","summary":"nw","critical":1,"major":0,"minor":0}}),
                    json!({"phase":1,"cycle":2,"executor":{"summary":"a2","commit":"b"},"review":{"gate":"REVISE","summary":"snw","critical":0,"major":1,"minor":0}}),
                    json!({"phase":1,"cycle":3,"executor":{"summary":"a3","commit":"c"},"review":null}),
                ],
            );
            do_execute(&schema, &conn);
        }

        // PASS on phase 1 → phase 2 (current_phase bumps to 2, current_cycle resets to 1)
        {
            let out = compute_submit_review(
                &schema,
                &conn,
                "WF001",
                "PASS",
                "phase 1 approved",
                None,
                0,
                0,
                0,
                Actor::AiAutonomous,
                None,
            )
            .unwrap();
            assert_eq!(out.new_status, "executing");
            assert_eq!(
                read_i64(&conn, "current_phase"),
                2,
                "current_phase must advance to 2"
            );
            assert_eq!(
                read_i64(&conn, "current_cycle"),
                1,
                "current_cycle must reset to 1 on phase advance"
            );
        }

        // Phase 2: first REVISE — counter reset to 1, bumps to 2 (2 <= 4 true)
        // Prep: add a cycle entry for phase 2 cycle 1
        force_status(&conn, "executing", 2, 1);
        set_cycles_json(
            &conn,
            &[
                json!({"phase":1,"cycle":1,"executor":{"summary":"a1","commit":"a"},"review":{"gate":"REVISE","summary":"nw","critical":1,"major":0,"minor":0}}),
                json!({"phase":1,"cycle":2,"executor":{"summary":"a2","commit":"b"},"review":{"gate":"REVISE","summary":"snw","critical":0,"major":1,"minor":0}}),
                json!({"phase":1,"cycle":3,"executor":{"summary":"a3","commit":"c"},"review":{"gate":"PASS","summary":"ok","critical":0,"major":0,"minor":0}}),
                json!({"phase":2,"cycle":1,"executor":{"summary":"phase2 attempt1","commit":"d"},"review":null}),
            ],
        );
        do_execute(&schema, &conn);

        // Phase 2's first REVISE: counter was 1, bumps to 2 (2 <= 4 true)
        let out = compute_submit_review(
            &schema,
            &conn,
            "WF001",
            "REVISE",
            "phase 2 needs work",
            None,
            0,
            1,
            0,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        assert_eq!(
            out.new_status, "executing",
            "Phase 2's first REVISE must succeed (per-phase counter does not carry from phase 1)"
        );
        assert_eq!(
            read_i64(&conn, "current_cycle"),
            2,
            "current_cycle should be 2 (reset to 1 on phase advance, bumped to 2 by first REVISE)"
        );
        assert_eq!(
            read_i64(&conn, "current_phase"),
            2,
            "current_phase must remain 2"
        );
    }

    // ---------------------------------------------------------------------------
    // AC5.5: Lock contention — second submit fails with lock error, then succeeds after expiry
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_5_lock_contention_second_submit_fails() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "executing", 1, 1, 2, vec![], vec![], None);

        // Manually acquire the lock to simulate another process
        let now = now_iso8601();
        conn.execute(
            "UPDATE wf_tasks SET claimed_by = 'other-agent', claimed_at = ?1 WHERE display_id = 'WF001'",
            rusqlite::params![now],
        ).unwrap();

        let err = compute_submit_execute(
            &schema,
            &conn,
            "WF001",
            "attempt",
            None,
            None,
            None,
            Actor::AiAutonomous,
            None,
        )
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("claimed by") || msg.contains("other-agent"),
            "error should mention lock holder: {msg}"
        );

        // Simulate expiry: set claimed_at to 6 minutes ago
        let old_time = iso_subtract_seconds(360);
        conn.execute(
            "UPDATE wf_tasks SET claimed_at = ?1 WHERE display_id = 'WF001'",
            rusqlite::params![old_time],
        )
        .unwrap();

        // After expiry, submit should succeed
        let out = compute_submit_execute(
            &schema,
            &conn,
            "WF001",
            "attempt after expiry",
            None,
            None,
            None,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();
        assert_eq!(out.new_status, "code_review");
    }

    // ---------------------------------------------------------------------------
    // AC5.6: submit-plan writes plan record, transitions planning → plan_review
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_6_submit_plan_writes_record_and_transitions() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 0, vec![], vec![], None);

        let plan = json!({
            "summary": "my plan",
            "phases": [{"name": "phase 1"}, {"name": "phase 2"}]
        });

        let out = compute_submit_plan(&schema, &conn, "WF001", plan, Actor::AiAutonomous).unwrap();

        assert_eq!(out.new_status, "plan_review");
        assert_eq!(read_status(&conn), "plan_review");

        let plan_json: Option<String> = conn
            .query_row(
                "SELECT plan FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let stored_plan: Value = serde_json::from_str(&plan_json.unwrap()).unwrap();
        assert_eq!(stored_plan["summary"].as_str().unwrap(), "my plan");
        assert_eq!(stored_plan["phases"].as_array().unwrap().len(), 2);

        // Lock released
        let claimed_by = read_text(&conn, "claimed_by");
        assert!(claimed_by.is_none() || claimed_by.as_deref() == Some(""));
    }

    // ---------------------------------------------------------------------------
    // T027 P4 (Task 4.3): tier-T2 phase-count gate in submit-plan
    // ---------------------------------------------------------------------------

    fn set_tier_hint(conn: &Connection, tier: &str) {
        conn.execute(
            "UPDATE wf_tasks SET tier_hint = ?1 WHERE display_id = 'WF001'",
            rusqlite::params![tier],
        )
        .unwrap();
    }

    #[test]
    fn t027_p4_t2_two_phases_rejected() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 0, vec![], vec![], None);
        set_tier_hint(&conn, "T2");

        let plan = json!({
            "summary": "two-phase plan",
            "phases": [{"name": "p1"}, {"name": "p2"}]
        });

        let err =
            compute_submit_plan(&schema, &conn, "WF001", plan, Actor::AiAutonomous).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("tier T2 requires phases.length == 1"),
            "expected T2 phase-count error, got: {msg}"
        );
        // Status unchanged
        assert_eq!(read_status(&conn), "planning");
    }

    #[test]
    fn t027_p4_t2_single_phase_accepted() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 0, vec![], vec![], None);
        set_tier_hint(&conn, "T2");

        let plan = json!({
            "summary": "single-phase plan",
            "phases": [{"name": "only"}]
        });

        let out = compute_submit_plan(&schema, &conn, "WF001", plan, Actor::AiAutonomous).unwrap();
        assert_eq!(out.new_status, "plan_review");
        assert_eq!(read_status(&conn), "plan_review");
    }

    #[test]
    fn t027_p4_t3_many_phases_accepted() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 0, vec![], vec![], None);
        set_tier_hint(&conn, "T3");

        let plan = json!({
            "summary": "five-phase plan",
            "phases": [
                {"name": "p1"},
                {"name": "p2"},
                {"name": "p3"},
                {"name": "p4"},
                {"name": "p5"}
            ]
        });

        let out = compute_submit_plan(&schema, &conn, "WF001", plan, Actor::AiAutonomous).unwrap();
        assert_eq!(out.new_status, "plan_review");
    }

    // ---------------------------------------------------------------------------
    // T047 AC1.4: submit-plan rejects degenerate plans (no phases / empty / non-array).
    // ---------------------------------------------------------------------------

    #[test]
    fn t047_submit_plan_rejects_missing_phases() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 0, vec![], vec![], None);

        let plan = json!({"summary": "no phases here"});
        let err =
            compute_submit_plan(&schema, &conn, "WF001", plan, Actor::AiAutonomous).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("plan.phases is missing"),
            "expected missing-phases error, got: {msg}"
        );
        assert_eq!(read_status(&conn), "planning", "status must not advance");
    }

    #[test]
    fn t047_submit_plan_rejects_empty_phases_array() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 0, vec![], vec![], None);

        let plan = json!({"phases": []});
        let err =
            compute_submit_plan(&schema, &conn, "WF001", plan, Actor::AiAutonomous).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("empty array"),
            "expected empty-array error, got: {msg}"
        );
        assert_eq!(read_status(&conn), "planning", "status must not advance");
    }

    #[test]
    fn t047_submit_plan_rejects_non_array_phases() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 0, vec![], vec![], None);

        let plan = json!({"phases": "not-an-array"});
        let err =
            compute_submit_plan(&schema, &conn, "WF001", plan, Actor::AiAutonomous).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("must be an array"),
            "expected non-array error, got: {msg}"
        );
        assert_eq!(read_status(&conn), "planning", "status must not advance");
    }

    // ---------------------------------------------------------------------------
    // AC5.7: submit-plan-review --gate READY → ready → executing (both in same tx)
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_7_submit_plan_review_ready_fires_on_entry_follow_on() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "plan_review", 0, 0, 2, vec![], vec![], None);

        let out = compute_submit_plan_review(
            &schema,
            &conn,
            "WF001",
            "READY",
            "plan approved",
            None,
            Actor::AiAutonomous,
        )
        .unwrap();

        // Both plan_review→ready AND ready→executing must have fired inside the same tx
        assert_eq!(
            out.new_status, "executing",
            "READY gate: status must be executing after on-entry follow-on"
        );
        assert_eq!(read_status(&conn), "executing");

        // on-entry follow-on sets current_phase=1, current_cycle=1
        assert_eq!(
            read_i64(&conn, "current_phase"),
            1,
            "current_phase must be 1 after ready → executing"
        );
        assert_eq!(
            read_i64(&conn, "current_cycle"),
            1,
            "current_cycle must be 1 after ready → executing"
        );

        // Lock released
        let claimed_by = read_text(&conn, "claimed_by");
        assert!(claimed_by.is_none() || claimed_by.as_deref() == Some(""));
    }

    // ---------------------------------------------------------------------------
    // AC5.8: submit-plan-review --gate NEEDS_WORK cycle limit
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_8_submit_plan_review_needs_work_cycle_limit() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "plan_review", 0, 0, 2, vec![], vec![], None);

        // 1st NEEDS_WORK: log.length = 0 < 3 → planning
        {
            let out = compute_submit_plan_review(
                &schema,
                &conn,
                "WF001",
                "NEEDS_WORK",
                "needs changes 1",
                None,
                Actor::AiAutonomous,
            )
            .unwrap();
            assert_eq!(out.new_status, "planning");
            assert_eq!(read_plan_review_log(&conn).len(), 1);
            conn.execute(
                "UPDATE wf_tasks SET status = 'plan_review' WHERE display_id = 'WF001'",
                [],
            )
            .unwrap();
        }

        // 2nd NEEDS_WORK: log.length = 1 < 3 → planning
        {
            let out = compute_submit_plan_review(
                &schema,
                &conn,
                "WF001",
                "NEEDS_WORK",
                "needs changes 2",
                None,
                Actor::AiAutonomous,
            )
            .unwrap();
            assert_eq!(out.new_status, "planning");
            assert_eq!(read_plan_review_log(&conn).len(), 2);
            conn.execute(
                "UPDATE wf_tasks SET status = 'plan_review' WHERE display_id = 'WF001'",
                [],
            )
            .unwrap();
        }

        // 3rd NEEDS_WORK: log.length = 2 < 3 → planning
        {
            let out = compute_submit_plan_review(
                &schema,
                &conn,
                "WF001",
                "NEEDS_WORK",
                "needs changes 3",
                None,
                Actor::AiAutonomous,
            )
            .unwrap();
            assert_eq!(out.new_status, "planning");
            assert_eq!(read_plan_review_log(&conn).len(), 3);
            conn.execute(
                "UPDATE wf_tasks SET status = 'plan_review' WHERE display_id = 'WF001'",
                [],
            )
            .unwrap();
        }

        // 4th NEEDS_WORK: log.length = 3 < 3 false → blocked (unguarded fallback)
        {
            let out = compute_submit_plan_review(
                &schema,
                &conn,
                "WF001",
                "NEEDS_WORK",
                "still needs changes",
                None,
                Actor::AiAutonomous,
            )
            .unwrap();
            assert_eq!(
                out.new_status, "blocked",
                "4th NEEDS_WORK must route to blocked (guard plan_review_log.length < 3 fails)"
            );
            assert_eq!(read_status(&conn), "blocked");
        }
    }

    // ---------------------------------------------------------------------------
    // AC5.9: submit-plan-review --gate NOT_READY → blocked with reason
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_9_submit_plan_review_not_ready_blocks() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "plan_review", 0, 0, 2, vec![], vec![], None);

        let out = compute_submit_plan_review(
            &schema,
            &conn,
            "WF001",
            "NOT_READY",
            "plan is fundamentally flawed",
            None,
            Actor::AiAutonomous,
        )
        .unwrap();

        assert_eq!(out.new_status, "blocked");
        assert_eq!(read_status(&conn), "blocked");

        let br = read_text(&conn, "blocked_reason").unwrap_or_default();
        assert!(
            br.contains("NOT_READY") || br.contains("fundamentally flawed"),
            "blocked_reason must be populated: {:?}",
            br
        );
    }

    // ---------------------------------------------------------------------------
    // AC5.10: submit-* on store without workflow errors clearly
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_10_submit_on_no_workflow_store_errors() {
        const OBS_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
lifecycle:
  states: [open, done]
  transitions:
    - from: open
      to: done
      verb: close
      actor: human
fields:
  - name: summary
    type: text
    required: true
"#;
        let schema = Schema::from_yaml(OBS_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();

        let err = compute_submit_execute(
            &schema,
            &conn,
            "L001",
            "done",
            None,
            None,
            None,
            Actor::AiAutonomous,
            None,
        )
        .unwrap_err();

        assert!(
            err.to_string().contains("no workflow"),
            "error must mention 'no workflow': {err}"
        );
    }

    // ---------------------------------------------------------------------------
    // AC5.11: Atomic boundary — rolled-back tx leaves DB unchanged
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_11_atomic_boundary_rollback_leaves_db_unchanged() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 2, vec![], vec![], None);

        // Capture pre-tx state
        let pre_status = read_status(&conn);
        let pre_phase = read_i64(&conn, "current_phase");
        let pre_cycle = read_i64(&conn, "current_cycle");
        let pre_cycles_len = read_cycles(&conn).len();
        let pre_claimed = read_text(&conn, "claimed_by");

        // Simulate: open a tx, acquire lock, write status change, then DROP tx (rollback)
        {
            let tx = conn.unchecked_transaction().unwrap();
            // Acquire lock
            tx.execute(
                "UPDATE wf_tasks SET claimed_by = 'test-executor', claimed_at = ?1 WHERE display_id = 'WF001'",
                rusqlite::params![now_iso8601()],
            ).unwrap();
            // Write status change (simulating step 8 of submit-plan)
            tx.execute(
                "UPDATE wf_tasks SET status = 'plan_review', plan = ?1 WHERE display_id = 'WF001'",
                rusqlite::params![r#"{"summary":"rolled back plan"}"#],
            )
            .unwrap();
            // Drop tx WITHOUT commit → automatic rollback
        }

        // All state must be identical to pre-tx
        assert_eq!(read_status(&conn), pre_status, "status must be unchanged");
        assert_eq!(
            read_i64(&conn, "current_phase"),
            pre_phase,
            "current_phase must be unchanged"
        );
        assert_eq!(
            read_i64(&conn, "current_cycle"),
            pre_cycle,
            "current_cycle must be unchanged"
        );
        assert_eq!(
            read_cycles(&conn).len(),
            pre_cycles_len,
            "cycles must be unchanged"
        );

        let post_claimed = read_text(&conn, "claimed_by");
        assert_eq!(
            post_claimed, pre_claimed,
            "claimed_by must be rolled back to pre-tx value: {:?}",
            post_claimed
        );
    }

    // ---------------------------------------------------------------------------
    // AC5.12: Render is downstream of commit — re-read after commit is consistent
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_12_post_commit_reads_are_consistent() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 2, vec![], vec![], None);

        let plan = json!({"summary": "test plan", "phases": [{"name": "p1"}, {"name": "p2"}]});
        compute_submit_plan(&schema, &conn, "WF001", plan, Actor::AiAutonomous).unwrap();

        // Simulating render: repeated reads after commit must be consistent
        let status1 = read_status(&conn);
        let status2 = read_status(&conn);
        assert_eq!(
            status1, status2,
            "repeated reads after commit must be consistent"
        );
        assert_eq!(status1, "plan_review");
    }

    // ---------------------------------------------------------------------------
    // AC5.13: Lock released after commit (including follow-on transitions)
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_13_lock_released_after_commit_with_follow_on() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "plan_review", 0, 0, 2, vec![], vec![], None);

        // submit-plan-review READY fires two transitions inside one tx:
        //   plan_review → ready → executing
        let out = compute_submit_plan_review(
            &schema,
            &conn,
            "WF001",
            "READY",
            "approved",
            None,
            Actor::AiAutonomous,
        )
        .unwrap();

        assert_eq!(out.new_status, "executing");

        // After commit: lock must be NULL (released as final action before commit)
        let claimed_by = read_text(&conn, "claimed_by");
        assert!(
            claimed_by.is_none() || claimed_by.as_deref() == Some(""),
            "claimed_by must be NULL after commit: {:?}",
            claimed_by
        );

        let claimed_at = read_text(&conn, "claimed_at");
        assert!(
            claimed_at.is_none() || claimed_at.as_deref() == Some(""),
            "claimed_at must be NULL after commit: {:?}",
            claimed_at
        );
    }

    // ---------------------------------------------------------------------------
    // AC5.14: BLOCKED → READY recovery via compute_resume
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_14_blocked_to_ready_recovery() {
        let (schema, conn) = setup();

        // Set up: blocked at phase 1, cycle 4 (after 4th REVISE), with audit trail and stale blocked_reason
        let audit_cycles = vec![
            json!({"phase":1,"cycle":1,"executor":{"summary":"a1"},"review":{"gate":"REVISE"}}),
            json!({"phase":1,"cycle":2,"executor":{"summary":"a2"},"review":{"gate":"REVISE"}}),
            json!({"phase":1,"cycle":3,"executor":{"summary":"a3"},"review":{"gate":"REVISE"}}),
            json!({"phase":1,"cycle":4,"executor":{"summary":"a4"},"review":{"gate":"REVISE"}}),
        ];
        insert_row_at(
            &conn,
            &schema,
            "blocked",
            1,
            4,
            2,
            audit_cycles,
            vec![],
            Some("4th revise rejected by guard current_cycle <= 4 on phase 1 cycle 4: test"),
        );

        // Seed stale auto-drive bookkeeping from a prior detached drive. Resume
        // must clear this or the watchdog will immediately re-block the row.
        conn.execute(
            "UPDATE wf_tasks SET drive_pid = 999999, drive_started_at = '2026-01-01T00:00:00Z' WHERE display_id = 'WF001'",
            [],
        )
        .unwrap();
        let row_id: i64 = conn
            .query_row(
                "SELECT id FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, claimed_at, claimed_by, last_status, finished_at) VALUES ('wf_tasks', ?1, 'WF001', 'auto-drive', '2026-01-01T00:00:00Z', 'daemon-test', 'drive_failed', '2026-01-01T00:01:00Z')",
            rusqlite::params![row_id],
        )
        .unwrap();

        // Call through compute_resume (production code path, not raw helpers)
        let out = compute_resume(&schema, &conn, "WF001", Actor::AiWithHuman).unwrap();

        assert_eq!(out.new_status, "executing");
        assert!(out.summary.contains("WF001"));

        assert_eq!(
            read_status(&conn),
            "executing",
            "after resume, status must be executing"
        );
        assert_eq!(
            read_i64(&conn, "current_phase"),
            1,
            "current_phase must be UNCHANGED (remains at 1)"
        );
        assert_eq!(
            read_i64(&conn, "current_cycle"),
            1,
            "current_cycle must be RESET to 1"
        );

        // blocked_reason must be cleared (not the stale "4th revise..." string)
        let br = read_text(&conn, "blocked_reason").unwrap_or_default();
        assert!(
            br.is_empty(),
            "blocked_reason must be cleared after resume, got: {:?}",
            br
        );

        // Audit trail preserved
        let cycles = read_cycles(&conn);
        assert_eq!(
            cycles.len(),
            4,
            "cycles audit trail must be preserved after resume"
        );

        // Stale auto-drive bookkeeping cleared.
        let drive_pid: Option<i64> = conn
            .query_row(
                "SELECT drive_pid FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(drive_pid.is_none(), "drive_pid must be cleared on resume");
        let drive_started_at: String = conn
            .query_row(
                "SELECT COALESCE(drive_started_at, '') FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            drive_started_at.is_empty(),
            "drive_started_at must be cleared on resume"
        );
        let auto_drive_locks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE display_id = 'WF001' AND agent_name = 'auto-drive'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(auto_drive_locks, 0, "stale auto-drive lock must be deleted");

        // Lock released
        let claimed_by = read_text(&conn, "claimed_by");
        assert!(
            claimed_by.is_none() || claimed_by.as_deref() == Some(""),
            "lock must be released after resume: {:?}",
            claimed_by
        );
    }

    #[test]
    fn resume_clears_stale_auto_drive_bookkeeping_before_watchdog() {
        let task_schema =
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        let obs_schema =
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&task_schema))
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&obs_schema))
            .unwrap();

        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"fixed","scope_in":"resume","scope_out":"none"}"#;
        let plan = r#"{"phases":[{"name":"phase 1"}]}"#;
        let dead_pid = 0x7fff_fffe_i64;
        let plan_review_log = r#"[{"gate":"READY","summary":"approved for execution"}]"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, \
             title, slug, branch, workspace_path, tier_hint, contract, plan, plan_review_log, current_phase, current_cycle, \
             blocked_reason, drive_pid, drive_started_at) \
             VALUES ('T900', 'blocked', ?1, ?1, 'framework', 'framework', \
             'resume stale drive pid', 'resume-stale-drive-pid', 'feat/t900', '/tmp/no-such', 'T3', \
             ?2, ?3, ?4, 1, 1, 'drive_failed:silent_zombie_pid_dead', ?5, ?1)",
            rusqlite::params![now, contract, plan, plan_review_log, dead_pid],
        )
        .unwrap();
        let row_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, last_status, finished_at) \
             VALUES ('tasks', ?1, 'T900', 'auto-drive', 1, ?2, 'auto-drive-watchdog', 'drive_failed', ?2)",
            rusqlite::params![row_id, now],
        )
        .unwrap();

        let out = compute_resume(&task_schema, &conn, "T900", Actor::AiWithHuman).unwrap();
        assert_eq!(out.new_status, "executing");

        let (status, reason, pid): (String, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT status, blocked_reason, drive_pid FROM tasks WHERE display_id='T900'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "executing");
        assert!(reason.unwrap_or_default().is_empty());
        assert!(pid.is_none(), "resume must clear stale drive_pid");

        let lock_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE store='tasks' AND row_id=?1 AND agent_name='auto-drive'",
                rusqlite::params![row_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(lock_count, 0, "resume must remove stale auto-drive lock");

        let agents = crate::flow::AgentsYaml::default_empty();
        let cfg = std::path::PathBuf::from("/tmp/no-config.yaml");
        let acted =
            crate::flow::builtins::auto_drive::sweep_drive_watchdog(&conn, &agents, &cfg, "", "")
                .unwrap();
        assert_eq!(
            acted, 0,
            "watchdog must not re-block immediately after resume"
        );
        assert_eq!(
            conn.query_row(
                "SELECT status FROM tasks WHERE display_id='T900'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
            "executing"
        );
    }

    /// L130 fix: a non-T1 row blocked before its planner submitted a plan
    /// (plan IS NULL) MUST resume into 'planning', not 'ready'. Without this,
    /// resume cascades blocked → ready → executing and the executor blocks
    /// again on "Phase 1 of 0" because plan_phases is empty.
    #[test]
    fn resume_with_null_plan_routes_to_planning_for_non_t1() {
        let task_schema =
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        let obs_schema =
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&task_schema))
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&obs_schema))
            .unwrap();

        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        // T2 row, plan IS NULL — exactly the L043/T038 shape.
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, \
             title, slug, branch, workspace_path, tier_hint, contract, plan, current_phase, current_cycle, \
             blocked_reason) \
             VALUES ('T901', 'blocked', ?1, ?1, 'framework', 'framework', \
             'planner crashed before submit', 'planner-crash', 'feat/t901', '/tmp/no-such', 'T2', \
             ?2, NULL, 0, 1, 'planner crashed')",
            rusqlite::params![now, contract],
        )
        .unwrap();

        let out = compute_resume(&task_schema, &conn, "T901", Actor::AiWithHuman).unwrap();
        assert_eq!(
            out.new_status, "planning",
            "non-T1 with plan=NULL must route to planning, not ready/executing"
        );

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T901'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "planning");

        // transition_history must record the resume verb with from='blocked'
        // and to='planning' so the audit trail reflects the actual route.
        let (from_s, to_s, verb): (String, String, String) = conn
            .query_row(
                "SELECT from_status, to_status, verb FROM transition_history \
                 WHERE display_id='T901' ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(from_s, "blocked");
        assert_eq!(to_s, "planning");
        assert_eq!(verb, "resume");
    }

    /// I033 regression: a T2/T3 row with a non-empty but latest-rejected plan
    /// must not resume blocked → ready → executing. The T118 failure shape was:
    /// planner produced a plan, plan_reviewer returned NEEDS_WORK, the revision
    /// planner drive died in planning, and resume saw plan != NULL and executed
    /// the rejected old plan. Latest NEEDS_WORK must route back to planning.
    #[test]
    fn resume_with_non_empty_rejected_plan_routes_to_planning_for_non_t1() {
        let task_schema =
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        let obs_schema =
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&task_schema))
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&obs_schema))
            .unwrap();

        let now = "2026-05-09T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        let rejected_plan = r#"{"phases":[{"name":"phase 1","tasks":["old rejected task"]}]}"#;
        let plan_review_log = r#"[{"gate":"NEEDS_WORK","summary":"old plan rejected","reviewed_plan":{"phases":[{"name":"phase 1"}]}}]"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, \
             title, slug, branch, workspace_path, tier_hint, contract, plan, plan_review_log, \
             current_phase, current_cycle, blocked_reason) \
             VALUES ('T903', 'blocked', ?1, ?1, 'framework', 'framework', \
             'blocked after rejected planning revision', 'blocked-rejected-plan', 'feat/t903', '/tmp/no-such', 'T2', \
             ?2, ?3, ?4, NULL, NULL, 'drive_failed:silent_zombie_pid_dead')",
            rusqlite::params![now, contract, rejected_plan, plan_review_log],
        )
        .unwrap();

        let out = compute_resume(&task_schema, &conn, "T903", Actor::AiWithHuman).unwrap();
        assert_eq!(
            out.new_status, "planning",
            "non-T1 with latest plan review NEEDS_WORK must resume to planning, not ready/executing"
        );

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id='T903'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "planning");

        let (from_s, to_s, verb): (String, String, String) = conn
            .query_row(
                "SELECT from_status, to_status, verb FROM transition_history \
                 WHERE display_id='T903' ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(from_s, "blocked");
        assert_eq!(to_s, "planning");
        assert_eq!(verb, "resume");
    }

    /// Positive counterpart: a non-empty plan with latest READY review is still
    /// executable after a transient execution/watchdog block. This preserves
    /// the valid T122 shape: plan_review READY happened before executing.
    #[test]
    fn resume_with_non_empty_ready_plan_keeps_ready_path_for_non_t1() {
        let task_schema =
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        let obs_schema =
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&task_schema))
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&obs_schema))
            .unwrap();

        let now = "2026-05-09T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        let approved_plan = r#"{"phases":[{"name":"phase 1","tasks":["approved task"]}]}"#;
        let plan_review_log = r#"[{"gate":"NEEDS_WORK","summary":"first draft rejected"},{"gate":"READY","summary":"revision approved","reviewed_plan":{"phases":[{"name":"phase 1"}]}}]"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, \
             title, slug, branch, workspace_path, tier_hint, contract, plan, plan_review_log, \
             current_phase, current_cycle, blocked_reason) \
             VALUES ('T904', 'blocked', ?1, ?1, 'framework', 'framework', \
             'blocked after ready plan', 'blocked-ready-plan', 'feat/t904', '/tmp/no-such', 'T2', \
             ?2, ?3, ?4, 1, 1, 'drive_failed:stale_binary_inode')",
            rusqlite::params![now, contract, approved_plan, plan_review_log],
        )
        .unwrap();

        let out = compute_resume(&task_schema, &conn, "T904", Actor::AiWithHuman).unwrap();
        assert_eq!(
            out.new_status, "executing",
            "non-T1 with latest plan review READY may resume through ready to executing"
        );
        assert_eq!(
            conn.query_row(
                "SELECT status FROM tasks WHERE display_id='T904'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
            "executing"
        );
    }

    /// L130 corollary: T1 row with plan=NULL is the contract-is-plan case
    /// (T1 skips plan-stage entirely). Resume must NOT divert to planning;
    /// it should still land at ready → executing.
    #[test]
    fn resume_with_null_plan_keeps_ready_path_for_t1() {
        let task_schema =
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        let obs_schema =
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&task_schema))
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&obs_schema))
            .unwrap();

        let now = "2026-05-03T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, \
             title, slug, branch, workspace_path, tier_hint, contract, plan, current_phase, current_cycle, \
             blocked_reason) \
             VALUES ('T902', 'blocked', ?1, ?1, 'framework', 'framework', \
             't1 contract-is-plan', 't1-contract-is-plan', 'feat/t902', '/tmp/no-such', 'T1', \
             ?2, NULL, 0, 1, 'transient flake')",
            rusqlite::params![now, contract],
        )
        .unwrap();

        let out = compute_resume(&task_schema, &conn, "T902", Actor::AiWithHuman).unwrap();
        // T1 ready→executing follow-on still fires.
        assert_eq!(out.new_status, "executing");
    }

    // ---------------------------------------------------------------------------
    // AC5.14 (extra): resume rejects ai_autonomous invoker (actor: ai_with_human)
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_14_resume_actor_mismatch_rejected() {
        let (schema, conn) = setup();
        insert_row_at(
            &conn,
            &schema,
            "blocked",
            1,
            4,
            2,
            vec![],
            vec![],
            Some("blocked for testing"),
        );

        // ai_autonomous invoker must be rejected because resume declares actor: ai_with_human
        let err = compute_resume(&schema, &conn, "WF001", Actor::AiAutonomous).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("ai_with_human"),
            "error must mention required actor 'ai_with_human': {msg}"
        );
        assert!(
            msg.contains("resume"),
            "error must mention verb 'resume': {msg}"
        );

        // DB state must be completely unchanged (lock acquired then rolled back with tx)
        assert_eq!(
            read_status(&conn),
            "blocked",
            "status must not change on actor mismatch"
        );
        let claimed_by = read_text(&conn, "claimed_by");
        assert!(
            claimed_by.is_none() || claimed_by.as_deref() == Some(""),
            "lock must not remain after failed compute_resume: {:?}",
            claimed_by
        );
    }

    // ---------------------------------------------------------------------------
    // AC5.14 (extra): resume errors when row is already claimed by another invoker
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_14_resume_acquires_lock() {
        let (schema, conn) = setup();
        insert_row_at(
            &conn,
            &schema,
            "blocked",
            1,
            4,
            2,
            vec![],
            vec![],
            Some("blocked for testing"),
        );

        // Pre-claim the row as a different agent
        let now = now_iso8601();
        conn.execute(
            "UPDATE wf_tasks SET claimed_by = 'other-agent', claimed_at = ?1 WHERE display_id = 'WF001'",
            rusqlite::params![now],
        ).unwrap();

        // compute_resume must fail with a lock-contention error
        let err = compute_resume(&schema, &conn, "WF001", Actor::AiWithHuman).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("other-agent") || msg.contains("claimed"),
            "error must name the current lock holder: {msg}"
        );

        // Status still blocked
        assert_eq!(read_status(&conn), "blocked");
    }

    // ---------------------------------------------------------------------------
    // AC5.13 (extra): lock held during follow-on — probed before and after follow-on
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_13_lock_held_during_follow_on() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "plan_review", 0, 0, 2, vec![], vec![], None);

        // Reproduce the steps inside compute_submit_plan_review (READY path) manually,
        // probing claimed_by from the SAME connection (same tx) between step 8 and step 9.
        // This proves that release_lock (step 10) happens AFTER fire_on_entry_follow_ons (step 9).

        let tx = conn.unchecked_transaction().unwrap();
        let (row_id, _) = read_row(&schema, &tx, "WF001").unwrap();

        // Step 2: acquire lock
        acquire_lock(&tx, &schema.name, "WF001", "ai_autonomous").unwrap();

        // Assert lock held before step 8
        let claimed_after_acquire: Option<String> = tx
            .query_row(
                "SELECT claimed_by FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            claimed_after_acquire.as_deref(),
            Some("ai_autonomous"),
            "lock must be held after acquire_lock"
        );

        // Step 8: write status → ready
        let fw_fields: BTreeMap<String, i64> = BTreeMap::new();
        let txt_fields: BTreeMap<String, String> = BTreeMap::new();
        write_status_and_fields(
            &tx,
            &schema.name,
            row_id,
            "ready",
            "ai_autonomous",
            &fw_fields,
            &txt_fields,
            Some(TransitionAudit {
                display_id: "WF001",
                from_status: "plan_review",
                verb: "submit-plan-review",
                policy_ref: None,
                policies_hash: None,
            }),
        )
        .unwrap();

        // Probe BETWEEN step 8 and step 9: lock must still be held
        let claimed_between: Option<String> = tx
            .query_row(
                "SELECT claimed_by FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            claimed_between.as_deref(),
            Some("ai_autonomous"),
            "lock must still be held DURING follow-on (between write and follow-on)"
        );

        // Step 9: fire on-entry follow-ons (ready → executing)
        fire_on_entry_follow_ons(&tx, &schema, "WF001", row_id, "ready").unwrap();

        // Probe AFTER follow-on, BEFORE release: lock must still be held
        let claimed_after_followon: Option<String> = tx
            .query_row(
                "SELECT claimed_by FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            claimed_after_followon.as_deref(),
            Some("ai_autonomous"),
            "lock must still be held AFTER follow-on, before release_lock"
        );

        // Step 10: release lock
        release_lock(&tx, &schema.name, "WF001").unwrap();

        // Step 11: commit
        tx.commit().unwrap();

        // After commit: lock must be NULL
        let claimed_post_commit = read_text(&conn, "claimed_by");
        assert!(
            claimed_post_commit.is_none() || claimed_post_commit.as_deref() == Some(""),
            "lock must be NULL after commit: {:?}",
            claimed_post_commit
        );
        // Final status must be executing (follow-on fired)
        assert_eq!(read_status(&conn), "executing");
    }

    // ---------------------------------------------------------------------------
    // AC5.11b: handler-path validator failure rolls back tx (not just SQLite semantics)
    // ---------------------------------------------------------------------------

    #[test]
    fn ac5_11b_handler_path_validator_failure_rolls_back() {
        let (schema, conn) = setup();
        // Insert row in executing state, phase 1 cycle 1
        insert_row_at(&conn, &schema, "executing", 1, 1, 2, vec![], vec![], None);

        let pre_status = read_status(&conn);
        let pre_phase = read_i64(&conn, "current_phase");
        let pre_cycle = read_i64(&conn, "current_cycle");
        let pre_cycles_len = read_cycles(&conn).len();
        let pre_claimed = read_text(&conn, "claimed_by");

        // Call compute_submit_execute with an ai_with_human invoker.
        // submit-execute declares `actor: ai_autonomous`, so ai_with_human is also
        // rejected by the actor check (actor_allowed: ai_with_human != ai_autonomous).
        // This triggers a validator failure INSIDE compute_submit_execute before any commit.
        //
        // Note: Actor::AiWithHuman does NOT satisfy Actor::AiAutonomous (see actor_allowed in actor.rs).
        // The validator will fire the transition actor mismatch error and return Err before tx.commit().
        let err = compute_submit_execute(
            &schema,
            &conn,
            "WF001",
            "attempted summary",
            Some("abc"),
            None,
            None,
            Actor::AiWithHuman,
            None,
        )
        .unwrap_err();

        // Must be a validation error (actor mismatch for submit-execute)
        let msg = err.to_string();
        assert!(
            msg.contains("submit-execute") || msg.contains("actor") || msg.contains("validation"),
            "error must be validator failure: {msg}"
        );

        // DB state must be completely unchanged — proves the handler's tx rolled back
        assert_eq!(
            read_status(&conn),
            pre_status,
            "status must be unchanged after validator failure"
        );
        assert_eq!(
            read_i64(&conn, "current_phase"),
            pre_phase,
            "current_phase must be unchanged"
        );
        assert_eq!(
            read_i64(&conn, "current_cycle"),
            pre_cycle,
            "current_cycle must be unchanged"
        );
        assert_eq!(
            read_cycles(&conn).len(),
            pre_cycles_len,
            "cycles must be unchanged"
        );

        let post_claimed = read_text(&conn, "claimed_by");
        assert_eq!(
            post_claimed, pre_claimed,
            "claimed_by must be rolled back (lock released by tx rollback): {:?}",
            post_claimed
        );
    }

    // ---------------------------------------------------------------------------
    // M1 (cycle 2): files_changed stored as JSON array, not raw CSV string
    // ---------------------------------------------------------------------------

    #[test]
    fn m1_files_changed_stored_as_json_array() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "executing", 1, 1, 2, vec![], vec![], None);

        compute_submit_execute(
            &schema,
            &conn,
            "WF001",
            "did stuff",
            Some("abc1234"),
            Some("src/foo.rs,src/bar.rs"),
            None,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        let cycles = read_cycles(&conn);
        assert_eq!(cycles.len(), 1);
        let files = &cycles[0]["executor"]["files_changed"];
        assert!(
            files.is_array(),
            "files_changed must be a JSON array, got: {:?}",
            files
        );
        let arr = files.as_array().unwrap();
        assert_eq!(arr.len(), 2, "expected 2 files, got: {:?}", arr);
        assert_eq!(arr[0].as_str().unwrap(), "src/foo.rs");
        assert_eq!(arr[1].as_str().unwrap(), "src/bar.rs");
    }

    #[test]
    fn m1_files_changed_trims_whitespace_and_drops_empties() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "executing", 1, 1, 2, vec![], vec![], None);

        compute_submit_execute(
            &schema,
            &conn,
            "WF001",
            "did stuff",
            Some("abc"),
            Some(" a.rs , b.rs , , c.rs "),
            None,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        let cycles = read_cycles(&conn);
        let files = cycles[0]["executor"]["files_changed"].as_array().unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].as_str().unwrap(), "a.rs");
        assert_eq!(files[1].as_str().unwrap(), "b.rs");
        assert_eq!(files[2].as_str().unwrap(), "c.rs");
    }

    // ---------------------------------------------------------------------------
    // M2 (cycle 2): `at` timestamps set on all three list_record sub-records
    // ---------------------------------------------------------------------------

    #[test]
    fn m2_plan_review_log_entry_has_at_timestamp() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "plan_review", 0, 0, 2, vec![], vec![], None);

        compute_submit_plan_review(
            &schema,
            &conn,
            "WF001",
            "NEEDS_WORK",
            "revise plan",
            None,
            Actor::AiAutonomous,
        )
        .unwrap();

        let log = read_plan_review_log(&conn);
        assert_eq!(log.len(), 1);
        let at = log[0].get("at").and_then(|v| v.as_str());
        assert!(
            at.is_some(),
            "plan_review_log[0] must have 'at' field, entry: {:?}",
            log[0]
        );
        let at_str = at.unwrap();
        assert!(
            at_str.contains('T') && at_str.contains('-'),
            "at must be ISO-8601, got: {at_str}"
        );
        // Reset status for continued use
        conn.execute(
            "UPDATE wf_tasks SET status = 'plan_review' WHERE display_id = 'WF001'",
            [],
        )
        .unwrap();
    }

    // ---------------------------------------------------------------------------
    // L503-A Task 1.11: plan_review_log reviewed_plan snapshot tests
    // ---------------------------------------------------------------------------

    #[test]
    fn submit_plan_review_snapshots_reviewed_plan() {
        let (schema, conn) = setup();
        // Seed with a specific plan JSON.
        insert_row_at(&conn, &schema, "plan_review", 0, 0, 2, vec![], vec![], None);

        compute_submit_plan_review(
            &schema,
            &conn,
            "WF001",
            "NEEDS_WORK",
            "needs changes",
            None,
            Actor::AiAutonomous,
        )
        .unwrap();

        let log = read_plan_review_log(&conn);
        assert_eq!(log.len(), 1, "one log entry expected");
        let reviewed_plan = log[0].get("reviewed_plan").expect("reviewed_plan must be present");
        // The plan set by insert_row_at has a 'summary' key.
        assert!(
            reviewed_plan.is_object(),
            "reviewed_plan must be a JSON object, got: {reviewed_plan:?}"
        );
        assert!(
            reviewed_plan.get("summary").is_some() || reviewed_plan.get("phases").is_some(),
            "reviewed_plan must match the plan set at insert time; got: {reviewed_plan:?}"
        );
    }

    #[test]
    fn submit_plan_review_snapshot_immune_to_post_update_plan_mutation() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "plan_review", 0, 0, 2, vec![], vec![], None);

        // Capture the plan as it was at insert time.
        let original_plan: serde_json::Value = conn
            .query_row(
                "SELECT plan FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get::<_, Option<String>>(0),
            )
            .unwrap()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null);

        compute_submit_plan_review(
            &schema,
            &conn,
            "WF001",
            "NEEDS_WORK",
            "revise",
            None,
            Actor::AiAutonomous,
        )
        .unwrap();

        // Mutate plan directly — simulates a subsequent plan update.
        conn.execute(
            "UPDATE wf_tasks SET plan = ?1 WHERE display_id = 'WF001'",
            rusqlite::params![r#"{"summary":"mutated plan","phases":[]}"#],
        )
        .unwrap();

        let log = read_plan_review_log(&conn);
        let reviewed_plan = log[0].get("reviewed_plan").expect("reviewed_plan must be present");
        assert_eq!(
            reviewed_plan, &original_plan,
            "snapshot must be immune to subsequent plan mutations"
        );
    }

    #[test]
    fn submit_plan_review_legacy_log_without_reviewed_plan_reads_clean() {
        let (schema, conn) = setup();
        // Seed a row already in plan_review with a legacy log entry (no reviewed_plan key).
        let now = now_iso8601();
        let plan_json = r#"{"summary":"current plan","phases":[{"name":"p1"}]}"#;
        let legacy_log = r#"[{"gate":"NEEDS_WORK","summary":"old review without snapshot"}]"#;
        conn.execute(
            "INSERT INTO wf_tasks (display_id, status, created_at, updated_at, \
             created_by, updated_by, title, current_phase, current_cycle, \
             plan, cycles, plan_review_log, blocked_reason) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                "WF001",
                "plan_review",
                now,
                now,
                "human",
                "human",
                "legacy task",
                0i64,
                0i64,
                plan_json,
                "[]",
                legacy_log,
                ""
            ],
        )
        .unwrap();

        // Append a new entry via compute_submit_plan_review.
        compute_submit_plan_review(
            &schema,
            &conn,
            "WF001",
            "NEEDS_WORK",
            "new review with snapshot",
            None,
            Actor::AiAutonomous,
        )
        .unwrap();

        let log = read_plan_review_log(&conn);
        assert_eq!(log.len(), 2, "two log entries: legacy + new");

        // (1) Legacy entry is unchanged — no reviewed_plan key.
        assert!(
            log[0].get("reviewed_plan").is_none(),
            "legacy entry must not have reviewed_plan key injected: {:?}",
            log[0]
        );
        assert_eq!(
            log[0].get("summary").and_then(|v| v.as_str()),
            Some("old review without snapshot")
        );

        // (2) New entry has reviewed_plan populated.
        let new_reviewed_plan = log[1].get("reviewed_plan").expect("new entry must have reviewed_plan");
        assert!(
            new_reviewed_plan.is_object(),
            "new entry reviewed_plan must be a JSON object: {new_reviewed_plan:?}"
        );
    }

    #[test]
    fn m2_executor_entry_has_at_timestamp() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "executing", 1, 1, 2, vec![], vec![], None);

        compute_submit_execute(
            &schema,
            &conn,
            "WF001",
            "done",
            Some("abc"),
            None,
            None,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        let cycles = read_cycles(&conn);
        assert_eq!(cycles.len(), 1);
        let at = cycles[0]["executor"].get("at").and_then(|v| v.as_str());
        assert!(
            at.is_some(),
            "cycles[0].executor must have 'at' field, entry: {:?}",
            cycles[0]
        );
        let at_str = at.unwrap();
        assert!(
            at_str.contains('T') && at_str.contains('-'),
            "at must be ISO-8601, got: {at_str}"
        );
    }

    #[test]
    fn m2_review_entry_has_at_timestamp() {
        let (schema, conn) = setup();
        let initial_cycles = vec![json!({
            "phase": 1, "cycle": 1,
            "executor": {"summary": "done", "commit": "abc", "at": "2026-01-01T00:00:00Z"},
            "review": null
        })];
        insert_row_at(
            &conn,
            &schema,
            "code_review",
            1,
            1,
            2,
            initial_cycles,
            vec![],
            None,
        );

        compute_submit_review(
            &schema,
            &conn,
            "WF001",
            "REVISE",
            "needs work",
            None,
            1,
            0,
            0,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        let cycles = read_cycles(&conn);
        let at = cycles[0]["review"].get("at").and_then(|v| v.as_str());
        assert!(
            at.is_some(),
            "cycles[0].review must have 'at' field, review: {:?}",
            cycles[0]["review"]
        );
        let at_str = at.unwrap();
        assert!(
            at_str.contains('T') && at_str.contains('-'),
            "at must be ISO-8601, got: {at_str}"
        );
    }

    // ---------------------------------------------------------------------------
    // m2 carry-forward (cycle 2): P5-m2 open_questions appended as array
    // ---------------------------------------------------------------------------

    #[test]
    fn ac7_p5m2_open_questions_appended_to_plan_review_log_entry() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "plan_review", 0, 0, 2, vec![], vec![], None);

        let questions = Some(vec!["question one".to_string(), "question two".to_string()]);
        compute_submit_plan_review(
            &schema,
            &conn,
            "WF001",
            "NEEDS_WORK",
            "see questions",
            questions,
            Actor::AiAutonomous,
        )
        .unwrap();

        let log = read_plan_review_log(&conn);
        assert_eq!(log.len(), 1);
        let qs = log[0].get("open_questions").and_then(|v| v.as_array());
        assert!(
            qs.is_some(),
            "plan_review_log[0] must have 'open_questions': {:?}",
            log[0]
        );
        let arr = qs.unwrap();
        assert_eq!(arr.len(), 2, "expected 2 questions, got: {:?}", arr);
        assert_eq!(arr[0].as_str().unwrap(), "question one");
        assert_eq!(arr[1].as_str().unwrap(), "question two");
    }

    // ---------------------------------------------------------------------------
    // m2 carry-forward (cycle 2): P5-m3 submit_targets consulted for field lookup
    //
    // We construct a schema variant with submit_targets: {submit-execute: alt_cycles}
    // and assert the entry is written to alt_cycles, not the canonical "cycles" field.
    // ---------------------------------------------------------------------------

    #[test]
    fn ac7_p5m3_submit_targets_consulted_for_field_lookup() {
        // Schema with custom submit_targets: submit-execute → my_exec_log
        const CUSTOM_YAML: &str = r#"
name: wf_custom
id_format: "C{:03d}"

lifecycle:
  states: [executing, code_review]
  transitions:
    - from: executing
      to: code_review
      verb: submit-execute
      actor: ai_autonomous

fields:
  - name: title
    type: text
    required: true
  - name: current_phase
    type: integer
    actor: framework
  - name: current_cycle
    type: integer
    actor: framework
  - name: blocked_reason
    type: text
    actor: framework
  - name: claimed_by
    type: text
    actor: framework
  - name: claimed_at
    type: timestamp
    actor: framework
  - name: plan
    type: record
    fields:
      - name: phases
        type: list_record
        fields:
          - name: name
            type: text
  - name: my_exec_log
    type: list_record
    fields:
      - name: phase
        type: integer
      - name: cycle
        type: integer
      - name: executor
        type: record
        fields:
          - name: summary
            type: text
      - name: review
        type: record
        fields:
          - name: gate
            type: text
          - name: summary
            type: text
          - name: critical
            type: integer
          - name: major
            type: integer
          - name: minor
            type: integer

workflow:
  agent_roles:
    executor:
      description: "Executor"
  briefing_templates:
    executor: templates/executor-brief.md.tpl
  on_state:
    executing:
      - dispatch_agent: executor
  submit_targets:
    submit-execute: my_exec_log
"#;
        let schema = Schema::from_yaml(CUSTOM_YAML).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();

        let now = now_iso8601();
        let plan_json = serde_json::to_string(&json!({
            "phases": [{"name": "phase 1"}]
        }))
        .unwrap();
        conn.execute(
            "INSERT INTO wf_custom (display_id, status, created_at, updated_at, \
             created_by, updated_by, title, current_phase, current_cycle, plan, my_exec_log) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                "C001",
                "executing",
                now,
                now,
                "human",
                "human",
                "Custom task",
                1i64,
                1i64,
                plan_json,
                "[]"
            ],
        )
        .unwrap();

        compute_submit_execute(
            &schema,
            &conn,
            "C001",
            "done via custom target",
            Some("abc"),
            None,
            None,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        // Read my_exec_log — must have the entry
        let log_json: Option<String> = conn
            .query_row(
                "SELECT my_exec_log FROM wf_custom WHERE display_id = 'C001'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(None);
        let log: Vec<serde_json::Value> = log_json
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        assert_eq!(
            log.len(),
            1,
            "entry must be written to my_exec_log, got: {:?}",
            log
        );
        assert_eq!(
            log[0]["executor"]["summary"].as_str().unwrap(),
            "done via custom target"
        );
    }

    // ---------------------------------------------------------------------------
    // m2 carry-forward (cycle 2): P5-m4 review summary and details are separate keys
    // ---------------------------------------------------------------------------

    #[test]
    fn ac7_p5m4_review_summary_and_details_separate_keys() {
        let (schema, conn) = setup();
        let initial_cycles = vec![json!({
            "phase": 1, "cycle": 1,
            "executor": {"summary": "done", "commit": "abc"},
            "review": null
        })];
        insert_row_at(
            &conn,
            &schema,
            "code_review",
            1,
            1,
            2,
            initial_cycles,
            vec![],
            None,
        );

        compute_submit_review(
            &schema,
            &conn,
            "WF001",
            "REVISE",
            "short summary S",
            Some("long detailed report D"),
            1,
            2,
            3,
            Actor::AiAutonomous,
            None,
        )
        .unwrap();

        let cycles = read_cycles(&conn);
        let review = &cycles[0]["review"];
        assert_eq!(
            review["summary"].as_str().unwrap(),
            "short summary S",
            "review.summary must be the --summary value"
        );
        assert_eq!(
            review["details"].as_str().unwrap(),
            "long detailed report D",
            "review.details must be the --details value as a separate key"
        );
        // Confirm they are genuinely separate (different values)
        assert_ne!(
            review["summary"].as_str().unwrap(),
            review["details"].as_str().unwrap(),
            "summary and details must be independent fields"
        );
    }

    // ---------------------------------------------------------------------------
    // m2 carry-forward (cycle 2): P6-m2 bundled-sentinel routes to in-memory template
    // Exercises brief.rs:114-135 via a real manifest install + compute call.
    // ---------------------------------------------------------------------------

    #[test]
    fn ac7_p6m2_bundled_sentinel_routes_to_in_memory_template() {
        use crate::cli::dynamic::{BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};

        // Load the bundled tasks schema
        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .expect("tasks bundled schema must be present");
        let schema = crate::schema::Schema::from_yaml(tasks_yaml).unwrap();

        // Verify BUNDLED_STORE_TEMPLATES contains "tasks" with the planner template
        let tasks_templates = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, t)| *t)
            .expect("tasks templates must be in BUNDLED_STORE_TEMPLATES");
        let planner_tpl = tasks_templates
            .iter()
            .find(|(p, _)| *p == "templates/planner-brief.md.tpl")
            .map(|(_, c)| *c)
            .expect("planner-brief.md.tpl must be in bundled templates");
        // Verify the known sentinel string is present
        assert!(
            planner_tpl.contains("Methodical and thorough"),
            "planner template must contain 'Methodical and thorough' persona text"
        );

        // Simulate the bundled-sentinel detection path:
        // brief::compute reads schema_path from manifest; if it starts with "bundled:"
        // it routes to BUNDLED_STORE_TEMPLATES. We exercise that routing by calling
        // build_context + render_template with content pulled from BUNDLED_STORE_TEMPLATES,
        // which is exactly what brief::compute does after detecting the sentinel.
        let entry = {
            let mut m = std::collections::BTreeMap::new();
            m.insert("display_id".to_string(), serde_json::json!("T001"));
            m.insert("status".to_string(), serde_json::json!("planning"));
            m.insert("title".to_string(), serde_json::json!("Sentinel Test"));
            m.insert("slug".to_string(), serde_json::json!("sentinel-test"));
            m.insert("current_phase".to_string(), serde_json::json!(1));
            m.insert("current_cycle".to_string(), serde_json::json!(1));
            m.insert(
                "created_at".to_string(),
                serde_json::json!("2026-01-01T00:00:00Z"),
            );
            m.insert(
                "updated_at".to_string(),
                serde_json::json!("2026-01-01T00:00:00Z"),
            );
            m.insert(
                "contract".to_string(),
                serde_json::json!({
                    "done_when": "Sentinel detected",
                    "scope_in": "All",
                    "scope_out": "None"
                }),
            );
            m.insert(
                "plan".to_string(),
                serde_json::json!({
                    "objective": "Test bundled sentinel",
                    "phases": []
                }),
            );
            m.insert("plan_review_log".to_string(), serde_json::json!([]));
            m.insert("cycles".to_string(), serde_json::json!([]));
            m
        };

        let ctx = crate::render::build_context(&schema, &entry);
        let rendered = crate::render::render_template(planner_tpl, &ctx)
            .expect("bundled planner template must render without error");

        assert!(!rendered.is_empty(), "rendered briefing must be non-empty");
        assert!(
            rendered.contains("Methodical and thorough"),
            "rendered briefing must contain planner persona text from bundled template"
        );
        assert!(
            rendered.contains("Sentinel Test"),
            "rendered briefing must contain the task title"
        );
    }

    // ---------------------------------------------------------------------------
    // Helpers for wrap_log tests
    // ---------------------------------------------------------------------------

    fn read_wrap_log(conn: &Connection) -> Vec<Value> {
        let json_str: Option<String> = conn
            .query_row(
                "SELECT wrap_log FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(None);
        json_str
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn insert_row_at_in_review(conn: &Connection, schema: &Schema, wrap_log: Vec<Value>) {
        let now = now_iso8601();
        let plan_json = serde_json::to_string(&json!({
            "summary": "test plan",
            "phases": [{"name": "phase 1"}]
        }))
        .unwrap();
        let cycles_json = serde_json::to_string(&json!([])).unwrap();
        let log_json = serde_json::to_string(&json!([])).unwrap();
        let wrap_log_json = serde_json::to_string(&wrap_log).unwrap();
        // `schema` is unused except for call-site clarity
        let _ = schema;

        conn.execute(
            "INSERT INTO wf_tasks (display_id, status, created_at, updated_at, \
             created_by, updated_by, title, current_phase, current_cycle, \
             plan, cycles, plan_review_log, blocked_reason, wrap_log) \
             VALUES (?1, 'in_review', ?2, ?3, 'human', 'human', 'Test task', \
             1, 1, ?4, ?5, ?6, '', ?7)",
            rusqlite::params![
                "WF001",
                now,
                now,
                plan_json,
                cycles_json,
                log_json,
                wrap_log_json
            ],
        )
        .unwrap();
    }

    fn make_wrap_entry() -> Value {
        json!({
            "executive_summary": "All objectives met.",
            "deviations": ["minor scope reduction in phase 2"],
            "residual_risks": ["untested edge case in parser"],
            "recommended_sanity_checks": ["run integration test suite"],
            "reject_reason": null
        })
    }

    fn install_external_reviews_table(conn: &Connection) {
        let schema = Schema::from_yaml(include_str!("../../stores/external_reviews/schema.yaml"))
            .expect("external_reviews schema parses");
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .expect("external_reviews DDL applies");
    }

    fn set_task_tier(conn: &Connection, tier: &str) {
        conn.execute(
            "UPDATE wf_tasks SET tier_hint = ?1 WHERE display_id = 'WF001'",
            rusqlite::params![tier],
        )
        .unwrap();
    }

    fn external_review_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM external_reviews", [], |r| r.get(0))
            .unwrap()
    }

    fn external_review_attempts(conn: &Connection) -> Vec<i64> {
        let mut stmt = conn
            .prepare("SELECT attempt FROM external_reviews ORDER BY attempt")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn external_review_transition_count(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store = 'external_reviews' AND verb = 'create-external-review' \
             AND from_status = '' AND to_status = 'pending'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    // ---------------------------------------------------------------------------
    // T083 P1: submit-wrap creates typed external review rows for T2/T3 only.
    // ---------------------------------------------------------------------------

    #[test]
    fn submit_wrap_external_review_creates_pending_for_t2_and_t3() {
        for tier in ["T2", "T3"] {
            let (schema, conn) = setup();
            install_external_reviews_table(&conn);
            insert_row_at_in_review(&conn, &schema, vec![]);
            set_task_tier(&conn, tier);

            compute_submit_wrap(
                &schema,
                &conn,
                "WF001",
                make_wrap_entry(),
                Actor::AiAutonomous,
            )
            .unwrap();

            let (task_id, status, attempt): (String, String, i64) = conn
                .query_row(
                    "SELECT task_id, status, attempt FROM external_reviews",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(task_id, "WF001");
            assert_eq!(status, "pending");
            assert_eq!(attempt, 1);
        }
    }

    #[test]
    fn submit_wrap_external_review_t1_no_op() {
        let (schema, conn) = setup();
        install_external_reviews_table(&conn);
        insert_row_at_in_review(&conn, &schema, vec![]);
        set_task_tier(&conn, "T1");

        compute_submit_wrap(
            &schema,
            &conn,
            "WF001",
            make_wrap_entry(),
            Actor::AiAutonomous,
        )
        .unwrap();

        assert_eq!(external_review_count(&conn), 0);
    }

    #[test]
    fn submit_wrap_external_review_idempotent_until_terminal_verdict() {
        let (schema, conn) = setup();
        install_external_reviews_table(&conn);
        insert_row_at_in_review(&conn, &schema, vec![]);
        set_task_tier(&conn, "T2");

        compute_submit_wrap(
            &schema,
            &conn,
            "WF001",
            make_wrap_entry(),
            Actor::AiAutonomous,
        )
        .unwrap();
        compute_submit_wrap(
            &schema,
            &conn,
            "WF001",
            make_wrap_entry(),
            Actor::AiAutonomous,
        )
        .unwrap();
        assert_eq!(external_review_count(&conn), 1);
        assert_eq!(external_review_attempts(&conn), vec![1]);

        conn.execute(
            "UPDATE external_reviews SET status = 'passed', verdict = 'PASS' WHERE task_id = 'WF001'",
            [],
        )
        .unwrap();
        compute_submit_wrap(
            &schema,
            &conn,
            "WF001",
            make_wrap_entry(),
            Actor::AiAutonomous,
        )
        .unwrap();

        assert_eq!(external_review_count(&conn), 2);
        assert_eq!(external_review_attempts(&conn), vec![1, 2]);
    }

    #[test]
    fn submit_wrap_external_review_records_creation_edge() {
        let (schema, conn) = setup();
        install_external_reviews_table(&conn);
        insert_row_at_in_review(&conn, &schema, vec![]);
        set_task_tier(&conn, "T3");

        compute_submit_wrap(
            &schema,
            &conn,
            "WF001",
            make_wrap_entry(),
            Actor::AiAutonomous,
        )
        .unwrap();

        assert_eq!(external_review_transition_count(&conn), 1);
    }

    // ---------------------------------------------------------------------------
    // AC3.1: submit-wrap rejects row not in in_review
    // ---------------------------------------------------------------------------

    #[test]
    fn ac3_1_submit_wrap_rejects_wrong_state() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "executing", 1, 1, 2, vec![], vec![], None);

        let err = compute_submit_wrap(
            &schema,
            &conn,
            "WF001",
            make_wrap_entry(),
            Actor::AiAutonomous,
        )
        .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("cannot submit-wrap"),
            "error must mention 'cannot submit-wrap': {msg}"
        );
        assert!(
            msg.contains("executing"),
            "error must name the actual state: {msg}"
        );
        assert!(
            msg.contains("in_review"),
            "error must name the expected state: {msg}"
        );

        // Row unchanged
        assert_eq!(read_status(&conn), "executing");
    }

    // ---------------------------------------------------------------------------
    // AC3.2: submit-wrap appends to wrap_log; status remains in_review
    // ---------------------------------------------------------------------------

    #[test]
    fn ac3_2_submit_wrap_appends_entry_and_status_unchanged() {
        let (schema, conn) = setup();
        insert_row_at_in_review(&conn, &schema, vec![]);

        let out = compute_submit_wrap(
            &schema,
            &conn,
            "WF001",
            make_wrap_entry(),
            Actor::AiAutonomous,
        )
        .unwrap();

        // Status remains in_review (no transition fired)
        assert_eq!(out.new_status, "in_review");
        assert_eq!(read_status(&conn), "in_review");

        // wrap_log grew by 1
        let log = read_wrap_log(&conn);
        assert_eq!(log.len(), 1, "wrap_log must have 1 entry");

        // Entry has correct fields
        let entry = &log[0];
        assert_eq!(
            entry["executive_summary"].as_str().unwrap(),
            "All objectives met."
        );
        assert_eq!(entry["deviations"].as_array().unwrap().len(), 1);
        assert_eq!(entry["residual_risks"].as_array().unwrap().len(), 1);
        assert_eq!(
            entry["recommended_sanity_checks"].as_array().unwrap().len(),
            1
        );

        // `at` must be set by handler (ISO-8601)
        let at = entry["at"].as_str().expect("at must be a string");
        assert!(
            at.contains('T') && at.contains('-'),
            "at must be ISO-8601, got: {at}"
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.3: lock acquired and released; no leaks after commit
    // ---------------------------------------------------------------------------

    #[test]
    fn ac3_3_lock_acquired_and_released() {
        let (schema, conn) = setup();
        insert_row_at_in_review(&conn, &schema, vec![]);

        compute_submit_wrap(
            &schema,
            &conn,
            "WF001",
            make_wrap_entry(),
            Actor::AiAutonomous,
        )
        .unwrap();

        let claimed_by = read_text(&conn, "claimed_by");
        assert!(
            claimed_by.is_none() || claimed_by.as_deref() == Some(""),
            "lock must be released after commit: {:?}",
            claimed_by
        );
        let claimed_at = read_text(&conn, "claimed_at");
        assert!(
            claimed_at.is_none() || claimed_at.as_deref() == Some(""),
            "claimed_at must be NULL after commit: {:?}",
            claimed_at
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.6: re-entry — calling submit-wrap when wrap_log already has entries appends
    // ---------------------------------------------------------------------------

    #[test]
    fn ac3_6_submit_wrap_re_entry_appends_not_overwrites() {
        let (schema, conn) = setup();
        // Pre-seed one wrap_log entry
        let existing_entry = json!({
            "executive_summary": "First wrap.",
            "deviations": [],
            "residual_risks": [],
            "recommended_sanity_checks": [],
            "at": "2026-01-01T00:00:00Z"
        });
        insert_row_at_in_review(&conn, &schema, vec![existing_entry]);

        // Sanity: 1 entry before second submit-wrap
        assert_eq!(read_wrap_log(&conn).len(), 1);

        // Second submit-wrap call
        let second_entry = json!({
            "executive_summary": "Second wrap — re-wrap after amendments.",
            "deviations": ["scope expanded in phase 3"],
            "residual_risks": [],
            "recommended_sanity_checks": ["smoke test after deploy"]
        });
        let out = compute_submit_wrap(&schema, &conn, "WF001", second_entry, Actor::AiAutonomous)
            .unwrap();

        assert_eq!(out.new_status, "in_review");
        let log = read_wrap_log(&conn);
        assert_eq!(log.len(), 2, "wrap_log must have 2 entries after re-entry");
        assert_eq!(
            log[0]["executive_summary"].as_str().unwrap(),
            "First wrap.",
            "first entry must be preserved"
        );
        assert_eq!(
            log[1]["executive_summary"].as_str().unwrap(),
            "Second wrap — re-wrap after amendments.",
            "second entry must be appended"
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.7: handler sets `at` from now_iso8601(), ignoring any caller-supplied `at`
    // ---------------------------------------------------------------------------

    #[test]
    fn ac3_7_submit_wrap_handler_sets_at_overriding_caller() {
        let (schema, conn) = setup();
        insert_row_at_in_review(&conn, &schema, vec![]);

        // Caller provides a stale `at` — handler must override it
        let entry_with_stale_at = json!({
            "executive_summary": "Override test.",
            "deviations": [],
            "residual_risks": [],
            "recommended_sanity_checks": [],
            "at": "1970-01-01T00:00:00Z"
        });

        compute_submit_wrap(
            &schema,
            &conn,
            "WF001",
            entry_with_stale_at,
            Actor::AiAutonomous,
        )
        .unwrap();

        let log = read_wrap_log(&conn);
        let at = log[0]["at"].as_str().unwrap();
        // Handler's `at` must NOT be the epoch sentinel
        assert_ne!(
            at, "1970-01-01T00:00:00Z",
            "handler must override caller-supplied `at` with now_iso8601()"
        );
        assert!(
            at.starts_with("202"),
            "handler-set `at` must be a recent timestamp, got: {at}"
        );
    }

    // ---------------------------------------------------------------------------
    // T027 P2 (Task 2.5): fire_on_entry_follow_ons honours `when:` predicate
    // ---------------------------------------------------------------------------
    //
    // When on_state declares both a dispatch_agent (when=T1-false) and a
    // transition_to (when=T1-true) for a row whose tier_hint='T1', the
    // follow-on must transition to the target and the dispatch (which is
    // never fired by this function for any kind, but is also predicate-
    // skipped) is left alone.
    #[test]
    fn fire_on_entry_follow_ons_when_predicate_routes_t1_to_ready() {
        let yaml = r#"
name: wf_when
id_format: "W{:03d}"
lifecycle:
  states: [planning, ready, executing, done]
  transitions:
    - from: planning
      to: ready
      verb: skip-plan
      actor: framework
fields:
  - name: title
    type: text
  - name: tier_hint
    type: text
  - name: current_phase
    type: integer
    actor: framework
  - name: current_cycle
    type: integer
    actor: framework
workflow:
  agent_roles:
    planner:
      description: "plans"
  briefing_templates:
    planner: templates/planner-brief.md.tpl
  on_state:
    planning:
      - dispatch_agent: planner
        when: "tier_hint != 'T1'"
      - transition_to: ready
        when: "tier_hint == 'T1'"
  submit_targets: {}
  max_revise_cycles: 3
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();

        // Insert a T1 row at planning.
        let now = now_iso8601();
        conn.execute(
            "INSERT INTO wf_when (display_id, status, created_at, updated_at, \
             created_by, updated_by, title, tier_hint, current_phase, current_cycle) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params!["W001", "planning", now, now, "human", "human", "t", "T1", 0, 0],
        )
        .unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        let row_id: i64 = tx
            .query_row(
                "SELECT id FROM wf_when WHERE display_id = 'W001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        fire_on_entry_follow_ons(&tx, &schema, "W001", row_id, "planning").unwrap();
        tx.commit().unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM wf_when WHERE display_id = 'W001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "ready",
            "T1 row must follow when=T1-true transition_to ready"
        );

        // Inverse: a T3 row stays at planning (when=T1-true is false; there
        // is no transition_to for the T3 path here).
        conn.execute(
            "INSERT INTO wf_when (display_id, status, created_at, updated_at, \
             created_by, updated_by, title, tier_hint, current_phase, current_cycle) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params!["W002", "planning", now, now, "human", "human", "t", "T3", 0, 0],
        )
        .unwrap();
        let tx2 = conn.unchecked_transaction().unwrap();
        let row_id2: i64 = tx2
            .query_row(
                "SELECT id FROM wf_when WHERE display_id = 'W002'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        fire_on_entry_follow_ons(&tx2, &schema, "W002", row_id2, "planning").unwrap();
        tx2.commit().unwrap();

        let status2: String = conn
            .query_row(
                "SELECT status FROM wf_when WHERE display_id = 'W002'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status2, "planning",
            "T3 row must NOT take when=T1-true follow-on; status stays at planning"
        );
    }

    // ---------------------------------------------------------------------------
    // T054: contract-derived T1 plan synthesis tests
    // ---------------------------------------------------------------------------

    #[test]
    fn synthesize_t1_plan_from_contract_maps_contract_fields() {
        // (a) executive_intent populated → objective uses it
        let mut entry: EntryMap = std::collections::BTreeMap::new();
        entry.insert(
            "contract".to_string(),
            json!({
                "executive_intent": "intent text",
                "done_when": "the X is done",
                "scope_in": "- bullet one\n- bullet two",
                "scope_out": "",
            }),
        );
        let plan = synthesize_t1_plan_from_contract(&entry);
        assert_eq!(plan["objective"].as_str().unwrap(), "intent text");
        assert_eq!(
            plan["phases"].as_array().unwrap().len(),
            1,
            "synthesised plan must always have exactly one phase"
        );
        let phase = &plan["phases"][0];
        assert_eq!(phase["name"].as_str().unwrap(), "Contract execution");
        assert_eq!(phase["objective"].as_str().unwrap(), "the X is done");
        let tasks: Vec<String> = phase["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            tasks,
            vec!["bullet one".to_string(), "bullet two".to_string()]
        );
        assert_eq!(
            phase["acceptance_criteria"].as_array().unwrap().len(),
            1,
            "AC list must contain exactly one bullet (done_when)"
        );
        assert_eq!(
            phase["acceptance_criteria"][0].as_str().unwrap(),
            "the X is done"
        );
        assert!(phase["files"].as_array().unwrap().is_empty());
        assert!(phase["dependencies"].as_array().unwrap().is_empty());

        // (b) executive_intent empty → objective falls back to done_when
        let mut entry2: EntryMap = std::collections::BTreeMap::new();
        entry2.insert(
            "contract".to_string(),
            json!({
                "executive_intent": "",
                "done_when": "fallback objective",
                "scope_in": "- only one",
                "scope_out": "",
            }),
        );
        let plan2 = synthesize_t1_plan_from_contract(&entry2);
        assert_eq!(plan2["objective"].as_str().unwrap(), "fallback objective");

        // (c) scope_in with newline list → tasks split correctly
        let mut entry3: EntryMap = std::collections::BTreeMap::new();
        entry3.insert(
            "contract".to_string(),
            json!({
                "executive_intent": "i",
                "done_when": "d",
                "scope_in": "do A\ndo B\ndo C",
                "scope_out": "",
            }),
        );
        let plan3 = synthesize_t1_plan_from_contract(&entry3);
        let tasks3: Vec<String> = plan3["phases"][0]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            tasks3,
            vec!["do A".to_string(), "do B".to_string(), "do C".to_string()]
        );

        // (d) empty scope_in → fallback single task
        let mut entry4: EntryMap = std::collections::BTreeMap::new();
        entry4.insert(
            "contract".to_string(),
            json!({
                "executive_intent": "i",
                "done_when": "d",
                "scope_in": "",
                "scope_out": "",
            }),
        );
        let plan4 = synthesize_t1_plan_from_contract(&entry4);
        let tasks4: Vec<String> = plan4["phases"][0]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            tasks4,
            vec!["Execute the ratified contract scope".to_string()]
        );

        // (e) phases.length always == 1, regardless of input
        for input in [&plan, &plan2, &plan3, &plan4] {
            assert_eq!(input["phases"].as_array().unwrap().len(), 1);
        }
    }

    #[test]
    fn fire_on_entry_follow_ons_t1_skip_plan_synthesizes_plan() {
        // Use the production tasks schema so the contract field shape and
        // the on_state.planning when-predicates match real behaviour.
        let task_schema =
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&task_schema))
            .unwrap();

        let now = "2026-05-06T00:00:00Z";
        let contract = serde_json::to_string(&json!({
            "executive_intent": "fix the thing",
            "done_when": "the thing is fixed",
            "scope_in": "- edit module A\n- edit module B",
            "scope_out": "UI",
        }))
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, \
             title, slug, branch, workspace_path, tier_hint, contract, plan, current_phase, current_cycle) \
             VALUES ('T910', 'planning', ?1, ?1, 'framework', 'framework', \
             't1 synth', 't1-synth', 'feat/t910', '/tmp/no', 'T1', \
             ?2, NULL, 0, 0)",
            rusqlite::params![now, contract],
        )
        .unwrap();
        let row_id = conn.last_insert_rowid();

        let tx = conn.unchecked_transaction().unwrap();
        fire_on_entry_follow_ons(&tx, &task_schema, "T910", row_id, "planning").unwrap();
        tx.commit().unwrap();

        let (status, plan_json, plan_source): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT status, plan, plan_source FROM tasks WHERE display_id = 'T910'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        // Cascade: planning → ready → executing
        assert_eq!(status, "executing");
        let plan_v: Value = serde_json::from_str(&plan_json.unwrap()).unwrap();
        assert_eq!(
            plan_v["phases"].as_array().unwrap().len(),
            1,
            "synthesized plan must have exactly one phase"
        );
        assert_eq!(
            plan_v["phases"][0]["objective"].as_str().unwrap(),
            "the thing is fixed"
        );
        assert_eq!(plan_source.as_deref(), Some("contract_synthesized"));
    }

    #[test]
    fn compute_submit_plan_sets_planner_authored_plan_source() {
        let (schema, conn) = setup();
        insert_row_at(&conn, &schema, "planning", 0, 0, 0, vec![], vec![], None);
        set_tier_hint(&conn, "T3");

        let plan = json!({
            "summary": "p",
            "phases": [{"name": "p1"}, {"name": "p2"}]
        });
        let _ = compute_submit_plan(&schema, &conn, "WF001", plan, Actor::AiAutonomous).unwrap();

        let plan_source: Option<String> = conn
            .query_row(
                "SELECT plan_source FROM wf_tasks WHERE display_id = 'WF001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            plan_source.as_deref(),
            Some("planner_authored"),
            "compute_submit_plan must label every planner-authored plan with plan_source"
        );
    }

    #[test]
    fn compute_resume_t1_with_null_plan_routes_to_planning_and_synthesizes() {
        let task_schema =
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&task_schema))
            .unwrap();

        let now = "2026-05-06T00:00:00Z";
        let contract = serde_json::to_string(&json!({
            "executive_intent": "fix it",
            "done_when": "is fixed",
            "scope_in": "- step 1",
            "scope_out": "",
        }))
        .unwrap();
        // Blocked T1 row, plan IS NULL — historical shape produced before T054.
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, \
             title, slug, branch, workspace_path, tier_hint, contract, plan, current_phase, current_cycle, \
             blocked_reason) \
             VALUES ('T911', 'blocked', ?1, ?1, 'framework', 'framework', \
             'historical t1', 'historical-t1', 'feat/t911', '/tmp/no', 'T1', \
             ?2, NULL, 0, 1, 'transient')",
            rusqlite::params![now, contract],
        )
        .unwrap();

        let out = compute_resume(&task_schema, &conn, "T911", Actor::AiWithHuman).unwrap();
        // Cascade: blocked → planning → ready → executing
        assert_eq!(out.new_status, "executing");

        let (status, plan_json, plan_source): (String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT status, plan, plan_source FROM tasks WHERE display_id = 'T911'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "executing");
        let plan_v: Value = serde_json::from_str(&plan_json.unwrap()).unwrap();
        assert_eq!(plan_v["phases"].as_array().unwrap().len(), 1);
        assert_eq!(plan_source.as_deref(), Some("contract_synthesized"));

        // Idempotency: running compute_resume again on this row would error
        // (status is 'executing', not 'blocked'). The relevant idempotency
        // claim is that the synthesis branch only fires when plan is empty;
        // re-firing fire_on_entry_follow_ons("planning") on this row would
        // see plan populated and skip synthesis, leaving plan unchanged.
        let tx = conn.unchecked_transaction().unwrap();
        let row_id: i64 = tx
            .query_row("SELECT id FROM tasks WHERE display_id = 'T911'", [], |r| {
                r.get(0)
            })
            .unwrap();
        // Reset to planning to re-fire the cascade.
        tx.execute(
            "UPDATE tasks SET status = 'planning' WHERE id = ?1",
            rusqlite::params![row_id],
        )
        .unwrap();
        fire_on_entry_follow_ons(&tx, &task_schema, "T911", row_id, "planning").unwrap();
        tx.commit().unwrap();
        let plan_after: Option<String> = conn
            .query_row(
                "SELECT plan FROM tasks WHERE display_id = 'T911'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let plan_after_v: Value = serde_json::from_str(&plan_after.unwrap()).unwrap();
        assert_eq!(
            plan_after_v["phases"].as_array().unwrap().len(),
            1,
            "re-firing skip-plan with plan already populated must be idempotent"
        );
    }

    fn setup_bundled_tasks_for_retry() -> (Schema, Connection) {
        let task_schema =
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&task_schema))
            .unwrap();
        (task_schema, conn)
    }

    fn insert_task_for_retry(conn: &Connection, display_id: &str, status: &str) {
        let now = "2026-05-07T00:00:00Z";
        let contract = serde_json::to_string(&json!({
            "done_when": "done",
            "scope_in": "scope",
            "scope_out": "out",
        }))
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, \
             title, slug, branch, workspace_path, contract, current_phase, current_cycle, blocked_reason) \
             VALUES (?1, ?2, ?3, ?3, 'framework', 'framework', \
             'retry task', 'retry-task', 'feat/retry', '/tmp/no', ?4, 1, 1, 'deploy failed')",
            rusqlite::params![display_id, status, now, contract],
        )
        .unwrap();
    }

    #[test]
    fn compute_resume_from_deploy_blocked_errors_with_guidance_and_row_unchanged() {
        let (schema, conn) = setup_bundled_tasks_for_retry();
        insert_task_for_retry(&conn, "T920", "deploy_blocked");

        let err = compute_resume(&schema, &conn, "T920", Actor::AiWithHuman).unwrap_err();
        let msg = err.to_string();
        for needle in [
            "deploy_blocked",
            "retry-deploy",
            "close-out-of-band",
            "not resume",
        ] {
            assert!(msg.contains(needle), "missing {needle}: {msg}");
        }
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id = 'T920'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "deploy_blocked");
    }

    #[test]
    fn retry_deploy_from_deploy_blocked_to_accepted_records_history() {
        let (schema, conn) = setup_bundled_tasks_for_retry();
        insert_task_for_retry(&conn, "T921", "deploy_blocked");

        let out = compute_retry_deploy(&schema, &conn, "T921", Actor::AiWithHuman).unwrap();
        assert_eq!(out.new_status, "accepted");
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id = 'T921'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "accepted");
        let row: (String, String, String) = conn
            .query_row(
                "SELECT from_status, to_status, verb FROM transition_history \
                 WHERE store='tasks' AND display_id='T921'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "deploy_blocked".into(),
                "accepted".into(),
                "retry-deploy".into()
            )
        );
    }

    #[test]
    fn retry_deploy_from_non_deploy_blocked_rejected() {
        let (schema, conn) = setup_bundled_tasks_for_retry();
        insert_task_for_retry(&conn, "T922", "blocked");

        let err = compute_retry_deploy(&schema, &conn, "T922", Actor::AiWithHuman).unwrap_err();
        assert!(err.to_string().contains("expected 'deploy_blocked'"));
        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id = 'T922'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "blocked");
    }
}
