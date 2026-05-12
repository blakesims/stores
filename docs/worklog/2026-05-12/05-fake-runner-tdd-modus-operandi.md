# Fake Runner TDD Modus Operandi

**Date:** 2026-05-12
**Type:** note

## Summary

We are now using fake-runner work in two TDD stages. Stage 1 tests and hardens the testing system itself. Stage 2 uses that hardened system to reproduce and fix real engine bugs.

All agents/workers/reviewers on this thread must follow this distinction.

## Stage 1 — TDD the testing system

**Goal:** make `STORES_LLM_OFF=1` / `runner=fake` a trustworthy operator test harness.

A happy-path fake run must go all the way through the real substrate:

```text
task created
→ scaffold/worktree
→ planner fake
→ plan-reviewer fake
→ executor fake with marker commit
→ code-reviewer fake
→ wrap fake
→ external-review fake
→ accept/integration where applicable
→ expected terminal state
```

**Rule:** if the failure is caused by fake-runner/test-mode infrastructure, fix it directly outside the substrate workflow.

Do not file substrate observations for Stage 1 harness failures. Do not dogfood the dogfood harness while the harness itself is broken. Use direct code edits plus executor/reviewer loops. The pass condition is literal: the failing fake-mode test now passes.

Stage 1 includes:

- no real Claude/Codex/Pi calls under `STORES_LLM_OFF=1`;
- private daemon reexec path respects fake mode;
- live DB migrations support fake metadata such as `external_reviews.runner='fake'`;
- fake external review persists successfully;
- fake marker executor can create real commits;
- watch/status/runs surfaces clearly show fake provenance;
- test runs leave repo/DB state understandable;
- eventual operator UX: `stores test ...` cases/suites.

## Stage 2 — TDD the real engine issues

Only enter Stage 2 once Stage 1 is reliable.

**Goal:** use deterministic fake scenarios to reproduce and fix real engine battlescars.

Examples:

- silent zombie / watchdog failure;
- stale daemon/private binary behavior;
- duplicate dispatch;
- stale submit;
- external-review convergence stalls;
- stale-base / stale external-review freshness;
- fake-reviewed acceptance safety;
- lock cleanup;
- watch/status truth;
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

The next milestone is one clean, repeatable happy-path fake test that reaches the expected terminal state without real LLM calls.

## Running issue list

Use this section as the local TDD todo list. Add an unchecked item when a fake-mode test fails. Check it only when the same test passes after the fix.

- [ ] Stale private daemon binary can bypass fake mode: `STORES_LLM_OFF=1 stores agents run --once` reexeced into `~/.local/share/stores/bin/stores` and launched a real Claude planner before the private binary was updated.
- [ ] Live DB CHECK constraint rejects fake external review: fake ER dispatch runs, but persistence fails with `CHECK constraint failed: runner IN ('codex', 'pi', 'claude-code')`.
- [ ] `stall-no-heartbeat` user-visible classification is wrong: task blocks as `payload_invalid` / `runner_payload_error` instead of a clear liveness/watchdog failure.
- [ ] No first-class `stores test` UX for named cases/suites such as `happy-path`, `T1`, `T2`, `T3`, `T3 with failed ER`, and battlescar scenarios.
