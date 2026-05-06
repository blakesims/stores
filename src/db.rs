use anyhow::{Context, Result};
use rusqlite::{Connection, Transaction};
use std::path::Path;

use crate::codegen::ddl::{validate_framework_ddl, SUBSTRATE_DDL};
use crate::handlers::framework_migrate::apply_framework_drift;

/// Env var: when set to "1", `db::open` validates SUBSTRATE_DDL but skips
/// the framework-drift auto-apply pass. Used by operators who want explicit
/// control over `stores migrate` runs.
const DISABLE_AUTOAPPLY_ENV: &str = "STORES_DISABLE_FRAMEWORK_AUTOAPPLY";

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
        None,
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

    fn count_history(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM transition_history", [], |r| r.get(0))
            .unwrap()
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
}
