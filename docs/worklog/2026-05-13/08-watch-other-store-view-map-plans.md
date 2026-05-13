# Watch Other Store Views Map Plans

**Date:** 2026-05-13
**Type:** plan
**Oracle-reviewed:** yes; revisions below incorporate the oracle findings.

## Summary

Map the task-map design intention onto the other `stores watch` focused lanes without pretending they are task phase maps. The shared intention is:

- one dense aligned table per focused lane;
- one compact visual column that exposes the lane's durable workflow truth;
- few glyphs with consistent semantic dimensions: position = lifecycle/checkpoint, fill = substate/result, superscript/count = attempts/items, color = active/passed/failed/waiting/unknown;
- projection first, rendering second, detail decode third;
- every glyph/color/count names a persisted source field; unknown beats guessed;
- raw JSON, file paths, debug tuples, and prose bags move to detail.

`TaskMapProjection`, `MapCell`, `MapGlyph`, and `MapSource` are task-specific today. Do **not** extend task enums into a universal dumping ground. Other lanes should use lane-specific projection structs/cell/source enums while sharing only primitives that truly generalize (`MapColor`, `MapConfidence`, superscript/style helpers). A neutral `VisualCell` may be extracted later, after at least two non-task lanes prove the shape.

## Shared architecture target

Extend the same seam used by the task map:

```text
src/tui/data.rs      loads lane-specific structured evidence into rows
src/tui/semantics.rs owns pure lane projections + source/confidence
src/tui/render.rs    focused lane paths consume projection cells/tables
src/tui/detail.rs    decodes selected projection and source/confidence
src/tui/app.rs       sorts/navigation follows lane projections where appropriate
```

Recommended implementation split:

1. Observations: closest to an actual workflow; already has `observation_watch_projection` and collapsed rows.
2. Intake: simple funnel; first fix/confirm priority loading or omit `PRI` in v1.
3. Review lane: first decide that the lane covers **all `Row::Review` rows** currently shown by the TUI (external/code reviews plus architecture reviews), or split architecture reviews into a separate future lane. This plan assumes the current mixed `Row::Review` lane and calls out data-query widening explicitly.
4. Engine health: system dashboard projection consuming `SystemHealth`, `EngineDetail`, and daemon liveness.
5. Optional consolidation: only after patterns repeat, extract `DenseTableSpec` / neutral `VisualCell` helpers.

## Common implementation notes

- Add per-lane `*_table_header`, `*_table_width`, and `format_*_table_line` functions. Do not route non-task lanes through task projection fallbacks.
- Add per-lane focused-lane sort/navigation tests when row order changes. `App::sort_flat_rows_for_projection_display` currently sorts tasks and observations only.
- Keep lane-specific source enums (`ObservationFlowSource`, `IntakeFunnelSource`, etc.) so detail can explain evidence precisely.
- Reuse `MapColor`, `MapConfidence`, superscript rendering, and style mapping helpers where useful.
- Do not update legacy `src/cli/watch.rs` ANSI output unless a separate task asks for parity.

## Plan A — Observations focused lane

### Design target

Observation rows become a dense lifecycle table. The flow is **not** task phases; it is a fixed observation checkpoint sequence:

```text
signal/evidence │ contract │ arch? │ resolution
```

The first cell intentionally merges raw candidate signal and investigation/evidence progress. This avoids a low-value extra candidate cell and matches the examples: `◌` means observed but not yet investigated; `●` means investigation/evidence is active or complete.

```text
ID     SUMMARY                                      FLOW        NEXT            AGE  PRI  TIER LINK  CNT
L041   task map evidence gap                         ◌ · ·       triage          3h   high T3         
L042   investigate eval length                       ● · ·       gather          2h   high T2         
L043   contract ready for ratification               ● ▣ ·       approve/revise  1h   high T3         
L044   waiting on external library behavior          △           dependency      6h   med  T2         
L045   architecture gate open                        ● ▣ ◈ ·     architecture    4h   high T3   A003 
L046   resolved by T020                              ● ▣ ✓       done            1d   norm T3   T020 
L047   investigation failed                          ▲           inspect         5h   high T2         
L048   collapsed noisy duplicates                    ◌ · ·       triage          8h   low  T1        ×4
```

Observation visual grammar:

```text
◌  candidate/raw signal awaiting triage or evidence work
●  investigation/evidence active or lifecycle-proven complete
▣  intent contract drafted/approved/resolution in progress
◈  architecture gate pending/open
✓  resolved/addressed
×  closed wont-fix/dropped
■  superseded/duplicate terminal
△  waiting pressure outside normal flow
▲  fault, e.g. investigation_failed
·  known later checkpoint not reached
?  missing/unknown evidence
```

### Data evidence needed

Already loaded on `ObsRow`:

- `lifecycle`, `status`, `waiting`, `waiting_kind`, `outcome`;
- `contract_state`, `tier_hint`, intent fields;
- `pending_architecture_review`, `open_architecture_review_id`;
- `superseded_by_id`, `task_id`, `resolution_pointer`;
- `investigation_failure_reason`, `evidence_pointers`, `recent_events`.

Likely Phase 1 data additions:

- load architecture-review summary for `open_architecture_review_id` when available: status/lifecycle/outcome/verdict. Until loaded, `◈` is exact for gate existence but unknown for gate outcome.
- if schema has structured investigation/evidence logs, load them. Otherwise `●` from lifecycle is lifecycle progress only; superscript/count may use `evidence_pointers.len()` exactly.

### Projection shape

Add `ObservationFlowProjection` with lane-specific cells/sources:

```rust
ObservationFlowProjection {
  cells: Vec<ObservationFlowCell>,
  next: Option<String>,
  reason: Option<String>,
  link: Option<String>,
  collapsed_count: Option<usize>,
  confidence: MapConfidence,
}

ObservationFlowCell {
  checkpoint: ObservationCheckpoint, // SignalEvidence | Contract | Architecture | Resolution | Fallback
  glyph: ObservationGlyph,
  count: Option<i64>,
  color_role: MapColor,
  active: bool,
  source: ObservationFlowSource,
  confidence: MapConfidence,
}
```

### Key rules

- Signal/evidence `◌` exact from `lifecycle=candidate` or candidate/open status.
- Signal/evidence `●` active from `lifecycle=investigating|ready`; passed/implied once contract/resolution progress proves the row moved beyond evidence. Do not claim “evidence gathered” unless evidence pointers/logs exist.
- Contract `▣` active/gate from `contract_state=draft|ready|approved`; use gate color for human ratification.
- T1/T2 rows that resolve inside observation lifecycle still use contract/resolution cells; do not show task phase dots.
- Architecture `◈` exact from `pending_architecture_review` or `open_architecture_review_id`; outcome color only after the architecture review state is loaded.
- Waiting `△` fallback outside flow from generic `waiting_kind`; place it into contract only when `waiting_kind=human_ratification` or `contract_state=draft` makes the checkpoint exact.
- Fault `▲` fallback from `investigation_failed` or `investigation_failure_reason`.
- Terminal `✓/×/■` from `outcome`, `lifecycle=closed`, or `superseded_by_id`.
- `task_id` is a `LINK` column, not proof that observation work passed.
- `Row::CollapsedObs` derives flow from the representative row only, preserves `×N` visibly in `CNT` or ID badge, and decodes member ids in detail.

### Rendering columns

```text
ID | SUMMARY | FLOW | NEXT | AGE | PRI | TIER | LINK | CNT
```

- `SUMMARY` is first-class and receives elastic width.
- `FLOW` is fixed/dense, with optional architecture cell.
- `NEXT` comes from projection (`triage`, `gather`, `approve`, `architecture`, `resolve`, `done`, `inspect`).
- `LINK` shows `task_id`, produced/open architecture review, or superseded target when present.
- `CNT` shows collapsed count only for `CollapsedObs`.

### Acceptance

- Projection tests cover candidate, investigating, evidence pointers, contract draft, ratification wait, approved/ready contract, architecture gate with/without loaded outcome, info/external/capacity wait, resolving, addressed, wont-fix, superseded, investigation failed, collapsed representative.
- Rendered-buffer tests verify headers align, summary remains visible, `LINK`/`CNT` bounded, and old `next:... linked:... held:...` prose bags disappear from rows.
- Detail pane decodes each checkpoint, source, confidence, waiting/fault reason, and collapsed member ids.
- Focused observations still sort/navigate by projection display order.

## Plan B — Intake focused lane

### Design target

Intake is a triage funnel, so its map emphasizes classification/routing rather than lifecycle depth:

```text
ID     SUMMARY                                      FLOW      DECISION   AGE  PRI    SRC      ROUTE
I018   fake runner crash surfaced by T036            ◌ · ·     triage     2h   high   exec
I019   needs operator clarification                  ● △ ·     info       4h   med    planner
I020   duplicate of L041                             ● ■       duplicate  1h   low    scout    L041
I021   routed to observation                         ● ✓       routed     20m  high   exec     L048
I022   escalated to architecture review              ● ◈       arch       15m  high   gate     A003
I023   dropped as noise                              ● ×       dropped    1d   low    scout
```

Intake visual grammar:

```text
◌  captured/new signal
●  triage/classification in progress or decision evidence exists
✓  routed/produced observation/task
◈  escalated to architecture review
■  duplicate
×  dropped/noise
△  waiting for missing info/held
▲  failed/invalid routing if such state exists
·  known next funnel checkpoint not reached
?  missing/unknown evidence
```

Fixed funnel checkpoints:

```text
capture │ triage │ route
```

### Data evidence needed

Already loaded on `IntakeRow` except one caveat:

- `lifecycle`, `status`, `waiting_kind`, `outcome`, `decision`;
- `missing_info_question`, `held_reason`, `next_action`;
- `routed_to_observation`, `routed_to_arch_review`, produced artifact ids;
- `duplicate_of`, `duplicate_of_id`;
- `risk_flags`, `cluster_key`, `source_agent`, `source_task`;
- `recon_round`, `decision_rationale`, `decision_confidence`, `decision_tier_hint`.

Caveat: `IntakeRow.priority` exists, but current loading may leave it unset depending on schema/query. Phase 1 must either load `priority` when the column exists or omit/render blank `PRI` in v1. Do not fill a priority column with guessed `normal` while claiming exact evidence.

### Projection shape

Add lane-specific `IntakeFunnelProjection`:

```rust
IntakeFunnelProjection {
  capture: IntakeFunnelCell,
  triage: IntakeFunnelCell,
  route: Option<IntakeFunnelCell>,
  decision: Option<String>,
  route_target: Option<String>,
  confidence: MapConfidence,
}
```

### Key rules

- Capture `◌` exact from row existence; active when lifecycle/status is new/draft.
- Triage `●` active from `lifecycle=triaging`; exact passed/implied when any `decision` or route target exists.
- Waiting `△` replaces triage cell when `waiting_kind=needs_info`; generic held reason may become fallback/waiting.
- Route cell:
  - `✓` routed to observation/task from produced/routed ids;
  - `◈` architecture review route from produced/routed arch review ids;
  - `■` duplicate from duplicate fields;
  - `×` dropped/noise from outcome/decision;
  - `?` unknown if closed without a recognized outcome.
- `risk_flags` and `cluster_key` are badges/detail, not map dimensions.

### Rendering columns

```text
ID | SUMMARY | FLOW | DECISION | AGE | PRI | SRC | ROUTE
```

- `SRC` is bounded `source_agent`.
- `ROUTE` is produced/routed target or duplicate target.
- `DECISION` is a bounded canonical label, not raw rationale.
- If priority is unavailable, `PRI` renders blank/unknown or is removed for v1.

### Acceptance

- Data test proves priority loading if `PRI` ships in v1.
- Projection tests cover new/draft, triaging, waiting missing info, held, routed observation, arch review, duplicate, dropped, closed unknown, and recon round/count display if used.
- Render tests verify aligned columns and removal of `priority:`, `source:`, `cluster:`, `next:` prose bags.
- Detail decodes decision rationale/confidence/risk flags/cluster/source task.
- Add focused intake sorting/navigation tests if projection order differs from current section order.

## Plan C — Review lane focused view

### Scope decision

The current focused `ExternalReviews` lane renders `Row::Review`, which includes more than simple external/code reviews in practice. Implementation must choose one of two paths before coding:

1. **Mixed review lane v1 (recommended for current UI):** one projection that covers external/code review statuses plus architecture-review statuses with an `review_kind`/source field.
2. **Split lanes later:** keep v1 only for external review rows, but then data loading and lane naming must stop mixing architecture reviews into `Row::Review`.

This plan assumes path 1: a mixed review lane with clear status families. It also requires widening the current external-review query if terminal rows (`passed`, `revise`, `superseded`) should appear in the focused lane; current loading is known to prefer active/pending/tooling-held rows.

### Design target

```text
ID     TASK   REVIEW      VERDICT    FINDINGS  AGE  RUNNER   SHA
R014   T036   ◌           pending    -         12m  codex    a1b2c3d
R015   T037   ◆           running    -         8m   codex    b2c3d4e
R016   T038   ✓²          pass       0/0/2     4m   codex    c3d4e5f
R017   T039   ↻³          revise     0/2/1     1h   codex    d4e5f6a
R018   T040   ▲²          tool       -         2h   codex    e5f6a7b
A003   L045   ◈           arch       -         1h   arch     -
R019   T041   ■           superseded -         1d   codex    f6a7b8c
```

Review visual grammar:

```text
◌  pending dispatch
◆  running/in review
✓  passed/verdict issued and no action needed
↻  revise/non-terminal findings gate
◈  architecture/human ratification gate
▲  tooling fault/held/failure
■  superseded
?  unknown status
superscript = attempts when >1, from structured attempts only
```

### Data evidence needed

Already loaded on `ReviewRow`:

- `status`, `lifecycle`, `outcome`, `runner`, `held_reason`, `next_retry_at`, `attempts`;
- `verdict`, `critical_count`, `major_count`, `minor_count`, `findings_count`;
- `base_sha`, `head_sha`, log/transcript paths, started/completed/duration;
- linked observation ids and produced task id.

Phase 1 data work:

- audit `load_rows` / architecture-review loading to document exactly which tables feed `Row::Review`;
- if terminal review rows should display, widen query/filter intentionally and cap recent terminal rows if needed;
- add a `review_kind` or equivalent projection source if row provenance can be known cheaply.

### Projection shape

Add lane-specific `ReviewLaneProjection`:

```rust
ReviewLaneProjection {
  cell: ReviewLaneCell,
  verdict_label: String,
  findings_label: String,
  retry_or_reason: Option<String>,
  review_kind: Option<String>,
  confidence: MapConfidence,
}
```

### Key rules

- `pending` -> `◌`, front/waiting for dispatch.
- `running` / architecture `in_review` -> `◆`, active work.
- `passed` -> `✓`, passed; findings label from structured counts if present.
- `revise` -> `↻`, gate; findings label from structured counts if present, unknown if absent.
- architecture human/ratification statuses -> `◈`, gate; do not call them code-review pass/fail.
- tooling held/fault -> `▲`, fault/held-fault from status. `next_retry_at` is a retry/reason column, not enough to recolor the state as normal waiting unless schema explicitly says retry-waiting.
- `superseded` -> `■`, exit.
- superscript from `attempts` when >1; do not infer attempts from retry timestamps.
- SHA is display provenance, not semantic state.

### Rendering columns

```text
ID | TASK | REVIEW | VERDICT | FINDINGS | AGE | RUNNER | SHA
```

- `TASK` is fixed width `task_id` or linked observation for architecture reviews.
- `REVIEW` is the styled projection cell.
- `FINDINGS` is `critical/major/minor` when available, else total, else `-`.
- `SHA` is short base/head provenance; full paths stay in detail.

### Acceptance

- Data tests document active vs terminal row loading and architecture-review provenance.
- Projection tests cover pending, running, passed with findings, revise with attempts, tooling held/fault, superseded, architecture in-review/gate, unknown.
- Render tests verify alignment and no `verdict:`, `attempts:`, `held:`, `task=... runner=...` prose bags.
- Detail decodes runner, retry, duration, log/transcript paths, linked observations, produced task, review kind, and source/confidence.
- Add focused review-lane sorting/navigation tests if projection order changes.

## Plan D — Engine health focused lane

### Design target

Engine health is not row-list work; it is a compact system state dashboard. Apply the dense visual grammar as check rows:

```text
CHECK       STATE   SIGNAL        AGE    COUNT  DETAIL
DAEMON      ✓       live          2m     -      pid 12345
LOCKS       △       stale?        18m    2      oldest claimed
RUNNERS     ◆       active        1m     4      from unfinished locks
AGENT RUNS  ✓       recent        6m     18     tokens 1.2M
```

Engine visual grammar:

```text
✓  clear/live/healthy
◆  active work currently running
△  waiting/manual/stale risk
▲  fault/down/stale lock beyond threshold
?  evidence unavailable
```

### Data evidence needed

Currently loaded:

- `SystemHealth.unfinished_dispatch_locks`, `oldest_claimed_at_epoch`;
- `EngineDetail.recent_daemon_starts`;
- `EngineDetail.unfinished_lock_rows` with `agent_name`, `claimed_by`, `claimed_at`, `heartbeat_at`, `liveness_label`, `attempts`;
- `EngineDetail.recent_agent_runs_by_role` historical aggregate;
- daemon liveness from `daemon::Liveness`.

Do not present live runner counts unless derived from unfinished locks/live-run summaries with explicit liveness thresholds. `recent_agent_runs_by_role` is historical/informational, not proof of active runners.

### Projection shape

Add lane-specific `EngineHealthProjection`:

```rust
EngineHealthProjection {
  checks: Vec<EngineCheck>,
  overall: EngineCheckCell,
  confidence: MapConfidence,
}

EngineCheck {
  name: &'static str,
  cell: EngineCheckCell,
  signal: Option<String>,
  count: Option<i64>,
  age: Option<String>,
  detail: Option<String>,
  source: EngineSource,
}
```

### Key rules

- Overall state is max severity of checks: fault > waiting > active > clear > unknown.
- Daemon live exact from liveness; daemon down with unfinished locks is fault; daemon absent with no locks is manual/waiting.
- Locks: count exact from unfinished locks; stale threshold from `claimed_at`/`heartbeat_at` and `liveness_label` determines waiting vs fault.
- Runners: active only from unfinished locks/live liveness, not historical agent-run aggregates.
- Agent runs: informational; do not mark unhealthy solely because no recent runs exist.

### Rendering target

```text
CHECK | STATE | SIGNAL | AGE | COUNT | DETAIL
```

Top card flow slots should consume the same `EngineHealthProjection` rather than separate `engine_flow_slots` heuristics, so focused engine detail and cockpit cards cannot disagree.

### Acceptance

- Projection tests cover daemon live, daemon down with locks, manual/no-daemon-no-locks, stale locks, active runners from lock rows, historical agent-run info, unavailable evidence.
- Render tests verify aligned engine check table and detail decode.
- Top cards and focused engine detail agree because they consume one projection.

## Plan E — Cross-lane cleanup after A-D

Only after at least observations + intake prove the shape:

1. Extract a neutral `VisualCell`/`VisualToken` helper if it removes duplication without hiding lane-specific source enums.
2. Extract table width helpers for `ID | SUMMARY | VISUAL | ...` lanes.
3. Keep lane-specific projection structs and source enums; do not collapse all lanes into an untyped `Vec<MapCell>` that loses doctrine.
4. Add one semantic regression test fixture per lane that catches prose-bag regressions and source/confidence drift.
5. Consider moving the projection contract into schema-declared `watch:` metadata only after Rust-owned contracts stabilize.

## Non-goals

- Do not make non-task lanes look like task phase maps.
- Do not use color or glyphs for states not backed by persisted fields.
- Do not hide required operator actions in detail only; `NEXT`/`DECISION`/`VERDICT` must remain row-visible.
- Do not update legacy `src/cli/watch.rs` ANSI output unless a separate task requests parity.
- Do not introduce animation before static projections and style tests are stable.
