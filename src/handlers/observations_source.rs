use anyhow::{bail, Result};
use clap::ArgMatches;
use serde_json::Value;

use crate::validate::EntryMap;

const CANONICAL_HELP: &str = "Use canonical --source-env <prod|sandbox> with --source-id <ID>.";

fn arg_value(matches: &ArgMatches, entry: &EntryMap, name: &str) -> Option<String> {
    matches
        .try_get_one::<String>(name)
        .ok()
        .flatten()
        .cloned()
        .or_else(|| {
            let field_name = name.replace('-', "_");
            entry.get(&field_name).and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            })
        })
}

pub fn normalize_cli_source_tuple(matches: &ArgMatches, entry: &mut EntryMap) -> Result<()> {
    let source_env = arg_value(matches, entry, "source-env");
    let source_id = arg_value(matches, entry, "source-id");
    let prod_source_id = arg_value(matches, entry, "prod-source-id");
    let sandbox_source_id = arg_value(matches, entry, "sandbox-source-id");
    let origin_db = arg_value(matches, entry, "origin-db");

    let canonical_present = source_env.is_some() || source_id.is_some();
    let legacy_present = prod_source_id.is_some() || sandbox_source_id.is_some() || origin_db.is_some();
    for legacy_col in ["prod_source_id", "sandbox_source_id", "origin_db"] {
        entry.remove(legacy_col);
    }

    if canonical_present && legacy_present {
        bail!(
            "conflicting observations source flags: deprecated --prod-source-id/--sandbox-source-id/--origin-db cannot be combined with canonical --source-env/--source-id. {CANONICAL_HELP}"
        );
    }

    if prod_source_id.is_some() && sandbox_source_id.is_some() {
        bail!(
            "ambiguous observations source aliases: --prod-source-id and --sandbox-source-id both supplied. {CANONICAL_HELP}"
        );
    }

    if canonical_present {
        if let Some(v) = source_env {
            entry.insert("source_env".to_string(), Value::String(v));
        }
        if let Some(v) = source_id {
            entry.insert("source_id".to_string(), Value::String(v));
        }
        return Ok(());
    }

    let mut mapped_env: Option<&str> = None;
    let mut mapped_id: Option<String> = None;
    if let Some(v) = prod_source_id {
        mapped_env = Some("prod");
        mapped_id = Some(v);
    }
    if let Some(v) = sandbox_source_id {
        mapped_env = Some("sandbox");
        mapped_id = Some(v);
    }
    if let Some(env) = origin_db.as_deref() {
        if !matches!(env, "prod" | "sandbox") {
            bail!("invalid --origin-db value '{env}'; expected prod or sandbox. {CANONICAL_HELP}");
        }
        if let Some(existing_env) = mapped_env {
            if existing_env != env {
                bail!(
                    "conflicting observations source aliases: --origin-db {env} does not match legacy id environment {existing_env}. {CANONICAL_HELP}"
                );
            }
        } else {
            mapped_env = Some(env);
        }
    }

    if legacy_present {
        eprintln!(
            "warning: --prod-source-id/--sandbox-source-id/--origin-db are deprecated observations source aliases; use --source-env/--source-id"
        );
    }
    if let Some(env) = mapped_env {
        entry.insert("source_env".to_string(), Value::String(env.to_string()));
    }
    if let Some(id) = mapped_id {
        entry.insert("source_id".to_string(), Value::String(id));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> clap::Command {
        clap::Command::new("x")
            .arg(clap::Arg::new("source-env").long("source-env"))
            .arg(clap::Arg::new("source-id").long("source-id"))
            .arg(clap::Arg::new("prod-source-id").long("prod-source-id"))
            .arg(clap::Arg::new("sandbox-source-id").long("sandbox-source-id"))
            .arg(clap::Arg::new("origin-db").long("origin-db"))
    }

    #[test]
    fn legacy_prod_maps_to_canonical_tuple() {
        let m = cmd().get_matches_from(["x", "--prod-source-id", "P123"]);
        let mut entry = EntryMap::new();
        normalize_cli_source_tuple(&m, &mut entry).unwrap();
        assert_eq!(entry.get("source_env"), Some(&Value::String("prod".into())));
        assert_eq!(entry.get("source_id"), Some(&Value::String("P123".into())));
    }

    #[test]
    fn canonical_and_legacy_conflict() {
        let m = cmd().get_matches_from(["x", "--source-env", "prod", "--prod-source-id", "P123"]);
        let mut entry = EntryMap::new();
        let msg = normalize_cli_source_tuple(&m, &mut entry).unwrap_err().to_string();
        assert!(msg.contains("--source-env/--source-id"), "{msg}");
    }

    #[test]
    fn prod_and_sandbox_legacy_conflict() {
        let m = cmd().get_matches_from([
            "x",
            "--prod-source-id",
            "P123",
            "--sandbox-source-id",
            "S456",
        ]);
        let mut entry = EntryMap::new();
        let msg = normalize_cli_source_tuple(&m, &mut entry).unwrap_err().to_string();
        assert!(msg.contains("ambiguous"), "{msg}");
    }
}
