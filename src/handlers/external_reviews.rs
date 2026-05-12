use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use crate::flow::builtins::accept_merge;
use crate::handlers::row::now_iso8601;

use crate::flow::config::{CodexCfg, ReviewCfg};
use crate::runner::{Runner, RunnerOutput};

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

const EXTERNAL_REVIEW_OUTPUT_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["verdict"],
  "properties": {
    "verdict": {"enum": ["PASS", "REVISE", "TOOLING_FAILURE"]},
    "critical_count": {"type": "integer", "minimum": 0},
    "major_count": {"type": "integer", "minimum": 0},
    "minor_count": {"type": "integer", "minimum": 0},
    "counts": {
      "type": "object",
      "properties": {
        "critical": {"type": "integer", "minimum": 0},
        "major": {"type": "integer", "minimum": 0},
        "minor": {"type": "integer", "minimum": 0}
      }
    },
    "findings": {}
  }
}"#;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalReviewGitPreflight {
    pub workspace_path: PathBuf,
    pub base_sha: String,
    pub head_sha: String,
    pub rebase_performed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalReviewRebaseConflict {
    pub base_sha: String,
    pub head_sha: String,
    pub conflict_files: Vec<String>,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalReviewGitPreparation {
    Ready(ExternalReviewGitPreflight),
    Conflict(ExternalReviewRebaseConflict),
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

pub fn prepare_external_review_git(
    conn: &Connection,
    task_id: &str,
) -> std::result::Result<ExternalReviewGitPreparation, ToolingError> {
    let task = load_task_row(conn, task_id).map_err(|e| ToolingError::new(e.to_string()))?;
    let workspace_path = task
        .workspace_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| ToolingError::new("TOOLING_FAILURE: missing workspace_path"))?;

    if let Some(branch) = task.branch.as_deref().filter(|s| !s.is_empty()) {
        git_output(&workspace_path, &["checkout", branch]).map_err(|e| {
            ToolingError::new(format!(
                "TOOLING_FAILURE: cannot checkout task branch '{branch}': {e}"
            ))
        })?;
    }

    let current_main = resolve_sha(&workspace_path, "main", "base")?;
    let subject_rev = task
        .branch
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("HEAD");
    let branch_base = git_output(&workspace_path, &["merge-base", subject_rev, "main"])
        .map_err(|e| ToolingError::new(format!("TOOLING_FAILURE: cannot resolve merge-base: {e}")))?
        .trim()
        .to_string();
    if branch_base == current_main {
        let head_sha = resolve_sha(&workspace_path, "HEAD", "head")?;
        return Ok(ExternalReviewGitPreparation::Ready(
            ExternalReviewGitPreflight {
                workspace_path,
                base_sha: current_main,
                head_sha,
                rebase_performed: false,
            },
        ));
    }

    // Capture the task branch tip BEFORE rebasing so that if the rebase
    // conflicts and is aborted, head_sha on the held row reflects the
    // branch state, not the transient rebase-in-progress HEAD.
    let task_branch_head_sha =
        resolve_sha(&workspace_path, "HEAD", "head").unwrap_or_else(|_| String::new());

    let rebase = Command::new("git")
        .args(["rebase", "main"])
        .current_dir(&workspace_path)
        .output()
        .map_err(|e| ToolingError::new(format!("TOOLING_FAILURE: cannot spawn git rebase: {e}")))?;
    if !rebase.status.success() {
        let conflict_files = accept_merge::list_conflict_files(&workspace_path);
        let stderr = String::from_utf8_lossy(&rebase.stderr).trim().to_string();
        accept_merge::abort_rebase(&workspace_path);
        // Use the pre-rebase head; after abort HEAD is restored to the branch
        // tip, but resolving again introduces a TOCTOU window — use the
        // value we captured before git touched it.
        return Ok(ExternalReviewGitPreparation::Conflict(
            ExternalReviewRebaseConflict {
                base_sha: current_main,
                head_sha: task_branch_head_sha,
                conflict_files,
                stderr,
            },
        ));
    }

    let head_sha = resolve_sha(&workspace_path, "HEAD", "head")?;
    Ok(ExternalReviewGitPreparation::Ready(
        ExternalReviewGitPreflight {
            workspace_path,
            base_sha: current_main,
            head_sha,
            rebase_performed: true,
        },
    ))
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

pub(crate) fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPassArgs<'a> {
    pub task_id: &'a str,
    pub transcript_path: &'a Path,
    pub base_sha: &'a str,
    pub head_sha: &'a str,
    pub runner: &'a str,
    pub actor: &'a str,
}

pub fn import_manual_pass(conn: &Connection, args: ImportPassArgs<'_>) -> Result<String> {
    if args.runner != "manual-codex" && args.runner != "manual" {
        anyhow::bail!("import-pass runner must be manual-codex or manual");
    }
    if !args.transcript_path.exists() {
        anyhow::bail!(
            "import-pass transcript does not exist: {}",
            args.transcript_path.display()
        );
    }

    let task = load_task_row(conn, args.task_id)?;
    let workspace = task
        .workspace_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "import-pass requires task {task_id} workspace_path",
                task_id = args.task_id
            )
        })?;
    let current_head =
        resolve_sha(&workspace, "HEAD", "head").map_err(|e| anyhow::anyhow!(e.message))?;
    if args.head_sha != current_head {
        anyhow::bail!(
            "import-pass head mismatch for {}: supplied {}, current head is {}",
            args.task_id,
            args.head_sha,
            current_head
        );
    }
    let current_base =
        resolve_sha(&workspace, "main", "base").map_err(|e| anyhow::anyhow!(e.message))?;
    if args.base_sha != current_base {
        anyhow::bail!(
            "import-pass base mismatch for {}: supplied {}, current main is {}",
            args.task_id,
            args.base_sha,
            current_base
        );
    }

    let tx = conn.unchecked_transaction()?;
    let next_id_num: i64 = tx.query_row(
        "SELECT COALESCE(MAX(id), 0) + 1 FROM external_reviews",
        [],
        |r| r.get(0),
    )?;
    let display_id = format!("ER{next_id_num:03}");
    let attempt: i64 = tx.query_row(
        "SELECT COALESCE(MAX(attempt), 0) + 1 FROM external_reviews WHERE task_id=?1",
        params![args.task_id],
        |r| r.get(0),
    )?;
    let now = now_iso8601();
    let transcript = args.transcript_path.display().to_string();
    // Older live DBs still constrain external_reviews.runner to the executable
    // runner enum (codex/pi/claude-code). Keep import-pass compatible there;
    // transition history below preserves the operator-supplied manual label.
    let stored_runner = match args.runner {
        "manual-codex" | "manual" => "codex",
        other => other,
    };
    tx.execute(
        "INSERT INTO external_reviews (display_id,status,created_at,updated_at,created_by,updated_by,task_id,attempt,adapter,runner,base_sha,head_sha,verdict,critical_count,major_count,minor_count,started_at,completed_at,duration_ms,transcript_path,contract_ref,plan_ref,wrap_log_ref,diff_ref,prior_review_ref) VALUES (?1,'passed',?2,?2,?3,?3,?4,?5,'external_review',?6,?7,?8,'PASS',0,0,0,?2,?2,0,?9,?10,?11,?12,?13,?14)",
        params![display_id, now, args.actor, args.task_id, attempt, stored_runner, args.base_sha, args.head_sha, transcript, format!("tasks:{}:contract", args.task_id), format!("tasks:{}:plan", args.task_id), format!("tasks:{}:wrap_log", args.task_id), format!("git-diff:{}..{}", args.base_sha, args.head_sha), format!("tasks:{}:cycles", args.task_id)],
    )?;
    let row_id = tx.last_insert_rowid();
    crate::db::insert_transition_history(
        &tx,
        "external_reviews",
        row_id,
        &display_id,
        "",
        "passed",
        "import-pass",
        args.actor,
        None,
        None,
        Some(&format!(
            "manual PASS import runner={} transcript_path={} base_sha={} head_sha={}",
            args.runner,
            args.transcript_path.display(),
            args.base_sha,
            args.head_sha
        )),
    )?;
    tx.commit()?;
    Ok(display_id)
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

fn parse_runner_review_output(out: &RunnerOutput) -> Result<ParsedReviewOutput> {
    if let Some(value) = out.structured_output.as_ref() {
        parse_structured_review_output(value)
    } else {
        parse_codex_review_output(review_output_text(out))
    }
}

fn parse_structured_review_output(value: &Value) -> Result<ParsedReviewOutput> {
    let verdict = match value.get("verdict").and_then(Value::as_str) {
        Some("PASS") => ExternalReviewVerdict::Pass,
        Some("REVISE") => ExternalReviewVerdict::Revise,
        Some("TOOLING_FAILURE") => ExternalReviewVerdict::ToolingFailure,
        Some(other) => anyhow::bail!("unknown structured external review verdict '{other}'"),
        None => anyhow::bail!("structured external review output missing verdict"),
    };
    let counts_obj = value.get("counts");
    let count = |flat: &str, nested: &str| -> usize {
        value
            .get(flat)
            .and_then(Value::as_u64)
            .or_else(|| {
                counts_obj
                    .and_then(|c| c.get(nested))
                    .and_then(Value::as_u64)
            })
            .unwrap_or(0) as usize
    };
    Ok(ParsedReviewOutput {
        verdict,
        counts: FindingCounts {
            critical: count("critical_count", "critical"),
            major: count("major_count", "major"),
            minor: count("minor_count", "minor"),
        },
    })
}

fn parse_verdict(output: &str) -> Result<ExternalReviewVerdict> {
    for line in output.lines() {
        let upper = line.trim().to_ascii_uppercase();
        if upper == "PASS"
            || upper.starts_with("VERDICT: PASS")
            || upper.starts_with("VERDICT=PASS")
            || upper.starts_with("RESULT: PASS")
            || upper.starts_with("RESULT=PASS")
            || starts_with_leading_verdict(&upper, "PASS")
        {
            return Ok(ExternalReviewVerdict::Pass);
        }
        if upper == "REVISE"
            || upper.starts_with("VERDICT: REVISE")
            || upper.starts_with("VERDICT=REVISE")
            || upper.starts_with("RESULT: REVISE")
            || upper.starts_with("RESULT=REVISE")
            || starts_with_leading_verdict(&upper, "REVISE")
        {
            return Ok(ExternalReviewVerdict::Revise);
        }
        if upper == "TOOLING_FAILURE"
            || upper.starts_with("VERDICT: TOOLING_FAILURE")
            || upper.starts_with("VERDICT=TOOLING_FAILURE")
            || upper.starts_with("RESULT: TOOLING_FAILURE")
            || upper.starts_with("RESULT=TOOLING_FAILURE")
            || starts_with_leading_verdict(&upper, "TOOLING_FAILURE")
        {
            return Ok(ExternalReviewVerdict::ToolingFailure);
        }
    }

    if has_revise_finding_marker(output) {
        return Ok(ExternalReviewVerdict::Revise);
    }

    if output.trim().is_empty() {
        anyhow::bail!("external review output missing PASS/REVISE/TOOLING_FAILURE verdict")
    }
    anyhow::bail!("parse-fallback-needed")
}

fn starts_with_leading_verdict(line: &str, verdict: &str) -> bool {
    line.strip_prefix(verdict)
        .and_then(|rest| rest.chars().next())
        .map(|c| c.is_ascii_whitespace() || c.is_ascii_punctuation())
        .unwrap_or(false)
}

fn has_revise_finding_marker(output: &str) -> bool {
    output.lines().any(|line| {
        let lower_owned = line.trim_start().to_ascii_lowercase();
        let lower = strip_list_marker_prefix(&lower_owned);
        ["[critical]", "[major]", "[minor]", "[p1]", "[p2]", "[p3]"]
            .iter()
            .any(|marker| lower.starts_with(marker))
    })
}

fn strip_list_marker_prefix(line: &str) -> &str {
    let trimmed = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
        .unwrap_or(line);
    trimmed.trim_start()
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

pub fn effective_review_config(config_path: &Path, review: &ReviewCfg) -> ReviewCfg {
    if crate::flow::config::fake_external_review_enabled(config_path) {
        let mut fake = review.clone();
        fake.runner = "fake".to_string();
        fake.model = Some("fake-random-v1".to_string());
        fake
    } else {
        review.clone()
    }
}

pub fn build_review_runner(review: &ReviewCfg, codex: &CodexCfg) -> Result<Box<dyn Runner>> {
    match review.runner.as_str() {
        "codex" => Ok(Box::new(crate::runner::CodexRunner::with_config(
            codex.command.clone(),
            codex.args.clone(),
            review.model.clone(),
            review.timeout_secs,
        ))),
        "pi" | "claude-code" | "fake" => crate::runner::select(&review.runner),
        other => {
            anyhow::bail!(
                "unknown review.runner '{other}'; expected codex, pi, claude-code, or fake"
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn run_external_review_attempt(
    conn: &Connection,
    review_display_id: &str,
    task_id: &str,
    review: &ReviewCfg,
    codex: &CodexCfg,
    workspace_override: Option<&Path>,
    base_override: Option<&str>,
    head_override: Option<&str>,
) -> std::result::Result<ParsedReviewOutput, ToolingError> {
    let config_path = crate::flow::config::default_config_path()
        .unwrap_or_else(|_| std::path::PathBuf::from(".stores/config.yaml"));
    run_external_review_attempt_with_config(
        conn,
        review_display_id,
        task_id,
        review,
        codex,
        &config_path,
        workspace_override,
        base_override,
        head_override,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_external_review_attempt_with_config(
    conn: &Connection,
    review_display_id: &str,
    task_id: &str,
    review: &ReviewCfg,
    codex: &CodexCfg,
    config_path: &Path,
    workspace_override: Option<&Path>,
    base_override: Option<&str>,
    head_override: Option<&str>,
) -> std::result::Result<ParsedReviewOutput, ToolingError> {
    let bundle = load_review_input_bundle(
        conn,
        task_id,
        workspace_override,
        base_override,
        head_override,
    )?;
    let prompt = render_codex_prompt(&bundle);
    let effective_review = effective_review_config(config_path, review);
    let runner = build_review_runner(&effective_review, codex)
        .map_err(|e| ToolingError::new(e.to_string()))?;
    let started = Instant::now();
    let mut runner_env = crate::flow::config::fake_runner_env(config_path);
    if effective_review.runner == "fake" {
        runner_env.push((
            "STORES_FAKE_CONFIGURED_HARNESS_ID".to_string(),
            review.runner.clone(),
        ));
        if let Some(model) = &review.model {
            runner_env.push(("STORES_FAKE_CONFIGURED_MODEL_ID".to_string(), model.clone()));
        }
        runner_env.push(("STORES_FAKE_TASK_ID".to_string(), task_id.to_string()));
        runner_env.push((
            "STORES_FAKE_PHASE".to_string(),
            "external-review".to_string(),
        ));
        runner_env.push((
            "STORES_FAKE_ATTEMPT".to_string(),
            review_display_id.to_string(),
        ));
    }
    let out = runner
        .spawn_with_invocation_and_env(
            "external-review",
            "",
            &prompt,
            Some(EXTERNAL_REVIEW_OUTPUT_SCHEMA),
            Some(&bundle.workspace_path.to_string_lossy()),
            None,
            &runner_env,
        )
        .map_err(|e| ToolingError::new(format!("TOOLING_FAILURE: review runner failed: {e:#}")))?;
    let duration_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
    // payload_error must be checked BEFORE exit_code / verdict parsing.
    // A runner can return exit_code=0 with payload_error set (schema/payload-shape
    // failure). Proceeding to parse stdout in that case can produce a false PASS.
    //
    // IMPORTANT: persist runner telemetry (stdout/stderr/transcript/log) BEFORE
    // returning Err.  Operators debugging a TOOLING_FAILURE need the runner output;
    // skipping persist here leaves them with no artefacts.
    if let Some(pe) = out.payload_error.as_deref() {
        let tooling_failure_parsed = ParsedReviewOutput {
            verdict: ExternalReviewVerdict::ToolingFailure,
            counts: FindingCounts {
                critical: 0,
                major: 0,
                minor: 0,
            },
        };
        // Best-effort persist: if this fails, swallow the error so the primary
        // ToolingError (the payload_error reason) is what the caller sees.
        let _ = persist_review_runner_result(
            conn,
            review_display_id,
            &bundle,
            &effective_review,
            codex,
            &out,
            &tooling_failure_parsed,
            duration_ms,
        );
        return Err(ToolingError::new(format!(
            "TOOLING_FAILURE: runner payload error: {pe}"
        )));
    }
    let parsed = if out.exit_code == 0 {
        parse_runner_review_output(&out).map_err(|e| {
            ToolingError::new(format!(
                "TOOLING_FAILURE: external review output parse failed: {e}"
            ))
        })?
    } else {
        ParsedReviewOutput {
            verdict: ExternalReviewVerdict::ToolingFailure,
            counts: FindingCounts {
                critical: 0,
                major: 0,
                minor: 0,
            },
        }
    };
    persist_review_runner_result(
        conn,
        review_display_id,
        &bundle,
        &effective_review,
        codex,
        &out,
        &parsed,
        duration_ms,
    )
    .map_err(|e| {
        ToolingError::new(format!(
            "TOOLING_FAILURE: persist review result failed: {e}"
        ))
    })?;
    Ok(parsed)
}

fn review_output_text(out: &RunnerOutput) -> &str {
    out.final_message.as_deref().unwrap_or(&out.stdout)
}

fn external_review_columns(conn: &Connection) -> Result<std::collections::BTreeSet<String>> {
    let mut stmt = conn.prepare("PRAGMA table_info(external_reviews)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect::<std::result::Result<std::collections::BTreeSet<_>, _>>()
        .map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
fn persist_review_runner_result(
    conn: &Connection,
    review_display_id: &str,
    bundle: &ReviewInputBundle,
    review: &ReviewCfg,
    codex: &CodexCfg,
    out: &RunnerOutput,
    parsed: &ParsedReviewOutput,
    duration_ms: i64,
) -> Result<()> {
    let cols = external_review_columns(conn)?;
    let mut updates: Vec<(&str, String)> = Vec::new();
    let verdict = parsed.verdict.as_str().to_string();
    let critical = parsed.counts.critical.to_string();
    let major = parsed.counts.major.to_string();
    let minor = parsed.counts.minor.to_string();
    let metadata = serde_json::json!({
        "runner": review.runner,
        "configured_model": review.model,
        "harness_id": out.telemetry.harness_id,
        "provider_id": out.telemetry.provider_id,
        "api_id": out.telemetry.api_id,
        "configured_harness_id": out.telemetry.configured_harness_id,
        "configured_model_id": out.telemetry.configured_model_id,
        "session_id": out.session_id,
        "structured_output_source": out.structured_output_source,
        "exit_code": out.exit_code,
        "payload_error": out.payload_error,
        "transcript_path": out.telemetry.transcript_path,
        "stderr_log_path": out.telemetry.stderr_log_path,
        "codex_command": codex.command,
        "codex_args": codex.args,
    })
    .to_string();
    let findings = out
        .structured_output
        .as_ref()
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()))
        .unwrap_or_else(|| review_output_text(out).to_string());
    let started_at = out.telemetry.started_at.clone().unwrap_or_default();
    let completed_at = out.telemetry.ended_at.clone().unwrap_or_default();
    let duration = duration_ms.to_string();
    let transcript_path = out.telemetry.transcript_path.clone().unwrap_or_default();
    let log_path = out
        .telemetry
        .stderr_log_path
        .clone()
        .unwrap_or_else(|| transcript_path.clone());
    let model_id = out.telemetry.model_id.clone().unwrap_or_default();
    let base_sha = bundle.base_sha.clone();
    let head_sha = bundle.head_sha.clone();

    let candidates = [
        ("adapter", "external_review".to_string()),
        ("runner", review.runner.clone()),
        ("model_id", model_id),
        ("model_metadata", metadata),
        ("base_sha", base_sha),
        ("head_sha", head_sha),
        ("verdict", verdict),
        ("critical_count", critical),
        ("major_count", major),
        ("minor_count", minor),
        ("started_at", started_at),
        ("completed_at", completed_at),
        ("duration_ms", duration),
        ("log_path", log_path),
        ("transcript_path", transcript_path),
        ("contract_ref", format!("tasks/{}/contract", bundle.task_id)),
        ("plan_ref", format!("tasks/{}/plan", bundle.task_id)),
        ("wrap_log_ref", format!("tasks/{}/wrap_log", bundle.task_id)),
        (
            "diff_ref",
            format!("{}..{}", bundle.base_sha, bundle.head_sha),
        ),
        (
            "prior_review_ref",
            format!("external_reviews/task/{}", bundle.task_id),
        ),
        ("findings", findings),
    ];
    for (col, val) in candidates {
        if cols.contains(col) {
            updates.push((col, val));
        }
    }
    if updates.is_empty() {
        return Ok(());
    }
    let set_sql = updates
        .iter()
        .enumerate()
        .map(|(idx, (col, _))| format!("{col}=?{}", idx + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "UPDATE external_reviews SET {set_sql} WHERE display_id=?{}",
        updates.len() + 1
    );
    let mut values: Vec<&dyn rusqlite::ToSql> = updates
        .iter()
        .map(|(_, v)| v as &dyn rusqlite::ToSql)
        .collect();
    values.push(&review_display_id);
    conn.execute(&sql, values.as_slice())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_off_effective_review_config_selects_fake() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let old = std::env::var_os("STORES_LLM_OFF");
        std::env::set_var("STORES_LLM_OFF", "1");
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.yaml");
        std::fs::write(
            &config_path,
            "review:\n  runner: codex\nfake_runner:\n  fake_external_review: true\n",
        )
        .unwrap();
        let review = ReviewCfg {
            runner: "codex".to_string(),
            model: Some("gpt-x".to_string()),
            ..ReviewCfg::default()
        };
        let effective = effective_review_config(&config_path, &review);
        assert_eq!(effective.runner, "fake");
        let runner = build_review_runner(&effective, &CodexCfg::default()).unwrap();
        assert_eq!(runner.name(), "fake");
        match old {
            Some(v) => std::env::set_var("STORES_LLM_OFF", v),
            None => std::env::remove_var("STORES_LLM_OFF"),
        }
    }

    fn built_fake_agent_bin() -> std::path::PathBuf {
        if let Some(path) = option_env!("CARGO_BIN_EXE_stores-fake-agent") {
            return std::path::PathBuf::from(path);
        }
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        if path.file_name().and_then(|s| s.to_str()) == Some("deps") {
            path.pop();
        }
        path.push("stores-fake-agent");
        path
    }

    fn run_git(repo: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn llm_off_external_review_invokes_fake_not_codex() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let old_llm = std::env::var_os("STORES_LLM_OFF");
        let old_fake_bin = std::env::var_os("STORES_FAKE_AGENT_BIN");
        std::env::set_var("STORES_LLM_OFF", "1");
        std::env::set_var("STORES_FAKE_AGENT_BIN", built_fake_agent_bin());

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        run_git(&repo, &["init", "-b", "main"]);
        run_git(&repo, &["config", "user.email", "fake@example.test"]);
        run_git(&repo, &["config", "user.name", "Fake Test"]);
        std::fs::write(repo.join("file.txt"), "one\n").unwrap();
        run_git(&repo, &["add", "file.txt"]);
        run_git(&repo, &["commit", "-m", "init"]);

        let config_path = tmp.path().join("config.yaml");
        std::fs::write(&config_path, "fake_runner:\n  delay_ms: 0\n").unwrap();
        let codex_log = tmp.path().join("codex.log");
        let codex = CodexCfg {
            command: "/bin/sh".to_string(),
            args: vec![
                "-c".to_string(),
                format!("echo invoked >> {}; exit 42", codex_log.display()),
            ],
        };
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (display_id TEXT PRIMARY KEY, contract TEXT, plan TEXT, cycles TEXT, wrap_log TEXT, workspace_path TEXT, branch TEXT);
             CREATE TABLE external_reviews (display_id TEXT, task_id TEXT, attempt INTEGER, verdict TEXT, critical_count INTEGER, major_count INTEGER, minor_count INTEGER, findings TEXT, runner TEXT, model_id TEXT, model_metadata TEXT, transcript_path TEXT, log_path TEXT, status TEXT, base_sha TEXT, head_sha TEXT, started_at TEXT, completed_at TEXT, duration_ms INTEGER);",
        )
        .unwrap();
        conn.execute(
            r#"INSERT INTO tasks (display_id, contract, plan, cycles, wrap_log, workspace_path, branch) VALUES ('TLLM', '{"done_when":"done"}', '{"phases":[]}', '[]', '[]', ?1, NULL)"#,
            [repo.to_string_lossy().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO external_reviews (display_id, task_id, attempt) VALUES ('ERLLM', 'TLLM', 1)",
            [],
        )
        .unwrap();

        let parsed = run_external_review_attempt_with_config(
            &conn,
            "ERLLM",
            "TLLM",
            &ReviewCfg {
                runner: "codex".to_string(),
                model: Some("gpt-real".to_string()),
                ..ReviewCfg::default()
            },
            &codex,
            &config_path,
            Some(&repo),
            Some("main"),
            Some("HEAD"),
        )
        .unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Pass);
        assert!(
            !codex_log.exists(),
            "STORES_LLM_OFF external review must not invoke configured codex command"
        );
        let (runner, metadata): (String, String) = conn
            .query_row(
                "SELECT runner, model_metadata FROM external_reviews WHERE display_id='ERLLM'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(runner, "fake");
        assert!(metadata.contains("stores-fake"), "{metadata}");
        assert!(metadata.contains("codex"), "{metadata}");

        match old_llm {
            Some(v) => std::env::set_var("STORES_LLM_OFF", v),
            None => std::env::remove_var("STORES_LLM_OFF"),
        }
        match old_fake_bin {
            Some(v) => std::env::set_var("STORES_FAKE_AGENT_BIN", v),
            None => std::env::remove_var("STORES_FAKE_AGENT_BIN"),
        }
    }

    #[test]
    fn codex_output_starting_with_revise_word_parses_revise() {
        let parsed = parse_codex_review_output("REVISE\n[major] blocking issue\n").unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Revise);
        assert_eq!(parsed.counts.major, 1);
    }

    #[test]
    fn verdict_parser_accepts_bare_prefix_and_leading_word_prose_forms() {
        let cases = [
            ("PASS", ExternalReviewVerdict::Pass),
            (
                "PASS. The rate-limit classification is correct.",
                ExternalReviewVerdict::Pass,
            ),
            ("PASS The rationale follows.", ExternalReviewVerdict::Pass),
            ("VERDICT: PASS", ExternalReviewVerdict::Pass),
            ("RESULT: PASS", ExternalReviewVerdict::Pass),
            ("REVISE", ExternalReviewVerdict::Revise),
            ("REVISE. [major] fix this.", ExternalReviewVerdict::Revise),
            ("REVISE [major] fix this.", ExternalReviewVerdict::Revise),
            ("VERDICT: REVISE", ExternalReviewVerdict::Revise),
            ("RESULT: REVISE", ExternalReviewVerdict::Revise),
            ("TOOLING_FAILURE", ExternalReviewVerdict::ToolingFailure),
            (
                "TOOLING_FAILURE. Runner output was unavailable.",
                ExternalReviewVerdict::ToolingFailure,
            ),
            (
                "TOOLING_FAILURE Runner output was unavailable.",
                ExternalReviewVerdict::ToolingFailure,
            ),
            (
                "VERDICT: TOOLING_FAILURE",
                ExternalReviewVerdict::ToolingFailure,
            ),
            (
                "RESULT: TOOLING_FAILURE",
                ExternalReviewVerdict::ToolingFailure,
            ),
        ];

        for (output, expected) in cases {
            assert_eq!(parse_verdict(output).unwrap(), expected, "{output}");
        }
    }

    #[test]
    fn structured_output_path_still_ignores_text_verdict_parser() {
        let out = RunnerOutput {
            stdout: "PASS. prose that should not matter".to_string(),
            stderr: String::new(),
            exit_code: 0,
            final_message: None,
            structured_output: Some(serde_json::json!({
                "verdict": "REVISE",
                "counts": {"critical": 1, "major": 2, "minor": 3}
            })),
            session_id: Some("s".to_string()),
            structured_output_source: Some("test"),
            telemetry: crate::runner::AgentRunTelemetry::default(),
            payload_error: None,
        };

        let parsed = parse_runner_review_output(&out).unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Revise);
        assert_eq!(parsed.counts.critical, 1);
        assert_eq!(parsed.counts.major, 2);
        assert_eq!(parsed.counts.minor, 3);
    }

    #[test]
    fn codex_prose_with_revise_finding_markers_infers_revise() {
        for marker in ["[critical]", "[major]", "[minor]", "[P1]", "[P2]", "[P3]"] {
            let output = format!("Review summary: changes required.\n  {marker} fix this\n");
            let parsed = parse_codex_review_output(&output).unwrap();
            assert_eq!(
                parsed.verdict,
                ExternalReviewVerdict::Revise,
                "marker {marker} should infer REVISE"
            );
        }
    }

    #[test]
    fn codex_t103_er326_shape_p2_marker_infers_revise() {
        let output = "The implementation does not satisfy the contract.\n\n[P2] parse_verdict does not infer REVISE from codex finding markers.\n";
        let parsed = parse_codex_review_output(output).unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Revise);
    }

    #[test]
    fn codex_markdown_list_p2_marker_infers_revise() {
        let output = "The conflicted rebase path still persists a transient head SHA.\n\nFull review comments:\n\n- [P2] Capture held rebase head after abort — src/handlers/external_reviews.rs:193\n  On conflicted stale rebases, this records HEAD before git rebase --abort.\n";
        let parsed = parse_codex_review_output(output).unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Revise);
    }

    #[test]
    fn codex_markdown_list_star_major_marker_infers_revise() {
        let output = "Findings:\n\n* [major] something is wrong here\n";
        let parsed = parse_codex_review_output(output).unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Revise);
    }

    #[test]
    fn codex_markdown_list_plus_critical_marker_infers_revise() {
        let output = "Issues:\n\n+ [critical] critical problem\n";
        let parsed = parse_codex_review_output(output).unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Revise);
    }

    #[test]
    fn codex_prose_with_result_pass_line_infers_pass() {
        let parsed =
            parse_codex_review_output("Review summary: no blocking findings.\n\nResult: PASS\n")
                .unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Pass);
    }

    #[test]
    fn codex_nonempty_without_markers_needs_parse_fallback() {
        let err = parse_codex_review_output("Review summary: inconclusive prose only.\n")
            .expect_err("unmarked non-empty prose should request parse fallback");
        assert_eq!(err.to_string(), "parse-fallback-needed");
    }

    #[test]
    fn codex_token_not_leading_does_not_infer_pass_from_prose() {
        let err = parse_codex_review_output("The verdict is PASS because the prose says so.\n")
            .expect_err("non-leading PASS token must not silently pass");
        assert_eq!(err.to_string(), "parse-fallback-needed");
    }

    #[test]
    fn codex_embedded_finding_marker_does_not_infer_revise() {
        let err =
            parse_codex_review_output("Review summary mentions [P2] inline, not as a finding.\n")
                .expect_err("finding marker fallback is line-leading only");
        assert_eq!(err.to_string(), "parse-fallback-needed");
    }

    #[test]
    fn structured_review_output_parses_flat_counts() {
        let value = serde_json::json!({
            "verdict": "REVISE",
            "critical_count": 1,
            "major_count": 2,
            "minor_count": 3,
            "findings": [{"severity":"major","message":"fix"}]
        });
        let parsed = parse_structured_review_output(&value).unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Revise);
        assert_eq!(parsed.counts.critical, 1);
        assert_eq!(parsed.counts.major, 2);
        assert_eq!(parsed.counts.minor, 3);
    }

    #[test]
    fn structured_review_output_parses_nested_counts() {
        let value = serde_json::json!({
            "verdict": "PASS",
            "counts": {"critical": 0, "major": 0, "minor": 1}
        });
        let parsed = parse_structured_review_output(&value).unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Pass);
        assert_eq!(parsed.counts.critical, 0);
        assert_eq!(parsed.counts.major, 0);
        assert_eq!(parsed.counts.minor, 1);
    }

    #[test]
    fn runner_review_output_prefers_structured_output() {
        let out = RunnerOutput {
            stdout: "not parseable text".to_string(),
            stderr: String::new(),
            exit_code: 0,
            final_message: None,
            structured_output: Some(serde_json::json!({
                "verdict": "PASS",
                "critical_count": 0,
                "major_count": 0,
                "minor_count": 0
            })),
            session_id: Some("s".to_string()),
            structured_output_source: Some("pi-tool"),
            telemetry: crate::runner::AgentRunTelemetry::default(),
            payload_error: None,
        };
        let parsed = parse_runner_review_output(&out).unwrap();
        assert_eq!(parsed.verdict, ExternalReviewVerdict::Pass);
    }

    /// Helper: build a RunnerOutput that would parse as PASS but carries
    /// a payload_error — simulates pi / claude-code / codex adapters returning
    /// exit_code=0 with a schema/payload-shape failure.
    fn pass_output_with_payload_error(adapter: &str) -> RunnerOutput {
        RunnerOutput {
            stdout: format!(
                r#"{{"verdict":"PASS","critical_count":0,"major_count":0,"minor_count":0,"adapter":"{}"}}"#,
                adapter
            ),
            stderr: String::new(),
            exit_code: 0,
            final_message: Some(
                r#"{"verdict":"PASS","critical_count":0,"major_count":0,"minor_count":0}"#
                    .to_string(),
            ),
            structured_output: Some(serde_json::json!({
                "verdict": "PASS",
                "critical_count": 0,
                "major_count": 0,
                "minor_count": 0
            })),
            session_id: Some("s".to_string()),
            structured_output_source: Some("sdk"),
            telemetry: crate::runner::AgentRunTelemetry::default(),
            // exit_code=0 but payload failed schema validation upstream
            payload_error: Some(format!("{adapter}: structured output missing final_output")),
        }
    }

    /// Verify that parse_runner_review_output alone returns PASS (confirming
    /// the stdout is well-formed), then assert that payload_error.is_some()
    /// would short-circuit to TOOLING_FAILURE before parse is reached.
    fn assert_payload_error_short_circuits(out: &RunnerOutput) {
        // Without the guard, parse would succeed (PASS):
        let would_be_pass = parse_runner_review_output(out).unwrap();
        assert_eq!(
            would_be_pass.verdict,
            ExternalReviewVerdict::Pass,
            "test precondition: stdout must parse as PASS"
        );
        // The guard must catch the payload_error before we reach parse:
        assert!(
            out.payload_error.is_some(),
            "test precondition: payload_error must be set"
        );
        // Simulate the guard in run_external_review_attempt:
        let result: std::result::Result<ParsedReviewOutput, ToolingError> =
            if let Some(pe) = out.payload_error.as_deref() {
                Err(ToolingError::new(format!(
                    "TOOLING_FAILURE: runner payload error: {pe}"
                )))
            } else if out.exit_code == 0 {
                parse_runner_review_output(out).map_err(|e| {
                    ToolingError::new(format!(
                        "TOOLING_FAILURE: external review output parse failed: {e}"
                    ))
                })
            } else {
                Err(ToolingError::new("non-zero exit".to_string()))
            };
        let err = result.expect_err("payload_error must produce Err(ToolingError)");
        assert_eq!(err.verdict, ExternalReviewVerdict::ToolingFailure);
        assert!(err.message.contains("payload error"));
    }

    #[test]
    fn pi_adapter_payload_error_classifies_as_tooling_failure_not_pass() {
        assert_payload_error_short_circuits(&pass_output_with_payload_error("pi"));
    }

    #[test]
    fn claude_code_adapter_payload_error_classifies_as_tooling_failure_not_pass() {
        assert_payload_error_short_circuits(&pass_output_with_payload_error("claude-code"));
    }

    #[test]
    fn codex_adapter_payload_error_classifies_as_tooling_failure_not_pass() {
        assert_payload_error_short_circuits(&pass_output_with_payload_error("codex"));
    }
}
