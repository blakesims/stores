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
        CREATE TABLE tasks (id INTEGER PRIMARY KEY AUTOINCREMENT, display_id TEXT, status TEXT, source_intake TEXT);
        "#,
    ).unwrap();
    conn
}

#[derive(Clone, Copy)]
struct IntakeCase {
    id: &'static str,
    status: &'static str,
    decision: Option<&'static str>,
    obs: Option<&'static str>,
    arch: Option<&'static str>,
    dup: Option<&'static str>,
    task: Option<&'static str>,
}
#[derive(Clone, Copy)]
struct ObsCase {
    id: &'static str,
    status: &'static str,
    contract: Option<&'static str>,
    pending: i64,
    rk: Option<&'static str>,
    res: Option<&'static str>,
    merge: Option<&'static str>,
    resolved_by: Option<&'static str>,
    task: Option<&'static str>,
    open_arch: Option<&'static str>,
    superseded_by: Option<&'static str>,
}
#[derive(Clone, Copy)]
struct ArchCase {
    id: &'static str,
    status: &'static str,
    verdict: Option<&'static str>,
    src: Option<&'static str>,
    cascade_task: Option<&'static str>,
}

fn intake_cases() -> Vec<IntakeCase> {
    vec![
        IntakeCase {
            id: "I001",
            status: "draft",
            decision: None,
            obs: None,
            arch: None,
            dup: None,
            task: None,
        },
        IntakeCase {
            id: "I002",
            status: "triaging",
            decision: None,
            obs: None,
            arch: None,
            dup: None,
            task: None,
        },
        IntakeCase {
            id: "I003",
            status: "needs_info",
            decision: Some("needs_info"),
            obs: None,
            arch: None,
            dup: None,
            task: None,
        },
        IntakeCase {
            id: "I004",
            status: "routed",
            decision: Some("duplicate"),
            obs: None,
            arch: None,
            dup: Some("I001"),
            task: None,
        },
        IntakeCase {
            id: "I005",
            status: "routed",
            decision: Some("fast_track"),
            obs: None,
            arch: None,
            dup: None,
            task: Some("T005"),
        },
        IntakeCase {
            id: "I006",
            status: "routed",
            decision: Some("normal_observation"),
            obs: Some("L001"),
            arch: None,
            dup: None,
            task: None,
        },
        IntakeCase {
            id: "I007",
            status: "routed",
            decision: Some("arch_review_candidate"),
            obs: Some("L002"),
            arch: Some("A001"),
            dup: None,
            task: None,
        },
        IntakeCase {
            id: "I008",
            status: "dropped",
            decision: Some("reject_noise"),
            obs: None,
            arch: None,
            dup: None,
            task: None,
        },
    ]
}

fn obs_cases() -> Vec<ObsCase> {
    vec![
        ObsCase {
            id: "L001",
            status: "open",
            contract: Some("draft"),
            pending: 0,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L002",
            status: "needs_investigation",
            contract: Some("ready"),
            pending: 0,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: Some("A002"),
            superseded_by: None,
        },
        ObsCase {
            id: "L003",
            status: "investigating",
            contract: None,
            pending: 0,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: Some("A003"),
            superseded_by: None,
        },
        ObsCase {
            id: "L004",
            status: "investigated",
            contract: Some("ready"),
            pending: 0,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L005",
            status: "investigation_failed",
            contract: Some("draft"),
            pending: 0,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L006",
            status: "confirmed",
            contract: Some("ready"),
            pending: 0,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L007",
            status: "ready",
            contract: Some("ready"),
            pending: 0,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L008",
            status: "needs_info",
            contract: Some("draft"),
            pending: 1,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: Some("A001"),
            superseded_by: None,
        },
        ObsCase {
            id: "L009",
            status: "in_progress",
            contract: Some("ready"),
            pending: 0,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: Some("T009"),
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L010",
            status: "resolved",
            contract: Some("ready"),
            pending: 0,
            rk: Some("addressed_by_task"),
            res: Some("T010"),
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L011",
            status: "resolved",
            contract: Some("ready"),
            pending: 0,
            rk: Some("addressed_by_commit"),
            res: Some("abc123"),
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L012",
            status: "resolved",
            contract: Some("ready"),
            pending: 0,
            rk: Some("addressed_by_observation"),
            res: Some("L001"),
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L013",
            status: "resolved",
            contract: Some("ready"),
            pending: 0,
            rk: Some("merged_with_cluster"),
            res: None,
            merge: Some("L001"),
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
        ObsCase {
            id: "L014",
            status: "resolved",
            contract: Some("ready"),
            pending: 0,
            rk: Some("superseded"),
            res: Some("L099"),
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: Some("L099"),
        },
        ObsCase {
            id: "L015",
            status: "wont_fix",
            contract: Some("ready"),
            pending: 0,
            rk: None,
            res: None,
            merge: None,
            resolved_by: None,
            task: None,
            open_arch: None,
            superseded_by: None,
        },
    ]
}

fn arch_cases() -> Vec<ArchCase> {
    vec![
        ArchCase {
            id: "A001",
            status: "pending",
            verdict: None,
            src: Some("L008"),
            cascade_task: None,
        },
        ArchCase {
            id: "A002",
            status: "in_review",
            verdict: None,
            src: Some("L002"),
            cascade_task: None,
        },
        ArchCase {
            id: "A003",
            status: "awaiting_human_ratification",
            verdict: None,
            src: Some("L003"),
            cascade_task: None,
        },
        ArchCase {
            id: "A004",
            status: "verdict_issued",
            verdict: Some("allow_local_fix"),
            src: Some("L004"),
            cascade_task: None,
        },
        ArchCase {
            id: "A005",
            status: "verdict_issued",
            verdict: Some("create_primitive_task"),
            src: Some("L005"),
            cascade_task: Some("T777"),
        },
        ArchCase {
            id: "A006",
            status: "withdrawn",
            verdict: None,
            src: Some("L006"),
            cascade_task: None,
        },
        ArchCase {
            id: "A007",
            status: "superseded",
            verdict: None,
            src: Some("L007"),
            cascade_task: None,
        },
        ArchCase {
            id: "A008",
            status: "verdict_issued",
            verdict: Some("reframe_contract"),
            src: Some("L008"),
            cascade_task: None,
        },
        ArchCase {
            id: "A009",
            status: "verdict_issued",
            verdict: Some("merge_with_cluster"),
            src: Some("L009"),
            cascade_task: None,
        },
        ArchCase {
            id: "A010",
            status: "verdict_issued",
            verdict: Some("block_pending_fixes"),
            src: Some("L010"),
            cascade_task: None,
        },
        ArchCase {
            id: "A011",
            status: "verdict_issued",
            verdict: Some("request_human_arch_decision"),
            src: Some("L011"),
            cascade_task: None,
        },
        ArchCase {
            id: "A012",
            status: "verdict_issued",
            verdict: Some("propose_doctrine_update"),
            src: Some("L012"),
            cascade_task: None,
        },
    ]
}

fn seed_representative(conn: &Connection) {
    for c in intake_cases() {
        conn.execute("INSERT INTO intake (display_id,status,decision,routed_to_observation,routed_to_arch_review,duplicate_of) VALUES (?1,?2,?3,?4,?5,?6)", rusqlite::params![c.id, c.status, c.decision, c.obs, c.arch, c.dup]).unwrap();
        if let Some(task) = c.task {
            conn.execute(
                "INSERT INTO tasks (display_id,status,source_intake) VALUES (?1,'complete',?2)",
                rusqlite::params![task, c.id],
            )
            .unwrap();
        }
    }
    for c in obs_cases() {
        let ic = c.contract.map(|v| json!({"contract_state": v}).to_string());
        conn.execute("INSERT INTO observations (display_id,status,intent_contract,pending_architecture_review,resolution_kind,resolution,merge_target_id,resolved_by,task_id) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)", rusqlite::params![c.id, c.status, ic, c.pending, c.rk, c.res, c.merge, c.resolved_by, c.task]).unwrap();
    }
    for c in arch_cases() {
        let cascade = c
            .cascade_task
            .map(|t| json!([{"target":t,"decision":"create_followup"}]).to_string());
        conn.execute("INSERT INTO architecture_reviews (display_id,status,verdict,source_observation,cascade_decisions,updated_at) VALUES (?1,?2,?3,?4,?5,'now')", rusqlite::params![c.id, c.status, c.verdict, c.src, cascade]).unwrap();
    }
}

fn contract(c: Option<&str>) -> Option<&'static str> {
    match c {
        Some("draft") => Some("draft"),
        Some("ready") => Some("approved"),
        _ => Some("none"),
    }
}
fn dominant_artifact(c: IntakeCase) -> (Option<&'static str>, Option<&'static str>) {
    if let Some(v) = c.obs {
        (Some("observation"), Some(v))
    } else if let Some(v) = c.arch {
        (Some("architecture_review"), Some(v))
    } else if let Some(v) = c.task {
        (Some("task"), Some(v))
    } else {
        (None, None)
    }
}

#[test]
fn backfill_matches_projection_and_preserves_legacy_columns() {
    let conn = pre_t148_conn();
    seed_representative(&conn);
    let legacy_intake: Vec<(String, String, Option<String>)> = conn
        .prepare("SELECT display_id,status,decision FROM intake ORDER BY display_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let legacy_obs: Vec<(String, String, Option<String>)> = conn
        .prepare("SELECT display_id,status,resolution_kind FROM observations ORDER BY display_id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let legacy_arch: Vec<(String, String, Option<String>, Option<String>)> = conn.prepare("SELECT display_id,status,verdict,source_observation FROM architecture_reviews ORDER BY display_id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<_, _>>().unwrap();

    apply_framework_drift(&conn).unwrap();

    assert_eq!(
        legacy_intake,
        conn.prepare("SELECT display_id,status,decision FROM intake ORDER BY display_id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<(String, String, Option<String>)>, _>>()
            .unwrap()
    );
    assert_eq!(
        legacy_obs,
        conn.prepare(
            "SELECT display_id,status,resolution_kind FROM observations ORDER BY display_id"
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<Vec<(String, String, Option<String>)>, _>>()
        .unwrap()
    );
    assert_eq!(legacy_arch, conn.prepare("SELECT display_id,status,verdict,source_observation FROM architecture_reviews ORDER BY display_id").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().collect::<Result<Vec<(String,String,Option<String>,Option<String>)>, _>>().unwrap());

    let mut checked = 0;
    for c in intake_cases() {
        let (kind, id) = dominant_artifact(c);
        let p = project_intake(&IntakeRowInput {
            display_id: c.id,
            status: c.status,
            decision: c.decision,
            routed_to_observation: c.obs,
            routed_to_arch_review: c.arch,
            produced_task_id: c.task,
            produced_artifact_kind: kind,
            produced_artifact_id: id,
            duplicate_of: c.dup,
        });
        let got: (String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>) = conn.query_row("SELECT lifecycle,waiting_kind,outcome,produced_observation_id,produced_architecture_review_id,produced_task_id,produced_artifact_kind,produced_artifact_id,duplicate_of_id FROM intake WHERE display_id=?1", [c.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?))).unwrap();
        assert_eq!(
            got,
            (
                p.lifecycle.as_str().into(),
                p.waiting.map(|w| w.as_str().into()),
                p.outcome.map(|o| o.as_str().into()),
                p.references.produced_observation_id,
                p.references.produced_architecture_review_id,
                p.references.produced_task_id,
                p.references.produced_artifact_kind,
                p.references.produced_artifact_id,
                p.references.duplicate_of_id
            ),
            "{}",
            c.id
        );
        checked += 1;
    }
    for c in obs_cases() {
        let p = project_observation(
            &ObsRowInput {
                display_id: c.id,
                status: c.status,
                contract_state: contract(c.contract),
                pending_architecture_review: Some(c.pending != 0),
                clearable_by_ruling: None,
                open_architecture_review_id: c.open_arch,
                resolution_kind: c.rk,
                resolution: c.res,
                merge_target_id: c.merge,
                resolved_by: c.resolved_by,
                task_id: c.task,
                addressed_by_commit_sha: None,
                superseded_by_id: c.superseded_by,
            },
            None,
        );
        let got: (String, String, i64, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>) = conn.query_row("SELECT lifecycle,contract_state,waiting,waiting_kind,outcome,open_architecture_review_id,addressed_by_task_id,addressed_by_commit_sha,superseded_by_id FROM observations WHERE display_id=?1", [c.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?))).unwrap();
        assert_eq!(
            got,
            (
                p.lifecycle.as_str().into(),
                p.contract_state.as_str().into(),
                if p.waiting.is_some() { 1 } else { 0 },
                p.waiting.map(|w| w.as_str().into()),
                p.outcome.map(|o| o.as_str().into()),
                p.references.open_architecture_review_id,
                p.references.addressed_by_task_id,
                p.references.addressed_by_commit_sha,
                p.references.superseded_by_id
            ),
            "{}",
            c.id
        );
        checked += 1;
    }
    for c in arch_cases() {
        let linked = c.src.map(|s| vec![s]).unwrap_or_default();
        let p = project_arch_review(&ArchReviewRowInput {
            display_id: c.id,
            status: c.status,
            verdict: c.verdict,
            source_observation: c.src,
            source_intake: None,
            linked_observation_ids: linked,
            supersedes: None,
            merge_target_id: None,
            produced_task_id: c.cascade_task,
            superseded_by_id: None,
            updated_at: Some("now"),
        });
        let got: (String, Option<String>, String, Option<String>, Option<String>) = conn.query_row("SELECT lifecycle,outcome,linked_observation_ids,produced_task_id,superseded_by_id FROM architecture_reviews WHERE display_id=?1", [c.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
        assert_eq!(
            got,
            (
                p.lifecycle.as_str().into(),
                p.outcome.map(|o| o.as_str().into()),
                serde_json::to_string(&p.references.linked_observation_ids).unwrap(),
                p.references.produced_task_id,
                p.references.superseded_by_id
            ),
            "{}",
            c.id
        );
        checked += 1;
    }
    assert!(
        checked >= 20,
        "checked {checked} projection-equivalent rows"
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
            "SELECT addressed_by_commit_sha FROM observations WHERE display_id='L011'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "abc123"
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
            "SELECT produced_task_id FROM intake WHERE display_id='I005'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "T005"
    );
    assert_eq!(
        conn.query_row::<String, _, _>(
            "SELECT produced_artifact_kind FROM intake WHERE display_id='I007'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "observation"
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
    assert_eq!(
        conn.query_row::<String, _, _>(
            "SELECT produced_task_id FROM architecture_reviews WHERE display_id='A005'",
            [],
            |r| r.get(0)
        )
        .unwrap(),
        "T777"
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
    let before: String = conn.query_row("SELECT json_group_array(json_object('d',display_id,'l',lifecycle,'o',outcome,'a',coalesce(produced_artifact_id,''))) FROM intake", [], |r| r.get(0)).unwrap();
    apply_framework_drift(&conn).unwrap();
    let after: String = conn.query_row("SELECT json_group_array(json_object('d',display_id,'l',lifecycle,'o',outcome,'a',coalesce(produced_artifact_id,''))) FROM intake", [], |r| r.get(0)).unwrap();
    assert_eq!(before, after);
}
