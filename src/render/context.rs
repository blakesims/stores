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

/// Build the template render context for a given schema entry.
pub fn build_context(schema: &Schema, entry: &EntryMap) -> Value {
    let mut obj = Map::new();

    // Copy every top-level entry value into the context.
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

    Value::Object(obj)
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

    // AC3.5: top-level keys equal schema field names plus current_cycle_for_phase.
    #[test]
    fn context_top_level_keys_match_schema_plus_engine_key() {
        let schema = wf_schema();
        let entry = entry_from(json!({
            "title": "Test task",
            "status": "planning"
        }));
        let ctx = build_context(&schema, &entry);
        let obj = ctx.as_object().unwrap();

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
        // No extra keys beyond schema fields + engine key.
        let expected_count = schema.fields.len() + 1;
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
