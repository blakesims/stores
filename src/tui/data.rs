//! Section-grouping query layer.
//!
//! Reads tasks + observations from `.stores/db.sqlite` (read-only) and
//! classifies each row into actionable watch sections:
//!
//!   Tasks: ACTIONABLE CURRENT WORK · BLOCKED NEEDS ACTION · DEPLOY RECOVERY · RECENTLY TERMINAL
//!   Obs:   RATIFIABLE · OPEN-NO-CONTRACT · OTHER

use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const SECS_PER_DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    TasksActionableCurrentWork,
    TasksBlockedNeedsAction,
    TasksDeployRecovery,
    TasksNeedsTriage,
    TasksRecentlyTerminal,
    ObsRatifiable,
    ObsOpenNoContract,
    ObsOther,
}

impl Section {
    pub fn label(self) -> &'static str {
        match self {
            Section::TasksActionableCurrentWork => "TASKS · ACTIONABLE CURRENT WORK",
            Section::TasksBlockedNeedsAction => "TASKS · BLOCKED NEEDS ACTION",
            Section::TasksDeployRecovery => "TASKS · DEPLOY RECOVERY",
            Section::TasksNeedsTriage => "TASKS · NEEDS TRIAGE",
            Section::TasksRecentlyTerminal => "TASKS · RECENTLY TERMINAL",
            Section::ObsRatifiable => "OBS · RATIFIABLE",
            Section::ObsOpenNoContract => "OBS · OPEN-NO-CONTRACT",
            Section::ObsOther => "OBS · OTHER",
        }
    }

    pub const ALL: [Section; 8] = [
        Section::TasksActionableCurrentWork,
        Section::TasksBlockedNeedsAction,
        Section::TasksDeployRecovery,
        Section::TasksNeedsTriage,
        Section::TasksRecentlyTerminal,
        Section::ObsRatifiable,
        Section::ObsOpenNoContract,
        Section::ObsOther,
    ];
}

/// Watch task classification knobs. Default hides stale terminal exhaust
/// older than 48 hours and caps recent terminal rows to the 5 newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchClassifyOptions {
    pub show_all_history: bool,
    pub recent_terminal_days: u64,
    pub recent_terminal_limit: usize,
}

impl Default for WatchClassifyOptions {
    fn default() -> Self {
        Self {
            show_all_history: false,
            recent_terminal_days: 2,
            recent_terminal_limit: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Row {
    Task(TaskRow),
    Obs(ObsRow),
}

#[derive(Debug, Clone, Default)]
pub struct TaskRow {
    pub display_id: String,
    pub status: String,
    pub title: String,
    pub claimed_by: Option<String>,
    pub updated_at: String,
    pub tier_hint: Option<String>,
    pub linked_observations: Vec<String>,
    pub blocked_reason: Option<String>,
    pub blocked_reason_class: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ObsRow {
    pub display_id: String,
    pub status: String,
    pub priority: String,
    pub summary: String,
    pub updated_at: String,
    /// `intent_contract.contract_state`, when present.
    pub contract_state: Option<String>,
    pub tier_hint: Option<String>,
    pub investigation_failure_reason: Option<String>,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VisibilityClass {
    ActionableRecovery,
    HistoricalNoise,
    NeedsTriage,
}

impl VisibilityClass {
    pub fn as_str(self) -> &'static str {
        match self {
            VisibilityClass::ActionableRecovery => "actionable_recovery",
            VisibilityClass::HistoricalNoise => "historical_noise",
            VisibilityClass::NeedsTriage => "needs_triage",
        }
    }
}

pub fn row_visibility_class(row: &Row, task_status_by_id: &HashMap<String, String>) -> VisibilityClass {
    match row {
        Row::Task(t) => task_visibility_class(t),
        Row::Obs(o) => obs_visibility_class(o, task_status_by_id),
    }
}

pub fn task_visibility_class(t: &TaskRow) -> VisibilityClass {
    if is_in_flight_task_status(&t.status) {
        return VisibilityClass::ActionableRecovery;
    }
    let reason = t.blocked_reason.as_deref().unwrap_or("").to_ascii_lowercase();
    let reason_class = blocked_reason_class(t.blocked_reason.as_deref());
    if reason.starts_with("silent_zombie")
        || reason.starts_with("drive_failed:silent_zombie")
    {
        return VisibilityClass::HistoricalNoise;
    }
    if reason.contains("accept_installed_inert")
        && matches!(t.status.as_str(), "cargo_installed" | "schema_migrated" | "accepted" | "complete" | "closed_out_of_band" | "abandoned")
    {
        return VisibilityClass::HistoricalNoise;
    }
    if t.status == "deploy_blocked" {
        if is_recoverable_deploy_reason(&reason) {
            return VisibilityClass::ActionableRecovery;
        }
        return VisibilityClass::NeedsTriage;
    }
    if t.status == "blocked" {
        return match reason_class {
            "rate_limit" | "retry" | "dependency" | "user" | "deploy" => VisibilityClass::ActionableRecovery,
            "unknown" => VisibilityClass::NeedsTriage,
            _ => VisibilityClass::ActionableRecovery,
        };
    }
    VisibilityClass::ActionableRecovery
}

pub fn obs_visibility_class(o: &ObsRow, task_status_by_id: &HashMap<String, String>) -> VisibilityClass {
    let lower = o.summary.to_ascii_lowercase();
    if lower.starts_with("deploy-blocked:") {
        if let Some(task_id) = extract_task_id(&o.summary) {
            if task_status_by_id
                .get(&task_id)
                .map(|s| is_terminal_task_status(s))
                .unwrap_or(false)
            {
                return VisibilityClass::HistoricalNoise;
            }
        }
        if lower.contains("retry-deploy-recoverable") || lower.contains("recoverable") {
            return VisibilityClass::ActionableRecovery;
        }
        return VisibilityClass::NeedsTriage;
    }
    VisibilityClass::ActionableRecovery
}

fn is_recoverable_deploy_reason(reason: &str) -> bool {
    reason.contains("retry-deploy-recoverable") || reason.contains("recoverable")
}

fn extract_task_id(s: &str) -> Option<String> {
    for word in s.split(|c: char| !c.is_ascii_alphanumeric()) {
        if word.len() >= 2 && word.starts_with('T') && word[1..].chars().all(|c| c.is_ascii_digit()) {
            return Some(word.to_string());
        }
    }
    None
}

pub fn is_in_flight_task_status(s: &str) -> bool {
    matches!(s, "executing" | "plan_review" | "code_review" | "in_review" | "planning" | "ready")
}

pub fn surface_counts(rows: &[Row], _show_all_history: bool) -> ((usize, usize), (usize, usize)) {
    let ctx = task_status_by_id(rows);
    let mut task = (0, 0);
    let mut obs = (0, 0);
    for row in rows {
        let class = row_visibility_class(row, &ctx);
        match row {
            Row::Task(_) => {
                task.1 += 1;
                if class == VisibilityClass::ActionableRecovery {
                    task.0 += 1;
                }
            }
            Row::Obs(_) => {
                obs.1 += 1;
                if class == VisibilityClass::ActionableRecovery {
                    obs.0 += 1;
                }
            }
        }
    }
    (task, obs)
}

fn task_status_by_id(rows: &[Row]) -> HashMap<String, String> {
    rows.iter().filter_map(|r| match r {
        Row::Task(t) => Some((t.display_id.clone(), t.status.clone())),
        Row::Obs(_) => None,
    }).collect()
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
        "SELECT display_id, status, COALESCE(title, ''), claimed_by, COALESCE(updated_at, ''),
                tier_hint, COALESCE(linked_observations, '[]'), blocked_reason
         FROM tasks",
    )?;
    let task_iter = stmt.query_map([], |r| {
        let linked_raw: String = r.get(6)?;
        let blocked_reason: Option<String> = r.get(7).ok();
        let blocked_reason_class = blocked_reason_class(blocked_reason.as_deref()).to_string();
        Ok(TaskRow {
            display_id: r.get(0)?,
            status: r.get(1)?,
            title: r.get(2)?,
            claimed_by: r.get(3)?,
            updated_at: r.get(4)?,
            tier_hint: r.get(5).ok(),
            linked_observations: serde_json::from_str(&linked_raw).unwrap_or_default(),
            blocked_reason,
            blocked_reason_class: Some(blocked_reason_class),
        })
    })?;
    for r in task_iter.flatten() {
        rows.push(Row::Task(r));
    }

    let mut stmt = conn.prepare(
        "SELECT display_id, status, COALESCE(priority, ''), COALESCE(summary, ''),
                COALESCE(updated_at, ''),
                json_extract(intent_contract, '$.contract_state'),
                json_extract(intent_contract, '$.tier_hint'),
                investigation_failure_reason
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
            tier_hint: r.get(6).ok(),
            investigation_failure_reason: r.get(7).ok(),
        })
    })?;
    for r in obs_iter.flatten() {
        rows.push(Row::Obs(r));
    }

    Ok(rows)
}

/// Classify each row into a section using default watch options.
pub fn classify(rows: &[Row]) -> Vec<(Section, Vec<usize>)> {
    classify_with_options(rows, WatchClassifyOptions::default())
}

/// Classify each row into a section. Returns `[(Section, indices)]` in the
/// canonical section order; sections with no rows are still present.
pub fn classify_with_options(
    rows: &[Row],
    opts: WatchClassifyOptions,
) -> Vec<(Section, Vec<usize>)> {
    classify_with_options_at(rows, opts, now_epoch())
}

fn classify_with_options_at(
    rows: &[Row],
    opts: WatchClassifyOptions,
    now: i64,
) -> Vec<(Section, Vec<usize>)> {
    let mut buckets: Vec<(Section, Vec<usize>)> =
        Section::ALL.iter().map(|s| (*s, Vec::new())).collect();
    let mut terminal = Vec::new();
    let task_ctx = task_status_by_id(rows);

    for (i, row) in rows.iter().enumerate() {
        if !opts.show_all_history && row_visibility_class(row, &task_ctx) == VisibilityClass::HistoricalNoise {
            continue;
        }
        match section_for(row) {
            Some(Section::TasksRecentlyTerminal) => terminal.push(i),
            Some(sec) => push_bucket(&mut buckets, sec, i),
            None => {}
        }
    }

    terminal.sort_by(|a, b| {
        let ta = task_updated_epoch(&rows[*a]).unwrap_or(i64::MIN);
        let tb = task_updated_epoch(&rows[*b]).unwrap_or(i64::MIN);
        tb.cmp(&ta)
            .then_with(|| rows[*a].display_id().cmp(rows[*b].display_id()))
    });

    let _ = now;
    for idx in terminal {
        push_bucket(&mut buckets, Section::TasksRecentlyTerminal, idx);
    }

    buckets
}

fn push_bucket(buckets: &mut [(Section, Vec<usize>)], sec: Section, idx: usize) {
    let bucket = buckets
        .iter_mut()
        .find(|(s, _)| *s == sec)
        .expect("section_for returns a member of Section::ALL");
    bucket.1.push(idx);
}

fn section_for(row: &Row) -> Option<Section> {
    match row {
        Row::Task(t) => match t.status.as_str() {
            "planning" | "ready" | "executing" | "plan_review" | "code_review" | "in_review" => {
                Some(Section::TasksActionableCurrentWork)
            }
            "blocked" => {
                if task_visibility_class(t) == VisibilityClass::NeedsTriage {
                    Some(Section::TasksNeedsTriage)
                } else {
                    Some(Section::TasksBlockedNeedsAction)
                }
            }
            "deploy_blocked" => {
                if task_visibility_class(t) == VisibilityClass::NeedsTriage {
                    Some(Section::TasksNeedsTriage)
                } else {
                    Some(Section::TasksDeployRecovery)
                }
            }
            "closed_out_of_band" | "accepted" | "complete" | "cargo_installed"
            | "schema_migrated" | "rejected" | "abandoned" => Some(Section::TasksRecentlyTerminal),
            _ => Some(Section::TasksActionableCurrentWork),
        },
        Row::Obs(o) => {
            if o.contract_state.as_deref() == Some("ready") {
                Some(Section::ObsRatifiable)
            } else if o.status == "open" && o.contract_state.is_none() {
                Some(Section::ObsOpenNoContract)
            } else {
                Some(Section::ObsOther)
            }
        }
    }
}

pub fn is_terminal_task_status(s: &str) -> bool {
    matches!(
        s,
        "closed_out_of_band"
            | "accepted"
            | "complete"
            | "cargo_installed"
            | "schema_migrated"
            | "rejected"
            | "abandoned"
    )
}

pub fn blocked_reason_class(reason: Option<&str>) -> &'static str {
    let raw = reason.unwrap_or("").trim();
    if raw.is_empty() {
        return "unknown";
    }
    // Codex T059-r1 MEDIUM: real blocked_reason values are often structured
    // JSON like `{"exit_code":1,"kind":"rate_limit","reset_at":...}` written
    // by the drive runner. Parse the `kind` first; fall back to substring
    // heuristics on the raw text only if the JSON path doesn't yield one of
    // the known classes.
    if raw.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(kind) = v.get("kind").and_then(|k| k.as_str()) {
                match kind {
                    "rate_limit" => return "rate_limit",
                    "retry" => return "retry",
                    "dependency" => return "dependency",
                    "user" => return "user",
                    "deploy" => return "deploy",
                    "stale" => return "stale",
                    _ => {} // unknown kind → fall through to heuristics below
                }
            }
        }
    }
    let r = raw.to_lowercase();
    if r.contains("rate limit")
        || r.contains("ratelimit")
        || r.contains("429")
        || r.contains("rate_limit")
    {
        "rate_limit"
    } else if r.contains("retry") || r.contains("again") || r.contains("transient") {
        "retry"
    } else if r.contains("depend") || r.contains("blocked by") || r.contains("waiting on") {
        "dependency"
    } else if r.contains("user")
        || r.contains("human")
        || r.contains("approval")
        || r.contains("approve")
    {
        "user"
    } else if r.contains("deploy") || r.contains("release") || r.contains("production") {
        "deploy"
    } else if r.contains("stale")
        || r.contains("old")
        || r.contains("timeout")
        || r.contains("timed out")
    {
        "stale"
    } else {
        "unknown"
    }
}

fn task_updated_epoch(row: &Row) -> Option<i64> {
    match row {
        Row::Task(t) => parse_epoch(&t.updated_at),
        Row::Obs(_) => None,
    }
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_epoch(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(v) = s.parse::<i64>() {
        return Some(v);
    }
    let date = s.get(0..10)?;
    let mut parts = date.split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    let (hh, mm, ss) = if let Some(time) = s.get(11..19) {
        let mut t = time.split(':');
        (
            t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
            t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
            t.next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
        )
    } else {
        (0, 0, 0)
    };
    let days = days_from_civil(y, m, d)?;
    Some(days * SECS_PER_DAY + hh * 3600 + mm * 60 + ss)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = y - (m <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = m as i32 + if m > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + d as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some((era * 146097 + doe - 719468) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    fn task(status: &str) -> Row {
        task_at(status, NOW.to_string(), None)
    }

    fn task_at(status: &str, updated_at: impl Into<String>, blocked_reason: Option<&str>) -> Row {
        let updated_at = updated_at.into();
        Row::Task(TaskRow {
            display_id: format!("T-{status}-{updated_at}"),
            status: status.to_string(),
            title: "t".to_string(),
            claimed_by: None,
            updated_at,
            tier_hint: None,
            linked_observations: Vec::new(),
            blocked_reason: blocked_reason.map(str::to_string),
            blocked_reason_class: Some(blocked_reason_class(blocked_reason).to_string()),
        })
    }

    fn task_with_id(id: &str, status: &str, updated_at: i64) -> Row {
        Row::Task(TaskRow {
            display_id: id.to_string(),
            status: status.to_string(),
            title: "t".to_string(),
            updated_at: updated_at.to_string(),
            blocked_reason_class: Some("unknown".to_string()),
            ..Default::default()
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
            tier_hint: None,
            investigation_failure_reason: None,
        })
    }

    fn bucket(buckets: &[(Section, Vec<usize>)], section: Section) -> Vec<usize> {
        buckets
            .iter()
            .find(|(s, _)| *s == section)
            .unwrap()
            .1
            .clone()
    }

    #[test]
    fn section_classification() {
        // blocked/deploy_blocked with no blocked_reason → unknown class → NeedsTriage section.
        let rows = vec![
            task("plan_review"),      // idx 0 → TasksActionableCurrentWork
            task("blocked"),          // idx 1 → TasksNeedsTriage (no reason → unknown)
            task("deploy_blocked"),   // idx 2 → TasksNeedsTriage (no reason → unknown)
            task("accepted"),         // idx 3 → TasksRecentlyTerminal
            obs("open", Some("ready")), // idx 4 → ObsRatifiable
            obs("open", None),        // idx 5 → ObsOpenNoContract
            obs("resolved", None),    // idx 6 → ObsOther
        ];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert_eq!(buckets.len(), 8);
        assert_eq!(
            buckets.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            Section::ALL.to_vec()
        );
        let b = |sec: Section| -> Vec<usize> {
            buckets.iter().find(|(s, _)| *s == sec).unwrap().1.clone()
        };
        assert_eq!(b(Section::TasksActionableCurrentWork), vec![0usize]);
        assert_eq!(b(Section::TasksBlockedNeedsAction), Vec::<usize>::new());
        assert_eq!(b(Section::TasksDeployRecovery), Vec::<usize>::new());
        assert_eq!(b(Section::TasksNeedsTriage), vec![1usize, 2]);
        assert_eq!(b(Section::TasksRecentlyTerminal), vec![3usize]);
        assert_eq!(b(Section::ObsRatifiable), vec![4usize]);
        assert_eq!(b(Section::ObsOpenNoContract), vec![5usize]);
        assert_eq!(b(Section::ObsOther), vec![6usize]);
    }

    #[test]
    fn task_status_mapping_is_exhaustive() {
        // blocked/deploy_blocked with no reason → unknown class → TasksNeedsTriage.
        // Use an explicit recoverable reason to get TasksBlockedNeedsAction / TasksDeployRecovery.
        let mappings: &[(&str, Option<&str>, Section)] = &[
            ("planning", None, Section::TasksActionableCurrentWork),
            ("plan_review", None, Section::TasksActionableCurrentWork),
            ("ready", None, Section::TasksActionableCurrentWork),
            ("executing", None, Section::TasksActionableCurrentWork),
            ("code_review", None, Section::TasksActionableCurrentWork),
            ("blocked", Some("rate_limit 429"), Section::TasksBlockedNeedsAction),
            ("blocked", None, Section::TasksNeedsTriage),
            ("complete", None, Section::TasksRecentlyTerminal),
            ("in_review", None, Section::TasksActionableCurrentWork),
            ("accepted", None, Section::TasksRecentlyTerminal),
            ("rejected", None, Section::TasksRecentlyTerminal),
            ("deploy_blocked", Some("retry-deploy-recoverable"), Section::TasksDeployRecovery),
            ("deploy_blocked", None, Section::TasksNeedsTriage),
            ("closed_out_of_band", None, Section::TasksRecentlyTerminal),
            ("cargo_installed", None, Section::TasksRecentlyTerminal),
            ("schema_migrated", None, Section::TasksRecentlyTerminal),
            ("abandoned", None, Section::TasksRecentlyTerminal),
        ];
        for (status, reason, expected) in mappings {
            let r = task_at(status, NOW.to_string(), *reason);
            assert_eq!(section_for(&r), Some(*expected), "task status {status} reason {reason:?}");
        }
    }

    #[test]
    fn rejected_not_actionable_and_closed_out_of_band_terminal() {
        let rows = vec![task("rejected"), task("closed_out_of_band")];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert!(bucket(&buckets, Section::TasksActionableCurrentWork).is_empty());
        assert_eq!(bucket(&buckets, Section::TasksRecentlyTerminal).len(), 2);
        assert!(bucket(&buckets, Section::TasksRecentlyTerminal).contains(&0));
        assert!(bucket(&buckets, Section::TasksRecentlyTerminal).contains(&1));
    }

    #[test]
    fn terminal_rows_remain_visible_unless_historical_noise() {
        let old = NOW - 49 * 3600;
        let rows = vec![
            task_with_id("T-schema", "schema_migrated", old),
            task_with_id("T-closed", "closed_out_of_band", old),
            task_with_id("T-rejected", "rejected", old),
            task_with_id("T-accepted", "accepted", old),
            task_with_id("T-complete", "complete", old),
            task_with_id("T-cargo", "cargo_installed", old),
        ];
        let buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        for sec in [
            Section::TasksActionableCurrentWork,
            Section::TasksBlockedNeedsAction,
            Section::TasksDeployRecovery,
            Section::TasksNeedsTriage,
            Section::TasksRecentlyTerminal,
        ] {
            assert_eq!(bucket(&buckets, sec).len(), if sec == Section::TasksRecentlyTerminal { 6 } else { 0 }, "{sec:?} visibility");
        }
    }

    #[test]
    fn terminal_rows_are_uncapped_by_default_and_all_history_matches() {
        let rows: Vec<Row> = (0..7)
            .map(|i| task_with_id(&format!("T{i}"), "accepted", NOW - i * 60))
            .collect();
        let default_buckets = classify_with_options_at(&rows, WatchClassifyOptions::default(), NOW);
        assert_eq!(
            bucket(&default_buckets, Section::TasksRecentlyTerminal),
            vec![0, 1, 2, 3, 4, 5, 6]
        );

        let all_buckets = classify_with_options_at(
            &rows,
            WatchClassifyOptions {
                show_all_history: true,
                ..Default::default()
            },
            NOW,
        );
        assert_eq!(
            bucket(&all_buckets, Section::TasksRecentlyTerminal),
            vec![0, 1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn blocked_reason_classes_cover_first_pass_examples() {
        let cases = [
            (Some("hit rate limit 429"), "rate_limit"),
            (Some("retry after transient failure"), "retry"),
            (Some("waiting on dependency T123"), "dependency"),
            (Some("needs human approval"), "user"),
            (Some("deploy window closed"), "deploy"),
            (Some("stale lock timed out"), "stale"),
            (Some(""), "unknown"),
            (Some("opaque"), "unknown"),
            (None, "unknown"),
        ];
        for (reason, expected) in cases {
            assert_eq!(blocked_reason_class(reason), expected, "{reason:?}");
        }
    }

    #[test]
    fn parse_epoch_accepts_rfc3339_prefix() {
        assert_eq!(parse_epoch("1970-01-01T00:00:00Z"), Some(0));
    }
}
