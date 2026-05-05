//! Section-grouping query layer.
//!
//! Reads tasks + observations from `.stores/db.sqlite` (read-only) and
//! classifies each row into one of seven sections:
//!
//!   Tasks: NEEDS REVIEW · IN FLIGHT · DEPLOY BLOCKED · ACCEPTED
//!   Obs:   RATIFIABLE · OPEN-NO-CONTRACT · OTHER

use anyhow::Result;
use rusqlite::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    TasksNeedsReview,
    TasksInFlight,
    TasksDeployBlocked,
    TasksAccepted,
    ObsRatifiable,
    ObsOpenNoContract,
    ObsOther,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::TasksNeedsReview => "TASKS · NEEDS REVIEW",
            Section::TasksInFlight => "TASKS · IN FLIGHT",
            Section::TasksDeployBlocked => "TASKS · DEPLOY BLOCKED",
            Section::TasksAccepted => "TASKS · ACCEPTED",
            Section::ObsRatifiable => "OBS · RATIFIABLE",
            Section::ObsOpenNoContract => "OBS · OPEN-NO-CONTRACT",
            Section::ObsOther => "OBS · OTHER",
        }
    }

    pub const ALL: [Section; 7] = [
        Section::TasksNeedsReview,
        Section::TasksInFlight,
        Section::TasksDeployBlocked,
        Section::TasksAccepted,
        Section::ObsRatifiable,
        Section::ObsOpenNoContract,
        Section::ObsOther,
    ];
}

#[derive(Debug, Clone)]
pub enum Row {
    Task(TaskRow),
    Obs(ObsRow),
}

#[derive(Debug, Clone)]
pub struct TaskRow {
    pub display_id: String,
    pub status: String,
    pub title: String,
    pub claimed_by: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ObsRow {
    pub display_id: String,
    pub status: String,
    pub priority: String,
    pub summary: String,
    pub updated_at: String,
    /// `intent_contract.contract_state`, when present.
    pub contract_state: Option<String>,
}

impl Row {
    pub fn display_id(&self) -> &str {
        match self {
            Row::Task(t) => &t.display_id,
            Row::Obs(o) => &o.display_id,
        }
    }

    pub fn title_or_summary(&self) -> &str {
        match self {
            Row::Task(t) => &t.title,
            Row::Obs(o) => &o.summary,
        }
    }
}

/// Load tasks + observations from the db. Errors out only on hard sqlite
/// failures; per-row decode errors are skipped (matches `cli/watch.rs`).
pub fn load_rows(conn: &Connection) -> Result<Vec<Row>> {
    let mut rows = Vec::new();

    let mut stmt = conn.prepare(
        "SELECT display_id, status, COALESCE(title, ''), claimed_by, COALESCE(updated_at, '')
         FROM tasks",
    )?;
    let task_iter = stmt.query_map([], |r| {
        Ok(TaskRow {
            display_id: r.get(0)?,
            status: r.get(1)?,
            title: r.get(2)?,
            claimed_by: r.get(3)?,
            updated_at: r.get(4)?,
        })
    })?;
    for r in task_iter.flatten() {
        rows.push(Row::Task(r));
    }

    let mut stmt = conn.prepare(
        "SELECT display_id, status, COALESCE(priority, ''), COALESCE(summary, ''),
                COALESCE(updated_at, ''),
                json_extract(intent_contract, '$.contract_state')
         FROM observations",
    )?;
    let obs_iter = stmt.query_map([], |r| {
        Ok(ObsRow {
            display_id: r.get(0)?,
            status: r.get(1)?,
            priority: r.get(2)?,
            summary: r.get(3)?,
            updated_at: r.get(4)?,
            contract_state: r.get(5).ok(),
        })
    })?;
    for r in obs_iter.flatten() {
        rows.push(Row::Obs(r));
    }

    Ok(rows)
}

/// Classify each row into a section. Returns `[(Section, indices)]` in the
/// canonical section order; sections with no rows are still present (with
/// empty index lists) so the renderer can show `(0)` headers.
pub fn classify(rows: &[Row]) -> Vec<(Section, Vec<usize>)> {
    let mut buckets: Vec<(Section, Vec<usize>)> =
        Section::ALL.iter().map(|s| (*s, Vec::new())).collect();

    for (i, row) in rows.iter().enumerate() {
        let sec = section_for(row);
        let bucket = buckets
            .iter_mut()
            .find(|(s, _)| *s == sec)
            .expect("section_for returns a member of Section::ALL");
        bucket.1.push(i);
    }

    buckets
}

fn section_for(row: &Row) -> Section {
    match row {
        Row::Task(t) => match t.status.as_str() {
            // Review states — operator attention required.
            "plan_review" | "code_review" | "in_review" => Section::TasksNeedsReview,
            // Ship gate.
            "deploy_blocked" => Section::TasksDeployBlocked,
            // Terminal-success.
            "accepted" | "complete" | "cargo_installed" | "schema_migrated" => {
                Section::TasksAccepted
            }
            // Everything else (planning, ready, executing, blocked, rejected) is in-flight.
            _ => Section::TasksInFlight,
        },
        Row::Obs(o) => {
            if o.contract_state.as_deref() == Some("ready") {
                Section::ObsRatifiable
            } else if o.status == "open" && o.contract_state.is_none() {
                Section::ObsOpenNoContract
            } else {
                Section::ObsOther
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(status: &str) -> Row {
        Row::Task(TaskRow {
            display_id: format!("T-{status}"),
            status: status.to_string(),
            title: "t".to_string(),
            claimed_by: None,
            updated_at: String::new(),
        })
    }

    fn obs(status: &str, contract: Option<&str>) -> Row {
        Row::Obs(ObsRow {
            display_id: format!("L-{status}"),
            status: status.to_string(),
            priority: "normal".to_string(),
            summary: "s".to_string(),
            updated_at: String::new(),
            contract_state: contract.map(str::to_string),
        })
    }

    #[test]
    fn section_classification() {
        let rows = vec![
            task("plan_review"),         // TasksNeedsReview
            task("executing"),           // TasksInFlight
            task("deploy_blocked"),      // TasksDeployBlocked
            task("accepted"),            // TasksAccepted
            obs("open", Some("ready")),  // ObsRatifiable
            obs("open", None),           // ObsOpenNoContract
            obs("resolved", None),       // ObsOther
        ];
        let buckets = classify(&rows);
        // Seven sections, in canonical order.
        assert_eq!(buckets.len(), 7);
        let labels: Vec<Section> = buckets.iter().map(|(s, _)| *s).collect();
        assert_eq!(labels, Section::ALL.to_vec());

        // Each input row landed in the expected section (one row per bucket).
        for (i, (_, indices)) in buckets.iter().enumerate() {
            assert_eq!(indices, &vec![i], "section {} should hold row {}", i, i);
        }
    }

    #[test]
    fn task_status_mapping_is_exhaustive() {
        // Map every task status the schema declares onto a section.
        let mappings: &[(&str, Section)] = &[
            ("planning", Section::TasksInFlight),
            ("plan_review", Section::TasksNeedsReview),
            ("ready", Section::TasksInFlight),
            ("executing", Section::TasksInFlight),
            ("code_review", Section::TasksNeedsReview),
            ("blocked", Section::TasksInFlight),
            ("complete", Section::TasksAccepted),
            ("in_review", Section::TasksNeedsReview),
            ("accepted", Section::TasksAccepted),
            ("rejected", Section::TasksInFlight),
            ("deploy_blocked", Section::TasksDeployBlocked),
            ("cargo_installed", Section::TasksAccepted),
            ("schema_migrated", Section::TasksAccepted),
        ];
        for (status, expected) in mappings {
            let r = task(status);
            assert_eq!(section_for(&r), *expected, "task status {status}");
        }
    }
}
