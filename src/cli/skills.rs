use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Bundled skills — embedded at compile time (option a)
// ---------------------------------------------------------------------------

pub static BUNDLED_SKILLS: &[(&str, &str)] = &[
    (
        "observation:log",
        include_str!("../../skills/observation:log/SKILL.md"),
    ),
    (
        "observation:triage",
        include_str!("../../skills/observation:triage/SKILL.md"),
    ),
    ("gate:walk", include_str!("../../skills/gate:walk/SKILL.md")),
    ("task:next", include_str!("../../skills/task:next/SKILL.md")),
    (
        "tasks:start",
        include_str!("../../skills/tasks:start/SKILL.md"),
    ),
];

// ---------------------------------------------------------------------------
// CLI args (parsed by caller from clap matches)
// ---------------------------------------------------------------------------

pub enum SkillsCmd {
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

fn skills_dir(global: bool) -> Result<PathBuf> {
    if global {
        let home = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("~"));
        Ok(home.join(".claude").join("skills"))
    } else {
        Ok(std::env::current_dir()?.join(".claude").join("skills"))
    }
}

fn skill_path(base: &Path, name: &str) -> PathBuf {
    base.join(name).join("SKILL.md")
}

// ---------------------------------------------------------------------------
// Subcommand handlers
// ---------------------------------------------------------------------------

pub fn run(cmd: SkillsCmd) -> Result<()> {
    match cmd {
        SkillsCmd::List => list(),
        SkillsCmd::Install { name, all, global } => {
            if all {
                install_all(global)
            } else if let Some(n) = name {
                install_one(&n, global, false)
            } else {
                bail!("specify a skill name or --all");
            }
        }
        SkillsCmd::Uninstall { name, global } => uninstall_one(&name, global),
    }
}

fn list() -> Result<()> {
    let local_base = skills_dir(false)?;
    let global_base = skills_dir(true)?;

    println!("Available bundled skills:");
    for (name, _) in BUNDLED_SKILLS {
        let local_installed = skill_path(&local_base, name).exists();
        let global_installed = skill_path(&global_base, name).exists();
        let annotation = match (local_installed, global_installed) {
            (true, true) => "  (installed, installed --global)",
            (true, false) => "  (installed)",
            (false, true) => "  (installed --global)",
            (false, false) => "",
        };
        println!("  {name}{annotation}");
    }
    println!();
    println!("Install with: stores skills install <name> [--global]");
    Ok(())
}

fn install_one(name: &str, global: bool, silent_if_same: bool) -> Result<()> {
    let content = BUNDLED_SKILLS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .ok_or_else(|| anyhow::anyhow!("unknown skill '{name}'; run `stores skills list`"))?;

    let base = skills_dir(global)?;
    let dest = skill_path(&base, name);

    if dest.exists() {
        let existing = std::fs::read_to_string(&dest)?;
        if existing == content {
            if !silent_if_same {
                println!("Already installed: {}", dest.display());
            }
            return Ok(());
        } else {
            bail!(
                "Skill exists with different content; remove or use --force: {}",
                dest.display()
            );
        }
    }

    // Create parent dir and write
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&dest, content)?;
    println!("Installed skill '{name}' to {}", dest.display());
    Ok(())
}

fn install_all(global: bool) -> Result<()> {
    for (name, _) in BUNDLED_SKILLS {
        install_one(name, global, true)?;
    }
    Ok(())
}

fn uninstall_one(name: &str, global: bool) -> Result<()> {
    // Validate skill name
    if !BUNDLED_SKILLS.iter().any(|(n, _)| *n == name) {
        bail!("unknown skill '{name}'; run `stores skills list`");
    }

    let base = skills_dir(global)?;
    let dest = skill_path(&base, name);

    if !dest.exists() {
        println!("Not installed: {name}");
        return Ok(());
    }

    std::fs::remove_file(&dest)?;

    // Remove parent dir if now empty
    if let Some(parent) = dest.parent() {
        if parent.exists() {
            let empty = std::fs::read_dir(parent)?.next().is_none();
            if empty {
                std::fs::remove_dir(parent)?;
            }
        }
    }

    println!("Uninstalled skill '{name}'");
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
            .join(format!("stores-skills-test-{}-{}", std::process::id(), ns))
            .join(".claude")
            .join("skills");
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn write_skill_to(base: &Path, name: &str, content: &str) {
        let p = skill_path(base, name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
    }

    // -----------------------------------------------------------------------
    // Helpers that operate on an explicit base dir (avoid env side-effects)
    // -----------------------------------------------------------------------

    fn install_to(name: &str, base: &Path, silent: bool) -> Result<()> {
        let content = BUNDLED_SKILLS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, c)| *c)
            .ok_or_else(|| anyhow::anyhow!("unknown skill '{name}'"))?;

        let dest = skill_path(base, name);

        if dest.exists() {
            let existing = fs::read_to_string(&dest)?;
            if existing == content {
                if !silent {
                    println!("Already installed: {}", dest.display());
                }
                return Ok(());
            } else {
                bail!(
                    "Skill exists with different content; remove or use --force: {}",
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
    // AC9-a: fresh install writes the file
    // -----------------------------------------------------------------------
    #[test]
    fn fresh_install_writes_file() {
        let base = make_tmp_base();
        let name = "observation:log";
        install_to(name, &base, false).unwrap();

        let dest = skill_path(&base, name);
        assert!(dest.exists(), "SKILL.md should exist after install");

        let on_disk = fs::read_to_string(&dest).unwrap();
        let bundled = BUNDLED_SKILLS.iter().find(|(n, _)| *n == name).unwrap().1;
        assert_eq!(on_disk, bundled, "content must match bundled");
    }

    // -----------------------------------------------------------------------
    // AC9-b: idempotent re-install (same content → no error)
    // -----------------------------------------------------------------------
    #[test]
    fn idempotent_reinstall_ok() {
        let base = make_tmp_base();
        let name = "gate:walk";
        install_to(name, &base, false).unwrap();
        // second call must succeed (same content)
        install_to(name, &base, false).unwrap();
    }

    // -----------------------------------------------------------------------
    // AC9-c: conflict on different content → error
    // -----------------------------------------------------------------------
    #[test]
    fn conflict_different_content_errors() {
        let base = make_tmp_base();
        let name = "task:next";
        // Write a file with different content first
        write_skill_to(&base, name, "# different content\n");
        let err = install_to(name, &base, false).unwrap_err();
        assert!(
            err.to_string().contains("different content"),
            "expected conflict error, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // Extra: uninstall removes file + empty dir
    // -----------------------------------------------------------------------
    #[test]
    fn uninstall_removes_file_and_dir() {
        let base = make_tmp_base();
        let name = "observation:triage";
        install_to(name, &base, false).unwrap();

        let dest = skill_path(&base, name);
        assert!(dest.exists());

        // Replicate uninstall logic directly
        fs::remove_file(&dest).unwrap();
        let parent = dest.parent().unwrap();
        let empty = fs::read_dir(parent).unwrap().next().is_none();
        if empty {
            fs::remove_dir(parent).unwrap();
        }

        assert!(!dest.exists());
        assert!(!parent.exists());
    }

    // -----------------------------------------------------------------------
    // BUNDLED_SKILLS must contain all expected names
    // -----------------------------------------------------------------------
    #[test]
    fn all_skills_bundled() {
        let names: Vec<&str> = BUNDLED_SKILLS.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"observation:log"));
        assert!(names.contains(&"observation:triage"));
        assert!(names.contains(&"gate:walk"));
        assert!(names.contains(&"task:next"));
        assert!(names.contains(&"tasks:start"));
        assert_eq!(names.len(), 5);
    }

    // -----------------------------------------------------------------------
    // tasks:start install writes byte-identical content (AC8.1)
    // -----------------------------------------------------------------------
    #[test]
    fn tasks_start_install_byte_identical() {
        let base = make_tmp_base();
        let name = "tasks:start";
        install_to(name, &base, false).unwrap();

        let dest = skill_path(&base, name);
        assert!(dest.exists(), "SKILL.md should exist after install");

        let on_disk = fs::read_to_string(&dest).unwrap();
        let bundled = BUNDLED_SKILLS.iter().find(|(n, _)| *n == name).unwrap().1;
        assert_eq!(
            on_disk, bundled,
            "installed content must be byte-identical to bundled"
        );
    }
}
