use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const TERMINAL_TARGET_STATUSES: &[&str] = &[
    "integrated",
    "schema_migrated",
    "cargo_installed",
    "closed_out_of_band",
    "rejected",
    "abandoned",
];

const TERMINAL_WORKTREE_REMOVAL_STATUSES: &[&str] = &[
    "integrated",
    "schema_migrated",
    "cargo_installed",
    "closed_out_of_band",
    "rejected",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupMode {
    DryRun,
    ExecuteTargetsOnly,
    ExecuteRemoveClean,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupTaskRow {
    pub display_id: String,
    pub status: String,
    pub lifecycle: Option<String>,
    pub active_step: Option<String>,
    pub integration_step: Option<String>,
    pub blocked: Option<bool>,
    pub workspace_path: PathBuf,
    pub drive_pid: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupClassification {
    TargetCandidate,
    WorktreeRemovalCandidate,
    MainRepo,
    ActiveStatus,
    WorktreeRemovalStatusNotEligible,
    MissingWorkspace,
    MissingTarget,
    LiveDrivePid(i64),
    LiveCurrentRunMarker(PathBuf),
    LiveProcessUnderWorkspace(u32),
    DirtyWorktree,
    UnmergedBranch,
    GitCheckFailed(String),
}

impl CleanupClassification {
    fn label(&self) -> String {
        match self {
            CleanupClassification::TargetCandidate => "target_candidate".to_string(),
            CleanupClassification::WorktreeRemovalCandidate => {
                "worktree_removal_candidate".to_string()
            }
            CleanupClassification::MainRepo => "skip_main_repo".to_string(),
            CleanupClassification::ActiveStatus => "skip_active_status".to_string(),
            CleanupClassification::WorktreeRemovalStatusNotEligible => {
                "skip_worktree_removal_status_not_eligible".to_string()
            }
            CleanupClassification::MissingWorkspace => "skip_missing_workspace".to_string(),
            CleanupClassification::MissingTarget => "skip_missing_target".to_string(),
            CleanupClassification::LiveDrivePid(pid) => format!("skip_live_drive_pid:{pid}"),
            CleanupClassification::LiveCurrentRunMarker(path) => {
                format!("skip_live_current_marker:{}", path.display())
            }
            CleanupClassification::LiveProcessUnderWorkspace(pid) => {
                format!("skip_live_process_under_workspace:{pid}")
            }
            CleanupClassification::DirtyWorktree => "skip_dirty_worktree".to_string(),
            CleanupClassification::UnmergedBranch => "skip_unmerged_branch".to_string(),
            CleanupClassification::GitCheckFailed(msg) => format!("skip_git_check_failed:{msg}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CleanupCandidate {
    pub row: CleanupTaskRow,
    pub classification: CleanupClassification,
    pub target_path: PathBuf,
    pub target_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CleanupReport {
    pub main_repo: PathBuf,
    pub rows_seen: usize,
    pub candidates: Vec<CleanupCandidate>,
    pub skipped: Vec<CleanupCandidate>,
    pub removal_candidates: Vec<CleanupCandidate>,
    pub removal_skipped: Vec<CleanupCandidate>,
    pub deleted_targets: Vec<CleanupCandidate>,
    pub removed_worktrees: Vec<CleanupCandidate>,
    pub db_bytes: u64,
    pub wal_bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub struct TerminalCleanupReport {
    pub display_id: String,
    pub target_deleted: Option<CleanupCandidate>,
    pub target_skip: Option<CleanupClassification>,
    pub worktree_removed: Option<CleanupCandidate>,
    pub worktree_skip: Option<CleanupClassification>,
}

impl CleanupReport {
    pub fn candidate_count(&self) -> usize {
        self.candidates.len()
    }

    pub fn reclaimable_bytes(&self) -> u64 {
        self.candidates.iter().map(|c| c.target_bytes).sum()
    }

    pub fn deleted_bytes(&self) -> u64 {
        self.deleted_targets.iter().map(|c| c.target_bytes).sum()
    }
}

pub fn run_cleanup_worktrees(conn: &Connection, mode: CleanupMode) -> Result<CleanupReport> {
    let main_repo = cleanup_main_repo()?;
    let db_path = crate::paths::db_path()?;
    let db_bytes = file_len(&db_path);
    let wal_bytes = file_len(&db_path.with_file_name("db.sqlite-wal"));

    let rows = load_task_rows(conn)?;
    let mut report = CleanupReport {
        main_repo: main_repo.clone(),
        rows_seen: rows.len(),
        db_bytes,
        wal_bytes,
        ..CleanupReport::default()
    };

    for row in rows {
        let classification = classify_row_for_target_cleanup(&row, &main_repo);
        let target_path = row.workspace_path.join("target");
        let target_bytes = if target_path.is_dir() {
            dir_size_bytes(&target_path).unwrap_or(0)
        } else {
            0
        };
        let candidate = CleanupCandidate {
            row,
            classification: classification.clone(),
            target_path,
            target_bytes,
        };
        if classification == CleanupClassification::TargetCandidate {
            report.candidates.push(candidate);
        } else {
            report.skipped.push(candidate);
        }
    }

    for row in load_task_rows(conn)? {
        let classification = classify_row_for_worktree_removal(&row, &main_repo);
        let target_path = row.workspace_path.join("target");
        let target_bytes = if target_path.is_dir() {
            dir_size_bytes(&target_path).unwrap_or(0)
        } else {
            0
        };
        let candidate = CleanupCandidate {
            row,
            classification: classification.clone(),
            target_path,
            target_bytes,
        };
        if classification == CleanupClassification::WorktreeRemovalCandidate {
            report.removal_candidates.push(candidate);
        } else {
            report.removal_skipped.push(candidate);
        }
    }

    match mode {
        CleanupMode::DryRun => {}
        CleanupMode::ExecuteTargetsOnly => execute_targets_only(conn, &main_repo, &mut report)?,
        CleanupMode::ExecuteRemoveClean => execute_remove_clean(conn, &main_repo, &mut report)?,
    }

    print_report(&report, mode);
    Ok(report)
}

pub fn cleanup_terminal_task(conn: &Connection, display_id: &str) -> Result<TerminalCleanupReport> {
    let main_repo = cleanup_main_repo()?;
    let mut report = TerminalCleanupReport {
        display_id: display_id.to_string(),
        ..TerminalCleanupReport::default()
    };

    let Some(row) = load_task_row(conn, display_id)? else {
        report.target_skip = Some(CleanupClassification::MissingWorkspace);
        report.worktree_skip = Some(CleanupClassification::MissingWorkspace);
        return Ok(report);
    };

    let target_classification = classify_row_for_target_cleanup(&row, &main_repo);
    if target_classification == CleanupClassification::TargetCandidate {
        let target_path = row.workspace_path.join("target");
        let target_bytes = dir_size_bytes(&target_path).unwrap_or(0);
        fs::remove_dir_all(&target_path)
            .with_context(|| format!("removing target dir {}", target_path.display()))?;
        report.target_deleted = Some(CleanupCandidate {
            row: row.clone(),
            classification: CleanupClassification::TargetCandidate,
            target_path,
            target_bytes,
        });
    } else {
        report.target_skip = Some(target_classification);
    }

    let Some(fresh_row) = load_task_row(conn, display_id)? else {
        report.worktree_skip = Some(CleanupClassification::MissingWorkspace);
        return Ok(report);
    };
    let removal_classification = classify_row_for_worktree_removal(&fresh_row, &main_repo);
    if removal_classification == CleanupClassification::WorktreeRemovalCandidate {
        let out = Command::new("git")
            .args([
                "-C",
                main_repo.to_str().unwrap_or("."),
                "worktree",
                "remove",
                fresh_row.workspace_path.to_str().unwrap_or(""),
            ])
            .output()
            .with_context(|| {
                format!("spawning git worktree remove for {}", fresh_row.display_id)
            })?;
        if !out.status.success() {
            anyhow::bail!(
                "git worktree remove {} failed: {}",
                fresh_row.workspace_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        report.worktree_removed = Some(CleanupCandidate {
            target_path: fresh_row.workspace_path.join("target"),
            target_bytes: 0,
            row: fresh_row,
            classification: CleanupClassification::WorktreeRemovalCandidate,
        });
    } else {
        report.worktree_skip = Some(removal_classification);
    }

    Ok(report)
}

fn cleanup_main_repo() -> Result<PathBuf> {
    let stores_dir = crate::paths::stores_dir()?;
    let main_repo = stores_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(canonicalize_lossy(&main_repo))
}

fn execute_targets_only(
    conn: &Connection,
    main_repo: &Path,
    report: &mut CleanupReport,
) -> Result<()> {
    for candidate in report.candidates.clone() {
        // Re-fetch the row and re-check filesystem/process state immediately
        // before mutation. A long-running cleanup must not delete a target for
        // a task that changed status, picked up a live drive_pid, or restarted
        // a current run after the initial audit pass.
        let Some(fresh_row) = load_task_row(conn, &candidate.row.display_id)? else {
            continue;
        };
        if classify_row_for_target_cleanup(&fresh_row, main_repo)
            != CleanupClassification::TargetCandidate
        {
            continue;
        }
        let fresh_target_path = fresh_row.workspace_path.join("target");
        if fresh_target_path.is_dir() {
            fs::remove_dir_all(&fresh_target_path)
                .with_context(|| format!("removing target dir {}", fresh_target_path.display()))?;
            report.deleted_targets.push(CleanupCandidate {
                row: fresh_row,
                target_path: fresh_target_path,
                ..candidate
            });
        }
    }
    Ok(())
}

fn execute_remove_clean(
    conn: &Connection,
    main_repo: &Path,
    report: &mut CleanupReport,
) -> Result<()> {
    for candidate in report.removal_candidates.clone() {
        // Re-fetch and re-classify immediately before removing a worktree.
        let Some(fresh_row) = load_task_row(conn, &candidate.row.display_id)? else {
            continue;
        };
        if classify_row_for_worktree_removal(&fresh_row, main_repo)
            != CleanupClassification::WorktreeRemovalCandidate
        {
            continue;
        }
        let out = Command::new("git")
            .args([
                "-C",
                main_repo.to_str().unwrap_or("."),
                "worktree",
                "remove",
                fresh_row.workspace_path.to_str().unwrap_or(""),
            ])
            .output()
            .with_context(|| {
                format!("spawning git worktree remove for {}", fresh_row.display_id)
            })?;
        if !out.status.success() {
            anyhow::bail!(
                "git worktree remove {} failed: {}",
                fresh_row.workspace_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        report.removed_worktrees.push(CleanupCandidate {
            row: fresh_row,
            ..candidate
        });
    }
    Ok(())
}

fn load_task_rows(conn: &Connection) -> Result<Vec<CleanupTaskRow>> {
    let sql = task_row_select_sql(
        conn,
        "WHERE COALESCE(workspace_path, '') != '' ORDER BY display_id",
    )?;
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], cleanup_task_row_from_sql)?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn load_task_row(conn: &Connection, display_id: &str) -> Result<Option<CleanupTaskRow>> {
    let sql = task_row_select_sql(conn, "WHERE display_id = ?1")?;
    conn.query_row(&sql, [display_id], cleanup_task_row_from_sql)
        .optional()
        .map_err(Into::into)
}

fn task_row_select_sql(conn: &Connection, suffix: &str) -> Result<String> {
    let cols = table_columns(conn, "tasks")?;
    let opt = |name: &str, default_sql: &str| -> String {
        if cols.iter().any(|c| c == name) {
            name.to_string()
        } else {
            default_sql.to_string()
        }
    };
    Ok(format!(
        "SELECT display_id, status, {lifecycle}, {active_step}, {integration_step}, {blocked}, workspace_path, {drive_pid} \
         FROM tasks {suffix}",
        lifecycle = opt("lifecycle", "NULL"),
        active_step = opt("active_step", "NULL"),
        integration_step = opt("integration_step", "NULL"),
        blocked = opt("blocked", "NULL"),
        drive_pid = opt("drive_pid", "NULL"),
    ))
}

fn cleanup_task_row_from_sql(r: &rusqlite::Row<'_>) -> rusqlite::Result<CleanupTaskRow> {
    let blocked_int: Option<i64> = r.get(5)?;
    let workspace: String = r.get(6)?;
    Ok(CleanupTaskRow {
        display_id: r.get(0)?,
        status: r.get(1)?,
        lifecycle: r.get(2)?,
        active_step: r.get(3)?,
        integration_step: r.get(4)?,
        blocked: blocked_int.map(|v| v != 0),
        workspace_path: PathBuf::from(workspace),
        drive_pid: r.get(7)?,
    })
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn classify_row_for_target_cleanup(
    row: &CleanupTaskRow,
    main_repo: &Path,
) -> CleanupClassification {
    if let Some(skip) = common_terminal_live_skip(row, main_repo, TERMINAL_TARGET_STATUSES) {
        return skip;
    }
    let target = row.workspace_path.join("target");
    if !target.is_dir() {
        return CleanupClassification::MissingTarget;
    }
    CleanupClassification::TargetCandidate
}

pub fn classify_row_for_worktree_removal(
    row: &CleanupTaskRow,
    main_repo: &Path,
) -> CleanupClassification {
    if let Some(skip) =
        common_terminal_live_skip(row, main_repo, TERMINAL_WORKTREE_REMOVAL_STATUSES)
    {
        if skip == CleanupClassification::ActiveStatus && row.status == "abandoned" {
            return CleanupClassification::WorktreeRemovalStatusNotEligible;
        }
        return skip;
    }
    match git_status_clean(&row.workspace_path) {
        Ok(true) => {}
        Ok(false) => return CleanupClassification::DirtyWorktree,
        Err(msg) => return CleanupClassification::GitCheckFailed(msg),
    }
    match branch_merged_to_main(&row.workspace_path, main_repo) {
        Ok(true) => CleanupClassification::WorktreeRemovalCandidate,
        Ok(false) => CleanupClassification::UnmergedBranch,
        Err(msg) => CleanupClassification::GitCheckFailed(msg),
    }
}

fn common_terminal_live_skip(
    row: &CleanupTaskRow,
    main_repo: &Path,
    eligible_statuses: &[&str],
) -> Option<CleanupClassification> {
    let workspace = canonicalize_lossy(&row.workspace_path);
    let main_repo = canonicalize_lossy(main_repo);
    if workspace == main_repo {
        return Some(CleanupClassification::MainRepo);
    }
    if !eligible_statuses.contains(&row.status.as_str()) {
        return Some(CleanupClassification::ActiveStatus);
    }
    if !row.workspace_path.is_dir() {
        return Some(CleanupClassification::MissingWorkspace);
    }
    if let Some(pid) = row.drive_pid {
        if pid > 0 && pid_is_live(pid) {
            return Some(CleanupClassification::LiveDrivePid(pid));
        }
    }
    if let Some(marker) = live_current_marker(&row.workspace_path, &row.display_id) {
        return Some(CleanupClassification::LiveCurrentRunMarker(marker));
    }
    if let Some(pid) = process_under_path(&row.workspace_path) {
        return Some(CleanupClassification::LiveProcessUnderWorkspace(pid));
    }
    None
}

fn git_status_clean(workspace: &Path) -> std::result::Result<bool, String> {
    let out = Command::new("git")
        .args([
            "-C",
            workspace.to_str().unwrap_or("."),
            "status",
            "--porcelain",
        ])
        .output()
        .map_err(|e| format!("git_status_spawn:{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git_status:{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().is_empty())
}

fn branch_merged_to_main(workspace: &Path, main_repo: &Path) -> std::result::Result<bool, String> {
    let head = git_rev_parse(workspace, "HEAD")?;
    let main = git_rev_parse(main_repo, "main")?;
    let out = Command::new("git")
        .args([
            "-C",
            main_repo.to_str().unwrap_or("."),
            "merge-base",
            "--is-ancestor",
            &head,
            &main,
        ])
        .output()
        .map_err(|e| format!("merge_base_spawn:{e}"))?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "merge_base:{}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

fn git_rev_parse(repo: &Path, rev: &str) -> std::result::Result<String, String> {
    let out = Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap_or("."),
            "rev-parse",
            "--verify",
            rev,
        ])
        .output()
        .map_err(|e| format!("rev_parse_spawn:{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "rev_parse_{rev}:{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn live_current_marker(workspace: &Path, display_id: &str) -> Option<PathBuf> {
    let runs_dir = workspace.join(".stores").join("runs");
    let entries = fs::read_dir(&runs_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.starts_with(&format!("current-{display_id}-")) || !name.ends_with(".json") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap_or_default();
        if text.contains("\"status\":\"running\"")
            || text.contains("\"status\": \"running\"")
            || text.contains("\"status\":\"live\"")
            || text.contains("\"status\": \"live\"")
        {
            return Some(path);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn pid_is_live(pid: i64) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(not(target_os = "linux"))]
fn pid_is_live(_pid: i64) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn process_under_path(path: &Path) -> Option<u32> {
    let root = canonicalize_lossy(path);
    let proc = fs::read_dir("/proc").ok()?;
    for entry in proc.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let proc_dir = entry.path();
        let cwd = proc_dir.join("cwd");
        if let Ok(cwd_target) = fs::read_link(cwd) {
            let cwd_target = canonicalize_lossy(&cwd_target);
            if cwd_target.starts_with(&root) {
                return Some(pid);
            }
        }
        let fd_dir = proc_dir.join("fd");
        if let Ok(fds) = fs::read_dir(fd_dir) {
            for fd in fds.flatten() {
                if let Ok(fd_target) = fs::read_link(fd.path()) {
                    let fd_target = canonicalize_lossy(&fd_target);
                    if fd_target.starts_with(&root) {
                        return Some(pid);
                    }
                }
            }
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn process_under_path(_path: &Path) -> Option<u32> {
    None
}

fn canonicalize_lossy(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn dir_size_bytes(path: &Path) -> Result<u64> {
    let mut total = 0;
    if !path.exists() {
        return Ok(0);
    }
    #[cfg(unix)]
    let mut seen_inodes = std::collections::HashSet::<(u64, u64)>::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.file_type().is_symlink() {
                continue;
            }
            #[cfg(unix)]
            {
                let key = (meta.dev(), meta.ino());
                if !seen_inodes.insert(key) {
                    continue;
                }
            }
            total += disk_usage_bytes(&meta);
            if meta.is_dir() {
                stack.push(entry.path());
            }
        }
    }
    Ok(total)
}

#[cfg(unix)]
fn disk_usage_bytes(meta: &fs::Metadata) -> u64 {
    meta.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn disk_usage_bytes(meta: &fs::Metadata) -> u64 {
    meta.len()
}

fn print_report(report: &CleanupReport, mode: CleanupMode) {
    println!(
        "mode\t{}",
        match mode {
            CleanupMode::DryRun => "dry-run",
            CleanupMode::ExecuteTargetsOnly => "execute-targets-only",
            CleanupMode::ExecuteRemoveClean => "execute-remove-clean",
        }
    );
    println!("main_repo\t{}", report.main_repo.display());
    println!("rows_seen\t{}", report.rows_seen);
    println!("db_bytes\t{}", report.db_bytes);
    println!("wal_bytes\t{}", report.wal_bytes);
    println!("target_candidates\t{}", report.candidate_count());
    println!("target_reclaimable_bytes\t{}", report.reclaimable_bytes());
    println!(
        "worktree_removal_candidates\t{}",
        report.removal_candidates.len()
    );
    if mode == CleanupMode::ExecuteTargetsOnly {
        println!("target_deleted_count\t{}", report.deleted_targets.len());
        println!("target_deleted_bytes\t{}", report.deleted_bytes());
    }
    if mode == CleanupMode::ExecuteRemoveClean {
        println!("worktree_removed_count\t{}", report.removed_worktrees.len());
    }
    println!("\n# target cleanup candidates");
    println!("display_id\tstatus\ttarget_bytes\tworkspace_path\ttarget_path");
    for c in &report.candidates {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            c.row.display_id,
            c.row.status,
            c.target_bytes,
            c.row.workspace_path.display(),
            c.target_path.display()
        );
    }
    println!("\n# skipped workspaces");
    println!("display_id\tstatus\treason\tworkspace_path");
    for c in &report.skipped {
        println!(
            "{}\t{}\t{}\t{}",
            c.row.display_id,
            c.row.status,
            c.classification.label(),
            c.row.workspace_path.display()
        );
    }
    println!("\n# clean worktree removal candidates");
    println!("display_id\tstatus\tworkspace_path");
    for c in &report.removal_candidates {
        println!(
            "{}\t{}\t{}",
            c.row.display_id,
            c.row.status,
            c.row.workspace_path.display()
        );
    }
    println!("\n# skipped worktree removals");
    println!("display_id\tstatus\treason\tworkspace_path");
    for c in &report.removal_skipped {
        println!(
            "{}\t{}\t{}\t{}",
            c.row.display_id,
            c.row.status,
            c.classification.label(),
            c.row.workspace_path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::process::Command;
    use tempfile::tempdir;

    fn git(repo: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git -C {} {:?} failed: {}",
            repo.display(),
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_main_repo(path: &Path) {
        fs::create_dir_all(path).unwrap();
        let out = Command::new("git")
            .arg("init")
            .arg("-b")
            .arg("main")
            .arg(path)
            .output()
            .unwrap();
        assert!(out.status.success(), "git init failed");
        git(path, &["config", "user.email", "test@example.com"]);
        git(path, &["config", "user.name", "Test User"]);
        fs::write(path.join("README.md"), b"hello").unwrap();
        fs::write(path.join(".gitignore"), b"target/\n").unwrap();
        git(path, &["add", "README.md", ".gitignore"]);
        git(path, &["commit", "-m", "init"]);
    }

    fn setup_cleanup_db(
        stores: &Path,
        task_id: &str,
        status: &str,
        workspace: &Path,
    ) -> Connection {
        fs::create_dir_all(stores).unwrap();
        fs::write(stores.join("db.sqlite"), b"db").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                display_id TEXT,
                status TEXT,
                lifecycle TEXT,
                active_step TEXT,
                integration_step TEXT,
                blocked INTEGER,
                workspace_path TEXT,
                drive_pid INTEGER
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,workspace_path) VALUES (?1,?2,?3)",
            rusqlite::params![task_id, status, workspace.to_str().unwrap()],
        )
        .unwrap();
        conn
    }

    fn row(id: &str, status: &str, workspace: &Path) -> CleanupTaskRow {
        CleanupTaskRow {
            display_id: id.to_string(),
            status: status.to_string(),
            lifecycle: None,
            active_step: None,
            integration_step: None,
            blocked: None,
            workspace_path: workspace.to_path_buf(),
            drive_pid: None,
        }
    }

    #[test]
    fn classify_terminal_with_target_as_candidate() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("main");
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&main).unwrap();
        fs::create_dir_all(wt.join("target")).unwrap();
        assert_eq!(
            classify_row_for_target_cleanup(&row("T001", "integrated", &wt), &main),
            CleanupClassification::TargetCandidate
        );
    }

    #[test]
    fn classify_active_and_main_repo_are_skipped() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("main");
        let wt = tmp.path().join("wt");
        fs::create_dir_all(main.join("target")).unwrap();
        fs::create_dir_all(wt.join("target")).unwrap();
        assert_eq!(
            classify_row_for_target_cleanup(&row("TMAIN", "integrated", &main), &main),
            CleanupClassification::MainRepo
        );
        assert_eq!(
            classify_row_for_target_cleanup(&row("TACT", "executing", &wt), &main),
            CleanupClassification::ActiveStatus
        );
    }

    #[test]
    fn classify_live_current_marker_is_skipped() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("main");
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&main).unwrap();
        fs::create_dir_all(wt.join("target")).unwrap();
        fs::create_dir_all(wt.join(".stores/runs")).unwrap();
        fs::write(
            wt.join(".stores/runs/current-T001-executor.json"),
            r#"{"display_id":"T001","role":"executor","status":"running"}"#,
        )
        .unwrap();
        match classify_row_for_target_cleanup(&row("T001", "integrated", &wt), &main) {
            CleanupClassification::LiveCurrentRunMarker(path) => {
                assert!(path.ends_with("current-T001-executor.json"));
            }
            other => panic!("expected live current marker skip, got {other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn classify_process_under_workspace_is_skipped() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("main");
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&main).unwrap();
        fs::create_dir_all(wt.join("target")).unwrap();

        let mut child = Command::new("sh")
            .arg("-c")
            .arg("cd \"$1\" && sleep 30")
            .arg("sh")
            .arg(&wt)
            .spawn()
            .unwrap();

        let mut observed = None;
        for _ in 0..20 {
            match classify_row_for_target_cleanup(&row("T001", "integrated", &wt), &main) {
                CleanupClassification::LiveProcessUnderWorkspace(pid) => {
                    observed = Some(pid);
                    break;
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(50)),
            }
        }

        let _ = child.kill();
        let _ = child.wait();

        assert!(
            observed.is_some(),
            "a process with cwd under workspace must block target cleanup"
        );
    }

    #[test]
    fn execute_reloads_row_before_deleting_target() {
        let tmp = tempdir().unwrap();
        let stores = tmp.path().join("repo/.stores");
        let term = tmp.path().join("term");
        fs::create_dir_all(&stores).unwrap();
        fs::create_dir_all(term.join("target")).unwrap();
        fs::write(stores.join("db.sqlite"), b"db").unwrap();
        fs::write(term.join("target/file"), b"artifact").unwrap();

        let _guard = crate::cli::test_support::ENV_LOCK.lock().unwrap();
        crate::paths::clear_stores_dir_override_for_tests();
        crate::paths::set_stores_dir_override(stores.clone()).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                display_id TEXT,
                status TEXT,
                lifecycle TEXT,
                active_step TEXT,
                integration_step TEXT,
                blocked INTEGER,
                workspace_path TEXT,
                drive_pid INTEGER
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,workspace_path,drive_pid) VALUES ('T001','integrated',?1,?2)",
            rusqlite::params![term.to_str().unwrap(), std::process::id() as i64],
        )
        .unwrap();

        let report = run_cleanup_worktrees(&conn, CleanupMode::ExecuteTargetsOnly).unwrap();
        assert_eq!(report.deleted_targets.len(), 0);
        assert!(term.join("target").exists());
        crate::paths::clear_stores_dir_override_for_tests();
    }

    #[test]
    fn target_only_execute_deletes_only_terminal_target() {
        let tmp = tempdir().unwrap();
        let stores = tmp.path().join("repo/.stores");
        let main = tmp.path().join("repo");
        let term = tmp.path().join("term");
        let active = tmp.path().join("active");
        fs::create_dir_all(&stores).unwrap();
        fs::create_dir_all(main.join("target")).unwrap();
        fs::create_dir_all(term.join("target")).unwrap();
        fs::create_dir_all(active.join("target")).unwrap();
        fs::write(stores.join("db.sqlite"), b"db").unwrap();
        fs::write(term.join("target/file"), b"artifact").unwrap();
        fs::write(active.join("target/file"), b"artifact").unwrap();

        let _guard = crate::cli::test_support::ENV_LOCK.lock().unwrap();
        crate::paths::clear_stores_dir_override_for_tests();
        crate::paths::set_stores_dir_override(stores.clone()).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (
                display_id TEXT,
                status TEXT,
                lifecycle TEXT,
                active_step TEXT,
                integration_step TEXT,
                blocked INTEGER,
                workspace_path TEXT,
                drive_pid INTEGER
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,workspace_path) VALUES ('T001','integrated',?1)",
            rusqlite::params![term.to_str().unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,workspace_path) VALUES ('T002','executing',?1)",
            rusqlite::params![active.to_str().unwrap()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,status,workspace_path) VALUES ('TMAIN','integrated',?1)",
            rusqlite::params![main.to_str().unwrap()],
        )
        .unwrap();

        let report = run_cleanup_worktrees(&conn, CleanupMode::ExecuteTargetsOnly).unwrap();
        assert_eq!(report.deleted_targets.len(), 1);
        assert!(!term.join("target").exists());
        assert!(active.join("target").exists());
        assert!(main.join("target").exists());
        crate::paths::clear_stores_dir_override_for_tests();
    }

    #[test]
    fn classify_clean_merged_worktree_as_removal_candidate() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("repo");
        let wt = tmp.path().join("wt");
        init_main_repo(&main);
        git(
            &main,
            &["worktree", "add", "-b", "feat/t1", wt.to_str().unwrap()],
        );

        assert_eq!(
            classify_row_for_worktree_removal(&row("T001", "integrated", &wt), &main),
            CleanupClassification::WorktreeRemovalCandidate
        );
    }

    #[test]
    fn abandoned_worktrees_are_not_removal_candidates_without_disposition_policy() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("repo");
        let wt = tmp.path().join("wt");
        init_main_repo(&main);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "feat/abandoned",
                wt.to_str().unwrap(),
            ],
        );

        assert_eq!(
            classify_row_for_worktree_removal(&row("T999", "abandoned", &wt), &main),
            CleanupClassification::WorktreeRemovalStatusNotEligible
        );
    }

    #[test]
    fn classify_dirty_and_unmerged_worktrees_are_skipped_for_removal() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("repo");
        let dirty = tmp.path().join("dirty");
        let unmerged = tmp.path().join("unmerged");
        init_main_repo(&main);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "feat/dirty",
                dirty.to_str().unwrap(),
            ],
        );
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "feat/unmerged",
                unmerged.to_str().unwrap(),
            ],
        );
        fs::write(dirty.join("untracked.txt"), b"dirty").unwrap();
        fs::write(unmerged.join("new.txt"), b"new").unwrap();
        git(&unmerged, &["add", "new.txt"]);
        git(&unmerged, &["commit", "-m", "unmerged"]);

        assert_eq!(
            classify_row_for_worktree_removal(&row("TDIRTY", "integrated", &dirty), &main),
            CleanupClassification::DirtyWorktree
        );
        assert_eq!(
            classify_row_for_worktree_removal(&row("TUNMERGED", "integrated", &unmerged), &main),
            CleanupClassification::UnmergedBranch
        );
    }

    #[test]
    fn execute_remove_clean_removes_only_clean_merged_worktree() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("repo");
        let stores = main.join(".stores");
        let clean = tmp.path().join("clean");
        init_main_repo(&main);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "feat/clean",
                clean.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(clean.join("target")).unwrap();
        fs::write(clean.join("target/file"), b"artifact").unwrap();

        let _guard = crate::cli::test_support::ENV_LOCK.lock().unwrap();
        crate::paths::clear_stores_dir_override_for_tests();
        crate::paths::set_stores_dir_override(stores.clone()).unwrap();
        let conn = setup_cleanup_db(&stores, "T001", "integrated", &clean);

        let report = run_cleanup_worktrees(&conn, CleanupMode::ExecuteRemoveClean).unwrap();
        assert_eq!(report.removed_worktrees.len(), 1);
        assert!(!clean.exists(), "clean merged worktree should be removed");
        crate::paths::clear_stores_dir_override_for_tests();
    }

    #[test]
    fn terminal_cleanup_deletes_target_then_removes_clean_worktree() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("repo");
        let stores = main.join(".stores");
        let clean = tmp.path().join("clean-terminal");
        init_main_repo(&main);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "feat/clean-terminal",
                clean.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(clean.join("target")).unwrap();
        fs::write(clean.join("target/file"), b"artifact").unwrap();

        let _guard = crate::cli::test_support::ENV_LOCK.lock().unwrap();
        crate::paths::clear_stores_dir_override_for_tests();
        crate::paths::set_stores_dir_override(stores.clone()).unwrap();
        let conn = setup_cleanup_db(&stores, "T777", "integrated", &clean);

        let report = cleanup_terminal_task(&conn, "T777").unwrap();
        assert!(report.target_deleted.is_some());
        assert!(report.worktree_removed.is_some());
        assert!(!clean.exists(), "clean terminal worktree should be removed");
        crate::paths::clear_stores_dir_override_for_tests();
    }

    #[test]
    fn terminal_cleanup_keeps_dirty_worktree_source_only() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("repo");
        let stores = main.join(".stores");
        let dirty = tmp.path().join("dirty-terminal");
        init_main_repo(&main);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "feat/dirty-terminal",
                dirty.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(dirty.join("target")).unwrap();
        fs::write(dirty.join("target/file"), b"artifact").unwrap();
        fs::write(dirty.join("untracked.txt"), b"keep me").unwrap();

        let _guard = crate::cli::test_support::ENV_LOCK.lock().unwrap();
        crate::paths::clear_stores_dir_override_for_tests();
        crate::paths::set_stores_dir_override(stores.clone()).unwrap();
        let conn = setup_cleanup_db(&stores, "T778", "closed_out_of_band", &dirty);

        let report = cleanup_terminal_task(&conn, "T778").unwrap();
        assert!(report.target_deleted.is_some());
        assert_eq!(
            report.worktree_skip,
            Some(CleanupClassification::DirtyWorktree)
        );
        assert!(dirty.exists(), "dirty terminal worktree source must remain");
        assert!(!dirty.join("target").exists(), "target should be deleted");
        assert!(
            dirty.join("untracked.txt").exists(),
            "source residue must remain"
        );
        crate::paths::clear_stores_dir_override_for_tests();
    }

    #[test]
    fn terminal_cleanup_abandoned_deletes_target_but_does_not_remove_worktree() {
        let tmp = tempdir().unwrap();
        let main = tmp.path().join("repo");
        let stores = main.join(".stores");
        let abandoned = tmp.path().join("abandoned-terminal");
        init_main_repo(&main);
        git(
            &main,
            &[
                "worktree",
                "add",
                "-b",
                "feat/abandoned-terminal",
                abandoned.to_str().unwrap(),
            ],
        );
        fs::create_dir_all(abandoned.join("target")).unwrap();
        fs::write(abandoned.join("target/file"), b"artifact").unwrap();

        let _guard = crate::cli::test_support::ENV_LOCK.lock().unwrap();
        crate::paths::clear_stores_dir_override_for_tests();
        crate::paths::set_stores_dir_override(stores.clone()).unwrap();
        let conn = setup_cleanup_db(&stores, "T779", "abandoned", &abandoned);

        let report = cleanup_terminal_task(&conn, "T779").unwrap();
        assert!(report.target_deleted.is_some());
        assert_eq!(
            report.worktree_skip,
            Some(CleanupClassification::WorktreeRemovalStatusNotEligible)
        );
        assert!(
            abandoned.exists(),
            "abandoned worktree stays for disposition"
        );
        assert!(
            !abandoned.join("target").exists(),
            "target should be deleted"
        );
        crate::paths::clear_stores_dir_override_for_tests();
    }
}
