# Live Fake Traversal Matrix Merged Plan

**Date:** 2026-05-17
**Type:** note

## Summary

Build `stores test matrix` as a no-LLM Stores wind tunnel. The only substituted component is nondeterministic runner text generation, replaced by deterministic fake-runner subprocesses. The daemon, database, schemas, validators, subscribers, transition history, external-review rows, worktrees, branches, commits, merge/refusal behavior, dispatch locks, markers, and telemetry remain real.

Folded correction from the latest discussion: **real implementation does not require mutating the operator's active checkout by default.** The default mode should be an isolated real arena:

> Real implementation, isolated arena.

So the matrix has two live modes:

- `--mode lab` — default. Create an isolated real git repo + real `.stores/db.sqlite` + real daemon path under `.stores/test-labs/<run-id>/`, then run the matrix there. This preserves real side effects without polluting the active repo.
- `--mode current` — explicit opt-in. Run against the current checkout and current `.stores/db.sqlite` for bugs that only reproduce in the operating repo.

The guiding rule remains:

> The harness fabricates real preconditions; Stores produces real consequences.

A matrix row is not allowed to fake a target consequence such as `stale_base`, `integration_blocked`, `blocked`, or `integrated`. It may only configure fake-runner outputs and real setup/perturbation actions, then assert the real consequences Stores produced.

## Inherited inputs

This merged plan combines:

- `01-live-fake-traversal-matrix-plan.md` — strongest architectural framing: four-dimension enumeration, no-consequence-faking DSL, synthetic authority/provenance safety, early `stores test enumerate`.
- `02-live-fake-traversal-matrix-plan-review.md` — strongest safety pressure: raw-SQL live-path audit, fail-closed test authority, cleanup/isolation, negative fake-mode leakage tests.
- `03-live-fake-traversal-matrix-plan-v2.md` — strongest concrete product shape: stable case catalog, `transition_history` path oracle, `stores test matrix`, artifact bundles, report output.
- `04-traversal-matrix-plan-v2-review.md` — strongest feasibility/correctness pressure: `active_step`/`integration_step` path assertions, tier-A/tier-B authority correction, missing schema edges, honest runtime, L046 timing, integration-blocked induction.
- Latest design correction — default to isolated **lab mode** while preserving a dangerous/current-repo opt-in.

## Goals

1. Run high-fidelity Stores traversal tests with **zero real LLM calls**.
2. Exercise real Stores machinery below the runner boundary.
3. Produce a strict PASS/FAIL matrix grounded in `transition_history` and final observable DB/git/runner artifacts.
4. Make RED rows useful: a RED is a genuine substrate behavior mismatch, not a harness error or mocked outcome.
5. Support TDD workflow: add/select row → run RED → fix substrate → run GREEN.
6. Keep active repo pollution avoidable through `--mode lab` default.
7. Preserve authority doctrine: test provenance prevents accidental production-row operations; underlying verbs still use correct tier-A/tier-B actor semantics.

## Non-goals

- Not a simulator of Stores' lifecycle.
- Not a mock integration lane.
- Not a replacement for normal unit tests.
- Not proof that real LLM agents make good judgments.
- Not a default pre-commit test for every code change.
- Not permission to raw-SQL-write live outcomes. Reads for proof are fine; writes go through Stores verbs/handlers or are confined to isolated unit fixtures that do not claim live-mode fidelity.

## Core model

A case is a composition of four dimensions:

1. **State-machine edges** — schema transitions, subscriber edges, integration substeps, external-review freshness, liveness/watchdog paths.
2. **Runner output alphabet** — deterministic fake outputs per role: planner, plan-reviewer, executor, code-reviewer, wrap, external-review.
3. **Real perturbations** — real actions such as advancing main with a marker commit, dirtying a worktree, creating a merge conflict, killing/stalling a fake subprocess, attempting duplicate drive.
4. **Authority events** — real verbs such as ratify observation, accept, reject, resume, amend, retry-integration, abandon, close-out-of-band.

Cases must use stable coverage IDs from day one:

```yaml
coverage:
  schema_edges:
    - tasks:planning:submit-plan:plan_review
  runner_outcomes:
    - planner:valid_plan_3_phase
    - plan_reviewer:NEEDS_WORK
  perturbations:
    - git:advance_main_after_er_pass
  authority_events:
    - task:accept
```

## Modes

### `--mode lab` default

`stores test matrix --mode lab` creates a disposable-but-real arena:

```text
.stores/test-labs/<run-id>/
  repo/                         # real git repo/clone
    .stores/db.sqlite            # real Stores DB for the lab
    fake-runner-markers/         # real committed marker files
  worktrees/                     # real git worktrees if needed
  test-matrix/                   # proof bundles and reports
```

Lab mode must run the same Stores binary/daemon/fake-runner seams as current mode. It may seed a minimal Stores substrate, but after seeding it must use normal verbs/subscribers/daemon ticks for lifecycle movement.

Lab mode validates realness for daemon, SQLite, schemas, validators, subscribers, fake-runner subprocesses, dispatch locks, worktrees, commits, merge conflicts, ER rows, integration attempts, transition history, and no-real-LLM assertions.

### `--mode current` explicit opt-in

`stores test matrix --mode current` uses the active repo and active `.stores/db.sqlite`. It is for bugs involving current backlog, live daemon residue, active checkout dirtiness, or production-substrate history.

It should require a loud flag once implemented, e.g.:

```bash
stores test matrix --mode current --only git-stale-base-refuses --i-understand-this-mutates-current-repo
```

Current mode creates real test-owned artifacts in the active repo and must print every row, worktree, branch, commit, ER id, and proof bundle.

## DSL guardrails

Case YAML may describe setup actions, runner outputs, perturbations, authority events, and expectations. It may not prescribe target consequences outside `expect`.

Forbidden outside `expect`:

- `final_status`
- `force_status`
- `external_review_status`
- `integration_result`
- `blocked_reason`
- `stale_base`
- `stale_external_review`
- any direct target lifecycle/status field used as setup

Allowed shape:

```yaml
id: T3-pr1
suite: smoke
entrypoint: synthetic-task
mode: lab
coverage:
  schema_edges: [...]
  runner_outcomes: [...]
  perturbations: []
  authority_events: [task:accept]
runner_script:
  planner: { attempts: [valid_plan_3_phase, valid_plan_3_phase] }
  plan_reviewer: { attempts: [NEEDS_WORK, READY] }
  executor: { attempts: [marker_commit] }
  code_reviewer: { attempts: [PASS] }
  wrap: { attempts: [PASS] }
  external_review: { attempts: [PASS] }
perturbations: []
authority:
  events:
    - accept_task
expect:
  terminal:
    status: integrated
    lifecycle: done
  visited:
    - { from_status: planning, to_status: plan_review, verb: submit-plan }
    - { from_status: plan_review, to_status: planning, verb: submit-plan-review }
    - { from_status: plan_review, to_status: ready, verb: submit-plan-review }
  cycles:
    plan_review: 2
  no_real_llm: true
```

## Expectation oracle

`transition_history` is the path oracle. Terminal state alone is insufficient.

`visited` entries are ordered subsequences. Omitted `visited` means the visited check is skipped; never auto-derive a path silently.

Visited entries must be able to match:

- `from_status` / `to_status`
- `lifecycle_from` / `lifecycle_to`
- `active_step_from` / `active_step_to`
- `integration_step_from` / `integration_step_to`
- `verb`
- `invoker` when relevant

This is required for integration self-loop/substep cases such as freshness refusal, dirty worktree, merge conflict, and clean integration substeps.

A row is RED if any asserted dimension mismatches:

- terminal state/lifecycle/blocker mismatch;
- visited path missing or out of order;
- cycle count mismatch;
- external-review status/runner mismatch;
- integration refusal/substep mismatch;
- liveness/blocker mismatch;
- no-real-LLM assertion fails;
- authority event targeted a non-test row or used wrong tier semantics.

Harness errors are distinct from RED: setup/preflight failures should be reported as `ERROR`, not as substrate behavior failures.

## Authority model

Use **test provenance plus real actor semantics**.

1. Every test-created row gets test provenance: `test_run_id`, `test_case_id`, `synthetic=true` where first-class fields exist; otherwise loud title/body/metadata markers until fields exist.
2. Test authority refuses to act on rows that do not belong to the current test run/case.
3. After provenance passes, commands use the correct schema actor path:
   - Tier-A/token required: `accept`, `reject`, `abandon`, `close-out-of-band`, observation contract approval.
   - Tier-B/no token required: `resume`, `amend`, `retry-integration`, fixture `tasks add`.
4. The harness may present the approve token uniformly for convenience, but documentation must not imply tier-B requires it.
5. Fixture task creation is not a U-moment. It is test-owned tier-B setup.
6. Every authority event is logged in the proof bundle.

Lab mode can use a lab-local approval token; current mode should use the host approval token or a documented explicit test authority path that still fails closed by provenance.

## Commands

Initial command surface:

```bash
stores test enumerate --catalog smoke
stores test enumerate --catalog full --coverage

stores test matrix --mode lab --only T3-pr1 --watch
stores test matrix --mode lab --catalog smoke --watch
stores test matrix --mode lab --catalog full --report md --continue-on-failure

stores test matrix --mode current --only git-stale-base-refuses \
  --i-understand-this-mutates-current-repo --watch
```

Later:

```bash
stores test matrix --report html
stores test matrix --ci --report json
stores test matrix prune --keep-last 5
stores test artifacts list
stores test artifacts show <run-id> <case-id>
```

## Artifact bundle

Per case:

```text
.stores/test-matrix/<run-id>/<case-id>/
  case.yaml
  fake.case.yaml
  transcript.log
  transition_history.json
  task.json
  external_reviews.json
  integration_attempts.json
  dispatch_locks.json
  agent_runs.json
  git-log.txt
  authority-events.json
  proof.txt
  result.json
```

Lab mode stores bundles under the lab directory and may also copy the matrix-level report to the active repo for easy inspection.

## Initial smoke catalog

Keep smoke small enough to run in roughly 30 minutes in lab mode:

1. `T3-hp-with-substeps` — happy path, asserts all integration substeps.
2. `T3-pr1` — plan-review NEEDS_WORK once, recovers.
3. `T3-cr1` — code-review REVISE once, recovers.
4. `T3-er-tooling` — fake external review tooling-held, contained.
5. `git-stale-base-refuses` — real main movement after fake ER PASS produces freshness refusal; this may initially be RED and becomes the first substrate fix target.

Before Phase 2, add must-have catalog rows or waivers for:

- `T3-pr-not-ready` — `plan_review → blocked` via `NOT_READY`.
- `T3-cr-fail` — `code_review → blocked` via hard `FAIL`.
- `T3-er-revise-from-blocked-runner` — ER revise recovery from blocked.
- `T3-hp-delegated-policy` — `complete → integration_queued` delegated-policy path.
- `T2-multi-phase-rejected` — T2 plan-shape enforcement.

## Phased implementation

### Phase 0 — Safety, lab-mode foundation, and plan fidelity

Deliverables:

- Audit `src/cli/test.rs` and related live harness code for raw-SQL writes. Remove live-path writes or confine them to isolated non-live unit fixtures that do not claim matrix fidelity.
- Define test provenance and test authority wrapper.
- Define lab mode arena creation: real git repo, real `.stores`, real daemon path, fake-runner preflight, lab-local artifacts.
- Define marker naming convention:
  - paths: `fake-runner-markers/<run-id>/<case-id>/<fact>.txt`
  - commits: `fake-run(<case-id>): <fact>`
  - branches/worktrees: `stores-test/<run-id>/<case-id>`
- Negative tests:
  - no fake env leakage outside `stores test` child commands;
  - missing/stale fake-runner binary fails before mutation;
  - fake-review acceptance cannot affect non-test rows;
  - test authority refuses non-current-run rows;
  - no real runner appears in `agent_runs` for fake cases.

Exit criteria:

- Phase 0 tests pass.
- No known live-path raw-SQL writes remain in matrix/harness paths.
- Lab arena can be created and inspected, even before full matrix orchestration exists.

### Phase 1 — DSL, enumeration, and expectation engine

Deliverables:

- Matrix module skeleton: `src/cli/test/matrix/{mod.rs,spec.rs,dimensions.rs,enumerate.rs,expect.rs,render.rs,artifacts.rs}` or equivalent.
- Case normalization with forbidden-consequence-field validation.
- `VisitedEdge` expectation checker over `transition_history`, including lifecycle/active_step/integration_step fields.
- `stores test enumerate` command for smoke/full catalogs and coverage IDs.
- Unit tests for:
  - forbidden DSL fields;
  - omitted `visited` skips path check;
  - ordered subsequence matching;
  - integration-step matching;
  - catalog IDs and prune rules.

Exit criteria:

- `stores test enumerate --catalog smoke` works without live mutation.
- Expectation engine tests pass.

### Phase 2 — Lab matrix MVP

Deliverables:

- `stores test matrix --mode lab --catalog smoke` serial runner.
- Per-case strategy dispatch, not one monolithic `LiveHarness::run`.
- Per-case artifact bundle and terminal/Markdown report.
- Per-case teardown that keeps lab/current operator views sane while preserving proof.
- Smoke catalog runs with fake runners only.

Exit criteria:

- A lab smoke run completes with PASS/FAIL rows, not harness errors.
- At least one row may be RED due to real substrate mismatch; that is acceptable and desired if proof is clear.
- No real LLM agent runs are recorded.

### Phase 3 — Current-mode opt-in and authority hardening

Deliverables:

- `--mode current` with explicit dangerous opt-in flag.
- Current-mode provenance checks, authority events, artifact output, cleanup guidance.
- Tier-A/Tier-B docs and tests.
- Host approval token handling for current-mode tier-A events.

Exit criteria:

- Current mode refuses to run without explicit opt-in.
- Current mode can run one selected fake-only case and produce proof.

### Phase 4 — Battlescar expansion

Deliverables:

- Liveness cases: no-heartbeat, nonzero exit, payload invalid, duplicate drive.
- Git cases: dirty worktree, merge conflict, stale external review/head mutation.
- `live-duplicate-drive` uses controlled fake delay so first drive holds dispatch lock before second starts.
- `integration_blocked` recovery reuses merge-conflict/stale setup plus `retry-integration`.

Exit criteria:

- Each battlescar row either PASSes or produces a substrate RED with proof.

### Phase 5 — Observation/intake entry and human-verb catalog

Deliverables:

- Observation auto-promote case: observation → contract → ratify → L046 promotion → task drive.
- Explicit proof of L046 timing under `run --once` loop, or use a real daemon window with deadline.
- Human-verb cases: reject/amend, abandon, close-out-of-band, resume from blocked.
- Backlog/waiver list for per-source abandon/close-out-of-band coverage.

Exit criteria:

- Observation entry proves upstream path in lab mode.
- Human-verb rows run through real verbs and transition history.

### Phase 6 — Coverage, reports, and CI/scheduled use

Deliverables:

- Schema-transition coverage check: every transition covered or explicitly waived.
- HTML and JSON reports.
- `--ci` mode for lab smoke/full as appropriate.
- Prune/list/show artifact commands.
- Runtime documentation: smoke ~30m target, full ~2–3h initially.

Exit criteria:

- Coverage report is actionable.
- Full catalog can run in lab mode without real LLM calls.

## Operating discipline once shipped

For future substrate work:

1. Identify the behavior.
2. Add/select a matrix row.
3. Run `stores test matrix --mode lab --only <case-id>` and capture RED.
4. Fix Stores.
5. Re-run the same row GREEN.
6. Commit case + fix with proof.
7. Use `--mode current` only when the bug depends on active repo/live DB residue.

## Progress as of 2026-05-18

Shipped and reviewed:

- **Phase 0 — safety/lab foundation:** live-path raw SQL retry-freeze was removed, fake env restoration and fake-runner preflight tests were added, minimal lab arena creation exists, and current-mode fake-review acceptance is provenance-guarded before any `tasks accept` call.
- **Phase 1 — DSL/enumeration/expectation:** `stores test enumerate --catalog smoke|full --coverage` exists; case DSL rejects consequence-faking fields outside `$.cases.<case-id>.expect`; `VisitedEdge` supports status/lifecycle/active-step/integration-step/verb/invoker ordered-subsequence matching over `transition_history`.
- **Phase 2 — lab matrix MVP:** `stores test matrix --mode lab` exists, writes artifacts under `.stores/test-matrix/<run-id>/`, and distinguishes PASS/FAIL/SKIP/ERROR. Lab smoke currently runs four executable fake-only rows and skips the current-only stale-freshness row:
  - `T3-hp-with-substeps` PASS;
  - `T3-pr1` PASS, with durable loop-count assertion (`planner=2`, `plan_reviewer=2`);
  - `T3-cr1` PASS, with durable loop-count assertion (`executor=2`, `code_reviewer=2`);
  - `T3-er-tooling` PASS;
  - `git-stale-base-refuses` SKIP in lab mode.
- **Phase 3 — current-mode proof:** `stores test matrix --mode current` requires `--i-understand-this-mutates-current-repo`. The current-mode `git-stale-base-refuses` row ran through the real current repo/DB/daemon path with fake runners only and produced a real matrix FAIL, not ERROR.

Current proof commands:

```bash
cargo test -q cli::test --bin stores
# PASS: 32 tests

stores test matrix --mode lab --catalog smoke --watch
# 4 PASS / 0 FAIL / 1 SKIP / 0 ERROR

stores test matrix --mode current --only git-stale-base-refuses \
  --watch --i-understand-this-mutates-current-repo
# 0 PASS / 1 FAIL / 0 SKIP / 0 ERROR
```

The current RED produced task `T019`, a fake external review PASS, a real fenced main marker commit (`88b7e276 fake-run(T019): stale-base main advance`), then integration parked at `integrating/task_review` with `integration_attempts=null` and an unfinished integrate lock instead of emitting a typed freshness decision/recovery. The matrix proved this as a substrate FAIL while also proving `no_real_llm=ok`.

## Revised next action

Do **not** jump straight to more catalog rows. First fix the stale-freshness semantics exposed by `git-stale-base-refuses`.

The decision is no longer simply “hard block vs auto-review.” The better next design is a typed freshness classifier that distinguishes:

- `Fresh` — land;
- `RefreshOnly` — main moved but reviewed/tested scope is unaffected;
- `RetestRequired` — review remains valid but test/pre-land evidence is stale;
- `ReReviewRequired` — main or branch changes invalidate review evidence;
- `Conflict` — refresh/merge conflict needs resolution;
- `StaleBaseHistoryRewrite` — reviewed base is no longer reachable;
- `BranchHeadChanged` — candidate changed after review.

Next implementation target:

1. Add a pure freshness classifier and tests around base/head/scope/risk inputs.
2. Wire integration so stale freshness produces a typed decision and releases/finalizes locks; no limbo at `integrating/task_review`.
3. Update the matrix expectation for `git-stale-base-refuses` from “must hard-block” to “must not integrate, must not wedge, must emit typed freshness decision/next action.”
4. Re-run current-mode row RED→GREEN.

## Follow-ups

- Track any existing WIP rows/artifacts separately from lab-mode work.
- Keep old plan/review notes intact as source material.
- Improve artifact bundles: current-mode proof is mostly stdout today; copy structured task/ER/git/transition facts into `proof.txt`/`result.json`.
- Add non-mutating tests for the current-mode ack gate.
- After stale-freshness GREEN, continue Phase 4 battlescars: dirty worktree, merge conflict, stale head mutation, duplicate drive, no heartbeat, nonzero exit, payload invalid.
- Phase 5 remains observation/intake entry and human-verb catalog.
- Phase 6 remains coverage/reporting/CI/prune work.
