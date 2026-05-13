use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
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
}

impl Default for CaseExpect {
    fn default() -> Self {
        Self {
            task_status: default_expect_task_status(),
            lifecycle: default_expect_lifecycle(),
            external_review_status: default_expect_external_review_status(),
            external_review: None,
            no_real_llm: true,
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
        let task_head_x = git_sha(&workspace.workspace_path, "HEAD")?;
        println!(
            "[executor] task head X={} marker_hint=fake-runner-markers/{}/",
            task_head_x, self.task_id
        );
        println!(
            "[external-review] {} runner={} status={} verdict={} base={} head={} superseded_by={}",
            er.display_id,
            er.runner.as_deref().unwrap_or("-"),
            er.status,
            er.verdict.as_deref().unwrap_or("-"),
            er.base_sha.as_deref().unwrap_or("-"),
            er.head_sha.as_deref().unwrap_or("-"),
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
                bail!(
                    "expected stale/freshness refusal; command_output={} state_evidence={} snapshot={:?}",
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
        let er = conn
            .query_row(
                "SELECT display_id,status,verdict,runner,base_sha,head_sha,superseded_by \
             FROM external_reviews WHERE task_id=?1 ORDER BY id DESC LIMIT 1",
                [&self.task_id],
                |r| {
                    Ok(LiveExternalReviewProof {
                        display_id: r.get(0)?,
                        status: r.get(1)?,
                        verdict: r.get(2)?,
                        runner: r.get(3)?,
                        base_sha: r.get(4)?,
                        head_sha: r.get(5)?,
                        superseded_by: r.get(6)?,
                    })
                },
            )
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
        let accept = run_live_stores_cmd_output(
            &self.root,
            ["tasks", "accept", &self.task_id, "--invoker", "human"],
            "stores tasks accept",
        )?;
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
        outputs.push(daemon);
        Ok(LiveRefusalProof::from_outputs(outputs))
    }

    fn conn(&self) -> Result<Connection> {
        let mut last_err = None;
        for _ in 0..20 {
            match crate::db::open(&self.db_path) {
                Ok(conn) => return Ok(conn),
                Err(err) if err.to_string().contains("database is locked") => {
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
        ) = conn.query_row(
            "SELECT status,lifecycle,active_step,workspace_path,branch,blocked_reason,blocker_kind,blocked_reason_class,integration_attempts FROM tasks WHERE display_id=?1",
            [&self.task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)),
        )?;
        let er: Option<(String, String, Option<String>, Option<String>)> = conn.query_row(
            "SELECT display_id,status,verdict,runner FROM external_reviews WHERE task_id=?1 ORDER BY id DESC LIMIT 1",
            [&self.task_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).ok();
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
        run_live_stores_cmd(
            &self.root,
            ["tasks", "accept", &self.task_id, "--invoker", "human"],
            "stores tasks accept",
        )?;
        run_live_stores_cmd(
            &self.root,
            ["tasks", "enqueue-integration", &self.task_id],
            "stores tasks enqueue-integration",
        )?;
        Ok(())
    }

    fn isolate_live_case(&self) -> Result<()> {
        self.freeze_latest_tooling_held_review_retry()?;
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

    fn freeze_latest_tooling_held_review_retry(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE external_reviews \
             SET next_retry_at='9999-12-31T23:59:59Z', updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now') \
             WHERE id = (SELECT id FROM external_reviews WHERE task_id=?1 ORDER BY id DESC LIMIT 1) \
               AND status='tooling_held'",
            [&self.task_id],
        )
        .with_context(|| format!("freezing latest tooling-held review retry for {}", self.task_id))?;
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
    let normalized = text.to_ascii_lowercase().replace('-', "_");
    normalized.contains("stale_external_review")
        || normalized.contains("stale external review")
        || normalized.contains("stale_base")
        || normalized.contains("freshness")
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
            "authoritative",
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
        Ok(())
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
"#;
        let m: TestManifest = serde_yaml::from_str(raw).unwrap();
        let c = m.cases.get("custom").unwrap();
        assert_eq!(c.stages["code_reviewer"].attempts[0].outcome, "REVISE");
        assert_eq!(c.expect.task_status, "in_review");
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
            "external review head mismatch after rebase",
        ] {
            assert!(is_freshness_refusal(text), "{text}");
        }
        assert!(!is_freshness_refusal(
            "runner crashed without stale evidence"
        ));
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
    fn live_case_isolation_freezes_tooling_retry_without_changing_status() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("db.sqlite");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE external_reviews (id INTEGER PRIMARY KEY, display_id TEXT, status TEXT, task_id TEXT, next_retry_at TEXT, updated_at TEXT);\n\
             INSERT INTO external_reviews (display_id,status,task_id,next_retry_at,updated_at) VALUES ('ERUNIT','tooling_held','TUNIT','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');",
        )
        .unwrap();
        drop(conn);
        let h = LiveHarness {
            case_name: "unit".to_string(),
            task_id: "TUNIT".to_string(),
            db_path,
            root: dir.path().to_path_buf(),
        };

        h.freeze_latest_tooling_held_review_retry().unwrap();

        let conn = h.conn().unwrap();
        let row: (String, String) = conn
            .query_row(
                "SELECT status,next_retry_at FROM external_reviews WHERE display_id='ERUNIT'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "tooling_held".to_string(),
                "9999-12-31T23:59:59Z".to_string()
            )
        );
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
