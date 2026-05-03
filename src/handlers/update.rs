use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;

use crate::codegen::ddl::quote_ident;
use crate::schema::{actor::InvokerCtx, FieldType, Schema};
use crate::validate::{self, Op};

use super::row::{build_entry_map, now_iso8601, read_row};

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
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
                    return Some(vec![s.trim_end_matches('\n').to_string()]);
                }
                return std::fs::read_to_string(path)
                    .ok()
                    .map(|s| vec![s.trim_end_matches('\n').to_string()]);
            }
        }
        match matches.try_get_many::<String>(cli_name) {
            Ok(Some(vals)) => {
                let collected: Vec<String> = vals.cloned().collect();
                if collected.is_empty() {
                    None
                } else {
                    Some(collected)
                }
            }
            _ => None,
        }
    })?;

    // Merge diff into existing; deep-merge Record-typed fields so sibling
    // sub-fields not present in the diff are preserved.
    let mut merged = existing.clone();
    for (k, v) in &diff {
        let is_record = schema
            .fields
            .iter()
            .any(|f| f.name == *k && matches!(f.ty, crate::schema::FieldType::Record(_)));
        if is_record {
            if let (Some(Value::Object(existing_obj)), Value::Object(new_obj)) =
                (merged.get(k).cloned(), v)
            {
                let mut combined = existing_obj.clone();
                for (sk, sv) in new_obj {
                    combined.insert(sk.clone(), sv.clone());
                }
                merged.insert(k.clone(), Value::Object(combined));
                continue;
            }
        }
        merged.insert(k.clone(), v.clone());
    }

    // Run validator against merged entry; actor checks scoped to diff only.
    validate::validate(schema, &merged, Op::Update(diff.clone()), invoker)
        .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    // Build SET clause for only the fields in diff + updated_*
    let now = now_iso8601();
    let invoker_str = invoker.actor.to_string();

    let mut set_parts: Vec<String> = vec![format!("updated_at = ?1"), format!("updated_by = ?2")];
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
                FieldType::Record(_) => {
                    // Use the deep-merged value (not the partial diff) so sibling
                    // sub-fields preserved in `merged` are written to the DB.
                    let write_val = merged.get(&field.name).unwrap_or(new_val);
                    let json_str =
                        serde_json::to_string(write_val).unwrap_or_else(|_| "null".to_string());
                    sql_values.push(rusqlite::types::Value::Text(json_str));
                }
                FieldType::List(_)
                | FieldType::ListRecord(_)
                | FieldType::ListFk { .. }
                | FieldType::Json => {
                    let json_str =
                        serde_json::to_string(new_val).unwrap_or_else(|_| "null".to_string());
                    sql_values.push(rusqlite::types::Value::Text(json_str));
                }
                FieldType::Bool => {
                    let i = match new_val {
                        Value::Bool(b) => {
                            if *b {
                                1
                            } else {
                                0
                            }
                        }
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
        quote_ident(&schema.name)
    );

    conn.execute(&sql, rusqlite::params_from_iter(sql_values.iter()))
        .context("update row")?;

    println!("Updated {display_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::schema::actor::Actor;
    use crate::schema::Schema;

    const RECORD_SCHEMA: &str = r#"
name: rstore
id_format: "R{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: details
    type: record
    fields:
      - name: notes
        type: text
      - name: severity
        type: text
"#;

    fn build_cmd(schema: &Schema, verb: &'static str, with_display_id: bool) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd = clap::Command::new(verb);
        if with_display_id {
            cmd = cmd.arg(clap::Arg::new("display_id").required(true));
        }
        for leaf in &leaves {
            cmd = cmd.arg(
                clap::Arg::new(leaf.cli_name.clone())
                    .long(leaf.cli_name.clone())
                    .required(false),
            );
        }
        cmd
    }

    fn setup() -> (Schema, Connection) {
        let schema = Schema::from_yaml(RECORD_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    #[test]
    fn update_record_subfield_preserves_siblings() {
        let (schema, conn) = setup();

        // INSERT row with both sub-fields populated (cli_name = sub-field name only)
        let add_cmd = build_cmd(&schema, "add", false);
        let add_matches =
            add_cmd.get_matches_from(["add", "--notes", "keep-me", "--severity", "info"]);
        crate::handlers::add::run(&schema, &conn, &add_matches, Actor::Human.into()).unwrap();

        // UPDATE only severity
        let upd_cmd = build_cmd(&schema, "update", true);
        let upd_matches = upd_cmd.get_matches_from(["update", "R001", "--severity", "warning"]);
        run(&schema, &conn, &upd_matches, Actor::Human.into()).unwrap();

        // Read back and assert notes is preserved, severity is updated
        let json_str: String = conn
            .query_row(
                "SELECT details FROM rstore WHERE display_id = 'R001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            v["notes"], "keep-me",
            "notes must be preserved after partial Record update"
        );
        assert_eq!(v["severity"], "warning", "severity must reflect the update");
    }
}
