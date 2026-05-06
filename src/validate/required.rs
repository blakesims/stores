use crate::schema::{Field, RequiredWhenExpr};
use crate::validate::error::RuleKind;
use crate::validate::{EntryMap, ValidationError};
use serde_json::Value;

/// Walk the entry using a dotted path, descending into nested Objects.
pub fn lookup<'a>(entry: &'a EntryMap, path: &[String]) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    // First segment is a key in the top-level EntryMap.
    let first = entry.get(&path[0])?;
    if path.len() == 1 {
        return Some(first);
    }
    // Recurse into nested Value::Object for subsequent segments.
    let mut cur = first;
    for seg in &path[1..] {
        match cur {
            Value::Object(map) => {
                cur = map.get(seg)?;
            }
            _ => return None,
        }
    }
    Some(cur)
}

/// Check `required` and `required_when` rules for a single field.
///
/// `field_path` is the fully-qualified dot-path for this field (e.g.
/// `["contract", "done_when"]`).  The lookup for the field's own value uses
/// that path.  The lookup for the `required_when` LHS is resolved from the
/// top-level `entry` regardless of nesting, so a sub-field expression like
/// `triage.verdict == 'T3'` reaches out of its parent Record into a sibling.
pub fn check_required(
    field: &Field,
    field_path: &[String],
    entry: &EntryMap,
    errors: &mut Vec<ValidationError>,
) {
    let field_value = lookup(entry, field_path);
    let is_absent = field_value.is_none() || field_value == Some(&Value::Null);

    // --- required ---
    if field.required && is_absent {
        errors.push(ValidationError {
            field_path: field_path.to_vec(),
            rule: RuleKind::Required,
            message: "required".to_string(),
        });
    }

    // --- required_when ---
    if let Some(RequiredWhenExpr {
        lhs_path,
        rhs_literal,
    }) = &field.required_when
    {
        let lhs_value = lookup(entry, lhs_path);
        let triggered = lhs_value.and_then(|v| v.as_str()) == Some(rhs_literal.as_str());

        if triggered && is_absent {
            let condition = format!("{} == '{}'", lhs_path.join("."), rhs_literal);
            errors.push(ValidationError {
                field_path: field_path.to_vec(),
                rule: RuleKind::RequiredWhen {
                    expr: condition.clone(),
                },
                message: format!("required (because {})", condition),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::required_when::parse as parse_rw;
    use crate::schema::{Field, FieldType};
    use std::collections::BTreeMap;

    fn text_field(name: &str, required: bool, required_when: Option<&str>) -> Field {
        Field {
            name: name.to_string(),
            ty: FieldType::Text,
            required,
            required_when: required_when.map(|s| parse_rw(s).unwrap()),
            pattern: None,
            actor: None,
            enum_values: None,
            description: None,
            auto_increment: false,
            auto_increment_within: None,
            default: None,
        }
    }

    #[test]
    fn required_field_missing_fires() {
        let field = text_field("summary", true, None);
        let entry: EntryMap = BTreeMap::new();
        let mut errors = vec![];
        check_required(&field, &["summary".to_string()], &entry, &mut errors);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].rule, RuleKind::Required);
    }

    #[test]
    fn required_field_present_ok() {
        let field = text_field("summary", true, None);
        let mut entry: EntryMap = BTreeMap::new();
        entry.insert("summary".into(), serde_json::Value::String("hello".into()));
        let mut errors = vec![];
        check_required(&field, &["summary".to_string()], &entry, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn required_when_triggers_when_condition_met_and_field_absent() {
        let field = text_field("done_when", false, Some("triage.verdict == 'T3'"));
        // Entry: triage.verdict = T3, contract.done_when absent
        let mut entry: EntryMap = BTreeMap::new();
        let mut triage = serde_json::Map::new();
        triage.insert("verdict".into(), serde_json::Value::String("T3".into()));
        entry.insert("triage".into(), serde_json::Value::Object(triage));

        let mut errors = vec![];
        check_required(
            &field,
            &["contract".to_string(), "done_when".to_string()],
            &entry,
            &mut errors,
        );
        assert_eq!(errors.len(), 1);
        match &errors[0].rule {
            RuleKind::RequiredWhen { expr } => {
                assert!(expr.contains("triage.verdict"), "expr: {expr}");
                assert!(expr.contains("T3"), "expr: {expr}");
            }
            other => panic!("expected RequiredWhen, got {:?}", other),
        }
    }

    #[test]
    fn required_when_silent_when_condition_not_met() {
        let field = text_field("done_when", false, Some("triage.verdict == 'T3'"));
        let mut entry: EntryMap = BTreeMap::new();
        let mut triage = serde_json::Map::new();
        triage.insert("verdict".into(), serde_json::Value::String("T1".into()));
        entry.insert("triage".into(), serde_json::Value::Object(triage));

        let mut errors = vec![];
        check_required(
            &field,
            &["contract".to_string(), "done_when".to_string()],
            &entry,
            &mut errors,
        );
        assert!(errors.is_empty(), "should be silent when verdict is T1");
    }

    #[test]
    fn required_when_silent_when_field_present() {
        let field = text_field("done_when", false, Some("triage.verdict == 'T3'"));
        let mut entry: EntryMap = BTreeMap::new();
        let mut triage = serde_json::Map::new();
        triage.insert("verdict".into(), serde_json::Value::String("T3".into()));
        entry.insert("triage".into(), serde_json::Value::Object(triage));
        let mut contract = serde_json::Map::new();
        contract.insert(
            "done_when".into(),
            serde_json::Value::String("fix it".into()),
        );
        entry.insert("contract".into(), serde_json::Value::Object(contract));

        let mut errors = vec![];
        check_required(
            &field,
            &["contract".to_string(), "done_when".to_string()],
            &entry,
            &mut errors,
        );
        assert!(errors.is_empty());
    }

    /// AC2: cross-Record required_when — sub-field whose lhs_path resolves into a
    /// sibling Record (contract.done_when; required_when: triage.verdict == 'T3').
    #[test]
    fn cross_record_required_when_fires_for_t3() {
        let field = text_field("done_when", false, Some("triage.verdict == 'T3'"));
        let mut entry: EntryMap = BTreeMap::new();
        let mut triage = serde_json::Map::new();
        triage.insert("verdict".into(), serde_json::Value::String("T3".into()));
        entry.insert("triage".into(), serde_json::Value::Object(triage));
        // contract.done_when absent

        let mut errors = vec![];
        check_required(
            &field,
            &["contract".to_string(), "done_when".to_string()],
            &entry,
            &mut errors,
        );
        assert_eq!(errors.len(), 1, "must fire for T3");
        assert_eq!(errors[0].field_dot(), "contract.done_when");
    }

    #[test]
    fn cross_record_required_when_silent_for_non_t3() {
        let field = text_field("done_when", false, Some("triage.verdict == 'T3'"));
        let mut entry: EntryMap = BTreeMap::new();
        let mut triage = serde_json::Map::new();
        triage.insert("verdict".into(), serde_json::Value::String("T2".into()));
        entry.insert("triage".into(), serde_json::Value::Object(triage));

        let mut errors = vec![];
        check_required(
            &field,
            &["contract".to_string(), "done_when".to_string()],
            &entry,
            &mut errors,
        );
        assert!(errors.is_empty(), "must be silent for non-T3");
    }
}
