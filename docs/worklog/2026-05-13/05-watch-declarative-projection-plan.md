# Watch Declarative Projection Plan

**Date:** 2026-05-13
**Type:** note

## Summary

Move `stores watch` toward schema-declared/operator-truth projection in a safe transitional step: add a typed Rust `WatchProjection` seam for tasks and observations so top cards, focused display groups, and row state use one grammar, while legacy `Section` remains an internal compatibility/routing layer for now.

Oracle reviewed the first draft and tightened the plan: do **not** rewrite or rename the legacy `Section` taxonomy directly. It is used by filtering, focus, collapse state, mission compact rendering, and tests. Instead, add projection-based **display groups** for cockpit rendering. This gives the user-visible alignment without pretending the schema/frontend architecture is fully solved in one pass.

## Target outcome

For tasks and observations first:

- one projection function classifies each row into a watch slot;
- top-card counts use that projection;
- focused cockpit sections are display groups derived from that projection;
- row text suppresses broad state already communicated by the display group;
- subtype/stage/signal remains visible;
- raw schema/debug truth remains in detail panes;
- the code clearly marks this projection as a transitional seam toward future schema `watch:` metadata.

## Non-goals

- Do not migrate the database schema.
- Do not remove legacy `status` or legacy `Section` buckets.
- Do not implement YAML `watch:` parsing in this task.
- Do not attempt to make intake, external reviews, or engine fully generic yet.
- Do not break the readable top-card grid, responsive fallback, or Catppuccin severity colors.
- Do not hide subtype information such as `runner failed`, `capacity`, `plan-gate`, or `contract draft`.

## Design principle

Legacy `Section` remains an internal compatibility taxonomy. Cockpit display groups become the user-facing taxonomy.

```text
raw row fields -> WatchProjection -> top card counts
                            \-> focused display groups
                            \-> context-aware row text
                            \-> future schema watch: metadata shape
```

## Phase 1 — Add task `WatchProjection`; route task top-card counts through it

### Goal

Create a typed transitional projection model in the TUI semantics layer and make task top-card counts consume it. Do not change focused table sections yet.

### Likely files

- `src/tui/semantics.rs`
- `src/tui/data.rs`
- tests in `tui::semantics::tests`, `tui::data::tests`, `tests/tui_watch_semantic_regression.rs`

### Implementation shape

Add projection primitives near existing presentation helpers:

```rust
pub enum WatchSlotId { Front, Work, Gate, Exit, Wait, Fault }
pub enum WatchAttention { Exhaust, Flow, Fault, Neutral }

pub struct WatchProjection {
    pub slot: WatchSlotId,
    pub slot_label: &'static str,
    pub glyph: &'static str,
    pub row_stage: &'static str,
    pub row_signal: Option<String>,
    pub next_action: Option<&'static str>,
    pub attention: WatchAttention,
}
```

Add:

```rust
pub fn task_watch_projection(task: &TaskRow) -> WatchProjection
```

Task precedence:

1. terminal/done -> `Exit / done`;
2. blocked -> `Wait` or `Fault` by blocker kind;
3. integration -> `Gate` with row stage `ship` for this pass;
4. active planning/coding -> `Work` with row stage `plan` or `exec`;
5. active planning_review/coding_review/wrapping -> `Gate` with row stage `plan-gate`, `code-gate`, or `accept`;
6. queued -> `Front / queued`.

Update `apply_task_to_flow` to use `task_watch_projection`, eliminating an independent task slot classifier.

### Acceptance

- `apply_task_to_flow` no longer independently implements task slot classification.
- Task top-card counts remain correct for queued/work/gate/wait/fault/done representative rows.
- Focused task table labels remain unchanged in this phase.
- Tests cover projection of queued, plan, exec, plan-gate, accept, capacity wait, runner fault, and done.
- Targeted tests pass:
  - `cargo test -q tui::semantics::tests:: --lib`
  - `cargo test -q tui::data::tests::store_flow_model_counts_mixed_rows_per_lane --lib`
  - `cargo test --test tui_watch_semantic_regression`

## Phase 2 — Task display groups and context-aware task rows

### Goal

Make the task-focused cockpit view use the same grammar as the task top card without replacing legacy `Section` internals.

### Likely files

- `src/tui/render.rs`
- `src/tui/semantics.rs`
- tests in `src/tui/render.rs`, `tests/tui_watch_cockpit.rs`, `tests/tui_watch_semantic_regression.rs`

### Implementation shape

Keep `App.sections: Vec<(Section, Vec<usize>)>` unchanged. In `draw_focused_table`, when `focused_store == Tasks`, derive projection display groups from visible task rows:

```text
QUEUED
WORKING
GATE
WAITING
FAILED
DONE only if terminal rows are already included in that area
```

Rows should be formatted with parent display slot context:

```rust
format_task_line_with_context(task, external_review, Some(parent_slot))
```

Suppression rule:

- Under `QUEUED`, do not repeat `queued`; show `no worktree` if relevant.
- Under `WORKING`, show finer stage `plan` or `exec`.
- Under `GATE`, show finer stage `plan-gate`, `code-gate`, `accept`, or `ship`.
- Under `WAITING`, show subtype such as `capacity`, `dependency`, `rate limit` rather than `waiting-capacity`.
- Under `FAILED`, show subtype such as `runner failed`, `review blocked`, `tests failed`, not generic `failed`.

### Acceptance

- Task-focused cockpit section headers are projection display groups: `QUEUED`, `WORKING`, `GATE`, `WAITING`, `FAILED`.
- Old user-facing task cockpit labels such as `ACTIVE`, `AWAITING HUMAN ACCEPTANCE`, and `HELD-TRIAGE` do not appear in task-focused cockpit output.
- Display group counts match task top-card counts for the rows included in the focused table.
- Rows do not repeat broad section state but preserve subtype/stage/signal.
- Legacy `Section` remains available for filters/navigation/internal compatibility.
- Rendered-buffer tests cover representative task rows and row-state suppression.

## Phase 3 — Observation projection and display groups

### Goal

Apply the same projection/display-group pattern to observations while preserving Option A mutual exclusivity.

### Likely files

- `src/tui/semantics.rs`
- `src/tui/data.rs`
- `src/tui/render.rs`
- tests in data/render/semantic regression

### Implementation shape

Add:

```rust
pub fn observation_watch_projection(row: &ObsRow) -> WatchProjection
```

Observation precedence:

1. closed/superseded/wont-fix -> `Exit / closed`;
2. architecture gate, human ratification, contract draft/approved/ready -> `Gate / contract gate`;
3. non-contract waiting_kind -> `Wait / waiting`;
4. in_progress/investigating -> `Work / investigate`;
5. candidate/open -> `Front / candidates`;
6. investigation_failed or explicit fault -> `Fault / errors` where available.

Use the projection for observation top-card counts and observation-focused display groups:

```text
CANDIDATES
INVESTIGATE
CONTRACT GATE
WAITING
ERRORS
CLOSED only if closed rows are shown in that area
```

Handle `Row::CollapsedObs` explicitly by using the representative row projection.

Rows should suppress broad section state while preserving finer stage/next action:

- under `CONTRACT GATE`: show `draft`, `approved`, or `architecture`, plus next action;
- under `WAITING`: show `info needed`, `external dependency`, etc.;
- under `CANDIDATES`: omit repeated `candidate`, show priority/next triage/summary;
- no raw `contract:`, `tier:`, `lifecycle=`, `waiting_kind=` in list rows.

### Acceptance

- Observation top-card and focused display groups use identical projection buckets/counts.
- Option A holds: each row counts in one visible bucket.
- Rows do not repeat broad section state or raw schema vocabulary.
- Collapsed observation clusters are explicitly tested.
- Detail/debug still exposes raw fields.

## Phase 4 — Document the seam and guard against drift

### Goal

Make it clear that the typed Rust projection is a transitional schema seam and add guard tests so future work does not drift back into independent vocabularies.

### Likely files

- `src/tui/semantics.rs` comments/module docs
- `docs/worklog/2026-05-13/05-watch-declarative-projection-plan.md` final notes if needed
- regression tests

### Acceptance

- Code comments state that `WatchProjection` mirrors the future schema `watch:` block.
- Guard tests prove task top-card labels and task display section labels derive from projection vocabulary.
- Guard tests prove observation top-card labels and observation display section labels derive from projection vocabulary.
- No old task display labels (`ACTIVE`, `AWAITING HUMAN ACCEPTANCE`, `HELD-TRIAGE`) appear in task-focused cockpit output.
- No old observation display labels (`PRIORITY`, generic `OBSERVATIONS`) appear in observation-focused cockpit output after Phase 3.

## Final success criteria

- Tasks and observations no longer have independent top-card/section/row grammars in the cockpit.
- Legacy internal `Section` is preserved but no longer dictates user-facing task/observation display groups.
- The typed projection is centralized and ready to be moved into schema metadata later.
- Tests lock the alignment so future UI changes cannot silently reintroduce divergent vocabulary.
