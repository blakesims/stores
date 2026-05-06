use anyhow::{bail, Result};
use clap::ArgMatches;
use std::collections::HashMap;

use crate::cli::auth;
use crate::db;
use crate::handlers;
use crate::manifest::Manifest;
use crate::paths::db_path;
use crate::schema::{
    actor::{Actor, InvokerCtx},
    Schema,
};

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
                    let display_id = sub
                        .get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let plan_json = read_plan_json(sub)?;
                    handlers::submit::run_submit_plan(
                        schema, &conn, display_id, plan_json, invoker,
                    )?;
                }
                Some(("submit-plan-review", sub)) => {
                    let display_id = sub
                        .get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let gate = sub
                        .get_one::<String>("gate")
                        .ok_or_else(|| anyhow::anyhow!("submit-plan-review requires --gate"))?
                        .as_str();
                    let summary =
                        read_text_or_file(sub, "summary", "summary-from-file").unwrap_or_default();
                    // P5-m2: --open-questions-from-file reads a file with one question per line
                    let open_questions = read_lines_from_file(sub, "open-questions-from-file");
                    handlers::submit::run_submit_plan_review(
                        schema,
                        &conn,
                        display_id,
                        gate,
                        &summary,
                        open_questions,
                        invoker,
                    )?;
                }
                Some(("submit-execute", sub)) => {
                    let display_id = sub
                        .get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let exec_summary =
                        read_text_or_file(sub, "summary", "summary-from-file").unwrap_or_default();
                    let commit_sha = sub.get_one::<String>("commit").map(|s| s.as_str());
                    let files_changed = sub.get_one::<String>("files-changed").map(|s| s.as_str());
                    let notes = read_notes_from_file(sub);
                    handlers::submit::run_submit_execute(
                        schema,
                        &conn,
                        display_id,
                        &exec_summary,
                        commit_sha,
                        files_changed,
                        notes.as_deref(),
                        invoker,
                    )?;
                }
                Some(("submit-review", sub)) => {
                    let display_id = sub
                        .get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let gate = sub
                        .get_one::<String>("gate")
                        .ok_or_else(|| anyhow::anyhow!("submit-review requires --gate"))?
                        .as_str();
                    // P5-m4: --summary is a string; --details-from-file reads file content into details sub-field
                    let summary = sub
                        .get_one::<String>("summary")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let details = read_file_content(sub, "details-from-file");
                    let critical = sub.get_one::<i64>("critical").copied().unwrap_or(0);
                    let major = sub.get_one::<i64>("major").copied().unwrap_or(0);
                    let minor = sub.get_one::<i64>("minor").copied().unwrap_or(0);
                    handlers::submit::run_submit_review(
                        schema,
                        &conn,
                        display_id,
                        gate,
                        summary,
                        details.as_deref(),
                        critical,
                        major,
                        minor,
                        invoker,
                    )?;
                }
                Some(("submit-wrap", sub)) => {
                    let display_id = sub
                        .get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    let executive_summary =
                        read_file_content(sub, "summary-from-file").unwrap_or_default();
                    let deviations =
                        read_lines_from_file(sub, "deviations-from-file").unwrap_or_default();
                    let residual_risks =
                        read_lines_from_file(sub, "residual-risks-from-file").unwrap_or_default();
                    let sanity_checks =
                        read_lines_from_file(sub, "sanity-checks-from-file").unwrap_or_default();
                    // `reasoning-from-file` is accepted at the CLI but not persisted;
                    // it is consumed here for completeness and discarded (not in schema).
                    let _reasoning = read_file_content(sub, "reasoning-from-file");

                    let mut obj = serde_json::Map::new();
                    obj.insert(
                        "executive_summary".to_string(),
                        serde_json::Value::String(executive_summary),
                    );
                    obj.insert(
                        "deviations".to_string(),
                        serde_json::Value::Array(
                            deviations
                                .into_iter()
                                .map(serde_json::Value::String)
                                .collect(),
                        ),
                    );
                    obj.insert(
                        "residual_risks".to_string(),
                        serde_json::Value::Array(
                            residual_risks
                                .into_iter()
                                .map(serde_json::Value::String)
                                .collect(),
                        ),
                    );
                    obj.insert(
                        "recommended_sanity_checks".to_string(),
                        serde_json::Value::Array(
                            sanity_checks
                                .into_iter()
                                .map(serde_json::Value::String)
                                .collect(),
                        ),
                    );
                    let wrap_entry = serde_json::Value::Object(obj);

                    handlers::submit::run_submit_wrap(
                        schema, &conn, display_id, wrap_entry, invoker,
                    )?;
                }
                Some(("resume", sub)) if schema.workflow.is_some() => {
                    let display_id = sub
                        .get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    handlers::submit::run_resume(schema, &conn, display_id, invoker)?;
                }
                Some(("retry-deploy", sub)) if schema.workflow.is_some() => {
                    let display_id = sub
                        .get_one::<String>("display_id")
                        .map(|s| s.as_str())
                        .unwrap_or("");
                    handlers::submit::run_retry_deploy(schema, &conn, display_id, invoker)?;
                }
                Some(("drive", sub)) => {
                    let display_id = sub.get_one::<String>("display_id").cloned();
                    let auto = sub.get_flag("auto");
                    let max_iters = sub.get_one::<usize>("max-iters").copied().unwrap_or(50);
                    let mock_fixture = sub.get_one::<String>("mock").map(std::path::PathBuf::from);
                    #[cfg(feature = "runner-claude-code")]
                    let claude_code = sub.get_flag("claude-code");
                    #[cfg(feature = "runner-claude-code")]
                    let testing = sub.get_flag("testing");
                    #[cfg(feature = "runner-claude-code")]
                    let claude_code_model = sub.get_one::<String>("claude-code-model").cloned();
                    #[cfg(feature = "runner-pi")]
                    let pi = sub.get_flag("pi");
                    let args = handlers::drive::DriveArgs {
                        display_id,
                        auto,
                        mock_fixture,
                        #[cfg(feature = "runner-claude-code")]
                        claude_code,
                        #[cfg(feature = "runner-claude-code")]
                        testing,
                        #[cfg(feature = "runner-claude-code")]
                        claude_code_model,
                        #[cfg(feature = "runner-pi")]
                        pi,
                        max_iters,
                    };
                    handlers::drive::run_drive(schema, args)?;
                }
                Some(("guide", sub)) => {
                    let display_id = sub
                        .get_one::<String>("display_id")
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("guide requires a display_id"))?;
                    let mock_fixture = sub.get_one::<String>("mock").map(std::path::PathBuf::from);
                    #[cfg(feature = "runner-claude-code")]
                    let claude_code = sub.get_flag("claude-code");

                    if store.name == "gate" {
                        let args = handlers::guide::GateGuideArgs {
                            display_id,
                            mock_fixture,
                            #[cfg(feature = "runner-claude-code")]
                            claude_code,
                        };
                        handlers::guide::run_gate_guide(schema, args)?;
                    } else {
                        // tasks (stub form)
                        let args = handlers::guide::TasksGuideArgs {
                            display_id,
                            mock_fixture,
                            #[cfg(feature = "runner-claude-code")]
                            claude_code,
                        };
                        handlers::guide::run_tasks_guide(schema, args)?;
                    }
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
                Some(("next-id", _sub)) => {
                    handlers::next_id::run_next_id()?;
                }
                Some((verb, sub)) => {
                    // override-risk and override-policy are observations-only non-transition verbs
                    if verb == "override-risk" {
                        handlers::overrides::run_override_risk(schema, &conn, sub, invoker)?;
                    } else if verb == "override-policy" {
                        handlers::overrides::run_override_policy(schema, &conn, sub, invoker)?;
                    } else if schema.lifecycle.transitions.iter().any(|t| t.verb == verb) {
                        if verb == "reject" {
                            // reject requires --reason (clap enforces required=true at parse time).
                            let reason = sub
                                .get_one::<String>("reason")
                                .ok_or_else(|| anyhow::anyhow!("reject requires --reason"))?;
                            handlers::transition::run_reject(schema, &conn, sub, invoker, reason)?;
                        } else if verb == "close-out-of-band" {
                            let commit = sub.get_one::<String>("commit").ok_or_else(|| {
                                anyhow::anyhow!("close-out-of-band requires --commit")
                            })?;
                            handlers::transition::run_close_out_of_band(
                                schema, &conn, sub, invoker, commit,
                            )?;
                        } else if verb == "close_as_addressed" {
                            // close_as_addressed requires --resolution; clap enforces required=true.
                            let resolution =
                                sub.get_one::<String>("resolution").ok_or_else(|| {
                                    anyhow::anyhow!("close_as_addressed requires --resolution")
                                })?;
                            handlers::transition::run_close_as_addressed(
                                schema, &conn, sub, invoker, resolution,
                            )?;
                        } else {
                            handlers::transition::run(schema, &conn, sub, invoker, verb)?;
                        }
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
    if lines.is_empty() {
        None
    } else {
        Some(lines)
    }
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
fn detect_actor(matches: &ArgMatches) -> Result<Actor> {
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
    // Auto-detect from environment — delegate to Actor::from_env so the two
    // code paths cannot drift (e.g. on whether CLAUDECODE="" counts as AI).
    Ok(Actor::from_env())
}

/// Phase 2: resolve actor + validate `--approve-token`.
///
/// On `--approve-token <T>` supplied, verify the plaintext against the on-disk
/// hash via constant-time compare. If the token is supplied but does not
/// verify, exit non-zero with a clear error (do NOT silently drop). If the
/// flag is absent, `token_valid` is `false` and behaviour is unchanged.
fn detect_invoker(matches: &ArgMatches) -> Result<InvokerCtx> {
    let actor = detect_actor(matches)?;
    let token_valid = match matches.get_one::<String>("approve-token") {
        Some(tok) => {
            if !auth::verify_approve_token(tok) {
                bail!(
                    "invalid approval token: --approve-token did not match the \
                     stored hash at ${{STORES_TOKEN_DIR:-~/.config/stores}}/approve.token.hash. \
                     Run `stores auth show` to obtain a valid token, or `stores auth init` \
                     if no token has been provisioned."
                );
            }
            true
        }
        None => false,
    };
    Ok(InvokerCtx { actor, token_valid })
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
        let err = detect_actor(&m).unwrap_err();
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
        let err = detect_actor(&m).unwrap_err();
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
        let err = detect_actor(&m).unwrap_err();
        assert!(
            err.to_string().contains("unknown --invoker value"),
            "error should reject empty string: {err}"
        );
    }

    #[test]
    fn invoker_flag_accepts_human() {
        let m = matches_with_invoker(Some("human"));
        assert_eq!(detect_actor(&m).unwrap(), Actor::Human);
    }

    #[test]
    fn invoker_flag_falls_back_to_env_when_absent() {
        let m = matches_with_invoker(None);
        // env detection always returns Ok — we just check it doesn't error
        assert!(detect_actor(&m).is_ok());
    }

    // ---- Phase 2: --approve-token plumbing ----

    use crate::cli::test_support::ENV_LOCK;
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn matches_with_token(invoker: Option<&str>, token: Option<&str>) -> ArgMatches {
        let cmd = Command::new("test")
            .arg(Arg::new("invoker").long("invoker").required(false))
            .arg(
                Arg::new("approve-token")
                    .long("approve-token")
                    .required(false),
            );
        let mut argv: Vec<&str> = vec!["test"];
        if let Some(v) = invoker {
            argv.push("--invoker");
            argv.push(v);
        }
        if let Some(t) = token {
            argv.push("--approve-token");
            argv.push(t);
        }
        cmd.get_matches_from(argv)
    }

    fn write_hash_for(token_dir: &std::path::Path, plaintext: &str) {
        let mut h = Sha256::new();
        h.update(plaintext.as_bytes());
        std::fs::write(
            token_dir.join("approve.token.hash"),
            hex::encode(h.finalize()),
        )
        .unwrap();
    }

    fn unique_token_dir(tag: &str) -> std::path::PathBuf {
        let ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "stores-dispatch-{}-{}-{}",
            tag,
            std::process::id(),
            ns
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// AC2.5: `--approve-token` absent → token_valid=false; behaviour identical to today.
    #[test]
    fn approve_token_absent_yields_token_valid_false() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let m = matches_with_token(Some("human"), None);
        let ctx = detect_invoker(&m).unwrap();
        assert_eq!(ctx.actor, Actor::Human);
        assert!(!ctx.token_valid);
    }

    /// AC2.2: valid token → token_valid=true, no error from the token check.
    #[test]
    fn approve_token_valid_yields_token_valid_true() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tdir = unique_token_dir("valid");
        let prev = std::env::var("STORES_TOKEN_DIR").ok();
        std::env::set_var("STORES_TOKEN_DIR", &tdir);

        let plaintext = "the-real-token-xyz";
        write_hash_for(&tdir, plaintext);

        let m = matches_with_token(Some("ai_with_human"), Some(plaintext));
        let ctx = detect_invoker(&m).unwrap();
        assert_eq!(ctx.actor, Actor::AiWithHuman);
        assert!(ctx.token_valid);

        match prev {
            Some(v) => std::env::set_var("STORES_TOKEN_DIR", v),
            None => std::env::remove_var("STORES_TOKEN_DIR"),
        }
    }

    /// AC2.1: invalid token → bail with `invalid approval token` in the error.
    #[test]
    fn approve_token_invalid_bails_with_clear_error() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tdir = unique_token_dir("invalid");
        let prev = std::env::var("STORES_TOKEN_DIR").ok();
        std::env::set_var("STORES_TOKEN_DIR", &tdir);

        write_hash_for(&tdir, "real-token");

        let m = matches_with_token(Some("ai_with_human"), Some("wrong-token"));
        let err = detect_invoker(&m).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid approval token"),
            "error must contain 'invalid approval token'; got: {msg}"
        );

        match prev {
            Some(v) => std::env::set_var("STORES_TOKEN_DIR", v),
            None => std::env::remove_var("STORES_TOKEN_DIR"),
        }
    }
}
