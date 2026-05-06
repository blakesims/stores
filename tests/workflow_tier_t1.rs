//! T027 Phase 5 — end-to-end T1 drive shape.
//!
//! T1 tasks bypass planner + plan_reviewer (the contract IS the plan). The
//! row scaffolds at `planning`, the `when: tier_hint == 'T1'` follow-on fires
//! the `skip-plan` framework transition (planning → ready), the on-entry
//! `ready → executing` follow-on advances to executing, and drive then runs
//! executor → code-reviewer → wrap. Asserts:
//!
//!   * zero `planner` and zero `plan-reviewer` spawns
//!   * exactly one `skip-plan` row in transition_history on planning→ready
//!   * final status: in_review
//!   * runner role trace: [executor, code-reviewer, wrap]

use rusqlite::Connection;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::drive::drive_loop;
use stores::handlers::submit::fire_on_entry_follow_ons;
use stores::runner::mock::MockRunner;
use stores::runner::RunnerOutput;
use stores::schema::Schema;

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

fn make_run_output(stdout: &str) -> RunnerOutput {
    let last = stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|s| s.to_string());
    RunnerOutput {
        stdout: stdout.to_string(),
        stderr: String::new(),
        exit_code: 0,
        final_message: last,
        structured_output: None,
        session_id: None,
        structured_output_source: None,
    }
}

fn insert_t1_task_at_planning(conn: &Connection, display_id: &str) {
    let now = "2026-05-04T00:00:00Z";
    let contract = r#"{"done_when":"contract is the plan","scope_in":"a","scope_out":"b"}"#;
    // T1 rows have plan=NULL: contract IS the plan, so submit-review PASS
    // resolves through the schema's explicit `tier_hint == 'T1'` guard only.
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, tier_hint, contract, plan, \
         current_phase, current_cycle, cycles, plan_review_log, wrap_log, \
         created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'planning', 't1 task', 't1-task', 'T1', ?2, NULL, 0, 0, '[]', '[]', '[]', \
         ?3, ?3, 'human', 'human')",
        rusqlite::params![display_id, contract, now],
    )
    .unwrap();
}

#[test]
fn t1_drive_skips_planner_and_plan_reviewer() {
    let schema = tasks_schema();
    let conn = fresh_db();
    let display_id = "T001";
    insert_t1_task_at_planning(&conn, display_id);

    // Fire the initial on-entry follow-ons (planning→ready via skip-plan,
    // then ready→executing via the `start` framework transition). This is
    // the same hook `add::run` invokes after scaffolding a workflow row.
    let row_id: i64 = conn
        .query_row(
            "SELECT id FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    {
        let tx = conn.unchecked_transaction().unwrap();
        fire_on_entry_follow_ons(&tx, &schema, display_id, row_id, "planning").unwrap();
        tx.commit().unwrap();
    }

    let status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        status, "executing",
        "T1 row must auto-advance planning→ready→executing after on-entry follow-ons"
    );

    let skip_plan_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='tasks' AND display_id=?1 \
               AND from_status='planning' AND to_status='ready' AND verb='skip-plan'",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        skip_plan_count, 1,
        "transition_history must contain exactly one skip-plan row on planning→ready"
    );

    let executor_out = make_run_output(
        r#"{"role":"executor","summary":"did the thing","commit":"abc","files_changed":["src/x.rs"]}"#,
    );
    let code_reviewer_out = make_run_output(
        r#"{"role":"code-reviewer","gate":"PASS","summary":"looks good","critical":0,"major":0,"minor":0}"#,
    );
    let wrap_out = make_run_output(
        r#"{"role":"wrap","executive_summary":"done","deviations":[],"residual_risks":[],"recommended_sanity_checks":[]}"#,
    );
    let runner = MockRunner::new(vec![executor_out, code_reviewer_out, wrap_out]);

    drive_loop(&schema, &conn, display_id, &runner, 50).expect("drive_loop should succeed");

    let final_status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        final_status, "in_review",
        "T1 task must end at in_review after drive (executor → code-reviewer → wrap)"
    );

    let roles = runner.roles_seen();
    assert_eq!(
        runner.remaining_count(),
        0,
        "all 3 mock outputs must be consumed; roles seen: {:?}",
        roles
    );
    assert!(
        !roles.iter().any(|r| r == "planner"),
        "planner must never be dispatched on T1; roles: {:?}",
        roles
    );
    assert!(
        !roles.iter().any(|r| r == "plan-reviewer"),
        "plan-reviewer must never be dispatched on T1; roles: {:?}",
        roles
    );
    assert_eq!(
        roles,
        vec![
            "executor".to_string(),
            "code-reviewer".to_string(),
            "wrap".to_string()
        ],
        "T1 dispatch trace must be exactly executor → code-reviewer → wrap"
    );

    // Sanity: also assert via transition_history that no plan-related verbs
    // ever fired (no submit-plan, no submit-plan-review).
    let plan_verbs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='tasks' AND display_id=?1 \
               AND verb IN ('submit-plan', 'submit-plan-review')",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        plan_verbs, 0,
        "T1 row must not record any submit-plan or submit-plan-review transitions"
    );

    // T1 plan remains NULL: contract remains the plan.
    let plan_str: Option<String> = conn
        .query_row(
            "SELECT plan FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(plan_str, None);
}
