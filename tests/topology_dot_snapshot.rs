//! AC2.4: Golden-snapshot test — `emit_dot()` against
//! `tests/fixtures/topology/expected.dot` for the bundled
//! tasks+observations+gate trio.

use std::collections::HashMap;
use std::path::PathBuf;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::cli::topology::{emit_dot, Format, Opts};
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

/// RAII guard that saves `NO_COLOR` on construction and restores it on drop.
/// Prevents env mutation from leaking into later tests in the same process.
struct NoColorGuard {
    saved: Option<std::ffi::OsString>,
}

impl NoColorGuard {
    fn new() -> Self {
        let saved = std::env::var_os("NO_COLOR");
        std::env::remove_var("NO_COLOR");
        Self { saved }
    }
}

impl Drop for NoColorGuard {
    fn drop(&mut self) {
        match &self.saved {
            Some(val) => std::env::set_var("NO_COLOR", val),
            None => std::env::remove_var("NO_COLOR"),
        }
    }
}

#[test]
fn ac2_4_dot_snapshot_matches() {
    // Remove NO_COLOR so the snapshot is always the colored variant,
    // regardless of the operator shell environment (fixes topology flake I003).
    // The guard restores the previous value (or removes the var) on test exit.
    let _guard = NoColorGuard::new();
    let (manifest, schemas) = build_trio();
    let opts = Opts {
        format: Format::Dot,
        store_filter: None,
        no_icons: false,
    };
    let actual = emit_dot(&manifest, &schemas, &opts);

    let fixture_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/topology/expected.dot");

    if std::env::var("UPDATE_TOPOLOGY_FIXTURES").is_ok() {
        std::fs::write(&fixture_path, &actual).unwrap();
    }

    let expected = std::fs::read_to_string(&fixture_path).unwrap_or_default();
    assert_eq!(
        actual, expected,
        "dot snapshot mismatch — re-run with UPDATE_TOPOLOGY_FIXTURES=1 if intentional"
    );
}

#[test]
fn ac2_1_three_zone_clusters_present() {
    let (manifest, schemas) = build_trio();
    let opts = Opts {
        format: Format::Dot,
        store_filter: None,
        no_icons: false,
    };
    let out = emit_dot(&manifest, &schemas, &opts);
    assert!(
        out.starts_with("digraph stores_topology {"),
        "must start with digraph header"
    );
    assert!(
        out.contains("subgraph cluster_z0_cross_store"),
        "Z0 cluster missing"
    );
    assert!(
        out.contains("subgraph cluster_z1_tasks"),
        "Z1 tasks cluster missing"
    );
    assert!(
        out.contains("subgraph cluster_z1_observations"),
        "Z1 observations cluster missing"
    );
    assert!(
        out.contains("subgraph cluster_z1_gate"),
        "Z1 gate cluster missing"
    );
    assert!(
        out.contains("subgraph cluster_z2_workflow_tasks"),
        "Z2 workflow_tasks cluster missing"
    );
    assert!(out.trim_end().ends_with('}'), "must end with closing brace");
}

#[test]
fn ac2_6_store_filter_z1_only_but_z0_complete() {
    let (manifest, schemas) = build_trio();
    let opts = Opts {
        format: Format::Dot,
        store_filter: Some("tasks".into()),
        no_icons: false,
    };
    let out = emit_dot(&manifest, &schemas, &opts);
    // Z0 still references all installed stores
    assert!(
        out.contains("\"z0_observations\""),
        "Z0 must still show observations"
    );
    assert!(out.contains("\"z0_gate\""), "Z0 must still show gate");
    assert!(out.contains("\"z0_tasks\""), "Z0 must show tasks");
    // Z1 only contains tasks
    assert!(
        out.contains("subgraph cluster_z1_tasks"),
        "Z1 tasks must be present"
    );
    assert!(
        !out.contains("subgraph cluster_z1_observations"),
        "Z1 observations must be filtered out"
    );
    assert!(
        !out.contains("subgraph cluster_z1_gate"),
        "Z1 gate must be filtered out"
    );
}

#[test]
fn ac2_8_no_icons_uses_text_codes() {
    let (manifest, schemas) = build_trio();
    let opts = Opts {
        format: Format::Dot,
        store_filter: None,
        no_icons: true,
    };
    let out = emit_dot(&manifest, &schemas, &opts);
    // text-codes appear in labels (multi-line: actor prefix \n verb [\n [GATE]]):
    assert!(
        out.contains("[label=\"A\\nsubmit-plan\""),
        "expected A-prefixed text-code label (multi-line, no gate): {out}"
    );
    assert!(
        out.contains("H+\\nresume"),
        "expected H+ text-code for resume (multi-line)"
    );
    // No Nerd Font glyphs (the four code points)
    for glyph in &["\u{f544}", "\u{f2b5}", "\u{f007}", "\u{f013}"] {
        assert!(
            !out.contains(glyph),
            "no-icons mode must not contain glyph U+{:04X}",
            glyph.chars().next().unwrap() as u32
        );
    }
}

/// AC2.2: graphviz syntax check — gated on `dot` being on PATH.
#[test]
fn ac2_2_dot_syntax_valid_when_graphviz_installed() {
    let dot_present = std::process::Command::new("dot")
        .arg("-V")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !dot_present {
        eprintln!("skipping: graphviz `dot` not on PATH");
        return;
    }
    let (manifest, schemas) = build_trio();
    let opts = Opts {
        format: Format::Dot,
        store_filter: None,
        no_icons: false,
    };
    let src = emit_dot(&manifest, &schemas, &opts);
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("dot")
        .args(["-Tsvg", "-o", "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(src.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "dot rejected emitted source: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
