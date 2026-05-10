use rusqlite::Connection;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::builtins::fire_framework_transition_for;
use stores::handlers::framework_migrate::apply_framework_drift;
use stores::schema::Schema;

fn tasks_schema_yaml() -> &'static str {
    BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "tasks")
        .map(|(_, y)| *y)
        .unwrap()
}

fn tasks_schema() -> Schema {
    Schema::from_yaml(tasks_schema_yaml()).unwrap()
}

fn conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(&tasks_schema())).unwrap();
    conn
}

fn seed(conn: &Connection, id: &str, status: &str, lifecycle: &str, integration_step: &str) {
    conn.execute(
        "INSERT INTO tasks (display_id,status,title,slug,contract,activation,lifecycle,active_step,integration_step,blocked,created_at,updated_at,created_by,updated_by) \
         VALUES (?1,?2,'x',?3,'{\"done_when\":\"x\",\"scope_in\":\"x\",\"scope_out\":\"x\"}','active',?4,'none',?5,0,'n','n','framework','framework')",
        rusqlite::params![id, status, id.to_ascii_lowercase(), lifecycle, integration_step],
    )
    .unwrap();
}

fn row(conn: &Connection, id: &str) -> (String, String, String, String) {
    conn.query_row(
        "SELECT status,lifecycle,integration_step,post_integration_step FROM tasks WHERE display_id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

#[test]
fn generic_lifecycle_excludes_repo_specific() {
    let schema = tasks_schema();
    for state in ["queued", "active", "integration", "done"] {
        assert!(schema.lifecycle.states.iter().any(|s| s == state), "missing {state}");
    }
    for state in ["cargo_installed", "schema_migrated", "deploy_blocked"] {
        assert!(
            !schema.lifecycle.states.iter().any(|s| s == state),
            "repo-specific state leaked into lifecycle.states: {state}"
        );
    }
}

#[test]
fn repo_specific_terms_are_quarantined_to_field_enum_or_compat_comments() {
    for (idx, line) in tasks_schema_yaml().lines().enumerate() {
        let mentions_repo_specific = ["cargo_installed", "schema_migrated", "deploy_blocked"]
            .iter()
            .any(|term| line.contains(term));
        if !mentions_repo_specific {
            continue;
        }
        let allowed = line.contains("enum_values") || line.contains("compatibility");
        assert!(
            allowed,
            "repo-specific post-integration term leaked into generic schema surface at line {}: {}",
            idx + 1,
            line
        );
    }
}

#[test]
fn cargo_install_marks_post_integration_without_changing_generic_lifecycle() {
    let conn = conn();
    let schema = tasks_schema();
    seed(&conn, "T601", "integrated", "done", "none");

    fire_framework_transition_for(
        &conn,
        &schema,
        "T601",
        "mark_cargo_installed",
        std::collections::BTreeMap::new(),
        "",
        None,
    )
    .unwrap();

    assert_eq!(
        row(&conn, "T601"),
        ("integrated".into(), "done".into(), "none".into(), "cargo_installed".into())
    );
}

#[test]
fn schema_migrate_chains_from_cargo_post_step_without_status_state() {
    let conn = conn();
    let schema = tasks_schema();
    seed(&conn, "T602", "integrated", "done", "none");
    fire_framework_transition_for(
        &conn,
        &schema,
        "T602",
        "mark_cargo_installed",
        std::collections::BTreeMap::new(),
        "",
        None,
    )
    .unwrap();
    fire_framework_transition_for(
        &conn,
        &schema,
        "T602",
        "mark_schema_migrated",
        std::collections::BTreeMap::new(),
        "",
        None,
    )
    .unwrap();
    assert_eq!(
        row(&conn, "T602"),
        ("integrated".into(), "done".into(), "none".into(), "schema_migrated".into())
    );
}

#[test]
fn third_party_agents_omitting_stores_subscribers_do_not_affect_integrated_done_edge() {
    let conn = conn();
    let schema = tasks_schema();
    seed(&conn, "T603", "integrating", "integration", "verifying");

    fire_framework_transition_for(
        &conn,
        &schema,
        "T603",
        "mark_integrated",
        std::collections::BTreeMap::new(),
        "",
        None,
    )
    .unwrap();

    assert_eq!(
        row(&conn, "T603"),
        ("integrated".into(), "done".into(), "none".into(), "none".into())
    );
}

#[test]
fn pre_migration_cargo_installed_backfills_to_done_post_step() {
    let conn = conn();
    conn.execute_batch("ALTER TABLE tasks DROP COLUMN post_integration_step;").unwrap();
    conn.execute(
        "INSERT INTO tasks (display_id,status,title,slug,contract,activation,lifecycle,active_step,integration_step,blocked,created_at,updated_at,created_by,updated_by) \
         VALUES ('T604','cargo_installed','x','t604','{\"done_when\":\"x\",\"scope_in\":\"x\",\"scope_out\":\"x\"}','inactive','queued','none','none',0,'n','n','framework','framework')",
        [],
    )
    .unwrap();

    apply_framework_drift(&conn).unwrap();

    assert_eq!(
        row(&conn, "T604"),
        ("integrated".into(), "done".into(), "none".into(), "cargo_installed".into())
    );
}
