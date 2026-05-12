# Fake Runner TDD Modus Operandi

**Date:** 2026-05-12
**Type:** note

## Summary

We are using fake-runner work in two TDD stages. Stage 1 hardens the testing system itself. Stage 2 uses that hardened system to reproduce and fix real engine bugs.

All agents/workers/reviewers on this thread must follow this distinction.

## Boundary rule

The boundary is not "fake-runner code" vs "engine code." Some engine code is part of harness fidelity.

- **Stage 1:** harness-fidelity failures, even when the fix is in engine code. Example: daemon reexec bypasses `STORES_LLM_OFF` and launches real Claude.
- **Stage 2:** engine-correctness failures surfaced through a working harness. Example: a deterministic fake scenario reproduces a real stale-submit or watchdog bug after the harness is already trusted.

If Stage 1 surfaces a non-blocking Stage 2 bug, append it to the Stage 2 candidate list and stop. Do not derail harness bring-up into open-ended engine debugging.

## Stage 1 — TDD the testing system

**Goal:** make `STORES_LLM_OFF=1` / `runner=fake` a trustworthy operator test harness.

A happy-path fake run must go all the way through the **real project substrate database and real daemon path** to **`integrated`**. The only mocked component is LLM execution:

```text
real observation/intake or real test task row in current .stores/db.sqlite
→ real daemon sees it
→ real scaffold/worktree path
→ real auto-drive dispatch lock / process management
→ planner fake
→ plan-reviewer fake
→ executor fake with marker commit
→ code-reviewer fake
→ wrap fake
→ real external-review row, runner fake
→ fake-review acceptance explicitly allowed for test mode
→ real integration lane
→ integrated
```

Synthetic temp-DB harnesses are useful unit/integration tests, but they do **not** satisfy Stage 1 operator UX. Blake must be able to run the command in this repo and watch the row move in normal `stores watch` / `stores tasks status`.

**Rule:** if the failure is caused by fake-runner/test-mode infrastructure, fix it directly outside the substrate workflow.

Do not file substrate observations for Stage 1 harness failures. Do not dogfood the dogfood harness while the harness itself is broken. Use direct code edits plus executor/reviewer loops. The pass condition is literal: the failing fake-mode test now passes.

**Sunset clause:** this direct-edit escape is temporary. When all current Stage 1 issues are checked and the transition criteria below pass, default work returns to substrate-driven flow for non-harness work.

Stage 1 includes:

- no real Claude/Codex/Pi calls under `STORES_LLM_OFF=1`;
- private daemon reexec path respects fake mode and is covered by post-reexec sentinel tests;
- live DB migrations support fake metadata such as `external_reviews.runner='fake'`;
- fake external review persists successfully;
- fake marker executor can create real commits;
- watch/status/runs accurately show fake-run state and fake provenance;
- test runs leave repo/DB state understandable;
- concrete operator UX: `stores test run <case> --live --watch` and `stores test suite <suite> --live --watch`, where `--live` means current repo DB + real daemon path, not a temp synthetic DB.

### Stage 1 → Stage 2 transition criteria

Do not enter Stage 2 until all are true:

1. Live happy-path waterfall (`stores test run happy-path --live --watch`) reaches `integrated` on **three consecutive clean runs** in the current repo DB and is visible in normal `stores watch`.
2. A CI/test sentinel asserts **zero real LLM subprocess invocations** across fake scenarios, including the daemon post-reexec path.
3. All currently open Stage 1 issue-list items are checked.

### Test hygiene rule

All tests that mutate `STORES_LLM_OFF` or fake-runner env vars must use `crate::runner::test_support::ENV_LOCK` and restore the original value on exit.

## Stage 2 — TDD the real engine issues

Only enter Stage 2 once Stage 1 meets the transition criteria.

**Goal:** use deterministic fake scenarios to reproduce and fix real engine battlescars.

Examples / candidate list:

- silent zombie / watchdog failure;
- stale daemon/private binary behavior unrelated to fake-mode fidelity;
- duplicate dispatch;
- stale submit;
- external-review convergence stalls;
- stale-base / stale external-review freshness;
- fake-reviewed acceptance safety;
- lock cleanup;
- watch/status truth for real stuck-task states reproduced via fake;
- resume from blocked role;
- auto-drive disabled staying disabled.

Here the fake runner is not under test. The engine is under test.

Workflow:

1. choose/add a deterministic fake scenario;
2. run it through the test harness;
3. observe the engine failure;
4. fix the engine behavior;
5. rerun the same scenario;
6. pass means the engine issue is fixed and regression-covered.

## Current operating mode

We are currently in **Stage 1**.

The next milestone is one clean, repeatable **live** happy-path fake test that writes to the real current `.stores/db.sqlite`, is picked up by the real daemon path, is visible in normal `stores watch`, and reaches `integrated` without real LLM calls.

## Running Stage 1 issue list

Use this section as the local TDD todo list. Add an unchecked item when a fake-mode test fails. Check it only when the same test passes after the fix.

- [ ] Stale private daemon binary can bypass fake mode: `STORES_LLM_OFF=1 stores agents run --once` reexeced into `~/.local/share/stores/bin/stores` and launched a real Claude planner before the private binary was updated. Fix must include sentinel coverage for the post-reexec process, not only the initial CLI process.
- [ ] Live DB CHECK constraint rejects fake external review: fake ER dispatch runs, but persistence fails with `CHECK constraint failed: runner IN ('codex', 'pi', 'claude-code')`. Fix shape: ship a migration/schema repair that rebuilds the affected SQLite table constraint to include `fake` as a valid runner; do not rely only on YAML drift.
- [ ] `stall-no-heartbeat` user-visible classification is wrong: task blocks as `payload_invalid` / `runner_payload_error` instead of a clear liveness/watchdog failure.
- [x] Focused Stage 1 harness test reaches `integrated` with fake planner/plan-reviewer/executor(marker commit)/code-reviewer/wrap, fake external-review PASS, explicit fake-review accept marker, integration landing, and no real codex sentinel invocation: `cargo test fake_happy_path_waterfall_reaches_integrated_without_real_llm_invocations -- --test-threads=8`.
- [x] `stores test run happy-path --live --delay-ms 5000 --watch` writes to the current repo DB, uses the real daemon path, is visible in normal `stores watch`, exits 0, and asserts task reaches `integrated` with no real LLM subprocesses.
- [x] `stores test run t3-failed-er --live --delay-ms 5000 --watch` writes to the current repo DB, uses the real daemon path, exits 0, and asserts the expected fake external-review held/failure state.
- [x] Configurable YAML test cases exist for live mode: an operator can define per-stage fake outputs/outcomes (planner, plan-reviewer, executor, code-reviewer, wrap, external-review, accept/integration expectations) to fabricate extreme cases without code changes.
- [x] `stores test run --live --case-file <path> --watch` executes a YAML-defined case through the real DB + real daemon fake harness and asserts the configured expected state.
