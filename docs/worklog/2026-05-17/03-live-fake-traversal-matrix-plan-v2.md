# Live Fake Traversal Matrix Plan V2

**Date:** 2026-05-17
**Type:** note

## Summary

Build `stores test matrix` — a traversal-matrix runner on top of the existing live fake-runner harness. The matrix generates ~25 canonical traversals through the task lifecycle (happy / plan-review loops / code-review loops / ER variants / git freshness / liveness / observation entry / human verbs / recovery), runs each through the **real** daemon, **real** DB, **real** worktrees and merges with **only LLM text generation faked**, and emits a pass/fail table grounded in `transition_history`. Path matching is **strict**: both terminal state and the visited-states sequence must match expectation. U-moments (accept / reject / ratify-observation / amend / abandon / retry-integration / resume) are automated via the host-bound `~/.config/stores/approve.token` and `--invoker ai_with_human --approve-token <T>` — same authority path Blake uses, no test-only bypass.

The matrix-as-product is a TDD wind tunnel that turns every substrate bug into "add a row, observe RED, fix, observe GREEN". The current `stale-base-refuses` red (T018 parked at `integration_step=task_review` with `integration_attempts=null` and an unfinished integrate lock) is row 1 in the catalog and the first substrate-task this matrix gates.

## Details

### What already exists (build-on inventory)

- `src/runner/fake.rs` — first-class `FakeRunner` launching the `stores-fake-agent` subprocess; produces valid planner / plan_reviewer / executor / code_reviewer / wrap / external_review payloads through the same boundary as Claude/Codex. Selected via `STORES_LLM_OFF=1` + `STORES_FAKE_AGENT_BIN` + `STORES_FAKE_CASE_FILE`/`STORES_FAKE_CASE_NAME`.
- `src/cli/test.rs` (2168 lines) — `TestCase { tier, executor_mode, stages: {role: {outcome | attempts: [outcome,…]}}, expect }`. `attempts` already encodes retry sequences (`plan_reviewer.attempts=[NEEDS_WORK, READY]` = plan-review reject-then-recover).
- Presets: `happy-path`, `t3-failed-er`, `stale-base-refuses`.
- `LiveHarness` — real `.stores/db.sqlite`, real worktree, real `stores agents run --once`, real ER rows, real `tasks accept`/`enqueue-integration`. `run_stale_base_refuses` is a dedicated branch demonstrating the "fabricate real preconditions, let substrate produce real consequences" pattern.
- `Harness` — synthetic-repo in-process drive_loop for unit-ish parity checks.
- U-moment primitives: `STORES_ALLOW_FAKE_REVIEW_ACCEPT=1` (already wired), host-bound approve token at `~/.config/stores/approve.token` (0600), `--invoker ai_with_human --approve-token <T>` accepted for all tier-A writes.
- `stores/tasks/schema.yaml` (409 lines) — every state, transition, guard, and cycle budget (`plan_review_log.length < 5`, `current_cycle <= 4`) declared as data. Enumerable.
- `transition_history` row-per-edge with `store/display_id/from_status/to_status/verb/invoker/occurred_at/actor_note` — the truth source for visited-states assertions.

### What's missing (build list)

1. No path enumeration — three hand-authored presets.
2. `CaseExpect` collapses to a single final state; no visited-states / cycle-count / refusal-reason / liveness expectations.
3. No matrix runner. `stores test run` is one-case.
4. Liveness / git-failure cases (no-heartbeat, payload-invalid, dirty worktree, merge conflict, duplicate drive) catalogued in `01-live-fake-runner-scenario-tdd-plan.md` but not implemented.
5. Observation-entry path (observation → ratify → L046 auto-promote → drive) not wired as a test entrypoint.
6. Human-verb cases (reject/amend, abandon, closed-out-of-band, retry-integration) not yet harness-driven.
7. `stale-base-refuses` is RED (T018 wedge — task parks at `integration_step=task_review` with `integration_attempts=null` after `mark_refresh_done`, unfinished integrate lock, no typed freshness refusal). The harness produces the red proof correctly; the substrate bug is unfixed.

### Design stance (anchored in 05-13 doctrine)

- **The harness fabricates real preconditions; the substrate produces real consequences.** No mocked outcomes. No raw-SQL writes to force final states. Real worktrees, real commits, real ER rows, real daemon, real DB.
- **Visited-states is the truth column.** `transition_history` is the ground truth, not stage-output sequences. A case passes only if the actual edges fired match the expected ordered subsequence. This catches sneaky path changes (an edge gets skipped, a guard fires that shouldn't) that pure terminal-state matching would miss.
- **Authority is real authority.** U-moments use the same `--invoker ai_with_human --approve-token <T>` path Blake uses. No `--test-mode-bypass-acceptance` flag. The token is read from the host file once per run; absent token = fail loud.
- **Fenced artifacts.** Every test creates marker commits in `fake-runner-markers/<task-id>-<case>/...` with `fake-run(<task-id>): <case> <fact>` messages. Auditable, additive, never `git add .`.
- **Concurrency is later.** Real daemon is a singleton — matrix runs cases serially in v1. Per-case temp-DB / daemonless modes are a separate question.

### The matrix architecture

```
┌────────────────────────────────────────────────────────────────────┐
│ src/cli/test/matrix/                                               │
│   mod.rs           — subcommand entry                              │
│   dimensions.rs    — orthogonal-dim enum + Dimensions struct        │
│   enumerate.rs     — Dimensions -> Vec<TraversalSpec>              │
│   spec.rs          — TraversalSpec { id, tier, stages, expect,…}   │
│   expect.rs        — visited-states checker against transition_h.  │
│   render.rs        — terminal table, markdown, HTML report         │
│   artifacts.rs     — per-case proof bundle persistence             │
│                                                                    │
│ src/cli/test.rs    — extended `CaseExpect` (visited, cycles,       │
│                      integration, liveness)                        │
│ src/runner/fake.rs — scenario additions: no_heartbeat,             │
│                      nonzero_exit (already partial), payload_inv.  │
└────────────────────────────────────────────────────────────────────┘
```

### Extended expectation language

```yaml
expect:
  terminal:
    state: integrated              # or in_review/blocked/rejected/abandoned/integration_blocked/closed_out_of_band
    lifecycle: done                # active/integration/done
    blocker_kind: null             # task_review / runner / main_red when blocked
  visited:                         # ordered subsequence of (from_status -> to_status)
    - { from: planning,    to: plan_review }
    - { from: plan_review, to: planning }      # loop proof
    - { from: plan_review, to: ready }
    - { from: ready,       to: executing }
    - { from: executing,   to: code_review }
    - { from: code_review, to: complete }
    - { from: complete,    to: in_review }
    - { from: in_review,   to: accepted }
    - { from: accepted,    to: integration_queued }
    - { from: integration_queued, to: integrating }
    - { from: integrating, to: integrated }
  cycles:
    plan_review: 2
    code_review: 1
    external_review: 1
  external_review:
    runner: fake
    final_status: passed
  integration:
    refused_reason_contains: stale_external_review   # optional
  liveness:
    runner_payload_error: false
    blocked_runner: false
  no_real_llm: true
```

Strict matching: every entry in `visited` must appear as a row in `transition_history` for this task, in order; gaps allowed (subsequence, not contiguous). Cycle counts are computed from `transition_history` counts of specific edges. The integration / liveness / external_review blocks are pure equality checks against DB state at end-of-run.

### Traversal generator (orthogonal dimensions)

```rust
pub struct Dimensions {
    pub tiers:           Vec<Tier>,        // T1, T2, T3
    pub pr_rejects:      Vec<u8>,          // 0, 1, K-1 (=4), K (=5 = budget exhausted)
    pub cr_revises:      Vec<u8>,          // 0, 1, K-1 (=3), K (=4 = budget exhausted)
    pub er_outcome:      Vec<ErOutcome>,   // Pass, ReviseOnce, ToolingFailure, PayloadInvalid
    pub executor_mode:   Vec<ExecutorMode>,// MarkerFile, ScriptedPatch
    pub git_pressure:    Vec<GitPressure>, // None, MainAdvances, DirtyWorktree, MergeConflict
    pub liveness:        Vec<Liveness>,    // None, NoHeartbeat, NonzeroExit, PayloadInvalid, DuplicateDrive
    pub entry:           Vec<Entry>,       // SyntheticTask, ObservationAutoPromote
    pub human_verb:      Vec<HumanVerb>,   // None, RejectAmend, Abandon, ClosedOutOfBand, RetryIntegration
}

pub fn enumerate(dims: &Dimensions) -> Vec<TraversalSpec> { … }
```

`enumerate` does NOT do full cartesian product — it prunes infeasible combos (e.g. `git_pressure ≠ None` + `liveness ≠ None` simultaneously, `entry = ObservationAutoPromote` + `human_verb = ClosedOutOfBand`), and groups into named **families** so the report stays human-readable. Stable id: `T{tier}-pr{N}-cr{M}-er-{outcome}-int-{pressure}-live-{mode}-entry-{e}-verb-{v}`.

### Initial catalog (~25 rows, full lifecycle)

| Family | ID | Description | Expected terminal | Key visited edges |
|---|---|---|---|---|
| happy | `T1-hp` | T1 skip-plan, marker executor | `integrated/done` | `planning→ready` (skip-plan) |
| happy | `T2-hp` | T2 one-phase plan | `integrated/done` | full chain, one phase |
| happy | `T3-hp` | T3 multi-phase happy | `integrated/done` | full chain |
| plan-review-loop | `T3-pr1` | 1 NEEDS_WORK then READY | `integrated/done` | `plan_review→planning` once |
| plan-review-loop | `T3-pr4` | 4 NEEDS_WORK then READY (under budget) | `integrated/done` | `plan_review→planning` ×4 |
| plan-review-loop | `T3-pr5-budget` | 5 NEEDS_WORK (budget exhausted) | `blocked/active, blocker=task_review` | `plan_review→blocked` |
| code-review-loop | `T3-cr1` | 1 REVISE then PASS | `integrated/done` | `code_review→executing` once |
| code-review-loop | `T3-cr-budget` | budget-exhausted REVISE | `blocked/active, blocker=task_review` | `code_review→blocked` |
| er-loop | `T3-er-revise-pass` | ER REVISE → executor → PASS | `integrated/done` | `in_review→executing` via ER |
| er-loop | `T3-er-tooling` | ER TOOLING_FAILURE | `in_review/active, ER tooling_held` | ER row visible |
| er-loop | `T3-er-payload-invalid` | ER bad JSON | `in_review`, `runner_payload_error` typed | error visible |
| compound | `T3-pr1-cr1-er1` | one of each loop, happy terminal | `integrated/done` | all three loops |
| executor | `T3-hp-scripted-patch` | scripted_patch executor mode | `integrated/done` | full chain |
| git-freshness | `git-stale-base-refuses` | main advances after ER PASS | freshness refusal (**RED today — T018**) | refusal transition expected, not observed |
| git-freshness | `git-dirty-worktree` | uncommitted file before integrate | dirty refusal | dirty-rejected edge |
| git-freshness | `git-merge-conflict` | conflicting commit on main | conflict refusal | conflict-blocked edge |
| liveness | `live-no-heartbeat` | fake exits without final payload | `blocked, blocker=runner` | `*→blocked` via `mark_drive_failed` |
| liveness | `live-nonzero-exit` | fake exit code != 0 | `blocked, blocker=runner` | same |
| liveness | `live-payload-invalid` | malformed final output | `blocked, runner_payload_error visible` | same |
| liveness | `live-duplicate-drive` | second drive while first lock held | first survives, second refused | no duplicate edges in history |
| human-verbs | `T3-reject-amend-integrate` | reject at in_review, amend, re-drive | `integrated/done` | `in_review→rejected→planning→…→integrated` |
| human-verbs | `T3-abandon-planning` | abandon during planning | `abandoned/done` | `planning→abandoned` |
| human-verbs | `T3-closed-out-of-band` | close-out-of-band from in_review | `closed_out_of_band/done` | terminal edge |
| recovery | `T3-integration-blocked-retry` | block during integrate, retry | `integrated/done` | `integration_blocked→integration_queued→integrated` |
| entry | `obs-auto-promote-happy` | observation → ratify → L046 promote → drive | `integrated/done`, obs `resolved` | obs `draft→ready`, then full task chain |

### `stores test matrix` subcommand

```bash
stores test matrix [--catalog smoke|full]
                   [--only <case-id>]
                   [--filter family=X[,Y]]
                   [--tier T1,T2,T3]
                   [--report md|html|json]
                   [--watch]
                   [--ci]
                   [--continue-on-failure]
```

Defaults: `--catalog smoke` (10 rows), `--report md`, serial execution. `--ci` makes exit nonzero on any RED row and emits machine-readable JSON last line. `--continue-on-failure` runs every row even when earlier ones fail (the default in non-ci mode).

### Per-case artifact bundle

```
.stores/test-matrix/<run-id>/<case-id>/
  case.yaml                     # rendered TraversalSpec
  fake.case.yaml                # fake-runner subprocess case file
  transcript.log                # stdout/stderr of stores test run
  transition_history.json       # rows for this display_id
  external_reviews.json
  task.json                     # final task row
  integration_attempts.json
  dispatch_locks.json           # for liveness/duplicate-drive forensics
  agent_runs.json               # all fake; non-fake count must be 0
  git-log.txt                   # commits made by the case
  proof.txt                     # human-readable narration
  result.json                   # { id, expected, observed, verdict, duration_ms, llm_subprocess_count, mismatches: [...] }
```

The `<run-id>` is a timestamp+random suffix; `.stores/test-matrix/<run-id>/index.{md,html,json}` is the matrix-level report linking each row's bundle.

### Report output

Terminal (colorized):

```
Stores Live Fake Traversal Matrix — 2026-05-17  run=run-1747-1430
CASE                            FAMILY              EXPECTED             OBSERVED             VERDICT  DUR
─────────────────────────────────────────────────────────────────────────────────────────────────────
T1-hp                           happy               integrated           integrated           PASS     0m22s
T2-hp                           happy               integrated           integrated           PASS     0m34s
T3-hp                           happy               integrated           integrated           PASS     0m41s
T3-pr1                          plan-review-loop    integrated           integrated           PASS     0m48s
T3-pr5-budget                   plan-review-loop    blocked:task_review  blocked:task_review  PASS     1m05s
T3-cr-budget                    code-review-loop    blocked:task_review  blocked:task_review  PASS     1m11s
T3-er-revise-pass               er-loop             integrated           integrated           PASS     0m55s
git-stale-base-refuses          git-freshness       freshness-refuse     parked-no-attempt    FAIL     2m14s
git-dirty-worktree              git-freshness       dirty-refuse         dirty-refuse         PASS     0m38s
live-no-heartbeat               liveness            blocked:runner       blocked:runner       PASS     2m02s
obs-auto-promote-happy          entry               integrated           integrated           PASS     1m32s
…
Summary: 22 PASS / 3 FAIL / 0 SKIP   real_llm_subprocess_total=0   duration=37m18s
Report: .stores/test-matrix/run-1747-1430/index.html
```

Markdown: same table + per-row collapsible proof excerpts.
HTML: same + per-row deep-link to the artifact bundle, run-over-run sparkline of duration.
JSON: stable schema for downstream tooling.

### Phased build

Each phase is one or two commits. Phases land in order; each leaves the system more useful than before.

**Phase 1 — Schema-driven traversal generator + visited-states expectation language.**
- Add `src/cli/test/matrix/` skeleton (`mod.rs`, `dimensions.rs`, `enumerate.rs`, `spec.rs`).
- Extend `CaseExpect` with `visited`, `cycles`, `integration`, `liveness` fields. Keep backward compat with existing presets (default = old behavior).
- Add transition-history reader (`SELECT from_status, to_status FROM transition_history WHERE store='tasks' AND display_id=? ORDER BY id`) and ordered-subsequence matcher.
- Unit tests in `tests/matrix_generator.rs` (no live DB): given Dimensions, expected set of TraversalSpec ids; given mock transition_history, expected match/mismatch verdicts.
- Tests for the prune rules (no `git_pressure≠None` + `liveness≠None`, etc).
- One commit.

**Phase 2 — `stores test matrix` subcommand (orchestration + smoke catalog).**
- Add CLI surface, artifact persistence, terminal-table renderer.
- Wire the existing `LiveHarness` per-case; `run_stale_base_refuses` becomes one of many cases dispatched by case-id.
- Smoke catalog = 10 rows: `T3-hp`, `T3-pr1`, `T3-pr5-budget`, `T3-cr1`, `T3-er-tooling`, `T3-er-revise-pass`, `git-stale-base-refuses` (expected RED), `live-no-heartbeat`, `T3-reject-amend-integrate`, `obs-auto-promote-happy`.
- Markdown report only in this phase; HTML/JSON defer to Phase 6.
- First real run: `stores test matrix --catalog smoke` produces a real matrix with one RED row (stale-base-refuses) and the rest GREEN, or surfaces other unknown REDs we then triage.
- One commit.

**Phase 3 — U-moment automation hardening.**
- Read approve token at matrix start from `~/.config/stores/approve.token`; refuse to run if missing (`fail loud, no test-mode bypass`).
- Route all tier-A writes through `--invoker ai_with_human --approve-token <T>`: `tasks accept`, `tasks reject`, `tasks abandon`, `tasks resume`, `tasks amend`, `tasks retry-integration`, `tasks close-out-of-band`, `observations update --contract-state ready`.
- Emit `[auth] tier-A writes will be token-grounded (case=<id> token=present)` line per case.
- Existing `STORES_ALLOW_FAKE_REVIEW_ACCEPT=1` stays — that's fake-ER acceptance safety, orthogonal to token-grounding.
- One commit.

**Phase 4 — Liveness + git-failure cases.**
- Extend `stores-fake-agent` scenarios: `no_heartbeat` (start, write partial transcript, exit silently after delay), `nonzero_exit` (exit code 7 with no payload), `payload_invalid` (emit malformed JSON then exit 0). Most of these exist partially — make them first-class case modes.
- `live-duplicate-drive`: harness spawns a second `stores agents run --once` while first is mid-drive; assert second observes `dispatch_locks` live-owner row and refuses.
- `git-dirty-worktree`: after fake ER PASS, write `fake-runner-markers/<id>-dirty/uncommitted.txt` to worktree without staging, then `tasks accept` / `enqueue-integration`; assert dirty refusal.
- `git-merge-conflict`: stage conflicting commits on main and task branch (same marker file, different contents); assert conflict-blocked.
- Each family is a separate commit (4 commits) so each is a real TDD red-green cycle — bug surface, fix substrate where needed, prove green.
- Expected outcome: some of these surface live substrate bugs (TBD — that's the point).

**Phase 5 — Observation-entry + human-verb cases.**
- `obs-auto-promote-happy`: harness invokes `observations add`, drafts intent contract, calls `observations update --contract-state ready --approve-token <T>`, waits ≤10s for L046 auto-promote subscriber to spawn the task, then drives synthetic task to integrated. Asserts observation `status=resolved` linked to task, task `linked_observations` contains the obs id.
- `T3-reject-amend-integrate`: at `in_review`, harness calls `tasks reject --reason "matrix test reject"`, then `tasks amend`, then re-drives. Asserts `rejected→planning→…→integrated` edges.
- `T3-abandon-planning`, `T3-closed-out-of-band`: terminal-history cases.
- `T3-integration-blocked-retry`: induce `integration_blocked` (TBD how — likely via fake ER scenario or main-red simulation), then `tasks retry-integration`. Asserts `integration_blocked→integration_queued→integrated`.
- One commit.

**Phase 6 — HTML report + CI mode.**
- HTML report as a single self-contained file with collapsible per-case proof bundles, deep-link to artifact bundle, run-over-run sparkline of duration vs prior runs (read prior `result.json` files under `.stores/test-matrix/`).
- `--ci` flag: exit nonzero on any RED row; emit machine-readable `results.json` last line on stdout for CI pipelines.
- `--report json` standalone.
- One commit.

### Failure-tolerance / what counts as RED

A row is RED if any of:
- terminal state ≠ expected;
- visited-states ordered subsequence does not match (missing edge, wrong order);
- cycle counts mismatch;
- ER block mismatch;
- integration block mismatch;
- liveness block mismatch;
- `no_real_llm=true` but `agent_runs` shows any non-fake row (= real LLM subprocess fired).

The matrix's verdict is per-row; the summary line aggregates. There is no "soft fail" — every column expectation is a hard assertion. The matrix's value is exactly that strictness.

### Operating discipline once shipped

Future engine work runs as:

1. Identify the substrate behavior under test or under repair.
2. Add (or pick) the matrix row that demonstrates it.
3. Run `stores test matrix --only <case-id>` and capture RED proof.
4. Fix substrate code.
5. Re-run same case; capture GREEN.
6. Commit case + fix as a linked pair (or in same commit if small).

The current `stale-base-refuses` RED becomes the first ticket the matrix gates. Its substrate-side fix (the `task_review_policy=none` task parking at `integration_step=task_review` with `integration_attempts=null` and an unfinished integrate lock after `mark_refresh_done`) is the immediate post-Phase-2 substrate task.

### Risks / open questions

- **Concurrency.** Real daemon is a singleton, so matrix runs serially in v1. 25 rows × ~1 minute avg = ~25 min per full run. Acceptable for v1; bothersome at scale. Daemonless drive mode or per-case temp-DB instances are a future question.
- **`stale-base-refuses` is RED — does that break the matrix?** No. The matrix expects RED on that row until the substrate is fixed; when fixed, expectation flips. RED rows are the matrix doing its job, not the matrix being broken.
- **`integration_blocked` induction.** Producing `integration_blocked` deterministically without simulating real `main_red` may need a new fake-runner scenario (e.g. fake integration check returning blocked). Cost-weighed in Phase 5.
- **Observation auto-promote timing.** L046 subscriber polls; if it's >10s, harness needs a longer wait or an event-driven hook. Phase 5 spike.
- **Catalog drift.** As the lifecycle schema evolves, the catalog must follow. Mitigation: dimension definitions live next to the schema-reader code; a `cargo test` lint compares catalog edges to declared schema transitions.
- **Per-run artifact growth.** 25 rows × ~10 JSON files = ~250 files per run. Add `.stores/test-matrix/` to `.gitignore`. Add `stores test matrix prune --keep-last N`.

### What this is NOT

- Not a unit-test replacement. Existing in-process `Harness` tests stay.
- Not a substitute for the real-world dogfood loop. Real Claude/Codex runs still happen; the matrix proves the substrate's *plumbing* is sound, not that the LLM agents are good.
- Not an excuse to skip filing substrate observations for friction encountered while building the matrix — every friction is data, file an intake/observation.
- Not a concurrency / load test. Serial v1.

### Immediate next step

Phase 1: scaffold `src/cli/test/matrix/`, extend `CaseExpect`, write the transition-history-backed expectation checker. Land the smoke catalog generator (~10 rows) as unit tests. No live runs yet — Phase 2 is when the harness wires to the live daemon. This separates the data model from the orchestration so each is reviewable independently.

## Follow-ups

- File T3 task for the matrix system itself (or proceed as substrate-repair-lane direct work given the convergence-stall doctrine from 2026-05-08 — TBD with Blake).
- File observation against the T018 wedge once matrix Phase 2 reproduces it deterministically.
- Cross-reference with `01-live-fake-traversal-matrix-plan.md` after code-review of this plan completes (per Blake's instruction).

