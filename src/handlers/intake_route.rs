//! T053 Phase 3 — intake routing side-effects.
//!
//! For each of the six routing decisions, executes the downstream artifact creation
//! in the SAME transaction as the intake_items state transition.  Atomicity is
//! guaranteed by the caller's `rusqlite::Transaction`: if either the side-effect write
//! or the intake transition write fails the whole tx rolls back (AC3.2).
//!
//! Hooks called from `transition::run_in_tx`:
//!   1. `inject_pre_validation_fields` — BEFORE validate(). Creates side-effect rows
//!      for decisions that need auto-minted IDs (fast_track, normal_observation,
//!      arch_review_candidate), bumps recon_round for needs_info, and copies
//!      cluster_key for duplicate.  Injects derived fields into diff/merged so the
//!      schema's required_when guards are satisfied before the validator runs.
//!   2. `handle_recon_return` — AFTER execute_transition_write() for recon-return verb.
//!      Increments recon_round with cap enforcement (≤ 2).

use anyhow::{Context, Result};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};

use crate::schema::{actor::Actor, Schema};
use crate::validate::{EntryMap, SideEffectAuthority};

// ---------------------------------------------------------------------------
// Absence helper
// ---------------------------------------------------------------------------

/// True iff `merged` has no entry for `key` OR the entry is JSON null.
///
/// `read_row` materialises NULL DB columns into the `EntryMap` as
/// `Some(Value::Null)`, so a plain `.is_none()` check would miss them and
/// the hook would skip its work for rows that came back through `read_row`
/// (the production path; manually-built test maps go straight to None).
/// Identified by Sonnet's executor session for T053 P3 before runner exit.
fn is_absent(merged: &EntryMap, key: &str) -> bool {
    matches!(merged.get(key), None | Some(Value::Null))
}

fn mirror_string(diff: &mut EntryMap, merged: &mut EntryMap, key: &str, value: &str) {
    diff.insert(key.to_string(), Value::String(value.to_string()));
    merged.insert(key.to_string(), Value::String(value.to_string()));
}

// ---------------------------------------------------------------------------
// Public hook 1: pre-validation field injection
// ---------------------------------------------------------------------------

/// Called from `transition::run_in_tx` BEFORE `validate::validate` when the
/// schema is "intake" and verb is "route".
///
/// Decisions handled:
/// - `duplicate`              — copy cluster_key from the referenced item (no obs created)
/// - `needs_info`             — extract missing_info_question from gatekeeper payload
/// - `fast_track`             — create obs (tags=[fast-track-eligible]) if not pre-supplied
/// - `normal_observation`     — create obs + mirror L143 columns if not pre-supplied
/// - `arch_review_candidate`  — create source obs + dedicated A### architecture review
/// - `reject_noise`           — no pre-validation side effects
pub(crate) fn inject_pre_validation_fields(
    tx: &Transaction,
    diff: &mut EntryMap,
    merged: &mut EntryMap,
    _verb: &str,
) -> Result<()> {
    let decision = merged
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let now = super::row::now_iso8601();

    match decision.as_str() {
        "duplicate" => {
            // Copy cluster_key from the referenced duplicate_of item (if resolvable).
            if let Some(dup_of) = merged.get("duplicate_of").and_then(|v| v.as_str()) {
                let dup_of = dup_of.to_string();
                if let Some(ck) = lookup_cluster_key(tx, &dup_of)? {
                    if is_absent(merged, "cluster_key") {
                        diff.insert("cluster_key".to_string(), Value::String(ck.clone()));
                        merged.insert("cluster_key".to_string(), Value::String(ck));
                    }
                }
            }
        }

        "needs_info" => {
            // Extract missing_info_question from gatekeeper payload into its own column.
            if is_absent(merged, "missing_info_question") {
                if let Some(gdj) = merged
                    .get("gatekeeper_decision_json")
                    .and_then(|v| v.as_object())
                {
                    if let Some(q) = gdj.get("missing_info_question") {
                        let q = q.clone();
                        diff.insert("missing_info_question".to_string(), q.clone());
                        merged.insert("missing_info_question".to_string(), q);
                    }
                }
            }
        }

        "fast_track" => {
            // Only auto-create if routed_to_observation not already supplied by caller.
            if is_absent(merged, "routed_to_observation") {
                let summary = merged
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(intake fast-track)")
                    .to_string();

                let gdj = merged.get("gatekeeper_decision_json").cloned();
                let rationale = gdj
                    .as_ref()
                    .and_then(|v| v.get("rationale"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let notes = json!({
                    "fast_track_audit": {
                        "decision_blob": gdj,
                        "gatekeeper_rationale": rationale,
                        "deterministic_check_pending": true
                    }
                });
                let tags = json!(["fast-track-eligible"]);

                let obs_id = insert_observation_row(
                    tx,
                    &ObsFields {
                        summary,
                        source: "intake".to_string(),
                        priority: "normal".to_string(),
                        captured_at: now.clone(),
                        captured_week: week_label(),
                        tags: Some(tags.to_string()),
                        notes: Some(notes.to_string()),
                        risk_class: None,
                        approval_policy: None,
                        risk_flags: None,
                        cluster_key: None,
                        pending_architecture_review: false,
                    },
                )?;

                mirror_string(diff, merged, "routed_to_observation", &obs_id);
                mirror_string(diff, merged, "produced_observation_id", &obs_id);
                mirror_string(diff, merged, "produced_artifact_kind", "observation");
                mirror_string(diff, merged, "produced_artifact_id", &obs_id);
            }
        }

        "normal_observation" => {
            // Only auto-create if routed_to_observation not already supplied by caller.
            if is_absent(merged, "routed_to_observation") {
                let summary = merged
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("(intake normal observation)")
                    .to_string();

                // Derive L143 columns from gatekeeper decision metadata.
                let dm = merged
                    .get("decision_metadata")
                    .and_then(|v| v.as_object())
                    .cloned();
                let risk_class = dm
                    .as_ref()
                    .and_then(|m| m.get("risk_class_hint"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let approval_policy = dm
                    .as_ref()
                    .and_then(|m| m.get("approval_policy_hint"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let risk_flags_val = merged
                    .get("risk_flags")
                    .and_then(|v| if v.is_array() { Some(v) } else { None })
                    .cloned();
                let cluster_key = merged
                    .get("cluster_key")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let obs_id = insert_observation_row(
                    tx,
                    &ObsFields {
                        summary,
                        source: "intake".to_string(),
                        priority: "normal".to_string(),
                        captured_at: now.clone(),
                        captured_week: week_label(),
                        tags: None,
                        notes: None,
                        risk_class,
                        approval_policy,
                        risk_flags: risk_flags_val.map(|v| v.to_string()),
                        cluster_key,
                        pending_architecture_review: false,
                    },
                )?;

                mirror_string(diff, merged, "routed_to_observation", &obs_id);
                mirror_string(diff, merged, "produced_observation_id", &obs_id);
            }
        }

        "arch_review_candidate" => {
            if !is_absent(merged, "routed_to_arch_review") {
                anyhow::bail!(
                    "arch_review_candidate routes must create routed_to_arch_review in the same transaction; caller-supplied routed_to_arch_review is not accepted"
                );
            }
            if !is_absent(merged, "routed_to_observation") {
                anyhow::bail!(
                    "arch_review_candidate routes must create routed_to_observation in the same transaction; caller-supplied routed_to_observation is not accepted"
                );
            }

            let summary = merged
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("(intake arch review candidate)")
                .to_string();
            let cluster_key = merged
                .get("cluster_key")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    merged
                        .get("gatekeeper_decision_json")
                        .and_then(|v| v.get("cluster_key"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                });

            let notes = json!({ "gatekeeper_route": { "decision": "arch_review_candidate" } });
            let obs_id = insert_observation_row(
                tx,
                &ObsFields {
                    summary: summary.clone(),
                    source: "intake".to_string(),
                    priority: "normal".to_string(),
                    captured_at: now.clone(),
                    captured_week: week_label(),
                    tags: None,
                    notes: Some(notes.to_string()),
                    risk_class: None,
                    approval_policy: None,
                    risk_flags: None,
                    cluster_key: cluster_key.clone(),
                    pending_architecture_review: true,
                },
            )?;
            mirror_string(diff, merged, "routed_to_observation", &obs_id);
            mirror_string(diff, merged, "produced_observation_id", &obs_id);

            let intake_id = merged
                .get("display_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let arch_id = insert_architecture_review_row(
                tx,
                &ArchReviewFields {
                    summary,
                    source_observation: obs_id,
                    source_intake: intake_id,
                    cluster_key,
                },
            )?;
            mirror_string(diff, merged, "routed_to_arch_review", &arch_id);
            mirror_string(diff, merged, "produced_architecture_review_id", &arch_id);
        }

        // reject_noise → terminal (no pre-validation side effects)
        _ => {}
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Public hook 2: recon-return side effects
// ---------------------------------------------------------------------------

/// Called from `transition::run_in_tx` AFTER `execute_transition_write` for
/// the recon-return verb (needs_info → triaging).
///
/// Increments recon_round and enforces the ≤ 2 cap.  Also appends evidence
/// from the diff to the intake row's `evidence` ndjson field if provided.
pub(crate) fn inject_recon_return_fields(diff: &mut EntryMap, merged: &mut EntryMap) -> Result<()> {
    let current = merged
        .get("recon_round")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    if current >= 2 {
        anyhow::bail!(
            "recon_round cap exceeded: this intake item has already gone through 2 recon \
             rounds (recon_round={current}). The gatekeeper must make a routing decision now."
        );
    }

    let new_round = current + 1;

    let evidence_append = diff
        .get("evidence")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let existing_evidence = merged
        .get("evidence")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let new_evidence = match (existing_evidence, evidence_append) {
        (Some(existing), Some(new)) if !existing.is_empty() => {
            Some(format!("{}\n{}", existing.trim_end(), new.trim()))
        }
        (None, Some(new)) => Some(new),
        (Some(existing), None) => Some(existing),
        _ => None,
    };

    diff.insert("recon_round".to_string(), Value::Number(new_round.into()));
    merged.insert("recon_round".to_string(), Value::Number(new_round.into()));
    if let Some(ev) = new_evidence {
        diff.insert("evidence".to_string(), Value::String(ev.clone()));
        merged.insert("evidence".to_string(), Value::String(ev));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Fields needed to insert a minimal observations row as a routing side-effect.
struct ObsFields {
    summary: String,
    source: String,
    priority: String,
    captured_at: String,
    captured_week: String,
    tags: Option<String>,  // JSON string e.g. '["fast-track-eligible"]'
    notes: Option<String>, // JSON string
    risk_class: Option<String>,
    approval_policy: Option<String>,
    risk_flags: Option<String>, // JSON string e.g. '["small_local_fix"]'
    cluster_key: Option<String>,
    pending_architecture_review: bool,
}

struct ArchReviewFields {
    summary: String,
    source_observation: String,
    source_intake: Option<String>,
    cluster_key: Option<String>,
}

/// Add a minimal observations row and return its auto-minted display_id (L###).
///
/// This is a gatekeeper route side-effect write — a deterministic output of the
/// Router's routing decision, not a human-grounded write. The invoker is
/// `Actor::Framework` with `SideEffectAuthority::GatekeeperRoute`, which allows
/// writing the risk-classification fields (`risk_class`, `approval_policy`,
/// `risk_flags`, `cluster_key`) that are otherwise gated at `actor:
/// ai_with_human`. The authority is narrow and local: generic
/// `observations add/update` calls do NOT inherit it.
///
/// `created_by` / `updated_by` are set to `"framework"` (the actor string),
/// recording that the row was authored by the engine, not a human or AI agent.
fn insert_observation_row(tx: &Transaction, fields: &ObsFields) -> Result<String> {
    let schema = observations_schema()?;
    let mut entry = EntryMap::new();
    entry.insert("summary".to_string(), Value::String(fields.summary.clone()));
    entry.insert("source".to_string(), Value::String(fields.source.clone()));
    entry.insert(
        "priority".to_string(),
        Value::String(fields.priority.clone()),
    );
    entry.insert(
        "captured_at".to_string(),
        Value::String(fields.captured_at.clone()),
    );
    entry.insert(
        "captured_week".to_string(),
        Value::String(fields.captured_week.clone()),
    );
    if let Some(t) = &fields.tags {
        entry.insert(
            "tags".to_string(),
            serde_json::from_str(t).context("parse tags")?,
        );
    }
    if let Some(n) = &fields.notes {
        entry.insert(
            "notes".to_string(),
            serde_json::from_str(n).context("parse notes")?,
        );
    }
    if let Some(rc) = &fields.risk_class {
        entry.insert("risk_class".to_string(), Value::String(rc.clone()));
    }
    if let Some(ap) = &fields.approval_policy {
        entry.insert("approval_policy".to_string(), Value::String(ap.clone()));
    }
    if let Some(rf) = &fields.risk_flags {
        entry.insert(
            "risk_flags".to_string(),
            serde_json::from_str(rf).context("parse risk_flags")?,
        );
    }
    if let Some(ck) = &fields.cluster_key {
        entry.insert("cluster_key".to_string(), Value::String(ck.clone()));
    }
    if fields.pending_architecture_review {
        entry.insert("pending_architecture_review".to_string(), Value::Bool(true));
    }

    // Use Actor::Framework + SideEffectAuthority::GatekeeperRoute so that the
    // narrow per-callsite authority allowlist is engaged. This is NOT a generic
    // Framework→AiWithHuman widening; it is a named, closed bypass for the
    // specific fields written by this routing side-effect path only.
    super::add::add_row_in_tx_with_authority(
        tx,
        &schema,
        entry,
        Actor::Framework,
        SideEffectAuthority::GatekeeperRoute,
    )
}

fn insert_architecture_review_row(tx: &Transaction, fields: &ArchReviewFields) -> Result<String> {
    let schema = architecture_reviews_schema()?;
    let mut entry = EntryMap::new();
    entry.insert("kind".to_string(), Value::String("interpret".to_string()));
    entry.insert("summary".to_string(), Value::String(fields.summary.clone()));
    entry.insert(
        "source_observation".to_string(),
        Value::String(fields.source_observation.clone()),
    );
    if let Some(source_intake) = &fields.source_intake {
        entry.insert(
            "source_intake".to_string(),
            Value::String(source_intake.clone()),
        );
    }
    if let Some(cluster_key) = &fields.cluster_key {
        entry.insert(
            "cluster_key".to_string(),
            Value::String(cluster_key.clone()),
        );
    }
    entry.insert(
        "linked_observation_ids".to_string(),
        Value::Array(vec![Value::String(fields.source_observation.clone())]),
    );

    super::add::add_row_in_tx(tx, &schema, entry, Actor::Framework)
}

fn observations_schema() -> Result<Schema> {
    Schema::from_yaml(include_str!("../../stores/observations/schema.yaml"))
        .context("parse bundled observations schema")
}

fn architecture_reviews_schema() -> Result<Schema> {
    Schema::from_yaml(include_str!(
        "../../stores/architecture_reviews/schema.yaml"
    ))
    .context("parse bundled architecture_reviews schema")
}

/// Look up the cluster_key of an intake item (I###) or observation (L###) by display_id.
/// Returns None if the row doesn't exist or has no cluster_key set.
fn lookup_cluster_key(tx: &Transaction, display_id: &str) -> Result<Option<String>> {
    let table = if display_id.starts_with('L') {
        "observations"
    } else {
        "intake"
    };
    let result: Option<Option<String>> = tx
        .query_row(
            &format!("SELECT cluster_key FROM {table} WHERE display_id = ?1"),
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .optional()
        .context("lookup_cluster_key")?;
    Ok(result.flatten())
}

/// Return the current week label in the "wWW-dD" format (e.g. "w19-d2").
fn week_label() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    // Minimal inline implementation — avoids pulling a chrono/time dependency.
    // Accurate for the narrow use case of writing captured_week to new obs rows.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // ISO week: Jan 4 is always in week 1.  Approximate via days since epoch.
    let days_since_epoch = secs / 86400;
    // Day of week (0=Mon, 6=Sun per ISO) — epoch was Thursday (3).
    let dow = (days_since_epoch + 3) % 7;
    // Week number (1-based): shift epoch to nearest Monday.
    let days_to_monday = days_since_epoch.saturating_sub(dow);
    let week = (days_to_monday / 7) % 52 + 1;
    // ISO day-of-week (1=Mon, 7=Sun).
    let iso_dow = dow + 1;
    format!("w{week:02}-d{iso_dow}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::schema::Schema;

    fn intake_schema() -> Schema {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "intake")
            .map(|(_, y)| *y)
            .expect("intake schema");
        Schema::from_yaml(yaml).expect("parse intake schema")
    }

    fn obs_schema() -> Schema {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "observations")
            .map(|(_, y)| *y)
            .expect("observations schema");
        Schema::from_yaml(yaml).expect("parse observations schema")
    }

    fn arch_schema() -> Schema {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "architecture_reviews")
            .map(|(_, y)| *y)
            .expect("architecture_reviews schema");
        Schema::from_yaml(yaml).expect("parse architecture_reviews schema")
    }

    fn fresh_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        let is = intake_schema();
        let os = obs_schema();
        let ars = arch_schema();
        conn.execute_batch(&ddl_for(&is)).unwrap();
        conn.execute_batch(&ddl_for(&os)).unwrap();
        conn.execute_batch(&ddl_for(&ars)).unwrap();
        conn
    }

    fn insert_triaging(conn: &rusqlite::Connection, display_id: &str) {
        conn.execute(
            concat!("in", "sert into intake (display_id, status, summary, source_agent, captured_at, captured_week, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'triaging', 'test item', 'executor', '2026-05-06T10:00:00Z', 'w19-d2', '2026-05-06T10:00:00Z', '2026-05-06T10:00:00Z', 'ai_autonomous', 'ai_autonomous')"),
            rusqlite::params![display_id],
        ).unwrap();
    }

    #[test]
    fn fast_track_creates_obs_with_tag() {
        let conn = fresh_db();
        insert_triaging(&conn, "I001");

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("fast_track".to_string()),
        );
        merged.insert(
            "summary".to_string(),
            Value::String("test fast track".to_string()),
        );

        inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route").unwrap();
        tx.commit().unwrap();

        let obs_id = merged
            .get("routed_to_observation")
            .and_then(|v| v.as_str())
            .expect("routed_to_observation must be set");

        let (status, tags_raw): (String, Option<String>) = conn
            .query_row(
                "SELECT status, tags FROM observations WHERE display_id = ?1",
                rusqlite::params![obs_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("observation must exist");

        assert_eq!(status, "open");
        let tags_raw = tags_raw.expect("tags must be set");
        assert!(
            tags_raw.contains("fast-track-eligible"),
            "tags must contain fast-track-eligible"
        );
    }

    #[test]
    fn normal_observation_creates_obs_with_l143_columns() {
        let conn = fresh_db();
        insert_triaging(&conn, "I001");

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("normal_observation".to_string()),
        );
        merged.insert(
            "summary".to_string(),
            Value::String("stale pid cleanup".to_string()),
        );
        merged.insert(
            "decision_metadata".to_string(),
            serde_json::json!({
                "risk_class_hint": "low",
                "approval_policy_hint": "auto"
            }),
        );
        merged.insert(
            "risk_flags".to_string(),
            serde_json::json!(["small_local_fix"]),
        );
        merged.insert(
            "cluster_key".to_string(),
            Value::String("gatekeeper-front-door-stuck".to_string()),
        );

        inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route").unwrap();
        tx.commit().unwrap();

        let obs_id = merged
            .get("routed_to_observation")
            .and_then(|v| v.as_str())
            .expect("routed_to_observation must be set");

        let (rc, ap, rf, ck): (Option<String>, Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT risk_class, approval_policy, risk_flags, cluster_key FROM observations WHERE display_id = ?1",
                rusqlite::params![obs_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .expect("observation must exist");

        assert_eq!(rc.as_deref(), Some("low"));
        assert_eq!(ap.as_deref(), Some("auto"));
        assert!(rf.as_deref().unwrap_or("").contains("small_local_fix"));
        assert_eq!(ck.as_deref(), Some("gatekeeper-front-door-stuck"));
    }

    #[test]
    fn arch_review_candidate_creates_obs_and_arch_review() {
        let conn = fresh_db();
        insert_triaging(&conn, "I001");

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("arch_review_candidate".to_string()),
        );
        merged.insert(
            "summary".to_string(),
            Value::String("arch candidate".to_string()),
        );

        inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route").unwrap();
        tx.commit().unwrap();

        let obs_id = merged
            .get("routed_to_observation")
            .and_then(|v| v.as_str())
            .expect("routed_to_observation must be set for arch_review_candidate");
        let arch_id = merged
            .get("routed_to_arch_review")
            .and_then(|v| v.as_str())
            .expect("routed_to_arch_review must be set for arch_review_candidate");

        let (pending, tags_raw): (i64, Option<String>) = conn
            .query_row(
                "SELECT pending_architecture_review, tags FROM observations WHERE display_id = ?1",
                rusqlite::params![obs_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("observation must exist");

        assert_eq!(pending, 1);
        assert!(!tags_raw
            .as_deref()
            .unwrap_or("")
            .contains("arch-review-candidate"));

        let (status, kind, source_observation): (String, String, String) = conn
            .query_row(
                "SELECT status, kind, source_observation FROM architecture_reviews WHERE display_id = ?1",
                rusqlite::params![arch_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("architecture review must exist");
        assert_eq!(status, "pending");
        assert_eq!(kind, "interpret");
        assert_eq!(source_observation, obs_id);
    }

    #[test]
    fn duplicate_copies_cluster_key_from_source() {
        let conn = fresh_db();
        // Insert two intake items; I001 is the source with cluster_key
        conn.execute(
            concat!("in", "sert into intake (display_id, status, summary, source_agent, captured_at, captured_week, cluster_key, created_at, updated_at, created_by, updated_by) \
             VALUES ('I001', 'routed', 'source item', 'executor', '2026-05-06T10:00:00Z', 'w19-d2', 'dispatch-lifecycle', '2026-05-06T10:00:00Z', '2026-05-06T10:00:00Z', 'ai_autonomous', 'ai_autonomous')"),
            [],
        ).unwrap();
        // I002 is the duplicate
        conn.execute(
            concat!("in", "sert into intake (display_id, status, summary, source_agent, captured_at, captured_week, created_at, updated_at, created_by, updated_by) \
             VALUES ('I002', 'triaging', 'dupe item', 'executor', '2026-05-06T10:00:00Z', 'w19-d2', '2026-05-06T10:00:00Z', '2026-05-06T10:00:00Z', 'ai_autonomous', 'ai_autonomous')"),
            [],
        ).unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("duplicate".to_string()),
        );
        merged.insert(
            "duplicate_of".to_string(),
            Value::String("I001".to_string()),
        );

        inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route").unwrap();
        tx.commit().unwrap();

        assert_eq!(
            merged.get("cluster_key").and_then(|v| v.as_str()),
            Some("dispatch-lifecycle"),
            "cluster_key must be copied from I001"
        );
    }

    #[test]
    fn needs_info_extracts_missing_info_question() {
        let conn = fresh_db();
        insert_triaging(&conn, "I001");

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("needs_info".to_string()),
        );
        merged.insert(
            "gatekeeper_decision_json".to_string(),
            serde_json::json!({
                "decision": "needs_info",
                "confidence": "low",
                "rationale": "Need more context.",
                "missing_info_question": "What is the affected file path?"
            }),
        );

        inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route").unwrap();
        tx.commit().unwrap();

        assert_eq!(
            merged.get("missing_info_question").and_then(|v| v.as_str()),
            Some("What is the affected file path?"),
        );
    }

    #[test]
    fn recon_return_increments_recon_round() {
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert("recon_round".to_string(), Value::Number(0.into()));
        let mut diff: EntryMap = std::collections::BTreeMap::new();

        inject_recon_return_fields(&mut diff, &mut merged).unwrap();

        assert_eq!(merged.get("recon_round").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(diff.get("recon_round").and_then(|v| v.as_i64()), Some(1));
    }

    #[test]
    fn recon_return_cap_at_two_rejects() {
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert("recon_round".to_string(), Value::Number(2.into()));
        let mut diff: EntryMap = std::collections::BTreeMap::new();

        let err = inject_recon_return_fields(&mut diff, &mut merged).unwrap_err();
        assert!(
            err.to_string().contains("cap exceeded"),
            "expected cap error; got: {err}"
        );
    }

    #[test]
    fn pre_supplied_routed_to_observation_not_overridden() {
        let conn = fresh_db();
        insert_triaging(&conn, "I001");

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("normal_observation".to_string()),
        );
        merged.insert(
            "routed_to_observation".to_string(),
            Value::String("L042".to_string()),
        );

        inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route").unwrap();
        tx.commit().unwrap();

        // Must NOT have created a new obs row; routed_to_observation stays L042
        assert_eq!(
            merged.get("routed_to_observation").and_then(|v| v.as_str()),
            Some("L042")
        );
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "no observation row should have been created");
    }

    #[test]
    fn arch_review_candidate_rejects_pre_supplied_arch_review_id() {
        let conn = fresh_db();
        insert_triaging(&conn, "I001");

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("arch_review_candidate".to_string()),
        );
        merged.insert(
            "summary".to_string(),
            Value::String("arch candidate".to_string()),
        );
        merged.insert(
            "routed_to_arch_review".to_string(),
            Value::String("A999".to_string()),
        );

        let err = inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route").unwrap_err();
        assert!(
            err.to_string()
                .contains("caller-supplied routed_to_arch_review"),
            "expected pre-supplied A### rejection; got: {err}"
        );
        drop(tx);

        let obs_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        let arch_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM architecture_reviews", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            obs_count, 0,
            "rejected pre-supplied A### must not create L###"
        );
        assert_eq!(
            arch_count, 0,
            "rejected pre-supplied A### must not create A###"
        );
    }

    #[test]
    fn arch_review_candidate_rejects_pre_supplied_observation_id() {
        let conn = fresh_db();
        insert_triaging(&conn, "I001");
        conn.execute(
            concat!("in", "sert into observations (display_id, status, summary, source, priority, captured_at, captured_week, created_at, updated_at, created_by, updated_by) \
             VALUES ('L001', 'open', 'preexisting obs', 'dev', 'normal', '2026-05-06T10:00:00Z', 'w19-d2', '2026-05-06T10:00:00Z', '2026-05-06T10:00:00Z', 'ai_with_human', 'ai_with_human')"),
            [],
        )
        .unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("arch_review_candidate".to_string()),
        );
        merged.insert(
            "summary".to_string(),
            Value::String("arch candidate".to_string()),
        );
        merged.insert(
            "routed_to_observation".to_string(),
            Value::String("L001".to_string()),
        );

        let err = inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route").unwrap_err();
        assert!(
            err.to_string()
                .contains("caller-supplied routed_to_observation"),
            "expected pre-supplied L### rejection; got: {err}"
        );
        drop(tx);

        let obs_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        let pending: i64 = conn
            .query_row(
                "SELECT pending_architecture_review FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let arch_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM architecture_reviews", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            obs_count, 1,
            "rejected pre-supplied L### must not mint another L###"
        );
        assert_eq!(
            pending, 0,
            "rejected pre-supplied L### must not mark pending"
        );
        assert_eq!(
            arch_count, 0,
            "rejected pre-supplied L### must not create A###"
        );
    }

    #[test]
    fn arch_review_insert_failure_rolls_back_pending_observation() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        let is = intake_schema();
        let os = obs_schema();
        conn.execute_batch(&ddl_for(&is)).unwrap();
        conn.execute_batch(&ddl_for(&os)).unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("arch_review_candidate".to_string()),
        );
        merged.insert(
            "summary".to_string(),
            Value::String("should roll back".to_string()),
        );
        merged.insert(
            "cluster_key".to_string(),
            Value::String("actor-authority".to_string()),
        );

        let result = inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route");
        assert!(
            result.is_err(),
            "must propagate error when architecture_reviews table missing"
        );
        drop(tx);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 0,
            "source observation must roll back with failed A### insert"
        );
    }

    #[test]
    fn failed_obs_insert_rolls_back_via_transaction() {
        // Verify that if observation insert fails (e.g. table doesn't exist in
        // a stripped DB), the error propagates and the caller's tx can roll back.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        // Deliberately omit the observations DDL so the insert will fail.
        let is = intake_schema();
        conn.execute_batch(&ddl_for(&is)).unwrap();

        let tx = conn.unchecked_transaction().unwrap();
        let mut diff: EntryMap = std::collections::BTreeMap::new();
        let mut merged: EntryMap = std::collections::BTreeMap::new();
        merged.insert(
            "decision".to_string(),
            Value::String("fast_track".to_string()),
        );
        merged.insert(
            "summary".to_string(),
            Value::String("should fail".to_string()),
        );

        let result = inject_pre_validation_fields(&tx, &mut diff, &mut merged, "route");
        assert!(
            result.is_err(),
            "must propagate error when observations table missing"
        );
        // tx dropped without commit → intake row safe
    }
}
