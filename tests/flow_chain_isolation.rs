//! T019 Phase 4 — chain isolation integration test.
//!
//! Drives the post-accept ceremony (accept-merge → cargo-install →
//! schema-migrate) against two independent task rows on independent
//! tempdir repos and asserts:
//!   * AC4.1: each row's chain progresses independently — both reach
//!     `schema_migrated` with `mark_cargo_installed` and
//!     `mark_schema_migrated` audit rows present.
//!   * AC4.1: a merge conflict on one row (T101) does not prevent the
//!     other row (T100) from completing the full chain.
//!   * AC4.2: the post-accept-chain.yaml fixture parses cleanly via
//!     `AgentsYaml::from_yaml` and resolves `deployment_specialist`.

use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::builtins::{accept_merge, cargo_install, schema_migrate, DispatchCtx};
use stores::flow::AgentsYaml;
use stores::schema::Schema;

fn git(repo: &Path, args: &[&str]) -> std::process::Output {
    let mut full: Vec<&str> = vec!["-C", repo.to_str().unwrap()];
    full.extend_from_slice(args);
    Command::new("git").args(&full).output().unwrap()
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Set up an independent tempdir repo that is both a valid cargo crate
/// (copied from `tests/fixtures/cargo-install-noop`) and a git repo with
/// a `main` branch plus a feature branch named `branch`. The feature
/// branch adds a uniquely-named file `<unique>.txt` so two such repos
/// can both fast-merge into their own main without colliding.
fn setup_chain_repo(branch: &str, unique: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cargo-install-noop");
    copy_dir(&src, &repo);

    assert!(git(&repo, &["init", "-b", "main"]).status.success());
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);

    git(&repo, &["checkout", "-b", branch]);
    std::fs::write(repo.join(format!("{}.txt", unique)), format!("{}\n", unique)).unwrap();
    git(&repo, &["add", &format!("{}.txt", unique)]);
    git(&repo, &["commit", "-m", "feat"]);
    git(&repo, &["checkout", "main"]);

    write_bundled_manifest(&repo);
    (tmp, repo)
}

/// Set up a repo where `main` and `branch` diverge on `Cargo.toml`,
/// guaranteeing a merge conflict.
fn setup_conflict_repo(branch: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cargo-install-noop");
    copy_dir(&src, &repo);

    assert!(git(&repo, &["init", "-b", "main"]).status.success());
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);

    git(&repo, &["checkout", "-b", branch]);
    std::fs::write(repo.join("Cargo.toml"), "branch-side\n").unwrap();
    git(&repo, &["add", "Cargo.toml"]);
    git(&repo, &["commit", "-m", "branch change"]);
    git(&repo, &["checkout", "main"]);
    std::fs::write(repo.join("Cargo.toml"), "main-side\n").unwrap();
    git(&repo, &["add", "Cargo.toml"]);
    git(&repo, &["commit", "-m", "main change"]);

    write_bundled_manifest(&repo);
    (tmp, repo)
}

/// Initialize the workspace's on-disk substrate so the schema-migrate
/// subprocess (T031 P1) finds a real `.stores/db.sqlite` to operate on.
/// Walks `init::run()` + `install::run(<name>)` for each bundled store under
/// `root` as CWD, then restores the original CWD. Holds a process-wide
/// mutex because `set_current_dir` is global.
fn write_bundled_manifest(root: &Path) {
    let _g = cwd_lock().lock().unwrap_or_else(|e| e.into_inner());
    let old_cwd = std::env::current_dir().expect("get cwd");
    std::env::set_current_dir(root).expect("cd workspace");
    stores::cli::init::run().expect("stores init");
    for &name in stores::cli::dynamic::BUNDLED_STORE_NAMES {
        stores::install::run(&PathBuf::from(name))
            .unwrap_or_else(|e| panic!("install {name} failed: {e}"));
    }
    std::env::set_current_dir(&old_cwd).expect("restore cwd");
}

/// Process-wide CWD lock for this test binary — `setup_chain_repo` /
/// `setup_conflict_repo` chdir into each fresh tempdir during init+install.
fn cwd_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

fn fresh_db_with_substrate() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    for name in ["tasks", "observations", "gate"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        let s = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&s)).unwrap();
    }
    conn
}

fn insert_accepted_task(conn: &Connection, display_id: &str, branch: &str, workspace_path: &str) {
    let now = "2026-05-03T00:00:00Z";
    let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'accepted', 'test', 't', ?2, ?3, ?4, ?5, ?5, 'framework', 'framework')",
        rusqlite::params![display_id, branch, workspace_path, contract, now],
    )
    .unwrap();
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

fn status_of(conn: &Connection, display_id: &str) -> String {
    conn.query_row(
        "SELECT status FROM tasks WHERE display_id = ?1",
        rusqlite::params![display_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn count_history(conn: &Connection, display_id: &str, verb: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM transition_history \
         WHERE store='tasks' AND display_id = ?1 AND verb = ?2",
        rusqlite::params![display_id, verb],
        |r| r.get(0),
    )
    .unwrap()
}

fn cfg_path() -> PathBuf {
    PathBuf::from("/tmp/stores-test-no-config-flow-chain.yaml")
}

/// AC4.2: the production fixture parses and `deployment_specialist`
/// resolves to a declared agent.
#[test]
fn ac4_2_post_accept_chain_fixture_parses() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agents-yaml/post-accept-chain.yaml");
    let yaml = std::fs::read_to_string(&path).unwrap();
    let parsed = AgentsYaml::from_yaml(&yaml).expect("post-accept-chain fixture must parse");

    let names: Vec<&str> = parsed.agents.iter().map(|a| a.name.as_str()).collect();
    assert!(names.contains(&"accept-merge"), "names: {:?}", names);
    assert!(names.contains(&"cargo-install"), "names: {:?}", names);
    assert!(names.contains(&"schema-migrate"), "names: {:?}", names);
    assert!(names.contains(&"user-escalation"), "names: {:?}", names);

    let spec = parsed.deployment_specialist.as_deref();
    assert_eq!(spec, Some("user-escalation"));
    assert!(
        parsed.agents.iter().any(|a| Some(a.name.as_str()) == spec),
        "deployment_specialist must resolve to a declared agent"
    );
}

/// AC4.1: chain isolation — T100 (clean) reaches schema_migrated via the
/// full ceremony; T101 (merge conflict) flips to deploy_blocked at the
/// accept-merge link and does not prevent T100 from completing.
#[test]
fn ac4_1_chain_isolation_failure_does_not_block_peer() {
    // Independent tempdir CARGO_HOME / target dir so the test does not
    // pollute the developer's shared cargo cache.
    let cargo_home = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    std::env::set_var("CARGO_HOME", cargo_home.path());
    std::env::set_var("CARGO_TARGET_DIR", target_dir.path());
    // T031 P1: schema-migrate now spawns a subprocess. Point it at the
    // test-built binary so it picks up this branch's bundled schemas.
    std::env::set_var("STORES_BIN", env!("CARGO_BIN_EXE_stores"));

    let (_t100_tmp, t100_repo) = setup_chain_repo("feat/t100", "t100-only");
    let (_t101_tmp, t101_repo) = setup_conflict_repo("feat/t101");

    let conn = fresh_db_with_substrate();
    insert_accepted_task(&conn, "T100", "feat/t100", t100_repo.to_str().unwrap());
    insert_accepted_task(&conn, "T101", "feat/t101", t101_repo.to_str().unwrap());

    let agents = AgentsYaml::default_empty();
    let cfg = cfg_path();
    let ctx = DispatchCtx {
        conn: &conn,
        agents: &agents,
        config_path: &cfg,
        policies_hash: "",
    };

    // --- T101 chain (conflict): accept-merge flips to deploy_blocked. ---
    let row_t101 = task_row_json(&conn, "T101");
    accept_merge::run(&row_t101, &ctx).expect("accept-merge T101 returns Ok even on conflict");
    assert_eq!(
        status_of(&conn, "T101"),
        "deploy_blocked",
        "T101 must flip to deploy_blocked on merge conflict"
    );

    // --- T100 chain (clean): full three-link ceremony must complete. ---
    let row_t100 = task_row_json(&conn, "T100");
    let code = accept_merge::run(&row_t100, &ctx).expect("accept-merge T100");
    assert_eq!(code, 0);
    assert_eq!(
        status_of(&conn, "T100"),
        "accepted",
        "T100 stays at accepted after clean merge"
    );

    // Refresh row (status unchanged but defensive against future schema additions).
    let row_t100 = task_row_json(&conn, "T100");
    let code = cargo_install::run(&row_t100, &ctx).expect("cargo-install T100");
    assert_eq!(code, 0, "cargo-install T100 must succeed on noop fixture");
    assert_eq!(status_of(&conn, "T100"), "cargo_installed");

    let row_t100 = task_row_json(&conn, "T100");
    let code = schema_migrate::run(&row_t100, &ctx).expect("schema-migrate T100");
    assert_eq!(code, 0);
    assert_eq!(
        status_of(&conn, "T100"),
        "schema_migrated",
        "T100 must reach schema_migrated"
    );

    // Audit rows for T100's full chain.
    assert_eq!(
        count_history(&conn, "T100", "mark_cargo_installed"),
        1,
        "T100 must have one mark_cargo_installed history row"
    );
    assert_eq!(
        count_history(&conn, "T100", "mark_schema_migrated"),
        1,
        "T100 must have one mark_schema_migrated history row"
    );

    // T101 must NOT have advanced past deploy_blocked.
    assert_eq!(
        count_history(&conn, "T101", "mark_cargo_installed"),
        0,
        "T101 must not have mark_cargo_installed (chain halted at accept-merge)"
    );
    assert_eq!(
        count_history(&conn, "T101", "mark_schema_migrated"),
        0,
        "T101 must not have mark_schema_migrated"
    );
    assert_eq!(
        count_history(&conn, "T101", "mark_deploy_blocked"),
        1,
        "T101 must have one mark_deploy_blocked history row"
    );

    std::env::remove_var("CARGO_HOME");
    std::env::remove_var("CARGO_TARGET_DIR");
}
