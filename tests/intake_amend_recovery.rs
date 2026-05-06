//! T053 Phase 3: reject_noise amend recovery actor enforcement.

use rusqlite::Connection;
use std::process::Command;

fn stores_cmd(dir: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stores"));
    cmd.current_dir(dir);
    cmd.env("CLAUDECODE", "1");
    cmd
}

fn run_ok(dir: &std::path::Path, args: &[&str]) {
    let out = stores_cmd(dir).args(args).output().expect("run stores");
    assert!(
        out.status.success(),
        "stores {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run_err(dir: &std::path::Path, args: &[&str]) -> String {
    let out = stores_cmd(dir).args(args).output().expect("run stores");
    assert!(
        !out.status.success(),
        "stores {:?} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn scalar(dir: &std::path::Path, sql: &str) -> String {
    let conn = Connection::open(dir.join(".stores/db.sqlite")).expect("open db");
    conn.query_row(sql, [], |r| r.get::<_, String>(0))
        .expect("query scalar")
}

#[test]
fn reject_noise_amend_requires_ai_with_human_and_recovers_to_triaging() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

    run_ok(tmp.path(), &["init"]);
    run_ok(
        tmp.path(),
        &["install", root.join("stores/intake_items").to_str().unwrap()],
    );
    run_ok(
        tmp.path(),
        &["install", root.join("stores/observations").to_str().unwrap()],
    );

    run_ok(
        tmp.path(),
        &[
            "intake",
            "add",
            "--invoker",
            "ai_autonomous",
            "--summary",
            "noise report",
            "--source-agent",
            "executor",
            "--captured-at",
            "2026-05-06T10:00:00Z",
            "--captured-week",
            "w19-d2",
        ],
    );
    let id = scalar(
        tmp.path(),
        "SELECT display_id FROM intake ORDER BY id DESC LIMIT 1",
    );

    run_ok(
        tmp.path(),
        &["intake", "claim-triage", &id, "--invoker", "ai_autonomous"],
    );
    run_ok(
        tmp.path(),
        &[
            "intake",
            "route",
            &id,
            "--invoker",
            "ai_autonomous",
            "--decision",
            "reject_noise",
            "--gatekeeper-decision-json",
            r#"{"decision":"reject_noise","confidence":"high","rationale":"noise"}"#,
        ],
    );
    assert_eq!(
        scalar(
            tmp.path(),
            &format!("SELECT status FROM intake WHERE display_id='{id}'"),
        ),
        "dropped"
    );

    let err = run_err(
        tmp.path(),
        &["intake", "amend", &id, "--invoker", "ai_autonomous"],
    );
    assert!(
        err.contains("ai_with_human") || err.contains("ai_autonomous") || err.contains("actor"),
        "expected actor error, got: {err}"
    );
    assert_eq!(
        scalar(
            tmp.path(),
            &format!("SELECT status FROM intake WHERE display_id='{id}'"),
        ),
        "dropped",
        "failed autonomous amend must leave dropped row unchanged"
    );

    run_ok(
        tmp.path(),
        &["intake", "amend", &id, "--invoker", "ai_with_human"],
    );
    assert_eq!(
        scalar(
            tmp.path(),
            &format!("SELECT status FROM intake WHERE display_id='{id}'"),
        ),
        "triaging"
    );
}
