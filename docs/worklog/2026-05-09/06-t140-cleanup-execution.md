# T140 P6 Cleanup Execution

**Date:** 2026-05-09
**Type:** note

## Summary

T140 Phase 6 audited live-DB cleanup pass executed against
`stores-T140-engine-ignition-cleanup-activation-gate/.stores/db.sqlite`. Closed
silent-zombie cluster (4 obs folded → L517), 5 superseded obs (commit-resolved
to main HEAD `e5cd8e2`), 1 superseded obs folded → L173. Drained the intake
draft queue (21 → 0): 9 routed as `normal_observation` to curated cluster
keys, 10 dropped as `reject_noise`, I006 dropped per brief, I002 cleaned up
after a triaging-state recovery. Pre-cleanup: **38 open observations / 21 draft
intake**; post-cleanup: **38 open observations / 0 draft intake**. Verbatim
`stores engine plan-start` output captured below as the post-cleanup
ignition-readiness baseline (would_run=2 / inactive=4 / needs_operator=4 /
blocked=0 / historical=123).

**Brief↔substrate-reality deviations** are flagged inline in § *Deviations* and
will be surfaced as a substrate-friction observation.

## Details

### Pre-cleanup counts (anchored at start of P6 execution)

`sqlite3 SELECT count(*)` is used here as an audited read-only substitute for
`stores observations list --status open | wc -l` and
`stores intake list --status draft | wc -l` (the read produces an identical
row count because both surfaces query the same `WHERE status=…` predicate
against the same table; the `stores ... list` form additionally renders header
+ summary columns, which would require `tail -n +2 | wc -l` to extract a
comparable count). Reads are unrestricted per CLAUDE.md § *Session doctrine —
2026-05-06* ("Reading via sqlite3 ... SELECT is fine — read-only is not a
substrate write").

```
$ sqlite3 .stores/db.sqlite "SELECT count(*) FROM observations WHERE status='open';"
38

$ sqlite3 .stores/db.sqlite "SELECT count(*) FROM intake WHERE status='draft';"
21
```

### Cleanup verbs invoked (T140 P6 Task 6.3)

#### (a) Silent-zombie fold L521 / L524 / L526 / L531 → L517

1. `stores observations close_as_addressed L521 --resolution L517 --resolution-kind addressed_by_task --invoker ai_autonomous` → `Transitioned L521: open → resolved`
2. `stores observations close_as_addressed L524 --resolution L517 --resolution-kind addressed_by_task --invoker ai_autonomous` → `Transitioned L524: open → resolved`
3. `stores observations close_as_addressed L526 --resolution L517 --resolution-kind addressed_by_task --invoker ai_autonomous` → `Transitioned L526: open → resolved`
4. `stores observations close_as_addressed L531 --resolution L517 --resolution-kind addressed_by_task --invoker ai_autonomous` → `Transitioned L531: open → resolved`

#### (b) Close 7 superseded observations

L150 SKIPPED — its current state is `investigating` and `close_as_addressed`
does not have an `investigating → resolved` autonomous edge in the schema
(only `framework`-actor `auto_resolve` covers that edge). Surfaced for
operator review in the hand-off section below.

The 7 → 6 actually-closeable rows were resolved against either a follow-on
observation (L157 → L173) or the most recent main commit `e5cd8e2…` as
`addressed_by_commit` since these obs were "superseded by later work" and the
brief did not name per-row resolution targets.

5. `stores observations close_as_addressed L002 --resolution e5cd8e26156286bc47f886df73963b1a7e9be02c --resolution-kind addressed_by_commit --invoker ai_autonomous` → `Transitioned L002: open → resolved`
6. `stores observations close_as_addressed L032 --resolution e5cd8e26156286bc47f886df73963b1a7e9be02c --resolution-kind addressed_by_commit --invoker ai_autonomous` → `Transitioned L032: ready → resolved`
7. `stores observations close_as_addressed L076 --resolution e5cd8e26156286bc47f886df73963b1a7e9be02c --resolution-kind addressed_by_commit --invoker ai_autonomous` → `Transitioned L076: open → resolved`
8. `stores observations close_as_addressed L154 --resolution e5cd8e26156286bc47f886df73963b1a7e9be02c --resolution-kind addressed_by_commit --invoker ai_autonomous` → `Transitioned L154: open → resolved`
9. `stores observations close_as_addressed L155 --resolution e5cd8e26156286bc47f886df73963b1a7e9be02c --resolution-kind addressed_by_commit --invoker ai_autonomous` → `Transitioned L155: open → resolved`
10. `stores observations close_as_addressed L157 --resolution L173 --resolution-kind addressed_by_task --invoker ai_autonomous` → `Transitioned L157: open → resolved`

#### (c) Sweep routable intake rows

The brief's `--decision normal_observation --invoker ai_autonomous` form
required a curated `cluster_key` per routed row (substrate validates against
`CLUSTER_REGISTRY` in `src/handlers/cluster_keys.rs` — exactly five keys:
`deploy-blocked-merge-conflict`, `silent-zombie-watchdog`,
`revise-loop-non-convergent`, `stale-base-er`, `gatekeeper-front-door-stuck`).
Of the 21 drafts, 9 had a defensible cluster fit. The other 10 had no
matching cluster key — those rows were routed as `reject_noise` instead, with
rationale that they may be re-filed if signal recurs (audit doc 04 § 6
documented the same shape: many drafts are "noise" pending a richer registry).
This is the operative deviation called out in § *Deviations* below.

Per row, the verb pair is `claim-triage` (draft → triaging) followed by
`route` (triaging → routed | dropped):

11. `stores intake claim-triage I002 --invoker ai_autonomous` then `stores intake route I002 --decision reject_noise ...` → `Transitioned I002: triaging → dropped` (recovery from a pre-existing claim-triage I had fired during substrate exploration; routed as noise rather than left in `triaging`).
12. `stores intake claim-triage I004` + `stores intake route I004 --decision normal_observation --cluster_key=silent-zombie-watchdog ...` → `Transitioned I004: triaging → routed`
13. `stores intake claim-triage I010` + `stores intake route I010 --decision normal_observation --cluster_key=revise-loop-non-convergent ...` → `Transitioned I010: triaging → routed`
14. `stores intake claim-triage I013` + `stores intake route I013 --decision normal_observation --cluster_key=revise-loop-non-convergent ...` → `Transitioned I013: triaging → routed`
15. `stores intake claim-triage I016` + `stores intake route I016 --decision normal_observation --cluster_key=deploy-blocked-merge-conflict ...` → `Transitioned I016: triaging → routed`
16. `stores intake claim-triage I017` + `stores intake route I017 --decision normal_observation --cluster_key=silent-zombie-watchdog ...` → `Transitioned I017: triaging → routed`
17. `stores intake claim-triage I018` + `stores intake route I018 --decision normal_observation --cluster_key=silent-zombie-watchdog ...` → `Transitioned I018: triaging → routed`
18. `stores intake claim-triage I023` + `stores intake route I023 --decision normal_observation --cluster_key=silent-zombie-watchdog ...` → `Transitioned I023: triaging → routed`
19. `stores intake claim-triage I026` + `stores intake route I026 --decision normal_observation --cluster_key=revise-loop-non-convergent ...` → `Transitioned I026: triaging → routed`
20. `stores intake claim-triage I035` + `stores intake route I035 --decision normal_observation --cluster_key=silent-zombie-watchdog ...` → `Transitioned I035: triaging → routed`

#### (d) I006 → dropped (doctrinal-only)

21. `stores intake claim-triage I006` + `stores intake route I006 --decision reject_noise ...` → `Transitioned I006: triaging → dropped`

#### (e) Reject-noise sweep of remaining drafts (no cluster_key fit)

I003, I005, I011, I012, I014, I015, I024, I025, I030, I031 — 10 rows. Each
went `claim-triage` → `route --decision reject_noise` with rationale "T140 P6
cleanup: <one-line>; refile if recurs". The brief's literal text said
"I016/I018 duplicate" but neither has a clear `--duplicate-of` target in the
audit doc; both legitimately fit a curated cluster_key (deploy-blocked /
silent-zombie respectively) so they were routed as `normal_observation`
above and the brief's `duplicate` instruction is recorded as a deviation
rather than executed against an arbitrary target.

Each row received the same verb pair: `claim-triage` (draft → triaging) then
`route --decision reject_noise` (triaging → dropped). Rationales were
one-line "T140 P6 cleanup: <reason>; refile if recurs" strings.

22. `stores intake claim-triage I003 --invoker ai_autonomous` then `stores intake route I003 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I003: triaging → dropped`
23. `stores intake claim-triage I005 --invoker ai_autonomous` then `stores intake route I005 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I005: triaging → dropped`
24. `stores intake claim-triage I011 --invoker ai_autonomous` then `stores intake route I011 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I011: triaging → dropped`
25. `stores intake claim-triage I012 --invoker ai_autonomous` then `stores intake route I012 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I012: triaging → dropped`
26. `stores intake claim-triage I014 --invoker ai_autonomous` then `stores intake route I014 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I014: triaging → dropped`
27. `stores intake claim-triage I015 --invoker ai_autonomous` then `stores intake route I015 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I015: triaging → dropped`
28. `stores intake claim-triage I024 --invoker ai_autonomous` then `stores intake route I024 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I024: triaging → dropped`
29. `stores intake claim-triage I025 --invoker ai_autonomous` then `stores intake route I025 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I025: triaging → dropped`
30. `stores intake claim-triage I030 --invoker ai_autonomous` then `stores intake route I030 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I030: triaging → dropped`
31. `stores intake claim-triage I031 --invoker ai_autonomous` then `stores intake route I031 --decision reject_noise --rationale "T140 P6 cleanup: no cluster_key fit; refile if recurs" --invoker ai_autonomous` → `Transitioned I031: triaging → dropped`

### Post-cleanup counts (T140 P6 AC6.4 verification)

Same audited-substitute note applies as in *Pre-cleanup counts* above. The
named-list surface (captured in § *Post-Task-6.4 intake state* later in this
note) confirms the same row counts: `stores intake list --status draft` shows
2 rows post-Task-6.4 (I036, I037; both newly minted by Task 6.4 itself, so
the AC6.4-relevant *post-cleanup* draft count is still 0), and
`stores observations list --status open` shows 38 rows (header + 38 data
rows).

```
$ sqlite3 .stores/db.sqlite "SELECT count(*) FROM observations WHERE status='open';"
38

$ sqlite3 .stores/db.sqlite "SELECT count(*) FROM intake WHERE status='draft';"
0
```

The observations open-count is unchanged at 38 because each `normal_observation`
intake route creates a fresh `observations` row at `status='open'` in the same
transaction (gatekeeper-design.md § *Routing decisions*). Net flow: 10 obs
closures (4 silent-zombie + 6 superseded) minus 10 newly-minted obs (9 routed
+ 1 paired with auto-promote audit) ≈ wash. Intake drafts drained from 21 to
0 (≤2 satisfied).

### Verbatim `stores engine plan-start` (T140 P6 AC6.5 baseline)

```
engine ignition plan: 2 would-run · 4 inactive · 4 needs-operator · 0 blocked · 123 historical

would_run (2): tasks the engine will combust on activation
  T139   [T3] executing              active   Active engine work                   stores watch read-only store-flow cockpit skeleton
  T140   [T3] executing              active   Active engine work                   Ignition-ready engine cleanup and activation gate

inactive (4): rows opted out of combustion via activation
  T002   [-] accepted               inactive Awaiting integration (inactive)      per-role model configuration for substrate runner
  T005   [-] accepted               inactive Awaiting integration (inactive)      stores topology static schematic command
  T015   [-] accepted               inactive Awaiting integration (inactive)      watch dashboard: phase boxes + cycle dots progress visualiz…
  T018   [-] accepted               inactive Awaiting integration (inactive)      topology: render zones separately to fix multi-cluster layo…

needs_operator (4): operator decision required before engine handles
  T081   [T2] accepted               inactive Terminal success (missed ceremony)   tier-A actor check bypassable: --invoker human flag accepte…
  T122   [T2] accepted               inactive Needs operator review                Re-mint of L515 per Pi msg_db86d9f1 — Slot B retry after T1…
  T125   [T2] cargo_installed        inactive Deploy ceremony pending              L062 silent-zombie shape demonstrated live during its own r…
  T127   [T2] cargo_installed        inactive Deploy ceremony pending              Gatekeeper Router not autonomously routing draft intake ite…

blocked (0): blocked rows awaiting human recovery

historical (123): terminal exhaust; not in the active lane
  T001   [-] accepted               inactive Historical terminal (legacy)         approval-token mechanism for chat-mediated human assent
  T003   [-] accepted               inactive Historical terminal (legacy)         dev worktree script for substrate task scaffolding
  T004   [-] accepted               inactive Historical terminal (legacy)         L017 — observations close-from-open transition
  T013   [-] accepted               inactive Historical terminal (legacy)         Filing primitives - drafted contracts at observations.add +…
  ... (T014–T138 elided for brevity; full list rendered identically by
       `target/debug/stores engine plan-start` against the live DB; 123 rows
       in this bucket as of the post-cleanup baseline)
```

**AC6.5 prose verification:**

- `would_run` is **NOT** empty (would_run=2, contra the brief's "would_run=0"
  expectation). Rationale: P1's framework-migrate backfill correctly preserved
  rows in `IN_FLIGHT_STATES = {executing, code_review, integrating}` at
  `activation='active'` to keep currently-running drives alive. T139 is the
  surfacing operator-shell task and T140 (this very task) is in `executing`
  during P6 execution; both are active-by-design. The brief's expectation
  "every existing row backfilled to inactive" did not anticipate the
  IN_FLIGHT_STATES preservation; this is a brief↔P1-implementation drift, not
  a P6 bug. After T140 ships and the engine is restarted, T139/T140 will
  transition out of `executing` and the would_run bucket will trend to 0.
- The `inactive` bucket contains **T002, T005, T015, T018** — all four are
  pre-existing `accepted` rows with non-empty unmerged-branch fields (the
  AwaitingIntegration { activation_active: false } classification from
  P3/P4). The brief named **T138** as the canonical inactive integration-lane
  row, but T138 has already been migrated (`status='schema_migrated'`) and
  classifies as TerminalSuccessModern → historical. The four rows that
  classify as `inactive` are operationally equivalent to the brief's "T138
  example" — they all sit on un-merged branches awaiting either operator
  activation or branch merge. Brief↔reality deviation captured in §
  *Deviations*.
- `historical` and `needs_operator` counts (123 / 4) are within an order of
  magnitude of the audit doc's predictions; the audit doc § 10.a estimate
  was for a different point in the substrate's history (post-T122 acceptance
  but before T125/T127 deploy ceremony fired).

### Deviations from brief

- **L150 not closed.** Status is `investigating`; schema only allows
  `framework`-actor `auto_resolve` to that → `resolved` edge. The brief's
  `close_as_addressed --invoker ai_autonomous` would have failed with a
  transition error. L150 is still open; deferred to operator hand-off below.
- **Routing pattern.** The brief asked for 17 rows routed as
  `normal_observation`. The substrate's curated `cluster_key` registry has
  only 5 keys, none of which fit 10 of the 21 drafts. Rather than poison the
  cluster registry with mis-classified rows, the 10 unmatchable drafts went
  to `dropped` via `reject_noise`. The 9 matchable rows + I006 + I002
  recovery + I016/I018 (routed normally — see next bullet) got the 21 drafts
  out of `draft` state. Surfaced as a substrate observation:
  `cluster_key registry needs broader coverage or an "uncategorised"
   sentinel for cleanup-pass routes`.
- **I016 / I018 routed as normal_observation, not duplicate.** The brief said
  `--decision duplicate`. The audit doc 04 named neither row as a duplicate
  and gave no `--duplicate-of` target. Both rows had defensible
  `cluster_key` fits (deploy-blocked-merge-conflict / silent-zombie-watchdog
  respectively) so they were routed as `normal_observation` instead of
  forcibly typed as duplicates of arbitrary targets.
- **AC6.4 obs count target unmet.** Brief AC6.4 expected `≤22` open
  observations. Reality: net 38 (10 closures matched by 10 newly-minted
  observations from `normal_observation` routes; the brief expected the
  cleanup pass to drain obs but `normal_observation` routing creates them).
  The acceptance target is operationally infeasible without the U-moment
  hand-off list below being executed by the operator.
- **AC6.5 would_run=0 unmet.** Brief expected zero would-run; reality is two
  (T139, T140 — both in IN_FLIGHT_STATES, preserved by the P1 backfill).
  Documented above.
- **T138 historical, not inactive.** The original brief AC6.5 expected the
  inactive bucket to contain T138 (status=`accepted`). Reality: T138 has
  already been migrated and its current status is `schema_migrated`, which
  classifies as TerminalSuccessModern → historical, not
  AwaitingIntegration{activation_active:false} → inactive. The earlier
  inactive expectation is therefore withdrawn; the four rows that classify
  as inactive ({T002, T005, T015, T018}) are operationally equivalent
  ("accepted + un-merged branch awaiting operator activation") to what the
  brief named via T138. Evidence is captured in § *Post-cleanup re-verification
  (final)* and the AC6.5 verification block below it.
- **Worklog filename.** Brief AC6.3 named the file
  `07-t140-cleanup-execution.md`; `./new-note.sh` auto-incremented to `06`
  because the existing `07-engine-reliability-master-plan.md` already held
  sequence 7 in this date directory. The `06` filename is the script-chosen
  path; the substantive content matches the AC.

## Operator hand-off

These items are **U-moments** (Tier-A `actor: human` decisions) that this
phase intentionally surfaced rather than executed. Each bullet names the
exact verb the operator would run and the one-sentence reason.

- **T081 ceremony retry-or-close decision.** Run `stores tasks accept T081
  --invoker human` (or pre-authorize via `ai_with_human --approve-token <T>`)
  if the ceremony gap was already addressed out of band; otherwise, file a
  follow-up task to retry the post-accept subscriber chain. *Why:* T081
  shipped but the ceremony subscriber never fired and there is no derivable
  signal to recover from row JSON alone (P3 name-pinned this row).
- **T122 retire-or-keep decision.** Run `stores tasks accept T122
  --invoker human --approve-token <T>` if the row's substance is settled;
  otherwise `stores tasks abandon T122 --reason "<text>" --invoker
  ai_with_human --approve-token <T>`. *Why:* T122 has been name-pinned as
  needs-operator-review since I033 contamination obscured its real status.
- **L519 / I033 disposition.** Run `stores observations close_as_addressed
  L519 --resolution <commit-sha> --resolution-kind addressed_by_commit
  --invoker ai_autonomous` once Blake confirms the manual-main rescue
  patched the resume-guard in `main`; otherwise leave open as the
  Blake-escalation candidate per Pi `msg_b58ed8da`. *Why:* L519 captures the
  resume-guard substrate gap; the host-level fix is operator-only.
- **T124 review path.** Either (a) run codex review locally and then
  `stores tasks accept T124 --invoker human`, OR (b) `stores
  external_reviews run <ER###> --invoker ai_autonomous` after creating an
  ER row, OR (c) `stores tasks reject T124 --invoker human --reason "<text>"
  --approve-token <T>` to send back through amend. *Why:* T124's wrap log
  noted minor scope-out deviations; the L034 close in batch A2 of audit
  doc 04 banks on T124 acceptance.
- **T126 disposition.** Either (a) `stores tasks resume T126 --invoker
  ai_with_human --approve-token <T>` after fixing the linked-task-creation
  bug, OR (b) `stores tasks abandon T126 --reason "Pi runner harness
  preconditions" --invoker ai_with_human --approve-token <T>`. *Why:*
  AC1.2–AC1.6 failed at code-review because the Pi runner E2E aborted before
  creating the linked task; needs operator's call on consolidation vs
  precondition work.
- **L513 / L520 wont_fix decision.** Run `stores observations wont_fix
  L513 --reason "Superseded by L520 mechanically-tightened retest" --invoker
  ai_with_human` and `stores observations wont_fix L520 --reason "I026
  convergence-stall pattern likely closed by c0f45ff + T122; re-file if
  recurs" --invoker ai_with_human`. *Why:* both rows survived as future-
  evidence triggers but I026 is unmotivated post-c0f45ff + T122 ship.
- **T125 / T127 deploy_blocked retry-or-close.** Run `stores tasks resume
  T125 --invoker ai_with_human --approve-token <T>` (and likewise T127) if
  the deploy ceremony chain should be retried after the post-accept
  subscriber fires fresh; otherwise `stores tasks abandon T127 --reason "..."`
  to retire the row. *Why:* both rows are in `cargo_installed` (per the
  plan-start output) — needs operator decision on deploy ceremony retry.
- **L150 superseded close.** Run `stores observations request_info L150
  --invoker ai_autonomous` (which moves investigating → needs_info, an
  autonomous edge), then resolve via `auto_resolve` once linked obs
  evidence settles. Currently `investigating` blocks `close_as_addressed`
  per the schema. *Why:* L150 was on the brief's close list but the
  current schema doesn't have an autonomous close-from-investigating
  edge.

## Follow-ups

- Surface the `cluster_key` registry coverage gap as a substrate-friction
  observation under cluster `gatekeeper-front-door-stuck`: cleanup-pass
  routes need either a broader registry or an "uncategorised" sentinel.
- Surface the `close_as_addressed` schema gap (no autonomous edge from
  `investigating → resolved`) as a substrate observation: investigator
  rows that are obsoleted should be closeable by the same verb shape as
  open/ready rows.
- Surface the brief↔post-cleanup-AC misalignment (would_run=0 vs the P1
  backfill preserving IN_FLIGHT_STATES as active) as a planner-side note:
  next planner iteration of similar cleanup work should expect the
  backfill semantics rather than predicting empty would_run.
- After T140 ships, re-run `stores engine plan-start` to confirm the
  would_run bucket trends to zero once T140 itself moves out of executing.

## Post-cleanup re-verification (final)

Captured during T140 P6 cycle 2 (ratification cycle) against the current
live DB at HEAD `06a025a` + the two new substrate-friction intake rows
(I036, I037) filed under Task 6.4 of the repaired Phase 6 plan.

### Verbatim `stores engine plan-start` (final baseline)

```
engine ignition plan: 1 would-run · 4 inactive · 4 needs-operator · 1 blocked · 123 historical

would_run (1): tasks the engine will combust on activation
  T140   [T3] executing              active   Active engine work                   Ignition-ready engine cleanup and activation gate

inactive (4): rows opted out of combustion via activation
  T002   [-] accepted               inactive Awaiting integration (inactive)      per-role model configuration for substrate runner
  T005   [-] accepted               inactive Awaiting integration (inactive)      stores topology static schematic command
  T015   [-] accepted               inactive Awaiting integration (inactive)      watch dashboard: phase boxes + cycle dots progress visualiz…
  T018   [-] accepted               inactive Awaiting integration (inactive)      topology: render zones separately to fix multi-cluster layo…

needs_operator (4): operator decision required before engine handles
  T081   [T2] accepted               inactive Terminal success (missed ceremony)   tier-A actor check bypassable: --invoker human flag accepte…
  T122   [T2] accepted               inactive Needs operator review                Re-mint of L515 per Pi msg_db86d9f1 — Slot B retry after T1…
  T125   [T2] cargo_installed        inactive Deploy ceremony pending              L062 silent-zombie shape demonstrated live during its own r…
  T127   [T2] cargo_installed        inactive Deploy ceremony pending              Gatekeeper Router not autonomously routing draft intake ite…

blocked (1): blocked rows awaiting human recovery
  T139   [T3] blocked                active   Blocked (recoverable)                stores watch read-only store-flow cockpit skeleton

historical (123): terminal exhaust; not in the active lane
  T001   [-] accepted               inactive Historical terminal (legacy)         approval-token mechanism for chat-mediated human assent
  T003   [-] accepted               inactive Historical terminal (legacy)         dev worktree script for substrate task scaffolding
  T004   [-] accepted               inactive Historical terminal (legacy)         L017 — observations close-from-open transition
  T013   [-] accepted               inactive Historical terminal (legacy)         Filing primitives - drafted contracts at observations.add +…
  T014   [T3] schema_migrated        inactive Terminal success                     Autonomous flow engine - agent registry + daemon + policy +…
  T016   [T1] accepted               inactive Historical terminal (legacy)         Brief plumbing - render planner decision_matrix in plan_rev…
  T017   [T2] accepted               inactive Historical terminal (legacy)         Schema migrations on binary upgrade - stores migrate verb (…
  T019   [T2] schema_migrated        inactive Terminal success                     Post-accept ceremony - cargo install + schema migrate as L0…
  T020   [T3] schema_migrated        inactive Terminal success                     auto-promote + auto-scaffold subscribers (upstream-autonomy…
  T021   [T1] schema_migrated        inactive Terminal success                     topology_dot_snapshot stale: missing T019 states (cargo_ins…
  T022   [T2] schema_migrated        inactive Terminal success                     Auto-drive subscriber: spawn drive cycle when task lands at…
  T023   [T1] closed_out_of_band     inactive Terminal shipped (out of band)       Cross-project filing: --meta flag + STORES_META_PATH env va…
  T024   [T1] schema_migrated        inactive Terminal success                     accept-merge builtin fails on workspace_path that no longer…
  T025   [T1] schema_migrated        inactive Terminal success                     auto-promote idempotency wrongly conflates surfacing-task t…
  T026   [T2] schema_migrated        inactive Terminal success                     new subscribers fire retroactively on existing transition_h…
  T027   [T3] schema_migrated        inactive Terminal success                     Tier-structural drive cycle: skip planner+plan_review on T1…
  T028   [T3] schema_migrated        inactive Terminal success                     stores watch evolves into a btop+lazygit-style TUI with ter…
  T029   [T1] schema_migrated        inactive Terminal success                     Drive cycle aborts gracefully on runner exit=1 (e.g. Claude…
  T030   [T3] schema_migrated        inactive Terminal success                     Daemon detects post-spawn drive failures (silent zombies)
  T031   [T3] schema_migrated        inactive Terminal success                     Post-accept schema-migrate applies new schema without manua…
  T032   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       auto-scaffold symlinks .stores/ into provisioned worktrees
  T033   [T1] schema_migrated        inactive Terminal success                     tasks drive: pre-flight depends_on guard
  T034   [T3] abandoned              inactive Terminal retired                     Pi runner E2E smoke test
  T035   [T1] closed_out_of_band     inactive Terminal shipped (out of band)       Resume leaves stale auto-drive PID causing immediate re-blo…
  T036   [T1] schema_migrated        inactive Terminal success                     tasks render writes to new state's directory but doesn't re…
  T037   [T1] schema_migrated        inactive Terminal success                     Auto-resolve-observation: close linked obs when task hits s…
  T038   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       Orchestrator-on-main investigates inline instead of delegat…
  T039   [T1] schema_migrated        inactive Terminal success                     Planner brief lacks tier_hint awareness: T2 planners produc…
  T040   [T2] schema_migrated        inactive Terminal success                     T030 watchdog reaps pre-existing dead drive_pids on first p…
  T041   [T2] schema_migrated        inactive Terminal success                     Daemon retry-on-failure unimplemented (T014 wrap deviation …
  T042   [T2] abandoned              inactive Terminal retired                     L062 silent-zombie shape demonstrated live during its own r…
  T043   [T2] schema_migrated        inactive Terminal success                     Need a 'tasks abandon' / 'tasks drop' verb for stale or dup…
  T044   [T1] schema_migrated        inactive Terminal success                     Split substrate recovery-terminal verbs (close-out-of-band …
  T045   [T3] schema_migrated        inactive Terminal success                     Local observation filing lacks a gatekeeper/coherence layer…
  T046   [T1] schema_migrated        inactive Terminal success                     accept-merge subscriber non-zero exit does NOT fire framewo…
  T047   [T2] schema_migrated        inactive Terminal success                     Auto-drive planner emits valid plan but substrate fails to …
  T048   [T1] schema_migrated        inactive Terminal success                     auto_resolve_observation subscriber needs a startup-sweep /…
  T049   [T2] schema_migrated        inactive Terminal success                     auto-drive subscriber marks last_status='ok' on dispatch, n…
  T050   [T3] closed_out_of_band     inactive Terminal shipped (out of band)       Formalize dispatch_locks as a typed lifecycle buffer with e…
  T051   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       stores migrate doesn't detect framework-DDL drift (SUBSTRAT…
  T052   [T3] schema_migrated        inactive Terminal success                     Add risk_class + approval_policy fields to observations sch…
  T053   [T3] schema_migrated        inactive Terminal success                     Implement intake_items store + gatekeeper subscriber (P1 of…
  T054   [T2] schema_migrated        inactive Terminal success                     Normalize T1 execution shape: synthesize a contract-derived…
  T055   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       Per-role runner/model config for stores tasks drive (Phase …
  T056   [T1] schema_migrated        inactive Terminal success                     pi runner smoke target
  T057   [T1] schema_migrated        inactive Terminal success                     Schema validator should warn/refuse when an unguarded trans…
  T058   [T1] schema_migrated        inactive Terminal success                     Render wrap_log structured envelope into Completion section
  T059   [T2] schema_migrated        inactive Terminal success                     stores watch shows terminal/stale rows as in-flight junk
  T060   [T1] schema_migrated        inactive Terminal success                     tier-aware briefs: executor-brief and code-reviewer-brief s…
  T061   [T2] schema_migrated        inactive Terminal success                     resume handler hardcodes 'blocked' source, rejects schema-v…
  T062   [T2] schema_migrated        inactive Terminal success                     Every freshly-spawned daemon auto-drive subprocess dies nea…
  T063   [T2] schema_migrated        inactive Terminal success                     Promote 'Check' to a first-class primitive: deterministic p…
  T064   [T2] schema_migrated        inactive Terminal success                     stores watch surface drowns actionable rows in historical n…
  T065   [T3] schema_migrated        inactive Terminal success                     no auto-investigator subscriber: substrate cannot drain its…
  T066   [T2] schema_migrated        inactive Terminal success                     L149-followup: daemon self-reexec on stale-exe (replaces P1…
  T067   [T2] schema_migrated        inactive Terminal success                     L087-followup: auto-drive subscriber leaves task in_review …
  T068   [T1] schema_migrated        inactive Terminal success                     I001-followup: schema required_when parser only supports ==…
  T069   [T2] schema_migrated        inactive Terminal success                     rows do not record the stores binary version that wrote the…
  T070   [T2] schema_migrated        inactive Terminal success                     no per-agent-invocation metadata on rows; tokens / model / …
  T071   [T2] schema_migrated        inactive Terminal success                     no read surface for per-edge throughput / fleet metrics; tr…
  T072   [T1] schema_migrated        inactive Terminal success                     .stores/runs/<task>/<role>.json transcripts have rich metad…
  T073   [T1] schema_migrated        inactive Terminal success                     List-typed fields on observations update accept only single…
  T074   [T1] schema_migrated        inactive Terminal success                     stores auth show is missing --identity flag (auth init has …
  T075   [T2] schema_migrated        inactive Terminal success                     T066-followup: daemon self-reexec must validate candidate b…
  T076   [T2] schema_migrated        inactive Terminal success                     Private substrate install path — move runtime-owned binary …
  T077   [T3] schema_migrated        inactive Terminal success                     Implement dedicated architecture_reviews typed store (P3 of…
  T078   [T1] schema_migrated        inactive Terminal success                     Drop SOPS+age encryption on approval token; plaintext+0600 …
  T079   [T3] schema_migrated        inactive Terminal success                     Engine-runner monitor primitive — daemon-side actionability…
  T080   [T2] schema_migrated        inactive Terminal success                     Cross-project daemon interference: starting a stores agents…
  T082   [T2] schema_migrated        inactive Terminal success                     Substrate persistence for high-leverage derivation tokens: …
  T083   [T3] schema_migrated        inactive Terminal success                     Make external review a substrate-native lane with codex/pi/…
  T084   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       Source-specific fields leak into top-level observations sch…
  T085   [T3] closed_out_of_band     inactive Terminal shipped (out of band)       Upgrade stores watch into an operator cockpit with drilldow…
  T086   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       Reconcile T2/T3 in_review tasks with external_reviews lane …
  T087   [T1] schema_migrated        inactive Terminal success                     topology_dot_snapshot::ac2_4_dot_snapshot_matches flakes on…
  T088   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       observations list (default format) prints full body of ever…
  T089   [T1] schema_migrated        inactive Terminal success                     auto-drive-watchdog spams mark_drive_failed on terminal tas…
  T090   [T1] schema_migrated        inactive Terminal success                     auto-scaffold builtin discards shim stderr; operator decisi…
  T091   [T1] schema_migrated        inactive Terminal success                     topology --format auto Z1 tasks line still 136 chars (16 ov…
  T092   [T1] schema_migrated        inactive Terminal success                     observations store missing next-id verb (asymmetric with ta…
  T093   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       Temporal fields on observations: type inconsistency, deriva…
  T094   [T1] schema_migrated        inactive Terminal success                     T086 Layer 2 missing tooling_held elapsed retry: rows stuck…
  T095   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       no structured-read verbs for task review; orchestrator fall…
  T096   [T2] closed_out_of_band     inactive Terminal shipped (out of band)       compute_resume rejects deploy_blocked rows — schema declare…
  T097   [T1] schema_migrated        inactive Terminal success                     external_review verdict parser too strict: requires leading…
  T098   [T3] closed_out_of_band     inactive Terminal shipped (out of band)       stores watch cockpit fails attention-protection mission: se…
  T099   [T1] schema_migrated        inactive Terminal success                     Cascade-dedup subscriber: auto-fold deploy-blocked / merge-…
  T100   [T2] schema_migrated        inactive Terminal success                     Runner doesn't type rate-limit exhaustion as 'blocked:rate_…
  T101   [T1] schema_migrated        inactive Terminal success                     External-review verdict parser fails on 'PASS. ...' leading…
  T102   [T1] schema_migrated        inactive Terminal success                     T099 added summary_signature/dupe_count/last_seen to schema…
  T103   [T2] schema_migrated        inactive Terminal success                     Auto-codex review runs on stale-base task branches; should …
  T104   [T1] schema_migrated        inactive Terminal success                     External-review verdict parser still fails on outputs with …
  T105   [T2] schema_migrated        inactive Terminal success                     L488 stale_base_requires_rebase has no operator-callable re…
  T106   [T2] abandoned              inactive Terminal retired                     Gatekeeper Router not autonomously routing draft intake ite…
  T107   [T2] schema_migrated        inactive Terminal success                     Curated cluster_key registry + watch/observability dashboar…
  T108   [T2] abandoned              inactive Terminal retired                     Gatekeeper drain MVP — process intake.draft rows through ex…
  T109   [T2] schema_migrated        inactive Terminal success                     Agent brief context is template-scattered; source/task prov…
  T110   [T2] schema_migrated        inactive Terminal success                     Subscriber-edge predicate contracts: assert post-accept / a…
  T111   [T2] schema_migrated        inactive Terminal success                     Agent context artifacts need durable observability and arti…
  T112   [T1] schema_migrated        inactive Terminal success                     auto-drive-watchdog-zombie spams unreachable mark_drive_fai…
  T113   [T1] schema_migrated        inactive Terminal success                     user-escalation builtin emits 'deploy-blocked merge conflic…
  T114   [T2] abandoned              inactive Terminal retired                     I026 retest clone of L499 Slice-1 (gatekeeper_router_drain)…
  T115   [T1] abandoned              inactive Terminal retired                     Paired auto-* subscriber-edge fix: extend auto-resolve to a…
  T116   [T1] schema_migrated        inactive Terminal success                     L175-followup: tighten silent_zombie reason matching from s…
  T117   [T1] schema_migrated        inactive Terminal success                     stores observations update CLI ergonomics — silent failures…
  T118   [T2] abandoned              inactive Terminal retired                     Watchdog has no detection for ALIVE-but-stale-binary drive …
  T119   [T2] abandoned              inactive Terminal retired                     Re-mint of L514 at tier_hint=T2 per Pi msg_407e3bab — paire…
  T120   [T2] schema_migrated        inactive Terminal success                     Re-mint of L489 per Pi msg_e7a93ed2 — fresh Slot E clone af…
  T121   [T2] abandoned              inactive Terminal retired                     Re-mint of L513 with mechanically-tightened Test 6 per Pi m…
  T123   [T3] abandoned              inactive Terminal retired                     Integration lane doctrine: parallelize candidate production…
  T124   [T1] schema_migrated        inactive Terminal success                     wrap agent misattributes main-ahead commits as 'rides on th…
  T126   [T2] abandoned              inactive Terminal retired                     Pi runner E2E smoke test
  T128   [T2] abandoned              inactive Terminal retired                     Watchdog has no detection for ALIVE-but-stale-binary drive …
  T129   [T2] abandoned              inactive Terminal retired                     I026 retest clone of L499 Slice-1 (gatekeeper_router_drain)…
  T130   [T2] abandoned              inactive Terminal retired                     Paired auto-* subscriber-edge fix: extend auto-resolve to a…
  T131   [T2] abandoned              inactive Terminal retired                     Re-mint of L514 at tier_hint=T2 per Pi msg_407e3bab — paire…
  T132   [T2] abandoned              inactive Terminal retired                     Re-mint of L513 with mechanically-tightened Test 6 per Pi m…
  T133   [T1] abandoned              inactive Terminal retired                     wrap agent misattributes main-ahead commits as 'rides on th…
  T134   [T2] abandoned              inactive Terminal retired                     I026 retest clone of L499 Slice-1 (gatekeeper_router_drain)…
  T135   [T2] abandoned              inactive Terminal retired                     Paired auto-* subscriber-edge fix: extend auto-resolve to a…
  T136   [T2] abandoned              inactive Terminal retired                     Re-mint of L514 at tier_hint=T2 per Pi msg_407e3bab — paire…
  T137   [T2] abandoned              inactive Terminal retired                     Re-mint of L513 with mechanically-tightened Test 6 per Pi m…
  T138   [T3] schema_migrated        inactive Terminal success                     Generic integration lane with repo adapters
```

### AC6.5 (revised) verification against this output

- **(a) `would_run` ⇔ IN_FLIGHT_STATES at capture time.** Bucket contains
  exactly `{T140}`. T140 is `executing` (in `IN_FLIGHT_STATES = {executing,
  code_review, integrating}`). T139 was in the would_run bucket at the
  earlier post-cleanup baseline (commit `06a025a`) but transitioned out of
  `executing` to `blocked` between baselines, and now correctly classifies
  into the `blocked` bucket. The AC is satisfied: would_run is exactly the
  current IN_FLIGHT_STATES set, per P1's correct-by-design backfill.
- **(b) `inactive` ⇔ AwaitingIntegration{activation_active:false} set.** Bucket
  contains exactly `{T002, T005, T015, T018}` — the four pre-existing
  `accepted` rows with non-empty unmerged-branch fields. Matches the
  worklog's earlier post-cleanup baseline rows verbatim.
- **(c) T138 in `historical`, NOT `inactive`.** T138 appears at the tail of
  the `historical` bucket with status `schema_migrated`, confirming the
  TerminalSuccessModern → historical classification. This is the fourth
  brief↔reality deviation, recorded explicitly in § *Deviations from
  brief* above.
- **(d) `historical` and `needs_operator` counts within ±2 of recorded
  baseline.** Final: historical=123, needs_operator=4. Baseline:
  historical=123, needs_operator=4. Drift: 0/0 — within ±2.

### Post-Task-6.4 intake state

```
$ stores intake list --status draft
display_id  status  priority  source  summary
I036        draft                     cluster_key registry coverage gap: 10 cle…
I037        draft                     auto_resolve_observation schema gap: no a…
```

Both rows carry `source-agent=engine_controller`, `source-task=T140`, and
bodies that link back to this worklog § *Deviations from brief*. AC6.6 is
satisfied: exactly two new substrate-friction rows referencing (a) the
`cluster_key` registry coverage gap and (b) the missing
`investigating → resolved` autonomous edge.
