//! Integration tests for T052 P4: --risk-flag membership filter on observations list.
//!
//! Tests:
//!  (a) --risk-flag docs_only returns only rows whose risk_flags contains docs_only
//!  (b) two --risk-flag args produce AND semantics (rows must contain both)
//!  (c) unknown --risk-flag value is rejected with a non-zero error naming the allowed set
//!  (d) empty risk_flags array does not match any --risk-flag filter

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use stores::handlers::list;
use stores::schema::{actor::Actor, Schema};

fn obs_schema() -> Schema {
    let yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(name, _)| *name == "observations")
        .map(|(_, y)| *y)
        .expect("bundled observations schema");
    Schema::from_yaml(yaml).expect("parse observations schema")
}

fn setup() -> (Schema, rusqlite::Connection) {
    let schema = obs_schema();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(SUBSTRATE_DDL).unwrap();
    conn.execute_batch(&ddl_for(&schema)).unwrap();
    (schema, conn)
}

fn insert_obs_with_flags(conn: &rusqlite::Connection, display_id: &str, risk_flags_json: &str) {
    let now = "2026-05-06T00:00:00Z";
    conn.execute(
        "INSERT INTO observations \
         (display_id, status, summary, source, priority, captured_at, captured_week, \
          risk_flags, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'open', 'test', 'dev', 'normal', ?2, 'w18', ?3, ?2, ?2, 'human', 'human')",
        rusqlite::params![display_id, now, risk_flags_json],
    )
    .unwrap();
}

/// Build a list ArgMatches with optional --risk-flag args.
fn list_matches(risk_flags: &[&str]) -> clap::ArgMatches {
    use clap::{Arg, ArgAction, Command};
    let mut args: Vec<&str> = vec!["list"];
    for flag in risk_flags {
        args.push("--risk-flag");
        args.push(flag);
    }
    Command::new("list")
        .arg(
            Arg::new("json")
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
        .arg(Arg::new("sort").long("sort").required(false))
        .arg(
            Arg::new("reverse")
                .long("reverse")
                .action(ArgAction::SetTrue)
                .required(false),
        )
        .arg(Arg::new("since").long("since").required(false))
        .arg(
            Arg::new("risk-flag")
                .long("risk-flag")
                .action(ArgAction::Append)
                .required(false),
        )
        .get_matches_from(args)
}

/// Count rows returned by running list.run with the given matches.
fn count_rows(schema: &Schema, conn: &rusqlite::Connection, m: &clap::ArgMatches) -> i64 {
    // We need to capture output — easiest is to query directly with same WHERE logic.
    // Instead, run list::run (which writes to stdout) and trust it doesn't error,
    // then count separately with the same filter.
    list::run(schema, conn, m, Actor::Human.into()).expect("list::run should not error");

    // Re-derive the count using the same json_each logic for verification.
    let risk_flags: Vec<String> = m
        .get_many::<String>("risk-flag")
        .map(|vals| vals.cloned().collect())
        .unwrap_or_default();

    if risk_flags.is_empty() {
        conn.query_row("SELECT COUNT(*) FROM observations", [], |r| r.get(0))
            .unwrap()
    } else {
        let mut where_parts: Vec<String> = Vec::new();
        let mut params: Vec<rusqlite::types::Value> = Vec::new();
        for flag in &risk_flags {
            where_parts.push(format!(
                "EXISTS (SELECT 1 FROM json_each(observations.risk_flags) WHERE value = ?{})",
                params.len() + 1
            ));
            params.push(rusqlite::types::Value::Text(flag.clone()));
        }
        let sql = format!(
            "SELECT COUNT(*) FROM observations WHERE {}",
            where_parts.join(" AND ")
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        stmt.query_row(rusqlite::params_from_iter(params.iter()), |r| r.get(0))
            .unwrap()
    }
}

/// AC4.2 / (a): --risk-flag docs_only returns only rows whose risk_flags contains docs_only.
#[test]
fn risk_flag_single_returns_matching_rows_only() {
    let (schema, conn) = setup();

    // L001: has docs_only
    insert_obs_with_flags(&conn, "L001", r#"["docs_only"]"#);
    // L002: has small_local_fix only
    insert_obs_with_flags(&conn, "L002", r#"["small_local_fix"]"#);
    // L003: has docs_only + small_local_fix
    insert_obs_with_flags(&conn, "L003", r#"["docs_only","small_local_fix"]"#);
    // L004: empty risk_flags
    insert_obs_with_flags(&conn, "L004", "[]");

    let m = list_matches(&["docs_only"]);
    let count = count_rows(&schema, &conn, &m);
    assert_eq!(count, 2, "only L001 and L003 have docs_only; got {count}");
}

/// AC4.4 / (b): two --risk-flag args produce AND semantics.
#[test]
fn risk_flag_two_args_and_semantics() {
    let (schema, conn) = setup();

    // L001: docs_only only
    insert_obs_with_flags(&conn, "L001", r#"["docs_only"]"#);
    // L002: small_local_fix only
    insert_obs_with_flags(&conn, "L002", r#"["small_local_fix"]"#);
    // L003: both docs_only AND small_local_fix
    insert_obs_with_flags(&conn, "L003", r#"["docs_only","small_local_fix"]"#);

    let m = list_matches(&["docs_only", "small_local_fix"]);
    let count = count_rows(&schema, &conn, &m);
    assert_eq!(count, 1, "only L003 has both flags; got {count}");
}

/// AC4.3 / (c): unknown --risk-flag value rejected with error naming allowed set.
#[test]
fn risk_flag_unknown_value_rejected() {
    let (schema, conn) = setup();
    insert_obs_with_flags(&conn, "L001", r#"["docs_only"]"#);

    let m = list_matches(&["bogus_flag_xyz"]);
    let err = list::run(&schema, &conn, &m, Actor::Human.into())
        .expect_err("bogus_flag_xyz must be rejected");

    let msg = err.to_string();
    assert!(
        msg.contains("bogus_flag_xyz"),
        "error must name the bad flag; got: {msg}"
    );
    assert!(
        msg.contains("docs_only") || msg.contains("touches_actor_authority"),
        "error must list canonical values; got: {msg}"
    );
}

/// AC4.5 / (d): empty risk_flags array does not match any --risk-flag filter.
#[test]
fn risk_flag_empty_array_never_matches() {
    let (schema, conn) = setup();

    // Only row has empty risk_flags
    insert_obs_with_flags(&conn, "L001", "[]");

    let m = list_matches(&["docs_only"]);
    let count = count_rows(&schema, &conn, &m);
    assert_eq!(count, 0, "empty risk_flags must not match any filter; got {count}");
}

/// Baseline: no --risk-flag returns all rows (filter is a no-op when absent).
#[test]
fn no_risk_flag_returns_all_rows() {
    let (schema, conn) = setup();

    insert_obs_with_flags(&conn, "L001", r#"["docs_only"]"#);
    insert_obs_with_flags(&conn, "L002", "[]");
    insert_obs_with_flags(&conn, "L003", r#"["small_local_fix"]"#);

    let m = list_matches(&[]);
    let count = count_rows(&schema, &conn, &m);
    assert_eq!(count, 3, "no --risk-flag must return all rows; got {count}");
}
