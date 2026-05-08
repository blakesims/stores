# Handover — pi-architect

**Date:** 2026-05-08
**Type:** handover
**Role:** pi-architect

## Active thread

`/home/blake/repos/.agent-comm/threads/2026-05-08-01-stores-2026-05-08-session.md`

## Current responsibility

Pi is protecting the architectural direction during wind-down. Main ruling from the last discussion: the next fidelity gain should move to the front of the engine (queue curation / watch truth / triage), not re-promote reviewer-runner into the normal review path.

Big-picture engine status:

- Back-end/review fidelity is now relatively strong: substrate-native `external_reviews` is canonical for T2/T3, stale-base pre-review protection shipped via T103/L488, rate-limit-aware retry shipped via T100/L484, cascade-dedup shipped via T099/L483 and repaired by T102/L491.
- Front-end/observational fidelity is now the weak layer: `stores watch` still mixes internal `code_review`, final `in_review`, external Codex review history, accepted/recovered terminal rows, priority candidates, observation drafts, resolved rows, and intake states into confusing buckets.
- Blake's desired mental model is pipeline-shaped, not flat-list-shaped: inbox/intake → observation triage/draft/info/architecture-review → ratifiable intent contract → task execution/review/deploy → terminal history. Watch should make these boxes and transitions visible.
- Larger Heart/Architect plan: wherever stores is used, the project has a Heart/Philosophy/Primitives layer. Architecture-gray observations should route to an Architect agent that interprets the observation against that project's Heart and primitives, using `architecture_reviews` as the typed ruling buffer. Queue-curator/gatekeeper are the front door; Architect is the global coherence governor above local planner/executor/reviewer perspectives.
- Throughput concern: tiny fixes should not always pay full observation→triage→contract→task→executor→review→codex ceremony. The repair lane is one answer for substrate-blocking bugs; a broader “fast path / right-sized ceremony” design is still open.
- Git/worktree management remains a major throughput risk: L486 canonical-mainline control-plane doctrine and L488 pre-review rebase help, but priority + file-overlap scheduling is still needed before raising WIP aggressively.

SOP updates made this handover:

- Added `.claude/skills/queue-curator/SKILL.md`.
- Updated `.claude/skills/engine-controller/SKILL.md` with substrate repair lane + post-T083 review doctrine.
- Updated `.claude/skills/pi-architect/SKILL.md` with 2026-05-08 priority posture + repair-lane approval duty.
- Updated `.claude/skills/reviewer-runner/SKILL.md` with fallback/audit role + escalation triggers.
- Added `docs/worklog/2026-05-08/handover-queue-curator.md`.

## Active work / processes

| item | status | pid | worktree | branch | commit | next action |
|---|---|---|---|---|---|---|
| T098 / L480 | WIP, phase 5 cycle 2; last known internal `code_review`/revise loop | see engine-controller handover | task worktree | task branch | latest in handover/thread | Let engine-controller finish; it is the live cockpit/watch-fidelity task. |
| queue-curator | SOP + handover created, not yet started as live role | — | — | — | 61460f6 | Start next session if Blake wants front-end triage fidelity work. |
| docs/engine-health.md | stale after later ships | — | main | main | pre-later-session | Refresh early next session before using it as priority source. |

## Do not do

- Do not start new implementation work during wind-down.
- Do not promote reviewer-runner back to default review path unless Path A is concretely broken.
- Do not let queue-curator implement code or make architecture decisions.
- Do not raw-SQL the substrate DB.

## First step for next agent

1. Refresh `docs/engine-health.md` before treating it as source-of-truth. It is stale: it predates the later T100/T101/T102/T103/T104 ships, direct parser/L488 recovery patches, L489/L492/L497/L498 filings, and the front-of-engine priority pivot.
2. Read the updated skill files, especially `.claude/skills/queue-curator/SKILL.md`.
3. Coordinate with engine-controller on whether to start queue-curator in the next session.

## Notes

Current role split:

- Path A substrate-native `external_reviews` is canonical T2/T3 review gate.
- reviewer-runner is fallback/audit witness when Path A is sick/self-referential or Pi/Blake asks.
- queue-curator is the temporary manual prototype for future triage/scheduler machinery.
- engine-controller runs the daemon and can use the substrate repair lane for narrow tested direct-on-main repairs when the substrate blocks itself.

Priority list for next architectural session:

1. Finish/land T098/L480 cockpit attention fixes; it is directly addressing Blake's watch confusion.
2. Refresh `docs/engine-health.md` to reflect today's actual shipped state and priority pivot.
3. Start queue-curator for a live `QUEUE-SNAPSHOT` and feedback on observation/intake/task pipeline shape.
4. Turn Blake's pipeline mental model into watch/triage design: inbox/intake, observation triage, architecture review, ratifiable contracts, tasks, terminal history should be visually distinct boxes, not mixed flat lists.
5. Prioritize a watch nomenclature cleanup: distinguish internal `code_review`, final `in_review`, and substrate-native external/Codex review. Avoid overloading “review”.
6. Design right-sized ceremony / fast path for trivial fixes so 1-10 line repairs do not require more process than code, without weakening audit or authority boundaries.
7. Then resume priority + file-overlap scheduler work grounded in L486/L488.
8. Keep L489 stale-binary watchdog, L492 schema-drift durability, L497 parser durability, and L498 external_review recovery durability as important but secondary unless they block the engine again.
