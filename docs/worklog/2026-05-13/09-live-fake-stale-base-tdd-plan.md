# Live Fake Stale Base TDD Plan

**Date:** 2026-05-13
**Type:** note
**Oracle-reviewed:** yes; revisions below incorporate the oracle critique.

## Summary

Implement the first real live fake-runner battlescar scenario: `stale-base-refuses`. The case name preserves the historical battlescar label, but the precise expected proof is broader and more accurate: **a freshness refusal after fake external-review PASS and real main movement**, likely surfaced today as `stale_external_review` rather than exact `stale_base`.

The goal is not to mock a stale label. It is to fabricate the real precondition under the live repo/daemon path — a fake-reviewed task branch based on old `main`, then a real fenced marker commit that advances `main` — and let Stores' normal acceptance/integration/freshness path refuse it. The scenario must be runnable with no LLM calls at all, including planner/executor/code-reviewer/wrap/external-review.

This is the first concrete TDD wind-tunnel case from `01-live-fake-runner-scenario-tdd-plan.md`, using the stale freshness / ER precheck / integration non-convergence battlescar family from `03-t146-engine-friction-audit-and-t148-start.md`.

## Approved DONE_WHEN

`stores test run stale-base-refuses --live --watch` (or a documented equivalent case-file invocation) runs against the real repo with the real daemon path and fake runners only; it creates a synthetic task, real worktree/branch, fake executor marker commit, fake external-review PASS with recorded base/head, advances main with a fenced marker commit, attempts acceptance/integration through normal commands, and proves the real substrate refuses with stale-base/freshness evidence visible in command output and `stores watch`; the run prints all proof artifacts and asserts zero non-fake `agent_runs` / no LLM calls.

Oracle clarification: with safe additive main movement, exact `stale_base` may not be the canonical label because the reviewed base can remain an ancestor of current `main`. Current integration code is more likely to refuse as `stale_external_review` after refresh/rebase changes the candidate head. The accepted proof is therefore **freshness refusal caused by genuine main movement after review**, with output naming the actual canonical reason.

## Design stance

- The harness fabricates real preconditions; Stores produces real consequences.
- Do not set a task directly to `stale_base`, `stale_external_review`, `blocked`, or any target outcome.
- Do not insert terminal external-review rows directly as the proof. Fake ER may run through the same runner/external-review seam, but the freshness refusal must come from normal accept/integration checks.
- Real commits are allowed, but fenced and auditable.
- Use an additive marker commit on `main`; do **not** force-rewrite/orphan main in the live repo just to force exact `stale_base`.
- This should be a named live case, not a one-off shell recipe.
- The harness uses the real daemon code path (`stores agents run --once` repeatedly) unless/until a separate attach-to-detached-daemon mode is added. That is the intended meaning of “real daemon path” here.

## Current code seam

`src/cli/test.rs` already has:

- `stores test run <case> --live --watch` plumbing.
- fake-mode preflight and private daemon binary refresh.
- `LiveHarness` that creates a live synthetic task and drives `stores agents run --once`.
- fake runner env: `STORES_LLM_OFF=1`, `STORES_FAKE_AGENT_BIN`, `STORES_FAKE_CASE_FILE`, `STORES_ALLOW_FAKE_REVIEW_ACCEPT=1`.
- happy-path and failed-ER presets.
- proof checks for non-fake `agent_runs` and real external-review runners.

Known gaps:

- `stale-base-refuses` is not a built-in preset.
- `LiveHarness::run` is happy-path/failed-ER shaped and auto-integrates only when expected final status is `integrated`.
- `matches_expect` cannot express “we attempted normal accept/integration and got freshness refusal reason X”.
- `snapshot()` does not load enough proof data: workspace/branch, ER base/head, integration attempts, task blocked/freshness reason, main/task SHAs.
- Non-integrated live cases currently call generic isolation/deactivation; this scenario must bypass that so Blake can watch the live refused row in `stores watch`.

## Implementation plan

### Phase 1 — Add the named stale freshness case and route it separately

Add a `stale-base-refuses` preset/case shape in `src/cli/test.rs`.

Expected case semantics:

```yaml
cases:
  stale-base-refuses:
    tier: T3
    executor_mode: marker_file
    stages:
      planner: { outcome: PASS }
      plan_reviewer: { outcome: PASS }
      executor: { outcome: PASS }
      code_reviewer: { outcome: PASS }
      wrap: { outcome: PASS }
      external_review: { outcome: PASS }
    expect:
      task_status: non_integrated
      external_review_status: passed
      no_real_llm: true
      integration_result: refused
      reason_contains_any:
        - stale_external_review
        - stale external review
        - freshness
        - stale_base
```

Do not force this through the existing happy-path `matches_expect` loop. Add a dedicated live scenario branch/method, e.g. `run_live_stale_base_refuses`, selected by `case_name == "stale-base-refuses"`.

### Phase 2 — Wait for real worktree and fake ER PASS

Dedicated live flow:

1. Create the live synthetic task as today.
2. Run the real daemon path until the task has a real `workspace_path` and `branch`.
3. Record `base_a = git rev-parse main` from the live repo.
4. Continue daemon steps until fake ER PASS is persisted.
5. Read actual proof from DB and git, not assumptions:
   - task status/lifecycle/active_step;
   - workspace path and branch;
   - latest ER display id/status/verdict/runner/base_sha/head_sha/superseded_by;
   - task branch/worktree HEAD;
   - initial main SHA.

Fake ER `base_sha`/`head_sha` should be trusted only after reading the actual `external_reviews` row. The proof must assert ER base/head were captured before later main movement.

### Phase 3 — Fabricate the real stale freshness precondition

After fake ER PASS has persisted base/head:

1. Advance real `main` with a tiny fenced marker commit, staging only the marker path:

```text
fake-runner-markers/<task-id>-stale-base-refuses/main-advance.txt
commit: fake-run(<task-id>): stale-base main advance
```

2. Record new `main_b = git rev-parse main`.
3. Assert `main_b != base_a` and print both.
4. Do not force-rewrite main; additive movement is the safe live precondition.

Use a distinct marker path from the fake executor marker to avoid merge conflicts unless the scenario explicitly wants a conflict.

### Phase 4 — Attempt normal accept/integration and assert refusal

Attempt normal commands after main movement:

1. `stores tasks accept <task> --invoker human` under explicit test-harness fake acceptance semantics already used by the live harness.
2. `stores tasks enqueue-integration <task>` if accept succeeds.
3. Run `stores agents run --once` / integration path as needed.

Valid outcomes:

- `tasks accept` refuses with a freshness/stale external review reason;
- or accept succeeds but enqueue/integration refuses/blocks with freshness/stale external review reason.

Invalid outcome:

- task reaches `integrated`/`done` with the stale reviewed head unrefreshed.

The assertion should classify current canonical refusal strings, including:

- `stale_external_review`;
- `stale external review head`;
- `external review head` with mismatch evidence;
- `freshness`;
- `stale_base`.

### Phase 5 — Print proof and leave visible

The live command should print a concise proof transcript:

```text
[backup] .../.stores/backups/...
[setup] task=T205 worktree=... branch=...
[setup] base A=<sha>
[executor] task head X=<sha> marker=<path>
[external-review] ER461 runner=fake status=passed verdict=PASS base=<sha> head=<sha>
[setup] advanced main B=<sha> commit=<sha> marker=<path>
[accept/integration] refused reason=<canonical reason/output>
[assert] PASS freshness refusal was genuine; integrated=false
[assert] no_real_llm=ok non_fake_agent_runs=0 real_er_runners=0
[watch] stores watch --all
[cleanup optional] stores tasks deactivate <task> --reason ...
```

Do not call generic `isolate_live_case()` or direct SQL retry-freeze helpers. The row should remain visible/recoverable for `stores watch` so Blake can observe the live issue. Print optional cleanup guidance separately.

### Phase 6 — Tests

Add unit/integration tests where possible without mutating the live repo:

- preset loading recognizes `stale-base-refuses`;
- fake-mode scenario for it is all-pass;
- live scenario routing selects the dedicated stale freshness branch rather than happy-path integration;
- refusal classifier recognizes current stale/freshness strings;
- fenced main marker helper stages/uses the expected path/message;
- proof snapshot queries tolerate missing optional integration fields.

Do not make tests depend on the developer's real `.stores/db.sqlite`.

### Phase 7 — Live validation

Run the actual command after implementation:

```bash
cargo build --bin stores --bin stores-fake-agent
./target/debug/stores test run stale-base-refuses --live --watch
```

If the installed/current binary path matters, use the documented equivalent invocation but preserve the user-facing command in help/docs.

If the live run unexpectedly integrates, preserve that as the red proof and do not paper over it. The TDD value is the genuine substrate behavior.

## Guardrails

- Do not raw-SQL write final task states.
- Do not use the existing direct SQL retry-freeze helper for this scenario.
- Do not skip fake-mode preflight.
- Do not call real Codex/Pi/Claude for ER/wrap/code review.
- Do not hide main marker commits; print them.
- Explicitly stage only the fenced marker file when advancing main; never `git add .`.
- Do not mix unrelated rustfmt/generated task projection churn into the implementation commit.
- A detached daemon running concurrently may race with the harness. The command should print enough context to make this visible; robust attach-to-detached-daemon mode is out of scope for this first case.

## Acceptance checklist for implementation

- `cargo build --bin stores --bin stores-fake-agent` passes.
- Focused tests for preset/routing/refusal classifier/proof helpers pass.
- Live `stale-base-refuses` run reaches a real non-integrated freshness refusal or captures an unexpected integration as red proof.
- Output includes DB backup, task id, worktree, branch, ER id/status/verdict/runner/base/head, main before/after SHAs, marker commit, refusal reason, no-LLM assertions, and watch command.
- `stores watch --all` can show the live refused task after the command exits.
