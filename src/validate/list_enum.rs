use crate::schema::Field;
use crate::validate::error::RuleKind;
use crate::validate::required::lookup;
use crate::validate::{EntryMap, ValidationError};

/// Check that every element of a List(Text) field value, if present, is one
/// of the values declared in `field.list_enum`.  Emits one `ValidationError`
/// per offending element naming the bad value and the allowed set.
pub fn check_list_enum(
    field: &Field,
    field_path: &[String],
    entry: &EntryMap,
    errors: &mut Vec<ValidationError>,
) {
    let allowed = match &field.list_enum {
        Some(v) => v,
        None => return, // no list_enum constraint on this field
    };

    let val = match lookup(entry, field_path) {
        Some(v) => v,
        None => return, // absent: let required checks handle that
    };

    let arr = match val.as_array() {
        Some(a) => a,
        None => return, // non-array: type-shape check handles this
    };

    for element in arr {
        let s = match element.as_str() {
            Some(s) => s,
            None => continue, // non-string element: skip (type coercion not our job)
        };
        if !allowed.contains(&s.to_string()) {
            errors.push(ValidationError {
                field_path: field_path.to_vec(),
                rule: RuleKind::ListEnum,
                message: format!(
                    "value '{}' is not one of the allowed flags: [{}]",
                    s,
                    allowed.join(", ")
                ),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Field, FieldType};
    use std::collections::BTreeMap;

    fn list_enum_field(name: &str, allowed: Vec<&str>) -> Field {
        Field {
            name: name.to_string(),
            ty: FieldType::List(Box::new(FieldType::Text)),
            required: false,
            required_when: None,
            pattern: None,
            actor: None,
            enum_values: None,
            description: None,
            auto_increment: false,
            auto_increment_within: None,
            default: None,
            list_enum: Some(allowed.iter().map(|s| s.to_string()).collect()),
        }
    }

    fn entry_with_array(key: &str, vals: &[&str]) -> BTreeMap<String, serde_json::Value> {
        let mut m = BTreeMap::new();
        m.insert(
            key.to_string(),
            serde_json::Value::Array(
                vals.iter()
                    .map(|s| serde_json::Value::String(s.to_string()))
                    .collect(),
            ),
        );
        m
    }

    #[test]
    fn all_canonical_values_pass() {
        let field = list_enum_field("flags", vec!["docs_only", "small_local_fix"]);
        let entry = entry_with_array("flags", &["docs_only", "small_local_fix"]);
        let mut errors = vec![];
        check_list_enum(&field, &["flags".to_string()], &entry, &mut errors);
        assert!(errors.is_empty(), "canonical values should pass: {:?}", errors);
    }

    #[test]
    fn non_canonical_value_fails_with_message() {
        let field = list_enum_field("flags", vec!["docs_only", "small_local_fix"]);
        let entry = entry_with_array("flags", &["docs_only", "bogus_flag"]);
        let mut errors = vec![];
        check_list_enum(&field, &["flags".to_string()], &entry, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].rule, RuleKind::ListEnum);
        assert!(
            errors[0].message.contains("bogus_flag"),
            "message must name the bad value: {}",
            errors[0].message
        );
        assert!(
            errors[0].message.contains("docs_only"),
            "message must list the allowed values: {}",
            errors[0].message
        );
    }

    #[test]
    fn empty_array_passes() {
        let field = list_enum_field("flags", vec!["docs_only"]);
        let entry = entry_with_array("flags", &[]);
        let mut errors = vec![];
        check_list_enum(&field, &["flags".to_string()], &entry, &mut errors);
        assert!(errors.is_empty(), "empty array must pass: {:?}", errors);
    }

    #[test]
    fn order_independent_both_orderings_pass() {
        let field = list_enum_field("flags", vec!["docs_only", "small_local_fix"]);

        let entry_ab = entry_with_array("flags", &["docs_only", "small_local_fix"]);
        let mut errors = vec![];
        check_list_enum(&field, &["flags".to_string()], &entry_ab, &mut errors);
        assert!(errors.is_empty(), "[docs_only, small_local_fix] should pass");

        let entry_ba = entry_with_array("flags", &["small_local_fix", "docs_only"]);
        let mut errors = vec![];
        check_list_enum(&field, &["flags".to_string()], &entry_ba, &mut errors);
        assert!(errors.is_empty(), "[small_local_fix, docs_only] should pass");
    }

    #[test]
    fn multiple_bad_values_each_produce_one_error() {
        let field = list_enum_field("flags", vec!["docs_only"]);
        let entry = entry_with_array("flags", &["bad1", "bad2"]);
        let mut errors = vec![];
        check_list_enum(&field, &["flags".to_string()], &entry, &mut errors);
        assert_eq!(errors.len(), 2, "one error per bad value; got {:?}", errors);
        assert!(errors[0].message.contains("bad1"));
        assert!(errors[1].message.contains("bad2"));
    }

    #[test]
    fn absent_field_produces_no_error() {
        let field = list_enum_field("flags", vec!["docs_only"]);
        let entry: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let mut errors = vec![];
        check_list_enum(&field, &["flags".to_string()], &entry, &mut errors);
        assert!(errors.is_empty(), "absent field should not error: {:?}", errors);
    }

    #[test]
    fn no_list_enum_constraint_skips_check() {
        let field = Field {
            name: "tags".to_string(),
            ty: FieldType::List(Box::new(FieldType::Text)),
            required: false,
            required_when: None,
            pattern: None,
            actor: None,
            enum_values: None,
            description: None,
            auto_increment: false,
            auto_increment_within: None,
            default: None,
            list_enum: None,
        };
        let entry = entry_with_array("tags", &["anything", "goes"]);
        let mut errors = vec![];
        check_list_enum(&field, &["tags".to_string()], &entry, &mut errors);
        assert!(errors.is_empty(), "no list_enum = no constraint = pass");
    }
}
