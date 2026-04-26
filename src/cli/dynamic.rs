use clap::{Arg, ArgAction, Command};
use std::collections::HashMap;

use crate::manifest::Manifest;
use crate::schema::{FieldType, Schema};
use crate::schema::flatten::leaf_args;

// ---------------------------------------------------------------------------
// Bundled stores — embedded at compile time
// ---------------------------------------------------------------------------

/// Names of bundled stores (the subdirectory name == schema name).
pub static BUNDLED_STORE_NAMES: &[&str] = &["observations", "gate"];

/// Embedded schema.yaml content for each bundled store (same order as BUNDLED_STORE_NAMES).
pub static BUNDLED_STORE_SCHEMAS: &[(&str, &str)] = &[
    (
        "observations",
        include_str!("../../stores/observations/schema.yaml"),
    ),
    (
        "gate",
        include_str!("../../stores/gate/schema.yaml"),
    ),
];

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
        )
        // list-installable subcommand
        .subcommand(
            Command::new("list-installable")
                .about("List stores bundled with the binary (run `stores install <name>` to install one)"),
        )
        // Skills subcommand
        .subcommand(
            Command::new("skills")
                .about("Manage bundled Claude Code skill suggestions")
                .subcommand(Command::new("list").about("List available bundled skills"))
                .subcommand(
                    Command::new("install")
                        .about("Install a bundled skill into .claude/skills/")
                        .arg(Arg::new("name").help("Skill name").required(false))
                        .arg(
                            Arg::new("all")
                                .long("all")
                                .action(ArgAction::SetTrue)
                                .help("Install all bundled skills"),
                        )
                        .arg(
                            Arg::new("global")
                                .long("global")
                                .action(ArgAction::SetTrue)
                                .help("Install to ~/.claude/skills/ instead of ./.claude/skills/"),
                        ),
                )
                .subcommand(
                    Command::new("uninstall")
                        .about("Remove an installed bundled skill")
                        .arg(
                            Arg::new("name")
                                .help("Skill name")
                                .required(true),
                        )
                        .arg(
                            Arg::new("global")
                                .long("global")
                                .action(ArgAction::SetTrue)
                                .help("Remove from ~/.claude/skills/ instead of ./.claude/skills/"),
                        ),
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

/// Build a subcommand for a single store with add/show/list/update/schema verbs
/// plus one verb per lifecycle transition declared in the schema.
/// If the schema has `workflow: Some(_)`, also registers `next-action` and `brief`.
fn build_store_command(schema: &Schema) -> Command {
    // Get leaf args — uniqueness already enforced at install time
    let leaves = leaf_args(schema).unwrap_or_default();

    let add_cmd = build_add_cmd(&leaves, schema);
    let update_cmd = build_update_cmd(&leaves, schema);
    let show_cmd = build_show_cmd();
    let list_cmd = build_list_cmd(schema);
    let schema_cmd = build_schema_cmd();

    // Base verb names reserved by the framework
    const BASE_VERBS: &[&str] = &["add", "show", "list", "update", "schema"];

    let mut store_cmd = Command::new(schema.name.clone())
        .about(format!("Operate on the '{}' store", schema.name))
        .subcommand(add_cmd)
        .subcommand(show_cmd)
        .subcommand(list_cmd)
        .subcommand(update_cmd)
        .subcommand(schema_cmd);

    // Workflow-only verbs: only when schema has a workflow declaration.
    if schema.workflow.is_some() {
        store_cmd = store_cmd
            .subcommand(build_next_action_cmd())
            .subcommand(build_brief_cmd())
            .subcommand(build_render_cmd())
            .subcommand(build_submit_plan_cmd())
            .subcommand(build_submit_plan_review_cmd())
            .subcommand(build_submit_execute_cmd())
            .subcommand(build_submit_review_cmd())
            .subcommand(build_resume_cmd());
    }

    // Register one subcommand per transition verb
    for transition in &schema.lifecycle.transitions {
        let verb = &transition.verb;
        // Warn if verb collides with a base verb; skip it (don't crash)
        if BASE_VERBS.contains(&verb.as_str()) {
            eprintln!(
                "warning: transition verb '{}' in store '{}' collides with a base verb; skipping",
                verb, schema.name
            );
            continue;
        }
        let transition_cmd = build_transition_cmd(verb, &leaves);
        store_cmd = store_cmd.subcommand(transition_cmd);
    }

    store_cmd
}

/// Build the `next-action` command: positional <id> + --json (global).
fn build_next_action_cmd() -> Command {
    Command::new("next-action")
        .about("Show which agent should act next on a workflow entry (read-only)")
        .arg(
            Arg::new("display_id")
                .help("Display ID of the entry")
                .required(true),
        )
}

/// Build the `brief` command: positional <id> + optional --for <agent> + --json (global).
fn build_brief_cmd() -> Command {
    Command::new("brief")
        .about("Print the agent briefing for a workflow entry (read-only)")
        .arg(
            Arg::new("display_id")
                .help("Display ID of the entry")
                .required(true),
        )
        .arg(
            Arg::new("for")
                .long("for")
                .help("Agent role to generate the briefing for (defaults to next-action answer)")
                .required(false),
        )
}

/// Build the `render` command: positional <id> + optional --dry-run.
fn build_render_cmd() -> Command {
    Command::new("render")
        .about("Render main.md for a workflow entry to disk (read-only against DB)")
        .arg(
            Arg::new("display_id")
                .help("Display ID of the entry")
                .required(true),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Print rendered content to stdout without writing to disk")
                .required(false),
        )
}

/// Build the `submit-plan` command.
fn build_submit_plan_cmd() -> Command {
    Command::new("submit-plan")
        .about("Submit the plan for a workflow entry (planning → plan_review)")
        .arg(Arg::new("display_id").help("Display ID").required(true))
        .arg(
            Arg::new("plan-from-file")
                .long("plan-from-file")
                .help("Path to plan JSON file (use '-' for stdin)")
                .required(false),
        )
}

/// Build the `submit-plan-review` command.
fn build_submit_plan_review_cmd() -> Command {
    Command::new("submit-plan-review")
        .about("Submit a plan review (plan_review → ready | planning | blocked)")
        .arg(Arg::new("display_id").help("Display ID").required(true))
        .arg(
            Arg::new("gate")
                .long("gate")
                .help("Gate decision: READY | NEEDS_WORK | NOT_READY")
                .required(true),
        )
        .arg(
            Arg::new("summary")
                .long("summary")
                .help("Review summary")
                .required(false),
        )
        .arg(
            Arg::new("summary-from-file")
                .long("summary-from-file")
                .help("Load summary from file (use '-' for stdin)")
                .required(false),
        )
        .arg(
            Arg::new("open-questions-from-file")
                .long("open-questions-from-file")
                .help("Load open questions from file (one per line)")
                .required(false),
        )
}

/// Build the `submit-execute` command.
fn build_submit_execute_cmd() -> Command {
    Command::new("submit-execute")
        .about("Submit execution results (executing → code_review)")
        .arg(Arg::new("display_id").help("Display ID").required(true))
        .arg(
            Arg::new("summary")
                .long("summary")
                .help("Execution summary")
                .required(false),
        )
        .arg(
            Arg::new("summary-from-file")
                .long("summary-from-file")
                .help("Load summary from file")
                .required(false),
        )
        .arg(
            Arg::new("commit")
                .long("commit")
                .help("Git commit SHA")
                .required(false),
        )
        .arg(
            Arg::new("files-changed")
                .long("files-changed")
                .help("Comma-separated list of changed files")
                .required(false),
        )
        .arg(
            Arg::new("notes-from-file")
                .long("notes-from-file")
                .help("Load additional notes from file")
                .required(false),
        )
}

/// Build the `submit-review` command.
fn build_submit_review_cmd() -> Command {
    Command::new("submit-review")
        .about("Submit a code review (code_review → executing | complete | blocked)")
        .arg(Arg::new("display_id").help("Display ID").required(true))
        .arg(
            Arg::new("gate")
                .long("gate")
                .help("Gate decision: PASS | REVISE | FAIL")
                .required(true),
        )
        .arg(
            Arg::new("critical")
                .long("critical")
                .help("Number of critical findings")
                .value_parser(clap::value_parser!(i64))
                .required(false),
        )
        .arg(
            Arg::new("major")
                .long("major")
                .help("Number of major findings")
                .value_parser(clap::value_parser!(i64))
                .required(false),
        )
        .arg(
            Arg::new("minor")
                .long("minor")
                .help("Number of minor findings")
                .value_parser(clap::value_parser!(i64))
                .required(false),
        )
        .arg(
            Arg::new("summary")
                .long("summary")
                .help("Review summary")
                .required(false),
        )
        .arg(
            Arg::new("details-from-file")
                .long("details-from-file")
                .help("Load detailed findings from file")
                .required(false),
        )
}

/// Build the `resume` command.
fn build_resume_cmd() -> Command {
    Command::new("resume")
        .about("Resume a blocked workflow entry (blocked → ready → executing)")
        .arg(Arg::new("display_id").help("Display ID").required(true))
        .arg(
            Arg::new("summary")
                .long("summary")
                .help("Optional reason for resuming")
                .required(false),
        )
}

/// Build a transition verb subcommand: positional display_id + all leaf args.
fn build_transition_cmd(
    verb: &str,
    leaves: &[crate::schema::flatten::LeafArg<'_>],
) -> Command {
    build_leaf_cmd_owned(verb.to_string(), leaves, true)
}

/// Same as `build_leaf_cmd` but accepts an owned verb String (for transition verbs).
fn build_leaf_cmd_owned(
    verb: String,
    leaves: &[crate::schema::flatten::LeafArg<'_>],
    needs_display_id: bool,
) -> Command {
    let about = format!("{verb} an entry");
    let mut cmd = Command::new(verb).about(about);

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

/// Build the `list` command with filter/sort/limit flags.
fn build_list_cmd(schema: &Schema) -> Command {
    // Build the sorted hint for --sort help text
    let col_names: Vec<String> = {
        let mut cols = vec![
            "status".to_string(),
            "created_at".to_string(),
            "updated_at".to_string(),
            "created_by".to_string(),
            "updated_by".to_string(),
            "display_id".to_string(),
        ];
        for f in &schema.fields {
            cols.push(f.name.clone());
        }
        cols
    };
    let cols_help = col_names.join(", ");

    Command::new("list")
        .about("list entries")
        .arg(
            Arg::new("status")
                .long("status")
                .help("Filter rows where status == value")
                .required(false),
        )
        .arg(
            Arg::new("limit")
                .long("limit")
                .help("Limit result count to N rows")
                .value_parser(clap::value_parser!(u64))
                .required(false),
        )
        .arg(
            Arg::new("sort")
                .long("sort")
                .help(format!("Order by column ascending. Valid columns: {cols_help}"))
                .required(false),
        )
        .arg(
            Arg::new("reverse")
                .long("reverse")
                .action(ArgAction::SetTrue)
                .help("Reverse the sort order (descending)")
                .required(false),
        )
        .arg(
            Arg::new("since")
                .long("since")
                .help("Filter rows where created_at >= date (YYYY-MM-DD)")
                .required(false),
        )
}

/// Build the `schema` command.
fn build_schema_cmd() -> Command {
    Command::new("schema").about("Print the schema for this store")
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
