# Manual Engine Control North Star

**Date:** 2026-05-09
**Type:** iteration-3 worklog
**Status:** active

## Seed Goal

Manually drive the stores engine with Blake in the loop, while preventing the engine from picking up random tasks or spawning uncontrolled work.

## North Star

Restore operator trust by making every next state transition intentional, visible, and reversible-at-the-process-level: no broad daemon restarts, no surprise auto-promotions, no blind drive loops, and no hidden reviewer/drive work. The queue should move only when Blake and the assisting agent choose a specific row and a specific verb/path.

## Operating Mode

- **Daemon stays stopped** unless explicitly restarted for a narrow, predeclared action.
- **No broad `stores agents run`** while queue shape is unstable.
- **No blind `stores tasks drive` loops** on arbitrary rows.
- **One row at a time** unless Blake explicitly batches a mechanically-identical action.
- **Prefer read/inspect → propose → execute** for any task lifecycle move.
- **Keep useful running work only when already started by Blake/engine-op; do not spawn new work opportunistically.**
- **No raw-SQL writes.** Read-only SQL is allowed for diagnosis.

## Immediate Control Objective

Find and use a safe manual-control pattern that prevents random task pickup while still allowing deliberate progress on selected rows.

Candidate patterns to verify from source/CLI before use:

1. Direct task verbs only (`accept`, `reject`, `abandon`, `submit-review`, `recover-stale-base`, etc.) with daemon stopped.
2. Direct single-row `stores tasks drive <TID> --max-iters N` only after confirming the row is safe to drive.
3. Manual review path for in-review rows when ER/codex lane would otherwise require daemon.
4. If ER must run, prefer a narrow external-review dispatch path or manual codex invocation over daemon restart.

## Current Known Queue Shape

Last verified snapshot after engine-op pause:

- Daemon: stopped.
- Drives: reaped.
- Monitors: stopped except agent-comm watch.
- `T125`: `in_review`, ER357 PASS; likely ready for Blake/U3 accept.
- `T127`: `in_review`, ER358 PASS; likely ready for Blake/U3 accept.
- `T124`: `in_review`, has executor commit + wrap; no ER row found yet; needs manual review or narrow ER path.
- `T126`: `blocked`, substantive code-review FAIL; needs salvage-vs-abandon decision.
- `T123`: `blocked`, stale-binary/I033 contamination risk; hold.
- `T116`: `blocked`, T1 silent_zombie; hold until WIP low.
- `T108`: `blocked`, contract drift / repeated NEEDS_WORK; needs revise or abandon/remint decision.
- `T128`: was `executing`; engine-op says all drives reaped including T128; needs fresh status check before action.
- `T134`–`T137`: fresh planning remints caused by auto-promote startup sweep after abandoned tasks; zero cycles; likely queue-curation candidates, not automatic drive candidates.

## Guardrails

- Do not restart daemon broadly.
- Do not run `stores agents run --once` as a convenience; it can fire startup sweeps and remint/scaffold/drive multiple rows.
- Do not abandon/accept/reject without Blake grounding where required.
- Do not resume I033-shaped rows with non-empty rejected plans until the manual-main I033 fix is confirmed landed.
- Do not use `git add -A` / `git add .`; stage explicit files only.

## First Verification Steps

1. Confirm no daemon/drive processes are running.
2. Confirm live task statuses from CLI.
3. Inspect config/source for any supported pause/maintenance/narrow-dispatch knobs.
4. Decide the first safe manual move with Blake.

## Done When

- We have a documented manual-control procedure for progressing selected rows without broad daemon pickup.
- Random task pickup is prevented by process state and verified by CLI/process checks.
- The next row action is chosen deliberately with Blake, not by daemon heuristics.
