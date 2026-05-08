use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Command;

fn observation(
    display_id: &str,
    status: &str,
    priority: &str,
    source: &str,
    summary: &str,
    body: &str,
) -> BTreeMap<String, Value> {
    [
        ("display_id", Value::String(display_id.to_string())),
        ("status", Value::String(status.to_string())),
        ("priority", Value::String(priority.to_string())),
        ("source", Value::String(source.to_string())),
        ("summary", Value::String(summary.to_string())),
        ("body", Value::String(body.to_string())),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

#[test]
fn renderer_default_is_scannable_and_json_is_complete() {
    let long_summary_1 = "summary one contains enough words to require deterministic truncation at a narrow terminal width";
    let long_summary_2 = "summary two also contains enough words to require deterministic truncation at a narrow terminal width";
    let long_body_1 =
        "body one must remain absent from the default tabular observations list output";
    let long_body_2 =
        "body two must remain absent from the default tabular observations list output";
    let entries = vec![
        observation("L001", "new", "high", "cli", long_summary_1, long_body_1),
        observation("L002", "triaged", "low", "api", long_summary_2, long_body_2),
    ];

    let text = stores::output::list_table_text_with_width(&entries, 72);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "header plus one data line per observation");
    assert!(lines[0].contains("display_id"));
    assert!(lines[0].contains("status"));
    assert!(lines[0].contains("priority"));
    assert!(lines[0].contains("source"));
    assert!(lines[0].contains("summary"));
    assert!(lines[1].contains("L001"));
    assert!(lines[1].contains("new"));
    assert!(lines[1].contains("high"));
    assert!(lines[1].contains("cli"));
    assert!(lines[1].ends_with('…'));
    assert!(lines[2].contains("L002"));
    assert!(lines[2].contains("triaged"));
    assert!(lines[2].contains("low"));
    assert!(lines[2].contains("api"));
    assert!(lines[2].ends_with('…'));
    assert!(!text.contains(long_body_1));
    assert!(!text.contains(long_body_2));
    assert!(!text.contains(long_summary_1));
    assert!(!text.contains(long_summary_2));

    let json_text = stores::output::list_json_text(&entries);
    let json: Value = serde_json::from_str(&json_text).unwrap();
    assert_eq!(json[0]["summary"], long_summary_1);
    assert_eq!(json[0]["body"], long_body_1);
    assert_eq!(json[1]["summary"], long_summary_2);
    assert_eq!(json[1]["body"], long_body_2);
}

fn stores_cmd(workspace: &std::path::Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_stores"));
    cmd.current_dir(workspace);
    cmd
}

fn run_ok(cmd: &mut Command) -> String {
    let output = cmd.output().expect("run stores command");
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("stdout utf8")
}

#[test]
fn cli_observations_list_default_is_scannable_and_json_is_complete() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let workspace = tmp.path();
    run_ok(stores_cmd(workspace).arg("init"));
    run_ok(stores_cmd(workspace).args(["install", "observations"]));

    let long_summary_1 = "cli summary one contains enough words to require deterministic truncation at a narrow terminal width";
    let long_summary_2 = "cli summary two also contains enough words to require deterministic truncation at a narrow terminal width";
    let long_body_1 = "cli body one must remain absent from default observations list text";
    let long_body_2 = "cli body two must remain absent from default observations list text";

    run_ok(stores_cmd(workspace).args([
        "observations",
        "add",
        "--summary",
        long_summary_1,
        "--body",
        long_body_1,
        "--source",
        "dev",
        "--priority",
        "high",
        "--captured-at",
        "2026-05-08T00:00:00Z",
        "--captured-week",
        "w19-d5",
    ]));
    run_ok(stores_cmd(workspace).args([
        "observations",
        "add",
        "--summary",
        long_summary_2,
        "--body",
        long_body_2,
        "--source",
        "qa",
        "--priority",
        "low",
        "--captured-at",
        "2026-05-08T00:01:00Z",
        "--captured-week",
        "w19-d5",
    ]));

    let text = run_ok(
        stores_cmd(workspace)
            .env("COLUMNS", "72")
            .args(["observations", "list"]),
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "header plus one data line per observation");
    assert!(lines[0].contains("display_id"));
    assert!(lines[0].contains("status"));
    assert!(lines[0].contains("priority"));
    assert!(lines[0].contains("source"));
    assert!(lines[0].contains("summary"));
    assert!(lines[1].contains("L001"));
    assert!(lines[1].contains("open"));
    assert!(lines[1].contains("high"));
    assert!(lines[1].contains("dev"));
    assert!(lines[1].ends_with('…'));
    assert!(lines[2].contains("L002"));
    assert!(lines[2].contains("open"));
    assert!(lines[2].contains("low"));
    assert!(lines[2].contains("qa"));
    assert!(lines[2].ends_with('…'));
    assert!(!text.contains(long_body_1));
    assert!(!text.contains(long_body_2));
    assert!(!text.contains(long_summary_1));
    assert!(!text.contains(long_summary_2));

    let json_text = run_ok(stores_cmd(workspace).args(["observations", "list", "--json"]));
    let json: Value = serde_json::from_str(&json_text).unwrap();
    assert_eq!(json[0]["summary"], long_summary_1);
    assert_eq!(json[0]["body"], long_body_1);
    assert_eq!(json[1]["summary"], long_summary_2);
    assert_eq!(json[1]["body"], long_body_2);
}
