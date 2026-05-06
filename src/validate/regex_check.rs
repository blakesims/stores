use crate::schema::Field;
use crate::validate::error::RuleKind;
use crate::validate::required::lookup;
use crate::validate::{EntryMap, ValidationError};

/// Check that a Text field's value, if present, matches the declared `pattern` regex.
pub fn check_pattern(
    field: &Field,
    field_path: &[String],
    entry: &EntryMap,
    errors: &mut Vec<ValidationError>,
) {
    let pattern = match &field.pattern {
        Some(p) => p,
        None => return,
    };

    let val = match lookup(entry, field_path) {
        Some(v) => v,
        None => return, // absent: let required checks handle that
    };

    let s = match val.as_str() {
        Some(s) => s,
        None => return,
    };

    let re = match regex::Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => {
            errors.push(ValidationError {
                field_path: field_path.to_vec(),
                rule: RuleKind::Pattern {
                    pattern: pattern.clone(),
                },
                message: format!("invalid pattern '{}': {}", pattern, e),
            });
            return;
        }
    };

    if !re.is_match(s) {
        errors.push(ValidationError {
            field_path: field_path.to_vec(),
            rule: RuleKind::Pattern {
                pattern: pattern.clone(),
            },
            message: format!("value '{}' does not match pattern '{}'", s, pattern),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Field, FieldType};
    use crate::validate::error::RuleKind;
    use std::collections::BTreeMap;

    fn text_field_with_pattern(name: &str, pattern: &str) -> Field {
        Field {
            name: name.to_string(),
            ty: FieldType::Text,
            required: false,
            required_when: None,
            pattern: Some(pattern.to_string()),
            actor: None,
            enum_values: None,
            description: None,
            auto_increment: false,
            auto_increment_within: None,
            default: None,
        }
    }

    #[test]
    fn matching_pattern_passes() {
        let field = text_field_with_pattern("slug", r"^[a-z0-9-]+$");
        let mut entry: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        entry.insert(
            "slug".into(),
            serde_json::Value::String("hello-world".into()),
        );
        let mut errors = vec![];
        check_pattern(&field, &["slug".to_string()], &entry, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn non_matching_pattern_fails() {
        let field = text_field_with_pattern("slug", r"^[a-z0-9-]+$");
        let mut entry: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        entry.insert(
            "slug".into(),
            serde_json::Value::String("Hello World!".into()),
        );
        let mut errors = vec![];
        check_pattern(&field, &["slug".to_string()], &entry, &mut errors);
        assert_eq!(errors.len(), 1);
        match &errors[0].rule {
            RuleKind::Pattern { pattern } => assert_eq!(pattern, r"^[a-z0-9-]+$"),
            other => panic!("expected Pattern, got {:?}", other),
        }
        assert!(errors[0].message.contains("Hello World!"));
        assert!(errors[0].message.contains(r"^[a-z0-9-]+$"));
    }

    #[test]
    fn absent_field_no_pattern_error() {
        let field = text_field_with_pattern("slug", r"^[a-z0-9-]+$");
        let entry: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let mut errors = vec![];
        check_pattern(&field, &["slug".to_string()], &entry, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn field_without_pattern_skipped() {
        let field = Field {
            name: "title".to_string(),
            ty: FieldType::Text,
            required: false,
            required_when: None,
            pattern: None,
            actor: None,
            enum_values: None,
            description: None,
            auto_increment: false,
            auto_increment_within: None,
            default: None,
        };
        let mut entry: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        entry.insert("title".into(), serde_json::Value::String("anything".into()));
        let mut errors = vec![];
        check_pattern(&field, &["title".to_string()], &entry, &mut errors);
        assert!(errors.is_empty());
    }
}
