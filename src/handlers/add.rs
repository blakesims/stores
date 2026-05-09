use anyhow::{Context, Result};
use clap::ArgMatches;
use rusqlite::{Connection, Transaction};
use serde_json::Value;

use crate::codegen::ddl::quote_ident;
use crate::id_format;
use crate::schema::{
    actor::{Actor, InvokerCtx},
    FieldType, Schema,
};
use crate::validate::{self, Op, SideEffectAuthority};

use super::row::{build_entry_map, now_iso8601};
use super::submit::fire_on_entry_follow_ons;

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    // T013 P2: --lock-contract shorthand. When present we finalise the
    // intent_contract atomically before validation runs. The flag is only
    // registered on the observations store; for any other store the lookup
    // returns false and this is a no-op.
    let lock_contract = matches.try_contains_id("lock-contract").unwrap_or(false)
        && matches.get_flag("lock-contract");

    // Build entry from CLI args
    let mut entry = build_entry_map(schema, |cli_name| {
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

        // --<name>-from-file takes precedence (single-element vec carrying the file body).
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
        super::observations_source::normalize_cli_source_tuple(matches, &mut entry)?;
    }

    // T013 P2: --lock-contract finalisation. Reject ai_autonomous up front
    // (the lock implies human grounding); then set contract_state=ready and
    // auto-fill drafted_at / approved_at / approved_by where the invoker is
    // permitted to write them. Required-when rules on the contract sub-fields
    // fire during validation below, so a lock without objective/acceptance/
    // in_scope/out_of_scope/tier_hint/type produces the usual missing-field
    // errors.
    if lock_contract {
        if invoker.actor == Actor::AiAutonomous {
            anyhow::bail!(
                "--lock-contract requires human grounding; --invoker ai_autonomous is rejected. \
                 Pass --invoker human, or --invoker ai_with_human --approve-token <T>."
            );
        }

        let now = now_iso8601();
        let approver_permitted = invoker.actor == Actor::Human
            || (invoker.actor == Actor::AiWithHuman && invoker.token_valid);

        let ic = entry
            .entry("intent_contract".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if let Value::Object(map) = ic {
            map.insert(
                "contract_state".to_string(),
                Value::String("ready".to_string()),
            );
            map.entry("drafted_at".to_string())
                .or_insert_with(|| Value::String(now.clone()));
            if approver_permitted {
                map.entry("approved_at".to_string())
                    .or_insert_with(|| Value::String(now.clone()));
                map.entry("approved_by".to_string())
                    .or_insert_with(|| Value::String(invoker.actor.to_string()));
            }
        }
    }

    // T013 P3: tier_hint inheritance on tasks add. When --linked-observations
    // is supplied and --tier-hint is absent, look up each linked observation's
    // intent_contract.tier_hint. If the present rows unanimously agree on a
    // single non-null tier, auto-inherit it. Otherwise (disagreement, or any
    // present row missing a tier), bail with a clear error listing each obs
    // and its tier so the user can pass --tier-hint explicitly. Missing
    // observation rows produce a stderr warning and are excluded from the
    // tier set (soft-FK semantics; AC3.5).
    if schema.name == "tasks" && !entry.contains_key("tier_hint") {
        if let Some(Value::Array(linked)) = entry.get("linked_observations").cloned() {
            if !linked.is_empty() {
                let mut found: Vec<(String, Option<String>)> = Vec::new();
                for v in &linked {
                    let Some(obs_id) = v.as_str() else { continue };
                    let raw: Option<String> = conn
                        .query_row(
                            "SELECT intent_contract FROM observations WHERE display_id = ?1",
                            rusqlite::params![obs_id],
                            |r| r.get(0),
                        )
                        .ok();
                    match raw {
                        None => {
                            eprintln!(
                                "warning: linked observation '{obs_id}' not found; \
                                 skipping for tier_hint inference"
                            );
                        }
                        Some(s) => {
                            let tier = serde_json::from_str::<Value>(&s).ok().and_then(|jv| {
                                jv.get("tier_hint")
                                    .and_then(|t| t.as_str())
                                    .map(|s| s.to_string())
                            });
                            found.push((obs_id.to_string(), tier));
                        }
                    }
                }
                if !found.is_empty() {
                    let first = found[0].1.clone();
                    let unanimous = first.is_some() && found.iter().all(|(_, t)| t == &first);
                    if unanimous {
                        entry.insert("tier_hint".to_string(), Value::String(first.unwrap()));
                    } else {
                        let listing = found
                            .iter()
                            .map(|(id, t)| {
                                format!("  {id} -> {}", t.as_deref().unwrap_or("(no tier)"))
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        anyhow::bail!(
                            "linked observations disagree on tier_hint (or some are unset); \
                             pass --tier-hint <T1|T2|T3> explicitly:\n{listing}"
                        );
                    }
                }
            }
        }
    }

    // Run validator
    validate::validate(schema, &entry, Op::Add, invoker)
        .map_err(|errs| anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs)))?;

    // T052 P1: materialise field-level defaults for any top-level field the CLI
    // did not supply. Applied AFTER validation so framework-applied defaults
    // never trip actor checks (overrides remain gated; defaults are
    // conservative, declared in the schema, and not user writes).
    for field in &schema.fields {
        if let Some(default_value) = &field.default {
            let absent = matches!(entry.get(&field.name), None | Some(Value::Null));
            if absent {
                entry.insert(field.name.clone(), default_value.clone());
            }
        }
    }

    // L001: optional --display-id override. When supplied, parse + collision-check
    // up-front so we fail before any INSERT.
    let explicit_display_id: Option<(String, i64)> =
        match matches.try_get_one::<String>("display-id").ok().flatten() {
            Some(supplied) => {
                let parsed_id = id_format::parse(&schema.id_format, supplied)
                    .map_err(|e| anyhow::anyhow!("--display-id format error: {e}"))?;
                // Collision check against existing rows.
                let existing: Option<i64> = conn
                    .query_row(
                        &format!(
                            "SELECT id FROM {} WHERE display_id = ?1",
                            quote_ident(&schema.name)
                        ),
                        rusqlite::params![supplied],
                        |r| r.get(0),
                    )
                    .ok();
                if existing.is_some() {
                    anyhow::bail!(
                        "--display-id collision: '{}' already exists in store '{}'",
                        supplied,
                        schema.name
                    );
                }
                Some((supplied.clone(), parsed_id))
            }
            None => None,
        };

    // Populate reserved fields
    let now = now_iso8601();
    let initial_status = schema.lifecycle.resolved_initial_state()?.to_string();
    let invoker_str = invoker.actor.to_string();

    // T020 P2 (Decision Matrix Q1): --lock-contract on observations lands the row
    // directly at 'confirmed'. The synthetic open→investigating→confirmed walk
    // markers are written into transition_history below; the post-INSERT
    // auto-ratify hook (Phase 1, Task 1.3) then fires confirmed→ready.
    let effective_initial_status = if lock_contract && schema.name == "observations" {
        "confirmed".to_string()
    } else {
        initial_status.clone()
    };

    // Collect columns + values for INSERT
    // Reserved: display_id (placeholder ""), status, created_at, updated_at,
    //           created_by, updated_by
    // Schema fields: iterate, serialize Record/List as JSON

    let mut col_names: Vec<String> = Vec::new();
    let mut placeholders: Vec<String> = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();

    // L001: if --display-id was supplied, INSERT an explicit `id` column first so
    // the resulting rowid matches the supplied display_id's numeric portion.
    if let Some((_, parsed_id)) = &explicit_display_id {
        col_names.push("id".to_string());
        placeholders.push(format!("?{}", values.len() + 1));
        values.push(rusqlite::types::Value::Integer(*parsed_id));
    }

    // Reserved fields (display_id is a placeholder; we UPDATE it after insert
    // unless --display-id was supplied, in which case we set it directly here.)
    col_names.push("display_id".to_string());
    placeholders.push(format!("?{}", values.len() + 1));
    let display_id_placeholder = match &explicit_display_id {
        Some((supplied, _)) => supplied.clone(),
        None => "__PLACEHOLDER__".to_string(),
    };
    values.push(rusqlite::types::Value::Text(display_id_placeholder));

    for (col, val) in [
        ("status", effective_initial_status.clone()),
        ("created_at", now.clone()),
        ("updated_at", now.clone()),
        ("created_by", invoker_str.clone()),
        ("updated_by", invoker_str.clone()),
    ] {
        col_names.push(col.to_string());
        placeholders.push(format!("?{}", values.len() + 1));
        values.push(rusqlite::types::Value::Text(val));
    }

    let mut param_idx = values.len() + 1;
    for field in &schema.fields {
        let val = entry.get(&field.name);
        col_names.push(field.name.clone());
        placeholders.push(format!("?{param_idx}"));
        param_idx += 1;

        match &field.ty {
            FieldType::Record(_)
            | FieldType::List(_)
            | FieldType::ListRecord(_)
            | FieldType::ListFk { .. }
            | FieldType::Json => {
                // Serialize to JSON string
                let json_str = match val {
                    Some(v) => serde_json::to_string(v).unwrap_or_else(|_| "null".to_string()),
                    None => "null".to_string(),
                };
                values.push(rusqlite::types::Value::Text(json_str));
            }
            FieldType::Bool => {
                let sql_val = match val {
                    Some(Value::Bool(b)) => rusqlite::types::Value::Integer(if *b { 1 } else { 0 }),
                    Some(Value::Number(n)) => {
                        rusqlite::types::Value::Integer(n.as_i64().unwrap_or(0))
                    }
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
    let sql = format!(
        "INSERT INTO {} ({col_list}) VALUES ({ph_list})",
        quote_ident(&schema.name)
    );

    // Execute inside a transaction; render display_id from last_insert_rowid
    // (or use the explicit one supplied via --display-id).
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(&sql).context("prepare insert")?;
        stmt.execute(rusqlite::params_from_iter(values.iter()))?;
    }
    let rowid = tx.last_insert_rowid();
    let display_id = match &explicit_display_id {
        Some((supplied, _)) => {
            // L001: bump the AUTOINCREMENT counter past the supplied id so the
            // next auto-mint does not collide. SQLite tracks the high-water mark
            // in `sqlite_sequence`; AUTOINCREMENT picks max(sqlite_sequence,
            // max(rowid)) + 1, so we only need to write when our explicit id
            // exceeds the existing sqlite_sequence value.
            let current_seq: Option<i64> = tx
                .query_row(
                    "SELECT seq FROM sqlite_sequence WHERE name = ?1",
                    rusqlite::params![&schema.name],
                    |r| r.get(0),
                )
                .ok();
            match current_seq {
                Some(seq) if seq >= rowid => {
                    // Already past the explicit id — nothing to do.
                }
                Some(_) => {
                    tx.execute(
                        "UPDATE sqlite_sequence SET seq = ?1 WHERE name = ?2",
                        rusqlite::params![rowid, &schema.name],
                    )?;
                }
                None => {
                    tx.execute(
                        "INSERT INTO sqlite_sequence (name, seq) VALUES (?1, ?2)",
                        rusqlite::params![&schema.name, rowid],
                    )?;
                }
            }
            supplied.clone()
        }
        None => {
            let rendered = id_format::render(&schema.id_format, rowid);
            tx.execute(
                &format!(
                    "UPDATE {} SET display_id = ?1 WHERE id = ?2",
                    quote_ident(&schema.name)
                ),
                rusqlite::params![rendered, rowid],
            )?;
            rendered
        }
    };

    // T020 P2 (Task 2.3, Decision Matrix Q2): emit a synthetic 'create' transition
    // row for every successful add. from_status is the empty string '' (NOT NULL)
    // so the daemon's `WHERE from_status = ?2` SQL in agents_run.rs:140 matches it
    // — keeping the empty-string convention consistent across producers.
    crate::db::insert_transition_history(
        &tx,
        &schema.name,
        rowid,
        &display_id,
        "",
        &initial_status,
        "create",
        &invoker_str,
        None,
        None,
        None,
    )?;

    // T020 P2 (Task 2.1 / Decision Matrix Q1): --lock-contract synthesises the
    // open→investigating→confirmed walk and then fires the Phase 1 auto-ratify
    // hook (confirmed→ready) so the row lands at 'ready' in a single transaction.
    if lock_contract && schema.name == "observations" {
        crate::db::insert_transition_history(
            &tx,
            &schema.name,
            rowid,
            &display_id,
            "open",
            "investigating",
            "investigate",
            "framework",
            None,
            None,
            None,
        )?;
        crate::db::insert_transition_history(
            &tx,
            &schema.name,
            rowid,
            &display_id,
            "investigating",
            "confirmed",
            "confirm",
            &invoker_str,
            None,
            None,
            None,
        )?;
        crate::handlers::transition::maybe_auto_ratify_observation(
            &tx,
            schema,
            rowid,
            &display_id,
            &entry,
            None,
            None,
            None,
        )?;
    }

    // T027 P2 (Task 2.3): fire on-entry follow-ons for the initial state of a
    // workflow-shaped row.  Without this, on_state actions on the initial
    // state (e.g. `transition_to: ready` gated by `when: tier_hint == 'T1'`)
    // never fire because no submit-* verb runs before the row is observed.
    if schema.workflow.is_some() {
        fire_on_entry_follow_ons(&tx, schema, &display_id, rowid, &effective_initial_status)?;
    }

    tx.commit()?;

    println!("{display_id}");
    Ok(())
}

/// Add one row inside the caller's transaction using the store schema's typed
/// add path: defaults, validation, auto-minted display_id, and create audit row.
pub(crate) fn add_row_in_tx(
    tx: &Transaction,
    schema: &Schema,
    mut entry: crate::validate::EntryMap,
    invoker: Actor,
) -> Result<String> {
    let initial_status = schema.lifecycle.resolved_initial_state()?.to_string();
    let invoker_str = invoker.to_string();
    let now = now_iso8601();

    entry.insert("status".to_string(), Value::String(initial_status.clone()));
    entry.entry("created_at".to_string()).or_insert_with(|| Value::String(now.clone()));
    entry.entry("updated_at".to_string()).or_insert_with(|| Value::String(now.clone()));
    entry.entry("created_by".to_string()).or_insert_with(|| Value::String(invoker_str.clone()));
    entry.entry("updated_by".to_string()).or_insert_with(|| Value::String(invoker_str.clone()));

    validate::validate(schema, &entry, Op::Add, invoker.into()).map_err(|errs| {
        anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs))
    })?;

    let mut col_names = vec!["display_id".to_string()];
    let mut placeholders = vec!["?1".to_string()];
    let mut values: Vec<rusqlite::types::Value> = vec![rusqlite::types::Value::Text("__PLACEHOLDER__".to_string())];
    let mut param_idx = 2usize;

    for common in ["status", "created_at", "updated_at", "created_by", "updated_by"] {
        if let Some(Value::String(s)) = entry.get(common) {
            col_names.push(common.to_string());
            placeholders.push(format!("?{param_idx}"));
            param_idx += 1;
            values.push(rusqlite::types::Value::Text(s.clone()));
        }
    }

    for field in &schema.fields {
        if field.name == "display_id" {
            continue;
        }
        let Some(val) = entry.get(&field.name) else { continue; };
        col_names.push(field.name.clone());
        placeholders.push(format!("?{param_idx}"));
        param_idx += 1;

        match &field.ty {
            FieldType::Record(_)
            | FieldType::List(_)
            | FieldType::ListRecord(_)
            | FieldType::ListFk { .. }
            | FieldType::Json => values.push(rusqlite::types::Value::Text(
                serde_json::to_string(val).unwrap_or_else(|_| "null".to_string()),
            )),
            FieldType::Bool => {
                let i = match val {
                    Value::Bool(b) => i64::from(*b),
                    Value::Number(n) => n.as_i64().unwrap_or(0),
                    _ => 0,
                };
                values.push(rusqlite::types::Value::Integer(i));
            }
            FieldType::Integer => values.push(rusqlite::types::Value::Integer(val.as_i64().unwrap_or(0))),
            _ => {
                let s = match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                values.push(rusqlite::types::Value::Text(s));
            }
        }
    }

    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote_ident(&schema.name),
        col_names.join(", "),
        placeholders.join(", ")
    );
    tx.execute(&sql, rusqlite::params_from_iter(values.iter()))
        .context("add_row_in_tx: add row")?;

    let rowid = tx.last_insert_rowid();
    let display_id = id_format::render(&schema.id_format, rowid);
    tx.execute(
        &format!("UPDATE {} SET display_id = ?1 WHERE id = ?2", quote_ident(&schema.name)),
        rusqlite::params![display_id, rowid],
    )
    .context("add_row_in_tx: mint display_id")?;

    crate::db::insert_transition_history(
        tx,
        &schema.name,
        rowid,
        &display_id,
        "",
        &initial_status,
        "create",
        &invoker_str,
        None,
        None,
        None,
    )?;

    Ok(display_id)
}

/// Like `add_row_in_tx`, but accepts a `SideEffectAuthority` for named
/// internal framework code-paths.
///
/// This entry point is used exclusively by `insert_observation_row` in the
/// gatekeeper route side-effect path. Generic callers (all CLI-driven adds)
/// use `add_row_in_tx` with `authority=None` and are unaffected.
///
/// `created_by` / `updated_by` are set to the string representation of the
/// invoker actor (e.g. `"framework"`); the provenance of the specific
/// code-path is documented in the calling function's comments.
pub(crate) fn add_row_in_tx_with_authority(
    tx: &Transaction,
    schema: &Schema,
    mut entry: crate::validate::EntryMap,
    invoker: Actor,
    authority: SideEffectAuthority,
) -> Result<String> {
    let initial_status = schema.lifecycle.resolved_initial_state()?.to_string();
    let invoker_str = invoker.to_string();
    let now = now_iso8601();

    entry.insert("status".to_string(), Value::String(initial_status.clone()));
    entry.entry("created_at".to_string()).or_insert_with(|| Value::String(now.clone()));
    entry.entry("updated_at".to_string()).or_insert_with(|| Value::String(now.clone()));
    entry.entry("created_by".to_string()).or_insert_with(|| Value::String(invoker_str.clone()));
    entry.entry("updated_by".to_string()).or_insert_with(|| Value::String(invoker_str.clone()));

    validate::validate_with_authority(schema, &entry, Op::Add, invoker.into(), Some(authority))
        .map_err(|errs| {
            anyhow::anyhow!("validation failed:\n{}", validate::pretty_print(&errs))
        })?;

    let mut col_names = vec!["display_id".to_string()];
    let mut placeholders = vec!["?1".to_string()];
    let mut values: Vec<rusqlite::types::Value> =
        vec![rusqlite::types::Value::Text("__PLACEHOLDER__".to_string())];
    let mut param_idx = 2usize;

    for common in ["status", "created_at", "updated_at", "created_by", "updated_by"] {
        if let Some(Value::String(s)) = entry.get(common) {
            col_names.push(common.to_string());
            placeholders.push(format!("?{param_idx}"));
            param_idx += 1;
            values.push(rusqlite::types::Value::Text(s.clone()));
        }
    }

    for field in &schema.fields {
        if field.name == "display_id" {
            continue;
        }
        let Some(val) = entry.get(&field.name) else {
            continue;
        };
        col_names.push(field.name.clone());
        placeholders.push(format!("?{param_idx}"));
        param_idx += 1;

        match &field.ty {
            FieldType::Record(_)
            | FieldType::List(_)
            | FieldType::ListRecord(_)
            | FieldType::ListFk { .. }
            | FieldType::Json => values.push(rusqlite::types::Value::Text(
                serde_json::to_string(val).unwrap_or_else(|_| "null".to_string()),
            )),
            FieldType::Bool => {
                let i = match val {
                    Value::Bool(b) => i64::from(*b),
                    Value::Number(n) => n.as_i64().unwrap_or(0),
                    _ => 0,
                };
                values.push(rusqlite::types::Value::Integer(i));
            }
            FieldType::Integer => {
                values.push(rusqlite::types::Value::Integer(val.as_i64().unwrap_or(0)))
            }
            _ => {
                let s = match val {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                values.push(rusqlite::types::Value::Text(s));
            }
        }
    }

    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote_ident(&schema.name),
        col_names.join(", "),
        placeholders.join(", ")
    );
    tx.execute(&sql, rusqlite::params_from_iter(values.iter()))
        .context("add_row_in_tx_with_authority: add row")?;

    let rowid = tx.last_insert_rowid();
    let display_id = id_format::render(&schema.id_format, rowid);
    tx.execute(
        &format!(
            "UPDATE {} SET display_id = ?1 WHERE id = ?2",
            quote_ident(&schema.name)
        ),
        rusqlite::params![display_id, rowid],
    )
    .context("add_row_in_tx_with_authority: mint display_id")?;

    crate::db::insert_transition_history(
        tx,
        &schema.name,
        rowid,
        &display_id,
        "",
        &initial_status,
        "create",
        &invoker_str,
        None,
        None,
        None,
    )?;

    Ok(display_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::db;
    use crate::schema::actor::Actor;
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
            cmd = cmd.arg(
                clap::Arg::new(leaf.cli_name.clone())
                    .long(leaf.cli_name.clone())
                    .required(false),
            );
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

        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

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

        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

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

        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

        let display_id: String = conn
            .query_row("SELECT display_id FROM tstore WHERE id = 1", [], |r| {
                r.get(0)
            })
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
        let matches =
            cmd.get_matches_from(["add", "--title", "test row", "--external-refs", raw_refs]);

        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

        let (_, entry) = crate::handlers::row::read_row(&schema, &conn, "R001").unwrap();
        let refs_val = entry
            .get("external_refs")
            .expect("external_refs must be present");
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
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();

        let cmd = build_test_add_cmd(&schema);
        let matches = cmd.get_matches_from(["add", "--external-refs", "{not json"]);

        let err = run(&schema, &conn, &matches, Actor::Human.into()).unwrap_err();
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
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();

        let cmd = build_test_add_cmd(&schema);
        let matches = cmd.get_matches_from(["add", "--external-refs", "{not json"]);

        let err = run(&schema, &conn, &matches, Actor::Human.into()).unwrap_err();
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

    // ---- T006 Phase 3: hyphenated store name CRUD round-trip ----
    //
    // AC Phase 3 trap-test: install a schema with name `obs-test-1006` and exercise
    // add / read_row (show) / list / update / transition.  If any of the 17 SQL
    // interpolation sites was NOT quoted, this test fails on the verb that hits it.

    const HYPHEN_SCHEMA: &str = r#"
name: obs-test-1006
id_format: "O{:03d}"
lifecycle:
  states: [open, reviewed]
  initial_state: open
  transitions:
    - from: open
      to: reviewed
      verb: review
      actor: human
fields:
  - name: summary
    type: text
    required: true
  - name: priority
    type: enum
    enum_values: [low, medium, high]
  - name: tags
    type:
      list: text
"#;

    fn hyphen_schema_and_conn() -> (Schema, Connection) {
        let schema = Schema::from_yaml(HYPHEN_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    fn build_add_cmd_for(schema: &Schema) -> clap::Command {
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
    }

    fn build_verb_cmd_for(schema: &Schema, verb: &'static str) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd =
            clap::Command::new(verb).arg(clap::Arg::new("display_id").required(true).index(1));
        for leaf in &leaves {
            cmd = cmd.arg(
                clap::Arg::new(leaf.cli_name.clone())
                    .long(leaf.cli_name.clone())
                    .required(false),
            );
        }
        cmd
    }

    /// Phase 3 AC trap-test: all CRUD verbs succeed against a store with a hyphenated name.
    /// Exercises sites: add (INSERT + UPDATE display_id), read_row (SELECT), list (SELECT),
    /// update (UPDATE), transition (UPDATE).
    #[test]
    fn hyphenated_store_name_crud_round_trip() {
        let (schema, conn) = hyphen_schema_and_conn();

        // 1. add — tests INSERT INTO and UPDATE display_id sites
        let add_cmd = build_add_cmd_for(&schema);
        let add_matches = add_cmd.get_matches_from([
            "add",
            "--summary",
            "hyphen store test",
            "--priority",
            "high",
            "--tags",
            "a|b",
        ]);
        run(&schema, &conn, &add_matches, Actor::Human.into())
            .expect("add must succeed against hyphenated store name");

        // 2. show (read_row) — tests SELECT FROM site
        let (_, entry) = crate::handlers::row::read_row(&schema, &conn, "O001")
            .expect("read_row must succeed against hyphenated store name");
        assert_eq!(
            entry.get("summary").and_then(|v| v.as_str()),
            Some("hyphen store test"),
            "summary must round-trip"
        );

        // 3. list — tests SELECT FROM list site
        let list_cmd = {
            let mut cmd = clap::Command::new("list");
            cmd = cmd.arg(clap::Arg::new("status").long("status").required(false));
            cmd = cmd.arg(clap::Arg::new("limit").long("limit").required(false));
            cmd = cmd.arg(clap::Arg::new("sort").long("sort").required(false));
            cmd = cmd.arg(
                clap::Arg::new("reverse")
                    .long("reverse")
                    .required(false)
                    .action(clap::ArgAction::SetTrue),
            );
            cmd = cmd.arg(
                clap::Arg::new("json")
                    .long("json")
                    .required(false)
                    .action(clap::ArgAction::SetTrue),
            );
            cmd = cmd.arg(clap::Arg::new("since").long("since").required(false));
            cmd
        };
        let list_matches = list_cmd.get_matches_from(["list"]);
        crate::handlers::list::run(&schema, &conn, &list_matches, Actor::Human.into())
            .expect("list must succeed against hyphenated store name");

        // 4. update — tests UPDATE site
        let update_cmd = build_verb_cmd_for(&schema, "update");
        let update_matches =
            update_cmd.get_matches_from(["update", "O001", "--summary", "updated summary"]);
        crate::handlers::update::run(&schema, &conn, &update_matches, Actor::Human.into())
            .expect("update must succeed against hyphenated store name");

        // Verify update applied
        let (_, entry2) = crate::handlers::row::read_row(&schema, &conn, "O001").unwrap();
        assert_eq!(
            entry2.get("summary").and_then(|v| v.as_str()),
            Some("updated summary"),
            "update must persist to the hyphenated store"
        );

        // 5. transition — tests UPDATE inside execute_transition_write
        let trans_cmd = build_verb_cmd_for(&schema, "review");
        let trans_matches = trans_cmd.get_matches_from(["review", "O001"]);
        crate::handlers::transition::run(
            &schema,
            &conn,
            &trans_matches,
            Actor::Human.into(),
            "review",
        )
        .expect("transition must succeed against hyphenated store name");

        let (_, entry3) = crate::handlers::row::read_row(&schema, &conn, "O001").unwrap();
        assert_eq!(
            entry3.get("status").and_then(|v| v.as_str()),
            Some("reviewed"),
            "transition must update status in the hyphenated store"
        );
    }

    // ---- T006 Phase 4: repeatable list flags (ArgAction::Append) ----
    //
    // Schema with a top-level List(Text) field (`in_scope`) used for all three AC tests.

    const LIST_FLAG_SCHEMA: &str = r#"
name: scopestore
id_format: "S{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
  - name: in_scope
    type:
      list: text
"#;

    fn list_flag_schema_and_conn() -> (Schema, Connection) {
        let schema = Schema::from_yaml(LIST_FLAG_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    /// Build an `add` command where List(_) fields use ArgAction::Append,
    /// mirroring what `cli/dynamic.rs` does at runtime.
    fn build_add_cmd_with_append(schema: &Schema) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd = clap::Command::new("add");
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

    fn get_in_scope(conn: &Connection, display_id: &str) -> Vec<String> {
        let raw: String = conn
            .query_row(
                "SELECT in_scope FROM scopestore WHERE display_id = ?1",
                [display_id],
                |r| r.get(0),
            )
            .unwrap();
        let val: serde_json::Value = serde_json::from_str(&raw).unwrap();
        val.as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    /// AC Phase 4 — pipe-free repeatable form: `--in-scope a --in-scope b` → `["a", "b"]`.
    /// Regression-trap: this form previously errored with "the argument '--in-scope' cannot
    /// be used multiple times". After ArgAction::Append it must succeed and produce two elements.
    #[test]
    fn list_field_repeatable_form() {
        let (schema, conn) = list_flag_schema_and_conn();
        let cmd = build_add_cmd_with_append(&schema);
        let matches = cmd.get_matches_from([
            "add",
            "--title",
            "repeatable test",
            "--in-scope",
            "a",
            "--in-scope",
            "b",
        ]);
        run(&schema, &conn, &matches, Actor::Human.into()).expect("repeatable form must succeed");
        let items = get_in_scope(&conn, "S001");
        assert_eq!(
            items,
            vec!["a", "b"],
            "repeatable form: expected [\"a\", \"b\"], got {:?}",
            items
        );
    }

    /// AC Phase 4 — backwards-compat pipe form: `--in-scope "a|b"` → `["a", "b"]`.
    /// Verifies existing pipe-separated form continues to work after the ArgAction::Append change.
    #[test]
    fn list_field_pipe_form() {
        let (schema, conn) = list_flag_schema_and_conn();
        let cmd = build_add_cmd_with_append(&schema);
        let matches = cmd.get_matches_from(["add", "--title", "pipe test", "--in-scope", "a|b"]);
        run(&schema, &conn, &matches, Actor::Human.into()).expect("pipe form must succeed");
        let items = get_in_scope(&conn, "S001");
        assert_eq!(
            items,
            vec!["a", "b"],
            "pipe form: expected [\"a\", \"b\"], got {:?}",
            items
        );
    }

    /// AC Phase 4 — mixed form: `--in-scope "a|b" --in-scope "c"` → `["a", "b", "c"]`.
    /// The join-with-"|" strategy in the get_arg closure collapses "a|b" + "c" to "a|b|c"
    /// before coerce_value splits on "|", yielding three elements.
    /// NOTE: this also documents the known limitation — a literal "|" within a value is
    /// indistinguishable from a separator (see Decision Matrix).
    #[test]
    fn list_field_mixed_form() {
        let (schema, conn) = list_flag_schema_and_conn();
        let cmd = build_add_cmd_with_append(&schema);
        let matches = cmd.get_matches_from([
            "add",
            "--title",
            "mixed test",
            "--in-scope",
            "a|b",
            "--in-scope",
            "c",
        ]);
        run(&schema, &conn, &matches, Actor::Human.into()).expect("mixed form must succeed");
        let items = get_in_scope(&conn, "S001");
        assert_eq!(
            items,
            vec!["a", "b", "c"],
            "mixed form: expected [\"a\", \"b\", \"c\"], got {:?}",
            items
        );
    }

    // ---- T008 Phase 2: Json field write-then-read integration test ----

    const JSON_SCHEMA: &str = r#"
name: jstore
id_format: "J{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
    required: true
  - name: notes
    type: json
    required: false
"#;

    fn json_schema_and_conn() -> (Schema, Connection) {
        let schema = Schema::from_yaml(JSON_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    /// T008 Phase 2 integration test: add with a JSON object value, read back via read_row,
    /// assert the field round-trips as a structured Value::Object (not a string blob).
    #[test]
    fn json_field_write_then_read_round_trips_as_object() {
        let (schema, conn) = json_schema_and_conn();
        let cmd = build_test_add_cmd(&schema);
        let raw_notes = r#"{"k":"v","arr":[1,2]}"#;
        let matches =
            cmd.get_matches_from(["add", "--title", "json test row", "--notes", raw_notes]);

        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

        // NOTE: read_row currently returns Value::Null for Json columns because Phase 4
        // extends the read-path match to include FieldType::Json. Until Phase 4 ships,
        // verify that the stored TEXT is correctly serialised by querying SQLite directly.
        let stored_notes: String = conn
            .query_row(
                "SELECT notes FROM jstore WHERE display_id = 'J001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // The value stored must be parseable JSON matching the original object
        let parsed: serde_json::Value =
            serde_json::from_str(&stored_notes).expect("stored notes must be valid JSON");
        assert_eq!(parsed["k"], "v", "notes.k must round-trip");
        assert_eq!(
            parsed["arr"],
            serde_json::json!([1, 2]),
            "notes.arr must round-trip"
        );
    }

    /// T008 Phase 2: absent Json field stores the JSON literal "null" (Decision 4).
    #[test]
    fn json_field_absent_stores_null_literal() {
        let (schema, conn) = json_schema_and_conn();
        let cmd = build_test_add_cmd(&schema);
        let matches = cmd.get_matches_from(["add", "--title", "no notes row"]);

        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

        let stored_notes: String = conn
            .query_row(
                "SELECT notes FROM jstore WHERE display_id = 'J001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            stored_notes, "null",
            "absent Json field must store \"null\" literal (Decision 4)"
        );
    }

    // ---- L001: --display-id flag on `add` ----
    //
    // The four AC tests from the L001 contract:
    //   1. explicit --display-id succeeds and stores the supplied id
    //   2. subsequent auto-mint advances past the supplied id (no collision)
    //   3. duplicate --display-id is rejected with a collision error
    //   4. malformed --display-id is rejected with a format error
    // Plus a safety test that the auto-mint path still works when the flag is absent.

    /// Build an `add` command that includes the `--display-id` flag, mirroring
    /// what `cli/dynamic.rs::build_add_cmd` registers at runtime.
    fn build_test_add_cmd_with_display_id(schema: &Schema) -> clap::Command {
        let mut cmd = build_test_add_cmd(schema);
        cmd = cmd.arg(
            clap::Arg::new("display-id")
                .long("display-id")
                .required(false),
        );
        cmd
    }

    #[test]
    fn add_with_explicit_display_id_succeeds() {
        let (schema, conn) = in_memory_schema_and_conn();
        let cmd = build_test_add_cmd_with_display_id(&schema);
        let matches =
            cmd.get_matches_from(["add", "--display-id", "T013", "--title", "explicit id row"]);

        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

        let (rowid, display_id): (i64, String) = conn
            .query_row(
                "SELECT id, display_id FROM tstore WHERE display_id = 'T013'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row with display_id=T013 must exist");
        assert_eq!(
            display_id, "T013",
            "stored display_id must equal supplied value"
        );
        assert_eq!(
            rowid, 13,
            "rowid must equal numeric portion of supplied display id"
        );
    }

    #[test]
    fn add_after_explicit_display_id_advances_auto_mint_past_supplied() {
        let (schema, conn) = in_memory_schema_and_conn();
        let cmd1 = build_test_add_cmd_with_display_id(&schema);
        let m1 = cmd1.get_matches_from(["add", "--display-id", "T013", "--title", "first"]);
        run(&schema, &conn, &m1, Actor::Human.into()).unwrap();

        // Subsequent auto-mint must be T014, not T002 — the AUTOINCREMENT counter
        // must have been bumped past the explicit id.
        let cmd2 = build_test_add_cmd_with_display_id(&schema);
        let m2 = cmd2.get_matches_from(["add", "--title", "second"]);
        run(&schema, &conn, &m2, Actor::Human.into()).unwrap();

        let display_id: String = conn
            .query_row(
                "SELECT display_id FROM tstore WHERE title = 'second'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            display_id, "T014",
            "auto-mint must advance past explicitly-supplied id"
        );
    }

    #[test]
    fn add_with_colliding_display_id_is_rejected() {
        let (schema, conn) = in_memory_schema_and_conn();
        // Seed: insert T013 explicitly.
        let cmd1 = build_test_add_cmd_with_display_id(&schema);
        run(
            &schema,
            &conn,
            &cmd1.get_matches_from(["add", "--display-id", "T013", "--title", "first"]),
            Actor::Human.into(),
        )
        .unwrap();

        // Second insert with the same explicit id must fail.
        let cmd2 = build_test_add_cmd_with_display_id(&schema);
        let m2 = cmd2.get_matches_from(["add", "--display-id", "T013", "--title", "second"]);
        let err = run(&schema, &conn, &m2, Actor::Human.into()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("collision"),
            "error must mention 'collision'; got: {msg}"
        );
        assert!(
            msg.contains("T013"),
            "error must name the colliding id; got: {msg}"
        );
    }

    #[test]
    fn add_with_malformed_display_id_is_rejected() {
        let (schema, conn) = in_memory_schema_and_conn();
        let cmd = build_test_add_cmd_with_display_id(&schema);
        let matches = cmd.get_matches_from(["add", "--display-id", "Tabc", "--title", "bad id"]);

        let err = run(&schema, &conn, &matches, Actor::Human.into()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("format error") || msg.contains("ASCII digits"),
            "error must indicate a format problem; got: {msg}"
        );
    }

    #[test]
    fn add_without_display_id_flag_still_auto_mints() {
        // Regression-trap: the auto-mint path must continue to work unchanged
        // when --display-id is absent (AC5).
        let (schema, conn) = in_memory_schema_and_conn();
        let cmd = build_test_add_cmd_with_display_id(&schema);
        let matches = cmd.get_matches_from(["add", "--title", "auto"]);
        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

        let display_id: String = conn
            .query_row("SELECT display_id FROM tstore WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            display_id, "T001",
            "auto-mint path must still produce T001 when no flag"
        );
    }

    // ---- T013 P2: --lock-contract shorthand on observations add ----
    //
    // Minimal observations-shaped schema with the intent_contract sub-fields
    // that matter for the AC tests: contract_state enum, drafted_at,
    // approved_by/approved_at (actor:human), and the required_when sub-fields
    // (objective/type/in_scope/out_of_scope/acceptance/tier_hint).

    const OBS_LOCK_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
lifecycle:
  states: [open, investigating, confirmed, ready]
  initial_state: open
  transitions:
    - {from: open, to: investigating, verb: investigate, actor: ai_autonomous}
    - {from: investigating, to: confirmed, verb: confirm, actor: ai_with_human}
    - {from: confirmed, to: ready, verb: ratify, actor: framework}
fields:
  - name: summary
    type: text
    required: true
  - name: intent_contract
    type: record
    fields:
      - name: contract_state
        type: enum
        enum_values: [draft, ready]
      - name: drafted_at
        type: timestamp
      - name: objective
        type: text
        required_when: "intent_contract.contract_state == 'ready'"
      - name: type
        type: enum
        enum_values: [work, investigation]
        required_when: "intent_contract.contract_state == 'ready'"
      - name: in_scope
        type:
          list: text
        required_when: "intent_contract.contract_state == 'ready'"
      - name: out_of_scope
        type:
          list: text
        required_when: "intent_contract.contract_state == 'ready'"
      - name: acceptance
        type:
          list: text
        required_when: "intent_contract.contract_state == 'ready'"
      - name: tier_hint
        type: enum
        enum_values: [T1, T2, T3]
        required_when: "intent_contract.contract_state == 'ready'"
      - name: approved_by
        type: text
        actor: human
        required_when: "intent_contract.contract_state == 'ready'"
      - name: approved_at
        type: timestamp
        actor: human
        required_when: "intent_contract.contract_state == 'ready'"
"#;

    fn obs_lock_schema_and_conn() -> (Schema, Connection) {
        let schema = Schema::from_yaml(OBS_LOCK_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    /// Build an `add` command mirroring runtime: leaf args + --lock-contract
    /// (registered for the observations store only).
    fn build_obs_add_cmd(schema: &Schema) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd = clap::Command::new("add");
        for leaf in &leaves {
            let mut arg = clap::Arg::new(leaf.cli_name.clone())
                .long(leaf.cli_name.clone())
                .required(false);
            if matches!(leaf.field.ty, crate::schema::FieldType::List(_)) {
                arg = arg.action(clap::ArgAction::Append);
            }
            cmd = cmd.arg(arg);
        }
        cmd = cmd.arg(
            clap::Arg::new("lock-contract")
                .long("lock-contract")
                .action(clap::ArgAction::SetTrue)
                .required(false),
        );
        cmd
    }

    fn lock_contract_args() -> Vec<&'static str> {
        vec![
            "add",
            "--summary",
            "lock test",
            "--objective",
            "ship the lock-contract shorthand",
            "--type",
            "work",
            "--in-scope",
            "obs",
            "--out-of-scope",
            "tasks",
            "--acceptance",
            "lock works",
            "--tier-hint",
            "T2",
            "--lock-contract",
        ]
    }

    /// AC2.1: --lock-contract + --invoker ai_autonomous → reject with an error
    /// naming both --lock-contract and ai_autonomous.
    #[test]
    fn lock_contract_rejects_ai_autonomous() {
        let (schema, conn) = obs_lock_schema_and_conn();
        let cmd = build_obs_add_cmd(&schema);
        let matches = cmd.get_matches_from(lock_contract_args());

        let err = run(&schema, &conn, &matches, Actor::AiAutonomous.into()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("--lock-contract") && msg.contains("ai_autonomous"),
            "error must name both --lock-contract and ai_autonomous; got: {msg}"
        );
    }

    /// AC2.2 / Done When (4b): --invoker human + --lock-contract with all
    /// required sub-fields → row inserted with contract_state='ready' and
    /// approved_by/at populated.
    #[test]
    fn lock_contract_human_with_required_fields_writes_ready() {
        let (schema, conn) = obs_lock_schema_and_conn();
        let cmd = build_obs_add_cmd(&schema);
        let matches = cmd.get_matches_from(lock_contract_args());

        run(&schema, &conn, &matches, Actor::Human.into())
            .expect("human + --lock-contract with required fields must succeed");

        let raw_ic: String = conn
            .query_row(
                "SELECT intent_contract FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ic: serde_json::Value = serde_json::from_str(&raw_ic).unwrap();
        assert_eq!(ic["contract_state"], "ready");
        assert_eq!(ic["approved_by"], "human");
        assert!(
            ic["approved_at"].as_str().is_some(),
            "approved_at must be populated"
        );
        assert!(
            ic["drafted_at"].as_str().is_some(),
            "drafted_at must be populated"
        );
    }

    /// AC2.3: --invoker ai_with_human --approve-token <T> --lock-contract with
    /// required fields → row written; approved_by='ai_with_human'.
    #[test]
    fn lock_contract_ai_with_human_token_writes_ready() {
        let (schema, conn) = obs_lock_schema_and_conn();
        let cmd = build_obs_add_cmd(&schema);
        let matches = cmd.get_matches_from(lock_contract_args());

        let invoker = InvokerCtx {
            actor: Actor::AiWithHuman,
            token_valid: true,
        };
        run(&schema, &conn, &matches, invoker)
            .expect("ai_with_human + token + --lock-contract must succeed");

        let raw_ic: String = conn
            .query_row(
                "SELECT intent_contract FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ic: serde_json::Value = serde_json::from_str(&raw_ic).unwrap();
        assert_eq!(ic["contract_state"], "ready");
        assert_eq!(ic["approved_by"], "ai_with_human");
    }

    /// AC2.4: --invoker human + --lock-contract WITHOUT required contract
    /// sub-fields → validation fails citing each missing field.
    #[test]
    fn lock_contract_human_without_required_fields_fails_validation() {
        let (schema, conn) = obs_lock_schema_and_conn();
        let cmd = build_obs_add_cmd(&schema);
        let matches = cmd.get_matches_from([
            "add",
            "--summary",
            "missing required fields",
            "--lock-contract",
        ]);

        let err = run(&schema, &conn, &matches, Actor::Human.into()).unwrap_err();
        let msg = err.to_string();
        for required_field in [
            "objective",
            "type",
            "in_scope",
            "out_of_scope",
            "acceptance",
            "tier_hint",
        ] {
            assert!(
                msg.contains(required_field),
                "validation error must cite '{required_field}'; got: {msg}"
            );
        }
    }

    /// AC2.5 / Done When (4d): --invoker ai_autonomous WITHOUT --lock-contract
    /// → row inserted with contract_state='draft' (default behaviour).
    /// We seed contract_state explicitly here because the schema does not
    /// default-fill it; the point of this test is that a non-locked add does
    /// NOT mutate intent_contract.
    #[test]
    fn ai_autonomous_without_lock_writes_draft_unchanged() {
        let (schema, conn) = obs_lock_schema_and_conn();
        let cmd = build_obs_add_cmd(&schema);
        let matches = cmd.get_matches_from([
            "add",
            "--summary",
            "draft path",
            "--contract-state",
            "draft",
        ]);

        run(&schema, &conn, &matches, Actor::AiAutonomous.into())
            .expect("ai_autonomous without --lock-contract must succeed");

        let raw_ic: String = conn
            .query_row(
                "SELECT intent_contract FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ic: serde_json::Value = serde_json::from_str(&raw_ic).unwrap();
        assert_eq!(ic["contract_state"], "draft");
        assert!(ic.get("approved_by").map(|v| v.is_null()).unwrap_or(true));
        assert!(ic.get("approved_at").map(|v| v.is_null()).unwrap_or(true));
    }

    /// Done When (4c): two-step file-now / approve-later flow — human supplies
    /// --intent-contract-approved-by/at WITHOUT --lock-contract → the row is
    /// inserted with those fields populated and contract_state stays draft
    /// (because we did not promote it).
    #[test]
    fn human_without_lock_can_seed_approved_fields_without_promoting_state() {
        let (schema, conn) = obs_lock_schema_and_conn();
        let cmd = build_obs_add_cmd(&schema);
        let matches = cmd.get_matches_from([
            "add",
            "--summary",
            "two step",
            "--contract-state",
            "draft",
            "--approved-by",
            "human",
            "--approved-at",
            "2026-05-03T12:00:00Z",
        ]);

        run(&schema, &conn, &matches, Actor::Human.into())
            .expect("human without --lock-contract may seed approved_by/at on a draft contract");

        let raw_ic: String = conn
            .query_row(
                "SELECT intent_contract FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let ic: serde_json::Value = serde_json::from_str(&raw_ic).unwrap();
        assert_eq!(ic["contract_state"], "draft");
        assert_eq!(ic["approved_by"], "human");
        assert_eq!(ic["approved_at"], "2026-05-03T12:00:00Z");
    }

    // ---- T013 P3: tier_hint inheritance on tasks add ----
    //
    // Schemas: a minimal `observations` store with intent_contract.tier_hint
    // (so we can seed obs rows with tiers) plus a minimal `tasks` store with
    // top-level tier_hint and a list_fk linked_observations -> observations.

    const OBS_TIER_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: summary
    type: text
    required: true
  - name: intent_contract
    type: record
    fields:
      - name: tier_hint
        type: enum
        enum_values: [T1, T2, T3]
"#;

    const TASKS_TIER_SCHEMA: &str = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [planning]
  transitions: []
fields:
  - {name: title, type: text, required: true}
  - {name: linked_observations, type: list_fk, ref: observations}
  - name: tier_hint
    type: enum
    enum_values: [T1, T2, T3]
    required: false
"#;

    /// Open one connection containing BOTH the observations and tasks tables.
    fn tier_inh_schemas_and_conn() -> (Schema, Schema, Connection) {
        let obs = Schema::from_yaml(OBS_TIER_SCHEMA).unwrap();
        let tasks = Schema::from_yaml(TASKS_TIER_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&obs))
            .unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&tasks))
            .unwrap();
        (obs, tasks, conn)
    }

    fn build_tasks_add_cmd(schema: &Schema) -> clap::Command {
        let leaves = crate::schema::flatten::leaf_args(schema).unwrap();
        let mut cmd = clap::Command::new("add");
        for leaf in &leaves {
            let mut arg = clap::Arg::new(leaf.cli_name.clone())
                .long(leaf.cli_name.clone())
                .required(false);
            if matches!(
                leaf.field.ty,
                crate::schema::FieldType::List(_) | crate::schema::FieldType::ListFk { .. }
            ) {
                arg = arg.action(clap::ArgAction::Append);
            }
            cmd = cmd.arg(arg);
        }
        cmd
    }

    /// Seed an observation row with the given tier_hint (or none).
    fn seed_obs(conn: &Connection, obs_schema: &Schema, tier: Option<&str>) -> String {
        let cmd = build_obs_add_cmd(obs_schema);
        let mut argv = vec![
            "add".to_string(),
            "--summary".to_string(),
            "seed".to_string(),
        ];
        if let Some(t) = tier {
            argv.push("--tier-hint".to_string());
            argv.push(t.to_string());
        }
        let matches = cmd.get_matches_from(argv);
        run(obs_schema, conn, &matches, Actor::Human.into()).unwrap();
        // Return the just-minted display_id (count rows).
        conn.query_row(
            "SELECT display_id FROM observations ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    }

    fn read_task_tier(conn: &Connection, display_id: &str) -> Option<String> {
        let raw: rusqlite::types::Value = conn
            .query_row(
                "SELECT tier_hint FROM tasks WHERE display_id = ?1",
                [display_id],
                |r| r.get(0),
            )
            .unwrap();
        match raw {
            rusqlite::types::Value::Text(s) => Some(s),
            _ => None,
        }
    }

    /// AC3.1: two linked obs both T3, no --tier-hint → tasks.tier_hint = 'T3'.
    #[test]
    fn tier_inheritance_unanimous_t3_inherits() {
        let (obs, tasks, conn) = tier_inh_schemas_and_conn();
        let l1 = seed_obs(&conn, &obs, Some("T3"));
        let l2 = seed_obs(&conn, &obs, Some("T3"));

        let cmd = build_tasks_add_cmd(&tasks);
        let matches = cmd.get_matches_from([
            "add",
            "--title",
            "inherits T3",
            "--linked-observations",
            &l1,
            "--linked-observations",
            &l2,
        ]);
        run(&tasks, &conn, &matches, Actor::Human.into())
            .expect("unanimous tier_hint must inherit");

        assert_eq!(read_task_tier(&conn, "T001").as_deref(), Some("T3"));
    }

    /// AC3.2: linked obs disagree (T2 + T3), no --tier-hint → reject naming both ids.
    #[test]
    fn tier_inheritance_disagreement_rejects() {
        let (obs, tasks, conn) = tier_inh_schemas_and_conn();
        let l1 = seed_obs(&conn, &obs, Some("T2"));
        let l2 = seed_obs(&conn, &obs, Some("T3"));

        let cmd = build_tasks_add_cmd(&tasks);
        let matches = cmd.get_matches_from([
            "add",
            "--title",
            "should fail",
            "--linked-observations",
            &l1,
            "--linked-observations",
            &l2,
        ]);
        let err = run(&tasks, &conn, &matches, Actor::Human.into()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&l1), "error must name '{l1}': {msg}");
        assert!(msg.contains(&l2), "error must name '{l2}': {msg}");
        assert!(
            msg.contains("T2") && msg.contains("T3"),
            "error must show both tiers: {msg}"
        );
        assert!(
            msg.contains("--tier-hint"),
            "error must instruct passing --tier-hint: {msg}"
        );
    }

    /// AC3.3: same disagreement WITH --tier-hint T3 → succeeds, tier_hint='T3'.
    #[test]
    fn tier_inheritance_explicit_flag_overrides_disagreement() {
        let (obs, tasks, conn) = tier_inh_schemas_and_conn();
        let l1 = seed_obs(&conn, &obs, Some("T2"));
        let l2 = seed_obs(&conn, &obs, Some("T3"));

        let cmd = build_tasks_add_cmd(&tasks);
        let matches = cmd.get_matches_from([
            "add",
            "--title",
            "explicit wins",
            "--linked-observations",
            &l1,
            "--linked-observations",
            &l2,
            "--tier-hint",
            "T3",
        ]);
        run(&tasks, &conn, &matches, Actor::Human.into())
            .expect("explicit --tier-hint must override disagreement");
        assert_eq!(read_task_tier(&conn, "T001").as_deref(), Some("T3"));
    }

    /// AC3.4: no linked obs and no --tier-hint → succeeds with tier_hint NULL.
    #[test]
    fn tier_inheritance_no_linked_no_flag_succeeds_null() {
        let (_obs, tasks, conn) = tier_inh_schemas_and_conn();

        let cmd = build_tasks_add_cmd(&tasks);
        let matches = cmd.get_matches_from(["add", "--title", "no obs no flag"]);
        run(&tasks, &conn, &matches, Actor::Human.into())
            .expect("no linked obs and no flag must succeed");
        assert_eq!(read_task_tier(&conn, "T001"), None);
    }

    /// AC3.5: unknown linked obs id → succeeds with stderr warning, tier_hint absent.
    #[test]
    fn tier_inheritance_unknown_linked_obs_warns_and_succeeds() {
        let (_obs, tasks, conn) = tier_inh_schemas_and_conn();

        let cmd = build_tasks_add_cmd(&tasks);
        let matches = cmd.get_matches_from([
            "add",
            "--title",
            "missing link",
            "--linked-observations",
            "L999",
        ]);
        // Warning goes to stderr; we cannot capture it from the test process
        // without redirection plumbing, but the call must succeed and tier_hint
        // must remain NULL (no inference attempted from a missing row).
        run(&tasks, &conn, &matches, Actor::Human.into())
            .expect("missing linked obs must produce a warning, not a hard fail");
        assert_eq!(read_task_tier(&conn, "T001"), None);
    }

    /// Belt-and-braces: explicit --tier-hint wins even when linked obs unanimously
    /// agree on a different tier (i.e. the inference branch is properly skipped).
    #[test]
    fn tier_inheritance_explicit_flag_overrides_agreement() {
        let (obs, tasks, conn) = tier_inh_schemas_and_conn();
        let l1 = seed_obs(&conn, &obs, Some("T2"));
        let l2 = seed_obs(&conn, &obs, Some("T2"));

        let cmd = build_tasks_add_cmd(&tasks);
        let matches = cmd.get_matches_from([
            "add",
            "--title",
            "explicit beats agreement",
            "--linked-observations",
            &l1,
            "--linked-observations",
            &l2,
            "--tier-hint",
            "T3",
        ]);
        run(&tasks, &conn, &matches, Actor::Human.into()).unwrap();
        assert_eq!(read_task_tier(&conn, "T001").as_deref(), Some("T3"));
    }

    /// Linked obs that exist but have no tier_hint → not unanimous → reject with
    /// "(no tier)" listed for those rows.
    #[test]
    fn tier_inheritance_present_obs_without_tier_rejects() {
        let (obs, tasks, conn) = tier_inh_schemas_and_conn();
        let l1 = seed_obs(&conn, &obs, Some("T2"));
        let l2 = seed_obs(&conn, &obs, None);

        let cmd = build_tasks_add_cmd(&tasks);
        let matches = cmd.get_matches_from([
            "add",
            "--title",
            "mixed null",
            "--linked-observations",
            &l1,
            "--linked-observations",
            &l2,
        ]);
        let err = run(&tasks, &conn, &matches, Actor::Human.into()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains(&l2), "error must name '{l2}': {msg}");
        assert!(
            msg.contains("no tier"),
            "error must mention '(no tier)': {msg}"
        );
    }

    // ---- T020 P2: synthetic 'create' transition_history row + lock-contract walk ----

    /// AC2.2: tasks add inserts exactly one transition_history row with
    /// from_status='' (empty string, not NULL), to_status='planning',
    /// verb='create', store='tasks'.
    #[test]
    fn tasks_add_emits_planning_arrival() {
        const TASKS_MINI: &str = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [planning, complete]
  initial_state: planning
  transitions: []
fields:
  - {name: title, type: text, required: true}
"#;
        let schema = Schema::from_yaml(TASKS_MINI).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();

        let cmd = build_test_add_cmd(&schema);
        let matches = cmd.get_matches_from(["add", "--title", "first task"]);
        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

        // Exactly one row, with the synthetic create shape.
        let (store, display_id, from, to, verb, invoker): (
            String,
            String,
            String,
            String,
            String,
            String,
        ) = conn
            .query_row(
                "SELECT store, display_id, from_status, to_status, verb, invoker \
                 FROM transition_history WHERE store='tasks'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .expect("exactly one transition_history row for tasks add");
        assert_eq!(store, "tasks");
        assert_eq!(display_id, "T001");
        assert_eq!(from, "", "from_status must be empty string, not NULL");
        assert_eq!(to, "planning");
        assert_eq!(verb, "create");
        assert_eq!(invoker, "human");

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM transition_history WHERE store='tasks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "tasks add must emit exactly one create row");
    }

    /// AC2.2 (b): observations add (no --lock-contract) inserts exactly one
    /// synthetic create row with from_status='' to_status='open'.
    #[test]
    fn observations_add_no_lock_emits_open_arrival() {
        let (schema, conn) = obs_lock_schema_and_conn();
        let cmd = build_obs_add_cmd(&schema);
        let matches = cmd.get_matches_from(["add", "--summary", "no lock"]);
        run(&schema, &conn, &matches, Actor::AiAutonomous.into()).unwrap();

        let rows: Vec<(String, String, String, String)> = conn
            .prepare(
                "SELECT from_status, to_status, verb, invoker FROM transition_history \
                 WHERE store='observations' AND display_id='L001' ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows.len(), 1, "expected one create row; got {rows:?}");
        assert_eq!(rows[0].0, "", "from_status must be empty string");
        assert_eq!(rows[0].1, "open");
        assert_eq!(rows[0].2, "create");
        assert_eq!(rows[0].3, "ai_autonomous");
    }

    /// AC2.3: observations add --lock-contract --invoker human inserts the
    /// synthetic create row PLUS confirmed→ready transition (verb=ratify,
    /// invoker=framework). Final row status='ready'.
    #[test]
    fn lock_contract_lands_at_ready() {
        let (schema, conn) = obs_lock_schema_and_conn();
        let cmd = build_obs_add_cmd(&schema);
        let matches = cmd.get_matches_from(lock_contract_args());

        run(&schema, &conn, &matches, Actor::Human.into()).expect("lock-contract must succeed");

        // Final row status is 'ready'.
        let status: String = conn
            .query_row(
                "SELECT status FROM observations WHERE display_id='L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "ready", "row must land at 'ready' after lock-contract");

        // transition_history contains the create row + the synthetic walk + ratify.
        let rows: Vec<(String, String, String, String)> = conn
            .prepare(
                "SELECT from_status, to_status, verb, invoker FROM transition_history \
                 WHERE store='observations' AND display_id='L001' ORDER BY id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        // create + investigate + confirm + ratify = 4 rows
        assert_eq!(rows.len(), 4, "expected 4 transition_history rows; got {rows:?}");
        assert_eq!(rows[0], ("".to_string(), "open".to_string(), "create".to_string(), "human".to_string()));
        assert_eq!(rows[1], ("open".to_string(), "investigating".to_string(), "investigate".to_string(), "framework".to_string()));
        assert_eq!(rows[2], ("investigating".to_string(), "confirmed".to_string(), "confirm".to_string(), "human".to_string()));
        assert_eq!(rows[3], ("confirmed".to_string(), "ready".to_string(), "ratify".to_string(), "framework".to_string()));
    }

    // T027 P2 (Task 2.3 / AC2.4): a T1 row scaffolded via add is observable as
    // status=ready immediately afterward, because fire_on_entry_follow_ons fires
    // on the initial state and the on_state.planning TransitionTo with
    // when=T1-true fires.
    #[test]
    fn add_t1_row_lands_at_ready_via_initial_on_entry_follow_on() {
        let yaml = r#"
name: wf_add_when
id_format: "W{:03d}"
lifecycle:
  states: [planning, ready, executing, done]
  transitions:
    - from: planning
      to: ready
      verb: skip-plan
      actor: framework
fields:
  - name: title
    type: text
    required: true
  - name: tier_hint
    type: text
  - name: current_phase
    type: integer
    actor: framework
  - name: current_cycle
    type: integer
    actor: framework
workflow:
  agent_roles:
    planner:
      description: "plans"
  briefing_templates:
    planner: templates/planner-brief.md.tpl
  on_state:
    planning:
      - dispatch_agent: planner
        when: "tier_hint != 'T1'"
      - transition_to: ready
        when: "tier_hint == 'T1'"
  submit_targets: {}
  max_revise_cycles: 3
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();

        let matches = build_test_add_cmd(&schema)
            .get_matches_from(["add", "--title", "t1 task", "--tier-hint", "T1"]);
        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

        let status: String = conn
            .query_row(
                "SELECT status FROM wf_add_when WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status, "ready",
            "T1 row added via run must land at status=ready (initial on-entry follow-on fired)"
        );

        // Inverse: a T3 row stays at planning.
        let matches3 = build_test_add_cmd(&schema)
            .get_matches_from(["add", "--title", "t3 task", "--tier-hint", "T3"]);
        run(&schema, &conn, &matches3, Actor::Human.into()).unwrap();
        let status3: String = conn
            .query_row(
                "SELECT status FROM wf_add_when WHERE id = 2",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            status3, "planning",
            "T3 row stays at planning (no when=T1-true follow-on fires)"
        );
    }

    // T117: --acceptance-from-file on observations add

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

    fn build_add_cmd_with_acceptance_from_file(schema: &Schema) -> clap::Command {
        let mut cmd = build_test_add_cmd(schema);
        cmd = cmd.arg(
            clap::Arg::new("acceptance-from-file")
                .long("acceptance-from-file")
                .required(false),
        );
        cmd
    }

    #[test]
    fn t117_add_acceptance_from_file_writes_lines_as_list() {
        let schema = Schema::from_yaml(OBS_ACCEPTANCE_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&crate::codegen::ddl::ddl_for(&schema))
            .unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "ac one\nac two\nac three\n").unwrap();

        let cmd = build_add_cmd_with_acceptance_from_file(&schema);
        let matches = cmd.get_matches_from([
            "add",
            "--summary",
            "test row",
            "--acceptance-from-file",
            tmp.path().to_str().unwrap(),
        ]);
        run(&schema, &conn, &matches, Actor::Human.into()).unwrap();

        let raw: String = conn
            .query_row(
                "SELECT acceptance FROM observations WHERE display_id = 'L001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let stored: Vec<String> = serde_json::from_str(&raw).unwrap();
        assert_eq!(stored, vec!["ac one", "ac two", "ac three"]);
    }
}
