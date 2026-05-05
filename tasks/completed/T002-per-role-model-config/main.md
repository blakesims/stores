# T002: per-role model configuration for substrate runner

## Meta
- **Status:** accepted
- **Created:** 2026-05-03T06:07:04Z
- **Last Updated:** 2026-05-03T07:38:05Z
- **Current Phase:** 4
- **Current Cycle:** 1
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

### Review 1
- **Gate:** READY
- **Summary:** Plan is executable: 4 phases trace cleanly to Done When #1-#7, every AC is mechanically verifiable (cargo build/test exit codes, grep substrings, git ls-files, argv capture via shim). Phase ordering is correct (Phase 2 depends on Phase 1&#x27;s with_models; Phase 3 depends on both; Phase 4 is the e2e gate). Backward-compat with_model(), --testing precedence, CLI-overrides-config precedence, and the spawn-line model log are all explicitly covered. Minor notes the executor can resolve in-flight: AC3.6&#x27;s parenthetical (&#x27;mock returns Some(&quot;&lt;default&gt;&quot;)&#x27;) is slightly off — mock returns None per Task 1.6&#x27;s default impl and unwrap_or(&quot;&lt;default&gt;&quot;) yields the substring; the assertion (log contains &#x27;(model&#x3D;&#x27;) still holds. AC3.3 relies on std::env::current_dir() so the test will need to chdir into the tempdir or the executor can refactor build_runner to take repo_root — not a blocker. Underscore-vs-hyphen normalization is acknowledged in Task 2.3; default_models() in Task 2.2 lists underscore keys and the loader normalizes — executor should keep one canonical form post-load.
- **At:** 2026-05-03T06:13:44Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Replaced ClaudeCodeRunner::model: Option&lt;String&gt; with models: HashMap&lt;String,String&gt;; spawn() emits --model&#x3D;&lt;m&gt; via models.get(role). Added ROLE_KEYS const, with_models() and public model_for(); rewrote with_model() to populate all 5 role keys (backward-compat). Extended Runner trait with default model_for returning None (mock inherits it). Added 3 unit tests (with_model_populates_all_five_role_keys, model_for_returns_some_when_present_none_when_absent, spawn_passes_per_role_model_flag using argv-recording shim). cargo build --features runner-claude-code succeeds; 515 lib tests + 2 integration tests pass.
- **Commit:** `0377c0633c0c40e58c0bd3c45d8b0acaff34efa3`
- **Files:**
  - `src/runner/mod.rs`
  - `src/runner/claude_code.rs`
- **At:** 2026-05-03T06:15:59Z
### Phase 2 / Cycle 1
- **Status:** PASS
- **Summary:** Implemented src/runner/config.rs with RunnerConfig/ClaudeCodeConfig (serde_yaml), default_models() returning the 5-role opus/opus/sonnet/opus/opus map, and load_models_from_repo() that merges .stores/runner.yaml over defaults with underscore→hyphen role-key normalisation. Registered pub mod config in src/runner/mod.rs. Committed default .stores/runner.yaml; narrowed .gitignore from /.stores/ to /.stores/db.sqlite, /.stores/runs/, /.stores/runs.lock so runner.yaml is tracked. cargo build clean; 4 runner::config::tests pass; full cargo test --features runner-claude-code green (519 + 2).
- **Commit:** `559bb15c6cbeabe3aa631ed3e9d7e95b04b218fb`
- **Files:**
  - `src/runner/config.rs`
  - `src/runner/mod.rs`
  - `.stores/runner.yaml`
  - `.gitignore`
- **At:** 2026-05-03T06:19:02Z
### Phase 3 / Cycle 1
- **Status:** PASS
- **Summary:** Phase 3: Added 5 per-role model fields to DriveArgs (model_planner/_plan_reviewer/_executor/_code_reviewer/_wrap), gated on feature runner-claude-code. Extracted build_models_map(args, repo_root) helper that layers defaults &lt; runner.yaml &lt; CLI overrides &lt; --testing (haiku, highest precedence) and feeds ClaudeCodeRunner::with_models. Added 5 --model-&lt;role&gt; clap args in build_drive_cmd (made pub(crate) so tests can render its --help). Wired dispatch.rs to read flags via get_one::&lt;String&gt;. Updated 2 existing DriveArgs fixtures with new fields. Replaced spawn-line eprintln with format_spawn_line helper that includes &#x60;(model&#x3D;&lt;m&gt;)&#x60; substring (or &#x60;&lt;default&gt;&#x60; when runner.model_for returns None). Added 7 new unit tests covering AC3.2-3.6 (defaults from yaml, runner.yaml override, CLI override beats yaml, --testing beats CLI flags, format substring assertions, drive --help lists all five flags). cargo build --features runner-claude-code green; all 526 tests pass on default and runner-claude-code feature sets. Deviation note for AC3.6: unit-tested via format_spawn_line() helper rather than stderr capture, since the codebase has no stderr-capture infra; the helper extraction makes the format string directly assertable.
- **Commit:** `4e31092914a40a8177b01f0c47d50b3b3ccfa65d`
- **Files:**
  - `src/cli/dispatch.rs`
  - `src/cli/dynamic.rs`
  - `src/handlers/drive.rs`
- **At:** 2026-05-03T06:26:19Z
### Phase 4 / Cycle 1
- **Status:** PASS
- **Summary:** Added argv_recorder shim to SHIM_DIR (writes argv to $ARGV_OUT or cwd-relative argv.txt) and two new tests: per_role_model_passes_through_to_argv drives spawn for all 5 roles with distinct model aliases and verifies --model&#x3D;&lt;alias&gt; in recorded argv per role; unknown_role_omits_model_flag verifies no --model&#x3D; token when the role has no map entry. Appended a 14-line &#x27;Per-role model configuration&#x27; section to README.md pointing at .stores/runner.yaml and the --model-&lt;role&gt; overrides. cargo build clean; full suite 528 + 2 fixture tests green with --features runner-claude-code.
- **Commit:** `1b8812f`
- **Files:**
  - `src/runner/claude_code.rs`
  - `README.md`
- **At:** 2026-05-03T06:29:25Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 5 ACs verified: cargo build --features runner-claude-code succeeds; all 38 runner tests pass including 3 new tests (with_model_populates_all_five_role_keys, model_for_returns_some_when_present_none_when_absent, spawn_passes_per_role_model_flag); model_for() returns Some/None as specified; spawn emits --model&#x3D;&lt;m&gt; for mapped roles and omits the flag when absent. 0 critical, 0 major, 3 minor (doc-comment placement, missing blank line, brief SHA typo).
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] ClaudeCodeRunner struct doc-comment is now glued onto ROLE_KEYS.
File: src/runner/claude_code.rs:58-71
Evidence: The original /// block describing &#x27;Runner that shells out to the claude CLI...&#x27; (lines 58-63) immediately precedes the new line &#x60;/// Canonical role keys recognised by the per-role model map.&#x60; (line 64) and then &#x60;pub const ROLE_KEYS: [&amp;str; 5]&#x60;. There is no blank line separating them, so rustdoc will attribute the entire paragraph + the new sentence to ROLE_KEYS, and ClaudeCodeRunner (line 73) loses its doc comment entirely.
Expected: ClaudeCodeRunner retains its prior doc comment; ROLE_KEYS gets only its own one-liner.
Suggestion: Insert a blank line between line 63 and the new ROLE_KEYS doc comment, and move ROLE_KEYS either above the struct doc-comment block or below the struct (e.g., right above the &#x60;impl ClaudeCodeRunner&#x60; block).

[MINOR] No direct test for &#x60;with_models()&#x60; constructor.
File: src/runner/claude_code.rs:945-991
Evidence: AC tests cover with_model() (single-model fan-out) and model_for(), and &#x60;with_models&#x60; is exercised indirectly by &#x60;model_for_returns_some_when_present_none_when_absent&#x60; and &#x60;spawn_passes_per_role_model_flag&#x60; Case A. There is no test that explicitly asserts &#x60;ClaudeCodeRunner::with_models(map).models &#x3D;&#x3D; map&#x60; for an arbitrary multi-role map (e.g. planner&#x3D;opus, executor&#x3D;sonnet, code-reviewer&#x3D;opus). Not blocking — the constructor is trivial — but a one-liner would harden the public API surface that Phase 3 will rely on.
Suggestion: Add a small test that constructs a 3-entry map, builds via with_models, and asserts model_for returns the expected values for each role and None for the unmapped two.

[INFORMATIONAL] Brief commit SHA appears to be a typo.
Evidence: Executor reported commit&#x3D;0377c0633c0c40e58c0bd3c45d8b0acaff34efa3 but &#x60;git rev-parse HEAD&#x60; is 0377c067d67b15bec8334a99955e8e91b54504c7. Short prefixes match (0377c06), so this is the same commit on HEAD — likely a transcription artifact, not a real mismatch.

[INFORMATIONAL] Role key casing convention.
ROLE_KEYS uses kebab-case (&#x60;plan-reviewer&#x60;, &#x60;code-reviewer&#x60;), which matches what &#x60;drive::handlers::drive&#x60; already passes to spawn() after &#x60;agent_role.replace(&#x27;_&#x27;, &#x27;-&#x27;)&#x60; normalization (src/handlers/drive.rs:503). The done-when contract (3) lists CLI flag forms in snake_case (&#x60;--model-plan_reviewer&#x60;); Phase 3 will need to normalize CLI input (snake → kebab) before keying into the map. Flagging now so Phase 3 planner/executor are aware. Not a Phase 1 defect.
- **At:** 2026-05-03T06:17:25Z

### Phase 2 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 4 ACs verified: 4 runner::config tests green, .stores/runner.yaml is git-tracked, .gitignore narrowed to db.sqlite/runs/runs.lock, roundtrip test passes. 0 critical, 0 major, 3 minor (no validation of unknown role keys; underscore→hyphen normalization is one-way only; default_models docstring doesn&#x27;t list role values).
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] Unknown role keys silently accepted.
File: src/runner/config.rs:60-62 (the merge loop in load_models_from_repo)
Evidence: &#x60;for (k, v) in cfg.claude_code.models { merged.insert(normalize_role_key(&amp;k), v); }&#x60; accepts any key — a typo like &#x60;planr: haiku&#x60; lands in the map as the literal &#x60;planr&#x60; and is invisibly ignored when spawn looks up by role.
Expected: AC2 contract names exactly five canonical roles. Foreign keys should at least warn.
Suggestion: After merging, log a warning (or filter+warn) for keys not in the canonical 5-role set. Non-blocking for this phase since spawn lookup will simply miss; revisit in P3 when CLI overrides land.

[MINOR] normalize_role_key is one-way (underscore→hyphen) and undocumented in the public function.
File: src/runner/config.rs:46-48
Evidence: &#x60;fn normalize_role_key(k: &amp;str) -&gt; String { k.replace(&#x27;_&#x27;, &quot;-&quot;) }&#x60; — file-level doc-comment mentions the normalization, but &#x60;load_models_from_repo&#x60; doc does not. A caller reading only that fn signature would not know underscored keys are accepted.
Suggestion: Add a one-liner to load_models_from_repo&#x27;s doc-comment: &#x27;YAML keys with underscores are accepted as aliases for hyphenated canonical form.&#x27;

[MINOR] default_models() doc-comment doesn&#x27;t list which model is which.
File: src/runner/config.rs:36
Evidence: &#x60;/// The built-in 5-role default map.&#x60; — the next reader has to scan the body to learn planner&#x3D;opus, executor&#x3D;sonnet, etc. The runner.yaml comment is more informative than the rustdoc.
Suggestion: Expand to &#x27;/// The built-in 5-role default map: planner&#x3D;opus, plan-reviewer&#x3D;opus, executor&#x3D;sonnet, code-reviewer&#x3D;opus, wrap&#x3D;opus.&#x27; so rustdoc readers don&#x27;t need to read the body.

[INFORMATIONAL] cargo test --features runner-claude-code runner::config:: → 4 passed, 0 failed.
[INFORMATIONAL] git ls-files .stores/runner.yaml → &#x27;.stores/runner.yaml&#x27; (tracked).
[INFORMATIONAL] .gitignore narrowed correctly: /.stores/ removed, /.stores/db.sqlite + /.stores/runs/ + /.stores/runs.lock added — runner.yaml is no longer caught by a parent-directory ignore.
- **At:** 2026-05-03T06:19:59Z

### Phase 3 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 6 ACs verified: cargo build --features runner-claude-code green; 526 tests pass (7 new P3 tests pass); --help lists --model-{planner,plan-reviewer,executor,code-reviewer,wrap}; defaults from code match contract (planner/plan-reviewer/code-reviewer/wrap&#x3D;opus, executor&#x3D;sonnet); CLI overrides beat runner.yaml; --testing forces all 5 roles to haiku beating per-role flags; spawn-line includes (model&#x3D;&lt;m&gt;) substring (or &lt;default&gt; when None). 0 critical, 0 major, 3 minor (AC3.6 verified via helper rather than stderr capture; build_runner uses cwd not git-root for repo path; DriveArgs::testing doc not updated to mention new precedence chain).
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
[MINOR] AC3.6 verification deviation
File: src/handlers/drive.rs (tests &#x60;spawn_line_format_contains_model_substring&#x60;, &#x60;spawn_line_format_default_model_when_unset&#x60;)
Evidence: AC3.6 specifies &#x27;verified by capturing stderr in an existing drive_loop integration test&#x27;. Executor extracted &#x60;format_spawn_line()&#x60; and asserts the format string directly.
Expected: stderr capture in a drive_loop integration test (per AC wording).
Suggestion: Functionally equivalent — the helper is invoked at drive.rs:686 in the real spawn path, so the substring guarantee holds. Acceptable as documented deviation; no change required unless plan-reviewer disagrees.

[MINOR] build_runner uses cwd, not detected repo root, for runner.yaml lookup
File: src/handlers/drive.rs:381 (&#x60;build_models_map(args, &amp;std::env::current_dir()?)&#x60;)
Evidence: If &#x60;stores tasks drive&#x60; is invoked from a subdirectory, &#x60;.stores/runner.yaml&#x60; resolution will silently fall through to baked-in defaults rather than reading the project file.
Expected: Pass the git/workspace root (analogous to other repo-root-aware handlers).
Suggestion: Use the same workspace-root resolution helper used elsewhere in the codebase (e.g. &#x60;paths::stores_dir_for_repo&#x60;-style lookup). Defer to a follow-up if Phase 2&#x27;s loader contract intentionally took a &#x60;&amp;Path&#x60; to keep responsibility shifted to caller.

[MINOR] DriveArgs::testing doc-comment not extended
File: src/handlers/drive.rs:165 (approximate)
Evidence: The doc comment on &#x60;pub testing: bool&#x60; predates the per-role field block; readers must hop down to the per-role fields&#x27; doc to see the precedence relationship.
Expected: One-line note that --testing has higher precedence than per-role overrides.
Suggestion: Append &#x27;Beats per-role &#x60;model_*&#x60; overrides.&#x27; to the existing doc-comment on &#x60;testing&#x60;.
- **At:** 2026-05-03T06:27:38Z

### Phase 4 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. AC4.1 per_role_model_passes_through_to_argv passes; AC4.2 unknown_role_omits_model_flag passes; AC4.3 full suite green (528 unit + 2 fixture tests, no regressions); AC4.4 README contains both &#x27;runner.yaml&#x27; and &#x27;--model-planner&#x27; mentions. Argv-recorder shim uses per-role tempdir for workspace_path so parallel-test contention is avoided. 0 critical, 0 major, 2 minor (cosmetic/docs).
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
Git reality check: HEAD&#x3D;1b8812f, files changed match submission (README.md, src/runner/claude_code.rs, +97/-1). Diff inspected end-to-end.

AC verification:
- AC4.1 PASS: cargo test --features runner-claude-code per_role_model_passes_through_to_argv → ok. The test inserts all 5 hyphenated role keys (planner, plan-reviewer, executor, code-reviewer, wrap) — these match the canonical keys used in src/cli/agents.rs:25-30 and src/runner/config.rs:38-41, so the test exercises the real role-naming surface. Each role uses its own tempdir as workspace_path; the shim writes argv.txt under that cwd; assertion checks for exact line &#x27;--model&#x3D;&lt;alias&gt;&#x27;. Real spawn() at src/runner/claude_code.rs:383 emits &#x60;--model&#x3D;&lt;m&gt;&#x60; (no space) — assertion form matches.
- AC4.2 PASS: cargo test --features runner-claude-code unknown_role_omits_model_flag → ok. Asserts no line starts with &#x27;--model&#x3D;&#x27; when role is unmapped — correct semantics.
- AC4.3 PASS: cargo build clean; cargo test --features runner-claude-code → 528 passed; integration fixture suite → 2 passed. No regressions.
- AC4.4 PASS: grep -n &#x27;runner.yaml\|--model-planner&#x27; README.md returns lines 310 and 322. Both mentions present, surrounded by a coherent 14-line documentation block.

[MINOR] README YAML example uses underscored keys (plan_reviewer, code_reviewer) while src/runner/config.rs:1-6 documents hyphenated form as canonical. The loader&#x27;s normalize_role_key (config.rs:45-47) accepts both, so the example is functional, but the README does not mention that hyphenated keys are also accepted. Consider adding a one-line note (&quot;underscores or hyphens accepted&quot;) to avoid confusion for users who copy-paste from drive&#x27;s CLI flags.

[MINOR] argv_recorder shim falls back to &#x27;$(pwd)/argv.txt&#x27; when ARGV_OUT is unset; the doc-comment at claude_code.rs:580-585 explains this clearly. However, neither current test sets ARGV_OUT, so the fallback path is effectively the only path exercised. Either drop the ARGV_OUT branch as YAGNI or add a test that uses it — keeping unexercised code paths in test fixtures is a small future-maintenance hazard.

[INFORMATIONAL] cargo build emits 3 pre-existing warnings under the &#x60;stores&#x60; bin test target (not introduced by this phase) — out of scope for this review.

All T002 phases now complete. Done When contract items 1-7 verifiable: per-role models map (P1), runner.yaml schema+loader (P2), drive CLI overrides + spawn log line (P3), end-to-end argv passthrough + docs (P4).
- **At:** 2026-05-03T06:30:56Z

---

## Completion
- **Accepted:** 2026-05-03T07:38:05Z
- **Branch:** feat/T002-per-role-model-config

