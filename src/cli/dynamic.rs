use clap::{Arg, ArgAction, Command, ValueHint};
use std::collections::HashMap;

use crate::manifest::Manifest;
use crate::schema::flatten::leaf_args;
use crate::schema::{FieldType, Schema};

// ---------------------------------------------------------------------------
// Bundled stores — embedded at compile time
// ---------------------------------------------------------------------------

/// Names of bundled stores (the subdirectory name == schema name).
pub static BUNDLED_STORE_NAMES: &[&str] = &[
    "observations",
    "gate",
    "tasks",
    "intake",
    "daemon_starts",
    "architecture_reviews",
    "external_reviews",
];

/// Embedded schema.yaml content for each bundled store (same order as BUNDLED_STORE_NAMES).
pub static BUNDLED_STORE_SCHEMAS: &[(&str, &str)] = &[
    (
        "observations",
        include_str!("../../stores/observations/schema.yaml"),
    ),
    ("gate", include_str!("../../stores/gate/schema.yaml")),
    ("tasks", include_str!("../../stores/tasks/schema.yaml")),
    (
        "intake",
        include_str!("../../stores/intake_items/schema.yaml"),
    ),
    (
        "daemon_starts",
        include_str!("../../stores/daemon_starts/schema.yaml"),
    ),
    (
        "architecture_reviews",
        include_str!("../../stores/architecture_reviews/schema.yaml"),
    ),
    (
        "external_reviews",
        include_str!("../../stores/external_reviews/schema.yaml"),
    ),
];

/// Embedded template content for bundled workflow stores.
///
/// Map: store-name → list of (template-relative-path, content).
/// Path keys must match schema's `briefing_templates` and `render_template` values.
/// Used by brief.rs and render.rs when `schema_path` starts with `"bundled:"`.
pub static BUNDLED_STORE_TEMPLATES: &[(&str, &[(&str, &str)])] = &[
    (
        "tasks",
        &[
            (
                "templates/planner-brief.md.tpl",
                include_str!("../../stores/tasks/templates/planner-brief.md.tpl"),
            ),
            (
                "templates/plan-reviewer-brief.md.tpl",
                include_str!("../../stores/tasks/templates/plan-reviewer-brief.md.tpl"),
            ),
            (
                "templates/executor-brief.md.tpl",
                include_str!("../../stores/tasks/templates/executor-brief.md.tpl"),
            ),
            (
                "templates/code-reviewer-brief.md.tpl",
                include_str!("../../stores/tasks/templates/code-reviewer-brief.md.tpl"),
            ),
            (
                "templates/wrap-brief.md.tpl",
                include_str!("../../stores/tasks/templates/wrap-brief.md.tpl"),
            ),
            (
                "templates/main.md.tpl",
                include_str!("../../stores/tasks/templates/main.md.tpl"),
            ),
        ],
    ),
    (
        "intake",
        &[
            (
                "templates/gatekeeper-brief.md.tpl",
                include_str!("../../stores/intake_items/templates/gatekeeper-brief.md.tpl"),
            ),
            (
                "templates/recon-brief.md.tpl",
                include_str!("../../stores/intake_items/templates/recon-brief.md.tpl"),
            ),
        ],
    ),
    (
        "architecture_reviews",
        &[(
            "templates/main.md.tpl",
            include_str!("../../stores/architecture_reviews/templates/main.md.tpl"),
        )],
    ),
    (
        "external_reviews",
        &[(
            "templates/main.md.tpl",
            include_str!("../../stores/external_reviews/templates/main.md.tpl"),
        )],
    ),
];

/// Build the root `stores` Command with all installed stores added as subcommands.
pub fn build_root(manifest: &Manifest, schemas: &HashMap<String, Schema>) -> Command {
    let mut root = Command::new("stores")
        .about("Schema-driven store framework")
        .version(env!("CARGO_PKG_VERSION"))
        .long_version(crate::version::build_identity_diagnostics())
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
            Arg::new("meta")
                .long("meta")
                .global(true)
                .num_args(0..=1)
                .default_missing_value("")
                .require_equals(true)
                .value_hint(ValueHint::DirPath)
                .help(
                    "File this invocation against the META substrate (the substrate's own substrate) \
                     — for substrate self-issue filing only. With a value: --meta=<PATH>. \
                     Without a value: reads STORES_META_PATH. NOT a generic store-routing flag; \
                     for that use --stores-root <PATH>.",
                ),
        )
        .arg(
            Arg::new("stores-root")
                .long("stores-root")
                .global(true)
                .num_args(0..=1)
                .default_missing_value("")
                .require_equals(true)
                .value_hint(ValueHint::DirPath)
                .help(
                    "Route this invocation at the substratum store rooted at PATH (the directory \
                     containing .stores/). Use when running a stores subcommand from a cwd that \
                     is not the store root (e.g. a worktree or adapter-style cwd whose .stores/ \
                     does not exist). For filing META observations/intake/tasks against the \
                     substrate itself, use --meta instead.",
                ),
        )
        .arg(
            Arg::new("approve-token")
                .long("approve-token")
                .global(true)
                .help(
                    "Plaintext approval token for host-bound human assent. \
                     Verified against ~/.config/stores/approve.token.hash via \
                     constant-time compare.",
                ),
        )
        .subcommand(
            Command::new("__llm-off-sentinel")
                .hide(true)
                .about("Internal fake-mode binary capability sentinel"),
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
        // Migrate subcommand — diff installed-store schemas against the live DB
        .subcommand(
            Command::new("migrate")
                .about("Diff installed-store schemas against the live DB and emit additive ALTER TABLE statements (DRY-RUN by default)")
                .arg(
                    Arg::new("apply")
                        .long("apply")
                        .action(ArgAction::SetTrue)
                        .help("Execute the emitted SQL inside a transaction (default is DRY-RUN)"),
                ),
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
                .about("Manage the host-bound approval token")
                .subcommand(
                    Command::new("init")
                        .about("Generate the plaintext approval token at mode 0600")
                        .arg(
                            Arg::new("force")
                                .long("force")
                                .action(ArgAction::SetTrue)
                                .help("Overwrite existing approve.token or approve.token.hash"),
                        ),
                )
                .subcommand(Command::new("show").about("Print the plaintext approval token")),
        )
        // Metrics subcommand — transition_history throughput/read-surface report
        .subcommand(
            Command::new("metrics")
                .about("Report transition_history throughput metrics (--json or --text)")
                .arg(
                    Arg::new("window")
                        .long("window")
                        .help("Metrics window: duration like 1h/30m or RFC3339 timestamp")
                        .required(true),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Render JSON output (default)"),
                )
                .arg(
                    Arg::new("text")
                        .long("text")
                        .action(ArgAction::SetTrue)
                        .help("Render stable text output instead of JSON"),
                )
                .arg(
                    Arg::new("now")
                        .long("now")
                        .value_hint(ValueHint::Other)
                        .help("Override wall-clock 'now' for duration windows (RFC3339); makes output deterministic"),
                ),
        )
        // Runner/model telemetry rollup
        .subcommand(
            Command::new("runner-stats")
                .about("Summarize raw agent_runs telemetry by role, harness, model, and thinking effort")
                .after_help("Warning: raw operational telemetry only. Rows marked payload_valid=0 are excluded by default when available; use --include-dirty-data to include them. Do not treat this output as statistical inference.")
                .arg(
                    Arg::new("display_id")
                        .long("display-id")
                        .help("Restrict stats to a single task display ID")
                        .required(false),
                )
                .arg(
                    Arg::new("role")
                        .long("role")
                        .help("Restrict stats to a single agent role")
                        .required(false),
                )
                .arg(
                    Arg::new("harness")
                        .long("harness")
                        .help("Restrict stats to a single harness ID")
                        .required(false),
                )
                .arg(
                    Arg::new("model")
                        .long("model")
                        .help("Restrict stats to a single model ID")
                        .required(false),
                )
                .arg(
                    Arg::new("thinking")
                        .long("thinking")
                        .help("Restrict stats to a single effective thinking effort")
                        .required(false),
                )
                .arg(
                    Arg::new("since")
                        .long("since")
                        .help("Restrict stats to runs with started_at at or after this RFC3339 timestamp")
                        .required(false),
                )
                .arg(
                    Arg::new("until")
                        .long("until")
                        .help("Restrict stats to runs with started_at at or before this RFC3339 timestamp")
                        .required(false),
                )
                .arg(
                    Arg::new("include_dirty_data")
                        .long("include-dirty-data")
                        .action(ArgAction::SetTrue)
                        .help("Include rows marked payload_valid=0; excluded by default when that column exists"),
                )
                .arg(
                    Arg::new("json")
                        .long("json")
                        .action(ArgAction::SetTrue)
                        .help("Render JSON output"),
                ),
        )
        // Engine operator diagnostics
        .subcommand(
            Command::new("engine")
                .about("Operator diagnostics for engine startup and dispatch health")
                .subcommand(
                    Command::new("locks")
                        .about("Classify dispatch_locks as live, retrying, stale, orphaned, or failed")
                        .arg(
                            Arg::new("json")
                                .long("json")
                                .action(ArgAction::SetTrue)
                                .help("Render JSON output"),
                        ),
                )
                .subcommand(
                    Command::new("plan-start")
                        .about("Print the ignition plan: classify every tasks row into would-run, inactive, needs-operator, blocked, historical")
                        .arg(
                            Arg::new("json")
                                .long("json")
                                .action(ArgAction::SetTrue)
                                .help("Render JSON output"),
                        ),
                ),
        )

        // ResourceLock primitive CLI
        .subcommand(
            Command::new("resource-locks")
                .about("Acquire, release, list, and recover ResourceLock rows")
                .subcommand(
                    Command::new("acquire")
                        .about("Acquire a ResourceLock")
                        .arg(Arg::new("resource").long("resource").required(true))
                        .arg(Arg::new("owner").long("owner").required(true))
                        .arg(Arg::new("owner-kind").long("owner-kind").value_parser(["task", "job"]).required(true))
                        .arg(Arg::new("ttl-secs").long("ttl-secs").value_parser(clap::value_parser!(u64)))
                        .arg(Arg::new("claim-source").long("claim-source")),
                )
                .subcommand(
                    Command::new("release")
                        .about("Release a ResourceLock")
                        .arg(Arg::new("resource").long("resource").required(true))
                        .arg(Arg::new("token").long("token").required(true)),
                )
                .subcommand(Command::new("list").about("List ResourceLock rows"))
                .subcommand(Command::new("recover-stale").about("Recover expired ResourceLock rows")),
        )
        // Runs transcript index/query surface
        .subcommand(
            Command::new("runs")
                .about("List and show .stores/runs transcripts for a task")
                .subcommand(
                    Command::new("list")
                        .about("List cycle-linked transcripts for a task")
                        .arg(Arg::new("display_id").help("Task display ID").required(true)),
                )
                .subcommand(
                    Command::new("show")
                        .about("Print one cycle-linked transcript")
                        .arg(Arg::new("display_id").help("Task display ID").required(true))
                        .arg(
                            Arg::new("phase")
                                .long("phase")
                                .help("Phase number")
                                .value_parser(clap::value_parser!(i64))
                                .required(true),
                        )
                        .arg(
                            Arg::new("cycle")
                                .long("cycle")
                                .help("Cycle number (optional; required only when ambiguous)")
                                .value_parser(clap::value_parser!(i64))
                                .required(false),
                        )
                        .arg(
                            Arg::new("role")
                                .long("role")
                                .help("Agent role")
                                .required(true),
                        ),
                )
                .subcommand(
                    Command::new("current")
                        .about("Show the current live runner marker for a task")
                        .arg(Arg::new("display_id").help("Task display ID").required(true))
                        .arg(Arg::new("role").long("role").help("Agent role")),
                )
                .subcommand(
                    Command::new("tail")
                        .about("Print the current live runner log for a task")
                        .arg(Arg::new("display_id").help("Task display ID").required(true))
                        .arg(Arg::new("role").long("role").help("Agent role"))
                        .arg(
                            Arg::new("raw")
                                .long("raw")
                                .help("Print the live raw stdout transcript")
                                .action(clap::ArgAction::SetTrue),
                        )
                        .arg(
                            Arg::new("stderr")
                                .long("stderr")
                                .help("Print the live stderr log")
                                .action(clap::ArgAction::SetTrue),
                        ),
                )
                .subcommand(
                    Command::new("gc")
                        .about("Dry-run or execute .stores/runs transcript garbage collection")
                        .arg(
                            Arg::new("dry-run")
                                .long("dry-run")
                                .help("Report GC candidates without mutating files (default)")
                                .action(clap::ArgAction::SetTrue),
                        )
                        .arg(
                            Arg::new("execute")
                                .long("execute")
                                .help("Replace selected transcript/log files with tombstones")
                                .action(clap::ArgAction::SetTrue),
                        )
                        .arg(
                            Arg::new("max-bytes")
                                .long("max-bytes")
                                .help("Total .stores/runs cap before GC stops (default 20G)"),
                        )
                        .arg(
                            Arg::new("warn-bytes")
                                .long("warn-bytes")
                                .help("Warn when .stores/runs exceeds this size (default 10G)"),
                        )
                        .arg(
                            Arg::new("per-file-warn-bytes")
                                .long("per-file-warn-bytes")
                                .help("Warn on individual run files above this size (default 1G)"),
                        )
                        .arg(
                            Arg::new("largest")
                                .long("largest")
                                .help("Number of largest files to report (default 20)")
                                .value_parser(clap::value_parser!(usize)),
                        ),
                ),
        )
        // Watch subcommand — ratatui TUI (T028); --legacy falls back to ANSI POC
        .subcommand(
            Command::new("watch")
                .about("Live-tail the substrate: tasks + observations, refreshing in place")
                .arg(
                    Arg::new("interval")
                        .long("interval")
                        .help("Refresh interval in seconds (default 1.0)")
                        .value_parser(clap::value_parser!(f64))
                        .required(false),
                )
                .arg(
                    Arg::new("state")
                        .long("state")
                        .help("Filter rows by state (e.g. executing, plan_review)")
                        .required(false),
                )
                .arg(
                    Arg::new("priority")
                        .long("priority")
                        .help("Filter rows by priority (high|normal|low)")
                        .required(false),
                )
                .arg(
                    Arg::new("tier")
                        .long("tier")
                        .help("Filter rows by tier hint (T0|T1|T2|T3)")
                        .required(false),
                )
                .arg(
                    Arg::new("since")
                        .long("since")
                        .help("Filter rows updated since (ISO timestamp or duration like 1h)")
                        .required(false),
                )
                .arg(
                    Arg::new("all")
                        .long("all")
                        .action(ArgAction::SetTrue)
                        .help("Show the full watch surface, including historical noise"),
                )
                .arg(
                    Arg::new("all-history")
                        .long("all-history")
                        .action(ArgAction::SetTrue)
                        .help("Show the full watch surface, including historical noise (legacy alias)"),
                )
                .arg(
                    Arg::new("legacy")
                        .long("legacy")
                        .action(ArgAction::SetTrue)
                        .help("Use the legacy ANSI POC instead of the ratatui TUI"),
                ),
        )
        // Topology subcommand — static schematic of stores, state machines, and workflow firing order
        .subcommand(
            Command::new("topology")
                .about("Print a static schematic of stores, per-store state machines, and the tasks workflow firing order")
                .arg(
                    Arg::new("format")
                        .long("format")
                        .help("Output format: auto (dot via graphviz, falls back to source) | dot | mermaid")
                        .required(false),
                )
                .arg(
                    Arg::new("store")
                        .long("store")
                        .help("Filter Z1/Z2 to a single store (Z0 still shows the whole graph)")
                        .required(false),
                )
                .arg(
                    Arg::new("no-icons")
                        .long("no-icons")
                        .action(ArgAction::SetTrue)
                        .help("Disable Nerd Font glyphs; use text codes (A / H+ / H! / F)"),
                ),
        )
        // Agents subcommand (parallel to skills; installs flat <name>.md files to .claude/agents/)
        .subcommand(
            Command::new("agents")
                .about("Manage bundled workflow agent system prompts")
                .subcommand(Command::new("list").about("List available bundled agents"))
                .subcommand(
                    Command::new("stop")
                        .about("Stop the detached agents daemon for this project")
                        .arg(
                            Arg::new("force")
                                .long("force")
                                .action(ArgAction::SetTrue)
                                .help(
                                    "If the daemon does not exit within the graceful timeout, \
                                     escalate to SIGKILL. Default: error on timeout without killing.",
                                ),
                        ),
                )
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
                )
                // Phase 4: autonomous-flow daemon
                .subcommand(
                    Command::new("run")
                        .about("Run the autonomous-flow daemon: poll subscribed transitions, claim, dispatch")
                        .arg(
                            Arg::new("poll-interval")
                                .long("poll-interval")
                                .help("Poll interval in seconds (default: 5)")
                                .value_parser(clap::value_parser!(f64))
                                .required(false),
                        )
                        .arg(
                            Arg::new("detach")
                                .long("detach")
                                .action(ArgAction::SetTrue)
                                .help("Daemonize (fork + setsid); parent prints child PID and exits"),
                        )
                        .arg(
                            Arg::new("log-file")
                                .long("log-file")
                                .help("Redirect stdout/stderr to this file (required with --detach)")
                                .required(false),
                        )
                        .arg(
                            Arg::new("max-iters")
                                .long("max-iters")
                                .help("Maximum poll iterations before exiting (for testing)")
                                .value_parser(clap::value_parser!(usize))
                                .hide(true)
                                .required(false),
                        )
                        .arg(
                            Arg::new("once")
                                .long("once")
                                .action(ArgAction::SetTrue)
                                .help("Run a single poll iteration (no daemonize, no loop) and exit"),
                        ),
                )
                // Phase 4: backfill placeholder (impl in Phase 7)
                .subcommand(
                    Command::new("backfill")
                        .about("One-off scan for accepted-but-unmerged rows; applies accept-merge sequentially (Phase 7)"),
                )
                .subcommand(
                    Command::new("telemetry-backfill")
                        .about("Backfill missing historical Pi agent_run telemetry from transcript JSONL"),
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
        "next-action",
        "brief",
        "render",
        "drive",
        "status",
        "guide",
        "next-id",
        "submit-plan",
        "submit-plan-review",
        "submit-execute",
        "submit-review",
        "submit-wrap",
        "resume",
        "retry-deploy",
    ];

    let mut store_cmd =
        Command::new(schema.name.clone()).about(format!("Operate on the '{}' store", schema.name));
    if schema.name == "architecture_reviews" {
        store_cmd = store_cmd
            .visible_alias("architecture-reviews")
            .disable_help_subcommand(true)
            .subcommand(add_cmd)
            .subcommand(list_cmd)
            .subcommand(show_cmd);
    } else {
        store_cmd = store_cmd
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
                .subcommand(build_retry_deploy_cmd())
                .subcommand(build_guide_cmd());
        }
    }

    // recover-stale-base is a tasks-only operator recovery verb.
    if schema.name == "tasks" {
        store_cmd = store_cmd.subcommand(build_recover_stale_base_cmd());
        store_cmd = store_cmd.subcommand(build_reconcile_accepted_cmd());
        store_cmd = store_cmd.subcommand(build_enqueue_integration_cmd());
        store_cmd = store_cmd.subcommand(build_run_integration_cmd());
        store_cmd = store_cmd.subcommand(build_cleanup_worktrees_cmd());
        // T140 P1: activation primitive — `tasks activate` / `tasks deactivate`.
        store_cmd = store_cmd
            .subcommand(build_activate_cmd())
            .subcommand(build_deactivate_cmd());
    }

    // external_reviews run is a narrow operator-control verb: dispatch exactly
    // one review row without daemon startup sweeps/watchdog/engine-runner.
    if schema.name == "external_reviews" {
        store_cmd = store_cmd
            .subcommand(build_external_review_run_cmd())
            .subcommand(build_external_review_create_pending_cmd())
            .subcommand(build_external_review_import_pass_cmd());
    }

    // `guide` is also registered on the `gate` store (full form), which has no workflow.
    if schema.name == "gate" {
        store_cmd = store_cmd.subcommand(build_guide_cmd());
    }

    // next-id, override-risk, override-policy, clusters, and overdue-ready are
    // observations-only non-workflow verbs.
    if schema.name == "observations" {
        store_cmd = store_cmd
            .subcommand(build_next_id_cmd())
            .subcommand(build_override_risk_cmd())
            .subcommand(build_override_policy_cmd())
            .subcommand(
                Command::new("clusters")
                    .about("Group open/ready observations by curated cluster_key (single-shot)")
                    .arg(
                        Arg::new("json")
                            .long("json")
                            .action(ArgAction::SetTrue)
                            .help("Emit JSON output"),
                    ),
            )
            .subcommand(
                Command::new("overdue-ready")
                    .about(
                        "List ready observations whose linked task is in a terminal-success state",
                    )
                    .arg(
                        Arg::new("json")
                            .long("json")
                            .action(ArgAction::SetTrue)
                            .help("Emit JSON output"),
                    ),
            );
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
        let mut transition_cmd = if schema.name == "architecture_reviews" {
            build_architecture_review_transition_cmd(verb, &leaves)
        } else {
            build_transition_cmd(verb, &leaves)
        };
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
        // `abandon` (T043) requires a human-supplied reason written to the
        // top-level `abandoned_reason` field. The schema declares
        // `abandoned_reason` as a non-required leaf, so we add a separate
        // `--reason` flag here that the dispatcher routes into run_abandon.
        if verb == "abandon" {
            transition_cmd = transition_cmd.arg(
                Arg::new("reason")
                    .long("reason")
                    .value_name("text")
                    .help("Why this row is being abandoned (written to abandoned_reason)")
                    .required(true),
            );
        }
        // close_as_addressed (observations open → resolved) requires a reference
        // to the artifact that addressed the row: a task-id (T###), an
        // observation-id (L###), or a commit-sha (7-40 hex chars). Substrate
        // verifies the shape; the verb refuses without it.
        if verb == "close_as_addressed" {
            // The schema usually registers a non-required `--resolution` leaf.
            // Some transition-specific leaf filtering can omit it; keep command
            // construction total by adding the verb-required flag in that case.
            let resolution_help = "Reference to the artifact that addressed this observation: \
                     task-id (T###), observation-id (L###), or commit-sha \
                     (7-40 hex chars)";
            if transition_cmd
                .get_arguments()
                .any(|a| a.get_id() == "resolution")
            {
                transition_cmd = transition_cmd
                    .mut_arg("resolution", |a| a.required(true).help(resolution_help));
            } else {
                transition_cmd = transition_cmd.arg(
                    Arg::new("resolution")
                        .long("resolution")
                        .value_name("ref")
                        .help(resolution_help)
                        .required(true),
                );
            }
        }
        // close-out-of-band (tasks recovery-terminal) requires --commit <SHA>:
        // the merge-target SHA recorded as provenance in transition_history.
        if verb == "close-out-of-band" {
            transition_cmd = transition_cmd.arg(
                Arg::new("commit")
                    .long("commit")
                    .help(
                        "Merge-target git SHA (7-40 hex chars) reachable from main; \
                         recorded in transition_history as provenance",
                    )
                    .required(true),
            );
        }
        store_cmd = store_cmd.subcommand(transition_cmd);
    }

    if schema.name == "architecture_reviews" {
        store_cmd = store_cmd.subcommand(build_render_cmd());
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
        .about(
            "Submit the wrap agent's synthesis brief (in_review; append-only write to wrap_log[])",
        )
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
        .about("Resume blocked work-cycle execution (blocked → ready/planning; not deploy_blocked recovery)")
        .arg(Arg::new("display_id").help("Display ID").required(true))
        .arg(
            Arg::new("summary")
                .long("summary")
                .help("Optional reason for resuming blocked work-cycle execution")
                .required(false),
        )
        .arg(
            Arg::new("no-dispatch")
                .long("no-dispatch")
                .help("Recovery only: repair blocked task state but skip immediate on-entry dispatch/follow-ons and leave activation inactive for orphaned-result/manual reconciliation workflows")
                .action(clap::ArgAction::SetTrue)
                .required(false),
        )
}

/// Build the `retry-deploy` command.
fn build_retry_deploy_cmd() -> Command {
    Command::new("retry-deploy")
        .about("Retry deploy-blocked release ceremony (deploy_blocked → accepted; use after fixing deploy issue)")
        .arg(Arg::new("display_id").help("Display ID").required(true))
}

/// Build the `reconcile-accepted` command (tasks only).
///
/// I027 / T107 operator-grounded recovery verb (T138 P3 update): re-fire the
/// post-`integrated` chain (cargo-install → schema-migrate) for a row stranded
/// at `integrated` (or mid-chain `cargo_installed`) whose branch is already
/// merged to main. The integration lane (T138) owns the merge step now, so
/// this verb no longer drives accept-merge. Allowed only from
/// {integrated, cargo_installed}. Requires ai_with_human or human invoker.
fn build_reconcile_accepted_cmd() -> Command {
    Command::new("reconcile-accepted")
        .about(
            "Reconcile a task stranded at status='integrated' (or mid-chain 'cargo_installed') \
             whose branch is already merged to main but whose stores-specific post-`integrated` \
             chain (cargo-install / schema-migrate) never fired. Re-fires cargo-install and \
             schema-migrate via direct builtin invocation; the framework actor fires the \
             mark_cargo_installed and mark_schema_migrated transitions normally. The integration \
             lane (T138) owns the merge step, so this verb does NOT drive accept-merge. Requires \
             --invoker ai_with_human or human; ai_autonomous is rejected. Fails loud if the \
             branch is not merged to main (use `tasks retry-integration` from integration_blocked \
             for pre-integrated recovery).",
        )
        .arg(
            Arg::new("display_id")
                .help("Display ID of the task (T###)")
                .required(true),
        )
}

/// Build the `enqueue-integration` command (tasks only).
///
/// Operator recovery for rows accepted while the daemon/on-entry engine was off:
/// fire the framework-owned `accepted → integration_queued` transition without
/// granting arbitrary framework authority at the CLI.
fn build_enqueue_integration_cmd() -> Command {
    Command::new("enqueue-integration")
        .about(
            "Recovery: enqueue an accepted active task into the integration lane \
             (accepted → integration_queued) when the engine was off and the \
             framework on-entry transition did not fire.",
        )
        .arg(
            Arg::new("display_id")
                .help("Display ID of the accepted task (T###)")
                .required(true),
        )
}

/// Build the `run-integration` command (tasks only).
///
/// Operator recovery for an `integration_queued` row when the daemon is off or
/// historical seeding skipped the just-created integration edge.
fn build_run_integration_cmd() -> Command {
    Command::new("run-integration")
        .about(
            "Recovery: run builtin:integrate once for an integration_queued task \
             without starting the full agents daemon.",
        )
        .arg(
            Arg::new("display_id")
                .help("Display ID of the integration_queued task (T###)")
                .required(true),
        )
}

/// Build the `external_reviews create-pending` command.
fn build_external_review_create_pending_cmd() -> Command {
    Command::new("create-pending")
        .about("Create a fresh pending external review for a task head (operator recovery)")
        .arg(Arg::new("task_id").help("Task display ID").required(true))
        .arg(Arg::new("base-sha").long("base-sha").required(true))
        .arg(Arg::new("head-sha").long("head-sha").required(true))
}

/// Build the `external_reviews import-pass` command.
fn build_external_review_import_pass_cmd() -> Command {
    Command::new("import-pass")
        .about("Import an already completed manual external-review PASS for the current task head")
        .arg(Arg::new("task_id").help("Task display ID").required(true))
        .arg(
            Arg::new("transcript-path")
                .long("transcript-path")
                .required(true),
        )
        .arg(Arg::new("base-sha").long("base-sha").required(true))
        .arg(Arg::new("head-sha").long("head-sha").required(true))
        .arg(
            Arg::new("runner")
                .long("runner")
                .required(true)
                .value_parser(["manual-codex", "manual"]),
        )
}

/// Build the `external_reviews run` command.
fn build_external_review_run_cmd() -> Command {
    Command::new("run")
        .about("Run exactly one external_review row without daemon startup sweeps/watchdog/engine-runner")
        .arg(Arg::new("display_id").help("ER### display ID").required(true))
}

/// Build the `cleanup-worktrees` command (tasks only).
fn build_cleanup_worktrees_cmd() -> Command {
    Command::new("cleanup-worktrees")
        .about("Audit and safely clean terminal task worktree build artifacts")
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .action(ArgAction::SetTrue)
                .help("Print candidates and reclaim estimates without deleting anything"),
        )
        .arg(
            Arg::new("execute")
                .long("execute")
                .action(ArgAction::SetTrue)
                .help("Execute an explicit cleanup action"),
        )
        .arg(
            Arg::new("targets-only")
                .long("targets-only")
                .action(ArgAction::SetTrue)
                .requires("execute")
                .conflicts_with("remove-clean")
                .help("With --execute, delete only terminal worktree target/ directories"),
        )
        .arg(
            Arg::new("remove-clean")
                .long("remove-clean")
                .action(ArgAction::SetTrue)
                .requires("execute")
                .conflicts_with("targets-only")
                .help("With --execute, remove clean, merged terminal worktrees without --force"),
        )
}

/// Build the `recover-stale-base` command (tasks only).
///
/// Operator recovery verb: after rebase, supersedes all tooling_held
/// stale_base_requires_rebase ER rows for the task and spawns one fresh
/// pending external_review against the current branch tip. Requires
/// ai_with_human or human invoker; ai_autonomous is rejected.
fn build_recover_stale_base_cmd() -> Command {
    Command::new("recover-stale-base")
        .about(
            "Recover a task stuck in stale_base_requires_rebase: after rebase of the task \
             branch, supersede all tooling_held ER rows and spawn a fresh pending \
             external_review against the current branch tip. Requires --invoker ai_with_human \
             or human; ai_autonomous is rejected. The daemon picks up the new pending ER \
             and runs the normal external_review supersede path.",
        )
        .arg(
            Arg::new("display_id")
                .help("Display ID of the task (T###)")
                .required(true),
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
/// Read-only verb: prints the next available display ID as a single line.
fn build_next_id_cmd() -> Command {
    Command::new("next-id").about("Print the next available display ID")
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
        )
        .arg(
            Arg::new("claude-code-model")
                .long("claude-code-model")
                .help("Force a Claude Code model for all roles (e.g. sonnet, opus)")
                .required(false),
        );

    #[cfg(feature = "runner-pi")]
    let cmd = cmd.arg(
        Arg::new("pi")
            .long("pi")
            .action(ArgAction::SetTrue)
            .help("Use the Pi SDK runner (requires runner-pi feature)")
            .required(false),
    );

    cmd
}

/// Build a transition verb subcommand: positional display_id + all leaf args.
fn build_transition_cmd(verb: &str, leaves: &[crate::schema::flatten::LeafArg<'_>]) -> Command {
    build_leaf_cmd_owned(verb.to_string(), leaves, true)
}

/// Build architecture_reviews transition commands with operator-facing help.
fn build_architecture_review_transition_cmd(
    verb: &str,
    leaves: &[crate::schema::flatten::LeafArg<'_>],
) -> Command {
    let mut cmd = match verb {
        "issue-verdict" => build_leaf_cmd_owned(verb.to_string(), leaves, true).about(
            "Issue an architecture review verdict. Requires actor ai_with_human. \
             --kind is interpret|amend; --verdict is allow_local_fix|reframe_contract|\
             merge_with_cluster|create_primitive_task|block_pending_fixes|\
             propose_doctrine_update|request_human_arch_decision. Amend verdicts require \
             cascade_decisions as a JSON array of {target,decision,rationale?} and move to \
             awaiting_human_ratification.",
        ),
        "ratify-amend" => Command::new(verb.to_string())
            .about(
                "Ratify an amend architecture review. Requires actor human plus a valid \
                 tier-A --approve-token; ai_autonomous and ai_with_human are rejected even \
                 with a valid token. Moves awaiting_human_ratification to verdict_issued.",
            )
            .arg(
                Arg::new("display_id")
                    .help("Display ID of the architecture review (A###)")
                    .required(true),
            ),
        "supersede" => Command::new(verb.to_string())
            .about(
                "Mark an architecture review superseded. Requires actor ai_with_human. \
                 New rulings can also set --supersedes A### during issue-verdict to \
                 transition the prior ruling to the superseded terminal.",
            )
            .arg(
                Arg::new("display_id")
                    .help("Display ID of the architecture review (A###)")
                    .required(true),
            ),
        _ => build_transition_cmd(verb, leaves),
    };

    if verb == "issue-verdict" {
        cmd = cmd.mut_arg("cascade-decisions", |a| {
            a.help(
                "Required for --kind amend: JSON array of objects, e.g. \
                 '[{\"target\":\"docs/heart-and-architect.md\",\"decision\":\"update\",\
                 \"rationale\":\"why\"}]'. decision must be keep|update|supersede|\
                 withdraw|create_followup; strict overlap validation is deferred.",
            )
        });
    }
    cmd
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
        if is_reserved(&leaf.cli_name)
            || leaf.field.actor == Some(crate::schema::actor::Actor::Framework)
        {
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
        let is_plain_list = matches!(leaf.field.ty, FieldType::List(_));

        let mut help = leaf
            .field
            .description
            .clone()
            .unwrap_or_else(|| leaf.cli_name.clone());
        if is_plain_list {
            help.push_str(
                " (repeat flag or comma-separate values; escape commas/backslashes with backslash; empty value sets [])",
            );
        } else if is_list {
            help.push_str(" (repeat flag for multiple values)");
        }

        let mut arg = Arg::new(leaf.cli_name.clone())
            .long(leaf.cli_name.clone())
            .help(help)
            .required(false);

        if is_list {
            arg = arg.action(ArgAction::Append);
        }
        if is_plain_list {
            arg = arg.value_name("VALUE[,VALUE]");
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
fn build_add_cmd(leaves: &[crate::schema::flatten::LeafArg<'_>], schema: &Schema) -> Command {
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
    // T140 P2: --activate shorthand on `tasks add`. Without the flag, newly
    // created task rows land at activation='inactive' (schema default); with
    // --activate they land at 'active'. The activation field's actor gate
    // (ai_with_human) means --activate combined with --invoker ai_autonomous
    // is rejected fail-loud by the validator. No new authority gate here —
    // the schema is the enforcement surface.
    if schema.name == "architecture_reviews" {
        cmd = cmd.arg(
            Arg::new("linked-observations")
                .long("linked-observations")
                .action(ArgAction::Append)
                .value_delimiter(',')
                .value_name("L###[,L###]")
                .help("Comma-separated or repeated L### ids covered by this architecture review; source_observation is included if supplied")
                .required(false),
        );
    }
    if schema.name == "tasks" {
        cmd = cmd.arg(
            Arg::new("activate")
                .long("activate")
                .action(ArgAction::SetTrue)
                .help(
                    "Mint the task at activation='active' (combustion-ready). \
                     Without this flag, the row lands at activation='inactive' \
                     and must be armed via `tasks activate <id> --reason <text>`. \
                     Requires --invoker ai_with_human or --invoker human; the \
                     activation field's actor gate rejects ai_autonomous.",
                )
                .required(false),
        );
    }
    // T013 P2: --lock-contract shorthand on observations add. Atomically
    // finalises a drafted intent_contract at add time: sets
    // intent_contract.contract_state = "ready", auto-fills drafted_at/
    // approved_at/approved_by where permitted, and rejects ai_autonomous.
    if schema.name == "observations" {
        cmd = cmd.arg(
            Arg::new("lock-contract")
                .long("lock-contract")
                .action(ArgAction::SetTrue)
                .help(
                    "Finalise the intent_contract atomically: sets contract_state=ready \
                     and auto-fills drafted_at/approved_at/approved_by. Requires --invoker \
                     human (or --invoker ai_with_human --approve-token <T>); rejects \
                     ai_autonomous. All required contract sub-fields must be supplied.",
                )
                .required(false),
        );
        cmd = cmd.arg(
            Arg::new("acceptance-from-file")
                .long("acceptance-from-file")
                .help(
                    "Load 'acceptance' list from a file (one criterion per line; use '-' for stdin)",
                )
                .required(false),
        );
    }
    cmd
}

/// Build the `update` command with leaf args + positional display_id.
fn build_update_cmd(leaves: &[crate::schema::flatten::LeafArg<'_>], schema: &Schema) -> Command {
    let mut cmd = build_leaf_cmd("update", leaves, true);
    if schema.name == "observations" {
        cmd = cmd.arg(
            Arg::new("acceptance-from-file")
                .long("acceptance-from-file")
                .help(
                    "Load 'acceptance' list from a file (one criterion per line; use '-' for stdin)",
                )
                .required(false),
        );
    }
    cmd
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
        // Skip reserved names AND framework-only fields. Framework fields are
        // populated by handler code (claimed_by, drive_pid, abandoned_reason
        // via run_abandon, etc.) and exposing them on generic add/update is
        // misleading even when validation rejects the write.
        if is_reserved(&leaf.cli_name)
            || leaf.field.actor == Some(crate::schema::actor::Actor::Framework)
        {
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

        let is_plain_list = matches!(leaf.field.ty, FieldType::List(_));

        // Main arg — clone the String into a Box<str> to satisfy Into<Id>
        let mut help = leaf
            .field
            .description
            .clone()
            .unwrap_or_else(|| leaf.cli_name.clone());
        if is_plain_list {
            help.push_str(
                " (repeat flag or comma-separate values; escape commas/backslashes with backslash; empty value sets [])",
            );
        } else if is_list {
            help.push_str(" (repeat flag for multiple values)");
        }

        let mut arg = Arg::new(leaf.cli_name.clone())
            .long(leaf.cli_name.clone())
            .help(help)
            .required(false);

        if is_list {
            arg = arg.action(ArgAction::Append);
        }
        if is_plain_list {
            arg = arg.value_name("VALUE[,VALUE]");
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
        .arg(
            Arg::new("field")
                .long("field")
                .value_name("name")
                .help("Print a selected field/path from the entry (for example: title or contract.done_when)")
                .required(false),
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

    let mut cmd = Command::new("list")
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
                .help(format!(
                    "Order by column ascending. Valid columns: {cols_help}"
                ))
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
        );

    // Observations-only: source tuple filters + --risk-flag <FLAG> (repeatable, AND semantics)
    if schema.name == "observations" {
        cmd = cmd
            .arg(
                Arg::new("source-env")
                    .long("source-env")
                    .help("Filter by canonical source_env (prod|sandbox); pair with --source-id when selecting one source row")
                    .required(false),
            )
            .arg(
                Arg::new("source-id")
                    .long("source-id")
                    .help("Filter by canonical source_id; pair with --source-env when selecting one source row")
                    .required(false),
            )
            .arg(
                Arg::new("prod-source-id")
                    .long("prod-source-id")
                    .help("DEPRECATED filter alias for --source-env prod --source-id <ID>")
                    .required(false),
            )
            .arg(
                Arg::new("sandbox-source-id")
                    .long("sandbox-source-id")
                    .help("DEPRECATED filter alias for --source-env sandbox --source-id <ID>")
                    .required(false),
            )
            .arg(
                Arg::new("origin-db")
                    .long("origin-db")
                    .help("DEPRECATED filter alias for --source-env <prod|sandbox>")
                    .required(false),
            )
            .arg(
                Arg::new("risk-flag")
                    .long("risk-flag")
                    .action(ArgAction::Append)
                    .help(
                        "Filter rows whose risk_flags array contains FLAG (repeatable; multiple = AND). \
                         Must be one of the 13 canonical risk flag values.",
                    )
                    .required(false),
            );
    }

    cmd
}

/// Build the `schema` command.
fn build_schema_cmd() -> Command {
    Command::new("schema").about("Print the schema for this store")
}

/// Build `tasks activate` command (T140 P1, tasks only).
/// Flips activation to 'active'. Requires --reason (recorded as actor_note).
fn build_activate_cmd() -> Command {
    Command::new("activate")
        .about(
            "Arm a tasks row for combustion (sets activation='active'). \
             Requires --reason; tier-B (--invoker ai_with_human or human; \
             ai_autonomous is rejected).",
        )
        .arg(
            Arg::new("display_id")
                .help("Display ID of the task")
                .required(true),
        )
        .arg(
            Arg::new("reason")
                .long("reason")
                .value_name("text")
                .help(
                    "Required reason for arming the row \
                     (recorded in transition_history.actor_note)",
                )
                .required(true),
        )
}

/// Build `tasks deactivate` command (T140 P1, tasks only).
/// Flips activation to 'inactive'. Requires --reason (recorded as actor_note).
fn build_deactivate_cmd() -> Command {
    Command::new("deactivate")
        .about(
            "Disarm a tasks row (sets activation='inactive'). \
             Requires --reason; tier-B (--invoker ai_with_human or human; \
             ai_autonomous is rejected).",
        )
        .arg(
            Arg::new("display_id")
                .help("Display ID of the task")
                .required(true),
        )
        .arg(
            Arg::new("reason")
                .long("reason")
                .value_name("text")
                .help(
                    "Required reason for disarming the row \
                     (recorded in transition_history.actor_note)",
                )
                .required(true),
        )
}

/// Build `override-risk` command (observations only).
/// Accepts --risk-class, --risk-flags, --cluster-key, --reason (required).
fn build_override_risk_cmd() -> Command {
    Command::new("override-risk")
        .about("Override risk metadata (risk_class, risk_flags, cluster_key) on an observation. Tier-B: requires ai_with_human.")
        .arg(
            Arg::new("display_id")
                .help("Display ID of the observation")
                .required(true),
        )
        .arg(
            Arg::new("reason")
                .long("reason")
                .help("Required reason for the override (recorded in transition_history.actor_note)")
                .required(true),
        )
        .arg(
            Arg::new("risk-class")
                .long("risk-class")
                .help("New risk_class value (low|normal|architecture|security|authority)")
                .required(false),
        )
        .arg(
            Arg::new("risk-flags")
                .long("risk-flags")
                .action(ArgAction::Append)
                .help("Risk flag to add (repeat flag or comma-separate values; escape commas/backslashes with backslash; canonical values from docs/risk-and-cluster-taxonomy.md)")
                .required(false),
        )
        .arg(
            Arg::new("cluster-key")
                .long("cluster-key")
                .help("New cluster_key value")
                .required(false),
        )
}

/// Build `override-policy` command (observations only).
/// Accepts --approval-policy, --reason (required).
fn build_override_policy_cmd() -> Command {
    Command::new("override-policy")
        .about(
            "Override approval_policy on an observation. Relaxation requires effective human \
             (--invoker human or --invoker ai_with_human --approve-token <T>). \
             Escalation requires ai_with_human.",
        )
        .arg(
            Arg::new("display_id")
                .help("Display ID of the observation")
                .required(true),
        )
        .arg(
            Arg::new("reason")
                .long("reason")
                .help(
                    "Required reason for the override (recorded in transition_history.actor_note)",
                )
                .required(true),
        )
        .arg(
            Arg::new("approval-policy")
                .long("approval-policy")
                .help("New approval_policy value (auto|human|architecture)")
                .required(true),
        )
}

/// Names that collide with our reserved layer or clap builtins.
fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "id" | "display-id"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_starts_schema_is_bundled_and_parseable() {
        assert!(BUNDLED_STORE_NAMES.contains(&"daemon_starts"));
        let yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "daemon_starts")
            .map(|(_, yaml)| *yaml)
            .expect("daemon_starts schema bundled");
        let schema = crate::schema::Schema::from_yaml(yaml).unwrap();
        assert_eq!(schema.name, "daemon_starts");
    }

    #[test]
    fn intake_templates_are_bundled() {
        let templates = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(name, _)| *name == "intake")
            .map(|(_, templates)| *templates)
            .expect("intake templates bundled");
        assert!(templates.iter().any(|(path, content)| {
            *path == "templates/gatekeeper-brief.md.tpl" && content.contains("Six decisions")
        }));
        assert!(templates.iter().any(|(path, content)| {
            *path == "templates/recon-brief.md.tpl" && content.contains("Required output")
        }));
    }

    #[test]
    fn gatekeeper_brief_template_renders_populated_intake_row() {
        let intake_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "intake")
            .map(|(_, yaml)| *yaml)
            .expect("intake schema bundled");
        let schema = crate::schema::Schema::from_yaml(intake_yaml).unwrap();
        let mut row = crate::validate::EntryMap::new();
        row.insert("display_id".to_string(), serde_json::json!("I001"));
        row.insert("status".to_string(), serde_json::json!("triaging"));
        row.insert(
            "summary".to_string(),
            serde_json::json!("unique summary token"),
        );
        row.insert("source_agent".to_string(), serde_json::json!("executor"));
        row.insert("body".to_string(), serde_json::json!("unique body token"));
        row.insert(
            "evidence".to_string(),
            serde_json::json!("unique evidence token"),
        );
        let template = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(name, _)| *name == "intake")
            .and_then(|(_, templates)| {
                templates
                    .iter()
                    .find(|(path, _)| *path == "templates/gatekeeper-brief.md.tpl")
                    .map(|(_, content)| *content)
            })
            .expect("gatekeeper template bundled");
        let ctx = crate::render::build_context(&schema, &row);
        let rendered = crate::render::render_template(template, &ctx).unwrap();
        for expected in [
            "Gatekeeper Brief: I001",
            "unique summary token",
            "source_agent: executor",
            "unique body token",
            "unique evidence token",
        ] {
            assert!(
                rendered.contains(expected),
                "rendered gatekeeper brief missing `{expected}`:\n{rendered}"
            );
        }
    }

    #[test]
    fn architecture_reviews_add_splits_comma_linked_observations() {
        let ar_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "architecture_reviews")
            .map(|(_, yaml)| *yaml)
            .expect("architecture_reviews schema bundled");
        let schema = crate::schema::Schema::from_yaml(ar_yaml).unwrap();
        let mut cmd = build_store_command(&schema);
        let add = cmd.find_subcommand_mut("add").unwrap().clone();
        let matches = add
            .try_get_matches_from([
                "add",
                "--linked-observations",
                "L010,L011,L012",
                "--linked-observations",
                "L013",
            ])
            .unwrap();
        let got: Vec<String> = matches
            .get_many::<String>("linked-observations")
            .unwrap()
            .cloned()
            .collect();
        assert_eq!(got, vec!["L010", "L011", "L012", "L013"]);
    }

    #[test]
    fn observations_update_help_describes_list_multi_value_input() {
        let observations_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "observations")
            .map(|(_, yaml)| *yaml)
            .expect("observations schema bundled");
        let schema = crate::schema::Schema::from_yaml(observations_yaml).unwrap();
        let mut cmd = build_store_command(&schema);
        let update = cmd.find_subcommand_mut("update").unwrap();
        let help = update.render_long_help().to_string();
        assert!(help.contains("--risk-flags <VALUE[,VALUE]>"), "{help}");
        assert!(
            help.contains("repeat flag or comma-separate values"),
            "{help}"
        );
        assert!(help.contains("escape commas/backslashes"), "{help}");
    }

    #[test]
    fn t084_observations_add_update_help_exposes_canonical_and_deprecated_source_flags() {
        let observations_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "observations")
            .map(|(_, yaml)| *yaml)
            .expect("observations schema bundled");
        let schema = crate::schema::Schema::from_yaml(observations_yaml).unwrap();
        let mut cmd = build_store_command(&schema);
        for verb in ["add", "update"] {
            let sub = cmd.find_subcommand_mut(verb).unwrap();
            let help = sub.render_long_help().to_string();
            assert!(help.contains("--source-env"), "{verb}: {help}");
            assert!(help.contains("--source-id"), "{verb}: {help}");
            assert!(help.contains("--prod-source-id"), "{verb}: {help}");
            assert!(help.contains("--sandbox-source-id"), "{verb}: {help}");
            assert!(help.contains("--origin-db"), "{verb}: {help}");
            assert!(help.contains("DEPRECATED"), "{verb}: {help}");
        }
    }

    #[test]
    fn metrics_command_and_existing_top_level_verbs_are_registered() {
        let manifest = Manifest::empty();
        let schemas = HashMap::new();
        let cmd = build_root(&manifest, &schemas);
        let names: Vec<_> = cmd
            .get_subcommands()
            .map(|s| s.get_name().to_string())
            .collect();
        for expected in ["metrics", "skills", "agents", "topology", "watch"] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
        let metrics = cmd.find_subcommand("metrics").expect("metrics command");
        let args: Vec<_> = metrics
            .get_arguments()
            .map(|a| a.get_id().as_str().to_string())
            .collect();
        assert!(args.contains(&"window".to_string()));
        assert!(args.contains(&"text".to_string()));
        assert!(
            args.contains(&"json".to_string()),
            "metrics must expose a local --json flag; got args: {args:?}"
        );
    }

    #[test]
    fn runner_stats_exposes_raw_telemetry_filters() {
        let manifest = Manifest::empty();
        let schemas = HashMap::new();
        let cmd = build_root(&manifest, &schemas);
        let runner_stats = cmd
            .find_subcommand("runner-stats")
            .expect("runner-stats command");
        let args: Vec<_> = runner_stats
            .get_arguments()
            .map(|a| a.get_id().as_str().to_string())
            .collect();
        for expected in [
            "display_id",
            "role",
            "harness",
            "model",
            "thinking",
            "since",
            "until",
            "include_dirty_data",
            "json",
        ] {
            assert!(
                args.contains(&expected.to_string()),
                "missing {expected}: {args:?}"
            );
        }
    }

    #[test]
    fn recon_template_does_not_show_forbidden_cli_commands_as_allowed() {
        let content = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(name, _)| *name == "intake")
            .and_then(|(_, templates)| {
                templates
                    .iter()
                    .find(|(path, _)| *path == "templates/recon-brief.md.tpl")
                    .map(|(_, content)| *content)
            })
            .expect("recon template bundled");
        for forbidden in [
            "stores observations add",
            "stores tasks add",
            "stores intake add",
            "stores tasks submit-",
            "stores intake route",
            "stores tasks accept",
            "stores tasks reject",
        ] {
            assert!(
                !content.contains(forbidden),
                "recon template must not present forbidden command `{forbidden}`"
            );
        }
        assert!(content.contains("Do not call workflow submission verbs"));
        assert!(content.contains("routing verbs"));
        assert!(content.contains("acceptance verbs"));
        assert!(content.contains("rejection verbs"));
    }
}
