# Live Fake Traversal Matrix Plan Review

**Date:** 2026-05-17
**Type:** note

## Summary

The plan is directionally strong and mostly matches Blake's requirement: fake only runner text generation while exercising the real daemon, database, git, external-review, integration, marker, and telemetry surfaces. Its strongest parts are the explicit no-mocks rule, the separation between runner outputs and real substrate consequences, phased rollout, and a scoped synthetic-authority design.

I would revise the plan before implementation in four areas:

1. make synthetic authority a Phase-0/Phase-2 blocker rather than an optional MVP bridge;
2. add a harness isolation/cleanup budget for live `.stores/db.sqlite`, worktrees, branches, marker commits, and proof retention;
3. tighten the no-mocks invariant with explicit "forbidden DSL fields" and proof checks for consequence-faking;
4. add negative/safety tests for command/env leakage, production-row targeting, and stale fake binaries.

## Correct / strong

- **Correct boundary:** The summary states the intended invariant precisely: deterministic fake runners are allowed, but daemon path, `.stores/db.sqlite`, worktrees, branches, commits, external-review rows, integration attempts, subprocess markers, telemetry, validators, subscribers, and git consequences remain real (`01-live-fake-traversal-matrix-plan.md:8-14`). This matches the user's no-LLM/no-mocks requirement.
- **Good distinction between output and consequence:** The plan explicitly warns that labels like `stale_external_review` must be real environmental consequences, not fake runner outputs (`01-live-fake-traversal-matrix-plan.md:114-124`). This is essential for testing Stores rather than testing a simulator.
- **Bounded traversal strategy is sane:** Edge coverage -> boundary coverage -> runner alphabet -> perturbations -> selected pairwise coverage (`01-live-fake-traversal-matrix-plan.md:169-177`) is the right way to avoid combinatorial blowup while still producing an actionable pass/fail matrix.
- **Phasing is mostly well scoped:** The plan starts with current live harness cases, then DSL/proof schema, synthetic authority, smoke matrix, enumeration, generator, battlescars, and upstream traversal (`01-live-fake-traversal-matrix-plan.md:400-576`). That sequence avoids boiling the ocean.
- **Proof artifacts are appropriately concrete:** The proposed proof JSON includes DB backup path, ids, branches/worktrees, SHAs, marker commits, agent run ids, daemon invocations, authority events, final snapshots, and no-real-LLM assertion (`01-live-fake-traversal-matrix-plan.md:357-378`). That is the right artifact surface for debugging red rows.

## Findings and recommendations

### P0 — Do not allow the current `--invoker human` bridge into the broad matrix MVP

The plan lists a temporary bridge: keep existing harness behavior for `tasks accept --invoker human` inside the live fake harness, make it loud, and replace it before broad matrix suites (`01-live-fake-traversal-matrix-plan.md:332-337`). The current WIP really does this: live acceptance shells out to `stores tasks accept <task> --invoker human` (`src/cli/test.rs:995-1000`), and the harness globally sets `STORES_ALLOW_FAKE_REVIEW_ACCEPT=1` (`src/cli/test.rs:166-180`, `src/cli/test.rs:1133-1155`).

Recommendation: promote synthetic authority from "temporary bridge allowed" to a hard gate before `stores test suite smoke --live --matrix`. A one-case battlescar can tolerate the bridge while developing, but a matrix command repeatedly applying synthetic accept/retry/ratify needs the real scoped mechanism first. Minimum acceptance criteria:

- `stores test` can authorize only rows with test provenance and the current test-run id.
- The same command path fails closed for a normal production row.
- The proof records include every synthetic authority event.
- No broad suite command depends on `--invoker human` without token/provenance enforcement.

### P0 — The existing live harness contains raw SQL writes that contradict Phase 0 safety

The plan says "No raw SQL writes" in non-goals and safety constraints (`01-live-fake-traversal-matrix-plan.md:37-40`, `01-live-fake-traversal-matrix-plan.md:595-605`), and Phase 0 repeats the constraint (`01-live-fake-traversal-matrix-plan.md:418-422`). However, the current WIP still writes directly in at least two places:

- `freeze_latest_tooling_held_review_retry` updates `external_reviews.next_retry_at` directly (`src/cli/test.rs:1036-1045`).
- The non-live temporary harness inserts task rows directly (`src/cli/test.rs:1687-1695`).

Recommendation: make Phase 0 include a raw-SQL-write audit and removal/containment task. Reads are fine for proof. Writes should go through verbs/handlers, or the plan should explicitly scope any remaining direct writes to throwaway in-memory/unit-test harnesses only and forbid them in `--live` paths. For the live matrix, direct DB writes would invalidate the central claim that Stores produces real consequences.

### P1 — Add a live artifact isolation and cleanup phase

The plan backs up `.stores/db.sqlite` and prints artifacts (`01-live-fake-traversal-matrix-plan.md:364-378`, `01-live-fake-traversal-matrix-plan.md:595-605`), and it asks how much cleanup should exist as an open question (`01-live-fake-traversal-matrix-plan.md:607-613`). That is not quite enough for a live matrix that creates real tasks, branches, worktrees, marker commits, ER rows, and integration attempts.

Recommendation: add an explicit phase or Phase-0 deliverable for artifact lifecycle:

- standard namespace for branches, worktrees, marker paths, task titles/slugs, and proof ids;
- deterministic detection of stale test-owned artifacts from prior red runs;
- a `stores test artifacts` / `stores test cleanup --test-run <id>` design that never deletes red proof by default;
- clear rule for main-marker commits: they are intentional, fenced, printed, and never force-rewritten;
- proof retention path decision before Phase 3 (`.stores/test-runs/` vs `target/stores-test-runs/`) so suite output links are stable.

Without this, repeated matrix runs could pollute the repo enough that later failures are caused by residue rather than the tested traversal.

### P1 — Make consequence-faking impossible in the DSL, not just discouraged

The plan correctly says stale/refusal consequences must not be faked as runner outputs (`01-live-fake-traversal-matrix-plan.md:114-124`) and that case files should not prescribe direct final-state mutation (`01-live-fake-traversal-matrix-plan.md:179-181`). Recommendation: turn this into schema validation:

- forbid fields like `final_status`, `force_status`, `external_review_status`, or `integration_result` in setup/runner sections;
- allow expected outcomes only under `expect`, never under setup;
- require perturbations to be verbs/actions with proof hooks, not desired states;
- fail case normalization if a runner alphabet entry names a substrate classification that should be produced by Stores.

This preserves the no-mocks invariant as the DSL grows.

### P1 — Add negative no-LLM and fake-runner leakage tests

The plan has good positive assertions for fake mode (`01-live-fake-traversal-matrix-plan.md:29-33`, `01-live-fake-traversal-matrix-plan.md:603-604`). It should also require negative tests:

- fake-runner binary missing/stale -> preflight fails before mutating live state;
- any agent run with non-fake harness id -> suite fails and proof points at the row;
- external review runner of `codex`/`pi`/`claude-code` -> suite fails;
- fake env vars do not leak into subsequent non-test commands in the same process/session;
- `STORES_ALLOW_FAKE_REVIEW_ACCEPT` or successor test-authority env cannot affect commands outside `stores test --live` and test-owned rows.

Current WIP already has a sentinel/preflight shape (`src/cli/test.rs:307-327`) and no-real-LLM DB assertions (`src/cli/test.rs:1093-1107`), so these are feasible extensions rather than new architecture.

### P2 — Clarify live vs throwaway modes

The user specifically wants live `.stores/db.sqlite` and real git consequences. The plan supports that, but it also mentions dry/fake fixtures for proof schema validation (`01-live-fake-traversal-matrix-plan.md:441-445`). That is fine for unit tests, but the plan should define mode boundaries:

- unit tests may use temp DBs/repos for parsing and proof schema tests;
- `stores test run/suite --live` must use the current repo's real `.stores/db.sqlite` and real git;
- matrix PASS claims are valid only for `--live` runs, not dry/unit fixtures.

This prevents a later implementation from accidentally satisfying the matrix with a temp simulator.

### P2 — Put enumeration after stable proof, but add IDs for coverage from the beginning

Deferring dynamic schema parsing until Phase 4 is reasonable (`01-live-fake-traversal-matrix-plan.md:493-514`). However, Phase 1 should still add stable coverage labels to each case: schema edge ids, runner alphabet ids, perturbation ids, and authority-event ids. The matrix can initially print static coverage, then Phase 4 can replace the catalog source with parser output. This avoids rewriting case files when enumeration arrives.

## Open questions to carry forward

- Which synthetic authority representation is least likely to weaken production semantics: distinct invoker, test-only approval token, or wrapper around existing invokers (`01-live-fake-traversal-matrix-plan.md:607-610`)? My recommendation is an internal `TestAuthority` wrapper plus first-class test provenance/test-run id checks, not a broadly accepted CLI invoker.
- Should first-class provenance columns be added before broad suite use? Title/body markers are acceptable for one-off live harnesses, but columns are safer for fail-closed authorization and cleanup.
- Should `.stores/test-runs/` be gitignored if used? The plan should answer before proof JSON starts accumulating.

## Recommended revised next action

Start with a tightened Phase 0/1/2 slice:

1. audit and remove/contain raw SQL writes from live paths;
2. define proof JSON and case normalization for existing cases;
3. implement scoped synthetic authority for `accept_task` on test-owned rows;
4. add negative tests proving non-test rows and non-`stores test --live` commands cannot use it;
5. then ship the five-case smoke matrix.
