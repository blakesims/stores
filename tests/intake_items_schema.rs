//! T053 Phase 1 — intake_items store schema validation tests.
//!
//! Covers:
//!   - Schema parses from bundled YAML (ddl roundtrip)
//!   - id_format "I{:03d}" is registered
//!   - 6 lifecycle states are present
//!   - Key transitions are declared with correct actor classes
//!   - reopen is actor: ai_with_human (AC1.4 guard)
//!   - Cross-row required_when guards are schema-declared (AC1.5)
//!   - DDL can be applied to an in-memory SQLite DB
//!   - Schema installs as CLI subcommands (AC1.2 shape verification)

use rusqlite::Connection;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::schema::{
    actor::Actor,
    lifecycle::select_transition,
    Schema,
};
use stores::validate::{self, EntryMap, Op};
use serde_json::json;

fn intake_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "intake")
        .map(|(_, y)| *y)
        .expect("'intake' not found in BUNDLED_STORE_SCHEMAS");
    Schema::from_yaml(yaml).expect("intake schema must parse")
}

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    let s = intake_schema();
    conn.execute_batch(&ddl_for(&s)).unwrap();
    conn
}

fn entry(fields: serde_json::Value) -> EntryMap {
    match fields {
        serde_json::Value::Object(map) => map.into_iter().collect(),
        _ => panic!("expected JSON object"),
    }
}

// ---- Schema structure ----

#[test]
fn schema_name_is_intake() {
    let s = intake_schema();
    assert_eq!(s.name, "intake");
}

#[test]
fn schema_id_format_is_i_three_digits() {
    let s = intake_schema();
    assert_eq!(s.id_format, "I{:03d}");
}

#[test]
fn schema_has_five_states() {
    let s = intake_schema();
    let states = &s.lifecycle.states;
    for expected in &["draft", "triaging", "needs_info", "routed", "dropped"] {
        assert!(
            states.iter().any(|st| st == expected),
            "missing state: {expected}"
        );
    }
    assert!(!states.iter().any(|st| st == "escalated"), "escalated state must not exist");
    assert_eq!(states.len(), 5, "expected exactly 5 states, got {}", states.len());
}

#[test]
fn schema_initial_state_is_draft() {
    let s = intake_schema();
    assert_eq!(s.lifecycle.resolved_initial_state().unwrap(), "draft");
}

#[test]
fn claim_triage_is_ai_autonomous() {
    let s = intake_schema();
    let t = s.lifecycle.transitions.iter().find(|t| t.verb == "claim-triage")
        .expect("claim-triage transition missing");
    assert_eq!(t.actor, Some(Actor::AiAutonomous));
    assert_eq!(t.from, "draft");
    assert_eq!(t.to, "triaging");
}

#[test]
fn reopen_requires_ai_with_human() {
    let s = intake_schema();
    let t = s.lifecycle.transitions.iter().find(|t| t.verb == "reopen")
        .expect("reopen transition missing");
    assert_eq!(t.actor, Some(Actor::AiWithHuman), "reopen must require ai_with_human");
    assert_eq!(t.from, "dropped");
    assert_eq!(t.to, "draft");
}

#[test]
fn recon_return_is_ai_autonomous() {
    let s = intake_schema();
    let t = s.lifecycle.transitions.iter().find(|t| t.verb == "recon-return")
        .expect("recon-return transition missing");
    assert_eq!(t.actor, Some(Actor::AiAutonomous));
    assert_eq!(t.from, "needs_info");
    assert_eq!(t.to, "triaging");
}

#[test]
fn arch_review_candidate_uses_route_verb_to_routed() {
    use stores::schema::expr::{Lhs, Op, Rhs};
    // arch_review_candidate is a normal Router outcome via the route verb (not a separate lifecycle family).
    let s = intake_schema();
    let t = s.lifecycle.transitions.iter()
        .find(|t| {
            if t.verb != "route" { return false; }
            match &t.guard {
                Some(g) => {
                    g.lhs == Lhs::Path(vec!["decision".to_string()])
                        && g.op == Op::Eq
                        && g.rhs == Rhs::Literal("arch_review_candidate".to_string())
                }
                None => false,
            }
        })
        .expect("route transition with guard decision == 'arch_review_candidate' missing");
    assert_eq!(t.actor, Some(Actor::AiAutonomous));
    assert_eq!(t.from, "triaging");
    assert_eq!(t.to, "routed");
    // escalate-arch-review verb must not exist
    assert!(
        s.lifecycle.transitions.iter().all(|t| t.verb != "escalate-arch-review"),
        "escalate-arch-review transition must not exist"
    );
}

#[test]
fn route_transitions_cover_all_decisions() {
    let s = intake_schema();
    let route_transitions: Vec<_> = s.lifecycle.transitions.iter()
        .filter(|t| t.verb == "route")
        .collect();
    // All from triaging
    for t in &route_transitions {
        assert_eq!(t.from, "triaging", "route from unexpected state: {}", t.from);
        assert_eq!(t.actor, Some(Actor::AiAutonomous));
    }
    // Guard-based routing to different states
    let has_needs_info = route_transitions.iter().any(|t| t.to == "needs_info");
    let has_routed = route_transitions.iter().any(|t| t.to == "routed");
    let has_dropped = route_transitions.iter().any(|t| t.to == "dropped");
    assert!(has_needs_info, "route should have a needs_info branch");
    assert!(has_routed, "route should have a routed branch");
    assert!(has_dropped, "route should have a dropped branch");
}

#[test]
fn transition_ambiguity_is_clean() {
    let s = intake_schema();
    s.lifecycle.validate_transition_ambiguity()
        .expect("transition ambiguity check must pass");
}

// ---- DDL roundtrip ----

#[test]
fn ddl_roundtrip() {
    let _conn = fresh_db();
    // If fresh_db() doesn't panic, the DDL round-trips
}

#[test]
fn ddl_creates_intake_table() {
    let conn = fresh_db();
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='intake'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    assert!(exists, "intake table must be created by DDL");
}

// ---- Cross-row required_when guards (AC1.5) ----

#[test]
fn route_duplicate_without_duplicate_of_fails_validation() {
    let s = intake_schema();
    // Simulate a merged entry where decision=duplicate but duplicate_of is absent
    let merged = entry(json!({
        "status": "triaging",
        "summary": "test",
        "source_agent": "planner",
        "captured_at": "2026-05-06T10:00:00Z",
        "captured_week": "w19-d2",
        "decision": "duplicate"
        // duplicate_of intentionally absent
    }));
    let diff = entry(json!({ "decision": "duplicate" }));
    let result = validate::validate(
        &s,
        &merged,
        Op::Transition("route".to_string(), diff),
        Actor::AiAutonomous.into(),
    );
    assert!(result.is_err(), "route --decision duplicate without duplicate_of must fail");
    let errs = result.unwrap_err();
    let msg = validate::pretty_print(&errs);
    assert!(
        msg.contains("duplicate_of"),
        "error must mention duplicate_of; got: {msg}"
    );
}

#[test]
fn route_duplicate_with_duplicate_of_passes_validation() {
    let s = intake_schema();
    let merged = entry(json!({
        "status": "triaging",
        "summary": "test",
        "source_agent": "planner",
        "captured_at": "2026-05-06T10:00:00Z",
        "captured_week": "w19-d2",
        "decision": "duplicate",
        "duplicate_of": "I001"
    }));
    let diff = entry(json!({ "decision": "duplicate", "duplicate_of": "I001" }));
    validate::validate(
        &s,
        &merged,
        Op::Transition("route".to_string(), diff),
        Actor::AiAutonomous.into(),
    ).expect("route --decision duplicate WITH duplicate_of must pass");
}

#[test]
fn route_normal_observation_without_routed_to_observation_fails_validation() {
    let s = intake_schema();
    let merged = entry(json!({
        "status": "triaging",
        "summary": "test",
        "source_agent": "planner",
        "captured_at": "2026-05-06T10:00:00Z",
        "captured_week": "w19-d2",
        "decision": "normal_observation"
        // routed_to_observation intentionally absent
    }));
    let diff = entry(json!({ "decision": "normal_observation" }));
    let result = validate::validate(
        &s,
        &merged,
        Op::Transition("route".to_string(), diff),
        Actor::AiAutonomous.into(),
    );
    assert!(result.is_err(), "route --decision normal_observation without routed_to_observation must fail");
    let errs = result.unwrap_err();
    let msg = validate::pretty_print(&errs);
    assert!(
        msg.contains("routed_to_observation"),
        "error must mention routed_to_observation; got: {msg}"
    );
}

#[test]
fn route_normal_observation_with_routed_to_observation_passes_validation() {
    let s = intake_schema();
    let merged = entry(json!({
        "status": "triaging",
        "summary": "test",
        "source_agent": "planner",
        "captured_at": "2026-05-06T10:00:00Z",
        "captured_week": "w19-d2",
        "decision": "normal_observation",
        "routed_to_observation": "L042"
    }));
    let diff = entry(json!({ "decision": "normal_observation", "routed_to_observation": "L042" }));
    validate::validate(
        &s,
        &merged,
        Op::Transition("route".to_string(), diff),
        Actor::AiAutonomous.into(),
    ).expect("route --decision normal_observation WITH routed_to_observation must pass");
}

// ---- Guard-based transition selection ----

#[test]
fn route_guard_selects_needs_info_state() {
    let s = intake_schema();
    let merged = entry(json!({
        "status": "triaging",
        "decision": "needs_info"
    }));
    let t = select_transition(
        &s.lifecycle.transitions,
        "triaging",
        "route",
        None,
        &merged,
    ).expect("route from triaging with decision=needs_info must resolve");
    assert_eq!(t.to, "needs_info");
}

#[test]
fn route_guard_selects_dropped_for_reject_noise() {
    let s = intake_schema();
    let merged = entry(json!({
        "status": "triaging",
        "decision": "reject_noise"
    }));
    let t = select_transition(
        &s.lifecycle.transitions,
        "triaging",
        "route",
        None,
        &merged,
    ).expect("route from triaging with decision=reject_noise must resolve");
    assert_eq!(t.to, "dropped");
}

#[test]
fn route_guard_selects_routed_for_fast_track() {
    let s = intake_schema();
    let merged = entry(json!({
        "status": "triaging",
        "decision": "fast_track"
    }));
    let t = select_transition(
        &s.lifecycle.transitions,
        "triaging",
        "route",
        None,
        &merged,
    ).expect("route from triaging with decision=fast_track must resolve");
    assert_eq!(t.to, "routed");
}

// ---- Fixture loading ----

#[test]
fn minimal_fixture_has_required_fields() {
    let fixture_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/intake_items_minimal.yaml");
    let text = std::fs::read_to_string(&fixture_path)
        .expect("intake_items_minimal.yaml must exist");
    let v: serde_yaml::Value = serde_yaml::from_str(&text)
        .expect("fixture must be valid YAML");
    let map = v.as_mapping().expect("fixture must be a YAML mapping");
    for field in &["summary", "source_agent", "captured_at", "captured_week"] {
        assert!(
            map.contains_key(&serde_yaml::Value::String(field.to_string())),
            "fixture must contain required field: {field}"
        );
    }
}
