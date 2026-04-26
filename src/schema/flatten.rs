use crate::schema::{Field, FieldType, Schema};
use anyhow::{anyhow, Result};

/// A single leaf argument produced by flattening Record sub-fields.
#[derive(Debug, Clone)]
pub struct LeafArg<'a> {
    /// CLI flag name in kebab-case (leaf field's own name only; no parent prefix).
    pub cli_name: String,
    /// Full dot-path from schema root, e.g. `["contract", "done_when"]`.
    pub path: Vec<String>,
    /// Reference to the leaf Field struct.
    pub field: &'a Field,
}

/// Convert a field name (snake_case) to a CLI flag name (kebab-case).
fn to_kebab(name: &str) -> String {
    name.replace('_', "-")
}

/// Walk a field, appending leaf args to `out`.
///
/// - Non-Record fields are leaves themselves.
/// - Record fields recurse one level into their sub-fields.
/// - `List(_)` is treated as a single leaf (not recursed), regardless of inner type.
fn walk_field<'a>(
    field: &'a Field,
    parent_path: &[String],
    out: &mut Vec<LeafArg<'a>>,
) {
    match &field.ty {
        FieldType::Record(sub_fields) => {
            let mut path = parent_path.to_vec();
            path.push(field.name.clone());
            for sub in sub_fields {
                walk_field(sub, &path, out);
            }
        }
        _ => {
            // Leaf
            let mut path = parent_path.to_vec();
            path.push(field.name.clone());
            out.push(LeafArg {
                cli_name: to_kebab(&field.name),
                path,
                field,
            });
        }
    }
}

/// Flatten a schema into leaf CLI args.
///
/// Returns an error if two leaves have the same `cli_name`, naming both
/// parent paths.
pub fn leaf_args(schema: &Schema) -> Result<Vec<LeafArg<'_>>> {
    let mut leaves: Vec<LeafArg<'_>> = Vec::new();

    for field in &schema.fields {
        walk_field(field, &[], &mut leaves);
    }

    // Uniqueness check
    use std::collections::HashMap;
    let mut seen: HashMap<String, Vec<String>> = HashMap::new();
    for leaf in &leaves {
        seen.entry(leaf.cli_name.clone())
            .or_default()
            .push(leaf.path.join("."));
    }
    let collisions: Vec<_> = seen
        .iter()
        .filter(|(_, paths)| paths.len() > 1)
        .collect();
    if !collisions.is_empty() {
        let msgs: Vec<String> = collisions
            .iter()
            .map(|(name, paths)| {
                format!("cli_name '{}' collides at paths: {}", name, paths.join(", "))
            })
            .collect();
        return Err(anyhow!(
            "leaf_args uniqueness violation:\n{}",
            msgs.join("\n")
        ));
    }

    Ok(leaves)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Schema;

    const FIXTURE: &str = r#"
name: tasks
id_format: "T{:03d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: title
    type: text
  - name: triage
    type: record
    fields:
      - name: verdict
        type: text
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
"#;

    #[test]
    fn five_leaf_args_correct_names() {
        let schema = Schema::from_yaml(FIXTURE).unwrap();
        let args = leaf_args(&schema).unwrap();
        let names: Vec<&str> = args.iter().map(|a| a.cli_name.as_str()).collect();
        assert_eq!(names, vec!["title", "verdict", "notes", "done-when", "scope-in", "scope-out"],
            "got: {:?}", names);
        // AC6: exactly these 5 (plus title = 6 total, but AC says 5 for the two Records fixture;
        // our fixture also has top-level 'title' so 6 total — that's fine, AC6 fixture
        // has no top-level scalars, so let's also test the exact AC6 fixture)
    }

    #[test]
    fn ac6_exact_fixture() {
        // AC6 fixture: only Records triage{verdict,notes} + contract{done_when,scope_in,scope_out}
        let yaml = r#"
name: ac6
id_format: "A{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: triage
    type: record
    fields:
      - name: verdict
        type: text
      - name: notes
        type: text
  - name: contract
    type: record
    fields:
      - name: done_when
        type: text
      - name: scope_in
        type: text
      - name: scope_out
        type: text
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let args = leaf_args(&schema).unwrap();
        assert_eq!(args.len(), 5);
        let names: Vec<&str> = args.iter().map(|a| a.cli_name.as_str()).collect();
        assert_eq!(names, vec!["verdict", "notes", "done-when", "scope-in", "scope-out"]);
    }

    #[test]
    fn paths_are_correct() {
        let yaml = r#"
name: ac6
id_format: "A{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: contract
    type: record
    fields:
      - name: done_when
        type: text
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let args = leaf_args(&schema).unwrap();
        assert_eq!(args[0].path, vec!["contract", "done_when"]);
    }

    #[test]
    fn collision_returns_error_naming_both_paths() {
        let yaml = r#"
name: col
id_format: "C{:01d}"
lifecycle:
  states: [open]
  transitions: []
fields:
  - name: triage
    type: record
    fields:
      - name: notes
        type: text
  - name: review
    type: record
    fields:
      - name: notes
        type: text
"#;
        let schema = Schema::from_yaml(yaml).unwrap();
        let err = leaf_args(&schema).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("notes"), "should mention 'notes': {msg}");
        assert!(msg.contains("triage.notes"), "should mention triage.notes: {msg}");
        assert!(msg.contains("review.notes"), "should mention review.notes: {msg}");
    }
}
