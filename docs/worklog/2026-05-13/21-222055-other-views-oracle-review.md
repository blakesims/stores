Inherited decisions:
- The task-map design is not “decorate rows with icons”; it is a source-backed projection pipeline: load structured evidence in `data.rs`, derive pure semantics in `semantics.rs`, render aligned focused tables in `render.rs`, and decode source/confidence in `detail.rs`.
- `WatchProjection` remains the broad slot/grouping seam; lane-specific maps are extensions under it. `Section` stays compatibility/navigation internals.
- Task phases are task-specific. Other lanes should get lane-native visual projections; unknown/implied evidence must not become green/red truth.
- Rows should become dense aligned tables with first-class summaries and bounded action/provenance columns; raw JSON, paths, debug tuples, and prose bags belong in detail.

Diagnosis:
- The plan mostly preserves the design intention. It explicitly rejects forcing task phases onto other lanes and correctly proposes lane-specific projections plus data/render/detail work.
- The biggest hardening need is to make the projection primitives concrete. Current `MapCell`, `MapGlyph`, and `MapSource` are task-map shaped (`Queued`, `Planning`, `PlanReview`, `Cycles`, etc.). Several proposed plans reuse `MapCell` while requiring glyphs/sources it cannot represent (`✓`, `×`, `■`, `◆`, `↻`, `◈`, lane-specific sources). That would either contort the task enum or silently erase source specificity.
- The current architecture already has partial projection sorting/rendering only for tasks and observations. Intake/external reviews/engine will need explicit app/render integration, not just new semantic structs.

Drift / contradiction check:
1. **Projection primitive drift.** The plan says “do not over-abstract” but then uses `MapCell` directly for intake, external reviews, and engine. That conflicts with the current code: `MapCell` is coupled to task `MapGlyph`/`MapSource`. Revise to either:
   - create lane-specific cell types for all lanes (`ObservationFlowCell`, `IntakeFunnelCell`, `ExternalReviewCell`, `EngineCheckCell`), sharing only `MapColor`/`MapConfidence` and render helper concepts; or
   - introduce a neutral `VisualCell { glyph: &'static str, count: Option<i64>, color_role, active, source_label/source enum, confidence }` and keep lane-specific source enums outside it.
   Do not extend task `MapGlyph` into a universal dumping ground.

2. **Observation flow positions are inconsistent.** The prose says fixed checkpoints are `candidate │ evidence │ contract │ arch? │ resolution`, but examples use four tokens where the first token appears to mean “evidence/candidate” (`◌ · · ·`, then `● ▣ · ·`). If candidate and evidence are separate checkpoints, later rows are missing a candidate-passed marker. If they are merged, the checkpoint list is wrong. Recommended revision: define observation flow as `signal/evidence │ contract │ arch? │ resolution`, where `◌` means raw/candidate not yet investigated and `●` means investigation/evidence active or complete. This matches the examples and avoids an extra low-value cell.

3. **Observation “evidence gathered” proof is under-specified.** `ObsRow` loads `evidence_pointers`, `lifecycle`, and recent events, but not a structured investigation log. Rendering `●` as “evidence gathered” from `lifecycle=ready|investigating` can be acceptable only as lifecycle progress, not as proof that evidence exists. If count/superscript is used, source it only from `evidence_pointers.len()` and mark missing logs as implied/unknown.

4. **Collapsed observations are omitted.** Current focused observations include `Row::CollapsedObs` with representative row, collapsed count, summary prefix, and primary id. The plan needs rules for `CollapsedObs`: preserve `×N` visibly (either badge next to ID or `COUNT` column), derive flow from representative only, and decode member ids in detail. Otherwise implementation will regress existing duplicate-summary grouping.

5. **Intake PRI column is currently unsupported.** `IntakeRow` has `priority: Option<String>`, but `load_intake_rows` currently sets `priority: None`. The plan says priority is already loaded. Revise Phase 1 for intake to load a `priority` column if present, otherwise render `normal`/blank with implied confidence. Without that, a `PRI` column would be mostly guessed.

6. **External review plan overstates loaded row coverage.** `load_rows` currently selects `external_reviews` only where `status IN ('pending','running','tooling_held')`; it does not load `passed`, `revise`, or `superseded` external review rows. The plan’s examples/tests for passed/revise/superseded either require changing that query/scope or should be declared out of initial scope. Also `Row::Review` includes `architecture_reviews` rows with statuses like `in_review`, `awaiting_human_ratification`, and `verdict_issued`; the “External reviews” lane is actually a mixed review lane today. The plan must decide whether to include architecture-review-specific mapping in Plan C or split “external/code review” from “architecture review” later.

7. **External review tooling color rule is too loose.** “failed/waiting based on held reason/retry timestamp” can mislead. Safer rule: status drives primary glyph/severity (`tooling_held`/`tool_fault` => `▲` fault or held-fault); `next_retry_at` is a retry/age column or reason signal, not enough to recolor the fault as normal waiting unless schema explicitly marks it retry-waiting.

8. **Engine health source model needs to use existing `EngineDetail`.** The plan’s “runner-role active counts if live-run/dispatch data can support it cheaply” should name current available sources: `unfinished_lock_rows` has `agent_name`, `claimed_by`, `heartbeat_at`, `liveness_label`, `attempts`; `recent_agent_runs_by_role` is historical aggregate, not live runners. Do not present `RUNNERS ◆ active 4` unless derived from unfinished locks/live run summaries with liveness thresholds. Agent-runs should stay informational.

9. **App sorting/navigation integration is missing for non-observation lanes.** `App::sort_flat_rows_for_projection_display` currently sorts only tasks and observations by `WatchProjection`. Intake and external reviews will retain section order unless extended. Each plan should include whether the focused lane sort order changes and add tests for navigation order, like the current task/observation tests.

10. **Rendering split should be per-lane, not through task projection fallback.** Current `format_row_line_for_task_projection` falls back to prose renderers for non-task rows; `format_row_line_for_observation_projection` still uses old observation table-ish rendering. Implementation should add lane-specific `*_table_header`, `*_table_width`, and `format_*_table_line` functions, then optionally extract helpers after two lanes. The plan hints this, but should say it explicitly for each lane.

Recommendation:
- Keep the overall plan, but revise it before implementation with the above corrections.
- Split the work into four independent implementation tasks/lane plans, not one all-lanes task. Recommended order remains good with one adjustment:
  1. **Observations** first, but include collapsed-row handling and resolve the checkpoint-position ambiguity.
  2. **Intake** second, after adding/loading priority or removing `PRI` from v1.
  3. **Review lane** third, but first decide whether it covers both `external_reviews` and `architecture_reviews`; update data loading if passed/revise/superseded rows are intended.
  4. **Engine health** fourth, as a separate dashboard projection consuming `EngineDetail` and daemon liveness.
- Add a short “common implementation note” to the plan: share `MapColor`, `MapConfidence`, `superscript_number`, and style helpers where useful, but keep lane-specific glyph/source enums until a neutral `VisualCell` is proven.

Risks:
- If implementers reuse `MapCell` unchanged, they will either pollute task-specific enums or make non-task source/confidence labels vague.
- If the review lane does not account for architecture-review rows, the current “External Reviews” focused lane may render misleading verdicts or unknown states for architecture gates.
- If observation flow positions are not fixed before coding, snapshot tests may lock in an ambiguous map that users cannot decode.
- If intake priority is not loaded first, the dense table will replace prose bags with a column of guessed/default values.

Need from main agent:
- Decide whether to revise the plan doc now or treat these as pre-implementation notes.
- Decide whether Plan C is “all `Row::Review` rows” (external + architecture reviews) or only `external_reviews`, with architecture reviews split later.

Suggested execution prompt:
- No implementation handoff yet. First handoff should be a narrow doc-revision worker: “Revise `docs/worklog/2026-05-13/08-watch-other-store-view-map-plans.md` to incorporate oracle findings: fix projection primitive strategy, observation checkpoint ambiguity/collapsed rows, intake priority loading, review-lane scope/data query, engine source rules, and per-lane sorting/render integration. Do not implement code.”
