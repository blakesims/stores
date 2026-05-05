# T015: watch dashboard: phase boxes + cycle dots progress visualization

## Meta
- **Status:** in_review
- **Created:** 2026-05-03T10:43:40Z
- **Last Updated:** 2026-05-03T10:55:30Z
- **Current Phase:** 2
- **Current Cycle:** 1
- **Blocked Reason:** —
- **Branch:** feat/T015-watch-phase-boxes-cycle-dots
- **Capability:** observability

## Task

Replace the text-based status column for cycling tasks in &#x60;stores watch&#x60; (e.g. &#x60;execute P3/4 R2/3&#x60;) with a graphical &quot;Design A&quot; representation: a row of phase boxes encoding position and state, plus a small row of cycle dots encoding revise pressure. Three independent visual channels — position / state / pressure — that read at a glance and drop the verb word entirely. Non-cycling states keep their current bare-name rendering.

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - Modify src/cli/watch.rs::format_task_status to emit the Design A glyph rows for executing / code_review states.
- Add helpers in watch.rs: render_phase_boxes(current_phase, total_phases, in_code_review, color) and render_cycle_dots(current_cycle, max_cycles).
- ANSI color application limited to the current-phase box per the green/yellow/red rules in done-when.
- Adjust the column width constants in render_task_line so the boxes+dots fit cleanly alongside the title.
- Tests in src/cli/watch.rs (#[cfg(test)] mod tests) covering the matrix in done-when.
- Brief README note (optional) documenting the glyph language at the bottom of the watch section, if a watch section already exists; otherwise no doc change.
- **Out:** - No change to TaskRow or query_tasks SQL — current_phase, current_cycle, total_phases are already plumbed through.
- No change to observations panel rendering.
- No change to non-watch surfaces (&#x60;tasks status&#x60;, &#x60;tasks show&#x60;, brief output, etc.) — those keep their text-based phase/cycle reporting.
- No schema reads — MAX_CYCLES_DISPLAY stays hardcoded at 3 (still POC).
- No live animation, no spinners, no progress-bar fills.
- No configurable glyph set, no &#x60;--ascii&#x60; fallback flag — this is a unicode-only render.
- Verb word kept inline — explicitly dropped per the design discussion (the box shape carries the verb).
- No new dependencies; pure stdlib + ANSI escape strings.

### Done When
- For tasks in &#x60;executing&#x60; or &#x60;code_review&#x60;, the watch status column renders as: phase boxes (one box per phase, ordered left-to-right) + a 1-char spacer + cycle dots (one slot per max_revise_cycles, currently 3).
- Box glyphs:
    ▰  completed phase (passed code_review)
    ▮  current phase, state &#x3D; executing
    ◐  current phase, state &#x3D; code_review
    ▱  future phase (not yet started)
- Color is applied only to the CURRENT box (the ▮ or ◐):
    green   — proceeding autonomously (default for executing/code_review)
    yellow  — needs human (used when status is blocked or in_review)
    red     — max cycles exhausted (current_cycle &gt; MAX_CYCLES_DISPLAY)
- Other boxes use default terminal color (no per-box coloring needed).
- Cycle dot row:
    ●  revise burned (one dot per cycle past 1; so cycle 2 &#x3D; &#x60;●··&#x60;, cycle 3 &#x3D; &#x60;●●·&#x60;)
    ·  available revise slot remaining
- Non-cycling task statuses (planning, plan_review→&quot;reviewing plan&quot;, ready, in_review, complete, accepted, blocked, rejected) keep their existing bare-name rendering.
- Status column width fits up to 6 phases comfortably without disrupting the title column. Tasks with &gt; 6 phases truncate to &#x60;▰▰▰…▮▱&#x60; style — first 3 phases, ellipsis, current+next visible.
- Falls back to the existing text format (&#x60;execute P?/? R1/3&#x60;) when current_phase / total_phases / current_cycle are missing or the plan JSON cannot be parsed.
- Tests cover the rendering matrix: (executing|code_review) × (cycle 1, 2, 3, max+1) × (3-phase, 6-phase, 12-phase plan), plus the missing-data fallback and the &gt;6-phase truncation.

### Assumptions
- The user&#x27;s terminal renders standard Unicode block characters (▰▮◐▱) and bullet/middle-dot (●·) without requiring Nerd Fonts. These are widely supported in modern terminals (xterm, konsole, gnome-terminal, alacritty, kitty, etc.).
- ANSI 16-color support — the existing watch implementation already assumes this.
- current_phase, current_cycle, and total_phases are populated correctly by the workflow engine during cycling states (verified empirically during the T005 drive).
- max_revise_cycles in stores/tasks/schema.yaml is 3 (the current value); MAX_CYCLES_DISPLAY in watch.rs stays in sync with it manually for now.

### Phases

#### Phase 1: Phase 1: Helpers + integration into format_task_status
- **Objective:** Introduce render_phase_boxes and render_cycle_dots helpers in src/cli/watch.rs and wire them into format_task_status for executing/code_review, with the full-vs-truncated logic and missing-data fallback in place.
- **Tasks:**
  - Task 1.1: Add a const set of glyphs (DONE&#x3D;&#x27;▰&#x27;, CURRENT_EXEC&#x3D;&#x27;▮&#x27;, CURRENT_REVIEW&#x3D;&#x27;◐&#x27;, FUTURE&#x3D;&#x27;▱&#x27;, DOT_BURNED&#x3D;&#x27;●&#x27;, DOT_AVAILABLE&#x3D;&#x27;·&#x27;) near the existing MAX_CYCLES_DISPLAY block in src/cli/watch.rs.
  - Task 1.2: Add &#x60;fn render_phase_boxes(current_phase: i64, total_phases: i64, in_code_review: bool, color: &amp;&#x27;static str) -&gt; String&#x60; that emits one box per phase (1-indexed current_phase), color-wraps ONLY the current box with ANSI, leaves other boxes uncolored, and truncates plans with total_phases &gt; 6 to the form &#x60;▰▰▰…&lt;current&gt;&lt;next&gt;&#x60; (first 3 boxes + ellipsis + current + one trailing future box if any).
  - Task 1.3: Add &#x60;fn render_cycle_dots(current_cycle: i64, max_cycles: i64) -&gt; String&#x60; returning &#x60;max_cycles&#x60; characters: &#x60;●&#x60; for each cycle past 1 (clamped at max_cycles), &#x60;·&#x60; for remaining slots; e.g. cycle 1 → &#x60;···&#x60;, cycle 2 → &#x60;●··&#x60;, cycle 3 → &#x60;●●·&#x60;, cycle ≥ max+1 → &#x60;●●●&#x60;.
  - Task 1.4: Add &#x60;fn current_box_color(task_status: &amp;str, current_cycle: i64) -&gt; &amp;&#x27;static str&#x60; returning ANSI_RED when current_cycle &gt; MAX_CYCLES_DISPLAY, ANSI_YELLOW when status is &#x60;blocked&#x60; or &#x60;in_review&#x60;, else ANSI_GREEN. (Note: blocked/in_review are non-cycling so this branch is currently dead but kept to honor done-when wording — document with a single-line &#x60;// why&#x60; comment.)
  - Task 1.5: Modify &#x60;format_task_status&#x60; so that for &#x60;executing&#x60; and &#x60;code_review&#x60; with all of (current_phase, total_phases, current_cycle) present, it returns &#x60;{phase_boxes} {cycle_dots}&#x60; (1-char ASCII space spacer); when any of those is None, fall back to the existing &#x60;execute P?/? R?/3&#x60; text format already coded.
  - Task 1.6: In &#x60;render_task_line&#x60;, compute the printable (escape-stripped) width of the status string and pad to a fixed visible-column width that fits 6 boxes + spacer + 3 dots (&#x3D; 10 cols) plus headroom for the longest non-cycling label (&#x60;reviewing plan&#x60; &#x3D; 14). Pick a constant (recommend 16) and add a small &#x60;visible_width&#x60; helper that ignores &#x60;\x1b[...m&#x60; sequences. Replace the &#x60;{:&lt;19}&#x60; format with manual padding using this helper.
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo build&#x60; succeeds with no new warnings.
  - [ ] AC1.2: &#x60;cargo clippy --all-targets -- -D warnings&#x60; passes.
  - [ ] AC1.3: Manual smoke: a TaskRow with status&#x3D;executing, current_phase&#x3D;2, total_phases&#x3D;4, current_cycle&#x3D;1 renders status text containing &#x60;▰▮▱▱&#x60; and &#x60; ···&#x60; (verified by inspecting format_task_status return value in a debug print or via the Phase 2 tests).
  - [ ] AC1.4: A TaskRow missing current_phase still renders the legacy &#x60;execute P?/? R1/3&#x60; (or bare &#x60;executing&#x60;) — fallback path preserved.
  - [ ] AC1.5: &#x60;cargo build --bin stores&#x60; succeeds and &#x60;stores --help&#x60; still works (no runtime regression to the watch entry point).
- **Files:** `src/cli/watch.rs`
#### Phase 2: Phase 2: Unit-test matrix
- **Objective:** Add &#x60;#[cfg(test)] mod tests&#x60; in src/cli/watch.rs covering the rendering matrix from done-when, including truncation and fallback.
- **Tasks:**
  - Task 2.1: Add a small TaskRow constructor helper inside the test module (e.g. &#x60;fn row(status, phase, total, cycle)&#x60;) returning a TaskRow with sensible defaults for the unused fields.
  - Task 2.2: Test &#x60;render_phase_boxes&#x60; directly for: 3-phase plan at phases 1/2/3 (executing and code_review variants), 6-phase plan at phase 4, 12-phase plan at phase 5 (truncated form &#x60;▰▰▰…▮▱&#x60;) and at phase 12 (current at end, no trailing future box).
  - Task 2.3: Test &#x60;render_cycle_dots&#x60; for cycles 1, 2, 3, and 4 (max+1 clamps to &#x60;●●●&#x60;).
  - Task 2.4: Test &#x60;current_box_color&#x60; for the green/yellow/red branches.
  - Task 2.5: Test &#x60;format_task_status&#x60; end-to-end across the matrix (executing|code_review) × (cycle 1, 2, 3, 4) × (3, 6, 12 phases): assert the returned string contains the expected glyph row and dot row separated by exactly one space.
  - Task 2.6: Test the missing-data fallback: TaskRow with current_phase&#x3D;None returns the legacy text format (or bare status name).
  - Task 2.7: Test that non-cycling statuses (planning, plan_review, ready, in_review, complete, accepted, blocked, rejected) return their existing bare names (&#x60;plan_review&#x60; → &#x60;reviewing plan&#x60;, others unchanged).
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;cargo test --lib cli::watch&#x60; passes with all new tests green.
  - [ ] AC2.2: Test count for the new mod is ≥ 12 (covers the matrix dimensions called out in done-when).
  - [ ] AC2.3: At least one test asserts the truncated-form output exactly equals &#x60;▰▰▰…▮▱&#x60; for a 12-phase plan at phase 4 (uncolored portion only — color is applied only to the current box, so the assertion strips ANSI or matches with escapes interleaved).
  - [ ] AC2.4: &#x60;cargo test&#x60; (full suite) passes — no regressions elsewhere.
- **Files:** `src/cli/watch.rs`
- **Dependencies:** Phase 1 must be complete so the helpers exist

---

## Plan Review

### Review 1
- **Gate:** READY
- **Summary:** Plan is executable. Two phases with mechanical ACs (cargo build/clippy/test, exact-glyph string assertions, ≥12-test matrix). Helpers, color rules, truncation, and fallback all map cleanly to done-when. Minor edge (current_phase ≤ 3 in a &gt;6-phase plan) is unspecified but the executor can apply the obvious interpretation without re-planning.
- **At:** 2026-05-03T10:45:13Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** REVISE
- **Summary:** Implemented Phase 1: added glyph constants (▰▮◐▱●·), STATUS_COL_WIDTH&#x3D;16, render_phase_boxes (1-indexed, current-box-only ANSI, &gt;6-phase truncation to &#x27;&lt;3&gt;…&lt;cur&gt;&lt;next?&gt;&#x27;), render_cycle_dots (●·, clamped at max), current_box_color (red&gt;MAX, yellow blocked/in_review, else green — dead branch documented), visible_width (skips \x1b[...m), and rewired format_task_status + render_task_line to use them with manual padding. Legacy &#x27;execute P?/? R?/3&#x27; and bare-status fallbacks preserved when phase data is missing. cargo build + --bin stores succeed with no new warnings; clippy errors present in repo are pre-existing in src/handlers/submit.rs, unrelated to watch.rs. Tests deferred to Phase 2 per plan.
- **Commit:** `fc65f07d8a4c8ae857eb08292a0b41344c09dabc`
- **Files:**
  - `src/cli/watch.rs`
- **At:** 2026-05-03T10:48:09Z
### Phase 1 / Cycle 2
- **Status:** PASS
- **Summary:** Fixed: [MAJOR] render_phase_boxes truncation duplicated the current box when current_phase&lt;&#x3D;3 in &gt;6-phase plans. Added a current_phase&lt;&#x3D;3 branch that renders the first 6 boxes verbatim with no ellipsis; the existing &gt;3 arm continues to emit the …&lt;current&gt;&lt;next?&gt; form. cargo build clean. Three minors deferred per reviewer guidance (pre-existing repo-wide clippy debt out of scope; bare_color/wrap micro-opt non-blocking; AC1.3 verification rolls into Phase 2 tests as instructed).
- **Commit:** `8fffbb5dea3b7d78bd877511db063e3fd273a45a`
- **Files:**
  - `src/cli/watch.rs`
- **At:** 2026-05-03T10:51:09Z
### Phase 2 / Cycle 1
- **Status:** PASS
- **Summary:** Added #[cfg(test)] mod tests in src/cli/watch.rs with 14 tests covering the rendering matrix per done-when: render_phase_boxes for 3/6/12-phase plans (exec + code_review variants), truncated form ▰▰▰…▮▱ at 12-phase phase 5, end-at-12 no-trailing-future, render_cycle_dots for cycles 1..&#x3D;4, current_box_color green/yellow/red, format_task_status full matrix (status × cycle × phase-count), missing-data fallback, and non-cycling statuses (plan_review→&#x27;reviewing plan&#x27;). cargo test --lib cli::watch: 14 passed. Full cargo test has one pre-existing failure (topology_dot_render: graphviz utf8 format unsupported) unrelated to this phase — confirmed via git stash.
- **Commit:** `95068e6`
- **Files:**
  - `src/cli/watch.rs`
- **At:** 2026-05-03T10:53:31Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** REVISE
- **Summary:** Phase 1 wiring is solid and all five ACs verify (cargo build clean, no new warnings, watch.rs has no clippy issues, --bin stores builds, fallback preserved). One MAJOR truncation bug: render_phase_boxes duplicates the current box when current_phase ≤ 3 in a &gt;6-phase plan (produces e.g. &#x27;▰◐▱…◐▱&#x27; for total&#x3D;12, current&#x3D;2). Three minor nits.
- **Findings:** 0 critical, 1 major, 3 minor
**Details:**
[MAJOR] render_phase_boxes truncation duplicates the current box when current_phase ≤ 3 in a &gt;6-phase plan.
File: src/cli/watch.rs:319-328
Evidence: Trace render_phase_boxes(current_phase&#x3D;2, total_phases&#x3D;12, in_code_review&#x3D;true, color&#x3D;GREEN):
  - loop 1..&#x3D;3 emits: box1&#x3D;▰ (done), box2&#x3D;◐ (current, ANSI-wrapped), box3&#x3D;▱ (future)
  - push &#x27;…&#x27;
  - push_box(current_phase&#x3D;2) emits another ◐ ANSI-wrapped
  - current&lt;total, push GLYPH_FUTURE → ▱
  Final: ▰&lt;color&gt;◐&lt;reset&gt;▱…&lt;color&gt;◐&lt;reset&gt;▱ — current box rendered twice.
Expected: done-when contract specifies &#x27;&gt;6 phases truncate to ▰▰▰…▮▱ style — first 3 phases, ellipsis, current+next visible.&#x27; That style only makes visual sense when current_phase &gt; 3. When current_phase ≤ 3, the implementation must not show the current box twice.
Suggestion: Branch on current_phase in the truncation arm. Either (a) when current_phase &lt;&#x3D; 3, render the first 6 boxes verbatim with no ellipsis (simplest), or (b) skip the redundant push_box(current_phase) call inside the prefix range. Concretely:
  } else if current_phase &lt;&#x3D; 3 {
      for i in 1..&#x3D;6 { push_box(&amp;mut out, i); }
  } else {
      for i in 1..&#x3D;3 { push_box(&amp;mut out, i); }
      out.push(&#x27;…&#x27;);
      push_box(&amp;mut out, current_phase);
      if current_phase &lt; total_phases { out.push(GLYPH_FUTURE); }
  }
This bug is verifiable now by reading the code and will resurface as failing tests in Phase 2 if not fixed first; better to fix here so Phase 2 tests pin the correct behavior, not the buggy one.

[MINOR] AC1.2 (cargo clippy --all-targets -- -D warnings) does not strictly pass repo-wide.
File: pre-existing failures in src/handlers/submit.rs (too_many_arguments x4), src/db.rs / others (is_multiple_of, identity-map, suffix-strip, redundant-closure, etc.)
Evidence: cargo clippy --all-targets -- -D warnings → &#x27;could not compile stores due to 27 previous errors&#x27; (lib) and 34 (lib test). None of the warnings reference src/cli/watch.rs.
Expected: AC1.2 says clippy passes.
Suggestion: These are pre-existing and unrelated to this phase; the executor correctly flagged them. Acceptable to record AC1.2 as &#x27;passes for the changed file&#x27; rather than block the phase. Consider a separate observation or task to clean up the pre-existing clippy debt; do not bundle into T015.

[MINOR] format_task_status computes bare_color even when the cycling branch ignores it.
File: src/cli/watch.rs:257-258
Evidence: bare_color and the wrap closure are computed unconditionally, but the (Some, Some, Some) branch returns format!(&quot;{boxes} {dots}&quot;) without invoking wrap.
Suggestion: Move bare_color/wrap inside the arms that use it, or accept the trivial cost. Non-blocking.

[MINOR] AC1.3 &#x27;manual smoke … verified by inspecting format_task_status return value in a debug print or via the Phase 2 tests&#x27; has no in-Phase-1 verification artifact.
Evidence: No debug print, no #[test], no doctest added in Phase 1.
Expected: Some Phase-1-resident demonstration that the path produces ▰▮▱▱ + &#x27; ···&#x27; for the documented input.
Suggestion: Acceptable to defer to Phase 2 tests per the plan&#x27;s stated split, but note that the truncation MAJOR above means deferred verification has already missed a real bug. When the truncation fix lands, add at minimum a unit test in Phase 2 that asserts on (total&#x3D;12, current&#x3D;2) producing the corrected style.

Verification log:
- AC1.1 cargo build → Finished, no warnings. PASS.
- AC1.2 cargo clippy --all-targets -- -D warnings → fails repo-wide; watch.rs clean. PARTIAL (pre-existing, see MINOR above).
- AC1.3 manual smoke of ▰▮▱▱ &#x27; ···&#x27; → not verified in Phase 1; deferred to Phase 2. PARTIAL.
- AC1.4 missing current_phase falls through to wrap(&amp;t.status) bare-status, line 274. Code path verified. PASS.
- AC1.5 cargo build --bin stores → Finished. PASS.

Fix the MAJOR truncation bug, then resubmit; minors can roll into Phase 2.
- **At:** 2026-05-03T10:50:35Z

### Phase 1 / Cycle 2
- **Gate:** PASS
- **Summary:** PASS. MAJOR truncation bug from cycle 1 is fixed correctly: new current_phase&lt;&#x3D;3 arm renders the first 6 boxes verbatim, eliminating the duplicate-current-box artifact. Trace (total&#x3D;12,current&#x3D;2,code_review) → ▰&lt;GREEN&gt;◐&lt;RESET&gt;▱▱▱▱ as required. cargo build clean, no warnings. watch.rs has no clippy issues (pre-existing repo-wide debt out of scope per cycle 1 ruling). AC1.1/1.4/1.5 verified; AC1.3 manual smoke remains deferred to Phase 2 tests per plan. Phase 2 should pin this corrected behavior with a &gt;6-phase / current&lt;&#x3D;3 test case so the fix can&#x27;t regress.
- **Findings:** 0 critical, 0 major, 1 minor
**Details:**
Verification log:
- AC1.1 cargo build → Finished, no warnings. PASS.
- AC1.2 cargo clippy --all-targets -- -D warnings repo-wide still fails on pre-existing src/handlers/submit.rs / src/db.rs items; watch.rs itself emits zero warnings under clippy. PARTIAL→accepted (out of T015 scope, see Phase 1 cycle 1 ruling). Recommend a separate observation/task for the repo-wide clippy debt.
- AC1.3 ▰▮▱▱ + &#x27; ···&#x27; demo → no in-Phase-1 artifact added; rolls into Phase 2 tests. Acceptable per plan.
- AC1.4 missing current_phase falls through to the bare wrap(&amp;t.status) branch around line 274; code path verified unchanged. PASS.
- AC1.5 cargo build --bin stores → Finished. PASS.

Diff review (commit 8fffbb5):
- Adds 5 lines: &#x60;} else if current_phase &lt;&#x3D; 3 { for i in 1..&#x3D;6 { push_box(&amp;mut out, i); } }&#x60; between the &#x60;total_phases &lt;&#x3D; 6&#x60; arm and the &#x60;&gt;3&#x60; truncation arm. Single targeted fix, no scope creep, no collateral changes elsewhere in watch.rs.
- Trace matrix:
  - total&#x3D;12,current&#x3D;2,code_review → arm 2 (&lt;&#x3D;3): ▰&lt;color&gt;◐&lt;reset&gt;▱▱▱▱ ✓ (was ▰&lt;color&gt;◐&lt;reset&gt;▱…&lt;color&gt;◐&lt;reset&gt;▱)
  - total&#x3D;12,current&#x3D;3,executing → arm 2: ▰▰&lt;color&gt;▮&lt;reset&gt;▱▱▱ ✓
  - total&#x3D;12,current&#x3D;4,executing → arm 3 (&gt;3): ▰▰▰…&lt;color&gt;▮&lt;reset&gt;▱ ✓ unchanged
  - total&#x3D;12,current&#x3D;12,executing → arm 3: ▰▰▰…&lt;color&gt;▮&lt;reset&gt; (no future) ✓ unchanged
  - total&#x3D;6,current&#x3D;2 → arm 1: ▰&lt;color&gt;▮&lt;reset&gt;▱▱▱▱ ✓ unchanged

[MINOR / informational] Edge case at total&#x3D;7,current&#x3D;3 hits arm 2 and renders only boxes 1..&#x3D;6, hiding phase 7. The done-when contract specifies &#x27;first 3 phases, ellipsis, current+next visible&#x27; which only triggers on current&gt;3; the implementation as-suggested in cycle 1 review accepts this (rendering 6 verbatim is the simplest correct fix for current&lt;&#x3D;3). Phase 2 tests should include a (total&#x3D;7,current&#x3D;2 or 3) case to pin this behavior intentionally rather than incidentally. Non-blocking; matches the cycle 1 reviewer&#x27;s recommended (a) variant.

Deferred minors from cycle 1 acknowledged by executor and confirmed acceptable to roll forward.
- **At:** 2026-05-03T10:52:09Z

### Phase 2 / Cycle 1
- **Gate:** PASS
- **Summary:** All 4 ACs met. cargo test --lib cli::watch passes (14 tests, ≥12 required). Truncated-form ▰▰▰…▮▱ asserted exactly. Full cargo test has one pre-existing failure (tests/topology_dot_render.rs ac3_1, from T005 commit b10f58f) — graphviz on this host rejects &#x27;utf8&#x27; format; unrelated to T015 and not a regression. Phase 2 complete; this was the final phase, so the task is now done.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
Verification:
- git diff --name-only HEAD~1 HEAD → src/cli/watch.rs (matches executor&#x27;s claim)
- git show 95068e6 --stat → 187 insertions, all in src/cli/watch.rs
- cargo test --lib cli::watch → 14 passed, 0 failed (AC2.1, AC2.2 ✓)
- phase_boxes_12_phase_truncated asserts strip_ansi(render_phase_boxes(5,12,false,GREEN)) &#x3D;&#x3D; &quot;▰▰▰…▮▱&quot; (AC2.3 satisfied semantically)
- cargo test (full) → only failure is tests/topology_dot_render.rs::ac3_1 with &#x60;Format: &quot;utf8&quot; not recognized&#x60; from local graphviz; git log on that file shows it originated in T005 (commit b10f58f), pre-dating T015. AC2.4 ✓ (no regressions introduced).

Quality spot-check (src/cli/watch.rs:467+):
- strip_ansi helper is local, simple, and correct for CSI &#x27;m&#x27; sequences used here.
- Matrix test loops status × cycle 1..&#x3D;4 × 5 phase configurations (40 assertions) — covers done-when matrix dimensions.
- non_cycling_statuses test correctly verifies plan_review→&quot;reviewing plan&quot; mapping.
- Fallback tests cover both None-everywhere → bare status and Some(phase)+None(total) → legacy &#x60;execute P?/? R1/3&#x60; form.
- current_box_color_branches exercises green/yellow/red, including the documented &quot;dead&quot; blocked/in_review branches (defensive coverage).

[MINOR] AC2.3 wording specifies &quot;12-phase plan at phase 4&quot; but test phase_boxes_12_phase_truncated uses phase 5 of 12 (src/cli/watch.rs:514). Both inputs render the identical string ▰▰▰…▮▱ (current&gt;3 takes the truncation branch with first 3 done + ellipsis + current + 1 future), so the assertion&#x27;s substance is correct. Suggestion: change &#x60;render_phase_boxes(5, 12, ...)&#x60; → &#x60;render_phase_boxes(4, 12, ...)&#x60; to literally match the AC text. Non-blocking — output is byte-identical.

[MINOR] format_task_status_glyph_and_dots_separated_by_one_space (src/cli/watch.rs:553) only checks one row (executing, 2/3, cycle 2). The matrix test transitively confirms the single-space contract for 40 other rows via the &#x60;format!(&quot;{want_glyphs} {want_dots}&quot;)&#x60; expectation, so coverage is fine; the standalone test is mostly documentation. No action needed.

[INFORMATIONAL] tests/topology_dot_render.rs::ac3_1 fails on this host because graphviz on Linux rejects format &quot;utf8&quot;. This is pre-existing T005 territory — out of scope for T015. May warrant its own observation if it persists in CI.
- **At:** 2026-05-03T10:54:54Z

---

## Completion
- **In Review:** 2026-05-03T10:55:30Z — awaiting human GO/NO_GO

