use anyhow::{bail, Context, Result};
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

fn main() {
    if let Err(err) = run() {
        eprintln!("stores-fake-agent: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let role = std::env::var("STORES_FAKE_ROLE").unwrap_or_else(|_| "executor".to_string());
    let task_id = std::env::var("STORES_FAKE_TASK_ID").unwrap_or_else(|_| "unknown".into());
    let phase = std::env::var("STORES_FAKE_PHASE").unwrap_or_else(|_| "unknown".into());
    let cycle = std::env::var("STORES_FAKE_CYCLE").unwrap_or_else(|_| "unknown".into());
    let attempt = std::env::var("STORES_FAKE_ATTEMPT").unwrap_or_else(|_| "unknown".into());
    let session_id = std::env::var("STORES_FAKE_SESSION_ID").unwrap_or_else(|_| "unknown".into());
    let mut delay_ms = std::env::var("STORES_FAKE_DELAY_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(5000);
    let seed = std::env::var("STORES_FAKE_SEED").unwrap_or_else(|_| "0".to_string());
    let scenario = std::env::var("STORES_FAKE_SCENARIO").unwrap_or_else(|_| "all-pass".to_string());
    let canonical_scenario = stores::runner::fake::canonical_fake_scenario(&scenario);
    let model_id = stores::runner::fake::fake_model_id_for_scenario(&canonical_scenario);
    if matches!(
        canonical_scenario.as_str(),
        "long-delay-heartbeat" | "stall-no-heartbeat" | "sigterm-ignoring-stall"
    ) && std::env::var_os("STORES_FAKE_DELAY_MS").is_none()
    {
        delay_ms = 2_000;
    }
    let decision = stores::runner::fake::decide_fake_outcome(
        &canonical_scenario,
        &seed,
        &task_id,
        &role,
        &phase,
        &cycle,
        &attempt,
    );
    let workspace = std::env::var_os("STORES_FAKE_WORKSPACE_PATH")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let git_start_sha = git_head(&workspace).ok();

    emit(json!({
        "type": "system",
        "subtype": "stores_fake_start",
        "session_id": session_id,
        "task_id": task_id,
        "role": role,
        "phase": phase,
        "cycle": cycle,
        "attempt": attempt,
        "scenario": decision.scenario,
        "seed": seed,
        "model": &model_id,
        "provider": "stores-fake",
        "api": "stores-fake-agent-v1",
        "git_start_sha": git_start_sha
    }))?;

    emit(stores::runner::fake::fake_decision_event(&decision))?;

    if matches!(
        decision.outcome,
        "STALL_NO_HEARTBEAT" | "SIGTERM_IGNORE_STALL"
    ) {
        controlled_stall(decision.outcome == "SIGTERM_IGNORE_STALL", delay_ms)?;
        return Ok(());
    }

    emit(json!({
        "type": "assistant",
        "message": {
            "model": &model_id,
            "usage": {
                "input_tokens": 0,
                "output_tokens": 0,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0
            },
            "content": [{
                "type": "text",
                "text": format!("FAKE runner executing role {role}; no LLM call was made.")
            }]
        }
    }))?;

    heartbeat_delay(delay_ms)?;

    match decision.outcome {
        "NONZERO_EXIT" => {
            eprintln!("stores-fake-agent: class=infra scripted nonzero exit");
            std::process::exit(42);
        }
        "PAYLOAD_INVALID_EXIT_0" => {
            emit(json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "duration_ms": delay_ms,
                "model": &model_id,
                "provider": "stores-fake",
                "api": "stores-fake-agent-v1",
                "result": "class=payload intentionally invalid output"
            }))?;
        }
        "MESSY_LEGACY_OUTPUT" => {
            let payload = stores::runner::fake::fake_payload_for_role(&role)
                .with_context(|| format!("building fake payload for role {role}"))?;
            emit(json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "duration_ms": delay_ms,
                "model": &model_id,
                "provider": "stores-fake",
                "api": "stores-fake-agent-v1",
                "result": format!("messy fake prose before JSON\n```json\n{}\n```\ntrailing prose", payload)
            }))?;
        }
        _ => {
            let payload = scripted_payload_for_role(
                &role,
                decision.outcome,
                &workspace,
                &task_id,
                &phase,
                &cycle,
                &attempt,
            )
            .with_context(|| format!("building fake payload for role {role}"))?;
            let git_end_sha = git_head(&workspace).ok();
            emit(json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "duration_ms": delay_ms,
                "duration_api_ms": 0,
                "num_turns": 1,
                "model": &model_id,
                "provider": "stores-fake",
                "api": "stores-fake-agent-v1",
                "git_start_sha": git_start_sha,
                "git_end_sha": git_end_sha,
                "usage": {
                    "input_tokens": 0,
                    "output_tokens": 0,
                    "cache_read_input_tokens": 0,
                    "cache_creation_input_tokens": 0
                },
                "structured_output": payload,
                "result": payload.to_string()
            }))?;
        }
    }

    Ok(())
}

fn scripted_payload_for_role(
    role: &str,
    outcome: &str,
    workspace: &Path,
    task_id: &str,
    phase: &str,
    cycle: &str,
    attempt: &str,
) -> Result<serde_json::Value> {
    let mut payload = stores::runner::fake::fake_payload_for_role(role)?;
    let normalized_role = role.replace('_', "-");
    if normalized_role == "executor" {
        apply_executor_mode(&mut payload, workspace, task_id, phase, cycle, attempt)?;
    }
    if normalized_role == "external-review" {
        verify_external_review_brief()?;
    }
    match (normalized_role.as_str(), outcome) {
        ("plan-reviewer", "NEEDS_WORK") => {
            payload["gate"] = json!("NEEDS_WORK");
            payload["summary"] = json!("FAKE plan review requested one scripted revision.");
        }
        ("code-reviewer", "REVISE") => {
            payload["gate"] = json!("REVISE");
            payload["summary"] = json!("FAKE code review requested one scripted revision.");
            payload["counts"] = json!({"critical": 0, "major": 1, "minor": 0});
        }
        ("external-review", "REVISE") => {
            payload["verdict"] = json!("REVISE");
            payload["major_count"] = json!(1);
            payload["counts"] = json!({"critical": 0, "major": 1, "minor": 0});
            payload["findings"] = json!([{ "severity": "major", "message": "scripted fake external review revision" }]);
            payload["summary"] = json!("FAKE external review requested one scripted revision.");
        }
        ("external-review", "TOOLING_FAILURE") => {
            payload["verdict"] = json!("TOOLING_FAILURE");
            payload["summary"] = json!("FAKE external review scripted tooling failure.");
        }
        _ => {}
    }
    Ok(payload)
}

fn apply_executor_mode(
    payload: &mut serde_json::Value,
    workspace: &Path,
    task_id: &str,
    phase: &str,
    cycle: &str,
    attempt: &str,
) -> Result<()> {
    let mode = std::env::var("STORES_FAKE_EXECUTOR_MODE").unwrap_or_else(|_| "no_op".to_string());
    match mode.as_str() {
        "" | "no_op" => Ok(()),
        "marker_file" => {
            let rel = format!("fake-runner-markers/{task_id}-p{phase}-c{cycle}-a{attempt}.txt");
            let abs = workspace.join(&rel);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating marker parent {}", parent.display()))?;
            }
            std::fs::write(
                &abs,
                format!(
                    "stores-fake-agent marker\ntask_id={task_id}\nphase={phase}\ncycle={cycle}\nattempt={attempt}\n"
                ),
            )
            .with_context(|| format!("writing marker {}", abs.display()))?;
            git(&["add", &rel], workspace)?;
            let commit = commit_fake(workspace, task_id, "marker_file")?;
            payload["summary"] =
                json!("FAKE executor wrote and committed a deterministic marker file.");
            payload["commit"] = json!(commit);
            payload["files_changed"] = json!([rel]);
            Ok(())
        }
        "scripted_patch" => {
            let patch = std::env::var("STORES_FAKE_SCRIPTED_PATCH")
                .context("STORES_FAKE_SCRIPTED_PATCH is required for scripted_patch mode")?;
            git(&["apply", &patch], workspace)?;
            let files = git_changed_files(workspace)?;
            if files.is_empty() {
                bail!("scripted_patch mode applied no changes");
            }
            for file in &files {
                git(&["add", file], workspace)?;
            }
            let commit = commit_fake(workspace, task_id, "scripted_patch")?;
            payload["summary"] =
                json!("FAKE executor applied and committed a scripted patch fixture.");
            payload["commit"] = json!(commit);
            payload["files_changed"] = json!(files);
            Ok(())
        }
        other => bail!("unknown STORES_FAKE_EXECUTOR_MODE '{other}'"),
    }
}

fn verify_external_review_brief() -> Result<()> {
    let Some(path) = std::env::var_os("STORES_FAKE_REVIEW_BRIEF_PATH").map(PathBuf::from) else {
        return Ok(());
    };
    let meta = std::fs::metadata(&path)
        .with_context(|| format!("fake external review brief missing: {}", path.display()))?;
    if !meta.is_file() {
        bail!(
            "fake external review brief is not a file: {}",
            path.display()
        );
    }
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading fake external review brief: {}", path.display()))?;
    if body.trim().is_empty() {
        bail!("fake external review brief is empty: {}", path.display());
    }
    Ok(())
}

fn git_head(workspace: &Path) -> Result<String> {
    Ok(git_output(&["rev-parse", "HEAD"], workspace)?
        .trim()
        .to_string())
}

fn git_changed_files(workspace: &Path) -> Result<Vec<String>> {
    let out = git_output(
        &["ls-files", "--others", "--modified", "--exclude-standard"],
        workspace,
    )?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

fn commit_fake(workspace: &Path, task_id: &str, mode: &str) -> Result<String> {
    git(
        &[
            "-c",
            "user.name=stores-fake-agent",
            "-c",
            "user.email=stores-fake-agent@example.invalid",
            "commit",
            "-m",
            &format!("FAKE executor {task_id}: {mode}\n\nProvenance: stores-fake-agent executor_mode={mode}"),
        ],
        workspace,
    )?;
    git_head(workspace)
}

fn git(args: &[&str], workspace: &Path) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .with_context(|| format!("spawning git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn git_output(args: &[&str], workspace: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
        .with_context(|| format!("spawning git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn controlled_stall(ignore_sigterm: bool, delay_ms: u64) -> Result<()> {
    #[cfg(unix)]
    if ignore_sigterm {
        unsafe {
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
        }
    }
    std::thread::sleep(Duration::from_millis(delay_ms.max(60_000)));
    Ok(())
}

fn heartbeat_delay(delay_ms: u64) -> Result<()> {
    if delay_ms == 0 {
        return Ok(());
    }
    let interval_ms = std::env::var("STORES_FAKE_HEARTBEAT_INTERVAL_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(1000);
    let start = Instant::now();
    let total = Duration::from_millis(delay_ms);
    let interval = Duration::from_millis(interval_ms);
    while start.elapsed() < total {
        let remaining = total.saturating_sub(start.elapsed());
        std::thread::sleep(std::cmp::min(remaining, interval));
        emit(json!({
            "type": "fake_heartbeat",
            "elapsed_ms": start.elapsed().as_millis() as u64,
            "source": "stores-fake-agent"
        }))?;
    }
    Ok(())
}

fn emit(value: serde_json::Value) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", serde_json::to_string(&value)?)?;
    stdout.flush()?;
    Ok(())
}
