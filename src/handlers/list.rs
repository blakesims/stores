use anyhow::Result;
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::schema::{actor::Actor, FieldType, Schema};
use crate::output;

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: Actor,
) -> Result<()> {
    let json_flag = matches.get_flag("json");

    let mut col_names: Vec<String> = vec![
        "id".to_string(),
        "display_id".to_string(),
        "status".to_string(),
        "created_at".to_string(),
        "updated_at".to_string(),
        "created_by".to_string(),
        "updated_by".to_string(),
    ];
    for field in &schema.fields {
        col_names.push(field.name.clone());
    }

    let col_list = col_names.join(", ");
    let sql = format!("SELECT {col_list} FROM {} ORDER BY id", schema.name);

    let mut stmt = conn.prepare(&sql)?;
    let mut entries: Vec<BTreeMap<String, Value>> = Vec::new();

    let rows = stmt.query_map([], |row| {
        let mut map: BTreeMap<String, Value> = BTreeMap::new();
        for (i, col) in col_names.iter().enumerate() {
            let v: rusqlite::types::Value = row.get(i)?;
            map.insert(col.clone(), sqlite_to_json(v));
        }
        Ok(map)
    })?;

    for row_result in rows {
        let raw_map = row_result?;
        let mut entry: BTreeMap<String, Value> = BTreeMap::new();

        // Reserved columns
        for key in &["display_id", "status", "created_at", "updated_at", "created_by", "updated_by"] {
            if let Some(v) = raw_map.get(*key) {
                entry.insert(key.to_string(), v.clone());
            }
        }

        // Schema fields: decode JSON for Record/List
        for field in &schema.fields {
            if let Some(raw_val) = raw_map.get(&field.name) {
                match &field.ty {
                    FieldType::Record(_) | FieldType::List(_) => {
                        if let Value::String(json_str) = raw_val {
                            if !json_str.is_empty() && json_str != "null" {
                                if let Ok(parsed) = serde_json::from_str::<Value>(json_str) {
                                    entry.insert(field.name.clone(), parsed);
                                    continue;
                                }
                            }
                        }
                        entry.insert(field.name.clone(), Value::Null);
                    }
                    _ => {
                        entry.insert(field.name.clone(), raw_val.clone());
                    }
                }
            }
        }

        entries.push(entry);
    }

    if json_flag {
        output::print_list_json(&entries);
    } else {
        for entry in &entries {
            let did = entry
                .get("display_id")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            output::print_list_text(did, entry);
        }
    }

    Ok(())
}

fn sqlite_to_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => Value::from(i),
        rusqlite::types::Value::Real(f) => {
            Value::from(serde_json::Number::from_f64(f).unwrap_or(serde_json::Number::from(0)))
        }
        rusqlite::types::Value::Text(s) => Value::String(s),
        rusqlite::types::Value::Blob(b) => Value::String(String::from_utf8_lossy(&b).to_string()),
    }
}
