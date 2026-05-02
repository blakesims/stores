# T012 — workspace_path field + tasks next-id verb

**Date:** 2026-05-02
**Type:** task-completion note (paired with `tasks/completed/T012-workspace-path-and-next-id/`)

## Summary

Shipped the substrate-side hooks for T011's wrapper boundary: an optional `workspace_path` field on the `tasks` row that drive uses as the canonicalized cwd of every spawned agent (preserving the SDK session-fresh-on-cwd-mismatch guard), plus a read-only `stores tasks next-id` verb that scans `tasks/{active,planning,paused,completed,archived}/` to mint the next ID race-free.

Two phases, one full revision saga in Phase 1 (3 cycles to land a clean test suite under parallel harness), Phase 2 PASS first cycle. CodeRabbit Stage 6: 1 batch, 2 findings (legit), then No findings.

## Details

### What shipped

- `stores/tasks/schema.yaml` — `workspace_path: text, required: false` adjacent to `branch`.
- `Runner::spawn` trait gains `Option<&str>` workspace_path; both runner impls (Claude Code + Mock) updated.
- `ClaudeCodeRunner::spawn` canonicalizes `workspace_path` once at spawn entry; falls back to `resolve_cwd()` when `None`. Inline comment at the canonicalize site references the file-top SDK-guard doc.
- Drive validates pre-spawn: errors with `[T###] workspace_path 'P' does not exist` OR `... is not a directory` (the second error caught by CR; added `is_dir()` check).
- `stores tasks next-id` — pure inner `next_id_for_root(&Path) -> Result<String>` + thin CLI wrapper. Lenient on missing canonical dirs, ignores non-canonical (`tasks/ongoing/`).
- Tests: 6 new in `next_id.rs` (empty/sparse/highest/missing-dirs/non-task-entries/non-canonical), 4 in drive (the four spawn-time cases + 1 added on CR for is_dir), 2 in claude_code (canonicalised-when-some / falls-back-when-none).

### Phase 1 saga (3 cycles, ~6h elapsed)

The first cycle shipped the schema/trait/drive/runner work cleanly on every load-bearing axis (SDK guard preserved, validation in drive with display_id + path, MockRunner reuses the existing `unsafe impl Send`). But the two new runner-level tests (`workspace_path_canonicalised_when_some`, `workspace_path_falls_back_to_inherited_when_none`) used `unsafe { std::env::set_var("PATH", ...) }` to install a shim, which races at the libc level (setenv/getenv aren't thread-safe).

**Cycle 2** (orchestrator inline fix, ~21 LOC): added a `static PATH_MUTEX` to serialize PATH-mutating tests, plus acquired the project-wide `crate::paths::test_cwd_lock()` on the inherited-cwd test (because other tests in `paths.rs` mutate process-global cwd via `set_current_dir`). Reduced flake from ~50% to ~8% but did not eliminate it — libc-level race persists regardless of intra-Rust mutex, and a separate ETXTBSY race surfaced on a previously-untouched test.

**Cycle 3** (executor refactor, single-file commit `0687e9a`): pivoted to the cycle-1 reviewer's recommendation. Added `bin: PathBuf` field on `ClaudeCodeRunner` with a `#[cfg(test)] pub(crate) fn with_bin(...)` builder. All four PATH-using tests now use `with_bin(absolute_shim_path)` — no `unsafe set_var` anywhere. `OnceLock<ShimDir>` writes shims once into a tempdir held for the test-binary lifetime (intentional leak — avoids drop-vs-running-shim races). 27 consecutive clean parallel runs across reviewer + orchestrator independent verification.

### Phase 2 (1 cycle, clean)

`tasks next-id` shipped to spec on first executor pass. 6 unit tests, all PASS, all 6 ACs verified including the AC2.6 smoke check (`stores tasks next-id` from this repo prints `T013` because the highest existing ID at scan time is T012). 0/3 revisions. Pleasant contrast to Phase 1.

### Stage 6 CodeRabbit

Two findings, both legit, both inline-fixed (~20 LOC):

1. `drive.rs:603` — `exists()`-only validation let regular files slip through, deferring failure to `current_dir()`'s non-directory infra error. Added `is_dir()` check with a distinct error message ("is not a directory" vs "does not exist") so users know which fix applies. Plus a new test `workspace_path_set_to_file_errors_at_spawn` covering the new branch.
2. Test name `workspace_path_set_and_exists_canonicalizes` was misleading — it actually verifies pass-through to MockRunner (which doesn't canonicalize; that's the runner's job, exercised by the runner-level tests). Renamed to `workspace_path_set_propagates_to_runner`. Comment updated to cross-reference where canonicalization IS verified.

Re-ran `cr review --type all --base feat/T011-docs-wrapper-boundary --plain`: **No findings.**

## Lessons learned

1. **`unsafe std::env::set_var` is unsafe in tests, not just unsound.** The pre-existing tests in `claude_code.rs` had been using this pattern for months without failure. Adding two more tests of the same shape pushed the parallel-execution probability over the visibility threshold. The fix is not a mutex — it's eliminating the global mutation entirely (`with_bin` injection). Don't paper over libc-level races with Rust-level locks.

2. **The orchestrator-fix budget is for fixes whose scope is genuinely small AND whose mechanism is correct.** My cycle-2 mutex was small (~21 LOC) but the mechanism didn't actually eliminate the race (only narrowed its window). The reviewer was right to REVISE again. When in doubt about whether a fix is mechanically sufficient, bounce to executor — they have more context to verify the fix end-to-end.

3. **CodeRabbit caught a real bug Phase 1's review missed.** The `exists()` vs `is_dir()` issue is a textbook "validate at the boundary" fix that the cycle-1 reviewer overlooked because the test suite happened to exercise only the missing-path branch. Stage 6 is doing real work even when per-phase reviews pass.

4. **Single-purpose tests should have honest names.** `workspace_path_set_and_exists_canonicalizes` claimed more than it verified. The rename cost zero and made the test layer more honest; downstream readers won't be confused about what's exercised where.

5. **Phase split was correct.** The Decision Matrix's "two phases" call paid off: Phase 2 was self-contained and shipped on first cycle while Phase 1 was burning revisions. Compressing them would have entangled review focus and likely cost more time overall.

6. **Carry-forward (T011's lesson) was honored:** filled `## Completion` BEFORE flipping `Status: COMPLETE`. CR didn't catch a workflow-protocol violation this time.

7. **Executor committed mid-workflow on cycle 3** (commit `0687e9a`) instead of letting the orchestrator commit. This was flagged in the cycle-3 review as a process observation. For T013/T014, reinforce the rule in the executor brief: "Do NOT commit. Phase 1's cycle-3 commit by the executor was an out-of-process action that the workflow flagged; do not repeat it." (Added that line to the Phase 2 executor brief proactively; Phase 2 obeyed.)

## Follow-ups

- **T013 (queued):** Reviewer envelope + storage schema migration (binary severity for code-reviewer; new `notes`/`observations` fields on plan/code reviewer envelopes). Now unblocked by T012 (cwd semantics no longer at risk of being re-litigated mid-stream).
- **T014 (queued):** Framework write-path (envelope `observations[]` → `observations.add` with source pointer) + brief overlay + templates. Now unblocked.
- **`tasks/ongoing/`** — non-canonical directory exists in this repo but is not in `tasks/CLAUDE.md`'s canonical layout. Plan-reviewer's Q1 flagged it; plan correctly ignores it; T012 ship verified `non_canonical_directories_ignored`. Worth a one-line cleanup task to either remove the dir or document it in `tasks/CLAUDE.md`.
- **Pre-existing `unused import: crate::db` warnings** in `src/handlers/transition.rs:286` and `src/handlers/update.rs:157`. Not introduced by T012; not in scope. Worth a 5-min sweep when next touching those files.
