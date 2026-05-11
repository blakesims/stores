use anyhow::{Context, Result};
use rusqlite::Connection;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const TERMINAL_TARGET_STATUSES: &[&str] = &[
    "integrated",
    "schema_migrated",
    "cargo_installed",
    "closed_out_of_band",
    "rejected",
    "abandoned",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupMode {
    DryRun,
    ExecuteTargetsOnly,
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
    MainRepo,
    ActiveStatus,
    MissingWorkspace,
    MissingTarget,
    LiveDrivePid(i64),
    LiveCurrentRunMarker(PathBuf),
    LiveProcessUnderWorkspace(u32),
}

impl CleanupClassification {
    fn label(&self) -> String {
        match self {
            CleanupClassification::TargetCandidate => "target_candidate".to_string(),
            CleanupClassification::MainRepo => "skip_main_repo".to_string(),
            CleanupClassification::ActiveStatus => "skip_active_status".to_string(),
            CleanupClassification::MissingWorkspace => "skip_missing_workspace".to_string(),
            CleanupClassification::MissingTarget => "skip_missing_target".to_string(),
            CleanupClassification::LiveDrivePid(pid) => format!("skip_live_drive_pid:{pid}"),
            CleanupClassification::LiveCurrentRunMarker(path) => {
                format!("skip_live_current_marker:{}", path.display())
            }
            CleanupClassification::LiveProcessUnderWorkspace(pid) => {
                format!("skip_live_process_under_workspace:{pid}")
            }
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
    pub deleted_targets: Vec<CleanupCandidate>,
    pub db_bytes: u64,
    pub wal_bytes: u64,
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
    let stores_dir = crate::paths::stores_dir()?;
    let main_repo = stores_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let main_repo = canonicalize_lossy(&main_repo);
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

    if mode == CleanupMode::ExecuteTargetsOnly {
        for candidate in report.candidates.clone() {
            // Re-check immediately before mutation so an operator cannot race a stale
            // dry-run classification into deleting a target that became active.
            if classify_row_for_target_cleanup(&candidate.row, &main_repo)
                != CleanupClassification::TargetCandidate
            {
                continue;
            }
            if candidate.target_path.is_dir() {
                fs::remove_dir_all(&candidate.target_path).with_context(|| {
                    format!("removing target dir {}", candidate.target_path.display())
                })?;
                report.deleted_targets.push(candidate);
            }
        }
    }

    print_report(&report, mode);
    Ok(report)
}

fn load_task_rows(conn: &Connection) -> Result<Vec<CleanupTaskRow>> {
    let cols = table_columns(conn, "tasks")?;
    let opt = |name: &str, default_sql: &str| -> String {
        if cols.iter().any(|c| c == name) {
            name.to_string()
        } else {
            default_sql.to_string()
        }
    };
    let sql = format!(
        "SELECT display_id, status, {lifecycle}, {active_step}, {integration_step}, {blocked}, workspace_path, {drive_pid} \
         FROM tasks WHERE COALESCE(workspace_path, '') != '' ORDER BY display_id",
        lifecycle = opt("lifecycle", "NULL"),
        active_step = opt("active_step", "NULL"),
        integration_step = opt("integration_step", "NULL"),
        blocked = opt("blocked", "NULL"),
        drive_pid = opt("drive_pid", "NULL"),
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |r| {
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
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
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
    let workspace = canonicalize_lossy(&row.workspace_path);
    let main_repo = canonicalize_lossy(main_repo);
    if workspace == main_repo {
        return CleanupClassification::MainRepo;
    }
    if !TERMINAL_TARGET_STATUSES.contains(&row.status.as_str()) {
        return CleanupClassification::ActiveStatus;
    }
    if !row.workspace_path.is_dir() {
        return CleanupClassification::MissingWorkspace;
    }
    let target = row.workspace_path.join("target");
    if !target.is_dir() {
        return CleanupClassification::MissingTarget;
    }
    if let Some(pid) = row.drive_pid {
        if pid > 0 && pid_is_live(pid) {
            return CleanupClassification::LiveDrivePid(pid);
        }
    }
    if let Some(marker) = live_current_marker(&row.workspace_path, &row.display_id) {
        return CleanupClassification::LiveCurrentRunMarker(marker);
    }
    if let Some(pid) = process_under_path(&row.workspace_path) {
        return CleanupClassification::LiveProcessUnderWorkspace(pid);
    }
    CleanupClassification::TargetCandidate
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
        let cwd = entry.path().join("cwd");
        if let Ok(cwd_target) = fs::read_link(cwd) {
            let cwd_target = canonicalize_lossy(&cwd_target);
            if cwd_target.starts_with(&root) {
                return Some(pid);
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
        }
    );
    println!("main_repo\t{}", report.main_repo.display());
    println!("rows_seen\t{}", report.rows_seen);
    println!("db_bytes\t{}", report.db_bytes);
    println!("wal_bytes\t{}", report.wal_bytes);
    println!("target_candidates\t{}", report.candidate_count());
    println!("target_reclaimable_bytes\t{}", report.reclaimable_bytes());
    if mode == CleanupMode::ExecuteTargetsOnly {
        println!("target_deleted_count\t{}", report.deleted_targets.len());
        println!("target_deleted_bytes\t{}", report.deleted_bytes());
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

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
}
