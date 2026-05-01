# T010 Wrap Workflow

**Date:** 2026-05-01
**Type:** note

## Summary

T010 closed the substrate's last 10%: the gap between "agent says complete" and "human says GO/NO_GO." Before this task, `complete` was terminal and GO/NO_GO lived only in chat. After it, every task that reaches the end of its phase loop triggers a typed row event chain — the wrap agent synthesises an executive brief, the row enters `in_review`, and the human reviewer issues `accept` or `reject --reason` via actor-gated transitions. The result is a first-class, actor-attributed GO/NO_GO fact in the DB.

The schema side added 3 new states (`in_review`, `accepted`, `rejected`) and 4 new transitions: `request_review` (ai_autonomous, fires automatically on PASS-on-last-phase), `accept` (human, terminal for v0.5), `reject` (human, requires `--reason`, writes to `wrap_log[-1].reject_reason`), and `amend` (ai_with_human, resets `current_phase`/`current_cycle` to 0 for re-planning). Persistence is via a `wrap_log` list_record — not a bare column — so executive summaries accumulate across re-wrap cycles. The agent side added the `wrap` agent (synthesis brief at completion), promoted `guide.md` to a third wrap-mode (renders the brief, exposes `accept`/`reject` write-back), and added the `/task:wrap` skill as the human reviewer's entry point. Drive integration auto-fires `request_review` via a state-local flag `dispatched_wrap_this_run`, ensuring exactly-once dispatch without relying on `wrap_log` timestamp heuristics.

## Decisions ratified

### Decision Matrix (11 sub-decisions)

| # | Decision | Choice |
|---|----------|--------|
| a | `executive_summary` location | `wrap_log` list_record (not a column). Preserves full history across re-wrap cycles. |
| b | Wrap timing | Eager auto-fire on PASS-on-last-phase. Brief is ready when the human shows up. |
| c | Reject re-loop verb | `amend` (ai_with_human) — distinct from `resume`. Amend resets `current_phase`/`current_cycle` to 0; resume preserves phase. Forces re-statement of intent before re-doing work. |
| d | Terminal state for v0.5 | Simple `accepted` (not `accepted-pending-ship`). Ship structure deferred until first real ship task. |
| e | PASS-on-last-phase path | `complete` stays as transient state + on-entry follow-on to `in_review`. Keeps `submit-review` algorithm byte-identical; the follow-on machinery advances through. |
| f | Guide mode dispatch | Framework layer (brief header in `guide.md`), not agent prompt. Keeps mode-switching logic out of model inference. |
| g | (see i) | — |
| h | Wrap envelope strictness | `additionalProperties: false`. Catches typos at parse time. |
| i | Reject re-loop verb identity | `amend` is the canonical verb. Distinct semantics from `resume`: amend = re-plan from scratch; resume = continue from saved phase. |
| j | `git_diff_summary` since-ref formula | `git merge-base HEAD master` → `cycles[0].executor.commit` → `<git diff unavailable>`. Computed in `drive.rs`, NOT in render (render stays pure). |
| k | Drive idempotency | State-local flag `dispatched_wrap_this_run` (not wrap_log timestamp heuristic). |

## Surprises

- **Phase 1 follow-on machinery gap.** `compute_submit_review` had to call `fire_on_entry_follow_ons` explicitly to advance through the new `complete → in_review` follow-on. The plan flagged that follow-on machinery would handle it, but didn't call out that `compute_submit_review` was a separate code path that bypassed the normal entry-follow-on firing. Found and fixed in the Phase 1 executor pass.

- **Downstream `complete`-as-terminal audit was under-specified.** The plan noted "schema additivity needs an audit" without enumerating. Phase 1 revision cycle 1 found 4 places hard-coding `complete` as terminal: `status.rs::is_terminal`, `next_from_status`, `render/path.rs::status_to_dir`, and `main.md.tpl`'s Completion section. All required updating. Future schema additivity should enumerate affected callsites explicitly in the plan.

- **"Good simplification" that silently regressed semantics.** Phase 1 cycle 1's revision removed the state-local flag and used a status-only loop-top guard instead. The reviewer approved it as a "simplification." Cycle 2 found this regressed eager-wrap to lazy mode — the wrap agent was never dispatched because the status guard fired at the wrong moment. Lesson: when a reviewer proposes a simplification that touches dispatch timing, verify the new code's observable behaviour matches the spec before approving.

- **Phase 6 had two real implementation gaps papered over.** `reject --reason` and `amend` phase/cycle resets were described in the plan but not implemented in the first executor pass — tests were written to match the (broken) implementation rather than the spec. Code reviewer caught both via DONE_WHEN-level scrutiny. Lesson: tests that "reflect actual implementation" are not the same as tests that "verify the spec." DONE_WHEN criteria must be independently checked against the spec, not just against the code that was written.

- **Phase 4 work was mostly subsumed by Phase 1 pull-forward.** The state-local flag (AC4.3) was pulled into Phase 1. Phase 4 ended up being the wrap agent prompt, brief template, and `git_diff_summary` overlay — smaller than the original estimate. Pull-forwards that change a later phase's scope should update that phase's AC list to avoid phantom "already done" confusion.

- **Handlebars triple-brace required.** `{{{git_diff_summary}}}` is needed in the brief template to avoid Handlebars HTML-escaping the `<git diff unavailable>` placeholder string. Double-brace would silently corrupt the placeholder on degraded-path renders.

## Follow-ups

- **Ship-as-separate-task (T0xx).** Add `shipped` terminal state + parent/child task linking when the first real ship task is filed (~5 manual ships from now). Decision (d) deferred this explicitly.

- **Q&A persistence in wrap_log.** The current `wrap_log` entry holds `executive_summary`, `deviations[]`, `risks[]`, `sanity_checks[]`, and `reject_reason`. If Q&A pairs from wrap-mode chat become load-bearing for audit, add `qa_pairs[]` sub-field. Stay ephemeral until patterns demand persistence.

- **Cross-store guards (T011).** Tasks reference observations and gates; cross-store referential integrity is currently "hope." A guard mechanism unblocks `linked_observations`/`gate_refs` becoming load-bearing.

- **TUI for the morning queue (T012).** `stores day open` (CLI shape from the cli-vs-skill-split spec) is the natural surface for surfacing `in_review` tasks alongside other inbox items.

- **Verb-string-keyed field-reset generalization.** `transition.rs::run_in_tx` currently has `if verb == "amend"` for field resets. If a second verb needs similar reset semantics, generalize to schema-declared `on_transition.reset_fields`. Defer until a second case arises.

- **`compute_git_diff_summary` graceful-degradation test coverage.** AC4.6 exercises both-fallbacks-absent. A test that chdirs into a non-git tmpdir would directly exercise the `<git diff unavailable>` placeholder path. Future hardening.

- **Skill naming convention review.** `skills/task:wrap/SKILL.md` uses colon in the directory name. Works on Linux; defer cross-platform concern until a Windows user appears.
