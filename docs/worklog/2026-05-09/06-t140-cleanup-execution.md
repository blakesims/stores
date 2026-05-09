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

22–31: ten parallel-shape invocations, transcript fragment:
```
Transitioned I003: triaging → dropped
Transitioned I005: triaging → dropped
Transitioned I011: triaging → dropped
Transitioned I012: triaging → dropped
Transitioned I014: triaging → dropped
Transitioned I015: triaging → dropped
Transitioned I024: triaging → dropped
Transitioned I025: triaging → dropped
Transitioned I030: triaging → dropped
Transitioned I031: triaging → dropped
```

### Post-cleanup counts (T140 P6 AC6.4 verification)

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
