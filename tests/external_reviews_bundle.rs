use rusqlite::Connection;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use stores::flow::config::{CodexCfg, ReviewCfg};
use stores::handlers::external_reviews::{
    load_review_input_bundle, mark_attempt_tooling_failure_ready, parse_codex_review_output,
    render_codex_prompt, run_external_review_attempt, tooling_failure_ready_json,
    ExternalReviewVerdict,
};
use tempfile::TempDir;

fn init_db(conn: &Connection, workspace: &str) {
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
            task_id TEXT,
            attempt INTEGER,
            verdict TEXT,
            critical_count INTEGER,
            major_count INTEGER,
            minor_count INTEGER,
            findings TEXT
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (display_id, contract, plan, cycles, wrap_log, workspace_path, branch)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            "T083",
            r#"{"done_when":"done when contract is reviewed","scope_in":"in","scope_out":"out"}"#,
            r#"{"objective":"obj","phases":[{"name":"Phase Alpha"},{"name":"Phase Beta"}]}"#,
            r#"[{"phase":1,"cycle":1,"review":{"gate":"PASS"}}]"#,
            r#"[{"executive_summary":"old summary"},{"executive_summary":"latest wrap summary"}]"#,
            workspace,
            "feature/external-review",
        ),
    )
    .unwrap();
    conn.execute(
        "INSERT INTO external_reviews VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        (
            "ER001",
            "T083",
            1_i64,
            "REVISE",
            0_i64,
            1_i64,
            2_i64,
            r#"[{"severity":"major","summary":"prior finding"}]"#,
        ),
    )
    .unwrap();
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[cfg(unix)]
fn codex_shim(script: &str) -> (TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("codex-shim.sh");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
}

fn temp_git_repo() -> (TempDir, String, String) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.email", "stores@example.test"]);
    git(dir.path(), &["config", "user.name", "Stores Test"]);
    std::fs::write(dir.path().join("review.txt"), "base\n").unwrap();
    git(dir.path(), &["add", "review.txt"]);
    git(dir.path(), &["commit", "-m", "base"]);
    let base = git(dir.path(), &["rev-parse", "HEAD"]);
    std::fs::write(dir.path().join("review.txt"), "base\nhead change\n").unwrap();
    git(dir.path(), &["commit", "-am", "head"]);
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    (dir, base, head)
}

#[test]
fn external_review_bundle_prompt_contains_canonical_inputs() {
    let (repo, base, head) = temp_git_repo();
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn, repo.path().to_str().unwrap());

    let bundle = load_review_input_bundle(&conn, "T083", None, Some(&base), Some(&head)).unwrap();
    let prompt = render_codex_prompt(&bundle);

    assert!(prompt.contains("done when contract is reviewed"));
    assert!(prompt.contains("Phase Alpha"));
    assert!(prompt.contains("Phase Beta"));
    assert!(prompt.contains("latest wrap summary"));
    assert!(prompt.contains("verdict=REVISE"));
    assert!(prompt.contains(&base));
    assert!(prompt.contains(&head));
    assert!(prompt.contains("+head change"));
}

#[test]
fn missing_git_base_returns_tooling_failure_ready_error() {
    let (repo, _base, head) = temp_git_repo();
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn, repo.path().to_str().unwrap());

    let err = load_review_input_bundle(&conn, "T083", None, Some("missing-base"), Some(&head))
        .unwrap_err();
    assert_eq!(err.verdict, ExternalReviewVerdict::ToolingFailure);
    assert!(err.message.contains("TOOLING_FAILURE"));
    let ready = tooling_failure_ready_json(&err);
    assert_eq!(ready["verdict"], "TOOLING_FAILURE");
    assert_ne!(ready["verdict"], "PASS");
    assert_ne!(ready["verdict"], "REVISE");

    mark_attempt_tooling_failure_ready(&conn, "ER001", &err).unwrap();
    let (verdict, findings): (String, String) = conn
        .query_row(
            "SELECT verdict, findings FROM external_reviews WHERE display_id='ER001'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(verdict, "TOOLING_FAILURE");
    assert!(findings.contains("TOOLING_FAILURE"));
}

#[test]
fn codex_output_parser_maps_verdicts_and_finding_counts() {
    let pass = parse_codex_review_output("VERDICT: PASS\n").unwrap();
    assert_eq!(pass.verdict, ExternalReviewVerdict::Pass);
    assert_eq!(pass.counts.critical, 0);

    let revise = parse_codex_review_output(
        "VERDICT: REVISE\n[critical] c\n[major] m1\nmajor: m2\n[minor] n\n",
    )
    .unwrap();
    assert_eq!(revise.verdict, ExternalReviewVerdict::Revise);
    assert_eq!(revise.counts.critical, 1);
    assert_eq!(revise.counts.major, 2);
    assert_eq!(revise.counts.minor, 1);

    let tooling =
        parse_codex_review_output("VERDICT=TOOLING_FAILURE\n[minor] diagnostic\n").unwrap();
    assert_eq!(tooling.verdict, ExternalReviewVerdict::ToolingFailure);
    assert_eq!(tooling.counts.minor, 1);
}

#[test]
#[cfg(unix)]
fn codex_shim_invocation_persists_runner_metadata() {
    let (repo, base, head) = temp_git_repo();
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
            task_id TEXT,
            attempt INTEGER,
            adapter TEXT,
            runner TEXT,
            model_id TEXT,
            model_metadata TEXT,
            base_sha TEXT,
            head_sha TEXT,
            verdict TEXT,
            critical_count INTEGER,
            major_count INTEGER,
            minor_count INTEGER,
            started_at TEXT,
            completed_at TEXT,
            duration_ms INTEGER,
            log_path TEXT,
            transcript_path TEXT,
            contract_ref TEXT,
            plan_ref TEXT,
            wrap_log_ref TEXT,
            diff_ref TEXT,
            prior_review_ref TEXT,
            findings TEXT
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tasks (display_id, contract, plan, cycles, wrap_log, workspace_path, branch)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            "T083",
            r#"{"done_when":"review me"}"#,
            r#"{"phases":[{"name":"Phase"}]}"#,
            "[]",
            r#"[{"executive_summary":"wrapped"}]"#,
            repo.path().to_str().unwrap(),
            "feature/external-review",
        ),
    )
    .unwrap();
    conn.execute(
        "INSERT INTO external_reviews (display_id, task_id, attempt) VALUES ('ER900', 'T083', 1)",
        [],
    )
    .unwrap();

    let runs = tempfile::tempdir().unwrap();
    let old_runs = std::env::var_os("STORES_RUNS_DIR");
    std::env::set_var("STORES_RUNS_DIR", runs.path());
    let (_shim_dir, shim) = codex_shim(
        "#!/bin/sh\nstdin=$(cat)\nprintf '%s' \"$stdin\" | grep -q 'Review task T083' || exit 9\necho 'VERDICT: PASS'\necho '[minor] note'\n",
    );
    let review = ReviewCfg {
        runner: "codex".to_string(),
        model: Some("shim-model".to_string()),
        max_parallel: 1,
        timeout_secs: 10,
    };
    let codex = CodexCfg {
        command: shim.to_string_lossy().to_string(),
        args: Vec::new(),
    };
    let parsed = run_external_review_attempt(
        &conn,
        "ER900",
        "T083",
        &review,
        &codex,
        None,
        Some(&base),
        Some(&head),
    )
    .unwrap();
    assert_eq!(parsed.verdict, ExternalReviewVerdict::Pass);

    let (runner, model_id, verdict, transcript_path, log_path, metadata): (
        String,
        String,
        String,
        String,
        String,
        String,
    ) = conn
        .query_row(
            "SELECT runner, model_id, verdict, transcript_path, log_path, model_metadata FROM external_reviews WHERE display_id='ER900'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();
    assert_eq!(runner, "codex");
    assert_eq!(model_id, "shim-model");
    assert_eq!(verdict, "PASS");
    assert!(std::path::Path::new(&transcript_path).exists());
    assert!(std::path::Path::new(&log_path).exists());
    assert_ne!(log_path, transcript_path);
    assert!(transcript_path.starts_with(runs.path().to_str().unwrap()));
    assert!(log_path.starts_with(runs.path().to_str().unwrap()));
    assert!(transcript_path.ends_with(".codex.transcript.log"));
    assert!(log_path.ends_with(".codex.stderr.log"));
    assert!(metadata.contains("session_id"));
    assert!(metadata.contains("stderr_log_path"));

    match old_runs {
        Some(v) => std::env::set_var("STORES_RUNS_DIR", v),
        None => std::env::remove_var("STORES_RUNS_DIR"),
    }
}
