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
use crate::render::{build_context, render_template_with_overlay};
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
    let mut overlay = build_source_observation_overlay(conn, &entry)?;
    // I022 repair-lane: merge external-review REVISE backpressure overlay so the
    // CLI `brief` verb surfaces the same external-review findings as auto-drive.
    for (k, v) in build_external_review_overlay(conn, &entry)? {
        overlay.insert(k, v);
    }
    let rendered = render_template_with_overlay(&template_text, &ctx, &overlay)?;

    Ok(BriefOutput {
        agent: agent_role,
        brief_markdown: rendered,
    })
}

pub(crate) fn build_source_observation_overlay(
    conn: &Connection,
    entry: &crate::validate::EntryMap,
) -> Result<std::collections::HashMap<String, serde_json::Value>> {
    let mut overlay = std::collections::HashMap::new();
    let ids: Vec<String> = match entry.get("linked_observations") {
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        Some(serde_json::Value::String(s)) => serde_json::from_str::<serde_json::Value>(s)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => Vec::new(),
    };

    if ids.is_empty() {
        overlay.insert(
            "source_observations".to_string(),
            serde_json::Value::Array(Vec::new()),
        );
        return Ok(overlay);
    }

    let mut observations = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT display_id, summary, intent_contract FROM observations WHERE display_id=?1",
    )?;
    for obs_id in ids {
        let row = stmt.query_row(rusqlite::params![obs_id], |r| {
            let display_id: String = r.get(0)?;
            let summary: Option<String> = r.get(1).ok();
            let intent_raw: Option<String> = r.get(2).ok();
            Ok((display_id, summary.unwrap_or_default(), intent_raw))
        });
        let Ok((display_id, summary, intent_raw)) = row else {
            continue;
        };
        let intent_contract = intent_raw
            .as_deref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .unwrap_or(serde_json::Value::Null);
        observations.push(serde_json::json!({
            "display_id": display_id,
            "summary": summary,
            "intent_contract": intent_contract,
        }));
    }

    overlay.insert(
        "source_observations".to_string(),
        serde_json::Value::Array(observations),
    );
    Ok(overlay)
}

/// I022 repair-lane (Pi msg_31492ff7 shape A): surface the latest REVISE-verdict
/// `external_reviews` row for this task as `external_review_backpressure` in the
/// brief overlay, so respawned executor / code-reviewer briefs include the
/// codex/external-review findings text. Without this overlay, the existing
/// `cycles[].review` Revision Context section only carries in-cycle code-reviewer
/// backpressure; external-review REVISE fires never reach the executor, which is
/// the bug T107 cycle-2 (run 1506) demonstrated.
///
/// Returns null in the overlay when no REVISE-verdict ER exists for the task —
/// the template `{{#if}}` handles absence cleanly.
pub(crate) fn build_external_review_overlay(
    conn: &Connection,
    entry: &crate::validate::EntryMap,
) -> Result<std::collections::HashMap<String, serde_json::Value>> {
    let mut overlay = std::collections::HashMap::new();
    let task_display_id = entry
        .get("display_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if task_display_id.is_empty() {
        overlay.insert(
            "external_review_backpressure".to_string(),
            serde_json::Value::Null,
        );
        return Ok(overlay);
    }

    let mut stmt = match conn.prepare(
        "SELECT display_id, runner, verdict, attempt, head_sha, base_sha, findings, \
                critical_count, major_count, minor_count \
         FROM external_reviews \
         WHERE task_id = ?1 AND verdict = 'REVISE' \
         ORDER BY id DESC LIMIT 1",
    ) {
        Ok(s) => s,
        Err(_) => {
            overlay.insert(
                "external_review_backpressure".to_string(),
                serde_json::Value::Null,
            );
            return Ok(overlay);
        }
    };

    let row = stmt.query_row(rusqlite::params![task_display_id], |r| {
        let display_id: String = r.get(0)?;
        let runner: Option<String> = r.get(1).ok();
        let verdict: Option<String> = r.get(2).ok();
        let attempt: Option<i64> = r.get(3).ok();
        let head_sha: Option<String> = r.get(4).ok();
        let base_sha: Option<String> = r.get(5).ok();
        let findings: Option<String> = r.get(6).ok();
        let critical_count: Option<i64> = r.get(7).ok();
        let major_count: Option<i64> = r.get(8).ok();
        let minor_count: Option<i64> = r.get(9).ok();
        Ok(serde_json::json!({
            "display_id": display_id,
            "runner": runner.unwrap_or_default(),
            "verdict": verdict.unwrap_or_default(),
            "attempt": attempt.unwrap_or(0),
            "head_sha": head_sha.unwrap_or_default(),
            "base_sha": base_sha.unwrap_or_default(),
            "findings": findings.unwrap_or_default(),
            "critical_count": critical_count.unwrap_or(0),
            "major_count": major_count.unwrap_or(0),
            "minor_count": minor_count.unwrap_or(0),
        }))
    });

    overlay.insert(
        "external_review_backpressure".to_string(),
        row.unwrap_or(serde_json::Value::Null),
    );
    Ok(overlay)
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

        // Build a minimal fixture entry with primary ADR 0001 state columns.
        let entry: crate::validate::EntryMap = {
            let mut m = std::collections::BTreeMap::new();
            m.insert("display_id".to_string(), serde_json::json!("T001"));
            m.insert("status".to_string(), serde_json::json!("legacy_unknown"));
            m.insert("title".to_string(), serde_json::json!("Test Task"));
            m.insert("lifecycle".to_string(), serde_json::json!("integration"));
            m.insert("active_step".to_string(), serde_json::json!("none"));
            m.insert("integration_step".to_string(), serde_json::json!("testing"));
            m.insert("blocked".to_string(), serde_json::json!(true));
            m.insert("blocker_kind".to_string(), serde_json::json!("main_red"));
            m.insert(
                "post_integration_step".to_string(),
                serde_json::json!("repo_specific"),
            );
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

        let roles = ["planner", "plan_reviewer", "executor", "code_reviewer", "wrap"];
        let template_paths = [
            "templates/planner-brief.md.tpl",
            "templates/plan-reviewer-brief.md.tpl",
            "templates/executor-brief.md.tpl",
            "templates/code-reviewer-brief.md.tpl",
            "templates/wrap-brief.md.tpl",
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
            for expected in [
                "lifecycle=integration",
                "active_step=none",
                "integration_step=testing",
                "blocked=true",
                "blocker_kind=main_red",
                "post_integration_step=repo_specific",
            ] {
                assert!(
                    rendered.contains(expected),
                    "{} brief must contain primary task-state token {expected}: {rendered}",
                    role
                );
            }
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

    #[test]
    fn planner_revision_brief_includes_rejected_plan() {
        use crate::cli::dynamic::{BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};
        use crate::render::{build_context, render_template};

        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .expect("tasks schema");
        let schema = Schema::from_yaml(tasks_yaml).unwrap();

        let mut entry = std::collections::BTreeMap::new();
        entry.insert("display_id".to_string(), serde_json::json!("T123"));
        entry.insert("status".to_string(), serde_json::json!("planning"));
        entry.insert("title".to_string(), serde_json::json!("Revise Plan"));
        entry.insert("slug".to_string(), serde_json::json!("revise-plan"));
        entry.insert("tier_hint".to_string(), serde_json::json!("T3"));
        entry.insert("current_phase".to_string(), serde_json::json!(0));
        entry.insert("current_cycle".to_string(), serde_json::json!(0));
        entry.insert(
            "contract".to_string(),
            serde_json::json!({
                "done_when": "Done",
                "scope_in": "In",
                "scope_out": "Out"
            }),
        );
        entry.insert(
            "plan".to_string(),
            serde_json::json!({
                "objective": "UNIQUE_REJECTED_PLAN_OBJECTIVE",
                "phases": [{
                    "name": "Rejected Phase",
                    "objective": "Rejected objective",
                    "tasks": ["Rejected task"],
                    "acceptance_criteria": ["Rejected AC"],
                    "files": ["src/rejected.rs"],
                    "dependencies": []
                }]
            }),
        );
        entry.insert(
            "plan_review_log".to_string(),
            serde_json::json!([{
                "gate": "NEEDS_WORK",
                "summary": "UNIQUE_REVIEW_BACKPRESSURE",
                "open_questions": ["What about invariant X?"]
            }]),
        );
        entry.insert("cycles".to_string(), serde_json::json!([]));

        let templates = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, t)| *t)
            .expect("tasks templates");
        let tpl = templates
            .iter()
            .find(|(p, _)| *p == "templates/planner-brief.md.tpl")
            .map(|(_, c)| *c)
            .expect("planner template");
        let rendered = render_template(tpl, &build_context(&schema, &entry)).expect("render");

        assert!(rendered.contains("## Revision Context"), "{rendered}");
        assert!(
            rendered.contains("UNIQUE_REJECTED_PLAN_OBJECTIVE"),
            "{rendered}"
        );
        assert!(rendered.contains("Rejected task"), "{rendered}");
        assert!(
            rendered.contains("UNIQUE_REVIEW_BACKPRESSURE"),
            "{rendered}"
        );
        assert!(
            rendered.contains("reconstruct the previous plan"),
            "{rendered}"
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
            m.insert(
                "title".to_string(),
                serde_json::json!(format!("{tier} task")),
            );
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

    #[test]
    fn executor_and_code_reviewer_revision_briefs_call_out_backpressure() {
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
            .expect("executor template");
        let cr_tpl = templates
            .iter()
            .find(|(p, _)| *p == "templates/code-reviewer-brief.md.tpl")
            .map(|(_, c)| *c)
            .expect("code reviewer template");

        let mut entry = std::collections::BTreeMap::new();
        entry.insert("display_id".to_string(), serde_json::json!("T124"));
        entry.insert("status".to_string(), serde_json::json!("executing"));
        entry.insert("title".to_string(), serde_json::json!("Revise Code"));
        entry.insert("slug".to_string(), serde_json::json!("revise-code"));
        entry.insert("tier_hint".to_string(), serde_json::json!("T3"));
        entry.insert("current_phase".to_string(), serde_json::json!(1));
        entry.insert("current_cycle".to_string(), serde_json::json!(2));
        entry.insert(
            "contract".to_string(),
            serde_json::json!({"done_when":"Done","scope_in":"In","scope_out":"Out"}),
        );
        entry.insert(
            "plan".to_string(),
            serde_json::json!({
                "objective": "Plan",
                "phases": [{
                    "name": "Phase One",
                    "objective": "Do phase",
                    "tasks": ["Task"],
                    "acceptance_criteria": ["AC"],
                    "files": [],
                    "dependencies": []
                }]
            }),
        );
        entry.insert("plan_review_log".to_string(), serde_json::json!([]));
        entry.insert(
            "cycles".to_string(),
            serde_json::json!([
                {
                    "phase": 1,
                    "cycle": 1,
                    "executor": {
                        "summary": "UNIQUE_PRIOR_EXECUTOR_SUMMARY",
                        "commit": "abc123",
                        "files_changed": ["src/lib.rs"]
                    },
                    "review": {
                        "gate": "REVISE",
                        "summary": "UNIQUE_REVISE_SUMMARY",
                        "details": "UNIQUE_REVISE_DETAILS",
                        "critical": 0,
                        "major": 1,
                        "minor": 0
                    }
                },
                {
                    "phase": 1,
                    "cycle": 2,
                    "executor": {
                        "summary": "UNIQUE_CURRENT_EXECUTOR_SUMMARY",
                        "files_changed": ["src/lib.rs"]
                    },
                    "review": null
                }
            ]),
        );

        let ctx = build_context(&schema, &entry);
        let executor = render_template(executor_tpl, &ctx).expect("executor render");
        assert!(
            executor.contains("## Revision Context for This Phase"),
            "{executor}"
        );
        assert!(
            executor.contains("UNIQUE_PRIOR_EXECUTOR_SUMMARY"),
            "{executor}"
        );
        assert!(executor.contains("UNIQUE_REVISE_SUMMARY"), "{executor}");
        assert!(executor.contains("revision cycle 2"), "{executor}");

        let code_reviewer = render_template(cr_tpl, &ctx).expect("code reviewer render");
        assert!(
            code_reviewer.contains("## Re-review Context"),
            "{code_reviewer}"
        );
        assert!(
            code_reviewer.contains("UNIQUE_CURRENT_EXECUTOR_SUMMARY"),
            "{code_reviewer}"
        );
        assert!(
            code_reviewer.contains("UNIQUE_REVISE_DETAILS"),
            "{code_reviewer}"
        );
    }

    // ---------------------------------------------------------------------------
    // I022 repair-lane: external_review overlay surfaces REVISE-verdict findings
    // (Pi msg_31492ff7 shape A). Tests verify (a) the overlay function returns
    // null when no ER row exists, (b) returns the latest REVISE-verdict row when
    // multiple ER rows exist, and (c) the rendered executor template includes the
    // findings text in the External Review Backpressure section.
    // ---------------------------------------------------------------------------

    /// Helper: create the external_reviews table directly (mirrors the substrate
    /// schema enough for the SELECT in build_external_review_overlay).
    fn create_external_reviews_table(conn: &Connection) {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS external_reviews (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                display_id TEXT UNIQUE NOT NULL,
                status TEXT NOT NULL,
                task_id TEXT,
                attempt INTEGER,
                runner TEXT,
                head_sha TEXT,
                base_sha TEXT,
                verdict TEXT,
                critical_count INTEGER,
                major_count INTEGER,
                minor_count INTEGER,
                findings TEXT
            );
            "#,
        )
        .unwrap();
    }

    fn entry_with_display_id(display_id: &str) -> crate::validate::EntryMap {
        let mut m = std::collections::BTreeMap::new();
        m.insert("display_id".to_string(), serde_json::json!(display_id));
        m
    }

    #[test]
    fn build_external_review_overlay_returns_null_when_no_er_row() {
        let schema = four_role_schema();
        let (_dir, conn) = open_db_with_schema(&schema);
        create_external_reviews_table(&conn);

        let entry = entry_with_display_id("T999");
        let overlay = build_external_review_overlay(&conn, &entry).unwrap();
        let v = overlay
            .get("external_review_backpressure")
            .expect("overlay key present");
        assert!(v.is_null(), "expected null when no ER row, got {v}");
    }

    #[test]
    fn build_external_review_overlay_returns_latest_revise_row() {
        let schema = four_role_schema();
        let (_dir, conn) = open_db_with_schema(&schema);
        create_external_reviews_table(&conn);

        // Insert older PASS row (must be ignored), then older REVISE, then newest REVISE.
        conn.execute(
            "INSERT INTO external_reviews (display_id, status, task_id, attempt, runner, \
             head_sha, base_sha, verdict, critical_count, major_count, minor_count, findings) \
             VALUES ('ER001','closed','T107',1,'codex','aaa111','base000','PASS',0,0,0,'OLD_PASS')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO external_reviews (display_id, status, task_id, attempt, runner, \
             head_sha, base_sha, verdict, critical_count, major_count, minor_count, findings) \
             VALUES ('ER002','revise','T107',2,'codex','bbb222','base000','REVISE',0,1,0,'OLD_REVISE_TEXT')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO external_reviews (display_id, status, task_id, attempt, runner, \
             head_sha, base_sha, verdict, critical_count, major_count, minor_count, findings) \
             VALUES ('ER003','revise','T107',6,'codex','ccc333','base111','REVISE',0,1,0,\
             'NEWEST_REVISE_FINDINGS_KEEP_CLUSTER_KEYS_IN_ONE_REGISTRY')",
            [],
        )
        .unwrap();

        let entry = entry_with_display_id("T107");
        let overlay = build_external_review_overlay(&conn, &entry).unwrap();
        let v = overlay.get("external_review_backpressure").unwrap();
        assert_eq!(v["display_id"], serde_json::json!("ER003"));
        assert_eq!(v["verdict"], serde_json::json!("REVISE"));
        assert_eq!(v["attempt"], serde_json::json!(6));
        assert_eq!(v["head_sha"], serde_json::json!("ccc333"));
        assert_eq!(v["base_sha"], serde_json::json!("base111"));
        assert_eq!(v["runner"], serde_json::json!("codex"));
        assert_eq!(
            v["findings"],
            serde_json::json!("NEWEST_REVISE_FINDINGS_KEEP_CLUSTER_KEYS_IN_ONE_REGISTRY")
        );
        assert_eq!(v["major_count"], serde_json::json!(1));
    }

    #[test]
    fn executor_template_renders_external_review_findings_in_backpressure_section() {
        use crate::cli::dynamic::{BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};
        use crate::render::render_template_with_overlay;

        // Load the bundled tasks schema + executor template.
        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .expect("tasks schema");
        let schema = Schema::from_yaml(tasks_yaml).unwrap();

        let executor_tpl = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .and_then(|(_, ts)| {
                ts.iter()
                    .find(|(p, _)| *p == "templates/executor-brief.md.tpl")
                    .map(|(_, c)| *c)
            })
            .expect("executor template");

        // Minimal fixture entry (cycle 1, no in-cycle code-review backpressure yet).
        let entry: crate::validate::EntryMap = {
            let mut m = std::collections::BTreeMap::new();
            m.insert("display_id".to_string(), serde_json::json!("T107"));
            m.insert("status".to_string(), serde_json::json!("executing"));
            m.insert("title".to_string(), serde_json::json!("Test Task"));
            m.insert("slug".to_string(), serde_json::json!("test-task"));
            m.insert("current_phase".to_string(), serde_json::json!(1));
            m.insert("current_cycle".to_string(), serde_json::json!(1));
            m.insert(
                "contract".to_string(),
                serde_json::json!({
                    "done_when": "Feature ships",
                    "scope_in": "in",
                    "scope_out": "out",
                }),
            );
            m.insert(
                "plan".to_string(),
                serde_json::json!({
                    "phases": [
                        {
                            "name": "P1",
                            "objective": "do thing",
                            "tasks": ["t1"],
                            "acceptance_criteria": ["ac1"],
                        }
                    ]
                }),
            );
            m.insert("cycles".to_string(), serde_json::json!([]));
            m
        };

        let ctx = build_context(&schema, &entry);

        // Hand-build the overlay matching what build_external_review_overlay produces.
        let mut overlay: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();
        overlay.insert(
            "external_review_backpressure".to_string(),
            serde_json::json!({
                "display_id": "ER340",
                "runner": "codex",
                "verdict": "REVISE",
                "attempt": 6,
                "head_sha": "aa65090",
                "base_sha": "ed33d8d",
                "critical_count": 0,
                "major_count": 1,
                "minor_count": 0,
                "findings": "[major] Keep cluster keys in one registry structure — cluster_keys.rs:27-33\n\nCURATED_CLUSTER_KEY_PATTERNS still repeats the same five cluster-key strings.",
            }),
        );

        let rendered =
            render_template_with_overlay(executor_tpl, &ctx, &overlay).expect("executor render");

        // Section header is present.
        assert!(
            rendered.contains("## External Review Backpressure"),
            "missing External Review Backpressure section in: {rendered}"
        );
        // ER metadata visible.
        assert!(rendered.contains("ER340"), "missing ER id: {rendered}");
        assert!(rendered.contains("aa65090"), "missing head_sha: {rendered}");
        assert!(rendered.contains("ed33d8d"), "missing base_sha: {rendered}");
        assert!(rendered.contains("codex"), "missing runner: {rendered}");
        // Findings text is present (the literal cluster_keys.rs:27-33 fragment).
        assert!(
            rendered.contains("cluster_keys.rs:27-33"),
            "missing findings filename:line in: {rendered}"
        );
        assert!(
            rendered.contains("CURATED_CLUSTER_KEY_PATTERNS"),
            "missing findings body keyword in: {rendered}"
        );
    }

    #[test]
    fn executor_template_omits_external_review_section_when_overlay_null() {
        use crate::cli::dynamic::{BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};
        use crate::render::render_template_with_overlay;

        let tasks_yaml = BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(n, _)| *n == "tasks")
            .map(|(_, y)| *y)
            .expect("tasks schema");
        let schema = Schema::from_yaml(tasks_yaml).unwrap();

        let executor_tpl = BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(n, _)| *n == "tasks")
            .and_then(|(_, ts)| {
                ts.iter()
                    .find(|(p, _)| *p == "templates/executor-brief.md.tpl")
                    .map(|(_, c)| *c)
            })
            .expect("executor template");

        let entry: crate::validate::EntryMap = {
            let mut m = std::collections::BTreeMap::new();
            m.insert("display_id".to_string(), serde_json::json!("T200"));
            m.insert("status".to_string(), serde_json::json!("executing"));
            m.insert("title".to_string(), serde_json::json!("No-ER Task"));
            m.insert("slug".to_string(), serde_json::json!("no-er-task"));
            m.insert("current_phase".to_string(), serde_json::json!(1));
            m.insert("current_cycle".to_string(), serde_json::json!(1));
            m.insert(
                "contract".to_string(),
                serde_json::json!({
                    "done_when": "x",
                    "scope_in": "y",
                    "scope_out": "z",
                }),
            );
            m.insert(
                "plan".to_string(),
                serde_json::json!({
                    "phases": [{"name": "P", "objective": "o", "tasks": [], "acceptance_criteria": []}]
                }),
            );
            m.insert("cycles".to_string(), serde_json::json!([]));
            m
        };

        let ctx = build_context(&schema, &entry);

        let mut overlay: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::new();
        overlay.insert(
            "external_review_backpressure".to_string(),
            serde_json::Value::Null,
        );

        let rendered =
            render_template_with_overlay(executor_tpl, &ctx, &overlay).expect("executor render");

        assert!(
            !rendered.contains("## External Review Backpressure"),
            "section must NOT render when overlay is null: {rendered}"
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
