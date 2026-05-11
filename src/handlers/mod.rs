pub mod activate;
pub mod add;
pub mod agent_run_telemetry_backfill;
pub mod agents_backfill;
pub mod agents_run;
pub mod agents_stop;
pub mod architecture_reviews;
pub mod architecture_reviews_backfill;
pub mod brief;
pub mod brief_contracts;
pub mod cluster_keys;
pub mod disposition;
pub mod drive;
pub mod external_review_run;
pub mod external_reviews;
pub mod framework_migrate;
pub mod guide;
pub mod intake_route;
pub mod lifecycle_overlay;
pub mod list;
pub mod migrate;
pub mod next_action;
pub mod next_id;
pub mod observation_arch_gate;
pub(crate) mod observations_source;
pub mod overrides;
pub mod reconcile_accepted;
pub mod recover_stale_base;
pub mod render;
pub mod resource_locks;
pub mod row;
pub mod schema_show;
pub mod show;
pub mod status;
pub mod submit;
pub mod transition;
pub mod upstream_overlay;
pub mod update;

// T140 P3: re-export the derived operator_disposition surface for P4/P5/P6.
pub use disposition::{operator_disposition, BranchStateSource, Disposition, PlanStartBucket};

/// Canonical predicate for whether a task row is "blocked".
///
/// # Bug 1 fix (T005-P1)
///
/// Prior to this helper, `status.rs:160` tested
/// `task.blocked_reason.is_some() || task.status == "blocked"`.
/// The DB historically writes `""` (empty string) for never-blocked rows, so
/// `Option::is_some()` returned `true` for `Some("")`, causing `status` to
/// report `blocked=true` while `next-action` (which only checked
/// `status == "blocked"`) correctly reported `blocked=false`.
///
/// # Canonical interpretation
///
/// A row is blocked **if and only if** its workflow `status` field equals
/// `"blocked"`.  A non-empty `blocked_reason` is a *description* attached to a
/// blocked row, not the predicate itself.  Empty-string `""` and `None` are
/// both treated as "not blocked" — they are historical artefacts of the DB
/// defaulting the column to `""` rather than `NULL`.
pub fn is_blocked(status: &str, _blocked_reason: Option<&str>) -> bool {
    status == "blocked"
}
