//! `builtin:gatekeeper-stub` — deterministic test-only gatekeeper subscriber.
//!
//! Production wiring is a shell-out gatekeeper agent. This builtin exists so
//! agents.yaml subscriber tests can prove the router seam without spawning an
//! LLM CLI.

use anyhow::anyhow;
use serde_json::Value;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};
use crate::handlers::row::read_row;
use crate::handlers::transition::execute_transition_write;
use crate::schema::actor::Actor;
use crate::schema::lifecycle::select_transition;
use crate::validate::{self, EntryMap, Op};

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row
        .get("display_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("gatekeeper-stub row missing display_id"))?;

    let schema = crate::flow::builtins::load_store_schema("intake")?;
    let tx = ctx.conn.unchecked_transaction()?;
    let (row_id, existing) = read_row(&schema, &tx, display_id)?;
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let decision_json = serde_json::json!({
        "decision": "needs_info",
        "confidence": "low",
        "rationale": "builtin gatekeeper stub default: request recon evidence before classification",
        "missing_info_question": "What concrete file, command, or transition demonstrates this intake item?"
    });

    let mut diff = EntryMap::new();
    diff.insert("decision".to_string(), Value::String("needs_info".to_string()));
    diff.insert("gatekeeper_decision_json".to_string(), decision_json);

    let mut merged = existing.clone();
    for (k, v) in &diff {
        merged.insert(k.clone(), v.clone());
    }

    let transition = select_transition(
        &schema.lifecycle.transitions,
        current_status,
        "route",
        None,
        &merged,
    )?;

    crate::handlers::transition::maybe_validate_and_mirror_gatekeeper_decision(
        &schema,
        &mut diff,
        &mut merged,
    )?;
    crate::handlers::intake_route::inject_pre_validation_fields(
        &tx,
        &mut diff,
        &mut merged,
        "route",
    )?;

    validate::validate(
        &schema,
        &merged,
        Op::Transition("route".to_string(), diff.clone()),
        Actor::AiAutonomous.into(),
    )
    .map_err(|errs| anyhow!("gatekeeper-stub route validation failed:\n{}", validate::pretty_print(&errs)))?;

    execute_transition_write(
        &tx,
        &schema,
        row_id,
        display_id,
        current_status,
        &transition.to,
        "route",
        &diff,
        &merged,
        Actor::AiAutonomous,
        None,
        if ctx.policies_hash.is_empty() {
            None
        } else {
            Some(ctx.policies_hash)
        },
        Some("builtin:gatekeeper-stub"),
    )?;

    tx.commit()?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::flow::AgentsYaml;
    use crate::schema::Schema;
    use rusqlite::Connection;
    use std::path::Path;

    fn fresh_db() -> (Connection, Schema) {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        let intake_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "intake")
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(intake_yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        (conn, schema)
    }

    #[test]
    fn dispatch_builtin_routes_triaging_row_to_needs_info() {
        let (conn, _schema) = fresh_db();
        conn.execute(
            "INSERT INTO intake (display_id, status, summary, source_agent, captured_at, captured_week, created_at, updated_at, created_by, updated_by) \
             VALUES ('I001', 'triaging', 'stub route seam', 'executor', '2026-05-06T00:00:00Z', 'w18-d3', '2026-05-06T00:00:00Z', '2026-05-06T00:00:00Z', 'ai', 'ai')",
            [],
        )
        .unwrap();
        let row = serde_json::json!({"display_id":"I001"});
        let agents = AgentsYaml::default_empty();
        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: Path::new("/tmp/no-config.yaml"),
            policies_hash: "",
        };
        let res = crate::flow::builtins::dispatch_builtin("gatekeeper-stub", &row, &ctx)
            .expect("builtin registered")
            .unwrap();
        assert_eq!(res, 0);
        let status: String = conn
            .query_row("SELECT status FROM intake WHERE display_id='I001'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "needs_info");
        let question: String = conn
            .query_row("SELECT missing_info_question FROM intake WHERE display_id='I001'", [], |r| r.get(0))
            .unwrap();
        assert!(question.contains("concrete file"));
    }
}
