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
//!
//! Stale-workspace fallback (L145): when `workspace_path` is missing or
//! unusable (worktree cleaned after merge), fall back to the daemon's cwd
//! for the cargo install path. The cwd MUST be a git repo or the fallback
//! fails loudly with guidance rather than silently installing from the wrong
//! place. Mirrors accept-merge's `resolve_main_repo_for_check` pattern.

use anyhow::{bail, Context};
use serde_json::Value;
use std::path::PathBuf;
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

    let main_repo = if workspace_path.is_empty() {
        eprintln!(
            "[cargo-install] {}: workspace_path empty; attempting daemon cwd fallback",
            display_id
        );
        resolve_main_repo_via_cwd(display_id)?
    } else {
        match resolve_main_repo(workspace_path) {
            Some(p) => p,
            None => {
                eprintln!(
                    "[cargo-install] {}: workspace_path '{}' stale; installing from daemon cwd",
                    display_id, workspace_path
                );
                resolve_main_repo_via_cwd(display_id)?
            }
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

/// Resolve a main-repo path via the daemon's current working directory.
///
/// Validates that cwd is a git repo (via `git rev-parse --git-common-dir`) AND
/// that the Cargo.toml at cwd belongs to the `stores` crate. Both checks must
/// pass; failing either causes a loud bail with actionable guidance rather than
/// silently installing from the wrong place.
fn resolve_main_repo_via_cwd(display_id: &str) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().with_context(|| {
        format!(
            "[cargo-install] {}: could not read daemon cwd for stale-workspace fallback",
            display_id
        )
    })?;

    // Gate 1: cwd must be a git repository.
    let out = Command::new("git")
        .args([
            "-C",
            cwd.to_str().unwrap_or("."),
            "rev-parse",
            "--git-common-dir",
        ])
        .output()
        .with_context(|| {
            format!(
                "[cargo-install] {}: git rev-parse failed in daemon cwd '{}'",
                display_id,
                cwd.display()
            )
        })?;
    if !out.status.success() {
        bail!(
            "[cargo-install] {}: daemon cwd '{}' is not a git repository; \
             cannot use as stale-workspace fallback. \
             Fix: ensure the daemon is started from inside the stores repository.",
            display_id,
            cwd.display()
        );
    }

    // Gate 2: cwd must be the stores crate, not just any git/Cargo repo.
    // Read Cargo.toml and extract the [package] name field with a simple
    // line-scan (no extra toml dep needed — the guard is correct for any
    // realistic Cargo.toml layout).
    let cargo_toml_path = cwd.join("Cargo.toml");
    let cargo_toml = std::fs::read_to_string(&cargo_toml_path).map_err(|_| {
        anyhow::anyhow!(
            "[cargo-install] {}: daemon cwd '{}' does not contain a Cargo.toml; \
             cannot install stores from here. \
             Fix: restore workspace_path or start the daemon from inside the stores repository.",
            display_id,
            cwd.display()
        )
    })?;
    let package_name = extract_cargo_package_name(&cargo_toml);
    match package_name.as_deref() {
        Some("stores") => {}
        Some(other) => bail!(
            "[cargo-install] {}: daemon cwd '{}' is a Cargo project ('{}') but not the stores crate; \
             cannot install from here. \
             Fix: restore workspace_path or invoke retry-deploy from the stores repository cwd.",
            display_id,
            cwd.display(),
            other
        ),
        None => bail!(
            "[cargo-install] {}: daemon cwd '{}' has a Cargo.toml but no parseable [package] name; \
             cannot verify this is the stores crate. \
             Fix: restore workspace_path or start the daemon from inside the stores repository.",
            display_id,
            cwd.display()
        ),
    }

    eprintln!(
        "[cargo-install] {}: workspace_path stale; installing stores crate from daemon cwd {}",
        display_id,
        cwd.display()
    );
    Ok(cwd)
}

/// Extract the `name` field from the `[package]` section of a Cargo.toml
/// string using a simple line-scan. Returns `None` if the section or field is
/// absent or unparseable. Does not depend on a toml-parsing crate.
fn extract_cargo_package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        // Section header detection.
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        // Match `name = "..."` (with optional whitespace around `=`).
        if let Some(rest) = trimmed.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                if rest.starts_with('"') && rest.ends_with('"') && rest.len() >= 2 {
                    return Some(rest[1..rest.len() - 1].to_string());
                }
            }
        }
    }
    None
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
