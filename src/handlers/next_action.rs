/// `next-action` handler — read-only workflow verb.
///
/// Returns which agent should act next on a workflow-shaped store entry,
/// along with the 9-key response shape (AC4.1 / AC4.2).
///
/// JSON shape (all 9 keys always present; null when not applicable):
/// ```json
/// {
///   "id": "T003",
///   "status": "executing",
///   "current_phase": 2,
///   "current_cycle": 1,
///   "next_agent": "executor",
///   "blocked": false,
///   "blocked_reason": null,
///   "claimed_by": null,
///   "claimed_at": null
/// }
/// ```
///
/// AC4.6: When `status == "blocked"`, `next_agent` is null and `blocked` is true.
/// AC4.7: Errors with a clear message on non-workflow stores (gated at CLI layer,
///         but double-checked here in case the handler is called directly).
use anyhow::{bail, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::schema::{actor::InvokerCtx, workflow::{StateAction, Workflow}, Schema};

use super::row::read_row;

/// Structured output returned by `compute`.  All 9 keys from the AC4.1/AC4.2 contract.
#[derive(Debug, Serialize, Deserialize)]
pub struct NextActionOutput {
    pub id: String,
    pub status: String,
    pub current_phase: Value,
    pub current_cycle: Value,
    pub next_agent: Option<String>,
    pub blocked: bool,
    pub blocked_reason: Value,
    pub claimed_by: Value,
    pub claimed_at: Value,
}

/// Find the first `DispatchAgent` role for the given status in the workflow's
/// `on_state` map.  Returns `None` for unknown states, blocked, or states with
/// no `DispatchAgent` action.  Engine-fired actions (`Increment`, `TransitionTo`)
/// are never returned.
pub fn find_next_agent(workflow: &Workflow, status: &str) -> Option<String> {
    if status == "blocked" {
        return None;
    }
    workflow.on_state.get(status).and_then(|actions| {
        actions.iter().find_map(|a| {
            if let StateAction::DispatchAgent(role) = a {
                Some(role.clone())
            } else {
                None
            }
        })
    })
}

/// Pure logic: read the row and compute the 9-key output.
/// Does NOT print anything.  `run` calls this and formats the result.
pub(crate) fn compute(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
) -> Result<NextActionOutput> {
    // AC4.7: must have a workflow declaration.
    let workflow = match &schema.workflow {
        Some(wf) => wf,
        None => bail!(
            "store '{}' has no workflow declaration; verb only works on workflow-shaped stores",
            schema.name
        ),
    };

    let (_id, entry) = read_row(schema, conn, display_id)?;

    let status = entry
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let current_phase = entry.get("current_phase").cloned().unwrap_or(Value::Null);
    let current_cycle = entry.get("current_cycle").cloned().unwrap_or(Value::Null);
    let claimed_by = entry.get("claimed_by").cloned().unwrap_or(Value::Null);
    let claimed_at = entry.get("claimed_at").cloned().unwrap_or(Value::Null);
    let blocked_reason = entry.get("blocked_reason").cloned().unwrap_or(Value::Null);

    let is_blocked = crate::handlers::is_blocked(&status, blocked_reason.as_str());
    let next_agent = if is_blocked {
        None
    } else {
        find_next_agent(workflow, &status)
    };

    Ok(NextActionOutput {
        id: display_id.to_string(),
        status,
        current_phase,
        current_cycle,
        next_agent,
        blocked: is_blocked,
        blocked_reason,
        claimed_by,
        claimed_at,
    })
}

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: InvokerCtx,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let json_flag = matches.get_flag("json");

    // AC4.7 guard + all logic live in compute().
    let out = compute(schema, conn, display_id)?;

    if json_flag {
        let json_out = json!({
            "id": out.id,
            "status": out.status,
            "current_phase": out.current_phase,
            "current_cycle": out.current_cycle,
            "next_agent": out.next_agent,
            "blocked": out.blocked,
            "blocked_reason": out.blocked_reason,
            "claimed_by": out.claimed_by,
            "claimed_at": out.claimed_at,
        });
        println!("{}", serde_json::to_string_pretty(&json_out)?);
    } else {
        // Text form: key: value lines (same 9 keys).
        println!("id: {}", out.id);
        println!("status: {}", out.status);
        println!("current_phase: {}", json_value_to_text(&out.current_phase));
        println!("current_cycle: {}", json_value_to_text(&out.current_cycle));
        println!(
            "next_agent: {}",
            out.next_agent.as_deref().unwrap_or("null")
        );
        println!("blocked: {}", out.blocked);
        println!("blocked_reason: {}", json_value_to_text(&out.blocked_reason));
        println!("claimed_by: {}", json_value_to_text(&out.claimed_by));
        println!("claimed_at: {}", json_value_to_text(&out.claimed_at));
    }

    Ok(())
}

/// Render a JSON Value as a text value for the key: value output.
/// null → "null", strings without quotes, numbers without quotes.
fn json_value_to_text(v: &Value) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::quote_ident;
    use crate::db;
    use crate::schema::Schema;
    use serde_json::json;
    use tempfile::tempdir;

    /// A workflow schema without reserved-column duplicates for DB tests.
    fn wf_schema() -> Schema {
        let yaml = r#"
name: wf_tasks
id_format: "WF{:03d}"
lifecycle:
  states: [planning, executing, blocked, done]
  transitions:
    - from: planning
      to: executing
      verb: start
      actor: ai_autonomous
fields:
  - name: title
    type: text
    required: true
  - name: current_phase
    type: integer
    actor: framework
  - name: current_cycle
    type: integer
    actor: framework
workflow:
  agent_roles:
    planner:
      description: "Creates the implementation plan"
    executor:
      description: "Implements the plan"
  briefing_templates:
    planner: templates/planner-brief.md.tpl
    executor: templates/executor-brief.md.tpl
  on_state:
    planning:
      - dispatch_agent: planner
    executing:
      - dispatch_agent: executor
  submit_targets: {}
  max_revise_cycles: 3
"#;
        Schema::from_yaml(yaml).unwrap()
    }

    fn open_db_with_schema(schema: &Schema) -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.db");
        let conn = db::open(&db_file).unwrap();
        let ddl = crate::codegen::ddl::ddl_for(schema);
        conn.execute_batch(&ddl).unwrap();
        (dir, conn)
    }

    fn insert_wf_row(
        conn: &Connection,
        schema: &Schema,
        display_id: &str,
        status: &str,
        current_phase: i64,
        current_cycle: i64,
    ) {
        const RESERVED: &[&str] = &[
            "display_id", "status", "created_at", "updated_at", "created_by", "updated_by",
        ];

        let mut cols: Vec<String> = vec![
            "display_id".to_string(),
            "status".to_string(),
            "created_at".to_string(),
            "updated_at".to_string(),
            "created_by".to_string(),
            "updated_by".to_string(),
        ];
        for f in &schema.fields {
            if !RESERVED.contains(&f.name.as_str()) {
                cols.push(f.name.clone());
            }
        }

        let mut params: Vec<rusqlite::types::Value> = vec![
            rusqlite::types::Value::Text(display_id.to_string()),
            rusqlite::types::Value::Text(status.to_string()),
            rusqlite::types::Value::Text("2026-01-01T00:00:00Z".to_string()),
            rusqlite::types::Value::Text("2026-01-01T00:00:00Z".to_string()),
            rusqlite::types::Value::Text("human".to_string()),
            rusqlite::types::Value::Text("human".to_string()),
        ];

        for f in &schema.fields {
            if RESERVED.contains(&f.name.as_str()) {
                continue;
            }
            let v = match f.name.as_str() {
                "title" => rusqlite::types::Value::Text("Test Task".to_string()),
                "current_phase" => rusqlite::types::Value::Integer(current_phase),
                "current_cycle" => rusqlite::types::Value::Integer(current_cycle),
                _ => rusqlite::types::Value::Null,
            };
            params.push(v);
        }

        let placeholders: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            quote_ident(&schema.name),
            cols.join(", "),
            placeholders.join(", ")
        );

        conn.execute(&sql, rusqlite::params_from_iter(params)).unwrap();
    }

    // AC4.1: executing row → next_agent: executor, blocked: false
    #[test]
    fn next_action_executing_returns_executor() {
        let schema = wf_schema();
        let (_dir, conn) = open_db_with_schema(&schema);
        insert_wf_row(&conn, &schema, "WF001", "executing", 2, 1);

        let out = compute(&schema, &conn, "WF001").unwrap();
        assert_eq!(out.status, "executing");
        assert_eq!(out.next_agent, Some("executor".to_string()));
        assert!(!out.blocked);
        assert_eq!(out.current_phase, json!(2));
        assert_eq!(out.current_cycle, json!(1));
        // AC4.2: JSON round-trip — all 9 keys present
        let v = serde_json::to_value(&out).unwrap();
        for key in &["id","status","current_phase","current_cycle","next_agent","blocked","blocked_reason","claimed_by","claimed_at"] {
            assert!(v.get(key).is_some(), "missing key in JSON output: {key}");
        }
    }

    // AC4.1: planning row → next_agent: planner
    #[test]
    fn next_action_planning_returns_planner() {
        let schema = wf_schema();
        let (_dir, conn) = open_db_with_schema(&schema);
        insert_wf_row(&conn, &schema, "WF002", "planning", 1, 0);

        let out = compute(&schema, &conn, "WF002").unwrap();
        assert_eq!(out.next_agent, Some("planner".to_string()));
        assert!(!out.blocked);
    }

    // AC4.6: blocked row → blocked: true, next_agent: null
    #[test]
    fn next_action_blocked_returns_null_agent() {
        let schema = wf_schema();
        let (_dir, conn) = open_db_with_schema(&schema);
        insert_wf_row(&conn, &schema, "WF003", "blocked", 1, 1);

        let out = compute(&schema, &conn, "WF003").unwrap();
        assert!(out.blocked);
        assert_eq!(out.next_agent, None);
        // JSON round-trip: blocked: true, next_agent: null
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v["blocked"], json!(true));
        assert_eq!(v["next_agent"], Value::Null);
    }

    // Bug 1 / T005-P1: blocked_reason shapes — status drives the predicate
    //
    // Parametric over blocked_reason ∈ {NULL, "", "real reason"} × status ∈
    // {"blocked", "executing"}.  The predicate must track status, never reason.
    //
    // Note: wf_schema() does not declare blocked_reason as a schema field, so we
    // add the column via ALTER TABLE after DDL setup, then UPDATE after insert
    // (same pattern as drive.rs:1118-1121).
    #[test]
    fn next_action_blocked_reason_shapes() {
        let schema = wf_schema();
        let (_dir, conn) = open_db_with_schema(&schema);

        // Add blocked_reason column (not in wf_schema() fields, so DDL omits it).
        conn.execute_batch(&format!(
            "ALTER TABLE {} ADD COLUMN blocked_reason TEXT",
            schema.name
        ))
        .unwrap();

        // Insert rows for each combination.
        // reason shapes: NULL, "", "real reason"
        let reason_shapes: &[(&str, Option<&str>)] = &[
            ("null",   None),
            ("empty",  Some("")),
            ("real",   Some("real reason")),
        ];

        for (label, reason) in reason_shapes {
            // --- status = "executing" → blocked must be false ---
            let exec_id = format!("WF_{label}_exec");
            insert_wf_row(&conn, &schema, &exec_id, "executing", 1, 1);
            if let Some(r) = reason {
                conn.execute(
                    &format!("UPDATE {} SET blocked_reason = ?1 WHERE display_id = ?2", quote_ident(&schema.name)),
                    rusqlite::params![r, exec_id],
                )
                .unwrap();
            }

            let out = compute(&schema, &conn, &exec_id).unwrap();
            assert!(
                !out.blocked,
                "status=executing, reason={label} → blocked must be false; got blocked={}",
                out.blocked
            );

            // --- status = "blocked" → blocked must be true ---
            let blk_id = format!("WF_{label}_blk");
            insert_wf_row(&conn, &schema, &blk_id, "blocked", 1, 1);
            if let Some(r) = reason {
                conn.execute(
                    &format!("UPDATE {} SET blocked_reason = ?1 WHERE display_id = ?2", quote_ident(&schema.name)),
                    rusqlite::params![r, blk_id],
                )
                .unwrap();
            }

            let out = compute(&schema, &conn, &blk_id).unwrap();
            assert!(
                out.blocked,
                "status=blocked, reason={label} → blocked must be true; got blocked={}",
                out.blocked
            );
        }
    }

    // AC4.7: non-workflow schema → compute returns error naming the store
    #[test]
    fn next_action_no_workflow_errors() {
        let yaml = r#"
name: obs
id_format: "O{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let (_dir, conn) = open_db_with_schema(&schema);
        // compute() must return an error; the message names the store.
        let err = compute(&schema, &conn, "O001").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("obs"),
            "error must name the store: {msg}"
        );
        assert!(
            msg.contains("no workflow declaration"),
            "error must mention workflow: {msg}"
        );
    }
}
