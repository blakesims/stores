//! T140 P6 / Task 6.1: regression fixture seeded from the actual current DB
//! shape (the dirty snapshot the audit doc enumerates) so future schema
//! changes cannot silently re-classify rows.
//!
//! Asserts:
//!   (a) `operator_disposition` classifies each fixture row to the bucket the
//!       audit doc names (audit doc § 1 mapping in
//!       docs/worklog/2026-05-09/04-manual-cleanup-triage-audit.md);
//!   (b) `stores engine plan-start` (run via the binary, like
//!       `tests/cli_engine_plan_start.rs`) emits the documented buckets;
//!   (c) the `linked_observations` field on T138's row is preserved through
//!       to plan-start's JSON output.

use rusqlite::Connection;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::disposition::{
    operator_disposition, BranchStateSource, Disposition, PlanStartBucket,
};
use stores::schema::Schema;

// ---------- Fixture helpers ----------

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
    seed_dirty_snapshot(&conn);
}

fn insert_task(
    conn: &Connection,
    display_id: &str,
    status: &str,
    activation: &str,
    branch: &str,
    tier_hint: &str,
    linked_observations: Option<&str>,
) -> i64 {
    let now = "2026-05-09T00:00:00Z";
    let slug = format!("task-{}", display_id.to_ascii_lowercase());
    let contract = r#"{"done_when":"d","scope_in":"i","scope_out":"o"}"#;
    let linked = linked_observations.unwrap_or("[]");
    conn.execute(
        "INSERT INTO tasks \
         (display_id, status, title, slug, contract, branch, tier_hint, activation, \
          linked_observations, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, 'framework', 'framework')",
        rusqlite::params![
            display_id,
            status,
            format!("task {display_id}"),
            slug,
            contract,
            branch,
            tier_hint,
            activation,
            linked,
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

/// Seed the audit-doc dirty snapshot: T001–T018 legacy accepted, T081 (ceremony
/// gap), T122 (needs operator review), T125/T127 (deploy ceremony pending),
/// T138 (live cleanup baseline: schema_migrated/historical), synthetic accepted
/// integration rows in both activation states, T139 (active engine work), plus representative bulk-class rows
/// (schema_migrated, abandoned, closed_out_of_band, blocked, rejected). Plus
/// observations rows pinning the cross-store linkage shape — they live in the
/// observations store but exist here so the fixture DB carries the same shape
/// as the live DB.
fn seed_dirty_snapshot(conn: &Connection) {
    // ---- T001..T018 — legacy accepted (HistoricalTerminalLegacy) ----
    // Half the rows carry an explicit pre-cutoff accepted_at; the other half
    // omit it — both paths fold to historical.
    for n in 1..=18 {
        let id = format!("T{:03}", n);
        let row_id = insert_task(conn, &id, "accepted", "inactive", "", "T2", None);
        if n % 2 == 0 {
            record_accepted_at(conn, row_id, &id, "2026-04-01T00:00:00Z");
        }
    }

    // ---- T081 — TerminalSuccessMissedCeremony (name-pinned) ----
    let t081 = insert_task(conn, "T081", "accepted", "inactive", "", "T2", None);
    record_accepted_at(conn, t081, "T081", "2026-05-04T12:00:00Z");

    // ---- T122 — NeedsOperatorReview (name-pinned) ----
    let t122 = insert_task(conn, "T122", "accepted", "inactive", "", "T2", None);
    record_accepted_at(conn, t122, "T122", "2026-05-05T12:00:00Z");

    // ---- T125, T127 — DeployCeremonyPending (post-cutoff accepted, no branch
    //      OR a merged branch) ----
    let t125 = insert_task(conn, "T125", "accepted", "inactive", "", "T2", None);
    record_accepted_at(conn, t125, "T125", "2026-05-06T12:00:00Z");
    let t127 = insert_task(conn, "T127", "accepted", "inactive", "", "T2", None);
    record_accepted_at(conn, t127, "T127", "2026-05-06T12:00:00Z");

    // ---- T138 — live cleanup baseline: TerminalSuccessModern / historical ----
    // Phase 6 repair accepted the live reality that T138 is already
    // schema_migrated/historical. Preserve its linked_observations payload in
    // plan-start JSON so the cross-store linkage shape remains visible.
    insert_task(
        conn,
        "T138",
        "schema_migrated",
        "inactive",
        "feat/T138-integration-lane",
        "T2",
        Some(r#"["L538","L540"]"#),
    );

    // Synthetic accepted rows still pin the activation-driven AwaitingIntegration
    // split without pretending live T138 is still awaiting integration.
    insert_task(
        conn,
        "T138_ACCEPTED_ACTIVE",
        "accepted",
        "active",
        "feat/T138-integration-lane-active",
        "T2",
        Some(r#"["L538","L540"]"#),
    );
    insert_task(
        conn,
        "T138_ACCEPTED_INACTIVE",
        "accepted",
        "inactive",
        "feat/T138-integration-lane-inactive",
        "T2",
        Some(r#"["L538","L540"]"#),
    );

    // ---- T139 — ActiveEngineWork ----
    insert_task(conn, "T139", "executing", "active", "", "T3", None);

    // ---- bulk classes: schema_migrated / abandoned / closed_out_of_band ----
    for n in 200..205 {
        insert_task(
            conn,
            &format!("T{n}"),
            "schema_migrated",
            "inactive",
            "",
            "T3",
            None,
        );
    }
    for n in 300..305 {
        insert_task(
            conn,
            &format!("T{n}"),
            "abandoned",
            "inactive",
            "",
            "T3",
            None,
        );
    }
    for n in 400..403 {
        insert_task(
            conn,
            &format!("T{n}"),
            "closed_out_of_band",
            "inactive",
            "",
            "T3",
            None,
        );
    }

    // ---- blocked + rejected ----
    insert_task(conn, "T610", "blocked", "inactive", "", "T3", None);
    insert_task(conn, "T611", "blocked", "inactive", "", "T3", None);
    insert_task(conn, "T620", "rejected", "inactive", "", "T3", None);

    // ---- representative observations rows for cross-store linkage shape ----
    // L032/L150/L538/L540 — the audit doc names these as the rows the live DB
    // carries. We seed minimal rows so the fixture DB shape mirrors live;
    // operator_disposition itself does not read observations, but the rows
    // pin that the substrate's observations table is part of the snapshot.
    let _ = conn.execute(
        "INSERT INTO observations \
         (display_id, status, summary, captured_at, captured_week, source_agent, created_at, updated_at, created_by, updated_by) \
         VALUES \
            ('L032', 'ready',         's', '2026-05-09T00:00:00Z', 'w19-d6', 'orchestrator', '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z', 'framework', 'framework'), \
            ('L150', 'investigating', 's', '2026-05-09T00:00:00Z', 'w19-d6', 'orchestrator', '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z', 'framework', 'framework'), \
            ('L538', 'ready',         's', '2026-05-09T00:00:00Z', 'w19-d6', 'orchestrator', '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z', 'framework', 'framework'), \
            ('L540', 'ready',         's', '2026-05-09T00:00:00Z', 'w19-d6', 'orchestrator', '2026-05-09T00:00:00Z', '2026-05-09T00:00:00Z', 'framework', 'framework')",
        [],
    );

    // ---- minimal dispatch_locks row pinning the snapshot shape ----
    let _ = conn.execute(
        "INSERT INTO dispatch_locks \
         (lock_key, store, display_id, holder_pid, claimed_at) \
         VALUES ('audit-snapshot-lock-1', 'tasks', 'T139', NULL, '2026-05-09T00:00:00Z')",
        [],
    );
}

// ---------- BranchStateSource for unit-style assertions ----------

/// Mock that reports every branch as unmerged. Mirrors the disposition unit
/// tests' `MockBranchState` so the fixture's accepted+unmerged rows classify
/// as `AwaitingIntegration` deterministically.
struct AlwaysUnmerged;

impl BranchStateSource for AlwaysUnmerged {
    fn branch_unmerged(&self, _branch: &str) -> anyhow::Result<bool> {
        Ok(true)
    }
}

fn today() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-05-09T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn classify_row(
    conn: &Connection,
    display_id: &str,
    branch_state: &dyn BranchStateSource,
) -> Disposition {
    let mut stmt = conn
        .prepare(
            "SELECT display_id, status, COALESCE(activation,'inactive'), \
                    COALESCE(branch,''), COALESCE(linked_observations,'[]') \
             FROM tasks WHERE display_id = ?1",
        )
        .expect("prepare");
    let mut rows = stmt
        .query(rusqlite::params![display_id])
        .expect("query row");
    let row = rows
        .next()
        .expect("query next")
        .unwrap_or_else(|| panic!("display_id {display_id} not found"));
    let did: String = row.get(0).unwrap();
    let status: String = row.get(1).unwrap();
    let activation: String = row.get(2).unwrap();
    let branch: String = row.get(3).unwrap();
    let linked: String = row.get(4).unwrap();

    // Look up accepted_at (max occurred_at where to_status='accepted') for
    // this display_id — mirrors load_plan_start in src/cli/engine.rs.
    let accepted_at: Option<String> = conn
        .query_row(
            "SELECT MAX(occurred_at) FROM transition_history \
             WHERE store='tasks' AND display_id=?1 AND to_status='accepted'",
            rusqlite::params![display_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();

    let mut row_json = serde_json::json!({
        "display_id": did,
        "status": status,
        "activation": activation,
        "branch": branch,
        "linked_observations": serde_json::from_str::<Value>(&linked).unwrap_or(serde_json::json!([])),
    });
    if let Some(at) = accepted_at {
        row_json["accepted_at"] = serde_json::json!(at);
    }
    operator_disposition(&row_json, today(), branch_state)
}

// ---------- AC6.1 (a) — disposition mapping ----------

#[test]
fn dirty_snapshot_t001_to_t018_classify_as_historical_terminal_legacy() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);
    let conn = Connection::open(&db_path).expect("open db");

    for n in 1..=18 {
        let id = format!("T{:03}", n);
        let d = classify_row(&conn, &id, &AlwaysUnmerged);
        assert!(
            matches!(d, Disposition::HistoricalTerminalLegacy),
            "{id} must classify as HistoricalTerminalLegacy; got {d:?}"
        );
        assert_eq!(d.plan_start_bucket(), PlanStartBucket::Historical);
    }
}

#[test]
fn dirty_snapshot_t081_classifies_as_terminal_success_missed_ceremony() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);
    let conn = Connection::open(&db_path).expect("open db");

    let d = classify_row(&conn, "T081", &AlwaysUnmerged);
    assert!(matches!(d, Disposition::TerminalSuccessMissedCeremony));
    assert_eq!(d.plan_start_bucket(), PlanStartBucket::NeedsOperator);
}

#[test]
fn dirty_snapshot_t122_classifies_as_needs_operator_review() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);
    let conn = Connection::open(&db_path).expect("open db");

    let d = classify_row(&conn, "T122", &AlwaysUnmerged);
    assert!(matches!(d, Disposition::NeedsOperatorReview));
    assert_eq!(d.plan_start_bucket(), PlanStartBucket::NeedsOperator);
}

#[test]
fn dirty_snapshot_t125_t127_classify_as_deploy_ceremony_pending() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);
    let conn = Connection::open(&db_path).expect("open db");

    for id in &["T125", "T127"] {
        let d = classify_row(&conn, id, &AlwaysUnmerged);
        assert!(
            matches!(d, Disposition::DeployCeremonyPending),
            "{id} must classify as DeployCeremonyPending; got {d:?}"
        );
        assert_eq!(d.plan_start_bucket(), PlanStartBucket::NeedsOperator);
    }
}

#[test]
fn dirty_snapshot_t138_is_historical_and_synthetic_rows_cover_awaiting_integration_split() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);
    let conn = Connection::open(&db_path).expect("open db");

    let t138 = classify_row(&conn, "T138", &AlwaysUnmerged);
    assert!(
        matches!(t138, Disposition::TerminalSuccessModern),
        "live T138 fixture must classify as historical schema_migrated; got {t138:?}"
    );
    assert_eq!(t138.plan_start_bucket(), PlanStartBucket::Historical);

    let active = classify_row(&conn, "T138_ACCEPTED_ACTIVE", &AlwaysUnmerged);
    assert_eq!(
        active,
        Disposition::AwaitingIntegration {
            activation_active: true,
        }
    );
    assert_eq!(active.plan_start_bucket(), PlanStartBucket::WouldRun);

    let inactive = classify_row(&conn, "T138_ACCEPTED_INACTIVE", &AlwaysUnmerged);
    assert_eq!(
        inactive,
        Disposition::AwaitingIntegration {
            activation_active: false,
        }
    );
    assert_eq!(inactive.plan_start_bucket(), PlanStartBucket::Inactive);
}

#[test]
fn dirty_snapshot_t139_classifies_as_active_engine_work() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);
    let conn = Connection::open(&db_path).expect("open db");

    let d = classify_row(&conn, "T139", &AlwaysUnmerged);
    assert!(matches!(d, Disposition::ActiveEngineWork));
    assert_eq!(d.plan_start_bucket(), PlanStartBucket::WouldRun);
}

#[test]
fn dirty_snapshot_bulk_classes_classify_as_historical() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);
    let conn = Connection::open(&db_path).expect("open db");

    for n in 200..205 {
        let d = classify_row(&conn, &format!("T{n}"), &AlwaysUnmerged);
        assert!(
            matches!(d, Disposition::TerminalSuccessModern),
            "T{n} (schema_migrated) must classify as TerminalSuccessModern; got {d:?}"
        );
    }
    for n in 300..305 {
        let d = classify_row(&conn, &format!("T{n}"), &AlwaysUnmerged);
        assert!(
            matches!(d, Disposition::TerminalRetired),
            "T{n} (abandoned) must classify as TerminalRetired; got {d:?}"
        );
    }
    for n in 400..403 {
        let d = classify_row(&conn, &format!("T{n}"), &AlwaysUnmerged);
        assert!(
            matches!(d, Disposition::TerminalShippedOob),
            "T{n} (closed_out_of_band) must classify as TerminalShippedOob; got {d:?}"
        );
    }
}

#[test]
fn dirty_snapshot_blocked_and_rejected_classify_correctly() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);
    let conn = Connection::open(&db_path).expect("open db");

    for id in &["T610", "T611"] {
        let d = classify_row(&conn, id, &AlwaysUnmerged);
        assert!(
            matches!(d, Disposition::BlockedRecoverable),
            "{id} must classify as BlockedRecoverable; got {d:?}"
        );
        assert_eq!(d.plan_start_bucket(), PlanStartBucket::Blocked);
    }
    let d = classify_row(&conn, "T620", &AlwaysUnmerged);
    assert!(matches!(d, Disposition::TerminalRejected));
    assert_eq!(d.plan_start_bucket(), PlanStartBucket::Historical);
}

// ---------- AC6.1 (b) — `stores engine plan-start` end-to-end ----------

fn run_plan_start_json(cwd: &Path) -> Value {
    let bin = env!("CARGO_BIN_EXE_stores");
    let out = Command::new(bin)
        .current_dir(cwd)
        .args(["engine", "plan-start", "--json"])
        .output()
        .expect("run stores engine plan-start --json");
    assert!(
        out.status.success(),
        "plan-start --json must succeed: stderr=\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).expect("utf-8 json");
    serde_json::from_str(&stdout).expect("valid JSON")
}

#[test]
fn dirty_snapshot_plan_start_emits_documented_buckets_with_required_fixture_rows() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);

    let v = run_plan_start_json(tmp.path());

    // Top-level keys are exactly the five contract buckets.
    let mut keys: Vec<&str> = v
        .as_object()
        .expect("object")
        .keys()
        .map(|s| s.as_str())
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "blocked",
            "historical",
            "inactive",
            "needs_operator",
            "would_run"
        ]
    );

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

    let total_rows: usize = [
        "would_run",
        "inactive",
        "needs_operator",
        "blocked",
        "historical",
    ]
    .iter()
    .map(|k| bucket_ids(k).len())
    .sum();
    assert!(
        total_rows >= 30,
        "fixture must seed ≥30 rows across all 5 plan-start buckets; got {total_rows}"
    );

    // ---- audit doc § 1 named rows ----
    // T001..T018 → historical
    let hist = bucket_ids("historical");
    for n in 1..=18 {
        let id = format!("T{:03}", n);
        assert!(
            hist.contains(&id),
            "{id} must land in historical bucket; bucket={hist:?}"
        );
    }
    // T081 / T122 → needs_operator
    let needs_op = bucket_ids("needs_operator");
    assert!(needs_op.contains(&"T081".to_string()));
    assert!(needs_op.contains(&"T122".to_string()));
    // T125 / T127 → needs_operator (DeployCeremonyPending)
    assert!(needs_op.contains(&"T125".to_string()));
    assert!(needs_op.contains(&"T127".to_string()));
    // Live T138 → historical; synthetic accepted rows cover activation split.
    assert!(
        hist.contains(&"T138".to_string()),
        "T138 must land in historical; historical={hist:?}"
    );
    let wr = bucket_ids("would_run");
    assert!(
        wr.contains(&"T138_ACCEPTED_ACTIVE".to_string()),
        "T138_ACCEPTED_ACTIVE must land in would_run; would_run={wr:?}"
    );
    let inactive = bucket_ids("inactive");
    assert!(
        inactive.contains(&"T138_ACCEPTED_INACTIVE".to_string()),
        "T138_ACCEPTED_INACTIVE must land in inactive; inactive={inactive:?}"
    );
    // T139 → would_run
    assert!(wr.contains(&"T139".to_string()));

    // ---- bulk classes ----
    for n in 200..205 {
        assert!(hist.contains(&format!("T{n}")));
    }
    for n in 300..305 {
        assert!(hist.contains(&format!("T{n}")));
    }
    for n in 400..403 {
        assert!(hist.contains(&format!("T{n}")));
    }

    // ---- T610 / T611 (blocked) and T620 (rejected) ----
    let blocked = bucket_ids("blocked");
    assert!(blocked.contains(&"T610".to_string()));
    assert!(blocked.contains(&"T611".to_string()));
    assert!(hist.contains(&"T620".to_string()));

    // Smoke: every row across every bucket has a distinct display_id (no
    // duplicate-bucketing) and at least 5 buckets contain at least one row.
    let mut seen = std::collections::HashSet::new();
    let mut non_empty = 0usize;
    for k in [
        "would_run",
        "inactive",
        "needs_operator",
        "blocked",
        "historical",
    ] {
        let ids = bucket_ids(k);
        if !ids.is_empty() {
            non_empty += 1;
        }
        for id in ids {
            assert!(
                seen.insert(id.clone()),
                "duplicate display_id {id} across buckets"
            );
        }
    }
    assert_eq!(
        non_empty, 5,
        "all 5 buckets must be non-empty in the dirty snapshot"
    );
}

// ---------- AC6.1 (c) — linked_observations preservation ----------

#[test]
fn dirty_snapshot_t138_linked_observations_preserved_through_plan_start_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_path = tmp.path().join(".stores").join("db.sqlite");
    create_fixture_db(&db_path);

    let v = run_plan_start_json(tmp.path());
    // Live T138 now lives in historical, and the synthetic accepted rows cover
    // the would_run/inactive activation split. All carry through the
    // linked_observations payload from the seeded row (audit doc names L538,
    // L540 as the cross-store linkage shape).
    let bucket_entry = |bucket: &str, id: &str| -> Option<Value> {
        v.get(bucket).and_then(|x| x.as_array()).and_then(|arr| {
            arr.iter()
                .find(|e| e.get("display_id").and_then(|d| d.as_str()) == Some(id))
                .cloned()
        })
    };

    let t138_entry =
        bucket_entry("historical", "T138").expect("T138 must appear in historical JSON");
    let active_entry = bucket_entry("would_run", "T138_ACCEPTED_ACTIVE")
        .expect("T138_ACCEPTED_ACTIVE must appear in would_run JSON");
    let inactive_entry = bucket_entry("inactive", "T138_ACCEPTED_INACTIVE")
        .expect("T138_ACCEPTED_INACTIVE must appear in inactive JSON");
    assert_eq!(
        t138_entry.get("status").and_then(|s| s.as_str()),
        Some("schema_migrated")
    );
    assert_eq!(
        active_entry.get("status").and_then(|s| s.as_str()),
        Some("accepted")
    );
    assert_eq!(
        inactive_entry.get("status").and_then(|s| s.as_str()),
        Some("accepted")
    );

    let t138_linked = t138_entry
        .get("linked_observations")
        .and_then(|v| v.as_array())
        .expect("plan-start JSON entry must include linked_observations array");
    let t138_linked: Vec<String> = t138_linked
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    assert!(
        t138_linked.contains(&"L538".to_string()) && t138_linked.contains(&"L540".to_string()),
        "T138 plan-start JSON must preserve linked_observations [L538, L540]; got {t138_linked:?}"
    );

    let active_linked = active_entry
        .get("linked_observations")
        .and_then(|v| v.as_array())
        .expect("plan-start JSON entry must include linked_observations array");
    let active_linked: Vec<String> = active_linked
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    assert!(
        active_linked.contains(&"L538".to_string())
            && active_linked.contains(&"L540".to_string()),
        "T138 plan-start JSON must preserve linked_observations [L538, L540]; got {active_linked:?}"
    );

    let inactive_linked = inactive_entry
        .get("linked_observations")
        .and_then(|v| v.as_array())
        .expect("plan-start JSON entry must include linked_observations array");
    let inactive_linked: Vec<String> = inactive_linked
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    assert!(
        inactive_linked.contains(&"L538".to_string())
            && inactive_linked.contains(&"L540".to_string()),
        "T138_INACTIVE plan-start JSON must preserve linked_observations [L538, L540]; got {inactive_linked:?}"
    );
}
