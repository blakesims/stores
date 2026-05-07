use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExternalReviewVerdict {
    Pass,
    Revise,
    ToolingFailure,
}

impl ExternalReviewVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Revise => "REVISE",
            Self::ToolingFailure => "TOOLING_FAILURE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindingCounts {
    pub critical: usize,
    pub major: usize,
    pub minor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedReviewOutput {
    pub verdict: ExternalReviewVerdict,
    pub counts: FindingCounts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolingError {
    pub verdict: ExternalReviewVerdict,
    pub message: String,
}

impl ToolingError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            verdict: ExternalReviewVerdict::ToolingFailure,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PriorExternalReview {
    pub attempt: i64,
    pub verdict: Option<String>,
    pub critical_count: Option<i64>,
    pub major_count: Option<i64>,
    pub minor_count: Option<i64>,
    pub findings: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReviewInputBundle {
    pub task_id: String,
    pub done_when: String,
    pub contract: Value,
    pub plan: Value,
    pub plan_phase_names: Vec<String>,
    pub cycles: Value,
    pub prior_external_reviews: Vec<PriorExternalReview>,
    pub latest_wrap_log_executive_summary: String,
    pub wrap_log: Value,
    pub workspace_path: PathBuf,
    pub branch: Option<String>,
    pub base_sha: String,
    pub head_sha: String,
    pub diff: String,
}

#[derive(Debug)]
struct TaskRow {
    display_id: String,
    contract: String,
    plan: String,
    cycles: String,
    wrap_log: String,
    workspace_path: Option<String>,
    branch: Option<String>,
}

pub fn load_review_input_bundle(
    conn: &Connection,
    task_id: &str,
    workspace_override: Option<&Path>,
    base_override: Option<&str>,
    head_override: Option<&str>,
) -> std::result::Result<ReviewInputBundle, ToolingError> {
    let task = load_task_row(conn, task_id).map_err(|e| ToolingError::new(e.to_string()))?;
    let workspace_path = workspace_override
        .map(Path::to_path_buf)
        .or_else(|| task.workspace_path.as_deref().map(PathBuf::from))
        .ok_or_else(|| ToolingError::new("TOOLING_FAILURE: missing workspace_path"))?;

    let base_sha = resolve_sha(&workspace_path, base_override.unwrap_or("main"), "base")?;
    let head_sha = resolve_sha(&workspace_path, head_override.unwrap_or("HEAD"), "head")?;
    let diff = git_output(&workspace_path, &["diff", &base_sha, &head_sha])
        .map_err(|e| ToolingError::new(format!("TOOLING_FAILURE: cannot resolve diff: {e}")))?;

    let contract = parse_json_or_null(&task.contract);
    let plan = parse_json_or_null(&task.plan);
    let cycles = parse_json_or_null(&task.cycles);
    let wrap_log = parse_json_or_null(&task.wrap_log);
    let done_when = contract
        .get("done_when")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let plan_phase_names = plan
        .get("phases")
        .and_then(Value::as_array)
        .map(|phases| {
            phases
                .iter()
                .filter_map(|p| p.get("name").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let latest_wrap_log_executive_summary = wrap_log
        .as_array()
        .and_then(|rows| rows.last())
        .and_then(|row| row.get("executive_summary"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let prior_external_reviews =
        load_prior_external_reviews(conn, task_id).map_err(|e| ToolingError::new(e.to_string()))?;

    Ok(ReviewInputBundle {
        task_id: task.display_id,
        done_when,
        contract,
        plan,
        plan_phase_names,
        cycles,
        prior_external_reviews,
        latest_wrap_log_executive_summary,
        wrap_log,
        workspace_path,
        branch: task.branch,
        base_sha,
        head_sha,
        diff,
    })
}

fn load_task_row(conn: &Connection, task_id: &str) -> Result<TaskRow> {
    conn.query_row(
        "SELECT display_id, COALESCE(contract,''), COALESCE(plan,''), COALESCE(cycles,''), COALESCE(wrap_log,''), workspace_path, branch FROM tasks WHERE display_id=?1",
        [task_id],
        |row| {
            Ok(TaskRow {
                display_id: row.get(0)?,
                contract: row.get(1)?,
                plan: row.get(2)?,
                cycles: row.get(3)?,
                wrap_log: row.get(4)?,
                workspace_path: row.get(5)?,
                branch: row.get(6)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| anyhow::anyhow!("task {task_id} not found"))
}

fn load_prior_external_reviews(
    conn: &Connection,
    task_id: &str,
) -> Result<Vec<PriorExternalReview>> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='external_reviews'",
        [],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT attempt, verdict, critical_count, major_count, minor_count, findings FROM external_reviews WHERE task_id=?1 AND verdict IS NOT NULL ORDER BY attempt",
    )?;
    let rows = stmt.query_map([task_id], |row| {
        Ok(PriorExternalReview {
            attempt: row.get(0)?,
            verdict: row.get(1)?,
            critical_count: row.get(2)?,
            major_count: row.get(3)?,
            minor_count: row.get(4)?,
            findings: row.get(5)?,
        })
    })?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn parse_json_or_null(input: &str) -> Value {
    serde_json::from_str(input).unwrap_or(Value::Null)
}

fn resolve_sha(repo: &Path, rev: &str, label: &str) -> std::result::Result<String, ToolingError> {
    git_output(repo, &["rev-parse", "--verify", rev])
        .map(|s| s.trim().to_string())
        .map_err(|e| {
            ToolingError::new(format!("TOOLING_FAILURE: missing git {label} '{rev}': {e}"))
        })
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("run git {} in {}", args.join(" "), repo.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn render_codex_prompt(bundle: &ReviewInputBundle) -> String {
    let prior = if bundle.prior_external_reviews.is_empty() {
        "None".to_string()
    } else {
        bundle
            .prior_external_reviews
            .iter()
            .map(|r| {
                format!(
                    "- attempt {} verdict={} critical={} major={} minor={} findings={}",
                    r.attempt,
                    r.verdict.as_deref().unwrap_or(""),
                    r.critical_count.unwrap_or(0),
                    r.major_count.unwrap_or(0),
                    r.minor_count.unwrap_or(0),
                    r.findings.as_deref().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "# External Code Review\n\nReview task {task_id} using the contract, plan, wrap log, prior reviews, and rebase-aware diff below.\n\n## Verdict instructions\nReturn exactly one verdict: PASS, REVISE, or TOOLING_FAILURE. Use severity-tagged findings: [critical], [major], [minor]. PASS only if no blocking findings remain. REVISE if executor changes are required. TOOLING_FAILURE if review inputs or tools are unusable.\n\n## Contract done_when\n{done_when}\n\n## Contract JSON\n```json\n{contract}\n```\n\n## Plan phase names\n{phase_names}\n\n## Plan JSON\n```json\n{plan}\n```\n\n## Latest wrap_log executive_summary\n{summary}\n\n## wrap_log JSON\n```json\n{wrap_log}\n```\n\n## Prior external review attempts\n{prior}\n\n## Git context\nWorkspace: {workspace}\nBranch: {branch}\nBase SHA: {base}\nHead SHA: {head}\n\n## Diff\n```diff\n{diff}\n```\n",
        task_id = bundle.task_id,
        done_when = bundle.done_when,
        contract = serde_json::to_string_pretty(&bundle.contract).unwrap_or_default(),
        phase_names = bundle.plan_phase_names.join("\n"),
        plan = serde_json::to_string_pretty(&bundle.plan).unwrap_or_default(),
        summary = bundle.latest_wrap_log_executive_summary,
        wrap_log = serde_json::to_string_pretty(&bundle.wrap_log).unwrap_or_default(),
        prior = prior,
        workspace = bundle.workspace_path.display(),
        branch = bundle.branch.as_deref().unwrap_or(""),
        base = bundle.base_sha,
        head = bundle.head_sha,
        diff = bundle.diff,
    )
}

pub fn parse_codex_review_output(output: &str) -> Result<ParsedReviewOutput> {
    let verdict = parse_verdict(output)?;
    let counts = FindingCounts {
        critical: count_severity(output, "critical"),
        major: count_severity(output, "major"),
        minor: count_severity(output, "minor"),
    };
    Ok(ParsedReviewOutput { verdict, counts })
}

fn parse_verdict(output: &str) -> Result<ExternalReviewVerdict> {
    for line in output.lines() {
        let upper = line.trim().to_ascii_uppercase();
        if upper == "PASS"
            || upper.starts_with("VERDICT: PASS")
            || upper.starts_with("VERDICT=PASS")
        {
            return Ok(ExternalReviewVerdict::Pass);
        }
        if upper == "REVISE"
            || upper.starts_with("VERDICT: REVISE")
            || upper.starts_with("VERDICT=REVISE")
        {
            return Ok(ExternalReviewVerdict::Revise);
        }
        if upper == "TOOLING_FAILURE"
            || upper.starts_with("VERDICT: TOOLING_FAILURE")
            || upper.starts_with("VERDICT=TOOLING_FAILURE")
        {
            return Ok(ExternalReviewVerdict::ToolingFailure);
        }
    }
    anyhow::bail!("external review output missing PASS/REVISE/TOOLING_FAILURE verdict")
}

fn count_severity(output: &str, severity: &str) -> usize {
    let bracket = format!("[{severity}]");
    let colon = format!("{severity}:");
    output
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains(&bracket) || lower.trim_start().starts_with(&colon)
        })
        .count()
}

pub fn tooling_failure_ready_json(error: &ToolingError) -> Value {
    serde_json::json!({
        "verdict": error.verdict.as_str(),
        "error": error.message,
    })
}

pub fn mark_attempt_tooling_failure_ready(
    conn: &Connection,
    display_id: &str,
    error: &ToolingError,
) -> Result<()> {
    let payload = tooling_failure_ready_json(error).to_string();
    conn.execute(
        "UPDATE external_reviews SET verdict='TOOLING_FAILURE', findings=?2 WHERE display_id=?1",
        params![display_id, payload],
    )?;
    Ok(())
}
