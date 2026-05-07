use serde_json::Value;
use std::collections::BTreeMap;

/// Print a single entry as human-readable text.
/// Nested Records are indented under their key. For observations, a non-null
/// intent_contract.harden_log is printed as a readable audit subsection; null
/// or absent harden_log is omitted from text output. JSON output is unchanged.
pub fn print_entry_text(entry: &BTreeMap<String, Value>) {
    print!("{}", entry_text(entry));
}

pub fn entry_text(entry: &BTreeMap<String, Value>) -> String {
    let mut out = String::new();
    push_map_text(&mut out, entry, 0);
    out
}

fn push_map_text(out: &mut String, map: &BTreeMap<String, Value>, indent: usize) {
    let pad = "  ".repeat(indent);
    for (k, v) in map {
        match v {
            Value::Object(obj) => {
                out.push_str(&format!("{pad}{k}:\n"));
                let mut sub: BTreeMap<String, Value> =
                    obj.iter().map(|(a, b)| (a.clone(), b.clone())).collect();
                if k == "intent_contract" {
                    let harden_log = sub.remove("harden_log");
                    push_map_text(out, &sub, indent + 1);
                    if let Some(h) = harden_log.as_ref().filter(|h| !h.is_null()) {
                        push_harden_log(out, h, indent + 1);
                    }
                } else {
                    push_map_text(out, &sub, indent + 1);
                }
            }
            Value::Array(arr) => {
                let parts: Vec<String> = arr
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| v.to_string())
                    })
                    .collect();
                out.push_str(&format!("{pad}{k}: {}\n", parts.join("|")));
            }
            Value::Null => {
                out.push_str(&format!("{pad}{k}: \n"));
            }
            other => {
                out.push_str(&format!("{pad}{k}: {}\n", value_display(other)));
            }
        }
    }
}

pub fn harden_log_markdown(v: &Value) -> String {
    let Some(obj) = v.as_object() else {
        return String::new();
    };
    let mut out = String::new();
    push_list_records(
        &mut out,
        "Decisions",
        obj.get("decisions"),
        &["id", "decision", "rationale", "source_quote"],
    );
    push_list_records(
        &mut out,
        "Scope cuts",
        obj.get("scope_cuts"),
        &["cut", "rationale", "source_quote"],
    );
    push_list_records(
        &mut out,
        "Alternatives rejected",
        obj.get("alternatives_rejected"),
        &["alternative", "why_rejected"],
    );
    push_list_records(
        &mut out,
        "Compress vs surface",
        obj.get("compress_vs_surface"),
        &["item", "judgment", "rationale"],
    );
    if let Some(arr) = obj.get("unresolved_questions").and_then(|v| v.as_array()) {
        if !arr.is_empty() {
            out.push_str("### Unresolved questions\n");
            for item in arr {
                if let Some(s) = item.as_str() {
                    out.push_str(&format!("- {s}\n"));
                }
            }
            out.push('\n');
        }
    }
    out
}

fn push_list_records(out: &mut String, title: &str, val: Option<&Value>, keys: &[&str]) {
    let Some(arr) = val.and_then(|v| v.as_array()) else {
        return;
    };
    if arr.is_empty() {
        return;
    }
    out.push_str(&format!("### {title}\n"));
    for item in arr {
        if let Some(obj) = item.as_object() {
            let first = keys
                .iter()
                .find_map(|k| obj.get(*k).and_then(|v| v.as_str()).map(|s| (*k, s)));
            if let Some((k, s)) = first {
                out.push_str(&format!("- **{k}:** {s}\n"));
            }
            for k in keys {
                if first.map(|(fk, _)| fk) == Some(*k) {
                    continue;
                }
                if let Some(s) = obj.get(*k).and_then(|v| v.as_str()) {
                    out.push_str(&format!("  - **{k}:** {s}\n"));
                }
            }
        }
    }
    out.push('\n');
}

fn push_harden_log(out: &mut String, v: &Value, indent: usize) {
    let pad = "  ".repeat(indent);
    let rendered = harden_log_markdown(v);
    if rendered.trim().is_empty() {
        return;
    }
    out.push_str(&format!("{pad}harden_log:\n"));
    for line in rendered.lines() {
        out.push_str(&format!("{pad}  {line}\n"));
    }
}

fn value_display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// Print a list of entries as a scannable human-readable table.
pub fn print_list_table(entries: &[BTreeMap<String, Value>]) {
    print!("{}", list_table_text(entries));
}

pub fn list_table_text(entries: &[BTreeMap<String, Value>]) -> String {
    list_table_text_with_width(entries, terminal_width())
}

pub fn list_table_text_with_width(entries: &[BTreeMap<String, Value>], width: usize) -> String {
    const HEADERS: [&str; 5] = ["display_id", "status", "priority", "source", "summary"];
    let rows: Vec<[String; 5]> = entries.iter().map(standard_list_row).collect();

    let mut widths = [
        HEADERS[0].len(),
        HEADERS[1].len(),
        HEADERS[2].len(),
        HEADERS[3].len(),
    ];
    for row in &rows {
        for i in 0..4 {
            widths[i] = widths[i].max(row[i].chars().count());
        }
    }

    let fixed_width: usize = widths.iter().sum::<usize>() + (HEADERS.len() - 1) * 2;
    let summary_width = width.saturating_sub(fixed_width).max(1);

    let mut out = String::new();
    out.push_str(&format!(
        "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}\n",
        HEADERS[0],
        HEADERS[1],
        HEADERS[2],
        HEADERS[3],
        HEADERS[4],
        w0 = widths[0],
        w1 = widths[1],
        w2 = widths[2],
        w3 = widths[3]
    ));
    for row in rows {
        out.push_str(&format!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {:<w3$}  {}\n",
            row[0],
            row[1],
            row[2],
            row[3],
            truncate_to_width(&row[4], summary_width),
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3]
        ));
    }
    out
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|w| *w > 0)
        .unwrap_or(80)
}

fn standard_list_row(entry: &BTreeMap<String, Value>) -> [String; 5] {
    [
        scalar_cell(entry.get("display_id")),
        scalar_cell(entry.get("status")),
        scalar_cell(entry.get("priority")),
        scalar_cell(entry.get("source")),
        summary_or_title(entry),
    ]
}

fn summary_or_title(entry: &BTreeMap<String, Value>) -> String {
    let summary = scalar_cell(entry.get("summary"));
    if summary.is_empty() {
        scalar_cell(entry.get("title"))
    } else {
        summary
    }
}

fn scalar_cell(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => one_line(s),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_to_width(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len <= width {
        return s.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(width - 1).collect();
    out.push('…');
    out
}

/// Print a single entry as JSON.
pub fn print_entry_json(entry: &BTreeMap<String, Value>) {
    let v = Value::Object(entry.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    print_value_json(&v);
}

/// Print a selected value: scalars as one raw line, objects/lists as JSON.
pub fn print_selected_value(value: &Value) {
    match value {
        Value::Object(_) | Value::Array(_) => print_value_json(value),
        Value::Null => println!(),
        other => println!("{}", value_display(other)),
    }
}

/// Print any selected value as valid JSON.
pub fn print_value_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".to_string())
    );
}

/// Print a list of entries as JSON array.
pub fn print_list_json(entries: &[BTreeMap<String, Value>]) {
    println!("{}", list_json_text(entries));
}

pub fn list_json_text(entries: &[BTreeMap<String, Value>]) -> String {
    let arr: Vec<Value> = entries
        .iter()
        .map(|e| Value::Object(e.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
        .collect();
    serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(fields: &[(&str, Value)]) -> BTreeMap<String, Value> {
        fields
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn list_table_has_header_and_one_data_line_per_entry() {
        let entries = vec![
            entry(&[
                ("display_id", Value::String("L001".into())),
                ("status", Value::String("new".into())),
                ("priority", Value::String("high".into())),
                ("source", Value::String("cli".into())),
                ("summary", Value::String("first".into())),
                ("body", Value::String("body should not render".into())),
            ]),
            entry(&[
                ("display_id", Value::String("L002".into())),
                ("status", Value::String("triaged".into())),
                ("priority", Value::String("low".into())),
                ("source", Value::String("api".into())),
                ("summary", Value::String("second".into())),
            ]),
        ];

        let text = list_table_text_with_width(&entries, 120);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("display_id"));
        assert!(lines[0].contains("status"));
        assert!(lines[0].contains("priority"));
        assert!(lines[0].contains("source"));
        assert!(lines[0].contains("summary"));
        assert!(lines[1].contains("L001"));
        assert!(lines[2].contains("L002"));
        assert!(!text.contains("body should not render"));
    }

    #[test]
    fn long_summary_truncates_to_supplied_width() {
        let entries = vec![entry(&[
            ("display_id", Value::String("L001".into())),
            ("status", Value::String("new".into())),
            ("priority", Value::String("high".into())),
            ("source", Value::String("cli".into())),
            (
                "summary",
                Value::String("abcdefghijklmnopqrstuvwxyz".into()),
            ),
        ])];

        let text = list_table_text_with_width(&entries, 45);
        let data = text.lines().nth(1).unwrap();
        assert!(data.ends_with('…'), "data line should be truncated: {data}");
        assert!(!data.contains("abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn title_fallback_is_used_for_task_like_rows() {
        let entries = vec![entry(&[
            ("display_id", Value::String("T001".into())),
            ("status", Value::String("open".into())),
            ("title", Value::String("task title fallback".into())),
        ])];

        let text = list_table_text_with_width(&entries, 120);
        assert!(text.contains("task title fallback"));
    }

    #[test]
    fn missing_priority_and_source_are_blank() {
        let entries = vec![entry(&[
            ("display_id", Value::String("T001".into())),
            ("status", Value::String("open".into())),
            ("title", Value::String("task title".into())),
        ])];

        let text = list_table_text_with_width(&entries, 120);
        let data = text.lines().nth(1).unwrap();
        assert!(data.starts_with("T001        open"));
        assert!(data.contains("task title"));
    }
}
