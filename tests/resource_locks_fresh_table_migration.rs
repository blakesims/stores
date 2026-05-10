use std::process::Command;

const OLD_SUBSTRATE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS transition_history (
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
    occurred_at TEXT NOT NULL,
    actor_note TEXT
);
CREATE TABLE IF NOT EXISTS dispatch_locks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    store TEXT NOT NULL,
    row_id INTEGER NOT NULL,
    display_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    transition_id INTEGER,
    claimed_at TEXT NOT NULL,
    claimed_by TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 1,
    last_status TEXT,
    finished_at TEXT,
    daemon_epoch TEXT,
    claim_source TEXT CHECK(claim_source IN ('try_claim','retry_claim','manual','legacy')),
    attempt INTEGER,
    pid INTEGER,
    heartbeat_at TEXT,
    postcondition_id TEXT,
    postcondition_args TEXT,
    terminal_reason TEXT CHECK(terminal_reason IN ('ok','exit_nonzero','error','silent_zombie','timeout','halted','legacy_unknown','rate_limit')),
    next_retry_at TEXT,
    UNIQUE(store, row_id, agent_name)
);
CREATE TABLE IF NOT EXISTS daemon_starts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    display_id TEXT UNIQUE NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT,
    updated_at TEXT,
    created_by TEXT,
    updated_by TEXT,
    pid INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    binary_path TEXT NOT NULL,
    binary_version TEXT NOT NULL,
    git_sha TEXT NOT NULL,
    argv TEXT NOT NULL,
    log_file TEXT,
    cwd TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS agent_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    display_id TEXT NOT NULL,
    phase INTEGER NOT NULL,
    cycle INTEGER NOT NULL,
    role TEXT NOT NULL,
    model_id TEXT NOT NULL,
    harness_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT NOT NULL,
    exit_code INTEGER NOT NULL,
    tokens_in INTEGER,
    tokens_out INTEGER,
    prompt_cache_hits INTEGER,
    transcript_path TEXT NOT NULL,
    brief_text TEXT
);
CREATE TABLE IF NOT EXISTS engine_runner_heartbeats (
    iteration INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    saw_tasks INTEGER NOT NULL,
    saw_intake INTEGER NOT NULL,
    saw_observations INTEGER NOT NULL,
    actionable INTEGER NOT NULL,
    held INTEGER NOT NULL,
    dispatched INTEGER NOT NULL,
    PRIMARY KEY(iteration, started_at)
);
CREATE TABLE IF NOT EXISTS engine_runner_actions (
    store TEXT NOT NULL,
    row_id INTEGER NOT NULL,
    classification TEXT NOT NULL,
    action TEXT,
    held_reason TEXT,
    dispatched INTEGER NOT NULL CHECK(dispatched IN (0,1)),
    last_logged_at TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(store, row_id)
);
CREATE TABLE IF NOT EXISTS substrate_migrations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    applied_at TEXT NOT NULL,
    binary_version TEXT NOT NULL,
    table_name TEXT NOT NULL,
    column_name TEXT NOT NULL,
    ddl_applied TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS framework_migrations (
    id TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL,
    note TEXT
);
";

#[test]
fn creates_resource_locks_on_same_existing_db_path() {
    let dbfile = tempfile::NamedTempFile::new().unwrap();
    let temp_path = dbfile.path().to_path_buf();
    let raw = rusqlite::Connection::open(&temp_path).unwrap();
    raw.execute_batch(OLD_SUBSTRATE_DDL).unwrap();
    let before: i64 = raw.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='resource_locks'", [], |r| r.get(0)).unwrap();
    assert_eq!(before, 0);
    drop(raw);
    let conn = stores::db::open(&temp_path).unwrap();
    let after: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='resource_locks'", [], |r| r.get(0)).unwrap();
    assert_eq!(after, 1);
    drop(conn);

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".stores")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&temp_path, root.path().join(".stores/db.sqlite")).unwrap();
    #[cfg(not(unix))]
    std::fs::hard_link(&temp_path, root.path().join(".stores/db.sqlite")).unwrap();
    let bin = env!("CARGO_BIN_EXE_stores");
    let list = Command::new(bin).current_dir(root.path()).args(["resource-locks", "list"]).output().unwrap();
    assert!(list.status.success(), "{}", String::from_utf8_lossy(&list.stderr));
    assert!(String::from_utf8_lossy(&list.stdout).trim().is_empty());
    let acq = Command::new(bin).current_dir(root.path()).args(["resource-locks","acquire","--resource","main_branch","--owner","T999","--owner-kind","task","--invoker","human"]).output().unwrap();
    assert!(acq.status.success(), "{}", String::from_utf8_lossy(&acq.stderr));
    let token = String::from_utf8_lossy(&acq.stdout).trim().to_string();
    let rel = Command::new(bin).current_dir(root.path()).args(["resource-locks","release","--resource","main_branch","--token",&token,"--invoker","human"]).output().unwrap();
    assert!(rel.status.success(), "{}", String::from_utf8_lossy(&rel.stderr));
}
