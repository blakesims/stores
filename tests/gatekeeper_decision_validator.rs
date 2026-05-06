//! T053 Phase 2 — gatekeeper_decision_json validation + field-mirroring integration tests.
//!
//! Covers:
//!   AC2.1 — gatekeeper_decision_validator: required_when conditionals, enum violations
//!   AC2.2 — fast_track with PROHIBIT flag rejected fail-loud (touches_lifecycle)
//!   AC2.3 — valid route writes risk_flags + cluster_key to top-level columns
//!   AC2.4 — derive_risk_class / derive_approval_policy matrix (6 representative vectors)
//!   AC2.5 — decision_metadata captures rationale after a routed transition

use rusqlite::Connection;
use serde_json::json;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::transition;
use stores::schema::{actor::Actor, risk_taxonomy, Schema};

fn intake_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "intake")
        .map(|(_, y)| *y)
        .expect("'intake' not found in BUNDLED_STORE_SCHEMAS");
    Schema::from_yaml(yaml).expect("intake schema must parse")
}

fn fresh_db(schema: &Schema) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(schema)).unwrap();
    conn
}

/// Insert an intake row directly at `triaging` state (bypasses `add` → `claim-triage` steps
/// so tests can focus on the route/validate surface).
fn insert_triaging_row(conn: &Connection, display_id: &str) {
    conn.execute(
        "INSERT INTO intake (display_id, status, summary, source_agent, captured_at, captured_week, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'triaging', 'test intake item', 'executor', '2026-05-06T10:00:00Z', 'w19-d2', '2026-05-06T10:00:00Z', '2026-05-06T10:00:00Z', 'ai_autonomous', 'ai_autonomous')",
        rusqlite::params![display_id],
    ).unwrap();
}

// ---- AC2.1: required_when conditionals via validate_gatekeeper_decision ----

#[test]
fn ac2_1_tier_hint_required_when_fast_track() {
    let v = json!({
        "decision": "fast_track",
        "confidence": "high",
        "rationale": "Typo fix.",
        "risk_flags": ["docs_only"]
        // tier_hint absent → must fail
    });
    let errs = stores::validate::gatekeeper_decision::validate_gatekeeper_decision(&v)
        .unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("tier_hint") && e.contains("fast_track")),
        "expected tier_hint required error; got: {:?}", errs
    );
}

#[test]
fn ac2_1_cluster_key_required_when_normal_observation() {
    let v = json!({
        "decision": "normal_observation",
        "confidence": "medium",
        "rationale": "Stale PID.",
        "tier_hint": "T2"
        // cluster_key absent → must fail
    });
    let errs = stores::validate::gatekeeper_decision::validate_gatekeeper_decision(&v)
        .unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("cluster_key") && e.contains("normal_observation")),
        "expected cluster_key required error; got: {:?}", errs
    );
}

#[test]
fn ac2_1_duplicate_candidates_required_when_duplicate() {
    let v = json!({
        "decision": "duplicate",
        "confidence": "high",
        "rationale": "Already filed.",
        "cluster_key": "dispatch-lifecycle"
        // duplicate_candidates absent → must fail
    });
    let errs = stores::validate::gatekeeper_decision::validate_gatekeeper_decision(&v)
        .unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("duplicate_candidates") && e.contains("required")),
        "expected duplicate_candidates required error; got: {:?}", errs
    );
}

#[test]
fn ac2_1_missing_info_question_required_when_needs_info() {
    let v = json!({
        "decision": "needs_info",
        "confidence": "low",
        "rationale": "Need more evidence."
        // missing_info_question absent → must fail
    });
    let errs = stores::validate::gatekeeper_decision::validate_gatekeeper_decision(&v)
        .unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("missing_info_question") && e.contains("required")),
        "expected missing_info_question required error; got: {:?}", errs
    );
}

#[test]
fn ac2_1_invalid_decision_enum_rejected() {
    let v = json!({
        "decision": "not_a_real_decision",
        "confidence": "high",
        "rationale": "x"
    });
    let errs = stores::validate::gatekeeper_decision::validate_gatekeeper_decision(&v)
        .unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("not_a_real_decision")),
        "expected decision enum violation; got: {:?}", errs
    );
}

#[test]
fn ac2_1_risk_flags_enum_enforced() {
    let v = json!({
        "decision": "reject_noise",
        "confidence": "high",
        "rationale": "x",
        "risk_flags": ["definitely_not_a_flag"]
    });
    let errs = stores::validate::gatekeeper_decision::validate_gatekeeper_decision(&v)
        .unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("definitely_not_a_flag")),
        "expected risk flag enum violation; got: {:?}", errs
    );
}

// ---- AC2.2: fast_track with PROHIBIT flag rejected fail-loud ----

#[test]
fn ac2_2_fast_track_with_touches_lifecycle_rejected_fail_loud() {
    let schema = intake_schema();
    let conn = fresh_db(&schema);
    insert_triaging_row(&conn, "I001");

    // Build the add-like CLI command using the schema's leaf args
    let leaves = stores::schema::flatten::leaf_args(&schema).unwrap();
    let mut cmd = clap::Command::new("route")
        .arg(clap::Arg::new("display_id").required(true).index(1));
    for leaf in &leaves {
        cmd = cmd.arg(
            clap::Arg::new(leaf.cli_name.clone())
                .long(leaf.cli_name.clone())
                .required(false),
        );
    }

    let decision_json = json!({
        "decision": "fast_track",
        "confidence": "high",
        "rationale": "Looks small but touches lifecycle.",
        "tier_hint": "T1",
        "risk_flags": ["touches_lifecycle"]
    })
    .to_string();

    let matches = cmd.get_matches_from([
        "route",
        "I001",
        "--decision",
        "fast_track",
        "--gatekeeper-decision-json",
        &decision_json,
    ]);

    let err = transition::run(&schema, &conn, &matches, Actor::AiAutonomous.into(), "route")
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("PROHIBITED") && msg.contains("touches_lifecycle"),
        "expected PROHIBIT violation message; got: {msg}"
    );
}

// ---- AC2.3: valid route writes risk_flags + cluster_key to top-level columns ----

#[test]
fn ac2_3_valid_route_mirrors_risk_flags_and_cluster_key() {
    let schema = intake_schema();
    let conn = fresh_db(&schema);
    insert_triaging_row(&conn, "I001");

    let leaves = stores::schema::flatten::leaf_args(&schema).unwrap();
    let mut cmd = clap::Command::new("route")
        .arg(clap::Arg::new("display_id").required(true).index(1));
    for leaf in &leaves {
        cmd = cmd.arg(
            clap::Arg::new(leaf.cli_name.clone())
                .long(leaf.cli_name.clone())
                .required(false),
        );
    }

    let decision_json = json!({
        "decision": "normal_observation",
        "confidence": "medium",
        "rationale": "Stale PID cleanup needed.",
        "tier_hint": "T2",
        "risk_flags": ["small_local_fix"],
        "cluster_key": "dispatch-lifecycle"
    })
    .to_string();

    let matches = cmd.get_matches_from([
        "route",
        "I001",
        "--decision",
        "normal_observation",
        "--gatekeeper-decision-json",
        &decision_json,
        "--routed-to-observation",
        "L042",
    ]);

    transition::run(&schema, &conn, &matches, Actor::AiAutonomous.into(), "route").unwrap();

    // Verify top-level columns were populated without JSON parsing
    let (risk_flags_raw, cluster_key): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT risk_flags, cluster_key FROM intake WHERE display_id = 'I001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    let risk_flags_raw = risk_flags_raw.expect("risk_flags must be populated");
    let cluster_key = cluster_key.expect("cluster_key must be populated");

    assert!(
        risk_flags_raw.contains("small_local_fix"),
        "risk_flags must contain small_local_fix; got: {risk_flags_raw}"
    );
    assert_eq!(cluster_key, "dispatch-lifecycle");
}

// ---- AC2.4: derive_risk_class / derive_approval_policy matrix ----

#[test]
fn ac2_4_derive_risk_class_six_representative_vectors() {
    // Row 1 from docs/risk-and-cluster-taxonomy.md worked examples: T0 + docs_only → low
    assert_eq!(risk_taxonomy::derive_risk_class(&["docs_only"]), "low");

    // Row 2: T1 + small_local_fix → low
    assert_eq!(risk_taxonomy::derive_risk_class(&["small_local_fix"]), "low");

    // Row 3: touches_lifecycle + contradicts_prior_decision → architecture
    assert_eq!(
        risk_taxonomy::derive_risk_class(&["touches_lifecycle", "contradicts_prior_decision"]),
        "architecture"
    );

    // Row 4: touches_actor_authority + authority_surface_drift + security_sensitive → authority (wins)
    assert_eq!(
        risk_taxonomy::derive_risk_class(&[
            "touches_actor_authority",
            "authority_surface_drift",
            "security_sensitive"
        ]),
        "authority"
    );

    // Row 5: touches_lifecycle + touches_schema_core + changes_boundary → architecture
    assert_eq!(
        risk_taxonomy::derive_risk_class(&[
            "touches_lifecycle",
            "touches_schema_core",
            "changes_boundary"
        ]),
        "architecture"
    );

    // Row 6: no flags (ordinary substrate work) → normal per worked example row 6 in docs
    assert_eq!(risk_taxonomy::derive_risk_class(&[]), "normal");
}

#[test]
fn ac2_4_derive_approval_policy_six_representative_vectors() {
    // fast_track → always auto
    assert_eq!(risk_taxonomy::derive_approval_policy("T1", "low", "fast_track"), "auto");

    // T0 + normal → auto
    assert_eq!(risk_taxonomy::derive_approval_policy("T0", "normal", "normal_observation"), "auto");

    // T1 + normal → human
    assert_eq!(risk_taxonomy::derive_approval_policy("T1", "normal", "normal_observation"), "human");

    // T2 + architecture → architecture
    assert_eq!(
        risk_taxonomy::derive_approval_policy("T2", "architecture", "arch_review_candidate"),
        "architecture"
    );

    // T3 + normal → human
    assert_eq!(risk_taxonomy::derive_approval_policy("T3", "normal", "normal_observation"), "human");

    // T2 + security → architecture (security ≥ architecture gate)
    assert_eq!(
        risk_taxonomy::derive_approval_policy("T2", "security", "arch_review_candidate"),
        "architecture"
    );
}

// ---- AC2.5: decision_metadata captures gatekeeper rationale ----

#[test]
fn ac2_5_decision_metadata_populated_after_route() {
    let schema = intake_schema();
    let conn = fresh_db(&schema);
    insert_triaging_row(&conn, "I001");

    let leaves = stores::schema::flatten::leaf_args(&schema).unwrap();
    let mut cmd = clap::Command::new("route")
        .arg(clap::Arg::new("display_id").required(true).index(1));
    for leaf in &leaves {
        cmd = cmd.arg(
            clap::Arg::new(leaf.cli_name.clone())
                .long(leaf.cli_name.clone())
                .required(false),
        );
    }

    let decision_json = json!({
        "decision": "normal_observation",
        "confidence": "medium",
        "rationale": "Stale lock files need cleanup.",
        "tier_hint": "T2",
        "risk_flags": ["small_local_fix"],
        "cluster_key": "dispatch-lifecycle"
    })
    .to_string();

    let matches = cmd.get_matches_from([
        "route",
        "I001",
        "--decision",
        "normal_observation",
        "--gatekeeper-decision-json",
        &decision_json,
        "--routed-to-observation",
        "L099",
    ]);

    transition::run(&schema, &conn, &matches, Actor::AiAutonomous.into(), "route").unwrap();

    let meta_raw: Option<String> = conn
        .query_row(
            "SELECT decision_metadata FROM intake WHERE display_id = 'I001'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let meta_raw = meta_raw.expect("decision_metadata must be populated");
    let meta: serde_json::Value =
        serde_json::from_str(&meta_raw).expect("decision_metadata must be valid JSON");

    assert_eq!(
        meta.get("matched_cluster_key")
            .and_then(|v| v.as_str()),
        Some("dispatch-lifecycle"),
        "matched_cluster_key must be set"
    );
    assert_eq!(
        meta.get("risk_class_hint").and_then(|v| v.as_str()),
        Some("low"),
        "risk_class_hint must be 'low' for small_local_fix"
    );
    assert_eq!(
        meta.get("approval_policy_hint").and_then(|v| v.as_str()),
        Some("auto"),
        "approval_policy_hint must be 'auto' for T2+low per § 3 matrix"
    );
    assert_eq!(
        meta.get("rationale").and_then(|v| v.as_str()),
        Some("Stale lock files need cleanup."),
        "rationale from gatekeeper decision payload must be captured in decision_metadata"
    );
    assert_eq!(
        meta.get("confidence").and_then(|v| v.as_str()),
        Some("medium"),
        "confidence from gatekeeper decision payload must be captured in decision_metadata"
    );
    assert_eq!(
        meta.get("tier_hint").and_then(|v| v.as_str()),
        Some("T2"),
        "tier_hint from gatekeeper decision payload must be captured in decision_metadata"
    );
}

// ---- Additional: reject_noise routes to dropped without mirroring side-effects ----

#[test]
fn reject_noise_routes_to_dropped_state() {
    let schema = intake_schema();
    let conn = fresh_db(&schema);
    insert_triaging_row(&conn, "I001");

    let leaves = stores::schema::flatten::leaf_args(&schema).unwrap();
    let mut cmd = clap::Command::new("route")
        .arg(clap::Arg::new("display_id").required(true).index(1));
    for leaf in &leaves {
        cmd = cmd.arg(
            clap::Arg::new(leaf.cli_name.clone())
                .long(leaf.cli_name.clone())
                .required(false),
        );
    }

    let decision_json = json!({
        "decision": "reject_noise",
        "confidence": "high",
        "rationale": "Re-symptom of a fully-resolved issue."
    })
    .to_string();

    let matches = cmd.get_matches_from([
        "route",
        "I001",
        "--decision",
        "reject_noise",
        "--gatekeeper-decision-json",
        &decision_json,
    ]);

    transition::run(&schema, &conn, &matches, Actor::AiAutonomous.into(), "route").unwrap();

    let status: String = conn
        .query_row(
            "SELECT status FROM intake WHERE display_id = 'I001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "dropped");
}

// ---- Additional: route without gatekeeper_decision_json still works (nullable field) ----

#[test]
fn route_without_gatekeeper_decision_json_allowed() {
    // The gatekeeper_decision_json field is required: false in the schema.
    // A route can proceed without it (the validator only fires when the field is set).
    let schema = intake_schema();
    let conn = fresh_db(&schema);
    insert_triaging_row(&conn, "I001");

    let leaves = stores::schema::flatten::leaf_args(&schema).unwrap();
    let mut cmd = clap::Command::new("route")
        .arg(clap::Arg::new("display_id").required(true).index(1));
    for leaf in &leaves {
        cmd = cmd.arg(
            clap::Arg::new(leaf.cli_name.clone())
                .long(leaf.cli_name.clone())
                .required(false),
        );
    }

    // route without --gatekeeper-decision-json; required_when for duplicate_of fires
    // because decision=duplicate needs duplicate_of. Use reject_noise (no side-effects needed).
    let matches = cmd.get_matches_from([
        "route",
        "I001",
        "--decision",
        "reject_noise",
    ]);

    // Should succeed: no gatekeeper_decision_json validation runs for null field
    transition::run(&schema, &conn, &matches, Actor::AiAutonomous.into(), "route").unwrap();

    let status: String = conn
        .query_row(
            "SELECT status FROM intake WHERE display_id = 'I001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "dropped");
}
