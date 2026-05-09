# Queue-curator disposition audit + fold proposals

**Date:** 2026-05-09 (post-cleanup)
**Author:** queue-curator (Claude)
**Type:** audit / proposal — read-only; **no mutations performed**
**Coordination thread:** `~/repos/.agent-comm/threads/2026-05-09-01-queue-triage-schema-migration.md`

## Why this exists

Blake asked queue-curator and a parallel agent to triage the current backlog and propose a clean schema mapping for the residue + draft contracts. The two agents converged on a derived-first sequence (see thread): `operator_disposition` as a derived view (no migration), `stores engine plan-start` as a read-only operator surface, legacy retirement via existing `close-out-of-band` verb, open-obs sweep via `triage_bucket`. Blake then expanded scope to include explicit activation gating in the upcoming architecture-review brief (other agent's deliverable).

This document is queue-curator's parallel deliverable: the read-only audit that feeds the architecture-review brief and identifies the queue hygiene that can happen independently of the schema decision.

**Doctrine:** classify before migrating; derive before stored-column promotion; use existing verbs over new ones.

## Counts at audit time (2026-05-09)

| Surface | Status | Count |
|---|---|---|
| Tasks | accepted | 13 |
| Tasks | schema_migrated | 80 |
| Tasks | cargo_installed | 2 (T125, T127) |
| Tasks | executing | 1 (T139) |
| Tasks | abandoned | 21 |
| Tasks | closed_out_of_band | 15 |
| Observations | open | 35 |
| Observations | ready | 3 (L032, L538, L540) |
| Observations | investigating | 1 (L150) |
| Observations | resolved | 490 |
| Observations | wont_fix | 11 |
| Intake | draft | 21 |
| Intake | routed | 11 |
| Intake | dropped | 3 |

**Substrate friction surfaced during this audit (file as observations after Blake unblocks writes):**
1. `stores watch` errors with `No such device or address (os error 6)` — front-of-engine display dead.
2. `stores tasks list --status` rejects multi-value flags (already filed as L482).
3. Skill template references `intake_items`; actual table is `intake` — would silently fail in the boilerplate watcher loop.

---

## 1. Task disposition mapping (the schema-overload core)

The 13 `accepted` rows mix three semantics:

| Row | Status | Title (truncated) | Proposed disposition | Reason |
|---|---|---|---|---|
| T001 | accepted | approval-token mechanism | `historical_terminal_legacy` | Pre-integration era; shipped before `schema_migrated` ceremony existed. |
| T002 | accepted | per-role model configuration | `historical_terminal_legacy` | Same era. |
| T003 | accepted | dev worktree script | `historical_terminal_legacy` | Same era. |
| T004 | accepted | observations close-from-open transition | `historical_terminal_legacy` | Same era. |
| T005 | accepted | stores topology static schematic | `historical_terminal_legacy` | Same era. |
| T013 | accepted | filing primitives — drafted contracts at obs.add + tier_hint on tasks | `historical_terminal_legacy` | Same era. |
| T015 | accepted | watch dashboard phase boxes / cycle dots | `historical_terminal_legacy` | Same era. |
| T016 | accepted | brief plumbing — render planner decision_matrix | `historical_terminal_legacy` | Same era. |
| T017 | accepted | schema migrations on binary upgrade | `historical_terminal_legacy` | Same era. |
| T018 | accepted | topology zone-separated rendering | `historical_terminal_legacy` | Same era. |
| T081 | accepted | tier-A actor check bypass closed | `terminal_success_missed_ceremony` | engine-health says T081 ✅ shipped (line 76 / 139); never progressed `cargo_installed` → `schema_migrated`. Subscriber gap candidate. |
| T122 | accepted | re-mint of L515 per Pi msg_db86d9f1 | `needs_operator_review` | Recent re-mint after T119 silent_zombie + I033 contamination. Engine-health snapshot doesn't confirm shipped/retired. **Surface to Blake.** |
| T138 | accepted | Generic integration lane with repo adapters | `awaiting_integration` | Current integration-lane work; this is the row L538 ratified. Status semantically distinct from the legacy 12. |

**Plus:**
| T125 | cargo_installed | L062 silent-zombie demo | `deploy_ceremony_pending` | Stranded mid-post-accept ceremony. engine-health says accepted after fresh ER PASS. |
| T127 | cargo_installed | Gatekeeper Router auto-route | `deploy_ceremony_pending` | Same shape as T125. |
| T139 | executing | watch read-only cockpit skeleton | `active_engine_work` | Live drive. |

**80 `schema_migrated` rows:** all → `terminal_success_modern`. Bulk class.
**21 `abandoned` rows:** all → `terminal_retired`. Bulk class.
**15 `closed_out_of_band` rows:** all → `terminal_shipped_oob`. Bulk class.

### Proposed clean schema (derived view, no migration)

The other agent's 4-field proposal collapses to **one derived column** under the converged plan:

```
operator_disposition = derive(status, accepted_at, claimed_by, branch, last_transition):
  case status:
    'executing'                                    → 'active_engine_work'
    'cargo_installed' / 'schema_migrating'         → 'deploy_ceremony_pending'
    'accepted' AND <pre-2026-05-04>                → 'historical_terminal_legacy'  (or 'terminal_success_missed_ceremony' for T081)
    'accepted' AND <post-2026-05-04> AND <branch unmerged>
                                                   → 'awaiting_integration'
    'accepted' AND <post-2026-05-04> AND <merged>  → 'terminal_success_modern'
    'schema_migrated'                              → 'terminal_success_modern'
    'abandoned'                                    → 'terminal_retired'
    'closed_out_of_band'                           → 'terminal_shipped_oob'
    'rejected'                                     → 'terminal_rejected'
    'blocked' / 'deploy_blocked'                   → 'blocked_recoverable'
    'planning' / 'plan_review' / 'ready' / 'code_review' / 'in_review'
                                                   → 'engine_actionable'
```

**Test fixture (must pass before merge):**
- T001–T018 → `historical_terminal_legacy`
- T081 → `terminal_success_missed_ceremony`
- T122 → `needs_operator_review` (or whichever bucket survives audit)
- T125, T127 → `deploy_ceremony_pending`
- T138 → `awaiting_integration`
- T139 → `active_engine_work`
- L032, L538, L540 → `linked_to_active_task` (ready obs paired with live task; not "about to mint")
- L150 → `linked_to_terminal_task` (investigating obs paired with shipped task — stale)

### Cleanup that does **not** require new schema

| Row | Verb | Reason |
|---|---|---|
| T001–T005, T013, T015–T018 | `tasks close-out-of-band <id> --reason "shipped_legacy_accepted_pre_integration" --merge-commit <SHA>` | Existing T044 verb; preserves audit trail; removes from `accepted` overload. |
| T081 | Investigate why ceremony didn't progress; either retry-deploy (if subscriber missed) or close-out-of-band with reason `shipped_subscriber_gap`. **Needs operator decision.** | Existing verbs. |
| T122 | Read full cycle history; either close-out-of-band (if shipped) or `tasks abandon` (if retired). **Needs operator decision.** | Existing verbs. |

**No schema change required for this cleanup.** The derived view + use of existing terminal verbs is sufficient.

---

## 2. Open observations classified into 7 buckets

The bucket vocabulary from the thread (real_backlog / duplicate_keeper / superseded_by_task / stale_battle_scar / needs_investigation / arch_review_candidate / doctrinal_doc_only) applied to the 39 open + ready + investigating rows:

### Bucket: `superseded_by_task` — should close, work shipped
| Obs | Linked task | Shipping evidence |
|---|---|---|
| L032 (ready, T013) | T032 | engine-health line 121 ✅ T032 — auto-scaffold symlinks. **Stale battle scar / status mismatch.** |
| L150 (investigating, T048) | T099 | engine-health line 107 ✅ T099 — cascade-dedup subscriber. **Stale battle scar / status mismatch.** |
| L002 | T043 | engine-health line 169 ✅ T043 — `tasks abandon` shipped. |
| L154 (draft) | T053 | engine-health line 160 ✅ T053 — Router seam shipped; phase boundary advice implicitly followed. |
| L155 (draft) | T077 | engine-health line 164 ✅ T077 — architecture_reviews store shipped. |
| L076 | T039 (partial) | T039 ✅ shipped tier-aware brief; L076's "no auto-recovery edge" remainder may still be open — re-read before closing. |

### Bucket: `duplicate_keeper` — fold cluster
| Cluster | Members | Proposed keeper |
|---|---|---|
| `silent_zombie_drive_failed` | L517 (T118), L521 (T119), L524 (T116), L526 (T121), L531 (T123) | **L517** (lowest ID, oldest framework instance). Fold L521/524/526/531 with `merge-target-id=L517`. |

### Bucket: `arch_review_candidate` — promote to A### via T077
| Obs | Why arch_review |
|---|---|
| L084 | priority vs severity conflation — schema doctrine question, named in engine-health priority ladder |
| L085 | first-class duplicate_of / merged_into — schema doctrine; aligned with this thread's decision to defer stored columns |
| L086 | capability vs capability_ids no documented rule — schema doctrine |
| L486 (draft, T3) | canonical mainline control-plane doctrine — explicitly named in engine-health priority ladder; doctrine-level |

### Bucket: `real_backlog` — keep open, ratify on demand
| Obs | Why |
|---|---|
| L006 (T2) | observations runner asymmetry; engine-health ⚪ T2 |
| L012 (T3) | agent context inspector; engine-health ⚪ T3 |
| L019 (T3) | DockerRunner / standardized sandbox; engine-health ⚪ T3 |
| L028 (T2) | drive-spawned agents lack /observe; engine-health ⚪ T2 |
| L035 (T3) | schema-enforced inter-agent context refs; engine-health ⚪ T3 |
| L061 (T2) | no pre-promotion acceptance precheck; engine-health ⚪ T2 |
| L070 | accept-merge conflict drops side effects; engine-health ⚪ |
| L072 | code-reviewer REPLAN dead-ends as blocked; engine-health ⚪ |
| L108 (T2) | fire_on_entry_follow_ons retroactive; engine-health ⚪ T2 |
| L116 (T2) | seeder race; engine-health ⚪ T2 |
| L121 | Pi runner timeout / liveness |
| L122 (T2) | manual drive doesn't set drive_pid; engine-health ⚪ T2 |
| L156 (draft, T3) | fast-track waits for Check primitive — Check shipped (T063 ✅), now ratifiable |
| L157 (draft, T3) | cluster-key registry P5 — sequencing note; could fold into L173 |
| L172 (T3) | fast-track auto-execution + Check P4; engine-health ⚪ T3 |
| L481 (T2) | observations add stale-schema CLI bug |
| L482 (T2) | CLI multi-value flag splitting |
| L492 (T3) | schema-yaml vs ddl.rs drift class |
| L497 (T3) | external-review verdict parser hardening |
| L500 (draft, T2) | gatekeeper drain failure semantics — concrete narrow fix, ratifiable |
| L529 (T3) | stores watch as flowtop graph monitor — overlap with L540/T139 in flight; may be foldable post-T139 |
| L539 (draft, T2) | obs contract authoring CLI brittle (queue-curator's filing) |

### Bucket: `linked_to_active_task` — leave alone
| Obs | Linked task | Why |
|---|---|---|
| L538 (ready) | T138 | T138 driving against this contract. Auto-resolves on T138 ship. |
| L540 (ready) | T139 | T139 driving against this contract. Auto-resolves on T139 ship. |

### Bucket counts
- superseded_by_task: 6 (close)
- duplicate_keeper cluster: 1 cluster of 5 (fold to 1)
- arch_review_candidate: 4 (promote via T077)
- real_backlog: 22 (keep)
- linked_to_active_task: 2 (leave alone)

After classification, **22 of the 39 are real backlog**. The rest are noise that derived view + close + fold + promote can quiet.

---

## 3. Intake routing proposal (21 drafts)

| Intake | Source | Date | Proposed routing | Reason |
|---|---|---|---|---|
| I002 | code_reviewer | 05-07 | `normal_observation` | codex grepping raw .sqlite — substantive bug |
| I003 | code_reviewer | 05-07 | `normal_observation` | NO_COLOR test sensitivity — substantive flake |
| I004 | executor | 05-07 | `normal_observation` | e2e flake — substantive |
| I005 | substrate_agent | 05-07 | `normal_observation` | auto-cleanup gap — substantive |
| I006 | engine_controller | 05-07 | `doctrinal_doc_only` | dual-path-audit SOP — pure doc edit per CLAUDE.md doc-only exception |
| I010 | orchestrator | 05-08 | `normal_observation` | T041 rate-limit unawareness |
| I011 | orchestrator | 05-08 | `normal_observation` | matches engine-health GAP-log-fd-drift; could merge into existing GAP |
| I012 | orchestrator | 05-08 | `normal_observation` | matches engine-health GAP-stop-foreground; could merge into existing GAP |
| I013 | orchestrator | 05-08 | `normal_observation` | reviewer-runner empty-commit thrash — substantive |
| I014 | orchestrator | 05-08 | `normal_observation` | cargo test --lib --release flake — test infra |
| I015 | orchestrator | 05-08 | `normal_observation` | auto-detect already-merged feature branches — overlaps with this thread's discussion; possibly arch_review_candidate |
| I016 | orchestrator | 05-08 | `duplicate` | matches L150 / T099 cascade-dedup ✅ shipped |
| I017 | engine_controller | 05-08 | `normal_observation` | stale dispatch_locks observability — engine-health priority 9 |
| I018 | engine_controller | 05-08 | `duplicate` | rate-limit typing — matches L484 / T100 in flight |
| I023 | orchestrator | 05-08 | `normal_observation` | zombie-pid watchdog flips in_review→blocked while ER running |
| I024 | queue_curator | 05-08 | `normal_observation` | auto-resolve coverage gap — substantive (paired with manual cleanup) |
| I025 | queue_curator | 05-08 | `normal_observation` | auto-promote re-fire edge |
| I026 | queue_curator | 05-08 | `normal_observation` | planner paraphrases MUST-level invariants |
| I030 | engine-controller | 05-09 | `normal_observation` | auth.token plaintext missing while .hash intact |
| I031 | queue-curator | 05-09 | `normal_observation` | obs update list-field stepwise REPLACES — CLI ergonomics |
| I035 | engine-controller | 05-09 | `normal_observation` | silent_zombie pattern under WIP=5 / T2 cycle-3 — analytical, possibly the keeper for the L517 cluster |

**Totals after routing:**
- normal_observation: 18 → ratifiable observations
- duplicate: 2 → fold into existing keepers
- doctrinal_doc_only: 1 → CLAUDE.md edit; no observation row

---

## 4. Silent-zombie cluster fold proposal (concrete verbs)

**Cluster:** L517, L521, L524, L526, L531 — all share signature `drive-failed: task T### silent_zombie on branch 'feat/T###-auto-promoted-l###'`.

**Rationale:** T099 cascade-dedup is shipped (engine-health line 107) and prevents NEW dupes; these five are pre-T099 cascade artifacts. Mechanical evidence — same summary signature, same root cause, different task IDs.

**Choice of keeper:** L517 (lowest ID; preserves the framework-instance audit trail). I035 (analytical observation about the pattern) is **not** the right keeper because it carries different intent — pattern analysis, not framework instance.

**Proposed verbs (NOT executed; awaiting Blake's go):**
```
stores observations close_as_addressed L521 --resolution L517 --resolution-kind addressed_by_observation --merge-target-id L517 --invoker ai_autonomous
stores observations close_as_addressed L524 --resolution L517 --resolution-kind addressed_by_observation --merge-target-id L517 --invoker ai_autonomous
stores observations close_as_addressed L526 --resolution L517 --resolution-kind addressed_by_observation --merge-target-id L517 --invoker ai_autonomous
stores observations close_as_addressed L531 --resolution L517 --resolution-kind addressed_by_observation --merge-target-id L517 --invoker ai_autonomous
```

**Then on L517 (the keeper):** consider amending its summary or notes to record the fold count + member IDs for audit, or leaving as-is and relying on `merge_target_id` reverse pointers.

**Risk:** none. All 5 rows are framework-fired evidence-only observations; no contracts, no linked tasks-in-flight. Folding does not lose information (the merge_target_id reverse-pointer preserves the audit trail per L085 schema's whispered duplicate_of mechanism).

---

## 5. Draft-contract triage (7 rows)

| Obs | Tier | Status | Proposed action | Reason |
|---|---|---|---|---|
| L154 | T1 | draft | `close_as_addressed --resolution T053 --resolution-kind addressed_by_task` | T053/L142 shipped Router seam. Phase boundary advice implicitly followed. |
| L155 | T3 | draft | `close_as_addressed --resolution T077 --resolution-kind addressed_by_task` | T077 ✅ shipped architecture_reviews store. |
| L156 | T3 | draft | **Amend contract; ratify** | T063 Check ✅ shipped (prerequisite met). Now ratifiable. Pair with L172 fast-track. |
| L157 | T3 | draft | **Fold into L173** (`close_as_addressed --resolution L173 --resolution-kind addressed_by_observation --merge-target-id L173`) | L157 is sequencing advice for L173 cluster_key registry. Not substrate work itself. |
| L486 | T3 | draft | **Promote to A### via T077** (architecture_review_candidate) | Doctrine-level; named in engine-health priority ladder. Use intake gatekeeper `route --decision arch_review_candidate` or direct `architecture_reviews add`. |
| L500 | T2 | draft | **Ratify contract; promote to T2 task** | Concrete narrow gatekeeper drain hardening. No supersession. |
| L539 | T2 | draft | **Ratify contract; promote to T2 task** (or cluster with L481+L482 as one CLI-ergonomics-pass T2) | Obs-contract CLI brittle. Sister to L481/L482; could fold all three into one task. |

---

## 6. Pending architecture-review questions for Pi (other agent's brief)

These overlap with what the other agent is drafting; flagging for cross-reference:

1. Does `operator_disposition` belong in the substrate (derived SQL VIEW or rust function on `tasks`/`observations`/`intake`) or entirely in the watch layer?
2. What's the activation-gating shape Blake authorized? Per-row `activation_state` field, or a separate `task_activation_queue` join table, or a session-scoped manifest file?
3. Does the derived `operator_disposition` need to feed subscriber predicates (e.g. dispatch only on `engine_actionable`) or is it operator-CLI-only for the first ship?
4. Should `historical_terminal_legacy` be a real terminal status (replacing `accepted` for T001–T018) via close-out-of-band, or stay an inferred bucket?
5. Confirm: T081 ceremony-gap and T122 re-mint contamination need operator decision before the derived view test fixture is locked.

## 7. Recommended order of execution (post-Blake-unblock)

The architecture-review brief (other agent) is the bottleneck for stored-schema or derived-view shape. Independent of that, the following are safe queue hygiene that reduces the dirty fixture before the new primitive lands:

1. **Surface T081 + T122 to Blake** for terminal-disposition decision. (Read-only audit + question.)
2. **Fold silent-zombie cluster** (4 close_as_addressed verbs against L517 keeper).
3. **Close 6 superseded-by-task obs** (L032, L150, L002, L154, L155, possibly L076 after re-read).
4. **Promote L156, L500, L539** as ratifiable contracts (the contract-state hop and `tasks add` from observation; L156 amend first).
5. **Fold L157 into L173** (one close_as_addressed).
6. **Promote L486** via architecture-review path (intake `route --decision arch_review_candidate` or direct `architecture_reviews add`).
7. **Cluster L084/L085/L086 for arch-review** as schema-doctrine question batch.
8. **Sweep 18 normal_observation intake** with gatekeeper `route` verb.
9. **Drop I006** as doctrinal_doc_only; edit CLAUDE.md instead.
10. **Mark I016 + I018 duplicate** of L150/L484.
11. **Retire T001–T018, T015–T018** via `tasks close-out-of-band` with reason `shipped_legacy_accepted_pre_integration`.

Steps 2–11 use **only existing verbs** (close_as_addressed, gatekeeper route, tasks close-out-of-band). No new schema, no migration.

After all of that, the dirty fixture is:
- 1 active row (T139)
- 1 awaiting_integration row (T138)
- ≤2 deploy_ceremony_pending rows (T125, T127 — pending Blake's ceremony retry decision)
- 22-ish real_backlog observations
- 0 superseded / dupe / legacy noise

That's the surface against which the derived `operator_disposition` view should be tested.

---

## 8. Footnotes

- **No mutations performed by this audit.** All findings are read-only SELECTs, CLAUDE.md / engine-health.md cross-reference, and proposed verb invocations.
- **Coordination thread carries the schema discussion.** This audit feeds into the other agent's architecture-review brief; the brief is the ratification artifact.
- **Substrate friction filed as observations later** — `stores watch` brokenness, skill template `intake_items` bug, CLI multi-value flag bug all belong as L### rows once Blake unblocks writes.

## 9. Addendum (2026-05-09, post-Blake-decisions, two rounds)

### 9.a Round 1 — narrowed v1 (msg_89e6f719) corrections

Blake's first ask_user pass narrowed scope to tasks-only. Two corrections to §1/§7 from that round:

**(a) §1's legacy retire proposal was invalid.** I proposed `tasks close-out-of-band <id> --reason shipped_legacy_accepted_pre_integration` to retire T001-T018. The schema's transition allowlist on `close-out-of-band` **refuses from `accepted` and `cargo_installed`** (verb is intended for `complete` / `blocked` / `deploy_blocked`-shape recovery, not terminal-success rows). The verb cannot be used as written.

**(b) §7 step 11 (legacy retire) is dropped.** Blake's decision is **derived-only classification, no retire**. The 10 legacy rows + T081 + T122 keep raw status `accepted`; the derived `operator_disposition` view classifies them without mutating the rows. Cleaner than my proposal — preserves audit trail, sidesteps the schema-allowlist friction.

### 9.b Round 2 — scope expanded again (msg_dd28a2c6): T140 owns full DB cleanup

Blake rejected the round-1 narrowing as too slow/partial. **T140 (`Ignition-ready engine cleanup and activation gate`) was created** and now owns:

- Derived task `operator_disposition` view
- Per-row activation primitive (default-inactive for new/promoted)
- Schema-enforced gating for work-starting task paths
- Observation auto-promote mints inactive tasks
- Tasks-only `stores engine plan-start`
- Exhaustive subscriber-class taxonomy
- T138 transition behavior under activation
- Backfill/migration behavior for existing rows
- **Full audited cleanup/remapping of tasks + observations + intake** so stale/superseded/duplicate rows are folded/closed/routed and the queue is understandable
- Watch/status consumption of disposition where needed

The "obs/intake cleanup is parallel-safe hygiene that defers to a separate decision" framing from round 1 is **superseded**. T140's acceptance criteria require the cleanup to land alongside the activation primitive. Hygiene may execute in batches inside T140's plan, but it is no longer out-of-scope.

**§7 sequence under T140:** all 10 surviving steps (2, 3, 5, 6, 7, 8, 9, 10 plus the §1 disposition mapping itself) become T140 plan inputs. Step 11 (legacy retire) remains dropped per round-1 schema-fact correction. The derived view classifies legacy rows in place.

**Open questions still on T140's planner**:
1. Activation-state of pre-existing rows on ship day (backfill default).
2. Exhaustive subscriber-class taxonomy (which subscribers count as "work-starting").
3. T138's specific transition path post-activation.
4. Schema-enforcement of the gate (Check / `StateAction when:` predicate vs. runtime branching).

## 10. Bucket reconciliation against Blake's expected end groupings

Blake (via msg_dd28a2c6) gave the expected post-cleanup groupings. This section maps every row in the live DB into Blake's vocabulary. **This is the row-level feedstock for T140's planner.**

### 10.a Tasks (Blake's 10 buckets)

| Bucket | Rows | Count | Provenance |
|---|---|---|---|
| `active_work` | T139 | 1 | status=executing; live drive |
| `inactive_ready` / `awaiting_activation` | (none yet) | 0 | Will populate post-activation primitive ship: ratifiable rows minted inactive |
| `awaiting_integration` | T138 | 1 | status=accepted; current integration-lane work; L538 ratified contract |
| `blocked_recoverable` | (none) | 0 | No `blocked` / `deploy_blocked` rows in current snapshot |
| `needs_operator_review` | T081, T122 | 2 | T081: shipped per engine-health but stuck `accepted` (subscriber gap); T122: re-mint post-I033 contamination, status semantics unclear |
| `deploy_ceremony_pending` | T125, T127 | 2 | Both `cargo_installed`; stranded mid-post-accept ceremony |
| `historical_terminal_legacy` | T001-T005, T013, T015-T018 | 10 | `accepted` from pre-integration era |
| `terminal_success` | 80 `schema_migrated` rows | 80 | Modern post-accept-ceremony complete |
| `terminal_shipped_oob` | 15 `closed_out_of_band` rows | 15 | Manual-merge-ceremony recovery terminal |
| `terminal_retired` | 21 `abandoned` rows | 21 | Intentionally retired (superseded/duplicate/stale) |

**Total: 132 task rows mapped, all 10 buckets referenced (8 occupied today, 2 will populate post-primitive-ship).**

### 10.b Observations (Blake's 10 buckets)

| Bucket | Rows | Count | Notes |
|---|---|---|---|
| `linked_to_active_task` | L538 (T138), L540 (T139) | 2 | Ratified contracts driving live tasks |
| `linked_to_inactive_task` | (none yet) | 0 | Will populate post-primitive-ship as auto-promote mints inactive |
| `ready_to_promote_inactive` | (none cleanly today) | 0 | L032 is `ready` but linked-to-shipped T013 → belongs in `superseded_or_resolved` instead |
| `real_backlog` | L006, L012, L019, L028, L035, L061, L070, L072, L108, L116, L121, L122, L156 (post-amend), L172, L481, L482, L492, L497, L500 (ratifiable), L529, L539 (ratifiable) | 22 | Open + ratifiable; not superseded; not architecture-doctrine |
| `needs_investigation` | (none cleanly today) | 0 | L150 is currently `investigating` but is actually superseded by T099 ✅ |
| `arch_review_candidate` | L084, L085, L086, L486 | 4 | Schema-doctrine questions; route via T077 architecture_reviews |
| `duplicate_or_folded` | L521, L524, L526, L531 (fold into L517) | 4 | Silent-zombie cluster members |
| `superseded_or_resolved` | L002 (T043), L032 (T032), L076 (T039), L150 (T099), L154 (T053), L155 (T077), L157 (→L173) | 7 | Work shipped or sequencing-advice folded |
| `terminal_resolved` | 490 `resolved` rows | 490 | Already terminal |
| `terminal_wont_fix` | 11 `wont_fix` rows | 11 | Already terminal |

**Plus 1 keeper for the silent-zombie cluster:** L517 stays open as `real_backlog` (or its own pattern-keeper bucket — clarify with planner).

**Total: 540 observation rows mapped. Open + investigating + ready (39 rows) split: 22 real_backlog · 4 arch_review · 4 dupe-fold · 7 superseded · 2 linked-to-active · 0 needs_investigation · 0 ready_to_promote_inactive (until primitive ships).**

### 10.c Intake (Blake's 7 buckets)

| Bucket | Rows | Count | Notes |
|---|---|---|---|
| `draft_triage_backlog` | (transient) | 21 today | Pre-routing inbox |
| `routable_to_observation` | I002, I003, I004, I005, I010, I011, I012, I013, I014, I017, I023, I024, I025, I026, I030, I031, I035 | 17 | Substantive bug/gap reports → gatekeeper `route --decision normal_observation` |
| `duplicate` | I016 (matches L150/T099), I018 (matches L484/T100) | 2 | Route as duplicate |
| `doctrinal_doc_only` | I006 | 1 | Edit CLAUDE.md instead of routing; route as `dropped` with reason |
| `arch_review_candidate` | I015 | 1 | "Auto-detect already-merged feature branches" overlaps with mainline-control-plane doctrine (L486); route as arch-review |
| `terminal_routed` | 11 already-routed | 11 | Already terminal |
| `terminal_dropped` | 3 already-dropped | 3 | Already terminal |

**Total: 35 intake rows mapped. Of the 21 drafts: 17 → observation, 2 → duplicate, 1 → doctrinal, 1 → arch-review.**

### 10.d Activation classification

Per Blake's contract: every active-bucket task (`active_work`, `inactive_ready`, `awaiting_integration`, `blocked_recoverable`, `needs_operator_review`, `deploy_ceremony_pending`) needs an activation decision on ship day.

| Task | Bucket | Recommended ship-day activation | Reason |
|---|---|---|---|
| T139 | active_work | **active** | Live drive in progress; deactivating would interrupt |
| T138 | awaiting_integration | **active** OR **inactive** — needs Blake | Currently moving into integration; deactivating freezes integration work. If inactive, operator must explicitly activate to integrate. |
| T125 | deploy_ceremony_pending | **inactive** | Stranded; should not auto-resume ceremony without operator review |
| T127 | deploy_ceremony_pending | **inactive** | Same |
| T081 | needs_operator_review | **inactive** | Operator decides terminal disposition; no auto-action |
| T122 | needs_operator_review | **inactive** | Same |

Terminal buckets (`historical_terminal_legacy`, `terminal_success`, `terminal_shipped_oob`, `terminal_retired`) are not activation-relevant. The activation field is meaningful only on rows whose subscribers might fire work.

### 10.e plan-start sections (Blake's 6)

Mapping the bucket vocabulary into Blake's `stores engine plan-start` output:

| Section | Includes | Source buckets |
|---|---|---|
| `WOULD RUN` | Tasks with active=true and a pending work-starting edge | `active_work`, `awaiting_integration` (if active) |
| `INACTIVE / ARMED OFF` | Tasks with active=false but otherwise dispatchable | `inactive_ready`, `awaiting_integration` (if inactive), `deploy_ceremony_pending` (if inactive), `needs_operator_review` |
| `NEEDS OPERATOR` | Tasks awaiting human decision | `needs_operator_review`, `deploy_ceremony_pending` flagged for stranded-recovery |
| `BLOCKED` | Tasks with active gate but blocked/deploy_blocked status | `blocked_recoverable` |
| `HISTORICAL EXHAUST` | Terminal rows that the engine will not touch | `historical_terminal_legacy`, `terminal_success`, `terminal_shipped_oob`, `terminal_retired` |
| `QUEUE HYGIENE` | Counts/highlights from obs+intake surfaces | obs `superseded_or_resolved`/`duplicate_or_folded`/`arch_review_candidate` summaries; intake `draft_triage_backlog` count |

### 10.f Concrete cleanup batch under T140

Once T140's planner is ready and Blake/Pi gate is open, the cleanup work is:

**Batch A — obs supersedes (mechanical):** close 7 obs (L002, L032, L076, L150, L154, L155, L157) via `close_as_addressed` against the named shipping task / merge-target.

**Batch B — silent-zombie fold (mechanical):** close 4 obs (L521, L524, L526, L531) merged into L517.

**Batch C — arch-review promotions:** route 4 obs (L084, L085, L086, L486) via gatekeeper `route --decision arch_review_candidate` to mint A### rows.

**Batch D — intake routing:** route 17 obs-bound intake to observation, 2 as duplicate, 1 as doctrinal-dropped, 1 as arch-review.

**Batch E — observation contract amend + ratify (will mint inactive once primitive ships):** L156 (amend; T063 prerequisite met), L500 (ratify), L539 (ratify, possibly clustered with L481/L482).

**Batch F — task disposition (no mutation, just derived-view sanity test):** legacy 10 rows + T081 + T122 + T125 + T127 + T138 + T139 must hit their expected bucket without any mutation.

Total touched rows in the cleanup: ≈35 obs/intake writes + 0 task mutations. The activation primitive itself + plan-start CLI is the rest of T140's substantive work.
