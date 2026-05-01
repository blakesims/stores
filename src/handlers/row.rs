use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::Value;

use crate::codegen::ddl::quote_ident;
use crate::schema::{FieldType, Schema};
use crate::validate::EntryMap;

/// Build a nested EntryMap from flat CLI args.
///
/// `get_args` returns the raw string values for a given cli_name (e.g. "linked-observations"),
/// or None if the arg was not provided.  Most fields receive a single value; list-typed
/// fields receive one entry per repeated `--<flag>` occurrence.
///
/// For `ListFk`/`ListRecord` fields, `build_entry_map` accepts three input shapes:
/// (1) a single JSON array (back-compat with the v0.1 single-arg convention);
/// (2) a single bare value (auto-promoted to a 1-element array — `L001` → `["L001"]`
///     for ListFk; a single JSON object → `[{...}]` for ListRecord);
/// (3) repeated `--<flag>` occurrences (each becomes one element).
pub fn build_entry_map<F>(schema: &Schema, get_args: F) -> Result<EntryMap>
where
    F: Fn(&str) -> Option<Vec<String>>,
{
    use crate::schema::flatten::leaf_args;
    let leaves = leaf_args(schema)?;

    let mut entry: EntryMap = BTreeMap::new();

    for leaf in &leaves {
        let raws = match get_args(&leaf.cli_name) {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };

        let value = assemble_field_value(&leaf.field.ty, &raws);

        // Insert value at the correct depth.
        insert_at_path(&mut entry, &leaf.path, value);
    }

    Ok(entry)
}

/// Combine one or more raw CLI inputs into the JSON value for a field.
///
/// Scalars and `Record`/`Json`/`List` fields delegate to `coerce_value` after
/// joining (List uses pipe-join for back-compat).  `ListFk` and `ListRecord`
/// support repeated `--<flag>` and bare-value auto-promote per the
/// `build_entry_map` doc.
fn assemble_field_value(ty: &FieldType, raws: &[String]) -> Value {
    match ty {
        FieldType::List(_) => {
            // Pipe-join all inputs; coerce_value splits on '|' (preserves `--foo "X|Y"`
            // and `--foo X --foo Y` equivalence).
            let joined = raws.join("|");
            coerce_value(ty, &joined)
        }
        FieldType::ListFk { .. } => {
            if raws.len() == 1 {
                let raw = &raws[0];
                match serde_json::from_str::<Value>(raw) {
                    Ok(Value::Array(arr)) => Value::Array(arr),
                    // Bare display_id (or any non-JSON-array): auto-promote to single-element array.
                    _ => Value::Array(vec![Value::String(raw.clone())]),
                }
            } else {
                Value::Array(raws.iter().cloned().map(Value::String).collect())
            }
        }
        FieldType::ListRecord(_) => {
            if raws.len() == 1 {
                let raw = &raws[0];
                match serde_json::from_str::<Value>(raw) {
                    Ok(Value::Array(arr)) => Value::Array(arr),
                    // Single JSON object → wrap as 1-element array.
                    Ok(v @ Value::Object(_)) => Value::Array(vec![v]),
                    // Anything else (bad JSON, scalar, etc.): sentinel for the validator.
                    _ => Value::String(raw.clone()),
                }
            } else {
                // Multi-arg: each must parse as JSON. On any parse failure, sentinel the
                // whole field (matches T006 REVISE-1 bad-JSON UX — surface via validator).
                let mut parsed: Vec<Value> = Vec::with_capacity(raws.len());
                for r in raws {
                    match serde_json::from_str::<Value>(r) {
                        Ok(v) => parsed.push(v),
                        Err(_) => return Value::String(r.clone()),
                    }
                }
                Value::Array(parsed)
            }
        }
        // Scalars, Record, Json: take the first input and run through coerce_value.
        // (Repeated flags on scalar fields are not blocked by clap but we use the first.)
        _ => coerce_value(ty, &raws[0]),
    }
}

/// Insert `value` into `entry` following `path` (any depth ≥ 1).
///
/// Creates intermediate `Value::Object` nodes as needed.  Silently ignores
/// writes into non-object intermediates (schema validation catches those
/// separately).
pub fn insert_at_path(entry: &mut EntryMap, path: &[String], value: Value) {
    match path.len() {
        0 => {} // nothing to do
        1 => {
            entry.insert(path[0].clone(), value);
        }
        _ => {
            // Ensure the intermediate node is an Object
            let parent = entry
                .entry(path[0].clone())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(ref mut map) = parent {
                insert_at_path_value(map, &path[1..], value);
            }
            // If the intermediate node is not an Object (type mismatch), silently
            // skip — schema validation will catch the misshapen entry.
        }
    }
}

fn insert_at_path_value(map: &mut serde_json::Map<String, Value>, path: &[String], value: Value) {
    match path.len() {
        0 => {}
        1 => {
            map.insert(path[0].clone(), value);
        }
        _ => {
            let parent = map
                .entry(path[0].clone())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(ref mut child_map) = parent {
                insert_at_path_value(child_map, &path[1..], value);
            }
        }
    }
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
        // ListRecord and ListFk: the raw CLI string is expected to be a JSON array.
        // On success return the parsed array.  On parse failure OR non-array shape,
        // return Value::String(raw) as a sentinel so the validator's type-shape check
        // fires for both required AND optional fields (Value::Null is a valid nullable
        // value; Value::String is never valid for a list_record/list_fk column, so the
        // type-mismatch error fires unconditionally).
        FieldType::ListRecord(_) | FieldType::ListFk { .. } => {
            match serde_json::from_str::<Value>(raw) {
                Ok(Value::Array(arr)) => Value::Array(arr),
                _ => Value::String(raw.to_string()),
            }
        }
        // Json: any valid JSON shape (object/array/scalar/null) is accepted.
        // On parse failure, return Value::String(raw) as a sentinel so the validator's
        // type-shape check can fire (Phase 3).  Decision 2: top-level JSON strings
        // ('"hello"') parse to Value::String and are false-flagged as sentinels —
        // documented limitation; users should wrap in object or use Text field type.
        FieldType::Json => match serde_json::from_str::<Value>(raw) {
            Ok(v) => v,
            Err(_) => Value::String(raw.to_string()),
        },
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
///
/// All JSON TEXT columns (Record, List, ListRecord, ListFk) are deserialized
/// back into their native `serde_json::Value` shapes.  Depth ≥ 3 nesting
/// (e.g. `cycles[0].executor.summary`) is preserved verbatim — the JSON
/// round-trip is identity for arbitrary nesting.
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
        table = quote_ident(&schema.name)
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

    // Reconstruct nested EntryMap: for JSON TEXT columns, parse the stored JSON.
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
                // All JSON TEXT column types — deserialize as opaque JSON.
                // For Record: Value::Object (arbitrary depth).
                // For List: Value::Array of scalars.
                // For ListRecord: Value::Array of Objects (arbitrary depth per element).
                // For ListFk: Value::Array of display_id strings.
                // For Json: any valid JSON value (object/array/scalar/null).
                FieldType::Record(_)
                | FieldType::List(_)
                | FieldType::ListRecord(_)
                | FieldType::ListFk { .. }
                | FieldType::Json => {
                    if let Value::String(json_str) = raw_val {
                        if !json_str.is_empty() {
                            if let Ok(parsed) = serde_json::from_str::<Value>(json_str) {
                                entry.insert(field.name.clone(), parsed);
                                continue;
                            }
                        }
                    }
                    // Null / empty / unparseable
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

// ---------------------------------------------------------------------------
// Tests (Task 1.9 — AC1.9: depth-3 round-trip) + T006 Phase 2 ACs
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::schema::Schema;
    use serde_json::json;
    use tempfile::tempdir;

    // ---- T008 Phase 2: coerce_value for Json ----

    #[test]
    fn coerce_value_json_parses_object() {
        let ty = FieldType::Json;
        let raw = r#"{"k":"v"}"#;
        let result = coerce_value(&ty, raw);
        match result {
            Value::Object(map) => {
                assert_eq!(map.get("k").and_then(|v| v.as_str()), Some("v"));
            }
            other => panic!("expected Value::Object, got: {:?}", other),
        }
    }

    #[test]
    fn coerce_value_json_parses_array() {
        let ty = FieldType::Json;
        let raw = "[1,2,3]";
        let result = coerce_value(&ty, raw);
        match result {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 3);
                assert_eq!(arr[0], Value::from(1i64));
                assert_eq!(arr[1], Value::from(2i64));
                assert_eq!(arr[2], Value::from(3i64));
            }
            other => panic!("expected Value::Array, got: {:?}", other),
        }
    }

    #[test]
    fn coerce_value_json_parses_scalar() {
        let ty = FieldType::Json;
        let raw = "42";
        let result = coerce_value(&ty, raw);
        match result {
            Value::Number(n) => {
                assert_eq!(n.as_i64(), Some(42));
            }
            other => panic!("expected Value::Number, got: {:?}", other),
        }
    }

    #[test]
    fn coerce_value_json_bad_returns_sentinel_string() {
        let ty = FieldType::Json;
        let raw = "{not json";
        let result = coerce_value(&ty, raw);
        assert_eq!(
            result,
            Value::String(raw.to_string()),
            "bad JSON must return sentinel String"
        );
    }

    // ---- T006 Phase 2: coerce_value for ListRecord / ListFk ----

    #[test]
    fn coerce_value_list_record_valid_json_returns_array() {
        let ty = FieldType::ListRecord(vec![]); // sub-fields not needed for coerce test
        let raw = r#"[{"system":"docker","kind":"container","id":"foo"}]"#;
        let result = coerce_value(&ty, raw);
        match result {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 1);
                assert_eq!(arr[0]["system"], "docker");
                assert_eq!(arr[0]["kind"], "container");
                assert_eq!(arr[0]["id"], "foo");
            }
            other => panic!("expected Value::Array, got: {:?}", other),
        }
    }

    #[test]
    fn coerce_value_list_record_bad_json_returns_sentinel_string() {
        // T006 REVISE 1: bad JSON returns Value::String(raw) (sentinel) not Value::Null.
        // Value::Null is a valid nullable value and would silently pass validation for
        // optional fields; Value::String triggers the type-shape validator unconditionally.
        let ty = FieldType::ListRecord(vec![]);
        let raw = "{not json";
        let result = coerce_value(&ty, raw);
        assert_eq!(
            result,
            Value::String(raw.to_string()),
            "bad JSON must return sentinel String (T006 REVISE 1)"
        );
    }

    #[test]
    fn coerce_value_list_record_non_array_json_returns_sentinel_string() {
        // T006 REVISE 1: non-array JSON also returns sentinel String, not Null.
        let ty = FieldType::ListRecord(vec![]);
        let raw = r#"{"system":"docker"}"#; // object, not array
        let result = coerce_value(&ty, raw);
        assert_eq!(
            result,
            Value::String(raw.to_string()),
            "non-array JSON must return sentinel String (T006 REVISE 1)"
        );
    }

    #[test]
    fn coerce_value_list_fk_valid_json_returns_array() {
        use crate::schema::FieldType;
        let ty = FieldType::ListFk { ref_store: "tasks".to_string() };
        let raw = r#"["L001","L002"]"#;
        let result = coerce_value(&ty, raw);
        match result {
            Value::Array(arr) => {
                let ids: Vec<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
                assert_eq!(ids, vec!["L001", "L002"]);
            }
            other => panic!("expected Value::Array, got: {:?}", other),
        }
    }

    #[test]
    fn coerce_value_list_fk_bad_json_returns_sentinel_string() {
        // T006 REVISE 1: bad JSON returns Value::String(raw) sentinel, not Value::Null.
        use crate::schema::FieldType;
        let ty = FieldType::ListFk { ref_store: "tasks".to_string() };
        let raw = "{not json";
        let result = coerce_value(&ty, raw);
        assert_eq!(result, Value::String(raw.to_string()));
    }

    // ---- assemble_field_value: repeated-flag and bare-string auto-promote ----
    // Closes the L267-walk friction: --linked-observations L001 (bare) and
    // --linked-observations L001 --linked-observations L002 (repeated) both work.

    #[test]
    fn assemble_list_fk_bare_string_auto_promotes_to_single_element_array() {
        let ty = FieldType::ListFk { ref_store: "tasks".to_string() };
        let v = assemble_field_value(&ty, &["L001".to_string()]);
        assert_eq!(v, Value::Array(vec![Value::String("L001".to_string())]));
    }

    #[test]
    fn assemble_list_fk_single_json_array_passes_through() {
        let ty = FieldType::ListFk { ref_store: "tasks".to_string() };
        let v = assemble_field_value(&ty, &[r#"["L001","L002"]"#.to_string()]);
        match v {
            Value::Array(arr) => {
                let ids: Vec<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
                assert_eq!(ids, vec!["L001", "L002"]);
            }
            other => panic!("expected Value::Array, got: {:?}", other),
        }
    }

    #[test]
    fn assemble_list_fk_repeated_flags_collect_to_array() {
        let ty = FieldType::ListFk { ref_store: "tasks".to_string() };
        let v = assemble_field_value(&ty, &["L001".to_string(), "L002".to_string(), "L003".to_string()]);
        match v {
            Value::Array(arr) => {
                let ids: Vec<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
                assert_eq!(ids, vec!["L001", "L002", "L003"]);
            }
            other => panic!("expected Value::Array, got: {:?}", other),
        }
    }

    #[test]
    fn assemble_list_record_single_json_object_wraps_in_array() {
        let ty = FieldType::ListRecord(vec![]);
        let raw = r#"{"system":"sentry","kind":"issue","id":"PROJ-1"}"#;
        let v = assemble_field_value(&ty, &[raw.to_string()]);
        match v {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 1);
                assert_eq!(arr[0]["system"], "sentry");
                assert_eq!(arr[0]["id"], "PROJ-1");
            }
            other => panic!("expected Value::Array, got: {:?}", other),
        }
    }

    #[test]
    fn assemble_list_record_single_json_array_passes_through() {
        let ty = FieldType::ListRecord(vec![]);
        let raw = r#"[{"system":"sentry","kind":"issue","id":"X"}]"#;
        let v = assemble_field_value(&ty, &[raw.to_string()]);
        match v {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 1);
                assert_eq!(arr[0]["system"], "sentry");
            }
            other => panic!("expected Value::Array, got: {:?}", other),
        }
    }

    #[test]
    fn assemble_list_record_repeated_json_objects_collect_to_array() {
        let ty = FieldType::ListRecord(vec![]);
        let v = assemble_field_value(&ty, &[
            r#"{"system":"sentry","kind":"issue","id":"X"}"#.to_string(),
            r#"{"system":"github","kind":"commit","id":"abc"}"#.to_string(),
        ]);
        match v {
            Value::Array(arr) => {
                assert_eq!(arr.len(), 2);
                assert_eq!(arr[0]["system"], "sentry");
                assert_eq!(arr[1]["system"], "github");
            }
            other => panic!("expected Value::Array, got: {:?}", other),
        }
    }

    #[test]
    fn assemble_list_record_bad_json_returns_sentinel_string() {
        // Preserves the T006 REVISE-1 contract: bad JSON surfaces via the validator,
        // not silently as Null.
        let ty = FieldType::ListRecord(vec![]);
        let v = assemble_field_value(&ty, &["{not json".to_string()]);
        assert_eq!(v, Value::String("{not json".to_string()));
    }

    #[test]
    fn assemble_list_pipe_join_back_compat() {
        // List(_) keeps its existing pipe-split semantics: --foo "X|Y" and
        // --foo X --foo Y produce the same array.
        let ty = FieldType::List(Box::new(FieldType::Text));
        let from_pipe = assemble_field_value(&ty, &["X|Y".to_string()]);
        let from_repeat = assemble_field_value(&ty, &["X".to_string(), "Y".to_string()]);
        assert_eq!(from_pipe, from_repeat);
    }

    // ---- T008 Phase 4: read_row round-trip for Json fields ----

    const JSON_FIELD_SCHEMA: &str = r#"
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

    fn setup_json_schema_and_db() -> (Schema, tempfile::TempDir, rusqlite::Connection) {
        let schema = Schema::from_yaml(JSON_FIELD_SCHEMA).unwrap();
        let (dir, conn) = open_test_db();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, dir, conn)
    }

    /// Phase 4 AC1: read_row returns Value::Object for a Json field stored as JSON TEXT.
    #[test]
    fn read_row_json_field_returns_structured_object() {
        let (schema, _dir, conn) = setup_json_schema_and_db();

        let notes_json = r#"{"k":"v","arr":[1,2]}"#;
        conn.execute(
            "INSERT INTO jstore (display_id, status, created_at, updated_at, created_by, updated_by, title, notes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "J001", "open", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
                "human", "human", "Test", notes_json,
            ],
        ).unwrap();

        let (_id, entry) = read_row(&schema, &conn, "J001").unwrap();
        let notes = entry.get("notes").expect("notes should be present");

        // Must be a structured object, not a string
        match notes {
            Value::Object(map) => {
                assert_eq!(map.get("k").and_then(|v| v.as_str()), Some("v"), "notes.k should be 'v'");
                let arr = map.get("arr").and_then(|v| v.as_array()).expect("notes.arr should be array");
                assert_eq!(arr.len(), 2);
                assert_eq!(arr[0], Value::from(1i64));
                assert_eq!(arr[1], Value::from(2i64));
            }
            other => panic!("expected Value::Object for notes, got: {:?}", other),
        }
    }

    /// Phase 4 AC4: empty / NULL Json cell on read yields Value::Null.
    #[test]
    fn read_row_json_field_null_cell_returns_null() {
        let (schema, _dir, conn) = setup_json_schema_and_db();

        // Store JSON literal "null" (the Decision 4 absent-field default)
        conn.execute(
            "INSERT INTO jstore (display_id, status, created_at, updated_at, created_by, updated_by, title, notes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "J002", "open", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
                "human", "human", "Null Test", "null",
            ],
        ).unwrap();

        let (_id, entry) = read_row(&schema, &conn, "J002").unwrap();
        let notes = entry.get("notes").expect("notes key should be present");
        assert_eq!(*notes, Value::Null, "stored 'null' literal should read back as Value::Null");
    }

    // Schema with depth-3 nesting: plan.phases[N].name and cycles[N].executor.summary
    const DEPTH3_SCHEMA: &str = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [open, done]
  transitions: []
fields:
  - name: title
    type: text
  - name: plan
    type: record
    fields:
      - name: done_when
        type: text
      - name: phases
        type: list_record
        fields:
          - name: name
            type: text
          - name: objective
            type: text
  - name: cycles
    type: list_record
    fields:
      - name: phase
        type: integer
      - name: cycle
        type: integer
      - name: executor
        type: record
        fields:
          - name: summary
            type: text
          - name: commit
            type: text
  - name: depends_on
    type: list_fk
    ref: tasks
"#;

    fn open_test_db() -> (tempfile::TempDir, rusqlite::Connection) {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.db");
        let conn = db::open(&db_file).unwrap();
        (dir, conn)
    }

    fn setup_schema_and_db() -> (Schema, tempfile::TempDir, rusqlite::Connection) {
        let schema = Schema::from_yaml(DEPTH3_SCHEMA).unwrap();
        let (dir, conn) = open_test_db();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();
        (schema, dir, conn)
    }

    #[test]
    fn list_record_and_list_fk_ddl_is_text() {
        let schema = Schema::from_yaml(DEPTH3_SCHEMA).unwrap();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        assert!(ddl.contains("plan TEXT"), "plan should be TEXT: {ddl}");
        assert!(ddl.contains("cycles TEXT"), "cycles should be TEXT: {ddl}");
        assert!(ddl.contains("depends_on TEXT"), "depends_on should be TEXT: {ddl}");
    }

    /// AC1.9: plan.phases[2].name (record → list_record → element → string) round-trips.
    #[test]
    fn depth3_plan_phases_round_trips() {
        let (schema, _dir, conn) = setup_schema_and_db();

        let plan_json = json!({
            "done_when": "All phases pass review",
            "phases": [
                {"name": "Phase 1", "objective": "Foundation"},
                {"name": "Phase 2", "objective": "Engine"},
                {"name": "Phase 3", "objective": "Integration"}
            ]
        });

        // Insert row directly
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, title, plan, cycles, depends_on) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                "T001", "open", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
                "human", "human", "Test Task",
                serde_json::to_string(&plan_json).unwrap(),
                serde_json::to_string(&json!([])).unwrap(),
                serde_json::to_string(&json!([])).unwrap(),
            ],
        ).unwrap();

        let (_id, entry) = read_row(&schema, &conn, "T001").unwrap();

        // Verify plan.phases[2].name round-trips
        let plan = entry.get("plan").expect("plan should be present");
        let phases = plan.get("phases").expect("phases should be present");
        let phase3_name = phases[2]["name"].as_str().expect("phase 3 name should be a string");
        assert_eq!(phase3_name, "Phase 3", "plan.phases[2].name should round-trip");
    }

    /// AC1.9: cycles[1].executor.summary (list_record → record → string) round-trips.
    #[test]
    fn depth3_cycles_executor_summary_round_trips() {
        let (schema, _dir, conn) = setup_schema_and_db();

        let cycles_json = json!([
            {
                "phase": 1,
                "cycle": 1,
                "executor": { "summary": "First cycle done", "commit": "abc123" }
            },
            {
                "phase": 1,
                "cycle": 2,
                "executor": { "summary": "Revised cycle", "commit": "def456" }
            }
        ]);

        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, title, plan, cycles, depends_on) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                "T002", "open", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
                "human", "human", "Cycle Test",
                serde_json::to_string(&json!({"done_when": "", "phases": []})).unwrap(),
                serde_json::to_string(&cycles_json).unwrap(),
                serde_json::to_string(&json!([])).unwrap(),
            ],
        ).unwrap();

        let (_id, entry) = read_row(&schema, &conn, "T002").unwrap();

        let cycles = entry.get("cycles").expect("cycles should be present");
        let summary = cycles[1]["executor"]["summary"].as_str()
            .expect("cycles[1].executor.summary should be a string");
        assert_eq!(summary, "Revised cycle", "cycles[1].executor.summary should round-trip");
    }

    /// AC1.8: depends_on list_fk round-trips as Vec<String>.
    #[test]
    fn list_fk_round_trips() {
        let (schema, _dir, conn) = setup_schema_and_db();

        let depends_on = json!(["T001", "T002"]);
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, title, plan, cycles, depends_on) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                "T003", "open", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
                "human", "human", "FK Test",
                serde_json::to_string(&json!({"done_when": "", "phases": []})).unwrap(),
                serde_json::to_string(&json!([])).unwrap(),
                serde_json::to_string(&depends_on).unwrap(),
            ],
        ).unwrap();

        let (_id, entry) = read_row(&schema, &conn, "T003").unwrap();
        let dep = entry.get("depends_on").expect("depends_on should be present");
        let ids: Vec<&str> = dep.as_array().unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["T001", "T002"]);
    }

    /// m2 (AC1.7): update overwrites a `cycles` JSON cell and reads it back correctly.
    /// Covers add → update → show round-trip for a list_record column.
    #[test]
    fn cycles_update_round_trips() {
        let (schema, _dir, conn) = setup_schema_and_db();

        // Initial cycles: one element
        let initial_cycles = json!([
            { "phase": 1, "cycle": 1, "executor": { "summary": "first draft", "commit": "abc" } }
        ]);
        conn.execute(
            "INSERT INTO tasks (display_id, status, created_at, updated_at, created_by, updated_by, title, plan, cycles, depends_on) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                "T004", "open", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
                "human", "human", "Update Test",
                serde_json::to_string(&json!({"done_when": "", "phases": []})).unwrap(),
                serde_json::to_string(&initial_cycles).unwrap(),
                serde_json::to_string(&json!([])).unwrap(),
            ],
        ).unwrap();

        // Read back initial
        let (_id, entry) = read_row(&schema, &conn, "T004").unwrap();
        let cycles = entry.get("cycles").expect("cycles present").as_array().unwrap();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0]["executor"]["summary"], "first draft");

        // Update: replace cycles with two elements (add an element, modify the first)
        let updated_cycles = json!([
            { "phase": 1, "cycle": 1, "executor": { "summary": "revised draft", "commit": "def" } },
            { "phase": 1, "cycle": 2, "executor": { "summary": "final", "commit": "ghi" } }
        ]);
        conn.execute(
            "UPDATE tasks SET cycles = ?1 WHERE display_id = ?2",
            rusqlite::params![
                serde_json::to_string(&updated_cycles).unwrap(),
                "T004",
            ],
        ).unwrap();

        // Read back updated
        let (_id, entry2) = read_row(&schema, &conn, "T004").unwrap();
        let cycles2 = entry2.get("cycles").expect("cycles present").as_array().unwrap();
        assert_eq!(cycles2.len(), 2, "should have 2 cycles after update");
        assert_eq!(cycles2[0]["executor"]["summary"], "revised draft");
        assert_eq!(cycles2[0]["executor"]["commit"], "def");
        assert_eq!(cycles2[1]["executor"]["summary"], "final");
        assert_eq!(cycles2[1]["executor"]["commit"], "ghi");
    }
}
