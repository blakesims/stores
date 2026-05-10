//! `builtin:cargo-install` — refresh the stores-private daemon binary after
//! the generic integration lane lands a candidate.
//!
//! T138 P3: this is now a stores-repo-specific subscriber on
//! (integrating → integrated). The source state expectation is `integrated`
//! (post-T138; previously `accepted`). On success it fires
//! `mark_cargo_installed` (integrated → cargo_installed); the
//! schema-migrate subscriber takes over from there.
//!
//! Builds into an isolated cargo install root, validates the candidate `stores`
//! binary, then atomically promotes it to the private daemon binary path.

use anyhow::{bail, Context};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::runner::liveness::{self, LivenessThresholds};

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
    let install_root = tempfile::Builder::new()
        .prefix("stores-cargo-install-root-")
        .tempdir()
        .context("creating isolated cargo install root")?;

    let mut cmd = Command::new("cargo");
    cmd.args([
        "install",
        "--path",
        main_repo.to_str().unwrap_or("."),
        "--features",
        &features,
        "--root",
        install_root.path().to_str().unwrap_or("."),
        "--quiet",
    ]);
    let install = liveness::run_streaming_with_liveness(
        &mut cmd,
        &LivenessThresholds::from_env(),
        |_| {},
        |_| {},
    )
    .with_context(|| format!("spawning cargo install for {}", display_id))?;

    if install.killed_for.is_some() || install.exit_code != 0 {
        let stderr = if let Some(killed) = &install.killed_for {
            if install.stderr.trim().is_empty() {
                killed.label()
            } else {
                format!("{}\n{}", killed.label(), install.stderr)
            }
        } else {
            install.stderr.clone()
        };
        let blocked_reason = format_cargo_blocked_reason(&main_repo, &features, &stderr);
        block_deploy(row, ctx, display_id, blocked_reason)?;
        return Ok(0);
    }

    let candidate = install_root.path().join("bin").join("stores");
    if let Err(e) = crate::handlers::agents_run::validate_stores_binary_candidate(&candidate) {
        let blocked_reason = format!(
            "cargo install --path {} --features {} produced invalid stores candidate at {}:\n{:#}",
            main_repo.display(),
            features,
            candidate.display(),
            e
        );
        block_deploy(row, ctx, display_id, blocked_reason)?;
        return Ok(0);
    }

    let private_path = crate::paths::ensure_daemon_binary_parent()?;
    let promote_path = private_path.with_extension(format!("candidate.{}", std::process::id()));
    std::fs::copy(&candidate, &promote_path).with_context(|| {
        format!(
            "copying validated candidate {} to promotion path {}",
            candidate.display(),
            promote_path.display()
        )
    })?;
    if let Ok(md) = std::fs::metadata(&candidate) {
        let _ = std::fs::set_permissions(&promote_path, md.permissions());
    }
    std::fs::rename(&promote_path, &private_path).with_context(|| {
        format!(
            "promoting validated stores candidate to private daemon path {}",
            private_path.display()
        )
    })?;

    eprintln!(
        "[cargo-install] {}: ok ({} features={} private_path={})",
        display_id,
        main_repo.display(),
        features,
        private_path.display()
    );
    fire_framework_transition(
        ctx.conn,
        display_id,
        "mark_cargo_installed",
        std::collections::BTreeMap::new(),
        ctx.policies_hash,
    )
    .with_context(|| format!("firing mark_cargo_installed for {}", display_id))?;
    Ok(0)
}

fn format_cargo_blocked_reason(main_repo: &Path, features: &str, stderr: &str) -> String {
    let tail: Vec<&str> = stderr.lines().collect();
    let start = tail.len().saturating_sub(20);
    let tail_joined = tail[start..].join("\n");
    format!(
        "cargo install --path {} --features {} failed:\n{}",
        main_repo.display(),
        features,
        tail_joined.trim()
    )
}

fn block_deploy(
    row: &Value,
    ctx: &DispatchCtx,
    display_id: &str,
    blocked_reason: String,
) -> anyhow::Result<()> {
    fire_mark_deploy_blocked(ctx.conn, display_id, &blocked_reason, ctx.policies_hash)
        .with_context(|| format!("flipping {} to deploy_blocked", display_id))?;

    let event = NotifyEvent {
        row_id: display_id.to_string(),
        transition_attempted: "tasks: accepted→deploy_blocked".to_string(),
        policy_id_or_actor_halt: "cargo-install: build/candidate failure".to_string(),
        summary: blocked_reason.clone(),
    };
    let _ = crate::flow::notify_with_path(ctx.config_path, event);

    dispatch_to_specialist(row, ctx, display_id, "cargo-install");
    Ok(())
}

/// Resolve a main-repo path via the daemon's current working directory.
fn resolve_main_repo_via_cwd(display_id: &str) -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().with_context(|| {
        format!(
            "[cargo-install] {}: could not read daemon cwd for stale-workspace fallback",
            display_id
        )
    })?;

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

fn extract_cargo_package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("name") {
            if !rest.starts_with(|c: char| c.is_whitespace() || c == '=') {
                continue;
            }
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim_start();
                if let Some(after_open) = rest.strip_prefix('"') {
                    if let Some(end) = after_open.find('"') {
                        return Some(after_open[..end].to_string());
                    }
                }
            }
        }
    }
    None
}

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
