//! Obs-drafting confirm-gate: parse the draft JSON the side-car wrote and
//! file the observation by shelling out to `stores observations add`.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::process::Command;

use crate::tui::app::ObsDraftConfirm;

/// Schema of the JSON the obs-drafting side-car writes to
/// `.stores/runs/sidecar/obs-draft-<session>.json`.
#[derive(Debug, Deserialize)]
pub struct DraftedObservation {
    pub summary: String,
    #[serde(default)]
    pub body: String,
    #[serde(default = "default_priority")]
    pub priority: String,
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default)]
    pub task_id: Option<String>,
}

fn default_priority() -> String {
    "normal".to_string()
}

fn default_source() -> String {
    "dev".to_string()
}

/// Parse the draft body and shell out to `stores observations add` to
/// create the observation. Honor-system: `--invoker ai_with_human`
/// (filing an obs is not a tier-A write, no token required).
pub fn file_observation(draft: &ObsDraftConfirm) -> Result<()> {
    let parsed: DraftedObservation =
        serde_json::from_str(&draft.body).context("obs-draft body is not valid JSON")?;

    let (captured_at, captured_week) = now_iso_and_week();

    let mut cmd = Command::new("stores");
    cmd.args([
        "observations",
        "add",
        "--invoker",
        "ai_with_human",
        "--summary",
        &parsed.summary,
        "--source",
        &parsed.source,
        "--priority",
        &parsed.priority,
        "--captured-at",
        &captured_at,
        "--captured-week",
        &captured_week,
        "--body",
        &parsed.body,
    ]);
    if let Some(tid) = &parsed.task_id {
        if !tid.is_empty() {
            cmd.args(["--task-id", tid]);
        }
    }
    let status = cmd.status().context("failed to exec stores observations add")?;
    if !status.success() {
        anyhow::bail!("stores observations add exited with {status}");
    }
    Ok(())
}

/// Best-effort ISO-8601 + week label derived from `SystemTime::now()`. We
/// emit a UTC clock string and a `wNN` ISO-week stamp; year math is simple
/// enough that we don't pull a calendar crate.
fn now_iso_and_week() -> (String, String) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = epoch_to_ymdhms(secs);
    let iso = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z");
    let week = iso_week_of(y, mo, d);
    (iso, format!("w{week:02}"))
}

fn epoch_to_ymdhms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let s = (secs % 60) as u32;
    let mi = ((secs / 60) % 60) as u32;
    let h = ((secs / 3600) % 24) as u32;
    let days = (secs / 86400) as i64;
    let (y, mo, d) = days_to_ymd(days);
    (y, mo, d, h, mi, s)
}

/// Convert days-since-epoch (1970-01-01) to (y, m, d).
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let mut y = 1970i32;
    let mut remaining = days;
    loop {
        let dy = if is_leap(y) { 366 } else { 365 };
        if remaining < dy {
            break;
        }
        remaining -= dy;
        y += 1;
    }
    let dm = month_lengths(y);
    let mut m = 0;
    while m < 12 && remaining >= dm[m] as i64 {
        remaining -= dm[m] as i64;
        m += 1;
    }
    (y, (m + 1) as u32, (remaining + 1) as u32)
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

fn month_lengths(y: i32) -> [u32; 12] {
    let feb = if is_leap(y) { 29 } else { 28 };
    [31, feb, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
}

/// ISO 8601 week number for a given Y-M-D. Approximate: returns the week
/// of year using the simple "ordinal // 7 + 1" rule. Good enough for the
/// captured_week label which only needs ordering, not strict ISO semantics.
fn iso_week_of(y: i32, mo: u32, d: u32) -> u32 {
    let lengths = month_lengths(y);
    let mut ordinal: u32 = d;
    for len in lengths.iter().take((mo as usize).saturating_sub(1)) {
        ordinal += len;
    }
    ((ordinal - 1) / 7) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drafted_observation_parses_minimal() {
        let json = r#"{"summary":"x"}"#;
        let p: DraftedObservation = serde_json::from_str(json).unwrap();
        assert_eq!(p.summary, "x");
        assert_eq!(p.priority, "normal");
        assert_eq!(p.source, "dev");
    }

    #[test]
    fn drafted_observation_parses_full() {
        let json = r#"{"summary":"a","body":"b","priority":"high","source":"plan","task_id":"T7"}"#;
        let p: DraftedObservation = serde_json::from_str(json).unwrap();
        assert_eq!(p.priority, "high");
        assert_eq!(p.task_id.as_deref(), Some("T7"));
    }
}
