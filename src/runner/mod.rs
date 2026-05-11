/// Runner abstraction for stores workflow orchestration.
///
/// # v0.3 minimalism
///
/// The `Runner` trait is intentionally minimal for v0.3. It covers exactly what
/// `stores tasks drive` needs: synchronous, blocking spawn of a single agent role
/// with a system prompt and a brief. The trait is `Send` so it can be boxed and
/// stored in structs, but no async surface is exposed yet.
///
/// # Deferred extensions
///
/// The following capabilities are explicitly deferred to a future minor version:
///
/// - **Streaming**: `spawn` returns a completed `RunnerOutput`. Streaming output
///   (tokio, async-stream, etc.) requires an async trait surface and a runtime.
///   Deferred until there is a concrete UX requirement (e.g. live progress to TTY
///   while a long-running agent executes).
///
/// - **Cancellation**: No cancellation token is threaded through `spawn`. The
///   calling loop can bound execution via `--max-iters` at the drive layer, but
///   mid-spawn cancellation (SIGINT forwarding, timeout) is deferred.
///
/// - **Structured agent input**: `brief` is passed as a plain string. Future
///   versions may accept a typed `Brief` struct or a structured input format once
///   the brief schema stabilises.
///
/// - **Multiple outputs / tool calls**: `RunnerOutput` surfaces a single
///   `final_message` extracted from the last JSON-envelope line. Multi-turn or
///   tool-call-aware runners are deferred.
///
/// # Adding a new runner
///
/// 1. Create `src/runner/<name>.rs`.
/// 2. Implement `Runner` for your struct.
/// 3. Add a match arm to `select` in this module.
/// 4. Gate behind a Cargo feature if the runner has heavy dependencies.
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::cell::RefCell;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub mod codex;
pub mod liveness;
pub mod mock;
pub mod sap;

pub use codex::CodexRunner;

#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;

    /// Serializes tests that mutate process-wide runner environment variables.
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());
}

#[cfg(feature = "runner-claude-code")]
pub mod claude_code;

#[cfg(feature = "runner-claude-code")]
pub use claude_code::ClaudeCodeRunner;

#[cfg(feature = "runner-pi")]
pub mod pi;

#[cfg(feature = "runner-pi")]
pub use pi::PiRunner;

/// Pre-spawn live-discoverability context allocated by the drive layer.
///
/// Runners should honor this when present instead of minting their own session id
/// and transcript paths, so operators can discover live output while `spawn` is
/// still blocked on the child process.
#[derive(Debug, Clone)]
pub struct RunnerInvocationContext {
    pub session_id: String,
    pub flat_transcript_path: PathBuf,
    pub stderr_log_path: PathBuf,
    pub events_path: PathBuf,
    pub status_path: PathBuf,
}

#[derive(Debug)]
pub struct RunnerLiveEventSink {
    events_file: Rc<RefCell<File>>,
    status_path: PathBuf,
    current_activity: Option<String>,
    last_event_type: Option<String>,
}

impl RunnerLiveEventSink {
    pub fn open(invocation: &RunnerInvocationContext) -> Result<Rc<RefCell<Self>>> {
        let parent = invocation.events_path.parent().ok_or_else(|| {
            anyhow::anyhow!(
                "events path has no parent: {}",
                invocation.events_path.display()
            )
        })?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating live events dir: {}", parent.display()))?;
        let events_file = File::create(&invocation.events_path).with_context(|| {
            format!(
                "creating live events file: {}",
                invocation.events_path.display()
            )
        })?;
        let sink = Self {
            events_file: Rc::new(RefCell::new(events_file)),
            status_path: invocation.status_path.clone(),
            current_activity: None,
            last_event_type: None,
        };
        let rc = Rc::new(RefCell::new(sink));
        rc.borrow_mut().write_status("started")?;
        Ok(rc)
    }

    pub fn record_stdout_line(&mut self, line: &str, mapped: Vec<serde_json::Value>) -> Result<()> {
        self.record_event(json!({"type":"heartbeat","stream":"stdout"}))?;
        for event in mapped {
            self.record_event(event)?;
        }
        if line.trim().is_empty() {
            self.write_status("heartbeat")?;
        }
        Ok(())
    }

    pub fn record_stderr_line(&mut self, _line: &str) -> Result<()> {
        self.record_event(json!({"type":"heartbeat","stream":"stderr"}))
    }

    fn record_event(&mut self, mut event: serde_json::Value) -> Result<()> {
        let now = crate::handlers::row::now_iso8601();
        let event_type = event
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("event")
            .to_string();
        if let serde_json::Value::Object(ref mut map) = event {
            map.entry("ts".to_string())
                .or_insert_with(|| serde_json::Value::String(now.clone()));
        }
        match event_type.as_str() {
            "assistant_text" => self.current_activity = Some("assistant_text".to_string()),
            "tool_start" => {
                let name = event.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                self.current_activity = Some(format!("tool:{name}"));
            }
            "tool_end" => self.current_activity = None,
            "retry" => self.current_activity = Some("api_retry".to_string()),
            _ => {}
        }
        self.last_event_type = Some(event_type.clone());
        {
            let mut file = self.events_file.borrow_mut();
            writeln!(file, "{}", serde_json::to_string(&event)?)?;
            file.flush()?;
        }
        self.write_status(&event_type)
    }

    fn write_status(&self, event_type: &str) -> Result<()> {
        if let Some(parent) = self.status_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating live status dir: {}", parent.display()))?;
        }
        let now = crate::handlers::row::now_iso8601();
        let status = json!({
            "last_event_at": now,
            "last_event_type": event_type,
            "current_activity": self.current_activity,
        });
        std::fs::write(&self.status_path, serde_json::to_vec_pretty(&status)?)
            .with_context(|| format!("writing live runner status: {}", self.status_path.display()))
    }
}

pub fn events_path_for_transcript(transcript_path: &Path, session_id: &str) -> PathBuf {
    transcript_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(session_id)
        .join("events.jsonl")
}

pub fn status_path_for_transcript(transcript_path: &Path, session_id: &str) -> PathBuf {
    transcript_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(session_id)
        .join("status.json")
}

/// Per-agent invocation telemetry emitted by a runner at source.
#[derive(Debug, Clone, Default)]
pub struct AgentRunTelemetry {
    pub model_id: Option<String>,
    pub harness_id: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub prompt_cache_hits: Option<i64>,
    pub transcript_path: Option<String>,
    pub stderr_log_path: Option<String>,
    pub configured_harness_id: Option<String>,
    pub configured_model_id: Option<String>,
    pub configured_thinking_effort: Option<String>,
    pub effective_model_id: Option<String>,
    pub effective_thinking_effort: Option<String>,
    pub thinking_effort_source: Option<String>,
    pub provider_id: Option<String>,
    pub api_id: Option<String>,
    pub session_id: Option<String>,
    pub workspace_path: Option<String>,
    pub runner_exit_kind: Option<String>,
    pub payload_valid: Option<bool>,
    pub payload_error: Option<String>,
    pub cache_read_tokens: Option<i64>,
    pub cache_write_tokens: Option<i64>,
    pub cost_total: Option<f64>,
}

impl AgentRunTelemetry {
    /// Fully-populated telemetry for mock/test use.
    ///
    /// All required fields (`model_id`, `harness_id`, `started_at`, `ended_at`,
    /// `transcript_path`) are populated. `transcript_path` is a REAL file written
    /// under `<workspace_path>/.stores/runs/mock-<uuid>.json` so that
    /// `insert_agent_run`'s required-field validation (non-None, non-empty,
    /// resolves to a real path) is satisfied without spinning up a full runner.
    ///
    /// **Invariant:** transcript_path is NEVER under system temp (`/tmp`,
    /// `$TMPDIR`, etc.) and NEVER under `target/test-mock-runs`. The mock
    /// invariant matches the production invariant: transcripts live under the
    /// workspace `.stores/runs/` tree.
    ///
    /// Path selection (in order):
    /// 1. `STORES_RUNS_DIR` env var (set by tests to redirect transcript writes).
    /// 2. `<workspace_path>/.stores/runs/` — callers MUST supply a valid workspace
    ///    directory (typically a `tempfile::tempdir()` in tests).
    ///
    /// # Panics
    /// Panics if neither `STORES_RUNS_DIR` is set nor `workspace_path` is provided.
    ///
    /// For tests that assert specific field values (e.g. `happy_path_one_phase_mock`),
    /// replace the telemetry on the returned `RunnerOutput` with a
    /// fully-specified `AgentRunTelemetry` that points at real synthesized files.
    pub fn with_mock_defaults(workspace_path: &std::path::Path) -> Self {
        let now = crate::handlers::row::now_iso8601();
        // Choose a runs dir that is never under system temp and never under target/.
        // Priority: STORES_RUNS_DIR (set by tests) → <workspace_path>/.stores/runs/.
        let stub_id = uuid::Uuid::new_v4();
        let runs_dir = match std::env::var_os("STORES_RUNS_DIR") {
            Some(p) => std::path::PathBuf::from(p),
            None => workspace_path.join(".stores").join("runs"),
        };
        let _ = std::fs::create_dir_all(&runs_dir);
        let stub_path = runs_dir.join(format!("mock-{stub_id}.json"));
        let _ = std::fs::write(&stub_path, b"{\"model\":\"mock-model\"}\n");
        let transcript_path = Some(stub_path.to_string_lossy().into_owned());
        Self {
            model_id: Some("mock-model".to_string()),
            harness_id: Some("mock".to_string()),
            started_at: Some(now.clone()),
            ended_at: Some(now),
            transcript_path,
            ..Self::default()
        }
    }
}

/// The output produced by a single `Runner::spawn` call.
#[derive(Debug, Clone)]
pub struct RunnerOutput {
    /// Complete stdout captured from the runner process (or synthesised by the
    /// mock runner).
    pub stdout: String,
    /// Complete stderr captured from the runner process.
    pub stderr: String,
    /// Process exit code. `0` conventionally means success.
    pub exit_code: i32,
    /// The last non-empty stdout line that parses as a JSON object. `None` when
    /// no such line exists or the JSON is malformed. Populated by defensive
    /// scanning — malformed JSON does not cause `spawn` to return `Err`.
    ///
    /// # Deprecated
    /// Prefer `structured_output` when available. `final_message` is kept for
    /// backwards compatibility with mock fixtures and the legacy last-line-scan
    /// path. Will be removed in v0.5.
    pub final_message: Option<String>,
    /// Parsed JSON value from `--json-schema`-validated structured output
    /// (`result.structured_output` in the stream-json event). `None` when the
    /// runner did not use `--json-schema` (e.g. mock runner with no schema, or
    /// a legacy runner path).
    ///
    /// Drive prefers this field over `final_message` when both are present.
    pub structured_output: Option<serde_json::Value>,
    /// Runner-generated UUID (`uuid::Uuid::new_v4()`) minted at spawn time.
    /// Names the `.stores/runs/<session_id>.jsonl` transcript. `None` for mock
    /// runner (no session concept) and for runners that do not generate
    /// session IDs. Drive uses this for logging on failure and for human
    /// `--resume` workflows.
    pub session_id: Option<String>,
    /// Which extraction layer populated `structured_output`:
    /// - `Some("sdk")` — `result.structured_output` from the stream-json event
    ///   (schema-validated by the claude CLI).
    /// - `Some("sap")` — extracted from `result.result` text via SAP fallback.
    /// - `None` — `structured_output` is `None`; drive falls through to legacy
    ///   `final_message` / last-line scan.
    ///
    /// Used for postmortem logging (Phase 2 AC2.9). Mock runner always returns
    /// `None` unless tests explicitly set it.
    pub structured_output_source: Option<&'static str>,
    /// Runner/source telemetry for persistence in `agent_runs`.
    pub telemetry: AgentRunTelemetry,
    /// Payload-level validation error surfaced separately from `exit_code`.
    ///
    /// `exit_code` always reflects the real child process exit status.
    /// When the runner process exits 0 but the output fails schema validation
    /// (missing `final_output`, wrong shape, etc.), the runner sets this field
    /// instead of overriding `exit_code`. Drive checks this field after
    /// persisting telemetry and surfaces it via the same abort path as
    /// a non-zero exit.
    ///
    /// `None` means no payload error. `Some(reason)` means payload validation
    /// failed with the given human-readable reason.
    pub payload_error: Option<String>,
}

/// A synchronous, blocking agent runner.
///
/// Implementations must be `Send` so they can be placed in a `Box<dyn Runner>`
/// and used across threads (e.g. within a tokio `spawn_blocking` call when an
/// async wrapper is added in a future version).
pub trait Runner: Send {
    /// The canonical name of this runner, e.g. `"mock"` or `"claude-code"`.
    fn name(&self) -> &str;

    /// Spawn an agent run synchronously.
    ///
    /// # Parameters
    /// - `role`: the workflow role being run (e.g. `"planner"`, `"executor"`).
    /// - `system_prompt`: the full system prompt to pass to the agent.
    /// - `brief`: the per-task briefing text (passed to the agent as the user
    ///   turn or via stdin depending on the runner).
    /// - `schema`: optional JSON-schema text to pass via `--json-schema`. When
    ///   `Some`, the runner requests schema-validated structured output and
    ///   populates `RunnerOutput.structured_output` from the result event.
    ///   When `None`, the runner falls through to the legacy line-scan path
    ///   and `structured_output` is `None`.
    /// - `workspace_path`: optional path to use as the working directory for
    ///   the spawned agent. If `Some`, the runner MUST canonicalize and lock
    ///   this path at spawn entry. The Anthropic SDK silently mints a fresh
    ///   session if cwd differs between spawn and resume calls (see
    ///   `claude_code.rs:305-306`). If `None`, the runner uses the inherited
    ///   cwd (current behavior).
    ///
    /// # Errors
    /// Returns `Err` only for infrastructure failures (process failed to launch,
    /// mock queue exhausted, etc.). A non-zero `exit_code` in `RunnerOutput` is
    /// not an `Err` — callers must inspect `exit_code` themselves.
    fn spawn(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<RunnerOutput>;

    /// Spawn with an optional preallocated live invocation context.
    ///
    /// The default preserves existing runner behavior for implementations that
    /// do not yet support live discoverability.
    fn spawn_with_invocation(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
        _invocation: Option<&RunnerInvocationContext>,
    ) -> Result<RunnerOutput> {
        self.spawn(role, system_prompt, brief, schema, workspace_path)
    }
}

/// Returns a comma-separated list of always-available runners (no feature gate).
fn available_runners() -> String {
    [
        Some("mock"),
        Some("codex"),
        #[cfg(feature = "runner-claude-code")]
        Some("claude-code"),
        #[cfg(feature = "runner-pi")]
        Some("pi"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(", ")
}

/// Factory: construct a `Runner` by name.
///
/// # Available runners
/// - `"mock"` — always available; programmable canned-response queue used in
///   tests.
/// - `"claude-code"` — available only with the `runner-claude-code` Cargo
///   feature; shells out to `claude -p`.
///
/// # Errors
/// Returns `Err` for unknown names (with the list of available runners) or when
/// the requested runner requires a feature that is not compiled in.
pub fn select(name: &str) -> Result<Box<dyn Runner>> {
    match name {
        "mock" => Ok(Box::new(mock::MockRunner::new(vec![]))),
        "codex" => Ok(Box::new(codex::CodexRunner::new())),
        "claude-code" => {
            #[cfg(feature = "runner-claude-code")]
            {
                Ok(Box::new(claude_code::ClaudeCodeRunner::new()))
            }
            #[cfg(not(feature = "runner-claude-code"))]
            {
                bail!(
                    "runner 'claude-code' requires the runner-claude-code cargo feature; \
                     rebuild with `cargo install --features runner-claude-code`"
                )
            }
        }
        "pi" => {
            #[cfg(feature = "runner-pi")]
            {
                Ok(Box::new(pi::PiRunner::new()))
            }
            #[cfg(not(feature = "runner-pi"))]
            {
                bail!(
                    "runner 'pi' requires the runner-pi cargo feature; \
                     rebuild with `cargo install --features runner-pi`"
                )
            }
        }
        other => bail!(
            "unknown runner '{}'; available: {}",
            other,
            available_runners()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_mock() {
        let runner = select("mock").expect("mock runner should always be available");
        assert_eq!(runner.name(), "mock");
    }

    #[test]
    fn select_codex() {
        let runner = select("codex").expect("codex runner should always be available");
        assert_eq!(runner.name(), "codex");
    }

    #[test]
    fn select_unknown() {
        let err = select("does-not-exist")
            .err()
            .expect("should error for unknown runner");
        let msg = err.to_string();
        assert!(
            msg.contains("unknown runner"),
            "expected 'unknown runner' in error, got: {msg}"
        );
        assert!(
            msg.contains("available"),
            "expected 'available' runners list in error, got: {msg}"
        );
    }

    #[test]
    #[cfg(not(feature = "runner-claude-code"))]
    fn select_claude_code_without_feature() {
        let err = select("claude-code")
            .err()
            .expect("should error without feature");
        let msg = err.to_string();
        assert!(
            msg.contains("runner-claude-code"),
            "expected feature name in error, got: {msg}"
        );
    }

    #[test]
    #[cfg(feature = "runner-claude-code")]
    fn select_claude_code_with_feature() {
        let runner =
            select("claude-code").expect("claude-code runner should be available with feature");
        assert_eq!(runner.name(), "claude-code");
    }

    #[test]
    #[cfg(not(feature = "runner-pi"))]
    fn select_pi_without_feature() {
        let err = select("pi").err().expect("should error without feature");
        let msg = err.to_string();
        assert!(
            msg.contains("runner-pi"),
            "expected feature name in error, got: {msg}"
        );
    }

    #[test]
    #[cfg(feature = "runner-pi")]
    fn select_pi_with_feature() {
        let runner = select("pi").expect("pi runner should be available with feature");
        assert_eq!(runner.name(), "pi");
    }
}
