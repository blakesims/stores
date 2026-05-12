use anyhow::{Context, Result};
use serde_json::json;
use std::io::Write;
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
    if matches!(
        scenario.as_str(),
        "long-delay-heartbeat" | "stall-no-heartbeat" | "sigterm-ignoring-stall"
    ) && std::env::var_os("STORES_FAKE_DELAY_MS").is_none()
    {
        delay_ms = 2_000;
    }
    let decision = stores::runner::fake::decide_fake_outcome(
        &scenario, &seed, &task_id, &role, &phase, &cycle, &attempt,
    );

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
        "model": "fake-random-v1",
        "provider": "stores-fake",
        "api": "stores-fake-agent-v1"
    }))?;

    if matches!(
        decision.outcome,
        "STALL_NO_HEARTBEAT" | "SIGTERM_IGNORE_STALL"
    ) {
        emit(stores::runner::fake::fake_decision_event(&decision))?;
        controlled_stall(decision.outcome == "SIGTERM_IGNORE_STALL", delay_ms)?;
        return Ok(());
    }

    emit(json!({
        "type": "assistant",
        "message": {
            "model": "fake-random-v1",
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
    emit(stores::runner::fake::fake_decision_event(&decision))?;

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
                "model": "fake-random-v1",
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
                "model": "fake-random-v1",
                "provider": "stores-fake",
                "api": "stores-fake-agent-v1",
                "result": format!("messy fake prose before JSON\n```json\n{}\n```\ntrailing prose", payload)
            }))?;
        }
        _ => {
            let payload = scripted_payload_for_role(&role, decision.outcome)
                .with_context(|| format!("building fake payload for role {role}"))?;
            emit(json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "duration_ms": delay_ms,
                "duration_api_ms": 0,
                "num_turns": 1,
                "model": "fake-random-v1",
                "provider": "stores-fake",
                "api": "stores-fake-agent-v1",
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

fn scripted_payload_for_role(role: &str, outcome: &str) -> Result<serde_json::Value> {
    let mut payload = stores::runner::fake::fake_payload_for_role(role)?;
    let normalized_role = role.replace('_', "-");
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
