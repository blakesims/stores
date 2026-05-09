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
use stores::flow::builtins::{cargo_install, DispatchCtx};
use stores::flow::AgentsYaml;
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

/// T138 P3: helper for tests that need to seed a row directly into the
/// post-integrated state (the new legal source for reconcile-accepted).
fn insert_integrated_task(conn: &Connection, display_id: &str, branch: &str, workspace_path: &str) {
    let now = "2026-05-03T00:00:00Z";
    let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'integrated', 'test', 't', ?2, ?3, ?4, ?5, ?5, 'framework', 'framework')",
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

/// T138 P3 replacement for the legacy I027 post-accept assertion.
///
/// The pre-T138 form pinned `docs/agents-yaml-example.yaml` to the old
/// post-accept recovery edge (`deploy_blocked → accepted`). T138 retired
/// that ceremony in favour of the generic integration lane: the merge step
/// is now owned by the `integrate` builtin, and `cargo-install` /
/// `schema-migrate` are stores-specific subscribers post-`integrated`.
///
/// This test pins the canonical template's NEW shape so future drift in the
/// docs example fails-loud at the same place I027 used to bite:
///   * `integrate` subscribes to BOTH `(accepted, integration_queued)` and
///     `(integration_blocked, integration_queued)` — the two legal entry
///     edges into the lane (the latter is the recovery edge written by
///     `tasks retry-integration`).
///   * `cargo-install` subscribes ONLY to `(integrating, integrated)` — no
///     accepted-entry subscription, no `(integration_blocked, integrated)`
///     edge (which doesn't exist in the schema; Phase 1 forbids it).
///   * `schema-migrate` subscribes to `(integrated, cargo_installed)` —
///     the source state moved from `accepted` to `integrated`.
#[test]
fn t138_agents_yaml_example_subscribes_integration_lane_and_post_integrated_chain() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/agents-yaml-example.yaml");
    let yaml = std::fs::read_to_string(&path)
        .expect("docs/agents-yaml-example.yaml must be present");
    let parsed = AgentsYaml::from_yaml(&yaml)
        .expect("docs/agents-yaml-example.yaml must parse");

    // accept-merge must NOT be present in the docs example any more.
    assert!(
        !parsed.agents.iter().any(|a| a.name == "accept-merge"),
        "accept-merge agent must be removed from docs/agents-yaml-example.yaml under T138"
    );

    let integrate = parsed
        .agents
        .iter()
        .find(|a| a.name == "integrate")
        .expect("docs example must declare the integrate agent (T138)");
    assert!(
        integrate
            .subscribes_to
            .iter()
            .any(|s| s.transition.from == "accepted" && s.transition.to == "integration_queued"),
        "integrate must subscribe (accepted, integration_queued) — happy-path lane entry"
    );
    assert!(
        integrate
            .subscribes_to
            .iter()
            .any(|s| s.transition.from == "integration_blocked"
                && s.transition.to == "integration_queued"),
        "integrate must subscribe (integration_blocked, integration_queued) — \
         retry-integration recovery edge (T138 replacement for the I027 retry-deploy edge)"
    );

    let cargo = parsed
        .agents
        .iter()
        .find(|a| a.name == "cargo-install")
        .expect("docs example must declare cargo-install");
    let cargo_subs: Vec<(&str, &str)> = cargo
        .subscribes_to
        .iter()
        .map(|s| (s.transition.from.as_str(), s.transition.to.as_str()))
        .collect();
    assert_eq!(
        cargo_subs,
        vec![("integrating", "integrated")],
        "cargo-install must subscribe ONLY to (integrating, integrated) under T138; \
         no accepted-entry subscription, no (integration_blocked, integrated) edge \
         (Phase 1 schema forbids the latter). Got: {:?}",
        cargo_subs
    );

    let migrate = parsed
        .agents
        .iter()
        .find(|a| a.name == "schema-migrate")
        .expect("docs example must declare schema-migrate");
    assert!(
        migrate
            .subscribes_to
            .iter()
            .any(|s| s.transition.from == "integrated" && s.transition.to == "cargo_installed"),
        "schema-migrate must subscribe (integrated, cargo_installed) under T138 \
         (source state moved from accepted → integrated)"
    );
    assert!(
        !migrate
            .subscribes_to
            .iter()
            .any(|s| s.transition.from == "accepted" && s.transition.to == "cargo_installed"),
        "schema-migrate must NOT carry the legacy (accepted, cargo_installed) \
         subscription post-T138"
    );
}

// AC4.1 (T138 P3): RETIRED. Pre-T138 a test here drove the post-accept
// ceremony directly (accept-merge → cargo-install → schema-migrate) and
// asserted that a merge conflict on one row (accepted → deploy_blocked)
// did not block a peer's full chain. T138 moved the merge step into the
// generic integration lane (builtin:integrate, Phase 2) and removed the
// `(accepted → deploy_blocked)` edge entirely — accept-merge is no longer
// dispatched. Chain isolation in the new lane is exercised by the integrate
// builtin's own test suite (Phase 2) and by the two-candidate integration
// test (Phase 5).

// retry_deploy_daemon_poll_retries_post_accept_chain (T138 P3): RETIRED.
// Pre-T138 this test exercised retry-deploy's `deploy_blocked → accepted`
// recovery edge by replaying the accept-merge → cargo-install →
// schema-migrate chain through `agents_run::poll_once`. T138 removed
// accept-merge from dispatch and moved the merge step into the generic
// integration lane (builtin:integrate). The recovery shape is now
// `tasks retry-integration` (`integration_blocked → integration_queued`),
// covered by the integrate builtin's own test suite (Phase 2).

// retry_deploy_stale_workspace_cargo_install_cwd_fallback (T138 P3): RETIRED.
// Pre-T138 this test asserted that after retry-deploy (deploy_blocked →
// accepted), accept-merge no-oped on an already-merged branch and
// cargo-install fell back to cwd to fire `mark_cargo_installed`. Under
// T138, accept-merge is no longer dispatched and cargo-install's source
// state moved from `accepted` to `integrated` (it subscribes only to
// (integrating → integrated)). The cwd-fallback behaviour itself is still
// exercised directly in `cargo_install_cwd_fallback_rejects_wrong_crate`
// below and inside the integrate builtin's tests; the orchestration shape
// this test pinned is gone.

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

// ---------------------------------------------------------------------------
// I027 / T107 reconcile-accepted recovery verb tests.
// Pi msg_85be1b1c: operator-grounded recovery for `accepted` rows whose
// post-accept ceremony never fired (typical I027 case after a pre-I027
// retry-deploy missed the subscriber edge).
// ---------------------------------------------------------------------------

/// Actor gate: ai_autonomous must be rejected. Mirrors retry-deploy's gate.
#[test]
fn i027_reconcile_accepted_actor_gate_rejects_ai_autonomous() {
    use stores::handlers::reconcile_accepted::run_reconcile_accepted;

    let conn = fresh_db_with_substrate();
    insert_accepted_task(&conn, "T900", "feat/T900-test", "/tmp/nowhere-T900");
    let cfg = cfg_path();
    let err = run_reconcile_accepted(&conn, &cfg, "T900", Actor::AiAutonomous.into())
        .expect_err("ai_autonomous must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("ai_autonomous is not permitted"),
        "expected actor gate error; got: {msg}"
    );
}

/// Branch-not-merged guard: reconcile-accepted refuses to advance a row
/// whose branch hasn't been merged into main. T138 P3: the verb's source
/// statuses are now {integrated, cargo_installed} — an integrated row
/// always has its branch on main (the lane fast-merged it), but defensive
/// programming retains the merged-into-main check for handcrafted/legacy
/// rows that may have been advanced out-of-band.
#[test]
fn i027_reconcile_accepted_rejects_unmerged_branch() {
    use stores::handlers::reconcile_accepted::run_reconcile_accepted;
    let _env = cargo_env_lock().lock().unwrap_or_else(|e| e.into_inner());

    let (_tmp, repo) = setup_chain_repo("feat/T901-unmerged", "t901-unmerged");
    let conn = fresh_db_with_substrate();
    insert_integrated_task(
        &conn,
        "T901",
        "feat/T901-unmerged",
        repo.to_str().unwrap(),
    );

    // Branch is NOT pre-merged into main here, so reconcile-accepted must bail
    // even though the row's status sits at the new legal source `integrated`.
    let cfg = cfg_path();
    let err = run_reconcile_accepted(&conn, &cfg, "T901", Actor::AiWithHuman.into())
        .expect_err("must bail on unmerged branch");
    let msg = err.to_string();
    assert!(
        msg.contains("not merged into main"),
        "expected unmerged-branch error; got: {msg}"
    );
    assert_eq!(
        status_of(&conn, "T901"),
        "integrated",
        "T901 must remain at integrated (no transitions on bail)"
    );
    assert_eq!(
        count_history(&conn, "T901", "mark_cargo_installed"),
        0,
        "T901 must have zero mark_cargo_installed history on bail"
    );
}

/// T107-shape e2e (T138 P3 update): integrated row whose branch is
/// already on main but whose stores-specific post-`integrated` chain
/// never fired. reconcile-accepted must re-fire cargo-install
/// (mark_cargo_installed) and schema-migrate (mark_schema_migrated)
/// without pretending the row was deploy_blocked.
///
/// Pre-T138 this test seeded an `accepted` row and re-ran the full
/// post-accept ceremony (accept-merge → cargo-install → schema-migrate).
/// Post-T138 the integration lane owns the merge step, so reconcile-accepted
/// no longer drives accept-merge — its scope shrinks to the post-integrated
/// chain.
#[test]
fn i027_reconcile_accepted_advances_integrated_to_schema_migrated_for_merged_branch() {
    use stores::handlers::reconcile_accepted::run_reconcile_accepted;

    let _env = cargo_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let cargo_home = tempfile::tempdir().unwrap();
    let target_dir = tempfile::tempdir().unwrap();
    std::env::set_var("CARGO_HOME", cargo_home.path());
    std::env::set_var("CARGO_TARGET_DIR", target_dir.path());
    let private_bin = cargo_home.path().join("private-daemon/bin/stores");
    std::env::set_var("STORES_DAEMON_BIN_PATH", &private_bin);
    std::env::set_var("STORES_BIN", env!("CARGO_BIN_EXE_stores"));

    // Build a valid cargo + git repo with the branch already merged into main —
    // this is the T107-shape: work shipped (lane fast-merged), but stores-specific
    // post-integrated chain stranded at `integrated`.
    let (_tmp, repo) = setup_chain_repo("feat/T902-merged", "t902-merged");
    assert!(
        git(&repo, &["merge", "--no-ff", "--no-edit", "feat/T902-merged"])
            .status
            .success(),
        "pre-merge feat/T902-merged into main must succeed"
    );

    let conn = fresh_db_with_substrate();
    insert_integrated_task(&conn, "T902", "feat/T902-merged", repo.to_str().unwrap());

    // Overwrite Cargo.toml to package name=stores so cargo-install's cwd
    // validation accepts it (mirrors the round-3 pattern in retry_deploy_stale).
    let stores_cargo_toml =
        "[package]\nname = \"stores\"\nversion = \"0.0.1\"\nedition = \"2021\"\n\
         [[bin]]\nname = \"stores\"\npath = \"src/main.rs\"\n\
         [features]\ndefault = []\nrunner-claude-code = []\n";
    std::fs::write(repo.join("Cargo.toml"), stores_cargo_toml)
        .expect("overwrite Cargo.toml to name=stores for cwd validation");

    // Set daemon cwd to the live repo so cargo-install's cwd fallback works
    // (the same pattern retry_deploy_stale_workspace tests use).
    let _cwd_g = cwd_lock().lock().unwrap_or_else(|e| e.into_inner());
    let old_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(&repo).expect("set daemon cwd to live repo");

    let cfg = cfg_path();
    let result = run_reconcile_accepted(&conn, &cfg, "T902", Actor::AiWithHuman.into());

    std::env::set_current_dir(&old_cwd).expect("restore cwd");
    drop(_cwd_g);

    result.expect("reconcile-accepted T902 must succeed on merged-branch integrated row");

    // Final state: schema_migrated. transition_history shows the framework
    // verbs were fired (NOT mark_deploy_blocked / NOT a synthetic flip back —
    // the row stayed truthful at integrated → cargo_installed → schema_migrated).
    assert_eq!(
        status_of(&conn, "T902"),
        "schema_migrated",
        "T902 must reach schema_migrated via direct chain re-fire"
    );
    assert_eq!(
        count_history(&conn, "T902", "mark_cargo_installed"),
        1,
        "T902 must have exactly one mark_cargo_installed history row (no double-fire)"
    );
    assert_eq!(
        count_history(&conn, "T902", "mark_schema_migrated"),
        1,
        "T902 must have exactly one mark_schema_migrated history row"
    );
    assert_eq!(
        count_history(&conn, "T902", "mark_deploy_blocked"),
        0,
        "T902 must NOT be flipped to deploy_blocked (Pi rejected synthetic-flip approach)"
    );
    assert_eq!(
        count_history(&conn, "T902", "retry-deploy"),
        0,
        "T902 must NOT have a synthetic retry-deploy in history"
    );

    std::env::remove_var("CARGO_HOME");
    std::env::remove_var("CARGO_TARGET_DIR");
    std::env::remove_var("STORES_DAEMON_BIN_PATH");
}

/// Idempotency: calling reconcile-accepted on an already-schema_migrated row
/// must fail-loud "already reconciled", not silently no-op.
#[test]
fn i027_reconcile_accepted_rejects_already_reconciled_row() {
    use stores::handlers::reconcile_accepted::run_reconcile_accepted;

    let conn = fresh_db_with_substrate();
    let now = "2026-05-09T00:00:00Z";
    let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, created_at, updated_at, created_by, updated_by) \
         VALUES ('T903', 'schema_migrated', 'test', 't', 'feat/T903', '/tmp/T903', ?1, ?2, ?2, 'framework', 'framework')",
        rusqlite::params![contract, now],
    )
    .unwrap();
    let cfg = cfg_path();
    let err = run_reconcile_accepted(&conn, &cfg, "T903", Actor::AiWithHuman.into())
        .expect_err("schema_migrated row must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("already at status='schema_migrated'") || msg.contains("nothing to reconcile"),
        "expected already-reconciled error; got: {msg}"
    );
}
