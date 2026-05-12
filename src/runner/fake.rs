use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;

use super::liveness::{self, LivenessThresholds};
use super::{
    AgentRunTelemetry, Runner, RunnerInvocationContext, RunnerLiveEventSink, RunnerOutput,
};

const FAKE_MODEL_ID: &str = "fake-random-v1";
const FAKE_PROVIDER_ID: &str = "stores-fake";
const FAKE_API_ID: &str = "stores-fake-agent-v1";

#[derive(Debug, Clone, Default)]
pub struct FakeRunner {
    bin: Option<PathBuf>,
}

impl FakeRunner {
    pub fn new() -> Self {
        Self { bin: None }
    }

    pub fn with_bin(path: PathBuf) -> Self {
        Self { bin: Some(path) }
    }

    fn resolve_bin(&self) -> Result<PathBuf> {
        if let Some(path) = &self.bin {
            return Ok(path.clone());
        }
        if let Some(path) = std::env::var_os("STORES_FAKE_AGENT_BIN") {
            if !path.is_empty() {
                return Ok(PathBuf::from(path));
            }
        }
        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(sibling) = sibling_fake_agent(&current_exe) {
                return Ok(sibling);
            }
        }
        Ok(PathBuf::from("stores-fake-agent"))
    }

    fn spawn_inner(
        &self,
        role: &str,
        _system_prompt: &str,
        brief: &str,
        _schema: Option<&str>,
        workspace_path: Option<&str>,
        invocation: Option<&RunnerInvocationContext>,
        extra_env: &[(String, String)],
    ) -> Result<RunnerOutput> {
        let session_id = invocation
            .map(|i| i.session_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let cwd = match workspace_path {
            Some(p) => PathBuf::from(p)
                .canonicalize()
                .with_context(|| format!("workspace_path canonicalize failed: '{p}'"))?,
            None => std::env::current_dir().context("resolve current dir for fake runner")?,
        };
        let invocation_owned = match invocation {
            Some(i) => i.clone(),
            None => default_invocation(&cwd, &session_id)?,
        };

        let bin = self.resolve_bin()?;
        let mut cmd = Command::new(&bin);
        cmd.current_dir(&cwd);
        // Caller-provided env is applied first. The runner-owned context below
        // intentionally wins for STORES_FAKE_* identity/path keys so a caller
        // cannot accidentally desynchronize the child from RunnerInvocationContext.
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        cmd.env("STORES_FAKE_ROLE", role)
            .env("STORES_FAKE_SESSION_ID", &invocation_owned.session_id)
            .env(
                "STORES_FAKE_TRANSCRIPT_PATH",
                &invocation_owned.flat_transcript_path,
            )
            .env("STORES_FAKE_EVENTS_PATH", &invocation_owned.events_path)
            .env("STORES_FAKE_STATUS_PATH", &invocation_owned.status_path)
            .env(
                "STORES_FAKE_STDERR_LOG_PATH",
                &invocation_owned.stderr_log_path,
            )
            .env("STORES_FAKE_WORKSPACE_PATH", &cwd)
            .env("STORES_FAKE_BRIEF_LEN", brief.len().to_string());

        let transcript = open_file(&invocation_owned.flat_transcript_path, "transcript")?;
        let stderr_log = open_file(&invocation_owned.stderr_log_path, "stderr log")?;
        let event_sink = RunnerLiveEventSink::open(&invocation_owned)?;
        let started_at = crate::handlers::row::now_iso8601();

        let transcript_rc = Rc::new(RefCell::new(transcript));
        let stderr_rc = Rc::new(RefCell::new(stderr_log));
        let stdout_transcript = Rc::clone(&transcript_rc);
        let stderr_file = Rc::clone(&stderr_rc);
        let stdout_events = Rc::clone(&event_sink);
        let stderr_events = Rc::clone(&event_sink);

        let output = liveness::run_streaming_with_liveness(
            &mut cmd,
            &LivenessThresholds::from_env(),
            move |line| {
                append_line(&stdout_transcript, line)?;
                stdout_events
                    .borrow_mut()
                    .record_stdout_line(line, map_fake_stream_event(line))?;
                Ok(())
            },
            move |line| {
                append_line(&stderr_file, line)?;
                stderr_events.borrow_mut().record_stderr_line(line)?;
                Ok(())
            },
        )
        .with_context(|| format!("failed to launch fake runner `{}`", bin.display()))?;
        let ended_at = crate::handlers::row::now_iso8601();

        let stdout = output.stdout;
        let stderr = output.stderr;
        let exit_code = output.exit_code;
        let (structured_output, final_message) = extract_fake_structured_output(&stdout);
        let payload_valid = exit_code == 0 && structured_output.is_some();
        let payload_error = if exit_code == 0 && structured_output.is_none() {
            Some("fake runner produced no result.structured_output".to_string())
        } else {
            None
        };

        Ok(RunnerOutput {
            stdout,
            stderr,
            exit_code,
            final_message,
            structured_output,
            session_id: Some(invocation_owned.session_id),
            structured_output_source: if payload_valid { Some("fake") } else { None },
            telemetry: AgentRunTelemetry {
                model_id: Some(FAKE_MODEL_ID.to_string()),
                harness_id: Some("fake".to_string()),
                started_at: Some(started_at),
                ended_at: Some(ended_at),
                tokens_in: Some(0),
                tokens_out: Some(0),
                prompt_cache_hits: Some(0),
                transcript_path: Some(
                    invocation_owned
                        .flat_transcript_path
                        .to_string_lossy()
                        .to_string(),
                ),
                stderr_log_path: Some(
                    invocation_owned
                        .stderr_log_path
                        .to_string_lossy()
                        .to_string(),
                ),
                effective_model_id: Some(FAKE_MODEL_ID.to_string()),
                effective_thinking_effort: Some("none".to_string()),
                thinking_effort_source: Some("fake".to_string()),
                provider_id: Some(FAKE_PROVIDER_ID.to_string()),
                api_id: Some(FAKE_API_ID.to_string()),
                session_id: Some(session_id),
                workspace_path: Some(cwd.to_string_lossy().to_string()),
                runner_exit_kind: Some(if exit_code == 0 { "ok" } else { "nonzero" }.to_string()),
                payload_valid: Some(payload_valid),
                payload_error: payload_error.clone(),
                cache_read_tokens: Some(0),
                cache_write_tokens: Some(0),
                cost_total: Some(0.0),
                ..AgentRunTelemetry::default()
            },
            payload_error,
        })
    }
}

impl Runner for FakeRunner {
    fn name(&self) -> &str {
        "fake"
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

fn sibling_fake_agent(current_exe: &Path) -> Option<PathBuf> {
    let sibling = current_exe.parent()?.join("stores-fake-agent");
    sibling.exists().then_some(sibling)
}

fn default_invocation(cwd: &Path, session_id: &str) -> Result<RunnerInvocationContext> {
    let runs_dir = std::env::var_os("STORES_RUNS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.join(".stores").join("runs"));
    std::fs::create_dir_all(&runs_dir)
        .with_context(|| format!("creating fake runner runs dir: {}", runs_dir.display()))?;
    let flat_transcript_path = runs_dir.join(format!("{session_id}.jsonl"));
    let stderr_log_path = runs_dir.join(format!("{session_id}.stderr.log"));
    let events_path = super::events_path_for_transcript(&flat_transcript_path, session_id);
    let status_path = super::status_path_for_transcript(&flat_transcript_path, session_id);
    Ok(RunnerInvocationContext {
        session_id: session_id.to_string(),
        flat_transcript_path,
        stderr_log_path,
        events_path,
        status_path,
    })
}

fn open_file(path: &Path, label: &str) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating fake runner {label} dir: {}", parent.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("opening fake runner {label}: {}", path.display()))
}

fn append_line(file: &Rc<RefCell<File>>, line: &str) -> Result<()> {
    let mut f = file.borrow_mut();
    writeln!(f, "{line}")?;
    f.flush()?;
    Ok(())
}

fn map_fake_stream_event(line: &str) -> Vec<Value> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return vec![];
    };
    match v.get("type").and_then(|t| t.as_str()) {
        // RunnerLiveEventSink records a generic stdout heartbeat for every line;
        // do not add a second mapped heartbeat for fake heartbeat lines.
        Some("fake_heartbeat") => vec![],
        Some("assistant") => vec![json!({"type":"assistant_text","source":"stores-fake"})],
        Some("result") => vec![json!({"type":"final_output","source":"stores-fake"})],
        _ => vec![json!({"type":"event","source":"stores-fake"})],
    }
}

fn extract_fake_structured_output(stdout: &str) -> (Option<Value>, Option<String>) {
    let mut structured = None;
    let mut final_message = None;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("result") {
            if let Some(value) = v.get("structured_output") {
                structured = Some(value.clone());
            }
            final_message = v
                .get("result")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
        }
    }
    (structured, final_message)
}

pub fn fake_payload_for_role(role: &str) -> Result<Value> {
    match role {
        "planner" => Ok(json!({
            "role": "planner",
            "summary": "FAKE planner produced a one-phase test plan.",
            "phases": [{
                "name": "Fake execution",
                "objective": "Exercise the stores drive lifecycle without LLM calls.",
                "tasks": ["Run the fake executor"],
                "acceptance_criteria": ["Fake code review passes"],
                "files": [],
                "dependencies": []
            }],
            "decision_matrix": []
        })),
        "plan-reviewer" | "plan_reviewer" => Ok(json!({
            "role": "plan-reviewer",
            "gate": "READY",
            "summary": "FAKE plan review approved the synthetic plan.",
            "open_questions": []
        })),
        "executor" => Ok(json!({
            "role": "executor",
            "summary": "FAKE executor completed without modifying files.",
            "commit": null,
            "files_changed": []
        })),
        "code-reviewer" | "code_reviewer" => Ok(json!({
            "role": "code-reviewer",
            "gate": "PASS",
            "summary": "FAKE code review passed the synthetic execution.",
            "details": "stores-fake-agent generated this review; it is not model-quality evidence.",
            "counts": {"critical": 0, "major": 0, "minor": 0}
        })),
        "wrap" => Ok(json!({
            "role": "wrap",
            "reasoning": "FAKE wrap output for no-LLM dogfood.",
            "executive_summary": "FAKE wrap completed; this is not real review evidence.",
            "deviations": [],
            "residual_risks": ["Fake PASS does not imply the task is shippable."],
            "recommended_sanity_checks": ["Run a real review before accepting production work."]
        })),
        other => bail!("stores-fake-agent does not know role '{other}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_payloads_have_expected_roles() {
        let cases = [
            ("planner", "planner"),
            ("plan-reviewer", "plan-reviewer"),
            ("executor", "executor"),
            ("code-reviewer", "code-reviewer"),
            ("wrap", "wrap"),
        ];
        for (role, expected) in cases {
            let payload = fake_payload_for_role(role).unwrap();
            assert_eq!(payload.get("role").and_then(|v| v.as_str()), Some(expected));
        }
    }

    #[test]
    fn sibling_fake_agent_resolution_uses_current_exe_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("stores");
        let sibling = tmp.path().join("stores-fake-agent");
        std::fs::write(&current, "").unwrap();
        std::fs::write(&sibling, "").unwrap();
        assert_eq!(
            sibling_fake_agent(&current).as_deref(),
            Some(sibling.as_path())
        );
    }

    #[test]
    fn fake_runner_binary_override_works() {
        let tmp = tempfile::tempdir().unwrap();
        let shim = tmp.path().join("fake-agent.sh");
        std::fs::write(
            &shim,
            "#!/usr/bin/env bash\nprintf '%s\\n' '{\"type\":\"result\",\"structured_output\":{\"role\":\"executor\",\"summary\":\"ok\",\"files_changed\":[]},\"result\":\"{}\"}'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&shim).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&shim, perms).unwrap();
        }
        let runner = FakeRunner::with_bin(shim);
        let out = runner
            .spawn("executor", "", "", None, Some(tmp.path().to_str().unwrap()))
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.structured_output.as_ref().unwrap()["role"], "executor");
        assert_eq!(out.telemetry.harness_id.as_deref(), Some("fake"));
        assert_eq!(out.telemetry.provider_id.as_deref(), Some("stores-fake"));
    }
}
