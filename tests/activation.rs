//! T140 P1: integration-level coverage for the per-row activation primitive.
//!
//! The exhaustive lib-level suite lives at
//! `src/handlers/activate.rs#tests` and is the canonical surface for AC1.2
//! (`cargo test --lib activation`). This file mirrors the high-signal
//! acceptance scenarios through the public `stores::` API so the brief's
//! listed `tests/activation.rs` exists and the contract is exercised at the
//! crate boundary too.

use rusqlite::Connection;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::activate::{run_activate, run_deactivate};
use stores::handlers::framework_migrate::{apply_framework_drift, IN_FLIGHT_STATES};
use stores::schema::actor::Actor;
use stores::schema::Schema;

use clap::{Arg, ArgMatches, Command};

fn tasks_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(name, _)| *name == "tasks")
        .map(|(_, y)| *y)
        .expect("bundled tasks schema present");
    Schema::from_yaml(yaml).expect("tasks schema parses")
}

fn fresh_db_with_tasks() -> (Schema, Connection) {
    let schema = tasks_schema();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).ok();
    conn.execute_batch(&ddl_for(&schema)).unwrap();
    (schema, conn)
}

fn pre_t140_db() -> (Schema, Connection) {
    let (schema, conn) = fresh_db_with_tasks();
    conn.execute_batch("ALTER TABLE \"tasks\" DROP COLUMN \"activation\";")
        .expect("DROP activation simulates pre-T140 DB");
    (schema, conn)
}

fn insert_minimal_task(conn: &Connection, display_id: &str, status: &str) {
    let now = "2026-05-09T00:00:00Z";
    let slug = format!("task-{}", display_id.to_ascii_lowercase());
    let contract = r#"{"done_when":"d","scope_in":"i","scope_out":"o"}"#;
    conn.execute(
        "INSERT INTO tasks \
         (display_id, status, title, slug, contract, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, 'framework', 'framework')",
        rusqlite::params![
            display_id,
            status,
            format!("task {display_id}"),
            slug,
            contract,
            now
        ],
    )
    .unwrap();
}

fn select_activation(conn: &Connection, display_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT activation FROM tasks WHERE display_id = ?1",
        rusqlite::params![display_id],
        |r| r.get::<_, Option<String>>(0),
    )
    .unwrap()
}

fn build_activate_matches(args: &[&str]) -> ArgMatches {
    Command::new("activate")
        .arg(Arg::new("display_id").required(true).index(1))
        .arg(Arg::new("reason").long("reason").required(true))
        .get_matches_from(args)
}

/// AC1.3 — fresh DB row defaults to activation='inactive'.
#[test]
fn integration_ac1_3_fresh_db_default_is_inactive() {
    let (_schema, conn) = fresh_db_with_tasks();
    insert_minimal_task(&conn, "T100", "planning");
    assert_eq!(select_activation(&conn, "T100").unwrap(), "inactive");
}

/// AC1.4 — framework-migrate backfills IN_FLIGHT_STATES → 'active', rest → 'inactive'.
#[test]
fn integration_ac1_4_framework_migrate_backfill_classes() {
    let (_schema, conn) = pre_t140_db();
    insert_minimal_task(&conn, "T200", "planning");
    insert_minimal_task(&conn, "T201", "executing");
    insert_minimal_task(&conn, "T202", "code_review");
    insert_minimal_task(&conn, "T203", "integrating");
    insert_minimal_task(&conn, "T204", "accepted");

    let applied = apply_framework_drift(&conn).unwrap();
    assert!(applied
        .iter()
        .any(|m| m.table_name == "tasks" && m.column_name == "activation"));

    assert_eq!(select_activation(&conn, "T200").unwrap(), "inactive");
    assert_eq!(select_activation(&conn, "T201").unwrap(), "active");
    assert_eq!(select_activation(&conn, "T202").unwrap(), "active");
    assert_eq!(select_activation(&conn, "T203").unwrap(), "active");
    assert_eq!(select_activation(&conn, "T204").unwrap(), "inactive");

    assert_eq!(IN_FLIGHT_STATES, &["executing", "code_review", "integrating"]);
}

/// AC1.5 — ai_with_human activate flips to 'active' and writes audit row with
/// verb='activate' and actor_note=<reason>.
#[test]
fn integration_ac1_5_ai_with_human_activate_flips_and_audits() {
    let (schema, conn) = fresh_db_with_tasks();
    insert_minimal_task(&conn, "T999", "planning");

    let m = build_activate_matches(&[
        "activate",
        "T999",
        "--reason",
        "operator armed for integration",
    ]);
    run_activate(&schema, &conn, &m, Actor::AiWithHuman.into())
        .expect("ai_with_human activate must succeed");

    assert_eq!(select_activation(&conn, "T999").unwrap(), "active");

    let (verb, note, invoker): (String, Option<String>, String) = conn
        .query_row(
            "SELECT verb, actor_note, invoker FROM transition_history \
             WHERE display_id = 'T999' ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(verb, "activate");
    assert_eq!(note.as_deref(), Some("operator armed for integration"));
    assert_eq!(invoker, "ai_with_human");
}

/// AC1.6 — ai_autonomous activate is rejected and the row is unchanged.
#[test]
fn integration_ac1_6_ai_autonomous_rejected_row_unchanged() {
    let (schema, conn) = fresh_db_with_tasks();
    insert_minimal_task(&conn, "T999", "planning");

    let m = build_activate_matches(&[
        "activate",
        "T999",
        "--reason",
        "should be rejected",
    ]);
    let err = run_activate(&schema, &conn, &m, Actor::AiAutonomous.into())
        .expect_err("ai_autonomous activate MUST be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("ai_with_human")
            || msg.contains("ai_autonomous")
            || msg.contains("actor"),
        "expected actor-rejection wording; got: {msg}"
    );

    assert_eq!(select_activation(&conn, "T999").unwrap(), "inactive");

    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history WHERE display_id = 'T999'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 0, "rejected write must not write transition_history");
}

/// AC1.7 — missing --reason errors at clap parse time before the handler runs.
#[test]
fn integration_ac1_7_missing_reason_errors_at_cli_parse() {
    let cmd = Command::new("activate")
        .arg(Arg::new("display_id").required(true).index(1))
        .arg(Arg::new("reason").long("reason").required(true));
    let err = cmd
        .try_get_matches_from(["activate", "T999"])
        .expect_err("missing --reason must fail parse");
    let msg = err.to_string();
    assert!(
        msg.contains("--reason") || msg.to_lowercase().contains("required"),
        "error must cite --reason / required; got: {msg}"
    );
}

/// `deactivate` is the symmetric verb; ai_with_human flips active→inactive
/// and writes verb='deactivate' / actor_note=<reason>.
#[test]
fn integration_deactivate_mirrors_activate() {
    let (schema, conn) = fresh_db_with_tasks();
    insert_minimal_task(&conn, "T700", "planning");

    let m_arm = build_activate_matches(&["activate", "T700", "--reason", "arm"]);
    run_activate(&schema, &conn, &m_arm, Actor::AiWithHuman.into()).unwrap();
    assert_eq!(select_activation(&conn, "T700").unwrap(), "active");

    let m_disarm = build_activate_matches(&[
        "deactivate",
        "T700",
        "--reason",
        "stand down for review",
    ]);
    run_deactivate(&schema, &conn, &m_disarm, Actor::AiWithHuman.into())
        .expect("ai_with_human deactivate must succeed");
    assert_eq!(select_activation(&conn, "T700").unwrap(), "inactive");

    let (verb, note): (String, Option<String>) = conn
        .query_row(
            "SELECT verb, actor_note FROM transition_history \
             WHERE display_id = 'T700' ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(verb, "deactivate");
    assert_eq!(note.as_deref(), Some("stand down for review"));
}
