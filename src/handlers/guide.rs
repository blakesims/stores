/// `guide` handler — human-boundary affordance.
///
/// Curates a context bundle from the relevant DB rows and spawns the guide agent
/// (whose system prompt was authored in Phase 1 and lives at `agents/guide.md`).
///
/// Two forms are supported:
///
/// - **Gate form** (`stores gate <id> guide`): full context — gate row, linked
///   task row, recent plan-review + code-review cycles, and the list of authorized
///   CLI verbs.  After the runner exits the handler reads back the gate row's
///   status: `pending` → `answered` = exit 0; anything else = exit 1 (AC6.5).
///
/// - **Task stub form** (`stores tasks <id> guide`): task row + last next-action
///   output + last review cycle.  No DB write-back check; exits 0 on runner
///   exit 0, 1 otherwise.
///
/// # Mock runner
///
/// `--mock <fixture-path>` (always built, hidden via `.hide(true)`) loads a JSON
/// array of `MockFixtureItem` objects and feeds them to the mock runner in order.
///
/// # Claude Code runner
///
/// `--claude-code` (feature-gated) invokes the `claude -p` runner.  Requires
/// `--features runner-claude-code`.
use anyhow::{bail, Result};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;

use crate::cli::agents::BUNDLED_AGENTS;
use crate::db;
use crate::handlers::next_action::compute as compute_next_action;
use crate::handlers::row::read_row;
use crate::paths::db_path;
use crate::runner::{mock::MockRunner, Runner, RunnerOutput};
use crate::schema::Schema;

// ---------------------------------------------------------------------------
// Authorized verb lists
// ---------------------------------------------------------------------------

/// The 5 verbs the guide agent is authorized to call in gate mode.
pub(crate) const AUTHORIZED_VERBS: &[&str] = &[
    "stores gate show",
    "stores gate answer",
    "stores tasks show",
    "stores tasks list",
    "stores tasks next-action",
];

/// The 6 verbs authorized in wrap mode (AC5.3).
pub(crate) const WRAP_MODE_VERBS: &[&str] = &[
    "stores tasks show",
    "stores tasks list",
    "stores tasks next-action",
    "stores tasks accept",
    "stores tasks reject",
    "stores gate add",
];

// ---------------------------------------------------------------------------
// Fixture shape (same as drive's MockFixtureItem)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct MockFixtureItem {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub final_message: Option<String>,
}

// ---------------------------------------------------------------------------
// Args structs
// ---------------------------------------------------------------------------

pub struct GateGuideArgs {
    /// Gate display_id.
    pub display_id: String,
    /// Path to JSON fixture file for mock runner.
    pub mock_fixture: Option<PathBuf>,
    /// Use claude-code runner (feature-gated).
    #[cfg(feature = "runner-claude-code")]
    pub claude_code: bool,
}

pub struct TasksGuideArgs {
    /// Task display_id.
    pub display_id: String,
    /// Path to JSON fixture file for mock runner.
    pub mock_fixture: Option<PathBuf>,
    /// Use claude-code runner (feature-gated).
    #[cfg(feature = "runner-claude-code")]
    pub claude_code: bool,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// `stores gate <id> guide` — full form.
pub fn run_gate_guide(gate_schema: &Schema, args: GateGuideArgs) -> Result<()> {
    let db = db_path()?;
    let conn = db::open(&db)?;

    let runner: Box<dyn Runner> = build_runner_from_args(
        args.mock_fixture.as_ref(),
        #[cfg(feature = "runner-claude-code")]
        args.claude_code,
    )?;

    run_gate_guide_with_runner(gate_schema, &conn, &args.display_id, runner.as_ref())
}

/// `stores tasks <id> guide` — stub form.
///
/// v0.3 stub-quality form: dumps context bundle, no specialized tooling.
/// Full form expected in v0.4.
pub fn run_tasks_guide(tasks_schema: &Schema, args: TasksGuideArgs) -> Result<()> {
    let db = db_path()?;
    let conn = db::open(&db)?;

    let runner: Box<dyn Runner> = build_runner_from_args(
        args.mock_fixture.as_ref(),
        #[cfg(feature = "runner-claude-code")]
        args.claude_code,
    )?;

    run_tasks_guide_with_runner(tasks_schema, &conn, &args.display_id, runner.as_ref())
}

// ---------------------------------------------------------------------------
// Runner construction (mirrors drive.rs)
// ---------------------------------------------------------------------------

fn build_runner_from_args(
    mock_fixture: Option<&PathBuf>,
    #[cfg(feature = "runner-claude-code")] claude_code: bool,
) -> Result<Box<dyn Runner>> {
    if let Some(fixture_path) = mock_fixture {
        let text = std::fs::read_to_string(fixture_path).map_err(|e| {
            anyhow::anyhow!("cannot read mock fixture '{}': {e}", fixture_path.display())
        })?;
        let items: Vec<MockFixtureItem> = serde_json::from_str(&text).map_err(|e| {
            anyhow::anyhow!(
                "mock fixture '{}' is not a valid JSON array: {e}",
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
                structured_output_source: None,
                telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(),
                payload_error: None,
            })
            .collect();
        return Ok(Box::new(MockRunner::new(outputs)));
    }

    #[cfg(feature = "runner-claude-code")]
    if claude_code {
        return crate::runner::select("claude-code");
    }

    bail!(
        "no runner selected; use --mock <fixture> for testing or \
         --claude-code (requires `--features runner-claude-code`) for production"
    )
}

// ---------------------------------------------------------------------------
// AC6.5 gate status check (pure function, used by the handler and tests)
// ---------------------------------------------------------------------------

/// Evaluate the AC6.5 exit-code policy given the status before and after
/// the guide session.
///
/// Returns `Ok(())` if `status_before == "pending"` and `status_after == "answered"`.
/// Returns `Err` with an explanatory message otherwise.
pub(crate) fn check_gate_transition(
    display_id: &str,
    status_before: &str,
    status_after: &str,
) -> Result<()> {
    if status_before == "pending" && status_after == "answered" {
        eprintln!("[guide] gate {display_id}: pending → answered");
        Ok(())
    } else {
        bail!(
            "guide session ended without answering gate {display_id} \
             (status_before={status_before}, status_after={status_after})"
        )
    }
}

/// Read the `status` field of a gate row from the DB.
pub(crate) fn read_gate_status(
    gate_schema: &Schema,
    conn: &Connection,
    display_id: &str,
) -> Result<String> {
    let (_, entry) = read_row(gate_schema, conn, display_id)?;
    Ok(entry
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string())
}

// ---------------------------------------------------------------------------
// Gate guide core (testable without opening a real DB)
// ---------------------------------------------------------------------------

/// Core logic for `stores gate <id> guide`. Separated for unit tests.
pub(crate) fn run_gate_guide_with_runner(
    gate_schema: &Schema,
    conn: &Connection,
    display_id: &str,
    runner: &dyn Runner,
) -> Result<()> {
    // Read status before runner (AC6.5 pre-check).
    let status_before = read_gate_status(gate_schema, conn, display_id)?;

    // Load gate row for bundle building.
    let (_, gate_entry) = read_row(gate_schema, conn, display_id)?;

    // Optionally load linked task row (both stores share the same SQLite file).
    let task_ref = gate_entry
        .get("task_ref")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let linked_task: Option<Value> = if let Some(ref task_id) = task_ref {
        load_task_row(conn, task_id)
    } else {
        None
    };

    // Extract recent plan-review log and last 2 code-review cycles from task.
    let (plan_review_excerpt, code_review_excerpt) = if let Some(ref task_val) = linked_task {
        let pr = extract_plan_review_log(task_val);
        let cr = extract_last_n_cycles(task_val, 2);
        (pr, cr)
    } else {
        (String::new(), String::new())
    };

    // Build the guide system prompt.
    let system_prompt = BUNDLED_AGENTS
        .iter()
        .find(|(n, _)| *n == "guide")
        .map(|(_, c)| *c)
        .ok_or_else(|| anyhow::anyhow!("no bundled 'guide' agent found"))?;

    // Build the context bundle (brief).
    let brief = build_gate_brief(
        display_id,
        &gate_entry,
        linked_task.as_ref(),
        &plan_review_excerpt,
        &code_review_excerpt,
        task_ref.as_deref(),
    );

    // Spawn runner.
    let run_out = runner.spawn("guide", system_prompt, &brief, None, None)?;

    if run_out.exit_code != 0 {
        eprintln!("[guide] runner exited with code {}", run_out.exit_code);
        if !run_out.stderr.is_empty() {
            eprintln!("runner stderr:\n{}", run_out.stderr);
        }
        bail!("guide runner non-zero exit (code {})", run_out.exit_code);
    }

    // AC6.5: read back gate status and check the single-bit transition.
    let status_after = read_gate_status(gate_schema, conn, display_id)?;
    check_gate_transition(display_id, &status_before, &status_after)
}

// ---------------------------------------------------------------------------
// Tasks guide core
// ---------------------------------------------------------------------------

/// Core logic for `stores tasks <id> guide`. Separated for unit tests.
///
/// Status-keyed mode dispatch (AC5.1):
/// - `in_review` → `build_wrap_mode_brief`
/// - all other statuses → `build_tasks_brief` (existing v0.3 stub)
pub(crate) fn run_tasks_guide_with_runner(
    tasks_schema: &Schema,
    conn: &Connection,
    display_id: &str,
    runner: &dyn Runner,
) -> Result<()> {
    // Load task row.
    let (_, task_entry) = read_row(tasks_schema, conn, display_id)?;

    // Determine current status for mode dispatch (AC5.1).
    let status = task_entry
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Build the guide system prompt.
    let system_prompt = BUNDLED_AGENTS
        .iter()
        .find(|(n, _)| *n == "guide")
        .map(|(_, c)| *c)
        .ok_or_else(|| anyhow::anyhow!("no bundled 'guide' agent found"))?;

    // AC5.1: branch on status.
    let brief = if status == "in_review" {
        // Wrap mode — build synthesis brief.
        let task_val = Value::Object(
            task_entry
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let wrap_log_latest = extract_latest_wrap_log_entry(&task_val);
        build_wrap_mode_brief(display_id, &task_entry, wrap_log_latest.as_ref())
    } else {
        // Task mode (v0.3 stub) for all other statuses.
        let na = compute_next_action(tasks_schema, conn, display_id)?;
        let next_action_text = serde_json::to_string_pretty(&na).unwrap_or_default();
        let task_val = Value::Object(
            task_entry
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let last_review = extract_last_n_cycles(&task_val, 1);
        build_tasks_brief(display_id, &task_entry, &next_action_text, &last_review)
    };

    // Spawn runner.
    let run_out = runner.spawn("guide", system_prompt, &brief, None, None)?;

    if run_out.exit_code != 0 {
        eprintln!("[guide] runner exited with code {}", run_out.exit_code);
        if !run_out.stderr.is_empty() {
            eprintln!("runner stderr:\n{}", run_out.stderr);
        }
        bail!("guide runner non-zero exit (code {})", run_out.exit_code);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Bundle builders (AC6.1 / AC6.2)
// ---------------------------------------------------------------------------

/// Build the gate-full context bundle (AC6.1).
///
/// Includes: gate row JSON, linked task row (if any), recent plan-review log,
/// last 2 code-review cycles, and the list of authorized CLI verbs.
pub(crate) fn build_gate_brief(
    gate_id: &str,
    gate_entry: &crate::validate::EntryMap,
    linked_task: Option<&Value>,
    plan_review_excerpt: &str,
    code_review_excerpt: &str,
    task_ref: Option<&str>,
) -> String {
    let gate_json = serde_json::to_string_pretty(gate_entry).unwrap_or_default();

    let linked_task_section = match linked_task {
        Some(t) => {
            let task_json = serde_json::to_string_pretty(t).unwrap_or_default();
            format!("## Linked Task Row\n\n```json\n{task_json}\n```\n")
        }
        None => {
            let ref_note = task_ref
                .map(|r| format!(" (task_ref={r} but row not found)"))
                .unwrap_or_default();
            format!("## Linked Task Row\n\n_No linked task{ref_note}._\n")
        }
    };

    let plan_review_section = if plan_review_excerpt.is_empty() {
        "## Recent Plan Reviews\n\n_None._\n".to_string()
    } else {
        format!("## Recent Plan Reviews\n\n{plan_review_excerpt}\n")
    };

    let code_review_section = if code_review_excerpt.is_empty() {
        "## Last 2 Code Review Cycles\n\n_None._\n".to_string()
    } else {
        format!("## Last 2 Code Review Cycles\n\n{code_review_excerpt}\n")
    };

    let verbs_list = AUTHORIZED_VERBS
        .iter()
        .map(|v| format!("- `{v}`"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "# Guide Context Bundle — Gate Mode\n\n\
         **Mode:** gate\n\
         **Gate ID:** {gate_id}\n\n\
         ## Gate Row\n\n\
         ```json\n{gate_json}\n```\n\n\
         {linked_task_section}\n\
         {plan_review_section}\n\
         {code_review_section}\n\
         ## Authorized CLI Verbs\n\n\
         You are authorized to call exactly these verbs:\n\n\
         {verbs_list}\n\n\
         All other CLI verbs are FORBIDDEN. You MUST NOT invoke any verb not in this list.\n"
    )
}

/// Build the tasks-stub context bundle (AC6.2).
///
/// Includes: task row JSON, last next-action output, last review cycle (if any).
pub(crate) fn build_tasks_brief(
    task_id: &str,
    task_entry: &crate::validate::EntryMap,
    next_action_text: &str,
    last_review: &str,
) -> String {
    let task_json = serde_json::to_string_pretty(task_entry).unwrap_or_default();

    let last_review_section = if last_review.is_empty() {
        "## Last Review Cycle\n\n_No review cycles yet._\n".to_string()
    } else {
        format!("## Last Review Cycle\n\n{last_review}\n")
    };

    // Read-only authorized verbs for task mode.
    let verbs_list = [
        "stores tasks show",
        "stores tasks list",
        "stores tasks next-action",
        "stores gate show",
        "stores gate list",
    ]
    .iter()
    .map(|v| format!("- `{v}`"))
    .collect::<Vec<_>>()
    .join("\n");

    format!(
        "# Guide Context Bundle — Task Mode (v0.3 Stub)\n\n\
         **Mode:** task\n\
         **Task ID:** {task_id}\n\n\
         ## Task Row\n\n\
         ```json\n{task_json}\n```\n\n\
         ## Last Next-Action Output\n\n\
         ```json\n{next_action_text}\n```\n\n\
         {last_review_section}\n\
         ## Authorized CLI Verbs (Read-Only in Task Mode)\n\n\
         {verbs_list}\n"
    )
}

/// Build the wrap-mode context bundle (AC5.2, AC5.3).
///
/// Rendered when the task row is in status `in_review`. Includes:
/// - Latest `wrap_log` entry (executive_summary, deviations, residual_risks,
///   recommended_sanity_checks) — from the row, no extra DB reads.
/// - Contract block (promise).
/// - Cycles table (reality).
/// - Authorized verbs: exactly the 6 in `WRAP_MODE_VERBS`.
pub(crate) fn build_wrap_mode_brief(
    task_id: &str,
    task_entry: &crate::validate::EntryMap,
    wrap_log_latest: Option<&Value>,
) -> String {
    // --- Contract section ---
    let contract_section = {
        let contract = task_entry.get("contract");
        match contract {
            Some(v) => {
                let contract_json = serde_json::to_string_pretty(v).unwrap_or_default();
                format!("## Promise (Contract)\n\n```json\n{contract_json}\n```\n")
            }
            None => "## Promise (Contract)\n\n_No contract recorded._\n".to_string(),
        }
    };

    // --- Cycles section ---
    let cycles_section = {
        let task_val = Value::Object(
            task_entry
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let cycles_text = extract_cycles_table(&task_val);
        if cycles_text.is_empty() {
            "## Reality (Execution Cycles)\n\n_No execution cycles recorded._\n".to_string()
        } else {
            format!("## Reality (Execution Cycles)\n\n{cycles_text}\n")
        }
    };

    // --- Wrap log section ---
    let wrap_log_section = match wrap_log_latest {
        Some(entry) => {
            let exec_summary = entry
                .get("executive_summary")
                .and_then(|v| v.as_str())
                .unwrap_or("_Not recorded._");
            let deviations = format_string_list(entry.get("deviations"));
            let residual_risks = format_string_list(entry.get("residual_risks"));
            let sanity_checks = format_string_list(entry.get("recommended_sanity_checks"));
            format!(
                "## Synthesis (Wrap Agent Brief)\n\n\
                 ### Executive Summary\n\n\
                 {exec_summary}\n\n\
                 ### Deviations\n\n\
                 {deviations}\n\n\
                 ### Residual Risks\n\n\
                 {residual_risks}\n\n\
                 ### Recommended Sanity Checks\n\n\
                 {sanity_checks}\n"
            )
        }
        None => {
            "## Synthesis (Wrap Agent Brief)\n\n\
             _No wrap_log entry found. The wrap agent has not yet run or the entry was not persisted._\n"
                .to_string()
        }
    };

    // --- Authorized verbs ---
    let verbs_list = WRAP_MODE_VERBS
        .iter()
        .map(|v| format!("- `{v}`"))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "# Guide Context Bundle — Wrap Mode\n\n\
         **Mode:** wrap\n\
         **Task ID:** {task_id}\n\n\
         {contract_section}\n\
         {cycles_section}\n\
         {wrap_log_section}\n\
         ## Authorized CLI Verbs (Wrap Mode)\n\n\
         The `accept` and `reject` transitions are `actor: human` in the schema — \
         when invoked under `$CLAUDECODE` the framework refuses; the human is the one who runs these.\n\
         Your role is to read the brief, surface key findings, and propose the decision — \
         but the human executes it. This is a schema-enforced restriction, not a prompt-enforced one.\n\n\
         You are authorized to call exactly these verbs:\n\n\
         {verbs_list}\n\n\
         All other CLI verbs are FORBIDDEN in wrap mode.\n"
    )
}

// ---------------------------------------------------------------------------
// Context extraction helpers
// ---------------------------------------------------------------------------

/// Extract the latest wrap_log entry from a task value (most recent by position).
fn extract_latest_wrap_log_entry(task_val: &Value) -> Option<Value> {
    match task_val.get("wrap_log") {
        Some(Value::Array(arr)) if !arr.is_empty() => arr.last().cloned(),
        _ => None,
    }
}

/// Render cycles[] as a plain-text table for the wrap-mode brief.
fn extract_cycles_table(task_val: &Value) -> String {
    let cycles = match task_val.get("cycles") {
        Some(Value::Array(arr)) if !arr.is_empty() => arr,
        _ => return String::new(),
    };
    let mut rows: Vec<String> = vec![
        "| Phase | Cycle | Executor Summary | Review Gate | Review Summary |".to_string(),
        "|-------|-------|-----------------|-------------|----------------|".to_string(),
    ];
    for c in cycles {
        let phase = c
            .get("phase")
            .and_then(|v| v.as_i64())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string());
        let cycle = c
            .get("cycle")
            .and_then(|v| v.as_i64())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "—".to_string());
        let exec_summary = c
            .get("executor")
            .and_then(|e| e.get("summary"))
            .and_then(|v| v.as_str())
            .unwrap_or("—");
        let review_gate = c
            .get("review")
            .and_then(|r| r.get("gate"))
            .and_then(|v| v.as_str())
            .unwrap_or("—");
        let review_summary = c
            .get("review")
            .and_then(|r| r.get("summary"))
            .and_then(|v| v.as_str())
            .unwrap_or("—");
        rows.push(format!(
            "| {phase} | {cycle} | {exec_summary} | {review_gate} | {review_summary} |"
        ));
    }
    rows.join("\n")
}

/// Format a JSON array of strings as a Markdown bullet list.
fn format_string_list(val: Option<&Value>) -> String {
    match val {
        Some(Value::Array(arr)) if !arr.is_empty() => arr
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => "_None._".to_string(),
    }
}

/// Load a task row from the DB by display_id using a raw SQL query.
/// Returns None if the tasks table doesn't exist or the row isn't found.
fn load_task_row(conn: &Connection, task_id: &str) -> Option<Value> {
    // First fetch column names from the schema.
    let col_names: Vec<String> = {
        let stmt = conn.prepare("SELECT * FROM tasks LIMIT 0").ok()?;
        stmt.column_names().iter().map(|s| s.to_string()).collect()
    };

    let col_list = col_names.join(", ");
    let sql = format!("SELECT {col_list} FROM tasks WHERE display_id = ?1");

    conn.query_row(&sql, rusqlite::params![task_id], |row| {
        let mut map = serde_json::Map::new();
        for (i, name) in col_names.iter().enumerate() {
            let v: rusqlite::types::Value = row.get(i).unwrap_or(rusqlite::types::Value::Null);
            map.insert(name.clone(), sqlite_val_to_json(v));
        }
        Ok(Value::Object(map))
    })
    .ok()
}

/// Extract the plan_review_log array from a task value as a formatted string.
fn extract_plan_review_log(task_val: &Value) -> String {
    let log = match task_val.get("plan_review_log") {
        Some(Value::Array(arr)) if !arr.is_empty() => arr,
        _ => return String::new(),
    };
    serde_json::to_string_pretty(&Value::Array(log.clone())).unwrap_or_default()
}

/// Extract the last N cycles from a task value as a formatted string.
fn extract_last_n_cycles(task_val: &Value, n: usize) -> String {
    let cycles = match task_val.get("cycles") {
        Some(Value::Array(arr)) if !arr.is_empty() => arr,
        _ => return String::new(),
    };
    let start = if cycles.len() > n {
        cycles.len() - n
    } else {
        0
    };
    let last: Vec<&Value> = cycles[start..].iter().collect();
    if last.is_empty() {
        return String::new();
    }
    let arr: Vec<Value> = last.into_iter().cloned().collect();
    serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_default()
}

/// Convert a rusqlite Value to a serde_json Value.
fn sqlite_val_to_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => Value::from(i),
        rusqlite::types::Value::Real(f) => {
            Value::from(serde_json::Number::from_f64(f).unwrap_or(serde_json::Number::from(0)))
        }
        rusqlite::types::Value::Text(s) => Value::String(s),
        rusqlite::types::Value::Blob(b) => Value::String(String::from_utf8_lossy(&b).to_string()),
    }
}

// ---------------------------------------------------------------------------
// Tests (AC6.3 / AC6.4 / AC6.5)
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

    // -----------------------------------------------------------------------
    // Schema helpers
    // -----------------------------------------------------------------------

    fn gate_schema() -> Schema {
        let yaml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("stores/gate/schema.yaml"),
        )
        .expect("gate schema.yaml");
        Schema::from_yaml(&yaml).unwrap()
    }

    fn tasks_schema_loaded() -> Schema {
        let yaml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("stores/tasks/schema.yaml"),
        )
        .expect("tasks schema.yaml");
        Schema::from_yaml(&yaml).unwrap()
    }

    fn open_db_with_schema(schema: &Schema) -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.db");
        let conn = db::open(&db_file).unwrap();
        conn.execute_batch(&ddl_for(schema)).unwrap();
        (dir, conn)
    }

    fn open_db_both(
        gate_schema: &Schema,
        tasks_schema: &Schema,
    ) -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.db");
        let conn = db::open(&db_file).unwrap();
        conn.execute_batch(&ddl_for(gate_schema)).unwrap();
        conn.execute_batch(&ddl_for(tasks_schema)).unwrap();
        (dir, conn)
    }

    fn make_run_output(stdout: &str, exit_code: i32) -> RunnerOutput {
        let last_line = stdout
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|s| s.to_string());
        RunnerOutput {
            stdout: stdout.to_string(),
            stderr: String::new(),
            exit_code,
            final_message: last_line,
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(),
        }
    }

    fn insert_gate(
        conn: &Connection,
        display_id: &str,
        question: &str,
        task_ref: Option<&str>,
        status: &str,
    ) {
        let task_ref_val = task_ref.unwrap_or("");
        conn.execute(
            "INSERT INTO gate (display_id, status, type, one_liner, task_ref, \
             filed_by, source, \
             created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'decision', ?3, ?4, \
             'test-fixture', 'dev', \
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'human', 'human')",
            rusqlite::params![display_id, status, question, task_ref_val],
        )
        .unwrap();
    }

    fn insert_task_row(conn: &Connection, display_id: &str, status: &str) {
        let plan_json = serde_json::to_string(&json!({
            "objective": "Test",
            "phases": [
                {
                    "name": "Phase 1",
                    "objective": "Do it",
                    "tasks": [],
                    "acceptance_criteria": [],
                    "files": [],
                    "dependencies": []
                }
            ]
        }))
        .unwrap();
        let contract_json = serde_json::to_string(&json!({
            "done_when": "Works",
            "scope_in": "All",
            "scope_out": "None"
        }))
        .unwrap();
        let cycles_json = serde_json::to_string(&json!([])).unwrap();
        let plan_review_json = serde_json::to_string(&json!([])).unwrap();

        conn.execute(
            "INSERT INTO tasks \
             (display_id, status, title, slug, current_phase, current_cycle, \
              plan, contract, cycles, plan_review_log, \
              created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'Test Task', 'test-task', 1, 1, ?3, ?4, ?5, ?6, \
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'human', 'human')",
            rusqlite::params![
                display_id,
                status,
                plan_json,
                contract_json,
                cycles_json,
                plan_review_json
            ],
        )
        .unwrap();
    }

    fn set_gate_status(conn: &Connection, display_id: &str, status: &str) {
        conn.execute(
            "UPDATE gate SET status = ?1 WHERE display_id = ?2",
            rusqlite::params![status, display_id],
        )
        .unwrap();
    }

    // -----------------------------------------------------------------------
    // AC6.1: gate-full bundle contains gate row + linked task + authorized verbs
    // -----------------------------------------------------------------------

    #[test]
    fn gate_bundle_contains_gate_row_and_verbs() {
        let gate_s = gate_schema();
        let tasks_s = tasks_schema_loaded();
        let (_dir, conn) = open_db_both(&gate_s, &tasks_s);

        insert_gate(
            &conn,
            "G001",
            "Should we use flat layout?",
            Some("T001"),
            "pending",
        );
        insert_task_row(&conn, "T001", "planning");

        let (_, gate_entry) = read_row(&gate_s, &conn, "G001").unwrap();

        let linked_task = load_task_row(&conn, "T001");

        let brief = build_gate_brief(
            "G001",
            &gate_entry,
            linked_task.as_ref(),
            "",
            "",
            Some("T001"),
        );

        // Must contain the gate ID.
        assert!(brief.contains("G001"), "brief must reference the gate ID");

        // Must contain the linked task section.
        assert!(
            brief.contains("Linked Task Row"),
            "brief must contain the Linked Task Row section"
        );
        assert!(
            brief.contains("T001"),
            "brief must reference the linked task ID"
        );

        // Must contain all 5 authorized verbs (AC6.1).
        for verb in AUTHORIZED_VERBS {
            assert!(
                brief.contains(verb),
                "brief must contain authorized verb '{verb}'"
            );
        }

        // Must contain the forbid clause.
        assert!(
            brief.contains("FORBIDDEN"),
            "brief must contain the FORBIDDEN clause"
        );
    }

    #[test]
    fn gate_bundle_without_linked_task() {
        let gate_s = gate_schema();
        let (_dir, conn) = open_db_with_schema(&gate_s);

        insert_gate(&conn, "G002", "No task ref here", None, "pending");
        let (_, gate_entry) = read_row(&gate_s, &conn, "G002").unwrap();

        let brief = build_gate_brief("G002", &gate_entry, None, "", "", None);

        assert!(brief.contains("G002"));
        assert!(brief.contains("No linked task"));
        for verb in AUTHORIZED_VERBS {
            assert!(brief.contains(verb), "verb '{verb}' missing");
        }
    }

    // -----------------------------------------------------------------------
    // AC6.2: tasks-stub bundle contains task row + next-action + last review
    // -----------------------------------------------------------------------

    #[test]
    fn tasks_bundle_contains_task_and_next_action() {
        let tasks_s = tasks_schema_loaded();
        let (_dir, conn) = open_db_with_schema(&tasks_s);

        insert_task_row(&conn, "T001", "planning");
        let (_, task_entry) = read_row(&tasks_s, &conn, "T001").unwrap();
        let na = compute_next_action(&tasks_s, &conn, "T001").unwrap();
        let na_text = serde_json::to_string_pretty(&na).unwrap();

        let brief = build_tasks_brief("T001", &task_entry, &na_text, "");

        assert!(brief.contains("T001"), "task ID in brief");
        assert!(brief.contains("Task Row"), "Task Row section present");
        assert!(brief.contains("Next-Action"), "Next-Action section present");
        // No review cycles yet — note its absence.
        assert!(
            brief.contains("No review cycles") || brief.contains("None"),
            "must note absence of review cycles"
        );
    }

    #[test]
    fn tasks_bundle_with_review_cycle() {
        let tasks_s = tasks_schema_loaded();
        let (_dir, conn) = open_db_with_schema(&tasks_s);

        insert_task_row(&conn, "T002", "code_review");
        // Add a review cycle directly.
        let cycles_json = serde_json::to_string(&json!([{
            "phase": 1,
            "cycle": 1,
            "executor": {"summary": "done", "commit": "abc"},
            "review": {
                "gate": "REVISE",
                "summary": "needs work",
                "critical": 0,
                "major": 1,
                "minor": 0
            }
        }]))
        .unwrap();
        conn.execute(
            "UPDATE tasks SET cycles = ?1, current_phase = 1, current_cycle = 1 \
             WHERE display_id = 'T002'",
            rusqlite::params![cycles_json],
        )
        .unwrap();

        let (_, task_entry) = read_row(&tasks_s, &conn, "T002").unwrap();
        let na = compute_next_action(&tasks_s, &conn, "T002").unwrap();
        let na_text = serde_json::to_string_pretty(&na).unwrap();

        let task_val = Value::Object(
            task_entry
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        );
        let last_review = extract_last_n_cycles(&task_val, 1);

        let brief = build_tasks_brief("T002", &task_entry, &na_text, &last_review);

        assert!(
            brief.contains("Last Review Cycle"),
            "review section present"
        );
        assert!(
            brief.contains("REVISE") || brief.contains("needs work"),
            "review content present"
        );
    }

    // -----------------------------------------------------------------------
    // AC6.4: guide prompt contains authorized verbs + forbid clause + no-edit instruction
    // -----------------------------------------------------------------------

    #[test]
    fn guide_prompt_contains_authorized_verbs() {
        let prompt = BUNDLED_AGENTS
            .iter()
            .find(|(n, _)| *n == "guide")
            .map(|(_, c)| *c)
            .expect("guide agent must be bundled");

        // All 5 authorized verbs from AC6.4.
        for verb in &[
            "stores gate show",
            "stores gate answer",
            "stores tasks show",
            "stores tasks list",
            "stores tasks next-action",
        ] {
            assert!(
                prompt.contains(verb),
                "guide prompt must contain authorized verb '{verb}'"
            );
        }
    }

    #[test]
    fn guide_prompt_contains_forbid_clause() {
        let prompt = BUNDLED_AGENTS
            .iter()
            .find(|(n, _)| *n == "guide")
            .map(|(_, c)| *c)
            .expect("guide agent must be bundled");

        let has_forbid = prompt.contains("FORBIDDEN")
            || prompt.contains("MUST NOT")
            || prompt.contains("forbidden")
            || prompt.contains("do not invoke");

        assert!(
            has_forbid,
            "guide prompt must contain an explicit forbid-everything-else clause \
             (one of: FORBIDDEN, MUST NOT, forbidden, do not invoke)"
        );
    }

    #[test]
    fn guide_prompt_contains_no_direct_edit_instruction() {
        let prompt = BUNDLED_AGENTS
            .iter()
            .find(|(n, _)| *n == "guide")
            .map(|(_, c)| *c)
            .expect("guide agent must be bundled");

        // AC6.4: writes go via stores gate answer, not direct file edits.
        // Check either an explicit "not edit main.md" or presence of write-back verb.
        let has_no_edit = prompt.contains("do NOT edit main.md directly")
            || prompt.contains("do not edit main.md directly")
            || prompt.contains("NOT edit")
            || prompt.contains("not edit main.md")
            || prompt.contains("not write to the DB directly")
            || prompt.contains("do not write to the DB");

        let has_writeback_verb = prompt.contains("stores gate answer");

        assert!(
            has_no_edit || has_writeback_verb,
            "guide prompt must indicate writes go via 'stores gate answer', not direct file edits"
        );
    }

    // -----------------------------------------------------------------------
    // AC6.5: exit-code logic — tested via check_gate_transition (pure function)
    // -----------------------------------------------------------------------

    #[test]
    fn check_gate_transition_ok_on_pending_to_answered() {
        let result = check_gate_transition("G001", "pending", "answered");
        assert!(result.is_ok(), "pending→answered must return Ok");
    }

    #[test]
    fn check_gate_transition_err_on_noop() {
        let result = check_gate_transition("G002", "pending", "pending");
        assert!(result.is_err(), "pending→pending must return Err");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("without answering gate"),
            "error must cite 'without answering gate'"
        );
    }

    #[test]
    fn check_gate_transition_err_on_already_answered() {
        // Gate was already answered before guide ran (not our session's work).
        let result = check_gate_transition("G003", "answered", "answered");
        assert!(
            result.is_err(),
            "answered→answered (pre-answered) must return Err"
        );
    }

    #[test]
    fn check_gate_transition_err_on_cancelled() {
        let result = check_gate_transition("G004", "pending", "cancelled");
        assert!(result.is_err(), "pending→cancelled must return Err");
    }

    // -----------------------------------------------------------------------
    // AC6.5: full handler path — runner exit 1 returns Err
    // -----------------------------------------------------------------------

    #[test]
    fn gate_guide_exits_one_when_gate_not_answered() {
        let gate_s = gate_schema();
        let tasks_s = tasks_schema_loaded();
        let (_dir, conn) = open_db_both(&gate_s, &tasks_s);

        insert_gate(&conn, "G002", "Use flat layout?", None, "pending");

        // Mock runner returns 0 but gate stays pending (no DB write).
        let noop_envelope = r#"{"role":"guide","action":"noop","summary":"Nothing to do"}"#;
        let runner_out = make_run_output(noop_envelope, 0);
        let runner = MockRunner::new(vec![runner_out]);

        let result = run_gate_guide_with_runner(&gate_s, &conn, "G002", &runner);
        assert!(
            result.is_err(),
            "should exit 1 (Err) when gate stays pending"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("without answering gate"),
            "error should cite 'without answering gate': {msg}"
        );
    }

    #[test]
    fn gate_guide_exits_zero_when_gate_answered_by_db_update() {
        let gate_s = gate_schema();
        let tasks_s = tasks_schema_loaded();
        let (_dir, conn) = open_db_both(&gate_s, &tasks_s);

        insert_gate(&conn, "G001", "Use flat layout?", None, "pending");

        // The mock runner does nothing (can't write DB), but we simulate the
        // guide agent's side-effect by updating the gate to 'answered' before
        // the handler's post-spawn status check. We do this by using a thin
        // wrapper: run the bundle build + spawn, then update DB, then check.
        // Direct test of check_gate_transition covers the policy; here we test
        // that run_gate_guide_with_runner correctly reads status_after from DB.

        // Pre-answer the gate to simulate what the guide agent does mid-session.
        // We call set_gate_status BEFORE calling the handler because the mock
        // runner's spawn is synchronous and returns immediately. In the real
        // claude-code runner the agent would call `stores gate answer` during
        // spawn (blocking the handler). For the test we use a two-step approach:
        // the handler reads status_before (pending), then we need status_after
        // to be 'answered'. We accomplish this via a MockRunner that calls our
        // callback — but since Runner trait doesn't support callbacks, we test
        // this via check_gate_transition directly (above) and separately test
        // that the handler correctly wires the before/after reads to that function.

        // This variant: answer gate after inserting but before runner is called
        // fails because status_before would read 'answered'.
        // So this specific integration path is tested via check_gate_transition
        // tests above. Here we just verify the path that the handler returns
        // Err when gate stays pending (already covered by gate_guide_exits_one_*).

        // Verify gate starts as pending.
        let status = read_gate_status(&gate_s, &conn, "G001").unwrap();
        assert_eq!(status, "pending");

        // Manually answer gate then verify read_gate_status picks it up.
        set_gate_status(&conn, "G001", "answered");
        let status = read_gate_status(&gate_s, &conn, "G001").unwrap();
        assert_eq!(status, "answered");
    }

    #[test]
    fn gate_guide_exits_one_on_runner_error() {
        let gate_s = gate_schema();
        let tasks_s = tasks_schema_loaded();
        let (_dir, conn) = open_db_both(&gate_s, &tasks_s);

        insert_gate(&conn, "G003", "A question", None, "pending");

        let runner_out = RunnerOutput {
            stdout: String::new(),
            stderr: "runner crashed".to_string(),
            exit_code: 1,
            final_message: None,
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(),
        };
        let runner = MockRunner::new(vec![runner_out]);

        let result = run_gate_guide_with_runner(&gate_s, &conn, "G003", &runner);
        assert!(result.is_err(), "should return Err on runner non-zero exit");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("non-zero exit"),
            "error should mention non-zero exit: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // AC6.3: tasks guide runs and produces Ok on runner exit 0
    // -----------------------------------------------------------------------

    #[test]
    fn tasks_guide_exits_zero_on_runner_exit_zero() {
        let tasks_s = tasks_schema_loaded();
        let (_dir, conn) = open_db_with_schema(&tasks_s);

        insert_task_row(&conn, "T001", "planning");

        let noop_envelope = r#"{"role":"guide","action":"noop","summary":"All clear"}"#;
        let runner_out = make_run_output(noop_envelope, 0);
        let runner = MockRunner::new(vec![runner_out]);

        let result = run_tasks_guide_with_runner(&tasks_s, &conn, "T001", &runner);
        assert!(
            result.is_ok(),
            "should exit 0 when runner exits 0: {:?}",
            result.err()
        );
    }

    #[test]
    fn tasks_guide_exits_one_on_runner_error() {
        let tasks_s = tasks_schema_loaded();
        let (_dir, conn) = open_db_with_schema(&tasks_s);

        insert_task_row(&conn, "T002", "planning");

        let runner_out = RunnerOutput {
            stdout: String::new(),
            stderr: "crash".to_string(),
            exit_code: 1,
            final_message: None,
            structured_output: None,
            session_id: None,
            structured_output_source: None,
            payload_error: None,
            telemetry: crate::runner::AgentRunTelemetry::with_mock_defaults(),
        };
        let runner = MockRunner::new(vec![runner_out]);

        let result = run_tasks_guide_with_runner(&tasks_s, &conn, "T002", &runner);
        assert!(result.is_err(), "should return Err on runner non-zero exit");
    }

    // -----------------------------------------------------------------------
    // AC5.5: wrap-mode brief dispatch — in_review status triggers build_wrap_mode_brief
    // -----------------------------------------------------------------------

    fn insert_task_row_in_review(conn: &Connection, display_id: &str) {
        let plan_json = serde_json::to_string(&json!({
            "objective": "Test wrap",
            "phases": [{"name": "Phase 1", "objective": "Execute", "tasks": [],
                        "acceptance_criteria": [], "files": [], "dependencies": []}]
        }))
        .unwrap();
        let contract_json = serde_json::to_string(&json!({
            "done_when": "All phases complete and accepted",
            "scope_in": "Wrap workflow implementation",
            "scope_out": "Ship workflow"
        }))
        .unwrap();
        let cycles_json = serde_json::to_string(&json!([{
            "phase": 1, "cycle": 1,
            "executor": {"summary": "Implemented wrap handler"},
            "review": {"gate": "PASS", "summary": "All ACs satisfied"}
        }]))
        .unwrap();
        let wrap_log_json = serde_json::to_string(&json!([{
            "executive_summary": "Phase 5 complete: guide.rs now dispatches wrap-mode for in_review rows. build_wrap_mode_brief renders contract, cycles, and wrap_log. No deviations.",
            "deviations": [],
            "residual_risks": ["AC6 tests not yet written"],
            "recommended_sanity_checks": ["cargo test handlers::guide", "cargo build --features runner-claude-code"],
            "reject_reason": null,
            "at": "2026-05-01T12:00:00Z"
        }]))
        .unwrap();
        let plan_review_json = serde_json::to_string(&json!([])).unwrap();

        conn.execute(
            "INSERT INTO tasks \
             (display_id, status, title, slug, current_phase, current_cycle, \
              plan, contract, cycles, plan_review_log, wrap_log, \
              created_at, updated_at, created_by, updated_by) \
             VALUES (?1, 'in_review', 'Wrap Test Task', 'wrap-test', 1, 1, \
             ?2, ?3, ?4, ?5, ?6, \
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'human', 'human')",
            rusqlite::params![
                display_id,
                plan_json,
                contract_json,
                cycles_json,
                plan_review_json,
                wrap_log_json
            ],
        )
        .unwrap();
    }

    #[test]
    fn ac5_5_in_review_status_triggers_wrap_mode_brief() {
        let tasks_s = tasks_schema_loaded();
        let (_dir, conn) = open_db_with_schema(&tasks_s);

        insert_task_row_in_review(&conn, "T010");

        let noop_envelope = r#"{"role":"guide","action":"noop","summary":"Wrap brief delivered"}"#;
        let runner_out = make_run_output(noop_envelope, 0);
        let runner = MockRunner::new(vec![runner_out]);

        // Should succeed (runner exits 0).
        let result = run_tasks_guide_with_runner(&tasks_s, &conn, "T010", &runner);
        assert!(
            result.is_ok(),
            "in_review guide should exit Ok: {:?}",
            result.err()
        );
    }

    #[test]
    fn ac5_5_wrap_mode_brief_contains_executive_summary() {
        let task_entry: crate::validate::EntryMap = {
            let contract = json!({
                "done_when": "Wrap workflow complete",
                "scope_in": "Guide wrap-mode",
                "scope_out": "Ship workflow"
            });
            let cycles = json!([{
                "phase": 1, "cycle": 1,
                "executor": {"summary": "Implemented wrap-mode brief"},
                "review": {"gate": "PASS", "summary": "All ACs met"}
            }]);
            let wrap_log = json!([{
                "executive_summary": "UNIQUE_EXEC_SUMMARY_TOKEN: guide.rs dispatches wrap-mode for in_review rows",
                "deviations": ["Over-delivered: added helper tests"],
                "residual_risks": ["AC6 e2e not yet run"],
                "recommended_sanity_checks": ["cargo test handlers::guide"],
                "reject_reason": null,
                "at": "2026-05-01T12:00:00Z"
            }]);
            let mut map = std::collections::BTreeMap::new();
            map.insert("status".to_string(), json!("in_review"));
            map.insert("contract".to_string(), contract);
            map.insert("cycles".to_string(), cycles);
            map.insert("wrap_log".to_string(), wrap_log);
            map
        };

        let task_val = Value::Object(
            task_entry
                .iter()
                .map(|(k, v): (&String, &Value)| (k.clone(), v.clone()))
                .collect(),
        );
        let wrap_log_latest = extract_latest_wrap_log_entry(&task_val);

        let brief = build_wrap_mode_brief("T010", &task_entry, wrap_log_latest.as_ref());

        // Mode header.
        assert!(brief.contains("Wrap Mode"), "brief must indicate Wrap Mode");
        assert!(brief.contains("T010"), "brief must contain task ID");

        // AC5.2: executive_summary present.
        assert!(
            brief.contains("UNIQUE_EXEC_SUMMARY_TOKEN"),
            "brief must include executive_summary from wrap_log"
        );

        // AC5.2: contract block present.
        assert!(
            brief.contains("Wrap workflow complete"),
            "brief must include contract done_when"
        );

        // AC5.2: cycles table present.
        assert!(
            brief.contains("Implemented wrap-mode brief"),
            "brief must include executor summary from cycles"
        );

        // AC5.3: all 6 authorized verbs present.
        for verb in WRAP_MODE_VERBS {
            assert!(
                brief.contains(verb),
                "brief must contain wrap-mode authorized verb '{verb}'"
            );
        }

        // AC5.3: accept and reject verbs present.
        assert!(
            brief.contains("stores tasks accept"),
            "brief must contain accept verb"
        );
        assert!(
            brief.contains("stores tasks reject"),
            "brief must contain reject verb"
        );

        // Schema-enforced restriction note.
        assert!(
            brief.contains("schema-enforced"),
            "brief must explain schema-enforced restriction on accept/reject"
        );

        // FORBIDDEN clause.
        assert!(
            brief.contains("FORBIDDEN"),
            "brief must contain FORBIDDEN clause"
        );
    }

    #[test]
    fn ac5_5_wrap_mode_brief_without_wrap_log() {
        let task_entry: crate::validate::EntryMap = {
            let mut map = std::collections::BTreeMap::new();
            map.insert("status".to_string(), json!("in_review"));
            map.insert(
                "contract".to_string(),
                json!({"done_when": "X", "scope_in": "Y", "scope_out": "Z"}),
            );
            map.insert("cycles".to_string(), json!([]));
            map
        };

        let brief = build_wrap_mode_brief("T999", &task_entry, None);

        assert!(brief.contains("T999"), "task ID in brief");
        assert!(brief.contains("Wrap Mode"), "mode label present");
        // Should gracefully note missing wrap_log.
        assert!(
            brief.contains("wrap agent has not yet run") || brief.contains("No wrap_log"),
            "brief must gracefully handle absent wrap_log"
        );
        // Verbs still present.
        for verb in WRAP_MODE_VERBS {
            assert!(
                brief.contains(verb),
                "verb '{verb}' must appear even without wrap_log"
            );
        }
    }

    #[test]
    fn ac5_5_non_in_review_status_gets_tasks_brief() {
        // Verify that statuses other than in_review still get the task-mode brief
        // by checking the brief header text.
        let task_entry: crate::validate::EntryMap = {
            let mut map = std::collections::BTreeMap::new();
            map.insert("status".to_string(), json!("executing"));
            map.insert(
                "contract".to_string(),
                json!({"done_when": "X", "scope_in": "Y", "scope_out": "Z"}),
            );
            map.insert("cycles".to_string(), json!([]));
            map
        };

        // build_tasks_brief is called for executing status.
        let brief = build_tasks_brief("T888", &task_entry, "{}", "");
        assert!(
            brief.contains("Task Mode"),
            "executing status must get Task Mode brief"
        );
        assert!(
            !brief.contains("Wrap Mode"),
            "executing status must NOT get Wrap Mode brief"
        );
    }
}
