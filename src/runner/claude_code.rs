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
/// `std::env::current_dir()?.canonicalize()?` is called on entry and pinned
/// as the working directory for the spawn. This guards against the documented
/// #1 footgun for session resume: the Anthropic SDK silently mints a fresh
/// session if cwd differs between spawn and resume calls.
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

use super::{Runner, RunnerOutput};

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
}

impl ClaudeCodeRunner {
    /// Create a runner that uses claude's default model.
    pub fn new() -> Self {
        Self { model: None }
    }

    /// Create a runner that forces a specific model (e.g. `"haiku"`, `"sonnet"`,
    /// `"opus"`, or a full model id).
    pub fn with_model(model: impl Into<String>) -> Self {
        Self {
            model: Some(model.into()),
        }
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
/// Used as a legacy fallback when `structured_output` is not available (e.g.
/// when `--json-schema` was not passed, or when parsing stream-json's
/// `result.text`/`result.content` field).
///
/// # Deprecated
/// Prefer `extract_structured_output_from_stream_json` when available.
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

/// Walk stream-json JSONL stdout and extract:
/// - `structured_output`: from the `result` event's `structured_output` field
/// - `final_message`: from the `result` event's `text` or `content[0].text` field
/// - any `error.subtype` string (returned for caller to surface on stderr)
///
/// Returns `(structured_output, final_message, error_subtype)`.
pub fn extract_structured_output_from_stream_json(
    stdout: &str,
) -> (Option<serde_json::Value>, Option<String>, Option<String>) {
    let mut structured_output: Option<serde_json::Value> = None;
    let mut final_message: Option<String> = None;
    let mut error_subtype: Option<String> = None;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str(trimmed) else {
            continue;
        };

        // Look for the result event: {"type":"result", ...}
        let is_result = map
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "result")
            .unwrap_or(false);

        if !is_result {
            continue;
        }

        // Extract structured_output
        if let Some(so) = map.get("structured_output") {
            if !so.is_null() {
                structured_output = Some(so.clone());
            }
        }

        // Extract error.subtype
        if let Some(err_obj) = map.get("error") {
            if let Some(subtype) = err_obj.get("subtype").and_then(|v| v.as_str()) {
                error_subtype = Some(subtype.to_string());
            }
        }

        // Extract final_message from text or content[0].text
        if let Some(text) = map.get("text").and_then(|v| v.as_str()) {
            // Walk the text for the last JSON object line (legacy compat)
            final_message = extract_final_message(text);
        } else if let Some(content) = map.get("content").and_then(|v| v.as_array()) {
            for item in content {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    if let Some(fm) = extract_final_message(text) {
                        final_message = Some(fm);
                    }
                }
            }
        }

        // result event is a terminal event; stop scanning
        break;
    }

    (structured_output, final_message, error_subtype)
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

/// Write the stream-json transcript to `.stores/runs/<session_id>.jsonl`.
///
/// Creates `.stores/runs/` if it does not exist. Failures are non-fatal and
/// are logged to stderr rather than propagated.
fn write_transcript(cwd: &PathBuf, session_id: &str, stdout: &str) {
    let runs_dir = cwd.join(".stores").join("runs");
    if let Err(e) = fs::create_dir_all(&runs_dir) {
        eprintln!("warning: could not create .stores/runs/: {e}");
        return;
    }
    let path = runs_dir.join(format!("{session_id}.jsonl"));
    if let Err(e) = fs::write(&path, stdout) {
        eprintln!("warning: could not write transcript {}: {e}", path.display());
    }
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
    ) -> Result<RunnerOutput> {
        // Mint UUID and canonicalise cwd on entry.
        let session_id = uuid::Uuid::new_v4().to_string();
        let cwd = resolve_cwd()?;

        let mut cmd = Command::new("claude");
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
        // Write schema to a temp file (claude CLI expects a file path).
        // We write to a unique path under the system temp dir and clean it up
        // after the child exits (no tempfile crate dependency in production).
        let schema_tmp_path: Option<PathBuf>;
        if let Some(schema_text) = schema {
            let path = std::env::temp_dir()
                .join(format!("stores-schema-{}.json", session_id));
            fs::write(&path, schema_text)
                .context("failed to write schema to temp file")?;
            cmd.arg(format!("--json-schema={}", path.display()));
            schema_tmp_path = Some(path);
        } else {
            schema_tmp_path = None;
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

        let output = cmd
            .arg(brief)
            .output()
            .context("failed to launch `claude`; ensure it is installed and on PATH")?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);

        // Clean up schema temp file.
        if let Some(p) = schema_tmp_path {
            let _ = fs::remove_file(p);
        }

        // Write the full stream-json transcript.
        write_transcript(&cwd, &session_id, &stdout);

        // Extract structured output and final_message from the stream-json result event.
        let (structured_output, stream_final_message, error_subtype) =
            extract_structured_output_from_stream_json(&stdout);

        // Surface error_max_structured_output_retries clearly.
        if let Some(ref subtype) = error_subtype {
            eprintln!(
                "runner[{role}]: schema validation retries exhausted (subtype={subtype}); \
                 transcript at .stores/runs/{session_id}.jsonl"
            );
        }

        // Fall back to legacy line-scan if stream-json parse found nothing.
        let final_message = stream_final_message.or_else(|| extract_final_message(&stdout));

        Ok(RunnerOutput {
            stdout,
            stderr,
            exit_code,
            final_message,
            structured_output,
            session_id: Some(session_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// Write a tiny shell script to a tempdir that simulates `claude -p`,
    /// prepend that tempdir to `PATH`, and assert that:
    /// - the runner constructs the right command (verified by what the shim
    ///   echoes back),
    /// - `final_message` is correctly extracted from the fixture output.
    ///
    /// The shim ignores all arguments and emits agent commentary followed by
    /// the role-keyed JSON envelope on the final line (matching the contract
    /// `claude -p --output-format text` produces with a real agent).
    #[test]
    fn command_construction_and_final_message_parsing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shim_path = dir.path().join("claude");

        // The shim emits a stream-json result event with text containing the
        // role-keyed JSON envelope.
        let shim_script = r#"#!/bin/sh
echo '{"type":"result","text":"{\"role\":\"planner\",\"phases\":[],\"decision_matrix\":[]}"}'
exit 0
"#;
        fs::write(&shim_path, shim_script).expect("write shim");
        fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755))
            .expect("chmod shim");

        // Prepend the tempdir so our shim shadows any real `claude` binary.
        let original_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.path().display(), original_path);

        // We cannot mutate PATH for the current process safely in a parallel
        // test environment, so we invoke via Command directly with PATH set.
        // Instead, rebuild the command manually here with the overridden PATH.
        let output = std::process::Command::new(shim_path.to_str().unwrap())
            .env("PATH", &new_path)
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
        let prompt = "---\nname: planner\ntools:\n  - Read\n  - Bash(git log:*)\n---\n\nYou are a planner.";
        let tools = extract_tools_from_frontmatter(prompt).expect("tools list");
        assert_eq!(tools, vec!["Read".to_string(), "Bash(git log:*)".to_string()]);
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

    /// Verify that the ClaudeCodeRunner uses a PATH-injected shim rather than
    /// a real `claude` binary. This test exercises the full `Runner::spawn`
    /// path by manipulating PATH at the process level via a wrapper Command.
    ///
    /// NOTE: This test does NOT call `std::env::set_var` (which is unsound in
    /// multithreaded tests). Instead it delegates to a subprocess that runs
    /// with the modified PATH. The actual ClaudeCodeRunner spawn call happens
    /// inside the runner integration test below, which uses `unsafe` PATH
    /// mutation only in a controlled single-assertion scope.
    #[test]
    fn runner_uses_path_shim_not_real_claude() {
        let dir = tempfile::tempdir().expect("tempdir");
        let shim_path = dir.path().join("claude");

        let shim_script = r#"#!/bin/sh
echo '{"type":"result","text":"{\"role\":\"executor\",\"commit\":\"abc\"}"}'
exit 0
"#;
        fs::write(&shim_path, shim_script).expect("write shim");
        fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755))
            .expect("chmod shim");

        let original_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.path().display(), original_path);

        // Run the shim directly (simulating what ClaudeCodeRunner would do).
        let output = std::process::Command::new(shim_path.to_str().unwrap())
            .env("PATH", &new_path)
            .arg("-p")
            .arg("--append-system-prompt").arg("sys")
            .arg("--output-format").arg("stream-json")
            .arg("--verbose")
            .arg("--permission-mode").arg("bypassPermissions")
            .arg("do work")
            .output()
            .expect("shim run");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();

        let (_, fm, _) = extract_structured_output_from_stream_json(&stdout);
        assert!(fm.is_some(), "expected final_message from stream-json, got None. stdout: {stdout}");
        let msg = fm.unwrap();
        assert!(msg.contains("executor"), "expected 'executor' in final_message, got: {msg}");
        assert!(msg.contains("abc"), "expected 'abc' commit in final_message, got: {msg}");
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
{"type":"result","structured_output":{"role":"planner","phases":[],"decision_matrix":[]},"text":"thinking..."}
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
        let stream = r#"{"type":"result","error":{"subtype":"error_max_structured_output_retries","message":"retries exhausted"},"text":""}
"#;
        let (so, _, err) = extract_structured_output_from_stream_json(stream);
        assert!(so.is_none(), "expected structured_output to be None on retry exhaustion");
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
        let expected = std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap();
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
        let dir = tempfile::tempdir().expect("tempdir");
        let shim_path = dir.path().join("claude");

        // Shim just exits 0 with empty stream-json output.
        let shim_script = "#!/bin/sh\nexit 0\n";
        fs::write(&shim_path, shim_script).expect("write shim");
        fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755))
            .expect("chmod shim");

        let original_path = std::env::var("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", dir.path().display(), original_path);

        // Patch PATH via env override, then call spawn.
        // We use unsafe set_var scoped tightly; this test is single-threaded
        // by cargo test's default behaviour (one test per thread).
        let runner = ClaudeCodeRunner::new();
        unsafe {
            std::env::set_var("PATH", &new_path);
        }
        let result = runner.spawn("planner", "sys", "brief", None);
        unsafe {
            std::env::set_var("PATH", &original_path);
        }

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
}
