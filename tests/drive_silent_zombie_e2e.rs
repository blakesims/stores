//! T030 Phase 4: end-to-end test for the L062 silent-zombie watchdog using
//! the real `stores` binary (NOT a shell-stub override — that was the
//! masking pattern T022 introduced and which T030 specifically rejects).
//!
//! Bootstrap the workspace via the library (substrate DDL + tasks DDL), seed
//! a row in the L062 shape (status='executing', drive_pid=<dead>, closed
//! dispatch_lock with old `claimed_at` past the grace window, matching
//! transition_history), then invoke `stores agents run --once` and assert
//! the row flipped to `blocked` with `drive_failed:silent_zombie_pid_dead`.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

use rusqlite::Connection;
use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::schema::Schema;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stores"))
}

fn dead_pid() -> i64 {
    // High-numbered PID overwhelmingly likely to be unallocated on Linux
    // (default pid_max is typically 4194304 == 0x400000; this is well above).
    0x7fff_fffe
}

fn install_store_ddl(conn: &Connection, name: &str) {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, y)| *y)
        .unwrap_or_else(|| panic!("bundled schema for '{name}' missing"));
    let schema = Schema::from_yaml(yaml).unwrap();
    conn.execute_batch(&ddl_for(&schema)).unwrap();
}

#[test]
fn agents_run_once_flag_exists_and_succeeds() {
    // AC4.3: `stores agents run --once --help` exits 0; `--once` flag is documented.
    let help = Command::new(bin())
        .args(["agents", "run", "--help"])
        .output()
        .expect("invoke help");
    assert!(help.status.success(), "help should exit 0");
    let txt = String::from_utf8_lossy(&help.stdout);
    assert!(
        txt.contains("--once"),
        "agents run --help must document --once; got:\n{txt}"
    );
}

#[test]
fn silent_zombie_lock_already_closed_e2e() {
    let bin = bin();
    assert!(bin.exists(), "CARGO_BIN_EXE_stores must point at a built binary");

    let tmp = tempfile::tempdir().expect("tmpdir");
    let workspace = tmp.path();
    let stores_dir = workspace.join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();

    // ---- 1. Build .stores/db.sqlite with substrate + tasks + observations DDL ----
    let db_path = stores_dir.join("db.sqlite");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        install_store_ddl(&conn, "tasks");
        install_store_ddl(&conn, "observations");
    }

    // ---- 2. Empty manifest so any subsequent CLI invocation is well-formed ----
    std::fs::write(stores_dir.join("manifest.yaml"), "stores: []\n").unwrap();

    // ---- 3. Seed the L062 silent-zombie shape directly via SQL ----
    //
    // The watchdog grace cutoff is `now - ZOMBIE_GRACE_SECS` (10s). Use a
    // long-past `claimed_at` and `drive_started_at` so the row is well
    // outside the grace window regardless of test execution time.
    let stale = "2026-01-01T00:00:00Z";
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, \
                            contract, drive_pid, drive_started_at, \
                            created_at, updated_at, created_by, updated_by) \
         VALUES ('T999', 'executing', 'silent-zombie', 'silent-zombie', 'feat/x', \
                 '/tmp/no-such', \
                 '{\"done_when\":\"x\",\"scope_in\":\"y\",\"scope_out\":\"z\"}', \
                 ?1, ?2, ?2, ?2, 'framework', 'framework')",
        rusqlite::params![dead_pid(), stale],
    )
    .unwrap();
    let row_id: i64 = conn
        .query_row("SELECT id FROM tasks WHERE display_id='T999'", [], |r| {
            r.get(0)
        })
        .unwrap();

    // Closed auto-drive dispatch_lock (this is the L062-defining state: the
    // post-spawn `mark_claim_finished` already fired, so the open-lock sweep
    // skips it; only the silent-zombie scan can rescue this row).
    conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, claimed_at, \
          claimed_by, last_status, finished_at) \
         VALUES ('tasks', ?1, 'T999', 'auto-drive', 1, ?2, \
                 'test-claimer', 'ok', ?2)",
        rusqlite::params![row_id, stale],
    )
    .unwrap();

    // Seed a transition_history row so the row's audit trail is well-formed
    // before the framework writes `mark_drive_failed` on top.
    conn.execute(
        "INSERT INTO transition_history \
         (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
         VALUES ('tasks', ?1, 'T999', 'ready', 'executing', 'submit_plan_review', \
                 'ai_autonomous', ?2)",
        rusqlite::params![row_id, stale],
    )
    .unwrap();
    drop(conn);

    // ---- 4. Invoke the real binary: `stores agents run --once` ----
    let output = Command::new(&bin)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(workspace)
        .output()
        .expect("invoke daemon");
    assert!(
        output.status.success(),
        "agents run --once must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // ---- 5. Assert: row flipped to blocked with structured reason ----
    let conn = Connection::open(&db_path).unwrap();
    let (status, blocked_reason): (String, Option<String>) = conn
        .query_row(
            "SELECT status, blocked_reason FROM tasks WHERE display_id='T999'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        status, "blocked",
        "watchdog should have flipped row to 'blocked' (got '{status}')"
    );
    assert_eq!(
        blocked_reason.as_deref(),
        Some("drive_failed:silent_zombie_pid_dead"),
        "blocked_reason must carry the structured silent-zombie suffix"
    );

    // Exactly one transition_history row landed for the watchdog flip.
    let cnt: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history \
             WHERE display_id='T999' AND to_status='blocked'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cnt, 1, "exactly one to_status='blocked' history row must land");
}
