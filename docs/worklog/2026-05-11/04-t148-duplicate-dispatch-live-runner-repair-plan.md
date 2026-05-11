# T148 Duplicate Dispatch Live Runner Repair Plan

**Date:** 2026-05-11
**Type:** plan

## Summary

T148 is intentionally running, but the live run exposed two engine regressions we can now test directly:

1. **Duplicate executor dispatch:** two `stores tasks drive T148 --invoker ai_autonomous` processes and two `pi_runner --role executor` processes ran at the same time against the same worktree.
2. **Misleading live status:** `stores tasks status T148` displayed a completed planner `final_output` as the live runner while the executor was actually running and emitting heartbeats/tool events.

This repair should be test-driven against those exact shapes. The outcome is not just code that compiles; the installed binary must prove that the current T148 run no longer duplicates, stale live markers do not hide active runners, runner PID/status is correct, and the task can continue through the engine without dead-worker confusion.

## Current evidence

Observed process shape:

```text
471567 /home/blake/.local/share/stores/bin/stores tasks drive T148 --invoker ai_autonomous
510248 node .../pi_runner.mjs --role executor --cwd ...stores-T148-auto-promoted-l568 ... /tmp/stores-pi-T7MGc4
530874 /home/blake/.local/share/stores/bin/stores tasks drive T148 --invoker ai_autonomous
530908 node .../pi_runner.mjs --role executor --cwd ...stores-T148-auto-promoted-l568 ... /tmp/stores-pi-m9B7rp
```

Observed status bug:

```text
Live runner: role=planner runner=claude-code:opus status=completed ... event=final_output
```

But executor marker/status existed:

```json
current-T148-executor.json: { "status": "running", "updated_at": "2026-05-11T04:30:09Z" }
status.json: { "current_activity": "tool:bash", "last_event_at": "2026-05-11T04:40:33Z", "last_event_type": "heartbeat" }
```

Transition history confirms planner/plan-review advanced correctly:

```text
planning -> plan_review   submit-plan        04:26:43
plan_review -> ready      submit-plan-review 04:27:06
ready -> executing        start              04:27:06
```

So `final_output` is not failing to advance the planner; status is selecting the wrong marker and duplicate executor dispatch is the real safety issue.

## Problem statement

The task engine lacks a single hard invariant for work-start eligibility and live-run selection:

- A work-starting path can spawn a runner even while another drive/runner for the same task is already live.
- Dispatch lock state is not enough, or is being reused/overwritten, to prevent duplicate live processes.
- Live status chooses the newest marker `updated_at`, even if that marker is completed and a different role is currently running.
- Executor marker `updated_at` can remain at spawn time while semantic `status.json.last_event_at` advances, making active work look stale.

## Desired invariants

1. **Singleton work-start:** at most one live work-starting drive/runner per task role, and preferably at most one active drive per task, unless an explicit future multi-runner mode is introduced.
2. **Activation gate:** auto-dispatch paths only spawn when `activation=active` and the task state is work-startable.
3. **Observable skip:** if a dispatch is skipped due to existing live runner/lock/inactive state, it is logged/recorded as a skip, not converted into a task block.
4. **Correct live-run display:** status prefers running markers over completed markers, and freshness uses semantic `status.json.last_event_at` when present.
5. **Fresh marker or robust selection:** either live-event writes update marker `updated_at`, or all readers consistently use semantic status freshness instead of marker freshness.

## Proposed tracks

### Track A — dispatch eligibility / duplicate prevention

Implement or reuse one central guard for work-starting dispatch. Wire it into the auto-drive / engine-runner path that spawned the second executor.

Acceptance criteria:

- An active executing task with an existing live executor marker/process/dispatch lock does not spawn a second executor on `stores agents run --once` or equivalent daemon tick.
- An inactive task with workspace does not auto-drive.
- Skip reason is observable and names the reason (`inactive`, `existing_live_runner`, `existing_dispatch_lock`, etc.).
- Regression test models the T148 shape: existing running executor marker + dispatch lock + auto-drive tick => no new drive spawn and task not blocked.

### Track B — live status selection / stale marker repair

Fix `stores tasks status` and `stores runs current` selection so the active executor is displayed instead of a completed planner.

Acceptance criteria:

- Given a completed planner marker with newer marker `updated_at` and a running executor marker with older marker `updated_at` but newer `status.json.last_event_at`, status selects executor.
- `Live runner:` line includes role=executor, status=running, activity/tool/event from semantic status.
- Tests cover completed planner + running executor, and tolerate missing semantic status files.

### Track C — live marker freshness

If low-risk, update current-run marker `updated_at` whenever live semantic status is updated. If this touches too much runner plumbing, document as follow-up and rely on Track B's semantic freshness.

Acceptance criteria:

- Running executor marker no longer appears stale while events arrive, or readers no longer depend on marker `updated_at` for freshness.

## Batch strategy

Run Track A and Track B/C as independent worker→reviewer chains if they do not overlap. If overlap appears in `src/cli/runs.rs` / `src/handlers/status.rs` / runner sink code, combine B/C and keep A separate. Land reviewed changes, install binary, then validate against current T148.

## Validation plan

After implementation and install:

```bash
PATH=/usr/bin:$PATH cargo test <targeted tests>
PATH=/usr/bin:$PATH cargo build --locked
PATH=/usr/bin:$PATH cargo install --path . --locked
stores tasks status T148
pgrep -af 'stores tasks drive T148|pi_runner.*T148|stores-T148'
stores agents run --once
pgrep -af 'stores tasks drive T148|pi_runner.*T148|stores-T148'
stores tasks status T148
```

Expected post-fix proof:

- Status shows the active executor when one is running.
- A daemon/agent tick does not create a new duplicate T148 drive/runner.
- Existing stale completed planner marker does not mask active executor.
- PID/runner paths shown by status correspond to the actual live process when available.
- No task block is created merely because dispatch was skipped.

## Non-goals for this batch

- Full `resume --no-dispatch` semantic change.
- External review reconciliation from `blocked`.
- Manual external-review PASS import.
- Binary identity/version overhaul.

Those remain important but should follow once the active duplicate-dispatch and observability regression is fixed.

## Oracle hardening incorporated

Oracle reviewed this plan and hardened it with these additions:

- Add **Phase 0 containment/evidence** before implementation validation. Capture process trees, status, current-run markers, and dispatch locks. Pick a canonical executor so validation is not racing two executors in one worktree.
- Treat duplicate prevention as ownership, not just activation: alive `drive_pid`, unfinished dispatch lock with alive pid, and fresh running current-run marker are each live-owner evidence.
- Do not silently overwrite an unfinished lock with dead pid if a fresh running marker exists; classify/log the discrepancy instead.
- Put live-run selection in `src/cli/runs.rs`, not only status rendering, so `stores runs current` and `stores tasks status` share the same selector.
- Selection order: running markers first; within running, semantic `status.json.last_event_at`; then marker `updated_at`; completed/failed only win if no running marker exists.
- Do not expand Track C into broad runner plumbing. Either tiny marker freshness update or defer once Track B no longer depends on marker freshness.
- Do not claim full PID-reuse safety; prove the observed live-owner duplicate shape is blocked.

## Phase 0 containment/evidence log

Before code changes, capture current duplicate state and remove validation ambiguity by preserving the canonical executor and terminating the older duplicate tree.

Canonical executor selection rule for this incident:

- dispatch lock pid currently points at `530874`, the newer drive tree;
- `current-T148-executor.json` session is `9577850f-e8bd-4d49-8a58-3ab0b7f962aa`, started at `04:30:09Z`, matching pid `530874`;
- older duplicate tree `471567 -> 510248` is not the current lock owner.

Therefore preserve `530874 -> 530908` and terminate `471567 -> 510248` before validation.
