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
///
/// Depth support: paths of length 1 (top-level) or 2 (Record sub-field) are the
/// only shapes produced by the flat CLI arg surface for v0.1/v0.2.  ListRecord
/// and ListFk fields cannot be set via flat CLI args; they are written programmatically
/// by the submit handlers (Phase 5) using JSON directly.
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

        // Insert value at the correct depth.
        insert_at_path(&mut entry, &leaf.path, value);
    }

    Ok(entry)
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
        // Parse it; on failure (bad JSON or non-array shape) return Null so the
        // validator surfaces the field name via "required" or "invalid" error.
        FieldType::ListRecord(_) | FieldType::ListFk { .. } => {
            match serde_json::from_str::<Value>(raw) {
                Ok(Value::Array(arr)) => Value::Array(arr),
                _ => Value::Null,
            }
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
                FieldType::Record(_)
                | FieldType::List(_)
                | FieldType::ListRecord(_)
                | FieldType::ListFk { .. } => {
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
    fn coerce_value_list_record_bad_json_returns_null() {
        let ty = FieldType::ListRecord(vec![]);
        let raw = "{not json";
        let result = coerce_value(&ty, raw);
        assert_eq!(result, Value::Null, "bad JSON must return Null (fail-silent)");
    }

    #[test]
    fn coerce_value_list_record_non_array_json_returns_null() {
        let ty = FieldType::ListRecord(vec![]);
        let raw = r#"{"system":"docker"}"#; // object, not array
        let result = coerce_value(&ty, raw);
        assert_eq!(result, Value::Null, "non-array JSON must return Null");
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
    fn coerce_value_list_fk_bad_json_returns_null() {
        use crate::schema::FieldType;
        let ty = FieldType::ListFk { ref_store: "tasks".to_string() };
        let raw = "{not json";
        let result = coerce_value(&ty, raw);
        assert_eq!(result, Value::Null);
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
