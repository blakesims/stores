# T146 Engine Friction Audit And T148 Start

**Date:** 2026-05-11
**Type:** note

## Summary

T146 (ADR 0001 task schema simplification) did land on `main`, but only after the engine became non-convergent enough that we used the explicit escape hatch: manual merge/install/migrate followed by `stores tasks close-out-of-band`. No raw SQL writes were used.

This note is for the next engine-repair agent. It captures the friction observed during the T146 handoff, the duplicate/related rows a scout subagent found, evidence commands to verify the failure modes, and concrete repair hints.

T148 (ADR 0002 completion from L568) was created after T146 landed. Its worktree starts exactly at the T146 merge commit, so ADR 0002 begins from the completed ADR 0001 base.

## Final shipped state

T146 landed on `main` as:

```text
8f6883bcd3e5020f5e2ec3f48121f136d7fcf1b5 Merge T146 ADR0001 lifecycle simplification
```

Post-merge actions performed:

```bash
PATH=/usr/bin:$PATH cargo build --locked
PATH=/usr/bin:$PATH cargo install --path . --locked
stores migrate --apply
stores tasks close-out-of-band T146 \
  --commit 8f6883bcd3e5020f5e2ec3f48121f136d7fcf1b5 \
  --invoker ai_with_human \
  --approve-token <provided-by-user>
```

Current T146 status after close-out:

```bash
stores tasks status T146
# T146 status=closed_out_of_band lifecycle=done ... Disposition: Terminal shipped (out of band)
```

Important: main had unrelated dirty tracked files before the manual merge. They were preserved with:

```text
stash@{0}: On main: pre-t146-emergency-main-dirty-20260511110410
```

Untracked task projection directories were left untouched.

## T148 start confirmation

T148 is the auto-promoted ADR 0002 task from L568:

```bash
stores observations show L568 --json | jq '{status,task_id,contract_state:.intent_contract.contract_state}'
# {"status":"ready","task_id":"T148","contract_state":"ready"}

stores tasks status T148
# T148 status=planning ... Activation: inactive
```

Its worktree starts from the T146 merge commit:

```bash
git -C /home/blake/repos/experiments/stores-T148-auto-promoted-l568 rev-parse HEAD
# 8f6883bcd3e5020f5e2ec3f48121f136d7fcf1b5

git -C /home/blake/repos/experiments/stores-T148-auto-promoted-l568 log --oneline -3
# 8f6883b Merge T146 ADR0001 lifecycle simplification
# 177f75c T146 revise: rerun stale freshness substeps
# 46b61cd T146 revise: document stale freshness retry route
```

So yes: T148 starts exactly where ADR 0001/T146 finished.

Caveat: auto-promote/engine-runner spawned an auto-drive for T148 even though the task is inactive. I killed that drive and resumed T148 back to planning. Current intended state is inactive/planning, no active T148 runner.

## Duplicate/related rows found by scout subagent

A read-only scout checked for duplicate observations/tasks. Candidate related rows:

### Silent-zombie / watchdog false positives

- `L569` open — `drive-failed: task T146 silent_zombie on branch 'feat/T146-auto-promoted-l567'`.
- `L571` open — `drive-failed: task T148 silent_zombie on branch 'feat/T148-auto-promoted-l568'`.
- `L550` open — watchdog flips `in_review -> blocked` while external review row still running.
- `L552` open — broader silent-zombie pattern under WIP/cycle pressure.
- `L517` open — older keeper for silent-zombie cluster; folded/resolved related rows include `L521`, `L524`, `L526`, `L531`.
- `L548` open — daemon restart did not type-close stale `dispatch_locks`.
- Related historical tasks: `T030`, `T049`, `T112`, `T113`, `T116`, `T098`.

### External-review stale-base / non-convergence

- `L487` resolved — Codex ran against unrebased branch; stale-base REVISE was artifact.
- `L488` resolved — auto-codex review should rebase/hold before treating missing mainline code as regression.
- `L498` resolved — stale-base ER rows had no operator-callable recovery verb / permanent tooling-held loop.
- `.claude/skills/engine-controller/SKILL.md` already has convergence-stall doctrine for stale-base persistence and watchdog/ER races.

### Auto-promote / auto-drive ignition

- `T140` integrated — activation gate task.
- `L557` resolved / `T142` integrated — auto-drive redispatch must honor `agents.yaml`.
- `L558` open — spawn-side binary preflight.
- `L563` open — auto-drive uses `workspace_path` without `--meta` routing.
- `L122` open — manual drive lacks `drive_pid`; auto-drive can race-spawn duplicates.

### Manual ER PASS / accept precheck gap

Scout did not find a clear duplicate for: manual review PASS exists, but `tasks accept` cannot use it because the accept precheck only trusts the latest non-superseded `external_reviews` row.

Closest old row: `L061` (acceptance/precheck themed), but it does not cover this exact failure.

## Failure modes observed in this session

### 1. Watchdog blocks a row while a runner is still producing output

T146 had an orphaned code-reviewer that returned PASS after the task row had already been marked:

```text
blocked_reason=drive_failed:silent_zombie_pid_dead
```

The PASS was visible in:

```text
.stores/runs/46af3f4a-2e31-4293-b8d4-625a0ae1225d.jsonl
```

But it was not recorded in the task row until we manually resumed to `code_review` and submitted the already-produced PASS with `stores tasks submit-review`.

A surgical main fix was committed before the T146 merge:

```text
073cd9c fix: defer silent-zombie on recent heartbeat
```

That fix defers dead-`drive_pid` silent-zombie classification while the lock has a recent heartbeat. However, later external-review paths still drove T146 into:

```text
in_review -> executing (submit-external-review)
executing -> blocked (mark_drive_failed:silent_zombie_pid_dead)
```

This means the broader state/reconciliation problem remains.

Verify via:

```bash
sqlite3 .stores/db.sqlite "
select id, from_status, to_status, verb, invoker, occurred_at, substr(actor_note,1,160)
from transition_history
where store='tasks' and display_id='T146'
order by id desc limit 40;"
```

### 2. `resume` is dangerous because it can spawn new runners immediately

When we resumed T146 from `blocked` to `code_review`, the daemon auto-spawned a fresh code-reviewer before the orphaned PASS could be submitted. We had to kill the duplicate drive/runner and then submit the existing PASS.

Repair hint:

- Add an operator-safe "record orphaned result" or "resume without dispatch" path.
- Or make `resume` write a recoverable state with dispatch inhibited until an explicit next verb.
- At minimum, expose a CLI warning/dry-run showing what subscribers will fire after resume.

### 3. External review row can transition the task to `executing`, then fail to submit back if task is blocked

Official ER rows repeatedly did this shape:

```text
in_review -> executing via submit-external-review
ER row completes REVISE
submit-external-review cannot apply because task is now blocked
```

Example error from ER402/ER403/ER405/ER406:

```text
Error: no transition from 'blocked' via verb 'submit-external-review' with gate 'REVISE' found in schema; verb 'submit-external-review' is reachable from {in_review}
```

Repair hint:

- External review should not move the task into generic `executing` if its result reconciliation can race with watchdog/drive failure.
- Treat external review as an `external_reviews` lane with task row overlay/backpressure, or add a valid recovery edge from `blocked` for ER result reconciliation.
- If the task is blocked by `drive_failed:silent_zombie_pid_dead` but the ER row reached terminal REVISE/PASS, reconciliation should be idempotent and should not lose the ER verdict.

### 4. Official ER loop became non-convergent and design-finding-driven

After T146 was rebased on main in `/tmp/stores-T146-clean-review`, official ER rows kept finding real but increasingly architectural issues:

- `ER399` REVISE: framework migrate legacy `integrating`, freshness head SHA, TUI primary `none` fallback.
- `ER400` REVISE: in-module framework migrate test stale.
- `ER401` REVISE: wrapping states incorrectly lifecycle=`integration` before release.
- `ER402`/`ER403` REVISE: main lock released before push/finalization.
- `ER404`/`ER405` REVISE: stale freshness should rerun required substeps, not generic block.
- `ER406` REVISE: in-place substep mutation lacks subscriber-visible edge and can stall.

The loop was no longer converging toward T146 closure; it was surfacing follow-on engine architecture disputes. We stopped and used close-out-of-band.

Repair hint:

- Add an "ER finding is new task / follow-up, not release blocker" classification, with human/operator override and durable provenance.
- Make official ER support a scoped review target. The manual PASS reviewed the committed diff and prior findings, but official ER kept broadening into new integration-lane architecture.
- Add a first-class manual external-review PASS import path, so a reviewed transcript can satisfy accept precheck without raw SQL.

### 5. Accept precheck cannot use a manual PASS

`tasks accept` rejected even with the user-provided token because it checks the latest non-superseded `external_reviews` row and requires `status='passed' AND verdict='PASS'` at current head.

Relevant code:

```text
src/handlers/transition.rs::enforce_external_review_accept_precheck
```

Observed error before the final ER loop:

```text
external review PASS required for T146: attempt ER397 is TOOLING_FAILURE/held; retry or inspect held external review attempt ER397 (stale_base_requires_rebase)
```

Repair hint:

- Add a verb like `stores external_reviews import-pass <task> --transcript-path ... --base-sha ... --head-sha ... --runner manual-codex`.
- It should create a normal `passed/PASS` row with provenance and maybe `adapter=external_review`, `runner=codex` or a new `manual` enum.
- It must not require raw SQL and must be auditable.

### 6. Auto-promote/auto-drive started inactive tasks

After L568 was confirmed/ratified and auto-promoted to T148, `stores agents run --once` spawned:

```text
[auto-drive] T148: spawned drive pid=401987
```

But T148 status showed:

```text
Activation: inactive
```

This contradicts the expected activation gate. I killed the drive and resumed T148 back to planning.

A similar accidental remint happened first with L565 -> T147; T147 is now blocked/inactive and should be cleaned up/abandoned separately if it is indeed stale.

Repair hint:

- Re-check all auto-drive dispatch paths after T146: subscriber path, engine-runner redrive path, startup sweep path.
- Activation gating must apply to every spawn path, not only `agents.yaml` declarative subscribers.
- T142 supposedly fixed redispatch honoring `agents.yaml`, but this session produced inactive-task auto-drive again.

### 7. Installed binary skew remains confusing

`stores agents run --once` printed:

```text
daemon binary stale; reexecing into /home/blake/.local/share/stores/bin/stores (was version 0.7.0)
```

This happened after `cargo install --path . --locked` replaced `/home/blake/.cargo/bin/stores`. There are at least two binaries involved:

- `/home/blake/.cargo/bin/stores`
- `/home/blake/.local/share/stores/bin/stores`

Repair hint:

- Make `stores --version` include git SHA/build timestamp/install path.
- Make stale-binary messages include both source path and target path plus commit identity if available.
- Ensure operator commands and daemon reexec use the intended freshly installed binary consistently.

## Current task/worktree state to verify

```bash
stores tasks status T146
stores tasks status T148
stores observations show L568 --json | jq '{status,task_id,contract_state:.intent_contract.contract_state}'

git -C /home/blake/repos/experiments/stores-T148-auto-promoted-l568 rev-parse HEAD
# should be 8f6883bcd3e5020f5e2ec3f48121f136d7fcf1b5
```

T148 should remain inactive/planning until the operator explicitly activates or drives it.

## Suggested repair plan for the engine

1. **Stop false silent-zombie blocks first.** Make runner/drive liveness reconcile with child runner output and ER status before blocking a task.
2. **Make ER reconciliation idempotent.** ER result application should survive task row drift to `blocked` if the ER row itself reached terminal verdict.
3. **Add manual ER import.** Let operators convert a reviewed transcript into a `passed/PASS` external_reviews row with current head SHA.
4. **Harden activation gating.** Audit all auto-drive spawn paths, especially engine-runner redrive/startup sweep, against inactive rows.
5. **Add non-convergence escape verbs.** Prefer explicit close/import/override verbs with provenance over repeated ER loops or raw SQL temptation.
6. **Improve binary identity.** Include commit/path identity in stale-binary and daemon reexec diagnostics.

## Follow-ups

- File or fold a new high-priority observation for the manual ER PASS import gap if no duplicate exists.
- Fold the T146/T148 silent-zombie rows into the existing watchdog cluster (`L550`, `L552`, `L517`) or keep task-specific rows linked to a repair task.
- Decide whether T147 (from L565 remint) should be abandoned as stale/accidental.
- Before continuing T148, verify no T148 runner is active and activation is still inactive.
