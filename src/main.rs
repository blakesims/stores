mod cli;
mod codegen;
mod db;
pub mod flow;
pub mod handlers;
pub mod id_format;
mod install;
mod manifest;
mod output;
mod paths;
pub mod render;
pub mod runner;
pub mod schema;
pub mod validate;

use anyhow::Result;
use std::collections::HashMap;

use manifest::Manifest;
use schema::Schema;

fn main() -> Result<()> {
    // Determine whether a manifest exists (init must work without one)
    let manifest_exists = paths::manifest_path().map(|p| p.exists()).unwrap_or(false);

    // Load manifest + schemas if available
    let (manifest, schemas) = if manifest_exists {
        let m = Manifest::load()?;
        let mut s: HashMap<String, Schema> = HashMap::new();
        for store in &m.stores {
            let path_str = store.schema_path.to_string_lossy();
            let yaml = if let Some(bundled_name) = path_str.strip_prefix("bundled:") {
                // Bundled store: find embedded schema by name
                cli::dynamic::BUNDLED_STORE_SCHEMAS
                    .iter()
                    .find(|(n, _)| *n == bundled_name)
                    .map(|(_, y)| y.to_string())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "bundled store '{}' not found in binary; was the binary rebuilt?",
                            bundled_name
                        )
                    })?
            } else {
                let schema_file = store.schema_path.join("schema.yaml");
                std::fs::read_to_string(&schema_file)?
            };
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
        Some(("list-installable", _)) => {
            use cli::dynamic::BUNDLED_STORE_NAMES;
            println!("Bundled stores (run `stores install <name>` to install one or more):");
            println!();
            for name in BUNDLED_STORE_NAMES {
                println!("  {name}");
            }
        }
        Some(("setup", sub)) => {
            let global = *sub.get_one::<bool>("global").unwrap_or(&false);
            cli::setup::run(global)?;
        }
        Some(("auth", sub)) => {
            use cli::auth::{run as auth_run, AuthCmd};
            let cmd = match sub.subcommand() {
                Some(("init", isub)) => AuthCmd::Init {
                    recipient: isub.get_one::<String>("recipient").cloned(),
                    identity: isub
                        .get_one::<String>("identity")
                        .map(std::path::PathBuf::from),
                    force: *isub.get_one::<bool>("force").unwrap_or(&false),
                },
                Some(("show", _)) => AuthCmd::Show,
                _ => {
                    let mut cmd2 = cli::dynamic::build_root(&manifest, &schemas);
                    if let Some(auth_cmd) = cmd2.find_subcommand_mut("auth") {
                        auth_cmd.print_help()?;
                        println!();
                    }
                    return Ok(());
                }
            };
            auth_run(cmd)?;
        }
        Some(("topology", sub)) => {
            use cli::topology::{Format, Opts};
            let format = match sub.get_one::<String>("format").map(|s| s.as_str()) {
                Some("dot") => Format::Dot,
                Some("mermaid") => Format::Mermaid,
                Some("auto") | None => Format::Auto,
                Some(other) => {
                    eprintln!(
                        "error: unknown --format '{other}'; expected 'auto', 'dot', or 'mermaid'"
                    );
                    std::process::exit(2);
                }
            };
            let opts = Opts {
                format,
                store_filter: sub.get_one::<String>("store").cloned(),
                no_icons: *sub.get_one::<bool>("no-icons").unwrap_or(&false),
            };
            cli::topology::run(&manifest, &schemas, opts)?;
        }
        Some(("watch", sub)) => {
            let interval_secs = sub.get_one::<f64>("interval").copied().unwrap_or(1.0);
            let interval_ms = (interval_secs * 1000.0).max(100.0) as u64;
            cli::watch::run(interval_ms)?;
        }
        Some(("skills", sub)) => {
            use cli::skills::{run as skills_run, SkillsCmd};
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
        Some(("agents", sub)) => {
            use cli::agents::{run as agents_run, AgentsCmd};
            // `agents run` and `agents backfill` are dispatched separately
            // (handlers::agents_run) — they need DB access, not file install.
            if let Some(("run", rsub)) = sub.subcommand() {
                let poll_interval_secs =
                    rsub.get_one::<f64>("poll-interval").copied().unwrap_or(5.0);
                let detach = *rsub.get_one::<bool>("detach").unwrap_or(&false);
                let log_file = rsub.get_one::<String>("log-file").cloned();
                let max_iters = rsub.get_one::<usize>("max-iters").copied();
                let args = handlers::agents_run::RunArgs {
                    poll_interval_ms: (poll_interval_secs * 1000.0).max(50.0) as u64,
                    detach,
                    log_file,
                    max_iters,
                };
                handlers::agents_run::run_daemon(args)?;
                return Ok(());
            }
            if let Some(("backfill", _)) = sub.subcommand() {
                handlers::agents_run::run_backfill_placeholder()?;
                return Ok(());
            }
            let cmd = match sub.subcommand() {
                Some(("list", _)) => AgentsCmd::List,
                Some(("install", isub)) => AgentsCmd::Install {
                    name: isub.get_one::<String>("name").cloned(),
                    all: *isub.get_one::<bool>("all").unwrap_or(&false),
                    global: *isub.get_one::<bool>("global").unwrap_or(&false),
                },
                Some(("uninstall", usub)) => AgentsCmd::Uninstall {
                    name: usub.get_one::<String>("name").unwrap().clone(),
                    global: *usub.get_one::<bool>("global").unwrap_or(&false),
                },
                _ => {
                    // Print agents help
                    let mut cmd2 = cli::dynamic::build_root(&manifest, &schemas);
                    if let Some(agents_cmd) = cmd2.find_subcommand_mut("agents") {
                        agents_cmd.print_help()?;
                        println!();
                    }
                    return Ok(());
                }
            };
            agents_run(cmd)?;
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
