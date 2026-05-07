pub mod add;
pub mod agents_backfill;
pub mod agents_run;
pub mod architecture_reviews;
pub mod brief;
pub mod drive;
pub mod framework_migrate;
pub mod guide;
pub mod intake_route;
pub mod list;
pub mod migrate;
pub mod next_action;
pub mod next_id;
pub mod overrides;
pub mod render;
pub mod row;
pub mod schema_show;
pub mod show;
pub mod status;
pub mod submit;
pub mod transition;
pub mod update;

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
