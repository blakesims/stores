//! Engine-runner observability substrate.
//!
//! Phase 1 only records per-iteration heartbeats and per-row actionability
//! state. These helpers intentionally write only `engine_runner_*` tables and
//! never mutate task lifecycle, transition history, or dispatch locks.

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Counters persisted once per engine-runner poll iteration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeartbeatSummary {
    pub iteration: i64,
    pub saw_tasks: i64,
    pub saw_intake: i64,
    pub saw_observations: i64,
    pub actionable: i64,
    pub held: i64,
    pub dispatched: i64,
}

/// Latest actionability state for one substrate-visible row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionabilityRecord<'a> {
    pub store: &'a str,
    pub row_id: i64,
    pub classification: &'a str,
    pub held_reason: Option<&'a str>,
    pub dispatched: bool,
    pub last_logged_at: Option<&'a str>,
}

/// Insert one durable heartbeat row for a poll iteration.
pub fn record_heartbeat(
    conn: &Connection,
    summary: HeartbeatSummary,
    started_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO engine_runner_heartbeats \
         (iteration, started_at, saw_tasks, saw_intake, saw_observations, actionable, held, dispatched) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            summary.iteration,
            started_at,
            summary.saw_tasks,
            summary.saw_intake,
            summary.saw_observations,
            summary.actionable,
            summary.held,
            summary.dispatched,
        ],
    )
    .context("record engine_runner_heartbeats")?;
    Ok(())
}

/// Upsert the latest actionability state for one row.
pub fn upsert_actionability(
    conn: &Connection,
    record: ActionabilityRecord<'_>,
    updated_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO engine_runner_actions \
         (store, row_id, classification, held_reason, dispatched, last_logged_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(store, row_id) DO UPDATE SET \
         classification=excluded.classification, \
         held_reason=excluded.held_reason, \
         dispatched=excluded.dispatched, \
         last_logged_at=excluded.last_logged_at, \
         updated_at=excluded.updated_at",
        rusqlite::params![
            record.store,
            record.row_id,
            record.classification,
            record.held_reason,
            if record.dispatched { 1 } else { 0 },
            record.last_logged_at,
            updated_at,
        ],
    )
    .context("upsert engine_runner_actions")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::SUBSTRATE_DDL;

    #[test]
    fn actionability_upsert_replaces_latest_state_for_row() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();

        upsert_actionability(
            &conn,
            ActionabilityRecord {
                store: "tasks",
                row_id: 7,
                classification: "held",
                held_reason: Some("needs_human"),
                dispatched: false,
                last_logged_at: Some("2026-05-07T00:00:00Z"),
            },
            "2026-05-07T00:00:01Z",
        )
        .unwrap();
        upsert_actionability(
            &conn,
            ActionabilityRecord {
                store: "tasks",
                row_id: 7,
                classification: "actionable",
                held_reason: Some("orphaned_next_agent"),
                dispatched: true,
                last_logged_at: Some("2026-05-07T00:01:00Z"),
            },
            "2026-05-07T00:01:01Z",
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM engine_runner_actions WHERE store='tasks' AND row_id=7",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let row: (String, Option<String>, i64, String) = conn
            .query_row(
                "SELECT classification, held_reason, dispatched, updated_at \
                 FROM engine_runner_actions WHERE store='tasks' AND row_id=7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "actionable".to_string(),
                Some("orphaned_next_agent".to_string()),
                1,
                "2026-05-07T00:01:01Z".to_string(),
            )
        );
    }

    #[test]
    fn heartbeat_helper_inserts_only_heartbeat_row() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        record_heartbeat(
            &conn,
            HeartbeatSummary {
                iteration: 1,
                saw_tasks: 2,
                saw_intake: 3,
                saw_observations: 4,
                actionable: 5,
                held: 6,
                dispatched: 7,
            },
            "2026-05-07T00:00:00Z",
        )
        .unwrap();
        let row: (i64, i64, i64) = conn
            .query_row(
                "SELECT saw_tasks, held, dispatched FROM engine_runner_heartbeats \
                 WHERE iteration=1 AND started_at='2026-05-07T00:00:00Z'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, (2, 6, 7));
    }
}
