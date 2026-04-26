use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;

use crate::schema::{actor::Actor, FieldType, Schema};
use crate::validate;

use super::row::{build_entry_map, now_iso8601, read_row};

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: Actor,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    // Read existing row
    let (row_id, existing) = read_row(schema, conn, display_id)?;

    // Build diff entry from args
    let diff = build_entry_map(schema, |cli_name| {
        let from_file_key = format!("{cli_name}-from-file");
        if matches.try_contains_id(&from_file_key).unwrap_or(false) {
            if let Some(path) = matches.get_one::<String>(&from_file_key) {
                if path == "-" {
                    use std::io::Read;
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s).ok();
                    return Some(s.trim_end_matches('\n').to_string());
                }
                return std::fs::read_to_string(path).ok().map(|s| s.trim_end_matches('\n').to_string());
            }
        }
        matches.get_one::<String>(cli_name).cloned()
    })?;

    // Merge diff into existing
    let mut merged = existing.clone();
    for (k, v) in &diff {
        merged.insert(k.clone(), v.clone());
    }

    // Run validator (stub)
    validate::validate(schema, &merged, invoker)?;

    // Build SET clause for only the fields in diff + updated_*
    let now = now_iso8601();
    let invoker_str = invoker.to_string();

    let mut set_parts: Vec<String> = vec![
        format!("updated_at = ?1"),
        format!("updated_by = ?2"),
    ];
    let mut sql_values: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(now),
        rusqlite::types::Value::Text(invoker_str),
    ];
    let mut param_idx = 3usize;

    for field in &schema.fields {
        if let Some(new_val) = diff.get(&field.name) {
            set_parts.push(format!("{} = ?{param_idx}", field.name));
            param_idx += 1;

            match &field.ty {
                FieldType::Record(_) | FieldType::List(_) => {
                    let json_str = serde_json::to_string(new_val)
                        .unwrap_or_else(|_| "null".to_string());
                    sql_values.push(rusqlite::types::Value::Text(json_str));
                }
                FieldType::Bool => {
                    let i = match new_val {
                        Value::Bool(b) => if *b { 1 } else { 0 },
                        Value::Number(n) => n.as_i64().unwrap_or(0) as i32 as i64,
                        _ => 0,
                    };
                    sql_values.push(rusqlite::types::Value::Integer(i));
                }
                FieldType::Integer => {
                    let i = match new_val {
                        Value::Number(n) => n.as_i64().unwrap_or(0),
                        _ => 0,
                    };
                    sql_values.push(rusqlite::types::Value::Integer(i));
                }
                _ => {
                    let s = match new_val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    sql_values.push(rusqlite::types::Value::Text(s));
                }
            }
        }
    }

    // Append row_id as last param
    let where_param_idx = param_idx;
    sql_values.push(rusqlite::types::Value::Integer(row_id));

    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE {} SET {set_clause} WHERE id = ?{where_param_idx}",
        schema.name
    );

    conn.execute(&sql, rusqlite::params_from_iter(sql_values.iter()))
        .context("update row")?;

    println!("Updated {display_id}");
    Ok(())
}
