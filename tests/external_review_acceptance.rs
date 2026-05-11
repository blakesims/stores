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
    external_reviews::{import_manual_pass, run_external_review_attempt, ImportPassArgs},
    next_action, row, transition,
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

fn import_pass(
    conn: &Connection,
    task_id: &str,
    transcript_path: &Path,
    base_sha: &str,
    head_sha: &str,
) -> anyhow::Result<String> {
    import_manual_pass(
        conn,
        ImportPassArgs {
            task_id,
            transcript_path,
            base_sha,
            head_sha,
            runner: "manual-codex",
            actor: "ai_autonomous",
        },
    )
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
fn manual_import_pass_creates_auditable_pass_row() {
    let conn = Connection::open_in_memory().unwrap();
    let _schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    let transcript = ws.path().join("manual-codex.txt");
    std::fs::write(&transcript, "PASS\nreview transcript\n").unwrap();
    let base = rev_parse(ws.path(), "main");
    let head = head(ws.path());

    let er_id = import_pass(&conn, "T900", &transcript, &base, &head).unwrap();
    assert_eq!(er_id, "ER001");
    let row: (String, String, String, String, String, i64, i64, i64) = conn
        .query_row(
            "SELECT status, verdict, runner, transcript_path, head_sha, critical_count, major_count, minor_count FROM external_reviews WHERE display_id='ER001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .unwrap();
    assert_eq!(row.0, "passed");
    assert_eq!(row.1, "PASS");
    assert_eq!(row.2, "codex");
    assert_eq!(row.3, transcript.display().to_string());
    assert_eq!(row.4, head);
    assert_eq!((row.5, row.6, row.7), (0, 0, 0));
    let verb: String = conn
        .query_row(
            "SELECT verb FROM transition_history WHERE store='external_reviews' AND display_id='ER001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(verb, "import-pass");
}

#[test]
fn accept_precheck_recognizes_manual_import_at_current_head() {
    let conn = Connection::open_in_memory().unwrap();
    let schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    let transcript = ws.path().join("manual.txt");
    std::fs::write(&transcript, "PASS\n").unwrap();
    let base = rev_parse(ws.path(), "main");
    let head = head(ws.path());
    import_pass(&conn, "T900", &transcript, &base, &head).unwrap();

    accept(&schema, &conn).unwrap();
    let status: String = conn
        .query_row("SELECT status FROM tasks WHERE display_id='T900'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(status, "accepted");
}

#[test]
fn manual_import_rejects_wrong_head_or_base() {
    let conn = Connection::open_in_memory().unwrap();
    let _schema = install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "T3", "in_review");
    let transcript = ws.path().join("manual.txt");
    std::fs::write(&transcript, "PASS\n").unwrap();
    let head = head(ws.path());
    let base = rev_parse(ws.path(), "main");
    let wrong = "0000000000000000000000000000000000000000";

    let err = import_pass(&conn, "T900", &transcript, &base, wrong).unwrap_err().to_string();
    assert!(err.contains("head mismatch"), "{err}");
    let err = import_pass(&conn, "T900", &transcript, wrong, &head).unwrap_err().to_string();
    assert!(err.contains("base mismatch"), "{err}");
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM external_reviews", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
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
                integration_step: None,
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
    insert_task_bare(&conn, ws.path(), "T900", "in_review");
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

/// L498 backfill (Pi msg_a423719b): legacy stale_base_requires_rebase rows
/// created before the bounded-retry fix (c8d993e) have next_retry_at=NULL and
/// are permanently held. The reconcile-loop backfill must give them a bounded
/// next_retry_at so the existing elapsed-tooling-held retry path drains them.
#[test]
fn external_review_legacy_stale_base_held_row_gets_backfilled_next_retry_at() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);

    // Insert a synthetic legacy held row with next_retry_at=NULL — same shape
    // as ER330 in production before the c8d993e fix shipped.
    conn.execute(
        "INSERT INTO external_reviews (display_id, task_id, attempt, adapter, runner, status, verdict, held_reason, next_retry_at, base_sha, head_sha, created_at, updated_at) \
         VALUES ('ER999', 'T999', 1, 'external_review', 'codex', 'tooling_held', 'TOOLING_FAILURE', 'stale_base_requires_rebase', NULL, 'oldmain', 'oldhead', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    ).unwrap();

    // Run the backfill (the function the engine_runner reconcile-loop now calls).
    stores::flow::builtins::external_review::backfill_stale_base_next_retry(&conn).unwrap();

    // The legacy row should now have a non-NULL next_retry_at, preserving
    // status=tooling_held and held_reason for operator visibility.
    let (status, held_reason, next_retry_at): (String, String, Option<String>) = conn
        .query_row(
            "SELECT status, held_reason, next_retry_at FROM external_reviews WHERE display_id='ER999'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "tooling_held");
    assert_eq!(held_reason, "stale_base_requires_rebase");
    let next_retry = next_retry_at
        .expect("backfill must populate next_retry_at on legacy stale_base rows");
    assert!(!next_retry.is_empty());

    // Idempotent: running again must not re-write the timestamp on rows that
    // already have a populated next_retry_at.
    let snapshot = next_retry.clone();
    stores::flow::builtins::external_review::backfill_stale_base_next_retry(&conn).unwrap();
    let unchanged: String = conn
        .query_row(
            "SELECT next_retry_at FROM external_reviews WHERE display_id='ER999'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unchanged, snapshot, "backfill must not touch already-populated rows");

    // Non-stale-base held rows must NOT be touched.
    conn.execute(
        "INSERT INTO external_reviews (display_id, task_id, attempt, adapter, runner, status, verdict, held_reason, next_retry_at, base_sha, head_sha, created_at, updated_at) \
         VALUES ('ER998', 'T998', 1, 'external_review', 'codex', 'tooling_held', 'TOOLING_FAILURE', 'parse-fallback-needed', NULL, 'a', 'b', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        [],
    ).unwrap();
    stores::flow::builtins::external_review::backfill_stale_base_next_retry(&conn).unwrap();
    let other_retry: Option<String> = conn
        .query_row(
            "SELECT next_retry_at FROM external_reviews WHERE display_id='ER998'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(other_retry.is_none(), "non-stale-base held rows must not be backfilled");
}

/// L488 recovery gap (Pi msg_3cf7c3af): stale_base_requires_rebase rows MUST
/// have a bounded next_retry_at so the existing T086/L197 elapsed-tooling-held
/// retry path can drain them after the operator rebases the worktree. Prior
/// to this fix, next_retry_at=NULL meant the row was permanently held.
#[test]
fn external_review_stale_conflict_tooling_held_has_bounded_next_retry_at() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();

    git(ws.path(), &["checkout", "-b", "task-retry-bounded"]);
    std::fs::write(ws.path().join("README.md"), "task branch line\n").unwrap();
    git(ws.path(), &["add", "README.md"]);
    git(ws.path(), &["commit", "-m", "task edits README"]);

    git(ws.path(), &["checkout", "main"]);
    std::fs::write(ws.path().join("README.md"), "main conflicting line\n").unwrap();
    git(ws.path(), &["add", "README.md"]);
    git(ws.path(), &["commit", "-m", "main conflicts with task"]);

    insert_task_id(&conn, "T960", ws.path(), "task-retry-bounded", "T3", "in_review");
    insert_pending_review_for_task(&conn, "ER960", "T960", 1);

    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    let cfg = cfg(tmp.path(), &sh);
    run_review(&conn, &cfg, "ER960");

    let (status, held_reason, next_retry_at): (String, String, Option<String>) = conn
        .query_row(
            "SELECT status, held_reason, next_retry_at FROM external_reviews WHERE display_id='ER960'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(status, "tooling_held");
    assert_eq!(held_reason, "stale_base_requires_rebase");

    // Core assertion: next_retry_at MUST be set (not NULL) so the
    // elapsed-tooling-held retry path can drain the row after operator rebase.
    let next_retry = next_retry_at
        .expect("next_retry_at must be set so retry machinery can drain stale_base rows");
    assert!(
        !next_retry.is_empty(),
        "next_retry_at must be a non-empty timestamp, got: {next_retry}"
    );
}

// ============================================================
// T105 — recover-stale-base tests (Tasks 1.19-1.28)
// ============================================================

use stores::handlers::recover_stale_base::run_recover_stale_base;
use stores::schema::actor::InvokerCtx;

fn er_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "external_reviews")
        .map(|(_, y)| *y)
        .unwrap();
    Schema::from_yaml(yaml).unwrap()
}

fn insert_tooling_held_stale_base(
    conn: &Connection,
    er_id: &str,
    task_id: &str,
    attempt: i64,
    head_sha: &str,
    base_sha: &str,
) {
    conn.execute(
        "INSERT INTO external_reviews \
         (display_id, status, task_id, attempt, adapter, head_sha, base_sha, verdict, held_reason, next_retry_at, \
          created_at, updated_at, created_by, updated_by) \
         VALUES (?1,'tooling_held',?2,?3,'external_review',?4,?5,'TOOLING_FAILURE','stale_base_requires_rebase',NULL, \
                 '2026-05-01T00:00:00Z','2026-05-01T00:00:00Z','test','test')",
        rusqlite::params![er_id, task_id, attempt, head_sha, base_sha],
    )
    .unwrap();
}

fn invoker_ai_with_human() -> InvokerCtx {
    InvokerCtx { actor: Actor::AiWithHuman, token_valid: false }
}

fn invoker_human() -> InvokerCtx {
    InvokerCtx { actor: Actor::Human, token_valid: false }
}

/// Test 1 (Task 1.19): single held → recovery, then PASS e2e.
#[test]
fn recover_stale_base_test1_single_held_then_pass() {
    let conn = Connection::open_in_memory().unwrap();
    let _schema = install_db(&conn);
    let er_s = er_schema();

    let ws = git_workspace();

    // Create task branch with a commit.
    git(ws.path(), &["checkout", "-b", "task-t1-recover"]);
    std::fs::write(ws.path().join("task-t1.txt"), "task\n").unwrap();
    git(ws.path(), &["add", "task-t1.txt"]);
    git(ws.path(), &["commit", "-m", "task commit"]);
    let pre_rebase_head = head(ws.path());

    // Advance main (separate file, no conflict).
    git(ws.path(), &["checkout", "main"]);
    std::fs::write(ws.path().join("main-advance.txt"), "main\n").unwrap();
    git(ws.path(), &["add", "main-advance.txt"]);
    git(ws.path(), &["commit", "-m", "main advances"]);
    let new_main = head(ws.path());

    // Insert task row.
    insert_task_id(&conn, "T2001", ws.path(), "task-t1-recover", "T3", "in_review");

    // Insert one tooling_held/stale_base ER (simulating what the daemon would have created).
    insert_tooling_held_stale_base(&conn, "ER2001", "T2001", 1, &pre_rebase_head, "oldmain");

    // Operator rebases task branch.
    git(ws.path(), &["checkout", "task-t1-recover"]);
    git(ws.path(), &["rebase", "main"]);
    let post_rebase_head = head(ws.path());
    assert_ne!(post_rebase_head, pre_rebase_head, "rebase must move HEAD");

    // Call recover-stale-base.
    run_recover_stale_base(&conn, &er_s, "T2001", invoker_ai_with_human()).unwrap();

    // Assert ER2001 superseded with append-only fields intact (AC1.6).
    let (status, superseded_by, held_head, held_base, held_reason): (String, String, String, String, String) = conn
        .query_row(
            "SELECT status, superseded_by, head_sha, base_sha, held_reason FROM external_reviews WHERE display_id='ER2001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(status, "superseded");
    assert_eq!(superseded_by, "ER2002", "superseded_by must be ER2002");
    assert_eq!(held_head, pre_rebase_head, "head_sha must be unchanged (append-only)");
    assert_eq!(held_base, "oldmain", "base_sha must be unchanged (append-only)");
    assert_eq!(held_reason, "stale_base_requires_rebase", "held_reason must be unchanged (append-only)");

    // Assert ER2002 is pending with current SHAs (AC1.7).
    let (new_status, new_head, new_base): (String, String, String) = conn
        .query_row(
            "SELECT status, head_sha, base_sha FROM external_reviews WHERE display_id='ER2002'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(new_status, "pending");
    assert_eq!(new_head, post_rebase_head, "new ER head_sha must be post-rebase tip");
    assert_eq!(new_base, new_main, "new ER base_sha must be current main");

    // Watch-row exclusion: only pending appears in watch filter (AC1.7).
    let watch_rows: Vec<String> = {
        let mut s = conn
            .prepare(
                "SELECT display_id FROM external_reviews \
                 WHERE status IN ('pending','running','tooling_held') AND task_id='T2001'",
            )
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(watch_rows, vec!["ER2002"], "only new pending ER must appear in watch filter");

    // Transition history: one supersede row + one create row (AC1.9).
    let th_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history WHERE store='external_reviews'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(th_count >= 2, "must have at least 2 transition_history rows (1 supersede + 1 create)");

    let create_row: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='external_reviews' AND from_status='' AND to_status='pending' \
               AND verb='recover-stale-base' AND display_id='ER2002'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(create_row, 1, "must have one creation history row for ER2002");

    let supersede_row: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE store='external_reviews' AND from_status='tooling_held' AND to_status='superseded' \
               AND verb='supersede' AND display_id='ER2001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(supersede_row, 1, "must have one supersede history row for ER2001");

    // Idempotent fail-loud on second call (AC1.8): no new held rows, ER2002 pending.
    let err2 = run_recover_stale_base(&conn, &er_s, "T2001", invoker_ai_with_human())
        .unwrap_err()
        .to_string();
    assert!(
        err2.contains("no stale_base_requires_rebase") || err2.contains("fresh external_review"),
        "second call must fail-loud: {err2}"
    );

    // E2e: run ER2002 with PASS shim → task advances to accepted.
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\ncat >/dev/null\necho 'VERDICT: PASS'\n");
    let cfg_path = cfg(tmp.path(), &sh);
    let a = agents();
    external_review::run(
        &json!({"display_id": "ER2002"}),
        &DispatchCtx { conn: &conn, agents: &a, config_path: &cfg_path, policies_hash: "" },
    )
    .unwrap();
    let new_er_status: String = conn
        .query_row(
            "SELECT status FROM external_reviews WHERE display_id='ER2002'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(new_er_status, "passed");
}

/// Test 2 (Task 1.20): 5 stacked held → all superseded with same new ER id.
#[test]
fn recover_stale_base_test2_five_stacked_held() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let er_s = er_schema();

    let ws = git_workspace();
    git(ws.path(), &["checkout", "-b", "task-t2-recover"]);
    std::fs::write(ws.path().join("task.txt"), "task\n").unwrap();
    git(ws.path(), &["add", "task.txt"]);
    git(ws.path(), &["commit", "-m", "task"]);
    let pre_head = head(ws.path());

    // Advance main.
    git(ws.path(), &["checkout", "main"]);
    std::fs::write(ws.path().join("m.txt"), "m\n").unwrap();
    git(ws.path(), &["add", "m.txt"]);
    git(ws.path(), &["commit", "-m", "main"]);

    insert_task_id(&conn, "T2010", ws.path(), "task-t2-recover", "T3", "in_review");

    // Insert 5 held rows with different attempts.
    for i in 1..=5 {
        insert_tooling_held_stale_base(
            &conn,
            &format!("ER20{:02}", 9 + i),
            "T2010",
            i,
            &pre_head,
            "oldmain",
        );
    }

    // Rebase task branch.
    git(ws.path(), &["checkout", "task-t2-recover"]);
    git(ws.path(), &["rebase", "main"]);

    run_recover_stale_base(&conn, &er_s, "T2010", invoker_ai_with_human()).unwrap();

    // All 5 held rows → superseded with same new ER id.
    let superseded_ids: Vec<String> = {
        let mut s = conn
            .prepare(
                "SELECT DISTINCT superseded_by FROM external_reviews \
                 WHERE task_id='T2010' AND status='superseded'",
            )
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(superseded_ids.len(), 1, "all held rows must share the same superseded_by");

    let superseded_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_reviews WHERE task_id='T2010' AND status='superseded'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(superseded_count, 5, "all 5 held rows must be superseded");

    let pending_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_reviews WHERE task_id='T2010' AND status='pending'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(pending_count, 1, "exactly one new pending ER must be created");
}

/// Test 3 (Task 1.21): no held rows → fail-loud with 'no stale_base_requires_rebase'.
#[test]
fn recover_stale_base_test3_no_held_rows() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let er_s = er_schema();

    let ws = git_workspace();
    insert_task_id(&conn, "T2020", ws.path(), "main", "T3", "in_review");

    let err = run_recover_stale_base(&conn, &er_s, "T2020", invoker_ai_with_human())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("no stale_base_requires_rebase"),
        "error must contain 'no stale_base_requires_rebase': {err}"
    );
}

/// Test 4 (Task 1.22): held row head_sha == current HEAD → fail-loud with 'rebase the task branch'.
#[test]
fn recover_stale_base_test4_no_rebase_performed() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let er_s = er_schema();

    let ws = git_workspace();
    git(ws.path(), &["checkout", "-b", "task-t4-norebase"]);
    std::fs::write(ws.path().join("t4.txt"), "t4\n").unwrap();
    git(ws.path(), &["add", "t4.txt"]);
    git(ws.path(), &["commit", "-m", "t4"]);
    let current_tip = head(ws.path());

    insert_task_id(&conn, "T2030", ws.path(), "task-t4-norebase", "T3", "in_review");
    // Insert held row with head_sha == current tip (no rebase performed).
    insert_tooling_held_stale_base(&conn, "ER2030", "T2030", 1, &current_tip, "oldmain");

    let err = run_recover_stale_base(&conn, &er_s, "T2030", invoker_ai_with_human())
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("rebase the task branch"),
        "error must contain 'rebase the task branch': {err}"
    );
    assert!(err.contains(&current_tip[..7]), "error must include current head SHA: {err}");
}

/// Test 5a (Task 1.23): handler-level AiAutonomous gate.
#[test]
fn recover_stale_base_test5a_handler_level_ai_autonomous_rejected() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let er_s = er_schema();

    let ws = git_workspace();
    insert_task_id(&conn, "T2040", ws.path(), "main", "T3", "in_review");

    let ai_auto = InvokerCtx { actor: Actor::AiAutonomous, token_valid: false };
    let err = run_recover_stale_base(&conn, &er_s, "T2040", ai_auto)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("ai_with_human"),
        "AiAutonomous error must contain 'ai_with_human': {err}"
    );

    // Positive control: AiWithHuman proceeds past the gate (fails on no held rows, not on actor).
    let ai_human = invoker_ai_with_human();
    let err2 = run_recover_stale_base(&conn, &er_s, "T2040", ai_human)
        .unwrap_err()
        .to_string();
    assert!(
        !err2.contains("ai_with_human"),
        "AiWithHuman must pass the actor gate; got: {err2}"
    );
    assert!(
        err2.contains("no stale_base_requires_rebase") || err2.contains("not found"),
        "AiWithHuman must fail for a different reason: {err2}"
    );
}

/// Test 5b (Task 1.24): CLI-level dispatch with ai_autonomous is rejected with 'ai_with_human'.
#[test]
fn recover_stale_base_test5b_cli_dispatch_ai_autonomous_rejected() {
    use std::collections::HashMap;
    use stores::cli::{dispatch, dynamic::build_root};
    use stores::manifest::{InstalledStore, Manifest};
    use stores::schema::StoreScope;

    // Build manifest with tasks store.
    let manifest = Manifest {
        stores: vec![InstalledStore {
            name: "tasks".to_string(),
            schema_path: std::path::PathBuf::from("bundled:tasks"),
            installed_at: "2026-01-01T00:00:00Z".to_string(),
            table_name: "tasks".to_string(),
            scope: StoreScope::Repo,
        }],
    };

    // Build schemas map with tasks + external_reviews.
    let mut schemas = HashMap::new();
    for name in ["tasks", "external_reviews"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        schemas.insert(name.to_string(), schema);
    }

    // Build CLI command tree and parse args.
    let cmd = build_root(&manifest, &schemas);
    let matches = cmd
        .try_get_matches_from([
            "stores",
            "tasks",
            "recover-stale-base",
            "T900",
            "--invoker",
            "ai_autonomous",
        ])
        .unwrap();

    // Create a temp dir with .stores/ so db_path() resolves and db::open() can create the file.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".stores")).unwrap();
    let orig_cwd = std::env::current_dir().unwrap();

    // Use a file-local mutex to prevent CWD interference across parallel tests.
    static CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_current_dir(tmp.path()).unwrap();

    let result = dispatch::dispatch(&matches, &manifest, &schemas);

    // Restore CWD before asserting.
    let _ = std::env::set_current_dir(&orig_cwd);
    drop(_guard);

    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("ai_with_human"),
        "CLI dispatch with ai_autonomous must return error containing 'ai_with_human': {err}"
    );
}

/// Test 6 (Task 1.25): T098-shape e2e — conflict → held → rebase → recover → PASS.
#[test]
fn recover_stale_base_test6_t098_shape_e2e() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let er_s = er_schema();

    let ws = git_workspace();

    // Task branch edits README.md (will conflict with main).
    git(ws.path(), &["checkout", "-b", "task-t6-conflict"]);
    std::fs::write(ws.path().join("README.md"), "task version\n").unwrap();
    git(ws.path(), &["add", "README.md"]);
    git(ws.path(), &["commit", "-m", "task edits README"]);

    // Main also edits README.md (conflict).
    git(ws.path(), &["checkout", "main"]);
    std::fs::write(ws.path().join("README.md"), "main conflicting version\n").unwrap();
    git(ws.path(), &["add", "README.md"]);
    git(ws.path(), &["commit", "-m", "main conflicts"]);

    insert_task_id(&conn, "T2060", ws.path(), "task-t6-conflict", "T3", "in_review");
    insert_pending_review_for_task(&conn, "ER2060", "T2060", 1);

    // Run external review → conflict → tooling_held/stale_base.
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    let cfg_path = cfg(tmp.path(), &sh);
    let a = agents();
    external_review::run(
        &json!({"display_id": "ER2060"}),
        &DispatchCtx { conn: &conn, agents: &a, config_path: &cfg_path, policies_hash: "" },
    )
    .unwrap();

    let held_reason: String = conn
        .query_row(
            "SELECT held_reason FROM external_reviews WHERE display_id='ER2060'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(held_reason, "stale_base_requires_rebase");

    // Operator resolves: rebase with -X ours to keep task branch content.
    git(ws.path(), &["checkout", "task-t6-conflict"]);
    git(ws.path(), &["-c", "rebase.instructionFormat=%s", "rebase", "-X", "ours", "main"]);

    // Call recover-stale-base.
    run_recover_stale_base(&conn, &er_s, "T2060", invoker_human()).unwrap();

    // Find the new pending ER.
    let new_er_id: String = conn
        .query_row(
            "SELECT display_id FROM external_reviews WHERE task_id='T2060' AND status='pending'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    // Run external review for the new pending ER → PASS.
    let sh2 = shim(tmp.path(), "#!/bin/sh\ncat >/dev/null\necho 'VERDICT: PASS'\n");
    let cfg2 = cfg(tmp.path(), &sh2);
    external_review::run(
        &json!({"display_id": new_er_id}),
        &DispatchCtx { conn: &conn, agents: &a, config_path: &cfg2, policies_hash: "" },
    )
    .unwrap();

    let final_status: String = conn
        .query_row(
            "SELECT status FROM external_reviews WHERE display_id=?1",
            [&new_er_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(final_status, "passed");
}

/// Test 7 (Task 1.26): superseded rows do NOT appear in watch filter.
#[test]
fn recover_stale_base_test7_superseded_not_in_watch_filter() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let er_s = er_schema();

    let ws = git_workspace();
    git(ws.path(), &["checkout", "-b", "task-t7-watch"]);
    std::fs::write(ws.path().join("t7.txt"), "t7\n").unwrap();
    git(ws.path(), &["add", "t7.txt"]);
    git(ws.path(), &["commit", "-m", "t7"]);
    let pre_head = head(ws.path());

    git(ws.path(), &["checkout", "main"]);
    std::fs::write(ws.path().join("m7.txt"), "m7\n").unwrap();
    git(ws.path(), &["add", "m7.txt"]);
    git(ws.path(), &["commit", "-m", "main7"]);

    insert_task_id(&conn, "T2070", ws.path(), "task-t7-watch", "T3", "in_review");
    insert_tooling_held_stale_base(&conn, "ER2070", "T2070", 1, &pre_head, "oldmain");
    insert_tooling_held_stale_base(&conn, "ER2071", "T2070", 2, &pre_head, "oldmain");

    git(ws.path(), &["checkout", "task-t7-watch"]);
    git(ws.path(), &["rebase", "main"]);

    run_recover_stale_base(&conn, &er_s, "T2070", invoker_human()).unwrap();

    // Watch filter: pending/running/tooling_held only.
    let watch_rows: Vec<String> = {
        let mut s = conn
            .prepare(
                "SELECT display_id FROM external_reviews \
                 WHERE status IN ('pending','running','tooling_held') AND task_id='T2070'",
            )
            .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert_eq!(
        watch_rows.len(),
        1,
        "only new pending ER must appear in watch filter, got: {:?}",
        watch_rows
    );
    assert_ne!(watch_rows[0], "ER2070", "superseded ER2070 must not appear");
    assert_ne!(watch_rows[0], "ER2071", "superseded ER2071 must not appear");

    // Superseded count must be 2 (held_reason intact for historical queries).
    let superseded_held: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM external_reviews WHERE status='tooling_held' AND task_id='T2070'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(superseded_held, 0, "no rows remain as tooling_held after recovery");
}

/// Test 8 (Task 1.27): pre-patch DB migration — run on a DB without superseded_by column.
#[test]
fn recover_stale_base_test8_pre_patch_db_migration() {
    let conn = Connection::open_in_memory().unwrap();
    // Install DB schema but CREATE external_reviews WITHOUT the superseded_by column
    // to simulate a pre-patch DB.
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    let tasks_yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "tasks")
        .map(|(_, y)| *y)
        .unwrap();
    conn.execute_batch(&ddl_for(&Schema::from_yaml(tasks_yaml).unwrap())).unwrap();

    // Create external_reviews WITHOUT superseded_by column.
    conn.execute_batch(
        "CREATE TABLE external_reviews ( \
            id INTEGER PRIMARY KEY AUTOINCREMENT, \
            display_id TEXT, \
            status TEXT, \
            task_id TEXT, \
            attempt INTEGER, \
            adapter TEXT, \
            base_sha TEXT, \
            head_sha TEXT, \
            verdict TEXT, \
            held_reason TEXT, \
            next_retry_at TEXT, \
            created_at TEXT, \
            updated_at TEXT, \
            created_by TEXT, \
            updated_by TEXT \
         )",
    )
    .unwrap();

    let er_s = er_schema();
    let ws = git_workspace();

    git(ws.path(), &["checkout", "-b", "task-t8-prepatch"]);
    std::fs::write(ws.path().join("t8.txt"), "t8\n").unwrap();
    git(ws.path(), &["add", "t8.txt"]);
    git(ws.path(), &["commit", "-m", "t8"]);
    let pre_head = head(ws.path());

    git(ws.path(), &["checkout", "main"]);
    std::fs::write(ws.path().join("m8.txt"), "m8\n").unwrap();
    git(ws.path(), &["add", "m8.txt"]);
    git(ws.path(), &["commit", "-m", "main8"]);

    insert_task_id(&conn, "T2080", ws.path(), "task-t8-prepatch", "T3", "in_review");

    // Insert held row directly (no superseded_by column yet).
    conn.execute(
        "INSERT INTO external_reviews \
         (display_id, status, task_id, attempt, adapter, head_sha, base_sha, verdict, held_reason, \
          created_at, updated_at, created_by, updated_by) \
         VALUES ('ER2080','tooling_held','T2080',1,'external_review',?1,'oldmain','TOOLING_FAILURE','stale_base_requires_rebase', \
                 '2026-05-01T00:00:00Z','2026-05-01T00:00:00Z','test','test')",
        rusqlite::params![pre_head],
    )
    .unwrap();

    git(ws.path(), &["checkout", "task-t8-prepatch"]);
    git(ws.path(), &["rebase", "main"]);

    // Run recover-stale-base — must succeed and add the column in-process.
    run_recover_stale_base(&conn, &er_s, "T2080", invoker_human()).unwrap();

    // Verify superseded_by column now exists via PRAGMA table_info.
    let cols: Vec<String> = {
        let mut s = conn.prepare("PRAGMA table_info('external_reviews')").unwrap();
        s.query_map([], |r| r.get(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert!(
        cols.contains(&"superseded_by".to_string()),
        "superseded_by column must exist after lazy ALTER: {:?}",
        cols
    );

    // Held row must now be superseded with superseded_by populated.
    let (status, superseded_by): (String, Option<String>) = conn
        .query_row(
            "SELECT status, superseded_by FROM external_reviews WHERE display_id='ER2080'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "superseded");
    assert!(
        superseded_by.is_some() && !superseded_by.as_deref().unwrap_or("").is_empty(),
        "superseded_by must be populated: {:?}",
        superseded_by
    );
}

/// Schema invariant test (Task 1.28 / AC1.4): external_reviews schema has superseded_by field
/// and tooling_held→superseded transition.
#[test]
fn recover_stale_base_schema_invariant_test() {
    use stores::schema::FieldType;

    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == "external_reviews")
        .map(|(_, y)| *y)
        .unwrap();
    let schema = Schema::from_yaml(yaml).unwrap();

    // Assert superseded_by field exists with correct type.
    let sb_field = schema.fields.iter().find(|f| f.name == "superseded_by");
    assert!(sb_field.is_some(), "schema must have superseded_by field");
    let sb = sb_field.unwrap();
    assert!(
        matches!(sb.ty, FieldType::Text),
        "superseded_by must be FieldType::Text, got: {:?}",
        sb.ty
    );
    assert!(!sb.required, "superseded_by must be not required");

    // Assert tooling_held→superseded transition exists.
    let has_edge = schema.lifecycle.transitions.iter().any(|t| {
        t.from == "tooling_held" && t.to == "superseded" && t.verb == "supersede"
    });
    assert!(
        has_edge,
        "schema must have tooling_held→superseded transition via supersede verb"
    );
}
