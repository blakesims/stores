//! T140 P3: derived `operator_disposition` classifier.
//!
//! Pure function over a tasks-row JSON value (which already includes
//! `linked_observations` as a list_fk field) plus today's date and a mockable
//! `BranchStateSource`, returning a [`Disposition`] enum that covers every
//! task bucket the audit doc names. A [`Disposition::plan_start_bucket`]
//! method maps each variant to one of the contract's five plan-start buckets
//! (`would_run | inactive | needs_operator | blocked | historical`).
//!
//! No stored column is added; this function is the canonical mapping
//! consumed by P4 (`engine plan-start`), P5 (`status`/`watch`), and P6's
//! fixture tests.
//!
//! # Scope
//!
//! Cross-store status (e.g. linked-observation lifecycle) is **out of scope**
//! for task disposition — it is an observation-bucket concern handled
//! separately if/when needed. `operator_disposition` accepts the row JSON
//! exclusively (which carries `linked_observations` as opaque ids) and never
//! reaches across stores.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;

/// Cutoff for the legacy-accepted-row classification.
///
/// Tasks with `status="accepted"` whose `accepted_at` predates this instant
/// (or for which `accepted_at` is missing) are treated as
/// [`Disposition::HistoricalTerminalLegacy`]. After the cutoff, accepted rows
/// fall into [`Disposition::DeployCeremonyPending`] because the auto-resolve
/// subscriber's `accepted` edge is the modern ceremony path.
pub const LEGACY_ACCEPTED_CUTOFF_RFC3339: &str = "2026-05-04T00:00:00Z";

/// Name-pinned exception: T081 shipped but its post-accept ceremony
/// subscriber never fired and there is no derivable signal recoverable from
/// row JSON alone, so we name-pin it as
/// [`Disposition::TerminalSuccessMissedCeremony`].
pub const TASK_T081_CEREMONY_GAP: &str = "T081";

/// Name-pinned exception: T122 is human-flagged as needs-operator-review
/// because its `accepted` status semantics became ambiguous after I033
/// contamination (drive-failed/silent-zombie residue obscured the row state),
/// so we name-pin it as [`Disposition::NeedsOperatorReview`] regardless of
/// what the raw status field reports.
pub const TASK_T122_NEEDS_OPERATOR_REVIEW: &str = "T122";

/// Lifecycle statuses the engine considers "in-flight" (a drive is
/// presumed underway and the row is mid-cycle). Mirrors
/// `framework_migrate::IN_FLIGHT_STATES` — duplicated here intentionally so
/// the disposition function does not depend on the migration crate's
/// internals.
const IN_FLIGHT_STATES: &[&str] = &["executing", "code_review", "integrating"];

/// Source of branch-merge truth. Trait so `disposition.rs` tests run without
/// spawning `git`. The default impl [`GitBranchStateSource`] shells out to
/// `git merge-base --is-ancestor`.
pub trait BranchStateSource {
    /// Return `true` if `branch` exists and has commits not yet merged into
    /// the integration target (typically `main`). Implementations are free to
    /// return `Err` for missing branches; the disposition function treats
    /// errors conservatively (defaults to "still in flight" semantics).
    fn branch_unmerged(&self, branch: &str) -> Result<bool>;
}

/// Default git-backed implementation. Uses `git merge-base --is-ancestor
/// <branch> main`: exit 0 means `branch` is an ancestor of `main` (i.e.
/// merged), exit 1 means unmerged, anything else propagates as `Err`.
pub struct GitBranchStateSource {
    /// Repository root the git invocations run from.
    pub repo: std::path::PathBuf,
    /// Target branch (typically `"main"`).
    pub target: String,
}

impl GitBranchStateSource {
    pub fn new(repo: impl Into<std::path::PathBuf>, target: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            target: target.into(),
        }
    }
}

impl BranchStateSource for GitBranchStateSource {
    fn branch_unmerged(&self, branch: &str) -> Result<bool> {
        let output = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", branch, &self.target])
            .current_dir(&self.repo)
            .output()
            .map_err(|e| {
                anyhow::anyhow!(
                    "git merge-base --is-ancestor {} {} in {}: {}",
                    branch,
                    self.target,
                    self.repo.display(),
                    e
                )
            })?;
        match output.status.code() {
            Some(0) => Ok(false), // ancestor of target == merged
            Some(1) => Ok(true),  // not an ancestor == unmerged
            _ => anyhow::bail!(
                "git merge-base --is-ancestor failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }
}

/// Operator-facing classification of a single tasks row.
///
/// Variants are exhaustive and ordered roughly from "active" to "terminal".
/// `AwaitingIntegration` and `EngineActionable` carry the row's activation
/// state inline so [`Disposition::plan_start_bucket`] can decide
/// would-run-vs-inactive without re-reading the row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind")]
pub enum Disposition {
    /// Mid-cycle row: status in `executing | code_review | integrating`.
    /// Drive is presumed underway.
    ActiveEngineWork,
    /// Finished work parked at the human acceptance gate, before integration release.
    AwaitingHumanAcceptance,
    /// Row in the integration lane (`integration_queued | integration_blocked`).
    /// Combustion gated by activation.
    AwaitingIntegration { activation_active: bool },
    /// Row that needs the daemon to drive (`planning | plan_review | ready
    /// | in_review`). Combustion gated by activation.
    EngineActionable { activation_active: bool },
    /// Post-accept ceremony pending: `complete | cargo_installed |
    /// integrated`, or post-cutoff `accepted` with the auto-resolve edge
    /// not yet fired.
    DeployCeremonyPending,
    /// Operator decision required (`deploy_blocked`, name-pinned T122, or
    /// any unknown status).
    NeedsOperatorReview,
    /// T081 name-pinned: shipped but ceremony subscriber gap left the row
    /// stranded with no derivable recovery signal.
    TerminalSuccessMissedCeremony,
    /// `status == "blocked"`: human/operator recovery needed.
    BlockedRecoverable,
    /// Legacy `accepted` row (pre-cutoff `accepted_at`, or missing). Hidden
    /// from active lanes; treated as historical exhaust.
    HistoricalTerminalLegacy,
    /// Modern terminal success (`status == "schema_migrated"`).
    TerminalSuccessModern,
    /// `status == "abandoned"`: intentionally retired.
    TerminalRetired,
    /// `status == "closed_out_of_band"`: shipped via manual commit, recorded
    /// out-of-band.
    TerminalShippedOob,
    /// `status == "rejected"`: reviewed-and-rejected on merits.
    TerminalRejected,
}

/// Plan-start bucket the contract names. Five buckets, mapped exhaustively
/// from [`Disposition::plan_start_bucket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStartBucket {
    WouldRun,
    Inactive,
    NeedsOperator,
    Blocked,
    Historical,
}

impl Disposition {
    /// Map this disposition to one of the contract's five plan-start
    /// buckets. The match is exhaustive so adding a new [`Disposition`]
    /// variant without updating this function is a compile error.
    pub fn plan_start_bucket(&self) -> PlanStartBucket {
        match self {
            Disposition::ActiveEngineWork => PlanStartBucket::WouldRun,
            Disposition::AwaitingHumanAcceptance => PlanStartBucket::NeedsOperator,
            Disposition::AwaitingIntegration { activation_active } => {
                if *activation_active {
                    PlanStartBucket::WouldRun
                } else {
                    PlanStartBucket::Inactive
                }
            }
            Disposition::EngineActionable { activation_active } => {
                if *activation_active {
                    PlanStartBucket::WouldRun
                } else {
                    PlanStartBucket::Inactive
                }
            }
            Disposition::DeployCeremonyPending
            | Disposition::NeedsOperatorReview
            | Disposition::TerminalSuccessMissedCeremony => PlanStartBucket::NeedsOperator,
            Disposition::BlockedRecoverable => PlanStartBucket::Blocked,
            Disposition::HistoricalTerminalLegacy
            | Disposition::TerminalSuccessModern
            | Disposition::TerminalRetired
            | Disposition::TerminalShippedOob
            | Disposition::TerminalRejected => PlanStartBucket::Historical,
        }
    }

    /// Short human-readable label for status/watch and plan-start text
    /// renders.
    pub fn display_label(&self) -> &'static str {
        match self {
            Disposition::ActiveEngineWork => "Active engine work",
            Disposition::AwaitingHumanAcceptance => "Awaiting human acceptance",
            Disposition::AwaitingIntegration {
                activation_active: true,
            } => "Awaiting integration (active)",
            Disposition::AwaitingIntegration {
                activation_active: false,
            } => "Awaiting integration (inactive)",
            Disposition::EngineActionable {
                activation_active: true,
            } => "Engine actionable (active)",
            Disposition::EngineActionable {
                activation_active: false,
            } => "Engine actionable (inactive)",
            Disposition::DeployCeremonyPending => "Deploy ceremony pending",
            Disposition::NeedsOperatorReview => "Needs operator review",
            Disposition::TerminalSuccessMissedCeremony => "Terminal success (missed ceremony)",
            Disposition::BlockedRecoverable => "Blocked (recoverable)",
            Disposition::HistoricalTerminalLegacy => "Historical terminal (legacy)",
            Disposition::TerminalSuccessModern => "Terminal success",
            Disposition::TerminalRetired => "Terminal retired",
            Disposition::TerminalShippedOob => "Terminal shipped (out of band)",
            Disposition::TerminalRejected => "Terminal rejected",
        }
    }
}

fn row_str<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(|v| v.as_str())
}

fn parse_iso(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Classify a single tasks row.
///
/// `row` is the row JSON exactly as the substrate exposes it (already
/// includes `linked_observations` as a list_fk array; no separate load
/// needed). `today` is the wall-clock instant the caller wants the
/// classification anchored to (passed in for test determinism, even though
/// the current rule set only consumes it transitively via the
/// [`LEGACY_ACCEPTED_CUTOFF_RFC3339`] check on `accepted_at`).
/// `branch_state` is consulted for the `in_review` branch-shipped sanity
/// refinement; tests pass [`MockBranchState`].
pub fn operator_disposition(
    row: &Value,
    _today: DateTime<Utc>,
    branch_state: &dyn BranchStateSource,
) -> Disposition {
    let display_id = row_str(row, "display_id").unwrap_or("");
    let status = row_str(row, "status").unwrap_or("");
    let activation = row_str(row, "activation").unwrap_or("inactive");
    let activation_active = activation == "active";
    let branch = row_str(row, "branch").filter(|s| !s.is_empty());

    // Name-pinned exceptions take precedence over any status-derived rule.
    if display_id == TASK_T081_CEREMONY_GAP {
        return Disposition::TerminalSuccessMissedCeremony;
    }
    if display_id == TASK_T122_NEEDS_OPERATOR_REVIEW {
        return Disposition::NeedsOperatorReview;
    }

    let lifecycle = row_str(row, "lifecycle");
    let active_step = row_str(row, "active_step").unwrap_or("none");
    let integration_step = row_str(row, "integration_step").unwrap_or("none");
    let blocked = row
        .get("blocked")
        .and_then(|v| v.as_bool().or_else(|| v.as_i64().map(|i| i != 0)))
        .unwrap_or(false);
    let blocker_kind = row_str(row, "blocker_kind").unwrap_or("none");

    if lifecycle.is_some() {
        if blocked || blocker_kind != "none" {
            return Disposition::BlockedRecoverable;
        }
        return match lifecycle.unwrap_or("active") {
            "done" => Disposition::TerminalSuccessModern,
            "integration" => {
                if matches!(integration_step, "deploying" | "verifying") {
                    Disposition::DeployCeremonyPending
                } else {
                    Disposition::AwaitingIntegration { activation_active }
                }
            }
            "queued" => Disposition::EngineActionable { activation_active },
            "active" => match active_step {
                "coding" | "coding_review" => Disposition::ActiveEngineWork,
                "wrapping" => Disposition::AwaitingHumanAcceptance,
                _ => Disposition::EngineActionable { activation_active },
            },
            _ => Disposition::NeedsOperatorReview,
        };
    }

    if IN_FLIGHT_STATES.contains(&status) {
        return Disposition::ActiveEngineWork;
    }

    match status {
        "abandoned" => Disposition::TerminalRetired,
        "closed_out_of_band" => Disposition::TerminalShippedOob,
        "rejected" => Disposition::TerminalRejected,
        "schema_migrated" => Disposition::TerminalSuccessModern,
        "blocked" => Disposition::BlockedRecoverable,
        "deploy_blocked" => Disposition::NeedsOperatorReview,
        "planning" | "plan_review" | "ready" => Disposition::EngineActionable { activation_active },
        "in_review" => {
            let acceptance_policy = row_str(row, "human_acceptance_policy").unwrap_or("optional");
            let accepted = row_str(row, "acceptance_decided_by").is_some();
            if acceptance_policy == "required" && !accepted {
                return Disposition::AwaitingHumanAcceptance;
            }
            // Sanity refinement: if the branch already shipped (merged into
            // target) but the row is still parked in_review, the operator
            // needs to disposition it. Errors and missing-branch fall back
            // to the conservative "still actionable" mapping.
            let shipped = branch
                .and_then(|b| branch_state.branch_unmerged(b).ok())
                .map(|unmerged| !unmerged)
                .unwrap_or(false);
            if shipped {
                Disposition::NeedsOperatorReview
            } else {
                Disposition::EngineActionable { activation_active }
            }
        }
        "complete" | "cargo_installed" | "integrated" => Disposition::DeployCeremonyPending,
        "accepted" => {
            // AC4.6: accepted rows still carrying an unmerged branch are in
            // the integration lane, gated by activation. Only when the branch
            // is missing or definitively merged do we fall through to the
            // legacy/ceremony classification. Err from the branch source is
            // treated conservatively as "still in the integration lane" — the
            // branch field's presence is the signal.
            if let Some(b) = branch {
                match branch_state.branch_unmerged(b) {
                    Ok(false) => {} // merged → fall through to legacy/ceremony rules
                    _ => return Disposition::AwaitingIntegration { activation_active },
                }
            }
            let cutoff = parse_iso(LEGACY_ACCEPTED_CUTOFF_RFC3339)
                .expect("LEGACY_ACCEPTED_CUTOFF_RFC3339 must be a valid RFC3339 instant");
            let accepted_at = row_str(row, "accepted_at").and_then(parse_iso);
            match accepted_at {
                Some(ts) if ts < cutoff => Disposition::HistoricalTerminalLegacy,
                None => Disposition::HistoricalTerminalLegacy,
                Some(_) => Disposition::DeployCeremonyPending,
            }
        }
        "integration_queued" | "integration_blocked" => {
            Disposition::AwaitingIntegration { activation_active }
        }
        _ => Disposition::NeedsOperatorReview,
    }
}

#[cfg(test)]
mod tests {
    //! T140 P3 fixture tests. Mirror the audit doc fixture
    //! (`docs/worklog/2026-05-09/04-manual-cleanup-triage-audit.md`) for the
    //! name-pinned and status-derived classifications, plus synthetic rows
    //! for every other [`Disposition`] variant.

    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    /// Test-only [`BranchStateSource`] backed by a string→bool map. `true`
    /// means the branch is still unmerged. Unknown branches return
    /// `Err(anyhow!("unknown branch"))` so the disposition logic exercises
    /// its conservative fallback path.
    struct MockBranchState(HashMap<String, bool>);

    impl MockBranchState {
        fn empty() -> Self {
            Self(HashMap::new())
        }
        fn with(entries: &[(&str, bool)]) -> Self {
            let mut m = HashMap::new();
            for (k, v) in entries {
                m.insert((*k).to_string(), *v);
            }
            Self(m)
        }
    }

    impl BranchStateSource for MockBranchState {
        fn branch_unmerged(&self, branch: &str) -> Result<bool> {
            self.0
                .get(branch)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("unknown branch: {branch}"))
        }
    }

    fn today() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-09T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn fixture_row(display_id: &str, status: &str) -> Value {
        json!({
            "display_id": display_id,
            "status": status,
            "activation": "inactive",
            "linked_observations": [],
            "branch": "",
        })
    }

    // ---- AC3.1 fixture-row tests (one per Disposition variant) ----

    /// Audit doc: T001–T018 are pre-substrate-T0 legacy accepted rows. The
    /// classifier folds the entire range to HistoricalTerminalLegacy via
    /// the pre-cutoff `accepted_at` rule (or missing accepted_at, which
    /// defaults to legacy).
    #[test]
    fn legacy_t001_to_t018_classify_as_historical_terminal_legacy() {
        let mock = MockBranchState::empty();
        for n in 1..=18 {
            let id = format!("T{:03}", n);
            // Half the rows carry an explicit pre-cutoff accepted_at; the
            // other half omit it, exercising both legacy paths.
            let mut row = fixture_row(&id, "accepted");
            if n % 2 == 0 {
                row["accepted_at"] = json!("2026-04-01T00:00:00Z");
            }
            let got = operator_disposition(&row, today(), &mock);
            assert_eq!(
                got,
                Disposition::HistoricalTerminalLegacy,
                "T{:03} must classify as HistoricalTerminalLegacy; got {:?}",
                n,
                got
            );
        }
    }

    #[test]
    fn t081_classifies_as_terminal_success_missed_ceremony() {
        let mock = MockBranchState::empty();
        // Even with a status that would normally derive elsewhere, the name
        // pin wins.
        let row = fixture_row("T081", "accepted");
        let got = operator_disposition(&row, today(), &mock);
        assert_eq!(got, Disposition::TerminalSuccessMissedCeremony);
    }

    #[test]
    fn t122_classifies_as_needs_operator_review() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T122", "accepted");
        let got = operator_disposition(&row, today(), &mock);
        assert_eq!(got, Disposition::NeedsOperatorReview);
    }

    #[test]
    fn t125_t127_classify_as_deploy_ceremony_pending() {
        let mock = MockBranchState::empty();
        for id in &["T125", "T127"] {
            let mut row = fixture_row(id, "accepted");
            // Post-cutoff accepted_at — the modern auto-resolve edge
            // applies.
            row["accepted_at"] = json!("2026-05-08T10:00:00Z");
            let got = operator_disposition(&row, today(), &mock);
            assert_eq!(
                got,
                Disposition::DeployCeremonyPending,
                "{} must classify as DeployCeremonyPending; got {:?}",
                id,
                got
            );
        }
    }

    #[test]
    fn t138_classifies_as_awaiting_integration() {
        // T138 is mid-integration; activation state varies by row. Verify
        // both arms.
        let mock = MockBranchState::empty();
        let mut active = fixture_row("T138", "integration_queued");
        active["activation"] = json!("active");
        assert_eq!(
            operator_disposition(&active, today(), &mock),
            Disposition::AwaitingIntegration {
                activation_active: true,
            }
        );

        let mut inactive = fixture_row("T138", "integration_blocked");
        inactive["activation"] = json!("inactive");
        assert_eq!(
            operator_disposition(&inactive, today(), &mock),
            Disposition::AwaitingIntegration {
                activation_active: false,
            }
        );
    }

    #[test]
    fn t139_classifies_as_active_engine_work() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T139", "executing");
        let got = operator_disposition(&row, today(), &mock);
        assert_eq!(got, Disposition::ActiveEngineWork);
    }

    #[test]
    fn schema_migrated_classifies_as_terminal_success_modern() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T200", "schema_migrated");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::TerminalSuccessModern
        );
    }

    #[test]
    fn abandoned_classifies_as_terminal_retired() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T201", "abandoned");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::TerminalRetired
        );
    }

    #[test]
    fn closed_out_of_band_classifies_as_terminal_shipped_oob() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T202", "closed_out_of_band");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::TerminalShippedOob
        );
    }

    #[test]
    fn blocked_classifies_as_blocked_recoverable() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T203", "blocked");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::BlockedRecoverable
        );
    }

    #[test]
    fn rejected_classifies_as_terminal_rejected() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T204", "rejected");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::TerminalRejected
        );
    }

    #[test]
    fn planning_classifies_as_engine_actionable_active() {
        let mock = MockBranchState::empty();
        let mut row = fixture_row("T205", "planning");
        row["activation"] = json!("active");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::EngineActionable {
                activation_active: true,
            }
        );
    }

    #[test]
    fn planning_classifies_as_engine_actionable_inactive() {
        let mock = MockBranchState::empty();
        let mut row = fixture_row("T206", "planning");
        row["activation"] = json!("inactive");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::EngineActionable {
                activation_active: false,
            }
        );
    }

    /// AC3.6: rows carrying linked_observations must be accepted by
    /// operator_disposition without panic and without reaching across stores
    /// to read observation state.
    #[test]
    fn linked_observations_field_is_accessible_on_input_row() {
        let mock = MockBranchState::empty();
        let mut row = fixture_row("T207", "planning");
        row["linked_observations"] = json!(["L001"]);
        // Accessing the field on the row JSON should be cheap and panic-free.
        assert_eq!(
            row["linked_observations"],
            json!(["L001"]),
            "fixture must expose linked_observations on the row"
        );
        let got = operator_disposition(&row, today(), &mock);
        assert_eq!(
            got,
            Disposition::EngineActionable {
                activation_active: false,
            },
            "linked_observations presence must not perturb the status-derived classification"
        );
    }

    // ---- AC3.2 purity ----

    #[test]
    fn operator_disposition_is_pure_repeated_calls_match() {
        let mock = MockBranchState::with(&[("feat/x", true)]);
        let mut row = fixture_row("T300", "in_review");
        row["branch"] = json!("feat/x");
        let a = operator_disposition(&row, today(), &mock);
        let b = operator_disposition(&row, today(), &mock);
        let c = operator_disposition(&row, today(), &mock);
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    // ---- AC3.4 serde::Serialize round-trip ----

    #[test]
    fn every_disposition_variant_serializes_to_json() {
        let variants = [
            Disposition::ActiveEngineWork,
            Disposition::AwaitingIntegration {
                activation_active: true,
            },
            Disposition::AwaitingIntegration {
                activation_active: false,
            },
            Disposition::EngineActionable {
                activation_active: true,
            },
            Disposition::EngineActionable {
                activation_active: false,
            },
            Disposition::DeployCeremonyPending,
            Disposition::NeedsOperatorReview,
            Disposition::TerminalSuccessMissedCeremony,
            Disposition::BlockedRecoverable,
            Disposition::HistoricalTerminalLegacy,
            Disposition::TerminalSuccessModern,
            Disposition::TerminalRetired,
            Disposition::TerminalShippedOob,
            Disposition::TerminalRejected,
        ];
        for v in &variants {
            let s = serde_json::to_string(v)
                .unwrap_or_else(|e| panic!("variant {:?} must serialize: {e}", v));
            assert!(
                s.contains("\"kind\""),
                "serialized form must include the kind tag; got {s}"
            );
        }
    }

    // ---- in_review sanity refinement using BranchStateSource ----

    #[test]
    fn in_review_with_unmerged_branch_stays_engine_actionable() {
        let mock = MockBranchState::with(&[("feat/in-flight", true)]);
        let mut row = fixture_row("T301", "in_review");
        row["branch"] = json!("feat/in-flight");
        row["activation"] = json!("active");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::EngineActionable {
                activation_active: true,
            }
        );
    }

    #[test]
    fn in_review_with_shipped_branch_routes_to_needs_operator_review() {
        let mock = MockBranchState::with(&[("feat/shipped", false)]);
        let mut row = fixture_row("T302", "in_review");
        row["branch"] = json!("feat/shipped");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::NeedsOperatorReview
        );
    }

    // ---- additional terminal/synthetic coverage ----

    #[test]
    fn deploy_blocked_classifies_as_needs_operator_review() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T303", "deploy_blocked");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::NeedsOperatorReview
        );
    }

    #[test]
    fn unknown_status_routes_to_needs_operator_review() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T304", "this-is-not-a-real-status");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::NeedsOperatorReview
        );
    }

    #[test]
    fn primary_blocked_integration_classifies_as_blocked_recoverable() {
        let mock = MockBranchState::empty();
        let mut row = fixture_row("T310", "legacy_unknown");
        row["lifecycle"] = json!("integration");
        row["active_step"] = json!("none");
        row["integration_step"] = json!("none");
        row["blocked"] = json!(true);
        row["blocker_kind"] = json!("main_red");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::BlockedRecoverable
        );
    }

    #[test]
    fn complete_classifies_as_deploy_ceremony_pending() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T305", "complete");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::DeployCeremonyPending
        );
    }

    #[test]
    fn cargo_installed_classifies_as_deploy_ceremony_pending() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T306", "cargo_installed");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::DeployCeremonyPending
        );
    }

    #[test]
    fn integrated_classifies_as_deploy_ceremony_pending() {
        let mock = MockBranchState::empty();
        let row = fixture_row("T307", "integrated");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::DeployCeremonyPending
        );
    }

    // ---- AC4.6 — accepted + unmerged branch is the integration lane ----

    /// AC4.6 (active arm): status='accepted', activation='active', branch
    /// unmerged → AwaitingIntegration { activation_active: true } → would_run.
    #[test]
    fn accepted_with_unmerged_branch_active_classifies_as_awaiting_integration_active() {
        let mock = MockBranchState::with(&[("feat/accepted-active", true)]);
        let mut row = fixture_row("T308", "accepted");
        row["activation"] = json!("active");
        row["branch"] = json!("feat/accepted-active");
        // Even with a post-cutoff accepted_at, the unmerged branch keeps the
        // row in the integration lane rather than DeployCeremonyPending.
        row["accepted_at"] = json!("2026-05-08T10:00:00Z");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::AwaitingIntegration {
                activation_active: true,
            }
        );
    }

    /// AC4.6 (inactive arm): status='accepted', activation='inactive', branch
    /// unmerged → AwaitingIntegration { activation_active: false } → inactive.
    #[test]
    fn accepted_with_unmerged_branch_inactive_classifies_as_awaiting_integration_inactive() {
        let mock = MockBranchState::with(&[("feat/accepted-inactive", true)]);
        let mut row = fixture_row("T309", "accepted");
        row["activation"] = json!("inactive");
        row["branch"] = json!("feat/accepted-inactive");
        row["accepted_at"] = json!("2026-05-08T10:00:00Z");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::AwaitingIntegration {
                activation_active: false,
            }
        );
    }

    /// Accepted row whose branch is definitively merged falls through to the
    /// existing legacy/ceremony classification (post-cutoff →
    /// DeployCeremonyPending here).
    #[test]
    fn accepted_with_merged_branch_falls_through_to_ceremony_or_legacy() {
        let mock = MockBranchState::with(&[("feat/shipped", false)]);
        let mut row = fixture_row("T310", "accepted");
        row["branch"] = json!("feat/shipped");
        row["accepted_at"] = json!("2026-05-08T10:00:00Z");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::DeployCeremonyPending
        );
    }

    /// Accepted row with empty branch field is unaffected by the new path.
    /// Pre-cutoff accepted_at → HistoricalTerminalLegacy.
    #[test]
    fn accepted_with_empty_branch_uses_legacy_cutoff_logic() {
        let mock = MockBranchState::empty();
        let mut row = fixture_row("T311", "accepted");
        row["accepted_at"] = json!("2026-04-01T00:00:00Z");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::HistoricalTerminalLegacy
        );
    }

    /// Accepted row with a non-empty branch but the branch source erroring
    /// (e.g. not a git repo) is treated conservatively as still in the
    /// integration lane — the branch field's presence is the signal.
    #[test]
    fn accepted_with_branch_source_error_falls_into_integration_lane() {
        let mock = MockBranchState::empty(); // unknown branch → Err
        let mut row = fixture_row("T312", "accepted");
        row["activation"] = json!("inactive");
        row["branch"] = json!("feat/unknown-to-mock");
        assert_eq!(
            operator_disposition(&row, today(), &mock),
            Disposition::AwaitingIntegration {
                activation_active: false,
            }
        );
    }

    // ---- plan_start_bucket() coverage ----

    #[test]
    fn plan_start_bucket_maps_active_engine_work_to_would_run() {
        assert_eq!(
            Disposition::ActiveEngineWork.plan_start_bucket(),
            PlanStartBucket::WouldRun
        );
    }

    #[test]
    fn plan_start_bucket_maps_awaiting_integration_active_to_would_run() {
        assert_eq!(
            Disposition::AwaitingIntegration {
                activation_active: true,
            }
            .plan_start_bucket(),
            PlanStartBucket::WouldRun
        );
    }

    #[test]
    fn plan_start_bucket_maps_awaiting_integration_inactive_to_inactive() {
        assert_eq!(
            Disposition::AwaitingIntegration {
                activation_active: false,
            }
            .plan_start_bucket(),
            PlanStartBucket::Inactive
        );
    }

    #[test]
    fn plan_start_bucket_maps_engine_actionable_active_to_would_run() {
        assert_eq!(
            Disposition::EngineActionable {
                activation_active: true,
            }
            .plan_start_bucket(),
            PlanStartBucket::WouldRun
        );
    }

    #[test]
    fn plan_start_bucket_maps_engine_actionable_inactive_to_inactive() {
        assert_eq!(
            Disposition::EngineActionable {
                activation_active: false,
            }
            .plan_start_bucket(),
            PlanStartBucket::Inactive
        );
    }

    #[test]
    fn plan_start_bucket_maps_ceremony_and_review_variants_to_needs_operator() {
        for v in [
            Disposition::DeployCeremonyPending,
            Disposition::NeedsOperatorReview,
            Disposition::TerminalSuccessMissedCeremony,
        ] {
            assert_eq!(v.plan_start_bucket(), PlanStartBucket::NeedsOperator);
        }
    }

    #[test]
    fn plan_start_bucket_maps_blocked_to_blocked() {
        assert_eq!(
            Disposition::BlockedRecoverable.plan_start_bucket(),
            PlanStartBucket::Blocked
        );
    }

    #[test]
    fn plan_start_bucket_maps_terminal_variants_to_historical() {
        for v in [
            Disposition::HistoricalTerminalLegacy,
            Disposition::TerminalSuccessModern,
            Disposition::TerminalRetired,
            Disposition::TerminalShippedOob,
            Disposition::TerminalRejected,
        ] {
            assert_eq!(v.plan_start_bucket(), PlanStartBucket::Historical);
        }
    }

    // ---- display_label() ----

    #[test]
    fn display_label_distinguishes_active_vs_inactive_for_dual_variants() {
        assert_eq!(
            Disposition::AwaitingIntegration {
                activation_active: true,
            }
            .display_label(),
            "Awaiting integration (active)"
        );
        assert_eq!(
            Disposition::AwaitingIntegration {
                activation_active: false,
            }
            .display_label(),
            "Awaiting integration (inactive)"
        );
        assert_eq!(
            Disposition::EngineActionable {
                activation_active: true,
            }
            .display_label(),
            "Engine actionable (active)"
        );
        assert_eq!(
            Disposition::EngineActionable {
                activation_active: false,
            }
            .display_label(),
            "Engine actionable (inactive)"
        );
    }
}
