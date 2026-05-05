//! T026 P2 — E2E integration test for the starting-line seeder.
//!
//! Verifies that adding a new subscriber after a daemon restart does NOT
//! retroactively fire on pre-startup transition_history rows, while NEW
//! transitions occurring after startup still dispatch normally.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::flow::agents_yaml::{load_from_path, TransitionEdge};
use stores::flow::policies_yaml::PoliciesYaml;
use stores::flow::{AgentEntry, AgentsYaml, BackoffKind, RetryPolicy, Subscription};
use stores::handlers::agents_run::{poll_once, seed_starting_line};
use stores::schema::Schema;

fn fresh_db(path: &Path) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    for name in ["tasks", "observations", "gate"] {
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, y)| *y)
            .unwrap();
        let s = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&s)).unwrap();
    }
    conn
}

fn noop_agent(name: &str, store: &str, from: &str, to: &str) -> AgentEntry {
    AgentEntry {
        name: name.to_string(),
        subscribes_to: vec![Subscription {
            store: store.to_string(),
            transition: TransitionEdge {
                from: from.to_string(),
                to: to.to_string(),
            },
            predicate: None,
        }],
        command: "/bin/true".to_string(),
        claim_window_secs: 300,
        retry_policy: RetryPolicy {
            max_attempts: 3,
            backoff: BackoffKind::Linear,
        },
        command_args: None,
    }
}

fn insert_history(
    conn: &Connection,
    store: &str,
    row_id: i64,
    display_id: &str,
    from: &str,
    to: &str,
) {
    conn.execute(
        "INSERT INTO transition_history \
         (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'submit', 'ai_autonomous', '2026-05-04T00:00:00Z')",
        rusqlite::params![store, row_id, display_id, from, to],
    )
    .unwrap();
}

fn empty_policies() -> PoliciesYaml {
    PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    }
}

fn cfg_path() -> PathBuf {
    PathBuf::from("/tmp/stores-test-nonexistent-config.yaml")
}

/// AC2.3: a new subscriber added after startup must NOT fire on pre-startup
/// transitions; new transitions occurring AFTER seeding fire normally.
#[test]
fn new_subscriber_after_restart_does_not_fire_on_historical_rows() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("t.sqlite");
    let conn = fresh_db(&db);

    // Seed a tasks row + transition_history for ready→in_review.
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, contract, \
                            created_at, updated_at, created_by, updated_by) \
         VALUES ('T100', 'in_review', 't', 't', 'feat/t', \
                 '{\"done_when\":\"x\",\"scope_in\":\"y\",\"scope_out\":\"z\"}', \
                 '2026-05-04T00:00:00Z', '2026-05-04T00:00:00Z', 'framework', 'framework')",
        [],
    )
    .unwrap();
    let row_id: i64 = conn
        .query_row("SELECT id FROM tasks WHERE display_id='T100'", [], |r| r.get(0))
        .unwrap();
    insert_history(&conn, "tasks", row_id, "T100", "ready", "in_review");

    // (c) Sanity check: a noop agent seeing this transition would dispatch.
    let original_agents = AgentsYaml {
        agents: vec![noop_agent("original", "tasks", "ready", "in_review")],
        deployment_specialist: None,
    };
    let policies = empty_policies();
    let n_sanity = poll_once(&conn, &original_agents, &policies, &cfg_path(), "claimer-1").unwrap();
    assert_eq!(n_sanity, 1, "sanity: matching transition would dispatch normally");

    // (d) Restart simulation: a fresh AgentsYaml with a NEW subscriber name.
    // Without seeding, late-comer would fire on the historical row.
    let late_agents = AgentsYaml {
        agents: vec![noop_agent("late-comer", "tasks", "ready", "in_review")],
        deployment_specialist: None,
    };
    let seeded = seed_starting_line(&conn, &late_agents).unwrap();
    assert_eq!(seeded, 1, "exactly one historical row seeded for late-comer");

    let n_after_seed =
        poll_once(&conn, &late_agents, &policies, &cfg_path(), "claimer-2").unwrap();
    assert_eq!(
        n_after_seed, 0,
        "late-comer must NOT fire on pre-startup transition"
    );

    let claimed_by: String = conn
        .query_row(
            "SELECT claimed_by FROM dispatch_locks \
             WHERE store='tasks' AND row_id=?1 AND agent_name='late-comer'",
            rusqlite::params![row_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        claimed_by, "starting-line-marker",
        "the only late-comer lock must be the starting-line marker"
    );

    // (e) NEW transition occurring AFTER seeding still dispatches normally.
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, contract, \
                            created_at, updated_at, created_by, updated_by) \
         VALUES ('T101', 'in_review', 't', 't', 'feat/t2', \
                 '{\"done_when\":\"x\",\"scope_in\":\"y\",\"scope_out\":\"z\"}', \
                 '2026-05-04T00:00:00Z', '2026-05-04T00:00:00Z', 'framework', 'framework')",
        [],
    )
    .unwrap();
    let row_id2: i64 = conn
        .query_row("SELECT id FROM tasks WHERE display_id='T101'", [], |r| r.get(0))
        .unwrap();
    insert_history(&conn, "tasks", row_id2, "T101", "ready", "in_review");

    let n_fresh = poll_once(&conn, &late_agents, &policies, &cfg_path(), "claimer-3").unwrap();
    assert_eq!(n_fresh, 1, "fresh post-seed transition must dispatch normally");

    let claimed_by_fresh: String = conn
        .query_row(
            "SELECT claimed_by FROM dispatch_locks \
             WHERE store='tasks' AND row_id=?1 AND agent_name='late-comer'",
            rusqlite::params![row_id2],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(
        claimed_by_fresh, "starting-line-marker",
        "fresh dispatch lock must NOT carry the starting-line marker"
    );
}

/// Seeding against an empty transition_history is a no-op and does not
/// swallow fresh transitions for the production agents.yaml fixture.
#[test]
fn seed_preserves_existing_drive_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("t.sqlite");
    let conn = fresh_db(&db);

    let agents_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agents.yaml");
    let agents = load_from_path(&agents_path).expect("production agents.yaml must parse");

    let seeded = seed_starting_line(&conn, &agents).unwrap();
    assert_eq!(seeded, 0, "empty transition_history → 0 rows seeded");

    let cnt: i64 = conn
        .query_row("SELECT COUNT(*) FROM dispatch_locks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(cnt, 0, "no dispatch_locks rows after seed against empty history");

    // Insert a fresh observations confirmed→ready transition (the auto-promote
    // edge). poll_once should claim it for the auto-promote subscriber.
    conn.execute(
        "INSERT INTO observations (display_id, status, summary, source, priority, \
                                   captured_at, captured_week, created_at, updated_at, \
                                   created_by, updated_by) \
         VALUES ('L500', 'ready', 'fresh', 'dev', 'normal', \
                 '2026-05-04T00:00:00Z', 'w18-d7', \
                 '2026-05-04T00:00:00Z', '2026-05-04T00:00:00Z', 'human', 'human')",
        [],
    )
    .unwrap();
    let obs_row_id: i64 = conn
        .query_row(
            "SELECT id FROM observations WHERE display_id='L500'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    insert_history(&conn, "observations", obs_row_id, "L500", "confirmed", "ready");

    let policies = empty_policies();
    // Iterate poll_once briefly — auto-promote will attempt to claim and may
    // fail at builtin execution due to missing scaffolding, but the CLAIM
    // (i.e. dispatch_locks insert with claimed_by != starting-line-marker)
    // is what we verify: the seeder must not have pre-claimed it.
    let start = Instant::now();
    let mut claimed = false;
    while start.elapsed() < Duration::from_secs(5) {
        let _ = poll_once(&conn, &agents, &policies, &cfg_path(), "seed-test").unwrap();
        let cb: Option<String> = conn
            .query_row(
                "SELECT claimed_by FROM dispatch_locks \
                 WHERE store='observations' AND row_id=?1 AND agent_name='auto-promote'",
                rusqlite::params![obs_row_id],
                |r| r.get(0),
            )
            .ok();
        if let Some(by) = cb {
            assert_ne!(
                by, "starting-line-marker",
                "fresh post-seed transition must NOT be marked starting-line"
            );
            claimed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        claimed,
        "auto-promote must claim the fresh confirmed→ready transition after seeding"
    );
}
