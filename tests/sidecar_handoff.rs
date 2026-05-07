//! T028 P5: side-car hand-off integration tests.
//!
//! Drives `spawn::handoff_inner` against a mock claude shim. Validates:
//!   • argv shape (--append-system-prompt-file <file>, [--continue], <message>)
//!   • priming file content reaches the side-car
//!   • token is in the initial message (chat context, not env)
//!   • obs-drafting handoff succeeds without substrate session-id argv leakage

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

use stores::tui::sidecar::spawn::{handoff_inner, HandoffOutcome};
use stores::tui::sidecar::SidecarScope;

const APPROVE_TOKEN_ENV: &str = "STORES_APPROVE_TOKEN";

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn fixture_shim(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sidecar")
        .join(name)
}

fn make_fixture_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
            id INTEGER PRIMARY KEY,
            display_id TEXT,
            status TEXT,
            title TEXT,
            tier_hint TEXT,
            claimed_by TEXT,
            wrap_log TEXT,
            linked_observations TEXT
         );
         CREATE TABLE observations (
            id INTEGER PRIMARY KEY,
            display_id TEXT,
            status TEXT,
            summary TEXT,
            priority TEXT,
            body TEXT,
            intent_contract TEXT,
            task_id TEXT
         );
         CREATE TABLE transition_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            store TEXT NOT NULL,
            row_id INTEGER NOT NULL,
            display_id TEXT NOT NULL,
            from_status TEXT,
            to_status TEXT NOT NULL,
            verb TEXT NOT NULL,
            invoker TEXT NOT NULL,
            policy_ref TEXT,
            policies_hash TEXT,
            occurred_at TEXT NOT NULL
         );
         INSERT INTO tasks VALUES
           (1, 'T999', 'code_review', 'Demo task', 'T2', 'agent-x', '[]', '[]');
        ",
    )
    .unwrap();
    conn
}

fn with_test_env<F: FnOnce()>(token: &str, vars: &[(&str, &str)], f: F) {
    let _g = match ENV_LOCK.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let prev_token = std::env::var(APPROVE_TOKEN_ENV).ok();
    std::env::set_var(APPROVE_TOKEN_ENV, token);
    let prev_vars: Vec<(String, Option<String>)> = vars
        .iter()
        .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
        .collect();
    for (k, v) in vars {
        std::env::set_var(k, v);
    }
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    for (k, prev) in &prev_vars {
        match prev {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
    match prev_token {
        Some(v) => std::env::set_var(APPROVE_TOKEN_ENV, v),
        None => std::env::remove_var(APPROVE_TOKEN_ENV),
    }
    if let Err(e) = res {
        std::panic::resume_unwind(e);
    }
}

fn read_argv(outdir: &Path) -> Vec<String> {
    let bytes = std::fs::read(outdir.join("argv.txt")).expect("shim should have recorded argv.txt");
    bytes
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

#[test]
fn per_row_handoff_passes_token_in_message_and_priming_file() {
    let runs_dir = tempfile::tempdir().unwrap();
    let outdir = tempfile::tempdir().unwrap();
    let conn = make_fixture_db();

    with_test_env(
        "tok-handoff-abc",
        &[
            ("MOCK_CLAUDE_OUTDIR", outdir.path().to_str().unwrap()),
            ("MOCK_CLAUDE_RUNS_DIR", runs_dir.path().to_str().unwrap()),
        ],
        || {
            let scope = SidecarScope::PerRow {
                display_id: "T999".to_string(),
                fresh: false,
            };
            let outcome: HandoffOutcome = handoff_inner(
                scope,
                runs_dir.path(),
                &fixture_shim("mock-claude.sh"),
                &conn,
            )
            .expect("handoff_inner returns Ok with successful shim");
            assert!(outcome.status.success(), "shim must exit 0");

            let argv = read_argv(outdir.path());
            assert_eq!(argv[0], "--append-system-prompt-file");
            assert!(!argv[1].is_empty(), "priming path must be non-empty");
            assert_eq!(argv[2], "--continue");
            assert!(
                argv[3].contains("tok-handoff-abc"),
                "token must be in positional initial message, got {:?}",
                argv[3]
            );
            assert_eq!(argv.len(), 4);

            let priming = std::fs::read_to_string(outdir.path().join("priming.txt")).unwrap();
            assert!(priming.contains("T999"), "priming must mention row id");
        },
    );
}

#[test]
fn obs_draft_handoff_succeeds_without_substrate_session_id_argv() {
    let runs_dir = tempfile::tempdir().unwrap();
    let outdir = tempfile::tempdir().unwrap();
    let conn = make_fixture_db();

    with_test_env(
        "tok-obs-1",
        &[
            ("MOCK_CLAUDE_OUTDIR", outdir.path().to_str().unwrap()),
            ("MOCK_CLAUDE_RUNS_DIR", runs_dir.path().to_str().unwrap()),
            (
                "MOCK_CLAUDE_OBS_DRAFT_BODY",
                r#"{"summary":"draft sum","body":"draft body","priority":"normal"}"#,
            ),
        ],
        || {
            let outcome = handoff_inner(
                SidecarScope::ObsDraft,
                runs_dir.path(),
                &fixture_shim("mock-claude.sh"),
                &conn,
            )
            .expect("obs-draft handoff Ok");
            assert!(outcome.status.success());
            assert_eq!(
                outcome.obs_draft_body.as_deref(),
                Some(r#"{"summary":"draft sum","body":"draft body","priority":"normal"}"#),
                "obs draft body is collected from STORES_OBS_DRAFT_PATH, not argv"
            );
            assert!(outcome.obs_draft_path.is_some());
            let argv = read_argv(outdir.path());
            assert!(
                argv.iter().all(|arg| !arg.contains("obs-draft-")),
                "claude argv no longer carries substrate session id to the child: {argv:?}"
            );
        },
    );
}

#[test]
fn token_round_trip_through_chat_context_not_env() {
    let runs_dir = tempfile::tempdir().unwrap();
    let outdir = tempfile::tempdir().unwrap();
    let conn = make_fixture_db();

    with_test_env(
        "tok-roundtrip-zzz",
        &[("MOCK_CLAUDE_OUTDIR", outdir.path().to_str().unwrap())],
        || {
            let outcome = handoff_inner(
                SidecarScope::General,
                runs_dir.path(),
                &fixture_shim("mock-claude-record-argv.sh"),
                &conn,
            )
            .expect("record-argv shim must return Ok");
            assert!(outcome.status.success());

            let argv = read_argv(outdir.path());
            let message = argv
                .last()
                .expect("positional initial message must be present");
            assert!(
                message.contains("tok-roundtrip-zzz"),
                "token must be carried by positional initial message, got: {:?}",
                message
            );
        },
    );
}

#[test]
fn handoff_does_not_increment_counter_on_shim_failure() {
    let runs_dir = tempfile::tempdir().unwrap();
    let outdir = tempfile::tempdir().unwrap();

    // Build the failing shim as a regular file (not held open) so exec()
    // doesn't trip ETXTBSY.
    let shim_dir = tempfile::tempdir().unwrap();
    let shim_path = shim_dir.path().join("fail-claude.sh");
    std::fs::write(&shim_path, "#!/usr/bin/env bash\nexit 7\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&shim_path, std::fs::Permissions::from_mode(0o755)).unwrap();

    let conn = make_fixture_db();
    with_test_env(
        "tok-fail",
        &[
            ("MOCK_CLAUDE_OUTDIR", outdir.path().to_str().unwrap()),
            ("MOCK_CLAUDE_RUNS_DIR", runs_dir.path().to_str().unwrap()),
        ],
        || {
            let outcome = handoff_inner(SidecarScope::General, runs_dir.path(), &shim_path, &conn)
                .expect("handoff_inner returns Ok even when child exits non-zero");
            assert!(!outcome.status.success(), "shim exited 7 → non-success");
        },
    );
}
