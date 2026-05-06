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
    assert_eq!(claims, 0, "no dispatch_locks entry must be created on stale");
    assert!(drive_pid.is_none(), "drive_pid must remain NULL on stale");
    assert_eq!(guard.check_stale().unwrap(), Some(STALE_DAEMON_MESSAGE));
}

#[test]
fn stale_daemon_command_exits_nonzero_and_emits_one_message_line() {
    let tmp = tempfile::tempdir().unwrap();
    let stores_dir = tmp.path().join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();
    std::fs::write(stores_dir.join("manifest.yaml"), "stores: []\n").unwrap();
    let db = Connection::open(stores_dir.join("db.sqlite")).unwrap();
    db.execute_batch(SUBSTRATE_DDL).unwrap();
    drop(db);

    let output = Command::new(env!("CARGO_BIN_EXE_stores"))
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(tmp.path())
        .env("STORES_TEST_DAEMON_FORCE_STALE", "1")
        .output()
        .expect("invoke stale daemon command");

    assert!(!output.status.success(), "stale daemon must exit nonzero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let matching_lines: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains(STALE_DAEMON_MESSAGE))
        .collect();
    assert_eq!(
        matching_lines.len(),
        1,
        "stderr must contain exactly one stale message line; stderr:\n{stderr}"
    );
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
///   4. Assert: exits non-zero, exactly one stale message line on stderr.
///
/// The inode mismatch is real — no env-var bypass.
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
        .output()
        .expect("spawn stores agents run --once");

    // Step 4: assert stale detection.
    assert!(
        !output.status.success(),
        "stale daemon (real inode replace) must exit nonzero; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let matching: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains(STALE_DAEMON_MESSAGE))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one stale message line expected; stderr:\n{stderr}"
    );
}
