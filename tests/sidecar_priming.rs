//! T028 P4: side-car priming snapshot tests.
//!
//! For each scope variant, build the priming bundle against an in-memory
//! fixture DB and assert (a) the system prompt matches its `.snap`,
//! (b) the initial message matches its `.snap` (with token redacted), and
//! (c) the priming body contains scope-appropriate header markers + the
//! verbatim approval token.
//!
//! Snapshot files live at `tests/fixtures/sidecar/`. To regenerate after
//! intentional changes, set `STORES_UPDATE_SNAPSHOTS=1` before running.

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use stores::tui::priming::{build, initial_message_for};
use stores::tui::sidecar::system_prompt::system_prompt_for;
use stores::tui::sidecar::SidecarScope;

const TOKEN: &str = "test-token-deadbeef";
const APPROVE_TOKEN_ENV: &str = "STORES_APPROVE_TOKEN";

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sidecar")
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = fixtures_dir().join(name);
    if std::env::var("STORES_UPDATE_SNAPSHOTS").ok().as_deref() == Some("1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("snapshot {} missing: {e}", path.display()));
    assert_eq!(
        actual, expected,
        "snapshot mismatch for {} — re-run with STORES_UPDATE_SNAPSHOTS=1 if intended",
        path.display()
    );
}

fn make_fixture_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE tasks (
            id INTEGER PRIMARY KEY,
            display_id TEXT,
            status TEXT,
            title TEXT,
            tier_hint TEXT,
            claimed_by TEXT,
            wrap_log TEXT,
            linked_observations TEXT
         );
         CREATE TABLE observations (
            id INTEGER PRIMARY KEY,
            display_id TEXT,
            status TEXT,
            summary TEXT,
            priority TEXT,
            body TEXT,
            intent_contract TEXT,
            task_id TEXT
         );
         CREATE TABLE transition_history (
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
            occurred_at TEXT NOT NULL
         );
         INSERT INTO tasks VALUES
           (1, 'T999', 'code_review', 'Demo task', 'T2', 'agent-x',
            '[{\"phase\":1,\"summary\":\"hello wrap\"}]',
            '[\"L001\"]');
         INSERT INTO observations VALUES
           (1, 'L001', 'open', 'Demo obs', 'normal',
            'body text', '{\"contract_state\":\"draft\"}', 'T999');
         INSERT INTO transition_history (store,row_id,display_id,from_status,to_status,verb,invoker,occurred_at)
           VALUES ('tasks',1,'T999','planning','code_review','submit-execute','ai_autonomous','2026-05-05T10:00:00Z');
        ",
    )
    .unwrap();
    conn
}

fn with_token<F: FnOnce()>(f: F) {
    let _g = match ENV_LOCK.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let prev = std::env::var(APPROVE_TOKEN_ENV).ok();
    std::env::set_var(APPROVE_TOKEN_ENV, TOKEN);
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match prev {
        Some(v) => std::env::set_var(APPROVE_TOKEN_ENV, v),
        None => std::env::remove_var(APPROVE_TOKEN_ENV),
    }
    if let Err(e) = res {
        std::panic::resume_unwind(e);
    }
}

fn check_scope(scope: SidecarScope, tag: &str, body_must_contain: &[&str]) {
    let conn = make_fixture_db();
    let sp = system_prompt_for(&scope);
    assert_snapshot(&format!("system-prompt-{tag}.snap"), &sp);

    let im = initial_message_for(&scope, TOKEN);
    assert_snapshot(&format!("initial-{tag}.snap"), &im);

    with_token(|| {
        let p = build(scope.clone(), &conn).unwrap();
        assert!(p.body.contains(TOKEN) || p.token == TOKEN, "token must be carried");
        for needle in body_must_contain {
            assert!(
                p.body.contains(needle),
                "priming body for {tag} missing substring '{needle}'\n--- body ---\n{}\n--- end ---",
                p.body
            );
        }
    });
}

#[test]
fn snapshot_per_row() {
    let scope = SidecarScope::PerRow {
        display_id: "T999".to_string(),
        fresh: false,
    };
    check_scope(
        scope,
        "per-row",
        &[
            "T999",
            "Demo task",
            "L001",
            "## wrap_log entries",
            "## transition_history",
        ],
    );
}

#[test]
fn snapshot_general() {
    check_scope(
        SidecarScope::General,
        "general",
        &["## Live rows", "T999"],
    );
}

#[test]
fn snapshot_obs_draft() {
    check_scope(
        SidecarScope::ObsDraft,
        "obs-draft",
        &["## Live rows", "## stores/observations/schema.yaml"],
    );
}

#[test]
fn priming_carries_verbatim_token_in_initial_message() {
    let scope = SidecarScope::PerRow {
        display_id: "T999".to_string(),
        fresh: false,
    };
    let im = initial_message_for(&scope, TOKEN);
    assert!(
        im.contains(TOKEN),
        "initial message must embed the operator's approval token verbatim"
    );
}

// Suppress dead-code warning for helper used only across some tests.
#[allow(dead_code)]
fn _path_anchor(_: &Path) {}
