use clap::{Arg, Command};
use rusqlite::Connection;
use serde_json::json;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::transition;
use stores::schema::{actor::Actor, flatten::leaf_args, Schema};

fn schema(name: &str) -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, y)| *y)
        .expect("bundled schema");
    Schema::from_yaml(yaml).unwrap()
}

fn cmd(schema: &Schema, verb: &'static str) -> Command {
    let mut cmd = Command::new(verb).arg(Arg::new("display_id").required(true).index(1));
    for leaf in leaf_args(schema).unwrap() {
        cmd = cmd.arg(Arg::new(leaf.cli_name.clone()).long(leaf.cli_name).required(false));
    }
    cmd
}

fn setup() -> (Schema, Connection) {
    let intake = schema("intake");
    let observations = schema("observations");
    let architecture_reviews = schema("architecture_reviews");
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(&intake)).unwrap();
    conn.execute_batch(&ddl_for(&observations)).unwrap();
    conn.execute_batch(&ddl_for(&architecture_reviews)).unwrap();
    (intake, conn)
}

fn insert_draft(conn: &Connection, id: &str) {
    conn.execute(
        "INSERT INTO intake (display_id,status,summary,source_agent,captured_at,captured_week,created_at,updated_at,created_by,updated_by) \
         VALUES (?1,'draft','smoke intake','executor','2026-05-11T00:00:00Z','w20-d1','2026-05-11T00:00:00Z','2026-05-11T00:00:00Z','ai_autonomous','ai_autonomous')",
        [id],
    )
    .unwrap();
}

#[test]
fn draft_to_triaging_to_routed_normal_writes_primary_tuple_and_produced_observation() {
    let (schema, conn) = setup();
    insert_draft(&conn, "I001");

    let claim = cmd(&schema, "claim-triage").get_matches_from(["claim-triage", "I001"]);
    transition::run(&schema, &conn, &claim, Actor::AiAutonomous.into(), "claim-triage").unwrap();

    let decision_json = json!({
        "decision": "normal_observation",
        "confidence": "high",
        "rationale": "needs observation contract",
        "tier_hint": "T2",
        "cluster_key": "gatekeeper-front-door-stuck",
        "risk_flags": ["docs_only"]
    })
    .to_string();
    let route = cmd(&schema, "route").get_matches_from([
        "route",
        "I001",
        "--decision",
        "normal_observation",
        "--gatekeeper-decision-json",
        &decision_json,
    ]);
    transition::run(&schema, &conn, &route, Actor::AiAutonomous.into(), "route").unwrap();

    let row: (String, String, Option<String>, String, String, String) = conn
        .query_row(
            "SELECT status,lifecycle,waiting_kind,outcome,routed_to_observation,produced_observation_id FROM intake WHERE display_id='I001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(row.0, "routed");
    assert_eq!(row.1, "closed");
    assert_eq!(row.2, None);
    assert_eq!(row.3, "routed_to_observation");
    assert!(row.4.starts_with('L'));
    assert_eq!(row.5, row.4);
}

#[test]
fn fast_track_writes_outcome_and_artifact_causal_trail() {
    let (schema, conn) = setup();
    insert_draft(&conn, "I001");
    let claim = cmd(&schema, "claim-triage").get_matches_from(["claim-triage", "I001"]);
    transition::run(&schema, &conn, &claim, Actor::AiAutonomous.into(), "claim-triage").unwrap();

    let decision_json = json!({
        "decision": "fast_track",
        "confidence": "high",
        "rationale": "small local docs-only route",
        "tier_hint": "T1",
        "risk_flags": ["docs_only"]
    })
    .to_string();
    let route = cmd(&schema, "route").get_matches_from([
        "route",
        "I001",
        "--decision",
        "fast_track",
        "--gatekeeper-decision-json",
        &decision_json,
    ]);
    transition::run(&schema, &conn, &route, Actor::AiAutonomous.into(), "route").unwrap();

    let row: (String, String, String, String, String, String) = conn
        .query_row(
            "SELECT status,lifecycle,outcome,produced_observation_id,produced_artifact_kind,produced_artifact_id FROM intake WHERE display_id='I001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(row.0, "routed");
    assert_eq!(row.1, "closed");
    assert_eq!(row.2, "fast_tracked");
    assert!(row.3.starts_with('L'));
    assert_eq!(row.4, "observation");
    assert_eq!(row.5, row.3);
}
