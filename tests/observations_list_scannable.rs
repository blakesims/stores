use serde_json::Value;
use std::collections::BTreeMap;

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
fn observations_list_default_is_scannable_and_json_is_complete() {
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
