# Plan Review — T012: workspace_path field + tasks next-id verb

**Reviewer:** plan-reviewer agent
**Date:** 2026-05-02
**Gate:** READY

## Verdict

**READY.** The plan is honest, scope-disciplined, and traces every Acceptance Criterion to a DONE_WHEN clause. The Decision Matrix is unusually thorough (15 rows) and pre-empts every reasonable executor-side question. The executor can proceed.

The plan also resists two traps it could plausibly have fallen into: (a) re-litigating the locked decisions from the morning design discussion (it does not — every locked call is recorded as "Intent Contract pre-locked, defended for completeness"), and (b) folding `next-id` into Phase 1 just to finish faster (the split is justified by independent failure modes and different review focus, which is correct).

One small accuracy nit and one interpretation note are below; neither blocks the gate.

## DONE_WHEN coverage

| DONE_WHEN clause | Plan coverage | Verdict |
|---|---|---|
| C1 — `tasks` schema has `workspace_path: text, required: false` | Phase 1, schema delta line, AC1.1 | Covered |
| C2 — drive uses workspace_path as canonicalized cwd, preserving the SDK session-fresh guard | Phase 1 — runner conditional, file-top doc updated, inline comment referencing lines 33–38 / 305–306 mandated, AC1.2 + AC1.3 | Covered (see "SDK guard" note below) |
| C3 — when unset, drive uses inherited cwd (no regression) | Phase 1 — `None` branch falls through to `resolve_cwd()`; AC1.4 explicitly requires the existing `cwd_canonicalised_before_spawn` test to still pass | Covered |
| C4 — set + missing path → loud error at spawn time | Phase 1 — drive pre-check via `Path::new(p).exists()`, `bail!` with display_id + missing path; AC1.5 + dedicated test asserting runner queue is undrained | Covered |
| C5 — `tasks next-id` scans the five canonical dirs, prints next ID, read-only | Phase 2 — entire phase; AC2.1, AC2.2, AC2.3 | Covered |
| C6 — tests cover four spawn-time cases + canonicalize-stable across spawn/resume + next-id scan | Phase 1 — four named drive tests + two runner-level tests; Phase 2 — six next-id tests; AC1.6, AC1.7, AC2.4 | Covered (see "spawn/resume" interpretation note below) |

All six clauses reachable from ACs without inference. Mapping is honest — every "(DONE_WHEN N)" tag in the AC list actually verifies clause N.

## SDK guard preservation — load-bearing check

This is the highest-risk surface in the plan and I checked it carefully. The current code at `src/runner/claude_code.rs:297-309` shows `spawn` reads `let cwd = resolve_cwd()?;` exactly once, immediately at function entry, then passes `&cwd` to `cmd.current_dir(&cwd)`. The plan's Phase 1 instruction (line 113 of main.md) replaces that single `let cwd = resolve_cwd()?;` with a single `if workspace_path.is_some() { canonicalize once } else { resolve_cwd() }` — also at function entry, also computed once per `spawn()` invocation, also passed to `cmd.current_dir(&cwd)` exactly once. There is no inner site at which canonicalization could leak in, and no per-call mutation of cwd. The guard at lines 33–38 is preserved structurally, not just by convention. Decision Matrix row 15 also mandates an inline comment at the new conditional, which is the right defensive move for a future drive-by edit.

## Trait signature ripple — call site coverage

Verified by independent grep. All `runner.spawn(...)` call sites in the codebase:
- `src/handlers/drive.rs:609` (production) — plan covers
- `src/handlers/guide.rs:274` (production, gate form) — plan covers (passes `None`)
- `src/handlers/guide.rs:347` (production, tasks stub form) — plan covers (passes `None`)
- `src/runner/mock.rs:105, 110, 118, 133, 134, 141, 166` (in-module tests) — plan covers
- `src/runner/claude_code.rs:665, 717` (in-module tests) — implicitly covered by "update the spawn signature in this file"; the plan does not call them out by line number but they are inside the same `#[cfg(test)] mod tests` module touched in Phase 1

That is every call site. No misses.

The doctest at `src/runner/mock.rs:30` is also called out by the plan and needs the `None` parameter added — confirmed.

## Validation location — is drive's pre-check sufficient?

Decision Matrix row 5 puts validation in drive (not the runner). Verified the rationale: drive has the task display_id (used in the error message via `[{display_id}] workspace_path '{p}' does not exist`), the runner does not. Good.

There is one small gap worth noting (not a gate-blocker): if a future caller invokes `runner.spawn(...)` directly with a `Some(workspace_path)` that points at a missing directory, the runner's `canonicalize().context("workspace_path canonicalize failed")?` will surface the underlying IO error. That is a less-friendly message than drive's pre-check, but it is still an error, not silent fallback. Decision Matrix row 5 explicitly chose (b) over (c) "defense in depth" and gives the right reasoning. The runner's own `canonicalize()` call is the implicit second line of defense. Acceptable.

## Test seam for `next-id` — regex and pure-function

Regex `^T(\d{3,})(-|$)` verified against the cases the plan promises:
- `T999-x` matches, captures `999`. ✓
- `T013` (no slug) matches, captures `013`. ✓
- `T9` rejected (fewer than 3 digits). ✓
- `Toops-x` rejected (no digits). ✓
- `T009foo` rejected (no `-` or end after digits). ✓
- `README.md` rejected (no leading `T<digits>`). ✓
- `notes/` (dir name without `T###`) rejected. ✓

The `non_task_entries_ignored` test (Phase 2) covers both file (`README.md`) and dir (`notes/`) cases per the plan. Good.

The pure-function test seam (`next_id_for_root(root: &Path)`) per Decision Matrix row 13 is the right call — `set_current_dir` in tests is process-global state and dangerous in parallel test runs, and the codebase already uses this pattern (`resolve_cwd()` exposed at `claude_code.rs:269`). Defended well.

## Scope discipline

Verified the plan's "test fixture line numbers" against the file. `drive.rs` lines 1198, 1319, 1360, 1407, 1440, 1482, 1511, 1573, 1715, 1883, 1939, 1987, 2028 are all `MockRunner::new(vec![...])` constructors — call sites that *consume* the runner via `drive_loop(...)`. The plan correctly characterises these as "mechanical signature updates" — they do not call `runner.spawn(...)` directly; they only need to compile against the new `MockRunner::spawn` signature once the trait is updated. No behavioral change. Confirmed in scope.

## Plan accuracy nit (non-blocking)

Phase 1, line 127 of main.md says: "Doctest / unit tests at lines 1078, 1110, etc. updated mechanically." Lines 1078 and 1110 in `guide.rs` are NOT `runner.spawn(...)` call sites — they are `MockRunner::new(vec![runner_out])` constructors and prose comments inside test fixtures. Like the drive.rs fixtures, they will compile against the updated `MockRunner` without source-level changes (the trait method signature changes, not the constructor signature). The plan's broader claim ("mechanical updates") is correct; the specific line numbers for guide.rs test fixtures are an over-citation — there is nothing to update at those lines.

This is a documentation accuracy nit, not a structural problem. The executor will discover this on inspection and skip the update. Worth fixing on a future revision but does not warrant NEEDS_WORK.

## Spawn/resume continuity — interpretation note (non-blocking)

DONE_WHEN clause 6 says tests cover "set+canonicalize-stable across spawn/resume." The plan's test `workspace_path_canonicalize_stable_across_spawns` runs the row through two consecutive spawn cycles within a single `drive_loop` and asserts byte-identical paths. This is the strongest test that is reachable today, because the runner does not yet implement `--resume` (`claude_code.rs:23-24` calls it a "future workflow"). The canonicalize-once-per-spawn pattern *structurally* protects future resume by ensuring the cwd computation is deterministic and side-effect-free. So the test exercises the *mechanism* that protects spawn/resume, even though it cannot exercise resume itself.

If the orchestrator wants to be strict, the AC could note this explicitly ("the spawn/resume continuity guarantee is verified structurally via canonicalize-once contract; resume itself is deferred to the v0.4 resume feature"). The plan's current AC1.6 wording is honest enough; calling this out further is polish, not correctness.

## Hidden assumptions / risks — none new

The plan's Risks/Assumptions section captures everything I would have flagged:
- SDK guard preservation (covered by inline comment + structural canonicalize-once placement)
- Both `Runner` impls move together (covered by Phase 1 file list)
- Lenient on missing dirs for `next-id` (covered by Phase 2 + Decision Matrix row 8)
- Carry-forward from T011: fill `## Completion` before flipping `Status: COMPLETE` (covered)

## Open questions

**Q1 (low-priority, non-blocking, planner already raised):** `tasks/ongoing/` exists in this repo as an empty directory but is non-canonical per `tasks/CLAUDE.md`. The plan ignores it (Decision Matrix row 14, Phase 2 test `non_canonical_directories_ignored`). Confirmed appropriate — `next-id` should not silently extend the canonical layout. This is worth surfacing to the orchestrator but does not change the plan: should `tasks/ongoing/` be removed in a follow-up cleanup task, or is it intentional? My read is that it is vestigial and removable in a separate small task; not in scope here.

**Q2 (carry-forward from T011, planner already raised):** Fill `## Completion` before flipping `Status: COMPLETE`. The Intent Contract Risks/Assumptions captures this. Re-flagged for orchestrator attention. No new content from me.

**Q3 (new, low-priority):** When the executor adds `MockRunner::workspace_paths_seen: RefCell<Vec<Option<String>>>` per Decision Matrix row 9, ensure the `Send` impl is still safe. `RefCell<Vec<Option<String>>>` is `!Sync` but `Send` (matches the existing `RefCell<Vec<RunnerOutput>>` queue field). The existing `unsafe impl Send for MockRunner` at `src/runner/mock.rs:61` already covers this; no new unsafe needed. Mention it so the executor doesn't accidentally add a second `unsafe impl`.

## Risk of misaligned output

Low. The plan's per-file change list is precise enough that the executor cannot wander. The four spawn-time tests are named and bound to specific assertions. The Decision Matrix names every alternative considered and why it was rejected, so the executor cannot accidentally re-implement option (b) where (a) was chosen.

The one residual risk: an executor might be tempted to add a path-existence check at write-time (Decision Matrix doesn't have a row for this, but the Intent Contract Out-of-Scope explicitly forbids it). The plan's Out-of-Scope list mirrors this. Worth a carry-forward note.

## Carry-forward notes for the executor

1. **Do NOT add a path-existence check at `tasks add` / `tasks update` write time.** Validation is spawn-time only. The Intent Contract and the plan both forbid write-time validation, and adding it would silently constrain users who legitimately set `workspace_path` ahead of provisioning the worktree.

2. **The inline comment at the new conditional in `claude_code.rs` is load-bearing.** Decision Matrix row 15 specifies one-line. Suggested wording: `// Canonicalize once at spawn entry; the SDK silently mints a fresh session if cwd differs across resume calls (see lines 33–38).` Future drive-by edits MUST see this without scrolling.

3. **`MockRunner::workspace_paths_seen` reuses the existing `unsafe impl Send`.** No new `unsafe` needed; the existing `RefCell<Vec<RunnerOutput>>` field demonstrates the pattern.

4. **The drive.rs and guide.rs test fixtures (mock constructors) do NOT need source-level updates** — they will compile through the trait change automatically because they don't call `runner.spawn(...)` directly. The plan's line citations at `guide.rs:1078, 1110` are incorrect targets; skip them. The actual production `runner.spawn(...)` sites at `guide.rs:274, 347` are the only ones requiring a `None` argument.

5. **Carry-forward from T011: fill `## Completion` before flipping `Status: COMPLETE`.** Already in the Intent Contract; restated here so the orchestrator does not re-discover the omission.

6. **`tasks/ongoing/` exists but is empty in this repo.** The `non_canonical_directories_ignored` test (Phase 2) creates a `tasks/ongoing/T999-x` fixture inside `tempdir()`, which is unrelated to the real-repo `tasks/ongoing/`. No conflict.
