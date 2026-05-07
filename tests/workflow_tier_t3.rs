//! T027 Phase 5 — T3 unchanged: full 5-stage drive cycle still runs.
//!
//! T3 tasks (or tier_hint unset) take the planner → plan-reviewer → executor
//! → code-reviewer → wrap path. This regression-guards against the T1/T2
//! tier-shape changes leaking into the T3 path.

use rusqlite::Connection;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::drive::drive_loop;
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
        payload_error: None,
        telemetry: stores::runner::AgentRunTelemetry::with_mock_defaults(),
    }
}

fn insert_t3_task_at_planning(conn: &Connection, display_id: &str) {
    let now = "2026-05-04T00:00:00Z";
    let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, tier_hint, contract, \
         current_phase, current_cycle, cycles, plan_review_log, wrap_log, \
         created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'planning', 't3 task', 't3-task', 'T3', ?2, 0, 0, '[]', '[]', '[]', \
         ?3, ?3, 'human', 'human')",
        rusqlite::params![display_id, contract, now],
    )
    .unwrap();
}

fn planner_fixture() -> &'static str {
    include_str!("fixtures/agent_outputs/planner.json")
}
fn plan_reviewer_fixture() -> &'static str {
    include_str!("fixtures/agent_outputs/plan-reviewer.json")
}
fn executor_fixture() -> &'static str {
    include_str!("fixtures/agent_outputs/executor.json")
}
fn code_reviewer_fixture() -> &'static str {
    include_str!("fixtures/agent_outputs/code-reviewer.json")
}
fn wrap_fixture() -> &'static str {
    // Single-line wrap envelope (the bundled wrap.json fixture is pretty-printed
    // across multiple lines, which the last-line envelope parser cannot reach).
    r#"{"role":"wrap","executive_summary":"stub","deviations":[],"residual_risks":[],"recommended_sanity_checks":[]}"#
}

#[test]
fn t3_full_five_stage_cycle_runs() {
    let schema = tasks_schema();
    let conn = fresh_db();
    let display_id = "T100";
    insert_t3_task_at_planning(&conn, display_id);

    // T072 r6: executor and code-reviewer must have session_id (MINOR 1 requirement).
    let mut executor_out = make_run_output(executor_fixture());
    executor_out.session_id = Some("t3-exec-session".to_string());
    let mut code_reviewer_out = make_run_output(code_reviewer_fixture());
    code_reviewer_out.session_id = Some("t3-review-session".to_string());
    let runner = MockRunner::new(vec![
        make_run_output(planner_fixture()),
        make_run_output(plan_reviewer_fixture()),
        executor_out,
        code_reviewer_out,
        make_run_output(wrap_fixture()),
    ]);

    drive_loop(&schema, &conn, display_id, &runner, 50).expect("T3 drive_loop should succeed");

    let final_status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id = ?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        final_status, "in_review",
        "T3 task must end at in_review after the full 5-stage cycle"
    );

    let roles = runner.roles_seen();
    assert_eq!(
        runner.remaining_count(),
        0,
        "all 5 mock outputs must be consumed; roles seen: {:?}",
        roles
    );
    assert_eq!(
        roles,
        vec![
            "planner".to_string(),
            "plan-reviewer".to_string(),
            "executor".to_string(),
            "code-reviewer".to_string(),
            "wrap".to_string(),
        ],
        "T3 dispatch trace must be planner → plan-reviewer → executor → code-reviewer → wrap"
    );

    // No skip-plan transition fired on T3.
    let skip_plan_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='tasks' AND display_id=?1 AND verb='skip-plan'",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        skip_plan_count, 0,
        "T3 row must NOT record any skip-plan transition"
    );

    // submit-plan and submit-plan-review both fired exactly once.
    let plan_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='tasks' AND display_id=?1 AND verb='submit-plan'",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        plan_count, 1,
        "T3 must record exactly one submit-plan transition"
    );

    let plan_review_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='tasks' AND display_id=?1 AND verb='submit-plan-review'",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        plan_review_count, 1,
        "T3 must record exactly one submit-plan-review transition"
    );
}
