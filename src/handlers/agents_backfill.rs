//! `stores agents backfill` — Phase 7 of T014.
//!
//! One-off scan: find every `tasks` row with `status = 'accepted'` whose
//! `branch` is set but not yet merged into the project main branch, and
//! reuse the accept-merge builtin to fast-merge each one sequentially.
//!
//! Idempotent: a branch already in `git branch --merged main` is skipped.
//! Conflicts surface via the same `deploy_blocked + ntfy` path as the live
//! daemon (`accept_merge::run` does the work).
//!
//! Backfill is a side-effect catchup — it does NOT write a new
//! `transition_history` row for the accepted-state itself (the row was
//! already accepted).

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

use crate::flow::builtins::{accept_merge, DispatchCtx};
use crate::flow::AgentsYaml;

pub fn run_backfill() -> Result<()> {
    let stores_dir = crate::paths::stores_dir()?;
    let agents_path = stores_dir.join("agents.yaml");
    let agents = if agents_path.exists() {
        crate::flow::agents_yaml::load_from_path(&agents_path)
            .context("loading .stores/agents.yaml")?
    } else {
        AgentsYaml::default_empty()
    };
    let config_path = stores_dir.join("config.yaml");

    let db_path = crate::paths::db_path()?;
    let conn = crate::db::open(&db_path)?;

    let rows = scan_accepted_unmerged(&conn)?;
    println!("{} rows scanned", rows.len());

    for row in &rows {
        let display_id = row
            .get("display_id")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let branch = row.get("branch").and_then(|v| v.as_str()).unwrap_or("");
        let workspace_path = row
            .get("workspace_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if branch.is_empty() || workspace_path.is_empty() {
            println!("  {} skipped: branch or workspace_path empty", display_id);
            continue;
        }
        if branch_already_merged(workspace_path, branch) {
            println!("  {} skipped: branch '{}' already merged", display_id, branch);
            continue;
        }

        let ctx = DispatchCtx {
            conn: &conn,
            agents: &agents,
            config_path: &config_path,
            policies_hash: "",
        };
        match accept_merge::run(row, &ctx) {
            Ok(0) => println!("  {} merged: branch '{}'", display_id, branch),
            Ok(c) => println!("  {} non-zero exit ({}): branch '{}'", display_id, c, branch),
            Err(e) => println!("  {} error: {}", display_id, e),
        }
    }

    Ok(())
}

/// Return all `tasks` rows where `status='accepted'`. Each row is a flat JSON
/// object (column → value). We don't filter by "merged" here — that requires
/// a `git` call per row, which `run_backfill` does in the loop.
fn scan_accepted_unmerged(conn: &Connection) -> Result<Vec<Value>> {
    // Some test DBs may not have all columns; SELECT * is robust.
    let mut stmt = conn.prepare("SELECT * FROM tasks WHERE status = 'accepted'")?;
    let cols: Vec<String> = stmt
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut obj = serde_json::Map::new();
        for (i, name) in cols.iter().enumerate() {
            let v: rusqlite::types::Value = row.get(i)?;
            let jv = match v {
                rusqlite::types::Value::Null => Value::Null,
                rusqlite::types::Value::Integer(i) => Value::from(i),
                rusqlite::types::Value::Real(f) => {
                    Value::from(serde_json::Number::from_f64(f).unwrap_or(0.into()))
                }
                rusqlite::types::Value::Text(s) => Value::String(s),
                rusqlite::types::Value::Blob(b) => {
                    Value::String(String::from_utf8_lossy(&b).to_string())
                }
            };
            obj.insert(name.clone(), jv);
        }
        out.push(Value::Object(obj));
    }
    Ok(out)
}

/// Heuristic: `git -C <main_repo> branch --merged main` lists branches whose
/// tip is reachable from main. We resolve `main_repo` from `workspace_path`
/// the same way `accept-merge` does. On any git failure, return false (let
/// accept-merge attempt the merge and report properly).
fn branch_already_merged(workspace_path: &str, branch: &str) -> bool {
    let main_repo = match resolve_main_repo(workspace_path) {
        Some(p) => p,
        None => return false,
    };
    let out = Command::new("git")
        .args([
            "-C",
            main_repo.to_str().unwrap_or("."),
            "branch",
            "--merged",
            "main",
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.lines()
                .map(|l| l.trim_start_matches('*').trim())
                .any(|name| name == branch)
        }
        _ => false,
    }
}

fn resolve_main_repo(workspace_path: &str) -> Option<PathBuf> {
    // Same logic as flow::builtins::resolve_main_repo (private). Re-implement
    // to avoid widening visibility for one caller.
    let out = Command::new("git")
        .args(["-C", workspace_path, "rev-parse", "--git-common-dir"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let common = if PathBuf::from(&raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        PathBuf::from(workspace_path).join(raw)
    };
    let canon = common.canonicalize().ok()?;
    canon.parent().map(|p| p.to_path_buf())
}
