//! Pure semantic presentation mapping for `stores watch`.
//!
//! This module translates schema/debug tuples into operator-facing labels and
//! glyphs. Rendering phases consume these structs later; rows/details keep raw
//! tuples elsewhere.

use serde_json::Value;

use super::daemon::Liveness;
use super::data::{
    obs_lifecycle, task_active_step, task_integration_step, task_is_blocked,
    task_is_terminal_primary, task_lifecycle, IntakeRow, ObsRow, ReviewRow, SystemHealth, TaskRow,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationSeverity {
    Front,
    Work,
    Gate,
    Exit,
    Wait,
    Fault,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presentation {
    pub glyph: &'static str,
    pub label: String,
    pub severity: PresentationSeverity,
    pub signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowSlotPresentation {
    pub slot: PresentationSeverity,
    pub glyph: &'static str,
    pub label: &'static str,
    pub count: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchSlotId {
    Front,
    Work,
    Gate,
    Exit,
    Wait,
    Fault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchAttention {
    Exhaust,
    Flow,
    Fault,
    Neutral,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchProjection {
    pub slot: WatchSlotId,
    pub slot_label: &'static str,
    pub glyph: &'static str,
    pub row_stage: &'static str,
    pub row_signal: Option<String>,
    pub next_action: Option<&'static str>,
    pub attention: WatchAttention,
}

pub fn task_watch_projection(task: &TaskRow) -> WatchProjection {
    let presentation = task_presentation(task);
    let slot = watch_slot_id(presentation.severity);
    WatchProjection {
        slot,
        slot_label: watch_slot_label(slot),
        glyph: presentation.glyph,
        row_stage: task_watch_stage(&presentation.label),
        row_signal: presentation.signal,
        next_action: None,
        attention: watch_attention(slot),
    }
}

fn task_watch_stage(label: &str) -> &'static str {
    match label {
        "done" => "done",
        "ship" => "ship",
        "queued" => "queued",
        "plan" => "plan",
        "plan-gate" => "plan-gate",
        "exec" => "exec",
        "code-gate" => "code-gate",
        "accept" => "accept",
        "work" => "work",
        "waiting-capacity" => "waiting-capacity",
        "waiting-dependency" => "waiting-dependency",
        "runner-failed" => "runner-failed",
        "rate-limited" => "rate-limited",
        "needs-human" => "needs-human",
        "review-blocked" => "review-blocked",
        "stale-base" => "stale-base",
        "config-fault" => "config-fault",
        "tests-failed" => "tests-failed",
        "main-red" => "main-red",
        "deploy-failed" => "deploy-failed",
        "migration-failed" => "migration-failed",
        _ => "blocked",
    }
}

fn watch_slot_id(severity: PresentationSeverity) -> WatchSlotId {
    match severity {
        PresentationSeverity::Front => WatchSlotId::Front,
        PresentationSeverity::Work => WatchSlotId::Work,
        PresentationSeverity::Gate => WatchSlotId::Gate,
        PresentationSeverity::Exit => WatchSlotId::Exit,
        PresentationSeverity::Wait => WatchSlotId::Wait,
        PresentationSeverity::Fault => WatchSlotId::Fault,
    }
}

fn watch_slot_label(slot: WatchSlotId) -> &'static str {
    match slot {
        WatchSlotId::Front => "queued",
        WatchSlotId::Work => "working",
        WatchSlotId::Gate => "gate",
        WatchSlotId::Exit => "done",
        WatchSlotId::Wait => "waiting",
        WatchSlotId::Fault => "failed",
    }
}

fn watch_attention(slot: WatchSlotId) -> WatchAttention {
    match slot {
        WatchSlotId::Exit => WatchAttention::Exhaust,
        WatchSlotId::Fault => WatchAttention::Fault,
        WatchSlotId::Front | WatchSlotId::Work | WatchSlotId::Gate | WatchSlotId::Wait => {
            WatchAttention::Flow
        }
    }
}

pub fn task_presentation(task: &TaskRow) -> Presentation {
    if task_is_terminal_primary(task) {
        return presentation("■", "done", PresentationSeverity::Exit, None);
    }
    if task_is_blocked(task) {
        return blocked_task_presentation(task);
    }

    match task_lifecycle(task) {
        "integration" => presentation("▱", "ship", PresentationSeverity::Gate, None),
        "queued" => presentation(
            "◌",
            "queued",
            PresentationSeverity::Front,
            task_signal(task),
        ),
        "active" => match task_active_step(task) {
            "planning" => presentation("◆", "plan", PresentationSeverity::Work, task_signal(task)),
            "planning_review" => presentation(
                "◇",
                "plan-gate",
                PresentationSeverity::Gate,
                task_signal(task),
            ),
            "coding" => presentation("▣", "exec", PresentationSeverity::Work, task_signal(task)),
            "coding_review" => presentation(
                "◈",
                "code-gate",
                PresentationSeverity::Gate,
                task_signal(task),
            ),
            "wrapping" => {
                presentation("▰", "accept", PresentationSeverity::Gate, task_signal(task))
            }
            _ => presentation("◆", "work", PresentationSeverity::Work, task_signal(task)),
        },
        _ => presentation(
            "◌",
            "queued",
            PresentationSeverity::Front,
            task_signal(task),
        ),
    }
}

fn blocked_task_presentation(task: &TaskRow) -> Presentation {
    let kind = task
        .blocker_kind
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "none")
        .or_else(|| {
            task.blocked_reason_class
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "none")
        })
        .unwrap_or("blocked");
    let (glyph, label, severity) = match kind {
        "capacity" => (
            "△",
            "waiting-capacity".to_string(),
            PresentationSeverity::Wait,
        ),
        "dependency" => (
            "△",
            "waiting-dependency".to_string(),
            PresentationSeverity::Wait,
        ),
        "runner" => (
            "▲",
            "runner-failed".to_string(),
            PresentationSeverity::Fault,
        ),
        "rate_limit" => ("△", "rate-limited".to_string(), PresentationSeverity::Wait),
        "human_acceptance" => ("⋯", "needs-human".to_string(), PresentationSeverity::Gate),
        "task_review" => (
            "▲",
            "review-blocked".to_string(),
            PresentationSeverity::Fault,
        ),
        "stale_base" => ("△", "stale-base".to_string(), PresentationSeverity::Wait),
        "config" => ("▲", "config-fault".to_string(), PresentationSeverity::Fault),
        "test_failure" => ("▲", "tests-failed".to_string(), PresentationSeverity::Fault),
        "main_red" => ("▲", "main-red".to_string(), PresentationSeverity::Fault),
        "deploy" => (
            "▲",
            "deploy-failed".to_string(),
            PresentationSeverity::Fault,
        ),
        "migration" => (
            "▲",
            "migration-failed".to_string(),
            PresentationSeverity::Fault,
        ),
        other => (
            "▲",
            format!("{}-blocked", other.replace('_', "-")),
            PresentationSeverity::Fault,
        ),
    };
    Presentation {
        glyph,
        label,
        severity,
        signal: blocked_signal(task).or_else(|| task_signal(task)),
    }
}

fn task_signal(task: &TaskRow) -> Option<String> {
    if task
        .workspace_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
        && matches!(task_lifecycle(task), "queued" | "active")
    {
        return Some("no worktree".to_string());
    }
    if task_lifecycle(task) == "integration" {
        let step = task_integration_step(task);
        if step != "none" {
            return Some(step.replace('_', "-"));
        }
    }
    None
}

fn blocked_signal(task: &TaskRow) -> Option<String> {
    let raw = task.blocked_reason.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        if let Some(code) = value.get("exit_code").and_then(Value::as_i64) {
            return Some(format!("exit {code}"));
        }
        if let Some(kind) = value.get("kind").and_then(Value::as_str) {
            return Some(kind.replace('_', "-"));
        }
    }
    Some(raw.to_string())
}

pub fn observation_presentation(row: &ObsRow) -> Presentation {
    if row.pending_architecture_review.unwrap_or(false)
        || row
            .open_architecture_review_id
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
    {
        return presentation("◈", "arch-gate", PresentationSeverity::Gate, None);
    }
    if row
        .superseded_by_id
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
        || matches!(row.outcome.as_deref(), Some("superseded"))
    {
        return presentation("■", "superseded", PresentationSeverity::Exit, None);
    }
    if matches!(row.waiting_kind.as_deref(), Some("info_needed")) {
        return presentation(
            "⋯",
            "needs-info",
            PresentationSeverity::Wait,
            Some("waiting".to_string()),
        );
    }
    if matches!(row.waiting_kind.as_deref(), Some("human_ratification")) {
        return presentation(
            "◈",
            "contract-draft",
            PresentationSeverity::Gate,
            Some("contract draft".to_string()),
        );
    }
    if matches!(row.contract_state.as_deref(), Some("draft")) {
        return presentation(
            "◈",
            "contract-draft",
            PresentationSeverity::Gate,
            Some("contract draft".to_string()),
        );
    }
    if matches!(row.contract_state.as_deref(), Some("approved" | "ready")) {
        return presentation("▰", "contract-approved", PresentationSeverity::Gate, None);
    }
    match obs_lifecycle(row) {
        "candidate" => presentation(
            "◌",
            "candidate",
            PresentationSeverity::Front,
            Some("needs triage".to_string()),
        ),
        "ready" | "investigating" => presentation(
            "◆",
            "investigate",
            PresentationSeverity::Work,
            contract_signal(row),
        ),
        "in_progress" => presentation("▣", "resolving", PresentationSeverity::Work, None),
        "closed" => match row.outcome.as_deref().unwrap_or(row.status.as_str()) {
            "wont_fix" | "wont-fix" => {
                presentation("×", "wont-fix", PresentationSeverity::Exit, None)
            }
            "superseded" => presentation("■", "superseded", PresentationSeverity::Exit, None),
            _ => presentation("✓", "addressed", PresentationSeverity::Exit, None),
        },
        _ => presentation(
            "◆",
            "investigate",
            PresentationSeverity::Work,
            contract_signal(row),
        ),
    }
}

fn contract_signal(row: &ObsRow) -> Option<String> {
    match row.contract_state.as_deref() {
        Some("draft") => Some("contract draft".to_string()),
        Some("approved" | "ready") => Some("contract approved".to_string()),
        _ => None,
    }
}

pub fn intake_presentation(row: &IntakeRow) -> Presentation {
    let lifecycle = row.lifecycle.as_deref().unwrap_or(row.status.as_str());
    if lifecycle == "closed" {
        return match row
            .outcome
            .as_deref()
            .or(row.decision.as_deref())
            .unwrap_or(row.status.as_str())
        {
            "routed_to_observation" | "routed" => {
                presentation("✓", "routed", PresentationSeverity::Exit, route_signal(row))
            }
            "escalated_to_architecture_review" | "architecture_review" => presentation(
                "◈",
                "arch-review",
                PresentationSeverity::Gate,
                route_signal(row),
            ),
            "marked_duplicate" | "duplicate" => presentation(
                "≡",
                "duplicate",
                PresentationSeverity::Exit,
                route_signal(row),
            ),
            "dropped_as_noise" | "dropped" => {
                presentation("×", "dropped", PresentationSeverity::Exit, None)
            }
            _ => presentation("✓", "routed", PresentationSeverity::Exit, route_signal(row)),
        };
    }
    match lifecycle {
        "new" | "draft" => presentation("◌", "new", PresentationSeverity::Front, None),
        "triaging" => presentation("◆", "triage", PresentationSeverity::Work, None),
        "waiting" | "needs_info" => presentation(
            "?",
            "needs-info",
            PresentationSeverity::Gate,
            row.missing_info_question.clone(),
        ),
        _ => presentation("◌", "new", PresentationSeverity::Front, None),
    }
}

fn route_signal(row: &IntakeRow) -> Option<String> {
    row.routed_to_observation
        .clone()
        .or_else(|| row.produced_observation_id.clone())
        .or_else(|| row.routed_to_arch_review.clone())
        .or_else(|| row.produced_architecture_review_id.clone())
        .or_else(|| row.duplicate_of.clone())
        .or_else(|| row.duplicate_of_id.clone())
}

pub fn external_review_presentation(row: &ReviewRow) -> Presentation {
    match row.status.as_str() {
        "pending" => presentation(
            "◌",
            "pending",
            PresentationSeverity::Front,
            Some("waiting for dispatch".to_string()),
        ),
        "running" => presentation("◆", "running", PresentationSeverity::Work, None),
        "passed" => presentation(
            "✓",
            "passed",
            PresentationSeverity::Exit,
            findings_signal(row),
        ),
        "revise" => presentation(
            "↻",
            "revise",
            PresentationSeverity::Gate,
            findings_signal(row),
        ),
        "tooling_held" | "tool_fault" | "tool-fault" => presentation(
            "▲",
            "tool-fault",
            PresentationSeverity::Fault,
            row.held_reason
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "none")
                .map(str::to_string),
        ),
        "superseded" => presentation("■", "superseded", PresentationSeverity::Exit, None),
        other => presentation("◌", other, PresentationSeverity::Front, None),
    }
}

pub fn external_review_runner_label(row: &ReviewRow) -> &str {
    let runner = row.runner.trim();
    if runner.is_empty() || runner == "unknown" {
        "—"
    } else {
        runner
    }
}

fn findings_signal(row: &ReviewRow) -> Option<String> {
    match (row.critical_count, row.major_count, row.minor_count) {
        (Some(c), Some(mj), Some(mn)) => Some(format!("{c}/{mj}/{mn} findings")),
        _ => row.findings_count.map(|n| format!("{n} findings")),
    }
}

pub fn engine_presentation(health: &SystemHealth, daemon: &Liveness) -> Presentation {
    let daemon_live = matches!(daemon, Liveness::Live { .. });
    if daemon_live {
        return presentation("✓", "clear", PresentationSeverity::Exit, None);
    }
    if health.unfinished_dispatch_locks > 0 {
        return presentation(
            "▲",
            "daemon down",
            PresentationSeverity::Fault,
            Some(format!("{} locks", health.unfinished_dispatch_locks)),
        );
    }
    presentation("△", "manual", PresentationSeverity::Wait, None)
}

pub fn engine_flow_slots(health: &SystemHealth, daemon: &Liveness) -> Vec<FlowSlotPresentation> {
    let state = engine_presentation(health, daemon);
    vec![
        FlowSlotPresentation {
            slot: PresentationSeverity::Front,
            glyph: "◌",
            label: "dispatch",
            count: Some(0),
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Work,
            glyph: "◆",
            label: "runners",
            count: Some(0),
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Gate,
            glyph: "◇",
            label: "locks",
            count: Some(health.unfinished_dispatch_locks),
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Exit,
            glyph: "✓",
            label: "clear",
            count: if state.label == "clear" {
                Some(1)
            } else {
                Some(0)
            },
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Wait,
            glyph: "△",
            label: if state.label == "manual" {
                "manual"
            } else {
                "wait"
            },
            count: None,
        },
        FlowSlotPresentation {
            slot: PresentationSeverity::Fault,
            glyph: "▲",
            label: if state.label == "daemon down" {
                "daemon down"
            } else {
                "fault"
            },
            count: if state.severity == PresentationSeverity::Fault {
                Some(1)
            } else {
                Some(0)
            },
        },
    ]
}

fn presentation(
    glyph: &'static str,
    label: impl Into<String>,
    severity: PresentationSeverity,
    signal: Option<String>,
) -> Presentation {
    Presentation {
        glyph,
        label: label.into(),
        severity,
        signal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(lifecycle: &str, active_step: &str) -> TaskRow {
        TaskRow {
            display_id: "T001".to_string(),
            status: "planning".to_string(),
            lifecycle: Some(lifecycle.to_string()),
            active_step: Some(active_step.to_string()),
            integration_step: Some("none".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn task_blocker_labels_runner_failed_and_waiting_capacity() {
        let runner = TaskRow {
            blocked: Some(true),
            blocker_kind: Some("runner".to_string()),
            blocked_reason: Some(r#"{"exit_code":42,"kind":"runner_crash"}"#.to_string()),
            lifecycle: Some("active".to_string()),
            ..Default::default()
        };
        let p = task_presentation(&runner);
        assert_eq!(
            (p.glyph, p.label.as_str(), p.severity, p.signal.as_deref()),
            (
                "▲",
                "runner-failed",
                PresentationSeverity::Fault,
                Some("exit 42")
            )
        );

        let capacity = TaskRow {
            blocked: Some(true),
            blocker_kind: Some("capacity".to_string()),
            lifecycle: Some("queued".to_string()),
            ..Default::default()
        };
        let p = task_presentation(&capacity);
        assert_eq!(
            (p.glyph, p.label.as_str(), p.severity),
            ("△", "waiting-capacity", PresentationSeverity::Wait)
        );
    }

    #[test]
    fn task_active_step_labels_match_watch_plan() {
        let cases = [
            ("active", "planning", "◆", "plan"),
            ("active", "planning_review", "◇", "plan-gate"),
            ("active", "coding", "▣", "exec"),
            ("active", "coding_review", "◈", "code-gate"),
            ("active", "wrapping", "▰", "accept"),
        ];
        for (lifecycle, step, glyph, label) in cases {
            let p = task_presentation(&task(lifecycle, step));
            assert_eq!((p.glyph, p.label.as_str()), (glyph, label));
        }
        let ship = TaskRow {
            lifecycle: Some("integration".to_string()),
            integration_step: Some("queued".to_string()),
            ..Default::default()
        };
        let p = task_presentation(&ship);
        assert_eq!((p.glyph, p.label.as_str()), ("▱", "ship"));
    }

    #[test]
    fn task_watch_projection_covers_task_slots_and_stages() {
        let cases = [
            (task("queued", "none"), WatchSlotId::Front, "queued"),
            (task("active", "planning"), WatchSlotId::Work, "plan"),
            (task("active", "coding"), WatchSlotId::Work, "exec"),
            (
                task("active", "planning_review"),
                WatchSlotId::Gate,
                "plan-gate",
            ),
            (task("active", "wrapping"), WatchSlotId::Gate, "accept"),
            (
                TaskRow {
                    blocked: Some(true),
                    blocker_kind: Some("capacity".to_string()),
                    lifecycle: Some("queued".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Wait,
                "waiting-capacity",
            ),
            (
                TaskRow {
                    blocked: Some(true),
                    blocker_kind: Some("runner".to_string()),
                    blocked_reason: Some(r#"{"exit_code":42}"#.to_string()),
                    lifecycle: Some("active".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Fault,
                "runner-failed",
            ),
            (
                TaskRow {
                    lifecycle: Some("done".to_string()),
                    ..Default::default()
                },
                WatchSlotId::Exit,
                "done",
            ),
        ];

        for (row, slot, stage) in cases {
            let projection = task_watch_projection(&row);
            assert_eq!((projection.slot, projection.row_stage), (slot, stage));
        }
    }

    #[test]
    fn observation_contract_labels_are_semantic() {
        let draft = ObsRow {
            lifecycle: Some("candidate".to_string()),
            waiting_kind: Some("human_ratification".to_string()),
            ..Default::default()
        };
        assert_eq!(observation_presentation(&draft).label, "contract-draft");

        let approved = ObsRow {
            contract_state: Some("approved".to_string()),
            ..Default::default()
        };
        assert_eq!(
            observation_presentation(&approved).label,
            "contract-approved"
        );

        let info = ObsRow {
            waiting_kind: Some("info_needed".to_string()),
            ..Default::default()
        };
        let p = observation_presentation(&info);
        assert_eq!((p.glyph, p.label.as_str()), ("⋯", "needs-info"));
    }

    #[test]
    fn intake_labels_cover_front_work_gate_exit() {
        assert_eq!(
            intake_presentation(&IntakeRow {
                lifecycle: Some("new".to_string()),
                ..Default::default()
            })
            .label,
            "new"
        );
        assert_eq!(
            intake_presentation(&IntakeRow {
                lifecycle: Some("triaging".to_string()),
                ..Default::default()
            })
            .label,
            "triage"
        );
        assert_eq!(
            intake_presentation(&IntakeRow {
                lifecycle: Some("waiting".to_string()),
                ..Default::default()
            })
            .label,
            "needs-info"
        );
        assert_eq!(
            intake_presentation(&IntakeRow {
                lifecycle: Some("closed".to_string()),
                outcome: Some("marked_duplicate".to_string()),
                ..Default::default()
            })
            .label,
            "duplicate"
        );
    }

    #[test]
    fn external_review_labels_and_unknown_runner_are_clean() {
        let pending = ReviewRow {
            status: "pending".to_string(),
            runner: "unknown".to_string(),
            ..Default::default()
        };
        let p = external_review_presentation(&pending);
        assert_eq!(
            (p.glyph, p.label.as_str(), p.signal.as_deref()),
            ("◌", "pending", Some("waiting for dispatch"))
        );
        assert_eq!(external_review_runner_label(&pending), "—");

        let tooling = ReviewRow {
            status: "tooling_held".to_string(),
            held_reason: Some("missing wrap brief".to_string()),
            ..Default::default()
        };
        assert_eq!(external_review_presentation(&tooling).label, "tool-fault");
    }

    #[test]
    fn engine_labels_manual_vs_fault() {
        let manual = engine_presentation(&SystemHealth::default(), &Liveness::Dead);
        assert_eq!(
            (manual.glyph, manual.label.as_str(), manual.severity),
            ("△", "manual", PresentationSeverity::Wait)
        );

        let fault = engine_presentation(
            &SystemHealth {
                unfinished_dispatch_locks: 2,
                oldest_claimed_at_epoch: None,
            },
            &Liveness::Dead,
        );
        assert_eq!(
            (
                fault.glyph,
                fault.label.as_str(),
                fault.severity,
                fault.signal.as_deref()
            ),
            (
                "▲",
                "daemon down",
                PresentationSeverity::Fault,
                Some("2 locks")
            )
        );
    }
}
