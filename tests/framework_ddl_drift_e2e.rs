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

    // substrate_migrations contains actor_note plus newer additive transition_history tuple columns.
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
    let actor_note = rows
        .iter()
        .find(|r| r.0 == "transition_history" && r.1 == "actor_note")
        .expect("rows must include transition_history.actor_note");
    assert_eq!(actor_note.2, env!("CARGO_PKG_VERSION"));
    // RFC-3339 UTC with trailing Z + lexicographically <= now.
    let applied_at = &actor_note.3;
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
        .query_row("SELECT COUNT(*) FROM substrate_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 7, "second open must not insert duplicate audit rows");
}

#[test]
fn version_n_db_open_creates_agent_runs_table() {
    let _g = ENV_GUARD.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let dbp = tmp.path().join("db.sqlite");
    seed_version_n(&dbp);
    let conn = db::open(&dbp).unwrap();
    let cols = col_names(&conn, "agent_runs");
    for name in [
        "display_id",
        "phase",
        "cycle",
        "role",
        "model_id",
        "harness_id",
        "started_at",
        "ended_at",
        "exit_code",
        "tokens_in",
        "tokens_out",
        "prompt_cache_hits",
        "transcript_path",
    ] {
        assert!(cols.iter().any(|c| c == name), "missing {name}: {cols:?}");
    }
}

#[test]
fn fresh_db_has_empty_substrate_migrations() {
    let _g = ENV_GUARD.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let dbp = tmp.path().join("db.sqlite");
    let conn = db::open(&dbp).unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM substrate_migrations", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 0, "fresh-DB open must not record any drift");
}

/// L503-A Task 1.12: pre-L503 agent_runs (no brief_text) gets the column added.
#[test]
fn version_n_db_with_legacy_agent_runs_gets_brief_text_column() {
    let _g = ENV_GUARD.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let dbp = tmp.path().join("db.sqlite");

    // Seed a DB that has agent_runs WITHOUT brief_text (pre-L503 shape).
    {
        let conn = Connection::open(&dbp).unwrap();
        conn.execute_batch(VERSION_N_SUBSTRATE_DDL).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_runs ( \
                 id INTEGER PRIMARY KEY AUTOINCREMENT, \
                 display_id TEXT NOT NULL, \
                 phase INTEGER NOT NULL, \
                 cycle INTEGER NOT NULL, \
                 role TEXT NOT NULL, \
                 model_id TEXT NOT NULL, \
                 harness_id TEXT NOT NULL, \
                 started_at TEXT NOT NULL, \
                 ended_at TEXT NOT NULL, \
                 exit_code INTEGER NOT NULL, \
                 tokens_in INTEGER, \
                 tokens_out INTEGER, \
                 prompt_cache_hits INTEGER, \
                 transcript_path TEXT NOT NULL \
             );",
        )
        .unwrap();
        // Insert a legacy row (no brief_text column yet).
        conn.execute(
            "INSERT INTO agent_runs \
             (display_id, phase, cycle, role, model_id, harness_id, \
              started_at, ended_at, exit_code, tokens_in, tokens_out, \
              prompt_cache_hits, transcript_path) \
             VALUES ('T000', 1, 1, 'planner', 'old-model', 'old-harness', \
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:01Z', 0, \
                     0, 0, 0, '/tmp/run.jsonl')",
            [],
        )
        .unwrap();
    }

    // Pre-fix invariant: brief_text absent.
    {
        let conn = Connection::open(&dbp).unwrap();
        let cols = col_names(&conn, "agent_runs");
        assert!(
            !cols.iter().any(|c| c == "brief_text"),
            "fixture must lack brief_text; got: {cols:?}"
        );
    }

    // Open via current binary — apply_framework_drift must add brief_text.
    let conn = db::open(&dbp).unwrap();

    // (1) brief_text column is present, nullable (notnull=0).
    let col_info: Vec<(String, String, i64)> = conn
        .prepare("PRAGMA table_info(agent_runs)")
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(1)?,  // name
                r.get::<_, String>(2)?,  // type
                r.get::<_, i64>(3)?,     // notnull
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    let brief_col = col_info
        .iter()
        .find(|(name, _, _)| name == "brief_text")
        .expect("brief_text must be present after db::open");
    assert_eq!(
        brief_col.1.to_uppercase(),
        "TEXT",
        "brief_text type must be TEXT"
    );
    assert_eq!(brief_col.2, 0, "brief_text must be nullable (notnull=0)");

    // (2) Legacy row brief_text is NULL (no backfill).
    let got: Option<String> = conn
        .query_row(
            "SELECT brief_text FROM agent_runs WHERE display_id='T000'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(got.is_none(), "legacy row brief_text must be NULL, not backfilled");

    // (3) Fresh INSERT with brief_text succeeds and round-trips.
    conn.execute(
        "INSERT INTO agent_runs \
         (display_id, phase, cycle, role, model_id, harness_id, \
          started_at, ended_at, exit_code, tokens_in, tokens_out, \
          prompt_cache_hits, transcript_path, brief_text) \
         VALUES ('T999', 1, 1, 'executor', 'm', 'h', \
                 '2026-01-01T00:00:00Z', '2026-01-01T00:00:01Z', 0, \
                 0, 0, 0, '/tmp/run2.jsonl', 'abc')",
        [],
    )
    .unwrap();
    let fresh: String = conn
        .query_row(
            "SELECT brief_text FROM agent_runs WHERE display_id='T999'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fresh, "abc", "fresh INSERT brief_text must round-trip");
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
            .query_row("SELECT COUNT(*) FROM substrate_migrations", [], |r| {
                r.get(0)
            })
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
