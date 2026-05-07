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
    let tmp = tempfile::tempdir().expect("tmpdir");
    let help = Command::new(bin())
        .current_dir(tmp.path())
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
    assert!(
        bin.exists(),
        "CARGO_BIN_EXE_stores must point at a built binary"
    );

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
    // T040: override the daemon-epoch gate so the in-lifetime detection
    // still fires for this test's hard-coded `claimed_at` (2026-01-01).
    // Without the override, run_daemon would capture today's timestamp as
    // the epoch and the silent-zombie scan would (correctly) skip the row
    // as a pre-existing zombie from a prior daemon lifetime.
    let output = Command::new(&bin)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(workspace)
        .env("STORES_DAEMON_EPOCH", "1970-01-01T00:00:00Z")
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
    assert_eq!(
        cnt, 1,
        "exactly one to_status='blocked' history row must land"
    );
}

/// L141 -> L134: end-to-end regression for the closed-ok-lock dead-PID detection path.
///
/// The L134/T050 contract replaces T049's open-lock approach with typed
/// terminal reasons. After spawn the lock closes immediately with
/// `last_status='ok'`, `terminal_reason='ok'`,
/// `postcondition_id='drive_pid_recorded_or_terminal'`, and `drive_pid` is
/// set on the tasks row. A drive that dies after spawn but before any submit
/// is still detectable: the watchdog JOINs tasks + dispatch_locks filtering
/// on `terminal_reason != 'silent_zombie'` (not on `finished_at IS NULL`),
/// and detects the dead PID via `tasks.drive_pid` + in-cycle status.
///
/// Proves the key safety property: a closed-ok lock does NOT mean a dead
/// post-spawn drive escapes watchdog detection. Same observable outcome
/// as T049 (row blocked with `drive_failed:silent_zombie_pid_dead`), but
/// via typed control-plane shape rather than open-lock state.
///
/// Reproduces the T045 conditions:
///   1. Auto-drive fires on a planning row → spawns a long-lived stub.
///   2. Test SIGKILLs the stub (no submit ever lands).
///   3. Second `agents run --once` → watchdog detects dead PID via
///      `drive_pid` + typed lock state → flips row to `blocked` with
///      `blocked_reason='drive_failed:silent_zombie_pid_dead'`.
#[test]
fn auto_drive_dead_pid_post_spawn_flips_to_blocked_e2e() {
    let bin = bin();
    assert!(
        bin.exists(),
        "CARGO_BIN_EXE_stores must point at a built binary"
    );

    let tmp = tempfile::tempdir().expect("tmpdir");
    let workspace = tmp.path();
    let stores_dir = workspace.join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();

    // ---- 1. .stores/db.sqlite with substrate + tasks + observations DDL ----
    let db_path = stores_dir.join("db.sqlite");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        install_store_ddl(&conn, "tasks");
        install_store_ddl(&conn, "observations");
    }
    std::fs::write(stores_dir.join("manifest.yaml"), "stores: []\n").unwrap();

    // ---- 2. agents.yaml with the auto-drive subscriber ----
    let agents_yaml = "agents:\n\
                       \x20\x20- name: auto-drive\n\
                       \x20\x20\x20\x20subscribes_to:\n\
                       \x20\x20\x20\x20\x20\x20- store: tasks\n\
                       \x20\x20\x20\x20\x20\x20\x20\x20transition:\n\
                       \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20from: \"\"\n\
                       \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20to: planning\n\
                       \x20\x20\x20\x20\x20\x20\x20\x20predicate:\n\
                       \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20op: \"!=\"\n\
                       \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20left: \"$workspace_path\"\n\
                       \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20right: \"\"\n\
                       \x20\x20\x20\x20command: \"builtin:auto-drive\"\n\
                       \x20\x20\x20\x20claim_window_secs: 300\n\
                       \x20\x20\x20\x20retry_policy:\n\
                       \x20\x20\x20\x20\x20\x20max_attempts: 1\n\
                       \x20\x20\x20\x20\x20\x20backoff: linear\n";
    std::fs::write(stores_dir.join("agents.yaml"), agents_yaml).unwrap();

    // ---- 3. Seed a planning task with workspace_path + history row ----
    let now_iso = "2026-05-04T00:00:00Z";
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, \
                            contract, created_at, updated_at, created_by, updated_by) \
         VALUES ('T849', 'planning', 'auto-drive-zombie', 'adz', 'feat/adz', ?1, \
                 '{\"done_when\":\"x\",\"scope_in\":\"y\",\"scope_out\":\"z\"}', \
                 ?2, ?2, 'ai_autonomous', 'ai_autonomous')",
        rusqlite::params![workspace.to_str().unwrap(), now_iso],
    )
    .unwrap();
    let row_id: i64 = conn
        .query_row("SELECT id FROM tasks WHERE display_id='T849'", [], |r| {
            r.get(0)
        })
        .unwrap();
    conn.execute(
        "INSERT INTO transition_history \
         (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
         VALUES ('tasks', ?1, 'T849', '', 'planning', 'submit', \
                 'ai_autonomous', ?2)",
        rusqlite::params![row_id, now_iso],
    )
    .unwrap();
    // Sentinel marker so the daemon's starting-line seeder skips re-seeding
    // (agent_has_been_seeded keys on agent_name). Without this, the seeder
    // would mark our just-inserted T849 history row as `skip-historical` and
    // poll_once's try_claim would lose the UNIQUE race, blocking dispatch.
    conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, \
          claimed_at, claimed_by, last_status, finished_at) \
         VALUES ('tasks', 999999, 'SENTINEL', 'auto-drive', 0, \
                 ?1, 'starting-line-marker', 'skip-historical', ?1)",
        rusqlite::params![now_iso],
    )
    .unwrap();
    drop(conn);

    // ---- 4. Long-lived stub so first --once spawns + records pid (alive) ----
    // sleep 600 keeps the grandchild alive across both --once invocations
    // until we explicitly SIGKILL it. The stub is invoked with "$@" expanded
    // to the display_id; the literal `#` swallows the trailing arg so `sleep`
    // does not see it.
    let stub_cmd = "sleep 600 #";

    // ---- 5. First --once: dispatch auto-drive, closing lock with typed ok (L141 -> L134) ----
    let out1 = Command::new(&bin)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(workspace)
        .env("STORES_DRIVE_CMD", stub_cmd)
        // T040 epoch override so the watchdog gate doesn't filter the row.
        .env("STORES_DAEMON_EPOCH", "1970-01-01T00:00:00Z")
        .output()
        .expect("invoke daemon (1)");
    assert!(
        out1.status.success(),
        "first agents run --once must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&out1.stderr)
    );

    // After dispatch, BEFORE we kill the stub: lock is CLOSED with typed ok
    // (L141 -> L134 invariant: lock closes immediately once
    // drive_pid_recorded_or_terminal postcondition passes), and drive_pid
    // is recorded on the tasks row.
    let conn = Connection::open(&db_path).unwrap();
    let (finished_at, last_status, terminal_reason, postcondition_id, drive_pid): (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
    ) = conn
        .query_row(
            "SELECT dl.finished_at, dl.last_status, dl.terminal_reason, dl.postcondition_id, t.drive_pid \
             FROM dispatch_locks dl JOIN tasks t ON t.id = dl.row_id \
             WHERE t.display_id='T849' AND dl.agent_name='auto-drive'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert!(
        finished_at.is_some(),
        "L141 -> L134: auto-drive lock must close once drive_pid_recorded_or_terminal passes (got finished_at={:?})",
        finished_at
    );
    assert_eq!(
        last_status.as_deref(),
        Some("ok"),
        "L141 -> L134: last_status='ok' on typed-clean close",
    );
    assert_eq!(
        terminal_reason.as_deref(),
        Some("ok"),
        "L141 -> L134: terminal_reason='ok' when postcondition passes",
    );
    assert_eq!(
        postcondition_id.as_deref(),
        Some("drive_pid_recorded_or_terminal"),
        "L141 -> L134: postcondition_id stamped on the lock row",
    );
    let pid = drive_pid.expect("drive_pid must be recorded after spawn");
    assert!(pid > 0, "drive_pid must be > 0; got {pid}");
    drop(conn);

    // ---- 6. SIGKILL the stub before any submit lands ----
    // The stub is `sleep 600`, reparented to PID 1 by the spawn double-fork.
    // SIGKILL guarantees the process is gone before the watchdog sweep.
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    // Brief wait for the kernel to reap. The watchdog uses kill(pid, 0)
    // (`pid_is_alive`); after SIGKILL + reaping, that returns ESRCH.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline && unsafe { libc::kill(pid as i32, 0) } == 0 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // Back-date drive_started_at past ZOMBIE_GRACE_SECS (10s) so the watchdog
    // HAVING cutoff fires deterministically without sleeping. Under L134 the
    // watchdog checks `drive_started_at < cutoff`; a freshly-spawned stub
    // would fall inside the grace window and be skipped. This is test-fixture
    // control only — it does not alter the safety property under test.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE tasks SET drive_started_at = '2026-01-01T00:00:00Z' \
             WHERE display_id = 'T849'",
            [],
        )
        .unwrap();
    }

    // ---- 7. Second --once: watchdog detects closed-ok-lock dead-PID (L141 -> L134) ----
    let out2 = Command::new(&bin)
        .args(["agents", "run", "--once", "--poll-interval", "0.05"])
        .current_dir(workspace)
        .env("STORES_DAEMON_EPOCH", "1970-01-01T00:00:00Z")
        .output()
        .expect("invoke daemon (2)");
    assert!(
        out2.status.success(),
        "second agents run --once must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&out2.stderr)
    );

    // ---- 8. Assertions: row blocked with structured silent-zombie reason ----
    let conn = Connection::open(&db_path).unwrap();
    let (status, blocked_reason): (String, Option<String>) = conn
        .query_row(
            "SELECT status, blocked_reason FROM tasks WHERE display_id='T849'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        status, "blocked",
        "L141 -> L134: row must flip to 'blocked' after watchdog detects dead PID; got '{status}'"
    );
    assert_eq!(
        blocked_reason.as_deref(),
        Some("drive_failed:silent_zombie_pid_dead"),
        "L141 -> L134: blocked_reason must carry the silent-zombie suffix"
    );

    // Lock terminal_reason updated to 'silent_zombie' by watchdog.
    let (finished_after, terminal_reason_after): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT finished_at, terminal_reason FROM dispatch_locks \
             WHERE display_id='T849' AND agent_name='auto-drive'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(
        finished_after.is_some(),
        "L141 -> L134: lock must remain closed after the silent-zombie flip"
    );
    assert_eq!(
        terminal_reason_after.as_deref(),
        Some("silent_zombie"),
        "L141 -> L134: watchdog updates terminal_reason to 'silent_zombie' after flip"
    );
}

/// T067 A1-strict: dead auto-drive handoff at in_review with pending
/// next_agent=wrap is re-dispatched by the daemon watchdog; wrap_log is
/// populated via the real compute_submit_wrap path, NOT via direct DB mutation.
///
/// A1-strict semantics (pi ruling): wrap_log is provenance, NOT a sentinel.
/// next_agent IS NOT NULL is the sole "pending work" signal. For in_review,
/// the schema always yields next_agent=Some("wrap") until the task transitions
/// out of in_review (human accept/reject). Therefore:
///
/// - The daemon faithfully re-dispatches wrap on every sweep while the task
///   remains in_review (next_agent IS NOT NULL).
/// - The dispatch_lock does NOT close with terminal_reason='ok' while the
///   task is still in_review — it stays in_flight (re-opened each cycle).
/// - The lock only reaches terminal_reason='ok' after the task leaves
///   in_review (accepted/rejected), at which point next_agent IS NULL.
///
/// This test proves:
///   (a) wrap fires at least once (wrap_log contains the submitted summary).
///   (b) drive_pid is updated (new drive was spawned by watchdog).
///   (c) task stays in_review (awaiting human accept/reject).
///   (d) dispatch_lock is in-flight (NOT closed) — daemon keeps sweeping.
///
/// (pi ruling r2 MAJOR 2: wrap_log must be populated via compute_submit_wrap,
/// not direct DB mutation. wrap_log provenance assertion remains valid; only
/// the "terminal_reason='ok'" assertion is removed as incompatible with A1.)
#[test]
fn pending_wrap_handoff_redispatched_by_agents_run_once_e2e() {
    let bin = bin();
    let tmp = tempfile::tempdir().expect("tmpdir");
    let workspace = tmp.path();
    let stores_dir = workspace.join(".stores");
    std::fs::create_dir_all(&stores_dir).unwrap();
    let db_path = stores_dir.join("db.sqlite");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        install_store_ddl(&conn, "tasks");
        install_store_ddl(&conn, "observations");
    }
    // Write a manifest that registers the bundled tasks store so that the
    // subprocess `stores tasks submit-wrap` can resolve the schema.
    std::fs::write(
        stores_dir.join("manifest.yaml"),
        "stores:\n- name: tasks\n  schema_path: bundled:tasks\n  installed_at: 2026-01-01T00:00:00Z\n  table_name: tasks\n  scope: repo\n",
    )
    .unwrap();

    let stale = "2026-01-01T00:00:00Z";
    let conn = Connection::open(&db_path).unwrap();
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, tier_hint, workspace_path, \
                            contract, drive_pid, drive_started_at, wrap_log, \
                            created_at, updated_at, created_by, updated_by) \
         VALUES ('T967', 'in_review', 'pending-wrap', 'pending-wrap', 'feat/wrap', 'T2', ?1, \
                 '{\"done_when\":\"x\",\"scope_in\":\"y\",\"scope_out\":\"z\"}', \
                 ?2, ?3, NULL, ?3, ?3, 'framework', 'framework')",
        rusqlite::params![workspace.to_str().unwrap(), dead_pid(), stale],
    )
    .unwrap();
    let row_id: i64 = conn
        .query_row("SELECT id FROM tasks WHERE display_id='T967'", [], |r| {
            r.get(0)
        })
        .unwrap();
    conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, \
          last_status, finished_at, terminal_reason, postcondition_id, postcondition_args) \
         VALUES ('tasks', ?1, 'T967', 'auto-drive', 1, ?2, 'dead-drive', \
                 'ok', ?2, 'ok', 'drive_pid_recorded_or_terminal', \
                 '{\"display_id\":\"T967\",\"store\":\"tasks\"}')",
        rusqlite::params![row_id, stale],
    )
    .unwrap();
    drop(conn);

    // STORES_DRIVE_CMD: invokes the real `stores tasks submit-wrap` verb so that
    // wrap_log is populated via compute_submit_wrap (NOT via direct DB mutation).
    // Uses a mktemp file to pass the summary because the detached grandchild has
    // stdin closed (spawn_detached_drive calls libc::close(STDIN_FILENO)).
    // The script receives the display_id as $1 via the auto-drive-stub wrapper.
    let bin_path = bin.to_str().expect("bin path is UTF-8");
    let drive_cmd = format!(
        r#"f=$(mktemp) && printf 'auto wrap' > "$f" && {bin_path} tasks submit-wrap "$1" --summary-from-file "$f"; rm -f "$f" #"#
    );
    // Use enough iterations and poll-interval for the detached subprocess (a full
    // `stores` binary invocation) to complete before Pass 2 checks the PID.
    let output = Command::new(&bin)
        .args([
            "agents",
            "run",
            "--max-iters",
            "10",
            "--poll-interval",
            "0.2",
        ])
        .current_dir(workspace)
        .env("STORES_DAEMON_EPOCH", "1970-01-01T00:00:00Z")
        .env("STORES_DRIVE_CMD", &drive_cmd)
        .output()
        .expect("invoke daemon");
    assert!(
        output.status.success(),
        "agents run --max-iters 10 failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // A1-strict ACs (pi ruling): next_agent IS NOT NULL is the sole pending-work
    // signal; wrap_log is provenance only.
    let conn = Connection::open(&db_path).unwrap();
    let (status, terminal_reason, finished_at, pid, wrap_log): (
        String,
        Option<String>,
        Option<String>,
        i64,
        Option<String>,
    ) = conn
        .query_row(
            "SELECT t.status, dl.terminal_reason, dl.finished_at, t.drive_pid, t.wrap_log \
         FROM tasks t JOIN dispatch_locks dl ON dl.row_id=t.id \
         WHERE t.display_id='T967' AND dl.agent_name='auto-drive'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    // (a) wrap_log populated via compute_submit_wrap (provenance assertion — valid under A1).
    assert!(
        wrap_log
            .as_deref()
            .unwrap_or("")
            .contains("auto wrap"),
        "wrap_log must be populated via compute_submit_wrap after wrap fires; got {wrap_log:?}"
    );
    // (b) new drive_pid was recorded (watchdog re-dispatched).
    assert!(
        pid > 0 && pid != dead_pid(),
        "watchdog must record a new drive_pid; got pid={pid}"
    );
    // (c) task stays in_review — awaiting human accept/reject.
    assert_eq!(status, "in_review");
    // (d) Stub-path lock state: the watchdog re-dispatched via STORES_DRIVE_CMD
    // (not the real `stores tasks drive` binary), so force_close_auto_drive_lock_ok
    // was NOT called. After mark_pending_handoff_lock the lock is in_flight
    // (finished_at=NULL, terminal_reason=NULL). The stub (submit-wrap) does not
    // close the lock, so it remains open.
    //
    // Note: r6 design explicitly allows `terminal_reason='ok'` + `finished_at IS NOT NULL`
    // on in_review rows when force_close_auto_drive_lock_ok fires (real `tasks drive` path).
    // That case is distinct from this stub path and is tested separately in the
    // `wrap_force_close_watchdog_no_redispatch` integration test.
    assert!(
        finished_at.is_none(),
        "stub path: lock must stay in-flight (finished_at=None) after watchdog re-dispatch \
         via STORES_DRIVE_CMD stub; got finished_at={finished_at:?}"
    );
    assert!(
        terminal_reason.is_none(),
        "stub path: lock terminal_reason must be NULL (in_flight:pending_next) after watchdog \
         re-dispatch; got terminal_reason={terminal_reason:?}"
    );
}
