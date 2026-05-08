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
use stores::cli::dynamic::{BUNDLED_STORE_NAMES, BUNDLED_STORE_SCHEMAS, BUNDLED_STORE_TEMPLATES};
use stores::schema::{FieldType, Schema};

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

#[test]
fn architecture_reviews_schema_and_template_fixture_parse() {
    let yaml =
        std::fs::read_to_string(project_root().join("stores/architecture_reviews/schema.yaml"))
            .expect("architecture_reviews schema fixture must be readable");
    let schema = Schema::from_yaml(&yaml).expect("architecture_reviews schema fixture must parse");

    assert_eq!(schema.name, "architecture_reviews");
    assert_eq!(schema.id_format, "A{:03d}");
    assert!(schema
        .lifecycle
        .states
        .iter()
        .any(|s| s == "awaiting_human_ratification"));

    let kind = schema.fields.iter().find(|f| f.name == "kind").unwrap();
    match &kind.ty {
        FieldType::Enum(values) => {
            assert_eq!(values, &vec!["interpret".to_string(), "amend".to_string()])
        }
        other => panic!("kind must be enum, got {other:?}"),
    }

    let verdict = schema.fields.iter().find(|f| f.name == "verdict").unwrap();
    match &verdict.ty {
        FieldType::Enum(values) => assert_eq!(values.len(), 7),
        other => panic!("verdict must be enum, got {other:?}"),
    }

    assert!(schema
        .fields
        .iter()
        .find(|f| f.name == "cascade_decisions")
        .and_then(|f| f.required_when.as_ref())
        .is_some());

    let template = std::fs::read_to_string(
        project_root().join("stores/architecture_reviews/templates/main.md.tpl"),
    )
    .expect("architecture_reviews main template fixture must be readable");
    for needle in [
        "{{display_id}}",
        "{{kind}}",
        "{{source_observation}}",
        "{{status}}",
    ] {
        assert!(template.contains(needle), "template missing {needle}");
    }
}

#[test]
fn architecture_reviews_is_bundled_with_render_template() {
    assert!(BUNDLED_STORE_NAMES.contains(&"architecture_reviews"));
    let bundled_yaml = BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(name, _)| *name == "architecture_reviews")
        .map(|(_, yaml)| *yaml)
        .expect("architecture_reviews bundled schema missing");
    let schema =
        Schema::from_yaml(bundled_yaml).expect("bundled architecture_reviews schema parses");
    assert_eq!(schema.name, "architecture_reviews");

    let template = BUNDLED_STORE_TEMPLATES
        .iter()
        .find(|(name, _)| *name == "architecture_reviews")
        .and_then(|(_, templates)| {
            templates
                .iter()
                .find(|(path, _)| *path == "templates/main.md.tpl")
        })
        .map(|(_, content)| *content)
        .expect("architecture_reviews bundled render template missing");
    assert!(template.contains("# {{display_id}}: {{summary}}"));
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

#[test]
fn t084_observations_source_tuple_schema_and_no_out_of_scope_drift() {
    let yaml = std::fs::read_to_string(project_root().join("stores/observations/schema.yaml"))
        .expect("observations schema readable");
    let schema = Schema::from_yaml(&yaml).expect("observations schema parses");

    let source = schema.fields.iter().find(|f| f.name == "source").unwrap();
    match &source.ty {
        FieldType::Enum(values) => assert_eq!(
            values,
            &vec![
                "dashboard".to_string(),
                "qa".to_string(),
                "dev".to_string(),
                "sentry".to_string(),
                "intake".to_string(),
                "converge".to_string(),
                "wrap".to_string(),
            ],
            "source enum value-set drifted"
        ),
        other => panic!("source must remain enum, got {other:?}"),
    }

    let source_env = schema.fields.iter().find(|f| f.name == "source_env").unwrap();
    match &source_env.ty {
        FieldType::Enum(values) => assert_eq!(values, &vec!["prod".to_string(), "sandbox".to_string()]),
        other => panic!("source_env must be enum, got {other:?}"),
    }
    assert!(!source_env.required);

    let source_id = schema.fields.iter().find(|f| f.name == "source_id").unwrap();
    assert!(matches!(source_id.ty, FieldType::Text));
    assert!(!source_id.required);

    for legacy in ["prod_source_id", "sandbox_source_id", "origin_db"] {
        let field = schema.fields.iter().find(|f| f.name == legacy).unwrap();
        assert!(!field.required, "{legacy} must remain nullable");
        assert!(field.description.as_deref().unwrap_or("").contains("DEPRECATED"), "{legacy} missing deprecation description");
    }

    let unchanged_fields = [
        ("qa_item_id", "integer", "QA checklist item id (source=qa dedup)"),
        ("tour_session_id", "integer", "Tour session id (source=qa dedup)"),
        ("step_index", "integer", "Step index within tour session (source=qa dedup)"),
        ("staff_user_id", "integer", "Staff user who triggered the QA observation"),
        ("message", "text", "Raw message text from the QA step (dedup key)"),
        ("contact_id", "integer", "Contact the observation is associated with"),
        ("field_name", "text", "Specific field name within the contact record, if applicable"),
    ];
    for (name, ty, description) in unchanged_fields {
        let field = schema.fields.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("{name} removed"));
        assert_eq!(format!("{:?}", field.ty).to_ascii_lowercase(), ty, "{name} type drifted");
        assert!(!field.required, "{name} required bit drifted");
        assert_eq!(field.description.as_deref(), Some(description), "{name} description drifted");
    }
}
