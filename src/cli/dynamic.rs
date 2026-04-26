use clap::{Arg, ArgAction, Command};
use std::collections::HashMap;

use crate::manifest::Manifest;
use crate::schema::{FieldType, Schema};
use crate::schema::flatten::leaf_args;

/// Build the root `stores` Command with all installed stores added as subcommands.
pub fn build_root(manifest: &Manifest, schemas: &HashMap<String, Schema>) -> Command {
    let mut root = Command::new("stores")
        .about("Schema-driven store framework")
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .global(true)
                .help("Output as JSON"),
        )
        .arg(
            Arg::new("invoker")
                .long("invoker")
                .global(true)
                .help("Override actor detection: human | ai_autonomous | ai_with_human"),
        )
        // Init subcommand
        .subcommand(
            Command::new("init").about("Initialize .stores/ in the current directory"),
        )
        // Install subcommand
        .subcommand(
            Command::new("install")
                .about("Install a store from a directory containing schema.yaml")
                .arg(
                    Arg::new("path")
                        .help("Path to the store directory")
                        .required(true),
                ),
        );

    // Add one subcommand per installed store
    for store in &manifest.stores {
        if let Some(schema) = schemas.get(&store.name) {
            root = root.subcommand(build_store_command(schema));
        }
    }

    root
}

/// Build a subcommand for a single store with add/show/list/update verbs.
fn build_store_command(schema: &Schema) -> Command {
    // Get leaf args — uniqueness already enforced at install time
    let leaves = leaf_args(schema).unwrap_or_default();

    let add_cmd = build_add_cmd(&leaves, schema);
    let update_cmd = build_update_cmd(&leaves, schema);
    let show_cmd = build_show_cmd();
    let list_cmd = build_list_cmd();

    Command::new(schema.name.clone())
        .about(format!("Operate on the '{}' store", schema.name))
        .subcommand(add_cmd)
        .subcommand(show_cmd)
        .subcommand(list_cmd)
        .subcommand(update_cmd)
}

/// Build the `add` command with leaf args.
fn build_add_cmd(
    leaves: &[crate::schema::flatten::LeafArg<'_>],
    _schema: &Schema,
) -> Command {
    build_leaf_cmd("add", leaves, false)
}

/// Build the `update` command with leaf args + positional display_id.
fn build_update_cmd(
    leaves: &[crate::schema::flatten::LeafArg<'_>],
    _schema: &Schema,
) -> Command {
    build_leaf_cmd("update", leaves, true)
}

fn build_leaf_cmd(
    verb: &'static str,
    leaves: &[crate::schema::flatten::LeafArg<'_>],
    needs_display_id: bool,
) -> Command {
    let mut cmd = Command::new(verb).about(format!("{verb} an entry"));

    if needs_display_id {
        cmd = cmd.arg(
            Arg::new("display_id")
                .help("Display ID of the entry")
                .required(true),
        );
    }

    for leaf in leaves {
        if is_reserved(&leaf.cli_name) {
            continue;
        }

        let is_text_like = matches!(
            leaf.field.ty,
            FieldType::Text | FieldType::Timestamp | FieldType::DisplayId
        );

        // Main arg — clone the String into a Box<str> to satisfy Into<Id>
        cmd = cmd.arg(
            Arg::new(leaf.cli_name.clone())
                .long(leaf.cli_name.clone())
                .help(
                    leaf.field
                        .description
                        .clone()
                        .unwrap_or_else(|| leaf.cli_name.clone()),
                )
                .required(false),
        );

        // --<name>-from-file companion for Text-like fields
        if is_text_like {
            let from_file_name = format!("{}-from-file", leaf.cli_name);
            cmd = cmd.arg(
                Arg::new(from_file_name.clone())
                    .long(from_file_name)
                    .help(format!(
                        "Load '{}' from a file path (use '-' for stdin)",
                        leaf.cli_name
                    ))
                    .required(false),
            );
        }
    }

    cmd
}

/// Build the `show` command.
fn build_show_cmd() -> Command {
    Command::new("show")
        .about("show an entry")
        .arg(
            Arg::new("display_id")
                .help("Display ID of the entry")
                .required(true),
        )
}

/// Build the `list` command.
fn build_list_cmd() -> Command {
    Command::new("list").about("list entries")
}

/// Names that collide with our reserved layer or clap builtins.
fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "id"
            | "display-id"
            | "status"
            | "created-at"
            | "updated-at"
            | "created-by"
            | "updated-by"
            | "json"
            | "help"
            | "version"
    )
}
