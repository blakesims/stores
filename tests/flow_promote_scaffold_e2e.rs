//! T020 Phase 6 — end-to-end ratify→promote→scaffold integration test.
//!
//! Drives the full upstream-autonomy chain on a tempdir repo:
//!
//!   * `observations add --lock-contract --invoker human`
//!     → walks open→investigating→confirmed and the framework auto-ratify
//!     hook fires confirmed→ready (Phase 1 + Phase 2).
//!   * `poll_once` with the production agents.yaml fixture →
//!     `builtin:auto-promote` mints a tasks row at `planning` with
//!     `linked_observations` populated and `obs.task_id` back-linked
//!     (Phase 3); `builtin:auto-scaffold` then sees the synthetic
//!     ''→planning create transition and runs the configured shell
//!     command, parses the last stdout line, and writes
//!     `tasks.workspace_path` (Phase 4).
//!   * Re-running `poll_once` is a no-op: auto-promote does not duplicate
//!     the tasks row and auto-scaffold does not re-run the command.

use rusqlite::Connection;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::load_from_path;
use stores::flow::policies_yaml::PoliciesYaml;
use stores::handlers::add;
use stores::handlers::agents_run::poll_once;
use stores::schema::{actor::Actor, FieldType, Schema};

fn git(repo: &Path, args: &[&str]) -> std::process::Output {
    let mut full: Vec<&str> = vec!["-C", repo.to_str().unwrap()];
    full.extend_from_slice(args);
    Command::new("git").args(&full).output().unwrap()
}

fn fresh_db(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
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

/// Build an `add` clap Command with leaf args + `--lock-contract` +
/// `--display-id`, mirroring src/cli/dynamic.rs::build_leaf_cmd_owned and
/// build_add_cmd. Reproduced here because the production builders are
/// crate-private.
fn build_obs_add_cmd(schema: &Schema) -> clap::Command {
    use clap::{Arg, ArgAction};
    let leaves = stores::schema::flatten::leaf_args(schema).unwrap();
    let mut cmd = clap::Command::new("add");
    for leaf in &leaves {
        let is_text_like = matches!(
            leaf.field.ty,
            FieldType::Text | FieldType::Timestamp | FieldType::DisplayId | FieldType::Json
        );
        let is_list = matches!(
            leaf.field.ty,
            FieldType::List(_) | FieldType::ListFk { .. } | FieldType::ListRecord(_)
        );
        let mut arg = Arg::new(leaf.cli_name.clone())
            .long(leaf.cli_name.clone())
            .required(false);
        if is_list {
            arg = arg.action(ArgAction::Append);
        }
        cmd = cmd.arg(arg);
        if is_text_like {
            let from_file_name = format!("{}-from-file", leaf.cli_name);
            cmd = cmd.arg(
                Arg::new(from_file_name.clone())
                    .long(from_file_name)
                    .required(false),
            );
        }
    }
    cmd = cmd.arg(Arg::new("display-id").long("display-id").required(false));
    cmd = cmd.arg(
        Arg::new("lock-contract")
            .long("lock-contract")
            .action(ArgAction::SetTrue)
            .required(false),
    );
    cmd
}

#[test]
fn ratify_promote_scaffold_e2e() {
    // ----- (Task 6.8) tempdir + git crate fixture -----
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();

    assert!(git(&repo, &["init", "-b", "main"]).status.success());
    git(&repo, &["config", "user.email", "test+t020p6@example.com"]);
    git(&repo, &["config", "user.name", "T020 P6 Test"]);
    std::fs::write(repo.join("README.md"), "init\n").unwrap();
    git(&repo, &["add", "README.md"]);
    let init = git(&repo, &["commit", "-m", "init"]);
    assert!(init.status.success(), "git init commit failed: {:?}", init);

    // .stores/ + db + scaffold config (Task 6.2).
    let stores_dir = repo.join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();
    let db_path = stores_dir.join("db.sqlite");
    let conn = fresh_db(&db_path);

    let cfg_path = stores_dir.join("config.yaml");
    let repo_canon = repo.canonicalize().unwrap();
    let repo_str = repo_canon.to_string_lossy().to_string();
    // The `mkdir -p` is redundant after `git worktree add` but keeps the
    // command simple to reason about. The final `echo` line is what
    // auto-scaffold parses as the workspace path.
    let scaffold_cmd = format!(
        "cd {repo} && git worktree add -b feat/{{display_id}}-{{slug}} \
         wt-{{display_id}} >/dev/null 2>&1 && echo {repo}/wt-{{display_id}}",
        repo = repo_str
    );
    std::fs::write(
        &cfg_path,
        format!("scaffold:\n  command: \"{}\"\n", scaffold_cmd),
    )
    .unwrap();

    // ----- (Task 6.3) observations add --lock-contract --invoker human -----
    let obs_yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "observations")
        .map(|(_, y)| *y)
        .unwrap();
    let obs_schema = Schema::from_yaml(obs_yaml).unwrap();
    let cmd = build_obs_add_cmd(&obs_schema);
    let matches = cmd.get_matches_from([
        "add",
        "--summary",
        "auto-promote candidate (T020 P6)",
        "--source",
        "dev",
        "--priority",
        "normal",
        "--captured-at",
        "2026-05-03T00:00:00Z",
        "--captured-week",
        "w18-d7",
        "--objective",
        "exercise the ratify-promote-scaffold chain end-to-end",
        "--type",
        "work",
        "--in-scope",
        "obs",
        "--out-of-scope",
        "tasks",
        "--acceptance",
        "tasks row created at planning with workspace_path",
        "--tier-hint",
        "T2",
        "--lock-contract",
    ]);
    let started = Instant::now();
    add::run(&obs_schema, &conn, &matches, Actor::Human.into())
        .expect("observations add --lock-contract --invoker human must succeed");

    let (obs_display, obs_status): (String, String) = conn
        .query_row(
            "SELECT display_id, status FROM observations LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        obs_status, "ready",
        "observation must land at 'ready' after lock-contract walk + auto-ratify"
    );
    let confirmed_to_ready: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='observations' AND display_id=?1 \
               AND from_status='confirmed' AND to_status='ready'",
            rusqlite::params![obs_display],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        confirmed_to_ready, 1,
        "exactly one confirmed→ready ratify transition must be recorded"
    );

    // ----- (Task 6.4) poll_once with the production agents.yaml fixture -----
    let agents_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents.yaml");
    let agents = load_from_path(&agents_path).expect("tests/fixtures/agents.yaml must parse");
    let policies = PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    };

    // L141 -> L134: under the typed dispatch_locks lifecycle, the lock closes
    // immediately after spawn (once drive_pid_recorded_or_terminal passes).
    // The watchdog detects post-spawn zombies via `tasks.drive_pid` + in-cycle
    // status rather than open-lock state. Stub STORES_DRIVE_CMD to a
    // long-running sleep so the spawned process stays alive across the watchdog
    // sweep, keeping the task in `planning` and preserving the test's
    // `status='planning'` invariant.
    std::env::set_var("STORES_DRIVE_CMD", "sleep 600 #");

    let n1 = poll_once(&conn, &agents, &policies, &cfg_path, "t020p6-claimer", "").unwrap();
    let elapsed = started.elapsed();
    // AC6.2: ratify→promote within ~5s wall-clock.
    assert!(
        elapsed.as_secs() < 5,
        "ratify→promote must complete within 5s; took {:?}",
        elapsed
    );
    assert!(
        n1 >= 1,
        "first poll_once must dispatch at least one builtin (auto-promote); got {}",
        n1
    );

    // ----- (Task 6.5) tasks row + back-link assertions -----
    let (task_display, task_status, contract_str, linked_str): (String, String, String, String) =
        conn.query_row(
            "SELECT display_id, status, contract, linked_observations FROM tasks LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert!(
        task_display.starts_with('T'),
        "minted task display_id must start with 'T'; got {task_display}"
    );
    assert_eq!(
        task_status, "planning",
        "auto-promoted task must land at planning"
    );
    let linked: Value = serde_json::from_str(&linked_str).unwrap();
    assert_eq!(
        linked,
        serde_json::json!([obs_display]),
        "linked_observations must include the source obs id"
    );
    let contract: Value = serde_json::from_str(&contract_str).unwrap();
    let done_when = contract
        .get("done_when")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        done_when.contains("ratify-promote-scaffold"),
        "contract.done_when must carry the obs intent_contract.objective; got: {done_when}"
    );
    let obs_task_id: Option<String> = conn
        .query_row(
            "SELECT task_id FROM observations WHERE display_id=?1",
            rusqlite::params![obs_display],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        obs_task_id.as_deref(),
        Some(task_display.as_str()),
        "obs.task_id must back-link to the new task"
    );

    // ----- (Task 6.6) second poll → auto-scaffold writes workspace_path -----
    // (auto-scaffold may already have fired in the first poll if its agent
    // entry sorted after auto-promote in the agents loop; either way, by the
    // end of the second poll the workspace_path must be populated.)
    let _n2 = poll_once(&conn, &agents, &policies, &cfg_path, "t020p6-claimer", "").unwrap();
    let workspace_path: Option<String> = conn
        .query_row(
            "SELECT workspace_path FROM tasks WHERE display_id=?1",
            rusqlite::params![task_display],
            |r| r.get(0),
        )
        .unwrap();
    let wp = workspace_path.unwrap_or_default();
    assert!(
        !wp.is_empty(),
        "workspace_path must be populated after auto-scaffold runs"
    );
    let wp_dir = PathBuf::from(&wp);
    // AC6.3: workspace_path points at an existing directory inside the tempdir.
    assert!(
        wp_dir.is_dir(),
        "workspace_path must point at an existing directory; got {wp}"
    );
    assert!(
        wp_dir.starts_with(&repo_canon),
        "workspace_path must live inside the tempdir ({repo_str}); got {wp}"
    );
    assert!(
        wp_dir.join(".git").exists(),
        "worktree marker .git must exist inside the scaffolded directory at {wp}"
    );

    // ----- (Task 6.7) third poll → idempotent: no new rows or mutations -----
    let task_count_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    let _n3 = poll_once(&conn, &agents, &policies, &cfg_path, "t020p6-claimer", "").unwrap();
    let task_count_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        task_count_before, task_count_after,
        "auto-promote must be idempotent across repeated poll_once invocations"
    );
    let wp_after: String = conn
        .query_row(
            "SELECT workspace_path FROM tasks WHERE display_id=?1",
            rusqlite::params![task_display],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        wp_after, wp,
        "workspace_path must not be re-mutated by a second auto-scaffold run"
    );

    // L141 -> L134: reap the long-running stub so we don't leak it past the test.
    let drive_pid: Option<i64> = conn
        .query_row(
            "SELECT drive_pid FROM tasks WHERE display_id=?1",
            rusqlite::params![task_display],
            |r| r.get(0),
        )
        .unwrap_or(None);
    if let Some(p) = drive_pid {
        unsafe {
            libc::kill(p as i32, libc::SIGTERM);
        }
    }
    std::env::remove_var("STORES_DRIVE_CMD");
}
