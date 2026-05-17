# Daily Summary — 2026-05-12

## Overview

The day centered on making no-LLM dogfooding real. The design moved from proposal to shipped fake-runner phases: a first-class Stores `FakeRunner` backed by a real `stores-fake-agent` subprocess, `STORES_LLM_OFF` selection, fake external review, deterministic scenarios, failure taxonomy, marker/scripted executor effects, and integration-pressure support.

Manual live stress tests then reframed the work into two TDD stages: first harden the fake-runner test harness itself, then use it to reproduce and fix real engine battlescars. By the end of the notes, Stage 1 had live operator UX (`stores test run ... --live --watch`) and YAML case support, with several harness gaps either fixed or explicitly tracked.

## Work Completed

- **Fake-runner architecture settled:** use Stores-native `FakeRunner` plus `stores-fake-agent` subprocess, not provider proxying or direct transition simulation.
- **Fake runner Phase 1 shipped:** explicit `runner=fake`, real subprocess lifecycle, valid role outputs, run artifacts, fake telemetry, and happy-path drive through standard runner seam.
- **Fake runner Phase 2 shipped:** `STORES_LLM_OFF` override for drive and external review, fake decision events, requested-vs-effective telemetry split, fake ER PASS, and fake-review acceptance safety.
- **Fake runner Phase 3 shipped:** named deterministic scenarios for plan-review reject, code-review revise, ER revise/tooling failure, payload invalid, nonzero, stall/heartbeat, and messy legacy output; substrate-path tests prove expected task/ER states.
- **Fake runner Phase 4 shipped:** executor modes (`no_op`, `marker_file`, `scripted_patch`), fake provenance commits, start/end git SHA metadata, fake external-review wrap-artifact fidelity, and marker-commit integration smoke.
- **Operator UX scoped:** `stores test run` / `stores test suite` should create real synthetic rows/worktrees, set no-LLM mode, run via real daemon paths, and summarize proof artifacts.
- **Manual stress tests run:** T154-T159 proved fake drive roles, plan-review/code-review recovery scenarios, payload-invalid, nonzero, and stall behavior; also surfaced live DB and binary-path gaps.
- **Stage 1 TDD doctrine captured:** direct-edit harness failures until no-LLM live happy path reaches `integrated` reliably; Stage 2 begins only after the harness is trusted.

## Notes Today

| # | Note | Topic |
|---|------|-------|
| 01 | [llm-off-fake-agent-proposal.md](./01-llm-off-fake-agent-proposal.md) | Initial token-free fake CLI proposal and runner seam reconnaissance. |
| 02 | [fake-runner-no-llm-dogfood-proposal.md](./02-fake-runner-no-llm-dogfood-proposal.md) | First-class fake runner recommendation, metadata truth, scripted scenarios, executor realism. |
| 03 | [combined-fake-runner-implementation-plan.md](./03-combined-fake-runner-implementation-plan.md) | Four-phase implementation plan plus shipped Phase 1–4 progress/learnings. |
| 04 | [fake-runner-test-mode-ux.md](./04-fake-runner-test-mode-ux.md) | Current fake usage, desired `stores test` UX, battlescar case catalog, manual stress test results. |
| 05 | [fake-runner-tdd-modus-operandi.md](./05-fake-runner-tdd-modus-operandi.md) | Stage 1/Stage 2 TDD boundary and live harness issue list. |

## Tensions

- **Subprocess realism vs implementation simplicity:** in-process fake would be easier, but would hide the liveness/PID/marker/watchdog bugs the harness is meant to expose.
- **Fake PASS safety:** fake reviews must move test rows, but must never masquerade as production review authority.
- **Live DB drift:** fake external review hit the existing SQLite CHECK constraint that excluded `runner=fake`; schema YAML alone was not enough.
- **Daemon binary skew:** `STORES_LLM_OFF=1` initially reexeced into a stale private daemon binary and launched real Claude, proving no-LLM mode needs post-reexec preflight.
- **Stall classification:** `stall-no-heartbeat` surfaced as payload invalid rather than a clear liveness/watchdog failure.

## Open Threads

- Build/finish `stores test` as the first-class operator scenario runner for named live cases and suites.
- Ensure fake-mode preflight validates both current CLI and private daemon binaries, including `stores-fake-agent` sibling resolution.
- Repair/migrate live DB CHECK constraints so `external_reviews.runner='fake'` persists cleanly.
- Make no-heartbeat stalls classify visibly as liveness/watchdog failures in status, blocked reason, and telemetry.
- Add inverse sentinel for explicit real-review opt-back-in under `STORES_LLM_OFF`.
- Keep fake runs valid for engine reliability analysis but excluded from model-quality analysis.

## Tomorrow

- Treat current mode as **Stage 1** until live happy-path fake runs reach `integrated` repeatedly through the real repo DB and daemon path with zero real LLM subprocesses.
- After Stage 1 passes, move to Stage 2 battlescar scenarios: stale freshness, silent zombie/no heartbeat, duplicate drive, dirty/merge conflict, stale/dead markers, and external-review convergence.
