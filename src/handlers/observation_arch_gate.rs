use anyhow::{bail, Context, Result};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::Value;

use crate::codegen::ddl::quote_ident;
use crate::schema::actor::Actor;
use crate::validate::EntryMap;

use super::row::now_iso8601;

#[derive(Debug, Clone)]
struct RulingRow {
    display_id: String,
    status: String,
    kind: String,
    verdict: String,
    verdict_issued_at: String,
    source_observation: Option<String>,
    merge_target_id: Option<String>,
}

/// Enforces the observations U1 architecture-review gate before confirmed→ready
/// ratification. If a pending observation is clearable by the referenced A###
/// ruling, this injects pending_architecture_review=false into the transition
/// diff so the clearance is persisted atomically with U1.
pub(crate) fn enforce_u1_architecture_gate(
    tx: &Transaction,
    observation_id: &str,
    persisted: &EntryMap,
    merged: &mut EntryMap,
    diff: &mut EntryMap,
) -> Result<()> {
    let persisted_pending = value_is_true(persisted.get("pending_architecture_review"));
    if !persisted_pending {
        return Ok(());
    }

    let ruling_id = merged
        .get("clearable_by_ruling")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "pending_architecture_review=true blocks U1 ratification: architecture review clearance requires clearable_by_ruling=A###"
            )
        })?;

    let ruling = read_ruling(tx, ruling_id)?.ok_or_else(|| {
        anyhow::anyhow!(
            "pending_architecture_review=true blocks U1 ratification: architecture review {ruling_id} not found"
        )
    })?;

    if ruling.status != "verdict_issued" {
        bail!(
            "pending_architecture_review=true blocks U1 ratification: architecture review {} is status={}, expected verdict_issued",
            ruling.display_id,
            ruling.status
        );
    }
    if ruling.source_observation.as_deref() != Some(observation_id) {
        bail!(
            "pending_architecture_review=true blocks U1 ratification: architecture review {} does not target observation {}",
            ruling.display_id,
            observation_id
        );
    }

    match (ruling.kind.as_str(), ruling.verdict.as_str()) {
        ("interpret", "allow_local_fix") => {}
        ("amend", "propose_doctrine_update") => {}
        ("interpret", "reframe_contract") => enforce_reframe_ack(observation_id, merged, &ruling)?,
        (_, "merge_with_cluster") => bail!(
            "pending_architecture_review=true cannot be cleared by U1: architecture review {} merged observation {} into cluster target {:?}",
            ruling.display_id,
            observation_id,
            ruling.merge_target_id
        ),
        (_, other) => bail!(
            "pending_architecture_review=true blocks U1 ratification: architecture review {} verdict {other} is not a clearing verdict",
            ruling.display_id
        ),
    }

    diff.insert(
        "pending_architecture_review".to_string(),
        Value::Bool(false),
    );
    merged.insert(
        "pending_architecture_review".to_string(),
        Value::Bool(false),
    );
    Ok(())
}

fn value_is_true(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(true)) => true,
        Some(Value::Number(n)) => n.as_i64() == Some(1),
        Some(Value::String(s)) => matches!(s.as_str(), "true" | "1" | "yes"),
        _ => false,
    }
}

fn enforce_reframe_ack(observation_id: &str, merged: &EntryMap, ruling: &RulingRow) -> Result<()> {
    let ack = merged
        .get("reframe_acknowledged_against")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if ack != ruling.display_id {
        bail!(
            "pending_architecture_review=true blocks U1 ratification: reframe_contract requires reframe_acknowledged_against={} for observation {}",
            ruling.display_id,
            observation_id
        );
    }
    let intent_updated_at = merged
        .get("intent_contract")
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("updated_at"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if intent_updated_at <= ruling.verdict_issued_at.as_str() {
        bail!(
            "pending_architecture_review=true blocks U1 ratification: reframe_contract requires intent_contract.updated_at > verdict_issued_at for architecture review {}",
            ruling.display_id
        );
    }
    Ok(())
}

/// Applies merge_with_cluster verdict side effects to the source observation in
/// the same transaction as issue-verdict: resolved terminal, A### provenance,
/// target L###, distinct resolution_kind, and pending gate cleared.
pub(crate) fn apply_merge_with_cluster_verdict(
    tx: &Transaction,
    ruling_id: &str,
    merged_ruling: &EntryMap,
    actor: Actor,
) -> Result<()> {
    if merged_ruling.get("verdict").and_then(|v| v.as_str()) != Some("merge_with_cluster") {
        return Ok(());
    }
    let source = merged_ruling
        .get("source_observation")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("merge_with_cluster verdict requires source_observation"))?;
    let target = merged_ruling
        .get("merge_target_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("merge_with_cluster verdict requires merge_target_id"))?;
    if source == target {
        bail!(
            "merge_with_cluster verdict requires merge_target_id distinct from source_observation"
        );
    }

    let status: Option<String> = tx
        .query_row(
            "SELECT status FROM observations WHERE display_id = ?1",
            [source],
            |r| r.get(0),
        )
        .optional()
        .context("merge_with_cluster: read source observation")?;
    let status = status.ok_or_else(|| {
        anyhow::anyhow!("merge_with_cluster source observation {source} not found")
    })?;
    if status == "resolved" {
        bail!("merge_with_cluster source observation {source} is already resolved/merged");
    }

    let now = now_iso8601();
    let actor_s = actor.to_string();
    tx.execute(
        "UPDATE observations SET updated_at=?1, updated_by=?2, status='resolved', lifecycle='closed', waiting=0, waiting_kind=NULL, outcome='merged_with_cluster', pending_architecture_review=0, resolved_at=?1, resolved_by=?3, merge_target_id=?4, resolution_kind='merged_with_cluster' WHERE display_id=?5",
        rusqlite::params![now, actor_s, ruling_id, target, source],
    )
    .context("merge_with_cluster: update source observation")?;
    Ok(())
}

fn read_ruling(tx: &Transaction, display_id: &str) -> Result<Option<RulingRow>> {
    let table = quote_ident("architecture_reviews");
    tx.query_row(
        &format!(
            "SELECT display_id,status,kind,COALESCE(verdict,''),COALESCE(verdict_issued_at,''),source_observation,merge_target_id FROM {table} WHERE display_id=?1"
        ),
        [display_id],
        |r| {
            Ok(RulingRow {
                display_id: r.get(0)?,
                status: r.get(1)?,
                kind: r.get(2)?,
                verdict: r.get(3)?,
                verdict_issued_at: r.get(4)?,
                source_observation: r.get(5)?,
                merge_target_id: r.get(6)?,
            })
        },
    )
    .optional()
    .context("read architecture review ruling")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let obs =
            crate::schema::Schema::from_yaml(include_str!("../../stores/observations/schema.yaml"))
                .unwrap();
        let arch = crate::schema::Schema::from_yaml(include_str!(
            "../../stores/architecture_reviews/schema.yaml"
        ))
        .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&obs))
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&arch))
            .unwrap();
        conn.execute(
            "INSERT INTO observations (display_id,status,created_at,updated_at,created_by,updated_by,summary,source,priority,captured_at,captured_week,pending_architecture_review,intent_contract) VALUES ('L001','confirmed','now','now','human','human','s','dev','normal','2026-05-07','w19-d4',1,?1)",
            [json!({"contract_state":"ready","updated_at":"2026-05-07T10:00:00Z"}).to_string()],
        ).unwrap();
        conn
    }

    fn pending_entry(clearable: Option<&str>) -> EntryMap {
        let mut m = EntryMap::new();
        m.insert("pending_architecture_review".into(), Value::Bool(true));
        m.insert(
            "intent_contract".into(),
            json!({"contract_state":"ready","updated_at":"2026-05-07T10:00:00Z"}),
        );
        if let Some(a) = clearable {
            m.insert("clearable_by_ruling".into(), Value::String(a.into()));
        }
        m
    }

    fn insert_ruling(
        conn: &Connection,
        id: &str,
        status: &str,
        kind: &str,
        verdict: &str,
        issued: &str,
    ) {
        conn.execute(
            "INSERT INTO architecture_reviews (display_id,status,created_at,updated_at,created_by,updated_by,kind,summary,source_observation,verdict,verdict_issued_at) VALUES (?1,?2,'now','now','ai_with_human','ai_with_human',?3,'s','L001',?4,?5)",
            rusqlite::params![id,status,kind,verdict,issued],
        ).unwrap();
    }

    #[test]
    fn pending_true_rejects_without_clearable_ruling() {
        let conn = setup();
        let tx = conn.unchecked_transaction().unwrap();
        let mut merged = pending_entry(None);
        let mut diff = EntryMap::new();
        let err =
            enforce_u1_architecture_gate(&tx, "L001", &merged.clone(), &mut merged, &mut diff)
                .unwrap_err();
        assert!(err.to_string().contains("pending_architecture_review"));
        assert!(err.to_string().contains("architecture review"));
    }

    #[test]
    fn pending_true_blocks_post_confirm_auto_ratify_path() {
        let conn = setup();
        let obs =
            crate::schema::Schema::from_yaml(include_str!("../../stores/observations/schema.yaml"))
                .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let row_id: i64 = tx
            .query_row(
                "SELECT id FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let persisted = pending_entry(None);
        let mut merged = pending_entry(None);
        // Regression: a confirm diff may try to clear this field before the
        // post-confirm auto-ratify hook. The gate must use persisted=true.
        merged.insert("pending_architecture_review".into(), Value::Bool(false));
        merged.insert(
            "intent_contract".into(),
            json!({
                "contract_state":"ready",
                "approved_by":"blake",
                "approved_at":"2026-05-07T10:00:00Z",
                "updated_at":"2026-05-07T10:00:00Z"
            }),
        );
        let err = crate::handlers::transition::maybe_auto_ratify_observation(
            &tx,
            &obs,
            row_id,
            "L001",
            &merged,
            Some(&persisted),
            None,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("pending_architecture_review"));
        assert!(err.to_string().contains("architecture review"));
    }

    #[test]
    fn persisted_pending_true_cannot_be_bypassed_by_same_diff_clear() {
        let conn = setup();
        let tx = conn.unchecked_transaction().unwrap();
        let persisted = pending_entry(None);
        let mut merged = pending_entry(None);
        merged.insert("pending_architecture_review".into(), Value::Bool(false));
        let mut diff = EntryMap::new();
        diff.insert("pending_architecture_review".into(), Value::Bool(false));
        let err = enforce_u1_architecture_gate(&tx, "L001", &persisted, &mut merged, &mut diff)
            .unwrap_err();
        assert!(err.to_string().contains("pending_architecture_review"));
        assert!(err.to_string().contains("architecture review"));
    }

    #[test]
    fn allow_local_fix_clears_only_after_verdict_issued() {
        let conn = setup();
        insert_ruling(
            &conn,
            "A001",
            "in_review",
            "interpret",
            "allow_local_fix",
            "",
        );
        let tx = conn.unchecked_transaction().unwrap();
        let mut merged = pending_entry(Some("A001"));
        let mut diff = EntryMap::new();
        assert!(
            enforce_u1_architecture_gate(&tx, "L001", &merged.clone(), &mut merged, &mut diff)
                .is_err()
        );
        tx.commit().unwrap();

        conn.execute("UPDATE architecture_reviews SET status='verdict_issued', verdict_issued_at='2026-05-07T09:00:00Z' WHERE display_id='A001'", []).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let mut merged = pending_entry(Some("A001"));
        let mut diff = EntryMap::new();
        enforce_u1_architecture_gate(&tx, "L001", &merged.clone(), &mut merged, &mut diff).unwrap();
        assert_eq!(
            diff.get("pending_architecture_review"),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn human_ratified_amend_clears_only_after_verdict_issued() {
        let conn = setup();
        insert_ruling(
            &conn,
            "A002",
            "awaiting_human_ratification",
            "amend",
            "propose_doctrine_update",
            "",
        );
        let tx = conn.unchecked_transaction().unwrap();
        let mut merged = pending_entry(Some("A002"));
        let mut diff = EntryMap::new();
        assert!(
            enforce_u1_architecture_gate(&tx, "L001", &merged.clone(), &mut merged, &mut diff)
                .is_err()
        );
        tx.commit().unwrap();

        conn.execute("UPDATE architecture_reviews SET status='verdict_issued', ratified_by='human', ratified_at='2026-05-07T09:00:00Z', verdict_issued_at='2026-05-07T09:00:00Z' WHERE display_id='A002'", []).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let mut merged = pending_entry(Some("A002"));
        let mut diff = EntryMap::new();
        enforce_u1_architecture_gate(&tx, "L001", &merged.clone(), &mut merged, &mut diff).unwrap();
        assert_eq!(
            diff.get("pending_architecture_review"),
            Some(&Value::Bool(false))
        );
    }

    #[test]
    fn reframe_requires_newer_contract_timestamp_and_ack() {
        let conn = setup();
        insert_ruling(
            &conn,
            "A003",
            "verdict_issued",
            "interpret",
            "reframe_contract",
            "2026-05-07T09:00:00Z",
        );
        let tx = conn.unchecked_transaction().unwrap();
        let mut merged = pending_entry(Some("A003"));
        let mut diff = EntryMap::new();
        assert!(
            enforce_u1_architecture_gate(&tx, "L001", &merged.clone(), &mut merged, &mut diff)
                .is_err()
        );

        merged.insert(
            "reframe_acknowledged_against".into(),
            Value::String("A003".into()),
        );
        merged.insert(
            "intent_contract".into(),
            json!({"updated_at":"2026-05-07T09:00:00Z"}),
        );
        assert!(
            enforce_u1_architecture_gate(&tx, "L001", &merged.clone(), &mut merged, &mut diff)
                .is_err()
        );

        merged.insert(
            "intent_contract".into(),
            json!({"updated_at":"2026-05-07T09:00:01Z"}),
        );
        enforce_u1_architecture_gate(&tx, "L001", &merged.clone(), &mut merged, &mut diff).unwrap();
    }

    #[test]
    fn merge_verdict_resolves_source_and_blocks_later_u1_by_status() {
        let conn = setup();
        conn.execute(
            "INSERT INTO architecture_reviews (display_id,status,created_at,updated_at,created_by,updated_by,kind,summary,source_observation,linked_observation_ids) VALUES ('A004','in_review','now','now','ai_with_human','ai_with_human','interpret','s','L001',?1)",
            [json!(["L001"]).to_string()],
        ).unwrap();
        let arch = crate::schema::Schema::from_yaml(include_str!(
            "../../stores/architecture_reviews/schema.yaml"
        ))
        .unwrap();
        let mut cmd = clap::Command::new("issue-verdict")
            .arg(clap::Arg::new("display_id").required(true).index(1));
        for leaf in crate::schema::flatten::leaf_args(&arch).unwrap() {
            cmd = cmd.arg(clap::Arg::new(leaf.cli_name.clone()).long(leaf.cli_name).required(false));
        }
        let matches = cmd.get_matches_from([
            "issue-verdict",
            "A004",
            "--kind",
            "interpret",
            "--verdict",
            "merge_with_cluster",
            "--rationale",
            "same cluster",
            "--source-observation",
            "L001",
            "--merge-target-id",
            "L999",
        ]);
        crate::handlers::architecture_reviews::run_issue_verdict(
            &arch,
            &conn,
            &matches,
            Actor::AiWithHuman.into(),
        )
        .unwrap();

        let obs_row: (String, i64, String, String, String) = conn.query_row(
            "SELECT status,pending_architecture_review,resolved_by,resolution_kind,outcome FROM observations WHERE display_id='L001'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        ).unwrap();
        assert_eq!(
            obs_row,
            (
                "resolved".into(),
                0,
                "A004".into(),
                "merged_with_cluster".into(),
                "merged_with_cluster".into()
            )
        );
        let arch_row: (String, String, String, String) = conn.query_row(
            "SELECT status,verdict,outcome,merge_target_id FROM architecture_reviews WHERE display_id='A004'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
        assert_eq!(arch_row.0, "verdict_issued");
        assert_eq!(arch_row.2, "merged_with_cluster");

        let obs_projection = crate::flow::adr0002_projection::project_observation(
            &crate::flow::adr0002_projection::ObsRowInput {
                display_id: "L001",
                status: &obs_row.0,
                contract_state: Some("approved"),
                pending_architecture_review: Some(false),
                clearable_by_ruling: None,
                open_architecture_review_id: None,
                resolution_kind: Some(&obs_row.3),
                resolution: None,
                merge_target_id: Some(&arch_row.3),
                resolved_by: Some(&obs_row.2),
                task_id: None,
                addressed_by_commit_sha: None,
                superseded_by_id: None,
            },
            None,
        );
        let arch_projection = crate::flow::adr0002_projection::project_arch_review(
            &crate::flow::adr0002_projection::ArchReviewRowInput {
                display_id: "A004",
                status: &arch_row.0,
                verdict: Some(&arch_row.1),
                source_observation: Some("L001"),
                source_intake: None,
                linked_observation_ids: vec!["L001"],
                supersedes: None,
                merge_target_id: Some(&arch_row.3),
                produced_task_id: None,
                superseded_by_id: None,
                updated_at: None,
            },
        );
        assert_eq!(
            obs_projection.outcome,
            Some(crate::flow::adr0002_projection::ObsOutcome::MergedWithCluster)
        );
        assert_eq!(
            arch_projection.outcome,
            Some(crate::flow::adr0002_projection::ArchReviewOutcome::MergedWithCluster)
        );
    }
}
