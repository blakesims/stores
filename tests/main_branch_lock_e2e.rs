use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::{Subscription, TransitionEdge};
use stores::flow::policies_yaml::PoliciesYaml;
use stores::flow::{AgentEntry, AgentsYaml, RetryPolicy};
use stores::handlers::agents_run::{poll_once_with_guard, FsBinaryIdentityProvider};
use stores::handlers::framework_migrate::ensure_integration_singleton_index;
use stores::handlers::resource_locks::{self, AcquireParams};
use stores::schema::actor::Actor;
use stores::schema::Schema;

fn git(repo: &Path, args: &[&str]) -> std::process::Output {
    let mut full: Vec<&str> = vec!["-C", repo.to_str().unwrap()];
    full.extend_from_slice(args);
    Command::new("git").args(&full).output().unwrap()
}

fn rev_parse(repo: &Path, rev: &str) -> String {
    let out = git(repo, &["rev-parse", rev]);
    assert!(
        out.status.success(),
        "rev-parse {rev}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
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
    std::fs::write(repo.join("doc.md"), "doc\n").unwrap();
    git(&repo, &["add", "doc.md"]);
    git(&repo, &["commit", "-m", "doc"]);
    (tmp, repo)
}

fn add_branch(repo: &Path, branch: &str, file: &str) {
    git(repo, &["checkout", "-b", branch]);
    std::fs::write(repo.join(file), format!("{file}\n")).unwrap();
    git(repo, &["add", file]);
    git(repo, &["commit", "-m", branch]);
    git(repo, &["checkout", "main"]);
}

fn prepare_db(conn: &Connection) {
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
    ensure_integration_singleton_index(conn).unwrap();
}

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    prepare_db(&conn);
    conn
}

fn fresh_file_db(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    prepare_db(&conn);
    conn
}

fn seed_queued_task(conn: &Connection, display_id: &str, branch: &str, workspace_path: &str) {
    let now = "2026-05-09T00:00:00Z";
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, activation, blocked_reason, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'integration_queued', 'test', 't', ?2, ?3, ?4, 'active', '', ?5, ?5, 'framework', 'framework')",
        rusqlite::params![display_id, branch, workspace_path, r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#, now],
    ).unwrap();
    let row_id: i64 = conn
        .query_row(
            "SELECT id FROM tasks WHERE display_id=?1",
            rusqlite::params![display_id],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO transition_history (store,row_id,display_id,from_status,to_status,verb,invoker,occurred_at) \
         VALUES ('tasks',?1,?2,'accepted','integration_queued','enqueue-integration','framework',?3)",
        rusqlite::params![row_id, display_id, now],
    ).unwrap();
}

fn agents(from: &str, to: &str, pre_land_check: &str) -> AgentsYaml {
    let mut args = serde_yaml::Mapping::new();
    args.insert(
        serde_yaml::Value::String("pre_land_check".into()),
        serde_yaml::Value::String(pre_land_check.into()),
    );
    AgentsYaml {
        agents: vec![AgentEntry {
            name: "integrate".into(),
            subscribes_to: vec![Subscription {
                store: "tasks".into(),
                transition: TransitionEdge {
                    from: from.into(),
                    to: to.into(),
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
fn cfg_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "stores-test-main-branch-lock-e2e-{}-{nanos}.yaml",
        std::process::id()
    ))
}

fn drive_until<F: Fn(&Connection) -> bool>(conn: &Connection, agents: &AgentsYaml, pred: F) {
    let p = policies();
    let cfg = cfg_path();
    for _ in 0..12 {
        poll_once_with_guard::<FsBinaryIdentityProvider>(
            conn, agents, &p, &cfg, "claimer", "epoch", None,
        )
        .unwrap();
        if pred(conn) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("predicate not satisfied");
}

fn retry_integration(conn: &Connection, display_id: &str) {
    conn.execute(
        "UPDATE tasks SET status='integration_queued' WHERE display_id=?1",
        [display_id],
    )
    .unwrap();
    let row_id: i64 = conn
        .query_row(
            "SELECT id FROM tasks WHERE display_id=?1",
            [display_id],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute(
        "INSERT INTO transition_history (store,row_id,display_id,from_status,to_status,verb,invoker,occurred_at) \
         VALUES ('tasks',?1,?2,'integration_blocked','integration_queued','retry-integration','ai_with_human','2026-05-09T01:00:00Z')",
        rusqlite::params![row_id, display_id],
    ).unwrap();
    conn.execute(
        "DELETE FROM dispatch_locks WHERE display_id=?1 AND agent_name='integrate'",
        [display_id],
    )
    .unwrap();
}

fn status(conn: &Connection, display_id: &str) -> String {
    conn.query_row(
        "SELECT status FROM tasks WHERE display_id=?1",
        [display_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn overlay(conn: &Connection, display_id: &str) -> (String, String, i64, Option<String>) {
    conn.query_row(
        "SELECT lifecycle, integration_step, blocked, blocker_kind FROM tasks WHERE display_id=?1",
        [display_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .unwrap()
}

fn lock_owner(conn: &Connection) -> Option<String> {
    conn.query_row(
        "SELECT owner_display_id FROM resource_locks WHERE resource_id='main_branch'",
        [],
        |r| r.get(0),
    )
    .ok()
}

#[test]
fn same_owner_token_rotation_blocks_before_merge_without_mutating_main() {
    let db_tmp = tempfile::tempdir().unwrap();
    let db_path = db_tmp.path().join("stores.db");
    let conn = fresh_file_db(&db_path);
    let (_repo_tmp, repo) = init_repo();
    add_branch(&repo, "feat/rotate", "rotate.txt");
    let hook = repo.join(".git/hooks/post-checkout");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nsqlite3 '{}' \"UPDATE resource_locks SET fencing_token='rotated-token' WHERE resource_id='main_branch' AND owner_display_id='T_ROTATE'\"\n",
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
    git(&repo, &["checkout", "feat/rotate"]);
    let pre_main = rev_parse(&repo, "main");
    seed_queued_task(&conn, "T_ROTATE", "feat/rotate", repo.to_str().unwrap());

    drive_until(
        &conn,
        &agents("accepted", "integration_queued", "true"),
        |c| status(c, "T_ROTATE") == "integration_blocked",
    );
    assert_eq!(rev_parse(&repo, "main"), pre_main);
    assert_eq!(
        overlay(&conn, "T_ROTATE"),
        (
            "integration".into(),
            "none".into(),
            1,
            Some("main_red".into())
        )
    );
    assert_eq!(lock_owner(&conn).as_deref(), Some("T_ROTATE"));
}

#[test]
fn busy_lock_blocks_without_mutating_main_then_retry_lands_and_releases() {
    let conn = fresh_db();
    let (_tmp, repo) = init_repo();
    add_branch(&repo, "feat/lock", "lock.txt");
    let pre_main = rev_parse(&repo, "main");
    seed_queued_task(&conn, "T_CANDIDATE", "feat/lock", repo.to_str().unwrap());

    let other = resource_locks::acquire(
        &conn,
        &AcquireParams {
            resource_id: "main_branch",
            owner_display_id: "T_OTHER",
            owner_kind: "task",
            ttl_secs: Some(600),
            claim_source: Some("test"),
            invoker: Actor::Framework,
        },
    )
    .unwrap();

    drive_until(
        &conn,
        &agents("accepted", "integration_queued", "true"),
        |c| status(c, "T_CANDIDATE") == "integration_blocked",
    );
    assert_eq!(rev_parse(&repo, "main"), pre_main);
    assert_eq!(
        overlay(&conn, "T_CANDIDATE"),
        (
            "integration".into(),
            "none".into(),
            1,
            Some("main_red".into())
        )
    );
    assert_eq!(lock_owner(&conn).as_deref(), Some("T_OTHER"));

    resource_locks::release(&conn, "main_branch", &other.0, Actor::Framework).unwrap();
    retry_integration(&conn, "T_CANDIDATE");
    drive_until(
        &conn,
        &agents("integration_blocked", "integration_queued", "true"),
        |c| status(c, "T_CANDIDATE") == "integrated",
    );
    assert!(lock_owner(&conn).is_none());
    let audits: i64 = conn.query_row("SELECT COUNT(*) FROM transition_history WHERE store='resource_locks' AND display_id='main_branch' AND invoker='framework'", [], |r| r.get(0)).unwrap();
    assert!(
        audits >= 2,
        "expected acquire+release resource_locks audits, got {audits}"
    );
}

#[test]
fn stale_freshness_reroute_releases_main_branch_lock() {
    let conn = fresh_db();
    let (_tmp, repo) = init_repo();
    git(&repo, &["checkout", "-b", "feat/conflict"]);
    std::fs::write(repo.join("doc.md"), "candidate\n").unwrap();
    git(&repo, &["add", "doc.md"]);
    git(&repo, &["commit", "-m", "candidate"]);
    git(&repo, &["checkout", "main"]);
    seed_queued_task(&conn, "T_FAIL", "feat/conflict", repo.to_str().unwrap());
    let script = format!("set -e; cd {}; git checkout main; echo main > doc.md; git add doc.md; git commit -m main-change; git checkout feat/conflict", repo.to_str().unwrap());

    drive_until(
        &conn,
        &agents("accepted", "integration_queued", &script),
        |c| status(c, "T_FAIL") == "integration_blocked",
    );
    let (blocked, blocker_kind, reason): (bool, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT blocked, blocker_kind, integration_blocked_reason FROM tasks WHERE display_id='T_FAIL'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!(blocked);
    assert_eq!(blocker_kind.as_deref(), Some("main_red"));
    assert!(reason.unwrap_or_default().starts_with("stale_review"));
    assert!(
        lock_owner(&conn).is_none(),
        "Drop guard must release after stale freshness reroute"
    );
}

#[test]
fn stale_lock_recovery_writes_transition_history() {
    let conn = fresh_db();
    let (_tmp, repo) = init_repo();
    add_branch(&repo, "feat/stale", "stale.txt");
    seed_queued_task(&conn, "T_STALE", "feat/stale", repo.to_str().unwrap());
    conn.execute(
        "INSERT INTO resource_locks (resource_id, owner_kind, owner_display_id, fencing_token, acquired_at, expires_at, claim_source) \
         VALUES ('main_branch','task','T_OLD','tok-old','2000-01-01T00:00:00Z','2000-01-01T00:00:01Z','test')",
        [],
    ).unwrap();

    drive_until(
        &conn,
        &agents("accepted", "integration_queued", "true"),
        |c| status(c, "T_STALE") == "integrated",
    );
    assert!(lock_owner(&conn).is_none());
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transition_history WHERE store='resource_locks' AND display_id='main_branch' AND verb='recover_stale' AND invoker='framework'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(n, 1);
}
