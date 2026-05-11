use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTranscript {
    pub display_id: String,
    pub phase: i64,
    pub cycle: i64,
    pub role: String,
    pub path: PathBuf,
}

pub enum RunsCmd {
    List {
        display_id: String,
    },
    Show {
        display_id: String,
        phase: i64,
        cycle: Option<i64>,
        role: String,
    },
    Current {
        display_id: String,
        role: Option<String>,
    },
    Tail {
        display_id: String,
        role: Option<String>,
        raw: bool,
        stderr: bool,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CurrentRunMarker {
    pub display_id: String,
    pub phase: Option<i64>,
    pub cycle: Option<i64>,
    pub role: String,
    pub runner: Option<String>,
    pub session_id: Option<String>,
    pub status: Option<String>,
    pub transcript_path: Option<PathBuf>,
    pub stderr_log_path: Option<PathBuf>,
    pub events_path: Option<PathBuf>,
    pub status_path: Option<PathBuf>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CurrentRunStatus {
    pub last_event_at: Option<String>,
    pub last_event_type: Option<String>,
    pub current_activity: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CurrentRun {
    pub marker_path: PathBuf,
    pub marker: CurrentRunMarker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentRunLiveness {
    NotRunning,
    RunningLive,
    RunningStale { reason: String },
    Unknown,
}

impl CurrentRunLiveness {
    pub fn label(&self) -> &'static str {
        match self {
            CurrentRunLiveness::NotRunning => "not_running",
            CurrentRunLiveness::RunningLive => "live",
            CurrentRunLiveness::RunningStale { .. } => "stale_marker",
            CurrentRunLiveness::Unknown => "unknown",
        }
    }
}

const CURRENT_RUN_FRESH_SECS: i64 = 15 * 60;

fn now_utc() -> DateTime<Utc> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    DateTime::<Utc>::from_timestamp(secs, 0).unwrap_or_else(|| {
        DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is a valid DateTime")
    })
}

pub fn run(cmd: RunsCmd) -> Result<()> {
    match cmd {
        RunsCmd::List { display_id } => {
            let stores_dir = crate::paths::stores_dir()?;
            let rows = list_for_task(&stores_dir, &display_id)?;
            println!("phase\tcycle\trole\ttranscript_path");
            for row in rows {
                println!(
                    "{}\t{}\t{}\t{}",
                    row.phase,
                    row.cycle,
                    row.role,
                    row.path.display()
                );
            }
            Ok(())
        }
        RunsCmd::Show {
            display_id,
            phase,
            cycle,
            role,
        } => {
            let stores_dir = crate::paths::stores_dir()?;
            let row = find_transcript(&stores_dir, &display_id, phase, cycle, &role)?;
            let read_path = resolve_transcript_path(&stores_dir, &row.path);
            let body = fs::read_to_string(&read_path).with_context(|| {
                format!(
                    "failed to read transcript {} (resolved to {})",
                    row.path.display(),
                    read_path.display()
                )
            })?;
            println!("{body}");
            Ok(())
        }
        RunsCmd::Current { display_id, role } => {
            let stores_dir = crate::paths::stores_dir()?;
            let current = find_current_run(&stores_dir, &display_id, role.as_deref())?;
            print_current_run(&stores_dir, &current);
            Ok(())
        }
        RunsCmd::Tail {
            display_id,
            role,
            raw,
            stderr,
        } => {
            if !raw && !stderr {
                bail!("runs tail currently requires --raw or --stderr");
            }
            if raw && stderr {
                bail!("runs tail accepts only one of --raw or --stderr");
            }
            let stores_dir = crate::paths::stores_dir()?;
            let current = find_current_run(&stores_dir, &display_id, role.as_deref())?;
            let path = if stderr {
                current
                    .marker
                    .stderr_log_path
                    .as_ref()
                    .context("current run marker does not include stderr_log_path")?
            } else {
                current
                    .marker
                    .transcript_path
                    .as_ref()
                    .context("current run marker does not include transcript_path")?
            };
            let read_path = resolve_marker_path(&stores_dir, &current.marker_path, path);
            let body = fs::read_to_string(&read_path).with_context(|| {
                format!(
                    "failed to read live run log {} (resolved to {})",
                    path.display(),
                    read_path.display()
                )
            })?;
            print!("{body}");
            io::stdout().flush().ok();
            Ok(())
        }
    }
}

fn print_current_run(stores_dir: &Path, current: &CurrentRun) {
    let m = &current.marker;
    println!("display_id\t{}", m.display_id);
    println!("role\t{}", m.role);
    if let Some(phase) = m.phase {
        println!("phase\t{phase}");
    }
    if let Some(cycle) = m.cycle {
        println!("cycle\t{cycle}");
    }
    if let Some(runner) = &m.runner {
        println!("runner\t{runner}");
    }
    if let Some(status) = &m.status {
        println!("status\t{status}");
    }
    if let Some(updated_at) = &m.updated_at {
        println!("updated_at\t{updated_at}");
    }
    println!("marker_path\t{}", current.marker_path.display());
    if let Some(path) = &m.transcript_path {
        println!(
            "transcript_path\t{}",
            resolve_marker_path(stores_dir, &current.marker_path, path).display()
        );
    }
    if let Some(path) = &m.stderr_log_path {
        println!(
            "stderr_log_path\t{}",
            resolve_marker_path(stores_dir, &current.marker_path, path).display()
        );
    }
    if let Some(path) = &m.status_path {
        println!(
            "status_path\t{}",
            resolve_marker_path(stores_dir, &current.marker_path, path).display()
        );
    }
    if let Ok(Some(status)) = read_current_status(stores_dir, current) {
        if let Some(last_event_at) = status.last_event_at {
            println!("last_event_at\t{last_event_at}");
        }
        if let Some(last_event_type) = status.last_event_type {
            println!("last_event_type\t{last_event_type}");
        }
        if let Some(current_activity) = status.current_activity {
            println!("current_activity\t{current_activity}");
        }
    }
    let liveness = current_run_liveness(stores_dir, current, now_utc())
        .unwrap_or(CurrentRunLiveness::Unknown);
    println!("liveness\t{}", liveness.label());
    if let CurrentRunLiveness::RunningStale { reason } = liveness {
        println!("liveness_reason\t{reason}");
    }
}

pub fn find_current_run(
    stores_dir: &Path,
    display_id: &str,
    role: Option<&str>,
) -> Result<CurrentRun> {
    let live_stores_dir = live_stores_dir_for_task(stores_dir, display_id)?;
    let runs_dir = live_stores_dir.join("runs");
    let mut candidates = Vec::new();

    if let Some(role) = role {
        let marker_path = runs_dir.join(format!("current-{display_id}-{role}.json"));
        if !marker_path.exists() {
            bail!(
                "no current live run marker found for {display_id} role {role}: {}",
                marker_path.display()
            );
        }
        candidates.push(read_current_marker(&marker_path)?);
    } else {
        let entries = fs::read_dir(&runs_dir)
            .with_context(|| format!("failed to read runs dir {}", runs_dir.display()))?;
        let prefix = format!("current-{display_id}-");
        for entry in entries {
            let entry =
                entry.with_context(|| format!("reading entry in {}", runs_dir.display()))?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with(&prefix) && name.ends_with(".json") {
                candidates.push(read_current_marker(&path)?);
            }
        }
        if candidates.is_empty() {
            bail!(
                "no current live run markers found for {display_id} in {}",
                runs_dir.display()
            );
        }
        candidates.sort_by(|a, b| compare_current_run_candidates(stores_dir, a, b));
    }

    candidates
        .pop()
        .context("no current live run marker candidate selected")
}

fn compare_current_run_candidates(
    stores_dir: &Path,
    a: &CurrentRun,
    b: &CurrentRun,
) -> std::cmp::Ordering {
    current_run_selection_key(stores_dir, a)
        .cmp(&current_run_selection_key(stores_dir, b))
        .then_with(|| a.marker.role.cmp(&b.marker.role))
}

fn current_run_selection_key(stores_dir: &Path, current: &CurrentRun) -> (u8, String, String) {
    let liveness = current_run_liveness(stores_dir, current, now_utc())
        .unwrap_or(CurrentRunLiveness::Unknown);
    let running_rank = match liveness {
        CurrentRunLiveness::RunningLive => 2,
        CurrentRunLiveness::RunningStale { .. } => 0,
        CurrentRunLiveness::NotRunning => 1,
        CurrentRunLiveness::Unknown => {
            if current.marker.status.as_deref() == Some("running") { 1 } else { 0 }
        }
    };
    let semantic_freshness = read_current_status(stores_dir, current)
        .ok()
        .flatten()
        .and_then(|status| status.last_event_at)
        .unwrap_or_default();
    let marker_freshness = current.marker.updated_at.clone().unwrap_or_default();
    (running_rank, semantic_freshness, marker_freshness)
}

fn read_current_marker(path: &Path) -> Result<CurrentRun> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("failed to read live run marker {}", path.display()))?;
    let marker: CurrentRunMarker = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse live run marker {}", path.display()))?;
    Ok(CurrentRun {
        marker_path: path.to_path_buf(),
        marker,
    })
}

pub fn current_status_path(stores_dir: &Path, current: &CurrentRun) -> Option<PathBuf> {
    if let Some(status_path) = &current.marker.status_path {
        return Some(resolve_marker_path(
            stores_dir,
            &current.marker_path,
            status_path,
        ));
    }
    let session_id = current.marker.session_id.as_deref()?;
    let transcript_path = current.marker.transcript_path.as_ref()?;
    let resolved_transcript =
        resolve_marker_path(stores_dir, &current.marker_path, transcript_path);
    Some(crate::runner::status_path_for_transcript(
        &resolved_transcript,
        session_id,
    ))
}

pub fn read_current_status(
    stores_dir: &Path,
    current: &CurrentRun,
) -> Result<Option<CurrentRunStatus>> {
    let Some(path) = current_status_path(stores_dir, current) else {
        return Ok(None);
    };
    if !path.exists() {
        return Ok(None);
    }
    let body = fs::read_to_string(&path)
        .with_context(|| format!("failed to read live runner status {}", path.display()))?;
    let status: CurrentRunStatus = serde_json::from_str(&body)
        .with_context(|| format!("failed to parse live runner status {}", path.display()))?;
    Ok(Some(status))
}

pub fn current_run_liveness(
    stores_dir: &Path,
    current: &CurrentRun,
    now: DateTime<Utc>,
) -> Result<CurrentRunLiveness> {
    if current.marker.status.as_deref() != Some("running") {
        return Ok(CurrentRunLiveness::NotRunning);
    }
    if has_corroborating_live_owner(stores_dir, &current.marker.display_id)? {
        return Ok(CurrentRunLiveness::RunningLive);
    }
    let last_at = read_current_status(stores_dir, current)
        .ok()
        .flatten()
        .and_then(|status| status.last_event_at)
        .or_else(|| current.marker.updated_at.clone());
    let Some(last_at) = last_at else {
        return Ok(CurrentRunLiveness::RunningStale {
            reason: "running marker has no semantic heartbeat, marker timestamp, or live owner"
                .to_string(),
        });
    };
    if timestamp_is_fresh(&last_at, now, CURRENT_RUN_FRESH_SECS) {
        Ok(CurrentRunLiveness::RunningLive)
    } else {
        Ok(CurrentRunLiveness::RunningStale {
            reason: format!(
                "running marker has stale last_event_at/updated_at {last_at} and no live owner"
            ),
        })
    }
}

fn timestamp_is_fresh(ts: &str, now: DateTime<Utc>, fresh_secs: i64) -> bool {
    let Ok(parsed) = DateTime::parse_from_rfc3339(ts).map(|dt| dt.with_timezone(&Utc)) else {
        return false;
    };
    now.signed_duration_since(parsed) <= Duration::seconds(fresh_secs)
}

fn has_corroborating_live_owner(stores_dir: &Path, display_id: &str) -> Result<bool> {
    let db_path = stores_dir.join("db.sqlite");
    if !db_path.exists() {
        return Ok(false);
    }
    let conn = Connection::open(&db_path)
        .with_context(|| format!("failed to open substrate DB {}", db_path.display()))?;
    let row: Option<(i64, Option<i64>)> = conn
        .query_row(
            "SELECT id, drive_pid FROM tasks WHERE display_id = ?1",
            [display_id],
            |r| Ok((r.get(0)?, r.get(1).ok().flatten())),
        )
        .optional()
        .context("lookup task live owner row")?;
    let Some((row_id, drive_pid)) = row else {
        return Ok(false);
    };
    if drive_pid
        .filter(|pid| *pid > 0)
        .is_some_and(|pid| crate::handlers::agents_run::pid_is_alive(pid as i32))
    {
        return Ok(true);
    }
    let mut stmt = conn.prepare(
        "SELECT COALESCE(pid, 0) FROM dispatch_locks \
         WHERE store='tasks' AND row_id=?1 AND agent_name='auto-drive' AND finished_at IS NULL",
    )?;
    let pids = stmt.query_map([row_id], |r| r.get::<_, i64>(0))?;
    for pid in pids.filter_map(|r| r.ok()) {
        if pid > 0 && crate::handlers::agents_run::pid_is_alive(pid as i32) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn live_stores_dir_for_task(stores_dir: &Path, display_id: &str) -> Result<PathBuf> {
    let db_path = stores_dir.join("db.sqlite");
    let conn = Connection::open(&db_path)
        .with_context(|| format!("failed to open substrate DB {}", db_path.display()))?;
    let has_workspace_path: bool = conn
        .prepare("PRAGMA table_info(tasks)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .any(|name| name == "workspace_path");
    let workspace: Option<String> = if has_workspace_path {
        conn.query_row(
            "SELECT workspace_path FROM tasks WHERE display_id = ?1",
            [display_id],
            |r| r.get(0),
        )
        .optional()
        .context("lookup task workspace_path")?
    } else {
        None
    };
    if let Some(workspace) = workspace.filter(|w| !w.trim().is_empty()) {
        return Ok(PathBuf::from(workspace).join(".stores"));
    }
    Ok(stores_dir.to_path_buf())
}

pub(crate) fn resolve_marker_path(stores_dir: &Path, marker_path: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Some(marker_stores_dir) = marker_path
        .parent()
        .and_then(|runs| runs.parent())
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(".stores"))
    {
        if let Ok(stripped) = path.strip_prefix(".stores") {
            return marker_stores_dir.join(stripped);
        }
        return marker_stores_dir.join(path);
    }
    resolve_transcript_path(stores_dir, path)
}

pub fn list_for_task(stores_dir: &Path, display_id: &str) -> Result<Vec<RunTranscript>> {
    let db_path = stores_dir.join("db.sqlite");
    let conn = Connection::open(&db_path)
        .with_context(|| format!("failed to open substrate DB {}", db_path.display()))?;

    // MAJOR 2 (T072 r4): use the guarded helper — creating the VIEW directly
    // here would bypass the tasks-table existence check and install an invalid
    // VIEW when tasks is not installed.  If tasks is absent, return a clean
    // error rather than a cryptic SQL error about a missing base table.
    let tasks_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !tasks_exists {
        anyhow::bail!("tasks store not installed; cannot query runs");
    }
    crate::db::ensure_runs_view_if_tasks_exists(&conn).context("apply runs view DDL")?;

    // Query the runs VIEW — the substrate's official query surface for
    // (display_id, phase, cycle, role, transcript_path) tuples.  JSON decoding
    // stays inside SQLite (json_each / json_extract); Rust only handles rows.
    let mut stmt = conn
        .prepare(
            "SELECT phase, cycle, role, transcript_path \
             FROM runs \
             WHERE display_id = ?1 \
             ORDER BY phase, cycle, role",
        )
        .context("prepare runs view query")?;

    let view_rows: Vec<(i64, i64, String, String)> = stmt
        .query_map([display_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .context("query runs view")?
        .collect::<rusqlite::Result<_>>()
        .context("collect runs view rows")?;

    if view_rows.is_empty() {
        bail!("no transcript backlinks found for {display_id} in tasks.cycles");
    }

    let mut rows = Vec::new();
    for (phase, cycle, role, path_str) in view_rows {
        let path = PathBuf::from(&path_str);
        let read_path = resolve_transcript_path(stores_dir, &path);
        if !read_path.exists() {
            bail!(
                "missing transcript for {display_id} phase {phase} cycle {cycle} role {role}: {} does not exist (resolved to {})",
                path.display(),
                read_path.display()
            );
        }
        rows.push(RunTranscript {
            display_id: display_id.to_string(),
            phase,
            cycle,
            role,
            path,
        });
    }

    Ok(rows)
}

pub fn find_transcript(
    stores_dir: &Path,
    display_id: &str,
    phase: i64,
    cycle: Option<i64>,
    role: &str,
) -> Result<RunTranscript> {
    let db_path = stores_dir.join("db.sqlite");
    let conn = Connection::open(&db_path)
        .with_context(|| format!("failed to open substrate DB {}", db_path.display()))?;

    // MAJOR 2 (T072 r4): use the guarded helper — do NOT create the VIEW
    // directly here (bypasses tasks-table existence check).  Clean error when
    // tasks store is absent.
    let tasks_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if !tasks_exists {
        anyhow::bail!("tasks store not installed; cannot query runs");
    }
    crate::db::ensure_runs_view_if_tasks_exists(&conn).context("apply runs view DDL")?;

    // MAJOR 1 (T072 r4): when --cycle is absent, default to the LATEST cycle
    // for the given phase+role (highest cycle number — deterministic DESC
    // ordering).  --cycle remains an optional disambiguator for callers that
    // want a specific historical cycle.
    //
    // When --cycle is provided: exact match (existing behaviour).
    // When --cycle is absent:  ORDER BY cycle DESC LIMIT 1 → latest cycle.
    let (p, c, r, path_str): (i64, i64, String, String) = if let Some(cyc) = cycle {
        // Exact-cycle path: check existence first for a clean error.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM runs \
                 WHERE display_id = ?1 AND phase = ?2 AND role = ?3 AND cycle = ?4",
                params![display_id, phase, role, cyc],
                |r| r.get(0),
            )
            .context("count matching runs rows (exact cycle)")?;
        if count == 0 {
            bail!("missing transcript for {display_id} phase {phase} role {role} cycle {cyc}");
        }
        conn.query_row(
            "SELECT phase, cycle, role, transcript_path \
             FROM runs \
             WHERE display_id = ?1 AND phase = ?2 AND role = ?3 AND cycle = ?4",
            params![display_id, phase, role, cyc],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .context("fetch matching runs row (exact cycle)")?
    } else {
        // Latest-cycle path: ORDER BY cycle DESC LIMIT 1.
        // Returns the highest cycle number — deterministic when multiple cycles
        // exist for the same phase+role (e.g. executor retry cycles).
        let result = conn.query_row(
            "SELECT phase, cycle, role, transcript_path \
                 FROM runs \
                 WHERE display_id = ?1 AND phase = ?2 AND role = ?3 \
                 ORDER BY cycle DESC LIMIT 1",
            params![display_id, phase, role],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        );
        match result {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                bail!("missing transcript for {display_id} phase {phase} role {role}");
            }
            Err(e) => return Err(e).context("fetch latest runs row"),
        }
    };

    let path = PathBuf::from(&path_str);
    let read_path = resolve_transcript_path(stores_dir, &path);
    if !read_path.exists() {
        bail!(
            "missing transcript for {display_id} phase {p} cycle {c} role {r}: {} does not exist (resolved to {})",
            path.display(),
            read_path.display()
        );
    }

    Ok(RunTranscript {
        display_id: display_id.to_string(),
        phase: p,
        cycle: c,
        role: r,
        path,
    })
}

fn resolve_transcript_path(stores_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Ok(stripped) = path.strip_prefix(".stores") {
        if let Some(root) = stores_dir.parent() {
            return root.join(".stores").join(stripped);
        }
    }
    stores_dir.join(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(stores.join("runs")).unwrap();
        fs::write(
            stores.join("runs/executor-session.jsonl"),
            r#"{"role":"executor","summary":"fixture executor"}"#,
        )
        .unwrap();
        fs::write(
            stores.join("runs/review-session.jsonl"),
            r#"{"role":"code-reviewer","gate":"PASS"}"#,
        )
        .unwrap();
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        let cycles = serde_json::json!([
            {
                "phase": 2,
                "cycle": 2,
                "executor": {"transcript_path": ".stores/runs/executor-session.jsonl"}
            },
            {
                "phase": 2,
                "cycle": 1,
                "executor": {"transcript_path": ".stores/runs/executor-session.jsonl"},
                "review": {"transcript_path": ".stores/runs/review-session.jsonl"}
            }
        ]);
        conn.execute(
            "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
            params!["T999", serde_json::to_string(&cycles).unwrap()],
        )
        .unwrap();
        tmp
    }

    #[test]
    fn list_outputs_deterministic_order_from_cycle_backlinks() {
        let tmp = fixture();
        let rows = list_for_task(&tmp.path().join(".stores"), "T999").unwrap();
        let keys: Vec<_> = rows
            .iter()
            .map(|r| {
                (
                    r.phase,
                    r.cycle,
                    r.role.as_str(),
                    r.path.to_string_lossy().to_string(),
                )
            })
            .collect();
        assert_eq!(
            keys,
            vec![
                (
                    2,
                    1,
                    "code-reviewer",
                    ".stores/runs/review-session.jsonl".to_string()
                ),
                (
                    2,
                    1,
                    "executor",
                    ".stores/runs/executor-session.jsonl".to_string()
                ),
                (
                    2,
                    2,
                    "executor",
                    ".stores/runs/executor-session.jsonl".to_string()
                ),
            ]
        );
    }

    #[test]
    fn show_finds_existing_transcript_backlink() {
        let tmp = fixture();
        let row =
            find_transcript(&tmp.path().join(".stores"), "T999", 2, Some(1), "executor").unwrap();
        assert_eq!(
            row.path,
            PathBuf::from(".stores/runs/executor-session.jsonl")
        );
    }

    #[test]
    fn current_resolves_marker_before_agent_run_completion() {
        let tmp = fixture();
        let stores = tmp.path().join(".stores");
        let transcript = stores.join("runs/live-session.jsonl");
        let stderr = stores.join("runs/live-session.stderr.log");
        fs::write(&transcript, "first live line\n").unwrap();
        fs::write(&stderr, "stderr live line\n").unwrap();
        fs::write(
            stores.join("runs/current-T999-executor.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "phase": 7,
                "cycle": 2,
                "role": "executor",
                "runner": "pi",
                "session_id": "live-session",
                "status": "running",
                "updated_at": "2026-05-11T02:05:47Z",
                "transcript_path": transcript,
                "stderr_log_path": stderr,
            }))
            .unwrap(),
        )
        .unwrap();

        let current = find_current_run(&stores, "T999", Some("executor")).unwrap();
        assert_eq!(current.marker.display_id, "T999");
        assert_eq!(current.marker.role, "executor");
        assert_eq!(current.marker.status.as_deref(), Some("running"));
        let body = fs::read_to_string(current.marker.transcript_path.as_ref().unwrap()).unwrap();
        assert_eq!(body, "first live line\n");
    }

    #[test]
    fn current_status_reads_session_status_next_to_transcript() {
        let tmp = fixture();
        let stores = tmp.path().join(".stores");
        let transcript = stores.join("runs/live-session.jsonl");
        let status_path = stores.join("runs/live-session/status.json");
        fs::create_dir_all(status_path.parent().unwrap()).unwrap();
        fs::write(&transcript, "line\n").unwrap();
        fs::write(
            &status_path,
            r#"{
  "last_event_at": "2026-05-11T02:06:00Z",
  "last_event_type": "retry",
  "current_activity": "api_retry"
}"#,
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-executor.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "executor",
                "session_id": "live-session",
                "status": "running",
                "transcript_path": transcript,
            }))
            .unwrap(),
        )
        .unwrap();

        let current = find_current_run(&stores, "T999", Some("executor")).unwrap();
        assert_eq!(current_status_path(&stores, &current).unwrap(), status_path);
        let status = read_current_status(&stores, &current).unwrap().unwrap();
        assert_eq!(status.last_event_type.as_deref(), Some("retry"));
        assert_eq!(status.current_activity.as_deref(), Some("api_retry"));
    }

    #[test]
    fn current_status_prefers_explicit_marker_status_path() {
        let tmp = fixture();
        let stores = tmp.path().join(".stores");
        let derived = stores.join("runs/live-session/status.json");
        let explicit = stores.join("runs/custom-status.json");
        fs::create_dir_all(derived.parent().unwrap()).unwrap();
        fs::write(
            &derived,
            r#"{
  "last_event_at": "2026-05-11T02:00:00Z",
  "last_event_type": "heartbeat",
  "current_activity": null
}"#,
        )
        .unwrap();
        fs::write(
            &explicit,
            r#"{
  "last_event_at": "2026-05-11T02:06:00Z",
  "last_event_type": "tool_start",
  "current_activity": "tool:bash"
}"#,
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-executor.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "executor",
                "session_id": "live-session",
                "status": "running",
                "transcript_path": stores.join("runs/live-session.jsonl"),
                "status_path": explicit,
            }))
            .unwrap(),
        )
        .unwrap();

        let current = find_current_run(&stores, "T999", Some("executor")).unwrap();
        assert_eq!(current_status_path(&stores, &current).unwrap(), explicit);
        let status = read_current_status(&stores, &current).unwrap().unwrap();
        assert_eq!(status.last_event_type.as_deref(), Some("tool_start"));
        assert_eq!(status.current_activity.as_deref(), Some("tool:bash"));
    }

    #[test]
    fn current_without_role_picks_latest_updated_marker() {
        let tmp = fixture();
        let stores = tmp.path().join(".stores");
        fs::write(
            stores.join("runs/current-T999-planner.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "planner",
                "status": "running",
                "updated_at": "2026-05-11T02:00:00Z",
                "transcript_path": stores.join("runs/planner.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-executor.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "executor",
                "status": "running",
                "updated_at": "2026-05-11T02:05:47Z",
                "transcript_path": stores.join("runs/executor.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();

        let current = find_current_run(&stores, "T999", None).unwrap();
        assert_eq!(current.marker.role, "executor");
    }

    #[test]
    fn current_without_role_prefers_running_marker_with_fresher_semantic_status() {
        let tmp = fixture();
        let stores = tmp.path().join(".stores");
        let executor_status = stores.join("runs/executor-session/status.json");
        fs::create_dir_all(executor_status.parent().unwrap()).unwrap();
        fs::write(
            &executor_status,
            r#"{
  "last_event_at": "2026-05-11T04:40:33Z",
  "last_event_type": "heartbeat",
  "current_activity": "tool:bash"
}"#,
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-planner.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "planner",
                "status": "completed",
                "updated_at": "2026-05-11T04:45:00Z",
                "transcript_path": stores.join("runs/planner.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-executor.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "executor",
                "session_id": "executor-session",
                "status": "running",
                "updated_at": "2026-05-11T04:30:09Z",
                "transcript_path": stores.join("runs/executor-session.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();

        let current = find_current_run(&stores, "T999", None).unwrap();
        assert_eq!(current.marker.role, "executor");
        let status = read_current_status(&stores, &current).unwrap().unwrap();
        assert_eq!(status.last_event_at.as_deref(), Some("2026-05-11T04:40:33Z"));
    }

    #[test]
    fn current_without_role_treats_missing_semantic_status_as_non_authoritative() {
        let tmp = fixture();
        let stores = tmp.path().join(".stores");
        fs::write(
            stores.join("runs/current-T999-planner.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "planner",
                "status": "completed",
                "updated_at": "2026-05-11T04:45:00Z",
                "transcript_path": stores.join("runs/planner.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-executor.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "executor",
                "session_id": "missing-status-session",
                "status": "running",
                "updated_at": "2026-05-11T04:30:09Z",
                "transcript_path": stores.join("runs/executor.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();

        let current = find_current_run(&stores, "T999", None).unwrap();
        assert_eq!(current.marker.role, "planner");
    }

    #[test]
    fn running_marker_with_stale_heartbeat_and_no_live_owner_is_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(stores.join("runs/stale-session")).unwrap();
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        conn.execute(
            "CREATE TABLE tasks (id INTEGER PRIMARY KEY, display_id TEXT UNIQUE NOT NULL, drive_pid INTEGER, workspace_path TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE dispatch_locks (store TEXT, row_id INTEGER, agent_name TEXT, finished_at TEXT, pid INTEGER)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, display_id, drive_pid, workspace_path) VALUES (1, 'T999', NULL, '')",
            [],
        )
        .unwrap();
        fs::write(
            stores.join("runs/stale-session/status.json"),
            r#"{
  "last_event_at": "2026-05-11T01:00:00Z",
  "last_event_type": "heartbeat",
  "current_activity": "tool:bash"
}"#,
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-executor.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "executor",
                "session_id": "stale-session",
                "status": "running",
                "updated_at": "2026-05-11T01:00:00Z",
                "transcript_path": stores.join("runs/stale-session.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();

        let current = find_current_run(&stores, "T999", Some("executor")).unwrap();
        let liveness = current_run_liveness(
            &stores,
            &current,
            DateTime::parse_from_rfc3339("2026-05-11T02:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();
        assert_eq!(
            liveness,
            CurrentRunLiveness::RunningStale {
                reason: "running marker has stale last_event_at/updated_at 2026-05-11T01:00:00Z and no live owner".to_string()
            }
        );
    }

    #[test]
    fn running_marker_with_live_drive_pid_is_live_even_with_stale_heartbeat() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(stores.join("runs/stale-session")).unwrap();
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        conn.execute(
            "CREATE TABLE tasks (id INTEGER PRIMARY KEY, display_id TEXT UNIQUE NOT NULL, drive_pid INTEGER, workspace_path TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE dispatch_locks (store TEXT, row_id INTEGER, agent_name TEXT, finished_at TEXT, pid INTEGER)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, display_id, drive_pid, workspace_path) VALUES (1, 'T999', ?1, '')",
            [std::process::id() as i64],
        )
        .unwrap();
        fs::write(
            stores.join("runs/stale-session/status.json"),
            r#"{
  "last_event_at": "2026-05-11T01:00:00Z",
  "last_event_type": "heartbeat",
  "current_activity": "tool:bash"
}"#,
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-executor.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "executor",
                "session_id": "stale-session",
                "status": "running",
                "updated_at": "2026-05-11T01:00:00Z",
                "transcript_path": stores.join("runs/stale-session.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();

        let current = find_current_run(&stores, "T999", Some("executor")).unwrap();
        let liveness = current_run_liveness(
            &stores,
            &current,
            DateTime::parse_from_rfc3339("2026-05-11T02:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )
        .unwrap();
        assert_eq!(liveness, CurrentRunLiveness::RunningLive);
    }

    #[test]
    fn stale_running_marker_does_not_outrank_completed_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(stores.join("runs/stale-session")).unwrap();
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        conn.execute(
            "CREATE TABLE tasks (id INTEGER PRIMARY KEY, display_id TEXT UNIQUE NOT NULL, drive_pid INTEGER, workspace_path TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE dispatch_locks (store TEXT, row_id INTEGER, agent_name TEXT, finished_at TEXT, pid INTEGER)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, display_id, drive_pid, workspace_path) VALUES (1, 'T999', NULL, '')",
            [],
        )
        .unwrap();
        fs::write(
            stores.join("runs/stale-session/status.json"),
            r#"{
  "last_event_at": "2026-05-11T01:00:00Z",
  "last_event_type": "heartbeat",
  "current_activity": "tool:bash"
}"#,
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-executor.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "executor",
                "session_id": "stale-session",
                "status": "running",
                "updated_at": "2026-05-11T01:00:00Z",
                "transcript_path": stores.join("runs/stale-session.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            stores.join("runs/current-T999-code_reviewer.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "display_id": "T999",
                "role": "code_reviewer",
                "status": "completed",
                "updated_at": "2026-05-11T01:30:00Z",
                "transcript_path": stores.join("runs/review.jsonl"),
            }))
            .unwrap(),
        )
        .unwrap();

        let current = find_current_run(&stores, "T999", None).unwrap();
        assert_eq!(current.marker.role, "code_reviewer");
    }

    #[test]
    fn missing_transcript_errors_cleanly() {
        let tmp = fixture();
        let err =
            find_transcript(&tmp.path().join(".stores"), "T999", 3, None, "executor").unwrap_err();
        assert!(err
            .to_string()
            .contains("missing transcript for T999 phase 3 role executor"));
    }

    #[test]
    fn missing_linked_file_errors_cleanly() {
        let tmp = fixture();
        fs::remove_file(tmp.path().join(".stores/runs/review-session.jsonl")).unwrap();
        let err = list_for_task(&tmp.path().join(".stores"), "T999").unwrap_err();
        assert!(err
            .to_string()
            .contains("missing transcript for T999 phase 2 cycle 1 role code-reviewer"));
    }

    // ---- T072 r2: runs VIEW tests ----

    /// Build a minimal in-memory DB (tasks table + VIEW) and verify the VIEW
    /// exists and returns expected columns.  Uses rusqlite::Connection directly
    /// to isolate the DDL from the filesystem fixture.
    #[test]
    fn runs_view_exists_after_ddl_applied() {
        use crate::codegen::ddl::RUNS_VIEW_DDL;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        conn.execute_batch(RUNS_VIEW_DDL)
            .expect("RUNS_VIEW_DDL must apply cleanly");
        // Verify the view exists via sqlite_master.
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='view' AND name='runs'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "runs VIEW must exist after DDL is applied");
    }

    /// VIEW returns expected rows from a fixture cycles JSON blob.
    #[test]
    fn runs_view_returns_expected_rows() {
        use crate::codegen::ddl::RUNS_VIEW_DDL;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        conn.execute_batch(RUNS_VIEW_DDL).unwrap();
        let cycles = serde_json::json!([
            {
                "phase": 1, "cycle": 1,
                "executor": {"transcript_path": ".stores/runs/ex1.jsonl"},
                "review":   {"transcript_path": ".stores/runs/rv1.jsonl"}
            },
            {
                "phase": 2, "cycle": 1,
                "executor": {"transcript_path": ".stores/runs/ex2.jsonl"}
            }
        ]);
        conn.execute(
            "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
            params!["T001", serde_json::to_string(&cycles).unwrap()],
        )
        .unwrap();

        let mut stmt = conn
            .prepare(
                "SELECT display_id, phase, cycle, role, transcript_path \
                 FROM runs WHERE display_id='T001' ORDER BY phase, cycle, role",
            )
            .unwrap();
        let rows: Vec<(String, i64, i64, String, String)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();

        assert_eq!(rows.len(), 3, "expected 3 rows (2 executor + 1 reviewer)");
        assert_eq!(
            rows[0],
            (
                "T001".into(),
                1,
                1,
                "code-reviewer".into(),
                ".stores/runs/rv1.jsonl".into()
            )
        );
        assert_eq!(
            rows[1],
            (
                "T001".into(),
                1,
                1,
                "executor".into(),
                ".stores/runs/ex1.jsonl".into()
            )
        );
        assert_eq!(
            rows[2],
            (
                "T001".into(),
                2,
                1,
                "executor".into(),
                ".stores/runs/ex2.jsonl".into()
            )
        );
    }

    /// Empty cycles array produces zero rows without error.
    #[test]
    fn runs_view_empty_cycles_no_error() {
        use crate::codegen::ddl::RUNS_VIEW_DDL;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        conn.execute_batch(RUNS_VIEW_DDL).unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
            params!["T002", "[]"],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM runs WHERE display_id='T002'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "empty cycles must yield zero runs rows");
    }

    // ---- T072 r4: MAJOR 1 — latest-cycle default ----

    /// When --cycle is absent, `find_transcript` returns the row with the
    /// highest cycle number (cycle DESC ordering — deterministic).
    #[test]
    fn show_without_cycle_returns_latest_cycle() {
        let tmp = fixture();
        // fixture has phase=2 executor at cycle=1 AND cycle=2; cycle=2 is latest.
        let row = find_transcript(
            &tmp.path().join(".stores"),
            "T999",
            2,
            None, // no --cycle: should default to latest (cycle=2)
            "executor",
        )
        .unwrap();
        assert_eq!(
            row.cycle, 2,
            "show without --cycle must return the highest cycle (latest)"
        );
        assert_eq!(
            row.path,
            PathBuf::from(".stores/runs/executor-session.jsonl")
        );
    }

    /// When --cycle is provided, `find_transcript` returns that exact cycle.
    #[test]
    fn show_with_explicit_cycle_returns_that_cycle() {
        let tmp = fixture();
        // fixture has executor at cycle=1 and cycle=2; request cycle=1 explicitly.
        let row = find_transcript(
            &tmp.path().join(".stores"),
            "T999",
            2,
            Some(1), // explicit --cycle=1
            "executor",
        )
        .unwrap();
        assert_eq!(
            row.cycle, 1,
            "explicit --cycle must return that exact cycle"
        );
        assert_eq!(
            row.path,
            PathBuf::from(".stores/runs/executor-session.jsonl")
        );
    }

    // ---- T072 r4: MAJOR 2 — tasks-absent clean error ----

    /// When the tasks table is absent, `list_for_task` returns a clean error
    /// rather than a cryptic SQL error about a missing base table.
    #[test]
    fn list_errors_cleanly_when_tasks_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(&stores).unwrap();
        // Open a DB with NO tasks table (substrate-only, or just empty).
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        drop(conn); // close; list_for_task opens its own connection

        let err = list_for_task(&stores, "T999").unwrap_err();
        assert!(
            err.to_string().contains("tasks store not installed"),
            "expected clean tasks-absent error, got: {err}"
        );
    }

    /// When the tasks table is absent, `find_transcript` returns a clean error.
    #[test]
    fn show_errors_cleanly_when_tasks_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(&stores).unwrap();
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        drop(conn);

        let err = find_transcript(&stores, "T999", 1, None, "executor").unwrap_err();
        assert!(
            err.to_string().contains("tasks store not installed"),
            "expected clean tasks-absent error, got: {err}"
        );
    }

    /// VIEW is idempotent: applying RUNS_VIEW_DDL twice must not error.
    #[test]
    fn runs_view_ddl_is_idempotent() {
        use crate::codegen::ddl::RUNS_VIEW_DDL;
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        conn.execute_batch(RUNS_VIEW_DDL).unwrap();
        conn.execute_batch(RUNS_VIEW_DDL).expect(
            "applying RUNS_VIEW_DDL a second time must be a no-op (CREATE VIEW IF NOT EXISTS)",
        );
    }
}
