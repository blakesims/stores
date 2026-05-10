use clap::{Arg, Command};
use rusqlite::Connection;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::{lifecycle_overlay, submit, transition};
use stores::schema::actor::Actor;
use stores::schema::Schema;

fn tasks_schema() -> Schema {
    Schema::from_yaml(include_str!("../stores/tasks/schema.yaml")).unwrap()
}

fn matches(id: &str) -> clap::ArgMatches {
    Command::new("t")
        .arg(Arg::new("display_id").required(true))
        .try_get_matches_from(["t", id])
        .unwrap()
}

fn setup(status: &str, tier: &str) -> (Schema, Connection) {
    let schema = tasks_schema();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(&schema)).unwrap();
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, tier_hint, activation, current_phase, current_cycle, contract, plan) VALUES ('T001', ?1, 't', 't001', ?2, 'active', 1, 1, ?3, ?4)",
        rusqlite::params![
            status,
            tier,
            r#"{"done_when":"done","scope_in":"in","scope_out":"out"}"#,
            r#"{"phases":[{"name":"p"}]}"#
        ],
    )
    .unwrap();
    (schema, conn)
}

fn row(conn: &Connection) -> (String, String, String, String, i64, Option<String>) {
    conn.query_row(
        "SELECT status, lifecycle, active_step, integration_step, blocked, blocker_kind FROM tasks WHERE display_id='T001'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    )
    .unwrap()
}

#[test]
fn t1_walk_updates_overlay_on_every_edge() {
    let (schema, conn) = setup("planning", "T1");
    let m = matches("T001");

    transition::run(&schema, &conn, &m, Actor::Framework.into(), "skip-plan").unwrap();
    assert_eq!(
        row(&conn),
        (
            "ready".into(),
            "active".into(),
            "none".into(),
            "none".into(),
            0,
            None
        )
    );

    transition::run(&schema, &conn, &m, Actor::Framework.into(), "start").unwrap();
    assert_eq!(
        row(&conn),
        (
            "executing".into(),
            "active".into(),
            "coding".into(),
            "none".into(),
            0,
            None
        )
    );

    submit::run_submit_execute(
        &schema,
        &conn,
        "T001",
        "done",
        Some("abc1234"),
        Some("src/x.rs"),
        None,
        Actor::AiAutonomous.into(),
    )
    .unwrap();
    assert_eq!(
        row(&conn),
        (
            "code_review".into(),
            "active".into(),
            "coding_review".into(),
            "none".into(),
            0,
            None
        )
    );

    submit::run_submit_review(
        &schema,
        &conn,
        "T001",
        "PASS",
        "ok",
        None,
        0,
        0,
        0,
        Actor::AiAutonomous.into(),
    )
    .unwrap();
    assert_eq!(
        row(&conn),
        (
            "in_review".into(),
            "integration".into(),
            "wrapping".into(),
            "none".into(),
            0,
            None
        )
    );
}

#[test]
fn lock_busy_merge_failure_reason_maps_to_main_red() {
    let overlay = lifecycle_overlay::derive(
        "mark_integration_blocked",
        "integrating",
        "integration_blocked",
        None,
        Some("merge_failure: main_branch lock held by T_OTHER; will retry"),
    )
    .unwrap();
    assert_eq!(overlay.lifecycle, "integration");
    assert_eq!(overlay.integration_step, "none");
    assert!(overlay.blocked);
    assert_eq!(overlay.blocker_kind, Some("main_red".into()));
}

#[test]
fn t3_review_revise_fallback_blocks_with_task_review_kind() {
    let (schema, conn) = setup("plan_review", "T3");
    submit::run_submit_plan_review(
        &schema,
        &conn,
        "T001",
        "READY",
        "ready",
        None,
        Actor::AiAutonomous.into(),
    )
    .unwrap();
    for i in 1..=4 {
        submit::run_submit_execute(
            &schema,
            &conn,
            "T001",
            "done",
            Some("abc1234"),
            Some("src/x.rs"),
            None,
            Actor::AiAutonomous.into(),
        )
        .unwrap();
        let _ = submit::run_submit_review(
            &schema,
            &conn,
            "T001",
            "REVISE",
            "revise",
            None,
            0,
            1,
            0,
            Actor::AiAutonomous.into(),
        );
        if i < 4 {
            assert_eq!(row(&conn).0, "executing");
        }
    }

    assert_eq!(
        row(&conn),
        (
            "blocked".into(),
            "active".into(),
            "none".into(),
            "none".into(),
            1,
            Some("task_review".into())
        )
    );
}
