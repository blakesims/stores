use anyhow::{Context, Result};
use clap::ArgMatches;
use regex::Regex;
use rusqlite::Connection;
use std::sync::OnceLock;

use crate::schema::{actor::InvokerCtx, Schema};

type BucketRow = (String, String, String);
type Bucket = (String, Vec<BucketRow>);

// ---------------------------------------------------------------------------
// Curated registry (single source of truth)
// ---------------------------------------------------------------------------

/// Unified curated registry. Each entry is (key, conservative_backfill_regex).
/// This is the single source of truth: DDL CHECK, write validation, and backfill
/// classification are all derived from this slice. Adding an entry here
/// automatically updates all three without any edit to schema.yaml.
pub const CLUSTER_REGISTRY: &[(&str, &str)] = &[
    ("deploy-blocked-merge-conflict", r"(?i)\bmerge[- ]conflict\b"),
    ("silent-zombie-watchdog", r"(?i)\bsilent[- ]zombie\b"),
    ("revise-loop-non-convergent", r"(?i)\brevise[- ]loop\b"),
    ("stale-base-er", r"(?i)\bstale[- ]base\b"),
    ("gatekeeper-front-door-stuck", r"(?i)\bgatekeeper\b"),
];

/// Returns the curated cluster key names, derived from `CLUSTER_REGISTRY`.
/// Callers use this for DDL CHECK generation, write validation, and tests.
pub fn curated_cluster_keys() -> &'static [&'static str] {
    static KEYS: OnceLock<Vec<&'static str>> = OnceLock::new();
    KEYS.get_or_init(|| CLUSTER_REGISTRY.iter().map(|(k, _)| *k).collect())
}

/// Returns the SQL CHECK clause fragment for the cluster_key column.
/// e.g. `CHECK (cluster_key IN ('deploy-blocked-merge-conflict', ...))`
pub fn check_clause_sql() -> String {
    let list = curated_cluster_keys()
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("CHECK (cluster_key IN ({list}))")
}

/// Validate that `v` is one of the curated registry keys.
/// On error, returns the canonical error message including all allowed values.
pub fn validate_value(v: &str) -> Result<(), String> {
    if curated_cluster_keys().contains(&v) {
        Ok(())
    } else {
        let allowed = curated_cluster_keys().join(", ");
        Err(format!(
            "unknown cluster_key '{v}'; allowed values: [{allowed}]"
        ))
    }
}

/// Compiled pattern cache (lazy-init).
fn compiled_patterns() -> &'static Vec<(&'static str, Regex)> {
    static CACHE: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        CLUSTER_REGISTRY
            .iter()
            .map(|(key, pat)| (*key, Regex::new(pat).expect("valid cluster_key regex")))
            .collect()
    })
}

/// Classify a summary string against the curated registry.
/// Returns `Some(key)` if exactly one registry regex matches, `None` if zero
/// or more than one match (ambiguous / unrelated).
pub fn classify_summary(summary: &str) -> Option<&'static str> {
    let matches: Vec<&'static str> = compiled_patterns()
        .iter()
        .filter_map(|(key, re)| if re.is_match(summary) { Some(*key) } else { None })
        .collect();
    if matches.len() == 1 {
        Some(matches[0])
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// CLI handlers
// ---------------------------------------------------------------------------

/// Pure bucket builder — extracted for testability.
pub(crate) fn build_clusters(conn: &Connection) -> Result<Vec<Bucket>> {
    let mut stmt = conn.prepare(
        "SELECT display_id, cluster_key, captured_at, summary \
         FROM observations \
         WHERE status IN ('open','ready') \
         ORDER BY cluster_key NULLS LAST, captured_at DESC",
    )?;
    let rows: Vec<(String, Option<String>, String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("query observations for clusters")?;

    // Group by cluster_key (None → "(uncategorized)")
    let mut bucket_map: std::collections::BTreeMap<String, Vec<(String, String, String)>> =
        std::collections::BTreeMap::new();
    for (display_id, cluster_key, captured_at, summary) in rows {
        let key = cluster_key.unwrap_or_else(|| "(uncategorized)".to_string());
        bucket_map
            .entry(key)
            .or_default()
            .push((display_id, captured_at, summary));
    }

    // Sort buckets: descending count; ties broken by key name.
    let mut buckets: Vec<Bucket> = bucket_map.into_iter().collect();
    buckets.sort_by(|(ka, va), (kb, vb)| {
        vb.len().cmp(&va.len()).then_with(|| ka.cmp(kb))
    });
    Ok(buckets)
}

/// `stores observations clusters` — single-shot grouping of open/ready
/// observations by cluster_key.
pub fn run_clusters_cmd(
    _schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: InvokerCtx,
) -> Result<()> {
    run_clusters_cmd_to(conn, matches, &mut std::io::stdout())
}

pub(crate) fn run_clusters_cmd_to(
    conn: &Connection,
    matches: &ArgMatches,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let json_flag = matches.get_flag("json");
    let buckets = build_clusters(conn)?;

    if json_flag {
        let json_buckets: Vec<serde_json::Value> = buckets
            .iter()
            .map(|(key, rows)| {
                let row_arr: Vec<serde_json::Value> = rows
                    .iter()
                    .take(5)
                    .map(|(id, ts, summ)| {
                        serde_json::json!({
                            "display_id": id,
                            "captured_at": ts,
                            "summary": summ
                        })
                    })
                    .collect();
                serde_json::json!({
                    "cluster_key": key,
                    "count": rows.len(),
                    "rows": row_arr
                })
            })
            .collect();
        let obj = serde_json::json!({ "buckets": json_buckets });
        writeln!(out, "{}", serde_json::to_string_pretty(&obj)?)?;
    } else {
        for (key, rows) in &buckets {
            writeln!(out, "{key} ({})", rows.len())?;
            for (display_id, captured_at, summary) in rows.iter().take(5) {
                writeln!(out, "  {display_id} {captured_at} {summary}")?;
            }
        }
    }

    Ok(())
}

/// `stores observations overdue-ready` — lists ready observations whose
/// linked task is in a terminal-success state.
pub fn run_overdue_ready_cmd(
    _schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: InvokerCtx,
) -> Result<()> {
    run_overdue_ready_cmd_to(conn, matches, &mut std::io::stdout())
}

pub(crate) fn run_overdue_ready_cmd_to(
    conn: &Connection,
    matches: &ArgMatches,
    out: &mut dyn std::io::Write,
) -> Result<()> {
    let json_flag = matches.get_flag("json");

    let mut stmt = conn.prepare(
        "SELECT o.display_id, o.task_id, o.captured_at, o.summary \
         FROM observations o \
         JOIN tasks t ON o.task_id = t.display_id \
         WHERE o.status = 'ready' \
           AND t.status IN ('accepted','closed_out_of_band','schema_migrated') \
         ORDER BY o.captured_at",
    )?;
    let rows: Vec<(String, Option<String>, String, String)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()
        .context("query observations for overdue-ready")?;

    if json_flag {
        let arr: Vec<serde_json::Value> = rows
            .iter()
            .map(|(id, task_id, captured_at, summary)| {
                serde_json::json!({
                    "display_id": id,
                    "task_id": task_id,
                    "captured_at": captured_at,
                    "summary": summary
                })
            })
            .collect();
        writeln!(out, "{}", serde_json::to_string_pretty(&arr)?)?;
    } else {
        for (display_id, task_id, captured_at, summary) in &rows {
            let tid = task_id.as_deref().unwrap_or("(none)");
            writeln!(out, "{display_id} task={tid} {captured_at} {summary}")?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::ddl::ddl_for;
    use crate::schema::{actor::Actor, Schema};
    use rusqlite::Connection;

    const OBSERVATIONS_YAML: &str = include_str!("../../stores/observations/schema.yaml");
    const TASKS_YAML: &str = include_str!("../../stores/tasks/schema.yaml");

    fn install_schemas(conn: &Connection) {
        let obs = Schema::from_yaml(OBSERVATIONS_YAML).unwrap();
        let tasks = Schema::from_yaml(TASKS_YAML).unwrap();
        conn.execute_batch(&ddl_for(&obs)).unwrap();
        conn.execute_batch(&ddl_for(&tasks)).unwrap();
    }

    fn insert_observation(
        conn: &Connection,
        display_id: &str,
        status: &str,
        cluster_key: Option<&str>,
        task_id: Option<&str>,
        captured_at: &str,
        summary: &str,
    ) {
        let ck_val: rusqlite::types::Value = cluster_key
            .map(|s| rusqlite::types::Value::Text(s.to_string()))
            .unwrap_or(rusqlite::types::Value::Null);
        let tid_val: rusqlite::types::Value = task_id
            .map(|s| rusqlite::types::Value::Text(s.to_string()))
            .unwrap_or(rusqlite::types::Value::Null);
        conn.execute(
            "INSERT INTO observations \
             (display_id, status, created_at, updated_at, created_by, updated_by, \
              summary, source, priority, captured_at, captured_week, cluster_key, task_id) \
             VALUES (?1, ?2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', \
                     'ai_autonomous', 'ai_autonomous', ?3, 'dev', 'normal', ?4, 'w20-d1', ?5, ?6)",
            rusqlite::params![display_id, status, summary, captured_at, ck_val, tid_val],
        )
        .expect("insert observation");
    }

    fn insert_task(conn: &Connection, display_id: &str, status: &str) {
        conn.execute(
            "INSERT INTO tasks \
             (display_id, status, created_at, updated_at, created_by, updated_by, title, slug) \
             VALUES (?1, ?2, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', \
                     'ai_autonomous', 'ai_autonomous', ?1, ?1)",
            rusqlite::params![display_id, status],
        )
        .expect("insert task");
    }

    fn invoker_auto() -> InvokerCtx {
        InvokerCtx::bare(Actor::AiAutonomous)
    }

    // ---- validate_value tests ----

    #[test]
    fn validate_value_accepts_all_registry_keys() {
        for key in curated_cluster_keys() {
            assert!(
                validate_value(key).is_ok(),
                "registry key '{key}' must be accepted by validate_value"
            );
        }
    }

    #[test]
    fn validate_value_rejects_bogus_key_with_allowed_list() {
        let err = validate_value("bogus-key").unwrap_err();
        assert!(
            err.contains("unknown cluster_key 'bogus-key'"),
            "error must mention bogus-key: {err}"
        );
        for key in curated_cluster_keys() {
            assert!(
                err.contains(key),
                "error must list allowed key '{key}': {err}"
            );
        }
    }

    // ---- classify_summary tests ----

    #[test]
    fn classify_summary_matches_deploy_blocked() {
        assert_eq!(
            classify_summary("deploy blocked by merge conflict"),
            Some("deploy-blocked-merge-conflict")
        );
        assert_eq!(
            classify_summary("merge conflict in branch"),
            Some("deploy-blocked-merge-conflict")
        );
    }

    #[test]
    fn classify_summary_ambiguous_returns_none() {
        // Matches both deploy-blocked and stale-base-er → ambiguous
        assert_eq!(
            classify_summary("stale-base merge conflict"),
            None,
            "ambiguous summary must return None"
        );
    }

    #[test]
    fn classify_summary_unrelated_returns_none() {
        assert_eq!(classify_summary("something unrelated entirely"), None);
    }

    // ---- single-source-of-truth drift test (AC1.9) ----

    #[test]
    fn single_source_of_truth_drift_test() {
        let schema = Schema::from_yaml(OBSERVATIONS_YAML).expect("parse observations schema");
        let ddl = ddl_for(&schema);

        for key in curated_cluster_keys() {
            // (a) DDL CHECK contains the key
            assert!(
                ddl.contains(key),
                "DDL cluster_key CHECK must contain registry key '{key}':\n{ddl}"
            );
            // (b) validate_value accepts each key
            assert!(
                validate_value(key).is_ok(),
                "validate_value must accept registry key '{key}'"
            );
        }

        // (c) bogus key error message contains all 5 keys
        let err = validate_value("never-a-real-key-xyz").unwrap_err();
        for key in curated_cluster_keys() {
            assert!(
                err.contains(key),
                "error message must list all keys; missing '{key}': {err}"
            );
        }
    }

    // ---- clusters cmd: 3+2+5 fixture (AC1.5) ----

    fn setup_3_2_5_fixture(conn: &Connection) {
        // 3 deploy-blocked rows (status=open)
        for i in 0..3u32 {
            insert_observation(
                conn,
                &format!("L{:03}", i + 1),
                "open",
                Some("deploy-blocked-merge-conflict"),
                None,
                &format!("2026-01-{:02}T10:00:00Z", i + 1),
                "deploy blocked summary",
            );
        }
        // 2 silent-zombie rows (status=ready)
        for i in 0..2u32 {
            insert_observation(
                conn,
                &format!("L{:03}", i + 4),
                "ready",
                Some("silent-zombie-watchdog"),
                None,
                &format!("2026-01-{:02}T10:00:00Z", i + 4),
                "silent zombie summary",
            );
        }
        // 5 untagged (cluster_key=NULL, status=open)
        for i in 0..5u32 {
            insert_observation(
                conn,
                &format!("L{:03}", i + 6),
                "open",
                None,
                None,
                &format!("2026-01-{:02}T10:00:00Z", i + 6),
                "untagged summary",
            );
        }
    }

    #[test]
    fn clusters_fixture_3_2_5_text_output() {
        use clap::{Arg, ArgAction, Command};
        let conn = Connection::open_in_memory().unwrap();
        install_schemas(&conn);
        setup_3_2_5_fixture(&conn);

        // Text output: descending count order — (uncategorized)(5), deploy-blocked(3), silent-zombie(2)
        let cmd = Command::new("clusters")
            .arg(Arg::new("json").long("json").action(ArgAction::SetTrue));
        let matches = cmd.get_matches_from(["clusters"]);
        let mut out = Vec::<u8>::new();
        run_clusters_cmd_to(&conn, &matches, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        // First bucket header must be (uncategorized) (5) — highest count
        let lines: Vec<&str> = text.lines().collect();
        let headers: Vec<&str> = lines.iter().copied().filter(|l| !l.starts_with("  ")).collect();
        assert_eq!(headers.len(), 3, "expected 3 bucket headers: {text}");
        assert!(
            headers[0].starts_with("(uncategorized) (5)"),
            "first bucket must be (uncategorized)(5), got: {}",
            headers[0]
        );
        assert!(
            headers[1].starts_with("deploy-blocked-merge-conflict (3)"),
            "second bucket must be deploy-blocked(3), got: {}",
            headers[1]
        );
        assert!(
            headers[2].starts_with("silent-zombie-watchdog (2)"),
            "third bucket must be silent-zombie(2), got: {}",
            headers[2]
        );
    }

    #[test]
    fn clusters_fixture_3_2_5_json_output() {
        use clap::{Arg, ArgAction, Command};
        let conn = Connection::open_in_memory().unwrap();
        install_schemas(&conn);
        setup_3_2_5_fixture(&conn);

        // JSON output: buckets array length 3, correct order and counts
        let cmd = Command::new("clusters")
            .arg(Arg::new("json").long("json").action(ArgAction::SetTrue));
        let matches = cmd.get_matches_from(["clusters", "--json"]);
        let mut out = Vec::<u8>::new();
        run_clusters_cmd_to(&conn, &matches, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text)
            .expect("--json output must be valid JSON");
        let buckets = v["buckets"].as_array().expect("buckets must be array");
        assert_eq!(buckets.len(), 3, "expected 3 buckets in JSON");
        assert_eq!(buckets[0]["cluster_key"].as_str().unwrap(), "(uncategorized)");
        assert_eq!(buckets[0]["count"].as_u64().unwrap(), 5);
        assert_eq!(buckets[1]["cluster_key"].as_str().unwrap(), "deploy-blocked-merge-conflict");
        assert_eq!(buckets[1]["count"].as_u64().unwrap(), 3);
        assert_eq!(buckets[2]["cluster_key"].as_str().unwrap(), "silent-zombie-watchdog");
        assert_eq!(buckets[2]["count"].as_u64().unwrap(), 2);
    }

    #[test]
    fn clusters_exits_cleanly_no_watch() {
        // AC1.8: run_clusters_cmd returns Ok without --watch
        use clap::{Arg, ArgAction, Command};
        let conn = Connection::open_in_memory().unwrap();
        install_schemas(&conn);
        let schema = Schema::from_yaml(OBSERVATIONS_YAML).unwrap();
        let cmd = Command::new("clusters")
            .arg(Arg::new("json").long("json").action(ArgAction::SetTrue));
        let matches = cmd.get_matches_from(["clusters"]);
        let result = run_clusters_cmd(&schema, &conn, &matches, invoker_auto());
        assert!(result.is_ok(), "clusters cmd must return Ok: {result:?}");
    }

    // ---- overdue-ready fixture (AC1.6) ----

    fn build_overdue_ready_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        install_schemas(&conn);

        for (tid, status) in [
            ("T001", "accepted"),
            ("T002", "closed_out_of_band"),
            ("T003", "schema_migrated"),
            ("T004", "open"),
            ("T005", "abandoned"),
            ("T006", "rejected"),
        ] {
            insert_task(&conn, tid, status);
        }

        for (i, (lid, tid)) in [
            ("L001", "T001"),
            ("L002", "T002"),
            ("L003", "T003"),
            ("L004", "T004"),
            ("L005", "T005"),
            ("L006", "T006"),
        ]
        .iter()
        .enumerate()
        {
            insert_observation(
                &conn,
                lid,
                "ready",
                None,
                Some(tid),
                &format!("2026-01-{:02}T10:00:00Z", i + 1),
                "summary",
            );
        }

        conn
    }

    #[test]
    fn overdue_ready_fixture() {
        let conn = build_overdue_ready_fixture();

        let ids: Vec<String> = {
            let mut stmt = conn
                .prepare(
                    "SELECT o.display_id \
                     FROM observations o \
                     JOIN tasks t ON o.task_id = t.display_id \
                     WHERE o.status = 'ready' \
                       AND t.status IN ('accepted','closed_out_of_band','schema_migrated') \
                     ORDER BY o.captured_at",
                )
                .unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };

        assert_eq!(ids.len(), 3, "expected exactly 3 terminal-success rows: {ids:?}");
        assert!(ids.contains(&"L001".to_string()), "L001 (accepted)");
        assert!(ids.contains(&"L002".to_string()), "L002 (cob)");
        assert!(ids.contains(&"L003".to_string()), "L003 (schema_migrated)");
        assert!(!ids.contains(&"L004".to_string()), "L004 (open) excluded");
        assert!(!ids.contains(&"L005".to_string()), "L005 (abandoned) excluded");
        assert!(!ids.contains(&"L006".to_string()), "L006 (rejected) excluded");
    }

    #[test]
    fn overdue_ready_cmd_text_output() {
        let conn = build_overdue_ready_fixture();
        let matches = clap::Command::new("t")
            .arg(clap::Arg::new("json").long("json").action(clap::ArgAction::SetTrue))
            .get_matches_from(["t"]);

        let mut out = Vec::<u8>::new();
        run_overdue_ready_cmd_to(&conn, &matches, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();

        // Exactly 3 terminal-success rows appear in the output
        assert!(text.contains("L001"), "L001 (accepted) must appear");
        assert!(text.contains("L002"), "L002 (cob) must appear");
        assert!(text.contains("L003"), "L003 (schema_migrated) must appear");
        assert!(!text.contains("L004"), "L004 (open) must not appear");
        assert!(!text.contains("L005"), "L005 (abandoned) must not appear");
        assert!(!text.contains("L006"), "L006 (rejected) must not appear");
        assert_eq!(text.lines().count(), 3, "exactly 3 output lines");
    }

    #[test]
    fn overdue_ready_cmd_json_output() {
        let conn = build_overdue_ready_fixture();
        let matches = clap::Command::new("t")
            .arg(clap::Arg::new("json").long("json").action(clap::ArgAction::SetTrue))
            .get_matches_from(["t", "--json"]);

        let mut out = Vec::<u8>::new();
        run_overdue_ready_cmd_to(&conn, &matches, &mut out).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        let arr = parsed.as_array().expect("JSON output must be an array");

        assert_eq!(arr.len(), 3, "JSON array must have exactly 3 entries");
        let ids: Vec<&str> = arr
            .iter()
            .map(|v| v["display_id"].as_str().unwrap())
            .collect();
        assert!(ids.contains(&"L001"), "L001 in JSON");
        assert!(ids.contains(&"L002"), "L002 in JSON");
        assert!(ids.contains(&"L003"), "L003 in JSON");
        assert!(!ids.contains(&"L004"), "L004 excluded from JSON");
        assert!(!ids.contains(&"L005"), "L005 excluded from JSON");
        assert!(!ids.contains(&"L006"), "L006 excluded from JSON");
    }

    // ---- check_clause_sql ----

    #[test]
    fn check_clause_sql_contains_all_keys() {
        let sql = check_clause_sql();
        assert!(sql.starts_with("CHECK (cluster_key IN ("), "format: {sql}");
        for key in curated_cluster_keys() {
            assert!(sql.contains(key), "check clause missing '{key}': {sql}");
        }
    }
}
