# Handover — pi-architect

**Date:** 2026-05-08
**Type:** handover
**Role:** pi-architect

## Active / next thread

Current session thread was:

`/home/blake/repos/.agent-comm/threads/2026-05-08-01-2026-05-08-2-agent-comms.md`

Next session thread from engine-controller wind-down:

`/home/blake/repos/.agent-comm/threads/2026-05-08-01-stores-2026-05-09-session.md`

## Current architectural state

Posture remains: **front-of-engine fidelity first**, but afternoon work changed the immediate diagnosis. A major substrate fix landed:

- `c0f45ff Fix revision agent briefs` — planner / executor / code_reviewer revision briefs now include the actual artifact under review in `## Revision Context`, not just prior review commentary.

This likely supersedes or narrows earlier diagnoses around:

- I022 — executor/REVISE respawn missing review findings.
- I026 — planner literal-invariant drift.
- T106/T108 plan-review cycle-limit behavior.

Next Pi should not blindly promote I022/I026 as originally framed. Re-evaluate them against `c0f45ff` first.

## Engine state at wind-down

From engine-controller wind-down (`docs/worklog/2026-05-08/handover-engine-controller.md`, supersedes morning version):

- Daemon PID `1809888` alive under `c0f45ff`; monitors armed.
- T107 / L173 is the live empirical test of `c0f45ff`: fresh post-resume drive `1812044`, at executing → code_review boundary at wind-down.
  - First read next session: did cycle 1 land a targeted `cluster_keys.rs:27-33` fix?
  - If next external review PASSes or finds something new, `c0f45ff` is validated.
  - If the same `cluster_keys.rs:27-33` finding repeats, the problem is now genuine capability/contract semantics, not missing revision context.
- T108 / L499 remains parked at plan-review cycle limit. Pi ruled not to bypass plan_review with `plan-from-file`; either park/abandon or use a reset-to-planning path only with Blake confirmation.
- WIP at wind-down: 1.

## Shipped / important afternoon changes

- `87f3667` — I023 watchdog gate: do not mark in-review tasks failed while external_review lane is active.
- `45224e1` — T098/L480 merged close-out-of-band; cockpit/watch attention work shipped.
- `5e4753f` — T105/L498 recover-stale-base shipped; already useful in production.
- `98da6b5` — engine-controller SOP convergence-stall update.
- `c0f45ff` — revision briefs include prior artifact/context; major root-cause fix.

## Pi rulings that matter next

- Path A substrate-native `external_reviews` remains canonical. Do not re-promote reviewer-runner by default.
- Keep WIP conservative until `c0f45ff` is validated by T107.
- T108: do **not** inject a plan to skip plan_review. If continuing T108, acceptable path is reset-to-planning so normal plan_review still applies, and only with Blake confirmation because it touches lifecycle/authority shape.
- L485/T106/T108: original broad gatekeeper-drain contract was too much for T2; Slice 1/2 split is still conceptually right, but c0f45ff may change whether T108 can be recovered.
- Repeated identical REVISE after `c0f45ff` is stronger evidence of model/contract capability failure, not feedback relay failure.
- “Race the operator” is invalid architecture: if success depends on accepting/resuming before watchdog/reconciler flips state, fix the control plane or use grounded close-out-of-band.

Relevant ruling msg ids in active thread:

- `msg_ccf83e9e` — approve I023 repair before racing T098 accept.
- `msg_ed0a9435` — split L485/T106 rather than force rejected plan.
- `msg_b171ada2` — T108 manual replacement plan idea, but preserve literal invariant.
- `msg_7cef2d5e` — do **not** add `plan-from-file` bypass / skip plan_review; park T108 or reset-to-planning only with Blake.
- `msg_401e9671` — convergence-stall SOP amendments.

## Pending rows / cohorts to re-evaluate

- I022 — likely superseded/narrowed by `c0f45ff`; confirm before routing/promoting.
- I026 — likely narrowed: with revision context fixed, continued literal-invariant drift is a real capability/prompt issue, not missing prior artifact.
- I024 — independent: auto-resolve subscriber missing terminal-success edges (`accepted`/`closed_out_of_band`/possibly `schema_migrated`) for ready observations. Cohort includes stale ready rows such as L032/L043/L053/L056/L069/L113/L134/L144/L193.
- I025 — independent: auto-promote one-shot edge missing after task abandonment + observation re-ratification. L485 orphan is evidence.
- L500 — Slice 2 gatekeeper-drain failure-semantics hardening; draft only, depends on L499/T108/Slice 1 ship.
- L485 — orphaned ready observation after T106 abandoned; known I025-class case.

Queue-curator reported no active curation decisions pending. See `docs/worklog/2026-05-08/handover-queue-curator.md` once refreshed.

## First step for next Pi

1. Join/watch next thread: `/home/blake/repos/.agent-comm/threads/2026-05-08-01-stores-2026-05-09-session.md`.
2. Read engine-controller handover: `docs/worklog/2026-05-08/handover-engine-controller.md`.
3. Check T107 result under `c0f45ff` before making priority calls.
4. Re-evaluate I022/I026 scope/supersession.
5. Decide with engine-controller whether T108 should remain parked, be abandoned/refiled, or use a reset-to-planning path with Blake confirmation.

## Do not do

- Do not widen WIP before T107 validates the revision-context fix.
- Do not bypass plan_review with manual plan injection.
- Do not fold I024/I025 into the c0f45ff cohort; they are independent subscriber-edge bugs.
- Do not let queue-curator implement code or make architecture decisions.
- Do not raw-SQL write the substrate DB.
