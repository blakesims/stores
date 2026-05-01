# Code Review — T010 Phase 4 (Wrap agent prompt + brief template + drive integration)

- **Reviewer:** code-reviewer agent (cycle 0)
- **Reviewed commit:** `13662ca` (impl) + `1480fab` (execution log)
- **Date:** 2026-05-01
- **Verdict:** **PASS**
- **Revision count:** 0/3 (Phase 4 closes on first pass)
- **Files changed (per `git show 13662ca --stat`):**
  - `agents/wrap.md` — +302/−45 (production prompt; stub marker removed)
  - `src/handlers/drive.rs` — +390/−1 (`compute_git_diff_summary` + overlay wiring + 8 new tests)
  - `src/render/engine.rs` — +70/−2 (`render_template_with_overlay` + 1 test)
  - `src/render/mod.rs` — +1 (export)
  - `stores/tasks/templates/wrap-brief.md.tpl` — +78/−13 (production template; stub marker removed)

## Verification matrix

| AC | Requirement | Test / verification | Status |
|----|-------------|---------------------|--------|
| 4.1 | `next-action` on `in_review` returns `next_agent: "wrap"` | Pulled forward in Phase 1 cycle 2; schema.yaml confirms `on_state.in_review.dispatch_agent: wrap`; existing tests cover. | PASS |
| 4.2 | Drive spawns wrap via mock runner | Pulled forward in Phase 1 cycle 2; `happy_path_one_phase_mock` + `in_review_first_iteration_dispatches_wrap` exercise. | PASS |
| 4.3 / 4.3a | State-local flag + re-entry safety | Pulled forward; `dispatched_wrap_this_run` flag at `drive.rs:421`; `in_review_re_entry_after_amend_dispatches_fresh_wrap` covers cross-run re-entry. | PASS |
| 4.4 | Wrap brief template renders without error | `wrap_brief_template_renders_with_fixture_row` (drive.rs:2049) — renders against fixture row with 3 cycles; asserts T001, Promise, Reality, Diff, Your Job sections appear; asserts `<git diff unavailable>` (catches a regression to double-brace `{{git_diff_summary}}`). | PASS |
| 4.5 | `git_diff_summary` assembled in `drive.rs`, not render | `compute_git_diff_summary` at `drive.rs:355` (`pub(crate)`); `context.rs` diff vs master is empty (`git diff master..HEAD -- src/render/context.rs`); overlay is wired in drive at lines 538-573 via `render_template_with_overlay`. Two tests: `wrap_brief_includes_git_diff_summary` (drive-level) and `render_template_with_overlay_merges_correctly` (render engine, 4 sub-cases including overlay-wins-on-conflict). | PASS |
| 4.6 | Graceful degradation: returns `<git diff unavailable>` placeholder; no panic; warning logged | `git_diff_summary_unavailable_when_no_git_and_no_commit` + `git_diff_summary_with_first_executor_commit_fallback` — both assert non-empty return + no panic. Source code verifies the fallback chain (`drive.rs:373-392`) writes the literal string `"<git diff unavailable>"` and emits a stderr warning before returning. | PASS (with caveat — see Finding 2) |
| 4.7 | Strengthened drive tests assert `wrap_log[]` content | `happy_path_one_phase_mock_wrap_log_content`, `in_review_first_iteration_dispatches_wrap_log_content`, `in_review_re_entry_after_amend_wrap_log_content` — all three assert log length, latest `executive_summary == "stub"` (matches `wrap_fixture_json()`), and non-empty `at`. | PASS |
| 4.8 | `BUNDLED_AGENTS` count == 6 | `bundled_agents_registry_complete_and_idempotent` test (cli/agents.rs:346) asserts `names.len() == 6`. wrap is the 6th entry at `agents.rs:34`. | PASS |

## Build & test gates

- `cargo build --features runner-claude-code`: **clean** (no new warnings; pre-existing 3 dead-code warnings on `add.rs`/`transition.rs`/`update.rs` predate this branch).
- `cargo test --features runner-claude-code`: **468 unit + 2 integration = 470 total**, all green. Matches executor's claim.
- `bash tests/drive_e2e.sh`: **PASS** (AC7.1 happy path + AC7.1b revise-once both green; final state `in_review`; brief written; awaiting human).
- `bash tests/tasks_e2e.sh`: **PASS** (16 steps green).

## Out-of-scope hygiene

`git show 13662ca --stat` shows exactly 5 files changed: `agents/wrap.md`, `src/handlers/drive.rs`, `src/render/engine.rs`, `src/render/mod.rs`, `stores/tasks/templates/wrap-brief.md.tpl` (+ task log in the docs commit `1480fab`).

**Critical purity check:** `git diff master..HEAD -- src/render/context.rs` is **empty**. Render stays pure `(schema, entry) → Value`. The shell-out for `git_diff_summary` is in `drive.rs` only. Decision Matrix row (j) honoured.

## Decision (j) compliance — since-ref formula

`compute_git_diff_summary` (drive.rs:355) implements the documented chain:

1. `git merge-base HEAD master` (drive.rs:373)
2. Fallback: `first_executor_commit` (drive.rs:374-378)
3. Final fallback: emit `"<git diff unavailable>"` + stderr warning (drive.rs:380-392)

Diff body assembly (drive.rs:395-401) joins `git log --oneline <since-ref>..HEAD` and `git diff --stat <since-ref>..HEAD` inside a fenced block. Matches plan exactly.

## Spot-checks

- **Wrap envelope schema match.** `agents/schemas/wrap.schema.json` requires `role` + `executive_summary`; `additionalProperties: false`. The agent prompt's example envelope (lines 193-202) and schema description (lines 204-215) match field-for-field. ✓
- **Triple-brace requirement.** `wrap-brief.md.tpl:44` uses `{{{git_diff_summary}}}`. The AC4.4 test assertion `rendered.contains("<git diff unavailable>")` would fail if a future maintainer downgraded to double-brace (Handlebars escapes `<` and `>` to `&lt;` `&gt;`). Implicit regression coverage exists; a comment in the template would make the intent clearer (see Finding 1).
- **Persona match.** Agent line 26 reads "Senior reviewer's sherpa." — exact plan wording. ✓
- **Forbidden tool list.** Agent lines 280-286 list submit-*, render, next-action, accept, reject, and Edit/Write — comprehensive.
- **Authorized tools.** Frontmatter (lines 11-19) lists Read, Glob, Grep, Bash(git diff:*), Bash(git log:*), Bash(git show:*), Bash(stores tasks show:*), Bash(stores tasks list:*) — 8 entries.

## Findings (informational, non-blocking)

### F1. TRIVIAL — No inline comment near `{{{git_diff_summary}}}` documenting why triple-brace is required.

`wrap-brief.md.tpl:44` uses `{{{...}}}` without a comment explaining that double-brace would HTML-escape the `<>` in the placeholder and the fenced diff block. The test `wrap_brief_template_renders_with_fixture_row` catches the regression by asserting the literal `<git diff unavailable>` appears in output, but a `{{!-- triple-brace required: '<git diff unavailable>' must not be HTML-escaped --}}` Handlebars comment above the line would prevent a future maintainer from "fixing" it during a cleanup pass. Trivial; fold into Phase 6 doc-cleanup.

### F2. MINOR — `git_diff_summary_unavailable_when_no_git_and_no_commit` test name doesn't reflect what it asserts.

The test name implies it exercises the `<git diff unavailable>` fallback path, but in this repo `git merge-base HEAD master` succeeds (we are on a feature branch), so the test only confirms `compute_git_diff_summary(None, None)` returns a non-empty string and does not panic. The body comment correctly notes the limitation, but the function name is misleading. To actually exercise the placeholder path the test would need `std::env::set_current_dir` to a non-git temp dir before calling — feasible (the executor noted they could not "reliably simulate no git binary" but a non-git directory is reliable). Acceptable proxy for AC4.6's "doesn't panic" invariant; flagged for Phase 6 strengthening if desired.

### F3. TRIVIAL — Pre-existing stale comment NOT fixed.

`drive.rs:1463-1464` (in `in_review_re_entry_after_amend_dispatches_fresh_wrap`) still says:
```
// Simulate non-empty wrap_log from a prior wrap dispatch (Phase 1 stub; Phase 3
// will write this via compute_submit_wrap).
```
Phase 3 reviewer's Finding 2 (Phase 3 review file, F2 in main.md:619) flagged this and recommended Phase 4 or Phase 6 fix it. Phase 4 executor claims (main.md:653) "Phase 3 Finding 1 (stale comment...) addressed by adding the strengthened test variants with the correct comment" — but the original comment is unchanged; only the new sibling test got the correct comment. The original stale text remains. Fold into Phase 6 doc-cleanup.

### F4. TRIVIAL — Pre-existing stale doc comment in `cli/agents.rs:6`.

Header comment reads "Registry: `BUNDLED_AGENTS` (5 entries) vs `BUNDLED_SKILLS` (5 entries)." Phase 1 (commit `9aaef2d`) added wrap making it 6, but the doc comment was not bumped. Predates Phase 4 and not introduced here, but the AC4.8 verification surfaced it. Fold into Phase 6 doc-cleanup.

### F5. TRIVIAL — Executor commit-message claims off-by-three.

`13662ca` commit message says "11 new tests" / "Tests: 470 total (468 unit + 2 integration), all pass." Test count is correct (470 = 462 prior + 8 new). New test functions added in this commit: `read_wrap_log_for` helper (not a test), plus 7 tests in `drive::tests` and 1 in `render::engine::tests` = **8 new tests**, not 11. Similarly main.md:638 says "3 existing tests strengthened" — actually 3 NEW tests with `_wrap_log_content` suffix were added; the originals (`happy_path_one_phase_mock`, `in_review_first_iteration_dispatches_wrap`, `in_review_re_entry_after_amend_dispatches_fresh_wrap`) are unchanged. Outcome (AC4.7 coverage exists) matches plan; counting nit only.

### F6. TRIVIAL — Three original wrap-related drive tests still queue-drain-only.

Phase 3 reviewer's recommendation was "append [wrap_log assertions] to `happy_path_one_phase_mock` and similar to the other two `in_review_*` tests." Executor chose to add 3 new sibling tests instead. Net coverage (queue-drain + wrap_log content) is strictly more than the recommendation, but the original 3 tests remain weak proxies on their own. Acceptable interpretation; flagged so future readers don't expect the originals to assert content.

## Status update

**EXECUTING_PHASE_5** — Phase 5 (guide wrap-mode + `/task:wrap` skill) is unblocked. None of the findings block the gate; all six are doc-cleanup items appropriate for Phase 6.

## DONE_WHEN propagation check

Of the six DONE_WHEN bullets in the intent contract, Phase 4 directly delivers parts of bullets 1, 3, and 6:

- **Bullet 1** (wrap agent + schema + envelope): agent prompt now production-grade (was Phase 1 stub); schema delivered Phase 2; envelope persisted Phase 3.
- **Bullet 3** (`executive_summary` persisted via wrap_log): unchanged from Phase 3; AC4.7 strengthens coverage of the wire.
- **Bullet 6** (verifiable end-to-end): `bash tests/drive_e2e.sh` exercises drive → request_review → in_review → wrap dispatch → submit-wrap → exit; passes.

Bullets 2 (lifecycle states), 4 (guide wrap-mode), 5 (`/task:wrap` skill) are Phases 1 (done), 5 (next), 5 (next) respectively.
