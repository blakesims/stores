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

### Batch A — Fix current RED and queue protocol foundation

Deliverables:

1. Minimal pure freshness classifier with unit tests.
2. Integration consumes classifier at front-of-queue.
3. Stale/no-fresh cases finalize/release integration locks and record typed `freshness_decision` / `next_action` evidence.
4. Update `git-stale-base-refuses` matrix expectation to “must not integrate, must not wedge, must emit typed decision.”
5. Current-mode row flips RED→GREEN.

Acceptance:

```bash
cargo test -q freshness --bin stores
cargo test -q cli::test --bin stores
stores test matrix --mode current --only git-stale-base-refuses \
  --watch --i-understand-this-mutates-current-repo
# PASS, no real LLM, no wedge
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
