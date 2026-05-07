use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone)]
pub struct MetricsArgs {
    pub window: String,
    pub text: bool,
    pub json: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct MetricsReport {
    pub window: WindowReport,
    pub per_edge: Vec<EdgeMetric>,
    pub ratification_cycle_time: RatificationMetrics,
    pub revise_rate: ReviseSection,
    pub agent_runs: AgentRunsSection,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct WindowReport {
    pub requested: String,
    pub since: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct EdgeMetric {
    pub store: String,
    pub edge: String,
    pub count: usize,
    pub p50_seconds: Option<i64>,
    pub p95_seconds: Option<i64>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct RatificationMetrics {
    pub open_to_ready: PercentileMetric,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct PercentileMetric {
    pub count: usize,
    pub p50_seconds: Option<i64>,
    pub p95_seconds: Option<i64>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ReviseSection {
    pub rows: Vec<ReviseMetric>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct ReviseMetric {
    pub phase: String,
    pub tier_hint: String,
    pub task_type: String,
    pub revise_count: usize,
    pub total_reviews: usize,
    pub revise_rate: f64,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AgentRunsSection {
    pub rows: Vec<AgentRunMetric>,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct AgentRunMetric {
    pub role: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone)]
struct TransitionRow {
    store: String,
    display_id: String,
    from_status: Option<String>,
    to_status: String,
    occurred_at: String,
    occurred_epoch: i64,
}

pub fn run(args: MetricsArgs) -> Result<()> {
    let db_path = crate::paths::db_path()?;
    let conn = crate::db::open(&db_path).context("open .stores/db.sqlite")?;
    let report = build_report(&conn, &args.window)?;
    if args.text && !args.json {
        print!("{}", render_text(&report));
    } else {
        println!("{}", render_json(&report)?);
    }
    Ok(())
}

pub fn build_report(conn: &Connection, window: &str) -> Result<MetricsReport> {
    let (window_report, since) = parse_window(window)?;
    let since_epoch = epoch_seconds(&since).unwrap_or(i64::MIN);
    let rows = load_transition_rows(conn)?;
    Ok(MetricsReport {
        window: window_report,
        per_edge: per_edge_metrics(&rows, since_epoch),
        ratification_cycle_time: RatificationMetrics {
            open_to_ready: ratification_metric(&rows, since_epoch),
        },
        revise_rate: revise_metrics(conn)?,
        agent_runs: agent_run_metrics(conn, &since)?,
    })
}

fn parse_window(window: &str) -> Result<(WindowReport, String)> {
    if epoch_seconds(window).is_some() {
        return Ok((
            WindowReport {
                requested: window.to_string(),
                since: window.to_string(),
            },
            window.to_string(),
        ));
    }
    let secs = parse_duration_seconds(window)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    Ok((
        WindowReport {
            requested: window.to_string(),
            since: format!("now-{window}"),
        },
        format_epoch_utc(now - secs),
    ))
}

fn parse_duration_seconds(window: &str) -> Result<i64> {
    let (num, unit) = window.split_at(window.len().saturating_sub(1));
    let n: i64 = num
        .parse()
        .with_context(|| format!("invalid --window '{window}'"))?;
    match unit {
        "s" => Ok(n),
        "m" => Ok(n * 60),
        "h" => Ok(n * 3600),
        "d" => Ok(n * 86400),
        _ => bail!("invalid --window '{window}'; use duration like 1h or RFC3339 timestamp"),
    }
}

fn load_transition_rows(conn: &Connection) -> Result<Vec<TransitionRow>> {
    let mut stmt = conn.prepare(
        "SELECT store, display_id, from_status, to_status, occurred_at
         FROM transition_history
         ORDER BY store ASC, display_id ASC, occurred_at ASC, rowid ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        let occurred_at: String = r.get(4)?;
        Ok(TransitionRow {
            store: r.get(0)?,
            display_id: r.get(1)?,
            from_status: r.get(2)?,
            to_status: r.get(3)?,
            occurred_epoch: epoch_seconds(&occurred_at).unwrap_or(0),
            occurred_at,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn per_edge_metrics(rows: &[TransitionRow], since_epoch: i64) -> Vec<EdgeMetric> {
    let mut previous_by_item: HashMap<(&str, &str), &TransitionRow> = HashMap::new();
    let mut grouped: BTreeMap<(String, String), Vec<i64>> = BTreeMap::new();
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for row in rows {
        let edge = format!(
            "{} -> {}",
            row.from_status.as_deref().unwrap_or("(create)"),
            row.to_status
        );
        let key = (row.store.clone(), edge);
        if row.occurred_epoch >= since_epoch {
            *counts.entry(key.clone()).or_insert(0) += 1;
            if let Some(prev) = previous_by_item.get(&(row.store.as_str(), row.display_id.as_str()))
            {
                grouped
                    .entry(key)
                    .or_default()
                    .push(row.occurred_epoch - prev.occurred_epoch);
            }
        }
        previous_by_item.insert((row.store.as_str(), row.display_id.as_str()), row);
    }
    counts
        .into_iter()
        .map(|((store, edge), count)| {
            let vals = grouped
                .remove(&(store.clone(), edge.clone()))
                .unwrap_or_default();
            EdgeMetric {
                store,
                edge,
                count,
                p50_seconds: percentile(&vals, 50),
                p95_seconds: percentile(&vals, 95),
            }
        })
        .collect()
}

fn ratification_metric(rows: &[TransitionRow], since_epoch: i64) -> PercentileMetric {
    let mut by_item: BTreeMap<(String, String), Vec<&TransitionRow>> = BTreeMap::new();
    for row in rows {
        by_item
            .entry((row.store.clone(), row.display_id.clone()))
            .or_default()
            .push(row);
    }
    let mut vals = Vec::new();
    for (_item, mut rs) in by_item {
        rs.sort_by_key(|r| (&r.occurred_at, r.occurred_epoch));
        let open = rs
            .iter()
            .find(|r| r.to_status == "open")
            .or_else(|| rs.iter().find(|r| r.from_status.as_deref() == Some("open")));
        let ready = rs.iter().find(|r| r.to_status == "ready");
        if let (Some(o), Some(r)) = (open, ready) {
            if r.occurred_epoch >= since_epoch && r.occurred_epoch >= o.occurred_epoch {
                vals.push(r.occurred_epoch - o.occurred_epoch);
            }
        }
    }
    PercentileMetric {
        count: vals.len(),
        p50_seconds: percentile(&vals, 50),
        p95_seconds: percentile(&vals, 95),
    }
}

fn revise_metrics(conn: &Connection) -> Result<ReviseSection> {
    let mut notes = Vec::new();
    if !table_exists(conn, "tasks")? || !column_exists(conn, "tasks", "cycles")? {
        return Ok(ReviseSection {
            rows: vec![],
            notes: vec!["tasks.cycles unavailable; revise_rate source unavailable".into()],
        });
    }
    let has_task_type =
        column_exists(conn, "tasks", "task_type")? || column_exists(conn, "tasks", "type")?;
    if !has_task_type {
        notes.push("task_type source unavailable; using task_type=unknown".into());
    }
    let tier_col = if column_exists(conn, "tasks", "tier_hint")? {
        "tier_hint"
    } else {
        "'unknown'"
    };
    let type_expr = if column_exists(conn, "tasks", "task_type")? {
        "task_type"
    } else if column_exists(conn, "tasks", "type")? {
        "type"
    } else {
        "'unknown'"
    };
    let sql = format!("SELECT cycles, COALESCE({tier_col}, 'unknown'), COALESCE({type_expr}, 'unknown') FROM tasks");
    let mut stmt = conn.prepare(&sql)?;
    let mut grouped: BTreeMap<(String, String, String), (usize, usize)> = BTreeMap::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Option<String>>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (cycles, tier, task_type) = row?;
        let Some(cycles) = cycles else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&cycles) else {
            continue;
        };
        let mut reviews = Vec::new();
        collect_reviews(&v, &mut reviews);
        for (phase, verdict) in reviews {
            let entry = grouped
                .entry((phase, tier.clone(), task_type.clone()))
                .or_insert((0, 0));
            entry.1 += 1;
            if verdict == "REVISE" {
                entry.0 += 1;
            }
        }
    }
    let rows = grouped
        .into_iter()
        .map(
            |((phase, tier_hint, task_type), (revise_count, total_reviews))| ReviseMetric {
                phase,
                tier_hint,
                task_type,
                revise_count,
                total_reviews,
                revise_rate: if total_reviews == 0 {
                    0.0
                } else {
                    revise_count as f64 / total_reviews as f64
                },
            },
        )
        .collect();
    Ok(ReviseSection { rows, notes })
}

fn collect_reviews(v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(m) => {
            let phase = m
                .get("phase")
                .or_else(|| m.get("phase_name"))
                .and_then(|p| {
                    if let Some(s) = p.as_str() {
                        Some(s.to_string())
                    } else if let Some(n) = p.as_i64() {
                        Some(n.to_string())
                    } else {
                        p.as_u64().map(|n| n.to_string())
                    }
                });
            let review = m.get("review").and_then(Value::as_object);
            let verdict = review
                .and_then(|r| {
                    r.get("gate")
                        .or_else(|| r.get("verdict"))
                        .or_else(|| r.get("result"))
                        .or_else(|| r.get("status"))
                        .or_else(|| r.get("outcome"))
                })
                .or_else(|| m.get("gate"))
                .or_else(|| m.get("verdict"))
                .or_else(|| m.get("result"))
                .or_else(|| m.get("status"))
                .or_else(|| m.get("outcome"))
                .and_then(Value::as_str);
            if let (Some(p), Some(verdict)) = (phase, verdict) {
                let verdict = verdict.to_ascii_uppercase();
                if verdict == "PASS" || verdict == "REVISE" {
                    out.push((p, verdict));
                }
            }
            for child in m.values() {
                collect_reviews(child, out);
            }
        }
        Value::Array(a) => {
            for child in a {
                collect_reviews(child, out);
            }
        }
        _ => {}
    }
}

fn agent_run_metrics(conn: &Connection, since: &str) -> Result<AgentRunsSection> {
    if !table_exists(conn, "agent_runs")? {
        return Ok(AgentRunsSection {
            rows: vec![],
            notes: vec!["agent_runs not yet captured".into()],
        });
    }
    let cols = table_columns(conn, "agent_runs")?;
    let role = pick_col(&cols, &["role", "agent_role"]);
    let ts = pick_col(&cols, &["occurred_at", "started_at", "finished_at"]);
    let input = pick_col(&cols, &["input_tokens"]);
    let output = pick_col(&cols, &["output_tokens"]);
    let total = pick_col(&cols, &["total_tokens"]);
    let (Some(role), Some(ts)) = (role, ts) else {
        return Ok(AgentRunsSection {
            rows: vec![],
            notes: vec!["agent_runs schema not recognized".into()],
        });
    };
    if input.is_none() && output.is_none() && total.is_none() {
        return Ok(AgentRunsSection {
            rows: vec![],
            notes: vec!["agent_runs schema not recognized".into()],
        });
    }
    let i = input.unwrap_or_else(|| "0".to_string());
    let o = output.unwrap_or_else(|| "0".to_string());
    let total_expr = total.unwrap_or_else(|| format!("COALESCE({i},0)+COALESCE({o},0)"));
    let sql = format!("SELECT {role}, SUM(COALESCE({i},0)), SUM(COALESCE({o},0)), SUM(COALESCE({total_expr}, COALESCE({i},0)+COALESCE({o},0))) FROM agent_runs WHERE {ts} >= ?1 GROUP BY {role} ORDER BY {role} ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([since], |r| {
        Ok(AgentRunMetric {
            role: r.get(0)?,
            input_tokens: r.get::<_, Option<i64>>(1)?.unwrap_or(0),
            output_tokens: r.get::<_, Option<i64>>(2)?.unwrap_or(0),
            total_tokens: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
        })
    })?;
    Ok(AgentRunsSection {
        rows: rows.collect::<rusqlite::Result<Vec<_>>>()?,
        notes: vec![],
    })
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |r| r.get::<_, i64>(0),
    )? > 0)
}
fn column_exists(conn: &Connection, table: &str, col: &str) -> Result<bool> {
    Ok(table_columns(conn, table)?.iter().any(|c| c == col))
}
fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}
fn pick_col(cols: &[String], names: &[&str]) -> Option<String> {
    names
        .iter()
        .find(|n| cols.iter().any(|c| c == **n))
        .map(|s| s.to_string())
}

fn percentile(vals: &[i64], pct: usize) -> Option<i64> {
    if vals.is_empty() {
        return None;
    }
    let mut v = vals.to_vec();
    v.sort_unstable();
    let idx = ((pct * v.len()).div_ceil(100)).saturating_sub(1);
    v.get(idx).copied()
}

pub fn render_json(report: &MetricsReport) -> Result<String> {
    Ok(serde_json::to_string_pretty(report)?)
}

pub fn render_text(report: &MetricsReport) -> String {
    let mut out = String::new();
    out.push_str(&format!("metrics window since {}\n", report.window.since));
    out.push_str("per_edge\n");
    for r in &report.per_edge {
        out.push_str(&format!(
            "  store={} edge={} count={} p50_seconds={:?} p95_seconds={:?}\n",
            r.store, r.edge, r.count, r.p50_seconds, r.p95_seconds
        ));
    }
    let rat = &report.ratification_cycle_time.open_to_ready;
    out.push_str(&format!(
        "ratification_cycle_time.open_to_ready count={} p50_seconds={:?} p95_seconds={:?}\n",
        rat.count, rat.p50_seconds, rat.p95_seconds
    ));
    out.push_str("revise_rate\n");
    for r in &report.revise_rate.rows {
        out.push_str(&format!("  phase={} tier_hint={} task_type={} revise_count={} total_reviews={} revise_rate={:.3}\n", r.phase, r.tier_hint, r.task_type, r.revise_count, r.total_reviews, r.revise_rate));
    }
    for n in &report.revise_rate.notes {
        out.push_str(&format!("  note={}\n", n));
    }
    out.push_str("agent_runs\n");
    for r in &report.agent_runs.rows {
        out.push_str(&format!(
            "  role={} input_tokens={} output_tokens={} total_tokens={}\n",
            r.role, r.input_tokens, r.output_tokens, r.total_tokens
        ));
    }
    for n in &report.agent_runs.notes {
        out.push_str(&format!("  note={}\n", n));
    }
    out
}

fn epoch_seconds(s: &str) -> Option<i64> {
    let s = s.strip_suffix('Z').unwrap_or(s);
    let (date, time) = s.split_once('T').or_else(|| s.split_once(' '))?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let y = d.next()??;
    let m = d.next()??;
    let day = d.next()??;
    let time = time.split('.').next().unwrap_or(time);
    let mut t = time.split(':').map(|p| p.parse::<i64>().ok());
    let hh = t.next()??;
    let mm = t.next()??;
    let ss = t.next()??;
    Some(days_from_civil(y, m, day) * 86400 + hh * 3600 + mm * 60 + ss)
}

fn format_epoch_utc(epoch: i64) -> String {
    let days = epoch.div_euclid(86400);
    let secs = epoch.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - (m <= 2) as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    (y + (m <= 2) as i64, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE transition_history (store TEXT, display_id TEXT, from_status TEXT, to_status TEXT, occurred_at TEXT);").unwrap();
        c
    }
    fn ins(c: &Connection, store: &str, id: &str, from: Option<&str>, to: &str, ts: &str) {
        c.execute(
            "INSERT INTO transition_history VALUES (?1,?2,?3,?4,?5)",
            (store, id, from, to, ts),
        )
        .unwrap();
    }

    #[test]
    fn transition_history_per_edge_percentiles_are_exact() {
        let c = conn();
        ins(&c, "tasks", "T1", None, "planning", "2026-01-01T00:00:00Z");
        ins(
            &c,
            "tasks",
            "T1",
            Some("planning"),
            "plan_review",
            "2026-01-01T00:02:00Z",
        );
        ins(&c, "tasks", "T2", None, "planning", "2026-01-01T00:00:00Z");
        ins(
            &c,
            "tasks",
            "T2",
            Some("planning"),
            "plan_review",
            "2026-01-01T00:04:00Z",
        );
        let r = build_report(&c, "2025-12-31T00:00:00Z").unwrap();
        let edge = r
            .per_edge
            .iter()
            .find(|e| e.store == "tasks" && e.edge == "planning -> plan_review")
            .unwrap();
        assert_eq!(
            (edge.count, edge.p50_seconds, edge.p95_seconds),
            (2, Some(120), Some(240))
        );
    }

    #[test]
    fn ratification_open_to_ready_percentiles_are_exact() {
        let c = conn();
        ins(&c, "tasks", "T1", None, "open", "2026-01-01T00:00:00Z");
        ins(
            &c,
            "tasks",
            "T1",
            Some("open"),
            "ready",
            "2026-01-01T00:10:00Z",
        );
        ins(&c, "tasks", "T2", None, "open", "2026-01-01T00:00:00Z");
        ins(
            &c,
            "tasks",
            "T2",
            Some("open"),
            "ready",
            "2026-01-01T00:20:00Z",
        );
        let r = build_report(&c, "2025-12-31T00:00:00Z").unwrap();
        assert_eq!(
            r.ratification_cycle_time.open_to_ready,
            PercentileMetric {
                count: 2,
                p50_seconds: Some(600),
                p95_seconds: Some(1200)
            }
        );
    }

    #[test]
    fn revise_rate_groups_by_phase_tier_unknown_task_type() {
        let c = conn();
        c.execute_batch("CREATE TABLE tasks (display_id TEXT, tier_hint TEXT, cycles TEXT);")
            .unwrap();
        c.execute("INSERT INTO tasks VALUES ('T1','T1',?1)", [r#"[{"phase":1,"cycle":1,"review":{"gate":"PASS"}},{"phase":1,"cycle":2,"review":{"gate":"REVISE"}},{"phase":2,"cycle":1,"review":{"gate":"PASS"}}]"#]).unwrap();
        let r = build_report(&c, "2025-12-31T00:00:00Z").unwrap();
        let row = r
            .revise_rate
            .rows
            .iter()
            .find(|m| m.phase == "1" && m.tier_hint == "T1")
            .unwrap();
        assert_eq!(
            (row.task_type.as_str(), row.revise_count, row.total_reviews),
            ("unknown", 1, 2)
        );
        assert_eq!(row.revise_rate, 0.5);
        assert!(r
            .revise_rate
            .notes
            .iter()
            .any(|n| n.contains("task_type=unknown")));
    }

    #[test]
    fn agent_runs_absent_and_minimal_shape() {
        let c = conn();
        let r = build_report(&c, "2025-12-31T00:00:00Z").unwrap();
        assert!(r
            .agent_runs
            .notes
            .contains(&"agent_runs not yet captured".to_string()));
        c.execute_batch("CREATE TABLE agent_runs (role TEXT, occurred_at TEXT, input_tokens INTEGER, output_tokens INTEGER, total_tokens INTEGER);").unwrap();
        c.execute(
            "INSERT INTO agent_runs VALUES ('executor','2026-01-01T00:00:00Z',10,20,30)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO agent_runs VALUES ('executor','2026-01-01T00:01:00Z',1,2,3)",
            [],
        )
        .unwrap();
        let r = build_report(&c, "2025-12-31T00:00:00Z").unwrap();
        assert_eq!(
            r.agent_runs.rows[0],
            AgentRunMetric {
                role: "executor".into(),
                input_tokens: 11,
                output_tokens: 22,
                total_tokens: 33
            }
        );
    }

    #[test]
    fn json_shape_stable_across_reruns() {
        let c = conn();
        ins(&c, "tasks", "T1", None, "planning", "2026-01-01T00:00:00Z");
        let a = render_json(&build_report(&c, "2025-12-31T00:00:00Z").unwrap()).unwrap();
        let b = render_json(&build_report(&c, "2025-12-31T00:00:00Z").unwrap()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn duration_window_json_omits_volatile_clock_cutoff() {
        let c = conn();
        let a = render_json(&build_report(&c, "1h").unwrap()).unwrap();
        let b = render_json(&build_report(&c, "1h").unwrap()).unwrap();
        assert_eq!(a, b);
        assert!(a.contains(r#""requested": "1h""#));
        assert!(a.contains(r#""since": "now-1h""#));
    }

    #[test]
    fn text_output_contains_expected_sections() {
        let c = conn();
        ins(&c, "tasks", "T1", None, "open", "2026-01-01T00:00:00Z");
        ins(
            &c,
            "tasks",
            "T1",
            Some("open"),
            "ready",
            "2026-01-01T00:05:00Z",
        );
        let txt = render_text(&build_report(&c, "2025-12-31T00:00:00Z").unwrap());
        assert!(txt.contains("per_edge"));
        assert!(txt.contains("store=tasks edge=open -> ready count=1"));
        assert!(txt.contains("ratification_cycle_time.open_to_ready count=1"));
        assert!(txt.contains("revise_rate"));
        assert!(txt.contains("agent_runs"));
        assert!(txt.contains("agent_runs not yet captured"));
    }
}
