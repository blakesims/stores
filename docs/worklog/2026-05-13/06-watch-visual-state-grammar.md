# Watch Visual State Grammar

**Date:** 2026-05-13
**Type:** note

## Summary

Captured the emerging `stores watch` visual-state grammar: use few shape families, but combine them densely with position, fill, superscript cycle count, color, and subtle animation. This is the design primitive for task phase maps and should inform other store views.

## Details

Core design rule:

```text
shape       = workflow family
position    = phase index / pipeline position
fill        = substate inside that family, especially work vs review/gate
superscript = cycle count
color       = review/result/pressure state: active, passed, failed, waiting
animation   = currently active cell, gentle breathing only
```

This intentionally avoids multiplying glyph geometries. One logical phase gets exactly one visual slot. The slot's fill/color/superscript conveys substate.

### Glyph families

Planning is a circle family:

```text
◌  queued / pre-planning / not started
○  planning, still changing the plan
●  plan review / plan gate / plan result
◉  possible alternate for reviewed circle with visible infill gap (U+25C9 FISHEYE)
◎  possible alternate for ring/bullseye where inner/outer distinction matters
```

Execution is a square family:

```text
·  planned phase exists but has not been reached
□  executing this phase
▣  code review / phase review / phase result
▰  wrap / acceptance valve, if shown as final slot
```

Pressure/fault remains triangle family:

```text
△  waiting / non-failure pressure
▲  fault / failed / blocked by error
```

### Superscript cycle count

Cycle 1 has no superscript. Cycle 2+ gets standard superscript numerals:

```text
○    planning cycle 1
○²   planning cycle 2
○³   planning cycle 3
●³   plan review cycle 3
□²   executing phase cycle 2
▣²   reviewing phase cycle 2
▣¹²  reviewing phase cycle 12
```

Superscripts are unbounded and compact. Minor horizontal jitter at double digits is acceptable.

### Color semantics

Color carries result and activity:

```text
dim gray       not reached / inactive
blue/teal      active work: planning or executing
peach/yellow   active review/gate, waiting pressure
green          review passed / completed slot
red            review failed / fault
```

A gentle breathing animation can mark the currently active cell. It should be subtle and should not animate completed history.

Color must not be the only truth. The selected detail pane should spell out the current state, cycle, and failure reason for accessibility and monochrome fallback.

### One slot per logical phase

Do not render both work and review for the same phase. If a plan is in review cycle 3, render one planning slot:

```text
●³
```

Do **not** render:

```text
○³ ●³
```

The filled circle already implies planning happened and is now in review/result state.

Likewise for execution phase 2 review cycle 2:

```text
✓? no — avoid extra check geometry
▣² in the phase-2 position, colored peach/green/red depending active/pass/fail
```

The phase position plus glyph state is the source of truth.

### Planned phase dots

The plan creates the number of execution phases, from 1..N. The map should show that shape immediately:

```text
○ │ ·              one planned phase, planning now
● │ □ · ·          three planned phases, phase 1 executing
● │ ▣ □² ·         phase 1 reviewed/passed, phase 2 executing cycle 2
● │ ▣ ▣² ·         phase 2 in review cycle 2
● │ ▣ ▣ ▰          phases complete, acceptance/wrap
```

The dots are important: they show how many phases exist before those phases start.

### Candidate task row shape

Task rows should prioritize task identity and task title, then one dense state map:

```text
ID     SUMMARY                                             MAP          REASON   AGE  TIER
T001   synthetic queued inactive plan task                  ◌ │ · · ·             7h   T3
T002   synthetic active planning task                       ○ │ · · ·             7h   T3
T003   synthetic task paused in plan review                 ● │ · · ·             7h   T3
T004   synthetic ready task awaiting coding                 ● │ □ · ·             7h   T3
T011   retrying phase two after review                      ● │ ▣ □² ·            2h   T3
T012   phase two back in review                             ● │ ▣ ▣² ·            2h   T3
T009   stores test live happy-path                          ● │ ▣ ▣ ▰             7h   T3
T007   synthetic observation-linked capacity wait           △                    7h   T2
T010   synthetic fake runner nonzero blocked task           ▲           runner   7h   T3
```

The `SUMMARY` column is the task title, not status prose. It is first-class and bounded/truncated by table width.

### Design doctrine extracted

Use few shapes, densely:

- Prefer adding a dimension (position, fill, superscript, color, animation) over adding another glyph family.
- Keep one visual slot per logical lifecycle unit.
- Use text for row identity, summary, age, tier, and exceptional reason; use graphics for lifecycle position.
- Preserve raw/debug fields in detail panes, not scan rows.

## Follow-ups

- Implement task focused rows as an aligned table with columns like `ID | SUMMARY | MAP | REASON | AGE | TIER`.
- Audit `tasks.cycles`, `current_phase`, `current_cycle`, `plan_review_log`, and transition history to determine how much of the historical map can be rendered honestly.
- Add a legend/detail explanation for the map grammar.
- Consider whether the same dimensional grammar can map to observations/intake/reviews without forcing task-specific concepts onto them.
