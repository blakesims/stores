# Traversal Matrix Plan V2 Review

**Date:** 2026-05-17
**Type:** note

## Summary

**Verdict: REVISE.** The plan is architecturally coherent and the visited-states-from-`transition_history` assertion approach is well-grounded — but the catalog has real gaps against `stores/tasks/schema.yaml`, several phase-feasibility claims are softer than the prose admits (especially Phase 5's `integration_blocked` induction and the observation-auto-promote-via-`run --once`), the wall-clock estimate is off by ~3-4×, and the doctrinal alignment slips in one specific place (the tier-A vs tier-B verb classification). None of these are kill-shots; all are addressable with sharpening before Phase 1 lands. The matrix-as-TDD-wind-tunnel premise is correct and the existing `src/cli/test.rs` + `src/runner/fake.rs` seam is in good shape to compose against.

Do not start coding Phase 1 until the catalog gaps, the L046 timing answer, the `CaseExpect` backward-compat semantics, and the token/no-token distinction across U-verbs are nailed down. Then ship.

## Details

### 1. Architectural soundness — mostly green

The `visited-states ordered subsequence over transition_history` assertion model is **the right primitive**. `transition_history` is the only ground truth in the substrate for which edges actually fired; the row schema (`store`, `display_id`, `from_status`, `to_status`, `verb`, `invoker`, `occurred_at`, `actor_note`, plus the ADR0001 `lifecycle_from/to`, `active_step_from/to`, `integration_step_from/to` columns that `src/handlers/transition.rs:1810` already maintains) is rich enough to express every meaningful expectation, **including** the lifecycle/step transitions that the bare `status` column hides.

The plan mentions only `from_status`/`to_status`; it should explicitly state whether visited entries can also key on `integration_step` transitions, since several of the catalog rows (stale-base, dirty-worktree, merge-conflict) park inside the `integrating → integrating` self-loop and the only distinguishing column is `integration_step`. **REVISE:** extend the `visited` schema to allow `integration_step_from/to` (and `active_step_from/to`) keys, not just `from/to`. Otherwise stale-base-refuses cannot express "got to `integration_step=task_review` but never reached `merging`" without an out-of-band check.

The orthogonal-dimensions struct (`Dimensions { tiers, pr_rejects, cr_revises, er_outcome, executor_mode, git_pressure, liveness, entry, human_verb }`) cleanly factors the space and the prune-incompatible-combos approach is reasonable. The matrix module skeleton (`mod.rs` / `dimensions.rs` / `enumerate.rs` / `spec.rs` / `expect.rs` / `render.rs` / `artifacts.rs`) is a clean separation of concerns. No coupling traps spotted.

One **hidden coupling trap** worth calling out: the matrix runner serializes through `LiveHarness::new` which calls `backup_live_db(&db_path)` (`src/cli/test.rs:395`). If the matrix loops 25 cases each creating a fresh `LiveHarness`, that's 25 backups and 25 task-row insertions into the *same* live DB. The plan's "v1 serial" is fine, but each case must clean up after itself (deactivate task, or at minimum mark abandoned/closed_out_of_band) or the matrix will pile up `inactive` rows that the user's daily `stores tasks` listings will then have to wade through. The current single-case `stale-base-refuses` ends with a `[cleanup optional]` deactivate hint to the user (`src/cli/test.rs:593`); 25-case matrix runs cannot leave 25 such hints. **REVISE:** add an explicit per-case teardown to the artifact bundle phase (deactivate + abandon-with-reason as the harness's last step), and document it as part of the per-case lifecycle.

### 2. Feasibility per phase

**Phase 1 (data model + unit tests).** Trivially feasible. The transition-history reader is `SELECT from_status, to_status, integration_step_from, integration_step_to FROM transition_history WHERE store='tasks' AND display_id=?1 ORDER BY id` and an ordered-subsequence matcher is ~30 lines. Unit tests against `Connection::open_in_memory()` with hand-inserted rows are straightforward.

**Phase 2 (orchestration + smoke catalog).** Feasible but the wall-clock estimate is wrong. The plan says "25 rows × ~1 minute avg = ~25 min." Reading `LiveHarness::run` (`src/cli/test.rs:406-492`), `wait_for_workspace` polls daemon-once at 1s intervals up to 300s, `wait_for_fake_er_pass` up to 900s, `attempt_accept_enqueue_and_integration` up to 45s. The happy path through `drive_loop` alone runs planner → plan-review → executor → code-review → wrap which is ≥5 daemon ticks of ≥1s plus subprocess spawn overhead for each fake-agent (~1-2s each). Realistic per-case: **3-6 minutes** for a clean happy path; **8-12 minutes** for git-pressure cases that wait on multiple integration sub-steps. A 25-case full run is more like **90-180 minutes**, not 25 minutes. That's still acceptable for a wind-tunnel run, but the plan's number undersells the cost and will set false expectations when the matrix lands. **REVISE:** redo the budget estimate honestly; consider whether the smoke catalog should be 5 (not 10) rows so it returns in <30 min.

**Phase 3 (U-moment automation hardening).** Feasible. The `--invoker ai_with_human --approve-token <T>` plumbing is already shipped (`src/cli/auth.rs:171` `verify_approve_token`); the matrix only needs to (a) read the file once at startup, (b) pass `--approve-token <T>` to every tier-A CLI call. The plan says "refuse to run if missing (fail loud, no test-mode bypass)" — good, that's the correct doctrine. **One slip** (see § 5 below): `tasks add --invoker ai_with_human` for harness-created fixture tasks does NOT need a token (the row's required fields are `actor: ai_with_human`, tier-B). The plan should distinguish between (a) fixture-creation `ai_with_human` writes that are honor-system tier-B and (b) U-moment `ai_with_human --approve-token` writes that are tier-A. As written it implies the matrix tokenizes everything, which is more than the schema actually requires.

**Phase 4 (liveness + git-failure cases).** Mostly feasible. Reading `src/runner/fake.rs:497-534`, the four liveness/payload scenarios already exist: `payload-invalid-exit-0`, `nonzero-exit`, `stall-no-heartbeat`, `sigterm-ignoring-stall`. So "Most of these exist partially — make them first-class case modes" is correct but understates how complete it is — they ARE first-class scenarios, they just aren't selected through the case-file → `STORES_FAKE_CASE_NAME` path yet. The `git-dirty-worktree` and `git-merge-conflict` cases require the harness to **write to the worktree directly** or to `main` before integrate fires; that mechanism already exists (`src/cli/test.rs:699-729` `advance_main_with_marker`). Feasible.

The `live-duplicate-drive` case is the trickiest: spawning two `stores agents run --once` while one is mid-drive needs the first to actually be mid-drive at the moment the second is spawned, which is a race window the harness has to engineer (probably by setting `STORES_FAKE_DELAY_MS` high and timing the second spawn). Feasible but flaky if not careful. **REVISE:** explicitly describe the race-window mechanism for `live-duplicate-drive`; the harness needs `STORES_FAKE_DELAY_MS=N` set per-case such that drive #1 is *guaranteed* to be holding the dispatch lock when drive #2 starts.

**Phase 5 (observation-entry + human-verb + integration-blocked-retry).** Here the plan gets honest about its problems, and the risks section flags them. Two real concerns:

(a) **Observation auto-promote timing.** L046's `auto-promote` builtin is dispatched as a subscriber from `src/flow/builtins/mod.rs:79` and the daemon's startup-sweep also drains it (`src/handlers/agents_run.rs:1230`). The plan says "harness invokes observations add, drafts intent contract, calls observations update --contract-state ready --approve-token <T>, waits ≤10s for L046 auto-promote subscriber to spawn the task." But the LiveHarness loop only calls `stores agents run --once` which exits after one tick (`src/cli/test.rs:860-895`). It is not guaranteed that the subscriber for the obs-update notification will be processed within a single `run --once` invocation — typically notifications enqueue and the next sweep drains them. **REVISE:** spell out exactly how the auto-promote subscriber fires under `run --once`. Either (i) demonstrate via test that a second `run --once` immediately after `observations update --contract-state ready` *does* drain the promote, (ii) call `run --once` in a loop with a deadline ≤10s, (iii) admit this case requires a real `stores agents run` daemon for the obs-update period and run it explicitly. As written the plan hopes-and-prays.

(b) **`integration_blocked` induction.** The plan admits this is TBD. **It's not TBD — it's already covered by Phase 4.** `git-stale-base-refuses` and `git-merge-conflict` both call `fire_mark_integration_blocked` (`src/flow/builtins/integrate.rs:303, 282`) which is the only path to that state. So `T3-integration-blocked-retry` is just `git-merge-conflict` → resolve conflict on `main` → `tasks retry-integration` → assert `integration_queued → integrated`. The schema declares the `retry-integration` verb cleanly (`stores/tasks/schema.yaml:324`). **REVISE:** drop the "TBD how" hedge and explicitly reuse the git-merge-conflict setup as the precondition for `T3-integration-blocked-retry`. It's the same fabrication, just with a recovery step.

**Phase 6 (HTML + CI).** Straightforward; no feasibility concerns.

### 3. Catalog correctness — significant gaps

I walked `stores/tasks/schema.yaml` lifecycle transitions edge-by-edge against the catalog. The catalog covers the main spine well but misses several legitimate edges that a "wind-tunnel" matrix should exercise:

**Missed edges (all live in `schema.yaml` today):**

1. **`plan_review → blocked` via `NOT_READY` gate.** `schema.yaml:234`. Distinct from `NEEDS_WORK` budget exhaustion; gate is `NOT_READY` not `NEEDS_WORK`. Catalog covers only the budget-exhausted `NEEDS_WORK` path (`T3-pr5-budget`). Add `T3-pr-not-ready`.

2. **`code_review → blocked` via `FAIL` gate.** `schema.yaml:258`. Distinct from `REVISE`-budget exhaustion. Add `T3-cr-fail`.

3. **`submit-external-review` REVISE re-entering from `blocked` state.** `schema.yaml:336` — the narrow ER-reconciliation path: a drifted-to-blocked task receives a current-head ER REVISE and re-enters `executing`. Add `T3-er-revise-from-blocked-runner`.

4. **`release-to-integration` direct from `complete` without going through `in_review/accepted`.** `schema.yaml:340`. This is the `human_acceptance_policy = delegated_by_policy` path (LiveHarness already uses `delegated_by_policy` per `src/cli/test.rs:1390`). Catalog doesn't distinguish `complete → integration_queued` (delegated) from `in_review → accepted → integration_queued` (human acceptance). Add `T3-hp-delegated-policy`.

5. **`human_acceptance_policy = required` vs `optional` vs `delegated_by_policy`.** Three values; matrix tests only the policy LiveHarness defaults to. Add at least one row per policy value, or document explicitly that the matrix tests only the `delegated_by_policy` path.

6. **`activation` gate on `integration_queued → integrating`.** `schema.yaml:296` guard `activation == 'active'`. Today's harness always calls `tasks add --activate`. A `T3-inactive-stays-queued` row would prove the gate works.

7. **T2 plan-shape enforcement.** T027 says T2 rejects `phases.length != 1`. Catalog has `T2-hp` (happy) but no `T2-multi-phase-rejected` to prove the plan-shape gate fires.

8. **`mark_drive_failed` from each source state.** `schema.yaml:271-275` declares five `* → blocked` edges (planning, plan_review, ready, executing, code_review). Catalog has `live-no-heartbeat` and `live-nonzero-exit` but they don't distinguish *which* source state the drive failed from. A wind-tunnel should pick at least 2 (e.g. drive-fail-during-executing, drive-fail-during-code-review) so you can prove the recovery edge is per-state-correct.

9. **`resume` from `blocked → planning`.** `schema.yaml:268`, U4 moment. Catalog implicitly covers it inside reject-amend-integrate, but the explicit edge is `blocked → planning` via `resume`, distinct from `rejected → planning` via `amend`. Add `T3-pr-budget-then-resume` to chain `T3-pr5-budget`'s blocked terminal into a resume.

10. **`abandon` from each non-terminal state.** `schema.yaml:348-359` declares twelve `* → abandoned` edges. Catalog has `T3-abandon-planning`. Adding 2-3 more (`T3-abandon-executing`, `T3-abandon-blocked`) would prove the verb's per-state correctness.

11. **`close-out-of-band` from each source state.** `schema.yaml:205-217` declares thirteen `* → closed_out_of_band` edges. Catalog has `T3-closed-out-of-band` (from `in_review`). Adding 1-2 more (from `blocked`, from `integrating`) would harden it.

12. **`integration_step` substep transitions inside `integrating → integrating`.** `schema.yaml:297-301`: `mark_refresh_done`, `mark_task_review_done`, `mark_testing_done`, `mark_merge_done`, `mark_deploy_done`. These are the substeps the stale-base case parks inside. The visited-states checker MUST be able to express "saw `integration_step=task_review` but not `merging`" — and the catalog should explicitly include `T3-hp` visited entries that walk all five substeps cleanly.

**No catalog overlaps spotted.** The 25 named rows are distinct.

**Unreachable paths in the catalog:** none spotted — every named row maps to a real schema transition.

The honest count is: catalog has ~25 rows, the schema has ~50+ distinct `(from, to, verb, gate)` tuples. The matrix doesn't need to hit every tuple, but the gaps above are first-class edges someone WILL break in a refactor, so they belong in v1 or in an explicit `BACKLOG_TRANSITIONS` list with a justification.

### 4. Existing code seam — composes cleanly, with one rewiring

I read `src/cli/test.rs` (2168 lines) and `src/runner/fake.rs` (1234 lines) at the relevant offsets.

**`CaseExpect` extension (`src/cli/test.rs:56-80`) — clean.** Today it's a flat struct with five fields. Adding nested `visited: Option<Vec<VisitedEdge>>`, `cycles: Option<Cycles>`, `integration: Option<Integration>`, `liveness: Option<Liveness>` is straightforward serde, and the `#[serde(default)]` semantics keep existing presets compiling. **One semantic gap the plan does not nail down:** what does "default = old behavior" mean? If a case omits `visited`, does the matcher skip the visited check entirely, or does it auto-derive an expected sequence from `task_status`? **REVISE:** explicit answer required. Recommendation: omitted → skipped (strict opt-in). Auto-derivation is a footgun.

**Matrix-module skeleton — clean.** `src/cli/test/matrix/mod.rs` slots into the existing `src/cli/test.rs` via re-export. The plan describes `src/cli/test.rs` as carrying the extended `CaseExpect`, with `matrix/` as a sibling submodule that consumes it. That composes fine, but note: `test.rs` is already 2168 lines and is the single file owning case loading, fake-mode env setup, the in-process `Harness`, and the `LiveHarness`. Adding `matrix/` as a submodule is the right move; do NOT inline the matrix runner into `test.rs`.

**`LiveHarness` reuse — works, with one missing surface.** `LiveHarness::run` currently dispatches by case-name (`src/cli/test.rs:197` `if is_stale_base_refuses_case(&case_name)`). The plan says "wire the existing `LiveHarness` per-case; `run_stale_base_refuses` becomes one of many cases dispatched by case-id." That's a real rewiring: the existing single-case `if/else` becomes a per-case-id dispatch table. Mechanical, but it should be explicit in Phase 2. Also, `LiveHarness::run` has hardcoded waiting logic that assumes "drive to in_review, then accept, then poll to integrated" (`src/cli/test.rs:406-492`). Cases like `T3-abandon-planning` and `T3-closed-out-of-band` will park at a non-`integrated` terminal — the existing loop's deadline / terminal-state checking already handles unexpected blocked/rejected states (`src/cli/test.rs:472-482`) but the matrix needs an explicit "watch for THIS expected terminal" mode, not the implicit one in `LiveHarness::run`. **REVISE:** Phase 2 should explicitly factor `LiveHarness::run` into a per-case strategy (drive-to-integrated, drive-to-terminal, drive-to-blocked-and-recover, drive-to-abandoned), not a single big match.

**Fake-runner scenario surface.** `src/runner/fake.rs:497-534` already declares `payload-invalid-exit-0`, `nonzero-exit`, `stall-no-heartbeat`, `sigterm-ignoring-stall` scenarios. The plan's "scenario additions: no_heartbeat, nonzero_exit (already partial), payload_inv" understates how done this is. The work in Phase 4 is to wire these scenarios as CASE outcomes via the existing `scripted_case_outcome` path (`src/runner/fake.rs:583-615`), which already normalizes `STALL_NO_HEARTBEAT` etc. via `normalize_scripted_outcome` (`src/runner/fake.rs:617-632`). So the actual Phase 4 build is mostly catalog + harness wiring, not new fake-runner code.

### 5. Doctrinal alignment — one real slip, one terminology issue

**The slip: tier-A vs tier-B in U-moments.**

The plan says (Phase 3): "Route all tier-A writes through `--invoker ai_with_human --approve-token <T>`: tasks accept, tasks reject, tasks abandon, tasks resume, tasks amend, tasks retry-integration, tasks close-out-of-band, observations update --contract-state ready."

Reading `stores/tasks/schema.yaml`:
- `tasks accept` (line 339) — `actor: human` ⇒ **tier-A**, token required. ✓
- `tasks reject` (line 343) — `actor: human` ⇒ **tier-A**, token required. ✓
- `tasks abandon` (lines 348-359) — `actor: human` ⇒ **tier-A**, token required. ✓
- `tasks resume` (line 268) — `actor: ai_with_human` ⇒ **tier-B** (honor system), token NOT required.
- `tasks amend` (line 345) — `actor: ai_with_human` ⇒ **tier-B**, token NOT required.
- `tasks retry-integration` (line 324) — `actor: ai_with_human` ⇒ **tier-B**, token NOT required.
- `tasks close-out-of-band` (lines 205-217) — `actor: human` ⇒ **tier-A**, token required. ✓
- `observations update --contract-state ready` — per `CLAUDE.md` doctrine, this is the U1 ratify path and is tier-A (`intent_contract.approved_by/at` is `actor: human`). ✓

So the plan conflates tier-A and tier-B. Per `CLAUDE.md` § *Approval-token doctrine*: "Fields and transitions marked `actor: ai_with_human` accept `--invoker ai_with_human` without a token." Routing tier-B verbs through `--approve-token` is harmless (token is accepted, just not required), but the plan's wording implies the token is the *gating mechanism* for all of them, which is doctrinally wrong. **REVISE:** rewrite the Phase 3 bullet to distinguish "tier-A (must present token)" from "tier-B (honor-system, no token required but harness MAY present it for uniformity)."

**The terminology issue: "fixture creation is also a U-moment?"**

`LiveHarness` creates tasks via `tasks add --invoker ai_with_human` (`src/cli/test.rs:1377-1381`). The `tasks add` row's required fields (`title`, `slug`) are `actor: ai_with_human` (`schema.yaml:6-7`), tier-B. So no token is required for fixture creation. The plan's "U-moments (accept / reject / ratify-observation / amend / abandon / retry-integration / resume) are automated via the host-bound token" implicitly excludes `tasks add` from U-moments — correct, since `CLAUDE.md`'s three U-moments are U1 ratification, U3 acceptance, U4 resume/amend/abandon, none of which are task creation. Fixture-create-task is `ai_with_human` because the harness *is* the AI proposing the row on behalf of the operator's test session, but it's tier-B and ungated. **REVISE:** make this explicit in the plan so readers don't think the harness's `tasks add` is elevating itself to U1.

**Otherwise:**

- **"Fabricate real preconditions; substrate produces real consequences"** — respected (real worktrees, real ER rows, real daemon, real DB).
- **"No raw-SQL writes"** — respected (the plan reads `transition_history` via SELECT, never UPDATEs).
- **"Fenced marker artifacts"** — respected (`fake-runner-markers/<task-id>-<case>/...`).
- **Per-case artifact bundle under `.stores/test-matrix/<run-id>/`** — clean, additive, gitignorable. Good.

### 6. Risks / open questions — what the plan misses

The risks section lists concurrency, RED-row-not-broken, integration_blocked induction, auto-promote timing, catalog drift, artifact growth. **Missing risks worth flagging:**

(a) **Per-case DB pollution across the run.** 25 task rows accumulating in `.stores/db.sqlite`. The matrix should deactivate + abandon each completed case. Today the user's `stores tasks ls` would show 25 matrix-generated rows. Mitigation: per-case teardown step (deactivate + abandon-with-reason="matrix-test-cleanup"). Same hygiene point as § 1.

(b) **Dispatch lock leakage on failure.** If a case fails mid-drive (panic, kill, timeout), the `dispatch_locks` row may stay live-owned by a dead PID. Subsequent cases will then refuse to drive until the watchdog reclaims, or the matrix run abandons mid-flight. **Mitigation:** matrix start-of-run sweeps stale `dispatch_locks`; or each case operates on a unique task-id and the dispatch lock is keyed per-task so leakage is per-case-isolated. Worth confirming which.

(c) **Live ER row pollution.** Each ER case creates `external_reviews` rows. By case 25 the table has 25+ rows. Forensically useful but inspect-tooling will need pagination/filtering. Minor; document.

(d) **Markers committed to main are intentional but pollute git log.** `fake-runner-markers/` paths on `main` from prior runs accumulate. Mitigation: `.gitignore fake-runner-markers/` or add a `stores test matrix clean` verb that drops them.

(e) **Daemon process spawn cost.** Each `run_daemon_once` spawns a child `stores agents run --once`. Across a 25-case run that's potentially hundreds of subprocess invocations. Process accounting and stderr aggregation matter; the artifact bundle already captures per-case transcripts — good.

(f) **Schema evolution drift.** The "catalog drift" risk is named but not actioned. **Add a concrete mechanism:** a `cargo test` check that enumerates `stores/tasks/schema.yaml` transitions and asserts every transition has at least one catalog row that exercises it (or is explicitly waived via a `WAIVED_TRANSITIONS` list with a justification). This turns catalog drift into a build-time failure rather than a runtime hope.

(g) **The matrix as TDD wind-tunnel is meant to gate substrate fixes — but who runs it?** Local-only? Pre-commit? CI? The plan mentions `--ci` but doesn't commit to where. **Decide:** if the matrix is the wind-tunnel doctrine claims, it must run on every substrate-touching PR. That means CI infrastructure (parallel runners, time budget) is a real cost the plan does not enumerate.

### 7. Concrete REVISE points the plan author should address

In priority order:

**R1 (must fix before Phase 1).** `CaseExpect` backward-compat semantics: when a case omits `visited`, the visited check is **skipped**, not auto-derived. Document this explicitly.

**R2 (must fix before Phase 1).** `visited` schema must allow `integration_step_from/to` and `active_step_from/to` keys, not just `from_status/to_status`. Otherwise stale-base / dirty-worktree / merge-conflict cases cannot express their substep-level assertions.

**R3 (must fix before Phase 2).** Honest wall-clock estimate. Replace "25 rows × ~1 min = 25 min" with "smoke catalog (≤10 rows) ~30 min; full catalog ~2-3 hours." Decide smoke vs full default.

**R4 (must fix before Phase 2).** Per-case teardown step in the harness lifecycle (deactivate + abandon-with-reason) so matrix runs don't pollute the user's task list.

**R5 (must fix before Phase 2).** Refactor `LiveHarness::run` into a per-case strategy (drive-to-integrated, drive-to-blocked-and-recover, drive-to-terminal-non-integrated, drive-to-abandoned). The current single-case match won't carry the matrix's diversity.

**R6 (must fix before Phase 3).** Distinguish tier-A (token-required) from tier-B (honor-system) verbs in the Phase 3 description. Specifically: `accept`, `reject`, `abandon`, `close-out-of-band`, `observations update --contract-state ready` are tier-A; `resume`, `amend`, `retry-integration` are tier-B. The matrix MAY present the token uniformly but the schema does not require it on tier-B.

**R7 (must fix before Phase 3).** Make explicit that `tasks add --invoker ai_with_human` for fixture creation is NOT a U-moment; it's tier-B honor-system. Otherwise readers will mistakenly believe the matrix's fixture creation is operator-grounded U1.

**R8 (must fix before Phase 4).** Spell out the `live-duplicate-drive` race window mechanism: `STORES_FAKE_DELAY_MS=N` on drive #1 (where N is large enough to guarantee the dispatch lock is held when drive #2 starts).

**R9 (must fix before Phase 5).** Resolve the L046 auto-promote timing question: is `stores agents run --once` after `observations update --contract-state ready` sufficient to drain the promote? If not, run in a loop with a deadline, or run a real daemon for the obs-update period. Stop hoping.

**R10 (must fix before Phase 5).** Drop the "integration_blocked induction is TBD" hedge. Reuse the `git-merge-conflict` setup as the precondition; add `tasks retry-integration` as the recovery step (tier-B, no token needed).

**R11 (add to catalog before Phase 2).** Missing edges from § 3 — at minimum: `T3-pr-not-ready`, `T3-cr-fail`, `T3-er-revise-from-blocked-runner`, `T3-hp-delegated-policy`, `T3-hp-with-substeps` (walks all five integration substeps explicitly). The others (`abandon` per state, `close-out-of-band` per state, T2 multi-phase rejection) can defer but should be tracked in a `BACKLOG_TRANSITIONS` list.

**R12 (add to risks).** Per-case DB/lock/marker cleanup, schema-drift build-time check, CI infrastructure cost. Already enumerated in § 6.

**R13 (Phase 6 nice-to-have).** A `--catalog-coverage` mode that reports "of N declared schema transitions, M are exercised by the matrix, K are waived." This is the matrix-vs-schema integrity check that prevents the catalog from drifting silently.

### Bottom line

The plan is *almost* ready. Architecture sound, seam in good shape, doctrine mostly respected, visited-states primitive well-grounded. But it has real gaps that, left unresolved, will produce a v1 matrix that gives false confidence: missing edges (the catalog doesn't enumerate all the legitimate transitions), a wall-clock estimate that will make the first full run feel "broken," a doctrinal slip on tier-A vs tier-B that will propagate to future test-mode code, and two Phase 5 hand-waves (auto-promote timing, integration_blocked induction) that need answers before coding begins.

REVISE → land R1-R10 → Phase 1 starts. R11-R13 can ride along.

## Follow-ups

- After REVISE, cross-reference with `01-live-fake-traversal-matrix-plan.md` and `02-live-fake-traversal-matrix-plan-review.md` per Blake's instruction (held until this independent review is in).
- If the plan author concurs with R6/R7, propagate the tier-A/tier-B language to any future test-mode docs to prevent the same slip recurring.
- The matrix-vs-schema schema-drift build-time check (R13 / risk f) deserves a tracking observation once the matrix is past Phase 2 — it's the durable artifact that protects the catalog from going stale.
