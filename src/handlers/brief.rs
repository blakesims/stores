/// `brief` handler — read-only workflow verb.
///
/// Prints the agent briefing for a workflow-shaped store entry.
/// Defaults to the agent indicated by `next-action`; `--for <agent>` overrides.
///
/// AC4.3: markdown to stdout (default).
/// AC4.4: --for <agent> overrides the default.
/// AC4.5: --for <unknown> errors with all available role names in the message.
/// AC4.7: errors on non-workflow stores.
///
/// Template approach: read the template text from disk at call time using
/// `Workflow::resolve_from_disk` with the store root from the `schema_path` in
/// the manifest.  This is the "read on demand" path described in the carry-forward
/// note; P2-M1 (Phase 5) will thread WorkflowResolved cleanly into main.rs.
use anyhow::{bail, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::paths::stores_dir_for;
use crate::render::{build_context, render_template};
use crate::schema::{actor::InvokerCtx, Schema};

use super::next_action::find_next_agent;
use super::row::read_row;

/// Structured output returned by `compute`.
#[derive(Debug, Serialize, Deserialize)]
pub struct BriefOutput {
    pub agent: String,
    pub brief_markdown: String,
}

/// Pure logic: determine the target agent, load and render the template, return
/// the structured output.  Does NOT print anything.  `run` calls this.
pub(crate) fn compute(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: InvokerCtx,
) -> Result<BriefOutput> {
    // AC4.7: must have a workflow declaration.
    let workflow = match &schema.workflow {
        Some(wf) => wf,
        None => bail!(
            "store '{}' has no workflow declaration; verb only works on workflow-shaped stores",
            schema.name
        ),
    };

    // Task 4.5: scope-aware path — used as the fallback store root below.
    let stores_dir = stores_dir_for(schema.scope)?;

    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let for_agent = matches.get_one::<String>("for").map(|s| s.as_str());

    let (_id, entry) = read_row(schema, conn, display_id)?;

    // Determine the target agent role.
    let agent_role: String = if let Some(explicit) = for_agent {
        // AC4.5: validate against known roles.
        if !workflow.agent_roles.contains_key(explicit) {
            let mut available: Vec<&str> =
                workflow.agent_roles.keys().map(|k| k.as_str()).collect();
            available.sort();
            bail!(
                "unknown agent role '{}'; available roles: {}",
                explicit,
                available.join(", ")
            );
        }
        explicit.to_string()
    } else {
        // Default: use next-action's answer.
        let status = entry
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        match find_next_agent(workflow, &status, &entry) {
            Some(role) => role,
            None => bail!(
                "cannot determine default agent for entry '{}' in status '{}'; \
                 use --for <agent> to specify",
                display_id,
                status
            ),
        }
    };

    // Look up the briefing template path for this role.
    let template_path = workflow
        .briefing_templates
        .get(&agent_role)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "workflow: no briefing_template for agent role '{}'",
                agent_role
            )
        })?;

    // Resolve the store root from the manifest to find the template.
    // P6-m2: detect "bundled:<name>" sentinel and route to BUNDLED_STORE_TEMPLATES;
    // otherwise read from disk as before.
    let manifest = Manifest::load()?;
    let schema_path_str = manifest
        .stores
        .iter()
        .find(|s| s.name == schema.name)
        .map(|s| s.schema_path.to_string_lossy().to_string())
        .unwrap_or_default();

    let template_text = if let Some(bundled_name) = schema_path_str.strip_prefix("bundled:") {
        // Look up from BUNDLED_STORE_TEMPLATES
        let tpl_key = template_path.to_string_lossy();
        crate::cli::dynamic::BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(name, _)| *name == bundled_name)
            .and_then(|(_, templates)| {
                templates
                    .iter()
                    .find(|(path, _)| *path == tpl_key.as_ref())
                    .map(|(_, content)| content.to_string())
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "bundled store '{}': no template '{}' in BUNDLED_STORE_TEMPLATES",
                    bundled_name,
                    tpl_key
                )
            })?
    } else {
        let store_root = manifest
            .stores
            .iter()
            .find(|s| s.name == schema.name)
            .map(|s| s.schema_path.clone())
            .unwrap_or_else(|| stores_dir.clone());
        let full_template_path = store_root.join(template_path);
        std::fs::read_to_string(&full_template_path).map_err(|e| {
            anyhow::anyhow!(
                "cannot read briefing template '{}': {}",
                full_template_path.display(),
                e
            )
        })?
    };

    // Build the render context and render the template.
    let ctx = build_context(schema, &entry);
    let rendered = render_template(&template_text, &ctx)?;

    Ok(BriefOutput {
        agent: agent_role,
        brief_markdown: rendered,
    })
}

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: InvokerCtx,
) -> Result<()> {
    let json_flag = matches.get_flag("json");

    let out = compute(schema, conn, matches, invoker)?;

    if json_flag {
        let json_out = serde_json::json!({
            "agent": out.agent,
            "brief_markdown": out.brief_markdown,
        });
        println!("{}", serde_json::to_string_pretty(&json_out)?);
    } else {
        print!("{}", out.brief_markdown);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::schema::actor::Actor;
    use crate::schema::Schema;
    use clap::{Arg, ArgAction, Command};
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn four_role_schema() -> Schema {
        let yaml = r#"
name: full_wf
id_format: "T{:03d}"
lifecycle:
  states: [planning, plan_review, executing, code_review, blocked]
  transitions: []
fields:
  - name: title
    type: text
    required: true
workflow:
  agent_roles:
    planner:
      description: "Creates the implementation plan"
    plan_reviewer:
      description: "Reviews the plan"
    executor:
      description: "Implements the plan"
    code_reviewer:
      description: "Reviews the code"
  briefing_templates:
    planner: templates/planner.md.tpl
    plan_reviewer: templates/plan-reviewer.md.tpl
    executor: templates/executor.md.tpl
    code_reviewer: templates/code-reviewer.md.tpl
  on_state:
    planning:
      - dispatch_agent: planner
    plan_review:
      - dispatch_agent: plan_reviewer
    executing:
      - dispatch_agent: executor
    code_review:
      - dispatch_agent: code_reviewer
  submit_targets: {}
  max_revise_cycles: 3
"#;
        Schema::from_yaml(yaml).unwrap()
    }

    fn open_db_with_schema(schema: &Schema) -> (tempfile::TempDir, Connection) {
        let dir = tempdir().unwrap();
        let db_file = dir.path().join("test.db");
        let conn = db::open(&db_file).unwrap();
        let ddl = crate::codegen::ddl::ddl_for(schema);
        conn.execute_batch(&ddl).unwrap();
        (dir, conn)
    }

    /// Build minimal ArgMatches for the `brief` subcommand with a given `--for` value.
    fn matches_for(display_id: &str, for_agent: Option<&str>) -> clap::ArgMatches {
        let cmd = Command::new("brief")
            .arg(Arg::new("display_id").required(true))
            .arg(Arg::new("for").long("for").required(false))
            .arg(
                Arg::new("json")
                    .long("json")
                    .action(ArgAction::SetTrue)
                    .required(false),
            );
        let mut args = vec!["brief", display_id];
        if let Some(agent) = for_agent {
            args.push("--for");
            args.push(agent);
        }
        cmd.get_matches_from(args)
    }

    // AC4.5 (M2 fix): call compute() with unknown agent; assert error contains all
    // four role names AND the bad agent name.  This exercises the actual bail! in
    // brief.rs, not a copy of the format string.
    #[test]
    fn brief_compute_unknown_agent_error_lists_all_roles() {
        let schema = four_role_schema();
        let (_dir, conn) = open_db_with_schema(&schema);

        // Insert a row so read_row doesn't error before we hit the agent check.
        conn.execute(
            "INSERT INTO full_wf (display_id, status, created_at, updated_at, created_by, updated_by, title) \
             VALUES ('T001','planning','2026-01-01','2026-01-01','human','human','Test')",
            [],
        ).unwrap();

        let matches = matches_for("T001", Some("nonexistent_agent"));
        let err = compute(&schema, &conn, &matches, Actor::AiAutonomous.into()).unwrap_err();
        let msg = err.to_string();

        // AC4.5: all four roles AND the unknown role must appear in the real error.
        assert!(msg.contains("planner"), "must contain 'planner': {msg}");
        assert!(
            msg.contains("plan_reviewer"),
            "must contain 'plan_reviewer': {msg}"
        );
        assert!(msg.contains("executor"), "must contain 'executor': {msg}");
        assert!(
            msg.contains("code_reviewer"),
            "must contain 'code_reviewer': {msg}"
        );
        assert!(
            msg.contains("nonexistent_agent"),
            "must contain the bad role name: {msg}"
        );
    }

    // AC4.7: non-workflow schema → compute returns error naming the store.
    #[test]
    fn brief_compute_no_workflow_errors() {
        let yaml = r#"
name: obs
id_format: "O{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let (_dir, conn) = open_db_with_schema(&schema);

        let matches = matches_for("O001", None);
        let err = compute(&schema, &conn, &matches, Actor::AiAutonomous.into()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("obs"), "error must name the store: {msg}");
        assert!(
            msg.contains("no workflow declaration"),
            "error must mention workflow: {msg}"
        );
    }

    // ---------------------------------------------------------------------------
    // AC7.4: All four briefing templates render successfully on a fixture row.
    // Uses the bundled tasks schema + BUNDLED_STORE_TEMPLATES, installs to a temp
    // manifest so brief.rs's bundled-sentinel detection is exercised.
    // ---------------------------------------------------------------------------

    #[test]
    fn ac7_4_all_four_briefing_templates_render_successfully() {
        use crate::cli::dynamic::{BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};
        use crate::render::{build_context, render_template};

        // Load the tasks schema
        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .expect("tasks schema");
        let schema = Schema::from_yaml(tasks_yaml).unwrap();

        // Build a minimal fixture entry (planning state)
        let entry: crate::validate::EntryMap = {
            let mut m = std::collections::BTreeMap::new();
            m.insert("display_id".to_string(), serde_json::json!("T001"));
            m.insert("status".to_string(), serde_json::json!("planning"));
            m.insert("title".to_string(), serde_json::json!("Test Task"));
            m.insert("slug".to_string(), serde_json::json!("test-task"));
            m.insert("current_phase".to_string(), serde_json::json!(1));
            m.insert("current_cycle".to_string(), serde_json::json!(1));
            m.insert(
                "created_at".to_string(),
                serde_json::json!("2026-01-01T00:00:00Z"),
            );
            m.insert(
                "updated_at".to_string(),
                serde_json::json!("2026-01-01T00:00:00Z"),
            );
            m.insert(
                "contract".to_string(),
                serde_json::json!({
                    "done_when": "Feature X works end-to-end",
                    "scope_in": "All API endpoints",
                    "scope_out": "UI changes"
                }),
            );
            m.insert("plan".to_string(), serde_json::json!({
                "objective": "Implement the feature",
                "phases": [
                    {"name": "Phase 1: Setup", "objective": "Configure", "tasks": [], "acceptance_criteria": [], "files": [], "dependencies": []}
                ]
            }));
            m.insert("plan_review_log".to_string(), serde_json::json!([]));
            m.insert("cycles".to_string(), serde_json::json!([]));
            m
        };
        let ctx = build_context(&schema, &entry);

        // Get templates from BUNDLED_STORE_TEMPLATES
        let templates = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, t)| *t)
            .expect("tasks templates");

        let roles = ["planner", "plan_reviewer", "executor", "code_reviewer"];
        let template_paths = [
            "templates/planner-brief.md.tpl",
            "templates/plan-reviewer-brief.md.tpl",
            "templates/executor-brief.md.tpl",
            "templates/code-reviewer-brief.md.tpl",
        ];

        for (role, tpl_path) in roles.iter().zip(template_paths.iter()) {
            let content = templates
                .iter()
                .find(|(p, _)| p == tpl_path)
                .map(|(_, c)| *c)
                .unwrap_or_else(|| panic!("template {} missing", tpl_path));
            let rendered = render_template(content, &ctx)
                .unwrap_or_else(|e| panic!("{} render failed: {}", role, e));
            // Critical sections: title and done_when must appear
            assert!(
                rendered.contains("Test Task"),
                "{} brief must contain title: {}",
                role,
                &rendered[..200.min(rendered.len())]
            );
            assert!(
                rendered.contains("Feature X works end-to-end"),
                "{} brief must contain done_when",
                role
            );
        }
    }

    // ---------------------------------------------------------------------------
    // T016: Decision Matrix rendering in plan-reviewer brief.
    // ---------------------------------------------------------------------------

    fn render_plan_reviewer_brief_with_plan(plan: serde_json::Value) -> String {
        use crate::cli::dynamic::{BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};
        use crate::render::{build_context, render_template};

        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .expect("tasks schema");
        let schema = Schema::from_yaml(tasks_yaml).unwrap();

        let entry: crate::validate::EntryMap = {
            let mut m = std::collections::BTreeMap::new();
            m.insert("display_id".to_string(), serde_json::json!("T001"));
            m.insert("status".to_string(), serde_json::json!("plan_review"));
            m.insert("title".to_string(), serde_json::json!("Test Task"));
            m.insert("slug".to_string(), serde_json::json!("test-task"));
            m.insert("current_phase".to_string(), serde_json::json!(1));
            m.insert("current_cycle".to_string(), serde_json::json!(1));
            m.insert(
                "created_at".to_string(),
                serde_json::json!("2026-01-01T00:00:00Z"),
            );
            m.insert(
                "updated_at".to_string(),
                serde_json::json!("2026-01-01T00:00:00Z"),
            );
            m.insert(
                "contract".to_string(),
                serde_json::json!({
                    "done_when": "Feature X works end-to-end",
                    "scope_in": "All API endpoints",
                    "scope_out": "UI changes"
                }),
            );
            m.insert("plan".to_string(), plan);
            m.insert("plan_review_log".to_string(), serde_json::json!([]));
            m.insert("cycles".to_string(), serde_json::json!([]));
            m
        };
        let ctx = build_context(&schema, &entry);

        let templates = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, t)| *t)
            .expect("tasks templates");
        let content = templates
            .iter()
            .find(|(p, _)| *p == "templates/plan-reviewer-brief.md.tpl")
            .map(|(_, c)| *c)
            .expect("plan-reviewer template");
        render_template(content, &ctx).expect("render")
    }

    #[test]
    fn plan_reviewer_brief_renders_decision_matrix_with_three_entries() {
        let plan = serde_json::json!({
            "objective": "Implement the feature",
            "phases": [
                {"name": "Phase 1: Setup", "objective": "Configure", "tasks": [], "acceptance_criteria": [], "files": [], "dependencies": []}
            ],
            "decision_matrix": [
                {
                    "decision": "Storage backend choice",
                    "options": ["sqlite", "postgres", "in-memory"],
                    "chosen": "sqlite",
                    "rationale": "Embedded zero-config fits single-user CLI."
                },
                {
                    "decision": "Template engine selection",
                    "options": ["handlebars", "tera"],
                    "chosen": "handlebars",
                    "rationale": "Already a dependency; familiar syntax."
                },
                {
                    "decision": "Brief delivery channel",
                    "options": ["stdin pipe", "tempfile path", "env var"],
                    "chosen": "stdin pipe",
                    "rationale": "Avoids tempfile cleanup; works on all platforms."
                }
            ]
        });
        let rendered = render_plan_reviewer_brief_with_plan(plan);

        assert!(
            rendered.contains("## Decision Matrix"),
            "must contain Decision Matrix header: {rendered}"
        );
        for name in [
            "Storage backend choice",
            "Template engine selection",
            "Brief delivery channel",
        ] {
            assert!(rendered.contains(name), "missing decision name {name}");
        }
        for chosen in ["sqlite", "handlebars", "stdin pipe"] {
            assert!(rendered.contains(chosen), "missing chosen value {chosen}");
        }
        for rationale in [
            "Embedded zero-config fits single-user CLI.",
            "Already a dependency; familiar syntax.",
            "Avoids tempfile cleanup; works on all platforms.",
        ] {
            assert!(
                rendered.contains(rationale),
                "missing rationale {rationale}"
            );
        }

        // AC2.4: section ordering.
        let cur = rendered.find("## Current Plan").expect("Current Plan");
        let dm = rendered
            .find("## Decision Matrix")
            .expect("Decision Matrix");
        let prior = rendered
            .find("## Prior Plan Reviews")
            .expect("Prior Plan Reviews");
        assert!(cur < dm, "Decision Matrix must come after Current Plan");
        assert!(
            dm < prior,
            "Decision Matrix must come before Prior Plan Reviews"
        );
    }

    #[test]
    fn plan_reviewer_brief_omits_decision_matrix_when_absent() {
        let plan = serde_json::json!({
            "objective": "Implement the feature",
            "phases": [
                {"name": "Phase 1: Setup", "objective": "Configure", "tasks": [], "acceptance_criteria": [], "files": [], "dependencies": []}
            ]
        });
        let rendered = render_plan_reviewer_brief_with_plan(plan);

        assert!(
            rendered.contains("(no decisions recorded)"),
            "empty-state placeholder must render: {rendered}"
        );
        assert!(
            !rendered.contains("{{"),
            "rendered output must not contain literal handlebars markup"
        );
        assert!(
            !rendered.contains("undefined"),
            "rendered output must not contain 'undefined'"
        );
    }

    // ---------------------------------------------------------------------------
    // T039: Planner brief tier_hint awareness.
    // ---------------------------------------------------------------------------

    fn render_planner_brief_with_tier(tier: Option<&str>) -> String {
        use crate::cli::dynamic::{BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};
        use crate::render::{build_context, render_template};

        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .expect("tasks schema");
        let schema = Schema::from_yaml(tasks_yaml).unwrap();

        let entry: crate::validate::EntryMap = {
            let mut m = std::collections::BTreeMap::new();
            m.insert("display_id".to_string(), serde_json::json!("T001"));
            m.insert("status".to_string(), serde_json::json!("planning"));
            m.insert("title".to_string(), serde_json::json!("Test Task"));
            m.insert("slug".to_string(), serde_json::json!("test-task"));
            m.insert("current_phase".to_string(), serde_json::json!(1));
            m.insert("current_cycle".to_string(), serde_json::json!(1));
            if let Some(t) = tier {
                m.insert("tier_hint".to_string(), serde_json::json!(t));
            }
            m.insert(
                "contract".to_string(),
                serde_json::json!({
                    "done_when": "Feature X works end-to-end",
                    "scope_in": "All API endpoints",
                    "scope_out": "UI changes"
                }),
            );
            m.insert("plan_review_log".to_string(), serde_json::json!([]));
            m.insert("cycles".to_string(), serde_json::json!([]));
            m
        };
        let ctx = build_context(&schema, &entry);

        let templates = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, t)| *t)
            .expect("tasks templates");
        let content = templates
            .iter()
            .find(|(p, _)| *p == "templates/planner-brief.md.tpl")
            .map(|(_, c)| *c)
            .expect("planner-brief template");
        render_template(content, &ctx).expect("render")
    }

    #[test]
    fn planner_brief_t1_carries_defensive_should_not_be_invoked_note() {
        let rendered = render_planner_brief_with_tier(Some("T1"));
        assert!(
            rendered.contains("**Tier:** T1"),
            "T1 brief must label tier: {rendered}"
        );
        assert!(
            rendered.contains("SHOULD NOT be invoked"),
            "T1 brief must contain defensive note: {rendered}"
        );
        // T1 must NOT carry T2/T3 instructions.
        assert!(
            !rendered.contains("Produce exactly one phase"),
            "T1 brief must not contain T2 instruction"
        );
        assert!(
            !rendered.contains("Decompose into multiple phases"),
            "T1 brief must not contain T3 instruction"
        );
    }

    #[test]
    fn planner_brief_t2_contains_produce_exactly_one_phase_instruction() {
        let rendered = render_planner_brief_with_tier(Some("T2"));
        assert!(
            rendered.contains("**Tier:** T2"),
            "T2 brief must label tier: {rendered}"
        );
        assert!(
            rendered.contains("Produce exactly one phase"),
            "T2 brief must contain explicit one-phase instruction: {rendered}"
        );
        assert!(
            rendered.contains("phases.length != 1"),
            "T2 brief must explain the schema rejection: {rendered}"
        );
        assert!(
            !rendered.contains("Decompose into multiple phases"),
            "T2 brief must not contain T3 instruction"
        );
        assert!(
            !rendered.contains("SHOULD NOT be invoked"),
            "T2 brief must not contain T1 defensive note"
        );
    }

    #[test]
    fn planner_brief_t3_contains_multi_phase_decomposition_instruction() {
        let rendered = render_planner_brief_with_tier(Some("T3"));
        assert!(
            rendered.contains("**Tier:** T3"),
            "T3 brief must label tier: {rendered}"
        );
        assert!(
            rendered.contains("Decompose into multiple phases"),
            "T3 brief must contain multi-phase instruction: {rendered}"
        );
        assert!(
            !rendered.contains("Produce exactly one phase"),
            "T3 brief must not contain T2 instruction"
        );
        assert!(
            !rendered.contains("SHOULD NOT be invoked"),
            "T3 brief must not contain T1 defensive note"
        );
    }

    #[test]
    fn planner_brief_unset_tier_falls_back_with_flag() {
        let rendered = render_planner_brief_with_tier(None);
        assert!(
            rendered.contains("**Tier:** _unset_"),
            "unset-tier brief must label tier as unset: {rendered}"
        );
        assert!(
            rendered.contains("flag the missing tier"),
            "unset-tier brief must instruct planner to flag: {rendered}"
        );
    }

    // T060: executor and code-reviewer briefs are tier-aware. T1 uses the
    // contract as the plan; T3 keeps the existing phase-decomposition sections.
    #[test]
    fn executor_and_code_reviewer_briefs_branch_by_tier() {
        use crate::cli::dynamic::{BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};
        use crate::render::{build_context, render_template};

        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .expect("tasks schema");
        let schema = Schema::from_yaml(tasks_yaml).unwrap();

        let templates = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, t)| *t)
            .expect("tasks templates");
        let executor_tpl = templates
            .iter()
            .find(|(p, _)| *p == "templates/executor-brief.md.tpl")
            .map(|(_, c)| *c)
            .expect("executor-brief template");
        let cr_tpl = templates
            .iter()
            .find(|(p, _)| *p == "templates/code-reviewer-brief.md.tpl")
            .map(|(_, c)| *c)
            .expect("code-reviewer-brief template");

        let plan = serde_json::json!({
            "objective": "fix the thing",
            "phases": [{
                "name": "Contract execution",
                "objective": "the thing is fixed",
                "tasks": ["edit module A", "edit module B"],
                "acceptance_criteria": ["the thing is fixed"],
                "files": [],
                "dependencies": []
            }]
        });

        let entry_for_tier = |tier: &str| -> crate::validate::EntryMap {
            let mut m = std::collections::BTreeMap::new();
            m.insert("display_id".to_string(), serde_json::json!("T001"));
            m.insert("status".to_string(), serde_json::json!("executing"));
            m.insert("title".to_string(), serde_json::json!(format!("{tier} task")));
            m.insert("slug".to_string(), serde_json::json!("tier-task"));
            m.insert("current_phase".to_string(), serde_json::json!(1));
            m.insert("current_cycle".to_string(), serde_json::json!(1));
            m.insert("tier_hint".to_string(), serde_json::json!(tier));
            m.insert("plan".to_string(), plan.clone());
            m.insert(
                "contract".to_string(),
                serde_json::json!({
                    "executive_intent": "fix the thing",
                    "done_when": "the thing is fixed",
                    "scope_in": "edit module A\nedit module B",
                    "scope_out": "do not edit module C"
                }),
            );
            m.insert(
                "created_at".to_string(),
                serde_json::json!("2026-01-01T00:00:00Z"),
            );
            m.insert(
                "updated_at".to_string(),
                serde_json::json!("2026-01-01T00:00:00Z"),
            );
            m.insert("plan_review_log".to_string(), serde_json::json!([]));
            m.insert("cycles".to_string(), serde_json::json!([]));
            m
        };

        let t1_ctx = build_context(&schema, &entry_for_tier("T1"));
        let t1_exec = render_template(executor_tpl, &t1_ctx).expect("T1 executor brief render");
        assert!(
            t1_exec.contains("**Tier:** T1 (contract-is-plan)"),
            "T1 executor brief must label contract-is-plan tier: {t1_exec}"
        );
        assert!(
            t1_exec.contains("## Scope")
                && t1_exec.contains("edit module A")
                && t1_exec.contains("do not edit module C"),
            "T1 executor brief must show contract scope: {t1_exec}"
        );
        assert!(
            t1_exec.contains("## What to Do (T1 contract-is-plan)"),
            "T1 executor brief must show T1 guidance: {t1_exec}"
        );
        assert!(
            !t1_exec.contains("**Current Phase:** 1 of 1")
                && !t1_exec.contains("## Current Phase to Execute"),
            "T1 executor brief must skip phase decomposition: {t1_exec}"
        );

        let t1_cr = render_template(cr_tpl, &t1_ctx).expect("T1 code-reviewer brief render");
        assert!(
            t1_cr.contains("## What to Review (T1 contract-is-plan)"),
            "T1 code-reviewer brief must show T1 review guidance: {t1_cr}"
        );
        assert!(
            !t1_cr.contains("## Phase Being Reviewed")
                && !t1_cr.contains("**Current Phase:** 1 of 1"),
            "T1 code-reviewer brief must skip phase decomposition: {t1_cr}"
        );

        let t3_ctx = build_context(&schema, &entry_for_tier("T3"));
        let t3_exec = render_template(executor_tpl, &t3_ctx).expect("T3 executor brief render");
        assert!(
            t3_exec.contains("**Current Phase:** 1 of 1")
                && t3_exec.contains("## Current Phase to Execute")
                && t3_exec.contains("Contract execution"),
            "T3 executor brief must keep phase decomposition: {t3_exec}"
        );
        assert!(
            !t3_exec.contains("contract-is-plan"),
            "T3 executor brief must not show T1 guidance: {t3_exec}"
        );

        let t3_cr = render_template(cr_tpl, &t3_ctx).expect("T3 code-reviewer brief render");
        assert!(
            t3_cr.contains("**Current Phase:** 1 of 1")
                && t3_cr.contains("## Phase Being Reviewed")
                && t3_cr.contains("Contract execution"),
            "T3 code-reviewer brief must keep phase review section: {t3_cr}"
        );
        assert!(
            !t3_cr.contains("contract-is-plan"),
            "T3 code-reviewer brief must not show T1 guidance: {t3_cr}"
        );
    }

    // find_next_agent helper test — kept for regression coverage.
    #[test]
    fn find_next_agent_returns_first_dispatch() {
        // Use the workflow_minimal fixture (2 roles) for this helper test.
        let yaml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/workflow_minimal/schema.yaml"),
        )
        .unwrap();
        let schema = Schema::from_yaml(&yaml).unwrap();
        let workflow = schema.workflow.as_ref().unwrap();

        let entry = std::collections::BTreeMap::new();
        let agent = find_next_agent(workflow, "planning", &entry);
        assert_eq!(agent, Some("planner".to_string()));

        let agent2 = find_next_agent(workflow, "executing", &entry);
        assert_eq!(agent2, Some("executor".to_string()));

        let agent3 = find_next_agent(workflow, "blocked", &entry);
        assert_eq!(agent3, None);
    }
}
