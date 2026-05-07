use std::fs;
use std::process::Command;

#[test]
fn runs_list_and_show_fixture_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let runs_dir = tmp.path().join(".stores/runs/T777");
    fs::create_dir_all(&runs_dir).unwrap();
    fs::copy(
        "tests/fixtures/runs/T777/executor.json",
        runs_dir.join("executor.json"),
    )
    .unwrap();
    fs::copy(
        "tests/fixtures/runs/T777/code-reviewer.json",
        runs_dir.join("code-reviewer.json"),
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_stores");
    let list = Command::new(bin)
        .current_dir(tmp.path())
        .args(["runs", "list", "T777"])
        .output()
        .expect("failed to invoke stores runs list");
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8(list.stdout).unwrap();
    assert!(stdout.contains("phase\tcycle\trole\ttranscript_path"));
    assert!(stdout.contains("1\t1\tcode-reviewer"));
    assert!(stdout.contains("1\t1\texecutor"));

    let show = Command::new(bin)
        .current_dir(tmp.path())
        .args(["runs", "show", "T777", "--phase", "1", "--role", "executor"])
        .output()
        .expect("failed to invoke stores runs show");
    assert!(
        show.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    let body = String::from_utf8(show.stdout).unwrap();
    assert!(body.contains("fixture executor"));
}
