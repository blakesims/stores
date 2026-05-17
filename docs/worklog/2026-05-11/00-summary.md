# Daily Summary — 2026-05-11

## Overview

The day started with T146 wedged by runner liveness, silent-zombie, stale-submit, and code-review recovery problems. T146 ultimately landed out-of-band as ADR 0001 task lifecycle simplification, then T148 carried ADR 0002 upstream lifecycle/read-model work through a noisy but successful integration. The session also shipped several engine repairs directly on `main`: live runner telemetry/status, duplicate-dispatch guards, stale-binary advisory semantics, manual-drive singleton checks, stale/dead marker truth, malformed runner payload visibility, storage cleanup tooling, and runner telemetry/backfill.

The dominant lesson was that T148 friction was a coupled system failure, not a single bug: duplicate/noisy external reviews, overloaded `in_review` semantics, schema/live-DB drift, stale integration/deploy wiring, prompt overreach, and runner metadata/liveness weaknesses amplified one another until operator rescue was required.

## Work Completed

- **T146 landed out-of-band** as `8f6883b Merge T146 ADR0001 lifecycle simplification`; built, installed, migrated, and closed via `stores tasks close-out-of-band` with token-mediated approval.
- **T148 ADR 0002 completed and integrated** as `55caf51 Merge branch 'feat/T148-auto-promoted-l568'`; final recovery used interrupted WIP (`d8a89b4`) plus manual-ER import compatibility (`244af10`), then accepted/integrated/installed/migrated.
- **Runner liveness and observability improvements shipped:** wall-clock timeout became advisory; live transcripts, current-run markers, semantic events/status, `stores runs current/tail`, task-status live runner display, and TUI live activity panes landed.
- **Duplicate-dispatch/live-status repairs shipped:** status now prefers active running markers over completed stale markers, and auto/manual drive paths refuse live-owner duplicates.
- **Stale-binary and stale-marker safety shipped:** post-spawn stale executable drift is advisory; manual duplicate drives refuse same-task/same-worktree live owners; stale running markers are labeled instead of holding truth forever.
- **Runner payload errors became visible:** malformed exit-0 final outputs block as typed `runner_payload_error` and mark current-run artifacts failed.
- **Storage cleanup tooling shipped:** `tasks cleanup-worktrees`, terminal target cleanup hooks, shared `CARGO_TARGET_DIR`, and `runs gc` with caps/tombstones/race hardening.
- **Runner telemetry shipped:** additive `agent_runs` telemetry, Pi nested metadata extraction/backfill, `runner-stats`, and bounded backfill (`scanned=1757 updated=632 skipped=1121 parse_failed=4`).

## Notes Today

| # | Note | Topic |
|---|------|-------|
| 01 | [t146-liveness-and-workflow-handover.md](./01-t146-liveness-and-workflow-handover.md) | T146 state, wall-clock timeout evidence, auto-resume/duplicate-drive hazards, and next-step cautions. |
| 02 | [live-runner-streaming-plan.md](./02-live-runner-streaming-plan.md) | Live runner transcript/event/status architecture and implementation progress through TUI live activity. |
| 03 | [t146-engine-friction-audit-and-t148-start.md](./03-t146-engine-friction-audit-and-t148-start.md) | T146 final close-out, T148 start, duplicate rows, and engine repair hints. |
| 04 | [t148-duplicate-dispatch-live-runner-repair-plan.md](./04-t148-duplicate-dispatch-live-runner-repair-plan.md) | Duplicate executor dispatch and stale live-status repair plan + validation. |
| 05 | [t148-remaining-runner-safety-partials-plan.md](./05-t148-remaining-runner-safety-partials-plan.md) | Stale-binary boundary split, manual singleton guard, stale/dead marker truth, payload error visibility. |
| 06 | [runner-telemetry-harness-model-thinking-plan.md](./06-runner-telemetry-harness-model-thinking-plan.md) | Harness/model/thinking telemetry plan and scope boundary from review lifecycle work. |
| 07 | [review-lifecycle-stabilization-plan.md](./07-review-lifecycle-stabilization-plan.md) | T148 status/closure notes and final recovery state. |
| 08 | [t148-autopsy-logs-db.md](./08-t148-autopsy-logs-db.md) | DB/log chronology of T148 runner churn, ER duplication, and nonconvergence. |
| 09 | [t148-autopsy-schema-lifecycle.md](./09-t148-autopsy-schema-lifecycle.md) | Schema/lifecycle drift: overloaded `in_review`, live CHECK/default drift, ADR0002 migration side paths. |
| 10 | [t148-autopsy-er-daemon-engine.md](./10-t148-autopsy-er-daemon-engine.md) | ER daemon/create-pending duplicate surfaces and per-row vs per-task/head safety. |
| 11 | [t148-autopsy-git-integration.md](./11-t148-autopsy-git-integration.md) | Integration lane findings: dirty main, stale agents wiring, post-integrated deploy friction. |
| 12 | [t148-autopsy-prompts-review-loop.md](./12-t148-autopsy-prompts-review-loop.md) | Prompt/process convergence failures: broad ER prompt, no revise budget, over-finding/testing bias. |
| 13 | [t148-autopsy-consolidation.md](./13-t148-autopsy-consolidation.md) | Consolidated root-cause map and prioritized follow-up slices. |
| 14 | [storage-auto-cleanup-plan.md](./14-storage-auto-cleanup-plan.md) | Storage cleanup policy, commands, implementation status, and remaining limits. |
| 15 | [runner-telemetry-dogfood-handoff.md](./15-runner-telemetry-dogfood-handoff.md) | Telemetry branch/backfill status and remaining configured-field gap. |

## Tensions

- **Out-of-band rescue vs dogfood doctrine:** T146/T148 required direct merges/recovery, but the notes preserve the audit trail and explicitly avoid raw SQL writes.
- **External review as gate vs architecture auditor:** T148 showed that whole-contract ER without a revise budget turns real edge discovery into an unbounded compliance hunt.
- **Schema intent vs live SQLite constraints:** YAML allowed runner values such as `manual`/`manual-codex`, while the live DB rejected them; normal migrate did not catch CHECK/default drift.
- **`in_review` overloading:** wrap dispatch, ER readiness, human acceptance, and release pressure all share one surface, creating ambiguous subscriber/order behavior.
- **Live metadata truth:** repairs improved current-run selection, but terminal task rows can still retain stale `drive_pid`/runner metadata.

## Open Threads

- Harden `external_reviews create-pending` with current-head/current-base validation, per-task/head active-attempt guard, and human-grounded recovery semantics.
- Add ER convergence policy: revise budget, duplicate finding fingerprints, PASS-with-follow-up/split-follow-up path.
- Repair live `.stores/agents.yaml` post-integrated wiring and remove stale accept-edge deploy subscribers.
- Add CHECK/default drift detection or explicit rebuilds, starting with `external_reviews.runner`.
- Clean terminal runner metadata on task terminal transitions.
- Investigate why configured Pi model/thinking fields stayed blank while effective Pi metadata/backfill worked.
- Close/abandon duplicate T147 with human/token grounding; do not drive it again.

## Tomorrow

- Prioritize ER duplicate-prevention/convergence and stale agents wiring before another broad architecture task.
- Use runner telemetry as operational data, but keep fake/infra/stale/duplicate/convergence rows excluded from model-quality conclusions until clean-data labels exist.
- If reclaiming disk, run cleanup dry-runs first, then explicit `--execute --targets-only` after reviewing candidates.
