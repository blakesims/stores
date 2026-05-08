//! Codex CLI runner for substrate-native external review.
//!
//! Preserves the current review invocation shape: `codex exec` with bypassed
//! sandbox/approval flags, `--color never`, and the review prompt on stdin.

use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::{AgentRunTelemetry, Runner, RunnerOutput};

#[derive(Debug, Clone)]
pub struct CodexRunner {
    bin: PathBuf,
    args: Vec<String>,
    model: Option<String>,
    timeout_secs: u64,
}

impl CodexRunner {
    pub fn new() -> Self {
        Self {
            bin: PathBuf::from("codex"),
            args: default_codex_args(),
            model: None,
            timeout_secs: 1800,
        }
    }

    pub fn with_config(
        bin: impl Into<PathBuf>,
        args: Vec<String>,
        model: Option<String>,
        timeout_secs: u64,
    ) -> Self {
        Self {
            bin: bin.into(),
            args,
            model,
            timeout_secs,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_bin(mut self, bin: PathBuf) -> Self {
        self.bin = bin;
        self
    }
}

impl Default for CodexRunner {
    fn default() -> Self {
        Self::new()
    }
}

pub fn default_codex_args() -> Vec<String> {
    vec![
        "exec".to_string(),
        "--dangerously-bypass-approvals-and-sandbox".to_string(),
        "--color".to_string(),
        "never".to_string(),
    ]
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

fn runs_dir(cwd: &std::path::Path) -> PathBuf {
    std::env::var_os("STORES_RUNS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.join(".stores").join("runs"))
}

fn write_run_file(
    cwd: &std::path::Path,
    session_id: &str,
    suffix: &str,
    content: &str,
) -> Result<PathBuf> {
    let dir = runs_dir(cwd);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let path = dir.join(format!("{session_id}{suffix}"));
    fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let status = child.wait()?;
            return Ok(status);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn read_pipe_thread<R>(mut pipe: R) -> thread::JoinHandle<Result<Vec<u8>>>
where
    R: std::io::Read + Send + 'static,
{
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}

impl Runner for CodexRunner {
    fn name(&self) -> &str {
        "codex"
    }

    fn spawn(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        _schema: Option<&str>,
        workspace_path: Option<&str>,
    ) -> Result<RunnerOutput> {
        let session_id = uuid::Uuid::new_v4().to_string();
        let cwd = resolve_cwd(workspace_path)?;
        let prompt = if system_prompt.is_empty() {
            brief.to_string()
        } else {
            format!("{system_prompt}\n\n{brief}")
        };

        let mut cmd = Command::new(&self.bin);
        cmd.current_dir(&cwd)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(model) = &self.model {
            cmd.arg("--model").arg(model);
        }

        let started_at = crate::handlers::row::now_iso8601();
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to launch `{}`", self.bin.display()))?;
        child
            .stdin
            .as_mut()
            .context("codex stdin unavailable")?
            .write_all(prompt.as_bytes())?;
        drop(child.stdin.take());

        let stdout_handle = child.stdout.take().map(read_pipe_thread);
        let stderr_handle = child.stderr.take().map(read_pipe_thread);
        let status = wait_with_timeout(&mut child, Duration::from_secs(self.timeout_secs))?;
        let ended_at = crate::handlers::row::now_iso8601();
        let stdout_bytes = stdout_handle
            .map(|h| h.join().unwrap_or_else(|_| Ok(Vec::new())))
            .transpose()?
            .unwrap_or_default();
        let stderr_bytes = stderr_handle
            .map(|h| h.join().unwrap_or_else(|_| Ok(Vec::new())))
            .transpose()?
            .unwrap_or_default();
        let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
        let transcript_path = write_run_file(&cwd, &session_id, ".codex.transcript.log", &stdout)?;
        let log_path = write_run_file(&cwd, &session_id, ".codex.stderr.log", &stderr)?;

        Ok(RunnerOutput {
            stdout,
            stderr,
            exit_code: status.code().unwrap_or(-1),
            final_message: None,
            structured_output: None,
            session_id: Some(session_id),
            structured_output_source: None,
            telemetry: AgentRunTelemetry {
                model_id: Some(
                    self.model
                        .clone()
                        .unwrap_or_else(|| "codex:default".to_string()),
                ),
                harness_id: Some(format!("codex:{}", role)),
                started_at: Some(started_at),
                ended_at: Some(ended_at),
                transcript_path: Some(transcript_path.to_string_lossy().to_string()),
                prompt_cache_hits: None,
                tokens_in: None,
                tokens_out: None,
                stderr_log_path: Some(log_path.to_string_lossy().to_string()),
            },
            payload_error: if status.success() {
                None
            } else {
                Some(format!(
                    "codex exited with status {} (log: {})",
                    status,
                    log_path.display()
                ))
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn shim(script: &str) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("codex-shim.sh");
        fs::write(&p, script).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        (d, p)
    }

    #[test]
    fn synthetic_shim_receives_stdin_and_writes_transcript() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let runs = tempfile::tempdir().unwrap();
        let old = std::env::var_os("STORES_RUNS_DIR");
        std::env::set_var("STORES_RUNS_DIR", runs.path());
        let (_d, bin) =
            shim("#!/bin/sh\ncat - > /dev/null\necho 'VERDICT: PASS'\necho 'log line' >&2\n");
        let runner = CodexRunner::new().with_bin(bin);
        let out = runner
            .spawn(
                "external-review",
                "sys",
                "brief",
                None,
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert!(out.stdout.contains("VERDICT: PASS"));
        let transcript = out.telemetry.transcript_path.unwrap();
        assert!(std::path::Path::new(&transcript).exists());
        match old {
            Some(v) => std::env::set_var("STORES_RUNS_DIR", v),
            None => std::env::remove_var("STORES_RUNS_DIR"),
        }
    }
}
