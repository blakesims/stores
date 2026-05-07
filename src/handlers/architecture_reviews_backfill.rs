use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

use crate::schema::{actor::Actor, Schema};
use crate::validate::EntryMap;

/// Idempotently backfill legacy arch-review-candidate observations into A### rows.
///
/// Scans observations whose legacy identity was either tag
/// `arch-review-candidate`, `notes.pending_architecture_review=true`, or the
/// typed `pending_architecture_review` column.  For each source L### without an
/// existing architecture_reviews.source_observation row, creates one pending
/// interpret ruling and marks the observation pending in the same transaction.
pub(crate) fn run_backfill(conn: &Connection) -> Result<usize> {
    if !table_exists(conn, "observations")? || !table_exists(conn, "architecture_reviews")? {
        return Ok(0);
    }
    let tx = conn
        .unchecked_transaction()
        .context("architecture_reviews backfill: begin tx")?;
    let created = run_backfill_in_tx(&tx)?;
    tx.commit()
        .context("architecture_reviews backfill: commit tx")?;
    Ok(created)
}

/// Test/internal deterministic entry point; caller owns the transaction.
pub(crate) fn run_backfill_in_tx(tx: &Transaction) -> Result<usize> {
    let candidates = legacy_candidates(tx)?;
    let mut created = 0usize;
    for candidate in candidates {
        if existing_arch_review(tx, &candidate.display_id)?.is_some() {
            ensure_pending_marker(tx, &candidate.display_id, None, candidate.notes.as_deref())?;
            continue;
        }
        let arch_id = insert_architecture_review(tx, &candidate)?;
        ensure_pending_marker(
            tx,
            &candidate.display_id,
            Some(&arch_id),
            candidate.notes.as_deref(),
        )?;
        created += 1;
    }
    Ok(created)
}

#[derive(Debug)]
struct Candidate {
    display_id: String,
    summary: String,
    cluster_key: Option<String>,
    notes: Option<String>,
}

fn legacy_candidates(tx: &Transaction) -> Result<Vec<Candidate>> {
    let mut stmt = tx.prepare(
        "SELECT display_id, summary, cluster_key, tags, notes, pending_architecture_review \
         FROM observations \
         WHERE COALESCE(pending_architecture_review, 0) = 1 \
            OR COALESCE(tags, '') LIKE '%arch-review-candidate%' \
            OR COALESCE(notes, '') LIKE '%pending_architecture_review%' \
         ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<i64>>(5)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (display_id, summary, cluster_key, tags, notes, pending) = row?;
        if pending == Some(1) || has_legacy_tag(tags.as_deref()) || notes_pending(notes.as_deref()) {
            out.push(Candidate {
                display_id,
                summary,
                cluster_key,
                notes,
            });
        }
    }
    Ok(out)
}

fn existing_arch_review(tx: &Transaction, source_observation: &str) -> Result<Option<String>> {
    tx.query_row(
        "SELECT display_id FROM architecture_reviews WHERE source_observation = ?1 ORDER BY id LIMIT 1",
        params![source_observation],
        |r| r.get(0),
    )
    .optional()
    .context("architecture_reviews backfill: lookup existing A###")
}

fn insert_architecture_review(tx: &Transaction, candidate: &Candidate) -> Result<String> {
    let schema = architecture_reviews_schema()?;
    let mut entry = EntryMap::new();
    entry.insert("kind".to_string(), Value::String("interpret".to_string()));
    entry.insert("summary".to_string(), Value::String(candidate.summary.clone()));
    entry.insert(
        "source_observation".to_string(),
        Value::String(candidate.display_id.clone()),
    );
    if let Some(cluster_key) = &candidate.cluster_key {
        entry.insert("cluster_key".to_string(), Value::String(cluster_key.clone()));
    }
    super::add::add_row_in_tx(tx, &schema, entry, Actor::Framework)
}

fn ensure_pending_marker(
    tx: &Transaction,
    observation_id: &str,
    arch_id: Option<&str>,
    existing_notes: Option<&str>,
) -> Result<()> {
    let mut notes = existing_notes
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| json!({}));
    if let Some(aid) = arch_id {
        notes["architecture_reviews_backfill"] = json!({ "ruling": aid });
    }
    let notes_raw = serde_json::to_string(&notes)?;
    tx.execute(
        "UPDATE observations \
         SET pending_architecture_review = 1, notes = ?2, updated_at = ?3, updated_by = 'framework' \
         WHERE display_id = ?1",
        params![observation_id, notes_raw, super::row::now_iso8601()],
    )
    .context("architecture_reviews backfill: mark observation pending")?;
    Ok(())
}

fn has_legacy_tag(tags: Option<&str>) -> bool {
    tags.and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.as_array().cloned())
        .map(|arr| arr.iter().any(|v| v.as_str() == Some("arch-review-candidate")))
        .unwrap_or_else(|| tags.unwrap_or("").contains("arch-review-candidate"))
}

fn notes_pending(notes: Option<&str>) -> bool {
    notes
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .and_then(|v| v.get("pending_architecture_review").and_then(|b| b.as_bool()).or_else(|| {
            v.get("architecture_review")
                .and_then(|o| o.get("pending_architecture_review"))
                .and_then(|b| b.as_bool())
        }))
        == Some(true)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    let exists: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            params![table],
            |r| r.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

fn architecture_reviews_schema() -> Result<Schema> {
    Schema::from_yaml(include_str!(
        "../../stores/architecture_reviews/schema.yaml"
    ))
    .context("parse bundled architecture_reviews schema")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        let obs = Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap();
        let arch = Schema::from_yaml(include_str!("../../stores/architecture_reviews/schema.yaml")).unwrap();
        conn.execute_batch(&ddl_for(&obs)).unwrap();
        conn.execute_batch(&ddl_for(&arch)).unwrap();
        conn
    }

    #[test]
    fn backfill_is_idempotent_for_legacy_tagged_observation() {
        let conn = setup();
        conn.execute(
            "INSERT INTO observations (display_id,status,created_at,updated_at,created_by,updated_by,summary,source,priority,captured_at,captured_week,tags,pending_architecture_review) \
             VALUES ('L001','open','now','now','ai_with_human','ai_with_human','legacy arch','dev','normal','now','w19-d2','[\"arch-review-candidate\"]',0)",
            [],
        )
        .unwrap();

        assert_eq!(run_backfill(&conn).unwrap(), 1);
        assert_eq!(run_backfill(&conn).unwrap(), 0);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM architecture_reviews", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        let pending: i64 = conn
            .query_row(
                "SELECT pending_architecture_review FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 1);
    }
}
