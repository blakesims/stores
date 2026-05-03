//! `builtin:cargo-install` — refresh the `stores` binary after a task is
//! merged into main.
//!
//! Subscribes to the post-accept-merge transition (row=accepted, branch
//! merged). Runs `cargo install --path <main_repo> --features <csv> --quiet`
//! from the main working tree. On success fires `mark_cargo_installed`
//! (framework actor) so the chain can advance to `builtin:schema-migrate`.
//! On failure flips the row to `deploy_blocked` with the tail of cargo's
//! stderr captured in `blocked_reason`, fires `ntfy`, and dispatches the
//! row to the configured `deployment_specialist`.

use anyhow::Context;
use serde_json::Value;
use std::process::Command;

use crate::flow::builtins::{
    dispatch_to_specialist, fire_framework_transition, fire_mark_deploy_blocked, resolve_main_repo,
    BuiltinResult, DispatchCtx,
};
use crate::flow::NotifyEvent;

const DEFAULT_FEATURES: &str = "runner-claude-code";

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let display_id = row.get("display_id").and_then(|v| v.as_str()).unwrap_or("");
    let workspace_path = row
        .get("workspace_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if workspace_path.is_empty() {
        eprintln!(
            "[cargo-install] {}: workspace_path empty; cannot locate project root",
            display_id
        );
        return Ok(1);
    }

    let main_repo = match resolve_main_repo(workspace_path) {
        Some(p) => p,
        None => {
            eprintln!(
                "[cargo-install] {}: could not resolve main repo from workspace_path '{}'",
                display_id, workspace_path
            );
            return Ok(1);
        }
    };

    let features = features_for_entry(ctx);

    let install = Command::new("cargo")
        .args([
            "install",
            "--path",
            main_repo.to_str().unwrap_or("."),
            "--features",
            &features,
            "--quiet",
        ])
        .output()
        .with_context(|| format!("spawning cargo install for {}", display_id))?;

    if install.status.success() {
        eprintln!(
            "[cargo-install] {}: ok ({} features={})",
            display_id,
            main_repo.display(),
            features
        );
        fire_framework_transition(
            ctx.conn,
            display_id,
            "mark_cargo_installed",
            std::collections::BTreeMap::new(),
            ctx.policies_hash,
        )
        .with_context(|| format!("firing mark_cargo_installed for {}", display_id))?;
        return Ok(0);
    }

    let stderr = String::from_utf8_lossy(&install.stderr).to_string();
    let tail: Vec<&str> = stderr.lines().collect();
    let start = tail.len().saturating_sub(20);
    let tail_joined = tail[start..].join("\n");
    let blocked_reason = format!(
        "cargo install --path {} --features {} failed:\n{}",
        main_repo.display(),
        features,
        tail_joined.trim()
    );

    fire_mark_deploy_blocked(ctx.conn, display_id, &blocked_reason, ctx.policies_hash)
        .with_context(|| format!("flipping {} to deploy_blocked", display_id))?;

    let event = NotifyEvent {
        row_id: display_id.to_string(),
        transition_attempted: "tasks: accepted→deploy_blocked".to_string(),
        policy_id_or_actor_halt: "cargo-install: build failure".to_string(),
        summary: blocked_reason.clone(),
    };
    let _ = crate::flow::notify_with_path(ctx.config_path, event);

    dispatch_to_specialist(row, ctx, display_id, "cargo-install");

    Ok(0)
}

/// Look up the `cargo-install` agent entry in agents.yaml and read its
/// `command_args.features` (string or sequence). Falls back to
/// `DEFAULT_FEATURES` when absent.
fn features_for_entry(ctx: &DispatchCtx) -> String {
    let entry = ctx.agents.agents.iter().find(|a| {
        a.command
            .strip_prefix("builtin:")
            .map(|kw| kw == "cargo-install")
            .unwrap_or(false)
    });
    let Some(entry) = entry else {
        return DEFAULT_FEATURES.to_string();
    };
    let Some(args) = entry.command_args.as_ref() else {
        return DEFAULT_FEATURES.to_string();
    };
    let key = serde_yaml::Value::String("features".into());
    match args.get(&key) {
        Some(serde_yaml::Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(serde_yaml::Value::Sequence(seq)) if !seq.is_empty() => seq
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(","),
        _ => DEFAULT_FEATURES.to_string(),
    }
}
