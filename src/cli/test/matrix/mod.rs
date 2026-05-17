#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixMode {
    Lab,
    Current,
}

impl MatrixMode {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "lab" => Ok(Self::Lab),
            "current" => Ok(Self::Current),
            other => bail!("unsupported stores test matrix mode '{other}' (expected lab|current)"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lab => "lab",
            Self::Current => "current",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MatrixOpts {
    pub catalog: Catalog,
    pub mode: MatrixMode,
    pub only: Option<String>,
    pub watch: bool,
    pub current_ack: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MatrixVerdict {
    Pass,
    Fail,
    Skip,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatrixCaseResult {
    pub id: String,
    pub family: String,
    pub expected: String,
    pub observed: String,
    pub verdict: MatrixVerdict,
    pub artifact_dir: String,
    pub message: String,
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

pub fn run_matrix(opts: MatrixOpts) -> Result<()> {
    if opts.mode == MatrixMode::Current && !opts.current_ack {
        bail!("stores test matrix --mode current requires --i-understand-this-mutates-current-repo");
    }
    let specs = select_matrix_specs(opts.catalog, opts.only.as_deref())?;
    let run_id = matrix_run_id();
    let root = PathBuf::from(".stores").join("test-matrix").join(&run_id);
    std::fs::create_dir_all(&root)
        .with_context(|| format!("creating matrix artifact root {}", root.display()))?;

    println!(
        "Stores Fake Traversal Matrix — mode={} catalog={} run={} cases={}",
        opts.mode.as_str(),
        opts.catalog.as_str(),
        run_id,
        specs.len()
    );
    println!(
        "{:<28} {:<18} {:<22} {:<10} ARTIFACT",
        "CASE", "FAMILY", "EXPECTED", "VERDICT"
    );
    println!("{}", "─".repeat(96));

    let mut results = Vec::new();
    for spec in specs {
        let artifact_dir = root.join(spec.id);
        std::fs::create_dir_all(&artifact_dir)
            .with_context(|| format!("creating artifact dir {}", artifact_dir.display()))?;
        let result = run_matrix_case(opts.mode, &spec, &artifact_dir, opts.watch);
        let case_result = match result {
            Ok(result) => result,
            Err(err) => MatrixCaseResult {
                id: spec.id.to_string(),
                family: spec.family.to_string(),
                expected: spec.expected.to_string(),
                observed: "harness-error".to_string(),
                verdict: MatrixVerdict::Error,
                artifact_dir: artifact_dir.display().to_string(),
                message: err.to_string(),
            },
        };
        write_case_artifacts(&artifact_dir, &case_result)?;
        println!(
            "{:<28} {:<18} {:<22} {:<10} {}",
            case_result.id,
            case_result.family,
            case_result.expected,
            verdict_label(case_result.verdict),
            case_result.artifact_dir
        );
        if !case_result.message.is_empty() {
            println!("  {}", case_result.message);
        }
        results.push(case_result);
    }

    write_index_artifact(&root, &run_id, opts.mode, opts.catalog, &results)?;
    let pass = results
        .iter()
        .filter(|r| r.verdict == MatrixVerdict::Pass)
        .count();
    let fail = results
        .iter()
        .filter(|r| r.verdict == MatrixVerdict::Fail)
        .count();
    let skip = results
        .iter()
        .filter(|r| r.verdict == MatrixVerdict::Skip)
        .count();
    let error = results
        .iter()
        .filter(|r| r.verdict == MatrixVerdict::Error)
        .count();
    println!("Summary: {pass} PASS / {fail} FAIL / {skip} SKIP / {error} ERROR");
    println!("Report: {}", root.display());
    Ok(())
}

fn print_coverage(label: &str, tags: &[&str]) {
    if tags.is_empty() {
        println!("  {label}: -");
    } else {
        println!("  {label}: {}", tags.join(","));
    }
}

fn select_matrix_specs(catalog: Catalog, only: Option<&str>) -> Result<Vec<TraversalSpec>> {
    if let Some("matrix-intentional-red") = only {
        return Ok(vec![intentional_red_spec()]);
    }
    let specs = catalog_specs(catalog);
    if let Some(only) = only {
        let spec = specs
            .into_iter()
            .find(|spec| spec.id == only)
            .with_context(|| {
                format!(
                    "matrix case '{only}' not found in {} catalog",
                    catalog.as_str()
                )
            })?;
        Ok(vec![spec])
    } else {
        Ok(specs)
    }
}

fn run_matrix_case(
    mode: MatrixMode,
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    match mode {
        MatrixMode::Lab => run_lab_case(spec, artifact_dir, watch),
        MatrixMode::Current => run_current_case(spec, artifact_dir, watch),
    }
}

fn run_lab_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    match spec.id {
        "T3-hp-with-substeps" => {
            let case_file = write_happy_with_substeps_case_file(artifact_dir)?;
            run_existing_fake_harness_case(
                spec,
                artifact_dir,
                "T3-hp-with-substeps",
                Some(case_file),
                watch,
            )
        }
        "matrix-intentional-red" => {
            let case_file = write_intentional_red_case_file(artifact_dir)?;
            run_existing_fake_harness_case(
                spec,
                artifact_dir,
                "matrix-intentional-red",
                Some(case_file),
                watch,
            )
        }
        _ => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: "not-implemented-in-phase-2-mvp".to_string(),
            verdict: MatrixVerdict::Skip,
            artifact_dir: artifact_dir.display().to_string(),
            message: "Phase 2 MVP only executes T3-hp-with-substeps and matrix-intentional-red; this row is cataloged for later phases".to_string(),
        }),
    }
}

fn run_current_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    match spec.id {
        "git-stale-base-refuses" => run_current_fake_harness_case(
            spec,
            artifact_dir,
            "stale-base-refuses",
            watch,
        ),
        _ => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: "not-implemented-in-current-mode-mvp".to_string(),
            verdict: MatrixVerdict::Skip,
            artifact_dir: artifact_dir.display().to_string(),
            message: "Current-mode MVP only executes git-stale-base-refuses; use lab mode for local fake harness rows".to_string(),
        }),
    }
}

fn run_current_fake_harness_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    case_name: &str,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let opts = super::TestRunOpts {
        case_name: Some(case_name.to_string()),
        case_file: None,
        delay_ms: Some(0),
        watch,
        live: true,
    };
    let result = super::run(opts);
    let artifact_dir_display = artifact_dir.display().to_string();
    match result {
        Ok(()) => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: spec.expected.to_string(),
            verdict: MatrixVerdict::Pass,
            artifact_dir: artifact_dir_display,
            message: "current-mode row ran through real current repo daemon path with fake runners only".to_string(),
        }),
        Err(err) if is_expectation_mismatch(&err) || is_current_red_proof(&err) => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: err.to_string(),
            verdict: MatrixVerdict::Fail,
            artifact_dir: artifact_dir_display,
            message: format!("RED current-mode substrate mismatch: {err}"),
        }),
        Err(err) => Err(err),
    }
}

fn run_existing_fake_harness_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    case_name: &str,
    case_file: Option<PathBuf>,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let opts = super::TestRunOpts {
        case_name: Some(case_name.to_string()),
        case_file,
        delay_ms: Some(0),
        watch,
        live: false,
    };
    let result = super::run(opts);
    let artifact_dir_display = artifact_dir.display().to_string();
    match result {
        Ok(()) => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: spec.expected.to_string(),
            verdict: MatrixVerdict::Pass,
            artifact_dir: artifact_dir_display,
            message: "executable lab MVP row ran existing fake harness with real fake-runner subprocess, SQLite temp DB, git repo/worktree, and no real LLM".to_string(),
        }),
        Err(err) if is_expectation_mismatch(&err) => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: err.to_string(),
            verdict: MatrixVerdict::Fail,
            artifact_dir: artifact_dir_display,
            message: format!("RED expectation mismatch: {err}"),
        }),
        Err(err) => Err(err),
    }
}

fn is_current_red_proof(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("stale-base-refuses RED proof")
}

fn is_expectation_mismatch(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("expected task_status=")
        || msg.contains("expected lifecycle=")
        || msg.contains("expected external_review_status=")
        || msg.contains("expected visited edge")
}

fn write_happy_with_substeps_case_file(artifact_dir: &Path) -> Result<PathBuf> {
    let path = artifact_dir.join("happy-with-substeps-case.yaml");
    std::fs::write(
        &path,
        r#"cases:
  T3-hp-with-substeps:
    tier: T3
    executor_mode: marker_file
    stages:
      planner: { outcome: PASS }
      plan_reviewer: { outcome: PASS }
      executor: { outcome: PASS }
      code_reviewer: { outcome: PASS }
      wrap: { outcome: PASS }
      external_review: { outcome: PASS }
    expect:
      task_status: integrated
      lifecycle: done
      external_review_status: passed
      no_real_llm: true
      visited:
        - { from_status: in_review, to_status: accepted, verb: accept }
        - { from_status: accepted, to_status: integration_queued, verb: release-to-integration }
        - { from_status: integration_queued, to_status: integrating, verb: start-integration }
        - { from_status: integrating, to_status: integrating, verb: mark_refresh_done, integration_step_from: refreshing, integration_step_to: task_review }
        - { from_status: integrating, to_status: integrating, verb: mark_task_review_done, integration_step_from: task_review, integration_step_to: testing }
        - { from_status: integrating, to_status: integrating, verb: mark_testing_done, integration_step_from: testing, integration_step_to: merging }
        - { from_status: integrating, to_status: integrating, verb: mark_merge_done, integration_step_from: merging, integration_step_to: deploying }
"#,
    )
    .with_context(|| format!("writing happy substeps case file {}", path.display()))?;
    Ok(path)
}

fn write_intentional_red_case_file(artifact_dir: &Path) -> Result<PathBuf> {
    let path = artifact_dir.join("intentional-red-case.yaml");
    std::fs::write(
        &path,
        r#"cases:
  matrix-intentional-red:
    tier: T3
    executor_mode: marker_file
    stages:
      planner: { outcome: PASS }
      plan_reviewer: { outcome: PASS }
      executor: { outcome: PASS }
      code_reviewer: { outcome: PASS }
      wrap: { outcome: PASS }
      external_review: { outcome: PASS }
    expect:
      task_status: blocked
      lifecycle: active
      external_review_status: passed
      no_real_llm: true
"#,
    )
    .with_context(|| format!("writing intentional RED case file {}", path.display()))?;
    Ok(path)
}

fn write_case_artifacts(artifact_dir: &Path, result: &MatrixCaseResult) -> Result<()> {
    let result_json = serde_json::to_string_pretty(result)?;
    std::fs::write(artifact_dir.join("result.json"), result_json)
        .with_context(|| format!("writing {}", artifact_dir.join("result.json").display()))?;
    let proof = format!(
        "case: {}\nfamily: {}\nexpected: {}\nobserved: {}\nverdict: {}\nmessage: {}\n",
        result.id,
        result.family,
        result.expected,
        result.observed,
        verdict_label(result.verdict),
        result.message
    );
    std::fs::write(artifact_dir.join("proof.txt"), proof)
        .with_context(|| format!("writing {}", artifact_dir.join("proof.txt").display()))?;
    Ok(())
}

fn write_index_artifact(
    root: &Path,
    run_id: &str,
    mode: MatrixMode,
    catalog: Catalog,
    results: &[MatrixCaseResult],
) -> Result<()> {
    let index = serde_json::json!({
        "run_id": run_id,
        "mode": mode.as_str(),
        "catalog": catalog.as_str(),
        "results": results,
    });
    std::fs::write(
        root.join("index.json"),
        serde_json::to_string_pretty(&index)?,
    )
    .with_context(|| format!("writing {}", root.join("index.json").display()))?;
    let mut md = format!(
        "# Stores Fake Traversal Matrix\n\nrun_id: `{run_id}`  \\nmode: `{}`  \\ncatalog: `{}`\n\n| Case | Family | Expected | Verdict | Artifact |\n|---|---|---|---|---|\n",
        mode.as_str(),
        catalog.as_str()
    );
    for result in results {
        md.push_str(&format!(
            "| `{}` | {} | {} | {} | `{}` |\n",
            result.id,
            result.family,
            result.expected,
            verdict_label(result.verdict),
            result.artifact_dir
        ));
    }
    std::fs::write(root.join("index.md"), md)
        .with_context(|| format!("writing {}", root.join("index.md").display()))?;
    Ok(())
}

fn verdict_label(verdict: MatrixVerdict) -> &'static str {
    match verdict {
        MatrixVerdict::Pass => "PASS",
        MatrixVerdict::Fail => "FAIL",
        MatrixVerdict::Skip => "SKIP",
        MatrixVerdict::Error => "ERROR",
    }
}

fn matrix_run_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("run-{}-{}", now.as_secs(), std::process::id())
}

fn intentional_red_spec() -> TraversalSpec {
    TraversalSpec {
        id: "matrix-intentional-red",
        family: "matrix-engine",
        description:
            "intentional RED row: happy fake traversal with deliberately wrong terminal expectation",
        expected: "blocked/active (intentional mismatch)",
        coverage: CoverageTags {
            schema_edges: vec!["matrix:intentional-red"],
            runner_outcomes: vec![
                "planner:PASS",
                "plan_reviewer:PASS",
                "executor:PASS",
                "code_reviewer:PASS",
                "wrap:PASS",
                "external_review:PASS",
            ],
            perturbations: vec![],
            authority_events: vec!["task:accept"],
        },
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
