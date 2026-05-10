use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use stores::flow::freshness::{check_freshness, FreshnessOutcome};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn repo() -> (tempfile::TempDir, PathBuf, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "base\n").unwrap();
    git(&repo, &["add", "src/lib.rs"]);
    git(&repo, &["commit", "-m", "base"]);
    let base = git(&repo, &["rev-parse", "main"]);
    std::fs::write(repo.join("src/lib.rs"), "main moved\n").unwrap();
    git(&repo, &["add", "src/lib.rs"]);
    git(&repo, &["commit", "-m", "main move"]);
    let head = git(&repo, &["rev-parse", "main"]);
    (tmp, repo, base, head)
}

fn row(repo: &Path, base: &str, scope: Value) -> Value {
    json!({
        "workspace_path": repo.to_str().unwrap(),
        "review_base_sha": base,
        "review_head_sha": "candidate-head",
        "test_base_sha": base,
        "test_head_sha": "candidate-head",
        "branch_head_sha": "candidate-head",
        "affected_scope": scope,
    })
}

#[test]
fn fresh_inputs_allow_merge() {
    let (_tmp, repo, _base, head) = repo();
    let row = row(&repo, &head, json!(["src/lib.rs"]));
    assert_eq!(
        check_freshness(&row, &head).unwrap(),
        FreshnessOutcome::Ready
    );
}

#[test]
fn missing_durable_inputs_forces_rerun() {
    let (_tmp, repo, base, head) = repo();
    let mut row = row(&repo, &base, json!(["src/lib.rs"]));
    row.as_object_mut().unwrap().remove("review_base_sha");
    assert!(matches!(
        check_freshness(&row, &head).unwrap(),
        FreshnessOutcome::StaleRequiresRereview(_)
    ));
}

#[test]
fn overlap_forces_rerun() {
    let (_tmp, repo, base, head) = repo();
    let row = row(&repo, &base, json!(["src/lib.rs"]));
    assert_eq!(
        check_freshness(&row, &head).unwrap(),
        FreshnessOutcome::StaleRequiresRereview(vec!["src/lib.rs".to_string()])
    );
}

#[test]
fn main_moved_without_overlap_allows_cheap_refresh_then_merge() {
    let (_tmp, repo, base, head) = repo();
    let row = row(&repo, &base, json!(["docs/readme.md"]));
    assert_eq!(
        check_freshness(&row, &head).unwrap(),
        FreshnessOutcome::StaleRequiresRefresh(vec!["docs/readme.md".to_string()])
    );
}

#[test]
fn missing_branch_head_sha_forces_refresh() {
    let (_tmp, repo, base, head) = repo();
    let mut row = row(&repo, &base, json!(["src/lib.rs"]));
    row.as_object_mut().unwrap().remove("branch_head_sha");
    assert!(matches!(
        check_freshness(&row, &head).unwrap(),
        FreshnessOutcome::StaleRequiresRefresh(_)
    ));
}
