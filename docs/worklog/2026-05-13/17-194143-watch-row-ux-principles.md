# `stores watch` Focused-Row Table UI/UX Principles

## Core diagnosis

The current row-density direction is still too much like formatted prose and not enough like a terminal cockpit table. A btop-like UI works because it has **true columns**, **stable horizontal scan paths**, **strong visual hierarchy**, and **compact symbolic state**. The next `stores watch` focused-row design should stop emitting rows as “ID + semantic phrase + bag of facts” and instead render each focused store as a real, aligned, colored monospaced table.

## Non-negotiable principles

### 1. A table must be a table

Column headings and row values must align exactly in monospaced cells.

Requirements:

- Every visible column has a fixed or computed width.
- Header labels start at the same x-coordinate as the corresponding row values.
- Numeric/compact columns use stable widths and preferably right alignment where useful.
- Long text columns truncate/ellipsize inside their budget; they do not push other columns.
- The focused table should use the available width, not cluster content near the left.
- The summary column is the flex column: all extra width goes there first.

Anti-pattern:

```text
ID     STAGE              SIGNAL          AGE   PRI   SUMMARY
T002   ◆ plan             no worktree     6h    T3    synthetic active planning task
```

This is only acceptable if it is actually rendered with fixed cell widths. In HTML/specs, use real table layout or exact monospaced grid; do not fake it with loosely spaced text.

### 2. Do not make the operator decode prose bags

Each cell should answer one question.

Bad row shape:

```text
T002  ◆ plan no worktree       not scaffolded age:6h tier:T3 · workspace:none · synthetic...
```

Problems:

- stage and readiness are mixed;
- `no worktree`, `not scaffolded`, and `workspace:none` repeat one concept;
- `age` and `tier` are hidden in prose;
- summary competes with debug facts;
- JSON/debug appears in the scan row.

Better principle:

```text
ID     FLOW        STEP        CHECKS       AGE   TIER  SUMMARY
T002   ◆ work      ◆ plan      □wt □sc ✓pl   6h    T3    synthetic active planning task
```

### 3. Separate broad flow, task step, readiness, and detail

For tasks, the top-card slot is not enough. Tasks have a richer internal execution shape:

- broad flow bucket: queued / working / gate / waiting / failed / done;
- task step: plan / plan-gate / exec / code-gate / accept / ship;
- phase/cycle progress: phase N/M, cycle N, review loop, wrap/external review;
- readiness/checks: worktree, scaffold, plan, branch, runner/log, review state;
- summary: task title/story.

Do not collapse all of this into `SIGNAL`.

### 4. `priority` and `tier` are orthogonal

Do not use one column named `PRI` for both observation priority and task tier.

- Task `T3` = tier / size / complexity / cycle shape. It is not priority.
- Observation `high/normal/low` = priority/risk/importance. It is not task tier.

Implication:

- Task table column should be named `TIER`, `SIZE`, or `CLASS`, not `PRI`.
- Observation table column should be named `PRIO` or `RISK`.
- If a future universal table exists, the column must be store-local, e.g. `CLASS`, with task rendering `T3` and observation rendering `high`; but this loses precision. Prefer store-specific column labels in focused tables.

### 5. Readiness should be visual checks, not vague text

`no worktree`, `not scaffolded`, and `workspace:none` are implementation words. They should become a compact readiness/check cell.

Possible task check glyphs:

```text
✓ present / satisfied
□ missing / not yet
◐ partial / in progress
! fault
— not applicable / unknown
```

Possible task readiness columns/checks:

```text
WT  worktree exists
SC  scaffolded
PL  plan exists/approved
BR  branch/workspace ready
RV  review/external review state
RN  runner/live run/log present
```

Compact cell examples:

```text
□wt □sc —pl        queued, no workspace yet
✓wt ✓sc ◐pl        planning/review in progress
✓wt ✓sc ✓pl        ready for execution
✓wt !rv            review failed
!rn                runner failed
```

If abbreviations are used, the header must define them. Example:

```text
CHECKS
WT SC PL RV RN
□  □  —  —  —
```

or a legend line/detail pane must be available. Avoid unexplained `signal: no worktree`.

### 6. Summary is first-class, but bounded

The summary/title/story is not optional. It is the main human context. But it must live in one flex column with a hard budget.

Rules:

- Summary is a column, not an afterthought after `·` separators.
- Summary gets remaining width after fixed columns.
- Summary truncates with a visible ellipsis if needed.
- Debug details never steal summary width in the row.
- The detail pane expands the selected row’s full summary/story/debug.

### 7. Color should reinforce column semantics and severity

Use btop-like visual hierarchy:

- Header row: brighter/bold, maybe yellow/blue depending theme.
- Section headers: cyan/blue, strong but not louder than faults.
- ID column: muted/magenta, stable.
- Flow/stage glyphs: colored by semantic severity.
  - work: blue/teal;
  - gate: mauve/yellow;
  - wait: yellow/peach;
  - fault: red/bold;
  - done/exhaust: green/dim.
- Checks:
  - ✓ green;
  - □ dim gray;
  - ◐ yellow;
  - ! red.
- Age: dim normally, yellow/red only if stale.
- Tier/priority: color only if meaningful for attention; do not over-color every cell.

Color should not be the only carrier of meaning. Glyph + label/check must still work monochrome.

### 8. Use glyphs where they encode real state, not decoration

Task glyphs should carry existing semantics:

```text
◌ queued/front
◆ planning/work
◇ plan-gate/gate
▣ execution
◈ code/review gate
▰ accept/wrap
▱ ship/integration
△ waiting
▲ fault
✓ done/pass
```

Use them in the `FLOW`/`STEP` cells. Do not hide them in prose. Do not invent decorative glyphs that are not tied to a state.

## Task-focused table schema

Recommended task table columns:

```text
ID     FLOW       STEP        PHASE     CHECKS           AGE   TIER  SUMMARY
```

Column meanings:

| Column | Meaning | Examples |
|---|---|---|
| `ID` | task display id | `T002` |
| `FLOW` | top-card bucket / section bucket | `◆ work`, `◇ gate`, `△ wait`, `▲ fail` |
| `STEP` | task-specific step or subtype | `◆ plan`, `▣ exec`, `◇ plan-gate`, `runner`, `capacity` |
| `PHASE` | progress shape, not prose | `1/3`, `cycle2`, `wrap`, `—` |
| `CHECKS` | readiness/status checks | `□wt □sc ◐pl`, `✓wt ✓sc ✓pl`, `!rn` |
| `AGE` | age/staleness | `6h`, `2d` |
| `TIER` | complexity/tier, not priority | `T1`, `T2`, `T3` |
| `SUMMARY` | bounded first-class title/story | truncated text |

Possible wide rendering:

```text
TASKS                                                            sort: flow → age
ID     FLOW      STEP          PHASE   CHECKS              AGE   TIER  SUMMARY
▾ QUEUED (2)
T001   ◌ queue   —             —       □wt □sc —pl —rv     6h    T3    synthetic queued inactive plan task
T006   ◌ queue   —             —       □wt □sc —pl —rv     6h    T2    synthetic abandoned queued task

▾ WORKING (2)
T002   ◆ work    ◆ plan        —       □wt □sc ◐pl —rv     6h    T3    synthetic active planning task
T004   ◆ work    ▣ exec        1/1     ✓wt ✓sc ✓pl —rv     6h    T3    synthetic ready task awaiting coding

▾ GATE (2)
T003   ◇ gate    ◇ plan-gate   —       □wt □sc ◐pl —rv     6h    T3    synthetic task paused in plan review
T009   ◇ gate    ▰ accept      wrap    ✓wt ✓sc ✓pl ◐rv     6h    T3    stores test live happy-path

▾ WAITING (2)
T007   △ wait    capacity      —       □wt □sc —pl —rv     6h    T2    synthetic observation-linked capacity wait
T008   △ wait    capacity      —       □wt □sc —pl —rv     6h    T3    synthetic runner candidate wait

▾ FAILED (2)
T010   ▲ fail    runner        —       !rn                 6h    T3    fake runner nonzero blocked task
T005   ▲ fail    review        —       !rv                 6h    T3    synthetic plan rejected to create blocked state
```

Notes:

- `exit 42` is not a good primary table value by itself. It can be a detail/signal subvalue, but the table should first say `runner` + `!rn`. If width allows, `STEP` could be `runner:42`; otherwise detail pane shows `exit 42`.
- `workspace:none` should not appear in rows. It becomes `□wt`.
- `not scaffolded` should not appear as prose. It becomes `□sc`.
- If the selected detail pane shows raw paths/logs/reasons, the row can stay compact.

## Observation-focused table schema

Observations are not tasks. They need priority/risk, not tier/complexity as the main class.

Recommended observation table columns:

```text
ID     FLOW       STATE        NEED        AGE   PRIO  SUMMARY
```

Examples:

```text
OBSERVATIONS                                                     sort: flow → priority
ID     FLOW       STATE          NEED             AGE   PRIO  SUMMARY
▾ CONTRACT GATE (7)
L006   ◇ gate     draft          approve/revise   6h    high  synthetic architecture-risk contract draft
L009   ◇ gate     draft          approve/revise   6h    high  synthetic linked-task blocker observation
L005   ◇ gate     draft          approve/revise   6h    norm  synthetic investigated draft contract

▾ WAITING (1)
L003   △ wait     info-needed    answer info      6h    norm  synthetic observation needing info
```

Observation concepts:

- `PRIO` means priority/risk, not tier.
- `STATE` is candidate/investigate/draft/approved/info-needed/fault subtype.
- `NEED` is the next operator/review valve.
- Summary remains first-class.

## Anti-patterns to avoid

1. **Unaligned pseudo-tables**
   - Headers not aligned with values.
   - Variable prose before fixed facts.

2. **Generic `SIGNAL` dumping ground**
   - `signal: no worktree` and `ready: not scaffolded` make the operator interpret implementation state.

3. **Repeating one concept in three words**
   - `no worktree`, `workspace:none`, `not scaffolded` in the same row.
   - `contract-draft contract draft contract:draft`.

4. **Conflating priority and tier**
   - `T3` is not priority.
   - Observation `high` is not task complexity.

5. **Dropping summary**
   - Summary is first-class context and must remain in the row.

6. **Paragraph rows**
   - Avoid two-line explanatory prose by default.
   - Use detail pane for explanation.

7. **Raw debug in scan rows**
   - JSON, raw lifecycle tuples, paths, and `none` fields belong in detail/debug.

8. **Decorative glyphs**
   - Every glyph must map to state/check meaning.

## Implementation implications

- The focused table needs a real column layout engine, not string concatenation with arbitrary `·` separators.
- Column specs should be explicit per store or projection type:

```rust
ColumnSpec {
    key: "checks",
    title: "CHECKS",
    width: Fixed(14),
    align: Left,
    style_role: Checks,
}
```

- Rows should be converted into cells before rendering:

```rust
FocusedRowCells {
    id,
    flow,
    step,
    phase,
    checks,
    age,
    tier_or_priority,
    summary,
}
```

- The summary column should receive remaining width.
- Wide/narrow behavior should drop optional columns in order, never destroy alignment:
  1. Drop `PHASE` if too narrow.
  2. Compress `CHECKS` from labeled `□wt □sc ◐pl` to glyph-only under a header legend.
  3. Shorten `FLOW` label but keep glyph.
  4. Preserve `ID`, `STEP`, `AGE`, class (`TIER`/`PRIO`), and `SUMMARY` as long as possible.
