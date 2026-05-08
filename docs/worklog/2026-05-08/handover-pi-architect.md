# Handover — pi-architect

**Date:** 2026-05-08
**Type:** handover
**Role:** pi-architect

## Active thread

`/home/blake/repos/.agent-comm/threads/2026-05-08-01-stores-2026-05-08-session.md`

## Current responsibility

Pi is protecting the architectural direction during wind-down. Main ruling from the last discussion: the next fidelity gain should move to the front of the engine (queue curation / watch truth / triage), not re-promote reviewer-runner into the normal review path.

SOP updates made this handover:

- Added `.claude/skills/queue-curator/SKILL.md`.
- Updated `.claude/skills/engine-controller/SKILL.md` with substrate repair lane + post-T083 review doctrine.
- Updated `.claude/skills/pi-architect/SKILL.md` with 2026-05-08 priority posture + repair-lane approval duty.
- Updated `.claude/skills/reviewer-runner/SKILL.md` with fallback/audit role + escalation triggers.
- Added `docs/worklog/2026-05-08/handover-queue-curator.md`.

## Active work / processes

| item | status | pid | worktree | branch | commit | next action |
|---|---|---|---|---|---|---|

## Do not do

- Do not start new implementation work during wind-down.
- Do not promote reviewer-runner back to default review path unless Path A is concretely broken.
- Do not let queue-curator implement code or make architecture decisions.
- Do not raw-SQL the substrate DB.

## First step for next agent

Read the updated skill files, especially `.claude/skills/queue-curator/SKILL.md`, then coordinate with engine-controller on whether to start queue-curator in the next session.

## Notes

Current role split:

- Path A substrate-native `external_reviews` is canonical T2/T3 review gate.
- reviewer-runner is fallback/audit witness when Path A is sick/self-referential or Pi/Blake asks.
- queue-curator is the temporary manual prototype for future triage/scheduler machinery.
- engine-controller runs the daemon and can use the substrate repair lane for narrow tested direct-on-main repairs when the substrate blocks itself.
