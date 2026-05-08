# L480 Contract Pre Ratify Review

**Date:** 2026-05-08
**Type:** note

## Summary

**Verdict: RATIFY-AFTER-SHARPENING.** L480's contract has the right shape and the right scope, but six of its eight acceptance criteria are not mechanically checkable as written, and the scope list omits four creep vectors a planner would otherwise pull in. Tier T3 is correct and survives a phase-decomposition test (three natural phases). The cockpit code surface (`Section` enum + `classify_with_options` + `cockpit_header_items`) cleanly admits the changes the contract names, so feasibility is not in doubt — what's at stake is whether the contract pins the planner tightly enough that downstream review can mechanically check the diff. Top three sharpening edits (S-severity): pin the section taxonomy by enumerated label list (not "distinct from any AI-internal review bucket"), pin the dedup primitive's storage surface (extend `row_visibility_class` vs new `Section::CollapsedDupes` vs wrapper-row — planner picks but contract names the constraint), and replace the live-realistic fixture criterion with a numeric spec (cluster counts, statuses, lock counts, `last_status` distribution). Below: nine recommendations ranked by severity, plus the phase shape, the fixture spec, and two side-frictions Blake should file.

## Details

### Sharpening recommendations

**Rec 1 — [S] Pin section taxonomy by enumerated label list (acceptance #1, #2, #4)**

(a) Current text: "No two `Section::label` values render the same string in the cockpit (cf src/tui/data.rs:36-46 collisions today) | RATIFY (U1) section renders observations with intent_contract.contract_state='ready' (and only those); section header is distinct from any AI-internal review bucket | AI-internal plan_review/code_review tasks fold into ACTIVE WORK (or a clearly AI-only section) — they do not share a bucket with U1 or U3 rows".

(b) Why broken: "distinct from any AI-internal review bucket" is reviewer-judgment. "(or a clearly AI-only section)" is a free choice that lets the planner ship a 12-label taxonomy or a 4-label taxonomy and call either compliant. The collision check (#1) is mechanical, but #2 and #4 leave the labels themselves unfixed. Code-reviewer cannot mechanically verify #2 / #4 against a diff.

(c) Replacement (single combined acceptance line, replaces #1 + #2 + #4):
```
Cockpit Section::label() returns one of exactly these strings, with no two variants returning the same string: ACTIVE WORK | RATIFY-U1 | ACCEPT-U3 | HELD-BLOCKED | HELD-DEPLOY | HELD-TRIAGE | HELD-INTAKE | HELD-AI-REVIEW | TERMINAL | PRIORITY | OBSERVATIONS | INTAKE-OPEN | INTAKE-ROUTED | EXTERNAL-REVIEW. Section::ObsRatifiable renders only observations with intent_contract.contract_state='ready'; tasks with status in {plan_review, code_review} render in HELD-AI-REVIEW; tasks with status='in_review' render in ACCEPT-U3.
```

(d) Severity: **S** — without this the entire mission of L480 (distinct labels per semantic bucket) is judgment-graded.

---

**Rec 2 — [S] Pin the dedup primitive's storage surface (acceptance #5, in_scope #3)**

(a) Current text: "When >= 2 observations share an identical summary, cockpit collapses them to one row with a count badge; expanding the row reveals the underlying display_ids" / in_scope: "Per-summary observation deduplication primitive (count badge, display_ids on expand)".

(b) Why broken: "Expanding the row reveals the underlying display_ids" is undefined as a render contract — there is no expand verb in the cockpit today. Three architecturally-different implementations would all pass: (i) extend `row_visibility_class` to a `CollapsedDupe { count, ids }` variant; (ii) add `Section::CollapsedDupes` and rebucket; (iii) introduce a `Row::CollapsedDupes` wrapper variant. (i) and (iii) are the right shape; (ii) breaks the bucket invariant. The contract must pin the constraint or the planner ships (ii).

(c) Replacement:
```
acceptance: When >= 2 observations within the same Section share an identical summary, the cockpit renders one row containing the summary + a count badge of the form '×N' + the lexicographically-first display_id; the remaining display_ids are accessible via the existing detail-view keystroke (Enter on the collapsed row), which lists them. Collapse is per-Section (an obs in OBSERVATIONS and an obs in PRIORITY with identical summary do NOT collapse). Dedup must not introduce a new Section variant; implement as a Row wrapper or a render-time fold inside draw_rows.
in_scope: Per-summary observation dedup primitive in src/tui/data.rs (Row::CollapsedObs wrapper OR fold inside draw_rows; planner picks one and documents the choice in the plan); MUST NOT add a new Section variant.
```

(d) Severity: **S** — leaving this open forces the architectural choice into review-by-vibes.

---

**Rec 3 — [S] Replace the live-realistic fixture criterion with a numeric spec (acceptance #8)**

(a) Current text: "New snapshot fixture covers a live-realistic state (>= 50 dupe obs on one summary, DEAD daemon, dangling locks, both U1 and U3 buckets non-empty) and renders <= 30 visible lines in a 24-line viewport".

(b) Why broken: ">= 50 dupe obs on one summary" can be satisfied with one cluster of 50 — but the live-state pain is **multi-cluster** repetition (76 + 47 + 40 + 35 + 31 + 28 + 24 = 281 rows across 7 clusters in the live DB right now, per `sqlite3` SELECT). A planner who reads the contract literally builds a fixture that doesn't exercise the multi-cluster failure mode. "Dangling locks" — how many? "Both buckets non-empty" — with how many rows? "<= 30 visible lines" — at what `--all` setting? what cockpit width?

(c) Replacement:
```
A new fixture tests/fixtures/watch/live_realistic.snap (without --all, default 80-col width, default 24-line viewport) is generated from a seed DB containing: (a) 7 distinct dupe-summary clusters with row counts {76, 47, 40, 35, 31, 28, 24} all targeting deploy_blocked tasks whose summary starts 'deploy-blocked: task T###'; (b) >= 4 tasks in status='in_review' (the U3 surface); (c) >= 3 observations with intent_contract.contract_state='ready' (the U1 surface); (d) daemon_liveness=DEAD with >= 8 dispatch_locks rows where finished_at IS NULL and last_status='in_flight:pending_next' (epoch-shift cleanup); (e) >= 2 silent_zombie tasks. The rendered fixture is <= 30 visible body lines (excluding the 4-line header), the system-alert row is body-line 1, the 7 dupe clusters render as 7 collapsed rows (not 281), and at least one row each appears in HELD-AI-REVIEW, RATIFY-U1, ACCEPT-U3, HELD-INTAKE.
```

(d) Severity: **S** — this is the only criterion that exercises the cockpit's mission against the noise floor; numbers must be surgical.

---

**Rec 4 — [S] Pin the system-alert row's data path and predicate (acceptance #7)**

(a) Current text: "When daemon liveness=DEAD AND >= 1 dispatch_locks row has finished_at IS NULL, cockpit's first body row is a system-alert (red) naming the lock count and the staleness window".

(b) Why broken: "Staleness window" is undefined. The current `cockpit_header_items` reads from `app.status_bar.daemon_liveness`; nothing in `data.rs` queries `dispatch_locks`. Implicit demand: extend the data-load layer to query `dispatch_locks`. Either pin that surface in scope or strike the criterion.

(c) Replacement:
```
acceptance: When app.status_bar.daemon_liveness=Dead AND the count of dispatch_locks rows with finished_at IS NULL is >= 1, the first body row (immediately after the header block) is a system-alert row rendered in Color::Red BOLD, with text of the exact shape 'system-alert: daemon DEAD; N dangling locks; oldest started Xh ago' where N is the count and X is hours-since-MIN(claimed_at) rounded down. When the predicate is false, no system-alert row is rendered.
in_scope: src/tui/data.rs gains a dispatch_locks read (count + MIN(claimed_at) WHERE finished_at IS NULL) wired into the cockpit_model() return; src/tui/render.rs cockpit_header_items emits the alert row when the predicate holds.
```

(d) Severity: **S**.

---

**Rec 5 — [A] Lock the silent_zombie surface to a named section (acceptance #6)**

(a) Current text: "silent_zombie observations and drive_failed:silent_zombie tasks are no longer routed to HistoricalNoise; they surface in a dedicated section visible in the default cockpit (no --all required)".

(b) Why broken: "Dedicated section" without naming it lets the planner pick any bucket. Combined with Rec 1's enumerated taxonomy this should resolve to a specific label.

(c) Replacement:
```
silent_zombie observations and tasks with blocked_reason starting with 'silent_zombie' or 'drive_failed:silent_zombie' route to Section::TasksHeldZombie with label 'HELD-ZOMBIE' (added to the enumerated taxonomy in Rec 1); they render in the default cockpit (no --all required); their visibility class is no longer HistoricalNoise.
```

(d) Severity: **A** — gets demoted to A only because Rec 1's taxonomy edit incidentally fixes most of the ambiguity if applied first.

---

**Rec 6 — [A] Add a one-sentence done-when**

(a) Current text: `objective` is a paragraph; there is no `done_when`-shaped sentence.

(b) Why broken: Wrap-reviewer needs a single string they can hold against the merged diff. The objective paragraph is fine for orienting a planner but doesn't compress to a yes/no check.

(c) Replacement (add as the first acceptance line):
```
done-when: A fresh `stores watch` invocation against a live-realistic seed DB (Rec 3 fixture) shows distinct labels for every Section, the U1 / U3 / AI-review buckets segregated, the 7 dupe clusters rendered as 7 rows not 281, silent_zombie surfaced by default, and a system-alert row when the daemon-DEAD + dangling-locks predicate holds — all visible in <= 34 lines (header + body) in a 24-line viewport with horizontal scrolling.
```

(d) Severity: **A**.

---

**Rec 7 — [A] Add four scope-creep vectors to out_of_scope**

(a) Current text: out_of_scope has 7 entries, none covering: extending dedup to tasks; restructuring `Section` from enum to struct/trait; touching `External Reviews` lane formatting alongside the U1/U3 split; renaming `--all` semantics now that more classes surface by default.

(b) Why broken: All four are temptations a planner reading "make the cockpit mission-aligned" will pull in. Two of them (dedup-extends-to-tasks, --all rename) look like obvious wins from inside the work and will smuggle in.

(c) Replacement (append to out_of_scope, pipe-separated):
```
Per-summary deduplication of TASKS (only observations dedup in this contract; task-dedup is a separate observation) | Refactoring Section from enum to struct/trait/typeclass — separate observation if needed | External Reviews lane formatting changes (the EXTERNAL-REVIEW label may be renamed per Rec 1 but the lane's row-format and content are out of scope) | Renaming or re-semantic'ing the --all flag — separate observation
```

(d) Severity: **A**.

---

**Rec 8 — [A] Restate "fold AI-internal plan_review/code_review" mechanically**

(a) Current text: "AI-internal plan_review/code_review tasks fold into ACTIVE WORK (or a clearly AI-only section) — they do not share a bucket with U1 or U3 rows".

(b) Why broken: Subsumed into Rec 1 if Rec 1 lands. Listed here only to confirm the mapping: `tasks.status in ('plan_review', 'code_review')` -> `HELD-AI-REVIEW`. If Rec 1 is rejected, this acceptance must be sharpened independently.

(c) Replacement: Strike if Rec 1 lands; otherwise replace with: "Tasks where status in ('plan_review', 'code_review') route to a section whose label does not contain the substring 'REVIEW' — they share no Section variant with ObsRatifiable or any task with status='in_review'."

(d) Severity: **A** (conditional on Rec 1).

---

**Rec 9 — [B] `TasksRecentlyTerminal` rename to convey done-not-action**

(a) Current text: "section currently labelled 'TasksRecentlyTerminal/ACCEPT' is renamed to convey already-done not action-pending".

(b) Why broken: "Convey already-done" is reviewer-judgment. Subsumed into Rec 1 (`TERMINAL` is the proposed label there).

(c) Replacement: Strike if Rec 1 lands; otherwise: "Section::TasksRecentlyTerminal.label() returns 'TERMINAL' (not 'ACCEPT'); Section::TasksRecentlyTerminal does not appear in any code path that emits a U3 prompt."

(d) Severity: **B**.

---

### Tier-fit: T3 confirmed

T3 is right. Three natural phases the planner can split:

- **Phase 1 — taxonomy + dedup primitive (`src/tui/data.rs`):** new label set per Rec 1; new `HELD-AI-REVIEW`, `HELD-ZOMBIE`, `RATIFY-U1`, `ACCEPT-U3`, `TERMINAL` variants; `classify_with_options` rerouting; `Row::CollapsedObs` (or render-time fold) + dispatch_locks query in cockpit_model. No render changes yet; tested via unit tests against the new fixture.
- **Phase 2 — render layer (`src/tui/render.rs`):** system-alert row in `cockpit_header_items`; collapsed-row format in `format_row_line`/`draw_rows`; section ordering update. Tested against `tests/fixtures/watch/*.snap` regressions + the new `live_realistic.snap`.
- **Phase 3 — fixture freeze + golden snapshot:** the new live-realistic seed + snapshot fixture (Rec 3); update existing `default.snap` and `all.snap` to the new label set; verify the dedup count badge and system-alert format.

If the planner cannot decompose into ≥2 phases without overlap, downgrade to T2 and constrain `phases.length == 1`. The above suggests phases are non-degenerate (data → render → fixtures).

### Body grounding vs acceptance — finding-to-criterion map

| Body finding | Acceptance criterion | Status |
|---|---|---|
| F1 (label collisions, S) | #1 | covered (will be Rec 1's enumerated taxonomy) |
| F2 (ObsRatifiable fuses U1+U3+AI-review, S) | #2, #3, #4 | covered (Rec 1) |
| F3 (no per-summary dedup, S) | #5 | covered (Rec 2 sharpens) |
| F4 (silent_zombie hidden, F) | #6 | covered (Rec 5 sharpens) |
| F5 (no system-alert row, A) | #7 | covered (Rec 4 sharpens) |

No body finding is acceptance-orphaned. No acceptance criterion is body-unsupported. Coverage is clean.

### Live-realistic fixture spec (from Rec 3, restated for ratification convenience)

| element | spec |
|---|---|
| dupe clusters | 7 distinct summaries, row counts {76, 47, 40, 35, 31, 28, 24} (matches today's live DB) |
| cluster summary shape | `deploy-blocked: task T### merge conflict on branch 'feat/T###-auto-promoted-l###'` |
| in_review tasks | >= 4 |
| ratifiable obs (`contract_state='ready'`) | >= 3 |
| daemon liveness | Dead |
| dangling dispatch_locks | >= 8 with `finished_at IS NULL` and `last_status='in_flight:pending_next'` |
| silent_zombie tasks | >= 2 |
| viewport | 24 lines, 80 cols, no `--all` |
| budget | <= 30 body lines, system-alert at body-line 1 |
| coverage | one row each in HELD-AI-REVIEW, RATIFY-U1, ACCEPT-U3, HELD-INTAKE |

### Final recommendation

**RATIFY-AFTER-SHARPENING (apply Recs 1, 2, 3, 4 before Blake's `observations update L480 ... --contract-state ready`; Recs 5–9 are nice-to-haves that strictly speaking can be applied after ratification only if the planner doesn't ship before they land — but cleaner to apply all nine in one update).**

## Follow-ups

- File substrate observation: `observations add` raw signal failed with `source_env: required` because the on-disk schema was stale relative to the binary; required `stores migrate --apply` to resolve. Single-line summary: "observations add fails with `source_env: required` against a fresh DB until `stores migrate --apply` runs; binary should self-detect schema-mismatch and exit-with-actionable-message rather than rejecting the field as missing".
- File substrate observation: repeated `--acceptance "text with, comma"` invocations comma-split into N items instead of being stored as one item per `--acceptance` flag; needed `\,` escape and final `--acceptance` count was wrong (13 vs intended 8). Single-line summary: "CLI multi-value flags (`--acceptance`, `--in-scope`, `--out-of-scope`) split on un-escaped commas inside quoted strings; should split on flag-repetition boundaries only, OR document the `\,` escape contract in `--help`".
- Both frictions are out of L480's scope; file as separate observations under `intent_contract.tier_hint=T2` (CLI ergonomics, single-phase work each).
