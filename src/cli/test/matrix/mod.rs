#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use crate::flow::agents_yaml::{Subscription, TransitionEdge};
use crate::flow::policies_yaml::PoliciesYaml;
use crate::flow::{AgentEntry, AgentsYaml, RetryPolicy};
use crate::handlers::agents_run::{poll_once_with_guard, FsBinaryIdentityProvider};
use crate::runner::{FakeRunner, Runner};
use crate::schema::Schema;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Catalog {
    Smoke,
    Full,
    Queue,
}

impl Catalog {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "smoke" => Ok(Self::Smoke),
            "full" => Ok(Self::Full),
            "queue" => Ok(Self::Queue),
            other => bail!("unknown stores test catalog '{other}' (expected smoke|full|queue)"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Full => "full",
            Self::Queue => "queue",
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
    match catalog {
        Catalog::Smoke => smoke_specs(),
        Catalog::Full => {
            let mut specs = smoke_specs();
            specs.extend(full_extra_specs());
            specs
        }
        Catalog::Queue => queue_specs(),
    }
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
        bail!(
            "stores test matrix --mode current requires --i-understand-this-mutates-current-repo"
        );
    }
    let run_id = matrix_run_id();
    let root = PathBuf::from(".stores").join("test-matrix").join(&run_id);
    run_matrix_to_root(opts, &run_id, &root)
}

fn run_matrix_to_root(opts: MatrixOpts, run_id: &str, root: &Path) -> Result<()> {
    let specs = select_matrix_specs(opts.catalog, opts.only.as_deref())?;
    std::fs::create_dir_all(root)
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
        "T3-pr1" => {
            let case_file = write_plan_review_reject_once_case_file(artifact_dir)?;
            run_existing_fake_harness_case(spec, artifact_dir, "T3-pr1", Some(case_file), watch)
        }
        "T3-cr1" => {
            let case_file = write_code_review_revise_once_case_file(artifact_dir)?;
            run_existing_fake_harness_case(spec, artifact_dir, "T3-cr1", Some(case_file), watch)
        }
        "T3-er-tooling" => {
            let case_file = write_er_tooling_case_file(artifact_dir)?;
            run_existing_fake_harness_case(spec, artifact_dir, "T3-er-tooling", Some(case_file), watch)
        }
        "queue-two-happy" => run_queue_two_happy(spec, artifact_dir, watch),
        "queue-overlap-needs-review" => run_queue_overlap_needs_review(spec, artifact_dir, watch),
        "queue-branch-head-changed" => run_queue_branch_head_changed(spec, artifact_dir, watch),
        "queue-conflict-blocked" => run_queue_conflict_blocked(spec, artifact_dir, watch),
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
            message: "Lab MVP executes T3-hp-with-substeps, T3-pr1, T3-cr1, T3-er-tooling, and matrix-intentional-red; this row is cataloged for later phases".to_string(),
        }),
    }
}

struct QueueLab {
    _tmp: tempfile::TempDir,
    conn: Connection,
    repo: PathBuf,
    agents: AgentsYaml,
}

impl QueueLab {
    fn new() -> Result<Self> {
        let tmp = tempfile::tempdir().context("create queue matrix tempdir")?;
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo)?;
        git_ok(&repo, &["init", "-b", "main"])?;
        git_ok(&repo, &["config", "user.email", "fake@example.test"])?;
        git_ok(&repo, &["config", "user.name", "Fake Test"])?;
        std::fs::write(repo.join("README.md"), "queue matrix base\n")?;
        git_ok(&repo, &["add", "README.md"])?;
        git_ok(&repo, &["commit", "-m", "base"])?;
        std::fs::write(repo.join("shared.txt"), "base\n")?;
        git_ok(&repo, &["add", "shared.txt"])?;
        git_ok(&repo, &["commit", "-m", "shared base"])?;

        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SUBSTRATE_DDL)?;
        for name in ["tasks", "external_reviews", "observations", "intake"] {
            let schema = bundled_schema_for_matrix(name)?;
            conn.execute_batch(&ddl_for(&schema))?;
        }
        crate::handlers::framework_migrate::ensure_integration_singleton_index(&conn)?;
        let agents = integrate_only_agents_for_matrix("true", false, "origin");
        Ok(Self {
            _tmp: tmp,
            conn,
            repo,
            agents,
        })
    }

    fn main_sha(&self) -> Result<String> {
        git_sha_matrix(&self.repo, "main")
    }

    fn fake_marker_branch(&self, branch: &str, task_id: &str) -> Result<String> {
        git_ok(&self.repo, &["checkout", "main"])?;
        git_ok(&self.repo, &["checkout", "-b", branch])?;
        let out = FakeRunner::with_bin(super::fake_agent_bin()?).spawn_with_invocation_and_env(
            "executor",
            "",
            "",
            None,
            Some(self.repo.to_str().unwrap()),
            None,
            &[
                ("STORES_FAKE_DELAY_MS".to_string(), "0".to_string()),
                (
                    "STORES_FAKE_EXECUTOR_MODE".to_string(),
                    "marker_file".to_string(),
                ),
                ("STORES_FAKE_TASK_ID".to_string(), task_id.to_string()),
                ("STORES_FAKE_PHASE".to_string(), "1".to_string()),
                ("STORES_FAKE_CYCLE".to_string(), "1".to_string()),
                ("STORES_FAKE_ATTEMPT".to_string(), "1".to_string()),
            ],
        )?;
        if out.exit_code != 0 {
            bail!("fake executor for {task_id} exited {}", out.exit_code);
        }
        let head = self.main_or_branch_sha(branch)?;
        git_ok(&self.repo, &["checkout", "main"])?;
        Ok(head)
    }

    fn branch_file_commit(
        &self,
        branch: &str,
        file: &str,
        contents: &str,
        msg: &str,
    ) -> Result<String> {
        git_ok(&self.repo, &["checkout", "main"])?;
        git_ok(&self.repo, &["checkout", "-b", branch])?;
        std::fs::write(self.repo.join(file), contents)?;
        git_ok(&self.repo, &["add", file])?;
        git_ok(&self.repo, &["commit", "-m", msg])?;
        let head = self.main_or_branch_sha(branch)?;
        git_ok(&self.repo, &["checkout", "main"])?;
        Ok(head)
    }

    fn mutate_branch_file(
        &self,
        branch: &str,
        file: &str,
        contents: &str,
        msg: &str,
    ) -> Result<String> {
        git_ok(&self.repo, &["checkout", branch])?;
        std::fs::write(self.repo.join(file), contents)?;
        git_ok(&self.repo, &["add", file])?;
        git_ok(&self.repo, &["commit", "-m", msg])?;
        let head = self.main_or_branch_sha(branch)?;
        git_ok(&self.repo, &["checkout", "main"])?;
        Ok(head)
    }

    fn seed_task(&self, display_id: &str, branch: &str) -> Result<()> {
        seed_queued_task_matrix(&self.conn, display_id, branch, self.repo.to_str().unwrap())
    }

    fn insert_passed_er(&self, er_id: &str, task_id: &str, base: &str, head: &str) -> Result<()> {
        insert_passed_er_matrix(&self.conn, er_id, task_id, 1, base, head)
    }

    fn drive_until<F>(&self, max_iters: usize, predicate: F) -> Result<usize>
    where
        F: FnMut(&Connection) -> bool,
    {
        drive_queue_daemon_until(&self.conn, &self.agents, max_iters, predicate)
    }

    fn status(&self, task_id: &str) -> Result<String> {
        self.conn
            .query_row(
                "SELECT status FROM tasks WHERE display_id=?1",
                [task_id],
                |r| r.get(0),
            )
            .with_context(|| format!("read status for {task_id}"))
    }

    fn attempt_field(&self, task_id: &str, field: &str) -> Result<Option<String>> {
        let path = format!("$[#-1].{}", field);
        self.conn.query_row(
            &format!("SELECT json_extract(integration_attempts, '{}') FROM tasks WHERE display_id=?1", path),
            [task_id],
            |r| r.get(0),
        ).with_context(|| format!("read attempt field {field} for {task_id}"))
    }

    fn blocked_reason(&self, task_id: &str) -> Result<String> {
        let v: Option<String> = self.conn.query_row(
            "SELECT integration_blocked_reason FROM tasks WHERE display_id=?1",
            [task_id],
            |r| r.get(0),
        )?;
        Ok(v.unwrap_or_default())
    }

    fn main_or_branch_sha(&self, rev: &str) -> Result<String> {
        git_sha_matrix(&self.repo, rev)
    }
}

fn queue_result_pass(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    observed: impl Into<String>,
    message: impl Into<String>,
) -> MatrixCaseResult {
    MatrixCaseResult {
        id: spec.id.to_string(),
        family: spec.family.to_string(),
        expected: spec.expected.to_string(),
        observed: observed.into(),
        verdict: MatrixVerdict::Pass,
        artifact_dir: artifact_dir.display().to_string(),
        message: message.into(),
    }
}

fn run_queue_two_happy(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let lab = QueueLab::new()?;
    lab.fake_marker_branch("feat/q1", "Q001")?;
    lab.fake_marker_branch("feat/q2", "Q002")?;
    lab.seed_task("Q001", "feat/q1")?;
    lab.seed_task("Q002", "feat/q2")?;
    lab.drive_until(30, |conn| {
        task_status_matrix(conn, "Q001") == "integrated"
            && task_status_matrix(conn, "Q002") == "integrated"
    })?;
    let q1_landed = lab
        .attempt_field("Q001", "landed_main_sha")?
        .unwrap_or_default();
    let q2_base = lab
        .attempt_field("Q002", "base_main_sha")?
        .unwrap_or_default();
    if q1_landed.is_empty() || q2_base != q1_landed {
        bail!("expected Q002 to integrate against Q001 landed main; q1_landed={q1_landed:?} q2_base={q2_base:?}");
    }
    if watch {
        println!(
            "queue-two-happy main={} q1_outcome={:?} q2_outcome={:?}",
            lab.main_sha()?,
            lab.attempt_field("Q001", "outcome")?,
            lab.attempt_field("Q002", "outcome")?
        );
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "Q001,Q002 integrated serially",
        "real queue lab integrated two fake-marker branches; Q002 base_main_sha equals Q001 landed_main_sha",
    ))
}

fn run_queue_overlap_needs_review(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let lab = QueueLab::new()?;
    lab.fake_marker_branch("feat/qn1", "QN001")?;
    let base = lab.main_sha()?;
    let reviewed_head = lab.fake_marker_branch("feat/qn2", "QN002")?;
    lab.seed_task("QN001", "feat/qn1")?;
    lab.seed_task("QN002", "feat/qn2")?;
    lab.insert_passed_er("ER-QN002", "QN002", &base, &reviewed_head)?;
    lab.drive_until(30, |conn| {
        task_status_matrix(conn, "QN001") == "integrated"
            && task_status_matrix(conn, "QN002") == "integration_blocked"
    })?;
    let qn1_landed = lab
        .attempt_field("QN001", "landed_main_sha")?
        .unwrap_or_default();
    let qn2_base = lab
        .attempt_field("QN002", "base_main_sha")?
        .unwrap_or_default();
    if qn1_landed.is_empty() || qn2_base != qn1_landed {
        bail!("expected QN002 to classify at front against QN001 landed main; qn1_landed={qn1_landed:?} qn2_base={qn2_base:?}");
    }
    let decision = lab
        .attempt_field("QN002", "freshness_decision")?
        .unwrap_or_default();
    let reason = lab.blocked_reason("QN002")?;
    if decision != "NeedsReview" || !reason.contains("needs_review") {
        bail!("expected QN002 NeedsReview, got decision={decision:?} reason={reason:?}");
    }
    if watch {
        println!(
            "queue-overlap-needs-review qn1={} qn2_decision={} reason={}",
            lab.status("QN001")?,
            decision,
            reason
        );
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "QN001 integrated; QN002 NeedsReview",
        "second queued candidate was classified at front after main moved and routed to typed NeedsReview",
    ))
}

fn run_queue_branch_head_changed(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let lab = QueueLab::new()?;
    let base = lab.main_sha()?;
    let reviewed_head = lab.fake_marker_branch("feat/qbranch", "QB001")?;
    lab.mutate_branch_file(
        "feat/qbranch",
        "branch-extra.txt",
        "changed after review\n",
        "branch changed after review",
    )?;
    lab.seed_task("QB001", "feat/qbranch")?;
    lab.insert_passed_er("ER-QB001", "QB001", &base, &reviewed_head)?;
    lab.drive_until(10, |conn| {
        task_status_matrix(conn, "QB001") == "integration_blocked"
    })?;
    let decision = lab
        .attempt_field("QB001", "freshness_decision")?
        .unwrap_or_default();
    if decision != "NeedsReview" {
        bail!("expected branch-head changed NeedsReview, got {decision:?}");
    }
    if watch {
        println!(
            "queue-branch-head-changed decision={} reason={}",
            decision,
            lab.blocked_reason("QB001")?
        );
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "QB001 NeedsReview",
        "candidate branch changed after ER stamp and was routed to typed NeedsReview",
    ))
}

fn run_queue_conflict_blocked(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let lab = QueueLab::new()?;
    lab.branch_file_commit(
        "feat/qc1",
        "shared.txt",
        "first branch\n",
        "first conflicting branch",
    )?;
    lab.branch_file_commit(
        "feat/qc2",
        "shared.txt",
        "second branch\n",
        "second conflicting branch",
    )?;
    lab.seed_task("QC001", "feat/qc1")?;
    lab.seed_task("QC002", "feat/qc2")?;
    lab.drive_until(30, |conn| {
        task_status_matrix(conn, "QC001") == "integrated"
            && task_status_matrix(conn, "QC002") == "integration_blocked"
    })?;
    let outcome = lab.attempt_field("QC002", "outcome")?.unwrap_or_default();
    let reason = lab.blocked_reason("QC002")?;
    if !matches!(outcome.as_str(), "rebase_conflict" | "merge_failure")
        && !reason.contains("conflict")
    {
        bail!("expected conflict blocked outcome, got outcome={outcome:?} reason={reason:?}");
    }
    if watch {
        println!(
            "queue-conflict-blocked outcome={} reason={}",
            outcome, reason
        );
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "QC001 integrated; QC002 conflict blocked",
        "second queued candidate hit a real git conflict at front of queue and was integration_blocked",
    ))
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
            message:
                "current-mode row ran through real current repo daemon path with fake runners only"
                    .to_string(),
        }),
        Err(err) if is_expectation_mismatch(&err) || is_current_red_proof(&err) => {
            Ok(MatrixCaseResult {
                id: spec.id.to_string(),
                family: spec.family.to_string(),
                expected: spec.expected.to_string(),
                observed: err.to_string(),
                verdict: MatrixVerdict::Fail,
                artifact_dir: artifact_dir_display,
                message: format!("RED current-mode substrate mismatch: {err}"),
            })
        }
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

fn write_plan_review_reject_once_case_file(artifact_dir: &Path) -> Result<PathBuf> {
    let path = artifact_dir.join("plan-review-reject-once-case.yaml");
    std::fs::write(
        &path,
        r#"cases:
  T3-pr1:
    tier: T3
    executor_mode: marker_file
    stages:
      planner: { outcome: PASS }
      plan_reviewer:
        attempts:
          - outcome: NEEDS_WORK
          - outcome: READY
      executor: { outcome: PASS }
      code_reviewer: { outcome: PASS }
      wrap: { outcome: PASS }
      external_review: { outcome: PASS }
    expect:
      task_status: integrated
      lifecycle: done
      external_review_status: passed
      no_real_llm: true
"#,
    )
    .with_context(|| {
        format!(
            "writing plan-review reject once case file {}",
            path.display()
        )
    })?;
    Ok(path)
}

fn write_code_review_revise_once_case_file(artifact_dir: &Path) -> Result<PathBuf> {
    let path = artifact_dir.join("code-review-revise-once-case.yaml");
    std::fs::write(
        &path,
        r#"cases:
  T3-cr1:
    tier: T3
    executor_mode: marker_file
    stages:
      planner: { outcome: PASS }
      plan_reviewer: { outcome: PASS }
      executor: { outcome: PASS }
      code_reviewer:
        attempts:
          - outcome: REVISE
          - outcome: PASS
      wrap: { outcome: PASS }
      external_review: { outcome: PASS }
    expect:
      task_status: integrated
      lifecycle: done
      external_review_status: passed
      no_real_llm: true
"#,
    )
    .with_context(|| {
        format!(
            "writing code-review revise once case file {}",
            path.display()
        )
    })?;
    Ok(path)
}

fn write_er_tooling_case_file(artifact_dir: &Path) -> Result<PathBuf> {
    let path = artifact_dir.join("er-tooling-case.yaml");
    std::fs::write(
        &path,
        r#"cases:
  T3-er-tooling:
    tier: T3
    executor_mode: marker_file
    stages:
      planner: { outcome: PASS }
      plan_reviewer: { outcome: PASS }
      executor: { outcome: PASS }
      code_reviewer: { outcome: PASS }
      wrap: { outcome: PASS }
      external_review:
        attempts:
          - outcome: TOOLING_FAILURE
    expect:
      task_status: in_review
      lifecycle: active
      external_review_status: tooling_held
      no_real_llm: true
"#,
    )
    .with_context(|| format!("writing ER tooling case file {}", path.display()))?;
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

fn bundled_schema_for_matrix(name: &str) -> Result<Schema> {
    let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, y)| *y)
        .with_context(|| format!("bundled schema {name}"))?;
    Schema::from_yaml(yaml)
}

fn git_out_matrix(repo: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .with_context(|| format!("run git {} in {}", args.join(" "), repo.display()))
}

fn git_ok(repo: &Path, args: &[&str]) -> Result<()> {
    let out = git_out_matrix(repo, args)?;
    if !out.status.success() {
        bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            repo.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

fn git_sha_matrix(repo: &Path, rev: &str) -> Result<String> {
    let out = git_out_matrix(repo, &["rev-parse", rev])?;
    if !out.status.success() {
        bail!(
            "git rev-parse {rev} failed in {}: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn seed_queued_task_matrix(
    conn: &Connection,
    display_id: &str,
    branch: &str,
    workspace_path: &str,
) -> Result<()> {
    let now = "2026-05-18T00:00:00Z";
    let contract =
        r#"{"done_when":"queue matrix","scope_in":"fake queue matrix","scope_out":"production"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, branch, workspace_path, contract, activation, blocked_reason, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, 'integration_queued', 'stores test queue', 'stores-test-queue', ?2, ?3, ?4, 'active', '', ?5, ?5, 'stores-test', 'stores-test')",
        params![display_id, branch, workspace_path, contract, now],
    )?;
    let row_id: i64 = conn.query_row(
        "SELECT id FROM tasks WHERE display_id=?1",
        [display_id],
        |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO transition_history (store,row_id,display_id,from_status,to_status,verb,invoker,occurred_at) \
         VALUES ('tasks', ?1, ?2, 'accepted', 'integration_queued', 'enqueue-integration', 'framework', ?3)",
        params![row_id, display_id, now],
    )?;
    Ok(())
}

fn insert_passed_er_matrix(
    conn: &Connection,
    er_id: &str,
    task_id: &str,
    attempt: i64,
    base_sha: &str,
    head_sha: &str,
) -> Result<()> {
    let now = "2026-05-18T00:00:00Z";
    conn.execute(
        "INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,base_sha,head_sha,verdict,created_at,updated_at,created_by,updated_by,runner) \
         VALUES (?1, 'passed', ?2, ?3, 'external_review', ?4, ?5, 'PASS', ?6, ?6, 'stores-test', 'stores-test', 'fake')",
        params![er_id, task_id, attempt, base_sha, head_sha, now],
    )?;
    Ok(())
}

fn integrate_only_agents_for_matrix(
    pre_land_check: &str,
    allow_push: bool,
    push_remote: &str,
) -> AgentsYaml {
    let mut args = serde_yaml::Mapping::new();
    args.insert(
        serde_yaml::Value::String("pre_land_check".into()),
        serde_yaml::Value::String(pre_land_check.into()),
    );
    args.insert(
        serde_yaml::Value::String("allow_push".into()),
        serde_yaml::Value::Bool(allow_push),
    );
    args.insert(
        serde_yaml::Value::String("push_remote".into()),
        serde_yaml::Value::String(push_remote.into()),
    );
    AgentsYaml {
        agents: vec![AgentEntry {
            name: "integrate".to_string(),
            subscribes_to: vec![Subscription {
                store: "tasks".to_string(),
                transition: TransitionEdge {
                    from: "accepted".to_string(),
                    to: "integration_queued".to_string(),
                },
                integration_step: None,
                predicate: None,
            }],
            command: "builtin:integrate".to_string(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy::default(),
            command_args: Some(args),
        }],
        deployment_specialist: None,
    }
}

fn empty_policies_for_matrix() -> PoliciesYaml {
    PoliciesYaml {
        hash: String::new(),
        policies: vec![],
    }
}

fn drive_queue_daemon_until<F>(
    conn: &Connection,
    agents: &AgentsYaml,
    max_iters: usize,
    mut predicate: F,
) -> Result<usize>
where
    F: FnMut(&Connection) -> bool,
{
    let policies = empty_policies_for_matrix();
    let cfg = PathBuf::from("/tmp/stores-test-matrix-queue-config.yaml");
    for i in 0..max_iters {
        poll_once_with_guard::<FsBinaryIdentityProvider>(
            conn,
            agents,
            &policies,
            &cfg,
            "matrix-queue",
            "matrix-epoch",
            None,
        )?;
        if predicate(conn) {
            return Ok(i + 1);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("queue matrix predicate not satisfied within {max_iters} poll iterations")
}

fn task_status_matrix(conn: &Connection, display_id: &str) -> String {
    conn.query_row(
        "SELECT status FROM tasks WHERE display_id=?1",
        [display_id],
        |r| r.get(0),
    )
    .unwrap_or_else(|_| "<missing>".to_string())
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

fn queue_specs() -> Vec<TraversalSpec> {
    vec![
        TraversalSpec {
            id: "queue-two-happy",
            family: "queue",
            description: "two queued candidates land serially through capacity-1 integration",
            expected: "both-integrated",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:integration_queued:start-integration:integrating",
                    "tasks:integrating:mark_deploy_done:integrated",
                ],
                runner_outcomes: vec!["executor:marker_commit"],
                perturbations: vec!["queue:two_candidates"],
                authority_events: vec!["policy:integration_queue"],
            },
        },
        TraversalSpec {
            id: "queue-overlap-needs-review",
            family: "queue",
            description:
                "queued candidate reviewed before main movement gets typed NeedsReview at front",
            expected: "needs-review",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:integrating:mark_integration_blocked:integration_blocked",
                ],
                runner_outcomes: vec!["executor:marker_commit"],
                perturbations: vec!["queue:main_moves_before_second_candidate"],
                authority_events: vec!["policy:integration_queue"],
            },
        },
        TraversalSpec {
            id: "queue-branch-head-changed",
            family: "queue",
            description: "candidate branch mutates after review and is routed to NeedsReview",
            expected: "needs-review",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:integrating:mark_integration_blocked:integration_blocked",
                ],
                runner_outcomes: vec!["executor:marker_commit"],
                perturbations: vec!["git:branch_head_changed_after_review"],
                authority_events: vec!["policy:integration_queue"],
            },
        },
        TraversalSpec {
            id: "queue-conflict-blocked",
            family: "queue",
            description: "front-of-queue refresh conflict routes to typed integration_blocked",
            expected: "integration-blocked/conflict",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:integrating:mark_integration_blocked:integration_blocked",
                ],
                runner_outcomes: vec!["executor:marker_commit"],
                perturbations: vec!["git:front_of_queue_rebase_conflict"],
                authority_events: vec!["policy:integration_queue"],
            },
        },
    ]
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
    fn generated_expansion_case_files_parse_expected_outcomes() {
        let dir = tempfile::tempdir().unwrap();

        let pr = write_plan_review_reject_once_case_file(dir.path()).unwrap();
        let pr_manifest: crate::cli::test::TestManifest =
            serde_yaml::from_str(&std::fs::read_to_string(pr).unwrap()).unwrap();
        let pr_case = &pr_manifest.cases["T3-pr1"];
        assert_eq!(
            pr_case.stages["plan_reviewer"].attempts[0].outcome,
            "NEEDS_WORK"
        );
        assert_eq!(pr_case.stages["plan_reviewer"].attempts[1].outcome, "READY");
        assert_eq!(pr_case.expect.task_status, "integrated");

        let cr = write_code_review_revise_once_case_file(dir.path()).unwrap();
        let cr_manifest: crate::cli::test::TestManifest =
            serde_yaml::from_str(&std::fs::read_to_string(cr).unwrap()).unwrap();
        let cr_case = &cr_manifest.cases["T3-cr1"];
        assert_eq!(
            cr_case.stages["code_reviewer"].attempts[0].outcome,
            "REVISE"
        );
        assert_eq!(cr_case.stages["code_reviewer"].attempts[1].outcome, "PASS");
        assert_eq!(cr_case.expect.task_status, "integrated");

        let er = write_er_tooling_case_file(dir.path()).unwrap();
        let er_manifest: crate::cli::test::TestManifest =
            serde_yaml::from_str(&std::fs::read_to_string(er).unwrap()).unwrap();
        let er_case = &er_manifest.cases["T3-er-tooling"];
        assert_eq!(
            er_case.stages["external_review"].attempts[0].outcome,
            "TOOLING_FAILURE"
        );
        assert_eq!(er_case.expect.task_status, "in_review");
        assert_eq!(er_case.expect.external_review_status, "tooling_held");
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

    #[test]
    fn queue_catalog_lists_batch_b_cases() {
        assert_eq!(Catalog::parse("queue").unwrap(), Catalog::Queue);
        assert_eq!(Catalog::Queue.as_str(), "queue");

        let ids: Vec<_> = catalog_specs(Catalog::Queue)
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "queue-two-happy",
                "queue-overlap-needs-review",
                "queue-branch-head-changed",
                "queue-conflict-blocked",
            ]
        );
    }

    #[test]
    fn queue_matrix_lab_output_writes_queue_report() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let old_fake_bin = std::env::var_os("STORES_FAKE_AGENT_BIN");
        std::env::set_var(
            "STORES_FAKE_AGENT_BIN",
            target_debug_bin("stores-fake-agent"),
        );

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("matrix-output");
        let result = run_matrix_to_root(
            MatrixOpts {
                catalog: Catalog::Queue,
                mode: MatrixMode::Lab,
                only: Some("queue-two-happy".to_string()),
                watch: false,
                current_ack: false,
            },
            "unit-queue-run",
            &root,
        );
        match old_fake_bin {
            Some(value) => std::env::set_var("STORES_FAKE_AGENT_BIN", value),
            None => std::env::remove_var("STORES_FAKE_AGENT_BIN"),
        }
        result.unwrap();

        let index: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("index.json")).unwrap())
                .unwrap();
        assert_eq!(index["mode"], "lab");
        assert_eq!(index["catalog"], "queue");
        assert_eq!(index["results"][0]["id"], "queue-two-happy");
        assert_eq!(index["results"][0]["verdict"], "PASS");

        let report = std::fs::read_to_string(root.join("index.md")).unwrap();
        assert!(report.contains("catalog: `queue`"));
        assert!(report.contains("`queue-two-happy`"));
        assert!(root.join("queue-two-happy").join("proof.txt").exists());
    }

    fn target_debug_bin(name: &str) -> PathBuf {
        let mut path = std::env::current_exe().expect("unit test current_exe");
        path.pop();
        if path.file_name().and_then(|n| n.to_str()) == Some("deps") {
            path.pop();
        }
        path.push(name);
        assert!(
            path.exists(),
            "expected built {name} binary at {}; run `cargo test --bins --tests` or `cargo build --bin stores --bin stores-fake-agent` before this test",
            path.display()
        );
        path
    }
}
