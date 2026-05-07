//! First-class deterministic Check primitive.
//!
//! Initial registry slice: code-level checks only, no schema/table/CLI surface.

use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DRIVE_PID_RECORDED_OR_TERMINAL: &str = "drive_pid_recorded_or_terminal";
pub const GATEKEEPER_DECISION_VALID: &str = "gatekeeper-decision-valid";

#[derive(Debug, Clone, Copy)]
pub struct CheckCtx<'a> {
    pub conn: Option<&'a Connection>,
    pub companion: Option<&'a Value>,
}

impl<'a> CheckCtx<'a> {
    pub fn with_conn(conn: &'a Connection) -> Self {
        Self {
            conn: Some(conn),
            companion: None,
        }
    }

    pub fn without_conn() -> Self {
        Self {
            conn: None,
            companion: None,
        }
    }
}

/// Outcome of a Check evaluation.
///
/// ## Serialization shape (intentional flat design)
///
/// `CheckOutcome` serializes as a lowercase string (`"pass"` or `"fail"`).  The associated
/// failure reason lives in the sibling `reason` field on [`CheckResult`].  This is a deliberate
/// choice: the flat shape keeps `CheckResult` JSON human-readable and easy to grep/match
/// without needing to unwrap a nested variant.
///
/// Audit consumers should match on `outcome == "fail"` and read the sibling `reason` value.
/// They do **not** need to special-case `reason: null` for the pass branch — a `CheckResult`
/// with `outcome == "pass"` is guaranteed to have `reason: null`.
///
/// Contract shape from L135: `outcome: Pass | Fail(reason)` — reason is carried in the flat
/// sibling `reason` field rather than embedded in the variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckOutcome {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub outcome: CheckOutcome,
    pub check_id: String,
    pub args: Value,
    pub observed_at: String,
    pub reason: Option<Value>,
}

impl CheckResult {
    pub fn pass(check_id: &str, args: &Value) -> Self {
        Self {
            outcome: CheckOutcome::Pass,
            check_id: check_id.to_string(),
            args: args.clone(),
            observed_at: crate::handlers::row::now_iso8601(),
            reason: None,
        }
    }

    pub fn fail(check_id: &str, args: &Value, reason: Value) -> Self {
        Self {
            outcome: CheckOutcome::Fail,
            check_id: check_id.to_string(),
            args: args.clone(),
            observed_at: crate::handlers::row::now_iso8601(),
            reason: Some(reason),
        }
    }

    pub fn is_pass(&self) -> bool {
        self.outcome == CheckOutcome::Pass
    }
}

pub trait Check: Sync {
    fn id(&self) -> &'static str;
    fn evaluate(&self, ctx: CheckCtx<'_>, args: &Value) -> Result<CheckResult>;
}

struct DrivePidRecordedOrTerminal;
struct GatekeeperDecisionValid;

static DRIVE_PID_CHECK: DrivePidRecordedOrTerminal = DrivePidRecordedOrTerminal;
static GATEKEEPER_DECISION_CHECK: GatekeeperDecisionValid = GatekeeperDecisionValid;

static REGISTRY: &[&dyn Check] = &[&DRIVE_PID_CHECK, &GATEKEEPER_DECISION_CHECK];

pub fn registry() -> &'static [&'static dyn Check] {
    REGISTRY
}

pub fn lookup(id: &str) -> Option<&'static dyn Check> {
    REGISTRY.iter().copied().find(|check| check.id() == id)
}

pub fn evaluate(id: &str, ctx: CheckCtx<'_>, args: &Value) -> Result<Option<CheckResult>> {
    lookup(id)
        .map(|check| check.evaluate(ctx, args))
        .transpose()
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

fn auto_drive_terminal_ok(conn: &Connection, display_id: &str) -> Result<bool> {
    let schema = crate::flow::builtins::load_tasks_schema()?;
    let out = match crate::handlers::next_action::compute(&schema, conn, display_id) {
        Ok(out) => out,
        Err(_) => {
            let status: Option<String> = conn
                .query_row(
                    "SELECT status FROM tasks WHERE display_id = ?1",
                    rusqlite::params![display_id],
                    |r| r.get(0),
                )
                .ok();
            return Ok(status.as_deref().is_some_and(|s| {
                matches!(
                    s,
                    "accepted"
                        | "rejected"
                        | "blocked"
                        | "deploy_blocked"
                        | "cargo_installed"
                        | "schema_migrated"
                        | "closed_out_of_band"
                        | "abandoned"
                )
            }));
        }
    };
    // A1-strict (pi ruling): next_agent IS NULL is the sole terminal-ok signal.
    // wrap_log is durable history and MUST NOT be consulted for control flow.
    //
    // next_agent IS NULL: no agent work pending — covers hard-terminal states
    // (accepted, rejected, abandoned, …) and any state whose schema dispatch_agent
    // conditions are all false.
    //
    // next_agent IS NOT NULL: work still pending regardless of what wrap_log
    // contains. For in_review rows the schema always yields next_agent=Some("wrap")
    // until the task transitions out of in_review; that transition (not wrap_log)
    // is the completion signal.
    Ok(out.next_agent.is_none()
        || matches!(
            out.status.as_str(),
            "accepted"
                | "rejected"
                | "blocked"
                | "deploy_blocked"
                | "cargo_installed"
                | "schema_migrated"
                | "closed_out_of_band"
                | "abandoned"
        ))
}

impl Check for DrivePidRecordedOrTerminal {
    fn id(&self) -> &'static str {
        DRIVE_PID_RECORDED_OR_TERMINAL
    }

    fn evaluate(&self, ctx: CheckCtx<'_>, args: &Value) -> Result<CheckResult> {
        let Some(conn) = ctx.conn else {
            return Ok(CheckResult::fail(
                self.id(),
                args,
                json!({"message":"missing check context: sqlite connection"}),
            ));
        };
        let Some(display_id) = arg_str(args, "display_id") else {
            return Ok(CheckResult::fail(
                self.id(),
                args,
                json!({"message":"missing arg: display_id"}),
            ));
        };
        if auto_drive_terminal_ok(conn, display_id)? {
            Ok(CheckResult::pass(self.id(), args))
        } else {
            Ok(CheckResult::fail(
                self.id(),
                args,
                json!({"message": format!(
                    "auto-drive terminal-ok requires next_agent IS NULL OR row in terminal state for task {display_id}"
                )}),
            ))
        }
    }
}

impl Check for GatekeeperDecisionValid {
    fn id(&self) -> &'static str {
        GATEKEEPER_DECISION_VALID
    }

    fn evaluate(&self, _ctx: CheckCtx<'_>, args: &Value) -> Result<CheckResult> {
        let payload = args.get("gatekeeper_decision_json").unwrap_or(args);
        match crate::validate::gatekeeper_decision::validate_gatekeeper_decision(payload) {
            Ok(()) => Ok(CheckResult::pass(self.id(), args)),
            Err(errors) => Ok(CheckResult::fail(
                self.id(),
                args,
                json!({"message":"gatekeeper_decision_json validation failed", "errors": errors}),
            )),
        }
    }
}

/// Format a Check failure into an operator-readable error string that carries BOTH:
/// - Human-readable diagnostic lines (the legacy fail-loud text operators already see).
/// - A JSON-serialized `CheckResult` payload so downstream audit consumers can parse
///   `check_id`, `args`, `observed_at`, and `reason` without string-scraping.
///
/// Output shape:
/// ```text
/// gatekeeper_decision_json validation failed:
/// - <diagnostic line 1>
/// - <diagnostic line 2>
/// [check] {"check_id":"...","outcome":"fail",...}
/// ```
pub fn format_check_failure(result: &CheckResult) -> String {
    let diagnostics = gatekeeper_failure_diagnostics(result);
    let structured = serde_json::to_string(result)
        .unwrap_or_else(|_| format!("{{\"check_id\":\"{}\"}}", result.check_id));
    format!(
        "gatekeeper_decision_json validation failed:\n- {}\n[check] {}",
        diagnostics.join("\n- "),
        structured,
    )
}

pub fn gatekeeper_failure_diagnostics(result: &CheckResult) -> Vec<String> {
    result
        .reason
        .as_ref()
        .and_then(|r| r.get("errors"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| {
            result
                .reason
                .as_ref()
                .map(|v| vec![v.to_string()])
                .unwrap_or_else(|| vec!["check failed without reason".to_string()])
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
    use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
    use crate::schema::Schema;
    use serde_json::json;

    fn fresh_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .unwrap();
        let schema = Schema::from_yaml(yaml).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        conn
    }

    fn insert_task(conn: &Connection, display_id: &str, status: &str, drive_pid: Option<i64>) {
        let now = "2026-05-06T00:00:00Z";
        let contract = r#"{"done_when":"x","scope_in":"y","scope_out":"z"}"#;
        conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, branch, tier_hint, drive_pid, contract, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'test', 't', 'feat/x', 'T2', ?3, ?4, ?5, ?5, 'framework', 'framework')",
            rusqlite::params![display_id, status, drive_pid, contract, now],
        )
        .unwrap();
    }

    #[test]
    fn registry_contains_exact_initial_check_ids() {
        let ids: Vec<&str> = registry().iter().map(|c| c.id()).collect();
        assert_eq!(
            ids,
            vec![DRIVE_PID_RECORDED_OR_TERMINAL, GATEKEEPER_DECISION_VALID]
        );
        assert!(lookup(DRIVE_PID_RECORDED_OR_TERMINAL).is_some());
        assert!(lookup(GATEKEEPER_DECISION_VALID).is_some());
        assert!(lookup("task_workspace_exists").is_none());
    }

    #[test]
    fn lookup_evaluate_round_trips_id_and_args_for_both_checks() {
        let conn = fresh_db();
        insert_task(&conn, "T300", "executing", Some(123));
        let args = json!({"display_id":"T300", "store":"tasks"});
        let result = evaluate(
            DRIVE_PID_RECORDED_OR_TERMINAL,
            CheckCtx::with_conn(&conn),
            &args,
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.check_id, DRIVE_PID_RECORDED_OR_TERMINAL);
        assert_eq!(result.args, args);
        assert_eq!(result.outcome, CheckOutcome::Fail);

        let args = json!({"gatekeeper_decision_json":{
            "decision":"reject_noise", "confidence":"high", "rationale":"local noise"
        }});
        let result = evaluate(GATEKEEPER_DECISION_VALID, CheckCtx::without_conn(), &args)
            .unwrap()
            .unwrap();
        assert_eq!(result.check_id, GATEKEEPER_DECISION_VALID);
        assert_eq!(result.args, args);
        assert_eq!(result.outcome, CheckOutcome::Pass);
    }

    #[test]
    fn failing_drive_pid_check_returns_structured_result() {
        let conn = fresh_db();
        insert_task(&conn, "T302", "planning", None);
        let args = json!({"display_id":"T302"});
        let result = evaluate(
            DRIVE_PID_RECORDED_OR_TERMINAL,
            CheckCtx::with_conn(&conn),
            &args,
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.outcome, CheckOutcome::Fail);
        assert_eq!(result.check_id, DRIVE_PID_RECORDED_OR_TERMINAL);
        assert_eq!(
            result.args.get("display_id").and_then(|v| v.as_str()),
            Some("T302")
        );
        assert!(!result.observed_at.is_empty());
        assert!(result
            .reason
            .as_ref()
            .unwrap()
            .to_string()
            .contains("terminal-ok"));
    }

    /// A1-strict: in_review + next_agent IS NOT NULL → NOT terminal-ok.
    /// wrap_log content must not affect this result.
    #[test]
    fn auto_drive_terminal_ok_in_review_next_agent_not_null_fails() {
        let conn = fresh_db();
        insert_task(&conn, "T303", "in_review", Some(123));
        let args = json!({"display_id":"T303"});

        // Without wrap_log: still Fail (next_agent IS NOT NULL for in_review).
        let result = evaluate(
            DRIVE_PID_RECORDED_OR_TERMINAL,
            CheckCtx::with_conn(&conn),
            &args,
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.outcome, CheckOutcome::Fail);

        // With non-empty wrap_log: STILL Fail — wrap_log is not the sentinel.
        conn.execute(
            "UPDATE tasks SET wrap_log = ?1 WHERE display_id = 'T303'",
            rusqlite::params![r#"[{"executive_summary":"done"}]"#],
        )
        .unwrap();
        let result = evaluate(
            DRIVE_PID_RECORDED_OR_TERMINAL,
            CheckCtx::with_conn(&conn),
            &args,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            result.outcome,
            CheckOutcome::Fail,
            "wrap_log must not flip terminal-ok; next_agent IS NULL is the sole signal"
        );
    }

    /// A1-strict: terminal-ok for in_review is achieved by transitioning OUT of
    /// in_review (accepted/rejected). At that point next_agent IS NULL → Pass.
    #[test]
    fn auto_drive_terminal_ok_accepted_passes_regardless_of_wrap_log() {
        let conn = fresh_db();
        insert_task(&conn, "T304", "accepted", Some(123));
        // Set a wrap_log to confirm it does not interfere.
        conn.execute(
            "UPDATE tasks SET wrap_log = ?1 WHERE display_id = 'T304'",
            rusqlite::params![r#"[{"executive_summary":"done"}]"#],
        )
        .unwrap();
        let args = json!({"display_id":"T304"});
        let result = evaluate(
            DRIVE_PID_RECORDED_OR_TERMINAL,
            CheckCtx::with_conn(&conn),
            &args,
        )
        .unwrap()
        .unwrap();
        assert_eq!(result.outcome, CheckOutcome::Pass);
    }

    #[test]
    fn malformed_gatekeeper_json_returns_structured_result_with_diagnostic() {
        let args = json!({"gatekeeper_decision_json":{"confidence":"certain"}});
        let result = evaluate(GATEKEEPER_DECISION_VALID, CheckCtx::without_conn(), &args)
            .unwrap()
            .unwrap();
        assert_eq!(result.outcome, CheckOutcome::Fail);
        assert_eq!(result.check_id, GATEKEEPER_DECISION_VALID);
        assert!(result.args.get("gatekeeper_decision_json").is_some());
        assert!(!result.observed_at.is_empty());
        let reason = result.reason.as_ref().unwrap().to_string();
        assert!(
            reason.contains("decision is required") || reason.contains("confidence"),
            "{reason}"
        );
    }
}
