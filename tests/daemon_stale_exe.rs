use anyhow::{anyhow, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use stores::codegen::ddl::SUBSTRATE_DDL;
use stores::flow::agents_yaml::TransitionEdge;
use stores::flow::{AgentEntry, AgentsYaml, BackoffKind, PoliciesYaml, RetryPolicy, Subscription};
use stores::handlers::agents_run::{
    poll_once_with_guard, BinaryIdentity, BinaryIdentityProvider, DaemonExeGuard, DaemonExeStatus,
    STALE_DAEMON_MESSAGE,
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
    assert_eq!(n, 0);
    let (claims, drive_pid): (i64, Option<i64>) = c.query_row(
        "SELECT (SELECT COUNT(*) FROM dispatch_locks WHERE agent_name='auto-drive'), drive_pid FROM tasks WHERE display_id='T149'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(claims, 0);
    assert!(drive_pid.is_none());
    assert_eq!(guard.check_stale().unwrap(), Some(STALE_DAEMON_MESSAGE));
}

#[test]
fn fresh_auto_drive_still_records_positive_drive_pid() {
    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
