# T140 Manual Integration And ER Timing

**Date:** 2026-05-10
**Type:** note / integration recovery / design observation

## Summary

T140 reached `accepted` while the broad engine/daemon path was intentionally off. I promoted it manually through the integration lane, ran a fresh external review after the integration rebase changed the branch head, retried integration with Blake's token, and reconciled the stores-specific post-integrated chain. Final state: **T140 `schema_migrated`**.

This session also exposed a design pressure: external review currently happens before integration, but integration rebases onto current `main`. Any intervening main change can invalidate a prior PASS even when the task branch's own work is unchanged. Blake proposed moving ER to the integration point immediately before merge. That deserves follow-up design because it likely reduces stale-review churn, with the trade-off that substantive review findings are found later in the lane.

## What happened

Starting state:

- T140 was `accepted`, `activation=active`, but not merged/deployed.
- The engine was off, so `accepted → integration_queued` did not fire automatically.
- Main did not yet contain the final T140 branch commits.
- T140 branch final head had moved during integration from the previously reviewed SHA to a rebased candidate SHA.

Manual recovery performed:

1. Added and used narrow manual recovery verbs:
   - `stores tasks enqueue-integration <TID>` — framework-owned `accepted → integration_queued` recovery when the engine was off.
   - `stores tasks run-integration <TID>` — run `builtin:integrate` once without starting the full daemon.
   - `stores external_reviews create-pending <TID> --base-sha <sha> --head-sha <sha>` — create a fresh pending ER for a known task head.
2. Enqueued T140:
   - `accepted → integration_queued`.
3. Ran integration once:
   - `integration_queued → integrating → integration_blocked` with `stale_external_review` because ER368 reviewed old head `2650aa7`, while the integration candidate had moved.
4. Created and ran fresh ER:
   - ER369 reviewed the rebased candidate.
   - ER369 result: `PASS`.
5. Retried integration with human-grounded token:
   - `integration_blocked → integration_queued → integrating → integrated`.
6. Ran post-integrated recovery:
   - `tasks reconcile-accepted T140 ...`
   - cargo install OK, schema migrate no-op/in-sync.
   - final state: `schema_migrated`.

Commits made on main during this recovery:

- `749e1dc` — `tasks: add manual integration recovery verbs`
- `fe13f07` — `external-reviews: add manual pending review creation`

Earlier same-session substrate fix also relevant:

- `f75a82c` — `drive: ingest valid envelopes despite runner nonzero exit`

## Substrate vs operator/manual work

The stale handover/master-plan distinction still applies:

- The substrate should own routine, typed lifecycle work: task drive, review, acceptance gates, integration queueing, integration, cargo-install, schema-migrate, and observation/intake cleanup through verbs.
- The operator/manual lane is for narrow, explicit recovery when the engine itself is off or interlocked: single-row verbs, one-shot builtins, read-only diagnostics, and explicit close/retry actions. It should not raw-SQL mutate state.
- When the substrate hurts, the pain is data: file/record the friction, add a narrow primitive if needed, then use that primitive. Do not hand-edit markdown or the DB to simulate lifecycle progress.
- Direct code edits on main are acceptable for substrate-repair-lane work when the dogfood path is blocked by the substrate itself, but the resulting state transition should still be performed with substrate verbs where possible.

Today's recovery followed that shape: no raw SQL writes; the missing operations were promoted into explicit CLI verbs, then T140 moved via lifecycle transitions and builtins.

## ER timing design observation

Blake's observation: ER may be happening too early. Current shape means:

1. Task reaches `in_review`.
2. External review runs against current task branch head and current-ish base.
3. Later, integration rebases/refreshes against newer `main`.
4. If `main` moved, the branch head changes and the old ER is stale.
5. Integration blocks even if the task's substantive code did not change.

Proposed direction to evaluate: move or duplicate the final ER to the integration point immediately before merge:

```text
accepted → integration_queued → integrating
  refresh/rebase against main
  run ER against refreshed candidate
  PASS → merge
  REVISE → integration_blocked with findings
```

Potential benefits:

- Fewer stale ER invalidations caused only by main moving.
- Review is tied to the exact candidate that would merge.
- Integration lane becomes the single place that proves freshness.

Trade-off:

- Real issues are found later, while the row is already in the integration lane.
- A REVISE at that point blocks the integration slot unless the lane releases quickly and records actionable findings.

Likely compromise worth designing:

- Keep earlier ER as a useful pre-accept signal if desired.
- Treat the integration-point ER as the authoritative merge gate.
- On integration ER `REVISE`, transition to `integration_blocked` with findings and release the singleton slot.
- Watch/status should distinguish `stale_external_review` from `integration_review_revise`.

## Follow-ups

- Design task/observation: integration-point authoritative ER gate to reduce stale-review churn.
- Decide whether earlier external review remains mandatory pre-accept, advisory, or skipped for selected tiers.
- Harden manual recovery verbs added today with tests and docs, or fold them into a more coherent `tasks promote-integration` / `tasks run-integration` operator surface.
- Clean up T139, which was separately marked blocked by stale binary inode while manual engine work replaced `target/debug/stores` during its detached drive.
