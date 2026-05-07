# T004: L017 — observations close-from-open transition

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T08:49:51Z
- **Last Updated:** 2026-05-03T09:01:40Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —

## Task

First propulsion-visible task: drain the queue. Add the close-from-open edge so observations resolved by other work can land in resolved state without walking the full 4-hop lifecycle. After ship, backfill closes existing already-addressed observations and the user sees fuel flowing through the cylinder for the first time.

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** schema.yaml transition row + validator update; handler for new verb; unit + integration tests; one-shot backfill of already-addressed observations as part of this task.
- **Out:** Field-actor-vs-transition-actor matrix reconciliation (separate, deeper); L022 policy layer (auto_resolve.when predicates) — this task adds the edge, policy decides when to auto-fire it; L018 watcher/event-bus; ntfy hooks; auth-UX patch (L013/L014/L015).

### Done When
(1) New transition open→resolved exists in observations lifecycle, gated actor: ai_autonomous. (2) New verb (close-as-addressed) takes --resolution &lt;task-id|obs-id|commit-sha&gt;, records the reference, moves row to resolved. (3) Tests cover: valid close on task, valid close on commit, rejection when --resolution missing, clear behavior when already-resolved. (4) Backfill pass: close already-addressed observations (L007, L008 by T001; L016 by commit 82501d3; others surfaced during execute). Queue visibly drains.

### Phases

#### Phase 1: Phase 1: Schema transition + handler plumbing
- **Objective:** Add the open→resolved transition and a special-cased close-from-open handler that requires --resolution and writes resolved_at atomically.
- **Tasks:**
  - Task 1.1: Add a new transition row to stores/observations/schema.yaml: from: open, to: resolved, verb: close_as_addressed, actor: ai_autonomous (placed near the existing open→wont_fix and open→investigating rows; comment block explaining intent — first propulsion-visible task: drain queue without 4-hop walk).
  - Task 1.2: Add a &#x60;run_close_as_addressed&#x60; function in src/handlers/transition.rs (mirrors run_reject pattern at lines 28-85): opens a tx, writes resolution + resolved_at into the diff, calls run_in_tx for the lifecycle transition, commits. Format-validate &#x60;resolution&#x60; argument against regex &#x60;^(T\d{3,}|L\d{3,}|[0-9a-f]{7,40})$&#x60; and bail with a clear error citing the three accepted forms if it fails.
  - Task 1.3: In src/cli/dynamic.rs build_store_command (lines ~318-329 where &#x60;reject&#x60;&#x27;s --reason is special-cased), add a parallel arm: when verb &#x3D;&#x3D; &quot;close_as_addressed&quot;, attach a clap-required &#x60;--resolution &lt;REF&gt;&#x60; arg with help text listing the three accepted forms.
  - Task 1.4: In src/cli/dispatch.rs (lines ~225-235 in the generic transition routing block), special-case verb &#x3D;&#x3D; &quot;close_as_addressed&quot; similar to reject: read the required --resolution string and call handlers::transition::run_close_as_addressed instead of run.
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo build&#x60; succeeds.
  - [ ] AC1.2: &#x60;stores observations close_as_addressed --help&#x60; shows a required --resolution flag with help text mentioning task-id, observation-id, and commit-sha.
  - [ ] AC1.3: Running close_as_addressed without --resolution exits non-zero with a clap-style &#x27;required&#x27; error mentioning --resolution.
  - [ ] AC1.4: Running close_as_addressed --resolution garbage on an open row exits non-zero with an error message naming the three accepted forms (T###, L###, commit-sha).
  - [ ] AC1.5: A successful close_as_addressed on an open row sets status&#x3D;&#x27;resolved&#x27;, resolution&#x3D;&lt;value&gt;, and resolved_at to a non-null ISO timestamp.
- **Files:** `stores/observations/schema.yaml`, `src/handlers/transition.rs`, `src/cli/dynamic.rs`, `src/cli/dispatch.rs`
#### Phase 2: Phase 2: Unit + integration tests
- **Objective:** Cover the four DONE_WHEN test cases plus a regression trap for already-resolved.
- **Tasks:**
  - Task 2.1: Add unit tests in src/handlers/transition.rs#tests using the existing OBS_SCHEMA-style minimal schema (extend it to include the open→resolved transition): (a) close_as_addressed_with_task_id_succeeds (writes status&#x3D;resolved, resolution, resolved_at); (b) close_as_addressed_with_commit_sha_succeeds (40-char hex); (c) close_as_addressed_without_resolution_rejected (fails at clap layer — drive the test through dispatch surface or bail directly from run_close_as_addressed if --resolution missing); (d) close_as_addressed_already_resolved_rejected (state-machine error, row unchanged); (e) close_as_addressed_with_invalid_format_rejected (error names accepted forms).
  - Task 2.2: Append a Step 11 to tests/observations_e2e.sh that exercises end-to-end: add L008-equivalent open observation, close_as_addressed with --resolution T001, assert status&#x3D;resolved, resolution&#x3D;T001, resolved_at non-empty; then attempt to close again → expect non-zero exit and unchanged row.
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;cargo test handlers::transition&#x60; passes; the five new test names appear in the output.
  - [ ] AC2.2: &#x60;bash tests/observations_e2e.sh&#x60; passes through the new Step 11 with PASS lines for all four sub-assertions.
  - [ ] AC2.3: No prior test output regresses (tests/observations_e2e.sh still prints all 8 original DONE_WHEN PASS lines).
- **Files:** `src/handlers/transition.rs`, `tests/observations_e2e.sh`
- **Dependencies:** Phase 1 complete
#### Phase 3: Phase 3: Backfill already-addressed observations
- **Objective:** Drain the queue: run close_as_addressed on L007, L008, L016, and any other already-addressed observations surfaced during execute.
- **Tasks:**
  - Task 3.1: Run &#x60;stores observations list --invoker ai_autonomous --status open&#x60; (and other non-resolved states) to enumerate candidates. Cross-reference against task contract knowledge: L007 (resolved by substrate-T001 — investigate-as-ai_autonomous), L008 (resolved by substrate-T001 — token-mediated approved_by), L016 (resolved by commit 82501d3 — hex-encode approval token).
  - Task 3.2: For each already-addressed observation, run &#x60;stores observations close_as_addressed &lt;id&gt; --invoker ai_autonomous --resolution &lt;ref&gt;&#x60;. Use the substrate-T### form for task-resolved, the commit short-sha (≥7 hex) for commit-resolved.
  - Task 3.3: For observations in non-open states (e.g. investigating/confirmed) that are nonetheless already-addressed, halt and report — do NOT add new transitions for those states in this task (out-of-scope; the contract specifies open→resolved only). List them in the wrap as residual queue items.
  - Task 3.4: After backfill, run &#x60;stores observations list --invoker ai_autonomous --status resolved&#x60; and capture the count + display IDs in the wrap-summary; run &#x60;stores observations list --invoker ai_autonomous --status open&#x60; and report any remaining open rows so the queue drain is visible.
- **Acceptance Criteria:**
  - [ ] AC3.1: L007, L008, L016 all show status&#x3D;&#x27;resolved&#x27; and a non-empty resolution field via &#x60;stores observations show &lt;id&gt; --json&#x60;.
  - [ ] AC3.2: L007.resolution and L008.resolution reference the substrate task that addressed them; L016.resolution is a hex commit sha (≥7 chars).
  - [ ] AC3.3: The wrap-summary lists the full set of L-ids closed in this task and any open observations that remain (residual queue).
  - [ ] AC3.4: No observation in a state OTHER than open had close_as_addressed attempted on it (the verb is gated to open→resolved only).
- **Files:** `.stores/db.sqlite`
- **Dependencies:** Phase 1 + Phase 2 complete

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

