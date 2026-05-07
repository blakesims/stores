# T034: Pi runner E2E smoke test

## Meta
- **Status:** code_review
- **Created:** 2026-05-05T14:34:58Z
- **Last Updated:** 2026-05-05T14:49:35Z
- **Current Phase:** 2
- **Current Cycle:** 1
- **Blocked Reason:** —
- **Branch:** feat/T034-auto-promoted-l110

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - Create or update a tiny disposable smoke-test artifact; drive the generated task with --pi; inspect transcript for Pi SDK final_output.
- **Out:** - Large implementation changes; changing production workflow semantics; accepting/rejecting the task after drive.

### Done When
Validate that stores tasks drive can run a tiny task through the Pi SDK runner and produce pi-tool structured output.

Acceptance:
- A linked task is promoted and driven with runner-pi; .stores/runs transcript contains final_output; drive log says via pi runner; task reaches a post-agent state without Claude runner involvement.

### Phases

#### Phase 1: Phase 1: Land pi runner artifacts in branch
- **Objective:** Bring the already-written pi runner module + helper into feat/T034 so the substrate can spawn a Pi-backed agent.
- **Tasks:**
  - Task 1.1: Copy /home/blake/repos/experiments/stores/src/runner/pi.rs into src/runner/pi.rs verbatim
  - Task 1.2: Copy /home/blake/repos/experiments/stores/agents/sidecar/pi_runner.mjs and system-prompt.md into agents/sidecar/
  - Task 1.3: Update Cargo.toml to add &#x60;runner-pi &#x3D; [&quot;dep:jsonschema&quot;]&#x60; feature (optional jsonschema already gated)
  - Task 1.4: In src/runner/mod.rs add &#x60;#[cfg(feature &#x3D; &quot;runner-pi&quot;)] pub mod pi;&#x60; and a &#x60;&quot;pi&quot;&#x60; arm in &#x60;select&#x60; mirroring the claude-code arm; extend &#x60;available_runners()&#x60; accordingly
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo build --features runner-pi&#x60; succeeds with zero warnings on new code
  - [ ] AC1.2: &#x60;cargo test --features runner-pi runner::pi&#x60; runs the 5 in-file unit tests and all pass
  - [ ] AC1.3: &#x60;cargo build --no-default-features&#x60; still compiles (pi module fully feature-gated)
- **Files:** `Cargo.toml`, `src/runner/mod.rs`, `src/runner/pi.rs`, `agents/sidecar/pi_runner.mjs`, `agents/sidecar/system-prompt.md`
#### Phase 2: Phase 2: Wire --pi flag into drive
- **Objective:** Expose the pi runner through &#x60;stores tasks drive --pi&#x60; so the orchestrator can pick it.
- **Tasks:**
  - Task 2.1: Add &#x60;#[cfg(feature &#x3D; &quot;runner-pi&quot;)] pub pi: bool&#x60; to DriveArgs in src/handlers/drive.rs
  - Task 2.2: In &#x60;build_runner&#x60;, after the --mock branch and parallel to --claude-code, add a feature-gated branch returning &#x60;crate::runner::select(&quot;pi&quot;)&#x60; when &#x60;args.pi&#x60; is true
  - Task 2.3: Update the &#x27;no runner selected&#x27; error message to mention &#x60;--pi&#x60;
  - Task 2.4: In src/cli/dynamic.rs (drive subcommand builder) register a &#x60;--pi&#x60; boolean flag, gated under &#x60;#[cfg(feature &#x3D; &quot;runner-pi&quot;)]&#x60;, mirroring &#x60;--claude-code&#x60;
  - Task 2.5: In src/main.rs drive dispatch, plumb the parsed --pi flag into DriveArgs (feature-gated)
  - Task 2.6: At drive_loop spawn-time, emit a single stderr log line &#x60;via pi runner&#x60; (or &#x60;via {runner.name()} runner&#x60;) immediately before the first agent spawn so the contract&#x27;s &#x27;drive log says via pi runner&#x27; is observable
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;cargo build --features runner-pi,runner-claude-code&#x60; succeeds
  - [ ] AC2.2: &#x60;stores tasks drive --help&#x60; (built with runner-pi) lists &#x60;--pi&#x60;
  - [ ] AC2.3: &#x60;stores tasks drive --pi &lt;id&gt; --max-iters 0&#x60; (no spawn) errors only on the iter cap, NOT on missing runner — proving --pi was accepted
  - [ ] AC2.4: New unit test in drive.rs &#x60;pi_flag_selects_pi_runner&#x60; builds a DriveArgs with &#x60;pi: true&#x60; and asserts &#x60;build_runner(&amp;args).unwrap().name() &#x3D;&#x3D; &quot;pi&quot;&#x60;
  - [ ] AC2.5: &#x60;cargo build&#x60; with default features (runner-claude-code only, no runner-pi) still compiles and &#x60;--claude-code&#x60; path is untouched
- **Files:** `src/handlers/drive.rs`, `src/cli/dynamic.rs`, `src/main.rs`
- **Dependencies:** Phase 1 complete: pi runner registered in &#x60;select&#x60;
#### Phase 3: Phase 3: Create disposable smoke-test task
- **Objective:** Mint a tiny T1 task whose drive cycle is the smoke-test target — sized so a single executor spawn can produce a final_output payload.
- **Tasks:**
  - Task 3.1: File observation L-pi-smoke via &#x60;stores observations add --invoker ai_autonomous --summary &#x27;pi runner smoke target&#x27; --source dev --priority normal --captured-at $(date -Iseconds) --captured-week $(date +w%V) --task-id T034 --body &#x27;Disposable smoke-test target for the Pi runner. Done-when: a markdown file docs/worklog/&lt;date&gt;/NN-pi-smoke-marker.md exists with a single timestamped line.&#x27;&#x60;
  - Task 3.2: Author a T1 contract for that observation (tier_hint&#x3D;T1, contract-is-plan) with a trivial done_when (write a one-line marker file). Ratify with &#x60;observations update LXXX --contract-state ready --approved-by blake --approved-at &lt;now&gt; --invoker ai_with_human --approve-token &lt;T&gt;&#x60; (HALT for user to provide approve-token; do not fabricate)
  - Task 3.3: Wait ≤10s for the auto-promote subscriber to mint the linked task (check &#x60;stores tasks status&#x60; until a new T### appears). Record its display id as SMOKE_ID
  - Task 3.4: Confirm SMOKE_ID&#x27;s tier_hint is T1 via &#x60;stores tasks render SMOKE_ID&#x60; and inspect the projection for tier&#x3D;T1 (so planner+plan_reviewer are skipped per T027)
- **Acceptance Criteria:**
  - [ ] AC3.1: Observation row exists with &#x60;contract_state&#x3D;ready&#x60; and &#x60;tier_hint&#x3D;T1&#x60;
  - [ ] AC3.2: A linked task SMOKE_ID exists (status &#x60;ready&#x60; or &#x60;planning&#x60;) with &#x60;linked_observations&#x60; containing L-pi-smoke
  - [ ] AC3.3: SMOKE_ID&#x27;s tier is T1 (skip-plan edge will fire on first drive iteration)
- **Files:** `.stores/db.sqlite (substrate writes)`, `tasks/active/&lt;SMOKE_ID&gt;-*/main.md (rendered projection)`
- **Dependencies:** Phase 2 complete (so we have a &#x60;--pi&#x60;-aware binary to drive with), User must paste decrypted approve-token in chat for U1 ratification (HALT point)
#### Phase 4: Phase 4: Drive smoke task with --pi and verify pi-tool output
- **Objective:** Execute the smoke target end-to-end through the Pi runner and verify all four contract acceptance signals.
- **Tasks:**
  - Task 4.1: Pre-flight: confirm &#x60;node --version&#x60; works and &#x60;STORES_PI_SDK_PATH&#x60; (or default &#x60;/home/blake/.npm-global/lib/node_modules/@mariozechner/pi-coding-agent/dist/index.js&#x60;) exists; abort + file an observation if missing
  - Task 4.2: Run &#x60;stores tasks drive SMOKE_ID --pi --max-iters 5 2&gt;&amp;1 | tee /tmp/pi-smoke-drive.log&#x60;
  - Task 4.3: Inspect &#x60;.stores/runs/*.jsonl&#x60; newest file: assert it contains a line with &#x60;&quot;type&quot;:&quot;final_output&quot;&#x60; and a &#x60;payload&#x60; object
  - Task 4.4: Inspect /tmp/pi-smoke-drive.log: assert it contains &#x60;via pi runner&#x60; and contains zero occurrences of &#x60;claude&#x60; (case-insensitive grep)
  - Task 4.5: Inspect SMOKE_ID&#x27;s status via &#x60;stores tasks status SMOKE_ID&#x60;: assert it has advanced past &#x60;ready&#x60; (e.g. to &#x60;complete&#x60;, &#x60;code_review&#x60;, or any post-spawn state)
  - Task 4.6: Write docs/worklog/2026-05-05/NN-pi-runner-smoke-result.md (via ./new-note.sh) capturing the four observed signals + the session_id of the transcript
  - Task 4.7: Do NOT run &#x60;tasks accept SMOKE_ID&#x60; — scope-out forbids it. Leave the smoke task in its post-agent state for later disposal
- **Acceptance Criteria:**
  - [ ] AC4.1: &#x60;.stores/runs/&lt;sid&gt;.jsonl&#x60; for the drive run contains at least one &#x60;&quot;type&quot;:&quot;final_output&quot;&#x60; event
  - [ ] AC4.2: drive log contains exactly one line matching &#x60;via pi runner&#x60; and zero lines mentioning the claude runner being spawned
  - [ ] AC4.3: SMOKE_ID&#x27;s status is NOT &#x60;ready&#x60; and NOT &#x60;planning&#x60; after drive returns (i.e., the runner executed and the substrate transitioned)
  - [ ] AC4.4: Worklog note exists under docs/worklog/2026-05-05/ recording session_id, drive exit, and the four signals
  - [ ] AC4.5: SMOKE_ID has not been accepted/rejected (scope-out respected)
- **Files:** `.stores/runs/&lt;sid&gt;.jsonl`, `docs/worklog/2026-05-05/NN-pi-runner-smoke-result.md`, `/tmp/pi-smoke-drive.log (transient)`
- **Dependencies:** Phase 3 complete: SMOKE_ID exists, Phase 1+2 complete: --pi-aware binary built and on PATH (or invoked via target/debug/stores), Pi SDK installed at expected node_modules path

---

## Plan Review

### Review 1
- **Gate:** READY
- **Summary:** Plan is executable. All 4 phases have mechanical ACs traceable to the contract&#x27;s four signals (linked task promoted, final_output in transcript, &#x27;via pi runner&#x27; in log, post-agent state). Phase ordering is correct: artifacts → flag wiring → smoke task mint → drive+verify. Decision matrix covers the five real choices (artifact source, tier, mint path, log shape, SDK location). One minor concern flagged for the executor: Task 4.4&#x27;s &#x27;zero occurrences of claude (case-insensitive)&#x27; grep is stricter than AC4.2&#x27;s &#x27;no claude runner spawn line&#x27; — false positives possible from feature names or agent paths in the log. Executor should treat AC4.2 as authoritative and scope the grep to spawn/runner lines.
- **At:** 2026-05-05T14:39:52Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Copied src/runner/pi.rs and agents/sidecar/{pi_runner.mjs,system-prompt.md} from dev branch. Added runner-pi cargo feature, registered pi module + select arm + available_runners entry behind #[cfg(feature &#x3D; &quot;runner-pi&quot;)]. cargo build --features runner-pi: zero warnings; cargo test --features runner-pi runner::pi: 6/6 pass (plan said 5; helper has 6); cargo build --no-default-features: clean (warning fixed via #[allow(unused_mut)] on the conditional vec). agents/sidecar/system-prompt.md was already present and identical to source.
- **Commit:** `94d89ff`
- **Files:**
  - `Cargo.toml`
  - `src/runner/mod.rs`
  - `src/runner/pi.rs`
  - `agents/sidecar/pi_runner.mjs`
  - `agents/sidecar/system-prompt.md`
- **At:** 2026-05-05T14:41:57Z
### Phase 2 / Cycle 1
- **Status:** Submitted — awaiting review
- **Summary:** Implemented T034 P2 --pi drive wiring: DriveArgs/build_runner select pi, dynamic CLI exposes --pi behind runner-pi, dispatch plumbs it, and drive_loop logs &#x60;via {runner.name()} runner&#x60; before first spawn. Deviation: drive dispatch is in src/cli/dispatch.rs, not src/main.rs. Verified cargo build with runner-pi,runner-claude-code; default cargo build; --help lists --pi; --pi T034 --max-iters 0 errors on iter cap; pi_flag_selects_pi_runner passes. Full cargo test remains red in unrelated sidecar_handoff tests.
- **Commit:** `5c59bb4f642c78ee4776e811b8fbaaee1b5ef7a2`
- **Files:**
  - `src/handlers/drive.rs`
  - `src/cli/dynamic.rs`
  - `src/cli/dispatch.rs`
- **At:** 2026-05-05T14:49:35Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 3 ACs verified: cargo build --features runner-pi succeeds with no warnings; cargo test --features runner-pi runner::pi runs 6/6 tests (executor flagged the 5-vs-6 plan discrepancy — helper has one additional test, acceptable); cargo build --no-default-features compiles clean. Commit 94d89ff matches the claimed file set (Cargo.toml, src/runner/mod.rs, src/runner/pi.rs, agents/sidecar/pi_runner.mjs); system-prompt.md was already tracked, executor&#x27;s note is accurate. Two minor findings on workspace hygiene.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
[MINOR] Uncommitted refactor in working tree.
File: src/runner/mod.rs:137-147
Evidence: &#x60;git status --porcelain&#x60; shows &#x60; M src/runner/mod.rs&#x60;. The committed version (94d89ff) uses &#x60;#[allow(unused_mut)] let mut runners &#x3D; vec![&quot;mock&quot;]; … runners.push(...);&#x60; while the working tree has rewritten available_runners() to use an array-literal with cfg-gated elements (cleaner, no unused_mut suppressor needed).
Expected: Phase commit should equal working-tree state for the files in scope; AC verification passes the same way against either version, but the dirty diff signals the executor either started P2 work or left an experimental refactor unsubmitted.
Suggestion: Either commit the array-literal refactor as a follow-up (&#x60;T034 P1 follow-up: array-literal for available_runners&#x60;) or revert the working-tree change with &#x60;git checkout -- src/runner/mod.rs&#x60; before P2 starts, so subsequent phases build off a clean tree.

[MINOR] AC1.2 phrasing drift vs reality.
File: plan acceptance criteria for Phase 1
Evidence: AC1.2 says &quot;runs the 5 in-file unit tests&quot; but &#x60;cargo test --features runner-pi runner::pi&#x60; reports 6 tests passed. Executor explicitly noted this in their submission summary.
Expected: ACs should match implementation.
Suggestion: Update the plan&#x27;s AC1.2 to read &quot;runs the in-file unit tests (currently 6) and all pass&quot; so future re-runs aren&#x27;t tripped up by the off-by-one. Non-blocking — executor surfaced the discrepancy honestly.

[INFORMATIONAL] Untracked tasks/active/T034-… and tasks/planning/T034-… directories present in workspace — these are the substrate&#x27;s render projection and planning artifacts, expected during a drive cycle. Not part of P1 scope; not blocking.
- **At:** 2026-05-05T14:43:35Z

---

## Completion
_Not yet complete._

