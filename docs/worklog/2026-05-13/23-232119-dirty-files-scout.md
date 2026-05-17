# Code Context

## Files Retrieved
1. `src/tui/footer.rs` (diff hunk around `format_text`, current lines ~71-75) - requested TUI file; shows single-line rustfmt collapse only.
2. `src/tui/input.rs` (diff hunk around tests, current lines ~396-402) - requested TUI file; shows rustfmt multiline `assert_eq!` only.
3. `src/tui/sidecar/obs_draft.rs` (diff hunk around current lines ~63-66) - requested sidecar file; shows rustfmt method-chain wrapping only.
4. `src/tui/sidecar/session.rs` (diff hunk around current lines ~90-95) - requested sidecar file; shows rustfmt method-chain wrapping only.
5. `src/tui/sidecar/system_prompt.rs` (diff hunk around current lines ~16-40) - requested sidecar file; shows rustfmt arm/assert wrapping only.
6. `tests/external_review_daemon.rs` (diff hunks around current lines ~216-590) - broad test sample; shows rustfmt query/assert wrapping only.
7. `tests/resource_locks_e2e.rs` (diff hunks around current lines ~3-120) - broad test sample; shows rustfmt expanding compressed one-line helpers/argument arrays.
8. `tests/gatekeeper_decision_validator.rs` (diff hunks around current lines ~51-390) - broad test sample; shows rustfmt line wrapping only.
9. `src/cli/metrics.rs` (diff hunks around current lines ~120-1505) - large source sample; shows rustfmt wrapping, including test calls/assertions.
10. `src/handlers/transition.rs` (diff hunks around current lines ~93-2525) - large source sample; shows rustfmt wrapping long calls/tests.
11. `stores/intake_items/schema.yaml` (diff hunks around lines 7-200) - YAML block-style reformat of existing inline transition/enum lists.
12. `tasks/active/T001-test-task/main.md` and `tasks/planning/T001-test-task/main.md` (full diffs) - tracked generated test task projections with substantive timestamp/content changes.
13. Untracked: `.tmp/pi-trash/*`, `tasks/active/T143-test-task/main.md`, `tasks/active/T802-test-task/main.md`, `tasks/active/T803-test-task/main.md`, `tasks/planning/T801-test-task/main.md`, `topbar-phase1-revision-review.md`, `watch-row-ux-principles.md` - temp/review/test-task artifacts.

## Key Code / Evidence

- `git status --short` shows 89 tracked modified files plus untracked temp/task/review notes.
- `git diff --stat` shows 3382 insertions / 1274 deletions; `git diff -w --stat` remains large because rustfmt changes line structure/trailing commas, not just spaces.
- `cargo fmt --check` produced no output, so current Rust files are rustfmt-compliant.
- All modified Rust files have clustered mtimes from `2026-05-13 14:14:01` to `14:14:02 +0700`, consistent with one formatter sweep.
- Requested TUI/sidecar files are formatting-only by inspection:
  - `src/tui/footer.rs`: `Row::Intake` tuple collapsed from multi-line to one line.
  - `src/tui/input.rs`: `assert_eq!` expanded to multi-line.
  - `src/tui/sidecar/*`: method chains/match arms/asserts wrapped by rustfmt.
- Broad tests sampled are formatting-only by inspection, especially one-line compact helpers/arrays expanded in `tests/resource_locks_e2e.rs` and `query_row`/assert wrapping in external review and gatekeeper tests.
- `stores/intake_items/schema.yaml` is not rustfmt; it appears YAML-formatter-only: inline transition maps and enum arrays were expanded, with no apparent value changes in sampled hunks.
- Tracked task markdown is not formatting-only:
  - `tasks/active/T001-test-task/main.md` changed task content from the CLI-agents plan/review/execution material to generic `Test` / `Do something`, reset review/execution sections, and updated timestamps/wraps.
  - `tasks/planning/T001-test-task/main.md` only timestamp changed.
- Untracked task dirs (`T143`, `T801`, `T802`, `T803`) are generated synthetic/test-task projections dated `2026-05-13T10:27:07Z`; likely test or dogfood fixtures accidentally emitted into repo task projections.
- `.tmp/pi-trash/*` contains timestamped review/plan-review markdown from today; likely local agent/review scratch moved to trash.
- `topbar-phase1-revision-review.md` is an untracked code-review note; it explicitly says the working tree was already dirty with many unrelated modified files and untracked test-task dirs.
- `watch-row-ux-principles.md` is an untracked design note for watch row UX.

## Architecture / Likely Origin

- Main dirty Rust set is almost certainly caused by a broad `cargo fmt` run over pre-existing unformatted code/tests, not by feature edits. Evidence: identical mtime cluster across all modified `.rs` files, `cargo fmt --check` clean, and sampled hunks are rustfmt-shaped only.
- `stores/intake_items/schema.yaml` likely came from a YAML formatter (or schema render/normalization) on 2026-05-12; it is separate from the Rust fmt sweep.
- Task markdown changes/untracked task dirs likely came from running substrate tests or task-render/drive flows against the real repo paths instead of an isolated temp repo, producing synthetic `T###-test-task` projections.
- Review/design markdown and `.tmp/pi-trash` are human/agent scratch artifacts from current watch/topbar review work, not source changes.

## Recommendation

1. Treat modified `.rs` files, including `src/tui/footer.rs`, `src/tui/input.rs`, `src/tui/sidecar/*`, and broad tests, as one formatting-only `cargo fmt` sweep. If not intentionally wanted in the next commit, revert them as unrelated noise; otherwise commit separately as a pure format-only change.
2. Treat `stores/intake_items/schema.yaml` separately as YAML-format-only pending a value-level check; do not mix with Rust fmt or feature commits.
3. Do not keep tracked `tasks/*/T001-test-task/main.md` changes unless intentionally updating generated projections; likely revert tracked task projection churn.
4. Delete or move untracked synthetic task dirs and scratch review/design files if no longer needed; keep review notes only if they are intended durable artifacts.

## Start Here

Start with `git diff -- src/tui/footer.rs src/tui/input.rs src/tui/sidecar/*` to verify the requested TUI files are pure rustfmt. Then handle task projection churn separately with `git diff -- tasks/active/T001-test-task/main.md tasks/planning/T001-test-task/main.md` because those are substantive generated-content changes, not formatting.
