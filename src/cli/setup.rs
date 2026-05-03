/// `stores setup` — single-command bootstrap.
///
/// Composes in order:
///   1. `cli::init::run()` — creates `.stores/db.sqlite` + `.stores/manifest.yaml`
///   2. For each bundled store: `install::run(<name>)` — idempotent (already-installed skipped)
///   3. `cli::skills::run(SkillsCmd::Install { all: true, ... })` — installs all 5 bundled skills
///   4. `cli::agents::run(AgentsCmd::Install { all: true, ... })` — installs all 5 bundled agents
///
/// A failure in any layer aborts subsequent layers and propagates the error (AC4.6).
use anyhow::Result;

use crate::cli::agents::{run as agents_run, AgentsCmd};
use crate::cli::dynamic::BUNDLED_STORE_NAMES;
use crate::cli::init;
use crate::cli::skills::{run as skills_run, SkillsCmd};
use crate::install;

pub fn run(global: bool) -> Result<()> {
    // -----------------------------------------------------------------------
    // Layer 1: init
    // -----------------------------------------------------------------------
    println!("=== init ===");
    init::run()?;

    // -----------------------------------------------------------------------
    // Layer 2: bundled stores
    // -----------------------------------------------------------------------
    println!("=== stores ===");
    for &name in BUNDLED_STORE_NAMES {
        let result = install::run(std::path::Path::new(name));
        match result {
            Ok(()) => {}
            Err(e) => {
                let msg = e.to_string();
                // Treat "already installed from this path" as idempotent success (AC4.2).
                // install_bundled bails with a message containing "already installed".
                if msg.contains("already installed") {
                    println!("Already installed: {name}");
                } else {
                    return Err(e);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Layer 3: skills
    // -----------------------------------------------------------------------
    println!("=== skills ===");
    skills_run(SkillsCmd::Install {
        name: None,
        all: true,
        global,
    })?;

    // -----------------------------------------------------------------------
    // Layer 4: agents
    // -----------------------------------------------------------------------
    println!("=== agents ===");
    agents_run(AgentsCmd::Install {
        name: None,
        all: true,
        global,
    })?;

    println!("Setup complete.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    /// Create an isolated tempdir and set CWD + HOME for a single test closure.
    ///
    /// Skills and agents use `std::env::current_dir()` for local installs, so we
    /// need CWD pointing into the tempdir.  Setting HOME prevents writes to the
    /// real ~/.claude/.
    ///
    /// NOTE: `std::env::set_current_dir` is process-global, so tests that call
    /// this helper are NOT safe to run in parallel.  They are tagged
    /// `#[serial_test::serial]` — but that crate is not yet in Cargo.toml.
    /// Instead we use a mutex to serialise the two tests in this module.
    fn with_isolated_env<F>(f: F)
    where
        F: FnOnce(&PathBuf),
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        let ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let tmp =
            std::env::temp_dir().join(format!("stores-setup-test-{}-{}", std::process::id(), ns));
        fs::create_dir_all(&tmp).unwrap();

        // Save original state
        let orig_cwd = std::env::current_dir().unwrap();
        let orig_home = std::env::var("HOME").ok();

        // Switch CWD + HOME into the tempdir so local (non-global) installs land there
        std::env::set_current_dir(&tmp).unwrap();
        std::env::set_var("HOME", &tmp);

        f(&tmp);

        // Restore
        std::env::set_current_dir(&orig_cwd).unwrap();
        match orig_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    // Use the process-wide CWD lock from paths.rs so setup tests serialise with
    // all other tests that call set_current_dir.
    fn cwd_lock() -> &'static std::sync::Mutex<()> {
        crate::paths::test_cwd_lock()
    }

    // -----------------------------------------------------------------------
    // AC4.1 + AC4.5: fresh bootstrap creates all expected artifacts
    // -----------------------------------------------------------------------
    #[test]
    fn fresh_bootstrap_creates_all_artifacts() {
        let _guard = cwd_lock().lock().unwrap_or_else(|e| e.into_inner());
        with_isolated_env(|tmp| {
            // Run setup (non-global so .claude/ lands in CWD == tmp)
            run(false).expect("setup should succeed on fresh dir");

            // .stores/db.sqlite
            assert!(
                tmp.join(".stores").join("db.sqlite").exists(),
                ".stores/db.sqlite must exist"
            );
            // .stores/manifest.yaml
            assert!(
                tmp.join(".stores").join("manifest.yaml").exists(),
                ".stores/manifest.yaml must exist"
            );

            // Three bundled stores in manifest
            let manifest_text =
                fs::read_to_string(tmp.join(".stores").join("manifest.yaml")).unwrap();
            for store_name in crate::cli::dynamic::BUNDLED_STORE_NAMES {
                assert!(
                    manifest_text.contains(store_name),
                    "manifest must mention store '{store_name}'"
                );
            }

            // 5 skill files under .claude/skills/
            use crate::cli::skills::BUNDLED_SKILLS;
            for (name, _) in BUNDLED_SKILLS {
                let p = tmp
                    .join(".claude")
                    .join("skills")
                    .join(name)
                    .join("SKILL.md");
                assert!(
                    p.exists(),
                    "skill '{name}' should be installed at {}",
                    p.display()
                );
            }

            // 5 agent files under .claude/agents/
            use crate::cli::agents::BUNDLED_AGENTS;
            for (name, _) in BUNDLED_AGENTS {
                let p = tmp
                    .join(".claude")
                    .join("agents")
                    .join(format!("{name}.md"));
                assert!(
                    p.exists(),
                    "agent '{name}' should be installed at {}",
                    p.display()
                );
            }
        });
    }

    // -----------------------------------------------------------------------
    // AC4.2 + AC4.5: idempotent re-run exits 0 and does not error
    // -----------------------------------------------------------------------
    #[test]
    fn idempotent_rerun_succeeds() {
        let _guard = cwd_lock().lock().unwrap_or_else(|e| e.into_inner());
        with_isolated_env(|_tmp| {
            run(false).expect("first setup should succeed");
            run(false).expect("second setup must also succeed (idempotent)");
        });
    }
}
