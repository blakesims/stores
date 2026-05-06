//! T051: framework-DDL drift applies BEFORE any subscriber poll.
//!
//! `db::open` (called from `run_drive`) auto-applies framework drift before
//! the daemon enters its subscriber loop. Test the ordering invariant via
//! timestamp lex-comparison: any dispatch_lock claimed AFTER `db::open`
//! returns must have `claimed_at >= substrate_migrations.applied_at`. Both
//! timestamps share the same RFC-3339-UTC-seconds format.

use rusqlite::Connection;
use stores::db;
use tempfile::TempDir;

/// Frozen older SUBSTRATE_DDL lacking actor_note (forces drift).
const VERSION_N_SUBSTRATE_DDL: &str = "\
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
    occurred_at TEXT NOT NULL
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

#[test]
fn drift_applied_before_lock_claimed_at() {
    let tmp = TempDir::new().unwrap();
    let dbp = tmp.path().join("db.sqlite");
    {
        let conn = Connection::open(&dbp).unwrap();
        conn.execute_batch(VERSION_N_SUBSTRATE_DDL).unwrap();
    }

    // db::open auto-applies drift here.
    let conn = db::open(&dbp).unwrap();

    // Simulate a subscriber claim that happens AFTER open returns.
    let claimed_at = stores::handlers::row::now_iso8601();
    conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by) \
         VALUES ('observations', 1, 'L001', 'investigator', NULL, ?1, 'test')",
        rusqlite::params![claimed_at],
    )
    .unwrap();

    let applied_at: String = conn
        .query_row(
            "SELECT applied_at FROM substrate_migrations \
             WHERE table_name='transition_history' AND column_name='actor_note'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    assert!(
        applied_at.as_str() <= claimed_at.as_str(),
        "framework drift applied_at ({applied_at}) must be <= post-open claimed_at ({claimed_at})"
    );
}
