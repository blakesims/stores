//! AC3.1: when `dot` is on PATH, `render_via_dot` exercises the live
//! production spawn path and produces non-empty UTF-8 output. Gated on
//! `dot -V` succeeding so CI without graphviz still passes (fallback is
//! covered by the unit tests in src/cli/topology.rs).

use std::collections::HashMap;
use std::path::PathBuf;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::cli::topology::{Format, Opts, RenderOutcome, emit_dot, render_via_dot};
use stores::manifest::{InstalledStore, Manifest};
use stores::schema::{Schema, StoreScope};

fn dot_on_path() -> bool {
    std::process::Command::new("dot")
        .arg("-V")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn ac3_1_render_via_dot_produces_utf8_when_graphviz_installed() {
    if !dot_on_path() {
        eprintln!("skipping: graphviz `dot` not on PATH");
        return;
    }

    let mut schemas: HashMap<String, Schema> = HashMap::new();
    for (name, yaml) in BUNDLED_STORE_SCHEMAS {
        schemas.insert((*name).to_string(), Schema::from_yaml(yaml).unwrap());
    }
    let manifest = Manifest {
        stores: vec![InstalledStore {
            name: "tasks".into(),
            schema_path: PathBuf::from("bundled:tasks"),
            installed_at: "fixture".into(),
            table_name: "tasks".into(),
            scope: StoreScope::Repo,
        }],
    };
    let opts = Opts {
        format: Format::Dot,
        store_filter: None,
        no_icons: false,
    };
    let source = emit_dot(&manifest, &schemas, &opts);

    match render_via_dot(&source) {
        RenderOutcome::Rendered(s) => {
            assert!(!s.is_empty(), "rendered output must be non-empty");
            assert!(s.is_char_boundary(s.len()), "rendered output must be valid UTF-8");
        }
        RenderOutcome::Fallback { reason, .. } => {
            panic!("expected Rendered with dot on PATH, got Fallback: {reason:?}");
        }
    }
}
