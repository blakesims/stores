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
pub mod tui;
pub mod validate;
mod version;

use anyhow::Result;
use std::collections::HashMap;

use manifest::Manifest;
use schema::Schema;

/// T023 P2 — Pre-parse `--meta` from raw argv so the override is installed
/// before manifest/schema loading. Returns `Some("")` for `--meta` without a
/// value (sentinel: defer to STORES_META_PATH), `Some(val)` for `--meta val`
/// or `--meta=val`, `None` if the flag is absent.
fn parse_meta_from_argv() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--meta" {
            // Bare `--meta` is the sentinel form: defer to STORES_META_PATH.
            // Use `--meta=PATH` to pass an explicit value (avoids ambiguity
            // with the following subcommand name).
            return Some(String::new());
        } else if let Some(rest) = a.strip_prefix("--meta=") {
            return Some(rest.to_string());
        }
        i += 1;
    }
    None
}

/// Pre-parse `--stores-root` from raw argv. Returns `Some("")` for bare
/// `--stores-root` (sentinel: defer to STORES_ROOT), `Some(val)` for
/// `--stores-root=val`, `None` if absent.
fn parse_stores_root_from_argv() -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if a == "--stores-root" {
            return Some(String::new());
        } else if let Some(rest) = a.strip_prefix("--stores-root=") {
            return Some(rest.to_string());
        }
        i += 1;
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteChoice {
    Default,
    Meta,
    StoresRoot,
}

fn decide_route(
    meta_flag: Option<&str>,
    meta_env_present: bool,
    root_flag: Option<&str>,
    root_env_present: bool,
) -> Result<RouteChoice> {
    let meta_source_present = meta_flag.is_some() || meta_env_present;
    let root_source_present = root_flag.is_some() || root_env_present;
    if meta_source_present && root_source_present {
        anyhow::bail!(
            "--meta and --stores-root are mutually exclusive: --meta files at the META substrate, --stores-root operates on a substratum store. Pick one."
        );
    }
    if root_flag.is_some() || root_env_present {
        Ok(RouteChoice::StoresRoot)
    } else if meta_flag.is_some() || meta_env_present {
        Ok(RouteChoice::Meta)
    } else {
        Ok(RouteChoice::Default)
    }
}

fn main() -> Result<()> {
    // T023 P2: --meta early-bind. If --meta is present on argv or
    // STORES_META_PATH is set, resolve and install the stores-dir override
    // BEFORE manifest/schema loading so every downstream consumer of
    // `paths::stores_dir()` routes to the META substrate.
    let meta_flag = parse_meta_from_argv();
    let stores_root_flag = parse_stores_root_from_argv();
    let meta_env_present = std::env::var("STORES_META_PATH")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let stores_root_env_present = std::env::var("STORES_ROOT")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    match decide_route(
        meta_flag.as_deref(),
        meta_env_present,
        stores_root_flag.as_deref(),
        stores_root_env_present,
    )? {
        RouteChoice::StoresRoot => {
            let root = paths::resolve_stores_root(stores_root_flag.as_deref())?;
            paths::set_stores_dir_override(root.join(".stores"))?;
        }
        RouteChoice::Meta => {
            let meta_root = paths::resolve_meta_path(meta_flag.as_deref())?;
            paths::set_stores_dir_override(meta_root.join(".stores"))?;
        }
        RouteChoice::Default => {}
    }

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
        Some(("__llm-off-sentinel", _)) => {
            println!("stores-llm-off-sentinel=ok");
        }
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
        Some(("migrate", sub)) => {
            handlers::migrate::run_migrate(sub.get_flag("apply"))?;
        }
        Some(("setup", sub)) => {
            let global = *sub.get_one::<bool>("global").unwrap_or(&false);
            cli::setup::run(global)?;
        }
        Some(("test", sub)) => match sub.subcommand() {
            Some(("run", rsub)) => {
                cli::test::run(cli::test::TestRunOpts {
                    case_name: rsub.get_one::<String>("case").cloned(),
                    case_file: rsub
                        .get_one::<String>("case-file")
                        .map(std::path::PathBuf::from),
                    delay_ms: rsub.get_one::<u64>("delay-ms").copied(),
                    watch: *rsub.get_one::<bool>("watch").unwrap_or(&false),
                    live: *rsub.get_one::<bool>("live").unwrap_or(&false),
                })?;
            }
            Some(("suite", ssub)) => {
                let suite = ssub.get_one::<String>("suite").unwrap().as_str();
                let cases: &[&str] = match suite {
                    "dogfood-smoke" | "battlescars" => &["happy-path", "t3-failed-er"],
                    other => anyhow::bail!("unknown stores test suite '{other}'"),
                };
                for case in cases {
                    cli::test::run(cli::test::TestRunOpts {
                        case_name: Some((*case).to_string()),
                        case_file: None,
                        delay_ms: ssub.get_one::<u64>("delay-ms").copied(),
                        watch: *ssub.get_one::<bool>("watch").unwrap_or(&false),
                        live: *ssub.get_one::<bool>("live").unwrap_or(&false),
                    })?;
                }
            }
            Some(("enumerate", esub)) => {
                let catalog = cli::test::matrix::Catalog::parse(
                    esub.get_one::<String>("catalog")
                        .map(String::as_str)
                        .unwrap_or("smoke"),
                )?;
                cli::test::matrix::run_enumerate(cli::test::matrix::EnumerateOpts {
                    catalog,
                    coverage: *esub.get_one::<bool>("coverage").unwrap_or(&false),
                })?;
            }
            Some(("matrix", msub)) => {
                if let Some(("prune", psub)) = msub.subcommand() {
                    cli::test::matrix::prune_matrix_runs(
                        *psub.get_one::<usize>("keep-last").unwrap_or(&0),
                    )?;
                } else {
                    let catalog = cli::test::matrix::Catalog::parse(
                        msub.get_one::<String>("catalog")
                            .map(String::as_str)
                            .unwrap_or("smoke"),
                    )?;
                    let mode = cli::test::matrix::MatrixMode::parse(
                        msub.get_one::<String>("mode")
                            .map(String::as_str)
                            .unwrap_or("lab"),
                    )?;
                    let report = cli::test::matrix::MatrixReport::parse(
                        msub.get_one::<String>("report")
                            .map(String::as_str)
                            .unwrap_or("md"),
                    )?;
                    cli::test::matrix::run_matrix(cli::test::matrix::MatrixOpts {
                        catalog,
                        mode,
                        only: msub.get_one::<String>("only").cloned(),
                        watch: *msub.get_one::<bool>("watch").unwrap_or(&false),
                        current_ack: *msub
                            .get_one::<bool>("i-understand-this-mutates-current-repo")
                            .unwrap_or(&false),
                        report,
                        ci: *msub.get_one::<bool>("ci").unwrap_or(&false),
                    })?;
                }
            }
            _ => {
                let mut cmd2 = cli::dynamic::build_root(&manifest, &schemas);
                if let Some(test_cmd) = cmd2.find_subcommand_mut("test") {
                    test_cmd.print_help()?;
                    println!();
                }
            }
        },
        Some(("auth", sub)) => {
            use cli::auth::{run as auth_run, AuthCmd};
            let cmd = match sub.subcommand() {
                Some(("init", isub)) => AuthCmd::Init {
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
        Some(("metrics", sub)) => {
            let args = cli::metrics::MetricsArgs {
                window: sub.get_one::<String>("window").unwrap().clone(),
                text: *sub.get_one::<bool>("text").unwrap_or(&false),
                // Accept metrics-local --json or global --json flag.
                json: *sub.get_one::<bool>("json").unwrap_or(&false) || matches.get_flag("json"),
                now: sub.get_one::<String>("now").cloned(),
            };
            cli::metrics::run(args)?;
        }
        Some(("runner-stats", sub)) => {
            let json = *sub.get_one::<bool>("json").unwrap_or(&false) || matches.get_flag("json");
            let filters = cli::runner_stats::RunnerStatsFilters {
                display_id: sub.get_one::<String>("display_id").map(|s| s.as_str()),
                role: sub.get_one::<String>("role").map(|s| s.as_str()),
                harness: sub.get_one::<String>("harness").map(|s| s.as_str()),
                model: sub.get_one::<String>("model").map(|s| s.as_str()),
                thinking: sub.get_one::<String>("thinking").map(|s| s.as_str()),
                since: sub.get_one::<String>("since").map(|s| s.as_str()),
                until: sub.get_one::<String>("until").map(|s| s.as_str()),
                include_dirty_data: *sub.get_one::<bool>("include_dirty_data").unwrap_or(&false),
            };
            cli::runner_stats::run(json, filters)?;
        }
        Some(("engine", sub)) => {
            match sub.subcommand() {
                Some(("locks", lsub)) => {
                    let json =
                        *lsub.get_one::<bool>("json").unwrap_or(&false) || matches.get_flag("json");
                    cli::engine::run_locks(json)?;
                }
                Some(("plan-start", psub)) => {
                    let json =
                        *psub.get_one::<bool>("json").unwrap_or(&false) || matches.get_flag("json");
                    // Read-only end-to-end: plan-start opens the DB read-only and
                    // must NOT trigger startup sweeps, daemon ticks, or any
                    // side-effecting subscriber. (T140 P4 contract.)
                    cli::engine::run_plan_start(json)?;
                }
                _ => {
                    let mut cmd2 = cli::dynamic::build_root(&manifest, &schemas);
                    if let Some(engine_cmd) = cmd2.find_subcommand_mut("engine") {
                        engine_cmd.print_help()?;
                        println!();
                    }
                }
            }
        }
        Some(("resource-locks", sub)) => {
            cli::dispatch::dispatch_resource_locks(&matches, sub)?;
        }
        Some(("runs", sub)) => {
            use cli::runs::{run as runs_run, RunsCmd};
            let cmd = match sub.subcommand() {
                Some(("list", lsub)) => RunsCmd::List {
                    display_id: lsub.get_one::<String>("display_id").unwrap().clone(),
                },
                Some(("show", ssub)) => RunsCmd::Show {
                    display_id: ssub.get_one::<String>("display_id").unwrap().clone(),
                    phase: *ssub.get_one::<i64>("phase").unwrap(),
                    cycle: ssub.get_one::<i64>("cycle").copied(),
                    role: ssub.get_one::<String>("role").unwrap().clone(),
                },
                Some(("current", csub)) => RunsCmd::Current {
                    display_id: csub.get_one::<String>("display_id").unwrap().clone(),
                    role: csub.get_one::<String>("role").cloned(),
                },
                Some(("tail", tsub)) => RunsCmd::Tail {
                    display_id: tsub.get_one::<String>("display_id").unwrap().clone(),
                    role: tsub.get_one::<String>("role").cloned(),
                    raw: *tsub.get_one::<bool>("raw").unwrap_or(&false),
                    stderr: *tsub.get_one::<bool>("stderr").unwrap_or(&false),
                },
                Some(("gc", gsub)) => {
                    let mut opts = cli::runs::RunsGcOpts::default();
                    opts.execute = *gsub.get_one::<bool>("execute").unwrap_or(&false);
                    if *gsub.get_one::<bool>("dry-run").unwrap_or(&false) && opts.execute {
                        anyhow::bail!("runs gc accepts only one of --dry-run or --execute");
                    }
                    if let Some(s) = gsub.get_one::<String>("max-bytes") {
                        opts.max_bytes = cli::runs::parse_size_bytes(s)?;
                    }
                    if let Some(s) = gsub.get_one::<String>("warn-bytes") {
                        opts.warn_bytes = cli::runs::parse_size_bytes(s)?;
                    }
                    if let Some(s) = gsub.get_one::<String>("per-file-warn-bytes") {
                        opts.per_file_warn_bytes = cli::runs::parse_size_bytes(s)?;
                    }
                    if let Some(n) = gsub.get_one::<usize>("largest") {
                        opts.largest = *n;
                    }
                    RunsCmd::Gc(opts)
                }
                _ => {
                    let mut cmd2 = cli::dynamic::build_root(&manifest, &schemas);
                    if let Some(runs_cmd) = cmd2.find_subcommand_mut("runs") {
                        runs_cmd.print_help()?;
                        println!();
                    }
                    return Ok(());
                }
            };
            runs_run(cmd)?;
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
            let legacy = *sub.get_one::<bool>("legacy").unwrap_or(&false);
            let all_history = *sub.get_one::<bool>("all-history").unwrap_or(&false)
                || *sub.get_one::<bool>("all").unwrap_or(&false);
            if legacy {
                cli::watch::run(interval_ms, all_history)?;
            } else {
                let opts = stores::tui::TuiOpts {
                    interval_ms,
                    state_filter: sub.get_one::<String>("state").cloned(),
                    priority_filter: sub.get_one::<String>("priority").cloned(),
                    tier_filter: sub.get_one::<String>("tier").cloned(),
                    since_filter: sub.get_one::<String>("since").cloned(),
                    legacy: false,
                    all_history,
                    claude_bin: None,
                };
                stores::tui::run(opts)?;
            }
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
                let once = *rsub.get_one::<bool>("once").unwrap_or(&false);
                let max_iters = rsub.get_one::<usize>("max-iters").copied().or(if once {
                    Some(1)
                } else {
                    None
                });
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
                handlers::agents_backfill::run_backfill()?;
                return Ok(());
            }
            if let Some(("telemetry-backfill", _)) = sub.subcommand() {
                handlers::agent_run_telemetry_backfill::run_agent_run_telemetry_backfill()?;
                return Ok(());
            }
            if let Some(("stop", stop_sub)) = sub.subcommand() {
                let force = *stop_sub.get_one::<bool>("force").unwrap_or(&false);
                handlers::agents_stop::run_stop(handlers::agents_stop::StopOptions { force })?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_route_errors_when_meta_and_stores_root_conflict() {
        let err = decide_route(Some("/meta"), false, Some("/root"), false).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));

        let err = decide_route(Some(""), false, Some(""), false).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));

        let err = decide_route(Some("/meta"), false, Some(""), false).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));

        let err = decide_route(None, true, None, true).unwrap_err();
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn decide_route_selects_single_route_sources() {
        assert_eq!(
            decide_route(None, false, Some("/root"), false).unwrap(),
            RouteChoice::StoresRoot
        );
        assert_eq!(
            decide_route(Some("/meta"), false, None, false).unwrap(),
            RouteChoice::Meta
        );
        assert_eq!(
            decide_route(None, false, None, false).unwrap(),
            RouteChoice::Default
        );
    }
}
