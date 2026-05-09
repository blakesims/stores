//! T140 P4: integration coverage for `stores engine plan-start`.
//!
//! Seeds a fixture DB spanning every plan-start bucket (13 rows across all
//! 5 buckets), then exercises the binary's text and JSON output. Pins the
//! activation-driven WouldRun-vs-Inactive split for the integration lane via
//! status='accepted' + unmerged branch rows (T504 active → would_run, T512
//! inactive → inactive — the AC4.6 spec) plus the parallel
//! integration_queued/integration_blocked rows. Verifies that running
//! plan-start does NOT mutate the DB (tasks-table content hash +
//! transition_history row count are byte-identical before and after).

use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::schema::Schema;

fn tasks_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(name, _)| *name == "tasks")
        .map(|(_, y)| *y)
        .expect("bundled tasks schema present");
    Schema::from_yaml(yaml).expect("tasks schema parses")
}

fn create_fixture_db(db_path: &Path) {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).expect("create .stores dir");
    }
    let conn = Connection::open(db_path).expect("open fresh db");
    conn.execute_batch(SUBSTRATE_DDL).expect("substrate ddl");
    let schema = tasks_schema();
    conn.execute_batch(&ddl_for(&schema))
        .expect("tasks table ddl");
    seed_rows(&conn);
}

fn insert_task(
    conn: &Connection,
    display_id: &str,
    status: &str,
    activation: Option<&str>,
    branch: Option<&str>,
    tier_hint: Option<&str>,
) -> i64 {
    let now = "2026-05-09T00:00:00Z";
    let slug = format!("task-{}", display_id.to_ascii_lowercase());
    let contract = r#"{"done_when":"d","scope_in":"i","scope_out":"o"}"#;
    let activation = activation.unwrap_or("inactive");
    let branch = branch.unwrap_or("");
    let tier_hint = tier_hint.unwrap_or("T3");
    conn.execute(
        "INSERT INTO tasks \
         (display_id, status, title, slug, contract, branch, tier_hint, activation, \
          created_at, updated_at, created_by, updated_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, 'framework', 'framework')",
        rusqlite::params![
            display_id,
            status,
            format!("task {display_id}"),
            slug,
            contract,
            branch,
            tier_hint,
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
    // would_run bucket
    // - ActiveEngineWork (status=executing)
    insert_task(conn, "T501", "executing", Some("active"), None, Some("T3"));
    // - EngineActionable + activation=active (planning)
    insert_task(conn, "T502", "planning", Some("active"), None, Some("T3"));
    // - AwaitingIntegration + activation=active (integration_queued)
    insert_task(
        conn,
        "T503",
        "integration_queued",
        Some("active"),
        None,
        Some("T2"),
    );
    // - AC4.6 active arm: status='accepted' + unmerged branch + activation=active
    //   → AwaitingIntegration { activation_active: true } → would_run.
    //   The fixture cwd is not a git repo, so the binary's GitBranchStateSource
    //   errors on `git merge-base --is-ancestor`; the classifier's conservative
    //   fallback treats the non-empty branch field as still in the integration
    //   lane (covered by the disposition unit tests).
    let id_acc_active = insert_task(
        conn,
        "T504",
        "accepted",
        Some("active"),
        Some("feat/T504-active-unmerged"),
        Some("T2"),
    );
    record_accepted_at(conn, id_acc_active, "T504", "2026-05-08T10:00:00Z");

    // inactive bucket
    // - EngineActionable + activation=inactive (planning)
    insert_task(conn, "T510", "planning", Some("inactive"), None, Some("T3"));
    // - AwaitingIntegration + activation=inactive (integration_blocked)
    insert_task(
        conn,
        "T511",
        "integration_blocked",
        Some("inactive"),
        None,
        Some("T2"),
    );
    // - AC4.6 inactive arm: status='accepted' + unmerged branch + activation=inactive
    //   → AwaitingIntegration { activation_active: false } → inactive.
    let id_acc_inactive = insert_task(
        conn,
        "T512",
        "accepted",
        Some("inactive"),
        Some("feat/T512-inactive-unmerged"),
        Some("T2"),
    );
    record_accepted_at(conn, id_acc_inactive, "T512", "2026-05-08T10:00:00Z");

    // needs_operator bucket
    // - DeployCeremonyPending (status=integrated)
    insert_task(conn, "T520", "integrated", Some("inactive"), None, Some("T3"));
    // - NeedsOperatorReview via deploy_blocked
    insert_task(
        conn,
        "T521",
        "deploy_blocked",
        Some("inactive"),
        None,
        Some("T3"),
    );

    // blocked bucket
    insert_task(conn, "T530", "blocked", Some("inactive"), None, Some("T3"));

    // historical bucket
    // - HistoricalTerminalLegacy (status=accepted, pre-cutoff, no branch)
    let id_legacy = insert_task(conn, "T540", "accepted", Some("inactive"), None, Some("T3"));
    record_accepted_at(conn, id_legacy, "T540", "2026-04-01T12:00:00Z");
    // - TerminalSuccessModern (schema_migrated)
    insert_task(
        conn,
        "T541",
        "schema_migrated",
        Some("inactive"),
        None,
        Some("T3"),
    );
    // - TerminalRetired (abandoned)
    insert_task(conn, "T542", "abandoned", Some("inactive"), None, Some("T3"));
}

fn hash_table(conn: &Connection, table: &str) -> String {
    use std::fmt::Write;
    let sql = format!(
        "SELECT * FROM \"{}\" ORDER BY id",
        table.replace('"', "")
    );
    let mut stmt = conn.prepare(&sql).expect("prepare table dump");
    let column_count = stmt.column_count();
    let mut rows = stmt.query([]).expect("query table");
    let mut buf = String::new();
    while let Some(r) = rows.next().expect("next row") {
        for i in 0..column_count {
            let v: rusqlite::types::Value = r.get(i).expect("column value");
            let _ = write!(buf, "{:?}|", v);
        }
        buf.push('\n');
    }
    buf
}

fn count_table(conn: &Connection, table: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM \"{}\"", table.replace('"', ""));
    conn.query_row(&sql, [], |r| r.get(0)).unwrap_or(0)
}

fn run_plan_start(cwd: &Path, json: bool) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_stores");
    let mut cmd = Command::new(bin);
    cmd.current_dir(cwd).args(["engine", "plan-start"]);
    if json {
        cmd.arg("--json");
    }
    cmd.output().expect("run stores engine plan-start")
}

// ---- AC4.1 ----

#[test]
fn plan_start_help_is_listed_under_engine() {
    let bin = env!("CARGO_BIN_EXE_stores");
    let engine_help = Command::new(bin)
        .args(["engine", "--help"])
        .output()
        .expect("run engine --help");
    assert!(
        engine_help.status.success(),
        "engine --help must succeed: {:?}",
        engine_help.status
    );
    let stdout = String::from_utf8_lossy(&engine_help.stdout);
    assert!(
        stdout.contains("plan-start"),
        "engine --help must list plan-start subverb; got:\n{stdout}"
    );

    let ps_help = Command::new(bin)
        .args(["engine", "plan-start", "--help"])
        .output()
        .expect("run plan-start --help");
    assert!(
        ps_help.status.success(),
        "plan-start --help must succeed: {:?}",
        ps_help.status
    );
    let ps_stdout = String::from_utf8_lossy(&ps_help.stdout);
    assert!(
        ps_stdout.contains("ignition plan") || ps_stdout.contains("plan-start"),
        "plan-start --help must describe the verb; got:\n{ps_stdout}"
    );
}

// ---- AC4.2 + AC4.5 (text mode) ----

#[test]
fn plan_start_text_mode_emits_summary_and_section_headers_in_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);

    let out = run_plan_start(tmp.path(), false);
    assert!(
        out.status.success(),
        "plan-start (text) must succeed: stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Top-line summary contains all five bucket counts.
    assert!(
        stdout.contains("engine ignition plan: 4 would-run"),
        "summary must report 4 would-run; got:\n{stdout}"
    );
    assert!(
        stdout.contains("3 inactive"),
        "summary must report 3 inactive; got:\n{stdout}"
    );
    assert!(
        stdout.contains("2 needs-operator"),
        "summary must report 2 needs-operator; got:\n{stdout}"
    );
    assert!(
        stdout.contains("1 blocked"),
        "summary must report 1 blocked; got:\n{stdout}"
    );
    assert!(
        stdout.contains("3 historical"),
        "summary must report 3 historical; got:\n{stdout}"
    );

    // Bucket section headers appear in documented order.
    let order = ["would_run (", "inactive (", "needs_operator (", "blocked (", "historical ("];
    let mut cursor = 0usize;
    for header in &order {
        let idx = stdout[cursor..].find(header).unwrap_or_else(|| {
            panic!("missing or out-of-order section header {header}; got:\n{stdout}")
        });
        cursor += idx + header.len();
    }
}

// ---- AC4.3 (JSON mode keys) ----

#[test]
fn plan_start_json_mode_emits_exactly_the_five_top_level_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);

    let out = run_plan_start(tmp.path(), true);
    assert!(
        out.status.success(),
        "plan-start --json must succeed: stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8 json");
    let v: Value = serde_json::from_str(&stdout).expect("valid JSON");
    let obj = v.as_object().expect("top-level object");

    let mut keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
    keys.sort();
    assert_eq!(
        keys,
        vec!["blocked", "historical", "inactive", "needs_operator", "would_run"],
        "JSON top-level keys must be exactly the five contract buckets; got {keys:?}"
    );
}

// ---- AC4.2 + AC4.6 (bucket assignments + activation split) ----

#[test]
fn plan_start_bucket_assignments_match_fixture_dispositions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);

    let out = run_plan_start(tmp.path(), true);
    assert!(out.status.success(), "plan-start --json must succeed");
    let v: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout))
        .expect("valid JSON");

    let bucket_ids = |key: &str| -> Vec<String> {
        v.get(key)
            .and_then(|x| x.as_array())
            .expect("bucket array")
            .iter()
            .map(|e| {
                e.get("display_id")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string()
            })
            .collect()
    };

    let mut wr = bucket_ids("would_run");
    wr.sort();
    assert_eq!(
        wr,
        vec![
            "T501".to_string(),
            "T502".to_string(),
            "T503".to_string(),
            "T504".to_string(),
        ],
        "would_run must contain ActiveEngineWork + active EngineActionable + \
         active AwaitingIntegration (integration_queued) + active accepted-with-unmerged-branch"
    );

    let mut inactive = bucket_ids("inactive");
    inactive.sort();
    assert_eq!(
        inactive,
        vec![
            "T510".to_string(),
            "T511".to_string(),
            "T512".to_string(),
        ],
        "inactive must contain inactive EngineActionable + inactive AwaitingIntegration \
         (integration_blocked) + inactive accepted-with-unmerged-branch"
    );

    let mut needs_op = bucket_ids("needs_operator");
    needs_op.sort();
    assert_eq!(
        needs_op,
        vec!["T520".to_string(), "T521".to_string()],
        "needs_operator must contain DeployCeremonyPending + NeedsOperatorReview"
    );

    assert_eq!(
        bucket_ids("blocked"),
        vec!["T530".to_string()],
        "blocked must contain BlockedRecoverable"
    );

    let mut hist = bucket_ids("historical");
    hist.sort();
    assert_eq!(
        hist,
        vec!["T540".to_string(), "T541".to_string(), "T542".to_string()],
        "historical must contain legacy accepted + schema_migrated + abandoned"
    );

    // AC4.6 (as written): the activation-driven split is pinned by
    // status='accepted' + unmerged branch rows: T504 (active → would_run) and
    // T512 (inactive → inactive). The integration_queued/blocked rows (T503,
    // T511) provide the same split for the older-status path.
    assert!(
        bucket_ids("would_run").contains(&"T504".to_string()),
        "T504 (status=accepted, branch unmerged, activation=active) must land in would_run"
    );
    assert!(
        bucket_ids("inactive").contains(&"T512".to_string()),
        "T512 (status=accepted, branch unmerged, activation=inactive) must land in inactive"
    );
    assert!(
        bucket_ids("would_run").contains(&"T503".to_string()),
        "T503 (integration_queued, activation=active) must land in would_run"
    );
    assert!(
        bucket_ids("inactive").contains(&"T511".to_string()),
        "T511 (integration_blocked, activation=inactive) must land in inactive"
    );
}

// ---- AC4.4 (read-only: no DB mutation) ----

#[test]
fn plan_start_does_not_mutate_the_db() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);

    let pre = {
        let conn = Connection::open(&db_path).expect("open db");
        let h = hash_table(&conn, "tasks");
        let n = count_table(&conn, "transition_history");
        (h, n)
    };

    // Run both modes; neither must perturb the DB.
    let _ = run_plan_start(tmp.path(), false);
    let _ = run_plan_start(tmp.path(), true);

    let post = {
        let conn = Connection::open(&db_path).expect("open db");
        let h = hash_table(&conn, "tasks");
        let n = count_table(&conn, "transition_history");
        (h, n)
    };

    assert_eq!(
        pre.0, post.0,
        "tasks table content hash must be byte-identical before and after plan-start"
    );
    assert_eq!(
        pre.1, post.1,
        "transition_history row count must be byte-identical before and after plan-start"
    );
}

// ---- AC4.5 cross-check (text summary counts == JSON array lengths) ----

#[test]
fn plan_start_text_summary_counts_match_json_array_lengths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);

    let json_out = run_plan_start(tmp.path(), true);
    assert!(json_out.status.success());
    let v: Value =
        serde_json::from_str(&String::from_utf8_lossy(&json_out.stdout)).expect("valid JSON");
    let len = |k: &str| v.get(k).and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0);
    let n_would = len("would_run");
    let n_inactive = len("inactive");
    let n_op = len("needs_operator");
    let n_block = len("blocked");
    let n_hist = len("historical");

    let text_out = run_plan_start(tmp.path(), false);
    assert!(text_out.status.success());
    let stdout = String::from_utf8_lossy(&text_out.stdout);
    let expected = format!(
        "engine ignition plan: {} would-run · {} inactive · {} needs-operator · {} blocked · {} historical",
        n_would, n_inactive, n_op, n_block, n_hist
    );
    assert!(
        stdout.contains(&expected),
        "text summary must match JSON array lengths; expected substring=\n{expected}\nfull stdout=\n{stdout}"
    );
}
