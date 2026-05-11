/// Workflow opt-in declaration block (Phase 2 — data model only, no engine).
///
/// A schema with no `workflow:` key leaves `Schema.workflow = None` and
/// behaves exactly as v0.1.  Nothing in this module alters store behaviour.
use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer};

// ---------------------------------------------------------------------------
// AgentRole
// ---------------------------------------------------------------------------

/// A named agent role — just a typed name binding with an optional description.
/// The role name must appear as a key in `briefing_templates`.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRole {
    pub name: String,
    pub description: Option<String>,
}

// ---------------------------------------------------------------------------
// StateAction
// ---------------------------------------------------------------------------

/// The kind of action in the `on_state` list for a given state.
///
/// The YAML representation uses tagged strings:
///   - `dispatch_agent: planner`   → `DispatchAgent("planner")`
///   - `increment: current_cycle`  → `Increment("current_cycle")`
///   - `transition_to: executing`  → `TransitionTo("executing")`
#[derive(Debug, Clone, PartialEq)]
pub enum StateActionKind {
    /// Dispatch the named agent role when entering this state.
    DispatchAgent(String),
    /// Auto-increment the named field when entering this state.
    Increment(String),
    /// Automatically transition to the named state when entering this state.
    TransitionTo(String),
}

/// An action in the `on_state` list for a given state, with an optional
/// guard expression that gates whether the action fires for a given row.
///
/// The YAML representation accepts an optional sibling `when:` key alongside
/// the action key:
///   - `{dispatch_agent: planner, when: "tier_hint != 'T1'"}`
///
/// When `when` is `None` the action always fires (T027 P1: parsing only —
/// execution paths do not yet consult the predicate).
#[derive(Debug, Clone, PartialEq)]
pub struct StateAction {
    pub kind: StateActionKind,
    pub when: Option<crate::schema::expr::Expr>,
}

impl StateAction {
    /// Convenience constructor: an unguarded action.
    pub fn new(kind: StateActionKind) -> Self {
        Self { kind, when: None }
    }
}

// ---------------------------------------------------------------------------
// Workflow (raw YAML shape — paths not yet resolved)
// ---------------------------------------------------------------------------

/// Opt-in workflow declaration parsed from the `workflow:` key in schema.yaml.
///
/// After install, callers work with [`WorkflowResolved`] where template paths
/// are replaced with their in-memory text contents.
#[derive(Debug, Clone, PartialEq)]
pub struct Workflow {
    /// Named agent roles.  Keys must match keys in `briefing_templates`.
    pub agent_roles: BTreeMap<String, AgentRole>,
    /// Map from role name to template file path (relative to store package root).
    pub briefing_templates: BTreeMap<String, std::path::PathBuf>,
    /// Path to the main.md render template (relative to store package root).
    pub render_template: Option<std::path::PathBuf>,
    /// Handlebars-templated output path for the rendered main.md
    /// e.g. `"tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md"`.
    pub render_target_path: Option<String>,
    /// Per-state action lists; keys must be existing lifecycle state names.
    pub on_state: BTreeMap<String, Vec<StateAction>>,
    /// Maps submit-verb name → field name the verb writes to.
    pub submit_targets: BTreeMap<String, String>,
    /// Maximum revise cycles before a BLOCKED guard fires.  Defaults to 3.
    pub max_revise_cycles: Option<u32>,
}

// ---------------------------------------------------------------------------
// WorkflowResolved — in-memory variant (templates as text, not paths)
// ---------------------------------------------------------------------------

/// In-memory representation of a workflow after install-time resolution.
/// Template files are read from disk once and embedded here; no runtime FS access.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowResolved {
    pub agent_roles: BTreeMap<String, AgentRole>,
    /// Map from role name to template *text* (was path in [`Workflow`]).
    pub briefing_templates: BTreeMap<String, String>,
    /// Render template *text* (was path in [`Workflow`]).
    pub render_template: Option<String>,
    pub render_target_path: Option<String>,
    pub on_state: BTreeMap<String, Vec<StateAction>>,
    pub submit_targets: BTreeMap<String, String>,
    pub max_revise_cycles: u32,
}

// ---------------------------------------------------------------------------
// Serde — raw structs for deserialization
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawAgentRole {
    #[serde(default)]
    description: Option<String>,
}

/// Raw single action entry in the YAML list.
/// Accepts either:
///   `dispatch_agent: planner`   (map with one key)
///   `increment: current_cycle`
///   `transition_to: executing`
#[derive(Debug)]
struct RawStateAction(StateAction);

impl<'de> Deserialize<'de> for RawStateAction {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = RawStateAction;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    "a state action map (dispatch_agent/increment/transition_to) with optional `when:` guard"
                )
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut kind: Option<StateActionKind> = None;
                let mut when_str: Option<String> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "dispatch_agent" => {
                            let v: String = map.next_value()?;
                            if kind.is_some() {
                                return Err(de::Error::custom(
                                    "state action map has multiple action keys; \
                                     expected exactly one of dispatch_agent / increment / transition_to",
                                ));
                            }
                            kind = Some(StateActionKind::DispatchAgent(v));
                        }
                        "increment" => {
                            let v: String = map.next_value()?;
                            if kind.is_some() {
                                return Err(de::Error::custom(
                                    "state action map has multiple action keys; \
                                     expected exactly one of dispatch_agent / increment / transition_to",
                                ));
                            }
                            kind = Some(StateActionKind::Increment(v));
                        }
                        "transition_to" => {
                            let v: String = map.next_value()?;
                            if kind.is_some() {
                                return Err(de::Error::custom(
                                    "state action map has multiple action keys; \
                                     expected exactly one of dispatch_agent / increment / transition_to",
                                ));
                            }
                            kind = Some(StateActionKind::TransitionTo(v));
                        }
                        "when" => {
                            let v: String = map.next_value()?;
                            when_str = Some(v);
                        }
                        other => {
                            return Err(de::Error::custom(format!(
                                "unknown state action key '{other}'; \
                                 expected dispatch_agent, increment, transition_to, or when"
                            )));
                        }
                    }
                }

                let kind = kind.ok_or_else(|| {
                    de::Error::custom(
                        "empty or guard-only state action map; \
                         expected one of dispatch_agent / increment / transition_to",
                    )
                })?;
                let when = match when_str {
                    Some(s) => Some(crate::schema::expr::parse_guard(&s).map_err(|e| {
                        de::Error::custom(format!(
                            "invalid `when:` guard expression {:?}: {}",
                            s, e
                        ))
                    })?),
                    None => None,
                };
                Ok(RawStateAction(StateAction { kind, when }))
            }
        }

        d.deserialize_map(V)
    }
}

#[derive(Debug, Deserialize)]
struct RawWorkflow {
    #[serde(default)]
    agent_roles: BTreeMap<String, RawAgentRole>,
    #[serde(default)]
    briefing_templates: BTreeMap<String, String>,
    #[serde(default)]
    render_template: Option<String>,
    #[serde(default)]
    render_target_path: Option<String>,
    #[serde(default)]
    on_state: BTreeMap<String, Vec<RawStateAction>>,
    #[serde(default)]
    submit_targets: BTreeMap<String, String>,
    #[serde(default)]
    max_revise_cycles: Option<u32>,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<RawAgentRole> for AgentRole {
    fn from(r: RawAgentRole) -> Self {
        // The name is the map key; caller fills it in.
        AgentRole {
            name: String::new(), // filled by Workflow::from_raw
            description: r.description,
        }
    }
}

impl Workflow {
    /// Parse a `Workflow` from raw YAML deserialization output.
    fn from_raw(raw: RawWorkflow) -> Self {
        let agent_roles: BTreeMap<String, AgentRole> = raw
            .agent_roles
            .into_iter()
            .map(|(k, v)| {
                let mut role = AgentRole::from(v);
                role.name = k.clone();
                (k, role)
            })
            .collect();

        let briefing_templates: BTreeMap<String, std::path::PathBuf> = raw
            .briefing_templates
            .into_iter()
            .map(|(k, v)| (k, std::path::PathBuf::from(v)))
            .collect();

        let render_template = raw.render_template.map(std::path::PathBuf::from);

        let on_state: BTreeMap<String, Vec<StateAction>> = raw
            .on_state
            .into_iter()
            .map(|(state, actions)| {
                let acts: Vec<StateAction> = actions.into_iter().map(|a| a.0).collect();
                (state, acts)
            })
            .collect();

        Workflow {
            agent_roles,
            briefing_templates,
            render_template,
            render_target_path: raw.render_target_path,
            on_state,
            submit_targets: raw.submit_targets,
            max_revise_cycles: raw.max_revise_cycles,
        }
    }
}

impl<'de> Deserialize<'de> for Workflow {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawWorkflow::deserialize(d)?;
        Ok(Workflow::from_raw(raw))
    }
}

// ---------------------------------------------------------------------------
// Resolution — path → text
// ---------------------------------------------------------------------------

impl Workflow {
    /// Resolve a `Workflow` into a `WorkflowResolved` by reading template files
    /// from `store_root`.  Called at install time for path-installed stores.
    pub fn resolve_from_disk(
        &self,
        store_root: &std::path::Path,
    ) -> anyhow::Result<WorkflowResolved> {
        let mut briefing_templates = BTreeMap::new();
        for (role, path) in &self.briefing_templates {
            let full = store_root.join(path);
            let text = std::fs::read_to_string(&full).map_err(|e| {
                anyhow::anyhow!(
                    "workflow: cannot read briefing template '{}' for role '{}': {}",
                    full.display(),
                    role,
                    e
                )
            })?;
            briefing_templates.insert(role.clone(), text);
        }

        let render_template = self
            .render_template
            .as_ref()
            .map(|path| {
                let full = store_root.join(path);
                std::fs::read_to_string(&full).map_err(|e| {
                    anyhow::anyhow!(
                        "workflow: cannot read render template '{}': {}",
                        full.display(),
                        e
                    )
                })
            })
            .transpose()?;

        Ok(WorkflowResolved {
            agent_roles: self.agent_roles.clone(),
            briefing_templates,
            render_template,
            render_target_path: self.render_target_path.clone(),
            on_state: self.on_state.clone(),
            submit_targets: self.submit_targets.clone(),
            max_revise_cycles: self.max_revise_cycles.unwrap_or(3),
        })
    }

    /// Build a `WorkflowResolved` from pre-loaded template strings (for bundled
    /// stores where templates are embedded via `include_str!` at compile time).
    pub fn resolve_from_strings(
        &self,
        briefing_texts: BTreeMap<String, String>,
        render_text: Option<String>,
    ) -> WorkflowResolved {
        WorkflowResolved {
            agent_roles: self.agent_roles.clone(),
            briefing_templates: briefing_texts,
            render_template: render_text,
            render_target_path: self.render_target_path.clone(),
            on_state: self.on_state.clone(),
            submit_targets: self.submit_targets.clone(),
            max_revise_cycles: self.max_revise_cycles.unwrap_or(3),
        }
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Known submit verb names for validation.
const SUBMIT_VERBS: &[&str] = &[
    "submit-plan",
    "submit-execute",
    "submit-review",
    "submit-plan-review",
    "submit-wrap",
];

impl Workflow {
    /// Validate cross-references within the workflow block itself:
    ///   - every `agent_roles` key has a `briefing_templates` entry
    ///   - every `on_state` key is an existing lifecycle state
    ///   - every `DispatchAgent(role)` references an existing `agent_roles` key
    ///   - `submit_targets` keys are known submit verb names
    ///   - `submit_targets` values name existing fields (field lookup provided by caller)
    ///   - `submit_targets["submit-plan"]` must target a `record` field
    ///   - other submit verbs must target a `list_record` or `record` field
    pub fn validate_cross_refs(
        &self,
        lifecycle_states: &[String],
        field_lookup: &dyn Fn(&str) -> Option<FieldShape>,
    ) -> anyhow::Result<()> {
        // 1. Every agent_role key must have a briefing_templates entry.
        for role_key in self.agent_roles.keys() {
            if !self.briefing_templates.contains_key(role_key.as_str()) {
                anyhow::bail!(
                    "workflow: agent_role '{}' has no entry in briefing_templates",
                    role_key
                );
            }
        }

        // 2. Every on_state key must be an existing lifecycle state.
        for state_key in self.on_state.keys() {
            if !lifecycle_states.iter().any(|s| s == state_key) {
                anyhow::bail!(
                    "workflow: on_state references unknown lifecycle state '{}'",
                    state_key
                );
            }
        }

        // 3. Every DispatchAgent(role) must reference an existing agent_roles key.
        for (state, actions) in &self.on_state {
            for action in actions {
                if let StateActionKind::DispatchAgent(role) = &action.kind {
                    if !self.agent_roles.contains_key(role.as_str()) {
                        anyhow::bail!(
                            "workflow: on_state['{}'] references unknown agent role '{}' \
                             in DispatchAgent",
                            state,
                            role
                        );
                    }
                }
            }
        }

        // 4. submit_targets validation.
        for (verb, field_name) in &self.submit_targets {
            // 4a. Key must be a known submit verb.
            if !SUBMIT_VERBS.contains(&verb.as_str()) {
                anyhow::bail!(
                    "workflow: submit_targets key '{}' is not a known submit verb; \
                     expected one of: {}",
                    verb,
                    SUBMIT_VERBS.join(", ")
                );
            }

            // 4b. Value must name an existing field.
            let shape = field_lookup(field_name).ok_or_else(|| {
                anyhow::anyhow!(
                    "workflow: submit_targets['{}'] references non-existent field '{}'",
                    verb,
                    field_name
                )
            })?;

            // 4c. Type-shape checks.
            match verb.as_str() {
                "submit-plan" => {
                    if shape != FieldShape::Record {
                        anyhow::bail!(
                            "workflow: submit_targets['submit-plan'] points at field '{}' \
                             which has type {:?}; submit-plan requires a 'record' field",
                            field_name,
                            shape
                        );
                    }
                }
                "submit-execute" | "submit-review" | "submit-plan-review" => {
                    if shape != FieldShape::Record && shape != FieldShape::ListRecord {
                        anyhow::bail!(
                            "workflow: submit_targets['{}'] points at field '{}' \
                             which has type {:?}; this verb requires a 'list_record' or 'record' field",
                            verb,
                            field_name,
                            shape
                        );
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }
}

/// Coarse field-shape descriptor used for submit_targets validation.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldShape {
    Record,
    ListRecord,
    Other,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_workflow_yaml() -> &'static str {
        r#"
agent_roles:
  planner:
    description: "Creates the implementation plan"
  executor:
    description: "Implements the plan"
briefing_templates:
  planner: templates/planner-brief.md.tpl
  executor: templates/executor-brief.md.tpl
render_template: templates/main.md.tpl
render_target_path: "tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md"
on_state:
  planning:
    - dispatch_agent: planner
  executing:
    - dispatch_agent: executor
    - increment: current_cycle
submit_targets:
  submit-plan: plan
  submit-execute: cycles
max_revise_cycles: 3
"#
    }

    #[test]
    fn workflow_parses_minimal() {
        let w: Workflow = serde_yaml::from_str(minimal_workflow_yaml()).unwrap();
        assert_eq!(w.agent_roles.len(), 2);
        assert!(w.agent_roles.contains_key("planner"));
        assert!(w.agent_roles.contains_key("executor"));
        assert_eq!(
            w.briefing_templates["planner"],
            std::path::PathBuf::from("templates/planner-brief.md.tpl")
        );
        assert_eq!(
            w.render_template,
            Some(std::path::PathBuf::from("templates/main.md.tpl"))
        );
        assert_eq!(
            w.render_target_path.as_deref(),
            Some("tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md")
        );
        assert_eq!(w.max_revise_cycles, Some(3));
    }

    #[test]
    fn workflow_on_state_actions_parse() {
        let w: Workflow = serde_yaml::from_str(minimal_workflow_yaml()).unwrap();
        let executing = w.on_state.get("executing").unwrap();
        assert_eq!(executing.len(), 2);
        assert_eq!(
            executing[0],
            StateAction::new(StateActionKind::DispatchAgent("executor".to_string()))
        );
        assert_eq!(
            executing[1],
            StateAction::new(StateActionKind::Increment("current_cycle".to_string()))
        );
    }

    #[test]
    fn workflow_submit_targets_parse() {
        let w: Workflow = serde_yaml::from_str(minimal_workflow_yaml()).unwrap();
        assert_eq!(w.submit_targets["submit-plan"], "plan");
        assert_eq!(w.submit_targets["submit-execute"], "cycles");
    }

    #[test]
    fn workflow_agent_role_names_filled() {
        let w: Workflow = serde_yaml::from_str(minimal_workflow_yaml()).unwrap();
        assert_eq!(w.agent_roles["planner"].name, "planner");
        assert_eq!(w.agent_roles["executor"].name, "executor");
    }

    #[test]
    fn workflow_validate_unknown_on_state_errors() {
        let w: Workflow = serde_yaml::from_str(minimal_workflow_yaml()).unwrap();
        let states = vec!["open".to_string()]; // "planning" and "executing" missing
        let err = w.validate_cross_refs(&states, &|_| None).unwrap_err();
        assert!(
            err.to_string().contains("unknown lifecycle state"),
            "err: {err}"
        );
    }

    #[test]
    fn workflow_validate_unknown_dispatch_agent_errors() {
        let yaml = r#"
agent_roles:
  planner:
    description: "Planner"
briefing_templates:
  planner: templates/planner-brief.md.tpl
on_state:
  planning:
    - dispatch_agent: ghost_role
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let states = vec!["planning".to_string()];
        let err = w.validate_cross_refs(&states, &|_| None).unwrap_err();
        assert!(
            err.to_string().contains("unknown agent role 'ghost_role'"),
            "err: {err}"
        );
    }

    #[test]
    fn workflow_validate_missing_briefing_template_for_role_errors() {
        let yaml = r#"
agent_roles:
  planner:
    description: "Planner"
  executor:
    description: "Executor"
briefing_templates:
  planner: templates/planner-brief.md.tpl
on_state: {}
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let states: Vec<String> = vec![];
        let err = w.validate_cross_refs(&states, &|_| None).unwrap_err();
        assert!(
            err.to_string().contains("executor") && err.to_string().contains("briefing_templates"),
            "err: {err}"
        );
    }

    #[test]
    fn workflow_validate_submit_targets_unknown_field_errors() {
        let yaml = r#"
agent_roles: {}
briefing_templates: {}
on_state: {}
submit_targets:
  submit-plan: nonexistent_field
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let states: Vec<String> = vec![];
        let err = w.validate_cross_refs(&states, &|_| None).unwrap_err();
        assert!(err.to_string().contains("nonexistent_field"), "err: {err}");
    }

    #[test]
    fn workflow_validate_submit_plan_wrong_type_errors() {
        let yaml = r#"
agent_roles: {}
briefing_templates: {}
on_state: {}
submit_targets:
  submit-plan: cycles_field
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let states: Vec<String> = vec![];
        // cycles_field exists but is a list_record
        let lookup = |name: &str| {
            if name == "cycles_field" {
                Some(FieldShape::ListRecord)
            } else {
                None
            }
        };
        let err = w.validate_cross_refs(&states, &lookup).unwrap_err();
        assert!(
            err.to_string().contains("submit-plan") && err.to_string().contains("record"),
            "err: {err}"
        );
    }

    #[test]
    fn workflow_validate_submit_execute_accepts_list_record() {
        let yaml = r#"
agent_roles: {}
briefing_templates: {}
on_state: {}
submit_targets:
  submit-execute: cycles_field
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let states: Vec<String> = vec![];
        let lookup = |name: &str| {
            if name == "cycles_field" {
                Some(FieldShape::ListRecord)
            } else {
                None
            }
        };
        assert!(w.validate_cross_refs(&states, &lookup).is_ok());
    }

    #[test]
    fn workflow_validate_submit_execute_wrong_type_errors() {
        let yaml = r#"
agent_roles: {}
briefing_templates: {}
on_state: {}
submit_targets:
  submit-execute: title_field
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let states: Vec<String> = vec![];
        let lookup = |name: &str| {
            if name == "title_field" {
                Some(FieldShape::Other)
            } else {
                None
            }
        };
        let err = w.validate_cross_refs(&states, &lookup).unwrap_err();
        assert!(
            err.to_string().contains("submit-execute") && err.to_string().contains("list_record"),
            "err: {err}"
        );
    }

    #[test]
    fn workflow_empty_block_parses() {
        let yaml = "{}";
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        assert!(w.agent_roles.is_empty());
        assert!(w.on_state.is_empty());
        assert!(w.submit_targets.is_empty());
        assert!(w.max_revise_cycles.is_none());
    }

    #[test]
    fn state_action_transition_to_parses() {
        let yaml = r#"
agent_roles: {}
briefing_templates: {}
on_state:
  planning:
    - transition_to: executing
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let actions = w.on_state.get("planning").unwrap();
        assert_eq!(
            actions[0],
            StateAction::new(StateActionKind::TransitionTo("executing".to_string()))
        );
    }

    // ---- Task 2.3+2.4: resolve_from_disk ----

    #[test]
    fn resolve_from_disk_reads_templates() {
        // Use the workflow_minimal fixture directory
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/workflow_minimal");
        let yaml = std::fs::read_to_string(fixture.join("schema.yaml")).unwrap();
        // Extract the workflow sub-block from the fixture YAML
        let full: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
        let wf_yaml = serde_yaml::to_string(full.get("workflow").unwrap()).unwrap();
        let wf: Workflow = serde_yaml::from_str(&wf_yaml).unwrap();
        let resolved = wf.resolve_from_disk(&fixture).unwrap();
        assert!(resolved.briefing_templates.contains_key("planner"));
        assert!(resolved.briefing_templates.contains_key("executor"));
        assert!(resolved.briefing_templates["planner"].contains("Planner Briefing"));
        assert!(resolved.render_template.is_some());
        assert_eq!(resolved.max_revise_cycles, 5);
    }

    #[test]
    fn resolve_from_disk_missing_template_errors() {
        let yaml = r#"
agent_roles:
  planner: {}
briefing_templates:
  planner: templates/nonexistent.md.tpl
on_state: {}
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let err = wf.resolve_from_disk(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("nonexistent.md.tpl"), "err: {err}");
    }

    #[test]
    fn resolve_from_strings_sets_max_revise_cycles_default() {
        let yaml = r#"
agent_roles: {}
briefing_templates: {}
on_state: {}
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        let resolved = wf.resolve_from_strings(BTreeMap::new(), None);
        // default when max_revise_cycles is None
        assert_eq!(resolved.max_revise_cycles, 5);
    }

    // ---- T027 P1: optional `when:` guard on state actions ----

    #[test]
    fn state_action_when_guard_parses() {
        let yaml = r#"
agent_roles:
  planner: {}
briefing_templates:
  planner: templates/p.md.tpl
on_state:
  planning:
    - dispatch_agent: planner
      when: "tier_hint != 'T1'"
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let actions = w.on_state.get("planning").unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(
            actions[0].kind,
            StateActionKind::DispatchAgent("planner".to_string())
        );
        let guard = actions[0].when.as_ref().expect("when must be Some");
        // Round-trip via parse_guard so we don't depend on the AST internals here.
        let expected = crate::schema::expr::parse_guard("tier_hint != 'T1'").unwrap();
        assert_eq!(guard, &expected);
    }

    #[test]
    fn state_action_without_when_is_none() {
        let yaml = r#"
agent_roles:
  planner: {}
briefing_templates:
  planner: templates/p.md.tpl
on_state:
  planning:
    - dispatch_agent: planner
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let actions = w.on_state.get("planning").unwrap();
        assert!(actions[0].when.is_none());
    }

    #[test]
    fn state_action_invalid_when_errors() {
        let yaml = r#"
agent_roles:
  planner: {}
briefing_templates:
  planner: templates/p.md.tpl
on_state:
  planning:
    - dispatch_agent: planner
      when: "tier_hint && other"
"#;
        let err = serde_yaml::from_str::<Workflow>(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid `when:` guard expression"),
            "err: {msg}"
        );
    }

    #[test]
    fn state_action_unknown_sibling_key_errors() {
        let yaml = r#"
agent_roles:
  planner: {}
briefing_templates:
  planner: templates/p.md.tpl
on_state:
  planning:
    - dispatch_agent: planner
      bogus: "x"
"#;
        let err = serde_yaml::from_str::<Workflow>(yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown state action key 'bogus'"),
            "err: {msg}"
        );
    }

    #[test]
    fn state_action_when_round_trips_with_transition_to() {
        let yaml = r#"
agent_roles: {}
briefing_templates: {}
on_state:
  planning:
    - transition_to: ready
      when: "tier_hint == 'T1'"
"#;
        let w: Workflow = serde_yaml::from_str(yaml).unwrap();
        let actions = w.on_state.get("planning").unwrap();
        assert_eq!(
            actions[0].kind,
            StateActionKind::TransitionTo("ready".to_string())
        );
        assert!(actions[0].when.is_some());
    }

    #[test]
    fn resolve_from_strings_custom_max_revise_cycles() {
        let yaml = r#"
agent_roles: {}
briefing_templates: {}
on_state: {}
max_revise_cycles: 5
"#;
        let wf: Workflow = serde_yaml::from_str(yaml).unwrap();
        let resolved = wf.resolve_from_strings(BTreeMap::new(), None);
        assert_eq!(resolved.max_revise_cycles, 5);
    }
}
