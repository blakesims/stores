use anyhow::{anyhow, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use stores::codegen::ddl::SUBSTRATE_DDL;
use stores::flow::agents_yaml::TransitionEdge;
use stores::flow::{AgentEntry, AgentsYaml, BackoffKind, PoliciesYaml, RetryPolicy, Subscription};
use stores::handlers::agents_run::{
    poll_once_with_guard, BinaryIdentity, BinaryIdentityProvider, DaemonExeGuard, DaemonExeStatus,
    STALE_DAEMON_MESSAGE, STALE_HALTED,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone)]
struct MockIdentityProvider {
    identities: HashMap<PathBuf, BinaryIdentity>,
}

impl BinaryIdentityProvider for MockIdentityProvider {
    fn identity(&self, path: &Path) -> Result<BinaryIdentity> {
        self.identities
            .get(path)
            .copied()
            .ok_or_else(|| anyhow!("missing mock identity for {}", path.display()))
    }
}

fn mock_guard(
    startup: BinaryIdentity,
    launch_path: PathBuf,
    current: BinaryIdentity,
) -> DaemonExeGuard<MockIdentityProvider> {
    let mut identities = HashMap::new();
    identities.insert(launch_path.clone(), current);
    DaemonExeGuard::new(startup, launch_path, MockIdentityProvider { identities })
}

fn conn() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(SUBSTRATE_DDL).unwrap();
    c.execute_batch(
        "CREATE TABLE tasks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            display_id TEXT UNIQUE NOT NULL,
            status TEXT NOT NULL,
            tier_hint TEXT,
            branch TEXT,
            workspace_path TEXT,
            drive_pid INTEGER,
            drive_started_at TEXT,
            updated_at TEXT
        );",
    )
    .unwrap();
    c
}

fn policies() -> PoliciesYaml {
    PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    }
}

fn auto_drive_agent() -> AgentEntry {
    AgentEntry {
        name: "auto-drive".to_string(),
        subscribes_to: vec![Subscription {
            store: "tasks".to_string(),
            transition: TransitionEdge {
                from: "".to_string(),
                to: "planning".to_string(),
            },
            predicate: Some(stores::flow::predicate::PredicateExpr::Neq {
                left: serde_json::json!("$workspace_path"),
                right: serde_json::json!(""),
            }),
        }],
        command: "builtin:auto-drive".to_string(),
        claim_window_secs: 300,
        retry_policy: RetryPolicy {
            max_attempts: 3,
            backoff: BackoffKind::Linear,
        },
        command_args: None,
    }
}

fn init_empty_stores_dir(root: &Path) {
    let stores_dir = root.join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();
    std::fs::write(stores_dir.join("manifest.yaml"), "stores: []\n").unwrap();
    let db = Connection::open(stores_dir.join("db.sqlite")).unwrap();
    db.execute_batch(SUBSTRATE_DDL).unwrap();
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum StubHelpBehavior {
    Valid,
    Empty,
    MissingMarker,
    Timeout,
}

#[cfg(unix)]
fn write_reexec_stub(path: &Path, args_file: &Path, exit_code: i32, help: StubHelpBehavior) {
    use std::os::unix::fs::PermissionsExt;
    let help_case = match help {
        StubHelpBehavior::Valid => "echo 'stores - Schema-driven store framework'; exit 0",
        StubHelpBehavior::Empty => "exit 0",
        StubHelpBehavior::MissingMarker => "echo 'not the expected binary'; exit 0",
        StubHelpBehavior::Timeout => "sleep 3; exit 0",
    };
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then {help_case}; fi\nprintf '%s\\n' \"$0\" \"$@\" > {}\nexit {}\n",
        args_file.display(),
        exit_code
    );
    std::fs::write(path, script).unwrap();
    let mut perms = std::fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).unwrap();
}

#[cfg(unix)]
fn assert_validation_failure_context(stderr: &str, launch_path: &Path, reason: &str) {
    let size = std::fs::metadata(launch_path).unwrap().len().to_string();
    assert!(
        stderr.contains("candidate stores binary failed validation"),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&launch_path.display().to_string()),
        "stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("size={size}")),
        "stderr:\n{stderr}"
    );
    assert!(stderr.contains("--help"), "stderr:\n{stderr}");
    assert!(stderr.contains(reason), "stderr:\n{stderr}");
    // L182 codex-revise: exit_status must be present in the diagnostic line.
    assert!(
        stderr.contains("exit_status="),
        "exit_status field missing from diagnostic; stderr:\n{stderr}"
    );
}

fn insert_candidate(conn: &Connection, id: i64, display_id: &str, workspace_path: &Path) {
    conn.execute(
        "INSERT INTO tasks (id, display_id, status, tier_hint, branch, workspace_path)
         VALUES (?1, ?2, 'planning', 'T2', 'feat/stale', ?3)",
        rusqlite::params![id, display_id, workspace_path.to_string_lossy()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO transition_history
         (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at)
         VALUES ('tasks', ?1, ?2, '', 'planning', 'submit', 'ai_autonomous', '2026-05-07T00:00:00Z')",
        rusqlite::params![id, display_id],
    )
    .unwrap();
}

#[test]
fn daemon_exe_identity_mock_detects_changed_launch_path_identity() {
    let guard = mock_guard(
        BinaryIdentity { dev: 10, ino: 20 },
        PathBuf::from("/usr/local/bin/stores"),
        BinaryIdentity { dev: 10, ino: 21 },
    );
    assert_eq!(
        guard.current_status().unwrap(),
        DaemonExeStatus::Stale {
            message: STALE_DAEMON_MESSAGE
        }
    );
}

#[test]
fn stale_auto_drive_leaves_drive_pid_null_and_no_claim() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Reset process-local dedup flag so this test runs cleanly regardless of
    // order.
    STALE_HALTED.store(false, Ordering::SeqCst);
    std::env::remove_var("STORES_DRIVE_CMD");
    let c = conn();
    let tmp = tempfile::tempdir().unwrap();
    insert_candidate(&c, 149, "T149", tmp.path());
    let agents = AgentsYaml {
        agents: vec![auto_drive_agent()],
        deployment_specialist: None,
    };
    let guard = mock_guard(
        BinaryIdentity { dev: 1, ino: 1 },
        PathBuf::from("/usr/local/bin/stores"),
        BinaryIdentity { dev: 1, ino: 2 },
    );

    // With the MAJOR 1 fix, stale detection now returns Err from
    // poll_once_with_guard (bail-propagation path) rather than Ok(0), so that
    // the outer run_daemon loop exits rather than continuing to poll a stale
    // binary. The stale message is emitted exactly once via STALE_HALTED.
    let result = poll_once_with_guard(
        &c,
        &agents,
        &policies(),
        &tmp.path().join("config.yaml"),
        "test",
        "epoch",
        Some(&guard),
    );
    assert!(
        result.is_err(),
        "stale binary must cause poll_once_with_guard to return Err"
    );
    assert_eq!(
        result.unwrap_err().to_string(),
        STALE_DAEMON_MESSAGE,
        "error message must be the canonical stale message"
    );
    // No claim or spawn should have occurred.
    let (claims, drive_pid): (i64, Option<i64>) = c.query_row(
        "SELECT (SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='auto-drive'), drive_pid FROM tasks WHERE display_id='T149'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(
        claims, 0,
        "no dispatch_locks entry must be created on stale"
    );
    assert!(drive_pid.is_none(), "drive_pid must remain NULL on stale");
    assert_eq!(guard.check_stale().unwrap(), Some(STALE_DAEMON_MESSAGE));
}

#[cfg(unix)]
#[test]
fn stale_then_reexec_happy_path_records_preserved_argv() {
    use std::os::unix::process::CommandExt;

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let launch_path = tmp.path().join("stores_launch_stub");
    let args_file = tmp.path().join("argv.txt");
    write_reexec_stub(&launch_path, &args_file, 0, StubHelpBehavior::Valid);

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&launch_path)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_TEST_DAEMON_FORCE_STALE", "1")
        .env("STORES_DAEMON_BIN_PATH", &launch_path)
        .output()
        .expect("invoke stale daemon command");

    assert!(output.status.success(), "reexec stub should exit 0");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let attempt_lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("daemon binary stale; reexecing into"))
        .collect();
    assert_eq!(attempt_lines.len(), 1, "stderr:\n{stderr}");
    assert!(
        attempt_lines[0].contains(&format!(
            "daemon binary stale; reexecing into {} (was version {})",
            launch_path.display(),
            env!("CARGO_PKG_VERSION")
        )),
        "line: {}",
        attempt_lines[0]
    );
    let argv = std::fs::read_to_string(&args_file).unwrap();
    assert!(argv.contains("agents\nrun\n"), "argv:\n{argv}");
    assert!(argv.contains("--once\n"), "argv:\n{argv}");
    assert!(argv.contains("--poll-interval\n0.05\n"), "argv:\n{argv}");
}

#[cfg(unix)]
#[test]
fn stale_then_reexec_fails_fallback_exits_nonzero() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let launch_path = tmp.path().join("stores_not_executable");
    std::fs::write(&launch_path, "not executable\n").unwrap();
    let mut perms = std::fs::metadata(&launch_path).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&launch_path, perms).unwrap();

    // STORES_DAEMON_BIN_PATH points at the non-executable stub, which is also
    // used as arg0. With validate-on-shortcut (T076 codex-revise), the daemon
    // fails at startup when the existing private binary fails validation —
    // before stale detection can run. The observable contract is identical:
    // non-zero exit + spawn-error context in stderr.
    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&launch_path)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_TEST_DAEMON_FORCE_STALE", "1")
        .env("STORES_DAEMON_BIN_PATH", &launch_path)
        .output()
        .expect("invoke stale daemon command");

    assert!(
        !output.status.success(),
        "validation fallback must exit nonzero"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_validation_failure_context(&stderr, &launch_path, "spawn error");
}

/// LOW (T075 codex r3 follow-up): missing launch-path candidate is rejected fail-loud
/// at daemon startup-identity-guard construction, before the daemon can run any
/// subscriptions or attempt a reexec. The reviewer-runner suggested arg0->missing
/// would surface via the spawn-error mapping at the validation step; in practice
/// the identity-guard fails earlier (stat() of the missing launch path), which is
/// a stronger fail-loud (no work performed, no exec attempted). This test pins
/// that behavior so a future refactor that defers the stat (and would let the
/// daemon partially run before discovering the missing path) trips the test.
#[cfg(unix)]
#[test]
fn stale_reexec_missing_launch_path_rejected_at_startup() {
    use std::os::unix::process::CommandExt;

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let launch_path = tmp.path().join("nonexistent");

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&launch_path)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env_remove("STORES_TEST_DAEMON_FORCE_STALE")
        .env(
            "STORES_DAEMON_BIN_PATH",
            tmp.path().join("private/bin/stores"),
        )
        .output()
        .expect("invoke stale daemon command");

    assert!(
        output.status.success(),
        "private first-run migration should seed from current_exe and reexec; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// MEDIUM fix (Pi Option A): --detach must be stripped from reexec argv so the
/// daemon does not attempt to re-daemonize on self-reexec. --invoker and
/// --log-file must be preserved.
#[cfg(unix)]
#[test]
fn reexec_argv_strips_detach_preserves_invoker_and_log_file() {
    use std::os::unix::process::CommandExt;
    use std::time::{Duration, Instant};

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let launch_path = tmp.path().join("stores_launch_stub_detach");
    let args_file = tmp.path().join("argv-detach.txt");
    let log_file = tmp.path().join("daemon.log");
    write_reexec_stub(&launch_path, &args_file, 0, StubHelpBehavior::Valid);

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&launch_path)
        .args([
            "--invoker",
            "human",
            "agents",
            "run",
            "--detach",
            "--log-file",
            log_file.to_str().unwrap(),
            "--poll-interval",
            "0.05",
        ])
        .current_dir(tmp.path())
        .env("STORES_TEST_DAEMON_FORCE_STALE", "1")
        .env("STORES_DAEMON_BIN_PATH", &launch_path)
        .output()
        .expect("invoke detached stale daemon command");

    assert!(output.status.success(), "detach parent should exit 0");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !args_file.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let argv = std::fs::read_to_string(&args_file).unwrap();
    // --invoker and --log-file must survive the reexec.
    assert!(argv.contains("--invoker\nhuman\n"), "argv:\n{argv}");
    assert!(
        argv.contains(&format!("--log-file\n{}\n", log_file.display())),
        "argv:\n{argv}"
    );
    // --detach must be stripped so the reexec-ed daemon does not re-daemonize.
    assert!(
        !argv.contains("--detach\n"),
        "--detach must be stripped from reexec argv:\n{argv}"
    );
}

#[cfg(unix)]
#[test]
fn stale_reexec_stub_missing_marker_candidate_rejected_without_exec() {
    use std::os::unix::process::CommandExt;

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let launch_path = tmp.path().join("stores_missing_marker_stub");
    let args_file = tmp.path().join("argv-missing-marker.txt");
    write_reexec_stub(&launch_path, &args_file, 0, StubHelpBehavior::MissingMarker);

    // STORES_DAEMON_BIN_PATH points at the missing-marker stub, which is also
    // arg0. With validate-on-shortcut (T076 codex-revise), the daemon fails at
    // startup when the existing private binary fails the marker check — before
    // stale detection runs. Observable contract: non-zero exit, path in stderr,
    // marker reason in stderr, candidate NOT exec'd.
    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&launch_path)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_TEST_DAEMON_FORCE_STALE", "1")
        .env("STORES_DAEMON_BIN_PATH", &launch_path)
        .output()
        .expect("invoke stale daemon command");

    assert!(
        !output.status.success(),
        "missing-marker candidate must fail"
    );
    assert!(
        !args_file.exists(),
        "candidate agents-run path must not be exec'd"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_validation_failure_context(&stderr, &launch_path, "missing stores marker");
}

#[cfg(unix)]
#[test]
fn stale_reexec_empty_output_candidate_rejected_without_exec() {
    use std::os::unix::process::CommandExt;

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let launch_path = tmp.path().join("stores_empty_help_stub");
    let args_file = tmp.path().join("argv-empty.txt");
    write_reexec_stub(&launch_path, &args_file, 0, StubHelpBehavior::Empty);

    // STORES_DAEMON_BIN_PATH points at the empty-help stub, which is also
    // arg0. With validate-on-shortcut (T076 codex-revise), the daemon fails at
    // startup when the existing private binary fails the empty-stdout check —
    // before stale detection runs. Observable contract: non-zero exit, path in
    // stderr, empty-stdout reason, candidate NOT exec'd.
    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&launch_path)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_TEST_DAEMON_FORCE_STALE", "1")
        .env("STORES_DAEMON_BIN_PATH", &launch_path)
        .output()
        .expect("invoke stale daemon command");

    assert!(!output.status.success(), "empty --help candidate must fail");
    assert!(
        !args_file.exists(),
        "candidate agents-run path must not be exec'd"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_validation_failure_context(&stderr, &launch_path, "empty stdout");
}

#[cfg(unix)]
#[test]
fn stale_reexec_timeout_candidate_rejected_without_exec() {
    use std::os::unix::process::CommandExt;
    use std::time::{Duration, Instant};

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let launch_path = tmp.path().join("stores_timeout_help_stub");
    let args_file = tmp.path().join("argv-timeout.txt");
    write_reexec_stub(&launch_path, &args_file, 0, StubHelpBehavior::Timeout);

    // STORES_DAEMON_BIN_PATH points at the timeout stub, which is also arg0.
    // With validate-on-shortcut (T076 codex-revise), the daemon fails at
    // startup when the existing private binary times out — before stale
    // detection runs. Observable contract: non-zero exit within the timeout
    // deadline, path in stderr, timeout reason, candidate NOT exec'd.
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&launch_path)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_TEST_DAEMON_FORCE_STALE", "1")
        .env("STORES_DAEMON_BIN_PATH", &launch_path)
        .output()
        .expect("invoke stale daemon command");
    let elapsed = started.elapsed();

    assert!(!output.status.success(), "hung --help candidate must fail");
    assert!(
        elapsed < Duration::from_millis(2250),
        "timeout validation must be deadline-bounded; elapsed={elapsed:?}"
    );
    assert!(
        !args_file.exists(),
        "candidate agents-run path must not be exec'd"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_validation_failure_context(&stderr, &launch_path, "timeout");
}

#[cfg(unix)]
#[test]
fn stale_reexec_fresh_binary_passes_and_records_normalized_argv() {
    use std::os::unix::process::CommandExt;

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let launch_path = tmp.path().join("stores_valid_help_stub");
    let args_file = tmp.path().join("argv-valid.txt");
    write_reexec_stub(&launch_path, &args_file, 0, StubHelpBehavior::Valid);

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&launch_path)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_TEST_DAEMON_FORCE_STALE", "1")
        .env("STORES_DAEMON_BIN_PATH", &launch_path)
        .output()
        .expect("invoke stale daemon command");

    assert!(
        output.status.success(),
        "valid candidate must reexec successfully"
    );
    let argv = std::fs::read_to_string(&args_file).unwrap();
    assert!(
        argv.contains(&format!("{}\n", launch_path.display())),
        "argv:\n{argv}"
    );
    assert!(argv.contains("agents\nrun\n"), "argv:\n{argv}");
    assert!(argv.contains("--once\n"), "argv:\n{argv}");
    assert!(argv.contains("--poll-interval\n0.05\n"), "argv:\n{argv}");
}

/// HIGH fix: the daemon must bail BEFORE opening the DB whenever startup fails.
///
/// This test verifies that a corrupt/invalid private binary causes an early
/// non-zero exit before the DB is migrated or seeded.  With validate-on-shortcut
/// (T076 codex-revise), the daemon now validates the existing private binary
/// before the stale-detection path runs, so the failure happens even earlier
/// than the original "stale-at-startup" scenario — but the invariant (DB stays
/// untouched) is identical and stronger.
///
/// Strategy: set STORES_DAEMON_BIN_PATH to a non-executable file so the
/// existence-shortcut validation fails immediately.  Daemon must exit non-zero
/// and must NOT have opened/migrated the DB file.
#[cfg(unix)]
#[test]
fn stale_at_startup_no_db_side_effects_before_reexec() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;

    let tmp = tempfile::tempdir().unwrap();
    let stores_dir = tmp.path().join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();
    std::fs::write(stores_dir.join("manifest.yaml"), "stores: []\n").unwrap();
    // Create the DB file but leave it empty (no DDL) so any migration/seed
    // attempt would write rows we can detect.
    std::fs::write(stores_dir.join("db.sqlite"), "").unwrap();

    // Non-executable stub as the private binary: triggers validate-on-shortcut
    // failure before the daemon can open the DB.
    let launch_path = tmp.path().join("stores_stale_stub");
    std::fs::write(&launch_path, "not executable\n").unwrap();
    let mut perms = std::fs::metadata(&launch_path).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&launch_path, perms).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&launch_path)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_TEST_DAEMON_FORCE_STALE", "1")
        .env("STORES_DAEMON_BIN_PATH", &launch_path)
        .output()
        .expect("invoke stale-at-startup daemon command");

    // Must exit non-zero (validation failed fail-loud).
    assert!(
        !output.status.success(),
        "invalid private binary must cause non-zero exit"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Validation failure context must be present.
    assert!(
        stderr.contains(&launch_path.display().to_string()),
        "private path must appear in error; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("validation")
            || stderr.contains("failed")
            || stderr.contains("private daemon binary")
            || stderr.contains("spawn error"),
        "error must describe validation failure; stderr:\n{stderr}"
    );
    // The DB file we placed is empty (0 bytes), meaning no migration ran.
    // If migration had run, rusqlite would have written the DDL and the file
    // would be non-empty (SQLite page size ≥ 4096 bytes).
    let db_size = std::fs::metadata(stores_dir.join("db.sqlite"))
        .unwrap()
        .len();
    assert_eq!(
        db_size, 0,
        "DB must remain untouched (size 0) when daemon fails before DB open"
    );
}

#[cfg(unix)]
#[test]
fn fresh_identity_no_reexec_attempt() {
    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env_remove("STORES_TEST_DAEMON_FORCE_STALE")
        .env(
            "STORES_DAEMON_BIN_PATH",
            tmp.path().join("private-bin/stores"),
        )
        .output()
        .expect("invoke fresh daemon command");

    assert!(output.status.success(), "fresh daemon --once should exit 0");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(STALE_DAEMON_MESSAGE), "stderr:\n{stderr}");
}

#[test]
fn fresh_auto_drive_still_records_positive_drive_pid() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    STALE_HALTED.store(false, Ordering::SeqCst);
    let c = conn();
    let tmp = tempfile::tempdir().unwrap();
    insert_candidate(&c, 150, "T150", tmp.path());
    let agents = AgentsYaml {
        agents: vec![auto_drive_agent()],
        deployment_specialist: None,
    };
    let ident = BinaryIdentity { dev: 2, ino: 2 };
    let guard = mock_guard(ident, PathBuf::from("/usr/local/bin/stores"), ident);
    std::env::set_var("STORES_DRIVE_CMD", "sleep 2 #");

    let n = poll_once_with_guard(
        &c,
        &agents,
        &policies(),
        &tmp.path().join("config.yaml"),
        "test",
        "epoch",
        Some(&guard),
    )
    .unwrap();
    std::env::remove_var("STORES_DRIVE_CMD");
    assert_eq!(n, 1);
    let pid: i64 = c
        .query_row(
            "SELECT drive_pid FROM tasks WHERE display_id='T150'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(pid > 0);
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// Real Unix inode-replacement integration test (MINOR codex fix).
///
/// Proves the actual `/proc/self/exe` vs launch-path inode-mismatch scenario
/// that triggered the L149 incident — without relying on
/// `STORES_TEST_DAEMON_FORCE_STALE`.
///
/// Strategy:
///   1. Copy the stores binary to a temp path A (inode A).
///   2. Copy the stores binary to a second temp path B (inode B), then
///      atomically rename B → A so the file at path A now has inode B.
///   3. Launch the REAL stores binary (path from CARGO_BIN_EXE_stores) with
///      argv[0] overridden to path A via `Command::arg0`. The daemon resolves:
///        - current_exe (startup_identity)  = CARGO_BIN_EXE_stores inode
///        - launch_path = path A            = inode B  (≠ startup_identity)
///   4. Assert: self-reexecs successfully and emits one reexec line on stderr.
///
/// The inode mismatch is real — no env-var bypass.
#[cfg(unix)]
#[test]
fn first_run_migration_seeds_private_binary_and_records_private_launch_path() {
    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let private = tmp.path().join("home/.local/share/stores/bin/stores");

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_DAEMON_BIN_PATH", &private)
        .env_remove("STORES_TEST_DAEMON_FORCE_STALE")
        .output()
        .expect("first-run daemon migration");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(private.exists(), "private binary must be seeded");
    let conn = Connection::open(tmp.path().join(".stores/db.sqlite")).unwrap();
    let recorded: String = conn
        .query_row("SELECT binary_path FROM daemon_starts LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(recorded, private.display().to_string());
}

#[cfg(unix)]
#[test]
fn changed_global_launch_does_not_replace_private_or_emit_canonical_stale_message() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let private = tmp.path().join("private/bin/stores");
    std::fs::create_dir_all(private.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_stores"), &private).unwrap();
    let mut perms = std::fs::metadata(&private).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&private, perms).unwrap();
    let before = std::fs::metadata(&private).unwrap().len();

    let cargo_home = tmp.path().join("cargo-home");
    let global = cargo_home.join("bin/stores");
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_stores"), &global).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&global)
        .unwrap()
        .write_all(b"\n# simulated external cargo install overwrite\n")
        .unwrap();
    let mut perms = std::fs::metadata(&global).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&global, perms).unwrap();
    assert_ne!(std::fs::metadata(&global).unwrap().len(), before);

    let output = Command::new(&global)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_DAEMON_BIN_PATH", &private)
        .env("CARGO_HOME", &cargo_home)
        .env(
            "PATH",
            format!(
                "{}:{}",
                global.parent().unwrap().display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env_remove("STORES_TEST_DAEMON_FORCE_STALE")
        .output()
        .expect("daemon launched via changed global stores path");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::metadata(&private).unwrap().len(), before);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains(STALE_DAEMON_MESSAGE), "stderr:\n{stderr}");
}

#[cfg(unix)]
#[test]
fn replacing_private_binary_reexecs_validated_replacement_once() {
    use std::os::unix::process::CommandExt;

    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());
    let private = tmp.path().join("private/bin/stores");
    std::fs::create_dir_all(private.parent().unwrap()).unwrap();
    let args_file = tmp.path().join("replacement-argv.txt");
    write_reexec_stub(&private, &args_file, 0, StubHelpBehavior::Valid);

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .arg0(&private)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_DAEMON_BIN_PATH", &private)
        .env_remove("STORES_TEST_DAEMON_FORCE_STALE")
        .output()
        .expect("private replacement reexec");

    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<_> = stderr
        .lines()
        .filter(|l| l.contains("daemon binary stale; reexecing into"))
        .collect();
    assert_eq!(lines.len(), 1, "stderr:\n{stderr}");
    assert!(
        lines[0].contains(&private.display().to_string()),
        "{}",
        lines[0]
    );
    assert!(args_file.exists(), "validated replacement must be exec'd");
}

#[cfg(unix)]
#[test]
fn real_inode_replace_triggers_stale_detection() {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;

    let tmp = tempfile::tempdir().unwrap();
    let stores_dir = tmp.path().join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();
    std::fs::write(stores_dir.join("manifest.yaml"), "stores: []\n").unwrap();
    let db = Connection::open(stores_dir.join("db.sqlite")).unwrap();
    db.execute_batch(SUBSTRATE_DDL).unwrap();
    drop(db);

    // The real stores binary compiled by cargo.
    let stores_exe = std::path::Path::new(env!("CARGO_BIN_EXE_stores"));

    // Step 1: copy to path A.
    let path_a = tmp.path().join("stores_launch");
    std::fs::copy(stores_exe, &path_a).unwrap();
    let mut perms = std::fs::metadata(&path_a).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path_a, perms).unwrap();

    // Step 2: copy to path B, then atomic rename B → A (new inode at path A).
    let path_b = tmp.path().join("stores_replacement");
    std::fs::copy(stores_exe, &path_b).unwrap();
    let mut perms = std::fs::metadata(&path_b).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path_b, perms).unwrap();
    std::fs::rename(&path_b, &path_a).unwrap();

    // Step 3: launch the real stores binary with argv[0] = path_a.
    // `current_exe()` inside the daemon resolves to stores_exe's inode (S).
    // `launch_path` resolves to path_a via argv[0] → inode B ≠ S → stale.
    let output = Command::new(stores_exe)
        .arg0(&path_a)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        // Ensure STORES_TEST_DAEMON_FORCE_STALE is NOT set so this exercises
        // the real dev/ino path.
        .env_remove("STORES_TEST_DAEMON_FORCE_STALE")
        .env(
            "STORES_DAEMON_BIN_PATH",
            tmp.path().join("private-bin/stores"),
        )
        .output()
        .expect("spawn stores agents run --once");

    // Step 4: assert stale detection self-reexeced into path_a.
    assert!(
        output.status.success(),
        "stale daemon (real inode replace) must self-reexec then exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let matching: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("daemon binary stale; reexecing into"))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one reexec message line expected; stderr:\n{stderr}"
    );
    assert!(!stderr.contains(STALE_DAEMON_MESSAGE), "stderr:\n{stderr}");
}

/// HIGH fix (codex-revise): existence-shortcut must validate before returning.
///
/// Pre-creates a corrupt (random bytes) private binary and asserts that
/// `stores agents run` exits non-zero with a clear validation failure message
/// rather than blindly trusting the corrupt path.  This exercises the
/// validate-on-shortcut branch added in T076 codex-revise.
#[cfg(unix)]
#[test]
fn corrupt_existing_private_binary_rejected_fail_loud() {
    let tmp = tempfile::tempdir().unwrap();
    init_empty_stores_dir(tmp.path());

    // Create the private path with its parent directory and fill it with
    // random bytes (not a valid ELF / script).
    let private = tmp.path().join("private/bin/stores");
    std::fs::create_dir_all(private.parent().unwrap()).unwrap();
    // Write clearly-invalid content.
    std::fs::write(&private, b"\x7fELF-INVALID-CORRUPT-BYTES\x00\x01\x02\x03").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&private).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&private, perms).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_DAEMON_BIN_PATH", &private)
        .env_remove("STORES_TEST_DAEMON_FORCE_STALE")
        .output()
        .expect("invoke daemon with corrupt private binary");

    assert!(
        !output.status.success(),
        "corrupt existing private binary must cause non-zero exit; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Must surface validation failure context (path + reason), not silently
    // use the corrupt binary or panic.
    assert!(
        stderr.contains(&private.display().to_string()),
        "private path must appear in error output; stderr:\n{stderr}"
    );
    // Should mention validation failure
    assert!(
        stderr.contains("validation")
            || stderr.contains("failed")
            || stderr.contains("private daemon binary"),
        "error output must describe validation failure; stderr:\n{stderr}"
    );
}
