use rusqlite::{params, Connection};
use std::fs;
use std::process::Command;

#[test]
fn runs_list_and_show_cycle_backlinked_jsonl_transcripts() {
    let tmp = tempfile::tempdir().unwrap();
    let stores = tmp.path().join(".stores");
    fs::create_dir_all(stores.join("runs")).unwrap();
    fs::write(
        stores.join("runs/session-executor.jsonl"),
        r#"{"role":"executor","summary":"fixture executor jsonl"}"#,
    )
    .unwrap();
    fs::write(
        stores.join("runs/session-review.jsonl"),
        r#"{"role":"code-reviewer","gate":"PASS"}"#,
    )
    .unwrap();

    let conn = Connection::open(stores.join("db.sqlite")).unwrap();
    conn.execute(
        "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
        [],
    )
    .unwrap();
    let cycles = serde_json::json!([{
        "phase": 1,
        "cycle": 1,
        "executor": {"transcript_path": ".stores/runs/session-executor.jsonl"},
        "review": {"transcript_path": ".stores/runs/session-review.jsonl"}
    }]);
    conn.execute(
        "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
        params!["T777", serde_json::to_string(&cycles).unwrap()],
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
    // Verify deterministic order: ORDER BY phase, cycle, role (from the VIEW query).
    // Exact line comparison catches regressions in ordering that `contains` would miss.
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec![
            "phase\tcycle\trole\ttranscript_path",
            "1\t1\tcode-reviewer\t.stores/runs/session-review.jsonl",
            "1\t1\texecutor\t.stores/runs/session-executor.jsonl",
        ],
        "runs list output must be deterministically ordered by phase, cycle, role"
    );

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
    assert!(body.contains("fixture executor jsonl"));
}

#[test]
fn runs_show_succeeds_when_sibling_transcript_missing() {
    // Regression guard for the MAJOR r3 finding: runs show must query only the
    // selected (display_id, phase, role) row, not validate every transcript in
    // the task.  If the review transcript is missing but we ask for executor,
    // the command must succeed.
    let tmp = tempfile::tempdir().unwrap();
    let stores = tmp.path().join(".stores");
    fs::create_dir_all(stores.join("runs")).unwrap();
    // Only the executor transcript exists on disk — review transcript is absent.
    fs::write(
        stores.join("runs/executor-only.jsonl"),
        r#"{"role":"executor","summary":"executor only"}"#,
    )
    .unwrap();

    let conn = Connection::open(stores.join("db.sqlite")).unwrap();
    conn.execute(
        "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
        [],
    )
    .unwrap();
    let cycles = serde_json::json!([{
        "phase": 1,
        "cycle": 1,
        "executor": {"transcript_path": ".stores/runs/executor-only.jsonl"},
        "review":   {"transcript_path": ".stores/runs/review-absent.jsonl"}
    }]);
    conn.execute(
        "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
        params!["T779", serde_json::to_string(&cycles).unwrap()],
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_stores");
    let show = Command::new(bin)
        .current_dir(tmp.path())
        .args(["runs", "show", "T779", "--phase", "1", "--role", "executor"])
        .output()
        .expect("failed to invoke stores runs show");
    assert!(
        show.status.success(),
        "runs show executor must succeed even when sibling review transcript is missing\nstderr: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    let body = String::from_utf8(show.stdout).unwrap();
    assert!(body.contains("executor only"));
}

#[test]
fn runs_show_missing_linked_transcript_errors_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let stores = tmp.path().join(".stores");
    fs::create_dir_all(stores.join("runs")).unwrap();
    let conn = Connection::open(stores.join("db.sqlite")).unwrap();
    conn.execute(
        "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
        [],
    )
    .unwrap();
    let cycles = serde_json::json!([{
        "phase": 1,
        "cycle": 1,
        "executor": {"transcript_path": ".stores/runs/missing-session.jsonl"}
    }]);
    conn.execute(
        "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
        params!["T778", serde_json::to_string(&cycles).unwrap()],
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_stores");
    let show = Command::new(bin)
        .current_dir(tmp.path())
        .args(["runs", "show", "T778", "--phase", "1", "--role", "executor"])
        .output()
        .expect("failed to invoke stores runs show");
    assert!(!show.status.success());
    let stderr = String::from_utf8_lossy(&show.stderr);
    assert!(stderr.contains("missing transcript for T778 phase 1 cycle 1 role executor"));
}
