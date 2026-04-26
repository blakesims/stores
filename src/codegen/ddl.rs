use crate::schema::{FieldType, Schema};

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
        FieldType::Bool => Some(format!("{field_name} INTEGER CHECK ({field_name} IN (0,1))")),
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
        FieldType::List(_) | FieldType::Record(_) => None,
    }
}

/// Generate a `CREATE TABLE IF NOT EXISTS` DDL statement for the given schema.
///
/// Column ordering: reserved columns first, then user-declared scalar fields
/// in schema order, then JSON columns for List/Record fields in schema order.
/// This produces deterministic SQL for the same input.
pub fn ddl_for(schema: &Schema) -> String {
    let table = &schema.name;

    // Collect scalar column defs (Text, Integer, Bool, Timestamp, DisplayId, Enum)
    let mut scalar_defs: Vec<String> = Vec::new();
    // Collect JSON column defs (List, Record)
    let mut json_defs: Vec<String> = Vec::new();

    for field in &schema.fields {
        match &field.ty {
            FieldType::Record(_) | FieldType::List(_) => {
                json_defs.push(format!("{} TEXT", field.name));
            }
            ty => {
                if let Some(def) = scalar_col_def(&field.name, ty) {
                    scalar_defs.push(def);
                }
            }
        }
    }

    // Build full column list: reserved + scalars + JSON blobs
    let mut all_cols: Vec<String> = Vec::new();
    all_cols.extend(RESERVED_COLUMNS.iter().map(|s| s.to_string()));
    all_cols.extend(scalar_defs);
    all_cols.extend(json_defs);

    let col_block = all_cols
        .iter()
        .map(|c| format!("    {c}"))
        .collect::<Vec<_>>()
        .join(",\n");

    format!("CREATE TABLE IF NOT EXISTS {table} (\n{col_block}\n);")
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
        assert!(ddl.contains("id INTEGER PRIMARY KEY AUTOINCREMENT"), "missing id: {ddl}");
        assert!(ddl.contains("display_id TEXT UNIQUE NOT NULL"), "missing display_id: {ddl}");
        assert!(ddl.contains("status TEXT NOT NULL"), "missing status: {ddl}");
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
        assert!(ddl.contains("count INTEGER"), "missing count INTEGER: {ddl}");
        // Bool → INTEGER with CHECK
        assert!(ddl.contains("active INTEGER CHECK (active IN (0,1))"), "missing bool check: {ddl}");
        // Timestamp → TEXT
        assert!(ddl.contains("observed_at TEXT"), "missing observed_at TEXT: {ddl}");
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
        let expected = concat!(
            "CREATE TABLE IF NOT EXISTS kitchen_sink (\n",
            "    id INTEGER PRIMARY KEY AUTOINCREMENT,\n",
            "    display_id TEXT UNIQUE NOT NULL,\n",
            "    status TEXT NOT NULL,\n",
            "    created_at TEXT,\n",
            "    updated_at TEXT,\n",
            "    created_by TEXT,\n",
            "    updated_by TEXT,\n",
            "    title TEXT,\n",
            "    count INTEGER,\n",
            "    active INTEGER CHECK (active IN (0,1)),\n",
            "    priority TEXT CHECK (priority IN ('low', 'medium', 'high')),\n",
            "    ref_id TEXT,\n",
            "    observed_at TEXT,\n",
            "    tags TEXT,\n",
            "    details TEXT\n",
            ");"
        );
        assert_eq!(ddl, expected, "DDL snapshot mismatch.\nGot:\n{ddl}");
    }
}
