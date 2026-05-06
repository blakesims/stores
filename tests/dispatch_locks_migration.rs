//! T050 Phase 1 / L134: integration tests for the typed-dispatch_locks
//! migration. Builds a DB with the OLD pre-L134 dispatch_locks schema
//! (column set without the 9 new typed fields), seeds legacy rows, runs
//! ensure_dispatch_locks_typed + backfill_legacy_locks, and verifies the
//! shape & idempotency of the migration.

use rusqlite::Connection;
use stores::handlers::agents_run::{backfill_legacy_locks, ensure_dispatch_locks_typed};

/// Pre-L134 SUBSTRATE_DDL: dispatch_locks WITHOUT the 9 typed columns and
/// WITHOUT the framework_migrations ledger table. Mirrors the schema as it
/// existed in `main` immediately before T050 P1.
const OLD_SUBSTRATE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS transition_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    store TEXT NOT NULL,
    row_id INTEGER NOT NULL,
    display_id TEXT NOT NULL,
    from_status TEXT,
    to_status TEXT NOT NULL,
    verb TEXT NOT NULL,
    invoker TEXT NOT NULL,
    policy_ref TEXT,
    policies_hash TEXT,
    occurred_at TEXT NOT NULL,
    actor_note TEXT
);
CREATE TABLE IF NOT EXISTS dispatch_locks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    store TEXT NOT NULL,
    row_id INTEGER NOT NULL,
    display_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    transition_id INTEGER,
    claimed_at TEXT NOT NULL,
    claimed_by TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 1,
    last_status TEXT,
    finished_at TEXT,
    UNIQUE(store, row_id, agent_name)
);
";

fn open_old_schema_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(OLD_SUBSTRATE_DDL).unwrap();
    // framework_migrations ledger isn't present in the OLD DDL; the migration
    // path must tolerate that. We create it here (as the new SUBSTRATE_DDL
    // would on a fresh open) so the test can record into it. In production,
    // db::open creates framework_migrations via the new SUBSTRATE_DDL before
    // ensure_dispatch_locks_typed runs.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS framework_migrations (
            id TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL,
            note TEXT
        );",
    )
    .unwrap();
    conn
}

fn insert_legacy_lock(conn: &Connection, agent: &str, row_id: i64, display_id: &str, last_status: &str) {
    conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, \
          attempts, last_status, finished_at) \
         VALUES ('tasks', ?1, ?2, ?3, 1, '2026-04-01T00:00:00Z', 'daemon-pre-l134', \
                 1, ?4, '2026-04-01T00:00:01Z')",
        rusqlite::params![row_id, display_id, agent, last_status],
    )
    .unwrap();
}

fn count_columns(conn: &Connection) -> std::collections::HashSet<String> {
    let mut stmt = conn.prepare("PRAGMA table_info('dispatch_locks')").unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

#[test]
fn migration_adds_typed_columns_and_backfills_legacy_rows() {
    let conn = open_old_schema_db();

    // Verify we start without the typed columns.
    let before = count_columns(&conn);
    assert!(!before.contains("daemon_epoch"), "starting state must lack daemon_epoch");
    assert!(!before.contains("terminal_reason"), "starting state must lack terminal_reason");

    // Three legacy rows with distinct last_status patterns.
    insert_legacy_lock(&conn, "agent-a", 1, "T001", "ok");
    insert_legacy_lock(&conn, "agent-b", 2, "T002", "exit=11");
    insert_legacy_lock(&conn, "agent-c", 3, "T003", "error: boom");

    ensure_dispatch_locks_typed(&conn).expect("ensure_dispatch_locks_typed");
    let n = backfill_legacy_locks(&conn).expect("backfill_legacy_locks");
    assert_eq!(n, 3, "all three legacy rows must be backfilled");

    // All 9 typed columns must now exist.
    let cols = count_columns(&conn);
    for name in [
        "daemon_epoch",
        "claim_source",
        "attempt",
        "pid",
        "heartbeat_at",
        "postcondition_id",
        "postcondition_args",
        "terminal_reason",
        "next_retry_at",
    ] {
        assert!(cols.contains(name), "expected typed column {name} after migration");
    }

    // terminal_reason derivation per row.
    let map: std::collections::HashMap<String, String> = conn
        .prepare("SELECT display_id, terminal_reason FROM dispatch_locks")
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(map.get("T001").map(String::as_str), Some("ok"));
    assert_eq!(map.get("T002").map(String::as_str), Some("exit_nonzero"));
    assert_eq!(map.get("T003").map(String::as_str), Some("error"));

    // claim_source = 'legacy' for every backfilled row.
    let non_legacy: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dispatch_locks WHERE claim_source != 'legacy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(non_legacy, 0, "every legacy row must have claim_source='legacy'");

    // Exactly one framework_migrations row with the L134 id.
    let migration_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM framework_migrations WHERE id='L134-dispatch-locks-typed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(migration_count, 1, "exactly one L134 migration row");
}

#[test]
fn migration_is_idempotent() {
    let conn = open_old_schema_db();
    insert_legacy_lock(&conn, "agent-a", 1, "T001", "ok");

    // First run: applies migration + backfills 1 row.
    ensure_dispatch_locks_typed(&conn).unwrap();
    let n1 = backfill_legacy_locks(&conn).unwrap();
    assert_eq!(n1, 1);

    // Snapshot for diff checks.
    let mig_count_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM framework_migrations", [], |r| r.get(0))
        .unwrap();
    let row_snapshot_before: (String, String, i64) = conn
        .query_row(
            "SELECT terminal_reason, claim_source, attempt FROM dispatch_locks WHERE display_id='T001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    // Second run: no error, no new framework_migrations row, no row updates.
    ensure_dispatch_locks_typed(&conn).expect("idempotent ensure");
    let n2 = backfill_legacy_locks(&conn).expect("idempotent backfill");
    assert_eq!(n2, 0, "second backfill must update zero rows");

    let mig_count_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM framework_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        mig_count_before, mig_count_after,
        "second ensure must not add a framework_migrations row"
    );

    let row_snapshot_after: (String, String, i64) = conn
        .query_row(
            "SELECT terminal_reason, claim_source, attempt FROM dispatch_locks WHERE display_id='T001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(row_snapshot_before, row_snapshot_after, "row contents unchanged");
}
