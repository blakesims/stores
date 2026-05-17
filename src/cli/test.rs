use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::codegen::ddl::ddl_for;
use crate::flow::builtins::DispatchCtx;
use crate::flow::{AgentEntry, AgentsYaml, RetryPolicy};
use crate::handlers::drive::drive_loop;
use crate::schema::Schema;

#[path = "test/matrix/mod.rs"]
pub mod matrix;

#[derive(Debug, Clone)]
pub struct TestRunOpts {
    pub case_name: Option<String>,
    pub case_file: Option<PathBuf>,
    pub delay_ms: Option<u64>,
    pub watch: bool,
    pub live: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TestManifest {
    pub cases: BTreeMap<String, TestCase>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TestCase {
    #[serde(default = "default_tier")]
    pub tier: String,
    #[serde(default)]
    pub delay_ms: Option<u64>,
    #[serde(default = "default_executor_mode")]
    pub executor_mode: String,
    #[serde(default)]
    pub stages: BTreeMap<String, StageScript>,
    #[serde(default)]
    pub expect: CaseExpect,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct StageScript {
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub attempts: Vec<AttemptScript>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AttemptScript {
    pub outcome: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaseExpect {
    #[serde(default = "default_expect_task_status")]
    pub task_status: String,
    #[serde(default = "default_expect_lifecycle")]
    pub lifecycle: String,
    #[serde(default = "default_expect_external_review_status")]
    pub external_review_status: String,
    #[serde(default)]
    pub external_review: Option<String>,
    #[serde(default = "default_true")]
    pub no_real_llm: bool,
    #[allow(dead_code)]
    #[serde(default)]
    pub visited: Option<Vec<matrix::VisitedEdge>>,
}

impl Default for CaseExpect {
    fn default() -> Self {
        Self {
            task_status: default_expect_task_status(),
            lifecycle: default_expect_lifecycle(),
            external_review_status: default_expect_external_review_status(),
            external_review: None,
            no_real_llm: true,
            visited: None,
        }
    }
}

fn default_tier() -> String {
    "T3".to_string()
}
fn default_executor_mode() -> String {
    "marker_file".to_string()
}
fn default_expect_task_status() -> String {
    "integrated".to_string()
}
fn default_expect_lifecycle() -> String {
    "done".to_string()
}
fn default_expect_external_review_status() -> String {
    "passed".to_string()
}
fn default_true() -> bool {
    true
}

const PRESET_HAPPY: &str = r#"
cases:
  happy-path:
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
"#;

const PRESET_FAILED_ER: &str = r#"
cases:
  t3-failed-er:
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
"#;

const PRESET_STALE_BASE_REFUSES: &str = r#"
cases:
  stale-base-refuses:
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
      task_status: non_integrated
      lifecycle: active
      external_review_status: passed
      no_real_llm: true
"#;

pub fn run(opts: TestRunOpts) -> Result<()> {
    let (case_name, case, case_file_for_fake) = load_case(&opts)?;
    validate_case_shape(&case)?;
    let delay_ms = opts.delay_ms.or(case.delay_ms).unwrap_or(5000);
    preflight_fake_mode()?;

    let mut env_restore = EnvRestore::capture(&[
        "STORES_LLM_OFF",
        "STORES_FAKE_AGENT_BIN",
        "STORES_FAKE_SCENARIO",
        "STORES_FAKE_DELAY_MS",
        "STORES_FAKE_EXECUTOR_MODE",
        "STORES_FAKE_CASE_FILE",
        "STORES_FAKE_CASE_NAME",
        "STORES_ALLOW_FAKE_REVIEW_ACCEPT",
    ]);
    env_restore.set("STORES_LLM_OFF", "1");
    env_restore.set("STORES_FAKE_DELAY_MS", delay_ms.to_string());
    env_restore.set("STORES_FAKE_EXECUTOR_MODE", &case.executor_mode);
    env_restore.set("STORES_ALLOW_FAKE_REVIEW_ACCEPT", "1");
    env_restore.set("STORES_FAKE_CASE_NAME", &case_name);
    if let Some(path) = &case_file_for_fake {
        env_restore.set("STORES_FAKE_CASE_FILE", path.to_string_lossy().to_string());
    } else {
        env_restore.set("STORES_FAKE_SCENARIO", preset_scenario(&case_name));
    }
    if let Ok(bin) = fake_agent_bin() {
        env_restore.set("STORES_FAKE_AGENT_BIN", bin.to_string_lossy().to_string());
    }

    println!(
        "stores-test case={case_name} tier={} live={} llm_off=1 delay_ms={delay_ms} executor_mode={}",
        case.tier, opts.live, case.executor_mode
    );

    if opts.live {
        let h = LiveHarness::new(&case_name)?;
        if is_stale_base_refuses_case(&case_name) {
            return h.run_stale_base_refuses(opts.watch);
        }
        return h.run(&case, &case.expect, opts.watch);
    }

    let h = Harness::new(&case_name)?;
    let _cwd_restore = CwdRestore::pushd(h._tmp.path())?;
    if opts.watch {
        h.progress("created synthetic repo/db")?;
    }

    let runner = crate::runner::FakeRunner::with_bin(fake_agent_bin()?);
    drive_loop(&h.tasks_schema, &h.conn, &h.task_id, &runner, 40)
        .with_context(|| format!("drive loop failed for {}", h.task_id))?;
    if opts.watch {
        h.progress("after drive")?;
    }

    h.ensure_in_review_or_expected_failure()?;
    h.run_external_review()?;
    if opts.watch {
        h.progress("after external-review")?;
    }

    let er_status = h.external_review_status()?;
    if er_status == "passed" {
        h.accept_and_integrate()?;
        if opts.watch {
            h.progress("after integration")?;
        }
    }

    h.assert_expectations(&case.expect)?;
    if case.expect.no_real_llm {
        h.assert_no_real_llm()?;
    }
    h.summary()?;
    drop(env_restore);
    Ok(())
}

fn load_case(opts: &TestRunOpts) -> Result<(String, TestCase, Option<PathBuf>)> {
    if let Some(path) = &opts.case_file {
        let case_path = path
            .canonicalize()
            .with_context(|| format!("resolving {}", path.display()))?;
        let raw = std::fs::read_to_string(&case_path)
            .with_context(|| format!("reading {}", case_path.display()))?;
        validate_case_manifest_yaml(&raw)?;
        let manifest: TestManifest =
            serde_yaml::from_str(&raw).context("parsing test case YAML")?;
        let name = opts
            .case_name
            .clone()
            .or_else(|| manifest.cases.keys().next().cloned())
            .context("case file must contain at least one case")?;
        let case = manifest
            .cases
            .get(&name)
            .with_context(|| format!("case '{name}' not found"))?
            .clone();
        return Ok((name, case, Some(case_path)));
    }
    let name = opts
        .case_name
        .clone()
        .unwrap_or_else(|| "happy-path".to_string());
    let raw = match name.as_str() {
        "happy-path" => PRESET_HAPPY,
        "t3-failed-er" | "t3-er-fail" => PRESET_FAILED_ER,
        "stale-base-refuses" => PRESET_STALE_BASE_REFUSES,
        other => bail!("unknown stores test preset '{other}' (use --case-file for custom YAML)"),
    };
    validate_case_manifest_yaml(raw)?;
    let manifest: TestManifest = serde_yaml::from_str(raw).unwrap();
    let key = if name == "t3-er-fail" {
        "t3-failed-er"
    } else {
        &name
    };
    let case = manifest.cases.get(key).unwrap().clone();
    let file = std::env::temp_dir().join(format!("stores-test-{key}-case.yaml"));
    std::fs::write(&file, raw)?;
    Ok((key.to_string(), case, Some(file)))
}

fn preset_scenario(name: &str) -> &'static str {
    match name {
        "t3-failed-er" | "t3-er-fail" => "external-review-tooling-failure",
        _ => "all-pass",
    }
}

fn is_stale_base_refuses_case(case_name: &str) -> bool {
    case_name == "stale-base-refuses"
}

fn validate_case_manifest_yaml(raw: &str) -> Result<()> {
    let value: serde_yaml::Value =
        serde_yaml::from_str(raw).context("parsing test case YAML for validation")?;
    validate_no_consequence_faking(&value, &mut Vec::new(), "$")
}

fn validate_no_consequence_faking(
    value: &serde_yaml::Value,
    path_parts: &mut Vec<String>,
    path: &str,
) -> Result<()> {
    match value {
        serde_yaml::Value::Mapping(map) => {
            for (key, nested) in map {
                let key_name = key.as_str().unwrap_or("<non-string-key>");
                let next_path = format!("{path}.{key_name}");
                path_parts.push(key_name.to_string());
                if is_forbidden_consequence_key(key_name) && !is_under_case_level_expect(path_parts)
                {
                    bail!(
                        "case DSL field '{key_name}' is only allowed under $.cases.<case-id>.expect (found at {next_path})"
                    );
                }
                validate_no_consequence_faking(nested, path_parts, &next_path)?;
                path_parts.pop();
            }
        }
        serde_yaml::Value::Sequence(items) => {
            for (idx, nested) in items.iter().enumerate() {
                validate_no_consequence_faking(nested, path_parts, &format!("{path}[{idx}]"))?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn is_under_case_level_expect(path_parts: &[String]) -> bool {
    path_parts.len() >= 4 && path_parts[0] == "cases" && path_parts[2] == "expect"
}

fn is_forbidden_consequence_key(key: &str) -> bool {
    matches!(
        key,
        "final_status"
            | "force_status"
            | "external_review_status"
            | "integration_result"
            | "blocked_reason"
            | "stale_base"
            | "stale_external_review"
    )
}

fn validate_case_shape(case: &TestCase) -> Result<()> {
    for (role, stage) in &case.stages {
        if stage.outcome.is_none() && stage.attempts.is_empty() {
            bail!("stage '{role}' must define outcome or attempts");
        }
        for attempt in &stage.attempts {
            if attempt.outcome.trim().is_empty() {
                bail!("stage '{role}' has an empty attempt outcome");
            }
        }
    }
    Ok(())
}

fn preflight_fake_mode() -> Result<()> {
    let current = stores_bin_for_preflight().context("resolve current stores binary")?;
    sentinel(&current, "current stores binary")?;
    if let Ok(private) = crate::paths::daemon_binary_path() {
        let needs_refresh =
            !private.exists() || !same_file_bytes(&current, &private).unwrap_or(false);
        if needs_refresh {
            if let Some(parent) = private.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&current, &private).with_context(|| {
                format!(
                    "installing current stores binary for private daemon reexec {}",
                    private.display()
                )
            })?;
        }
        sentinel(&private, "private daemon reexec binary")?;
    }
    let fake = fake_agent_bin()?;
    if !fake.exists() {
        bail!("stores-fake-agent not found at {}", fake.display());
    }
    Ok(())
}

fn same_file_bytes(left: &Path, right: &Path) -> Result<bool> {
    Ok(std::fs::read(left)? == std::fs::read(right)?)
}

fn stores_bin_for_preflight() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = std::env::var_os("STORES_TEST_STORES_BIN") {
        return Ok(PathBuf::from(path));
    }
    std::env::current_exe().context("resolve current stores binary")
}

fn sentinel(bin: &Path, label: &str) -> Result<()> {
    let out = Command::new(bin)
        .arg("__llm-off-sentinel")
        .env("STORES_LLM_OFF", "1")
        .output()
        .with_context(|| format!("running sentinel for {label}: {}", bin.display()))?;
    if !out.status.success()
        || !String::from_utf8_lossy(&out.stdout).contains("stores-llm-off-sentinel=ok")
    {
        bail!(
            "fake-mode sentinel failed for {label}: status={} stderr={}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    println!("preflight {label}=ok path={}", bin.display());
    Ok(())
}

fn fake_agent_bin() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("STORES_FAKE_AGENT_BIN") {
        return Ok(PathBuf::from(p));
    }
    let cur = std::env::current_exe()?;
    if let Some(parent) = cur.parent() {
        let sibling = parent.join("stores-fake-agent");
        if sibling.exists() {
            return Ok(sibling);
        }
    }
    Ok(PathBuf::from("stores-fake-agent"))
}

fn ensure_stores_test_task_provenance(conn: &Connection, task_id: &str) -> Result<()> {
    let row: Option<(String, String, Option<String>)> = conn
        .query_row(
            "SELECT title, slug, contract FROM tasks WHERE display_id=?1",
            [task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .with_context(|| format!("reading test task provenance for {task_id}"))?;
    let Some((title, slug, contract)) = row else {
        bail!("task {task_id} not found for test-authorized action");
    };

    let title_is_test = title.starts_with("stores test live ") || title == "stores test synthetic";
    let slug_is_test = slug.starts_with("stores-test-live-") || slug == "stores-test";
    let contract_is_test = contract.as_deref().is_some_and(|raw| {
        raw.contains("stores test live fake runner rows only")
            || raw.contains("fake harness reaches expectation")
            || raw.contains("fake harness only")
    });

    if title_is_test && slug_is_test && contract_is_test {
        return Ok(());
    }

    bail!(
        "task {task_id} is not stores-test owned; title={title:?} slug={slug:?}. refusing test-authorized fake-review action"
    )
}

struct LiveHarness {
    case_name: String,
    task_id: String,
    db_path: PathBuf,
    root: PathBuf,
}

impl LiveHarness {
    fn new(case_name: &str) -> Result<Self> {
        let root = std::env::current_dir().context("resolve live repo root")?;
        let db_path = crate::paths::db_path()?;
        if !db_path.exists() {
            bail!(
                "live mode requires current repo .stores/db.sqlite at {}",
                db_path.display()
            );
        }
        let backup_path = backup_live_db(&db_path)?;
        println!("live db backup={}", backup_path.display());
        let task_id = create_live_task(case_name)?;
        Ok(Self {
            case_name: case_name.to_string(),
            task_id,
            db_path,
            root,
        })
    }

    fn run(&self, _case: &TestCase, expect: &CaseExpect, watch: bool) -> Result<()> {
        println!(
            "live task={} status_cmd=`stores tasks status {}` watch_cmd=`stores watch --all`",
            self.task_id, self.task_id
        );
        if watch {
            self.progress("created")?;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
        let mut released = false;
        let mut last = String::new();
        loop {
            self.run_daemon_once()?;
            let snap = self.snapshot()?;
            let line = format!(
                "task={} status={} lifecycle={} active_step={} er={:?}",
                self.task_id,
                snap.status,
                snap.lifecycle.clone().unwrap_or_default(),
                snap.active_step.clone().unwrap_or_default(),
                snap.er
            );
            if watch || line != last {
                println!("progress live: {line}");
                last = line;
            }
            if !released
                && expect.task_status == "integrated"
                && snap.status == "in_review"
                && snap.er_status() == Some("passed")
            {
                self.accept_for_integration()?;
                released = true;
                println!(
                    "progress live: task={} accepted-for-daemon-integration",
                    self.task_id
                );
                continue;
            }
            if self.matches_expect(expect, &snap) {
                self.assert_no_real_llm()?;
                if expect.task_status != "integrated" {
                    self.isolate_live_case()?;
                    let stable = self.snapshot()?;
                    if !self.matches_expect(expect, &stable) {
                        bail!(
                            "live case {} changed during isolation: before={:?} after={:?}",
                            self.case_name,
                            snap,
                            stable
                        );
                    }
                    println!(
                        "progress live: task={} isolated activation=inactive",
                        self.task_id
                    );
                }
                println!(
                    "summary live task={} status={} lifecycle={} er={:?} no_real_llm=ok marker_files_on_main_are_intentional_fake_runner_proof",
                    self.task_id,
                    snap.status,
                    snap.lifecycle.unwrap_or_default(),
                    snap.er
                );
                return Ok(());
            }
            if matches!(
                snap.status.as_str(),
                "blocked" | "deploy_blocked" | "rejected" | "integration_blocked"
            ) && expect.task_status != snap.status
            {
                bail!(
                    "live task {} reached unexpected terminal/blocked state: {:?}",
                    self.task_id,
                    snap
                );
            }
            if std::time::Instant::now() > deadline {
                bail!(
                    "timed out waiting for live case {} task {} to reach expectations",
                    self.case_name,
                    self.task_id
                );
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    fn run_stale_base_refuses(&self, watch: bool) -> Result<()> {
        println!(
            "[setup] task={} status_cmd=`stores tasks status {}` watch_cmd=`stores watch --all`",
            self.task_id, self.task_id
        );
        let workspace = self.wait_for_workspace(watch)?;
        let base_a = git_sha(&self.root, "main")?;
        println!(
            "[setup] task={} worktree={} branch={} base A={}",
            self.task_id,
            workspace.workspace_path.display(),
            workspace.branch,
            base_a
        );

        let er = self.wait_for_fake_er_pass(watch)?;
        let er_base = required_er_sha(er.base_sha.as_deref(), "base_sha", &er.display_id)?;
        let er_head = required_er_sha(er.head_sha.as_deref(), "head_sha", &er.display_id)?;
        let task_head_x = git_sha(&workspace.workspace_path, "HEAD")?;
        if er_head != task_head_x {
            bail!(
                "fake ER head proof does not match task worktree HEAD: er={} head={} worktree_head={}",
                er.display_id,
                er_head,
                task_head_x
            );
        }
        let task_markers = fake_marker_files_at_head(&workspace.workspace_path)?;
        println!(
            "[executor] task head X={} markers={}",
            task_head_x,
            proof_list(&task_markers)
        );
        println!(
            "[external-review] {} runner={} status={} verdict={} base={} head={} superseded_by={}",
            er.display_id,
            er.runner.as_deref().unwrap_or("-"),
            er.status,
            er.verdict.as_deref().unwrap_or("-"),
            er_base,
            er_head,
            er.superseded_by.as_deref().unwrap_or("-")
        );

        let main_marker = main_advance_marker_path(&self.task_id, &self.case_name);
        let main_commit = self.advance_main_with_marker(&main_marker)?;
        let main_b = git_sha(&self.root, "main")?;
        if main_b == base_a {
            bail!("main marker commit did not advance main: base={base_a} current={main_b}");
        }
        println!(
            "[setup] advanced main B={} commit={} marker={}",
            main_b,
            main_commit,
            main_marker.display()
        );

        let refusal = self.attempt_accept_enqueue_and_integration()?;
        if !is_freshness_refusal(&refusal.combined_output) {
            let snap = self.snapshot()?;
            if matches!(snap.status.as_str(), "integrated" | "done")
                || snap.lifecycle.as_deref() == Some("done")
            {
                bail!(
                    "stale-base-refuses RED proof: task integrated after main moved; output={} snapshot={:?}",
                    refusal.combined_output,
                    snap
                );
            }
            let state_reason = snap.refusal_evidence();
            if !is_freshness_refusal(&state_reason) {
                self.assert_no_real_llm()?;
                println!("[assert] no_real_llm=ok non_fake_agent_runs=0 real_er_runners=0");
                bail!(
                    "stale-base-refuses RED proof: expected stale/freshness refusal, but live integration did not produce one; command_output={} state_evidence={} snapshot={:?}",
                    refusal.combined_output,
                    state_reason,
                    snap
                );
            }
        }

        self.assert_no_real_llm()?;
        let final_snap = self.snapshot()?;
        if matches!(final_snap.status.as_str(), "integrated" | "done")
            || final_snap.lifecycle.as_deref() == Some("done")
        {
            bail!("expected non-integrated task after freshness refusal, got {final_snap:?}");
        }
        println!("[accept/integration] refused reason={}", refusal.summary());
        println!(
            "[assert] PASS freshness refusal was genuine integrated=false task={} status={} lifecycle={}",
            self.task_id,
            final_snap.status,
            final_snap.lifecycle.as_deref().unwrap_or("-")
        );
        println!("[assert] no_real_llm=ok non_fake_agent_runs=0 real_er_runners=0");
        println!("[watch] stores watch --all");
        println!(
            "[cleanup optional] stores tasks deactivate {} --reason 'stores test stale-base-refuses proof captured' --invoker ai_with_human",
            self.task_id
        );
        Ok(())
    }

    fn wait_for_workspace(&self, watch: bool) -> Result<LiveWorkspaceProof> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            self.run_daemon_once()?;
            if let Some(workspace) = self.workspace_proof()? {
                return Ok(workspace);
            }
            if watch {
                self.progress("waiting-workspace")?;
            }
            if std::time::Instant::now() > deadline {
                bail!(
                    "timed out waiting for live task {} workspace/branch",
                    self.task_id
                );
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    fn wait_for_fake_er_pass(&self, watch: bool) -> Result<LiveExternalReviewProof> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(900);
        loop {
            self.run_daemon_once()?;
            if let Some(er) = self.latest_er_proof()? {
                if er.status == "passed" && er.verdict.as_deref() == Some("PASS") {
                    if er.runner.as_deref() != Some("fake") {
                        bail!("expected fake external review runner, got {er:?}");
                    }
                    return Ok(er);
                }
            }
            if watch {
                self.progress("waiting-fake-er-pass")?;
            }
            if std::time::Instant::now() > deadline {
                bail!(
                    "timed out waiting for fake ER PASS for live task {}",
                    self.task_id
                );
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    fn workspace_proof(&self) -> Result<Option<LiveWorkspaceProof>> {
        let conn = self.conn()?;
        let row: Option<(Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT workspace_path,branch FROM tasks WHERE display_id=?1",
                [&self.task_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        let Some((workspace_path, branch)) = row else {
            return Ok(None);
        };
        let Some(workspace_path) = workspace_path.filter(|s| !s.trim().is_empty()) else {
            return Ok(None);
        };
        let Some(branch) = branch.filter(|s| !s.trim().is_empty()) else {
            return Ok(None);
        };
        let path = PathBuf::from(workspace_path);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(LiveWorkspaceProof {
            workspace_path: path,
            branch,
        }))
    }

    fn latest_er_proof(&self) -> Result<Option<LiveExternalReviewProof>> {
        let conn = self.conn()?;
        let verdict_expr = optional_column_expr(&conn, "external_reviews", "verdict")?;
        let runner_expr = optional_column_expr(&conn, "external_reviews", "runner")?;
        let base_sha_expr = optional_column_expr(&conn, "external_reviews", "base_sha")?;
        let head_sha_expr = optional_column_expr(&conn, "external_reviews", "head_sha")?;
        let superseded_by_expr = optional_column_expr(&conn, "external_reviews", "superseded_by")?;
        let sql = format!(
            "SELECT display_id,status,{verdict_expr},{runner_expr},{base_sha_expr},{head_sha_expr},{superseded_by_expr} \
             FROM external_reviews WHERE task_id=?1 ORDER BY id DESC LIMIT 1"
        );
        let er = conn
            .query_row(&sql, [&self.task_id], |r| {
                Ok(LiveExternalReviewProof {
                    display_id: r.get(0)?,
                    status: r.get(1)?,
                    verdict: r.get(2)?,
                    runner: r.get(3)?,
                    base_sha: r.get(4)?,
                    head_sha: r.get(5)?,
                    superseded_by: r.get(6)?,
                })
            })
            .ok();
        Ok(er)
    }

    fn advance_main_with_marker(&self, marker: &Path) -> Result<String> {
        let current_branch = git_stdout(&self.root, &["rev-parse", "--abbrev-ref", "HEAD"])?;
        if current_branch.trim() != "main" {
            bail!("live stale-base scenario must run from main; current branch={current_branch}");
        }
        let full_path = self.root.join(marker);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating marker dir {}", parent.display()))?;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        std::fs::write(
            &full_path,
            format!(
                "stores test stale-base-refuses\ntask={}\ncase={}\ncreated_unix={}\n",
                self.task_id,
                self.case_name,
                now.as_secs()
            ),
        )
        .with_context(|| format!("writing marker {}", full_path.display()))?;
        let marker_str = marker
            .to_str()
            .with_context(|| format!("marker path is not utf-8: {}", marker.display()))?;
        git_ok(&self.root, &["add", marker_str])?;
        let message = main_advance_commit_message(&self.task_id);
        git_ok(&self.root, &["commit", "-m", &message])?;
        git_sha(&self.root, "HEAD")
    }

    fn attempt_accept_enqueue_and_integration(&self) -> Result<LiveRefusalProof> {
        let mut outputs = Vec::new();
        let accept = self.test_authorized_accept_output()?;
        outputs.push(accept.clone());
        if !accept.success {
            return Ok(LiveRefusalProof::from_outputs(outputs));
        }

        let enqueue = run_live_stores_cmd_output(
            &self.root,
            ["tasks", "enqueue-integration", &self.task_id],
            "stores tasks enqueue-integration",
        )?;
        outputs.push(enqueue.clone());
        if !enqueue.success {
            return Ok(LiveRefusalProof::from_outputs(outputs));
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
        let mut last_status = String::new();
        let mut task_review_stall_seen_at: Option<std::time::Instant> = None;
        loop {
            let daemon = match self.run_daemon_once() {
                Ok(()) => LiveCommandOutput {
                    label: "stores agents run --once".to_string(),
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
                Err(err) => LiveCommandOutput {
                    label: "stores agents run --once".to_string(),
                    success: false,
                    stdout: String::new(),
                    stderr: err.to_string(),
                },
            };
            let daemon_success = daemon.success;
            outputs.push(daemon);

            let snap = self.snapshot()?;
            let mut state_evidence = snap.refusal_evidence();
            if let Some(lock) = self.latest_integrate_lock_evidence()? {
                if !state_evidence.is_empty() {
                    state_evidence.push('\n');
                }
                state_evidence.push_str(&lock);
            }
            let status_line = format!(
                "task={} status={} lifecycle={} evidence={}",
                self.task_id,
                snap.status,
                snap.lifecycle.as_deref().unwrap_or("-"),
                state_evidence
            );
            if status_line != last_status {
                outputs.push(LiveCommandOutput {
                    label: "integration state".to_string(),
                    success: true,
                    stdout: status_line.clone(),
                    stderr: String::new(),
                });
                last_status = status_line;
            }
            if is_freshness_refusal(&state_evidence)
                || matches!(
                    snap.status.as_str(),
                    "blocked" | "deploy_blocked" | "integration_blocked"
                )
                || matches!(snap.status.as_str(), "integrated" | "done")
                || snap.lifecycle.as_deref() == Some("done")
            {
                return Ok(LiveRefusalProof::from_outputs(outputs));
            }
            if snap.status == "integrating"
                && snap.integration_step.as_deref() == Some("task_review")
                && snap.integration_attempts.as_deref().unwrap_or("") == "null"
                && state_evidence.contains("integrate_lock=unfinished")
            {
                let first_seen =
                    *task_review_stall_seen_at.get_or_insert_with(std::time::Instant::now);
                if first_seen.elapsed() >= std::time::Duration::from_secs(12) {
                    outputs.push(LiveCommandOutput {
                        label: "integration task_review stall".to_string(),
                        success: false,
                        stdout: String::new(),
                        stderr: format!(
                            "integrate_dispatch_lock_unfinished_after_mark_refresh_done: task parked at integration_step=task_review with integration_attempts=null; {state_evidence}"
                        ),
                    });
                    return Ok(LiveRefusalProof::from_outputs(outputs));
                }
            } else {
                task_review_stall_seen_at = None;
            }
            if !daemon_success {
                return Ok(LiveRefusalProof::from_outputs(outputs));
            }
            if std::time::Instant::now() > deadline {
                outputs.push(LiveCommandOutput {
                    label: "integration wait".to_string(),
                    success: false,
                    stdout: String::new(),
                    stderr: format!("timed out waiting for integration refusal; last {snap:?}"),
                });
                return Ok(LiveRefusalProof::from_outputs(outputs));
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }

    fn conn(&self) -> Result<Connection> {
        let mut last_err = None;
        for _ in 0..20 {
            match crate::db::open(&self.db_path) {
                Ok(conn) => return Ok(conn),
                Err(err) if err.to_string().to_ascii_lowercase().contains("locked") => {
                    last_err = Some(err);
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("database open failed")))
    }

    fn run_daemon_once(&self) -> Result<()> {
        let mut last = None;
        for _ in 0..20 {
            let out = Command::new(stores_bin_for_preflight()?)
                .args(["agents", "run", "--once", "--poll-interval", "0.2"])
                .current_dir(&self.root)
                .env("STORES_LLM_OFF", "1")
                .env("STORES_PRIVATE_DAEMON_REEXEC", "1")
                .env("LIBSQLITE3_SYS_USE_PKG_CONFIG", "1")
                .output()
                .context("running live daemon once")?;
            if out.status.success() {
                return Ok(());
            }
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let locked = |s: &str| s.contains("database") && s.contains("locked");
            if !locked(&stderr) && !locked(&stdout) {
                bail!(
                    "stores agents run --once failed: status={} stderr={} stdout={}",
                    out.status,
                    stderr,
                    stdout
                );
            }
            last = Some((out.status, stderr, stdout));
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        let (status, stderr, stdout) = last.expect("daemon retry recorded last failure");
        bail!(
            "stores agents run --once failed after lock retries: status={} stderr={} stdout={}",
            status,
            stderr,
            stdout
        )
    }

    fn snapshot(&self) -> Result<LiveSnapshot> {
        let conn = self.conn()?;
        let lifecycle_expr = optional_column_expr(&conn, "tasks", "lifecycle")?;
        let active_step_expr = optional_column_expr(&conn, "tasks", "active_step")?;
        let workspace_path_expr = optional_column_expr(&conn, "tasks", "workspace_path")?;
        let branch_expr = optional_column_expr(&conn, "tasks", "branch")?;
        let blocked_reason_expr = optional_column_expr(&conn, "tasks", "blocked_reason")?;
        let blocker_kind_expr = optional_column_expr(&conn, "tasks", "blocker_kind")?;
        let blocked_reason_class_expr =
            optional_column_expr(&conn, "tasks", "blocked_reason_class")?;
        let integration_attempts_expr =
            optional_column_expr(&conn, "tasks", "integration_attempts")?;
        let integration_blocked_reason_expr =
            optional_column_expr(&conn, "tasks", "integration_blocked_reason")?;
        let integration_step_expr = optional_column_expr(&conn, "tasks", "integration_step")?;
        let sql = format!(
            "SELECT status,{lifecycle_expr},{active_step_expr},{workspace_path_expr},{branch_expr},{blocked_reason_expr},{blocker_kind_expr},{blocked_reason_class_expr},{integration_attempts_expr},{integration_blocked_reason_expr},{integration_step_expr} FROM tasks WHERE display_id=?1"
        );
        let (
            status,
            lifecycle,
            active_step,
            workspace_path,
            branch,
            blocked_reason,
            blocker_kind,
            blocked_reason_class,
            integration_attempts,
            integration_blocked_reason,
            integration_step,
        ): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = conn.query_row(&sql, [&self.task_id], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
                r.get(8)?,
                r.get(9)?,
                r.get(10)?,
            ))
        })?;
        let verdict_expr = optional_column_expr(&conn, "external_reviews", "verdict")?;
        let runner_expr = optional_column_expr(&conn, "external_reviews", "runner")?;
        let er_sql = format!(
            "SELECT display_id,status,{verdict_expr},{runner_expr} FROM external_reviews WHERE task_id=?1 ORDER BY id DESC LIMIT 1"
        );
        let er: Option<(String, String, Option<String>, Option<String>)> = conn
            .query_row(&er_sql, [&self.task_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .ok();
        Ok(LiveSnapshot {
            status,
            lifecycle,
            active_step,
            workspace_path,
            branch,
            blocked_reason,
            blocker_kind,
            blocked_reason_class,
            integration_attempts,
            integration_blocked_reason,
            integration_step,
            er,
        })
    }

    fn progress(&self, label: &str) -> Result<()> {
        let snap = self.snapshot()?;
        println!(
            "progress live {label}: task={} status={} lifecycle={:?} active_step={:?} workspace={:?} branch={:?} er={:?}",
            self.task_id,
            snap.status,
            snap.lifecycle,
            snap.active_step,
            snap.workspace_path,
            snap.branch,
            snap.er
        );
        Ok(())
    }

    fn accept_for_integration(&self) -> Result<()> {
        let accept = self.test_authorized_accept_output()?;
        if !accept.success {
            bail!(
                "stores test-authorized accept failed: stderr={} stdout={}",
                accept.stderr,
                accept.stdout
            );
        }
        run_live_stores_cmd(
            &self.root,
            ["tasks", "enqueue-integration", &self.task_id],
            "stores tasks enqueue-integration",
        )?;
        Ok(())
    }

    fn test_authorized_accept_output(&self) -> Result<LiveCommandOutput> {
        let conn = self.conn()?;
        ensure_stores_test_task_provenance(&conn, &self.task_id).with_context(|| {
            format!(
                "refusing fake-review accept for non-test task {}",
                self.task_id
            )
        })?;
        run_live_stores_cmd_output(
            &self.root,
            ["tasks", "accept", &self.task_id, "--invoker", "human"],
            "stores test-authorized tasks accept",
        )
    }

    fn isolate_live_case(&self) -> Result<()> {
        // Phase-0 safety: live/current-repo cases must not raw-SQL write
        // substrate rows to manufacture or freeze outcomes. Deactivation goes
        // through the normal task verb; any future retry control needs a real
        // external-review/test-authority verb rather than an UPDATE here.
        let out = Command::new(stores_bin_for_preflight()?)
            .args([
                "tasks",
                "deactivate",
                &self.task_id,
                "--reason",
                "stores test live case reached expected held state",
                "--invoker",
                "ai_with_human",
            ])
            .current_dir(&self.root)
            .env("STORES_LLM_OFF", "1")
            .output()
            .context("deactivating held live test task")?;
        if !out.status.success() {
            bail!(
                "stores tasks deactivate failed: status={} stderr={} stdout={}",
                out.status,
                String::from_utf8_lossy(&out.stderr),
                String::from_utf8_lossy(&out.stdout)
            );
        }
        Ok(())
    }

    fn matches_expect(&self, expect: &CaseExpect, snap: &LiveSnapshot) -> bool {
        let want_er = expect
            .external_review
            .as_deref()
            .unwrap_or(&expect.external_review_status);
        snap.status == expect.task_status
            && snap.lifecycle.as_deref().unwrap_or("") == expect.lifecycle
            && snap.er_status() == Some(want_er)
    }

    fn latest_integrate_lock_evidence(&self) -> Result<Option<String>> {
        let conn = self.conn()?;
        let has_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='dispatch_locks'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if has_table == 0 {
            return Ok(None);
        }
        let row: Option<(String, Option<String>, Option<String>, i64, Option<String>, Option<String>)> = conn
            .query_row(
                "SELECT claimed_at,last_status,finished_at,attempts,terminal_reason,postcondition_id \
                 FROM dispatch_locks WHERE display_id=?1 AND agent_name='integrate' \
                 ORDER BY id DESC LIMIT 1",
                [&self.task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            )
            .ok();
        Ok(row.map(
            |(claimed_at, last_status, finished_at, attempts, terminal_reason, postcondition_id)| {
                let state = if finished_at.is_some() { "finished" } else { "unfinished" };
                format!(
                    "integrate_lock={state} claimed_at={claimed_at} attempts={attempts} last_status={} terminal_reason={} postcondition={}",
                    last_status.as_deref().unwrap_or("-"),
                    terminal_reason.as_deref().unwrap_or("-"),
                    postcondition_id.as_deref().unwrap_or("-"),
                )
            },
        ))
    }

    fn assert_no_real_llm(&self) -> Result<()> {
        let conn = self.conn()?;
        let non_fake: i64 = conn.query_row("SELECT COUNT(*) FROM agent_runs WHERE display_id=?1 AND COALESCE(harness_id,'') != 'fake'", [&self.task_id], |r| r.get(0)).unwrap_or(0);
        if non_fake != 0 {
            bail!(
                "expected zero non-fake agent_runs for {}, got {non_fake}",
                self.task_id
            );
        }
        let real_er: i64 = conn.query_row("SELECT COUNT(*) FROM external_reviews WHERE task_id=?1 AND COALESCE(runner,'') IN ('codex','pi','claude-code')", [&self.task_id], |r| r.get(0)).unwrap_or(0);
        if real_er != 0 {
            bail!(
                "expected zero real-runner external_reviews for {}, got {real_er}",
                self.task_id
            );
        }
        Ok(())
    }
}

fn optional_column_expr(conn: &Connection, table: &str, column: &str) -> Result<String> {
    if live_table_has_column(conn, table, column)? {
        Ok(column.to_string())
    } else {
        Ok("NULL".to_string())
    }
}

fn live_table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn run_live_stores_cmd<const N: usize>(root: &Path, args: [&str; N], label: &str) -> Result<()> {
    let out = run_live_stores_cmd_output(root, args, label)?;
    if !out.success {
        bail!(
            "{label} failed: stderr={} stdout={}",
            out.stderr,
            out.stdout
        );
    }
    Ok(())
}

fn run_live_stores_cmd_output<const N: usize>(
    root: &Path,
    args: [&str; N],
    label: &str,
) -> Result<LiveCommandOutput> {
    let out = Command::new(stores_bin_for_preflight()?)
        .args(args)
        .current_dir(root)
        .env("STORES_LLM_OFF", "1")
        .env("STORES_ALLOW_FAKE_REVIEW_ACCEPT", "1")
        .output()
        .with_context(|| label.to_string())?;
    Ok(LiveCommandOutput {
        label: label.to_string(),
        success: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

fn is_freshness_refusal(text: &str) -> bool {
    let normalized = text
        .to_ascii_lowercase()
        .replace('-', "_")
        .replace("stale_base_refuses", "");
    normalized.contains("stale_external_review")
        || normalized.contains("stale external review")
        || normalized.contains("stale_base")
        || normalized.contains("freshness")
        || normalized.contains("stale_review")
        || normalized.contains("stale review")
        || normalized.contains("stale_test")
        || normalized.contains("stale test")
        || (normalized.contains("external review head") && normalized.contains("stale"))
        || (normalized.contains("external review head") && normalized.contains("mismatch"))
        || (normalized.contains("external_review")
            && normalized.contains("head")
            && normalized.contains("mismatch"))
}

fn main_advance_marker_path(task_id: &str, case_name: &str) -> PathBuf {
    PathBuf::from("fake-runner-markers")
        .join(format!("{}-{}", task_id, sanitize(case_name)))
        .join("main-advance.txt")
}

fn main_advance_commit_message(task_id: &str) -> String {
    format!("fake-run({task_id}): stale-base main advance")
}

fn required_er_sha(value: Option<&str>, field: &str, er_id: &str) -> Result<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .with_context(|| format!("fake external review {er_id} did not record {field}"))
}

fn fake_marker_files_at_head(repo: &Path) -> Result<Vec<String>> {
    let out = git_stdout(repo, &["ls-tree", "-r", "--name-only", "HEAD"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|path| path.starts_with("fake-runner-markers/"))
        .map(str::to_string)
        .collect())
}

fn proof_list(items: &[String]) -> String {
    if items.is_empty() {
        "-".to_string()
    } else {
        items.join(",")
    }
}

fn git_sha(repo: &Path, rev: &str) -> Result<String> {
    Ok(git_stdout(repo, &["rev-parse", rev])?.trim().to_string())
}

fn git_stdout(repo: &Path, args: &[&str]) -> Result<String> {
    let out = git_out(repo, args)?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn first_non_empty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

#[derive(Debug, Clone)]
struct LiveSnapshot {
    status: String,
    lifecycle: Option<String>,
    active_step: Option<String>,
    workspace_path: Option<String>,
    branch: Option<String>,
    blocked_reason: Option<String>,
    blocker_kind: Option<String>,
    blocked_reason_class: Option<String>,
    integration_attempts: Option<String>,
    integration_blocked_reason: Option<String>,
    integration_step: Option<String>,
    er: Option<(String, String, Option<String>, Option<String>)>,
}
impl LiveSnapshot {
    fn er_status(&self) -> Option<&str> {
        self.er.as_ref().map(|(_, s, _, _)| s.as_str())
    }

    fn refusal_evidence(&self) -> String {
        [
            self.blocked_reason.as_deref(),
            self.blocker_kind.as_deref(),
            self.blocked_reason_class.as_deref(),
            self.integration_blocked_reason.as_deref(),
            self.integration_step.as_deref(),
            self.integration_attempts.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
    }
}

#[derive(Debug, Clone)]
struct LiveWorkspaceProof {
    workspace_path: PathBuf,
    branch: String,
}

#[derive(Debug, Clone)]
struct LiveExternalReviewProof {
    display_id: String,
    status: String,
    verdict: Option<String>,
    runner: Option<String>,
    base_sha: Option<String>,
    head_sha: Option<String>,
    superseded_by: Option<String>,
}

#[derive(Debug, Clone)]
struct LiveCommandOutput {
    label: String,
    success: bool,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Clone)]
struct LiveRefusalProof {
    outputs: Vec<LiveCommandOutput>,
    combined_output: String,
}

impl LiveRefusalProof {
    fn from_outputs(outputs: Vec<LiveCommandOutput>) -> Self {
        let combined_output = outputs
            .iter()
            .map(|out| {
                format!(
                    "{} success={}\n{}\n{}",
                    out.label, out.success, out.stdout, out.stderr
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            outputs,
            combined_output,
        }
    }

    fn summary(&self) -> String {
        self.outputs
            .iter()
            .find(|out| !out.success)
            .map(|out| {
                first_non_empty_line(&out.stderr)
                    .or_else(|| first_non_empty_line(&out.stdout))
                    .unwrap_or_else(|| out.label.clone())
            })
            .unwrap_or_else(|| {
                first_non_empty_line(&self.combined_output)
                    .unwrap_or_else(|| "no command refusal; see task state evidence".to_string())
            })
    }
}

fn backup_live_db(db_path: &Path) -> Result<PathBuf> {
    let stores_dir = db_path
        .parent()
        .with_context(|| format!("db path has no parent: {}", db_path.display()))?;
    let backup_dir = stores_dir.join("backups");
    std::fs::create_dir_all(&backup_dir)
        .with_context(|| format!("creating backup dir {}", backup_dir.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let backup_path = backup_dir.join(format!(
        "db.sqlite.{}.{:09}.bak",
        now.as_secs(),
        now.subsec_nanos()
    ));
    std::fs::copy(db_path, &backup_path).with_context(|| {
        format!(
            "backing up live db {} to {}",
            db_path.display(),
            backup_path.display()
        )
    })?;
    Ok(backup_path)
}

fn create_live_task(case_name: &str) -> Result<String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let slug = format!("stores-test-live-{}-{ts}", sanitize(case_name));
    let out = Command::new(stores_bin_for_preflight()?)
        .args([
            "tasks",
            "add",
            "--invoker",
            "ai_with_human",
            "--activate",
            "--title",
            &format!("stores test live {case_name}"),
            "--slug",
            &slug,
            "--tier-hint",
            "T3",
            "--human-acceptance-policy",
            "delegated_by_policy",
            "--task-review-policy",
            live_task_review_policy(case_name),
            "--done-when",
            "live fake harness reaches expected state",
            "--scope-in",
            "stores test live fake runner rows only",
            "--scope-out",
            "product behavior changes",
        ])
        .env("STORES_LLM_OFF", "1")
        .output()
        .context("creating live stores test task")?;
    if !out.status.success() {
        bail!(
            "stores tasks add failed: status={} stderr={} stdout={}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        );
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let id = stdout
        .split_whitespace()
        .find(|t| t.starts_with('T'))
        .context("tasks add did not print task id")?
        .to_string();
    Ok(id)
}

fn live_task_review_policy(case_name: &str) -> &'static str {
    if is_stale_base_refuses_case(case_name) {
        // This scenario is specifically testing post-ER integration freshness.
        // An authoritative task-review policy parks integration at the review
        // substep before the freshness/landing path can demonstrate the issue.
        "none"
    } else {
        "authoritative"
    }
}

#[allow(dead_code)]
#[derive(Debug)]
struct LabArena {
    root: PathBuf,
    repo: PathBuf,
    stores_dir: PathBuf,
    db_path: PathBuf,
    approval_token_path: PathBuf,
}

#[allow(dead_code)]
fn create_lab_arena(base: &Path, run_id: &str) -> Result<LabArena> {
    if run_id.trim().is_empty() || run_id.contains('/') || run_id.contains('\\') {
        bail!("lab run_id must be a non-empty path segment");
    }
    let root = base.join(run_id);
    let repo = root.join("repo");
    let stores_dir = repo.join(".stores");
    std::fs::create_dir_all(stores_dir.join("runs"))?;

    let tasks_schema = bundled_schema("tasks")?;
    let external_reviews_schema = bundled_schema("external_reviews")?;
    let conn = Connection::open(stores_dir.join("db.sqlite"))?;
    conn.execute_batch(&ddl_for(&tasks_schema))?;
    conn.execute_batch(&ddl_for(&external_reviews_schema))?;
    crate::db::ensure_runs_view_if_tasks_exists(&conn)?;
    crate::handlers::framework_migrate::ensure_integration_singleton_index(&conn)?;
    drop(conn);

    git_ok(&repo, &["init", "-b", "main"])?;
    git_ok(&repo, &["config", "user.email", "fake@example.test"])?;
    git_ok(&repo, &["config", "user.name", "Fake Test"])?;
    std::fs::write(repo.join("README.md"), format!("stores lab {run_id}\n"))?;
    git_ok(&repo, &["add", "README.md"])?;
    git_ok(&repo, &["commit", "-m", "lab base"])?;

    let approval_token_path = root.join("approve.token");
    std::fs::write(&approval_token_path, format!("lab-token-{run_id}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = std::fs::metadata(&approval_token_path)?.permissions();
        p.set_mode(0o600);
        std::fs::set_permissions(&approval_token_path, p)?;
    }

    Ok(LabArena {
        root,
        repo,
        stores_dir: stores_dir.clone(),
        db_path: stores_dir.join("db.sqlite"),
        approval_token_path,
    })
}

struct Harness {
    _tmp: tempfile::TempDir,
    conn: Connection,
    tasks_schema: Schema,
    task_id: String,
    repo: PathBuf,
    workspace: PathBuf,
    config_path: PathBuf,
    codex_sentinel: PathBuf,
}

impl Harness {
    fn new(case_name: &str) -> Result<Self> {
        let tmp = tempfile::tempdir().context("create stores test tempdir")?;
        let stores_dir = tmp.path().join(".stores");
        std::fs::create_dir_all(stores_dir.join("runs"))?;
        let tasks_schema = bundled_schema("tasks")?;
        let external_reviews_schema = bundled_schema("external_reviews")?;
        let conn = Connection::open(stores_dir.join("db.sqlite"))?;
        conn.execute_batch(&ddl_for(&tasks_schema))?;
        conn.execute_batch(&ddl_for(&external_reviews_schema))?;
        crate::db::ensure_runs_view_if_tasks_exists(&conn)?;
        crate::handlers::framework_migrate::ensure_integration_singleton_index(&conn)?;

        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo)?;
        git_ok(&repo, &["init", "-b", "main"])?;
        git_ok(&repo, &["config", "user.email", "fake@example.test"])?;
        git_ok(&repo, &["config", "user.name", "Fake Test"])?;
        std::fs::write(repo.join("README.md"), "stores test base\n")?;
        git_ok(&repo, &["add", "README.md"])?;
        git_ok(&repo, &["commit", "-m", "base"])?;

        let task_id = format!("TTEST{}", std::process::id());
        let branch = format!("stores-test/{}-{}", sanitize(case_name), std::process::id());
        let workspace = tmp.path().join("worktree");
        git_ok(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                &branch,
                workspace.to_str().unwrap(),
            ],
        )?;
        std::fs::create_dir_all(workspace.join(".stores").join("runs"))?;
        insert_task(&conn, &task_id, workspace.to_str().unwrap(), &branch)?;

        let codex_sentinel = tmp.path().join("codex-was-invoked");
        let codex_cmd = tmp.path().join("codex-sentinel.sh");
        std::fs::write(
            &codex_cmd,
            format!(
                "#!/usr/bin/env bash\ntouch {}\nexit 99\n",
                codex_sentinel.display()
            ),
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&codex_cmd)?.permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&codex_cmd, p)?;
        }
        let config_path = tmp.path().join("config.yaml");
        std::fs::write(&config_path, format!("review:\n  runner: codex\n  timeout_secs: 5\ncodex:\n  command: {}\n  args: []\nfake_runner:\n  delay_ms: 0\n  scenario: all-pass\n  executor_mode: marker_file\n  fake_external_review: true\n", codex_cmd.display()))?;
        Ok(Self {
            _tmp: tmp,
            conn,
            tasks_schema,
            task_id,
            repo,
            workspace,
            config_path,
            codex_sentinel,
        })
    }

    fn progress(&self, label: &str) -> Result<()> {
        let (status, lifecycle): (String, Option<String>) = self.conn.query_row(
            "SELECT status,lifecycle FROM tasks WHERE display_id=?1",
            [&self.task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let er: Option<(String, String)> = self.conn.query_row(
            "SELECT display_id,status FROM external_reviews WHERE task_id=?1 ORDER BY id DESC LIMIT 1", [&self.task_id], |r| Ok((r.get(0)?, r.get(1)?))).ok();
        let runs: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM agent_runs WHERE display_id=?1",
                [&self.task_id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        println!(
            "progress {label}: task={} status={} lifecycle={:?} er={:?} agent_runs={}",
            self.task_id, status, lifecycle, er, runs
        );
        Ok(())
    }

    fn ensure_in_review_or_expected_failure(&self) -> Result<()> {
        Ok(())
    }

    fn run_external_review(&self) -> Result<()> {
        // Non-live temp harness fixture seeding. This in-memory/temp-db path is
        // intentionally not used as proof that the live substrate produced the
        // pending ER row; live/current/lab matrix paths must create rows through
        // real verbs/subscribers.
        self.conn.execute("INSERT INTO external_reviews (display_id,status,task_id,attempt,adapter,created_at,updated_at,created_by,updated_by) VALUES ('ERTEST','pending',?1,1,'external_review','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','stores-test','stores-test')", [&self.task_id])?;
        let agents = AgentsYaml {
            agents: vec![],
            deployment_specialist: None,
        };
        crate::flow::builtins::external_review::run(
            &json!({"display_id":"ERTEST"}),
            &DispatchCtx {
                conn: &self.conn,
                agents: &agents,
                config_path: &self.config_path,
                policies_hash: "",
            },
        )?;
        Ok(())
    }

    fn external_review_status(&self) -> Result<String> {
        self.conn
            .query_row(
                "SELECT status FROM external_reviews WHERE display_id='ERTEST'",
                [],
                |r| r.get(0),
            )
            .context("read external review status")
    }

    fn accept_and_integrate(&self) -> Result<()> {
        let cmd =
            clap::Command::new("accept").arg(clap::Arg::new("display_id").required(true).index(1));
        let m = cmd.get_matches_from(["accept", self.task_id.as_str()]);
        crate::handlers::transition::run(
            &self.tasks_schema,
            &self.conn,
            &m,
            crate::schema::actor::Actor::Human.into(),
            "accept",
        )?;
        let cmd = clap::Command::new("release-to-integration")
            .arg(clap::Arg::new("display_id").required(true).index(1));
        let m = cmd.get_matches_from(["release-to-integration", self.task_id.as_str()]);
        crate::handlers::transition::run(
            &self.tasks_schema,
            &self.conn,
            &m,
            crate::schema::actor::Actor::Framework.into(),
            "release-to-integration",
        )?;
        let mut args = serde_yaml::Mapping::new();
        args.insert(
            serde_yaml::Value::String("pre_land_check".into()),
            serde_yaml::Value::String("true".into()),
        );
        args.insert(
            serde_yaml::Value::String("allow_push".into()),
            serde_yaml::Value::Bool(false),
        );
        let agents = AgentsYaml {
            agents: vec![AgentEntry {
                name: "integrate".into(),
                subscribes_to: vec![],
                command: "builtin:integrate".into(),
                claim_window_secs: 300,
                retry_policy: RetryPolicy::default(),
                command_args: Some(args),
            }],
            deployment_specialist: None,
        };
        crate::flow::builtins::integrate::run(
            &json!({"display_id": self.task_id, "branch": self.branch()?, "workspace_path": self.workspace.to_string_lossy()}),
            &DispatchCtx {
                conn: &self.conn,
                agents: &agents,
                config_path: &self.config_path,
                policies_hash: "",
            },
        )?;
        Ok(())
    }

    fn branch(&self) -> Result<String> {
        self.conn
            .query_row(
                "SELECT branch FROM tasks WHERE display_id=?1",
                [&self.task_id],
                |r| r.get(0),
            )
            .context("read task branch")
    }

    fn assert_expectations(&self, expect: &CaseExpect) -> Result<()> {
        let (status, lifecycle): (String, Option<String>) = self.conn.query_row(
            "SELECT status,lifecycle FROM tasks WHERE display_id=?1",
            [&self.task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if status != expect.task_status {
            bail!("expected task_status={} got {}", expect.task_status, status);
        }
        if lifecycle.as_deref().unwrap_or("") != expect.lifecycle {
            bail!(
                "expected lifecycle={} got {:?}",
                expect.lifecycle,
                lifecycle
            );
        }
        let er = self.external_review_status()?;
        let want_er = expect
            .external_review
            .as_deref()
            .unwrap_or(&expect.external_review_status);
        if er != want_er {
            bail!("expected external_review_status={} got {}", want_er, er);
        }
        if let Some(visited) = expect.visited.as_deref() {
            let rows = self.transition_history_rows()?;
            match matrix::match_visited_subsequence(Some(visited), &rows) {
                matrix::VisitedMatch::Matched => {}
                matrix::VisitedMatch::Skipped => {}
                matrix::VisitedMatch::Missing {
                    expected_index,
                    expected,
                } => {
                    bail!(
                        "expected visited edge #{expected_index} not found in transition_history: {:?}",
                        expected
                    );
                }
            }
        }
        Ok(())
    }

    fn transition_history_rows(&self) -> Result<Vec<matrix::TransitionHistoryRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT from_status,to_status,lifecycle_from,lifecycle_to,active_step_from,active_step_to,integration_step_from,integration_step_to,verb,invoker \
             FROM transition_history WHERE store='tasks' AND display_id=?1 ORDER BY id",
        )?;
        let rows = stmt.query_map([&self.task_id], |r| {
            Ok(matrix::TransitionHistoryRow {
                from_status: r.get(0)?,
                to_status: r.get(1)?,
                lifecycle_from: r.get(2)?,
                lifecycle_to: r.get(3)?,
                active_step_from: r.get(4)?,
                active_step_to: r.get(5)?,
                integration_step_from: r.get(6)?,
                integration_step_to: r.get(7)?,
                verb: r.get(8)?,
                invoker: r.get(9)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("read transition_history rows")
    }

    fn assert_no_real_llm(&self) -> Result<()> {
        if self.codex_sentinel.exists() {
            bail!("real codex sentinel was invoked");
        }
        let non_fake: i64 = self.conn.query_row("SELECT COUNT(*) FROM agent_runs WHERE display_id=?1 AND COALESCE(harness_id,'') != 'fake'", [&self.task_id], |r| r.get(0))?;
        if non_fake != 0 {
            bail!("expected zero non-fake agent_runs, got {non_fake}");
        }
        Ok(())
    }

    fn summary(&self) -> Result<()> {
        self.progress("final")?;
        let main_tree = git_out(&self.repo, &["ls-tree", "-r", "--name-only", "main"])?;
        println!(
            "summary task={} marker_on_main={}",
            self.task_id,
            String::from_utf8_lossy(&main_tree.stdout).contains("fake-runner-markers/")
        );
        Ok(())
    }
}

fn bundled_schema(name: &str) -> Result<Schema> {
    let yaml = crate::cli::dynamic::BUNDLED_STORE_SCHEMAS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, y)| *y)
        .with_context(|| format!("bundled schema {name}"))?;
    Schema::from_yaml(yaml)
}

fn insert_task(conn: &Connection, task_id: &str, workspace: &str, branch: &str) -> Result<()> {
    // Non-live temp harness fixture seeding only. This helper must not be used
    // by current-repo/live proof paths, where task creation needs to go through
    // `stores tasks add` or the future lab/current test authority path.
    let contract = json!({
        "done_when": "fake harness reaches expectation",
        "scope_in": "fake harness only",
        "scope_out": "production work"
    });
    let plan = json!({"objective":"stores-test seed","phases":[{"name":"Fake execution","objective":"Exercise fake harness","tasks":[],"acceptance_criteria":[],"files":[],"dependencies":[]}]});
    conn.execute("INSERT INTO tasks (display_id,status,title,slug,tier_hint,created_at,updated_at,created_by,updated_by,contract,plan,plan_review_log,cycles,wrap_log,current_phase,current_cycle,workspace_path,branch,activation,lifecycle,active_step) VALUES (?1,'planning','stores test synthetic','stores-test','T3','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z','stores-test','stores-test',?2,?3,'[]','[]',NULL,0,0,?4,?5,'active','active','planning')", params![task_id, contract.to_string(), plan.to_string(), workspace, branch])?;
    Ok(())
}

fn git_ok(repo: &Path, args: &[&str]) -> Result<()> {
    let out = git_out(repo, args)?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}
fn git_out(repo: &Path, args: &[&str]) -> Result<std::process::Output> {
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .context("run git")
}
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

struct CwdRestore(PathBuf);
impl CwdRestore {
    fn pushd(path: &Path) -> Result<Self> {
        let old = std::env::current_dir().context("read current dir")?;
        std::env::set_current_dir(path).with_context(|| format!("chdir {}", path.display()))?;
        Ok(Self(old))
    }
}
impl Drop for CwdRestore {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.0);
    }
}

struct EnvRestore(Vec<(String, Option<std::ffi::OsString>)>);
impl EnvRestore {
    fn capture(keys: &[&str]) -> Self {
        Self(
            keys.iter()
                .map(|k| (k.to_string(), std::env::var_os(k)))
                .collect(),
        )
    }
    fn set(&mut self, key: &str, value: impl AsRef<std::ffi::OsStr>) {
        std::env::set_var(key, value);
    }
}
impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (k, v) in self.0.drain(..) {
            match v {
                Some(x) => std::env::set_var(k, x),
                None => std::env::remove_var(k),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yaml_case_with_attempts() {
        let raw = r#"cases:
  custom:
    stages:
      code_reviewer:
        attempts:
          - outcome: REVISE
          - outcome: PASS
    expect:
      task_status: in_review
      lifecycle: active
      external_review_status: tooling_held
      visited:
        - from_status: planning
          to_status: plan_review
          verb: submit-plan
"#;
        validate_case_manifest_yaml(raw).unwrap();
        let m: TestManifest = serde_yaml::from_str(raw).unwrap();
        let c = m.cases.get("custom").unwrap();
        assert_eq!(c.stages["code_reviewer"].attempts[0].outcome, "REVISE");
        assert_eq!(c.expect.task_status, "in_review");
        assert_eq!(
            c.expect.visited.as_ref().unwrap()[0].from_status.as_deref(),
            Some("planning")
        );
    }

    #[test]
    fn case_dsl_rejects_consequence_faking_outside_expect() {
        let raw = r#"cases:
  bad:
    stages:
      executor:
        outcome: PASS
        integration_result: refused
    expect:
      task_status: in_review
      external_review_status: passed
"#;
        let err = validate_case_manifest_yaml(raw).unwrap_err();
        assert!(
            err.to_string().contains("integration_result")
                && err.to_string().contains("$.cases.<case-id>.expect"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn case_dsl_rejects_nested_expect_under_stage() {
        let raw = r#"cases:
  bad:
    stages:
      executor:
        outcome: PASS
        expect:
          integration_result: refused
    expect:
      task_status: in_review
      external_review_status: passed
"#;
        let err = validate_case_manifest_yaml(raw).unwrap_err();
        assert!(
            err.to_string().contains("integration_result")
                && err.to_string().contains("$.cases.<case-id>.expect"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn case_dsl_allows_consequence_expectations_under_case_level_expect() {
        let raw = r#"cases:
  ok:
    stages:
      executor:
        outcome: PASS
    expect:
      task_status: in_review
      external_review_status: passed
      integration_result: refused
      blocked_reason: stale_external_review
"#;
        validate_case_manifest_yaml(raw).unwrap();
    }

    #[test]
    fn case_file_path_is_canonicalized_for_child_runner_cwd_changes() {
        let _cwd_guard = crate::paths::test_cwd_lock()
            .lock()
            .expect("cwd lock poisoned");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.yaml");
        std::fs::write(
            &path,
            "cases:\n  custom:\n    stages:\n      external_review:\n        outcome: TOOLING_FAILURE\n    expect:\n      task_status: in_review\n      lifecycle: active\n      external_review_status: tooling_held\n",
        )
        .unwrap();
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let opts = TestRunOpts {
            case_name: Some("custom".to_string()),
            case_file: Some(PathBuf::from("case.yaml")),
            delay_ms: None,
            watch: false,
            live: false,
        };
        let (_name, _case, fake_path) = load_case(&opts).unwrap();
        std::env::set_current_dir(old).unwrap();
        assert!(fake_path.unwrap().is_absolute());
    }

    #[test]
    fn stale_base_refuses_preset_loads_as_all_pass_fake_live_case() {
        let opts = TestRunOpts {
            case_name: Some("stale-base-refuses".to_string()),
            case_file: None,
            delay_ms: None,
            watch: false,
            live: true,
        };
        let (name, case, fake_path) = load_case(&opts).unwrap();
        assert_eq!(name, "stale-base-refuses");
        assert!(is_stale_base_refuses_case(&name));
        assert_eq!(preset_scenario(&name), "all-pass");
        assert_eq!(case.expect.external_review_status, "passed");
        assert_eq!(case.executor_mode, "marker_file");
        assert!(fake_path
            .unwrap()
            .ends_with("stores-test-stale-base-refuses-case.yaml"));
    }

    #[test]
    fn freshness_refusal_classifier_covers_current_canonical_spellings() {
        for text in [
            "stale_external_review: review head no longer matches",
            "stale external review head: rerun required",
            "freshness check refused integration",
            "stale_base current_main differs",
            "stale_review: affected scope requires rerun",
            "integration_blocked_reason=stale_test",
            "external review head mismatch after rebase",
        ] {
            assert!(is_freshness_refusal(text), "{text}");
        }
        assert!(!is_freshness_refusal(
            "runner crashed without stale evidence"
        ));
        assert!(!is_freshness_refusal(
            "stores test stale-base-refuses timed out waiting for integration refusal"
        ));
    }

    #[test]
    fn stale_base_live_task_review_policy_does_not_park_before_freshness_path() {
        assert_eq!(live_task_review_policy("stale-base-refuses"), "none");
        assert_eq!(live_task_review_policy("happy-path"), "authoritative");
        assert_eq!(live_task_review_policy("t3-failed-er"), "authoritative");
    }

    #[test]
    fn stale_base_main_marker_helpers_are_fenced_and_specific() {
        assert_eq!(
            main_advance_marker_path("T205", "stale-base-refuses"),
            PathBuf::from("fake-runner-markers/T205-stale-base-refuses/main-advance.txt")
        );
        assert_eq!(
            main_advance_commit_message("T205"),
            "fake-run(T205): stale-base main advance"
        );
        assert_eq!(
            required_er_sha(Some(" abc123 "), "head_sha", "ER1").unwrap(),
            "abc123"
        );
        assert!(required_er_sha(None, "base_sha", "ER1").is_err());
    }

    #[test]
    fn proof_list_formats_marker_paths_without_guessing() {
        assert_eq!(proof_list(&[]), "-");
        assert_eq!(
            proof_list(&[
                "fake-runner-markers/T205-p1-c1-a1.txt".to_string(),
                "fake-runner-markers/T205-p1-c2-a2.txt".to_string(),
            ]),
            "fake-runner-markers/T205-p1-c1-a1.txt,fake-runner-markers/T205-p1-c2-a2.txt"
        );
    }

    #[test]
    fn live_refusal_summary_prefers_first_failed_command_line() {
        let proof = LiveRefusalProof::from_outputs(vec![
            LiveCommandOutput {
                label: "accept".to_string(),
                success: true,
                stdout: "accepted\n".to_string(),
                stderr: String::new(),
            },
            LiveCommandOutput {
                label: "integration".to_string(),
                success: false,
                stdout: String::new(),
                stderr: "stale_external_review: rerun review\nmore detail".to_string(),
            },
        ]);
        assert_eq!(proof.summary(), "stale_external_review: rerun review");
        assert!(is_freshness_refusal(&proof.combined_output));
    }

    #[test]
    fn env_restore_restores_llm_off_under_runner_env_lock() {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let old = std::env::var_os("STORES_LLM_OFF");
        let mut restore = EnvRestore::capture(&["STORES_LLM_OFF"]);
        restore.set("STORES_LLM_OFF", "1");
        drop(restore);
        assert_eq!(std::env::var_os("STORES_LLM_OFF"), old);
    }

    #[test]
    fn live_db_backup_helper_copies_db_before_live_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let stores_dir = dir.path().join(".stores");
        std::fs::create_dir_all(&stores_dir).unwrap();
        let db_path = stores_dir.join("db.sqlite");
        std::fs::write(&db_path, b"before-create").unwrap();

        let backup = backup_live_db(&db_path).unwrap();
        std::fs::write(&db_path, b"after-create").unwrap();

        assert_eq!(backup.parent().unwrap(), stores_dir.join("backups"));
        assert!(backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("db.sqlite."));
        assert_eq!(std::fs::read(&backup).unwrap(), b"before-create");
        assert_eq!(std::fs::read(&db_path).unwrap(), b"after-create");
    }

    #[test]
    fn lab_arena_foundation_creates_real_git_repo_and_stores_db() {
        let dir = tempfile::tempdir().unwrap();
        let lab = create_lab_arena(dir.path(), "run-phase0").unwrap();

        assert!(lab.root.ends_with("run-phase0"));
        assert_eq!(lab.stores_dir, lab.repo.join(".stores"));
        assert!(lab.db_path.exists());
        assert!(lab.approval_token_path.exists());
        let head = git_out(&lab.repo, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main");
        let conn = Connection::open(&lab.db_path).unwrap();
        let tasks_table: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tasks_table, 1);
    }

    #[test]
    fn test_authority_provenance_accepts_stores_test_task_markers() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (display_id TEXT PRIMARY KEY, title TEXT, slug TEXT, contract TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,title,slug,contract) VALUES (?1,?2,?3,?4)",
            params![
                "TTEST",
                "stores test live happy-path",
                "stores-test-live-happy-path-123",
                json!({"scope_in":"stores test live fake runner rows only"}).to_string()
            ],
        )
        .unwrap();

        ensure_stores_test_task_provenance(&conn, "TTEST").unwrap();
    }

    #[test]
    fn test_authority_provenance_refuses_prod_shaped_task() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE tasks (display_id TEXT PRIMARY KEY, title TEXT, slug TEXT, contract TEXT);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (display_id,title,slug,contract) VALUES (?1,?2,?3,?4)",
            params![
                "TPROD",
                "real product work",
                "real-product-work",
                json!({"scope_in":"production work"}).to_string()
            ],
        )
        .unwrap();

        let err = ensure_stores_test_task_provenance(&conn, "TPROD").unwrap_err();
        assert!(
            err.to_string().contains("not stores-test owned"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn fake_mode_preflight_fails_when_fake_runner_binary_missing() {
        with_harness_env(|| {
            let missing = tempfile::tempdir()
                .unwrap()
                .path()
                .join("missing-stores-fake-agent");
            std::env::set_var("STORES_FAKE_AGENT_BIN", &missing);

            let err = preflight_fake_mode().unwrap_err();
            assert!(
                err.to_string().contains("stores-fake-agent not found"),
                "unexpected error: {err}"
            );
        });
    }

    #[test]
    fn live_mode_expectation_match_requires_task_lifecycle_and_er_status() {
        let h = LiveHarness {
            case_name: "unit".to_string(),
            task_id: "TUNIT".to_string(),
            db_path: PathBuf::from("/tmp/no-db"),
            root: PathBuf::from("/tmp"),
        };
        let expect = CaseExpect::default();
        let good = LiveSnapshot {
            status: "integrated".to_string(),
            lifecycle: Some("done".to_string()),
            active_step: None,
            workspace_path: None,
            branch: None,
            blocked_reason: None,
            blocker_kind: None,
            blocked_reason_class: None,
            integration_attempts: None,
            integration_blocked_reason: None,
            integration_step: None,
            er: Some((
                "ERUNIT".to_string(),
                "passed".to_string(),
                Some("PASS".to_string()),
                Some("fake".to_string()),
            )),
        };
        assert!(h.matches_expect(&expect, &good));
        let bad_er = LiveSnapshot {
            er: Some((
                "ERUNIT".to_string(),
                "running".to_string(),
                None,
                Some("fake".to_string()),
            )),
            ..good
        };
        assert!(!h.matches_expect(&expect, &bad_er));
    }

    #[test]
    fn stores_test_run_restores_fake_env_after_return() {
        with_harness_env(|| {
            let fake_bin = stores_fake_agent_bin_for_unit_tests();
            let expected: Vec<(&str, Option<std::ffi::OsString>)> = vec![
                ("STORES_LLM_OFF", Some("preexisting-llm-off".into())),
                ("STORES_FAKE_AGENT_BIN", Some(fake_bin.into_os_string())),
                ("STORES_FAKE_SCENARIO", Some("preexisting-scenario".into())),
                ("STORES_FAKE_DELAY_MS", Some("12345".into())),
                ("STORES_FAKE_EXECUTOR_MODE", Some("preexisting-mode".into())),
                ("STORES_FAKE_CASE_FILE", None),
                ("STORES_FAKE_CASE_NAME", Some("preexisting-case".into())),
                (
                    "STORES_ALLOW_FAKE_REVIEW_ACCEPT",
                    Some("preexisting-allow".into()),
                ),
            ];
            for (key, value) in &expected {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }

            run(TestRunOpts {
                case_name: Some("happy-path".to_string()),
                case_file: None,
                delay_ms: Some(0),
                watch: false,
                live: false,
            })
            .expect("happy-path harness run should return successfully");

            for (key, value) in expected {
                assert_eq!(
                    std::env::var_os(key),
                    value,
                    "{key} leaked after stores test run"
                );
            }
        });
    }

    #[test]
    fn stores_test_run_happy_path_reaches_integrated_done() {
        with_harness_env(|| {
            run(TestRunOpts {
                case_name: Some("happy-path".to_string()),
                case_file: None,
                delay_ms: Some(0),
                watch: false,
                live: false,
            })
            .expect("happy-path harness run should reach integrated/done");
        });
    }

    #[test]
    fn stores_test_run_failed_external_review_holds_in_review() {
        with_harness_env(|| {
            run(TestRunOpts {
                case_name: Some("t3-failed-er".to_string()),
                case_file: None,
                delay_ms: Some(0),
                watch: false,
                live: false,
            })
            .expect("failed external-review harness run should match held expectation");
        });
    }

    #[test]
    fn stores_test_run_case_file_executes_configured_expectations() {
        with_harness_env(|| {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("case.yaml");
            std::fs::write(
                &path,
                r#"cases:
  yaml-happy:
    tier: T3
    delay_ms: 0
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
"#,
            )
            .unwrap();
            run(TestRunOpts {
                case_name: Some("yaml-happy".to_string()),
                case_file: Some(path),
                delay_ms: Some(0),
                watch: false,
                live: false,
            })
            .expect("YAML case-file harness run should execute configured expectations");
        });
    }

    fn with_harness_env<T>(f: impl FnOnce() -> T) -> T {
        let _env_guard = crate::runner::test_support::ENV_LOCK
            .lock()
            .expect("runner env lock poisoned");
        let _cwd_guard = crate::paths::test_cwd_lock()
            .lock()
            .expect("cwd lock poisoned");
        let tasks_status_before = git_tasks_status();
        let tmp = tempfile::tempdir().expect("test tempdir");
        let fake_bin = stores_fake_agent_bin_for_unit_tests();
        let stores_bin = stores_bin_for_unit_tests();
        let daemon_bin = tmp.path().join("private-daemon").join("stores");
        let mut restore = EnvRestore::capture(&[
            "STORES_LLM_OFF",
            "STORES_FAKE_AGENT_BIN",
            "STORES_FAKE_SCENARIO",
            "STORES_FAKE_DELAY_MS",
            "STORES_FAKE_EXECUTOR_MODE",
            "STORES_FAKE_CASE_FILE",
            "STORES_FAKE_CASE_NAME",
            "STORES_ALLOW_FAKE_REVIEW_ACCEPT",
            "STORES_DAEMON_BIN_PATH",
            "STORES_TEST_STORES_BIN",
        ]);
        restore.set("STORES_FAKE_AGENT_BIN", fake_bin);
        restore.set("STORES_TEST_STORES_BIN", stores_bin);
        restore.set("STORES_DAEMON_BIN_PATH", daemon_bin);
        let out = f();
        drop(restore);
        assert_eq!(
            git_tasks_status(),
            tasks_status_before,
            "stores test harness must not dirty repo task projections"
        );
        out
    }

    fn stores_fake_agent_bin_for_unit_tests() -> PathBuf {
        if let Some(path) = option_env!("CARGO_BIN_EXE_stores-fake-agent") {
            return PathBuf::from(path);
        }
        target_debug_bin("stores-fake-agent")
    }

    fn stores_bin_for_unit_tests() -> PathBuf {
        if let Some(path) = option_env!("CARGO_BIN_EXE_stores") {
            return PathBuf::from(path);
        }
        target_debug_bin("stores")
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

    fn git_tasks_status() -> String {
        let out = Command::new("git")
            .args(["status", "--short", "--", "tasks"])
            .output()
            .expect("git status -- tasks");
        assert!(
            out.status.success(),
            "git status failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}
