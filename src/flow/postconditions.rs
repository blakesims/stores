//! Named-predicate registry mapping `postcondition_id` strings to pure
//! `(Connection, args, ctx) -> Result<bool>` functions over substrate state.
//!
//! Phase 2 of T050: registry only, no behavioural wiring. The framework will
//! consult `lookup()` once a subscriber returns and verify the predicate
//! before stamping `dispatch_locks.terminal_reason='ok'` (Phase 4+).

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

/// Pure substrate-state predicate. `args` carries the postcondition's bound
/// arguments (e.g. `{"display_id": "T123"}` or `{"observation_id": "L045"}`);
/// `ctx` is an optional companion value (the dispatch_locks row, the trigger
/// row, …) passed through unchanged for postconditions that need it.
pub type PostconditionFn = fn(&Connection, &Value, Option<&Value>) -> Result<bool>;

/// Catalogue of registered postcondition identifiers. `Other(String)` is the
/// escape hatch for postconditions declared by out-of-tree subscribers; those
/// are not resolvable by `lookup()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostconditionId {
    TaskExistsForLinkedObservation,
    TaskWorkspaceExists,
    DrivePidRecordedOrTerminal,
    CargoInstalledState,
    SchemaMigratedState,
    Other(String),
}

impl PostconditionId {
    pub fn as_str(&self) -> &str {
        match self {
            PostconditionId::TaskExistsForLinkedObservation => "task_exists_for_linked_observation",
            PostconditionId::TaskWorkspaceExists => "task_workspace_exists",
            PostconditionId::DrivePidRecordedOrTerminal => "drive_pid_recorded_or_terminal",
            PostconditionId::CargoInstalledState => "cargo_installed_state",
            PostconditionId::SchemaMigratedState => "schema_migrated_state",
            PostconditionId::Other(s) => s.as_str(),
        }
    }

    pub fn parse(s: &str) -> PostconditionId {
        match s {
            "task_exists_for_linked_observation" => PostconditionId::TaskExistsForLinkedObservation,
            "task_workspace_exists" => PostconditionId::TaskWorkspaceExists,
            "drive_pid_recorded_or_terminal" => PostconditionId::DrivePidRecordedOrTerminal,
            "cargo_installed_state" => PostconditionId::CargoInstalledState,
            "schema_migrated_state" => PostconditionId::SchemaMigratedState,
            other => PostconditionId::Other(other.to_string()),
        }
    }
}

/// Resolve a postcondition_id string to its pure-fn implementation. Returns
/// `None` for unknown ids (including `Other(_)` strings without a built-in
/// implementation).
pub fn lookup(id: &str) -> Option<PostconditionFn> {
    match id {
        "task_exists_for_linked_observation" => Some(task_exists_for_linked_observation),
        "task_workspace_exists" => Some(task_workspace_exists),
        "drive_pid_recorded_or_terminal" => Some(drive_pid_recorded_or_terminal),
        "cargo_installed_state" => Some(cargo_installed_state),
        "schema_migrated_state" => Some(schema_migrated_state),
        _ => None,
    }
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// True iff some `tasks` row references the given `observation_id` (LXXX) in
/// its `linked_observations` list_fk column. The list is JSON-encoded text;
/// a `LIKE '%LXXX%'` probe is sufficient because L-ids are unambiguous.
///
/// Args: `{"observation_id": "L045"}`.
pub fn task_exists_for_linked_observation(
    conn: &Connection,
    args: &Value,
    _ctx: Option<&Value>,
) -> Result<bool> {
    let obs_id = arg_str(args, "observation_id")
        .ok_or_else(|| anyhow::anyhow!("missing arg: observation_id"))?;
    let pattern = format!("%{}%", obs_id);
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE linked_observations LIKE ?1",
        rusqlite::params![pattern],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// True iff the named task row exists with a non-empty `workspace_path`.
///
/// Args: `{"display_id": "T123"}`.
pub fn task_workspace_exists(
    conn: &Connection,
    args: &Value,
    _ctx: Option<&Value>,
) -> Result<bool> {
    let display_id =
        arg_str(args, "display_id").ok_or_else(|| anyhow::anyhow!("missing arg: display_id"))?;
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks \
         WHERE display_id = ?1 AND workspace_path IS NOT NULL AND workspace_path <> ''",
        rusqlite::params![display_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// True iff the named task row records a positive `drive_pid` OR is parked in
/// a terminal-from-drive's-perspective status (`blocked`, `accepted`,
/// `deploy_blocked`, `schema_migrated`). The disjunction covers both the
/// happy path (drive recorded its pid) and the post-drive terminal states the
/// row may already have advanced into by the time the postcondition runs.
///
/// Args: `{"display_id": "T123"}`.
pub fn drive_pid_recorded_or_terminal(
    conn: &Connection,
    args: &Value,
    _ctx: Option<&Value>,
) -> Result<bool> {
    let display_id =
        arg_str(args, "display_id").ok_or_else(|| anyhow::anyhow!("missing arg: display_id"))?;
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks \
         WHERE display_id = ?1 AND ( \
           (drive_pid IS NOT NULL AND drive_pid > 0) \
           OR status IN ('blocked','accepted','deploy_blocked','schema_migrated') \
         )",
        rusqlite::params![display_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// True iff the named task row is at `cargo_installed`.
///
/// Args: `{"display_id": "T123"}`.
pub fn cargo_installed_state(
    conn: &Connection,
    args: &Value,
    _ctx: Option<&Value>,
) -> Result<bool> {
    let display_id =
        arg_str(args, "display_id").ok_or_else(|| anyhow::anyhow!("missing arg: display_id"))?;
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE display_id = ?1 AND status = 'cargo_installed'",
        rusqlite::params![display_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// True iff the named task row is at `schema_migrated`.
///
/// Args: `{"display_id": "T123"}`.
pub fn schema_migrated_state(
    conn: &Connection,
    args: &Value,
    _ctx: Option<&Value>,
) -> Result<bool> {
    let display_id =
        arg_str(args, "display_id").ok_or_else(|| anyhow::anyhow!("missing arg: display_id"))?;
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tasks WHERE display_id = ?1 AND status = 'schema_migrated'",
        rusqlite::params![display_id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::schema::Schema;
    use serde_json::json;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        conn
    }

    fn insert_task(
        conn: &Connection,
        display_id: &str,
        status: &str,
        workspace_path: Option<&str>,
        drive_pid: Option<i64>,
        linked_observations: Option<&str>,
    ) {
        let now = "2026-05-06T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, drive_pid, linked_observations, contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'test', 't', 'feat/x', ?3, ?4, ?5, ?6, ?7, ?7, 'framework', 'framework')",
            rusqlite::params![display_id, status, workspace_path, drive_pid, linked_observations, contract, now],
        )
        .unwrap();
    }

    #[test]
    fn task_exists_for_linked_observation_satisfies_and_fails() {
        let conn = fresh_db();
        insert_task(&conn, "T100", "planning", None, None, Some(r#"["L045"]"#));
        insert_task(&conn, "T101", "planning", None, None, Some(r#"["L099"]"#));

        assert!(
            task_exists_for_linked_observation(&conn, &json!({"observation_id": "L045"}), None)
                .unwrap()
        );
        assert!(
            !task_exists_for_linked_observation(&conn, &json!({"observation_id": "L777"}), None)
                .unwrap()
        );
    }

    #[test]
    fn task_workspace_exists_satisfies_and_fails() {
        let conn = fresh_db();
        insert_task(&conn, "T200", "planning", Some("/tmp/repo"), None, None);
        insert_task(&conn, "T201", "planning", Some(""), None, None);

        assert!(task_workspace_exists(&conn, &json!({"display_id": "T200"}), None).unwrap());
        assert!(!task_workspace_exists(&conn, &json!({"display_id": "T201"}), None).unwrap());
        assert!(!task_workspace_exists(&conn, &json!({"display_id": "T999"}), None).unwrap());
    }

    #[test]
    fn drive_pid_recorded_or_terminal_satisfies_and_fails() {
        let conn = fresh_db();
        // Satisfies via positive drive_pid.
        insert_task(&conn, "T300", "executing", None, Some(12345), None);
        // Satisfies via terminal status (blocked).
        insert_task(&conn, "T301", "blocked", None, None, None);
        // Fails: planning with no pid.
        insert_task(&conn, "T302", "planning", None, None, None);
        // Fails: drive_pid=0 is not "recorded".
        insert_task(&conn, "T303", "planning", None, Some(0), None);

        assert!(
            drive_pid_recorded_or_terminal(&conn, &json!({"display_id": "T300"}), None).unwrap()
        );
        assert!(
            drive_pid_recorded_or_terminal(&conn, &json!({"display_id": "T301"}), None).unwrap()
        );
        assert!(
            !drive_pid_recorded_or_terminal(&conn, &json!({"display_id": "T302"}), None).unwrap()
        );
        assert!(
            !drive_pid_recorded_or_terminal(&conn, &json!({"display_id": "T303"}), None).unwrap()
        );
    }

    #[test]
    fn cargo_installed_state_satisfies_and_fails() {
        let conn = fresh_db();
        insert_task(&conn, "T400", "cargo_installed", None, None, None);
        insert_task(&conn, "T401", "accepted", None, None, None);

        assert!(cargo_installed_state(&conn, &json!({"display_id": "T400"}), None).unwrap());
        assert!(!cargo_installed_state(&conn, &json!({"display_id": "T401"}), None).unwrap());
    }

    #[test]
    fn schema_migrated_state_satisfies_and_fails() {
        let conn = fresh_db();
        insert_task(&conn, "T500", "schema_migrated", None, None, None);
        insert_task(&conn, "T501", "cargo_installed", None, None, None);

        assert!(schema_migrated_state(&conn, &json!({"display_id": "T500"}), None).unwrap());
        assert!(!schema_migrated_state(&conn, &json!({"display_id": "T501"}), None).unwrap());
    }

    #[test]
    fn lookup_returns_some_for_known_ids_and_none_for_unknown() {
        for id in [
            "task_exists_for_linked_observation",
            "task_workspace_exists",
            "drive_pid_recorded_or_terminal",
            "cargo_installed_state",
            "schema_migrated_state",
        ] {
            assert!(lookup(id).is_some(), "lookup({}) returned None", id);
        }
        assert!(lookup("does_not_exist").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn postcondition_id_round_trip() {
        for id in [
            "task_exists_for_linked_observation",
            "task_workspace_exists",
            "drive_pid_recorded_or_terminal",
            "cargo_installed_state",
            "schema_migrated_state",
        ] {
            assert_eq!(PostconditionId::parse(id).as_str(), id);
        }
        // Other(_) preserves the unknown string.
        match PostconditionId::parse("custom_thing") {
            PostconditionId::Other(s) => assert_eq!(s, "custom_thing"),
            other => panic!("expected Other; got {:?}", other),
        }
    }
}
