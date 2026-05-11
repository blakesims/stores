# T148 Remaining Runner Safety Partials Plan

**Date:** 2026-05-11
**Type:** plan

## Summary

After the T148 duplicate-dispatch/live-status repair, several partial risks remain. The most urgent is binary update isolation: updating/installing the stores binary from `main` must not kill, stale-block, or confuse workers already running in task worktrees. A running worker should either continue using the binary it started with, or adopt a newer binary only at a safe handoff boundary after the current runner completes.

This plan batches the remaining partials into focused phases with worker→reviewer chains and live validation.

## Current failure signal

Another agent observed after a main binary update:

```text
no T148 drive process
no pi runner
no cargo test
T148 blocked with drive_failed:stale_binary_inode
```

That means the binary update effectively killed or invalidated backend work, while stale live marker state could still imply a runner was active. This violates the desired worker-isolation contract.

## Desired global invariants

1. **Binary isolation:** Updating/installing `stores` on `main` must not kill or stale-block already-running task workers in worktrees.
2. **Safe binary adoption:** A worker may use a newer binary only after its current runner finishes and the next dispatch starts; never mid-run.
3. **No duplicate same-worktree workers:** Autonomous and manual drive paths should refuse to start a second worker for the same task/worktree when a live owner exists.
4. **Dead-worker truth:** Stale running markers must not be reported as live when no drive/runner process exists and the marker is old/dead.
5. **Malformed final output is visible:** Bad final payloads become a typed, visible tooling/payload error and do not silently stall or masquerade as runner liveness.

## Phase 1 — binary isolation / stale-exe semantics

Problem: stale binary detection currently blocks/kills a worker when the installed binary changes underneath it. This is backwards for task work: running workers should be pinned to their launch binary identity until they complete.

Scope:

- Inspect stale-exe guard paths in daemon/agents_run/auto_drive/drive loop.
- Distinguish **daemon control-plane stale binary** from **already-running task worker binary**.
- Pre-spawn checks may require a fresh candidate binary before starting new work.
- Post-spawn/running workers should not be marked `drive_failed:stale_binary_inode` merely because main/global binary changed.
- If a drive process itself detects its own executable inode no longer matches the install path, it should continue or report advisory identity drift, not fail the task, unless the binary is corrupted/unusable before spawn.

Acceptance criteria:

- Test: start/fake a drive with old binary identity, mutate installed binary identity, verify existing in-flight drive is not marked failed solely for stale inode.
- Test: daemon/auto-drive pre-spawn still reexecs or refuses stale control-plane binary before starting new work.
- Test: status/watch surfaces binary drift as advisory/identity info, not task-blocking failure, for already-running workers.
- Live validation: updating/installing main binary while T148 has a runner does not kill/block that runner.

## Phase 2 — manual drive / same-worktree singleton guard

Problem: Track A guarded autonomous dispatch, but manual `stores tasks drive T148` and cross-task same-worktree cases may still spawn duplicates.

Scope:

- Apply the same central dispatch eligibility/live-owner checks to manual drive entry where safe.
- At minimum, warn/refuse when the same task has live drive_pid/live lock/fresh running marker.
- Prefer also refusing if another nonterminal task points at the same workspace_path and has live owner evidence.
- Provide an explicit override flag only if truly necessary and visibly dangerous.

Acceptance criteria:

- Manual `stores tasks drive <id>` refuses when that task already has a live runner/drive owner.
- Manual drive refuses or loudly warns when another live task owns the same workspace_path.
- Autonomous behavior from prior fix remains unchanged.
- Tests cover same-task manual duplicate and same-worktree live-owner shape.

## Phase 3 — dead marker truth + malformed final output visibility

Problem: Live markers can outlive processes, and malformed final output/payload errors need to be visible as payload errors rather than stale liveness.

Scope:

- `stores tasks status` / `stores runs current` should indicate stale/dead marker if marker says running but no corroborating process/lock exists and semantic heartbeat is stale.
- Do not select ancient running marker as authoritative live runner forever.
- Ensure runner payload errors are persisted and displayed via current run status / agent run telemetry / task block reason.

Acceptance criteria:

- Test: running marker with stale semantic status and no live pid is rendered as stale/dead/advisory rather than fresh live work.
- Test: malformed `final_output` produces visible payload_error/tooling failure and does not advance the task.
- Existing happy path structured final_output still advances.

## Phase strategy

Run phases as sequential batches because Phase 1 touches core stale-binary semantics and may affect dispatch/run logic. Phase 2 can follow once Phase 1 defines the live-owner primitive. Phase 3 can run after Phase 2 or in parallel only if it stays confined to status/runs display and runner output parsing.

For each phase:

1. Oracle review/hardening.
2. Worker implementation in isolated worktree.
3. Reviewer pass.
4. Merge to main.
5. Targeted tests + build/install.
6. Live validation against T148 where applicable.

## Non-goals

- Full ER reconciliation/import repair.
- Full resume semantic redesign, except where manual drive guard intersects recovery safety.
- Large binary identity UX overhaul beyond what Phase 1 requires to prevent task-killing stale-exe behavior.

## Follow-ups

- ER reconciliation/import remains next after runner safety.
- `resume --no-dispatch` remains important if manual drive guard does not cover recovery-triggered dispatch.

## Oracle hardening incorporated

Oracle reviewed and sharpened the plan:

- Add mandatory **Phase 0 evidence/mapping** before code: enumerate every stale-binary emitter and classify it as daemon startup, pre-dispatch, running drive, runner child, watchdog/postcondition, etc.
- Rename Phase 1 to **stale-exe boundary split: pre-spawn fatal, post-spawn advisory**. Do not weaken daemon stale-binary protection before spawn, but never block a task merely because the global install path changed after a drive started.
- Pair stale/dead marker truth with Phase 1/validation, because otherwise status can confuse a killed backend with a stale marker.
- Use one live-owner primitive across autonomous dispatch, manual drive, and stale marker/dead-owner classification.
- Split malformed `final_output` into a lower-priority subphase if needed; it is important but less urgent than preventing binary-update worker loss.
- Add hard acceptance: after binary update while a fresh live owner exists, there must be no new `mark_drive_failed` transition with `stale_binary_inode`.
- Manual duplicate guard must cover same workspace, not only same task. Any override must be scary, explicit, logged, and unavailable to `ai_autonomous`.

## Revised phase order

### Phase 0 — evidence and stale-binary callsite map

- Grep/map every `stale_binary_inode`, stale-exe, reexec, and `STALE_DAEMON` callsite.
- Query transition history for T148's `drive_failed:stale_binary_inode` source path.
- Capture process/marker/lock state before changing code.
- Classify stale checks as pre-spawn fatal or post-spawn advisory.

### Phase 1 — stale-exe boundary split: pre-spawn fatal, post-spawn advisory

- Preserve daemon/control-plane pre-spawn reexec/refuse behavior.
- Running task invocation binary drift becomes advisory and must not write `mark_drive_failed` / `drive_failed:stale_binary_inode`.
- Watchdog must not classify an alive/fresh-heartbeating drive as failed solely because its executable inode differs from current install path.
- Test: fake/live owner + mutated install identity => no task block, no stale_binary_inode transition.
- Live proof: update/install main binary while T148 runner is live; same runner continues or naturally completes; no duplicate; no stale_binary_inode block.

### Phase 1B — stale/dead marker truth

- Marker `status=running` + stale semantic heartbeat + no live PID/lock should render as stale/dead evidence, not fresh live work.
- Stale uncorroborated markers do not hold dispatch forever.
- Preserve markers as audit evidence; do not silently delete.

### Phase 2 — manual drive and same-worktree singleton guard

- Manual drive refuses same-task live owner.
- Manual drive refuses same-workspace live owner from another nonterminal task.
- Stale uncorroborated marker can warn but should not permanently wedge manual recovery.
- If override exists, it must require non-autonomous invoker and be logged loudly.

### Phase 3 — malformed final_output visibility

- Bad payload remains visible as payload_error/tooling failure.
- Bad payload does not advance state.
- Bad payload does not leave an eternal running marker.
