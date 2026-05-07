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

/// Print a list of entries as human-readable text.
pub fn print_list_text(display_id: &str, entry: &BTreeMap<String, Value>) {
    print!("{display_id}  ");
    // One-line summary: print scalar fields only
    for (k, v) in entry {
        match v {
            Value::Object(_) | Value::Null => {}
            other => print!("{k}={} ", value_display(other)),
        }
    }
    println!();
}

/// Print a single entry as JSON.
pub fn print_entry_json(entry: &BTreeMap<String, Value>) {
    let v = Value::Object(entry.iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    println!(
        "{}",
        serde_json::to_string_pretty(&v).unwrap_or_else(|_| "{}".to_string())
    );
}

/// Print a list of entries as JSON array.
pub fn print_list_json(entries: &[BTreeMap<String, Value>]) {
    let arr: Vec<Value> = entries
        .iter()
        .map(|e| Value::Object(e.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_else(|_| "[]".to_string())
    );
}
