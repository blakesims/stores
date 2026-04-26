mod cli;
mod codegen;
mod db;
pub mod id_format;
mod install;
mod manifest;
mod paths;
pub mod schema;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "stores", about = "Schema-driven store framework")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Remaining args for dynamically-added store subcommands (Phase 4)
    #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
    args: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize .stores/ in the current directory
    Init,
    /// Install a store from a directory containing schema.yaml
    Install {
        /// Path to the store directory
        path: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init) => cli::init::run(),
        Some(Commands::Install { path }) => {
            install::run(std::path::Path::new(&path))
        }
        None => {
            if cli.args.is_empty() {
                // Print help when no subcommand given
                let mut cmd = Cli::command();
                cmd.print_help()?;
                println!();
            } else {
                eprintln!(
                    "Unknown subcommand '{}'. Run `stores init` first or `stores install <path>`.",
                    cli.args[0]
                );
                std::process::exit(1);
            }
            Ok(())
        }
    }
}
