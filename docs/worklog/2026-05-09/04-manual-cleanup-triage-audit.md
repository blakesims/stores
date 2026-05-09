# Manual Cleanup Triage Audit

**Date:** 2026-05-09
**Type:** note

## Summary

Read-only triage audit of the paused stores engine to support Blake's manual cleanup. Four parallel subagents inspected (A) task rows, (B) observations + ER, (C) stale infrastructure, (D) intake + engine-health crosscheck. No verbs invoked; no DB writes; no daemon touches. Doc is a command-ready execution queue.

**Counts at audit time (~07:00Z, 2026-05-09):**
- Daemon: stopped. Live drives: 0.
- Active task rows needing disposition: 10 (T108, T116, T123, T124, T126, T128, T134–T137).
- Open observations: 43 (13 ready). Source-obs at re-promote risk: **5** (L034, L513, L514, L515, L520).
- Drive-failed/deploy-blocked obs cluster: 12 (L517, L521, L524–L536).
- Post-accept residue: 3 ready obs that should auto-close but didn't (L087, L485, L489) — I024 evidence.
- Stale `dispatch_locks`: 16 (all NULL/dead pids, all `ignore-before-manual`).
- Orphan worktrees: **13** for abandoned tasks; **9** for blocked tasks.
- Stale-binary process: **1 — PID 1845176 (`stores watch`) on deleted binary.** ⚠ Must kill before next `cargo install`.
- Intake drafts: 28; net actionable after fold/reject: 21.
- ER queue: 0 pending/running/tooling_held; T124 has no ER row yet.

**Execution-queue front loading (speed-first):**
1. Kill `stores watch` PID 1845176 (binary-upgrade safety).
2. Source-obs disposition (L034, L513, L514, L515, L520) — prevents re-promote loop on next daemon start.
3. Post-accept residue cleanup (L087, L485, L489).
4. Per-task abandons for T134–T137 (after source-obs cleared) + T108.
5. Drive-failed obs cleanup (close residue, keep evidence rows).
6. Worktree pruning (13 abandoned).
7. Surface T124 / T126 / T128 / L519 to Blake for judgment.
8. Stale `dispatch_locks` need NO action (watchdog reaps on next tick).

## Details

### 0. Confidence rubric (used throughout)

- **High** — direct CLI + DB + git evidence agrees, action is mechanical.
- **Medium** — evidence agrees but lifecycle implication depends on daemon behavior (subscriber fires, watchdog ticks, etc.).
- **Low** — conflicting/missing evidence or requires product judgment.

Blast radius classes:
- `local-row-only` — affects only the named row.
- `linked-obs-task-pair` — affects the task and its source observation jointly.
- `may-trigger-subscriber-on-daemon-start` — restart could fire a subscriber that mutates downstream rows.
- `could-remint-if-source-obs-left-ready` — auto-promote will create a fresh task on next daemon start.

---

### 1. Execution Queue (start here)

#### Batch A — safe mechanical, no Blake judgment required (run first)

| order | action | command shape | confidence | blast | why safe |
|---|---|---|---|---|---|
| A1 | Kill stale-binary watch | `kill 1845176` | High | local | PID dead-binary `(deleted)` inode; not the daemon; not holding task state. |
| A2 | Close L034 (T124 residue) | `stores observations close_as_addressed L034 --resolution T124 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | could-remint | T124 (in_review) is the live work; T133 was the auto-promote double-fire orphan. Closing L034 prevents the re-mint cascade. |
| A3 | Close L514 (T122 shipped substance) | `stores observations close_as_addressed L514 --resolution T122 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | could-remint | T122 accepted/merged 07766bd carries the paired auto-* fix. |
| A4 | Close L515 (T122 shipped substance) | `stores observations close_as_addressed L515 --resolution T122 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | could-remint | Same shape as L514. |
| A5 | Close L087 (T125 accepted) | `stores observations close_as_addressed L087 --resolution T125 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T125 accepted post-rebase + ER359 PASS. I024 gap is why this didn't auto-close. |
| A6 | Close L485 (T127 accepted) | `stores observations close_as_addressed L485 --resolution T127 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T127 accepted post-rebase + ER360 PASS. Same I024 gap. |
| A7 | Close L489 (T120 shipped + self-validated) | `stores observations close_as_addressed L489 --resolution T120 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T120 schema_migrated; detector self-validated against T121 + T123 stale-binary catches. T128 secondary attempt is residue (handled separately). |
| A8 | Close L525 (T117 cleared) | `stores observations close_as_addressed L525 --resolution T117 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T117 cleared per Pi msg_46309df0 (`bdaf46a`); silent_zombie evidence is captured by I034. |
| A9 | Close L527 (T122 shipped) | `stores observations close_as_addressed L527 --resolution T122 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T122 accepted; silent_zombie evidence captured by I034. |
| A10 | Close L530 (T122 deploy resolved) | `stores observations close_as_addressed L530 --resolution T122 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T122 accepted/merged 07766bd; deploy conflict resolved by rebase ceremony. |
| A11 | Abandon T134 (orphan remint, 0 cycles) | `stores tasks abandon T134 --reason "Auto-promote startup-sweep remint with 0 agent_runs and dead PID 2374273; source obs L513 superseded — see batch B." --invoker ai_with_human --approve-token <T>` | High | local | Zero substance; clears clutter. Source obs L513 disposition handled in Batch B. |
| A12 | Abandon T135 (orphan remint, 0 cycles) | `stores tasks abandon T135 --reason "Auto-promote startup-sweep remint with 0 agent_runs and dead PID 2374275; source obs L514 closed in batch A4." --invoker ai_with_human --approve-token <T>` | High | local | Same shape. |
| A13 | Abandon T136 (orphan remint, 0 cycles) | `stores tasks abandon T136 --reason "Auto-promote startup-sweep remint with 0 agent_runs and dead PID 2374279; source obs L515 closed in batch A4." --invoker ai_with_human --approve-token <T>` | High | local | Same shape. |
| A14 | Abandon T137 (orphan remint, 0 cycles) | `stores tasks abandon T137 --reason "Auto-promote startup-sweep remint with 0 agent_runs and dead PID 2374295; source obs L520 disposition in batch B." --invoker ai_with_human --approve-token <T>` | High | local | Same shape. |
| A15 | Close L533 (T134 silent_zombie residue) | `stores observations close_as_addressed L533 --resolution T134 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T134 abandoned in A11; this drive-failed obs is derivative. |
| A16 | Close L534 (T135 silent_zombie residue) | `stores observations close_as_addressed L534 --resolution T135 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T135 abandoned in A12. |
| A17 | Close L535 (T136 silent_zombie residue) | `stores observations close_as_addressed L535 --resolution T136 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T136 abandoned in A13. |
| A18 | Close L536 (T137 silent_zombie residue) | `stores observations close_as_addressed L536 --resolution T137 --resolution-kind addressed_by_task --invoker ai_autonomous` | High | local | T137 abandoned in A14. |
| A19 | Fold I007 → I003 (NO_COLOR flake dup) | (no verb; document fold via `stores intake route I007 --decision duplicate --duplicate-of I003 --invoker ai_autonomous`) | High | local | Same test, same root cause. |
| A20 | Fold I020 → I011 (foreground FD leak dup) | `stores intake route I020 --decision duplicate --duplicate-of I011 --invoker ai_autonomous` | High | local | Same issue. |
| A21 | Fold I021 → I012 (foreground pidfile dup) | `stores intake route I021 --decision duplicate --duplicate-of I012 --invoker ai_autonomous` | High | local | Same issue. |
| A22 | Fold I034 → I035 (silent_zombie dup) | `stores intake route I034 --decision duplicate --duplicate-of I035 --invoker ai_autonomous` | Med | local | Subjective which is keeper; I035 (engine-controller) more thorough; I034 (queue-curator) was filed first. Picking I035 keeper for completeness. |
| A23 | Reject I008 (already shipped) | `stores intake route I008 --decision reject_noise --invoker ai_autonomous` | High | local | T089/L196 fixed watchdog spam 2026-05-07. |
| A24 | Reject I009 (already shipped) | `stores intake route I009 --decision reject_noise --invoker ai_autonomous` | High | local | T083/L188 made ER lane substrate-native 2026-05-07. |
| A25 | Reject I019 (already shipped) | `stores intake route I019 --decision reject_noise --invoker ai_autonomous` | High | local | T099/L150 cascade-dedup live since 2026-05-08. |
| A26 | Prune 13 abandoned-task worktrees | `git worktree remove /home/blake/repos/experiments/stores-T034-...` × 13 (see §5c) | High | local | Mechanical filesystem cleanup; no substrate side-effects. |

#### Batch B — Blake decision required (medium confidence)

| row | decision needed | options | recommended option | confidence |
|---|---|---|---|---|
| **L513** | Disposition (I026 retest, narrow form) | (a) `wont_fix` superseded by L520; (b) `close_as_addressed` against T120; (c) keep ready awaiting I026 reproducer | (a) `stores observations wont_fix L513 --reason "Superseded by L520 mechanically-tightened retest after Pi narrowing ruling msg_f2783660" --invoker ai_with_human` | Med |
| **L520** | Disposition (mechanically-tightened I026 retest) | (a) `wont_fix` (I026 unmotivated post-c0f45ff + T122); (b) keep ready as future-evidence trigger | (a) `stores observations wont_fix L520 --reason "I026 convergence-stall pattern likely closed by c0f45ff + T122; re-file if recurrence surfaces" --invoker ai_with_human` | Med |
| **T108** | Contract↔plan misalignment (4 NEEDS_WORK cycles) | (a) abandon-and-remint with sharpened L499 contract; (b) revise L499's intent_contract; (c) keep blocked | (a) `stores tasks abandon T108 --reason "Plan-review cycle limit (4 cycles, latest contradicts contract error semantics: plan routes draft→triaging before failure, contract requires draft+fail-loud+no-retry). Remint needs sharpened L499 contract." --invoker ai_with_human --approve-token <T>` | Med |
| **T124** | in_review path (no ER row) | (a) manual codex review + `submit-review`; (b) narrow `stores external_reviews run` after creating ER row; (c) accept on Blake's discretion (wrap noted scope-out deviations); (d) reject + revise | (a) or (b) — wrap deviations are minor (extra plan files; runner-side helpers) but real; suggest codex review or narrow ER. T124 carries the L034 fix; do NOT close L034 (A2) until T124 ships. | Med |
| **T126** | substantive code-reviewer FAIL | (a) revise (fix linked-task-creation bug + remove extra worklog file + re-execute); (b) abandon-and-remint with narrower AC; (c) consolidate into broader Pi-runner work | (a) IF the linked-task-creation bug is small; (b) IF the Pi runner harness needs precondition work first | Low |
| **T128** | executor commit valid (e74f7c9), drive reaped | (a) abandon (commit is meaningful; merge separately if useful); (b) accept (treats commit as terminal); (c) resume after I033 fix | (a) — drive death is environmental; substance shipped; abandon-then-evaluate-commit is cleanest. Then close L489 in A7 covers source obs (T120 already shipped the watchdog work). | Med |
| **L519** | I033 obs disposition (resume-guard) | (a) close_as_addressed against the manual-rescue commit IF Blake landed the resume-guard fix in main; (b) keep open as Blake-escalation candidate per msg_b58ed8da | Need Blake to confirm: did manual rescue patch the resume-guard, or only patched per-row? | Low |

#### Batch C — do not touch yet

| row | hold reason | unblock condition |
|---|---|---|
| T116 | T1 silent_zombie; no I033 risk; needs WIP≤2 + I034 root-cause better understood | Resume cleanly after WIP drops; if recurs post-resume, escalate to abandon-and-remint |
| T123 | T3 with 4 NEEDS_WORK cycles + non-empty rejected plan = exact I033 contamination shape | Blake's manual-main I033 resume-guard fix lands; then `stores tasks resume T123` routes to planning cycle 5 |
| L517 | T118 abandoned; silent_zombie evidence | Keep as I034 evidence; close after I034 fix ships |
| L521 | T119 abandoned; silent_zombie evidence | Same |
| L524 | T116 still blocked-held | Close after T116 ships or is abandoned |
| L526 | T121 abandoned; silent_zombie evidence | Keep as I034 evidence |
| L531 | T123 still blocked-held | Close after T123 ships or is abandoned |
| L532 | T128 silent_zombie residue | Close after T128 disposition (Batch B) |
| 16 dispatch_locks | All NULL/dead pids; watchdog reaps on next daemon tick | None — restart-safe as-is; do **not** raw-SQL DELETE |
| 63 stale `drive_pid` rows on terminal/blocked tasks | Watchdog clears on next tick | Same — restart-safe |
| Intake items I023, I026 (others see §6) | Need routing later but not blocking | Route during normal queue-curation post-cleanup |

---

### 2. Daemon-restart risk model

**If you run `stores agents run` right now, here is what would fire:**

- **Auto-promote candidates:** **0** today (all `ready` source-obs already have child tasks; no orphaned ready obs without `task_id`). However: if Batch A2/A3/A4/A5/A6/A7/A8/A9/A10 are NOT executed and the source obs are left in `ready` with abandoned/dead child tasks, the auto-promote subscriber's startup-sweep WILL re-mint replacements (this is the 06:27Z cascade Blake just escaped). **Run Batch A first; THEN restart is safe.**
- **Auto-resolve candidates:** 3 today (L087, L485, L489 — but auto-resolve subscriber only covers `cargo_installed → schema_migrated` edge; does NOT cover the `accepted` edge per engine-health.md L049 + I024). Result: nothing fires; rows remain stuck. Batch A5/A6/A7 closes them manually.
- **Auto-drive candidates:** All blocked rows are in `blocked` status; auto-drive does not fire on `blocked`. Planning rows T134–T137 would be re-driven on next daemon tick — but they have dead drive_pids and 0 agent_runs; the dispatcher would attempt re-dispatch. **Abandon T134–T137 (Batch A11–A14) first.** After abandons, no auto-drive surprises.
- **Watchdog candidates:** 63 stale `drive_pid` rows on terminal/blocked tasks. Watchdog ticks would clear them via `mark_drive_failed` (already-blocked rows are no-op). 16 stale `dispatch_locks`: watchdog reaps on first tick. Both are restart-safe and self-healing.
- **External-review candidates:** 0 pending/running/tooling_held rows. T124 has no ER row; daemon would NOT auto-spawn one (ER spawn is event-driven on transition into in_review, which already fired and was no-op'd; manual ER dispatch needed).
- **Subscriber-edge candidates:**
  - I016 deploy-blocked merge-conflict subscriber would fire per-tick on any unresolved deploy_blocked row. Currently zero such rows after T122 acceptance. Restart-safe.
  - L016 cascade subscriber would auto-file `drive-failed` obs if any drive dies post-restart. Restart-safe behavior.

**Summary: After Batch A executes, daemon restart is safe-ish.** "Safe-ish" = no surprise re-mints, no stale-lock interference, no auto-resolve misfires. Outstanding restart risks: T128/T134–T137 dispositions before restart (Batch B/A); stale-binary process kill (A1); I033 fix landed before T123 resume.

---

### 3. Task rows

| ID | tier | status | recommended | blast radius | confidence |
|---|---|---|---|---|---|
| T108 | T2 | blocked | abandon-and-remint (Batch B) | local-row-only | Med |
| T116 | T1 | blocked | hold (Batch C) | local-row-only | High |
| T123 | T3 | blocked | hold pending I033 (Batch C) | local-row-only | High |
| T124 | T1 | in_review | surface to Blake (Batch B) | linked-obs-task-pair | Med |
| T126 | T2 | blocked | surface to Blake (Batch B) | local-row-only | Low |
| T128 | T2 | blocked | abandon (Batch B; commit e74f7c9 valid) | may-trigger-subscriber | Med |
| T134 | T2 | blocked | abandon (Batch A11) | could-remint | High |
| T135 | T2 | blocked | abandon (Batch A12) | could-remint | High |
| T136 | T2 | blocked | abandon (Batch A13) | could-remint | High |
| T137 | T2 | blocked | abandon (Batch A14) | could-remint | High |

**Detail blocks (Med/Low confidence only):**

- **T108 — Med.** L499 contract requires error rows stay in draft with fail-loud log + no-same-tick-retry; latest plan moves them draft→triaging before failure. After 4 NEEDS_WORK cycles the planner can't satisfy the contract. Abandon-and-remint needs sharper L499 contract OR a planner-level hint. Branch has 1 commit (98da6b5; doc-only convergence-stall SOP), no executor work. Source obs L499 stays ready; whoever remints should sharpen first. Evidence: `sqlite3 .stores/db.sqlite 'SELECT json_array_length(plan_review_log) FROM tasks WHERE display_id="T108";'` → 4.
- **T124 — Med.** Executor + code_reviewer + wrap all completed (3 agent_runs, exit 0). Wrap_log notes 2 deviations: (i) scope-out runner-side helpers landed in src/handlers/drive.rs despite contract scoping that out, (ii) extra docs/plans/2026-05-09/* files in branch diff. No external_reviews row exists. Path forward: either codex review locally or use the new narrow `stores external_reviews run <ERID>` verb (per worklog 03 commit 06403b5) after creating an ER row. Do NOT close L034 (A2) until T124 ships — A2 banks on T124 acceptance. If T124 gets rejected, re-open L034 or re-ratify.
- **T126 — Low.** Code-reviewer FAIL on AC1.2–AC1.6 (Pi runner E2E run aborted before creating linked task) + AC1.8 (extra worklog file in diff). The AC1.8 cleanup is mechanical; AC1.2–AC1.6 requires understanding why the linked-task creation aborted. May indicate Pi runner harness needs precondition work, or just a small bug in the test scaffold. Blake's call: revise vs abandon vs consolidate.
- **T128 — Med.** Executor + code_reviewer ran (exit 0); committed `e74f7c9 T128 P1: contract verified via T118/T120; fix T123 agents.yaml regression`. Then drive reaped → blocked silent_zombie. Substance is real; the death is environmental. Abandon allows the commit to be merged separately or treated as wisdom-incorporated; resume risks re-running and producing duplicate work. L489 source-obs is T120-anchored (Slot E watchdog already shipped + self-validated), so closing L489 in A7 doesn't depend on T128's fate.

---

### 4. Observations

#### 4a. Source-obs remint-risk table (HIGHEST LEVERAGE)

| obs | current task_id | linked tasks (history) | desired future? | recommended disposition | remint risk if left ready | confidence |
|---|---|---|---|---|---|---|
| **L034** | T133 (abandoned) | T124 (in_review, real diff f45c2ee), T133 (orphan twin, 0 cycles) | T124 ships the fix; L034 closes against T124 | Batch A2: `close_as_addressed → T124` | **YES — auto-promote double-fire pattern** | High |
| **L513** | T134 (blocked) | T114 (abandoned, I026 retest), T129 (abandoned), T134 (orphan, 0 cycles) | Superseded by L520 mechanically-tightened retest | Batch B: `wont_fix` (superseded by L520) | **YES — would re-mint** | Med |
| **L514** | T135 (blocked) | T115/T130/T131 (abandoned), T135 (orphan, 0 cycles) | T122 (accepted) shipped the substance | Batch A3: `close_as_addressed → T122` | **YES — would re-mint** | High |
| **L515** | T136 (blocked) | T119/T131 (abandoned), T136 (orphan, 0 cycles) | T122 (accepted) shipped the substance | Batch A4: `close_as_addressed → T122` | **YES — would re-mint** | High |
| **L520** | T137 (blocked) | T121 (abandoned), T132 (abandoned, deferred), T137 (orphan, 0 cycles) | I026 retest unmotivated post-c0f45ff + T122 | Batch B: `wont_fix` (with re-file-if-recurs note) | **YES — would re-mint** | Med |

Cleanup ordering: **close source obs FIRST (Batch A2/A3/A4 + Batch B for L513/L520), THEN abandon T134/T135/T136/T137 (Batch A11–A14), THEN clear residue obs L533–L536 (Batch A15–A18).** Reverse order would leave a window where source obs is ready + child task is gone, and a daemon tick would re-mint.

#### 4b. Drive-failed/deploy-blocked cluster

| obs | parent | parent status | disposition | command shape | confidence |
|---|---|---|---|---|---|
| L517 | T118 | abandoned (I033 victim) | keep-as-evidence (I034) | none | Med |
| L521 | T119 | abandoned (I033 victim) | keep-as-evidence (I034) | none | Med |
| L524 | T116 | blocked-held | keep-as-evidence; close after T116 ships | none now | High |
| L525 | T117 | accepted (cleared per Pi msg_46309df0) | A8: close_as_addressed → T117 | (see Batch A8) | High |
| L526 | T121 | abandoned (manual rescue) | keep-as-evidence (I034) | none | High |
| L527 | T122 | accepted | A9: close_as_addressed → T122 | (see Batch A9) | High |
| L530 | T122 | accepted (deploy resolved by rebase) | A10: close_as_addressed → T122 | (see Batch A10) | High |
| L531 | T123 | blocked-held | keep-as-evidence; close after T123 ships | none now | High |
| L532 | T128 | blocked | hold pending T128 disposition (Batch B) | none now | Med |
| L533–L536 | T134–T137 | will-be-abandoned | A15–A18: close after parent abandon | (see Batch A) | High |

#### 4c. Post-accept residue (I024 evidence)

L087 / L485 / L489 are ready obs whose parent tasks shipped, but auto-resolve subscriber's edge coverage is `cargo_installed → schema_migrated` only. **These three are the live evidence behind I024.** Manual closure in Batch A5–A7 unblocks them. After cleanup, recommend filing a follow-up to widen the auto-resolve subscriber to cover the `accepted` edge.

#### 4d. Pending disposition

- **L519 (I033 obs)** — Surface to Blake. If manual-main rescue landed the resume-guard fix, close_as_addressed against the commit (Med-High confidence pending Blake confirmation). If only per-row patched, leave open as Blake-escalation candidate per Pi msg_b58ed8da.
- **I034 vs I035 dedup** — Both intake-draft. I035 keeper recommended (engine-controller's body more thorough); fold via Batch A22. Med confidence (subjective; either keeper works).

---

### 5. Stale infrastructure

#### 5a. Dispatch locks (16 stale)

ALL classified `ignore-before-manual`. Every lock has either NULL or dead holder_pid; daemon's watchdog reaps on first tick after restart. **NO MANUAL ACTION.** Per CLAUDE.md doctrine, raw-SQL DELETE is forbidden — and here it isn't even necessary.

#### 5b. Stale-binary process — CRITICAL ⚠

**PID 1845176 — `stores watch`** running on deleted binary (`/proc/1845176/exe → .../stores (deleted)`). Etime ~1h45m. Blocks future `cargo install` from cleanly replacing the binary inode (next install would orphan another inode the same way; T120's detector would fire on the next drive cycle).

**Action: `kill 1845176`** (Batch A1). Safe — `watch` is read-only monitoring, not the daemon.

#### 5c. Orphan worktrees — 13 for abandoned tasks

Recommended `git worktree remove` (mechanical, no substrate side-effect):
```
/home/blake/repos/experiments/stores-T034-auto-promoted-l110
/home/blake/repos/experiments/stores-T042-auto-promoted-l087
/home/blake/repos/experiments/stores-T106-auto-promoted-l485
/home/blake/repos/experiments/stores-T114-auto-promoted-l513
/home/blake/repos/experiments/stores-T115-auto-promoted-l514
/home/blake/repos/experiments/stores-T118-auto-promoted-l489
/home/blake/repos/experiments/stores-T119-auto-promoted-l515
/home/blake/repos/experiments/stores-T121-auto-promoted-l520
/home/blake/repos/experiments/stores-T129-auto-promoted-l513
/home/blake/repos/experiments/stores-T130-auto-promoted-l514
/home/blake/repos/experiments/stores-T131-auto-promoted-l515
/home/blake/repos/experiments/stores-T132-auto-promoted-l520
/home/blake/repos/experiments/stores-T133-auto-promoted-l034
```
Plus 9 worktrees for currently-blocked tasks (T108/T116/T123/T126/T128/T134–T137) — leave alone until those tasks are dispositioned.

#### 5d. Drive_pid leftovers

63 rows with non-NULL `drive_pid` on terminal/blocked tasks. Sample of 12 verified all-dead. Watchdog clears on next daemon tick. Restart-safe; no action.

---

### 6. Intake drafts triage (28 items)

**21 routable as observation** (file via `stores intake route I### --decision normal_observation ...` when ready):
- High confidence: I003, I004, I010, I011, I012, I016, I018, I024, I025, I030, I031
- Medium confidence: I005, I006, I013, I014, I015, I017, I023, I026, I034 (or I035 if A22 inverts), I033(L519)

**4 fold dupes** (Batch A19–A22): I007→I003, I020→I011, I021→I012, I034→I035 (or invert; subjective).

**3 reject-noise** (Batch A23–A25): I008 (T089/L196 shipped), I009 (T083/L188 shipped), I019 (T099/L150 shipped).

**Doctrinal-only / leave as draft:** I002 (already documented in engine-health.md line 191).

**Highest-priority routes after cleanup:**
1. **I030 (auth token plaintext missing)** — blocks Tier-A writes; needs Blake decision + likely manual fix (Pi msg_b58ed8da escalate-to-Blake territory).
2. **I024 (auto-resolve accepted-edge gap)** — live evidence is L087/L485/L489 in this audit.
3. **I031 (list-field stepwise-update REPLACE)** — affects manual queue-curation workflows.

---

### 7. Pre-restart checklist

Before next `stores agents run`:

1. ✅ Daemon already stopped (verified).
2. ⚠ **Kill PID 1845176** (Batch A1) — binary-upgrade safety.
3. ✅ Execute Batch A2–A18 in order — closes source obs before child-task abandon to prevent re-mint window.
4. ✅ Make Batch B decisions on T108 / T124 / T126 / T128 / L513 / L520 / L519.
5. ✅ Confirm I033 resume-guard status (does main carry the fix?) before any T123 resume.
6. ⏳ Optional: prune 13 abandoned worktrees (Batch A26).
7. ✅ Stale `dispatch_locks` and `drive_pid` leftovers: no action; watchdog handles.

After all the above, daemon restart is safe. Recommend constrained mode initially per worklog 03 Phase 2 (no broad `agents run --once`).

---

### 8. Engine-health.md sections to update post-cleanup

(Don't update yet — identified for the post-cleanup engine-health refresh per CLAUDE.md sweep doctrine.)

- **Layer 8, L049 row** — note the accepted-edge gap (links to I024).
- **Layer 8, I024 row** — validated by live evidence (L087/L485/L489).
- **Priority ladder, item 5** — re-rank I024 + auto-resolve coverage.
- **Layer 8, L137 row** — clarify backfill only covers `schema_migrated`, not `accepted`.
- **Recently shipped: L489/T120** — Slot E watchdog stale-binary detector closure narrative (caught T121 + T123 in the wild).
- **Recently shipped: T117 / T122 / T125 / T127** — manual-rescue acceptance ceremony.

---

### 9. Open questions for Blake (not queue-curator's call)

1. **L519 / I033 status** — did the manual-main rescue patch the resume-guard, or only patched per-row? Determines L519 disposition (close vs Blake-escalation candidate).
2. **T124 review path** — codex local review, narrow ER via new verb (06403b5), or direct accept/reject? Wrap deviations are minor but real.
3. **T126 disposition** — revise (fix linked-task creation + re-execute), abandon-and-remint with narrower AC, or consolidate into broader Pi-runner work?
4. **L513 / L520 dispositions** — `wont_fix` (recommended) or keep-ready as future-evidence trigger for I026 reproducer?
5. **I034 vs I035 keeper choice** — subjective; either works.
6. **I030 (auth token)** — Blake-escalation per Pi msg_b58ed8da; this audit doesn't recommend a fix shape since the plaintext file restoration is host-level.
7. **Worktree pruning** — bulk remove the 13 abandoned-task worktrees now, or stage that for a separate cleanup pass?

## Follow-ups

- After Blake executes Batch A: re-run `stores tasks status` + the source-obs query to confirm 0 candidates would auto-promote on restart.
- After Batch B decisions: file fresh observations / remint tasks as needed; record dispositions back into this note.
- After daemon restart: monitor for any auto-resolve / auto-promote misfires; if any fire, file as I024/I025 evidence.
- Engine-health.md refresh once cleanup stable (per § 8).
- Auth token (I030) restoration is a Blake-only action; queue-curator can't help further until plaintext token is on-host.

