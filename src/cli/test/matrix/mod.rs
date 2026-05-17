#![allow(dead_code)]

use anyhow::{bail, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Catalog {
    Smoke,
    Full,
}

impl Catalog {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "smoke" => Ok(Self::Smoke),
            "full" => Ok(Self::Full),
            other => bail!("unknown stores test catalog '{other}' (expected smoke|full)"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnumerateOpts {
    pub catalog: Catalog,
    pub coverage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoverageTags {
    pub schema_edges: Vec<&'static str>,
    pub runner_outcomes: Vec<&'static str>,
    pub perturbations: Vec<&'static str>,
    pub authority_events: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraversalSpec {
    pub id: &'static str,
    pub family: &'static str,
    pub description: &'static str,
    pub expected: &'static str,
    pub coverage: CoverageTags,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct VisitedEdge {
    pub from_status: Option<String>,
    pub to_status: Option<String>,
    pub lifecycle_from: Option<String>,
    pub lifecycle_to: Option<String>,
    pub active_step_from: Option<String>,
    pub active_step_to: Option<String>,
    pub integration_step_from: Option<String>,
    pub integration_step_to: Option<String>,
    pub verb: Option<String>,
    pub invoker: Option<String>,
}

impl VisitedEdge {
    pub fn from_to(from_status: &str, to_status: &str) -> Self {
        Self {
            from_status: Some(from_status.to_string()),
            to_status: Some(to_status.to_string()),
            ..Self::default()
        }
    }

    pub fn with_verb(mut self, verb: &str) -> Self {
        self.verb = Some(verb.to_string());
        self
    }

    pub fn with_integration_step(mut self, from: &str, to: &str) -> Self {
        self.integration_step_from = Some(from.to_string());
        self.integration_step_to = Some(to.to_string());
        self
    }

    pub fn with_lifecycle(mut self, from: &str, to: &str) -> Self {
        self.lifecycle_from = Some(from.to_string());
        self.lifecycle_to = Some(to.to_string());
        self
    }

    pub fn with_active_step(mut self, from: &str, to: &str) -> Self {
        self.active_step_from = Some(from.to_string());
        self.active_step_to = Some(to.to_string());
        self
    }

    pub fn with_invoker(mut self, invoker: &str) -> Self {
        self.invoker = Some(invoker.to_string());
        self
    }

    pub fn matches(&self, row: &TransitionHistoryRow) -> bool {
        opt_matches(&self.from_status, &row.from_status)
            && opt_matches(&self.to_status, &row.to_status)
            && opt_matches(&self.lifecycle_from, &row.lifecycle_from)
            && opt_matches(&self.lifecycle_to, &row.lifecycle_to)
            && opt_matches(&self.active_step_from, &row.active_step_from)
            && opt_matches(&self.active_step_to, &row.active_step_to)
            && opt_matches(&self.integration_step_from, &row.integration_step_from)
            && opt_matches(&self.integration_step_to, &row.integration_step_to)
            && opt_matches(&self.verb, &row.verb)
            && opt_matches(&self.invoker, &row.invoker)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TransitionHistoryRow {
    pub from_status: Option<String>,
    pub to_status: Option<String>,
    pub lifecycle_from: Option<String>,
    pub lifecycle_to: Option<String>,
    pub active_step_from: Option<String>,
    pub active_step_to: Option<String>,
    pub integration_step_from: Option<String>,
    pub integration_step_to: Option<String>,
    pub verb: Option<String>,
    pub invoker: Option<String>,
}

impl TransitionHistoryRow {
    pub fn from_to(from_status: &str, to_status: &str, verb: &str) -> Self {
        Self {
            from_status: Some(from_status.to_string()),
            to_status: Some(to_status.to_string()),
            verb: Some(verb.to_string()),
            ..Self::default()
        }
    }

    pub fn with_integration_step(mut self, from: &str, to: &str) -> Self {
        self.integration_step_from = Some(from.to_string());
        self.integration_step_to = Some(to.to_string());
        self
    }

    pub fn with_lifecycle(mut self, from: &str, to: &str) -> Self {
        self.lifecycle_from = Some(from.to_string());
        self.lifecycle_to = Some(to.to_string());
        self
    }

    pub fn with_active_step(mut self, from: &str, to: &str) -> Self {
        self.active_step_from = Some(from.to_string());
        self.active_step_to = Some(to.to_string());
        self
    }

    pub fn with_invoker(mut self, invoker: &str) -> Self {
        self.invoker = Some(invoker.to_string());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisitedMatch {
    Skipped,
    Matched,
    Missing {
        expected_index: usize,
        expected: VisitedEdge,
    },
}

pub fn match_visited_subsequence(
    expected: Option<&[VisitedEdge]>,
    rows: &[TransitionHistoryRow],
) -> VisitedMatch {
    let Some(expected) = expected else {
        return VisitedMatch::Skipped;
    };
    let mut row_start = 0usize;
    for (expected_index, edge) in expected.iter().enumerate() {
        let mut found = None;
        for (row_index, row) in rows.iter().enumerate().skip(row_start) {
            if edge.matches(row) {
                found = Some(row_index);
                break;
            }
        }
        match found {
            Some(row_index) => row_start = row_index + 1,
            None => {
                return VisitedMatch::Missing {
                    expected_index,
                    expected: edge.clone(),
                }
            }
        }
    }
    VisitedMatch::Matched
}

pub fn catalog_specs(catalog: Catalog) -> Vec<TraversalSpec> {
    let mut specs = smoke_specs();
    if catalog == Catalog::Full {
        specs.extend(full_extra_specs());
    }
    specs
}

pub fn run_enumerate(opts: EnumerateOpts) -> Result<()> {
    let specs = catalog_specs(opts.catalog);
    println!(
        "stores test catalog={} cases={}",
        opts.catalog.as_str(),
        specs.len()
    );
    for spec in &specs {
        println!(
            "{}\t{}\t{}\t{}",
            spec.id, spec.family, spec.expected, spec.description
        );
        if opts.coverage {
            print_coverage("schema", &spec.coverage.schema_edges);
            print_coverage("runner", &spec.coverage.runner_outcomes);
            print_coverage("perturbation", &spec.coverage.perturbations);
            print_coverage("authority", &spec.coverage.authority_events);
        }
    }
    Ok(())
}

fn print_coverage(label: &str, tags: &[&str]) {
    if tags.is_empty() {
        println!("  {label}: -");
    } else {
        println!("  {label}: {}", tags.join(","));
    }
}

fn smoke_specs() -> Vec<TraversalSpec> {
    vec![
        TraversalSpec {
            id: "T3-hp-with-substeps",
            family: "happy",
            description: "T3 happy path with explicit integration substep coverage",
            expected: "integrated/done",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:planning:submit-plan:plan_review",
                    "tasks:plan_review:submit-plan-review:ready",
                    "tasks:ready:start-execution:executing",
                    "tasks:executing:submit-code:code_review",
                    "tasks:code_review:submit-code-review:complete",
                    "tasks:complete:release-to-integration:integration_queued",
                    "tasks:integrating:mark_refresh_done:integrating/task_review",
                    "tasks:integrating:mark_task_review_done:integrating/testing",
                    "tasks:integrating:mark_testing_done:integrating/merging",
                    "tasks:integrating:mark_merge_done:integrating/deploying",
                    "tasks:integrating:mark_deploy_done:integrated",
                ],
                runner_outcomes: vec![
                    "planner:valid_plan_3_phase",
                    "plan_reviewer:READY",
                    "executor:marker_commit",
                    "code_reviewer:PASS",
                    "wrap:PASS",
                    "external_review:PASS",
                ],
                perturbations: vec![],
                authority_events: vec!["task:accept"],
            },
        },
        TraversalSpec {
            id: "T3-pr1",
            family: "plan-review-loop",
            description: "plan reviewer returns NEEDS_WORK once, then READY",
            expected: "integrated/done",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:plan_review:submit-plan-review:planning/NEEDS_WORK",
                    "tasks:plan_review:submit-plan-review:ready/READY",
                ],
                runner_outcomes: vec!["plan_reviewer:NEEDS_WORK", "plan_reviewer:READY"],
                perturbations: vec![],
                authority_events: vec!["task:accept"],
            },
        },
        TraversalSpec {
            id: "T3-cr1",
            family: "code-review-loop",
            description: "code reviewer returns REVISE once, then PASS",
            expected: "integrated/done",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:code_review:submit-code-review:executing/REVISE",
                    "tasks:code_review:submit-code-review:complete/PASS",
                ],
                runner_outcomes: vec!["code_reviewer:REVISE", "code_reviewer:PASS"],
                perturbations: vec![],
                authority_events: vec!["task:accept"],
            },
        },
        TraversalSpec {
            id: "T3-er-tooling",
            family: "external-review",
            description: "fake external review tooling-held is contained in review state",
            expected: "in_review/tooling_held",
            coverage: CoverageTags {
                schema_edges: vec!["external_reviews:running:submit-external-review:tooling_held"],
                runner_outcomes: vec!["external_review:TOOLING_FAILURE"],
                perturbations: vec![],
                authority_events: vec![],
            },
        },
        TraversalSpec {
            id: "git-stale-base-refuses",
            family: "git-freshness",
            description: "main advances after fake ER PASS; integration must refuse freshness",
            expected: "freshness-refusal",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:in_review:accept:accepted",
                    "tasks:accepted:enqueue-integration:integration_queued",
                    "tasks:integrating:freshness-refusal:integration_blocked",
                ],
                runner_outcomes: vec!["external_review:PASS"],
                perturbations: vec!["git:advance_main_after_er_pass"],
                authority_events: vec!["task:accept"],
            },
        },
    ]
}

fn full_extra_specs() -> Vec<TraversalSpec> {
    vec![
        TraversalSpec {
            id: "T3-pr-not-ready",
            family: "plan-review-loop",
            description: "plan reviewer returns NOT_READY hard block",
            expected: "blocked/task_review",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:plan_review:submit-plan-review:blocked/NOT_READY"],
                runner_outcomes: vec!["plan_reviewer:NOT_READY"],
                perturbations: vec![],
                authority_events: vec![],
            },
        },
        TraversalSpec {
            id: "T3-cr-fail",
            family: "code-review-loop",
            description: "code reviewer returns FAIL hard block",
            expected: "blocked/task_review",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:code_review:submit-code-review:blocked/FAIL"],
                runner_outcomes: vec!["code_reviewer:FAIL"],
                perturbations: vec![],
                authority_events: vec![],
            },
        },
        TraversalSpec {
            id: "T3-er-revise-from-blocked-runner",
            family: "external-review",
            description: "external review REVISE recovers a blocked task to execution",
            expected: "executing/recovery",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:blocked:submit-external-review:executing/REVISE"],
                runner_outcomes: vec!["external_review:REVISE"],
                perturbations: vec!["runner:block_then_er_revise"],
                authority_events: vec![],
            },
        },
        TraversalSpec {
            id: "T3-hp-delegated-policy",
            family: "happy",
            description:
                "delegated acceptance policy releases complete task directly to integration queue",
            expected: "integrated/done",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:complete:release-to-integration:integration_queued"],
                runner_outcomes: vec!["wrap:PASS", "external_review:PASS"],
                perturbations: vec![],
                authority_events: vec!["policy:delegated_acceptance"],
            },
        },
        TraversalSpec {
            id: "T2-multi-phase-rejected",
            family: "plan-shape",
            description: "T2 rejects planner output with more than one phase",
            expected: "blocked_or_plan_review_rejection",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:planning:submit-plan:shape_rejected/T2"],
                runner_outcomes: vec!["planner:valid_plan_3_phase"],
                perturbations: vec![],
                authority_events: vec![],
            },
        },
    ]
}

fn opt_matches(expected: &Option<String>, actual: &Option<String>) -> bool {
    match expected {
        Some(expected) => actual.as_deref() == Some(expected.as_str()),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_visited_skips_path_check() {
        assert_eq!(match_visited_subsequence(None, &[]), VisitedMatch::Skipped);
    }

    #[test]
    fn ordered_subsequence_matches_with_gaps() {
        let rows = vec![
            TransitionHistoryRow::from_to("planning", "plan_review", "submit-plan"),
            TransitionHistoryRow::from_to("plan_review", "planning", "submit-plan-review"),
            TransitionHistoryRow::from_to("planning", "plan_review", "submit-plan"),
            TransitionHistoryRow::from_to("plan_review", "ready", "submit-plan-review"),
        ];
        let expected = vec![
            VisitedEdge::from_to("plan_review", "planning").with_verb("submit-plan-review"),
            VisitedEdge::from_to("plan_review", "ready").with_verb("submit-plan-review"),
        ];
        assert_eq!(
            match_visited_subsequence(Some(&expected), &rows),
            VisitedMatch::Matched
        );
    }

    #[test]
    fn ordered_subsequence_reports_first_missing_edge() {
        let rows = vec![TransitionHistoryRow::from_to(
            "planning",
            "plan_review",
            "submit-plan",
        )];
        let expected = vec![
            VisitedEdge::from_to("planning", "plan_review").with_verb("submit-plan"),
            VisitedEdge::from_to("plan_review", "ready").with_verb("submit-plan-review"),
        ];
        assert!(matches!(
            match_visited_subsequence(Some(&expected), &rows),
            VisitedMatch::Missing {
                expected_index: 1,
                ..
            }
        ));
    }

    #[test]
    fn integration_step_fields_participate_in_matching() {
        let rows = vec![
            TransitionHistoryRow::from_to("integrating", "integrating", "mark_refresh_done")
                .with_integration_step("refreshing", "task_review"),
            TransitionHistoryRow::from_to("integrating", "integrating", "mark_task_review_done")
                .with_integration_step("task_review", "testing"),
        ];
        let expected = vec![VisitedEdge::from_to("integrating", "integrating")
            .with_verb("mark_task_review_done")
            .with_integration_step("task_review", "testing")];
        assert_eq!(
            match_visited_subsequence(Some(&expected), &rows),
            VisitedMatch::Matched
        );
    }

    #[test]
    fn lifecycle_fields_participate_in_matching() {
        let rows = vec![
            TransitionHistoryRow::from_to("complete", "in_review", "request-review")
                .with_lifecycle("active", "active"),
        ];
        let expected = vec![VisitedEdge::from_to("complete", "in_review")
            .with_verb("request-review")
            .with_lifecycle("active", "active")];
        let mismatch = vec![VisitedEdge::from_to("complete", "in_review")
            .with_verb("request-review")
            .with_lifecycle("integration", "active")];

        assert_eq!(
            match_visited_subsequence(Some(&expected), &rows),
            VisitedMatch::Matched
        );
        assert!(matches!(
            match_visited_subsequence(Some(&mismatch), &rows),
            VisitedMatch::Missing { .. }
        ));
    }

    #[test]
    fn active_step_fields_participate_in_matching() {
        let rows = vec![
            TransitionHistoryRow::from_to("planning", "plan_review", "submit-plan")
                .with_active_step("planning", "planning_review"),
        ];
        let expected = vec![VisitedEdge::from_to("planning", "plan_review")
            .with_verb("submit-plan")
            .with_active_step("planning", "planning_review")];
        let mismatch = vec![VisitedEdge::from_to("planning", "plan_review")
            .with_verb("submit-plan")
            .with_active_step("coding", "planning_review")];

        assert_eq!(
            match_visited_subsequence(Some(&expected), &rows),
            VisitedMatch::Matched
        );
        assert!(matches!(
            match_visited_subsequence(Some(&mismatch), &rows),
            VisitedMatch::Missing { .. }
        ));
    }

    #[test]
    fn invoker_field_participates_in_matching() {
        let rows = vec![
            TransitionHistoryRow::from_to("in_review", "accepted", "accept")
                .with_invoker("ai_with_human"),
        ];
        let expected = vec![VisitedEdge::from_to("in_review", "accepted")
            .with_verb("accept")
            .with_invoker("ai_with_human")];
        let mismatch = vec![VisitedEdge::from_to("in_review", "accepted")
            .with_verb("accept")
            .with_invoker("ai_autonomous")];

        assert_eq!(
            match_visited_subsequence(Some(&expected), &rows),
            VisitedMatch::Matched
        );
        assert!(matches!(
            match_visited_subsequence(Some(&mismatch), &rows),
            VisitedMatch::Missing { .. }
        ));
    }

    #[test]
    fn smoke_catalog_contains_stable_initial_ids() {
        let ids: Vec<_> = catalog_specs(Catalog::Smoke)
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "T3-hp-with-substeps",
                "T3-pr1",
                "T3-cr1",
                "T3-er-tooling",
                "git-stale-base-refuses",
            ]
        );
    }

    #[test]
    fn full_catalog_extends_smoke_with_must_have_edges() {
        let ids: Vec<_> = catalog_specs(Catalog::Full)
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert!(ids.contains(&"T3-pr-not-ready"));
        assert!(ids.contains(&"T3-cr-fail"));
        assert!(ids.contains(&"T3-er-revise-from-blocked-runner"));
        assert!(ids.contains(&"T3-hp-delegated-policy"));
        assert!(ids.contains(&"T2-multi-phase-rejected"));
    }
}
