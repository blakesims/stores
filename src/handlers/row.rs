use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use crate::schema::{FieldType, Schema};
use crate::validate::EntryMap;

/// Build a nested EntryMap from flat CLI args.
/// `get_arg` returns the raw string value for a given cli_name (e.g. "done-when"),
/// or None if the arg was not provided.
pub fn build_entry_map<F>(schema: &Schema, get_arg: F) -> Result<EntryMap>
where
    F: Fn(&str) -> Option<String>,
{
    use crate::schema::flatten::leaf_args;
    let leaves = leaf_args(schema)?;

    let mut entry: EntryMap = BTreeMap::new();

    for leaf in &leaves {
        let raw = get_arg(&leaf.cli_name);

        if raw.is_none() {
            continue;
        }
        let raw = raw.unwrap();

        let value = coerce_value(&leaf.field.ty, &raw);

        // Nest into entry map following the path.
        // path can be ["field"] or ["record", "subfield"]
        if leaf.path.len() == 1 {
            entry.insert(leaf.path[0].clone(), value);
        } else {
            // path.len() == 2 for Record sub-fields (v0.1 only one level deep)
            let parent = leaf.path[0].clone();
            let child = leaf.path[1].clone();
            let rec = entry
                .entry(parent)
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(ref mut map) = rec {
                map.insert(child, value);
            }
        }
    }

    Ok(entry)
}

/// Coerce a raw string to a typed serde_json::Value.
pub fn coerce_value(ty: &FieldType, raw: &str) -> Value {
    match ty {
        FieldType::Integer => raw
            .parse::<i64>()
            .map(Value::from)
            .unwrap_or_else(|_| Value::String(raw.to_string())),
        FieldType::Bool => match raw.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Value::Bool(true),
            _ => Value::Bool(false),
        },
        FieldType::List(_) => {
            // Split on "|"
            let parts: Vec<Value> = raw
                .split('|')
                .map(|s| Value::String(s.to_string()))
                .collect();
            Value::Array(parts)
        }
        _ => Value::String(raw.to_string()),
    }
}

/// Return current UTC time as ISO-8601 string.
pub fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, mo, d, h, mi, s) = unix_to_ymd_hms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn unix_to_ymd_hms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let s = secs % 60;
    let total_min = secs / 60;
    let mi = total_min % 60;
    let total_hr = total_min / 60;
    let h = total_hr % 24;
    let days = total_hr / 24;
    let (y, mo, d) = days_to_ymd(days);
    (y, mo, d, h as u32, mi as u32, s as u32)
}

fn days_to_ymd(mut days: u64) -> (u32, u32, u32) {
    let mut year = 1970u32;
    loop {
        let dy = days_in_year(year) as u64;
        if days < dy { break; }
        days -= dy;
        year += 1;
    }
    let mut month = 1u32;
    loop {
        let dm = days_in_month(year, month) as u64;
        if days < dm { break; }
        days -= dm;
        month += 1;
    }
    (year, month, days as u32 + 1)
}

fn is_leap(y: u32) -> bool { (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) }
fn days_in_year(y: u32) -> u32 { if is_leap(y) { 366 } else { 365 } }
fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap(y) { 29 } else { 28 },
        _ => 31,
    }
}

/// Read a row from the DB by display_id and reconstruct an EntryMap.
pub fn read_row(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
) -> Result<(i64, EntryMap)> {
    // Determine all column names we want to read
    let mut cols: Vec<String> = vec![
        "id".to_string(),
        "display_id".to_string(),
        "status".to_string(),
        "created_at".to_string(),
        "updated_at".to_string(),
        "created_by".to_string(),
        "updated_by".to_string(),
    ];

    // Add schema field columns
    for field in &schema.fields {
        cols.push(field.name.clone());
    }

    let col_list = cols.join(", ");
    let sql = format!(
        "SELECT {col_list} FROM {table} WHERE display_id = ?1",
        table = schema.name
    );

    let row_data: Result<(i64, BTreeMap<String, Value>), _> = conn.query_row(
        &sql,
        rusqlite::params![display_id],
        |row| {
            let mut map: BTreeMap<String, Value> = BTreeMap::new();
            for (i, col) in cols.iter().enumerate() {
                let v: rusqlite::types::Value = row.get(i)?;
                map.insert(col.clone(), sqlite_to_json(v));
            }
            let id: i64 = row.get(0)?;
            Ok((id, map))
        },
    );

    let (id, raw_map) = row_data.map_err(|e| {
        if e == rusqlite::Error::QueryReturnedNoRows {
            anyhow::anyhow!("no entry with display_id '{display_id}'")
        } else {
            anyhow::anyhow!("db error: {e}")
        }
    })?;

    // Reconstruct nested EntryMap: for Record/List fields, parse JSON text column
    let mut entry: EntryMap = BTreeMap::new();

    // Copy reserved columns
    for key in &["display_id", "status", "created_at", "updated_at", "created_by", "updated_by"] {
        if let Some(v) = raw_map.get(*key) {
            entry.insert(key.to_string(), v.clone());
        }
    }

    // Process schema fields
    for field in &schema.fields {
        if let Some(raw_val) = raw_map.get(&field.name) {
            match &field.ty {
                FieldType::Record(_) | FieldType::List(_) => {
                    // Column is JSON-as-TEXT; deserialize it
                    if let Value::String(json_str) = raw_val {
                        if !json_str.is_empty() {
                            if let Ok(parsed) = serde_json::from_str::<Value>(json_str) {
                                entry.insert(field.name.clone(), parsed);
                                continue;
                            }
                        }
                    }
                    // Null / empty
                    entry.insert(field.name.clone(), Value::Null);
                }
                _ => {
                    entry.insert(field.name.clone(), raw_val.clone());
                }
            }
        }
    }

    Ok((id, entry))
}

fn sqlite_to_json(v: rusqlite::types::Value) -> Value {
    match v {
        rusqlite::types::Value::Null => Value::Null,
        rusqlite::types::Value::Integer(i) => Value::from(i),
        rusqlite::types::Value::Real(f) => {
            Value::from(serde_json::Number::from_f64(f).unwrap_or(serde_json::Number::from(0)))
        }
        rusqlite::types::Value::Text(s) => Value::String(s),
        rusqlite::types::Value::Blob(b) => {
            Value::String(String::from_utf8_lossy(&b).to_string())
        }
    }
}
