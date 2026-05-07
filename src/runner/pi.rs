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
use std::path::PathBuf;
use std::process::{Command, Output};

use super::{Runner, RunnerOutput};

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

fn write_transcript(cwd: &PathBuf, session_id: &str, stdout: &str) {
    let runs_dir = std::env::var_os("STORES_RUNS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.join(".stores").join("runs"));
    if fs::create_dir_all(&runs_dir).is_ok() {
        let _ = fs::write(runs_dir.join(format!("{session_id}.jsonl")), stdout);
    }
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

fn output_with_etxtbsy_retry(cmd: &mut Command) -> std::io::Result<Output> {
    let mut last_err = None;
    for _ in 0..5 {
        match cmd.output() {
            Ok(output) => return Ok(output),
            Err(e) if e.raw_os_error() == Some(26) => {
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.expect("ETXTBSY retry loop records last error"))
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
        let output = output_with_etxtbsy_retry(&mut cmd).context("failed to launch pi helper; ensure node and @mariozechner/pi-coding-agent are available")?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);
        write_transcript(&cwd, &session_id, &stdout);

        let payload = extract_final_output(&stdout);
        if exit_code == 0 && payload.is_none() {
            bail!("pi helper exited 0 but did not emit final_output");
        }
        if let (Some(s), Some(p)) = (schema, payload.as_ref()) {
            validate_payload(s, p)?;
        }
        Ok(RunnerOutput {
            stdout,
            stderr,
            exit_code,
            final_message: None,
            structured_output: payload,
            session_id: Some(session_id),
            structured_output_source: Some("pi-tool"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn shim(script: &str) -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("shim.sh");
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
        let (_d, helper) = shim(
            "#!/bin/sh\necho '{\"type\":\"final_output\",\"payload\":{\"role\":\"executor\"}}'\n",
        );
        let runner = PiRunner::with_bin_and_helper(PathBuf::from("/bin/sh"), helper);
        let schema = r#"{"type":"object","required":["summary"],"properties":{"summary":{"type":"string"}}}"#;
        let err = runner
            .spawn(
                "executor",
                "sys",
                "brief",
                Some(schema),
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap_err();
        assert!(err.to_string().contains("schema validation"));
    }

    #[test]
    fn missing_final_tool_call_errors_when_helper_exits_zero() {
        let (_d, helper) =
            shim("#!/bin/sh\necho '{\"type\":\"message\",\"text\":\"done\"}'\nexit 0\n");
        let runner = PiRunner::with_bin_and_helper(PathBuf::from("/bin/sh"), helper);
        let err = runner
            .spawn(
                "planner",
                "sys",
                "brief",
                None,
                Some(env!("CARGO_MANIFEST_DIR")),
            )
            .unwrap_err();
        assert!(err.to_string().contains("did not emit final_output"));
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
    fn writes_transcript() {
        let runs = tempfile::tempdir().unwrap();
        std::env::set_var("STORES_RUNS_DIR", runs.path());
        let sid = "pi-transcript-test";
        write_transcript(
            &PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            sid,
            "{\"type\":\"final_output\"}\n",
        );
        let text = fs::read_to_string(runs.path().join(format!("{sid}.jsonl"))).unwrap();
        assert!(text.contains("final_output"));
    }
}
