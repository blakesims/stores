//! T140 P5: integration coverage for `stores tasks status <id>` surfacing
//! the operator_disposition label.
//!
//! Seeds a fixture DB with one row per representative disposition class
//! (≥5 dispositions), then exercises the binary's text output and asserts
//! the rendered `Disposition:` line carries the expected
//! `display_label()` string.
//!
//! Single-source-of-truth check: the disposition keyword identifiers
//! (`historical_terminal_legacy`, `awaiting_integration`, etc.) must NOT
//! appear in src/handlers/status.rs or src/cli/watch.rs — the rendered
//! labels come from `Disposition::display_label`.

use rusqlite::Connection;
use std::path::Path;
use std::process::Command;

fn stores_bin() -> String {
    env!("CARGO_BIN_EXE_stores").to_string()
}

/// Run `stores setup` in `cwd` so the manifest + bundled store schemas
/// (including `tasks`) are wired and the dynamic `tasks status` subcommand
/// is available.
fn stores_setup(cwd: &Path) {
    let out = Command::new(stores_bin())
        .current_dir(cwd)
        .arg("setup")
        .output()
        .expect("run stores setup");
    assert!(
        out.status.success(),
        "stores setup must succeed: stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn create_fixture_db(workspace: &Path) {
    stores_setup(workspace);
    let db_path = workspace.join(".stores").join("db.sqlite");
    let conn = Connection::open(&db_path).expect("open fixture db");
    seed_rows(&conn);
}

fn insert_task(
    conn: &Connection,
    display_id: &str,
    status: &str,
    activation: &str,
    branch: &str,
) -> i64 {
    let now = "2026-05-09T00:00:00Z";
    let slug = format!("task-{}", display_id.to_ascii_lowercase());
    let contract = r#"{"done_when":"d","scope_in":"i","scope_out":"o"}"#;
    conn.execute(
        "INSERT INTO tasks \
         (display_id, status, title, slug, contract, branch, tier_hint, activation, \
          created_at, updated_at, created_by, updated_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'T3', ?7, ?8, ?8, 'framework', 'framework')",
        rusqlite::params![
            display_id,
            status,
            format!("status-disposition fixture {display_id}"),
            slug,
            contract,
            branch,
            activation,
            now
        ],
    )
    .expect("insert task");
    conn.last_insert_rowid()
}

fn record_accepted_at(conn: &Connection, row_id: i64, display_id: &str, occurred_at: &str) {
    conn.execute(
        "INSERT INTO transition_history \
         (store, row_id, display_id, from_status, to_status, verb, invoker, occurred_at) \
         VALUES ('tasks', ?1, ?2, 'integrated', 'accepted', 'accept', 'human', ?3)",
        rusqlite::params![row_id, display_id, occurred_at],
    )
    .expect("insert transition_history");
}

fn seed_rows(conn: &Connection) {
    // ActiveEngineWork — status=executing
    insert_task(conn, "T601", "executing", "active", "");
    // EngineActionable (active) — status=planning, activation=active
    insert_task(conn, "T602", "planning", "active", "");
    // EngineActionable (inactive) — status=planning, activation=inactive
    insert_task(conn, "T603", "planning", "inactive", "");
    // BlockedRecoverable — status=blocked
    insert_task(conn, "T604", "blocked", "inactive", "");
    // TerminalSuccessModern — status=schema_migrated
    insert_task(conn, "T605", "schema_migrated", "inactive", "");
    // TerminalRetired — status=abandoned
    insert_task(conn, "T606", "abandoned", "inactive", "");
    // HistoricalTerminalLegacy — status=accepted, pre-cutoff accepted_at, no branch
    let id_legacy = insert_task(conn, "T607", "accepted", "inactive", "");
    record_accepted_at(conn, id_legacy, "T607", "2026-04-01T12:00:00Z");
    // NeedsOperatorReview — status=deploy_blocked
    insert_task(conn, "T608", "deploy_blocked", "inactive", "");
}

fn run_status(cwd: &Path, display_id: &str) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_stores");
    Command::new(bin)
        .current_dir(cwd)
        .args(["tasks", "status", display_id])
        .output()
        .expect("run stores tasks status")
}

// ---- AC5.1 ----

/// AC5.1: rendered status output carries the expected disposition labels
/// for each representative disposition class. ≥5 classes covered.
#[test]
fn status_output_carries_disposition_label_for_each_class() {
    let tmp = tempfile::tempdir().expect("tempdir");
    create_fixture_db(tmp.path());

    let cases: &[(&str, &str)] = &[
        ("T601", "Active engine work"),
        ("T602", "Engine actionable (active)"),
        ("T603", "Engine actionable (inactive)"),
        ("T604", "Blocked (recoverable)"),
        ("T605", "Terminal success"),
        ("T606", "Terminal retired"),
        ("T607", "Historical terminal (legacy)"),
        ("T608", "Needs operator review"),
    ];
    assert!(cases.len() >= 5, "AC5.1 minimum: 5 classes");

    for (display_id, expected_label) in cases {
        let out = run_status(tmp.path(), display_id);
        assert!(
            out.status.success(),
            "stores tasks status {display_id} must succeed: stderr=\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let expected_line = format!("Disposition: {expected_label}");
        assert!(
            stdout.contains(&expected_line),
            "{display_id}: expected substring `{expected_line}` in stdout:\n{stdout}"
        );
        assert!(
            stdout.contains("Activation:"),
            "{display_id}: status output must include Activation: line; got:\n{stdout}"
        );
    }
}

// ---- AC5.2 ----

/// AC5.2 (active engine work): a row with status=executing surfaces
/// `Disposition: Active engine work` regardless of activation flag.
#[test]
fn executing_row_disposition_is_active_engine_work() {
    let tmp = tempfile::tempdir().expect("tempdir");
    create_fixture_db(tmp.path());

    let out = run_status(tmp.path(), "T601");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Disposition: Active engine work"),
        "T601 (executing) must classify as Active engine work; got:\n{stdout}"
    );
}

/// AC5.2 (awaiting integration): an `integration_queued` / `integration_blocked`
/// row surfaces `Awaiting integration (active|inactive)` matching its
/// activation flag.
#[test]
fn awaiting_integration_row_disposition_matches_activation() {
    let tmp = tempfile::tempdir().expect("tempdir");
    create_fixture_db(tmp.path());

    let db_path = tmp.path().join(".stores").join("db.sqlite");
    let conn = Connection::open(&db_path).expect("open fixture db");
    insert_task(&conn, "T620", "integration_queued", "active", "");
    insert_task(&conn, "T621", "integration_blocked", "inactive", "");
    drop(conn);

    let out_active = run_status(tmp.path(), "T620");
    assert!(out_active.status.success());
    let stdout_active = String::from_utf8_lossy(&out_active.stdout);
    assert!(
        stdout_active.contains("Disposition: Awaiting integration (active)"),
        "T620 (integration_queued, active) must classify as Awaiting integration (active); got:\n{stdout_active}"
    );

    let out_inactive = run_status(tmp.path(), "T621");
    assert!(out_inactive.status.success());
    let stdout_inactive = String::from_utf8_lossy(&out_inactive.stdout);
    assert!(
        stdout_inactive.contains("Disposition: Awaiting integration (inactive)"),
        "T621 (integration_blocked, inactive) must classify as Awaiting integration (inactive); got:\n{stdout_inactive}"
    );
}

// ---- AC5.4 (single-source-of-truth grep) ----

/// AC5.4: disposition variant identifiers may live only in
/// `src/handlers/disposition.rs` and the docs. status.rs and watch.rs must
/// route through `Disposition::display_label` rather than hand-coding the
/// keyword strings.
#[test]
fn disposition_keyword_strings_only_appear_in_disposition_module() {
    let needles = [
        "historical_terminal_legacy",
        "awaiting_integration",
        "active_engine_work",
        "engine_actionable",
        "deploy_ceremony_pending",
        "terminal_success_missed_ceremony",
        "blocked_recoverable",
        "terminal_success_modern",
        "terminal_retired",
        "terminal_shipped_oob",
        "terminal_rejected",
        "needs_operator_review",
    ];
    let forbidden_files = [
        "src/handlers/status.rs",
        "src/cli/watch.rs",
    ];
    for path in &forbidden_files {
        let contents =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        for needle in &needles {
            assert!(
                !contents.contains(needle),
                "disposition keyword `{needle}` must not appear in {path}; \
                 route through Disposition::display_label instead"
            );
        }
    }
}
