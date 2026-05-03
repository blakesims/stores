/// AC1.3: Validate all 5 agent fixture files against their corresponding JSON Schemas.
///
/// Each schema must validate the existing fixture in
/// `tests/fixtures/agent_outputs/<role>.json` using the `jsonschema` crate.
/// If a fixture fails, the schema is wrong — fixtures encode v0.3's correct
/// envelope shape.
///
/// Also includes a negative test asserting that `additionalProperties: false`
/// causes validation failure when a stray field is injected into each fixture.
use jsonschema::JSONSchema;
use serde_json::Value;
use std::path::PathBuf;

fn project_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is set by cargo test and points to the crate root.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load_json(path: &PathBuf) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("failed to parse JSON at {}: {e}", path.display()))
}

fn fixture_path(role: &str) -> PathBuf {
    project_root()
        .join("tests")
        .join("fixtures")
        .join("agent_outputs")
        .join(format!("{role}.json"))
}

fn schema_path(role: &str) -> PathBuf {
    project_root()
        .join("agents")
        .join("schemas")
        .join(format!("{role}.schema.json"))
}

struct RoleCase {
    role: &'static str,
    /// Extra field to inject for the negative (additionalProperties) test.
    stray_key: &'static str,
}

fn role_cases() -> &'static [RoleCase] {
    &[
        RoleCase {
            role: "planner",
            stray_key: "unexpected_planner_field",
        },
        RoleCase {
            role: "plan-reviewer",
            stray_key: "unexpected_pr_field",
        },
        RoleCase {
            role: "executor",
            stray_key: "unexpected_executor_field",
        },
        RoleCase {
            role: "code-reviewer",
            stray_key: "unexpected_cr_field",
        },
        RoleCase {
            role: "guide",
            stray_key: "unexpected_guide_field",
        },
        RoleCase {
            role: "wrap",
            stray_key: "unexpected_wrap_field",
        },
    ]
}

/// AC1.3: each fixture validates against its schema.
#[test]
fn all_fixtures_validate_against_schemas() {
    for case in role_cases() {
        let schema_val = load_json(&schema_path(case.role));
        let compiled = JSONSchema::compile(&schema_val)
            .unwrap_or_else(|e| panic!("schema for '{}' failed to compile: {e}", case.role));

        let fixture_val = load_json(&fixture_path(case.role));
        let result = compiled.validate(&fixture_val);
        if let Err(errors) = result {
            let msgs: Vec<String> = errors.map(|e| e.to_string()).collect();
            panic!(
                "fixture '{}' failed schema validation:\n{}",
                case.role,
                msgs.join("\n")
            );
        }
    }
}

/// AC1.3 negative: a fixture with a stray extra field must be rejected by
/// `additionalProperties: false`.
#[test]
fn fixtures_with_stray_field_rejected_by_schema() {
    for case in role_cases() {
        let schema_val = load_json(&schema_path(case.role));
        let compiled = JSONSchema::compile(&schema_val)
            .unwrap_or_else(|e| panic!("schema for '{}' failed to compile: {e}", case.role));

        let mut fixture_val = load_json(&fixture_path(case.role));
        // Inject a stray field.
        fixture_val
            .as_object_mut()
            .expect("fixture must be a JSON object")
            .insert(
                case.stray_key.to_string(),
                Value::String("stray".to_string()),
            );

        let result = compiled.validate(&fixture_val);
        assert!(
            result.is_err(),
            "schema for '{}' must reject a fixture with stray field '{}', but validation passed",
            case.role,
            case.stray_key
        );
    }
}
