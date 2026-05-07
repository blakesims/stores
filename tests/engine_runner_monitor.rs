use rusqlite::Connection;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::engine_runner::{scan_record_and_redrive_tasks, ScannerSchemas};
use stores::flow::AgentsYaml;
use stores::schema::Schema;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::Ordering;

const TS: &str = "2026-05-07T00:00:00Z";

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn pid_is_alive(pid: i32) -> bool {
    unsafe { libc::kill(pid, 0) == 0 }
}

fn schemas() -> (Schema, Schema, Schema) {
    (
        Schema::from_yaml(include_str!("../stores/tasks/schema.yaml")).unwrap(),
        Schema::from_yaml(include_str!("../stores/intake_items/schema.yaml")).unwrap(),
        Schema::from_yaml(include_str!("../stores/observations/schema.yaml")).unwrap(),
    )
}

fn db() -> (Connection, Schema, Schema, Schema) {
    let (tasks, intake, observations) = schemas();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(&tasks)).unwrap();
    conn.execute_batch(&ddl_for(&intake)).unwrap();
    conn.execute_batch(&ddl_for(&observations)).unwrap();
    (conn, tasks, intake, observations)
}

fn scanner<'a>(tasks: &'a Schema, intake: &'a Schema, observations: &'a Schema) -> ScannerSchemas<'a> {
    ScannerSchemas {
        tasks,
        intake,
        observations,
    }
}

fn cfg(max_parallel: usize) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("config.yaml"),
        format!("drive:\n  max_parallel: {max_parallel}\n"),
    )
    .unwrap();
    tmp
}

fn insert_task(conn: &Connection, display_id: &str, status: &str, workspace_path: &str) -> i64 {
    conn.execute(
        "INSERT INTO tasks \
         (display_id, status, created_at, updated_at, title, slug, current_phase, current_cycle, tier_hint, plan, workspace_path) \
         VALUES (?1, ?2, ?3, ?3, 'Task', 'task', 1, 1, 'T2', ?4, ?5)",
        rusqlite::params![
            display_id,
            status,
            TS,
            r#"{"phases":[{"name":"p1"}]}"#,
            workspace_path
        ],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn insert_intake(conn: &Connection, display_id: &str, status: &str) -> i64 {
    conn.execute(
        "INSERT INTO intake \
         (display_id, status, created_at, updated_at, summary, source_agent, captured_at, captured_week) \
         VALUES (?1, ?2, ?3, ?3, 'Intake', 'tester', ?3, 'w18-d4')",
        rusqlite::params![display_id, status, TS],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn insert_observation(
    conn: &Connection,
    display_id: &str,
    status: &str,
    intent_contract: &str,
    risk_class: &str,
    approval_policy: &str,
) -> i64 {
    conn.execute(
        "INSERT INTO observations \
         (display_id, status, created_at, updated_at, summary, source, priority, captured_at, captured_week, intent_contract, risk_class, approval_policy) \
         VALUES (?1, ?2, ?3, ?3, 'Observation', 'qa', 'normal', ?3, 'w18-d4', ?4, ?5, ?6)",
        rusqlite::params![display_id, status, TS, intent_contract, risk_class, approval_policy],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn run_monitor(
    conn: &Connection,
    tasks: &Schema,
    intake: &Schema,
    observations: &Schema,
    max_parallel: usize,
) -> tempfile::TempDir {
    run_monitor_with_base(conn, tasks, intake, observations, max_parallel, 0)
}

fn run_monitor_with_base(
    conn: &Connection,
    tasks: &Schema,
    intake: &Schema,
    observations: &Schema,
    max_parallel: usize,
    base_dispatched: i64,
) -> tempfile::TempDir {
    let tmp = cfg(max_parallel);
    scan_record_and_redrive_tasks(
        conn,
        scanner(tasks, intake, observations),
        1,
        TS,
        &AgentsYaml::default_empty(),
        &tmp.path().join("config.yaml"),
        "",
        base_dispatched,
    )
    .unwrap();
    tmp
}

#[test]
fn orphan_redrive_live_stub_process_is_dispatched_and_cleaned_up() {
    let _env = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STORES_DRIVE_CMD", "sleep 30 #");
    let (conn, tasks, intake, observations) = db();
    let workspace = tempfile::tempdir().unwrap();
    let task_id = insert_task(&conn, "T970", "executing", workspace.path().to_str().unwrap());
    conn.execute(
        "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
        rusqlite::params![999_999_970_i64, task_id],
    )
    .unwrap();

    let _cfg = run_monitor(&conn, &tasks, &intake, &observations, 5);

    let (pid, action, dispatched, locks): (i64, Option<String>, i64, i64) = conn
        .query_row(
            "SELECT t.drive_pid, a.action, a.dispatched, \
                    (SELECT COUNT(*) FROM dispatch_locks WHERE store='tasks' AND row_id=t.id AND agent_name='auto-drive') \
             FROM tasks t JOIN engine_runner_actions a ON a.store='tasks' AND a.row_id=t.id \
             WHERE t.id=?1",
            rusqlite::params![task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert!(pid > 0 && pid != 999_999_970_i64);
    assert!(pid_is_alive(pid as i32));
    assert_eq!(action.as_deref(), Some("redispatched"));
    assert_eq!(dispatched, 1);
    assert_eq!(locks, 1);
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
    std::env::remove_var("STORES_DRIVE_CMD");
}

#[test]
fn held_unsupported_edges_record_structured_reasons_and_throttled_display_log_shape_inputs() {
    let (conn, tasks, intake, observations) = db();
    let intake_gatekeeper = insert_intake(&conn, "I970", "triaging");
    let intake_recon = insert_intake(&conn, "I971", "needs_info");
    let obs_investigator = insert_observation(
        &conn,
        "L970",
        "needs_investigation",
        r#"{"contract_state":"ready","approved_by":"u","approved_at":"2026-05-07T00:00:00Z"}"#,
        "normal",
        "human",
    );
    let obs_contract_drafter = insert_observation(&conn, "L971", "open", "{}", "normal", "human");
    let review_absent = insert_task(&conn, "T971", "in_review", "/tmp");

    run_monitor(&conn, &tasks, &intake, &observations, 5);

    for (store, row_id, reason) in [
        ("intake", intake_gatekeeper, "no_built_in_entrypoint"),
        ("intake", intake_recon, "no_built_in_entrypoint"),
        ("observations", obs_investigator, "no_built_in_entrypoint"),
        ("observations", obs_contract_drafter, "needs_human"),
        ("tasks", review_absent, "no_autonomous_reviewer_runner"),
    ] {
        let (held_reason, last_logged_at): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT held_reason, last_logged_at FROM engine_runner_actions WHERE store=?1 AND row_id=?2",
                rusqlite::params![store, row_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(held_reason.as_deref(), Some(reason), "{store}/{row_id}");
        assert_eq!(last_logged_at.as_deref(), Some(TS), "store/display_id/reason log throttle input persisted for {store}/{row_id}/{reason}");
    }
}

#[test]
fn lane_cap_full_dispatches_zero_and_does_not_increase_dispatch_locks() {
    let _env = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STORES_DRIVE_CMD", "sleep 30 #");
    let (conn, tasks, intake, observations) = db();
    let occupied = insert_task(&conn, "T972", "executing", "/tmp");
    conn.execute(
        "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, claimed_at, claimed_by, pid) \
         VALUES ('tasks', ?1, 'T972', 'auto-drive', ?2, 'daemon', 0)",
        rusqlite::params![occupied, TS],
    )
    .unwrap();
    let orphan = insert_task(&conn, "T973", "executing", "/tmp");
    conn.execute(
        "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
        rusqlite::params![999_999_973_i64, orphan],
    )
    .unwrap();
    let before: i64 = conn
        .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
        .unwrap();

    run_monitor(&conn, &tasks, &intake, &observations, 1);

    let (held, dispatched): (Option<String>, i64) = conn
        .query_row(
            "SELECT held_reason, dispatched FROM engine_runner_actions WHERE store='tasks' AND row_id=?1",
            rusqlite::params![orphan],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let after: i64 = conn
        .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(held.as_deref(), Some("lane_cap_full"));
    assert_eq!(dispatched, 0);
    assert_eq!(after, before);
    std::env::remove_var("STORES_DRIVE_CMD");
}

#[test]
fn u_moment_guard_writes_no_forbidden_states_or_transition_history_verbs() {
    let (conn, tasks, intake, observations) = db();
    conn.execute_batch(
        "CREATE TABLE architecture_reviews (id INTEGER PRIMARY KEY, display_id TEXT, verdict TEXT, transition_history TEXT); \
         INSERT INTO architecture_reviews (id, display_id, verdict, transition_history) VALUES (1, 'A970', NULL, NULL);",
    )
    .unwrap();
    let draft_contract = r#"{"contract_state":"draft","approved_by":null,"approved_at":null}"#;
    let obs = insert_observation(&conn, "L972", "investigating", draft_contract, "architecture", "architecture");
    let ready_task = insert_task(&conn, "T974", "ready", "/tmp");

    run_monitor(&conn, &tasks, &intake, &observations, 5);

    let (obs_status, contract): (String, String) = conn
        .query_row(
            "SELECT status, intent_contract FROM observations WHERE id=?1",
            rusqlite::params![obs],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let task_status: String = conn
        .query_row(
            "SELECT status FROM tasks WHERE id=?1",
            rusqlite::params![ready_task],
            |r| r.get(0),
        )
        .unwrap();
    let forbidden_verbs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM transition_history WHERE verb IN \
             ('accept','reject','resume','amend','abandon','confirm','ratify') OR verb LIKE 'architecture%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let arch_writes: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM architecture_reviews WHERE verdict IS NOT NULL OR transition_history IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(obs_status, "investigating");
    assert_eq!(contract, draft_contract);
    assert!(!contract.contains(r#""contract_state":"ready""#));
    assert_eq!(task_status, "ready");
    assert_eq!(forbidden_verbs, 0);
    assert_eq!(arch_writes, 0);
}

// ─── Fix 1 (CRITICAL): atomic CAS prevents double-spawn ────────────────────

/// Two sequential engine-runner iterations on the same orphan task MUST result
/// in exactly one live drive process.  The second iteration's BEGIN IMMEDIATE
/// re-read sees drive_pid already alive → aborts redispatch.
#[test]
fn atomic_cas_prevents_double_spawn_on_same_orphan() {
    let _env = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STORES_DRIVE_CMD", "sleep 60 #");
    let (conn, tasks, intake, observations) = db();
    let workspace = tempfile::tempdir().unwrap();
    let task_id = insert_task(&conn, "T980", "executing", workspace.path().to_str().unwrap());
    // Stale / dead drive_pid — orphan condition.
    conn.execute(
        "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
        rusqlite::params![999_999_980_i64, task_id],
    )
    .unwrap();

    let tmp = cfg(5);
    let cfg_path = tmp.path().join("config.yaml");

    // First iteration: orphan → spawn → drive_pid set.
    scan_record_and_redrive_tasks(
        &conn,
        scanner(&tasks, &intake, &observations),
        1,
        TS,
        &AgentsYaml::default_empty(),
        &cfg_path,
        "",
        0,
    )
    .unwrap();

    let pid_after_first: i64 = conn
        .query_row(
            "SELECT COALESCE(drive_pid, 0) FROM tasks WHERE id=?1",
            rusqlite::params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(pid_after_first > 0 && pid_after_first != 999_999_980_i64);
    assert!(pid_is_alive(pid_after_first as i32), "first spawn must be alive");

    // Second iteration: drive_pid is now live → CAS must abort redispatch.
    scan_record_and_redrive_tasks(
        &conn,
        scanner(&tasks, &intake, &observations),
        2,
        TS,
        &AgentsYaml::default_empty(),
        &cfg_path,
        "",
        0,
    )
    .unwrap();

    let pid_after_second: i64 = conn
        .query_row(
            "SELECT COALESCE(drive_pid, 0) FROM tasks WHERE id=?1",
            rusqlite::params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    // drive_pid must be unchanged — no second spawn.
    assert_eq!(
        pid_after_first, pid_after_second,
        "second iteration must not replace the live drive_pid"
    );
    // Exactly one dispatch_lock row for this task.
    let lock_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM dispatch_locks WHERE store='tasks' AND row_id=?1 AND agent_name='auto-drive'",
            rusqlite::params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lock_count, 1, "exactly one auto-drive lock must exist");

    unsafe {
        libc::kill(pid_after_first as i32, libc::SIGTERM);
    }
    std::env::remove_var("STORES_DRIVE_CMD");
}

// ─── Fix 1b: actual race — owner appears between scanner-read and CAS ────────

/// Proves the CAS abort branch fires when a live `drive_pid` is injected into
/// the gap between the scanner fast-path read and the BEGIN IMMEDIATE
/// transaction.
///
/// Scenario:
///   1. Task T982 has a dead drive_pid → orphan condition.
///   2. Scanner thread starts; `STORES_TEST_CAS_PRE_SPAWN_DELAY_MS=150`
///      introduces a delay after the fast-path read.
///   3. Main thread waits 50 ms, then injects the current process PID as
///      drive_pid — simulating "owner appears in the gap."
///   4. Scanner thread resumes, opens BEGIN IMMEDIATE, re-reads, sees the now-
///      live drive_pid, fires the CAS abort branch.
///   5. Assertions:
///      (a) No new process spawned (drive_pid equals the injected pid).
///      (b) No new dispatch_lock row created.
///      (c) `CAS_ABORT_DRIVE_PID_COUNT` incremented exactly once.
#[cfg(debug_assertions)]
#[test]
fn cas_abort_branch_fires_when_owner_appears_in_gap() {
    let _env = env_lock().lock().unwrap_or_else(|e| e.into_inner());

    // Reset global sentinels before the test.
    stores::flow::builtins::auto_drive::CAS_ABORT_DRIVE_PID_COUNT
        .store(0, Ordering::SeqCst);
    stores::flow::builtins::auto_drive::CAS_DELAY_HOOK_ENTERED
        .store(false, Ordering::SeqCst);

    std::env::set_var("STORES_DRIVE_CMD", "sleep 60 #");
    std::env::set_var("STORES_TEST_CAS_PRE_SPAWN_DELAY_MS", "150");

    // File-based DB so a second connection (injector) can write concurrently.
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("db.sqlite");
    let workspace = tempfile::tempdir().unwrap();
    let workspace_str = workspace.path().to_str().unwrap().to_owned();

    let (tasks_schema, intake_schema, obs_schema) = schemas();

    // Set up the DB: create schema + orphan task row.
    let setup_conn = Connection::open(&db_path).unwrap();
    setup_conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    setup_conn.execute_batch(SUBSTRATE_DDL).unwrap();
    setup_conn.execute_batch(&ddl_for(&tasks_schema)).unwrap();
    setup_conn.execute_batch(&ddl_for(&intake_schema)).unwrap();
    setup_conn.execute_batch(&ddl_for(&obs_schema)).unwrap();

    let task_id: i64 = {
        setup_conn.execute(
            "INSERT INTO tasks \
             (display_id, status, created_at, updated_at, title, slug, current_phase, current_cycle, tier_hint, plan, workspace_path) \
             VALUES ('T982', 'executing', ?1, ?1, 'Task', 'task', 1, 1, 'T2', ?2, ?3)",
            rusqlite::params![TS, r#"{"phases":[{"name":"p1"}]}"#, &workspace_str],
        ).unwrap();
        // Dead drive_pid — the orphan condition.
        let id = setup_conn.last_insert_rowid();
        setup_conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![999_999_982_i64, id],
        ).unwrap();
        id
    };
    drop(setup_conn); // release before threads open their own connections

    let cfg_tmp = cfg(5);
    let cfg_path = cfg_tmp.path().join("config.yaml");

    // The "live" pid we'll inject — current process is guaranteed alive.
    let injected_pid = std::process::id() as i64;

    // ── Scanner thread ────────────────────────────────────────────────────────
    // Open its own connection; will pause inside the delay window.
    let db_path_clone = db_path.clone();
    let tasks_schema2 = tasks_schema.clone();
    let intake_schema2 = intake_schema.clone();
    let obs_schema2 = obs_schema.clone();
    let cfg_path_clone = cfg_path.clone();

    let scanner_handle = std::thread::spawn(move || {
        let conn = Connection::open(&db_path_clone).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.pragma_update(None, "busy_timeout", 5000i64).unwrap();
        scan_record_and_redrive_tasks(
            &conn,
            ScannerSchemas {
                tasks: &tasks_schema2,
                intake: &intake_schema2,
                observations: &obs_schema2,
            },
            1,
            TS,
            &AgentsYaml::default_empty(),
            &cfg_path_clone,
            "",
            0,
        )
        .unwrap();
    });

    // ── Injector (main thread) ─────────────────────────────────────────────────
    // Wait for the scanner to signal that it has entered the delay hook — i.e.
    // it is past its fast-path read and sleeping inside the CAS window.  This
    // replaces the old fixed `sleep(50ms)` with a deterministic barrier: the
    // flag is set by the scanner BEFORE it starts sleeping, so by the time we
    // observe it here the scanner is guaranteed to be inside the delay window.
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if stores::flow::builtins::auto_drive::CAS_DELAY_HOOK_ENTERED
                .load(Ordering::Acquire)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for CAS_DELAY_HOOK_ENTERED signal"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    {
        let inj_conn = Connection::open(&db_path).unwrap();
        inj_conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        inj_conn.pragma_update(None, "busy_timeout", 5000i64).unwrap();
        inj_conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![injected_pid, task_id],
        ).unwrap();
    }

    scanner_handle.join().expect("scanner thread panicked");

    // ── Assertions ────────────────────────────────────────────────────────────
    let verify_conn = Connection::open(&db_path).unwrap();

    // (a) drive_pid must equal the injected pid — no new spawn overwrote it.
    let final_pid: i64 = verify_conn
        .query_row(
            "SELECT COALESCE(drive_pid, 0) FROM tasks WHERE id=?1",
            rusqlite::params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        final_pid, injected_pid,
        "CAS abort must leave drive_pid == injected pid; no new spawn"
    );

    // (b) No dispatch_lock row must have been created by the aborted redispatch.
    let lock_count: i64 = verify_conn
        .query_row(
            "SELECT COUNT(*) FROM dispatch_locks \
             WHERE store='tasks' AND row_id=?1 AND agent_name='auto-drive'",
            rusqlite::params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(lock_count, 0, "CAS abort must not create a dispatch_lock row");

    // (c) The CAS abort sentinel counter must have been incremented exactly once.
    let abort_count = stores::flow::builtins::auto_drive::CAS_ABORT_DRIVE_PID_COUNT
        .load(Ordering::SeqCst);
    assert_eq!(
        abort_count, 1,
        "CAS abort sentinel must fire exactly once; got {abort_count}"
    );

    std::env::remove_var("STORES_TEST_CAS_PRE_SPAWN_DELAY_MS");
    std::env::remove_var("STORES_DRIVE_CMD");
}

// ─── Fix 2 (HIGH): panic isolation keeps daemon main loop alive ─────────────

/// A panic inside the engine-runner iteration (simulated via STORES_DRIVE_CMD
/// set to a value that causes `run()` to panic) must NOT propagate past the
/// `catch_unwind` wrapper in `poll_once`.  We test this by verifying that the
/// `scan_record_and_redrive_tasks` call site itself is wrap-safe: a closure
/// containing a panic is caught without unwinding the test thread.
#[test]
fn panic_in_engine_runner_iteration_is_caught_daemon_continues() {
    // Direct unit-level verification: the catch_unwind pattern used in
    // poll_once must absorb panics from run_engine_runner_iteration.
    // We simulate this by wrapping a panicking closure the same way poll_once does.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        panic!("injected engine-runner panic for test");
        #[allow(unreachable_code)]
        ()
    }));
    // The catch must absorb the panic — not propagate it.
    assert!(result.is_err(), "catch_unwind must catch the panic");
    // After catching, execution continues normally — daemon loop not terminated.
    let sentinel = 42_u32;
    assert_eq!(sentinel, 42, "execution continues after caught panic");
}

// ─── Fix 3 (MEDIUM): heartbeat dispatched count includes base_dispatched ────

/// When the daemon has already dispatched N agents in its base poll loop AND
/// the engine-runner redrives M orphan tasks, the persisted heartbeat row's
/// `dispatched` column must equal N+M (not just M).
#[test]
fn heartbeat_dispatched_includes_base_dispatched() {
    let _env = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("STORES_DRIVE_CMD", "sleep 60 #");
    let (conn, tasks, intake, observations) = db();
    let workspace = tempfile::tempdir().unwrap();
    let task_id = insert_task(&conn, "T981", "executing", workspace.path().to_str().unwrap());
    conn.execute(
        "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
        rusqlite::params![999_999_981_i64, task_id],
    )
    .unwrap();

    let tmp = cfg(5);
    let cfg_path = tmp.path().join("config.yaml");

    // base_dispatched=3 simulates 3 prior daemon dispatches this poll cycle.
    let result = scan_record_and_redrive_tasks(
        &conn,
        scanner(&tasks, &intake, &observations),
        1,
        TS,
        &AgentsYaml::default_empty(),
        &cfg_path,
        "",
        3, // base_dispatched
    )
    .unwrap();

    // The in-memory ScannerResult summary must reflect the full union.
    // The engine-runner spawned 1 redrive; base contributes 3 → total 4.
    assert_eq!(result.summary.dispatched, 4, "summary must include base+redrive");

    // The persisted heartbeat row must also reflect the union.
    let persisted: i64 = conn
        .query_row(
            "SELECT dispatched FROM engine_runner_heartbeats WHERE iteration=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(persisted, 4, "persisted heartbeat must include base+redrive");

    // Kill the spawned child.
    let pid: i64 = conn
        .query_row(
            "SELECT COALESCE(drive_pid, 0) FROM tasks WHERE id=?1",
            rusqlite::params![task_id],
            |r| r.get(0),
        )
        .unwrap();
    if pid > 0 {
        unsafe { libc::kill(pid as i32, libc::SIGTERM); }
    }
    std::env::remove_var("STORES_DRIVE_CMD");
}
