/// Build a `serde_json::Value` render context from a schema + entry map.
///
/// The output shape mirrors the schema fields, with one engine-only addition:
///   `current_cycle_for_phase` — a JSON object mapping phase_number (string key)
///   → latest cycle_number for that phase.
///
/// Example: if `cycles = [{phase_number: 1, cycle_number: 1},
///                          {phase_number: 1, cycle_number: 2},
///                          {phase_number: 2, cycle_number: 1}]`
/// then `current_cycle_for_phase = {"1": 2, "2": 1}`.
///
/// Missing or null entry values pass through as JSON null so templates receive
/// them and can use `{{default …}}` or `{{#if …}}` as needed.
use serde_json::{Map, Value};

use crate::schema::Schema;
use crate::validate::EntryMap;

/// Reserved column names that are always available in every store entry.
/// These are always included in the render context so templates can use
/// `{{status}}`, `{{display_id}}`, etc. regardless of whether the schema
/// explicitly declares them as fields.
const RESERVED_ENTRY_KEYS: &[&str] = &[
    "display_id",
    "status",
    "created_at",
    "updated_at",
    "created_by",
    "updated_by",
];

/// Build the template render context for a given schema entry.
pub fn build_context(schema: &Schema, entry: &EntryMap) -> Value {
    let mut obj = Map::new();

    // Include reserved columns first so templates can always use {{status}} etc.
    for key in RESERVED_ENTRY_KEYS {
        let val = entry.get(*key).cloned().unwrap_or(Value::Null);
        obj.insert(key.to_string(), val);
    }

    // Copy every top-level schema field value into the context (may overlap with
    // reserved keys if a schema declares a field with the same name — schema field
    // wins since it's inserted after).
    for field in &schema.fields {
        let val = entry
            .get(&field.name)
            .cloned()
            .unwrap_or(Value::Null);
        obj.insert(field.name.clone(), val);
    }

    // Derive `current_cycle_for_phase` from the `cycles` list_record field if present.
    // The cycles array elements are expected to be JSON objects with at least:
    //   `phase_number` (integer) and `cycle_number` (integer).
    // We also accept `phase` / `cycle` as fallback aliases for ergonomics with
    // schemas that use shorter names.
    let ccfp = derive_current_cycle_for_phase(entry);
    obj.insert("current_cycle_for_phase".to_string(), ccfp);

    // Derive `plan_phases_count` — number of phases in plan.phases (0 if absent).
    // Derive `current_phase_idx` — current_phase minus 1 (for 0-based {{#each}} index matching).
    let plan_phases_count = entry
        .get("plan")
        .and_then(|v| parse_json_if_string(v))
        .and_then(|v| v.get("phases").and_then(|p| p.as_array()).map(|a| a.len()))
        .unwrap_or(0);
    obj.insert("plan_phases_count".to_string(), Value::from(plan_phases_count as i64));

    let current_phase = entry
        .get("current_phase")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let current_phase_idx = if current_phase > 0 { current_phase - 1 } else { 0 };
    obj.insert("current_phase_idx".to_string(), Value::from(current_phase_idx));

    // Derive `cycles_have_reviews` — true if any cycle has a non-null review.
    let cycles_have_reviews = entry
        .get("cycles")
        .and_then(|v| parse_json_if_string(v))
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .any(|c| c.get("review").map(|r| !r.is_null()).unwrap_or(false));
    obj.insert("cycles_have_reviews".to_string(), Value::from(cycles_have_reviews));

    Value::Object(obj)
}

/// Parse a Value if it's a JSON string, or clone it if already a Value.
fn parse_json_if_string(v: &Value) -> Option<Value> {
    match v {
        Value::String(s) => serde_json::from_str(s).ok(),
        other => Some(other.clone()),
    }
}

/// Scan the `cycles` field of the entry and compute the max cycle_number per
/// phase_number.  Returns a JSON object with string keys (phase number) and
/// integer values (latest cycle number for that phase).
fn derive_current_cycle_for_phase(entry: &EntryMap) -> Value {
    let mut map = Map::new();

    let cycles_val = match entry.get("cycles") {
        Some(v) => v,
        None => return Value::Object(map),
    };

    // cycles can be:
    //   1. A JSON array (already parsed from DB)
    //   2. A JSON string (raw TEXT from DB that needs parsing)
    let arr: Vec<Value> = match cycles_val {
        Value::Array(a) => a.clone(),
        Value::String(s) => {
            // Try to parse as JSON array
            match serde_json::from_str::<Value>(s) {
                Ok(Value::Array(a)) => a,
                _ => return Value::Object(map),
            }
        }
        _ => return Value::Object(map),
    };

    for item in &arr {
        let obj = match item.as_object() {
            Some(o) => o,
            None => continue,
        };

        // Accept `phase_number` or `phase` as the phase key.
        let phase_num = obj
            .get("phase_number")
            .or_else(|| obj.get("phase"))
            .and_then(|v| v.as_i64());

        // Accept `cycle_number` or `cycle` as the cycle key.
        let cycle_num = obj
            .get("cycle_number")
            .or_else(|| obj.get("cycle"))
            .and_then(|v| v.as_i64());

        if let (Some(ph), Some(cy)) = (phase_num, cycle_num) {
            let key = ph.to_string();
            let current = map
                .get(&key)
                .and_then(|v| v.as_i64())
                .unwrap_or(i64::MIN);
            if cy > current {
                map.insert(key, Value::Number(cy.into()));
            }
        }
    }

    Value::Object(map)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Schema;
    use serde_json::json;

    fn wf_schema() -> Schema {
        let yaml = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/workflow_minimal/schema.yaml"),
        )
        .unwrap();
        Schema::from_yaml(&yaml).unwrap()
    }

    fn entry_from(v: Value) -> EntryMap {
        match v {
            Value::Object(m) => m.into_iter().collect(),
            _ => panic!("expected object"),
        }
    }

    // AC3.5: top-level keys include reserved columns, schema field names, and
    // the engine-only `current_cycle_for_phase` key.
    #[test]
    fn context_top_level_keys_match_schema_plus_engine_key() {
        let schema = wf_schema();
        let entry = entry_from(json!({
            "title": "Test task",
            "status": "planning"
        }));
        let ctx = build_context(&schema, &entry);
        let obj = ctx.as_object().unwrap();

        // All reserved columns must be present.
        for key in RESERVED_ENTRY_KEYS {
            assert!(
                obj.contains_key(*key),
                "missing reserved key '{}' in context",
                key
            );
        }
        // All schema field names must be present.
        for field in &schema.fields {
            assert!(
                obj.contains_key(&field.name),
                "missing field '{}' in context",
                field.name
            );
        }
        // Engine-only key must be present.
        assert!(obj.contains_key("current_cycle_for_phase"));
        // Total count: unique(reserved ∪ schema_fields) + engine-derived keys.
        // Engine-derived keys: current_cycle_for_phase, plan_phases_count,
        // current_phase_idx, cycles_have_reviews (4 total).
        // Some schema fields may overlap with reserved names (e.g. display_id);
        // use a set to count unique keys.
        let mut expected_keys: std::collections::BTreeSet<&str> =
            RESERVED_ENTRY_KEYS.iter().copied().collect();
        for f in &schema.fields {
            expected_keys.insert(f.name.as_str());
        }
        const ENGINE_DERIVED_KEY_COUNT: usize = 4; // current_cycle_for_phase, plan_phases_count, current_phase_idx, cycles_have_reviews
        let expected_count = expected_keys.len() + ENGINE_DERIVED_KEY_COUNT;
        assert_eq!(
            obj.len(),
            expected_count,
            "context has {} keys, expected {}; keys: {:?}",
            obj.len(),
            expected_count,
            obj.keys().collect::<Vec<_>>()
        );
    }

    // current_cycle_for_phase: basic derivation.
    #[test]
    fn current_cycle_for_phase_derived_correctly() {
        let schema = wf_schema();
        let entry = entry_from(json!({
            "title": "T",
            "cycles": [
                {"phase_number": 1, "cycle_number": 1},
                {"phase_number": 1, "cycle_number": 2},
                {"phase_number": 2, "cycle_number": 1}
            ]
        }));
        let ctx = build_context(&schema, &entry);
        let ccfp = &ctx["current_cycle_for_phase"];
        assert_eq!(ccfp["1"], json!(2));
        assert_eq!(ccfp["2"], json!(1));
    }

    // current_cycle_for_phase: empty when no cycles.
    #[test]
    fn current_cycle_for_phase_empty_when_no_cycles() {
        let schema = wf_schema();
        let entry = entry_from(json!({"title": "T"}));
        let ctx = build_context(&schema, &entry);
        let ccfp = ctx["current_cycle_for_phase"].as_object().unwrap();
        assert!(ccfp.is_empty());
    }

    // current_cycle_for_phase: accepts short aliases (phase / cycle).
    #[test]
    fn current_cycle_for_phase_short_aliases() {
        let schema = wf_schema();
        let entry = entry_from(json!({
            "title": "T",
            "cycles": [
                {"phase": 3, "cycle": 5}
            ]
        }));
        let ctx = build_context(&schema, &entry);
        let ccfp = &ctx["current_cycle_for_phase"];
        assert_eq!(ccfp["3"], json!(5));
    }

    // current_cycle_for_phase: cycles stored as JSON string (DB TEXT).
    #[test]
    fn current_cycle_for_phase_from_json_string() {
        let schema = wf_schema();
        let entry = entry_from(json!({
            "title": "T",
            "cycles": "[{\"phase_number\":1,\"cycle_number\":3}]"
        }));
        let ctx = build_context(&schema, &entry);
        let ccfp = &ctx["current_cycle_for_phase"];
        assert_eq!(ccfp["1"], json!(3));
    }

    // Missing fields render as null in context (not absent).
    #[test]
    fn missing_field_renders_as_null() {
        let schema = wf_schema();
        let entry = entry_from(json!({"title": "T"}));
        let ctx = build_context(&schema, &entry);
        // description is not in entry — should appear as null
        assert_eq!(ctx["description"], Value::Null);
    }

    // AC3.5 fixture: render planner-brief.md.tpl byte-for-byte against a known context.
    #[test]
    fn planner_brief_fixture_renders_correctly() {
        use crate::render::render_template;

        let tpl_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/workflow_minimal/templates/planner-brief.md.tpl");
        let tpl = std::fs::read_to_string(&tpl_path).unwrap();

        let schema = wf_schema();
        // cycles is the list_record field in the schema; use it for {{#each}} iteration.
        let entry = entry_from(json!({
            "title": "Implement feature X",
            "status": "planning",
            "current_phase": 1,
            "description": "Build something great.",
            "cycles": [
                {"phase": 1, "summary": "Research done"},
                {"phase": 2, "summary": "Code written"}
            ]
        }));
        let ctx = build_context(&schema, &entry);
        let rendered = render_template(&tpl, &ctx).unwrap();

        // Verify all four substitution patterns are exercised:
        // 1. Text passthrough — the heading text "Planner Briefing" is literal.
        assert!(rendered.contains("# Planner Briefing —"), "static text missing");
        // 2. Variable substitution — {{title}}.
        assert!(rendered.contains("Implement feature X"), "title substitution failed");
        // 3. {{#each cycles}} list iteration.
        assert!(rendered.contains("- Phase 1: Research done"), "each cycles item 1 failed");
        assert!(rendered.contains("- Phase 2: Code written"), "each cycles item 2 failed");
        // 4. {{#if (eq status "BLOCKED")}} — status is "planning" so else branch fires.
        assert!(rendered.contains("Not blocked."), "eq conditional failed");
        assert!(!rendered.contains("This task is blocked"), "eq true branch should not fire");

        // Full byte-for-byte expected output.
        // Note: Handlebars {{#each}} emits each item with its trailing newline
        // but does NOT add a blank line after the block — the blank line before
        // "## Blocked Reason" comes from the template line that follows {{/each}}.
        let expected = "# Planner Briefing — Implement feature X\n\
\n\
**Status:** planning\n\
**Phase:** 1\n\
\n\
## Objective\n\
\n\
Build something great.\n\
\n\
## Prior Cycles\n\
\n\
- Phase 1: Research done\n\
- Phase 2: Code written\n\
## Blocked Reason\n\
\n\
Not blocked.\n\
\n\
## Instructions\n\
\n\
You are the planner. Create an implementation plan.\n";

        assert_eq!(
            rendered, expected,
            "rendered output does not match expected byte-for-byte"
        );
    }
}
