# T012: workspace_path field + tasks next-id verb

## Meta
- **Status:** COMPLETE
- **Created:** 2026-05-02
- **Last Updated:** 2026-05-02
- **Blocked Reason:** —


## Task

Add an optional `workspace_path` field to the `tasks` schema so project scripts (e.g. `./dev new`) can pin where each task's spawned agents run. Drive uses the path as the canonicalized cwd at spawn time, preserving the existing SDK session-fresh-on-cwd-mismatch guard at `src/runner/claude_code.rs:305-306`. Drive errors loud at spawn time if the path is set but missing — no silent fallback.

Also add a read-only `stores tasks next-id` verb that scans `tasks/{active,planning,paused,completed,archived}/` for the highest existing `T###` and prints the next available ID. Project scripts call this to coordinate IDs across worktrees without races.

Together these are the substrate-side hooks for the wrapper boundary T011 just documented in `docs/philosophy.md` — the project-script-wraps-stores field (`workspace_path`) and the project-script-asks-stores verb (`next-id`). This is task #2 of the four-task ship plan from the 2026-05-02 worklog (`docs/worklog/2026-05-02/01-real-world-workflow-takeover-analysis.md`, Tensions D + E).

## Intent Contract

**Executive intent.** The substrate today silently inherits the orchestrator's cwd when spawning agents, which makes multi-worktree workflows (`/task:open` in a per-task worktree) unsafe — agents can land in the wrong tree. T012 makes the cwd explicit via a row-stored field and gives project scripts a race-free way to mint IDs across worktrees. Pinning these now closes the substrate side of the wrapper boundary so T013 (reviewer envelope migration) and T014 (framework write-path) can proceed without re-litigating cwd or ID semantics.

**DONE_WHEN.**
1. `tasks` schema has `workspace_path: text, required: false`.
2. When set, drive uses it as the canonicalized cwd for every spawned agent (preserving the SDK session-fresh-on-cwd-mismatch guard at `src/runner/claude_code.rs:305-306`).
3. When unset, drive uses inherited cwd (current behavior, no regression).
4. When set but the path doesn't exist, drive errors at spawn time with a clear message — no silent fallback.
5. `stores tasks next-id` verb scans `tasks/{active,planning,paused,completed,archived}/` for the highest `T###` and prints the next available ID. Read-only, no state.
6. Tests cover the four spawn-time cases (set+exists, set+missing, unset, set+canonicalize-stable across spawn/resume) and the next-id scan.

**Scope boundaries.**
- **In scope:**
  - `stores/tasks/schema.yaml` — add `workspace_path` field (placement near existing `branch` field at line 8)
  - `src/runner/mod.rs` — `Runner::spawn` trait signature gains `Option<&str>` workspace_path
  - `src/runner/claude_code.rs` — implement new signature; canonicalize-and-lock once at spawn (DO NOT re-canonicalize per call); preserve session-fresh guard
  - `src/runner/mock.rs` — update mock to new signature
  - `src/handlers/drive.rs` — read workspace_path from row at the existing `runner.spawn(...)` call site (~line 609); pass through; error at spawn time if path set but missing
  - CLI dispatch site for tasks subcommands — add `next-id` verb (read-only directory scan)
  - Tests for all of the above
- **Out of scope:**
  - No hook system for project-side scripts (workspace_path is written by the project script at task creation; stores does not invoke setup scripts or create worktrees)
  - No worktree creation, no setup-script invocation, no `cd` semantics beyond cwd at spawn
  - No path-existence check at write time (workspace can become invalid later; that's fine, write was valid at the time)
  - No path enum / typed-path (plain `text`, matches existing schema convention)
  - No retroactive backfill of existing tasks (field is optional)
  - No changes to other stores' schemas (tasks-only)

**Proposed approach.** Two natural phases:
- **Phase 1 — workspace_path.** Schema field → Runner trait signature → both runner impls (canonicalize-and-lock in ClaudeCodeRunner) → drive plumbing with spawn-time validation → tests. ~30-50 LOC + test code.
- **Phase 2 — next-id verb.** CLI dispatch → directory scan → tests. Smaller. Planner may fold into Phase 1 if trivial.

**Risks / assumptions.**
- The SDK session-fresh-on-cwd-mismatch guard (`src/runner/claude_code.rs:305-306`) MUST be preserved. Any workspace_path implementation that re-canonicalizes per call (rather than once at spawn) silently breaks session continuity for resumed agents. New code must comment-reference this guard so future readers see the constraint.
- `Runner::spawn` signature change breaks `MockRunner`; both impls move together in the same phase.
- `next-id` scanning multiple status directories assumes the canonical layout in `tasks/CLAUDE.md` (active/planning/paused/completed/archived). If a directory is missing, scan it as empty rather than erroring (lenient).
- Carry-forward from T011: fill `## Completion` section *before* flipping `Status: COMPLETE` (CodeRabbit Stage 6 caught this on T011).

**Open decisions.** None. All five (field placement, type, validation policy, trait signature change, next-id behavior) were locked during the morning design discussion via AskUserQuestion. See `docs/worklog/2026-05-02/01-real-world-workflow-takeover-analysis.md` Tensions D + E for the rationale.

---

## Plan

### Objective

Close the substrate side of the wrapper boundary documented in T011 by giving project-side scripts (e.g. `./dev new`) two precise hooks: (a) an optional `workspace_path` field on the `tasks` row that pins the cwd of every spawned agent, and (b) a read-only `stores tasks next-id` verb that mints the next task ID via a directory scan. Together these unblock multi-worktree workflows where the orchestrator lives in the main worktree but task work happens in a feature worktree — without expanding the substrate's authority surface beyond what T011 already locked in.

### Scope

- **In Scope:**
  - `stores/tasks/schema.yaml` — add `workspace_path: text, required: false` (next to existing `branch` field at line 8).
  - `src/runner/mod.rs` — extend the `Runner::spawn` trait signature with a new `workspace_path: Option<&str>` parameter and document the canonicalize-and-lock contract in the trait doc comment.
  - `src/runner/claude_code.rs` — implement the new signature: when `workspace_path` is `Some`, canonicalize that path once at spawn entry and use it as `cmd.current_dir(...)`; when `None`, preserve current behavior (`resolve_cwd()` from inherited cwd). Comment must reference the SDK session-fresh-on-cwd-mismatch guard at lines 33–38 / 305–306.
  - `src/runner/mock.rs` — update `MockRunner::spawn` signature; behavior unchanged (mock ignores cwd, but should record it for test introspection — see Decision Matrix row 9).
  - `src/handlers/drive.rs` — at the existing `runner.spawn(...)` call site (line 609), extract `workspace_path` from `entry`, validate (set + missing → loud error before spawn), and thread through.
  - `src/handlers/guide.rs` — both `runner.spawn(...)` call sites (lines 274, 347) must pass through the new parameter. For the guide handler, pass `None` for now — the guide is a v0.3 stub and operates on the orchestrator's cwd; no row-driven workspace_path semantics in scope here. (Documented in Decision Matrix row 10.)
  - `src/cli/dynamic.rs` — register a new `next-id` workflow-only subcommand on the `tasks` store (mirrors the existing `build_status_cmd` / `build_drive_cmd` pattern at lines 504–546). Add `"next-id"` to the `WORKFLOW_VERBS` reserved list at line 205.
  - `src/cli/dispatch.rs` — add a `Some(("next-id", _))` arm in the `tasks` workflow dispatch block (sibling of the `("status", sub)` arm at line 203).
  - `src/handlers/` — add a new `next_id.rs` module (or fold into an existing handler — see Decision Matrix row 11) that implements the directory scan and prints the next ID.
  - `src/handlers/mod.rs` — register the new module if added.
  - Tests:
    - `src/runner/mock.rs` — extend existing tests for the new signature; add an assertion that the workspace_path arg is recorded on the runner.
    - `src/runner/claude_code.rs` — add unit tests for the canonicalize-when-set / inherited-when-unset behavior, mirroring the existing `cwd_canonicalised_before_spawn` test (line 630).
    - `src/handlers/drive.rs` (in-module `#[cfg(test)] mod tests`) — add the four spawn-time cases (set+exists, set+missing, unset, set+canonicalize-stable across repeated spawns within one drive run).
    - `src/handlers/next_id.rs` (or wherever the verb lives) — add scan tests covering: empty tree, sparse tree (only `completed/`), highest in `archived/`, missing directories, non-T### entries ignored.

- **Out of Scope:**
  - Worktree creation, project-script invocation, hook system. Stores never invokes any project-side script.
  - Any change to the guide handler's row-driven cwd semantics. (Guide remains orchestrator-cwd. Trait change is mechanical; behavioral change is drive-only.)
  - Path-existence check at `tasks add` / `tasks update` write time. The path can become invalid later (worktree deleted); that is fine. Validation is spawn-time only.
  - Backfilling `workspace_path` on existing T### rows.
  - Path-typed schema field type. Plain `text`, matches existing convention (Intent Contract locked).
  - Any change to other stores' schemas (tasks-only).
  - Renaming or reorganizing existing CLI verbs.
  - Streaming output, async runner, cancellation tokens (deferred per `src/runner/mod.rs` doc comment).

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | `workspace_path` field — schema, trait signature, both runner impls, drive plumbing with spawn-time validation, full test coverage of the four spawn-time cases | Medium |
| 2 | `tasks next-id` verb — CLI registration, dispatch arm, handler with directory scan, scan tests | Low |

### Phase Details

#### Phase 1: workspace_path field + cwd plumbing

- **Objective:** Give every `tasks` row an optional `workspace_path` that drive uses as the canonicalized cwd for every spawned agent, with a loud error at spawn time when the path is set but missing. Preserve the SDK session-fresh guard (path canonicalized once at spawn, not re-canonicalized per call).

- **Files to modify:**
  - `stores/tasks/schema.yaml` — insert one line after line 8 (the `branch` field): `- {name: workspace_path, type: text, required: false}`.
  - `src/runner/mod.rs` — add `workspace_path: Option<&str>` parameter to `Runner::spawn` (positional, last, after `schema`). Update the `# Parameters` doc comment block. The trait doc must explicitly call out: "If `Some`, the runner MUST canonicalize and lock this path at spawn entry. The Anthropic SDK silently mints a fresh session if cwd differs between spawn and resume calls (see `claude_code.rs:305-306`)."
  - `src/runner/claude_code.rs`:
    - Update `impl Runner for ClaudeCodeRunner` `spawn` signature.
    - Replace `let cwd = resolve_cwd()?;` (line 306) with conditional logic: when `workspace_path` is `Some(p)`, call `PathBuf::from(p).canonicalize().context("workspace_path canonicalize failed: ...")?` once at spawn entry and use that as `cwd`. When `None`, call `resolve_cwd()` as today.
    - Add an inline comment above the new conditional referencing the session-fresh guard at lines 33–38 / 305–306 verbatim — this is a guardrail for future readers.
    - Update `# cwd canonicalisation` section in the file's top doc comment to describe both branches.
  - `src/runner/mock.rs`:
    - Update `MockRunner::spawn` signature to accept the new parameter.
    - Add an `Option<RefCell<Vec<Option<String>>>>`-style field (or simpler: `RefCell<Vec<Option<String>>>` named `workspace_paths_seen`) so tests can introspect what cwd was passed per-call.
    - Update the doctest at line 30 and all in-module tests (lines 105, 110, 118, 133, 134, 141, 166) to pass the new parameter (`None` for existing tests).
  - `src/handlers/drive.rs`:
    - At line 609 (the `runner.spawn(...)` call), before the call, extract `workspace_path` from `entry` using the same pattern used for `branch` (lines 557–560: `entry.get("workspace_path").and_then(|v| v.as_str())`).
    - Validate: if `Some(p)` and `!std::path::Path::new(p).exists()`, `bail!` with a message including the task display ID and the missing path. Validation lives in drive (not in the runner) — keeps the runner's contract narrow ("if you give me a path, I will use it") and keeps user-facing errors in the orchestration layer where context (display_id, task) is available. See Decision Matrix row 5.
    - Pass `workspace_path` through to `runner.spawn(...)`.
    - All in-module test fixtures at lines 1198, 1319, 1360, 1407, 1440, 1482, 1511, 1573, 1715, 1883, 1939, 1987, 2028 need the new param (`None` for existing tests; new tests below for the cases that matter).
  - `src/handlers/guide.rs`:
    - Both `runner.spawn(...)` call sites at lines 274 and 347 take `None` for the new parameter (guide is orchestrator-cwd; row-driven workspace_path is a drive concern only).
    - Doctest / unit tests at lines 1078, 1110, etc. updated mechanically.
  - **New tests** in `src/handlers/drive.rs` `#[cfg(test)] mod tests` (or a new sub-module `mod workspace_path_tests`):
    - `workspace_path_unset_uses_inherited_cwd` — row with no workspace_path; verify MockRunner records `None` for the cwd arg on the spawned planner.
    - `workspace_path_set_and_exists_canonicalizes` — row with workspace_path set to a `tempfile::tempdir()` path; verify MockRunner records the canonicalized path on every spawn.
    - `workspace_path_set_but_missing_errors_at_spawn` — row with workspace_path pointing at a non-existent directory; verify drive returns `Err` whose message contains the path and the display_id, and that the runner queue is untouched (no spawn happened).
    - `workspace_path_canonicalize_stable_across_spawns` — same row drives through two consecutive cycles (e.g. planner → executor); verify the path recorded on both spawns is byte-identical, demonstrating the canonicalize-once contract is honored per spawn-call (this is the "spawn/resume continuity" guarantee in test form).
  - **New tests** in `src/runner/claude_code.rs` `#[cfg(test)] mod tests` (mirror existing `cwd_canonicalised_before_spawn` at line 630):
    - `workspace_path_canonicalised_when_some` — when `workspace_path: Some(<tempdir path>)` is passed, the spawn pins the canonicalized form of that path (assert via shim that echoes its cwd into stdout, similar to the existing shim pattern at line 696).
    - `workspace_path_falls_back_to_inherited_when_none` — when `workspace_path: None`, behavior matches today (cwd == `resolve_cwd()`).

- **Acceptance Criteria (verifiable by code-reviewer against `git diff` and `cargo test`):**
  - [ ] AC1.1 (DONE_WHEN 1) — `stores/tasks/schema.yaml` contains the line `- {name: workspace_path, type: text, required: false}` adjacent to the existing `branch` field. `git diff stores/tasks/schema.yaml` shows exactly that one-line addition.
  - [ ] AC1.2 (DONE_WHEN 2,3) — `Runner::spawn` in `src/runner/mod.rs` has signature `fn spawn(&self, role: &str, system_prompt: &str, brief: &str, schema: Option<&str>, workspace_path: Option<&str>) -> Result<RunnerOutput>`. The trait doc comment explicitly references the SDK session-fresh-on-cwd-mismatch guard.
  - [ ] AC1.3 (DONE_WHEN 2) — `ClaudeCodeRunner::spawn` canonicalizes `workspace_path` once at spawn entry when `Some`, uses it as `cmd.current_dir(...)`. An inline comment on the new branch references `claude_code.rs:305-306` (or the equivalent post-edit lines) and the SDK footgun.
  - [ ] AC1.4 (DONE_WHEN 3) — when `workspace_path` is `None`, `ClaudeCodeRunner::spawn` calls `resolve_cwd()` (current behavior, no regression). Existing test `cwd_canonicalised_before_spawn` still passes unchanged.
  - [ ] AC1.5 (DONE_WHEN 4) — drive errors at spawn time when `workspace_path` is set but the path does not exist. Error message includes the task's display_id and the missing path. No silent fallback to inherited cwd. Test `workspace_path_set_but_missing_errors_at_spawn` passes and verifies the runner queue is undrained (no spawn occurred).
  - [ ] AC1.6 (DONE_WHEN 6, partial — four spawn-time cases) — all four new drive tests pass:
    - `workspace_path_unset_uses_inherited_cwd`
    - `workspace_path_set_and_exists_canonicalizes`
    - `workspace_path_set_but_missing_errors_at_spawn`
    - `workspace_path_canonicalize_stable_across_spawns`
  - [ ] AC1.7 (DONE_WHEN 6, partial — runner-level coverage) — both new claude_code unit tests pass: `workspace_path_canonicalised_when_some` and `workspace_path_falls_back_to_inherited_when_none`.
  - [ ] AC1.8 — `cargo test --all-features` passes with no skips and no new warnings beyond what existed pre-task.
  - [ ] AC1.9 — `tests/tasks_e2e.sh` and `tests/drive_e2e.sh` still pass (no behavior change for tasks without workspace_path; the field is optional and inert when unset).

#### Phase 2: `tasks next-id` verb

- **Objective:** Add a read-only `stores tasks next-id` verb that scans `tasks/{active,planning,paused,completed,archived}/` for the highest existing `T###` directory and prints the next available ID. Lenient: missing directories scan as empty. No state, no row writes, no DB touch.

- **Files to modify:**
  - `src/cli/dynamic.rs`:
    - Add `"next-id"` to the `WORKFLOW_VERBS` reserved list at line 205 (so it cannot be shadowed by a transition verb of the same name).
    - Add a new `build_next_id_cmd()` function near `build_status_cmd` (around line 513), shape: `Command::new("next-id").about("Print the next available task ID by scanning tasks/{active,planning,paused,completed,archived}/")`. No flags in v0.3.
    - Register it in the workflow-verbs block at line 219 (`store_cmd = store_cmd...subcommand(build_next_id_cmd())`).
  - `src/cli/dispatch.rs`:
    - Add a `Some(("next-id", _sub))` arm sibling to the `("status", sub)` arm at line 203, calling `handlers::next_id::run_next_id()?;`.
  - `src/handlers/next_id.rs` (new file):
    - Public function `run_next_id() -> Result<()>` that:
      - Resolves the scan root: current working directory + `tasks/`. (See Decision Matrix row 12 — cwd-relative is the right call here for the v0.3 use case.)
      - Iterates the five canonical subdirs (`active`, `planning`, `paused`, `completed`, `archived`), tolerating missing dirs as empty (just `continue`).
      - For each present dir, reads its entries, filters to `T###`-shaped names (regex `^T(\d{3,})(-|$)`), parses the numeric portion as `u32`.
      - Tracks the maximum across all dirs (default 0 if none found).
      - Prints `T{:03d}` of (max + 1) to stdout, no trailing newline beyond the standard `println!`.
    - Implementation must be small (~50 LOC including tests).
  - `src/handlers/mod.rs` — add `pub mod next_id;`.
  - **New tests** in `src/handlers/next_id.rs` `#[cfg(test)] mod tests` (using `tempfile::tempdir()` + `std::env::set_current_dir`-scoped helper, or a pure-function `next_id_for_root(root: &Path) -> Result<String>` that the public `run_next_id` wraps — preferred pattern, see Decision Matrix row 13):
    - `empty_tree_returns_t001` — empty `tasks/` directory → prints `T001`.
    - `single_completed_t005_returns_t006` — only `tasks/completed/T005-foo` exists → `T006`.
    - `highest_in_archived_wins` — `archived/T010-x`, `completed/T003-y` → `T011`.
    - `missing_directories_treated_as_empty` — only `planning/` exists with `T007-z`; the other four dirs do not exist → `T008`. No error.
    - `non_task_entries_ignored` — `completed/README.md`, `completed/T004-real`, `completed/notes/` (dir without T###) → `T005`.
    - `non_canonical_directories_ignored` — `tasks/ongoing/T999-x` (note: `ongoing/` exists in this repo but is not in the canonical list — see Decision Matrix row 14) → not counted; result based only on the five canonical dirs.

- **Acceptance Criteria:**
  - [ ] AC2.1 (DONE_WHEN 5) — `stores tasks next-id` is a registered subcommand. `stores tasks --help` lists `next-id` with its about-string.
  - [ ] AC2.2 (DONE_WHEN 5) — `stores tasks next-id` invoked from a directory containing `tasks/` prints exactly the next ID, formatted `T{:03d}`, on stdout. No state mutation: re-running the verb produces the same output (verified by AC2.3 and the empty-tree test).
  - [ ] AC2.3 (DONE_WHEN 5) — verb is read-only: no new files created, no rows inserted, no logs written. `git status` after running is clean. (Verified by inspection during code review; not a test assertion.)
  - [ ] AC2.4 (DONE_WHEN 6, partial — next-id scan) — all six new `next_id` tests pass.
  - [ ] AC2.5 — `cargo test --all-features` passes (combined with AC1.8 — single full-suite run is sufficient).
  - [ ] AC2.6 — running `stores tasks next-id` in this repo right now would print `T013` (current highest is T012 in `planning/`). Code-reviewer can verify by checkout-and-run, or trust the equivalent test fixture. (Optional smoke check; not a hard gate.)

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| 1. Phase split | (a) Single phase covering everything. (b) Two phases (workspace_path, then next-id). (c) Three phases (schema, then trait+runner, then next-id). | (b) — two phases. | The two pieces are independently testable, touch different files, and have different risk profiles: Phase 1 changes a trait signature (rippling through 4+ files) and is the high-risk slice; Phase 2 is a self-contained read-only verb with no shared surface. Splitting them lets the code-reviewer apply different scrutiny — Phase 1 needs careful review of the SDK guard preservation and the spawn-time error path; Phase 2 needs to verify the directory scan is lenient and pure. Folding into one phase (a) would dilute review focus and conflate two independent failure modes. Splitting Phase 1 further into schema + trait (c) is process for its own sake — the trait change requires the schema field to exist for the integration tests to make sense, so they ship together. |
| 2. Schema field placement in `stores/tasks/schema.yaml` | (a) Adjacent to `branch` (line 8). (b) In a new "physical layout" subsection. (c) Inside the `contract` record. | (a) — adjacent to `branch`. | The Intent Contract pre-locked this; the verification confirms it. Both fields describe "where this task's work physically lives" — same conceptual cluster. (b) over-engineers a single-line addition. (c) is wrong: `contract` is for human-supplied intent (objective, scope, etc.), not orchestration metadata. |
| 3. `Runner::spawn` parameter style | (a) Add positional `Option<&str>` param at the end. (b) Introduce a `SpawnRequest` struct. (c) Builder pattern. (d) Per-runner setter (`runner.set_workspace_path(...)` before spawn). | (a) — positional `Option<&str>`. | The trait already takes 4 positional args (`role`, `system_prompt`, `brief`, `schema`); adding a 5th matches the established shape. (b) and (c) would be a separate refactor with its own risk; in scope per the Intent Contract is "trait signature change," not "trait restructure." (d) breaks the stateless-call contract — multiple call sites would need to remember to set then unset, and forgetting would silently leak the previous task's cwd into the next spawn. The locked decision in the Intent Contract is `Option<&str>`; this row defends why nothing else is reasonable here. |
| 4. Where in drive to read `workspace_path` from the row | (a) Inline at the spawn call site (line 609), using the same `entry.get("workspace_path").and_then(|v| v.as_str())` pattern already used for `branch` at lines 557–560. (b) A helper function `extract_workspace_path(entry: &Value) -> Option<&str>`. (c) At the top of the drive loop, cached on a struct. | (a) — inline at the spawn site. | Symmetry with how `branch` is extracted (lines 557–560) is the strongest signal — same shape, same idioms, immediately recognizable to any reader who has read the diff for `branch`. A helper (b) would be one extra layer of indirection for a 2-line lookup. Caching (c) is wrong: the row is already loaded once per drive-iteration, and re-reading the field is free (it's a HashMap lookup). |
| 5. Spawn-time validation location (set+missing → error) | (a) Inside `ClaudeCodeRunner::spawn` (the runner errors when canonicalize fails). (b) In drive, before calling `runner.spawn(...)`. (c) Both — defense in depth. | (b) — in drive. | The user-facing error wants the task display_id and the missing path, both of which drive has but the runner does not. The runner's contract is narrower ("if you give me a path, I will use it; if it can't be canonicalized, I'll surface the IO error"). Putting validation in drive keeps the runner's failure modes reduced to genuine infrastructure errors and gives the user a clear, contextualized error like `[T012] workspace_path '/tmp/foo' does not exist`. The runner's `canonicalize()` would *also* fail in this case, but as an underlying IO error without context; drive's pre-check produces the better message and short-circuits before the runner is even invoked. (c) is over-engineering — defense in depth here means duplicated error messages, not extra safety. |
| 6. `next-id` CLI verb placement | (a) New workflow-only verb on the `tasks` store, registered in `dynamic.rs` alongside `status` / `drive`. (b) Top-level `stores next-id`. (c) `stores tasks list --next` flag on the existing `list` verb. | (a) — workflow-only verb on `tasks`. | The verb is tasks-specific: it scans `tasks/` and produces `T###`-shaped IDs. A top-level verb (b) would imply other stores have similar semantics, which they don't. A `--next` flag on `list` (c) overloads `list` (which prints rows from the DB) with a directory-scan responsibility — different data source, different read path. Workflow-only (a) keeps the verb gated behind `schema.workflow.is_some()` (line 219), which `tasks` is, and follows the established pattern for `status`, `drive`, etc. The verb needs to be added to `WORKFLOW_VERBS` (line 205) so a future schema author can't shadow it via a transition verb of the same name. |
| 7. What `next-id` prints | (a) Just the formatted ID (`T013\n`). (b) JSON `{"next_id": "T013"}`. (c) Two lines: ID and the highest-existing-id it derived from. | (a) — just `T013\n`. | Project scripts will consume it via shell substitution (`tid=$(stores tasks next-id)`), which wants minimal output. JSON (b) forces every consumer to install `jq` for a one-field response. Two-line debug output (c) is what `--verbose` would be for — out of scope for v0.3, can be added later without breaking single-line consumers. |
| 8. `next-id` behavior on missing directories | (a) Lenient — treat missing dirs as empty. (b) Strict — error if any of the five canonical dirs is missing. | (a) — lenient. | Locked in the Intent Contract (Risks/assumptions bullet 3). Defended here for completeness: the canonical layout is documented in `tasks/CLAUDE.md` but a fresh repo or one that hasn't yet ratified any tasks won't have all five dirs. Erroring would force every project script to pre-create empty dirs to use the verb. Lenient also makes the verb safe to run as a "ping" — first invocation in a virgin tree returns `T001` with no setup needed. |
| 9. Should `MockRunner` record workspace_path per-call? | (a) Add a `workspace_paths_seen: RefCell<Vec<Option<String>>>` field, push on each `spawn`. (b) Don't record — keep mock minimal; tests verify behavior indirectly. | (a) — record. | The four spawn-time tests need to assert the cwd that *would have been* used. The mock doesn't actually `cd` anywhere, so without explicit recording the only way to assert "the right cwd was passed" is via integration with the real claude_code runner — which adds shim complexity for a unit test. Recording is one new field + one `push` per spawn; trivial, and the existing `remaining_count()` accessor establishes the precedent that mock exposes test-introspection state. |
| 10. Pass `None` for `workspace_path` from `guide.rs`? | (a) Pass `None` — guide remains orchestrator-cwd. (b) Read `workspace_path` in guide too, mirror drive's behavior. (c) Conditional based on guide mode (gate-form vs tasks-form). | (a) — pass `None`. | The guide is a v0.3 stub (per dispatch.rs:193 comment: "tasks (stub form)") that produces a context bundle; it does not run a long agent loop and does not depend on workspace cwd for correctness. Extending it to honor workspace_path is a separate concern and not in DONE_WHEN. Doing it now (b) would expand scope into a handler the Intent Contract did not list. The trait change forces the parameter to exist at the call site; passing `None` is the minimum mechanical change. A future task can revisit this if guide grows row-driven cwd needs. |
| 11. Where the `next-id` handler lives | (a) New file `src/handlers/next_id.rs`. (b) Fold into `src/handlers/status.rs` as a sibling function. (c) Inline in dispatch.rs. | (a) — new file. | The handlers/ directory has one file per verb (`drive.rs`, `guide.rs`, `status.rs`, `next_action.rs`, etc.), confirmed by `ls src/handlers/`. A new verb gets a new file. (b) breaks the convention and bloats `status.rs` with unrelated logic. (c) puts business logic in the dispatcher, breaking the dispatcher's "thin glue" role. |
| 12. Scan-root resolution for `next-id` | (a) Always scan `./tasks/` relative to cwd. (b) Walk up looking for the nearest `tasks/` directory (like `git rev-parse`). (c) Configurable via `--root <path>`. | (a) — cwd-relative. | The verb is intended to be called by project scripts that already `cd` to the project root before invoking. The Intent Contract describes the use case as "project script wraps stores"; that script knows where the project root is. (b) introduces a discovery algorithm (and edge cases — what if `tasks/` exists in a parent unrelated repo?) for marginal convenience. (c) is the right escape hatch but is YAGNI for v0.3 — can be added later without breaking the no-flag default. The internal handler should still take a `root: &Path` parameter for testability (Decision Matrix row 13); only the public CLI defaults to cwd-relative. |
| 13. Test seam for `next-id` | (a) Pure inner function `next_id_for_root(root: &Path) -> Result<String>` + thin public `run_next_id()` that resolves cwd and calls inner. (b) Public function takes a `root` arg with a default. (c) Tests `cd` into `tempdir()` and call `run_next_id()`. | (a) — pure inner function. | Pure functions over real-path inputs are trivially testable without `set_current_dir` (which is process-global state and dangerous in parallel test runs — Rust's test harness defaults to multi-threaded). (b) leaks an implementation detail to callers. (c) introduces test-ordering coupling and is the failure mode that breaks once tests run in parallel. (a) is the standard Rust pattern and the existing codebase already uses it (e.g. `resolve_cwd()` exposed for test access at `claude_code.rs:269`). |
| 14. Treatment of non-canonical `tasks/ongoing/` directory | (a) Ignore it — only scan the five canonical dirs from the Intent Contract. (b) Add `ongoing` to the scan list. (c) Error if `ongoing/` is detected. | (a) — ignore, scan only canonical five. | The canonical layout in `tasks/CLAUDE.md` lists only `active/planning/paused/completed/archived`. `ongoing/` exists in this repo (verified via `ls tasks/`) but is non-canonical — likely vestigial. The Intent Contract explicitly enumerates the five dirs. Adding `ongoing/` to the scan (b) would deviate from the contract and from the documented convention. Erroring (c) is too aggressive — the directory may legitimately contain something the user cares about, just not for ID minting. Worth flagging to plan-reviewer in case `ongoing/` is intentional and `tasks/CLAUDE.md` is out of date — see "Open questions for plan-reviewer" below. |
| 15. Comment-referencing the SDK guard in new code | (a) One-line comment on the new conditional. (b) A multi-line block comment with the verbatim explanation. (c) Update only the file-top doc, no inline comment. | (a) — one-line inline comment. | The file-top doc already explains the guard (lines 33–38). The inline comment exists to make the constraint visible *at the point of risk* — a future reader editing the conditional should see the warning without scrolling to the top of the file. One line is enough: "Canonicalize once at spawn entry; the SDK silently mints a fresh session if cwd differs across resume calls (see lines 33–38)." (b) is cargo-cult verbosity. (c) leaves the risk invisible to a future drive-by edit. |

### Open questions for plan-reviewer

- **Q1 (low-priority, non-blocking):** `tasks/ongoing/` exists in this repo but is not in the canonical layout per `tasks/CLAUDE.md` (which lists only `active/planning/paused/completed/archived`). Decision Matrix row 14 chooses to ignore it. Is `tasks/ongoing/` vestigial (and should it be removed in a follow-up cleanup task), or is it intentional and `tasks/CLAUDE.md` is out of date? The plan as drafted does not depend on the answer — `next-id` ignores it either way per the locked Intent Contract scan list — but it would be worth surfacing for the orchestrator's attention. Not a gating question for this plan.
- **Q2 (carry-forward from T011):** Per the T011 retrospective ("CodeRabbit Stage 6 caught a workflow-level issue"), the orchestrator should fill `## Completion` *before* declaring `Status: COMPLETE`. The Intent Contract already calls this out as a Risks/assumptions bullet. Flagging again here so the orchestrator does not need to re-discover it.

---

## Plan Review
_Plan-reviewer agent fills this section._

- **Gate:** READY
- **Open Questions Finalized:**
  - Q1 (`tasks/ongoing/`): non-blocking. Plan correctly ignores it per locked Intent Contract; surface to orchestrator as a possible future cleanup task. No plan change needed.
  - Q2 (T011 carry-forward): non-blocking. Already in Intent Contract Risks/Assumptions; re-flagged in carry-forward notes for executor.
  - Q3 (new, low-priority): `MockRunner::workspace_paths_seen` field reuses the existing `unsafe impl Send` at `src/runner/mock.rs:61`. No new `unsafe` block needed; flagged so executor does not add a second one. Non-blocking.
- **Issues Found:**
  - **Accuracy nit (non-blocking):** Phase 1 line 127 cites `guide.rs:1078, 1110` as "doctest / unit tests updated mechanically." Those line numbers are `MockRunner::new(vec![...])` constructors, not `runner.spawn(...)` call sites — they will compile through the trait change automatically. The actual production `runner.spawn(...)` sites at `guide.rs:274, 347` are the only ones needing `None`. Documentation accuracy issue, not a structural problem; executor will discover on inspection.
  - **Interpretation note (non-blocking):** DONE_WHEN clause 6's "set+canonicalize-stable across spawn/resume" is verified structurally (canonicalize-once contract via two consecutive spawns) rather than via actual `--resume`, because `--resume` is a deferred v0.4 feature per `claude_code.rs:23-24`. Plan's test approach is the strongest verification reachable today. Wording in AC1.6 is honest enough; could be made fully explicit on a future polish pass.
- **Plan strengths (recorded for completeness):**
  - 15-row Decision Matrix is unusually thorough; pre-empts every reasonable executor question.
  - All `runner.spawn(...)` call sites verified by independent grep — none missed.
  - SDK guard preservation (clause 2) is structurally protected, not just documented: the canonicalize-once-per-spawn pattern is enforced at the function-entry conditional with a mandated inline comment.
  - Validation in drive (Decision Matrix row 5) is correctly chosen — drive has the display_id needed for a quality error message; runner's `canonicalize()` provides implicit secondary defense.
  - Pure inner function `next_id_for_root(root: &Path)` (Decision Matrix row 13) is the right test seam — avoids `set_current_dir` parallel-test hazard and mirrors existing `resolve_cwd()` pattern.
  - Regex `^T(\d{3,})(-|$)` correctly accepts `T999-x` and `T013` while rejecting `T9`, `Toops-x`, and `T009foo`.

> Details: plan-review.md

---

## Execution Log
_Executor agent fills this section per phase._

### Phase 1: workspace_path field + cwd plumbing
- **Status:** COMPLETE (cycle 3)
- **Started:** 2026-05-02
- **Completed:** 2026-05-02
- **Commits:** pending code review
- **Files Modified:**
  - `stores/tasks/schema.yaml` — added `workspace_path: text, required: false` adjacent to `branch`
  - `src/runner/mod.rs` — extended `Runner::spawn` with `workspace_path: Option<&str>` (last param); updated trait doc with SDK guard reference
  - `src/runner/claude_code.rs` — implemented new signature with canonicalize-once conditional; updated file-top doc; added 2 new tests; cycle 3 see below
  - `src/runner/mock.rs` — updated `MockRunner::spawn` signature; added `workspace_paths_seen` field + accessor; updated 5 in-module test call sites
  - `src/handlers/drive.rs` — added workspace_path extraction + pre-spawn validation + pass-through; added 4 new workspace_path tests
  - `src/handlers/guide.rs` — updated 2 `runner.spawn(...)` call sites to pass `None`
  - `tasks/active/T012-workspace-path-and-next-id/main.md` — this execution log
- **Notes (cycles 1 & 2):**
  - Line numbers in the plan drifted slightly (e.g. drive.rs spawn call was at ~609 before; tests at different offsets). No structural issues — all call sites found via `cargo build` error output.
  - Pre-task warning baseline: 3x `unused import: crate::db` in drive.rs tests (pre-existing). No new warnings added.
  - `workspace_path_set_and_exists_canonicalizes` test: MockRunner records the raw string from the row (not yet canonicalized); that is correct — the mock doesn't canonicalize. The runner-level test `workspace_path_canonicalised_when_some` verifies canonicalization via a real shim. Noted inline in the test comment.
  - All 487 tests pass; both e2e scripts pass; `git diff --stat` matches exactly the 7 in-scope files.

#### Cycle 3 — ETXTBSY + cwd-dangling race elimination (src/runner/claude_code.rs only)

- **Races addressed:**
  1. **Race A: `unsafe set_var(PATH)` / ENOENT on `resolve_cwd()`** — Tests `session_id_is_valid_uuid_v4_propagated_to_output` and `json_schema_arg_is_passed_inline` called `runner.spawn(..., workspace_path: None)`, triggering `resolve_cwd()` → `std::env::current_dir()`. `paths::tests` concurrently calls `set_current_dir(tmp)` then `drop(tmp)` while holding `cwd_lock`, leaving the process cwd dangling for any thread NOT holding the lock. Fixed by passing `workspace_path: Some(env!("CARGO_MANIFEST_DIR"))` in both tests — eliminates the `resolve_cwd()` call entirely.
  2. **Race B: ETXTBSY on shim exec** — `workspace_path_canonicalised_when_some` was the only shim-exec test NOT holding `cwd_lock`. Despite using a stable OnceLock shim in `target/test-shims/`, ETXTBSY fired when `paths::tests` was concurrently executing (these tests run `git` and `set_current_dir`). Holding `cwd_lock` in this test serializes against those concurrent writers, preventing the ETXTBSY at the kernel level. Root cause: the Linux 6.17 kernel's `execve` path returns ETXTBSY when concurrent write activity (from git, tempdir churn, or kernel inode bookkeeping) coincides with the exec of a file in the same filesystem region.
  3. **Race C: per-test shim write overhead** — Cycles 1 & 2 used per-test `write_shim` helpers and `PATH_MUTEX`. Cycle 3 replaced with `OnceLock<ShimDir>` (shims written once at test-binary startup). All four previously PATH-mutating tests now use `runner.with_bin(shims().XXX)` — no `unsafe set_var`, no `PATH_MUTEX`.

- **Changes in cycle 3 (`src/runner/claude_code.rs`):**
  - Removed `PATH_MUTEX` (was added in cycle 2). No `unsafe set_var` anywhere in tests.
  - Added `bin: PathBuf` field to `ClaudeCodeRunner`; added `#[cfg(test)] pub(crate) fn with_bin(mut self, bin: PathBuf) -> Self`.
  - Added `struct ShimDir` + `static SHIM_DIR: OnceLock<ShimDir>` + `fn init_shims() -> ShimDir` + `fn shims() -> &'static ShimDir`. Shims written once to `target/test-shims/shims-XXX/` (TempDir in cargo target, held for process lifetime).
  - `ShimDir.dir` annotated `#[allow(dead_code)]` (kept alive for TempDir drop semantics, never read directly).
  - Refactored `session_id_is_valid_uuid_v4_propagated_to_output`, `json_schema_arg_is_passed_inline`, `workspace_path_canonicalised_when_some`, `workspace_path_falls_back_to_inherited_when_none` to use `with_bin`.
  - `workspace_path_canonicalised_when_some`: workspace changed from `tempfile::tempdir()` to `CARGO_MANIFEST_DIR/target` (stable path, no /tmp inode churn); added `cwd_lock` acquisition.
  - `session_id_is_valid_uuid_v4_propagated_to_output`, `json_schema_arg_is_passed_inline`: workspace changed to `Some(CARGO_MANIFEST_DIR)` to bypass `resolve_cwd()`.
  - Canonicalize-once block at lines 329–334 byte-identical to cycle 1 (no behavioral change to production path).

- **Stability verification:** 50 consecutive `cargo test --all-features` runs under default parallelism — 0 failures (50/50). Previous cycle 2: 6/75 fails (~8%).
- **Scope:** `git diff --stat` from base: only `src/runner/claude_code.rs` changed in cycle 3 (all other file changes are cycle 1/2 work committed together).
- **Warnings:** 3 pre-existing `unused import: crate::db` — unchanged. No new warnings.

### Phase 2: tasks next-id verb
- **Status:** COMPLETE
- **Started:** 2026-05-02
- **Completed:** 2026-05-02
- **Commits:** pending code review (per scope discipline — do not commit)
- **Files Modified:**
  - `src/handlers/next_id.rs` — NEW: `run_next_id()` public entry point + pure `next_id_for_root(root: &Path)` inner; 6 unit tests; `OnceLock<Regex>`-cached `^T(\d{3,})(-|$)` pattern; lenient on missing dirs
  - `src/handlers/mod.rs` — added `pub mod next_id;` (alphabetized between `next_action` and `render`)
  - `src/cli/dynamic.rs` — added `"next-id"` to `WORKFLOW_VERBS`; added `build_next_id_cmd()`; registered in workflow-verbs block
  - `src/cli/dispatch.rs` — added `Some(("next-id", _sub))` arm sibling to `("status", sub)` arm
  - `tasks/active/T012-workspace-path-and-next-id/main.md` — this execution log
- **Notes:**
  - No plan deviations. All four files match the scope list exactly.
  - Pre-existing 3x `unused import: crate::db` warnings unchanged; no new warnings.
  - `cargo test --all-features`: 493 unit + 2 integration = 495 total. 5/5 consecutive runs clean.
  - Smoke check: `stores tasks next-id` from temp dir with `tasks/active/T012-test` + `tasks/completed/T005-done` → prints `T013`. AC2.6 satisfied.
  - `cargo run -- tasks --help` lists `next-id` with about-string. AC2.1 satisfied.
  - `tests/tasks_e2e.sh` and `tests/drive_e2e.sh` both PASS.
  - `git diff --stat` shows exactly 3 modified + 1 untracked = 4 in-scope files.

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

### Phase 1
- **Gate:** PASS (cycle 3 — flake eliminated; with_bin + OnceLock<ShimDir> + cwd_lock pattern verified)
- **Cycle 1 Gate:** REVISE — flake at AC1.7 + AC1.8 (~50%) on the two new runner tests. Resolution proposed: refactor to `Command::new(shim_path).env("PATH", ...)` pattern (lines 471-486).
- **Cycle 2 Gate:** REVISE — orchestrator chose a different mechanism (PATH_MUTEX + cwd lock applied to all four PATH-mutating tests in `claude_code.rs::tests`). Outcome: flake reduced from ~50% to ~8% (6 fails / 75 parallel runs) but not eliminated. Two distinct races: (A) `unsafe set_var(PATH)` is unsound regardless of inter-test mutex because libc's `setenv`/`getenv` are not thread-safe at the libc level — any parallel reader can observe a torn PATH and the runner's `Command::new("claude")` either ENOENTs or invokes the wrong binary; (B) `ETXTBSY` race on `runner_uses_path_shim_not_real_claude` — `fs::write(&shim)` then immediate `Command::new(shim).output()` can fail when the kernel has not released the write fd. Cycle 2 addressed neither root cause; it only narrowed Race A's window.
- **Cycle 3 Gate:** PASS — executor pivoted to the cycle-1 recommendation (and went one step further). Single-file fix in `src/runner/claude_code.rs` (commit `0687e9a`):
  - Production: new `bin: PathBuf` field on `ClaudeCodeRunner`, defaulted to `PathBuf::from("claude")`. `Command::new(&self.bin)` replaces `Command::new("claude")` — functionally identical for production (PATH lookup of bare-name `"claude"` is unchanged). Test-only `#[cfg(test)] pub(crate) fn with_bin(mut self, bin: PathBuf) -> Self` builder.
  - Test infrastructure: `static SHIM_DIR: OnceLock<ShimDir>` — four named shim scripts (`silent`, `planner`, `executor`, `cwd_printer`) written ONCE at first access into a `tempfile::TempDir` under `target/test-shims/`. `TempDir` is held by the OnceLock for the test-binary lifetime (intentional leak — avoids drop-vs-running-shim races). All four PATH-using tests now invoke `with_bin(shims().XXX.clone())` — no PATH mutation, no per-test shim write.
  - PATH_MUTEX removed entirely (cycle-2 scaffolding gone). Pre-existing `session_id` and `json_schema` tests pass `workspace_path: Some(CARGO_MANIFEST_DIR)` to bypass `resolve_cwd()` (avoids the cwd-dangling race from `paths::tests`' `set_current_dir(&tmp); drop(tmp)`).
  - `crate::paths::test_cwd_lock()` acquired in `workspace_path_canonicalised_when_some` (`:890`) and `workspace_path_falls_back_to_inherited_when_none` (`:922`) — serializes against `paths::tests` writers.
- **Revision Count:** 3/3
- **Test counts (independently verified, cycle 3):**
  - `cargo build --tests --all-features`: clean, no new warnings (3 pre-existing `unused import: crate::db` unchanged).
  - `cargo test --all-features` (parallel default): **12 pass / 0 fail across 12 back-to-back runs**. 489 tests each run (487 unit + 2 integration). Combined with the orchestrator's reported 15/15, the empirical sample is 27 consecutive clean parallel runs. Cycle-2's 8% flake is gone.
  - `bash tests/tasks_e2e.sh`: PASS.
  - `bash tests/drive_e2e.sh`: PASS.
- **Cycle-3 verification highlights:**
  - `with_bin` confirmed `#[cfg(test)] pub(crate)` at `claude_code.rs:94-95` — invisible in release builds.
  - Canonicalize-once block at `claude_code.rs:329-334` is byte-identical to cycles 1 & 2 — DONE_WHEN clause 2 (SDK guard) preserved structurally and by inline comment at lines 327-328.
  - `OnceLock<ShimDir>` is sound: each test uses a DIFFERENT named shim, no test mutates a shim after init, lifetime is process-wide.
  - Scope: cycle-3 commit (`0687e9a`) touched only `src/runner/claude_code.rs` + this `main.md`. Cumulative T012 diff still touches the seven in-scope files — no Phase 2 territory disturbed.
  - Assertions preserved: all four PATH-using tests still call `runner.spawn(...)`; assertions byte-equivalent to cycle 1 except for the `workspace_path: Some(CARGO_MANIFEST_DIR)` arg added to `session_id` and `json_schema` (which routes around the racing dependency without changing what is asserted).
- **Process observation (informational, not blocking):** cycle-3 commit `0687e9a` was authored by `Blake Sims` with `Co-Authored-By: Claude Sonnet 4.6` — i.e. the executor pair, not the orchestrator. The workflow assigns commit responsibility to the orchestrator at phase boundaries; this was bypassed. Work itself is sound; gate is unaffected. Surfaced for the orchestrator's process-improvement loop.

> Details: code-review-phase-1.md (Cycle 3 section)

### Phase 2
- **Gate:** PASS
- **Reviewed:** 2026-05-02
- **Revision Count:** 0/3
- **Acceptance criteria (all 6 verified):**
  - AC2.1 — `next-id` registered on `tasks --help` with the planned about-string. Verified by `stores tasks --help` against an init+install fixture in `/tmp/t012-smoke`. Verb is correctly gated on `schema.workflow.is_some()` (`dynamic.rs:219`); `observations --help` does NOT list it (control verified).
  - AC2.2 — Smoke check from this repo's working tree: `stores tasks next-id` printed `T013\n`. Re-running printed `T013\n` again — idempotent / no state mutation.
  - AC2.3 — `git status --porcelain` after smoke run was unchanged (only the in-scope Phase 2 files). The dispatcher's pre-match `db::open()` at `dispatch.rs:27-28` does create `.stores/db.sqlite` if absent, but that path is gitignored and pre-existed — pre-existing dispatcher behavior, not a Phase 2 regression. `next_id::run_next_id()` itself does no DB or fs writes.
  - AC2.4 — All 6 new tests pass: `empty_tree_returns_t001`, `single_completed_t005_returns_t006`, `highest_in_archived_wins`, `missing_directories_treated_as_empty`, `non_task_entries_ignored`, `non_canonical_directories_ignored` (the last asserts `tasks/ongoing/T999-x` is correctly excluded).
  - AC2.5 — `cargo test --all-features`: 493 unit + 2 integration = **495 pass / 0 fail across 6 consecutive runs**. Phase 1's deterministic baseline preserved (no flake re-introduced).
  - AC2.6 — Confirmed `T013` printed in this repo's working tree (highest = T012 in `active/`).
- **Implementation quality (verified against Decision Matrix rows 6–14):**
  - Row 13 (test seam): pure `next_id_for_root(root: &Path) -> Result<String>` is the inner function; thin `run_next_id()` resolves cwd and prints. Tests call only `next_id_for_root` with `tempdir()` paths; **no `set_current_dir` anywhere**. Safe under parallel test harness.
  - Row 12 (regex): `^T(\d{3,})(-|$)` — independently verified against 8 edge cases (`T001`✓, `T012-foo`✓, `T9`✗, `T9999-x`✓, `Toops-x`✗, `T001.bak`✗, `T999-x`✓, `T009foo`✗). All match plan-reviewer's strengths bullet.
  - Caching: `task_id_re()` uses `static RE: OnceLock<Regex>` + `get_or_init` — compiled once per process, not per directory entry. Correct.
  - Row 8 (lenient missing dirs): `if !subdir.exists() { continue; }` plus a defensive `match read_dir(...) { Err(_) => continue, ... }`. No error returned for missing dirs.
  - Row 14 (non-canonical dirs): `CANONICAL_SUBDIRS` is the exact five-dir list; `tasks/ongoing/` is silently ignored. Test `non_canonical_directories_ignored` verifies this with a `T999-x` decoy in `ongoing/`.
  - Row 7 (output): `println!("{}", next)` — exactly `T013\n`, no JSON, no debug, no prefix.
  - Row 6 (CLI registration): `"next-id"` added to `WORKFLOW_VERBS` reserved list; `build_next_id_cmd()` registered inside the `schema.workflow.is_some()` block; dispatch arm sibling to `("status", sub)`. All three plan-mandated CLI changes present and correctly placed.
- **Scope discipline:** `git diff --stat` shows exactly the four in-scope files (`src/cli/dispatch.rs`, `src/cli/dynamic.rs`, `src/handlers/mod.rs`, `tasks/active/T012-.../main.md`) plus the new `src/handlers/next_id.rs`. **No Phase 1 files touched** — no edits to `schema.yaml`, `runner/*`, `handlers/drive.rs`, or `handlers/guide.rs`. `git diff Cargo.toml Cargo.lock` is empty — no new dependencies (`regex`, `anyhow`, `tempfile` all pre-existed).
- **e2e:** `tests/tasks_e2e.sh` PASS, `tests/drive_e2e.sh` PASS — no regression in workflow happy/revise/wrap/accept paths or actor enforcement.
- **Warnings:** 3 pre-existing `unused import: crate::db` warnings unchanged. No new warnings.
- **Observations (informational, non-blocking):**
  - **Dispatcher cost.** `dispatch.rs:27-28` opens the SQLite DB unconditionally before matching the verb, so `next-id` (which doesn't need the DB) still pays the open + WAL pragma. Cosmetic, pre-existing dispatcher shape; not in Phase 2 scope to refactor. Could be addressed in a future cleanup task by hoisting the DB open into the verb arms that need it.
  - **`tasks/ongoing/`.** Plan-reviewer's Q1 still stands — the directory exists in this repo but is non-canonical. Phase 2 correctly ignores it; whether to delete or canonicalize it is a separate cleanup.
  - **Process.** Phase 2 was clean on the first executor pass (revision count 0/3) — sharp contrast to Phase 1's 3-cycle flake-elimination saga. Validates the planning-rigor investment: a small, well-scoped phase with locked decisions and a pure-function test seam shipped right.

> No long-form review file needed — the AC verification above is the full record.

---

## Completion

- **Completed:** 2026-05-02
- **Summary:** Shipped both substrate-side hooks for T011's wrapper boundary. (1) Optional `workspace_path: text` field on the `tasks` row; drive validates pre-spawn (errors loud on missing path AND on path-is-a-file — the latter caught by CodeRabbit Stage 6); `ClaudeCodeRunner::spawn` canonicalizes the path once at spawn entry, preserving the SDK session-fresh-on-cwd-mismatch guard. `MockRunner` updated, `Runner::spawn` trait gains `Option<&str>`, drive + guide call sites threaded. (2) Read-only `stores tasks next-id` verb: pure `next_id_for_root(&Path)` + thin CLI wrapper, scans the five canonical `tasks/{active,planning,paused,completed,archived}/` dirs (lenient on missing, ignores non-canonical `tasks/ongoing/`), `OnceLock<Regex>`-cached `^T(\d{3,})(-|$)`, prints `T{:03}\n`. Phase 1 took 3 cycles to land deterministically under parallel test harness (initial flake from `unsafe set_var(PATH)` libc-level race; eliminated cycle 3 via `ClaudeCodeRunner::with_bin` injection + `OnceLock<ShimDir>`). Phase 2 PASS first cycle. Stage 6 CodeRabbit: 1 batch (2 findings, both legit, both inline-fixed: `is_dir()` validation + test rename), then No findings. Final test count: 494 unit + 2 integration = 496 deterministic across parallel harness. e2e scripts PASS.
- **Commits:**
  - `995b9e8` chore(T012): scaffold — Intent Contract + GTM row, status PLANNING
  - `9e90a82` chore(T012): plan READY — move to active/, status EXECUTING_PHASE_1
  - `0687e9a` feat(T012 Phase 1 cycle 3): eliminate test flake — with_bin + OnceLock shims + cwd_lock
  - `3b224f1` review(T012 P1 cycle-3): PASS — flake eliminated; status EXECUTING_PHASE_2
  - `1d7b352` feat(T012 Phase 2): add `stores tasks next-id` verb
  - `e5ae0e9` fix(T012 Stage 6 CR): is_dir validation + test rename for honesty
- **Lessons Learned:**
  - **`unsafe std::env::set_var` is unsafe in tests, not just unsound.** Pre-existing tests had used the pattern for months without failure. Adding two more pushed the parallel-execution probability over the visibility threshold. The fix is not a mutex (cycle 2 attempted that and only narrowed the window) but eliminating the global mutation entirely (`with_bin` injection — cycle 3). Don't paper over libc-level races with Rust-level locks.
  - **The orchestrator-fix budget is for fixes whose mechanism is correct AND scope is small.** Cycle 2's mutex was small (~21 LOC) but mechanically insufficient. When uncertain whether a fix actually closes the race, bounce to executor — they have the end-to-end context to verify.
  - **CodeRabbit caught a real bug Phase 1's review missed.** `exists()`-vs-`is_dir()` is a textbook "validate at the boundary" fix that the cycle-1 reviewer overlooked because the test suite happened to exercise only the missing-path branch. Stage 6 does real work even when per-phase reviews pass.
  - **Honest test names matter.** `workspace_path_set_and_exists_canonicalizes` claimed more than it verified (MockRunner doesn't canonicalize — the runner-level tests do). Renaming to `workspace_path_set_propagates_to_runner` cost zero and made the test layer more honest.
  - **Phase split was correct.** Phase 2 shipped clean while Phase 1 was burning revisions; entangling them would have cost more time. Defending phase splits in the Decision Matrix paid off.
  - **Carry-forward from T011 honored:** filled `## Completion` BEFORE flipping `Status: COMPLETE`. CR did not catch a workflow-protocol violation this time.
  - **Process miss (cycle 3 executor commit):** Executor committed `0687e9a` themselves instead of letting the orchestrator commit. Surfaced in cycle-3 review. Phase 2 brief explicitly added the no-commit rule and Phase 2 obeyed. Good signal that briefing-tightening works.
- **Worklog note:** [03-t012-workspace-path-and-next-id.md](../../../docs/worklog/2026-05-02/03-t012-workspace-path-and-next-id.md)
