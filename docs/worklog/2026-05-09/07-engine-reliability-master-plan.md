# Engine Reliability Master Plan

**Date:** 2026-05-09
**Type:** master plan / worklog

## Summary

This note tracks the broader engine-reliability program around the current cleanup/activation work. T140 is the immediate ignition-ready task: clean/remap the current substrate DB, add the activation safety switch, and make `engine plan-start` explain what will combust. Around that, we should use a manual/operator lane for narrow reliability improvements that are additive, observable, and low-risk.

North star: starting the engine should feel like turning a key in a checked cockpit, not opening a haunted basement. The operator should know exactly what can run, what is inert, what is historical exhaust, what needs a decision, and what health faults exist.

## Active work in flight

| item | owner/lane | status | why it matters | notes |
|---|---|---|---|---|
| T139 — watch store-flow cockpit skeleton | engine task | executing phase 3/4 | Makes the engine visible as store-flow, not raw rows | T139 remains read-only; should consume disposition once available, not own activation writes. |
| T140 — ignition-ready cleanup + activation gate | engine task | planning/plan-review in progress | Makes DB clean/remapped and adds activation safety switch | Expanded acceptance: tasks + observations + intake cleanup/remap; per-row activation; plan-start. |
| Queue-curator audit | parallel agent / read-only | complete, updated to T140 scope | Durable mapping of current DB rows into operator buckets | `docs/worklog/2026-05-09/06-queue-curator-disposition-audit-and-fold-proposals.md` §10 is feedstock. |

## Program lanes

### Lane 1 — Ignition-ready engine surface (T140)

**Goal:** when T140 completes, the current engine DB is clean/remapped enough that the engine can be started intentionally, with activation as the safety trigger.

**Expected output groups:**

Tasks:

| group | meaning |
|---|---|
| `active_work` | activated row that can run now |
| `inactive_ready` / `awaiting_activation` | valid task exists but safety switch is off |
| `awaiting_integration` | accepted current-era task, integration gated by activation |
| `blocked_recoverable` | human/operator recovery needed |
| `needs_operator_review` | ambiguous/stranded row requiring decision |
| `deploy_ceremony_pending` | mid-release ceremony / stranded ceremony |
| `historical_terminal_legacy` | legacy accepted rows, hidden from active lanes |
| `terminal_success` | fully shipped modern terminal |
| `terminal_shipped_oob` | shipped out of band |
| `terminal_retired` | abandoned / rejected / retired |

Observations:

| group | meaning |
|---|---|
| `linked_to_active_task` | obs already represented by active task |
| `linked_to_inactive_task` | obs has child task waiting activation |
| `ready_to_promote_inactive` | ratified obs may mint inactive task |
| `real_backlog` | real issue, not junk |
| `needs_investigation` | live investigation needed |
| `arch_review_candidate` | doctrine/architecture route |
| `duplicate_or_folded` | duplicate closed/folded into keeper |
| `superseded_or_resolved` | stale/superseded by shipped work |
| `terminal_resolved` | resolved terminal |
| `terminal_wont_fix` | wont_fix terminal |

Intake:

| group | meaning |
|---|---|
| `draft_triage_backlog` | raw draft needing triage |
| `routable_to_observation` | should become observation |
| `duplicate` | fold/drop against existing item |
| `doctrinal_doc_only` | doc/SOP-only; no substrate row needed |
| `arch_review_candidate` | architecture review route |
| `terminal_routed` | already routed |
| `terminal_dropped` | dropped terminal |

**Acceptance shape:**

- Derived task `operator_disposition` exists and is tested against dirty snapshot rows.
- Per-row activation primitive exists.
- New/promoted tasks default inactive.
- Observation auto-promote mints inactive tasks.
- Work-starting task paths are schema-enforced behind activation.
- Safety/reconcile/cleanup subscribers are explicitly ungated.
- `stores engine plan-start` shows task ignition plan: `WOULD RUN`, `INACTIVE / ARMED OFF`, `NEEDS OPERATOR`, `BLOCKED`, `HISTORICAL EXHAUST`, `QUEUE HYGIENE`.
- Current DB cleanup/remap batches have run via substrate verbs or explicit T140 primitives; no raw SQL writes.

**Key design decisions already made:**

- Legacy accepted rows: derived-only for now, no retire mutation.
- Activation shape: per-row first, not queue/manifest.
- Activation enforcement: gate work-starting task paths only.
- Default activation: inactive for newly created/promoted tasks.
- Observation promotion: auto-promote still allowed, but mints inactive task.
- First `plan-start` command scope: tasks only, with queue-hygiene summary allowed.

**Open decisions T140 planner must pin:**

- Backfill behavior for existing rows on activation-primitive ship day.
- Exhaustive subscriber taxonomy: work-starting vs safety/reconcile/cleanup.
- T138 activation on ship day: active to continue integration or inactive to freeze pending operator trigger.
- Schema enforcement mechanism: `StateAction when:` / Check predicate vs runtime branching.

### Lane 2 — Daemon / lock inspector (manual-main candidate)

**Goal:** make daemon/dispatch health inspectable without starting broad automation.

Candidate commands:

```bash
stores locks status
stores engine locks
stores engine health
stores engine doctor
```

Desired buckets:

| bucket | meaning |
|---|---|
| `live_claim` | active lock with live pid / live subscriber |
| `retry_wait` | lock waiting for next retry/backoff |
| `stale_harmless` | completed/old lock debris that does not block dispatch |
| `stale_blocking` | stale unfinished lock preventing useful work |
| `orphaned` | lock points to dead/missing process or impossible row state |
| `fresh_failure` | recent terminal failure needing attention |

Why manual lane is acceptable:

- Can be read-only.
- Does not change lifecycle authority.
- Gives immediate operator trust.

Suggested first slice:

1. Read `dispatch_locks`, `daemon_starts`, `tasks.drive_pid`, `agent_runs`.
2. Classify locks into the buckets above.
3. Print row id, subscriber/agent, age, attempts, next_retry_at, terminal_reason, postcondition.
4. Add tests with fixture rows.

Out of scope for first slice: cleanup/delete locks automatically.

### Lane 3 — Runner/model telemetry + provenance (manual-main candidate if additive)

**Goal:** stop choosing models/runners by vibes and stop burning review cycles on mechanically invalid executor metadata.

Useful fields/surfaces:

| data | why |
|---|---|
| role | planner / executor / reviewer / wrap behavior differs |
| runner | claude-code / pi / codex / mock |
| model | Opus/Sonnet/etc. must be measurable |
| prompt/config hash | compare like with like |
| duration | cost and throughput |
| token/cost when available | economics |
| outcome | pass/revise/fail/tooling/rate-limit |
| transcript/log path | inspect failures |

Candidate commands:

```bash
stores runs list --role executor --since 24h
stores metrics runners --window 7d
stores tasks status T123 --show-runner-history
```

Safe manual slices:

1. Display existing `agent_runs` runner/model/duration data better.
2. Add missing additive telemetry columns only if migration is straightforward.
3. Add metrics view for role×runner×model outcomes.

Risk boundary: do not change model routing policy in the same manual patch.

#### Commit provenance hardening

The system should capture/validate executor commit hashes itself. Do not rely on a model to type a 40-character SHA correctly.

Desired behavior:

| behavior | why |
|---|---|
| `submit-execute` resolves actual `HEAD` from `tasks.workspace_path` when possible | review target becomes machine-captured, not model-reported |
| supplied `--commit` is validated with `git show <sha>` before accepting the submission | invalid metadata cannot burn a code-review cycle |
| if supplied SHA differs from workspace `HEAD`, fail loud or record both claimed/resolved under explicit policy | prevents ambiguous moving-target reviews |
| code reviewer/external reviewer reviews the recorded valid commit, not whatever HEAD happens to be later | eliminates reverse-race with manual-main work |

This is a high-priority follow-up because T140 cycle 1 burned a REVISE solely on an invalid executor-reported commit SHA while the actual implementation satisfied the ACs.

### Lane 4 — Rate-limit handling (manual-main candidate if narrow)

**Goal:** rate limits should become typed cooldowns, not silent zombies or generic drive failures.

Desired behavior:

| input | output |
|---|---|
| known provider 429 / retry-after | `blocked_reason=rate_limit:<provider>:<until-or-unknown>` |
| rate-limit in runner payload_error | same typed class |
| plan-start/watch | shows cooldown / retry time |
| retry scheduler | treats as cooldown, not flake, once scheduler work is in scope |

Safe first slice:

- Detect known rate-limit signatures at runner/drive boundary.
- Write typed blocked reason.
- Display in status/watch/plan-start.
- Tests for Claude/Pi/Codex-ish strings.

Risk boundary: do not overhaul retry scheduler in the first manual patch unless already isolated.

### Lane 5 — T1 fast path / right-sized ceremony

**Goal:** small safe work should not consume full T3-style ceremony.

Problem evidence:

- T116 was a tiny exact-token matcher but consumed too much cycle cost.
- The engine can be correct but economically bad.

Candidate improvements:

| improvement | risk | notes |
|---|---|---|
| T1 exact-token / doc-only helper | low | Manual-main likely OK. |
| T1 plan/review display and metrics | low | Observability first. |
| T1 external-review policy tuning | medium | Needs doctrine. |
| Auto-accept or bypass review for T1 | high | Do not manual-main without ratified design. |
| Focused test+review template for T1 | medium | Could be a task. |

Recommended path:

1. Measure T1 cycle time and revise rate.
2. Add T1-specific status/metrics.
3. Design a minimal T1 fast lane after activation cleanup lands.

### Lane 6 — Watch / cockpit continuity (T139 + follow-ups)

**Goal:** make the engine navigable and pleasant.

T139 Phase 1 should remain read-only:

- top store strip,
- focused store table,
- selected detail,
- terminal history hidden by default,
- no substrate mutations.

Follow-ups after T139/T140:

| follow-up | source |
|---|---|
| consume task `operator_disposition` | T140 |
| show activation state / armed-off rows | T140 |
| engine health lane uses lock inspector | Lane 2 |
| runner/model details in task drilldown | Lane 3 |
| rate-limit cooldown badges | Lane 4 |
| obs/intake triage bucket display | T140 cleanup/remap |

### Lane 7 — Intake/observation drain

**Goal:** prevent future junk drawer buildup.

T140 owns current cleanup/remap, but durable prevention needs:

- gatekeeper draft drain,
- duplicate/cluster handling,
- architecture-review routing,
- triage bucket surfacing,
- auto-resolve/auto-promote edge hardening.

Manual cleanup batches from queue-curator are feedstock, but T140 acceptance now expects enough of them applied/remapped to make the DB ignition-ready.

### Lane 8 — Integration/deploy hardening

**Goal:** T138 integration lane must become boring.

Watch points:

- T138 is currently accepted; T140 must decide activation/backfill behavior for it.
- Integration failures should be typed and recoverable.
- Post-integrated adapters should be visible.
- `accepted` should never again look like a terminal state in the operator cockpit.

Likely follow-ups:

- better integration-blocked UX,
- retry-integration plan-start preview,
- stale-base handling improvements,
- post-integrated subscriber status.

## Progress log

| time | update |
|---|---|
| 2026-05-09 | T139 running phase 3/4; watch cockpit underway. |
| 2026-05-09 | T140 created and driven; expanded to full ignition-ready cleanup/remap + activation gate. |
| 2026-05-09 | Queue-curator audit updated with full row-level mapping (§10). |
| 2026-05-09 | Manual-main candidates identified: locks inspector, telemetry, rate-limit typing, T1 fast path measurement. |
| 2026-05-09 | First manual-main slice shipped locally: `stores engine locks` read-only dispatch_locks inspector. Current live DB: live=0, retry_wait=0, stale_blocking=0, orphaned=0, fresh_failure=4 (T081/T122/T018 accepted-row ceremony failures), stale_harmless=1093. Tests: `cargo test --lib cli::engine::tests --quiet`; `cargo check --quiet`. |
| 2026-05-09 | Second manual-main slice added: `stores runner-stats` summarizes `agent_runs` by role × harness × model with run counts, failures, durations, and token totals. Added `--display-id` for task-scoped reads; T139 currently shows planner/executor on claude-code Opus and reviewers on Pi, all exit 0 so far. Live aggregate exposed current economics: pi executor/wrap have high failure counts; claude-code executor/planner runs are slower but currently all exit 0 in the aggregate. Tests: `cargo test --lib cli::runner_stats::tests --quiet`; `cargo check --quiet`. |
| 2026-05-09 | Added provenance-hardening follow-up to this plan after T140 cycle 1 burned a REVISE on an invalid executor-reported commit SHA. Desired fix: `submit-execute` captures/validates workspace HEAD itself and reviewers inspect immutable recorded commits. |
| 2026-05-09 | Implemented first provenance-hardening slice: `submit-execute` now captures workspace `HEAD` when `tasks.workspace_path` is readable, stores it as authoritative `executor.commit`, preserves mismatching model-provided SHA as `executor.claimed_commit` plus `executor.commit_resolution`, and code-reviewer brief now tells reviewers to treat system-captured commit as authoritative. Tests: `cargo test --lib submit_execute --quiet`; `cargo check --quiet`. Remaining hardening: make reviewer diff/checkouts fully commit-anchored instead of moving-HEAD. |

## Immediate next actions

1. Monitor T140 plan review; ensure plan includes full DB cleanup/remap, not just logic changes.
2. Monitor T139 phase 3 executor; keep cockpit work moving.
3. Next manual-main slice candidates:
   - add submit-execute commit provenance validation / system-captured HEAD; or
   - add rate-limit typed classification/status display; or
   - refine `stores engine locks` with age filtering / JSON consumers after T140 needs are clearer; or
   - add recent-runner-failure drilldown once `runner-stats` has proven useful.
4. Keep queue-curator's audit doc linked as T140 fixture/input.
5. Do not raw-SQL mutate `.stores/db.sqlite`; all cleanup goes through substrate verbs or T140 primitives.

## Parking lot

- Queue/manifest activation may come later, but v1 is per-row.
- Priority + file-overlap scheduler comes after activation/visibility.
- Watch actions come later; cockpit remains read-only until it earns authority.
- Legacy accepted rows stay derived-historical unless a later explicit retire-legacy verb is designed.
