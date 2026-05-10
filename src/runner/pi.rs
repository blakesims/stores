/// Pi SDK runner — feature-gated behind `runner-pi`.
///
/// Spawns `node agents/sidecar/pi_runner.mjs` with role, cwd, system prompt,
/// brief, and role schema passed as files. The helper runs a headless Pi SDK
/// session with in-memory settings/session state and a generated terminating
/// `final_output` tool. The helper emits JSONL events; this runner extracts the
/// last `{ type: "final_output", payload: ... }`, validates it against the role
/// schema, writes the full transcript to `.stores/runs/<session_id>.jsonl`, and
/// returns it as `RunnerOutput.structured_output` with source `pi-tool`.
use anyhow::{bail, Context, Result};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::liveness::{self, LivenessClass, LivenessThresholds};
use super::{AgentRunTelemetry, Runner, RunnerOutput};

pub struct PiRunner {
    node_bin: PathBuf,
    helper_path: PathBuf,
}

impl PiRunner {
    pub fn new() -> Self {
        Self {
            node_bin: PathBuf::from("node"),
            helper_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("agents")
                .join("sidecar")
                .join("pi_runner.mjs"),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_bin_and_helper(node_bin: PathBuf, helper_path: PathBuf) -> Self {
        Self {
            node_bin,
            helper_path,
        }
    }
}

impl Default for PiRunner {
    fn default() -> Self {
        Self::new()
    }
}

fn resolve_cwd(workspace_path: Option<&str>) -> Result<PathBuf> {
    match workspace_path {
        Some(p) => PathBuf::from(p)
            .canonicalize()
            .with_context(|| format!("workspace_path canonicalize failed: '{p}'")),
        None => std::env::current_dir()?
            .canonicalize()
            .context("failed to canonicalize current_dir"),
    }
}

/// Write the JSONL transcript to `<runs_dir>/<session_id>.jsonl`.
///
/// Returns `Ok(path)` where `path` is always under `runs_dir` (never under
/// system temp). If the primary write fails, attempts to write an error stub
/// at `<runs_dir>/<session_id>-error.json`. If that also fails, tries
/// `create_dir_all` on `runs_dir` and retries both writes. If all attempts
/// fail, returns `Err` — callers must NOT call `insert_agent_run`; the drive
/// layer marks the row failed without a persisted path.
///
/// **Invariant:** a successful return value is ALWAYS under `.stores/runs/`.
/// No `/tmp` fallback exists.
fn write_transcript(cwd: &Path, session_id: &str, stdout: &str) -> anyhow::Result<PathBuf> {
    let runs_dir = std::env::var_os("STORES_RUNS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.join(".stores").join("runs"));

    let try_write = |mkdir: bool| -> Option<PathBuf> {
        if mkdir {
            let _ = fs::create_dir_all(&runs_dir);
        }
        if runs_dir.is_dir() || fs::create_dir_all(&runs_dir).is_ok() {
            let path = runs_dir.join(format!("{session_id}.jsonl"));
            if fs::write(&path, stdout).is_ok() {
                return Some(path);
            }
            eprintln!(
                "warning: pi runner could not write transcript {}; writing error stub",
                path.display()
            );
            let stub_path = runs_dir.join(format!("{session_id}-error.json"));
            let stub_content =
                "{\"error\":\"transcript write failed\",\"reason\":\"primary write failed\"}\n";
            if fs::write(&stub_path, stub_content).is_ok() {
                return Some(stub_path);
            }
        }
        None
    };

    if let Some(p) = try_write(false) {
        return Ok(p);
    }
    if let Some(p) = try_write(true) {
        return Ok(p);
    }

    anyhow::bail!(
        "pi transcript write failed: could not write to {} (no /tmp fallback; \
         transcript_path must be under .stores/runs/)",
        runs_dir.display()
    )
}

fn extract_pi_telemetry(stdout: &str) -> (Option<String>, Option<i64>, Option<i64>, Option<i64>) {
    let mut model_id = None;
    let mut tokens_in = None;
    let mut tokens_out = None;
    let mut prompt_cache_hits = None;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        model_id = model_id.or_else(|| {
            v.get("model")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        });
        if let Some(u) = v.get("usage") {
            tokens_in = tokens_in.or_else(|| u.get("input_tokens").and_then(|x| x.as_i64()));
            tokens_out = tokens_out.or_else(|| u.get("output_tokens").and_then(|x| x.as_i64()));
            prompt_cache_hits =
                prompt_cache_hits.or_else(|| u.get("prompt_cache_hits").and_then(|x| x.as_i64()));
        }
    }
    (model_id, tokens_in, tokens_out, prompt_cache_hits)
}

pub fn extract_final_output(stdout: &str) -> Option<serde_json::Value> {
    let mut last = None;
    for line in stdout.lines() {
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str(line.trim()) else {
            continue;
        };
        if map.get("type").and_then(|v| v.as_str()) == Some("final_output") {
            last = map.get("payload").cloned();
        }
    }
    last
}

fn validate_payload(schema: &str, payload: &serde_json::Value) -> Result<()> {
    let schema_json: serde_json::Value =
        serde_json::from_str(schema).context("role schema is not valid JSON")?;
    let compiled = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(&schema_json)
        .map_err(|e| anyhow::anyhow!("role schema compile failed: {e}"))?;
    if let Err(errors) = compiled.validate(payload) {
        let joined = errors.map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        bail!("pi final_output payload failed schema validation: {joined}");
    }
    Ok(())
}

impl Runner for PiRunner {
    fn name(&self) -> &str {
        "pi"
    }

    fn spawn(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<RunnerOutput> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let cwd = resolve_cwd(workspace_path)?;
        let tmp = tempfile::Builder::new().prefix("stores-pi-").tempdir()?;
        let system_path = tmp.path().join("system.md");
        let brief_path = tmp.path().join("brief.md");
        fs::write(&system_path, system_prompt)?;
        fs::write(&brief_path, brief)?;
        let schema_path = if let Some(s) = schema {
            let p = tmp.path().join("schema.json");
            fs::write(&p, s)?;
            Some(p)
        } else {
            None
        };

        let mut cmd = Command::new(&self.node_bin);
        cmd.current_dir(&cwd)
            .arg(&self.helper_path)
            .arg("--role")
            .arg(role)
            .arg("--cwd")
            .arg(&cwd)
            .arg("--system")
            .arg(&system_path)
            .arg("--brief")
            .arg(&brief_path);
        if let Some(p) = &schema_path {
            cmd.arg("--schema").arg(p);
        }
        let started_at = crate::handlers::row::now_iso8601();
        let output = liveness::run_streaming_with_liveness(
            &mut cmd,
            &LivenessThresholds::from_env(),
            |_: &str| {
                if let Some(path) = std::env::var_os("STORES_HEARTBEAT_FILE") {
                    if let Ok(mut f) = OpenOptions::new()
                        .write(true)
                        .truncate(true)
                        .create(true)
                        .open(path)
                    {
                        let _ = writeln!(f, "{}", crate::handlers::row::now_iso8601());
                    }
                }
            },
            |_| {},
        )
        .context("failed to launch pi helper; ensure node and @mariozechner/pi-coding-agent are available")?;
        let ended_at = crate::handlers::row::now_iso8601();
        let stdout = output.stdout;
        let stderr = output.stderr;
        let exit_code = output.exit_code;
        // write_transcript returns Err if the path cannot be written under
        // .stores/runs/ — no /tmp fallback. On Err, propagate so the drive
        // layer marks the row failed without calling insert_agent_run.
        let transcript_path = Some(
            write_transcript(&cwd, &session_id, &stdout)
                .context("pi transcript write failed; not persisting agent_run row")?
                .to_string_lossy()
                .to_string(),
        );
        let (raw_model_id, tokens_in, tokens_out, prompt_cache_hits) =
            extract_pi_telemetry(&stdout);
        // Pi runner MUST emit a deterministic model_id at the source layer so
        // insert_agent_run never receives None. If the child transcript carries a
        // model string, prefer it; otherwise fall back to the deterministic sentinel
        // "pi:default". The DB contract is required = non-None, non-empty; the
        // source layer (here) satisfies it — db.rs never provides defaults.
        let model_id = raw_model_id.or_else(|| Some("pi:default".to_string()));

        // Build telemetry from invocation-level data regardless of payload
        // validity — telemetry belongs to the invocation, not the payload.
        let telemetry = AgentRunTelemetry {
            model_id,
            harness_id: Some("pi".to_string()),
            started_at: Some(started_at),
            ended_at: Some(ended_at),
            tokens_in,
            tokens_out,
            prompt_cache_hits,
            transcript_path,
            stderr_log_path: None,
        };

        if matches!(
            output.killed_for,
            Some(LivenessClass::StalledNoOutput { .. } | LivenessClass::WallClockTimeout { .. })
        ) {
            return Ok(RunnerOutput {
                stdout,
                stderr,
                exit_code: -1,
                final_message: None,
                structured_output: None,
                session_id: Some(session_id),
                structured_output_source: None,
                telemetry,
                payload_error: output.payload_error,
            });
        }

        // Payload-level failures are surfaced via `payload_error` so that
        // `exit_code` always reflects the REAL child process exit status.
        // Drive persists telemetry (with the real exit_code) first, then checks
        // `payload_error` and surfaces it via the same abort path as non-zero exit.
        let payload = extract_final_output(&stdout);
        if exit_code == 0 && payload.is_none() {
            return Ok(RunnerOutput {
                stdout,
                stderr,
                exit_code,
                final_message: None,
                structured_output: None,
                session_id: Some(session_id),
                structured_output_source: None,
                telemetry,
                payload_error: Some("pi helper exited 0 but did not emit final_output".to_string()),
            });
        }
        if let (Some(s), Some(p)) = (schema, payload.as_ref()) {
            if let Err(e) = validate_payload(s, p) {
                return Ok(RunnerOutput {
                    stdout,
                    stderr,
                    exit_code,
                    final_message: None,
                    structured_output: None,
                    session_id: Some(session_id),
                    structured_output_source: None,
                    telemetry,
                    payload_error: Some(format!("{e:#}")),
                });
            }
        }
        Ok(RunnerOutput {
            stdout,
            stderr,
            exit_code,
            final_message: None,
            structured_output: payload,
            session_id: Some(session_id),
            structured_output_source: Some("pi-tool"),
            telemetry,
            payload_error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SHIM_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn shim(script: &str) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let shim_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-shims");
        fs::create_dir_all(&shim_dir).unwrap();
        let idx = SHIM_COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = shim_dir.join(format!("pi-shim-{}-{idx}.sh", std::process::id()));
        {
            let mut f = fs::File::create(&p).unwrap();
            use std::io::Write as _;
            f.write_all(script.as_bytes()).unwrap();
            f.sync_all().unwrap();
        }
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        (d, p)
    }

    #[test]
    fn extracts_last_final_output() {
        let out = "{\"type\":\"final_output\",\"payload\":{\"a\":1}}\n{\"type\":\"final_output\",\"payload\":{\"a\":2}}\n";
        assert_eq!(extract_final_output(out).unwrap()["a"], 2);
    }

    #[test]
    fn success_populates_pi_tool_structured_output() {
        let (_d, helper) = shim("#!/bin/sh\necho '{\"type\":\"final_output\",\"payload\":{\"role\":\"executor\",\"summary\":\"ok\"}}'\n");
        let runner = PiRunner::with_bin_and_helper(PathBuf::from("/bin/sh"), helper);
        let schema = r#"{"type":"object","required":["role","summary"],"properties":{"role":{"const":"executor"},"summary":{"type":"string"}}}"#;
        let out = runner
            .spawn(
                "executor",
                "sys",
                "brief",
                Some(schema),
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap();
        assert_eq!(out.structured_output_source, Some("pi-tool"));
        assert_eq!(out.structured_output.unwrap()["summary"], "ok");
    }

    #[test]
    fn malformed_payload_errors() {
        // Payload validation failure → Ok(RunnerOutput) with real exit_code (0)
        // from the child process, and the schema validation message in payload_error.
        // Telemetry is persisted by the caller (drive) before surfacing the error.
        let (_d, bin) = shim(
            "#!/bin/sh\necho '{\"type\":\"final_output\",\"payload\":{\"role\":\"executor\"}}'\n",
        );
        let runner = PiRunner::with_bin_and_helper(PathBuf::from("/bin/sh"), bin);
        let schema = r#"{"type":"object","required":["summary"],"properties":{"summary":{"type":"string"}}}"#;
        let out = runner
            .spawn(
                "executor",
                "sys",
                "brief",
                Some(schema),
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap();
        // exit_code reflects the REAL child process exit status (0 here).
        assert_eq!(
            out.exit_code, 0,
            "payload failure preserves real child exit_code"
        );
        // payload_error carries the validation message, separate from exit_code.
        let payload_err = out.payload_error.as_deref().unwrap_or("");
        assert!(
            payload_err.contains("schema validation"),
            "schema validation error in payload_error: got {:?}",
            payload_err
        );
        assert!(out.structured_output.is_none());
        assert!(out.telemetry.harness_id.as_deref() == Some("pi"));
    }

    #[test]
    fn missing_final_tool_call_errors_when_helper_exits_zero() {
        // Missing final_output when child exits 0 → Ok(RunnerOutput) with real
        // exit_code (0) and the explanation in payload_error (not exit_code override).
        // Telemetry is persisted by the caller (drive) before surfacing the error.
        let (_d, bin) =
            shim("#!/bin/sh\necho '{\"type\":\"message\",\"text\":\"done\"}'\nexit 0\n");
        let runner = PiRunner::with_bin_and_helper(PathBuf::from("/bin/sh"), bin);
        let out = runner
            .spawn(
                "planner",
                "sys",
                "brief",
                None,
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap();
        // exit_code reflects the REAL child process exit status (0 here).
        assert_eq!(
            out.exit_code, 0,
            "missing final_output preserves real child exit_code"
        );
        // payload_error carries the explanation, separate from exit_code.
        let payload_err = out.payload_error.as_deref().unwrap_or("");
        assert!(
            payload_err.contains("did not emit final_output"),
            "explanation in payload_error: got {:?}",
            payload_err
        );
        assert!(out.telemetry.harness_id.as_deref() == Some("pi"));
    }

    #[test]
    fn non_zero_helper_exit_is_returned_not_infra_error() {
        let (_d, helper) = shim("#!/bin/sh\necho nope >&2\nexit 7\n");
        let runner = PiRunner::with_bin_and_helper(PathBuf::from("/bin/sh"), helper);
        let out = runner
            .spawn(
                "planner",
                "sys",
                "brief",
                None,
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap();
        assert_eq!(out.exit_code, 7);
        assert!(out.structured_output.is_none());
        assert!(out.stderr.contains("nope"));
    }

    #[test]
    fn pi_runner_kills_alive_no_output_subprocess_after_threshold() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        std::env::set_var("STORES_RUNNER_NO_OUTPUT_SECS", "2");
        std::env::set_var("STORES_RUNNER_WALL_CLOCK_MAX_SECS", "30");
        let runs = tempfile::tempdir().unwrap();
        std::env::set_var("STORES_RUNS_DIR", runs.path());
        let (_d, helper) = shim("#!/bin/sh\nexec sleep 5\n");
        let runner = PiRunner::with_bin_and_helper(PathBuf::from("/bin/sh"), helper);
        let started = std::time::Instant::now();
        let out = runner
            .spawn(
                "executor",
                "sys",
                "brief",
                None,
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap();
        assert!(
            started.elapsed() <= std::time::Duration::from_secs(4),
            "elapsed={:?}",
            started.elapsed()
        );
        assert_eq!(out.exit_code, -1);
        assert!(out.payload_error.unwrap_or_default().contains("no output"));
        std::env::remove_var("STORES_RUNNER_NO_OUTPUT_SECS");
        std::env::remove_var("STORES_RUNNER_WALL_CLOCK_MAX_SECS");
        std::env::remove_var("STORES_RUNS_DIR");
    }

    #[test]
    fn pi_runner_streams_lines_and_extends_heartbeat() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        std::env::set_var("STORES_RUNNER_NO_OUTPUT_SECS", "2");
        std::env::set_var("STORES_RUNNER_WALL_CLOCK_MAX_SECS", "30");
        let runs = tempfile::tempdir().unwrap();
        std::env::set_var("STORES_RUNS_DIR", runs.path());
        let heartbeat = tempfile::NamedTempFile::new().unwrap();
        std::env::set_var("STORES_HEARTBEAT_FILE", heartbeat.path());
        let (_d, helper) = shim(
            "#!/bin/sh\necho '{\"type\":\"final_output\",\"payload\":{\"role\":\"executor\",\"summary\":\"one\"}}'\nsleep 0.3\necho '{\"type\":\"final_output\",\"payload\":{\"role\":\"executor\",\"summary\":\"two\"}}'\nsleep 0.3\necho '{\"type\":\"final_output\",\"payload\":{\"role\":\"executor\",\"summary\":\"three\"}}'\n",
        );
        let runner = PiRunner::with_bin_and_helper(PathBuf::from("/bin/sh"), helper);
        let schema = r#"{"type":"object","required":["role","summary"],"properties":{"role":{"const":"executor"},"summary":{"type":"string"}}}"#;
        let out = runner
            .spawn(
                "executor",
                "sys",
                "brief",
                Some(schema),
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap();
        assert_eq!(out.payload_error, None);
        assert_eq!(out.structured_output.unwrap()["summary"], "three");
        assert_eq!(out.stdout.lines().count(), 3);
        let mtime = std::fs::metadata(heartbeat.path())
            .unwrap()
            .modified()
            .unwrap();
        assert!(mtime.elapsed().unwrap() <= std::time::Duration::from_secs(2));
        std::env::remove_var("STORES_RUNNER_NO_OUTPUT_SECS");
        std::env::remove_var("STORES_RUNNER_WALL_CLOCK_MAX_SECS");
        std::env::remove_var("STORES_RUNS_DIR");
        std::env::remove_var("STORES_HEARTBEAT_FILE");
    }

    #[test]
    fn writes_transcript() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let runs = tempfile::tempdir().unwrap();
        std::env::set_var("STORES_RUNS_DIR", runs.path());
        let sid = "pi-transcript-test";
        write_transcript(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            sid,
            "{\"type\":\"final_output\"}\n",
        )
        .expect("write_transcript must succeed with STORES_RUNS_DIR set");
        let text = fs::read_to_string(runs.path().join(format!("{sid}.jsonl"))).unwrap();
        assert!(text.contains("final_output"));
    }
}
