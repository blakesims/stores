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

/// Mirror of framework-owned DDL for column-level introspection. Every
/// SUBSTRATE_DDL table here MUST list every column declared in SUBSTRATE_DDL;
/// store-schema-owned tables may appear here only for additive framework-owned
/// columns that must be repaired before framework subscribers run.
pub const FRAMEWORK_DDL_TABLES: &[FrameworkTable] = &[
    FrameworkTable {
        name: "observations",
        columns: &[
            // T099: framework-written auto-file cascade dedup metadata. The
            // observations table itself is store-schema-owned, but these three
            // columns are required before framework `user-escalation` can run
            // its dedup SELECT/UPDATE safely on older DBs.
            FrameworkColumn { name: "summary_signature", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "summary_signature TEXT", additive: true },
            FrameworkColumn { name: "dupe_count", sql_type: "INTEGER", nullable: true, default_sql: Some("1"), full_def: "dupe_count INTEGER DEFAULT 1", additive: true },
            FrameworkColumn { name: "last_seen", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "last_seen TEXT", additive: true },
        ],
    },
    FrameworkTable {
        name: "tasks",
        columns: &[
            // T140 P1: per-row activation gate. The tasks table itself is
            // store-schema-owned, but `activation` is registered here as an
            // additive FrameworkColumn so older DBs (no activation column) get
            // ALTERed up by `apply_framework_drift`. NOT NULL DEFAULT 'inactive'
            // — additive-safe via the DEFAULT. Backfill of currently-running
            // rows to 'active' is handled in `framework_migrate::backfill_tasks_activation`.
            FrameworkColumn {
                name: "activation",
                sql_type: "TEXT",
                nullable: false,
                default_sql: Some("'inactive'"),
                full_def: "activation TEXT NOT NULL DEFAULT 'inactive' CHECK(activation IN ('active','inactive'))",
                additive: true,
            },
            FrameworkColumn {
                name: "lifecycle",
                sql_type: "TEXT",
                nullable: false,
                default_sql: Some("'queued'"),
                full_def: "lifecycle TEXT NOT NULL DEFAULT 'queued' CHECK(lifecycle IN ('queued','active','integration','done'))",
                additive: true,
            },
            FrameworkColumn {
                name: "active_step",
                sql_type: "TEXT",
                nullable: false,
                default_sql: Some("'none'"),
                full_def: "active_step TEXT NOT NULL DEFAULT 'none' CHECK(active_step IN ('none','planning','planning_review','coding','coding_review','wrapping'))",
                additive: true,
            },
            FrameworkColumn {
                name: "integration_step",
                sql_type: "TEXT",
                nullable: false,
                default_sql: Some("'none'"),
                full_def: "integration_step TEXT NOT NULL DEFAULT 'none' CHECK(integration_step IN ('none','queued','refreshing','task_review','testing','merging','deploying','verifying'))",
                additive: true,
            },
            FrameworkColumn {
                name: "blocked",
                sql_type: "INTEGER",
                nullable: false,
                default_sql: Some("0"),
                full_def: "blocked INTEGER NOT NULL DEFAULT 0 CHECK(blocked IN (0,1))",
                additive: true,
            },
            FrameworkColumn {
                name: "blocker_kind",
                sql_type: "TEXT",
                nullable: true,
                default_sql: None,
                full_def: "blocker_kind TEXT CHECK(blocker_kind IN ('capacity','dependency','runner','rate_limit','human_acceptance','task_review','stale_base','config','test_failure','main_red','deploy','migration'))",
                additive: true,
            },
            FrameworkColumn {
                name: "post_integration_step",
                sql_type: "TEXT",
                nullable: false,
                default_sql: Some("'none'"),
                full_def: "post_integration_step TEXT NOT NULL DEFAULT 'none' CHECK(post_integration_step IN ('none','cargo_installed','schema_migrated','deploy_blocked','deploy_verified'))",
                additive: true,
            },
            FrameworkColumn {
                name: "human_acceptance_policy",
                sql_type: "TEXT",
                nullable: false,
                default_sql: Some("'optional'"),
                full_def: "human_acceptance_policy TEXT NOT NULL DEFAULT 'optional' CHECK(human_acceptance_policy IN ('required','optional','delegated_by_policy'))",
                additive: true,
            },
            FrameworkColumn {
                name: "task_review_policy",
                sql_type: "TEXT",
                nullable: false,
                default_sql: Some("'none'"),
                full_def: "task_review_policy TEXT NOT NULL DEFAULT 'none' CHECK(task_review_policy IN ('none','advisory','authoritative','both'))",
                additive: true,
            },
            FrameworkColumn {
                name: "acceptance_decided_by",
                sql_type: "TEXT",
                nullable: true,
                default_sql: None,
                full_def: "acceptance_decided_by TEXT CHECK(acceptance_decided_by IN ('human','policy_delegate'))",
                additive: true,
            },
            FrameworkColumn { name: "acceptance_decided_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "acceptance_decided_at TEXT", additive: true },
        ],
    },
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
            FrameworkColumn { name: "lifecycle_from", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "lifecycle_from TEXT", additive: true },
            FrameworkColumn { name: "active_step_from", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "active_step_from TEXT", additive: true },
            FrameworkColumn { name: "integration_step_from", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "integration_step_from TEXT", additive: true },
            FrameworkColumn { name: "lifecycle_to", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "lifecycle_to TEXT", additive: true },
            FrameworkColumn { name: "active_step_to", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "active_step_to TEXT", additive: true },
            FrameworkColumn { name: "integration_step_to", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "integration_step_to TEXT", additive: true },
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
            FrameworkColumn { name: "terminal_reason", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "terminal_reason TEXT CHECK(terminal_reason IN ('ok','exit_nonzero','error','silent_zombie','timeout','halted','legacy_unknown','rate_limit'))", additive: true },
            FrameworkColumn { name: "next_retry_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "next_retry_at TEXT", additive: true },
        ],
    },
    FrameworkTable {
        name: "resource_locks",
        columns: &[
            FrameworkColumn { name: "resource_id", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "resource_id TEXT PRIMARY KEY", additive: false },
            FrameworkColumn { name: "owner_kind", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "owner_kind TEXT NOT NULL CHECK(owner_kind IN ('task','job'))", additive: false },
            FrameworkColumn { name: "owner_display_id", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "owner_display_id TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "fencing_token", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "fencing_token TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "acquired_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "acquired_at TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "heartbeat_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "heartbeat_at TEXT", additive: false },
            FrameworkColumn { name: "expires_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "expires_at TEXT", additive: false },
            FrameworkColumn { name: "daemon_epoch", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "daemon_epoch TEXT", additive: false },
            FrameworkColumn { name: "claim_source", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "claim_source TEXT", additive: false },
        ],
    },
    FrameworkTable {
        name: "daemon_starts",
        columns: &[
            FrameworkColumn { name: "id", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "id INTEGER PRIMARY KEY AUTOINCREMENT", additive: false },
            FrameworkColumn { name: "display_id", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "display_id TEXT UNIQUE NOT NULL", additive: false },
            FrameworkColumn { name: "status", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "status TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "created_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "created_at TEXT", additive: false },
            FrameworkColumn { name: "updated_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "updated_at TEXT", additive: false },
            FrameworkColumn { name: "created_by", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "created_by TEXT", additive: false },
            FrameworkColumn { name: "updated_by", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "updated_by TEXT", additive: false },
            FrameworkColumn { name: "pid", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "pid INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "started_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "started_at TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "binary_path", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "binary_path TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "binary_version", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "binary_version TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "git_sha", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "git_sha TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "argv", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "argv TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "log_file", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "log_file TEXT", additive: false },
            FrameworkColumn { name: "cwd", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "cwd TEXT NOT NULL", additive: false },
        ],
    },
    FrameworkTable {
        name: "agent_runs",
        columns: &[
            FrameworkColumn { name: "id", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "id INTEGER PRIMARY KEY AUTOINCREMENT", additive: false },
            FrameworkColumn { name: "display_id", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "display_id TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "phase", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "phase INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "cycle", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "cycle INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "role", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "role TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "model_id", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "model_id TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "harness_id", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "harness_id TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "started_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "started_at TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "ended_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "ended_at TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "exit_code", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "exit_code INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "tokens_in", sql_type: "INTEGER", nullable: true, default_sql: None, full_def: "tokens_in INTEGER", additive: false },
            FrameworkColumn { name: "tokens_out", sql_type: "INTEGER", nullable: true, default_sql: None, full_def: "tokens_out INTEGER", additive: false },
            FrameworkColumn { name: "prompt_cache_hits", sql_type: "INTEGER", nullable: true, default_sql: None, full_def: "prompt_cache_hits INTEGER", additive: false },
            FrameworkColumn { name: "transcript_path", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "transcript_path TEXT NOT NULL", additive: false },
            // L503-A: brief_text persists the rendered brief at dispatch time for observability.
            // Nullable so existing rows (and pre-L503 DBs) retain NULL without errors.
            FrameworkColumn { name: "brief_text", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "brief_text TEXT", additive: true },
            // Runner telemetry expansion: nullable additive columns so existing DBs migrate safely.
            FrameworkColumn { name: "configured_harness_id", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "configured_harness_id TEXT", additive: true },
            FrameworkColumn { name: "configured_model_id", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "configured_model_id TEXT", additive: true },
            FrameworkColumn { name: "configured_thinking_effort", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "configured_thinking_effort TEXT", additive: true },
            FrameworkColumn { name: "effective_model_id", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "effective_model_id TEXT", additive: true },
            FrameworkColumn { name: "effective_thinking_effort", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "effective_thinking_effort TEXT", additive: true },
            FrameworkColumn { name: "thinking_effort_source", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "thinking_effort_source TEXT", additive: true },
            FrameworkColumn { name: "provider_id", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "provider_id TEXT", additive: true },
            FrameworkColumn { name: "api_id", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "api_id TEXT", additive: true },
            FrameworkColumn { name: "session_id", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "session_id TEXT", additive: true },
            FrameworkColumn { name: "workspace_path", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "workspace_path TEXT", additive: true },
            FrameworkColumn { name: "runner_exit_kind", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "runner_exit_kind TEXT", additive: true },
            FrameworkColumn { name: "payload_valid", sql_type: "INTEGER", nullable: true, default_sql: None, full_def: "payload_valid INTEGER", additive: true },
            FrameworkColumn { name: "payload_error", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "payload_error TEXT", additive: true },
            FrameworkColumn { name: "cache_read_tokens", sql_type: "INTEGER", nullable: true, default_sql: None, full_def: "cache_read_tokens INTEGER", additive: true },
            FrameworkColumn { name: "cache_write_tokens", sql_type: "INTEGER", nullable: true, default_sql: None, full_def: "cache_write_tokens INTEGER", additive: true },
            FrameworkColumn { name: "cost_total", sql_type: "REAL", nullable: true, default_sql: None, full_def: "cost_total REAL", additive: true },
        ],
    },
    FrameworkTable {
        name: "engine_runner_heartbeats",
        columns: &[
            FrameworkColumn { name: "iteration", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "iteration INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "started_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "started_at TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "saw_tasks", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "saw_tasks INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "saw_intake", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "saw_intake INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "saw_observations", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "saw_observations INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "actionable", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "actionable INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "held", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "held INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "dispatched", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "dispatched INTEGER NOT NULL", additive: false },
        ],
    },
    FrameworkTable {
        name: "engine_runner_actions",
        columns: &[
            FrameworkColumn { name: "store", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "store TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "row_id", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "row_id INTEGER NOT NULL", additive: false },
            FrameworkColumn { name: "classification", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "classification TEXT NOT NULL", additive: false },
            FrameworkColumn { name: "action", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "action TEXT", additive: true },
            FrameworkColumn { name: "held_reason", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "held_reason TEXT", additive: false },
            FrameworkColumn { name: "dispatched", sql_type: "INTEGER", nullable: false, default_sql: None, full_def: "dispatched INTEGER NOT NULL CHECK(dispatched IN (0,1))", additive: false },
            FrameworkColumn { name: "last_logged_at", sql_type: "TEXT", nullable: true, default_sql: None, full_def: "last_logged_at TEXT", additive: false },
            FrameworkColumn { name: "updated_at", sql_type: "TEXT", nullable: false, default_sql: None, full_def: "updated_at TEXT NOT NULL", additive: false },
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
    actor_note TEXT,
    lifecycle_from TEXT,
    active_step_from TEXT,
    integration_step_from TEXT,
    lifecycle_to TEXT,
    active_step_to TEXT,
    integration_step_to TEXT
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
    terminal_reason TEXT CHECK(terminal_reason IN ('ok','exit_nonzero','error','silent_zombie','timeout','halted','legacy_unknown','rate_limit')),
    next_retry_at TEXT,
    UNIQUE(store, row_id, agent_name)
);
CREATE TABLE IF NOT EXISTS resource_locks (
    resource_id TEXT PRIMARY KEY,
    owner_kind TEXT NOT NULL CHECK(owner_kind IN ('task','job')),
    owner_display_id TEXT NOT NULL,
    fencing_token TEXT NOT NULL,
    acquired_at TEXT NOT NULL,
    heartbeat_at TEXT,
    expires_at TEXT,
    daemon_epoch TEXT,
    claim_source TEXT
);
CREATE TABLE IF NOT EXISTS daemon_starts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    display_id TEXT UNIQUE NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT,
    updated_at TEXT,
    created_by TEXT,
    updated_by TEXT,
    pid INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    binary_path TEXT NOT NULL,
    binary_version TEXT NOT NULL,
    git_sha TEXT NOT NULL,
    argv TEXT NOT NULL,
    log_file TEXT,
    cwd TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS agent_runs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    display_id TEXT NOT NULL,
    phase INTEGER NOT NULL,
    cycle INTEGER NOT NULL,
    role TEXT NOT NULL,
    model_id TEXT NOT NULL,
    harness_id TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT NOT NULL,
    exit_code INTEGER NOT NULL,
    tokens_in INTEGER,
    tokens_out INTEGER,
    prompt_cache_hits INTEGER,
    transcript_path TEXT NOT NULL,
    brief_text TEXT,
    configured_harness_id TEXT,
    configured_model_id TEXT,
    configured_thinking_effort TEXT,
    effective_model_id TEXT,
    effective_thinking_effort TEXT,
    thinking_effort_source TEXT,
    provider_id TEXT,
    api_id TEXT,
    session_id TEXT,
    workspace_path TEXT,
    runner_exit_kind TEXT,
    payload_valid INTEGER,
    payload_error TEXT,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER,
    cost_total REAL
);
CREATE TABLE IF NOT EXISTS engine_runner_heartbeats (
    iteration INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    saw_tasks INTEGER NOT NULL,
    saw_intake INTEGER NOT NULL,
    saw_observations INTEGER NOT NULL,
    actionable INTEGER NOT NULL,
    held INTEGER NOT NULL,
    dispatched INTEGER NOT NULL,
    PRIMARY KEY(iteration, started_at)
);
CREATE TABLE IF NOT EXISTS engine_runner_actions (
    store TEXT NOT NULL,
    row_id INTEGER NOT NULL,
    classification TEXT NOT NULL,
    action TEXT,
    held_reason TEXT,
    dispatched INTEGER NOT NULL CHECK(dispatched IN (0,1)),
    last_logged_at TEXT,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(store, row_id)
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

/// Virtual read-only VIEW over tasks.cycles JSON.  Applied by `db::open` on
/// every connection so the query surface is always available.  Uses SQLite
/// JSON1 functions (json_each / json_extract) to flatten the cycles JSON array
/// into one row per (display_id, phase, cycle, role).  Two UNION ALL branches:
/// one for the executor sub-record, one for the review sub-record; rows whose
/// sub-record or transcript_path is absent are filtered by the WHERE clause.
/// Per T072 L059 architecture decision (pi msg_32b0e4c4): SQL VIEW, not a
/// materialized table.
pub const RUNS_VIEW_DDL: &str = "\
CREATE VIEW IF NOT EXISTS runs AS
  SELECT
    t.display_id,
    CAST(json_extract(c.value, '$.phase') AS INTEGER) AS phase,
    CAST(json_extract(c.value, '$.cycle') AS INTEGER) AS cycle,
    'executor' AS role,
    json_extract(c.value, '$.executor.transcript_path') AS transcript_path
  FROM tasks t, json_each(t.cycles) c
  WHERE json_extract(c.value, '$.executor.transcript_path') IS NOT NULL
UNION ALL
  SELECT
    t.display_id,
    CAST(json_extract(c.value, '$.phase') AS INTEGER) AS phase,
    CAST(json_extract(c.value, '$.cycle') AS INTEGER) AS cycle,
    'code-reviewer' AS role,
    json_extract(c.value, '$.review.transcript_path') AS transcript_path
  FROM tasks t, json_each(t.cycles) c
  WHERE json_extract(c.value, '$.review.transcript_path') IS NOT NULL;
";

/// Read-only projection from runner telemetry to direct downstream outputs.
///
/// Phase 5 of the runner telemetry plan requires a stable primary join key before
/// outcome projection.  The current schema provides that key only for executor
/// and code-reviewer outputs: drive writes `agent_runs.session_id` and embeds the
/// deterministic `.stores/runs/<session_id>.jsonl` transcript backlink into the
/// committed `tasks.cycles` sub-record in the same submit transaction. Planner
/// and plan-reviewer outputs do not yet have an equivalent downstream backlink,
/// so this view intentionally excludes them rather than guessing by timestamp.
pub const RUNNER_OUTCOMES_VIEW_DDL: &str = "\
CREATE VIEW IF NOT EXISTS runner_outcomes AS
WITH cycle_outputs AS (
  SELECT
    t.display_id,
    CAST(json_extract(c.value, '$.phase') AS INTEGER) AS phase,
    CAST(json_extract(c.value, '$.cycle') AS INTEGER) AS cycle,
    'executor' AS role,
    json_extract(c.value, '$.executor.transcript_path') AS transcript_path,
    'submitted_execution' AS outcome_kind,
    NULL AS gate,
    json_extract(c.value, '$.executor.summary') AS summary,
    json_extract(c.value, '$.executor.commit') AS commit_sha,
    NULL AS critical,
    NULL AS major,
    NULL AS minor
  FROM tasks t, json_each(t.cycles) c
  WHERE json_extract(c.value, '$.executor.transcript_path') IS NOT NULL
UNION ALL
  SELECT
    t.display_id,
    CAST(json_extract(c.value, '$.phase') AS INTEGER) AS phase,
    CAST(json_extract(c.value, '$.cycle') AS INTEGER) AS cycle,
    'code_reviewer' AS role,
    json_extract(c.value, '$.review.transcript_path') AS transcript_path,
    'submitted_code_review' AS outcome_kind,
    json_extract(c.value, '$.review.gate') AS gate,
    json_extract(c.value, '$.review.summary') AS summary,
    NULL AS commit_sha,
    CAST(json_extract(c.value, '$.review.critical') AS INTEGER) AS critical,
    CAST(json_extract(c.value, '$.review.major') AS INTEGER) AS major,
    CAST(json_extract(c.value, '$.review.minor') AS INTEGER) AS minor
  FROM tasks t, json_each(t.cycles) c
  WHERE json_extract(c.value, '$.review.transcript_path') IS NOT NULL
)
SELECT
  ar.id AS agent_run_id,
  ar.display_id,
  ar.phase,
  ar.cycle,
  ar.role,
  ar.harness_id,
  ar.model_id,
  ar.configured_harness_id,
  ar.configured_model_id,
  ar.configured_thinking_effort,
  ar.effective_model_id,
  ar.effective_thinking_effort,
  ar.session_id,
  co.transcript_path AS downstream_transcript_path,
  ar.transcript_path AS agent_run_transcript_path,
  co.outcome_kind,
  co.gate,
  co.summary,
  co.commit_sha,
  co.critical,
  co.major,
  co.minor
FROM agent_runs ar
JOIN cycle_outputs co
  ON co.display_id = ar.display_id
 AND co.phase = ar.phase
 AND co.cycle = ar.cycle
 AND co.role = CASE WHEN ar.role = 'code-reviewer' THEN 'code_reviewer' ELSE ar.role END
 AND ar.session_id IS NOT NULL
 AND co.transcript_path = '.stores/runs/' || ar.session_id || '.jsonl';
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
///
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
                    // T107: observations.cluster_key gets a registry-derived CHECK
                    // constraint so SQLite enforces the allowed-list at the DB level.
                    // CLUSTER_REGISTRY in cluster_keys.rs is the single source of truth;
                    // the CHECK clause is generated from that registry, not schema.yaml.
                    if schema.name == "observations" && field.name == "cluster_key" {
                        let check = crate::handlers::cluster_keys::check_clause_sql();
                        def.push_str(&format!(" {check}"));
                    }
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
                // A column line begins with an identifier; table constraints
                // are skipped.
                if line.starts_with("UNIQUE") || line.starts_with("PRIMARY KEY") {
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
    fn daemon_starts_schema_contract_matches_expected_fields() {
        let yaml = include_str!("../../stores/daemon_starts/schema.yaml");
        let schema = Schema::from_yaml(yaml).expect("daemon_starts schema parses");
        assert_eq!(schema.name, "daemon_starts");
        let fields: std::collections::BTreeMap<&str, &FieldType> = schema
            .fields
            .iter()
            .map(|f| (f.name.as_str(), &f.ty))
            .collect();
        for (name, ty) in [
            ("pid", FieldType::Integer),
            ("started_at", FieldType::Timestamp),
            ("binary_path", FieldType::Text),
            ("binary_version", FieldType::Text),
            ("git_sha", FieldType::Text),
            ("argv", FieldType::Text),
            ("log_file", FieldType::Text),
            ("cwd", FieldType::Text),
        ] {
            assert_eq!(fields.get(name).copied(), Some(&ty), "field {name}");
        }
        assert!(
            !fields.contains_key("daemon_epoch"),
            "daemon_epoch must be absent from daemon_starts schema"
        );
    }

    #[test]
    fn framework_tables_match_substrate_ddl() {
        let scanned = scan_substrate_ddl_columns();
        let substrate_backed = FRAMEWORK_DDL_TABLES
            .iter()
            .filter(|t| scanned.contains_key(t.name))
            .count();
        assert_eq!(
            scanned.len(),
            substrate_backed,
            "SUBSTRATE_DDL table count {} != substrate-backed FRAMEWORK_DDL_TABLES count {} (scanned: {:?})",
            scanned.len(),
            substrate_backed,
            scanned.keys().collect::<Vec<_>>()
        );
        for t in FRAMEWORK_DDL_TABLES {
            let Some(scanned_cols) = scanned.get(t.name) else {
                assert!(
                    t.columns.iter().all(|c| c.additive),
                    "non-additive table {} declared in FRAMEWORK_DDL_TABLES but not in SUBSTRATE_DDL",
                    t.name
                );
                continue;
            };
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

    // ---- T052 P1: per-field DEFAULT clauses ----

    /// AC1.5 / Task 1.6 (a): DDL emits DEFAULT for the four risk taxonomy
    /// columns on the bundled observations schema.
    #[test]
    fn t052_p1_observations_ddl_emits_default_clauses_for_risk_taxonomy() {
        let yaml = include_str!("../../stores/observations/schema.yaml");
        let schema = Schema::from_yaml(yaml).expect("observations schema must parse");
        let ddl = ddl_for(&schema);

        // risk_class: enum with CHECK + DEFAULT 'normal'
        assert!(
            ddl.contains("risk_class TEXT CHECK (risk_class IN ('low', 'normal', 'architecture', 'security', 'authority')) DEFAULT 'normal'"),
            "risk_class DDL missing CHECK + DEFAULT 'normal':\n{ddl}"
        );

        // approval_policy: enum CHECK + DEFAULT 'human'
        assert!(
            ddl.contains("approval_policy TEXT CHECK (approval_policy IN ('auto', 'human', 'architecture')) DEFAULT 'human'"),
            "approval_policy DDL missing CHECK + DEFAULT 'human':\n{ddl}"
        );

        // risk_flags: list:text → JSON TEXT with DEFAULT '[]'
        assert!(
            ddl.contains("risk_flags TEXT DEFAULT '[]'"),
            "risk_flags DDL missing DEFAULT '[]':\n{ddl}"
        );

        // cluster_key: TEXT with registry CHECK constraint, no DEFAULT
        let cluster_line = ddl
            .lines()
            .find(|l| l.trim_start().starts_with("cluster_key"))
            .expect("cluster_key line present");
        assert!(
            cluster_line.contains("CHECK (cluster_key IN ("),
            "cluster_key DDL must contain CHECK clause: {cluster_line}"
        );
        assert!(
            !cluster_line.contains("DEFAULT"),
            "cluster_key must not carry a DEFAULT clause: {cluster_line}"
        );
    }

    /// T107: cluster_key CHECK constraint in DDL lists every entry in
    /// CLUSTER_REGISTRY exactly.
    #[test]
    fn cluster_key_ddl_check_constraint_lists_all_curated_keys() {
        use crate::handlers::cluster_keys::curated_cluster_keys;
        let yaml = include_str!("../../stores/observations/schema.yaml");
        let schema = Schema::from_yaml(yaml).expect("observations schema must parse");
        let cols = expected_columns(&schema);
        let cluster_col = cols
            .iter()
            .find(|c| c.name == "cluster_key")
            .expect("cluster_key column present");
        for key in curated_cluster_keys() {
            assert!(
                cluster_col.full_def.contains(key),
                "cluster_key full_def must contain registry key '{key}': {}",
                cluster_col.full_def
            );
        }
        // Also verify CHECK syntax
        assert!(
            cluster_col.full_def.contains("CHECK (cluster_key IN ("),
            "cluster_key full_def must contain CHECK clause: {}",
            cluster_col.full_def
        );
    }

    /// Task 1.6 (a): scalar DEFAULT clause is emitted via expected_columns.full_def.
    #[test]
    fn t052_p1_default_clause_in_expected_columns_full_def() {
        let yaml = include_str!("../../stores/observations/schema.yaml");
        let schema = Schema::from_yaml(yaml).unwrap();
        let cols = expected_columns(&schema);
        let risk_class = cols
            .iter()
            .find(|c| c.name == "risk_class")
            .expect("risk_class column");
        assert!(
            risk_class.full_def.contains("DEFAULT 'normal'"),
            "risk_class.full_def missing DEFAULT clause: {}",
            risk_class.full_def
        );
        let risk_flags = cols
            .iter()
            .find(|c| c.name == "risk_flags")
            .expect("risk_flags column");
        assert!(
            risk_flags.full_def.contains("DEFAULT '[]'"),
            "risk_flags.full_def missing DEFAULT '[]': {}",
            risk_flags.full_def
        );
        let cluster_key = cols
            .iter()
            .find(|c| c.name == "cluster_key")
            .expect("cluster_key column");
        assert!(
            !cluster_key.full_def.contains("DEFAULT"),
            "cluster_key must not have DEFAULT: {}",
            cluster_key.full_def
        );
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
