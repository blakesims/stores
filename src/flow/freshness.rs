use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::flow::builtins::resolve_main_repo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshnessOutcome {
    Ready,
    StaleRequiresRefresh(Vec<String>),
    StaleRequiresRereview(Vec<String>),
    StaleRequiresRetest(Vec<String>),
}

pub fn check_freshness(row: &Value, current_main_sha: &str) -> Result<FreshnessOutcome> {
    let branch_head = field(row, "branch_head_sha");
    if branch_head.is_none() {
        return Ok(FreshnessOutcome::StaleRequiresRefresh(affected_scope(row)));
    }

    let scope = affected_scope(row);
    if scope.is_empty() {
        return Ok(FreshnessOutcome::StaleRequiresRefresh(scope));
    }

    let review_base = match field(row, "review_base_sha") {
        Some(v) => v,
        None => return Ok(FreshnessOutcome::StaleRequiresRereview(affected_scope(row))),
    };
    let test_base = match field(row, "test_base_sha") {
        Some(v) => v,
        None => return Ok(FreshnessOutcome::StaleRequiresRetest(affected_scope(row))),
    };
    let branch_head = field(row, "branch_head_sha").expect("checked above");
    let review_head = match field(row, "review_head_sha") {
        Some(v) => v,
        None => return Ok(FreshnessOutcome::StaleRequiresRereview(affected_scope(row))),
    };
    let test_head = match field(row, "test_head_sha") {
        Some(v) => v,
        None => return Ok(FreshnessOutcome::StaleRequiresRetest(affected_scope(row))),
    };
    if review_head != branch_head {
        return Ok(FreshnessOutcome::StaleRequiresRereview(affected_scope(row)));
    }
    if test_head != branch_head {
        return Ok(FreshnessOutcome::StaleRequiresRetest(affected_scope(row)));
    }

    if review_base == current_main_sha && test_base == current_main_sha {
        return Ok(FreshnessOutcome::Ready);
    }

    let changed = main_changed_paths(row, &review_base, current_main_sha)?;
    if intersects(&scope, &changed) {
        return Ok(FreshnessOutcome::StaleRequiresRereview(overlap(&scope, &changed)));
    }

    let test_changed = if test_base == review_base {
        changed
    } else {
        main_changed_paths(row, &test_base, current_main_sha)?
    };
    if intersects(&scope, &test_changed) {
        return Ok(FreshnessOutcome::StaleRequiresRetest(overlap(&scope, &test_changed)));
    }
    Ok(FreshnessOutcome::StaleRequiresRefresh(scope))
}

fn field(row: &Value, name: &str) -> Option<String> {
    row.get(name)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
}

pub fn affected_scope(row: &Value) -> Vec<String> {
    let Some(v) = row.get("affected_scope") else {
        return Vec::new();
    };
    if let Some(arr) = v.as_array() {
        return arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect();
    }
    if let Some(s) = v.as_str() {
        if s.trim().is_empty() {
            return Vec::new();
        }
        if let Ok(arr) = serde_json::from_str::<Vec<String>>(s) {
            return arr.into_iter().filter(|p| !p.trim().is_empty()).collect();
        }
    }
    Vec::new()
}

fn main_changed_paths(row: &Value, base: &str, head: &str) -> Result<Vec<String>> {
    let repo = repo_from_row(row).context("freshness check requires workspace_path resolved to main repo")?;
    git_changed_paths(&repo, base, head)
}

fn repo_from_row(row: &Value) -> Option<PathBuf> {
    let workspace = row.get("workspace_path").and_then(|v| v.as_str())?;
    resolve_main_repo(workspace)
}

pub fn git_changed_paths(repo: &Path, base: &str, head: &str) -> Result<Vec<String>> {
    let out = Command::new("git")
        .args(["-C", repo.to_str().unwrap_or("."), "diff", "--name-only", base, head])
        .output()
        .with_context(|| format!("spawning git diff --name-only {base} {head}"))?;
    if !out.status.success() {
        anyhow::bail!(
            "git diff --name-only {base} {head} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn intersects(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| b.iter().any(|y| paths_overlap(x, y)))
}

fn overlap(a: &[String], b: &[String]) -> Vec<String> {
    let mut out = BTreeSet::new();
    for x in a {
        for y in b {
            if paths_overlap(x, y) {
                out.insert(x.clone());
            }
        }
    }
    out.into_iter().collect()
}

fn paths_overlap(a: &str, b: &str) -> bool {
    a == b
        || a.strip_suffix('/').is_some_and(|p| b.starts_with(&format!("{p}/")))
        || b.strip_suffix('/').is_some_and(|p| a.starts_with(&format!("{p}/")))
}
