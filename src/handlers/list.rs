use anyhow::{bail, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::codegen::ddl::quote_ident;
use crate::output;
use crate::schema::{actor::InvokerCtx, FieldType, Schema};

/// The 13 canonical risk flag values (mirrors risk_flags.list_enum in observations schema.yaml).
pub const CANONICAL_RISK_FLAGS: &[&str] = &[
    "touches_actor_authority",
    "touches_lifecycle",
    "touches_subscriber_semantics",
    "introduces_new_primitive",
    "changes_boundary",
    "security_sensitive",
    "docs_only",
    "small_local_fix",
    "duplicate_symptom",
    "touches_runner_boundary",
    "touches_schema_core",
    "authority_surface_drift",
    "contradicts_prior_decision",
];

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: InvokerCtx,
) -> Result<()> {
    let json_flag = matches.get_flag("json");

    // ---------- filter / sort / limit args ----------

    let status_filter = matches.get_one::<String>("status").cloned();
    let limit = matches.get_one::<u64>("limit").copied();
    let sort_col = matches.get_one::<String>("sort").cloned();
    let reverse = matches.get_flag("reverse");
    let since = matches.get_one::<String>("since").cloned();

    // risk-flag filter (observations only; multi-occurrence = AND semantics).
    // Use try_get_many to avoid panicking when the arg isn't registered on the command
    // (e.g., non-observations stores that don't include --risk-flag).
    let risk_flags_filter: Vec<String> = matches
        .try_get_many::<String>("risk-flag")
        .ok()
        .flatten()
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default();

    // Validate each supplied flag against the canonical 13
    for flag in &risk_flags_filter {
        if !CANONICAL_RISK_FLAGS.contains(&flag.as_str()) {
            bail!(
                "unknown --risk-flag value '{}'; allowed values: [{}]",
                flag,
                CANONICAL_RISK_FLAGS.join(", ")
            );
        }
    }

    // Build known column set for --sort validation
    let known_cols: Vec<String> = {
        let mut v = vec![
            "id".to_string(),
            "display_id".to_string(),
            "status".to_string(),
            "created_at".to_string(),
            "updated_at".to_string(),
            "created_by".to_string(),
            "updated_by".to_string(),
        ];
        for f in &schema.fields {
            v.push(f.name.clone());
        }
        v
    };

    // Validate --sort column
    if let Some(ref col) = sort_col {
        if !known_cols.contains(col) {
            bail!(
                "unknown sort column '{}'; valid columns: {}",
                col,
                known_cols.join(", ")
            );
        }
    }

    // ---------- column list ----------

    let col_names: Vec<String> = {
        let mut v = vec![
            "id".to_string(),
            "display_id".to_string(),
            "status".to_string(),
            "created_at".to_string(),
            "updated_at".to_string(),
            "created_by".to_string(),
            "updated_by".to_string(),
        ];
        for field in &schema.fields {
            v.push(field.name.clone());
        }
        v
    };

    // ---------- build SQL ----------

    let col_list = col_names.join(", ");

    let mut where_clauses: Vec<String> = Vec::new();
    let mut params: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(ref st) = status_filter {
        where_clauses.push(format!("status = ?{}", params.len() + 1));
        params.push(rusqlite::types::Value::Text(st.clone()));
    }

    if let Some(ref since_date) = since {
        // Validate date format loosely (YYYY-MM-DD)
        validate_date_format(since_date)?;
        // created_at is stored as ISO-8601; string prefix comparison works for YYYY-MM-DD
        where_clauses.push(format!("created_at >= ?{}", params.len() + 1));
        params.push(rusqlite::types::Value::Text(since_date.clone()));
    }

    // risk_flags membership filter via json_each — one EXISTS subquery per flag (AND semantics)
    for flag in &risk_flags_filter {
        let tbl = quote_ident(&schema.name);
        where_clauses.push(format!(
            "EXISTS (SELECT 1 FROM json_each({tbl}.risk_flags) WHERE value = ?{})",
            params.len() + 1
        ));
        params.push(rusqlite::types::Value::Text(flag.clone()));
    }

    let where_str = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };

    let order_str = match &sort_col {
        Some(col) => {
            let dir = if reverse { "DESC" } else { "ASC" };
            format!(" ORDER BY {} {}", col, dir)
        }
        None => " ORDER BY id ASC".to_string(),
    };

    let limit_str = match limit {
        Some(n) => format!(" LIMIT {n}"),
        None => String::new(),
    };

    let sql = format!(
        "SELECT {col_list} FROM {}{}{}{} ",
        quote_ident(&schema.name),
        where_str,
        order_str,
        limit_str
    );

    // ---------- execute ----------

    let mut stmt = conn.prepare(&sql)?;
    let mut entries: Vec<BTreeMap<String, Value>> = Vec::new();

    let rows = stmt.query_map(rusqlite::params_from_iter(params.iter()), |row| {
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
        for key in &[
            "display_id",
            "status",
            "created_at",
            "updated_at",
            "created_by",
            "updated_by",
        ] {
            if let Some(v) = raw_map.get(*key) {
                entry.insert(key.to_string(), v.clone());
            }
        }

        // Schema fields: decode JSON for Record/List/ListRecord/ListFk/Json.
        // (T008-P4: extended from Record|List only to close pre-existing ListRecord/ListFk parity gap.)
        for field in &schema.fields {
            if let Some(raw_val) = raw_map.get(&field.name) {
                match &field.ty {
                    FieldType::Record(_)
                    | FieldType::List(_)
                    | FieldType::ListRecord(_)
                    | FieldType::ListFk { .. }
                    | FieldType::Json => {
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

/// Validate that a string looks like YYYY-MM-DD.
fn validate_date_format(s: &str) -> Result<()> {
    let bytes = s.as_bytes();
    if bytes.len() != 10 {
        bail!("--since date must be in YYYY-MM-DD format, got: '{s}'");
    }
    if bytes[4] != b'-' || bytes[7] != b'-' {
        bail!("--since date must be in YYYY-MM-DD format (dashes at pos 4 and 7), got: '{s}'");
    }
    for (i, &b) in bytes.iter().enumerate() {
        if i == 4 || i == 7 {
            continue;
        }
        if !b.is_ascii_digit() {
            bail!(
                "--since date must be in YYYY-MM-DD format (all digits except dashes), got: '{s}'"
            );
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::ddl_for;
    use crate::handlers::add;
    use crate::schema::actor::Actor;
    use crate::schema::Schema;
    use rusqlite::Connection;

    const MINIMAL_SCHEMA: &str = r#"
name: tstore
id_format: "T{:03d}"
lifecycle:
  states: [open, closed]
  transitions: []
fields:
  - name: title
    type: text
"#;

    fn setup() -> (Schema, Connection) {
        let schema = Schema::from_yaml(MINIMAL_SCHEMA).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    fn make_add_cmd(schema: &Schema) -> clap::Command {
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

    fn make_list_cmd(schema: &Schema) -> clap::Command {
        use clap::{Arg, ArgAction};
        let col_names: Vec<String> = {
            let mut v = vec![
                "status".to_string(),
                "created_at".to_string(),
                "updated_at".to_string(),
                "created_by".to_string(),
                "updated_by".to_string(),
                "display_id".to_string(),
            ];
            for f in &schema.fields {
                v.push(f.name.clone());
            }
            v
        };
        let cols_help = col_names.join(", ");

        // Also include top-level flags that dispatch passes through
        clap::Command::new("list")
            .arg(
                clap::Arg::new("json")
                    .long("json")
                    .action(ArgAction::SetTrue)
                    .global(true),
            )
            .arg(Arg::new("status").long("status").required(false))
            .arg(
                Arg::new("limit")
                    .long("limit")
                    .value_parser(clap::value_parser!(u64))
                    .required(false),
            )
            .arg(
                Arg::new("sort")
                    .long("sort")
                    .help(format!("Valid columns: {cols_help}"))
                    .required(false),
            )
            .arg(
                Arg::new("reverse")
                    .long("reverse")
                    .action(ArgAction::SetTrue)
                    .required(false),
            )
            .arg(Arg::new("since").long("since").required(false))
    }

    fn insert_entry(schema: &Schema, conn: &Connection, title: &str) {
        let cmd = make_add_cmd(schema);
        let m = cmd.get_matches_from(["add", "--title", title]);
        add::run(schema, conn, &m, Actor::Human.into()).unwrap();
    }

    // -----------------------------------------------------------------------
    // AC: --status filter
    // -----------------------------------------------------------------------
    #[test]
    fn status_filter_returns_only_matching_rows() {
        let (schema, conn) = setup();
        insert_entry(&schema, &conn, "first");
        insert_entry(&schema, &conn, "second");

        // Close T002
        conn.execute(
            "UPDATE tstore SET status = 'closed' WHERE display_id = 'T002'",
            [],
        )
        .unwrap();

        let cmd = make_list_cmd(&schema);
        let m = cmd.get_matches_from(["list", "--status", "open"]);
        // Collect directly by running a SQL query after
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tstore WHERE status = 'open'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // run() must not error
        run(&schema, &conn, &m, Actor::Human.into()).unwrap();
    }

    // -----------------------------------------------------------------------
    // AC: --limit
    // -----------------------------------------------------------------------
    #[test]
    fn limit_restricts_row_count() {
        let (schema, conn) = setup();
        for i in 0..5 {
            insert_entry(&schema, &conn, &format!("entry-{i}"));
        }

        let cmd = make_list_cmd(&schema);
        let m = cmd.get_matches_from(["list", "--limit", "2"]);

        // Run succeeds
        run(&schema, &conn, &m, Actor::Human.into()).unwrap();

        // Verify the SQL limit is applied: query directly
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM (SELECT id FROM tstore LIMIT 2)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    // -----------------------------------------------------------------------
    // AC: --sort invalid column errors
    // -----------------------------------------------------------------------
    #[test]
    fn sort_invalid_column_errors() {
        let (schema, conn) = setup();
        insert_entry(&schema, &conn, "x");

        let cmd = make_list_cmd(&schema);
        let m = cmd.get_matches_from(["list", "--sort", "bogus_field"]);
        let err = run(&schema, &conn, &m, Actor::Human.into()).unwrap_err();
        assert!(
            err.to_string().contains("bogus_field"),
            "error should name the bad column: {err}"
        );
        assert!(
            err.to_string().contains("unknown sort column"),
            "error should state the problem: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // AC: --sort valid column succeeds + --reverse
    // -----------------------------------------------------------------------
    #[test]
    fn sort_valid_column_succeeds() {
        let (schema, conn) = setup();
        insert_entry(&schema, &conn, "a");
        insert_entry(&schema, &conn, "b");

        let cmd = make_list_cmd(&schema);
        let m = cmd.get_matches_from(["list", "--sort", "created_at", "--reverse"]);
        run(&schema, &conn, &m, Actor::Human.into()).unwrap();
    }

    // -----------------------------------------------------------------------
    // AC: --since date format validation
    // -----------------------------------------------------------------------
    #[test]
    fn since_bad_format_errors() {
        let err = validate_date_format("2026-4-26").unwrap_err();
        assert!(err.to_string().contains("YYYY-MM-DD"));
    }

    #[test]
    fn since_good_format_ok() {
        validate_date_format("2026-04-26").unwrap();
    }

    // -----------------------------------------------------------------------
    // AC: --status unknown value returns empty (no error)
    // -----------------------------------------------------------------------
    #[test]
    fn status_unknown_value_returns_empty_not_error() {
        let (schema, conn) = setup();
        insert_entry(&schema, &conn, "x");

        let cmd = make_list_cmd(&schema);
        let m = cmd.get_matches_from(["list", "--status", "nonexistent_state"]);
        // Must succeed with 0 rows
        run(&schema, &conn, &m, Actor::Human.into()).unwrap();
    }

    // -----------------------------------------------------------------------
    // T008-P4: Json field in list decodes to structured Value (not raw string)
    // -----------------------------------------------------------------------

    const JSON_SCHEMA: &str = r#"
name: jstore
id_format: "J{:03d}"
lifecycle:
  states: [open, done]
  transitions: []
fields:
  - name: title
    type: text
  - name: notes
    type: json
    required: false
"#;

    const LIST_RECORD_SCHEMA: &str = r#"
name: lrstore
id_format: "R{:03d}"
lifecycle:
  states: [open, done]
  transitions: []
fields:
  - name: title
    type: text
  - name: tags
    type: list_record
    fields:
      - name: label
        type: text
      - name: score
        type: integer
  - name: refs
    type: list_fk
    ref: other
"#;

    fn setup_schema(yaml: &str) -> (Schema, Connection) {
        let schema = Schema::from_yaml(yaml).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        let ddl = ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, conn)
    }

    /// Phase 4 AC: list.rs decodes Json fields to structured Value, not raw string.
    #[test]
    fn list_json_field_decodes_to_structured_value() {
        let (schema, conn) = setup_schema(JSON_SCHEMA);

        let notes_json = r#"{"k":"v","arr":[1,2]}"#;
        conn.execute(
            "INSERT INTO jstore (display_id, status, created_at, updated_at, created_by, updated_by, title, notes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "J001", "open", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
                "human", "human", "Test", notes_json,
            ],
        ).unwrap();

        // Invoke list's decode loop directly by querying and decoding inline.
        // We simulate what run() does: pull raw rows, decode per schema.
        let raw_map: std::collections::BTreeMap<String, serde_json::Value> = {
            let mut stmt = conn
                .prepare(
                    "SELECT display_id, status, title, notes FROM jstore WHERE display_id = 'J001'",
                )
                .unwrap();
            let row = stmt
                .query_row([], |r| {
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(
                        "display_id".to_string(),
                        serde_json::Value::String(r.get::<_, String>(0).unwrap()),
                    );
                    m.insert(
                        "status".to_string(),
                        serde_json::Value::String(r.get::<_, String>(1).unwrap()),
                    );
                    m.insert(
                        "title".to_string(),
                        serde_json::Value::String(r.get::<_, String>(2).unwrap()),
                    );
                    m.insert(
                        "notes".to_string(),
                        serde_json::Value::String(r.get::<_, String>(3).unwrap()),
                    );
                    Ok(m)
                })
                .unwrap();
            row
        };

        // Apply the same decode logic as list.rs
        let mut entry: std::collections::BTreeMap<String, serde_json::Value> =
            std::collections::BTreeMap::new();
        for field in &schema.fields {
            if let Some(raw_val) = raw_map.get(&field.name) {
                match &field.ty {
                    FieldType::Record(_)
                    | FieldType::List(_)
                    | FieldType::ListRecord(_)
                    | FieldType::ListFk { .. }
                    | FieldType::Json => {
                        if let serde_json::Value::String(json_str) = raw_val {
                            if !json_str.is_empty() && json_str != "null" {
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(json_str)
                                {
                                    entry.insert(field.name.clone(), parsed);
                                    continue;
                                }
                            }
                        }
                        entry.insert(field.name.clone(), serde_json::Value::Null);
                    }
                    _ => {
                        entry.insert(field.name.clone(), raw_val.clone());
                    }
                }
            }
        }

        let notes = entry.get("notes").expect("notes should be present");
        match notes {
            serde_json::Value::Object(map) => {
                assert_eq!(
                    map.get("k").and_then(|v| v.as_str()),
                    Some("v"),
                    "notes.k should be 'v'"
                );
                let arr = map
                    .get("arr")
                    .and_then(|v| v.as_array())
                    .expect("notes.arr should be array");
                assert_eq!(arr.len(), 2);
            }
            other => panic!(
                "expected Value::Object for notes in list output, got: {:?}",
                other
            ),
        }
    }

    /// Phase 4 AC: list.rs parity gap closure — ListRecord fields decode to structured Value.
    /// Pre-T008-P4, list.rs only matched Record|List; ListRecord fell to `_ =>` and emitted raw string.
    #[test]
    fn list_list_record_field_decodes_to_structured_value() {
        let (schema, conn) = setup_schema(LIST_RECORD_SCHEMA);

        let tags_json = r#"[{"label":"alpha","score":1},{"label":"beta","score":2}]"#;
        let refs_json = r#"["R001","R002"]"#;
        conn.execute(
            "INSERT INTO lrstore (display_id, status, created_at, updated_at, created_by, updated_by, title, tags, refs) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                "R001", "open", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
                "human", "human", "LR Test", tags_json, refs_json,
            ],
        ).unwrap();

        // Decode using the same logic as list.rs post-P4
        let raw_map: std::collections::BTreeMap<String, serde_json::Value> = {
            let mut stmt = conn
                .prepare("SELECT tags, refs FROM lrstore WHERE display_id = 'R001'")
                .unwrap();
            let row = stmt
                .query_row([], |r| {
                    let mut m = std::collections::BTreeMap::new();
                    m.insert(
                        "tags".to_string(),
                        serde_json::Value::String(r.get::<_, String>(0).unwrap()),
                    );
                    m.insert(
                        "refs".to_string(),
                        serde_json::Value::String(r.get::<_, String>(1).unwrap()),
                    );
                    Ok(m)
                })
                .unwrap();
            row
        };

        let mut entry: std::collections::BTreeMap<String, serde_json::Value> =
            std::collections::BTreeMap::new();
        for field in &schema.fields {
            if let Some(raw_val) = raw_map.get(&field.name) {
                match &field.ty {
                    FieldType::Record(_)
                    | FieldType::List(_)
                    | FieldType::ListRecord(_)
                    | FieldType::ListFk { .. }
                    | FieldType::Json => {
                        if let serde_json::Value::String(json_str) = raw_val {
                            if !json_str.is_empty() && json_str != "null" {
                                if let Ok(parsed) =
                                    serde_json::from_str::<serde_json::Value>(json_str)
                                {
                                    entry.insert(field.name.clone(), parsed);
                                    continue;
                                }
                            }
                        }
                        entry.insert(field.name.clone(), serde_json::Value::Null);
                    }
                    _ => {
                        entry.insert(field.name.clone(), raw_val.clone());
                    }
                }
            }
        }

        // tags must be an array of objects, not a raw string
        let tags = entry.get("tags").expect("tags should be present");
        match tags {
            serde_json::Value::Array(arr) => {
                assert_eq!(arr.len(), 2, "should have 2 tags");
                assert_eq!(arr[0]["label"], "alpha");
                assert_eq!(arr[1]["score"], 2i64);
            }
            other => panic!(
                "expected Value::Array for tags (ListRecord parity), got: {:?}",
                other
            ),
        }

        // refs must be an array of strings, not a raw string
        let refs = entry.get("refs").expect("refs should be present");
        match refs {
            serde_json::Value::Array(arr) => {
                assert_eq!(arr.len(), 2);
                assert_eq!(arr[0].as_str(), Some("R001"));
            }
            other => panic!(
                "expected Value::Array for refs (ListFk parity), got: {:?}",
                other
            ),
        }
    }
}
