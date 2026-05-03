# Stores Watch POC and Topology Discussion

**Date:** 2026-05-03
**Type:** note

## Summary

Built a quick `stores watch` POC: a top-level subcommand that opens `.stores/db.sqlite` read-only and renders a refreshing terminal frame (1 s default) showing tasks-by-status and recent observations, color-coded, with claimed-by attribution and `HH:MM:SS` updated-at suffixes. ~240 LOC, zero new deps, ANSI escapes only. Committed as `859750a`. The visceral observation: it's simple but it does *exactly* what I want — I can finally see whether anything is happening in the substrate without running ad-hoc `sqlite3 …` queries.

This validates the TUI direction over webapp/Swift for the observability story (see also `01-…` and `02-…` notes, if present).

## Details

### What `stores watch` ships

- New top-level subcommand (sibling to `init`/`setup`/`auth`), not per-store.
- Read-only SQLite open with `SQLITE_OPEN_READ_ONLY | SQLITE_OPEN_NO_MUTEX`. Polls; doesn't use `sqlite3_update_hook`.
- Two panels, hardcoded for now: `tasks` (sorted active-first by workflow position, max 20) and `observations` (open-first, max 10).
- Color discipline: green = executing/code_review, cyan = ready, yellow = planning/review, red = blocked/high-priority, dim = complete/low-priority.
- Frame written in one shot to minimise tearing; ANSI `\x1b[2J\x1b[H` clear-and-home each tick.
- `--interval` flag (default 1 s, floor 100 ms).

### Honest POC shortcomings (filed in the file's top comment)

1. Hardcoded to `tasks` + `observations`. Doesn't iterate `manifest.stores` and detect a "title-like" field. A real impl would render any installed store generically.
2. Polls on a timer; no push. Sound-on-event needs `sqlite3_update_hook` in a daemon, or the writer emitting events on a socket.
3. No `--workspace` for cross-worktree aggregation. Single-DB-only.
4. No interactivity (filter / sort / drill-in / pause).
5. Clock printed in UTC because I didn't want to pull `chrono` for a POC.

### Topology question — what's possible

User asked: same energy, but for the *topology* of stores — transitions, structure, where rows are flowing. Three flavors are useful and stack on top of each other:

**A. Static schema topology (`stores topology` or `stores tasks topology`).**
Read each store's `lifecycle.transitions` from schema.yaml, emit a Mermaid `stateDiagram-v2` (or graphviz `dot`) with state nodes, transition edges labelled `verb (actor [+ gate])`, and cross-store soft-FK edges (observations.task_id → tasks.display_id). Deterministic from schema; no runtime data. ~half a day, ~150 LOC. Output goes to stdout — paste into a docs page or pipe to `mmdc`. Gives a permanent reference for "what does this state machine even look like."

**B. Live count panel added to `watch`.**
For each store, render counts per state in workflow order:
```
tasks:        planning(2) → plan_review(0) → ready(1) → executing(3) → code_review(0) → in_review(1) → complete(8) | blocked(0)
observations: open(7) → investigating(1) → confirming(0) → claiming(0) → resolved(3) | rejected(0)
```
That single line per store turns the existing dashboard from "list of rows" into "where is the herd grazing." ~50 LOC additive change to `watch.rs`. Pure GROUP BY status.

**C. Transition stream.**
The most useful thing for the original "did anything happen" need. Diff the previous frame against the current: for each `display_id`, compare its prior `status` vs. its new `status`, and append a line to a ring buffer of recent events:
```
06:24:12  T013  executing → code_review     (ai_autonomous)
06:24:08  L012  open      → investigating
06:23:51  T014  ready     → executing
```
~150 LOC. Naturally extends to the eventual sound-on-event Mac app — same event shape, different sink.

**Stretch ideas if we keep going:**
- Per-state dwell-time histograms (where do tasks get stuck? computed from `cycles[]` JSON).
- Cross-store reference graph (`depends_on` chains, observation→task lineage).
- Schema-diff visualizer (when transitions appear/disappear between commits).
- Agent attribution heatmap (claimed-by × status).

### Recommended sequencing

1. **Today's POC** — `watch` (done).
2. **A + B together** — static topology emitter + live count panel. Low cost, high ratio. Two evening sessions.
3. **C — transition stream** — once (1) and (2) prove the model. This is the bridge to push-based observability and the eventual menu-bar app.
4. **Promote to a substrate task only after C lands.** Then the proper rebuild with `ratatui`, manifest iteration, and `sqlite3_update_hook` is justified. Not before — the POC is doing its job of teaching us what we actually want.

## Follow-ups

- File observation: `watch` is hardcoded to `tasks` + `observations`; doesn't iterate manifest. (T1 / fix-in-place when promoted.)
- File observation: `watch` polls instead of using `sqlite3_update_hook`; blocks the eventual sound-on-event ergonomics. (T2 / design decision for follow-on task.)
- File observation: `watch` clock is UTC; user expected local time. (T1 / cosmetic.)
- Decide whether to build A+B in this session or defer. Either is fine; both are < 1 day each.
- If A+B+C all land, write a refs doc (`docs/observability.md`) summarising the substrate's observability surface and the event shape for downstream consumers (TUI, webapp, Mac app).
