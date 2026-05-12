use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use stores::runner::RunnerInvocationContext;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn fake_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stores-fake-agent"))
}

#[test]
fn fake_runner_launches_real_fake_agent_and_writes_invocation_artifacts() {
    let _guard = env_lock().lock().unwrap();
    let old_bin = std::env::var_os("STORES_FAKE_AGENT_BIN");
    let old_delay = std::env::var_os("STORES_FAKE_DELAY_MS");
    std::env::set_var("STORES_FAKE_AGENT_BIN", fake_agent_bin());
    std::env::set_var("STORES_FAKE_DELAY_MS", "1");

    let tmp = tempfile::tempdir().unwrap();
    let runs = tmp.path().join(".stores").join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    let invocation = RunnerInvocationContext {
        session_id: "fake-session-1".to_string(),
        flat_transcript_path: runs.join("fake-session-1.jsonl"),
        stderr_log_path: runs.join("fake-session-1.stderr.log"),
        events_path: runs.join("fake-session-1").join("events.jsonl"),
        status_path: runs.join("fake-session-1").join("status.json"),
    };

    let runner = stores::runner::select("fake").unwrap();
    let out = runner
        .spawn_with_invocation(
            "planner",
            "system",
            "brief",
            None,
            Some(tmp.path().to_str().unwrap()),
            Some(&invocation),
        )
        .unwrap();

    assert_eq!(out.exit_code, 0);
    assert_eq!(out.session_id.as_deref(), Some("fake-session-1"));
    assert_eq!(out.structured_output_source, Some("sdk"));
    let structured = out.structured_output.as_ref().expect("structured output");
    assert_eq!(structured["role"], "planner");
    assert!(invocation.flat_transcript_path.exists());
    let transcript = std::fs::read_to_string(&invocation.flat_transcript_path).unwrap();
    assert!(
        transcript.contains("fake_heartbeat"),
        "nonzero fake delay should emit heartbeat lines; transcript was:\n{transcript}"
    );
    assert!(invocation.events_path.exists());
    assert!(invocation.status_path.exists());
    assert_eq!(out.telemetry.harness_id.as_deref(), Some("fake"));
    assert_eq!(out.telemetry.provider_id.as_deref(), Some("stores-fake"));
    assert_eq!(out.telemetry.payload_valid, Some(true));

    match old_bin {
        Some(v) => std::env::set_var("STORES_FAKE_AGENT_BIN", v),
        None => std::env::remove_var("STORES_FAKE_AGENT_BIN"),
    }
    match old_delay {
        Some(v) => std::env::set_var("STORES_FAKE_DELAY_MS", v),
        None => std::env::remove_var("STORES_FAKE_DELAY_MS"),
    }
}
