use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::{Connection, Transaction};
use serde_json::Value;

use crate::codegen::ddl::quote_ident;
use crate::schema::{
    actor::{Actor, InvokerCtx},
    lifecycle::select_transition,
    FieldType, Schema,
};
use crate::validate::{self, Op};

use super::row::{build_entry_map, now_iso8601, read_row};

/// Entry point for direct CLI use: opens its own transaction and delegates to `run_in_tx`.
pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
    verb: &str,
) -> Result<()> {
    let tx = conn
        .unchecked_transaction()
        .context("transition: begin tx")?;
    run_in_tx(&tx, schema, matches, invoker, verb)?;
    tx.commit().context("transition: commit tx")?;
    Ok(())
}

/// Entry point for `reject` — performs the status transition AND writes reject_reason
/// to wrap_log[-1].reject_reason, atomically in one transaction.
pub fn run_reject(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
    reason: &str,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let tx = conn.unchecked_transaction().context("reject: begin tx")?;

    // Read wrap_log BEFORE the transition (status is still in_review here).
    let (row_id, existing) = read_row(schema, &tx, display_id)?;

    let wrap_field = schema
        .workflow
        .as_ref()
        .and_then(|w| w.submit_targets.get("submit-wrap"))
        .map(|s| s.as_str())
        .unwrap_or("wrap_log");

    let mut wrap_list: Vec<Value> = existing
        .get(wrap_field)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Mutate wrap_log[-1].reject_reason (option b: update latest entry in-place).
    if let Some(last) = wrap_list.last_mut() {
        if let Value::Object(ref mut obj) = last {
            obj.insert(
                "reject_reason".to_string(),
                Value::String(reason.to_string()),
            );
        }
    } else {
        // wrap agent hasn't run yet — append a stub entry so the reason is never lost.
        let mut stub = serde_json::Map::new();
        stub.insert(
            "reject_reason".to_string(),
            Value::String(reason.to_string()),
        );
        stub.insert("at".to_string(), Value::String(now_iso8601()));
        wrap_list.push(Value::Object(stub));
    }

    // Fire the reject transition (status: in_review → rejected).
    run_in_tx(&tx, schema, matches, invoker, "reject")?;

    // Write updated wrap_log.
    let wrap_json = serde_json::to_string(&wrap_list)?;
    let qtable = quote_ident(&schema.name);
    tx.execute(
        &format!("UPDATE {qtable} SET {wrap_field} = ?1 WHERE id = ?2"),
        rusqlite::params![wrap_json, row_id],
    )
    .context("reject: write wrap_log reject_reason")?;

    tx.commit().context("reject: commit tx")?;
    Ok(())
}

/// Entry point for `close_as_addressed` (observations, open → resolved):
/// validates --resolution shape (T###, L###, or 7-40 hex commit sha), writes
/// `resolution` and `resolved_at` into the diff, and runs the lifecycle
/// transition atomically in one tx.
pub fn run_close_as_addressed(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
    resolution: &str,
) -> Result<()> {
    let re = regex::Regex::new(r"^(T\d{3,}|L\d{3,}|[0-9a-f]{7,40})$").unwrap();
    if !re.is_match(resolution) {
        anyhow::bail!(
            "--resolution '{resolution}' does not match an accepted reference form. \
             Accepted: task-id (T### / T0123), observation-id (L### / L0042), \
             or commit-sha (7-40 lowercase hex chars)."
        );
    }

    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let tx = conn
        .unchecked_transaction()
        .context("close_as_addressed: begin tx")?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Build diff with resolution + resolved_at injected.
    let now = now_iso8601();
    let mut diff = build_entry_map(schema, |cli_name| {
        match matches.try_get_many::<String>(cli_name) {
            Ok(Some(vals)) => {
                let collected: Vec<String> = vals.cloned().collect();
                if collected.is_empty() {
                    None
                } else {
                    Some(collected)
                }
            }
            _ => None,
        }
    })?;
    diff.insert(
        "resolution".to_string(),
        Value::String(resolution.to_string()),
    );
    diff.insert("resolved_at".to_string(), Value::String(now.clone()));

    let mut merged = existing.clone();
    for (k, v) in &diff {
        merged.insert(k.clone(), v.clone());
    }

    let transition = select_transition(
        &schema.lifecycle.transitions,
        current_status,
        "close_as_addressed",
        None,
        &merged,
    )?;

    validate::validate(
        schema,
        &merged,
        Op::Transition("close_as_addressed".to_string(), diff.clone()),
        invoker,
    )
    .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    execute_transition_write(
        &tx,
        schema,
        row_id,
        &transition.to,
        &diff,
        &merged,
        invoker.actor,
    )?;

    tx.commit().context("close_as_addressed: commit tx")?;

    println!(
        "Transitioned {display_id}: {} → {} (resolution={resolution})",
        transition.from, transition.to
    );
    Ok(())
}

/// Transaction-agnostic core.  All DB access uses `tx` (which is `Deref<Target=Connection>`).
/// Called by `run` (single-call CLI path) and by submit handlers that pass their own `tx`
/// for atomic multi-step operations (Phase 5 / task 5.7).
pub(crate) fn run_in_tx(
    tx: &Transaction,
    schema: &Schema,
    matches: &ArgMatches,
    invoker: InvokerCtx,
    verb: &str,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    // Read existing row (inside tx)
    let (row_id, existing) = read_row(schema, tx, display_id)?;

    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Build diff entry from CLI args
    let mut diff = build_entry_map(schema, |cli_name| {
        let from_file_key = format!("{cli_name}-from-file");
        if matches.try_contains_id(&from_file_key).unwrap_or(false) {
            if let Some(path) = matches.get_one::<String>(&from_file_key) {
                if path == "-" {
                    use std::io::Read;
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s).ok();
                    return Some(vec![s.trim_end_matches('\n').to_string()]);
                }
                return std::fs::read_to_string(path)
                    .ok()
                    .map(|s| vec![s.trim_end_matches('\n').to_string()]);
            }
        }
        match matches.try_get_many::<String>(cli_name) {
            Ok(Some(vals)) => {
                let collected: Vec<String> = vals.cloned().collect();
                if collected.is_empty() {
                    None
                } else {
                    Some(collected)
                }
            }
            _ => None,
        }
    })?;

    // Deep-merge diff into existing; Record-typed fields get sub-field-level merge
    let mut merged = existing.clone();
    for (k, v) in &diff {
        let is_record = schema
            .fields
            .iter()
            .any(|f| f.name == *k && matches!(f.ty, crate::schema::FieldType::Record(_)));
        if is_record {
            if let (Some(Value::Object(existing_obj)), Value::Object(new_obj)) =
                (merged.get(k).cloned(), v)
            {
                let mut combined = existing_obj.clone();
                for (sk, sv) in new_obj {
                    combined.insert(sk.clone(), sv.clone());
                }
                merged.insert(k.clone(), Value::Object(combined));
                continue;
            }
        }
        merged.insert(k.clone(), v.clone());
    }

    // Resolve the transition using the full selection algorithm (guard-aware).
    // Must run AFTER building merged so guards are evaluated against the post-diff entry.
    // Plain transitions never carry a requires_gate, so gate=None is correct.
    let transition = select_transition(
        &schema.lifecycle.transitions,
        current_status,
        verb,
        None,
        &merged,
    )?;

    // Run validator against merged entry; actor checks scoped to diff only.
    validate::validate(
        schema,
        &merged,
        Op::Transition(verb.to_string(), diff.clone()),
        invoker,
    )
    .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    // F2: `amend` (rejected → planning) resets current_phase/current_cycle to 0.
    // Decision Matrix row (i): "resets the row to phase 0".
    if verb == "amend" {
        merged.insert("current_phase".to_string(), Value::Number(0.into()));
        merged.insert("current_cycle".to_string(), Value::Number(0.into()));
        diff.insert("current_phase".to_string(), Value::Number(0.into()));
        diff.insert("current_cycle".to_string(), Value::Number(0.into()));
    }

    // Write: UPDATE merged fields + status = transition.to + updated_*
    execute_transition_write(
        tx,
        schema,
        row_id,
        &transition.to,
        &diff,
        &merged,
        invoker.actor,
    )?;

    println!(
        "Transitioned {display_id}: {} → {}",
        transition.from, transition.to
    );
    Ok(())
}

/// Write the transition state change into the DB (inside a caller-supplied transaction).
/// Used by both `run_in_tx` (CLI path) and submit handlers (engine path).
pub(crate) fn execute_transition_write(
    tx: &Transaction,
    schema: &Schema,
    row_id: i64,
    new_status: &str,
    diff: &crate::validate::EntryMap,
    merged: &crate::validate::EntryMap,
    invoker: Actor,
) -> Result<()> {
    let now = now_iso8601();
    let invoker_str = invoker.to_string();

    let mut set_parts: Vec<String> = vec![
        "updated_at = ?1".to_string(),
        "updated_by = ?2".to_string(),
        "status = ?3".to_string(),
    ];
    let mut sql_values: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(now),
        rusqlite::types::Value::Text(invoker_str),
        rusqlite::types::Value::Text(new_status.to_string()),
    ];
    let mut param_idx = 4usize;

    // Write every field that appeared in the diff (use merged value for Records)
    for field in &schema.fields {
        if let Some(new_val) = diff.get(&field.name) {
            set_parts.push(format!("{} = ?{param_idx}", field.name));
            param_idx += 1;

            match &field.ty {
                FieldType::Record(_) => {
                    let write_val = merged.get(&field.name).unwrap_or(new_val);
                    let json_str =
                        serde_json::to_string(write_val).unwrap_or_else(|_| "null".to_string());
                    sql_values.push(rusqlite::types::Value::Text(json_str));
                }
                FieldType::List(_)
                | FieldType::ListRecord(_)
                | FieldType::ListFk { .. }
                | FieldType::Json => {
                    let json_str =
                        serde_json::to_string(new_val).unwrap_or_else(|_| "null".to_string());
                    sql_values.push(rusqlite::types::Value::Text(json_str));
                }
                FieldType::Bool => {
                    let i = match new_val {
                        Value::Bool(b) => {
                            if *b {
                                1
                            } else {
                                0
                            }
                        }
                        Value::Number(n) => n.as_i64().unwrap_or(0) as i32 as i64,
                        _ => 0,
                    };
                    sql_values.push(rusqlite::types::Value::Integer(i));
                }
                FieldType::Integer => {
                    let i = match new_val {
                        Value::Number(n) => n.as_i64().unwrap_or(0),
                        _ => 0,
                    };
                    sql_values.push(rusqlite::types::Value::Integer(i));
                }
                _ => {
                    let s = match new_val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    sql_values.push(rusqlite::types::Value::Text(s));
                }
            }
        }
    }

    let where_param_idx = param_idx;
    sql_values.push(rusqlite::types::Value::Integer(row_id));

    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE {} SET {set_clause} WHERE id = ?{where_param_idx}",
        quote_ident(&schema.name)
    );

    tx.execute(&sql, rusqlite::params_from_iter(sql_values.iter()))
        .context("transition update row")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::schema::Schema;

    const OBS_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
default_actor: ai_with_human
lifecycle:
  states: [open, triaged, resolved, wont_fix]
  transitions:
    - from: open
      to: triaged
      verb: triage
      actor: ai_with_human
    - from: triaged
      to: resolved
      verb: resolve
      actor: ai_autonomous
    - from: triaged
      to: wont_fix
      verb: wont_fix
      actor: ai_with_human
    - from: open
      to: resolved
      verb: close_as_addressed
      actor: ai_autonomous
fields:
  - name: summary
    type: text
    required: true
  - name: triage
    type: record
    fields:
      - name: verdict
        type: enum
        enum_values: [T1, T2, T3]
      - name: notes
        type: text
        required: false
  - name: contract
    type: record
    fields:
      - name: done_when
        type: text
        required_when: "triage.verdict == 'T3'"
      - name: scope_in
        type: text
        required_when: "triage.verdict == 'T3'"
      - name: scope_out
        type: text
        required_when: "triage.verdict == 'T3'"
  - name: tags
    type:
      list: text
    required: false
  - name: resolution
    type: text
    required: false
    actor: ai_autonomous
  - name: resolved_at
    type: text
    required: false
    actor: ai_autonomous
"#;

    fn build_cmd(schema: &Schema, verb: &'static str) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd =
            clap::Command::new(verb).arg(clap::Arg::new("display_id").required(true).index(1));
        for leaf in &leaves {
            cmd = cmd.arg(
                clap::Arg::new(leaf.cli_name.clone())
                    .long(leaf.cli_name.clone())
                    .required(false),
            );
        }
        cmd
    }

    fn setup() -> (Schema, Connection) {
        let schema = Schema::from_yaml(OBS_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    fn insert_open_row(schema: &Schema, conn: &Connection) {
        let add_cmd = {
            let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
            let mut cmd = clap::Command::new("add");
            for leaf in &leaves {
                cmd = cmd.arg(
                    clap::Arg::new(leaf.cli_name.clone())
                        .long(leaf.cli_name.clone())
                        .required(false),
                );
            }
            cmd
        };
        let add_matches = add_cmd.get_matches_from(["add", "--summary", "test observation"]);
        crate::handlers::add::run(schema, conn, &add_matches, Actor::Human.into()).unwrap();
    }

    #[test]
    fn triage_t3_without_contract_fails() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from(["triage", "L001", "--verdict", "T3"]);
        let err = run(&schema, &conn, &matches, Actor::Human.into(), "triage").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("done_when") || msg.contains("validation failed"),
            "expected contract error: {msg}"
        );
    }

    #[test]
    fn triage_t3_with_contract_succeeds() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from([
            "triage",
            "L001",
            "--verdict",
            "T3",
            "--done-when",
            "X works",
            "--scope-in",
            "backend",
            "--scope-out",
            "frontend",
        ]);
        run(&schema, &conn, &matches, Actor::Human.into(), "triage").unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "triaged");
    }

    #[test]
    fn state_machine_rejects_wrong_from_state() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        // First triage succeeds
        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from(["triage", "L001", "--verdict", "T1"]);
        run(&schema, &conn, &matches, Actor::Human.into(), "triage").unwrap();

        // Second triage is rejected (state-machine legality now enforced by select_transition)
        let cmd2 = build_cmd(&schema, "triage");
        let matches2 = cmd2.get_matches_from(["triage", "L001", "--verdict", "T1"]);
        let err = run(&schema, &conn, &matches2, Actor::Human.into(), "triage").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("triage"),
            "expected state-machine error mentioning verb; got: {msg}"
        );
        assert!(
            msg.contains("triaged") || msg.contains("no transition"),
            "error should indicate state mismatch: {msg}"
        );
    }

    #[test]
    fn transition_actor_rejects_ai_autonomous_when_required_ai_with_human() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from(["triage", "L001", "--verdict", "T1"]);
        let err = run(
            &schema,
            &conn,
            &matches,
            Actor::AiAutonomous.into(),
            "triage",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("ai_with_human"),
            "expected error citing actor 'ai_with_human'; got: {msg}"
        );
        assert!(
            msg.contains("ai_autonomous"),
            "expected error citing invoker 'ai_autonomous'; got: {msg}"
        );
    }

    #[test]
    fn transition_actor_accepts_human_for_ai_with_human_transition() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from(["triage", "L001", "--verdict", "T1"]);
        run(&schema, &conn, &matches, Actor::Human.into(), "triage").unwrap();
    }

    #[test]
    fn transition_actor_accepts_ai_autonomous_for_ai_autonomous_transition() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        // First triage with Human (ai_with_human transition)
        let triage_cmd = build_cmd(&schema, "triage");
        let triage_matches = triage_cmd.get_matches_from(["triage", "L001", "--verdict", "T1"]);
        run(
            &schema,
            &conn,
            &triage_matches,
            Actor::Human.into(),
            "triage",
        )
        .unwrap();

        // resolve is ai_autonomous; invoker AiAutonomous should succeed
        let resolve_cmd = build_cmd(&schema, "resolve");
        let resolve_matches = resolve_cmd.get_matches_from(["resolve", "L001"]);
        run(
            &schema,
            &conn,
            &resolve_matches,
            Actor::AiAutonomous.into(),
            "resolve",
        )
        .unwrap();
    }

    #[test]
    fn resolve_transition_from_triaged_succeeds() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        // Triage first
        let triage_cmd = build_cmd(&schema, "triage");
        let triage_matches = triage_cmd.get_matches_from(["triage", "L001", "--verdict", "T1"]);
        run(
            &schema,
            &conn,
            &triage_matches,
            Actor::Human.into(),
            "triage",
        )
        .unwrap();

        // Resolve (actor: ai_autonomous)
        let resolve_cmd = build_cmd(&schema, "resolve");
        let resolve_matches = resolve_cmd.get_matches_from(["resolve", "L001"]);
        run(
            &schema,
            &conn,
            &resolve_matches,
            Actor::AiAutonomous.into(),
            "resolve",
        )
        .unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "resolved");
    }

    // Test that run_in_tx works with an explicit transaction (AC5.7 / C3 pattern)
    #[test]
    fn run_in_tx_uses_caller_transaction() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from(["triage", "L001", "--verdict", "T1"]);

        // Use run_in_tx directly with caller-owned transaction
        let tx = conn.unchecked_transaction().unwrap();
        run_in_tx(&tx, &schema, &matches, Actor::Human.into(), "triage").unwrap();
        // Before commit, status in tx is triaged; outside tx (other connection) still sees open
        // Commit
        tx.commit().unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "triaged");
    }

    // Test that rolled-back transaction leaves row unchanged (AC5.11 pattern)
    #[test]
    fn rolled_back_transaction_leaves_row_unchanged() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from(["triage", "L001", "--verdict", "T1"]);

        {
            let tx = conn.unchecked_transaction().unwrap();
            run_in_tx(&tx, &schema, &matches, Actor::Human.into(), "triage").unwrap();
            // tx drops without commit → rollback
        }

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "open", "rollback must restore original status");
    }

    // ---- Phase 1 (T006) regression-trap: guard evaluation in plain transitions ----

    /// Schema with two same-verb transitions partitioned by guard.
    /// Pre-fix: the bare `.find(|t| t.verb == verb)` always picked the first (T2) regardless.
    /// Post-fix: `select_transition` evaluates guards and picks the correct one.
    const GUARDED_PARTITIONED_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
default_actor: ai_autonomous
lifecycle:
  states: [confirmed, in_progress_t2, in_progress_t3]
  transitions:
    - from: confirmed
      to: in_progress_t2
      verb: ratify
      guard: "tier_hint == 'T2'"
      actor: ai_autonomous
    - from: confirmed
      to: in_progress_t3
      verb: ratify
      guard: "tier_hint == 'T3'"
      actor: ai_autonomous
fields:
  - name: summary
    type: text
    required: true
  - name: tier_hint
    type: text
    required: false
"#;

    fn setup_guarded() -> (Schema, Connection) {
        let schema = Schema::from_yaml(GUARDED_PARTITIONED_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    /// Insert a row directly at 'confirmed' with a given tier_hint.
    fn insert_confirmed_row(_schema: &Schema, conn: &Connection, tier: &str) {
        conn.execute(
            "INSERT INTO observations (display_id, status, summary, tier_hint) VALUES (?1, 'confirmed', 'test', ?2)",
            rusqlite::params![format!("L{:03}", 1), tier],
        ).unwrap();
    }

    #[test]
    fn guard_partitioned_picks_t3_transition_for_t3_row() {
        let (schema, conn) = setup_guarded();
        insert_confirmed_row(&schema, &conn, "T3");

        let cmd = build_cmd(&schema, "ratify");
        let matches = cmd.get_matches_from(["ratify", "L001"]);
        run(
            &schema,
            &conn,
            &matches,
            Actor::AiAutonomous.into(),
            "ratify",
        )
        .unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "in_progress_t3",
            "T3 row must land in in_progress_t3, not in_progress_t2"
        );
    }

    #[test]
    fn guard_partitioned_picks_t2_transition_for_t2_row() {
        let (schema, conn) = setup_guarded();
        insert_confirmed_row(&schema, &conn, "T2");

        let cmd = build_cmd(&schema, "ratify");
        let matches = cmd.get_matches_from(["ratify", "L001"]);
        run(
            &schema,
            &conn,
            &matches,
            Actor::AiAutonomous.into(),
            "ratify",
        )
        .unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "in_progress_t2",
            "T2 row must land in in_progress_t2, not in_progress_t3"
        );
    }

    #[test]
    fn guard_partitioned_rejects_when_no_guard_matches() {
        let (schema, conn) = setup_guarded();
        // Insert with tier_hint T1 — neither T2 nor T3 guard fires, no unguarded fallback
        insert_confirmed_row(&schema, &conn, "T1");

        let cmd = build_cmd(&schema, "ratify");
        let matches = cmd.get_matches_from(["ratify", "L001"]);
        let err = run(
            &schema,
            &conn,
            &matches,
            Actor::AiAutonomous.into(),
            "ratify",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("guard not satisfied") || msg.contains("no unguarded fallback"),
            "expected guard-not-satisfied error; got: {msg}"
        );
    }

    #[test]
    fn plain_transition_guard_false_rejected() {
        // Single-transition schema with a guard that is false for the row.
        let schema_yaml = r#"
name: observations
id_format: "L{:03d}"
default_actor: ai_autonomous
lifecycle:
  states: [confirmed, ready]
  transitions:
    - from: confirmed
      to: ready
      verb: ratify
      guard: "tier_hint == 'ready'"
      actor: ai_autonomous
fields:
  - name: summary
    type: text
    required: true
  - name: tier_hint
    type: text
    required: false
"#;
        let schema = Schema::from_yaml(schema_yaml).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();

        conn.execute(
            "INSERT INTO observations (display_id, status, summary, tier_hint) VALUES ('L001', 'confirmed', 'test', 'not_ready')",
            [],
        ).unwrap();

        let cmd = build_cmd(&schema, "ratify");
        let matches = cmd.get_matches_from(["ratify", "L001"]);
        let err = run(
            &schema,
            &conn,
            &matches,
            Actor::AiAutonomous.into(),
            "ratify",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("guard not satisfied") || msg.contains("no unguarded fallback"),
            "expected guard rejection; got: {msg}"
        );
    }

    #[test]
    fn plain_transition_guard_true_succeeds() {
        let schema_yaml = r#"
name: observations
id_format: "L{:03d}"
default_actor: ai_autonomous
lifecycle:
  states: [confirmed, ready]
  transitions:
    - from: confirmed
      to: ready
      verb: ratify
      guard: "tier_hint == 'ready'"
      actor: ai_autonomous
fields:
  - name: summary
    type: text
    required: true
  - name: tier_hint
    type: text
    required: false
"#;
        let schema = Schema::from_yaml(schema_yaml).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();

        conn.execute(
            "INSERT INTO observations (display_id, status, summary, tier_hint) VALUES ('L001', 'confirmed', 'test', 'ready')",
            [],
        ).unwrap();

        let cmd = build_cmd(&schema, "ratify");
        let matches = cmd.get_matches_from(["ratify", "L001"]);
        run(
            &schema,
            &conn,
            &matches,
            Actor::AiAutonomous.into(),
            "ratify",
        )
        .unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "ready");
    }

    #[test]
    fn validate_transition_ambiguity_still_rejects_unguarded_same_verb_pairs() {
        // Confirm install-time validator is unaffected by our extraction
        use crate::schema::lifecycle::Lifecycle;
        let yaml = r#"
states: [confirmed, a, b]
transitions:
  - from: confirmed
    to: a
    verb: ratify
  - from: confirmed
    to: b
    verb: ratify
"#;
        let lc: Lifecycle = serde_yaml::from_str(yaml).unwrap();
        let err = lc.validate_transition_ambiguity().unwrap_err();
        assert!(
            err.to_string().contains("ambiguous transition selection"),
            "install-time validator must still fire: {err}"
        );
    }

    // ---- T004 (L017): close_as_addressed (open → resolved) tests ----

    /// Build a clap command mirroring dynamic.rs's close_as_addressed augmentation:
    /// all leaf args + required `--resolution`.
    fn build_close_cmd(schema: &Schema) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd = clap::Command::new("close_as_addressed")
            .arg(clap::Arg::new("display_id").required(true).index(1));
        for leaf in &leaves {
            if leaf.cli_name == "resolution" {
                continue;
            }
            cmd = cmd.arg(
                clap::Arg::new(leaf.cli_name.clone())
                    .long(leaf.cli_name.clone())
                    .required(false),
            );
        }
        cmd = cmd.arg(
            clap::Arg::new("resolution")
                .long("resolution")
                .required(true),
        );
        cmd
    }

    fn read_obs(conn: &Connection) -> (String, Option<String>, Option<String>) {
        conn.query_row(
            "SELECT status, resolution, resolved_at FROM observations WHERE display_id = 'L001'",
            [],
            |r| Ok((r.get(0).unwrap(), r.get(1).ok(), r.get(2).ok())),
        )
        .unwrap()
    }

    #[test]
    fn close_as_addressed_with_task_id_succeeds() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_close_cmd(&schema);
        let matches = cmd.get_matches_from(["close_as_addressed", "L001", "--resolution", "T001"]);
        run_close_as_addressed(&schema, &conn, &matches, Actor::AiAutonomous.into(), "T001")
            .unwrap();

        let (status, resolution, resolved_at) = read_obs(&conn);
        assert_eq!(status, "resolved");
        assert_eq!(resolution.as_deref(), Some("T001"));
        assert!(
            resolved_at
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false),
            "resolved_at must be populated; got: {:?}",
            resolved_at
        );
    }

    #[test]
    fn close_as_addressed_with_commit_sha_succeeds() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let sha = "82501d3abcdef0123456789012345678901234ab"; // 40-char hex
        let cmd = build_close_cmd(&schema);
        let matches = cmd.get_matches_from(["close_as_addressed", "L001", "--resolution", sha]);
        run_close_as_addressed(&schema, &conn, &matches, Actor::AiAutonomous.into(), sha).unwrap();

        let (status, resolution, _resolved_at) = read_obs(&conn);
        assert_eq!(status, "resolved");
        assert_eq!(resolution.as_deref(), Some(sha));
    }

    #[test]
    fn close_as_addressed_without_resolution_rejected() {
        // Reproduces clap-layer rejection: `--resolution` is required=true (mirrors dynamic.rs
        // mut_arg). Parsing a close_as_addressed invocation without --resolution must fail
        // before any handler runs.
        let (schema, _conn) = setup();
        let cmd = build_close_cmd(&schema);
        let err = cmd
            .try_get_matches_from(["close_as_addressed", "L001"])
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--resolution") || msg.contains("required"),
            "expected clap error citing --resolution required; got: {msg}"
        );
    }

    #[test]
    fn close_as_addressed_already_resolved_rejected() {
        // Insert a row directly at status=resolved with a prior resolution.
        let (schema, conn) = setup();
        conn.execute(
            "INSERT INTO observations (display_id, status, summary, resolution, resolved_at) \
             VALUES ('L001', 'resolved', 'already done', 'T999', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let cmd = build_close_cmd(&schema);
        let matches = cmd.get_matches_from(["close_as_addressed", "L001", "--resolution", "T001"]);
        let err =
            run_close_as_addressed(&schema, &conn, &matches, Actor::AiAutonomous.into(), "T001")
                .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("close_as_addressed")
                || msg.contains("no transition")
                || msg.contains("resolved"),
            "expected state-machine error from already-resolved row; got: {msg}"
        );

        // Row unchanged: resolution still T999 (not overwritten by failed call).
        let (status, resolution, _) = read_obs(&conn);
        assert_eq!(status, "resolved");
        assert_eq!(
            resolution.as_deref(),
            Some("T999"),
            "row must be unchanged after rejected transition"
        );
    }

    #[test]
    fn close_as_addressed_with_invalid_format_rejected() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_close_cmd(&schema);
        let matches = cmd.get_matches_from([
            "close_as_addressed",
            "L001",
            "--resolution",
            "not-a-valid-ref",
        ]);
        let err = run_close_as_addressed(
            &schema,
            &conn,
            &matches,
            Actor::AiAutonomous.into(),
            "not-a-valid-ref",
        )
        .unwrap_err();
        let msg = err.to_string();
        // Error must enumerate the three accepted forms.
        assert!(
            msg.contains("task-id"),
            "error should name task-id form; got: {msg}"
        );
        assert!(
            msg.contains("observation-id"),
            "error should name observation-id form; got: {msg}"
        );
        assert!(
            msg.contains("commit-sha"),
            "error should name commit-sha form; got: {msg}"
        );

        // Row unchanged
        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "open",
            "row must remain open after format rejection"
        );
    }

    // ---- Phase 6 (T010): accept / reject / amend transition tests ----

    /// Minimal schema with in_review, accepted, rejected, and the three wrap transitions.
    const WRAP_SCHEMA: &str = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [executing, in_review, accepted, rejected, planning]
  transitions:
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
  - name: current_phase
    type: integer
    required: false
  - name: current_cycle
    type: integer
    required: false
  - name: wrap_log
    type: list_record
    fields:
      - name: executive_summary
        type: text
      - name: reject_reason
        type: text
      - name: at
        type: timestamp
"#;

    fn setup_wrap() -> (Schema, Connection) {
        let schema = Schema::from_yaml(WRAP_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    fn insert_wrap_row(conn: &Connection, status: &str, wrap_log_json: &str) {
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, wrap_log) VALUES (?1, ?2, 'Test', ?3)",
            rusqlite::params!["T001", status, wrap_log_json],
        )
        .unwrap();
    }

    fn insert_wrap_row_with_phase(
        conn: &Connection,
        status: &str,
        wrap_log_json: &str,
        current_phase: i64,
        current_cycle: i64,
    ) {
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, wrap_log, current_phase, current_cycle) VALUES (?1, ?2, 'Test', ?3, ?4, ?5)",
            rusqlite::params!["T001", status, wrap_log_json, current_phase, current_cycle],
        ).unwrap();
    }

    fn build_wrap_cmd(schema: &Schema, verb: &'static str) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd =
            clap::Command::new(verb).arg(clap::Arg::new("display_id").required(true).index(1));
        for leaf in &leaves {
            cmd = cmd.arg(
                clap::Arg::new(leaf.cli_name.clone())
                    .long(leaf.cli_name.clone())
                    .required(false),
            );
        }
        cmd
    }

    fn read_status_wrap(conn: &Connection) -> String {
        conn.query_row(
            "SELECT status FROM tasks WHERE display_id = 'T001'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn read_wrap_log(conn: &Connection) -> Vec<serde_json::Value> {
        let raw: Option<String> = conn
            .query_row(
                "SELECT wrap_log FROM tasks WHERE display_id = 'T001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        raw.and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn read_phase_cycle(conn: &Connection) -> (i64, i64) {
        conn.query_row(
            "SELECT current_phase, current_cycle FROM tasks WHERE display_id = 'T001'",
            [],
            |r| Ok((r.get(0).unwrap_or(0), r.get(1).unwrap_or(0))),
        )
        .unwrap()
    }

    // --- accept ---

    #[test]
    fn ac6_accept_happy_path_in_review_human_lands_accepted() {
        let (schema, conn) = setup_wrap();
        insert_wrap_row(
            &conn,
            "in_review",
            r#"[{"executive_summary":"Done","reject_reason":null,"at":"2026-01-01T00:00:00Z"}]"#,
        );

        let cmd = build_wrap_cmd(&schema, "accept");
        let matches = cmd.get_matches_from(["accept", "T001"]);
        run(&schema, &conn, &matches, Actor::Human.into(), "accept").unwrap();

        assert_eq!(read_status_wrap(&conn), "accepted");
    }

    #[test]
    fn ac6_accept_wrong_state_executing_rejected() {
        let (schema, conn) = setup_wrap();
        insert_wrap_row(&conn, "executing", "[]");

        let cmd = build_wrap_cmd(&schema, "accept");
        let matches = cmd.get_matches_from(["accept", "T001"]);
        let err = run(&schema, &conn, &matches, Actor::Human.into(), "accept").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("accept") || msg.contains("no transition") || msg.contains("executing"),
            "expected state-machine error; got: {msg}"
        );
        // Row unchanged
        assert_eq!(read_status_wrap(&conn), "executing");
    }

    #[test]
    fn ac6_accept_ai_autonomous_invoker_rejected() {
        let (schema, conn) = setup_wrap();
        insert_wrap_row(&conn, "in_review", "[]");

        let cmd = build_wrap_cmd(&schema, "accept");
        let matches = cmd.get_matches_from(["accept", "T001"]);
        let err = run(
            &schema,
            &conn,
            &matches,
            Actor::AiAutonomous.into(),
            "accept",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("transition 'accept'"),
            "expected actor mismatch on accept; got: {msg}"
        );
        assert!(
            msg.contains("requires actor 'human'"),
            "error must cite required actor; got: {msg}"
        );
        assert!(
            msg.contains("ai_autonomous"),
            "error must cite invoker; got: {msg}"
        );
        // Row unchanged
        assert_eq!(read_status_wrap(&conn), "in_review");
    }

    // --- reject ---

    fn build_reject_cmd(schema: &Schema) -> clap::Command {
        // build_wrap_cmd plus the required --reason arg (mirrors dynamic.rs augmentation).
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd =
            clap::Command::new("reject").arg(clap::Arg::new("display_id").required(true).index(1));
        for leaf in &leaves {
            cmd = cmd.arg(
                clap::Arg::new(leaf.cli_name.clone())
                    .long(leaf.cli_name.clone())
                    .required(false),
            );
        }
        cmd = cmd.arg(clap::Arg::new("reason").long("reason").required(true));
        cmd
    }

    /// F1 happy path: reject with --reason writes status=rejected AND reject_reason to wrap_log[-1].
    #[test]
    fn ac6_reject_writes_reason_to_wrap_log() {
        let (schema, conn) = setup_wrap();
        insert_wrap_row(
            &conn,
            "in_review",
            r#"[{"executive_summary":"Done","reject_reason":null,"at":"2026-01-01T00:00:00Z"}]"#,
        );

        let cmd = build_reject_cmd(&schema);
        let matches = cmd.get_matches_from(["reject", "T001", "--reason", "scope was wrong"]);
        let reason = matches.get_one::<String>("reason").unwrap().clone();
        run_reject(&schema, &conn, &matches, Actor::Human.into(), &reason).unwrap();

        assert_eq!(read_status_wrap(&conn), "rejected");
        let wrap_log = read_wrap_log(&conn);
        let last = wrap_log.last().expect("wrap_log must not be empty");
        let got = last
            .get("reject_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(
            got, "scope was wrong",
            "reject_reason must equal supplied --reason"
        );
    }

    /// F1 actor check: AiAutonomous invoker must be rejected at the transition layer.
    #[test]
    fn ac6_reject_ai_autonomous_invoker_rejected() {
        let (schema, conn) = setup_wrap();
        insert_wrap_row(&conn, "in_review", "[]");

        let cmd = build_reject_cmd(&schema);
        let matches = cmd.get_matches_from(["reject", "T001", "--reason", "x"]);
        let reason = matches.get_one::<String>("reason").unwrap().clone();
        let err = run_reject(
            &schema,
            &conn,
            &matches,
            Actor::AiAutonomous.into(),
            &reason,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("transition 'reject'"),
            "expected actor mismatch on reject; got: {msg}"
        );
        assert!(
            msg.contains("requires actor 'human'"),
            "error must cite required actor; got: {msg}"
        );
        // Row unchanged
        assert_eq!(read_status_wrap(&conn), "in_review");
    }

    /// F1: reject on empty wrap_log still persists reason (stub entry appended).
    #[test]
    fn ac6_reject_empty_wrap_log_stubs_entry_with_reason() {
        let (schema, conn) = setup_wrap();
        insert_wrap_row(&conn, "in_review", "[]");

        let cmd = build_reject_cmd(&schema);
        let matches = cmd.get_matches_from(["reject", "T001", "--reason", "no wrap agent run"]);
        let reason = matches.get_one::<String>("reason").unwrap().clone();
        run_reject(&schema, &conn, &matches, Actor::Human.into(), &reason).unwrap();

        assert_eq!(read_status_wrap(&conn), "rejected");
        let wrap_log = read_wrap_log(&conn);
        assert!(!wrap_log.is_empty(), "wrap_log must have a stub entry");
        let last = wrap_log.last().unwrap();
        let got = last
            .get("reject_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert_eq!(got, "no wrap agent run");
    }

    // --- amend ---

    #[test]
    fn ac6_amend_happy_path_rejected_lands_planning() {
        let (schema, conn) = setup_wrap();
        insert_wrap_row(&conn, "rejected", "[]");

        let cmd = build_wrap_cmd(&schema, "amend");
        let matches = cmd.get_matches_from(["amend", "T001"]);
        run(&schema, &conn, &matches, Actor::Human.into(), "amend").unwrap();

        assert_eq!(
            read_status_wrap(&conn),
            "planning",
            "amend must land at planning (Decision Matrix row i)"
        );
    }

    /// F2: amend resets current_phase and current_cycle to 0 (Decision Matrix row i).
    #[test]
    fn ac6_amend_resets_phase_and_cycle() {
        let (schema, conn) = setup_wrap();
        insert_wrap_row_with_phase(&conn, "rejected", "[]", 2, 3);

        // Verify seed values
        let (phase_before, cycle_before) = read_phase_cycle(&conn);
        assert_eq!(phase_before, 2);
        assert_eq!(cycle_before, 3);

        let cmd = build_wrap_cmd(&schema, "amend");
        let matches = cmd.get_matches_from(["amend", "T001"]);
        run(&schema, &conn, &matches, Actor::Human.into(), "amend").unwrap();

        assert_eq!(
            read_status_wrap(&conn),
            "planning",
            "amend must land at planning"
        );
        let (phase_after, cycle_after) = read_phase_cycle(&conn);
        assert_eq!(phase_after, 0, "amend must reset current_phase to 0");
        assert_eq!(cycle_after, 0, "amend must reset current_cycle to 0");
    }

    #[test]
    fn ac6_amend_from_wrong_state_accepted_rejected() {
        let (schema, conn) = setup_wrap();
        insert_wrap_row(&conn, "accepted", "[]");

        let cmd = build_wrap_cmd(&schema, "amend");
        let matches = cmd.get_matches_from(["amend", "T001"]);
        let err = run(&schema, &conn, &matches, Actor::Human.into(), "amend").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("amend") || msg.contains("no transition") || msg.contains("accepted"),
            "expected state-machine error for amend from accepted; got: {msg}"
        );
        // Row unchanged
        assert_eq!(read_status_wrap(&conn), "accepted");
    }
}
