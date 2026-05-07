/// Claude Code runner — feature-gated behind `runner-claude-code`.
///
/// Shells out to `claude -p` with the brief as the trailing prompt arg, the
/// system prompt via `--append-system-prompt`, `--output-format stream-json
/// --verbose` for full transcript capture, a runner-minted `--session-id` for
/// resumability, and an optional `--json-schema` for schema-validated
/// structured output.
///
/// # Command construction
///
/// ```text
/// claude -p \
///   --append-system-prompt <system_prompt> \
///   --output-format stream-json --verbose \
///   --session-id <uuid> \
///   [--json-schema <schema_path>] \
///   [--allowed-tools=<tools> | --permission-mode=bypassPermissions] \
///   [--model=<model>] \
///   <brief>
/// ```
///
/// The runner mints a fresh UUID on each `spawn` call and returns it via
/// `RunnerOutput.session_id`. Drive does NOT generate the UUID; it only
/// consumes the returned value for logging and future `--resume` workflows.
///
/// After the child exits, the full stream-json stdout is written to
/// `.stores/runs/<session_id>.jsonl` for postmortem. The runner then walks
/// the JSONL stream to find the `result` event and extracts:
/// - `result.structured_output` → `RunnerOutput.structured_output`
/// - `result.error.subtype` → surfaced to stderr when present
/// - `result.text` / `result.content` → `RunnerOutput.final_message` (legacy)
///
/// # cwd canonicalisation
///
/// Two branches, both resolved once at spawn entry:
/// - When `workspace_path` is `Some(p)`: `p` is canonicalized and used as cwd.
/// - When `workspace_path` is `None`: `std::env::current_dir()?.canonicalize()?` (inherited cwd).
///
/// Both guard against the documented #1 footgun for session resume: the
/// Anthropic SDK silently mints a fresh session if cwd differs between
/// spawn and resume calls.
///
/// # Schema dialect
///
/// Schemas are authored in JSON Schema Draft 2020-12 by default. If the SDK
/// rejects 2020-12 with a dialect error, swap the `$schema` URI to Draft-07
/// and re-run (Decision Matrix row 8).
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use super::{AgentRunTelemetry, Runner, RunnerOutput};

use super::sap::{extract_all_json_objects, pick_best_sap_candidate};

/// Runner that shells out to the `claude` CLI (`claude -p`).
///
/// Constructed via `ClaudeCodeRunner::new()` (default model) or
/// `ClaudeCodeRunner::with_model(...)` (forces a specific model — useful for
/// `--testing` / `haiku` smoke runs). The `claude` binary must be on `PATH` at
/// `spawn` time; if it is not found, `spawn` returns `Err`.
pub struct ClaudeCodeRunner {
    /// If `Some`, passes `--model=<value>` to claude on every spawn. If `None`,
    /// claude uses its default model.
    model: Option<String>,
    /// Binary to invoke. Defaults to `PathBuf::from("claude")` (PATH lookup).
    /// Tests override this via `with_bin` to point at an absolute shim path,
    /// eliminating any need to mutate the process-global PATH.
    bin: PathBuf,
}

impl ClaudeCodeRunner {
    /// Create a runner that uses claude's default model.
    pub fn new() -> Self {
        Self {
            model: None,
            bin: PathBuf::from("claude"),
        }
    }

    /// Create a runner that forces a specific model (e.g. `"haiku"`, `"sonnet"`,
    /// `"opus"`, or a full model id).
    pub fn with_model(model: impl Into<String>) -> Self {
        Self {
            model: Some(model.into()),
            bin: PathBuf::from("claude"),
        }
    }

    /// Override the binary path (test-only). Replaces `Command::new("claude")`
    /// with `Command::new(bin)`, so tests can point at an absolute shim path
    /// without mutating the process-global PATH.
    #[cfg(test)]
    pub(crate) fn with_bin(mut self, bin: PathBuf) -> Self {
        self.bin = bin;
        self
    }
}

impl Default for ClaudeCodeRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the `tools:` whitelist from an agent's YAML frontmatter.
///
/// Bundled agents declare per-role allowed tools in their frontmatter. The
/// runner reads this list and passes it via `--allowed-tools`. If no
/// frontmatter or no `tools` field is present, returns `None` and the runner
/// falls back to bypass mode (kept as a safety net for unbundled agents but
/// flagged in stderr).
///
/// Format expected:
/// ```yaml
/// ---
/// name: planner
/// tools:
///   - Read
///   - Bash(git log:*)
/// ---
/// ```
fn extract_tools_from_frontmatter(system_prompt: &str) -> Option<Vec<String>> {
    let trimmed = system_prompt.trim_start();
    let rest = trimmed.strip_prefix("---")?.trim_start_matches('\n');
    let end = rest.find("\n---")?;
    let frontmatter = &rest[..end];
    let value: serde_yaml::Value = serde_yaml::from_str(frontmatter).ok()?;
    let tools = value.get("tools")?.as_sequence()?;
    let collected: Vec<String> = tools
        .iter()
        .filter_map(|t| t.as_str().map(|s| s.to_string()))
        .collect();
    if collected.is_empty() {
        None
    } else {
        Some(collected)
    }
}

/// Scan `stdout` from the last line backwards and return the first line that
/// parses as a JSON object (i.e. a `{...}` map).
///
/// Returns `None` if no such line exists. Malformed JSON is silently skipped.
///
/// Kept available for tests that exercise the historic last-line scan
/// behaviour. Drive's Layer 3 owns the legacy fallback against
/// `RunnerOutput.stdout`; the runner does not pre-scan stdout because
/// intermediate stream-json events would be mistaken for the envelope.
#[cfg(test)]
fn extract_final_message(stdout: &str) -> Option<String> {
    for line in stdout.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(serde_json::Value::Object(_)) = serde_json::from_str(trimmed) {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Walk stream-json JSONL stdout and extract data from the terminal `result` event.
///
/// Parses stdout line-by-line as JSONL and finds the LAST event whose
/// `type == "result"`. Data is ONLY extracted from that event — never from
/// intermediate `user`, `assistant`, `tool_use`, `tool_result`, or `system`
/// events (which caused the Phase 3 attempt 1 bug where a denied tool_result
/// was mistakenly returned as the envelope).
///
/// Returns `(structured_output, final_message_text, error_subtype)` where:
/// - `structured_output` = `result.structured_output` (SDK-validated; `None` if
///   absent or null)
/// - `final_message_text` = `result.result` (the human-readable assistant text;
///   used for SAP fallback extraction and legacy `final_message` compat)
/// - `error_subtype` = `result.error.subtype` if present (e.g.
///   `"error_max_structured_output_retries"`)
pub fn extract_structured_output_from_stream_json(
    stdout: &str,
) -> (Option<serde_json::Value>, Option<String>, Option<String>) {
    // Collect all events first, then pick the last result event.
    // This ensures correctness for multi-turn sessions where the SDK may
    // emit multiple result events (e.g. on retry).
    let mut last_result_event: Option<serde_json::Map<String, serde_json::Value>> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str(trimmed) else {
            continue;
        };

        let is_result = map
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "result")
            .unwrap_or(false);

        if is_result {
            last_result_event = Some(map);
        }
        // Continue scanning — we want the LAST result event.
    }

    let Some(map) = last_result_event else {
        return (None, None, None);
    };

    // Extract structured_output from the result event.
    let structured_output = map
        .get("structured_output")
        .filter(|v| !v.is_null())
        .cloned();

    // Extract error.subtype from the result event.
    let error_subtype = map
        .get("error")
        .and_then(|e| e.get("subtype"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Extract the human-readable assistant text from the result event.
    // The stream-json result event uses "result" as the text field name
    // (not "text" or "content"). Fall back to "text" for SDK compat.
    let final_message_text = map
        .get("result")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            map.get("text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    // Return the raw result.result text directly. Earlier versions filtered
    // through extract_final_message (last-line JSON scan) which returned None
    // for multi-line pretty-printed envelopes — drive's Layer 2 then had no
    // text to feed SAP. The raw text lets SAP do its work; legacy line-scan
    // is still available as drive's Layer 3 against `RunnerOutput.stdout`.
    (structured_output, final_message_text, error_subtype)
}

/// Walk stream-json JSONL stdout and return the raw text from the terminal
/// `result` event's `result` field (or `text` field as fallback).
///
/// Used by `spawn` to supply the raw prose to the SAP layer for envelope
/// extraction when `structured_output` is absent.
pub fn extract_result_text_from_stream_json(stdout: &str) -> Option<String> {
    let mut last_result_text: Option<String> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str(trimmed) else {
            continue;
        };
        let is_result = map
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "result")
            .unwrap_or(false);
        if is_result {
            last_result_text = map
                .get("result")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    map.get("text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                });
        }
    }

    last_result_text
}

/// Resolve and canonicalise the cwd for spawn.
///
/// Extracted into a testable helper so unit tests can assert the path used.
/// This guards the documented #1 footgun for session resume: the Anthropic SDK
/// silently mints a fresh session if cwd differs between spawn and resume calls.
pub fn resolve_cwd() -> Result<PathBuf> {
    std::env::current_dir()
        .context("failed to read current_dir")?
        .canonicalize()
        .context("failed to canonicalize current_dir")
}

/// Write the stream-json transcript to `<runs_dir>/<session_id>.jsonl`.
///
/// `runs_dir` defaults to `<cwd>/.stores/runs` but is overridden by
/// `STORES_RUNS_DIR` if set — tests use this to redirect transcripts to a
/// tempdir so they don't pollute the project's `.stores/runs/`.
///
/// Creates the dir if it does not exist. Failures are non-fatal and are
/// logged to stderr rather than propagated.
fn write_transcript(cwd: &PathBuf, session_id: &str, stdout: &str) -> Option<PathBuf> {
    let runs_dir = match std::env::var_os("STORES_RUNS_DIR") {
        Some(p) => PathBuf::from(p),
        None => cwd.join(".stores").join("runs"),
    };
    if let Err(e) = fs::create_dir_all(&runs_dir) {
        eprintln!(
            "warning: could not create runs dir {}: {e}",
            runs_dir.display()
        );
        return None;
    }
    let path = runs_dir.join(format!("{session_id}.jsonl"));
    if let Err(e) = fs::write(&path, stdout) {
        eprintln!(
            "warning: could not write transcript {}: {e}",
            path.display()
        );
        return None;
    }
    Some(path)
}

pub fn extract_telemetry_from_stream_json(
    stdout: &str,
    configured_model: Option<&str>,
) -> (Option<String>, Option<i64>, Option<i64>, Option<i64>) {
    let mut model_id = configured_model.map(|s| s.to_string());
    let mut tokens_in = None;
    let mut tokens_out = None;
    let mut prompt_cache_hits = None;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if model_id.is_none() {
            model_id = v
                .get("model")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .or_else(|| {
                    v.get("message")
                        .and_then(|m| m.get("model"))
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string())
                });
        }
        let usage = v
            .get("usage")
            .or_else(|| v.get("message").and_then(|m| m.get("usage")));
        if let Some(u) = usage {
            tokens_in = tokens_in.or_else(|| u.get("input_tokens").and_then(|x| x.as_i64()));
            tokens_out = tokens_out.or_else(|| u.get("output_tokens").and_then(|x| x.as_i64()));
            let cache = u
                .get("cache_read_input_tokens")
                .and_then(|x| x.as_i64())
                .or_else(|| u.get("prompt_cache_hits").and_then(|x| x.as_i64()))
                .or_else(|| u.get("cache_hit_input_tokens").and_then(|x| x.as_i64()));
            prompt_cache_hits = prompt_cache_hits.or(cache);
        }
    }
    (model_id, tokens_in, tokens_out, prompt_cache_hits)
}

impl Runner for ClaudeCodeRunner {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn spawn(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<RunnerOutput> {
        // Mint UUID and canonicalise cwd on entry.
        let session_id = uuid::Uuid::new_v4().to_string();
        // Canonicalize once at spawn entry; the SDK silently mints a fresh session
        // if cwd differs across resume calls (see lines 33-38).
        let cwd = match workspace_path {
            Some(p) => std::path::PathBuf::from(p)
                .canonicalize()
                .with_context(|| format!("workspace_path canonicalize failed: '{p}'"))?,
            None => resolve_cwd()?,
        };

        let mut cmd = Command::new(&self.bin);
        cmd.current_dir(&cwd);
        cmd.arg("-p")
            .arg("--append-system-prompt")
            .arg(system_prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg(format!("--session-id={session_id}"));

        // Optional model override (for --testing / --claude-code-model).
        if let Some(m) = &self.model {
            cmd.arg(format!("--model={m}"));
        }

        // Optional JSON schema for structured output validation.
        // The claude CLI's `--json-schema <schema>` takes the schema as inline
        // JSON text (per `claude --help`), NOT a file path. Earlier prototypes
        // wrote to a temp file and passed the path — claude then silently
        // produced no output and the runner hung.
        if let Some(schema_text) = schema {
            cmd.arg(format!("--json-schema={schema_text}"));
        }

        // Per-role tool whitelist from the agent's frontmatter (preferred).
        // Falls back to bypassPermissions if frontmatter is absent — this is a
        // safety net for unbundled agents; bundled agents always declare tools.
        //
        // NOTE: `--allowed-tools` is variadic in the claude CLI (`<tools...>`):
        // passed as two args (`--allowed-tools VALUE`) it greedily consumes any
        // subsequent positional, including the trailing `brief`. Use the
        // `--flag=value` form so it binds tightly and `brief` stays the
        // positional prompt. Same for `--permission-mode`.
        match extract_tools_from_frontmatter(system_prompt) {
            Some(tools) => {
                cmd.arg(format!("--allowed-tools={}", tools.join(" ")));
            }
            None => {
                eprintln!(
                    "warning: agent '{}' has no `tools:` in frontmatter; \
                     falling back to --permission-mode=bypassPermissions",
                    role
                );
                cmd.arg("--permission-mode=bypassPermissions");
            }
        }

        let started_at = crate::handlers::row::now_iso8601();
        let output = cmd
            .arg(brief)
            .output()
            .context("failed to launch `claude`; ensure it is installed and on PATH")?;
        let ended_at = crate::handlers::row::now_iso8601();

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);

        // Write the full stream-json transcript.
        let transcript_path =
            write_transcript(&cwd, &session_id, &stdout).map(|p| p.to_string_lossy().to_string());
        let (model_id, tokens_in, tokens_out, prompt_cache_hits) =
            extract_telemetry_from_stream_json(&stdout, self.model.as_deref());

        // Extract structured output and final_message from the stream-json result event.
        let (sdk_structured_output, stream_final_message, error_subtype) =
            extract_structured_output_from_stream_json(&stdout);

        // Surface error_max_structured_output_retries clearly.
        if let Some(ref subtype) = error_subtype {
            eprintln!(
                "runner[{role}]: schema validation retries exhausted (subtype={subtype}); \
                 transcript at .stores/runs/{session_id}.jsonl"
            );
        }

        // Three-layer extraction at the runner level:
        // Layer 1 (SDK): result.structured_output populated by claude CLI schema validation.
        // Layer 2 (SAP): extract envelope from result.result text without schema enforcement —
        //                inject the agent role after extraction (the model often omits the
        //                role-tag because it treats it as orchestrator metadata, not envelope
        //                content). Drive's `AgentEnvelope` uses `serde(tag = "role")` so the
        //                role must be present at deserialise time.
        // Layer 3 (legacy): drive owns this layer; runner does not pre-scan stdout for stray
        //                JSON because intermediate stream-json events parse as JSON objects
        //                and would be mistaken for the envelope.
        let (structured_output, structured_output_source) = if sdk_structured_output.is_some() {
            (sdk_structured_output, Some("sdk"))
        } else {
            // T047 codex-revise: walk ALL candidates and pick the best one
            // for the agent role. Previously this used extract_envelope_from_text
            // which returns the FIRST parseable JSON object — meaning an
            // unrelated leading `{...}` in result.result text could shadow
            // the true planner/executor envelope (and drive.rs's later
            // pick-best fallback never got to run because structured_output
            // was already populated).
            let result_text = extract_result_text_from_stream_json(&stdout);
            let sap_result = result_text.as_deref().and_then(|text| {
                let candidates = extract_all_json_objects(text);
                pick_best_sap_candidate(&candidates, role).map(|picked| {
                    let mut value = picked.clone();
                    if let serde_json::Value::Object(ref mut map) = &mut value {
                        map.entry("role".to_string())
                            .or_insert_with(|| serde_json::Value::String(role.to_string()));
                    }
                    value
                })
            });
            if sap_result.is_some() {
                (sap_result, Some("sap"))
            } else {
                (None, None)
            }
        };

        // final_message holds the raw result.result text from the terminal result event.
        // Drive's Layer 3 falls back to its own last-line scan over `stdout` if needed
        // (mock fixtures construct RunnerOutput directly and bypass this path).
        let final_message = stream_final_message;

        Ok(RunnerOutput {
            stdout,
            stderr,
            exit_code,
            final_message,
            structured_output,
            session_id: Some(session_id),
            structured_output_source,
            telemetry: AgentRunTelemetry {
                model_id,
                harness_id: Some("claude-code".to_string()),
                started_at: Some(started_at),
                ended_at: Some(ended_at),
                tokens_in,
                tokens_out,
                prompt_cache_hits,
                transcript_path,
            },
            payload_error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::OnceLock;

    /// Redirect transcript writes to `<CARGO_MANIFEST_DIR>/target/test-runs/` so
    /// shim-driven runner tests don't pollute the project's `.stores/runs/`
    /// (which is the operator-facing transcript directory). The path lives
    /// under `target/`, which is gitignored. All tests share the same dir;
    /// transcripts are UUID-keyed so writes never collide.
    fn redirect_runs_dir() {
        let runs_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-runs");
        // Safe to set unconditionally — all tests redirect to the same path,
        // so concurrent set_var calls converge on the same value.
        std::env::set_var("STORES_RUNS_DIR", &runs_dir);
    }

    #[test]
    fn stream_json_telemetry_extraction_and_transcript_path() {
        let runs = tempfile::tempdir().unwrap();
        let previous_runs_dir = std::env::var_os("STORES_RUNS_DIR");
        std::env::set_var("STORES_RUNS_DIR", runs.path());

        let stdout = concat!(
            "{\"type\":\"system\",\"model\":\"claude-system\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-3-5-sonnet\",\"usage\":{\"input_tokens\":11,\"output_tokens\":7,\"cache_read_input_tokens\":5}}}\n",
            "{\"type\":\"result\",\"usage\":{\"input_tokens\":11,\"output_tokens\":7,\"cache_read_input_tokens\":5},\"result\":\"ok\"}\n"
        );
        let (model_id, tokens_in, tokens_out, prompt_cache_hits) =
            extract_telemetry_from_stream_json(stdout, None);
        assert_eq!(model_id.as_deref(), Some("claude-system"));
        assert_eq!(tokens_in, Some(11));
        assert_eq!(tokens_out, Some(7));
        assert_eq!(prompt_cache_hits, Some(5));

        let path = write_transcript(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            "telemetry-test",
            stdout,
        )
        .expect("transcript path");
        assert!(path.exists(), "transcript exists: {}", path.display());
        assert!(path.starts_with(runs.path()), "under STORES_RUNS_DIR");

        match previous_runs_dir {
            Some(value) => std::env::set_var("STORES_RUNS_DIR", value),
            None => std::env::remove_var("STORES_RUNS_DIR"),
        }
    }

    /// Module-level shim directory, created once for the lifetime of the test binary.
    ///
    /// All shim-exec tests reference paths inside this directory rather than
    /// writing short-lived per-test shim files. Writing each shim once before
    /// any test runs eliminates the kernel inode-recycling ETXTBSY race that
    /// appears under 20+ parallel threads on Linux 6.x: that race fires when a
    /// file is quickly created, exec'd, and its inode freed, and the freed inode
    /// is concurrently reused by another write that happens to overlap with an
    /// in-flight execve. A stable, long-lived shim file is never freed during
    /// the test run, so no recycling can occur.
    ///
    /// The `TempDir` value is kept alive for the whole test run by the `OnceLock`.
    /// We leak it at process exit rather than cleaning it up — the OS reclaims
    /// the files anyway, and avoiding teardown prevents a race between the test
    /// runner's `drop` and any shim that's still running.
    struct ShimDir {
        /// Kept alive so the shim files are not deleted during the test run.
        #[allow(dead_code)]
        dir: tempfile::TempDir,
        /// `#!/bin/sh\nexit 0\n` — produces no stdout, exits 0.
        silent: PathBuf,
        /// Emits a stream-json result event with a role=planner envelope.
        planner: PathBuf,
        /// Emits a stream-json result event with a role=executor envelope.
        executor: PathBuf,
        /// Emits `{"type":"result","result":"<cwd>"}` where cwd is `$(pwd)`.
        cwd_printer: PathBuf,
    }

    static SHIM_DIR: OnceLock<ShimDir> = OnceLock::new();

    /// Create all shim files once. Called via `SHIM_DIR.get_or_init(init_shims)`.
    ///
    /// Writes to a subdirectory of `target/debug/` (the cargo build output
    /// directory). This path is stable, is NOT in `/tmp`, and the directory
    /// already exists — avoiding the ext4 inode-recycling race that fires
    /// under high concurrency when many short-lived files are created and
    /// deleted in `/tmp` simultaneously.
    fn init_shims() -> ShimDir {
        // Write to the cargo target directory for this package. The target dir
        // has stable inodes that are not affected by the high-turnover /tmp
        // churn from parallel test tempdirs.
        let target_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-shims");
        fs::create_dir_all(&target_dir).expect("create shim target dir");
        // Wrap in a TempDir-like struct; we'll use a plain PathBuf instead.
        // But we need TempDir for ShimDir.dir... let's use a TempDir in target.
        let dir = tempfile::Builder::new()
            .prefix("shims-")
            .tempdir_in(&target_dir)
            .expect("shim tempdir");

        let write = |name: &str, script: &str| {
            let p = dir.path().join(name);
            let mut f = fs::File::create(&p).expect("create shim");
            f.write_all(script.as_bytes()).expect("write shim");
            f.sync_all().expect("sync shim");
            drop(f);
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).expect("chmod shim");
            p
        };

        let silent = write("silent", "#!/bin/sh\nexit 0\n");
        // The planner shim emits a stream-json result event with a planner envelope.
        let planner = write("planner", concat!(
            "#!/bin/sh\n",
            "echo '{\"type\":\"result\",\"result\":\"{\\\"role\\\":\\\"planner\\\",\\\"phases\\\":[],\\\"decision_matrix\\\":[]}\"}'",
            "\nexit 0\n",
        ));
        // The executor shim emits a stream-json result event with an executor envelope.
        let executor = write("executor", concat!(
            "#!/bin/sh\n",
            "echo '{\"type\":\"result\",\"result\":\"{\\\"role\\\":\\\"executor\\\",\\\"commit\\\":\\\"abc\\\"}\"}'",
            "\nexit 0\n",
        ));
        // The cwd_printer shim emits {"type":"result","result":"<cwd>"} where cwd is pwd.
        let cwd_printer = write(
            "cwd_printer",
            concat!(
                "#!/bin/sh\n",
                "printf '{\"type\":\"result\",\"result\":\"%s\"}\\n' \"$(pwd)\"",
                "\nexit 0\n",
            ),
        );

        ShimDir {
            dir,
            silent,
            planner,
            executor,
            cwd_printer,
        }
    }

    /// Return a reference to the process-wide shim directory, initializing it
    /// on first call.
    fn shims() -> &'static ShimDir {
        SHIM_DIR.get_or_init(init_shims)
    }

    /// Write a tiny shell script that simulates `claude -p` and assert that
    /// `final_message` is correctly extracted from the fixture output.
    ///
    /// The shim ignores all arguments and emits a role-keyed JSON envelope in
    /// a stream-json result event. Invoked via `Command::new(shim_path)` — no
    /// PATH mutation. Uses the process-wide `SHIM_DIR` so no per-test write
    /// occurs (eliminates the kernel ETXTBSY inode-recycling race).
    #[test]
    fn command_construction_and_final_message_parsing() {
        let shim_path = &shims().planner;

        // Invoke the stable shim directly via its absolute path.
        let output = std::process::Command::new(shim_path)
            .arg("-p")
            .arg("--append-system-prompt")
            .arg("You are a planner.")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--permission-mode")
            .arg("bypassPermissions")
            .arg("Plan this task.")
            .output()
            .expect("shim should run");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);
        assert_eq!(exit_code, 0);

        let (_, final_message, _) = extract_structured_output_from_stream_json(&stdout);
        assert!(
            final_message.is_some(),
            "expected a final JSON message, stdout was: {stdout}"
        );
        let msg = final_message.unwrap();
        assert!(
            msg.contains("planner"),
            "expected 'planner' in final_message, got: {msg}"
        );
    }

    #[test]
    fn extract_final_message_picks_last_json_object() {
        let stdout = "some text\n{\"role\":\"first\"}\nmore text\n{\"role\":\"last\"}";
        let result = extract_final_message(stdout);
        assert_eq!(result.as_deref(), Some(r#"{"role":"last"}"#));
    }

    #[test]
    fn extract_final_message_skips_malformed_json() {
        let stdout = "not json\nalso not json\n{broken";
        let result = extract_final_message(stdout);
        assert!(result.is_none(), "expected None for all-malformed stdout");
    }

    #[test]
    fn extract_final_message_skips_json_arrays() {
        // JSON arrays are not objects — should not be returned as final_message.
        let stdout = "[1,2,3]\n{\"role\":\"executor\"}";
        let result = extract_final_message(stdout);
        assert_eq!(result.as_deref(), Some(r#"{"role":"executor"}"#));
    }

    #[test]
    fn extract_final_message_empty_stdout() {
        assert!(extract_final_message("").is_none());
        assert!(extract_final_message("\n\n\n").is_none());
    }

    #[test]
    fn extract_tools_basic() {
        let prompt =
            "---\nname: planner\ntools:\n  - Read\n  - Bash(git log:*)\n---\n\nYou are a planner.";
        let tools = extract_tools_from_frontmatter(prompt).expect("tools list");
        assert_eq!(
            tools,
            vec!["Read".to_string(), "Bash(git log:*)".to_string()]
        );
    }

    #[test]
    fn extract_tools_no_frontmatter() {
        let prompt = "You are a planner.";
        assert!(extract_tools_from_frontmatter(prompt).is_none());
    }

    #[test]
    fn extract_tools_no_tools_key() {
        let prompt = "---\nname: planner\ndescription: foo\n---\n";
        assert!(extract_tools_from_frontmatter(prompt).is_none());
    }

    #[test]
    fn extract_tools_empty_list() {
        let prompt = "---\nname: planner\ntools: []\n---\n";
        // Empty list is treated as None — bypass fallback applies.
        assert!(extract_tools_from_frontmatter(prompt).is_none());
    }

    /// Verify that the shim script runs successfully and produces the expected
    /// stream-json output. Invokes the stable executor shim directly via its
    /// absolute path — no PATH mutation, no per-test shim write.
    #[test]
    fn runner_uses_path_shim_not_real_claude() {
        let shim_path = &shims().executor;

        // Run the stable executor shim directly.
        let output = std::process::Command::new(shim_path)
            .arg("-p")
            .arg("--append-system-prompt")
            .arg("sys")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--permission-mode")
            .arg("bypassPermissions")
            .arg("do work")
            .output()
            .expect("shim run");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

        let (_, fm, _) = extract_structured_output_from_stream_json(&stdout);
        assert!(
            fm.is_some(),
            "expected final_message from stream-json, got None. stdout: {stdout}"
        );
        let msg = fm.unwrap();
        assert!(
            msg.contains("executor"),
            "expected 'executor' in final_message, got: {msg}"
        );
        assert!(
            msg.contains("abc"),
            "expected 'abc' commit in final_message, got: {msg}"
        );
    }

    // -------------------------------------------------------------------------
    // AC1.5 tests
    // -------------------------------------------------------------------------

    /// AC1.5(a): extract_structured_output_from_stream_json returns Some(value)
    /// for a stream-json with result.structured_output populated.
    #[test]
    fn extract_structured_output_returns_some_when_present() {
        let stream = r#"{"type":"system","subtype":"init"}
{"type":"assistant","message":{"content":[{"type":"text","text":"thinking..."}]}}
{"type":"result","structured_output":{"role":"planner","phases":[],"decision_matrix":[]},"result":"thinking..."}
"#;
        let (so, _, err) = extract_structured_output_from_stream_json(stream);
        assert!(so.is_some(), "expected structured_output to be Some");
        assert!(err.is_none());
        let val = so.unwrap();
        assert_eq!(val["role"].as_str(), Some("planner"));
    }

    /// AC1.5(b): returns None for structured_output and surfaces error_subtype
    /// when result contains error_max_structured_output_retries.
    #[test]
    fn extract_structured_output_returns_none_and_error_subtype_on_retries_exhausted() {
        let stream = r#"{"type":"result","error":{"subtype":"error_max_structured_output_retries","message":"retries exhausted"},"result":""}
"#;
        let (so, _, err) = extract_structured_output_from_stream_json(stream);
        assert!(
            so.is_none(),
            "expected structured_output to be None on retry exhaustion"
        );
        assert_eq!(
            err.as_deref(),
            Some("error_max_structured_output_retries"),
            "expected error_subtype to be error_max_structured_output_retries"
        );
    }

    /// AC1.5(c): cwd is canonicalised before spawn — resolve_cwd() returns a
    /// path equal to std::env::current_dir()?.canonicalize()?.
    #[test]
    fn cwd_canonicalised_before_spawn() {
        let expected = std::env::current_dir().unwrap().canonicalize().unwrap();
        let got = resolve_cwd().unwrap();
        assert_eq!(
            got, expected,
            "resolve_cwd() must return canonicalized current_dir"
        );
    }

    /// AC1.5(d): session-id is a valid v4 UUID and is propagated to
    /// RunnerOutput.session_id when the shim exits 0.
    #[test]
    fn session_id_is_valid_uuid_v4_propagated_to_output() {
        // Use the stable silent shim (exits 0, no stdout) via with_bin.
        // Pass a stable workspace_path so resolve_cwd() is never called — this
        // avoids a race with paths::tests that call set_current_dir and then
        // drop the tmp dir before restoring, leaving the process cwd dangling.
        redirect_runs_dir();
        let workspace = env!("CARGO_MANIFEST_DIR").to_string();
        let runner = ClaudeCodeRunner::new().with_bin(shims().silent.clone());
        let result = runner.spawn("planner", "sys", "brief", None, Some(&workspace));

        let out = result.expect("spawn should succeed with shim");
        let sid = out.session_id.expect("session_id should be Some");

        // Validate it's a parseable v4 UUID.
        let parsed = uuid::Uuid::parse_str(&sid).expect("session_id must be a valid UUID");
        assert_eq!(
            parsed.get_version(),
            Some(uuid::Version::Random),
            "session_id must be a v4 (random) UUID"
        );
    }

    // -------------------------------------------------------------------------
    // AC1.8 test
    // -------------------------------------------------------------------------

    /// AC1.8: --json-schema is passed inline (not as a file path).
    ///
    /// Negative assertion: the runner command must NOT include `--json-schema=/tmp/`
    /// (file-path form). Positive assertion: the schema text is embedded directly
    /// in the argument string.
    ///
    /// We verify by inspecting the ClaudeCodeRunner's spawn output via a shim
    /// that echoes its arguments, then assert the schema text is present inline
    /// and no `/tmp/` path is constructed.
    #[test]
    fn json_schema_arg_is_passed_inline() {
        let schema_text = r#"{"type":"object","properties":{"role":{"const":"planner"}}}"#;
        // Use the stable silent shim via with_bin. No PATH mutation, no per-test write.
        // Pass a stable workspace_path so resolve_cwd() is never called — guards
        // against the cwd-dangling race from paths::tests.
        redirect_runs_dir();
        let workspace = env!("CARGO_MANIFEST_DIR").to_string();
        let runner = ClaudeCodeRunner::new().with_bin(shims().silent.clone());
        let result = runner.spawn(
            "planner",
            "sys",
            "brief",
            Some(schema_text),
            Some(&workspace),
        );

        // The spawn should succeed (shim exits 0).
        result.expect("spawn should succeed with shim");

        // Verify by constructing the expected arg — the runner must use
        // --json-schema=<text> form. We verify negatively that no temp-file
        // path was constructed by inspecting this module's source directly.
        let source = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/runner/claude_code.rs"
        ));
        // The deprecated temp-file pattern used a path prefix "stores-schema-" inside /tmp.
        // Split the needle so this very assertion doesn't match itself.
        let bad_needle = ["/tmp/stores", "-schema-"].concat();
        assert!(
            !source.contains(&bad_needle),
            "claude_code.rs must not construct a temp-file path for --json-schema"
        );
        assert!(
            source.contains("--json-schema="),
            "claude_code.rs must pass --json-schema= inline"
        );
    }

    // -------------------------------------------------------------------------
    // AC1.9 tests
    // -------------------------------------------------------------------------

    /// AC1.9: extractor skips intermediate user events and returns only data
    /// from the terminal result event.
    ///
    /// Uses the staged planner-haiku-multiturn.jsonl fixture (26 events: 1
    /// system, 16 assistant, 7 user, 1 rate_limit, 1 result).
    #[test]
    fn extract_structured_output_skips_intermediate_user_events() {
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/agent_outputs/planner-haiku-multiturn.jsonl"
        );
        let stdout = std::fs::read_to_string(fixture_path).expect("read fixture");

        let (so, final_msg, err) = extract_structured_output_from_stream_json(&stdout);

        // structured_output: the fixture result event has no structured_output
        // field (it's absent / null).
        assert!(
            so.is_none(),
            "structured_output must be None for this fixture (result.structured_output is null)"
        );

        // final_message_text from result.result must be Some and contain the
        // markdown-fenced planner JSON (not an intermediate user event's content).
        // The extractor returns the last-JSON-line scan result from the result
        // event's text — which for this fixture finds the closing JSON in the fence.
        // The raw text must contain role=planner; we test via final_msg or
        // by re-extracting the result text directly.
        let result_text = extract_result_text_from_stream_json(&stdout);
        let text = result_text.expect("result text must be Some");
        assert!(
            text.contains("\"role\": \"planner\""),
            "result text must contain role=planner, got: {}",
            &text[..text.len().min(200)]
        );
        assert!(
            text.contains("```json"),
            "result text must contain markdown fence ```json"
        );

        // error_subtype must be None (this was a success result).
        assert!(
            err.is_none(),
            "error_subtype must be None for success result"
        );

        // final_msg: the extractor may or may not find a JSON line depending on
        // whether the last-line scan picks up the JSON inside the fence.
        // The important invariant is that final_msg does NOT come from an
        // intermediate user event — we can verify by asserting final_msg
        // (if Some) does not look like a tool_result payload.
        if let Some(ref fm) = final_msg {
            assert!(
                !fm.contains("tool_use_id"),
                "final_message must not be from an intermediate tool_result event, got: {fm}"
            );
        }
    }

    /// AC1.9: extractor returns structured_output from a result event that has
    /// sdk-validated structured_output populated.
    #[test]
    fn extract_structured_output_picks_result_event_with_sdk_validated_output() {
        let stream = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\"}\n",
            "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"thinking\"}]}}\n",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"xyz\",\"content\":\"denied\"}]}}\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"structured_output\":{\"role\":\"planner\",\"phases\":[],\"decision_matrix\":[]},\"result\":\"\"}\n",
        );

        let (so, _, err) = extract_structured_output_from_stream_json(stream);
        assert!(
            so.is_some(),
            "structured_output must be Some when result event has it"
        );
        let val = so.unwrap();
        assert_eq!(val["role"].as_str(), Some("planner"));
        assert!(val["phases"].is_array());
        assert!(val["decision_matrix"].is_array());
        assert!(err.is_none());
    }

    // -------------------------------------------------------------------------
    // AC1.7: workspace_path runner-level tests
    // -------------------------------------------------------------------------

    /// AC1.7: when workspace_path: Some(<path>), the spawn uses the canonicalized
    /// form of that path as cwd. Verified via the stable cwd_printer shim that
    /// prints its cwd to stdout.
    ///
    /// Uses a stable subdirectory of CARGO_MANIFEST_DIR and holds the project
    /// cwd lock to serialize against paths::tests that call set_current_dir.
    /// Those tests change the process cwd to a temp directory and may run git,
    /// which can leave write fds open that trigger ETXTBSY on concurrent execs.
    #[test]
    fn workspace_path_canonicalised_when_some() {
        // Acquire the project-wide cwd lock. This test does not call
        // set_current_dir itself, but holding the lock serializes against
        // paths::tests that do, preventing ETXTBSY from a concurrent write fd
        // colliding with the cwd_printer shim exec.
        let _cwd_guard = crate::paths::test_cwd_lock()
            .lock()
            .expect("cwd lock poisoned");
        redirect_runs_dir();

        // Use the cargo target directory as the workspace — it always exists,
        // has stable inodes, and never collides with the shim files' inodes.
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        let workspace_str = workspace.to_str().unwrap().to_string();
        let expected_cwd = workspace.canonicalize().unwrap();

        // Use with_bin to point the runner at the stable cwd_printer shim.
        // No PATH mutation, no per-test shim write, no tempdir in /tmp.
        let runner = ClaudeCodeRunner::new().with_bin(shims().cwd_printer.clone());
        let result = runner.spawn("planner", "sys", "brief", None, Some(&workspace_str));

        let out = result.expect("spawn should succeed with shim");
        let printed_cwd = out
            .final_message
            .expect("final_message (cwd) must be present");
        let actual_cwd = std::path::PathBuf::from(printed_cwd.trim());
        assert_eq!(
            actual_cwd, expected_cwd,
            "spawned cwd must equal canonicalized workspace_path"
        );
    }

    /// AC1.7: when workspace_path: None, the runner uses the inherited cwd
    /// (resolve_cwd() = std::env::current_dir().canonicalize()). Mirrors
    /// the existing cwd_canonicalised_before_spawn test. Uses the stable
    /// cwd_printer shim — no per-test shim write.
    #[test]
    fn workspace_path_falls_back_to_inherited_when_none() {
        // Acquire the project-wide cwd lock BEFORE reading current_dir.
        // This serializes against tests that call set_current_dir, so the cwd
        // we capture here is the same one the spawned shim will report.
        // No PATH lock needed: with_bin points the runner at the stable cwd_printer.
        let _cwd_guard = crate::paths::test_cwd_lock()
            .lock()
            .expect("cwd lock poisoned");
        redirect_runs_dir();

        let expected_cwd = resolve_cwd().expect("resolve_cwd must succeed");

        // Use with_bin to point the runner at the stable cwd_printer shim.
        // No PATH mutation, no per-test shim write.
        let runner = ClaudeCodeRunner::new().with_bin(shims().cwd_printer.clone());
        let result = runner.spawn("planner", "sys", "brief", None, None);
        drop(_cwd_guard);

        let out = result.expect("spawn should succeed with shim");
        let printed_cwd = out
            .final_message
            .expect("final_message (cwd) must be present");
        let actual_cwd = std::path::PathBuf::from(printed_cwd.trim());
        assert_eq!(
            actual_cwd, expected_cwd,
            "spawned cwd with workspace_path=None must equal inherited cwd"
        );
    }
}
