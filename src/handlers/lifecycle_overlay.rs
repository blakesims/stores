//! Deterministic task lifecycle-overlay derivation for T144 P1.
//!
//! The table below mirrors `stores/tasks/schema.yaml:lifecycle.transitions`:
//! planning/plan_review/ready/executing/code_review are the active-lane rows;
//! blocked is reached from plan_review/code_review and drive-failure rows;
//! complete/in_review/accepted/integration_queued/integrating/integration_blocked
//! are the integration-lane rows; rejected/integrated/cargo_installed/
//! schema_migrated/closed_out_of_band/abandoned are terminal/done rows;
//! deploy_blocked is the deployment blocker row. Blocked-kind derivation rows
//! correspond to the schema comments around submit-review fallback/FAIL,
//! submit-plan-review fallback/NOT_READY, and mark_drive_failed transitions.

use anyhow::{bail, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleOverlay {
    pub lifecycle: String,
    pub active_step: String,
    pub integration_step: String,
    pub blocked: bool,
    pub blocker_kind: Option<String>,
    pub legacy_status: Option<String>,
}

fn overlay(
    lifecycle: &str,
    active_step: &str,
    integration_step: &str,
    blocked: bool,
    blocker_kind: Option<String>,
) -> LifecycleOverlay {
    LifecycleOverlay {
        lifecycle: lifecycle.to_string(),
        active_step: active_step.to_string(),
        integration_step: integration_step.to_string(),
        blocked,
        blocker_kind,
        legacy_status: None,
    }
}

pub fn derive(
    verb: &str,
    from_status: &str,
    to_status: &str,
    blocked_reason: Option<&str>,
    integration_blocked_reason: Option<&str>,
) -> Result<LifecycleOverlay> {
    if verb == "backfill_queued" {
        let mut out = match to_status {
            "blocked" => overlay(
                "queued",
                "none",
                "none",
                true,
                derive_blocked_kind(verb, from_status, blocked_reason),
            ),
            "deploy_blocked" => overlay("queued", "none", "none", true, Some("deploy".into())),
            "integration_blocked" => overlay(
                "queued",
                "none",
                "none",
                true,
                derive_integration_blocker_kind(integration_blocked_reason),
            ),
            "planning" | "plan_review" | "ready" | "complete" | "in_review" | "accepted"
            | "rejected" | "integration_queued" | "integrated" | "cargo_installed"
            | "schema_migrated" | "closed_out_of_band" | "abandoned" => {
                overlay("queued", "none", "none", false, None)
            }
            other => bail!("unknown task to_status for queued lifecycle backfill: {other}"),
        };
        out.legacy_status = Some(to_status.to_string());
        return Ok(out);
    }

    let mut out = match (verb, to_status) {
        ("create", "planning") => overlay("queued", "none", "none", false, None),
        ("start-integration", "integrating") => {
            overlay("integration", "none", "refreshing", false, None)
        }
        ("mark_refresh_done", "integrating") => {
            overlay("integration", "none", "task_review", false, None)
        }
        ("mark_task_review_done", "integrating") => {
            overlay("integration", "none", "testing", false, None)
        }
        ("mark_testing_done", "integrating") => {
            overlay("integration", "none", "merging", false, None)
        }
        ("mark_merge_done", "integrating") => {
            overlay("integration", "none", "deploying", false, None)
        }
        ("mark_deploy_done", "integrating") => {
            overlay("integration", "none", "verifying", false, None)
        }
        (_, "planning") => overlay("active", "planning", "none", false, None),
        (_, "plan_review") => overlay("active", "planning_review", "none", false, None),
        (_, "ready") => overlay("active", "none", "none", false, None),
        (_, "executing") => overlay("active", "coding", "none", false, None),
        (_, "code_review") => overlay("active", "coding_review", "none", false, None),
        (_, "blocked") => overlay(
            "active",
            "none",
            "none",
            true,
            derive_blocked_kind(verb, from_status, blocked_reason),
        ),
        (_, "complete") => overlay("integration", "wrapping", "none", false, None),
        (_, "in_review") => overlay("integration", "wrapping", "none", false, None),
        (_, "accepted") => overlay("integration", "none", "none", false, None),
        (_, "rejected") => overlay("done", "none", "none", false, None),
        (_, "deploy_blocked") => {
            overlay("integration", "none", "none", true, Some("deploy".into()))
        }
        (_, "integration_queued") => overlay("integration", "none", "none", false, None),
        (_, "integrating") => overlay("integration", "none", "merging", false, None),
        (_, "integration_blocked") => overlay(
            "integration",
            "none",
            "none",
            true,
            derive_integration_blocker_kind(integration_blocked_reason),
        ),
        (_, "integrated") => overlay("done", "none", "none", false, None),
        (_, "cargo_installed") => overlay("done", "none", "none", false, None),
        (_, "schema_migrated") => overlay("done", "none", "none", false, None),
        (_, "closed_out_of_band") => overlay("done", "none", "none", false, None),
        (_, "abandoned") => overlay("done", "none", "none", false, None),
        (_, other) => bail!("unknown task to_status for lifecycle overlay: {other}"),
    };
    out.legacy_status = Some(to_status.to_string());
    Ok(out)
}

pub fn legacy(overlay: &LifecycleOverlay) -> Result<String> {
    if let Some(status) = overlay.legacy_status.as_ref() {
        return Ok(status.clone());
    }
    match (
        overlay.lifecycle.as_str(),
        overlay.active_step.as_str(),
        overlay.integration_step.as_str(),
        overlay.blocked,
        overlay.blocker_kind.as_deref(),
    ) {
        ("queued", "none", "none", false, None) => Ok("planning".into()),
        ("active", "planning", "none", false, None) => Ok("planning".into()),
        ("active", "planning_review", "none", false, None) => Ok("plan_review".into()),
        ("active", "none", "none", false, None) => Ok("ready".into()),
        ("active", "coding", "none", false, None) => Ok("executing".into()),
        ("active", "coding_review", "none", false, None) => Ok("code_review".into()),
        ("active", "none", "none", true, _) => Ok("blocked".into()),
        ("integration", "wrapping", "none", false, None) => Ok("complete".into()),
        ("integration", "none", "none", false, None) => Ok("accepted".into()),
        ("integration", "none", "none", true, Some("deploy")) => Ok("deploy_blocked".into()),
        ("integration", "none", "refreshing", false, None)
        | ("integration", "none", "task_review", false, None)
        | ("integration", "none", "testing", false, None)
        | ("integration", "none", "merging", false, None)
        | ("integration", "none", "deploying", false, None)
        | ("integration", "none", "verifying", false, None) => Ok("integrating".into()),
        ("integration", "none", "none", true, _) => Ok("integration_blocked".into()),
        ("done", "none", "none", false, None) => Ok("integrated".into()),
        _ => bail!(
            "no legacy status projection for lifecycle overlay: {:?}",
            overlay
        ),
    }
}

fn derive_blocked_kind(
    verb: &str,
    from_status: &str,
    blocked_reason: Option<&str>,
) -> Option<String> {
    let reason = blocked_reason.unwrap_or("");
    if reason.contains("rate_limit") {
        return Some("rate_limit".to_string());
    }
    if reason.contains("silent_zombie") || verb == "mark_drive_failed" {
        return Some("runner".to_string());
    }
    if (verb == "submit-review" && from_status == "code_review")
        || (verb == "submit-plan-review" && from_status == "plan_review")
    {
        return Some("task_review".to_string());
    }
    Some("runner".to_string())
}

fn derive_integration_blocker_kind(integration_blocked_reason: Option<&str>) -> Option<String> {
    let reason = integration_blocked_reason.unwrap_or("");
    if reason.starts_with("rebase_conflict")
        || reason.starts_with("stale_base")
        || reason.starts_with("stale_external_review")
    {
        Some("stale_base".to_string())
    } else if reason.starts_with("pre_land_check_failed") {
        Some("test_failure".to_string())
    } else {
        Some("main_red".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tuple(o: LifecycleOverlay) -> (String, String, String, bool, Option<String>) {
        (
            o.lifecycle,
            o.active_step,
            o.integration_step,
            o.blocked,
            o.blocker_kind,
        )
    }

    #[test]
    fn full_status_matrix() {
        let cases = vec![
            ("planning", ("active", "planning", "none", false, None)),
            (
                "plan_review",
                ("active", "planning_review", "none", false, None),
            ),
            ("ready", ("active", "none", "none", false, None)),
            ("executing", ("active", "coding", "none", false, None)),
            (
                "code_review",
                ("active", "coding_review", "none", false, None),
            ),
            ("blocked", ("active", "none", "none", true, Some("runner"))),
            ("complete", ("integration", "wrapping", "none", false, None)),
            (
                "in_review",
                ("integration", "wrapping", "none", false, None),
            ),
            ("accepted", ("integration", "none", "none", false, None)),
            ("rejected", ("done", "none", "none", false, None)),
            (
                "deploy_blocked",
                ("integration", "none", "none", true, Some("deploy")),
            ),
            (
                "integration_queued",
                ("integration", "none", "none", false, None),
            ),
            (
                "integrating",
                ("integration", "none", "merging", false, None),
            ),
            (
                "integration_blocked",
                ("integration", "none", "none", true, Some("main_red")),
            ),
            ("integrated", ("done", "none", "none", false, None)),
            ("cargo_installed", ("done", "none", "none", false, None)),
            ("schema_migrated", ("done", "none", "none", false, None)),
            ("closed_out_of_band", ("done", "none", "none", false, None)),
            ("abandoned", ("done", "none", "none", false, None)),
        ];
        for (status, expected) in cases {
            let got = tuple(derive("backfill", "", status, None, None).unwrap());
            assert_eq!(
                got,
                (
                    expected.0.into(),
                    expected.1.into(),
                    expected.2.into(),
                    expected.3,
                    expected.4.map(str::to_string)
                ),
                "{status}"
            );
        }
    }

    #[test]
    fn deploy_blocked_maps_to_integration_with_deploy_kind() {
        assert_eq!(
            tuple(derive("x", "integrated", "deploy_blocked", None, None).unwrap()),
            (
                "integration".into(),
                "none".into(),
                "none".into(),
                true,
                Some("deploy".into())
            )
        );
    }

    #[test]
    fn integrating_maps_to_merging_step() {
        assert_eq!(
            derive("x", "integration_queued", "integrating", None, None)
                .unwrap()
                .integration_step,
            "merging"
        );
    }

    #[test]
    fn blocked_via_drive_failed_maps_to_runner() {
        assert_eq!(
            derive("mark_drive_failed", "executing", "blocked", None, None)
                .unwrap()
                .blocker_kind,
            Some("runner".into())
        );
    }

    #[test]
    fn blocked_via_review_fail_maps_to_task_review() {
        assert_eq!(
            derive("submit-review", "code_review", "blocked", None, None)
                .unwrap()
                .blocker_kind,
            Some("task_review".into())
        );
    }

    #[test]
    fn blocked_via_plan_review_fallback_maps_to_task_review() {
        assert_eq!(
            derive("submit-plan-review", "plan_review", "blocked", None, None)
                .unwrap()
                .blocker_kind,
            Some("task_review".into())
        );
    }

    #[test]
    fn blocked_via_rate_limit_reason_maps_to_rate_limit() {
        assert_eq!(
            derive(
                "x",
                "executing",
                "blocked",
                Some("terminal_reason=rate_limit"),
                None
            )
            .unwrap()
            .blocker_kind,
            Some("rate_limit".into())
        );
    }

    #[test]
    fn integration_rebase_conflict_maps_to_stale_base() {
        assert_eq!(
            derive(
                "x",
                "integrating",
                "integration_blocked",
                None,
                Some("rebase_conflict: x")
            )
            .unwrap()
            .blocker_kind,
            Some("stale_base".into())
        );
    }

    #[test]
    fn integration_stale_base_maps_to_stale_base() {
        assert_eq!(
            derive(
                "x",
                "integrating",
                "integration_blocked",
                None,
                Some("stale_base: x")
            )
            .unwrap()
            .blocker_kind,
            Some("stale_base".into())
        );
    }

    #[test]
    fn integration_stale_external_review_maps_to_stale_base() {
        assert_eq!(
            derive(
                "x",
                "integrating",
                "integration_blocked",
                None,
                Some("stale_external_review: x")
            )
            .unwrap()
            .blocker_kind,
            Some("stale_base".into())
        );
    }

    #[test]
    fn integration_pre_land_check_failed_maps_to_test_failure() {
        assert_eq!(
            derive(
                "x",
                "integrating",
                "integration_blocked",
                None,
                Some("pre_land_check_failed: x")
            )
            .unwrap()
            .blocker_kind,
            Some("test_failure".into())
        );
    }

    #[test]
    fn integration_merge_failure_maps_to_main_red() {
        assert_eq!(
            derive(
                "x",
                "integrating",
                "integration_blocked",
                None,
                Some("merge_failure: x")
            )
            .unwrap()
            .blocker_kind,
            Some("main_red".into())
        );
    }

    #[test]
    fn integration_push_failure_maps_to_main_red() {
        assert_eq!(
            derive(
                "x",
                "integrating",
                "integration_blocked",
                None,
                Some("push_failure: x")
            )
            .unwrap()
            .blocker_kind,
            Some("main_red".into())
        );
    }
}
