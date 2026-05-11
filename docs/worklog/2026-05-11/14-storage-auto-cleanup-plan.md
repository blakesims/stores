# Storage Auto Cleanup Plan

**Date:** 2026-05-11
**Type:** plan / storage hygiene / substrate repair

## Done When

The stores worktree ecosystem is automatically kept below a sane disk ceiling: terminal task worktrees no longer retain per-worktree Cargo targets after landing/closure, clean terminal worktrees are removed automatically when safe, dirty terminal worktrees are reduced to source-only by deleting build artifacts and surfaced for later disposition, run transcripts have an explicit GC/cap policy that protects live/current evidence but prevents runaway multi-GB JSONL accumulation, tests/CI avoid persistent per-worktree target growth, and the repo provides dry-run/execute operators proving the current 911G footprint can be reduced by hundreds of GB without force-removing dirty worktrees or deleting live runs.

## Concrete Policy Targets

- **Terminal task target storage:** steady-state target is **0G** under terminal task worktrees. Any terminal task with `<workspace_path>/target` is cleanup debt.
- **Whole worktree removal:** clean terminal non-abandoned worktrees should be removed automatically after artifact cleanup and merged-branch verification. Dirty terminal worktrees are preserved source-only and reported.
- **Abandoned worktrees:** target-only cleanup is immediate when no live process/marker exists. Whole-worktree removal is not automatic unless clean and age/disposition policy explicitly allows it.
- **Runs storage:** configurable cap, default **20G total `.stores/runs`**, warn above **10G**, and warn on any single transcript above **1G**. Live/current references are protected regardless of size.
- **Current incident target:** dry-run/execute operators must be able to show and safely reclaim hundreds of GB from the current **911G** footprint, with the first safe execution path focused on terminal `target/` deletion.

## Evidence From Dry Runs

Measured on 2026-05-11:

- `/home/blake/repos/experiments`: **911G** total.
- `/home/blake/repos/experiments/stores`: **102G**.
- `experiments/*/target`: **~854G** total across 117 target directories.
- Terminal task workspace `target/` directories: **~749.5G** reclaimable across 110 candidates.
- `.stores/runs`: **56G**.
- `.stores/runs` JSONL/log-ish files: **7664 files / 55.66G**.
- DB size is not the issue: `.stores/db.sqlite` **27M**, WAL **7.7M**.
- Task rows with workspace paths: `integrated=91`, `abandoned=22`, `closed_out_of_band=16`, `accepted=10`, `planning=2`, `executing=1`.
- Clean whole-worktree removal candidates: **22** terminal non-abandoned worktrees.
- Dirty terminal non-abandoned worktrees: **82**; many are dirty only because generated task projections or `.stores` residue exist, but the cleanup command must classify rather than assume this.
- Abandoned worktrees: **22**; target-only cleanup is safe when no live process/marker exists, whole-worktree removal needs separate disposition.
- Top runaway runs include `.stores/runs/67cd1850-...jsonl` at **17.81G** and `.stores/runs/4de49156-...jsonl` at **3.97G**.

## Root Causes

1. **Per-worktree Cargo targets.** Auto-scaffold creates persistent `stores-T###-*` worktrees. Cargo commands run inside each workspace, so each worktree accumulates a separate 5–18G `target/` directory.
2. **No terminal cleanup hook.** The integration/post-land lifecycle lands work but does not clear build artifacts or remove the worktree when it becomes safe.
3. **Whole-worktree removal is blocked by generated residue.** `git worktree remove` correctly refuses dirty trees, but many terminal worktrees are dirty from generated projections or `.stores` links rather than useful source edits.
4. **Run transcripts are unbounded.** `.stores/runs` has no retention/cap policy and can grow multi-GB JSONL files during runaway agent loops.
5. **Tests are not the main persistent leak.** Tests mention worktrees/targets, but the persistent `stores-T###-*` directories are real substrate task worktrees from `./dev scaffold {display_id}` / auto-scaffold. Some tests already use temp `CARGO_TARGET_DIR`; guardrails should keep that true.

## Safety Constraints

- Do not use `git worktree remove --force` as routine cleanup.
- Do not delete active task worktrees (`accepted`, `planning`, `executing`, integration queued/active/blocked, or live drive metadata) without explicit human disposition.
- Do not blindly delete `.stores/runs`; protect live/current markers, active session ids, and recent failed evidence.
- Deleting `target/` directories is allowed for terminal worktrees only after live-safety checks because it removes build output, but it is unsafe during active builds.
- The main repo worktree (`/home/blake/repos/experiments/stores`) is never a terminal cleanup target.
- Cleanup commands must support dry-run first and report expected reclaimed size.
- No raw SQL writes to the substrate DB.
- Hardcoded `/home/blake/repos/experiments` paths are evidence only; implementation must discover repo/workspace paths from the row, git, and config.

## Live-Safety Checks

Before deleting `target/` or removing a worktree, cleanup must verify:

1. task status/lifecycle is terminal or explicitly eligible;
2. `drive_pid` is empty or not live;
3. no `current-*.json` run marker for the task/role is live/running;
4. no process has cwd under `workspace_path` or an open file under `<workspace_path>/target` when feasible (`lsof`/`procfs` best-effort on Linux; fail closed when evidence suggests activity);
5. branch is merged or otherwise safe before whole-worktree removal;
6. worktree is clean before whole-worktree removal.

## Command Surfaces

Implement split, explicit operators:

```bash
stores tasks cleanup-worktrees --dry-run
stores tasks cleanup-worktrees --execute --targets-only
stores tasks cleanup-worktrees --execute --remove-clean
stores runs gc --dry-run
stores runs gc --execute
```

`--dry-run` must be safe and mutation-free. `--execute` must require an explicit action (`--targets-only`, `--remove-clean`, or GC flags) so a broad command cannot accidentally remove everything.

## Implementation Parts

### Part 1 — Add task worktree storage audit and target cleanup

Add `stores tasks cleanup-worktrees` with dry-run and target-only execution.

Dry-run prints:

- per-worktree `target/` sizes,
- task status/lifecycle classification from `.stores/db.sqlite`,
- terminal `target/` deletion candidates and reclaim total,
- clean terminal worktree removal candidates,
- dirty terminal worktrees requiring disposition,
- live-safety skip reasons,
- DB/WAL size only as context, not as cleanup target.

Target-only execution:

- terminal statuses: `integrated`, `schema_migrated`, `cargo_installed`, `closed_out_of_band`, `rejected`, and `abandoned` only when live-safety checks pass;
- excludes the main repo path;
- deletes only `<workspace_path>/target`, not source, `.git`, `.stores`, or task projections;
- reports deleted count and bytes.

Success criteria:

- Current dry-run shows roughly 110 target candidates and roughly 749.5G reclaim.
- Execution can reclaim hundreds of GB without caring whether the git worktree is dirty.
- Active task targets remain untouched.

### Part 2 — Clean terminal worktree removal

Extend `stores tasks cleanup-worktrees` with `--execute --remove-clean`.

Behavior:

- only considers terminal non-active rows after target cleanup;
- verifies live-safety checks;
- verifies branch merged/on-main-safe;
- verifies `git status --porcelain` is empty;
- runs `git worktree remove <workspace_path>` without `--force`;
- reports dirty/unmerged/missing/live skip reasons.

Success criteria:

- Current dry-run identifies the known clean terminal non-abandoned removal candidates.
- Dirty worktrees are preserved and reported with reason summaries.
- No force removal is used.

### Part 3 — Automatic terminal cleanup hooks

Add substrate cleanup hooks so manual cleanup is not a recurring chore.

Edges/classes:

- stores repo post-land completion: after `post_integration_step == schema_migrated`;
- generic success completion: integrated/done rows with no configured post-land chain;
- `closed_out_of_band` and `rejected`: target cleanup and clean-worktree removal when live-safe;
- `abandoned`: target cleanup immediately when live-safe; whole-worktree removal only if clean and policy allows.

Hook behavior:

1. delete `<workspace_path>/target` for eligible terminal task;
2. attempt whole-worktree removal only when clean, merged/safe, and policy allows;
3. if dirty, leave the worktree in place, record/surface that it is source-only but needs disposition;
4. never force-remove.

Success criteria:

- New terminal tasks do not retain large per-worktree `target/` directories.
- Clean terminal worktrees disappear automatically.
- Dirty terminal worktrees remain inspectable but no longer carry multi-GB build artifacts.

### Part 4 — Prevent per-worktree target duplication

Prefer command/env-level shared Cargo target injection over writing `.cargo/config.toml` into each worktree, because generated per-worktree config could make worktrees dirty and block removal.

Implement a repo-configurable shared target directory:

```yaml
cleanup:
  cargo_target_dir: ../.cargo-target/stores
```

or equivalent existing config shape. Drive/integration/pre-land subprocesses should set `CARGO_TARGET_DIR` when configured. Tests that need isolation can override it with tempdirs.

Success criteria:

- New worktrees do not accumulate independent 5–18G `target/` directories during normal drive/integration.
- Tests that create temporary workspaces keep build artifacts in temp or shared target locations.
- The chosen strategy relies on Cargo's own locking/cache semantics and does not invalidate pre-land checks.

### Part 5 — Runs JSONL GC and caps

Add `stores runs gc` with default policy:

- total cap default: **20G**;
- warning threshold default: **10G**;
- per-file warning threshold default: **1G**;
- protect current/live markers (`current-*.json`, active session ids, stderr/status paths);
- keep failed runs longer than successful runs;
- preserve `agent_runs` DB metadata even when transcript files are GC'd;
- if deleting or compressing a transcript, leave a tombstone/summary file or make `stores runs show` report `GC'd` cleanly rather than failing mysteriously;
- explicit refusal to delete unknown live/current references;
- dry-run reports largest files and exact reclaim candidates.

Success criteria:

- Current `.stores/runs` dry-run identifies the 17.81G and 3.97G runaway JSONL files.
- GC can reduce `.stores/runs` from 56G to the configured cap without deleting live evidence.
- Future multi-GB transcript growth is surfaced quickly by warning/cap output.

### Part 6 — Test and CI guardrails

Add tests for:

- classification of terminal vs active workspace cleanup candidates;
- target deletion excluding main repo and active tasks;
- live marker/process skip behavior;
- clean vs dirty worktree removal behavior;
- runs GC protecting current/live markers;
- DB/backlink behavior when transcript files are GC'd;
- giant transcript detection;
- scaffold/drive path ensuring tests use temp/shared `CARGO_TARGET_DIR` and do not create persistent targets under `experiments/stores-*`.

Success criteria:

- Regression tests fail if cleanup would touch active worktrees or live runs.
- Regression tests fail if storage audit silently ignores large target/run growth.

## Validation Plan

1. Run dry-run audit before changes and preserve the measured baseline in command output.
2. Run focused unit/integration tests for cleanup classification and runs GC.
3. Run dry-run audit after implementation; verify candidates and byte totals match expectations.
4. Execute only the safest artifact cleanup (`target/` deletion for terminal worktrees) after dry-run confirmation, if explicitly selected as part of the task execution.
5. Re-run disk usage checks to confirm reclaimed space.

## Non-Goals

- No forced deletion of dirty source worktrees.
- No raw SQL writes to `.stores/db.sqlite`.
- No broad `docker system prune` or unrelated system cleanup.
- No deletion of main repo `target/` unless explicitly requested separately.
- No permanent loss of live/current run evidence.
- No treating generated dirty residue as removable unless the residue is explicitly classified known-safe.

## Oracle Review Changes Applied

The oracle critique led to these changes before implementation:

- added numeric steady-state/cap targets;
- split command surfaces into explicit task cleanup and runs GC commands;
- defined live-safety checks;
- broadened cleanup beyond only `post_integration_step == schema_migrated`;
- replaced per-worktree `.cargo/config.toml` as the preferred target-sharing mechanism;
- defined transcript GC/backlink semantics;
- kept dirty generated residue as a classification problem, not an automatic deletion assumption.
