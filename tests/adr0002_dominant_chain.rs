use clap::{Arg, Command};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::PathBuf;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::builtins::{auto_promote, DispatchCtx};
use stores::flow::AgentsYaml;
use stores::handlers::transition;
use stores::schema::{actor::Actor, flatten::leaf_args, Schema};

fn schema(name: &str) -> Schema {
    Schema::from_yaml(BUNDLED_STORE_SCHEMAS.iter().find(|(n, _)| *n == name).unwrap().1).unwrap()
}

fn setup() -> (Schema, Connection) {
    let intake = schema("intake");
    let observations = schema("observations");
    let tasks = schema("tasks");
    let architecture_reviews = schema("architecture_reviews");
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(&intake)).unwrap();
    conn.execute_batch(&ddl_for(&observations)).unwrap();
    conn.execute_batch(&ddl_for(&tasks)).unwrap();
    conn.execute_batch(&ddl_for(&architecture_reviews)).unwrap();
    (intake, conn)
}

fn cmd(schema: &Schema, verb: &'static str) -> Command {
    let mut cmd = Command::new(verb).arg(Arg::new("display_id").required(true).index(1));
    for leaf in leaf_args(schema).unwrap() {
        cmd = cmd.arg(Arg::new(leaf.cli_name.clone()).long(leaf.cli_name).required(false));
    }
    cmd
}

fn insert_draft(conn: &Connection, id: &str) {
    conn.execute("INSERT INTO intake (display_id,status,lifecycle,summary,source_agent,captured_at,captured_week,created_at,updated_at,created_by,updated_by) VALUES (?1,'draft','new','inlet','executor','2026-05-11T00:00:00Z','w20-d1','now','now','ai_autonomous','ai_autonomous')", [id]).unwrap();
}

fn ctx(conn: &Connection) -> DispatchCtx<'_> {
    let agents: &'static AgentsYaml = Box::leak(Box::new(AgentsYaml::default_empty()));
    let cfg: &'static PathBuf = Box::leak(Box::new(PathBuf::from("/tmp/t148-dominant.yaml")));
    DispatchCtx { conn, agents, config_path: cfg, policies_hash: "" }
}

fn row(conn: &Connection, table: &str, id: &str) -> Value {
    let mut stmt = conn.prepare(&format!("SELECT * FROM {table} WHERE display_id=?1")).unwrap();
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    stmt.query_row([id], |r| {
        let mut obj = serde_json::Map::new();
        for (i, c) in cols.iter().enumerate() {
            let v: rusqlite::types::Value = r.get(i)?;
            obj.insert(c.clone(), match v {
                rusqlite::types::Value::Null => Value::Null,
                rusqlite::types::Value::Integer(n) => json!(n),
                rusqlite::types::Value::Real(f) => json!(f),
                rusqlite::types::Value::Text(s) => json!(s),
                rusqlite::types::Value::Blob(b) => json!(String::from_utf8_lossy(&b).to_string()),
            });
        }
        Ok(Value::Object(obj))
    }).unwrap()
}

fn claim_and_route(schema: &Schema, conn: &Connection, id: &str, decision: &str) {
    transition::run(schema, conn, &cmd(schema, "claim-triage").get_matches_from(["claim-triage", id]), Actor::AiAutonomous.into(), "claim-triage").unwrap();
    let decision_json = if decision == "normal_observation" {
        json!({"decision":decision,"confidence":"high","rationale":"route","tier_hint":"T1","cluster_key":"gatekeeper-front-door-stuck","risk_flags":["docs_only"]})
    } else {
        json!({"decision":decision,"confidence":"high","rationale":"route","tier_hint":"T1","risk_flags":["docs_only"]})
    }
    .to_string();
    transition::run(schema, conn, &cmd(schema, "route").get_matches_from(["route", id, "--decision", decision, "--gatekeeper-decision-json", &decision_json]), Actor::AiAutonomous.into(), "route").unwrap();
}

#[test]
fn fast_track_inlet_closes_with_complete_causal_trail() {
    let (intake, conn) = setup();
    insert_draft(&conn, "I001");
    claim_and_route(&intake, &conn, "I001", "fast_track");
    let got: (String, String, String, String, String) = conn.query_row("SELECT lifecycle,outcome,produced_observation_id,produced_artifact_kind,produced_artifact_id FROM intake WHERE display_id='I001'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap();
    assert_eq!(got.0, "closed");
    assert_eq!(got.1, "fast_tracked");
    assert!(got.2.starts_with('L'));
    assert_eq!(got.3, "observation");
    assert_eq!(got.4, got.2);
}

#[test]
fn dominant_chain_ratifies_primary_fields_end_to_end() {
    let (intake, conn) = setup();
    insert_draft(&conn, "I002");
    claim_and_route(&intake, &conn, "I002", "normal_observation");
    let (inlet_outcome, obs_id): (String, String) = conn.query_row("SELECT outcome,produced_observation_id FROM intake WHERE display_id='I002'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let ic = json!({"contract_state":"approved","objective":"dominant task","acceptance":["ok"],"in_scope":["code"],"out_of_scope":["other"],"tier_hint":"T1"});
    conn.execute("UPDATE observations SET status='ready', lifecycle='ready', contract_state='approved', waiting=0, intent_contract=?1 WHERE display_id=?2", rusqlite::params![ic.to_string(), obs_id]).unwrap();
    auto_promote::run(&row(&conn, "observations", &obs_id), &ctx(&conn)).unwrap();
    let obs: (String, String) = conn.query_row("SELECT contract_state,lifecycle FROM observations WHERE display_id=?1", [&obs_id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let linked: String = conn.query_row("SELECT linked_observations FROM tasks WHERE display_id='T001'", [], |r| r.get(0)).unwrap();
    assert_eq!(inlet_outcome, "routed_to_observation");
    assert_eq!(obs.0, "approved");
    assert_eq!(obs.1, "ready");
    assert!(linked.contains(&obs_id));
}
