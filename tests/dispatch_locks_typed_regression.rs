//! T050 Phase 5 — typed dispatch_locks regressions for L087/L107/L116/L122/L141.

use rusqlite::Connection;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::TransitionEdge;
use stores::flow::policies_yaml::PoliciesYaml;
use stores::flow::{AgentEntry, AgentsYaml, BackoffKind, RetryPolicy, Subscription};
use stores::handlers::agents_run::{poll_once, seed_starting_line};
use stores::schema::Schema;

fn env_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

struct EnvVarGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

fn fresh_db(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    for name in ["tasks", "observations", "gate"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
    }
    conn
}

fn empty_policies() -> PoliciesYaml {
    PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    }
}

fn cfg_path(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join("config.yaml")
}

fn agent(name: &str, store: &str, from: &str, to: &str, command: &str) -> AgentEntry {
    AgentEntry {
        name: name.to_string(),
        subscribes_to: vec![Subscription {
            store: store.to_string(),
            transition: TransitionEdge {
                from: from.to_string(),
                to: to.to_string(),
            },
            integration_step: None,
            predicate: None,
        }],
        command: command.to_string(),
        claim_window_secs: 300,
        retry_policy: RetryPolicy {
            max_attempts: 3,
            backoff: BackoffKind::Linear,
        },
        command_args: None,
    }
}

fn agents(one: AgentEntry) -> AgentsYaml {
    AgentsYaml {
        agents: vec![one],
        deployment_specialist: None,
    }
}

fn insert_history(
    conn: &Connection,
    store: &str,
    row_id: i64,
    display_id: &str,
    from: &str,
    to: &str,
) -> i64 {
    conn.execute(
        "INSERT INTO transition_history \
         (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'test', 'test', '2026-05-06T00:00:00Z')",
        rusqlite::params![store, row_id, display_id, from, to],
    )
    .unwrap();
    conn.last_insert_rowid()
}

fn insert_ready_obs(conn: &Connection, display_id: &str) -> i64 {
    let ic = json!({
        "contract_state": "ready",
        "objective": "typed regression",
        "type": "work",
        "in_scope": ["x"],
        "out_of_scope": ["y"],
        "acceptance": ["z"],
        "tier_hint": "T3",
        "approved_by": "pi",
        "approved_at": "2026-05-06T00:00:00Z"
    });
    conn.execute(
        "INSERT INTO observations \
         (display_id, status, summary, source, priority, captured_at, captured_week, intent_contract, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'ready', 'typed regression', 'dev', 'normal', '2026-05-06T00:00:00Z', 'w19-d3', ?2, '2026-05-06T00:00:00Z', '2026-05-06T00:00:00Z', 'human', 'human')",
        rusqlite::params![display_id, ic.to_string()],
    ).unwrap();
    let id = conn.last_insert_rowid();
    insert_history(conn, "observations", id, display_id, "confirmed", "ready");
    id
}

fn insert_obs_at(conn: &Connection, display_id: &str, status: &str) -> i64 {
    conn.execute(
        "INSERT INTO observations \
         (display_id, status, summary, body, source, priority, captured_at, captured_week, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, ?2, 'typed investigator regression', 'body for investigator', 'dev', 'normal', '2026-05-06T00:00:00Z', 'w19-d3', '2026-05-06T00:00:00Z', '2026-05-06T00:00:00Z', 'human', 'human')",
        rusqlite::params![display_id, status],
    ).unwrap();
    conn.last_insert_rowid()
}

fn investigator_agent(max_attempts: u32) -> AgentEntry {
    let mut a = agent(
        "investigator",
        "observations",
        "open",
        "needs_investigation",
        "builtin:investigator",
    );
    a.retry_policy.max_attempts = max_attempts;
    a
}

fn valid_investigator_envelope() -> String {
    json!({
        "evidence": ["typed dispatch lock evidence"],
        "duplicate_candidates": [],
        "confidence": "high",
        "proposed_tier": "T2",
        "grill_question": "What changed?"
    })
    .to_string()
}

struct InvestigatorCmdGuard {
    prev: Option<String>,
}

impl InvestigatorCmdGuard {
    fn set_success(invocations_path: &Path) -> Self {
        let prev = std::env::var("STORES_INVESTIGATOR_CMD").ok();
        let json = valid_investigator_envelope().replace('\'', r"'\''");
        let path = invocations_path.to_string_lossy().replace('\'', r"'\''");
        std::env::set_var(
            "STORES_INVESTIGATOR_CMD",
            format!("printf x >> '{}'; printf '%s' '{}'", path, json),
        );
        Self { prev }
    }

    fn set_failure() -> Self {
        let prev = std::env::var("STORES_INVESTIGATOR_CMD").ok();
        std::env::set_var(
            "STORES_INVESTIGATOR_CMD",
            "printf 'rate_limit: retry later' >&2; exit 17",
        );
        Self { prev }
    }
}

impl Drop for InvestigatorCmdGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("STORES_INVESTIGATOR_CMD", v),
            None => std::env::remove_var("STORES_INVESTIGATOR_CMD"),
        }
    }
}

fn insert_task(
    conn: &Connection,
    display_id: &str,
    status: &str,
    workspace_path: Option<&str>,
    drive_pid: Option<i64>,
) -> i64 {
    conn.execute(
        "INSERT INTO tasks \
         (display_id, status, title, slug, branch, tier_hint, workspace_path, drive_pid, contract, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, ?2, 'typed regression', 'typed-regression', 'feat/typed-regression', 'T2', ?3, ?4, \
                 '{\"done_when\":\"x\",\"scope_in\":\"y\",\"scope_out\":\"z\"}', \
                 '2026-05-06T00:00:00Z', '2026-05-06T00:00:00Z', 'test', 'test')",
        rusqlite::params![display_id, status, workspace_path, drive_pid],
    ).unwrap();
    conn.last_insert_rowid()
}

#[test]
fn l087_back_to_back_ratifies_stamp_ok_typed_postconditions() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = fresh_db(&tmp.path().join("db.sqlite"));
    let a = agents(agent(
        "auto-promote",
        "observations",
        "confirmed",
        "ready",
        "builtin:auto-promote",
    ));
    insert_ready_obs(&conn, "L087A");
    insert_ready_obs(&conn, "L087B");

    let dispatched = poll_once(
        &conn,
        &a,
        &empty_policies(),
        &cfg_path(&tmp),
        "l087",
        "epoch-l087",
    )
    .unwrap();
    assert_eq!(dispatched, 2);

    let mut stmt = conn
        .prepare(
            "SELECT display_id, postcondition_id, terminal_reason, last_status \
         FROM dispatch_locks WHERE agent_name='auto-promote' ORDER BY display_id",
        )
        .unwrap();
    let rows: Vec<(String, String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 2);
    for (_display_id, postcondition_id, terminal_reason, last_status) in rows {
        assert_eq!(postcondition_id, "task_exists_for_linked_observation");
        assert_eq!(terminal_reason, "ok");
        assert_eq!(last_status, "ok");
    }
}

#[test]
fn l107_zero_exit_without_convergence_demotes_to_error_via_postcondition() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = fresh_db(&tmp.path().join("db.sqlite"));
    let row_id = insert_task(&conn, "T107", "planning", None, None);
    insert_history(&conn, "tasks", row_id, "T107", "", "planning");
    let a = agents(agent(
        "auto-scaffold",
        "tasks",
        "",
        "planning",
        "builtin:auto-scaffold",
    ));

    let n = poll_once(
        &conn,
        &a,
        &empty_policies(),
        &cfg_path(&tmp),
        "l107",
        "epoch-l107",
    )
    .unwrap();
    assert_eq!(n, 1);
    let (postcondition_id, terminal_reason, last_status): (String, String, String) = conn.query_row(
        "SELECT postcondition_id, terminal_reason, last_status FROM dispatch_locks WHERE row_id=?1 AND agent_name='auto-scaffold'",
        rusqlite::params![row_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(postcondition_id, "task_workspace_exists");
    assert_eq!(terminal_reason, "error");
    assert!(last_status.contains("postcondition task_workspace_exists failed"));
}

#[test]
fn l116_seeded_rows_are_legacy_but_live_claim_is_try_claim_attempt_zero_same_epoch() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = fresh_db(&tmp.path().join("db.sqlite"));
    let historical = insert_task(&conn, "T116A", "in_review", None, None);
    insert_history(&conn, "tasks", historical, "T116A", "ready", "in_review");
    let a = agents(agent(
        "l116-agent",
        "tasks",
        "ready",
        "in_review",
        "/bin/true",
    ));

    let seeded = seed_starting_line(&conn, &a, i64::MAX).unwrap();
    assert_eq!(seeded, 1);
    let seed_shape: (String, Option<i64>) = conn.query_row(
        "SELECT claim_source, attempt FROM dispatch_locks WHERE row_id=?1 AND agent_name='l116-agent'",
        rusqlite::params![historical],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ).unwrap();
    assert_eq!(seed_shape.0, "legacy");

    let live = insert_task(&conn, "T116B", "in_review", None, None);
    insert_history(&conn, "tasks", live, "T116B", "ready", "in_review");
    let n = poll_once(
        &conn,
        &a,
        &empty_policies(),
        &cfg_path(&tmp),
        "l116",
        "epoch-l116",
    )
    .unwrap();
    assert_eq!(n, 1);
    let live_shape: (String, i64, String) = conn.query_row(
        "SELECT claim_source, attempt, daemon_epoch FROM dispatch_locks WHERE row_id=?1 AND agent_name='l116-agent'",
        rusqlite::params![live],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).unwrap();
    assert_eq!(
        live_shape,
        ("try_claim".to_string(), 1, "epoch-l116".to_string())
    );
}

#[test]
fn l122_silent_zombie_is_typed_and_retry_requires_next_retry_at() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = fresh_db(&tmp.path().join("db.sqlite"));
    let row_id = insert_task(
        &conn,
        "T122",
        "planning",
        Some(tmp.path().to_str().unwrap()),
        None,
    );
    let th_id = insert_history(&conn, "tasks", row_id, "T122", "", "planning");
    conn.execute(
        "INSERT INTO dispatch_locks \
         (store, row_id, display_id, agent_name, transition_id, claimed_at, claimed_by, attempts, attempt, finished_at, last_status, terminal_reason, next_retry_at) \
         VALUES ('tasks', ?1, 'T122', 'auto-drive', ?2, '2000-01-01T00:00:00Z', 'dead-daemon', 1, 1, '2000-01-01T00:00:01Z', 'drive_failed:silent_zombie_pid_dead', 'silent_zombie', NULL)",
        rusqlite::params![row_id, th_id],
    ).unwrap();
    let a = agents(agent(
        "auto-drive",
        "tasks",
        "",
        "planning",
        "builtin:auto-drive",
    ));
    let n1 = poll_once(&conn, &a, &empty_policies(), &cfg_path(&tmp), "l122", "").unwrap();
    assert_eq!(
        n1, 0,
        "terminal silent_zombie with NULL next_retry_at is neither watchdog-refired nor retried"
    );

    conn.execute(
        "UPDATE dispatch_locks SET next_retry_at='2000-01-01T00:00:02Z' WHERE row_id=?1",
        rusqlite::params![row_id],
    )
    .unwrap();
    let _guard = env_lock().lock().unwrap();
    let _drive_cmd = EnvVarGuard::set("STORES_DRIVE_CMD", "");
    let n2 = poll_once(&conn, &a, &empty_policies(), &cfg_path(&tmp), "l122", "").unwrap();
    assert_eq!(
        n2, 1,
        "non-null elapsed next_retry_at makes silent_zombie retry-eligible"
    );
    let (terminal_reason, attempt): (String, i64) = conn
        .query_row(
            "SELECT terminal_reason, attempt FROM dispatch_locks WHERE row_id=?1",
            rusqlite::params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(matches!(
        terminal_reason.as_str(),
        "error" | "silent_zombie"
    ));
    assert!(
        attempt >= 2,
        "elapsed next_retry_at must move the row beyond its original attempt; got {attempt}"
    );
}

#[test]
fn l141_auto_drive_spawn_pending_next_stays_in_flight() {
    let tmp = tempfile::tempdir().unwrap();
    let conn = fresh_db(&tmp.path().join("db.sqlite"));
    let row_id = insert_task(
        &conn,
        "T141",
        "planning",
        Some(tmp.path().to_str().unwrap()),
        None,
    );
    insert_history(&conn, "tasks", row_id, "T141", "", "planning");
    let a = agents(agent(
        "auto-drive",
        "tasks",
        "",
        "planning",
        "builtin:auto-drive",
    ));

    let _guard = env_lock().lock().unwrap();
    let _drive_cmd = EnvVarGuard::set("STORES_DRIVE_CMD", "");
    let n = poll_once(
        &conn,
        &a,
        &empty_policies(),
        &cfg_path(&tmp),
        "l141",
        "epoch-l141",
    )
    .unwrap();
    assert_eq!(n, 1);
    let (postcondition_id, terminal_reason, last_status, finished_at, drive_pid): (String, Option<String>, String, Option<String>, Option<i64>) = conn.query_row(
        "SELECT dl.postcondition_id, dl.terminal_reason, dl.last_status, dl.finished_at, t.drive_pid \
         FROM dispatch_locks dl JOIN tasks t ON t.id=dl.row_id \
         WHERE dl.row_id=?1 AND dl.agent_name='auto-drive'",
        rusqlite::params![row_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    ).unwrap();
    assert_eq!(postcondition_id, "drive_pid_recorded_or_terminal");
    assert_eq!(drive_pid, None);
    assert!(
        terminal_reason.is_none(),
        "terminal_reason must be NULL while next_agent is pending"
    );
    assert!(
        finished_at.is_none(),
        "pending next_agent keeps lock in-flight"
    );
    assert_eq!(last_status, "in_flight:pending_next");
}

#[test]
fn investigator_repeated_polls_keep_single_dispatch_lock_ok() {
    let _guard = env_lock().lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let conn = fresh_db(&tmp.path().join("db.sqlite"));
    let row_id = insert_obs_at(&conn, "L650", "needs_investigation");
    insert_history(
        &conn,
        "observations",
        row_id,
        "L650",
        "open",
        "needs_investigation",
    );
    let invocations = tmp.path().join("investigator.invocations");
    let _cmd = InvestigatorCmdGuard::set_success(&invocations);
    let mut a = investigator_agent(1);
    let duplicate_sub = a.subscribes_to[0].clone();
    a.subscribes_to.push(duplicate_sub);
    let agents = agents(a);

    let n1 = poll_once(
        &conn,
        &agents,
        &empty_policies(),
        &cfg_path(&tmp),
        "investigator-test",
        "epoch-investigator",
    )
    .unwrap();
    let n2 = poll_once(
        &conn,
        &agents,
        &empty_policies(),
        &cfg_path(&tmp),
        "investigator-test",
        "epoch-investigator",
    )
    .unwrap();

    assert_eq!(n1, 1, "first poll dispatches the investigator once");
    assert_eq!(
        n2, 0,
        "second poll does not redispatch the same transition attempt"
    );
    let invocations_count = std::fs::read_to_string(&invocations).unwrap().len();
    assert_eq!(invocations_count, 1, "stub investigator invoked once");
    let (locks, terminal_reason, last_status): (i64, String, String) = conn
        .query_row(
            "SELECT COUNT(*), MAX(terminal_reason), MAX(last_status) \
             FROM dispatch_locks WHERE row_id=?1 AND agent_name='investigator'",
            rusqlite::params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(locks, 1, "one dispatch_locks row for obs+investigator");
    assert_eq!(terminal_reason, "ok");
    assert_eq!(last_status, "ok");
    let status: String = conn
        .query_row(
            "SELECT status FROM observations WHERE display_id='L650'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "investigated");
}

#[test]
fn investigator_duplicate_transition_history_after_terminal_state_does_not_double_spawn() {
    let _guard = env_lock().lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let conn = fresh_db(&tmp.path().join("db.sqlite"));
    let row_id = insert_obs_at(&conn, "L651", "needs_investigation");
    insert_history(
        &conn,
        "observations",
        row_id,
        "L651",
        "open",
        "needs_investigation",
    );
    let invocations = tmp.path().join("investigator.invocations");
    let _cmd = InvestigatorCmdGuard::set_success(&invocations);
    let agents = agents(investigator_agent(1));

    assert_eq!(
        poll_once(
            &conn,
            &agents,
            &empty_policies(),
            &cfg_path(&tmp),
            "dup",
            "epoch-dup"
        )
        .unwrap(),
        1
    );
    assert_eq!(std::fs::read_to_string(&invocations).unwrap().len(), 1);

    for status in [
        "needs_investigation",
        "investigating",
        "investigated",
        "investigation_failed",
    ] {
        conn.execute(
            "UPDATE observations SET status=?1 WHERE display_id='L651'",
            rusqlite::params![status],
        )
        .unwrap();
        insert_history(
            &conn,
            "observations",
            row_id,
            "L651",
            "open",
            "needs_investigation",
        );
        let dispatched = poll_once(
            &conn,
            &agents,
            &empty_policies(),
            &cfg_path(&tmp),
            "dup",
            "epoch-dup",
        )
        .unwrap();
        assert_eq!(dispatched, 0, "status {status} must not double-spawn");
        let locks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dispatch_locks WHERE row_id=?1 AND agent_name='investigator'",
                rusqlite::params![row_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(locks, 1, "status {status} must not add dispatch_locks rows");
        assert_eq!(
            std::fs::read_to_string(&invocations).unwrap().len(),
            1,
            "status {status} must not invoke the stub again"
        );
    }
}

#[test]
fn investigator_failed_dispatch_lock_carries_failure_detail() {
    let _guard = env_lock().lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let conn = fresh_db(&tmp.path().join("db.sqlite"));
    let row_id = insert_obs_at(&conn, "L652", "needs_investigation");
    insert_history(
        &conn,
        "observations",
        row_id,
        "L652",
        "open",
        "needs_investigation",
    );
    let _cmd = InvestigatorCmdGuard::set_failure();
    let agents = agents(investigator_agent(1));

    let n = poll_once(
        &conn,
        &agents,
        &empty_policies(),
        &cfg_path(&tmp),
        "fail",
        "epoch-fail",
    )
    .unwrap();
    assert_eq!(n, 1);

    let (terminal_reason, last_status): (String, String) = conn
        .query_row(
            "SELECT terminal_reason, last_status FROM dispatch_locks \
             WHERE row_id=?1 AND agent_name='investigator'",
            rusqlite::params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(
        matches!(terminal_reason.as_str(), "exit_nonzero" | "error"),
        "unexpected terminal_reason: {terminal_reason}"
    );
    assert!(
        last_status.contains("rate_limit") || last_status.contains("subagent invocation failed"),
        "last_status must carry watch-visible failure detail; got: {last_status}"
    );
    let (status, reason): (String, Option<String>) = conn
        .query_row(
            "SELECT status, investigation_failure_reason FROM observations WHERE display_id='L652'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(status, "investigation_failed");
    assert!(reason.unwrap_or_default().contains("rate_limit"));
}
