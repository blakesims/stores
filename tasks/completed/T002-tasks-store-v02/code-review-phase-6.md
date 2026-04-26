# Phase 6 Code Review — `render` verb + idempotent main.md projection

- **Gate:** PASS
- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer agent
- **Cycle:** 1 of max 3
- **Status next:** EXECUTING_PHASE_7
- **Findings:** 0 critical / 0 major / 3 minor

---

## Verification of Acceptance Criteria

| AC | Result | Evidence |
|----|--------|----------|
| AC6.1 | PASS | `compute_render_returns_content_for_executing_row` (render.rs:304-319) asserts path contains `tasks/active/WF001-my-task/main.md` and content non-empty. `run_render_atomic_write_creates_file` (render.rs:345-358) confirms the file lands on disk via `run_render_in`. |
| AC6.2 | PASS (with caveat — see m1) | `compute_render_dry_run_no_write` (render.rs:463-482) confirms compute returns `dry_run=true` + populated content + no file on disk. CLI flag wired in `dynamic.rs:204-209` (`--dry-run` SetTrue). |
| AC6.3 | PASS | `run_render_moves_directory_on_status_change` (render.rs:361-385): a `complete` row with existing `tasks/active/WF001-dir-move-task/` is moved to `tasks/completed/WF001-dir-move-task/`; old path absent; main.md present in new path. Glob detection in `find_existing_task_dir` (path.rs:87-142) walks `tasks/*/` and matches `<display_id>-*`. |
| AC6.4 | PASS — marquee | `run_render_idempotent_content` (render.rs:388-406) renders twice with no DB change and `assert_eq!(content1, content2, "two renders with unchanged DB should produce byte-identical content")`. The atomic write (`std::fs::write` to `.tmp` then `std::fs::rename`) replaces the file each run, but the rendered text is verbatim identical because nothing in `compute_render_in` is non-deterministic (no timestamps, no UUIDs in the render path). |
| AC6.5 | PASS | `compute_render_blocked_reason_in_context` (render.rs:409-442): blocked row with `blocked_reason = "Waiting for human input on scope"` produces `ctx["blocked_reason"]` populated; full render path routes to `tasks/paused/`. |
| AC6.6 | PASS | `render_is_read_only_against_db` (render.rs:445-460): reads entry pre-call, runs `run_render_in`, reads entry post-call, asserts `entry_before == entry_after`. The handler signature takes `&Connection` (not `&mut`), and `compute_render_in` only calls `read_row` — no write paths exist. |

All six ACs verified by named tests. 284 unit tests pass (263 prior + 21 new); all 13 e2e steps green.

---

## What's good

- **Compute/run split applied uniformly.** `compute_render_in` (pure) and `run_render_in` (compute + write) follow the Phase 4 cycle-2 / Phase 5 pattern. Tests assert on `compute_render_in` output where useful; `run_render_in` is exercised end-to-end for atomic-write + dir-move ACs.
- **Explicit-root design.** Both `compute_render_in` and `run_render_in` take `repo_root: &Path` and `manifest_root: &Path` parameters; the `run_render` / `compute_render` wrappers fall back to cwd. This avoids `set_current_dir` in tests (test isolation under parallel runs preserved) and is the right shape for future thread-safety work. The new `Manifest::load_from(root: &Path)` (manifest.rs:71-79) is a minor additive API but the right tool.
- **Atomic write mirrors `manifest.rs::save`.** Lines 196-200: write to `<path>.md.tmp` (via `with_extension("md.tmp")`), then `std::fs::rename`. `with_extension` correctly replaces `.md` with `md.tmp` producing `main.md.tmp`. Parent dir is `create_dir_all`'d before the write, so first-time renders into a fresh `tasks/active/...` tree work.
- **Status → status_dir mapping is correct.** `status_to_dir` (path.rs:29-37) covers all seven workflow states + a safe `"active"` fallback for unknown statuses. Five dedicated tests pin each branch (`status_dir_planning_states`, `status_dir_active_states`, `status_dir_paused`, `status_dir_complete`, `status_dir_unknown_falls_back_to_active`).
- **Multi-match glob handled gracefully.** When `find_existing_task_dir` finds two directories for the same `display_id` (e.g. someone manually moved one without removing the other), it returns `None` + warning instead of crashing — render proceeds to the canonical path. `find_existing_dir_returns_none_on_multiple_matches` (path.rs:283-292) pins this contract. This matches plan task 6.3's "If zero or more than one match exists, render to the canonical path and emit a warning (don't error — render must be idempotent and recoverable)".
- **Directory-move failure is non-fatal.** `maybe_move_dir` returns `Err` on cross-device or permission failure; `run_render_in` catches with `eprintln!` warning + continues to write at the canonical path (lines 178-184). Idempotency is preserved across move failures — re-running render still produces the same text.
- **Read-only guarantee structurally enforced.** `compute_render_in` takes `&Connection`, calls only `read_row`, which is `SELECT`-only. There is no SQL write path in render.rs at all. AC6.6's test is belt-and-braces — the type system already prevents mutation.
- **Render context covers all template needs.** Status routes via `status` reserved key (already in `RESERVED_ENTRY_KEYS` from Phase 4); `blocked_reason` from the field; `current_phase`/`current_cycle` from auto-increment fields; `plan.phases`, `plan_review_log`, `cycles` as nested JSON.
- **CLI registration is minimal and correct.** `build_render_cmd()` (dynamic.rs:196-211) declares positional `display_id` (required) + `--dry-run` (SetTrue, optional). Wired into the workflow-only verb group at line 141 alongside next-action / brief / submit-* / resume. `dispatch.rs:52-54` routes cleanly to `handlers::render::run`.
- **Commit hygiene clean.** Five focused commits (`763c8fe`, `3c05cfe`, `507d461`, `d802afd`, `f2c474f`); no amends; no force-push; tightly scoped to render-related files plus the deliberate manifest.rs:load_from addition.

---

## Findings

### m1 (minor) — `compute_render_dry_run_no_write` does not exercise the `run_render_in` dry-run branch

**Location:** `src/handlers/render.rs:463-482`

**Observation:** The test calls `compute_render_in(..., dry_run=true, ...)` and asserts `!expected_path.exists()`. But `compute_render_in` never writes regardless of the `dry_run` flag — the flag is passed through to `RenderOutput.dry_run` and only consumed by `run_render_in` at line 166-169. So the test would pass identically with `dry_run=false`: it does not distinguish the two modes.

The actual dry-run-honors-the-flag behavior — i.e. `run_render_in(..., dry_run=true, ...)` prints to stdout AND skips disk write — is verified only by direct CLI probe + e2e (the e2e doesn't currently exercise render --dry-run; the CLI is registered but no e2e step calls it).

**Impact:** A regression where someone removes the `if output.dry_run { return Ok(()); }` guard in `run_render_in` would not fail any test in the unit suite. The behavior is structurally simple, so the gap is low-risk.

**Suggested fix (optional):** Add `run_render_in_dry_run_skips_write` that calls `run_render_in(..., dry_run=true, ...)` against a setup with no `tasks/active/...` directory and asserts the file is absent post-call. Or extend e2e.sh with a render --dry-run step.

**Disposition:** Accept as-is or fold into Phase 7's e2e expansion (Phase 7 lands the bundled `tasks` store; an e2e step exercising `stores tasks render T001 --dry-run` would close this).

---

### m2 (minor) — `render.rs` lacks the explicit Phase 7 bundled-sentinel TODO comment that `brief.rs` has

**Location:** `src/handlers/render.rs:100-125`

**Observation:** The executor's P2-M1 closure documentation explicitly notes that **both** `brief.rs` and `render.rs` use the on-demand template-load pattern (main.md:1460: "both `brief.rs` and `render.rs` re-read templates from disk at call time via `schema_path` from manifest"). However:

- `brief.rs:117-121` has an explicit, prominent TODO comment: `NOTE (Phase 7): when schema_path starts with "bundled:" (e.g. the tasks store), joining it with template_path produces a nonsensical filesystem path. Fix: detect the sentinel and route to the in-memory BUNDLED_STORE_TEMPLATES map. No bundled store has a workflow today so this is latent; Phase 7 must fix this when the tasks schema (workflow-shaped) is wired up.`

- `render.rs:100-103` has a comment about "option 2 from the P2-M1 carry-forward note" but does NOT call out the bundled-sentinel gap. Phase 7 will install the bundled `tasks` store with a `bundled:tasks` sentinel `schema_path`. When `stores tasks render T003` runs, line 112 (`store_root.join(render_tpl_path)`) will produce a path like `bundled:tasks/templates/main.md.tpl` which fails `read_to_string` cryptically.

**Impact:** Symmetric with brief.rs's m2 from Phase 4 — latent until Phase 7 lands. But brief.rs documents the gap loudly at the load site; render.rs does not. Phase 7 plan-review must catch the render.rs gap by inspecting render.rs (not by reading brief.rs's TODO and remembering to apply it symmetrically). The Phase 7 carry-forward in main.md should be explicit so the planner's read-first context catches it.

**Suggested fix:** Add a similarly-shaped NOTE block at render.rs:111 (just before `store_root.join(render_tpl_path)`):

```rust
// NOTE (Phase 7): when schema_path starts with "bundled:" (e.g. the `tasks`
// store), joining it with render_tpl_path produces a nonsensical filesystem
// path. Fix: detect the sentinel and route to the in-memory
// BUNDLED_STORE_TEMPLATES map (introduced by Phase 7.6). No bundled store has
// a workflow today so this is latent; Phase 7 must fix this when the `tasks`
// schema (workflow-shaped) is wired up.
```

**Disposition:** This is a documentation gap, not a correctness gap. Accept as Phase 7 carry-forward — main.md Learnings + Phase 7 planner read-first context must include this — OR fix in a one-line follow-up commit before Phase 7 begins. The code review log entry below records the carry-forward unambiguously so Phase 7 plan-review can verify.

---

### m3 (minor — informational) — `was_directory_move` recomputes `find_existing_task_dir` in `run_render_in`

**Location:** `src/handlers/render.rs:172-186`

**Observation:** `compute_render_in` calls `find_existing_task_dir` (line 89) and uses the result to set `output.was_directory_move`. Then `run_render_in` (line 173) calls `find_existing_task_dir` AGAIN to find the actual `existing` path for the move. Two filesystem walks for the same answer.

**Impact:** Negligible performance cost (the `tasks/` tree is small). But there's a TOCTOU race: if a concurrent renderer or a human moves a directory between the compute call and the run call, `was_directory_move` says yes but the second call returns None or a different path. The current code handles this gracefully (`if let (Some(existing), Some(target_d))`), so it's not a correctness bug.

**Suggested fix (optional):** Plumb the `existing_dir: Option<PathBuf>` into `RenderOutput` so `run_render_in` doesn't re-glob. Cleaner separation of compute (decides what) from run (does it).

**Disposition:** Accept. Tiny refactor; not blocking.

---

## P2-M1 Carry-forward Disposition

The executor closed P2-M1 (WorkflowResolved threading) via Option 2 (on-demand template load) rather than Option 1 (resolve at install + thread through main.rs schema map).

**Reviewer judgment: ACCEPT.**

Rationale:
- Option 2 adds one FS read per `render` / `brief` call. Render is not in a hot loop (orchestrators call it ~10× per task, not per second). Brief is similar. Performance cost is invisible in practice.
- Option 1 would have required a parallel `HashMap<String, WorkflowResolved>` in main.rs OR widening `Schema` to hold resolved templates. Both touch the schema-loading hot path and create a bigger surface for Phase 7 to navigate.
- The on-demand pattern is consistent across brief.rs and render.rs — both read templates the same way, both will need the same bundled-sentinel fix in Phase 7.
- The tests use explicit `manifest_root` parameters, so test isolation is preserved without `set_current_dir`.

**Concern noted:** brief.rs has the bundled-sentinel TODO loud and visible (brief.rs:117-121). Render.rs does not (m2 above). Phase 7 carry-forward must enumerate BOTH load sites explicitly.

---

## Carry-forward to Phase 7

Phase 7 plan-review MUST verify the following are addressed:

1. **P6-m2 (binding):** Bundled-store sentinel detection at the template load site. When `schema_path` starts with `"bundled:"`, both `brief.rs` and `render.rs` must route to the in-memory `BUNDLED_STORE_TEMPLATES` map (introduced in Phase 7.6) instead of joining with the disk path. Without this fix, `stores tasks render T003` and `stores tasks brief T003` will fail with "cannot read template" on any installed bundled `tasks` store.
2. **P5-m2, P5-m3, P5-m4 (already enumerated in Phase 5 cycle-2 carry-forward):** `--open-questions-from-file` flag, `submit_targets` lookups, `cycles[].review.details` decision.
3. **Phase 6 spec deviation:** The plan task 6.4 referred to `stores/tasks/templates/main.md.tpl`, but Phase 6 authored only the fixture template (`tests/fixtures/workflow_minimal/templates/main.md.tpl`). Phase 7 owns the bundled `stores/tasks/templates/main.md.tpl` per the original plan task 7.5. Not a Phase 6 defect — the executor correctly noted the scope error in main.md:1481.

---

## Summary

Phase 6 lands the `render` verb with all six ACs PASS. The marquee idempotency test (AC6.4) is structurally airtight — `compute_render_in` is deterministic given a constant DB row, and atomic write replaces the file byte-for-byte each call. Directory move on status change is graceful (warning + canonical-path fallback on failure). Read-only against the DB is structurally enforced by the `&Connection` type and the absence of any SQL write paths. P2-M1 closure via Option 2 (on-demand template load) is acceptable and Phase 7 inherits one binding carry-forward (bundled-sentinel detection in render.rs to match brief.rs).

Three minor findings, none gating: dry-run unit-test coverage gap (m1), missing render.rs bundled-sentinel TODO (m2), redundant `find_existing_task_dir` call (m3). All deferrable to Phase 7 or future polish.

**Gate: PASS. Status next: EXECUTING_PHASE_7.**
