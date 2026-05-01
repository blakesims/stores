# T010 — Plan Review (detail)

**Reviewer:** plan-reviewer
**Reviewed against:** `docs/philosophy.md`, `docs/worklog/2026-05-01/02-wrap-workflow-and-go-nogo-design.md`, `stores/tasks/schema.yaml`, `agents/guide.md`, `agents/schemas/guide.schema.json`, `src/handlers/{submit,drive,guide,transition}.rs`, `src/cli/{agents,dispatch,dynamic}.rs`, `src/render/context.rs`.
**Gate:** **NEEDS_WORK**

The plan is substantively sound and well-aligned with the source design. Decisions (a)–(h) are argued correctly. Phase decomposition is schema-first (Phase 1) which is the right shape, and the dependency chain is explicit. However, three issues must be resolved before execution begins. They are concrete, fixable, and concentrated in Phase 4.

---

## 1. DONE_WHEN coverage matrix

| DONE_WHEN bullet | Plan delivers? | Where | Gaps |
|---|---|---|---|
| 1. `agents/wrap.md` + `agents/schemas/wrap.schema.json`, envelope `{role, executive_summary, deviations[], residual_risks[], recommended_sanity_checks[]}`; brief = contract + cycles[] + git diff | YES | Phase 2 (schema), Phase 4 (prompt + brief template) | Brief assembly path — see issue **A** below |
| 2. Lifecycle states `in_review`/`accepted`/`rejected` + 4 transitions with `request_review:ai_autonomous`, `accept:human`, `reject:human + --reason`, plus `rejected → ?` resolved | YES | Phase 1 (Decision Matrix) — `rejected → planning` via `amend (ai_with_human)` | Verb naming consistency — see issue **D** below |
| 3. `executive_summary` persisted | YES — `wrap_log` `list_record` (Decision (a) ratified) | Phase 1, Phase 3 | None |
| 4. `agents/guide.md` graduates to wrap-mode at `in_review` | YES, framework-layer dispatch | Phase 5 | None — correctly placed at briefing-template selection layer |
| 5. `/task:wrap` skill rewritten | YES, slim per cli-vs-skill-split | Phase 5 | Path mismatch — see issue **F** below |
| 6. End-to-end fixture verifies full lifecycle | YES | Phase 6 — `tests/drive_e2e.sh` AC7.5, fixture `happy_2phase_with_wrap.jsonl` | None |

All six DONE_WHEN bullets are addressed. The issues below are about **how**, not **whether**.

---

## 2. Hot-spot decisions (the three the planner flagged)

### Hot-spot A — AC4.3 "drive done; waiting for human" exit predicate

**Planner's heuristic (current AC4.3):** drive checks `wrap_log.length > 0 AND latest wrap_log entry.at > row.updated_at - epsilon`.

**Verdict: REJECT the heuristic. Use a state-local flag.**

The `at > updated_at - epsilon` check is fragile for three reasons:

1. **Re-entered drive iterations.** If a user runs `stores tasks drive T001` while the row is in `in_review` (e.g. after a reject → amend → re-complete cycle), the drive loop walks back into `in_review` with a freshly-appended `wrap_log` entry from a *previous* iteration. `at` is older than the row's `updated_at` (the row got bumped on `accept`/`reject` transitions in between). The predicate would say "wrap not yet written" → drive re-spawns the wrap agent → the row gets a duplicate `wrap_log[]` entry for the same review cycle. Wrong behavior.
2. **Time skew + transaction ordering.** `at` is set inside `compute_submit_wrap`'s tx; `updated_at` is also set in that tx (or in `write_status_and_fields`). They can be equal-to-the-millisecond. The "epsilon" hides a race that shouldn't exist.
3. **Philosophical violation.** `philosophy.md` is explicit: schema is the contract, DB is the truth. A predicate of the form "did this row change recently?" is a process heuristic on top of the truth, not a derivation from it. The substrate's pattern is to add a row field if you need a fact.

**Required change:** make the dispatch idempotent at the **schema level**. Two clean options, in order of preference:

- **(preferred) State-local flag inside the iteration.** `drive_loop` already knows whether *this* iteration just dispatched the wrap agent and submitted — it has the `submit_out` from `dispatch_submit`. After dispatch, re-read status; if status is `in_review` AND the iteration's `submit_out` came from a `wrap` envelope, log `[<id>] in_review; brief written; awaiting accept/reject` and `return Ok(())`. No DB read needed; the flag is the local function's knowledge that it just wrote a wrap envelope. **For drive iterations that begin with status already at `in_review`** (re-entry), `next-action` should return no `next_agent` — see option B for how.

- **(supplementary) `dispatch_agent: wrap` is one-shot per row entry into `in_review`.** Add a guard on the `on_state.in_review.dispatch_agent` action: only dispatch if the latest `wrap_log[]` entry's `at` is older than the most recent transition into `in_review` (which is queryable from the audit log, not from `updated_at`). If the framework doesn't have a "most recent state-entry timestamp" query yet, this is the cleanest place to add one. Alternative shorthand: the `request_review` transition writes a sentinel row field (e.g. `last_request_review_at`); the `dispatch_agent: wrap` action's effective guard becomes `wrap_log.length == 0 OR wrap_log[-1].at < last_request_review_at`.

The state-local flag (option 1) is enough for v0.5 because drive *currently always starts from a fresh `next-action` call* — it would only re-enter `in_review` via human invocation, and at that point the human is owning the rerun. Option 2 is the substrate-clean version and would compose better if drive ever gets restarted mid-flight by an external trigger.

**Also update Phase 4 Decisions:** the "drive's per-iteration check looks at 'did the row's wrap_log grow since I started this iteration?'" sub-decision should be replaced with the flag-based formulation. The current text reads as the heuristic the planner started from; it must be edited to commit to the flag form.

---

### Hot-spot B — `WF_SCHEMA_YAML` test fixture migration

**Planner's choice:** update `WF_SCHEMA_YAML` in `src/handlers/submit.rs` mod tests to mirror the new lifecycle so existing assertions become "ends in `accepted`."

**Verdict: APPROVE the planner's choice with a caveat.**

This is the right call. The fixture should mirror prod schema; the alternative (frozen v0.4 fixture) creates a slow rot path where the fixture diverges from reality and the tests no longer prove anything about live behavior.

**Caveat — flag the specific assertions that change:**

I read `WF_SCHEMA_YAML` and the surrounding `mod tests`. The tests that assert PASS-on-last-phase lands at `complete` are at `submit.rs` lines ~1638–1656 (search for `compute_submit_review(...PASS...current_phase >= plan.phases.length...)` and the single PASS-last assertion). Those will need to change because under the new schema, PASS-last lands at `complete` (unchanged for one tx-step) but the on-entry follow-on then drives the row to `in_review` in the same tx. The plan's Phase 1 acceptance criterion **AC1.7** mentions "test assertions are migrated to walk to `accepted`" but is non-specific about which.

**Required change to Phase 1:** AC1.7 should explicitly enumerate the test functions that will need migration so the executor doesn't have to guess. At minimum:
- The "PASS-last → complete" tests (around lines 1638, 1656 in submit.rs).
- Any test that invokes `compute_submit_review` with `current_phase >= plan.phases.length` and asserts on terminal status.
- The drive_loop "complete after drive" test in `drive.rs::tests` (line ~968: `assert_eq!(na.status, "complete", "task should be complete after drive");`) — this assertion will be wrong post-T010 because PASS-last + on-entry chain advances to `in_review`, and only after a wrap envelope has been auto-submitted does drive exit. The fix depends on whether the test uses a runner that returns wrap envelopes — it almost certainly does not (it predates this work). The plan needs to either add a wrap envelope to that test's mock runner sequence or split the test into a "drive to in_review then wait" variant.

The plan must mention the `drive.rs::tests` migration as well, not just `submit.rs::tests`. Currently Phase 6's bullet only mentions adding new tests, not migrating the existing happy-path drive test that is going to break the moment Phase 1 lands.

---

### Hot-spot C — `git_diff_summary` context var: shell-out from render path

**Planner's choice:** extend `src/render/context.rs::build_context` to compute `git_diff_summary` by shelling out to `git log` / `git diff --stat`.

**Verdict: REJECT. Move diff computation to drive (pre-render).**

Three reasons:

1. **Render is supposed to be deterministic from row state.** `build_context` today reads only `(schema, entry)` — no environment, no shell-out, no I/O beyond what serde_json does. Adding a `git` shell-out makes render non-deterministic (the same row with the same schema can produce different briefs depending on the git working tree state at the moment of render). That's a real regression. It also means render now requires a working `git` binary and a git repo at cwd, which breaks any future caller that doesn't satisfy those (CI test fixtures, sandbox runs, mock execution).

2. **Git is not row state.** `philosophy.md` is explicit: the row is the truth. If we want a diff in the brief, the diff (or a derived summary of it) should be computed by the caller and passed in as **brief context** — exactly the same way the planner brief gets `cycles[]` from the row, or the executor brief gets phase-specific data.

3. **Coupling is wrong-way.** The render module should know nothing about the drive harness's environment. Drive knows it's running in a git repo, knows which branch the task lives on, and is the natural place to assemble agent-specific context.

**Required change to Phase 4:** remove the `src/render/` modification. Instead:
- In `drive.rs`, before the brief render for the wrap agent, compute the git diff summary locally (shelling out from drive is fine — drive already does I/O).
- Pass the result as an **extra context map argument** to `render_template`, OR mutate the local `ctx` `serde_json::Value` returned by `build_context` to add `git_diff_summary` before passing it in.
- The wrap-brief template uses `{{git_diff_summary}}` as before; the value comes from drive, not render.
- For non-wrap agents the variable is absent (no template references it), so no change.

If this requires a small extension to `render_template`'s signature to accept extra context, that's correct — it's the same pattern as a "context overlay." Cheaper than baking shell-out into render.

**Caveat:** the `branch` field on `tasks` is `required: false`. The plan's "graceful degradation when branch is unset" is fine, but the planner should also pick a sensible default for "no branch" — the worklog discussion implies "diff against master since task creation," which can be approximated by `git diff $(git merge-base HEAD master)..HEAD` regardless of `branch`. State the chosen formula in the Decision Matrix so the executor doesn't invent one.

---

## 3. Other issues found

### D — `amend` verb used in two contexts; pick one

The plan declares the `rejected → planning` verb as `amend` (Decision (g) and Phase 1). Existing schema has `resume` for `blocked → ready`. The text in main.md's "Open decisions / risks" section refers to it as `resume` once and `amend` twice. The risk is that the same verb name could later appear elsewhere with different semantics. **Required change:** add an explicit row to the Decision Matrix saying "verb name `amend` (NOT `resume`); rationale: distinct semantics — resume preserves current_phase, amend resets phase 0." (Phase 1 already says this in passing; promote it to a first-class decision so executors know it's deliberate, not transcription.)

### E — `submit-targets.submit-wrap == wrap_log` but wrap is the only user-callable submit verb that doesn't fit the existing pattern

`submit-plan-review` writes to `plan_review_log` (a list_record); `submit-wrap` writing to `wrap_log` parallels that exactly. **Approve**, but Phase 3 should explicitly walk through the dispatch.rs arm — including the four `--*-from-file` companions for `executive_summary`, `deviations`, `residual_risks`, `recommended_sanity_checks` — and confirm `read_lines_from_file` is the right helper for the three list-typed companions. The plan currently says "mirror submit-plan-review's arg style," which is right but vague. **Recommended:** add an AC3.7 sub-bullet listing exactly which helpers are reused.

### F — Skill location

The plan says `skills/task:wrap/SKILL.md` (new directory under repo root). The existing skills layout (`skills/gate:walk`, `skills/observation:log`, etc.) has no `SKILL.md` files — the convention I can see in the repo is `skills/<name>/<entrypoint>.md` or similar. **Required:** before Phase 5, the planner needs to confirm the actual skill-file layout by reading one existing skill. Current state of the plan is "a guess that may not match the loader." A single `ls skills/gate:walk/` worth of investigation, then commit to the right filename in the plan.

### G — Actor enforcement is real but Phase 6 testing of it is light

The plan calls for actor enforcement on `accept` (human only) and `reject` (human only). Phase 6 mentions these tests. **Required:** add a specific AC6.X bullet asserting that the test triggers `detect_invoker` resolution under both `CLAUDECODE=1` (auto AI) and unset (auto human) — not just the lower-level `Actor::AiAutonomous` rejection inside `transition::run`. The integration path is what matters; the unit-level rejection is already covered by the existing `transition_actor_rejects_*` patterns. Need at least one CLI-shape test (subprocess-style, like `tasks_e2e.sh`) where `CLAUDECODE=1 stores tasks accept T001` exits non-zero and `stores tasks accept T001` (no env) succeeds.

### H — Drive integration: who calls `next-action` after wrap envelope is submitted?

Phase 4 AC4.2 says "the row transitions to `in_review` (verifying drive→submit→follow-on→wrap-mode loop) within one drive iteration after `submit-review PASS-on-last-phase`." Reading the existing `drive_loop`:

1. Iteration N: `next-action` returns `next_agent: code_reviewer` (status=`code_review`). Spawn → submit-review PASS-last → tx writes status=`complete` and fires on-entry follow-on to `in_review`. Iteration N ends.
2. Iteration N+1: `next-action` reads status=`in_review` → returns `next_agent: wrap`. Spawn → submit-wrap → tx appends `wrap_log[]` and writes nothing else (no on-entry follow-on from `in_review`; `dispatch_agent` is not a tx-time follow-on, it's a next-action signal). Iteration N+1 ends.
3. Iteration N+2: `next-action` reads status=`in_review` again → if `dispatch_agent: wrap` is unconditional, returns `next_agent: wrap` *again*. Re-dispatches. **Infinite loop without the dispatch idempotency guard from issue A.**

The plan's "drive exits with 'waiting for human'" works only if the exit predicate from issue A is correct. The dependency between Phases 3, 4, and the issue-A fix must be made explicit: **Phase 4 cannot land until the dispatch idempotency mechanism is chosen and built into either `next-action` (option 2 from issue A) or `drive_loop` (option 1).** Right now the plan treats AC4.3 as a small post-hoc decision; it's actually load-bearing for the entire drive integration.

---

## 4. Things the plan got right (worth recording)

- **Schema-first phase ordering** with Phase 1 = lifecycle extension is correct and matches the philosophy thesis.
- **`wrap_log` as `list_record`** (Decision (a)) is the right call. Reject-then-re-wrap will produce a second entry; history is preserved without versioning gymnastics.
- **No `gate` field on the wrap envelope** (Phase 2 sub-decision) is correct. The wrap agent synthesises; the human decides via `accept`/`reject`. Conflating those is exactly the kind of authority leak the philosophy warns against.
- **Mode dispatch at the framework layer for guide** (Decision (f)) is correct and matches the existing two-mode pattern. The agent prompt describes all three modes and is **told** which it's in via the brief header — no row-state inspection in-prompt.
- **`accept`/`reject` as plain transitions** (not submit verbs) is correct. They go through `handlers::transition::run`, which already enforces actor and transition resolution. No new write path.
- **Strict envelope schema** (Decision (h)) is correct.
- **`amend → planning` rather than `→ executing`** (Decision (c)) is the philosophically clean call and the plan's argument is sound.
- **Skill is one-screen** is correct per the cli-vs-skill-split design.

---

## 5. What I want to see before marking READY

The planner needs to:

1. Replace AC4.3's heuristic with the state-local-flag formulation (Hot-spot A). Update the Phase 4 "Decisions made" sub-bullet accordingly. Optionally extend with the next-action-side guard if you want defense-in-depth.
2. Move `git_diff_summary` computation from `src/render/context.rs` into `drive.rs` and document the chosen "since-ref" formula in the Decision Matrix (Hot-spot C).
3. Enumerate the existing tests in `submit.rs::tests` and `drive.rs::tests` that need migration, in Phase 1 AC1.7 / Phase 6 (Hot-spot B).
4. Verify the skill file path against an existing skill in `skills/` and commit to the actual filename (issue F).
5. Add the `amend`-vs-`resume` verb naming row to the Decision Matrix (issue D).
6. Add a CLI-level actor-enforcement integration test bullet to Phase 6 (issue G).
7. Make explicit in Phase 4's "Dependencies" line that AC4.3's idempotency mechanism is a *prerequisite* for Phase 4's dispatch loop, not a post-hoc tweak (issue H).

None of these is a fundamental reshape. They're concrete, scoped corrections to a plan whose intent and architecture are correct. NEEDS_WORK is the right gate.

---

## 6. Open questions for the human

None. Decisions (a)–(h) are all argued and ratified by the plan. The seven items above are corrections, not open questions.

---

## 7. Gate: **NEEDS_WORK**

Re-run planner with the seven corrections above. Expect a single round trip — none of the issues require rethinking the architecture, just tightening the implementation specification.
