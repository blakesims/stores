//! AC3.1: when `graph-easy` is on PATH, `render_via_dot` exercises the
//! live production spawn path and produces non-empty UTF-8 output.
//! Gated on `graph-easy --version` succeeding so CI without it still
//! passes (fallback is covered by the unit tests in src/cli/topology.rs).
//!
//! L036: pre-fix this gated on `dot -V` and asserted the renderer (then
//! `dot -Tutf8`) emitted boxart. `dot -Tutf8` is not a real graphviz
//! format, so the live path failed silently — the gate skipped on
//! hosts without graphviz, and on hosts WITH graphviz the test would
//! have panicked at Fallback. The renderer is now `graph-easy
//! --as=boxart` (Debian/Ubuntu pkg `libgraph-easy-perl`); the gate
//! moves to match.

use std::collections::HashMap;
use std::path::PathBuf;

use stores::cli::dynamic::BUNDLED_STORE_SCHEMAS;
use stores::cli::topology::{emit_dot, render_auto, render_via_dot, Format, Opts, RenderOutcome};
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

fn graph_easy_on_path() -> bool {
    std::process::Command::new("graph-easy")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn ac3_1_render_via_dot_produces_utf8_when_graphviz_installed() {
    if !graph_easy_on_path() {
        eprintln!("skipping: `graph-easy` not on PATH (apt install libgraph-easy-perl)");
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
            assert!(
                s.is_char_boundary(s.len()),
                "rendered output must be valid UTF-8"
            );
        }
        RenderOutcome::Fallback { reason, .. } => {
            panic!("expected Rendered with dot on PATH, got Fallback: {reason:?}");
        }
    }
}

/// AC2.1: every line of `--format auto` output ≤ 120 columns.
#[test]
fn ac_max_line_width_under_120() {
    if !graph_easy_on_path() {
        eprintln!("skipping: `graph-easy` not on PATH (apt install libgraph-easy-perl)");
        return;
    }
    let (manifest, schemas) = build_trio();
    let opts = Opts {
        format: Format::Auto,
        store_filter: None,
        no_icons: false,
    };
    let out = render_auto(&manifest, &schemas, &opts);
    for (idx, line) in out.lines().enumerate() {
        let width = line.chars().count();
        assert!(
            width <= 120,
            "line {idx} is {width} cols (>120): {line:?}"
        );
    }
}

/// AC2.2: rendered auto output contains all expected `## Zk:` headers.
#[test]
fn ac_zone_headers_present() {
    if !graph_easy_on_path() {
        eprintln!("skipping: `graph-easy` not on PATH (apt install libgraph-easy-perl)");
        return;
    }
    let (manifest, schemas) = build_trio();
    let opts = Opts {
        format: Format::Auto,
        store_filter: None,
        no_icons: false,
    };
    let out = render_auto(&manifest, &schemas, &opts);
    for header in &[
        "## Z0: cross-store soft-FKs",
        "## Z1: tasks state machine",
        "## Z1: observations state machine",
        "## Z1: gate state machine",
        "## Z2: tasks workflow firing order",
    ] {
        assert!(
            out.contains(header),
            "expected header {header:?} not found in rendered output"
        );
    }
}
