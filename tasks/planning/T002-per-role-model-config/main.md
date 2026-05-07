# T002: per-role model configuration for substrate runner

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T06:07:04Z
- **Last Updated:** 2026-05-03T06:13:07Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —
- **Branch:** feat/T002-per-role-model-config

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** Runner code (HashMap&lt;role, model&gt; instead of single Option&lt;String&gt;); drive handler config-loading + per-role spawn; CLI flag plumbing on tasks drive; runner.yaml schema (sketched in scope_in but planner can refine); tests covering all four behaviors above; drive output log-line update for model-name visibility; small docs note pointing at runner.yaml.
- **Out:** Model-cost telemetry / spend tracking; AI-decides-which-model-per-task logic; MCP server (deferred per user); replacing claude CLI default model globally; rate-limit-aware fallbacks; per-task-row model overrides (project-level config is sufficient this iteration).

### Done When
(1) &#x60;.stores/runner.yaml&#x60; (or equivalent — name TBD by planner) accepts a &#x60;claude_code.models&#x60; map keyed by role: planner, plan_reviewer, executor, code_reviewer, wrap. Missing role falls back to claude CLI&#x27;s default (current behavior).
(2) ClaudeCodeRunner switches from &#x60;model: Option&lt;String&gt;&#x60; to a HashMap&lt;role, Option&lt;String&gt;&gt;; spawn picks per role. Backward compatibility for &#x60;with_model()&#x60; retained (sets all roles to that model).
(3) &#x60;tasks drive&#x60; reads runner.yaml at start (if present). CLI overrides per role: &#x60;--model-&lt;role&gt;&#x3D;&lt;m&gt;&#x60; (planner | plan_reviewer | executor | code_reviewer | wrap). &#x60;--testing&#x60; continues to force haiku globally (existing behavior preserved).
(4) Default config committed to repo: planner&#x3D;opus, plan_reviewer&#x3D;opus, executor&#x3D;sonnet, code_reviewer&#x3D;opus, wrap&#x3D;opus.
(5) Tests: per-role passthrough (mock runner captures spawn args + verifies --model&#x3D;&lt;role&#x27;s model&gt;); config fallback to default when role missing; CLI override beats config; --testing overrides all.
(6) drive output line gains the model used per spawn, e.g. &#x60;[T###] phase X cycle Y: spawning planner via claude-code runner (model&#x3D;opus)...&#x60; — closes the model-invisibility observability gap.
(7) cargo build + cargo test green on this branch with the &#x60;runner-claude-code&#x60; feature.

### Assumptions
Aliases &quot;opus&quot; / &quot;sonnet&quot; / &quot;haiku&quot; continue to resolve to current claude models in the installed claude CLI. The user&#x27;s &#x60;claude --model&#x3D;&lt;alias&gt;&#x60; works as documented. Running this task itself uses claude CLI&#x27;s existing default model since the new flags are what this task builds (recursion footnote — the task that ships per-role models cannot use them on its own drive).

### Phases

#### Phase 1: Phase 1: Runner refactor — per-role model map
- **Objective:** Convert ClaudeCodeRunner from a single Option&lt;String&gt; model to a HashMap&lt;RoleKey, String&gt;; spawn() looks up the role; backward-compatible with_model() sets all 5 roles to the same value; add a public accessor so drive can log the model per role.
- **Tasks:**
  - Task 1.1: In src/runner/claude_code.rs, replace the &#x60;model: Option&lt;String&gt;&#x60; field with &#x60;models: HashMap&lt;String, String&gt;&#x60; keyed by normalized role name (planner, plan-reviewer, executor, code-reviewer, wrap).
  - Task 1.2: Add &#x60;ClaudeCodeRunner::with_models(map: HashMap&lt;String, String&gt;) -&gt; Self&#x60; constructor.
  - Task 1.3: Rewrite &#x60;with_model(model)&#x60; to populate all 5 known role keys with the given value (backward-compat per Done When #2); keep the same signature and visibility.
  - Task 1.4: In &#x60;spawn()&#x60;, replace &#x60;if let Some(m) &#x3D; &amp;self.model&#x60; with &#x60;if let Some(m) &#x3D; self.models.get(role)&#x60; (after the normalized role string is computed; role param is already passed in).
  - Task 1.5: Add &#x60;pub fn model_for(&amp;self, role: &amp;str) -&gt; Option&lt;&amp;str&gt;&#x60; so drive can read the chosen model for the spawn-line log.
  - Task 1.6: Extend the &#x60;Runner&#x60; trait with &#x60;fn model_for(&amp;self, _role: &amp;str) -&gt; Option&lt;&amp;str&gt; { None }&#x60; (default impl returns None for runners without per-role models — mock and sap inherit it without change).
  - Task 1.7: Override &#x60;model_for&#x60; in &#x60;ClaudeCodeRunner&#x60; to return from the HashMap; mock and sap take the default.
- **Acceptance Criteria:**
  - [ ] AC1.1: cargo build --features runner-claude-code succeeds.
  - [ ] AC1.2: cargo test --features runner-claude-code -p stores runner:: passes including the existing claude_code shim tests (no regression).
  - [ ] AC1.3: with_model(&quot;haiku&quot;) populates all 5 role keys (verified by a unit test asserting models.len() &#x3D;&#x3D; 5 and every value &#x3D;&#x3D; &quot;haiku&quot;).
  - [ ] AC1.4: model_for(&quot;planner&quot;) returns Some(model) when the map contains an entry; None when absent.
  - [ ] AC1.5: A new shim-backed test asserts spawn passes &#x60;--model&#x3D;&lt;expected&gt;&#x60; for the role given to spawn() and omits &#x60;--model&#x3D;&#x60; when the role has no entry.
- **Files:** `src/runner/mod.rs`, `src/runner/claude_code.rs`, `src/runner/mock.rs`
#### Phase 2: Phase 2: runner.yaml schema + loader
- **Objective:** Define a small RunnerConfig type, a YAML loader that reads &#x60;.stores/runner.yaml&#x60;, and a built-in default (planner&#x3D;opus, plan_reviewer&#x3D;opus, executor&#x3D;sonnet, code_reviewer&#x3D;opus, wrap&#x3D;opus). Loader returns the merged map (file overrides defaults; missing roles fall back to defaults; if the file is absent, defaults apply).
- **Tasks:**
  - Task 2.1: Create src/runner/config.rs with &#x60;pub struct RunnerConfig { pub claude_code: ClaudeCodeConfig }&#x60; and &#x60;pub struct ClaudeCodeConfig { pub models: HashMap&lt;String, String&gt; }&#x60;. Use serde_yaml (already in Cargo.toml — verify).
  - Task 2.2: Add &#x60;pub fn default_models() -&gt; HashMap&lt;String, String&gt;&#x60; returning the 5-role default (keys: planner, plan_reviewer, executor, code_reviewer, wrap → opus, opus, sonnet, opus, opus).
  - Task 2.3: Add &#x60;pub fn load_models_from_repo(repo_root: &amp;Path) -&gt; Result&lt;HashMap&lt;String, String&gt;&gt;&#x60;: reads &#x60;&lt;repo_root&gt;/.stores/runner.yaml&#x60; if present, parses it, merges over defaults; returns defaults alone when the file is absent. Underscore role keys in the yaml are normalized to hyphenated form to match runner role keys.
  - Task 2.4: Commit &#x60;.stores/runner.yaml&#x60; with the documented defaults. Update &#x60;.gitignore&#x60; to ignore &#x60;.stores/db.sqlite&#x60;, &#x60;.stores/runs/&#x60;, and &#x60;.stores/runs.lock&#x60; instead of the entire &#x60;.stores/&#x60; directory so runner.yaml can be tracked.
  - Task 2.5: Add &#x60;pub mod config;&#x60; to src/runner/mod.rs.
- **Acceptance Criteria:**
  - [ ] AC2.1: cargo test runner::config:: passes (≥3 tests: default_models has all 5 keys; load_from_repo with no file returns defaults; load_from_repo with file overrides applies file values and falls back to default for missing keys).
  - [ ] AC2.2: &#x60;.stores/runner.yaml&#x60; is tracked by git (verified by &#x60;git ls-files .stores/runner.yaml&#x60; returning the path).
  - [ ] AC2.3: &#x60;.stores/db.sqlite&#x60; and &#x60;.stores/runs/&#x60; remain ignored (verified by &#x60;git check-ignore .stores/db.sqlite .stores/runs/transcript.jsonl&#x60;).
  - [ ] AC2.4: Roundtrip test: writing default_models() to YAML, parsing it back, yields the same map.
- **Files:** `src/runner/config.rs`, `src/runner/mod.rs`, `.stores/runner.yaml`, `.gitignore`
- **Dependencies:** Phase 1 complete: ClaudeCodeRunner accepts a HashMap
#### Phase 3: Phase 3: drive — config load + CLI overrides + --testing semantics + spawn-line log
- **Objective:** DriveArgs grows per-role override fields. build_runner loads runner.yaml, applies CLI overrides per role, and constructs ClaudeCodeRunner with the merged map. --testing continues to override all roles to haiku (highest precedence). drive&#x27;s spawn-line log includes the model.
- **Tasks:**
  - Task 3.1: In src/handlers/drive.rs DriveArgs, add &#x60;pub model_planner: Option&lt;String&gt;&#x60;, &#x60;pub model_plan_reviewer: Option&lt;String&gt;&#x60;, &#x60;pub model_executor: Option&lt;String&gt;&#x60;, &#x60;pub model_code_reviewer: Option&lt;String&gt;&#x60;, &#x60;pub model_wrap: Option&lt;String&gt;&#x60; (all gated on feature &#x60;runner-claude-code&#x60;).
  - Task 3.2: In build_runner, when &#x60;claude_code&#x60; is true: (a) call &#x60;config::load_models_from_repo(&amp;std::env::current_dir()?)&#x60; to get the base map; (b) overlay CLI overrides for any role flag that is Some; (c) if &#x60;args.testing&#x60; is true, overwrite the entire map to haiku (preserving Done When #3 — --testing forces haiku globally and beats per-role flags); (d) construct ClaudeCodeRunner via &#x60;with_models(map)&#x60;.
  - Task 3.3: In src/cli/dynamic.rs build_drive_cmd (under &#x60;#[cfg(feature &#x3D; &quot;runner-claude-code&quot;)]&#x60;), add five Args: &#x60;--model-planner&#x60;, &#x60;--model-plan-reviewer&#x60;, &#x60;--model-executor&#x60;, &#x60;--model-code-reviewer&#x60;, &#x60;--model-wrap&#x60;, each &#x60;value_parser(clap::value_parser!(String))&#x60;, with help text noting they override runner.yaml.
  - Task 3.4: In src/cli/dispatch.rs drive subcommand handler, read the five flags via &#x60;sub.get_one::&lt;String&gt;(...)&#x60; and populate the new DriveArgs fields.
  - Task 3.5: In drive_loop, change the pre-spawn log line to: &#x60;[{display_id}] phase {phase} cycle {cycle}: spawning {agent_role} via {runner_name} runner (model&#x3D;{model})... (may take 30-90s)&#x60; where model is &#x60;runner.model_for(&amp;agent_name_normalized).unwrap_or(&quot;&lt;default&gt;&quot;)&#x60;.
  - Task 3.6: Update the existing two test fixtures in src/handlers/drive.rs that construct DriveArgs literally (lines 1268, 1317) to include the new fields with &#x60;None&#x60; defaults.
- **Acceptance Criteria:**
  - [ ] AC3.1: cargo build --features runner-claude-code succeeds.
  - [ ] AC3.2: &#x60;stores tasks drive --help&#x60; lists --model-planner, --model-plan-reviewer, --model-executor, --model-code-reviewer, --model-wrap (verified by capturing &#x60;--help&#x60; output in a CLI smoke test).
  - [ ] AC3.3: With the default runner.yaml present and no CLI override, build_runner produces a ClaudeCodeRunner whose model_for returns: planner&#x3D;opus, plan_reviewer&#x3D;opus, executor&#x3D;sonnet, code_reviewer&#x3D;opus, wrap&#x3D;opus (verified via a unit test in drive.rs against a tempdir staged with the default runner.yaml).
  - [ ] AC3.4: --model-executor&#x3D;haiku CLI override beats runner.yaml&#x27;s executor&#x3D;sonnet (executor&#x3D;haiku, others&#x3D;defaults).
  - [ ] AC3.5: --testing forces all 5 roles to haiku, even when --model-executor&#x3D;sonnet is set on the same invocation (the existing --testing semantics are preserved per Done When #3).
  - [ ] AC3.6: drive spawn log line contains &#x60;(model&#x3D;&lt;m&gt;)&#x60; substring (verified by capturing stderr in an existing drive_loop integration test that uses the mock runner — mock returns &#x60;Some(&quot;&lt;default&gt;&quot;)&#x60; from model_for since it has no map; assert that the literal &#x60;(model&#x3D;&#x60; substring appears).
- **Files:** `src/handlers/drive.rs`, `src/cli/dynamic.rs`, `src/cli/dispatch.rs`
- **Dependencies:** Phase 2 complete: config loader exists, Phase 1 complete: ClaudeCodeRunner.with_models / model_for available
#### Phase 4: Phase 4: End-to-end shim test + docs note
- **Objective:** Add a single end-to-end test that drives a real ClaudeCodeRunner via a shim binary that records its argv, asserting the per-role --model arg is correct across all five roles. Add a one-paragraph docs note pointing at .stores/runner.yaml.
- **Tasks:**
  - Task 4.1: Add &#x60;argv_recorder&#x60; shim to the SHIM_DIR in src/runner/claude_code.rs tests: a /bin/sh script that writes its argv to $ARGV_OUT (env var) and emits a stream-json result event with role-keyed envelope text.
  - Task 4.2: Add test &#x60;per_role_model_passes_through_to_argv&#x60; that constructs a ClaudeCodeRunner with a 5-role map (each role → distinct model alias), calls spawn(role) for each role with $ARGV_OUT pointed at a per-role tempfile, and asserts the recorded argv contains &#x60;--model&#x3D;&lt;expected&gt;&#x60; for that role.
  - Task 4.3: Add test &#x60;unknown_role_omits_model_flag&#x60; asserting that spawning a role not present in the HashMap produces an argv with no &#x60;--model&#x3D;&#x60; token.
  - Task 4.4: Append a short section to README.md (or the existing nearest doc — verify) titled &quot;Per-role model configuration&quot; pointing at &#x60;.stores/runner.yaml&#x60; and the &#x60;--model-&lt;role&gt;&#x60; flags. ≤15 lines, no new doc files.
- **Acceptance Criteria:**
  - [ ] AC4.1: cargo test --features runner-claude-code per_role_model_passes_through_to_argv passes.
  - [ ] AC4.2: cargo test --features runner-claude-code unknown_role_omits_model_flag passes.
  - [ ] AC4.3: cargo build &amp;&amp; cargo test --features runner-claude-code is green for the full suite (no regressions to existing 50+ tests).
  - [ ] AC4.4: README.md (or chosen doc) contains a &#x60;runner.yaml&#x60; mention and a &#x60;--model-planner&#x60; mention (grep verifies).
- **Files:** `src/runner/claude_code.rs`, `README.md`
- **Dependencies:** Phase 3 complete

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

_No execution cycles yet._

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

