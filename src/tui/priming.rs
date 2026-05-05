//! Side-car priming pipeline.
//!
//! `build(scope, conn)` gathers context-bundle bytes appropriate to the
//! scope and packages them with the operator's approval token (read from
//! `STORES_APPROVE_TOKEN`). The result can be `write_to_tempfile()`'d for
//! hand-off to a Claude Code child process.

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use std::path::PathBuf;
use tempfile::NamedTempFile;

use super::sidecar::{system_prompt::system_prompt_for, SidecarScope};

const APPROVE_TOKEN_ENV: &str = "STORES_APPROVE_TOKEN";

/// Excerpts loaded from on-disk docs when relevant. Best-effort: missing
/// files yield empty strings (the priming still emits a header so the
/// receiving side-car can see what the harness tried to attach).
pub struct DocsExcerpts;

impl DocsExcerpts {
    fn engine_health() -> String {
        std::fs::read_to_string("docs/engine-health.md").unwrap_or_default()
    }
    fn philosophy() -> String {
        std::fs::read_to_string("docs/philosophy.md").unwrap_or_default()
    }
    fn observations_schema() -> String {
        std::fs::read_to_string("stores/observations/schema.yaml").unwrap_or_default()
    }
}

/// Fully-built priming bundle. `body` is the long context dump; the
/// system prompt + initial message are computed from `scope` + `token`.
#[derive(Debug)]
pub struct Priming {
    pub scope: SidecarScope,
    pub token: String,
    pub system_prompt: String,
    pub initial_message: String,
    pub body: String,
}

/// Build a priming bundle for `scope` against `conn`. Reads the
/// `STORES_APPROVE_TOKEN` env var (errors fail-loud when unset).
pub fn build(scope: SidecarScope, conn: &Connection) -> Result<Priming> {
    let token = std::env::var(APPROVE_TOKEN_ENV).map_err(|_| {
        anyhow!(
            "{APPROVE_TOKEN_ENV} env var must be set before spawning a side-car; \
             run `stores auth show` and export it for this shell"
        )
    })?;
    let body = match &scope {
        SidecarScope::PerRow { display_id, .. } => build_per_row(conn, display_id)?,
        SidecarScope::General => build_general(conn)?,
        SidecarScope::ObsDraft => build_obs_draft(conn)?,
    };
    let system_prompt = system_prompt_for(&scope);
    let initial_message = initial_message_for(&scope, &token);
    Ok(Priming {
        scope,
        token,
        system_prompt,
        initial_message,
        body,
    })
}

/// Render the operator-facing initial chat message: a one-line scope brief
/// and the verbatim approval token. The Claude Code child receives this as
/// its `--message` argument.
pub fn initial_message_for(scope: &SidecarScope, token: &str) -> String {
    let brief = match scope {
        SidecarScope::PerRow { display_id, fresh } => format!(
            "You are reviewing {display_id} (fresh={fresh}). The priming file is attached. \
             Read it, then propose one substrate-changing action and await my assent."
        ),
        SidecarScope::General => {
            "You are in orchestrator-mode general scope. The priming file lists every live row \
             plus engine-health and philosophy excerpts. Propose one substrate-changing action \
             and await my assent."
                .to_string()
        }
        SidecarScope::ObsDraft => {
            "You are drafting a new observation. The priming file lists live rows plus the \
             observations schema. Ask me one or two clarifying questions, draft the obs, \
             then propose `stores observations add ...` and await my assent."
                .to_string()
        }
    };
    format!(
        "{brief}\n\n\
         APPROVE_TOKEN={token}\n\n\
         (Token is for tier-A writes only — never echo, never log.)"
    )
}

impl Priming {
    /// Write the body to a NamedTempFile. Caller owns the file; dropping
    /// removes it from disk.
    pub fn write_to_tempfile(&self) -> Result<NamedTempFile> {
        let mut f = NamedTempFile::new()
            .context("failed to mkstemp for side-car priming body")?;
        std::io::Write::write_all(&mut f, self.body.as_bytes())
            .context("failed to write priming body to tempfile")?;
        Ok(f)
    }
}

// ---------------------------------------------------------------------------
// Scope-specific body builders
// ---------------------------------------------------------------------------

fn build_per_row(conn: &Connection, display_id: &str) -> Result<String> {
    let mut out = String::new();
    out.push_str(&format!("# Side-car priming · per-row · {display_id}\n\n"));

    out.push_str("## Row JSON\n\n");
    let row_json = fetch_row_json(conn, display_id)?;
    out.push_str("```json\n");
    out.push_str(&row_json);
    out.push_str("\n```\n\n");

    out.push_str("## Linked observations / tasks\n\n");
    for linked in fetch_linked_rows(conn, display_id)? {
        out.push_str("```json\n");
        out.push_str(&linked);
        out.push_str("\n```\n\n");
    }

    out.push_str("## wrap_log entries\n\n");
    out.push_str(&fetch_wrap_log(conn, display_id).unwrap_or_default());
    out.push('\n');

    out.push_str("## transition_history\n\n");
    out.push_str(&fetch_transitions(conn, display_id).unwrap_or_default());
    out.push('\n');

    out.push_str("## run-log tail\n\n");
    out.push_str(&run_log_tail(display_id).unwrap_or_default());
    out.push('\n');

    Ok(out)
}

fn build_general(conn: &Connection) -> Result<String> {
    let mut out = String::new();
    out.push_str("# Side-car priming · orchestrator-mode general\n\n");
    out.push_str("## Live rows\n\n");
    out.push_str(&fetch_live_rows(conn)?);
    out.push_str("\n## docs/engine-health.md\n\n");
    out.push_str(&DocsExcerpts::engine_health());
    out.push_str("\n## docs/philosophy.md\n\n");
    out.push_str(&DocsExcerpts::philosophy());
    Ok(out)
}

fn build_obs_draft(conn: &Connection) -> Result<String> {
    let mut out = String::new();
    out.push_str("# Side-car priming · obs-drafting\n\n");
    out.push_str("## Live rows (dedup reference)\n\n");
    out.push_str(&fetch_live_rows(conn)?);
    out.push_str("\n## stores/observations/schema.yaml\n\n");
    out.push_str(&DocsExcerpts::observations_schema());
    Ok(out)
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

fn fetch_row_json(conn: &Connection, display_id: &str) -> Result<String> {
    let table = if display_id.starts_with('T') {
        "tasks"
    } else if display_id.starts_with('L') {
        "observations"
    } else {
        return Err(anyhow!(
            "unknown display_id prefix in '{display_id}' (expected T### or L###)"
        ));
    };
    let sql = if table == "tasks" {
        "SELECT json_object('display_id', display_id, 'status', status, \
         'title', title, 'tier_hint', tier_hint, 'claimed_by', claimed_by, \
         'wrap_log', wrap_log, 'linked_observations', linked_observations) \
         FROM tasks WHERE display_id = ?1"
    } else {
        "SELECT json_object('display_id', display_id, 'status', status, \
         'summary', summary, 'priority', priority, 'body', body, \
         'intent_contract', intent_contract, 'task_id', task_id) \
         FROM observations WHERE display_id = ?1"
    };
    let s: Option<String> = conn
        .query_row(sql, [display_id], |r| r.get::<_, Option<String>>(0))
        .with_context(|| format!("row '{display_id}' not found in {table}"))?;
    Ok(s.unwrap_or_else(|| format!("{{\"display_id\": \"{display_id}\"}}")))
}

fn fetch_linked_rows(conn: &Connection, display_id: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if display_id.starts_with('T') {
        // Tasks → linked_observations JSON array.
        let raw: Option<String> = conn
            .query_row(
                "SELECT linked_observations FROM tasks WHERE display_id = ?1",
                [display_id],
                |r| r.get(0),
            )
            .ok();
        let ids: Vec<String> = raw
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        for lid in ids {
            if let Ok(j) = fetch_row_json(conn, &lid) {
                out.push(j);
            }
        }
    } else if display_id.starts_with('L') {
        // Observations → task_id soft-FK.
        let task: Option<String> = conn
            .query_row(
                "SELECT task_id FROM observations WHERE display_id = ?1",
                [display_id],
                |r| r.get(0),
            )
            .ok()
            .flatten();
        if let Some(tid) = task {
            if !tid.is_empty() {
                if let Ok(j) = fetch_row_json(conn, &tid) {
                    out.push(j);
                }
            }
        }
    }
    Ok(out)
}

fn fetch_wrap_log(conn: &Connection, display_id: &str) -> Result<String> {
    if !display_id.starts_with('T') {
        return Ok(String::new());
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT wrap_log FROM tasks WHERE display_id = ?1",
            [display_id],
            |r| r.get(0),
        )
        .ok()
        .flatten();
    Ok(raw.unwrap_or_default())
}

fn fetch_transitions(conn: &Connection, display_id: &str) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT occurred_at, from_status, to_status, verb, invoker \
         FROM transition_history WHERE display_id = ?1 ORDER BY id ASC",
    )?;
    let rows = stmt.query_map([display_id], |r| {
        Ok(format!(
            "{} {} → {} via {} ({})",
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?.unwrap_or_else(|| "∅".into()),
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut out = String::new();
    for r in rows.flatten() {
        out.push_str(&r);
        out.push('\n');
    }
    Ok(out)
}

fn fetch_live_rows(conn: &Connection) -> Result<String> {
    let mut out = String::new();
    let mut stmt = conn.prepare(
        "SELECT display_id, status, COALESCE(title, '') FROM tasks \
         WHERE status NOT IN ('accepted', 'complete', 'cargo_installed', 'schema_migrated') \
         ORDER BY display_id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(format!(
            "{}  [{}]  {}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for r in rows.flatten() {
        out.push_str(&r);
        out.push('\n');
    }
    out.push('\n');
    let mut stmt = conn.prepare(
        "SELECT display_id, status, COALESCE(summary, '') FROM observations \
         WHERE status NOT IN ('resolved', 'wont_fix') ORDER BY display_id ASC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(format!(
            "{}  [{}]  {}",
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for r in rows.flatten() {
        out.push_str(&r);
        out.push('\n');
    }
    Ok(out)
}

fn run_log_tail(display_id: &str) -> Option<String> {
    // Best-effort: the most recent .stores/runs/<session>.jsonl mentioning this row.
    let dir: PathBuf = crate::paths::stores_dir().ok()?.join("runs");
    let entries = std::fs::read_dir(&dir).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for ent in entries.flatten() {
        let p = ent.path();
        if p.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        let mtime = ent.metadata().and_then(|m| m.modified()).ok()?;
        let body = std::fs::read_to_string(&p).ok()?;
        if !body.contains(display_id) {
            continue;
        }
        match &best {
            Some((t, _)) if *t >= mtime => {}
            _ => best = Some((mtime, p)),
        }
    }
    let (_, path) = best?;
    let body = std::fs::read_to_string(&path).ok()?;
    let lines: Vec<&str> = body.lines().collect();
    let tail = lines
        .iter()
        .rev()
        .take(40)
        .rev()
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    Some(tail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn make_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                id INTEGER PRIMARY KEY,
                display_id TEXT,
                status TEXT,
                title TEXT,
                tier_hint TEXT,
                claimed_by TEXT,
                wrap_log TEXT,
                linked_observations TEXT
             );
             CREATE TABLE observations (
                id INTEGER PRIMARY KEY,
                display_id TEXT,
                status TEXT,
                summary TEXT,
                priority TEXT,
                body TEXT,
                intent_contract TEXT,
                task_id TEXT
             );
             CREATE TABLE transition_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                store TEXT NOT NULL,
                row_id INTEGER NOT NULL,
                display_id TEXT NOT NULL,
                from_status TEXT,
                to_status TEXT NOT NULL,
                verb TEXT NOT NULL,
                invoker TEXT NOT NULL,
                policy_ref TEXT,
                policies_hash TEXT,
                occurred_at TEXT NOT NULL
             );
             INSERT INTO tasks VALUES
               (1, 'T999', 'code_review', 'Demo task', 'T2', 'agent-x',
                '[{\"phase\":1,\"summary\":\"hello wrap\"}]',
                '[\"L001\"]');
             INSERT INTO observations VALUES
               (1, 'L001', 'open', 'Demo obs', 'normal',
                'body text', '{\"contract_state\":\"draft\"}', 'T999');
             INSERT INTO transition_history (store,row_id,display_id,from_status,to_status,verb,invoker,occurred_at)
               VALUES ('tasks',1,'T999','planning','code_review','submit-execute','ai_autonomous','2026-05-05T10:00:00Z');
            ",
        )
        .unwrap();
        conn
    }

    static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
        std::sync::OnceLock::new();

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        let m = ENV_LOCK.get_or_init(|| std::sync::Mutex::new(()));
        match m.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn with_token<F: FnOnce()>(value: &str, f: F) {
        let _g = env_guard();
        let prev = std::env::var(APPROVE_TOKEN_ENV).ok();
        std::env::set_var(APPROVE_TOKEN_ENV, value);
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match prev {
            Some(v) => std::env::set_var(APPROVE_TOKEN_ENV, v),
            None => std::env::remove_var(APPROVE_TOKEN_ENV),
        }
        if let Err(e) = res {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn per_row_priming_contains_row_json_linked_obs_wrap_log_and_token() {
        let conn = make_test_db();
        with_token("tok-test-123", || {
            let p = build(
                SidecarScope::PerRow {
                    display_id: "T999".to_string(),
                    fresh: false,
                },
                &conn,
            )
            .unwrap();
            assert!(p.body.contains("T999"), "row JSON missing");
            assert!(p.body.contains("Demo task"), "task title missing");
            assert!(p.body.contains("L001"), "linked obs row missing");
            assert!(p.body.contains("Demo obs"), "linked obs body missing");
            assert!(p.body.contains("hello wrap"), "wrap_log missing");
            assert!(p.body.contains("planning → code_review"), "transitions missing");
            assert_eq!(p.token, "tok-test-123");
            assert!(p.initial_message.contains("tok-test-123"), "token in initial");
        });
    }

    #[test]
    fn token_required_returns_error_when_unset() {
        let _g = env_guard();
        let conn = make_test_db();
        let prev = std::env::var(APPROVE_TOKEN_ENV).ok();
        std::env::remove_var(APPROVE_TOKEN_ENV);
        let err = build(SidecarScope::General, &conn).unwrap_err();
        let msg = format!("{err}");
        if let Some(v) = prev {
            std::env::set_var(APPROVE_TOKEN_ENV, v);
        }
        assert!(
            msg.contains(APPROVE_TOKEN_ENV),
            "error must reference env var name, got: {msg}"
        );
    }
}
