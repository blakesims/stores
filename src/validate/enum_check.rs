use crate::schema::Field;
use crate::validate::{EntryMap, ValidationError};
use crate::validate::error::RuleKind;
use crate::validate::required::lookup;

/// Check that the field's value, if present, is one of the declared enum_values.
pub fn check_enum(
    field: &Field,
    field_path: &[String],
    entry: &EntryMap,
    errors: &mut Vec<ValidationError>,
) {
    let allowed = match &field.enum_values {
        Some(v) => v,
        None => return, // not an enum field
    };

    let val = match lookup(entry, field_path) {
        Some(v) => v,
        None => return, // absent: let required checks handle that
    };

    let s = match val.as_str() {
        Some(s) => s,
        None => return, // non-string value: type coercion handled elsewhere
    };

    if !allowed.contains(&s.to_string()) {
        errors.push(ValidationError {
            field_path: field_path.to_vec(),
            rule: RuleKind::Enum,
            message: format!(
                "value '{}' is not one of the allowed values: [{}]",
                s,
                allowed.join(", ")
            ),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Field, FieldType};
    use std::collections::BTreeMap;

    fn enum_field(name: &str, values: Vec<&str>) -> Field {
        Field {
            name: name.to_string(),
            ty: FieldType::Enum(values.iter().map(|s| s.to_string()).collect()),
            required: false,
            required_when: None,
            pattern: None,
            actor: None,
            enum_values: Some(values.iter().map(|s| s.to_string()).collect()),
            description: None,
        }
    }

    #[test]
    fn valid_enum_value_passes() {
        let field = enum_field("priority", vec!["low", "medium", "high"]);
        let mut entry: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        entry.insert("priority".into(), serde_json::Value::String("low".into()));
        let mut errors = vec![];
        check_enum(&field, &["priority".to_string()], &entry, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn invalid_enum_value_fails() {
        let field = enum_field("priority", vec!["low", "medium", "high"]);
        let mut entry: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        entry.insert("priority".into(), serde_json::Value::String("critical".into()));
        let mut errors = vec![];
        check_enum(&field, &["priority".to_string()], &entry, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].rule, RuleKind::Enum);
        assert!(errors[0].message.contains("critical"));
        assert!(errors[0].message.contains("low"));
    }

    #[test]
    fn absent_enum_field_no_error() {
        let field = enum_field("priority", vec!["low", "medium", "high"]);
        let entry: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let mut errors = vec![];
        check_enum(&field, &["priority".to_string()], &entry, &mut errors);
        assert!(errors.is_empty());
    }
}
