PASS

## Review
- Correct: `src/tui/render.rs:262-269` now gates all-card mode on the sum of lane-specific minimum widths and lays cards out with per-lane `Constraint::Length` widths rather than a single worst-case card width.
- Correct: `src/tui/render.rs:293-313` derives each lane minimum from that lane's six slots, preserving full label words/count meta; `src/tui/render.rs:680-687` also uses lane-specific column widths inside each card.
- Correct: 140-column/full-width coverage is updated: `src/tui/render.rs:2371-2404` paints at width 140, asserts all five lane labels and readable slot words, and asserts no `+ more` fallback.
- Correct: 120/80 fallback coverage remains in `src/tui/render.rs:2407-2463`, asserting `+4 more`/`+ more` and readable focused-card labels without compact fragments.
- Correct: `tests/tui_watch_cockpit.rs:322-340` updates the integration expectation from compact tokens like `◌new`/`◆tri` to readable full-word labels and separated counts, and explicitly rejects the old compact codes.
- Correct: Targeted test passed: `cargo test cockpit_top_strip -- --nocapture` (lib/main unit tests and `tests/tui_watch_cockpit.rs` filtered test all passed).
- Correct: No staged files are present (`git diff --cached --name-only` produced no output). Note: the worktree has many unrelated unstaged modifications, but they are not staged.
