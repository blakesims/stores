use anyhow::{bail, Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone)]
pub struct MetricsArgs {
    pub window: String,
    pub text: bool,
    pub json: bool,
    /// Override wall-clock "now" for duration windows; RFC3339 string.
    /// When absent, actual wall clock is used.  Supplied by --now flag or tests.
    pub now: Option<String>,
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
    /// Absolute UTC cutoff resolved at invocation start (RFC3339).
    /// Always populated for all window forms so reruns can reproduce the exact
    /// cutoff by passing since_resolved as an absolute `--since`.
    pub since_resolved: String,
    /// True when the cutoff was derived from wall-clock time (bare duration
    /// window without --now).  Consecutive runs with a bare duration window
    /// will have the same data but differing since_resolved values.
    ///
    /// False when the cutoff is deterministic: absolute `--since` input OR
    /// duration + `--now` override.  Stable-output acceptance applies only to
    /// volatile_window=false runs.
    pub volatile_window: bool,
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
    let report = build_report(&conn, &args.window, args.now.as_deref())?;
    if args.text && !args.json {
        print!("{}", render_text(&report));
    } else {
        println!("{}", render_json(&report)?);
    }
    Ok(())
}

/// Build a metrics report.
///
/// `now_override`: when `Some`, uses this RFC3339 timestamp as "now" for
/// resolving duration windows (e.g. "1h").  When `None`, uses actual wall
/// clock.  Supplying a fixed value makes output deterministic across reruns.
pub fn build_report(conn: &Connection, window: &str, now_override: Option<&str>) -> Result<MetricsReport> {
    let (window_report, since) = parse_window(window, now_override)?;
    let since_epoch = epoch_seconds(&since).unwrap_or(i64::MIN);
    // Normalize the since cutoff to canonical UTC RFC3339 for SQL text comparison.
    // parse_window returns UTC strings for duration windows already; for absolute
    // RFC3339 inputs with non-UTC offsets (e.g. -08:00) we must convert to UTC
    // so SQLite's lexicographic >= works correctly against stored UTC timestamps.
    let normalized_utc_cutoff = normalize_to_utc(&since)
        .unwrap_or_else(|| since.clone());
    let rows = load_transition_rows(conn)?;
    Ok(MetricsReport {
        window: window_report,
        per_edge: per_edge_metrics(&rows, since_epoch),
        ratification_cycle_time: RatificationMetrics {
            open_to_ready: ratification_metric(&rows, since_epoch),
        },
        revise_rate: revise_metrics(conn, since_epoch, window)?,
        agent_runs: agent_run_metrics(conn, &normalized_utc_cutoff)?,
    })
}

fn parse_window(window: &str, now_override: Option<&str>) -> Result<(WindowReport, String)> {
    if epoch_seconds(window).is_some() {
        // Absolute RFC3339 input: normalize to UTC for consistent display.
        let resolved = normalize_to_utc(window).unwrap_or_else(|| window.to_string());
        return Ok((
            WindowReport {
                requested: window.to_string(),
                since: window.to_string(),
                since_resolved: resolved.clone(),
                // Absolute --since is always deterministic.
                volatile_window: false,
            },
            resolved,
        ));
    }
    let secs = parse_duration_seconds(window)?;
    // Resolve "now" ONCE at invocation start — either from the caller-supplied
    // override (deterministic) or the actual wall clock (volatile).
    let (now_epoch, volatile) = if let Some(override_str) = now_override {
        let e = epoch_seconds(override_str)
            .with_context(|| format!("--now value is not a valid RFC3339 timestamp: '{override_str}'"))?;
        (e, false)
    } else {
        let e = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;
        (e, true)
    };
    // Absolute cutoff resolved once — used for all SQL filtering in this invocation.
    let resolved = format_epoch_utc(now_epoch - secs);
    Ok((
        WindowReport {
            requested: window.to_string(),
            since: format!("now-{window}"),
            // Always emit since_resolved so the caller can reproduce this exact
            // cutoff by passing it as absolute --since.
            since_resolved: resolved.clone(),
            volatile_window: volatile,
        },
        resolved,
    ))
}

fn parse_duration_seconds(window: &str) -> Result<i64> {
    let (num, unit) = window.split_at(window.len().saturating_sub(1));
    let n: i64 = num
        .parse()
        .with_context(|| format!("invalid --window '{window}'"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        "d" => n * 86400,
        _ => bail!("invalid --window '{window}'; use duration like 1h or RFC3339 timestamp"),
    };
    if secs <= 0 {
        bail!("window must be positive (got '{window}')");
    }
    Ok(secs)
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
        // Skip no-op audit rows: from_status == to_status are audit events that
        // carry no lifecycle meaning.  They must NOT count toward edge counts or
        // edge latencies, and must NOT advance the "previous row" cursor so that
        // subsequent real lifecycle edges compute latency against the last REAL row.
        let is_noop = row
            .from_status
            .as_deref()
            .map(|f| f == row.to_status.as_str())
            .unwrap_or(false);
        if is_noop {
            continue;
        }

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
        rs.sort_by_key(|r| (r.occurred_epoch, r.occurred_at.clone()));
        // Collect 'open' rows in chronological order.
        // Prefer rows where to_status == "open" (explicit arrival in open state).
        // Fall back to rows where from_status == "open" (departure from open state)
        // only when no to_status=="open" rows exist — this matches the original
        // single-cycle heuristic for stores that log open→ready but not the
        // initial (create)→open transition.
        let open_rows: Vec<&TransitionRow> = {
            let direct: Vec<&TransitionRow> =
                rs.iter().copied().filter(|r| r.to_status == "open").collect();
            if direct.is_empty() {
                rs.iter()
                    .copied()
                    .filter(|r| r.from_status.as_deref() == Some("open"))
                    .collect()
            } else {
                direct
            }
        };
        // Walk open rows in order; for each, find the first 'ready' AT OR AFTER
        // the open's timestamp that hasn't been consumed by a prior open.
        let mut consumed_ready_idx: Option<usize> = None;
        for open_row in open_rows {
            let candidate = rs
                .iter()
                .enumerate()
                .filter(|(idx, r)| {
                    r.to_status == "ready"
                        && r.occurred_epoch >= open_row.occurred_epoch
                        && consumed_ready_idx.map(|c| *idx > c).unwrap_or(true)
                })
                .min_by_key(|(_, r)| r.occurred_epoch);
            if let Some((idx, ready_row)) = candidate {
                if ready_row.occurred_epoch >= since_epoch {
                    vals.push(ready_row.occurred_epoch - open_row.occurred_epoch);
                }
                consumed_ready_idx = Some(idx);
            }
        }
    }
    PercentileMetric {
        count: vals.len(),
        p50_seconds: percentile(&vals, 50),
        p95_seconds: percentile(&vals, 95),
    }
}

/// Compute REVISE rate from transition_history (windowed) or fall back to
/// `tasks.cycles` JSON (unwindowed).
///
/// **Primary path — transition_history:** `submit-review` and `submit-plan-review`
/// rows carry `occurred_at` timestamps, so the `--window` filter applies exactly.
/// Verdict mapping:
///   - submit-plan-review → ready          = PASS (plan review)
///   - submit-plan-review → planning|blocked = REVISE (plan review)
///   - submit-review      → complete       = PASS (code review)
///   - submit-review      → executing|blocked = REVISE (code review)
///
/// **Fallback — tasks.cycles:** used only when transition_history lacks the
/// relevant review-verb rows.  Cycles JSON has no per-event timestamps; data is
/// all-time regardless of --window.  A note is appended to make this visible.
fn revise_metrics(conn: &Connection, since_epoch: i64, window: &str) -> Result<ReviseSection> {
    // Try transition_history-based windowed computation first.
    if table_exists(conn, "transition_history")? {
        let th_cols = table_columns(conn, "transition_history")?;
        let has_verb = th_cols.iter().any(|c| c == "verb");
        let has_to = th_cols.iter().any(|c| c == "to_status");
        let has_ts = th_cols.iter().any(|c| c == "occurred_at");
        let has_store = th_cols.iter().any(|c| c == "store");
        if has_verb && has_to && has_ts {
            return revise_metrics_from_transition_history(conn, since_epoch, window, has_store);
        }
    }
    // Fallback: tasks.cycles (unwindowed).
    revise_metrics_from_cycles(conn, since_epoch, window)
}

/// Windowed REVISE rate derived from transition_history review verbs.
///
/// JOINs the `tasks` table (when present) by `display_id` to obtain
/// `task_type` so each REVISE-rate row is keyed by `(phase, task_type)`.
/// Falls back to `task_type = "unknown"` when the tasks table is absent
/// or a given display_id has no matching task row.
fn revise_metrics_from_transition_history(
    conn: &Connection,
    since_epoch: i64,
    window: &str,
    has_store: bool,
) -> Result<ReviseSection> {
    let store_filter = if has_store {
        "AND th.store = 'tasks'"
    } else {
        ""
    };
    // Convert since_epoch back to RFC3339 string for SQLite text comparison.
    // SQLite stores occurred_at as ISO-8601 text; lexicographic comparison works
    // for UTC timestamps (YYYY-MM-DDTHH:MM:SSZ).
    let since_str = if since_epoch == i64::MIN {
        "0000-00-00".to_string()
    } else {
        chrono::DateTime::from_timestamp(since_epoch, 0)
            .map(|dt: DateTime<Utc>| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_else(|| "0000-00-00".to_string())
    };

    // Determine whether tasks table exists and which column holds task_type.
    let tasks_type_expr = if table_exists(conn, "tasks")? {
        let tasks_cols = table_columns(conn, "tasks")?;
        if tasks_cols.iter().any(|c| c == "task_type") {
            Some("COALESCE(t.task_type, 'unknown')")
        } else if tasks_cols.iter().any(|c| c == "type") {
            Some("COALESCE(t.type, 'unknown')")
        } else {
            None
        }
    } else {
        None
    };

    let (join_clause, type_col) = if let Some(expr) = tasks_type_expr {
        (
            "LEFT JOIN tasks t ON t.display_id = th.display_id",
            expr.to_string(),
        )
    } else {
        ("", "'unknown'".to_string())
    };

    let sql = format!(
        "SELECT th.verb, th.to_status, {type_col} AS task_type \
         FROM transition_history th \
         {join_clause} \
         WHERE th.verb IN ('submit-review','submit-plan-review') \
         AND th.occurred_at >= ?1 \
         {store_filter}"
    );
    let mut stmt = conn.prepare(&sql)?;
    // Group by (phase, tier_hint="unknown", task_type).
    let mut grouped: BTreeMap<(String, String, String), (usize, usize)> = BTreeMap::new();
    let rows = stmt.query_map([&since_str], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (verb, to_status, task_type) = row?;
        let (phase, is_revise) = match (verb.as_str(), to_status.as_str()) {
            ("submit-plan-review", "ready") => ("plan_review".to_string(), false),
            ("submit-plan-review", _) => ("plan_review".to_string(), true),
            ("submit-review", "complete") => ("code_review".to_string(), false),
            ("submit-review", _) => ("code_review".to_string(), true),
            _ => continue,
        };
        let entry = grouped
            .entry((phase, "unknown".to_string(), task_type))
            .or_insert((0, 0));
        entry.1 += 1;
        if is_revise {
            entry.0 += 1;
        }
    }
    let rows_out = grouped
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
    Ok(ReviseSection {
        rows: rows_out,
        notes: vec![format!(
            "revise_rate source: transition_history (windowed; --window={window} applied)"
        )],
    })
}

/// Fallback: compute REVISE rate from `tasks.cycles` JSON (unwindowed).
fn revise_metrics_from_cycles(conn: &Connection, _since_epoch: i64, window: &str) -> Result<ReviseSection> {
    let mut notes = Vec::new();
    // Window note: cycles JSON lacks per-review timestamps; data is all-time.
    notes.push(format!(
        "revise_rate source: tasks.cycles (unwindowed; --window={window} not applied \
         — per-review timestamps not stored in cycles JSON; \
         transition_history table absent or missing verb/to_status/occurred_at columns)"
    ));
    if !table_exists(conn, "tasks")? || !column_exists(conn, "tasks", "cycles")? {
        notes.push("tasks.cycles unavailable; revise_rate rows empty".into());
        return Ok(ReviseSection { rows: vec![], notes });
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

/// Compute the p-th percentile of `vals` using linear interpolation.
///
/// Uses the standard fractional-rank formula:
///   index = (p / 100.0) * (n - 1)
/// If index is an integer, return that element; otherwise interpolate between
/// floor(index) and ceil(index).  This gives the true median for even-count
/// series (e.g. [120, 240] → p50 = 180, not 120).
fn percentile(vals: &[i64], pct: usize) -> Option<i64> {
    if vals.is_empty() {
        return None;
    }
    let mut v = vals.to_vec();
    v.sort_unstable();
    let n = v.len();
    if n == 1 {
        return Some(v[0]);
    }
    let index = (pct as f64 / 100.0) * (n - 1) as f64;
    let lo = index.floor() as usize;
    let hi = index.ceil() as usize;
    if lo == hi {
        Some(v[lo])
    } else {
        let frac = index - lo as f64;
        Some((v[lo] as f64 + frac * (v[hi] as f64 - v[lo] as f64)).round() as i64)
    }
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

/// Normalize an RFC3339 timestamp to canonical UTC form `YYYY-MM-DDTHH:MM:SSZ`.
///
/// Handles any valid RFC3339 offset (Z, +00:00, -08:00, etc.) and returns the
/// equivalent UTC instant.  Returns `None` for malformed input.
/// This is used to ensure SQL text comparisons against stored UTC timestamps
/// are correct regardless of the offset in the caller-supplied window.
fn normalize_to_utc(s: &str) -> Option<String> {
    let normalised;
    let s = if s.contains(' ') && !s.contains('T') {
        normalised = s.replacen(' ', "T", 1);
        &normalised
    } else {
        s
    };
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| {
            dt.with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Secs, true)
        })
}

/// Parse an RFC3339 timestamp string to Unix epoch seconds (UTC).
///
/// Handles all valid RFC3339 forms:
///   - `2026-01-01T00:00:00Z`
///   - `2026-01-01T00:00:00+00:00`
///   - `2026-01-01T00:00:00-08:00`
///   - space separator instead of `T` (SQLite common form)
///
/// Returns `None` for malformed input.
fn epoch_seconds(s: &str) -> Option<i64> {
    // Normalise space-separator to 'T' for chrono compatibility.
    let normalised;
    let s = if s.contains(' ') && !s.contains('T') {
        normalised = s.replacen(' ', "T", 1);
        &normalised
    } else {
        s
    };
    // chrono handles Z and ±HH:MM offsets natively.
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).timestamp())
}

fn format_epoch_utc(epoch: i64) -> String {
    DateTime::from_timestamp(epoch, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| format!("{epoch}"))
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
        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();
        let edge = r
            .per_edge
            .iter()
            .find(|e| e.store == "tasks" && e.edge == "planning -> plan_review")
            .unwrap();
        // Linear interpolation: [120, 240], n=2
        //   p50: index = 0.50 * 1 = 0.5 → 120 + 0.5 * (240 - 120) = 180
        //   p95: index = 0.95 * 1 = 0.95 → 120 + 0.95 * (240 - 120) = 234
        assert_eq!(
            (edge.count, edge.p50_seconds, edge.p95_seconds),
            (2, Some(180), Some(234))
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
        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();
        // Linear interpolation: [600, 1200], n=2
        //   p50: index = 0.50 * 1 = 0.5 → 600 + 0.5 * (1200 - 600) = 900
        //   p95: index = 0.95 * 1 = 0.95 → 600 + 0.95 * (1200 - 600) = 1170
        assert_eq!(
            r.ratification_cycle_time.open_to_ready,
            PercentileMetric {
                count: 2,
                p50_seconds: Some(900),
                p95_seconds: Some(1170)
            }
        );
    }

    #[test]
    fn ratification_two_cycles_paired_correctly() {
        // Item with two complete open→ready cycles. Verifies at-or-after pairing:
        // cycle1: open1 (T=0) → ready1 (T=600) = 600s
        // cycle2: open2 (T=900) → ready2 (T=1800) = 900s
        let c = conn();
        ins(&c, "obs", "L1", None, "open", "2026-01-01T00:00:00Z");
        ins(
            &c,
            "obs",
            "L1",
            Some("open"),
            "ready",
            "2026-01-01T00:10:00Z",
        );
        ins(
            &c,
            "obs",
            "L1",
            Some("ready"),
            "open",
            "2026-01-01T00:15:00Z",
        );
        ins(
            &c,
            "obs",
            "L1",
            Some("open"),
            "ready",
            "2026-01-01T00:30:00Z",
        );
        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();
        // cycle1 = 600s, cycle2 = 900s → count=2, p50=750, p95=885
        assert_eq!(
            r.ratification_cycle_time.open_to_ready,
            PercentileMetric {
                count: 2,
                p50_seconds: Some(750),
                p95_seconds: Some(885),
            }
        );
    }

    #[test]
    fn ratification_orphan_ready_before_open_not_paired() {
        // Degenerate: ready occurs before open (fixture noise / orphan).
        // The orphan ready must NOT be paired; the subsequent open has no
        // matching ready so contributes 0 cycles.
        let c = conn();
        // ready at T=0 (no prior open → orphan)
        ins(&c, "obs", "L2", None, "ready", "2026-01-01T00:00:00Z");
        // open at T=600 (no subsequent ready → no pair)
        ins(
            &c,
            "obs",
            "L2",
            Some("ready"),
            "open",
            "2026-01-01T00:10:00Z",
        );
        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();
        assert_eq!(
            r.ratification_cycle_time.open_to_ready,
            PercentileMetric {
                count: 0,
                p50_seconds: None,
                p95_seconds: None,
            }
        );
    }

    #[test]
    fn parse_duration_rejects_zero() {
        let err = parse_duration_seconds("0h").unwrap_err();
        assert!(
            err.to_string().contains("window must be positive"),
            "expected positive-error for '0h'; got: {err}"
        );
    }

    #[test]
    fn parse_duration_rejects_negative() {
        let err = parse_duration_seconds("-5h").unwrap_err();
        assert!(
            err.to_string().contains("window must be positive"),
            "expected positive-error for '-5h'; got: {err}"
        );
    }

    #[test]
    fn parse_duration_accepts_positive() {
        assert_eq!(parse_duration_seconds("1h").unwrap(), 3600);
    }

    #[test]
    fn revise_rate_groups_by_phase_tier_unknown_task_type() {
        let c = conn();
        c.execute_batch("CREATE TABLE tasks (display_id TEXT, tier_hint TEXT, cycles TEXT);")
            .unwrap();
        c.execute("INSERT INTO tasks VALUES ('T1','T1',?1)", [r#"[{"phase":1,"cycle":1,"review":{"gate":"PASS"}},{"phase":1,"cycle":2,"review":{"gate":"REVISE"}},{"phase":2,"cycle":1,"review":{"gate":"PASS"}}]"#]).unwrap();
        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();
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
        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();
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
        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();
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
        let a = render_json(&build_report(&c, "2025-12-31T00:00:00Z", None).unwrap()).unwrap();
        let b = render_json(&build_report(&c, "2025-12-31T00:00:00Z", None).unwrap()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn duration_window_bare_emits_since_resolved_and_volatile_true() {
        // Bare duration windows (no --now) MUST include since_resolved (always populated)
        // and volatile_window=true so the caller knows the cutoff is wall-clock-derived.
        // The rerun stability guarantee is: pass since_resolved as absolute --since to get
        // identical output; the bare form is intentionally volatile but auditable.
        let c = conn();
        let report = build_report(&c, "1h", None).unwrap();
        assert_eq!(report.window.requested, "1h");
        assert_eq!(report.window.since, "now-1h");
        assert!(
            !report.window.since_resolved.is_empty(),
            "since_resolved must always be populated; got empty string"
        );
        assert!(
            report.window.volatile_window,
            "volatile_window must be true for bare duration windows"
        );
        // Confirm JSON contains both since_resolved and volatile_window keys.
        let json = render_json(&report).unwrap();
        assert!(
            json.contains("since_resolved"),
            "JSON must contain since_resolved for bare --window 1h; got: {json}"
        );
        assert!(
            json.contains("volatile_window"),
            "JSON must contain volatile_window for bare --window 1h; got: {json}"
        );
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
        let txt = render_text(&build_report(&c, "2025-12-31T00:00:00Z", None).unwrap());
        assert!(txt.contains("per_edge"));
        assert!(txt.contains("store=tasks edge=open -> ready count=1"));
        assert!(txt.contains("ratification_cycle_time.open_to_ready count=1"));
        assert!(txt.contains("revise_rate"));
        assert!(txt.contains("agent_runs"));
        assert!(txt.contains("agent_runs not yet captured"));
    }

    // ── epoch_seconds: RFC3339 parser coverage (MAJOR-2 fix) ─────────────────

    #[test]
    fn epoch_seconds_parses_z_suffix() {
        // 2026-01-01T00:00:00Z → Unix epoch for that moment
        let e = epoch_seconds("2026-01-01T00:00:00Z").expect("Z form must parse");
        assert_eq!(e, 1767225600);
    }

    #[test]
    fn epoch_seconds_parses_plus_zero_offset() {
        // +00:00 is equivalent to Z
        let z = epoch_seconds("2026-01-01T00:00:00Z").unwrap();
        let off = epoch_seconds("2026-01-01T00:00:00+00:00").unwrap();
        assert_eq!(z, off, "+00:00 must equal Z");
    }

    #[test]
    fn epoch_seconds_parses_negative_offset() {
        // -08:00 means the UTC equivalent is +8h
        let z = epoch_seconds("2026-01-01T00:00:00Z").unwrap();
        let neg = epoch_seconds("2025-12-31T16:00:00-08:00").unwrap();
        assert_eq!(z, neg, "-08:00 offset must convert to correct UTC epoch");
    }

    #[test]
    fn epoch_seconds_rejects_malformed() {
        assert!(epoch_seconds("not-a-date").is_none());
        assert!(epoch_seconds("2026-13-01T00:00:00Z").is_none()); // month 13
        assert!(epoch_seconds("").is_none());
    }

    #[test]
    fn epoch_seconds_parses_sqlite_space_separator() {
        // SQLite stores timestamps as "2026-01-01 00:00:00"
        let z = epoch_seconds("2026-01-01T00:00:00Z").unwrap();
        let sp = epoch_seconds("2026-01-01 00:00:00Z").unwrap();
        assert_eq!(z, sp, "space-separator form must parse the same as T-separator");
    }

    // ── revise_rate window note (MAJOR-1 fix) ────────────────────────────────

    #[test]
    fn revise_rate_always_carries_window_note() {
        // Even when tasks table is absent the window-caveat note must be present.
        let c = conn(); // no tasks table
        let r = build_report(&c, "1h", None).unwrap();
        assert!(
            r.revise_rate.notes.iter().any(|n| n.contains("unwindowed")),
            "revise_rate.notes must include unwindowed caveat; got: {:?}",
            r.revise_rate.notes
        );
    }

    #[test]
    fn revise_rate_window_note_contains_window_arg() {
        let c = conn();
        c.execute_batch("CREATE TABLE tasks (display_id TEXT, tier_hint TEXT, cycles TEXT);")
            .unwrap();
        let r = build_report(&c, "30m", None).unwrap();
        let note = r
            .revise_rate
            .notes
            .iter()
            .find(|n| n.contains("unwindowed"))
            .unwrap();
        assert!(
            note.contains("--window=30m"),
            "note must echo the window arg; got: {note}"
        );
    }

    // ── percentile: interpolation coverage ───────────────────────────────────

    #[test]
    fn percentile_empty_returns_none() {
        assert_eq!(percentile(&[], 50), None);
        assert_eq!(percentile(&[], 95), None);
    }

    #[test]
    fn percentile_one_element_returns_that_element() {
        assert_eq!(percentile(&[42], 0), Some(42));
        assert_eq!(percentile(&[42], 50), Some(42));
        assert_eq!(percentile(&[42], 100), Some(42));
    }

    #[test]
    fn percentile_two_elements_interpolates() {
        // [120, 240], n=2
        //   p50: index = 0.5 * 1 = 0.5 → 120 + 0.5*(240-120) = 180
        //   p95: index = 0.95 * 1 = 0.95 → 120 + 0.95*(240-120) = 234
        //   p0:  index = 0 → 120
        //   p100: index = 1 → 240
        assert_eq!(percentile(&[120, 240], 50), Some(180));
        assert_eq!(percentile(&[120, 240], 95), Some(234));
        assert_eq!(percentile(&[120, 240], 0), Some(120));
        assert_eq!(percentile(&[120, 240], 100), Some(240));
    }

    #[test]
    fn percentile_odd_count_exact_median() {
        // [1, 2, 3], n=3
        //   p50: index = 0.5 * 2 = 1.0 (exact) → v[1] = 2
        assert_eq!(percentile(&[1, 2, 3], 50), Some(2));
    }

    #[test]
    fn percentile_even_count_interpolated_median() {
        // [1, 2, 3, 4], n=4
        //   p50: index = 0.5 * 3 = 1.5 → 2 + 0.5*(3-2) = 2.5 → rounds to 3
        //   (round-half-up for the midpoint)
        let p50 = percentile(&[1, 2, 3, 4], 50).unwrap();
        // With f64 round: 2.5.round() = 3 (round-half-away-from-zero)
        assert_eq!(p50, 3);
    }

    #[test]
    fn percentile_unsorted_input_sorts_first() {
        // Input [240, 120] must be treated as [120, 240] after sorting
        assert_eq!(percentile(&[240, 120], 50), Some(180));
    }

    #[test]
    fn percentile_large_series_p50_and_p95() {
        // [1..=100], n=100
        //   p50: index = 0.5 * 99 = 49.5 → v[49]+0.5*(v[50]-v[49]) = 50+0.5*1 = 50.5 → 51 (round)
        //   p95: index = 0.95 * 99 = 94.05 → v[94]+0.05*(v[95]-v[94]) = 95+0.05*1 = 95.05 → 95 (round)
        let vals: Vec<i64> = (1..=100).collect();
        let p50 = percentile(&vals, 50).unwrap();
        let p95 = percentile(&vals, 95).unwrap();
        assert_eq!(p50, 51);
        assert_eq!(p95, 95);
    }

    // ── MAJOR 1: windowed REVISE rate from transition_history ────────────────

    fn conn_with_th_verbs() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE transition_history \
             (store TEXT, display_id TEXT, from_status TEXT, to_status TEXT, \
              occurred_at TEXT, verb TEXT);",
        )
        .unwrap();
        c
    }
    fn ins_th(c: &Connection, verb: &str, to: &str, ts: &str) {
        c.execute(
            "INSERT INTO transition_history VALUES ('tasks','T1',NULL,?1,?2,?3)",
            (to, ts, verb),
        )
        .unwrap();
    }

    #[test]
    fn revise_rate_windowed_from_transition_history() {
        let c = conn_with_th_verbs();
        // 3 code reviews: 2 REVISE (→ executing), 1 PASS (→ complete)
        ins_th(&c, "submit-review", "executing", "2026-01-01T00:00:00Z");
        ins_th(&c, "submit-review", "executing", "2026-01-01T01:00:00Z");
        ins_th(&c, "submit-review", "complete", "2026-01-01T02:00:00Z");
        // 2 plan reviews: 1 REVISE (→ planning), 1 PASS (→ ready)
        ins_th(&c, "submit-plan-review", "planning", "2026-01-01T03:00:00Z");
        ins_th(&c, "submit-plan-review", "ready", "2026-01-01T04:00:00Z");

        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();

        let code_row = r
            .revise_rate
            .rows
            .iter()
            .find(|m| m.phase == "code_review")
            .expect("code_review row must exist");
        assert_eq!((code_row.revise_count, code_row.total_reviews), (2, 3));
        assert!((code_row.revise_rate - 2.0 / 3.0).abs() < 1e-9);

        let plan_row = r
            .revise_rate
            .rows
            .iter()
            .find(|m| m.phase == "plan_review")
            .expect("plan_review row must exist");
        assert_eq!((plan_row.revise_count, plan_row.total_reviews), (1, 2));

        // Note must say "windowed" not "unwindowed"
        assert!(
            r.revise_rate.notes.iter().any(|n| n.contains("windowed") && !n.contains("unwindowed")),
            "notes must confirm windowed source; got: {:?}",
            r.revise_rate.notes
        );
    }

    #[test]
    fn revise_rate_transition_history_respects_window() {
        let c = conn_with_th_verbs();
        // One REVISE before window, one after
        ins_th(&c, "submit-review", "executing", "2025-06-01T00:00:00Z"); // before window
        ins_th(&c, "submit-review", "complete", "2026-06-01T00:00:00Z");  // inside window
        // Window: 2026-01-01 onward
        let r = build_report(&c, "2026-01-01T00:00:00Z", None).unwrap();
        let code_row = r
            .revise_rate
            .rows
            .iter()
            .find(|m| m.phase == "code_review")
            .expect("code_review row must exist");
        // Only the PASS inside window counts; REVISE before window excluded
        assert_eq!((code_row.revise_count, code_row.total_reviews), (0, 1));
    }

    // ── per-task-type REVISE from transition_history JOIN tasks ─────────────

    /// Build a DB with transition_history+tasks where display_ids are linked.
    fn conn_with_th_and_tasks() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE transition_history \
             (store TEXT, display_id TEXT, from_status TEXT, to_status TEXT, \
              occurred_at TEXT, verb TEXT); \
             CREATE TABLE tasks (display_id TEXT, task_type TEXT);",
        )
        .unwrap();
        c
    }

    fn ins_th_id(c: &Connection, id: &str, verb: &str, to: &str, ts: &str) {
        c.execute(
            "INSERT INTO transition_history VALUES ('tasks',?1,NULL,?2,?3,?4)",
            (id, to, ts, verb),
        )
        .unwrap();
    }

    fn ins_task(c: &Connection, id: &str, task_type: &str) {
        c.execute(
            "INSERT INTO tasks (display_id, task_type) VALUES (?1, ?2)",
            (id, task_type),
        )
        .unwrap();
    }

    #[test]
    fn revise_rate_per_task_type_groups_by_type() {
        // Two task types: "feature" and "chore".
        // feature task T1: 2 code-review REVISE, 1 PASS
        // chore   task T2: 1 code-review PASS
        let c = conn_with_th_and_tasks();
        ins_task(&c, "T1", "feature");
        ins_task(&c, "T2", "chore");
        ins_th_id(&c, "T1", "submit-review", "executing", "2026-01-01T00:00:00Z");
        ins_th_id(&c, "T1", "submit-review", "executing", "2026-01-01T01:00:00Z");
        ins_th_id(&c, "T1", "submit-review", "complete",  "2026-01-01T02:00:00Z");
        ins_th_id(&c, "T2", "submit-review", "complete",  "2026-01-01T03:00:00Z");

        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();

        let feature_row = r
            .revise_rate
            .rows
            .iter()
            .find(|m| m.phase == "code_review" && m.task_type == "feature")
            .expect("feature code_review row must exist");
        assert_eq!((feature_row.revise_count, feature_row.total_reviews), (2, 3));

        let chore_row = r
            .revise_rate
            .rows
            .iter()
            .find(|m| m.phase == "code_review" && m.task_type == "chore")
            .expect("chore code_review row must exist");
        assert_eq!((chore_row.revise_count, chore_row.total_reviews), (0, 1));

        // Exactly 2 rows for code_review (one per task type)
        let code_rows: Vec<_> = r.revise_rate.rows.iter().filter(|m| m.phase == "code_review").collect();
        assert_eq!(code_rows.len(), 2, "expected 2 code_review rows (one per task type)");
    }

    #[test]
    fn revise_rate_per_task_type_unknown_when_no_tasks_table() {
        // When tasks table is absent, task_type falls back to "unknown".
        let c = conn_with_th_verbs(); // no tasks table
        ins_th(&c, "submit-review", "executing", "2026-01-01T00:00:00Z");
        ins_th(&c, "submit-review", "complete",  "2026-01-01T01:00:00Z");

        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();
        let code_row = r
            .revise_rate
            .rows
            .iter()
            .find(|m| m.phase == "code_review")
            .expect("code_review row must exist");
        assert_eq!(code_row.task_type, "unknown");
    }

    // ── Option B: volatile_window semantics ────────────────────────────────────

    #[test]
    fn absolute_since_has_volatile_false_and_since_resolved_equals_input() {
        // Absolute --since: volatile_window=false, since_resolved == normalized input.
        let c = conn();
        let report = build_report(&c, "2026-05-07T05:00:00Z", None).unwrap();
        assert!(!report.window.volatile_window, "absolute --since must be non-volatile");
        assert_eq!(report.window.since_resolved, "2026-05-07T05:00:00Z");
    }

    #[test]
    fn absolute_since_stability_identical_on_two_calls() {
        // Two calls with the same absolute --since must produce identical JSON.
        let c = conn();
        ins(&c, "tasks", "T1", None, "planning", "2026-05-07T06:00:00Z");
        let a = render_json(&build_report(&c, "2026-05-07T05:00:00Z", None).unwrap()).unwrap();
        let b = render_json(&build_report(&c, "2026-05-07T05:00:00Z", None).unwrap()).unwrap();
        assert_eq!(a, b, "absolute --since must produce stable output");
    }

    #[test]
    fn duration_plus_now_stability_identical_on_two_calls() {
        // duration + --now: volatile_window=false, two calls produce identical JSON.
        let c = conn();
        ins(&c, "tasks", "T1", None, "planning", "2026-05-07T06:00:00Z");
        let fixed_now = "2026-05-07T12:00:00Z";
        let a = render_json(&build_report(&c, "1h", Some(fixed_now)).unwrap()).unwrap();
        let b = render_json(&build_report(&c, "1h", Some(fixed_now)).unwrap()).unwrap();
        assert_eq!(a, b, "duration+--now must produce stable output");
    }

    #[test]
    fn bare_duration_has_volatile_true_and_since_resolved_present() {
        // Bare duration: volatile_window=true, since_resolved always populated.
        // Two consecutive calls may differ in since_resolved (wall-clock drift)
        // but both must have volatile_window=true and a non-empty since_resolved.
        let c = conn();
        let report = build_report(&c, "1h", None).unwrap();
        assert!(report.window.volatile_window, "bare duration must be volatile");
        assert!(!report.window.since_resolved.is_empty(), "since_resolved must be populated");
        // since_resolved must be a parseable RFC3339 timestamp.
        assert!(
            epoch_seconds(&report.window.since_resolved).is_some(),
            "since_resolved must be a valid RFC3339 timestamp; got: {}",
            report.window.since_resolved
        );
    }

    // ── MAJOR 2: no-op audit row exclusion ─────────────────────────────────────

    #[test]
    fn per_edge_skips_noop_audit_rows_in_counts() {
        // A no-op audit row (from_status == to_status) must not appear in edge counts.
        let c = conn();
        // Real transition: None -> planning
        ins(&c, "tasks", "T1", None, "planning", "2026-01-01T00:00:00Z");
        // No-op audit row: planning -> planning (same status)
        ins(&c, "tasks", "T1", Some("planning"), "planning", "2026-01-01T00:01:00Z");
        // Real transition: planning -> plan_review
        ins(&c, "tasks", "T1", Some("planning"), "plan_review", "2026-01-01T00:02:00Z");

        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();

        // "planning -> planning" edge must NOT appear
        let noop_edge = r
            .per_edge
            .iter()
            .find(|e| e.edge == "planning -> planning");
        assert!(noop_edge.is_none(), "no-op audit edge must be excluded; found: {:?}", noop_edge);

        // Real edges must still appear with correct counts
        let create_edge = r
            .per_edge
            .iter()
            .find(|e| e.store == "tasks" && e.edge == "(create) -> planning")
            .expect("(create) -> planning edge must exist");
        assert_eq!(create_edge.count, 1);

        let plan_edge = r
            .per_edge
            .iter()
            .find(|e| e.store == "tasks" && e.edge == "planning -> plan_review")
            .expect("planning -> plan_review edge must exist");
        assert_eq!(plan_edge.count, 1);
    }

    #[test]
    fn per_edge_noop_row_does_not_corrupt_latency_chain() {
        // A no-op audit row must not update the "previous row" cursor.
        // The latency for "planning -> plan_review" must be computed against
        // the last REAL lifecycle row ((create) -> planning at T+0), NOT the
        // no-op at T+60s.  Expected latency: 120s (00:02:00 - 00:00:00).
        let c = conn();
        // (create) -> planning at T+0
        ins(&c, "tasks", "T1", None, "planning", "2026-01-01T00:00:00Z");
        // no-op audit at T+60s  — must NOT advance the prev cursor
        ins(&c, "tasks", "T1", Some("planning"), "planning", "2026-01-01T00:01:00Z");
        // planning -> plan_review at T+120s
        ins(&c, "tasks", "T1", Some("planning"), "plan_review", "2026-01-01T00:02:00Z");

        let r = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();

        let plan_edge = r
            .per_edge
            .iter()
            .find(|e| e.store == "tasks" && e.edge == "planning -> plan_review")
            .expect("planning -> plan_review edge must exist");
        // Latency must be 120s (2 min), not 60s (1 min from no-op timestamp).
        assert_eq!(
            plan_edge.p50_seconds,
            Some(120),
            "latency must be measured against last real lifecycle row, not no-op; got: {:?}",
            plan_edge.p50_seconds
        );
    }

    // ── --json flag reachable via metrics-local flag (MINOR fix) ─────────────

    #[test]
    fn json_output_is_valid_json() {
        let c = conn();
        ins(&c, "tasks", "T1", None, "planning", "2026-01-01T00:00:00Z");
        let report = build_report(&c, "2025-12-31T00:00:00Z", None).unwrap();
        let json_str = render_json(&report).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).expect("render_json must produce valid JSON");
        assert!(
            parsed.get("window").is_some(),
            "JSON must contain 'window' key"
        );
        assert!(
            parsed.get("per_edge").is_some(),
            "JSON must contain 'per_edge' key"
        );
        assert!(
            parsed.get("revise_rate").is_some(),
            "JSON must contain 'revise_rate' key"
        );
    }

    // ── MAJOR 1: UTC normalization of RFC3339 window inputs ──────────────────

    #[test]
    fn normalize_to_utc_z_suffix_unchanged() {
        // Z-suffix is already UTC; normalized form must be canonical Z form.
        let n = normalize_to_utc("2026-01-01T05:00:00Z").unwrap();
        assert_eq!(n, "2026-01-01T05:00:00Z");
    }

    #[test]
    fn normalize_to_utc_plus_zero_offset_becomes_z() {
        // +00:00 is equivalent to Z; normalized form must be Z.
        let n = normalize_to_utc("2026-01-01T05:00:00+00:00").unwrap();
        assert_eq!(n, "2026-01-01T05:00:00Z");
    }

    #[test]
    fn normalize_to_utc_negative_offset_translates_correctly() {
        // -08:00 means wall clock is 8h behind UTC; UTC equivalent is +8h.
        // 2026-01-01T00:00:00-08:00 == 2026-01-01T08:00:00Z
        let n = normalize_to_utc("2026-01-01T00:00:00-08:00").unwrap();
        assert_eq!(n, "2026-01-01T08:00:00Z");
    }

    #[test]
    fn agent_run_metrics_utc_normalized_cutoff_filters_correctly() {
        // agent_run_metrics must use a UTC-normalized cutoff string so that
        // SQLite text comparison works against stored UTC timestamps.
        // Input: -08:00 window → UTC +8h.  Row at 2026-01-01T09:00:00Z (after)
        // and at 2026-01-01T07:00:00Z (before) should be separated correctly.
        let c = conn();
        c.execute_batch(
            "CREATE TABLE agent_runs \
             (role TEXT, occurred_at TEXT, input_tokens INTEGER, output_tokens INTEGER, total_tokens INTEGER);",
        )
        .unwrap();
        // Normalized cutoff: 2026-01-01T08:00:00Z (from -08:00 input)
        // Row before cutoff: 2026-01-01T07:00:00Z  → excluded
        c.execute(
            "INSERT INTO agent_runs VALUES ('executor','2026-01-01T07:00:00Z',5,5,10)",
            [],
        )
        .unwrap();
        // Row after cutoff: 2026-01-01T09:00:00Z  → included
        c.execute(
            "INSERT INTO agent_runs VALUES ('executor','2026-01-01T09:00:00Z',10,20,30)",
            [],
        )
        .unwrap();
        // Window: 2026-01-01T00:00:00-08:00 → UTC 2026-01-01T08:00:00Z
        let r = build_report(&c, "2026-01-01T00:00:00-08:00", None).unwrap();
        // Only the 09:00Z row should be included.
        assert_eq!(r.agent_runs.rows.len(), 1);
        assert_eq!(
            r.agent_runs.rows[0],
            AgentRunMetric {
                role: "executor".into(),
                input_tokens: 10,
                output_tokens: 20,
                total_tokens: 30,
            }
        );
    }

    // ── MAJOR 2: --now override makes duration-window output deterministic ────

    #[test]
    fn duration_window_with_now_override_is_deterministic() {
        // Calling build_report twice with the same --now override must produce
        // identical JSON output (no wall-clock dependency).
        let c = conn();
        ins(&c, "tasks", "T1", None, "planning", "2026-01-01T00:00:00Z");
        let fixed_now = "2026-05-07T12:00:00Z";
        let a = render_json(&build_report(&c, "1h", Some(fixed_now)).unwrap()).unwrap();
        let b = render_json(&build_report(&c, "1h", Some(fixed_now)).unwrap()).unwrap();
        assert_eq!(a, b, "output must be identical across reruns with fixed --now");
    }

    #[test]
    fn duration_window_now_override_resolves_correct_cutoff() {
        // With --now 2026-05-07T12:00:00Z and --window 1h,
        // since_resolved must be "2026-05-07T11:00:00Z" and volatile_window=false.
        let c = conn();
        let report = build_report(&c, "1h", Some("2026-05-07T12:00:00Z")).unwrap();
        assert_eq!(report.window.since_resolved, "2026-05-07T11:00:00Z");
        assert!(!report.window.volatile_window, "volatile_window must be false when --now is supplied");
    }

    #[test]
    fn duration_window_now_override_non_utc_resolves_correct_cutoff() {
        // --now with -05:00 offset: 2026-05-07T07:00:00-05:00 == 12:00:00Z.
        // --window 1h → since_resolved = "2026-05-07T11:00:00Z", volatile_window=false.
        let c = conn();
        let report = build_report(&c, "1h", Some("2026-05-07T07:00:00-05:00")).unwrap();
        assert_eq!(report.window.since_resolved, "2026-05-07T11:00:00Z");
        assert!(!report.window.volatile_window, "volatile_window must be false when --now is supplied");
    }
}
