# Handover — ADR 0001 task engine architecture and next main-lane work

**Date:** 2026-05-10
**Type:** handover / architecture state

## Summary

ADR 0001 is now the source of truth for the task-engine architecture direction:

- `docs/adr/0001-task-engine-lifecycle-and-integration.md`
- `docs/task-engine-architecture.md`

The first implementation task is active:

- `T144` — Task engine lifecycle v1: schema fields, watch, and `main_branch` ResourceLock

The immediate engine-friction fix shipped on main:

- `2e72f33` — extend runner no-output liveness timeout from 180s to 600s

## Current state

- T144 was opened directly as a T3 task from the ADR, not via the earlier small observation. The earlier too-small observation was closed as `wont_fix` because the architecture migration needs one real vertical slice, not a ceremonial T2 doc task.
- T144 briefly blocked as `drive_failed:no_output_idle_182s_threshold_180s` while its planner was still alive. The no-output threshold was too aggressive for a large planning stage.
- Main now has a 600s runner no-output default, installed to both normal and private daemon binary paths.
- T144 was resumed and is back at `planning`, active, next=`planner`.

Check with:

```bash
stores tasks status T144
stores engine plan-start
pgrep -af 'stores agents run|stores tasks drive T144|T144'
```

## Work ethic / boundary for next agent

Do not force every meta-substrate improvement through a full task ceremony.

Use substrate tasks for real implementation work that should exercise and repair the engine, especially lifecycle/schema/daemon/subscriber behavior. But direct main-lane work is appropriate for focused high-substrate meta fixes and docs that unblock the engine or clarify operator/agent behavior.

Good direct-main candidates:

- ADR/doc consolidation where the human and agent have already worked through the design.
- Small engine-observability or operator-trust fixes.
- Focused triage/inlet workflow design if it does not change schema/runtime yet.
- Worklog / handover / engine-health updates.

Good substrate-task candidates:

- T144 and other schema/runtime lifecycle changes.
- Daemon/subscriber behavior.
- ResourceLock implementation.
- Watch/UI behavior with meaningful code changes.
- Any change that should prove the dogfood path.

The guiding principle: work on high-leverage substrate meta issues that unblock the engine and reduce future ceremony. Do not let ceremony prevent fixing the ceremony machine.

## ADR 0001 references only

The current task-engine simplification work is grounded in:

- `docs/adr/0001-task-engine-lifecycle-and-integration.md`
- `docs/task-engine-architecture.md`

Those docs settled:

- task lifecycle: `queued | active | integration | done`
- active steps: `planning | planning_review | coding | coding_review | wrapping`
- integration steps: `refreshing | task_review | testing | merging | deploying | verifying`
- `task_reviewer` as whole-task review worker, distinct from `coding_reviewer`
- blocked/waiting as overlay, not lifecycle
- ResourceLock as DB-backed primitive for truth-mutating resources
- `main_branch` merge mutation serialized by resource lock, not by making all integration globally capacity 1
- human acceptance as explicit signature or human-ratified delegated policy, not silent automation

## User priorities to preserve

The user's scratch priorities remain:

- Simplify the massively complicated flow/state-change model into a unified streamlined flow. ADR 0001 is the first concrete step.
- Build a triage agent/workflow inside the system: clear rules for investigation, architecture review, confirmed, wont_fix, duplicate, needs_info, and route-to-task.
- Rethink fast-track/T1 so tiny deterministic fixes do not drown in reviews and ceremony.
- Rework external review timing and freshness so main moving does not invalidate unrelated reviews. ADR 0001 names `task_review` and merge-time freshness as the architecture direction.
- Let outside agents that file observations subscribe/monitor their filings so duplicate/wont_fix/routed status feeds back to them in real time.
- Make watch/observation/log summaries lightweight and immediately useful, roughly in the spirit of the `what-why-where` skill.

## What can happen next on main

The first priority is already underway through T144: implementing ADR 0001 as real task-engine behavior.

The second priority — triage agent/workflow — is a reasonable direct-main architecture/design lane next, if kept to doctrine/docs and not schema/runtime mutation. It should clarify:

- what enters the inlet;
- what decisions triage may make autonomously;
- when to route to architecture review;
- what duplicate/wont_fix/needs_info mean;
- how external agents subscribe to outcomes;
- what a fast-track/T1 path should do without excessive ceremony.

Recommendation: write a focused triage workflow architecture doc/ADR on main, reviewed directly, before opening implementation tasks. Do not start by coding a triage agent until the routing doctrine is as clear as ADR 0001.

## Follow-ups

1. Watch T144. If it blocks again for liveness, inspect process state before resuming.
2. Let T144 planner produce a real plan; do not hand-edit its task markdown.
3. Consider a direct-main triage workflow ADR/doc next.
4. Keep `docs/engine-health.md` updated when T144 or triage decisions change the long-running health picture.
