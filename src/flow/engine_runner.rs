//! Engine-runner observability substrate.
//!
//! Phase 1 only records per-iteration heartbeats and per-row actionability
//! state. These helpers intentionally write only `engine_runner_*` tables and
//! never mutate task lifecycle, transition history, or dispatch locks.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::BTreeMap;

use crate::codegen::ddl::quote_ident;
use crate::handlers::agents_run::pid_is_alive;
use crate::handlers::next_action::find_next_agent;
use crate::schema::Schema;
use crate::validate::EntryMap;

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

/// Schemas scanned by one engine-runner poll.
pub struct ScannerSchemas<'a> {
    pub tasks: &'a Schema,
    pub intake: &'a Schema,
    pub observations: &'a Schema,
}

/// Per-row scanner decision persisted to `engine_runner_actions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedRow {
    pub store: String,
    pub row_id: i64,
    pub classification: String,
    pub held_reason: Option<String>,
}

/// Result of one scanner pass before dispatch execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannerResult {
    pub summary: HeartbeatSummary,
    pub rows: Vec<ClassifiedRow>,
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

/// Scan active rows, classify actionability, and persist one heartbeat plus
/// per-row latest actionability records. This function is lifecycle read-only:
/// it writes only `engine_runner_heartbeats` and `engine_runner_actions`.
pub fn scan_and_record_actionability(
    conn: &Connection,
    schemas: ScannerSchemas<'_>,
    iteration: i64,
    started_at: &str,
) -> Result<ScannerResult> {
    let mut rows = Vec::new();
    rows.extend(scan_tasks(conn, schemas.tasks)?);
    rows.extend(scan_intake(conn, schemas.intake)?);
    rows.extend(scan_observations(conn, schemas.observations)?);

    let summary = HeartbeatSummary {
        iteration,
        saw_tasks: rows.iter().filter(|r| r.store == "tasks").count() as i64,
        saw_intake: rows.iter().filter(|r| r.store == "intake").count() as i64,
        saw_observations: rows.iter().filter(|r| r.store == "observations").count() as i64,
        actionable: rows
            .iter()
            .filter(|r| r.held_reason.is_none() && r.classification.starts_with("actionable_"))
            .count() as i64,
        held: rows.iter().filter(|r| r.held_reason.is_some()).count() as i64,
        dispatched: 0,
    };

    record_heartbeat(conn, summary, started_at)?;
    for row in &rows {
        upsert_actionability(
            conn,
            ActionabilityRecord {
                store: &row.store,
                row_id: row.row_id,
                classification: &row.classification,
                held_reason: row.held_reason.as_deref(),
                dispatched: false,
                last_logged_at: Some(started_at),
            },
            started_at,
        )?;
    }

    Ok(ScannerResult { summary, rows })
}

fn scan_tasks(conn: &Connection, schema: &Schema) -> Result<Vec<ClassifiedRow>> {
    let table = quote_ident(&schema.name);
    let sql = format!(
        "SELECT id, status, current_phase, current_cycle, tier_hint, plan, blocked_reason, drive_pid \
         FROM {table} WHERE status IN ('planning','plan_review','ready','executing','code_review','complete','in_review')"
    );
    let mut stmt = conn.prepare(&sql).context("prepare task scanner")?;
    let mut out = Vec::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<i64>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<i64>>(7)?,
        ))
    })?;

    let workflow = schema.workflow.as_ref();
    for row in rows {
        let (
            row_id,
            status,
            current_phase,
            current_cycle,
            tier_hint,
            plan,
            blocked_reason,
            drive_pid,
        ) = row?;
        let mut entry: EntryMap = BTreeMap::new();
        entry.insert("status".into(), json!(status));
        entry.insert("current_phase".into(), opt_i64_value(current_phase));
        entry.insert("current_cycle".into(), opt_i64_value(current_cycle));
        entry.insert("tier_hint".into(), opt_string_value(tier_hint));
        entry.insert("blocked_reason".into(), opt_string_value(blocked_reason));
        entry.insert("plan".into(), parse_json_text(plan));

        let next_agent = workflow.and_then(|wf| find_next_agent(wf, &status, &entry));
        let live_drive_owner = drive_pid
            .and_then(|pid| i32::try_from(pid).ok())
            .is_some_and(pid_is_alive);
        let (classification, held_reason) = if let Some(agent) = next_agent {
            if live_drive_owner {
                ("held".to_string(), Some("live_drive_owner".to_string()))
            } else if has_live_dispatch_lock(conn, &schema.name, row_id, &agent)? {
                ("held".to_string(), Some("live_dispatch_lock".to_string()))
            } else {
                ("actionable_task_redrive".to_string(), None)
            }
        } else {
            ("held".to_string(), Some("no_next_agent".to_string()))
        };
        out.push(ClassifiedRow {
            store: schema.name.clone(),
            row_id,
            classification,
            held_reason,
        });
    }
    Ok(out)
}

fn scan_intake(conn: &Connection, schema: &Schema) -> Result<Vec<ClassifiedRow>> {
    let table = quote_ident(&schema.name);
    let sql = format!("SELECT id, status FROM {table} WHERE status IN ('triaging','needs_info')");
    let mut stmt = conn.prepare(&sql).context("prepare intake scanner")?;
    let workflow = schema.workflow.as_ref();
    let mut out = Vec::new();
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (row_id, status) = row?;
        let entry: EntryMap = BTreeMap::new();
        let next_agent = workflow.and_then(|wf| find_next_agent(wf, &status, &entry));
        let (classification, held_reason) = match next_agent.as_deref() {
            Some("gatekeeper") | Some("recon") => ("actionable_intake_builtin".to_string(), None),
            Some(_) => (
                "held".to_string(),
                Some("no_built_in_entrypoint".to_string()),
            ),
            None => ("held".to_string(), Some("no_next_agent".to_string())),
        };
        out.push(ClassifiedRow {
            store: schema.name.clone(),
            row_id,
            classification,
            held_reason,
        });
    }
    Ok(out)
}

fn scan_observations(conn: &Connection, schema: &Schema) -> Result<Vec<ClassifiedRow>> {
    let table = quote_ident(&schema.name);
    let sql = format!(
        "SELECT id, status, intent_contract, risk_class, approval_policy \
         FROM {table} WHERE status IN ('open','needs_investigation','investigating','investigated','confirmed','ready','needs_info','in_progress')"
    );
    let mut stmt = conn.prepare(&sql).context("prepare observations scanner")?;
    let mut out = Vec::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
        ))
    })?;
    for row in rows {
        let (row_id, status, contract_text, risk_class, approval_policy) = row?;
        let contract = parse_json_text(contract_text);
        let contract_state = contract
            .get("contract_state")
            .and_then(Value::as_str)
            .unwrap_or("");
        let approved_by = contract.get("approved_by").and_then(Value::as_str);
        let approved_at = contract.get("approved_at").and_then(Value::as_str);
        let arch_surface = risk_class.as_deref() == Some("architecture")
            || approval_policy.as_deref() == Some("architecture");
        let awaiting_human_contract =
            contract_state == "draft" || approved_by.is_none() || approved_at.is_none();
        let (classification, held_reason) = if arch_surface {
            ("held".to_string(), Some("needs_architect".to_string()))
        } else if awaiting_human_contract {
            ("held".to_string(), Some("needs_human".to_string()))
        } else if matches!(status.as_str(), "investigated" | "confirmed" | "ready") {
            ("held".to_string(), Some("needs_human".to_string()))
        } else if status == "needs_investigation" {
            ("actionable_observation_investigator".to_string(), None)
        } else {
            (
                "held".to_string(),
                Some("no_built_in_entrypoint".to_string()),
            )
        };
        out.push(ClassifiedRow {
            store: schema.name.clone(),
            row_id,
            classification,
            held_reason,
        });
    }
    Ok(out)
}

fn has_live_dispatch_lock(
    conn: &Connection,
    store: &str,
    row_id: i64,
    agent: &str,
) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM dispatch_locks \
         WHERE store=?1 AND row_id=?2 AND agent_name=?3 AND finished_at IS NULL",
        rusqlite::params![store, row_id, agent],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

fn opt_i64_value(v: Option<i64>) -> Value {
    v.map(Value::from).unwrap_or(Value::Null)
}

fn opt_string_value(v: Option<String>) -> Value {
    v.map(Value::from).unwrap_or(Value::Null)
}

fn parse_json_text(v: Option<String>) -> Value {
    v.and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::schema::Schema;

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

    fn scanner_schemas() -> (Schema, Schema, Schema) {
        (
            Schema::from_yaml(include_str!("../../stores/tasks/schema.yaml")).unwrap(),
            Schema::from_yaml(include_str!("../../stores/intake_items/schema.yaml")).unwrap(),
            Schema::from_yaml(include_str!("../../stores/observations/schema.yaml")).unwrap(),
        )
    }

    fn open_scanner_db() -> (Connection, Schema, Schema, Schema) {
        let (tasks, intake, observations) = scanner_schemas();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&ddl_for(&tasks)).unwrap();
        conn.execute_batch(&ddl_for(&intake)).unwrap();
        conn.execute_batch(&ddl_for(&observations)).unwrap();
        (conn, tasks, intake, observations)
    }

    fn insert_task(conn: &Connection, display_id: &str, status: &str) -> i64 {
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, title, slug, current_phase, current_cycle, tier_hint, plan) \
             VALUES (?1, ?2, '2026-05-07T00:00:00Z', '2026-05-07T00:00:00Z', 'Task', 'task', 1, 1, 'T2', ?3)",
            rusqlite::params![display_id, status, r#"{"phases":[{"name":"p1"}]}"#],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_intake(conn: &Connection, display_id: &str, status: &str) -> i64 {
        conn.execute(
            "INSERT INTO intake (display_id, status, created_at, updated_at, summary, source_agent, captured_at, captured_week) \
             VALUES (?1, ?2, '2026-05-07T00:00:00Z', '2026-05-07T00:00:00Z', 'Intake', 'tester', '2026-05-07T00:00:00Z', 'w18-d4')",
            rusqlite::params![display_id, status],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_observation(
        conn: &Connection,
        display_id: &str,
        status: &str,
        contract: &str,
        risk_class: &str,
        approval_policy: &str,
    ) -> i64 {
        conn.execute(
            "INSERT INTO observations (display_id, status, created_at, updated_at, summary, source, priority, captured_at, captured_week, intent_contract, risk_class, approval_policy) \
             VALUES (?1, ?2, '2026-05-07T00:00:00Z', '2026-05-07T00:00:00Z', 'Observation', 'qa', 'normal', '2026-05-07T00:00:00Z', 'w18-d4', ?3, ?4, ?5)",
            rusqlite::params![display_id, status, contract, risk_class, approval_policy],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn scanner_counts_tasks_intake_observations_and_actionable_task_redrive() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T901", "executing");
        insert_intake(&conn, "I901", "triaging");
        insert_observation(
            &conn,
            "L901",
            "needs_investigation",
            "{}",
            "normal",
            "human",
        );

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            1,
            "2026-05-07T00:00:00Z",
        )
        .unwrap();

        assert_eq!(result.summary.saw_tasks, 1);
        assert_eq!(result.summary.saw_intake, 1);
        assert_eq!(result.summary.saw_observations, 1);
        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.classification == "actionable_task_redrive"
            && r.held_reason.is_none()));

        let persisted: String = conn
            .query_row(
                "SELECT classification FROM engine_runner_actions WHERE store='tasks' AND row_id=?1",
                rusqlite::params![task_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(persisted, "actionable_task_redrive");
    }

    #[test]
    fn scanner_treats_stale_drive_pid_as_orphaned_task_redrive() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T905", "executing");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![999_999_999_i64, task_id],
        )
        .unwrap();

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            5,
            "2026-05-07T00:04:00Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.classification == "actionable_task_redrive"
            && r.held_reason.is_none()));
    }

    #[test]
    fn scanner_holds_live_drive_pid_as_owner() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T906", "code_review");
        conn.execute(
            "UPDATE tasks SET drive_pid=?1 WHERE id=?2",
            rusqlite::params![std::process::id() as i64, task_id],
        )
        .unwrap();

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            6,
            "2026-05-07T00:05:00Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.classification == "held"
            && r.held_reason.as_deref() == Some("live_drive_owner")));
    }

    #[test]
    fn scanner_holds_u_moment_observation_without_lifecycle_writes() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let original_contract = r#"{"contract_state":"draft","approved_by":null,"approved_at":null,"objective":"draft"}"#;
        let obs_id = insert_observation(
            &conn,
            "L902",
            "investigating",
            original_contract,
            "normal",
            "human",
        );

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            2,
            "2026-05-07T00:01:00Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "observations"
            && r.row_id == obs_id
            && r.classification == "held"
            && r.held_reason.as_deref() == Some("needs_human")));
        let row: (String, String, String, String) = conn
            .query_row(
                "SELECT status, intent_contract, risk_class, approval_policy FROM observations WHERE id=?1",
                rusqlite::params![obs_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(row.0, "investigating");
        assert_eq!(row.1, original_contract);
        assert_eq!(row.2, "normal");
        assert_eq!(row.3, "human");

        let forbidden: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE verb IN ('accept','reject','resume','amend','abandon','confirm','ratify') OR verb LIKE 'architecture%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(forbidden, 0);
    }

    #[test]
    fn scanner_holds_non_investigating_draft_contract_as_needs_human() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let original_contract = r#"{"contract_state":"draft","approved_by":null,"approved_at":null,"objective":"draft"}"#;
        let obs_id = insert_observation(
            &conn,
            "L904",
            "open",
            original_contract,
            "normal",
            "human",
        );

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            7,
            "2026-05-07T00:06:00Z",
        )
        .unwrap();

        assert!(result.rows.iter().any(|r| r.store == "observations"
            && r.row_id == obs_id
            && r.classification == "held"
            && r.held_reason.as_deref() == Some("needs_human")));
        let row: (String, String) = conn
            .query_row(
                "SELECT status, intent_contract FROM observations WHERE id=?1",
                rusqlite::params![obs_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(row.0, "open");
        assert_eq!(row.1, original_contract);

        let forbidden: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE verb IN ('accept','reject','resume','amend','abandon','confirm','ratify') OR verb LIKE 'architecture%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(forbidden, 0);
    }

    #[test]
    fn scanner_holds_architecture_surface_for_architect() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let obs_id = insert_observation(
            &conn,
            "L903",
            "confirmed",
            "{}",
            "architecture",
            "architecture",
        );

        scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            3,
            "2026-05-07T00:02:00Z",
        )
        .unwrap();

        let held: Option<String> = conn
            .query_row(
                "SELECT held_reason FROM engine_runner_actions WHERE store='observations' AND row_id=?1",
                rusqlite::params![obs_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(held.as_deref(), Some("needs_architect"));
    }

    #[test]
    fn scanner_respects_live_task_dispatch_lock() {
        let (conn, tasks, intake, observations) = open_scanner_db();
        let task_id = insert_task(&conn, "T904", "code_review");
        conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, claimed_at, claimed_by) \
             VALUES ('tasks', ?1, 'T904', 'code_reviewer', '2026-05-07T00:00:00Z', 'daemon')",
            rusqlite::params![task_id],
        )
        .unwrap();

        let result = scan_and_record_actionability(
            &conn,
            ScannerSchemas {
                tasks: &tasks,
                intake: &intake,
                observations: &observations,
            },
            4,
            "2026-05-07T00:03:00Z",
        )
        .unwrap();

        assert_eq!(result.summary.actionable, 0);
        assert!(result.rows.iter().any(|r| r.store == "tasks"
            && r.row_id == task_id
            && r.held_reason.as_deref() == Some("live_dispatch_lock")));
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
