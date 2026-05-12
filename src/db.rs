use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction};

use crate::runner::AgentRunTelemetry;
use std::path::Path;

use crate::codegen::ddl::{
    validate_framework_ddl, RUNNER_OUTCOMES_VIEW_DDL, RUNS_VIEW_DDL, SUBSTRATE_DDL,
};
use crate::handlers::framework_migrate::apply_framework_drift;

/// Env var: when set to "1", `db::open` validates SUBSTRATE_DDL but skips
/// the framework-drift auto-apply pass. Used by operators who want explicit
/// control over `stores migrate` runs.
const DISABLE_AUTOAPPLY_ENV: &str = "STORES_DISABLE_FRAMEWORK_AUTOAPPLY";

/// Apply task-backed read-only VIEW DDL only when the `tasks` table exists in the DB.
///
/// SQLite recompiles all views on every DDL statement; a view that references
/// a missing base table causes every subsequent DDL (even on unrelated tables)
/// to fail with "error in view runs: no such table: main.tasks".  The tasks
/// store is installed separately from the substrate tables (via `stores install
/// tasks`), so this guard lets connections that only have the substrate tables
/// (e.g. a DB with only `observations` installed) proceed without the VIEWs.
///
/// When tasks is later installed (or already exists), the VIEWs are created
/// idempotently.  T072 / L059; runner_outcomes phase-5 telemetry projection.
pub(crate) fn ensure_runs_view_if_tasks_exists(conn: &Connection) -> Result<()> {
    let tasks_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if tasks_exists {
        conn.execute_batch(RUNS_VIEW_DDL)
            .context("apply runs view DDL")?;
        conn.execute_batch(RUNNER_OUTCOMES_VIEW_DDL)
            .context("apply runner_outcomes view DDL")?;
    }
    Ok(())
}

fn open_inner(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // Boot-time invariant check on the compiled-in SUBSTRATE_DDL: refuse to
    // start if any additive framework column would fail an ALTER on an
    // existing non-empty DB.
    validate_framework_ddl().context("framework DDL invariant check")?;
    // Substrate-level tables (store-agnostic). Idempotent (CREATE IF NOT EXISTS).
    conn.execute_batch(SUBSTRATE_DDL)
        .context("apply substrate DDL")?;
    // T072 / L059: runs VIEW over tasks.cycles JSON.  Only created when the
    // tasks table exists — SQLite schema recompilation during later DDL (e.g.
    // ALTER TABLE on unrelated tables) fails if a view references a missing
    // base table.  The tasks store is installed separately from the substrate
    // tables, so we guard the CREATE VIEW with a table-existence check.
    ensure_runs_view_if_tasks_exists(&conn).context("apply runs view DDL")?;
    // L134 / T050 Phase 1: typed-buffer migration + legacy backfill.
    // Idempotent; safe to run on every CLI verb that opens the DB.
    crate::handlers::agents_run::ensure_dispatch_locks_typed(&conn)
        .context("L134: ensure typed dispatch_locks columns")?;
    crate::handlers::agents_run::backfill_legacy_locks(&conn)
        .context("L134: backfill legacy dispatch_locks rows")?;
    Ok(conn)
}

/// Open the substrate DB and (unless `STORES_DISABLE_FRAMEWORK_AUTOAPPLY=1`)
/// auto-apply any framework-DDL drift on existing tables.
///
/// Boot path is silent: the apply report is discarded. Operators who want to
/// see what was applied should run `stores migrate` (which uses
/// `open_no_autoapply` so it can compute the diff before any mutation).
pub fn open(path: &Path) -> Result<Connection> {
    let conn = open_inner(path)?;
    let disabled = std::env::var(DISABLE_AUTOAPPLY_ENV)
        .ok()
        .filter(|v| v == "1")
        .is_some();
    if !disabled {
        apply_framework_drift(&conn).context("auto-apply framework DDL drift on boot")?;
        crate::handlers::migrate::repair_external_reviews_runner_fake_check(&conn)
            .context("auto-repair external_reviews.runner CHECK for fake runner")?;
    }
    Ok(conn)
}

/// Open the DB without running framework-DDL auto-apply. Used by
/// `stores migrate` so it can observe drift before applying it.
pub fn open_no_autoapply(path: &Path) -> Result<Connection> {
    open_inner(path)
}

/// Insert a row into `transition_history`. Called by every successful
/// lifecycle transition write (manual or policy-mediated).
///
/// `policy_ref` / `policies_hash` are `None` for manual transitions; the
/// autonomous flow daemon fills them when it dispatches policy-mediated writes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_agent_run(
    conn: &Connection,
    display_id: &str,
    phase: i64,
    cycle: i64,
    role: &str,
    exit_code: i32,
    telemetry: &AgentRunTelemetry,
    brief_text: Option<&str>,
) -> Result<()> {
    // All required telemetry fields must be supplied by the caller. The
    // `legacy_unknown` sentinel is migration/backfill-only — callers doing
    // historical backfill must pass it explicitly. New invocations that
    // genuinely lack telemetry (runner failed before producing any output)
    // must use a separate error shape (RunnerOutput::payload_error) rather
    // than smuggling missing values through these fields.
    let model_id = telemetry
        .model_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("insert_agent_run: model_id is required for new rows; supply it from the runner or use 'legacy_unknown' explicitly for backfill"))?
        .to_string();
    anyhow::ensure!(
        !model_id.is_empty(),
        "insert_agent_run: model_id must be non-empty"
    );
    let harness_id = telemetry
        .harness_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("insert_agent_run: harness_id is required for new rows"))?;
    anyhow::ensure!(
        !harness_id.is_empty(),
        "insert_agent_run: harness_id must be non-empty"
    );
    let started_at = telemetry
        .started_at
        .clone()
        .ok_or_else(|| anyhow::anyhow!("insert_agent_run: started_at is required for new rows"))?;
    anyhow::ensure!(
        !started_at.is_empty(),
        "insert_agent_run: started_at must be non-empty"
    );
    let ended_at = telemetry
        .ended_at
        .clone()
        .ok_or_else(|| anyhow::anyhow!("insert_agent_run: ended_at is required for new rows"))?;
    anyhow::ensure!(
        !ended_at.is_empty(),
        "insert_agent_run: ended_at must be non-empty"
    );
    let transcript_path = telemetry
        .transcript_path
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("insert_agent_run: transcript_path is required for new rows; supply it from the runner or use 'legacy_unknown' explicitly for backfill"))?
        .to_string();
    anyhow::ensure!(
        !transcript_path.is_empty(),
        "insert_agent_run: transcript_path must be non-empty"
    );
    conn.execute(
        "INSERT INTO agent_runs \
         (display_id, phase, cycle, role, model_id, harness_id, started_at, ended_at, exit_code, tokens_in, tokens_out, prompt_cache_hits, transcript_path, brief_text, configured_harness_id, configured_model_id, configured_thinking_effort, effective_model_id, effective_thinking_effort, thinking_effort_source, provider_id, api_id, session_id, workspace_path, runner_exit_kind, payload_valid, payload_error, cache_read_tokens, cache_write_tokens, cost_total) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
        rusqlite::params![
            display_id,
            phase,
            cycle,
            role,
            model_id,
            harness_id,
            started_at,
            ended_at,
            exit_code,
            telemetry.tokens_in,
            telemetry.tokens_out,
            telemetry.prompt_cache_hits,
            transcript_path,
            brief_text,
            telemetry.configured_harness_id,
            telemetry.configured_model_id,
            telemetry.configured_thinking_effort,
            telemetry.effective_model_id,
            telemetry.effective_thinking_effort,
            telemetry.thinking_effort_source,
            telemetry.provider_id,
            telemetry.api_id,
            telemetry.session_id,
            telemetry.workspace_path,
            telemetry.runner_exit_kind,
            telemetry.payload_valid,
            telemetry.payload_error,
            telemetry.cache_read_tokens,
            telemetry.cache_write_tokens,
            telemetry.cost_total,
        ],
    )
    .context("insert agent_runs")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_transition_history(
    tx: &Transaction,
    store: &str,
    row_id: i64,
    display_id: &str,
    from_status: &str,
    to_status: &str,
    verb: &str,
    invoker: &str,
    policy_ref: Option<&str>,
    policies_hash: Option<&str>,
    actor_note: Option<&str>,
) -> Result<()> {
    insert_transition_history_with_note(
        tx,
        store,
        row_id,
        display_id,
        from_status,
        to_status,
        verb,
        invoker,
        policy_ref,
        policies_hash,
        actor_note,
    )
}

/// Variant that records a free-form `actor_note` column. Used by recovery-
/// terminal verbs (e.g. close-out-of-band) to record the merge-target SHA as
/// provenance.
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_transition_history_with_note(
    tx: &Transaction,
    store: &str,
    row_id: i64,
    display_id: &str,
    from_status: &str,
    to_status: &str,
    verb: &str,
    invoker: &str,
    policy_ref: Option<&str>,
    policies_hash: Option<&str>,
    actor_note: Option<&str>,
) -> Result<()> {
    let occurred_at = crate::handlers::row::now_iso8601();
    tx.execute(
        "INSERT INTO transition_history \
         (store, row_id, display_id, from_status, to_status, verb, invoker, policy_ref, policies_hash, occurred_at, actor_note) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            store,
            row_id,
            display_id,
            from_status,
            to_status,
            verb,
            invoker,
            policy_ref,
            policies_hash,
            occurred_at,
            actor_note,
        ],
    )
    .context("insert_transition_history")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::ddl_for;
    use crate::schema::Schema;

    fn setup_obs() -> (Schema, Connection) {
        let yaml = r#"
name: observations
id_format: "L{:03d}"
default_actor: ai_with_human
lifecycle:
  states: [open, triaged]
  transitions:
    - {from: open, to: triaged, verb: triage, actor: ai_with_human}
fields:
  - name: summary
    type: text
    required: true
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        // mimic db::open: substrate DDL + per-store DDL
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        conn.execute_batch(&ddl_for(&schema)).unwrap();
        (schema, conn)
    }

    fn insert_open_row(_schema: &Schema, conn: &Connection) {
        conn.execute(
            "INSERT INTO observations (display_id, status, summary) VALUES ('L001', 'open', 'x')",
            [],
        )
        .unwrap();
    }

    fn agent_runs_columns(conn: &Connection) -> Vec<String> {
        conn.prepare("PRAGMA table_info(agent_runs)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn count_history(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM transition_history", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn fresh_db_open_creates_engine_runner_tables() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(&tmp.path().join("db.sqlite")).unwrap();
        conn.execute(
            "INSERT INTO engine_runner_heartbeats \
             (iteration, started_at, saw_tasks, saw_intake, saw_observations, actionable, held, dispatched) \
             VALUES (1, '2026-05-07T00:00:00Z', 0, 0, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO engine_runner_actions \
             (store, row_id, classification, held_reason, dispatched, last_logged_at, updated_at) \
             VALUES ('tasks', 1, 'held', 'needs_human', 0, NULL, '2026-05-07T00:00:00Z')",
            [],
        )
        .unwrap();
    }

    #[test]
    fn fresh_db_open_creates_agent_runs_columns() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = open(&tmp.path().join("db.sqlite")).unwrap();
        let cols = agent_runs_columns(&conn);
        for name in [
            "display_id",
            "phase",
            "cycle",
            "role",
            "model_id",
            "harness_id",
            "started_at",
            "ended_at",
            "exit_code",
            "tokens_in",
            "tokens_out",
            "prompt_cache_hits",
            "transcript_path",
            "configured_harness_id",
            "configured_model_id",
            "configured_thinking_effort",
            "effective_model_id",
            "effective_thinking_effort",
            "thinking_effort_source",
            "provider_id",
            "api_id",
            "session_id",
            "workspace_path",
            "runner_exit_kind",
            "payload_valid",
            "payload_error",
            "cache_read_tokens",
            "cache_write_tokens",
            "cost_total",
        ] {
            assert!(cols.iter().any(|c| c == name), "missing {name}: {cols:?}");
        }
    }

    #[test]
    fn existing_db_open_adds_agent_runs_telemetry_columns() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("db.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE agent_runs (
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
                    brief_text TEXT
                );",
            )
            .unwrap();
        }

        let conn = open(&path).unwrap();
        let cols = agent_runs_columns(&conn);
        for name in [
            "configured_harness_id",
            "configured_model_id",
            "configured_thinking_effort",
            "effective_model_id",
            "effective_thinking_effort",
            "thinking_effort_source",
            "provider_id",
            "api_id",
            "session_id",
            "workspace_path",
            "runner_exit_kind",
            "payload_valid",
            "payload_error",
            "cache_read_tokens",
            "cache_write_tokens",
            "cost_total",
        ] {
            assert!(cols.iter().any(|c| c == name), "missing {name}: {cols:?}");
        }
    }

    #[test]
    fn runner_outcomes_view_projects_executor_and_code_review_by_session_backlink() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                display_id TEXT PRIMARY KEY,
                cycles TEXT
            );",
        )
        .unwrap();
        ensure_runs_view_if_tasks_exists(&conn).unwrap();

        insert_agent_run(
            &conn,
            "T001",
            1,
            1,
            "executor",
            0,
            &AgentRunTelemetry {
                model_id: Some("gpt-5.5".to_string()),
                harness_id: Some("pi".to_string()),
                started_at: Some("2026-01-01T00:00:00Z".to_string()),
                ended_at: Some("2026-01-01T00:00:01Z".to_string()),
                transcript_path: Some("/workspace/.stores/runs/exec-session.jsonl".to_string()),
                session_id: Some("exec-session".to_string()),
                ..AgentRunTelemetry::default()
            },
            None,
        )
        .unwrap();
        insert_agent_run(
            &conn,
            "T001",
            1,
            1,
            "code_reviewer",
            0,
            &AgentRunTelemetry {
                model_id: Some("gpt-5.5".to_string()),
                harness_id: Some("pi".to_string()),
                started_at: Some("2026-01-01T00:00:02Z".to_string()),
                ended_at: Some("2026-01-01T00:00:03Z".to_string()),
                transcript_path: Some("/workspace/.stores/runs/review-session.jsonl".to_string()),
                session_id: Some("review-session".to_string()),
                ..AgentRunTelemetry::default()
            },
            None,
        )
        .unwrap();
        let cycles = serde_json::json!([{
            "phase": 1,
            "cycle": 1,
            "executor": {
                "summary": "implemented",
                "commit": "abc123",
                "transcript_path": ".stores/runs/exec-session.jsonl"
            },
            "review": {
                "gate": "PASS",
                "critical": 0,
                "major": 0,
                "minor": 1,
                "summary": "passes",
                "transcript_path": ".stores/runs/review-session.jsonl"
            }
        }]);
        conn.execute(
            "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
            rusqlite::params!["T001", cycles.to_string()],
        )
        .unwrap();

        let rows: Vec<(String, String, Option<String>, Option<String>, Option<i64>)> = conn
            .prepare(
                "SELECT role, outcome_kind, gate, summary, minor \
                 FROM runner_outcomes ORDER BY agent_run_id",
            )
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect();

        assert_eq!(
            rows,
            vec![
                (
                    "executor".to_string(),
                    "submitted_execution".to_string(),
                    None,
                    Some("implemented".to_string()),
                    None,
                ),
                (
                    "code_reviewer".to_string(),
                    "submitted_code_review".to_string(),
                    Some("PASS".to_string()),
                    Some("passes".to_string()),
                    Some(1),
                ),
            ]
        );
    }

    #[test]
    fn runner_outcomes_view_excludes_unlinked_planner_without_heuristics() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SUBSTRATE_DDL).unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                display_id TEXT PRIMARY KEY,
                plan TEXT,
                plan_review_log TEXT,
                cycles TEXT
            );",
        )
        .unwrap();
        ensure_runs_view_if_tasks_exists(&conn).unwrap();
        insert_agent_run(
            &conn,
            "T002",
            0,
            0,
            "planner",
            0,
            &AgentRunTelemetry {
                model_id: Some("gpt-5.5".to_string()),
                harness_id: Some("pi".to_string()),
                started_at: Some("2026-01-01T00:00:00Z".to_string()),
                ended_at: Some("2026-01-01T00:00:01Z".to_string()),
                transcript_path: Some("/workspace/.stores/runs/planner-session.jsonl".to_string()),
                session_id: Some("planner-session".to_string()),
                ..AgentRunTelemetry::default()
            },
            None,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id, plan, plan_review_log, cycles) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["T002", r#"{"summary":"plan exists"}"#, "[]", "[]",],
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM runner_outcomes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 0,
            "planner outputs lack a stable downstream backlink today"
        );
    }

    #[test]
    fn insert_agent_run_persists_required_fields() {
        let (_schema, conn) = setup_obs();
        let telemetry = AgentRunTelemetry {
            model_id: Some("m".to_string()),
            harness_id: Some("mock".to_string()),
            started_at: Some("2026-01-01T00:00:00Z".to_string()),
            ended_at: Some("2026-01-01T00:00:01Z".to_string()),
            tokens_in: Some(10),
            tokens_out: Some(20),
            prompt_cache_hits: Some(3),
            transcript_path: Some("/tmp/run.jsonl".to_string()),
            stderr_log_path: None,
            configured_harness_id: Some("mock".to_string()),
            configured_model_id: Some("mock-configured".to_string()),
            configured_thinking_effort: Some("medium".to_string()),
            effective_model_id: Some("mock-effective".to_string()),
            effective_thinking_effort: Some("medium".to_string()),
            thinking_effort_source: Some("config".to_string()),
            provider_id: Some("mock-provider".to_string()),
            api_id: Some("mock-api".to_string()),
            session_id: Some("session-1".to_string()),
            workspace_path: Some("/workspace".to_string()),
            runner_exit_kind: Some("nonzero".to_string()),
            payload_valid: Some(false),
            payload_error: Some("payload failed".to_string()),
            cache_read_tokens: Some(4),
            cache_write_tokens: Some(5),
            cost_total: Some(0.125),
        };
        insert_agent_run(&conn, "T001", 1, 2, "executor", 7, &telemetry, None).unwrap();
        let row: (
            String,
            i64,
            i64,
            String,
            String,
            i64,
            i64,
            i64,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            i64,
            String,
            i64,
            i64,
            f64,
        ) = conn
            .query_row(
                "SELECT display_id, phase, cycle, role, harness_id, tokens_in, tokens_out, prompt_cache_hits, transcript_path, configured_harness_id, configured_model_id, configured_thinking_effort, effective_model_id, effective_thinking_effort, thinking_effort_source, provider_id, api_id, session_id, workspace_path, runner_exit_kind, payload_valid, payload_error, cache_read_tokens, cache_write_tokens, cost_total FROM agent_runs",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                        r.get(9)?,
                        r.get(10)?,
                        r.get(11)?,
                        r.get(12)?,
                        r.get(13)?,
                        r.get(14)?,
                        r.get(15)?,
                        r.get(16)?,
                        r.get(17)?,
                        r.get(18)?,
                        r.get(19)?,
                        r.get(20)?,
                        r.get(21)?,
                        r.get(22)?,
                        r.get(23)?,
                        r.get(24)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, "T001");
        assert_eq!(row.1, 1);
        assert_eq!(row.2, 2);
        assert_eq!(row.3, "executor");
        assert_eq!(row.4, "mock");
        assert_eq!(row.5, 10);
        assert_eq!(row.6, 20);
        assert_eq!(row.7, 3);
        assert_eq!(row.8, "/tmp/run.jsonl");
        assert_eq!(row.9, "mock");
        assert_eq!(row.10, "mock-configured");
        assert_eq!(row.11, "medium");
        assert_eq!(row.12, "mock-effective");
        assert_eq!(row.13, "medium");
        assert_eq!(row.14, "config");
        assert_eq!(row.15, "mock-provider");
        assert_eq!(row.16, "mock-api");
        assert_eq!(row.17, "session-1");
        assert_eq!(row.18, "/workspace");
        assert_eq!(row.19, "nonzero");
        assert_eq!(row.20, 0);
        assert_eq!(row.21, "payload failed");
        assert_eq!(row.22, 4);
        assert_eq!(row.23, 5);
        assert_eq!(row.24, 0.125);
    }

    #[test]
    fn substrate_ddl_creates_transition_history_table() {
        let (_schema, conn) = setup_obs();
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='transition_history'",
            )
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(tables, vec!["transition_history".to_string()]);
    }

    /// AC1.5: a single transition write inserts exactly one row into
    /// transition_history with the expected column population.
    #[test]
    fn transition_history_inserts_one_row_per_transition() {
        use crate::schema::actor::Actor;

        let (schema, conn) = setup_obs();
        insert_open_row(&schema, &conn);

        assert_eq!(count_history(&conn), 0, "history starts empty");

        let cmd = clap::Command::new("triage")
            .arg(clap::Arg::new("display_id").required(true).index(1))
            .arg(clap::Arg::new("summary").long("summary"));
        let matches = cmd.get_matches_from(["triage", "L001"]);
        crate::handlers::transition::run(&schema, &conn, &matches, Actor::Human.into(), "triage")
            .unwrap();

        assert_eq!(count_history(&conn), 1, "one history row per transition");

        let (store, row_id, display_id, from, to, verb, invoker, policy_ref): (
            String,
            i64,
            String,
            Option<String>,
            String,
            String,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT store, row_id, display_id, from_status, to_status, verb, invoker, policy_ref FROM transition_history",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(store, "observations");
        assert!(row_id > 0);
        assert_eq!(display_id, "L001");
        assert_eq!(from.as_deref(), Some("open"));
        assert_eq!(to, "triaged");
        assert_eq!(verb, "triage");
        assert_eq!(invoker, "human");
        assert!(
            policy_ref.is_none(),
            "manual transition has NULL policy_ref"
        );
    }

    // L503-A: brief_text persistence tests (Task 1.9a + 1.9b)

    #[test]
    fn insert_agent_run_persists_brief_text() {
        let (_schema, conn) = setup_obs();
        let telemetry = AgentRunTelemetry {
            model_id: Some("m".to_string()),
            harness_id: Some("mock".to_string()),
            started_at: Some("2026-01-01T00:00:00Z".to_string()),
            ended_at: Some("2026-01-01T00:00:01Z".to_string()),
            tokens_in: Some(0),
            tokens_out: Some(0),
            prompt_cache_hits: Some(0),
            transcript_path: Some("/tmp/run.jsonl".to_string()),
            stderr_log_path: None,
            ..AgentRunTelemetry::default()
        };
        let expected = "# Phase 1: Foo\nsome brief content\n";
        insert_agent_run(
            &conn,
            "T001",
            1,
            1,
            "planner",
            0,
            &telemetry,
            Some(expected),
        )
        .unwrap();
        let got: String = conn
            .query_row(
                "SELECT brief_text FROM agent_runs WHERE display_id='T001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(got, expected, "brief_text must round-trip byte-equal");
    }

    #[test]
    fn insert_agent_run_accepts_null_brief_text() {
        let (_schema, conn) = setup_obs();
        let telemetry = AgentRunTelemetry {
            model_id: Some("m".to_string()),
            harness_id: Some("mock".to_string()),
            started_at: Some("2026-01-01T00:00:00Z".to_string()),
            ended_at: Some("2026-01-01T00:00:01Z".to_string()),
            tokens_in: Some(0),
            tokens_out: Some(0),
            prompt_cache_hits: Some(0),
            transcript_path: Some("/tmp/run.jsonl".to_string()),
            stderr_log_path: None,
            ..AgentRunTelemetry::default()
        };
        insert_agent_run(&conn, "T001", 1, 1, "planner", 0, &telemetry, None).unwrap();
        let got: Option<String> = conn
            .query_row(
                "SELECT brief_text FROM agent_runs WHERE display_id='T001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(got.is_none(), "brief_text must be NULL when None passed");
    }
}
