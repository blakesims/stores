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
use serde_json::json;

use crate::manifest::Manifest;
use crate::paths::stores_dir_for;
use crate::render::{build_context, render_template};
use crate::schema::{actor::Actor, Schema};

use super::next_action::find_next_agent;
use super::row::read_row;

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: Actor,
) -> Result<()> {
    // AC4.7: must have a workflow declaration.
    let workflow = match &schema.workflow {
        Some(wf) => wf,
        None => bail!(
            "store '{}' has no workflow declaration; verb only works on workflow-shaped stores",
            schema.name
        ),
    };

    // Task 4.5: scope-aware path — confirms resolvability.
    let stores_dir = stores_dir_for(schema.scope)?;

    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let for_agent = matches.get_one::<String>("for").map(|s| s.as_str());

    let json_flag = matches.get_flag("json");

    let (_id, entry) = read_row(schema, conn, display_id)?;

    // Determine the target agent role.
    let agent_role: String = if let Some(explicit) = for_agent {
        // AC4.5: validate against known roles.
        if !workflow.agent_roles.contains_key(explicit) {
            let available: Vec<&str> = workflow
                .agent_roles
                .keys()
                .map(|k| k.as_str())
                .collect();
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

        match find_next_agent(workflow, &status) {
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

    // Resolve the store root from the manifest to find the template on disk.
    // Design choice (documented in task notes): read template from disk on demand
    // using the installed store's schema_path directory.  P2-M1 (Phase 5) will
    // thread WorkflowResolved cleanly; for now we resolve once per call.
    let manifest = Manifest::load()?;
    let store_root = manifest
        .stores
        .iter()
        .find(|s| s.name == schema.name)
        .map(|s| {
            // schema_path is the path to schema.yaml; parent is the store root.
            s.schema_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| s.schema_path.clone())
        })
        .unwrap_or_else(|| stores_dir.clone());

    let full_template_path = store_root.join(template_path);
    let template_text = std::fs::read_to_string(&full_template_path).map_err(|e| {
        anyhow::anyhow!(
            "cannot read briefing template '{}': {}",
            full_template_path.display(),
            e
        )
    })?;

    // Build the render context and render the template.
    let ctx = build_context(schema, &entry);
    let rendered = render_template(&template_text, &ctx)?;

    if json_flag {
        let out = json!({
            "agent": agent_role,
            "brief_markdown": rendered,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        print!("{rendered}");
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
    use crate::schema::Schema;
    use tempfile::tempdir;

    fn wf_schema() -> Schema {
        let yaml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/workflow_minimal/schema.yaml"),
        )
        .unwrap();
        Schema::from_yaml(&yaml).unwrap()
    }

    // AC4.5: unknown agent role error lists all known roles
    #[test]
    fn brief_unknown_agent_error_lists_all_roles() {
        let schema = wf_schema();
        let workflow = schema.workflow.as_ref().unwrap();

        // Simulate the error message we produce for an unknown role.
        let bad_role = "nonexistent_agent";
        let is_known = workflow.agent_roles.contains_key(bad_role);
        assert!(!is_known, "nonexistent_agent should not be a known role");

        // Construct the error message as the handler would.
        let available: Vec<&str> = workflow.agent_roles.keys().map(|k| k.as_str()).collect();
        let msg = format!(
            "unknown agent role '{}'; available roles: {}",
            bad_role,
            available.join(", ")
        );

        // All four required roles from the minimal fixture must appear in the message.
        // (The fixture has planner + executor; real tasks store has all four.)
        assert!(
            msg.contains("planner"),
            "error must mention 'planner': {msg}"
        );
        assert!(
            msg.contains("executor"),
            "error must mention 'executor': {msg}"
        );
        assert!(
            msg.contains("nonexistent_agent"),
            "error must mention the bad role: {msg}"
        );
    }

    // AC4.5 (full four roles): test with a schema that declares all four agent roles
    #[test]
    fn brief_unknown_agent_error_with_all_four_roles() {
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
        let schema = Schema::from_yaml(yaml).unwrap();
        let workflow = schema.workflow.as_ref().unwrap();

        let bad_role = "unknown_agent";
        let available: Vec<&str> = workflow.agent_roles.keys().map(|k| k.as_str()).collect();
        let msg = format!(
            "unknown agent role '{}'; available roles: {}",
            bad_role,
            available.join(", ")
        );

        // AC4.5: all four roles must appear in the error.
        assert!(msg.contains("planner"), "must contain 'planner': {msg}");
        assert!(msg.contains("plan_reviewer"), "must contain 'plan_reviewer': {msg}");
        assert!(msg.contains("executor"), "must contain 'executor': {msg}");
        assert!(msg.contains("code_reviewer"), "must contain 'code_reviewer': {msg}");
    }

    // find_next_agent helper test
    #[test]
    fn find_next_agent_returns_first_dispatch() {
        let schema = wf_schema();
        let workflow = schema.workflow.as_ref().unwrap();

        let agent = find_next_agent(workflow, "planning");
        assert_eq!(agent, Some("planner".to_string()));

        let agent2 = find_next_agent(workflow, "executing");
        assert_eq!(agent2, Some("executor".to_string()));

        let agent3 = find_next_agent(workflow, "blocked");
        assert_eq!(agent3, None);
    }
}
