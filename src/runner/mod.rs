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
use anyhow::{bail, Result};

pub mod mock;
pub mod sap;

#[cfg(feature = "runner-claude-code")]
pub mod claude_code;

#[cfg(feature = "runner-claude-code")]
pub use claude_code::ClaudeCodeRunner;

#[cfg(feature = "runner-pi")]
pub mod pi;

#[cfg(feature = "runner-pi")]
pub use pi::PiRunner;

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
}

/// Returns a comma-separated list of always-available runners (no feature gate).
fn available_runners() -> String {
    [
        Some("mock"),
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
}
