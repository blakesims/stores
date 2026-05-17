# Live Fake Traversal Matrix Plan

**Date:** 2026-05-17
**Type:** note

## Summary

Build `stores test` into a live no-LLM traversal wind tunnel: deterministic fake runners prescribe runner outputs, but the daemon, `.stores/db.sqlite`, worktrees, branches, commits, external-review rows, integration attempts, subprocess markers, telemetry, validators, subscribers, and git consequences all remain real. The first implementation slice should not attempt to enumerate every possible path. It should establish the core model, scenario DSL, synthetic authority boundary, proof transcript, and matrix output using a small but representative MVP suite.

The guiding rule is:

> The harness fabricates real preconditions; Stores produces real consequences.

The fake runner is not a state-transition mock. It is a deterministic producer of valid/invalid runner outputs and tiny real executor effects at the same boundary used by real planner/reviewer/executor/wrap/external-review agents.

## Inherited context

Recent fake-runner work already established the desired stance:

- `stores test run ... --live` should use the current repo's real `.stores/db.sqlite`.
- Live cases should create real synthetic rows, worktrees, branches, marker commits, fake-runner subprocess runs, external-review rows, and integration attempts.
- The existing `src/cli/test.rs` WIP appears to include happy-path, failed-ER, and `stale-base-refuses` case support, fake-mode preflight, private daemon binary refresh, no-real-LLM checks, and live harness proof code.
- `stale-base-refuses` remains the right first battlescar because it is deterministic and validates real freshness refusal after fake ER PASS plus real main movement.

This plan generalizes that work from individual named cases into a traversal/matrix system.

## Goals

1. **No LLM calls during live testing.** Every runner role, including external review, must run through fake runner mode with loud provenance and post-run assertions that no non-fake agent runs occurred.
2. **No mocks below the runner boundary.** The daemon path, database writes, validators, subscriber transitions, git worktrees, commits, merge/rebase behavior, external-review rows, dispatch locks, markers, and telemetry must be real.
3. **Traversal cases are explicit and inspectable.** A case should describe how a packet moves through the system: entrypoint, runner outputs, real perturbations, authority events, expected observable consequences, and proof artifacts.
4. **Matrix output supports TDD.** Operators need a pass/fail table over runner situations and lifecycle traversals, with links to concrete live artifacts for each row.
5. **Human gates are automated safely.** Synthetic approval must be loud, test-scoped, and impossible to apply silently to production rows.

## Non-goals

- Do not create an abstract simulator of Stores' state machine.
- Do not directly set target task statuses such as `blocked`, `stale_base`, `integrated`, or `resolved` as a shortcut to passing a test.
- Do not raw-SQL write the database.
- Do not try to enumerate infinite loop paths literally.
- Do not make observation/intake ratification mandatory for every smoke case; upstream lifecycle cases should be separate from task-flow cases.
- Do not solve every existing `stores watch` display issue before the traversal harness works.

## Core model

Treat Stores as a real labeled transition system plus real side-effect surfaces.

A generated or hand-authored case is built from four dimensions:

### 1. State-machine edges

Source of truth:

- `stores/*/schema.yaml` transitions and actor gates;
- subscriber edges and daemon dispatch behavior;
- integration lane and external-review freshness policy;
- existing submit handlers and validation logic.

The enumerator should produce an edge catalog with records like:

```text
store=tasks from=plan_review verb=submit-plan-review gate=NEEDS_WORK to=planning actor=ai_autonomous guard="plan_review_log.length < 5"
store=tasks from=in_review verb=accept to=accepted actor=human
store=tasks from=integration_blocked verb=retry-integration to=integration_queued actor=ai_with_human
```

This catalog is not a replacement for the schema. It is a test planning artifact used to select coverage targets.

### 2. Runner output alphabet

Each runner role gets a bounded alphabet of deterministic outcomes. Initial alphabet:

```text
planner:
  valid_plan_1_phase
  valid_plan_3_phase
  invalid_payload
  nonzero_exit
  stall_no_heartbeat

plan_reviewer:
  READY
  NEEDS_WORK
  NOT_READY
  invalid_payload
  nonzero_exit

executor:
  no_op
  marker_file
  marker_commit
  scripted_patch
  dirty_worktree_effect
  conflicting_patch
  invalid_payload
  nonzero_exit
  stall_no_heartbeat
  long_running_with_heartbeat

code_reviewer:
  PASS
  REVISE
  CRITICAL_FAIL
  NEEDS_REPLAN
  invalid_payload
  nonzero_exit

wrap:
  PASS
  FAIL
  invalid_payload
  nonzero_exit

external_review:
  PASS
  REVISE
  TOOLING_HELD
  STALE_BASE_STYLE_REFUSAL
  STALE_EXTERNAL_REVIEW_STYLE_REFUSAL
  invalid_payload
  nonzero_exit
```

Important distinction: some labels are runner outputs (`REVISE`), while others should be real environmental consequences (`stale_external_review`). The case DSL must not let authors fake a substrate consequence as a runner output when the purpose is to test the substrate consequence.

### 3. Real environmental perturbations

These fabricate preconditions through real git/process/file activity:

```text
after_external_review_pass: advance_main_with_fenced_marker_commit
after_code_review_pass: mutate_task_head_with_marker_commit
after_executor: leave_uncommitted_dirty_file
before_integration: create_merge_conflict_between_task_and_main
during_runner: kill_fake_subprocess
during_runner: stop_heartbeat_but_leave_marker
while_live_owner_exists: attempt_duplicate_drive
before_accept: refresh_or_stale_external_review_head
```

Perturbations should be phrased as actions, not desired outcomes.

### 4. Authority events

Human-gated transitions become explicit test authority events:

```text
ratify_observation_contract
accept_task
reject_task
resume_blocked
amend_rejected
retry_integration
abandon_test_row
```

These events must run through real verbs and validators under synthetic-test authority, not direct database mutation.

## Bounding traversal enumeration

Traversal space is infinite because review and recovery loops can repeat. Bound it by equivalence classes per loop and per guarded threshold:

- `0_failures_then_pass`
- `1_failure_then_pass`
- `max_minus_1_failures_then_pass`
- `max_failures_then_blocked`
- `hard_fail_immediate`

For pairwise explosion, prefer staged coverage:

1. **Edge coverage:** every schema/subscriber transition used by at least one case.
2. **Boundary coverage:** every guard threshold and human gate tested at pass/fail edges.
3. **Runner alphabet coverage:** every runner outcome tested in at least one role-appropriate path.
4. **Perturbation coverage:** each real-world git/process perturbation tested in one focused case.
5. **Pairwise coverage:** selected runner outcome × perturbation × authority combinations only after the smoke matrix is stable.

The enumerator should be allowed to report uncovered edges/outcomes without requiring all to be implemented immediately.

## Scenario DSL

A case file should prescribe real setup and expected observations. It should not prescribe direct final-state mutation.

Sketch:

```yaml
name: code-review-replan-recovers
suite: smoke
entrypoint: synthetic-task
provenance:
  synthetic: true
  test_case: code-review-replan-recovers
  marker_namespace: fake-runner-markers/${case}/${task_id}

authority:
  mode: synthetic_test
  events:
    accept_task: true
    retry_integration: true

runner_script:
  planner:
    attempts:
      - valid_plan_3_phase
      - valid_plan_3_phase
  plan_reviewer:
    attempts:
      - READY
      - READY
  executor:
    attempts:
      - marker_commit
      - marker_commit
  code_reviewer:
    attempts:
      - NEEDS_REPLAN
      - PASS
  wrap:
    attempts:
      - PASS
  external_review:
    attempts:
      - PASS

perturbations: []

flow:
  daemon: run_once_until_quiescent
  external_review: normal_fake
  acceptance: synthetic_accept
  integration: normal_daemon

expect:
  final:
    lifecycle: integrated
    status: integrated
  counts:
    planner_attempts: 2
    executor_attempts: 2
    code_review_attempts: 2
  proof:
    no_real_llm: true
    real_worktree: true
    external_review_runner: fake
    marker_commit_landed: true
```

Observation entrypoint example:

```yaml
name: observation-ratify-auto-promotes-task
suite: upstream
entrypoint: observation
provenance:
  synthetic: true
  test_case: observation-ratify-auto-promotes-task

authority:
  mode: synthetic_test
  events:
    ratify_observation_contract: true

observation:
  summary: synthetic traversal matrix observation
  intent_contract:
    tier_hint: T3
    title: synthetic traversal task
    done_when: fake runner matrix proves auto-promotion
    scope_in: synthetic task creation
    scope_out: production behavior changes

flow:
  observation: create_then_synthetic_ratify
  promotion: wait_for_auto_promote

expect:
  final:
    observation_contract_state: approved
    task_created: true
    task_linked_to_observation: true
  proof:
    synthetic_authority_logged: true
```

Stale freshness example:

```yaml
name: stale-external-review-refuses
suite: battlescars
entrypoint: synthetic-task
runner_script:
  planner: { attempts: [valid_plan_3_phase] }
  plan_reviewer: { attempts: [READY] }
  executor: { attempts: [marker_commit] }
  code_reviewer: { attempts: [PASS] }
  wrap: { attempts: [PASS] }
  external_review: { attempts: [PASS] }
perturbations:
  - at: after_external_review_pass
    action: advance_main_with_fenced_marker_commit
flow:
  acceptance: attempt_synthetic_accept
  integration: attempt_normal_integration
expect:
  final:
    integrated: false
  refusal:
    reason_contains_any:
      - stale_external_review
      - freshness
      - stale_base
  proof:
    main_sha_changed_after_review: true
    review_runner: fake
```

## Synthetic authority design

Add an explicit synthetic authority boundary for `stores test`, not a general bypass.

Proposed rules:

1. The command must be running under `stores test run --live` or `stores test suite --live`.
2. The case must declare `authority.mode: synthetic_test`.
3. The target row must carry loud provenance, such as:
   - `synthetic=true` where first-class metadata exists, or
   - title/body/test metadata containing `stores-test` and `test_case=<case>` until first-class columns exist.
4. The transition must be listed in the case's `authority.events`.
5. The synthetic authority event must call the normal verb path and actor validation through a test-only invoker/approval mechanism, not raw SQL.
6. Every synthetic authority use must be printed in the proof transcript.
7. If a target row lacks test provenance, the command must fail closed.

Possible implementation options, in preference order:

- Add an internal `TestAuthority` wrapper used only by `stores test` that supplies the same proof fields required by existing handlers, while validators check row provenance.
- Add a `--test-authority <case>` flag accepted only by specific human-gated commands when the row is synthetic/test-owned.
- As a temporary bridge, keep existing harness behavior for `tasks accept --invoker human` only inside live fake harness, but make it loud in output and replace it with a proper scoped mechanism before broad matrix suites.

Open question: whether synthetic authority should be represented as a distinct invoker, a test-only approval token, or a command-layer wrapper around existing `human`/`ai_with_human` semantics. The plan should not silently weaken production actor gates.

## Pass/fail matrix output

`stores test suite <suite> --live --matrix` should end with a compact table plus artifact paths.

Example:

```text
CASE                                      ENTRY   EXPECTED                 RESULT  ARTIFACTS
happy-path-integrates                    task    integrated               PASS    T205 ER461 run=R802 head=abc123
plan-review-reject-once-recovers         task    integrated               PASS    T206 runs=2/1
plan-review-max-rejects-blocks           task    blocked:task_review      PASS    T207 reviews=5
code-review-replan-recovers              task    integrated               FAIL    T208 expected planning-loop got blocked
stale-external-review-refuses            git     freshness_refused        PASS    T209 ER462 base=aaa head=bbb main=ccc
duplicate-drive-refuses                  live    second_drive_refused     PASS    T210 owner=pid:12345
observation-ratify-auto-promotes-task    obs     task_created             PASS    L084/T211
```

Each row should have a machine-readable proof record as well as human output:

```text
.stores/test-runs/<timestamp>-<suite>/<case>.json
.stores/test-runs/<timestamp>-<suite>/<case>.log
```

Proof records should include:

- DB backup path;
- case name and suite;
- task/observation/intake/external-review ids;
- branch and worktree paths;
- start/end main SHA;
- task head/base SHAs;
- marker commit SHAs and paths;
- agent run ids and runner names;
- daemon run invocations;
- authority events used;
- final row snapshot;
- no-real-LLM assertion result;
- stderr/stdout snippets for failures/refusals.

## Command surface

Target commands:

```bash
stores test enumerate [--store tasks] [--suite smoke] [--format table|json]
stores test run <case> --live --watch [--case-file <path>]
stores test suite <suite> --live --matrix [--watch] [--max-cases N]
stores test artifacts [--latest] [--case <case>]
```

Initial behavior:

- `enumerate` can start as a static catalog plus hand-maintained coverage table, then evolve into schema-driven discovery.
- `run` executes one named case through the existing live harness.
- `suite` runs a fixed list and prints the matrix.
- `artifacts` is optional but useful once live proof files accumulate.

## Phased implementation plan

### Phase 0 — Stabilize current live harness baseline

Deliverables:

- Confirm current WIP cases: `happy-path`, `t3-failed-er`, and `stale-base-refuses`.
- Ensure fake-mode preflight proves the active CLI, private daemon binary, and `stores-fake-agent` are current enough and no real LLM runner is invoked.
- Make proof output consistent across current cases.
- Keep `stale-base-refuses` as the first deterministic battlescar case.

Validation:

```bash
cargo build --bin stores --bin stores-fake-agent
cargo test test_cli_test -- --nocapture   # or focused equivalent
stores test run happy-path --live --watch
stores test run stale-base-refuses --live --watch
```

Safety constraints:

- Stage only fenced marker files for main-advance commits.
- Print every created task/worktree/branch/commit/ER id.
- No raw SQL writes.

### Phase 1 — Define the case DSL and proof schema

Deliverables:

- A first `tests/fake-cases/` or `.stores/test-cases/` layout for YAML cases.
- Rust structs for scenario case, runner script, perturbations, authority events, expectations, and proof artifacts.
- A normalizer that maps current built-in presets into the same internal case model.
- A proof transcript schema written to JSON and displayed in human-readable form.

MVP cases to encode:

- `happy-path-integrates`
- `plan-review-reject-once-recovers`
- `code-review-revise-once-recovers`
- `failed-er-stays-contained`
- `stale-external-review-refuses` / current `stale-base-refuses` alias

Validation:

- Unit tests parse and normalize YAML cases.
- Built-in preset and YAML case produce equivalent fake runner script for happy path.
- Proof JSON validates against expected fields for a dry/fake fixture.

### Phase 2 — Implement synthetic authority safely

Deliverables:

- A scoped synthetic authority mechanism available only under `stores test --live`.
- Row provenance checks before any synthetic authority event.
- Loud proof transcript entries for every synthetic ratify/accept/resume/retry action.
- Refusal tests proving synthetic authority cannot apply to non-test rows.

First supported events:

- `accept_task`
- `retry_integration`
- `ratify_observation_contract`

Validation:

- Unit tests: non-synthetic row + synthetic authority fails closed.
- Live test: synthetic task can be accepted without approval token only through `stores test` and only with test provenance.
- Live test: observation contract can be synthetic-ratified and auto-promoted, with proof output.

### Phase 3 — MVP matrix suite

Deliverables:

- `stores test suite smoke --live --matrix` runs a fixed list of 5 cases.
- Matrix output includes PASS/FAIL, expected outcome, actual outcome, and artifact handles.
- Failure rows preserve red proof rather than hiding or cleaning it up.
- Suite-level no-real-LLM assertion aggregates all case proofs.

Smoke MVP:

1. `happy-path-integrates`
2. `plan-review-reject-once-recovers`
3. `code-review-revise-once-recovers`
4. `failed-er-stays-contained`
5. `stale-external-review-refuses`

Validation:

```bash
stores test suite smoke --live --matrix --watch
```

The suite is green only if every case uses fake runners only and produces expected live artifacts.

### Phase 4 — Enumeration catalog and coverage reporting

Deliverables:

- `stores test enumerate` prints the current edge/outcome catalog.
- A coverage mapper links cases to schema edges, runner alphabet outcomes, perturbations, and authority events.
- Output identifies untested edges/outcomes without making them blockers.

Initial implementation can be partially static:

- Start with task lifecycle, external review, and integration lane.
- Add observation/intake after task-flow smoke is stable.
- Add dynamic schema parsing once the shape is clear.

Validation:

```bash
stores test enumerate --store tasks --format table
stores test enumerate --suite smoke --format json
```

Expected result: the operator can see which transitions are covered by the smoke suite and which remain untested.

### Phase 5 — Traversal generator

Deliverables:

- Generate candidate cases from bounded loop equivalence classes:
  - `0_failures_then_pass`
  - `1_failure_then_pass`
  - `max_minus_1_failures_then_pass`
  - `max_failures_then_blocked`
  - `hard_fail_immediate`
- Generate only selected suites by default; do not auto-run hundreds of live cases.
- Allow generated cases to be materialized as YAML for review and versioning.

Validation:

```bash
stores test enumerate --generate plan-review-loop --format yaml
stores test run --case-file generated/plan-review-max-rejects-blocks.yaml --live --watch
```

### Phase 6 — Battlescar suite

Deliverables:

Add cases for recent engine failures:

- silent zombie / no heartbeat;
- legitimate long runner with heartbeat;
- duplicate drive refusal;
- dirty worktree refusal;
- merge conflict;
- stale external-review after task head mutation;
- stale/dead current-run marker truth.

Validation:

```bash
stores test suite battlescars --live --matrix --watch
```

Acceptance: each case creates the real bad precondition and proves the real substrate classification or refusal.

### Phase 7 — Upstream packet traversal suite

Deliverables:

- Observation/intake entrypoint cases:
  - intake -> gatekeeper classification -> observation;
  - observation -> synthetic ratification -> auto-promotion -> task;
  - T1 contract-is-plan path;
  - T2 one-phase plan path;
  - T3 full planner/reviewer path.
- Matrix rows link source observation/intake ids to created task ids.

Validation:

```bash
stores test suite upstream --live --matrix --watch
```

This phase should happen after synthetic authority is safe and task-flow smoke is stable.

## First useful MVP

The first useful MVP is deliberately small:

1. Normalize existing built-in live cases into a case model.
2. Add proof JSON output for each run.
3. Add `stores test suite smoke --live --matrix` with:
   - happy path integrates;
   - plan review rejects once then recovers;
   - code review revises once then recovers;
   - failed ER stays contained;
   - stale freshness refuses.
4. Add loud synthetic acceptance for test tasks only if required for the suite; defer observation ratification until Phase 7 unless it is cheap after the authority mechanism exists.
5. Add a static `stores test enumerate --suite smoke` showing what the MVP covers and what it does not.

This gives Blake a real pass/fail dashboard quickly without boiling the ocean.

## Safety constraints

- No raw SQL writes.
- Never `git add .`; stage explicit fenced marker paths only.
- Do not force-rewrite `main` in live mode.
- Do not hide marker commits; print them.
- Do not clean up red proof automatically.
- Fail closed if a synthetic authority event targets a row without test provenance.
- Always assert and print zero non-fake runner usage.
- Keep fake-review acceptance gated to test rows; fake ER PASS must never masquerade as production review authority.
- Back up `.stores/db.sqlite` before live mutation and print the backup path.

## Open questions

1. Should synthetic authority be a distinct invoker, a test-only approval token, or an internal wrapper around existing actor semantics?
2. Should test provenance become first-class schema fields (`synthetic`, `test_case`, `test_run_id`) or remain encoded in metadata/title/body initially?
3. Where should durable case files live: `tests/fake-cases/`, `.stores/test-cases/`, or another repo-owned directory?
4. Should proof JSON live under `.stores/test-runs/` or a gitignored `target/stores-test-runs/` path?
5. How much live artifact cleanup should exist, given that preserving proof is valuable but marker/worktree sprawl can become noisy?

## Recommended next action

Start with Phase 1 + Phase 3 MVP around the current `src/cli/test.rs` WIP:

- keep `stale-base-refuses` / stale freshness as the first battlescar;
- introduce the internal case/proof model;
- add matrix output for a five-case smoke suite;
- implement synthetic authority only as far as required for test-row acceptance and no-real-LLM proof;
- defer full schema-driven generation until the fixed smoke matrix is green.

