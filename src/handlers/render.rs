/// `render` handler — read-only workflow verb.
///
/// Reads a row from the DB, builds the render context, renders the workflow's
/// `render_template` via Handlebars, and writes `main.md` to the computed
/// target path (atomic write: `.tmp` → rename).
///
/// AC6.1: writes tasks/active/T003-<slug>/main.md for executing status.
/// AC6.2: --dry-run prints to stdout, writes nothing.
/// AC6.3: directory move when status_dir changes.
/// AC6.4: re-running produces byte-identical content (idempotent).
/// AC6.5: blocked status renders blocked_reason section.
/// AC6.6: does NOT modify any DB row (read-only).
use anyhow::{bail, Result};
use clap::ArgMatches;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::manifest::Manifest;
use crate::render::{build_context, render_template};
use crate::render::path::{find_existing_task_dir, maybe_move_dir, resolve_render_path};
use crate::schema::{actor::Actor, Schema};

use super::row::read_row;

/// Structured output from `compute_render`.  Holds all information needed to
/// perform the on-disk write without any further DB access.
#[derive(Debug, Serialize, Deserialize)]
pub struct RenderOutput {
    /// Absolute path where main.md should be written.
    pub path: PathBuf,
    /// Rendered content (Handlebars output).
    pub content: String,
    /// True when a directory was (or would be, in dry-run) moved.
    pub was_directory_move: bool,
    /// Dry-run flag — set true to skip the write.
    pub dry_run: bool,
}

/// Pure logic: resolve path, render content, determine if dir-move is needed.
/// Does NOT write to disk or DB.
///
/// `repo_root`: the root directory for task path resolution (typically cwd).
/// `manifest_root`: directory containing `.stores/` (also typically cwd).
pub(crate) fn compute_render_in(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    dry_run: bool,
    _invoker: Actor,
    repo_root: &Path,
    manifest_root: &Path,
) -> Result<RenderOutput> {
    // Must have a workflow with a render template.
    let workflow = match &schema.workflow {
        Some(wf) => wf,
        None => bail!(
            "store '{}' has no workflow declaration; render only works on workflow-shaped stores",
            schema.name
        ),
    };

    let render_tpl_path = match &workflow.render_template {
        Some(p) => p,
        None => bail!(
            "store '{}' workflow has no render_template; cannot render main.md",
            schema.name
        ),
    };

    let render_target_tpl = match &workflow.render_target_path {
        Some(t) => t.clone(),
        None => bail!(
            "store '{}' workflow has no render_target_path; cannot determine output path",
            schema.name
        ),
    };

    // Read the row (read-only — no DB mutations).
    let (_id, entry) = read_row(schema, conn, display_id)?;

    // Build render context.
    let ctx = build_context(schema, &entry);

    // Resolve the render target path.
    let target_path = resolve_render_path(&render_target_tpl, &ctx, repo_root)?;

    // Locate any existing on-disk task directory for this display_id.
    let existing_dir = find_existing_task_dir(repo_root, display_id);

    // Determine if a directory move is needed.
    let target_dir = target_path.parent().map(PathBuf::from);
    let was_directory_move =
        if let (Some(existing), Some(target_d)) = (&existing_dir, &target_dir) {
            existing != target_d && existing.exists()
        } else {
            false
        };

    // Load the render template.
    // P6-m2: detect "bundled:<name>" sentinel in schema_path; if present, look up
    // the template from BUNDLED_STORE_TEMPLATES instead of reading from disk.
    let manifest = Manifest::load_from(manifest_root)?;
    let store_entry = manifest.stores.iter().find(|s| s.name == schema.name);

    let template_text = if let Some(entry) = store_entry {
        let schema_path_str = entry.schema_path.to_string_lossy();
        if let Some(bundled_name) = schema_path_str.strip_prefix("bundled:") {
            let tpl_key = render_tpl_path.to_string_lossy();
            crate::cli::dynamic::BUNDLED_STORE_TEMPLATES
                .iter()
                .find(|(name, _)| *name == bundled_name)
                .and_then(|(_, templates)| {
                    templates.iter().find(|(path, _)| *path == tpl_key.as_ref()).map(|(_, content)| content.to_string())
                })
                .ok_or_else(|| anyhow::anyhow!(
                    "bundled store '{}': no render template '{}' in BUNDLED_STORE_TEMPLATES",
                    bundled_name,
                    tpl_key
                ))?
        } else {
            let full_path = entry.schema_path.join(render_tpl_path);
            std::fs::read_to_string(&full_path).map_err(|e| {
                anyhow::anyhow!(
                    "cannot read render template '{}': {}",
                    full_path.display(),
                    e
                )
            })?
        }
    } else {
        bail!(
            "store '{}' not found in manifest; cannot locate render template",
            schema.name
        )
    };

    // Render the template.
    let content = render_template(&template_text, &ctx)
        .map_err(|e| anyhow::anyhow!("template render failed for '{}': {}", display_id, e))?;

    Ok(RenderOutput {
        path: target_path,
        content,
        was_directory_move,
        dry_run,
    })
}

/// Pure logic using cwd for both repo_root and manifest_root.
pub fn compute_render(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    dry_run: bool,
    invoker: Actor,
) -> Result<RenderOutput> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    compute_render_in(schema, conn, display_id, dry_run, invoker, &cwd, &cwd)
}

/// Perform the on-disk write: directory move (if needed) + atomic write.
/// Uses explicit root paths — avoids cwd dependency in the write path.
pub(crate) fn run_render_in(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    dry_run: bool,
    invoker: Actor,
    repo_root: &Path,
    manifest_root: &Path,
) -> Result<()> {
    let output = compute_render_in(
        schema, conn, display_id, dry_run, invoker, repo_root, manifest_root,
    )?;

    if output.dry_run {
        print!("{}", output.content);
        return Ok(());
    }

    // Directory move (if status changed).
    if output.was_directory_move {
        let existing_dir = find_existing_task_dir(repo_root, display_id);
        let target_dir = output.path.parent().map(PathBuf::from);

        if let (Some(existing), Some(target_d)) = (existing_dir, target_dir) {
            if existing != target_d {
                if let Err(e) = maybe_move_dir(&existing, &target_d) {
                    eprintln!(
                        "warning: directory move failed ({}); writing to canonical path",
                        e
                    );
                }
            }
        }
    }

    // Ensure parent directory exists.
    if let Some(parent) = output.path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!("cannot create directory '{}': {}", parent.display(), e)
        })?;
    }

    // Atomic write: .tmp then rename.
    let tmp_path = output.path.with_extension("md.tmp");
    std::fs::write(&tmp_path, &output.content)
        .map_err(|e| anyhow::anyhow!("cannot write '{}': {}", tmp_path.display(), e))?;
    std::fs::rename(&tmp_path, &output.path)
        .map_err(|e| anyhow::anyhow!("cannot rename '{}': {}", tmp_path.display(), e))?;

    eprintln!("rendered: {}", output.path.display());
    Ok(())
}

/// Perform the on-disk write using cwd for both roots.
pub fn run_render(
    schema: &Schema,
    conn: &Connection,
    display_id: &str,
    dry_run: bool,
    invoker: Actor,
) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    run_render_in(schema, conn, display_id, dry_run, invoker, &cwd, &cwd)
}

/// Entry point called from dispatch.rs.
pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    invoker: Actor,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let dry_run = matches.get_flag("dry-run");

    run_render(schema, conn, display_id, dry_run, invoker)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::schema::Schema;
    use serde_json::json;
    use tempfile::tempdir;

    fn fixture_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/workflow_minimal")
    }

    fn fixture_schema() -> Schema {
        let yaml = std::fs::read_to_string(fixture_dir().join("schema.yaml")).unwrap();
        Schema::from_yaml(&yaml).unwrap()
    }

    /// Set up a temp dir with:
    ///   .stores/manifest.yaml → pointing to the workflow_minimal fixture
    ///   .stores/db.sqlite → with the fixture schema tables
    /// Returns (tempdir, conn, schema).
    fn setup_fixture_env(
        status: &str,
        slug: &str,
        display_id: &str,
    ) -> (tempfile::TempDir, rusqlite::Connection, Schema) {
        let tmp = tempdir().unwrap();
        let tmp_path = tmp.path();

        let stores_dir = tmp_path.join(".stores");
        std::fs::create_dir_all(&stores_dir).unwrap();

        let fixture = fixture_dir();
        let manifest_content = format!(
            "stores:\n  - name: wf_tasks\n    schema_path: {}\n    installed_at: 2026-01-01T00:00:00Z\n    table_name: wf_tasks\n    scope: worktree\n",
            fixture.display()
        );
        std::fs::write(stores_dir.join("manifest.yaml"), &manifest_content).unwrap();

        let db_file = stores_dir.join("db.sqlite");
        let conn = db::open(&db_file).unwrap();
        let schema = fixture_schema();
        let ddl = crate::codegen::ddl::ddl_for(&schema);
        conn.execute_batch(&ddl).unwrap();

        conn.execute(
            "INSERT INTO wf_tasks (display_id, status, created_at, updated_at, \
             created_by, updated_by, title, slug, description, current_phase, \
             current_cycle, blocked_reason, plan, cycles, plan_review_log) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            rusqlite::params![
                display_id, status, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z",
                "human", "human", "Test Task", slug,
                None::<String>, 1i64, 1i64, None::<String>,
                r#"{"objective":"","phases":[]}"#,
                "[]",
                "[]",
            ],
        ).unwrap();

        (tmp, conn, schema)
    }

    // AC6.1: compute_render returns content for an executing row.
    #[test]
    fn compute_render_returns_content_for_executing_row() {
        let (tmp, conn, schema) = setup_fixture_env("executing", "my-task", "WF001");
        let tmp_path = tmp.path().to_path_buf();

        let output = compute_render_in(
            &schema, &conn, "WF001", false, Actor::Human, &tmp_path, &tmp_path,
        ).unwrap();

        assert!(!output.content.is_empty(), "rendered content should not be empty");
        assert!(
            output.path.to_string_lossy().contains("tasks/active/WF001-my-task/main.md"),
            "path should contain tasks/active/WF001-my-task/main.md: {}",
            output.path.display()
        );
    }

    // AC6.2: dry-run uses compute_render (no write), captured output is same as content.
    #[test]
    fn compute_render_path_uses_status_dir_for_executing() {
        use crate::render::path::{resolve_render_path, status_to_dir};

        let ctx = json!({
            "display_id": "WF001",
            "slug": "my-task",
            "status": "executing",
        });

        let tpl = "tasks/{{status_dir}}/{{display_id}}-{{slug}}/main.md";
        let root = Path::new("/repo");
        let path = resolve_render_path(tpl, &ctx, root).unwrap();

        assert_eq!(
            path,
            PathBuf::from("/repo/tasks/active/WF001-my-task/main.md"),
            "executing status should produce active/ subdir"
        );
        assert_eq!(status_to_dir("executing"), "active");
    }

    // AC6.3: run_render_in creates file via atomic write.
    #[test]
    fn run_render_atomic_write_creates_file() {
        let (tmp, conn, schema) = setup_fixture_env("executing", "render-test", "WF001");
        let tmp_path = tmp.path().to_path_buf();

        run_render_in(&schema, &conn, "WF001", false, Actor::Human, &tmp_path, &tmp_path)
            .unwrap();

        let expected_path = tmp_path.join("tasks/active/WF001-render-test/main.md");
        assert!(expected_path.exists(), "main.md should exist at {}", expected_path.display());

        let content = std::fs::read_to_string(&expected_path).unwrap();
        assert!(!content.is_empty(), "rendered content should not be empty");
    }

    // AC6.3: directory is moved when status changes from active to completed.
    #[test]
    fn run_render_moves_directory_on_status_change() {
        let (tmp, conn, schema) = setup_fixture_env("complete", "dir-move-task", "WF001");
        let tmp_path = tmp.path().to_path_buf();

        // Create the directory in the "active" location as it would be before completion.
        let old_dir = tmp_path.join("tasks/active/WF001-dir-move-task");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("notes.md"), "some notes").unwrap();

        run_render_in(&schema, &conn, "WF001", false, Actor::Human, &tmp_path, &tmp_path)
            .unwrap();

        // The directory should have moved to completed/.
        let new_dir = tmp_path.join("tasks/completed/WF001-dir-move-task");
        assert!(new_dir.exists(), "directory should be at completed/: {}", new_dir.display());
        assert!(
            !old_dir.exists(),
            "old active/ directory should no longer exist: {}",
            old_dir.display()
        );
        // main.md should be in the new location.
        let main_md = new_dir.join("main.md");
        assert!(main_md.exists(), "main.md should exist in completed dir: {}", main_md.display());
    }

    // AC6.4: idempotency — two renders produce byte-identical content.
    #[test]
    fn run_render_idempotent_content() {
        let (tmp, conn, schema) = setup_fixture_env("executing", "idem-test", "WF001");
        let tmp_path = tmp.path().to_path_buf();

        run_render_in(&schema, &conn, "WF001", false, Actor::Human, &tmp_path, &tmp_path)
            .unwrap();
        let expected_path = tmp_path.join("tasks/active/WF001-idem-test/main.md");
        let content1 = std::fs::read_to_string(&expected_path).unwrap();

        run_render_in(&schema, &conn, "WF001", false, Actor::Human, &tmp_path, &tmp_path)
            .unwrap();
        let content2 = std::fs::read_to_string(&expected_path).unwrap();

        assert_eq!(
            content1, content2,
            "two renders with unchanged DB should produce byte-identical content"
        );
    }

    // AC6.5: blocked_reason appears in render context.
    #[test]
    fn compute_render_blocked_reason_in_context() {
        use crate::render::build_context;
        use crate::handlers::row::read_row;

        let (tmp, conn, schema) = setup_fixture_env("blocked", "blocked-task", "WF001");

        // Update the blocked_reason
        conn.execute(
            "UPDATE wf_tasks SET blocked_reason = ?1 WHERE display_id = ?2",
            rusqlite::params!["Waiting for human input on scope", "WF001"],
        ).unwrap();

        let tmp_path = tmp.path().to_path_buf();
        let (_id, entry) = read_row(&schema, &conn, "WF001").unwrap();
        let ctx = build_context(&schema, &entry);

        assert_eq!(
            ctx["blocked_reason"],
            json!("Waiting for human input on scope"),
            "blocked_reason should be in render context"
        );
        assert_eq!(ctx["status"], json!("blocked"));

        // Full render should also succeed.
        let output = compute_render_in(
            &schema, &conn, "WF001", false, Actor::Human, &tmp_path, &tmp_path,
        ).unwrap();
        assert!(
            output.path.to_string_lossy().contains("tasks/paused/"),
            "blocked status should route to paused/: {}",
            output.path.display()
        );
    }

    // AC6.6: render does not modify any DB row.
    #[test]
    fn render_is_read_only_against_db() {
        use crate::handlers::row::read_row;

        let (tmp, conn, schema) = setup_fixture_env("executing", "ro-test", "WF001");
        let tmp_path = tmp.path().to_path_buf();

        let (_, entry_before) = read_row(&schema, &conn, "WF001").unwrap();

        run_render_in(&schema, &conn, "WF001", false, Actor::Human, &tmp_path, &tmp_path)
            .unwrap();

        let (_, entry_after) = read_row(&schema, &conn, "WF001").unwrap();

        assert_eq!(entry_before, entry_after, "render must not modify any DB row");
    }

    // AC6.2: dry-run returns content without writing to disk.
    #[test]
    fn compute_render_dry_run_no_write() {
        let (tmp, conn, schema) = setup_fixture_env("executing", "dry-test", "WF001");
        let tmp_path = tmp.path().to_path_buf();

        let output = compute_render_in(
            &schema, &conn, "WF001", true, Actor::Human, &tmp_path, &tmp_path,
        ).unwrap();

        assert!(output.dry_run, "dry_run flag should be set");
        assert!(!output.content.is_empty(), "content should be populated in dry-run");

        // Nothing should be written.
        let expected_path = tmp_path.join("tasks/active/WF001-dry-test/main.md");
        assert!(
            !expected_path.exists(),
            "dry-run should not write the file: {}",
            expected_path.display()
        );
    }
}
