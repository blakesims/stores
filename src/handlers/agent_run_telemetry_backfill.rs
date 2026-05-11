use anyhow::{Context, Result};
use rusqlite::Connection;
use std::io::BufRead;
use std::path::{Path, PathBuf};

const MAX_BACKFILL_TRANSCRIPT_BYTES: u64 = 256 * 1024;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackfillCounts {
    pub scanned: usize,
    pub updated: usize,
    pub skipped: usize,
    pub parse_failed: usize,
}

#[derive(Debug)]
struct AgentRunRow {
    id: i64,
    transcript_path: String,
    effective_model_id: Option<String>,
    provider_id: Option<String>,
    api_id: Option<String>,
    session_id: Option<String>,
    workspace_path: Option<String>,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
    prompt_cache_hits: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    cost_total: Option<f64>,
    configured_harness_id: Option<String>,
    configured_model_id: Option<String>,
    configured_thinking_effort: Option<String>,
    effective_thinking_effort: Option<String>,
    thinking_effort_source: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct TranscriptTelemetry {
    effective_model_id: Option<String>,
    provider_id: Option<String>,
    api_id: Option<String>,
    tokens_in: Option<i64>,
    tokens_out: Option<i64>,
    cache_read_tokens: Option<i64>,
    cache_write_tokens: Option<i64>,
    cost_total: Option<f64>,
    configured_model_id: Option<String>,
    configured_thinking_effort: Option<String>,
    effective_thinking_effort: Option<String>,
    thinking_effort_source: Option<String>,
}

pub fn run_agent_run_telemetry_backfill() -> Result<()> {
    let db_path = crate::paths::db_path()?;
    let conn = crate::db::open(&db_path)?;
    let counts = backfill_pi_agent_run_telemetry(&conn)?;
    println!(
        "scanned={} updated={} skipped={} parse_failed={}",
        counts.scanned, counts.updated, counts.skipped, counts.parse_failed
    );
    Ok(())
}

pub fn backfill_pi_agent_run_telemetry(conn: &Connection) -> Result<BackfillCounts> {
    if !table_exists(conn, "agent_runs")? {
        return Ok(BackfillCounts::default());
    }

    let rows = load_pi_rows(conn)?;
    let mut counts = BackfillCounts {
        scanned: rows.len(),
        ..BackfillCounts::default()
    };

    for row in rows {
        if row.transcript_path.trim().is_empty() || row.transcript_path == "legacy_unknown" {
            counts.skipped += 1;
            continue;
        }
        if !row_needs_backfill(&row) {
            counts.skipped += 1;
            continue;
        }
        let path = PathBuf::from(&row.transcript_path);
        let extracted = match extract_transcript_telemetry_from_path(&path) {
            Ok(t) => t,
            Err(_) => {
                counts.parse_failed += 1;
                continue;
            }
        };
        if update_row(conn, &row, &path, &extracted)? {
            counts.updated += 1;
        } else {
            counts.skipped += 1;
        }
    }

    Ok(counts)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

fn load_pi_rows(conn: &Connection) -> Result<Vec<AgentRunRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, transcript_path, effective_model_id, provider_id, api_id, session_id, workspace_path, \
                tokens_in, tokens_out, prompt_cache_hits, cache_read_tokens, cache_write_tokens, cost_total, \
                configured_harness_id, configured_model_id, configured_thinking_effort, \
                effective_thinking_effort, thinking_effort_source \
         FROM agent_runs \
         WHERE harness_id = 'pi' OR configured_harness_id = 'pi'",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(AgentRunRow {
                id: r.get(0)?,
                transcript_path: r.get(1)?,
                effective_model_id: r.get(2)?,
                provider_id: r.get(3)?,
                api_id: r.get(4)?,
                session_id: r.get(5)?,
                workspace_path: r.get(6)?,
                tokens_in: r.get(7)?,
                tokens_out: r.get(8)?,
                prompt_cache_hits: r.get(9)?,
                cache_read_tokens: r.get(10)?,
                cache_write_tokens: r.get(11)?,
                cost_total: r.get(12)?,
                configured_harness_id: r.get(13)?,
                configured_model_id: r.get(14)?,
                configured_thinking_effort: r.get(15)?,
                effective_thinking_effort: r.get(16)?,
                thinking_effort_source: r.get(17)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn row_needs_backfill(row: &AgentRunRow) -> bool {
    row.effective_model_id.is_none()
        || row.provider_id.is_none()
        || row.api_id.is_none()
        || row.session_id.is_none()
        || row.workspace_path.is_none()
        || row.tokens_in.is_none()
        || row.tokens_out.is_none()
        || row.prompt_cache_hits.is_none()
        || row.cache_read_tokens.is_none()
        || row.cache_write_tokens.is_none()
        || row.cost_total.is_none()
        || row.configured_harness_id.is_none()
        || row.configured_model_id.is_none()
        || row.configured_thinking_effort.is_none()
        || row.effective_thinking_effort.is_none()
        || row.thinking_effort_source.is_none()
}

fn update_row(
    conn: &Connection,
    row: &AgentRunRow,
    transcript_path: &Path,
    extracted: &TranscriptTelemetry,
) -> Result<bool> {
    let session_id = row
        .session_id
        .is_none()
        .then(|| session_id_from_path(transcript_path))
        .flatten();
    let workspace_path = row
        .workspace_path
        .is_none()
        .then(|| workspace_from_transcript_path(transcript_path))
        .flatten();
    let effective_thinking_effort = if row.effective_thinking_effort.is_none() {
        Some(
            extracted
                .effective_thinking_effort
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        )
    } else {
        None
    };
    let thinking_effort_source = if row.thinking_effort_source.is_none() {
        Some(
            extracted
                .thinking_effort_source
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
        )
    } else {
        None
    };

    let would_update = (row.effective_model_id.is_none() && extracted.effective_model_id.is_some())
        || (row.provider_id.is_none() && extracted.provider_id.is_some())
        || (row.api_id.is_none() && extracted.api_id.is_some())
        || (row.tokens_in.is_none() && extracted.tokens_in.is_some())
        || (row.tokens_out.is_none() && extracted.tokens_out.is_some())
        || (row.prompt_cache_hits.is_none() && extracted.cache_read_tokens.is_some())
        || (row.cache_read_tokens.is_none() && extracted.cache_read_tokens.is_some())
        || (row.cache_write_tokens.is_none() && extracted.cache_write_tokens.is_some())
        || (row.cost_total.is_none() && extracted.cost_total.is_some())
        || (row.configured_harness_id.is_none())
        || (row.configured_model_id.is_none() && extracted.configured_model_id.is_some())
        || (row.configured_thinking_effort.is_none()
            && extracted.configured_thinking_effort.is_some())
        || session_id.is_some()
        || workspace_path.is_some()
        || effective_thinking_effort.is_some()
        || thinking_effort_source.is_some();

    if !would_update {
        return Ok(false);
    }

    conn.execute(
        "UPDATE agent_runs SET \
            effective_model_id = COALESCE(effective_model_id, ?1), \
            provider_id = COALESCE(provider_id, ?2), \
            api_id = COALESCE(api_id, ?3), \
            tokens_in = COALESCE(tokens_in, ?4), \
            tokens_out = COALESCE(tokens_out, ?5), \
            prompt_cache_hits = COALESCE(prompt_cache_hits, ?6), \
            cache_read_tokens = COALESCE(cache_read_tokens, ?7), \
            cache_write_tokens = COALESCE(cache_write_tokens, ?8), \
            cost_total = COALESCE(cost_total, ?9), \
            configured_harness_id = COALESCE(configured_harness_id, 'pi'), \
            configured_model_id = COALESCE(configured_model_id, ?10), \
            configured_thinking_effort = COALESCE(configured_thinking_effort, ?11), \
            effective_thinking_effort = COALESCE(effective_thinking_effort, ?12), \
            thinking_effort_source = COALESCE(thinking_effort_source, ?13), \
            session_id = COALESCE(session_id, ?14), \
            workspace_path = COALESCE(workspace_path, ?15) \
         WHERE id = ?16",
        rusqlite::params![
            extracted.effective_model_id,
            extracted.provider_id,
            extracted.api_id,
            extracted.tokens_in,
            extracted.tokens_out,
            extracted.cache_read_tokens,
            extracted.cache_read_tokens,
            extracted.cache_write_tokens,
            extracted.cost_total,
            extracted.configured_model_id,
            extracted.configured_thinking_effort,
            effective_thinking_effort,
            thinking_effort_source,
            session_id,
            workspace_path,
            row.id,
        ],
    )
    .context("backfill agent_runs telemetry")?;
    Ok(true)
}

fn extract_transcript_telemetry_from_path(path: &Path) -> Result<TranscriptTelemetry> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("open transcript {}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let mut out = TranscriptTelemetry::default();
    let mut line = String::new();
    let mut line_no = 0usize;
    let mut bytes_read = 0u64;
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .with_context(|| format!("read transcript {}", path.display()))?;
        if n == 0 {
            break;
        }
        bytes_read = bytes_read.saturating_add(n as u64);
        line_no += 1;
        absorb_transcript_line(&mut out, &line, line_no)?;
        if transcript_telemetry_complete(&out) || bytes_read >= MAX_BACKFILL_TRANSCRIPT_BYTES {
            break;
        }
    }
    Ok(out)
}

#[allow(dead_code)]
fn extract_transcript_telemetry(text: &str) -> Result<TranscriptTelemetry> {
    let mut out = TranscriptTelemetry::default();
    for (idx, line) in text.lines().enumerate() {
        absorb_transcript_line(&mut out, line, idx + 1)?;
    }
    Ok(out)
}

fn transcript_telemetry_complete(out: &TranscriptTelemetry) -> bool {
    out.effective_model_id.is_some()
        && out.provider_id.is_some()
        && out.api_id.is_some()
        && out.tokens_in.is_some()
        && out.tokens_out.is_some()
        && out.cache_read_tokens.is_some()
        && out.cache_write_tokens.is_some()
        && out.cost_total.is_some()
}

fn absorb_transcript_line(
    out: &mut TranscriptTelemetry,
    line: &str,
    line_no: usize,
) -> Result<()> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    let v: serde_json::Value = serde_json::from_str(trimmed)
        .with_context(|| format!("parse transcript JSONL line {}", line_no))?;
    if v.get("type").and_then(|x| x.as_str()) == Some("stores_config") {
        out.configured_model_id = out.configured_model_id.clone().or_else(|| {
            v.get("configured_model")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        });
        out.configured_thinking_effort = out.configured_thinking_effort.clone().or_else(|| {
            v.get("configured_thinking")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        });
        out.effective_thinking_effort = out.effective_thinking_effort.clone().or_else(|| {
            v.get("effective_thinking")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty() && *s != "unknown")
                .map(str::to_string)
        });
        out.thinking_effort_source = out.thinking_effort_source.clone().or_else(|| {
            v.get("thinking_source")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty() && *s != "unknown")
                .map(str::to_string)
        });
    }
    let message = v.get("message");
    out.effective_model_id = out.effective_model_id.clone().or_else(|| {
        message
            .and_then(|m| m.get("model"))
            .or_else(|| v.get("model"))
            .and_then(value_string)
    });
    out.provider_id = out.provider_id.clone().or_else(|| {
        message
            .and_then(|m| m.get("provider"))
            .or_else(|| v.get("provider"))
            .and_then(value_string)
    });
    out.api_id = out.api_id.clone().or_else(|| {
        message
            .and_then(|m| m.get("api"))
            .or_else(|| v.get("api"))
            .and_then(value_string)
    });
    if let Some(u) = message
        .and_then(|m| m.get("usage"))
        .or_else(|| v.get("usage"))
    {
        out.tokens_in = out
            .tokens_in
            .or_else(|| usage_i64(u, &["input_tokens", "input", "prompt_tokens"]));
        out.tokens_out = out
            .tokens_out
            .or_else(|| usage_i64(u, &["output_tokens", "output", "completion_tokens"]));
        out.cache_read_tokens = out.cache_read_tokens.or_else(|| {
            usage_i64(
                u,
                &[
                    "prompt_cache_hits",
                    "cache_read_input_tokens",
                    "cacheRead",
                    "cache_read_tokens",
                ],
            )
        });
        out.cache_write_tokens = out.cache_write_tokens.or_else(|| {
            usage_i64(
                u,
                &[
                    "cache_creation_input_tokens",
                    "cache_write_input_tokens",
                    "cache_write_tokens",
                ],
            )
        });
        out.cost_total = out
            .cost_total
            .or_else(|| usage_f64(u, &["cost_total", "cost"]));
    }
    Ok(())
}

fn usage_i64(usage: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| usage.get(*key).and_then(|x| x.as_i64()))
}

fn usage_f64(usage: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| usage.get(*key).and_then(|x| x.as_f64()))
}

fn value_string(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    v.as_object().and_then(|obj| {
        ["id", "name", "provider", "api"]
            .iter()
            .find_map(|key| obj.get(*key).and_then(|x| x.as_str()).map(str::to_string))
    })
}

fn session_id_from_path(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn workspace_from_transcript_path(path: &Path) -> Option<String> {
    let runs = path.parent()?;
    if runs.file_name().and_then(|s| s.to_str()) != Some("runs") {
        return None;
    }
    let stores = runs.parent()?;
    if stores.file_name().and_then(|s| s.to_str()) != Some(".stores") {
        return None;
    }
    stores
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_agent_runs(conn: &Connection) {
        conn.execute_batch(
            "CREATE TABLE agent_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                display_id TEXT NOT NULL,
                phase INTEGER NOT NULL,
                cycle INTEGER NOT NULL,
                role TEXT NOT NULL,
                model_id TEXT NOT NULL,
                harness_id TEXT NOT NULL,
                started_at TEXT NOT NULL,
                ended_at TEXT NOT NULL,
                exit_code INTEGER NOT NULL,
                tokens_in INTEGER,
                tokens_out INTEGER,
                prompt_cache_hits INTEGER,
                transcript_path TEXT NOT NULL,
                brief_text TEXT,
                configured_harness_id TEXT,
                configured_model_id TEXT,
                configured_thinking_effort TEXT,
                effective_model_id TEXT,
                effective_thinking_effort TEXT,
                thinking_effort_source TEXT,
                provider_id TEXT,
                api_id TEXT,
                session_id TEXT,
                workspace_path TEXT,
                runner_exit_kind TEXT,
                payload_valid INTEGER,
                payload_error TEXT,
                cache_read_tokens INTEGER,
                cache_write_tokens INTEGER,
                cost_total REAL
            );",
        )
        .unwrap();
    }

    fn insert_run(conn: &Connection, transcript_path: &str) {
        conn.execute(
            "INSERT INTO agent_runs (display_id, phase, cycle, role, model_id, harness_id, started_at, ended_at, exit_code, transcript_path)
             VALUES ('T001', 1, 1, 'executor', 'pi:default', 'pi', 's', 'e', 0, ?1)",
            [transcript_path],
        )
        .unwrap();
    }

    #[test]
    fn backfills_missing_pi_telemetry_from_nested_transcript_and_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().join("workspace");
        let runs = workspace.join(".stores/runs");
        std::fs::create_dir_all(&runs).unwrap();
        let transcript = runs.join("session-1.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"stores_config","configured_model":"gpt-5.5","configured_thinking":"high"}
{"type":"assistant","message":{"model":"gpt-5.5","provider":{"id":"openai-codex"},"api":{"id":"openai-codex-responses"},"usage":{"input_tokens":11,"output_tokens":7,"cache_read_input_tokens":3,"cache_creation_input_tokens":2,"cost_total":0.125}}}
"#,
        )
        .unwrap();
        let conn = Connection::open_in_memory().unwrap();
        create_agent_runs(&conn);
        insert_run(&conn, &transcript.to_string_lossy());

        let counts = backfill_pi_agent_run_telemetry(&conn).unwrap();
        assert_eq!(
            counts,
            BackfillCounts {
                scanned: 1,
                updated: 1,
                skipped: 0,
                parse_failed: 0
            }
        );
        let row: (String, String, String, i64, i64, i64, i64, i64, f64, String, String, String, String) = conn
            .query_row(
                "SELECT effective_model_id, provider_id, api_id, tokens_in, tokens_out, prompt_cache_hits, cache_read_tokens, cache_write_tokens, cost_total, configured_harness_id, configured_model_id, effective_thinking_effort, thinking_effort_source FROM agent_runs",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?, r.get(11)?, r.get(12)?)),
            )
            .unwrap();
        assert_eq!(row.0, "gpt-5.5");
        assert_eq!(row.1, "openai-codex");
        assert_eq!(row.2, "openai-codex-responses");
        assert_eq!(row.3, 11);
        assert_eq!(row.4, 7);
        assert_eq!(row.5, 3);
        assert_eq!(row.6, 3);
        assert_eq!(row.7, 2);
        assert_eq!(row.8, 0.125);
        assert_eq!(row.9, "pi");
        assert_eq!(row.10, "gpt-5.5");
        assert_eq!(row.11, "unknown");
        assert_eq!(row.12, "unknown");

        let second = backfill_pi_agent_run_telemetry(&conn).unwrap();
        assert_eq!(second.scanned, 1);
        assert_eq!(second.updated, 0);
        assert_eq!(second.skipped, 1);
        assert_eq!(second.parse_failed, 0);
    }

    #[test]
    fn stops_after_complete_telemetry_without_reading_trailing_noise() {
        let tmp = TempDir::new().unwrap();
        let transcript = tmp.path().join("complete-then-noise.jsonl");
        std::fs::write(
            &transcript,
            r#"{"type":"assistant","message":{"model":"gpt-5.5","provider":{"id":"openai-codex"},"api":{"id":"openai-codex-responses"},"usage":{"input_tokens":11,"output_tokens":7,"cache_read_input_tokens":3,"cache_creation_input_tokens":2,"cost_total":0.125}}}
not json
"#,
        )
        .unwrap();
        let extracted = extract_transcript_telemetry_from_path(&transcript).unwrap();
        assert_eq!(extracted.effective_model_id.as_deref(), Some("gpt-5.5"));
        assert_eq!(extracted.provider_id.as_deref(), Some("openai-codex"));
        assert_eq!(extracted.api_id.as_deref(), Some("openai-codex-responses"));
        assert_eq!(extracted.cost_total, Some(0.125));
    }

    #[test]
    fn does_not_overwrite_existing_values_and_reports_parse_failures() {
        let tmp = TempDir::new().unwrap();
        let transcript = tmp.path().join("bad.jsonl");
        std::fs::write(&transcript, "not json\n").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        create_agent_runs(&conn);
        conn.execute(
            "INSERT INTO agent_runs (display_id, phase, cycle, role, model_id, harness_id, started_at, ended_at, exit_code, transcript_path, effective_model_id)
             VALUES ('T001', 1, 1, 'executor', 'pi:default', 'pi', 's', 'e', 0, ?1, 'kept')",
            [transcript.to_string_lossy().to_string()],
        )
        .unwrap();

        let counts = backfill_pi_agent_run_telemetry(&conn).unwrap();
        assert_eq!(counts.scanned, 1);
        assert_eq!(counts.updated, 0);
        assert_eq!(counts.skipped, 0);
        assert_eq!(counts.parse_failed, 1);
        let model: String = conn
            .query_row("SELECT effective_model_id FROM agent_runs", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(model, "kept");
    }
}
