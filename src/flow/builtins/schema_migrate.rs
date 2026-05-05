//! `builtin:schema-migrate` — apply additive schema migrations after the
//! refreshed `stores` binary is in place.
//!
//! Subscribes to the post-cargo-install transition (row=cargo_installed).
//! Delegates the migration to a `stores migrate --apply` subprocess (resolved
//! via the `STORES_BIN` env override or PATH lookup) with `cwd =
//! workspace_path` so the freshly cargo-installed binary's bundled schemas
//! drive the diff — not the daemon's stale in-process schemas. On success
//! fires `mark_schema_migrated` (framework actor) — terminal in the
//! post-accept chain. On failure flips the row to `deploy_blocked` with the
//! subprocess stderr captured in `blocked_reason`, fires `ntfy`, and
//! dispatches the row to the configured `deployment_specialist`.

use anyhow::Context;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

use crate::flow::builtins::{
    dispatch_to_specialist, fire_framework_transition, fire_mark_deploy_blocked, BuiltinResult,
    DispatchCtx,
};
use crate::flow::NotifyEvent;

/// Resolve the `stores` binary to invoke for the schema-migrate subprocess.
/// Honors the `STORES_BIN` env override (used by tests); falls back to the
/// bare name `stores` so PATH/`~/.cargo/bin` lookup picks the freshly
/// cargo-installed binary.
pub fn resolve_stores_bin() -> String {
    std::env::var("STORES_BIN").unwrap_or_else(|_| "stores".into())
}

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
    let bin = resolve_stores_bin();

    let output = Command::new(&bin)
        .args(["migrate", "--apply"])
        .current_dir(&root)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            let blocked_reason = format!(
                "schema-migrate failed:\nfailed to spawn `{} migrate --apply`: {}",
                bin, e
            );
            fire_mark_deploy_blocked(ctx.conn, display_id, &blocked_reason, ctx.policies_hash)
                .with_context(|| format!("flipping {} to deploy_blocked", display_id))?;
            let event = NotifyEvent {
                row_id: display_id.to_string(),
                transition_attempted: "tasks: cargo_installed→deploy_blocked".to_string(),
                policy_id_or_actor_halt: "schema-migrate: subprocess spawn failure".to_string(),
                summary: blocked_reason.clone(),
            };
            let _ = crate::flow::notify_with_path(ctx.config_path, event);
            dispatch_to_specialist(row, ctx, display_id, "schema-migrate");
            return Ok(0);
        }
    };

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let applied = stdout
            .lines()
            .filter(|l| l.trim_start().starts_with("ALTER TABLE"))
            .count();
        if applied == 0 {
            eprintln!("[schema-migrate] {}: no-op (in-sync)", display_id);
        } else {
            eprintln!(
                "[schema-migrate] {}: applied {} column(s)",
                display_id, applied
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
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(20).collect();
        let tail_text = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
        let blocked_reason = format!("schema-migrate failed:\n{}", tail_text.trim());

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
