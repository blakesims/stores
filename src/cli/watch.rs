//! `stores watch` — POC live dashboard for the substrate.
//!
//! Polls `.stores/db.sqlite` on an interval and renders a refreshing
//! terminal frame: tasks grouped by status, recent observations, and a
//! footer with the timestamp + DB path. No external deps; ANSI escapes
//! only. Press Ctrl-C to exit.
//!
//! This is a quick proof of concept, not a production tool. Known
//! shortcomings (file as observations if we keep it):
//!  - Hardcoded to the `tasks` and `observations` stores; doesn't iterate
//!    the manifest. A real implementation would render any store with a
//!    title-like field.
//!  - No `--workspace` flag for cross-worktree aggregation.
//!  - Polls (1 Hz default); doesn't use `sqlite3_update_hook`. Push would
//!    be needed for sound-on-event ergonomics.
//!  - No interactive features (filter, sort, drill-in).

use anyhow::{bail, Result};
use rusqlite::{Connection, OpenFlags};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::paths::db_path;

const ANSI_CLEAR_HOME: &str = "\x1b[2J\x1b[H";
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_BOLD: &str = "\x1b[1m";
const ANSI_DIM: &str = "\x1b[2m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_MAGENTA: &str = "\x1b[35m";

/// Run the watch loop. Blocks until Ctrl-C.
pub fn run(interval_ms: u64) -> Result<()> {
    let db = db_path()?;
    if !db.exists() {
        bail!(
            ".stores/db.sqlite not found in '{}'; run `stores init` first",
            std::env::current_dir()?.display()
        );
    }

    // Read-only connection — we never mutate during a watch.
    let conn = Connection::open_with_flags(
        &db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut stdout = std::io::stdout().lock();
    let interval = Duration::from_millis(interval_ms);

    loop {
        let frame_start = Instant::now();
        let frame = render_frame(&conn, &db, interval_ms)?;
        // Write the whole frame in one shot to minimise tearing.
        write!(stdout, "{ANSI_CLEAR_HOME}{frame}")?;
        stdout.flush()?;

        let elapsed = frame_start.elapsed();
        if elapsed < interval {
            std::thread::sleep(interval - elapsed);
        }
    }
}

fn render_frame(conn: &Connection, db: &Path, interval_ms: u64) -> Result<String> {
    let mut out = String::with_capacity(4096);

    let now = local_clock_string();
    out.push_str(&format!(
        "{ANSI_BOLD}stores watch{ANSI_RESET}  {ANSI_DIM}{}  ·  {now}{ANSI_RESET}\n\n",
        db.display()
    ));

    // ---- TASKS -----------------------------------------------------------
    let tasks = query_tasks(conn).unwrap_or_default();
    let active_count = tasks
        .iter()
        .filter(|t| !is_terminal_task_status(&t.status))
        .count();
    out.push_str(&format!(
        "{ANSI_BOLD}TASKS{ANSI_RESET} {ANSI_DIM}({} active · {} total){ANSI_RESET}\n",
        active_count,
        tasks.len()
    ));

    if tasks.is_empty() {
        out.push_str(&format!("  {ANSI_DIM}(no rows){ANSI_RESET}\n"));
    } else {
        // Active first (in workflow order), terminal at the bottom.
        let mut sorted: Vec<&TaskRow> = tasks.iter().collect();
        sorted.sort_by_key(|t| (task_status_rank(&t.status), t.display_id.clone()));
        for t in sorted.iter().take(20) {
            out.push_str(&render_task_line(t));
        }
        if tasks.len() > 20 {
            out.push_str(&format!(
                "  {ANSI_DIM}... {} more{ANSI_RESET}\n",
                tasks.len() - 20
            ));
        }
    }

    // ---- OBSERVATIONS ----------------------------------------------------
    out.push('\n');
    let obs = query_observations(conn).unwrap_or_default();
    let open_count = obs.iter().filter(|o| o.status == "open").count();
    out.push_str(&format!(
        "{ANSI_BOLD}OBSERVATIONS{ANSI_RESET} {ANSI_DIM}({} open · {} total){ANSI_RESET}\n",
        open_count,
        obs.len()
    ));

    if obs.is_empty() {
        out.push_str(&format!("  {ANSI_DIM}(no rows){ANSI_RESET}\n"));
    } else {
        let mut sorted: Vec<&ObsRow> = obs.iter().collect();
        // Open first, then by updated_at desc.
        sorted.sort_by(|a, b| {
            let ra = if a.status == "open" { 0 } else { 1 };
            let rb = if b.status == "open" { 0 } else { 1 };
            ra.cmp(&rb).then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        for o in sorted.iter().take(10) {
            out.push_str(&render_obs_line(o));
        }
        if obs.len() > 10 {
            out.push_str(&format!(
                "  {ANSI_DIM}... {} more{ANSI_RESET}\n",
                obs.len() - 10
            ));
        }
    }

    out.push('\n');
    out.push_str(&format!(
        "{ANSI_DIM}[{:.1}s refresh · Ctrl-C to quit]{ANSI_RESET}\n",
        interval_ms as f64 / 1000.0
    ));

    Ok(out)
}

// ---------------------------------------------------------------------------
// Rows + queries
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct TaskRow {
    display_id: String,
    status: String,
    title: String,
    claimed_by: Option<String>,
    updated_at: String,
    current_phase: Option<i64>,
    current_cycle: Option<i64>,
    total_phases: Option<i64>,
}

/// Schema-declared limit (`max_revise_cycles: 3` in stores/tasks/schema.yaml).
/// Hardcoded here for the POC; promote to a schema read when this graduates.
const MAX_CYCLES_DISPLAY: i64 = 3;

// T015 P1: phase-box + cycle-dot glyphs.
const GLYPH_DONE: char = '▰';
const GLYPH_CURRENT_EXEC: char = '▮';
const GLYPH_CURRENT_REVIEW: char = '◐';
const GLYPH_FUTURE: char = '▱';
const GLYPH_DOT_BURNED: char = '●';
const GLYPH_DOT_AVAILABLE: char = '·';

/// Visible-column budget for the status column. Fits 6 boxes + spacer + 3 dots
/// (10) with headroom for the longest non-cycling label, "reviewing plan" (14).
const STATUS_COL_WIDTH: usize = 16;

#[derive(Debug)]
struct ObsRow {
    display_id: String,
    status: String,
    priority: String,
    summary: String,
    updated_at: String,
}

fn query_tasks(conn: &Connection) -> Result<Vec<TaskRow>> {
    let mut stmt = conn.prepare(
        "SELECT display_id, status, COALESCE(title, ''), claimed_by, COALESCE(updated_at, ''),
                current_phase, current_cycle,
                json_array_length(json_extract(plan, '$.phases'))
         FROM tasks",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(TaskRow {
            display_id: r.get(0)?,
            status: r.get(1)?,
            title: r.get(2)?,
            claimed_by: r.get(3)?,
            updated_at: r.get(4)?,
            current_phase: r.get(5)?,
            current_cycle: r.get(6)?,
            total_phases: r.get(7)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn query_observations(conn: &Connection) -> Result<Vec<ObsRow>> {
    let mut stmt = conn.prepare(
        "SELECT display_id, status, COALESCE(priority, ''), COALESCE(summary, ''), COALESCE(updated_at, '')
         FROM observations",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ObsRow {
            display_id: r.get(0)?,
            status: r.get(1)?,
            priority: r.get(2)?,
            summary: r.get(3)?,
            updated_at: r.get(4)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

fn render_task_line(t: &TaskRow) -> String {
    let claimed = t
        .claimed_by
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|c| format!(" {ANSI_DIM}· {c}{ANSI_RESET}"))
        .unwrap_or_default();
    let title = truncate(&t.title, 50);
    let updated = clock_suffix(&t.updated_at);
    let status_str = format_task_status(t);
    let pad = STATUS_COL_WIDTH.saturating_sub(visible_width(&status_str));
    let pad_str = " ".repeat(pad);
    format!(
        "  {ANSI_CYAN}{:<5}{ANSI_RESET} {status_str}{pad_str} {:<50}{claimed} {ANSI_DIM}{updated}{ANSI_RESET}\n",
        t.display_id, title
    )
}

/// Compact status renderer.
///
/// For executing/code_review with full phase data, returns the phase-box +
/// cycle-dot visualization (the current box self-colors via embedded ANSI;
/// other boxes/dots remain default-color). Falls back to the legacy text
/// format when phase data is partial, and to a bare colored state name
/// otherwise.
fn format_task_status(t: &TaskRow) -> String {
    let bare_color = task_status_color(&t.status);
    let wrap = |s: &str| format!("{bare_color}{s}{ANSI_RESET}");
    match t.status.as_str() {
        "plan_review" => wrap("reviewing plan"),
        "executing" | "code_review" => {
            match (t.current_phase, t.total_phases, t.current_cycle) {
                (Some(p), Some(n), Some(c)) if n > 0 => {
                    let in_review = t.status == "code_review";
                    let color = current_box_color(&t.status, c);
                    let boxes = render_phase_boxes(p, n, in_review, color);
                    let dots = render_cycle_dots(c, MAX_CYCLES_DISPLAY);
                    format!("{boxes} {dots}")
                }
                (Some(p), None, Some(c)) => {
                    let verb = if t.status == "executing" { "execute" } else { "review " };
                    wrap(&format!("{verb} P{p}/? R{c}/{MAX_CYCLES_DISPLAY}"))
                }
                _ => wrap(&t.status),
            }
        }
        other => wrap(other),
    }
}

/// Render the phase-box row. `current_phase` is 1-indexed. Only the current
/// box is wrapped in ANSI color; other boxes use the terminal default.
/// For total_phases > 6, truncates to `<3 boxes>…<current><next?>`.
fn render_phase_boxes(
    current_phase: i64,
    total_phases: i64,
    in_code_review: bool,
    color: &'static str,
) -> String {
    let current_glyph = if in_code_review {
        GLYPH_CURRENT_REVIEW
    } else {
        GLYPH_CURRENT_EXEC
    };
    let glyph_for = |i: i64| -> char {
        if i < current_phase {
            GLYPH_DONE
        } else if i == current_phase {
            current_glyph
        } else {
            GLYPH_FUTURE
        }
    };
    let mut out = String::new();
    let push_box = |out: &mut String, i: i64| {
        let g = glyph_for(i);
        if i == current_phase {
            out.push_str(color);
            out.push(g);
            out.push_str(ANSI_RESET);
        } else {
            out.push(g);
        }
    };
    if total_phases <= 6 {
        for i in 1..=total_phases {
            push_box(&mut out, i);
        }
    } else if current_phase <= 3 {
        // Current still inside the prefix; render first 6 verbatim, no ellipsis.
        for i in 1..=6 {
            push_box(&mut out, i);
        }
    } else {
        for i in 1..=3 {
            push_box(&mut out, i);
        }
        out.push('…');
        push_box(&mut out, current_phase);
        if current_phase < total_phases {
            out.push(GLYPH_FUTURE);
        }
    }
    out
}

/// Render the cycle-dot row: `max_cycles` chars total, one ● per cycle past 1
/// (clamped to max_cycles), · for remaining slots.
fn render_cycle_dots(current_cycle: i64, max_cycles: i64) -> String {
    let burned = (current_cycle - 1).clamp(0, max_cycles);
    let mut out = String::new();
    for i in 0..max_cycles {
        if i < burned {
            out.push(GLYPH_DOT_BURNED);
        } else {
            out.push(GLYPH_DOT_AVAILABLE);
        }
    }
    out
}

/// Color for the CURRENT box. The blocked/in_review branch is currently dead
/// (those statuses don't enter the cycling renderer) but kept to honor the
/// done-when wording so future flag-flips Just Work.
fn current_box_color(task_status: &str, current_cycle: i64) -> &'static str {
    if current_cycle > MAX_CYCLES_DISPLAY {
        ANSI_RED
    } else if task_status == "blocked" || task_status == "in_review" {
        ANSI_YELLOW
    } else {
        ANSI_GREEN
    }
}

/// Count visible columns in `s`, ignoring ANSI CSI escape sequences
/// (`\x1b[...m`). Treats every other char as one column — fine for our
/// monospace single-cell glyphs (▰▮◐▱●·).
fn visible_width(s: &str) -> usize {
    let mut count = 0usize;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c2 in chars.by_ref() {
                if c2 == 'm' {
                    break;
                }
            }
        } else {
            count += 1;
        }
    }
    count
}

fn render_obs_line(o: &ObsRow) -> String {
    let status_color = obs_status_color(&o.status);
    let priority_color = match o.priority.as_str() {
        "high" => ANSI_RED,
        "normal" => ANSI_YELLOW,
        "low" => ANSI_DIM,
        _ => ANSI_DIM,
    };
    let summary = truncate(&o.summary, 60);
    let updated = clock_suffix(&o.updated_at);
    format!(
        "  {ANSI_MAGENTA}{:<5}{ANSI_RESET} {status_color}{:<11}{ANSI_RESET} {priority_color}{:<6}{ANSI_RESET} {:<60} {ANSI_DIM}{updated}{ANSI_RESET}\n",
        o.display_id, o.status, o.priority, summary
    )
}

fn task_status_rank(s: &str) -> u8 {
    match s {
        "executing" | "code_review" => 0,
        "ready" => 1,
        "planning" | "plan_review" => 2,
        "in_review" => 3,
        "blocked" => 4,
        "rejected" => 5,
        "complete" => 9,
        _ => 6,
    }
}

fn task_status_color(s: &str) -> &'static str {
    match s {
        "executing" | "code_review" => ANSI_GREEN,
        "ready" => ANSI_CYAN,
        "planning" | "plan_review" | "in_review" => ANSI_YELLOW,
        "blocked" | "rejected" => ANSI_RED,
        "complete" => ANSI_DIM,
        _ => ANSI_RESET,
    }
}

fn obs_status_color(s: &str) -> &'static str {
    match s {
        "open" => ANSI_YELLOW,
        "investigating" | "confirming" | "claiming" => ANSI_GREEN,
        "resolved" | "rejected" => ANSI_DIM,
        _ => ANSI_RESET,
    }
}

fn is_terminal_task_status(s: &str) -> bool {
    matches!(s, "complete" | "rejected")
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        format!("{:<width$}", s, width = n)
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Pull `HH:MM:SS` out of an ISO 8601 timestamp like
/// `2026-05-03T14:23:11+00:00`. Falls back to the raw string truncated.
fn clock_suffix(ts: &str) -> String {
    if let Some(t_idx) = ts.find('T') {
        let after_t = &ts[t_idx + 1..];
        let take = after_t
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == ':')
            .collect::<String>();
        if take.len() >= 5 {
            return take;
        }
    }
    if ts.len() > 19 {
        ts[..19].to_string()
    } else {
        ts.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, phase: Option<i64>, total: Option<i64>, cycle: Option<i64>) -> TaskRow {
        TaskRow {
            display_id: "T999".to_string(),
            status: status.to_string(),
            title: "test".to_string(),
            claimed_by: None,
            updated_at: String::new(),
            current_phase: phase,
            current_cycle: cycle,
            total_phases: total,
        }
    }

    /// Strip ANSI CSI escape sequences (`\x1b[...m`).
    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c2 in chars.by_ref() {
                    if c2 == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    // --- render_phase_boxes -------------------------------------------------

    #[test]
    fn phase_boxes_3_phase_executing() {
        assert_eq!(strip_ansi(&render_phase_boxes(1, 3, false, ANSI_GREEN)), "▮▱▱");
        assert_eq!(strip_ansi(&render_phase_boxes(2, 3, false, ANSI_GREEN)), "▰▮▱");
        assert_eq!(strip_ansi(&render_phase_boxes(3, 3, false, ANSI_GREEN)), "▰▰▮");
    }

    #[test]
    fn phase_boxes_3_phase_code_review() {
        assert_eq!(strip_ansi(&render_phase_boxes(1, 3, true, ANSI_GREEN)), "◐▱▱");
        assert_eq!(strip_ansi(&render_phase_boxes(2, 3, true, ANSI_GREEN)), "▰◐▱");
        assert_eq!(strip_ansi(&render_phase_boxes(3, 3, true, ANSI_GREEN)), "▰▰◐");
    }

    #[test]
    fn phase_boxes_6_phase_at_phase_4() {
        assert_eq!(strip_ansi(&render_phase_boxes(4, 6, false, ANSI_GREEN)), "▰▰▰▮▱▱");
    }

    #[test]
    fn phase_boxes_12_phase_truncated() {
        assert_eq!(strip_ansi(&render_phase_boxes(5, 12, false, ANSI_GREEN)), "▰▰▰…▮▱");
    }

    #[test]
    fn phase_boxes_12_phase_at_end_no_trailing_future() {
        assert_eq!(strip_ansi(&render_phase_boxes(12, 12, false, ANSI_GREEN)), "▰▰▰…▮");
    }

    #[test]
    fn phase_boxes_current_box_is_colored() {
        let s = render_phase_boxes(2, 3, false, ANSI_GREEN);
        assert!(s.contains(ANSI_GREEN));
        assert!(s.contains(ANSI_RESET));
        // Color brackets only the current glyph.
        assert!(s.contains(&format!("{ANSI_GREEN}{GLYPH_CURRENT_EXEC}{ANSI_RESET}")));
    }

    // --- render_cycle_dots --------------------------------------------------

    #[test]
    fn cycle_dots_basic() {
        assert_eq!(render_cycle_dots(1, 3), "···");
        assert_eq!(render_cycle_dots(2, 3), "●··");
        assert_eq!(render_cycle_dots(3, 3), "●●·");
        assert_eq!(render_cycle_dots(4, 3), "●●●");
    }

    // --- current_box_color --------------------------------------------------

    #[test]
    fn current_box_color_branches() {
        assert_eq!(current_box_color("executing", 1), ANSI_GREEN);
        assert_eq!(current_box_color("code_review", 3), ANSI_GREEN);
        assert_eq!(current_box_color("blocked", 1), ANSI_YELLOW);
        assert_eq!(current_box_color("in_review", 2), ANSI_YELLOW);
        assert_eq!(current_box_color("executing", 4), ANSI_RED);
    }

    // --- format_task_status: full matrix -----------------------------------

    #[test]
    fn format_task_status_matrix() {
        for status in &["executing", "code_review"] {
            for cycle in 1..=4i64 {
                for &(phase, total, expected_glyphs) in &[
                    (1i64, 3i64, "▮▱▱"),
                    (2, 3, "▰▮▱"),
                    (3, 3, "▰▰▮"),
                    (4, 6, "▰▰▰▮▱▱"),
                    (5, 12, "▰▰▰…▮▱"),
                ] {
                    let in_review = *status == "code_review";
                    let want_glyphs = if in_review {
                        expected_glyphs
                            .replace(GLYPH_CURRENT_EXEC, &GLYPH_CURRENT_REVIEW.to_string())
                    } else {
                        expected_glyphs.to_string()
                    };
                    let want_dots = match cycle {
                        1 => "···",
                        2 => "●··",
                        3 => "●●·",
                        _ => "●●●",
                    };
                    let r = row(status, Some(phase), Some(total), Some(cycle));
                    let got = strip_ansi(&format_task_status(&r));
                    let want = format!("{want_glyphs} {want_dots}");
                    assert_eq!(
                        got, want,
                        "status={status} cycle={cycle} phase={phase}/{total}"
                    );
                }
            }
        }
    }

    #[test]
    fn format_task_status_glyph_and_dots_separated_by_one_space() {
        let r = row("executing", Some(2), Some(3), Some(2));
        let got = strip_ansi(&format_task_status(&r));
        let parts: Vec<&str> = got.split(' ').collect();
        assert_eq!(parts.len(), 2, "exactly one space separator: {got:?}");
        assert_eq!(parts[0], "▰▮▱");
        assert_eq!(parts[1], "●··");
    }

    // --- fallback / non-cycling --------------------------------------------

    #[test]
    fn fallback_when_phase_data_missing() {
        // No phase data at all → bare status name.
        let r = row("executing", None, None, None);
        assert_eq!(strip_ansi(&format_task_status(&r)), "executing");
    }

    #[test]
    fn fallback_legacy_text_when_total_missing() {
        let r = row("executing", Some(2), None, Some(1));
        assert_eq!(strip_ansi(&format_task_status(&r)), "execute P2/? R1/3");
    }

    #[test]
    fn fallback_legacy_text_for_code_review() {
        let r = row("code_review", Some(2), None, Some(2));
        assert_eq!(strip_ansi(&format_task_status(&r)), "review  P2/? R2/3");
    }

    #[test]
    fn non_cycling_statuses_render_bare_names() {
        for (status, expected) in &[
            ("planning", "planning"),
            ("plan_review", "reviewing plan"),
            ("ready", "ready"),
            ("in_review", "in_review"),
            ("complete", "complete"),
            ("accepted", "accepted"),
            ("blocked", "blocked"),
            ("rejected", "rejected"),
        ] {
            let r = row(status, Some(2), Some(3), Some(1));
            assert_eq!(
                strip_ansi(&format_task_status(&r)),
                *expected,
                "status={status}"
            );
        }
    }
}

fn local_clock_string() -> String {
    // Cheap clock without chrono: SystemTime → epoch secs → HH:MM:SS local-ish.
    // We approximate "local" by reading the TZ offset via libc's localtime_r
    // is overkill; for a POC, print UTC HH:MM:SS — clearly labelled.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02} UTC")
}
