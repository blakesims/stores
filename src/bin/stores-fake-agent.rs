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
    let session_id = std::env::var("STORES_FAKE_SESSION_ID").unwrap_or_else(|_| "unknown".into());
    let delay_ms = std::env::var("STORES_FAKE_DELAY_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(5000);
    let seed = std::env::var("STORES_FAKE_SEED").unwrap_or_else(|_| "0".to_string());
    let scenario = std::env::var("STORES_FAKE_SCENARIO").unwrap_or_else(|_| "all-pass".to_string());
    let policy_hash = format!("phase2:{scenario}:{delay_ms}:{seed}");

    emit(json!({
        "type": "system",
        "subtype": "stores_fake_start",
        "session_id": session_id,
        "role": role,
        "model": "fake-random-v1",
        "provider": "stores-fake",
        "api": "stores-fake-agent-v1"
    }))?;

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

    emit(json!({
        "type": "fake_decision",
        "role": role,
        "scenario": scenario,
        "seed": seed,
        "policy_hash": policy_hash,
        "roll": 0.0,
        "threshold": 1.0,
        "outcome": "PASS"
    }))?;

    let payload = stores::runner::fake::fake_payload_for_role(&role)
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
