use clap::{Arg, Command};
use rusqlite::Connection;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::TransitionEdge;
use stores::flow::builtins::{external_review, DispatchCtx};
use stores::flow::config::{CodexCfg, ReviewCfg};
use stores::flow::{AgentEntry, AgentsYaml, BackoffKind, RetryPolicy, Subscription};
use stores::handlers::{
    external_reviews::run_external_review_attempt, next_action, row, transition,
};
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

fn git(workspace: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn head(workspace: &Path) -> String {
    rev_parse(workspace, "HEAD")
}

fn rev_parse(workspace: &Path, rev: &str) -> String {
    String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", rev])
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

fn insert_task_id(
    conn: &Connection,
    id: &str,
    workspace: &Path,
    branch: &str,
    tier: &str,
    status: &str,
) {
    conn.execute(
        "INSERT INTO tasks (display_id,status,title,slug,workspace_path,branch,tier_hint,contract,plan,cycles,wrap_log,current_phase,current_cycle,created_at,updated_at,created_by,updated_by)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,'[]',?10,1,1,'2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')",
        rusqlite::params![
            id,
            status,
            format!("Review task {id}"),
            format!("review-task-{}", id.to_ascii_lowercase()),
            workspace.display().to_string(),
            branch,
            tier,
            json!({"done_when":"done","scope_in":"in","scope_out":"out"}).to_string(),
            json!({"phases":[{"name":"p1"}]}).to_string(),
            json!([{"executive_summary":"wrapped"}]).to_string(),
        ],
    ).unwrap();
}

fn insert_pending_review_for_task(conn: &Connection, review_id: &str, task_id: &str, attempt: i64) {
    conn.execute(
        "INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by) VALUES (?1,'pending',?2,?3,'external_review','2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')",
        rusqlite::params![review_id, task_id, attempt],
    )
    .unwrap();
}

fn run_review(conn: &Connection, cfg: &Path, review_id: &str) {
    let a = agents();
    external_review::run(
        &json!({"display_id": review_id}),
        &DispatchCtx {
            conn,
            agents: &a,
            config_path: cfg,
            policies_hash: "",
        },
    )
    .unwrap();
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
fn external_review_accept_precheck_t2_blocks_until_human_accept_after_pass() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T2", "in_review");
    let err = accept(&schema, &conn).unwrap_err().to_string();
    assert!(err.contains("external review PASS required"), "{err}");
    let status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id='T900'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "in_review", "PASS precheck must not auto-accept");

    insert_review(&conn, "ER002", "passed", "PASS", &head(ws.path()), 1);
    accept(&schema, &conn).unwrap();
    let accepted: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE display_id='T900'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(accepted, "accepted");
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
    let _ = conn.execute_batch("ALTER TABLE external_reviews ADD COLUMN held_reason TEXT");
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

/// MAJOR 2 fix: when `run_external_review_attempt` returns Err due to
/// `payload_error`, runner telemetry (transcript_path, log_path) MUST be
/// persisted to the DB row BEFORE the early return.
///
/// We trigger `payload_error` via a codex shim that exits non-zero (the codex
/// runner sets `payload_error` on any non-zero exit).  After the attempt we
/// assert the DB row has `log_path` and `transcript_path` populated — proving
/// that `persist_review_runner_result` ran before the Err was returned.
#[test]
fn external_review_payload_error_persists_telemetry_before_err_return() {
    let conn = install_db_bare();
    let ws = git_workspace();
    insert_task_bare(&conn, ws.path(), "T3", "in_review");
    conn.execute("INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by) VALUES ('ER940','running','T900',1,'external_review','2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')", []).unwrap();

    // Shim exits 1 → codex runner sets payload_error; stdout is non-empty so
    // the transcript has content to persist.
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(
        tmp.path(),
        "#!/bin/sh\necho 'runner output line'\necho 'more output' >&2\nexit 1\n",
    );
    let review_cfg = ReviewCfg {
        runner: "codex".to_string(),
        model: None,
        max_parallel: 1,
        timeout_secs: 5,
    };
    let codex_cfg = CodexCfg {
        command: sh.to_string_lossy().to_string(),
        args: vec![],
    };
    // run_external_review_attempt must return Err (payload_error from codex exit 1).
    let result = run_external_review_attempt(
        &conn,
        "ER940",
        "T900",
        &review_cfg,
        &codex_cfg,
        Some(ws.path()),
        None,
        None,
    );
    assert!(result.is_err(), "expected Err from payload_error path");

    // Telemetry columns must be populated (persist ran before the early return).
    let (log_path, transcript_path): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT log_path, transcript_path FROM external_reviews WHERE display_id='ER940'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(
        log_path.is_some() && !log_path.as_deref().unwrap_or("").is_empty(),
        "log_path must be persisted on payload_error early return; got {:?}",
        log_path
    );
    assert!(
        transcript_path.is_some() && !transcript_path.as_deref().unwrap_or("").is_empty(),
        "transcript_path must be persisted on payload_error early return; got {:?}",
        transcript_path
    );
}

/// Bare DB installer for MAJOR 2 test: installs only the minimal columns needed
/// by `run_external_review_attempt` + `persist_review_runner_result` without
/// requiring the full SUBSTRATE_DDL + bundled schemas setup.
fn install_db_bare() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
            display_id TEXT PRIMARY KEY,
            contract TEXT,
            plan TEXT,
            cycles TEXT,
            wrap_log TEXT,
            workspace_path TEXT,
            branch TEXT
        );
        CREATE TABLE external_reviews (
            display_id TEXT PRIMARY KEY,
            status TEXT,
            task_id TEXT,
            attempt INTEGER,
            adapter TEXT,
            verdict TEXT,
            critical_count INTEGER,
            major_count INTEGER,
            minor_count INTEGER,
            findings TEXT,
            log_path TEXT,
            transcript_path TEXT,
            model_id TEXT,
            model_metadata TEXT,
            base_sha TEXT,
            head_sha TEXT,
            started_at TEXT,
            completed_at TEXT,
            duration_ms INTEGER,
            runner TEXT,
            contract_ref TEXT,
            plan_ref TEXT,
            wrap_log_ref TEXT,
            diff_ref TEXT,
            prior_review_ref TEXT,
            created_at TEXT,
            updated_at TEXT,
            created_by TEXT,
            updated_by TEXT
        );",
    )
    .unwrap();
    conn
}

fn insert_task_bare(conn: &Connection, workspace: &std::path::Path, tier: &str, status: &str) {
    conn.execute(
        "INSERT INTO tasks (display_id, contract, plan, cycles, wrap_log, workspace_path, branch)
         VALUES ('T900',?1,?2,'[]',?3,?4,'main')",
        rusqlite::params![
            json!({"done_when":"done","scope_in":"in","scope_out":"out","tier_hint":tier})
                .to_string(),
            json!({"phases":[{"name":"p1"}]}).to_string(),
            json!([{"executive_summary":"wrapped"}]).to_string(),
            workspace.display().to_string(),
        ],
    )
    .unwrap();
    let _ = status; // status is in a separate column; insert above doesn't have it
}

#[test]
fn external_review_already_current_captures_current_main_and_unchanged_head() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    git(ws.path(), &["checkout", "-b", "task-current"]);
    std::fs::write(ws.path().join("task.txt"), "task\n").unwrap();
    git(ws.path(), &["add", "task.txt"]);
    git(ws.path(), &["commit", "-m", "task"]);
    let pre_head = head(ws.path());
    let current_main = rev_parse(ws.path(), "main");
    insert_task_id(&conn, "T901", ws.path(), "task-current", "T3", "in_review");
    insert_pending_review_for_task(&conn, "ER901", "T901", 1);

    let tmp = tempfile::tempdir().unwrap();
    let prompt = tmp.path().join("prompt-current.txt");
    let sh = shim(
        tmp.path(),
        &format!("#!/bin/sh\ncat > '{}'\necho 'VERDICT: PASS'\n", prompt.display()),
    );
    let cfg = cfg(tmp.path(), &sh);
    run_review(&conn, &cfg, "ER901");

    let (status, base_sha, head_sha): (String, String, String) = conn
        .query_row(
            "SELECT status, base_sha, head_sha FROM external_reviews WHERE display_id='ER901'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "passed");
    assert_eq!(base_sha, current_main);
    assert_eq!(head_sha, pre_head);
    assert_eq!(head(ws.path()), pre_head);
    assert!(std::fs::read_to_string(prompt).unwrap().contains(&format!("Head SHA: {pre_head}")));
}

#[test]
fn external_review_stale_branch_rebases_cleanly_before_codex() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    git(ws.path(), &["checkout", "-b", "task-stale"]);
    std::fs::write(ws.path().join("task.txt"), "task\n").unwrap();
    git(ws.path(), &["add", "task.txt"]);
    git(ws.path(), &["commit", "-m", "task"]);
    let old_head = head(ws.path());
    git(ws.path(), &["checkout", "main"]);
    std::fs::write(ws.path().join("main.txt"), "main advanced\n").unwrap();
    git(ws.path(), &["add", "main.txt"]);
    git(ws.path(), &["commit", "-m", "main advances"]);
    let new_main = head(ws.path());

    insert_task_id(&conn, "T902", ws.path(), "task-stale", "T3", "in_review");
    insert_pending_review_for_task(&conn, "ER902", "T902", 1);
    let tmp = tempfile::tempdir().unwrap();
    let count = tmp.path().join("count.txt");
    let sh = shim(
        tmp.path(),
        &format!("#!/bin/sh\ncat >/dev/null\nold=$(cat '{}' 2>/dev/null || echo 0)\nexpr $old + 1 > '{}'\necho 'VERDICT: PASS'\n", count.display(), count.display()),
    );
    let cfg = cfg(tmp.path(), &sh);
    run_review(&conn, &cfg, "ER902");

    let (status, base_sha, head_sha): (String, String, String) = conn
        .query_row(
            "SELECT status, base_sha, head_sha FROM external_reviews WHERE display_id='ER902'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "passed");
    assert_eq!(base_sha, new_main);
    assert_ne!(head_sha, old_head);
    assert_eq!(head_sha, head(ws.path()));
    assert_eq!(rev_parse(ws.path(), "task-stale"), head_sha);
    assert_eq!(std::fs::read_to_string(count).unwrap().trim(), "1");
}

#[test]
fn external_review_stale_branch_conflict_holds_without_codex() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    git(ws.path(), &["checkout", "-b", "task-conflict"]);
    std::fs::write(ws.path().join("README.md"), "task edit\n").unwrap();
    git(ws.path(), &["add", "README.md"]);
    git(ws.path(), &["commit", "-m", "task edits readme"]);
    git(ws.path(), &["checkout", "main"]);
    std::fs::write(ws.path().join("README.md"), "main edit\n").unwrap();
    git(ws.path(), &["add", "README.md"]);
    git(ws.path(), &["commit", "-m", "main edits readme"]);
    let new_main = head(ws.path());

    insert_task_id(&conn, "T903", ws.path(), "task-conflict", "T3", "in_review");
    insert_pending_review_for_task(&conn, "ER903", "T903", 1);
    let tmp = tempfile::tempdir().unwrap();
    let invoked = tmp.path().join("invoked.txt");
    let sh = shim(
        tmp.path(),
        &format!("#!/bin/sh\necho invoked > '{}'\necho 'VERDICT: PASS'\n", invoked.display()),
    );
    let cfg = cfg(tmp.path(), &sh);
    run_review(&conn, &cfg, "ER903");

    let (status, verdict, held_reason, base_sha): (String, String, String, String) = conn
        .query_row(
            "SELECT status, verdict, held_reason, base_sha FROM external_reviews WHERE display_id='ER903'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(status, "tooling_held");
    assert_eq!(verdict, "TOOLING_FAILURE");
    assert_eq!(held_reason, "stale_base_requires_rebase");
    assert_eq!(base_sha, new_main);
    assert!(!invoked.exists(), "codex shim must not run on conflicted rebase");
    assert!(!ws.path().join(".git/rebase-merge").exists());
    assert!(!ws.path().join(".git/rebase-apply").exists());
}

#[test]
fn external_review_three_task_recurrence_rebases_each_prompt_to_current_main() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    let tmp = tempfile::tempdir().unwrap();
    let prompt_dir = tmp.path().join("prompts");
    let expected_dir = tmp.path().join("expected-bases");
    let count_file = tmp.path().join("recurrence-count.txt");
    std::fs::create_dir_all(&prompt_dir).unwrap();
    std::fs::create_dir_all(&expected_dir).unwrap();
    let sh = shim(
        tmp.path(),
        &format!(
            "#!/bin/sh\nset -eu\nold=$(cat '{}' 2>/dev/null || echo 0)\nnext=$(expr $old + 1)\nprintf '%s\n' $next > '{}'\nprompt='{}/prompt-'$next'.txt'\ncat > \"$prompt\"\nbase=$(sed -n 's/^Base SHA: //p' \"$prompt\")\nif [ ! -f '{}/'$base ]; then echo 'VERDICT: REVISE'; echo '[major] stale-base omitted current mainline code'; exit 0; fi\necho 'VERDICT: PASS'\n",
            count_file.display(),
            count_file.display(),
            prompt_dir.display(),
            expected_dir.display()
        ),
    );
    let cfg = cfg(tmp.path(), &sh);

    for i in 1..=3 {
        git(ws.path(), &["checkout", "main"]);
        git(ws.path(), &["checkout", "-b", &format!("task-recur-{i}")]);
        std::fs::write(ws.path().join(format!("task-{i}.txt")), format!("task {i}\n")).unwrap();
        git(ws.path(), &["add", &format!("task-{i}.txt")]);
        git(ws.path(), &["commit", "-m", &format!("task {i}")]);
        insert_task_id(&conn, &format!("T91{i}"), ws.path(), &format!("task-recur-{i}"), "T3", "in_review");
        insert_pending_review_for_task(&conn, &format!("ER91{i}"), &format!("T91{i}"), i as i64);
    }

    for i in 1..=3 {
        git(ws.path(), &["checkout", "main"]);
        std::fs::write(ws.path().join(format!("mainline-{i}.txt")), format!("mainline {i}\n")).unwrap();
        git(ws.path(), &["add", &format!("mainline-{i}.txt")]);
        git(ws.path(), &["commit", "-m", &format!("main advances {i}")]);
        let main_sha = head(ws.path());
        std::fs::write(expected_dir.join(&main_sha), b"").unwrap();
        run_review(&conn, &cfg, &format!("ER91{i}"));
        let (status, verdict, base_sha): (String, String, String) = conn
            .query_row(
                "SELECT status, verdict, base_sha FROM external_reviews WHERE display_id=?1",
                [format!("ER91{i}")],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "passed");
        assert_eq!(verdict, "PASS");
        assert_eq!(base_sha, main_sha);
    }
}

/// ER325/ER326/ER328 (P2): On a conflicted stale rebase, `head_sha` on the
/// `tooling_held` row MUST equal the task branch tip that existed BEFORE
/// `git rebase main` was attempted, NOT a transient rebase-state HEAD.
///
/// Setup: task branch and main both edit the same line in README.md so that
/// `git rebase main` will conflict and be aborted.  We capture the task branch
/// tip before calling `run_review`, then assert the held row's `head_sha` matches.
#[test]
fn external_review_stale_conflict_tooling_held_head_sha_is_pre_rebase_tip() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();

    // Create task branch that edits the same file as main will edit later.
    git(ws.path(), &["checkout", "-b", "task-head-sha-check"]);
    std::fs::write(ws.path().join("README.md"), "task branch line\n").unwrap();
    git(ws.path(), &["add", "README.md"]);
    git(ws.path(), &["commit", "-m", "task edits README"]);

    // Record the task branch tip BEFORE any rebase attempt — this is what the
    // held row's head_sha must equal after the aborted rebase.
    let task_branch_tip = head(ws.path());

    // Advance main with a conflicting edit to the same file.
    git(ws.path(), &["checkout", "main"]);
    std::fs::write(ws.path().join("README.md"), "main conflicting line\n").unwrap();
    git(ws.path(), &["add", "README.md"]);
    git(ws.path(), &["commit", "-m", "main conflicts with task"]);

    insert_task_id(&conn, "T950", ws.path(), "task-head-sha-check", "T3", "in_review");
    insert_pending_review_for_task(&conn, "ER950", "T950", 1);

    let tmp = tempfile::tempdir().unwrap();
    // Shim would emit PASS, but it must never be called on a conflicted rebase.
    let invoked = tmp.path().join("invoked-head-sha.txt");
    let sh = shim(
        tmp.path(),
        &format!(
            "#!/bin/sh\necho invoked > '{}'\necho 'VERDICT: PASS'\n",
            invoked.display()
        ),
    );
    let cfg = cfg(tmp.path(), &sh);
    run_review(&conn, &cfg, "ER950");

    // The row must be tooling_held with the correct held_reason.
    let (status, verdict, held_reason, head_sha): (String, String, String, String) = conn
        .query_row(
            "SELECT status, verdict, held_reason, head_sha FROM external_reviews WHERE display_id='ER950'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(status, "tooling_held");
    assert_eq!(verdict, "TOOLING_FAILURE");
    assert_eq!(held_reason, "stale_base_requires_rebase");

    // Core assertion (ER328): head_sha must be the pre-rebase task branch tip,
    // not a transient rebase-in-progress commit.
    assert_eq!(
        head_sha, task_branch_tip,
        "head_sha on tooling_held row ({head_sha}) must equal the task branch tip \
         captured before git rebase ({task_branch_tip}), not a rebase-state HEAD"
    );

    // Codex shim must never have been invoked on a conflicted rebase.
    assert!(!invoked.exists(), "codex shim must not run when rebase conflicts");
    // Rebase state must be fully cleaned up.
    assert!(!ws.path().join(".git/rebase-merge").exists());
    assert!(!ws.path().join(".git/rebase-apply").exists());
}
