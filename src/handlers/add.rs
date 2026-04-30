use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;

use crate::id_format;
use crate::schema::{actor::Actor, FieldType, Schema};
use crate::validate::{self, Op};

use super::row::{build_entry_map, now_iso8601};

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: Actor,
) -> Result<()> {
    // Build entry from CLI args
    let entry = build_entry_map(schema, |cli_name| {
        // Check for --<name>-from-file first (only if clap registered it)
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

    // Run validator
    validate::validate(schema, &entry, Op::Add, invoker).map_err(|errs| {
        anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs))
    })?;

    // Populate reserved fields
    let now = now_iso8601();
    let initial_status = schema.lifecycle.resolved_initial_state()?.to_string();
    let invoker_str = invoker.to_string();

    // Collect columns + values for INSERT
    // Reserved: display_id (placeholder ""), status, created_at, updated_at,
    //           created_by, updated_by
    // Schema fields: iterate, serialize Record/List as JSON

    let mut col_names: Vec<String> = vec![
        "display_id".to_string(),
        "status".to_string(),
        "created_at".to_string(),
        "updated_at".to_string(),
        "created_by".to_string(),
        "updated_by".to_string(),
    ];
    let mut placeholders: Vec<String> = vec![
        "?1".to_string(),
        "?2".to_string(),
        "?3".to_string(),
        "?4".to_string(),
        "?5".to_string(),
        "?6".to_string(),
    ];
    let mut values: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text("__PLACEHOLDER__".to_string()),
        rusqlite::types::Value::Text(initial_status.clone()),
        rusqlite::types::Value::Text(now.clone()),
        rusqlite::types::Value::Text(now.clone()),
        rusqlite::types::Value::Text(invoker_str.clone()),
        rusqlite::types::Value::Text(invoker_str.clone()),
    ];

    let mut param_idx = 7usize;
    for field in &schema.fields {
        let val = entry.get(&field.name);
        col_names.push(field.name.clone());
        placeholders.push(format!("?{param_idx}"));
        param_idx += 1;

        match &field.ty {
            FieldType::Record(_) | FieldType::List(_) | FieldType::ListRecord(_) | FieldType::ListFk { .. } => {
                // Serialize to JSON string
                let json_str = match val {
                    Some(v) => serde_json::to_string(v)
                        .unwrap_or_else(|_| "null".to_string()),
                    None => "null".to_string(),
                };
                values.push(rusqlite::types::Value::Text(json_str));
            }
            FieldType::Bool => {
                let sql_val = match val {
                    Some(Value::Bool(b)) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
                    Some(Value::Number(n)) => rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0)),
                    _ => rusqlite::types::Value::Null,
                };
                values.push(sql_val);
            }
            FieldType::Integer => {
                let sql_val = match val {
                    Some(Value::Number(n)) => {
                        rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0))
                    }
                    Some(Value::String(s)) => {
                        // coerce_value may produce String if parse failed
                        s.parse::<i64>()
                            .map(rusqlite::types::Value::Integer)
                            .unwrap_or(rusqlite::types::Value::Null)
                    }
                    _ => rusqlite::types::Value::Null,
                };
                values.push(sql_val);
            }
            _ => {
                let sql_val = match val {
                    Some(Value::String(s)) => rusqlite::types::Value::Text(s.clone()),
                    Some(Value::Number(n)) => rusqlite::types::Value::Text(n.to_string()),
                    Some(Value::Bool(b)) => rusqlite::types::Value::Text(b.to_string()),
                    _ => rusqlite::types::Value::Null,
                };
                values.push(sql_val);
            }
        }
    }

    let col_list = col_names.join(", ");
    let ph_list = placeholders.join(", ");
    let sql = format!("INSERT INTO {} ({col_list}) VALUES ({ph_list})", schema.name);

    // Execute inside a transaction; render display_id from last_insert_rowid
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(&sql).context("prepare insert")?;
        stmt.execute(rusqlite::params_from_iter(values.iter()))?;
    }
    let rowid = tx.last_insert_rowid();
    let display_id = id_format::render(&schema.id_format, rowid);
    tx.execute(
        &format!("UPDATE {} SET display_id = ?1 WHERE id = ?2", schema.name),
        rusqlite::params![display_id, rowid],
    )?;
    tx.commit()?;

    println!("{display_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::schema::Schema;

    const MINIMAL_SCHEMA: &str = r#"
name: tstore
id_format: "T{:03d}"
lifecycle:
  states: [new, done]
  transitions: []
fields:
  - name: title
    type: text
"#;

    // T006 Phase 2 — schema with a list_record field for round-trip tests
    const LIST_RECORD_SCHEMA: &str = r#"
name: lrstore
id_format: "R{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
    required: true
  - name: external_refs
    type: list_record
    fields:
      - name: system
        type: text
        required: true
      - name: kind
        type: text
        required: true
      - name: id
        type: text
        required: true
"#;

    fn build_test_add_cmd(schema: &Schema) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd = clap::Command::new("add");
        for leaf in &leaves {
            cmd = cmd.arg(clap::Arg::new(leaf.cli_name.clone()).long(leaf.cli_name.clone()).required(false));
        }
        cmd
    }

    fn in_memory_schema_and_conn() -> (Schema, Connection) {
        let schema = Schema::from_yaml(MINIMAL_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        // Create table
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    #[test]
    fn add_sets_initial_status_to_first_state() {
        let (schema, conn) = in_memory_schema_and_conn();

        let cmd = build_test_add_cmd(&schema);
        let matches = cmd.get_matches_from(["add"]);

        run(&schema, &conn, &matches, Actor::Human).unwrap();

        let status: String = conn
            .query_row("SELECT status FROM tstore WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "new", "status must equal lifecycle.states[0]");
    }

    #[test]
    fn add_populates_created_and_updated_fields() {
        let (schema, conn) = in_memory_schema_and_conn();
        let cmd = build_test_add_cmd(&schema);
        let matches = cmd.get_matches_from(["add"]);

        run(&schema, &conn, &matches, Actor::Human).unwrap();

        let (created_at, updated_at, created_by, updated_by): (String, String, String, String) =
            conn.query_row(
                "SELECT created_at, updated_at, created_by, updated_by FROM tstore WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();

        assert!(!created_at.is_empty(), "created_at must be set");
        assert!(!updated_at.is_empty(), "updated_at must be set");
        assert!(!created_by.is_empty(), "created_by must be set");
        assert!(!updated_by.is_empty(), "updated_by must be set");
        assert_eq!(created_by, "human");
    }

    #[test]
    fn add_display_id_rendered_from_rowid() {
        let (schema, conn) = in_memory_schema_and_conn();
        let cmd = build_test_add_cmd(&schema);
        let matches = cmd.get_matches_from(["add"]);

        run(&schema, &conn, &matches, Actor::Human).unwrap();

        let display_id: String = conn
            .query_row("SELECT display_id FROM tstore WHERE id = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(display_id, "T001");
    }

    // ---- T006 Phase 2: list_record CLI round-trip ----

    fn list_record_schema_and_conn() -> (Schema, Connection) {
        let schema = Schema::from_yaml(LIST_RECORD_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    /// AC P2-1: add with a valid list_record JSON arg round-trips via read_row as
    /// Value::Array (not a string blob).
    #[test]
    fn list_record_cli_round_trips_as_array() {
        let (schema, conn) = list_record_schema_and_conn();
        let cmd = build_test_add_cmd(&schema);
        let raw_refs = r#"[{"system":"docker","kind":"container","id":"foo"}]"#;
        let matches = cmd.get_matches_from([
            "add",
            "--title", "test row",
            "--external-refs", raw_refs,
        ]);

        run(&schema, &conn, &matches, Actor::Human).unwrap();

        let (_, entry) = crate::handlers::row::read_row(&schema, &conn, "R001").unwrap();
        let refs_val = entry.get("external_refs").expect("external_refs must be present");
        match refs_val {
            serde_json::Value::Array(arr) => {
                assert_eq!(arr.len(), 1, "expected 1 element");
                assert_eq!(arr[0]["system"], "docker");
                assert_eq!(arr[0]["kind"], "container");
                assert_eq!(arr[0]["id"], "foo");
            }
            other => panic!(
                "external_refs must round-trip as Value::Array, got: {:?}",
                other
            ),
        }
    }

    /// AC P2-2 (list_record_bad_json_returns_validator_error): passing bad JSON for a
    /// required list_record field fails validation with an error that mentions the field name
    /// AND a JSON/array hint.
    ///
    /// T006 REVISE 1: coerce_value now returns Value::String(raw) on parse failure so the
    /// type-shape validator fires for both required and optional fields.
    #[test]
    fn list_record_bad_json_returns_validator_error() {
        const REQUIRED_LR_SCHEMA: &str = r#"
name: lrreq
id_format: "Q{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
  - name: external_refs
    type: list_record
    required: true
    fields:
      - name: system
        type: text
"#;
        let schema = Schema::from_yaml(REQUIRED_LR_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema)).unwrap();

        let cmd = build_test_add_cmd(&schema);
        let matches = cmd.get_matches_from([
            "add",
            "--external-refs", "{not json",
        ]);

        let err = run(&schema, &conn, &matches, Actor::Human).unwrap_err();
        let msg = err.to_string();
        // Must mention the field name AND include a JSON/array hint (REVISE 1 wording check)
        assert!(
            msg.contains("external_refs"),
            "error must mention field name 'external_refs'; got: {msg}"
        );
        assert!(
            msg.contains("JSON array") || msg.contains("json array") || msg.contains("array"),
            "error must hint at JSON array requirement; got: {msg}"
        );
    }

    /// AC P2-2 (REVISE 1): list_record_bad_json_optional_field_still_errors — passing bad
    /// JSON for an OPTIONAL list_record field MUST also produce a validation error.
    ///
    /// This is the critical case the original implementation missed: Value::Null is a valid
    /// nullable value so the required-rule never fires for optional fields. Value::String(raw)
    /// sentinel routes through the type-shape check which fires unconditionally.
    #[test]
    fn list_record_bad_json_optional_field_still_errors() {
        // Schema with external_refs OPTIONAL (no required: true at field level)
        const OPTIONAL_LR_SCHEMA: &str = r#"
name: lropt
id_format: "P{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
  - name: external_refs
    type: list_record
    fields:
      - name: system
        type: text
        required: true
"#;
        let schema = Schema::from_yaml(OPTIONAL_LR_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema)).unwrap();

        let cmd = build_test_add_cmd(&schema);
        let matches = cmd.get_matches_from([
            "add",
            "--external-refs", "{not json",
        ]);

        let err = run(&schema, &conn, &matches, Actor::Human).unwrap_err();
        let msg = err.to_string();
        // Must mention the field name AND include a JSON/array hint
        assert!(
            msg.contains("external_refs"),
            "error must mention field name 'external_refs' even for optional field; got: {msg}"
        );
        assert!(
            msg.contains("JSON array") || msg.contains("json array") || msg.contains("array"),
            "error must hint at JSON array requirement; got: {msg}"
        );
    }
}
