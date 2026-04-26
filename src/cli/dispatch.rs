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
