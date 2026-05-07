use clap::{Arg, Command};
use rusqlite::Connection;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::TransitionEdge;
use stores::flow::builtins::{external_review, DispatchCtx};
use stores::flow::{AgentEntry, AgentsYaml, BackoffKind, RetryPolicy, Subscription};
use stores::handlers::{next_action, row, transition};
use stores::schema::actor::Actor;
use stores::schema::Schema;

fn install_db(conn: &Connection) -> Schema {
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    let mut task_schema = None;
    for name in ["tasks", "external_reviews"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        if name == "tasks" {
            task_schema = Some(schema);
        }
    }
    task_schema.unwrap()
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
    tmp
}

fn head(workspace: &Path) -> String {
    String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(workspace)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string()
}

fn insert_task(conn: &Connection, workspace: &Path, tier: &str, status: &str) {
    conn.execute(
        "INSERT INTO tasks (display_id,status,title,slug,workspace_path,branch,tier_hint,contract,plan,cycles,wrap_log,current_phase,current_cycle,created_at,updated_at,created_by,updated_by)
         VALUES ('T900',?1,'Review task','review-task',?2,'main',?3,?4,?5,'[]',?6,1,1,'2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')",
        rusqlite::params![
            status,
            workspace.display().to_string(),
            tier,
            json!({"done_when":"done","scope_in":"in","scope_out":"out"}).to_string(),
            json!({"phases":[{"name":"p1"}]}).to_string(),
            json!([{"executive_summary":"wrapped"}]).to_string(),
        ],
    ).unwrap();
}

fn insert_review(
    conn: &Connection,
    id: &str,
    status: &str,
    verdict: &str,
    head_sha: &str,
    attempt: i64,
) {
    conn.execute(
        "INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,head_sha,verdict,created_at,updated_at,created_by,updated_by)
         VALUES (?1,?2,'T900',?3,'external_review',?4,?5,'2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')",
        rusqlite::params![id, status, attempt, head_sha, verdict],
    ).unwrap();
}

fn accept_cmd() -> Command {
    Command::new("accept").arg(Arg::new("display_id").required(true).index(1))
}

fn accept(schema: &Schema, conn: &Connection) -> anyhow::Result<()> {
    let matches = accept_cmd().get_matches_from(["accept", "T900"]);
    transition::run(schema, conn, &matches, Actor::Human.into(), "accept")
}

#[test]
fn external_review_accept_precheck_t3_without_pass_fails_loudly() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    let err = accept(&schema, &conn).unwrap_err().to_string();
    assert!(err.contains("external review PASS required"), "{err}");
}

#[test]
fn external_review_accept_precheck_t3_stale_head_pass_fails_loudly() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    insert_review(
        &conn,
        "ER001",
        "passed",
        "PASS",
        "0000000000000000000000000000000000000000",
        1,
    );
    let err = accept(&schema, &conn).unwrap_err().to_string();
    assert!(err.contains("stale external review head"), "{err}");
}

#[test]
fn external_review_accept_precheck_t3_current_head_pass_succeeds() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    insert_review(&conn, "ER001", "passed", "PASS", &head(ws.path()), 1);
    accept(&schema, &conn).unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id='T900'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "accepted");
}

#[test]
fn external_review_accept_precheck_t1_without_review_succeeds() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T1", "in_review");
    accept(&schema, &conn).unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id='T900'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "accepted");
}

#[test]
fn external_review_accept_precheck_tooling_failure_does_not_satisfy_accept() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    conn.execute_batch("ALTER TABLE external_reviews ADD COLUMN held_reason TEXT")
        .unwrap();
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    insert_review(
        &conn,
        "ER777",
        "tooling_held",
        "TOOLING_FAILURE",
        &head(ws.path()),
        1,
    );
    conn.execute(
        "UPDATE external_reviews SET held_reason='runner unavailable' WHERE display_id='ER777'",
        [],
    )
    .unwrap();
    let err = accept(&schema, &conn).unwrap_err().to_string();
    assert!(
        err.contains("ER777") && err.contains("TOOLING_FAILURE"),
        "{err}"
    );
}

fn shim(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join(format!("codex-shim-{}.sh", uuid_like()));
    std::fs::write(&p, body).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
        .to_string()
}

fn cfg(dir: &Path, shim: &Path) -> PathBuf {
    let p = dir.join(format!("config-{}.yaml", uuid_like()));
    std::fs::write(&p, format!("review:\n  runner: codex\n  max_parallel: 1\n  timeout_secs: 5\ncodex:\n  command: {}\n  args: []\n", shim.display())).unwrap();
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

#[test]
fn external_review_accept_precheck_revise_routes_executor_next_action() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T2", "in_review");
    conn.execute("INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by) VALUES ('ER900','pending','T900',1,'external_review','2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')", []).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: REVISE'\n");
    let cfg = cfg(tmp.path(), &sh);
    let agents = agents();
    let row = json!({"display_id":"ER900"});
    let ctx = DispatchCtx {
        conn: &conn,
        agents: &agents,
        config_path: &cfg,
        policies_hash: "",
    };
    external_review::run(&row, &ctx).unwrap();
    let (_id, entry) = row::read_row(&schema, &conn, "T900").unwrap();
    assert_eq!(
        entry.get("status").and_then(|v| v.as_str()),
        Some("executing")
    );
    let next = next_action::find_next_agent(schema.workflow.as_ref().unwrap(), "executing", &entry);
    assert_eq!(next.as_deref(), Some("executor"));
    assert_eq!(entry.get("current_cycle"), Some(&json!(2)));
}

#[test]
fn external_review_e2e_t3_codex_pass_then_accept_has_one_pass_row() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    conn.execute("INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by) VALUES ('ER910','pending','T900',1,'external_review','2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')", []).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(
        tmp.path(),
        "#!/bin/sh\ncat >/dev/null\necho 'VERDICT: PASS'\n",
    );
    let cfg = cfg(tmp.path(), &sh);
    let a = agents();
    let ctx = DispatchCtx {
        conn: &conn,
        agents: &a,
        config_path: &cfg,
        policies_hash: "",
    };
    external_review::run(&json!({"display_id":"ER910"}), &ctx).unwrap();

    let pass_rows: i64 = conn.query_row("SELECT COUNT(*) FROM external_reviews WHERE task_id='T900' AND verdict='PASS' AND status='passed'", [], |r| r.get(0)).unwrap();
    assert_eq!(pass_rows, 1);
    accept(&schema, &conn).unwrap();
    let status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id='T900'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "accepted");
}

#[test]
fn external_review_e2e_revise_then_executor_revision_second_bundle_has_prior_findings() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    conn.execute("INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by) VALUES ('ER920','pending','T900',1,'external_review','2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')", []).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let revise = shim(tmp.path(), "#!/bin/sh\ncat >/dev/null\necho 'VERDICT: REVISE'\necho '[major] first finding from e2e'\n");
    let revise_cfg = cfg(tmp.path(), &revise);
    let a = agents();
    external_review::run(
        &json!({"display_id":"ER920"}),
        &DispatchCtx {
            conn: &conn,
            agents: &a,
            config_path: &revise_cfg,
            policies_hash: "",
        },
    )
    .unwrap();
    let (_id, entry) = row::read_row(&schema, &conn, "T900").unwrap();
    assert_eq!(
        entry.get("status").and_then(|v| v.as_str()),
        Some("executing")
    );
    assert_eq!(
        next_action::find_next_agent(schema.workflow.as_ref().unwrap(), "executing", &entry)
            .as_deref(),
        Some("executor")
    );

    conn.execute("UPDATE tasks SET status='in_review', updated_at='2026-05-07T00:00:01Z' WHERE display_id='T900'", []).unwrap();
    conn.execute("INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by) VALUES ('ER921','pending','T900',2,'external_review','2026-05-07T00:00:01Z','2026-05-07T00:00:01Z','test','test')", []).unwrap();
    let seen = tmp.path().join("seen-prior.txt");
    let pass = shim(tmp.path(), &format!("#!/bin/sh\ncat > '{}'\ngrep -q 'first finding from e2e' '{}' || exit 7\necho 'VERDICT: PASS'\n", seen.display(), seen.display()));
    let pass_cfg = cfg(tmp.path(), &pass);
    external_review::run(
        &json!({"display_id":"ER921"}),
        &DispatchCtx {
            conn: &conn,
            agents: &a,
            config_path: &pass_cfg,
            policies_hash: "",
        },
    )
    .unwrap();
    assert!(std::fs::read_to_string(&seen)
        .unwrap()
        .contains("first finding from e2e"));
    let verdicts: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT verdict FROM external_reviews WHERE task_id='T900' ORDER BY attempt")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(verdicts, vec!["REVISE".to_string(), "PASS".to_string()]);
    accept(&schema, &conn).unwrap();
}

#[test]
fn external_review_e2e_tooling_failure_held_visible_and_accept_blocked() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    conn.execute("INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by) VALUES ('ER930','pending','T900',1,'external_review','2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')", []).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(
        tmp.path(),
        "#!/bin/sh\necho 'VERDICT: TOOLING_FAILURE'\necho '[minor] runner unavailable'\n",
    );
    let cfg = cfg(tmp.path(), &sh);
    let a = agents();
    external_review::run(
        &json!({"display_id":"ER930"}),
        &DispatchCtx {
            conn: &conn,
            agents: &a,
            config_path: &cfg,
            policies_hash: "",
        },
    )
    .unwrap();
    let rows = external_review::visible_status_rows(&conn)
        .unwrap()
        .join("\n");
    assert!(
        rows.contains("ER930")
            && rows.contains("held")
            && rows.contains("runner returned TOOLING_FAILURE"),
        "{rows}"
    );
    let err = accept(&schema, &conn).unwrap_err().to_string();
    assert!(
        err.contains("ER930") && err.contains("TOOLING_FAILURE"),
        "{err}"
    );
}
