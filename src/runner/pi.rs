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
use std::cell::RefCell;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use super::liveness::{self, LivenessClass, LivenessThresholds};
use super::{
    AgentRunTelemetry, Runner, RunnerInvocationContext, RunnerLiveEventSink, RunnerOutput,
};

pub struct PiRunner {
    node_bin: PathBuf,
    helper_path: PathBuf,
    configured_model: Option<String>,
    configured_thinking: Option<String>,
}

impl PiRunner {
    pub fn new() -> Self {
        Self {
            node_bin: PathBuf::from("node"),
            helper_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("agents")
                .join("sidecar")
                .join("pi_runner.mjs"),
            configured_model: None,
            configured_thinking: None,
        }
    }

    pub fn with_config(
        configured_model: Option<String>,
        configured_thinking: Option<String>,
    ) -> Self {
        Self {
            configured_model,
            configured_thinking,
            ..Self::new()
        }
    }

    #[cfg(test)]
    pub(crate) fn with_bin_and_helper(node_bin: PathBuf, helper_path: PathBuf) -> Self {
        Self {
            node_bin,
            helper_path,
            configured_model: None,
            configured_thinking: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_bin_helper_and_config(
        node_bin: PathBuf,
        helper_path: PathBuf,
        configured_model: Option<String>,
        configured_thinking: Option<String>,
    ) -> Self {
        Self {
            node_bin,
            helper_path,
            configured_model,
            configured_thinking,
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

fn transcript_path(cwd: &Path, session_id: &str) -> PathBuf {
    let runs_dir = std::env::var_os("STORES_RUNS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.join(".stores").join("runs"));
    runs_dir.join(format!("{session_id}.jsonl"))
}

fn stderr_path(cwd: &Path, session_id: &str) -> PathBuf {
    let runs_dir = std::env::var_os("STORES_RUNS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.join(".stores").join("runs"));
    runs_dir.join(format!("{session_id}.stderr.log"))
}

fn open_live_file(path: PathBuf, label: &str) -> anyhow::Result<(PathBuf, Rc<RefCell<File>>)> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("transcript path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "pi {label} write failed: could not create runs dir {} (no /tmp fallback)",
            parent.display()
        )
    })?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .with_context(|| format!("pi {label} write failed: could not open {}", path.display()))?;
    Ok((path, Rc::new(RefCell::new(file))))
}

fn open_live_transcript(
    cwd: &Path,
    session_id: &str,
    invocation: Option<&RunnerInvocationContext>,
) -> anyhow::Result<(PathBuf, Rc<RefCell<File>>)> {
    let path = invocation
        .map(|i| i.flat_transcript_path.clone())
        .unwrap_or_else(|| transcript_path(cwd, session_id));
    open_live_file(path, "transcript")
}

fn open_live_stderr(
    cwd: &Path,
    session_id: &str,
    invocation: Option<&RunnerInvocationContext>,
) -> anyhow::Result<(PathBuf, Rc<RefCell<File>>)> {
    let path = invocation
        .map(|i| i.stderr_log_path.clone())
        .unwrap_or_else(|| stderr_path(cwd, session_id));
    open_live_file(path, "stderr")
}

fn append_live_line(file: &Rc<RefCell<File>>, line: &str) -> anyhow::Result<()> {
    let mut file = file.borrow_mut();
    writeln!(file, "{line}")?;
    file.flush()?;
    Ok(())
}

fn text_from_pi_content(content: &serde_json::Value) -> Option<String> {
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    let parts = content.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(|v| v.as_str())
                .or_else(|| part.get("content").and_then(|v| v.as_str()))
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

/// Map JSONL events forwarded verbatim by `agents/sidecar/pi_runner.mjs` via
/// `session.subscribe(...)`, plus the stores-owned `final_output` tool event.
///
/// The Pi SDK event names pinned here are the ones observed/consumed by the
/// installed `pi-subagents` extension's foreground runner: `tool_execution_start`,
/// `tool_execution_end`, `tool_result_end`, and `message_end`. Keep accepting
/// both camelCase and snake_case tool id/name fields because SDK/plugin event
/// payloads have used both spellings across integrations.
fn map_pi_event(line: &str) -> Vec<serde_json::Value> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    match v.get("type").and_then(|x| x.as_str()) {
        Some("tool_execution_start") => events.push(serde_json::json!({
            "type": "tool_start",
            "id": v.get("toolCallId").or_else(|| v.get("tool_call_id")).cloned(),
            "name": v.get("toolName").or_else(|| v.get("tool_name")).and_then(|x| x.as_str()).unwrap_or("tool"),
            "args_preview": v.get("args").map(|args| args.to_string()),
        })),
        Some("tool_execution_end") | Some("tool_result_end") => events.push(serde_json::json!({
            "type": "tool_end",
            "id": v.get("toolCallId").or_else(|| v.get("tool_call_id")).cloned(),
            "name": v.get("toolName").or_else(|| v.get("tool_name")).and_then(|x| x.as_str()),
            "ok": !v.get("isError").or_else(|| v.get("is_error")).and_then(|x| x.as_bool()).unwrap_or(false),
        })),
        Some("message_end") => {
            if let Some(message) = v.get("message") {
                if message.get("role").and_then(|x| x.as_str()) == Some("assistant") {
                    if let Some(text) = message.get("content").and_then(text_from_pi_content) {
                        events.push(serde_json::json!({"type":"assistant_text", "text": text}));
                    }
                }
                if let Some(usage) = message.get("usage") {
                    events.push(serde_json::json!({
                        "type": "usage",
                        "input_tokens": usage.get("input_tokens").or_else(|| usage.get("input")),
                        "output_tokens": usage.get("output_tokens").or_else(|| usage.get("output")),
                        "cache_read_tokens": usage.get("cache_read_input_tokens").or_else(|| usage.get("cacheRead")),
                    }));
                }
            }
        }
        Some("final_output") => events.push(serde_json::json!({
            "type": "final_output",
            "payload": v.get("payload").cloned().unwrap_or(serde_json::Value::Null),
        })),
        _ => {
            if let Some(usage) = v.get("usage") {
                events.push(serde_json::json!({
                    "type": "usage",
                    "input_tokens": usage.get("input_tokens"),
                    "output_tokens": usage.get("output_tokens"),
                    "cache_read_tokens": usage.get("cache_read_input_tokens"),
                }));
            }
        }
    }
    events
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
#[cfg(test)]
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

#[derive(Debug, Default, PartialEq)]
struct PiExtractedTelemetry {
    model_id: Option<String>,
    provider_id: Option<String>,
    api_id: Option<String>,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
    prompt_cache_hits: Option<i64>,
    cache_write_tokens: Option<i64>,
    cost_total: Option<f64>,
    configured_model_id: Option<String>,
    configured_thinking_effort: Option<String>,
    effective_thinking_effort: Option<String>,
    thinking_effort_source: Option<String>,
}

fn usage_i64(usage: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| usage.get(*key).and_then(|x| x.as_i64()))
}

fn usage_f64(usage: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| usage.get(*key).and_then(|x| x.as_f64()))
}

fn value_string(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    v.as_object().and_then(|obj| {
        ["id", "name", "provider", "api"]
            .iter()
            .find_map(|key| obj.get(*key).and_then(|x| x.as_str()).map(str::to_string))
    })
}

fn extract_pi_telemetry(stdout: &str) -> PiExtractedTelemetry {
    let mut telemetry = PiExtractedTelemetry::default();
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if v.get("type").and_then(|x| x.as_str()) == Some("stores_config") {
            telemetry.configured_model_id = telemetry.configured_model_id.or_else(|| {
                v.get("configured_model")
                    .and_then(|x| x.as_str())
                    .map(str::to_string)
            });
            telemetry.configured_thinking_effort =
                telemetry.configured_thinking_effort.or_else(|| {
                    v.get("configured_thinking")
                        .and_then(|x| x.as_str())
                        .map(str::to_string)
                });
            telemetry.model_id = telemetry.model_id.or_else(|| {
                v.get("effective_model")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty() && *s != "unknown")
                    .map(str::to_string)
            });
            telemetry.effective_thinking_effort =
                telemetry.effective_thinking_effort.or_else(|| {
                    v.get("effective_thinking")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                });
            telemetry.thinking_effort_source = telemetry.thinking_effort_source.or_else(|| {
                v.get("thinking_source")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            });
        }
        let message = v.get("message");
        telemetry.model_id = telemetry.model_id.or_else(|| {
            message
                .and_then(|m| m.get("model"))
                .or_else(|| v.get("model"))
                .and_then(value_string)
        });
        telemetry.provider_id = telemetry.provider_id.or_else(|| {
            message
                .and_then(|m| m.get("provider"))
                .or_else(|| v.get("provider"))
                .and_then(value_string)
        });
        telemetry.api_id = telemetry.api_id.or_else(|| {
            message
                .and_then(|m| m.get("api"))
                .or_else(|| v.get("api"))
                .and_then(value_string)
        });
        let usage = message
            .and_then(|m| m.get("usage"))
            .or_else(|| v.get("usage"));
        if let Some(u) = usage {
            telemetry.tokens_in = telemetry
                .tokens_in
                .or_else(|| usage_i64(u, &["input_tokens", "input", "prompt_tokens"]));
            telemetry.tokens_out = telemetry
                .tokens_out
                .or_else(|| usage_i64(u, &["output_tokens", "output", "completion_tokens"]));
            telemetry.prompt_cache_hits = telemetry.prompt_cache_hits.or_else(|| {
                usage_i64(
                    u,
                    &[
                        "prompt_cache_hits",
                        "cache_read_input_tokens",
                        "cacheRead",
                        "cache_read_tokens",
                    ],
                )
            });
            telemetry.cache_write_tokens = telemetry.cache_write_tokens.or_else(|| {
                usage_i64(
                    u,
                    &[
                        "cache_creation_input_tokens",
                        "cache_write_input_tokens",
                        "cache_write_tokens",
                    ],
                )
            });
            telemetry.cost_total = telemetry
                .cost_total
                .or_else(|| usage_f64(u, &["cost_total", "cost"]));
        }
    }
    telemetry
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

impl PiRunner {
    fn spawn_inner(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
        invocation: Option<&RunnerInvocationContext>,
        extra_env: &[(String, String)],
    ) -> Result<RunnerOutput> {
        let session_id = invocation
            .map(|i| i.session_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
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
        cmd.current_dir(&cwd);
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        cmd.arg(&self.helper_path)
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
        if let Some(model) = &self.configured_model {
            cmd.arg("--model").arg(model);
        }
        if let Some(thinking) = &self.configured_thinking {
            cmd.arg("--thinking").arg(thinking);
        }
        let (live_transcript_path, live_transcript) =
            open_live_transcript(&cwd, &session_id, invocation)
                .context("pi transcript write failed; not launching runner")?;
        let (stderr_log_path, live_stderr) = open_live_stderr(&cwd, &session_id, invocation)
            .context("pi stderr log write failed; not launching runner")?;
        let event_sink = invocation.map(RunnerLiveEventSink::open).transpose()?;
        let started_at = crate::handlers::row::now_iso8601();
        let stdout_transcript = Rc::clone(&live_transcript);
        let stderr_log = Rc::clone(&live_stderr);
        let stdout_events = event_sink.clone();
        let stderr_events = event_sink.clone();
        let output = liveness::run_streaming_with_liveness(
            &mut cmd,
            &LivenessThresholds::from_env(),
            move |line| {
                append_live_line(&stdout_transcript, line)?;
                if let Some(sink) = &stdout_events {
                    sink.borrow_mut().record_stdout_line(line, map_pi_event(line))?;
                }
                Ok(())
            },
            move |line| {
                append_live_line(&stderr_log, line)?;
                if let Some(sink) = &stderr_events {
                    sink.borrow_mut().record_stderr_line(line)?;
                }
                Ok(())
            },
        )
        .context("failed to launch pi helper; ensure node and @mariozechner/pi-coding-agent are available")?;
        let ended_at = crate::handlers::row::now_iso8601();
        let stdout = output.stdout;
        let stderr = output.stderr;
        let exit_code = output.exit_code;
        let transcript_path = Some(live_transcript_path.to_string_lossy().to_string());
        let extracted = extract_pi_telemetry(&stdout);
        // Pi runner MUST emit a deterministic model_id at the source layer so
        // insert_agent_run never receives None. If the child transcript carries a
        // model string, prefer it; otherwise fall back to the deterministic sentinel
        // "pi:default". Configured model is intentionally not treated as effective
        // unless the transcript/provider reports it.
        let model_id = extracted
            .model_id
            .clone()
            .or_else(|| Some("pi:default".to_string()));

        let runner_exit_kind = if matches!(
            output.killed_for,
            Some(LivenessClass::StalledNoOutput { .. })
        ) {
            "stalled_no_output"
        } else if exit_code == 0 {
            "ok"
        } else {
            "nonzero"
        };

        // Build telemetry from invocation-level data regardless of payload
        // validity — telemetry belongs to the invocation, not the payload.
        let mut telemetry = AgentRunTelemetry {
            model_id,
            harness_id: Some("pi".to_string()),
            started_at: Some(started_at),
            ended_at: Some(ended_at),
            tokens_in: extracted.tokens_in,
            tokens_out: extracted.tokens_out,
            prompt_cache_hits: extracted.prompt_cache_hits,
            transcript_path,
            stderr_log_path: Some(stderr_log_path.to_string_lossy().to_string()),
            configured_harness_id: Some("pi".to_string()),
            configured_model_id: self
                .configured_model
                .clone()
                .or_else(|| extracted.configured_model_id.clone()),
            configured_thinking_effort: self
                .configured_thinking
                .clone()
                .or_else(|| extracted.configured_thinking_effort.clone()),
            effective_model_id: extracted.model_id.clone(),
            effective_thinking_effort: extracted.effective_thinking_effort.clone(),
            thinking_effort_source: extracted.thinking_effort_source.clone(),
            provider_id: extracted.provider_id,
            api_id: extracted.api_id,
            session_id: Some(session_id.clone()),
            workspace_path: Some(cwd.to_string_lossy().to_string()),
            runner_exit_kind: Some(runner_exit_kind.to_string()),
            cache_read_tokens: extracted.prompt_cache_hits,
            cache_write_tokens: extracted.cache_write_tokens,
            cost_total: extracted.cost_total,
            ..AgentRunTelemetry::default()
        };
        if self.configured_thinking.is_some() && telemetry.effective_thinking_effort.is_none() {
            telemetry.effective_thinking_effort = Some("unknown".to_string());
            telemetry.thinking_effort_source = Some("unknown".to_string());
        }

        if runner_exit_kind == "stalled_no_output" {
            telemetry.payload_valid = Some(false);
            telemetry.payload_error = output.payload_error.clone();
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
            let payload_error = "pi helper exited 0 but did not emit final_output".to_string();
            telemetry.payload_valid = Some(false);
            telemetry.payload_error = Some(payload_error.clone());
            return Ok(RunnerOutput {
                stdout,
                stderr,
                exit_code,
                final_message: None,
                structured_output: None,
                session_id: Some(session_id),
                structured_output_source: None,
                telemetry,
                payload_error: Some(payload_error),
            });
        }
        if let (Some(s), Some(p)) = (schema, payload.as_ref()) {
            if let Err(e) = validate_payload(s, p) {
                let payload_error = format!("{e:#}");
                telemetry.payload_valid = Some(false);
                telemetry.payload_error = Some(payload_error.clone());
                return Ok(RunnerOutput {
                    stdout,
                    stderr,
                    exit_code,
                    final_message: None,
                    structured_output: None,
                    session_id: Some(session_id),
                    structured_output_source: None,
                    telemetry,
                    payload_error: Some(payload_error),
                });
            }
        }
        telemetry.payload_valid = Some(exit_code == 0 && payload.is_some());
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
        self.spawn_inner(
            role,
            system_prompt,
            brief,
            schema,
            workspace_path,
            None,
            &[],
        )
    }

    fn spawn_with_invocation(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
        invocation: Option<&RunnerInvocationContext>,
    ) -> Result<RunnerOutput> {
        self.spawn_inner(
            role,
            system_prompt,
            brief,
            schema,
            workspace_path,
            invocation,
            &[],
        )
    }

    fn spawn_with_invocation_and_env(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,
        workspace_path: Option<&str>,
        invocation: Option<&RunnerInvocationContext>,
        extra_env: &[(String, String)],
    ) -> Result<RunnerOutput> {
        self.spawn_inner(
            role,
            system_prompt,
            brief,
            schema,
            workspace_path,
            invocation,
            extra_env,
        )
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
    fn spawn_with_invocation_uses_discoverable_live_paths_and_stderr() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let runs = tempfile::tempdir().unwrap();
        let transcript = runs.path().join("known-session.jsonl");
        let stderr_log = runs.path().join("known-session.stderr.log");
        let invocation = RunnerInvocationContext {
            session_id: "known-session".to_string(),
            flat_transcript_path: transcript.clone(),
            stderr_log_path: stderr_log.clone(),
            events_path: runs.path().join("known-session/events.jsonl"),
            status_path: runs.path().join("known-session/status.json"),
        };
        let (_d, helper) = shim("#!/bin/sh\necho live-one\necho live-err >&2\necho '{\"type\":\"final_output\",\"payload\":{\"role\":\"executor\",\"summary\":\"ok\"}}'\n");
        let runner = PiRunner::with_bin_and_helper(PathBuf::from("/bin/sh"), helper);
        let schema = r#"{"type":"object","required":["role","summary"],"properties":{"role":{"const":"executor"},"summary":{"type":"string"}}}"#;
        let out = runner
            .spawn_with_invocation(
                "executor",
                "sys",
                "brief",
                Some(schema),
                Some(env!("CARGO_MANIFEST_DIR")),
                Some(&invocation),
            )
            .unwrap();
        assert_eq!(out.session_id.as_deref(), Some("known-session"));
        assert_eq!(
            out.telemetry.transcript_path.as_deref(),
            Some(transcript.to_str().unwrap())
        );
        assert_eq!(
            out.telemetry.stderr_log_path.as_deref(),
            Some(stderr_log.to_str().unwrap())
        );
        assert!(fs::read_to_string(&transcript)
            .unwrap()
            .contains("live-one"));
        assert!(fs::read_to_string(&stderr_log)
            .unwrap()
            .contains("live-err"));
        let events = fs::read_to_string(&invocation.events_path).unwrap();
        assert!(events.contains("\"type\":\"heartbeat\""), "{events}");
        assert!(events.contains("\"type\":\"final_output\""), "{events}");
        let status = fs::read_to_string(&invocation.status_path).unwrap();
        assert!(status.contains("last_event_at"), "{status}");
    }

    #[test]
    fn maps_pi_tool_and_assistant_events() {
        // Shape mirrors Pi SDK events forwarded by `session.subscribe` in the
        // installed pi-subagents foreground runner (`toolName`, `toolCallId`,
        // `args`, and `message` are the current event field names there).
        let events = map_pi_event(
            r#"{"type":"tool_execution_start","toolName":"bash","toolCallId":"t1","args":{"command":"echo hi"}}"#,
        );
        assert_eq!(events[0]["type"], "tool_start");
        assert_eq!(events[0]["name"], "bash");

        let events = map_pi_event(
            r#"{"type":"message_end","message":{"role":"assistant","content":[{"text":"hello"}],"usage":{"input_tokens":1,"output_tokens":2}}}"#,
        );
        assert!(events.iter().any(|e| e["type"] == "assistant_text"));
        assert!(events.iter().any(|e| e["type"] == "usage"));
    }

    #[test]
    fn extracts_nested_pi_message_telemetry() {
        let stdout = r#"{"type":"message_end","message":{"role":"assistant","model":"gpt-5.5","provider":{"id":"openai-codex"},"api":{"name":"openai-codex-responses"},"usage":{"input_tokens":11,"output_tokens":22,"cache_read_input_tokens":3,"cache_creation_input_tokens":4,"cost_total":0.25}}}
{"type":"final_output","payload":{"ok":true}}
"#;
        let telemetry = extract_pi_telemetry(stdout);
        assert_eq!(telemetry.model_id.as_deref(), Some("gpt-5.5"));
        assert_eq!(telemetry.provider_id.as_deref(), Some("openai-codex"));
        assert_eq!(telemetry.api_id.as_deref(), Some("openai-codex-responses"));
        assert_eq!(telemetry.tokens_in, Some(11));
        assert_eq!(telemetry.tokens_out, Some(22));
        assert_eq!(telemetry.prompt_cache_hits, Some(3));
        assert_eq!(telemetry.cache_write_tokens, Some(4));
        assert_eq!(telemetry.cost_total, Some(0.25));
    }

    #[test]
    fn extracts_stores_config_telemetry_event() {
        let stdout = r#"{"type":"stores_config","configured_model":"gpt-5.5","configured_thinking":"high","effective_model":"unknown","effective_thinking":"unknown","thinking_source":"unknown"}
{"type":"final_output","payload":{"ok":true}}
"#;
        let telemetry = extract_pi_telemetry(stdout);
        assert_eq!(telemetry.configured_model_id.as_deref(), Some("gpt-5.5"));
        assert_eq!(
            telemetry.configured_thinking_effort.as_deref(),
            Some("high")
        );
        assert_eq!(telemetry.model_id, None);
        assert_eq!(
            telemetry.effective_thinking_effort.as_deref(),
            Some("unknown")
        );
        assert_eq!(telemetry.thinking_effort_source.as_deref(), Some("unknown"));
    }

    #[test]
    fn configured_model_and_thinking_are_forwarded_to_sidecar_args() {
        let (_d, helper) = shim(
            "#!/bin/sh\nprintf '%s\\n' \"$@\"\necho '{\"type\":\"final_output\",\"payload\":{\"role\":\"executor\",\"summary\":\"ok\"}}'\n",
        );
        let runner = PiRunner::with_bin_helper_and_config(
            PathBuf::from("/bin/sh"),
            helper,
            Some("gpt-5.5".to_string()),
            Some("high".to_string()),
        );
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
        let lines = out.stdout.lines().collect::<Vec<_>>();
        assert!(
            lines.windows(2).any(|w| w == ["--model", "gpt-5.5"]),
            "{}",
            out.stdout
        );
        assert!(
            lines.windows(2).any(|w| w == ["--thinking", "high"]),
            "{}",
            out.stdout
        );
        assert_eq!(out.telemetry.configured_harness_id.as_deref(), Some("pi"));
        assert_eq!(
            out.telemetry.configured_model_id.as_deref(),
            Some("gpt-5.5")
        );
        assert_eq!(
            out.telemetry.configured_thinking_effort.as_deref(),
            Some("high")
        );
        assert_eq!(
            out.telemetry.effective_thinking_effort.as_deref(),
            Some("unknown")
        );
        assert_eq!(out.telemetry.session_id, out.session_id);
        assert_eq!(out.telemetry.runner_exit_kind.as_deref(), Some("ok"));
        assert_eq!(out.telemetry.payload_valid, Some(true));
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
        let transcript_path = out.telemetry.transcript_path.as_deref().unwrap();
        let transcript = std::fs::read_to_string(transcript_path).unwrap();
        assert_eq!(
            transcript, out.stdout,
            "flat transcript is live stdout transcript"
        );
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
    fn pi_runner_stderr_progress_extends_heartbeat() {
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
            "#!/bin/sh\necho stderr-one >&2\nsleep 0.3\necho stderr-two >&2\nsleep 0.3\necho '{\"type\":\"final_output\",\"payload\":{\"role\":\"executor\",\"summary\":\"ok\"}}'\n",
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
        assert_eq!(out.exit_code, 0);
        assert!(out.stderr.contains("stderr-two"));
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
