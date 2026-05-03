//! `stores topology` — static schematic of the substrate.
//!
//! Phase 2: dot + mermaid emitters for the three-zone schematic
//! (Z0 cross-store FKs, Z1 per-store state machines, Z2 workflow firing order).

use anyhow::Result;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::process::{Command, Stdio};

use crate::manifest::Manifest;
use crate::schema::actor::Actor;
use crate::schema::Schema;
use crate::schema::{FieldType, StateAction};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Auto,
    Dot,
    Mermaid,
}

#[derive(Debug, Clone)]
pub struct Opts {
    pub format: Format,
    pub store_filter: Option<String>,
    pub no_icons: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorStyle {
    pub dot_color: &'static str,
    pub icon: &'static str,
    pub text_code: &'static str,
    pub label_prefix: String,
}

/// Resolve an actor (or absence thereof) to a renderable style.
///
/// `color_disabled` is true when `NO_COLOR` is set or the caller is the mermaid
/// emitter (which has no per-edge color); in that case `dot_color` is "" so the
/// caller can skip emitting a `color=` attribute.
///
/// `no_icons` strips the Nerd Font glyph; `label_prefix` becomes the text code
/// (`A` / `H+` / `H!` / `F`) on its own. With icons enabled, the prefix is
/// `<icon> <text_code>`.
pub fn actor_style(actor: Option<Actor>, no_icons: bool, color_disabled: bool) -> ActorStyle {
    let (dot_color, icon, text_code) = match actor {
        Some(Actor::AiAutonomous) => ("green", "\u{f544}", "A"),
        Some(Actor::AiWithHuman) => ("gold", "\u{f2b5}", "H+"),
        Some(Actor::Human) => ("red", "\u{f007}", "H!"),
        Some(Actor::Framework) | None => ("gray", "\u{f013}", "F"),
    };

    let label_prefix = if no_icons {
        text_code.to_string()
    } else {
        format!("{icon} {text_code}")
    };

    ActorStyle {
        dot_color: if color_disabled { "" } else { dot_color },
        icon,
        text_code,
        label_prefix,
    }
}

fn no_color_env() -> bool {
    std::env::var("NO_COLOR").is_ok()
}

// ---------------------------------------------------------------------------
// Dot emitter
// ---------------------------------------------------------------------------

/// Emit a graphviz `digraph` source for the topology.
///
/// Determinism: stores are walked in `manifest.stores` order; states/transitions
/// in their declared order. No HashMap iteration appears in output paths.
pub fn emit_dot(manifest: &Manifest, schemas: &HashMap<String, Schema>, opts: &Opts) -> String {
    let color_disabled = no_color_env();
    let mut out = String::new();
    out.push_str("digraph stores_topology {\n");
    out.push_str("  rankdir=TB;\n");
    out.push_str("  compound=true;\n");
    out.push('\n');

    write_z0_dot(&mut out, manifest, schemas);

    for store in &manifest.stores {
        if let Some(filter) = &opts.store_filter {
            if &store.name != filter {
                continue;
            }
        }
        if let Some(schema) = schemas.get(&store.name) {
            write_z1_dot(&mut out, &store.name, schema, opts, color_disabled);
        }
    }

    for store in &manifest.stores {
        if let Some(filter) = &opts.store_filter {
            if &store.name != filter {
                continue;
            }
        }
        if let Some(schema) = schemas.get(&store.name) {
            if schema.workflow.is_some() {
                write_z2_dot(&mut out, &store.name, schema, opts, color_disabled);
            }
        }
    }

    out.push_str("}\n");
    out
}

fn write_z0_dot(out: &mut String, manifest: &Manifest, schemas: &HashMap<String, Schema>) {
    out.push_str("  subgraph cluster_z0_cross_store {\n");
    out.push_str("    label=\"Z0: cross-store soft-FKs\";\n");
    write_z0_body(out, manifest, schemas, "    ");
    out.push_str("  }\n\n");
}

fn write_z0_body(
    out: &mut String,
    manifest: &Manifest,
    schemas: &HashMap<String, Schema>,
    indent: &str,
) {
    for store in &manifest.stores {
        let _ = writeln!(
            out,
            "{indent}\"z0_{}\" [shape=box, label=\"{}\"];",
            store.name, store.name
        );
    }

    for store in &manifest.stores {
        let Some(schema) = schemas.get(&store.name) else {
            continue;
        };
        for field in &schema.fields {
            if let FieldType::ListFk { ref_store } = &field.ty {
                if manifest.stores.iter().any(|s| &s.name == ref_store) {
                    let _ = writeln!(
                        out,
                        "{indent}\"z0_{}\" -> \"z0_{}\" [label=\"{}\"];",
                        store.name, ref_store, field.name
                    );
                }
            }
        }
    }
}

/// Emit a standalone `digraph` for the Z0 cross-store soft-FK zone.
///
/// Output begins with `digraph ` and ends with `}\n`; no `subgraph cluster_*`
/// wrapper, no `compound=true`. Suitable for rendering one zone at a time
/// through `graph-easy --as=boxart`.
pub fn emit_zone_z0_dot(manifest: &Manifest, schemas: &HashMap<String, Schema>) -> String {
    let mut out = String::new();
    out.push_str("digraph z0 {\n");
    out.push_str("  rankdir=TB;\n");
    write_z0_body(&mut out, manifest, schemas, "  ");
    out.push_str("}\n");
    out
}

fn write_z1_dot(
    out: &mut String,
    store_name: &str,
    schema: &Schema,
    opts: &Opts,
    color_disabled: bool,
) {
    let _ = writeln!(out, "  subgraph cluster_z1_{store_name} {{");
    let _ = writeln!(out, "    label=\"Z1: {store_name} state machine\";");
    write_z1_body(out, store_name, schema, opts, color_disabled, "    ");
    out.push_str("  }\n\n");
}

fn write_z1_body(
    out: &mut String,
    store_name: &str,
    schema: &Schema,
    opts: &Opts,
    color_disabled: bool,
    indent: &str,
) {
    let initial = schema.lifecycle.resolved_initial_state().unwrap_or("");
    for state in &schema.lifecycle.states {
        if state == initial {
            let _ = writeln!(
                out,
                "{indent}\"z1_{store_name}__{state}\" [label=\"{state}\", style=bold, peripheries=2];"
            );
        } else {
            let _ = writeln!(
                out,
                "{indent}\"z1_{store_name}__{state}\" [label=\"{state}\"];"
            );
        }
    }

    for t in &schema.lifecycle.transitions {
        let style = actor_style(t.actor, opts.no_icons, color_disabled);
        let label = format!("{} {}", style.label_prefix, t.verb);
        let color_attr = if style.dot_color.is_empty() {
            String::new()
        } else {
            format!(", color={}, fontcolor={}", style.dot_color, style.dot_color)
        };
        let _ = writeln!(
            out,
            "{indent}\"z1_{store_name}__{}\" -> \"z1_{store_name}__{}\" [label=\"{}\"{}];",
            t.from, t.to, label, color_attr
        );
    }
}

/// Emit a standalone `digraph` for one store's Z1 state machine.
pub fn emit_zone_z1_dot(store_name: &str, schema: &Schema, opts: &Opts) -> String {
    let color_disabled = no_color_env();
    let mut out = String::new();
    let _ = writeln!(out, "digraph z1_{store_name} {{");
    out.push_str("  rankdir=TB;\n");
    write_z1_body(&mut out, store_name, schema, opts, color_disabled, "  ");
    out.push_str("}\n");
    out
}

fn write_z2_dot(
    out: &mut String,
    store_name: &str,
    schema: &Schema,
    opts: &Opts,
    color_disabled: bool,
) {
    let _ = writeln!(out, "  subgraph cluster_z2_workflow_{store_name} {{");
    let _ = writeln!(out, "    label=\"Z2: {store_name} workflow firing order\";");
    write_z2_body(out, store_name, schema, opts, color_disabled, "    ");
    out.push_str("  }\n\n");
}

fn write_z2_body(
    out: &mut String,
    store_name: &str,
    schema: &Schema,
    opts: &Opts,
    color_disabled: bool,
    indent: &str,
) {
    let wf = schema.workflow.as_ref().unwrap();
    // Walk lifecycle states in declaration order; emit only those with on_state actions.
    for state in &schema.lifecycle.states {
        let Some(actions) = wf.on_state.get(state) else {
            continue;
        };
        if actions.is_empty() {
            continue;
        }
        let _ = writeln!(
            out,
            "{indent}\"z2_{store_name}__{state}\" [label=\"{state}\"];"
        );
        for (idx, action) in actions.iter().enumerate() {
            match action {
                StateAction::DispatchAgent(role) => {
                    let style =
                        actor_style(Some(Actor::AiAutonomous), opts.no_icons, color_disabled);
                    let role_node = format!("z2_{store_name}__{state}__role_{idx}_{role}");
                    let color_attr = if style.dot_color.is_empty() {
                        String::new()
                    } else {
                        format!(", color={}, fontcolor={}", style.dot_color, style.dot_color)
                    };
                    let _ = writeln!(
                        out,
                        "{indent}\"{role_node}\" [shape=ellipse, label=\"{role}\"];"
                    );
                    let _ = writeln!(
                        out,
                        "{indent}\"z2_{store_name}__{state}\" -> \"{role_node}\" [label=\"\u{2192} {role}\"{color_attr}];"
                    );
                }
                StateAction::TransitionTo(to_state) => {
                    let style = actor_style(Some(Actor::Framework), opts.no_icons, color_disabled);
                    let color_attr = if style.dot_color.is_empty() {
                        String::new()
                    } else {
                        format!(", color={}, fontcolor={}", style.dot_color, style.dot_color)
                    };
                    let _ = writeln!(
                        out,
                        "{indent}\"z2_{store_name}__{to_state}\" [label=\"{to_state}\"];"
                    );
                    let _ = writeln!(
                        out,
                        "{indent}\"z2_{store_name}__{state}\" -> \"z2_{store_name}__{to_state}\" [label=\"\u{21d2} auto\"{color_attr}];"
                    );
                }
                StateAction::Increment(_) => {
                    // Increments are bookkeeping; not graphed in firing-order view.
                }
            }
        }
    }
}

/// Emit a standalone `digraph` for one store's Z2 workflow firing order.
pub fn emit_zone_z2_dot(store_name: &str, schema: &Schema, opts: &Opts) -> String {
    let color_disabled = no_color_env();
    let mut out = String::new();
    let _ = writeln!(out, "digraph z2_{store_name} {{");
    out.push_str("  rankdir=TB;\n");
    write_z2_body(&mut out, store_name, schema, opts, color_disabled, "  ");
    out.push_str("}\n");
    out
}

/// Walk Z0, then per-store Z1, then per-workflow-store Z2, returning a
/// `(header, dot_source)` pair per zone. Headers match the wording used by
/// the mermaid emitter so multi-format output is consistent.
pub fn zones_for_auto(
    manifest: &Manifest,
    schemas: &HashMap<String, Schema>,
    opts: &Opts,
) -> Vec<(String, String)> {
    let mut zones: Vec<(String, String)> = Vec::new();
    zones.push((
        "Z0: cross-store soft-FKs".to_string(),
        emit_zone_z0_dot(manifest, schemas),
    ));
    for store in &manifest.stores {
        if let Some(filter) = &opts.store_filter {
            if &store.name != filter {
                continue;
            }
        }
        if let Some(schema) = schemas.get(&store.name) {
            zones.push((
                format!("Z1: {} state machine", store.name),
                emit_zone_z1_dot(&store.name, schema, opts),
            ));
        }
    }
    for store in &manifest.stores {
        if let Some(filter) = &opts.store_filter {
            if &store.name != filter {
                continue;
            }
        }
        if let Some(schema) = schemas.get(&store.name) {
            if schema.workflow.is_some() {
                zones.push((
                    format!("Z2: {} workflow firing order", store.name),
                    emit_zone_z2_dot(&store.name, schema, opts),
                ));
            }
        }
    }
    zones
}

// ---------------------------------------------------------------------------
// Mermaid emitter
// ---------------------------------------------------------------------------

/// Emit a markdown document with one `stateDiagram-v2` block per zone,
/// separated by `---` and `## Zk …` headings.  Mermaid does not support
/// per-edge color, so labels carry icon-only (or text-code-only) prefixes.
pub fn emit_mermaid(manifest: &Manifest, schemas: &HashMap<String, Schema>, opts: &Opts) -> String {
    let mut out = String::new();

    // Z0
    out.push_str("## Z0: cross-store soft-FKs\n\n");
    out.push_str("```mermaid\nstateDiagram-v2\n");
    for store in &manifest.stores {
        let _ = writeln!(out, "  state \"{}\" as {}", store.name, store.name);
    }
    for store in &manifest.stores {
        let Some(schema) = schemas.get(&store.name) else {
            continue;
        };
        for field in &schema.fields {
            if let FieldType::ListFk { ref_store } = &field.ty {
                if manifest.stores.iter().any(|s| &s.name == ref_store) {
                    let _ = writeln!(out, "  {} --> {} : {}", store.name, ref_store, field.name);
                }
            }
        }
    }
    out.push_str("```\n\n");

    // Z1 per store
    for store in &manifest.stores {
        if let Some(filter) = &opts.store_filter {
            if &store.name != filter {
                continue;
            }
        }
        let Some(schema) = schemas.get(&store.name) else {
            continue;
        };
        out.push_str("---\n\n");
        let _ = writeln!(out, "## Z1: {} state machine\n", store.name);
        out.push_str("```mermaid\nstateDiagram-v2\n");
        let initial = schema.lifecycle.resolved_initial_state().unwrap_or("");
        if !initial.is_empty() {
            let _ = writeln!(out, "  [*] --> {initial}");
        }
        for t in &schema.lifecycle.transitions {
            // Mermaid: no color; prefix-only.
            let style = actor_style(t.actor, opts.no_icons, true);
            let _ = writeln!(
                out,
                "  {} --> {} : {} {}",
                t.from, t.to, style.label_prefix, t.verb
            );
        }
        out.push_str("```\n\n");
    }

    // Z2 per workflow store
    for store in &manifest.stores {
        if let Some(filter) = &opts.store_filter {
            if &store.name != filter {
                continue;
            }
        }
        let Some(schema) = schemas.get(&store.name) else {
            continue;
        };
        let Some(wf) = schema.workflow.as_ref() else {
            continue;
        };
        out.push_str("---\n\n");
        let _ = writeln!(out, "## Z2: {} workflow firing order\n", store.name);
        out.push_str("```mermaid\nstateDiagram-v2\n");
        for state in &schema.lifecycle.states {
            let Some(actions) = wf.on_state.get(state) else {
                continue;
            };
            if actions.is_empty() {
                continue;
            }
            for (idx, action) in actions.iter().enumerate() {
                match action {
                    StateAction::DispatchAgent(role) => {
                        let style = actor_style(Some(Actor::AiAutonomous), opts.no_icons, true);
                        let _ = writeln!(out, "  state \"{role}\" as {state}_role_{idx}_{role}");
                        let _ = writeln!(
                            out,
                            "  {state} --> {state}_role_{idx}_{role} : {} \u{2192} {role}",
                            style.label_prefix
                        );
                    }
                    StateAction::TransitionTo(to_state) => {
                        let style = actor_style(Some(Actor::Framework), opts.no_icons, true);
                        let _ = writeln!(
                            out,
                            "  {state} --> {to_state} : {} \u{21d2} auto",
                            style.label_prefix
                        );
                    }
                    StateAction::Increment(_) => {}
                }
            }
        }
        out.push_str("```\n\n");
    }

    out
}

// ---------------------------------------------------------------------------
// Auto-format: shell out to `dot -Tutf8` with graceful fallback
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum FallbackReason {
    DotMissing,
    // The `String` payload is read by tests and available to future diagnostics;
    // `run()` currently ignores it via `reason: _`.
    #[allow(dead_code)]
    DotFailed(String),
}

#[derive(Debug)]
pub enum RenderOutcome {
    Rendered(String),
    Fallback {
        // Unused by `run()` after the per-zone rewrite (combined source is
        // regenerated via `emit_dot`); kept for the existing render_via_dot
        // unit tests that pattern-match on it.
        #[allow(dead_code)]
        source: String,
        // `run()` ignores `reason` (prints the fixed install hint); tests
        // pattern-match on it to verify the missing-vs-failed branch.
        #[allow(dead_code)]
        reason: FallbackReason,
    },
}

/// Result of one spawn-and-wait cycle of the `dot` tool.
#[derive(Debug)]
pub struct DotResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Spawner contract: take a dot source, return the spawn-and-wait result.
/// Production wires `real_dot_spawner`; tests pass a stub that returns
/// `ErrorKind::NotFound` to simulate a missing `dot` binary.
pub type DotSpawner = fn(&str) -> std::io::Result<DotResult>;

/// One-line note printed to stderr when graph-easy is not on PATH.
///
/// L036: graphviz has no `dot -Tutf8` format. The in-terminal ASCII
/// render is supplied by Perl's `graph-easy` (Debian/Ubuntu pkg
/// `libgraph-easy-perl`), which reads dot source and emits boxart.
pub const FALLBACK_NOTE_MISSING: &str =
    "note: 'graph-easy' not found on PATH \u{2014} install it for ASCII art \
     (e.g. apt install libgraph-easy-perl), or use --format mermaid / --format dot";

/// One-line note printed to stderr when graph-easy ran but exited non-zero.
pub const FALLBACK_NOTE_FAILED: &str =
    "note: 'graph-easy' ran but failed; falling back to dot source \u{2014} \
     try --format mermaid or report a bug";

/// Back-compat alias kept for any external readers; prefer the
/// reason-specific constants above.
#[deprecated(note = "use FALLBACK_NOTE_MISSING or FALLBACK_NOTE_FAILED")]
#[allow(dead_code)]
pub const FALLBACK_NOTE: &str = FALLBACK_NOTE_MISSING;

pub fn real_dot_spawner(dot_source: &str) -> std::io::Result<DotResult> {
    let mut child = Command::new("graph-easy")
        .args(["--as=boxart"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("failed to open dot stdin"))?;
        stdin.write_all(dot_source.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    Ok(DotResult {
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Pipe `dot_source` through `dot -Tutf8` (production) or a stubbed spawner
/// (tests).  Missing-binary or non-zero-exit conditions degrade into a
/// `Fallback` outcome carrying the original source — they are not errors,
/// so the function is infallible.
pub fn render_via_dot_with(spawner: DotSpawner, dot_source: &str) -> RenderOutcome {
    match spawner(dot_source) {
        Ok(r) if r.success => RenderOutcome::Rendered(r.stdout),
        Ok(r) => RenderOutcome::Fallback {
            source: dot_source.to_string(),
            reason: FallbackReason::DotFailed(r.stderr),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => RenderOutcome::Fallback {
            source: dot_source.to_string(),
            reason: FallbackReason::DotMissing,
        },
        Err(e) => RenderOutcome::Fallback {
            source: dot_source.to_string(),
            reason: FallbackReason::DotFailed(e.to_string()),
        },
    }
}

pub fn render_via_dot(dot_source: &str) -> RenderOutcome {
    render_via_dot_with(real_dot_spawner, dot_source)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Entry point for the `topology` subcommand.
///
/// `Format::Auto` shells out to `dot -Tutf8`; on missing or failing `dot` it
/// prints the dot source and a one-line install hint to stderr.
pub fn run(manifest: &Manifest, schemas: &HashMap<String, Schema>, opts: Opts) -> Result<()> {
    match opts.format {
        Format::Mermaid => {
            print!("{}", emit_mermaid(manifest, schemas, &opts));
        }
        Format::Dot => {
            print!("{}", emit_dot(manifest, schemas, &opts));
        }
        Format::Auto => {
            let zones = zones_for_auto(manifest, schemas, &opts);
            let mut rendered: Vec<(String, String)> = Vec::with_capacity(zones.len());
            let mut fallback_reason: Option<FallbackReason> = None;
            for (header, dot_source) in &zones {
                match render_via_dot(dot_source) {
                    RenderOutcome::Rendered(s) => rendered.push((header.clone(), s)),
                    RenderOutcome::Fallback { reason, .. } => {
                        fallback_reason = Some(reason);
                        break;
                    }
                }
            }
            if let Some(reason) = fallback_reason {
                let combined = emit_dot(manifest, schemas, &opts);
                print!("{combined}");
                match reason {
                    FallbackReason::DotMissing => eprintln!("{FALLBACK_NOTE_MISSING}"),
                    FallbackReason::DotFailed(_) => eprintln!("{FALLBACK_NOTE_FAILED}"),
                }
            } else {
                for (header, body) in rendered {
                    print!("## {header}\n\n{body}\n\n");
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_style_ai_autonomous_color_on_icons_on() {
        let s = actor_style(Some(Actor::AiAutonomous), false, false);
        assert_eq!(s.dot_color, "green");
        assert_eq!(s.text_code, "A");
        assert_eq!(s.icon, "\u{f544}");
        assert_eq!(s.label_prefix, format!("{} A", "\u{f544}"));
    }

    #[test]
    fn actor_style_ai_autonomous_color_off_icons_on() {
        let s = actor_style(Some(Actor::AiAutonomous), false, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.text_code, "A");
        assert_eq!(s.label_prefix, format!("{} A", "\u{f544}"));
    }

    #[test]
    fn actor_style_ai_autonomous_color_on_icons_off() {
        let s = actor_style(Some(Actor::AiAutonomous), true, false);
        assert_eq!(s.dot_color, "green");
        assert_eq!(s.label_prefix, "A");
    }

    #[test]
    fn actor_style_ai_autonomous_color_off_icons_off() {
        let s = actor_style(Some(Actor::AiAutonomous), true, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.label_prefix, "A");
    }

    #[test]
    fn actor_style_ai_with_human_all_modes() {
        let s = actor_style(Some(Actor::AiWithHuman), false, false);
        assert_eq!(s.dot_color, "gold");
        assert_eq!(s.text_code, "H+");
        assert_eq!(s.label_prefix, format!("{} H+", "\u{f2b5}"));

        let s = actor_style(Some(Actor::AiWithHuman), true, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.label_prefix, "H+");
    }

    #[test]
    fn actor_style_human_all_modes() {
        let s = actor_style(Some(Actor::Human), false, false);
        assert_eq!(s.dot_color, "red");
        assert_eq!(s.text_code, "H!");
        assert_eq!(s.label_prefix, format!("{} H!", "\u{f007}"));

        let s = actor_style(Some(Actor::Human), true, false);
        assert_eq!(s.dot_color, "red");
        assert_eq!(s.label_prefix, "H!");

        let s = actor_style(Some(Actor::Human), false, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.label_prefix, format!("{} H!", "\u{f007}"));
    }

    #[test]
    fn actor_style_framework_all_modes() {
        let s = actor_style(Some(Actor::Framework), false, false);
        assert_eq!(s.dot_color, "gray");
        assert_eq!(s.text_code, "F");
        assert_eq!(s.label_prefix, format!("{} F", "\u{f013}"));

        let s = actor_style(Some(Actor::Framework), true, true);
        assert_eq!(s.dot_color, "");
        assert_eq!(s.label_prefix, "F");
    }

    /// AC2.7: NO_COLOR=1 suppresses `color=` attributes on dot edges.
    #[test]
    fn ac2_7_no_color_env_suppresses_dot_color_attrs() {
        use crate::cli::dynamic::BUNDLED_STORE_SCHEMAS;
        use crate::cli::test_support::ENV_LOCK;
        use crate::manifest::{InstalledStore, Manifest};
        use crate::schema::StoreScope;
        use std::path::PathBuf;

        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Snapshot prior value, set NO_COLOR, restore after.
        let prior = std::env::var_os("NO_COLOR");
        unsafe {
            std::env::set_var("NO_COLOR", "1");
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
        let out = emit_dot(&manifest, &schemas, &opts);

        // Restore env BEFORE assertions so a panic doesn't leak NO_COLOR.
        match prior {
            Some(v) => unsafe {
                std::env::set_var("NO_COLOR", v);
            },
            None => unsafe {
                std::env::remove_var("NO_COLOR");
            },
        }

        assert!(
            !out.contains("color="),
            "NO_COLOR=1 must suppress all `color=` attributes; got: {out}"
        );
    }

    /// AC3.3: render_via_dot's fallback path engages when the spawner reports
    /// `ErrorKind::NotFound` (the simulated dot-missing case).
    #[test]
    fn render_via_dot_falls_back_when_missing() {
        fn missing_spawner(_src: &str) -> std::io::Result<DotResult> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        let outcome = render_via_dot_with(missing_spawner, "digraph X {}");
        match outcome {
            RenderOutcome::Fallback {
                source,
                reason: FallbackReason::DotMissing,
            } => {
                assert!(
                    source.starts_with("digraph"),
                    "fallback source must be raw dot; got: {source}"
                );
            }
            other => panic!("expected Fallback {{ DotMissing }}, got {other:?}"),
        }
    }

    /// AC3.2 surface check: the install-hint constant carries both pointers
    /// the user needs (apt package + mermaid alternative). L036 swapped
    /// the renderer from `dot -Tutf8` (no such format in graphviz) to
    /// `graph-easy --as=boxart` (Perl libgraph-easy-perl).
    #[test]
    fn fallback_note_mentions_install_and_mermaid_alternative() {
        assert!(FALLBACK_NOTE_MISSING.contains("libgraph-easy-perl"));
        assert!(FALLBACK_NOTE_MISSING.contains("--format mermaid"));
        assert!(FALLBACK_NOTE_FAILED.contains("--format mermaid"));
    }

    /// Non-zero exit from the spawner is also a fallback (not an error).
    #[test]
    fn render_via_dot_falls_back_on_nonzero_exit() {
        fn failing_spawner(_src: &str) -> std::io::Result<DotResult> {
            Ok(DotResult {
                success: false,
                stdout: String::new(),
                stderr: "syntax error near line 1".into(),
            })
        }
        let outcome = render_via_dot_with(failing_spawner, "digraph X {}");
        match outcome {
            RenderOutcome::Fallback {
                source,
                reason: FallbackReason::DotFailed(stderr),
            } => {
                assert!(source.starts_with("digraph"));
                assert!(stderr.contains("syntax error"));
            }
            other => panic!("expected Fallback {{ DotFailed }}, got {other:?}"),
        }
    }

    /// Successful render is forwarded verbatim.
    #[test]
    fn render_via_dot_returns_rendered_on_success() {
        fn ok_spawner(_src: &str) -> std::io::Result<DotResult> {
            Ok(DotResult {
                success: true,
                stdout: "rendered ascii here".into(),
                stderr: String::new(),
            })
        }
        let outcome = render_via_dot_with(ok_spawner, "digraph X {}");
        match outcome {
            RenderOutcome::Rendered(s) => assert_eq!(s, "rendered ascii here"),
            other => panic!("expected Rendered, got {other:?}"),
        }
    }

    #[test]
    fn actor_style_none_treated_as_framework() {
        let s = actor_style(None, false, false);
        assert_eq!(s.dot_color, "gray");
        assert_eq!(s.text_code, "F");
    }
}
