use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use crate::paths;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunnerStatsRow {
    pub role: String,
    pub harness_id: String,
    pub model_id: String,
    pub runs: i64,
    pub ok: i64,
    pub failed: i64,
    pub avg_duration_secs: f64,
    pub max_duration_secs: f64,
    pub tokens_in: i64,
    pub tokens_out: i64,
}

pub fn run(json: bool, display_id: Option<&str>) -> Result<()> {
    let db_path = paths::db_path()?;
    let conn = Connection::open(&db_path)
        .with_context(|| format!("opening stores db {}", db_path.display()))?;
    let rows = load_runner_stats(&conn, display_id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        print_text(&rows);
    }
    Ok(())
}

pub fn load_runner_stats(conn: &Connection, display_id: Option<&str>) -> Result<Vec<RunnerStatsRow>> {
    if !table_exists(conn, "agent_runs")? {
        return Ok(Vec::new());
    }

    let filter = if display_id.is_some() {
        "WHERE display_id = ?1"
    } else {
        ""
    };
    let sql = format!(
        "SELECT role,
                COALESCE(harness_id, ''),
                COALESCE(model_id, ''),
                COUNT(*) AS runs,
                SUM(CASE WHEN exit_code = 0 THEN 1 ELSE 0 END) AS ok,
                SUM(CASE WHEN exit_code = 0 THEN 0 ELSE 1 END) AS failed,
                AVG(COALESCE((julianday(ended_at)-julianday(started_at))*86400.0, 0.0)) AS avg_secs,
                MAX(COALESCE((julianday(ended_at)-julianday(started_at))*86400.0, 0.0)) AS max_secs,
                SUM(COALESCE(tokens_in, 0)) AS tokens_in,
                SUM(COALESCE(tokens_out, 0)) AS tokens_out
         FROM agent_runs
         {filter}
         GROUP BY role, COALESCE(harness_id, ''), COALESCE(model_id, '')
         ORDER BY role, runs DESC, harness_id, model_id"
    );
    let mut stmt = conn.prepare(&sql)?;

    let map_row = |r: &rusqlite::Row<'_>| {
        Ok(RunnerStatsRow {
            role: r.get(0)?,
            harness_id: r.get(1)?,
            model_id: r.get(2)?,
            runs: r.get(3)?,
            ok: r.get(4)?,
            failed: r.get(5)?,
            avg_duration_secs: r.get(6)?,
            max_duration_secs: r.get(7)?,
            tokens_in: r.get(8)?,
            tokens_out: r.get(9)?,
        })
    };

    let rows = if let Some(display_id) = display_id {
        stmt.query_map([display_id], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        stmt.query_map([], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(rows)
}

fn print_text(rows: &[RunnerStatsRow]) {
    println!("role\tharness\tmodel\truns\tok\tfailed\tavg_s\tmax_s\ttokens_in\ttokens_out");
    for r in rows {
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{:.1}\t{:.1}\t{}\t{}",
            r.role,
            dash(&r.harness_id),
            dash(&r.model_id),
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
    if s.is_empty() { "-" } else { s }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_by_role_harness_model() {
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
                transcript_path TEXT
            );
            INSERT INTO agent_runs (display_id,phase,cycle,role,model_id,harness_id,started_at,ended_at,exit_code,tokens_in,tokens_out)
            VALUES
              ('T1',1,1,'executor','opus','claude-code','2026-05-09T00:00:00Z','2026-05-09T00:01:00Z',0,10,20),
              ('T2',1,1,'executor','opus','claude-code','2026-05-09T00:00:00Z','2026-05-09T00:02:00Z',1,30,40),
              ('T3',1,1,'planner','sonnet','pi','2026-05-09T00:00:00Z','2026-05-09T00:00:30Z',0,NULL,NULL);",
        )
        .unwrap();

        let rows = load_runner_stats(&conn, None).unwrap();
        let executor = rows.iter().find(|r| r.role == "executor").unwrap();
        assert_eq!(executor.harness_id, "claude-code");
        assert_eq!(executor.model_id, "opus");
        assert_eq!(executor.runs, 2);
        assert_eq!(executor.ok, 1);
        assert_eq!(executor.failed, 1);
        assert_eq!(executor.tokens_in, 40);
        assert_eq!(executor.tokens_out, 60);
        assert!(executor.avg_duration_secs >= 89.0 && executor.avg_duration_secs <= 91.0);
    }
}
