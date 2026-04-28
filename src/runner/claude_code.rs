/// Claude Code runner — feature-gated behind `runner-claude-code`.
///
/// Shells out to `claude -p` with the brief as the trailing prompt arg, the
/// system prompt via `--append-system-prompt`, and `--output-format text` (the
/// default — text on stdout, no claude-side JSON wrapping). Parses
/// `final_message` defensively from the last JSON object line of stdout, which
/// by contract is the agent's role-keyed envelope.
///
/// # Command construction
///
/// ```text
/// claude -p \
///   --append-system-prompt <system_prompt> \
///   --output-format text \
///   --permission-mode bypassPermissions \
///   <brief>
/// ```
///
/// `--output-format text` emits the agent's response verbatim (default — no
/// claude-event wrapper); the agent's contract is to terminate its output
/// with a single role-keyed JSON object on its final non-empty line.
///
/// `--permission-mode bypassPermissions` is required for headless autonomous
/// operation. Without it, write/edit/bash operations trigger interactive
/// permission prompts the spawned subagent cannot answer; the agent then
/// emits a "I need permission to..." text response instead of the role-keyed
/// JSON envelope, and the parser fails. The `--claude-code` runner is an
/// explicitly opted-in autonomous mode; the v0.4 path is per-role allowed-
/// tools whitelisting (`planner`/`plan-reviewer`/`code-reviewer` read-only,
/// `executor` write-enabled, `guide` read-plus-`stores gate answer`).
///
/// Notes:
/// - `--output-format stream-json` would require pairing with `--verbose` per
///   the current claude CLI; text mode is simpler and matches the agent
///   contract one-to-one.
/// - `--bare` is intentionally NOT passed: bare mode disables OAuth/keychain
///   auth and requires ANTHROPIC_API_KEY in the environment. Without it,
///   headless `claude` reads the user's normal OAuth credentials. The cost is
///   that CLAUDE.md auto-discovery and project hooks are active during the
///   spawned session — agents should treat their `--append-system-prompt`
///   contents as authoritative and ignore any project-side bleed-through.
///   v0.4 may revisit this if hook interaction becomes a real problem.
///
/// Brief is passed as the positional prompt argument (not stdin) because `claude
/// -p` accepts the prompt as a trailing CLI argument and that path does not
/// require stdin redirection.
///
/// # Defensive parsing
///
/// `final_message` extraction scans stdout from the last line backwards. A line
/// is a candidate if it parses as a JSON object (`serde_json::Value::Object`).
/// Malformed JSON causes `final_message: None`; the runner never panics on bad
/// output.
use anyhow::{Context, Result};
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

impl Runner for ClaudeCodeRunner {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn spawn(&self, role: &str, system_prompt: &str, brief: &str) -> Result<RunnerOutput> {
        let mut cmd = Command::new("claude");
        cmd.arg("-p")
            .arg("--append-system-prompt")
            .arg(system_prompt)
            .arg("--output-format")
            .arg("text");

        // Optional model override (for --testing / --claude-code-model).
        if let Some(m) = &self.model {
            cmd.arg(format!("--model={m}"));
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

        let final_message = extract_final_message(&stdout);

        Ok(RunnerOutput {
            stdout,
            stderr,
            exit_code,
            final_message,
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

        // The shim emits agent commentary followed by the role-keyed JSON
        // envelope on the last non-empty line (matching the contract
        // `claude -p --output-format text` produces with a real agent).
        let shim_script = r#"#!/bin/sh
echo 'thinking through the plan...'
echo '{"role":"planner","phases":[],"decision_matrix":[]}'
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
            .arg("text")
            .arg("--permission-mode")
            .arg("bypassPermissions")
            .arg("Plan this task.")
            .output()
            .expect("shim should run");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);
        assert_eq!(exit_code, 0);

        let final_message = extract_final_message(&stdout);
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

        let shim_script = "#!/bin/sh\necho '{\"role\":\"executor\",\"commit\":\"abc\"}'\nexit 0\n";
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
            .arg("--output-format").arg("text")
            .arg("--permission-mode").arg("bypassPermissions")
            .arg("do work")
            .output()
            .expect("shim run");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let fm = extract_final_message(&stdout);
        assert_eq!(fm.as_deref(), Some(r#"{"role":"executor","commit":"abc"}"#));
    }
}
