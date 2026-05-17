# Oracle architectural review: dense visual task-map grammar

Gate: **WARN / CONDITIONAL PASS**

The design direction is architecturally sound **only if implemented as a truth-preserving projection over ADR 0001 fields and append-only evidence**, not as a renderer that invents lifecycle facts. The glyph grammar can reinforce ADR 0001 by making `lifecycle`, `active_step`, `current_phase`, `current_cycle`, `plan.phases`, `plan_review_log`, and `cycles` legible. It becomes a distortion if it renders historical pass/fail colors or cycle counts that are not backed by persisted structured data.

The most important constraint: **one visual slot per logical phase is correct, but every fill/color/superscript must have a named source field/event and an explicit unknown fallback.**

## Key inherited decisions to preserve

- ADR 0001 primary task truth is the tuple: `lifecycle`, `active_step`, `integration_step`, `blocked`, `blocker_kind`, plus phase/cycle fields.
- Legacy `status` remains compatibility/debug, not the semantic source of truth.
- `WatchProjection` is a transitional Rust-owned seam mirroring future schema `watch:` metadata; it should prevent independent frontend vocabularies, not become another hidden lifecycle.
- Raw/debug fields remain available in detail panes.
- The task map design says: few shapes, dense dimensions; one visual slot per logical phase; color/animation are secondary dimensions.

## Supported / gap matrix

| Visual claim | Can it be honestly derived now? | Current proof source | Current code access | Risk / gap |
|---|---:|---|---|---|
| Queued / pre-planning `◌` | Yes | `lifecycle=queued`, `active_step=none`, `blocked=false`; activation may add nuance | `TaskRow.lifecycle`, `active_step`, `blocked`, `activation` | Good. Do not use legacy `status=planning` as truth. |
| Currently planning `○` | Yes | `lifecycle=active`, `active_step=planning`, `blocked=false` | `task_lifecycle`, `task_active_step` | Good. Planning cycle count is not explicitly separate from plan review count; see below. |
| Currently in plan review `●` | Yes | `lifecycle=active`, `active_step=planning_review`, `blocked=false` | `task_active_step` | Good for current state. |
| Plan review cycle superscript `●²` / planning cycle superscript `○²` | Mostly, with care | `plan_review_log.length` records review attempts; `NEEDS_WORK` sends back to planning while length < 5 | Schema has structured `plan_review_log.gate`; current `TaskRow` only loads `plan_review_summaries`, not gates/count explicitly | Need parse/count structured plan review entries. Do not infer from `current_cycle` (that is execution phase cycle). |
| Plan review passed color green | Conditionally | Latest `plan_review_log.gate == READY`, or transition `plan_review -> ready` / current state past planning | Schema has `plan_review_log.gate`; transition_history has events | Current loader discards gate and only keeps summaries. Need structured gate loading or transition proof. T1 `skip-plan` is not a passed review; color should be distinct/neutral/synthesized, not green pass. |
| Plan review failed color red | Conditionally | Latest `plan_review_log.gate == NOT_READY`, or blocked with `blocker_kind=task_review` from plan_review | `blocker_kind=task_review`; transition_history `from_status=plan_review`; plan_review_log.gate | Current blocked row loses `active_step`; `blocker_kind=task_review` alone cannot distinguish plan review vs code review. Need transition_history/from_status or structured blocked_reason. |
| Number of execution phases as dots `· · ·` | Yes if plan exists | `plan.phases.length`; T054 says T1 synthesized one-phase plan | SQL computes `total_phases = json_array_length(plan.phases)` | Good. If plan missing/null, render unknown (`?`) not guessed dots. |
| Current execution phase position `□` | Yes | `lifecycle=active`, `active_step=coding`, `current_phase`, `total_phases` | `TaskRow.current_phase`, `total_phases` | Good if fields are present and in range. Unknown fallback if absent. |
| Current code review phase position `▣` | Yes | `lifecycle=active`, `active_step=coding_review`, `current_phase`, `total_phases` | Same | Good for current state. |
| Execution/review cycle superscript `□²`, `▣²` | Yes for current phase | `current_cycle` auto-increments within `current_phase`; `REVISE` increments it | `TaskRow.current_cycle` | Good for current active phase. Do not use it for planning cycles. |
| Previous phase passed color green | Mostly | Transition semantics: code_review PASS non-last increments `current_phase`; `cycles[].review.gate == PASS` records result | Schema `cycles` structured records include phase/cycle/review.gate | Current loader reduces `cycles` to text summaries, losing structured gate/phase/cycle. Need parse structured cycles. Without it, can use `i < current_phase` as implied pass, but mark as derived/implied. |
| Previous phase failed/revised history color red/orange | Partially | `cycles[].review.gate` includes PASS/REVISE/FAIL | Schema supports it | Current loader discards structured gate. Also final blocked rows may need transition history to locate failure position. |
| Review failed and executing phase N cycle M | Yes for current retry | `active_step=coding`, `current_phase=N`, `current_cycle=M>1`; previous `cycles` has REVISE | Current fields give current retry; cycles prove cause | Rendering `□²` is honest. Rendering a red failed review marker separately would violate one-slot rule unless sourced from cycles/detail. |
| Code review active/pass/fail color | Active: yes. Pass/fail: conditional | Active from `active_step=coding_review`; pass/fail from `cycles[].review.gate` / transition | Current active fields loaded; structured cycles not loaded | Need structured cycles before coloring historical review results. |
| Wrap / acceptance `▰` | Yes for current wrapping | `lifecycle=active`, `active_step=wrapping`; statuses complete/in_review/accepted have transitions with wrapping | `TaskRow.active_step`, external review state | Good, but acceptance vs external-review vs wrap are distinct valves; detail should disambiguate. |
| Integration / ship | Yes, but outside proposed task work map | `lifecycle=integration`, `integration_step` | `TaskRow.integration_step` | Do not overload `▰` for integration unless separately specified. |
| Waiting/capacity `△` | Yes | `blocked=true`, `blocker_kind=capacity/dependency/rate_limit/stale_base/etc.` | `TaskRow.blocked`, `blocker_kind` | Good. Waiting is pressure outside normal phase map. |
| Runner/config/test/deploy fault `▲` | Yes for kind; position often no | `blocked=true`, `blocker_kind=runner/config/test_failure/...`; blocked_reason JSON | Current presentation parses exit code | For runner failure from planning/executing/review, current row loses interrupted `active_step`; use transition_history `from_status` if positioning fault in map. Otherwise render reason column only. |
| Breathing active animation | Yes if bound to current active/claimed state | `active_step` active current cell; optionally `claimed_by`, `claimed_at`, `drive_pid`, `live_run` | Current fields include `claimed_by`, `claimed_at`, `drive_pid`, `live_run` | Must be purely presentational. Do not imply liveness unless sourced from live_run/heartbeat; active_step alone means current logical state, not necessarily live process. |

## Main architectural diagnosis

### What is safe

The grammar itself does not inherently violate ADR 0001. In fact, it can make ADR 0001 more legible if the map is implemented as a **pure projection**:

```text
ADR 0001 tuple + structured plan/review/cycle logs -> TaskMapProjection -> renderer
```

The strong parts already align with schema truth:

- circle family maps to planning/planning_review (`active_step`);
- square family maps to coding/coding_review (`active_step` + `current_phase`);
- superscript for execution cycles maps to `current_cycle`;
- phase dots map to `plan.phases.length`;
- wait/fault triangles map to `blocked` + `blocker_kind`.

### What is unsafe

The dangerous parts are **historical result colors** and **failure placement**:

- Green `●` for plan review passed requires structured proof (`plan_review_log.gate == READY` or transition proof), and current `TaskRow` does not load the gate.
- Green/red `▣` for previous code review result requires structured `cycles[].review.gate`; current loader flattens cycles into summaries.
- A blocked row with `blocker_kind=task_review` does not by itself say whether plan review or code review failed. ADR transitions set `active_step=none` on blocked. You need `transition_history.from_status` or structured blocked_reason to place the failure honestly.
- Runner failures can occur from planning, ready, executing, or code_review, but `mark_drive_failed` also sets `active_step=none`. Again, use transition_history/from_status if you want to show where it failed.

Therefore: **do not implement pass/fail color for historical plan/phase review slots until the loader carries structured plan_review/cycle evidence.** A neutral/implied state is acceptable as an interim.

## Overfitting assessment

Current risk: **medium**.

The design emerged from real UI pain, not just clean-seeded rows, but the examples are ahead of the currently loaded data model. The schema is richer than the current TUI row structs: `plan_review_log` and `cycles` contain exactly the sort of gates needed, but `src/tui/data.rs` currently discards them into `Vec<String>` summaries. If implementation proceeds by inferring colors from status strings or clean fixture titles, it will overfit and distort.

If implementation first upgrades data loading to structured review/cycle records, the risk drops substantially.

## Where this belongs

This should be part of the `WatchProjection` seam, but probably as a task-specific extension:

```rust
WatchProjection {
    slot: ...,
    row_stage: ...,
    task_map: Option<TaskMapProjection>,
}

TaskMapProjection {
    planning_slot: MapCell,
    phase_slots: Vec<MapCell>,
    wrap_slot: Option<MapCell>,
    reason: Option<String>,
    confidence: Exact | Implied | Unknown,
}
```

It should **not** be ad hoc renderer logic. The renderer should only draw `TaskMapProjection`. Eventually this maps naturally to schema-declared `watch:` metadata, but the phase/cycle map is sufficiently task-specific that it can remain a typed Rust projection until the schema metadata format is real.

## Guardrails required before implementation

1. **Source annotation per cell.** Every glyph/color/superscript must be backed by a source: `active_step`, `current_phase`, `current_cycle`, `plan.phases.length`, `plan_review_log.gate`, `cycles[].review.gate`, `transition_history.from_status`, or `blocker_kind`.
2. **Unknown beats guessed.** If `total_phases`, `current_phase`, `current_cycle`, plan review gates, or cycles are absent, render `?` / muted fallback / reason text; never invent phase dots or green pass states.
3. **Do not use legacy status as primary truth.** It can be compatibility fallback only where ADR tuple fields are absent, and such rows should be visually marked degraded/legacy in detail.
4. **Parse structured logs.** Extend `TaskRow` to carry structured `plan_review_log` gates and structured `cycles` entries. Summaries alone are insufficient for this design.
5. **Treat implied passes differently from proven passes.** `i < current_phase` implies previous phase passed via ADR transition semantics, but `cycles[].review.gate == PASS` is stronger proof. Consider dim green for implied, normal green for proven, or record confidence in detail.
6. **T1 skip-plan is not plan-review passed.** A synthesized one-phase plan should not show green plan-review pass unless a review actually happened. Use a neutral/synthesized state or detail annotation.
7. **Blocked positioning requires transition evidence.** For `task_review` and `runner` blockers, only place `▲` inside a planning/phase slot if transition history or structured reason proves the interrupted state/phase. Otherwise show `▲` as state plus reason column.
8. **Color is secondary, not sole truth.** Detail pane must spell out active/pass/fail/cycle, and tests should verify monochrome text/debug truth exists.
9. **Animation is liveness, not lifecycle.** Breathing may mark current logical cell; process liveness breathing should require `live_run`/heartbeat/claimed metadata. Do not animate historical cells.
10. **Golden mapping tests.** Add table-driven tests from schema-shaped rows to `TaskMapProjection`, not only rendered-buffer snapshots.

## Recommended implementation order

1. **Audit and extend data model first.** Add structured `PlanReviewEntry { gate, at, summary }` and `CycleEntry { phase, cycle, review_gate, executor_at, review_at, ... }` to `TaskRow`; keep existing summary vectors if needed for details.
2. **Build pure `TaskMapProjection`.** Implement in `semantics.rs` or a new `tui::task_map` module. It should return cells with glyph family, superscript, color role, source/confidence, and reason.
3. **Test projection against schema transitions.** Include fixtures for: queued, planning, plan_review NEEDS_WORK cycle, plan READY, T1 skip-plan, executing phase N cycle M, code_review, REVISE retry, PASS previous phases, task_review blocked from plan_review, task_review blocked from code_review, runner failed from executing, wrapping/in_review, integration.
4. **Render conservatively.** Initially render exact/current states and phase dots; use neutral/dim for historical slots where proof is only implied. Add green/red only after structured proof is available.
5. **Update detail pane.** For selected task, show an explicit decoded map legend: planning cycle, plan review latest gate, phase/cycle results, data sources/confidence.
6. **Only then add breathing color.** Make it subtle and optional; test that inactive/completed cells do not animate.
7. **Future step: schema metadata.** Once stable, migrate the projection contract toward `watch:` metadata. Do not block the immediate task-map renderer on YAML expression support.

## Final recommendation

Proceed, but with a **WARN gate**: implement the visual grammar only through a pure projection with structured data support and explicit unknown/implied handling. The design can reinforce ADR 0001; the architectural failure mode is not the glyphs, it is pretending that color/superscript history is known when the current TUI loader has thrown away the structured evidence.
