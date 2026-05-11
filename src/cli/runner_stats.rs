use std::collections::HashSet;

use anyhow::{Context, Result};
use rusqlite::{params_from_iter, Connection};
use serde::Serialize;

use crate::paths;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunnerStatsFilters<'a> {
    pub display_id: Option<&'a str>,
    pub role: Option<&'a str>,
    pub harness: Option<&'a str>,
    pub model: Option<&'a str>,
    pub thinking: Option<&'a str>,
    pub since: Option<&'a str>,
    pub until: Option<&'a str>,
    pub include_dirty_data: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunnerStatsRow {
    pub role: String,
    pub harness_id: String,
    pub model_id: String,
    pub thinking_effort: String,
    pub runs: i64,
    pub ok: i64,
    pub failed: i64,
    pub avg_duration_secs: f64,
    pub max_duration_secs: f64,
    pub tokens_in: i64,
    pub tokens_out: i64,
}

pub fn run(json: bool, filters: RunnerStatsFilters<'_>) -> Result<()> {
    let db_path = paths::db_path()?;
    let conn = Connection::open(&db_path)
        .with_context(|| format!("opening stores db {}", db_path.display()))?;
    let rows = load_runner_stats(&conn, &filters)?;
    print_caveat(filters.include_dirty_data);
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print_text(&rows);
    }
    Ok(())
}

pub fn load_runner_stats(
    conn: &Connection,
    filters: &RunnerStatsFilters<'_>,
) -> Result<Vec<RunnerStatsRow>> {
    if !table_exists(conn, "agent_runs")? {
        return Ok(Vec::new());
    }

    let columns = table_columns(conn, "agent_runs")?;
    let thinking_expr = if columns.contains("effective_thinking_effort") {
        "COALESCE(effective_thinking_effort, '')"
    } else if columns.contains("configured_thinking_effort") {
        "COALESCE(configured_thinking_effort, '')"
    } else {
        "''"
    };

    let model_expr = if columns.contains("effective_model_id") {
        "COALESCE(effective_model_id, model_id, '')"
    } else {
        "COALESCE(model_id, '')"
    };

    let mut where_clauses: Vec<String> = Vec::new();
    let mut params: Vec<&str> = Vec::new();

    if let Some(display_id) = filters.display_id {
        where_clauses.push("display_id = ?".to_string());
        params.push(display_id);
    }
    if let Some(role) = filters.role {
        where_clauses.push("role = ?".to_string());
        params.push(role);
    }
    if let Some(harness) = filters.harness {
        where_clauses.push("COALESCE(harness_id, '') = ?".to_string());
        params.push(harness);
    }
    if let Some(model) = filters.model {
        where_clauses.push(format!("{model_expr} = ?"));
        params.push(model);
    }
    if let Some(thinking) = filters.thinking {
        where_clauses.push(format!("{thinking_expr} = ?"));
        params.push(thinking);
    }
    if let Some(since) = filters.since {
        where_clauses.push("started_at >= ?".to_string());
        params.push(since);
    }
    if let Some(until) = filters.until {
        where_clauses.push("started_at <= ?".to_string());
        params.push(until);
    }
    if !filters.include_dirty_data && columns.contains("payload_valid") {
        where_clauses.push("COALESCE(payload_valid, 1) = 1".to_string());
    }

    let filter = if where_clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_clauses.join(" AND "))
    };
    let sql = format!(
        "SELECT role,
                COALESCE(harness_id, ''),
                {model_expr} AS model_id,
                {thinking_expr} AS thinking_effort,
                COUNT(*) AS runs,
                SUM(CASE WHEN exit_code = 0 THEN 1 ELSE 0 END) AS ok,
                SUM(CASE WHEN exit_code = 0 THEN 0 ELSE 1 END) AS failed,
                AVG(COALESCE((julianday(ended_at)-julianday(started_at))*86400.0, 0.0)) AS avg_secs,
                MAX(COALESCE((julianday(ended_at)-julianday(started_at))*86400.0, 0.0)) AS max_secs,
                SUM(COALESCE(tokens_in, 0)) AS tokens_in,
                SUM(COALESCE(tokens_out, 0)) AS tokens_out
         FROM agent_runs
         {filter}
         GROUP BY role, COALESCE(harness_id, ''), {model_expr}, {thinking_expr}
         ORDER BY role, runs DESC, harness_id, model_id, {thinking_expr}"
    );
    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt
        .query_map(params_from_iter(params), |r| {
            Ok(RunnerStatsRow {
                role: r.get(0)?,
                harness_id: r.get(1)?,
                model_id: r.get(2)?,
                thinking_effort: r.get(3)?,
                runs: r.get(4)?,
                ok: r.get(5)?,
                failed: r.get(6)?,
                avg_duration_secs: r.get(7)?,
                max_duration_secs: r.get(8)?,
                tokens_in: r.get(9)?,
                tokens_out: r.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn print_caveat(include_dirty_data: bool) {
    if include_dirty_data {
        eprintln!(
            "warning: raw operational telemetry; dirty payload rows are included. Do not treat as statistical inference."
        );
    } else {
        eprintln!(
            "warning: raw operational telemetry; rows marked payload_valid=0 are excluded when available. Do not treat as statistical inference."
        );
    }
}

fn print_text(rows: &[RunnerStatsRow]) {
    println!(
        "role\tharness\tmodel\tthinking\truns\tok\tfailed\tavg_s\tmax_s\ttokens_in\ttokens_out"
    );
    for r in rows {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}\t{:.1}\t{}\t{}",
            r.role,
            dash(&r.harness_id),
            dash(&r.model_id),
            dash(&r.thinking_effort),
            r.runs,
            r.ok,
            r.failed,
            r.avg_duration_secs,
            r.max_duration_secs,
            r.tokens_in,
            r.tokens_out
        );
    }
}

fn dash(s: &str) -> &str {
    if s.is_empty() {
        "-"
    } else {
        s
    }
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .map_err(Into::into)
}

fn table_columns(conn: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_by_role_harness_model_and_thinking() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_runs (
                id INTEGER PRIMARY KEY,
                display_id TEXT,
                phase INTEGER,
                cycle INTEGER,
                role TEXT,
                model_id TEXT,
                harness_id TEXT,
                started_at TEXT,
                ended_at TEXT,
                exit_code INTEGER,
                tokens_in INTEGER,
                tokens_out INTEGER,
                prompt_cache_hits INTEGER,
                transcript_path TEXT,
                effective_thinking_effort TEXT,
                payload_valid INTEGER
            );
            INSERT INTO agent_runs (display_id,phase,cycle,role,model_id,harness_id,started_at,ended_at,exit_code,tokens_in,tokens_out,effective_thinking_effort,payload_valid)
            VALUES
              ('T1',1,1,'executor','opus','claude-code','2026-05-09T00:00:00Z','2026-05-09T00:01:00Z',0,10,20,'high',1),
              ('T2',1,1,'executor','opus','claude-code','2026-05-09T00:00:00Z','2026-05-09T00:02:00Z',1,30,40,'high',1),
              ('T3',1,1,'planner','sonnet','pi','2026-05-09T00:00:00Z','2026-05-09T00:00:30Z',0,NULL,NULL,'low',1);",
        )
        .unwrap();

        let rows = load_runner_stats(&conn, &RunnerStatsFilters::default()).unwrap();
        let executor = rows.iter().find(|r| r.role == "executor").unwrap();
        assert_eq!(executor.harness_id, "claude-code");
        assert_eq!(executor.model_id, "opus");
        assert_eq!(executor.thinking_effort, "high");
        assert_eq!(executor.runs, 2);
        assert_eq!(executor.ok, 1);
        assert_eq!(executor.failed, 1);
        assert_eq!(executor.tokens_in, 40);
        assert_eq!(executor.tokens_out, 60);
        assert!(executor.avg_duration_secs >= 89.0 && executor.avg_duration_secs <= 91.0);

        let json = serde_json::to_value(&rows).unwrap();
        assert_eq!(json[0]["thinking_effort"], "high");
        assert!(json[0].get("confidence_interval").is_none());
    }

    #[test]
    fn prefers_effective_model_for_grouping_and_filtering_when_present() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_runs (
                id INTEGER PRIMARY KEY,
                display_id TEXT,
                role TEXT,
                model_id TEXT,
                effective_model_id TEXT,
                harness_id TEXT,
                started_at TEXT,
                ended_at TEXT,
                exit_code INTEGER,
                tokens_in INTEGER,
                tokens_out INTEGER
            );
            INSERT INTO agent_runs (display_id,role,model_id,effective_model_id,harness_id,started_at,ended_at,exit_code,tokens_in,tokens_out)
            VALUES
              ('T1','executor','pi:default','gpt-5.5','pi','2026-05-09T00:00:00Z','2026-05-09T00:01:00Z',0,1,2);",
        )
        .unwrap();

        let rows = load_runner_stats(
            &conn,
            &RunnerStatsFilters {
                model: Some("gpt-5.5"),
                ..RunnerStatsFilters::default()
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].model_id, "gpt-5.5");
    }

    #[test]
    fn filters_time_role_harness_model_thinking_and_dirty_data() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_runs (
                id INTEGER PRIMARY KEY,
                display_id TEXT,
                role TEXT,
                model_id TEXT,
                harness_id TEXT,
                started_at TEXT,
                ended_at TEXT,
                exit_code INTEGER,
                tokens_in INTEGER,
                tokens_out INTEGER,
                effective_thinking_effort TEXT,
                payload_valid INTEGER
            );
            INSERT INTO agent_runs (display_id,role,model_id,harness_id,started_at,ended_at,exit_code,tokens_in,tokens_out,effective_thinking_effort,payload_valid)
            VALUES
              ('T1','executor','opus','pi','2026-05-09T00:00:00Z','2026-05-09T00:01:00Z',0,1,1,'high',1),
              ('T2','executor','opus','pi','2026-05-09T01:00:00Z','2026-05-09T01:01:00Z',0,2,2,'high',0),
              ('T3','executor','sonnet','pi','2026-05-09T01:00:00Z','2026-05-09T01:01:00Z',0,3,3,'high',1),
              ('T4','planner','opus','pi','2026-05-09T01:00:00Z','2026-05-09T01:01:00Z',0,4,4,'high',1),
              ('T5','executor','opus','claude-code','2026-05-09T01:00:00Z','2026-05-09T01:01:00Z',0,5,5,'high',1),
              ('T6','executor','opus','pi','2026-05-09T02:00:00Z','2026-05-09T02:01:00Z',0,6,6,'low',1);",
        )
        .unwrap();

        let filters = RunnerStatsFilters {
            role: Some("executor"),
            harness: Some("pi"),
            model: Some("opus"),
            thinking: Some("high"),
            since: Some("2026-05-09T00:30:00Z"),
            until: Some("2026-05-09T01:30:00Z"),
            include_dirty_data: false,
            ..RunnerStatsFilters::default()
        };
        let rows = load_runner_stats(&conn, &filters).unwrap();
        assert!(rows.is_empty(), "dirty matching row is excluded by default");

        let rows = load_runner_stats(
            &conn,
            &RunnerStatsFilters {
                include_dirty_data: true,
                ..filters
            },
        )
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].runs, 1);
        assert_eq!(rows[0].tokens_in, 2);
    }
}
