## Review

Gate: PASS

- Correct: `App::flat_rows()` now sorts navigable task/observation rows through `sort_flat_rows_for_projection_display` after filtering/collapse/lane checks, so cursor navigation uses the same slot sequence that the projection renderer groups by (`Front`, `Work`, `Gate`, `Wait`, `Fault`, `Exit`) (`src/tui/app.rs:364`, `src/tui/app.rs:368-385`, `src/tui/app.rs:551-559`). This directly addresses the reported mismatch where Down followed canonical section order while the display was grouped by projection slot.
- Correct: The sort preserves within-slot order using `(fr.section, fr.row)` as tie-breakers (`src/tui/app.rs:375`, `src/tui/app.rs:383`), so the fix is targeted and does not introduce arbitrary row reordering inside a displayed projection group.
- Correct: The renderer's projection display order is the same slot order (`src/tui/render.rs:923-931`, `src/tui/render.rs:998-1006`), so the app-level navigation order now matches the rendered grouping.
- Correct: Regression tests cover both affected lanes. The task test verifies flat-row order and actual repeated `move_selection(1)` behavior across queued/work/gate order (`src/tui/app.rs:856-890`). The observation test verifies candidate/gate/wait projection ordering (`src/tui/app.rs:893-909`).
- Correct: Targeted tests passed: `cargo test projection_display_order` ran both new tests in `src/lib.rs` and `src/main.rs`; all 4 instances passed, with the rest filtered out.
- Note: The working tree contains many unrelated modified/untracked files (`git status --short` shows changes outside `src/tui/app.rs`, plus untracked `.tmp/`, task dirs, and `topbar-phase1-revision-review.md`). For this focused fix, only `src/tui/app.rs` should be staged/committed.
- Fixed: None applied.
- Blocker: None.
