use clap::{Arg, Command};
use rusqlite::Connection;
use serde_json::json;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::builtins::{dispatch_builtin, DispatchCtx};
use stores::flow::AgentsYaml;
use stores::handlers::activate::run_activate;
use stores::handlers::add::run as run_add;
use stores::schema::actor::{Actor, InvokerCtx};
use stores::schema::Schema;

fn schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "tasks")
        .unwrap()
        .1;
    Schema::from_yaml(yaml).unwrap()
}

fn db() -> (Schema, Connection) {
    let schema = schema();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(&schema)).unwrap();
    (schema, conn)
}

fn matches(id: &str) -> clap::ArgMatches {
    Command::new("activate")
        .arg(Arg::new("display_id").required(true).index(1))
        .arg(Arg::new("reason").long("reason").required(true))
        .get_matches_from(["activate", id, "--reason", "test"])
}

fn add_matches(schema: &Schema) -> clap::ArgMatches {
    let leaves = stores::schema::flatten::leaf_args(schema).unwrap();
    let mut cmd = Command::new("add");
    for leaf in &leaves {
        cmd = cmd.arg(Arg::new(leaf.cli_name.clone()).long(leaf.cli_name.clone()));
    }
    cmd.get_matches_from([
        "add",
        "--title",
        "new queued row",
        "--slug",
        "new-queued-row",
        "--done-when",
        "done",
        "--scope-in",
        "src",
        "--scope-out",
        "none",
    ])
}

fn insert_task(conn: &Connection, id: &str, lifecycle: &str, activation: &str, deps: Vec<&str>) {
    conn.execute(
        "INSERT INTO tasks (display_id,status,title,slug,contract,depends_on,activation,lifecycle,active_step,integration_step,blocked,created_at,updated_at,created_by,updated_by) \
         VALUES (?1,'planning',?2,?3,?4,?5,?6,?7,'none','none',0,'2026-05-11T00:00:00Z','2026-05-11T00:00:00Z','framework','framework')",
        rusqlite::params![
            id,
            format!("task {id}"),
            format!("task-{}", id.to_ascii_lowercase()),
            json!({"done_when":"d","scope_in":"i","scope_out":"o"}).to_string(),
            json!(deps).to_string(),
            activation,
            lifecycle,
        ],
    ).unwrap();
}

fn row(conn: &Connection, id: &str) -> (String, String, i64, Option<String>) {
    conn.query_row(
        "SELECT lifecycle, active_step, blocked, blocker_kind FROM tasks WHERE display_id=?1",
        rusqlite::params![id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

mod lifecycle_queued {
    use super::*;

    #[test]
    fn newly_added_shape_is_queued_projection() {
        let (schema, conn) = db();
        run_add(
            &schema,
            &conn,
            &add_matches(&schema),
            InvokerCtx::bare(Actor::AiWithHuman),
        )
        .unwrap();
        let (status, activation): (String, String) = conn
            .query_row(
                "SELECT status, activation FROM tasks WHERE display_id='T001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "planning");
        assert_eq!(activation, "inactive");
        assert_eq!(
            row(&conn, "T001"),
            ("queued".into(), "none".into(), 0, None)
        );
    }

    #[test]
    fn activate_transitions_queued_to_active_and_audits_both_rows() {
        let (schema, conn) = db();
        insert_task(&conn, "T001", "queued", "inactive", vec![]);
        run_activate(
            &schema,
            &conn,
            &matches("T001"),
            InvokerCtx::bare(Actor::AiWithHuman),
        )
        .unwrap();
        assert_eq!(row(&conn, "T001").0, "active");
        let verbs: Vec<String> = conn
            .prepare("SELECT verb FROM transition_history WHERE display_id='T001' ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(verbs, vec!["activate", "activate-task"]);
    }

    #[test]
    fn dependency_blocked_stays_queued() {
        let (schema, conn) = db();
        insert_task(&conn, "T001", "queued", "inactive", vec![]);
        insert_task(&conn, "T002", "queued", "inactive", vec!["T001"]);
        run_activate(
            &schema,
            &conn,
            &matches("T002"),
            InvokerCtx::bare(Actor::AiWithHuman),
        )
        .unwrap();
        assert_eq!(
            row(&conn, "T002"),
            ("queued".into(), "none".into(), 1, Some("dependency".into()))
        );
    }

    #[test]
    fn capacity_blocked_stays_queued() {
        let (schema, conn) = db();
        insert_task(&conn, "T001", "active", "active", vec![]);
        insert_task(&conn, "T002", "queued", "inactive", vec![]);
        run_activate(
            &schema,
            &conn,
            &matches("T002"),
            InvokerCtx::bare(Actor::AiWithHuman),
        )
        .unwrap();
        assert_eq!(
            row(&conn, "T002"),
            ("queued".into(), "none".into(), 1, Some("capacity".into()))
        );
    }

    #[test]
    fn subscriber_promotes_when_blocker_clears() {
        let (schema, conn) = db();
        insert_task(&conn, "T001", "queued", "inactive", vec![]);
        insert_task(&conn, "T002", "queued", "inactive", vec!["T001"]);
        run_activate(
            &schema,
            &conn,
            &matches("T002"),
            InvokerCtx::bare(Actor::AiWithHuman),
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET lifecycle='done', status='integrated' WHERE display_id='T001'",
            [],
        )
        .unwrap();
        let row_json = json!({"display_id":"T002"});
        let agents = AgentsYaml {
            agents: vec![],
            deployment_specialist: None,
        };
        let cfg = std::path::Path::new(".stores/agents.yaml");
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: cfg,
            policies_hash: "",
        };
        assert!(dispatch_builtin("activate-queued", &row_json, &ctx).is_some());
        assert_eq!(row(&conn, "T002").0, "active");
    }

    #[test]
    fn ai_autonomous_activate_is_rejected() {
        let (schema, conn) = db();
        insert_task(&conn, "T001", "queued", "inactive", vec![]);
        let err = run_activate(
            &schema,
            &conn,
            &matches("T001"),
            InvokerCtx::bare(Actor::AiAutonomous),
        )
        .unwrap_err();
        assert!(err.to_string().contains("validation failed"));
        let activation: String = conn
            .query_row(
                "SELECT activation FROM tasks WHERE display_id='T001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(activation, "inactive");
    }
}
