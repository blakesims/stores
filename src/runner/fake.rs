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
const DEFAULT_SCENARIO: &str = "all-pass";
const ENV_CONFIGURED_HARNESS: &str = "STORES_FAKE_CONFIGURED_HARNESS_ID";
const ENV_CONFIGURED_MODEL: &str = "STORES_FAKE_CONFIGURED_MODEL_ID";
const ENV_CONFIGURED_THINKING: &str = "STORES_FAKE_CONFIGURED_THINKING_EFFORT";

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

        let configured_harness = env_value(extra_env, ENV_CONFIGURED_HARNESS);
        let configured_model = env_value(extra_env, ENV_CONFIGURED_MODEL);
        let configured_thinking = env_value(extra_env, ENV_CONFIGURED_THINKING);

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
        let (structured_output, final_message, outcome_class, decision_outcome) =
            extract_fake_structured_output(&stdout);
        let legacy_payload_ok = matches!(outcome_class.as_deref(), Some("legacy"));
        let payload_valid = exit_code == 0 && (structured_output.is_some() || legacy_payload_ok);
        let payload_error = output.payload_error.clone().or_else(|| {
            if exit_code == 0 && !payload_valid {
                Some(
                    "class=payload fake runner produced invalid/missing structured output"
                        .to_string(),
                )
            } else {
                None
            }
        });
        let runner_exit_kind = if output.killed_for.is_some() {
            match decision_outcome.as_deref() {
                Some("SIGTERM_IGNORE_STALL") => "signal_sigkill_required",
                _ => "liveness_stalled_no_output",
            }
        } else if exit_code == 0 && payload_error.is_some() {
            "payload_invalid"
        } else if exit_code == 0 {
            "ok"
        } else {
            "infra_nonzero"
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
                configured_harness_id: configured_harness,
                configured_model_id: configured_model,
                configured_thinking_effort: configured_thinking,
                effective_model_id: Some(FAKE_MODEL_ID.to_string()),
                effective_thinking_effort: Some("none".to_string()),
                thinking_effort_source: Some("fake".to_string()),
                provider_id: Some(FAKE_PROVIDER_ID.to_string()),
                api_id: Some(FAKE_API_ID.to_string()),
                session_id: Some(session_id),
                workspace_path: Some(cwd.to_string_lossy().to_string()),
                runner_exit_kind: Some(runner_exit_kind.to_string()),
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

fn env_value(extra_env: &[(String, String)], key: &str) -> Option<String> {
    extra_env
        .iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .filter(|v| !v.is_empty())
}

fn map_fake_stream_event(line: &str) -> Vec<Value> {
    let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
        return vec![];
    };
    match v.get("type").and_then(|t| t.as_str()) {
        // RunnerLiveEventSink records a generic stdout heartbeat for every line;
        // do not add a second mapped heartbeat for fake heartbeat lines.
        Some("fake_heartbeat") => vec![],
        Some("fake_decision") => vec![v],
        Some("assistant") => vec![json!({"type":"assistant_text","source":"stores-fake"})],
        Some("result") => vec![json!({"type":"final_output","source":"stores-fake"})],
        _ => vec![json!({"type":"event","source":"stores-fake"})],
    }
}

fn extract_fake_structured_output(
    stdout: &str,
) -> (
    Option<Value>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let mut structured = None;
    let mut final_message = None;
    let mut outcome_class = None;
    let mut decision_outcome = None;
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) == Some("fake_decision") {
            outcome_class = v
                .get("class")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
            decision_outcome = v
                .get("outcome")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string());
        }
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
    (structured, final_message, outcome_class, decision_outcome)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeDecision {
    pub scenario: String,
    pub seed: String,
    pub task_id: String,
    pub role: String,
    pub phase: String,
    pub cycle: String,
    pub attempt: String,
    pub policy_hash: String,
    pub roll: u64,
    pub threshold: u64,
    pub outcome: &'static str,
    pub outcome_class: &'static str,
}

pub fn fake_decision_event(decision: &FakeDecision) -> Value {
    json!({
        "type": "fake_decision",
        "task_id": decision.task_id,
        "role": decision.role,
        "phase": decision.phase,
        "cycle": decision.cycle,
        "attempt": decision.attempt,
        "scenario": decision.scenario,
        "seed": decision.seed,
        "policy_hash": decision.policy_hash,
        "roll": decision.roll,
        "threshold": decision.threshold,
        "outcome": decision.outcome,
        "class": decision.outcome_class
    })
}

pub fn decide_fake_outcome(
    scenario: &str,
    seed: &str,
    task_id: &str,
    role: &str,
    phase: &str,
    cycle: &str,
    attempt: &str,
) -> FakeDecision {
    let scenario = scenario_env_override(scenario);
    let policy_hash = fake_policy_hash(&scenario, seed);
    let roll = deterministic_roll(
        &scenario,
        seed,
        task_id,
        role,
        phase,
        cycle,
        attempt,
        &policy_hash,
    );
    let first = ordinal(attempt).or_else(|| ordinal(cycle)).unwrap_or(1) <= 1;
    let normalized_role = role.replace('_', "-");
    let (outcome, outcome_class) = match scenario.as_str() {
        "all-pass" => ("PASS", "semantic"),
        "plan-review-reject-once" | "plan-reviewer-reject-once" => {
            if normalized_role == "plan-reviewer" && first {
                ("NEEDS_WORK", "semantic")
            } else {
                ("PASS", "semantic")
            }
        }
        "review-revise-once" | "code-review-revise-once" => {
            if normalized_role == "code-reviewer" && first {
                ("REVISE", "semantic")
            } else {
                ("PASS", "semantic")
            }
        }
        "external-review-revise-once" => {
            if normalized_role == "external-review" && first {
                ("REVISE", "semantic")
            } else {
                ("PASS", "semantic")
            }
        }
        "external-review-tooling-failure" => {
            if normalized_role == "external-review" {
                ("TOOLING_FAILURE", "tooling")
            } else {
                ("PASS", "semantic")
            }
        }
        "payload-invalid-exit-0" => ("PAYLOAD_INVALID_EXIT_0", "payload"),
        "nonzero-exit" => ("NONZERO_EXIT", "infra"),
        "long-delay-heartbeat" => ("PASS", "liveness"),
        "stall-no-heartbeat" => ("STALL_NO_HEARTBEAT", "liveness"),
        "sigterm-ignoring-stall" => ("SIGTERM_IGNORE_STALL", "signal"),
        "messy-prose-legacy-output" => ("MESSY_LEGACY_OUTPUT", "legacy"),
        _ => ("PASS", "semantic"),
    };
    FakeDecision {
        scenario,
        seed: seed.to_string(),
        task_id: task_id.to_string(),
        role: role.to_string(),
        phase: phase.to_string(),
        cycle: cycle.to_string(),
        attempt: attempt.to_string(),
        policy_hash,
        roll,
        threshold: 10_000,
        outcome,
        outcome_class,
    }
}

fn scenario_env_override(configured: &str) -> String {
    std::env::var("STORES_FAKE_SCENARIO")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if configured.trim().is_empty() {
                DEFAULT_SCENARIO.to_string()
            } else {
                configured.to_string()
            }
        })
}

fn fake_policy_hash(scenario: &str, seed: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"stores-fake-agent-v1\0");
    hasher.update(scenario.as_bytes());
    hasher.update(b"\0seed=");
    hasher.update(seed.as_bytes());
    hasher.update(b"\0delay=");
    hasher.update(
        std::env::var("STORES_FAKE_DELAY_MS")
            .unwrap_or_default()
            .as_bytes(),
    );
    hasher.update(b"\0heartbeat=");
    hasher.update(
        std::env::var("STORES_FAKE_HEARTBEAT_INTERVAL_MS")
            .unwrap_or_default()
            .as_bytes(),
    );
    let digest = hasher.finalize();
    format!("sha256:{:x}", digest)
}

fn deterministic_roll(
    scenario: &str,
    seed: &str,
    task_id: &str,
    role: &str,
    phase: &str,
    cycle: &str,
    attempt: &str,
    policy_hash: &str,
) -> u64 {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in [
        scenario,
        seed,
        task_id,
        role,
        phase,
        cycle,
        attempt,
        policy_hash,
    ] {
        hasher.update(part.as_bytes());
        hasher.update(b"\0");
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes) % 10_000
}

fn ordinal(value: &str) -> Option<i64> {
    let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<i64>().ok()
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
        "external-review" | "external_review" => Ok(json!({
            "verdict": "PASS",
            "critical_count": 0,
            "major_count": 0,
            "minor_count": 0,
            "counts": {"critical": 0, "major": 0, "minor": 0},
            "findings": [],
            "summary": "FAKE external review PASS; this is not real review evidence."
        })),
        other => bail!("stores-fake-agent does not know role '{other}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built_fake_agent_bin() -> std::path::PathBuf {
        if let Some(path) = option_env!("CARGO_BIN_EXE_stores-fake-agent") {
            return std::path::PathBuf::from(path);
        }
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        if path.file_name().and_then(|s| s.to_str()) == Some("deps") {
            path.pop();
        }
        path.push("stores-fake-agent");
        path
    }

    struct EnvRestore {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, old }
        }

        fn unset(key: &'static str) -> Self {
            let old = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, old }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match self.old.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

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
        let er = fake_payload_for_role("external-review").unwrap();
        assert_eq!(er.get("verdict").and_then(|v| v.as_str()), Some("PASS"));
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
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let _scenario = EnvRestore::unset("STORES_FAKE_SCENARIO");
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

    #[test]
    fn scripted_decisions_are_deterministic_for_same_context() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let _scenario = EnvRestore::unset("STORES_FAKE_SCENARIO");
        let a = decide_fake_outcome(
            "code-review-revise-once",
            "seed-a",
            "TDET",
            "code-reviewer",
            "1",
            "1",
            "1",
        );
        let b = decide_fake_outcome(
            "code-review-revise-once",
            "seed-a",
            "TDET",
            "code-reviewer",
            "1",
            "1",
            "1",
        );
        assert_eq!(fake_decision_event(&a), fake_decision_event(&b));
        assert_eq!(a.outcome, "REVISE");
        assert_eq!(a.outcome_class, "semantic");
        assert!(a.policy_hash.starts_with("sha256:"));
    }

    #[test]
    fn fake_runner_classifies_scripted_failure_classes() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let bin = built_fake_agent_bin();
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_str().unwrap();

        let cases = [
            (
                "payload-invalid-exit-0",
                "executor",
                0,
                Some("payload_invalid"),
                Some(false),
                None,
            ),
            (
                "nonzero-exit",
                "executor",
                42,
                Some("infra_nonzero"),
                Some(false),
                None,
            ),
            (
                "plan-reviewer-reject-once",
                "plan-reviewer",
                0,
                Some("ok"),
                Some(true),
                Some(("gate", "NEEDS_WORK")),
            ),
            (
                "code-review-revise-once",
                "code-reviewer",
                0,
                Some("ok"),
                Some(true),
                Some(("gate", "REVISE")),
            ),
            (
                "external-review-revise-once",
                "external-review",
                0,
                Some("ok"),
                Some(true),
                Some(("verdict", "REVISE")),
            ),
            (
                "external-review-tooling-failure",
                "external-review",
                0,
                Some("ok"),
                Some(true),
                Some(("verdict", "TOOLING_FAILURE")),
            ),
        ];
        for (scenario, role, exit_code, exit_kind, payload_valid, semantic_field) in cases {
            let _scenario = EnvRestore::set("STORES_FAKE_SCENARIO", scenario);
            let out = FakeRunner::with_bin(bin.clone())
                .spawn(role, "", "", None, Some(workspace))
                .unwrap();
            assert_eq!(out.exit_code, exit_code, "scenario {scenario}");
            assert_eq!(
                out.telemetry.runner_exit_kind.as_deref(),
                exit_kind,
                "scenario {scenario}"
            );
            assert_eq!(
                out.telemetry.payload_valid, payload_valid,
                "scenario {scenario}"
            );
            if let Some((field, expected)) = semantic_field {
                assert_eq!(
                    out.structured_output
                        .as_ref()
                        .and_then(|v| v.get(field))
                        .and_then(|v| v.as_str()),
                    Some(expected),
                    "scenario {scenario}"
                );
            }
            assert!(
                out.stdout.contains("\"type\":\"fake_decision\""),
                "scenario {scenario}"
            );
        }

        let _scenario = EnvRestore::set("STORES_FAKE_SCENARIO", "messy-prose-legacy-output");
        let out = FakeRunner::with_bin(bin)
            .spawn("executor", "", "", None, Some(workspace))
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(out.telemetry.runner_exit_kind.as_deref(), Some("ok"));
        assert_eq!(out.telemetry.payload_valid, Some(true));
        assert!(out.structured_output.is_none());
        assert!(
            out.final_message
                .as_deref()
                .unwrap_or("")
                .contains("```json"),
            "messy legacy output should leave SAP-parsable prose in final_message"
        );
    }

    #[test]
    fn fake_runner_liveness_and_signal_stalls_hit_watchdog_path() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let _delay = EnvRestore::set("STORES_FAKE_DELAY_MS", "5000");
        let _no_output = EnvRestore::set("STORES_RUNNER_NO_OUTPUT_SECS", "0");
        let _wall = EnvRestore::set("STORES_RUNNER_WALL_CLOCK_MAX_SECS", "30");
        let tmp = tempfile::tempdir().unwrap();
        for (scenario, kind) in [
            ("stall-no-heartbeat", "liveness_stalled_no_output"),
            ("sigterm-ignoring-stall", "signal_sigkill_required"),
        ] {
            let _scenario = EnvRestore::set("STORES_FAKE_SCENARIO", scenario);
            let out = FakeRunner::with_bin(built_fake_agent_bin())
                .spawn("executor", "", "", None, Some(tmp.path().to_str().unwrap()))
                .unwrap();
            assert_eq!(out.exit_code, -1, "scenario {scenario}");
            assert_eq!(out.telemetry.runner_exit_kind.as_deref(), Some(kind));
            assert!(
                out.payload_error
                    .as_deref()
                    .unwrap_or("")
                    .contains("timed out"),
                "scenario {scenario}"
            );
        }
    }
}
