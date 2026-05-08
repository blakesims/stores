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

/// The 5 curated cluster_key values. This const is the authoritative list:
/// schema.yaml declares only the field's existence; DDL CHECK and validator
/// allowed-list are both derived from this slice at compile time.
pub const CURATED_CLUSTER_KEYS: &[&str] = &[
    "deploy-blocked-merge-conflict",
    "silent-zombie-watchdog",
    "revise-loop-non-convergent",
    "stale-base-er",
    "gatekeeper-front-door-stuck",
];

/// Conservative regexes for backfill classification. Each tuple is (key, pattern).
/// Patterns are case-insensitive and designed to be mutually exclusive on
/// unambiguous inputs; ambiguous inputs (matching >1 pattern) return None.
pub const CURATED_CLUSTER_KEY_PATTERNS: &[(&str, &str)] = &[
    ("deploy-blocked-merge-conflict", r"(?i)\bmerge[- ]conflict\b"),
    ("silent-zombie-watchdog", r"(?i)\bsilent[- ]zombie\b"),
    ("revise-loop-non-convergent", r"(?i)\brevise[- ]loop\b"),
    ("stale-base-er", r"(?i)\bstale[- ]base\b"),
    ("gatekeeper-front-door-stuck", r"(?i)\bgatekeeper\b"),
];

/// Returns the SQL CHECK clause fragment for the cluster_key column.
/// e.g. `CHECK (cluster_key IN ('deploy-blocked-merge-conflict', ...))`
pub fn check_clause_sql() -> String {
    let list = CURATED_CLUSTER_KEYS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("CHECK (cluster_key IN ({list}))")
}

/// Validate that `v` is one of the curated registry keys.
/// On error, returns the canonical error message including all allowed values.
pub fn validate_value(v: &str) -> Result<(), String> {
    if CURATED_CLUSTER_KEYS.contains(&v) {
        Ok(())
    } else {
        let allowed = CURATED_CLUSTER_KEYS.join(", ");
        Err(format!(
            "unknown cluster_key '{v}'; allowed values: [{allowed}]"
        ))
    }
}

/// Compiled pattern cache (lazy-init).
fn compiled_patterns() -> &'static Vec<(&'static str, Regex)> {
    static CACHE: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    CACHE.get_or_init(|| {
        CURATED_CLUSTER_KEY_PATTERNS
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

/// `stores observations clusters` — single-shot grouping of open/ready
/// observations by cluster_key.
pub fn run_clusters_cmd(
    _schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: InvokerCtx,
) -> Result<()> {
    let json_flag = matches.get_flag("json");

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

    // Sort buckets: descending count; ties by key name; (uncategorized) always last.
    let mut buckets: Vec<Bucket> = bucket_map.into_iter().collect();
    buckets.sort_by(|(ka, va), (kb, vb)| {
        let a_uncat = ka == "(uncategorized)";
        let b_uncat = kb == "(uncategorized)";
        if a_uncat && !b_uncat {
            std::cmp::Ordering::Greater
        } else if !a_uncat && b_uncat {
            std::cmp::Ordering::Less
        } else {
            vb.len().cmp(&va.len()).then_with(|| ka.cmp(kb))
        }
    });

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
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        for (key, rows) in &buckets {
            println!("{key} ({})", rows.len());
            for (display_id, captured_at, summary) in rows.iter().take(5) {
                println!("  {display_id} {captured_at} {summary}");
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
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else {
        for (display_id, task_id, captured_at, summary) in &rows {
            let tid = task_id.as_deref().unwrap_or("(none)");
            println!("{display_id} task={tid} {captured_at} {summary}");
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
        for key in CURATED_CLUSTER_KEYS {
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
        for key in CURATED_CLUSTER_KEYS {
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

        for key in CURATED_CLUSTER_KEYS {
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
        for key in CURATED_CLUSTER_KEYS {
            assert!(
                err.contains(key),
                "error message must list all keys; missing '{key}': {err}"
            );
        }
    }

    // ---- clusters cmd: 3+2+5 fixture (AC1.5) ----

    #[test]
    fn clusters_fixture_3_2_5() {
        let conn = Connection::open_in_memory().unwrap();
        install_schemas(&conn);

        // 3 deploy-blocked rows (status=open)
        for i in 0..3u32 {
            insert_observation(
                &conn,
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
                &conn,
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
                &conn,
                &format!("L{:03}", i + 6),
                "open",
                None,
                None,
                &format!("2026-01-{:02}T10:00:00Z", i + 6),
                "untagged summary",
            );
        }

        // Verify bucket counts via direct query
        let mut bucket_map: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        let rows: Vec<(Option<String>,)> = {
            let mut stmt = conn
                .prepare(
                    "SELECT cluster_key FROM observations WHERE status IN ('open','ready')",
                )
                .unwrap();
            stmt.query_map([], |r| Ok((r.get::<_, Option<String>>(0)?,)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        for (ck,) in &rows {
            let key = ck.clone().unwrap_or_else(|| "(uncategorized)".to_string());
            *bucket_map.entry(key).or_insert(0) += 1;
        }
        assert_eq!(bucket_map.len(), 3, "expected 3 buckets: {bucket_map:?}");
        assert_eq!(bucket_map["deploy-blocked-merge-conflict"], 3);
        assert_eq!(bucket_map["silent-zombie-watchdog"], 2);
        assert_eq!(bucket_map["(uncategorized)"], 5);

        // Verify the cmd itself returns Ok (AC1.8: no --watch)
        use clap::{Arg, ArgAction, Command};
        let schema = Schema::from_yaml(OBSERVATIONS_YAML).unwrap();
        let cmd = Command::new("clusters")
            .arg(Arg::new("json").long("json").action(ArgAction::SetTrue));
        let matches = cmd.get_matches_from(["clusters"]);
        let result = run_clusters_cmd(&schema, &conn, &matches, invoker_auto());
        assert!(result.is_ok(), "clusters cmd must return Ok: {result:?}");
    }

    // ---- overdue-ready fixture (AC1.6) ----

    #[test]
    fn overdue_ready_fixture() {
        let conn = Connection::open_in_memory().unwrap();
        install_schemas(&conn);

        // Tasks in various states
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

        // 6 ready observations linked to each task
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

    // ---- check_clause_sql ----

    #[test]
    fn check_clause_sql_contains_all_keys() {
        let sql = check_clause_sql();
        assert!(sql.starts_with("CHECK (cluster_key IN ("), "format: {sql}");
        for key in CURATED_CLUSTER_KEYS {
            assert!(sql.contains(key), "check clause missing '{key}': {sql}");
        }
    }
}
