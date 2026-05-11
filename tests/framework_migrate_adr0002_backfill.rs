use rusqlite::Connection;
use serde_json::json;
use stores::flow::adr0002_projection::{
    project_arch_review, project_intake, project_observation, ArchReviewRowInput, IntakeRowInput,
    ObsRowInput,
};
use stores::handlers::framework_migrate::apply_framework_drift;

fn pre_t148_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE substrate_migrations (applied_at TEXT, binary_version TEXT, table_name TEXT, column_name TEXT, ddl_applied TEXT);
        CREATE TABLE intake (
            id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, decision TEXT,
            routed_to_observation TEXT, routed_to_arch_review TEXT, duplicate_of TEXT
        );
        CREATE TABLE observations (
            id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, intent_contract TEXT,
            pending_architecture_review INTEGER, clearable_by_ruling TEXT, resolution_kind TEXT,
            resolution TEXT, merge_target_id TEXT, resolved_by TEXT, task_id TEXT
        );
        CREATE TABLE architecture_reviews (
            id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, verdict TEXT,
            source_observation TEXT, source_intake TEXT, supersedes TEXT, merge_target_id TEXT,
            cascade_decisions TEXT, updated_at TEXT
        );
        "#,
    ).unwrap();
    conn
}

fn seed_representative(conn: &Connection) {
    let intake_cases: [(
        &str,
        &str,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
    ); 8] = [
        ("I001", "draft", None, None, None, None),
        ("I002", "triaging", None, None, None, None),
        ("I003", "needs_info", Some("needs_info"), None, None, None),
        (
            "I004",
            "routed",
            Some("duplicate"),
            None,
            None,
            Some("I001"),
        ),
        ("I005", "routed", Some("fast_track"), None, None, None),
        (
            "I006",
            "routed",
            Some("normal_observation"),
            Some("L001"),
            None,
            None,
        ),
        (
            "I007",
            "routed",
            Some("arch_review_candidate"),
            Some("L002"),
            Some("A001"),
            None,
        ),
        ("I008", "dropped", Some("reject_noise"), None, None, None),
    ];
    for (id, status, decision, obs, arch, dup) in intake_cases {
        conn.execute(
            "INSERT INTO intake (display_id,status,decision,routed_to_observation,routed_to_arch_review,duplicate_of) VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![id, status, decision, obs, arch, dup],
        ).unwrap();
    }
    let obs_cases: [(
        &str,
        &str,
        Option<&str>,
        i64,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
        Option<&str>,
    ); 15] = [
        (
            "L001",
            "open",
            Some("draft"),
            0,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "L002",
            "needs_investigation",
            Some("ready"),
            0,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "L003",
            "investigating",
            None,
            0,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "L004",
            "investigated",
            Some("ready"),
            0,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "L005",
            "investigation_failed",
            Some("draft"),
            0,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "L006",
            "confirmed",
            Some("ready"),
            0,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "L007",
            "ready",
            Some("ready"),
            0,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "L008",
            "needs_info",
            Some("draft"),
            1,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "L009",
            "in_progress",
            Some("ready"),
            0,
            None,
            None,
            None,
            None,
            Some("T009"),
        ),
        (
            "L010",
            "resolved",
            Some("ready"),
            0,
            Some("addressed_by_task"),
            Some("T010"),
            None,
            None,
            None,
        ),
        (
            "L011",
            "resolved",
            Some("ready"),
            0,
            Some("addressed_by_commit"),
            Some("abc123"),
            None,
            None,
            None,
        ),
        (
            "L012",
            "resolved",
            Some("ready"),
            0,
            Some("addressed_by_observation"),
            Some("L001"),
            None,
            None,
            None,
        ),
        (
            "L013",
            "resolved",
            Some("ready"),
            0,
            Some("merged_with_cluster"),
            None,
            Some("L001"),
            None,
            None,
        ),
        (
            "L014",
            "resolved",
            Some("ready"),
            0,
            Some("superseded"),
            Some("L099"),
            None,
            None,
            None,
        ),
        (
            "L015",
            "wont_fix",
            Some("ready"),
            0,
            None,
            None,
            None,
            None,
            None,
        ),
    ];
    for (id, status, contract, pending, rk, res, merge, resolved_by, task) in obs_cases {
        let ic = contract.map(|c| json!({"contract_state": c}).to_string());
        conn.execute(
            "INSERT INTO observations (display_id,status,intent_contract,pending_architecture_review,resolution_kind,resolution,merge_target_id,resolved_by,task_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![id, status, ic, pending, rk, res, merge, resolved_by, task],
        ).unwrap();
    }
    let cascade_json = json!([{"target":"T777","decision":"create_followup"}]).to_string();
    let arch_cases: [(&str, &str, Option<&str>, Option<&str>, Option<&str>); 7] = [
        ("A001", "pending", None, Some("L008"), None),
        ("A002", "in_review", None, Some("L002"), None),
        (
            "A003",
            "awaiting_human_ratification",
            None,
            Some("L003"),
            None,
        ),
        (
            "A004",
            "verdict_issued",
            Some("allow_local_fix"),
            Some("L004"),
            None,
        ),
        (
            "A005",
            "verdict_issued",
            Some("create_primitive_task"),
            Some("L005"),
            Some(cascade_json.as_str()),
        ),
        ("A006", "withdrawn", None, Some("L006"), None),
        ("A007", "superseded", None, Some("L007"), None),
    ];
    for (id, status, verdict, src, cascade) in arch_cases {
        conn.execute(
            "INSERT INTO architecture_reviews (display_id,status,verdict,source_observation,cascade_decisions,updated_at) VALUES (?1,?2,?3,?4,?5,'now')",
            rusqlite::params![id, status, verdict, src, cascade],
        ).unwrap();
    }
}

#[test]
fn backfill_matches_projection_and_preserves_legacy_columns() {
    let conn = pre_t148_conn();
    seed_representative(&conn);
    apply_framework_drift(&conn).unwrap();

    let got: (String, String, Option<String>) = conn
        .query_row(
            "SELECT status,lifecycle,produced_artifact_kind FROM intake WHERE display_id='I006'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        got,
        (
            "routed".into(),
            project_intake(&IntakeRowInput {
                display_id: "I006",
                status: "routed",
                decision: Some("normal_observation"),
                routed_to_observation: Some("L001"),
                routed_to_arch_review: None,
                produced_task_id: None,
                produced_artifact_kind: Some("observation"),
                produced_artifact_id: Some("L001"),
                duplicate_of: None
            })
            .lifecycle
            .as_str()
            .into(),
            Some("observation".into())
        )
    );

    let obs: (String, String, i64, Option<String>, Option<String>, Option<String>) = conn.query_row(
        "SELECT lifecycle,contract_state,waiting,waiting_kind,open_architecture_review_id,addressed_by_commit_sha FROM observations WHERE display_id='L011'",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
    ).unwrap();
    let p = project_observation(
        &ObsRowInput {
            display_id: "L011",
            status: "resolved",
            contract_state: Some("approved"),
            pending_architecture_review: Some(false),
            clearable_by_ruling: None,
            open_architecture_review_id: None,
            resolution_kind: Some("addressed_by_commit"),
            resolution: Some("abc123"),
            merge_target_id: None,
            resolved_by: None,
            task_id: None,
            addressed_by_commit_sha: None,
            superseded_by_id: None,
        },
        None,
    );
    assert_eq!(
        obs,
        (
            p.lifecycle.as_str().into(),
            p.contract_state.as_str().into(),
            0,
            None,
            None,
            Some("abc123".into())
        )
    );

    let arch: (String, Option<String>, String, Option<String>) = conn.query_row(
        "SELECT lifecycle,outcome,linked_observation_ids,produced_task_id FROM architecture_reviews WHERE display_id='A005'",
        [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).unwrap();
    let ap = project_arch_review(&ArchReviewRowInput {
        display_id: "A005",
        status: "verdict_issued",
        verdict: Some("create_primitive_task"),
        source_observation: Some("L005"),
        source_intake: None,
        linked_observation_ids: vec!["L005"],
        supersedes: None,
        merge_target_id: None,
        produced_task_id: Some("T777"),
        superseded_by_id: None,
        updated_at: Some("now"),
    });
    assert_eq!(
        arch,
        (
            ap.lifecycle.as_str().into(),
            ap.outcome.map(|o| o.as_str().into()),
            "[\"L005\"]".into(),
            Some("T777".into())
        )
    );
}

#[test]
fn typed_reference_backfill_fields_are_deterministic() {
    let conn = pre_t148_conn();
    seed_representative(&conn);
    apply_framework_drift(&conn).unwrap();
    assert_eq!(
        conn.query_row::<String, _, _>(
            "SELECT contract_state FROM observations WHERE display_id='L001'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "draft"
    );
    assert_eq!(
        conn.query_row::<String, _, _>(
            "SELECT contract_state FROM observations WHERE display_id='L002'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "approved"
    );
    assert_eq!(
        conn.query_row::<String, _, _>(
            "SELECT contract_state FROM observations WHERE display_id='L003'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "none"
    );
    assert_eq!(
        conn.query_row::<String, _, _>(
            "SELECT superseded_by_id FROM observations WHERE display_id='L014'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "L099"
    );
    assert_eq!(
        conn.query_row::<String, _, _>(
            "SELECT open_architecture_review_id FROM observations WHERE display_id='L008'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "A001"
    );
    assert_eq!(
        conn.query_row::<String, _, _>(
            "SELECT produced_artifact_id FROM intake WHERE display_id='I007'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "L002"
    );
    assert_eq!(
        conn.query_row::<String, _, _>(
            "SELECT linked_observation_ids FROM architecture_reviews WHERE display_id='A001'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "[\"L008\"]"
    );
}

#[test]
#[should_panic(expected = "I999: unmapped status mystery")]
fn unknown_legacy_status_fails_loud() {
    let conn = pre_t148_conn();
    conn.execute(
        "INSERT INTO intake (display_id,status) VALUES ('I999','mystery')",
        [],
    )
    .unwrap();
    apply_framework_drift(&conn).unwrap();
}

#[test]
fn second_backfill_is_idempotent() {
    let conn = pre_t148_conn();
    seed_representative(&conn);
    apply_framework_drift(&conn).unwrap();
    let before: String = conn.query_row("SELECT group_concat(display_id || ':' || lifecycle || ':' || coalesce(outcome,''), '|') FROM observations", [], |r| r.get(0)).unwrap();
    apply_framework_drift(&conn).unwrap();
    let after: String = conn.query_row("SELECT group_concat(display_id || ':' || lifecycle || ':' || coalesce(outcome,''), '|') FROM observations", [], |r| r.get(0)).unwrap();
    assert_eq!(before, after);
}
