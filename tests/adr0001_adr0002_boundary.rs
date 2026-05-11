use clap::{Arg, ArgAction, Command};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::PathBuf;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::builtins::{auto_promote, DispatchCtx};
use stores::flow::AgentsYaml;
use stores::handlers::{add, transition};
use stores::schema::{actor::Actor, flatten::leaf_args, Schema};

fn schema(name: &str) -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, y)| *y)
        .expect("bundled schema");
    Schema::from_yaml(yaml).unwrap()
}

fn setup() -> (Schema, Schema, Connection) {
    let observations = schema("observations");
    let tasks = schema("tasks");
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(&observations)).unwrap();
    conn.execute_batch(&ddl_for(&tasks)).unwrap();
    (observations, tasks, conn)
}

fn close_cmd(schema: &Schema) -> Command {
    let mut cmd = Command::new("close_as_addressed").arg(Arg::new("display_id").required(true));
    for leaf in leaf_args(schema).unwrap() {
        if leaf.cli_name != "resolution" {
            cmd = cmd.arg(Arg::new(leaf.cli_name.clone()).long(leaf.cli_name).required(false));
        }
    }
    cmd.arg(Arg::new("resolution").long("resolution").required(true))
}

fn task_add_cmd() -> Command {
    Command::new("add")
        .arg(Arg::new("title").long("title"))
        .arg(Arg::new("slug").long("slug"))
        .arg(Arg::new("done-when").long("done-when"))
        .arg(Arg::new("scope-in").long("scope-in"))
        .arg(Arg::new("scope-out").long("scope-out"))
        .arg(Arg::new("tier-hint").long("tier-hint"))
        .arg(Arg::new("linked-observations").long("linked-observations").action(ArgAction::Append))
}

fn ctx(conn: &Connection) -> DispatchCtx<'_> {
    let agents: &'static AgentsYaml = Box::leak(Box::new(AgentsYaml::default_empty()));
    let cfg: &'static PathBuf = Box::leak(Box::new(PathBuf::from("/tmp/t148-boundary.yaml")));
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

#[test]
fn closing_observation_addressed_by_missing_task_fails_loud() {
    let (obs, _tasks, conn) = setup();
    conn.execute("INSERT INTO observations (display_id,status,lifecycle,contract_state,waiting,summary,source,priority,captured_at,captured_week,created_at,updated_at,created_by,updated_by) VALUES ('L001','open','candidate','draft',0,'s','dev','normal','2026-05-11','w20-d1','now','now','human','human')", []).unwrap();
    let matches = close_cmd(&obs).get_matches_from(["close_as_addressed", "L001", "--resolution", "T999"]);
    let err = transition::run_close_as_addressed(&obs, &conn, &matches, Actor::AiAutonomous.into(), "T999").unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");
}

#[test]
fn task_add_can_link_closed_observation_historically() {
    let (_obs, tasks, conn) = setup();
    conn.execute("INSERT INTO observations (display_id,status,lifecycle,contract_state,waiting,outcome,summary,source,priority,captured_at,captured_week,created_at,updated_at,created_by,updated_by,intent_contract) VALUES ('L010','resolved','closed','approved',0,'addressed_by_task','closed obs','dev','normal','2026-05-11','w20-d1','now','now','human','human',?1)", [json!({"tier_hint":"T2"}).to_string()]).unwrap();
    let matches = task_add_cmd().get_matches_from(["add", "--title", "historical link", "--slug", "historical-link", "--done-when", "done", "--scope-in", "in", "--scope-out", "out", "--linked-observations", "L010"]);
    add::run(&tasks, &conn, &matches, Actor::AiWithHuman.into()).unwrap();
    let linked: String = conn.query_row("SELECT linked_observations FROM tasks WHERE display_id='T001'", [], |r| r.get(0)).unwrap();
    assert!(linked.contains("L010"));
}

#[test]
fn adr0002_upstream_ratification_preserves_adr0001_task_lifecycle() {
    let (_obs, _tasks, conn) = setup();
    let ic = json!({"contract_state":"approved","objective":"ship boundary","acceptance":["ok"],"in_scope":["code"],"out_of_scope":["docs"],"tier_hint":"T2"});
    conn.execute("INSERT INTO observations (display_id,status,lifecycle,contract_state,waiting,summary,source,priority,captured_at,captured_week,created_at,updated_at,created_by,updated_by,intent_contract) VALUES ('L020','ready','ready','approved',0,'ratified obs','dev','normal','2026-05-11','w20-d1','now','now','human','human',?1)", [ic.to_string()]).unwrap();
    auto_promote::run(&row(&conn, "observations", "L020"), &ctx(&conn)).unwrap();
    let got: (String, String, String) = conn.query_row("SELECT lifecycle,active_step,linked_observations FROM tasks WHERE display_id='T001'", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
    assert_eq!(got.0, "active");
    assert_eq!(got.1, "planning");
    assert!(got.2.contains("L020"));
}
