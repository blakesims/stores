use anyhow::{bail, Context, Result};
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
    List {
        display_id: String,
    },
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
            let rows = list_for_task(&crate::paths::stores_dir()?, &display_id)?;
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
            let row = find_transcript(
                &crate::paths::stores_dir()?,
                &display_id,
                phase,
                cycle,
                &role,
            )?;
            let body = fs::read_to_string(&row.path)
                .with_context(|| format!("failed to read transcript {}", row.path.display()))?;
            println!("{body}");
            Ok(())
        }
    }
}

pub fn list_for_task(stores_dir: &Path, display_id: &str) -> Result<Vec<RunTranscript>> {
    let task_dir = stores_dir.join("runs").join(display_id);
    if !task_dir.exists() {
        bail!(
            "no transcripts found for {display_id}: {} does not exist",
            task_dir.display()
        );
    }
    if !task_dir.is_dir() {
        bail!(
            "runs path for {display_id} is not a directory: {}",
            task_dir.display()
        );
    }

    let mut rows = Vec::new();
    for entry in fs::read_dir(&task_dir)
        .with_context(|| format!("failed to read runs directory {}", task_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        rows.push(row_from_path(display_id, &path)?);
    }

    if rows.is_empty() {
        bail!(
            "no transcript JSON files found for {display_id} in {}",
            task_dir.display()
        );
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

fn row_from_path(display_id: &str, path: &Path) -> Result<RunTranscript> {
    let body = fs::read_to_string(path)
        .with_context(|| format!("failed to read transcript {}", path.display()))?;
    let json: serde_json::Value = serde_json::from_str(&body)
        .with_context(|| format!("transcript is not valid JSON: {}", path.display()))?;

    let phase = first_i64(&json, &["phase", "phase_number", "current_phase"]).unwrap_or(1);
    let cycle = first_i64(&json, &["cycle", "cycle_number", "current_cycle"]).unwrap_or(1);
    let role = first_str(&json, &["role", "agent_role", "agent"])
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

    Ok(RunTranscript {
        display_id: display_id.to_string(),
        phase,
        cycle,
        role,
        path: path.to_path_buf(),
    })
}

fn first_i64(v: &serde_json::Value, keys: &[&str]) -> Option<i64> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_i64()))
}

fn first_str<'a>(v: &'a serde_json::Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".stores/runs/T999");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("executor.json"),
            r#"{"role":"executor","phase":2,"cycle":1,"summary":"done"}"#,
        )
        .unwrap();
        fs::write(
            dir.join("code-reviewer.json"),
            r#"{"role":"code-reviewer","phase_number":2,"cycle_number":1,"gate":"PASS"}"#,
        )
        .unwrap();
        fs::write(dir.join("notes.txt"), "ignored").unwrap();
        tmp
    }

    #[test]
    fn list_outputs_deterministic_order() {
        let tmp = fixture();
        let rows = list_for_task(&tmp.path().join(".stores"), "T999").unwrap();
        let keys: Vec<_> = rows
            .iter()
            .map(|r| (r.phase, r.cycle, r.role.as_str()))
            .collect();
        assert_eq!(keys, vec![(2, 1, "code-reviewer"), (2, 1, "executor")]);
    }

    #[test]
    fn show_finds_existing_transcript() {
        let tmp = fixture();
        let row =
            find_transcript(&tmp.path().join(".stores"), "T999", 2, None, "executor").unwrap();
        assert_eq!(
            row.path.file_name().and_then(|s| s.to_str()),
            Some("executor.json")
        );
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
}
