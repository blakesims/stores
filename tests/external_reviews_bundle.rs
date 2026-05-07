use rusqlite::Connection;
use stores::handlers::external_reviews::{
    load_review_input_bundle, mark_attempt_tooling_failure_ready, parse_codex_review_output,
    render_codex_prompt, tooling_failure_ready_json, ExternalReviewVerdict,
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
