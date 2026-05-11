//! T031 Phase 2 — post-accept schema-migrate end-to-end.
//!
//! Proves that the `builtin:schema-migrate` subscriber, when handed a row at
//! `cargo_installed`, applies the freshly-installed binary's bundled schema
//! to the live DB via subprocess (no manual `stores migrate --apply`) and
//! advances the row to `schema_migrated`.
//!
//! `STORES_BIN` is pointed at the test-built `stores` binary
//! (`env!("CARGO_BIN_EXE_stores")`) so the subprocess loads whatever bundled
//! schemas this branch defines. The two variants share the post-accept
//! ceremony shape but diverge on the binary:
//!   * success — STORES_BIN = test binary; expects column re-added and row
//!     transitions to `schema_migrated`.
//!   * failure — STORES_BIN = `/bin/false`; expects the row to flip to
//!     `deploy_blocked` with a non-empty `blocked_reason`.

use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use stores::flow::builtins::{schema_migrate, DispatchCtx};
use stores::flow::AgentsYaml;

/// CWD + STORES_BIN are process-globals; serialise the two test variants in
/// this binary so they don't clobber each other when run in parallel.
fn lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info(\"{}\")", table))
        .unwrap();
    stmt.query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

fn task_row_json(conn: &Connection, display_id: &str) -> Value {
    let mut stmt = conn
        .prepare("SELECT * FROM tasks WHERE display_id = ?1")
        .unwrap();
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(rusqlite::params![display_id]).unwrap();
    let row = rows.next().unwrap().unwrap();
    let mut obj = serde_json::Map::new();
    for (i, name) in cols.iter().enumerate() {
        let v: rusqlite::types::Value = row.get(i).unwrap();
        let jv = match v {
            rusqlite::types::Value::Null => Value::Null,
            rusqlite::types::Value::Integer(i) => Value::from(i),
            rusqlite::types::Value::Real(f) => {
                Value::from(serde_json::Number::from_f64(f).unwrap_or(0.into()))
            }
            rusqlite::types::Value::Text(s) => Value::String(s),
            rusqlite::types::Value::Blob(b) => {
                Value::String(String::from_utf8_lossy(&b).to_string())
            }
        };
        obj.insert(name.clone(), jv);
    }
    Value::Object(obj)
}

/// Direct INSERT of a `cargo_installed` task — mirrors the unit-test pattern
/// in `src/flow/builtins/mod.rs::insert_cargo_installed_task`. We sidestep
/// the normal `accepted → cargo_installed` walk because the schema-migrate
/// subscriber only cares about the precondition row state, not the path.
fn insert_cargo_installed_task(conn: &Connection, display_id: &str, workspace_path: &str) {
    let now = "2026-05-03T00:00:00Z";
    let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, lifecycle, active_step, integration_step, post_integration_step, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'integrated', 'test', 't', 'feat/x', ?2, ?3, 'done', 'none', 'none', 'cargo_installed', ?4, ?4, 'framework', 'framework')",
        rusqlite::params![display_id, workspace_path, contract, now],
    )
    .unwrap();
}

/// Set up `<root>/.stores/{db.sqlite,manifest.yaml}` with bundled
/// `observations` + `tasks` installed, then chdir back. Returns the absolute
/// `root`.
fn setup_workspace() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root: PathBuf = tmp.path().canonicalize().expect("canonicalize tmp");

    let old_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(&root).expect("cd tmp");

    stores::cli::init::run().expect("stores init");
    stores::install::run(&PathBuf::from("observations")).expect("install observations");
    stores::install::run(&PathBuf::from("tasks")).expect("install tasks");

    std::env::set_current_dir(&old_cwd).expect("restore cwd");
    (tmp, root)
}

/// Drop the first non-reserved TEXT column from `observations` so the
/// freshly-bundled binary's compute_plan diff has work to do. Returns the
/// dropped column's name. We pick lazily by introspecting the live table —
/// any TEXT column the bundled schema defines but we've removed will do.
fn drop_a_text_column(db_path: &Path) -> String {
    let conn = Connection::open(db_path).unwrap();
    // Reserved names that compute_plan/codegen treats specially. Avoid them
    // so our drop survives migrate's diff.
    // Mirrors RESERVED_COLUMNS in src/codegen/ddl.rs — compute_plan refuses
    // to auto-recover reserved columns, so dropping one yields a hard error
    // rather than the additive ALTER we want to exercise here.
    let reserved: &[&str] = &[
        "rowid",
        "id",
        "display_id",
        "status",
        "created_at",
        "updated_at",
        "created_by",
        "updated_by",
    ];
    let cols = columns(&conn, "observations");
    let target = cols
        .iter()
        .find(|c| {
            if reserved.contains(&c.as_str()) {
                return false;
            }
            // Only TEXT columns — DROP COLUMN on indexed/PK fails.
            let t: String = conn
                .query_row(
                    "SELECT type FROM pragma_table_info('observations') WHERE name = ?1",
                    rusqlite::params![c],
                    |r| r.get(0),
                )
                .unwrap_or_default();
            t == "TEXT"
        })
        .cloned()
        .expect("observations must expose a non-reserved TEXT column");
    conn.execute_batch(&format!(
        "ALTER TABLE \"observations\" DROP COLUMN \"{}\";",
        target
    ))
    .unwrap();
    target
}

#[test]
fn post_accept_schema_migrate_applies_addition_via_subprocess() {
    let _g = lock().lock().unwrap_or_else(|e| e.into_inner());

    let (_tmp, root) = setup_workspace();
    let db_file = root.join(".stores/db.sqlite");

    let dropped = drop_a_text_column(&db_file);

    // Insert the cargo_installed task on the workspace DB.
    let conn = Connection::open(&db_file).unwrap();
    insert_cargo_installed_task(&conn, "T700", root.to_str().unwrap());
    let row = task_row_json(&conn, "T700");

    // Point the subscriber at the test-built binary.
    std::env::set_var("STORES_BIN", env!("CARGO_BIN_EXE_stores"));

    let agents = AgentsYaml::default_empty();
    let cfg = PathBuf::from("/tmp/stores-test-no-config.yaml");
    let ctx = DispatchCtx {
        conn: &conn,
        agents: &agents,
        config_path: &cfg,
        policies_hash: "",
    };

    let res = schema_migrate::run(&row, &ctx).expect("schema_migrate::run");
    assert_eq!(res, 0, "schema_migrate must return Ok(0) on success");

    // (a) dropped column re-added.
    let cols = columns(&conn, "observations");
    assert!(
        cols.contains(&dropped),
        "expected '{}' to be re-added; got: {:?}",
        dropped,
        cols
    );

    // (b) task row remains integrated with stores-specific post step schema_migrated.
    let (status, post_step): (String, String) = conn
        .query_row(
            "SELECT status, post_integration_step FROM tasks WHERE display_id='T700'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "integrated");
    assert_eq!(post_step, "schema_migrated");

    // (c) transition_history records mark_schema_migrated.
    let (verb, from, to): (String, String, String) = conn
        .query_row(
            "SELECT verb, from_status, to_status FROM transition_history \
             WHERE store='tasks' AND display_id='T700' AND verb='mark_schema_migrated'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(verb, "mark_schema_migrated");
    assert_eq!(from, "integrated");
    assert_eq!(to, "integrated");

    std::env::remove_var("STORES_BIN");
}

#[test]
fn post_accept_schema_migrate_blocks_when_subprocess_fails() {
    let _g = lock().lock().unwrap_or_else(|e| e.into_inner());

    let (_tmp, root) = setup_workspace();
    let db_file = root.join(".stores/db.sqlite");

    let conn = Connection::open(&db_file).unwrap();
    insert_cargo_installed_task(&conn, "T701", root.to_str().unwrap());
    let row = task_row_json(&conn, "T701");

    // /bin/false exits 1 with empty output → subprocess "failure" branch.
    std::env::set_var("STORES_BIN", "/bin/false");

    let agents = AgentsYaml {
        agents: vec![],
        // Suppress further dispatch; an unresolved name short-circuits with
        // a stderr log, no observation written.
        deployment_specialist: Some("does-not-exist".to_string()),
    };
    let cfg = PathBuf::from("/tmp/stores-test-no-config.yaml");
    let ctx = DispatchCtx {
        conn: &conn,
        agents: &agents,
        config_path: &cfg,
        policies_hash: "",
    };

    schema_migrate::run(&row, &ctx).expect("schema_migrate::run");

    let (status, reason): (String, Option<String>) = conn
        .query_row(
            "SELECT status, blocked_reason FROM tasks WHERE display_id='T701'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "integrated");
    let post_step: String = conn
        .query_row(
            "SELECT post_integration_step FROM tasks WHERE display_id='T701'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(post_step, "deploy_blocked");
    let reason = reason.unwrap_or_default();
    assert!(
        !reason.is_empty(),
        "blocked_reason must be populated on subprocess failure"
    );
    assert!(
        reason.contains("schema-migrate failed"),
        "blocked_reason must carry the schema-migrate prefix; got: {reason}"
    );

    std::env::remove_var("STORES_BIN");
}
