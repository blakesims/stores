use crate::schema::{FieldType, Schema};

/// Quote a SQL identifier using double-quote delimiters (SQL standard).
/// Any internal `"` characters are escaped by doubling them.
/// This makes table names like `observations-1006` safe to use in DDL/DML.
pub(crate) fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Generic, store-agnostic substrate tables. Created once at `stores init`.
/// Currently: `transition_history` — one row per successful lifecycle transition
/// (manual or automatic). policy_ref / policies_hash are NULL for manual paths;
/// the autonomous flow engine fills them on policy-mediated transitions.
pub const SUBSTRATE_DDL: &str = "\
CREATE TABLE IF NOT EXISTS transition_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    store TEXT NOT NULL,
    row_id INTEGER NOT NULL,
    display_id TEXT NOT NULL,
    from_status TEXT,
    to_status TEXT NOT NULL,
    verb TEXT NOT NULL,
    invoker TEXT NOT NULL,
    policy_ref TEXT,
    policies_hash TEXT,
    occurred_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS dispatch_locks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    store TEXT NOT NULL,
    row_id INTEGER NOT NULL,
    display_id TEXT NOT NULL,
    agent_name TEXT NOT NULL,
    transition_id INTEGER,
    claimed_at TEXT NOT NULL,
    claimed_by TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 1,
    last_status TEXT,
    finished_at TEXT,
    UNIQUE(store, row_id, agent_name)
);
";

/// Reserved columns prepended to every generated table.
/// Order is fixed for determinism.
const RESERVED_COLUMNS: &[&str] = &[
    "id INTEGER PRIMARY KEY AUTOINCREMENT",
    "display_id TEXT UNIQUE NOT NULL",
    "status TEXT NOT NULL",
    "created_at TEXT",
    "updated_at TEXT",
    "created_by TEXT",
    "updated_by TEXT",
];

/// Map a scalar FieldType to its SQLite column definition fragment (type + optional CHECK).
/// Returns `None` for Record and List — those collapse to a single JSON TEXT column
/// and are handled separately.
fn scalar_col_def(field_name: &str, ty: &FieldType) -> Option<String> {
    match ty {
        FieldType::Text => Some(format!("{field_name} TEXT")),
        FieldType::Integer => Some(format!("{field_name} INTEGER")),
        FieldType::Bool => Some(format!(
            "{field_name} INTEGER CHECK ({field_name} IN (0,1))"
        )),
        FieldType::Timestamp => Some(format!("{field_name} TEXT")),
        FieldType::DisplayId => Some(format!("{field_name} TEXT")),
        FieldType::Enum(values) => {
            // Escape single quotes inside enum values by doubling them (SQL standard).
            // If a value contains a single quote, document loudly and replace.
            let escaped: Vec<String> = values
                .iter()
                .map(|v| {
                    if v.contains('\'') {
                        // v0.1 out-of-scope: fail loudly; caller should catch this
                        // but DDL codegen is infallible in the current design so we
                        // double-quote as a safe fallback and leave a note.
                        v.replace('\'', "''")
                    } else {
                        v.clone()
                    }
                })
                .collect();
            let list = escaped
                .iter()
                .map(|v| format!("'{v}'"))
                .collect::<Vec<_>>()
                .join(", ");
            Some(format!(
                "{field_name} TEXT CHECK ({field_name} IN ({list}))"
            ))
        }
        FieldType::List(_)
        | FieldType::Record(_)
        | FieldType::ListRecord(_)
        | FieldType::ListFk { .. }
        | FieldType::Json => None,
    }
}

/// Description of a column the substrate expects a generated table to have.
///
/// `name` and `sql_type` are the two halves of the column definition that
/// the migrate diff cares about; `full_def` is the complete fragment (with
/// any CHECK clause) that DDL codegen needs to emit a CREATE TABLE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedColumn {
    pub name: String,
    pub sql_type: String,
    pub full_def: String,
    pub is_reserved: bool,
}

/// Parse a reserved-column static string ("id INTEGER PRIMARY KEY AUTOINCREMENT")
/// into name + sql_type, keeping the full definition intact.
fn parse_reserved(def: &str) -> ExpectedColumn {
    let mut parts = def.splitn(3, ' ');
    let name = parts.next().expect("reserved col has name").to_string();
    let sql_type = parts.next().expect("reserved col has type").to_string();
    ExpectedColumn {
        name,
        sql_type,
        full_def: def.to_string(),
        is_reserved: true,
    }
}

/// Return the deterministic, ordered list of columns the substrate expects
/// for a generated store table: reserved columns first, then user scalar
/// fields in schema order, then JSON-blob columns (List/Record/etc.) in
/// schema order.
///
/// `ddl_for` is built on top of this; migrate.rs uses it to diff the live
/// DB against the compiled-in schema without re-implementing column logic.
pub fn expected_columns(schema: &Schema) -> Vec<ExpectedColumn> {
    let mut cols: Vec<ExpectedColumn> = Vec::new();

    for def in RESERVED_COLUMNS {
        cols.push(parse_reserved(def));
    }

    let mut scalar_cols: Vec<ExpectedColumn> = Vec::new();
    let mut json_cols: Vec<ExpectedColumn> = Vec::new();

    for field in &schema.fields {
        match &field.ty {
            FieldType::Record(_)
            | FieldType::List(_)
            | FieldType::ListRecord(_)
            | FieldType::ListFk { .. }
            | FieldType::Json => {
                json_cols.push(ExpectedColumn {
                    name: field.name.clone(),
                    sql_type: "TEXT".to_string(),
                    full_def: format!("{} TEXT", field.name),
                    is_reserved: false,
                });
            }
            ty => {
                if let Some(def) = scalar_col_def(&field.name, ty) {
                    let sql_type = match ty {
                        FieldType::Integer | FieldType::Bool => "INTEGER",
                        _ => "TEXT",
                    }
                    .to_string();
                    scalar_cols.push(ExpectedColumn {
                        name: field.name.clone(),
                        sql_type,
                        full_def: def,
                        is_reserved: false,
                    });
                }
            }
        }
    }

    cols.extend(scalar_cols);
    cols.extend(json_cols);
    cols
}

/// Generate a `CREATE TABLE IF NOT EXISTS` DDL statement for the given schema.
///
/// Column ordering: reserved columns first, then user-declared scalar fields
/// in schema order, then JSON columns for List/Record fields in schema order.
/// This produces deterministic SQL for the same input.
pub fn ddl_for(schema: &Schema) -> String {
    let table = quote_ident(&schema.name);
    let col_block = expected_columns(schema)
        .iter()
        .map(|c| format!("    {}", c.full_def))
        .collect::<Vec<_>>()
        .join(",\n");

    // Prepend the substrate-level DDL so any caller that runs `ddl_for(schema)`
    // (production install path *and* every test that builds a fresh connection)
    // gets the substrate `transition_history` table for free. Both blocks are
    // idempotent (CREATE IF NOT EXISTS), so running them twice is a no-op.
    format!("{SUBSTRATE_DDL}\nCREATE TABLE IF NOT EXISTS {table} (\n{col_block}\n);")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Schema;

    const ALL_TYPES_FIXTURE: &str =
        include_str!("../../tests/fixtures/all_types_store/schema.yaml");

    #[test]
    fn ddl_contains_reserved_columns() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let ddl = ddl_for(&schema);
        assert!(
            ddl.contains("id INTEGER PRIMARY KEY AUTOINCREMENT"),
            "missing id: {ddl}"
        );
        assert!(
            ddl.contains("display_id TEXT UNIQUE NOT NULL"),
            "missing display_id: {ddl}"
        );
        assert!(
            ddl.contains("status TEXT NOT NULL"),
            "missing status: {ddl}"
        );
        assert!(ddl.contains("created_at TEXT"), "missing created_at: {ddl}");
        assert!(ddl.contains("updated_at TEXT"), "missing updated_at: {ddl}");
        assert!(ddl.contains("created_by TEXT"), "missing created_by: {ddl}");
        assert!(ddl.contains("updated_by TEXT"), "missing updated_by: {ddl}");
    }

    #[test]
    fn ddl_scalar_column_types() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let ddl = ddl_for(&schema);
        // Text
        assert!(ddl.contains("title TEXT"), "missing title TEXT: {ddl}");
        // Integer
        assert!(
            ddl.contains("count INTEGER"),
            "missing count INTEGER: {ddl}"
        );
        // Bool → INTEGER with CHECK
        assert!(
            ddl.contains("active INTEGER CHECK (active IN (0,1))"),
            "missing bool check: {ddl}"
        );
        // Timestamp → TEXT
        assert!(
            ddl.contains("observed_at TEXT"),
            "missing observed_at TEXT: {ddl}"
        );
        // DisplayId → TEXT
        assert!(ddl.contains("ref_id TEXT"), "missing ref_id TEXT: {ddl}");
    }

    #[test]
    fn ddl_enum_check_constraint() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let ddl = ddl_for(&schema);
        // Enum with CHECK
        assert!(
            ddl.contains("priority TEXT CHECK (priority IN ('low', 'medium', 'high'))"),
            "missing enum check: {ddl}"
        );
    }

    #[test]
    fn ddl_json_columns_are_text() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let ddl = ddl_for(&schema);
        // List<Text> → TEXT (JSON)
        assert!(ddl.contains("tags TEXT"), "missing tags TEXT: {ddl}");
        // Record → TEXT (JSON)
        assert!(ddl.contains("details TEXT"), "missing details TEXT: {ddl}");
        // Json → TEXT (no CHECK clause)
        assert!(
            ddl.contains("metadata TEXT"),
            "missing metadata TEXT: {ddl}"
        );
        // Ensure no CHECK clause for the json column
        assert!(
            !ddl.contains("metadata TEXT CHECK"),
            "json field must not have CHECK clause: {ddl}"
        );
    }

    #[test]
    fn ddl_is_deterministic() {
        let schema1 = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let schema2 = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        assert_eq!(ddl_for(&schema1), ddl_for(&schema2));
    }

    #[test]
    fn ddl_snapshot() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let ddl = ddl_for(&schema);
        let expected = format!(
            "{SUBSTRATE_DDL}\n{}",
            concat!(
                "CREATE TABLE IF NOT EXISTS \"kitchen_sink\" (\n",
                "    id INTEGER PRIMARY KEY AUTOINCREMENT,\n",
                "    display_id TEXT UNIQUE NOT NULL,\n",
                "    status TEXT NOT NULL,\n",
                "    created_at TEXT,\n",
                "    updated_at TEXT,\n",
                "    created_by TEXT,\n",
                "    updated_by TEXT,\n",
                "    title TEXT,\n",
                "    slug TEXT,\n",
                "    count INTEGER,\n",
                "    active INTEGER CHECK (active IN (0,1)),\n",
                "    priority TEXT CHECK (priority IN ('low', 'medium', 'high')),\n",
                "    ref_id TEXT,\n",
                "    observed_at TEXT,\n",
                "    tags TEXT,\n",
                "    triage TEXT,\n",
                "    contract TEXT,\n",
                "    details TEXT,\n",
                "    metadata TEXT\n",
                ");"
            )
        );
        assert_eq!(ddl, expected, "DDL snapshot mismatch.\nGot:\n{ddl}");
    }

    /// AC1.11 (Task 1.11): A field with actor: framework produces the same DDL column
    /// type as an equivalent field without the actor constraint.  Storage is type-only;
    /// actor scoping is enforced by the validator, not the database.
    #[test]
    fn framework_actor_field_ddl_same_as_non_framework() {
        // Schema with claimed_by (text, actor: framework) and title (text, no actor)
        let yaml_framework = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: claimed_by
    type: text
    actor: framework
  - name: current_phase
    type: integer
    actor: framework
"#;
        let yaml_no_actor = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: claimed_by
    type: text
  - name: current_phase
    type: integer
"#;
        let schema_fw = Schema::from_yaml(yaml_framework).unwrap();
        let schema_no = Schema::from_yaml(yaml_no_actor).unwrap();
        let ddl_fw = ddl_for(&schema_fw);
        let ddl_no = ddl_for(&schema_no);
        assert_eq!(
            ddl_fw, ddl_no,
            "framework-actor fields must produce identical DDL to non-actor fields.\nFW:\n{ddl_fw}\nNO:\n{ddl_no}"
        );
        // Specifically check that claimed_by is TEXT (not modified by actor attribute)
        assert!(
            ddl_fw.contains("claimed_by TEXT"),
            "claimed_by must be TEXT: {ddl_fw}"
        );
        assert!(
            ddl_fw.contains("current_phase INTEGER"),
            "current_phase must be INTEGER: {ddl_fw}"
        );
    }

    // ---- quote_ident tests (Phase 3 / Finding C) ----

    #[test]
    fn quote_ident_plain() {
        assert_eq!(quote_ident("observations"), "\"observations\"");
    }

    #[test]
    fn quote_ident_hyphenated() {
        assert_eq!(quote_ident("observations-1006"), "\"observations-1006\"");
    }

    #[test]
    fn quote_ident_escapes_internal_double_quote() {
        assert_eq!(quote_ident("foo\"bar"), "\"foo\"\"bar\"");
    }

    // ---- expected_columns tests (Phase 1, T017) ----

    #[test]
    fn expected_columns_reserved_present_in_order() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let cols = expected_columns(&schema);
        let reserved: Vec<&ExpectedColumn> = cols.iter().filter(|c| c.is_reserved).collect();
        let names: Vec<&str> = reserved.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "id",
                "display_id",
                "status",
                "created_at",
                "updated_at",
                "created_by",
                "updated_by",
            ]
        );
        // sql_type populated for every reserved entry
        for c in &reserved {
            assert!(
                !c.sql_type.is_empty(),
                "reserved column {} has empty sql_type",
                c.name
            );
        }
        // id is INTEGER, the rest are TEXT
        assert_eq!(reserved[0].sql_type, "INTEGER");
        for c in &reserved[1..] {
            assert_eq!(c.sql_type, "TEXT", "reserved {} expected TEXT", c.name);
        }
    }

    #[test]
    fn expected_columns_text_field() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let cols = expected_columns(&schema);
        let title = cols.iter().find(|c| c.name == "title").expect("title col");
        assert!(!title.is_reserved);
        assert_eq!(title.sql_type, "TEXT");
        assert_eq!(title.full_def, "title TEXT");
    }

    #[test]
    fn expected_columns_bool_field_has_check() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let cols = expected_columns(&schema);
        let active = cols.iter().find(|c| c.name == "active").expect("active");
        assert_eq!(active.sql_type, "INTEGER");
        assert!(
            active.full_def.contains("CHECK (active IN (0,1))"),
            "bool full_def must include CHECK clause: {}",
            active.full_def
        );
    }

    #[test]
    fn expected_columns_enum_field_has_check() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let cols = expected_columns(&schema);
        let priority = cols
            .iter()
            .find(|c| c.name == "priority")
            .expect("priority");
        assert_eq!(priority.sql_type, "TEXT");
        assert!(
            priority.full_def.contains("CHECK (priority IN ("),
            "enum full_def must include CHECK clause: {}",
            priority.full_def
        );
    }

    #[test]
    fn expected_columns_json_blob_fields_have_no_check() {
        let schema = Schema::from_yaml(ALL_TYPES_FIXTURE).unwrap();
        let cols = expected_columns(&schema);
        for name in &["tags", "details", "metadata"] {
            let c = cols
                .iter()
                .find(|c| &c.name == name)
                .unwrap_or_else(|| panic!("missing json field {name}"));
            assert_eq!(c.sql_type, "TEXT", "{name} should be TEXT");
            assert_eq!(c.full_def, format!("{name} TEXT"), "{name} full_def");
            assert!(
                !c.full_def.contains("CHECK"),
                "{name} must not have CHECK clause"
            );
        }
    }

    /// AC Phase 3: DDL for a hyphenated store name produces a quoted identifier
    /// and is accepted by SQLite.
    #[test]
    fn ddl_hyphenated_name_accepted_by_sqlite() {
        let yaml = r#"
name: obs-test-1006
id_format: "O{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: summary
    type: text
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let ddl = ddl_for(&schema);
        assert!(
            ddl.contains("CREATE TABLE IF NOT EXISTS \"obs-test-1006\""),
            "expected quoted hyphenated identifier in DDL; got:\n{ddl}"
        );

        // Verify SQLite accepts the DDL.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(&ddl)
            .expect("SQLite must accept DDL with quoted hyphenated table name");
    }
}
