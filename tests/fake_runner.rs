use std::path::PathBuf;

use stores::runner::{FakeRunner, Runner, RunnerInvocationContext};

fn fake_agent_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stores-fake-agent"))
}

#[test]
fn fake_runner_launches_real_fake_agent_and_writes_invocation_artifacts() {
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

    let runner = FakeRunner::with_bin(fake_agent_bin());
    let out = runner
        .spawn_with_invocation_and_env(
            "planner",
            "system",
            "brief",
            None,
            Some(tmp.path().to_str().unwrap()),
            Some(&invocation),
            &[
                ("STORES_FAKE_DELAY_MS".to_string(), "50".to_string()),
                (
                    "STORES_FAKE_HEARTBEAT_INTERVAL_MS".to_string(),
                    "10".to_string(),
                ),
            ],
        )
        .unwrap();

    assert_eq!(out.exit_code, 0);
    assert_eq!(out.session_id.as_deref(), Some("fake-session-1"));
    assert_eq!(out.structured_output_source, Some("fake"));
    let structured = out.structured_output.as_ref().expect("structured output");
    assert_eq!(structured["role"], "planner");
    assert!(invocation.flat_transcript_path.exists());
    let transcript = std::fs::read_to_string(&invocation.flat_transcript_path).unwrap();
    let heartbeat_count = transcript.matches("fake_heartbeat").count();
    assert!(
        heartbeat_count >= 2,
        "fake delay should intentionally emit multiple heartbeat lines; count={heartbeat_count}; transcript was:\n{transcript}"
    );
    assert!(invocation.events_path.exists());
    assert!(invocation.status_path.exists());
    assert_eq!(out.telemetry.harness_id.as_deref(), Some("fake"));
    assert_eq!(out.telemetry.provider_id.as_deref(), Some("stores-fake"));
    assert_eq!(out.telemetry.payload_valid, Some(true));

    let out_with_requested = runner
        .spawn_with_invocation_and_env(
            "planner",
            "system",
            "brief",
            None,
            Some(tmp.path().to_str().unwrap()),
            Some(&invocation),
            &[
                (
                    "STORES_FAKE_CONFIGURED_HARNESS_ID".to_string(),
                    "pi".to_string(),
                ),
                (
                    "STORES_FAKE_CONFIGURED_MODEL_ID".to_string(),
                    "gpt-5.5".to_string(),
                ),
                (
                    "STORES_FAKE_CONFIGURED_THINKING_EFFORT".to_string(),
                    "medium".to_string(),
                ),
            ],
        )
        .unwrap();
    assert_eq!(
        out_with_requested.telemetry.harness_id.as_deref(),
        Some("fake")
    );
    assert_eq!(
        out_with_requested
            .telemetry
            .configured_harness_id
            .as_deref(),
        Some("pi")
    );
    assert_eq!(
        out_with_requested.telemetry.configured_model_id.as_deref(),
        Some("gpt-5.5")
    );
    assert_eq!(
        out_with_requested
            .telemetry
            .configured_thinking_effort
            .as_deref(),
        Some("medium")
    );
}
