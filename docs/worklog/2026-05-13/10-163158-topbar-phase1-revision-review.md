## Code Review

Gate: REVISE

### Summary
The revision is scoped to `src/tui/render.rs` and the targeted tests pass, but the responsive threshold is still wrong for the actual five-card strip. At a common 120-column cockpit, `draw_store_strip` chooses five equal cards because `120 / 5 == MIN_STORE_CARD_WIDTH` (24), yet those cards do not have enough per-cell width for full labels like `working`, `investigate`, or `contract gate`; `wrap_label` will replace them with `+ more`. That misses the Phase 1 goal of full-word readable labels at normal/wide sizes and the prior REVISE request to make the fallback protect the actual five-card strip.

### Git Reality
- Files changed in commit `e44410d`: `src/tui/render.rs` only.
- Working tree at review time is dirty with many unrelated modified files and untracked `tasks/...` test-task directories; they are not part of commit `e44410d`.
- Commands inspected:
  - `git status --short`
  - `git show --stat --oneline --decorate --name-only e44410d`
  - `git show --color=never -- src/tui/render.rs`
  - `cargo test -q tui::render::tests::`
  - `cargo test -q tui::input::tests::nav`
  - `cargo test -q tui::app::tests::cycle_focus`
  - `git diff --check -- src/tui/render.rs`

### Acceptance Criteria Verification
- [x] Top bar still renders five separate store cards at sufficiently wide widths: existing 120-label test checks card titles; full-label render test uses 260-column direct card drawing.
- [x] Six semantic slots remain in stable order: `lane_card_slots` still returns six slots per lane in canonical front/work/gate/exit/wait/fault order.
- [ ] Labels are full words at normal/wide cockpit size: at 120 columns the code renders five cards, but per-card cell widths are too small for common labels and `wrap_label` emits `+ more` instead of labels.
- [x] No glued cockpit-code strings in the targeted paths tested; rendered-buffer tests pass and `wrap_label` no longer truncates words.
- [x] Glyph/count are separated (`FlowSlot::meta()` emits `glyph count`, labels are rendered on separate lines).
- [x] Only outer card border plus internal dividers are used; no nested cell boxes added.
- [x] `TOP_STRIP_HEIGHT` remains consistently used by layout and tests.
- [x] Existing targeted semantic row/detail tests passed.
- [x] Rendered-buffer tests assert visible output.
- [ ] Narrow/common-width coverage is incomplete: 80-column focused fallback is tested, but the common 120-column case is not tested for full labels/no slot-level `+ more`.
- [x] No unrelated files were committed in `e44410d`.

### Findings
- Major: `src/tui/render.rs:28` / `src/tui/render.rs:244` — `MIN_STORE_CARD_WIDTH = 24` is not a readability threshold for the five-card grid. With width 120, `draw_store_strip` takes the five-card branch. Each card is 24 columns; inner width is 22; `split_three_widths(22)` gives `[8, 7, 7]`, then columns 1/2 lose one column to divider padding, so the effective label widths are 8, 6, and 6. Labels such as `working` (7), `investigate` (11), `contract` (8 in a 6-wide gate cell), and `tool fault` cannot render as full words and become `+ more` via `wrap_label`. Required fix: make the branch threshold reflect the actual minimum card width needed to render the six labels without slot-level `+ more`, or switch to the focused-card/`+4 more` fallback until five cards can show full labels. Add a rendered-buffer test at a common width (120 is the obvious regression width) proving the fallback/readability behavior.

### Revise Feedback
1. Replace the hard-coded 24-column wide-mode threshold with a computed or documented threshold based on real cell widths for the longest labels in the five-card layout. Do not enter five-card mode at 120 unless full labels are actually visible.
2. Add a rendered-buffer regression for 120 columns (or another documented common terminal width) asserting either focused-card `+4 more` fallback with full focused labels, or five cards with full labels and no slot-level `+ more` replacing semantic labels.
3. Keep the 80-column fallback test, and keep the commit scoped to `src/tui/render.rs` unless the fix genuinely needs a small helper elsewhere.

### Reviewer Notes
The revision did fix the prior word-fragment clipping mechanism and added an 80-column fallback test. The remaining problem is threshold correctness: tests pass because the full-label test uses a 260-column synthetic card strip, not the common-width path where the bug still appears by inspection of the actual layout math.
