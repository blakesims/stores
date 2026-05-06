//! T051: framework-DDL drift detection + auto-apply on boot.
//!
//! Reproduces the L144 actor_note bootstrap pain: a DB created by an older
//! binary (whose SUBSTRATE_DDL lacked actor_note on transition_history) is
//! re-opened by the current binary and must come up with the column added,
//! recorded in substrate_migrations.

use rusqlite::Connection;
use std::sync::Mutex;
use stores::db;
use tempfile::TempDir;

/// Tests in this file mutate the process-wide `STORES_DISABLE_FRAMEWORK_AUTOAPPLY`
/// env var; serialize them so a parallel test never observes a transient
/// `=1` set by another test.
static ENV_GUARD: Mutex<()> = Mutex::new(());

/// Frozen "version-N" SUBSTRATE_DDL — a copy of the historical DDL that
/// predates L144's actor_note column. This is the on-disk shape an older
/// binary would have left behind.
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

fn col_names(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn seed_version_n(path: &std::path::Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(VERSION_N_SUBSTRATE_DDL).unwrap();
}

#[test]
fn version_n_db_auto_applies_actor_note_on_open() {
    let _g = ENV_GUARD.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let dbp = tmp.path().join("db.sqlite");
    seed_version_n(&dbp);

    // Pre-fix invariant: actor_note absent.
    {
        let conn = Connection::open(&dbp).unwrap();
        let cols = col_names(&conn, "transition_history");
        assert!(
            !cols.iter().any(|c| c == "actor_note"),
            "fixture must lack actor_note; got: {cols:?}"
        );
    }

    // Open via current binary's db::open — auto-apply runs.
    let conn = db::open(&dbp).unwrap();

    let cols = col_names(&conn, "transition_history");
    assert!(
        cols.iter().any(|c| c == "actor_note"),
        "actor_note must be present post-open; got: {cols:?}"
    );

    // substrate_migrations contains exactly one matching row.
    let rows: Vec<(String, String, String, String)> = conn
        .prepare(
            "SELECT table_name, column_name, binary_version, applied_at \
             FROM substrate_migrations",
        )
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert_eq!(rows[0].0, "transition_history");
    assert_eq!(rows[0].1, "actor_note");
    assert_eq!(rows[0].2, env!("CARGO_PKG_VERSION"));
    // RFC-3339 UTC with trailing Z + lexicographically <= now.
    let applied_at = &rows[0].3;
    assert!(
        applied_at.ends_with('Z') && applied_at.len() == 20,
        "applied_at {applied_at:?} not RFC-3339 UTC seconds-precision"
    );
    let now = stores::handlers::row::now_iso8601();
    assert!(
        applied_at.as_str() <= now.as_str(),
        "applied_at {applied_at} > now {now}"
    );
}

#[test]
fn second_open_is_idempotent() {
    let _g = ENV_GUARD.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let dbp = tmp.path().join("db.sqlite");
    seed_version_n(&dbp);
    drop(db::open(&dbp).unwrap());
    let conn = db::open(&dbp).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM substrate_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1, "second open must not insert a duplicate audit row");
}

#[test]
fn fresh_db_has_empty_substrate_migrations() {
    let _g = ENV_GUARD.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let dbp = tmp.path().join("db.sqlite");
    let conn = db::open(&dbp).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM substrate_migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0, "fresh-DB open must not record any drift");
}

#[test]
fn disable_autoapply_env_var_skips_apply() {
    let _g = ENV_GUARD.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let dbp = tmp.path().join("db.sqlite");
    seed_version_n(&dbp);

    // Note: Rust tests run in the same process; setting env vars is global,
    // so we must save/restore carefully. The other tests in this file do
    // not depend on this var, but we still clean up after.
    std::env::set_var("STORES_DISABLE_FRAMEWORK_AUTOAPPLY", "1");
    {
        let conn = db::open(&dbp).unwrap();
        let cols = col_names(&conn, "transition_history");
        assert!(
            !cols.iter().any(|c| c == "actor_note"),
            "with env=1, actor_note must remain absent; got: {cols:?}"
        );
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM substrate_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }
    std::env::remove_var("STORES_DISABLE_FRAMEWORK_AUTOAPPLY");

    // Re-open: apply happens.
    let conn = db::open(&dbp).unwrap();
    let cols = col_names(&conn, "transition_history");
    assert!(
        cols.iter().any(|c| c == "actor_note"),
        "after unsetting env, actor_note must be applied; got: {cols:?}"
    );
}
