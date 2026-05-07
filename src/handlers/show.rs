use anyhow::{anyhow, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::schema::{actor::InvokerCtx, Schema};

use super::row::read_row;
use crate::output;

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: InvokerCtx,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let json_flag = matches.get_flag("json");

    let (_id, entry) = read_row(schema, conn, display_id)?;

    if let Some(field) = matches.get_one::<String>("field") {
        let value = select_field(&entry, field)?;
        if json_flag {
            output::print_value_json(value);
        } else {
            output::print_selected_value(value);
        }
    } else if json_flag {
        output::print_entry_json(&entry);
    } else {
        output::print_entry_text(&entry);
    }

    Ok(())
}

fn select_field<'a>(entry: &'a BTreeMap<String, Value>, path: &str) -> Result<&'a Value> {
    if path.is_empty() {
        return Err(anyhow!("missing field path: {path}"));
    }

    let mut parts = path.split('.');
    let first = parts.next().unwrap_or_default();
    let mut current = entry
        .get(first)
        .ok_or_else(|| anyhow!("missing field '{path}'"))?;

    for part in parts {
        current = match current {
            Value::Object(obj) => obj
                .get(part)
                .ok_or_else(|| anyhow!("missing field '{path}'"))?,
            Value::Array(arr) => {
                let idx: usize = part
                    .parse()
                    .map_err(|_| anyhow!("missing field '{path}'"))?;
                arr.get(idx)
                    .ok_or_else(|| anyhow!("missing field '{path}'"))?
            }
            _ => return Err(anyhow!("missing field '{path}'")),
        };
    }

    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn select_field_reads_nested_path() {
        let mut entry = BTreeMap::new();
        entry.insert("contract".to_string(), json!({"done_when": "done"}));
        assert_eq!(select_field(&entry, "contract.done_when").unwrap(), "done");
    }

    #[test]
    fn select_field_reports_missing_path() {
        let entry = BTreeMap::new();
        let err = select_field(&entry, "no_such_field").unwrap_err();
        assert!(err.to_string().contains("no_such_field"));
    }
}
