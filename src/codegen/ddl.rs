use crate::schema::{Field, FieldType, Schema};

/// One column declared by SUBSTRATE_DDL. Used by framework-migration drift
/// detection in `handlers::framework_migrate` to ALTER older DBs up to the
/// current binary's compiled-in DDL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameworkColumn {
    pub name: &'static str,
    pub sql_type: &'static str,
    pub nullable: bool,
    pub default_sql: Option<&'static str>,
    /// Full column-definition fragment as it appears inside the CREATE TABLE
    /// (matches SUBSTRATE_DDL verbatim minus the trailing comma).
    pub full_def: &'static str,
    /// True if this column was added to an already-shipped table after v0.1
    /// and is therefore a candidate for ALTER TABLE ADD COLUMN against an
    /// existing DB. Such columns MUST be nullable or carry a DEFAULT —
    /// `validate_framework_tables` enforces this. Columns present in the
    /// table's first CREATE TABLE version are `additive: false` (they only
    /// ever materialise via CREATE TABLE on a fresh DB).
    pub additive: bool,
}

/// One framework-internal table declared by SUBSTRATE_DDL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameworkTable {
    pub name: &'static str,
    pub columns: &'static [FrameworkColumn],
}

/// Mirror of SUBSTRATE_DDL for column-level introspection. Every table here
/// MUST list every column declared in SUBSTRATE_DDL — drift between the two
/// is checked by `framework_tables_match_substrate_ddl` below.
pub const FRAMEWORK_DDL_TABLES: &[FrameworkTable] = &[
    FrameworkTable {
        name: "transition_history",
        columns: &[
            FrameworkColumn { name: "id", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "id INTEGER PRIMARY KEY AUTOINCREMENT", additive: false },
            FrameworkColumn { name: "store", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "store TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "row_id", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "row_id INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "display_id", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "display_id TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "from_status", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "from_status TEXT", additive: false },
            FrameworkColumn { name: "to_status", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "to_status TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "verb", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "verb TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "invoker", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "invoker TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "policy_ref", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "policy_ref TEXT", additive: false },
            FrameworkColumn { name: "policies_hash", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "policies_hash TEXT", additive: false },
            FrameworkColumn { name: "occurred_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "occurred_at TEXT NOT NULL", additive: false },
            // L144: actor_note added post-v0.1; older DBs lack it. Nullable so
            // ALTER TABLE ADD COLUMN against existing rows is well-defined.
            FrameworkColumn { name: "actor_note", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "actor_note TEXT", additive: true },
        ],
    },
    FrameworkTable {
        name: "dispatch_locks",
        columns: &[
            FrameworkColumn { name: "id", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "id INTEGER PRIMARY KEY AUTOINCREMENT", additive: false },
            FrameworkColumn { name: "store", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "store TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "row_id", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "row_id INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "display_id", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "display_id TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "agent_name", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "agent_name TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "transition_id", sql_type: "INTEGER", nullable: true, default_sql: None, full_def: "transition_id INTEGER", additive: false },
            FrameworkColumn { name: "claimed_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "claimed_at TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "claimed_by", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "claimed_by TEXT NOT NULL", additive: false },
            // attempts: NOT NULL DEFAULT 1 — additive-safe via DEFAULT.
            FrameworkColumn { name: "attempts", sql_type: "INTEGER", nullable: false, default_sql: Some("1"), full_def: "attempts INTEGER NOT NULL DEFAULT 1", additive: true },
            FrameworkColumn { name: "last_status", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "last_status TEXT", additive: false },
            FrameworkColumn { name: "finished_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "finished_at TEXT", additive: false },
            // L134 / T050: typed lifecycle columns. Additive (added post-baseline).
            // L134 migration in `ensure_dispatch_locks_typed` runs at db::open
            // before `apply_framework_drift`, so on existing DBs these are
            // already present when drift detection sees them.
            FrameworkColumn { name: "daemon_epoch", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "daemon_epoch TEXT", additive: true },
            FrameworkColumn { name: "claim_source", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "claim_source TEXT CHECK(claim_source IN ('try_claim','retry_claim','manual','legacy'))", additive: true },
            FrameworkColumn { name: "attempt", sql_type: "INTEGER", nullable: true, default_sql: None, full_def: "attempt INTEGER", additive: true },
            FrameworkColumn { name: "pid", sql_type: "INTEGER", nullable: true, default_sql: None, full_def: "pid INTEGER", additive: true },
            FrameworkColumn { name: "heartbeat_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "heartbeat_at TEXT", additive: true },
            FrameworkColumn { name: "postcondition_id", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "postcondition_id TEXT", additive: true },
            FrameworkColumn { name: "postcondition_args", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "postcondition_args TEXT", additive: true },
            FrameworkColumn { name: "terminal_reason", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "terminal_reason TEXT CHECK(terminal_reason IN ('ok','exit_nonzero','error','silent_zombie','timeout','halted','legacy_unknown'))", additive: true },
            FrameworkColumn { name: "next_retry_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "next_retry_at TEXT", additive: true },
        ],
    },
    FrameworkTable {
        name: "substrate_migrations",
        columns: &[
            FrameworkColumn { name: "id", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "id INTEGER PRIMARY KEY AUTOINCREMENT", additive: false },
            FrameworkColumn { name: "applied_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "applied_at TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "binary_version", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "binary_version TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "table_name", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "table_name TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "column_name", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "column_name TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "ddl_applied", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "ddl_applied TEXT NOT NULL", additive: false },
        ],
    },
    FrameworkTable {
        name: "framework_migrations",
        columns: &[
            FrameworkColumn { name: "id", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "id TEXT PRIMARY KEY", additive: false },
            FrameworkColumn { name: "applied_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "applied_at TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "note", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "note TEXT", additive: false },
        ],
    },
];

/// Parse `full_def` (the column's SQL fragment) for NOT NULL / DEFAULT
/// presence. Case-insensitive. The validator MUST ground itself in the
/// actual SQL text rather than the parallel `nullable` / `default_sql`
/// metadata, because the metadata is hand-maintained and could disagree
/// with the SQL ("mirror lies"). Codex T051-r1 HIGH.
fn parse_full_def_safety(full_def: &str) -> (bool, bool) {
    // Cheap heuristic: search whitespace-tokenised SQL for NOT NULL and
    // DEFAULT keywords. Only called on additive columns (validator skips
    // PRIMARY KEY / baseline columns first); additive ALTER targets must
    // declare NOT NULL / DEFAULT explicitly in `full_def` for the gate to
    // be meaningful. DEFAULT can be followed by a literal (`DEFAULT 0`),
    // a parenthesized expression (`DEFAULT(0)`, `DEFAULT (now())`), a
    // string (`DEFAULT 'x'`), or a CURRENT_* function — accept any token
    // that is exactly "DEFAULT" OR starts with "DEFAULT(".
    let upper = full_def.to_ascii_uppercase();
    let toks: Vec<&str> = upper.split_whitespace().collect();
    let has_not_null = toks.windows(2).any(|w| w == ["NOT", "NULL"]);
    let has_default = toks
        .iter()
        .any(|tok| *tok == "DEFAULT" || tok.starts_with("DEFAULT("));
    (has_not_null, has_default)
}

/// Walk `tables` and return Err if any additive column declares `NOT NULL`
/// without a `DEFAULT` — adding such a column to an existing non-empty table
/// would fail at ALTER time. Validator parses each column's `full_def` SQL
/// text directly so the gate is grounded in the truth (the DDL string used
/// by ALTER), not in the parallel metadata flags. Used by db::open as a
/// boot-time invariant check. Also refuses if `full_def` and the metadata
/// flags disagree — a "mirror lies" guard so future contributors can't
/// silently bypass the gate by setting nullable=true on a NOT NULL column.
pub fn validate_framework_tables(tables: &[FrameworkTable]) -> anyhow::Result<()> {
    for t in tables {
        for c in t.columns {
            // Only `additive` columns are candidates for ALTER TABLE ADD
            // COLUMN against existing DBs — those are the only ones the gate
            // protects, and the only ones whose mirror-vs-SQL disagreement
            // matters at runtime. Baseline (additive=false) columns appear
            // only via CREATE TABLE on a fresh DB; PRIMARY KEY / SQLite-
            // implicit semantics make their nullable/default flags slippery
            // to express in `full_def` text, but they're harmless because
            // they're never ALTERed. Skip them.
            if !c.additive {
                continue;
            }
            // Mirror-vs-SQL consistency check: parse `full_def` directly
            // (the truth that ALTER TABLE will use) and refuse if metadata
            // disagrees. This is the "mirror lies" guard codex T051-r1
            // flagged: a future contributor can't silently bypass the gate
            // by setting nullable=true on a NOT NULL column.
            let (sql_not_null, sql_has_default) = parse_full_def_safety(c.full_def);
            if sql_not_null == c.nullable {
                anyhow::bail!(
                    "framework DDL mirror disagrees with SQL: {}.{} metadata nullable={} but full_def says {}NOT NULL (full_def='{}')",
                    t.name,
                    c.name,
                    c.nullable,
                    if sql_not_null { "" } else { "no " },
                    c.full_def
                );
            }
            if sql_has_default != c.default_sql.is_some() {
                anyhow::bail!(
                    "framework DDL mirror disagrees with SQL: {}.{} metadata default_sql={:?} but full_def {} DEFAULT (full_def='{}')",
                    t.name,
                    c.name,
                    c.default_sql,
                    if sql_has_default { "has" } else { "lacks" },
                    c.full_def
                );
            }
            // Ground the gate in the SQL text, not the metadata flag.
            if sql_not_null && !sql_has_default {
                anyhow::bail!(
                    "framework DDL invariant violated: {}.{} is NOT NULL without DEFAULT in full_def (would fail on ALTER TABLE ADD COLUMN against an existing non-empty DB; full_def='{}')",
                    t.name,
                    c.name,
                    c.full_def
                );
            }
        }
    }
    Ok(())
}

/// Same as `validate_framework_tables(FRAMEWORK_DDL_TABLES)`. Called from
/// `db::open` so the invariant is enforced fail-loud at boot.
pub fn validate_framework_ddl() -> anyhow::Result<()> {
    validate_framework_tables(FRAMEWORK_DDL_TABLES)
}

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
    occurred_at TEXT NOT NULL,
    actor_note TEXT
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
    daemon_epoch TEXT,
    claim_source TEXT CHECK(claim_source IN ('try_claim','retry_claim','manual','legacy')),
    attempt INTEGER,
    pid INTEGER,
    heartbeat_at TEXT,
    postcondition_id TEXT,
    postcondition_args TEXT,
    terminal_reason TEXT CHECK(terminal_reason IN ('ok','exit_nonzero','error','silent_zombie','timeout','halted','legacy_unknown')),
    next_retry_at TEXT,
    UNIQUE(store, row_id, agent_name)
);
CREATE TABLE IF NOT EXISTS substrate_migrations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    applied_at TEXT NOT NULL,
    binary_version TEXT NOT NULL,
    table_name TEXT NOT NULL,
    column_name TEXT NOT NULL,
    ddl_applied TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS framework_migrations (
    id TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL,
    note TEXT
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

/// Render the SQL fragment ` DEFAULT '<val>'` for a field's declared default,
/// or `None` if the field has no default. Quoting strategy:
/// - JSON null → `DEFAULT NULL` (no value materialised; equivalent to absent)
/// - JSON string → SQL string literal with single-quote doubling
/// - JSON number/bool → SQL literal (numbers as-is, bool → 0/1)
/// - JSON array/object → JSON-encoded, wrapped in single-quote SQL literal
///   (intent: `DEFAULT '[]'` for list:text fields with `default: '[]'`).
/// (T052 P1)
pub(crate) fn default_clause(field: &Field) -> Option<String> {
    let v = field.default.as_ref()?;
    let lit = match v {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("'{}'", s.replace('\'', "''")),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            let json = serde_json::to_string(v).unwrap_or_else(|_| "null".to_string());
            format!("'{}'", json.replace('\'', "''"))
        }
    };
    Some(format!(" DEFAULT {lit}"))
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
                let mut full_def = format!("{} TEXT", field.name);
                if let Some(suffix) = default_clause(field) {
                    full_def.push_str(&suffix);
                }
                json_cols.push(ExpectedColumn {
                    name: field.name.clone(),
                    sql_type: "TEXT".to_string(),
                    full_def,
                    is_reserved: false,
                });
            }
            ty => {
                if let Some(mut def) = scalar_col_def(&field.name, ty) {
                    let sql_type = match ty {
                        FieldType::Integer | FieldType::Bool => "INTEGER",
                        _ => "TEXT",
                    }
                    .to_string();
                    if let Some(suffix) = default_clause(field) {
                        def.push_str(&suffix);
                    }
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

    // ---- framework_ tests (T051 Phase 1) ----

    /// Parse a column-name set per CREATE TABLE block out of SUBSTRATE_DDL
    /// using a simple line scanner (no full SQL parser).
    fn scan_substrate_ddl_columns() -> std::collections::HashMap<String, Vec<String>> {
        let mut out: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut current_table: Option<String> = None;
        for raw in SUBSTRATE_DDL.lines() {
            let line = raw.trim();
            if let Some(rest) = line.strip_prefix("CREATE TABLE IF NOT EXISTS ") {
                let name = rest.trim_end_matches('(').trim().to_string();
                current_table = Some(name);
                continue;
            }
            if line.starts_with(')') {
                current_table = None;
                continue;
            }
            if let Some(table) = &current_table {
                if line.is_empty() {
                    continue;
                }
                // A column line begins with an identifier; UNIQUE(...) is
                // a table constraint we skip.
                if line.starts_with("UNIQUE") {
                    continue;
                }
                let name = line.split_whitespace().next().unwrap_or("").to_string();
                if name.is_empty() {
                    continue;
                }
                out.entry(table.clone()).or_default().push(name);
            }
        }
        out
    }

    #[test]
    fn framework_tables_match_substrate_ddl() {
        let scanned = scan_substrate_ddl_columns();
        assert_eq!(
            scanned.len(),
            FRAMEWORK_DDL_TABLES.len(),
            "FRAMEWORK_DDL_TABLES table count {} != SUBSTRATE_DDL table count {} (scanned: {:?})",
            FRAMEWORK_DDL_TABLES.len(),
            scanned.len(),
            scanned.keys().collect::<Vec<_>>()
        );
        for t in FRAMEWORK_DDL_TABLES {
            let scanned_cols = scanned.get(t.name).unwrap_or_else(|| {
                panic!("table {} declared in FRAMEWORK_DDL_TABLES but not in SUBSTRATE_DDL", t.name)
            });
            let const_cols: Vec<String> = t.columns.iter().map(|c| c.name.to_string()).collect();
            let scanned_set: std::collections::BTreeSet<&str> =
                scanned_cols.iter().map(String::as_str).collect();
            let const_set: std::collections::BTreeSet<&str> =
                const_cols.iter().map(String::as_str).collect();
            assert_eq!(
                scanned_set, const_set,
                "column-name drift in table {}: SUBSTRATE_DDL={:?}, FRAMEWORK_DDL_TABLES={:?}",
                t.name, scanned_cols, const_cols
            );
        }
    }

    #[test]
    fn framework_ddl_validates_clean() {
        validate_framework_ddl().expect("production SUBSTRATE_DDL must validate");
    }

    #[test]
    fn framework_ddl_validator_rejects_nonnullable_no_default() {
        let bad = &[FrameworkTable {
            name: "evil_table",
            columns: &[FrameworkColumn {
                name: "evil_col",
                sql_type: "TEXT",
                nullable: false,
                default_sql: None,
                full_def: "evil_col TEXT NOT NULL",
                additive: true,
            }],
        }];
        let err = validate_framework_tables(bad).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("evil_table"), "msg: {msg}");
        assert!(msg.contains("evil_col"), "msg: {msg}");
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
