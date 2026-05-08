use stores::tui::data::{ExternalReviewState, TaskRow};
use stores::tui::progress::task_progress;

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
fn t1_row_uses_state_fallback_not_phase_progress() {
    let p = task_progress(&task("T1", "executing", Some(1), Some(2), Some(1)), &ExternalReviewState::default());
    assert!(!p.visual);
    assert_eq!(p.text, "executing");
}

#[test]
fn t2_two_phase_executing_renders_glyph_sequence() {
    let p = task_progress(&task("T2", "executing", Some(1), Some(2), Some(1)), &ExternalReviewState::default());
    assert!(p.visual);
    assert_eq!(p.text, "▮▱ ···");
    assert!(!p.text.contains("executing"));
}

#[test]
fn t3_code_review_renders_review_marker_and_cycle_dots() {
    let p = task_progress(&task("T3", "code_review", Some(2), Some(3), Some(3)), &ExternalReviewState::default());
    assert_eq!(p.text, "▰◐▱ ●●·");
}

#[test]
fn t3_multi_phase_truncates() {
    let p = task_progress(&task("T3", "executing", Some(5), Some(12), Some(2)), &ExternalReviewState::default());
    assert_eq!(p.text, "▰▰▰…▮▱ ●··");
}

#[test]
fn wrap_in_review_external_unavailable_and_accepted_stages_render() {
    let wrap = task_progress(&task("T3", "in_review", Some(3), Some(3), Some(1)), &ExternalReviewState::default());
    assert!(wrap.text.contains("external review: unavailable / not installed"));

    let accepted = task_progress(&task("T2", "accepted", Some(2), Some(2), Some(1)), &ExternalReviewState::default());
    assert_eq!(accepted.text, "✓ accepted");
}
