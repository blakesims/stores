#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::codegen::ddl::{ddl_for, SUBSTRATE_DDL};
use crate::flow::agents_yaml::{Subscription, TransitionEdge};
use crate::flow::policies_yaml::PoliciesYaml;
use crate::flow::engine_runner::{scan_and_record_actionability, ScannerSchemas};
use crate::flow::{AgentEntry, AgentsYaml, RetryPolicy};
use crate::handlers::agents_run::{poll_once_with_guard, FsBinaryIdentityProvider};
use crate::runner::{FakeRunner, Runner};
use crate::schema::{actor::Actor, Schema};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Catalog {
    Smoke,
    Full,
    Queue,
    Battlescars,
    Upstream,
}

impl Catalog {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "smoke" => Ok(Self::Smoke),
            "full" => Ok(Self::Full),
            "queue" => Ok(Self::Queue),
            "battlescars" => Ok(Self::Battlescars),
            "upstream" => Ok(Self::Upstream),
            other => bail!(
                "unknown stores test catalog '{other}' (expected smoke|full|queue|battlescars|upstream)"
            ),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Full => "full",
            Self::Queue => "queue",
            Self::Battlescars => "battlescars",
            Self::Upstream => "upstream",
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
    pub report: MatrixReport,
    pub ci: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatrixReport {
    Md,
    Json,
}

impl MatrixReport {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "md" => Ok(Self::Md),
            "json" => Ok(Self::Json),
            other => bail!("unsupported stores test matrix report '{other}' (expected md|json)"),
        }
    }

    fn artifact_name(self) -> &'static str {
        match self {
            Self::Md => "index.md",
            Self::Json => "index.json",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MatrixCoverageSummary {
    pub schema_edges: Vec<String>,
    pub runner_outcomes: Vec<String>,
    pub perturbations: Vec<String>,
    pub authority_events: Vec<String>,
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
        Catalog::Battlescars => battlescar_specs(),
        Catalog::Upstream => upstream_specs(),
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
    let coverage = coverage_summary(&specs);
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

    write_index_artifact(&root, &run_id, opts.mode, opts.catalog, &coverage, &results)?;
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
    println!(
        "Report: {}",
        root.join(opts.report.artifact_name()).display()
    );
    if opts.ci && (fail > 0 || error > 0) {
        bail!("matrix CI failed: {fail} FAIL / {error} ERROR");
    }
    Ok(())
}

pub fn prune_matrix_runs(keep_last: usize) -> Result<()> {
    let root = PathBuf::from(".stores").join("test-matrix");
    if !root.exists() {
        println!("No matrix artifacts at {}", root.display());
        return Ok(());
    }
    let mut runs = Vec::new();
    for entry in std::fs::read_dir(&root)
        .with_context(|| format!("reading matrix artifact root {}", root.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        runs.push((modified, entry.path()));
    }
    runs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let total = runs.len();
    let mut removed = 0usize;
    for (_, path) in runs.into_iter().skip(keep_last) {
        std::fs::remove_dir_all(&path)
            .with_context(|| format!("removing matrix artifact run {}", path.display()))?;
        println!("removed {}", path.display());
        removed += 1;
    }
    println!(
        "Matrix artifacts: kept {} of {total}, removed {removed}",
        keep_last.min(total)
    );
    Ok(())
}

fn print_coverage(label: &str, tags: &[&str]) {
    if tags.is_empty() {
        println!("  {label}: -");
    } else {
        println!("  {label}: {}", tags.join(","));
    }
}

fn coverage_summary(specs: &[TraversalSpec]) -> MatrixCoverageSummary {
    let mut schema_edges = BTreeSet::new();
    let mut runner_outcomes = BTreeSet::new();
    let mut perturbations = BTreeSet::new();
    let mut authority_events = BTreeSet::new();
    for spec in specs {
        schema_edges.extend(
            spec.coverage
                .schema_edges
                .iter()
                .map(|tag| (*tag).to_string()),
        );
        runner_outcomes.extend(
            spec.coverage
                .runner_outcomes
                .iter()
                .map(|tag| (*tag).to_string()),
        );
        perturbations.extend(
            spec.coverage
                .perturbations
                .iter()
                .map(|tag| (*tag).to_string()),
        );
        authority_events.extend(
            spec.coverage
                .authority_events
                .iter()
                .map(|tag| (*tag).to_string()),
        );
    }
    MatrixCoverageSummary {
        schema_edges: schema_edges.into_iter().collect(),
        runner_outcomes: runner_outcomes.into_iter().collect(),
        perturbations: perturbations.into_iter().collect(),
        authority_events: authority_events.into_iter().collect(),
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
            run_existing_fake_harness_case(
                spec,
                artifact_dir,
                "T3-er-tooling",
                Some(case_file),
                watch,
            )
        }
        "queue-two-happy" => run_queue_two_happy(spec, artifact_dir, watch),
        "queue-overlap-needs-review" => run_queue_overlap_needs_review(spec, artifact_dir, watch),
        "queue-branch-head-changed" => run_queue_branch_head_changed(spec, artifact_dir, watch),
        "queue-conflict-blocked" => run_queue_conflict_blocked(spec, artifact_dir, watch),
        "dirty-worktree-refusal" => run_dirty_worktree_refusal(spec, artifact_dir, watch),
        "merge-conflict-blocked" => run_queue_conflict_blocked(spec, artifact_dir, watch),
        "stale-external-review-head-mutation" => {
            run_stale_external_review_head_mutation(spec, artifact_dir, watch)
        }
        "obs-auto-promote-happy" => run_obs_auto_promote_happy_case(spec, artifact_dir, watch),
        "reject-amend" => run_reject_amend_case(spec, artifact_dir, watch),
        "abandon" => run_upstream_abandon_case(spec, artifact_dir, watch),
        "close-out-of-band" => run_close_out_of_band_case(spec, artifact_dir, watch),
        "resume-blocked" => run_resume_blocked_case(spec, artifact_dir, watch),
        "retry-integration" => run_retry_integration_case(spec, artifact_dir, watch),
        "payload-invalid" => run_expected_harness_error_case(
            spec,
            artifact_dir,
            write_runner_battlescar_case_file(artifact_dir, spec.id, "PAYLOAD_INVALID")?,
            watch,
            "payload",
            "runner payload invalid output was classified as a blocked task",
        ),
        "nonzero-exit" => run_expected_harness_error_case(
            spec,
            artifact_dir,
            write_runner_battlescar_case_file(artifact_dir, spec.id, "NONZERO_EXIT")?,
            watch,
            "non-zero exit",
            "runner nonzero exit was classified as a blocked task",
        ),
        "no-heartbeat" => run_no_heartbeat_case(spec, artifact_dir, watch),
        "duplicate-drive-refusal" => run_duplicate_drive_refusal(spec, artifact_dir, watch),
        "stale-dead-current-run-marker" => run_stale_dead_current_run_marker(spec, artifact_dir, watch),
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
            message: format!(
                "case {} is cataloged but not executable in this matrix mode yet",
                spec.id
            ),
        }),
    }
}

struct QueueLab {
    _tmp: tempfile::TempDir,
    conn: Connection,
    repo: PathBuf,
    agents: AgentsYaml,
}

struct DriveLab {
    _tmp: tempfile::TempDir,
    conn: Connection,
    workspace: PathBuf,
    tasks_schema: Schema,
    intake_schema: Schema,
    observations_schema: Schema,
}

impl DriveLab {
    fn new() -> Result<Self> {
        let tmp = tempfile::tempdir().context("create drive matrix tempdir")?;
        let workspace = tmp.path().join("workspace");
        std::fs::create_dir_all(workspace.join(".stores").join("runs"))?;
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SUBSTRATE_DDL)?;
        let tasks_schema = bundled_schema_for_matrix("tasks")?;
        let intake_schema = bundled_schema_for_matrix("intake")?;
        let observations_schema = bundled_schema_for_matrix("observations")?;
        conn.execute_batch(&ddl_for(&tasks_schema))?;
        conn.execute_batch(&ddl_for(&intake_schema))?;
        conn.execute_batch(&ddl_for(&observations_schema))?;
        Ok(Self {
            _tmp: tmp,
            conn,
            workspace,
            tasks_schema,
            intake_schema,
            observations_schema,
        })
    }

    fn seed_active_task(&self, display_id: &str, status: &str) -> Result<()> {
        let now = "2026-05-18T00:00:00Z";
        let contract = r#"{"done_when":"drive battlescar matrix","scope_in":"fake drive matrix","scope_out":"production"}"#;
        let plan = r#"{"phases":[{"title":"matrix phase","steps":["fake"]}]}"#;
        self.conn.execute(
            "INSERT INTO tasks (display_id, status, title, slug, workspace_path, contract, plan, tier_hint, activation, lifecycle, active_step, integration_step, blocked, blocked_reason, created_at, updated_at, created_by, updated_by) \
             VALUES (?1, ?2, 'stores test drive battlescar', 'stores-test-drive-battlescar', ?3, ?4, ?5, 'T3', 'active', 'active', ?2, 'none', 0, '', ?6, ?6, 'stores-test', 'stores-test')",
            params![display_id, status, self.workspace.to_str().unwrap(), contract, plan, now],
        )?;
        Ok(())
    }

    fn row_id(&self, display_id: &str) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT id FROM tasks WHERE display_id=?1",
                [display_id],
                |r| r.get(0),
            )
            .with_context(|| format!("read row_id for {display_id}"))
    }

    fn insert_live_auto_drive_lock(&self, row_id: i64, display_id: &str) -> Result<()> {
        let now = crate::handlers::row::now_iso8601();
        self.conn.execute(
            "INSERT INTO dispatch_locks (store, row_id, display_id, agent_name, transition_id, claimed_at, heartbeat_at, claimed_by, pid) \
             VALUES ('tasks', ?1, ?2, 'auto-drive', 1, ?3, ?3, 'matrix-live-owner', 0)",
            params![row_id, display_id, now],
        )?;
        Ok(())
    }

    fn auto_drive_lock_count(&self, row_id: i64) -> Result<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM dispatch_locks WHERE store='tasks' AND row_id=?1 AND agent_name='auto-drive' AND finished_at IS NULL",
            params![row_id],
            |r| r.get(0),
        ).context("count active auto-drive locks")
    }

    fn scan_actionability(&self) -> Result<crate::flow::engine_runner::ScannerResult> {
        scan_and_record_actionability(
            &self.conn,
            ScannerSchemas {
                tasks: &self.tasks_schema,
                intake: &self.intake_schema,
                observations: &self.observations_schema,
            },
            1,
            &crate::handlers::row::now_iso8601(),
        )
    }

    fn actionability(&self, row_id: i64) -> Result<(String, Option<String>, bool)> {
        self.conn.query_row(
            "SELECT classification, held_reason, dispatched FROM engine_runner_actions WHERE store='tasks' AND row_id=?1",
            params![row_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
        ).with_context(|| format!("read engine_runner_actions for row_id={row_id}"))
    }

    fn marker_path(&self, display_id: &str) -> Result<PathBuf> {
        Ok(self
            .workspace
            .join(".stores")
            .join("runs")
            .join(format!("current-{display_id}-matrix.json")))
    }

    fn write_current_run_marker(&self, display_id: &str, status: &str, updated_at: &str) -> Result<()> {
        let path = self.marker_path(display_id)?;
        let body = serde_json::json!({
            "display_id": display_id,
            "phase": 0,
            "cycle": 0,
            "role": "planner",
            "runner": "fake",
            "session_id": "matrix-session",
            "status": status,
            "updated_at": updated_at,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&body)?)
            .with_context(|| format!("write current-run marker {}", path.display()))?;
        Ok(())
    }
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

fn run_stale_external_review_head_mutation(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let lab = QueueLab::new()?;
    let base = lab.main_sha()?;
    let reviewed_head = lab.fake_marker_branch("feat/stale-er-head", "BSER001")?;
    lab.mutate_branch_file(
        "feat/stale-er-head",
        "branch-extra.txt",
        "changed after external review\n",
        "branch changed after external review",
    )?;
    lab.seed_task("BSER001", "feat/stale-er-head")?;
    lab.insert_passed_er("ER-BSER001", "BSER001", &base, &reviewed_head)?;
    lab.drive_until(10, |conn| task_status_matrix(conn, "BSER001") == "integration_blocked")?;

    let outcome = lab
        .attempt_field("BSER001", "outcome")?
        .unwrap_or_default();
    let decision = lab
        .attempt_field("BSER001", "freshness_decision")?
        .unwrap_or_default();
    let completed_at = lab
        .attempt_field("BSER001", "completed_at")?
        .unwrap_or_default();
    let reason = lab.blocked_reason("BSER001")?;
    let unfinished_locks: i64 = lab.conn.query_row(
        "SELECT COUNT(*) FROM dispatch_locks WHERE store='tasks' AND display_id='BSER001' AND agent_name='integrate' AND finished_at IS NULL",
        [],
        |r| r.get(0),
    )?;
    if outcome != "needs_review"
        || decision != "NeedsReview"
        || completed_at.is_empty()
        || !reason.contains("needs_review")
        || unfinished_locks != 0
    {
        bail!(
            "expected typed NeedsReview with finalized integrate lock; outcome={outcome:?} decision={decision:?} completed_at={completed_at:?} reason={reason:?} unfinished_locks={unfinished_locks}"
        );
    }
    if watch {
        println!(
            "stale-external-review-head-mutation outcome={} decision={} reason={} unfinished_integrate_locks={}",
            outcome, decision, reason, unfinished_locks
        );
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "BSER001 NeedsReview with finalized integrate lock",
        "candidate head mutated after external review and was routed to typed NeedsReview with no unfinished integrate lock",
    ))
}

fn run_duplicate_drive_refusal(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let lab = DriveLab::new()?;
    lab.seed_active_task("BDD001", "planning")?;
    let row_id = lab.row_id("BDD001")?;
    lab.insert_live_auto_drive_lock(row_id, "BDD001")?;
    let before_locks = lab.auto_drive_lock_count(row_id)?;
    let result = lab.scan_actionability()?;
    let (classification, held_reason, dispatched) = lab.actionability(row_id)?;
    let after_locks = lab.auto_drive_lock_count(row_id)?;
    if result.rows.len() != 1
        || classification != "held"
        || held_reason.as_deref() != Some("live_dispatch_lock")
        || dispatched
        || before_locks != 1
        || after_locks != 1
    {
        bail!(
            "expected duplicate drive refusal via live_dispatch_lock without second dispatch; rows={} classification={classification:?} held_reason={held_reason:?} dispatched={dispatched} locks_before={before_locks} locks_after={after_locks}",
            result.rows.len()
        );
    }
    if watch {
        println!(
            "duplicate-drive-refusal classification={} held_reason={:?} dispatched={} locks_before={} locks_after={}",
            classification, held_reason, dispatched, before_locks, after_locks
        );
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "BDD001 held/live_dispatch_lock; no second dispatch",
        "engine scanner recorded duplicate-drive refusal as live_dispatch_lock and did not create a second active drive/dispatch",
    ))
}

fn run_stale_dead_current_run_marker(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let lab = DriveLab::new()?;
    lab.seed_active_task("BSM-STALE", "planning")?;
    lab.seed_active_task("BSM-LIVE", "planning")?;
    let stale_id = lab.row_id("BSM-STALE")?;
    let live_id = lab.row_id("BSM-LIVE")?;
    lab.write_current_run_marker("BSM-STALE", "running", "2020-01-01T00:00:00Z")?;
    let fresh_now = crate::handlers::row::now_iso8601();
    lab.write_current_run_marker("BSM-LIVE", "running", &fresh_now)?;
    let stale_marker = lab.marker_path("BSM-STALE")?;
    let live_marker = lab.marker_path("BSM-LIVE")?;
    let result = lab.scan_actionability()?;
    let (stale_classification, stale_reason, stale_dispatched) = lab.actionability(stale_id)?;
    let (live_classification, live_reason, live_dispatched) = lab.actionability(live_id)?;
    if result.rows.len() != 2
        || live_classification != "held"
        || live_reason.as_deref() != Some("live_runner_marker")
        || live_dispatched
        || stale_reason.as_deref() == Some("live_runner_marker")
        || !stale_marker.exists()
        || !live_marker.exists()
    {
        bail!(
            "expected fresh marker held and stale marker ignored without deletion; rows={} stale=({stale_classification:?},{stale_reason:?},dispatched={stale_dispatched}) live=({live_classification:?},{live_reason:?},dispatched={live_dispatched}) stale_exists={} live_exists={}",
            result.rows.len(),
            stale_marker.exists(),
            live_marker.exists()
        );
    }
    if watch {
        println!(
            "stale-dead-current-run-marker stale_reason={:?} stale_dispatched={} live_reason={:?} live_dispatched={} markers_preserved={}/{}",
            stale_reason,
            stale_dispatched,
            live_reason,
            live_dispatched,
            stale_marker.exists(),
            live_marker.exists()
        );
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "fresh marker held; stale marker ignored; markers preserved",
        "current-run marker truth distinguished fresh running marker from stale marker without blindly deleting state",
    ))
}

fn run_obs_auto_promote_happy_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let conn = upstream_conn()?;
    let obs_schema = bundled_schema_for_matrix("observations")?;
    seed_observation_matrix(&conn, "LHU001")?;
    let confirm = display_id_matches("confirm", "LHU001");
    crate::handlers::transition::run(
        &obs_schema,
        &conn,
        &confirm,
        Actor::AiWithHuman.into(),
        "confirm",
    )?;
    let (obs_status, obs_lifecycle, obs_contract_state): (String, String, String) = conn.query_row(
        "SELECT status,lifecycle,contract_state FROM observations WHERE display_id='LHU001'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let confirm_history = transition_count_matrix(&conn, "observations", "LHU001", "confirm", "ai_with_human")?;
    let ratify_history = transition_count_matrix(&conn, "observations", "LHU001", "ratify", "framework")?;
    let agents = AgentsYaml {
        agents: vec![AgentEntry {
            name: "auto-promote".to_string(),
            subscribes_to: vec![Subscription {
                store: "observations".to_string(),
                transition: TransitionEdge {
                    from: "confirmed".to_string(),
                    to: "ready".to_string(),
                },
                integration_step: None,
                predicate: None,
            }],
            command: "builtin:auto-promote".to_string(),
            claim_window_secs: 300,
            retry_policy: RetryPolicy::default(),
            command_args: None,
        }],
        deployment_specialist: None,
    };
    let cfg = PathBuf::from("/tmp/stores-test-matrix-upstream-config.yaml");
    let dispatched = poll_once_with_guard::<FsBinaryIdentityProvider>(
        &conn,
        &agents,
        &empty_policies_for_matrix(),
        &cfg,
        "matrix-upstream",
        "matrix-upstream-epoch",
        None,
    )?;
    let (task_id, linked_count): (String, i64) = conn.query_row(
        "SELECT observations.task_id, COUNT(tasks.id) FROM observations JOIN tasks ON tasks.display_id=observations.task_id, json_each(tasks.linked_observations) je WHERE observations.display_id='LHU001' AND je.value='LHU001'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let task_create = transition_count_matrix(&conn, "tasks", &task_id, "create", "ai_autonomous")?;
    let auto_promote_lock: i64 = conn.query_row(
        "SELECT COUNT(*) FROM dispatch_locks WHERE store='observations' AND display_id='LHU001' AND agent_name='auto-promote' AND finished_at IS NOT NULL AND terminal_reason='ok'",
        [],
        |r| r.get(0),
    )?;
    if dispatched != 1
        || obs_status != "ready"
        || obs_lifecycle != "ready"
        || obs_contract_state != "approved"
        || confirm_history != 1
        || ratify_history != 1
        || linked_count != 1
        || task_create != 1
        || auto_promote_lock != 1
    {
        bail!("obs auto-promote mismatch: dispatched={dispatched} obs={obs_status}/{obs_lifecycle}/{obs_contract_state} confirm={confirm_history} ratify={ratify_history} task={task_id} linked={linked_count} create={task_create} lock={auto_promote_lock}");
    }
    if watch {
        println!("upstream obs-auto-promote LHU001 task={task_id} confirm={confirm_history} ratify={ratify_history} dispatched={dispatched} lock={auto_promote_lock}");
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        format!("LHU001 ready→{task_id}"),
        "observation confirm auto-ratified and auto-promote subscriber created a linked task through poll_once_with_guard",
    ))
}

fn run_reject_amend_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let conn = upstream_conn()?;
    let schema = bundled_schema_for_matrix("tasks")?;
    seed_human_verb_task_matrix(&conn, "HU002", "in_review")?;
    let reject_matches = display_id_matches("reject", "HU002");
    let unauthorized = crate::handlers::transition::run_reject(
        &schema,
        &conn,
        &reject_matches,
        Actor::AiAutonomous.into(),
        "unauthorized reject",
    );
    if unauthorized.is_ok() {
        bail!("reject unexpectedly allowed ai_autonomous");
    }
    crate::handlers::transition::run_reject(
        &schema,
        &conn,
        &reject_matches,
        Actor::Human.into(),
        "matrix reject reason",
    )?;
    let amend_matches = display_id_matches("amend", "HU002");
    let amend_unauth = crate::handlers::transition::run(
        &schema,
        &conn,
        &amend_matches,
        Actor::AiAutonomous.into(),
        "amend",
    );
    if amend_unauth.is_ok() {
        bail!("amend unexpectedly allowed ai_autonomous");
    }
    crate::handlers::transition::run(
        &schema,
        &conn,
        &amend_matches,
        Actor::AiWithHuman.into(),
        "amend",
    )?;
    let (status, lifecycle, active_step): (String, String, String) = task_tuple_matrix(&conn, "HU002")?;
    let reject_history = transition_count_matrix(&conn, "tasks", "HU002", "reject", "human")?;
    let amend_history = transition_count_matrix(&conn, "tasks", "HU002", "amend", "ai_with_human")?;
    let wrap_has_reason: i64 = conn.query_row(
        "SELECT CASE WHEN wrap_log LIKE '%matrix reject reason%' THEN 1 ELSE 0 END FROM tasks WHERE display_id='HU002'",
        [],
        |r| r.get(0),
    )?;
    if status != "planning" || lifecycle != "active" || active_step != "planning" || reject_history != 1 || amend_history != 1 || wrap_has_reason != 1 {
        bail!("reject/amend mismatch: {status}/{lifecycle}/{active_step} reject={reject_history} amend={amend_history} reason={wrap_has_reason}");
    }
    if watch {
        println!("upstream reject-amend HU002 status={status} reject={reject_history} amend={amend_history}");
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "HU002 in_review→rejected→planning",
        "real reject and amend handlers enforced actor tiers and recorded transition_history",
    ))
}

fn run_upstream_abandon_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SUBSTRATE_DDL)?;
    let schema = bundled_schema_for_matrix("tasks")?;
    conn.execute_batch(&ddl_for(&schema))?;
    seed_human_verb_task_matrix(&conn, "HU001", "planning")?;

    let cmd = clap::Command::new("abandon")
        .arg(clap::Arg::new("display_id").required(true).index(1))
        .arg(clap::Arg::new("reason").long("reason").required(true));
    let matches = cmd.get_matches_from(["abandon", "HU001", "--reason", "matrix-abandon-lab"]);
    crate::handlers::transition::run_abandon(
        &schema,
        &conn,
        &matches,
        Actor::Human.into(),
        "matrix-abandon-lab",
    )?;

    let status = task_status_matrix(&conn, "HU001");
    let (lifecycle, active_step, reason): (String, String, String) = conn.query_row(
        "SELECT lifecycle, active_step, abandoned_reason FROM tasks WHERE display_id='HU001'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let history_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM transition_history WHERE store='tasks' AND display_id='HU001' AND from_status='planning' AND to_status='abandoned' AND verb='abandon' AND invoker='human'",
        [],
        |r| r.get(0),
    )?;
    if status != "abandoned" || reason != "matrix-abandon-lab" || history_count != 1 {
        bail!(
            "abandon lab mismatch: status={status} lifecycle={lifecycle} active_step={active_step} reason={reason:?} history_count={history_count}"
        );
    }
    if watch {
        println!(
            "upstream abandon HU001 status={status} lifecycle={lifecycle} active_step={active_step} history_count={history_count}"
        );
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "HU001 planning→abandoned via real run_abandon handler",
        "real human-verb lab fired the abandon transition handler and verified status, reason, and transition_history",
    ))
}

fn run_close_out_of_band_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let conn = upstream_conn()?;
    let schema = bundled_schema_for_matrix("tasks")?;
    seed_human_verb_task_matrix(&conn, "HU003", "planning")?;
    let commit = git_sha_matrix(Path::new("."), "main")?;
    let matches = clap::Command::new("close-out-of-band")
        .arg(clap::Arg::new("display_id").required(true).index(1))
        .arg(clap::Arg::new("commit").long("commit").required(true))
        .get_matches_from(["close-out-of-band", "HU003", "--commit", commit.as_str()]);
    let unauthorized = crate::handlers::transition::run_close_out_of_band(
        &schema,
        &conn,
        &matches,
        Actor::AiAutonomous.into(),
        &commit,
    );
    if unauthorized.is_ok() {
        bail!("close-out-of-band unexpectedly allowed ai_autonomous");
    }
    crate::handlers::transition::run_close_out_of_band(
        &schema,
        &conn,
        &matches,
        Actor::Human.into(),
        &commit,
    )?;
    let (status, lifecycle, active_step): (String, String, String) = task_tuple_matrix(&conn, "HU003")?;
    let history = transition_count_matrix(&conn, "tasks", "HU003", "close-out-of-band", "human")?;
    let note: Option<String> = conn.query_row(
        "SELECT actor_note FROM transition_history WHERE store='tasks' AND display_id='HU003' AND verb='close-out-of-band' ORDER BY id DESC LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    if status != "closed_out_of_band" || lifecycle != "done" || active_step != "none" || history != 1 || note.as_deref() != Some(commit.as_str()) {
        bail!("close-out-of-band mismatch: {status}/{lifecycle}/{active_step} history={history} note={note:?}");
    }
    if watch {
        println!("upstream close-out-of-band HU003 commit={} history={history}", &commit[..7]);
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "HU003 planning→closed_out_of_band",
        "real close-out-of-band handler validated reachable main SHA and recorded actor_note provenance",
    ))
}

fn run_resume_blocked_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let conn = upstream_conn()?;
    let schema = bundled_schema_for_matrix("tasks")?;
    seed_human_verb_task_matrix(&conn, "HU004", "blocked")?;
    conn.execute("UPDATE tasks SET blocked=1, blocker_kind='runner', active_step='none', blocked_reason='matrix blocked' WHERE display_id='HU004'", [])?;
    let matches = display_id_matches("resume", "HU004");
    let unauthorized = crate::handlers::transition::run(
        &schema,
        &conn,
        &matches,
        Actor::AiAutonomous.into(),
        "resume",
    );
    if unauthorized.is_ok() {
        bail!("resume unexpectedly allowed ai_autonomous");
    }
    crate::handlers::transition::run(
        &schema,
        &conn,
        &matches,
        Actor::AiWithHuman.into(),
        "resume",
    )?;
    let (status, lifecycle, active_step): (String, String, String) = task_tuple_matrix(&conn, "HU004")?;
    let (blocked, blocker_kind): (i64, Option<String>) = conn.query_row(
        "SELECT blocked, blocker_kind FROM tasks WHERE display_id='HU004'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let history = transition_count_matrix(&conn, "tasks", "HU004", "resume", "ai_with_human")?;
    if status != "planning" || lifecycle != "active" || active_step != "planning" || blocked != 0 || blocker_kind.is_some() || history != 1 {
        bail!("resume mismatch: {status}/{lifecycle}/{active_step} blocked={blocked} kind={blocker_kind:?} history={history}");
    }
    if watch {
        println!("upstream resume-blocked HU004 status={status} history={history}");
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "HU004 blocked→planning",
        "real resume transition cleared blocked overlay with ai_with_human authority",
    ))
}

fn run_retry_integration_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let conn = upstream_conn()?;
    let schema = bundled_schema_for_matrix("tasks")?;
    seed_human_verb_task_matrix(&conn, "HU005", "integration_blocked")?;
    conn.execute("UPDATE tasks SET lifecycle='integration', active_step='none', integration_step='none', blocked=1, blocker_kind='main_red', integration_blocked_reason='matrix retry' WHERE display_id='HU005'", [])?;
    let matches = display_id_matches("retry-integration", "HU005");
    let unauthorized = crate::handlers::transition::run(
        &schema,
        &conn,
        &matches,
        Actor::AiAutonomous.into(),
        "retry-integration",
    );
    if unauthorized.is_ok() {
        bail!("retry-integration unexpectedly allowed ai_autonomous");
    }
    crate::handlers::transition::run(
        &schema,
        &conn,
        &matches,
        Actor::AiWithHuman.into(),
        "retry-integration",
    )?;
    let (status, lifecycle, integration_step): (String, String, String) = conn.query_row(
        "SELECT status,lifecycle,integration_step FROM tasks WHERE display_id='HU005'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let (blocked, blocker_kind): (i64, Option<String>) = conn.query_row(
        "SELECT blocked, blocker_kind FROM tasks WHERE display_id='HU005'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let history = transition_count_matrix(&conn, "tasks", "HU005", "retry-integration", "ai_with_human")?;
    if status != "integration_queued" || lifecycle != "integration" || integration_step != "none" || blocked != 0 || blocker_kind.is_some() || history != 1 {
        bail!("retry-integration mismatch: {status}/{lifecycle}/{integration_step} blocked={blocked} kind={blocker_kind:?} history={history}");
    }
    if watch {
        println!("upstream retry-integration HU005 status={status} history={history}");
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "HU005 integration_blocked→integration_queued",
        "real retry-integration transition requeued integration with ai_with_human authority",
    ))
}

fn run_dirty_worktree_refusal(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let lab = QueueLab::new()?;
    lab.branch_file_commit(
        "feat/dirty",
        "dirty.txt",
        "branch version\n",
        "dirty branch change",
    )?;
    git_ok(&lab.repo, &["checkout", "main"])?;
    std::fs::write(lab.repo.join("dirty.txt"), "uncommitted main version\n")?;
    lab.seed_task("BD001", "feat/dirty")?;
    lab.drive_until(10, |conn| {
        task_status_matrix(conn, "BD001") == "integration_blocked"
    })?;
    let outcome = lab.attempt_field("BD001", "outcome")?.unwrap_or_default();
    let reason = lab.blocked_reason("BD001")?;
    if outcome != "merge_failure" || !reason.contains("checkout") {
        bail!("expected dirty checkout merge_failure, got outcome={outcome:?} reason={reason:?}");
    }
    if watch {
        println!(
            "dirty-worktree-refusal outcome={} reason={}",
            outcome, reason
        );
    }
    Ok(queue_result_pass(
        spec,
        artifact_dir,
        "BD001 integration_blocked/merge_failure",
        "dirty worktree caused real git checkout refusal and routed to integration_blocked",
    ))
}

fn run_expected_harness_error_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    case_file: PathBuf,
    watch: bool,
    expected_error_fragment: &str,
    pass_message: &str,
) -> Result<MatrixCaseResult> {
    let opts = super::TestRunOpts {
        case_name: Some(spec.id.to_string()),
        case_file: Some(case_file),
        delay_ms: Some(0),
        watch,
        live: false,
    };
    match super::run(opts) {
        Ok(()) => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: "unexpected-success".to_string(),
            verdict: MatrixVerdict::Fail,
            artifact_dir: artifact_dir.display().to_string(),
            message: format!("expected harness error containing {expected_error_fragment:?}"),
        }),
        Err(err) if format!("{err:#}").contains(expected_error_fragment) => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: format!("{err:#}"),
            verdict: MatrixVerdict::Pass,
            artifact_dir: artifact_dir.display().to_string(),
            message: pass_message.to_string(),
        }),
        Err(err) => Err(err),
    }
}

fn run_no_heartbeat_case(
    spec: &TraversalSpec,
    artifact_dir: &Path,
    watch: bool,
) -> Result<MatrixCaseResult> {
    let old_timeout = std::env::var_os("STORES_RUNNER_NO_OUTPUT_SECS");
    std::env::set_var("STORES_RUNNER_NO_OUTPUT_SECS", "1");
    let case_file = write_runner_battlescar_case_file(artifact_dir, spec.id, "STALL_NO_HEARTBEAT")?;
    let opts = super::TestRunOpts {
        case_name: Some(spec.id.to_string()),
        case_file: Some(case_file),
        delay_ms: Some(2000),
        watch,
        live: false,
    };
    let result = match super::run(opts) {
        Ok(()) => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: "unexpected-success".to_string(),
            verdict: MatrixVerdict::Fail,
            artifact_dir: artifact_dir.display().to_string(),
            message: "expected no-heartbeat harness timeout".to_string(),
        }),
        Err(err) if format!("{err:#}").contains("timed out") => Ok(MatrixCaseResult {
            id: spec.id.to_string(),
            family: spec.family.to_string(),
            expected: spec.expected.to_string(),
            observed: format!("{err:#}"),
            verdict: MatrixVerdict::Pass,
            artifact_dir: artifact_dir.display().to_string(),
            message:
                "runner with no heartbeat/no output was classified as a blocked liveness failure"
                    .to_string(),
        }),
        Err(err) => Err(err),
    };
    match old_timeout {
        Some(value) => std::env::set_var("STORES_RUNNER_NO_OUTPUT_SECS", value),
        None => std::env::remove_var("STORES_RUNNER_NO_OUTPUT_SECS"),
    }
    result
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

fn write_runner_battlescar_case_file(
    artifact_dir: &Path,
    case_id: &str,
    planner_outcome: &str,
) -> Result<PathBuf> {
    let path = artifact_dir.join(format!("{case_id}-case.yaml"));
    std::fs::write(
        &path,
        format!(
            r#"cases:
  {case_id}:
    tier: T3
    executor_mode: marker_file
    stages:
      planner: {{ outcome: {planner_outcome} }}
    expect:
      task_status: blocked
      lifecycle: active
      external_review_status: missing
      no_real_llm: true
"#
        ),
    )
    .with_context(|| format!("writing battlescar runner case file {}", path.display()))?;
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

fn upstream_conn() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SUBSTRATE_DDL)?;
    for name in ["tasks", "observations", "external_reviews", "intake"] {
        let schema = bundled_schema_for_matrix(name)?;
        conn.execute_batch(&ddl_for(&schema))?;
    }
    Ok(conn)
}

fn display_id_matches(verb: &str, display_id: &str) -> clap::ArgMatches {
    clap::Command::new("matrix-verb")
        .arg(clap::Arg::new("display_id").required(true).index(1))
        .get_matches_from([verb, display_id])
}

fn transition_count_matrix(
    conn: &Connection,
    store: &str,
    display_id: &str,
    verb: &str,
    invoker: &str,
) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM transition_history WHERE store=?1 AND display_id=?2 AND verb=?3 AND invoker=?4",
        params![store, display_id, verb, invoker],
        |r| r.get(0),
    ).with_context(|| format!("count transition {store}/{display_id}/{verb}/{invoker}"))
}

fn task_tuple_matrix(conn: &Connection, display_id: &str) -> Result<(String, String, String)> {
    conn.query_row(
        "SELECT status,lifecycle,active_step FROM tasks WHERE display_id=?1",
        [display_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).with_context(|| format!("read task tuple for {display_id}"))
}

fn row_json_matrix(conn: &Connection, table: &str, display_id: &str) -> Result<serde_json::Value> {
    let sql = format!("SELECT * FROM {table} WHERE display_id=?1");
    let mut stmt = conn.prepare(&sql)?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query([display_id])?;
    let row = rows.next()?.with_context(|| format!("missing row {table}/{display_id}"))?;
    let mut obj = serde_json::Map::new();
    for (i, name) in cols.iter().enumerate() {
        let v: rusqlite::types::Value = row.get(i)?;
        let jv = match v {
            rusqlite::types::Value::Null => serde_json::Value::Null,
            rusqlite::types::Value::Integer(n) => serde_json::Value::from(n),
            rusqlite::types::Value::Real(f) => serde_json::json!(f),
            rusqlite::types::Value::Text(s) => serde_json::Value::String(s),
            rusqlite::types::Value::Blob(b) => serde_json::Value::String(String::from_utf8_lossy(&b).to_string()),
        };
        obj.insert(name.clone(), jv);
    }
    Ok(serde_json::Value::Object(obj))
}

fn seed_observation_matrix(conn: &Connection, display_id: &str) -> Result<()> {
    let now = "2026-05-18T00:00:00Z";
    let intent_contract = serde_json::json!({
        "contract_state":"ready",
        "objective":"auto-promote matrix objective",
        "type":"work",
        "in_scope":["prove auto-promote"],
        "out_of_scope":["production mutation"],
        "acceptance":["linked task exists"],
        "tier_hint":"T1",
        "approved_by":"blake",
        "approved_at":now
    });
    conn.execute(
        "INSERT INTO observations (display_id,status,summary,source,priority,captured_at,intent_contract,lifecycle,contract_state,waiting,waiting_kind,pending_architecture_review,created_at,updated_at,created_by,updated_by) \
         VALUES (?1,'investigating','matrix auto promote','dev','normal',?2,?3,'candidate','draft',1,'human_ratification',0,?2,?2,'stores-test','stores-test')",
        params![display_id, now, serde_json::to_string(&intent_contract)?],
    )?;
    Ok(())
}

fn seed_human_verb_task_matrix(conn: &Connection, display_id: &str, status: &str) -> Result<()> {
    let now = "2026-05-18T00:00:00Z";
    let contract =
        r#"{"done_when":"human verb matrix","scope_in":"fake upstream matrix","scope_out":"production"}"#;
    conn.execute(
        "INSERT INTO tasks (display_id, status, title, slug, contract, activation, lifecycle, active_step, integration_step, blocked, blocked_reason, created_at, updated_at, created_by, updated_by) \
         VALUES (?1, ?2, 'stores test human verb', 'stores-test-human-verb', ?3, 'active', 'active', 'planning', 'none', 0, '', ?4, ?4, 'stores-test', 'stores-test')",
        params![display_id, status, contract, now],
    )?;
    Ok(())
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
    coverage: &MatrixCoverageSummary,
    results: &[MatrixCaseResult],
) -> Result<()> {
    let index = serde_json::json!({
        "run_id": run_id,
        "mode": mode.as_str(),
        "catalog": catalog.as_str(),
        "coverage": coverage,
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
    md.push_str("\n## Coverage\n\n");
    md.push_str(&format!(
        "- schema_edges: {}\n",
        coverage_md_list(&coverage.schema_edges)
    ));
    md.push_str(&format!(
        "- runner_outcomes: {}\n",
        coverage_md_list(&coverage.runner_outcomes)
    ));
    md.push_str(&format!(
        "- perturbations: {}\n",
        coverage_md_list(&coverage.perturbations)
    ));
    md.push_str(&format!(
        "- authority_events: {}\n",
        coverage_md_list(&coverage.authority_events)
    ));
    std::fs::write(root.join("index.md"), md)
        .with_context(|| format!("writing {}", root.join("index.md").display()))?;
    Ok(())
}

fn coverage_md_list(tags: &[String]) -> String {
    if tags.is_empty() {
        "-".to_string()
    } else {
        tags.iter()
            .map(|tag| format!("`{tag}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
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

fn upstream_specs() -> Vec<TraversalSpec> {
    vec![
        TraversalSpec {
            id: "obs-auto-promote-happy",
            family: "upstream",
            description: "ready observation contract auto-promotes into a task",
            expected: "observation-promoted",
            coverage: CoverageTags {
                schema_edges: vec!["observations:ready:auto-promote:ready"],
                runner_outcomes: vec![],
                perturbations: vec![],
                authority_events: vec!["observation:approve-contract"],
            },
        },
        TraversalSpec {
            id: "reject-amend",
            family: "human-verb",
            description: "human rejects an in-review task and an amended contract reopens planning",
            expected: "rejected-then-amended",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:in_review:reject:rejected",
                    "tasks:rejected:amend:planning",
                ],
                runner_outcomes: vec![],
                perturbations: vec![],
                authority_events: vec!["task:reject", "task:amend"],
            },
        },
        TraversalSpec {
            id: "abandon",
            family: "human-verb",
            description: "human abandons a non-terminal task through the real transition handler",
            expected: "abandoned",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:planning:abandon:abandoned"],
                runner_outcomes: vec![],
                perturbations: vec![],
                authority_events: vec!["task:abandon"],
            },
        },
        TraversalSpec {
            id: "close-out-of-band",
            family: "human-verb",
            description: "human closes a task as shipped outside the substrate",
            expected: "closed-out-of-band",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:planning:close-out-of-band:closed_out_of_band"],
                runner_outcomes: vec![],
                perturbations: vec!["git:main_reachable_commit"],
                authority_events: vec!["task:close-out-of-band"],
            },
        },
        TraversalSpec {
            id: "resume-blocked",
            family: "human-verb",
            description: "human resumes a blocked task back to planning",
            expected: "resumed-to-planning",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:blocked:resume:planning"],
                runner_outcomes: vec![],
                perturbations: vec!["task:blocked"],
                authority_events: vec!["task:resume"],
            },
        },
        TraversalSpec {
            id: "retry-integration",
            family: "human-verb",
            description: "human retries a typed integration-blocked task back into the queue",
            expected: "integration-requeued",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:integration_blocked:retry-integration:integration_queued"],
                runner_outcomes: vec![],
                perturbations: vec!["task:integration_blocked"],
                authority_events: vec!["task:retry-integration"],
            },
        },
    ]
}

fn battlescar_specs() -> Vec<TraversalSpec> {
    vec![
        TraversalSpec {
            id: "dirty-worktree-refusal",
            family: "git-liveness",
            description: "dirty worktree blocks checkout before integration refresh",
            expected: "integration-blocked/dirty-worktree",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:integrating:mark_integration_blocked:integration_blocked",
                ],
                runner_outcomes: vec![],
                perturbations: vec!["git:dirty_worktree"],
                authority_events: vec!["policy:integration_queue"],
            },
        },
        TraversalSpec {
            id: "merge-conflict-blocked",
            family: "git-liveness",
            description: "front-of-queue merge/rebase conflict blocks integration cleanly",
            expected: "integration-blocked/conflict",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:integrating:mark_integration_blocked:integration_blocked",
                ],
                runner_outcomes: vec!["executor:marker_commit"],
                perturbations: vec!["git:merge_conflict"],
                authority_events: vec!["policy:integration_queue"],
            },
        },
        TraversalSpec {
            id: "stale-external-review-head-mutation",
            family: "freshness",
            description: "candidate head mutates after external review and requires fresh review",
            expected: "needs-review",
            coverage: CoverageTags {
                schema_edges: vec![
                    "tasks:integrating:mark_integration_blocked:integration_blocked",
                ],
                runner_outcomes: vec!["external_review:PASS"],
                perturbations: vec!["git:branch_head_changed_after_review"],
                authority_events: vec!["policy:external_review_freshness"],
            },
        },
        TraversalSpec {
            id: "payload-invalid",
            family: "runner-liveness",
            description: "runner exits 0 with invalid structured payload and blocks task",
            expected: "blocked/payload-invalid",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:planning:mark_drive_failed:blocked"],
                runner_outcomes: vec!["planner:PAYLOAD_INVALID_EXIT_0"],
                perturbations: vec!["runner:invalid_payload"],
                authority_events: vec![],
            },
        },
        TraversalSpec {
            id: "nonzero-exit",
            family: "runner-liveness",
            description: "runner exits nonzero and blocks task with structured runner_crash reason",
            expected: "blocked/nonzero-exit",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:planning:mark_drive_failed:blocked"],
                runner_outcomes: vec!["planner:NONZERO_EXIT"],
                perturbations: vec!["runner:nonzero_exit"],
                authority_events: vec![],
            },
        },
        TraversalSpec {
            id: "no-heartbeat",
            family: "runner-liveness",
            description: "runner produces no heartbeat/output and timeout blocks task",
            expected: "blocked/no-heartbeat",
            coverage: CoverageTags {
                schema_edges: vec!["tasks:planning:mark_drive_failed:blocked"],
                runner_outcomes: vec!["planner:STALL_NO_HEARTBEAT"],
                perturbations: vec!["runner:no_heartbeat"],
                authority_events: vec![],
            },
        },
        TraversalSpec {
            id: "duplicate-drive-refusal",
            family: "drive-liveness",
            description: "duplicate drive owner is refused instead of starting a second drive",
            expected: "held/live_dispatch_lock",
            coverage: CoverageTags {
                schema_edges: vec!["engine_runner:tasks:held:live_dispatch_lock"],
                runner_outcomes: vec![],
                perturbations: vec!["drive:duplicate_owner"],
                authority_events: vec![],
            },
        },
        TraversalSpec {
            id: "stale-dead-current-run-marker",
            family: "drive-liveness",
            description: "stale/dead current-run marker truth is classified without wedging",
            expected: "held/live_runner_marker",
            coverage: CoverageTags {
                schema_edges: vec!["engine_runner:tasks:held:live_runner_marker"],
                runner_outcomes: vec![],
                perturbations: vec!["drive:stale_dead_marker"],
                authority_events: vec![],
            },
        },
    ]
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
    fn battlescar_catalog_lists_batch_c_cases() {
        assert_eq!(Catalog::parse("battlescars").unwrap(), Catalog::Battlescars);
        assert_eq!(Catalog::Battlescars.as_str(), "battlescars");

        let ids: Vec<_> = catalog_specs(Catalog::Battlescars)
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "dirty-worktree-refusal",
                "merge-conflict-blocked",
                "stale-external-review-head-mutation",
                "payload-invalid",
                "nonzero-exit",
                "no-heartbeat",
                "duplicate-drive-refusal",
                "stale-dead-current-run-marker",
            ]
        );
    }

    #[test]
    fn upstream_catalog_lists_batch_d_cases() {
        assert_eq!(Catalog::parse("upstream").unwrap(), Catalog::Upstream);
        assert_eq!(Catalog::Upstream.as_str(), "upstream");

        let ids: Vec<_> = catalog_specs(Catalog::Upstream)
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "obs-auto-promote-happy",
                "reject-amend",
                "abandon",
                "close-out-of-band",
                "resume-blocked",
                "retry-integration",
            ]
        );
    }

    #[test]
    fn upstream_matrix_lab_output_runs_abandon_report() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("matrix-output");
        run_matrix_to_root(
            MatrixOpts {
                catalog: Catalog::Upstream,
                mode: MatrixMode::Lab,
                only: Some("abandon".to_string()),
                watch: false,
                current_ack: false,
                report: MatrixReport::Md,
                ci: false,
            },
            "unit-upstream-run",
            &root,
        )
        .unwrap();

        let index: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(root.join("index.json")).unwrap())
                .unwrap();
        assert_eq!(index["mode"], "lab");
        assert_eq!(index["catalog"], "upstream");
        assert_eq!(index["results"][0]["id"], "abandon");
        assert_eq!(index["results"][0]["verdict"], "PASS");

        let report = std::fs::read_to_string(root.join("index.md")).unwrap();
        assert!(report.contains("catalog: `upstream`"));
        assert!(report.contains("`abandon`"));
        assert!(root.join("abandon").join("proof.txt").exists());
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
                report: MatrixReport::Md,
                ci: false,
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
