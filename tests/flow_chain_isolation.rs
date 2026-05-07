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
use stores::flow::{AgentsYaml, PoliciesYaml};
use stores::handlers::{agents_run, submit};
use stores::schema::actor::Actor;
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
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cargo-install-noop");
    copy_dir(&src, &repo);

    assert!(git(&repo, &["init", "-b", "main"]).status.success());
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "init"]);

    git(&repo, &["checkout", "-b", branch]);
    std::fs::write(
        repo.join(format!("{}.txt", unique)),
        format!("{}\n", unique),
    )
    .unwrap();
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
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cargo-install-noop");
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

fn cargo_env_lock() -> &'static std::sync::Mutex<()> {
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

    let accept = parsed
        .agents
        .iter()
        .find(|a| a.name == "accept-merge")
        .unwrap();
    assert!(accept
        .subscribes_to
        .iter()
        .any(|s| s.transition.from == "deploy_blocked" && s.transition.to == "accepted"));
    let cargo = parsed
        .agents
        .iter()
        .find(|a| a.name == "cargo-install")
        .unwrap();
    assert!(cargo
        .subscribes_to
        .iter()
        .any(|s| s.transition.from == "deploy_blocked" && s.transition.to == "accepted"));

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
    let _env = cargo_env_lock().lock().unwrap_or_else(|e| e.into_inner());
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

    let _cwd_g = cwd_lock().lock().unwrap_or_else(|e| e.into_inner());

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

/// retry-deploy writes deploy_blocked→accepted; daemon polling observes that
/// edge and re-runs the normal accept-merge/cargo-install/schema-migrate chain.
#[test]
fn retry_deploy_daemon_poll_retries_post_accept_chain() {
    let _env = cargo_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let cargo_home = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    std::env::set_var("CARGO_HOME", cargo_home.path());
    std::env::set_var("CARGO_TARGET_DIR", target_dir.path());
    std::env::set_var("STORES_BIN", env!("CARGO_BIN_EXE_stores"));

    let (_tmp, repo) = setup_chain_repo("feat/retry", "retry-only");
    let conn = fresh_db_with_substrate();
    insert_accepted_task(&conn, "T200", "feat/retry", repo.to_str().unwrap());
    conn.execute(
        "UPDATE tasks SET status='deploy_blocked', blocked_reason='fixed before retry' WHERE display_id='T200'",
        [],
    )
    .unwrap();

    let task_schema = Schema::from_yaml(
        BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .unwrap(),
    )
    .unwrap();
    submit::run_retry_deploy(&task_schema, &conn, "T200", Actor::AiWithHuman.into()).unwrap();
    assert_eq!(status_of(&conn, "T200"), "accepted");

    let yaml = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agents-yaml/post-accept-chain.yaml"),
    )
    .unwrap();
    let agents = AgentsYaml::from_yaml(&yaml).unwrap();
    let policies = PoliciesYaml::from_yaml("policies: []\n").unwrap();
    let cfg = cfg_path();

    let n1 = agents_run::poll_once(&conn, &agents, &policies, &cfg, "retry-test", "").unwrap();
    assert!(
        n1 >= 3,
        "retry edge should dispatch accept-merge, cargo-install, and schema-migrate through subscriber polling; got {n1}"
    );
    assert_eq!(status_of(&conn, "T200"), "schema_migrated");
    assert_eq!(count_history(&conn, "T200", "retry-deploy"), 1);
    assert_eq!(count_history(&conn, "T200", "mark_cargo_installed"), 1);
    assert_eq!(count_history(&conn, "T200", "mark_schema_migrated"), 1);

    let workflow_dispatches: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dispatch_locks WHERE display_id='T200' AND agent_name IN ('planner','executor','code_reviewer','plan_reviewer')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(workflow_dispatches, 0);

    std::env::remove_var("CARGO_HOME");
    std::env::remove_var("CARGO_TARGET_DIR");
}

/// T061 codex-revise round 2: stale-workspace retry-deploy chain.
///
/// Setup: T997 in deploy_blocked, branch `feat/T997-stale` already merged
/// into main, workspace cleaned (stale path). Daemon cwd is the live main
/// repo.
///
/// Assert:
///   - accept_merge no-ops (branch already merged, cargo-install peer
///     present — round-1 guard prevents mark_cargo_installed).
///   - cargo_install detects stale workspace, falls back to cwd, ACTUALLY
///     runs and fires mark_cargo_installed.
///   - Row reaches `cargo_installed` (not stranded at `accepted`).
///   - transition_history shows mark_cargo_installed with invoker='framework'.
#[test]
fn retry_deploy_stale_workspace_cargo_install_cwd_fallback() {
    let _env = cargo_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let cargo_home = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    std::env::set_var("CARGO_HOME", cargo_home.path());
    std::env::set_var("CARGO_TARGET_DIR", target_dir.path());
    std::env::set_var("STORES_BIN", env!("CARGO_BIN_EXE_stores"));

    // Build a valid cargo repo + git repo that has the branch already merged.
    let (tmp, repo) = setup_chain_repo("feat/T997-stale", "t997-stale-unique");

    // Pre-merge the branch into main so accept-merge's already-merged probe
    // returns true.
    assert!(
        git(&repo, &["merge", "--no-ff", "--no-edit", "feat/T997-stale"])
            .status
            .success(),
        "pre-merge feat/T997-stale into main must succeed"
    );

    let conn = fresh_db_with_substrate();

    // Stale workspace path — worktree was cleaned after the original merge.
    let stale_workspace = tmp.path().join("worktrees/T997-gone");
    insert_accepted_task(
        &conn,
        "T997",
        "feat/T997-stale",
        stale_workspace.to_str().unwrap(),
    );

    // Simulate retry-deploy: mark row deploy_blocked then retry.
    conn.execute(
        "UPDATE tasks SET status='deploy_blocked', blocked_reason='prior cargo failure' WHERE display_id='T997'",
        [],
    )
    .unwrap();

    let task_schema = Schema::from_yaml(
        BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .unwrap(),
    )
    .unwrap();
    submit::run_retry_deploy(&task_schema, &conn, "T997", Actor::AiWithHuman.into()).unwrap();
    assert_eq!(status_of(&conn, "T997"), "accepted");

    // Use production agents.yaml fixture (cargo-install is a peer subscriber
    // on deploy_blocked→accepted) so the accept-merge peer-detection guard fires.
    let yaml = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/agents-yaml/post-accept-chain.yaml"),
    )
    .unwrap();
    let agents = AgentsYaml::from_yaml(&yaml).unwrap();
    let cfg = cfg_path();

    // Set daemon cwd to the live repo so cargo-install's cwd fallback works.
    // Round-3: the cwd validation now requires Cargo.toml [package] name = "stores".
    // The noop fixture uses a different name, so overwrite it before cd-ing in.
    let stores_cargo_toml = format!(
        "[package]\nname = \"stores\"\nversion = \"0.0.1\"\nedition = \"2021\"\n\
         [[bin]]\nname = \"stores\"\npath = \"src/main.rs\"\n\
         [features]\ndefault = []\nrunner-claude-code = []\n"
    );
    std::fs::write(repo.join("Cargo.toml"), &stores_cargo_toml)
        .expect("overwrite Cargo.toml to name=stores for cwd validation");

    let _cwd_g = cwd_lock().lock().unwrap_or_else(|e| e.into_inner());
    let old_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&repo).expect("set daemon cwd to live repo");

    let ctx = DispatchCtx {
        conn: &conn,
        agents: &agents,
        config_path: &cfg,
        policies_hash: "",
    };

    // Step 1: accept-merge — branch already merged, cargo-install peer present.
    // Must no-op (not fire mark_cargo_installed). Row stays at accepted.
    let row = task_row_json(&conn, "T997");
    let am_rc = accept_merge::run(&row, &ctx).expect("accept-merge T997");
    assert_eq!(am_rc, 0, "accept-merge must succeed (no-op)");
    assert_eq!(
        status_of(&conn, "T997"),
        "accepted",
        "accept-merge must not advance row when cargo-install peer present"
    );
    assert_eq!(
        count_history(&conn, "T997", "mark_cargo_installed"),
        0,
        "accept-merge must not fire mark_cargo_installed when cargo-install peer present"
    );

    // Step 2: cargo-install — stale workspace, falls back to cwd (the live repo),
    // runs install, fires mark_cargo_installed. Row advances to cargo_installed.
    let row = task_row_json(&conn, "T997");
    let ci_rc = cargo_install::run(&row, &ctx).expect("cargo-install T997");

    std::env::set_current_dir(&old_cwd).expect("restore cwd");
    drop(_cwd_g);

    assert_eq!(ci_rc, 0, "cargo-install must succeed via cwd fallback");

    // Row must have advanced to cargo_installed (not stranded at accepted).
    assert_eq!(
        status_of(&conn, "T997"),
        "cargo_installed",
        "T997 must reach cargo_installed (not stranded at accepted)"
    );

    // mark_cargo_installed must appear in transition_history with invoker=framework.
    assert_eq!(
        count_history(&conn, "T997", "mark_cargo_installed"),
        1,
        "T997 must have exactly one mark_cargo_installed history row"
    );
    let invoker: String = conn
        .query_row(
            "SELECT invoker FROM transition_history \
             WHERE store='tasks' AND display_id='T997' AND verb='mark_cargo_installed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        invoker, "framework",
        "mark_cargo_installed must be fired by framework actor"
    );

    std::env::remove_var("CARGO_HOME");
    std::env::remove_var("CARGO_TARGET_DIR");
}

/// T061 codex-revise round 3: cwd fallback must reject non-stores Cargo crates.
///
/// Setup: T996 in accepted (stale workspace), daemon cwd is a git repo whose
/// Cargo.toml has [package] name = "not_stores" — a different crate.
///
/// Assert:
///   - cargo_install::run returns Err containing "not the stores crate".
///   - Row does NOT advance to cargo_installed (no mark_cargo_installed entry).
#[test]
fn cargo_install_cwd_fallback_rejects_wrong_crate() {
    // Build a git repo whose Cargo.toml names a non-stores crate.
    let tmp = tempfile::tempdir().unwrap();
    let wrong_repo = tmp.path().to_path_buf();

    // Minimal Cargo project with the wrong package name.
    std::fs::create_dir_all(wrong_repo.join("src")).unwrap();
    std::fs::write(
        wrong_repo.join("Cargo.toml"),
        "[package]\nname = \"not_stores\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [[bin]]\nname = \"not_stores\"\npath = \"src/main.rs\"\n",
    )
    .unwrap();
    std::fs::write(wrong_repo.join("src/main.rs"), "fn main() {}\n").unwrap();

    // Make it a git repo so the git-repo gate passes.
    assert!(git(&wrong_repo, &["init", "-b", "main"]).status.success());
    git(&wrong_repo, &["config", "user.email", "test@example.com"]);
    git(&wrong_repo, &["config", "user.name", "Test"]);
    git(&wrong_repo, &["add", "."]);
    git(&wrong_repo, &["commit", "-m", "init"]);

    let conn = fresh_db_with_substrate();
    // Stale workspace path (won't be accessed; cargo_install bails before cargo).
    let stale = wrong_repo.join("worktrees/T996-gone");
    insert_accepted_task(
        &conn,
        "T996",
        "feat/T996-wrong-crate",
        stale.to_str().unwrap(),
    );

    let agents = AgentsYaml::default_empty();
    let cfg = cfg_path();
    let ctx = DispatchCtx {
        conn: &conn,
        agents: &agents,
        config_path: &cfg,
        policies_hash: "",
    };

    // Hold cwd lock and set cwd to the wrong-crate repo.
    let _cwd_g = cwd_lock().lock().unwrap_or_else(|e| e.into_inner());
    let old_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&wrong_repo).expect("set cwd to wrong-crate repo");

    let row = task_row_json(&conn, "T996");
    let result = cargo_install::run(&row, &ctx);

    std::env::set_current_dir(&old_cwd).expect("restore cwd");
    drop(_cwd_g);

    // Must return Err, not Ok.
    assert!(
        result.is_err(),
        "cargo_install must fail for non-stores cwd crate"
    );
    let err_msg = format!("{:#}", result.unwrap_err());
    assert!(
        err_msg.contains("not the stores crate"),
        "error must mention 'not the stores crate'; got: {err_msg}"
    );

    // Row must NOT have advanced to cargo_installed.
    assert_eq!(
        status_of(&conn, "T996"),
        "accepted",
        "T996 must remain at accepted (cargo_install bailed before transition)"
    );
    assert_eq!(
        count_history(&conn, "T996", "mark_cargo_installed"),
        0,
        "T996 must have zero mark_cargo_installed history rows"
    );
}
