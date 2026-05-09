use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;

use crate::codegen::ddl::quote_ident;
use crate::schema::{actor::InvokerCtx, FieldType, Schema};
use crate::validate::{self, Op};

use super::row::{build_entry_map, deep_merge_entry_field, now_iso8601, read_row};

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
    let mut diff = build_entry_map(schema, |cli_name| {
        // --acceptance-from-file: read one criterion per line (observations only)
        if cli_name == "acceptance"
            && matches
                .try_contains_id("acceptance-from-file")
                .unwrap_or(false)
        {
            if let Some(path) = matches.get_one::<String>("acceptance-from-file") {
                let lines: Vec<String> = if path == "-" {
                    use std::io::Read;
                    let mut s = String::new();
                    std::io::stdin().read_to_string(&mut s).ok();
                    s.lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(str::to_string)
                        .collect()
                } else {
                    std::fs::read_to_string(path)
                        .map(|s| {
                            s.lines()
                                .filter(|l| !l.trim().is_empty())
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default()
                };
                if !lines.is_empty() {
                    return Some(lines);
                }
            }
        }

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

    if schema.name == "observations" {
        super::observations_source::normalize_cli_source_tuple(matches, &mut diff)?;
    }

    // Merge diff into existing; deep-merge Record-typed fields recursively so
    // sibling sub-fields at any nested depth are preserved.
    let mut merged = existing.clone();
    for (k, v) in &diff {
        let is_record = schema
            .fields
            .iter()
            .any(|f| f.name == *k && matches!(f.ty, crate::schema::FieldType::Record(_)));
        if is_record {
            deep_merge_entry_field(&mut merged, k, v);
        } else {
            merged.insert(k.clone(), v.clone());
        }
    }

    // L143 / T052: `approval_policy` is verb-owned. The dedicated
    // `override-policy` verb encodes the direction-aware tier-A/tier-B gate
    // (relaxation human→auto needs a token; escalation can be tier-B). The
    // generic `update` verb can't see direction without re-reading the
    // existing row, and even if it did the gate logic would have to be
    // duplicated in two places. Reject the field here and force callers
    // through the named verb. Codex T052 round 1 caught this leak.
    if schema.name == "observations" && diff.contains_key("approval_policy") {
        anyhow::bail!(
            "approval_policy must be changed via `stores observations override-policy`; \
             generic update is not the right surface for direction-gated authority fields"
        );
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

    const OBS_LIST_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: summary
    type: text
  - name: risk_flags
    type:
      list: text
    actor: ai_with_human
"#;

    fn build_cmd(schema: &Schema, verb: &'static str, with_display_id: bool) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd = clap::Command::new(verb);
        if with_display_id {
            cmd = cmd.arg(clap::Arg::new("display_id").required(true));
        }
        for leaf in &leaves {
            let mut arg = clap::Arg::new(leaf.cli_name.clone())
                .long(leaf.cli_name.clone())
                .required(false);
            if matches!(leaf.field.ty, crate::schema::FieldType::List(_)) {
                arg = arg.action(clap::ArgAction::Append);
            }
            cmd = cmd.arg(arg);
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

    fn observations_setup() -> (Schema, Connection) {
        let schema = Schema::from_yaml(OBS_LIST_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        let add_cmd = build_cmd(&schema, "add", false);
        let add_matches = add_cmd.get_matches_from(["add", "--summary", "row"]);
        crate::handlers::add::run(&schema, &conn, &add_matches, Actor::Human.into()).unwrap();
        (schema, conn)
    }

    fn stored_risk_flags(conn: &Connection) -> Vec<String> {
        let raw: String = conn
            .query_row(
                "SELECT risk_flags FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str::<Vec<String>>(&raw).unwrap()
    }

    #[test]
    fn observations_update_risk_flags_repeated_values() {
        let (schema, conn) = observations_setup();
        let upd_cmd = build_cmd(&schema, "update", true);
        let upd_matches =
            upd_cmd.get_matches_from(["update", "L001", "--risk-flags", "A", "--risk-flags", "B"]);
        run(&schema, &conn, &upd_matches, Actor::AiWithHuman.into()).unwrap();
        assert_eq!(stored_risk_flags(&conn), vec!["A", "B"]);
    }

    #[test]
    fn observations_update_risk_flags_comma_single_value() {
        let (schema, conn) = observations_setup();
        let upd_cmd = build_cmd(&schema, "update", true);
        let upd_matches = upd_cmd.get_matches_from(["update", "L001", "--risk-flags", "A,B"]);
        run(&schema, &conn, &upd_matches, Actor::AiWithHuman.into()).unwrap();
        assert_eq!(stored_risk_flags(&conn), vec!["A", "B"]);
    }

    #[test]
    fn observations_update_risk_flags_single_value_stored_as_array() {
        let (schema, conn) = observations_setup();
        let upd_cmd = build_cmd(&schema, "update", true);
        let upd_matches = upd_cmd.get_matches_from(["update", "L001", "--risk-flags", "A"]);
        run(&schema, &conn, &upd_matches, Actor::AiWithHuman.into()).unwrap();
        assert_eq!(stored_risk_flags(&conn), vec!["A"]);
    }

    // T117: structured error for invalid list-field value (names flag + rejected value)
    #[test]
    fn t117_invalid_list_value_error_names_flag_and_rejected_value() {
        let (schema, conn) = observations_setup();
        let upd_cmd = build_cmd(&schema, "update", true);
        // dangling escape triggers split_csvish error
        let upd_matches = upd_cmd.get_matches_from(["update", "L001", "--risk-flags", r"bad\"]);
        let err = run(&schema, &conn, &upd_matches, Actor::AiWithHuman.into()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--risk-flags"),
            "error must name the flag '--risk-flags'; got: {msg}"
        );
        assert!(
            msg.contains(r"bad\"),
            "error must include the rejected value; got: {msg}"
        );
    }

    // T117: --acceptance-from-file on observations update
    const OBS_ACCEPTANCE_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: summary
    type: text
  - name: acceptance
    type:
      list: text
"#;

    fn acceptance_setup() -> (Schema, Connection) {
        let schema = Schema::from_yaml(OBS_ACCEPTANCE_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        let add_cmd = build_cmd(&schema, "add", false);
        let add_matches = add_cmd.get_matches_from(["add", "--summary", "row"]);
        crate::handlers::add::run(&schema, &conn, &add_matches, Actor::Human.into()).unwrap();
        (schema, conn)
    }

    fn stored_acceptance(conn: &Connection) -> Vec<String> {
        let raw: String = conn
            .query_row(
                "SELECT acceptance FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str::<Vec<String>>(&raw).unwrap()
    }

    fn build_update_cmd_with_acceptance_from_file(schema: &Schema) -> clap::Command {
        let mut cmd = build_cmd(schema, "update", true);
        cmd = cmd.arg(
            clap::Arg::new("acceptance-from-file")
                .long("acceptance-from-file")
                .required(false),
        );
        cmd
    }

    #[test]
    fn t117_acceptance_from_file_writes_lines_as_list() {
        let (schema, conn) = acceptance_setup();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            "criterion one\ncriterion two\ncriterion three\n",
        )
        .unwrap();

        let upd_cmd = build_update_cmd_with_acceptance_from_file(&schema);
        let upd_matches = upd_cmd.get_matches_from([
            "update",
            "L001",
            "--acceptance-from-file",
            tmp.path().to_str().unwrap(),
        ]);
        run(&schema, &conn, &upd_matches, Actor::Human.into()).unwrap();
        assert_eq!(
            stored_acceptance(&conn),
            vec!["criterion one", "criterion two", "criterion three"]
        );
    }

    #[test]
    fn t117_acceptance_from_file_skips_blank_lines() {
        let (schema, conn) = acceptance_setup();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "first\n\nsecond\n   \nthird\n").unwrap();

        let upd_cmd = build_update_cmd_with_acceptance_from_file(&schema);
        let upd_matches = upd_cmd.get_matches_from([
            "update",
            "L001",
            "--acceptance-from-file",
            tmp.path().to_str().unwrap(),
        ]);
        run(&schema, &conn, &upd_matches, Actor::Human.into()).unwrap();
        assert_eq!(
            stored_acceptance(&conn),
            vec!["first", "second", "third"]
        );
    }
}
