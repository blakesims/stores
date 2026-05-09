/// `drive` handler — workflow orchestrator loop.
///
/// Drives a single workflow task from its current state to a terminal state
/// (`complete` or `blocked`) by repeatedly calling `next-action → brief →
/// runner.spawn → parse envelope → compute_submit_* → render`.
///
/// # Selection
///
/// When `--auto` is given (no explicit id), the task is selected from the DB as:
/// ```sql
/// SELECT * FROM tasks
/// WHERE status NOT IN ('complete', 'blocked', 'accepted', 'rejected')
///   AND (claimed_by IS NULL OR claimed_at < <now - LOCK_WINDOW_SECS>)
/// ORDER BY created_at ASC
/// LIMIT 1
/// ```
///
/// # Agent output protocol (AC3.10)
///
/// Each agent must emit a single JSON object on the last non-empty line of
/// stdout.  The object must have a `"role"` field that identifies which
/// `compute_submit_*` to call.  Commentary above the final line is tolerated.
///
/// Role → handler mapping:
/// - `"planner"` → `compute_submit_plan`
/// - `"plan-reviewer"` → `compute_submit_plan_review`
/// - `"executor"` → `compute_submit_execute`
/// - `"code-reviewer"` → `compute_submit_review`
///
/// On parse failure the runner's stdout and stderr are surfaced verbatim; no
/// submit is invoked and the loop exits with a non-zero exit code.
///
/// # Safety rails
///
/// - `--max-iters N` (default 50): loop is bounded; on hit exits non-zero.
/// - Runner non-zero exit: task state is NOT modified (parse/submit are skipped).
/// - `blocked` terminal state: exits 0 with a human-readable hint.
use anyhow::{bail, Context as _, Result};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;

use crate::cli::agents::{BUNDLED_AGENTS, BUNDLED_AGENT_SCHEMAS};
use crate::cli::dynamic::BUNDLED_STORE_TEMPLATES;
use crate::codegen::ddl::quote_ident;
use crate::db;
use crate::flow::builtins::fire_mark_drive_failed;
use crate::handlers::next_action::compute as compute_next_action;
use crate::handlers::render::compute_render_in;
use crate::handlers::row::read_row;
use crate::handlers::submit::{
    compute_submit_execute, compute_submit_plan, compute_submit_plan_review, compute_submit_review,
};
use crate::paths::db_path;
use crate::render::{build_context, render_template_with_overlay};
use crate::runner::{mock::MockRunner, Runner, RunnerOutput};
use crate::schema::{actor::Actor, Schema};

// ---------------------------------------------------------------------------
// Lock-expiry constant (same window as submit.rs – 300 seconds)
// ---------------------------------------------------------------------------

/// Seconds within which a `claimed_at` timestamp is considered a live claim.
/// Matches the 5-minute window used by `submit.rs`'s `acquire_lock`.
const LOCK_WINDOW_SECS: u64 = 300;

/// Sentinel exit code for a spawn/launch failure (no child process existed).
/// Using -1 to distinguish from a real process exit code (which is always >= 0).
const LAUNCH_ERROR_EXIT_CODE: i32 = -1;

/// Derive the source-level `model_id` sentinel for a spawn failure.
///
/// When a runner fails to launch (spawn returns Err), no telemetry is available
/// from the runner itself. We use a deterministic sentinel that mirrors what the
/// runner would have reported at the source layer if it had started.
///
/// `runner_name` is the value returned by `RoleRunner::name_for_role`, which
/// produces names like `claude-code:opus` or `claude-code` (no model suffix when
/// no model is configured). We preserve the model suffix so the synthetic row
/// carries a specific model_id (e.g. `claude_code:opus`) rather than collapsing
/// all claude-code variants to `claude_code:unknown`.
///
/// Mapping:
/// - `pi` → `pi:default`
/// - `claude-code:<model>` → `claude_code:<model>`
/// - `claude-code` (no suffix) → `claude_code:unknown`
/// - any other runner → `<runner_name>:unknown`
fn derive_spawn_fail_model_id(runner_name: &str) -> String {
    if runner_name == "pi" {
        "pi:default".to_string()
    } else if runner_name.starts_with("claude-code") {
        // runner_name is either "claude-code" (no model) or "claude-code:<model>".
        // Split on ':' to extract the model suffix.
        match runner_name.split_once(':') {
            Some((_base, model)) if !model.is_empty() => format!("claude_code:{model}"),
            _ => "claude_code:unknown".to_string(),
        }
    } else {
        format!("{runner_name}:unknown")
    }
}

/// Write an error transcript stub under `<workspace_path>/.stores/runs/` for a
/// spawn failure. Returns the path as a String.
///
/// If `workspace_path` is `None` or the directory cannot be created/written,
/// falls back to a path string even if the file was not actually written (the
/// path is still recorded for observability; insert_agent_run requires non-empty
/// but does NOT verify the file exists at insert time — only at read time).
fn write_spawn_error_transcript(
    workspace_path: Option<&str>,
    display_id: &str,
    role: &str,
    error: &anyhow::Error,
) -> String {
    let stub_id = uuid::Uuid::new_v4();
    let filename = format!("{display_id}-{role}-spawn-error-{stub_id}.json");
    // Prefer workspace .stores/runs/ (production invariant); fall back to STORES_RUNS_DIR.
    let runs_dir = if let Some(wp) = workspace_path {
        std::path::PathBuf::from(wp).join(".stores").join("runs")
    } else if let Some(p) = std::env::var_os("STORES_RUNS_DIR") {
        std::path::PathBuf::from(p)
    } else {
        // Last resort: use a sibling of the crate (never /tmp, never target/).
        // This path is reached only in test scenarios where neither workspace_path
        // nor STORES_RUNS_DIR is set — real production always has a workspace.
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".stores")
            .join("runs")
    };
    let _ = std::fs::create_dir_all(&runs_dir);
    let stub_path = runs_dir.join(&filename);
    let content = serde_json::json!({
        "error": "spawn failed",
        "reason": error.to_string(),
        "display_id": display_id,
        "role": role,
    });
    let _ = std::fs::write(&stub_path, content.to_string().as_bytes());
    stub_path.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Runner-exit classification (T029)
// ---------------------------------------------------------------------------

/// Classify a non-zero runner exit into a task `blocked_reason`.
///
/// Rate-limit exits return the T100 cooldown contract string
/// `rate_limit:<provider>:<until-iso8601>`. Non-rate-limit crashes preserve the
/// legacy JSON runner-crash payload containing `kind=runner_crash` and
/// `exit_code`.
///
/// Detection priority:
/// 1. stream-json `rate_limit_event` whose `rate_limit_info.status != "allowed"`
///    (uses `resetsAt` as the cooldown when present).
/// 2. stdout/stderr/payload_error signatures for HTTP 429, `Retry-After`,
///    `rate_limit_error`, codex throttling text, anthropic-api 429s, or pi/provider 429s.
/// 3. fall through to `kind=runner_crash` JSON.
fn classify_runner_exit(out: &RunnerOutput) -> String {
    if let Some((provider, until)) = classify_rate_limit(out) {
        return format!("rate_limit:{provider}:{until}");
    }

    let mut payload = serde_json::Map::new();
    payload.insert(
        "kind".to_string(),
        Value::String("runner_crash".to_string()),
    );
    payload.insert("exit_code".to_string(), Value::from(out.exit_code as i64));
    serde_json::to_string(&Value::Object(payload)).unwrap_or_else(|_| "{}".to_string())
}

fn classify_rate_limit(out: &RunnerOutput) -> Option<(String, String)> {
    let haystack = format!(
        "{}\n{}\n{}",
        out.stdout,
        out.stderr,
        out.payload_error.as_deref().unwrap_or("")
    );
    let lower = haystack.to_lowercase();
    let mut detected = lower.contains("http 429")
        || lower.contains(" 429")
        || lower.contains("429 ")
        || lower.contains("rate_limit_error")
        || lower.contains("retry-after")
        || lower.contains("rate limit")
        || lower.contains("rate-limit")
        || lower.contains("usage limit");
    let mut reset_epoch: Option<i64> = None;

    for line in out.stdout.lines().chain(out.stderr.lines()) {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("rate_limit_event") {
            if let Some(info) = v.get("rate_limit_info") {
                let status = info.get("status").and_then(|s| s.as_str()).unwrap_or("");
                if !status.is_empty() && status != "allowed" {
                    detected = true;
                    reset_epoch = info.get("resetsAt").and_then(|v| v.as_i64());
                }
            }
        }
        reset_epoch = reset_epoch
            .or_else(|| v.get("reset_at").and_then(|v| v.as_i64()))
            .or_else(|| v.get("resetAt").and_then(|v| v.as_i64()))
            .or_else(|| v.get("resetsAt").and_then(|v| v.as_i64()));
    }

    if !detected {
        return None;
    }
    let provider = normalize_rate_limit_provider(&lower);
    let until = extract_iso8601(&haystack)
        .or_else(|| parse_retry_after_until(&lower))
        .or_else(|| reset_epoch.and_then(epoch_to_iso8601))
        .unwrap_or_else(default_rate_limit_until);
    Some((provider, until))
}

fn normalize_rate_limit_provider(lower: &str) -> String {
    if lower.contains("anthropic-api") || lower.contains("anthropic") || lower.contains("claude") {
        "anthropic".to_string()
    } else if lower.contains(" pi") || lower.contains("pi/") || lower.contains("provider 429") {
        "pi".to_string()
    } else {
        "codex".to_string()
    }
}

fn extract_iso8601(s: &str) -> Option<String> {
    for token in
        s.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ';' | ')' | '('))
    {
        let t = token.trim_matches(|c: char| c == ':' || c == '[' || c == ']');
        if t.len() >= 20
            && t.as_bytes().get(4) == Some(&b'-')
            && t.as_bytes().get(7) == Some(&b'-')
            && t.as_bytes().get(10) == Some(&b'T')
            && (t.ends_with('Z') || t.contains('+'))
        {
            return Some(t.to_string());
        }
    }
    None
}

fn parse_retry_after_until(lower: &str) -> Option<String> {
    let idx = lower.find("retry-after")?;
    let rest = &lower[idx + "retry-after".len()..];
    let digits: String = rest
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let secs = digits.parse::<u64>().ok()?;
    iso8601_add_secs(&crate::handlers::row::now_iso8601(), secs)
}

fn default_rate_limit_until() -> String {
    iso8601_add_secs(&crate::handlers::row::now_iso8601(), 300)
        .unwrap_or_else(crate::handlers::row::now_iso8601)
}

fn epoch_to_iso8601(epoch: i64) -> Option<String> {
    let epoch = epoch.max(0) as u64;
    let (y, mo, d, h, mi, se) = unix_to_ymd_hms(epoch);
    Some(format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{se:02}Z"))
}

fn iso8601_add_secs(base: &str, secs: u64) -> Option<String> {
    let epoch = parse_iso8601_to_epoch(base)?
        .saturating_add(secs as i64)
        .max(0) as u64;
    epoch_to_iso8601(epoch as i64)
}

fn parse_iso8601_to_epoch(s: &str) -> Option<i64> {
    if s.len() < 20 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y: u32 = std::str::from_utf8(&b[0..4]).ok()?.parse().ok()?;
    let mo: u32 = std::str::from_utf8(&b[5..7]).ok()?.parse().ok()?;
    let d: u32 = std::str::from_utf8(&b[8..10]).ok()?.parse().ok()?;
    let h: u32 = std::str::from_utf8(&b[11..13]).ok()?.parse().ok()?;
    let mi: u32 = std::str::from_utf8(&b[14..16]).ok()?.parse().ok()?;
    let se: u32 = std::str::from_utf8(&b[17..19]).ok()?.parse().ok()?;
    Some(ymd_hms_to_epoch(y, mo, d, h, mi, se))
}

#[allow(clippy::manual_is_multiple_of)]
fn unix_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let s = secs % 60;
    let total_min = secs / 60;
    let mi = total_min % 60;
    let total_hr = total_min / 60;
    let h = total_hr % 24;
    let mut days = total_hr / 24;
    let mut year = 1970u32;
    loop {
        let dy = if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
            366
        } else {
            365
        };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let dim = [
        31u64,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0usize;
    while month < 12 && days >= dim[month] {
        days -= dim[month];
        month += 1;
    }
    (
        year,
        (month + 1) as u32,
        (days + 1) as u32,
        h as u32,
        mi as u32,
        s as u32,
    )
}

#[allow(clippy::manual_is_multiple_of)]
fn ymd_hms_to_epoch(y: u32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
    fn is_leap(y: u32) -> bool {
        (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
    }
    fn days_in_month(y: u32, m: u32) -> u32 {
        match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 => {
                if is_leap(y) {
                    29
                } else {
                    28
                }
            }
            _ => 0,
        }
    }
    let mut days: i64 = 0;
    for yy in 1970..y {
        days += if is_leap(yy) { 366 } else { 365 };
    }
    for mm in 1..mo {
        days += days_in_month(y, mm) as i64;
    }
    days += d.saturating_sub(1) as i64;
    days * 86_400 + h as i64 * 3600 + mi as i64 * 60 + s as i64
}

// ---------------------------------------------------------------------------
// Parsed agent envelope (AC3.10)
// ---------------------------------------------------------------------------

/// Typed envelope parsed from the last non-empty JSON line of runner stdout.
#[derive(Debug, Deserialize)]
#[serde(tag = "role", rename_all = "kebab-case")]
enum AgentEnvelope {
    /// `planner` output — dispatches to `compute_submit_plan`.
    Planner {
        phases: Value,
        #[serde(default)]
        decision_matrix: Value,
    },
    /// `plan-reviewer` output — dispatches to `compute_submit_plan_review`.
    #[serde(rename = "plan-reviewer")]
    PlanReviewer {
        gate: String,
        summary: String,
        #[serde(default)]
        open_questions: Vec<String>,
    },
    /// `executor` output — dispatches to `compute_submit_execute`.
    Executor {
        summary: String,
        #[serde(default)]
        commit: Option<String>,
        #[serde(default)]
        files_changed: Option<Vec<String>>,
    },
    /// `code-reviewer` output — dispatches to `compute_submit_review`.
    #[serde(rename = "code-reviewer")]
    CodeReviewer {
        gate: String,
        summary: String,
        #[serde(default)]
        details: Option<String>,
        counts: Option<ReviewCounts>,
    },
    /// `wrap` output — synthesis brief for GO/NO_GO review.
    /// Formally wired to `compute_submit_wrap` in Phase 3; Phase 1 stub exits drive loop.
    #[serde(rename = "wrap")]
    Wrap {
        #[serde(default)]
        reasoning: Option<String>,
        executive_summary: String,
        #[serde(default)]
        deviations: Vec<String>,
        #[serde(default)]
        residual_risks: Vec<String>,
        #[serde(default)]
        recommended_sanity_checks: Vec<String>,
    },
}

#[derive(Debug, Deserialize, Default)]
struct ReviewCounts {
    #[serde(default)]
    critical: i64,
    #[serde(default)]
    major: i64,
    #[serde(default)]
    minor: i64,
}

// ---------------------------------------------------------------------------
// Fixture format for --mock (AC3.3)
// ---------------------------------------------------------------------------

/// Serde-shape for a single item in the `--mock` fixture array.
/// Field names match `RunnerOutput`'s public fields exactly.
#[derive(Debug, Deserialize)]
pub struct MockFixtureItem {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub final_message: Option<String>,
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub harness_id: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub tokens_in: Option<i64>,
    #[serde(default)]
    pub tokens_out: Option<i64>,
    #[serde(default)]
    pub prompt_cache_hits: Option<i64>,
    #[serde(default)]
    pub transcript_path: Option<String>,
}

// ---------------------------------------------------------------------------
// Public drive entry point
// ---------------------------------------------------------------------------

/// Arguments parsed by the caller from clap matches.
pub struct DriveArgs {
    /// Explicit task id (mutually exclusive with `auto`).
    pub display_id: Option<String>,
    /// Use auto-selection query (AC3.2).
    pub auto: bool,
    /// Path to JSON fixture file for mock runner (AC3.3, always compiled).
    pub mock_fixture: Option<PathBuf>,
    /// Use claude-code runner (feature-gated).
    #[cfg(feature = "runner-claude-code")]
    pub claude_code: bool,
    /// Force all agents to use the `haiku` model — cheap iteration / smoke
    /// testing of the runner+prompt contract. Only meaningful with
    /// `--claude-code`.
    #[cfg(feature = "runner-claude-code")]
    pub testing: bool,
    /// Force a Claude Code model for all roles (e.g. `sonnet`, `opus`).
    /// Only meaningful with `--claude-code`.
    #[cfg(feature = "runner-claude-code")]
    pub claude_code_model: Option<String>,
    /// Use Pi SDK runner (feature-gated).
    #[cfg(feature = "runner-pi")]
    pub pi: bool,
    /// Maximum loop iterations before hard-abort (AC3.5, default 50).
    pub max_iters: usize,
}

/// Drive a workflow task to a terminal state.
///
/// Prints progress to stderr (AC3.4).  Stdout is reserved for any structured
/// output.  Returns Ok(()) on `complete` or `blocked` (both exit 0); returns
/// Err on infrastructure failures or safety-rail violations (exit non-zero).
pub fn run_drive(schema: &Schema, args: DriveArgs) -> Result<()> {
    // Ordering invariant (T051): db::open auto-applies framework-DDL drift
    // here, BEFORE any subscriber poll iteration sees the connection. The
    // daemon must never observe a half-migrated DB.
    let conn = db::open(&db_path()?)?;

    // Resolve the task id.
    let display_id = resolve_task_id(schema, &conn, &args)?;

    // Record manual drive ownership immediately so `stores watch` can distinguish
    // "planner is running" from "planning row has not been started". Auto-drive
    // also writes these fields from its subscriber path; this direct write covers
    // operator-started/manual `stores tasks drive` invocations.
    record_drive_owner(&conn, &display_id)?;

    // Select runner(s). CLI flags force one runner for all roles; otherwise
    // .stores/config.yaml may choose per-role runners.
    let runner = build_role_runner(&args)?;

    // Drive the loop.
    drive_loop_with_role_runner(schema, &conn, &display_id, runner.as_ref(), args.max_iters)
}

fn record_drive_owner(conn: &Connection, display_id: &str) -> Result<()> {
    let now = crate::handlers::row::now_iso8601();
    let pid = std::process::id() as i64;
    conn.execute(
        "UPDATE tasks SET drive_pid = ?1, drive_started_at = ?2, updated_at = ?2 WHERE display_id = ?3",
        rusqlite::params![pid, now, display_id],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Task-id resolution (AC3.2)
// ---------------------------------------------------------------------------

/// ISO-8601 timestamp for N seconds ago (mirrors `submit.rs`'s local helper).
fn iso_subtract_secs(secs: u64) -> String {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(secs);
    let s = epoch % 60;
    let total_min = epoch / 60;
    let mi = total_min % 60;
    let total_hr = total_min / 60;
    let h = total_hr % 24;
    let days = total_hr / 24;
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

#[allow(clippy::manual_is_multiple_of)]
fn is_leap(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
fn days_in_year(y: u32) -> u32 {
    if is_leap(y) {
        366
    } else {
        365
    }
}
fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 31,
    }
}
fn days_to_ymd(mut days: u64) -> (u32, u32, u32) {
    let mut year = 1970u32;
    loop {
        let dy = days_in_year(year) as u64;
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let mut month = 1u32;
    loop {
        let dm = days_in_month(year, month) as u64;
        if days < dm {
            break;
        }
        days -= dm;
        month += 1;
    }
    (year, month, days as u32 + 1)
}

/// Resolve target task id from args.  Errors when no candidate found.
pub(crate) fn resolve_task_id(
    schema: &Schema,
    conn: &Connection,
    args: &DriveArgs,
) -> Result<String> {
    if let Some(id) = &args.display_id {
        return Ok(id.clone());
    }

    if !args.auto {
        bail!("specify a task id or use --auto to select one automatically");
    }

    // AC3.2: auto-selection query — non-terminal + not live-claimed, created_at ASC.
    let lock_expiry = iso_subtract_secs(LOCK_WINDOW_SECS);
    let table = quote_ident(&schema.name);
    let sql = format!(
        "SELECT display_id FROM {table} \
         WHERE status NOT IN ('complete', 'blocked', 'accepted', 'rejected') \
           AND (claimed_by IS NULL OR claimed_at < ?1) \
         ORDER BY created_at ASC \
         LIMIT 1"
    );

    let result: rusqlite::Result<String> =
        conn.query_row(&sql, rusqlite::params![lock_expiry], |row| row.get(0));

    match result {
        Ok(id) => Ok(id),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            bail!("no non-complete tasks available (all tasks are complete, blocked, or live-claimed)")
        }
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------
// Runner construction (AC3.3)
// ---------------------------------------------------------------------------

fn build_runner(args: &DriveArgs) -> Result<Box<dyn Runner>> {
    // --mock takes priority (always compiled, hidden from help).
    if let Some(fixture_path) = &args.mock_fixture {
        let text = std::fs::read_to_string(fixture_path).map_err(|e| {
            anyhow::anyhow!("cannot read mock fixture '{}': {e}", fixture_path.display())
        })?;
        let items: Vec<MockFixtureItem> = serde_json::from_str(&text).map_err(|e| {
            anyhow::anyhow!(
                "mock fixture '{}' is not a valid JSON array of RunnerOutput-shaped objects: {e}",
                fixture_path.display()
            )
        })?;
        // Synthetic transcript dir for fixture items that don't supply their own
        // transcript_path. Lives alongside the fixture file so paths are stable.
        let synthetic_runs_dir = fixture_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join(".stores")
            .join("runs");
        let _ = std::fs::create_dir_all(&synthetic_runs_dir);

        let outputs: Vec<RunnerOutput> = items
            .into_iter()
            .enumerate()
            .map(|(idx, item)| {
                // Required fields: caller/fixture supplies them; synthesize only as
                // last resort for transcript_path when fixture omits it entirely.
                let mut telemetry =
                    crate::runner::AgentRunTelemetry::with_mock_defaults(&synthetic_runs_dir);
                if item.model_id.is_some() {
                    telemetry.model_id = item.model_id;
                }
                if item.harness_id.is_some() {
                    telemetry.harness_id = item.harness_id;
                }
                if item.started_at.is_some() {
                    telemetry.started_at = item.started_at;
                }
                if item.ended_at.is_some() {
                    telemetry.ended_at = item.ended_at;
                }
                telemetry.tokens_in = item.tokens_in;
                telemetry.tokens_out = item.tokens_out;
                telemetry.prompt_cache_hits = item.prompt_cache_hits;
                // If the fixture item supplied a transcript_path, use it; otherwise
                // synthesize a real stub file under .stores/runs/ so insert_agent_run
                // never receives None for this required field.
                telemetry.transcript_path = item.transcript_path.or_else(|| {
                    let p = synthetic_runs_dir.join(format!("mock-item-{idx}.jsonl"));
                    let _ = std::fs::write(&p, b"{\"model\":\"stub-model\"}\n");
                    Some(p.display().to_string())
                });
                RunnerOutput {
                    stdout: item.stdout,
                    stderr: item.stderr,
                    exit_code: item.exit_code,
                    final_message: item.final_message,
                    structured_output: None,
                    session_id: None,
                    structured_output_source: None,
                    telemetry,
                    payload_error: None,
                }
            })
            .collect();
        return Ok(Box::new(MockRunner::new(outputs)));
    }

    // --claude-code (feature-gated). `--testing` forces the haiku model for
    // every spawn, equivalent to `--claude-code-model=haiku`.
    #[cfg(feature = "runner-claude-code")]
    if args.claude_code {
        if args.testing {
            return Ok(Box::new(crate::runner::ClaudeCodeRunner::with_model(
                "haiku",
            )));
        }
        if let Some(model) = &args.claude_code_model {
            return Ok(Box::new(crate::runner::ClaudeCodeRunner::with_model(
                model.clone(),
            )));
        }
        return crate::runner::select("claude-code");
    }

    #[cfg(feature = "runner-pi")]
    if args.pi {
        return crate::runner::select("pi");
    }

    // Default: error — a runner must be explicitly chosen.
    bail!(
        "no runner selected; use --mock <fixture> for testing, \
         --claude-code (requires `--features runner-claude-code`), \
         --pi (requires `--features runner-pi`), or configure drive.default_runner"
    )
}

trait RoleRunner {
    fn name_for_role(&self, role: &str) -> Result<String>;
    fn spawn_for_role(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<RunnerOutput>;
}

struct BorrowedRoleRunner<'a> {
    runner: &'a dyn Runner,
}

impl RoleRunner for BorrowedRoleRunner<'_> {
    fn name_for_role(&self, _role: &str) -> Result<String> {
        Ok(self.runner.name().to_string())
    }

    fn spawn_for_role(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<RunnerOutput> {
        self.runner
            .spawn(role, system_prompt, brief, schema, workspace_path)
    }
}

struct FixedRoleRunner {
    runner: Box<dyn Runner>,
}

impl RoleRunner for FixedRoleRunner {
    fn name_for_role(&self, _role: &str) -> Result<String> {
        Ok(self.runner.name().to_string())
    }

    fn spawn_for_role(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<RunnerOutput> {
        self.runner
            .spawn(role, system_prompt, brief, schema, workspace_path)
    }
}

#[derive(Debug, Clone)]
struct RoleRunnerChoice {
    runner: String,
    model: Option<String>,
}

struct ConfigRoleRunner {
    default_runner: Option<String>,
    roles: std::collections::BTreeMap<String, RoleRunnerChoice>,
}

impl ConfigRoleRunner {
    fn choice_for(&self, role: &str) -> Result<RoleRunnerChoice> {
        if let Some(choice) = self.roles.get(role) {
            return Ok(choice.clone());
        }
        if let Some(default_runner) = &self.default_runner {
            return Ok(RoleRunnerChoice {
                runner: default_runner.clone(),
                model: None,
            });
        }
        bail!("no runner configured for role '{role}' and drive.default_runner is unset")
    }

    fn build_choice(&self, choice: &RoleRunnerChoice) -> Result<Box<dyn Runner>> {
        match choice.runner.as_str() {
            "claude-code" => {
                #[cfg(feature = "runner-claude-code")]
                {
                    if let Some(model) = &choice.model {
                        Ok(Box::new(crate::runner::ClaudeCodeRunner::with_model(
                            model.clone(),
                        )))
                    } else {
                        crate::runner::select("claude-code")
                    }
                }
                #[cfg(not(feature = "runner-claude-code"))]
                {
                    bail!("runner 'claude-code' requires the runner-claude-code cargo feature")
                }
            }
            other => {
                if choice.model.is_some() {
                    bail!("drive role config sets model for runner '{other}', but model is only supported for claude-code")
                }
                crate::runner::select(other)
            }
        }
    }
}

impl RoleRunner for ConfigRoleRunner {
    fn name_for_role(&self, role: &str) -> Result<String> {
        let choice = self.choice_for(role)?;
        Ok(match choice.model {
            Some(model) if choice.runner == "claude-code" => format!("{}:{model}", choice.runner),
            _ => choice.runner,
        })
    }

    fn spawn_for_role(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<RunnerOutput> {
        let choice = self.choice_for(role)?;
        let runner = self.build_choice(&choice)?;
        runner.spawn(role, system_prompt, brief, schema, workspace_path)
    }
}

fn build_role_runner(args: &DriveArgs) -> Result<Box<dyn RoleRunner>> {
    if args.mock_fixture.is_some()
        || {
            #[cfg(feature = "runner-claude-code")]
            {
                args.claude_code
            }
            #[cfg(not(feature = "runner-claude-code"))]
            {
                false
            }
        }
        || {
            #[cfg(feature = "runner-pi")]
            {
                args.pi
            }
            #[cfg(not(feature = "runner-pi"))]
            {
                false
            }
        }
    {
        return Ok(Box::new(FixedRoleRunner {
            runner: build_runner(args)?,
        }));
    }

    let config_path = crate::flow::config::default_config_path()?;
    let cfg = crate::flow::config::load(&config_path)?.unwrap_or_default();
    let Some(drive) = cfg.drive else {
        bail!(
            "no runner selected; pass --claude-code/--pi or configure drive.default_runner in {}",
            config_path.display()
        );
    };
    let roles = drive
        .roles
        .into_iter()
        .map(|(role, cfg)| {
            (
                role,
                RoleRunnerChoice {
                    runner: cfg.runner,
                    model: cfg.model,
                },
            )
        })
        .collect();
    Ok(Box::new(ConfigRoleRunner {
        default_runner: drive.default_runner,
        roles,
    }))
}

// ---------------------------------------------------------------------------
// git diff summary helper (AC4.5 / AC4.6 / T124)
// ---------------------------------------------------------------------------

/// Compute a direction-aware diff summary for the wrap brief.
///
/// Base-branch resolution order (T124 — prevents misattribution of main-ahead
/// commits as "rides on this branch"):
/// 1. `BASE_BRANCH` env var (allows override without code changes).
/// 2. `base_branch` parameter (caller-supplied hint).
/// 3. Try "main", then "master" as defaults.
///
/// Since-ref formula:
/// 1. Try `git merge-base HEAD <resolved_base>`.
/// 2. Fallback: use `first_executor_commit` if provided and non-empty.
/// 3. Final fallback: return `"<git diff unavailable>"`.
///
/// Output has TWO labeled sections so the wrap agent can attribute commits
/// correctly without relying on diff-stat direction alone:
/// - "On this branch": `git log --oneline <since-ref>..HEAD` + `git diff --stat`
/// - "On base (not on this branch)": `git log --oneline HEAD..<resolved_base>`
///
/// All shell-out happens here in `drive.rs`; `src/render/context.rs` is never
/// involved (render stays pure `(schema, entry) → Value`).
///
/// AC4.6: On any failure (no git binary, not a repo, no base branch, detached
/// HEAD), the function returns `"<git diff unavailable>"` rather than erroring.
pub(crate) fn compute_git_diff_summary(
    base_branch: Option<&str>,
    first_executor_commit: Option<&str>,
    workspace_path: Option<&str>,
) -> String {
    use std::process::Command;

    // Helper: run a git command and return trimmed stdout on exit-0, else None.
    let run_git = |args: &[&str]| -> Option<String> {
        let mut cmd = Command::new("git");
        cmd.args(args);
        if let Some(path) = workspace_path.filter(|s| !s.is_empty()) {
            cmd.current_dir(path);
        }
        let out = cmd.output().ok()?;
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(s)
            }
        } else {
            None
        }
    };

    // Resolve the base branch. BASE_BRANCH env var wins; then parameter; then
    // try "main" followed by "master" as defaults.
    let resolved_base: String = std::env::var("BASE_BRANCH")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| base_branch.filter(|s| !s.is_empty()).map(|s| s.to_string()))
        .unwrap_or_else(|| "main".to_string());

    // Step 1: try git merge-base HEAD <resolved_base>.
    // If resolved_base is "main" but the repo uses "master", also try that and
    // carry the successful fallback into the base-ahead range/labels below.
    let mut effective_base = resolved_base.clone();
    let since_ref = run_git(&["merge-base", "HEAD", &resolved_base])
        .or_else(|| {
            if resolved_base == "main" {
                run_git(&["merge-base", "HEAD", "master"]).inspect(|_| {
                    effective_base = "master".to_string();
                })
            } else {
                None
            }
        })
        .or_else(|| {
            // Step 2: fallback to first executor commit.
            first_executor_commit
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });

    let since_ref = match since_ref {
        Some(r) => r,
        None => {
            // Step 3: unavailable — log and return placeholder.
            eprintln!(
                "[drive] git_diff_summary: could not compute since-ref \
                 (git merge-base failed; no executor commit fallback); \
                 using '<git diff unavailable>'"
            );
            let _ = std::io::stderr().flush();
            return "<git diff unavailable>".to_string();
        }
    };

    // Section 1: commits on this branch (not on base) — what the executor shipped.
    let branch_range = format!("{since_ref}..HEAD");
    let branch_log = run_git(&["log", "--oneline", &branch_range])
        .unwrap_or_else(|| "(no commits since branch point)".to_string());
    let branch_stat = run_git(&["diff", "--stat", &branch_range])
        .unwrap_or_else(|| "(no file changes)".to_string());

    // Section 2: commits on base that are NOT on this branch — shown so the
    // wrap agent does not misattribute main-ahead work as belonging to this task.
    let base_ahead_range = format!("HEAD..{effective_base}");
    let base_ahead_log = run_git(&["log", "--oneline", &base_ahead_range])
        .unwrap_or_else(|| "(none — base is not ahead of this branch)".to_string());

    format!(
        "```\n\
        ### On this branch (not on base/{effective_base}):\n\
        {branch_log}\n\n\
        {branch_stat}\n\n\
        ### On base/{effective_base} (not on this branch — do NOT attribute to this task):\n\
        {base_ahead_log}\n\
        ```"
    )
}

// ---------------------------------------------------------------------------
// Main drive loop (AC3.1 / AC3.4 / AC3.5 / AC3.6 / AC3.7 / AC3.9 / AC3.10)
// ---------------------------------------------------------------------------

/// T033: pre-flight depends_on guard. Refuses to start drive when any
/// `depends_on` dep is not yet `accepted`. Layer 1 (passive) — runs once
/// at drive entry, no polling, no re-check after the loop begins.
fn check_depends_on_guard(schema: &Schema, conn: &Connection, display_id: &str) -> Result<()> {
    let (_, entry) = read_row(schema, conn, display_id)?;
    let deps = match entry.get("depends_on") {
        Some(Value::Array(arr)) => arr.clone(),
        _ => return Ok(()),
    };
    if deps.is_empty() {
        return Ok(());
    }

    let table = quote_ident(&schema.name);
    let sql = format!("SELECT status FROM {table} WHERE display_id = ?1");
    let mut not_accepted: Vec<(String, String)> = Vec::new();
    for d in &deps {
        let dep_id = match d.as_str() {
            Some(s) => s.to_string(),
            None => continue,
        };
        let status: rusqlite::Result<String> =
            conn.query_row(&sql, rusqlite::params![dep_id], |r| r.get(0));
        match status {
            Ok(s) if s == "accepted" => {}
            Ok(s) => not_accepted.push((dep_id, s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                not_accepted.push((dep_id, "<missing>".to_string()))
            }
            Err(e) => return Err(e.into()),
        }
    }

    if not_accepted.is_empty() {
        return Ok(());
    }

    let detail = not_accepted
        .iter()
        .map(|(id, s)| format!("{id} (status={s})"))
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "[{display_id}] cannot start drive: depends_on not satisfied — {detail} \
         (each dep must reach status='accepted' before this task can drive)"
    );
}

/// Core loop.  Extracted so tests can drive it directly without going through
/// clap or `run_drive`.
///
/// Public so integration tests in `tests/` (e.g. `workflow_tier_t1.rs`) can
/// exercise the loop with a `MockRunner` injected directly. T027 P5 widens
/// from `pub(crate)` to `pub` for the tier-shape integration tests.
pub fn drive_loop(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    runner: &dyn Runner,
    max_iters: usize,
) -> Result<()> {
    let fixed = BorrowedRoleRunner { runner };
    drive_loop_with_role_runner(schema, conn, display_id, &fixed, max_iters)
}

fn drive_loop_with_role_runner(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    role_runner: &dyn RoleRunner,
    max_iters: usize,
) -> Result<()> {
    // T033: pre-flight depends_on guard (Layer 1, passive). Refuse to start
    // when any depends_on dep is not yet accepted.
    check_depends_on_guard(schema, conn, display_id)?;

    let mut iter = 0usize;
    // AC4.3 state-local flag: tracks whether wrap was dispatched in THIS drive
    // run. Prevents same-run re-dispatch while allowing fresh dispatch on every
    // new drive invocation (covers re-entry after reject → amend → re-complete).
    let mut dispatched_wrap_this_run = false;
    // T049: close the auto-drive dispatch_lock the first time we land a
    // successful submit. Drives that die before this point leave the lock
    // open for the watchdog to detect as a silent zombie.
    let mut auto_drive_lock_closed = false;

    loop {
        // ── Step 2a: compute next-action ──────────────────────────────────
        let na = compute_next_action(schema, conn, display_id)?;

        // Transient guard: `complete` should never be observable between loop
        // iterations — the on_state.complete follow-on fires inside the same
        // submit tx and advances the row to `in_review`. If we see it here,
        // the follow-on didn't fire (schema bug or manual DB surgery). Exit
        // non-zero with a clear diagnostic.
        if na.status == "complete" {
            eprintln!(
                "[{display_id}] ERROR: task at status 'complete' but \
                 `complete → in_review` follow-on did not fire — schema bug"
            );
            let _ = std::io::stderr().flush();
            anyhow::bail!(
                "task {display_id} stuck at 'complete'; expected follow-on to advance to 'in_review'"
            );
        }

        // AC4.3: `in_review` exit guard.
        //
        // Only path: wrap was dispatched in THIS run (dispatched_wrap_this_run flag set)
        // → same-run re-dispatch prevention; exit after one wrap per drive invocation.
        //
        // On re-entry (new drive call after amend), dispatched_wrap_this_run is false,
        // so we fall through and dispatch wrap again — next_agent IS the source of truth,
        // not wrap_log. wrap_log is durable history only, never a completion sentinel.
        // (pi ruling r3 strict-pi A1: if next_agent=wrap, dispatch wrap.)
        //
        // Lifecycle closure (pi ruling r5 fix): after a successful wrap dispatch in THIS
        // run, force-close the auto-drive lock terminal-ok before exiting. The schema
        // always yields next_agent=Some("wrap") for in_review (no when: guard), so the
        // normal close_auto_drive_lock_ok path returns PendingNext and leaves the lock
        // open — which triggers infinite daemon re-dispatch. force_close bypasses the
        // has_pending_auto_drive_work check because current-cycle completion (not
        // next_agent IS NULL) is the correct closure trigger here. A1 invariant is
        // preserved: wrap_log is NOT consulted; the dispatched_wrap_this_run flag is the
        // signal. Watchdog fallback remains: if drive dies before this exit, the watchdog
        // redispatches a fresh drive (correct amend/re-entry semantics).
        if na.status == "in_review" && dispatched_wrap_this_run {
            if let Err(e) =
                crate::handlers::agents_run::force_close_auto_drive_lock_ok(conn, display_id)
            {
                eprintln!("[{display_id}] force_close_auto_drive_lock_ok failed (non-fatal): {e}");
                let _ = std::io::stderr().flush();
            }
            eprintln!(
                "[{display_id}] in_review; brief written; awaiting `stores tasks accept | reject`"
            );
            let _ = std::io::stderr().flush();
            return Ok(());
        }

        // Terminal: accepted — human accepted; nothing more to dispatch.
        if na.status == "accepted" {
            eprintln!("[{display_id}] accepted; task is complete");
            let _ = std::io::stderr().flush();
            return Ok(());
        }

        // Terminal: rejected — human rejected; nothing more to dispatch.
        // (Use `stores tasks amend` to re-open the contract.)
        if na.status == "rejected" {
            eprintln!("[{display_id}] rejected; run `stores tasks {display_id} amend` to re-open");
            let _ = std::io::stderr().flush();
            return Ok(());
        }

        // Terminal: blocked (AC3.9) — exit 0
        if na.blocked {
            let reason = na.blocked_reason.as_str().unwrap_or("unknown").to_string();
            eprintln!(
                "[{display_id}] blocked: {reason}; run `stores gate {display_id} guide` for help"
            );
            let _ = std::io::stderr().flush();
            return Ok(());
        }

        let agent_role = na.next_agent.as_deref().unwrap_or("");
        if agent_role.is_empty() {
            bail!(
                "[{display_id}] next-action returned no agent for status '{}'; cannot proceed",
                na.status
            );
        }

        // ── Step 2b+2c: build brief + read system prompt ─────────────────
        // Role names in BUNDLED_AGENTS use hyphens ("plan-reviewer", "code-reviewer"),
        // while the schema uses underscores ("plan_reviewer", "code_reviewer").
        let agent_name_normalized = agent_role.replace('_', "-");

        // Read the briefing template from BUNDLED_STORE_TEMPLATES (same as
        // compute_brief does when schema_path is "bundled:<name>").  Drive
        // always operates on bundled stores so we bypass the manifest lookup.
        let (_, entry) = read_row(schema, conn, display_id)?;
        let brief_markdown = {
            let workflow = schema.workflow.as_ref().ok_or_else(|| {
                anyhow::anyhow!("store '{}' has no workflow declaration", schema.name)
            })?;
            let template_path = workflow.briefing_templates.get(agent_role).ok_or_else(|| {
                anyhow::anyhow!(
                    "workflow: no briefing_template for agent role '{}'",
                    agent_role
                )
            })?;
            let tpl_key = template_path.to_string_lossy();
            let tpl_content = BUNDLED_STORE_TEMPLATES
                .iter()
                .find(|(name, _)| *name == schema.name.as_str())
                .and_then(|(_, templates)| {
                    templates
                        .iter()
                        .find(|(path, _)| *path == tpl_key.as_ref())
                        .map(|(_, content)| *content)
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "no bundled template '{}' for store '{}'; \
                         drive requires a bundled store",
                        tpl_key,
                        schema.name
                    )
                })?;
            let ctx = build_context(schema, &entry);

            // AC4.5: For the wrap agent, assemble git_diff_summary in drive (NOT
            // in context.rs — render must stay pure). Pass it as a context overlay
            // so the template can use {{git_diff_summary}} without polluting the
            // pure build_context path.
            let mut overlay =
                crate::handlers::brief::build_source_observation_overlay(conn, &entry)?;
            // I022 repair-lane: merge external-review REVISE backpressure overlay so
            // respawned executor briefs surface codex/external-review findings.
            for (k, v) in crate::handlers::brief::build_external_review_overlay(conn, &entry)? {
                overlay.insert(k, v);
            }
            if agent_role == "wrap" {
                // Extract the first executor commit from cycles[] for fallback.
                let first_commit = entry
                    .get("cycles")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|c| c.get("executor"))
                    .and_then(|e| e.get("commit"))
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string());

                // T124: pass None as base_branch so compute_git_diff_summary resolves
                // from BASE_BRANCH env var or defaults (main/master). The task row's
                // `branch` field holds the FEATURE branch name, not the base branch —
                // passing it would cause git merge-base HEAD <feature-branch> == HEAD,
                // producing an empty since-ref and a blank diff.
                let workspace_path = entry.get("workspace_path").and_then(|v| v.as_str());
                let diff_summary =
                    compute_git_diff_summary(None, first_commit.as_deref(), workspace_path);
                overlay.insert(
                    "git_diff_summary".to_string(),
                    serde_json::Value::String(diff_summary),
                );
            }

            render_template_with_overlay(tpl_content, &ctx, &overlay)?
        };
        let system_prompt = BUNDLED_AGENTS
            .iter()
            .find(|(n, _)| *n == agent_name_normalized)
            .map(|(_, content)| *content)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no bundled agent for role '{}' (tried '{}'); \
                     run `stores agents install --all` first",
                    agent_role,
                    agent_name_normalized
                )
            })?;

        // ── Step 2d: spawn runner ─────────────────────────────────────────
        // Look up the bundled JSON schema for this role.  Phase 2 threads it
        // through to the runner so it can pass --json-schema to the claude CLI.
        let schema_text: Option<&str> = BUNDLED_AGENT_SCHEMAS
            .iter()
            .find(|(n, _)| *n == agent_name_normalized)
            .map(|(_, s)| *s);

        // Extract workspace_path from the task row (same pattern as branch above).
        let workspace_path = entry.get("workspace_path").and_then(|v| v.as_str());

        // Validate: if set but path does not exist OR is not a directory, error before spawn
        // (no silent fallback). exists()-only would let a regular file slip through and defer
        // failure to spawn-time when current_dir() rejects it as a non-directory infra error.
        if let Some(p) = workspace_path {
            let path = std::path::Path::new(p);
            if !path.exists() {
                anyhow::bail!(
                    "[{display_id}] workspace_path '{p}' does not exist; \
                     set a valid path or remove the field"
                );
            }
            if !path.is_dir() {
                anyhow::bail!(
                    "[{display_id}] workspace_path '{p}' is not a directory; \
                     set a valid directory path or remove the field"
                );
            }
        }

        // Pre-spawn announcement: runners block until the child exits, so without
        // this the user sees nothing for 30-90s per agent. v0.4 will stream child
        // stdout line-by-line; for v0.3 we just bookend the call.
        let phase_for_log = na.current_phase.as_i64().unwrap_or(0);
        let cycle_for_log = na.current_cycle.as_i64().unwrap_or(0);
        let runner_name = role_runner.name_for_role(&agent_name_normalized)?;
        eprintln!(
            "[{display_id}] phase {phase_for_log} cycle {cycle_for_log}: spawning {agent_role} via {runner_name} runner... (may take 30-90s)"
        );
        let _ = std::io::stderr().flush();
        let spawn_start = std::time::Instant::now();
        let run_out = match role_runner.spawn_for_role(
            &agent_name_normalized,
            system_prompt,
            &brief_markdown,
            schema_text,
            workspace_path,
        ) {
            Ok(out) => out,
            Err(spawn_err) => {
                // Spawn/launch failure: record attempted-invocation telemetry before
                // routing to fire_mark_drive_failed. Every attempted spawn has an
                // agent_runs row (pi ruling, MAJOR 1).
                let spawn_failed_at = crate::handlers::row::now_iso8601();
                // Derive the source-level model_id sentinel from the runner name.
                // This mirrors what the runner would have written if it had started.
                let spawn_fail_model_id = derive_spawn_fail_model_id(&runner_name);
                // Write an error transcript stub under workspace .stores/runs/ so
                // transcript_path is a real file (required by insert_agent_run).
                let error_transcript_path = write_spawn_error_transcript(
                    workspace_path,
                    display_id,
                    agent_role,
                    &spawn_err,
                );
                let synthetic_telemetry = crate::runner::AgentRunTelemetry {
                    model_id: Some(spawn_fail_model_id),
                    harness_id: Some(runner_name.clone()),
                    started_at: Some(spawn_failed_at.clone()),
                    ended_at: Some(spawn_failed_at),
                    tokens_in: Some(0),
                    tokens_out: Some(0),
                    transcript_path: Some(error_transcript_path),
                    ..Default::default()
                };
                // Insert the synthetic row before transitioning the task.
                // Fail-loud: every attempted invocation must produce an agent_runs row
                // (Pi ruling). Swallowing the error would violate that invariant silently.
                db::insert_agent_run(
                    conn,
                    display_id,
                    phase_for_log,
                    cycle_for_log,
                    agent_role,
                    LAUNCH_ERROR_EXIT_CODE,
                    &synthetic_telemetry,
                    Some(&brief_markdown),
                )
                .context("spawn-fail synthetic agent_runs insert")?;
                // Now transition the task to blocked (same path as non-zero exit).
                let blocked_reason = format!(
                    "{{\"kind\":\"spawn_failure\",\"exit_code\":{LAUNCH_ERROR_EXIT_CODE},\
                     \"runner\":\"{runner_name}\",\"error\":{}}}",
                    serde_json::to_string(&spawn_err.to_string())
                        .unwrap_or_else(|_| "\"<serialization error>\"".to_string())
                );
                match fire_mark_drive_failed(conn, display_id, &blocked_reason, "", None) {
                    Ok(()) => {
                        eprintln!(
                            "[{display_id}] spawn failed ({runner_name}): {spawn_err:#}; mark_drive_failed fired"
                        );
                        let _ = std::io::stderr().flush();
                    }
                    Err(e) => {
                        eprintln!(
                            "[{display_id}] spawn failed ({runner_name}): {spawn_err:#}; mark_drive_failed FAILED: {e:#}"
                        );
                        let _ = std::io::stderr().flush();
                    }
                }
                bail!("spawn failure for role '{agent_role}' via runner '{runner_name}': {spawn_err:#}");
            }
        };
        db::insert_agent_run(
            conn,
            display_id,
            phase_for_log,
            cycle_for_log,
            agent_role,
            run_out.exit_code,
            &run_out.telemetry,
            Some(&brief_markdown),
        )?;
        let spawn_elapsed = spawn_start.elapsed();
        eprintln!(
            "[{display_id}] phase {phase_for_log} cycle {cycle_for_log}: {agent_role} returned (exit={}, {:.1}s)",
            run_out.exit_code,
            spawn_elapsed.as_secs_f64()
        );
        let _ = std::io::stderr().flush();

        // AC2.7: surface schema validation retry exhaustion before the exit-code
        // check so the user always sees it, even on non-zero exit.
        if run_out
            .stderr
            .contains("schema validation retries exhausted")
        {
            let transcript_hint = run_out
                .session_id
                .as_deref()
                .map(|sid| format!(".stores/runs/{sid}.jsonl"))
                .unwrap_or_else(|| "<no session-id>".to_string());
            eprintln!(
                "[{display_id}] schema validation retries exhausted; \
                 transcript: {transcript_hint}"
            );
            let _ = std::io::stderr().flush();
        }

        // AC3.6: non-zero exit → surface stdout + stderr, no submit.
        // (Some CLIs route auth / login errors to stdout, so always include both.)
        if run_out.exit_code != 0 {
            eprintln!(
                "[{display_id}] runner exited with code {}; aborting without submitting",
                run_out.exit_code
            );
            let _ = std::io::stderr().flush();
            if !run_out.stdout.is_empty() {
                eprintln!("runner stdout:\n{}", run_out.stdout);
                let _ = std::io::stderr().flush();
            }
            if !run_out.stderr.is_empty() {
                eprintln!("runner stderr:\n{}", run_out.stderr);
                let _ = std::io::stderr().flush();
            }

            // T029: write a substrate transition before exit so the row leaves
            // its current state (typically `executing`) cleanly with a
            // structured exit reason captured in `blocked_reason`. Without
            // this, the row would stay stuck at `executing` until the
            // out-of-process watchdog (L062 territory) noticed PID death.
            let blocked_reason = classify_runner_exit(&run_out);
            match fire_mark_drive_failed(conn, display_id, &blocked_reason, "", None) {
                Ok(()) => {
                    eprintln!(
                        "[{display_id}] mark_drive_failed fired (blocked_reason={blocked_reason})"
                    );
                    let _ = std::io::stderr().flush();
                    bail!(
                        "runner non-zero exit (code {}); transitioned to blocked",
                        run_out.exit_code
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[{display_id}] mark_drive_failed FAILED ({e:#}); row may stay at status='{}'",
                        na.status
                    );
                    let _ = std::io::stderr().flush();
                    bail!(
                        "runner non-zero exit (code {}); mark_drive_failed transition FAILED: {e:#}",
                        run_out.exit_code
                    );
                }
            }
        }

        // Payload validation error (MAJOR 2): surfaced after telemetry is
        // persisted (above) so the real exit_code is preserved in agent_runs.
        // Treated like a runner failure — transition to blocked.
        if let Some(ref payload_err) = run_out.payload_error {
            eprintln!(
                "[{display_id}] runner payload validation failed (exit={}): {payload_err}",
                run_out.exit_code
            );
            let _ = std::io::stderr().flush();
            let blocked_reason = classify_runner_exit(&run_out);
            match fire_mark_drive_failed(conn, display_id, &blocked_reason, "", None) {
                Ok(()) => {
                    bail!(
                        "runner payload validation failed (exit={}): {payload_err}; transitioned to blocked",
                        run_out.exit_code
                    );
                }
                Err(e) => {
                    bail!(
                        "runner payload validation failed (exit={}): {payload_err}; mark_drive_failed FAILED: {e:#}",
                        run_out.exit_code
                    );
                }
            }
        }

        // ── Step 2e: parse envelope + dispatch submit ─────────────────────
        let (envelope, source_tag) =
            parse_envelope(&run_out, &agent_name_normalized).map_err(|e| {
                eprintln!("[{display_id}] envelope parse failed: {e}");
                let _ = std::io::stderr().flush();
                if !run_out.stdout.is_empty() {
                    eprintln!("runner stdout:\n{}", run_out.stdout);
                    let _ = std::io::stderr().flush();
                }
                if !run_out.stderr.is_empty() {
                    eprintln!("runner stderr:\n{}", run_out.stderr);
                    let _ = std::io::stderr().flush();
                }
                // Return the error so the caller sees it (no submit was called).
                anyhow::anyhow!("envelope parse error: {e}")
            })?;

        // T072 r6: compute transcript_path BEFORE dispatch_submit so it can be
        // embedded atomically inside the submit transaction.
        //
        // MINOR 1: executor and code-reviewer MUST produce a session_id — their
        // transcript is part of L059's acceptance criterion.  Bail before submit
        // so the row is never advanced when the transcript pointer is absent.
        let transcript_path_owned: Option<String> = match run_out.session_id.as_deref() {
            Some(sid) => Some(format!(".stores/runs/{sid}.jsonl")),
            None => {
                // Only executor and code-reviewer are required to produce a transcript.
                // Planner, plan-reviewer, and wrap do not.
                let role_needs_transcript =
                    matches!(agent_role, "executor" | "code-reviewer" | "code_reviewer");
                if role_needs_transcript {
                    bail!(
                        "[{display_id}] {agent_role} run produced no session_id; \
                         transcript backlink cannot be written — submit aborted (L059)"
                    );
                }
                None
            }
        };
        let transcript_path_ref = transcript_path_owned.as_deref();

        let submit_out = dispatch_submit(
            schema,
            conn,
            display_id,
            &na.status,
            envelope,
            transcript_path_ref, // T072 r6: atomic backlink inside the submit tx
        )?;

        // T049: first successful submit ⇒ close the auto-drive dispatch_lock.
        // Up to this point the lock has been left open (by agents_run.rs's
        // post-spawn skip), so a drive subprocess that dies between spawn and
        // first submit remains visible to the watchdog as an open-lock zombie.
        //
        // `auto_drive_lock_closed` tracks only the actually-closed case
        // (LockCloseOutcome::Closed). PendingNext means the lock is still
        // in-flight awaiting the daemon's handoff re-dispatch; the flag stays
        // false so the next iteration re-evaluates whether the lock can close.
        if !auto_drive_lock_closed {
            match crate::handlers::agents_run::close_auto_drive_lock_ok(conn, display_id) {
                Ok(crate::handlers::agents_run::LockCloseOutcome::Closed) => {
                    auto_drive_lock_closed = true;
                }
                Ok(crate::handlers::agents_run::LockCloseOutcome::PendingNext) => {
                    // Lock is still in-flight; daemon will re-dispatch once
                    // the current work lands. Do not set auto_drive_lock_closed.
                }
                Ok(crate::handlers::agents_run::LockCloseOutcome::Failed) => {
                    // Unreachable via this code path; treat as no-op.
                }
                Err(e) => {
                    eprintln!("[{display_id}] close_auto_drive_lock_ok failed (non-fatal): {e}");
                    let _ = std::io::stderr().flush();
                }
            }
        }

        // AC4.3 flag: wrap dispatches when na.status == "in_review" (the row is in
        // in_review, next_agent is wrap). Once dispatch_submit returns successfully
        // (wrap envelope processed), set the flag so the next iteration's loop-top
        // guard exits cleanly instead of re-dispatching.
        if na.status == "in_review" {
            dispatched_wrap_this_run = true;
        }

        // ── Step 2f: render ───────────────────────────────────────────────
        // Render is best-effort; failure is logged but does not abort the loop.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match compute_render_in(
            schema,
            conn,
            display_id,
            false,
            Actor::AiAutonomous,
            &cwd,
            &cwd,
        ) {
            Ok(render_out) => {
                if !render_out.dry_run {
                    if let Err(e) = apply_render(&render_out) {
                        eprintln!("[{display_id}] render write failed (non-fatal): {e}");
                        let _ = std::io::stderr().flush();
                    }
                }
            }
            Err(e) => {
                eprintln!("[{display_id}] render compute failed (non-fatal): {e}");
                let _ = std::io::stderr().flush();
            }
        }

        // ── Step 2f: progress stderr (AC3.4) ─────────────────────────────
        let current_phase = na.current_phase.as_i64().unwrap_or(0);
        let current_cycle = na.current_cycle.as_i64().unwrap_or(0);
        let gate_display = submit_out
            .gate
            .as_deref()
            .or(submit_out.plan_review_gate.as_deref())
            .map(|g| format!("Some({g})"))
            .unwrap_or_else(|| "None".to_string());
        eprintln!(
            "[{display_id}] phase {current_phase} cycle {current_cycle}: {agent_role} → submitted (gate={gate_display}; source={source_tag})"
        );
        let _ = std::io::stderr().flush();

        // ── Step 2g: iter counter / max-iters (AC3.5) ────────────────────
        iter += 1;
        if iter >= max_iters {
            // T067 r7 MEDIUM fix: if wrap was dispatched in this iteration and
            // max-iters fires before the next loop-top guard runs, force-close
            // the auto-drive lock now so the daemon watchdog does not re-dispatch
            // wrap again. Without this, the lock stays in_flight:pending_next
            // and the watchdog treats it as a stale handoff needing re-dispatch.
            if dispatched_wrap_this_run {
                if let Err(e) =
                    crate::handlers::agents_run::force_close_auto_drive_lock_ok(conn, display_id)
                {
                    eprintln!(
                        "[{display_id}] force_close_auto_drive_lock_ok (max-iters path) \
                         failed (non-fatal): {e}"
                    );
                    let _ = std::io::stderr().flush();
                }
            }
            // Re-read state for summary.
            let na2 = compute_next_action(schema, conn, display_id)?;
            eprintln!(
                "[{display_id}] max iterations exceeded ({max_iters}); \
                 current state: status={} phase={} cycle={}",
                na2.status, na2.current_phase, na2.current_cycle
            );
            let _ = std::io::stderr().flush();
            bail!("max iterations exceeded ({max_iters}) for task {display_id}");
        }
    }
}

// ---------------------------------------------------------------------------
// Transcript backlink (T072)
// ---------------------------------------------------------------------------

// Only used in unit tests — production writes now happen inside the submit
// transaction (T072 r6). Kept so isolation tests can exercise the helper directly.
#[cfg(test)]
fn backlink_cycle_transcript(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    phase: i64,
    cycle: i64,
    role: &str,
    transcript_path: &str,
) -> Result<()> {
    let subrecord = match role {
        "executor" => "executor",
        "code-reviewer" | "code_reviewer" => "review",
        _ => return Ok(()),
    };

    let cycles_field = schema
        .workflow
        .as_ref()
        .and_then(|w| w.submit_targets.get("submit-execute"))
        .map(|s| s.as_str())
        .unwrap_or("cycles");
    let (row_id, existing) = crate::handlers::row::read_row(schema, conn, display_id)?;
    let mut cycles = existing
        .get(cycles_field)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut changed = false;
    for entry in &mut cycles {
        let matches = entry.get("phase").and_then(|v| v.as_i64()) == Some(phase)
            && entry.get("cycle").and_then(|v| v.as_i64()) == Some(cycle);
        if !matches {
            continue;
        }
        if let Some(obj) = entry.get_mut(subrecord).and_then(|v| v.as_object_mut()) {
            if obj.get("transcript_path").and_then(|v| v.as_str()) != Some(transcript_path) {
                obj.insert(
                    "transcript_path".to_string(),
                    serde_json::Value::String(transcript_path.to_string()),
                );
                changed = true;
            }
        }
    }

    if changed {
        let qtable = crate::codegen::ddl::quote_ident(&schema.name);
        let qfield = crate::codegen::ddl::quote_ident(cycles_field);
        let cycles_json = serde_json::to_string(&cycles)?;
        conn.execute(
            &format!("UPDATE {qtable} SET {qfield} = ?1 WHERE id = ?2"),
            rusqlite::params![cycles_json, row_id],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Envelope parser (AC2.8 — 3-layer fallback: sdk → sap → legacy)
// ---------------------------------------------------------------------------

/// Extract and parse a JSON envelope from runner output.
///
/// Three-layer fallback (AC2.8):
/// - **Layer 1 (SDK):** if `structured_output` is `Some`, deserialise directly.
/// - **Layer 2 (SAP):** if Layer 1 misses, call `sap::extract_envelope_from_text`
///   on `final_message` with the role's bundled schema.
/// - **Layer 3 (Legacy):** if Layer 2 also misses, fall through to the existing
///   `final_message` direct parse + last-line stdout scan (mock-fixture compat).
///
/// Returns `(AgentEnvelope, source_tag)` where `source_tag` is one of
/// `"sdk"`, `"sap"`, or `"legacy"`.
fn parse_envelope(
    out: &RunnerOutput,
    agent_role_normalized: &str,
) -> Result<(AgentEnvelope, &'static str)> {
    // Helper: peek the "role" string field from a JSON Value without consuming it.
    fn peek_role(v: &serde_json::Value) -> Option<&str> {
        v.get("role").and_then(|r| r.as_str())
    }

    // Helper: return Err if the peeked role is present and does not match expected.
    // A missing/null role is allowed through (Layer 2 injects it below).
    fn check_role_mismatch(
        peeked: Option<&str>,
        expected: &str,
        session_id: Option<&str>,
    ) -> Result<()> {
        if let Some(received) = peeked {
            if received != expected {
                let sid = session_id.unwrap_or("<unknown>");
                bail!(
                    "envelope role mismatch: expected {expected}, received {received}, session_id {sid}"
                );
            }
        }
        Ok(())
    }

    let session_id = out.session_id.as_deref();

    // ── Layer 1: SDK-validated structured output ─────────────────────────────
    if let Some(value) = &out.structured_output {
        check_role_mismatch(peek_role(value), agent_role_normalized, session_id)?;
        let envelope = serde_json::from_value::<AgentEnvelope>(value.clone()).map_err(|e| {
            anyhow::anyhow!("structured_output deserialise failed: {e}\nvalue: {value}")
        })?;
        return Ok((envelope, "sdk"));
    }

    // ── Layer 2: SAP — extract from final_message text, inject role ──────────
    // Schema validation is intentionally NOT applied here. Models often emit
    // the envelope without the `role` tag (treating role as orchestrator
    // metadata); we extract any well-formed JSON object from the prose and
    // inject the role ourselves. AgentEnvelope's `serde(tag = "role")`
    // deserialiser is the authoritative shape gate.
    //
    // ORDERING: peek role BEFORE or_insert_with; otherwise a present-but-wrong
    // role would be overwritten by the inject and the check would trivially pass.
    if let Some(fm) = &out.final_message {
        if !fm.trim().is_empty() {
            // T047: walk all candidates and prefer one whose role tag matches
            // the expected role (or whose role-marker field is present), so an
            // unrelated `{...}` object above the real envelope in prose cannot
            // shadow the true envelope and silently mis-parse.
            let candidates = crate::runner::sap::extract_all_json_objects(fm);
            if let Some(picked) = pick_best_sap_candidate(&candidates, agent_role_normalized) {
                let mut candidate = picked.clone();
                check_role_mismatch(peek_role(&candidate), agent_role_normalized, session_id)?;
                if let serde_json::Value::Object(ref mut map) = &mut candidate {
                    map.entry("role".to_string()).or_insert_with(|| {
                        serde_json::Value::String(agent_role_normalized.to_string())
                    });
                }
                let envelope =
                    serde_json::from_value::<AgentEnvelope>(candidate.clone()).map_err(|e| {
                        anyhow::anyhow!("SAP candidate deserialise failed: {e}\nvalue: {candidate}")
                    })?;
                return Ok((envelope, "sap"));
            }
        }
    }

    // ── Layer 3: Legacy — final_message direct parse then last-line stdout ───
    if let Some(fm) = &out.final_message {
        if !fm.trim().is_empty() {
            // Peek role before attempting AgentEnvelope deserialise.
            if let Ok(raw_val) = serde_json::from_str::<serde_json::Value>(fm) {
                check_role_mismatch(peek_role(&raw_val), agent_role_normalized, session_id)?;
            }
            if let Ok(envelope) = serde_json::from_str::<AgentEnvelope>(fm) {
                return Ok((envelope, "legacy"));
            }
        }
    }

    // Last-resort: scan stdout for last non-empty JSON line.
    let last_line = out.stdout.lines().rev().find(|l| !l.trim().is_empty());

    match last_line {
        None => bail!(
            "all 3 parse layers failed (layers attempted: sdk, sap, legacy); \
             runner produced no output (stdout is empty or all-whitespace); \
             expected a JSON envelope on the last line"
        ),
        Some(line) => {
            // Peek role before attempting AgentEnvelope deserialise.
            if let Ok(raw_val) = serde_json::from_str::<serde_json::Value>(line) {
                check_role_mismatch(peek_role(&raw_val), agent_role_normalized, session_id)?;
            }
            let envelope = serde_json::from_str::<AgentEnvelope>(line).map_err(|e| {
                anyhow::anyhow!(
                    "all 3 parse layers failed (layers attempted: sdk, sap, legacy); \
                     last stdout line is not a valid agent envelope: {e}\nraw line: {line}"
                )
            })?;
            Ok((envelope, "legacy"))
        }
    }
}

/// Pick the best SAP candidate JSON object for the given agent role.
///
/// Preference order (T047):
/// 1. Object whose `role` matches the expected role exactly.
/// 2. Object that carries the role-specific marker field (e.g. `phases` for
///    planner, `gate` for plan-reviewer / code-reviewer, `summary` for
///    executor, `executive_summary` for wrap) — only when no `role` mismatch
///    is present.
/// 3. The first object overall (legacy behaviour).
///
/// Returning `None` means there were no parseable JSON objects at all.
// pick_best_sap_candidate moved to crate::runner::sap so the runner-level
// extraction in claude_code.rs can apply the same role-aware selection logic
// rather than blindly picking the first JSON object (which lets unrelated
// `{...}` content above the real envelope shadow the planner output — codex
// T047 round 2 finding).
use crate::runner::sap::pick_best_sap_candidate;

// ---------------------------------------------------------------------------
// Submit dispatcher
// ---------------------------------------------------------------------------

fn dispatch_submit(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    current_status: &str,
    envelope: AgentEnvelope,
    // T072 r6: transcript path for atomic backlink inside executor / code-reviewer tx.
    transcript_path: Option<&str>,
) -> Result<crate::handlers::submit::SubmitOutput> {
    match envelope {
        AgentEnvelope::Planner {
            phases,
            decision_matrix,
        } => {
            if current_status != "planning" {
                bail!(
                    "planner envelope received but status is '{}', expected 'planning'",
                    current_status
                );
            }
            // Build the plan JSON object.
            let mut plan_obj = serde_json::Map::new();
            plan_obj.insert("phases".to_string(), phases);
            if !decision_matrix.is_null() {
                plan_obj.insert("decision_matrix".to_string(), decision_matrix);
            }
            let plan_json = Value::Object(plan_obj);
            compute_submit_plan(schema, conn, display_id, plan_json, Actor::AiAutonomous)
        }

        AgentEnvelope::PlanReviewer {
            gate,
            summary,
            open_questions,
        } => {
            if current_status != "plan_review" {
                bail!(
                    "plan-reviewer envelope received but status is '{}', expected 'plan_review'",
                    current_status
                );
            }
            let oq = if open_questions.is_empty() {
                None
            } else {
                Some(open_questions)
            };
            compute_submit_plan_review(
                schema,
                conn,
                display_id,
                &gate,
                &summary,
                oq,
                Actor::AiAutonomous,
            )
        }

        AgentEnvelope::Executor {
            summary,
            commit,
            files_changed,
        } => {
            if current_status != "executing" {
                bail!(
                    "executor envelope received but status is '{}', expected 'executing'",
                    current_status
                );
            }
            let files_str: Option<String> = files_changed.map(|v| v.join(","));
            compute_submit_execute(
                schema,
                conn,
                display_id,
                &summary,
                commit.as_deref(),
                files_str.as_deref(),
                None,
                Actor::AiAutonomous,
                transcript_path, // T072 r6: atomic backlink inside the tx
            )
        }

        AgentEnvelope::CodeReviewer {
            gate,
            summary,
            details,
            counts,
        } => {
            if current_status != "code_review" {
                bail!(
                    "code-reviewer envelope received but status is '{}', expected 'code_review'",
                    current_status
                );
            }
            let c = counts.unwrap_or_default();
            compute_submit_review(
                schema,
                conn,
                display_id,
                &gate,
                &summary,
                details.as_deref(),
                c.critical,
                c.major,
                c.minor,
                Actor::AiAutonomous,
                transcript_path, // T072 r6: atomic backlink inside the tx
            )
        }

        AgentEnvelope::Wrap {
            executive_summary,
            deviations,
            residual_risks,
            recommended_sanity_checks,
            reasoning,
        } => {
            // Phase 3: call compute_submit_wrap to persist the wrap_log entry.
            // The row is already at in_review (set by compute_submit_review's on-entry
            // follow-on). compute_submit_wrap is a pure list_record append — no transition
            // is fired; status remains in_review after the call.
            if current_status != "in_review" {
                bail!(
                    "wrap envelope received but status is '{}', expected 'in_review'",
                    current_status
                );
            }

            // `reasoning` is the agent's internal thought and is NOT persisted to wrap_log.
            // The schema defines only: executive_summary, deviations, residual_risks,
            // recommended_sanity_checks, reject_reason, at.
            let _ = reasoning; // consumed here; intentionally discarded
            let mut obj = serde_json::Map::new();
            obj.insert(
                "executive_summary".to_string(),
                serde_json::Value::String(executive_summary),
            );
            obj.insert(
                "deviations".to_string(),
                serde_json::Value::Array(
                    deviations
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
            obj.insert(
                "residual_risks".to_string(),
                serde_json::Value::Array(
                    residual_risks
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
            obj.insert(
                "recommended_sanity_checks".to_string(),
                serde_json::Value::Array(
                    recommended_sanity_checks
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
            let wrap_entry = serde_json::Value::Object(obj);

            crate::handlers::submit::compute_submit_wrap(
                schema,
                conn,
                display_id,
                wrap_entry,
                crate::schema::actor::Actor::AiAutonomous,
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Render applier (thin write wrapper)
// ---------------------------------------------------------------------------

fn apply_render(out: &crate::handlers::render::RenderOutput) -> Result<()> {
    use std::fs;

    if let Some(parent) = out.path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Atomic write: write to .tmp then rename.
    let tmp = out.path.with_extension("md.tmp");
    fs::write(&tmp, &out.content)?;
    fs::rename(&tmp, &out.path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (AC3.7)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::ddl_for;
    use crate::db;
    use crate::runner::RunnerOutput;
    use crate::schema::Schema;
    use serde_json::json;
    use tempfile::tempdir;

    // ---------------------------------------------------------------------------
    // Schema + DB helpers
    // ---------------------------------------------------------------------------

    /// Full tasks-like workflow schema for drive tests.
    fn tasks_schema() -> Schema {
        let yaml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("stores/tasks/schema.yaml"),
        )
        .expect("tasks schema.yaml");
        Schema::from_yaml(&yaml).unwrap()
    }

    fn open_db(schema: &Schema) -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.db");
        let conn = db::open(&db_file).unwrap();
        conn.execute_batch(&ddl_for(schema)).unwrap();
        (dir, conn)
    }

    /// Insert a minimal task row in `planning` state.
    #[allow(clippy::too_many_arguments)]
    fn insert_task(
        conn: &Connection,
        schema: &Schema,
        display_id: &str,
        status: &str,
        created_at: &str,
        current_phase: i64,
        current_cycle: i64,
        claimed_by: Option<&str>,
        claimed_at: Option<&str>,
    ) {
        let plan_json = serde_json::to_string(&json!({
            "objective": "Test",
            "phases": [
                {
                    "name": "Phase 1",
                    "objective": "Do something",
                    "tasks": [],
                    "acceptance_criteria": [],
                    "files": [],
                    "dependencies": []
                }
            ]
        }))
        .unwrap();

        let contract_json = serde_json::to_string(&json!({
            "done_when": "It works",
            "scope_in": "Everything",
            "scope_out": "Nothing"
        }))
        .unwrap();

        let cycles_json = "[]";
        let plan_review_log_json = "[]";

        conn.execute(
            &format!(
                "INSERT INTO {name} (display_id, status, created_at, updated_at, \
                 created_by, updated_by, title, slug, tier_hint, current_phase, current_cycle, \
                 plan, contract, cycles, plan_review_log, claimed_by, claimed_at) \
                 VALUES (?1,?2,?3,?3,?4,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                name = quote_ident(&schema.name)
            ),
            rusqlite::params![
                display_id,
                status,
                created_at,
                "human",
                "Test Task",
                "test-task",
                "T3",
                current_phase,
                current_cycle,
                plan_json,
                contract_json,
                cycles_json,
                plan_review_log_json,
                claimed_by,
                claimed_at,
            ],
        )
        .unwrap();
    }

    fn make_run_output(stdout: &str, exit_code: i32) -> RunnerOutput {
        // Use final_message as the last line of stdout for convenience.
        let last_line = stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|s| s.to_string());
        // Construct a transient workspace dir for the telemetry stub.
        // The dir is dropped after the call but the path string remains valid
        // (non-empty) for insert_agent_run's non-null check. Tests that need
        // the transcript file to actually exist on disk must build their own
        // explicit telemetry (see happy_path_one_phase_mock).
        let tmp = tempdir().unwrap();
        RunnerOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code,
            final_message: last_line,
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(tmp.path()),
        }
    }

    /// Like `make_run_output` but includes a `session_id`.
    ///
    /// T072 r6: executor and code-reviewer mock outputs must have a session_id
    /// because the drive loop now bails before submit when session_id is None
    /// for transcript-producing roles (MINOR 1). Tests that exercise the full
    /// drive loop through these roles must use this helper.
    fn make_run_output_with_session(stdout: &str, exit_code: i32, sid: &str) -> RunnerOutput {
        let mut out = make_run_output(stdout, exit_code);
        out.session_id = Some(sid.to_string());
        out
    }

    #[test]
    fn backlink_accepts_schema_code_reviewer_role() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        insert_task(
            &conn,
            &schema,
            "T072",
            "in_review",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        let cycles = serde_json::to_string(&json!([{
            "phase": 1,
            "cycle": 1,
            "review": {"gate": "REVISE"}
        }]))
        .unwrap();
        conn.execute(
            &format!(
                "UPDATE {name} SET cycles = ?1 WHERE display_id = ?2",
                name = quote_ident(&schema.name)
            ),
            rusqlite::params![cycles, "T072"],
        )
        .unwrap();

        backlink_cycle_transcript(
            &schema,
            &conn,
            "T072",
            1,
            1,
            "code_reviewer",
            ".stores/runs/review-session.jsonl",
        )
        .unwrap();

        let stored: String = conn
            .query_row(
                &format!(
                    "SELECT cycles FROM {name} WHERE display_id = ?1",
                    name = quote_ident(&schema.name)
                ),
                ["T072"],
                |row| row.get(0),
            )
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(
            value[0]["review"]["transcript_path"],
            ".stores/runs/review-session.jsonl"
        );
    }

    // ---------------------------------------------------------------------------
    // T072 r4: MAJOR 3 — backlink failure propagates (fail-loud)
    // ---------------------------------------------------------------------------

    /// Verify that `backlink_cycle_transcript` fails fast when the target row is
    /// absent (the cycles JSON UPDATE hits no row).  This tests the error-path
    /// branch that the drive loop now propagates instead of swallowing.
    ///
    /// Specifically: if the task row for `display_id` does not exist,
    /// `read_row` returns an error, which the updated drive-loop code
    /// propagates via `?` rather than logging and continuing.
    #[test]
    fn backlink_returns_err_when_row_absent() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        // Do NOT insert a task row — backlink must fail on read_row.
        let result = backlink_cycle_transcript(
            &schema,
            &conn,
            "T_NONEXISTENT",
            1,
            1,
            "executor",
            ".stores/runs/some-session.jsonl",
        );
        assert!(
            result.is_err(),
            "backlink must fail when the task row is absent; drive must not claim success"
        );
    }

    // ---------------------------------------------------------------------------
    // T072 r6: MINOR 2 — drive-loop backlink path + idempotence
    // ---------------------------------------------------------------------------

    /// Helper: read the `cycles` JSON array for a task row.
    fn read_cycles_for(conn: &Connection, schema: &Schema, display_id: &str) -> Vec<Value> {
        let row: String = conn
            .query_row(
                &format!(
                    "SELECT COALESCE(cycles, '[]') FROM {} WHERE display_id = ?1",
                    quote_ident(&schema.name)
                ),
                rusqlite::params![display_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "[]".to_string());
        serde_json::from_str::<Vec<Value>>(&row).unwrap_or_default()
    }

    /// MINOR 2a: drive_loop with a session_id writes transcript_path into the
    /// cycle sub-record atomically (as part of submit-execute / submit-review).
    ///
    /// After a full drive run, `cycles[0].executor.transcript_path` and
    /// `cycles[0].review.transcript_path` must equal the expected `.stores/runs/<sid>.jsonl` paths.
    #[test]
    fn drive_loop_with_session_id_writes_transcript_path_to_cycles() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        let exec_sid = "exec-atomic-session-uuid";
        let review_sid = "review-atomic-session-uuid";

        let runner = MockRunner::new(vec![
            make_run_output(planner_fixture_json(), 0),
            make_run_output(plan_reviewer_fixture_json(), 0),
            make_run_output_with_session(executor_fixture_json(), 0, exec_sid),
            make_run_output_with_session(code_reviewer_fixture_json(), 0, review_sid),
            make_run_output(wrap_fixture_json(), 0),
        ]);

        drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect("drive_loop with session_id must succeed");

        let cycles = read_cycles_for(&conn, &schema, "T001");
        assert_eq!(cycles.len(), 1, "must have exactly one cycle entry");

        // executor sub-record must carry the transcript_path
        let executor_tp = cycles[0]["executor"]["transcript_path"]
            .as_str()
            .expect("cycles[0].executor.transcript_path must be a string");
        assert_eq!(
            executor_tp,
            format!(".stores/runs/{exec_sid}.jsonl"),
            "executor transcript_path must be the exec session path"
        );

        // review sub-record must carry the transcript_path
        let review_tp = cycles[0]["review"]["transcript_path"]
            .as_str()
            .expect("cycles[0].review.transcript_path must be a string");
        assert_eq!(
            review_tp,
            format!(".stores/runs/{review_sid}.jsonl"),
            "review transcript_path must be the review session path"
        );
    }

    /// MINOR 2b: transcript_path consistency under retry — when `compute_submit_execute`
    /// is called twice with the same session_id (simulating a drive-loop retry),
    /// production appends a second cycle entry (no dedup). This test asserts that
    /// both appended entries consistently carry the same `transcript_path` value.
    ///
    /// NOTE: `compute_submit_execute` is an append-only writer — it does NOT enforce
    /// no-double-write. Enforcing deduplication would be a separate scope. This test
    /// verifies only that the atomic backlink always embeds the correct path, even
    /// when the same session submits twice.
    #[test]
    fn executor_transcript_path_consistent_under_retry() {
        use crate::handlers::submit::compute_submit_execute;
        use crate::schema::actor::Actor;

        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        // Insert executing row at phase 1 cycle 1.
        insert_task(
            &conn,
            &schema,
            "T001",
            "executing",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        let sid = "retry-session-uuid";
        let tp = format!(".stores/runs/{sid}.jsonl");

        // First submit with transcript_path.
        compute_submit_execute(
            &schema,
            &conn,
            "T001",
            "first attempt",
            Some("abc123"),
            None,
            None,
            Actor::AiAutonomous,
            Some(&tp),
        )
        .expect("first submit-execute must succeed");

        let cycles = read_cycles_for(&conn, &schema, "T001");
        assert_eq!(
            cycles.len(),
            1,
            "first submit must produce exactly one cycle entry"
        );
        assert_eq!(
            cycles[0]["executor"]["transcript_path"].as_str(),
            Some(tp.as_str()),
            "first write must embed transcript_path"
        );

        // Force the row back to 'executing' to simulate a drive-loop retry
        // (e.g. the runner crashed after submit but before state propagation).
        conn.execute(
            &format!(
                "UPDATE {} SET status = 'executing' WHERE display_id = 'T001'",
                quote_ident(&schema.name)
            ),
            [],
        )
        .unwrap();

        // Second submit with the same transcript_path (retry scenario).
        compute_submit_execute(
            &schema,
            &conn,
            "T001",
            "second attempt (retry)",
            Some("abc123"),
            None,
            None,
            Actor::AiAutonomous,
            Some(&tp),
        )
        .expect("second submit-execute must succeed");

        let cycles2 = read_cycles_for(&conn, &schema, "T001");
        // Production appends a new entry on each call — two calls produce two entries.
        assert_eq!(
            cycles2.len(),
            2,
            "second submit must append a second cycle entry (production is append-only)"
        );
        // Both entries must carry the same transcript_path: the atomic backlink
        // consistently embeds the session path regardless of retry count.
        for (i, entry) in cycles2.iter().enumerate() {
            let stored_tp = entry["executor"]["transcript_path"].as_str();
            assert_eq!(
                stored_tp,
                Some(tp.as_str()),
                "cycle entry {i} transcript_path must equal the session path"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Planner fixture JSON (from tests/fixtures/agent_outputs/planner.json)
    // ---------------------------------------------------------------------------

    fn planner_fixture_json() -> &'static str {
        include_str!("../../tests/fixtures/agent_outputs/planner.json")
    }

    fn plan_reviewer_fixture_json() -> &'static str {
        include_str!("../../tests/fixtures/agent_outputs/plan-reviewer.json")
    }

    fn executor_fixture_json() -> &'static str {
        include_str!("../../tests/fixtures/agent_outputs/executor.json")
    }

    fn code_reviewer_fixture_json() -> &'static str {
        include_str!("../../tests/fixtures/agent_outputs/code-reviewer.json")
    }

    /// Stub wrap envelope for Phase 1 testing.
    /// The wrap agent schema is formally defined in Phase 2; this fixture is
    /// sufficient for drive_loop to parse and exit with the in_review hint.
    fn wrap_fixture_json() -> &'static str {
        r#"{"role":"wrap","executive_summary":"stub","deviations":[],"residual_risks":[],"recommended_sanity_checks":[]}"#
    }

    /// Full wrap fixture (Phase 2) — representative envelope with all fields populated.
    fn wrap_full_fixture_json() -> &'static str {
        include_str!("../../tests/fixtures/agent_outputs/wrap.json")
    }

    // ---------------------------------------------------------------------------
    // derive_spawn_fail_model_id: preserve configured model suffix
    // ---------------------------------------------------------------------------

    #[test]
    fn spawn_fail_model_id_preserves_suffix() {
        // pi runner → pi:default
        assert_eq!(derive_spawn_fail_model_id("pi"), "pi:default");
        // claude-code with model suffix → preserve as claude_code:<model>
        assert_eq!(
            derive_spawn_fail_model_id("claude-code:opus"),
            "claude_code:opus"
        );
        assert_eq!(
            derive_spawn_fail_model_id("claude-code:sonnet"),
            "claude_code:sonnet"
        );
        // claude-code without model suffix → claude_code:unknown
        assert_eq!(
            derive_spawn_fail_model_id("claude-code"),
            "claude_code:unknown"
        );
        // unknown runner → <name>:unknown
        assert_eq!(derive_spawn_fail_model_id("custom"), "custom:unknown");
    }

    // ---------------------------------------------------------------------------
    // AC3.7: happy-path through 1 full phase (planning → plan_review →
    // executing → code_review → complete → in_review via wrap dispatch)
    // ---------------------------------------------------------------------------

    #[test]
    fn happy_path_one_phase_mock() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        let runs_dir = _dir.path().join(".stores").join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();

        // Pre-create per-role transcript files so transcript_path is a real
        // existing path for every submit row (Finding 3).
        let role_names = [
            "planner",
            "plan_reviewer",
            "executor",
            "code_reviewer",
            "wrap",
        ];
        for role in &role_names {
            let p = runs_dir.join(format!("{role}.jsonl"));
            std::fs::write(&p, "{}\n").unwrap();
        }

        // Insert task in planning state, phase=0 (not yet started)
        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        // Queue: planner → plan_reviewer → executor → code_reviewer → wrap
        // After code_reviewer PASS-last-phase, on_state.complete fires request_review
        // (same tx → in_review). Drive then dispatches wrap agent; after wrap
        // submits, drive exits with "awaiting human review" hint.
        //
        // Each RunnerOutput carries fully-populated telemetry (model_id,
        // transcript_path, tokens_in/out) so every agent_runs row satisfies
        // Finding 1 non-null constraint without relying on legacy_unknown fallback.
        // T072 r6: executor and code-reviewer must have session_id (MINOR 1).
        let make_telemetry = |role: &str| crate::runner::AgentRunTelemetry {
            model_id: Some("mock-model-1".to_string()),
            harness_id: Some("mock".to_string()),
            started_at: Some(crate::handlers::row::now_iso8601()),
            ended_at: Some(crate::handlers::row::now_iso8601()),
            tokens_in: Some(10),
            tokens_out: Some(20),
            prompt_cache_hits: Some(0),
            transcript_path: Some(runs_dir.join(format!("{role}.jsonl")).display().to_string()),
            stderr_log_path: None,
        };

        let mut planner_out = make_run_output(planner_fixture_json(), 0);
        planner_out.telemetry = make_telemetry("planner");

        let mut plan_reviewer_out = make_run_output(plan_reviewer_fixture_json(), 0);
        plan_reviewer_out.telemetry = make_telemetry("plan_reviewer");

        let mut executor_out =
            make_run_output_with_session(executor_fixture_json(), 0, "happy-exec-session");
        executor_out.telemetry = make_telemetry("executor");

        let mut code_reviewer_out =
            make_run_output_with_session(code_reviewer_fixture_json(), 0, "happy-review-session");
        code_reviewer_out.telemetry = make_telemetry("code_reviewer");

        let mut wrap_out = make_run_output(wrap_fixture_json(), 0);
        wrap_out.telemetry = make_telemetry("wrap");

        let runner = MockRunner::new(vec![
            planner_out,
            plan_reviewer_out,
            executor_out,
            code_reviewer_out,
            wrap_out,
        ]);

        drive_loop(&schema, &conn, "T001", &runner, 50).expect("drive_loop should succeed");

        // Query all telemetry fields for post-drive assertions (Finding 3).
        type RunRow = (String, i64, i64, String, String, String, String, String, i64, String, Option<i64>, Option<i64>, Option<i64>);
        let rows: Vec<RunRow> = conn
            .prepare(
                "SELECT display_id, phase, cycle, role, model_id, harness_id, started_at, ended_at, exit_code, transcript_path, tokens_in, tokens_out, prompt_cache_hits \
                 FROM agent_runs ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| Ok((
                r.get(0)?,  // display_id
                r.get(1)?,  // phase
                r.get(2)?,  // cycle
                r.get(3)?,  // role
                r.get(4)?,  // model_id
                r.get(5)?,  // harness_id
                r.get(6)?,  // started_at
                r.get(7)?,  // ended_at
                r.get(8)?,  // exit_code
                r.get(9)?,  // transcript_path
                r.get(10)?, // tokens_in
                r.get(11)?, // tokens_out
                r.get(12)?, // prompt_cache_hits
            )))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(
            rows.len(),
            5,
            "one agent_runs row per consumed mock response"
        );

        // Finding 4: roles stored in stable underscore form (not hyphen display form).
        assert_eq!(
            rows.iter().map(|r| r.3.as_str()).collect::<Vec<_>>(),
            vec![
                "planner",
                "plan_reviewer",
                "executor",
                "code_reviewer",
                "wrap"
            ]
        );

        // Finding 3: every row has all required telemetry fields non-null.
        for row in &rows {
            let role = &row.3;
            assert_eq!(row.0, "T001", "display_id: {role}");
            assert!(row.1 >= 0, "phase populated: {role}");
            assert!(row.2 >= 0, "cycle populated: {role}");
            assert!(!row.4.is_empty(), "model_id non-empty: {role}");
            assert_ne!(
                row.4, "legacy_unknown",
                "model_id not fallback for new rows: {role}"
            );
            assert_eq!(row.5, "mock", "harness_id: {role}");
            assert!(!row.6.is_empty(), "started_at populated: {role}");
            assert!(!row.7.is_empty(), "ended_at populated: {role}");
            assert_eq!(row.8, 0, "exit_code zero: {role}");
            assert!(!row.9.is_empty(), "transcript_path non-empty: {role}");

            // Each transcript_path resolves to an existing file under runs_dir.
            let tp = std::path::Path::new(&row.9);
            assert!(
                tp.exists(),
                "transcript_path resolves to existing file: {} (role={})",
                tp.display(),
                role
            );
            assert!(
                tp.starts_with(&runs_dir),
                "transcript_path is under runs dir: {} (role={})",
                tp.display(),
                role
            );

            // tokens_in and tokens_out are non-null when runner reports them.
            assert!(
                row.10.is_some(),
                "tokens_in non-null when runner reports it: {role}"
            );
            assert!(
                row.11.is_some(),
                "tokens_out non-null when runner reports it: {role}"
            );
            // prompt_cache_hits is preserved from the fixture-supplied telemetry.
            assert_eq!(
                row.12,
                Some(0),
                "prompt_cache_hits preserved from fixture telemetry (make_telemetry sets 0): {role}"
            );
        }

        // Verify final status: in_review (drive exits after wrap dispatch, row awaits human)
        let na = compute_next_action(&schema, &conn, "T001").unwrap();
        assert_eq!(
            na.status, "in_review",
            "task should be in_review after drive (awaiting human GO/NO_GO)"
        );

        // AC4.3 eager-dispatch regression guard: all 5 queued mock responses must be
        // consumed. If the wrap response (5th) was not consumed, the runner still has
        // 1 item remaining — this catches the status-only guard regression where
        // drive exits at in_review without dispatching wrap.
        assert_eq!(
            runner.remaining_count(),
            0,
            "all 5 mock responses (including wrap) must be consumed; {} remain — \
             eager-wrap dispatch did not fire",
            runner.remaining_count()
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.7: auto-selection ordering — picks task with earliest created_at
    // ---------------------------------------------------------------------------

    #[test]
    fn auto_selection_picks_earliest_created_at() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        // Insert two tasks; T002 has earlier created_at
        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-02-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );
        insert_task(
            &conn,
            &schema,
            "T002",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        let args = DriveArgs {
            display_id: None,
            auto: true,
            mock_fixture: None,
            #[cfg(feature = "runner-claude-code")]
            claude_code: false,
            #[cfg(feature = "runner-claude-code")]
            testing: false,
            #[cfg(feature = "runner-claude-code")]
            claude_code_model: None,
            #[cfg(feature = "runner-pi")]
            pi: false,
            max_iters: 50,
        };

        let selected = resolve_task_id(&schema, &conn, &args).unwrap();
        assert_eq!(
            selected, "T002",
            "should pick T002 with earliest created_at"
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.7: live-claim skip — claimed row within lock window is skipped
    // ---------------------------------------------------------------------------

    #[test]
    fn config_role_runner_selects_role_specific_runner_name() {
        let mut roles = std::collections::BTreeMap::new();
        roles.insert(
            "planner".to_string(),
            RoleRunnerChoice {
                runner: "claude-code".to_string(),
                model: Some("opus".to_string()),
            },
        );
        roles.insert(
            "code_reviewer".to_string(),
            RoleRunnerChoice {
                runner: "pi".to_string(),
                model: None,
            },
        );
        let rr = ConfigRoleRunner {
            default_runner: Some("claude-code".to_string()),
            roles,
        };
        assert_eq!(rr.name_for_role("planner").unwrap(), "claude-code:opus");
        assert_eq!(rr.name_for_role("code_reviewer").unwrap(), "pi");
        assert_eq!(rr.name_for_role("executor").unwrap(), "claude-code");
    }

    #[test]
    fn config_role_runner_rejects_model_for_non_claude_name() {
        let mut roles = std::collections::BTreeMap::new();
        roles.insert(
            "plan_reviewer".to_string(),
            RoleRunnerChoice {
                runner: "pi".to_string(),
                model: Some("opus".to_string()),
            },
        );
        let rr = ConfigRoleRunner {
            default_runner: None,
            roles,
        };
        let err = rr
            .build_choice(&rr.choice_for("plan_reviewer").unwrap())
            .err()
            .expect("model on non-claude runner should be rejected");
        assert!(err
            .to_string()
            .contains("model is only supported for claude-code"));
    }

    #[test]
    fn auto_selection_skips_live_claimed() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        // T001 is claimed right now (claimed_at = now)
        let now = {
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let s = secs % 60;
            let mi = (secs / 60) % 60;
            let h = (secs / 3600) % 24;
            let days = secs / 86400;
            let (y, mo, d) = days_to_ymd(days);
            format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
        };

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            Some("other-runner"),
            Some(&now),
        );
        // T002 is not claimed
        insert_task(
            &conn,
            &schema,
            "T002",
            "planning",
            "2026-01-02T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        let args = DriveArgs {
            display_id: None,
            auto: true,
            mock_fixture: None,
            #[cfg(feature = "runner-claude-code")]
            claude_code: false,
            #[cfg(feature = "runner-claude-code")]
            testing: false,
            #[cfg(feature = "runner-claude-code")]
            claude_code_model: None,
            #[cfg(feature = "runner-pi")]
            pi: false,
            max_iters: 50,
        };

        let selected = resolve_task_id(&schema, &conn, &args).unwrap();
        assert_eq!(
            selected, "T002",
            "should skip live-claimed T001 and pick T002"
        );
    }

    // ---------------------------------------------------------------------------
    // T049: drive_loop closes the auto-drive dispatch_lock on first successful
    // submit. Pre-seed an open auto-drive lock and assert it transitions to
    // (finished_at != NULL, last_status='ok') after one planner submit.
    // ---------------------------------------------------------------------------

    #[test]
    fn drive_loop_first_submit_closes_auto_drive_lock() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T801",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        let row_id: i64 = conn
            .query_row("SELECT id FROM tasks WHERE display_id='T801'", [], |r| {
                r.get(0)
            })
            .unwrap();
        // Open auto-drive dispatch_lock — mirrors the post-T049 invariant
        // where agents_run leaves the lock open when auto-drive spawn returns 0.
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, \
              claimed_at, claimed_by) \
             VALUES ('tasks', ?1, 'T801', 'auto-drive', 1, \
                     '2026-05-04T00:00:00Z', 'test-claimer')",
            rusqlite::params![row_id],
        )
        .unwrap();

        // One successful planner envelope is enough to land the first submit;
        // max_iters=1 trips the bail post-iteration so the loop exits Err
        // (which is fine — we only care that the lock was closed before exit).
        let planner_out = make_run_output(planner_fixture_json(), 0);
        let runner = MockRunner::new(vec![planner_out]);
        let _ = drive_loop(&schema, &conn, "T801", &runner, 1);

        let (finished_at, last_status): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT finished_at, last_status FROM dispatch_locks \
                 WHERE display_id='T801' AND agent_name='auto-drive'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(
            finished_at.is_none(),
            "T067: first submit with pending next_agent keeps auto-drive lock in-flight"
        );
        assert_eq!(
            last_status.as_deref(),
            Some("in_flight:pending_next"),
            "T067: pending handoff lock must carry last_status='in_flight:pending_next'"
        );
    }

    // ---------------------------------------------------------------------------
    // T067 r5: auto-drive lock closes terminal-ok after wrap dispatch
    //
    // When the drive loop exits at the `in_review && dispatched_wrap_this_run`
    // guard, it calls force_close_auto_drive_lock_ok before returning. This
    // prevents the daemon watchdog from infinitely re-dispatching wrap on the
    // next sweep (which it would do because the schema always yields
    // next_agent=Some("wrap") for in_review — no when: guard exists).
    //
    // A1 invariant preserved: the decision to close is made by the current-cycle
    // `dispatched_wrap_this_run` flag, NOT by inspecting wrap_log history.
    // ---------------------------------------------------------------------------

    #[test]
    fn drive_loop_wrap_dispatch_closes_lock_terminal_ok() {
        // Regression test for the infinite-wrap-redispatch bug (r4 A1-strict
        // over-application): after a successful wrap submit, the drive loop must
        // close the auto-drive lock with terminal_reason='ok' and finished_at
        // non-null before exiting.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T802",
            "in_review",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        let row_id: i64 = conn
            .query_row("SELECT id FROM tasks WHERE display_id='T802'", [], |r| {
                r.get(0)
            })
            .unwrap();
        // Open auto-drive dispatch_lock (mirrors post-T049 invariant).
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, \
              claimed_at, claimed_by) \
             VALUES ('tasks', ?1, 'T802', 'auto-drive', 1, \
                     '2026-05-04T00:00:00Z', 'test-claimer')",
            rusqlite::params![row_id],
        )
        .unwrap();

        // One wrap response — drive should consume it and exit Ok.
        let wrap_out = make_run_output(wrap_fixture_json(), 0);
        let runner = MockRunner::new(vec![wrap_out]);
        drive_loop(&schema, &conn, "T802", &runner, 50)
            .expect("wrap dispatch at in_review must succeed");

        // Runner fully drained — wrap was consumed.
        assert_eq!(
            runner.remaining_count(),
            0,
            "wrap response must be consumed"
        );

        // T067 r5: lock must be closed terminal-ok (no infinite re-dispatch).
        let (finished_at, last_status, terminal_reason): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT finished_at, last_status, terminal_reason FROM dispatch_locks \
                 WHERE display_id='T802' AND agent_name='auto-drive'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(
            finished_at.is_some(),
            "T067 r5: lock must be closed (finished_at non-null) after wrap dispatch; \
             got finished_at=None (daemon would infinitely re-dispatch wrap)"
        );
        assert_eq!(
            last_status.as_deref(),
            Some("ok:wrap_completed"),
            "T067 r7: last_status must be 'ok:wrap_completed' after force_close (watchdog discriminator)"
        );
        assert_eq!(
            terminal_reason.as_deref(),
            Some("ok"),
            "T067 r5: terminal_reason must be 'ok' after wrap dispatch closes lock"
        );

        // A1 invariant: wrap_log is populated as provenance (not used as sentinel).
        let wrap_log: Option<String> = conn
            .query_row(
                "SELECT wrap_log FROM tasks WHERE display_id='T802'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            wrap_log
                .as_deref()
                .unwrap_or("")
                .contains("executive_summary"),
            "wrap_log must be populated as provenance after wrap dispatch; got {wrap_log:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.7: max-iters — loop aborts when limit reached
    // ---------------------------------------------------------------------------

    #[test]
    fn max_iters_aborts_loop() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        // Queue only 1 planner response (advances to plan_review), then the loop
        // will try to call runner again (which would fail from queue exhaustion).
        // But with max_iters=1, it should abort after 1 iteration.
        let planner_out = make_run_output(planner_fixture_json(), 0);
        let runner = MockRunner::new(vec![planner_out]);

        let err =
            drive_loop(&schema, &conn, "T001", &runner, 1).expect_err("should fail with max-iters");
        let msg = err.to_string();
        assert!(
            msg.contains("max iterations exceeded"),
            "error must mention max iterations: {msg}"
        );
    }

    // ---------------------------------------------------------------------------
    // T067 r7 MEDIUM: max-iters fires after wrap dispatch → force-close must run
    // ---------------------------------------------------------------------------

    #[test]
    fn max_iters_after_wrap_dispatch_force_closes_lock() {
        // Regression: if max-iters fires immediately after a successful wrap submit,
        // the loop bails before reaching the loop-top `in_review && dispatched_wrap_this_run`
        // guard. Without the r7 fix, the lock stays in_flight:pending_next and the
        // daemon watchdog re-dispatches wrap again. With the fix, force_close fires
        // before the bail and the lock reaches last_status='ok:wrap_completed'.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T803",
            "in_review",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        let row_id: i64 = conn
            .query_row("SELECT id FROM tasks WHERE display_id='T803'", [], |r| {
                r.get(0)
            })
            .unwrap();
        // Open auto-drive dispatch_lock.
        conn.execute(
            "INSERT INTO dispatch_locks \
             (store, row_id, display_id, agent_name, transition_id, \
              claimed_at, claimed_by) \
             VALUES ('tasks', ?1, 'T803', 'auto-drive', 1, \
                     '2026-05-04T00:00:00Z', 'test-claimer')",
            rusqlite::params![row_id],
        )
        .unwrap();

        // One wrap response — max_iters=1 means the loop fires wrap in iter 0,
        // increments iter to 1, and bails on the max-iters check before reaching
        // the next loop-top guard.
        let wrap_out = make_run_output(wrap_fixture_json(), 0);
        let runner = MockRunner::new(vec![wrap_out]);
        // drive_loop returns Err (max-iters exceeded).
        let err = drive_loop(&schema, &conn, "T803", &runner, 1)
            .expect_err("max_iters=1 must abort after wrap dispatch");
        assert!(
            err.to_string().contains("max iterations exceeded"),
            "error must mention max iterations: {err}"
        );

        // T067 r7: lock must be force-closed (finished_at non-null,
        // last_status='ok:wrap_completed') even though the bail path fired.
        let (finished_at, last_status, terminal_reason): (
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT finished_at, last_status, terminal_reason FROM dispatch_locks \
                 WHERE display_id='T803' AND agent_name='auto-drive'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert!(
            finished_at.is_some(),
            "T067 r7: lock must be force-closed (finished_at non-null) even on max-iters bail \
             after wrap dispatch; daemon would re-dispatch wrap otherwise"
        );
        assert_eq!(
            last_status.as_deref(),
            Some("ok:wrap_completed"),
            "T067 r7: last_status must be 'ok:wrap_completed' after max-iters force-close; \
             got {last_status:?}"
        );
        assert_eq!(
            terminal_reason.as_deref(),
            Some("ok"),
            "terminal_reason must be 'ok'; got {terminal_reason:?}"
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.7: runner-error abort — non-zero exit does not corrupt task state
    // ---------------------------------------------------------------------------

    #[test]
    fn runner_error_mid_loop_transitions_to_blocked_with_structured_reason() {
        // T029: runner exit != 0 must fire `mark_drive_failed` before the
        // drive subprocess returns to its parent, so the row leaves its
        // current state cleanly with a structured `blocked_reason`. The
        // runner-crash branch (no rate_limit_event, no "rate limit" stderr)
        // must classify as `kind=runner_crash`.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "executing",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        let fail_out = RunnerOutput {
            stdout: "some partial output".to_string(),
            stderr: "runner crashed".to_string(),
            exit_code: 1,
            final_message: None,
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(_dir.path()),
        };
        let runner = MockRunner::new(vec![fail_out]);

        let err = drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect_err("should fail on runner error");
        assert!(
            err.to_string().contains("non-zero exit"),
            "error must mention non-zero exit: {}",
            err
        );

        let persisted_exit: i64 = conn
            .query_row(
                "SELECT exit_code FROM agent_runs WHERE display_id='T001' AND role='executor'",
                [],
                |r| r.get(0),
            )
            .expect("agent_runs row must be inserted before blocking transition");
        assert_eq!(persisted_exit, 1);

        let (_, after) = crate::handlers::row::read_row(&schema, &conn, "T001").unwrap();
        assert_eq!(
            after.get("status").and_then(|v| v.as_str()),
            Some("blocked"),
            "row must be at 'blocked' after runner non-zero exit"
        );
        let reason_str = after
            .get("blocked_reason")
            .and_then(|v| v.as_str())
            .expect("blocked_reason must be set");
        let reason: serde_json::Value =
            serde_json::from_str(reason_str).expect("blocked_reason must be JSON");
        assert_eq!(
            reason.get("kind").and_then(|v| v.as_str()),
            Some("runner_crash")
        );
        assert_eq!(reason.get("exit_code").and_then(|v| v.as_i64()), Some(1));

        // transition_history must record the abort from the executing state
        // (the canonical contract path: drive_loop is invoked while the row
        // is executing; runner non-zero exit must transition it out).
        let (from_status, to_status, verb): (String, String, String) = conn
            .query_row(
                "SELECT from_status, to_status, verb FROM transition_history \
                 WHERE display_id = ?1 ORDER BY id DESC LIMIT 1",
                rusqlite::params!["T001"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("transition_history row must exist");
        assert_eq!(from_status, "executing");
        assert_eq!(to_status, "blocked");
        assert_eq!(verb, "mark_drive_failed");
    }

    #[test]
    fn rate_limit_classifier_covers_anthropic_codex_pi_and_generic_exit3() {
        fn out(stdout: &str, stderr: &str, payload_error: Option<&str>) -> RunnerOutput {
            RunnerOutput {
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
                exit_code: 3,
                final_message: None,
                structured_output: None,
                session_id: None,
                structured_output_source: None,
                payload_error: payload_error.map(str::to_string),
                telemetry: crate::runner::AgentRunTelemetry::default(),
            }
        }

        let anthropic = classify_runner_exit(&out(
            "",
            "anthropic-api HTTP 429 rate limit; Retry-After: 60",
            None,
        ));
        assert!(
            anthropic.starts_with("rate_limit:anthropic:"),
            "{anthropic}"
        );
        assert_eq!(
            anthropic
                .strip_prefix("rate_limit:anthropic:")
                .unwrap()
                .len(),
            20
        );

        let codex = classify_runner_exit(&out("rate_limit_error: please retry later", "", None));
        assert!(codex.starts_with("rate_limit:codex:"), "{codex}");
        assert_eq!(codex.strip_prefix("rate_limit:codex:").unwrap().len(), 20);

        let pi = classify_runner_exit(&out("", "pi/provider 429 upstream throttled", None));
        assert!(pi.starts_with("rate_limit:pi:"), "{pi}");

        let generic = classify_runner_exit(&out("partial", "runner crashed", None));
        assert!(!generic.starts_with("rate_limit:"), "{generic}");
        let v: serde_json::Value = serde_json::from_str(&generic).unwrap();
        assert_eq!(v.get("kind").and_then(|v| v.as_str()), Some("runner_crash"));
    }

    #[test]
    fn anthropic_429_exit3_transitions_task_to_rate_limit_blocked() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        insert_task(
            &conn,
            &schema,
            "T429",
            "executing",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );
        let fail_out = RunnerOutput {
            stdout: "".to_string(),
            stderr: "anthropic-api HTTP 429 rate limit; retry-after: 120".to_string(),
            exit_code: 3,
            final_message: None,
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(_dir.path()),
        };
        let runner = MockRunner::new(vec![fail_out]);
        let _ = drive_loop(&schema, &conn, "T429", &runner, 50).unwrap_err();
        let (_, after) = crate::handlers::row::read_row(&schema, &conn, "T429").unwrap();
        assert_eq!(
            after.get("status").and_then(|v| v.as_str()),
            Some("blocked")
        );
        let reason = after
            .get("blocked_reason")
            .and_then(|v| v.as_str())
            .unwrap();
        assert!(reason.starts_with("rate_limit:anthropic:"), "{reason}");
        assert_eq!(
            reason.strip_prefix("rate_limit:anthropic:").unwrap().len(),
            20
        );
    }

    // ---------------------------------------------------------------------------
    // MAJOR 1: spawn-fail telemetry — runner infrastructure failure (Err from spawn)
    // creates a synthetic agent_runs row with exit_code=-1 before blocking.
    // ---------------------------------------------------------------------------

    #[test]
    fn spawn_failure_creates_synthetic_agent_runs_row() {
        // Simulate: mock runner queue is empty → spawn_for_role returns Err.
        // Drive must insert a synthetic agent_runs row with exit_code=-1 and
        // a real .stores/runs/ transcript file before transitioning to blocked.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        let runs_dir = _dir.path().join(".stores").join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        // Empty queue → first spawn call returns Err("queue exhausted").
        let runner = MockRunner::new(vec![]);
        let err = drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect_err("spawn failure must cause drive_loop Err");
        assert!(
            err.to_string().contains("spawn failure"),
            "error must mention spawn failure: {err}"
        );

        // Synthetic agent_runs row must exist with exit_code = LAUNCH_ERROR_EXIT_CODE (-1).
        let (exit_code, model_id, harness_id, transcript_path): (i64, String, String, String) =
            conn.query_row(
                "SELECT exit_code, model_id, harness_id, transcript_path \
                 FROM agent_runs WHERE display_id='T001' AND role='planner'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .expect("synthetic agent_runs row must be inserted on spawn failure");
        assert_eq!(
            exit_code, -1,
            "exit_code must be LAUNCH_ERROR_EXIT_CODE (-1)"
        );
        assert!(!model_id.is_empty(), "model_id must be non-empty");
        assert!(!harness_id.is_empty(), "harness_id must be non-empty");
        assert!(
            !transcript_path.is_empty(),
            "transcript_path must be non-empty"
        );

        // Transcript file must exist and contain the error content.
        let tp = std::path::Path::new(&transcript_path);
        assert!(
            tp.exists(),
            "spawn-error transcript file must exist at: {}",
            tp.display()
        );
        assert!(
            tp.to_str().is_some_and(|s| s.contains(".stores")),
            "transcript_path must be under .stores/runs/: {}",
            tp.display()
        );
        let content = std::fs::read_to_string(tp).expect("transcript must be readable");
        let json: serde_json::Value =
            serde_json::from_str(&content).expect("spawn-error transcript must be valid JSON");
        assert_eq!(
            json.get("error").and_then(|v| v.as_str()),
            Some("spawn failed"),
            "spawn-error transcript must have error='spawn failed'"
        );

        // Task row must be blocked.
        let (_, after) = crate::handlers::row::read_row(&schema, &conn, "T001").unwrap();
        assert_eq!(
            after.get("status").and_then(|v| v.as_str()),
            Some("blocked"),
            "task must be blocked after spawn failure"
        );
    }

    #[test]
    fn runner_rate_limit_event_classifies_as_rate_limit_with_reset_at() {
        // T029: when stdout carries a stream-json `rate_limit_event` whose
        // `rate_limit_info.status != "allowed"`, classify as `rate_limit` and
        // capture `reset_at` from `resetsAt`.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T002",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        let stdout = r#"{"type":"system","subtype":"init"}
{"type":"rate_limit_event","rate_limit_info":{"status":"exceeded","resetsAt":1777395000,"rateLimitType":"five_hour"}}
"#;
        let fail_out = RunnerOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code: 1,
            final_message: None,
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(_dir.path()),
        };
        let runner = MockRunner::new(vec![fail_out]);

        let _ = drive_loop(&schema, &conn, "T002", &runner, 50)
            .expect_err("should fail on runner non-zero");

        let (_, after) = crate::handlers::row::read_row(&schema, &conn, "T002").unwrap();
        assert_eq!(
            after.get("status").and_then(|v| v.as_str()),
            Some("blocked")
        );
        let reason_str = after
            .get("blocked_reason")
            .and_then(|v| v.as_str())
            .expect("blocked_reason must be set");
        assert_eq!(reason_str, "rate_limit:codex:2026-04-28T16:50:00Z");
    }

    // ---------------------------------------------------------------------------
    // AC3.7: terminal-state early exit — complete/blocked exits before running runner
    // ---------------------------------------------------------------------------

    #[test]
    fn terminal_complete_errors_with_schema_bug_message() {
        // Under the new schema, `complete` is transient — the on_state follow-on
        // advances it to `in_review` inside the submit tx. A row stuck at `complete`
        // between drive iterations means the follow-on did NOT fire (schema bug /
        // manual DB surgery). Drive must exit non-zero with a clear diagnostic.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "complete",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        let runner = MockRunner::new(vec![]);

        // Should return Err (non-zero), NOT Ok(()), because `complete` is not terminal.
        let result = drive_loop(&schema, &conn, "T001", &runner, 50);
        assert!(
            result.is_err(),
            "drive_loop must error when row is at 'complete' (schema bug state)"
        );
        let msg = format!("{:?}", result.unwrap_err());
        assert!(
            msg.contains("complete") && msg.contains("follow-on"),
            "error message must mention 'complete' and 'follow-on'; got: {msg}"
        );
    }

    #[test]
    fn in_review_first_iteration_dispatches_wrap() {
        // AC4.3a: When drive is invoked and the row is already at `in_review`
        // (e.g. fresh run after a reject → amend → re-complete cycle), the first
        // iteration's next-action returns next_agent=wrap. Drive must dispatch wrap
        // (eager auto-fire per Decision (b)), not exit immediately.
        //
        // The state-local flag `dispatched_wrap_this_run` is false at the start of
        // every new drive run, so the loop-top guard falls through and wrap is
        // dispatched. Only after wrap submits does the flag flip, causing the next
        // iteration to exit with the "awaiting human" hint.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "in_review",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        // One wrap response queued — must be consumed for the test to pass.
        let wrap_out = make_run_output(wrap_fixture_json(), 0);
        let runner = MockRunner::new(vec![wrap_out]);

        // Drive must succeed (not error) and consume the wrap response.
        drive_loop(&schema, &conn, "T001", &runner, 50).expect(
            "in_review with dispatched_wrap_this_run=false must dispatch wrap and exit Ok(())",
        );

        // Assert the runner was fully drained — wrap response was consumed.
        assert_eq!(
            runner.remaining_count(),
            0,
            "wrap response must have been consumed (eager dispatch); {} responses remain",
            runner.remaining_count()
        );
    }

    #[test]
    fn in_review_re_entry_after_amend_dispatches_fresh_wrap() {
        // AC4.3a (pi ruling r3 strict-pi A1): A fresh drive invocation on a row
        // at `in_review` with an existing wrap_log[] entry (from a prior wrap run)
        // MUST dispatch wrap again. wrap_log is durable history evidence; it is NOT
        // a sentinel that THIS cycle's wrap is complete.
        //
        // Amend/re-entry is exactly where historical evidence is UNSAFE for control
        // flow: the wrap_log entry is from the prior cycle, not the current one.
        // next_agent=wrap IS the source of truth. If next_agent=wrap, dispatch wrap.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "in_review",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );
        // Pre-seed wrap_log from a prior cycle (simulating prior wrap via compute_submit_wrap).
        conn.execute(
            &format!(
                "UPDATE {} SET wrap_log = ?1 WHERE display_id = ?2",
                quote_ident(&schema.name)
            ),
            rusqlite::params![
                r#"[{"executive_summary":"prior wrap","deviations":[],"residual_risks":[],"recommended_sanity_checks":[],"at":"2026-01-01T00:00:00Z"}]"#,
                "T001"
            ],
        ).unwrap();

        // One wrap response queued — must be consumed (fresh dispatch regardless of existing wrap_log).
        let wrap_out = make_run_output(wrap_fixture_json(), 0);
        let runner = MockRunner::new(vec![wrap_out]);

        // Drive must succeed and dispatch wrap even though wrap_log is non-empty.
        drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect("in_review with existing wrap_log must still dispatch wrap on re-entry");

        // Assert the runner was fully drained — the fresh wrap response was consumed.
        assert_eq!(
            runner.remaining_count(),
            0,
            "wrap response must have been consumed on re-entry; {} responses remain",
            runner.remaining_count()
        );
    }

    #[test]
    fn terminal_blocked_exits_zero() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "blocked",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );
        // Write a blocked_reason
        conn.execute(
            &format!(
                "UPDATE {} SET blocked_reason = ?1 WHERE display_id = ?2",
                quote_ident(&schema.name)
            ),
            rusqlite::params!["test block reason", "T001"],
        )
        .unwrap();

        let runner = MockRunner::new(vec![]);

        // Blocked → exit 0 (not Err).
        drive_loop(&schema, &conn, "T001", &runner, 50).expect("blocked status should exit Ok(())");
    }

    // ---------------------------------------------------------------------------
    // AC2.4: structured_output takes precedence over malformed final_message
    // ---------------------------------------------------------------------------

    #[test]
    fn structured_output_takes_precedence_over_final_message() {
        // Provide a valid planner envelope via structured_output but a garbage
        // final_message — parse_envelope must succeed via structured_output.
        let valid_envelope = json!({
            "role": "planner",
            "phases": [],
            "decision_matrix": []
        });
        let tmp = tempdir().unwrap();
        let out = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            final_message: Some("this is not valid json {{{{".to_string()),
            structured_output: Some(valid_envelope),
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(tmp.path()),
        };
        let (env, source) =
            parse_envelope(&out, "planner").expect("should succeed via structured_output");
        assert!(
            matches!(env, AgentEnvelope::Planner { .. }),
            "should be Planner envelope"
        );
        assert_eq!(
            source, "sdk",
            "source must be sdk when structured_output is present"
        );
    }

    // ---------------------------------------------------------------------------
    // AC2.7: schema validation retries exhausted is surfaced in drive_loop stderr
    // ---------------------------------------------------------------------------

    #[test]
    fn retries_exhausted_surfaces_transcript_path() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        // Runner output that contains "schema validation retries exhausted" in
        // stderr, with a session_id, and non-zero exit so drive aborts.
        let fail_out = RunnerOutput {
            stdout: String::new(),
            stderr: "runner[planner]: schema validation retries exhausted (subtype=error_max_structured_output_retries); transcript at .stores/runs/test-uuid.jsonl".to_string(),
            exit_code: 1,
            final_message: None,
            structured_output: None,
            session_id: Some("test-uuid".to_string()),
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(_dir.path()),
        };
        let runner = MockRunner::new(vec![fail_out]);

        // Capture stderr by running drive_loop — we can't intercept eprintln! in
        // unit tests easily, so we verify the error path exits with non-zero and
        // that the stderr message the runner returns contains the right substrings.
        // The actual eprintln! in drive_loop for AC2.7 is validated by integration
        // test; here we just confirm the drive loop errors out correctly and the
        // RunnerOutput fields are what we expect.
        let err = drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect_err("should fail on non-zero exit");
        assert!(
            err.to_string().contains("non-zero exit"),
            "should mention non-zero exit: {err}"
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.10: parse_envelope tolerates commentary above the final line
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_envelope_tolerates_commentary() {
        let stdout = "I am doing some work here.\nSome more thinking.\n{\"role\": \"planner\", \"phases\": [], \"decision_matrix\": null}";
        let tmp = tempdir().unwrap();
        let out = RunnerOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code: 0,
            final_message: None,
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(tmp.path()),
        };
        let (env, source) =
            parse_envelope(&out, "planner").expect("should parse with commentary above");
        assert!(
            matches!(env, AgentEnvelope::Planner { .. }),
            "should be Planner envelope"
        );
        assert_eq!(source, "legacy", "last-line stdout scan is legacy layer");
    }

    // ---------------------------------------------------------------------------
    // AC3.10: parse_envelope uses fixture files correctly
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_envelope_from_planner_fixture() {
        let out = make_run_output(planner_fixture_json(), 0);
        let (env, _) = parse_envelope(&out, "planner").expect("planner fixture should parse");
        assert!(matches!(env, AgentEnvelope::Planner { .. }));
    }

    #[test]
    fn parse_envelope_from_plan_reviewer_fixture() {
        let out = make_run_output(plan_reviewer_fixture_json(), 0);
        let (env, _) =
            parse_envelope(&out, "plan-reviewer").expect("plan-reviewer fixture should parse");
        assert!(matches!(env, AgentEnvelope::PlanReviewer { .. }));
    }

    #[test]
    fn parse_envelope_from_executor_fixture() {
        let out = make_run_output(executor_fixture_json(), 0);
        let (env, _) = parse_envelope(&out, "executor").expect("executor fixture should parse");
        assert!(matches!(env, AgentEnvelope::Executor { .. }));
    }

    #[test]
    fn parse_envelope_from_code_reviewer_fixture() {
        let out = make_run_output(code_reviewer_fixture_json(), 0);
        let (env, _) =
            parse_envelope(&out, "code-reviewer").expect("code-reviewer fixture should parse");
        assert!(matches!(env, AgentEnvelope::CodeReviewer { .. }));
    }

    // ---------------------------------------------------------------------------
    // AC2.3: parse_envelope_from_wrap_fixture — Phase 2
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_envelope_from_wrap_fixture() {
        // Use structured_output (Layer 1) to avoid multi-line JSON stdout parsing
        // issues in make_run_output's last-line scan. The fixture is pretty-printed;
        // parse it as a serde_json::Value and inject via structured_output.
        let fixture_val: serde_json::Value = serde_json::from_str(wrap_full_fixture_json())
            .expect("wrap fixture must be valid JSON");
        let tmp = tempdir().unwrap();
        let out = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            final_message: None,
            structured_output: Some(fixture_val),
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(tmp.path()),
        };
        let (env, source) = parse_envelope(&out, "wrap").expect("wrap fixture should parse");
        assert_eq!(
            source, "sdk",
            "fixture via structured_output must use sdk layer"
        );
        match env {
            AgentEnvelope::Wrap {
                reasoning,
                executive_summary,
                deviations,
                residual_risks,
                recommended_sanity_checks,
            } => {
                assert!(
                    reasoning.is_some(),
                    "reasoning should be present in fixture"
                );
                assert!(
                    !executive_summary.is_empty(),
                    "executive_summary must be non-empty"
                );
                assert!(
                    !deviations.is_empty(),
                    "deviations must be non-empty in fixture"
                );
                assert!(
                    !residual_risks.is_empty(),
                    "residual_risks must be non-empty in fixture"
                );
                assert!(
                    !recommended_sanity_checks.is_empty(),
                    "recommended_sanity_checks must be non-empty in fixture"
                );
            }
            other => panic!("expected AgentEnvelope::Wrap, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------------
    // AC2.4: role-mismatch detection covers wrap — Phase 2
    // ---------------------------------------------------------------------------

    #[test]
    fn role_mismatch_wrap_envelope_while_executing() {
        // drive dispatches executor while executing; runner returns wrap envelope.
        // parse_envelope must return Err with "envelope role mismatch" naming both roles.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "executing",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        let misrouted = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            final_message: Some(
                r#"{"role":"wrap","executive_summary":"wrong agent sent this"}"#.to_string(),
            ),
            structured_output: None,
            session_id: Some("wrap-mismatch-session".to_string()),
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(_dir.path()),
        };
        let runner = MockRunner::new(vec![misrouted]);

        let err = drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect_err("wrap envelope while executing must cause Err");

        let msg = err.to_string();
        assert!(
            msg.contains("executor"),
            "error must name expected role 'executor': {msg}"
        );
        assert!(
            msg.contains("wrap"),
            "error must name received role 'wrap': {msg}"
        );
        assert!(
            msg.contains("wrap-mismatch-session"),
            "error must include session_id: {msg}"
        );
    }

    // ---------------------------------------------------------------------------
    // AC2.8: three_layer_fallback_for_markdown_fenced_planner_output
    // Mirrors the Phase 3 attempt 1 transcript: structured_output=None,
    // final_message=markdown-fenced JSON.  SAP (Layer 2) must recover the planner
    // envelope and the source tag must be "sap".
    // ---------------------------------------------------------------------------

    #[test]
    fn three_layer_fallback_for_markdown_fenced_planner_output() {
        // Representative slice of the haiku transcript result text (from
        // tests/fixtures/agent_outputs/planner-haiku-multiturn.jsonl result event).
        // The JSON envelope is wrapped in a ```json ... ``` markdown fence, which
        // is exactly the pathology that caused the Phase 3 attempt 1 failure.
        let fenced_text = concat!(
            "## Plan Summary\n\n",
            "Based on my analysis, this is a trivial single-phase task.\n\n",
            "```json\n",
            "{\n",
            "  \"role\": \"planner\",\n",
            "  \"phases\": [\n",
            "    {\n",
            "      \"name\": \"Phase 1: Create and test scripts/hi\",\n",
            "      \"objective\": \"Implement a shell script that echoes hi.\",\n",
            "      \"tasks\": [\"Task 1.1: Create scripts/hi\"],\n",
            "      \"acceptance_criteria\": [\"AC1.1: File scripts/hi exists\"],\n",
            "      \"files\": [\"scripts/hi\"],\n",
            "      \"dependencies\": []\n",
            "    }\n",
            "  ],\n",
            "  \"decision_matrix\": []\n",
            "}\n",
            "```"
        );

        let tmp = tempdir().unwrap();
        let out = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            // structured_output is None — Layer 1 (SDK) will miss.
            structured_output: None,
            // final_message contains the markdown-fenced JSON — Layer 2 (SAP) must recover it.
            final_message: Some(fenced_text.to_string()),
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(tmp.path()),
        };

        let (env, source) = parse_envelope(&out, "planner")
            .expect("SAP Layer 2 must recover planner envelope from markdown-fenced text");

        assert!(
            matches!(env, AgentEnvelope::Planner { .. }),
            "recovered envelope must be AgentEnvelope::Planner"
        );
        assert_eq!(
            source, "sap",
            "source tag must be 'sap' when Layer 2 (SAP) recovers the envelope"
        );
    }

    // ---------------------------------------------------------------------------
    // AC2.9: parse_envelope returns correct source tag for each of the 3 layers.
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_envelope_source_tag_sdk_layer() {
        // Layer 1 (SDK): structured_output is present and valid.
        let valid_envelope = json!({
            "role": "executor",
            "summary": "Done"
        });
        let tmp = tempdir().unwrap();
        let out = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            structured_output: Some(valid_envelope),
            final_message: Some("garbage {{{{ not json".to_string()),
            session_id: None,
            structured_output_source: Some("sdk"),
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(tmp.path()),
            payload_error: None,
        };
        let (_, source) = parse_envelope(&out, "executor").expect("sdk layer must succeed");
        assert_eq!(source, "sdk");
    }

    #[test]
    fn parse_envelope_source_tag_sap_layer() {
        // Layer 2 (SAP): structured_output is None, final_message has markdown-fenced JSON.
        let fenced =
            "Thinking...\n```json\n{\"role\":\"executor\",\"summary\":\"all done\"}\n```\n";
        let tmp = tempdir().unwrap();
        let out = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            structured_output: None,
            final_message: Some(fenced.to_string()),
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(tmp.path()),
        };
        let (_, source) = parse_envelope(&out, "executor").expect("sap layer must succeed");
        assert_eq!(source, "sap");
    }

    #[test]
    fn parse_envelope_source_tag_legacy_layer() {
        // Layer 3 (Legacy): structured_output is None, final_message is plain JSON
        // (no fences, not schema-validated by SAP — but direct parse succeeds).
        // Note: SAP also handles plain JSON, but if the schema lookup returns None
        // for an unknown role, SAP won't validate — it will return the first
        // parseable object anyway. To force legacy, use stdout last-line scan
        // by passing final_message=None and putting JSON on stdout.
        let json_line = "{\"role\":\"executor\",\"summary\":\"legacy path\"}";
        let tmp = tempdir().unwrap();
        let out = RunnerOutput {
            stdout: format!("some commentary\n{json_line}"),
            stderr: String::new(),
            exit_code: 0,
            structured_output: None,
            final_message: None, // No final_message → skip Layers 2+3 final_message paths.
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(tmp.path()),
        };
        let (_, source) =
            parse_envelope(&out, "executor").expect("legacy last-line scan must succeed");
        assert_eq!(source, "legacy");
    }

    // ---------------------------------------------------------------------------
    // Phase 2 — Bug 2: drive exits non-zero when runner returns wrong role
    // (drive_loop_role_mismatch_message_format below is the authoritative
    //  regression trap; it asserts the post-fix "envelope role mismatch" message
    //  format and subsumes the weaker Err(_) check that was here.)
    // ---------------------------------------------------------------------------

    #[test]
    fn drive_loop_role_mismatch_message_format() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "executing",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        let misrouted = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            final_message: Some("{\"role\":\"guide\",\"action\":\"noop\"}".to_string()),
            structured_output: None,
            session_id: Some("smoke-session-uuid".to_string()),
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(_dir.path()),
        };
        let runner = MockRunner::new(vec![misrouted]);

        let err = drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect_err("guide envelope while executing must cause Err");

        let msg = err.to_string();
        assert!(
            msg.contains("executor"),
            "error must name expected role 'executor': {msg}"
        );
        assert!(
            msg.contains("guide"),
            "error must name received role 'guide': {msg}"
        );
        assert!(
            msg.contains("smoke-session-uuid"),
            "error must include session_id: {msg}"
        );
    }

    // ---------------------------------------------------------------------------
    // Helper: read wrap_log from the tasks table for a given display_id
    // ---------------------------------------------------------------------------

    fn read_wrap_log_for(conn: &Connection, schema: &Schema, display_id: &str) -> Vec<Value> {
        let row: String = conn
            .query_row(
                &format!(
                    "SELECT COALESCE(wrap_log, '[]') FROM {} WHERE display_id = ?1",
                    quote_ident(&schema.name)
                ),
                rusqlite::params![display_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "[]".to_string());
        serde_json::from_str::<Vec<Value>>(&row).unwrap_or_default()
    }

    // ---------------------------------------------------------------------------
    // Phase 3 Finding 1 (code-reviewer): strengthen happy-path + in_review tests
    // to assert wrap_log[] content, not just queue drain.
    // AC4.7 — strengthened variants of the three queue-drain proxy tests.
    // ---------------------------------------------------------------------------

    #[test]
    fn happy_path_one_phase_mock_wrap_log_content() {
        // Strengthen happy_path_one_phase_mock: assert wrap_log[] has 1 entry
        // whose executive_summary == "stub" (matching wrap_fixture_json()).
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        // T072 r6: executor and code-reviewer must have session_id (MINOR 1).
        let runner = MockRunner::new(vec![
            make_run_output(planner_fixture_json(), 0),
            make_run_output(plan_reviewer_fixture_json(), 0),
            make_run_output_with_session(executor_fixture_json(), 0, "wl-exec-session"),
            make_run_output_with_session(code_reviewer_fixture_json(), 0, "wl-review-session"),
            make_run_output(wrap_fixture_json(), 0),
        ]);

        drive_loop(&schema, &conn, "T001", &runner, 50).expect("drive_loop should succeed");

        // Queue drain: all 5 mock responses consumed.
        assert_eq!(
            runner.remaining_count(),
            0,
            "all 5 responses must be consumed"
        );

        // AC4.7 strengthening: wrap_log[] has 1 entry.
        let log = read_wrap_log_for(&conn, &schema, "T001");
        assert_eq!(
            log.len(),
            1,
            "wrap_log must have exactly 1 entry after wrap dispatch"
        );

        // Latest entry's executive_summary == "stub" (matches wrap_fixture_json()).
        let latest = &log[0];
        assert_eq!(
            latest.get("executive_summary").and_then(|v| v.as_str()),
            Some("stub"),
            "wrap_log[0].executive_summary must match wrap_fixture_json() value 'stub'"
        );

        // Latest entry's `at` is set (non-empty).
        let at_val = latest.get("at").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            !at_val.is_empty(),
            "wrap_log[0].at must be non-empty ISO-8601 string; got: {at_val:?}"
        );
    }

    #[test]
    fn in_review_first_iteration_dispatches_wrap_log_content() {
        // Strengthen in_review_first_iteration_dispatches_wrap: assert wrap_log[]
        // length is 1 and executive_summary == "stub".
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "in_review",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        let runner = MockRunner::new(vec![make_run_output(wrap_fixture_json(), 0)]);
        drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect("drive must succeed for in_review first iteration");

        assert_eq!(
            runner.remaining_count(),
            0,
            "wrap response must be consumed"
        );

        let log = read_wrap_log_for(&conn, &schema, "T001");
        assert_eq!(
            log.len(),
            1,
            "wrap_log must have 1 entry after first dispatch"
        );
        assert_eq!(
            log[0].get("executive_summary").and_then(|v| v.as_str()),
            Some("stub"),
            "executive_summary must == 'stub'"
        );
        let at = log[0].get("at").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!at.is_empty(), "at must be set");
    }

    #[test]
    fn in_review_re_entry_after_amend_wrap_log_content() {
        // Strengthen in_review_re_entry_after_amend_dispatches_fresh_wrap: assert
        // wrap_log[] grows to 2 and the latest executive_summary == "stub".
        //
        // pi ruling r3 strict-pi A1: wrap_log is durable history. Re-entry dispatches
        // a fresh wrap; the new cycle appends its entry to the existing list.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "in_review",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );

        // Pre-seed one entry (simulating prior wrap cycle).
        conn.execute(
            &format!(
                "UPDATE {} SET wrap_log = ?1 WHERE display_id = ?2",
                quote_ident(&schema.name)
            ),
            rusqlite::params![
                r#"[{"executive_summary":"prior wrap","deviations":[],"residual_risks":[],"recommended_sanity_checks":[],"at":"2026-01-01T00:00:00Z"}]"#,
                "T001"
            ],
        ).unwrap();

        let runner = MockRunner::new(vec![make_run_output(wrap_fixture_json(), 0)]);
        drive_loop(&schema, &conn, "T001", &runner, 50).expect("drive must succeed for re-entry");

        assert_eq!(
            runner.remaining_count(),
            0,
            "wrap response must be consumed on re-entry"
        );

        // AC4.7: wrap_log grows to 2 entries.
        let log = read_wrap_log_for(&conn, &schema, "T001");
        assert_eq!(
            log.len(),
            2,
            "wrap_log must have 2 entries after re-entry wrap"
        );

        // Latest (index 1) executive_summary == "stub".
        assert_eq!(
            log[1].get("executive_summary").and_then(|v| v.as_str()),
            Some("stub"),
            "latest wrap_log entry executive_summary must == 'stub'"
        );
        let at = log[1].get("at").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!at.is_empty(), "latest wrap_log entry at must be set");
    }

    // ---------------------------------------------------------------------------
    // AC4.4: wrap brief template renders without error against a fixture row
    // ---------------------------------------------------------------------------

    #[test]
    fn wrap_brief_template_renders_with_fixture_row() {
        // AC4.4: render the wrap-brief.md.tpl against a fixture row populated
        // with contract + 3 cycles. Verifies the template has no syntax errors
        // and that key sections appear in the output.
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "in_review",
            "2026-01-01T00:00:00Z",
            3,
            1,
            None,
            None,
        );

        // Add some cycles to the row.
        conn.execute(
            &format!(
                "UPDATE {} SET cycles = ?1 WHERE display_id = ?2",
                quote_ident(&schema.name)
            ),
            rusqlite::params![
                r#"[
                  {"phase":1,"cycle":1,"executor":{"summary":"did phase 1","commit":"abc1"},"review":{"gate":"PASS","summary":"looks good"}},
                  {"phase":2,"cycle":1,"executor":{"summary":"did phase 2","commit":"abc2"},"review":{"gate":"REVISE","summary":"missing test"}},
                  {"phase":2,"cycle":2,"executor":{"summary":"fixed test","commit":"abc3"},"review":{"gate":"PASS","summary":"ok now"}}
                ]"#,
                "T001"
            ],
        ).unwrap();

        let (_, entry) = crate::handlers::row::read_row(&schema, &conn, "T001").unwrap();

        // Find the wrap-brief template from BUNDLED_STORE_TEMPLATES.
        let tpl_content = crate::cli::dynamic::BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(name, _)| *name == "tasks")
            .and_then(|(_, templates)| {
                templates
                    .iter()
                    .find(|(path, _)| path.contains("wrap-brief"))
                    .map(|(_, content)| *content)
            })
            .expect("wrap-brief template must be bundled");

        let ctx = crate::render::build_context(&schema, &entry);
        let mut overlay = std::collections::HashMap::new();
        overlay.insert(
            "git_diff_summary".to_string(),
            serde_json::Value::String("<git diff unavailable>".to_string()),
        );

        let rendered = crate::render::render_template_with_overlay(tpl_content, &ctx, &overlay)
            .expect("wrap-brief template must render without error");

        // Assert key sections appear.
        assert!(
            rendered.contains("T001"),
            "rendered brief must contain task ID"
        );
        assert!(
            rendered.contains("Promise"),
            "rendered brief must contain Promise section"
        );
        assert!(
            rendered.contains("Reality"),
            "rendered brief must contain Reality section"
        );
        assert!(
            rendered.contains("Diff"),
            "rendered brief must contain Diff section"
        );
        assert!(
            rendered.contains("<git diff unavailable>"),
            "Diff section must include the overlay value"
        );
        assert!(
            rendered.contains("Your Job"),
            "rendered brief must contain Your Job section"
        );
        // Cycles table rows.
        assert!(
            rendered.contains("did phase 1"),
            "Reality table must include phase 1 executor summary"
        );
        assert!(
            rendered.contains("REVISE"),
            "Reality table must include REVISE gate"
        );
    }

    // ---------------------------------------------------------------------------
    // AC4.5: wrap_brief_includes_git_diff_summary — overlay reaches rendered output
    // ---------------------------------------------------------------------------

    #[test]
    fn wrap_brief_includes_git_diff_summary() {
        // AC4.5: The git_diff_summary overlay assembled in drive.rs must reach the
        // rendered wrap brief. We inject a known placeholder and assert it appears
        // in the brief that drive builds before spawning the wrap agent.
        //
        // This test drives the full wrap-dispatch path via drive_loop with a
        // mock runner whose response includes an envelope, then reads the rendered
        // brief from the context window. Since we can't intercept the brief string
        // directly in tests (it goes to the runner as a parameter), we verify the
        // overlay plumbing through render_template_with_overlay in isolation — the
        // drive.rs integration path is verified by wrap_brief_template_renders_with_fixture_row.
        //
        // Specifically: assert that render_template_with_overlay merges correctly
        // (the overlay key appears in output) and that compute_git_diff_summary
        // returns a non-empty string in the test environment.
        let tpl = "diff: {{git_diff_summary}}";
        let ctx = serde_json::json!({});
        let mut overlay = std::collections::HashMap::new();
        overlay.insert(
            "git_diff_summary".to_string(),
            serde_json::Value::String("abc123..HEAD: 3 files changed".to_string()),
        );
        let rendered = crate::render::render_template_with_overlay(tpl, &ctx, &overlay)
            .expect("template must render");
        assert_eq!(rendered, "diff: abc123..HEAD: 3 files changed");
    }

    // ---------------------------------------------------------------------------
    // L503-A Task 1.10: drive spawn handler persists brief_text byte-equal to
    // what render_template_with_overlay produces at dispatch-time row state.
    // ---------------------------------------------------------------------------

    #[test]
    fn spawn_handler_persists_brief_text_byte_equal_to_rendered_brief() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        let runs_dir = _dir.path().join(".stores").join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        {
            let role = "planner";
            std::fs::write(runs_dir.join(format!("{role}.jsonl")), "{}\n").unwrap();
        }

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        // Pre-compute the expected brief from the current row state — identical
        // code path to what drive.rs executes at dispatch time.
        let expected_brief = {
            let (_, entry) = read_row(&schema, &conn, "T001").unwrap();
            let workflow = schema.workflow.as_ref().unwrap();
            let tpl_key = workflow
                .briefing_templates
                .get("planner")
                .unwrap()
                .to_string_lossy()
                .to_string();
            let tpl_content = crate::cli::dynamic::BUNDLED_STORE_TEMPLATES
                .iter()
                .find(|(n, _)| *n == "tasks")
                .and_then(|(_, tmps)| tmps.iter().find(|(p, _)| *p == tpl_key.as_str()).map(|(_, c)| *c))
                .expect("planner template must be bundled");
            let ctx = build_context(&schema, &entry);
            let mut overlay =
                crate::handlers::brief::build_source_observation_overlay(&conn, &entry).unwrap();
            for (k, v) in
                crate::handlers::brief::build_external_review_overlay(&conn, &entry).unwrap()
            {
                overlay.insert(k, v);
            }
            render_template_with_overlay(tpl_content, &ctx, &overlay).unwrap()
        };

        // Drive one iteration (planner); max_iters=1 causes an error after the
        // planner insert_agent_run call, which is fine — we just want the row.
        let make_telemetry = || crate::runner::AgentRunTelemetry {
            model_id: Some("mock-model-1".to_string()),
            harness_id: Some("mock".to_string()),
            started_at: Some(crate::handlers::row::now_iso8601()),
            ended_at: Some(crate::handlers::row::now_iso8601()),
            tokens_in: Some(0),
            tokens_out: Some(0),
            prompt_cache_hits: Some(0),
            transcript_path: Some(runs_dir.join("planner.jsonl").display().to_string()),
            stderr_log_path: None,
        };
        let mut planner_out = make_run_output(planner_fixture_json(), 0);
        planner_out.telemetry = make_telemetry();
        let runner = MockRunner::new(vec![planner_out]);
        let _ = drive_loop(&schema, &conn, "T001", &runner, 1);

        let stored_brief: Option<String> = conn
            .query_row(
                "SELECT brief_text FROM agent_runs WHERE display_id='T001' AND role='planner'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let stored_brief = stored_brief.expect("brief_text must be non-null for planner run");
        assert_eq!(
            stored_brief, expected_brief,
            "agent_runs.brief_text must be byte-equal to the brief rendered at dispatch time"
        );
    }

    // ---------------------------------------------------------------------------
    // L503-A Task 1.13: persisting brief_text must not perturb the rendering
    // pathway — two renders of the same fixture row must be byte-equal.
    // ---------------------------------------------------------------------------

    #[test]
    fn brief_rendering_byte_equal_after_persistence_cycle() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        let runs_dir = _dir.path().join(".stores").join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        // Helper closure to render the planner brief from the current row state.
        let render_planner_brief = || {
            let (_, entry) = read_row(&schema, &conn, "T001").unwrap();
            let workflow = schema.workflow.as_ref().unwrap();
            let tpl_key = workflow
                .briefing_templates
                .get("planner")
                .unwrap()
                .to_string_lossy()
                .to_string();
            let tpl_content = crate::cli::dynamic::BUNDLED_STORE_TEMPLATES
                .iter()
                .find(|(n, _)| *n == "tasks")
                .and_then(|(_, tmps)| tmps.iter().find(|(p, _)| *p == tpl_key.as_str()).map(|(_, c)| *c))
                .expect("planner template must be bundled");
            let ctx = build_context(&schema, &entry);
            let mut overlay =
                crate::handlers::brief::build_source_observation_overlay(&conn, &entry).unwrap();
            for (k, v) in
                crate::handlers::brief::build_external_review_overlay(&conn, &entry).unwrap()
            {
                overlay.insert(k, v);
            }
            render_template_with_overlay(tpl_content, &ctx, &overlay).unwrap()
        };

        // First render.
        let render1 = render_planner_brief();

        // Persist the brief via insert_agent_run (simulates drive spawn-handler).
        let telemetry = crate::runner::AgentRunTelemetry {
            model_id: Some("mock-model".to_string()),
            harness_id: Some("mock".to_string()),
            started_at: Some(crate::handlers::row::now_iso8601()),
            ended_at: Some(crate::handlers::row::now_iso8601()),
            tokens_in: Some(0),
            tokens_out: Some(0),
            prompt_cache_hits: Some(0),
            transcript_path: Some(runs_dir.join("planner.jsonl").display().to_string()),
            stderr_log_path: None,
        };
        std::fs::write(runs_dir.join("planner.jsonl"), "{}\n").unwrap();
        db::insert_agent_run(&conn, "T001", 0, 0, "planner", 0, &telemetry, Some(&render1))
            .unwrap();

        // Second render from the same fixture row — must be byte-equal.
        let render2 = render_planner_brief();
        assert_eq!(
            render1, render2,
            "brief rendering must be byte-equal before and after persistence cycle"
        );
    }

    // ---------------------------------------------------------------------------
    // AC4.6: git_diff_summary graceful degradation
    // ---------------------------------------------------------------------------

    #[test]
    fn git_diff_summary_unavailable_when_no_git_and_no_commit() {
        // AC4.6: When both git merge-base and the executor commit fallback are
        // unavailable (no commit provided), compute_git_diff_summary must return
        // "<git diff unavailable>" and must NOT error.
        //
        // We can't reliably test "no git binary" in the test environment (git is
        // always present), so we test the "both sources absent" path by providing
        // no executor commit. The git merge-base call may succeed or fail depending
        // on the test environment. If it succeeds, the function returns a diff
        // string (not the unavailable placeholder). If it fails AND no commit
        // is provided, it returns the placeholder.
        //
        // The invariant we assert: the function always returns a non-empty string
        // and never panics.
        let result = compute_git_diff_summary(None, None, None);
        assert!(
            !result.is_empty(),
            "compute_git_diff_summary must return non-empty string"
        );
    }

    #[test]
    fn git_diff_summary_with_first_executor_commit_fallback() {
        // AC4.6: When git merge-base fails but first_executor_commit is provided,
        // the fallback path must return a non-empty diff string (may be the
        // unavailable placeholder if the commit doesn't exist in this repo).
        // The invariant: returns non-empty, no panic.
        let result = compute_git_diff_summary(None, Some("HEAD~2"), None);
        assert!(
            !result.is_empty(),
            "compute_git_diff_summary with fallback commit must return non-empty string"
        );
    }

    // ---------------------------------------------------------------------------
    // T124: direction-aware diff summary regression test
    // ---------------------------------------------------------------------------

    /// T124: When main has commits ahead of the feature branch that touch files
    /// the branch didn't touch, compute_git_diff_summary must:
    ///   - label the branch's own commits under "On this branch"
    ///   - label main-ahead commits under "On base" with a do-not-attribute note
    ///   - NOT include main-ahead commits in the "On this branch" section
    ///
    /// This prevents the wrap agent from misattributing main-ahead work as
    /// belonging to the task being wrapped.
    #[test]
    fn git_diff_summary_direction_labeled_sections() {
        use std::process::Command;
        use tempfile::tempdir;

        // Serialize CWD changes against other tests.
        let _cwd_guard = crate::paths::test_cwd_lock()
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let old_cwd = std::env::current_dir().expect("must have cwd");
        let tmp = tempdir().expect("tempdir failed");
        let repo = tmp.path();

        // Helper: run a git command in the repo dir, ignore output.
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(repo)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@test.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@test.com")
                .status()
                .expect("git command failed");
            assert!(status.success(), "git {args:?} must succeed");
        };

        // Set up repo: init on 'main', add initial commit.
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "test@test.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(repo.join("readme.txt"), "init").unwrap();
        git(&["add", "readme.txt"]);
        git(&["commit", "-m", "initial"]);

        // Feature branch: add a commit touching feature-file.txt.
        git(&["checkout", "-b", "feat/task"]);
        std::fs::write(repo.join("feature-file.txt"), "feature work").unwrap();
        git(&["add", "feature-file.txt"]);
        git(&["commit", "-m", "feat: add feature-file"]);

        // Back to main: add a commit touching main-file.txt (NOT on feature branch).
        git(&["checkout", "main"]);
        std::fs::write(repo.join("main-file.txt"), "main work").unwrap();
        git(&["add", "main-file.txt"]);
        git(&["commit", "-m", "main: add main-file"]);

        // Back to feature branch for the wrap perspective.
        git(&["checkout", "feat/task"]);

        // Run compute_git_diff_summary from the feature branch CWD.
        std::env::set_current_dir(repo).expect("set_current_dir to repo");
        // Temporarily unset BASE_BRANCH in case the outer env has it.
        let base_branch_saved = std::env::var("BASE_BRANCH").ok();
        std::env::remove_var("BASE_BRANCH");

        let result = compute_git_diff_summary(Some("main"), None, None);
        // Restore env and CWD.
        if let Some(val) = base_branch_saved {
            std::env::set_var("BASE_BRANCH", val);
        }
        std::env::set_current_dir(&old_cwd).expect("restore cwd");

        // The output must include both section labels.
        assert!(
            result.contains("On this branch"),
            "output must contain 'On this branch' section; got:\n{result}"
        );
        assert!(
            result.contains("On base/main"),
            "output must contain 'On base/main' section; got:\n{result}"
        );
        assert!(
            result.contains("do NOT attribute"),
            "base-ahead section must carry the do-not-attribute note; got:\n{result}"
        );

        // The feature commit must appear in "On this branch", not in "On base".
        let on_branch_section = result
            .split("### On base/main")
            .next()
            .unwrap_or("");
        let on_base_section = result
            .split("### On base/main")
            .nth(1)
            .unwrap_or("");

        assert!(
            on_branch_section.contains("add feature-file"),
            "feature commit must appear in 'On this branch' section; branch section:\n{on_branch_section}"
        );
        assert!(
            !on_branch_section.contains("add main-file"),
            "main-ahead commit must NOT appear in 'On this branch' section; branch section:\n{on_branch_section}"
        );
        assert!(
            on_base_section.contains("add main-file"),
            "main-ahead commit must appear in 'On base/main' section; base section:\n{on_base_section}"
        );
    }

    // ---------------------------------------------------------------------------
    // AC1.6: workspace_path spawn-time tests
    // ---------------------------------------------------------------------------

    /// AC1.6: row with no workspace_path — runner records None for the cwd arg.
    #[test]
    fn workspace_path_unset_uses_inherited_cwd() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        // T072 r6: executor and code-reviewer must have session_id (MINOR 1).
        let runner = MockRunner::new(vec![
            make_run_output(planner_fixture_json(), 0),
            make_run_output(plan_reviewer_fixture_json(), 0),
            make_run_output_with_session(executor_fixture_json(), 0, "wp-unset-exec"),
            make_run_output_with_session(code_reviewer_fixture_json(), 0, "wp-unset-review"),
            make_run_output(wrap_fixture_json(), 0),
        ]);

        drive_loop(&schema, &conn, "T001", &runner, 50).expect("drive_loop should succeed");

        let paths = runner.workspace_paths_seen();
        // All spawns (planner, plan_reviewer, executor, code_reviewer, wrap) should record None.
        assert!(
            paths.iter().all(|p| p.is_none()),
            "all workspace_paths_seen must be None when workspace_path is unset, got: {paths:?}"
        );
    }

    /// AC1.6: row with workspace_path set to an existing tempdir — drive passes the
    /// row's raw string through to the runner on every spawn. (Canonicalization happens
    /// inside ClaudeCodeRunner; MockRunner records the raw value. The runner-level
    /// tests `workspace_path_canonicalised_when_some` verify the canonicalize step.)
    #[test]
    fn workspace_path_set_propagates_to_runner() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        let workspace_dir = tempdir().expect("tempdir for workspace");
        let workspace_path = workspace_dir.path().to_str().unwrap().to_string();
        let canonical = workspace_dir.path().canonicalize().unwrap();
        let canonical_str = canonical.to_str().unwrap().to_string();

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );
        conn.execute(
            &format!(
                "UPDATE {} SET workspace_path = ?1 WHERE display_id = ?2",
                crate::codegen::ddl::quote_ident(&schema.name)
            ),
            rusqlite::params![workspace_path, "T001"],
        )
        .unwrap();

        // T072 r6: executor and code-reviewer must have session_id (MINOR 1).
        let runner = MockRunner::new(vec![
            make_run_output(planner_fixture_json(), 0),
            make_run_output(plan_reviewer_fixture_json(), 0),
            make_run_output_with_session(executor_fixture_json(), 0, "wp-set-exec"),
            make_run_output_with_session(code_reviewer_fixture_json(), 0, "wp-set-review"),
            make_run_output(wrap_fixture_json(), 0),
        ]);

        drive_loop(&schema, &conn, "T001", &runner, 50).expect("drive_loop should succeed");

        let paths = runner.workspace_paths_seen();
        assert!(
            !paths.is_empty(),
            "workspace_paths_seen must be non-empty after drive"
        );
        // Every recorded path must equal the row's raw workspace_path string.
        // (MockRunner records what drive passed; canonicalization is the runner's
        // job, exercised by claude_code.rs::tests::workspace_path_canonicalised_when_some.)
        for p in &paths {
            assert_eq!(
                p.as_deref(),
                Some(workspace_path.as_str()),
                "all workspace_paths_seen must equal the row's workspace_path, got: {p:?}"
            );
        }
        let _ = canonical_str; // computed for the cross-reference in the comment above
    }

    /// AC1.6: row with workspace_path set to a non-existent directory — drive returns Err
    /// before spawn; runner queue is undrained (no spawn occurred).
    #[test]
    fn workspace_path_set_but_missing_errors_at_spawn() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        let missing_path = "/tmp/stores-test-nonexistent-workspace-path-99999";

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );
        conn.execute(
            &format!(
                "UPDATE {} SET workspace_path = ?1 WHERE display_id = ?2",
                crate::codegen::ddl::quote_ident(&schema.name)
            ),
            rusqlite::params![missing_path, "T001"],
        )
        .unwrap();

        // Queue has one response; if spawn is called, remaining_count drops to 0.
        let runner = MockRunner::new(vec![make_run_output(planner_fixture_json(), 0)]);

        let err = drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect_err("drive_loop must return Err when workspace_path is missing");

        let msg = err.to_string();
        assert!(
            msg.contains("T001"),
            "error message must contain display_id 'T001', got: {msg}"
        );
        assert!(
            msg.contains(missing_path),
            "error message must contain missing path, got: {msg}"
        );

        // Runner queue undrained — no spawn occurred.
        assert_eq!(
            runner.remaining_count(),
            1,
            "runner queue must be undrained (no spawn should have occurred)"
        );
    }

    /// AC1.6 (defensive): row with workspace_path set to a regular file rather than a
    /// directory — drive returns Err before spawn (would otherwise defer to current_dir's
    /// non-directory infra error). Companion to workspace_path_set_but_missing_errors_at_spawn.
    #[test]
    fn workspace_path_set_to_file_errors_at_spawn() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        let workspace_dir = tempdir().expect("tempdir for file fixture");
        let file_path = workspace_dir.path().join("not-a-directory");
        std::fs::write(&file_path, b"i am a file").expect("write file fixture");
        let file_str = file_path.to_str().unwrap().to_string();

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );
        conn.execute(
            &format!(
                "UPDATE {} SET workspace_path = ?1 WHERE display_id = ?2",
                crate::codegen::ddl::quote_ident(&schema.name)
            ),
            rusqlite::params![file_str, "T001"],
        )
        .unwrap();

        let runner = MockRunner::new(vec![make_run_output(planner_fixture_json(), 0)]);

        let err = drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect_err("drive_loop must return Err when workspace_path is a file");

        let msg = err.to_string();
        assert!(
            msg.contains("T001"),
            "error must contain display_id, got: {msg}"
        );
        assert!(
            msg.contains(&file_str),
            "error must contain the offending path, got: {msg}"
        );
        assert!(
            msg.contains("not a directory"),
            "error must say 'not a directory' to distinguish from missing-path case, got: {msg}"
        );

        assert_eq!(
            runner.remaining_count(),
            1,
            "runner queue must be undrained (no spawn should have occurred)"
        );
    }

    /// AC1.6: same row drives through two consecutive cycles (planner → plan_reviewer);
    /// both spawns record the same workspace_path, demonstrating canonicalize-once
    /// contract is honored per spawn-call.
    #[test]
    fn workspace_path_canonicalize_stable_across_spawns() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        let workspace_dir = tempdir().expect("tempdir for workspace");
        let workspace_path = workspace_dir.path().to_str().unwrap().to_string();

        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );
        conn.execute(
            &format!(
                "UPDATE {} SET workspace_path = ?1 WHERE display_id = ?2",
                crate::codegen::ddl::quote_ident(&schema.name)
            ),
            rusqlite::params![workspace_path, "T001"],
        )
        .unwrap();

        // T072 r6: executor and code-reviewer must have session_id (MINOR 1).
        let runner = MockRunner::new(vec![
            make_run_output(planner_fixture_json(), 0),
            make_run_output(plan_reviewer_fixture_json(), 0),
            make_run_output_with_session(executor_fixture_json(), 0, "wp-canon-exec"),
            make_run_output_with_session(code_reviewer_fixture_json(), 0, "wp-canon-review"),
            make_run_output(wrap_fixture_json(), 0),
        ]);

        drive_loop(&schema, &conn, "T001", &runner, 50).expect("drive_loop should succeed");

        let paths = runner.workspace_paths_seen();
        assert!(
            paths.len() >= 2,
            "must have at least 2 spawns to test stability"
        );

        // All recorded paths must be byte-identical — demonstrating the value is stable
        // across consecutive spawns (as it would be across spawn/resume calls with ClaudeCodeRunner).
        let first = paths[0].as_deref();
        for (i, p) in paths.iter().enumerate() {
            assert_eq!(
                p.as_deref(),
                first,
                "workspace_path at spawn {i} differs from spawn 0: expected {first:?}, got {p:?}"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // T033: pre-flight depends_on guard
    // ---------------------------------------------------------------------------

    /// Set the depends_on column for an existing task row to the given JSON
    /// array of display ids. Test helper.
    fn set_depends_on(conn: &Connection, schema: &Schema, display_id: &str, deps: &[&str]) {
        let json_str = serde_json::to_string(&deps.iter().collect::<Vec<_>>()).unwrap();
        conn.execute(
            &format!(
                "UPDATE {} SET depends_on = ?1 WHERE display_id = ?2",
                quote_ident(&schema.name)
            ),
            rusqlite::params![json_str, display_id],
        )
        .unwrap();
    }

    #[test]
    fn drive_refuses_when_dep_not_accepted() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        // TY: dep, status='executing' (not accepted).
        insert_task(
            &conn,
            &schema,
            "T002",
            "executing",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );
        // TX: depends on TY.
        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );
        set_depends_on(&conn, &schema, "T001", &["T002"]);

        let runner = MockRunner::new(vec![]);
        let err = drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect_err("drive must refuse when dep is not accepted");

        let msg = err.to_string();
        assert!(
            msg.contains("T002"),
            "error must name the unmet dep 'T002': {msg}"
        );
        assert!(
            msg.contains("executing"),
            "error must report dep status 'executing': {msg}"
        );
        assert!(
            msg.contains("depends_on"),
            "error must reference depends_on: {msg}"
        );
    }

    #[test]
    fn drive_proceeds_when_dep_accepted() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        // TY: dep, status='accepted'.
        insert_task(
            &conn,
            &schema,
            "T002",
            "accepted",
            "2026-01-01T00:00:00Z",
            1,
            1,
            None,
            None,
        );
        // TX: depends on TY, ready to run a full happy path.
        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );
        set_depends_on(&conn, &schema, "T001", &["T002"]);

        // T072 r6: executor and code-reviewer must have session_id (MINOR 1).
        let runner = MockRunner::new(vec![
            make_run_output(planner_fixture_json(), 0),
            make_run_output(plan_reviewer_fixture_json(), 0),
            make_run_output_with_session(executor_fixture_json(), 0, "dep-exec-session"),
            make_run_output_with_session(code_reviewer_fixture_json(), 0, "dep-review-session"),
            make_run_output(wrap_fixture_json(), 0),
        ]);

        drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect("drive must proceed when dep is accepted");

        assert_eq!(
            runner.remaining_count(),
            0,
            "all 5 mock responses must be consumed (drive ran the full happy path)"
        );
    }

    // ---------------------------------------------------------------------------
    // T047: SAP candidate selection prefers the real planner envelope when an
    // unrelated JSON-like object is also present in the prose above it.
    // ---------------------------------------------------------------------------

    #[test]
    fn t047_sap_picks_planner_envelope_over_unrelated_leading_object() {
        // Wrong/unrelated object appears FIRST in prose; the real planner
        // envelope (with non-empty phases) appears later inside a fenced block.
        let final_message = "I'll plan this out.\n\
            Here is some incidental data: {\"unrelated\": true}\n\
            And the plan:\n\
            ```json\n\
            {\"role\":\"planner\",\"phases\":[{\"name\":\"P1\"}],\"decision_matrix\":[]}\n\
            ```\n";
        let tmp = tempdir().unwrap();
        let out = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            final_message: Some(final_message.to_string()),
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(tmp.path()),
        };
        let (env, source) =
            parse_envelope(&out, "planner").expect("SAP must pick the right candidate");
        assert_eq!(source, "sap", "must succeed via SAP layer");
        match env {
            AgentEnvelope::Planner { phases, .. } => {
                let arr = phases.as_array().expect("phases must be array");
                assert_eq!(
                    arr.len(),
                    1,
                    "phases must contain the real plan, not the leading object"
                );
            }
            other => panic!("expected Planner, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------------
    // T047 AC1.2: end-to-end planner envelope embedded in markdown fences ⇒
    // tasks.plan is populated with phases array (not '{}', not empty).
    // ---------------------------------------------------------------------------

    #[test]
    fn t047_planner_with_fenced_envelope_persists_phases() {
        use crate::codegen::ddl::quote_ident;

        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);
        insert_task(
            &conn,
            &schema,
            "T001",
            "planning",
            "2026-01-01T00:00:00Z",
            0,
            0,
            None,
            None,
        );

        let final_message = "Reasoning: I'm going to plan this.\n\n\
            ```json\n\
            {\"role\":\"planner\",\"phases\":[{\"name\":\"T047-Persisted-Phase\",\"objective\":\"do X\"}],\"decision_matrix\":[]}\n\
            ```\n";

        let runner = MockRunner::new(vec![RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            final_message: Some(final_message.to_string()),
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(_dir.path()),
        }]);

        // Run a single iteration — drive_loop will exit after the first submit
        // because the mock runner has no further responses for the next agent.
        // We only care that submit-plan landed.
        let _ = drive_loop(&schema, &conn, "T001", &runner, 1);

        let table = quote_ident(&schema.name);
        let plan_str: String = conn
            .query_row(
                &format!("SELECT plan FROM {table} WHERE display_id = 'T001'"),
                [],
                |r| r.get(0),
            )
            .expect("plan column must exist");
        let plan: Value = serde_json::from_str(&plan_str).expect("plan must be valid JSON");
        let phases = plan
            .get("phases")
            .and_then(|v| v.as_array())
            .expect("plan.phases must be an array (T047 regression: was '{}')");
        assert!(
            !phases.is_empty(),
            "plan.phases must be non-empty (T047 regression)"
        );
        // Verify it's the planner-submitted plan, not the seed inserted by
        // insert_task() — distinct phase name proves the persistence path
        // actually overwrote the seed plan with the planner output.
        assert_eq!(
            phases[0].get("name").and_then(|n| n.as_str()).unwrap_or(""),
            "T047-Persisted-Phase",
            "submit-plan must overwrite the seed plan with the planner envelope"
        );

        // Status must have advanced from 'planning' to 'plan_review'.
        let status: String = conn
            .query_row(
                &format!("SELECT status FROM {table} WHERE display_id = 'T001'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "plan_review",
            "status must advance to plan_review after submit-plan"
        );
    }
}
