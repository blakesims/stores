use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::Value;

use crate::codegen::ddl::quote_ident;
use crate::schema::{
    actor::{Actor, InvokerCtx},
    lifecycle::{select_transition, Transition},
    FieldType, Schema,
};
use crate::validate::{self, Op};

use super::row::{build_entry_map, deep_merge_entry_field, now_iso8601, read_row};

fn value_as_string(entry: &crate::validate::EntryMap, key: &str) -> Option<String> {
    entry
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn first_string(entry: &crate::validate::EntryMap, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| value_as_string(entry, key))
}

fn bool_value(entry: &crate::validate::EntryMap, key: &str) -> Option<bool> {
    entry.get(key).and_then(|v| match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => Some(n.as_i64() == Some(1)),
        Value::String(s) => Some(matches!(s.as_str(), "true" | "1" | "yes")),
        _ => None,
    })
}

fn string_list_value(entry: &crate::validate::EntryMap, key: &str) -> Vec<String> {
    entry
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn intent_contract_state(entry: &crate::validate::EntryMap) -> Option<String> {
    entry
        .get("intent_contract")
        .and_then(Value::as_object)
        .and_then(|o| o.get("contract_state"))
        .and_then(Value::as_str)
        .map(|s| if s == "ready" { "approved" } else { s }.to_string())
        .or_else(|| value_as_string(entry, "contract_state"))
}

fn upstream_store(schema: &Schema) -> Option<super::upstream_overlay::UpstreamStore> {
    let has_primary_lifecycle = schema.fields.iter().any(|f| f.name == "lifecycle");
    if !has_primary_lifecycle {
        return None;
    }
    match schema.name.as_str() {
        "intake" => Some(super::upstream_overlay::UpstreamStore::Intake),
        "observations" => Some(super::upstream_overlay::UpstreamStore::Observations),
        "architecture_reviews" => Some(super::upstream_overlay::UpstreamStore::ArchitectureReviews),
        _ => None,
    }
}

pub(crate) fn inject_upstream_primary_tuple(
    schema: &Schema,
    transition: &Transition,
    verb: &str,
    from_status: &str,
    to_status: &str,
    diff: &mut crate::validate::EntryMap,
    merged: &mut crate::validate::EntryMap,
) -> Result<()> {
    let Some(store) = upstream_store(schema) else {
        return Ok(());
    };
    merged.insert("status".into(), Value::String(to_status.to_string()));
    if schema.name == "observations" {
        if let Some(contract_state) = intent_contract_state(merged) {
            merged.insert(
                "contract_state".into(),
                Value::String(contract_state.clone()),
            );
            diff.insert("contract_state".into(), Value::String(contract_state));
        }
    }
    let refs = super::upstream_overlay::ReferencesIn {
        routed_to_observation: first_string(merged, &["produced_observation_id", "routed_to_observation"]),
        routed_to_arch_review: first_string(merged, &["produced_architecture_review_id", "routed_to_arch_review"]),
        produced_task_id: value_as_string(merged, "produced_task_id"),
        produced_artifact_kind: value_as_string(merged, "produced_artifact_kind"),
        produced_artifact_id: value_as_string(merged, "produced_artifact_id"),
        duplicate_of: first_string(merged, &["duplicate_of_id", "duplicate_of"]),
        contract_state: intent_contract_state(merged),
        pending_architecture_review: bool_value(merged, "pending_architecture_review"),
        clearable_by_ruling: value_as_string(merged, "clearable_by_ruling"),
        open_architecture_review_id: value_as_string(merged, "open_architecture_review_id"),
        resolution_kind: value_as_string(merged, "resolution_kind"),
        resolution: value_as_string(merged, "resolution"),
        merge_target_id: value_as_string(merged, "merge_target_id"),
        resolved_by: value_as_string(merged, "resolved_by"),
        task_id: value_as_string(merged, "task_id"),
        addressed_by_commit_sha: value_as_string(merged, "addressed_by_commit_sha"),
        superseded_by_id: value_as_string(merged, "superseded_by_id"),
        source_observation: value_as_string(merged, "source_observation"),
        source_intake: value_as_string(merged, "source_intake"),
        linked_observation_ids: string_list_value(merged, "linked_observation_ids"),
        supersedes: value_as_string(merged, "supersedes"),
        updated_at: value_as_string(merged, "updated_at"),
    };
    let decision_or_verdict = if schema.name == "intake" {
        value_as_string(merged, "decision")
    } else if schema.name == "architecture_reviews" {
        value_as_string(merged, "verdict")
    } else {
        None
    };
    let tuple = super::upstream_overlay::derive(
        store,
        verb,
        from_status,
        to_status,
        decision_or_verdict.as_deref(),
        refs,
    );
    match tuple {
        super::upstream_overlay::PrimaryTuple::Intake(p) => {
            insert_string(diff, merged, "lifecycle", p.lifecycle.as_str());
            insert_opt_string(diff, merged, "waiting_kind", p.waiting.map(|w| w.as_str()));
            insert_opt_string(diff, merged, "outcome", p.outcome.map(|o| o.as_str()));
            insert_opt_owned(diff, merged, "produced_observation_id", p.references.produced_observation_id);
            insert_opt_owned(diff, merged, "produced_architecture_review_id", p.references.produced_architecture_review_id);
            insert_opt_owned(diff, merged, "duplicate_of_id", p.references.duplicate_of_id);
        }
        super::upstream_overlay::PrimaryTuple::Observation(p) => {
            insert_string(diff, merged, "lifecycle", p.lifecycle.as_str());
            insert_string(diff, merged, "contract_state", p.contract_state.as_str());
            insert_bool(diff, merged, "waiting", p.waiting.is_some());
            insert_opt_string(diff, merged, "waiting_kind", p.waiting.map(|w| w.as_str()));
            insert_opt_string(diff, merged, "outcome", p.outcome.map(|o| o.as_str()));
            insert_opt_owned(
                diff,
                merged,
                "addressed_by_task_id",
                p.references.addressed_by_task_id,
            );
            insert_opt_owned(
                diff,
                merged,
                "addressed_by_commit_sha",
                p.references.addressed_by_commit_sha,
            );
            insert_opt_owned(
                diff,
                merged,
                "superseded_by_id",
                p.references.superseded_by_id,
            );
        }
        super::upstream_overlay::PrimaryTuple::ArchitectureReview(p) => {
            insert_string(diff, merged, "lifecycle", p.lifecycle.as_str());
            insert_opt_string(diff, merged, "outcome", p.outcome.map(|o| o.as_str()));
        }
    }
    assert_upstream_tuple_matches_projection(schema, merged)?;
    if let Some(lifecycle) = &transition.lifecycle {
        let got = value_as_string(merged, "lifecycle").unwrap_or_default();
        debug_assert_eq!(&got, lifecycle, "ADR0002 lifecycle annotation mismatch");
    }
    Ok(())
}

fn insert_string(
    diff: &mut crate::validate::EntryMap,
    merged: &mut crate::validate::EntryMap,
    key: &str,
    value: &str,
) {
    diff.insert(key.into(), Value::String(value.into()));
    merged.insert(key.into(), Value::String(value.into()));
}
fn insert_bool(
    diff: &mut crate::validate::EntryMap,
    merged: &mut crate::validate::EntryMap,
    key: &str,
    value: bool,
) {
    diff.insert(key.into(), Value::Bool(value));
    merged.insert(key.into(), Value::Bool(value));
}
fn insert_opt_string(
    diff: &mut crate::validate::EntryMap,
    merged: &mut crate::validate::EntryMap,
    key: &str,
    value: Option<&str>,
) {
    match value {
        Some(v) => insert_string(diff, merged, key, v),
        None => {
            diff.insert(key.into(), Value::Null);
            merged.insert(key.into(), Value::Null);
        }
    }
}
fn insert_opt_owned(
    diff: &mut crate::validate::EntryMap,
    merged: &mut crate::validate::EntryMap,
    key: &str,
    value: Option<String>,
) {
    match value {
        Some(v) => insert_string(diff, merged, key, &v),
        None => {
            if diff.contains_key(key) {
                diff.insert(key.into(), Value::Null);
                merged.insert(key.into(), Value::Null);
            }
        }
    }
}

pub(crate) fn strip_framework_overlay_from_validation_diff(
    schema: &Schema,
    diff: &crate::validate::EntryMap,
) -> crate::validate::EntryMap {
    if upstream_store(schema).is_none() {
        return diff.clone();
    }
    let mut d = diff.clone();
    for k in [
        "lifecycle",
        "waiting",
        "waiting_kind",
        "outcome",
        "contract_state",
        "addressed_by_task_id",
        "addressed_by_commit_sha",
        "superseded_by_id",
        "produced_observation_id",
        "produced_architecture_review_id",
        "duplicate_of_id",
        "produced_artifact_kind",
        "produced_artifact_id",
    ] {
        d.remove(k);
    }
    d
}

pub(crate) fn assert_upstream_tuple_matches_projection(
    schema: &Schema,
    merged: &crate::validate::EntryMap,
) -> Result<()> {
    let Some(store) = upstream_store(schema) else {
        return Ok(());
    };
    let status = value_as_string(merged, "status").unwrap_or_else(|| "".into());
    let refs = super::upstream_overlay::ReferencesIn {
        routed_to_observation: first_string(merged, &["produced_observation_id", "routed_to_observation"]),
        routed_to_arch_review: first_string(merged, &["produced_architecture_review_id", "routed_to_arch_review"]),
        produced_task_id: value_as_string(merged, "produced_task_id"),
        produced_artifact_kind: value_as_string(merged, "produced_artifact_kind"),
        produced_artifact_id: value_as_string(merged, "produced_artifact_id"),
        duplicate_of: first_string(merged, &["duplicate_of_id", "duplicate_of"]),
        contract_state: intent_contract_state(merged),
        pending_architecture_review: bool_value(merged, "pending_architecture_review"),
        clearable_by_ruling: value_as_string(merged, "clearable_by_ruling"),
        open_architecture_review_id: value_as_string(merged, "open_architecture_review_id"),
        resolution_kind: value_as_string(merged, "resolution_kind"),
        resolution: value_as_string(merged, "resolution"),
        merge_target_id: value_as_string(merged, "merge_target_id"),
        resolved_by: value_as_string(merged, "resolved_by"),
        task_id: value_as_string(merged, "task_id"),
        addressed_by_commit_sha: value_as_string(merged, "addressed_by_commit_sha"),
        superseded_by_id: value_as_string(merged, "superseded_by_id"),
        source_observation: value_as_string(merged, "source_observation"),
        source_intake: value_as_string(merged, "source_intake"),
        linked_observation_ids: string_list_value(merged, "linked_observation_ids"),
        supersedes: value_as_string(merged, "supersedes"),
        updated_at: value_as_string(merged, "updated_at"),
    };
    let decision_or_verdict = if schema.name == "intake" {
        value_as_string(merged, "decision")
    } else if schema.name == "architecture_reviews" {
        value_as_string(merged, "verdict")
    } else {
        None
    };
    let projected = super::upstream_overlay::derive(
        store,
        "invariant",
        "",
        &status,
        decision_or_verdict.as_deref(),
        refs,
    );
    match projected {
        super::upstream_overlay::PrimaryTuple::Intake(p) => {
            compare_str(merged, "lifecycle", p.lifecycle.as_str())?;
            compare_opt(merged, "waiting_kind", p.waiting.map(|w| w.as_str()))?;
            compare_opt(merged, "outcome", p.outcome.map(|o| o.as_str()))?;
            compare_opt(merged, "produced_observation_id", p.references.produced_observation_id.as_deref())?;
            compare_opt(merged, "produced_architecture_review_id", p.references.produced_architecture_review_id.as_deref())?;
            compare_opt(merged, "produced_task_id", p.references.produced_task_id.as_deref())?;
            compare_opt(merged, "produced_artifact_kind", p.references.produced_artifact_kind.as_deref())?;
            compare_opt(merged, "produced_artifact_id", p.references.produced_artifact_id.as_deref())?;
            compare_opt(merged, "duplicate_of_id", p.references.duplicate_of_id.as_deref())?;
        }
        super::upstream_overlay::PrimaryTuple::Observation(p) => {
            compare_str(merged, "lifecycle", p.lifecycle.as_str())?;
            compare_str(merged, "contract_state", p.contract_state.as_str())?;
            compare_opt(merged, "waiting_kind", p.waiting.map(|w| w.as_str()))?;
            compare_opt(merged, "outcome", p.outcome.map(|o| o.as_str()))?;
            compare_opt(merged, "addressed_by_task_id", p.references.addressed_by_task_id.as_deref())?;
            compare_opt(merged, "addressed_by_commit_sha", p.references.addressed_by_commit_sha.as_deref())?;
            compare_opt(merged, "superseded_by_id", p.references.superseded_by_id.as_deref())?;
        }
        super::upstream_overlay::PrimaryTuple::ArchitectureReview(p) => {
            compare_str(merged, "lifecycle", p.lifecycle.as_str())?;
            compare_opt(merged, "outcome", p.outcome.map(|o| o.as_str()))?;
            compare_opt(merged, "produced_task_id", p.references.produced_task_id.as_deref())?;
            compare_opt(merged, "superseded_by_id", p.references.superseded_by_id.as_deref())?;
        }
    }
    Ok(())
}
fn compare_str(entry: &crate::validate::EntryMap, field: &str, expected: &str) -> Result<()> {
    let got = entry.get(field).and_then(Value::as_str).unwrap_or("");
    if got != expected {
        anyhow::bail!(
            "ADR0002 primary tuple invariant mismatch: field {field} expected {expected} got {got}"
        );
    }
    Ok(())
}
fn compare_opt(
    entry: &crate::validate::EntryMap,
    field: &str,
    expected: Option<&str>,
) -> Result<()> {
    let got = entry.get(field).and_then(Value::as_str);
    if got != expected {
        anyhow::bail!(
            "ADR0002 primary tuple invariant mismatch: field {field} expected {:?} got {:?}",
            expected,
            got
        );
    }
    Ok(())
}

#[cfg(test)]
mod adr0002_primary_tuple_invariant {
    use super::*;
    use serde_json::Value;

    fn schema(path: &str) -> Schema {
        Schema::from_yaml(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn representative_row(schema_name: &str, t: &Transition) -> crate::validate::EntryMap {
        let mut row = crate::validate::EntryMap::new();
        row.insert("status".into(), Value::String(t.from.clone()));
        match schema_name {
            "intake" => {
                let decision = if t.verb == "route" {
                    match t.outcome.as_deref() {
                        Some("marked_duplicate") => "duplicate",
                        Some("fast_tracked") => "fast_track",
                        Some("routed_to_observation") => "normal_observation",
                        Some("escalated_to_architecture_review") => "arch_review_candidate",
                        Some("dropped_as_noise") => "reject_noise",
                        _ if t.to == "needs_info" => "needs_info",
                        _ => "normal_observation",
                    }
                } else {
                    "normal_observation"
                };
                row.insert("decision".into(), Value::String(decision.into()));
                row.insert("routed_to_observation".into(), Value::String("L001".into()));
                row.insert("produced_observation_id".into(), Value::String("L001".into()));
                row.insert("routed_to_arch_review".into(), Value::String("A001".into()));
                row.insert("produced_architecture_review_id".into(), Value::String("A001".into()));
                row.insert("duplicate_of".into(), Value::String("I000".into()));
                row.insert("duplicate_of_id".into(), Value::String("I000".into()));
                if decision == "fast_track" {
                    row.insert("produced_artifact_kind".into(), Value::String("observation".into()));
                    row.insert("produced_artifact_id".into(), Value::String("L001".into()));
                }
            }
            "observations" => {
                row.insert("contract_state".into(), Value::String("approved".into()));
                row.insert("intent_contract".into(), serde_json::json!({"contract_state":"ready"}));
                if t.to == "resolved" {
                    if t.verb == "supersede" {
                        row.insert("resolution_kind".into(), Value::String("superseded".into()));
                        row.insert("superseded_by_id".into(), Value::String("L999".into()));
                    } else if t.verb == "auto_resolve" {
                        row.insert("resolution_kind".into(), Value::String("auto_resolved".into()));
                        row.insert("resolution".into(), Value::String("T001".into()));
                    } else {
                        row.insert("resolution_kind".into(), Value::String("addressed_by_task".into()));
                        row.insert("resolution".into(), Value::String("T001".into()));
                    }
                }
            }
            "architecture_reviews" => {
                row.insert("kind".into(), Value::String(if t.to == "awaiting_human_ratification" { "amend" } else { "interpret" }.into()));
                row.insert("verdict".into(), Value::String(if t.to == "awaiting_human_ratification" { "propose_doctrine_update" } else { "allow_local_fix" }.into()));
                row.insert("source_observation".into(), Value::String("L001".into()));
                row.insert("source_intake".into(), Value::String("I001".into()));
                row.insert("linked_observation_ids".into(), serde_json::json!(["L001"]));
            }
            _ => {}
        }
        row
    }

    #[test]
    fn covers_every_declared_upstream_transition() {
        for path in [
            "stores/intake_items/schema.yaml",
            "stores/observations/schema.yaml",
            "stores/architecture_reviews/schema.yaml",
        ] {
            let schema = schema(path);
            for t in schema.lifecycle.transitions.clone() {
                let mut merged = representative_row(&schema.name, &t);
                let mut diff = crate::validate::EntryMap::new();
                inject_upstream_primary_tuple(
                    &schema,
                    &t,
                    &t.verb,
                    &t.from,
                    &t.to,
                    &mut diff,
                    &mut merged,
                )
                .unwrap_or_else(|e| panic!("{} {} {} -> {}: {e}", schema.name, t.verb, t.from, t.to));
                assert_upstream_tuple_matches_projection(&schema, &merged)
                    .unwrap_or_else(|e| panic!("{} {} {} -> {}: {e}", schema.name, t.verb, t.from, t.to));
            }
        }
    }
}

/// Read the policy_ref/policies_hash env vars set by the autonomous flow
/// daemon (Phase 5: agents_run.rs::run_dispatch). When unset (the manual CLI
/// path), returns `(None, None)` so transition_history records NULL — the
/// distinct sentinel for "manual transition" per AC5.4.
pub(crate) fn inject_tasks_overlay_into_diff(
    schema: &Schema,
    verb: &str,
    from: &str,
    to: &str,
    diff: &mut crate::validate::EntryMap,
    merged: &mut crate::validate::EntryMap,
) -> Result<()> {
    inject_tasks_overlay_into_diff_for_transition(schema, None, verb, from, to, diff, merged)
}

pub(crate) fn inject_tasks_overlay_into_diff_for_transition(
    schema: &Schema,
    transition: Option<&Transition>,
    verb: &str,
    from: &str,
    to: &str,
    diff: &mut crate::validate::EntryMap,
    merged: &mut crate::validate::EntryMap,
) -> Result<()> {
    if schema.name != "tasks" {
        return Ok(());
    }
    let overlay = if let Some(t) = transition {
        if let (Some(lifecycle), Some(active_step), Some(integration_step), Some(blocked)) = (
            t.lifecycle.as_ref(),
            t.active_step.as_ref(),
            t.integration_step.as_ref(),
            t.blocked,
        ) {
            crate::handlers::lifecycle_overlay::LifecycleOverlay {
                lifecycle: lifecycle.clone(),
                active_step: active_step.clone(),
                integration_step: integration_step.clone(),
                blocked,
                blocker_kind: t.blocker_kind.clone(),
                legacy_status: t.legacy_status.clone(),
            }
        } else {
            crate::handlers::lifecycle_overlay::derive(
                verb,
                from,
                to,
                merged.get("blocked_reason").and_then(|v| v.as_str()),
                merged
                    .get("integration_blocked_reason")
                    .and_then(|v| v.as_str()),
            )?
        }
    } else {
        crate::handlers::lifecycle_overlay::derive(
            verb,
            from,
            to,
            merged.get("blocked_reason").and_then(|v| v.as_str()),
            merged
                .get("integration_blocked_reason")
                .and_then(|v| v.as_str()),
        )?
    };
    let mut fields = vec![
        ("lifecycle", Value::String(overlay.lifecycle)),
        ("active_step", Value::String(overlay.active_step)),
        ("integration_step", Value::String(overlay.integration_step)),
        ("blocked", Value::Bool(overlay.blocked)),
        (
            "blocker_kind",
            overlay
                .blocker_kind
                .map(Value::String)
                .unwrap_or(Value::Null),
        ),
    ];
    let post_integration_step = match verb {
        "mark_cargo_installed" => Some("cargo_installed"),
        "mark_schema_migrated" => Some("schema_migrated"),
        "mark_deploy_blocked" => Some("deploy_blocked"),
        _ => None,
    };
    if let Some(step) = post_integration_step {
        fields.push(("post_integration_step", Value::String(step.to_string())));
    }
    for (k, v) in fields {
        diff.insert(k.to_string(), v.clone());
        merged.insert(k.to_string(), v);
    }
    Ok(())
}

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

    inject_upstream_primary_tuple(
        schema,
        transition,
        "close_as_addressed",
        current_status,
        &transition.to,
        &mut diff,
        &mut merged,
    )?;
    inject_tasks_overlay_into_diff(
        schema,
        "close_as_addressed",
        current_status,
        &transition.to,
        &mut diff,
        &mut merged,
    )?;

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
        println!("{display_id} already closed_out_of_band (no-op; commit={commit})");
        return Ok(());
    }

    // 3. Resolve transition (errors if from-state is terminal/disallowed).
    let mut merged = existing.clone();
    let transition = select_transition(
        &schema.lifecycle.transitions,
        current_status,
        "close-out-of-band",
        None,
        &merged,
    )?;

    let mut diff: crate::validate::EntryMap = std::collections::BTreeMap::new();

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

    inject_tasks_overlay_into_diff(
        schema,
        "close-out-of-band",
        current_status,
        &transition.to,
        &mut diff,
        &mut merged,
    )?;

    // 5. Write transition + audit row with SHA in actor_note.
    let now = now_iso8601();
    let invoker_str = invoker.actor.to_string();
    let qtable = quote_ident(&schema.name);
    let live_columns = {
        let mut stmt = tx
            .prepare(&format!("PRAGMA table_info({})", quote_ident(&schema.name)))
            .context("close-out-of-band: inspect live columns")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut cols = Vec::new();
        for row in rows {
            cols.push(row?);
        }
        cols
    };
    let has_overlay_columns = [
        "lifecycle",
        "active_step",
        "integration_step",
        "blocked",
        "blocker_kind",
    ]
    .iter()
    .all(|name| live_columns.iter().any(|c| c == name));
    if has_overlay_columns {
        tx.execute(
            &format!(
                "UPDATE {qtable} SET updated_at = ?1, updated_by = ?2, status = ?3, lifecycle = ?4, active_step = ?5, integration_step = ?6, blocked = ?7, blocker_kind = ?8 WHERE id = ?9"
            ),
            rusqlite::params![
                now,
                invoker_str,
                transition.to,
                merged
                    .get("lifecycle")
                    .and_then(|v| v.as_str())
                    .unwrap_or("active"),
                merged
                    .get("active_step")
                    .and_then(|v| v.as_str())
                    .unwrap_or("none"),
                merged
                    .get("integration_step")
                    .and_then(|v| v.as_str())
                    .unwrap_or("none"),
                if merged
                    .get("blocked")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    1
                } else {
                    0
                },
                merged.get("blocker_kind").and_then(|v| v.as_str()),
                row_id
            ],
        )
        .context("close-out-of-band: update row")?;
    } else {
        tx.execute(
            &format!(
                "UPDATE {qtable} SET updated_at = ?1, updated_by = ?2, status = ?3 WHERE id = ?4"
            ),
            rusqlite::params![now, invoker_str, transition.to, row_id],
        )
        .context("close-out-of-band: update row")?;
    }

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
        .map_err(|e| {
            anyhow::anyhow!(
                "--commit '{sha}' validation failed: could not run git ({e}). \
             close-out-of-band requires a real git repo with a 'main' ref."
            )
        })?;
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

/// Entry point for `abandon` (T043) — walks a stale or duplicate-shipped row to
/// the terminal `abandoned` state without burning a drive cycle. Idempotent on
/// already-abandoned rows. Requires a non-empty reason. Tier-A actor gating
/// (human or token-mediated ai_with_human) is enforced by the schema's actor field on each transition.
pub fn run_abandon(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
    reason: &str,
) -> Result<()> {
    if reason.trim().is_empty() {
        anyhow::bail!("--reason must be a non-empty string");
    }

    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let tx = conn.unchecked_transaction().context("abandon: begin tx")?;

    let (row_id, existing) = read_row(schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Idempotent: already abandoned → no-op success, no second audit row,
    // do not overwrite stored reason.
    if current_status == "abandoned" {
        tx.commit()
            .context("abandon: commit tx (idempotent no-op)")?;
        println!("Already abandoned {display_id}: no-op");
        return Ok(());
    }

    let now = now_iso8601();

    // Build base diff from CLI args (won't include --reason since there's no
    // matching schema field), then inject the abandon-specific fields.
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
        "abandoned_reason".to_string(),
        Value::String(reason.to_string()),
    );
    diff.insert("abandoned_at".to_string(), Value::String(now.clone()));

    let mut merged = existing.clone();
    for (k, v) in &diff {
        merged.insert(k.clone(), v.clone());
    }

    let transition = select_transition(
        &schema.lifecycle.transitions,
        current_status,
        "abandon",
        None,
        &merged,
    )?;

    // Validate caller authority for the lifecycle transition using a diff that
    // excludes framework-owned abandon metadata. Those two fields are written
    // only by this handler's narrow framework-authorized path below.
    let mut caller_diff = diff.clone();
    caller_diff.remove("abandoned_reason");
    caller_diff.remove("abandoned_at");
    validate::validate(
        schema,
        &merged,
        Op::Transition("abandon".to_string(), caller_diff),
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
        "abandon",
        &diff,
        &merged,
        invoker.actor,
        pref.as_deref(),
        phash.as_deref(),
        Some(reason),
    )?;

    tx.commit().context("abandon: commit tx")?;

    println!(
        "Transitioned {display_id}: {} → {} (abandoned)",
        transition.from, transition.to
    );
    Ok(())
}

fn enforce_external_review_accept_precheck(
    tx: &Transaction,
    display_id: &str,
    existing: &crate::validate::EntryMap,
) -> Result<()> {
    let tier = existing
        .get("tier_hint")
        .and_then(Value::as_str)
        .unwrap_or("");
    if tier != "T2" && tier != "T3" {
        return Ok(());
    }

    let table_exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='external_reviews'",
        [],
        |row| row.get(0),
    )?;
    if table_exists == 0 {
        anyhow::bail!("external review PASS required for {display_id}: no external_reviews table");
    }

    let current_head = resolve_accept_head(existing)?;
    let held_expr = if external_review_has_column(tx, "held_reason")? {
        "COALESCE(held_reason,'')"
    } else {
        "''"
    };
    let superseded_filter = if external_review_has_column(tx, "superseded_by")? {
        "AND COALESCE(superseded_by,'') = ''"
    } else {
        ""
    };
    let sql_current = format!(
        "SELECT display_id, COALESCE(status,''), COALESCE(verdict,''), COALESCE(head_sha,''), {held_expr} \
         FROM external_reviews \
         WHERE task_id=?1 {superseded_filter} AND COALESCE(head_sha,'') = ?2 \
         ORDER BY attempt DESC, id DESC LIMIT 1"
    );
    let current = tx
        .query_row(&sql_current, rusqlite::params![display_id, current_head], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .optional()?;

    if let Some((review_id, status, verdict, _head_sha, held_reason)) = current {
        if verdict == "TOOLING_FAILURE" || status == "tooling_held" {
            anyhow::bail!(
                "external review PASS required for {display_id}: current-head attempt {review_id} is TOOLING_FAILURE/held; retry or inspect held external review attempt {review_id} ({held_reason})"
            );
        }
        if status == "passed" && verdict == "PASS" {
            return Ok(());
        }
        anyhow::bail!(
            "external review PASS required for {display_id}: current-head external review attempt {review_id} has status={status} verdict={verdict}"
        );
    }

    let sql_latest = format!(
        "SELECT display_id, COALESCE(status,''), COALESCE(verdict,''), COALESCE(head_sha,''), {held_expr} \
         FROM external_reviews \
         WHERE task_id=?1 {superseded_filter} \
         ORDER BY attempt DESC, id DESC LIMIT 1"
    );
    let latest = tx
        .query_row(&sql_latest, [display_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .optional()?;

    let Some((review_id, status, verdict, head_sha, held_reason)) = latest else {
        anyhow::bail!("external review PASS required for {display_id}: no non-superseded external review attempt found");
    };

    if verdict == "TOOLING_FAILURE" || status == "tooling_held" {
        anyhow::bail!(
            "external review PASS required for {display_id}: no current-head review exists; latest non-superseded attempt {review_id} is TOOLING_FAILURE/held for head {head_sha}, current head is {current_head} ({held_reason})"
        );
    }
    anyhow::bail!(
        "stale external review head for {display_id}: latest non-superseded attempt {review_id} has status={status} verdict={verdict} reviewed head {head_sha}, current head is {current_head}"
    );
}

fn external_review_has_column(tx: &Transaction, name: &str) -> Result<bool> {
    let mut stmt = tx.prepare("PRAGMA table_info(external_reviews)")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let col: String = row.get(1)?;
        if col == name {
            return Ok(true);
        }
    }
    Ok(false)
}

fn resolve_accept_head(existing: &crate::validate::EntryMap) -> Result<String> {
    let workspace = existing
        .get("workspace_path")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(".");
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .current_dir(workspace)
        .output()
        .with_context(|| format!("resolve current HEAD in {workspace}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "external review PASS required but current head could not be resolved in {workspace}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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

    if schema.name == "tasks" && verb == "accept" {
        enforce_external_review_accept_precheck(tx, display_id, &existing)?;
    }

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

    if schema.name == "observations" && verb == "wont_fix" {
        diff.insert("wont_fix_at".to_string(), Value::String(now_iso8601()));
    }
    if schema.name == "observations" && verb == "supersede" {
        diff.insert(
            "resolution_kind".to_string(),
            Value::String("superseded".to_string()),
        );
        diff.insert("resolved_at".to_string(), Value::String(now_iso8601()));
    }
    if schema.name == "tasks" && verb == "accept" {
        let now = now_iso8601();
        diff.insert(
            "acceptance_decided_by".to_string(),
            Value::String("human".to_string()),
        );
        diff.insert("acceptance_decided_at".to_string(), Value::String(now));
    }

    // Deep-merge diff into existing; Record-typed fields get recursive
    // sub-field-level merge.
    let mut merged = existing.clone();
    for (k, v) in &diff {
        let is_record = schema
            .fields
            .iter()
            .any(|f| f.name == *k && matches!(f.ty, crate::schema::FieldType::Record(_)));
        if is_record {
            deep_merge_entry_field(&mut merged, k, v);
        } else {
            merged.insert(k.clone(), v.clone());
        }
    }

    // T077 P3: observations U1 architecture-review gate. A pending
    // architecture review blocks confirmed→ready ratification unless the
    // referenced A### verdict satisfies a typed clearing condition; successful
    // clearance writes pending_architecture_review=false in the same transition.
    if schema.name == "observations" && verb == "ratify" {
        super::observation_arch_gate::enforce_u1_architecture_gate(
            tx,
            display_id,
            &existing,
            &mut merged,
            &mut diff,
        )?;
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
    if schema.name == "intake" && verb == "route" {
        maybe_validate_and_mirror_gatekeeper_decision(schema, &mut diff, &mut merged)?;
        super::intake_route::inject_pre_validation_fields(tx, &mut diff, &mut merged, verb)?;
    }

    // T053 P3/P5: recon-return writes recon_round/evidence through the same typed
    // transition write as the status move, avoiding intake-specific raw writes.
    if schema.name == "intake" && verb == "recon-return" {
        super::intake_route::inject_recon_return_fields(&mut diff, &mut merged)?;
    }

    inject_upstream_primary_tuple(
        schema,
        transition,
        verb,
        current_status,
        &transition.to,
        &mut diff,
        &mut merged,
    )?;

    // Run validator against merged entry; actor checks scoped to diff only.
    let validation_diff = if schema.name == "tasks" && verb == "accept" {
        let mut d = diff.clone();
        d.remove("acceptance_decided_by");
        d.remove("acceptance_decided_at");
        d
    } else {
        strip_framework_overlay_from_validation_diff(schema, &diff)
    };
    validate::validate(
        schema,
        &merged,
        Op::Transition(verb.to_string(), validation_diff),
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

    inject_tasks_overlay_into_diff_for_transition(
        schema,
        Some(transition),
        verb,
        current_status,
        &transition.to,
        &mut diff,
        &mut merged,
    )?;

    // Write: UPDATE merged fields + legacy status projection + updated_*
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
            Some(&existing),
            pref.as_deref(),
            phash.as_deref(),
        )?;
    }

    Ok(())
}

/// T020 P1: post-confirm hook. If the just-confirmed observation's
/// `intent_contract` is `ready` AND has `approved_by` AND `approved_at`
/// populated, fire framework `ratify` (confirmed → ready) atomically in the
/// same caller-supplied transaction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn maybe_auto_ratify_observation(
    tx: &Transaction,
    schema: &Schema,
    row_id: i64,
    display_id: &str,
    merged: &crate::validate::EntryMap,
    persisted_for_gate: Option<&crate::validate::EntryMap>,
    policy_ref: Option<&str>,
    policies_hash: Option<&str>,
) -> Result<()> {
    let intent = match merged.get("intent_contract").and_then(|v| v.as_object()) {
        Some(o) => o,
        None => return Ok(()),
    };
    let contract_ready = intent.get("contract_state").and_then(|v| v.as_str()) == Some("ready");
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

    // Build a no-op diff and enforce the T077 architecture-review U1 gate
    // before resolving the ratify transition. Successful clearance mutates
    // both maps so pending_architecture_review=false is persisted atomically.
    let mut ratify_diff: crate::validate::EntryMap = std::collections::BTreeMap::new();
    let mut ratify_merged = merged.clone();
    let persisted_for_gate = persisted_for_gate.unwrap_or(merged);
    super::observation_arch_gate::enforce_u1_architecture_gate(
        tx,
        display_id,
        persisted_for_gate,
        &mut ratify_merged,
        &mut ratify_diff,
    )?;
    let from_status = "confirmed";
    let transition = select_transition(
        &schema.lifecycle.transitions,
        from_status,
        "ratify",
        None,
        &ratify_merged,
    )?;

    validate::validate(
        schema,
        &ratify_merged,
        Op::Transition("ratify".to_string(), ratify_diff.clone()),
        Actor::Framework.into(),
    )
    .map_err(|errs| {
        anyhow::anyhow!(
            "auto-ratify validation failed:\n{}",
            validate::pretty_print(&errs)
        )
    })?;

    inject_upstream_primary_tuple(
        schema,
        transition,
        "ratify",
        from_status,
        &transition.to,
        &mut ratify_diff,
        &mut ratify_merged,
    )?;
    inject_tasks_overlay_into_diff(
        schema,
        "ratify",
        from_status,
        &transition.to,
        &mut ratify_diff,
        &mut ratify_merged,
    )?;

    execute_transition_write(
        tx,
        schema,
        row_id,
        display_id,
        from_status,
        &transition.to,
        "ratify",
        &ratify_diff,
        &ratify_merged,
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
    let legacy_status = if schema.name == "tasks" {
        let overlay = crate::handlers::lifecycle_overlay::LifecycleOverlay {
            lifecycle: merged
                .get("lifecycle")
                .and_then(Value::as_str)
                .unwrap_or("active")
                .to_string(),
            active_step: merged
                .get("active_step")
                .and_then(Value::as_str)
                .unwrap_or("none")
                .to_string(),
            integration_step: merged
                .get("integration_step")
                .and_then(Value::as_str)
                .unwrap_or("none")
                .to_string(),
            blocked: merged
                .get("blocked")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            blocker_kind: merged
                .get("blocker_kind")
                .and_then(Value::as_str)
                .map(str::to_string),
            legacy_status: Some(new_status.to_string()),
        };
        crate::handlers::lifecycle_overlay::legacy(&overlay)?
    } else {
        new_status.to_string()
    };

    let mut set_parts: Vec<String> = vec![
        "updated_at = ?1".to_string(),
        "updated_by = ?2".to_string(),
        "status = ?3".to_string(),
    ];
    let mut sql_values: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(now),
        rusqlite::types::Value::Text(invoker_str.clone()),
        rusqlite::types::Value::Text(legacy_status.clone()),
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
                _ => match new_val {
                    Value::Null => sql_values.push(rusqlite::types::Value::Null),
                    Value::String(s) => sql_values.push(rusqlite::types::Value::Text(s.clone())),
                    other => sql_values.push(rusqlite::types::Value::Text(other.to_string())),
                },
            }
        }
    }

    let where_param_idx = param_idx;
    sql_values.push(rusqlite::types::Value::Integer(row_id));

    let prior_primary_tuple: Option<(Option<String>, Option<String>, Option<String>)> =
        if schema.name == "tasks" {
            tx.query_row(
                "SELECT lifecycle, active_step, integration_step FROM tasks WHERE id=?1",
                rusqlite::params![row_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .ok()
        } else {
            None
        };

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
        &legacy_status,
        verb,
        &invoker_str,
        policy_ref,
        policies_hash,
        actor_note,
    )?;

    if schema.name == "tasks" {
        let history_cols = {
            let mut stmt = tx.prepare("PRAGMA table_info(transition_history)")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            let mut cols = Vec::new();
            for row in rows {
                cols.push(row?);
            }
            cols
        };
        if [
            "lifecycle_from",
            "active_step_from",
            "integration_step_from",
            "lifecycle_to",
            "active_step_to",
            "integration_step_to",
        ]
        .iter()
        .all(|c| history_cols.iter().any(|h| h == c))
        {
            let derived_from =
                crate::handlers::lifecycle_overlay::derive("history", "", from_status, None, None)?;
            let lifecycle_from = prior_primary_tuple
                .as_ref()
                .and_then(|(lifecycle, _, _)| lifecycle.as_deref())
                .unwrap_or(&derived_from.lifecycle);
            let active_step_from = prior_primary_tuple
                .as_ref()
                .and_then(|(_, active_step, _)| active_step.as_deref())
                .unwrap_or(&derived_from.active_step);
            let integration_step_from = prior_primary_tuple
                .as_ref()
                .and_then(|(_, _, integration_step)| integration_step.as_deref())
                .unwrap_or(&derived_from.integration_step);
            tx.execute(
                "UPDATE transition_history SET lifecycle_from=?1, active_step_from=?2, integration_step_from=?3, lifecycle_to=?4, active_step_to=?5, integration_step_to=?6 WHERE id=last_insert_rowid()",
                rusqlite::params![
                    lifecycle_from,
                    active_step_from,
                    integration_step_from,
                    merged.get("lifecycle").and_then(Value::as_str),
                    merged.get("active_step").and_then(Value::as_str),
                    merged.get("integration_step").and_then(Value::as_str),
                ],
            )
            .context("transition_history primary tuple update")?;
        }
    }

    Ok(())
}

/// T053 P2: validate gatekeeper_decision_json and mirror risk_flags, cluster_key,
/// and decision_metadata into diff + merged so they are written in the same transaction.
///
/// Only fires for the `intake` store on the `route` verb.
/// Called after generic `validate::validate()` passes — the generic validator already
/// rejects badly-formed JSON, so here we know the value is either a parsed object or null.
pub(crate) fn maybe_validate_and_mirror_gatekeeper_decision(
    schema: &crate::schema::Schema,
    diff: &mut crate::validate::EntryMap,
    merged: &mut crate::validate::EntryMap,
) -> anyhow::Result<()> {
    let decision_json_val = match merged.get("gatekeeper_decision_json") {
        Some(v) => v.clone(),
        None => {
            // FIX 2: gatekeeper_decision_json is mandatory for route — reject absent field.
            anyhow::bail!(
                "gatekeeper_decision_json is required for the route verb; \
                 pass --gatekeeper-decision-json with the full gatekeeper output payload"
            );
        }
    };

    // Null means not yet set — reject fail-loud (same as absent).
    if decision_json_val.is_null() {
        anyhow::bail!(
            "gatekeeper_decision_json is required for the route verb and must be a non-null JSON \
             object; pass --gatekeeper-decision-json with the full gatekeeper output payload"
        );
    }

    // The generic JSON validator already caught malformed strings; here we expect an object.
    // If it's a String (sentinel), bail gracefully — the generic validator's error already fired.
    if decision_json_val.is_string() {
        return Ok(());
    }

    // Validate the gatekeeper decision payload through the code-level Check registry.
    let check_args = serde_json::json!({"gatekeeper_decision_json": decision_json_val.clone()});
    let check_result = crate::flow::checks::lookup(crate::flow::checks::GATEKEEPER_DECISION_VALID)
        .ok_or_else(|| anyhow::anyhow!("missing Check: gatekeeper-decision-valid"))?
        .evaluate(crate::flow::checks::CheckCtx::without_conn(), &check_args)?;
    if !check_result.is_pass() {
        anyhow::bail!(
            "{}",
            crate::flow::checks::format_check_failure(&check_result)
        );
    }

    // FIX 3: enforce --decision matches gatekeeper_decision_json.decision (exact equality).
    // Reject fail-loud if they differ to prevent mismatched state transitions.
    if let Some(obj) = decision_json_val.as_object() {
        let json_decision = obj.get("decision").and_then(|v| v.as_str()).unwrap_or("");
        let cli_decision = merged
            .get("decision")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !cli_decision.is_empty() && json_decision != cli_decision {
            anyhow::bail!(
                "--decision '{cli_decision}' does not match gatekeeper_decision_json.decision \
                 '{json_decision}'; they must be identical"
            );
        }
    }

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
        let risk_class = crate::schema::risk_taxonomy::derive_risk_class(&risk_flags_arr);
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
    #[allow(unused_imports)]
    use crate::db;
    use crate::schema::Schema;

    const OBS_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
default_actor: ai_with_human
lifecycle:
  states: [open, triaged, confirmed, resolved, wont_fix]
  transitions:
    - from: open
      to: wont_fix
      verb: wont_fix
      actor: ai_with_human
    - from: confirmed
      to: wont_fix
      verb: wont_fix
      actor: ai_with_human
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
    type: timestamp
    required: false
    actor: ai_autonomous
  - name: wont_fix_at
    type: timestamp
    required: false
    actor: ai_with_human
"#;

    fn overlay_tuple(
        o: crate::handlers::lifecycle_overlay::LifecycleOverlay,
    ) -> (String, String, String, bool, Option<String>) {
        (
            o.lifecycle,
            o.active_step,
            o.integration_step,
            o.blocked,
            o.blocker_kind,
        )
    }

    #[test]
    fn primary_tuple_round_trip() {
        let schema = Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap();
        for t in &schema.lifecycle.transitions {
            let expected =
                crate::handlers::lifecycle_overlay::derive(&t.verb, &t.from, &t.to, None, None)
                    .unwrap();
            let got = crate::handlers::lifecycle_overlay::LifecycleOverlay {
                lifecycle: t.lifecycle.clone().expect("transition lifecycle"),
                active_step: t.active_step.clone().expect("transition active_step"),
                integration_step: t
                    .integration_step
                    .clone()
                    .expect("transition integration_step"),
                blocked: t.blocked.expect("transition blocked"),
                blocker_kind: t.blocker_kind.clone(),
                legacy_status: t.legacy_status.clone(),
            };
            assert_eq!(
                overlay_tuple(got),
                overlay_tuple(expected),
                "{} {} -> {}",
                t.verb,
                t.from,
                t.to
            );
        }
    }

    #[test]
    fn adr0002_primary_tuple_invariant_happy_path() {
        let schema =
            Schema::from_yaml(include_str!("../../stores/intake_items/schema.yaml")).unwrap();
        let mut entry = crate::validate::EntryMap::new();
        entry.insert("status".into(), Value::String("routed".into()));
        entry.insert(
            "decision".into(),
            Value::String("normal_observation".into()),
        );
        entry.insert("routed_to_observation".into(), Value::String("L001".into()));
        entry.insert("produced_observation_id".into(), Value::String("L001".into()));
        entry.insert("lifecycle".into(), Value::String("closed".into()));
        entry.insert(
            "outcome".into(),
            Value::String("routed_to_observation".into()),
        );
        entry.insert("waiting_kind".into(), Value::Null);
        assert_upstream_tuple_matches_projection(&schema, &entry).unwrap();
    }

    #[test]
    fn adr0002_primary_tuple_invariant_names_disagreement_field() {
        let schema =
            Schema::from_yaml(include_str!("../../stores/intake_items/schema.yaml")).unwrap();
        let mut entry = crate::validate::EntryMap::new();
        entry.insert("status".into(), Value::String("routed".into()));
        entry.insert(
            "decision".into(),
            Value::String("normal_observation".into()),
        );
        entry.insert("routed_to_observation".into(), Value::String("L001".into()));
        entry.insert("produced_observation_id".into(), Value::String("L001".into()));
        entry.insert("lifecycle".into(), Value::String("waiting".into()));
        entry.insert(
            "outcome".into(),
            Value::String("routed_to_observation".into()),
        );
        let err = assert_upstream_tuple_matches_projection(&schema, &entry).unwrap_err();
        assert!(err.to_string().contains("field lifecycle"), "{err}");
    }

    #[test]
    fn observations_supersede_writes_primary_tuple_and_reference() {
        let schema =
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();
        conn.execute(
            "INSERT INTO observations (display_id,status,created_at,updated_at,created_by,updated_by,summary,source,priority,captured_at,captured_week) VALUES ('L001','open','now','now','ai_with_human','ai_with_human','s','dev','normal','2026-05-11T00:00:00Z','w20-d1')",
            [],
        ).unwrap();
        let cmd = build_cmd(&schema, "supersede");
        let matches = cmd.get_matches_from(vec!["supersede", "L001", "--superseded-by-id", "L002"]);
        let tx = conn.unchecked_transaction().unwrap();
        run_in_tx(
            &tx,
            &schema,
            &matches,
            Actor::AiWithHuman.into(),
            "supersede",
        )
        .unwrap();
        tx.commit().unwrap();
        let (status, lifecycle, outcome, superseded_by_id, resolution_kind): (String, String, String, String, String) = conn.query_row(
            "SELECT status,lifecycle,outcome,superseded_by_id,resolution_kind FROM observations WHERE display_id='L001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).unwrap();
        assert_eq!(status, "resolved");
        assert_eq!(lifecycle, "closed");
        assert_eq!(outcome, "superseded");
        assert_eq!(superseded_by_id, "L002");
        assert_eq!(resolution_kind, "superseded");
    }

    #[test]
    fn observations_contract_state_tracks_intent_contract_ready_alias() {
        let schema =
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();
        conn.execute(
            "INSERT INTO observations (display_id,status,created_at,updated_at,created_by,updated_by,summary,source,priority,captured_at,captured_week,intent_contract,contract_state) VALUES ('L001','investigating','now','now','ai_with_human','ai_with_human','s','dev','normal','2026-05-11T00:00:00Z','w20-d1',?1,'draft')",
            [serde_json::json!({"contract_state":"draft"}).to_string()],
        ).unwrap();
        let cmd = build_cmd(&schema, "confirm");
        let matches = cmd.get_matches_from(vec![
            "confirm",
            "L001",
            "--contract-state",
            "ready",
            "--approved-by",
            "blake",
            "--approved-at",
            "2026-05-11T00:00:00Z",
            "--objective",
            "approve contract",
            "--type",
            "work",
            "--in-scope",
            "scope",
            "--out-of-scope",
            "none",
            "--acceptance",
            "done",
            "--tier-hint",
            "T1",
        ]);
        let tx = conn.unchecked_transaction().unwrap();
        run_in_tx(&tx, &schema, &matches, Actor::Human.into(), "confirm").unwrap();
        tx.commit().unwrap();
        let contract_state: String = conn
            .query_row(
                "SELECT contract_state FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(contract_state, "approved");
    }

    #[test]
    fn legacy_projection_round_trip() {
        let statuses = [
            "planning",
            "plan_review",
            "ready",
            "executing",
            "code_review",
            "blocked",
            "complete",
            "in_review",
            "accepted",
            "rejected",
            "deploy_blocked",
            "integration_queued",
            "integrating",
            "integration_blocked",
            "integrated",
            "cargo_installed",
            "schema_migrated",
            "closed_out_of_band",
            "abandoned",
        ];
        for status in statuses {
            let overlay =
                crate::handlers::lifecycle_overlay::derive("test", "", status, None, None).unwrap();
            assert_eq!(
                crate::handlers::lifecycle_overlay::legacy(&overlay).unwrap(),
                status
            );
        }
    }

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

    fn init_git_repo_at_head(head_marker: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), head_marker).unwrap();
        std::process::Command::new("git").args(["init", "-q"]).current_dir(dir.path()).status().unwrap();
        std::process::Command::new("git").args(["config", "user.email", "test@example.com"]).current_dir(dir.path()).status().unwrap();
        std::process::Command::new("git").args(["config", "user.name", "Test"]).current_dir(dir.path()).status().unwrap();
        std::process::Command::new("git").args(["add", "file.txt"]).current_dir(dir.path()).status().unwrap();
        std::process::Command::new("git").args(["commit", "-q", "-m", "init"]).current_dir(dir.path()).status().unwrap();
        let out = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(dir.path()).output().unwrap();
        (dir, String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    fn accept_precheck_entry(workspace: &std::path::Path) -> crate::validate::EntryMap {
        let mut entry = crate::validate::EntryMap::new();
        entry.insert("display_id".into(), serde_json::json!("T900"));
        entry.insert("tier_hint".into(), serde_json::json!("T2"));
        entry.insert(
            "workspace_path".into(),
            serde_json::json!(workspace.to_string_lossy().to_string()),
        );
        entry
    }

    fn create_external_reviews_for_accept(conn: &Connection) {
        conn.execute_batch(
            r#"
            CREATE TABLE external_reviews (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                display_id TEXT,
                task_id TEXT,
                attempt INTEGER,
                status TEXT,
                verdict TEXT,
                head_sha TEXT,
                held_reason TEXT,
                superseded_by TEXT
            );
            "#,
        )
        .unwrap();
    }

    #[test]
    fn accept_precheck_rejects_stale_pass_head() {
        let conn = Connection::open_in_memory().unwrap();
        create_external_reviews_for_accept(&conn);
        let (repo, current_head) = init_git_repo_at_head("current");
        let stale_head = "0000000000000000000000000000000000000000";
        conn.execute(
            "INSERT INTO external_reviews (display_id, task_id, attempt, status, verdict, head_sha) \
             VALUES ('ER900', 'T900', 1, 'passed', 'PASS', ?1)",
            [stale_head],
        )
        .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let entry = accept_precheck_entry(repo.path());
        let err = enforce_external_review_accept_precheck(&tx, "T900", &entry).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("stale external review head"), "{msg}");
        assert!(msg.contains(&current_head), "{msg}");
    }

    #[test]
    fn accept_precheck_accepts_current_pass_despite_old_stale_rows() {
        let conn = Connection::open_in_memory().unwrap();
        create_external_reviews_for_accept(&conn);
        let (repo, current_head) = init_git_repo_at_head("current");
        let stale_head = "0000000000000000000000000000000000000000";
        conn.execute(
            "INSERT INTO external_reviews (display_id, task_id, attempt, status, verdict, head_sha, held_reason) \
             VALUES ('ER901', 'T900', 1, 'tooling_held', 'TOOLING_FAILURE', ?1, 'stale_base_requires_rebase')",
            [stale_head],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO external_reviews (display_id, task_id, attempt, status, verdict, head_sha) \
             VALUES ('ER902', 'T900', 2, 'revise', 'REVISE', ?1)",
            [stale_head],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO external_reviews (display_id, task_id, attempt, status, verdict, head_sha) \
             VALUES ('ER903', 'T900', 3, 'passed', 'PASS', ?1)",
            [&current_head],
        )
        .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let entry = accept_precheck_entry(repo.path());
        enforce_external_review_accept_precheck(&tx, "T900", &entry).unwrap();
    }

    #[test]
    fn accept_precheck_blocks_current_tooling_held() {
        let conn = Connection::open_in_memory().unwrap();
        create_external_reviews_for_accept(&conn);
        let (repo, current_head) = init_git_repo_at_head("current");
        conn.execute(
            "INSERT INTO external_reviews (display_id, task_id, attempt, status, verdict, head_sha, held_reason) \
             VALUES ('ER904', 'T900', 1, 'tooling_held', 'TOOLING_FAILURE', ?1, 'stale_base_requires_rebase')",
            [&current_head],
        )
        .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let entry = accept_precheck_entry(repo.path());
        let err = enforce_external_review_accept_precheck(&tx, "T900", &entry).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("current-head attempt ER904 is TOOLING_FAILURE/held"), "{msg}");
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

    #[test]
    fn observations_wont_fix_from_open_sets_wont_fix_at_only() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_cmd(&schema, "wont_fix");
        let matches = cmd.get_matches_from(["wont_fix", "L001"]);
        run(
            &schema,
            &conn,
            &matches,
            Actor::AiWithHuman.into(),
            "wont_fix",
        )
        .unwrap();

        let (_, entry) = crate::handlers::row::read_row(&schema, &conn, "L001").unwrap();
        assert_eq!(
            entry.get("status").and_then(|v| v.as_str()),
            Some("wont_fix")
        );
        assert!(entry
            .get("wont_fix_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .ends_with('Z'));
        assert!(matches!(entry.get("resolved_at"), None | Some(Value::Null)));
    }

    #[test]
    fn observations_wont_fix_from_confirmed_sets_wont_fix_at_only() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);
        conn.execute(
            "UPDATE observations SET status = 'confirmed' WHERE display_id = 'L001'",
            [],
        )
        .unwrap();

        let cmd = build_cmd(&schema, "wont_fix");
        let matches = cmd.get_matches_from(["wont_fix", "L001"]);
        run(
            &schema,
            &conn,
            &matches,
            Actor::AiWithHuman.into(),
            "wont_fix",
        )
        .unwrap();

        let (_, entry) = crate::handlers::row::read_row(&schema, &conn, "L001").unwrap();
        assert_eq!(
            entry.get("status").and_then(|v| v.as_str()),
            Some("wont_fix")
        );
        assert!(entry
            .get("wont_fix_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .ends_with('Z'));
        assert!(matches!(entry.get("resolved_at"), None | Some(Value::Null)));
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

        let (lifecycle_to, active_step_to, integration_step_to): (String, String, String) = conn
            .query_row(
                "SELECT lifecycle_to, active_step_to, integration_step_to FROM transition_history \
                 WHERE store='tasks' AND display_id='T001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (
                lifecycle_to.as_str(),
                active_step_to.as_str(),
                integration_step_to.as_str()
            ),
            ("done", "none", "none")
        );
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
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            rows.len(),
            2,
            "expected 2 transition_history rows; got {rows:?}"
        );
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
  states: [planning, executing, accepted, deploy_blocked, closed_out_of_band]
  transitions:
    - {from: planning, to: closed_out_of_band, verb: close-out-of-band, actor: human}
    - {from: executing, to: closed_out_of_band, verb: close-out-of-band, actor: human}
    - {from: deploy_blocked, to: closed_out_of_band, verb: close-out-of-band, actor: human}
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
            .arg(clap::Arg::new("commit").long("commit").required(true))
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
        let sha = String::from_utf8(rev.stdout).unwrap().trim().to_string();
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
        let m = build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", bogus]);
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
        Guard { prev, _lock: lock }
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

    #[test]
    fn coob_deploy_blocked_to_closed_records_sha_and_no_dispatch_work() {
        let (dir, sha) = coob_real_git_repo();
        let _g = scoped_cwd(dir.path());
        let (schema, conn) = setup_coob();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        insert_coob_row(&conn, "deploy_blocked");
        let m = build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", &sha]);
        run_close_out_of_band(&schema, &conn, &m, Actor::Human.into(), &sha).unwrap();
        assert_eq!(read_status(&conn), "closed_out_of_band");
        let audit = audit_rows_for(&conn);
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].0, "close-out-of-band");
        assert_eq!(audit[0].2.as_deref(), Some(sha.as_str()));
        let dispatches: i64 = conn
            .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            dispatches, 0,
            "manual close-out must not enqueue deploy subscribers"
        );

        let m2 = build_coob_cmd().get_matches_from(["close-out-of-band", "T001", "--commit", &sha]);
        run_close_out_of_band(&schema, &conn, &m2, Actor::Human.into(), &sha).unwrap();
        assert_eq!(read_status(&conn), "closed_out_of_band");
        assert_eq!(
            audit_rows_for(&conn).len(),
            1,
            "repeat remains terminal/idempotent"
        );
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
            err.to_string().contains("validation failed") || err.to_string().contains("actor"),
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

    // ---- T043: abandon verb tests ----

    /// Schema mirroring the production tasks shape (subset): the 8 non-terminal
    /// states allowed to abandon, plus the relevant terminal states for
    /// refusal tests, the abandoned terminal, and the 8 abandon transitions.
    const ABANDON_SCHEMA: &str = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [planning, plan_review, ready, executing, code_review, blocked, in_review, deploy_blocked, accepted, rejected, complete, cargo_installed, schema_migrated, closed_out_of_band, abandoned]
  transitions:
    - {from: planning, to: abandoned, verb: abandon, actor: human}
    - {from: plan_review, to: abandoned, verb: abandon, actor: human}
    - {from: ready, to: abandoned, verb: abandon, actor: human}
    - {from: executing, to: abandoned, verb: abandon, actor: human}
    - {from: code_review, to: abandoned, verb: abandon, actor: human}
    - {from: blocked, to: abandoned, verb: abandon, actor: human}
    - {from: in_review, to: abandoned, verb: abandon, actor: human}
    - {from: deploy_blocked, to: abandoned, verb: abandon, actor: human}
    - {from: complete, to: abandoned, verb: abandon, actor: human}
fields:
  - {name: title, type: text, required: true}
  - {name: abandoned_reason, type: text, required: false, actor: framework}
  - {name: abandoned_at, type: timestamp, required: false, actor: framework}
"#;

    fn setup_abandon() -> (Schema, Connection) {
        let schema = Schema::from_yaml(ABANDON_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::codegen::ddl::SUBSTRATE_DDL)
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();
        (schema, conn)
    }

    fn insert_abandon_row(conn: &Connection, display_id: &str, status: &str) {
        conn.execute(
            "INSERT INTO tasks (display_id, status, title) VALUES (?1, ?2, 'Test')",
            rusqlite::params![display_id, status],
        )
        .unwrap();
    }

    fn build_abandon_cmd(schema: &Schema) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd =
            clap::Command::new("abandon").arg(clap::Arg::new("display_id").required(true).index(1));
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

    fn read_abandon_row(
        conn: &Connection,
        display_id: &str,
    ) -> (String, Option<String>, Option<String>) {
        conn.query_row(
            "SELECT status, abandoned_reason, abandoned_at FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| Ok((r.get(0).unwrap(), r.get(1).ok(), r.get(2).ok())),
        )
        .unwrap()
    }

    /// AC1.3: abandon from each of the 9 allowed non-terminal states succeeds
    /// and writes abandoned_reason + abandoned_at. `complete` is included because
    /// it is transient (has an outgoing framework `request_review` edge), not a
    /// successful deployment terminal.
    #[test]
    fn abandon_from_each_allowed_state_succeeds() {
        for (idx, from_state) in [
            "planning",
            "plan_review",
            "ready",
            "executing",
            "code_review",
            "blocked",
            "in_review",
            "deploy_blocked",
            "complete",
        ]
        .iter()
        .enumerate()
        {
            let (schema, conn) = setup_abandon();
            let id = format!("T{:03}", idx + 1);
            insert_abandon_row(&conn, &id, from_state);

            let cmd = build_abandon_cmd(&schema);
            let matches = cmd.get_matches_from(["abandon", &id, "--reason", "stale"]);
            run_abandon(&schema, &conn, &matches, Actor::Human.into(), "stale").unwrap();

            let (status, reason, at) = read_abandon_row(&conn, &id);
            assert_eq!(
                status, "abandoned",
                "from {from_state} must land at abandoned"
            );
            assert_eq!(reason.as_deref(), Some("stale"));
            assert!(
                at.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
                "abandoned_at must be populated for {from_state}; got: {at:?}"
            );
        }
    }

    /// AC1.4: abandon from successful-terminal deployment states is refused.
    /// `complete` is intentionally NOT in this list — it is transient (with an
    /// outgoing framework `request_review` edge) and IS abandonable, covered
    /// by `abandon_from_each_allowed_state_succeeds`.
    #[test]
    fn abandon_from_terminal_states_refused() {
        for from_state in [
            "accepted",
            "rejected",
            "cargo_installed",
            "schema_migrated",
            "closed_out_of_band",
        ] {
            let (schema, conn) = setup_abandon();
            insert_abandon_row(&conn, "T001", from_state);

            let cmd = build_abandon_cmd(&schema);
            let matches = cmd.get_matches_from(["abandon", "T001", "--reason", "x"]);
            let err = run_abandon(&schema, &conn, &matches, Actor::Human.into(), "x").unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("abandon")
                    || msg.contains("no transition")
                    || msg.contains(from_state),
                "expected no-transition error from {from_state}; got: {msg}"
            );
            // Row unchanged
            let (status, reason, _at) = read_abandon_row(&conn, "T001");
            assert_eq!(
                status, from_state,
                "row must be unchanged from {from_state}"
            );
            assert!(
                reason.is_none(),
                "abandoned_reason must remain null after rejected call"
            );
        }
    }

    /// AC1.5: idempotent — running abandon on an already-abandoned row is a no-op.
    /// Second call must NOT add another transition_history row and must NOT
    /// overwrite the original reason.
    #[test]
    fn abandon_idempotent_on_already_abandoned() {
        let (schema, conn) = setup_abandon();
        insert_abandon_row(&conn, "T001", "ready");

        let cmd = build_abandon_cmd(&schema);
        let matches = cmd.get_matches_from(["abandon", "T001", "--reason", "first"]);
        run_abandon(&schema, &conn, &matches, Actor::Human.into(), "first").unwrap();

        // Second call: should be a no-op (early return).
        let cmd2 = build_abandon_cmd(&schema);
        let matches2 = cmd2.get_matches_from(["abandon", "T001", "--reason", "second"]);
        run_abandon(&schema, &conn, &matches2, Actor::Human.into(), "second").unwrap();

        let (status, reason, _at) = read_abandon_row(&conn, "T001");
        assert_eq!(status, "abandoned");
        assert_eq!(
            reason.as_deref(),
            Some("first"),
            "second call must not overwrite stored reason"
        );

        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history \
                 WHERE store='tasks' AND display_id='T001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "second call must not duplicate audit rows");
    }

    /// AC1.7: empty --reason rejected.
    #[test]
    fn abandon_empty_reason_rejected() {
        let (schema, conn) = setup_abandon();
        insert_abandon_row(&conn, "T001", "ready");

        let cmd = build_abandon_cmd(&schema);
        let matches = cmd.get_matches_from(["abandon", "T001", "--reason", "   "]);
        let err = run_abandon(&schema, &conn, &matches, Actor::Human.into(), "   ").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("non-empty"),
            "expected non-empty-reason error; got: {msg}"
        );
        // Row unchanged
        let (status, _, _) = read_abandon_row(&conn, "T001");
        assert_eq!(status, "ready");
    }

    /// AC1.6: tier-A enforcement — ai_autonomous invoker is rejected by the
    /// schema's actor: human gate, even with a valid token.
    #[test]
    fn abandon_ai_autonomous_invoker_rejected() {
        let (schema, conn) = setup_abandon();
        insert_abandon_row(&conn, "T001", "ready");

        let cmd = build_abandon_cmd(&schema);
        let matches = cmd.get_matches_from(["abandon", "T001", "--reason", "x"]);
        let err = run_abandon(
            &schema,
            &conn,
            &matches,
            InvokerCtx {
                actor: Actor::AiAutonomous,
                token_valid: true,
            },
            "x",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("human"),
            "expected error citing required actor 'human'; got: {msg}"
        );
        assert!(
            msg.contains("ai_autonomous"),
            "expected error citing invoker 'ai_autonomous'; got: {msg}"
        );
        // Row unchanged
        let (status, _, _) = read_abandon_row(&conn, "T001");
        assert_eq!(status, "ready");
    }

    #[test]
    fn abandon_ai_with_human_requires_valid_token() {
        let (schema, conn) = setup_abandon();
        insert_abandon_row(&conn, "T001", "ready");
        let cmd = build_abandon_cmd(&schema);
        let matches = cmd.get_matches_from(["abandon", "T001", "--reason", "x"]);

        let err =
            run_abandon(&schema, &conn, &matches, Actor::AiWithHuman.into(), "x").unwrap_err();
        assert!(err.to_string().contains("--approve-token"), "{err}");
        let (status, _, _) = read_abandon_row(&conn, "T001");
        assert_eq!(status, "ready");

        run_abandon(
            &schema,
            &conn,
            &matches,
            InvokerCtx {
                actor: Actor::AiWithHuman,
                token_valid: true,
            },
            "x",
        )
        .unwrap();
        let (status, reason, _) = read_abandon_row(&conn, "T001");
        assert_eq!(status, "abandoned");
        assert_eq!(reason.as_deref(), Some("x"));
    }

    /// AC1.3 (audit): abandon writes a transition_history row with verb=abandon
    /// and invoker matching the caller. Walk-through covers T032 (audit).
    #[test]
    fn abandon_writes_transition_history_audit_row() {
        let (schema, conn) = setup_abandon();
        insert_abandon_row(&conn, "T001", "blocked");

        let cmd = build_abandon_cmd(&schema);
        let matches = cmd.get_matches_from(["abandon", "T001", "--reason", "duplicate-shipped"]);
        run_abandon(
            &schema,
            &conn,
            &matches,
            Actor::Human.into(),
            "duplicate-shipped",
        )
        .unwrap();

        let (from_status, to_status, verb, invoker, actor_note): (
            String,
            String,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT from_status, to_status, verb, invoker, actor_note FROM transition_history \
                 WHERE store='tasks' AND display_id='T001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(from_status, "blocked");
        assert_eq!(to_status, "abandoned");
        assert_eq!(verb, "abandon");
        assert_eq!(invoker, "human");
        assert_eq!(actor_note, "duplicate-shipped");
    }

    /// T034 lifecycle walk-through: `abandoned` is terminal — there is no
    /// outgoing transition declared from it.
    #[test]
    fn abandoned_is_terminal_no_outgoing_transitions() {
        let schema = Schema::from_yaml(ABANDON_SCHEMA).unwrap();
        let outgoing: Vec<_> = schema
            .lifecycle
            .transitions
            .iter()
            .filter(|t| t.from == "abandoned")
            .collect();
        assert!(
            outgoing.is_empty(),
            "abandoned must be terminal; found outgoing: {outgoing:?}"
        );
        assert!(
            schema.lifecycle.states.iter().any(|s| s == "abandoned"),
            "abandoned must be a declared lifecycle state"
        );
    }
}
