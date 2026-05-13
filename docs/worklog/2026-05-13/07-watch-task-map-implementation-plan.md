# Watch Task Map Implementation Plan

**Date:** 2026-05-13
**Type:** note

## Summary

Implementation plan for the dense task visual map in `stores watch`: render task progress as one slot per logical phase using few shapes densely, while preserving ADR 0001/schema truth. Reviewer pushed the draft hard and confirmed the key point: the information is mostly accessible, but only if we first widen the TUI data model and wire the map through the existing `WatchProjection`/focused-render path instead of adding a parallel frontend interpretation.

## Design target

Task focused rows should become an aligned btop-like table:

```text
ID     SUMMARY                                             MAP          REASON   AGE  TIER
T001   synthetic queued inactive plan task                  ◌ │ · · ·             7h   T3
T002   synthetic active planning task                       ○ │ · · ·             7h   T3
T003   synthetic task paused in plan review                 ● │ · · ·             7h   T3
T004   synthetic active coding task                         ● │ □ · ·             7h   T3
T011   retrying phase two after review                      ● │ ▣ □² ·            2h   T3
T012   phase two back in review                             ● │ ▣ ▣² ·            2h   T3
T009   stores test live happy-path                          ● │ ▣ ▣ ▰             7h   T3
T007   synthetic observation-linked capacity wait           △                    7h   T2
T010   synthetic fake runner nonzero blocked task           ▲           runner   7h   T3
```

Important correction from review: durable execution is not represented by a stable `ready` visual state. The map’s execution slot is sourced from ADR fields such as `lifecycle=active`, `active_step=coding`, `current_phase`, and `current_cycle`. Legacy/status-only `ready` is compatibility fallback/implied only.

## Visual grammar

```text
shape       = workflow family
position    = phase index / pipeline position
fill        = substate inside the family
superscript = cycle/attempt count
color       = active / passed / failed / waiting
animation   = currently active cell, later and subtle
```

One logical phase gets one visual slot. Do not show both planning and plan-review for the planning phase; a filled circle with superscript already implies the planning attempts that preceded it.

```text
◌  queued / pre-planning / not started
○  planning, still changing the plan
●  plan review / plan result
·  planned execution phase exists but unreached
□  executing this phase
▣  code review / phase result
▰  wrap / acceptance valve
△  waiting / non-failure pressure outside normal map
▲  fault / failed outside normal map unless exact phase placement is proven
```

Superscripts:

- plan cell superscript = plan review attempt count / planning round, sourced from `plan_review_log.length` where available;
- execution/review cell superscript = `current_cycle` for current active phase, or latest structured `cycles[].cycle` for historical phase cell.

## Current architecture seam

Implementation must extend the existing path, not add a parallel renderer:

```text
src/tui/data.rs      loads structured evidence into TaskRow
src/tui/semantics.rs owns pure WatchProjection + TaskMapProjection
src/tui/render.rs    focused task path consumes projection cells
src/tui/detail.rs    decodes selected map and source/confidence
```

Existing `WatchProjection` remains the broad slot/grouping seam. The task map is a task-specific extension on that projection. Legacy `Section` remains internal compatibility for filters/navigation.

`src/cli/watch.rs` legacy/ANSI output is not the primary target unless a later task explicitly asks for parity.

## Is the risky information accessible?

Mostly yes, but not yet loaded in structured form.

Schema supports:

- `plan_review_log[]` with structured `gate: READY | NEEDS_WORK | NOT_READY`, summary, timestamp.
- `cycles[]` with structured `phase`, `cycle`, executor summary/commit, review gate `PASS | REVISE | FAIL`, review findings, timestamp.
- `current_phase`, `current_cycle` for current execution/review position.
- `plan.phases.length`, already loaded as `total_phases`.
- `transition_history` with `from_status`, `to_status`, `verb`, `occurred_at` for interruption provenance when queried deliberately.

Current TUI gap:

- `TaskRow` only keeps `plan_review_summaries: Vec<String>` and `cycle_summaries: Vec<String>`, flattening away structured gates/phase/cycle proof.
- `TaskRow.recent_events` only loads the newest five transition events; that is not enough to place old blocked failures inside a phase map.

Therefore Phase 1 must load structured evidence before historical green/red colors are allowed.

## Phase 1 — Load structured task evidence into TUI rows

### Goal

Make the TUI able to access the information needed to render the map honestly.

### Work

Add typed serde-backed structs in `src/tui/data.rs` or a nearby module:

```rust
TaskPlanReviewEntry {
  gate: PlanReviewGate, // READY | NEEDS_WORK | NOT_READY | Unknown(String)
  summary: Option<String>,
  at: Option<String>,
}

TaskCycleEntry {
  phase: i64,
  cycle: i64,
  executor_summary: Option<String>,
  executor_at: Option<String>,
  review_gate: Option<CycleReviewGate>, // PASS | REVISE | FAIL | Unknown(String)
  review_summary: Option<String>,
  review_at: Option<String>,
}
```

Extend `TaskRow` with:

```rust
plan_review_entries: Vec<TaskPlanReviewEntry>
cycle_entries: Vec<TaskCycleEntry>
```

Parsing requirements:

- tolerate missing optional fields;
- tolerate `review: null`;
- malformed JSON becomes empty/unknown, not panic;
- keep existing summary vectors if needed for detail compatibility;
- maintain compatibility with old tests that create `TaskRow::default()`.

### Acceptance

- Data tests prove `plan_review_log` gates and `cycles` phase/cycle/review gates load from JSON.
- Missing/malformed JSON degrades to empty/unknown.
- Existing details/tests still pass.
- No visual map rendering yet except internal projection tests if useful.

## Phase 2 — Build pure TaskMapProjection

### Goal

Create a pure schema-truth projection before rendering.

### Shape

Add in `src/tui/semantics.rs` or new `src/tui/task_map.rs`:

```rust
TaskMapProjection {
  planning: MapCell,
  phases: Vec<MapCell>,
  wrap: Option<MapCell>,
  reason: Option<String>,
  confidence: MapConfidence,
}

MapCell {
  glyph: MapGlyph,
  cycle: Option<i64>,
  color_role: MapColor,
  active: bool,
  source: MapSource,
  confidence: Exact | Implied | Unknown,
}
```

### Plan cell rules

- `◌` queued/pre-planning from `lifecycle=queued` or no plan/progress.
- `○` planning from `active_step=planning`.
- `●` active plan review from `active_step=planning_review`.
- `●` passed plan review only with proof:
  - latest `plan_review_log.gate == READY`, or exact transition proof;
  - T1 `contract_synthesized` / skip-plan is not a green plan-review pass; render neutral/implied.
- `●` failed plan review only with proof:
  - latest `plan_review_log.gate == NOT_READY`, or exact blocked/transition proof.
- Superscript derives from `plan_review_entries.len()` / attempt count, not from `current_cycle`.

### Execution phase rules

- Phase dots come from `total_phases`; if absent, render unknown (`?`) not guessed dots.
- `□` current executing phase from `active_step=coding`, `current_phase`, `current_cycle`.
- `▣` current code review from `active_step=coding_review`, `current_phase`, `current_cycle`.
- Historical phase cells group `cycles[]` by `phase`:
  - choose latest/max `cycle` for each phase;
  - if latest review gate is `PASS`, render green `▣` with that cycle superscript if >1;
  - if latest review gate is `REVISE` and the same phase is currently executing, current cell is `□<cycle>` active; do not render a misleading passed `▣`;
  - if latest review gate is `FAIL`, render red `▣` only if this is exact structured proof;
  - earlier REVISE followed by later PASS should render passed for the latest cycle.
- Current `current_cycle` applies to execution/review phase cycle, not planning.

### Blocked / fault rules

- `△`/`▲` outside the normal map is acceptable for waiting/fault when exact placement is not known.
- Placing `▲` inside a specific phase requires a dedicated proof source such as transition history from-status/current phase at interruption or structured blocked reason.
- Do not rely on the five newest `recent_events` to place old failures.

### Acceptance

Table-driven tests cover:

- queued before plan;
- planning cycle 1;
- plan review current;
- plan review attempt 3 from `plan_review_log.length`;
- plan review passed from `READY`;
- plan review failed from `NOT_READY`;
- T1 `contract_synthesized` does not falsely show green plan-review pass;
- executing phase N cycle M;
- code review phase N cycle M;
- previous phase pass after earlier revise;
- current executing cycle 2 after REVISE;
- current code review cycle 2;
- FAIL/blocked;
- runner/review blocked fallback with reason when interrupted position cannot be proven;
- wrap/acceptance.

## Phase 3 — Render aligned task table with MAP column

### Goal

Replace prose-heavy task rows with an aligned btop-like table in the existing focused task path.

### Columns

```text
ID | SUMMARY | MAP | REASON | AGE | TIER
```

Rendering requirements:

- add a visible header row aligned to the same x positions as row cells;
- `SUMMARY` is `TaskRow.title`, first-class, and receives remaining width after fixed columns;
- `MAP` renders `TaskMapProjection` using glyphs, superscripts, and color roles;
- `REASON` is bounded and only for wait/fault/blocking reason (`capacity`, `runner`, `review`, etc.);
- `AGE` and `TIER` are fixed/right-sized columns;
- raw JSON, paths, `workspace:none`, and debug tuples stay in detail;
- projection group headers may remain, but row columns must stay aligned under them.

### Acceptance

- Rendered-buffer test for the exact sample shape or equivalent representative rows.
- Column headings align exactly with row values.
- Rows no longer contain prose bags like `not scaffolded age:7h tier:T3 · workspace:none`.
- The map preserves the number of planned phases as dots.
- Superscripts render for cycle > 1.
- Summary remains visible and first-class.
- Existing up/down navigation stays aligned with visible rows.

## Phase 4 — Detail decode and color

### Goal

Make the visual map legible in detail and apply colors safely.

### Detail work

For selected task, detail pane should decode:

- planning state and attempt count;
- latest plan review gate and source;
- each phase cell state/cycle/gate/source/confidence;
- reason/fault source;
- unknown/implied fields explicitly.

### Color work

- Map color roles map to ratatui `Style`, not embedded strings.
- Active work: blue/teal.
- Active review/gate: peach/yellow.
- Passed/completed: green.
- Failed: red.
- Waiting: yellow/peach.
- Unknown/inactive: dim.
- Breathing animation is optional and should wait until static rendering is correct.

### Acceptance

- Rendered-buffer style tests verify representative colors.
- Detail text decodes map state in monochrome.
- No animation of historical/completed cells.

## Guardrails

- Every glyph/color/superscript must name a persisted source field/event.
- Unknown beats guessed.
- Legacy `status` is compatibility fallback only.
- Historical green/red review result requires structured proof from `plan_review_log` or `cycles`.
- Blocked failure placement inside the map requires transition evidence; otherwise render `▲` with reason outside phase placement.
- Projection tests come before renderer snapshots.

## Follow-ups

- Once stable, move this projection contract toward schema-declared `watch:` metadata.
- Apply the same few-shapes/dense-dimensions principle to observations/intake/reviews without forcing task-specific phase concepts onto them.
