//! Integration tests for T052 P2: risk_flags list_enum validation on observations.
//!
//! Verifies that the bundled observations schema's risk_flags field rejects
//! non-canonical values and accepts canonical ones, using the validate path.

use std::collections::BTreeMap;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::schema::{actor::Actor, Schema};
use stores::validate::{self, Op, ValidationError};
use stores::validate::error::RuleKind;

fn obs_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(name, _)| *name == "observations")
        .map(|(_, y)| *y)
        .expect("bundled observations schema must exist");
    Schema::from_yaml(yaml).expect("observations schema must parse")
}

fn str_val(s: &str) -> serde_json::Value {
    serde_json::Value::String(s.to_string())
}

fn flag_array(flags: &[&str]) -> serde_json::Value {
    serde_json::Value::Array(
        flags
            .iter()
            .map(|s| serde_json::Value::String(s.to_string()))
            .collect(),
    )
}

/// Minimal valid observation entry (required fields satisfied).
fn base_entry() -> BTreeMap<String, serde_json::Value> {
    let mut m = BTreeMap::new();
    m.insert("summary".to_string(), str_val("test observation"));
    m.insert("source".to_string(), str_val("dev"));
    m.insert("priority".to_string(), str_val("normal"));
    m.insert("captured_at".to_string(), str_val("2026-05-06T12:00:00+00:00"));
    m.insert("captured_week".to_string(), str_val("w19-d2"));
    m
}

fn list_enum_errors(errs: &[ValidationError]) -> Vec<&ValidationError> {
    errs.iter()
        .filter(|e| e.field_path == vec!["risk_flags".to_string()])
        .filter(|e| e.rule == RuleKind::ListEnum)
        .collect()
}

/// Merge base_entry with diff, simulating what the update handler does.
fn merge(base: &BTreeMap<String, serde_json::Value>, diff: &BTreeMap<String, serde_json::Value>) -> BTreeMap<String, serde_json::Value> {
    let mut merged = base.clone();
    for (k, v) in diff {
        merged.insert(k.clone(), v.clone());
    }
    merged
}

/// AC2.2 — update with a non-canonical risk_flag value fails with a
/// ValidationError naming the bad value and listing the allowed set.
#[test]
fn risk_flags_bogus_value_rejected() {
    let schema = obs_schema();
    let base = base_entry();
    let mut diff = BTreeMap::new();
    diff.insert("risk_flags".to_string(), flag_array(&["bogus_flag"]));
    let merged = merge(&base, &diff);

    let result = validate::validate(
        &schema,
        &merged,
        Op::Update(diff),
        Actor::AiWithHuman.into(),
    );

    let errs = result.expect_err("bogus_flag must produce a validation error");
    let le_errs = list_enum_errors(&errs);
    assert!(
        !le_errs.is_empty(),
        "expected at least one ListEnum error on risk_flags; got: {:?}",
        errs
    );
    let msg = &le_errs[0].message;
    assert!(
        msg.contains("bogus_flag"),
        "error message must name the bad value 'bogus_flag'; got: {}",
        msg
    );
    assert!(
        msg.contains("docs_only") || msg.contains("small_local_fix"),
        "error message must list canonical values; got: {}",
        msg
    );
}

/// AC2.2 — update with only canonical values succeeds.
#[test]
fn risk_flags_canonical_values_accepted() {
    let schema = obs_schema();
    let base = base_entry();
    let mut diff = BTreeMap::new();
    diff.insert(
        "risk_flags".to_string(),
        flag_array(&["docs_only", "small_local_fix"]),
    );
    let merged = merge(&base, &diff);

    validate::validate(
        &schema,
        &merged,
        Op::Update(diff),
        Actor::AiWithHuman.into(),
    )
    .expect("canonical risk_flags values must pass validation");
}

/// AC2.3 — empty risk_flags array is accepted.
#[test]
fn risk_flags_empty_array_accepted() {
    let schema = obs_schema();
    let base = base_entry();
    let mut diff = BTreeMap::new();
    diff.insert("risk_flags".to_string(), flag_array(&[]));
    let merged = merge(&base, &diff);

    validate::validate(
        &schema,
        &merged,
        Op::Update(diff),
        Actor::AiWithHuman.into(),
    )
    .expect("empty risk_flags array must pass validation");
}

/// AC2.4 — order-independent: both orderings of the same canonical flags pass.
#[test]
fn risk_flags_order_independent() {
    let schema = obs_schema();
    let base = base_entry();

    let orderings: &[&[&str]] = &[
        &["docs_only", "small_local_fix"],
        &["small_local_fix", "docs_only"],
    ];
    for flags in orderings {
        let mut diff = BTreeMap::new();
        diff.insert("risk_flags".to_string(), flag_array(flags));
        let merged = merge(&base, &diff);
        validate::validate(
            &schema,
            &merged,
            Op::Update(diff),
            Actor::AiWithHuman.into(),
        )
        .unwrap_or_else(|errs| {
            panic!(
                "ordering {:?} must pass; errors: {:?}",
                flags,
                errs.iter().map(|e| e.field_path.join(".")).collect::<Vec<_>>()
            )
        });
    }
}

/// Verify all 13 canonical flags are individually accepted.
#[test]
fn risk_flags_all_13_canonical_flags_accepted() {
    let canonical: &[&str] = &[
        "touches_actor_authority",
        "touches_lifecycle",
        "touches_subscriber_semantics",
        "introduces_new_primitive",
        "changes_boundary",
        "security_sensitive",
        "docs_only",
        "small_local_fix",
        "duplicate_symptom",
        "touches_runner_boundary",
        "touches_schema_core",
        "authority_surface_drift",
        "contradicts_prior_decision",
    ];
    let schema = obs_schema();
    let base = base_entry();

    for flag in canonical {
        let mut diff = BTreeMap::new();
        diff.insert("risk_flags".to_string(), flag_array(&[flag]));
        let merged = merge(&base, &diff);
        validate::validate(
            &schema,
            &merged,
            Op::Update(diff),
            Actor::AiWithHuman.into(),
        )
        .unwrap_or_else(|errs| {
            panic!(
                "canonical flag '{}' must be accepted; errors: {:?}",
                flag,
                errs.iter()
                    .map(|e| format!("{}: {}", e.field_path.join("."), e.message))
                    .collect::<Vec<_>>()
            )
        });
    }
}

/// Multiple bad values produce one error per bad value.
#[test]
fn risk_flags_multiple_bad_values_each_produce_error() {
    let schema = obs_schema();
    let base = base_entry();
    let mut diff = BTreeMap::new();
    diff.insert(
        "risk_flags".to_string(),
        flag_array(&["bad1", "docs_only", "bad2"]),
    );
    let merged = merge(&base, &diff);

    let errs = validate::validate(
        &schema,
        &merged,
        Op::Update(diff),
        Actor::AiWithHuman.into(),
    )
    .expect_err("bad flags must produce errors");

    let le_errs = list_enum_errors(&errs);
    assert_eq!(
        le_errs.len(),
        2,
        "expected 2 ListEnum errors (one per bad flag); got: {:?}",
        le_errs
    );
    let msgs: Vec<&str> = le_errs.iter().map(|e| e.message.as_str()).collect();
    assert!(
        msgs.iter().any(|m| m.contains("bad1")),
        "must name 'bad1': {:?}",
        msgs
    );
    assert!(
        msgs.iter().any(|m| m.contains("bad2")),
        "must name 'bad2': {:?}",
        msgs
    );
}
