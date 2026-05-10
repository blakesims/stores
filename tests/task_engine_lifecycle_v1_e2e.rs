use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::{Subscription, TransitionEdge};
use stores::flow::policies_yaml::PoliciesYaml;
use stores::flow::{AgentEntry, AgentsYaml, RetryPolicy};
use stores::flow::builtins::fire_framework_transition_for;
use stores::handlers::agents_run::{poll_once_with_guard, FsBinaryIdentityProvider};
use stores::handlers::framework_migrate::ensure_integration_singleton_index;
use stores::schema::Schema;

fn git(repo: &Path, args: &[&str]) -> std::process::Output {
    let mut full: Vec<&str> = vec!["-C", repo.to_str().unwrap()];
    full.extend_from_slice(args);
    Command::new("git").args(&full).output().unwrap()
}

fn init_repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    assert!(git(&repo, &["init", "-b", "main"]).status.success());
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("README.md"), "init\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-m", "init"]);
    std::fs::write(repo.join("base.txt"), "base\n").unwrap();
    git(&repo, &["add", "base.txt"]);
    git(&repo, &["commit", "-m", "base"]);
    (tmp, repo)
}

fn add_branch(repo: &Path, branch: &str) {
    assert!(git(repo, &["checkout", "-b", branch]).status.success());
    std::fs::write(repo.join("candidate.txt"), "candidate\n").unwrap();
    git(repo, &["add", "candidate.txt"]);
    assert!(git(repo, &["commit", "-m", "candidate"]).status.success());
}

fn tasks_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "tasks")
        .map(|(_, y)| *y)
        .unwrap();
    Schema::from_yaml(yaml).unwrap()
}

fn prepare_db(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    for name in ["tasks", "external_reviews", "observations"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        conn.execute_batch(&ddl_for(&Schema::from_yaml(yaml).unwrap()))
            .unwrap();
    }
    ensure_integration_singleton_index(&conn).unwrap();
    conn.execute_batch(
        "CREATE TABLE lock_window_probe (
            id INTEGER PRIMARY KEY,
            status TEXT NOT NULL,
            lifecycle TEXT NOT NULL,
            active_step TEXT NOT NULL,
            integration_step TEXT NOT NULL,
            blocked INTEGER NOT NULL,
            lock_count INTEGER NOT NULL
        );",
    )
    .unwrap();
    conn
}

fn seed_accepted_t3(conn: &Connection, display_id: &str, branch: &str, workspace_path: &str) {
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, tier_hint, branch, workspace_path, contract, activation, blocked_reason, lifecycle, active_step, integration_step, blocked, blocker_kind, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'accepted', 'lifecycle v1 e2e', 'lifecycle-v1-e2e', 'T3', ?2, ?3, ?4, 'active', '', 'integration', 'none', 'none', 0, NULL, ?5, ?5, 'framework', 'framework')",
        rusqlite::params![display_id, branch, workspace_path, r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#, "2026-05-10T00:00:00Z"],
    ).unwrap();
}

fn overlay(conn: &Connection, display_id: &str) -> (String, String, String, i64, Option<String>) {
    conn.query_row(
        "SELECT lifecycle, active_step, integration_step, blocked, blocker_kind FROM tasks WHERE display_id=?1",
        [display_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )
    .unwrap()
}

fn status(conn: &Connection, display_id: &str) -> String {
    conn.query_row(
        "SELECT status FROM tasks WHERE display_id=?1",
        [display_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn assert_overlay(
    conn: &Connection,
    display_id: &str,
    expected: (&str, &str, &str, i64, Option<&str>),
) {
    assert_eq!(
        overlay(conn, display_id),
        (
            expected.0.into(),
            expected.1.into(),
            expected.2.into(),
            expected.3,
            expected.4.map(str::to_string)
        )
    );
}

fn agents() -> AgentsYaml {
    let mut args = serde_yaml::Mapping::new();
    args.insert(
        serde_yaml::Value::String("pre_land_check".into()),
        serde_yaml::Value::String("true".into()),
    );
    args.insert(
        serde_yaml::Value::String("allow_push".into()),
        serde_yaml::Value::Bool(false),
    );
    AgentsYaml {
        agents: vec![AgentEntry {
            name: "integrate".into(),
            subscribes_to: vec![Subscription {
                store: "tasks".into(),
                transition: TransitionEdge {
                    from: "accepted".into(),
                    to: "integration_queued".into(),
                },
                integration_step: None,
                predicate: None,
            }],
            command: "builtin:integrate".into(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy::default(),
            command_args: Some(args),
        }],
        deployment_specialist: None,
    }
}

fn policies() -> PoliciesYaml {
    PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    }
}

#[test]
fn t3_lifecycle_overlay_and_main_branch_lock_span_integration() {
    let db_tmp = tempfile::tempdir().unwrap();
    let db_path = db_tmp.path().join("stores.db");
    let conn = prepare_db(&db_path);
    let (_repo_tmp, repo) = init_repo();
    add_branch(&repo, "feat/lifecycle-v1");

    let hook = repo.join(".git/hooks/post-checkout");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nsqlite3 '{}' \"INSERT INTO lock_window_probe (status,lifecycle,active_step,integration_step,blocked,lock_count) SELECT status,lifecycle,active_step,integration_step,blocked,(SELECT COUNT(*) FROM resource_locks WHERE resource_id='main_branch') FROM tasks WHERE display_id='T144E2E';\"\n",
            db_path.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook, perms).unwrap();
    }

    seed_accepted_t3(&conn, "T144E2E", "feat/lifecycle-v1", repo.to_str().unwrap());
    assert_eq!(status(&conn, "T144E2E"), "accepted");
    assert_overlay(&conn, "T144E2E", ("integration", "none", "none", 0, None));
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM resource_locks WHERE resource_id='main_branch'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );

    fire_framework_transition_for(
        &conn,
        &tasks_schema(),
        "T144E2E",
        "enqueue-integration",
        std::collections::BTreeMap::new(),
        "",
        None,
    )
    .unwrap();
    assert_eq!(status(&conn, "T144E2E"), "integration_queued");
    assert_overlay(&conn, "T144E2E", ("integration", "none", "queued", 0, None));

    let cfg = db_tmp.path().join("agents.yaml");
    let p = policies();
    for _ in 0..10 {
        poll_once_with_guard::<FsBinaryIdentityProvider>(
            &conn,
            &agents(),
            &p,
            &cfg,
            "claimer",
            "epoch",
            None,
        )
        .unwrap();
        if status(&conn, "T144E2E") == "integrated" {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    assert_eq!(status(&conn, "T144E2E"), "integrated");
    assert_overlay(&conn, "T144E2E", ("done", "none", "none", 0, None));
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM resource_locks WHERE resource_id='main_branch'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap(),
        0,
        "main_branch lock must be released after integrated"
    );

    let probe: (String, String, String, String, i64, i64) = conn
        .query_row(
            "SELECT status,lifecycle,active_step,integration_step,blocked,lock_count FROM lock_window_probe ORDER BY id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .unwrap();
    assert_eq!(
        probe,
        (
            "integrating".into(),
            "integration".into(),
            "none".into(),
            "merging".into(),
            0,
            1
        ),
        "hook probes the lock while status=integrating and integration_step=merging"
    );

    let (start_at, mark_at): (String, String) = conn
        .query_row(
            "SELECT \
             MAX(CASE WHEN verb='start-integration' THEN occurred_at END), \
             MAX(CASE WHEN verb='mark_verify_done' THEN occurred_at END) \
             FROM transition_history WHERE store='tasks' AND display_id='T144E2E'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let (acquire_at, release_at): (String, String) = conn
        .query_row(
            "SELECT \
             MAX(CASE WHEN verb='acquire' THEN occurred_at END), \
             MAX(CASE WHEN verb='release' THEN occurred_at END) \
             FROM transition_history WHERE store='resource_locks' AND display_id='main_branch'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(start_at <= acquire_at, "lock acquired after integrating entry");
    assert!(acquire_at <= release_at, "lock release follows acquisition");
    assert!(release_at <= mark_at, "lock released before verifying exit");

    let bad_integrating_steps: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history th JOIN tasks t ON t.display_id=th.display_id \
             WHERE th.display_id='T144E2E' AND t.blocked != 0",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad_integrating_steps, 0, "happy path remains unblocked");
}
