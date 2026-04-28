use anyhow::{bail, Result};
use clap::ArgMatches;
use std::collections::HashMap;

use crate::db;
use crate::handlers;
use crate::manifest::Manifest;
use crate::paths::db_path;
use crate::schema::{actor::Actor, Schema};

/// Route parsed ArgMatches to the right handler.
pub fn dispatch(
    matches: &ArgMatches,
    manifest: &Manifest,
    schemas: &HashMap<String, Schema>,
) -> Result<()> {
    // init and install are handled before dispatch; only store subcommands reach here
    let invoker = detect_invoker(matches)?;

    // Find which store subcommand was invoked
    for store in &manifest.stores {
        if let Some(store_matches) = matches.subcommand_matches(&store.name) {
            let schema = schemas
                .get(&store.name)
                .ok_or_else(|| anyhow::anyhow!("schema for '{}' not loaded", store.name))?;

            let db = db_path()?;
            let conn = db::open(&db)?;

            match store_matches.subcommand() {
                Some(("add", sub)) => {
                    handlers::add::run(schema, &conn, sub, invoker)?;
                }
                Some(("show", sub)) => {
                    handlers::show::run(schema, &conn, sub, invoker)?;
                }
                Some(("list", sub)) => {
                    handlers::list::run(schema, &conn, sub, invoker)?;
                }
                Some(("update", sub)) => {
                    handlers::update::run(schema, &conn, sub, invoker)?;
                }
                Some(("schema", sub)) => {
                    handlers::schema_show::run(schema, sub)?;
                }
                Some(("next-action", sub)) => {
                    handlers::next_action::run(schema, &conn, sub, invoker)?;
                }
                Some(("brief", sub)) => {
                    handlers::brief::run(schema, &conn, sub, invoker)?;
                }
                Some(("render", sub)) => {
                    handlers::render::run(schema, &conn, sub, invoker)?;
                }
                Some(("submit-plan", sub)) => {
                    let display_id = sub.get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let plan_json = read_plan_json(sub)?;
                    handlers::submit::run_submit_plan(schema, &conn, display_id, plan_json, invoker)?;
                }
                Some(("submit-plan-review", sub)) => {
                    let display_id = sub.get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let gate = sub.get_one::<String>("gate")
                        .ok_or_else(|| anyhow::anyhow!("submit-plan-review requires --gate"))?
                        .as_str();
                    let summary = read_text_or_file(sub, "summary", "summary-from-file")
                        .unwrap_or_default();
                    // P5-m2: --open-questions-from-file reads a file with one question per line
                    let open_questions = read_lines_from_file(sub, "open-questions-from-file");
                    handlers::submit::run_submit_plan_review(
                        schema, &conn, display_id, gate, &summary, open_questions, invoker,
                    )?;
                }
                Some(("submit-execute", sub)) => {
                    let display_id = sub.get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let exec_summary = read_text_or_file(sub, "summary", "summary-from-file")
                        .unwrap_or_default();
                    let commit_sha = sub.get_one::<String>("commit").map(|s| s.as_str());
                    let files_changed = sub.get_one::<String>("files-changed").map(|s| s.as_str());
                    let notes = read_notes_from_file(sub);
                    handlers::submit::run_submit_execute(
                        schema, &conn, display_id,
                        &exec_summary, commit_sha, files_changed,
                        notes.as_deref(),
                        invoker,
                    )?;
                }
                Some(("submit-review", sub)) => {
                    let display_id = sub.get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let gate = sub.get_one::<String>("gate")
                        .ok_or_else(|| anyhow::anyhow!("submit-review requires --gate"))?
                        .as_str();
                    // P5-m4: --summary is a string; --details-from-file reads file content into details sub-field
                    let summary = sub.get_one::<String>("summary")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let details = read_file_content(sub, "details-from-file");
                    let critical = sub.get_one::<i64>("critical").copied().unwrap_or(0);
                    let major = sub.get_one::<i64>("major").copied().unwrap_or(0);
                    let minor = sub.get_one::<i64>("minor").copied().unwrap_or(0);
                    handlers::submit::run_submit_review(
                        schema, &conn, display_id,
                        gate, summary, details.as_deref(), critical, major, minor,
                        invoker,
                    )?;
                }
                Some(("resume", sub)) => {
                    let display_id = sub.get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    handlers::submit::run_resume(schema, &conn, display_id, invoker)?;
                }
                Some(("drive", sub)) => {
                    let display_id = sub.get_one::<String>("display_id").cloned();
                    let auto = sub.get_flag("auto");
                    let max_iters = sub.get_one::<usize>("max-iters").copied().unwrap_or(50);
                    let mock_fixture = sub.get_one::<String>("mock").map(std::path::PathBuf::from);
                    #[cfg(feature = "runner-claude-code")]
                    let claude_code = sub.get_flag("claude-code");
                    let args = handlers::drive::DriveArgs {
                        display_id,
                        auto,
                        mock_fixture,
                        #[cfg(feature = "runner-claude-code")]
                        claude_code,
                        max_iters,
                    };
                    handlers::drive::run_drive(schema, args)?;
                }
                Some(("status", sub)) => {
                    let display_id = sub.get_one::<String>("display_id").cloned();
                    let follow = sub.get_flag("follow");
                    let interval_secs = sub.get_one::<f64>("interval").copied().unwrap_or(1.5);
                    let interval_ms = (interval_secs * 1000.0) as u64;
                    let max_iters = sub
                        .get_one::<usize>("max-iters")
                        .copied()
                        .unwrap_or(usize::MAX);
                    let args = handlers::status::StatusArgs {
                        display_id,
                        follow,
                        interval_ms,
                        max_iters,
                    };
                    handlers::status::run_status(args)?;
                }
                Some((verb, sub)) => {
                    // Check if this is a declared lifecycle transition verb
                    if schema.lifecycle.transitions.iter().any(|t| t.verb == verb) {
                        handlers::transition::run(schema, &conn, sub, invoker, verb)?;
                    } else {
                        bail!("unknown verb '{}' for store '{}'", verb, store.name);
                    }
                }
                None => {
                    // No verb: print store help
                    let _ = store_matches;
                    bail!("no verb given for store '{}'; try --help", store.name);
                }
            }
            return Ok(());
        }
    }

    bail!("no store subcommand matched")
}

/// Read a text value from either a direct arg or a "-from-file" companion.
fn read_text_or_file(sub: &ArgMatches, direct: &str, from_file: &str) -> Option<String> {
    // Check the direct arg first
    if let Some(v) = sub.get_one::<String>(direct) {
        return Some(v.clone());
    }
    // Then check the from-file companion
    if let Some(path) = sub.get_one::<String>(from_file) {
        if path == "-" {
            use std::io::Read;
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).ok();
            return Some(s.trim_end_matches('\n').to_string());
        }
        return std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim_end_matches('\n').to_string());
    }
    None
}

/// Read plan JSON from --plan-from-file.
fn read_plan_json(sub: &ArgMatches) -> anyhow::Result<serde_json::Value> {
    let path = sub
        .get_one::<String>("plan-from-file")
        .ok_or_else(|| anyhow::anyhow!("submit-plan requires --plan-from-file"))?;
    let text = if path == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        s
    } else {
        std::fs::read_to_string(path)?
    };
    serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("plan JSON parse error: {e}"))
}

/// Read a file's content from a named from-file arg (if present).
/// Returns None if the arg is absent.
fn read_file_content(sub: &ArgMatches, from_file: &str) -> Option<String> {
    let path = sub.get_one::<String>(from_file)?;
    if path == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).ok();
        Some(s.trim_end_matches('\n').to_string())
    } else {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim_end_matches('\n').to_string())
    }
}

/// Read lines from a file arg. Returns None if absent; Some(vec) with one item per non-empty line.
/// P5-m2: used by --open-questions-from-file.
fn read_lines_from_file(sub: &ArgMatches, from_file: &str) -> Option<Vec<String>> {
    let content = read_file_content(sub, from_file)?;
    let lines: Vec<String> = content
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() { None } else { Some(lines) }
}

/// Read notes from --notes-from-file (if present).
fn read_notes_from_file(sub: &ArgMatches) -> Option<String> {
    let path = sub.get_one::<String>("notes-from-file")?;
    if path == "-" {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).ok();
        Some(s.trim_end_matches('\n').to_string())
    } else {
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim_end_matches('\n').to_string())
    }
}

/// Detect invoker: check --invoker flag first, then fall back to $CLAUDECODE env var.
///
/// Returns `Err` if `--invoker` was explicitly supplied with an unrecognised value (including
/// the empty string).  If the flag is absent, env-detection runs and always succeeds.
fn detect_invoker(matches: &ArgMatches) -> Result<Actor> {
    // --invoker explicit override
    if let Some(inv) = matches.get_one::<String>("invoker") {
        return match inv.as_str() {
            "human" => Ok(Actor::Human),
            "ai_autonomous" => Ok(Actor::AiAutonomous),
            "ai_with_human" => Ok(Actor::AiWithHuman),
            "framework" => bail!(
                "'framework' is an internal actor used only by the workflow engine; \
                it cannot be passed via --invoker"
            ),
            other => bail!(
                "unknown --invoker value '{}'; valid: human, ai_autonomous, ai_with_human",
                other
            ),
        };
    }
    // Auto-detect from environment
    if std::env::var("CLAUDECODE").is_ok() {
        Ok(Actor::AiAutonomous)
    } else {
        Ok(Actor::Human)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{Arg, Command};

    /// Build a minimal ArgMatches with an optional --invoker value.
    fn matches_with_invoker(value: Option<&str>) -> ArgMatches {
        let cmd = Command::new("test").arg(
            Arg::new("invoker")
                .long("invoker")
                .value_name("INVOKER")
                .required(false),
        );
        match value {
            Some(v) => cmd.get_matches_from(["test", "--invoker", v]),
            None => cmd.get_matches_from(["test"]),
        }
    }

    #[test]
    fn invoker_flag_rejects_framework() {
        let m = matches_with_invoker(Some("framework"));
        let err = detect_invoker(&m).unwrap_err();
        assert!(
            err.to_string().contains("internal actor"),
            "error should cite 'internal actor': {err}"
        );
        assert!(
            err.to_string().contains("framework"),
            "error should name 'framework': {err}"
        );
    }

    #[test]
    fn invoker_flag_rejects_unknown_value() {
        let m = matches_with_invoker(Some("zorblax"));
        let err = detect_invoker(&m).unwrap_err();
        assert!(
            err.to_string().contains("zorblax"),
            "error should name the bad value: {err}"
        );
        assert!(
            err.to_string().contains("unknown --invoker value"),
            "error should state the flag: {err}"
        );
    }

    #[test]
    fn invoker_flag_rejects_empty_string() {
        let m = matches_with_invoker(Some(""));
        let err = detect_invoker(&m).unwrap_err();
        assert!(
            err.to_string().contains("unknown --invoker value"),
            "error should reject empty string: {err}"
        );
    }

    #[test]
    fn invoker_flag_accepts_human() {
        let m = matches_with_invoker(Some("human"));
        assert_eq!(detect_invoker(&m).unwrap(), Actor::Human);
    }

    #[test]
    fn invoker_flag_falls_back_to_env_when_absent() {
        let m = matches_with_invoker(None);
        // env detection always returns Ok — we just check it doesn't error
        assert!(detect_invoker(&m).is_ok());
    }
}
