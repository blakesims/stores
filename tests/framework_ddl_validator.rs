//! T051: boot-time validator for SUBSTRATE_DDL.
//!
//! `validate_framework_tables` must reject any *additive* column declared
//! `NOT NULL` without a `DEFAULT` (since ALTER TABLE ADD COLUMN against a
//! non-empty existing DB would fail). The production const must pass.

use rusqlite::Connection;
use stores::codegen::ddl::{
    validate_framework_ddl, validate_framework_tables, FrameworkColumn, FrameworkTable,
    FRAMEWORK_DDL_TABLES, SUBSTRATE_DDL,
};

#[test]
fn production_substrate_ddl_passes() {
    validate_framework_ddl().expect("production SUBSTRATE_DDL must validate");
    validate_framework_tables(FRAMEWORK_DDL_TABLES).expect("via slice");
}

#[test]
fn synthetic_nonnullable_no_default_rejected() {
    let bad: &[FrameworkTable] = &[FrameworkTable {
        name: "synthetic_table",
        columns: &[FrameworkColumn {
            name: "synthetic_col",
            sql_type: "TEXT",
            nullable: false,
            default_sql: None,
            full_def: "synthetic_col TEXT NOT NULL",
            additive: true,
        }],
    }];
    let err = validate_framework_tables(bad).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("synthetic_table"),
        "error must name table; msg: {msg}"
    );
    assert!(
        msg.contains("synthetic_col"),
        "error must name column; msg: {msg}"
    );
}

#[test]
fn fresh_substrate_ddl_creates_engine_runner_tables() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute(
        "INSERT INTO engine_runner_heartbeats \
         (iteration, started_at, saw_tasks, saw_intake, saw_observations, actionable, held, dispatched) \
         VALUES (1, '2026-05-07T00:00:00Z', 2, 3, 4, 5, 6, 7)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO engine_runner_actions \
         (store, row_id, classification, held_reason, dispatched, last_logged_at, updated_at) \
         VALUES ('tasks', 42, 'held', 'needs_human', 0, '2026-05-07T00:00:00Z', '2026-05-07T00:00:01Z')",
        [],
    )
    .unwrap();
}

#[test]
fn synthetic_nonnullable_with_default_accepted() {
    let ok: &[FrameworkTable] = &[FrameworkTable {
        name: "ok_table",
        columns: &[FrameworkColumn {
            name: "ok_col",
            sql_type: "INTEGER",
            nullable: false,
            default_sql: Some("0"),
            full_def: "ok_col INTEGER NOT NULL DEFAULT 0",
            additive: true,
        }],
    }];
    validate_framework_tables(ok).expect("NOT NULL DEFAULT must validate");
}
