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

/// Read the policy_ref/policies_hash env vars set by the autonomous flow
/// daemon (Phase 5: agents_run.rs::run_dispatch). When unset (the manual CLI
/// path), returns `(None, None)` so transition_history records NULL — the
/// distinct sentinel for "manual transition" per AC5.4.
pub(crate) fn read_policy_env() -> (Option<String>, Option<String>) {
    let pref = std::env::var("STORES_POLICY_REF")
        .ok()
        .filter(|s| !s.is_empty());
    let phash = std::env::var("STORES_POLICIES_HASH")
        .ok()
        .filter(|s| !s.is_empty());
    (pref, phash)
}

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

    let (pref, phash) = read_policy_env();
    execute_transition_write(
        &tx,
        schema,
        row_id,
        display_id,
        current_status,
        &transition.to,
        "close_as_addressed",
        &diff,
        &merged,
        invoker.actor,
        pref.as_deref(),
        phash.as_deref(),
        None,
    )?;

    tx.commit().context("close_as_addressed: commit tx")?;

    println!(
        "Transitioned {display_id}: {} → {} (resolution={resolution})",
        transition.from, transition.to
    );
    Ok(())
}

/// Entry point for `close-out-of-band` (tasks recovery-terminal):
/// validates --commit shape (7-40 hex chars) and reachability in `main`, then
/// transitions the row to `closed_out_of_band`. Idempotent: if the row is
/// already `closed_out_of_band`, prints a no-op line and returns Ok. Records
/// the SHA in `transition_history.actor_note`.
pub fn run_close_out_of_band(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
    commit: &str,
) -> Result<()> {
    // 1. Validate SHA shape: 7-40 lowercase hex chars.
    let sha_re = regex::Regex::new(r"^[0-9a-f]{7,40}$").unwrap();
    if !sha_re.is_match(commit) {
        anyhow::bail!(
            "--commit '{commit}' is not a valid git SHA (expected 7-40 lowercase hex chars)"
        );
    }

    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let tx = conn
        .unchecked_transaction()
        .context("close-out-of-band: begin tx")?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // 2. Idempotency: already in target state → no-op success. Done before
    // git validation so a re-run after the SHA has fallen out of the local
    // main (e.g. because the operator hasn't fetched recently) still no-ops.
    if current_status == "closed_out_of_band" {
        println!(
            "{display_id} already closed_out_of_band (no-op; commit={commit})"
        );
        return Ok(());
    }

    // 3. Resolve transition (errors if from-state is terminal/disallowed).
    let merged = existing.clone();
    let transition = select_transition(
        &schema.lifecycle.transitions,
        current_status,
        "close-out-of-band",
        None,
        &merged,
    )?;

    let diff: crate::validate::EntryMap = std::collections::BTreeMap::new();

    validate::validate(
        schema,
        &merged,
        Op::Transition("close-out-of-band".to_string(), diff.clone()),
        invoker,
    )
    .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    // 4. Validate SHA reachable in main. Last gate before the write — if we
    // reach this point, the row is non-terminal and the actor check passed,
    // so the SHA validation is the final precondition. The contract requires
    // real git validation; there is no production escape hatch.
    validate_sha_reachable_in_main(commit)?;

    // 5. Write transition + audit row with SHA in actor_note.
    let now = now_iso8601();
    let invoker_str = invoker.actor.to_string();
    let qtable = quote_ident(&schema.name);
    tx.execute(
        &format!(
            "UPDATE {qtable} SET updated_at = ?1, updated_by = ?2, status = ?3 WHERE id = ?4"
        ),
        rusqlite::params![now, invoker_str, transition.to, row_id],
    )
    .context("close-out-of-band: update row")?;

    let (pref, phash) = read_policy_env();
    crate::db::insert_transition_history_with_note(
        &tx,
        &schema.name,
        row_id,
        display_id,
        current_status,
        &transition.to,
        "close-out-of-band",
        &invoker_str,
        pref.as_deref(),
        phash.as_deref(),
        Some(commit),
    )?;

    tx.commit().context("close-out-of-band: commit tx")?;

    println!(
        "Transitioned {display_id}: {} → {} (commit={commit})",
        transition.from, transition.to
    );
    Ok(())
}

/// Verify that `sha` is reachable from the local `main` ref via
/// `git merge-base --is-ancestor <sha> main`. Errors fail-loud with a clear
/// message — recovery requires a real merge target.
///
/// Only `main` is consulted. The contract requires reachable-in-main; a
/// stale or divergent `master` ref must NOT silently satisfy this gate.
fn validate_sha_reachable_in_main(sha: &str) -> Result<()> {
    use std::process::Command;
    let out = Command::new("git")
        .args(["merge-base", "--is-ancestor", sha, "main"])
        .output()
        .map_err(|e| anyhow::anyhow!(
            "--commit '{sha}' validation failed: could not run git ({e}). \
             close-out-of-band requires a real git repo with a 'main' ref."
        ))?;
    if out.status.success() {
        return Ok(());
    }
    anyhow::bail!(
        "--commit '{sha}' is not reachable from main. \
         close-out-of-band requires the merge-target SHA to already be on \
         the 'main' branch (no fallback to 'master'). Run \
         `git fetch origin main && git merge-base --is-ancestor {sha} main` \
         to confirm."
    )
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

    // T053 P2/P3: validate + mirror the gatekeeper payload BEFORE routing side-effects.
    // The observation side-effect needs the derived L143 columns in `merged` so the
    // resulting observation and the intake transition are written atomically with the
    // same gatekeeper-derived risk_class / approval_policy / risk_flags / cluster_key.
    if schema.name == "intake" && matches!(verb, "route" | "escalate-arch-review") {
        maybe_validate_and_mirror_gatekeeper_decision(schema, &mut diff, &mut merged)?;
        super::intake_route::inject_pre_validation_fields(
            tx,
            &mut diff,
            &mut merged,
            verb,
        )?;
    }

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
    let (pref, phash) = read_policy_env();
    execute_transition_write(
        tx,
        schema,
        row_id,
        display_id,
        current_status,
        &transition.to,
        verb,
        &diff,
        &merged,
        invoker.actor,
        pref.as_deref(),
        phash.as_deref(),
        None,
    )?;

    println!(
        "Transitioned {display_id}: {} → {}",
        transition.from, transition.to
    );

    // T020 P1: post-confirm auto-ratify hook on observations. When a confirm
    // succeeds and the row's intent_contract is fully approved, framework
    // synchronously fires `ratify` (confirmed → ready) inside the same tx.
    // The schema guard checks contract_state=='ready'; we re-check
    // approved_by/approved_at != null here because the guard parser does not
    // support compound expressions.
    if schema.name == "observations" && verb == "confirm" {
        maybe_auto_ratify_observation(
            tx,
            schema,
            row_id,
            display_id,
            &merged,
            pref.as_deref(),
            phash.as_deref(),
        )?;
    }

    // T053 P3: intake recon-return hook.  Fires AFTER execute_transition_write so
    // the status row is already needs_info → triaging; then increments recon_round
    // and enforces the ≤ 2 cap.  Failure rolls back the entire transition.
    if schema.name == "intake" && verb == "recon-return" {
        super::intake_route::handle_recon_return(tx, row_id, &merged, &diff)?;
    }

    Ok(())
}

/// T020 P1: post-confirm hook. If the just-confirmed observation's
/// `intent_contract` is `ready` AND has `approved_by` AND `approved_at`
/// populated, fire framework `ratify` (confirmed → ready) atomically in the
/// same caller-supplied transaction.
pub(crate) fn maybe_auto_ratify_observation(
    tx: &Transaction,
    schema: &Schema,
    row_id: i64,
    display_id: &str,
    merged: &crate::validate::EntryMap,
    policy_ref: Option<&str>,
    policies_hash: Option<&str>,
) -> Result<()> {
    let intent = match merged.get("intent_contract").and_then(|v| v.as_object()) {
        Some(o) => o,
        None => return Ok(()),
    };
    let contract_ready = intent
        .get("contract_state")
        .and_then(|v| v.as_str())
        == Some("ready");
    let approved_by_set = intent
        .get("approved_by")
        .map(|v| match v {
            Value::Null => false,
            Value::String(s) => !s.is_empty(),
            _ => true,
        })
        .unwrap_or(false);
    let approved_at_set = intent
        .get("approved_at")
        .map(|v| match v {
            Value::Null => false,
            Value::String(s) => !s.is_empty(),
            _ => true,
        })
        .unwrap_or(false);
    if !(contract_ready && approved_by_set && approved_at_set) {
        return Ok(());
    }

    // Build a no-op diff and resolve the ratify transition from current status
    // (which is now 'confirmed' inside this tx).
    let ratify_diff: crate::validate::EntryMap = std::collections::BTreeMap::new();
    let from_status = "confirmed";
    let transition = select_transition(
        &schema.lifecycle.transitions,
        from_status,
        "ratify",
        None,
        merged,
    )?;

    validate::validate(
        schema,
        merged,
        Op::Transition("ratify".to_string(), ratify_diff.clone()),
        Actor::Framework.into(),
    )
    .map_err(|errs| {
        anyhow::anyhow!(
            "auto-ratify validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    execute_transition_write(
        tx,
        schema,
        row_id,
        display_id,
        from_status,
        &transition.to,
        "ratify",
        &ratify_diff,
        merged,
        Actor::Framework,
        policy_ref,
        policies_hash,
        None,
    )?;

    println!(
        "Auto-ratified {display_id}: {} → {} (framework)",
        from_status, transition.to
    );
    Ok(())
}

/// Write the transition state change into the DB (inside a caller-supplied transaction).
/// Used by both `run_in_tx` (CLI path) and submit handlers (engine path).
/// Also inserts an audit row into `transition_history` (T014 P1).
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_transition_write(
    tx: &Transaction,
    schema: &Schema,
    row_id: i64,
    display_id: &str,
    from_status: &str,
    new_status: &str,
    verb: &str,
    diff: &crate::validate::EntryMap,
    merged: &crate::validate::EntryMap,
    invoker: Actor,
    policy_ref: Option<&str>,
    policies_hash: Option<&str>,
    actor_note: Option<&str>,
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
        rusqlite::types::Value::Text(invoker_str.clone()),
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

    crate::db::insert_transition_history_with_note(
        tx,
        &schema.name,
        row_id,
        display_id,
        from_status,
        new_status,
        verb,
        &invoker_str,
        policy_ref,
        policies_hash,
        actor_note,
    )?;

    Ok(())
}

/// T053 P2: validate gatekeeper_decision_json and mirror risk_flags, cluster_key,
/// and decision_metadata into diff + merged so they are written in the same transaction.
///
/// Only fires for the `intake` store on `route` and `escalate-arch-review` verbs.
/// Called after generic `validate::validate()` passes — the generic validator already
/// rejects badly-formed JSON, so here we know the value is either a parsed object or null.
fn maybe_validate_and_mirror_gatekeeper_decision(
    schema: &crate::schema::Schema,
    diff: &mut crate::validate::EntryMap,
    merged: &mut crate::validate::EntryMap,
) -> anyhow::Result<()> {
    let decision_json_val = match merged.get("gatekeeper_decision_json") {
        Some(v) => v.clone(),
        None => return Ok(()),
    };

    // Null means not yet set — no validation required
    if decision_json_val.is_null() {
        return Ok(());
    }

    // The generic JSON validator already caught malformed strings; here we expect an object.
    // If it's a String (sentinel), bail gracefully — the generic validator's error already fired.
    if decision_json_val.is_string() {
        return Ok(());
    }

    // Validate the gatekeeper decision payload
    crate::validate::gatekeeper_decision::validate_gatekeeper_decision(&decision_json_val)
        .map_err(|errs| {
            anyhow::anyhow!(
                "gatekeeper_decision_json validation failed:\n- {}",
                errs.join("\n- ")
            )
        })?;

    // Mirror risk_flags and cluster_key to top-level indexed columns
    if let Some(obj) = decision_json_val.as_object() {
        // risk_flags → top-level column (type: json, stores as JSON array string)
        if let Some(flags) = obj.get("risk_flags") {
            diff.insert("risk_flags".to_string(), flags.clone());
            merged.insert("risk_flags".to_string(), flags.clone());
        }

        // cluster_key → top-level column (type: text)
        if let Some(ck) = obj.get("cluster_key") {
            diff.insert("cluster_key".to_string(), ck.clone());
            merged.insert("cluster_key".to_string(), ck.clone());
        }

        // decision_metadata: {matched_cluster_key, risk_flags_set, risk_class_hint, approval_policy_hint}
        let risk_flags_arr: Vec<&str> = obj
            .get("risk_flags")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        let risk_class =
            crate::schema::risk_taxonomy::derive_risk_class(&risk_flags_arr);
        let decision_str = obj.get("decision").and_then(|v| v.as_str()).unwrap_or("");
        let tier_hint = obj.get("tier_hint").and_then(|v| v.as_str()).unwrap_or("");
        let approval_policy = crate::schema::risk_taxonomy::derive_approval_policy(
            tier_hint,
            risk_class,
            decision_str,
        );

        let meta = serde_json::json!({
            "matched_cluster_key": obj.get("cluster_key"),
            "risk_flags_set": risk_flags_arr,
            "risk_class_hint": risk_class,
            "approval_policy_hint": approval_policy,
            "rationale": obj.get("rationale"),
            "confidence": obj.get("confidence"),
            "tier_hint": obj.get("tier_hint"),
        });

        // Check that decision_metadata is in the schema before injecting
        let has_decision_metadata = schema.fields.iter().any(|f| f.name == "decision_metadata");
        if has_decision_metadata {
            diff.insert("decision_metadata".to_string(), meta.clone());
            merged.insert("decision_metadata".to_string(), meta);
        }
    }

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

    // ---- T019 Phase 1: mark_cargo_installed framework transition test (AC1.2) ----

    /// Minimal schema mirroring T014's mark_deploy_blocked shape: a framework-actor
    /// transition from accepted → cargo_installed via verb mark_cargo_installed.
    const CARGO_INSTALL_SCHEMA: &str = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [accepted, cargo_installed]
  transitions:
    - from: accepted
      to: cargo_installed
      verb: mark_cargo_installed
      actor: framework
fields:
  - name: title
    type: text
    required: true
"#;

    /// AC1.2: A framework-invoker `mark_cargo_installed` transition from `accepted`
    /// lands the row at `cargo_installed` and writes a transition_history audit row
    /// with verb=mark_cargo_installed and invoker=framework.
    #[test]
    fn ac1_2_mark_cargo_installed_writes_transition_history() {
        let schema = Schema::from_yaml(CARGO_INSTALL_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();

        conn.execute(
            "INSERT INTO tasks (display_id, status, title) VALUES ('T001', 'accepted', 'Test')",
            [],
        )
        .unwrap();

        let cmd = build_wrap_cmd(&schema, "mark_cargo_installed");
        let matches = cmd.get_matches_from(["mark_cargo_installed", "T001"]);
        run(
            &schema,
            &conn,
            &matches,
            Actor::Framework.into(),
            "mark_cargo_installed",
        )
        .unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id = 'T001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "cargo_installed");

        let (verb, invoker): (String, String) = conn
            .query_row(
                "SELECT verb, invoker FROM transition_history \
                 WHERE store='tasks' AND display_id='T001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(verb, "mark_cargo_installed");
        assert_eq!(invoker, "framework");
    }

    // ---- T020 P1: post-confirm auto-ratify (observations) ----

    fn setup_bundled_observations() -> (Schema, Connection) {
        let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "observations")
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();
        (schema, conn)
    }

    /// Insert an observations row directly at `status=investigating` with the
    /// supplied intent_contract JSON.  Bypasses validation so tests can craft
    /// rows that drive the post-confirm hook deterministically.
    fn insert_investigating_obs(conn: &Connection, display_id: &str, intent_contract: &str) {
        let now = "2026-05-03T00:00:00Z";
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, summary, source, priority, captured_at, captured_week, intent_contract, \
              created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'investigating', 'test obs', 'dev', 'normal', ?2, 'w18-d3', ?3, ?2, ?2, 'ai_with_human', 'ai_with_human')",
            rusqlite::params![display_id, now, intent_contract],
        )
        .unwrap();
    }

    fn ready_approved_contract() -> String {
        serde_json::json!({
            "contract_state": "ready",
            "drafted_by": "test",
            "drafted_at": "2026-05-03T00:00:00Z",
            "objective": "do the thing",
            "type": "work",
            "in_scope": ["x"],
            "out_of_scope": ["y"],
            "acceptance": ["z"],
            "tier_hint": "T2",
            "approved_by": "blake",
            "approved_at": "2026-05-03T00:01:00Z",
        })
        .to_string()
    }

    fn ready_unapproved_contract() -> String {
        // contract_state == ready but approved_at missing.
        serde_json::json!({
            "contract_state": "ready",
            "drafted_by": "test",
            "drafted_at": "2026-05-03T00:00:00Z",
            "objective": "do the thing",
            "type": "work",
            "in_scope": ["x"],
            "out_of_scope": ["y"],
            "acceptance": ["z"],
            "tier_hint": "T2",
            "approved_by": "blake",
        })
        .to_string()
    }

    fn build_obs_cmd(schema: &Schema, verb: &'static str) -> clap::Command {
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

    /// AC1.3: confirm with a fully-approved contract auto-fires ratify and the
    /// row lands at `ready` with two transition_history rows
    /// (investigating→confirmed via confirm; confirmed→ready via ratify, framework).
    #[test]
    fn confirm_with_ready_contract_auto_ratifies() {
        let (schema, conn) = setup_bundled_observations();
        insert_investigating_obs(&conn, "L001", &ready_approved_contract());

        let cmd = build_obs_cmd(&schema, "confirm");
        let matches = cmd.get_matches_from(["confirm", "L001"]);
        run(&schema, &conn, &matches, Actor::Human.into(), "confirm").unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "ready", "row must auto-ratify to 'ready'");

        let rows: Vec<(String, String, String, String)> = conn
            .prepare(
                "SELECT from_status, to_status, verb, invoker FROM transition_history \
                 WHERE store='observations' AND display_id='L001' ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), 2, "expected 2 transition_history rows; got {rows:?}");
        assert_eq!(rows[0].0, "investigating");
        assert_eq!(rows[0].1, "confirmed");
        assert_eq!(rows[0].2, "confirm");
        assert_eq!(rows[1].0, "confirmed");
        assert_eq!(rows[1].1, "ready");
        assert_eq!(rows[1].2, "ratify");
        assert_eq!(rows[1].3, "framework");
    }

    /// AC1.4: confirm with contract.contract_state=='ready' but missing
    /// approved_at must NOT auto-ratify.  In practice required_when forces
    /// confirm itself to fail validation, so the row stays at 'investigating'
    /// and no transition_history row is written for it.  Either way: no
    /// auto-ratify fires.
    #[test]
    fn confirm_without_approval_does_not_auto_ratify() {
        let (schema, conn) = setup_bundled_observations();
        insert_investigating_obs(&conn, "L002", &ready_unapproved_contract());

        let cmd = build_obs_cmd(&schema, "confirm");
        let matches = cmd.get_matches_from(["confirm", "L002"]);
        let result = run(&schema, &conn, &matches, Actor::Human.into(), "confirm");
        // Confirm must fail because approved_at is required_when contract_state==ready.
        assert!(
            result.is_err(),
            "confirm with missing approved_at must fail validation"
        );

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id='L002'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "investigating",
            "row must stay at investigating when confirm fails"
        );

        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE store='observations' AND display_id='L002'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0, "no auto-ratify history when confirm itself failed");
    }

    /// AC1.4 (test c): direct ratify by a non-framework actor is rejected by
    /// the transition's actor gate.
    #[test]
    fn ratify_rejected_for_non_framework_actor() {
        let (schema, conn) = setup_bundled_observations();
        // Insert a row already at 'confirmed' with a fully-approved contract.
        let now = "2026-05-03T00:00:00Z";
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, summary, source, priority, captured_at, captured_week, intent_contract, \
              created_at, updated_at, created_by, updated_by) \
             VALUES ('L003', 'confirmed', 'test obs', 'dev', 'normal', ?1, 'w18-d3', ?2, ?1, ?1, 'human', 'human')",
            rusqlite::params![now, ready_approved_contract()],
        )
        .unwrap();

        let cmd = build_obs_cmd(&schema, "ratify");
        let matches = cmd.get_matches_from(["ratify", "L003"]);
        let err = run(&schema, &conn, &matches, Actor::Human.into(), "ratify").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("framework"),
            "expected actor mismatch citing 'framework'; got: {msg}"
        );

        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id='L003'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "confirmed",
            "row must be unchanged after rejected ratify"
        );
    }

    // ---- T044: close-out-of-band recovery-terminal verb tests ----

    const COOB_SCHEMA: &str = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [planning, executing, accepted, closed_out_of_band]
  transitions:
    - {from: planning, to: closed_out_of_band, verb: close-out-of-band, actor: human}
    - {from: executing, to: closed_out_of_band, verb: close-out-of-band, actor: human}
fields:
  - name: title
    type: text
    required: true
"#;

    fn setup_coob() -> (Schema, Connection) {
        let schema = Schema::from_yaml(COOB_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    fn insert_coob_row(conn: &Connection, status: &str) {
        conn.execute(
            "INSERT INTO tasks (display_id, status, title) VALUES ('T001', ?1, 'Test')",
            rusqlite::params![status],
        )
        .unwrap();
    }

    fn build_coob_cmd() -> clap::Command {
        clap::Command::new("close-out-of-band")
            .arg(clap::Arg::new("display_id").required(true).index(1))
            .arg(
                clap::Arg::new("commit")
                    .long("commit")
                    .required(true),
            )
    }

    fn read_status(conn: &Connection) -> String {
        conn.query_row(
            "SELECT status FROM tasks WHERE display_id = 'T001'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn audit_rows_for(conn: &Connection) -> Vec<(String, String, Option<String>)> {
        let mut s = conn
            .prepare(
                "SELECT verb, to_status, actor_note FROM transition_history \
                 WHERE display_id = 'T001' ORDER BY id ASC",
            )
            .unwrap();
        let rows = s
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        rows
    }

    /// Set up a real temp git repo with a single empty commit on `main`.
    /// Returns (TempDir guard, full SHA of the commit). cwd must be set to
    /// the tempdir for `git merge-base --is-ancestor <sha> main` to find it.
    fn coob_real_git_repo() -> (tempfile::TempDir, String) {
        use std::process::Command;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let must = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(p)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            out
        };
        must(&["init", "-q", "-b", "main"]);
        must(&["commit", "-q", "--allow-empty", "-m", "init"]);
        let rev = must(&["rev-parse", "HEAD"]);
        let sha = String::from_utf8(rev.stdout)
            .unwrap()
            .trim()
            .to_string();
        (dir, sha)
    }

    /// AC1+AC4+AC5: happy path. Walk a `planning` row to `closed_out_of_band`
    /// against a REAL git repo with the SHA actually reachable in main; audit
    /// row records the SHA in actor_note.
    #[test]
    fn coob_planning_to_closed_succeeds_and_records_sha() {
        let (dir, sha) = coob_real_git_repo();
        let _g = scoped_cwd(dir.path());
        let (schema, conn) = setup_coob();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        insert_coob_row(&conn, "planning");
        let m = build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", &sha]);
        run_close_out_of_band(&schema, &conn, &m, Actor::Human.into(), &sha).unwrap();
        assert_eq!(read_status(&conn), "closed_out_of_band");
        let audit = audit_rows_for(&conn);
        assert_eq!(audit.len(), 1, "one audit row");
        assert_eq!(audit[0].0, "close-out-of-band");
        assert_eq!(audit[0].1, "closed_out_of_band");
        assert_eq!(audit[0].2.as_deref(), Some(sha.as_str()));
    }

    /// AC2 (gate): unreachable SHA on a non-terminal row is REFUSED. Closes
    /// the prior-cycle MEDIUM finding that no test exercised the validation.
    /// Uses a real git repo whose 'main' does NOT contain the asserted SHA.
    #[test]
    fn coob_refuses_unreachable_sha() {
        let (dir, _real_sha) = coob_real_git_repo();
        let _g = scoped_cwd(dir.path());
        let (schema, conn) = setup_coob();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        insert_coob_row(&conn, "planning");
        // Well-formed SHA shape that is NOT in this repo.
        let bogus = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let m =
            build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", bogus]);
        let err = run_close_out_of_band(&schema, &conn, &m, Actor::Human.into(), bogus)
            .expect_err("unreachable SHA must be refused");
        let msg = err.to_string();
        assert!(
            msg.contains("not reachable from main"),
            "expected reachable-in-main error; got: {msg}"
        );
        assert_eq!(read_status(&conn), "planning", "row unchanged");
    }

    /// AC2 (gate): no fallback to `master`. Even if a divergent `master`
    /// branch exists with the SHA, the gate must fail because contract
    /// requires reachable in `main`.
    #[test]
    fn coob_no_master_fallback() {
        use std::process::Command;
        let (dir, _) = coob_real_git_repo();
        let p = dir.path();
        let must = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(p)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .output()
                .unwrap();
            assert!(out.status.success());
            out
        };
        // Create a 'master' branch with a different commit; main does NOT
        // contain that commit. Switch back to main so cwd is on main but
        // master holds the divergent SHA.
        must(&["checkout", "-q", "-b", "master"]);
        must(&["commit", "-q", "--allow-empty", "-m", "master-only"]);
        let rev = must(&["rev-parse", "HEAD"]);
        let master_only_sha = String::from_utf8(rev.stdout).unwrap().trim().to_string();
        must(&["checkout", "-q", "main"]);

        let _g = scoped_cwd(p);
        let (schema, conn) = setup_coob();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        insert_coob_row(&conn, "planning");
        let m = build_coob_cmd().get_matches_from([
            "close-out-of-band",
            "T001",
            "--commit",
            &master_only_sha,
        ]);
        let err = run_close_out_of_band(&schema, &conn, &m, Actor::Human.into(), &master_only_sha)
            .expect_err("master-only SHA must be refused");
        assert!(
            err.to_string().contains("not reachable from main"),
            "expected main-only error; got: {err}"
        );
    }

    /// Restores the previous cwd when dropped. Tests using cwd-scoped git
    /// validation must wrap the body in this guard. Uses the process-wide
    /// `paths::test_cwd_lock()` so we serialize with ALL other cwd-using
    /// tests in the suite, not just among ourselves.
    fn scoped_cwd(p: &std::path::Path) -> impl Drop {
        let lock = crate::paths::test_cwd_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(p).unwrap();
        struct Guard {
            prev: std::path::PathBuf,
            _lock: std::sync::MutexGuard<'static, ()>,
        }
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::env::set_current_dir(&self.prev);
            }
        }
        Guard {
            prev,
            _lock: lock,
        }
    }

    /// AC3: refused from terminal state (`accepted` is not declared as a
    /// from-state in the close-out-of-band transitions, so select_transition
    /// must error).
    #[test]
    fn coob_refused_from_terminal_accepted() {
        let (schema, conn) = setup_coob();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        insert_coob_row(&conn, "accepted");
        let sha = "abc1234";
        let m = build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", sha]);
        let err = run_close_out_of_band(&schema, &conn, &m, Actor::Human.into(), sha).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("close-out-of-band") || msg.contains("no transition"),
            "expected refusal citing the verb; got: {msg}"
        );
        assert_eq!(read_status(&conn), "accepted", "row unchanged");
    }

    /// AC6: idempotent. Calling on an already-closed_out_of_band row succeeds
    /// (no audit row written, status unchanged).
    #[test]
    fn coob_idempotent_when_already_closed() {
        let (schema, conn) = setup_coob();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        insert_coob_row(&conn, "closed_out_of_band");
        let sha = "abc1234";
        let m = build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", sha]);
        run_close_out_of_band(&schema, &conn, &m, Actor::Human.into(), sha).unwrap();
        assert_eq!(read_status(&conn), "closed_out_of_band");
        let audit = audit_rows_for(&conn);
        assert!(audit.is_empty(), "no audit row written on idempotent no-op");
    }

    /// AC2: --commit shape rejected if not 7-40 hex chars.
    #[test]
    fn coob_rejects_bad_sha_shape() {
        let (schema, conn) = setup_coob();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        insert_coob_row(&conn, "planning");
        let bad = "notahex"; // 7 chars but contains non-hex
        let m = build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", bad]);
        let err = run_close_out_of_band(&schema, &conn, &m, Actor::Human.into(), bad).unwrap_err();
        assert!(
            err.to_string().contains("not a valid git SHA"),
            "expected SHA-shape error; got: {err}"
        );
        assert_eq!(read_status(&conn), "planning", "row unchanged");
    }

    /// AC8: tier-A — ai_autonomous rejected even with a "valid" token bit.
    #[test]
    fn coob_rejects_ai_autonomous() {
        let (schema, conn) = setup_coob();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        insert_coob_row(&conn, "planning");
        let sha = "abc1234";
        let m = build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", sha]);
        let err =
            run_close_out_of_band(&schema, &conn, &m, Actor::AiAutonomous.into(), sha).unwrap_err();
        assert!(
            err.to_string().contains("validation failed")
                || err.to_string().contains("actor"),
            "expected actor-validation failure; got: {err}"
        );
        assert_eq!(read_status(&conn), "planning", "row unchanged");
    }

    /// AC8: tier-A — ai_with_human + valid token accepted.
    #[test]
    fn coob_accepts_ai_with_human_with_token() {
        let (dir, sha) = coob_real_git_repo();
        let _g = scoped_cwd(dir.path());
        let (schema, conn) = setup_coob();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        insert_coob_row(&conn, "executing");
        let m = build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", &sha]);
        let invoker = InvokerCtx {
            actor: Actor::AiWithHuman,
            token_valid: true,
        };
        run_close_out_of_band(&schema, &conn, &m, invoker, &sha).unwrap();
        assert_eq!(read_status(&conn), "closed_out_of_band");
    }
}
