pub mod actor;
pub mod flatten;
pub mod lifecycle;
pub mod parse;
pub mod required_when;
pub mod types;

pub use actor::Actor;
pub use lifecycle::{Lifecycle, Transition};
pub use required_when::Expr as RequiredWhenExpr;

use serde::{Deserialize, Deserializer};

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
}

// ---------------------------------------------------------------------------
// Field
// ---------------------------------------------------------------------------

/// A single field in a store schema, including optional sub-fields when
/// `ty == Record(...)`.
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
    /// Sub-fields for Record type (alternative representation)
    #[serde(default)]
    fields: Option<Vec<RawField>>,
}

/// Raw field type before full resolution.
#[derive(Debug)]
enum RawFieldType {
    Scalar(String),
    ListOf(String),
    Record,
}

impl<'de> Deserialize<'de> for RawFieldType {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{self, Visitor, MapAccess};
        use std::fmt;

        struct RFTVisitor;

        impl<'de> Visitor<'de> for RFTVisitor {
            type Value = RawFieldType;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a field type string or mapping")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<RawFieldType, E> {
                Ok(RawFieldType::Scalar(v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<RawFieldType, E> {
                Ok(RawFieldType::Scalar(v))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<RawFieldType, A::Error> {
                // Expect exactly one key: "list" or "record"
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
                    other => Err(de::Error::custom(format!(
                        "unknown type map key '{other}'"
                    ))),
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
            other => anyhow::bail!(
                "unknown field type '{other}'; expected one of: text, integer, bool, enum, list, record, display_id, timestamp"
            ),
        },
        RawFieldType::ListOf(inner) => {
            // inner must be a scalar type name
            let inner_ty = resolve_field_type(
                &RawFieldType::Scalar(inner.clone()),
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
    }
}

fn raw_to_field(r: &RawField) -> anyhow::Result<Field> {
    let ty = resolve_field_type(&r.ty, &r.enum_values, &r.fields)?;
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
        actor: r.actor.clone(),
        enum_values: r.enum_values.clone(),
        description: r.description.clone(),
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
}

impl Schema {
    pub fn from_yaml(yaml: &str) -> anyhow::Result<Schema> {
        let raw: RawSchema = serde_yaml::from_str(yaml)
            .map_err(|e| anyhow::anyhow!("YAML parse error: {}", e))?;

        // Validate id_format
        crate::id_format::validate(&raw.id_format)?;

        let fields: anyhow::Result<Vec<Field>> = raw.fields.iter().map(raw_to_field).collect();

        Ok(Schema {
            name: raw.name,
            id_format: raw.id_format,
            fields: fields?,
            lifecycle: raw.lifecycle,
            default_actor: raw.default_actor,
        })
    }
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
        assert!(contract.required_when.is_none(), "parent Record must not carry required_when");

        // Sub-field done_when must carry required_when
        let sub_fields = match &contract.ty {
            FieldType::Record(subs) => subs,
            _ => panic!("contract must be Record"),
        };
        let done_when = sub_fields.iter().find(|f| f.name == "done_when").unwrap();
        let rw = done_when.required_when.as_ref().expect("done_when must have required_when");
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
}
