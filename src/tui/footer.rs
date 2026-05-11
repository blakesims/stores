//! Selected-row footer: 1 line showing objective, status, tier, linked obs,
//! and last-update relative time.

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::time::{SystemTime, UNIX_EPOCH};

use super::app::App;
use super::data::Row;

/// Render the selected-row footer into `area`. When no row is selected,
/// renders a row-count placeholder.
pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let line = build_line(app);
    f.render_widget(
        Paragraph::new(line).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn build_line(app: &App) -> Line<'static> {
    let text = render_text(app, now_epoch_secs());
    Line::from(Span::raw(text))
}

/// Build the footer text against a fixed `now` (seconds since the epoch).
/// Snapshot tests pass a deterministic `now` to avoid wall-clock flakiness.
pub fn render_text(app: &App, now_secs: u64) -> String {
    let flat = app.flat_rows();
    if flat.is_empty() {
        return "no rows".to_string();
    }
    let cursor = match app.current_flat() {
        Some(c) => c,
        None => return format!("{} row(s) loaded", flat.len()),
    };
    let fr = flat[cursor];
    let row = &app.rows[fr.abs];
    format_text(row, cursor, flat.len(), now_secs)
}

fn format_text(row: &Row, cursor: usize, total: usize, now_secs: u64) -> String {
    let id = row.display_id();
    let summary = row.title_or_summary();
    // ADR 0002 compatibility-only T148 task 6.1: upstream raw status is a footer label only.
    let (status, tier, linked, updated_at) = match row {
        Row::Task(t) => (
            t.status.as_str(),
            t.tier_hint.as_deref().unwrap_or("?"),
            t.linked_observations.join(","),
            t.updated_at.as_str(),
        ),
        Row::Obs(o) => (
            o.status.as_str(),
            o.tier_hint.as_deref().unwrap_or("?"),
            String::new(),
            o.updated_at.as_str(),
        ),
        Row::CollapsedObs(c) => (
            c.representative.status.as_str(),
            c.representative.tier_hint.as_deref().unwrap_or("?"),
            format!("×{}", c.count),
            c.representative.updated_at.as_str(),
        ),
        Row::Review(r) => (
            r.status.as_str(),
            "review",
            format!("task:{} runner:{}", r.task_id, r.runner),
            r.next_retry_at.as_deref().unwrap_or(""),
        ),
        Row::Intake(i) => (
            i.status.as_str(),
            "?",
            String::new(),
            i.updated_at.as_str(),
        ),
    };
    let rel = relative_time(updated_at, now_secs);
    let linked_seg = if linked.is_empty() {
        String::new()
    } else {
        format!(" linked:{linked}")
    };
    // ADR 0002 compatibility-only T148 task 6.1: legacy investigation_failed label detail.
    let failure_seg = match row {
        Row::Obs(o) if o.status == "investigation_failed" => {
            let reason = o
                .investigation_failure_reason
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .unwrap_or("unknown");
            format!(" failure:{reason}")
        }
        Row::CollapsedObs(c) if c.representative.status == "investigation_failed" => {
            let reason = c
                .representative
                .investigation_failure_reason
                .as_deref()
                .map(str::trim)
                .filter(|r| !r.is_empty())
                .unwrap_or("unknown");
            format!(" failure:{reason}")
        }
        _ => String::new(),
    };
    format!(
        "[{cur}/{total}] {id} {summary} · status:{status} tier:{tier}{linked_seg}{failure_seg} · {rel}",
        cur = cursor + 1,
    )
}

/// Format a relative-time delta like `2m ago`, `3h ago`, `4d ago`. Empty /
/// unparseable timestamps render as `never`.
pub fn relative_time(ts: &str, now_secs: u64) -> String {
    let then = match iso8601_to_epoch_secs(ts) {
        Some(t) => t,
        None => return "never".to_string(),
    };
    let now_i = now_secs as i64;
    let diff = (now_i - then as i64).max(0);
    if diff < 60 {
        format!("{diff}s ago")
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else {
        format!("{}d ago", diff / 86400)
    }
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse an `YYYY-MM-DDTHH:MM:SSZ` timestamp into epoch seconds. Returns None
/// for any other shape (including empty).
pub fn iso8601_to_epoch_secs(ts: &str) -> Option<u64> {
    if ts.len() < 19 {
        return None;
    }
    let b = ts.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let y: u32 = ts.get(0..4)?.parse().ok()?;
    let mo: u32 = ts.get(5..7)?.parse().ok()?;
    let d: u32 = ts.get(8..10)?.parse().ok()?;
    let h: u32 = ts.get(11..13)?.parse().ok()?;
    let mi: u32 = ts.get(14..16)?.parse().ok()?;
    let s: u32 = ts.get(17..19)?.parse().ok()?;
    if !(1970..=9999).contains(&y) || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let days = ymd_to_days(y, mo, d)?;
    Some(days * 86400 + (h as u64) * 3600 + (mi as u64) * 60 + (s as u64))
}

fn ymd_to_days(y: u32, mo: u32, d: u32) -> Option<u64> {
    let mut days: u64 = 0;
    for year in 1970..y {
        days += days_in_year(year) as u64;
    }
    let dim = days_in_month(y, mo);
    if d == 0 || d > dim {
        return None;
    }
    for month in 1..mo {
        days += days_in_month(y, month) as u64;
    }
    days += (d - 1) as u64;
    Some(days)
}

fn is_leap(y: u32) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}

fn days_in_year(y: u32) -> u32 {
    if is_leap(y) {
        366
    } else {
        365
    }
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, Selection, TuiOpts};
    use crate::tui::data::{classify, Row, TaskRow};

    #[test]
    fn iso8601_round_trip() {
        // 2026-05-05T12:00:00Z is reproducible: count days from 1970-01-01.
        let secs = iso8601_to_epoch_secs("2026-05-05T12:00:00Z").unwrap();
        // 1970→2026 is 56 years; just sanity-check ordering and minute math.
        let earlier = iso8601_to_epoch_secs("2026-05-05T11:58:30Z").unwrap();
        assert_eq!(secs - earlier, 90);
    }

    #[test]
    fn relative_time_buckets() {
        let now = iso8601_to_epoch_secs("2026-05-05T12:00:00Z").unwrap();
        assert_eq!(relative_time("2026-05-05T11:58:30Z", now), "1m ago");
        assert_eq!(relative_time("2026-05-05T09:00:00Z", now), "3h ago");
        assert_eq!(relative_time("2026-05-02T12:00:00Z", now), "3d ago");
        assert_eq!(relative_time("", now), "never");
        assert_eq!(relative_time("not-a-time", now), "never");
    }

    #[test]
    fn snapshot_canonical_row() {
        // AC3.2: a canonical task row renders objective/status/tier/linked-obs/relative-time.
        let mut app = App::new(TuiOpts::default());
        app.rows = vec![Row::Task(TaskRow {
            display_id: "T028".to_string(),
            status: "executing".to_string(),
            title: "watch becomes a TUI".to_string(),
            claimed_by: None,
            updated_at: "2026-05-05T11:55:00Z".to_string(),
            tier_hint: Some("T3".to_string()),
            linked_observations: vec!["L075".to_string()],
            ..Default::default()
        })];
        app.sections = classify(&app.rows);
        app.apply_sort();
        // Land selection on the only row.
        let flat = app.flat_rows();
        assert_eq!(flat.len(), 1);
        app.selection = Selection {
            section: flat[0].section,
            row: flat[0].row,
        };
        let now = iso8601_to_epoch_secs("2026-05-05T12:00:00Z").unwrap();
        let text = render_text(&app, now);
        assert_eq!(
            text,
            "[1/1] T028 watch becomes a TUI · status:executing tier:T3 linked:L075 · 5m ago"
        );
    }
}
