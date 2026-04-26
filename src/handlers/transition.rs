use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;

use crate::schema::{actor::Actor, FieldType, Schema};
use crate::validate::{self, Op};

use super::row::{build_entry_map, now_iso8601, read_row};

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: Actor,
    verb: &str,
) -> Result<()> {
    // Resolve the transition definition from schema
    let transition = schema
        .lifecycle
        .transitions
        .iter()
        .find(|t| t.verb == verb)
        .ok_or_else(|| anyhow::anyhow!("no transition with verb '{}' in schema", verb))?;

    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    // Read existing row
    let (row_id, existing) = read_row(schema, conn, display_id)?;

    // State-machine legality check (in handler, not validator per Phase 5 review)
    let current_status = existing
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if current_status != transition.from {
        anyhow::bail!(
            "cannot {verb}: row is in state '{}', expected '{}'",
            current_status,
            transition.from
        );
    }

    // Build diff entry from CLI args
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
                return std::fs::read_to_string(path)
                    .ok()
                    .map(|s| s.trim_end_matches('\n').to_string());
            }
        }
        matches.get_one::<String>(cli_name).cloned()
    })?;

    // Deep-merge diff into existing; Record-typed fields get sub-field-level merge
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
    validate::validate(schema, &merged, Op::Transition(verb.to_string(), diff.clone()), invoker).map_err(
        |errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)),
    )?;

    // Write: UPDATE merged fields + status = transition.to + updated_*
    let now = now_iso8601();
    let invoker_str = invoker.to_string();

    let mut set_parts: Vec<String> = vec![
        "updated_at = ?1".to_string(),
        "updated_by = ?2".to_string(),
        format!("status = ?3"),
    ];
    let mut sql_values: Vec<rusqlite::types::Value> = vec![
        rusqlite::types::Value::Text(now),
        rusqlite::types::Value::Text(invoker_str),
        rusqlite::types::Value::Text(transition.to.clone()),
    ];
    let mut param_idx = 4usize;

    // Write every field that appeared in the diff (use merged value for Records)
    for field in &schema.fields {
        if let Some(new_val) = diff.get(&field.name) {
            set_parts.push(format!("{} = ?{param_idx}", field.name));
            param_idx += 1;

            match &field.ty {
                FieldType::Record(_) => {
                    let write_val = merged.get(&field.name).unwrap_or(new_val);
                    let json_str = serde_json::to_string(write_val)
                        .unwrap_or_else(|_| "null".to_string());
                    sql_values.push(rusqlite::types::Value::Text(json_str));
                }
                FieldType::List(_) => {
                    let json_str = serde_json::to_string(new_val)
                        .unwrap_or_else(|_| "null".to_string());
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

    let where_param_idx = param_idx;
    sql_values.push(rusqlite::types::Value::Integer(row_id));

    let set_clause = set_parts.join(", ");
    let sql = format!(
        "UPDATE {} SET {set_clause} WHERE id = ?{where_param_idx}",
        schema.name
    );

    conn.execute(&sql, rusqlite::params_from_iter(sql_values.iter()))
        .context("transition update row")?;

    println!(
        "Transitioned {display_id}: {} → {}",
        transition.from, transition.to
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::schema::Schema;

    const OBS_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
default_actor: ai_with_human
lifecycle:
  states: [open, triaged, resolved, wont_fix]
  transitions:
    - from: open
      to: triaged
      verb: triage
      actor: ai_with_human
    - from: triaged
      to: resolved
      verb: resolve
      actor: ai_autonomous
    - from: triaged
      to: wont_fix
      verb: wont_fix
      actor: ai_with_human
fields:
  - name: summary
    type: text
    required: true
  - name: triage
    type: record
    fields:
      - name: verdict
        type: enum
        enum_values: [T1, T2, T3]
      - name: notes
        type: text
        required: false
  - name: contract
    type: record
    fields:
      - name: done_when
        type: text
        required_when: "triage.verdict == 'T3'"
      - name: scope_in
        type: text
        required_when: "triage.verdict == 'T3'"
      - name: scope_out
        type: text
        required_when: "triage.verdict == 'T3'"
  - name: tags
    type:
      list: text
    required: false
"#;

    fn build_cmd(schema: &Schema, verb: &'static str) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd = clap::Command::new(verb).arg(
            clap::Arg::new("display_id")
                .required(true)
                .index(1),
        );
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
        let schema = Schema::from_yaml(OBS_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    fn insert_open_row(schema: &Schema, conn: &Connection) {
        let add_cmd = {
            let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
            let mut cmd = clap::Command::new("add");
            for leaf in &leaves {
                cmd = cmd.arg(
                    clap::Arg::new(leaf.cli_name.clone())
                        .long(leaf.cli_name.clone())
                        .required(false),
                );
            }
            cmd
        };
        let add_matches = add_cmd.get_matches_from(["add", "--summary", "test observation"]);
        crate::handlers::add::run(schema, conn, &add_matches, Actor::Human).unwrap();
    }

    #[test]
    fn triage_t3_without_contract_fails() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from(["triage", "L001", "--verdict", "T3"]);
        let err = run(&schema, &conn, &matches, Actor::Human, "triage").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("done_when") || msg.contains("validation failed"), "expected contract error: {msg}");
    }

    #[test]
    fn triage_t3_with_contract_succeeds() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from([
            "triage", "L001",
            "--verdict", "T3",
            "--done-when", "X works",
            "--scope-in", "backend",
            "--scope-out", "frontend",
        ]);
        run(&schema, &conn, &matches, Actor::Human, "triage").unwrap();

        let status: String = conn
            .query_row("SELECT status FROM observations WHERE display_id = 'L001'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "triaged");
    }

    #[test]
    fn state_machine_rejects_wrong_from_state() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        // First triage succeeds
        let cmd = build_cmd(&schema, "triage");
        let matches = cmd.get_matches_from([
            "triage", "L001",
            "--verdict", "T1",
        ]);
        run(&schema, &conn, &matches, Actor::Human, "triage").unwrap();

        // Second triage is rejected
        let cmd2 = build_cmd(&schema, "triage");
        let matches2 = cmd2.get_matches_from(["triage", "L001", "--verdict", "T1"]);
        let err = run(&schema, &conn, &matches2, Actor::Human, "triage").unwrap_err();
        assert!(
            err.to_string().contains("cannot triage"),
            "expected state-machine error: {}",
            err
        );
        assert!(
            err.to_string().contains("triaged"),
            "error should mention current state: {}",
            err
        );
    }

    #[test]
    fn resolve_transition_from_triaged_succeeds() {
        let (schema, conn) = setup();
        insert_open_row(&schema, &conn);

        // Triage first
        let triage_cmd = build_cmd(&schema, "triage");
        let triage_matches =
            triage_cmd.get_matches_from(["triage", "L001", "--verdict", "T1"]);
        run(&schema, &conn, &triage_matches, Actor::Human, "triage").unwrap();

        // Resolve (actor: ai_autonomous)
        let resolve_cmd = build_cmd(&schema, "resolve");
        let resolve_matches = resolve_cmd.get_matches_from(["resolve", "L001"]);
        run(&schema, &conn, &resolve_matches, Actor::AiAutonomous, "resolve").unwrap();

        let status: String = conn
            .query_row("SELECT status FROM observations WHERE display_id = 'L001'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(status, "resolved");
    }
}
