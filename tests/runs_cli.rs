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
    assert!(stdout.contains("phase\tcycle\trole\ttranscript_path"));
    assert!(stdout.contains("1\t1\tcode-reviewer\t.stores/runs/session-review.jsonl"));
    assert!(stdout.contains("1\t1\texecutor\t.stores/runs/session-executor.jsonl"));

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
