mod cli;
mod codegen;
mod db;
pub mod handlers;
pub mod id_format;
mod install;
mod manifest;
mod output;
mod paths;
pub mod schema;
pub mod validate;

use anyhow::Result;
use std::collections::HashMap;

use manifest::Manifest;
use schema::Schema;

fn main() -> Result<()> {
    // Determine whether a manifest exists (init must work without one)
    let manifest_exists = paths::manifest_path()
        .map(|p| p.exists())
        .unwrap_or(false);

    // Load manifest + schemas if available
    let (manifest, schemas) = if manifest_exists {
        let m = Manifest::load()?;
        let mut s: HashMap<String, Schema> = HashMap::new();
        for store in &m.stores {
            let schema_file = store.schema_path.join("schema.yaml");
            let yaml = std::fs::read_to_string(&schema_file)?;
            let schema = Schema::from_yaml(&yaml)?;
            s.insert(store.name.clone(), schema);
        }
        (m, s)
    } else {
        (Manifest::empty(), HashMap::new())
    };

    // Build the command tree dynamically
    let cmd = cli::dynamic::build_root(&manifest, &schemas);
    let matches = cmd.get_matches();

    match matches.subcommand() {
        Some(("init", _)) => {
            cli::init::run()?;
        }
        Some(("install", sub)) => {
            let path = sub.get_one::<String>("path").unwrap();
            install::run(std::path::Path::new(path))?;
        }
        Some(("skills", sub)) => {
            use cli::skills::{SkillsCmd, run as skills_run};
            let cmd = match sub.subcommand() {
                Some(("list", _)) => SkillsCmd::List,
                Some(("install", isub)) => SkillsCmd::Install {
                    name: isub.get_one::<String>("name").cloned(),
                    all: *isub.get_one::<bool>("all").unwrap_or(&false),
                    global: *isub.get_one::<bool>("global").unwrap_or(&false),
                },
                Some(("uninstall", usub)) => SkillsCmd::Uninstall {
                    name: usub.get_one::<String>("name").unwrap().clone(),
                    global: *usub.get_one::<bool>("global").unwrap_or(&false),
                },
                _ => {
                    // Print skills help
                    let mut cmd2 = cli::dynamic::build_root(&manifest, &schemas);
                    if let Some(skills_cmd) = cmd2.find_subcommand_mut("skills") {
                        skills_cmd.print_help()?;
                        println!();
                    }
                    return Ok(());
                }
            };
            skills_run(cmd)?;
        }
        Some((store_name, _)) => {
            // Must be a store subcommand — dispatch
            // Check store is known
            if manifest.stores.iter().any(|s| s.name == store_name) {
                cli::dispatch::dispatch(&matches, &manifest, &schemas)?;
            } else {
                eprintln!("Unknown subcommand '{store_name}'. Run `stores init` first or `stores install <path>`.");
                std::process::exit(1);
            }
        }
        None => {
            // Re-parse to print help
            let mut cmd2 = cli::dynamic::build_root(&manifest, &schemas);
            cmd2.print_help()?;
            println!();
        }
    }

    Ok(())
}
