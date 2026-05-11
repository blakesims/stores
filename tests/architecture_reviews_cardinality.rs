use clap::{Arg, ArgAction, Command};
use rusqlite::Connection;
use serde_json::json;
use stores::handlers::{add, architecture_reviews};
use stores::schema::{actor::Actor, Schema};

fn arch_schema() -> Schema {
    Schema::from_yaml(include_str!("../stores/architecture_reviews/schema.yaml")).unwrap()
}

fn obs_schema() -> Schema {
    Schema::from_yaml(include_str!("../stores/observations/schema.yaml")).unwrap()
}

fn setup() -> (Schema, Connection) {
    let arch = arch_schema();
    let obs = obs_schema();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&stores::codegen::ddl::ddl_for(&obs))
        .unwrap();
    conn.execute_batch(&stores::codegen::ddl::ddl_for(&arch))
        .unwrap();
    for id in ["L010", "L011", "L012", "L100"] {
        conn.execute(
            "INSERT INTO observations (display_id,status,lifecycle,contract_state,waiting,created_at,updated_at,created_by,updated_by,summary,source,priority,captured_at,captured_week,pending_architecture_review,intent_contract) VALUES (?1,'confirmed','candidate','draft',0,'now','now','human','human','s','dev','normal','2026-05-07','w19-d4',0,?2)",
            rusqlite::params![id, json!({"updated_at":"2026-05-07T10:00:00Z"}).to_string()],
        ).unwrap();
    }
    (arch, conn)
}

fn add_cmd() -> Command {
    Command::new("add")
        .arg(Arg::new("kind").long("kind"))
        .arg(Arg::new("summary").long("summary"))
        .arg(Arg::new("source-observation").long("source-observation"))
        .arg(
            Arg::new("linked-observations")
                .long("linked-observations")
                .action(ArgAction::Append),
        )
}

fn issue_cmd() -> Command {
    Command::new("issue-verdict")
        .arg(Arg::new("display_id").required(true))
        .arg(Arg::new("kind").long("kind"))
        .arg(Arg::new("verdict").long("verdict"))
        .arg(Arg::new("rationale").long("rationale"))
        .arg(Arg::new("merge-target-id").long("merge-target-id"))
        .arg(Arg::new("produced-task-id").long("produced-task-id"))
}

fn supersede_cmd() -> Command {
    Command::new("supersede").arg(Arg::new("display_id").required(true))
}

fn withdraw_cmd() -> Command {
    Command::new("withdraw").arg(Arg::new("display_id").required(true))
}

fn insert_review(conn: &Connection, id: &str, verdict: Option<&str>, extra: &str) {
    conn.execute(
        &format!("INSERT INTO architecture_reviews (display_id,status,lifecycle,created_at,updated_at,created_by,updated_by,kind,summary,linked_observation_ids,verdict,rationale{extra}) VALUES (?1,'in_review','reviewing','now','now','ai_with_human','ai_with_human','interpret','s',?2,?3,'why')"),
        rusqlite::params![id, json!(["L010","L011","L012"]).to_string(), verdict],
    ).unwrap();
}

fn issue(
    arch: &Schema,
    conn: &Connection,
    id: &str,
    verdict: &str,
    more: &[&str],
) -> anyhow::Result<()> {
    let mut argv = vec![
        "issue-verdict",
        id,
        "--kind",
        "interpret",
        "--verdict",
        verdict,
        "--rationale",
        "why",
    ];
    argv.extend_from_slice(more);
    let m = issue_cmd().get_matches_from(argv);
    architecture_reviews::run_issue_verdict(arch, conn, &m, Actor::AiWithHuman.into())
}

fn obs_tuple(
    conn: &Connection,
    id: &str,
) -> (
    String,
    i64,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    conn.query_row(
        "SELECT status,waiting,waiting_kind,outcome,pending_architecture_review,open_architecture_review_id,task_id,resolved_by FROM observations WHERE display_id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
    ).unwrap()
}

#[test]
fn add_rejects_second_open_gate_naming_existing_review() {
    let (arch, conn) = setup();
    let m = add_cmd().get_matches_from([
        "add",
        "--kind",
        "interpret",
        "--summary",
        "cluster",
        "--linked-observations",
        "L010,L011,L012",
    ]);
    add::run(&arch, &conn, &m, Actor::AiWithHuman.into()).unwrap();
    let m2 = add_cmd().get_matches_from([
        "add",
        "--kind",
        "interpret",
        "--summary",
        "dupe",
        "--linked-observations",
        "L010",
    ]);
    let err = add::run(&arch, &conn, &m2, Actor::AiWithHuman.into()).unwrap_err();
    assert!(err.to_string().contains("A001"));
}

#[test]
fn add_includes_source_observation_in_linked_list() {
    let (arch, conn) = setup();
    let m = add_cmd().get_matches_from([
        "add",
        "--kind",
        "interpret",
        "--summary",
        "cluster",
        "--source-observation",
        "L010",
        "--linked-observations",
        "L011,L012",
    ]);
    add::run(&arch, &conn, &m, Actor::AiWithHuman.into()).unwrap();
    let linked: String = conn
        .query_row(
            "SELECT linked_observation_ids FROM architecture_reviews WHERE display_id='A001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(linked.contains("L010") && linked.contains("L011") && linked.contains("L012"));
}

#[test]
fn merge_with_cluster_updates_all_three() {
    let (arch, conn) = setup();
    insert_review(&conn, "A001", None, "");
    issue(
        &arch,
        &conn,
        "A001",
        "merge_with_cluster",
        &["--merge-target-id", "L100"],
    )
    .unwrap();
    for id in ["L010", "L011", "L012"] {
        let row = obs_tuple(&conn, id);
        assert_eq!(row.0, "resolved");
        assert_eq!(row.3.as_deref(), Some("merged_with_cluster"));
        assert_eq!(row.7.as_deref(), Some("A001"));
    }
}

#[test]
fn merge_with_cluster_rollback_on_third_failure() {
    let (arch, conn) = setup();
    conn.execute("DELETE FROM observations WHERE display_id='L012'", [])
        .unwrap();
    insert_review(&conn, "A001", None, "");
    assert!(issue(
        &arch,
        &conn,
        "A001",
        "merge_with_cluster",
        &["--merge-target-id", "L100"]
    )
    .is_err());
    for id in ["L010", "L011"] {
        assert_eq!(obs_tuple(&conn, id).0, "confirmed");
    }
    let status: String = conn
        .query_row(
            "SELECT status FROM architecture_reviews WHERE display_id='A001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "in_review");
}

#[test]
fn local_fix_allowed_clears_all_gates() {
    let (arch, conn) = setup();
    insert_review(&conn, "A001", None, "");
    issue(&arch, &conn, "A001", "allow_local_fix", &[]).unwrap();
    for id in ["L010", "L011", "L012"] {
        assert_eq!(obs_tuple(&conn, id).4, 0);
    }
}

#[test]
fn contract_reframe_required_keeps_architecture_gate() {
    let (arch, conn) = setup();
    insert_review(&conn, "A001", None, "");
    issue(&arch, &conn, "A001", "reframe_contract", &[]).unwrap();
    for id in ["L010", "L011", "L012"] {
        let r = obs_tuple(&conn, id);
        assert_eq!(r.2.as_deref(), Some("architecture_review"));
        assert_eq!(r.5.as_deref(), Some("A001"));
    }
}

#[test]
fn primitive_task_created_writes_arch_and_all_observations() {
    let (arch, conn) = setup();
    insert_review(&conn, "A001", None, "");
    issue(
        &arch,
        &conn,
        "A001",
        "create_primitive_task",
        &["--produced-task-id", "T777"],
    )
    .unwrap();
    let produced: String = conn
        .query_row(
            "SELECT produced_task_id FROM architecture_reviews WHERE display_id='A001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(produced, "T777");
    for id in ["L010", "L011", "L012"] {
        let r = obs_tuple(&conn, id);
        assert_eq!(r.6.as_deref(), Some("T777"));
        assert_eq!(r.4, 0);
    }
}

#[test]
fn primitive_task_required_sets_linked_task_blocked() {
    let (arch, conn) = setup();
    insert_review(&conn, "A001", None, "");
    issue(&arch, &conn, "A001", "block_pending_fixes", &[]).unwrap();
    for id in ["L010", "L011", "L012"] {
        assert_eq!(
            obs_tuple(&conn, id).2.as_deref(),
            Some("linked_task_blocked")
        );
    }
}

#[test]
fn human_decision_required_sets_human_ratification() {
    let (arch, conn) = setup();
    insert_review(&conn, "A001", None, "");
    issue(&arch, &conn, "A001", "request_human_arch_decision", &[]).unwrap();
    for id in ["L010", "L011", "L012"] {
        assert_eq!(
            obs_tuple(&conn, id).2.as_deref(),
            Some("human_ratification")
        );
    }
}

#[test]
fn superseded_redirects_to_superseding_review() {
    let (arch, conn) = setup();
    conn.execute("INSERT INTO architecture_reviews (display_id,status,lifecycle,created_at,updated_at,created_by,updated_by,kind,summary,linked_observation_ids,superseded_by_id) VALUES ('A001','in_review','reviewing','now','now','ai_with_human','ai_with_human','interpret','s',?1,'A002')", [json!(["L010","L011","L012"]).to_string()]).unwrap();
    let m = supersede_cmd().get_matches_from(["supersede", "A001"]);
    architecture_reviews::run_supersede(&arch, &conn, &m, Actor::AiWithHuman.into()).unwrap();
    for id in ["L010", "L011", "L012"] {
        assert_eq!(obs_tuple(&conn, id).5.as_deref(), Some("A002"));
    }
}

#[test]
fn withdrawn_clears_linked_observations() {
    let (arch, conn) = setup();
    insert_review(&conn, "A001", None, "");
    conn.execute("UPDATE observations SET waiting=1, waiting_kind='architecture_review', pending_architecture_review=1, open_architecture_review_id='A001' WHERE display_id IN ('L010','L011','L012')", []).unwrap();
    let m = withdraw_cmd().get_matches_from(["withdraw", "A001"]);
    architecture_reviews::run_withdraw(&arch, &conn, &m, Actor::AiWithHuman.into()).unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM architecture_reviews WHERE display_id='A001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "withdrawn");
    for id in ["L010", "L011", "L012"] {
        let r = obs_tuple(&conn, id);
        assert_eq!(r.1, 0);
        assert!(r.2.is_none());
        assert_eq!(r.4, 0);
        assert!(r.5.is_none());
    }
}
