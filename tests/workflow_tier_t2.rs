//! T027 Phase 5 — T2 phase-count gate end-to-end.
//!
//! T2 plans must contain exactly one phase. submit-plan with a 2-phase plan
//! must error (status unchanged); submit-plan with a 1-phase plan must
//! advance the row to plan_review.

use rusqlite::Connection;
use serde_json::json;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::submit::run_submit_plan;
use stores::schema::{actor::Actor, Schema};

fn tasks_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "tasks")
        .map(|(_, y)| *y)
        .unwrap();
    Schema::from_yaml(yaml).unwrap()
}

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    let s = tasks_schema();
    conn.execute_batch(&ddl_for(&s)).unwrap();
    conn
}

fn insert_t2_task_at_planning(conn: &Connection, display_id: &str) {
    let now = "2026-05-04T00:00:00Z";
    let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, tier_hint, contract, \
         current_phase, current_cycle, cycles, plan_review_log, wrap_log, \
         created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'planning', 't2 task', 't2-task', 'T2', ?2, 0, 0, '[]', '[]', '[]', \
         ?3, ?3, 'human', 'human')",
        rusqlite::params![display_id, contract, now],
    )
    .unwrap();
}

#[test]
fn t2_two_phase_plan_rejected_one_phase_accepted() {
    let schema = tasks_schema();
    let conn = fresh_db();
    let display_id = "T010";
    insert_t2_task_at_planning(&conn, display_id);

    // Two-phase plan: must error.
    let two_phase = json!({
        "objective": "two-phase plan",
        "phases": [
            {
                "name": "p1",
                "objective": "first",
                "tasks": [],
                "acceptance_criteria": [],
                "files": [],
                "dependencies": []
            },
            {
                "name": "p2",
                "objective": "second",
                "tasks": [],
                "acceptance_criteria": [],
                "files": [],
                "dependencies": []
            }
        ]
    });

    let err = run_submit_plan(
        &schema,
        &conn,
        display_id,
        two_phase,
        Actor::AiAutonomous.into(),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("tier T2 requires phases.length == 1"),
        "expected T2 phase-count error, got: {msg}"
    );

    let status_after_reject: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status_after_reject, "planning",
        "T2 row must remain at planning after submit-plan rejection"
    );

    // Retry with one phase: must succeed and advance to plan_review.
    let one_phase = json!({
        "objective": "single-phase plan",
        "phases": [
            {
                "name": "only",
                "objective": "do it all",
                "tasks": [],
                "acceptance_criteria": [],
                "files": [],
                "dependencies": []
            }
        ]
    });

    run_submit_plan(
        &schema,
        &conn,
        display_id,
        one_phase,
        Actor::AiAutonomous.into(),
    )
    .expect("T2 submit-plan with phases.length == 1 must succeed");

    let status_after_accept: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status_after_accept, "plan_review",
        "T2 row must advance to plan_review after a 1-phase submit-plan"
    );
}
