use rusqlite::Connection;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::TransitionEdge;
use stores::flow::builtins::{external_review, DispatchCtx};
use stores::flow::{AgentEntry, AgentsYaml, BackoffKind, RetryPolicy, Subscription};
use stores::schema::Schema;

fn install_db(conn: &Connection) {
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    for name in ["tasks", "external_reviews"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
    }
}

fn git_workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "t@example.com"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "T"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::fs::write(tmp.path().join("README.md"), "base\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "base"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::fs::write(tmp.path().join("README.md"), "base\nhead\n").unwrap();
    tmp
}

fn insert_task(conn: &Connection, workspace: &Path, status: &str) {
    conn.execute(
        "INSERT INTO tasks (display_id,status,title,slug,workspace_path,branch,tier_hint,contract,plan,cycles,wrap_log,current_phase,current_cycle,created_at,updated_at,created_by,updated_by)
         VALUES ('T900',?1,'Review task','review-task',?2,'main','T2',?3,?4,'[]',?5,1,1,'2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')",
        rusqlite::params![
            status,
            workspace.display().to_string(),
            json!({"done_when":"done","scope_in":"in","scope_out":"out"}).to_string(),
            json!({"phases":[{"name":"p1"}]}).to_string(),
            json!([{"executive_summary":"wrapped"}]).to_string(),
        ],
    ).unwrap();
}

fn insert_review(conn: &Connection, id: &str, task: &str, status: &str, attempt: i64) -> i64 {
    conn.execute(
        "INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by)
         VALUES (?1,?2,?3,?4,'external_review','2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')",
        rusqlite::params![id, status, task, attempt],
    ).unwrap();
    conn.last_insert_rowid()
}

fn shim(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("codex-shim.sh");
    std::fs::write(&p, body).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn cfg(dir: &Path, shim: &Path, max_parallel: u32) -> PathBuf {
    let p = dir.join("config.yaml");
    std::fs::write(
        &p,
        format!(
            "review:\n  runner: codex\n  max_parallel: {max_parallel}\n  timeout_secs: 5\ncodex:\n  command: {}\n  args: []\n",
            shim.display()
        ),
    )
    .unwrap();
    p
}

fn agents() -> AgentsYaml {
    AgentsYaml {
        agents: vec![AgentEntry {
            name: "external-review".to_string(),
            subscribes_to: vec![Subscription {
                store: "external_reviews".to_string(),
                transition: TransitionEdge {
                    from: "".to_string(),
                    to: "pending".to_string(),
                },
                predicate: None,
            }],
            command: "builtin:external-review".to_string(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy {
                max_attempts: 1,
                backoff: BackoffKind::Linear,
            },
            command_args: None,
        }],
        deployment_specialist: None,
    }
}

fn ctx<'a>(conn: &'a Connection, agents: &'a AgentsYaml, cfg: &'a Path) -> DispatchCtx<'a> {
    DispatchCtx {
        conn,
        agents,
        config_path: cfg,
        policies_hash: "",
    }
}

#[test]
fn external_review_daemon_cap_hold_marks_second_pending_visible() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    external_review::visible_status_rows(&conn).unwrap(); // adds runtime cols
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    let cfg = cfg(tmp.path(), &sh, 1);
    insert_review(&conn, "ER001", "T900", "running", 1);
    insert_review(&conn, "ER002", "T901", "pending", 1);
    assert!(!external_review::cap_allows_or_log(&conn, &cfg, "ER002").unwrap());
    let (status, held): (String, String) = conn
        .query_row(
            "SELECT status, held_reason FROM external_reviews WHERE display_id='ER002'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(held, "cap-held");
    assert!(external_review::visible_status_rows(&conn)
        .unwrap()
        .join("\n")
        .contains("external-review task_id=T901 review_attempt_id=ER002"));
}

#[test]
fn external_review_daemon_tooling_failure_holds_with_retry_and_refs() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "in_review");
    insert_review(&conn, "ER003", "T900", "pending", 1);
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: TOOLING_FAILURE'\n");
    let cfg = cfg(tmp.path(), &sh, 1);
    let a = agents();
    let row = json!({"display_id":"ER003"});
    external_review::run(&row, &ctx(&conn, &a, &cfg)).unwrap();
    let (task_status, status, verdict, retry, log): (String, String, String, Option<String>, Option<String>) = conn.query_row(
        "SELECT t.status, er.status, er.verdict, er.next_retry_at, er.log_path FROM external_reviews er JOIN tasks t ON t.display_id=er.task_id WHERE er.display_id='ER003'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    ).unwrap();
    assert_eq!(task_status, "in_review");
    assert_eq!(status, "tooling_held");
    assert_eq!(verdict, "TOOLING_FAILURE");
    assert!(retry.is_some());
    assert!(log.unwrap().contains("codex"));
}

/// `tooling_held` rows with an elapsed `next_retry_at` are promoted back to
/// `pending` at the top of `run()` so they are re-tried on the same iteration.
#[test]
fn external_review_daemon_tooling_failure_retries_after_next_retry_at() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "in_review");
    // Pre-insert a tooling_held row with a next_retry_at that is already in the past.
    insert_review(&conn, "ER010", "T900", "tooling_held", 1);
    // Ensure the runtime columns exist (normally created by run()), then set the
    // past retry timestamp and verdict directly.
    external_review::visible_status_rows(&conn).unwrap();
    conn.execute(
        "UPDATE external_reviews SET next_retry_at='2000-01-01T00:00:00Z', held_reason='prev tooling failure', verdict='TOOLING_FAILURE' WHERE display_id='ER010'",
        [],
    ).unwrap();

    // Dispatch run() for ER010. promote_elapsed_tooling_held() fires first,
    // transitions ER010 → pending, then load_review_row() sees pending and
    // the normal path executes with the PASS shim.
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    let cfg = cfg(tmp.path(), &sh, 1);
    let a = agents();
    let row = json!({"display_id": "ER010"});
    external_review::run(&row, &ctx(&conn, &a, &cfg)).unwrap();

    let (status, verdict): (String, String) = conn.query_row(
        "SELECT status, verdict FROM external_reviews WHERE display_id='ER010'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    // After retry: promoted to pending, then ran to completion with PASS.
    assert_eq!(verdict, "PASS");
    assert_eq!(status, "passed");
}

/// Atomic CAS test: calling `promote_elapsed_tooling_held` twice on the same
/// elapsed `tooling_held` row (simulating two concurrent daemons) must result
/// in exactly ONE transition history record and ONE status change to `pending`.
/// The second invocation must be a no-op because the CAS UPDATE checks
/// `AND status='tooling_held'` — after the first call the row is `pending`.
#[test]
fn external_review_daemon_concurrent_promote_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "in_review");
    insert_review(&conn, "ER020", "T900", "tooling_held", 1);
    external_review::visible_status_rows(&conn).unwrap();
    conn.execute(
        "UPDATE external_reviews SET next_retry_at='2000-01-01T00:00:00Z', held_reason='prev tooling failure', verdict='TOOLING_FAILURE' WHERE display_id='ER020'",
        [],
    ).unwrap();

    // First promote: should succeed and insert one history record.
    external_review::promote_elapsed_tooling_held(&conn).unwrap();
    let (status_after_first,): (String,) = conn.query_row(
        "SELECT status FROM external_reviews WHERE display_id='ER020'",
        [],
        |r| Ok((r.get(0)?,)),
    ).unwrap();
    assert_eq!(status_after_first, "pending", "first promote must set status=pending");

    let history_after_first: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transition_history WHERE row_id=(SELECT id FROM external_reviews WHERE display_id='ER020') AND from_status='tooling_held' AND to_status='pending'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(history_after_first, 1, "first promote must insert exactly one history record");

    // Second promote: the row is already pending; the CAS must be a no-op.
    external_review::promote_elapsed_tooling_held(&conn).unwrap();
    let (status_after_second,): (String,) = conn.query_row(
        "SELECT status FROM external_reviews WHERE display_id='ER020'",
        [],
        |r| Ok((r.get(0)?,)),
    ).unwrap();
    assert_eq!(status_after_second, "pending", "second promote must leave status=pending");

    let history_after_second: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transition_history WHERE row_id=(SELECT id FROM external_reviews WHERE display_id='ER020') AND from_status='tooling_held' AND to_status='pending'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(
        history_after_second, 1,
        "second (no-op) promote must NOT insert a duplicate history record"
    );
}

#[test]
fn external_review_daemon_pass_and_revise_update_status_and_revise_routes_task() {
    for (id, verdict, expected_status, expected_task) in [
        ("ER004", "PASS", "passed", "in_review"),
        ("ER005", "REVISE", "revise", "executing"),
    ] {
        let conn = Connection::open_in_memory().unwrap();
        install_db(&conn);
        let ws = git_workspace();
        insert_task(&conn, ws.path(), "in_review");
        insert_review(&conn, id, "T900", "pending", 1);
        let tmp = tempfile::tempdir().unwrap();
        let sh = shim(
            tmp.path(),
            &format!("#!/bin/sh\necho 'VERDICT: {verdict}'\n"),
        );
        let cfg = cfg(tmp.path(), &sh, 1);
        let a = agents();
        let row = json!({"display_id": id});
        external_review::run(&row, &ctx(&conn, &a, &cfg)).unwrap();
        let (status, got_verdict, task_status): (String, String, String) = conn.query_row(
            "SELECT er.status, er.verdict, t.status FROM external_reviews er JOIN tasks t ON t.display_id=er.task_id WHERE er.display_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(status, expected_status);
        assert_eq!(got_verdict, verdict);
        assert_eq!(task_status, expected_task);
    }
}
