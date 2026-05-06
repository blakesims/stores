//! T053 FIX4 — regression tests for SideEffectAuthority::GatekeeperRoute.
//!
//! Covers Pi's three required regression tests:
//!   Test A — Generic Actor::Framework write to an actor:ai_with_human field
//!             WITHOUT GatekeeperRoute authority is REJECTED.
//!   Test B — Actor::Framework + Some(GatekeeperRoute) write to an allowlisted
//!             actor:ai_with_human field SUCCEEDS (mimics the route side-effect path).
//!   Test C — Actor::Framework + Some(GatekeeperRoute) write to a NON-allowlisted
//!             actor:ai_with_human field is REJECTED (authority is field-scoped,
//!             not blanket).
//!
//! These tests operate at the validate::validate / validate_with_authority level
//! so they are independent of the SQLite layer.

use std::collections::BTreeMap;
use stores::validate::{self, Op, SideEffectAuthority, ValidationError};
use stores::schema::{actor::{Actor, InvokerCtx}, Schema};
use serde_json::Value;

/// Minimal observations-like schema with three fields:
///   - `risk_class`  : actor:ai_with_human  (allowlisted by GatekeeperRoute)
///   - `approved_by` : actor:human           (NOT allowlisted, highest-tier gate)
///   - `summary`     : no actor constraint   (unrestricted)
const FIXTURE_SCHEMA: &str = r#"
name: obs_test
id_format: "L{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: summary
    type: text
    required: true

  - name: captured_at
    type: text
    required: true

  - name: captured_week
    type: text
    required: true

  - name: source
    type: text
    required: true

  - name: priority
    type: text
    required: true

  - name: risk_class
    type: text
    actor: ai_with_human

  - name: approved_by
    type: text
    actor: human
"#;

fn schema() -> Schema {
    Schema::from_yaml(FIXTURE_SCHEMA).expect("fixture schema must parse")
}

fn base_entry() -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    m.insert("summary".to_string(), Value::String("test".to_string()));
    m.insert("captured_at".to_string(), Value::String("2026-05-07T00:00:00Z".to_string()));
    m.insert("captured_week".to_string(), Value::String("w19-d3".to_string()));
    m.insert("source".to_string(), Value::String("intake".to_string()));
    m.insert("priority".to_string(), Value::String("normal".to_string()));
    m
}

fn framework_ctx() -> InvokerCtx {
    InvokerCtx::bare(Actor::Framework)
}

// ---------------------------------------------------------------------------
// Test A: Generic Framework (authority=None) → REJECTED on actor:ai_with_human
// ---------------------------------------------------------------------------

/// Generic Actor::Framework write to `risk_class` (actor:ai_with_human) without
/// GatekeeperRoute authority must be REJECTED.
///
/// This proves that Actor::Framework is NOT globally higher-authority and does
/// not satisfy ai_with_human gates for ordinary writes.
#[test]
fn test_a_framework_no_authority_rejects_ai_with_human_field() {
    let schema = schema();
    let mut entry = base_entry();
    entry.insert("risk_class".to_string(), Value::String("low".to_string()));

    // Public validate (authority=None) — the path generic `observations add` takes.
    let result = validate::validate(&schema, &entry, Op::Add, framework_ctx());

    assert!(
        result.is_err(),
        "Framework without authority must be rejected for actor:ai_with_human field"
    );
    let errs: Vec<ValidationError> = result.unwrap_err();
    assert!(
        errs.iter().any(|e| e.field_path.contains(&"risk_class".to_string())),
        "Error must reference 'risk_class'; got: {:?}", errs
    );
}

// ---------------------------------------------------------------------------
// Test B: Framework + GatekeeperRoute → ALLOWED on allowlisted field
// ---------------------------------------------------------------------------

/// Actor::Framework + Some(GatekeeperRoute) write to `risk_class` SUCCEEDS,
/// mimicking the gatekeeper route side-effect path.
///
/// The allowlist in actor_allowed_with_authority includes `risk_class` as a
/// direct output of the routing decision.
#[test]
fn test_b_framework_gatekeeper_route_allows_allowlisted_field() {
    let schema = schema();
    let mut entry = base_entry();
    entry.insert("risk_class".to_string(), Value::String("low".to_string()));

    // validate_with_authority — the path insert_observation_row takes.
    let result = validate::validate_with_authority(
        &schema,
        &entry,
        Op::Add,
        framework_ctx(),
        Some(SideEffectAuthority::GatekeeperRoute),
    );

    assert!(
        result.is_ok(),
        "Framework + GatekeeperRoute must succeed for allowlisted field 'risk_class'; \
         errors: {:?}",
        result.err()
    );
}

// ---------------------------------------------------------------------------
// Test C: Framework + GatekeeperRoute → REJECTED on non-allowlisted field
// ---------------------------------------------------------------------------

/// Actor::Framework + Some(GatekeeperRoute) write to `approved_by` (actor:human,
/// NOT in the GatekeeperRoute allowlist) is REJECTED.
///
/// This proves the authority is field-scoped, not blanket: GatekeeperRoute does
/// not grant Framework the ability to write human-gated fields, or any
/// ai_with_human-gated field outside the named allowlist.
#[test]
fn test_c_framework_gatekeeper_route_rejects_non_allowlisted_field() {
    let schema = schema();
    let mut entry = base_entry();
    // `approved_by` is actor:human — not in the GatekeeperRoute allowlist.
    entry.insert("approved_by".to_string(), Value::String("blake".to_string()));

    let result = validate::validate_with_authority(
        &schema,
        &entry,
        Op::Add,
        framework_ctx(),
        Some(SideEffectAuthority::GatekeeperRoute),
    );

    assert!(
        result.is_err(),
        "Framework + GatekeeperRoute must NOT satisfy actor:human gate on 'approved_by'"
    );
    let errs = result.unwrap_err();
    assert!(
        errs.iter().any(|e| e.field_path.contains(&"approved_by".to_string())),
        "Error must reference 'approved_by'; got: {:?}", errs
    );
}
