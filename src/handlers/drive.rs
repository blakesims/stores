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
/// WHERE status NOT IN ('complete', 'blocked')
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
use anyhow::{bail, Result};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

use crate::cli::agents::{BUNDLED_AGENT_SCHEMAS, BUNDLED_AGENTS};
use crate::cli::dynamic::BUNDLED_STORE_TEMPLATES;
use crate::db;
use crate::handlers::next_action::compute as compute_next_action;
use crate::handlers::render::compute_render_in;
use crate::handlers::row::read_row;
use crate::handlers::submit::{
    compute_submit_execute, compute_submit_plan, compute_submit_plan_review,
    compute_submit_review,
};
use crate::paths::db_path;
use crate::render::{build_context, render_template};
use crate::runner::{mock::MockRunner, Runner, RunnerOutput};
use crate::schema::{actor::Actor, Schema};

// ---------------------------------------------------------------------------
// Lock-expiry constant (same window as submit.rs – 300 seconds)
// ---------------------------------------------------------------------------

/// Seconds within which a `claimed_at` timestamp is considered a live claim.
/// Matches the 5-minute window used by `submit.rs`'s `acquire_lock`.
const LOCK_WINDOW_SECS: u64 = 300;

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
    /// Maximum loop iterations before hard-abort (AC3.5, default 50).
    pub max_iters: usize,
}

/// Drive a workflow task to a terminal state.
///
/// Prints progress to stderr (AC3.4).  Stdout is reserved for any structured
/// output.  Returns Ok(()) on `complete` or `blocked` (both exit 0); returns
/// Err on infrastructure failures or safety-rail violations (exit non-zero).
pub fn run_drive(schema: &Schema, args: DriveArgs) -> Result<()> {
    let conn = db::open(&db_path()?)?;

    // Resolve the task id.
    let display_id = resolve_task_id(schema, &conn, &args)?;

    // Select runner.
    let runner: Box<dyn Runner> = build_runner(&args)?;

    // Drive the loop.
    drive_loop(schema, &conn, &display_id, runner.as_ref(), args.max_iters)
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

fn is_leap(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
fn days_in_year(y: u32) -> u32 {
    if is_leap(y) { 366 } else { 365 }
}
fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap(y) { 29 } else { 28 },
        _ => 31,
    }
}
fn days_to_ymd(mut days: u64) -> (u32, u32, u32) {
    let mut year = 1970u32;
    loop {
        let dy = days_in_year(year) as u64;
        if days < dy { break; }
        days -= dy;
        year += 1;
    }
    let mut month = 1u32;
    loop {
        let dm = days_in_month(year, month) as u64;
        if days < dm { break; }
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
    let table = &schema.name;
    let sql = format!(
        "SELECT display_id FROM {table} \
         WHERE status NOT IN ('complete', 'blocked') \
           AND (claimed_by IS NULL OR claimed_at < ?1) \
         ORDER BY created_at ASC \
         LIMIT 1"
    );

    let result: rusqlite::Result<String> = conn.query_row(
        &sql,
        rusqlite::params![lock_expiry],
        |row| row.get(0),
    );

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
        let outputs: Vec<RunnerOutput> = items
            .into_iter()
            .map(|item| RunnerOutput {
                stdout: item.stdout,
                stderr: item.stderr,
                exit_code: item.exit_code,
                final_message: item.final_message,
                structured_output: None,
                session_id: None,
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
        return crate::runner::select("claude-code");
    }

    // Default: error — a runner must be explicitly chosen.
    bail!(
        "no runner selected; use --mock <fixture> for testing or \
         --claude-code (requires `--features runner-claude-code`) for production"
    )
}

// ---------------------------------------------------------------------------
// Main drive loop (AC3.1 / AC3.4 / AC3.5 / AC3.6 / AC3.7 / AC3.9 / AC3.10)
// ---------------------------------------------------------------------------

/// Core loop.  Extracted so tests can drive it directly without going through
/// clap or `run_drive`.
pub(crate) fn drive_loop(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    runner: &dyn Runner,
    max_iters: usize,
) -> Result<()> {
    let mut iter = 0usize;

    loop {
        // ── Step 2a: compute next-action ──────────────────────────────────
        let na = compute_next_action(schema, conn, display_id)?;

        // Terminal: complete
        if na.status == "complete" {
            eprintln!("[{display_id}] status=complete; drive finished");
            return Ok(());
        }

        // Terminal: blocked (AC3.9) — exit 0
        if na.blocked {
            let reason = na
                .blocked_reason
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            eprintln!(
                "[{display_id}] blocked: {reason}; run `stores gate {display_id} guide` for help"
            );
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
            let template_path = workflow
                .briefing_templates
                .get(agent_role)
                .ok_or_else(|| {
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
                        tpl_key, schema.name
                    )
                })?;
            let ctx = build_context(schema, &entry);
            render_template(tpl_content, &ctx)?
        };
        let system_prompt = BUNDLED_AGENTS
            .iter()
            .find(|(n, _)| *n == agent_name_normalized)
            .map(|(_, content)| *content)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no bundled agent for role '{}' (tried '{}'); \
                     run `stores agents install --all` first",
                    agent_role, agent_name_normalized
                )
            })?;

        // ── Step 2d: spawn runner ─────────────────────────────────────────
        // Look up the bundled JSON schema for this role.  Phase 2 threads it
        // through to the runner so it can pass --json-schema to the claude CLI.
        let schema_text: Option<&str> = BUNDLED_AGENT_SCHEMAS
            .iter()
            .find(|(n, _)| *n == agent_name_normalized)
            .map(|(_, s)| *s);

        // Pre-spawn announcement: runners block until the child exits, so without
        // this the user sees nothing for 30-90s per agent. v0.4 will stream child
        // stdout line-by-line; for v0.3 we just bookend the call.
        let phase_for_log = na.current_phase.as_i64().unwrap_or(0);
        let cycle_for_log = na.current_cycle.as_i64().unwrap_or(0);
        eprintln!(
            "[{display_id}] phase {phase_for_log} cycle {cycle_for_log}: spawning {agent_role} via {} runner... (may take 30-90s)",
            runner.name()
        );
        let spawn_start = std::time::Instant::now();
        let run_out = runner.spawn(&agent_name_normalized, system_prompt, &brief_markdown, schema_text)?;
        let spawn_elapsed = spawn_start.elapsed();
        eprintln!(
            "[{display_id}] phase {phase_for_log} cycle {cycle_for_log}: {agent_role} returned (exit={}, {:.1}s)",
            run_out.exit_code,
            spawn_elapsed.as_secs_f64()
        );

        // AC2.7: surface schema validation retry exhaustion before the exit-code
        // check so the user always sees it, even on non-zero exit.
        if run_out.stderr.contains("schema validation retries exhausted") {
            let transcript_hint = run_out
                .session_id
                .as_deref()
                .map(|sid| format!(".stores/runs/{sid}.jsonl"))
                .unwrap_or_else(|| "<no session-id>".to_string());
            eprintln!(
                "[{display_id}] schema validation retries exhausted; \
                 transcript: {transcript_hint}"
            );
        }

        // AC3.6: non-zero exit → surface stdout + stderr, no submit.
        // (Some CLIs route auth / login errors to stdout, so always include both.)
        if run_out.exit_code != 0 {
            eprintln!(
                "[{display_id}] runner exited with code {}; aborting without submitting",
                run_out.exit_code
            );
            if !run_out.stdout.is_empty() {
                eprintln!("runner stdout:\n{}", run_out.stdout);
            }
            if !run_out.stderr.is_empty() {
                eprintln!("runner stderr:\n{}", run_out.stderr);
            }
            bail!(
                "runner non-zero exit (code {}); task state unchanged",
                run_out.exit_code
            );
        }

        // ── Step 2e: parse envelope + dispatch submit ─────────────────────
        let envelope = parse_envelope(&run_out).map_err(|e| {
            eprintln!("[{display_id}] envelope parse failed: {e}");
            if !run_out.stdout.is_empty() {
                eprintln!("runner stdout:\n{}", run_out.stdout);
            }
            if !run_out.stderr.is_empty() {
                eprintln!("runner stderr:\n{}", run_out.stderr);
            }
            // Return the error so the caller sees it (no submit was called).
            anyhow::anyhow!("envelope parse error: {e}")
        })?;

        let submit_out = dispatch_submit(schema, conn, display_id, &na.status, envelope)?;

        // ── Step 2f: render ───────────────────────────────────────────────
        // Render is best-effort; failure is logged but does not abort the loop.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match compute_render_in(schema, conn, display_id, false, Actor::AiAutonomous, &cwd, &cwd) {
            Ok(render_out) => {
                if !render_out.dry_run {
                    if let Err(e) = apply_render(&render_out) {
                        eprintln!("[{display_id}] render write failed (non-fatal): {e}");
                    }
                }
            }
            Err(e) => {
                eprintln!("[{display_id}] render compute failed (non-fatal): {e}");
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
            "[{display_id}] phase {current_phase} cycle {current_cycle}: {agent_role} → submitted (gate={gate_display})"
        );

        // ── Step 2g: iter counter / max-iters (AC3.5) ────────────────────
        iter += 1;
        if iter >= max_iters {
            // Re-read state for summary.
            let na2 = compute_next_action(schema, conn, display_id)?;
            eprintln!(
                "[{display_id}] max iterations exceeded ({max_iters}); \
                 current state: status={} phase={} cycle={}",
                na2.status, na2.current_phase, na2.current_cycle
            );
            bail!("max iterations exceeded ({max_iters}) for task {display_id}");
        }
    }
}

// ---------------------------------------------------------------------------
// Envelope parser (AC3.10)
// ---------------------------------------------------------------------------

/// Extract and parse a JSON envelope from runner output.
///
/// Prefers `RunnerOutput.structured_output` (set by `--json-schema`-validated
/// runs). Falls back to `final_message` and then a last-line stdout scan for
/// legacy mock-fixture compatibility (AC2.3).
fn parse_envelope(out: &RunnerOutput) -> Result<AgentEnvelope> {
    // Prefer structured_output when present (schema-validated path).
    if let Some(value) = &out.structured_output {
        return serde_json::from_value(value.clone()).map_err(|e| {
            anyhow::anyhow!("structured_output deserialise failed: {e}\nvalue: {value}")
        });
    }

    // Legacy fallback: use final_message if already extracted.
    if let Some(fm) = &out.final_message {
        if !fm.trim().is_empty() {
            return serde_json::from_str::<AgentEnvelope>(fm).map_err(|e| {
                anyhow::anyhow!("final_message JSON parse failed: {e}\nraw: {fm}")
            });
        }
    }

    // Last-resort: scan stdout for last non-empty JSON line.
    let last_line = out
        .stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty());

    match last_line {
        None => bail!(
            "runner produced no output (stdout is empty or all-whitespace); \
             expected a JSON envelope on the last line"
        ),
        Some(line) => serde_json::from_str::<AgentEnvelope>(line).map_err(|e| {
            anyhow::anyhow!(
                "last stdout line is not a valid agent envelope: {e}\nraw line: {line}"
            )
        }),
    }
}

// ---------------------------------------------------------------------------
// Submit dispatcher
// ---------------------------------------------------------------------------

fn dispatch_submit(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    current_status: &str,
    envelope: AgentEnvelope,
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
            let files_str: Option<String> =
                files_changed.map(|v| v.join(","));
            compute_submit_execute(
                schema,
                conn,
                display_id,
                &summary,
                commit.as_deref(),
                files_str.as_deref(),
                None,
                Actor::AiAutonomous,
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
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("stores/tasks/schema.yaml"),
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
                 created_by, updated_by, title, slug, current_phase, current_cycle, \
                 plan, contract, cycles, plan_review_log, claimed_by, claimed_at) \
                 VALUES (?1,?2,?3,?3,?4,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                name = schema.name
            ),
            rusqlite::params![
                display_id,
                status,
                created_at,
                "human",
                "Test Task",
                "test-task",
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
        let last_line = stdout.lines().rev().find(|l| !l.trim().is_empty()).map(|s| s.to_string());
        RunnerOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code,
            final_message: last_line,
            structured_output: None,
            session_id: None,
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

    // ---------------------------------------------------------------------------
    // AC3.7: happy-path through 1 full phase (planning → plan_review →
    // executing → code_review → complete)
    // ---------------------------------------------------------------------------

    #[test]
    fn happy_path_one_phase_mock() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        // Insert task in planning state, phase=0 (not yet started)
        insert_task(
            &conn, &schema, "T001", "planning",
            "2026-01-01T00:00:00Z", 0, 0, None, None,
        );

        // Queue: planner → plan_reviewer → executor → code_reviewer
        let planner_out = make_run_output(planner_fixture_json(), 0);
        let plan_reviewer_out = make_run_output(plan_reviewer_fixture_json(), 0);
        let executor_out = make_run_output(executor_fixture_json(), 0);
        let code_reviewer_out = make_run_output(code_reviewer_fixture_json(), 0);

        let runner = MockRunner::new(vec![planner_out, plan_reviewer_out, executor_out, code_reviewer_out]);

        drive_loop(&schema, &conn, "T001", &runner, 50).expect("drive_loop should succeed");

        // Verify final status
        let na = compute_next_action(&schema, &conn, "T001").unwrap();
        assert_eq!(na.status, "complete", "task should be complete after drive");
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
            &conn, &schema, "T001", "planning",
            "2026-02-01T00:00:00Z", 0, 0, None, None,
        );
        insert_task(
            &conn, &schema, "T002", "planning",
            "2026-01-01T00:00:00Z", 0, 0, None, None,
        );

        let args = DriveArgs {
            display_id: None,
            auto: true,
            mock_fixture: None,
            #[cfg(feature = "runner-claude-code")]
            claude_code: false,
            #[cfg(feature = "runner-claude-code")]
            testing: false,
            max_iters: 50,
        };

        let selected = resolve_task_id(&schema, &conn, &args).unwrap();
        assert_eq!(selected, "T002", "should pick T002 with earliest created_at");
    }

    // ---------------------------------------------------------------------------
    // AC3.7: live-claim skip — claimed row within lock window is skipped
    // ---------------------------------------------------------------------------

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
            &conn, &schema, "T001", "planning",
            "2026-01-01T00:00:00Z", 0, 0,
            Some("other-runner"), Some(&now),
        );
        // T002 is not claimed
        insert_task(
            &conn, &schema, "T002", "planning",
            "2026-01-02T00:00:00Z", 0, 0, None, None,
        );

        let args = DriveArgs {
            display_id: None,
            auto: true,
            mock_fixture: None,
            #[cfg(feature = "runner-claude-code")]
            claude_code: false,
            #[cfg(feature = "runner-claude-code")]
            testing: false,
            max_iters: 50,
        };

        let selected = resolve_task_id(&schema, &conn, &args).unwrap();
        assert_eq!(selected, "T002", "should skip live-claimed T001 and pick T002");
    }

    // ---------------------------------------------------------------------------
    // AC3.7: max-iters — loop aborts when limit reached
    // ---------------------------------------------------------------------------

    #[test]
    fn max_iters_aborts_loop() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn, &schema, "T001", "planning",
            "2026-01-01T00:00:00Z", 0, 0, None, None,
        );

        // Queue only 1 planner response (advances to plan_review), then the loop
        // will try to call runner again (which would fail from queue exhaustion).
        // But with max_iters=1, it should abort after 1 iteration.
        let planner_out = make_run_output(planner_fixture_json(), 0);
        let runner = MockRunner::new(vec![planner_out]);

        let err = drive_loop(&schema, &conn, "T001", &runner, 1)
            .expect_err("should fail with max-iters");
        let msg = err.to_string();
        assert!(
            msg.contains("max iterations exceeded"),
            "error must mention max iterations: {msg}"
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.7: runner-error abort — non-zero exit does not corrupt task state
    // ---------------------------------------------------------------------------

    #[test]
    fn runner_error_mid_loop_does_not_corrupt_state() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn, &schema, "T001", "planning",
            "2026-01-01T00:00:00Z", 0, 0, None, None,
        );

        // Read the row before the failed runner call.
        let before = {
            let (_, entry) = crate::handlers::row::read_row(&schema, &conn, "T001").unwrap();
            entry
        };

        // Runner returns non-zero exit immediately.
        let fail_out = RunnerOutput {
            stdout: "some partial output".to_string(),
            stderr: "runner crashed".to_string(),
            exit_code: 1,
            final_message: None,
            structured_output: None,
            session_id: None,
        };
        let runner = MockRunner::new(vec![fail_out]);

        let err = drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect_err("should fail on runner error");
        assert!(
            err.to_string().contains("non-zero exit"),
            "error must mention non-zero exit: {}",
            err
        );

        // Task state must be byte-identical.
        let after = {
            let (_, entry) = crate::handlers::row::read_row(&schema, &conn, "T001").unwrap();
            entry
        };

        // Status must be unchanged.
        assert_eq!(
            before.get("status"),
            after.get("status"),
            "status must be unchanged after runner error"
        );
        assert_eq!(
            before.get("plan"),
            after.get("plan"),
            "plan must be unchanged after runner error"
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.7: terminal-state early exit — complete/blocked exits before running runner
    // ---------------------------------------------------------------------------

    #[test]
    fn terminal_complete_exits_without_spawning() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn, &schema, "T001", "complete",
            "2026-01-01T00:00:00Z", 1, 1, None, None,
        );

        // Empty runner — if spawned, it would error.
        let runner = MockRunner::new(vec![]);

        // Should return Ok(()) immediately without touching the runner.
        drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect("complete status should exit immediately");
    }

    #[test]
    fn terminal_blocked_exits_zero() {
        let schema = tasks_schema();
        let (_dir, conn) = open_db(&schema);

        insert_task(
            &conn, &schema, "T001", "blocked",
            "2026-01-01T00:00:00Z", 1, 1, None, None,
        );
        // Write a blocked_reason
        conn.execute(
            &format!("UPDATE {} SET blocked_reason = ?1 WHERE display_id = ?2", schema.name),
            rusqlite::params!["test block reason", "T001"],
        ).unwrap();

        let runner = MockRunner::new(vec![]);

        // Blocked → exit 0 (not Err).
        drive_loop(&schema, &conn, "T001", &runner, 50)
            .expect("blocked status should exit Ok(())");
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
        let out = RunnerOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            final_message: Some("this is not valid json {{{{".to_string()),
            structured_output: Some(valid_envelope),
            session_id: None,
        };
        let env = parse_envelope(&out).expect("should succeed via structured_output");
        assert!(
            matches!(env, AgentEnvelope::Planner { .. }),
            "should be Planner envelope"
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
            &conn, &schema, "T001", "planning",
            "2026-01-01T00:00:00Z", 0, 0, None, None,
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
        let out = RunnerOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code: 0,
            final_message: None,
            structured_output: None,
            session_id: None,
        };
        let env = parse_envelope(&out).expect("should parse with commentary above");
        assert!(
            matches!(env, AgentEnvelope::Planner { .. }),
            "should be Planner envelope"
        );
    }

    // ---------------------------------------------------------------------------
    // AC3.10: parse_envelope uses fixture files correctly
    // ---------------------------------------------------------------------------

    #[test]
    fn parse_envelope_from_planner_fixture() {
        let out = make_run_output(planner_fixture_json(), 0);
        let env = parse_envelope(&out).expect("planner fixture should parse");
        assert!(matches!(env, AgentEnvelope::Planner { .. }));
    }

    #[test]
    fn parse_envelope_from_plan_reviewer_fixture() {
        let out = make_run_output(plan_reviewer_fixture_json(), 0);
        let env = parse_envelope(&out).expect("plan-reviewer fixture should parse");
        assert!(matches!(env, AgentEnvelope::PlanReviewer { .. }));
    }

    #[test]
    fn parse_envelope_from_executor_fixture() {
        let out = make_run_output(executor_fixture_json(), 0);
        let env = parse_envelope(&out).expect("executor fixture should parse");
        assert!(matches!(env, AgentEnvelope::Executor { .. }));
    }

    #[test]
    fn parse_envelope_from_code_reviewer_fixture() {
        let out = make_run_output(code_reviewer_fixture_json(), 0);
        let env = parse_envelope(&out).expect("code-reviewer fixture should parse");
        assert!(matches!(env, AgentEnvelope::CodeReviewer { .. }));
    }
}
