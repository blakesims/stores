use clap::{Arg, ArgAction, Command};
use std::collections::HashMap;

use crate::manifest::Manifest;
use crate::schema::{FieldType, Schema};
use crate::schema::flatten::leaf_args;

// ---------------------------------------------------------------------------
// Bundled stores — embedded at compile time
// ---------------------------------------------------------------------------

/// Names of bundled stores (the subdirectory name == schema name).
pub static BUNDLED_STORE_NAMES: &[&str] = &["observations", "gate", "tasks"];

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
    (
        "tasks",
        include_str!("../../stores/tasks/schema.yaml"),
    ),
];

/// Embedded template content for bundled workflow stores.
///
/// Map: store-name → list of (template-relative-path, content).
/// Path keys must match schema's `briefing_templates` and `render_template` values.
/// Used by brief.rs and render.rs when `schema_path` starts with `"bundled:"`.
pub static BUNDLED_STORE_TEMPLATES: &[(&str, &[(&str, &str)])] = &[
    ("tasks", &[
        ("templates/planner-brief.md.tpl",
            include_str!("../../stores/tasks/templates/planner-brief.md.tpl")),
        ("templates/plan-reviewer-brief.md.tpl",
            include_str!("../../stores/tasks/templates/plan-reviewer-brief.md.tpl")),
        ("templates/executor-brief.md.tpl",
            include_str!("../../stores/tasks/templates/executor-brief.md.tpl")),
        ("templates/code-reviewer-brief.md.tpl",
            include_str!("../../stores/tasks/templates/code-reviewer-brief.md.tpl")),
        ("templates/wrap-brief.md.tpl",
            include_str!("../../stores/tasks/templates/wrap-brief.md.tpl")),
        ("templates/main.md.tpl",
            include_str!("../../stores/tasks/templates/main.md.tpl")),
    ]),
];

/// Build the root `stores` Command with all installed stores added as subcommands.
pub fn build_root(manifest: &Manifest, schemas: &HashMap<String, Schema>) -> Command {
    let mut root = Command::new("stores")
        .about("Schema-driven store framework")
        .version(env!("CARGO_PKG_VERSION"))
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
        .arg(
            Arg::new("approve-token")
                .long("approve-token")
                .global(true)
                .help(
                    "Plaintext approval token (chat-mediated human assent). \
                     Verified against ~/.config/stores/approve.token.hash via \
                     constant-time compare. Phase 2: validated only; Phase 3 \
                     consumes the bit to relax actor: human gates.",
                ),
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
        // Setup subcommand — single-command bootstrap
        .subcommand(
            Command::new("setup")
                .about("Bootstrap: init + install all bundled stores + install all skills and agents")
                .arg(
                    Arg::new("global")
                        .long("global")
                        .action(ArgAction::SetTrue)
                        .help("Install skills and agents to ~/.claude/ instead of ./.claude/"),
                ),
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
        )
        // Auth subcommand: approval-token init/show
        .subcommand(
            Command::new("auth")
                .about("Manage the approval token for chat-mediated human assent")
                .subcommand(
                    Command::new("init")
                        .about("Generate and age-encrypt the approval token (refuses raw plaintext age keys)")
                        .arg(
                            Arg::new("recipient")
                                .long("recipient")
                                .help("age recipient (age1...); auto-discovered from --identity if omitted")
                                .required(false),
                        )
                        .arg(
                            Arg::new("identity")
                                .long("identity")
                                .help("Path to age identity file (default: ~/.config/sops/age/keys.txt)")
                                .required(false),
                        )
                        .arg(
                            Arg::new("force")
                                .long("force")
                                .action(ArgAction::SetTrue)
                                .help("Overwrite existing approve.token.{age,hash}"),
                        ),
                )
                .subcommand(
                    Command::new("show")
                        .about("Decrypt and print the approval token (prompts for passphrase / hardware tap)"),
                ),
        )
        // Watch subcommand — POC live dashboard
        .subcommand(
            Command::new("watch")
                .about("Live-tail the substrate: tasks + observations, refreshing in place (POC)")
                .arg(
                    Arg::new("interval")
                        .long("interval")
                        .help("Refresh interval in seconds (default 1.0)")
                        .value_parser(clap::value_parser!(f64))
                        .required(false),
                ),
        )
        // Agents subcommand (parallel to skills; installs flat <name>.md files to .claude/agents/)
        .subcommand(
            Command::new("agents")
                .about("Manage bundled workflow agent system prompts")
                .subcommand(Command::new("list").about("List available bundled agents"))
                .subcommand(
                    Command::new("install")
                        .about("Install a bundled agent into .claude/agents/")
                        .arg(Arg::new("name").help("Agent name").required(false))
                        .arg(
                            Arg::new("all")
                                .long("all")
                                .action(ArgAction::SetTrue)
                                .help("Install all bundled agents"),
                        )
                        .arg(
                            Arg::new("global")
                                .long("global")
                                .action(ArgAction::SetTrue)
                                .help("Install to ~/.claude/agents/ instead of ./.claude/agents/"),
                        ),
                )
                .subcommand(
                    Command::new("uninstall")
                        .about("Remove an installed bundled agent")
                        .arg(
                            Arg::new("name")
                                .help("Agent name")
                                .required(true),
                        )
                        .arg(
                            Arg::new("global")
                                .long("global")
                                .action(ArgAction::SetTrue)
                                .help("Remove from ~/.claude/agents/ instead of ./.claude/agents/"),
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
    // Workflow verb names that are registered separately above — must not be duplicated
    // as lifecycle transition subcommands even if the schema declares them as transition verbs.
    const WORKFLOW_VERBS: &[&str] = &[
        "next-action", "brief", "render", "drive", "status", "guide", "next-id",
        "submit-plan", "submit-plan-review", "submit-execute", "submit-review", "submit-wrap", "resume",
    ];

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
            .subcommand(build_drive_cmd())
            .subcommand(build_status_cmd())
            .subcommand(build_next_id_cmd())
            .subcommand(build_submit_plan_cmd())
            .subcommand(build_submit_plan_review_cmd())
            .subcommand(build_submit_execute_cmd())
            .subcommand(build_submit_review_cmd())
            .subcommand(build_submit_wrap_cmd())
            .subcommand(build_resume_cmd())
            .subcommand(build_guide_cmd());
    }

    // `guide` is also registered on the `gate` store (full form), which has no workflow.
    if schema.name == "gate" {
        store_cmd = store_cmd.subcommand(build_guide_cmd());
    }

    // Register one subcommand per transition verb (de-duplicated against base/workflow verbs)
    let mut registered_verbs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for transition in &schema.lifecycle.transitions {
        let verb = &transition.verb;
        // Skip framework-actor transitions — they are engine-fired and must not appear in user-facing help
        if transition.actor == Some(crate::schema::actor::Actor::Framework) {
            continue;
        }
        // Skip base framework verbs
        if BASE_VERBS.contains(&verb.as_str()) {
            eprintln!(
                "warning: transition verb '{}' in store '{}' collides with a base verb; skipping",
                verb, schema.name
            );
            continue;
        }
        // Skip workflow verbs already registered above (workflow schemas declare these in transitions)
        if schema.workflow.is_some() && WORKFLOW_VERBS.contains(&verb.as_str()) {
            continue;
        }
        // Skip duplicate transition verbs (same verb can appear multiple times for multi-gate transitions)
        if !registered_verbs.insert(verb.clone()) {
            continue;
        }
        let mut transition_cmd = build_transition_cmd(verb, &leaves);
        // `reject` requires a human-supplied reason written to wrap_log[-1].reject_reason.
        // walk_field skips ListRecord fields, so we add --reason manually here.
        if verb == "reject" {
            transition_cmd = transition_cmd.arg(
                Arg::new("reason")
                    .long("reason")
                    .help("Rejection reason (written to wrap_log[-1].reject_reason)")
                    .required(true),
            );
        }
        // close_as_addressed (observations open → resolved) requires a reference
        // to the artifact that addressed the row: a task-id (T###), an
        // observation-id (L###), or a commit-sha (7-40 hex chars). Substrate
        // verifies the shape; the verb refuses without it.
        if verb == "close_as_addressed" {
            // The schema already declares a `resolution` text field, so the
            // leaf-arg walker registered a non-required `--resolution`. Promote
            // it to required and override the help text so the three accepted
            // forms (T###, L###, commit-sha) are visible at --help.
            transition_cmd = transition_cmd.mut_arg("resolution", |a| {
                a.required(true).help(
                    "Reference to the artifact that addressed this observation: \
                     task-id (T###), observation-id (L###), or commit-sha \
                     (7-40 hex chars)",
                )
            });
        }
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

/// Build the `submit-wrap` command.
fn build_submit_wrap_cmd() -> Command {
    Command::new("submit-wrap")
        .about("Submit the wrap agent's synthesis brief (in_review; append-only write to wrap_log[])")
        .arg(Arg::new("display_id").help("Display ID").required(true))
        .arg(
            Arg::new("summary-from-file")
                .long("summary-from-file")
                .help("Load executive_summary from file (use '-' for stdin)")
                .required(true),
        )
        .arg(
            Arg::new("deviations-from-file")
                .long("deviations-from-file")
                .help("Load deviations from file (one per line)")
                .required(false),
        )
        .arg(
            Arg::new("residual-risks-from-file")
                .long("residual-risks-from-file")
                .help("Load residual_risks from file (one per line)")
                .required(false),
        )
        .arg(
            Arg::new("sanity-checks-from-file")
                .long("sanity-checks-from-file")
                .help("Load recommended_sanity_checks from file (one per line)")
                .required(false),
        )
        .arg(
            Arg::new("reasoning-from-file")
                .long("reasoning-from-file")
                .help("Load optional reasoning from file")
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

/// Build the `status` command.
///
/// `stores tasks status <id>` prints a compact workflow telemetry frame.
/// Distinct from `show` (full debug dump): `status` is a live-tail view.
///
/// Flags:
/// - `--follow`           poll in a loop until terminal (AC5.2 / AC5.3)
/// - `--interval <secs>`  poll interval, default 1.5 s
/// - `--max-iters N`      (hidden) cap iterations for tests (AC5.6)
fn build_status_cmd() -> Command {
    Command::new("status")
        .about(
            "Print a compact workflow telemetry frame for a task \
             (use --follow to tail live; distinct from `show` which prints the full row)",
        )
        .arg(
            Arg::new("display_id")
                .help("Task display ID (omit to show all non-terminal tasks)")
                .required(false),
        )
        .arg(
            Arg::new("follow")
                .long("follow")
                .short('f')
                .action(ArgAction::SetTrue)
                .help("Poll in a loop; exit on complete/blocked or Ctrl-C (exit 130)"),
        )
        .arg(
            Arg::new("interval")
                .long("interval")
                .help("Poll interval in seconds (default 1.5)")
                .value_parser(clap::value_parser!(f64))
                .required(false),
        )
        .arg(
            Arg::new("max-iters")
                .long("max-iters")
                .help("Maximum poll iterations (for testing)")
                .value_parser(clap::value_parser!(usize))
                .hide(true)
                .required(false),
        )
}

/// Build the `next-id` command.
///
/// Read-only verb: scans `tasks/{active,planning,paused,completed,archived}/` for the
/// highest `T###` directory and prints the next available ID formatted as `T{:03}`.
/// No flags in v0.3 — callers consume the single-line output via shell substitution.
fn build_next_id_cmd() -> Command {
    Command::new("next-id")
        .about("Print the next available task ID by scanning tasks/{active,planning,paused,completed,archived}/")
}

/// Build the `guide` command.
///
/// Registered on both `gate` (full form: gate row + linked task + authorized verbs)
/// and `tasks` (stub form: task row + next-action + last review, v0.3 quality).
fn build_guide_cmd() -> Command {
    let cmd = Command::new("guide")
        .about(
            "Spawn the guide agent with a curated context bundle \
             (gate form: full context + write-back check; \
             tasks form: v0.3 stub — context only, no write-back)",
        )
        .arg(
            Arg::new("display_id")
                .help("Display ID of the gate or task to guide on")
                .required(true),
        )
        .arg(
            Arg::new("mock")
                .long("mock")
                .help("Path to a JSON fixture file for the mock runner (for testing)")
                .hide(true)
                .required(false),
        );

    #[cfg(feature = "runner-claude-code")]
    let cmd = cmd.arg(
        Arg::new("claude-code")
            .long("claude-code")
            .action(ArgAction::SetTrue)
            .help("Use the claude-code runner (requires runner-claude-code feature)")
            .required(false),
    );

    cmd
}

/// Build the `drive` command.
///
/// Drives a workflow task through the state machine to a terminal state using a
/// runner backend.  `--auto` selects the next non-terminal task by `created_at ASC`
/// (`WHERE status NOT IN ('complete', 'blocked') AND (claimed_by IS NULL OR
/// claimed_at < now - lock_window) ORDER BY created_at ASC LIMIT 1`).
fn build_drive_cmd() -> Command {
    let cmd = Command::new("drive")
        .about(
            "Drive a workflow task to a terminal state via a runner \
             (next-action → brief → spawn → submit-* → render loop)",
        )
        .arg(
            Arg::new("display_id")
                .help("Task display ID to drive (mutually exclusive with --auto)")
                .required(false),
        )
        .arg(
            Arg::new("auto")
                .long("auto")
                .action(ArgAction::SetTrue)
                .help(
                    "Auto-select the next non-complete task by created_at ASC \
                     (skips live-claimed rows within the 5-minute lock window)",
                )
                .required(false),
        )
        .arg(
            Arg::new("max-iters")
                .long("max-iters")
                .help("Maximum loop iterations before aborting (default: 50)")
                .value_parser(clap::value_parser!(usize))
                .required(false),
        )
        .arg(
            Arg::new("mock")
                .long("mock")
                .help("Path to a JSON fixture file for the mock runner (for testing)")
                .hide(true) // AC3.3: always built, hidden from --help
                .required(false),
        );

    // --claude-code only available when the feature is compiled in (AC3.3).
    #[cfg(feature = "runner-claude-code")]
    let cmd = cmd
        .arg(
            Arg::new("claude-code")
                .long("claude-code")
                .action(ArgAction::SetTrue)
                .help("Use the claude-code runner (requires runner-claude-code feature)")
                .required(false),
        )
        .arg(
            Arg::new("testing")
                .long("testing")
                .action(ArgAction::SetTrue)
                .help(
                    "Force all agents to use the haiku model (cheap iteration / \
                     prompt + runner contract smoke testing). Only meaningful with \
                     --claude-code.",
                )
                .required(false),
        );

    cmd
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
            FieldType::Text | FieldType::Timestamp | FieldType::DisplayId | FieldType::Json
        );
        let is_list = matches!(
            leaf.field.ty,
            FieldType::List(_) | FieldType::ListFk { .. } | FieldType::ListRecord(_)
        );

        let mut arg = Arg::new(leaf.cli_name.clone())
            .long(leaf.cli_name.clone())
            .help(
                leaf.field
                    .description
                    .clone()
                    .unwrap_or_else(|| leaf.cli_name.clone()),
            )
            .required(false);

        if is_list {
            arg = arg.action(ArgAction::Append);
        }

        cmd = cmd.arg(arg);

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
///
/// L001: also exposes an optional `--display-id <ID>` flag so callers can
/// pre-allocate the substrate row's display id (e.g. align with a filesystem
/// `next-id` scan). The handler validates the supplied id against the schema's
/// `id_format` and rejects collisions.
fn build_add_cmd(
    leaves: &[crate::schema::flatten::LeafArg<'_>],
    schema: &Schema,
) -> Command {
    let mut cmd = build_leaf_cmd("add", leaves, false);
    cmd = cmd.arg(
        Arg::new("display-id")
            .long("display-id")
            .help(format!(
                "Pre-allocate the row's display id (must match id_format '{}'). \
                 If absent, the substrate auto-mints the next id.",
                schema.id_format
            ))
            .required(false),
    );
    cmd
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
            FieldType::Text | FieldType::Timestamp | FieldType::DisplayId | FieldType::Json
        );
        let is_list = matches!(
            leaf.field.ty,
            FieldType::List(_) | FieldType::ListFk { .. } | FieldType::ListRecord(_)
        );

        // Main arg — clone the String into a Box<str> to satisfy Into<Id>
        let mut arg = Arg::new(leaf.cli_name.clone())
            .long(leaf.cli_name.clone())
            .help(
                leaf.field
                    .description
                    .clone()
                    .unwrap_or_else(|| leaf.cli_name.clone()),
            )
            .required(false);

        if is_list {
            arg = arg.action(ArgAction::Append);
        }

        cmd = cmd.arg(arg);

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
