pub mod actor;
pub mod expr;
pub mod flatten;
pub mod lifecycle;
pub mod parse;
pub mod required_when;
pub mod risk_taxonomy;
pub mod types;
pub mod workflow;

pub use actor::Actor;
pub use lifecycle::{Lifecycle, Transition};
pub use required_when::Expr as RequiredWhenExpr;
pub use workflow::{FieldShape, StateAction, StateActionKind, Workflow, WorkflowResolved};

use serde::{Deserialize, Deserializer};

// ---------------------------------------------------------------------------
// StoreScope  (Task 1.5)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreScope {
    /// Default: `.stores/` lives at the current worktree root (cwd).
    Worktree,
    /// `.stores/` lives at the canonical `.git/` common-dir parent (repo-wide).
    Repo,
    /// `.stores/` lives in `$HOME/.stores` (user-global).
    User,
}

impl<'de> Deserialize<'de> for StoreScope {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        match s.as_str() {
            "worktree" => Ok(StoreScope::Worktree),
            "repo" => Ok(StoreScope::Repo),
            "user" => Ok(StoreScope::User),
            other => Err(serde::de::Error::custom(format!(
                "unknown scope '{other}'; expected one of: worktree, repo, user"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// FieldType
// ---------------------------------------------------------------------------

/// All supported field types. Defined here (not types.rs) to avoid the
/// circular dependency between FieldType::Record and Field.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldType {
    Text,
    Integer,
    Bool,
    Enum(Vec<String>),
    List(Box<FieldType>),
    Record(Vec<Field>),
    DisplayId,
    Timestamp,
    /// A list of inline sub-records (stored as JSON TEXT).  Each element has
    /// the same shape defined by `Vec<Field>`.  Elements may themselves
    /// contain Record / ListRecord nests (depth ≥ 3 supported).  (Task 1.7)
    ListRecord(Vec<Field>),
    /// A soft foreign-key list stored as JSON TEXT containing an array of
    /// display_ids referencing another store.  No referential-integrity
    /// enforcement at write time.  (Task 1.8)
    ListFk {
        ref_store: String,
    },
    /// An opaque free-shape JSON payload stored as TEXT.  The framework does
    /// not impose any sub-key shape; value must be parseable JSON.  Valid at the
    /// top level and as a `list_record` sub-field (e.g. snapshot fields); NOT
    /// valid inside nested `record:` sub-fields (CLI coerce_value doesn't
    /// traverse deep record nesting).  (T008; updated Decision 1 in T111)
    Json,
}

// ---------------------------------------------------------------------------
// Field
// ---------------------------------------------------------------------------

/// A single field in a store schema, including optional sub-fields when
/// `ty == Record(...)` or `ty == ListRecord(...)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: FieldType,
    pub required: bool,
    pub required_when: Option<RequiredWhenExpr>,
    pub pattern: Option<String>,
    pub actor: Option<Actor>,
    pub enum_values: Option<Vec<String>>,
    pub description: Option<String>,
    /// Declarative: when true the engine auto-bumps this integer on each cycle.
    /// Validated: must be `ty == Integer` AND `actor == Some(Framework)`.  (Task 1.2)
    pub auto_increment: bool,
    /// Declarative: this field resets to 1 whenever the named field increments.
    /// Validated: target must exist, be an integer with actor==Framework, and
    /// must not name itself.  (Task 1.2)
    pub auto_increment_within: Option<String>,
    /// Declarative: per-field default value applied at two layers:
    /// (a) DDL codegen emits `DEFAULT '<val>'` so ALTER TABLE ADD COLUMN
    /// backfills existing rows on migration; (b) add handler materialises the
    /// default into the entry post-validation when the CLI did not supply a
    /// value.  (T052 P1)
    pub default: Option<serde_json::Value>,
    /// Declarative: for `List(Text)` fields, the allowed set of string values
    /// for each element.  Parsed from the `list_enum:` YAML key.  (T052 P2)
    pub list_enum: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Schema {
    pub name: String,
    pub id_format: String,
    pub fields: Vec<Field>,
    pub lifecycle: Lifecycle,
    /// Store-level default actor (individual fields may override).
    pub default_actor: Option<Actor>,
    /// Storage scope resolution hint.  Absent → Worktree (v0.1 default).  (Task 1.5)
    pub scope: StoreScope,
    /// Optional workflow declaration.  `None` for v0.1 stores (no `workflow:` key).
    pub workflow: Option<Workflow>,
}

// ---------------------------------------------------------------------------
// Deserialisation helpers (raw structs for serde_yaml, then converted)
// ---------------------------------------------------------------------------

/// Raw YAML representation of a field (before transformation).
#[derive(Debug, Deserialize)]
struct RawField {
    name: String,
    #[serde(rename = "type")]
    ty: RawFieldType,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    required_when: Option<String>,
    #[serde(default)]
    pattern: Option<String>,
    #[serde(default)]
    actor: Option<Actor>,
    #[serde(default)]
    enum_values: Option<Vec<String>>,
    #[serde(default)]
    description: Option<String>,
    /// Sub-fields for Record / ListRecord type
    #[serde(default)]
    fields: Option<Vec<RawField>>,
    /// For list_fk: the store name being referenced.
    #[serde(rename = "ref", default)]
    ref_store: Option<String>,
    /// auto_increment attribute (Task 1.2)
    #[serde(default)]
    auto_increment: bool,
    /// auto_increment_within attribute (Task 1.2)
    #[serde(default)]
    auto_increment_within: Option<String>,
    /// Per-field default value (T052 P1).
    #[serde(default)]
    default: Option<serde_json::Value>,
    /// Allowed values for each element in a List(Text) field (T052 P2).
    #[serde(default)]
    list_enum: Option<Vec<String>>,
}

/// Raw field type before full resolution.
#[derive(Debug)]
enum RawFieldType {
    Scalar(String),
    ListOf(String),
    Record,
    ListRecord,
    ListFk,
}

impl<'de> Deserialize<'de> for RawFieldType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct RFTVisitor;

        impl<'de> Visitor<'de> for RFTVisitor {
            type Value = RawFieldType;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a field type string or mapping")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<RawFieldType, E> {
                match v {
                    "list_record" => Ok(RawFieldType::ListRecord),
                    "list_fk" => Ok(RawFieldType::ListFk),
                    other => Ok(RawFieldType::Scalar(other.to_string())),
                }
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<RawFieldType, E> {
                match v.as_str() {
                    "list_record" => Ok(RawFieldType::ListRecord),
                    "list_fk" => Ok(RawFieldType::ListFk),
                    _ => Ok(RawFieldType::Scalar(v)),
                }
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<RawFieldType, A::Error> {
                // Expect exactly one key: "list", "record", "list_record", or "list_fk"
                let key: String = map
                    .next_key()?
                    .ok_or_else(|| de::Error::custom("empty type map"))?;
                match key.as_str() {
                    "list" => {
                        let inner: String = map.next_value()?;
                        Ok(RawFieldType::ListOf(inner))
                    }
                    "record" => {
                        // consume the value (sub-fields come from the parent `fields` key)
                        let _: serde::de::IgnoredAny = map.next_value()?;
                        Ok(RawFieldType::Record)
                    }
                    "list_record" => {
                        let _: serde::de::IgnoredAny = map.next_value()?;
                        Ok(RawFieldType::ListRecord)
                    }
                    "list_fk" => {
                        let _: serde::de::IgnoredAny = map.next_value()?;
                        Ok(RawFieldType::ListFk)
                    }
                    other => Err(de::Error::custom(format!("unknown type map key '{other}'"))),
                }
            }
        }

        d.deserialize_any(RFTVisitor)
    }
}

fn resolve_field_type(
    raw_ty: &RawFieldType,
    enum_values: &Option<Vec<String>>,
    sub_fields: &Option<Vec<RawField>>,
    ref_store: &Option<String>,
) -> anyhow::Result<FieldType> {
    match raw_ty {
        RawFieldType::Scalar(s) => match s.as_str() {
            "text" => Ok(FieldType::Text),
            "integer" => Ok(FieldType::Integer),
            "bool" => Ok(FieldType::Bool),
            "display_id" => Ok(FieldType::DisplayId),
            "timestamp" => Ok(FieldType::Timestamp),
            "enum" => {
                let vals = enum_values.clone().ok_or_else(|| {
                    anyhow::anyhow!("field type 'enum' requires 'enum_values' list")
                })?;
                Ok(FieldType::Enum(vals))
            }
            "record" => {
                let subs = sub_fields.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("field type 'record' requires a 'fields' list")
                })?;
                let converted: anyhow::Result<Vec<Field>> =
                    subs.iter().map(raw_to_field).collect();
                Ok(FieldType::Record(converted?))
            }
            "list_record" => {
                let subs = sub_fields.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("field type 'list_record' requires a 'fields' list")
                })?;
                let converted: anyhow::Result<Vec<Field>> =
                    subs.iter().map(raw_to_field).collect();
                Ok(FieldType::ListRecord(converted?))
            }
            "list_fk" => {
                let store = ref_store.clone().ok_or_else(|| {
                    anyhow::anyhow!("field type 'list_fk' requires a 'ref' store name")
                })?;
                Ok(FieldType::ListFk { ref_store: store })
            }
            "json" => Ok(FieldType::Json),
            other => anyhow::bail!(
                "unknown field type '{other}'; expected one of: text, integer, bool, enum, list, record, list_record, list_fk, display_id, timestamp, json"
            ),
        },
        RawFieldType::ListOf(inner) => {
            // inner must be a scalar type name
            let inner_ty = resolve_field_type(
                &RawFieldType::Scalar(inner.clone()),
                &None,
                &None,
                &None,
            )?;
            Ok(FieldType::List(Box::new(inner_ty)))
        }
        RawFieldType::Record => {
            let subs = sub_fields.as_ref().ok_or_else(|| {
                anyhow::anyhow!("field type 'record' (map form) requires a 'fields' list")
            })?;
            let converted: anyhow::Result<Vec<Field>> = subs.iter().map(raw_to_field).collect();
            Ok(FieldType::Record(converted?))
        }
        RawFieldType::ListRecord => {
            let subs = sub_fields.as_ref().ok_or_else(|| {
                anyhow::anyhow!("field type 'list_record' requires a 'fields' list")
            })?;
            let converted: anyhow::Result<Vec<Field>> = subs.iter().map(raw_to_field).collect();
            Ok(FieldType::ListRecord(converted?))
        }
        RawFieldType::ListFk => {
            let store = ref_store.clone().ok_or_else(|| {
                anyhow::anyhow!("field type 'list_fk' requires a 'ref' store name")
            })?;
            Ok(FieldType::ListFk { ref_store: store })
        }
    }
}

fn raw_to_field(r: &RawField) -> anyhow::Result<Field> {
    let ty = resolve_field_type(&r.ty, &r.enum_values, &r.fields, &r.ref_store)?;
    // Decision 1 (updated): reject `json` inside record sub-fields; allow it in
    // list_record sub-fields where JSON-snapshot fields (e.g. reviewed_plan) are
    // legitimate.  record-of-record nesting with json is still forbidden because
    // the CLI coerce_value path does not traverse deep nested objects and would
    // silently mis-encode raw-string sentinels there.
    if let FieldType::Record(subs) = &ty {
        for sub in subs {
            if sub.ty == FieldType::Json {
                anyhow::bail!(
                    "field '{}.{}': type 'json' may only appear at the top level",
                    r.name,
                    sub.name
                );
            }
        }
    }
    let required_when = r
        .required_when
        .as_deref()
        .map(required_when::parse)
        .transpose()?;
    Ok(Field {
        name: r.name.clone(),
        ty,
        required: r.required,
        required_when,
        pattern: r.pattern.clone(),
        actor: r.actor,
        enum_values: r.enum_values.clone(),
        description: r.description.clone(),
        auto_increment: r.auto_increment,
        auto_increment_within: r.auto_increment_within.clone(),
        default: r.default.clone(),
        list_enum: r.list_enum.clone(),
    })
}

/// Raw YAML schema root.
#[derive(Debug, Deserialize)]
struct RawSchema {
    name: String,
    id_format: String,
    #[serde(default)]
    default_actor: Option<Actor>,
    fields: Vec<RawField>,
    lifecycle: Lifecycle,
    #[serde(default)]
    scope: Option<StoreScope>,
    #[serde(default)]
    workflow: Option<Workflow>,
}

impl Schema {
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Schema> {
        validate_transition_guard_order(yaml)?;

        let raw: RawSchema =
            serde_yaml::from_str(yaml).map_err(|e| anyhow::anyhow!("YAML parse error: {}", e))?;

        // Validate id_format
        crate::id_format::validate(&raw.id_format)?;

        let fields: anyhow::Result<Vec<Field>> = raw.fields.iter().map(raw_to_field).collect();
        let fields = fields?;

        // Validate auto_increment fields (Task 1.2)
        validate_auto_increment(&fields)?;

        // Validate transition ambiguity (Task 1.10)
        raw.lifecycle.validate_transition_ambiguity()?;

        // Validate workflow cross-references (Task 2.5)
        if let Some(ref wf) = raw.workflow {
            let field_lookup = |name: &str| {
                fields.iter().find(|f| f.name == name).map(|f| match &f.ty {
                    FieldType::Record(_) => workflow::FieldShape::Record,
                    FieldType::ListRecord(_) => workflow::FieldShape::ListRecord,
                    _ => workflow::FieldShape::Other,
                })
            };
            wf.validate_cross_refs(&raw.lifecycle.states, &field_lookup)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
        }

        Ok(Schema {
            name: raw.name,
            id_format: raw.id_format,
            fields,
            lifecycle: raw.lifecycle,
            default_actor: raw.default_actor,
            scope: raw.scope.unwrap_or(StoreScope::Worktree),
            workflow: raw.workflow,
        })
    }
}

#[derive(Default)]
struct RawTransitionOrder {
    line: usize,
    from: Option<String>,
    verb: Option<String>,
    requires_gate: Option<String>,
    has_guard: bool,
}

/// Validate YAML transition order before serde drops source locations.
/// For each runtime selector bucket `(from, verb, requires_gate)`, guarded
/// variants must precede the unguarded fallback. An earlier unguarded fallback
/// shadows later guards because runtime selection returns the first fallback
/// after no guard evaluates true.
fn validate_transition_guard_order(yaml: &str) -> anyhow::Result<()> {
    use std::collections::HashMap;

    let mut in_transitions = false;
    let mut transitions_indent = 0usize;
    let mut current: Option<RawTransitionOrder> = None;
    let mut transitions: Vec<RawTransitionOrder> = Vec::new();

    fn indent_of(line: &str) -> usize {
        line.len() - line.trim_start().len()
    }

    fn strip_comment(line: &str) -> &str {
        line.split_once('#').map(|(left, _)| left).unwrap_or(line)
    }

    fn clean_value(raw: &str) -> String {
        raw.trim()
            .trim_end_matches(',')
            .trim_matches('}')
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .to_string()
    }

    fn apply_kv(t: &mut RawTransitionOrder, key: &str, value: &str) {
        match key.trim() {
            "from" => t.from = Some(clean_value(value)),
            "verb" => t.verb = Some(clean_value(value)),
            "requires_gate" => t.requires_gate = Some(clean_value(value)),
            "guard" | "when" => t.has_guard = true,
            _ => {}
        }
    }

    fn parse_pairs_into(t: &mut RawTransitionOrder, text: &str) {
        let s = text.trim().trim_start_matches('{').trim_end_matches('}');
        for part in s.split(',') {
            if let Some((key, value)) = part.split_once(':') {
                apply_kv(t, key, value);
            }
        }
    }

    for (idx, raw_line) in yaml.lines().enumerate() {
        let line_no = idx + 1;
        let uncommented = strip_comment(raw_line);
        let trimmed = uncommented.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = indent_of(uncommented);

        if !in_transitions {
            if trimmed == "transitions:" {
                in_transitions = true;
                transitions_indent = indent;
            }
            continue;
        }

        if indent <= transitions_indent && !trimmed.starts_with('-') {
            if let Some(t) = current.take() {
                transitions.push(t);
            }
            break;
        }

        if trimmed.starts_with("-") {
            if let Some(t) = current.take() {
                transitions.push(t);
            }
            let mut t = RawTransitionOrder {
                line: line_no,
                ..Default::default()
            };
            parse_pairs_into(&mut t, trimmed.trim_start_matches('-').trim());
            current = Some(t);
        } else if let Some(t) = current.as_mut() {
            parse_pairs_into(t, trimmed);
        }
    }

    if let Some(t) = current.take() {
        transitions.push(t);
    }

    let mut first_unguarded: HashMap<(String, String, Option<String>), usize> = HashMap::new();
    for t in transitions {
        let (Some(from), Some(verb)) = (t.from, t.verb) else {
            continue;
        };
        let key = (from, verb, t.requires_gate);
        if t.has_guard {
            if let Some(line) = first_unguarded.get(&key) {
                let gate = key.2.as_deref().unwrap_or("<none>");
                anyhow::bail!(
                    "transition guard order violation: unguarded transition at line {} shadows later guarded transition for (from='{}', verb='{}', requires_gate={}); place guarded transitions before the unguarded fallback",
                    line,
                    key.0,
                    key.1,
                    gate
                );
            }
        } else {
            first_unguarded.entry(key).or_insert(t.line);
        }
    }

    Ok(())
}

/// Validate auto_increment / auto_increment_within constraints (Task 1.2).
fn validate_auto_increment(fields: &[Field]) -> anyhow::Result<()> {
    for field in fields {
        if field.auto_increment {
            // Must be Integer
            if field.ty != FieldType::Integer {
                anyhow::bail!(
                    "field '{}': auto_increment requires type 'integer', got '{:?}'",
                    field.name,
                    field.ty
                );
            }
            // Must have actor: framework
            if field.actor != Some(Actor::Framework) {
                anyhow::bail!(
                    "field '{}': auto_increment requires actor 'framework'",
                    field.name
                );
            }
        }

        if let Some(within) = &field.auto_increment_within {
            // Cannot name itself
            if within == &field.name {
                anyhow::bail!(
                    "field '{}': auto_increment_within cannot reference itself",
                    field.name
                );
            }
            // Target must exist as a top-level integer field with actor: framework
            let target = fields.iter().find(|f| &f.name == within);
            match target {
                None => anyhow::bail!(
                    "field '{}': auto_increment_within target '{}' not found in schema",
                    field.name,
                    within
                ),
                Some(t) => {
                    if t.ty != FieldType::Integer {
                        anyhow::bail!(
                            "field '{}': auto_increment_within target '{}' must be type 'integer'",
                            field.name,
                            within
                        );
                    }
                    if t.actor != Some(Actor::Framework) {
                        anyhow::bail!(
                            "field '{}': auto_increment_within target '{}' must have actor 'framework'",
                            field.name,
                            within
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL_FIXTURE: &str = r#"
name: issues
id_format: "I{:03d}"
default_actor: human
lifecycle:
  states: [triage, active, done]
  transitions:
    - from: triage
      to: active
      verb: start
      actor: human
    - from: active
      to: done
      verb: close
      actor: ai_autonomous
fields:
  - name: summary
    type: text
    required: true
    description: "One-line summary"
  - name: count
    type: integer
  - name: resolved
    type: bool
  - name: priority
    type: enum
    enum_values: [low, medium, high]
    actor: human
  - name: tags
    type:
      list: text
  - name: triage
    type: record
    fields:
      - name: verdict
        type: text
        required: true
      - name: notes
        type: text
  - name: contract
    type: record
    fields:
      - name: done_when
        type: text
        required_when: "triage.verdict == 'T3'"
      - name: scope_in
        type: text
      - name: scope_out
        type: text
  - name: ref_id
    type: display_id
  - name: created_at
    type: timestamp
"#;

    #[test]
    fn parse_full_fixture() {
        let schema = Schema::from_yaml(FULL_FIXTURE).expect("parse failed");
        assert_eq!(schema.name, "issues");
        assert_eq!(schema.id_format, "I{:03d}");
        assert_eq!(schema.fields.len(), 9);
    }

    #[test]
    fn field_types_roundtrip() {
        let schema = Schema::from_yaml(FULL_FIXTURE).unwrap();
        let by_name = |n: &str| schema.fields.iter().find(|f| f.name == n).unwrap();

        assert_eq!(by_name("summary").ty, FieldType::Text);
        assert_eq!(by_name("count").ty, FieldType::Integer);
        assert_eq!(by_name("resolved").ty, FieldType::Bool);
        assert_eq!(
            by_name("priority").ty,
            FieldType::Enum(vec!["low".into(), "medium".into(), "high".into()])
        );
        assert_eq!(
            by_name("tags").ty,
            FieldType::List(Box::new(FieldType::Text))
        );
        assert_eq!(by_name("ref_id").ty, FieldType::DisplayId);
        assert_eq!(by_name("created_at").ty, FieldType::Timestamp);
    }

    #[test]
    fn record_subfield_required_when_on_subfield_not_parent() {
        let schema = Schema::from_yaml(FULL_FIXTURE).unwrap();
        let contract = schema.fields.iter().find(|f| f.name == "contract").unwrap();
        // Parent Record itself must NOT carry required_when
        assert!(
            contract.required_when.is_none(),
            "parent Record must not carry required_when"
        );

        // Sub-field done_when must carry required_when
        let sub_fields = match &contract.ty {
            FieldType::Record(subs) => subs,
            _ => panic!("contract must be Record"),
        };
        let done_when = sub_fields.iter().find(|f| f.name == "done_when").unwrap();
        let rw = done_when
            .required_when
            .as_ref()
            .expect("done_when must have required_when");
        assert_eq!(rw.lhs_path, vec!["triage", "verdict"]);
        assert_eq!(rw.rhs_literal, "T3");
    }

    #[test]
    fn actor_tag_on_field() {
        let schema = Schema::from_yaml(FULL_FIXTURE).unwrap();
        let priority = schema.fields.iter().find(|f| f.name == "priority").unwrap();
        assert_eq!(priority.actor, Some(Actor::Human));
    }

    #[test]
    fn transitions_parsed() {
        let schema = Schema::from_yaml(FULL_FIXTURE).unwrap();
        let t = &schema.lifecycle.transitions;
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].verb, "start");
        assert_eq!(t[1].actor, Some(Actor::AiAutonomous));
    }

    #[test]
    fn unknown_field_type_errors() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: f
    type: magic_type
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("magic_type"),
            "error should mention the bad type: {err}"
        );
    }

    #[test]
    fn unknown_actor_errors() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: f
    type: text
    actor: robot
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("robot"),
            "error should mention the bad actor: {err}"
        );
    }

    // ---- Task 1.2: auto_increment validation ----

    fn make_ai_schema(extra_fields: &str) -> String {
        format!(
            r#"
name: x
id_format: "X{{:01d}}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: current_phase
    type: integer
    actor: framework
  - name: current_cycle
    type: integer
    actor: framework
{extra_fields}
"#
        )
    }

    #[test]
    fn auto_increment_valid() {
        let yaml = make_ai_schema(
            "  - name: n\n    type: integer\n    actor: framework\n    auto_increment: true\n    auto_increment_within: current_phase",
        );
        Schema::from_yaml(&yaml).expect("valid auto_increment schema should parse");
    }

    #[test]
    fn auto_increment_non_integer_fails() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: n
    type: text
    actor: framework
    auto_increment: true
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string()
                .contains("auto_increment requires type 'integer'"),
            "err: {err}"
        );
    }

    #[test]
    fn auto_increment_without_framework_actor_fails() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: n
    type: integer
    actor: human
    auto_increment: true
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string()
                .contains("auto_increment requires actor 'framework'"),
            "err: {err}"
        );
    }

    #[test]
    fn auto_increment_within_missing_target_fails() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: current_cycle
    type: integer
    actor: framework
    auto_increment: true
    auto_increment_within: nonexistent_field
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string()
                .contains("auto_increment_within target 'nonexistent_field' not found"),
            "err: {err}"
        );
    }

    #[test]
    fn auto_increment_within_self_fails() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: n
    type: integer
    actor: framework
    auto_increment: true
    auto_increment_within: n
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string()
                .contains("auto_increment_within cannot reference itself"),
            "err: {err}"
        );
    }

    // ---- Task 1.5: scope parsing ----

    #[test]
    fn scope_defaults_to_worktree() {
        let schema = Schema::from_yaml(FULL_FIXTURE).unwrap();
        assert_eq!(schema.scope, StoreScope::Worktree);
    }

    #[test]
    fn scope_repo_parses() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
scope: repo
lifecycle:
  states: [open]
  transitions: []
fields: []
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        assert_eq!(schema.scope, StoreScope::Repo);
    }

    #[test]
    fn scope_user_parses() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
scope: user
lifecycle:
  states: [open]
  transitions: []
fields: []
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        assert_eq!(schema.scope, StoreScope::User);
    }

    #[test]
    fn scope_unknown_errors() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
scope: global
lifecycle:
  states: [open]
  transitions: []
fields: []
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("unknown scope 'global'"),
            "err: {err}"
        );
    }

    // ---- Task 1.7: ListRecord parsing ----

    #[test]
    fn list_record_parses() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: cycles
    type: list_record
    fields:
      - name: phase
        type: integer
      - name: executor
        type: record
        fields:
          - name: summary
            type: text
          - name: commit
            type: text
      - name: tags
        type:
          list: text
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let cycles = schema.fields.iter().find(|f| f.name == "cycles").unwrap();
        match &cycles.ty {
            FieldType::ListRecord(sub_fields) => {
                assert_eq!(sub_fields.len(), 3);
                // executor is a nested Record
                let executor = sub_fields.iter().find(|f| f.name == "executor").unwrap();
                assert!(matches!(executor.ty, FieldType::Record(_)));
            }
            _ => panic!("expected ListRecord, got {:?}", cycles.ty),
        }
    }

    // ---- Task 1.8: ListFk parsing ----

    #[test]
    fn list_fk_parses() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: depends_on
    type: list_fk
    ref: tasks
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let dep = schema
            .fields
            .iter()
            .find(|f| f.name == "depends_on")
            .unwrap();
        assert_eq!(
            dep.ty,
            FieldType::ListFk {
                ref_store: "tasks".to_string()
            }
        );
    }

    #[test]
    fn list_fk_missing_ref_errors() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: depends_on
    type: list_fk
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("requires a 'ref' store name"),
            "err: {err}"
        );
    }

    // ---- Task 2.2 AC tests: workflow: opt-in ----

    /// AC2.1: A schema without `workflow:` parses identically to v0.1;
    /// `schema.workflow.is_none()` and no regression on existing schemas.
    #[test]
    fn schema_without_workflow_is_none() {
        let schema = Schema::from_yaml(FULL_FIXTURE).unwrap();
        assert!(
            schema.workflow.is_none(),
            "v0.1 schema must have workflow == None"
        );
    }

    /// AC2.2: A schema with a complete `workflow:` block parses and all key
    /// fields round-trip.
    #[test]
    fn schema_with_workflow_parses() {
        let yaml = r#"
name: wf_test
id_format: "WF{:03d}"
lifecycle:
  states: [planning, executing, done]
  transitions: []
fields:
  - name: title
    type: text
    required: true
  - name: plan
    type: record
    fields:
      - name: summary
        type: text
  - name: cycles
    type: list_record
    fields:
      - name: phase
        type: integer
workflow:
  agent_roles:
    planner:
      description: "Creates the plan"
    executor:
      description: "Implements the plan"
  briefing_templates:
    planner: templates/planner.md.tpl
    executor: templates/executor.md.tpl
  render_template: templates/main.md.tpl
  render_target_path: "tasks/{{status_dir}}/{{display_id}}/main.md"
  on_state:
    planning:
      - dispatch_agent: planner
    executing:
      - dispatch_agent: executor
  submit_targets:
    submit-plan: plan
    submit-execute: cycles
  max_revise_cycles: 3
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let wf = schema.workflow.as_ref().expect("workflow must be Some");
        assert_eq!(wf.agent_roles.len(), 2);
        assert!(wf.agent_roles.contains_key("planner"));
        assert_eq!(
            wf.briefing_templates["planner"],
            std::path::PathBuf::from("templates/planner.md.tpl")
        );
        assert_eq!(
            wf.render_target_path.as_deref(),
            Some("tasks/{{status_dir}}/{{display_id}}/main.md")
        );
        assert_eq!(wf.submit_targets["submit-plan"], "plan");
        assert_eq!(wf.submit_targets["submit-execute"], "cycles");
        assert_eq!(wf.max_revise_cycles, Some(3));
    }

    /// AC2.3: A schema referencing an unknown lifecycle state in `on_state` errors
    /// with that state name in the message.
    #[test]
    fn schema_workflow_unknown_on_state_errors() {
        let yaml = r#"
name: wf_test
id_format: "WF{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields: []
workflow:
  agent_roles:
    planner: {}
  briefing_templates:
    planner: templates/p.md.tpl
  on_state:
    nonexistent_state:
      - dispatch_agent: planner
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("nonexistent_state"),
            "err must name the unknown state: {err}"
        );
    }

    /// AC2.4: A schema referencing an unknown agent role in DispatchAgent errors clearly.
    #[test]
    fn schema_workflow_unknown_dispatch_agent_errors() {
        let yaml = r#"
name: wf_test
id_format: "WF{:03d}"
lifecycle:
  states: [planning]
  transitions: []
fields: []
workflow:
  agent_roles:
    planner: {}
  briefing_templates:
    planner: templates/p.md.tpl
  on_state:
    planning:
      - dispatch_agent: ghost_role
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("ghost_role"),
            "err must name the unknown role: {err}"
        );
    }

    /// AC2.6: submit_targets pointing at non-existent field errors with field name.
    #[test]
    fn schema_workflow_submit_target_unknown_field_errors() {
        let yaml = r#"
name: wf_test
id_format: "WF{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields: []
workflow:
  agent_roles: {}
  briefing_templates: {}
  on_state: {}
  submit_targets:
    submit-plan: nonexistent_field
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("nonexistent_field"),
            "err must name the missing field: {err}"
        );
    }

    /// AC2.6: submit_targets["submit-plan"] pointing at a list_record field errors with type info.
    #[test]
    fn schema_workflow_submit_plan_wrong_type_errors() {
        let yaml = r#"
name: wf_test
id_format: "WF{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: cycles
    type: list_record
    fields:
      - name: phase
        type: integer
workflow:
  agent_roles: {}
  briefing_templates: {}
  on_state: {}
  submit_targets:
    submit-plan: cycles
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("submit-plan") && err.to_string().contains("record"),
            "err must mention submit-plan and record: {err}"
        );
    }

    // ---------------------------------------------------------------------------
    // T008 Phase 1: FieldType::Json parser tests
    // ---------------------------------------------------------------------------

    const JSON_FIELD_SCHEMA: &str = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: notes
    type: json
    required: false
"#;

    #[test]
    fn field_type_json_parses() {
        let schema =
            Schema::from_yaml(JSON_FIELD_SCHEMA).expect("schema with type: json should parse");
        let notes = schema.fields.iter().find(|f| f.name == "notes").unwrap();
        assert_eq!(
            notes.ty,
            FieldType::Json,
            "type: json must resolve to FieldType::Json"
        );
    }

    #[test]
    fn field_type_json_unknown_type_error_lists_json() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: f
    type: magic_type
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("json"),
            "unknown-type error must mention 'json' as a valid type: {err}"
        );
    }

    #[test]
    fn field_type_json_in_record_rejected() {
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: outer
    type: record
    fields:
      - name: inner_json
        type: json
"#;
        let err = Schema::from_yaml(yaml).unwrap_err();
        assert!(
            err.to_string().contains("may only appear at the top level"),
            "json inside record must error with 'may only appear at the top level': {err}"
        );
        assert!(
            err.to_string().contains("outer.inner_json"),
            "error must name the nested field path 'outer.inner_json': {err}"
        );
    }

    #[test]
    fn field_type_json_in_list_record_accepted() {
        // Decision 1 updated: `json` is allowed as a list_record sub-field so
        // snapshot fields like plan_review_log[].reviewed_plan can hold any JSON
        // value (object, null, etc.) rather than being constrained to object-only.
        let yaml = r#"
name: x
id_format: "X{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: items
    type: list_record
    fields:
      - name: payload
        type: json
        required: false
"#;
        Schema::from_yaml(yaml).expect("json inside list_record must now be accepted");
    }

    // ---------------------------------------------------------------------------
    // AC7.3: bundled tasks schema parses without errors
    // ---------------------------------------------------------------------------

    /// AC7.3: The bundled tasks schema YAML must parse cleanly — no type errors,
    /// no missing-state errors, no missing-agent-role errors.
    #[test]
    fn ac7_3_bundled_tasks_schema_parses() {
        let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "tasks")
            .map(|(_, content)| *content)
            .expect("tasks schema must be in BUNDLED_STORE_SCHEMAS");
        let schema = Schema::from_yaml(yaml).expect("tasks schema must parse without errors");
        assert_eq!(schema.name, "tasks");
        assert_eq!(schema.scope, StoreScope::Repo);
        assert!(
            schema.workflow.is_some(),
            "tasks schema must have a workflow block"
        );

        let wf = schema.workflow.as_ref().unwrap();
        // All four agent roles must be present
        assert!(
            wf.agent_roles.contains_key("planner"),
            "planner role missing"
        );
        assert!(
            wf.agent_roles.contains_key("plan_reviewer"),
            "plan_reviewer role missing"
        );
        assert!(
            wf.agent_roles.contains_key("executor"),
            "executor role missing"
        );
        assert!(
            wf.agent_roles.contains_key("code_reviewer"),
            "code_reviewer role missing"
        );

        // All four briefing templates must be declared
        assert!(
            wf.briefing_templates.contains_key("planner"),
            "planner template missing"
        );
        assert!(
            wf.briefing_templates.contains_key("plan_reviewer"),
            "plan_reviewer template missing"
        );
        assert!(
            wf.briefing_templates.contains_key("executor"),
            "executor template missing"
        );
        assert!(
            wf.briefing_templates.contains_key("code_reviewer"),
            "code_reviewer template missing"
        );

        // render_template must be present
        assert!(wf.render_template.is_some(), "render_template missing");

        // submit_targets must map all four submit verbs
        assert_eq!(
            wf.submit_targets.get("submit-plan").map(|s| s.as_str()),
            Some("plan")
        );
        assert_eq!(
            wf.submit_targets
                .get("submit-plan-review")
                .map(|s| s.as_str()),
            Some("plan_review_log")
        );
        assert_eq!(
            wf.submit_targets.get("submit-execute").map(|s| s.as_str()),
            Some("cycles")
        );
        assert_eq!(
            wf.submit_targets.get("submit-review").map(|s| s.as_str()),
            Some("cycles")
        );

        // lifecycle must have all expected states
        let states = &schema.lifecycle.states;
        for expected in &[
            "planning",
            "plan_review",
            "ready",
            "executing",
            "code_review",
            "blocked",
            "complete",
        ] {
            assert!(
                states.iter().any(|s| s == expected),
                "missing state: {}",
                expected
            );
        }
    }

    /// AC7.3b: All bundled task templates are present in BUNDLED_STORE_TEMPLATES.
    #[test]
    fn ac7_3b_bundled_tasks_templates_present() {
        let templates = crate::cli::dynamic::BUNDLED_STORE_TEMPLATES
            .iter()
            .find(|(name, _)| *name == "tasks")
            .map(|(_, t)| *t)
            .expect("tasks must be in BUNDLED_STORE_TEMPLATES");

        let template_names: Vec<&str> = templates.iter().map(|(name, _)| *name).collect();
        assert!(
            template_names.contains(&"templates/planner-brief.md.tpl"),
            "planner template missing"
        );
        assert!(
            template_names.contains(&"templates/plan-reviewer-brief.md.tpl"),
            "plan-reviewer template missing"
        );
        assert!(
            template_names.contains(&"templates/executor-brief.md.tpl"),
            "executor template missing"
        );
        assert!(
            template_names.contains(&"templates/code-reviewer-brief.md.tpl"),
            "code-reviewer template missing"
        );
        assert!(
            template_names.contains(&"templates/main.md.tpl"),
            "main.md template missing"
        );

        // Each template must have non-empty content
        for (name, content) in templates {
            assert!(!content.is_empty(), "template '{}' is empty", name);
        }
    }

    /// AC7.5: current_phase and current_cycle have actor: framework and cannot be
    /// set by non-framework invokers.
    #[test]
    fn ac7_5_framework_fields_have_framework_actor() {
        let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "tasks")
            .map(|(_, content)| *content)
            .expect("tasks schema must be in BUNDLED_STORE_SCHEMAS");
        let schema = Schema::from_yaml(yaml).unwrap();

        let current_phase = schema
            .fields
            .iter()
            .find(|f| f.name == "current_phase")
            .unwrap();
        assert_eq!(
            current_phase.actor,
            Some(Actor::Framework),
            "current_phase must have actor: framework"
        );

        let current_cycle = schema
            .fields
            .iter()
            .find(|f| f.name == "current_cycle")
            .unwrap();
        assert_eq!(
            current_cycle.actor,
            Some(Actor::Framework),
            "current_cycle must have actor: framework"
        );
    }

    fn minimal_schema(transitions: &str) -> String {
        format!(
            r#"name: guard_order
id_format: "G{{:03d}}"
lifecycle:
  states: [open, review, blocked, done]
  transitions:
{}
fields:
  - name: title
    type: text
"#,
            transitions
        )
    }

    #[test]
    fn unguarded_before_guarded_same_selector_errors_with_shadow_line() {
        let yaml = minimal_schema(
            r#"    - from: open
      to: blocked
      verb: close
    - from: open
      to: done
      verb: close
      when: "title == 'ok'""#,
        );
        let err = Schema::from_yaml(&yaml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("transition guard order violation"),
            "msg: {msg}"
        );
        assert!(
            msg.contains("line 6"),
            "msg should point at fallback line: {msg}"
        );
        assert!(msg.contains("from='open'"), "msg: {msg}");
        assert!(msg.contains("verb='close'"), "msg: {msg}");
    }

    #[test]
    fn guarded_before_unguarded_fallback_is_accepted() {
        let yaml = minimal_schema(
            r#"    - {from: open, to: done, verb: close, guard: "title == 'ok'"}
    - {from: open, to: blocked, verb: close}"#,
        );
        Schema::from_yaml(&yaml).unwrap();
    }

    #[test]
    fn single_guarded_transition_is_accepted() {
        let yaml =
            minimal_schema(r#"    - {from: open, to: done, verb: close, guard: "title == 'ok'"}"#);
        Schema::from_yaml(&yaml).unwrap();
    }

    #[test]
    fn multiple_guards_no_fallback_is_accepted() {
        let yaml = minimal_schema(
            r#"    - {from: open, to: review, verb: close, guard: "title == 'maybe'"}
    - {from: open, to: done, verb: close, guard: "title == 'ok'"}"#,
        );
        Schema::from_yaml(&yaml).unwrap();
    }

    #[test]
    fn only_unguarded_transitions_with_distinct_selectors_are_accepted() {
        let yaml = minimal_schema(
            r#"    - {from: open, to: done, verb: close}
    - {from: review, to: done, verb: close}"#,
        );
        Schema::from_yaml(&yaml).unwrap();
    }

    /// T013 P1 AC1.4: tasks schema declares a top-level `tier_hint` field as
    /// enum [T1, T2, T3], required: false, no actor.
    #[test]
    fn t013_p1_tasks_tier_hint_field_present() {
        let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "tasks")
            .map(|(_, content)| *content)
            .expect("tasks schema must be in BUNDLED_STORE_SCHEMAS");
        let schema = Schema::from_yaml(yaml).expect("tasks schema must parse");

        let tier_hint = schema
            .fields
            .iter()
            .find(|f| f.name == "tier_hint")
            .expect("tasks schema must declare a top-level `tier_hint` field");

        match &tier_hint.ty {
            FieldType::Enum(vals) => {
                assert_eq!(
                    vals,
                    &vec!["T1".to_string(), "T2".to_string(), "T3".to_string()],
                    "tier_hint enum_values must be [T1, T2, T3]"
                );
            }
            other => panic!("tier_hint must be enum type, got {:?}", other),
        }
        assert_eq!(
            tier_hint.enum_values.as_deref(),
            Some(&["T1".to_string(), "T2".to_string(), "T3".to_string()][..]),
            "tier_hint enum_values metadata must be [T1, T2, T3]"
        );
        assert!(
            !tier_hint.required,
            "tier_hint must be required: false (CLI-layer enforces presence)"
        );
        assert!(
            tier_hint.actor.is_none(),
            "tier_hint must have no actor (writable by any invoker)"
        );
    }

    // ----- T138 P1: integration lane states / transitions / fields -------

    fn bundled_tasks_schema() -> Schema {
        let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "tasks")
            .map(|(_, content)| *content)
            .expect("tasks schema must be in BUNDLED_STORE_SCHEMAS");
        Schema::from_yaml(yaml).expect("tasks schema must parse")
    }

    /// AC1.2 (a): the four new integration-lane states are present in
    /// lifecycle.states.
    #[test]
    fn t138_p1_integration_lane_states_present() {
        let schema = bundled_tasks_schema();
        for expected in [
            "integration_queued",
            "integrating",
            "integration_blocked",
            "integrated",
        ] {
            assert!(
                schema.lifecycle.states.iter().any(|s| s == expected),
                "missing state: {expected}; states: {:?}",
                schema.lifecycle.states
            );
        }
    }

    /// AC1.2 (b): the seven integration-lane verbs are declared as transition
    /// verbs in tasks lifecycle.
    #[test]
    fn t138_p1_integration_lane_verbs_present() {
        let schema = bundled_tasks_schema();
        let verbs: std::collections::HashSet<&str> = schema
            .lifecycle
            .transitions
            .iter()
            .map(|t| t.verb.as_str())
            .collect();
        for v in [
            "enqueue-integration",
            "start-integration",
            "mark_integrated",
            "mark_integration_blocked",
            "retry-integration",
            "mark_cargo_installed",
            "supersede",
        ] {
            // 'supersede' lives on external_reviews, not tasks; check below
            // separately. Skip here.
            if v == "supersede" {
                continue;
            }
            assert!(
                verbs.contains(v),
                "missing verb: {v}; verbs: {:?}",
                verbs.iter().collect::<Vec<_>>()
            );
        }
    }

    /// AC1.3: NO transition exists with from='accepted' and to='cargo_installed'.
    /// AC1.3: NO transition exists with from='integration_blocked' and
    /// to='integrated'.
    #[test]
    fn t138_p1_no_legacy_or_shortcut_transitions() {
        let schema = bundled_tasks_schema();
        let has_accepted_to_cargo = schema
            .lifecycle
            .transitions
            .iter()
            .any(|t| t.from == "accepted" && t.to == "cargo_installed");
        assert!(
            !has_accepted_to_cargo,
            "transition (accepted, cargo_installed) must NOT exist post-T138"
        );
        let has_blocked_to_integrated = schema
            .lifecycle
            .transitions
            .iter()
            .any(|t| t.from == "integration_blocked" && t.to == "integrated");
        assert!(
            !has_blocked_to_integrated,
            "transition (integration_blocked, integrated) must NOT exist; retry must re-traverse integration_queued → integrating → integrated"
        );
    }

    /// AC1.4: tasks schema declares integration_blocked_reason (text scalar)
    /// and integration_attempts (list_record) with the documented sub-field
    /// shape including reviewed_base_sha and outcome enum values stale_base
    /// and push_failure.
    #[test]
    fn t138_p1_integration_provenance_fields_present() {
        let schema = bundled_tasks_schema();

        let blocked_reason = schema
            .fields
            .iter()
            .find(|f| f.name == "integration_blocked_reason")
            .expect("integration_blocked_reason field must be present");
        assert_eq!(
            blocked_reason.ty,
            FieldType::Text,
            "integration_blocked_reason must be text scalar; got {:?}",
            blocked_reason.ty
        );
        assert_eq!(
            blocked_reason.actor,
            Some(Actor::Framework),
            "integration_blocked_reason must have actor: framework"
        );

        let attempts = schema
            .fields
            .iter()
            .find(|f| f.name == "integration_attempts")
            .expect("integration_attempts field must be present");
        let subs = match &attempts.ty {
            FieldType::ListRecord(s) => s,
            other => panic!("integration_attempts must be list_record, got {:?}", other),
        };
        assert_eq!(
            attempts.actor,
            Some(Actor::Framework),
            "integration_attempts must have actor: framework"
        );

        let sub_names: std::collections::HashSet<&str> =
            subs.iter().map(|f| f.name.as_str()).collect();
        for name in [
            "attempt_no",
            "base_main_sha",
            "candidate_head_before",
            "candidate_head_after",
            "landed_main_sha",
            "refresh_strategy",
            "pre_land_check_summary",
            "reviewed_base_sha",
            "outcome",
            "started_at",
            "completed_at",
        ] {
            assert!(
                sub_names.contains(name),
                "integration_attempts missing sub-field: {name}; got: {:?}",
                sub_names
            );
        }

        let outcome = subs
            .iter()
            .find(|f| f.name == "outcome")
            .expect("outcome sub-field");
        let outcome_vals = match &outcome.ty {
            FieldType::Enum(v) => v,
            other => panic!("outcome must be enum, got {:?}", other),
        };
        for v in [
            "integrated",
            "rebase_conflict",
            "stale_base",
            "stale_external_review",
            "pre_land_check_failed",
            "merge_failure",
            "push_failure",
        ] {
            assert!(
                outcome_vals.iter().any(|s| s == v),
                "outcome enum missing variant '{v}'; values: {:?}",
                outcome_vals
            );
        }
    }

    /// AC1.6 (companion): the bundled external_reviews schema declares the
    /// (passed → superseded, supersede, framework) transition, used by the
    /// integrate builtin to invalidate stale PASSes (stale_base /
    /// stale_external_review paths).
    #[test]
    fn t138_p1_external_reviews_passed_to_superseded_transition_present() {
        let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
            .iter()
            .find(|(name, _)| *name == "external_reviews")
            .map(|(_, content)| *content)
            .expect("external_reviews schema must be in BUNDLED_STORE_SCHEMAS");
        let schema = Schema::from_yaml(yaml).expect("external_reviews schema must parse");
        let t = schema
            .lifecycle
            .transitions
            .iter()
            .find(|t| t.from == "passed" && t.to == "superseded" && t.verb == "supersede")
            .expect("external_reviews must declare (passed, superseded, supersede)");
        assert_eq!(
            t.actor,
            Some(Actor::Framework),
            "(passed, superseded, supersede) actor must be framework"
        );
    }
}
