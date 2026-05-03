use serde_json::Value;
use std::collections::BTreeMap;

/// Print a single entry as human-readable text.
/// Nested Records are indented under their key.
pub fn print_entry_text(entry: &BTreeMap<String, Value>) {
    print_map_text(entry, 0);
}

fn print_map_text(map: &BTreeMap<String, Value>, indent: usize) {
    let pad = "  ".repeat(indent);
    for (k, v) in map {
        match v {
            Value::Object(obj) => {
                println!("{pad}{k}:");
                let sub: BTreeMap<String, Value> =
                    obj.iter().map(|(a, b)| (a.clone(), b.clone())).collect();
                print_map_text(&sub, indent + 1);
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
                println!("{pad}{k}: {}", parts.join("|"));
            }
            Value::Null => {
                println!("{pad}{k}: ");
            }
            other => {
                println!("{pad}{k}: {}", value_display(other));
            }
        }
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
