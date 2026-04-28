# T003: Framework-bundled workflow agents + runtime-agnostic orchestrator

## Meta
- **Status:** EXECUTING_PHASE_6
- **Created:** 2026-04-28
- **Last Updated:** 2026-04-27
- **Blocked Reason:** —

## Task

### Executive intent

Stores commits to "DB-as-truth + framework-as-engine" (β architecture, shipped in T002) but currently depends on the external `task-workflow` Claude Code plugin for the agents that drive the engine. That's an architectural inversion: the framework owns the workflow but borrows the workers. T003 closes the inversion.

Reframing in three layers:

- **L0** — human ↔ harness (Claude Code, pi, terminal)
- **L1** — agent ↔ agent spawn mechanism (Task tool, `claude -p`, OpenAI Assistants, in-process SDK)
- **L2** — workflow protocol (stores)

Stores commits to L2. After T003, L0 (harness) and L1 (spawn mechanism) become opt-in adapters. The orchestrator moves out of a Claude Code skill and into the CLI itself: `stores tasks drive` loops `next-action → brief → spawn → submit-* → render` and shells out to whichever runner the user picked. Bundled agent system prompts live as plain markdown — runtime-neutral.

A second strand fixes the human boundary: when a workflow blocks, the user faces a CLI gate alone with no way to talk through the context. `stores gate <id> guide` and `stores tasks <id> guide` curate the relevant DB rows + spawn a guide agent that can read and write back via the same CLI verbs.

A third strand fixes observability: `stores tasks status --follow` polls the DB and prints workflow state so the user can see what's happening without an agent's tool-call stream.

### DONE_WHEN

> In a fresh repo with stores installed (no `task-workflow` plugin, no Claude Code skill manually wired), `stores setup && stores tasks drive --auto --claude-code` picks the highest-priority non-complete task, drives it through the full state machine to `complete` (or surfaces a real `blocked` with `stores gate <id> guide` available to the human), and `stores tasks status --follow <id>` shows live progress throughout.

### Scope

**In scope**

- Bundle 4 agent system prompts as plain `agents/*.md` files: `planner`, `plan-reviewer`, `executor`, `code-reviewer`. Lift CLI-protocol language (currently in brief templates) into agent system prompts; briefs become lighter and store-specific.
- `BUNDLED_AGENTS` registry mirroring `BUNDLED_SKILLS` (`include_str!` at compile time).
- `stores agents {list,install,uninstall}` subcommand mirroring `cli/skills.rs` line-for-line where possible.
- `stores tasks drive [--auto] [<id>] [--<runner>]` orchestrator handler that loops `next-action → brief → runner.spawn → submit-* → render`.
- Runner trait + one impl: `runner/claude_code.rs` (~100 LOC, shells out to `claude -p` headless). Cargo feature-gated (`--features runner-claude-code`).
- Auto-task-selection in `--auto`: pick highest-priority non-complete task in the current scope, claim the lock, drive.
- `stores gate <id> guide [--<runner>]` — full implementation. Builds context bundle (the gate row + linked task rows + recent reviews + open questions), spawns a guide agent that has read access via `stores ... show` and write access via `stores gate answer` (plus other applicable verbs). Returns when the user resolves or escapes.
- `stores tasks <id> guide [--<runner>]` — minimal form (dump curated context bundle, spawn guide agent without specialized tooling). Stub-quality acceptable for v0.3; expand in v0.4.
- `stores tasks status --follow [<id>]` — polled text output (no TUI). Shows current_phase, current_cycle, status, last event, time-in-state. Refresh every 1–2 s. Exit on `complete` or `blocked` or Ctrl-C.
- `stores setup` convenience — runs `init` + `install tasks` (and `install observations` + `install gate`) + `skills install --all` + `agents install --all`. Idempotent.
- Update `tasks:start` skill to a one-line wrapper that invokes `stores tasks drive --auto --claude-code` (kept for users who prefer `/tasks:start`).
- Tests: unit tests for new handlers; e2e extension to `tests/tasks_e2e.sh` or new `tests/drive_e2e.sh` covering at least: `setup` idempotency, `drive` happy path, runner stub-mode (avoid hitting real `claude` CLI in CI — runner trait should be mockable).
- README updates: `stores setup` quickstart, `drive`/`status`/`guide` usage, runner feature flags.
- Cargo version bump 0.2.0 → 0.3.0.

**Out of scope (deferred to v0.4)**

- `runs` event-log store (provenance audit trail).
- Streaming inside-agent telemetry in `status` — depends on `runs`.
- Phase-reviewer / merge-reviewer agents + lifecycle states.
- `tasks:wrap` skill (Stage 6 CodeRabbit + Stage 7 completion summary).
- Second runner (pi, headless Python). The runner trait must support it cheaply; we don't write it.
- HTTP/JSON API for tasks store.
- TUI dashboard (ratatui). `status --follow` text output is the v0.3 form.
- Migrating T001/T002 (legacy boundary holds).
- Full-fat `tasks <id> guide` with specialized tooling (v0.4).
- Inline-triage support (intentionally NOT in stores — the harness owns that mode).
- The 6 deferred bugs from the v0.1 handoff (still on the backlog; pick up incidentally if encountered).

**Should remain unchanged**

- All v0.1 + v0.2 schema features and CLI verbs.
- The `tasks:start` skill name and invocation surface (`/tasks:start`); only its body becomes a wrapper.
- The `task-workflow` plugin. T003 removes the *dependency* on it — users with it installed continue to work.
- Briefing template structure (templates may shrink, but the `stores tasks brief` interface is preserved).

### Proposed approach (high level)

Bottom-up, mirroring the T002 pattern:

1. **Agent registry + install surface** — author the 4 agent prompts; add `BUNDLED_AGENTS`; clone `cli/skills.rs` to `cli/agents.rs`. Cheap, self-contained, testable.
2. **Runner abstraction** — define a `Runner` trait (`fn spawn(role, prompt) -> Result<AgentOutput>`); ship `runner/claude_code.rs` behind a feature flag; ship a `runner/mock.rs` always-on for tests.
3. **`drive` handler** — new handler that owns the orchestration loop. Reuses existing `next-action`, `brief`, `submit-*`, `render` handlers internally. Picks runner via flag.
4. **`setup` command** — thin wrapper composing existing init/install/skills-install/agents-install.
5. **`status --follow`** — new handler; polls DB; prints text frames.
6. **Guide handlers** — `gate <id> guide` and `tasks <id> guide`. Build context bundle; spawn guide agent via the runner.
7. **`tasks:start` skill rewrite** — one-line wrapper.
8. **Tests + docs + version bump**.

The planner will turn this into ordered phases with acceptance criteria.

### Risks / assumptions

- `claude -p` output format is stable enough for the runner to parse. Mitigation: feature gate; the runner accepts a JSON output mode (`--output-format stream-json` or similar) and parses defensively. CI uses the mock runner, never real `claude`.
- Authoring 4 system prompts from scratch is the long pole. Mitigation: heavy reuse from existing brief templates and the `task-workflow` plugin's prompts (read for shape, do not copy verbatim — license + drift).
- The runner trait's shape may need to evolve when a second runner is added (v0.4). Mitigation: keep the trait deliberately minimal in v0.3; we will refactor at the second-runner moment.
- `stores setup` writing to `~/.claude/` (with `--global`) crosses a directory boundary. Mitigation: default `setup` to local-only (`.claude/`); require explicit `--global` to touch home.
- `drive --auto` task selection is a policy call. Mitigation: simplest sensible policy = highest-priority then oldest non-complete; document; punt fairness/queues to v0.4.

### Open decisions (resolved)

| Decision | Resolution |
|---|---|
| Workflow harness for T003 itself | **Legacy filesystem** (this is the last legacy task; T004+ uses stores `tasks:start` once T003 ships) |
| `status` form factor | **Polled text** (`status --follow`); no TUI in v0.3 |
| Cargo feature gating for runners | **Yes** (`--features runner-claude-code`); mock runner always built |
| `tasks:start` skill | **Keep as one-line wrapper** around `stores tasks drive --auto --claude-code` |
| Guide agent surface | **Full** for `gate <id> guide`; **stub-quality OK** for `tasks <id> guide` in v0.3 |
| Quickstart command name | **`stores setup`** |

---

## Plan

### Objective

Close the L1/L2 architectural inversion in stores: the framework owns the workflow engine but currently borrows the agent prompts from the external `task-workflow` Claude Code plugin. T003 bundles the five workflow agents into the binary as plain markdown (4 workflow + 1 guide), ships a `Runner` trait with a feature-gated `claude_code` impl, builds a `stores tasks drive` orchestrator that composes existing CLI verbs into the `next-action → brief → spawn → submit-* → render` loop, and adds two human-boundary affordances (`status --follow` polled telemetry, `gate <id> guide` curated context bundle). After T003 ships, a fresh repo can run `stores setup && stores tasks drive --auto --claude-code` with zero external plugin dependencies.

DONE_WHEN-tracing language: `--auto` selects the **next non-complete task by `created_at ASC`** (priority dropped from v0.3 — see Decision Matrix). The DONE_WHEN clause "highest-priority non-complete task" is satisfied in v0.3 by FIFO selection; queueing/fairness/priority lands in v0.4 if needed.

### Scope

- **In Scope:**
  - 5 bundled agent prompts (`agents/{planner,plan-reviewer,executor,code-reviewer,guide}.md`) authored to be CLI-native — they receive a brief, follow it, and submit via `stores tasks submit-*` (or, for `guide`, via `stores gate answer`). Lift CLI-protocol language out of the brief templates and into the agent prompts; briefs become lighter and store-specific.
  - `BUNDLED_AGENTS` registry + `cli/agents.rs` (clone of `cli/skills.rs`).
  - `runner` module: `Runner` trait + always-on `runner/mock.rs` + feature-gated `runner/claude_code.rs` (Cargo feature `runner-claude-code`).
  - `stores tasks drive [<id>] [--auto] [--<runner>] [--max-iters N]` handler — composes existing handlers; spawns the runner; loops until terminal state.
  - Auto-task-selection policy (`--auto`): oldest non-complete task by `created_at ASC`, skipping tasks with a live claim lock.
  - `stores setup [--global]` convenience — composes `init` + bundled-store installs + `skills install --all` + `agents install --all`. Idempotent. Local-only by default.
  - `stores tasks status --follow [<id>]` — polled text frames at 1-2 s; exits on terminal state or Ctrl-C.
  - `stores gate <id> guide [--<runner>]` — full guide handler: builds context bundle (gate row + linked task rows + recent reviews), spawns guide agent that has read access via `stores ... show` and write access via `stores gate answer`.
  - `stores tasks <id> guide [--<runner>]` — minimal stub: dump curated context bundle + spawn guide agent with no specialized tooling.
  - `tasks:start` skill rewritten as a one-line wrapper around `stores tasks drive --auto --claude-code`.
  - Cargo version bump 0.2.0 → 0.3.0.
  - `tests/drive_e2e.sh` (mock-runner-driven happy path through the state machine).
  - README updates: `stores setup` quickstart, `drive`/`status`/`guide` usage, runner feature flags.

- **Out of Scope (deferred to v0.4 unless noted):**
  - `priority` column on the `tasks` schema (FIFO `created_at ASC` only in v0.3).
  - `runs` event-log store; streaming inside-agent telemetry.
  - Phase-reviewer / merge-reviewer agents + lifecycle states.
  - `tasks:wrap` skill.
  - Second runner (pi, headless Python). The `Runner` trait must support it cheaply; we don't write it.
  - HTTP/JSON API; TUI dashboard.
  - Migrating T001/T002 (legacy boundary holds; T003 itself uses the legacy filesystem workflow).
  - Full-fat `tasks <id> guide` with specialized tooling (v0.4).
  - The 6 deferred bugs from the v0.1 handoff.

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | Bundled agents registry + `agents` CLI subcommand + author 5 agent prompts (incl. guide) | Medium |
| 2 | Runner trait + mock impl + feature-gated `claude_code` impl | Medium |
| 3 | `stores tasks drive` orchestrator handler (incl. `--auto` selection) | High |
| 4 | `stores setup` convenience composer | Low |
| 5 | `stores tasks status --follow` polled output | Low |
| 6 | Guide handlers: full `gate <id> guide` + stub `tasks <id> guide` | Medium |
| 7 | Skill rewrite (`tasks:start`) + version bump + README + drive e2e | Medium |

### Phase Details

#### Phase 1: Bundled agents registry + author agent prompts

- **Objective:** Ship 5 agent system prompts as plain markdown files embedded in the binary (4 workflow agents + the `guide` agent — handlers for `guide` land in Phase 6, but the prompt is authored here alongside its peers), with an `agents` CLI subcommand that mirrors `skills` line-for-line. Establishes the bundling story end-to-end and proves the install/uninstall surface before any orchestrator work.
- **Dependencies:** None.
- **Files to create:**
  - `agents/planner.md`
  - `agents/plan-reviewer.md`
  - `agents/executor.md`
  - `agents/code-reviewer.md`
  - `agents/guide.md` (prompt body authored here; guide *handler* arrives in Phase 6)
  - `src/cli/agents.rs` (clone of `src/cli/skills.rs`, with `AGENTS_DIR_GLOBAL = ~/.claude/agents/`, `AGENTS_DIR_LOCAL = ./.claude/agents/`)
  - `tests/fixtures/agent_outputs/planner.json`, `tests/fixtures/agent_outputs/plan-reviewer.json`, `tests/fixtures/agent_outputs/executor.json`, `tests/fixtures/agent_outputs/code-reviewer.json`, `tests/fixtures/agent_outputs/guide.json` — canonical role-keyed JSON envelope examples that Phase 3's parser asserts against.
- **Files to modify:**
  - `src/cli/mod.rs` — add `pub mod agents;`
  - `src/cli/dynamic.rs` — register `agents` subcommand parallel to `skills` in `build_root`
  - `src/main.rs` — dispatch `agents` matches to `cli::agents::run` (parallel to `skills`)
- **Acceptance Criteria:**
  - [ ] AC1.1: `stores agents list` prints exactly 5 entries (`planner`, `plan-reviewer`, `executor`, `code-reviewer`, `guide`) with installed/uninstalled annotations matching the `skills list` format.
  - [ ] AC1.2: `stores agents install --all` writes 5 files to `.claude/agents/<name>.md` and re-running is idempotent (same content → no error).
  - [ ] AC1.3: `stores agents install <name>` writes that single file; `--global` writes to `~/.claude/agents/` instead.
  - [ ] AC1.4: `stores agents uninstall <name>` removes the file; uninstalling non-existent is non-fatal.
  - [ ] AC1.5: Conflict detection — if a file exists at the destination with different content, the installer errors with the same message format used by skills (`exists with different content; remove or use --force`).
  - [ ] AC1.6: Each agent prompt's YAML frontmatter declares `name` (the role), `description` (one-line trigger description), and optionally a `tools` whitelist of tools the agent may call. The `effort` field is NOT used (Claude Code's first-party subagent spec does not define it). The frontmatter exists so the parallel `/tasks:start` Task-tool path can register agents in Claude Code's subagent registry; the `claude_code` runner spawn-path (Phase 3) does NOT depend on registry registration — see Decision Matrix row "Runner spawn path".
  - [ ] AC1.7: Each agent prompt body specifies the CLI-native protocol: (a) read your brief from stdin/argv, (b) do the work, (c) submit via the named `stores tasks submit-*` verb (or `stores gate answer` for the guide), (d) emit a **single JSON object as the last line of stdout**, schema-keyed by role (`{"role": "planner", "phases": [...], "decision_matrix": [...]}`, `{"role": "plan-reviewer", "gate": "READY", "summary": "...", "open_questions": [...]}`, `{"role": "executor", "commit": "...", "files_changed": [...], "summary": "..."}`, `{"role": "code-reviewer", "gate": "PASS", "counts": {"critical": 0, "major": 0, "minor": 0}, "summary": "...", "details": "..."}`, `{"role": "guide", "action": "answered" | "blocked" | "noop", "summary": "..."}`). The prompt does NOT rely on a Task tool, plugin, or harness-specific construct. Fixtures at `tests/fixtures/agent_outputs/<role>.json` are canonical references.
  - [ ] AC1.7a: Each prompt explicitly names the schema verb(s) it submits to (e.g. planner → `stores tasks submit-plan`; plan-reviewer → `stores tasks submit-plan-review`; executor → `stores tasks submit-execute`; code-reviewer → `stores tasks submit-review`; guide → `stores gate answer`) and shows the JSON-envelope shape it must emit on its final stdout line.
  - [ ] AC1.7b: Each prompt has a section addressing failure modes — what to emit when there are open questions, when the agent is blocked, and when context is insufficient. The guide prompt additionally documents authorized vs forbidden CLI verbs.
  - [ ] AC1.7c: The planner prompt mirrors the Stage-1-7 structure of the existing `task-workflow` plugin's planner (objective → scope → phases → phase details → decision matrix → plan-notes → review handoff) without copying verbatim — read for shape, do not duplicate (license + drift hygiene).
  - [ ] AC1.7d: Per-prompt length budget is **400-1200 lines**. Prompts shorter than 400 lines are presumed under-specified; longer than 1200 invites bloat. Reviewer enforces this floor/ceiling at code review.
  - [ ] AC1.8: `cargo build` succeeds with the new `include_str!` paths; `cargo test cli::agents` covers fresh-install / idempotent-reinstall / conflict / uninstall (mirroring the existing `cli::skills::tests`). An `all_agents_bundled` test asserts `BUNDLED_AGENTS.len() == 5` (parallel to `all_skills_bundled`).
  - [ ] AC1.9: `cli/agents.rs` is a near-mechanical clone of `cli/skills.rs`. Differences limited to: registry contents, target directory (`agents/` vs `skills/`), file extension (`.md` direct, no `SKILL.md` subdirectory). Doc-comment on the file notes that the asymmetry with skills' nested `<name>/SKILL.md` is intentional and platform-driven (Claude Code's subagent loader scans flat).

#### Phase 2: Runner abstraction

- **Objective:** Define the `Runner` trait and its first two implementations (always-on mock, feature-gated `claude -p` shell-out). No `drive` handler yet — phase ships a usable abstraction with full unit-test coverage so phase 3 can plug it in.
- **Dependencies:** Phase 1 (so spawning has a known agent prompt registry to reference).
- **Files to create:**
  - `src/runner/mod.rs` — `Runner` trait + `RunnerOutput` struct + factory `pub fn select(name: &str) -> Result<Box<dyn Runner>>`
  - `src/runner/mock.rs` — always-built; programmable canned-response queue used by tests
  - `src/runner/claude_code.rs` — feature-gated behind `runner-claude-code`; shells out to `claude -p` with `--output-format stream-json` (or text fallback) and parses defensively
- **Files to modify:**
  - `Cargo.toml` — add `[features]` table with `default = []`, `runner-claude-code = []`
  - `src/main.rs` — add `pub mod runner;`
- **Trait shape (locked for v0.3, may evolve at second runner):**
  ```rust
  pub struct RunnerOutput {
      pub stdout: String,
      pub stderr: String,
      pub exit_code: i32,
      pub final_message: Option<String>, // post-processed final agent output
  }
  pub trait Runner: Send {
      fn name(&self) -> &str;
      fn spawn(&self, role: &str, system_prompt: &str, brief: &str) -> Result<RunnerOutput>;
  }
  ```
- **Acceptance Criteria:**
  - [ ] AC2.1: `cargo build` succeeds without features; `cargo build --features runner-claude-code` succeeds.
  - [ ] AC2.2: `cargo test runner::mock` covers: queued response is returned; empty queue errors with a clear message; `name()` returns `"mock"`.
  - [ ] AC2.3: `runner::select("mock")` returns the mock runner; `select("claude-code")` is `Ok` only when the feature is enabled, `Err` with a useful message otherwise; unknown names error with the list of available runners.
  - [ ] AC2.4: `runner::claude_code` has at least one unit test that uses a fixture `claude` shim on `PATH` (or a `which` injection point) to verify command-line construction; CI does not invoke real `claude`.
  - [ ] AC2.5: The trait is documented with a doc-comment block explaining v0.3's deliberate minimalism and which extensions (streaming, cancellation) are deferred.
  - [ ] AC2.6: No file outside `src/runner/` references `claude_code` directly; `select` is the only entry point.

#### Phase 3: `stores tasks drive` orchestrator

- **Objective:** Add a new workflow handler that composes existing `next-action`, `brief`, `submit-*`, and `render` handlers into the loop, picks a runner via flag, and drives a task to a terminal state. This is the centerpiece phase.

  Tests in this phase install the bundled agents via `stores agents install --all` directly; the user-facing `stores setup` convenience composer arrives in Phase 4.

  The `compute_*` functions on `brief.rs`, `submit.rs`, and `render.rs` are already `pub(crate)` — `drive` lives in the same crate, so direct in-process function calls work without any visibility changes. **No `pub` widening is required.**
- **Dependencies:** Phase 1 (agent prompts + `BUNDLED_AGENTS` registry + JSON envelope fixtures), Phase 2 (runner trait).
- **Files to create:**
  - `src/handlers/drive.rs` — main loop + `--auto` selection + safety rails (max-iters, cycle detection) + **role-keyed JSON-envelope parser** that decodes the runner's final-stdout-line into a typed enum and dispatches to the correct `compute_submit_*`.
  - `tests/handlers/drive_runner_error.rs` (or equivalent under `src/handlers/drive.rs` `#[cfg(test)] mod tests`) — fixture asserting that when the mock runner returns a non-zero exit mid-loop, the task row is byte-identical to its pre-iteration state (no partial `submit-*`).
- **Files to modify:**
  - `src/handlers/mod.rs` — register module
  - `src/cli/dynamic.rs` — register `drive` subcommand on workflow-shaped stores (parallel to `next-action`/`brief`)
  - `src/cli/dispatch.rs` — route `drive` to handler
- **Drive loop (pseudocode):**
  1. Resolve target id: explicit positional arg, else `--auto` selects by `created_at ASC` filtered to non-terminal (`status NOT IN ('complete','blocked')`) **and skipping rows where `claimed_by IS NOT NULL` AND `claimed_at` is within the lock-expiry window**; pick first; bail with explicit error when no candidates remain after the live-claim skip.
  2. Loop:
     a. Compute `next_action`. If terminal (`complete`/`blocked`), exit with appropriate exit code.
     b. Compute `brief` for the next agent.
     c. Read agent system prompt from the bundled `agents/<role>.md` (NOT from disk — `include_str!` via `BUNDLED_AGENTS`).
     d. `runner.spawn(role, system_prompt, brief)`.
     e. Parse runner output: take the **last non-empty line of stdout**, attempt to parse it as a role-keyed JSON object, and route to the matching `compute_submit_*` (planner → `compute_submit_plan`, plan-reviewer → `compute_submit_plan_review`, executor → `compute_submit_execute`, code-reviewer → `compute_submit_review`). On parse failure, surface the runner stdout/stderr and exit non-zero **without** invoking any `submit-*`.
     f. Render. Emit one-line stderr progress (`[T001] phase 2 cycle 1: executor → submitted`).
     g. Increment iter counter; bail if `--max-iters` hit (default 50).
- **Acceptance Criteria:**
  - [ ] AC3.1: `stores tasks drive <id> --mock <fixture>` (mock runner with a pre-loaded queue) drives a fixture task from `planning` to `complete` in a single invocation; final `next-action` reports `status=complete`.
  - [ ] AC3.2: `stores tasks drive --auto --mock <fixture>` selects the **next non-complete task by `created_at ASC`** (`WHERE status NOT IN ('complete', 'blocked') AND (claimed_by IS NULL OR claimed_at < now - lock_window) ORDER BY created_at ASC LIMIT 1`); with no candidates, errors with a clear message ("no non-complete tasks available"). The selection criterion is documented in `--help`.
  - [ ] AC3.3: `--mock` is **always built**, **hidden from `--help`** (clap `.hide(true)`), and accepts a path to a queued-response fixture file. `--claude-code` requires the `runner-claude-code` cargo feature; when missing, prints a remediation message ("rebuild with `cargo install --features runner-claude-code`").
  - [ ] AC3.4: Progress lines go to stderr (one per iteration); no progress noise on stdout. Stdout reserved for any structured output (`--json` aware).
  - [ ] AC3.5: `--max-iters N` (default 50) bounds the loop; on hit, exits non-zero with a clear "max iterations exceeded" message and current state summary.
  - [ ] AC3.6: When the runner errors mid-loop (non-zero exit, panic, or unparseable JSON envelope), drive surfaces the runner's stderr verbatim and exits non-zero — does NOT corrupt task state (no `submit-*` is called for that iteration). The fixture test at `tests/handlers/drive_runner_error.rs` (or equivalent path noted in Files-to-create) asserts the task row is unchanged across the failed iteration.
  - [ ] AC3.7: `cargo test handlers::drive` covers: happy path through 1 phase (mock); auto-selection ordering by `created_at ASC`; live-claim skip; max-iters bound; runner-error abort (mid-loop); terminal-state early exit.
  - [ ] AC3.8: Drive composes existing handlers via in-process function calls — does NOT shell out to itself. Calls go through the existing `pub(crate) compute_*` functions on `brief.rs`, `submit.rs`, and `render.rs`; no public-API widening is performed.
  - [ ] AC3.9: When the next-action result is a `blocked` status, drive exits 0 (not an error — block surfaced cleanly) and prints a one-line "blocked: <reason>; run `stores gate <id> guide` for help" hint.
  - [ ] AC3.10: **Agent output protocol**: each agent's final message is a single JSON object on the last line of stdout, schema-keyed by role. Drive parses that JSON and calls the matching `compute_submit_*`. Asserted against the role fixtures at `tests/fixtures/agent_outputs/<role>.json` shipped in Phase 1. Agent commentary above the final line is tolerated and discarded.

#### Phase 4: `stores setup` quickstart

- **Objective:** Single-command bootstrap. Composes `init` + bundled-store installs + skills install + agents install. Idempotent.
- **Dependencies:** Phase 1 (agents install must exist).
- **Files to create:**
  - `src/cli/setup.rs` — composer; calls `cli::init::run`, `install::run` for each bundled store, `cli::skills::run(SkillsCmd::Install { all: true, ... })`, `cli::agents::run(AgentsCmd::Install { all: true, ... })`
- **Files to modify:**
  - `src/cli/mod.rs` — add `pub mod setup;`
  - `src/cli/dynamic.rs` — register `setup` subcommand
  - `src/main.rs` — dispatch
- **Acceptance Criteria:**
  - [x] AC4.1: `stores setup` in a fresh directory creates `.stores/db.sqlite`, `.stores/manifest.yaml`, installs all 3 bundled stores (`observations`, `gate`, `tasks`), installs all 5 bundled skills under `./.claude/skills/`, installs all 5 bundled agents (`planner`, `plan-reviewer`, `executor`, `code-reviewer`, `guide`) under `./.claude/agents/`.
  - [x] AC4.2: Re-running `stores setup` is idempotent — exits 0, prints idempotency notes per layer ("Already initialized" / "Already installed: X").
  - [x] AC4.3: `stores setup --global` writes skills+agents to `~/.claude/` instead of local; the store DB still goes to `./.stores/`.
  - [x] AC4.4: Partial-state recovery: if `.stores/` exists but agents are missing, re-running `setup` only adds the missing layer (does not error or wipe).
  - [x] AC4.5: `cargo test cli::setup` covers fresh-bootstrap and idempotent-rerun in a tempdir.
  - [x] AC4.6: A failure in any layer (e.g. one bundled store install errors) aborts subsequent layers and surfaces the underlying error — no half-installed state is silently left behind.

#### Phase 5: `stores tasks status --follow`

- **Objective:** Polled text-frame observability. No TUI.

  **`status` vs `show` distinction (intentional noun choice):** `stores tasks show <id>` prints the **full task row** (every column, JSON-shaped — the existing v0.2 verb). `stores tasks status <id>` prints a **workflow telemetry frame** — a compact one-line view of `current_phase / current_cycle / status / next-action / blocked` aimed at humans watching live. They are not redundant: `show` is a debug dump; `status --follow` is a live tail.
- **Dependencies:** None hard, but conceptually pairs with `drive` from phase 3.
- **Files to create:**
  - `src/handlers/status.rs` — single handler with a polling loop
- **Files to modify:**
  - `src/cli/dynamic.rs` — register `status` subcommand on workflow-shaped stores; flags: `--follow`, optional positional `<id>`, `--interval <secs>` (default 1.5)
  - `src/cli/dispatch.rs` — route
- **Frame format:**
  ```
  [HH:MM:SS] T001 status=executing phase=2/3 cycle=1 next=executor blocked=false
  ```
- **Acceptance Criteria:**
  - [x] AC5.1: `stores tasks status <id>` (without `--follow`) prints a single frame and exits 0.
  - [x] AC5.2: `stores tasks status --follow <id>` re-prints a frame every interval; exits 0 on `complete` or `blocked`.
  - [x] AC5.3: `stores tasks status --follow` (no id) prints a multi-task table frame across all non-terminal tasks; exits when none remain or Ctrl-C.
  - [x] AC5.4: Ctrl-C is caught cleanly — last frame on screen, exit code 130.
  - [x] AC5.5: Frames suppress duplicate consecutive lines (same state → no spam); on state change, prints immediately.
  - [x] AC5.6: `cargo test handlers::status` covers single-frame mode + change detection (fixture row mutated mid-loop). Follow-loop tests are bounded by `--max-iters` test-only flag to avoid flakiness.

#### Phase 6: Guide handlers — `gate <id> guide` (full) + `tasks <id> guide` (stub)

- **Objective:** Human-boundary affordance. When the user faces a blocked task or gate, `guide` curates the relevant rows + spawns the guide agent (whose prompt was authored in Phase 1) and routes its JSON-envelope output back through the CLI. This phase is purely handler/dispatch work — no new prompt authoring.
- **Dependencies:** Phase 1 (`guide` agent prompt is already authored and registered in `BUNDLED_AGENTS`) and Phase 2 (runner trait).
- **Files to create:**
  - `src/handlers/guide.rs` — context-bundle builder + runner spawn for both `gate <id> guide` (full) and `tasks <id> guide` (stub)
- **Files to modify:**
  - `src/cli/dynamic.rs` — register `guide` subcommand on `gate` store (full) and `tasks` store (stub)
  - `src/cli/dispatch.rs` — route both
- **Context bundle (gate full form):**
  - The gate row (`stores gate show <id>`)
  - Linked task row if `task_ref` set (`stores tasks show <task_ref>`)
  - Recent plan-review log + last 2 cycle reviews from the linked task
  - The list of CLI verbs the guide is authorized to call (passed in the system prompt)
- **Context bundle (tasks stub form):**
  - The task row JSON
  - The last `next-action` output
  - The last cycle review (if any)
  - No specialized tooling beyond "ask the user clarifying questions and document them in `stores gate add` if a decision is needed"
- **Acceptance Criteria:**
  - [ ] AC6.1: `stores gate <id> guide --mock <fixture>` builds a context bundle (verifiable via mock runner capturing the prompt) that includes the gate row, the linked task row (if any), and the list of authorized CLI verbs.
  - [ ] AC6.2: `stores tasks <id> guide --mock <fixture>` builds a context bundle that includes the task row, last `next-action`, and last review.
  - [ ] AC6.3: `cargo test handlers::guide` covers both bundle shapes with fixture rows.
  - [ ] AC6.4: The guide agent prompt (authored in Phase 1) explicitly forbids editing main.md directly and instructs writes via `stores gate answer` / `stores tasks <verb>`. Phase 6 verifies via a parser-level test that the prompt body still contains the authorized-verbs list (`stores gate show`, `stores gate answer`, `stores tasks show`, `stores tasks list`, `stores tasks next-action`) and the explicit forbid-everything-else clause.
  - [ ] AC6.5: `gate guide` exits 0 if the gate row's `status` transitions from `pending` to `answered` during the session; otherwise exits 1 (covers runner crashes, user escape, and "agent ran but didn't answer" uniformly — no exit-code-2 / signal-capture distinction).
  - [ ] AC6.6: `tasks guide` is documented (in the agent prompt + README) as v0.3 stub-quality; expected expansion in v0.4.

#### Phase 7: Skill rewrite + version bump + README + drive e2e

- **Objective:** Final wire-up. Tighten the user-facing surface, prove the DONE_WHEN with a mock-runner-driven e2e (executor-side automation), and ship 0.3.0. A real-`claude` smoke is captured separately as a **manual soft gate** (see AC7.7 below) — it is recorded as evidence in the completion summary, not part of the executor's PASS/REVISE/FAIL cycle. The executor demonstrates DONE_WHEN coverage via the mock-runner e2e (AC7.1, AC7.1b) plus inspected agent prompts (Phase 1 ACs).
- **Dependencies:** Phases 1-6.
- **Files to create:**
  - `tests/drive_e2e.sh` — mock-runner-driven full-loop test (mirrors `tests/tasks_e2e.sh` shape, but uses `stores tasks drive --mock <fixture>` instead of manual `submit-*` calls)
  - `tests/fixtures/drive_e2e/happy_2phase.jsonl` — queued mock-runner responses for the AC7.1 happy path (N=2 phases, zero REVISE)
  - `tests/fixtures/drive_e2e/revise_once.jsonl` — queued responses for AC7.1b (one REVISE cycle on phase 2)
- **Files to modify:**
  - `skills/tasks:start/SKILL.md` — rewrite as a one-line wrapper: instructs the harness to invoke `stores tasks drive --auto --claude-code` (preserves the `/tasks:start` invocation surface; body shrinks ~95%).
  - `Cargo.toml` — version `0.2.0` → `0.3.0`.
  - `README.md` — replace the "13-step demo walk" intro with a `stores setup` quickstart at the top; add new sections for `drive`, `status --follow`, `gate guide`, `tasks guide`, and the runner feature flag.
  - `src/cli/skills.rs` — bump `BUNDLED_SKILLS` re-export count assertion (`all_skills_bundled` test) if needed.

- **Wrapper sketch (for AC7.3 — confirm 30-line budget realism):**
  ```markdown
  ---
  name: tasks:start
  description: Drive the next workflow task to completion via Claude Code.
  ---

  Invoke from the shell:

      stores tasks drive --auto --claude-code

  This selects the next non-complete task by `created_at ASC`, spawns the
  appropriate agent for the current workflow state via `claude -p`, and loops
  until the task reaches `complete` or `blocked`.

  If `blocked`, run `stores gate <id> guide --claude-code` to invoke the
  guide agent on the blocking gate.

  See `stores tasks drive --help` for flags (`--max-iters`, `--mock`).
  ```
  That's ~18 lines including frontmatter — well under 30. Budget confirmed.

- **Acceptance Criteria:**
  - [ ] AC7.1: `tests/drive_e2e.sh` (running against the **mock runner**) drives a fixture task with **N=2 phases and zero REVISE cycles** from `planning` to `complete` in a single `stores tasks drive` invocation. Final `stores tasks show` reports `status=complete`, `current_phase=2`, and both phases have one cycle each with PASS gates. The script seeds the task via `stores setup` + `stores tasks new`, runs `drive --mock <fixture>`, and asserts the final DB state.
  - [ ] AC7.1b: A second `tests/drive_e2e.sh` scenario (or a sibling fixture) drives a task with **one REVISE cycle** from `planning` to `complete`; the revised phase ends with cycle count = 2 and the final review gate is PASS.
  - [ ] AC7.2: `Cargo.toml` version is `0.3.0`; `cargo build` produces a `stores --version` of `0.3.0`.
  - [ ] AC7.3: New `tasks:start` body is ≤ 30 lines; runs the harness equivalent of `stores tasks drive --auto --claude-code` with no in-skill orchestration logic. (Wrapper sketch above demonstrates feasibility.)
  - [ ] AC7.4: README quickstart starts with `cargo install --path . --features runner-claude-code && stores setup && stores tasks drive --auto --claude-code` (the `--features` flag is part of the headline command — without it, the runtime emits a remediation message). The 13-step legacy walk moves to a "Manual workflow walk-through" subsection.
  - [ ] AC7.5: README documents the cargo feature flag (`--features runner-claude-code`) and lists the available runners (`mock`, `claude-code`).
  - [ ] AC7.6: `cargo test --all` passes; `cargo test --features runner-claude-code` also passes; `bash tests/tasks_e2e.sh` still passes (regression); `bash tests/drive_e2e.sh` passes.
  - [ ] AC7.7: **Manual soft gate** (NOT a hard executor gate). A `stores setup && stores tasks drive --auto --claude-code` run against a fresh test repo with a single seeded task is captured (transcript or screenshot) in the completion summary using a real `claude -p` runner. This is recorded as **v0.3 acceptance evidence at the merge level**, separate from the executor's PASS/REVISE/FAIL cycle. The executor proves DONE_WHEN via AC7.1 + AC7.1b + inspected agent prompts; AC7.7 is the human-driven smoke that verifies the prompts work with a real model. If the smoke fails, the right response is a follow-up issue against the prompts, not a rollback of the executor's PASS.
  - [ ] AC7.8: Version bump commit message states `T003 COMPLETE: framework-bundled agents + drive orchestrator on β architecture`.

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| Workflow harness for T003 itself | (a) legacy filesystem; (b) bootstrap onto stores | (a) | Locked in Intent Contract; bootstrapping a not-yet-shipped engine is a chicken-and-egg trap. T004+ uses stores. |
| `status` form factor | (a) polled text; (b) ratatui TUI; (c) JSON-only stream | (a) | Locked. Text scales; TUI ships in v0.4 with `runs` event log. |
| Runner gating | (a) cargo features; (b) runtime config; (c) always-built | (a) | Locked. Avoids dragging `claude`-CLI dependency into the bare binary. |
| `tasks:start` skill | (a) keep + wrapper; (b) delete; (c) move to setup-only | (a) | Locked. Preserves `/tasks:start` muscle memory; body becomes trivial. |
| `tasks <id> guide` depth | (a) full; (b) stub; (c) defer to v0.4 | (b) | Locked. Stub establishes the surface; full form needs `runs` and is too speculative for v0.3. |
| Quickstart name | (a) `stores setup`; (b) `stores quickstart`; (c) `stores bootstrap` | (a) | Locked. Matches industry idiom (`gh setup`, `npm setup`). |
| `setup` default scope | (a) local; (b) global; (c) ask | (a) | Locked. Crossing `~/.claude/` boundary requires explicit `--global` opt-in. |
| `--auto` task selection | (a) priority+oldest; (b) FIFO by `created_at ASC`; (c) explicit queue | (b) | Locked. The `tasks` schema has no `priority` column in v0.3 — adding one would require a schema bump + migration story (out of scope). FIFO via `WHERE status NOT IN ('complete','blocked') ORDER BY created_at ASC LIMIT 1` is testable today. Priority/queueing/fairness lands in v0.4 if needed. |
| Agent prompt format | (a) `agents/<name>.md` flat; (b) `agents/<name>/AGENT.md` mirror of skills | (a) | Claude Code's subagent loader scans flat `<base>/<name>.md` — nesting would prevent registry registration. Asymmetry with `cli/skills.rs` is intentional and platform-driven; `cli/agents.rs` doc-comment notes this. |
| Agent prompt frontmatter | (a) `name` + `description` + optional `tools`; (b) `name` + `description` + `effort` (task-workflow plugin shape); (c) free-form | (a) | Locked. Claude Code's first-party subagent spec uses `name` / `description` / optional `tools` / optional `model`. There is no `effort` field. The frontmatter only matters for the parallel `/tasks:start` Task-tool path (subagent registry); the `claude_code` runner spawn-path bypasses the registry. |
| Runner spawn path (`claude_code`) | (a) Task-tool / subagent-registry; (b) `claude -p` with prompt body via stdin or `--append-system-prompt`; (c) Claude SDK in-process | (b) | Locked for v0.3. The runner shells out to `claude -p`, feeding the bundled agent's prompt body directly (NOT relying on the agent being registered as a Claude Code subagent). The frontmatter exists for the orthogonal `/tasks:start` Task-tool path. (a) and (c) are deferred. |
| Agent output protocol | (a) trailing JSON object on stdout last line; (b) JSON-only stdout (strict); (c) sentinel-delimited blocks (e.g. `<<<BEGIN>>>...<<<END>>>`) | (a) | Locked. Tolerant of agent commentary above the final line; trivially generatable by both real and mock runners; parse logic is "take the last non-empty line, attempt JSON.parse, route by `role` key." Fixtures at `tests/fixtures/agent_outputs/<role>.json` are canonical. |
| `Runner` trait method | (a) `spawn(role, sys, brief) -> Result<Output>`; (b) async trait; (c) channel-based streaming | (a) | v0.3 deliberate minimalism; sync, single-shot, easy to mock. Streaming + async land at second-runner moment. |
| Mock runner availability | (a) always built, hidden via clap `.hide(true)`, takes `--mock <fixture-path>`; (b) `cfg(test)`-only; (c) feature-gated | (a) | Locked. `tests/drive_e2e.sh` is a shell script driving a release-mode binary; needs the mock runner accessible without a feature rebuild. Hidden visibility (clap `.hide(true)`) keeps it out of `--help` so it is not advertised as a stable user surface. |
| Drive loop composition | (a) in-process function calls; (b) shell out to self; (c) mixed | (a) | In-process is testable, atomic, and avoids fork overhead. The CLI verbs are thin wrappers around `compute_*` (`pub(crate)`) — `drive` calls those directly without `pub` widening. |
| `--max-iters` default | (a) 50; (b) 100; (c) unbounded | (a) | A 3-phase task with full revise budget hits ~12 iters; 50 is generous safety but bounded. |
| `drive` exit code on `blocked` | (a) 0 with hint; (b) non-zero error | (a) | A real `blocked` is a successful drive outcome — surfacing the block to the human is the deliverable. Reserve non-zero for runner failures and bugs. |
| `guide` write-access verbs | (a) only `gate answer`; (b) full task verbs; (c) read-only | (a) for gate; restrictive list documented in prompt | Smallest blast radius. Authorized list embedded in `agents/guide.md`: `stores gate show`, `stores gate answer`, `stores tasks show`, `stores tasks list`, `stores tasks next-action`. All other `stores` verbs explicitly forbidden. v0.4 expands as the trust model develops. |
| `guide` exit-code policy | (a) 0 on answered / 1 on runner error / 2 on user escape; (b) 0 if gate row transitions `pending→answered` else 1; (c) always 0 | (b) | Locked. Distinguishing "user escape" from "runner crash" requires signal handling that is unreliable across spawn paths. Single semantic check (DB transition) is testable and unambiguous. |
| Real-`claude` smoke gate | (a) hard merge-blocker inside Phase 7 PASS/REVISE; (b) manual soft gate captured as evidence in completion summary; (c) defer to v0.4 | (b) | Locked. (a) made executor success depend on a working `claude` CLI + credentials + non-deterministic model output — brittle and out of scope for executor automation. The mock-runner e2e (AC7.1/7.1b) + inspected agent prompts (Phase 1 ACs) prove DONE_WHEN at the executor level; the real-claude smoke is human-driven evidence at the merge level. |
| README quickstart command | (a) `cargo install --path . && stores setup && stores tasks drive --auto --claude-code`; (b) `cargo install --path . --features runner-claude-code && stores setup && stores tasks drive --auto --claude-code` | (b) | (a) silently fails at runtime because `--claude-code` requires the cargo feature. The headline command must be a working command. |
| README quickstart vs 13-step walk | (a) replace; (b) keep both with quickstart on top; (c) delete walk | (b) | Quickstart is the headline; the walk remains valuable for users debugging the framework internals. |

### Plan Notes

All open questions from the prior plan-review cycle have been adjudicated and folded into the ACs and Decision Matrix above. Specifically:

- **Q1** (priority column) → dropped from v0.3 entirely; FIFO `created_at ASC`. AC3.2 + Decision Matrix row updated.
- **Q2** (`--mock` exposure) → always built, hidden via clap `.hide(true)`, takes `--mock <fixture-path>` arg. AC3.3 updated.
- **Q3** (flat `agents/<name>.md` layout) → flat; `cli/agents.rs` doc-comment notes the platform-driven asymmetry with skills.
- **Q4** (guide authorized verbs) → exactly five verbs listed in `agents/guide.md` (Phase 1 authoring): `stores gate show`, `stores gate answer`, `stores tasks show`, `stores tasks list`, `stores tasks next-action`. All other `stores` verbs forbidden. AC6.4 verifies.
- **Q5** (`setup` phase ordering) → keep in Phase 4. Phase 1/2/3 tests use `stores agents install --all` directly until `setup` lands.

No open items remain for the plan-reviewer.

---

## Plan Review

- **Gate:** READY
- **Open Questions Finalized:** All Q1–Q5 locked in cycle 1; cycle 2 verifies application. (Q1 → drop priority, FIFO `created_at ASC`; Q2 → `--mock` always built, hidden via clap `.hide(true)`; Q3 → flat `agents/<name>.md`; Q4 → 5-verb authorized list embedded in `agents/guide.md`; Q5 → `setup` stays in Phase 4.)
- **Issues Found:** cycle 1: 4c / 7M / 6m → cycle 2: 0c / 0M / 1m. All 17 cycle-1 findings landed cleanly. One cosmetic cycle-2 nit (AC5.6 references a `--max-iters` flag not defined in Phase 5) — non-blocking. DONE_WHEN trace is solid: 10 of 10 clauses covered.

> Details: plan-review.md

---

## Execution Log
_Executor agent fills this section per phase._

### Phase 1: Bundled agents registry + author 5 agent prompts
- **Status:** AWAITING_REVIEW
- **Started:** 2026-04-28
- **Completed:** 2026-04-28 (executor returned)
- **Commits:** ae306cf
- **Files Created:** agents/{planner,plan-reviewer,executor,code-reviewer,guide}.md (415-502 lines each); src/cli/agents.rs; tests/fixtures/agent_outputs/{planner,plan-reviewer,executor,code-reviewer,guide}.json
- **Files Modified:** src/cli/mod.rs, src/cli/dynamic.rs, src/main.rs
- **Tests:** cargo test cli::agents PASSED (6 tests); cargo test PASSED (304 tests); cargo build PASSED
- **ACs claimed:** 1.1 ✓ 1.2 ✓ 1.3 ✓ 1.4 ✓ 1.5 ✓ 1.6 ✓ 1.7 ✓ 1.7a ✓ 1.7b ✓ 1.7c ✓ 1.7d ✓ 1.8 ✓ 1.9 ✓
- **Executor notes for code-reviewer:** uninstall_removes_file test mirrors skills' inline-fs pattern; agents.rs has no parent-dir cleanup since flat layout (no subdir to prune); prompt line counts at the lower end of 400-1200 range (415-502).

### Phase 4: `stores setup` quickstart
- **Status:** AWAITING_REVIEW
- **Started:** 2026-04-27
- **Completed:** 2026-04-27
- **Commits:** 718f5e3
- **Files Created:** src/cli/setup.rs
- **Files Modified:** src/cli/mod.rs, src/cli/dynamic.rs, src/main.rs, src/paths.rs (promoted CWD_LOCK to pub(crate) test_cwd_lock)
- **Tests:** cargo test cli::setup 2/2 PASS; cargo test 326/326 PASS; cargo build PASS
- **ACs claimed:** 4.1 ✓ 4.2 ✓ 4.3 ✓ 4.4 ✓ 4.5 ✓ 4.6 ✓
- **Executor notes for code-reviewer:**
  - Idempotent store re-install: install_bundled errors with "already installed" message; composer catches that substring and continues (treats it as success). No other error patterns from that function are suppressed.
  - Skills/agents install_all already passes `silent_if_same=true` — same-content files are silently skipped; different-content bails with error (propagated as AC4.6 abort).
  - Test isolation: both tests hold the shared `paths::test_cwd_lock()` mutex (process-wide) + call `set_current_dir` + restore CWD+HOME. This serialises against all other CWD-mutating tests in the binary (paths::tests uses the same lock). `unwrap_or_else(|e| e.into_inner())` on poison avoids cascading failures from prior panics.
  - AC4.3 (--global) is wired through all three sub-layers (skills, agents); init always uses local .stores/ regardless.
  - AC4.4 (partial recovery) falls out naturally from idempotency of each layer — no special composer logic needed.

### Phase 5: `stores tasks status --follow`
- **Status:** AWAITING_REVIEW
- **Started:** 2026-04-27
- **Completed:** 2026-04-27
- **Commits:** 5ee1809
- **Files Created:** src/handlers/status.rs
- **Files Modified:** src/cli/dynamic.rs, src/cli/dispatch.rs, src/handlers/mod.rs, Cargo.toml (libc direct dep)
- **Tests:** cargo test handlers::status 12/12 PASS; cargo test 338/338 PASS; cargo build PASS
- **ACs claimed:** 5.1 ✓ 5.2 ✓ 5.3 ✓ 5.4 ✓ 5.5 ✓ 5.6 ✓
- **Executor notes for code-reviewer:**
  - `libc` added as a direct dep (was already a transitive dep via rusqlite's bundled SQLite). Used only for the SIGINT handler (`libc::signal`). No other new deps.
  - SIGINT uses `unsafe libc::signal` (not `signal_hook`) — single `extern "C" fn` sets a static `AtomicBool`; loop polls it every 50 ms chunk during the sleep interval.
  - `run_follow_loop` is public and takes a `&Path` directly so tests bypass `db_path()` and use a tempdir path.
  - AC5.5 dedup keys on `(status, current_phase, current_cycle, blocked_reason)` — `StateKey` is a plain `#[derive(Eq, Hash)]` struct; `should_print` is a pure predicate tested independently.
  - `--max-iters` hidden flag: default `usize::MAX` in dispatch; tests pass 3 or 100 depending on what they test.
  - `total_phases` extracted from `plan` JSON column (`plan.phases.length`) for the `phase=N/M` slot; falls back to `-` if plan is null/absent.

### Phase 2: Runner abstraction
- **Status:** AWAITING_REVIEW
- **Started:** 2026-04-27
- **Completed:** 2026-04-27
- **Commits:** 61e4190 258251f
- **Files Created:** src/runner/mod.rs, src/runner/mock.rs, src/runner/claude_code.rs
- **Files Modified:** Cargo.toml (features table), src/main.rs (pub mod runner)
- **Tests:** cargo test runner (no feature): 8/8 PASS; cargo test --features runner-claude-code runner: 14/14 PASS; cargo build: PASS; cargo build --features runner-claude-code: PASS
- **ACs claimed:** 2.1 ✓ 2.2 ✓ 2.3 ✓ 2.4 ✓ 2.5 ✓ 2.6 ✓
- **Executor notes for code-reviewer:**
  - `available_runners()` uses cfg-based branching (not `mut vec + push`) to avoid unused-mut warning.
  - `MockRunner` uses `RefCell<Vec<_>>` (reversed for O(1) pop); `unsafe impl Send` justified because RefCell is not shared across threads — only moved.
  - `claude_code` tests avoid `std::env::set_var` (unsound in multi-threaded tests); shim is invoked directly via its absolute path; extract_final_message is tested as a pure function independently.
  - AC2.6 enforced: no file outside src/runner/ references `claude_code` directly — only `mod.rs`'s `#[cfg(feature)] pub mod claude_code` and `select()` routing.

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

### Phase 1
- **Gate:** PASS
- **Issues Found:** 0c/0M/3m
- **Revision Count:** 0/3
- **Verified:** All 13 ACs pass under inspection (1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.7a, 1.7b, 1.7c, 1.7d, 1.8, 1.9). `cargo test cli::agents` → 6 passed; `cargo test` → 304 passed; `cargo build` clean (release + dev). Live binary verification of install/uninstall/conflict/list passed. Skills regression check: `stores skills list` still works.
- **Minor findings (non-blocking):** (1) guide.md adds `stores gate list` to read-only verbs beyond Q4's strict 5-verb lockdown — all 5 mandated verbs still present + forbid clause intact, AC6.4 forward-test will pass; (2) planner.md has Stage 0 context gate prepended to the Stage 1-7 structure — useful safeguard, not a violation; (3) plan-reviewer.md at 415 lines is closest to the 400-line floor — substantive content confirmed.
- **Note:** executor reported "5 tests" but actual count is 6 (the bonus `flat_layout_not_nested` test). Cosmetic discrepancy, no impact.

> Details: code-review-phase-1.md

### Phase 2
- **Gate:** PASS
- **Issues Found:** 0c/1M/2m
- **Revision Count:** 1/3
- **Verified:** All 6 ACs (2.1–2.6). Test matrix matches claimed: `cargo test runner` 8/8, `cargo test --features runner-claude-code runner` 14/14. Full suite 312/318 (no-feature/with-feature) green. Dev + release builds clean both ways. AC2.6 leakage clean — `claude_code` referenced only inside `src/runner/`. Phase 1 regression (`cli::agents`, `stores agents list`, `stores skills list`) passes.
- **Major (M1):** `unsafe impl Send for MockRunner` at `src/runner/mock.rs:49-52` is redundant. `RefCell<Vec<RunnerOutput>>` is naturally `Send` via auto-trait derivation (verified by isolated `assert_send` probe). The impl is sound today but is dead code that masks any future `!Send` field regression. Recommend deletion (two-line fix) before or as part of Phase 3 P0 commit.
- **Minor findings:** (m1) reverse-then-pop FIFO is correct but `VecDeque::pop_front` would be idiomatic — style only; (m2) stale doc-comment in `runner_uses_path_shim_not_real_claude` (`src/runner/claude_code.rs:208-212`) references "the runner integration test below" that does not exist.
- **Note:** Executor's "RefCell not shared across threads" justification confuses `!Sync` with `!Send`. `RefCell<T>` is `!Sync` but `Send`-when-`T:Send` by auto-trait — no manual impl needed.

> Details: code-review-phase-2.md

### Phase 3: `stores tasks drive` orchestrator
- **Gate:** PASS
- **Issues Found:** 0c/0M/3m
- **Revision Count:** 0/3
- **Verified:** All 10 ACs (3.1–3.10). Test re-run: `cargo test handlers::drive` 12/12, `cargo test` 324/324, `cargo build` clean (default + `runner-claude-code` feature). Live `--help` confirms `--mock` hidden, `--claude-code` feature-gated. AC3.7's six required test scenarios all present and substantive (status/plan byte-equal pre/post for runner-error; live-claim uses real `now()`; max-iters asserts error message; commentary tolerated by parse_envelope; etc.). Public-API surface diff: 5 new pub items, all in drive.rs and all appropriate (DriveArgs, MockFixtureItem, run_drive, plus pub(crate) resolve_task_id/drive_loop); no widening of existing items.
- **AC3.8 deviation verdict:** ACCEPTED. `compute_brief`'s manifest lookup is purely a bundled-vs-filesystem branch selector; drive's inlined version commits to the bundled branch only, which is equivalent for bundled stores (the only store in v0.3, and the one DONE_WHEN scopes to). Executor was right that no public-API widening occurred, but for the wrong reason — `build_context` and `render_template` are already `pub` (re-exported via `pub use` in `src/render/mod.rs:7-8`), not `pub(crate)`. The pre-existing surface absorbed the new caller cleanly.
- **Minor findings (non-blocking):** (m1) `LOCK_WINDOW_SECS=300` in drive.rs is a redefinition, not a shared constant — submit.rs hardcodes the literal `300`; recommend lifting to one shared `pub(crate)` const in a follow-up. (m2) Drive's bundled-only limitation (filesystem-installed stores hard-fail at "drive requires a bundled store") is undocumented in `--help` — add one line. (m3) Render-failure logging is silent-by-design for v0.3, but the per-iteration progress message reads `na.current_phase`/`current_cycle` from before the submit, so the printed phase lags one step behind reality — cosmetic for `status --follow`.
- **Phase 4–7 plug-in:** Confirmed — `run_drive(schema, DriveArgs)` is stable; CLI args are stable; downstream phases need no rework.

> Details: code-review-phase-3.md

### Phase 4: `stores setup` quickstart
- **Gate:** PASS
- **Issues Found:** 0c/0M/3m
- **Revision Count:** 0/3
- **Verified:** All 6 ACs (4.1–4.6). `cargo test cli::setup` 2/2, `cargo test` 326/326 (was 324 pre-Phase-4 — +2 new, no regressions). Manual `stores setup` in tempdir creates db + manifest + 3 stores + 5 skills + 5 agents; manifest scopes correct (worktree/worktree/repo). Re-run exits 0; missing-layer recovery verified by removing `.claude/agents/` between runs (only that layer reinstalled). `--global` lands skills+agents in `$HOME/.claude/`; real `~/.claude/agents/` confirmed clean. AC4.6 abort: all four layers use `?`; bundled-stores loop only swallows `"already installed"` substring and re-raises everything else.
- **paths.rs deviation verdict:** ACCEPT. `pub(crate) fn paths::test_cwd_lock()` under `#[cfg(test)]` is the minimum exposure: existing `paths::tests` already had a private mutex with the same role, and setup tests genuinely cannot avoid mutating CWD (the production `init::run` / `skills_run` / `agents_run` resolve via `current_dir()` internally — unlike `cli::skills::tests` which use an explicit-base helper). No release-build surface widened. Public-API delta: `pub mod setup`, `pub fn setup::run`, `pub(crate) fn paths::test_cwd_lock` (cfg-test) — three additions, all required.
- **Minor findings (non-blocking):** (m1) skills/agents layers print no per-item idempotency message on re-run because `install_all` passes `silent_if_same: true` (`src/cli/skills.rs:135-140`, `src/cli/agents.rs:148-153`); init and stores layers comply, skills/agents are silent. Spec-cosmetic, not correctness. (m2) `with_isolated_env` (`src/cli/setup.rs:94-123`) is not panic-safe — CWD/HOME restoration is sequential after the closure body, so an `assert!` panic skips it; the shared `test_cwd_lock` is poison-recovered so the suite won't deadlock but state can leak. Use a `Drop` guard. (m3) Substring match `"already installed"` (setup.rs:37) is brittle; a typed `InstallOutcome::AlreadyInstalled` would be more robust — punch-list, not a defect.

> Details: code-review-phase-4.md

### Phase 5: `stores tasks status --follow`
- **Gate:** PASS
- **Issues Found:** 0c/0M/4m
- **Revision Count:** 0/3
- **Verified:** All 6 ACs (5.1–5.6). `cargo test handlers::status` 12/12; full suite 338/338 (was 326 → +12 new); two consecutive runs no flakes; `cargo build` clean. Live e2e in `/tmp/p5-e2e`: single-frame exits 0; `--follow` on terminal task exits 0; multi-task indented frame matches spec; `kill -INT $PID` → exit 130 with last frame retained; `--max-iters 3 --interval 0.1` runs deterministic. Frame format byte-matches spec: `[HH:MM:SS] T001 status=executing phase=2/3 cycle=1 next=executor blocked=false`. `--max-iters` confirmed hidden in `--help`. `run_follow_loop(&Path, StatusArgs)` is the test injection seam — used by all three loop tests.
- **libc + signal handling verdict:** ACCEPT. libc was already transitive (`getrandom` + `tempfile`); promoting to direct dep is honest. Signal handler is sound — `static AtomicBool` const-init, handler does only `store(true, SeqCst)` (async-signal-safe; no allocation, no locks). 50ms chunked sleep keeps Ctrl-C latency bounded. Portability adequate for advertised Linux+macOS surface (no `#[cfg(unix)]` guard recorded as m1-equivalent for v0.4).
- **Public-API delta:** 10 new `pub` items, all in `handlers::status` (binary-internal module reachable only from `dispatch.rs` and the test harness). No leakage into installed CLI surface.
- **Minor findings (non-blocking):** (m1) plan with empty `phases:[]` array renders `phase=N/0` instead of `N/-` — `Some(0)` should fall back to None in the formatter (live-verified edge case); (m2) no `should_print` test for `blocked_reason` change — coverage gap in an otherwise-complete matrix; (m3) two `clippy::map_identity` instances `prev_keys.get(id).map(|k| k)` at status.rs:331, 344; (m4) sleep loop subtracts `chunk` instead of `sleep_for` from remaining — equivalent under saturating-sub but reads sloppily.

> Details: code-review-phase-5.md

---

## Completion
_Final summary when task is complete._

- **Completed:** [DATE]
- **Summary:** ...
- **Commits:** ...
- **Lessons Learned:** ...
