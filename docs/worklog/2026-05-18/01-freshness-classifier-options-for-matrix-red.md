# Freshness Classifier Options For Matrix Red

**Date:** 2026-05-18
**Type:** note

## Summary

The fake traversal matrix has reached the point where it exposes a real substrate RED: `git-stale-base-refuses` runs against the current repo with fake runners only, advances real `main` after fake external-review PASS, then integration wedges at `integrating/task_review` instead of producing a typed freshness decision. The next design decision is not “hard block vs auto-review.” The better decision is to introduce a typed freshness classifier that decides whether main/head movement requires refresh, retest, re-review, conflict resolution, or hard block.

Blake's constraint: high parallel throughput matters. Think 10+ simultaneous worktrees and 10–12 branches/hour attempting final review/integration. In that world, “main moved, therefore always re-review” is too conservative, but “main moved, auto-land anyway” is unsafe.

## Current matrix status

Implemented/proven:

```bash
cargo test -q cli::test --bin stores
# PASS: 32 tests

stores test matrix --mode lab --catalog smoke --watch
# 4 PASS / 0 FAIL / 1 SKIP / 0 ERROR

stores test matrix --mode current --only git-stale-base-refuses \
  --watch --i-understand-this-mutates-current-repo
# 0 PASS / 1 FAIL / 0 SKIP / 0 ERROR
```

The current RED produced:

- task `T019`;
- fake runner-only planner/executor/review/wrap/external-review;
- fake external review PASS with recorded base/head;
- real fenced main marker commit `88b7e276 fake-run(T019): stale-base main advance`;
- integration attempt that parked at `integrating/task_review`, `integration_attempts=null`, unfinished integrate lock;
- `no_real_llm=ok`.

The important failure is not that it did not choose a particular policy. The failure is that it did not produce a clear typed freshness classification and recoverable next action.

## How large/high-throughput projects handle this class of problem

### 1. Strict merge queue

Branches enter a queue, rebase/merge onto current main, rerun required checks, then land.

Pros:
- very safe;
- simple mental model;
- avoids landing untested/unreviewed combinations.

Cons:
- can be expensive and slow;
- high main churn can starve branches;
- often over-invalidates harmless main movement.

### 2. Speculative merge queue / merge train

The system tests a speculative stack: `main + B`, `main + B + C`, `main + B + C + D`. If earlier branches land, later speculative results may remain valid.

Pros:
- high throughput;
- amortizes test cost;
- works well when many small branches land per hour.

Cons:
- much more complex;
- one failure can invalidate downstream speculative results;
- may be overkill for Stores right now.

### 3. Scope-aware freshness

Compare changed paths/scopes:

```text
branch_changed_paths = files changed by candidate
main_changed_paths_since_review = files changed on main since reviewed_base
```

If the sets do not overlap, review may remain valid and only refresh is needed. If they overlap, require re-review/retest depending on risk.

Pros:
- much smarter than “main moved = stale everything”;
- scales well;
- good fit for Stores because changed paths are available from git.

Cons:
- path overlap is an approximation;
- dependencies and generated/schema effects may not be visible from paths alone.

### 4. Ownership-aware invalidation

Use CODEOWNERS-like boundaries or store/module ownership. A branch touching `integration_lane` is invalidated by main changes in the same ownership/risk boundary, but not by unrelated docs/search/UI changes.

Pros:
- closer to human review responsibility;
- useful in monorepo/high-parallel environments.

Cons:
- needs ownership metadata;
- boundaries can drift.

### 5. Risk-tiered freshness

Classify movement by risk:

```text
docs-only main movement       -> refresh only
same test files changed       -> retest
same source/runtime changed   -> re-review + retest
schema/security/authority     -> hard block or senior review
```

Pros:
- pragmatic;
- avoids expensive review for harmless movement;
- matches Stores' existing risk-taxonomy direction.

Cons:
- needs a maintained risk classifier;
- early versions must fail conservative.

### 6. Optimistic auto-refresh

Auto-rebase/merge candidate onto latest main. If clean and scope/risk says safe, continue integration. If not, emit typed stale decision.

Pros:
- good high-throughput default;
- keeps humans out of harmless cases.

Cons:
- can feel spooky unless the proof is excellent;
- needs clear audit artifacts.

## Recommended Stores model

Use a typed freshness vector and classifier.

Inputs:

```text
reviewed_base_sha
reviewed_head_sha
tested_base_sha
tested_head_sha
branch_head_sha
current_main_sha
branch_changed_paths
main_changed_paths_since_review
main_changed_paths_since_test
risk_tier / risk labels
ownership_scope (later)
```

Output:

```rust
enum FreshnessDecision {
    Fresh,
    RefreshOnly { scope: Vec<String> },
    RetestRequired { scope: Vec<String> },
    ReReviewRequired { scope: Vec<String> },
    Conflict { paths: Vec<String> },
    StaleBaseHistoryRewrite,
    BranchHeadChanged,
}
```

The classifier should be a pure function first. Integration then consumes it.

## Semantics by case

### Fresh

All reviewed/tested base/head values match current reality.

Action: land.

### Refresh-only stale

Main advanced, but changed scope is disjoint and low-risk.

Action: refresh/rebase and continue. No fresh review required.

### Retest-required stale

Review judgment remains valid, but test/pre-land evidence is stale.

Action: rerun pre-land/test. Do not require human/code review unless tests fail.

### Re-review-required stale

Main changed in overlapping/risky/owned scope, or branch changed after review.

Action: do not land; request fresh review or route to review step. Must release/finalize integration lock and expose next action.

### Conflict

Refresh/rebase/merge cannot apply cleanly.

Action: `integration_blocked` or equivalent typed conflict state with paths/reason.

### Stale base / history rewrite

Reviewed base is no longer reachable from current main.

Action: hard block; fresh review required.

### Branch head changed

Candidate head differs from reviewed head.

Action: fresh review required. This is strict.

## Matrix expectation update

Rename or reinterpret `git-stale-base-refuses`. The current name came from the historical battlescar, but the acceptance condition should be broader and smarter:

```text
must not integrate stale candidate
must not wedge
must release/finalize integration lock
must emit typed freshness decision
must provide next action (refresh/retest/re-review/block)
must assert no real LLM
```

A future row can be more specific:

- `git-main-advance-refresh-only` — disjoint main change, refresh continues.
- `git-main-advance-rereview` — overlapping/risky main change, typed re-review required.
- `git-branch-head-changed` — branch mutated after review, strict re-review.
- `git-history-rewrite-stale-base` — base unreachable, hard block.
- `git-merge-conflict` — conflict decision.

## Implementation path

### Step 1 — classifier only

Add a pure classifier and tests. Do not wire integration first.

Test cases:

- fresh all values match;
- branch head changed after review;
- reviewed base unreachable/history rewrite;
- main advanced disjoint low-risk => refresh-only;
- main advanced overlapping branch scope => re-review;
- main advanced test-only/dependency scope => retest;
- empty/unknown scope fails conservative.

### Step 2 — integration consumes classifier

After refresh and before merge, integration should write a typed decision and exit cleanly when stale evidence invalidates landing.

Required invariant:

```text
no stale case leaves unfinished integrate lock + no next action
```

### Step 3 — matrix RED→GREEN

Update current-mode `git-stale-base-refuses` expectation to typed freshness decision/no wedge. Re-run:

```bash
stores test matrix --mode current --only git-stale-base-refuses \
  --watch --i-understand-this-mutates-current-repo
```

Expected GREEN should show a typed freshness outcome, not necessarily `integration_blocked` if the chosen action is re-review.

### Step 4 — expand battlescar rows

After classifier is in place:

- dirty worktree;
- merge conflict;
- stale external-review head mutation;
- no heartbeat;
- nonzero exit;
- invalid payload;
- duplicate drive.

## Recommendation

Implement the classifier conservatively first:

- unknown/overlap/risky => `ReReviewRequired`;
- clean disjoint low-risk => `RefreshOnly`;
- changed branch head => `BranchHeadChanged`;
- unreachable base => `StaleBaseHistoryRewrite`;
- conflict => `Conflict`.

Do not auto-create fresh reviews yet. First milestone is typed classification + no wedge. Once that is GREEN, add automation for `ReReviewRequired -> create/request fresh ER` as a later phase.

## Follow-ups

- Update matrix catalog names once classifier semantics are chosen.
- Improve current-mode artifacts so stdout proof becomes structured `result.json`/`proof.txt` content.
- Consider ownership/risk metadata after path-overlap classifier is stable.
