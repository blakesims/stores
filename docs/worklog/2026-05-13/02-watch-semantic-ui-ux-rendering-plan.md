# Watch Semantic UI/UX Rendering Plan

**Date:** 2026-05-13
**Type:** note

## Summary

The clean database experiment proved that `stores watch` is not hopeless, but it still leaks schema and implementation vocabulary into the operator cockpit. The next implementation task should make watch semantically intelligent: keep the existing lane-card top layout, keep row/detail density, keep live updates/log affordances, but translate raw ADR 0001 / ADR 0002 tuples into uniform operator language.

This is deliberately a UI/UX rendering plan, not a schema rewrite. Blake remains uncomfortable that current code can still write legacy `status` values into a fresh DB; that is a real architectural concern for a later schema-minimalism task. For this plan, assume the current schema exists and make `stores watch` stop making it feel polluted.

Core thesis:

> The cockpit should show what the row means to the operator, not how the row is encoded internally.

Primary output target:

- Default `stores watch` TUI.
- Legacy `stores watch --legacy` should either receive the same semantic vocabulary or be clearly treated as a lower-priority compatibility surface.
- Detail panes should retain raw data/debug tuple access, but row lists and top cards should be semantic.

## Context from the clean DB experiment

After archiving the old DB and creating a fresh schema DB, we seeded:

- 10 observations across open / investigating / needs-info / closed shapes.
- 10 tasks across queued / planning / plan-review / coding / in-review / blocked shapes.
- A fake-runner happy path (`T009`) to `in_review`.
- A fake-runner nonzero failure (`T010`) to `blocked:runner` with telemetry.

The resulting watch view was much cleaner than the old historical DB, but still uncomfortable:

```text
TASKS
▾ QUEUED (2)
  T001 planning lifecycle=queued active_step=none integration_step=none not scaffolded runner:none ...

▾ ACTIVE (3)
  T002 planning lifecycle=active active_step=planning integration_step=none not scaffolded runner:none ...
  T003 plan_review lifecycle=active active_step=planning_review integration_step=none plan ◐ runner:none ...
  T004 executing lifecycle=active active_step=coding integration_step=none ▮ ··· runner:none ...

▾ AWAITING HUMAN ACCEPTANCE (1)
  T009 in_review lifecycle=active active_step=wrapping integration_step=none wrap → ext:1 runner:none ...

▾ HELD-TRIAGE (4)
  T010 blocked:runner active:none:none runner:none ... reason:{"exit_code":42,"kind":"runner_crash"}
  T007 blocked:capacity not scaffolded runner:none ...
```

This is better than the old DB but still wrong for an operator cockpit. The UI exposes:

- raw legacy `status` words (`planning`, `plan_review`, `executing`, `in_review`);
- raw primary tuple internals (`lifecycle=... active_step=... integration_step=...`);
- empty/negative implementation facts (`runner:none`, `held_reason=none`, `next_retry_at=none`);
- vague buckets (`active`, `held`, `review`) that are not clear enough to Blake;
- daemon death as a generic alarm even when the current mode is intentionally manual/test.

## Non-goals

- Do not rewrite the schema in this task.
- Do not remove the legacy `status` column yet.
- Do not change lifecycle transition semantics.
- Do not remove detail pane information.
- Do not hide real system faults; translate them.
- Do not use emojis. Use monochrome / terminal-quality glyphs and possibly Nerd Font icons only where safe.
- Do not turn the cockpit into a sparse toy. Blake wants clarity, not less information.

## Important schema truth

### Why legacy task `status` still appears in a fresh DB

Fresh DB does not remove `status` because the current schema/code still writes it. A new row can legitimately have:

```text
status=planning
lifecycle=queued
active_step=none
integration_step=none
```

This is not old-row pollution. It is current compatibility design.

ADR 0001 primary task truth is:

```text
lifecycle / active_step / integration_step / blocked / blocker_kind
```

But the old `status` column still exists because:

- transition edges still use `from` / `to` states that match old status values;
- transition history still records `from_status` / `to_status`;
- old renderers/tests/readers still expect status strings like `planning`, `executing`, `in_review`;
- schema transitions currently write both legacy `status` and ADR 0001 tuple fields.

UI rule:

> Default watch views must treat `status` as compatibility/debug data, not operator truth.

### ADR 0001 task tuple

Primary task fields:

| Field | Role |
|---|---|
| `lifecycle` | broad engine lane: queued / active / integration / done |
| `active_step` | active-work substage: none / planning / planning_review / coding / coding_review / wrapping |
| `integration_step` | ship-lane substage: none / queued / refreshing / task_review / testing / merging / deploying / verifying |
| `blocked` | row is blocked or held from normal flow |
| `blocker_kind` | typed blocker reason class |

The UI should map this tuple to a single stage label plus a signal.

### ADR 0002 upstream tuple

Primary intake/observation fields:

| Store | Primary fields |
|---|---|
| Intake | `lifecycle`, `waiting_kind`, `outcome`, plus decision/routing fields |
| Observations | `lifecycle`, `contract_state`, `waiting`, `waiting_kind`, `outcome`, plus risk/cluster/linkage |
| Architecture reviews | `lifecycle`, `outcome`, open/ruling fields |

Observation `status` remains compatibility text. Default watch should foreground contract/routing meaning, not raw status.

## Top lane cards

Blake likes the current lane-card separation. Keep that shape: intake, observations, tasks, external reviews, engine. Do not replace it with a single merged strip.

Problem: current card words are too vague:

```text
TASKS
active: 4
held 4 · review 2
```

Questions this raises:

- What does active mean?
- What is held?
- Is held waiting, failed, blocked, or capacity?
- What kind of review?
- What should Blake do?

### Proposed top card grammar: same flow shape for every store

Use store-specific operator words, but every lane card must share the same meta-pattern. Blake's correction: each store has its own inner flow, but the top level should make those flows comparable. Do not design five unrelated cards.

Universal card grammar:

```text
◌ front   ◆ work
◇ gate    ✓ exit
△ wait    ▲ fault
```

Meaning:

| Glyph | Meta-slot | Meaning |
|---:|---|---|
| `◌` | front | newly arrived / queued / candidate / pending |
| `◆` | work | actively being processed |
| `◇` | gate | waiting on review, approval, contract, info, merge, or another valve |
| `✓` | exit | recently routed/resolved/done/passed |
| `△` | wait | non-failure waiting pressure |
| `▲` | fault | failed / blocked-by-error / tool fault |

Every lane card should render this shape, with local labels appropriate to that store.

Target sketch:

```text
┌ INTAKE ─────────────┐┌ OBSERVATIONS ──────┐┌ TASKS ─────────────┐┌ REVIEW ────────────┐┌ ENGINE ────────────┐
│◌ new 0   ◆ triage 0││◌ cand 8  ◆ inv 4  ││◌ queue 2 ◆ work 3 ││◌ pend 1 ◆ run 0   ││◌ disp 0 ◆ run 0   │
│◇ info 0  ✓ route +0││◇ contract 1 ✓ close+2│◇ gate 1  ✓ done +0 ││◇ revise 0 ✓ pass +0││◇ locks 0 ✓ clear   │
│△ wait 0  ▲ fault 0 ││△ wait 1  ▲ fault 0 ││△ wait 2  ▲ fail 2 ││△ wait 0  ▲ tool 0 ││△ manual ▲ fault 0 │
└────────────────────┘└────────────────────┘└────────────────────┘└────────────────────┘└────────────────────┘
```

This sketch is intentionally compact. Implementation can use fuller labels where width allows (`candidate`, `investigate`, `pending`, `running`, `tool-fault`), but the slot pattern must stay stable.

#### Intake card flow

Inner flow:

```text
raw signal → triage → needs-info gate → routed/dropped
```

Card:

```text
INTAKE
◌ new 0   ◆ triage 0
◇ info 0  ✓ routed +0
△ wait 0  ▲ fault 0
```

Mapping:

- `◌ new`: `lifecycle=new` / draft raw input.
- `◆ triage`: `lifecycle=triaging`.
- `◇ info`: `lifecycle=waiting` / evidence-needed gate.
- `✓ routed`: closed/routed outcomes, duplicate, fast-track, architecture-review route; recent exhaust only.
- `▲ fault`: dropped as noise or triage failure/fault; do not over-alarm normal `dropped` if it is an intentional sink.

#### Observations card flow

Inner flow:

```text
candidate → investigate/refine → contract gate → resolved/promoted
```

Card:

```text
OBSERVATIONS
◌ candidate 8  ◆ investigate 4
◇ contract 1   ✓ closed +2
△ wait 1       ▲ fault 0
```

Mapping:

- `◌ candidate`: `lifecycle=candidate`, not actively under evidence/refinement, no approved contract.
- `◆ investigate`: evidence gathering / contract shaping / investigation in progress.
- `◇ contract`: contract-draft, contract-approved, info-needed, human-ratification, or architecture gate. This is the ADR 0002 front-gate pressure slot.
- `✓ closed`: addressed, superseded, wont-fix, merged, resolved; recent exhaust only.
- `△ wait`: non-failure wait overlays such as info-needed or external dependency when not counted under contract gate.
- `▲ fault`: investigation_failed, broken linked task, overdue architecture gate, or other fault.

Use `contract-approved`, not `contract-ready`, in row/detail text. The top card may shorten to `contract` because the slot includes both draft and approved contract pressure.

#### Tasks card flow

Inner flow:

```text
queued → plan/exec work → accept/integration gate → done
```

Card:

```text
TASKS
◌ queued 2  ◆ work 3
◇ gate 1    ✓ done +0
△ wait 2    ▲ fail 2
```

Mapping:

- `◌ queued`: `lifecycle=queued` rows not yet flowing.
- `◆ work`: active planning, plan-gate, execution, or code-gate work. The row table gives exact stage (`plan`, `plan-gate`, `exec`, `code-gate`).
- `◇ gate`: acceptance/wrap/external-review/integration valve pressure. Row table can say `accept` or `ship`; top card uses the generic gate slot.
- `✓ done`: terminal/recent exhaust.
- `△ wait`: non-failure blockers such as capacity, dependency, rate-limit, human wait, or stale-base if recovery is expected by refresh/re-review.
- `▲ fail`: runner-failed, review-blocked, tests-failed, config-fault, main-red, deploy-failed, migration-failed.

Avoid top-card `active`, `held`, or generic `blocked`; use `work`, `wait`, and `fail` under the shared grammar.

#### External review card flow

Inner flow:

```text
pending → running → revise/tool gate → passed/superseded
```

Card:

```text
REVIEW
◌ pending 1  ◆ running 0
◇ revise 0   ✓ passed +0
△ wait 0     ▲ tool-fault 0
```

Mapping:

- `◌ pending`: pending review rows awaiting dispatch.
- `◆ running`: active review runner.
- `◇ revise`: semantic review rejection pressure; gates task progress.
- `✓ passed`: recent successful reviews.
- `△ wait`: retry delay / capacity wait, if distinct from pending.
- `▲ tool-fault`: `tooling_held` / runner infrastructure failure.

Hide `runner=unknown`, `held_reason=none`, and `next_retry_at=none` in row lists.

#### Engine card flow

Engine is not a store of business rows in the same way, but it still needs to follow the same visual grammar.

Inner flow:

```text
dispatch candidates → live runners → locks/gates → clear/healthy
```

Card in manual/test mode:

```text
ENGINE
◌ dispatch 0  ◆ runners 0
◇ locks 0     ✓ clear
△ manual      ▲ fault 0
```

Card when daemon is expected and unhealthy:

```text
ENGINE
◌ dispatch ?  ◆ runners ?
◇ locks 23    ✓ clear 0
△ wait 0      ▲ daemon down
```

Mapping:

- `◌ dispatch`: pending daemon/dispatch work.
- `◆ runners`: live runner processes.
- `◇ locks`: active locks / resource contention.
- `✓ clear`: no pressure in the relevant engine surfaces.
- `△ manual`: daemon off but not currently harmful; manual/test mode.
- `▲ fault`: stale locks, daemon down when required, runner/process health faults.

Implementation may need a conservative heuristic first:

- If daemon dead, locks clear, and no daemon-owned active dispatch appears required: render `△ manual` / `✓ clear`.
- If daemon dead and stale locks or daemon-owned active dispatch exists: render `▲ daemon down`.

## Task row rendering

### Current row problem

Current rows mix five vocabularies:

```text
T010 blocked:runner active:none:none runner:none age:37m tier:T3 · reason:{...}
```

This should become one operator stage and one signal:

```text
T010 ▲ runner-failed  exit 42  age:37m tier:T3 · synthetic fake runner nonzero blocked task
```

### Proposed task columns

Keep density but use meaningful columns:

```text
ID     STAGE           SIGNAL              AGE   TIER  TITLE
T002   ◆ plan          no worktree          41m   T3    synthetic active planning task
T003   ◇ plan-gate     plan ◐               41m   T3    synthetic task paused in plan review
T004   ▣ exec          phase 1/1            41m   T3    synthetic ready task awaiting coding
T009   ▰ accept        review pending       37m   T3    stores test live happy-path
T010   ▲ runner-failed exit 42              37m   T3    synthetic fake runner nonzero blocked task
```

If the existing table layout cannot add headers easily, the row text should still follow this grammar:

```text
T010  ▲ runner-failed · exit 42 · age:37m · T3 · synthetic fake runner nonzero blocked task
```

### Task stage mapping

Use high-quality glyphs, not emojis:

| ADR 0001 tuple / condition | Stage glyph | Stage label | Notes |
|---|---:|---|---|
| `lifecycle=queued`, `activation=inactive` | `◌` | `queued` | quiet; not armed |
| `lifecycle=queued`, `activation=active`, no worktree | `◌` or `◇` | `queued` / `needs scaffold` | signal should say no scaffold |
| `lifecycle=active`, `active_step=planning` | `◆` | `plan` | planner next/running/completed |
| `lifecycle=active`, `active_step=planning_review` | `◇` | `plan-gate` | plan-review pressure |
| `lifecycle=active`, `active_step=coding` | `▣` | `exec` | executor work |
| `lifecycle=active`, `active_step=coding_review` | `◈` | `code-gate` | code-review pressure |
| `lifecycle=active`, `active_step=wrapping` | `▰` | `accept` | wrap / external review / human acceptance |
| `lifecycle=integration` | `▱` | `ship` | integration lane |
| `lifecycle=done` | `■` | `done` | exhaust; hidden by default |
| `blocked=true`, `blocker_kind=runner` | `▲` | `runner-failed` | signal from blocked_reason JSON/log |
| `blocked=true`, `blocker_kind=task_review` | `▲` | `review-blocked` | plan/code review failed |
| `blocked=true`, `blocker_kind=capacity` | `△` | `waiting-capacity` | not a failure |
| `blocked=true`, `blocker_kind=dependency` | `△` | `waiting-dependency` | not a failure |
| `blocked=true`, `blocker_kind=human_acceptance` | `⋯` | `needs-human` | U-gate pressure |
| `blocked=true`, other blocker | `▲` | `<kind>-blocked` | translate per kind |

Precedence:

1. Terminal/done -> exhaust unless `--all`.
2. Blocked -> blocker semantic stage.
3. Integration -> ship stage.
4. Active -> active_step stage.
5. Queued -> queued/scaffold/armed stage.

### Blocker semantic mapping

Current enum:

```text
capacity, dependency, runner, rate_limit, human_acceptance, task_review,
stale_base, config, test_failure, main_red, deploy, migration
```

Proposed row labels:

| blocker_kind | Row label | Signal examples |
|---|---|---|
| `capacity` | `waiting-capacity` | `capacity`, `daemon off`, `no scaffold` |
| `dependency` | `waiting-dependency` | `blocked by T###` |
| `runner` | `runner-failed` | `exit 42`, `no heartbeat`, `payload invalid` |
| `rate_limit` | `rate-limited` | `provider limit`, `retry 12m` |
| `human_acceptance` | `needs-human` | `accept/reject`, `resume` |
| `task_review` | `review-blocked` | `NOT_READY`, `REVISE cap`, `FAIL` |
| `stale_base` | `stale-base` | `review base stale` |
| `config` | `config-fault` | `missing pre-land check` |
| `test_failure` | `tests-failed` | pre-land/test summary |
| `main_red` | `main-red` | main gate failed |
| `deploy` | `deploy-failed` | deploy step failed |
| `migration` | `migration-failed` | schema migrate failed |

Avoid generic `held` unless a row is intentionally parked and not failed/waiting.

### Hide empty fields in task rows

Default task rows must not show:

- `runner:none`
- `active:none:none`
- `lifecycle=... active_step=... integration_step=...`
- `workspace:none`
- `held_reason=none`
- `next_retry_at=none`

Instead:

- show runner/model only if a live or latest relevant run exists;
- show `no worktree` only if it matters for next action;
- show raw tuple only in detail/debug pane.

## Observation row rendering

### Current problem

Observation rows use legacy statuses directly:

```text
L006 investigating high synthetic architecture-risk contract draft
L003 needs_info normal synthetic observation needing info
```

This is readable, but not yet semantically aligned with ADR 0002.

### Proposed observation columns

```text
ID     STATE          CONTRACT     PRIORITY/RISK   SIGNAL              SUMMARY
L009   ◌ candidate    none         high            needs triage         synthetic linked-task blocker observation
L001   ◌ candidate    none         high            needs triage         synthetic high-priority open observation
L006   ◆ investigate  draft        high/arch       stale-base-er        synthetic architecture-risk contract draft
L005   ◆ investigate  draft        normal          contract draft       synthetic investigated draft contract
L003   ⋯ info-needed  none         normal          waiting              synthetic observation needing info
L007   ◆ investigate  none         normal          requested            synthetic wait-for-investigation observation
L010   ◆ investigate  none         normal          cluster:zombie       synthetic cluster-noise observation
```

Recent closed rows can appear in exhaust only:

```text
RECENT EXHAUST
L004   ✓ addressed    commit deadbee
L008   × wont-fix
```

### Observation stage mapping

| ADR 0002 fields / condition | Glyph | Label |
|---|---:|---|
| `lifecycle=candidate`, no draft | `◌` | `candidate` |
| `lifecycle=candidate`, investigating/evidence | `◆` | `investigate` |
| `waiting_kind=info_needed` | `⋯` | `info-needed` |
| `waiting_kind=human_ratification`, draft contract | `◈` | `contract-draft` |
| `contract_state=approved` or nested ready/approved | `▰` | `contract-approved` |
| `lifecycle=in_progress` | `▣` | `resolving` |
| `lifecycle=closed`, outcome addressed | `✓` | `addressed` |
| `lifecycle=closed`, outcome/wont_fix | `×` | `wont-fix` |
| `pending_architecture_review` / open architecture review | `◈` | `arch-gate` |

Use `contract-approved`, not `contract-ready` or bare `ready`, because ADR 0002 primary `contract_state` is `approved` and `ready` means different things in tasks and observations.

## Intake row rendering

Current clean DB has no intake rows. Still define the language now.

### Proposed intake card/table vocabulary

```text
INTAKE · routing
ID     STATE        AGE   SOURCE        ROUTE/RISK        SUMMARY
I041   ◌ new        6d    executor      watch-ux          watch output hides engine shape
I042   ◆ triage     2h    orchestrator  lifecycle         schema drift surfaced
I043   ⋯ info       1h    gatekeeper    needs evidence    confirm duplicate target
I044   ✓ routed     4m    gatekeeper    observation L###  routed to observation
I045   × dropped    2m    gatekeeper    noise             dropped as noise
```

Mapping:

| Intake lifecycle/outcome | Glyph | Label |
|---|---:|---|
| `new` / draft | `◌` | `new` |
| `triaging` | `◆` | `triage` |
| `waiting` / evidence_needed | `?` | `needs-info` |
| `closed` routed_to_observation | `✓` | `routed` |
| `closed` escalated_to_architecture_review | `◈` | `arch-review` |
| `closed` marked_duplicate | `≡` | `duplicate` |
| `closed` dropped_as_noise | `×` | `dropped` |

## External review row rendering

### Current problem

```text
ER001 pending task=T009 runner=unknown held_reason=none attempts=0 next_retry_at=none liveness=pending
```

This should be:

```text
ER001 ◌ pending  T009  runner —  waiting for review dispatch
```

### Proposed external review table

```text
ID      STATE        TASK   RUNNER   AGE   SIGNAL
ER001   ◌ pending    T009   —        37m   waiting for dispatch
ER002   ◆ running    T011   fake     2m    heartbeat 4s
ER003   ✓ passed     T012   codex    1m    0/0/0 findings
ER004   ↻ revise     T013   codex    5m    2 major findings
ER005   ▲ tool-fault    T014   fake     8m    missing wrap brief
ER006   ■ superseded T015   codex    1h    replaced by ER007
```

Mapping:

| ER status | Glyph | Label |
|---|---:|---|
| `pending` | `◌` | `pending` |
| `running` | `◆` | `running` |
| `passed` | `✓` | `passed` |
| `revise` | `↻` | `revise` |
| `tooling_held` | `▲` | `tool-fault` |
| `superseded` | `■` | `superseded` |

Hide:

- `runner=unknown` -> render `—` or omit.
- `held_reason=none` -> omit.
- `next_retry_at=none` -> omit.
- `liveness=pending` when it duplicates pending state.

## Detail pane

Blake likes the detail pane. Do not remove detail. Improve it.

Task detail should keep:

- story summary;
- done_when / executive intent;
- current state;
- next action;
- linked observations;
- artifact pointers;
- recent events;
- live runner/log preview.

Add or preserve a debug block, lower priority:

```text
Debug tuple
  status: planning
  lifecycle: queued
  active_step: none
  integration_step: none
  activation: inactive
  blocked: false
  blocker_kind: —
```

The debug tuple belongs in detail, not the row list.

Most important detail requirement:

> Live updates and running logs are critical. If a runner is live, detail should prioritize live role, runner/model, heartbeat age, stdout/stderr/transcript paths, and a short rolling event/log preview.

Suggested task detail order:

1. Semantic state / next valve.
2. Live runner/log panel if present.
3. Contract/story summary.
4. Progress map / review loops.
5. Links/artifacts.
6. Recent events.
7. Debug tuple.

## Detail examples

### Task detail for T010

```text
Task detail · T010

State
  ▲ runner-failed · planner · exit 42
  next: inspect run log or resume after fixing runner issue

Live / latest runner
  role: planner
  runner: fake
  status: failed
  stdout: .../b8273327.jsonl
  stderr: .../b8273327.stderr.log
  last event: heartbeat before nonzero exit

Story
  synthetic fake runner nonzero blocked task

Why it matters
  done_when: Fake runner nonzero case blocks visibly.

Debug tuple
  status: blocked
  lifecycle: active
  active_step: none
  integration_step: none
  blocked: true
  blocker_kind: runner
  blocked_reason: {"exit_code":42,"kind":"runner_crash"}
```

### External review detail for ER001

```text
External review detail · ER001

State
  ◌ pending · waiting for dispatch
  task: T009

Review target
  runner: —
  base/head: —
  attempts: 0

Next
  daemon/manual review runner must claim this row

Debug tuple
  status: pending
  runner: NULL
  held_reason: NULL
```

## Implementation sketch

Likely files:

- `src/tui/data.rs`
- `src/tui/render.rs`
- `src/tui/detail.rs`
- `src/cli/watch.rs` for legacy rendering parity
- tests in existing TUI/watch modules

### Step 1: Introduce semantic presentation structs

Create pure functions near `tui::data` or a new `tui::semantics` module:

```rust
struct TaskPresentation {
    glyph: &'static str,
    label: String,
    signal: Option<String>,
    severity: PresentationSeverity,
    debug_tuple: DebugTuple,
}

fn task_presentation(task: &TaskRow) -> TaskPresentation;
fn observation_presentation(row: &ObsRow) -> ObservationPresentation;
fn intake_presentation(row: &IntakeRow) -> IntakePresentation;
fn external_review_presentation(row: &ExternalReviewRow) -> ReviewPresentation;
```

Keep these pure and heavily tested. They are the semantic contract between schema and UI.

### Step 2: Replace task row raw tuple rendering

Default TUI rows should use presentation fields.

Acceptance:

- Default task rows do not contain `active:none:none`.
- Default task rows do not contain `runner:none`.
- Default task rows do not contain `lifecycle=... active_step=... integration_step=...`.
- Blocked rows display semantic labels like `runner-failed`, `review-blocked`, `waiting-capacity`.
- Existing detail pane still exposes raw tuple in debug section.

### Step 3: Implement shared-flow top lane cards

Keep lane cards, but make every card a store-specific projection of the same flow grammar.

Acceptance:

- Each card has front/work/gate/exit/wait/fault slots, using the canonical glyphs.
- No top card says `held` as a generic category.
- Tasks card distinguishes `△ wait` from `▲ fail`.
- Engine card distinguishes non-failure manual mode (`△ manual`) from actionable daemon failure (`▲ daemon down`).
- External review card uses `◌ pending`, `◆ running`, `◇ revise`, `✓ passed`, `▲ tool-fault`.

### Step 4: Observation/intake/external-review row semantics

Apply the same presentation pattern to non-task rows.

Acceptance:

- Observation rows show `candidate`, `investigate`, `info-needed`, `contract-draft`, `contract-approved`, `addressed`, `wont-fix`, or `arch-gate`.
- Intake rows show `new`, `triage`, `needs-info`, `routed`, `duplicate`, `dropped`, or `arch-review`.
- External review rows hide `runner=unknown`, `held_reason=none`, `next_retry_at=none`.

### Step 5: Detail pane enhancements

Keep current detail richness, but order it by operator meaning.

Acceptance:

- If a live/latest runner exists, detail shows it prominently.
- Debug tuple appears lower in detail, not row list.
- Recent events remain visible.

### Step 6: Regression fixtures from clean DB

Add tests using rows like the current seeded clean DB:

- queued inactive task;
- active planning task;
- plan review task;
- executing task;
- in_review / acceptance task with pending ER;
- runner-blocked task with JSON blocked reason;
- capacity-blocked queued task;
- candidate observation with no contract;
- draft contract observation;
- needs-info observation;
- pending ER with null runner.

Test assertions should be semantic, e.g.:

- contains `runner-failed`;
- contains `exit 42`;
- does not contain `runner:none`;
- does not contain `active:none:none`;
- does not contain `held_reason=none`.

## Proposed glossary

Use the same words everywhere.

### Cross-store pressure words

| Word | Meaning |
|---|---|
| `new` | unclaimed raw input |
| `triage` | classification/routing in progress |
| `candidate` | real issue/signal but not yet contract-approved |
| `investigate` | evidence/contract shaping in progress |
| `needs-info` | blocked on a narrow question/evidence need |
| `contract-draft` | intent exists but not approved |
| `contract-approved` | human-approved contract / promotable |
| `plan` | task planning stage |
| `plan-gate` | plan review stage |
| `exec` | executor/coding stage |
| `code-gate` | code review stage |
| `accept` | wrap/external review/human acceptance pressure |
| `ship` | integration lane |
| `waiting-*` | not failed; waiting on capacity/dependency/info |
| `*-failed` | tooling/runner/test/deploy/migration failure |
| `blocked` | generic fallback only when no better typed label exists |
| `done` | terminal/exhaust |

Avoid:

- `held` as a generic bucket;
- `active` without saying active doing what;
- `open` as a top-card headline;
- `ready` without qualifier;
- `runner:none`;
- raw schema tuple text in row lists.

## Resolved semantic decisions from Oracle review

1. Task `wrapping` is labeled `accept` in the process table, with the signal carrying `wrap ready`, `review pending`, `review passed`, or `needs Blake`.
2. Capacity is labeled `waiting-capacity`, not `queued-capacity` or `capacity-wait`; it is a non-failure waiting class.
3. The observations top-card headline is `needs-contract`; row labels can still use `candidate`, `investigate`, `contract-draft`, and `contract-approved`.
4. Engine mode uses conservative labels until explicit config exists: `engine: manual` when daemon is off and locks are clear, `engine: daemon down` when stale locks or daemon-owned work make absence actionable, and `engine: unknown` when impact cannot be inferred.
5. Task top-card pressure splits `waiting` and `failed`; do not use one generic `blocked` count in the top card.

## Oracle review incorporation

Oracle reviewed this plan after the initial draft and recommended tightening the glossary so the UI reinforces ADR 0001 / ADR 0002 primary terms instead of preserving transitional vocabulary. Blake then tightened the top-card requirement further: lane cards must keep store separation while using one shared flow grammar across all stores. This note incorporates both rounds. The key decisions are:

- Preserve separate lane cards for intake, observations, tasks, review, and engine.
- Make every top lane card follow the same meta-pattern: `◌ front`, `◆ work`, `◇ gate`, `✓ exit`, `△ wait`, `▲ fault`.
- Use store-local labels inside that shared pattern (`new/triage/info/routed`, `candidate/investigate/contract/closed`, `queued/work/gate/done`, `pending/running/revise/passed`, `dispatch/runners/locks/clear`).
- Use `contract-approved`, not `contract-ready`; ADR 0002 primary contract enum is `none | draft | approved`, while nested legacy `ready` aliases to approved.
- Split task pressure into `waiting` and `failed`; do not use top-card `held` or generic `blocked` when a clearer semantic class exists.
- Use `tool-fault` for external-review tooling holds.
- Use `engine: manual`, `engine: daemon`, `engine: daemon down`, and `engine: unknown` in detail text, while the top card uses the shared `△ manual` / `▲ daemon down` grammar.
- Reserve `held` only for explicit durable hold/park semantics. It is forbidden as a generic bucket for failed, waiting, blocked, pending, or queued rows.
- Use portable high-quality glyphs only; no emojis. Prefer `⋯` for needs-info / needs-human and reserve `▲` for actual faults.

### Canonical portable glyph set

```text
◌  queued / new / pending
◆  active work / investigate / running
◇  gate/check stage
◈  architecture or contract gate
▣  execution
▰  accept / approved / ready valve
▱  ship / integration
■  done / superseded
△  waiting / non-failure pressure
▲  fault / failed / blocked-by-error
✓  passed / addressed / routed success
×  dropped / wont-fix / terminal negative
↻  revise / retry / loop
⋯  needs-info / needs-human
≡  duplicate
—  unknown/none when a column must be present
```

### Final canonical glossary

Intake:

- `new`
- `triage`
- `needs-info`
- `routed`
- `duplicate`
- `arch-review`
- `dropped`

Observations:

- `candidate`
- `investigate`
- `needs-info`
- `contract-draft`
- `contract-approved`
- `arch-gate`
- `resolving`
- `addressed`
- `wont-fix`
- `superseded`

Tasks:

- `queued`
- `needs-scaffold`
- `plan`
- `plan-gate`
- `exec`
- `code-gate`
- `accept`
- `ship`
- `done`
- `waiting-capacity`
- `waiting-dependency`
- `needs-human`
- `rate-limited`
- `runner-failed`
- `review-blocked`
- `stale-base`
- `config-fault`
- `tests-failed`
- `main-red`
- `deploy-failed`
- `migration-failed`

External review:

- `pending`
- `running`
- `passed`
- `revise`
- `tool-fault`
- `superseded`

Engine:

- `engine: manual`
- `engine: daemon`
- `engine: daemon down`
- `engine: unknown`

## Completion criteria for the implementation task

The implementation is complete when, against the current clean seeded DB:

- The top lane cards are still separate store cards.
- Every top lane card follows the shared flow grammar: front/work/gate/exit/wait/fault.
- Task rows no longer display raw lifecycle tuples or `runner:none`.
- Blockers are semantic and typed.
- Observations show contract/routing semantics.
- External reviews hide null/none clutter.
- Engine health distinguishes daemon-off engine: manual from daemon-required failure.
- Detail panes retain raw debug tuple and live runner/log detail.
- Tests lock the semantic strings/glyphs for representative row shapes.
