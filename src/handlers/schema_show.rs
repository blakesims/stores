use anyhow::Result;
use clap::ArgMatches;
use serde_json::{json, Value};

use crate::schema::{Field, FieldType, Schema};

pub fn run(schema: &Schema, matches: &ArgMatches) -> Result<()> {
    let json_flag = matches.get_flag("json");

    if json_flag {
        let obj = schema_to_json(schema);
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        print_text(schema);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Text output
// ---------------------------------------------------------------------------

fn print_text(schema: &Schema) {
    println!("Store: {}", schema.name);
    println!("  id_format: {}", schema.id_format);

    if let Some(ref da) = schema.default_actor {
        println!("  default_actor: {da}");
    }

    println!();
    println!("Fields:");
    for field in &schema.fields {
        print_field_text(field, "  ");
    }

    println!();
    println!("Lifecycle:");
    let lc = &schema.lifecycle;
    let initial = lc.resolved_initial_state().unwrap_or("(none)");
    println!("  states: {}", lc.states.join(", "));
    println!("  initial_state: {initial}");

    if !lc.transitions.is_empty() {
        println!("  transitions:");
        for t in &lc.transitions {
            let actor_str = t
                .actor
                .as_ref()
                .map(|a| format!("  [actor: {a}]"))
                .unwrap_or_default();
            println!("    {} {} -> {}{}", t.verb, t.from, t.to, actor_str);
        }
    }
}

fn print_field_text(field: &Field, indent: &str) {
    let ty_str = field_type_str(&field.ty);
    let req_str = if field.required { " [required]" } else { "" };
    let actor_str = field
        .actor
        .as_ref()
        .map(|a| format!(" [actor: {a}]"))
        .unwrap_or_default();

    println!("{indent}{}: {}{}{}", field.name, ty_str, req_str, actor_str);

    if let Some(ref desc) = field.description {
        println!("{indent}  description: {desc}");
    }

    if let Some(ref rw) = field.required_when {
        println!("{indent}  required_when: {}", rw.condition_string());
    }

    if let Some(ref ev) = field.enum_values {
        println!("{indent}  enum_values: {}", ev.join(", "));
    }

    // Print sub-fields for Record
    if let FieldType::Record(ref sub_fields) = field.ty {
        let sub_indent = format!("{indent}  ");
        for sf in sub_fields {
            print_field_text(sf, &sub_indent);
        }
    }
}

fn field_type_str(ty: &FieldType) -> &'static str {
    match ty {
        FieldType::Text => "text",
        FieldType::Integer => "integer",
        FieldType::Bool => "bool",
        FieldType::Enum(_) => "enum",
        FieldType::List(_) => "list",
        FieldType::Record(_) => "record",
        FieldType::DisplayId => "display_id",
        FieldType::Timestamp => "timestamp",
        FieldType::ListRecord(_) => "list_record",
        FieldType::ListFk { .. } => "list_fk",
        FieldType::Json => "json",
    }
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

fn schema_to_json(schema: &Schema) -> Value {
    let fields: Vec<Value> = schema.fields.iter().map(field_to_json).collect();

    let transitions: Vec<Value> = schema
        .lifecycle
        .transitions
        .iter()
        .map(|t| {
            let mut obj = json!({
                "from": t.from,
                "to": t.to,
                "verb": t.verb,
            });
            if let Some(ref a) = t.actor {
                obj["actor"] = json!(a.to_string());
            }
            obj
        })
        .collect();

    let initial = schema
        .lifecycle
        .resolved_initial_state()
        .unwrap_or("(none)")
        .to_string();

    let mut obj = json!({
        "name": schema.name,
        "id_format": schema.id_format,
        "fields": fields,
        "lifecycle": {
            "states": schema.lifecycle.states,
            "initial_state": initial,
            "transitions": transitions,
        },
    });

    if let Some(ref da) = schema.default_actor {
        obj["default_actor"] = json!(da.to_string());
    }

    obj
}

fn field_to_json(field: &Field) -> Value {
    let ty_str = field_type_str(&field.ty);

    let mut obj = json!({
        "name": field.name,
        "type": ty_str,
        "required": field.required,
    });

    if let Some(ref desc) = field.description {
        obj["description"] = json!(desc);
    }

    if let Some(ref rw) = field.required_when {
        obj["required_when"] = json!(rw.condition_string());
    }

    if let Some(ref a) = field.actor {
        obj["actor"] = json!(a.to_string());
    }

    if let Some(ref ev) = field.enum_values {
        obj["enum_values"] = json!(ev);
    }

    if let FieldType::Record(ref sub_fields) = field.ty {
        obj["fields"] = json!(sub_fields.iter().map(field_to_json).collect::<Vec<_>>());
    }

    if let FieldType::List(ref inner) = field.ty {
        obj["inner_type"] = json!(field_type_str(inner));
    }

    obj
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Schema;

    const OBS_SCHEMA: &str = r#"
name: observations
id_format: "L{:03d}"
lifecycle:
  states: [open, triaged, resolved]
  transitions:
    - from: open
      to: triaged
      verb: triage
      actor: ai_with_human
fields:
  - name: summary
    type: text
    required: true
    description: "One-line summary"
  - name: triage
    type: record
    fields:
      - name: verdict
        type: enum
        enum_values: [T1, T2, T3]
  - name: contract
    type: record
    fields:
      - name: done_when
        type: text
        required_when: "triage.verdict == 'T3'"
        description: "Done criteria"
"#;

    #[test]
    fn json_output_has_required_fields() {
        let schema = Schema::from_yaml(OBS_SCHEMA).unwrap();
        let obj = schema_to_json(&schema);

        assert_eq!(obj["name"], "observations");
        assert_eq!(obj["id_format"], "L{:03d}");
        assert!(obj["fields"].is_array());
        assert!(obj["lifecycle"].is_object());
        assert_eq!(obj["lifecycle"]["initial_state"], "open");
    }

    #[test]
    fn json_fields_include_names() {
        let schema = Schema::from_yaml(OBS_SCHEMA).unwrap();
        let obj = schema_to_json(&schema);
        let field_names: Vec<&str> = obj["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["name"].as_str().unwrap())
            .collect();
        assert!(field_names.contains(&"summary"));
        assert!(field_names.contains(&"triage"));
        assert!(field_names.contains(&"contract"));
    }

    #[test]
    fn json_required_when_present_on_subfield() {
        let schema = Schema::from_yaml(OBS_SCHEMA).unwrap();
        let obj = schema_to_json(&schema);
        let contract = obj["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "contract")
            .unwrap();
        let sub_fields = contract["fields"].as_array().unwrap();
        let done_when = sub_fields
            .iter()
            .find(|f| f["name"] == "done_when")
            .unwrap();
        let rw = done_when["required_when"].as_str().unwrap();
        assert!(
            rw.contains("triage.verdict"),
            "required_when should mention triage.verdict: {rw}"
        );
        assert!(rw.contains("T3"), "required_when should mention T3: {rw}");
    }

    #[test]
    fn json_transitions_have_actor() {
        let schema = Schema::from_yaml(OBS_SCHEMA).unwrap();
        let obj = schema_to_json(&schema);
        let t = &obj["lifecycle"]["transitions"].as_array().unwrap()[0];
        assert_eq!(t["verb"], "triage");
        assert_eq!(t["actor"], "ai_with_human");
    }
}
