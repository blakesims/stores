use rusqlite::Connection;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
#[cfg(debug_assertions)]
use std::sync::atomic::Ordering;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::TransitionEdge;
use stores::flow::builtins::{external_review, DispatchCtx};
use stores::flow::engine_runner::{
    reconcile_pending_external_review_dispatch, ExternalReviewDispatchOutcome,
};
use stores::flow::{AgentEntry, AgentsYaml, BackoffKind, RetryPolicy, Subscription};
use stores::schema::Schema;

fn install_db(conn: &Connection) {
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    for name in ["tasks", "external_reviews"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
    }
}

fn git_workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "t@example.com"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "T"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::fs::write(tmp.path().join("README.md"), "base\n").unwrap();
    std::process::Command::new("git")
        .args(["add", "README.md"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::process::Command::new("git")
        .args(["commit", "-m", "base"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    std::fs::write(tmp.path().join("README.md"), "base\nhead\n").unwrap();
    tmp
}

fn insert_task(conn: &Connection, workspace: &Path, status: &str) {
    conn.execute(
        "INSERT INTO tasks (display_id,status,title,slug,workspace_path,branch,tier_hint,contract,plan,cycles,wrap_log,current_phase,current_cycle,created_at,updated_at,created_by,updated_by)
         VALUES ('T900',?1,'Review task','review-task',?2,'main','T2',?3,?4,'[]',?5,1,1,'2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')",
        rusqlite::params![
            status,
            workspace.display().to_string(),
            json!({"done_when":"done","scope_in":"in","scope_out":"out"}).to_string(),
            json!({"phases":[{"name":"p1"}]}).to_string(),
            json!([{"executive_summary":"wrapped"}]).to_string(),
        ],
    ).unwrap();
}

fn insert_review(conn: &Connection, id: &str, task: &str, status: &str, attempt: i64) -> i64 {
    conn.execute(
        "INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by)
         VALUES (?1,?2,?3,?4,'external_review','2026-05-07T00:00:00Z','2026-05-07T00:00:00Z','test','test')",
        rusqlite::params![id, status, task, attempt],
    ).unwrap();
    conn.last_insert_rowid()
}

fn shim(dir: &Path, body: &str) -> PathBuf {
    let p = dir.join("codex-shim.sh");
    std::fs::write(&p, body).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn cfg(dir: &Path, shim: &Path, max_parallel: u32) -> PathBuf {
    let p = dir.join("config.yaml");
    std::fs::write(
        &p,
        format!(
            "review:\n  runner: codex\n  max_parallel: {max_parallel}\n  timeout_secs: 5\ncodex:\n  command: {}\n  args: []\n",
            shim.display()
        ),
    )
    .unwrap();
    p
}

fn agents() -> AgentsYaml {
    AgentsYaml {
        agents: vec![AgentEntry {
            name: "external-review".to_string(),
            subscribes_to: vec![Subscription {
                store: "external_reviews".to_string(),
                transition: TransitionEdge {
                    from: "".to_string(),
                    to: "pending".to_string(),
                },
                predicate: None,
            }],
            command: "builtin:external-review".to_string(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy {
                max_attempts: 1,
                backoff: BackoffKind::Linear,
            },
            command_args: None,
        }],
        deployment_specialist: None,
    }
}

fn ctx<'a>(conn: &'a Connection, agents: &'a AgentsYaml, cfg: &'a Path) -> DispatchCtx<'a> {
    DispatchCtx {
        conn,
        agents,
        config_path: cfg,
        policies_hash: "",
    }
}

#[test]
fn external_review_daemon_cap_hold_marks_second_pending_visible() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    external_review::visible_status_rows(&conn).unwrap(); // adds runtime cols
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    let cfg = cfg(tmp.path(), &sh, 1);
    insert_review(&conn, "ER001", "T900", "running", 1);
    insert_review(&conn, "ER002", "T901", "pending", 1);
    assert!(!external_review::cap_allows_or_log(&conn, &cfg, "ER002").unwrap());
    let (status, held): (String, String) = conn
        .query_row(
            "SELECT status, held_reason FROM external_reviews WHERE display_id='ER002'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "pending");
    assert_eq!(held, "cap-held");
    assert!(external_review::visible_status_rows(&conn)
        .unwrap()
        .join("\n")
        .contains("external-review task_id=T901 review_attempt_id=ER002"));
}

#[test]
fn external_review_daemon_tooling_failure_holds_with_retry_and_refs() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "in_review");
    insert_review(&conn, "ER003", "T900", "pending", 1);
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: TOOLING_FAILURE'\n");
    let cfg = cfg(tmp.path(), &sh, 1);
    let a = agents();
    let row = json!({"display_id":"ER003"});
    external_review::run(&row, &ctx(&conn, &a, &cfg)).unwrap();
    let (task_status, status, verdict, retry, log): (String, String, String, Option<String>, Option<String>) = conn.query_row(
        "SELECT t.status, er.status, er.verdict, er.next_retry_at, er.log_path FROM external_reviews er JOIN tasks t ON t.display_id=er.task_id WHERE er.display_id='ER003'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    ).unwrap();
    assert_eq!(task_status, "in_review");
    assert_eq!(status, "tooling_held");
    assert_eq!(verdict, "TOOLING_FAILURE");
    assert!(retry.is_some());
    assert!(log.unwrap().contains("codex"));
}

/// `tooling_held` rows with an elapsed `next_retry_at` are promoted back to
/// `pending` at the top of `run()` so they are re-tried on the same iteration.
#[test]
fn external_review_daemon_tooling_failure_retries_after_next_retry_at() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "in_review");
    // Pre-insert a tooling_held row with a next_retry_at that is already in the past.
    insert_review(&conn, "ER010", "T900", "tooling_held", 1);
    // Ensure the runtime columns exist (normally created by run()), then set the
    // past retry timestamp and verdict directly.
    external_review::visible_status_rows(&conn).unwrap();
    conn.execute(
        "UPDATE external_reviews SET next_retry_at='2000-01-01T00:00:00Z', held_reason='prev tooling failure', verdict='TOOLING_FAILURE' WHERE display_id='ER010'",
        [],
    ).unwrap();

    // Dispatch run() for ER010. promote_elapsed_tooling_held() fires first,
    // transitions ER010 → pending, then load_review_row() sees pending and
    // the normal path executes with the PASS shim.
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    let cfg = cfg(tmp.path(), &sh, 1);
    let a = agents();
    let row = json!({"display_id": "ER010"});
    external_review::run(&row, &ctx(&conn, &a, &cfg)).unwrap();

    let (status, verdict): (String, String) = conn.query_row(
        "SELECT status, verdict FROM external_reviews WHERE display_id='ER010'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    // After retry: promoted to pending, then ran to completion with PASS.
    assert_eq!(verdict, "PASS");
    assert_eq!(status, "passed");
}

/// Real concurrent race test: two independent connections call
/// `promote_elapsed_tooling_held` at the same time against the same
/// `tooling_held` row.  Exactly ONE must win the BEGIN IMMEDIATE lock and
/// promote the row; the other must no-op cleanly.
///
/// Sync mechanism (matching T079 r4 / T076 r6 precedent):
///   - `STORES_TEST_PROMOTE_DELAY_MS=150` is set so the first thread sleeps
///     before opening its BEGIN IMMEDIATE transaction.
///   - `PROMOTE_DELAY_HOOK_ENTERED` (AtomicBool, debug_assertions only) is
///     set by the delay hook BEFORE the sleep so the second thread can
///     deterministically know the first is inside the delay window before
///     it too calls `promote_elapsed_tooling_held`.
///
/// Assertions:
///   (a) Exactly ONE row ends up with status=pending.
///   (b) Exactly ONE transition_history record for tooling_held→pending exists.
///   (c) No second history record is inserted by the losing thread.
#[cfg(debug_assertions)]
#[test]
fn external_review_daemon_concurrent_promote_idempotent() {
    // Reset the sentinel before the test.
    stores::flow::builtins::external_review::PROMOTE_DELAY_HOOK_ENTERED
        .store(false, Ordering::SeqCst);

    std::env::set_var("STORES_TEST_PROMOTE_DELAY_MS", "150");

    // Use a temp-file DB so two independent connections can reach the same DB.
    let db_file = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_file.path().to_owned();

    // Setup: install schema + one elapsed tooling_held row via a dedicated
    // setup connection that is dropped before the race threads open theirs.
    {
        let setup_conn = Connection::open(&db_path).unwrap();
        setup_conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        install_db(&setup_conn);
        let ws = git_workspace();
        insert_task(&setup_conn, ws.path(), "in_review");
        insert_review(&setup_conn, "ER020", "T900", "tooling_held", 1);
        external_review::visible_status_rows(&setup_conn).unwrap();
        setup_conn.execute(
            "UPDATE external_reviews SET next_retry_at='2000-01-01T00:00:00Z', held_reason='prev tooling failure', verdict='TOOLING_FAILURE' WHERE display_id='ER020'",
            [],
        ).unwrap();
    } // setup_conn dropped here

    // ── Thread A (first to enter delay hook) ─────────────────────────────────
    let db_path_a = db_path.clone();
    let thread_a = std::thread::spawn(move || {
        let conn = Connection::open(&db_path_a).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.pragma_update(None, "busy_timeout", 5000i64).unwrap();
        external_review::promote_elapsed_tooling_held(&conn).unwrap();
    });

    // ── Main thread (Thread B) ────────────────────────────────────────────────
    // Wait until Thread A signals it has entered the delay hook (it is past its
    // pre-tx point and sleeping in the delay window).
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if stores::flow::builtins::external_review::PROMOTE_DELAY_HOOK_ENTERED
                .load(Ordering::Acquire)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for PROMOTE_DELAY_HOOK_ENTERED signal"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    // Thread A is inside the delay window.  Thread B now calls promote too —
    // it will either win or block on BEGIN IMMEDIATE until Thread A commits.
    let conn_b = Connection::open(&db_path).unwrap();
    conn_b.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn_b.pragma_update(None, "busy_timeout", 5000i64).unwrap();
    external_review::promote_elapsed_tooling_held(&conn_b).unwrap();

    thread_a.join().expect("thread A panicked");

    std::env::remove_var("STORES_TEST_PROMOTE_DELAY_MS");

    // ── Assertions ────────────────────────────────────────────────────────────
    let verify_conn = Connection::open(&db_path).unwrap();

    // (a) Exactly one row must be pending.
    let (status,): (String,) = verify_conn.query_row(
        "SELECT status FROM external_reviews WHERE display_id='ER020'",
        [],
        |r| Ok((r.get(0)?,)),
    ).unwrap();
    assert_eq!(status, "pending", "exactly one thread must promote to pending");

    // (b) Exactly one transition history record for tooling_held→pending.
    let history_count: i64 = verify_conn.query_row(
        "SELECT COUNT(*) FROM transition_history \
         WHERE row_id=(SELECT id FROM external_reviews WHERE display_id='ER020') \
           AND from_status='tooling_held' AND to_status='pending'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(
        history_count, 1,
        "exactly ONE tooling_held→pending transition record must exist; got {history_count}"
    );
}

#[test]
fn external_review_daemon_pass_and_revise_update_status_and_revise_routes_task() {
    for (id, verdict, expected_status, expected_task) in [
        ("ER004", "PASS", "passed", "in_review"),
        ("ER005", "REVISE", "revise", "executing"),
    ] {
        let conn = Connection::open_in_memory().unwrap();
        install_db(&conn);
        let ws = git_workspace();
        insert_task(&conn, ws.path(), "in_review");
        insert_review(&conn, id, "T900", "pending", 1);
        let tmp = tempfile::tempdir().unwrap();
        let sh = shim(
            tmp.path(),
            &format!("#!/bin/sh\necho 'VERDICT: {verdict}'\n"),
        );
        let cfg = cfg(tmp.path(), &sh, 1);
        let a = agents();
        let row = json!({"display_id": id});
        external_review::run(&row, &ctx(&conn, &a, &cfg)).unwrap();
        let (status, got_verdict, task_status): (String, String, String) = conn.query_row(
            "SELECT er.status, er.verdict, t.status FROM external_reviews er JOIN tasks t ON t.display_id=er.task_id WHERE er.display_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        ).unwrap();
        assert_eq!(status, expected_status);
        assert_eq!(got_verdict, verdict);
        assert_eq!(task_status, expected_task);
    }
}

// ─── Layer 2 reconciler tests ──────────────────────────────────────────────
//
// These tests verify that the Layer 2 state-driven reconciler dispatches
// pending external_review rows regardless of transition-history seeder state.
// Each test calls `reconcile_pending_external_review_dispatch` directly so the
// Layer 2 path is exercised in isolation.

/// Test: subscriber added AFTER ER row exists (the ER001 production bug).
///
/// Scenario: an external_reviews row was minted (by Layer 1 or any other path)
/// and its `""→pending` transition_history record exists.  Later, the
/// `external-review` subscriber is added to agents.yaml.  The L055/L116 seeder
/// marks the TH row `skip-historical` by inserting a dispatch_locks row with
/// `finished_at` set and `last_status='skip-historical'`.
///
/// Layer 2 MUST treat that skip-historical lock as a "finished" lock (not a
/// live one) because its `finished_at IS NOT NULL`, and MUST dispatch the
/// pending ER row on the next tick.
#[test]
fn layer2_subscriber_added_after_er_row_dispatches_on_tick() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "in_review");

    // Insert ER row at status=pending (simulates Layer 1 backfill).
    let er_row_id = insert_review(&conn, "ER030", "T900", "pending", 1);
    // Insert the ""→pending transition_history record (as created by Layer 1).
    conn.execute(
        "INSERT INTO transition_history (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
         VALUES ('external_reviews', ?1, 'ER030', '', 'pending', 'create-external-review', 'framework', '2026-05-07T00:00:00Z')",
        rusqlite::params![er_row_id],
    ).unwrap();
    let th_id: i64 = conn.last_insert_rowid();

    // Simulate L055 seeder: insert a skip-historical dispatch_lock with
    // finished_at set (i.e., the seeder pre-claimed this TH row as historical).
    let now = "2026-05-07T10:00:00Z";
    conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, \
          claimed_at, claimed_by, last_status, finished_at, \
          daemon_epoch, claim_source, attempt, terminal_reason) \
         VALUES ('external_reviews', ?1, 'ER030', 'external-review', ?2, \
                 ?3, 'starting-line-marker', 'skip-historical', ?3, \
                 '', 'legacy', 0, 'legacy_unknown')",
        rusqlite::params![er_row_id, th_id, now],
    ).unwrap();

    // Verify the skip-historical lock is present and finished_at IS NOT NULL.
    let finished_at: Option<String> = conn.query_row(
        "SELECT finished_at FROM dispatch_locks WHERE display_id='ER030' AND agent_name='external-review'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert!(finished_at.is_some(), "skip-historical lock must have finished_at set");

    // Layer 2 reconciler tick with a PASS shim — should dispatch ER030.
    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    let cfg_path = cfg(tmp.path(), &sh, 2);
    let a = agents();

    let dispatches = reconcile_pending_external_review_dispatch(
        &conn,
        &a,
        &cfg_path,
        "",
    ).unwrap();

    assert_eq!(dispatches.len(), 1, "exactly one ER row should be dispatched");
    assert_eq!(dispatches[0].review_display_id, "ER030");
    assert_eq!(
        dispatches[0].outcome,
        ExternalReviewDispatchOutcome::Dispatched,
        "outcome must be Dispatched, not CapHeld or NoRunner"
    );

    // ER030 must have transitioned out of pending (run() executed).
    let status: String = conn.query_row(
        "SELECT status FROM external_reviews WHERE display_id='ER030'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_ne!(status, "pending", "ER030 must no longer be pending after Layer2 dispatch");
}

/// Test: daemon restart with a pending ER row and no dispatch_lock.
///
/// After a daemon restart, pending ER rows have no dispatch_lock at all (the
/// previous daemon's locks were never closed, or the row was minted via CLI).
/// Layer 2 MUST dispatch these rows.
#[test]
fn layer2_daemon_restart_pending_er_row_dispatches() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "in_review");

    // ER row at pending with NO dispatch_lock at all.
    insert_review(&conn, "ER031", "T900", "pending", 1);

    let lock_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM dispatch_locks WHERE display_id='ER031'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(lock_count, 0, "no dispatch_lock should exist before Layer2 tick");

    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    let cfg_path = cfg(tmp.path(), &sh, 2);
    let a = agents();

    let dispatches = reconcile_pending_external_review_dispatch(
        &conn,
        &a,
        &cfg_path,
        "",
    ).unwrap();

    assert!(!dispatches.is_empty(), "Layer2 must dispatch ER031");
    assert_eq!(
        dispatches[0].outcome,
        ExternalReviewDispatchOutcome::Dispatched
    );

    let status: String = conn.query_row(
        "SELECT status FROM external_reviews WHERE display_id='ER031'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_ne!(status, "pending", "ER031 must be dispatched off pending");
}

/// Test: repeated ticks do NOT duplicate dispatch.
///
/// After Layer 2 dispatches an ER row (transitioning it to passed/tooling_held),
/// a second tick must not re-dispatch it (status is no longer 'pending').
#[test]
fn layer2_repeated_tick_does_not_duplicate_dispatch() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    let ws = git_workspace();
    insert_task(&conn, ws.path(), "in_review");
    insert_review(&conn, "ER032", "T900", "pending", 1);

    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    let cfg_path = cfg(tmp.path(), &sh, 2);
    let a = agents();

    // Tick 1: dispatches ER032.
    let tick1 = reconcile_pending_external_review_dispatch(&conn, &a, &cfg_path, "").unwrap();
    assert_eq!(tick1.len(), 1, "first tick should dispatch one row");
    assert_eq!(tick1[0].outcome, ExternalReviewDispatchOutcome::Dispatched);

    // Tick 2: ER032 is no longer pending; nothing to dispatch.
    let tick2 = reconcile_pending_external_review_dispatch(&conn, &a, &cfg_path, "").unwrap();
    assert!(
        tick2.is_empty(),
        "second tick must not re-dispatch: ER032 is no longer pending"
    );

    // Verify only one transition history record for pending→running exists.
    let running_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transition_history \
         WHERE store='external_reviews' AND from_status='pending' AND to_status='running'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(running_count, 1, "exactly ONE pending→running transition must exist");
}

/// Test: lane cap is respected — reconciler holds when cap full.
///
/// When a review is already running (count_running_reviews = cap), Layer 2
/// must NOT dispatch additional rows; it must mark them cap-held and return
/// the CapHeld outcome.
#[test]
fn layer2_lane_cap_respected_when_at_capacity() {
    let conn = Connection::open_in_memory().unwrap();
    install_db(&conn);
    // Ensure runtime columns exist.
    external_review::visible_status_rows(&conn).unwrap();

    // One review already running — fills the cap=1 lane.
    insert_review(&conn, "ER040", "T901", "running", 1);
    // One review pending — should be held, not dispatched.
    insert_review(&conn, "ER041", "T902", "pending", 1);

    let tmp = tempfile::tempdir().unwrap();
    let sh = shim(tmp.path(), "#!/bin/sh\necho 'VERDICT: PASS'\n");
    // cap=1 so the running ER040 fills the lane.
    let cfg_path = cfg(tmp.path(), &sh, 1);
    let a = agents();

    let dispatches = reconcile_pending_external_review_dispatch(&conn, &a, &cfg_path, "").unwrap();

    // ER041 must appear as CapHeld.
    assert_eq!(dispatches.len(), 1, "ER041 should appear in results (held)");
    assert_eq!(dispatches[0].review_display_id, "ER041");
    assert_eq!(
        dispatches[0].outcome,
        ExternalReviewDispatchOutcome::CapHeld,
        "Layer2 must hold ER041 when cap is full"
    );

    // ER041 must remain at status=pending (not transitioned).
    let status: String = conn.query_row(
        "SELECT status FROM external_reviews WHERE display_id='ER041'",
        [],
        |r| r.get(0),
    ).unwrap();
    assert_eq!(status, "pending", "cap-held row must remain at pending");
}
