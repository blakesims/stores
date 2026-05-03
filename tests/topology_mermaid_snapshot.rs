//! AC2.5: Golden-snapshot test — `emit_mermaid()` against
//! `tests/fixtures/topology/expected.md`.

use std::collections::HashMap;
use std::path::PathBuf;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::cli::topology::{emit_mermaid, Format, Opts};
use stores::manifest::{InstalledStore, Manifest};
use stores::schema::{Schema, StoreScope};

fn build_trio() -> (Manifest, HashMap<String, Schema>) {
    let mut schemas: HashMap<String, Schema> = HashMap::new();
    for (name, yaml) in BUNDLED_STORE_SCHEMAS {
        schemas.insert((*name).to_string(), Schema::from_yaml(yaml).unwrap());
    }
    let manifest = Manifest {
        stores: vec![
            InstalledStore {
                name: "tasks".into(),
                schema_path: PathBuf::from("bundled:tasks"),
                installed_at: "fixture".into(),
                table_name: "tasks".into(),
                scope: StoreScope::Repo,
            },
            InstalledStore {
                name: "observations".into(),
                schema_path: PathBuf::from("bundled:observations"),
                installed_at: "fixture".into(),
                table_name: "observations".into(),
                scope: StoreScope::Worktree,
            },
            InstalledStore {
                name: "gate".into(),
                schema_path: PathBuf::from("bundled:gate"),
                installed_at: "fixture".into(),
                table_name: "gate".into(),
                scope: StoreScope::Worktree,
            },
        ],
    };
    (manifest, schemas)
}

#[test]
fn ac2_5_mermaid_snapshot_matches() {
    let (manifest, schemas) = build_trio();
    let opts = Opts {
        format: Format::Mermaid,
        store_filter: None,
        no_icons: false,
    };
    let actual = emit_mermaid(&manifest, &schemas, &opts);

    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/topology/expected.md");

    if std::env::var("UPDATE_TOPOLOGY_FIXTURES").is_ok() {
        std::fs::write(&fixture_path, &actual).unwrap();
    }

    let expected = std::fs::read_to_string(&fixture_path).unwrap_or_default();
    assert_eq!(
        actual, expected,
        "mermaid snapshot mismatch — re-run with UPDATE_TOPOLOGY_FIXTURES=1 if intentional"
    );
}

#[test]
fn ac2_3_starts_with_z0_and_has_workflow_state_diagram() {
    let (manifest, schemas) = build_trio();
    let opts = Opts {
        format: Format::Mermaid,
        store_filter: None,
        no_icons: false,
    };
    let out = emit_mermaid(&manifest, &schemas, &opts);
    assert!(
        out.starts_with("## Z0"),
        "must begin with `## Z0` heading; got: {}",
        &out[..40]
    );
    // At least one stateDiagram-v2 block per workflow store (tasks):
    let count = out.matches("stateDiagram-v2").count();
    assert!(
        count >= 2,
        "expected at least 2 stateDiagram-v2 blocks (Z0 + Z2 tasks), found {count}"
    );
    assert!(
        out.contains("## Z2: tasks workflow firing order"),
        "Z2 tasks workflow heading missing"
    );
}
