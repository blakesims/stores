# T005: Drive substrate fixes — `blocked` divergence + envelope-mismatch handling + log visibility

## Meta
- **Status:** CODE_REVIEW
- **Created:** 2026-04-30
- **Last Updated:** 2026-04-30
- **Blocked Reason:** —

## Task

`stores tasks drive --auto --claude-code` hung indefinitely on a 2-phase hello-world smoke test (`/tmp/t003-smoke`). Forensic audit of the JSONL transcripts and DB state surfaced **three real bugs** — including, ironically, the substrate-disagreement failure mode the project's own `docs/philosophy.md` was written to prevent. Fix all three so drive can be trusted on this repo's own tasks (T006 onwards).

### Forensic summary

5 spawns ran cleanly:

| spawn | role intended | role emitted | result |
|---|---|---|---|
| 1 (planner) | planner | planner | 2-phase plan |
| 2 (plan-review) | plan-reviewer | plan-reviewer | READY |
| 3 (executor P1) | executor | executor | created index.html |
| 4 (code-review P1) | code-reviewer | code-reviewer | PASS phase 1 |
| 5 (executor P2) | **executor** (per `next-action`) | **guide** ⚠ | `{"role":"guide","action":"noop"}` |

Spawn 5 returned a `guide` envelope while drive was at `status=executing, current_phase=2` waiting for an executor. Drive then sat silent for 5+ minutes at 0% CPU until SIGTERMed.

### Three bugs (ordered by severity)

**Bug 1 — `status` and `next-action` disagree on `blocked`.** `src/handlers/status.rs:160` does `task.blocked_reason.is_some() || task.status == "blocked"`. The DB stores `blocked_reason` as the empty string `""` (not NULL) for never-blocked rows, and `Option<String>::is_some()` returns `true` for `Some("")`. Result: `stores tasks status T001` reports `blocked=true` while `stores tasks next-action T001 --json` reports `"blocked": false`. Two readers, two answers, on the same row. The `next-action` view is the correct one (`src/handlers/next_action.rs:97` only checks `status == "blocked"`).

**Bug 2 — Drive hangs on un-routable envelope role.** `parse_envelope` at `src/handlers/drive.rs:550` deserialises via `AgentEnvelope` (a `serde(tag = "role")` enum with variants Planner/PlanReviewer/Executor/CodeReviewer). When the agent emits `role:"guide"`, deserialization returns `Err("unknown variant guide")`. The `?` operator at line 477 should propagate this and bail with non-zero exit. Instead, the process went silent (no log output, no DB writes, sleeping at 0% CPU). The proximate cause may be tail-buffered stderr (Bug 3 below), but the deeper concern is that drive's behavior on un-routable envelopes is not observable end-to-end.

**Bug 3 — Drive progress is invisible behind buffered pipes.** Drive's per-spawn `eprintln!` "spawning {agent_role} via claude-code runner..." (drive.rs:431) is buffered when piped into `tail -N`. The smoke-test invocation `stores tasks drive --auto --claude-code 2>&1 | tail -100` never displayed any progress; when SIGTERM eventually killed both processes, tail had nothing to flush, so the output file is 0 bytes. Operators cannot see what drive is doing live, and post-mortem recovery of the log is fragile. Fixes are some combination of: explicit stderr flushing, line-buffering, or a doc/help recommendation against `tail -N` wrapping.

### What's NOT in this task

- The four POC findings from `stores/observations_1006/` (transition guards on plain transitions, `list_record` write path, store-name dash in DDL, list-flag repeatability) — those are independent and should land as T006+.
- Diagnosing why the executor agent emitted a `guide` envelope. That's a model-discipline / system-prompt issue downstream of fixing how drive *handles* the mismatch. Worth a follow-up note but not gating this task.

### DONE_WHEN

A 2-phase hello-world smoke test runs `stores tasks drive --auto --claude-code` to `complete` without operator intervention; if any spawn returns an unrecognised envelope role, drive exits non-zero within seconds with an error message naming `(expected_role, received_role, session_id)`; and `stores tasks status` and `stores tasks next-action` agree on `blocked` for every row in unit tests covering `blocked_reason` ∈ {NULL, "", "real reason"}.

---

## Plan

### Objective

Eliminate the three substrate bugs that caused `stores tasks drive --auto --claude-code` to hang silently on the 2-phase hello-world smoke test, so that drive can be trusted to run T006+ on this repo's own tasks. Concretely:

> **DONE_WHEN:** A 2-phase hello-world smoke test runs `stores tasks drive --auto --claude-code` to `complete` without operator intervention; if any spawn returns an unrecognised envelope role, drive exits non-zero within seconds with an error message naming `(expected_role, received_role, session_id)`; and `stores tasks status` and `stores tasks next-action` agree on `blocked` for every row in unit tests covering `blocked_reason` ∈ {NULL, "", "real reason"}.

Three production bugs to fix, each independently committable, plus a final smoke-test phase that re-validates end-to-end against haiku.

### Scope

- **In Scope:**
  - `src/handlers/status.rs` — replace the `task.blocked_reason.is_some() || task.status == "blocked"` detector at line 160 with a shared helper.
  - `src/handlers/next_action.rs` — switch to the shared helper (semantics unchanged: it already only checks `status == "blocked"`).
  - `src/handlers/mod.rs` (or new sibling file `src/handlers/blocked.rs`) — host `pub fn is_blocked(status: &str, blocked_reason: Option<&str>) -> bool`.
  - `src/handlers/drive.rs` — make the `parse_envelope` error path observable: pre-validate the envelope role against `na.next_agent` (the role drive expects) before deserialise, and on mismatch bail with a single anyhow error that mentions `expected`, `received`, and `session_id`. Adopt explicit `io::stderr().flush()` after each pre-spawn / post-spawn `eprintln!` so progress is visible through `tail -N`.
  - Unit tests in `src/handlers/status.rs` and `src/handlers/next_action.rs` covering `blocked_reason ∈ {NULL, "", "real reason"}`.
  - A new `drive_loop`-driven integration test in `src/handlers/drive.rs` that feeds a mock-runner output containing `{"role":"guide","action":"noop"}` while the task is in `executing`, and asserts `Err(_)` whose message contains all three of `expected`, `received`, and the session id.
  - One end-to-end smoke phase that re-runs the 2-phase hello-world (`/tmp/t005-smoke` or equivalent fresh tempdir) and confirms drive exits 0 with task `status=complete`.
- **Out of Scope:**
  - 10.06 schema porting (`stores/observations_1006/`).
  - Transition-guard enforcement on plain (non-workflow) transitions.
  - `list_record` write-path round-trip and store-name-with-dash DDL escaping.
  - List-flag repeatability.
  - Diagnosing why the executor agent emitted a `guide` envelope (system-prompt / model-discipline issue, separate task).
  - Any change to `AgentEnvelope` variant set, `dispatch_submit` semantics, schema files under `agents/schemas/`, or runner internals beyond what's needed to surface the role mismatch.

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | Shared `is_blocked()` helper + reporters agree on `blocked` | Low |
| 2 | Drive surfaces unroutable-role mismatch with `(expected, received, session_id)` | Medium |
| 3 | Drive progress visible through pipe wrappers (explicit stderr flush) | Low |
| 4 | End-to-end haiku smoke re-validation | Low (operator-driven) |

### Phase Details

#### Phase 1: Shared `is_blocked()` helper; status & next-action agree

- **Objective:** Eliminate Bug 1. Introduce one canonical predicate that decides whether a task row is "blocked", consume it from both reporters, and pin the contract with unit tests over the three `blocked_reason` shapes.
- **Files to modify:**
  - `src/handlers/mod.rs` — add `pub fn is_blocked(status: &str, blocked_reason: Option<&str>) -> bool`. Definition: `status == "blocked"`. (The empty-string case is collapsed to "not blocked" because `next_action.rs:97` already takes that view, and the DB historically writes `""` for never-blocked rows.) Doc-comment names the bug it closes and the source-of-truth interpretation: a row is blocked iff its workflow status is `blocked`. Truthy `blocked_reason` is a *description* attached to a blocked row, not the predicate.
  - `src/handlers/status.rs` — replace line 160 (`let blocked = task.blocked_reason.is_some() || task.status == "blocked";`) with `let blocked = crate::handlers::is_blocked(&task.status, task.blocked_reason.as_deref());`.
  - `src/handlers/next_action.rs` — replace line 97 (`let is_blocked = status == "blocked";`) with `let is_blocked = crate::handlers::is_blocked(&status, blocked_reason.as_str());`. (Adjust the `as_deref` shape for the `serde_json::Value` field; passing `None` when the field is `Value::Null` and `Some(s)` when it's a `String` is sufficient — keep the existing semantics, just route the decision through the helper.)
- **Files to add:** none (helper lives in `src/handlers/mod.rs`).
- **Acceptance Criteria:**
  - [ ] `cargo test --lib handlers::status` includes three new tests `blocked_helper_null_reason`, `blocked_helper_empty_reason`, `blocked_helper_real_reason` that call `format_task_line` on a `TaskState` with `status="executing"` and `blocked_reason ∈ {None, Some(""), Some("real reason")}`, asserting `blocked=false` for the first two and `blocked=false` for the third (because `status != "blocked"`); plus a fourth test that sets `status="blocked"` with each of the three reasons and asserts `blocked=true` for all three.
  - [ ] `cargo test --lib handlers::next_action` includes a parallel `next_action_blocked_reason_shapes` test that calls `compute()` for the same three reason shapes (with `status="blocked"` and with `status="executing"`) and asserts `out.blocked` matches the status-based predicate, never the reason-based one. Reuse the existing `wf_schema()` / `insert_wf_row` scaffold in the bottom of `next_action.rs`.
  - [ ] `cargo test --all` passes.

#### Phase 2: Drive surfaces unroutable-role envelope as `(expected, received, session_id)` error

- **Objective:** Eliminate Bug 2. When the runner returns an envelope whose `role` does not match `AgentEnvelope`'s variant set (or matches a *different* variant than drive expected for the current status), drive must exit non-zero within seconds and the error must name the expected role, the received role, and the runner-issued `session_id`.
- **Files to modify:**
  - `src/handlers/drive.rs` —
    1. Extend `parse_envelope`'s signature to `fn parse_envelope(out: &RunnerOutput, agent_role_normalized: &str, expected_role: &str) -> Result<(AgentEnvelope, &'static str)>`. Before the existing serde deserialise calls in Layers 1/2/3, peek the `role` field on the candidate `Value` (or, for Layer 3 stdout-scan, on the parsed line). If the peeked role is not equal to `expected_role`, return `Err` immediately with the format string `"envelope role mismatch: expected {expected_role}, received {role}, session_id {session_id}"`, where `session_id` falls back to the literal string `"<unknown>"` when `out.session_id` is `None`. Do not let serde return the generic `unknown variant guide` message — the new mismatch error subsumes it for *both* unknown variants and known-but-wrong variants.
    2. At the call site (line 477), pass `agent_role` (or `agent_name_normalized`) as `expected_role`. The existing `?` propagation already aborts the loop with a non-zero exit code; verify by reading the existing `eprintln!("[{display_id}] envelope parse failed: {e}");` line — the new error message arrives via `e` and is therefore surfaced.
  - `src/handlers/drive.rs` (tests block, ~line 1080+) — add `drive_loop_unroutable_role_exits_nonzero` and `drive_loop_role_mismatch_message_format`:
    - First test: insert a task with `status="executing"`, build a `MockRunner` that returns `RunnerOutput { stdout: "{\"role\":\"guide\",\"action\":\"noop\"}", final_message: Some(..), structured_output: Some(..), session_id: Some("smoke-session-uuid"), exit_code: 0, .. }`. Assert `drive_loop(...)` returns `Err(_)`.
    - Second test: same setup; assert the error message contains the three substrings `"expected"` (or `"executor"` literally — pick whichever is canonical and assert it), `"guide"`, and `"smoke-session-uuid"`. (One test would do but splitting them keeps the assertion failure messages diagnostic.)
- **Acceptance Criteria:**
  - [ ] `cargo test --lib handlers::drive::tests::drive_loop_unroutable_role_exits_nonzero` and `..::drive_loop_role_mismatch_message_format` pass.
  - [ ] All four existing `parse_envelope_from_*_fixture` tests still pass after the signature change (they pass a matching `expected_role`).
  - [ ] `cargo test --all` passes; `tests/drive_e2e.sh` still passes (the mock fixtures already emit the right role for the current status, so the new mismatch check is a no-op for them).

#### Phase 3: Drive progress visible through pipe wrappers (explicit stderr flush)

- **Objective:** Eliminate Bug 3. After every `eprintln!` in `drive_loop` (the four existing pre-spawn / post-spawn / submit-progress / blocked-exit announcements at lines 430, 437, 460, 478, 516, 525), call `std::io::stderr().flush().ok()` so a `2>&1 | tail -N` wrapper sees per-spawn progress as it happens.
- **Files to modify:**
  - `src/handlers/drive.rs` — add `use std::io::Write;` to the imports; introduce a small helper `fn eprintln_flushed(args: std::fmt::Arguments)` (or a `macro_rules!` `eprintln_flush!`) used at each existing announcement site. Cleaner alternative: leave the `eprintln!`s in place and append a single `let _ = std::io::stderr().flush();` after each progress announcement. Pick the latter — minimum diff, no macro hygiene risk.
- **Acceptance Criteria:**
  - [ ] Manual: `stores tasks drive --auto --claude-code 2>&1 | tail -100` shows the "spawning ... runner" line within 1 second of drive entering each spawn (verified during Phase 4 smoke; recorded in the Phase 3 executor log as a one-line note "tail-N visibility confirmed").
  - [ ] No regression in any unit test; `cargo test --all` passes.

#### Phase 4: End-to-end haiku smoke re-validation

- **Objective:** Confirm the three fixes hold under a real claude-code spawn. This phase is the literal `DONE_WHEN`.
- **Files to modify:** none (operator-driven; only artefacts captured).
- **Steps:**
  1. `mkdir -p /tmp/t005-smoke && cd /tmp/t005-smoke && rm -rf .stores tasks`
  2. `stores setup`
  3. `stores tasks add --invoker human --title "Hello world" --slug "hello" --done-when "echo hi prints hi" --scope-in "scripts/" --scope-out "src/"`
  4. `stores tasks drive --auto --claude-code --testing 2>&1 | tee drive-smoke.log`
  5. After exit, capture `stores tasks status T001 --json` and `stores tasks next-action T001 --json` to verify they agree.
- **Acceptance Criteria:** _(this is the literal `DONE_WHEN`)_
  - [ ] `stores tasks drive --auto --claude-code` runs to `status=complete` without operator intervention on a fresh 2-phase hello-world tempdir.
  - [ ] If any spawn returns an unrecognised envelope role (manually constructed sanity check via mock fixture, *not* expected during the haiku run itself), drive exits non-zero within seconds with an error message naming `(expected_role, received_role, session_id)`. Demonstrate via the Phase 2 unit test artefact rather than re-spawning haiku.
  - [ ] `stores tasks status T001` and `stores tasks next-action T001` both report `blocked=false` for the completed row, and the relevant Phase 1 unit tests stay green for `blocked_reason ∈ {NULL, "", "real reason"}`.

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| Where the `is_blocked()` helper lives | (a) `src/handlers/mod.rs` as `pub fn`; (b) new file `src/handlers/blocked.rs`; (c) duplicate in both reporters as a `pub(crate) fn` re-imported | (a) | It's three lines and shared by exactly two callers; a sibling file is over-structuring. `mod.rs` already houses cross-handler glue. |
| Helper signature | (a) `(status: &str, blocked_reason: Option<&str>) -> bool`; (b) `(task: &TaskState) -> bool`; (c) trait method on `TaskState` | (a) | `next_action` doesn't have a `TaskState` (it builds a `NextActionOutput` from `read_row`'s `EntryMap`). A free function over primitives crosses the call-site mismatch without dragging types around. |
| Empty-string `blocked_reason` interpretation | (a) "not blocked" (status-only predicate); (b) "blocked" (existing `status.rs` behaviour); (c) treat empty string as a data error | (a) | `next_action.rs:97` is already on (a); the DB historically writes `""` for never-blocked rows; the only divergent reader is the buggy `status.rs:160`. Aligning to (a) is the smaller, more conservative move. (c) would require a migration and is out of scope. |
| Where to enforce role-mismatch | (a) inside `parse_envelope` (signature gains `expected_role`); (b) at the call site after `parse_envelope` returns; (c) inside `dispatch_submit` | (a) | `dispatch_submit` already enforces *status*-vs-envelope-variant compatibility (lines 634, 655, 684, 710), but only after deserialise succeeds — so it can't catch the un-routable case. Catching at `parse_envelope` is the earliest point with both `expected_role` and the candidate `role` field in scope; it also keeps the error path single-source so future Layer-4 additions inherit the check for free. |
| Bug 3 fix shape | (a) explicit `stderr().flush()` after each progress eprintln; (b) line-buffer stderr globally via `setvbuf` libc; (c) document operator workaround `--unbuffered` and skip the code change | (a) | (b) requires unsafe + cross-platform thinking for one feature already considered "in-scope visibility". (c) shifts the burden to operators and contradicts the DONE_WHEN's "without operator intervention" framing. (a) is 6 one-line additions and zero behavioural surprise — same diff size as documenting (c) but actually fixes it. |
| Smoke-test phase as code-reviewable artefact | (a) require a captured `drive-smoke.log` and DB dump in the executor log; (b) trust the operator's PASS/FAIL claim | (a) | T004 set the precedent (Phase 3 transcripts referenced from `tests/fixtures/agent_outputs/`). The log is cheap to capture and is the only post-mortem trace if Phase 4 fails on a real model run. |

---

## Plan Review

- **Gate:** READY
- **Open Questions Finalized:** none — every divergence the planner could not have decided alone (helper location, signature, empty-string semantics, role-mismatch layer, eprintln strategy, smoke-log capture) is recorded in the Decision Matrix with a defensible rationale. No question requires human input before execution.
- **Issues Found:** four minor / cosmetic items (M1–M4 below). None are gating; the executor can fix them in-flight without a re-plan. Two implementation risks (R1, R2) are noted for the executor but do not block.

### Verification against the seven review checks

1. **DONE_WHEN alignment.** All three clauses map cleanly:
   - Clause "runs to `complete` without operator intervention" → Phase 4 AC line 133 (verbatim).
   - Clause "exits non-zero within seconds with `(expected_role, received_role, session_id)`" → Phase 2 AC line 109 plus the explicit `drive_loop_role_mismatch_message_format` test asserting all three substrings (`expected`/`executor`, `guide`, `smoke-session-uuid`). The Phase 2 inner format string at plan line 103 is the canonical implementation.
   - Clause "`status` and `next-action` agree on `blocked` for `blocked_reason ∈ {NULL, "", "real reason"}`" → Phase 1 AC bullets 1–2 cover both reporters in parallel tests across all three reason shapes.

2. **Bug coverage completeness.** Bug 1 → Phase 1, Bug 2 → Phase 2, Bug 3 → Phase 3, integration → Phase 4. All three forensic root causes are addressed; nothing in `## Task` is left dangling. Out-of-scope items (POC findings, model-discipline diagnosis) are explicitly excluded in the Scope block.

3. **Phase independence.** Verified. Phase 1 introduces a new helper used only by status/next_action. Phase 2 modifies `parse_envelope`'s signature in drive.rs, untouched by Phase 1. Phase 3 sprinkles `stderr().flush()` calls; orthogonal to Phases 1–2. Phase 4 is operator-driven verification. No forward dependencies.

4. **Test scope sanity.**
   - `wf_schema()` exists at `next_action.rs:190`, `insert_wf_row` at `next_action.rs:240`. AC4.1 tests start at line 299. Confirmed.
   - `MockRunner`, `drive_loop`, and `insert_task` are real in `drive.rs`. The `~line 1080+` placement claim is correct (existing `terminal_complete_exits_without_spawning` at line 1090, `terminal_blocked_exits_zero` at 1108, `structured_output_takes_precedence_over_final_message` at 1134). New tests slot in cleanly.
   - `format_task_line` is at `status.rs:149` and `insert_task` (the helper, takes `Option<&str>` for `blocked_reason`) is at `status.rs:419`. The plan's "~line 474" is one of the existing blocked tests; new tests will sit nearby. Confirmed.

5. **Decision Matrix soundness.**
   - **Empty-string `blocked_reason` interpretation (status-only).** Sound. `next_action.rs:97` is already on this view; the DB historically writes `""`; the choice aligns the divergent reader (`status.rs:160`) to the existing correct one. No regression risk for `status="blocked"` rows with `blocked_reason=""` because the predicate is purely on `status`, so those still report `blocked=true`. The existing `next_action_blocked_returns_null_agent` test (line 333) already covers that case and stays green.
   - **Role-mismatch enforcement at `parse_envelope`.** Sound but with a Layer-2 caveat (see R1). The plan correctly identifies that `dispatch_submit`'s status checks (lines 634, 655, 684, 710) only run *after* deserialise, so they cannot catch unknown variants. Layer 1 (SDK `structured_output`) does NOT bypass the check as long as the role-peek is added at the entry of `parse_envelope` before any layer's deserialise. The plan says "before the existing serde deserialise calls in Layers 1/2/3" — that's the correct position.

6. **Anti-patterns.** Clean. No speculative refactors (e.g. the plan resists renaming `parse_envelope`). No "while we're at it" creep into the v0.4 POC items — they're explicitly listed Out of Scope. Phase 3's manual AC ("verified during Phase 4 smoke; recorded in the Phase 3 executor log as a one-line note") does carry the captured artefact (`drive-smoke.log`) per Decision Matrix row 6, satisfying the "no manual without artefact" rule. No tests assert on log formatting (Phase 2 asserts substrings on the `Err` message — that's a contract assertion, fine).

7. **Implementation risk (Phase 2 signature change blast radius).** `parse_envelope` has 11 call sites: 1 production (line 477) + 10 in tests (lines 1152, 1217, 1232, 1239, 1246, 1253, 1303, 1336, 1353, 1375). Each takes a single role string today; after the change each needs a second role string. For the existing tests, both arguments will be the same value (e.g. `"planner"` twice). Mechanical, but see M1 about whether a second parameter is even needed.

### Issues found (M = minor, R = risk)

- **M1 — Redundant signature (cosmetic).** Plan adds `expected_role: &str` alongside the existing `agent_role_normalized: &str`. At every call site they will be identical (`agent_name_normalized` is fed to both). Consider collapsing to a single parameter and using it for both SAP role-injection and the new role-peek check. Saves 11 test-site edits and one parameter. Not blocking — execution-time judgement.

- **M2 — Layer-2 role-peek ordering ambiguity.** Plan says "peek the `role` field on the candidate `Value`" without specifying pre- or post-`or_insert_with` (drive.rs:571–573). To catch the literal Bug 2 payload `{"role":"guide","action":"noop"}`, the peek must run on the raw extracted candidate **before** role injection (otherwise SAP's `or_insert_with` would overwrite a missing role with `expected_role` and the peek would always pass). For the failing payload this happens to be moot because the role is present, but document the ordering in the executor's diff to avoid a regression later. Not blocking.

- **M3 — Phase 1 AC typo.** Plan line 94 reads "asserting `blocked=false` for the first two and `blocked=false` for the third". The third is also `false` (status-only predicate when `status="executing"`), so the assertion is correct, but the duplication reads as a copy-paste error. Reword to "all three" during execution.

- **M4 — `insert_wf_row` does not currently accept `blocked_reason`.** The helper at `next_action.rs:240` defaults non-reserved fields to `Null`. To exercise `blocked_reason ∈ {"", "real reason"}` in Phase 1's `next_action_blocked_reason_shapes` test, the executor must either (a) extend `insert_wf_row` with an optional `blocked_reason` arg or (b) follow the insert with an `UPDATE … SET blocked_reason = ?` (the existing `terminal_blocked_exits_zero` test in drive.rs uses pattern (b) at lines 1118–1121). The plan does not call this out. Either path is small; flag for the executor.

- **R1 — Layer 2 role injection vs. role-peek.** Tied to M2. Risk only if the executor places the peek *after* SAP's `or_insert_with`, which would let any role-less candidate through unchecked. Mitigation: peek runs at the top of `parse_envelope`, before any layer body. Phase 2's two new tests will catch a regression here only if at least one fixture has the role tag present and mismatched (the planned `{"role":"guide", ...}` test does). A second test variant with an absent role and a role-less candidate would harden the contract — recommended but not required.

- **R2 — `parse_envelope` Layer 1 (SDK) and the role-peek are both authoritative now.** With the role-peek added, an SDK-validated `structured_output` whose `role` field disagrees with `expected_role` will be rejected before Layer 1's deserialise runs. That is the desired behaviour. There is no path where Layer 1 silently passes through a wrong role: `structured_output` is opaque JSON until deserialised, and the role-peek inspects the same `Value`. Confirmed safe.

### Rationale

The plan is tightly scoped to the three forensic bugs, each phase is independently committable, and every DONE_WHEN clause has a corresponding acceptance criterion. The Decision Matrix records the only six choices that needed deciding, and the chosen options are the smaller-diff / smaller-blast-radius branches in every case. Test scaffolds named in the plan (`wf_schema`, `insert_wf_row`, `MockRunner`, `format_task_line`, `insert_task`, `drive_loop`) all exist at or near the line offsets the planner cited. The Layer-2 ordering and the redundant-signature points are easy execution-time fixes, not planner-time blockers. There are no scope-creep moves and no manual-only acceptance criteria without captured artefacts. Gate: READY.

> Details inline above; no separate plan-review.md needed.

---

## Execution Log

### Phase 1 — Shared `is_blocked()` helper; reporters agree on `blocked`

- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Completed:** 2026-04-30
- **Commit:** `6aa6e12`
- **Files modified:**
  - `src/handlers/mod.rs` — added `pub fn is_blocked(status: &str, _blocked_reason: Option<&str>) -> bool` with doc-comment naming Bug 1 and the canonical interpretation.
  - `src/handlers/status.rs` — replaced `task.blocked_reason.is_some() || task.status == "blocked"` (line 160) with `crate::handlers::is_blocked(&task.status, task.blocked_reason.as_deref())`. Added 4 new tests: `blocked_helper_null_reason`, `blocked_helper_empty_reason`, `blocked_helper_real_reason`, `blocked_helper_status_blocked_all_reasons`.
  - `src/handlers/next_action.rs` — replaced `let is_blocked = status == "blocked"` (line 97) with `crate::handlers::is_blocked(&status, blocked_reason.as_str())`. Added `next_action_blocked_reason_shapes` test.
- **Notes:**
  - `wf_schema()` in next_action tests does not declare `blocked_reason` as a field so DDL omits it. Used `ALTER TABLE … ADD COLUMN blocked_reason TEXT` followed by UPDATE-after-INSERT pattern (same as drive.rs:1118) to exercise `""` and `"real reason"` shapes.
  - `_blocked_reason` parameter is intentionally unused (predicate is status-only per Decision Matrix). The parameter is preserved in the signature for forward-compatibility and to make the canonical interpretation explicit at every call site.
  - `cargo test --all`: 373 unit + 2 integration tests pass, 0 failures.

---

## Code Review Log

### Phase 1 — Shared `is_blocked()` helper; reporters agree on `blocked`

- **Gate:** PASS
- **Reviewed commit:** `6aa6e12`
- **Reviewer:** code-reviewer (Opus 4.7 1M)
- **Date:** 2026-04-30
- **Counts:** 0 critical, 0 major, 2 minor
- **Tests run:**
  - `cargo test handlers::status` → 16 passed (including all 4 new helper tests).
  - `cargo test handlers::next_action` → 5 passed (including `next_action_blocked_reason_shapes`).
  - `cargo test --all` → 373 unit + 2 integration = 375 passed, 0 failed.
- **AC verification:**
  - [x] Status handler: 4 new helper tests present and passing (`blocked_helper_null_reason`, `blocked_helper_empty_reason`, `blocked_helper_real_reason`, `blocked_helper_status_blocked_all_reasons`).
  - [x] next-action handler: `next_action_blocked_reason_shapes` exercises both `status` shapes × all three `blocked_reason` shapes.
  - [x] `cargo test --all` 375/375 passing.
- **Bug-1 regression-trap proof:** `blocked_helper_empty_reason` is decisive. With `status="executing"` and `blocked_reason=Some("")`, the *old* `status.rs:160` predicate (`blocked_reason.is_some() || status=="blocked"`) returns `true || false = true` → would have produced `blocked=true`. The new test asserts `blocked=false` and passes only because `is_blocked` is status-only. The old code is unambiguously caught.
- **Cross-reporter agreement:** structurally guaranteed because both reporters call the same `is_blocked` helper. There is no co-located test that calls both reporters and asserts equality, but the ACs do not require one. Noted as Minor 1.
- **Out-of-scope check:** `git show 6aa6e12 --stat` reports exactly the 3 expected files (`src/handlers/{mod.rs, next_action.rs, status.rs}`); no drive.rs, no parse_envelope, no stderr changes leaked in.
- **Doc comment quality:** `is_blocked`'s doc comment names Bug 1, references the old buggy predicate verbatim, and states the canonical interpretation (status-only; reason is a description, not the gate). Faithful to the Decision Matrix.
- **Minor 1 (deferred):** No test directly asserts `format_task_line(...)`'s `blocked=` substring agrees with `compute(...).blocked` for the same row. Structural agreement is guaranteed by the shared helper, but a defensive co-located assertion would harden against future divergence (e.g. if either reporter ever inlines the predicate again). Recommend adding when next touching either file; not blocking.
- **Minor 2 (deferred):** `_blocked_reason: Option<&str>` is intentionally unused; the underscore prefix and doc-comment together make this clear, and call sites pass meaningful expressions (`task.blocked_reason.as_deref()`, `blocked_reason.as_str()`) which document intent at the use site. Acceptable; flagged only because future readers may wonder why the parameter exists. Decision Matrix and doc-comment already justify it.
- **Verdict:** PASS. All ACs met; the tests directly catch the original bug; no scope creep; doc-comment is faithful; full suite green. Advance to Phase 2.

### Phase 2 — Drive surfaces unroutable-role envelope as `(expected, received, session_id)` error

- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Completed:** 2026-04-30
- **Commit:** `2acc3b9`
- **Files modified:**
  - `src/handlers/drive.rs` — extended `parse_envelope` body with two nested helper closures (`peek_role`, `check_role_mismatch`) and added role-peek calls before deserialise in each of the three layers (Layer 1 SDK, Layer 2 SAP, Layer 3 Legacy final_message + last-line stdout). Layer 2 peek runs BEFORE `or_insert_with` injection (M2/R1 ordering). Signature unchanged (single `agent_role_normalized` parameter serves as both SAP inject-role and expected-role, per M1 collapse recommendation). Added two new tests: `drive_loop_unroutable_role_exits_nonzero` and `drive_loop_role_mismatch_message_format`.
- **Notes:**
  - M1 (redundant parameter): collapsed to a single parameter — `agent_role_normalized` is used as both the SAP inject-role and the expected-role for the mismatch check. No call-site edits needed (all 10 test call sites already pass the correct role string).
  - M2/R1 (Layer 2 ordering): peek runs on the raw extracted candidate before `or_insert_with`, documented in a code comment.
  - Layer 3 Legacy: added `serde_json::from_str::<serde_json::Value>` pre-parse to get a `Value` for peeking before the `AgentEnvelope` deserialise attempt (both final_message path and last-line stdout path).
  - `cargo test --all`: 377 tests pass (375 + 2 new), 0 failures.
  - `tests/drive_e2e.sh`: both AC7.1 and AC7.1b pass.

---

## Completion
_Final summary when task is complete._
