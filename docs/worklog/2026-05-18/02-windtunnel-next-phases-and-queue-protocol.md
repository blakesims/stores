# Windtunnel Next Phases And Queue Protocol

**Date:** 2026-05-18
**Type:** note

## Summary

This is the single handoff note for continuing the no-LLM Stores windtunnel after the initial fake traversal matrix MVP. It summarizes what has shipped, names the current RED, spells out the simple queue/freshness protocol, and lays out the remaining phases so we can bulk-complete the windtunnel rather than trickle one phase at a time.

Guiding principle:

> Real implementation, isolated arena by default. The harness fabricates real preconditions; Stores produces real consequences. Fake only runner text generation.

The key next design is not a speculative merge train. It is a **serial integration queue with typed evidence stamps and front-of-queue freshness classification**. That is simple enough to ship, detailed enough to avoid hidden dominoes, and strong enough to support a high-throughput windtunnel.

## Source notes / references

- Main merged plan: `docs/worklog/2026-05-17/05-live-fake-traversal-matrix-merged-plan.md`
- Merged plan review: `docs/worklog/2026-05-17/06-live-fake-traversal-matrix-merged-plan-review.md`
- Freshness classifier options: `docs/worklog/2026-05-18/01-freshness-classifier-options-for-matrix-red.md`
- HTML mockup: `http://zen:6067/reports/20260517-152409-stores-live-fake-matrix-mockup/`
- Freshness options report: `http://zen:6067/reports/20260518-062347-freshness-red-next-steps/`

## Reviewed base plan preserved here

This note does **not** replace the heavily reviewed original plan. It preserves that plan by reference and maps its reviewed phases into the current batch execution plan.

Reviewed source material:

- `docs/worklog/2026-05-17/01-live-fake-traversal-matrix-plan.md` — original four-dimension model: state-machine edges × runner output alphabet × real perturbations × authority events; scenario DSL; synthetic/test authority; proof artifacts; smoke/battlescar/upstream/report phases.
- `docs/worklog/2026-05-17/02-live-fake-traversal-matrix-plan-review.md` — review pressure: raw-SQL audit, fail-closed authority/provenance, artifact cleanup, DSL parse-time consequence-faking rejection, no-LLM leakage tests.
- `docs/worklog/2026-05-17/03-live-fake-traversal-matrix-plan-v2.md` — concrete matrix product: `transition_history` path oracle, stable case IDs, `stores test matrix`, per-case artifact bundle, report shape.
- `docs/worklog/2026-05-17/04-traversal-matrix-plan-v2-review.md` — corrections: active/integration-step path assertions, honest runtime, tier-A/tier-B distinction, catalog gaps, L046 timing, integration-blocked induction.
- `docs/worklog/2026-05-17/05-live-fake-traversal-matrix-merged-plan.md` — canonical merged plan with lab/current mode correction and phased implementation.
- `docs/worklog/2026-05-17/06-live-fake-traversal-matrix-merged-plan-review.md` — merged plan review; passed Phase 0 start.

Original reviewed phase map:

| Original phase | Reviewed intent | Current status / current batch |
|---|---|---|
| Phase 0 — Safety/lab foundation | Remove live raw-SQL shortcuts; fake-mode preflight; no env leakage; test provenance; lab arena. | **Done** via `6b04b53c`. Follow-ups: richer TestAuthority/token semantics and structured artifacts. |
| Phase 1 — DSL/enumeration/expectation | `stores test enumerate`; catalog IDs; forbidden consequence fields; `transition_history` ordered path oracle including active/integration steps. | **Done** via `ed8c9966`. |
| Phase 2 — Lab matrix MVP | `stores test matrix --mode lab`; PASS/FAIL/SKIP/ERROR; artifacts; executable smoke. | **Mostly done** via `283e4ad3` and `b38f3e2a`: 4 lab smoke rows PASS, current-only stale row SKIP in lab. |
| Phase 3 — Current-mode opt-in | Explicit dangerous opt-in; current repo/DB/daemon proof; fake-only assertion. | **Done** via `bc2f1e7f`: current stale row produces real FAIL, not ERROR. |
| Phase 4 — Battlescar expansion | Git/liveness/duplicate-drive/freshness rows; each row PASSes or emits useful RED. | **Next**, split into Batch A (freshness/queue foundation), Batch B (queue rows), Batch C (battlescars). |
| Phase 5 — Observation/intake + human verbs | Observation auto-promote, L046 timing, reject/amend, abandon, close-out-of-band, resume, retry. | Batch D. |
| Phase 6 — Coverage/reporting/CI | HTML/JSON reports, schema coverage, CI mode, prune/list/show artifacts. | Batch E. |

Direction has **not** changed. The only architectural amendment is the queue/freshness protocol below: reviewed branches enter a serial integration queue; authoritative freshness classification/testing happens at queue front; queued-behind items are not eagerly invalidated/re-reviewed when main moves.

Review status:

- Original windtunnel phases were heavily reviewed and remain valid.
- This note adds the queue/freshness amendment and batch execution mapping. That amendment should receive oracle/reviewer pressure before Batch A implementation, but the already-reviewed Phase 0–3 work does not need replanning.

## Work already done

Shipped commits include:

- `6b04b53c test: add fake harness phase zero safeguards`
  - removed live raw-SQL retry-freeze path;
  - added fake env restoration/preflight tests;
  - added minimal lab arena foundation;
  - added current-mode test provenance guard before fake-review acceptance.
- `ed8c9966 test: add fake traversal enumeration`
  - added `stores test enumerate --catalog smoke|full --coverage`;
  - added matrix catalog specs and stable IDs;
  - added DSL validation forbidding consequence-faking outside case-level `expect`;
  - added `VisitedEdge` ordered-subsequence matching over transition history.
- `283e4ad3 test: add fake traversal matrix mvp`
  - added `stores test matrix --mode lab`;
  - added PASS/FAIL/SKIP/ERROR matrix rows;
  - added artifacts under `.stores/test-matrix/<run-id>/`;
  - added intentional RED proof row.
- `bc2f1e7f test: add current-mode fake matrix proof`
  - added `--mode current` with required `--i-understand-this-mutates-current-repo` ack;
  - mapped current-mode `git-stale-base-refuses` to the real current repo/DB/daemon path.
- `b38f3e2a test: execute fake matrix smoke loops`
  - made `T3-pr1`, `T3-cr1`, and `T3-er-tooling` executable lab rows;
  - added durable loop-count assertions for PR/CR loop rows.
- `6d77ef85 docs: update fake matrix freshness plan`
  - updated the main plan and recorded classifier options.

Proven commands:

```bash
cargo test -q cli::test --bin stores
# PASS: 32 tests

stores test matrix --mode lab --catalog smoke --watch
# 4 PASS / 0 FAIL / 1 SKIP / 0 ERROR

stores test matrix --mode current --only git-stale-base-refuses \
  --watch --i-understand-this-mutates-current-repo
# 0 PASS / 1 FAIL / 0 SKIP / 0 ERROR
```

Current RED:

- `git-stale-base-refuses` creates a real task (`T019` in the proof run), fake ER PASS, real main marker commit (`88b7e276 fake-run(T019): stale-base main advance`), then integration parks at `integrating/task_review` with `integration_attempts=null` and an unfinished integrate lock.
- The failure is not merely “wrong block kind.” The failure is **no typed freshness decision and no clean next action**.

## Queue / freshness protocol

### Core rule

> A candidate is reviewed before queue entry, but tested and freshness-classified when it reaches the front of the integration queue. Main movement behind the front does not trigger immediate rework; it only marks queued candidates as needing classification at their turn.

This avoids domino storms while still accounting for all details.

### Evidence stamps

Each candidate should carry stamps tied to a specific `(base_main_sha, candidate_head_sha)` pair.

Review stamp:

```json
{
  "kind": "external_review",
  "base_sha": "A",
  "head_sha": "X2",
  "scope": ["src/foo.rs"],
  "verdict": "PASS",
  "review_id": "ER123"
}
```

Test stamp:

```json
{
  "kind": "pre_land",
  "base_sha": "B",
  "head_sha": "Y2",
  "command": "cargo test ...",
  "result": "PASS"
}
```

Integration attempt:

```json
{
  "attempt": 1,
  "started_main_sha": "B",
  "original_review_base": "A",
  "candidate_head_before": "X2",
  "candidate_head_after_refresh": "Y2",
  "freshness_decision": "RefreshOnly|NeedsReview|Blocked|Fresh",
  "test_result": "PASS|FAIL|SKIP",
  "outcome": "integrated|needs_review|integration_blocked"
}
```

### Simple queue lifecycle

For one item:

```text
1. Candidate reaches final-review-ready.
2. External review runs and produces a review stamp.
3. If ER PASS, candidate enters integration queue.
4. When candidate reaches front of queue:
   a. read current main;
   b. refresh/rebase candidate onto current main;
   c. classify freshness;
   d. run authoritative pre-land test/check if classification permits;
   e. land or route to typed next action.
```

### Four queued items example

Initial:

```text
main = A
Q = [T1, T2, T3, T4]
all reviewed at base A
```

T1 reaches front:

```text
refresh T1 onto A
test T1 on A
merge T1
main = B
```

T2/T3/T4 are not immediately re-reviewed. They become “base potentially stale / classify at front.”

T2 reaches front:

```text
current_main = B
refresh T2 onto B
classify A->B against T2 scope
if safe: test and land
if overlap/risky: NeedsReview
if conflict: Blocked
```

Then T3 is classified against whatever main is when T3 reaches front. There is no eager domino cascade.

### Minimal decisions for first implementation

Keep the first classifier intentionally small:

```rust
enum FreshnessDecision {
    Fresh,
    NeedsReview { reason: String, scope: Vec<String> },
    Blocked { reason: String, scope: Vec<String> },
}
```

Meanings:

- `Fresh` — reviewed head/base still match current reality; continue and land.
- `NeedsReview` — review evidence is stale: main changed in relevant/unknown way or branch head changed after review. Do not land; release/finalize integration lock; expose next action.
- `Blocked` — conflict, unreachable reviewed base/history rewrite, dirty worktree, tooling failure, or other operator-action-required issue.

Do **not** implement `RefreshOnly`, `RetestRequired`, or merge trains in the first fix. Those are valuable later, but the immediate bug is a wedge/unclear decision.

### Later classifier expansion

After the minimal gate is GREEN:

```rust
enum FreshnessDecision {
    Fresh,
    RefreshOnly { scope: Vec<String> },
    RetestRequired { scope: Vec<String> },
    ReReviewRequired { scope: Vec<String> },
    Conflict { paths: Vec<String> },
    StaleBaseHistoryRewrite,
    BranchHeadChanged,
}
```

This enables high-throughput behavior without over-reviewing harmless main movement.

## Bulk-completion roadmap

The goal is to complete the windtunnel enough that it is broadly useful, not just one RED fix.

### Batch completion status as of 2026-05-18

Bulk execution completed after oracle review of this note:

- **Batch A — GREEN.** `7cb88425 fix: record needs-review freshness decisions` maps stale ER/head freshness to typed `NeedsReview`, finalizes integration attempts/locks, and flips current-mode `git-stale-base-refuses` from FAIL to PASS with fake runners only.
- **Batch B — GREEN MVP.** `b6fc52e4 test: add queue windtunnel catalog` adds `queue` catalog with four executable lab rows: two-happy serial queue, overlap NeedsReview, branch-head-changed NeedsReview, and conflict blocked. `stores test matrix --mode lab --catalog queue --watch` produced `4 PASS / 0 FAIL / 0 SKIP / 0 ERROR`.
- **Batch C — useful RED MVP.** `c96c2f93 test: add battlescar windtunnel catalog` adds `battlescars` catalog. Validation produced `5 PASS / 1 FAIL / 2 SKIP / 0 ERROR`: dirty worktree, merge conflict, payload-invalid, nonzero-exit, no-heartbeat PASS; stale external-review head mutation is a useful RED; duplicate-drive and stale/dead marker are explicit SKIP.
- **Batch D — MVP.** `1c58c4ef test: add upstream windtunnel catalog` adds `upstream` catalog. Validation produced `1 PASS / 0 FAIL / 5 SKIP / 0 ERROR`: real `abandon` human-verb row PASS; observation auto-promote and other human verbs are explicit SKIP.
- **Batch E — GREEN MVP.** `48a4293e test: add matrix reporting controls` adds `--report md|json`, `--ci`, `matrix prune --keep-last`, and coverage summaries in reports.

Latest focused validation:

```bash
cargo test -q cli::test --bin stores
# PASS: 37 tests

stores test matrix --mode lab --catalog queue --watch
# PASS: 4 PASS / 0 FAIL / 0 SKIP / 0 ERROR

stores test matrix --mode lab --catalog battlescars --watch
# Useful RED: 5 PASS / 1 FAIL / 2 SKIP / 0 ERROR

stores test matrix --mode lab --catalog upstream --watch
# MVP: 1 PASS / 0 FAIL / 5 SKIP / 0 ERROR

stores test matrix --catalog upstream --only abandon --report json --ci
# PASS and reports index.json

stores test matrix prune --keep-last 9999
# PASS, no removals in validation run
```

Remaining windtunnel gaps after bulk pass:

- Turn Batch C's `stale-external-review-head-mutation` useful RED into GREEN.
- Implement skipped Batch C rows: duplicate-drive refusal and stale/dead current-run marker truth.
- Implement skipped Batch D rows: observation auto-promote, reject/amend, close-out-of-band, resume-blocked, retry-integration.
- Improve artifact bundles with structured stdout/transcript/task/ER/git/transition facts; current reports are useful but still thin.
- Add richer schema-transition coverage/waiver checking beyond aggregate coverage tags.

## Later-phase completion contracts

These phases convert the post-bulk gaps into explicit completion gates. The earlier “MVP GREEN” bar is no longer sufficient: each listed row/feature must be executable, asserted with real substrate consequences, and mechanically checked by the stated command. `SKIP` is not acceptable for these phases unless a phase explicitly introduces a durable waiver row and the done_when count is updated in this note.

### Phase F — Complete Batch C battlescar/liveness truth

Objective: turn the `battlescars` catalog from useful-RED MVP into a complete executable liveness/regression suite for known integration, runner, and drive scars.

Rows/features:

1. `stale-external-review-head-mutation` becomes GREEN with typed `NeedsReview` evidence. Do not retire it in Phase F; if later proven duplicate, that requires a separate documented waiver phase.
2. `duplicate-drive-refusal` becomes executable and proves duplicate/manual drive cannot create double-dispatch or corrupt active run state.
3. `stale-dead-current-run-marker` becomes executable and proves stale/dead run-marker truth is detected and handled deterministically.
4. Existing GREEN rows stay GREEN: dirty worktree refusal, merge conflict blocked, payload invalid, nonzero exit, and no heartbeat.

Done when:

```bash
cargo test -q cli::test --bin stores
stores test matrix --mode lab --catalog battlescars --watch
# Required: 8 PASS / 0 FAIL / 0 SKIP / 0 ERROR
```

Acceptance criteria:

- No real LLM calls.
- No raw SQL writes outside isolated lab seeding helpers; no direct final-outcome SQL faking.
- No harness-only consequence faking: substrate handlers/daemon paths produce the final status/evidence.
- Each row asserts at least one durable evidence source relevant to the scar: task row/status/lifecycle/step, transition history, dispatch lock/run state, integration attempts, and/or git SHA/worktree facts.
- `stale-external-review-head-mutation` asserts `integration_attempts[-1].outcome = "needs_review"`, `freshness_decision = "NeedsReview"`, nonempty `completed_at`, and no unfinished integrate lock.
- `duplicate-drive-refusal` asserts there is no second active drive/dispatch for the same task and records the refusal/held reason.
- `stale-dead-current-run-marker` distinguishes a live current run from a stale/dead marker; it must not blindly delete state.
- Failure mode for each row is PASS/FAIL with evidence, not ERROR or wedge.

Non-goals: no merge train, no broad queue redesign, no real agent/LLM execution, no observation/human-verb expansion.

### Phase G — Complete Batch D upstream and human-verb truth

Objective: turn the `upstream` catalog from one real `abandon` row plus skips into an executable lab suite for observation/intake promotion and human lifecycle verbs.

Rows/features:

1. `obs-auto-promote-happy`: observation contract approval/test authority triggers auto-promote into a linked task.
2. `reject-amend`: reject path and amend/reopen path use real transition handlers and record transition history.
3. `abandon`: keep existing PASS row.
4. `close-out-of-band`: prove close-out-of-band records shipped/manual outcome and does not masquerade as normal integration.
5. `resume-blocked`: blocked task resumes through the real transition path.
6. `retry-integration`: integration-blocked task retries through the real integration retry path.

Done when:

```bash
cargo test -q cli::test --bin stores
stores test matrix --mode lab --catalog upstream --watch
# Required: 6 PASS / 0 FAIL / 0 SKIP / 0 ERROR
```

Acceptance criteria:

- Uses real transition handlers/subscribers, not direct final-state edits.
- Actor/authority expectations are asserted by tier:
  - `reject`, `abandon`, and `close-out-of-band` are human-tier rows and must show successful authorized lab/test-human execution plus transition-history invoker evidence.
  - `amend`, `resume`, and `retry-integration` are ai-with-human-tier rows and must show successful authorized lab/test-ai-with-human execution plus transition-history invoker evidence.
  - At least one unauthorized/fail-closed check per tier/class must be exercised somewhere in the upstream catalog or a companion unit test.
- `obs-auto-promote-happy` proves observation → approved contract/test authority → task promotion with durable link/evidence.
- Human verb rows assert final task status/lifecycle/step, transition history event, reason/comment fields where applicable, and no unrelated task mutation.
- `retry-integration` asserts a real integration retry edge and resulting queue/block/integration behavior.
- No real LLM calls.

Non-goals: no production human-token UX redesign, no new observation lifecycle architecture, no current-mode mutation requirement unless a row explicitly needs it.

### Phase H — Artifact bundle and report hardening

Objective: make matrix failures self-diagnosing by bundling structured substrate/git/runner evidence, not just a summary line.

Features:

- Every executable case artifact directory has an `artifact_manifest.json` listing required files, generated files, and explicit `not_applicable` reasons.
- Required/conditional files include: `summary.json`, `task.json`, `transition_history.json`, `dispatch_locks.json`, `agent_runs.json`, `external_reviews.json`, `integration_attempts.json`, `git.json`, bounded `stdout.txt`/`stderr.txt` or `transcript.txt`, and `no_llm.json`.
- JSON/Markdown reports link to per-case artifacts and include verdict counts, per-case expected vs actual, artifact paths, and coverage tags.
- Add a mechanical artifact validator used by tests or by `stores test matrix` itself; replace spot checks with machine checks for every executable row.

Done when:

```bash
cargo test -q cli::test --bin stores
stores test matrix --mode lab --catalog smoke --report json --watch
stores test matrix --mode lab --catalog battlescars --report json --watch
stores test matrix --mode lab --catalog upstream --report json --watch
# Required: artifact validator passes for every PASS/FAIL executable row in these runs.
```

Acceptance criteria:

- Artifacts are generated for FAIL as well as PASS.
- Missing conditional files must be explained in `artifact_manifest.json` with a bounded not-applicable reason.
- Artifact capture must not hide or mutate the underlying outcome.
- Prune keeps/removes only matrix artifact directories, not live substrate data.

Non-goals: no polished HTML redesign, no long-term retention policy beyond prune controls, no unbounded full trace capture.

### Phase I — Schema transition coverage and waiver gate

Objective: make coverage precise enough to show which schema transitions are covered, uncovered, or intentionally waived.

Features:

- Expand `full` catalog semantics or add an equivalent aggregate so the full coverage gate includes smoke, queue, battlescars, and upstream rows. The implementation must not silently mean only the old smoke+extra catalog.
- `stores test enumerate --catalog full --coverage` reports covered transitions, uncovered transitions, waived transitions with rationale, and catalog rows responsible for each covered transition.
- Matrix reports include the same coverage summary.
- Enhance `--ci` or add an explicit stricter flag so CI fails mechanically on unexpected FAIL/ERROR, accidental SKIP, and unwaived required transition gaps.

Done when:

```bash
cargo test -q cli::test --bin stores
stores test enumerate --catalog full --coverage
stores test matrix --mode lab --catalog full --ci --report json
# Required: 0 unexpected FAIL, 0 ERROR, 0 accidental SKIP, 0 unwaived required transition gaps.
```

Acceptance criteria:

- Coverage is tied to real asserted transition/history paths, not only row labels.
- Waivers are explicit and durable: transition id/name, reason, owner/context, and temporary/permanent classification.
- CI distinguishes expected/useful RED only if explicitly waived, accidental FAIL, unsupported SKIP, and infrastructure ERROR.
- Existing smoke, queue, battlescars, and upstream catalogs remain runnable individually.

Non-goals: no requirement to cover every theoretical transition if the waiver is explicit and justified; no schema redesign unless coverage extraction exposes a blocking model gap.

### Phase J — Full windtunnel completion gate

Objective: declare the no-LLM windtunnel complete for the current plan.

Done when:

```bash
cargo test -q cli::test --bin stores
stores test matrix --mode lab --catalog smoke --ci --report json
stores test matrix --mode lab --catalog queue --ci --report json
stores test matrix --mode lab --catalog battlescars --ci --report json
stores test matrix --mode lab --catalog upstream --ci --report json
stores test matrix --mode lab --catalog full --ci --report json
stores test enumerate --catalog full --coverage
stores test matrix prune --keep-last 20
stores test matrix --mode current --only git-stale-base-refuses \
  --watch --i-understand-this-mutates-current-repo
```

Acceptance criteria:

- No real LLM calls in any matrix validation.
- All intended rows PASS or are explicitly waived; no accidental SKIP.
- No ERROR.
- Artifact bundles exist for every executable row.
- Coverage report has no unwaived required gaps.
- Current-mode stale-base proof remains GREEN, emits typed `NeedsReview`, finalizes locks, and does not wedge.
- Current-mode run artifacts document task id, marker commit, DB backup, no-LLM proof, and cleanup/deactivation instructions.
- Working tree is clean after validation except intentional current-mode marker commits/artifacts documented by the run.

Non-goals: no merge train, no eager re-review of queued-behind tasks, no broad product UX polish beyond diagnostic reports/artifacts.

Ordering: Phase F, then G, then H, then I, then J. If a phase reveals a real product bug, keep the row as FAIL/useful RED only while fixing that same phase; do not advance to the next phase until the phase done_when is met or a documented waiver is added by a later explicit waiver gate.

### Batch A — Fix current RED and queue protocol foundation

Batch A implementation contract after oracle review:

- **No new task state.** Minimal `NeedsReview` maps to the existing `integration_blocked` state via `mark_integration_blocked` / existing integration-blocking machinery.
- **Typed decision storage:** canonical per-attempt evidence lives in `tasks.integration_attempts[-1]`; operator-facing summary is copied into `tasks.integration_blocked_reason`.
- **Minimal conservative classifier only:** no `RefreshOnly`, no `RetestRequired`, no auto-review in Batch A.
- **Full serial queue assumption for now:** only one row is actively `integrating`; queued-behind rows remain `integration_queued` and are classified only after acquiring the integration slot.
- **Lock/postcondition finalization is part of correctness:** a row is not GREEN if it still has an unfinished integrate lock, null/unfinished integration attempt, or `integrating/task_review` limbo with no next action.

Minimal Batch A classifier decisions:

```text
reviewed base unreachable     -> Blocked(stale_base_history_rewrite)
refresh/rebase conflict       -> Blocked(conflict)
reviewed head != current head -> NeedsReview(branch_head_changed/stale_external_review)
main moved after review       -> NeedsReview(main_moved_after_review)
else                          -> Fresh
```

`NeedsReview` target shape for Batch A:

```text
status = integration_blocked
lifecycle = integration
integration_step = none
blocked = true
blocker_kind = main_red
integration_blocked_reason contains "needs_review" or "stale_external_review"
integration_attempts[-1].outcome = "needs_review"
integration_attempts[-1].freshness_decision = "NeedsReview"
integration_attempts[-1].freshness_reason = <typed reason>
integration_attempts[-1].next_action = "request_fresh_review"
integration_attempts[-1].completed_at nonempty
integrate/dispatch lock finalized
no_real_llm = true
```

Deliverables:

1. Minimal pure freshness classifier with unit tests using only existing/easy inputs: reviewed base/head, candidate head before/after refresh, current main, base reachability, refresh result.
2. Integration consumes classifier at front-of-queue.
3. Stale/no-fresh cases finalize/release integration locks and record typed `freshness_decision` / `next_action` evidence.
4. Update `git-stale-base-refuses` matrix expectation to “must not integrate, must not wedge, must emit typed `NeedsReview` decision.”
5. Current-mode row flips RED→GREEN.

Acceptance:

```bash
cargo test -q freshness --bin stores
cargo test -q cli::test --bin stores
stores test matrix --mode current --only git-stale-base-refuses \
  --watch --i-understand-this-mutates-current-repo
# PASS, no real LLM, no wedge, typed NeedsReview evidence
```

### Batch B — Queue matrix rows

Deliverables:

Add matrix rows that prove no eager domino behavior:

1. `queue-two-happy` — T1 lands, T2 classifies at front and lands.
2. `queue-disjoint-refresh` — T1 changes disjoint scope, T2 is not re-reviewed unnecessarily.
3. `queue-overlap-needs-review` — T1 changes overlapping scope, T2 gets typed `NeedsReview` and does not land.
4. `queue-branch-head-changed` — candidate branch mutates after review, typed `NeedsReview`.
5. `queue-conflict-blocked` — front-of-queue refresh conflicts, typed `Blocked`.

Acceptance:

```bash
stores test matrix --mode lab --catalog queue --watch
# rows produce PASS/FAIL/SKIP/ERROR matrix with real git/db/fake-runner artifacts
```

### Batch C — Battlescar/liveness expansion

Deliverables:

Add executable rows for:

- dirty worktree refusal;
- merge conflict;
- stale external-review head mutation;
- duplicate drive refusal;
- no heartbeat;
- nonzero exit;
- invalid payload;
- stale/dead current-run marker truth.

Acceptance:

```bash
stores test matrix --mode lab --catalog battlescars --watch
# no real LLM; each row PASSes or produces a useful RED, not ERROR
```

### Batch D — Observation/intake and human verbs

Deliverables:

- `obs-auto-promote-happy`: observation → contract → ratify/test-authority → auto-promote → task drive.
- Prove L046 timing under `agents run --once` loop or run a bounded real daemon window.
- Human verbs: reject/amend, abandon, close-out-of-band, resume blocked, retry integration.

Acceptance:

```bash
stores test matrix --mode lab --catalog upstream --watch
```

### Batch E — Reporting/coverage/productization

Deliverables:

- Structured artifact bundles include task, ER, transition history, integration attempts, git SHAs, runner rows, and stdout transcript.
- HTML/JSON reports.
- `--ci` mode with nonzero exit on unexpected FAIL/ERROR.
- `stores test matrix prune --keep-last N`.
- Schema transition coverage report with covered/uncovered/waived transitions.

Acceptance:

```bash
stores test enumerate --catalog full --coverage
stores test matrix --mode lab --catalog smoke --report html
stores test matrix --mode lab --catalog full --ci --report json
```

## Recommended execution strategy

Use worker → reviewer chains in batches, not tiny phases:

1. **Batch A chain:** classifier + integration wiring + current RED GREEN.
2. **Batch B chain:** queue rows/protocol.
3. **Batch C chain:** battlescars.
4. **Batch D chain:** observation/intake + human verbs.
5. **Batch E chain:** reporting/coverage/prune/CI.

Each chain should:

- start by adding/updating matrix rows;
- run rows RED where appropriate;
- implement substrate/harness fixes;
- re-run GREEN or document a real RED;
- be reviewed before commit.

## Non-goals for now

- No speculative merge train yet.
- No eager rebase/re-review of every queued item whenever main changes.
- No hidden auto-review until typed `NeedsReview` is stable and visible.
- No real LLM calls in matrix validation.

## Follow-ups

- Decide exact field/storage location for `freshness_decision` and `next_action`: new task fields vs integration_attempts JSON only. Keep first version minimal; integration_attempts JSON may be enough if `stores watch` can display it later.
- Capture current-mode stdout proof into artifact bundles.
- Clean or explicitly retain `T019` and marker commit as current RED proof.
