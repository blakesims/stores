//! Compact task progress indicators for watch cockpit rows.
//!
//! T2/T3 rows get visual phase boxes + cycle dots. T1 / partial-plan rows get
//! readable state text and do not imply multi-phase progress.

use super::data::{ExternalReviewState, TaskRow};

pub const MAX_CYCLES_DISPLAY: i64 = 3;

const GLYPH_DONE: char = '▰';
const GLYPH_CURRENT_EXEC: char = '▮';
const GLYPH_CURRENT_REVIEW: char = '◐';
const GLYPH_FUTURE: char = '▱';
const GLYPH_DOT_BURNED: char = '●';
const GLYPH_DOT_AVAILABLE: char = '·';

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskProgress {
    pub visual: bool,
    pub text: String,
}

pub fn task_progress(t: &TaskRow, external_review: &ExternalReviewState) -> TaskProgress {
    let tier = t.tier_hint.as_deref().unwrap_or("").to_ascii_uppercase();
    let visual_tier = matches!(tier.as_str(), "T2" | "T3");
    if !visual_tier {
        return TaskProgress { visual: false, text: fallback_status(t) };
    }

    let terminal = terminal_stage(&t.status);
    if let Some(text) = terminal {
        return TaskProgress { visual: true, text };
    }

    match t.status.as_str() {
        "plan_review" => TaskProgress { visual: true, text: "plan ◐".to_string() },
        "executing" | "code_review" => match (t.current_phase, t.total_phases, t.current_cycle) {
            (Some(p), Some(n), Some(c)) if n > 0 => {
                let boxes = phase_boxes(p, n, t.status == "code_review");
                let dots = cycle_dots(c, MAX_CYCLES_DISPLAY);
                TaskProgress { visual: true, text: format!("{boxes} {dots}") }
            }
            (Some(p), None, Some(c)) => TaskProgress { visual: false, text: format!("P{p}/? C{c}/{MAX_CYCLES_DISPLAY}") },
            _ => TaskProgress { visual: false, text: fallback_status(t) },
        },
        "complete" | "in_review" => TaskProgress { visual: true, text: wrap_stage(external_review) },
        _ => TaskProgress { visual: false, text: fallback_status(t) },
    }
}

pub fn phase_boxes(current_phase: i64, total_phases: i64, in_code_review: bool) -> String {
    let current = if in_code_review { GLYPH_CURRENT_REVIEW } else { GLYPH_CURRENT_EXEC };
    let glyph_for = |i: i64| -> char {
        if i < current_phase { GLYPH_DONE } else if i == current_phase { current } else { GLYPH_FUTURE }
    };
    let mut out = String::new();
    if total_phases <= 6 {
        for i in 1..=total_phases { out.push(glyph_for(i)); }
    } else if current_phase <= 3 {
        for i in 1..=6 { out.push(glyph_for(i)); }
    } else {
        for i in 1..=3 { out.push(glyph_for(i)); }
        out.push('…');
        out.push(glyph_for(current_phase));
        if current_phase < total_phases { out.push(GLYPH_FUTURE); }
    }
    out
}

pub fn cycle_dots(current_cycle: i64, max_cycles: i64) -> String {
    let burned = (current_cycle - 1).clamp(0, max_cycles);
    let mut out = String::new();
    for i in 0..max_cycles {
        out.push(if i < burned { GLYPH_DOT_BURNED } else { GLYPH_DOT_AVAILABLE });
    }
    out
}

fn fallback_status(t: &TaskRow) -> String {
    match (t.current_phase, t.total_phases, t.current_cycle) {
        _ => t.status.clone(),
    }
}

fn terminal_stage(status: &str) -> Option<String> {
    match status {
        "accepted" => Some("✓ accepted".to_string()),
        "deploy" | "cargo_installed" | "schema_migrated" => Some("✓ deploy".to_string()),
        _ => None,
    }
}

fn wrap_stage(external_review: &ExternalReviewState) -> String {
    match external_review {
        ExternalReviewState::Available { rows } => format!("wrap → ext:{rows}"),
        ExternalReviewState::Unavailable { reason } => format!("wrap → {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(tier: &str, status: &str, phase: Option<i64>, total: Option<i64>, cycle: Option<i64>) -> TaskRow {
        TaskRow {
            display_id: "T999".to_string(),
            status: status.to_string(),
            title: "progress".to_string(),
            tier_hint: Some(tier.to_string()),
            current_phase: phase,
            total_phases: total,
            current_cycle: cycle,
            ..Default::default()
        }
    }

    #[test]
    fn t1_no_progress_fallback() {
        let p = task_progress(&task("T1", "executing", Some(1), Some(2), Some(1)), &ExternalReviewState::default());
        assert!(!p.visual);
        assert_eq!(p.text, "executing");
    }

    #[test]
    fn t2_executing_and_code_review_glyphs() {
        let exec = task_progress(&task("T2", "executing", Some(1), Some(2), Some(1)), &ExternalReviewState::default());
        let review = task_progress(&task("T2", "code_review", Some(2), Some(2), Some(2)), &ExternalReviewState::default());
        assert_eq!(exec.text, "▮▱ ···");
        assert_eq!(review.text, "▰◐ ●··");
    }

    #[test]
    fn t3_multi_phase_truncation() {
        let p = task_progress(&task("T3", "executing", Some(5), Some(12), Some(3)), &ExternalReviewState::default());
        assert_eq!(p.text, "▰▰▰…▮▱ ●●·");
    }

    #[test]
    fn wrap_in_review_external_unavailable_stage() {
        let p = task_progress(&task("T3", "in_review", Some(3), Some(3), Some(1)), &ExternalReviewState::default());
        assert!(p.text.contains("wrap → external review: unavailable / not installed"));
    }

    #[test]
    fn accepted_completion_stage() {
        let p = task_progress(&task("T2", "accepted", Some(2), Some(2), Some(1)), &ExternalReviewState::default());
        assert_eq!(p.text, "✓ accepted");
    }
}
