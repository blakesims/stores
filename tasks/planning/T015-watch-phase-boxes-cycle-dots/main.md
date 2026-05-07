# T015: watch dashboard: phase boxes + cycle dots progress visualization

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T10:43:40Z
- **Last Updated:** 2026-05-03T10:45:01Z
- **Current Phase:** 
- **Current Cycle:** 
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

_No plan reviews yet._


---

## Execution Log

_No execution cycles yet._

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

