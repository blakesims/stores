# Oracle review — watch declarative projection plan

## Verdict

The direction is right, but the current plan is too broad and underspecified for safe immediate execution. The risky part is not the top-card renderer; it is that `Section` is an old cross-store operational taxonomy used by filtering, focus, collapse state, tests, and mission-compact rendering. Renaming/replacing sections directly in Phase 1 could create hidden breakage outside the visible watch rows.

Recommended improvement: do **not** start by changing `Section` as the canonical model. Start by adding a typed projection layer that produces a **watch display section** beside the legacy `Section`. Use it in cockpit rendering for tasks/observations only, while keeping legacy `Section` as the internal bucket/filter compatibility layer until a later cleanup. This gives the user-visible grammar alignment without pretending the schema/frontend architecture is solved in one pass.

## Inherited decisions / constraints to preserve

- The user agreed with the report’s recommended approach: move toward schema-declared/operator-truth projection so top cards, sections, and rows use one grammar.
- This implementation is the first step, not the final YAML-driven generic store system.
- No database migration or legacy `status` removal in this pass.
- Do not break current top-card readable grid, responsive fallback, or Catppuccin severity coloring.
- Raw/debug tuples must remain available in detail panes.
- Tasks and observations are the first proving ground; intake/external reviews/engine can remain custom for now.
- Option A for observations is now in force: top-card observation buckets are mutually exclusive.
- Recent UX doctrine: avoid hiding truth through arbitrary frontend translation. Any typed projection added now should be explicitly labeled as a transitional schema-seam, not the final source of truth.

## Diagnosis: what the plan gets right

- It correctly identifies the core defect: top-card counts, section headers, and row text are currently produced by independent frontend classifiers.
- It correctly narrows first implementation to tasks and observations.
- It correctly calls out row-state suppression: if the section says `QUEUED`, every row should not repeat `queued`.
- It correctly avoids building a YAML expression engine immediately.
- It correctly keeps the future `watch:` schema metadata as the destination.

## Main risks / hidden contradictions

### 1. Directly renaming/replacing `Section` is too dangerous

`Section` is not just display text. It is used for:

- canonical ordering (`Section::ALL`),
- filters (`src/tui/filter.rs` names like `accept_u3`, `held_ai_review`, `needs_triage`),
- collapse state and keyboard navigation,
- mission compact behavior,
- recent terminal handling,
- tests that assert specific buckets,
- some non-task rows being intentionally routed into task/obs priority sections.

If a worker changes `Section::TasksActionableCurrentWork => "WORKING"` and collapses `TasksAcceptU3`, `TasksBlockedNeedsAction`, `TasksNeedsTriage`, etc. into generic slots, it may lose important operational distinctions and break filters.

### 2. Top-card slots are six canonical meta-slots, but focused sections need not be exactly six raw buckets internally

User-visible sections should align with the grammar, but the engine may still need subcategories for routing/debug. For example:

- `FAILED` may contain runner failure, review-blocked, deploy failed, main-red.
- `WAITING` may contain capacity, dependency, rate-limit, stale-base.
- `GATE` may contain plan-review, code-review, acceptance/wrapping, integration valves.

The display can group these under one section while row stage/signal retains the subtype. Do not destroy subtype information.

### 3. “One projection source” must be concretely defined

Current code already has `task_presentation`, `observation_presentation`, `apply_task_to_flow`, `apply_obs_to_flow`, and `section_for`. The plan says “one projection” but does not say which functions move or which call sites stop using old logic. Without that, a worker may add a fourth abstraction while leaving old classifiers in place.

### 4. Observation phase is smaller than task phase, but has dedup/collapse complications

Observation rows can be `Row::Obs` or `Row::CollapsedObs`. Collapsed observations carry a stored `section` and a representative row. If observation display sections become projection-based, collapsed rows need explicit handling; otherwise they can remain in stale `PRIORITY`/`OBSERVATIONS` labels.

### 5. “No frontend dictionaries” is a future goal, not an immediate acceptance criterion

The current pass will necessarily contain Rust mappings from ADR fields to display slots. The plan should explicitly say these mappings are transitional and must be centralized in a module that mirrors the future `watch:` schema shape.

## Improved execution plan

Use **three small worker/review chains**, not one large refactor. Each chain should commit independently and include rendered-buffer tests.

---

## Phase 1 — Add transitional watch projection model, wire task top-card counts only

### Goal

Create one typed projection function for tasks and make task top-card counts use it. Do **not** change focused sections yet. This proves the source-of-truth function without destabilizing navigation/filters.

### Files

Likely files:

- `src/tui/semantics.rs` — best home for row-level projections because it already contains `task_presentation` and `observation_presentation`.
- `src/tui/data.rs` — update `apply_task_to_flow` to consume task projection.
- `src/tui/render.rs` — only if `TopSlotAttention` / `FlowSlot` visibility needs adjustment; avoid moving renderer concerns into data.
- Tests in existing `tui::semantics::tests`, `tui::data::tests`, and maybe `tests/tui_watch_semantic_regression.rs`.

### Implementation details

Introduce a neutral projection type with names that match future schema metadata:

```rust
pub enum WatchSlotId {
    Front,
    Work,
    Gate,
    Exit,
    Wait,
    Fault,
}

pub struct WatchProjection {
    pub slot: WatchSlotId,
    pub slot_label: &'static str, // queued/working/gate/done/waiting/failed for tasks
    pub glyph: &'static str,
    pub row_stage: &'static str,  // plan, plan-gate, exec, accept, capacity, runner failed
    pub row_signal: Option<String>,
    pub next_action: Option<&'static str>,
    pub attention: WatchAttention,
}
```

Do not expose renderer-private `TopSlotAttention` from `render.rs`; define a semantics-level `WatchAttention` or keep attention mapping in render with a narrow conversion. Avoid circular dependency from `data.rs` to `render.rs`.

Add:

```rust
pub fn task_watch_projection(task: &TaskRow) -> WatchProjection
```

Precedence should mirror inherited decisions:

1. terminal/done -> `Exit / done`
2. blocked -> `Wait` or `Fault` according to blocker kind
3. integration -> likely `Gate` or `Work`? Be explicit. Recommended for this pass: integration active ship stages map to `Gate` unless later top-card grammar adds ship. Do not silently drop integration distinctions; row_stage can be `ship`.
4. active planning/coding -> `Work`
5. active planning_review/coding_review/wrapping -> `Gate`
6. queued -> `Front / queued`

Then update `apply_task_to_flow` to use `task_watch_projection` instead of duplicating the same precedence.

### Acceptance

- `apply_task_to_flow` no longer independently re-implements task slot classification.
- Task top-card counts remain identical or intentionally corrected, with tests for representative queued/work/gate/wait/fault/done rows.
- Existing focused table labels remain unchanged in Phase 1.
- Tests pass:
  - `cargo test -q tui::semantics::tests:: --lib`
  - `cargo test -q tui::data::tests::store_flow_model_counts_mixed_rows_per_lane --lib`
  - `cargo test --test tui_watch_semantic_regression`

---

## Phase 2 — Render task focused sections from projections, suppress duplicate row state

### Goal

Make the user-visible task table use the same grammar as the task top card without replacing legacy `Section` internals.

### Files

- `src/tui/render.rs` — focused table rendering and task row formatter changes.
- `src/tui/data.rs` — only if helper methods on `Section` are needed; avoid broad classification rewrite.
- `src/tui/semantics.rs` — may add `task_watch_projection` helpers for display names.
- Tests: `tests/tui_watch_cockpit.rs`, `tests/tui_watch_semantic_regression.rs`, `src/tui/render.rs` unit tests.

### Implementation details

Preferred low-risk design:

- Keep `App.sections: Vec<(Section, Vec<usize>)>` unchanged.
- In `draw_focused_table`, when `app.focused_store == StoreLane::Tasks`, derive **display groups** from the visible task rows using `task_watch_projection`.
- Display group order: `QUEUED`, `WORKING`, `GATE`, `WAITING`, `FAILED`, `DONE`.
  - If `DONE` is still hidden in the recent-exhaust strip, keep that existing behavior; do not move terminal history into the main table unless already visible.
- Row formatter receives the parent display slot/context, e.g.:

```rust
format_task_line_with_context(task, external_review, Some(parent_slot))
```

- Suppression rule:
  - If row `slot_label == section label`, omit the broad row stage.
  - Still show finer stage/signal when useful:
    - `WORKING`: show `plan` or `exec`.
    - `GATE`: show `plan-gate`, `code-gate`, `accept`, `ship`.
    - `WAITING`: show `capacity`, `dependency`, `rate limit`, not `waiting-capacity`.
    - `FAILED`: show `runner failed`, `review blocked`, `tests failed`, not generic `failed`.
    - `QUEUED`: omit `queued` entirely; show `no worktree` if relevant.

Do **not** delete legacy sections yet. They can remain for filters and non-cockpit paths. The display grouping should be an adapter layer.

### Acceptance

- In task-focused cockpit view, section headers match top-card grammar:
  - `QUEUED`, `WORKING`, `GATE`, `WAITING`, `FAILED` (and `DONE` only if terminal rows are shown in that area).
- Counts in those visible task display sections match task top-card slot counts for the same included rows.
- Rows do not repeat broad section state:
  - no `T001  ◌ queued` under `QUEUED`;
  - no `waiting-capacity` under `WAITING` if `capacity` can be shown;
  - no generic duplicate `failed` under `FAILED`.
- Important subtype remains visible: `plan`, `exec`, `plan-gate`, `accept`, `runner failed`, `capacity`.
- Existing filters/collapse/navigation still work or tests are explicitly updated for the display grouping.
- Tests:
  - rendered buffer test for task-focused view at representative clean DB shape;
  - assertion that top-card `queued/working/gate/waiting/failed` counts match section counts;
  - assertion that row duplication is absent.

---

## Phase 3 — Apply same projection/display grouping to observations

### Goal

Move observation top-card counts, focused sections, and rows behind one projection function, retaining Option A mutual exclusivity.

### Files

- `src/tui/semantics.rs` — add `observation_watch_projection` or upgrade `observation_presentation` to return slot + row stage.
- `src/tui/data.rs` — update `apply_obs_to_flow` to consume projection.
- `src/tui/render.rs` — observation display grouping and context-aware row suppression.
- Tests in `tui::data::tests`, `tui::render::tests`, `tests/tui_watch_semantic_regression.rs`.

### Implementation details

Add:

```rust
pub fn observation_watch_projection(row: &ObsRow) -> WatchProjection
```

Precedence should match Option A:

1. closed/superseded/wont-fix -> `Exit / closed`
2. architecture gate, human ratification, contract draft/approved/ready -> `Gate / contract gate`
3. non-contract waiting_kind -> `Wait / waiting`
4. in_progress/investigating -> `Work / investigate`
5. candidate/open -> `Front / candidates`
6. investigation_failed / broken linked task / explicit fault -> `Fault / errors` if available in fields

For collapsed observations:

- Either compute projection from representative and display under that projection slot, or keep collapsed rows in the original section but label the displayed section by representative projection.
- Add a test so collapsed observation clusters do not regress to `PRIORITY`/`OBSERVATIONS` if the focused observation table is meant to use projection groups.

Rows:

- Under `CONTRACT GATE`, show finer stage `draft`, `approved`, or `architecture`, plus `next: approve/revise` or `next: promote/resolve`.
- Under `WAITING`, show `info needed`, `external dependency`, etc.
- Under `CANDIDATES`, omit repeated `candidate`; show priority/summary/next triage.
- Keep raw `contract_state`, `waiting_kind`, lifecycle/status in detail/debug only.

### Acceptance

- Observation top-card and focused sections use identical buckets/counts: `CANDIDATES`, `INVESTIGATE`, `CONTRACT GATE`, `CLOSED`, `WAITING`, `ERRORS`.
- Option A still holds: a row counts in one visible bucket only.
- Rows do not repeat section state or raw schema vocabulary (`contract:`, `tier:`, `lifecycle=`, `waiting_kind=`).
- Collapsed observation rows are handled explicitly.
- Tests cover candidate, contract draft, contract approved/ready alias, info-needed, arch gate, closed, and collapsed cluster.

---

## Phase 4 — Document the seam and add architecture guard tests

### Goal

Make the transition explicit: this is a typed Rust projection that intentionally mirrors the future schema `watch:` block. Prevent future drift.

### Files

- `docs/worklog/2026-05-13/05-watch-declarative-projection-plan.md` may be updated after implementation with final notes, or add a small code comment module doc in `semantics.rs`.
- `src/tui/semantics.rs` module docs/comments.
- Tests in render/data/semantic regression.

### Implementation details

Add concise documentation, not a broad new doc unless needed:

- `WatchProjection` is transitional.
- Renderer must consume projection output for top slots/sections/rows.
- Future work: move projection metadata into `stores/*/schema.yaml watch:`.
- Raw fields remain detail/debug.

Guard tests:

- Task top-card labels and task display section labels are generated from the same projection labels.
- Observation top-card labels and observation display section labels are generated from the same projection labels.
- No old task section labels (`ACTIVE`, `AWAITING HUMAN ACCEPTANCE`, `HELD-TRIAGE`) appear in task-focused cockpit output.
- No old observation section labels (`PRIORITY`, generic `OBSERVATIONS`) appear in observation-focused cockpit output, if Phase 3 completes that display grouping.

## Scope tightening recommendations

1. **Do not call this “declarative” in code yet unless it is actually data-driven.** Use names like `WatchProjection` / `Projection` and document “future schema-declared seam.”
2. **Do not make YAML `watch:` metadata in this task.** That is a follow-up after the typed projection shape stabilizes.
3. **Do not remove/reshape `Section` in this task.** Build display grouping on top of legacy sections first.
4. **Do not generalize all stores.** Intake/reviews/engine can remain current until task/observation alignment proves the pattern.
5. **Do not hide subtypes.** Section says broad slot; row says subtype/stage/signal.
6. **Do not let top-card count tests use a different inclusion set than focused sections.** Be explicit about terminal rows/recent exhaust and hidden history.

## Suggested worker/review chain prompts

### Worker 1 prompt

Implement Phase 1 only: add a typed transitional `WatchProjection` model in the TUI semantics layer and route task top-card counts through `task_watch_projection`. Do not change focused table sections yet. Add unit tests for queued/work/gate/wait/fault/done task projections and data tests proving `apply_task_to_flow` uses the same slot outcomes. Preserve existing top-card layout/colors and detail behavior. Run targeted semantics/data/semantic-regression tests and commit only relevant files.

### Worker 2 prompt

Implement Phase 2 only: render task-focused cockpit sections from `task_watch_projection` display slots (`QUEUED`, `WORKING`, `GATE`, `WAITING`, `FAILED`, optionally `DONE` only where existing terminal rows are visible) while keeping legacy `Section` internals for filters/navigation. Make task row formatting context-aware so rows do not repeat broad section state but keep finer subtype/stage/signal. Add rendered-buffer tests that top-card task counts match focused section counts and old labels like `ACTIVE`, `AWAITING HUMAN ACCEPTANCE`, `HELD-TRIAGE` no longer appear in task-focused output. Run render/cockpit/semantic-regression tests and commit only relevant files.

### Worker 3 prompt

Implement Phase 3 only: add `observation_watch_projection`, route observation top-card counts through it, and render observation-focused sections from projection buckets (`CANDIDATES`, `INVESTIGATE`, `CONTRACT GATE`, `WAITING`, `ERRORS`, closed only if shown). Preserve Option A mutual exclusivity and handle `CollapsedObs` explicitly. Make observation row formatting context-aware to avoid repeated/raw `contract:`/`tier:`/section-state vocabulary while preserving detail/debug fields. Add tests for candidate, contract draft, approved/ready alias, info-needed, arch gate, closed, and collapsed cluster. Run data/render/semantic-regression tests and commit only relevant files.

### Worker 4 prompt

Implement Phase 4 only: add concise documentation/comments around the transitional watch projection seam and architecture guard tests proving task/observation top-card labels and focused section labels derive from the same projection vocabulary. Do not add YAML parsing or migrate schemas. Run relevant tests and commit only docs/tests/code comments touched.

## Final recommendation

Proceed, but with the above narrower sequencing. The most important correction is to avoid rewriting the legacy `Section` taxonomy as if it were just display state. Treat it as compatibility/internal routing for now; introduce projection-based **display groups** for the cockpit. That honors the user’s goal while avoiding a broad schema/frontend refactor disguised as a UI polish task.
