use std::process::Command;

fn stores_bin() -> &'static str {
    env!("CARGO_BIN_EXE_stores")
}

fn repo_root() -> &'static str {
    env!("CARGO_MANIFEST_DIR")
}

#[test]
fn tasks_cleanup_worktrees_help_lists_safe_modes() {
    let output = Command::new(stores_bin())
        .current_dir(repo_root())
        .args(["tasks", "cleanup-worktrees", "--help"])
        .output()
        .expect("invoke stores tasks cleanup-worktrees --help");

    assert!(
        output.status.success(),
        "cleanup-worktrees --help failed: status={:?}\nstderr={} ",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("help output must be UTF-8");
    for expected in [
        "--dry-run",
        "--execute",
        "--targets-only",
        "--remove-clean",
        "without deleting anything",
        "without --force",
    ] {
        assert!(
            stdout.contains(expected),
            "expected `{expected}` in cleanup-worktrees help; got:\n{stdout}"
        );
    }
}

#[test]
fn runs_gc_help_lists_caps_and_execute_guardrails() {
    let output = Command::new(stores_bin())
        .current_dir(repo_root())
        .args(["runs", "gc", "--help"])
        .output()
        .expect("invoke stores runs gc --help");

    assert!(
        output.status.success(),
        "runs gc --help failed: status={:?}\nstderr={} ",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("help output must be UTF-8");
    for expected in [
        "--dry-run",
        "--execute",
        "--max-bytes",
        "--warn-bytes",
        "--per-file-warn-bytes",
        "--largest",
        "default 20G",
        "default 10G",
        "default 1G",
    ] {
        assert!(
            stdout.contains(expected),
            "expected `{expected}` in runs gc help; got:\n{stdout}"
        );
    }
}
