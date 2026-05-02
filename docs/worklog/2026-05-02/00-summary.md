# Daily Summary — 2026-05-02

## Overview

Two interleaved threads today: (1) a substantial discussion-as-design exercise on whether the `stores` substrate can take over Blake's real-world client `/task:open <L-id>` workflow, which produced a 4-task ship plan; and (2) execution of task #1 (T011 — pinning the substrate-vs-wrapper boundary in `docs/philosophy.md`), which shipped clean through planner, plan-reviewer, executor, code-reviewer, and CodeRabbit Stage 6.

The day starts with no formal task queue and ends with one task shipped + three queued (T012, T013, T014) with verified scoping and confirmed design decisions.

## Work Completed

- **T011 — Document the wrapper boundary in `docs/philosophy.md`** — shipped first cycle through every gate. Worklog entry: `02-t011-docs-wrapper-boundary.md`. Branch: `feat/T011-docs-wrapper-boundary` (stacked on `feat/T010-wrap-workflow`, neither yet on `master`).
- **Real-world workflow takeover analysis** — full analytical pre-step: transcript mapping, philosophy reread (corrected two of my own framings mid-discussion), 5 tensions worked through to decision, verification of 4 codebase items via Explore subagents, ship plan finalized and confirmed via AskUserQuestion. Worklog entry: `01-real-world-workflow-takeover-analysis.md`.

## Notes Today

| # | Note | Topic |
|---|------|-------|
| 01 | [real-world-workflow-takeover-analysis.md](./01-real-world-workflow-takeover-analysis.md) | The morning's design discussion. Maps the L310→T281 client workflow to stores capabilities; works through 5 tensions (A: mode-2 wrappers, B: notes propagation, C: recursive spawning, D: TID picking, E: workspace_path); produces 4-task ship plan with verified scoping. Read this first — every later artifact references it. |
| 02 | [t011-docs-wrapper-boundary.md](./02-t011-docs-wrapper-boundary.md) | T011 completion note. Captures what shipped, how the workflow ran (PASS first cycle on every gate), and 4 lessons (voice-match, single-phase plans, heading-drop verification, fill Completion before declaring COMPLETE). |

## Open Threads

- **T012 (next, queued and ready to start):** `workspace_path` field on tasks + `tasks next-id` verb, bundled. ~half-day each. See handover below.
- **T013 (queued):** Reviewer envelope + storage schema migration (binary severity for code-reviewer; new `notes`/`observations` fields on plan/code reviewer envelopes). Medium, ~10 files.
- **T014 (queued):** Framework write-path (envelope `observations[]` → `observations.add` with source pointer) + brief overlay + templates. Medium.
- **Tiny CLAUDE.md update worth doing:** pin "fill `## Completion` *before* setting `Status: COMPLETE`" in `tasks/CLAUDE.md` so CR Stage 6 doesn't catch it on every future task. Out of scope for T011; could be a one-line follow-up commit.
- **Stacked-branch CR caveat:** the workflow's Stage 6a rebase guidance assumes branching from main; for stacked branches (current pattern), use `cr review --base <parent-branch>`. Worth noting in the workflow docs if stacking stays routine.

## Tomorrow

T012 is the next planned ship. See handover below for everything the next agent needs.

---

## Handover: starting T012

The next agent picking this up should be able to start T012 from cold with what's in this section + the linked notes. Read in this order:

### 1. Read these first (in order)

1. `docs/worklog/2026-05-02/01-real-world-workflow-takeover-analysis.md` — the source of truth for *why* T012 exists and what shape it has. Specifically: (a) the **Tension D** subsection (TID picking via `stores tasks next-id`), (b) the **Tension E** subsection (workspace_path field), (c) the **Ship plan** table at the bottom.
2. `docs/worklog/2026-05-02/02-t011-docs-wrapper-boundary.md` — how the previous task ran through the workflow. Lessons that apply to T012 too.
3. `docs/philosophy.md` — specifically the new `## What's outside the substrate` section that T011 added. T012 implements the schema side of that boundary (workspace_path is the project-script-wraps-stores field; next-id is the project-script-asks-stores verb).
4. `tasks/CLAUDE.md` — task lifecycle protocol.

### 2. Critical context (decisions already made — do not re-litigate)

- **Workspace_path on row, not via hook.** Confirmed by Blake via AskUserQuestion. Project script writes the path at task creation; drive reads it; stores never invokes setup scripts or creates worktrees. ([philosophy](../philosophy.md) backs this — wrappers wrap stores, not the other way around.)
- **Field type:** plain `text`, optional. (No path enum exists in any stores schema today; convention is plain text.)
- **The SDK session-fresh footgun is already guarded** in `src/runner/claude_code.rs:308-309`. The path passed via workspace_path **MUST be canonicalized once at spawn and locked** for the duration of the agent's run. Any workspace_path implementation that fails to canonicalize will silently break session continuity. Add a comment in the new code referencing this.
- **`Runner::spawn` trait signature changes** to take an `Option<&str>` workspace_path. This breaks both `ClaudeCodeRunner` and `MockRunner` — both need to be updated.
- **Validation policy:** drive errors at spawn time if workspace_path is set but the path doesn't exist (no silent fallback to `pwd` — that would silently put work in the wrong place). At write time, no path-existence check (workspace can become invalid later; that's fine, write was valid at the time).
- **`tasks next-id` verb:** read-only scan of `tasks/{active,planning,paused,completed,archived}/` for the highest `T###`, returns next available. Project scripts (e.g. `./dev new`) call this to coordinate IDs across worktrees. No state, no writes.

### 3. Codebase landmarks (already verified)

| What | Where |
|---|---|
| Tasks schema (where `workspace_path` field goes) | `stores/tasks/schema.yaml` — fits naturally in the relationship/identity cluster, near the existing `branch` field |
| Spawn site (where `current_dir` is set) | `src/runner/claude_code.rs:308-309` |
| Runner trait (signature change site) | `src/runner/claude_code.rs` (find the `Runner` trait def) + `src/runner/mock.rs` |
| Drive loop (where workspace_path needs to be threaded from row → spawn) | `src/handlers/drive.rs` (look near the existing `runner.spawn(...)` call) |
| Existing audit of all this | `01-real-world-workflow-takeover-analysis.md` § "Tension E — Working directory propagation" → "Verification" subsection |

### 4. Branching

T011 was branched from `feat/T010-wrap-workflow` (stacked, since T010 is not yet on master). Recommend the same pattern for T012:

```bash
git checkout feat/T011-docs-wrapper-boundary
git checkout -b feat/T012-workspace-path-and-next-id
```

If Blake has merged T010/T011 to master in the interim, branch from master instead. Check with `git log --oneline master..feat/T010-wrap-workflow` first.

### 5. Suggested Intent Contract / DONE_WHEN starting point

The next planner agent will draft the real one, but here's the shape from the worklog:

> **DONE_WHEN.** (1) The `tasks` schema has a `workspace_path: text, required: false` field. (2) When set, drive uses it as the canonicalized cwd for every spawned agent (preserving the SDK session-fresh-on-cwd-mismatch guard). (3) When unset, drive falls back to inherited cwd (current behavior). (4) When set but pointing to a non-existent path, drive errors at spawn time with a clear message. (5) `stores tasks next-id` verb scans the tasks/ directory tree and prints the next available `T###`. (6) Tests cover all four spawn-time cases (set+exists, set+missing, unset, set+canonicalize-stable across spawn/resume).

### 6. Known phase shape

Two natural phases:
- **Phase 1: workspace_path** — schema field, runner trait signature, both runner implementations, drive plumbing, tests. ~30-50 LOC + test code.
- **Phase 2: next-id verb** — CLI dispatch, scan logic, tests. Smaller; might fold into Phase 1 if the planner judges it trivial.

### 7. Nothing to fix from T011

T011 shipped clean. CodeRabbit Stage 6 final run was "No findings." There's nothing to circle back on for T012's setup.

### 8. One lesson worth carrying forward

The CodeRabbit finding on T011 was structural: orchestrator set `Status: COMPLETE` before filling `## Completion`. **Fill the Completion section before flipping Status.** Tasks/CLAUDE.md doesn't say this explicitly today; the next agent could either honor it implicitly or open a tiny follow-up commit to document it.
