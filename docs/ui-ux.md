# UI / UX Doctrine

`stores` UI should expose the substrate as a living engine, not as a raw database dump.

The operator is watching flow: signals enter intake, become observations, become contracts/tasks/reviews, and exit through shipped/resolved/retired sinks. The UI should make engine health legible at a glance: where work is moving, where pressure is accumulating, and whether the bottleneck is the AI, review, deployment, backlog, tooling, or the human operator.

## Core principles

1. **Flow before rows.** Default views show store-level flow, queue depth, rates, age, and pressure before listing individual rows.
2. **Graphics before prose.** Use terminal graphics, color, sparklines, phase glyphs, and badges to show pressure and progress. Text explains after selection/drilldown.
   - Use few shapes densely: shape = workflow family, position = phase/pipeline index, fill = substate, superscript = cycle count, color = result/pressure, animation = currently active. Do not add a new glyph family when fill/color/superscript can carry the distinction.
3. **Stores are lanes.** The top-level cockpit is organized by stores/lane surfaces: intake, observations, tasks, external reviews, and engine health. Task internals such as planning/execution/code-review belong inside the task lane, not as top-level stores.
4. **Rows are drilldown.** Selecting a lane shows a sortable/filterable row table for that store; selecting a row shows details, lifecycle position, logs, evidence, and suggested next action.
5. **Action is a valve.** Human approvals, resumes, ratifications, and retries are part of the engine. They should be visible as pressure/valves, not separated from system flow.
6. **History is exhaust, not the dashboard.** Terminal/shipped/retired rows are summarized as recent exhaust and hidden from the main list by default.
7. **Generic, with custom renderers.** Any store should have a generic status-count/table/detail fallback. Known stores get richer renderers for their semantics.
8. **System faults are health badges.** Daemon death, stale locks, runner failures, and missing subscribers matter, but should not dominate the cockpit unless they affect flow.

## Target cockpit shape

Default `stores watch` should be TUI-first, btop-like, and read-only initially.

```text
┌ STORE FLOW ────────────────────────────────────────────────────────────────┐
│ INTAKE        OBSERVATIONS       TASKS             EXTERNAL REVIEWS ENGINE │
│ draft 21 ⚠    open 35 ⚠         active 1 ●        revise 341       dev ⚠  │
│ +2/h -0/h     +5/h -1/h         held 0            tooling 0        locks  │
│ drop 3        ready 2           plan ✕✕✕✓         passed 15        daemon │
├───────────────────────────────┬───────────────────────────────────────────┤
│ FOCUSED STORE ROWS            │ SELECTED ROW DETAIL                       │
│ sortable/filterable table     │ lifecycle, evidence, logs, next action    │
├───────────────────────────────┴───────────────────────────────────────────┤
│ EVENT STREAM / ACTIVE AGENT LOGS / RECENT EXHAUST                         │
└───────────────────────────────────────────────────────────────────────────┘
```

Navigation model:

- `←/→`: move focus across store lanes.
- `↑/↓`: move within the focused store table.
- `/`: filter.
- `s`: sort.
- `Enter`: drill into selected row.
- `Tab`: rotate major panes.
- actions come later; first version is read-only with suggested commands.

## Store renderers

### Intake

Show triage workload and routing pressure:

- status counts: draft, triaging, needs_info, routed, dropped
- inflow/outflow rates
- oldest/p95 age
- source agent/task
- cluster key
- risk flags
- decision / routed target / duplicate / arch-review candidate

### Observations

Show contract pressure and promotion safety:

- open, investigating, ready, resolved, wont_fix
- contract_state and tier_hint
- linked child task / auto-promote risk
- source and priority
- resolution or wont_fix sink
- evidence and investigation failures

### Tasks

Show active engine work and review loops:

- task title/summary as first-class row content; it is not status prose
- lifecycle state and phase/cycle as a dense visual map
- one visual slot per logical phase: planning slot, then 1..N execution phase slots created by the plan
- circle family for planning (`◌` queued/pre-plan, `○` planning, `●` plan review/result; `◉` is a possible reviewed-circle variant with an infill gap)
- square family for execution (`·` unreached planned phase, `□` executing, `▣` code review/result, `▰` acceptance/wrap)
- superscript numerals for cycle counts (`□²`, `▣¹²`); no superscript means cycle 1
- color for active/pass/fail/waiting: active work blue/teal, active review peach/yellow, passed green, failed red, inactive dim
- subtle breathing animation only for the currently active cell
- blocked/deploy_blocked reason class as a bounded reason column, not raw JSON
- workspace/log/transcript/debug pointers in detail panes, not scan rows
- recent terminal exhaust only, not full history

Detailed task-map design note: `docs/worklog/2026-05-13/06-watch-visual-state-grammar.md`.

### External reviews

Show formal review lane health:

- pending/running/passed/revise/tooling_held/superseded
- runner/model
- duration
- finding counts
- stale base/head metadata
- log/transcript paths

### Engine health

Show operational substrate health:

- daemon starts/liveness
- dispatch locks and stale unfinished locks
- runner actions/held reasons
- heartbeats
- agent_runs and transcript paths

Engine health should be compact unless unhealthy.

## Existing data we can use now

The current substrate already supports much of this:

- `transition_history` gives event flow and rolling rates.
- `tasks.plan_review_log`, `tasks.cycles`, `current_phase`, `current_cycle` give task loop/phase graphics.
- `intake` has source, evidence, decisions, risk flags, cluster keys, and route targets.
- `observations` has intent contracts, tier hints, status, source, evidence, and resolution fields.
- `external_reviews` has runner/model, verdict, findings, SHAs, durations, logs, and transcripts.
- `agent_runs`, `dispatch_locks`, `daemon_starts`, and `engine_runner_*` tables expose engine/runtime health.

The main missing primitive is durable queue-depth time series. First derive sparklines/rates from `transition_history`; add explicit sampling only if replay is too slow or insufficient.

## Mockups

Design references live under:

- `docs/worklog/2026-05-09/watch-mockups/index.html`
- `flowgraph-process-split.html` — strongest default direction from round 2.
- `store-focus-intake.html` — intake lane drilldown.
- `store-focus-observations.html` — observation lane drilldown.
- `store-focus-tasks.html` — task lane drilldown.
- `review-loop-microscope.html` — task review/phase drilldown.

## Phased approach

### Phase 1 — Correct the mental model

Replace misleading default watch output with a store-focused cockpit skeleton:

- top strip: intake / observations / tasks / external reviews / engine
- current status counts per store
- focused store table
- selected row detail pane
- hide terminal history by default except recent exhaust
- keep read-only

### Phase 2 — Add flow and pressure

Make the cockpit feel alive:

- rolling rates from `transition_history`
- age metrics for queues
- sparklines for inflow/outflow
- review-loop badges/glyphs
- pressure coloring for backlog, loops, stale locks, deploy/review failures

### Phase 3 — Rich store-specific drilldowns

Implement custom renderers:

- intake routing view
- observation contract/promote safety view
- task lifecycle/phase/review microscope
- external-review findings/runner view
- engine health view

### Phase 4 — Logs and liveness

Connect row detail to live artifacts:

- active agent transcript/log tailing
- runner/model display from config and `agent_runs`
- manual-engine vs daemon distinction
- stale lock impact explanation

### Phase 5 — Interaction and actions

After read-only trust is established:

- fast filters and sorts
- command suggestions
- guarded actions for safe operations
- token/confirmation gates for U-moments
- no hidden writes from the cockpit

The UI should earn authority gradually: first make the engine visible, then make it navigable, then make it actionable.
