# Live Fake Runner Scenario TDD Plan

**Date:** 2026-05-13
**Type:** note

## Summary

The fake-runner work should become a live scenario orchestrator for Stores TDD: it fabricates real preconditions in the actual repository, then lets the real substrate produce the consequences. The test harness must not mock task states, external-review outcomes, liveness judgments, stale-base labels, or integration results. It should create real rows, real worktrees, real commits, real main-branch movement, real subprocesses, real dispatch locks, real run markers, real external-review rows, and real integration attempts. The only substituted component is the nondeterministic LLM/code-review text producer, replaced by a deterministic fake runner that emits valid agent outputs through the same runner boundary as real agents.

Blake is explicitly comfortable with genuine marker commits for this purpose. That unlocks the simplest high-fidelity approach: use tiny fenced commits/files to create observable git and lifecycle facts, rather than trying to keep the substrate pristine by avoiding the very surfaces we need to test.

The core rule:

> The test harness fabricates real preconditions; the Stores substrate produces real consequences.

Bad examples:

- Set a task directly to `stale_base`.
- Mock a watchdog decision.
- Pretend a merge conflict occurred.
- Insert terminal external-review statuses without exercising the runner/review path.

Good examples:

- Move `main` after a fake external-review PASS, then ask integration/acceptance to proceed and observe stale-base refusal.
- Start a fake subprocess, stop heartbeats, and let the real watchdog/status code classify it.
- Create conflicting real commits in a task worktree and on `main`, then run the integration lane.
- Produce a fake executor marker commit in the real task worktree, then let code review, wrap, external review, acceptance, and integration run normally.

## Current context

The previous fake-runner work got Stage 1.2 to PASS review:

- `stores test run ... --live` uses the current repo's real `.stores/db.sqlite`.
- It creates a database backup before mutation.
- It can run via the real daemon/scaffold/auto-drive/external-review/integration path.
- It supports YAML case files in live mode.
- Failed fake external-review live rows can be isolated/frozen so later happy-path runs do not reprocess them.
- Happy path uses `tasks accept` plus `tasks enqueue-integration` plus daemon integration path, not a direct state shortcut.
- Marker commits/files on `main` are surfaced as intentional fake-run proof.
- Validated live rows included a held/failing ER case and a happy path that reached `integrated/done`.

The next move is to turn this into an intentional TDD discipline, not just a one-off harness demo.

## Design stance

### Do not mock outcomes

The harness must not write the answer it wants to test. It should not directly mutate the substrate into the final state under inspection. For example, a stale-base test must not label a row stale; it must create a real base mismatch. A watchdog test must not directly mark a runner dead; it must create process/marker/heartbeat evidence from which the real watchdog/status code reaches that conclusion.

### Synthetic intent is acceptable

The human/LLM intent does not need to be real for every scenario. A test case may create a synthetic task directly. The task content can be boring and explicitly marked as test-owned. What matters is that after the task row exists, the normal machinery is used.

This gives us multiple real lifecycle entrypoints:

1. **Task-flow entrypoint:** create a synthetic task row directly, create worktree, then exercise drive/review/integration.
2. **Observation/intake entrypoint:** create a synthetic observation/intake item, move through contract/ratification/auto-promotion surfaces, then exercise task flow.
3. **Integration-entrypoint:** create or reuse a task branch/review state, then exercise acceptance/integration freshness and git behavior.
4. **Liveness-entrypoint:** create a live runner/process/marker/lock situation, then exercise watch/status/watchdog behavior.

The smoke suite should start at the task-flow entrypoint to keep failure attribution sharp. Observation ratification and auto-promotion should be separate cases, not mandatory ceremony for every substrate test.

### Real worktrees are mandatory

Git state is not an implementation detail here; it is part of the substrate's truth surface. Without real worktrees and commits, we cannot test the bugs that have actually hurt:

- stale base;
- stale external-review freshness;
- dirty worktree refusal;
- merge/rebase conflicts;
- branch/head SHA checks;
- marker commits;
- integration lane behavior;
- duplicate workers on the same worktree;
- daemon/runner current-run marker truth.

Therefore `stores test run --live` cases that claim to test task execution or integration should create real worktrees unless the case is explicitly scoped to a pre-worktree layer.

### Real commits are acceptable, but fenced

Marker commits are allowed. They should be tiny, explicit, and easy to audit. Recommended convention:

- Branch/worktree names: `stores-test/<task-id>-<case>` and `../stores-test-<task-id>-<case>` or the existing task-worktree convention with test slug included.
- Files: `fake-runner-markers/<task-id>-<case>/...` or `.stores-test/<task-id>-<case>/...` depending on what is already accepted by repo ignore/policy.
- Commit messages: `fake-run(<task-id>): <case> marker` or `stores-test(<case>): <specific setup fact>`.
- Task metadata: include visible `test_mode=true`, `test_case=<case>`, and `synthetic=true` fields if supported, or equivalent explicit labels in task title/body and run output.

The point is not to keep main perfectly clean; the point is to make the test traces honest, discoverable, and intentionally harmless.

## Proposed `stores test` mental model

`stores test` is a scenario orchestrator, not a mock runner and not a shortcut engine.

A live test case should generally do this:

```text
stores test run <case> --live --watch
  -> backup .stores/db.sqlite
  -> create or select a real synthetic test row
  -> create a real worktree/branch when relevant
  -> configure deterministic fake-runner scenario
  -> start/trigger the real daemon or real command path
  -> fake agents emit valid outputs through the normal runner seam
  -> real submit handlers mutate lifecycle state
  -> real external-review rows are created/run when relevant
  -> real acceptance/integration commands are invoked when relevant
  -> harness asserts final observable state
  -> harness prints a proof transcript: task id, worktree, commits, rows, run artifacts, expected vs observed
```

The fake runner's role is narrow:

- produce valid planner / plan-reviewer / executor / code-reviewer / wrap / external-review outputs;
- apply small real executor effects when configured, such as marker files and commits;
- create realistic run artifacts, heartbeats, transcripts, telemetry, exit codes, and payload failures;
- make fake provenance loud.

Everything after the runner output should be the same substrate codepath as live agents.

## Smoke suite plan

The smoke suite proves the wind tunnel before we use it for bug TDD. These cases should be fast, visible, and boring.

Command:

```bash
stores test suite smoke --live --delay-ms 5000 --watch
```

Suggested cases:

### 1. `happy-path-integrates`

Purpose: prove the baseline live fake path reaches `integrated/done` through the real daemon and integration lane.

Real setup and path:

```text
create synthetic T3-ish task
create real worktree
planner fake submits valid plan
plan-reviewer fake submits READY
executor fake writes marker file
executor fake commits marker file
code-reviewer fake PASS
wrap fake PASS
external-review row created
fake external-review PASS through normal ER path
test-mode acceptance explicitly allowed
tasks enqueue-integration
real daemon/integration lane lands marker commit
assert integrated/done
```

Expected visible proof:

- task id;
- worktree path;
- marker commit hash;
- external-review row id and runner `fake`;
- integration attempt id/details;
- final lifecycle state;
- `agent_runs` rows showing fake provenance;
- no Codex/Pi/Claude subprocess invocation.

### 2. `plan-review-reject-once-recovers`

Purpose: prove planning feedback loops work through real submit handlers.

Real path:

```text
planner fake submits plan attempt 1
plan-reviewer fake returns NEEDS_WORK
planner fake submits amended plan attempt 2
plan-reviewer fake returns READY
executor marker commit
code-reviewer PASS
wrap PASS
ER PASS
accept/integrate
assert integrated/done with two planner attempts and one rejection
```

This tests real cycle accounting, plan replacement/amendment semantics, and projection/status rendering.

### 3. `code-review-revise-once-recovers`

Purpose: prove execution/review repair loops work.

Real path:

```text
executor fake commits marker v1
code-reviewer fake returns REVISE with finding
executor fake commits marker v2 / fix marker
code-reviewer fake returns PASS
wrap PASS
ER PASS
accept/integrate
assert integrated/done with two executor/code-review cycles
```

This tests real revise transition behavior, cycle bookkeeping, and multi-commit branch integration.

### 4. `failed-er-stays-contained`

Purpose: prove failed/held fake external reviews do not poison future live fake tests or get reprocessed accidentally.

Real path:

```text
task reaches in_review
external-review fake returns tooling_held or configured failure
assert task remains in expected held/in_review/inactive state
assert no accept/integration occurs
run happy-path after it
assert happy-path does not reprocess failed ER row
```

This is already partially proven by the recent T177/T178 live rows. It should become a named smoke/regression case.

### 5. `stale-base-refuses`

Purpose: bridge smoke into battlescar realism by proving git freshness is real.

Real path:

```text
record base A from current main
create synthetic task/worktree from A
fake executor commits task marker X on task branch
fake code-review PASS
fake wrap PASS
fake ER PASS records base_sha=A and head_sha=X
advance main to B with a real fenced marker commit
attempt accept/integration
assert stale-base/freshness refusal, not integration success
```

This case must not mock stale-base. The stale condition is genuine because current `main` differs from the base recorded by the review.

Expected proof output:

```text
[setup] base A=<sha>
[executor] task head X=<sha>
[external-review] pass recorded base=A head=X
[setup] advanced main B=<sha>
[integration] refused stale_base current_main=B review_base=A
[assert] PASS
```

## Battlescar suite plan

Once the smoke suite is green, build a battlescar suite that turns recent painful failures into repeatable red/green cases.

Command:

```bash
stores test suite battlescars --live --delay-ms 5000 --watch
```

Candidate cases:

### Silent zombie / no heartbeat

Real precondition fabrication:

- start fake runner subprocess;
- write partial transcript/status/current-run marker;
- stop heartbeating or exit in a controlled bad way;
- leave the evidence shape that a real crash/stall leaves.

Expected substrate consequence:

- watchdog/status classifies as liveness/watchdog failure;
- task blocked reason is explicit, not vague `payload_invalid` if the intended issue is no-heartbeat;
- telemetry/run marker remains inspectable.

### Legitimate long runner with heartbeat

Real precondition fabrication:

- start fake runner with long delay;
- emit regular heartbeats;
- exceed old wall-clock thresholds.

Expected consequence:

- `stores watch` shows long-running/advisory;
- runner is not killed solely due elapsed wall-clock;
- final output can still be accepted if it eventually arrives.

### Duplicate drive refusal

Real precondition fabrication:

- start a delayed fake drive on a task;
- while the process/lock/marker is live, invoke a second manual or daemon path drive for the same task/worktree.

Expected consequence:

- second drive refuses loudly with live-owner evidence;
- no duplicate runner is spawned;
- first runner remains valid.

### Dirty worktree refusal

Real precondition fabrication:

- let fake executor commit marker;
- before integration, write an uncommitted file into the task worktree.

Expected consequence:

- acceptance/pre-land/integration refuses dirty worktree;
- output identifies dirty path(s);
- no accidental landing.

### Merge conflict

Real precondition fabrication:

- task branch changes a tracked test marker line one way;
- main changes the same line another way;
- integration attempts to merge/rebase.

Expected consequence:

- real git conflict/tooling-held state;
- task/integration status tells the operator where the conflict is;
- no silent partial integration.

### Stale external-review freshness

Real precondition fabrication:

- fake ER PASS for head/base;
- mutate task head or main after review;
- attempt accept/integration.

Expected consequence:

- freshness gate refuses stale review;
- row remains recoverable via rerun/review refresh path.

### Stale/dead current-run marker truth

Real precondition fabrication:

- create a real current-run marker via fake runner;
- kill runner or let it exit without cleanup;
- wait or age marker so it is stale.

Expected consequence:

- `stores watch` and `runs current` distinguish marker-only stale state from live work;
- no false live-owner block forever.

## Observation/intake tests should be separate

Blake asked whether a test should file an observation, ratify the observation/contract, auto-promote, create a worktree, then continue. That is a valid scenario, but it should not be the default smoke path.

Recommended separation:

### `intake-auto-promote-happy-path`

Purpose: test the upstream inlet/observation/contract/auto-promote layer.

Potential path:

```text
create synthetic intake/observation row with test label
draft or supply intent contract
use explicit test-mode/human-grounded approval mechanism if required
auto-promote subscriber creates task
assert task linked back to observation
then optionally hand off to normal fake task-flow smoke
```

This is important, but it tests authority and upstream lifecycle. If it fails, that should not block diagnosing baseline task drive/integration behavior.

### Authority caution

Do not invent a silent fake human. If ratification requires `human` or `ai_with_human` with approval token, the live test harness must either:

- require an explicit test-mode approval flag that is visibly not production authority;
- use a test-only isolated substrate/project where authority semantics are intentionally configured;
- or stop before ratification and assert the expected gate.

For the core task-flow smoke, avoid this entire issue by starting at a synthetic task row with explicit `test_mode` provenance.

## User-facing output requirements

A live test must be watchable and auditable. The command output should not just say PASS/FAIL; it should narrate the real artifacts it created and the real checks it made.

Example for stale-base:

```text
stores test run stale-base-refuses --live --watch

[preflight] live DB: /home/blake/repos/experiments/stores/.stores/db.sqlite
[backup] .stores/backups/test-2026-05-13T12-30-00.sqlite
[setup] created synthetic task T205: stores-test stale-base-refuses
[setup] created worktree ../stores-test-T205-stale-base-refuses
[setup] base A=111aaa
[drive] daemon picked up T205
[executor] fake marker commit X=9ac31f2 fake-run(T205): stale-base task marker
[review] fake code-reviewer PASS
[wrap] fake wrap PASS
[external-review] fake PASS ER461 base=111aaa head=9ac31f2
[setup] advanced main B=222bbb commit=fake-run(T205): stale-base main advance
[integration] attempted enqueue/integration
[result] refused stale_base current_main=222bbb review_base=111aaa
[assert] PASS stale-base was genuine and refused
[artifacts] runs=.stores/runs/... worktree=../stores-test-T205-stale-base-refuses
```

Example for happy path:

```text
stores test run happy-path-integrates --live --delay-ms 5000 --watch

[backup] ...
[setup] T206 worktree=../stores-test-T206-happy-path-integrates
[watch] planning -> ready -> executing -> in_review -> accepted -> integrating -> integrated
[executor] marker commit X=...
[external-review] ER462 runner=fake status=passed
[integration] landed on main commit Y=...
[assert] PASS integrated/done
```

Parallel operator view:

```bash
stores watch --all
stores tasks status T205
stores external-reviews list --task T205
git log --oneline -10 -- fake-runner-markers .stores-test
```

## Agent-facing TDD workflow

For future substrate work, the agentic discipline should be:

1. Identify the engine behavior under test.
2. Add or select a live fake scenario that fabricates the real precondition.
3. Run it and capture the red failure.
4. Fix the substrate/harness code.
5. Rerun the same scenario and capture green.
6. Commit the scenario and the fix together or as clearly linked commits.

Template:

```bash
stores test run --live --case-file tests/fake-cases/battlescars/<case>.yaml --watch
# observe RED
# fix code
stores test run --live --case-file tests/fake-cases/battlescars/<case>.yaml --watch
# observe GREEN
```

The case file is the regression. The live run is the proof.

## YAML case shape

The case language should describe real setup actions and expected observable consequences, not direct final-state mutation.

Sketch:

```yaml
name: stale-base-refuses
mode: live
entrypoint: synthetic-task
labels:
  suite: battlescars
  synthetic: true
fake_runner:
  scenario: all-pass
  delay_ms: 5000
worktree:
  create: true
  executor_effect:
    mode: marker_file
    commit: true
    path: fake-runner-markers/${task_id}/task.txt
preconditions:
  - type: record_main_base
    as: base_a
  - type: after_external_review_pass
    action:
      type: advance_main
      file: fake-runner-markers/${task_id}/main-advance.txt
      commit_message: "fake-run(${task_id}): advance main for stale-base"
flow:
  drive: daemon
  external_review: fake_pass
  accept: attempt_test_mode
  integration: attempt
expect:
  task:
    final_lifecycle: in_review
  integration:
    result: refused
    reason_contains: stale_base
  external_review:
    latest_status: passed
    runner: fake
  git:
    current_main_differs_from_review_base: true
```

The exact schema can evolve, but the important distinction is that `preconditions` are real actions and `expect` asserts visible outcomes.

## Cleanup and state management

Because Blake is okay with genuine commits, cleanup does not need to erase evidence. However, the harness should keep state understandable.

Recommendations:

- Always backup DB before live mutation.
- Print every created task id, branch, worktree, commit, and ER id.
- Use fenced marker paths and commit messages.
- Include a `stores test list-live-artifacts` or equivalent report later if artifact sprawl becomes hard to reason about.
- Prefer tiny additive marker files over modifying real source files, except for explicit merge-conflict cases.
- For conflict/stale-base cases, use dedicated marker files so conflicts are safe and understandable.
- Do not silently delete rows or rewrite DB. If cleanup exists, it should be an explicit cleanup command that archives/marks test rows, not raw SQL deletion.

## Open design decisions

1. **Test metadata storage:** whether to add first-class `test_mode/test_case/synthetic` columns/metadata, or encode in titles/config/artifacts initially.
2. **Marker path convention:** whether to standardize on `fake-runner-markers/` or `.stores-test/` for committed files.
3. **Production fake-review safety:** exact spelling of the explicit allow/test marker that permits fake-reviewed acceptance/integration for test rows without weakening real production acceptance.
4. **Observation ratification tests:** whether to use a test-only authority flag, require approval token, or stop at gate assertion for live observation-path tests.
5. **Artifact reporting:** whether `stores test run` should append a structured JSON proof record for later dashboarding.
6. **Main-branch marker commits:** now acceptable, but should still be visibly fenced and optionally batchable/squashable if they become too numerous.

## Immediate next implementation plan

1. Extend or document `stores test run happy-path --live --watch` as `happy-path-integrates`, with explicit real worktree and marker commit proof.
2. Add `stale-base-refuses` as the first git-freshness smoke/battlescar bridge case.
3. Ensure output prints the exact artifact proof: DB backup, task id, worktree, fake runner rows, ER id, base/head SHAs, marker commits, integration result.
4. Add a `smoke` suite that runs:
   - `happy-path-integrates`
   - `plan-review-reject-once-recovers`
   - `code-review-revise-once-recovers`
   - `failed-er-stays-contained`
   - `stale-base-refuses`
5. Only after the smoke suite is stable, add the battlescar suite for liveness/watchdog/dirty/conflict/duplicate-drive cases.
6. Make future engine tasks start by adding or selecting a scenario case, running it red, then fixing to green.

## The simple solution captured

The simple solution is not to avoid real git effects. The simple solution is to make the fake runner a controlled source of real effects.

We do not need mocks to keep tests safe. We need tiny, fenced, intentional artifacts:

- real synthetic rows;
- real worktrees;
- real fake-runner subprocesses;
- real marker commits;
- real main movement;
- real conflict/dirty/stale preconditions;
- real daemon and integration consequences.

That gives the operator what they asked for: the ability to see the system actually working or breaking, through the same surfaces they use for real work.
