# Daily Summary — 2026-05-13

## Overview

The day turned fake-runner work into a live TDD wind tunnel and then pivoted hard into `stores watch` cockpit design. The fake-runner stance became explicit: fabricate real preconditions in the live repo/daemon path, then let Stores produce real consequences. The first battlescar scenario selected was `stale-base-refuses`, clarified as a genuine freshness refusal after fake ER PASS plus real main movement.

The watch work moved through multiple design iterations: semantic row/card vocabulary, readable 3×2 top-card grids with severity coloring, a typed `WatchProjection` seam, dense task visual maps, and lane-specific plans for observations/intake/reviews/engine. The recurring doctrine was the same as fake-runner TDD: show operator truth through real substrate evidence, not raw schema tuples or mocked interpretations.

## Work Completed

- **Live fake-runner scenario discipline captured:** `stores test` should create real synthetic rows, real worktrees, real fake-runner subprocesses, real marker commits, real ER rows, and real integration/freshness outcomes; only LLM text generation is fake.
- **Smoke/battlescar suite plan drafted:** happy path, plan-review reject/recover, code-review revise/recover, failed ER containment, stale freshness, then liveness/duplicate/dirty/conflict/stale-marker cases.
- **Watch semantic UI plan drafted:** translate ADR tuples into operator language; hide raw `status`/`lifecycle`/`active_step`/`runner:none` from row lists; keep debug tuples in detail.
- **Top-card grammar stabilized:** five separate store cards share front/work/gate/exit/wait/fault slots, later rendered as readable 3×2 grids with full labels and Catppuccin-style severity colors.
- **Watch topbar first implementation recorded:** semantic presentation layer, row rendering cleanup, detail reorder, top-card grid, severity styling, and installed binaries; follow-up ambiguity remains in observation counts/row repetition.
- **Declarative projection seam planned:** add typed Rust `WatchProjection` for tasks/observations as a transitional path toward future schema-declared `watch:` metadata, while preserving legacy `Section` internally.
- **Task visual map grammar defined:** one slot per logical phase, few glyph families, superscript cycles, color for active/pass/fail/wait, detail decode for source/confidence.
- **Task map implementation plan reviewed:** widen TUI data model for structured plan review/cycle evidence, build pure `TaskMapProjection`, render aligned `ID | SUMMARY | MAP | REASON | AGE | TIER` tables, then add detail/color.
- **Other store view-map plans reviewed:** observations, intake, mixed review lane, and engine health get lane-specific projections instead of being forced into task phase maps.
- **First battlescar implementation plan drafted:** `stale-base-refuses` should run live, fake-only, advance main with fenced marker commit after fake ER PASS, attempt normal accept/integration, and assert genuine freshness refusal.

## Notes Today

| # | Note | Topic |
|---|------|-------|
| 01 | [live-fake-runner-scenario-tdd-plan.md](./01-live-fake-runner-scenario-tdd-plan.md) | Live fake-runner scenario doctrine, smoke suite, battlescar suite, YAML case shape, artifact proof requirements. |
| 02 | [watch-semantic-ui-ux-rendering-plan.md](./02-watch-semantic-ui-ux-rendering-plan.md) | Semantic `stores watch` cockpit vocabulary, top-card grammar, row/detail mappings, implementation sketch. |
| 03 | [watch-topbar-grid-color-plan.md](./03-watch-topbar-grid-color-plan.md) | Readable top-card grid and severity-color implementation plan. |
| 04 | [watch-topbar-progress.md](./04-watch-topbar-progress.md) | Watch semantic/topbar implementation progress and remaining observation-count/row repetition follow-ups. |
| 05 | [watch-declarative-projection-plan.md](./05-watch-declarative-projection-plan.md) | `WatchProjection` seam for task/observation top-card counts, display groups, and row text. |
| 05a | [watch-truthfulness-gap-report.html](./05-watch-truthfulness-gap-report.html) | Visual report artifact for watch truthfulness gaps. |
| 06 | [watch-visual-state-grammar.md](./06-watch-visual-state-grammar.md) | Dense visual grammar: shape family, position, fill, superscript, color, animation. |
| 06a | [watch-row-density-design-spec.html](./06-watch-row-density-design-spec.html) | Visual report artifact for watch row density/design. |
| 07 | [watch-task-map-implementation-plan.md](./07-watch-task-map-implementation-plan.md) | Task map data/projection/render/detail implementation plan and guardrails. |
| 08 | [watch-other-store-view-map-plans.md](./08-watch-other-store-view-map-plans.md) | Observation/intake/review/engine focused-lane map plans. |
| 09 | [live-fake-stale-base-tdd-plan.md](./09-live-fake-stale-base-tdd-plan.md) | Concrete `stale-base-refuses` live fake scenario plan and acceptance checklist. |

## Tensions

- **Readable cockpit vs dense cockpit:** Blake wants high information density, but not compressed codes; the design response is full-word top-card grids and dense visual maps with detail decode.
- **Projection vs schema rewrite:** watch needs semantic truth now, but legacy `status`/`Section` remain internal compatibility. The typed projection seam avoids pretending the schema is fixed.
- **Observation double-counting:** `candidates 8` and `waiting 8` appearing together exposed that top-card slots must be mutually exclusive/operator-meaningful, not independent raw predicates.
- **Fake test evidence vs repo cleanliness:** real marker commits/worktrees are accepted because they are the substrate truth surface, but they must be fenced, printed, and auditable.
- **Exact stale label vs genuine freshness refusal:** additive main movement may surface as `stale_external_review`, not literal `stale_base`; the scenario should assert the real canonical refusal.

## Open Threads

- Implement `stale-base-refuses` as the first live fake battlescar scenario and keep refused rows visible in `stores watch`.
- Build the smoke suite: happy-path integrates, plan-review reject/recover, code-review revise/recover, failed-ER containment, stale freshness refusal.
- Fix observation top-card projection so candidate/waiting counts are mutually exclusive and not cognitively duplicated.
- Redesign observation focused rows to one state, one signal, one summary; raw contract/status/debug fields belong in detail.
- Implement `WatchProjection` phases for tasks and observations, then task visual maps after widening structured TUI evidence.
- Carry the dense map/projection approach to intake, review, and engine only with lane-specific source enums and detail decoders.

## Tomorrow

- Start with fake-runner wind-tunnel TDD: implement/run `stale-base-refuses` red/green through the live repo path.
- Then continue watch cockpit convergence: observation projection/counts first, task map data/projection second, rendering after source/confidence tests are locked.
