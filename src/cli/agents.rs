/// Bundled-agents registry and install/uninstall surface.
///
/// This module is a near-mechanical clone of `cli/skills.rs` with three
/// intentional differences:
///
/// 1. Registry: `BUNDLED_AGENTS` (5 entries) vs `BUNDLED_SKILLS` (5 entries).
/// 2. Target directory: `.claude/agents/<name>.md` (flat) vs
///    `.claude/skills/<name>/SKILL.md` (nested).
/// 3. File layout: agents are installed as `<name>.md` directly under the
///    agents directory, NOT in a `<name>/AGENT.md` subdirectory.
///
/// The asymmetry with skills' nested `<name>/SKILL.md` is **intentional and
/// platform-driven**: Claude Code's subagent loader scans flat
/// `<agents_dir>/<name>.md` — nesting would prevent registry registration.
use anyhow::{bail, Result};
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Bundled agents — embedded at compile time
// ---------------------------------------------------------------------------

pub static BUNDLED_AGENTS: &[(&str, &str)] = &[
    ("planner", include_str!("../../agents/planner.md")),
    (
        "plan-reviewer",
        include_str!("../../agents/plan-reviewer.md"),
    ),
    ("executor", include_str!("../../agents/executor.md")),
    (
        "code-reviewer",
        include_str!("../../agents/code-reviewer.md"),
    ),
    ("guide", include_str!("../../agents/guide.md")),
    ("wrap", include_str!("../../agents/wrap.md")),
];

/// Bundled JSON Schemas for agent envelopes — embedded at compile time.
///
/// Each entry is `(role_name, schema_json_text)` and mirrors `BUNDLED_AGENTS`
/// role-for-role. The schema text is the raw JSON Schema Draft 2020-12 document
/// (or Draft-07 if the SDK rejected 2020-12 during AC1.5 testing — see Phase 1
/// executor summary for the chosen dialect).
///
/// These schemas are threaded by drive into `runner.spawn(..., Some(schema))`
/// to request schema-validated structured output from the claude CLI via
/// `--json-schema`.
pub static BUNDLED_AGENT_SCHEMAS: &[(&str, &str)] = &[
    (
        "planner",
        include_str!("../../agents/schemas/planner.schema.json"),
    ),
    (
        "plan-reviewer",
        include_str!("../../agents/schemas/plan-reviewer.schema.json"),
    ),
    (
        "executor",
        include_str!("../../agents/schemas/executor.schema.json"),
    ),
    (
        "code-reviewer",
        include_str!("../../agents/schemas/code-reviewer.schema.json"),
    ),
    (
        "guide",
        include_str!("../../agents/schemas/guide.schema.json"),
    ),
    (
        "wrap",
        include_str!("../../agents/schemas/wrap.schema.json"),
    ),
];

// ---------------------------------------------------------------------------
// CLI args (parsed by caller from clap matches)
// ---------------------------------------------------------------------------

pub enum AgentsCmd {
    List,
    Install {
        name: Option<String>,
        all: bool,
        global: bool,
    },
    Uninstall {
        name: String,
        global: bool,
    },
}

// ---------------------------------------------------------------------------
// Resolve target directory
// ---------------------------------------------------------------------------

fn agents_dir(global: bool) -> Result<PathBuf> {
    if global {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("~"));
        Ok(home.join(".claude").join("agents"))
    } else {
        Ok(std::env::current_dir()?.join(".claude").join("agents"))
    }
}

/// Flat path: `<base>/<name>.md` (NOT nested like skills).
fn agent_path(base: &PathBuf, name: &str) -> PathBuf {
    base.join(format!("{name}.md"))
}

// ---------------------------------------------------------------------------
// Subcommand handlers
// ---------------------------------------------------------------------------

pub fn run(cmd: AgentsCmd) -> Result<()> {
    match cmd {
        AgentsCmd::List => list(),
        AgentsCmd::Install { name, all, global } => {
            if all {
                install_all(global)
            } else if let Some(n) = name {
                install_one(&n, global, false)
            } else {
                bail!("specify an agent name or --all");
            }
        }
        AgentsCmd::Uninstall { name, global } => uninstall_one(&name, global),
    }
}

fn list() -> Result<()> {
    let local_base = agents_dir(false)?;
    let global_base = agents_dir(true)?;

    println!("Available bundled agents:");
    for (name, _) in BUNDLED_AGENTS {
        let local_installed = agent_path(&local_base, name).exists();
        let global_installed = agent_path(&global_base, name).exists();
        let annotation = match (local_installed, global_installed) {
            (true, true) => "  (installed, installed --global)",
            (true, false) => "  (installed)",
            (false, true) => "  (installed --global)",
            (false, false) => "",
        };
        println!("  {name}{annotation}");
    }
    println!();
    println!("Install with: stores agents install <name> [--global]");
    Ok(())
}

fn install_one(name: &str, global: bool, silent_if_same: bool) -> Result<()> {
    let content = BUNDLED_AGENTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .ok_or_else(|| anyhow::anyhow!("unknown agent '{name}'; run `stores agents list`"))?;

    let base = agents_dir(global)?;
    let dest = agent_path(&base, name);

    if dest.exists() {
        let existing = std::fs::read_to_string(&dest)?;
        if existing == content {
            if !silent_if_same {
                println!("Already installed: {}", dest.display());
            }
            return Ok(());
        } else {
            bail!(
                "Agent exists with different content; remove or use --force: {}",
                dest.display()
            );
        }
    }

    // Create parent dir and write flat file
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, content)?;
    println!("Installed agent '{name}' to {}", dest.display());
    Ok(())
}

fn install_all(global: bool) -> Result<()> {
    for (name, _) in BUNDLED_AGENTS {
        install_one(name, global, true)?;
    }
    Ok(())
}

fn uninstall_one(name: &str, global: bool) -> Result<()> {
    // Validate agent name
    if !BUNDLED_AGENTS.iter().any(|(n, _)| *n == name) {
        bail!("unknown agent '{name}'; run `stores agents list`");
    }

    let base = agents_dir(global)?;
    let dest = agent_path(&base, name);

    if !dest.exists() {
        println!("Not installed: {name}");
        return Ok(());
    }

    std::fs::remove_file(&dest)?;
    // No subdirectory to clean up (flat layout — no parent dir to prune)
    println!("Uninstalled agent '{name}'");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_tmp_base() -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let base = std::env::temp_dir()
            .join(format!("stores-agents-test-{}-{}", std::process::id(), ns))
            .join(".claude")
            .join("agents");
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn write_agent_to(base: &PathBuf, name: &str, content: &str) {
        let p = agent_path(base, name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
    }

    // -----------------------------------------------------------------------
    // Helpers that operate on an explicit base dir (avoid env side-effects)
    // -----------------------------------------------------------------------

    fn install_to(name: &str, base: &PathBuf, silent: bool) -> Result<()> {
        let content = BUNDLED_AGENTS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, c)| *c)
            .ok_or_else(|| anyhow::anyhow!("unknown agent '{name}'"))?;

        let dest = agent_path(base, name);

        if dest.exists() {
            let existing = fs::read_to_string(&dest)?;
            if existing == content {
                if !silent {
                    println!("Already installed: {}", dest.display());
                }
                return Ok(());
            } else {
                bail!(
                    "Agent exists with different content; remove or use --force: {}",
                    dest.display()
                );
            }
        }

        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, content)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // AC1.2 / AC1.8: fresh install writes the file (flat layout)
    // -----------------------------------------------------------------------
    #[test]
    fn fresh_install_writes_file() {
        let base = make_tmp_base();
        let name = "planner";
        install_to(name, &base, false).unwrap();

        let dest = agent_path(&base, name);
        assert!(dest.exists(), "agent .md should exist after install");
        // Flat: no nested subdirectory
        assert_eq!(dest.file_name().unwrap(), format!("{name}.md").as_str());

        let on_disk = fs::read_to_string(&dest).unwrap();
        let bundled = BUNDLED_AGENTS.iter().find(|(n, _)| *n == name).unwrap().1;
        assert_eq!(on_disk, bundled, "content must match bundled");
    }

    // -----------------------------------------------------------------------
    // AC1.2: idempotent re-install (same content → no error)
    // -----------------------------------------------------------------------
    #[test]
    fn idempotent_reinstall_ok() {
        let base = make_tmp_base();
        let name = "executor";
        install_to(name, &base, false).unwrap();
        // second call must succeed (same content)
        install_to(name, &base, false).unwrap();
    }

    // -----------------------------------------------------------------------
    // AC1.5: conflict on different content → error
    // -----------------------------------------------------------------------
    #[test]
    fn conflict_different_content_errors() {
        let base = make_tmp_base();
        let name = "code-reviewer";
        // Write a file with different content first
        write_agent_to(&base, name, "# different content\n");
        let err = install_to(name, &base, false).unwrap_err();
        assert!(
            err.to_string().contains("different content"),
            "expected conflict error, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // AC1.4: uninstall removes the flat file
    // -----------------------------------------------------------------------
    #[test]
    fn uninstall_removes_file() {
        let base = make_tmp_base();
        let name = "guide";
        install_to(name, &base, false).unwrap();

        let dest = agent_path(&base, name);
        assert!(dest.exists());

        fs::remove_file(&dest).unwrap();
        assert!(!dest.exists());
        // No empty-dir cleanup needed for flat layout
    }

    // -----------------------------------------------------------------------
    // AC1.8: BUNDLED_AGENTS must contain exactly 6 entries (includes wrap; T010 Phase 1)
    // -----------------------------------------------------------------------
    #[test]
    fn all_agents_bundled() {
        let names: Vec<&str> = BUNDLED_AGENTS.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"planner"));
        assert!(names.contains(&"plan-reviewer"));
        assert!(names.contains(&"executor"));
        assert!(names.contains(&"code-reviewer"));
        assert!(names.contains(&"guide"));
        assert!(names.contains(&"wrap"));
        assert_eq!(names.len(), 6, "BUNDLED_AGENTS must contain exactly 6 entries");
    }

    // -----------------------------------------------------------------------
    // AC1.4: BUNDLED_AGENT_SCHEMAS has 6 entries; role-name set matches
    // BUNDLED_AGENTS. (wrap schema added in T010 Phase 1 stub; formalized Phase 2)
    // -----------------------------------------------------------------------
    #[test]
    fn bundled_schemas_count_matches_agents() {
        let agent_names: std::collections::BTreeSet<&str> =
            BUNDLED_AGENTS.iter().map(|(n, _)| *n).collect();
        let schema_names: std::collections::BTreeSet<&str> =
            BUNDLED_AGENT_SCHEMAS.iter().map(|(n, _)| *n).collect();

        assert_eq!(
            BUNDLED_AGENT_SCHEMAS.len(),
            6,
            "BUNDLED_AGENT_SCHEMAS must contain exactly 6 entries"
        );
        assert_eq!(
            agent_names, schema_names,
            "BUNDLED_AGENT_SCHEMAS role names must match BUNDLED_AGENTS role names"
        );

        // Each schema must be valid JSON.
        for (name, text) in BUNDLED_AGENT_SCHEMAS {
            let parsed: serde_json::Result<serde_json::Value> = serde_json::from_str(text);
            assert!(
                parsed.is_ok(),
                "schema for '{name}' is not valid JSON: {}",
                parsed.err().unwrap()
            );
        }
    }

    // -----------------------------------------------------------------------
    // AC1.9: flat layout — agent_path returns <base>/<name>.md (no subdir)
    // -----------------------------------------------------------------------
    #[test]
    fn flat_layout_not_nested() {
        let base = PathBuf::from("/tmp/test-agents");
        for (name, _) in BUNDLED_AGENTS {
            let p = agent_path(&base, name);
            // Must be exactly 2 components deep: base + <name>.md
            assert_eq!(
                p.parent().unwrap(),
                base.as_path(),
                "agent '{name}' path must be flat (no subdirectory)"
            );
            assert_eq!(
                p.file_name().unwrap().to_str().unwrap(),
                format!("{name}.md"),
                "agent '{name}' filename must be <name>.md"
            );
        }
    }
}
