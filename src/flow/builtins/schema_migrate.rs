//! `builtin:schema-migrate` — apply additive schema migrations after the
//! refreshed `stores` binary is in place.
//!
//! Subscribes to the post-cargo-install transition (row=cargo_installed).
//! Runs `crate::handlers::migrate::apply_at` against the substrate
//! connection, using the row's `workspace_path` as the manifest root. On
//! success fires `mark_schema_migrated` (framework actor) — terminal in the
//! post-accept chain. On failure flips the row to `deploy_blocked` with the
//! migrate error captured in `blocked_reason`, fires `ntfy`, and dispatches
//! the row to the configured `deployment_specialist`.

use anyhow::Context;
use serde_json::Value;
use std::path::PathBuf;

use crate::flow::builtins::{
    dispatch_to_specialist, fire_framework_transition, fire_mark_deploy_blocked, BuiltinResult,
    DispatchCtx,
};
use crate::flow::NotifyEvent;
use crate::handlers::migrate;

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
    let workspace_path = row
        .get("workspace_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if workspace_path.is_empty() {
        eprintln!(
            "[schema-migrate] {}: workspace_path empty; cannot locate manifest",
            display_id
        );
        return Ok(1);
    }

    let root = PathBuf::from(workspace_path);

    match migrate::apply_at(ctx.conn, &root) {
        Ok(report) => {
            if report.is_no_op() {
                eprintln!("[schema-migrate] {}: no-op (in-sync)", display_id);
            } else {
                eprintln!(
                    "[schema-migrate] {}: applied {} column(s)",
                    display_id,
                    report.applied_columns.len()
                );
            }
            fire_framework_transition(
                ctx.conn,
                display_id,
                "mark_schema_migrated",
                std::collections::BTreeMap::new(),
                ctx.policies_hash,
            )
            .with_context(|| format!("firing mark_schema_migrated for {}", display_id))?;
            Ok(0)
        }
        Err(e) => {
            let err_text = format!("{:#}", e);
            let blocked_reason = format!("schema-migrate failed:\n{}", err_text.trim());

            fire_mark_deploy_blocked(ctx.conn, display_id, &blocked_reason, ctx.policies_hash)
                .with_context(|| format!("flipping {} to deploy_blocked", display_id))?;

            let event = NotifyEvent {
                row_id: display_id.to_string(),
                transition_attempted: "tasks: cargo_installed→deploy_blocked".to_string(),
                policy_id_or_actor_halt: "schema-migrate: migrate failure".to_string(),
                summary: blocked_reason.clone(),
            };
            let _ = crate::flow::notify_with_path(ctx.config_path, event);

            dispatch_to_specialist(row, ctx, display_id, "schema-migrate");

            Ok(0)
        }
    }
}
