use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunTranscript {
    pub display_id: String,
    pub phase: i64,
    pub cycle: i64,
    pub role: String,
    pub path: PathBuf,
}

pub enum RunsCmd {
    List { display_id: String },
    Show {
        display_id: String,
        phase: i64,
        cycle: Option<i64>,
        role: String,
    },
}

pub fn run(cmd: RunsCmd) -> Result<()> {
    match cmd {
        RunsCmd::List { display_id } => {
            let stores_dir = crate::paths::stores_dir()?;
            let rows = list_for_task(&stores_dir, &display_id)?;
            println!("phase\tcycle\trole\ttranscript_path");
            for row in rows {
                println!(
                    "{}\t{}\t{}\t{}",
                    row.phase,
                    row.cycle,
                    row.role,
                    row.path.display()
                );
            }
            Ok(())
        }
        RunsCmd::Show {
            display_id,
            phase,
            cycle,
            role,
        } => {
            let stores_dir = crate::paths::stores_dir()?;
            let row = find_transcript(&stores_dir, &display_id, phase, cycle, &role)?;
            let read_path = resolve_transcript_path(&stores_dir, &row.path);
            let body = fs::read_to_string(&read_path).with_context(|| {
                format!(
                    "failed to read transcript {} (resolved to {})",
                    row.path.display(),
                    read_path.display()
                )
            })?;
            println!("{body}");
            Ok(())
        }
    }
}

pub fn list_for_task(stores_dir: &Path, display_id: &str) -> Result<Vec<RunTranscript>> {
    let db_path = stores_dir.join("db.sqlite");
    let conn = Connection::open(&db_path)
        .with_context(|| format!("failed to open substrate DB {}", db_path.display()))?;
    let cycles_json: String = conn
        .query_row(
            "SELECT cycles FROM tasks WHERE display_id = ?1",
            [display_id],
            |row| row.get(0),
        )
        .with_context(|| format!("task {display_id} not found in substrate DB"))?;

    let cycles: serde_json::Value = serde_json::from_str(&cycles_json)
        .with_context(|| format!("task {display_id} cycles JSON is invalid"))?;
    let cycles = cycles
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("task {display_id} cycles field is not an array"))?;

    let mut rows = Vec::new();
    for entry in cycles {
        let phase = entry.get("phase").and_then(|v| v.as_i64()).unwrap_or(1);
        let cycle = entry.get("cycle").and_then(|v| v.as_i64()).unwrap_or(1);
        collect_role_transcript(stores_dir, display_id, phase, cycle, entry, "executor", &mut rows)?;
        collect_role_transcript(
            stores_dir,
            display_id,
            phase,
            cycle,
            entry,
            "code-reviewer",
            &mut rows,
        )?;
    }

    if rows.is_empty() {
        bail!("no transcript backlinks found for {display_id} in tasks.cycles");
    }

    rows.sort_by(|a, b| {
        (a.phase, a.cycle, a.role.as_str(), a.path.to_string_lossy()).cmp(&(
            b.phase,
            b.cycle,
            b.role.as_str(),
            b.path.to_string_lossy(),
        ))
    });
    Ok(rows)
}

pub fn find_transcript(
    stores_dir: &Path,
    display_id: &str,
    phase: i64,
    cycle: Option<i64>,
    role: &str,
) -> Result<RunTranscript> {
    let matches: Vec<RunTranscript> = list_for_task(stores_dir, display_id)?
        .into_iter()
        .filter(|r| {
            r.phase == phase && r.role == role && cycle.map(|c| r.cycle == c).unwrap_or(true)
        })
        .collect();

    match matches.len() {
        0 => bail!(
            "missing transcript for {display_id} phase {phase} role {role}{}",
            cycle.map(|c| format!(" cycle {c}")).unwrap_or_default()
        ),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => bail!(
            "multiple transcripts for {display_id} phase {phase} role {role}; pass --cycle to disambiguate"
        ),
    }
}

fn collect_role_transcript(
    stores_dir: &Path,
    display_id: &str,
    phase: i64,
    cycle: i64,
    entry: &serde_json::Value,
    role: &str,
    rows: &mut Vec<RunTranscript>,
) -> Result<()> {
    let subrecord = match role {
        "executor" => "executor",
        "code-reviewer" => "review",
        _ => role,
    };
    let Some(path_str) = entry
        .get(subrecord)
        .and_then(|v| v.get("transcript_path"))
        .and_then(|v| v.as_str())
    else {
        return Ok(());
    };

    let path = PathBuf::from(path_str);
    let read_path = resolve_transcript_path(stores_dir, &path);
    if !read_path.exists() {
        bail!(
            "missing transcript for {display_id} phase {phase} cycle {cycle} role {role}: {} does not exist (resolved to {})",
            path.display(),
            read_path.display()
        );
    }

    rows.push(RunTranscript {
        display_id: display_id.to_string(),
        phase,
        cycle,
        role: role.to_string(),
        path,
    });
    Ok(())
}

fn resolve_transcript_path(stores_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    if let Ok(stripped) = path.strip_prefix(".stores") {
        if let Some(root) = stores_dir.parent() {
            return root.join(".stores").join(stripped);
        }
    }
    stores_dir.join(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let stores = tmp.path().join(".stores");
        fs::create_dir_all(stores.join("runs")).unwrap();
        fs::write(
            stores.join("runs/executor-session.jsonl"),
            r#"{"role":"executor","summary":"fixture executor"}"#,
        )
        .unwrap();
        fs::write(
            stores.join("runs/review-session.jsonl"),
            r#"{"role":"code-reviewer","gate":"PASS"}"#,
        )
        .unwrap();
        let conn = Connection::open(stores.join("db.sqlite")).unwrap();
        conn.execute(
            "CREATE TABLE tasks (display_id TEXT UNIQUE NOT NULL, cycles TEXT)",
            [],
        )
        .unwrap();
        let cycles = serde_json::json!([
            {
                "phase": 2,
                "cycle": 2,
                "executor": {"transcript_path": ".stores/runs/executor-session.jsonl"}
            },
            {
                "phase": 2,
                "cycle": 1,
                "executor": {"transcript_path": ".stores/runs/executor-session.jsonl"},
                "review": {"transcript_path": ".stores/runs/review-session.jsonl"}
            }
        ]);
        conn.execute(
            "INSERT INTO tasks (display_id, cycles) VALUES (?1, ?2)",
            params!["T999", serde_json::to_string(&cycles).unwrap()],
        )
        .unwrap();
        tmp
    }

    #[test]
    fn list_outputs_deterministic_order_from_cycle_backlinks() {
        let tmp = fixture();
        let rows = list_for_task(&tmp.path().join(".stores"), "T999").unwrap();
        let keys: Vec<_> = rows
            .iter()
            .map(|r| (r.phase, r.cycle, r.role.as_str(), r.path.to_string_lossy().to_string()))
            .collect();
        assert_eq!(
            keys,
            vec![
                (
                    2,
                    1,
                    "code-reviewer",
                    ".stores/runs/review-session.jsonl".to_string()
                ),
                (
                    2,
                    1,
                    "executor",
                    ".stores/runs/executor-session.jsonl".to_string()
                ),
                (
                    2,
                    2,
                    "executor",
                    ".stores/runs/executor-session.jsonl".to_string()
                ),
            ]
        );
    }

    #[test]
    fn show_finds_existing_transcript_backlink() {
        let tmp = fixture();
        let row = find_transcript(
            &tmp.path().join(".stores"),
            "T999",
            2,
            Some(1),
            "executor",
        )
        .unwrap();
        assert_eq!(row.path, PathBuf::from(".stores/runs/executor-session.jsonl"));
    }

    #[test]
    fn missing_transcript_errors_cleanly() {
        let tmp = fixture();
        let err =
            find_transcript(&tmp.path().join(".stores"), "T999", 3, None, "executor").unwrap_err();
        assert!(err
            .to_string()
            .contains("missing transcript for T999 phase 3 role executor"));
    }

    #[test]
    fn missing_linked_file_errors_cleanly() {
        let tmp = fixture();
        fs::remove_file(tmp.path().join(".stores/runs/review-session.jsonl")).unwrap();
        let err = list_for_task(&tmp.path().join(".stores"), "T999").unwrap_err();
        assert!(err.to_string().contains(
            "missing transcript for T999 phase 2 cycle 1 role code-reviewer"
        ));
    }
}
