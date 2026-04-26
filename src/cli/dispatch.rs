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
    let invoker = detect_invoker(matches);

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
fn detect_invoker(matches: &ArgMatches) -> Actor {
    // --invoker explicit override
    if let Some(inv) = matches.get_one::<String>("invoker") {
        match inv.as_str() {
            "human" => return Actor::Human,
            "ai_autonomous" => return Actor::AiAutonomous,
            "ai_with_human" => return Actor::AiWithHuman,
            _ => {} // fall through to env detection
        }
    }
    // Auto-detect from environment
    if std::env::var("CLAUDECODE").is_ok() {
        Actor::AiAutonomous
    } else {
        Actor::Human
    }
}
