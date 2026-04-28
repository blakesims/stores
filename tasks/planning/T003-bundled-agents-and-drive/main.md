# T003: Framework-bundled workflow agents + runtime-agnostic orchestrator

## Meta
- **Status:** PLAN_REVIEW
- **Created:** 2026-04-28
- **Last Updated:** 2026-04-28
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

Close the L1/L2 architectural inversion in stores: the framework owns the workflow engine but currently borrows the agent prompts from the external `task-workflow` Claude Code plugin. T003 bundles the four workflow agents into the binary as plain markdown, ships a `Runner` trait with a feature-gated `claude_code` impl, builds a `stores tasks drive` orchestrator that composes existing CLI verbs into the `next-action → brief → spawn → submit-* → render` loop, and adds two human-boundary affordances (`status --follow` polled telemetry, `gate <id> guide` curated context bundle). After T003 ships, a fresh repo can run `stores setup && stores tasks drive --auto --claude-code` with zero external plugin dependencies.

### Scope

- **In Scope:**
  - 4 bundled agent prompts (`agents/{planner,plan-reviewer,executor,code-reviewer}.md`) authored to be CLI-native — they receive a brief, follow it, and submit via `stores tasks submit-*`. Lift CLI-protocol language out of the brief templates and into the agent prompts; briefs become lighter and store-specific.
  - `BUNDLED_AGENTS` registry + `cli/agents.rs` (clone of `cli/skills.rs`).
  - `runner` module: `Runner` trait + always-on `runner/mock.rs` + feature-gated `runner/claude_code.rs` (Cargo feature `runner-claude-code`).
  - `stores tasks drive [<id>] [--auto] [--<runner>] [--max-iters N]` handler — composes existing handlers; spawns the runner; loops until terminal state.
  - Auto-task-selection policy (`--auto`): highest priority, then oldest non-complete in the current scope.
  - `stores setup [--global]` convenience — composes `init` + bundled-store installs + `skills install --all` + `agents install --all`. Idempotent. Local-only by default.
  - `stores tasks status --follow [<id>]` — polled text frames at 1-2 s; exits on terminal state or Ctrl-C.
  - `stores gate <id> guide [--<runner>]` — full guide handler: builds context bundle (gate row + linked task rows + recent reviews), spawns guide agent that has read access via `stores ... show` and write access via `stores gate answer`.
  - `stores tasks <id> guide [--<runner>]` — minimal stub: dump curated context bundle + spawn guide agent with no specialized tooling.
  - `tasks:start` skill rewritten as a one-line wrapper around `stores tasks drive --auto --claude-code`.
  - Cargo version bump 0.2.0 → 0.3.0.
  - `tests/drive_e2e.sh` (mock-runner-driven happy path through the state machine).
  - README updates: `stores setup` quickstart, `drive`/`status`/`guide` usage, runner feature flags.

- **Out of Scope (deferred to v0.4 unless noted):**
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
| 1 | Bundled agents registry + `agents` CLI subcommand + author 4 agent prompts | Medium |
| 2 | Runner trait + mock impl + feature-gated `claude_code` impl | Medium |
| 3 | `stores tasks drive` orchestrator handler (incl. `--auto` selection) | High |
| 4 | `stores setup` convenience composer | Low |
| 5 | `stores tasks status --follow` polled output | Low |
| 6 | Guide handlers: full `gate <id> guide` + stub `tasks <id> guide` | Medium |
| 7 | Skill rewrite (`tasks:start`) + version bump + README + drive e2e | Medium |

### Phase Details

#### Phase 1: Bundled agents registry + author agent prompts

- **Objective:** Ship the 4 agent system prompts as plain markdown files embedded in the binary, with an `agents` CLI subcommand that mirrors `skills` line-for-line. Establishes the bundling story end-to-end and proves the install/uninstall surface before any orchestrator work.
- **Dependencies:** None.
- **Files to create:**
  - `agents/planner.md`
  - `agents/plan-reviewer.md`
  - `agents/executor.md`
  - `agents/code-reviewer.md`
  - `src/cli/agents.rs` (clone of `src/cli/skills.rs`, with `AGENTS_DIR_GLOBAL = ~/.claude/agents/`, `AGENTS_DIR_LOCAL = ./.claude/agents/`)
- **Files to modify:**
  - `src/cli/mod.rs` — add `pub mod agents;`
  - `src/cli/dynamic.rs` — register `agents` subcommand parallel to `skills` in `build_root`
  - `src/main.rs` — dispatch `agents` matches to `cli::agents::run` (parallel to `skills`)
- **Acceptance Criteria:**
  - [ ] AC1.1: `stores agents list` prints exactly 4 entries (`planner`, `plan-reviewer`, `executor`, `code-reviewer`) with installed/uninstalled annotations matching the `skills list` format.
  - [ ] AC1.2: `stores agents install --all` writes 4 files to `.claude/agents/<name>.md` and re-running is idempotent (same content → no error).
  - [ ] AC1.3: `stores agents install <name>` writes that single file; `--global` writes to `~/.claude/agents/` instead.
  - [ ] AC1.4: `stores agents uninstall <name>` removes the file; uninstalling non-existent is non-fatal.
  - [ ] AC1.5: Conflict detection — if a file exists at the destination with different content, the installer errors with the same message format used by skills (`exists with different content; remove or use --force`).
  - [ ] AC1.6: Each agent prompt YAML frontmatter declares `name`, `description`, `effort` (mirroring task-workflow plugin shape) so Claude Code's Task tool can register them.
  - [ ] AC1.7: Each agent prompt body specifies the CLI-native protocol: (a) read your brief from stdin/argv, (b) do the work, (c) submit via the named `stores tasks submit-*` verb, (d) print a structured one-line success/failure summary. The prompt does NOT rely on a Task tool, plugin, or harness-specific construct.
  - [ ] AC1.8: `cargo build` succeeds with the new `include_str!` paths; `cargo test cli::agents` covers fresh-install / idempotent-reinstall / conflict / uninstall (mirroring the existing `cli::skills::tests`).
  - [ ] AC1.9: `cli/agents.rs` is a near-mechanical clone of `cli/skills.rs`. Differences limited to: registry contents, target directory (`agents/` vs `skills/`), file extension (`.md` direct, no `SKILL.md` subdirectory).

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
- **Dependencies:** Phase 1 (agent prompts), Phase 2 (runner trait).
- **Files to create:**
  - `src/handlers/drive.rs` — main loop + `--auto` selection + safety rails (max-iters, cycle detection)
- **Files to modify:**
  - `src/handlers/mod.rs` — register module
  - `src/cli/dynamic.rs` — register `drive` subcommand on workflow-shaped stores (parallel to `next-action`/`brief`)
  - `src/cli/dispatch.rs` — route `drive` to handler
  - `src/handlers/brief.rs` — expose `compute(...)` publicly so `drive` can call it without re-shelling
  - `src/handlers/render.rs`, `src/handlers/submit.rs` — same expose-`compute` treatment if not already public
- **Drive loop (pseudocode):**
  1. Resolve target id: explicit positional arg, else `--auto` selects by `(priority DESC, created_at ASC)` filtered to non-terminal (`status NOT IN ('complete','blocked')`); on tie pick first; bail with explicit error when no candidates.
  2. Loop:
     a. Compute `next_action`. If terminal (`complete`/`blocked`), exit with appropriate exit code.
     b. Compute `brief` for the next agent.
     c. Read agent system prompt from the bundled `agents/<role>.md` (NOT from disk — `include_str!` via `BUNDLED_AGENTS`).
     d. `runner.spawn(role, system_prompt, brief)`.
     e. Parse runner output → invoke the appropriate `submit-*` handler in-process.
     f. Render. Emit one-line stderr progress (`[T001] phase 2 cycle 1: executor → submitted`).
     g. Increment iter counter; bail if `--max-iters` hit (default 50).
- **Acceptance Criteria:**
  - [ ] AC3.1: `stores tasks drive <id> --mock` (mock runner with a pre-loaded queue) drives a fixture task from `planning` to `complete` in a single invocation; final `next-action` reports `status=complete`.
  - [ ] AC3.2: `stores tasks drive --auto --mock` selects the highest-priority non-complete task; with priorities tied, picks the oldest by `created_at`; with no candidates, errors with a clear message ("no non-complete tasks available").
  - [ ] AC3.3: `--mock` is a hidden test-only flag (or always-available; document the choice). `--claude-code` requires the cargo feature; when missing, prints a remediation message ("rebuild with `cargo install --features runner-claude-code`").
  - [ ] AC3.4: Progress lines go to stderr (one per iteration); no progress noise on stdout. Stdout reserved for any structured output (`--json` aware).
  - [ ] AC3.5: `--max-iters N` (default 50) bounds the loop; on hit, exits non-zero with a clear "max iterations exceeded" message and current state summary.
  - [ ] AC3.6: When the runner errors mid-loop, drive surfaces the runner's stderr verbatim and exits non-zero — does NOT corrupt task state (no `submit-*` is called for that iteration).
  - [ ] AC3.7: `cargo test handlers::drive` covers: happy path through 1 phase (mock); auto-selection ordering; max-iters bound; runner-error abort; terminal-state early exit.
  - [ ] AC3.8: Drive composes existing handlers via in-process function calls — does NOT shell out to itself.
  - [ ] AC3.9: When the next-action result is a `blocked` status, drive exits 0 (not an error — block surfaced cleanly) and prints a one-line "blocked: <reason>; run `stores gate <id> guide` for help" hint.

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
  - [ ] AC4.1: `stores setup` in a fresh directory creates `.stores/db.sqlite`, `.stores/manifest.yaml`, installs all 3 bundled stores (`observations`, `gate`, `tasks`), installs all 5 bundled skills under `./.claude/skills/`, installs all 4 bundled agents under `./.claude/agents/`.
  - [ ] AC4.2: Re-running `stores setup` is idempotent — exits 0, prints idempotency notes per layer ("Already initialized" / "Already installed: X").
  - [ ] AC4.3: `stores setup --global` writes skills+agents to `~/.claude/` instead of local; the store DB still goes to `./.stores/`.
  - [ ] AC4.4: Partial-state recovery: if `.stores/` exists but agents are missing, re-running `setup` only adds the missing layer (does not error or wipe).
  - [ ] AC4.5: `cargo test cli::setup` covers fresh-bootstrap and idempotent-rerun in a tempdir.
  - [ ] AC4.6: A failure in any layer (e.g. one bundled store install errors) aborts subsequent layers and surfaces the underlying error — no half-installed state is silently left behind.

#### Phase 5: `stores tasks status --follow`

- **Objective:** Polled text-frame observability. No TUI.
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
  - [ ] AC5.1: `stores tasks status <id>` (without `--follow`) prints a single frame and exits 0.
  - [ ] AC5.2: `stores tasks status --follow <id>` re-prints a frame every interval; exits 0 on `complete` or `blocked`.
  - [ ] AC5.3: `stores tasks status --follow` (no id) prints a multi-task table frame across all non-terminal tasks; exits when none remain or Ctrl-C.
  - [ ] AC5.4: Ctrl-C is caught cleanly — last frame on screen, exit code 130.
  - [ ] AC5.5: Frames suppress duplicate consecutive lines (same state → no spam); on state change, prints immediately.
  - [ ] AC5.6: `cargo test handlers::status` covers single-frame mode + change detection (fixture row mutated mid-loop). Follow-loop tests are bounded by `--max-iters` test-only flag to avoid flakiness.

#### Phase 6: Guide handlers — `gate <id> guide` (full) + `tasks <id> guide` (stub)

- **Objective:** Human-boundary affordance. When the user faces a blocked task or gate, `guide` curates the relevant rows + spawns a guide agent that can read/write back via the same CLI verbs.
- **Dependencies:** Phase 1 (need a guide agent prompt) and Phase 2 (runner trait).
- **Files to create:**
  - `agents/guide.md` — guide agent system prompt (read-mostly, can call `stores ... show`, `stores gate answer`, `stores tasks show`).
  - `src/handlers/guide.rs` — context-bundle builder + runner spawn
- **Files to modify:**
  - `src/cli/agents.rs` — add `guide` to `BUNDLED_AGENTS` (so it ships in `setup`)
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
  - [ ] AC6.1: `stores gate <id> guide --mock` builds a context bundle (verifiable via mock runner capturing the prompt) that includes the gate row, the linked task row (if any), and the list of authorized CLI verbs.
  - [ ] AC6.2: `stores tasks <id> guide --mock` builds a context bundle that includes the task row, last `next-action`, and last review.
  - [ ] AC6.3: `cargo test handlers::guide` covers both bundle shapes with fixture rows.
  - [ ] AC6.4: The guide agent prompt explicitly forbids editing main.md directly and instructs writes via `stores gate answer` / `stores tasks <verb>`.
  - [ ] AC6.5: `gate guide` exits 0 when the gate is `answered` (target reached); exits 1 if the runner errors; exits 2 if the user escapes (signal — best-effort capture).
  - [ ] AC6.6: `tasks guide` is documented (in the agent prompt + README) as v0.3 stub-quality; expected expansion in v0.4.

#### Phase 7: Skill rewrite + version bump + README + drive e2e

- **Objective:** Final wire-up. Tighten the user-facing surface, prove the DONE_WHEN with a mock-runner-driven e2e, and ship 0.3.0.
- **Dependencies:** Phases 1-6.
- **Files to create:**
  - `tests/drive_e2e.sh` — mock-runner-driven full-loop test (mirrors `tests/tasks_e2e.sh` shape, but uses `stores tasks drive --mock` instead of manual `submit-*` calls)
- **Files to modify:**
  - `skills/tasks:start/SKILL.md` — rewrite as a one-line wrapper: instructs the harness to invoke `stores tasks drive --auto --claude-code` (preserves the `/tasks:start` invocation surface; body shrinks ~95%).
  - `Cargo.toml` — version `0.2.0` → `0.3.0`.
  - `README.md` — replace the "13-step demo walk" intro with a `stores setup` quickstart at the top; add new sections for `drive`, `status --follow`, `gate guide`, `tasks guide`, and the runner feature flag.
  - `src/cli/skills.rs` — bump `BUNDLED_SKILLS` re-export count assertion (`all_skills_bundled` test) if needed.
- **Acceptance Criteria:**
  - [ ] AC7.1: `tests/drive_e2e.sh` runs end-to-end against a fresh tempdir: `stores setup` → seed task row → `stores tasks drive --mock` (with a queued mock runner script) → final `stores tasks show` reports `status=complete`. All 16 step-equivalents from `tasks_e2e.sh` validated through this single drive call.
  - [ ] AC7.2: `Cargo.toml` version is `0.3.0`; `cargo build` produces a `stores --version` of `0.3.0`.
  - [ ] AC7.3: New `tasks:start` body is ≤ 30 lines; runs the harness equivalent of `stores tasks drive --auto --claude-code` with no in-skill orchestration logic.
  - [ ] AC7.4: README quickstart starts with `cargo install --path . && stores setup && stores tasks drive --auto --claude-code`. The 13-step legacy walk moves to a "Manual workflow walk-through" subsection.
  - [ ] AC7.5: README documents the cargo feature flag (`--features runner-claude-code`) and lists the available runners (`mock`, `claude-code`).
  - [ ] AC7.6: `cargo test --all` passes; `cargo test --features runner-claude-code` also passes; `bash tests/tasks_e2e.sh` still passes (regression); `bash tests/drive_e2e.sh` passes.
  - [ ] AC7.7: A final manual smoke (run-and-screenshot in the completion summary) confirms `stores setup && stores tasks drive --auto --claude-code` against a fresh test repo with a single seeded task drives to `complete` using a real `claude -p` runner. (This step is the DONE_WHEN proof; it gates the merge.)
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
| `--auto` task selection | (a) priority+oldest; (b) FIFO; (c) explicit queue | (a) | Locked. Simplest sensible policy; queues land in v0.4 if needed. |
| Agent prompt format | (a) `agents/<name>.md` flat; (b) `agents/<name>/AGENT.md` mirror of skills | (a) | Claude Code's plugin convention is flat `.md` files in `agents/`; mirroring skills' subdirectory adds noise without payoff. |
| `Runner` trait method | (a) `spawn(role, sys, brief) -> Result<Output>`; (b) async trait; (c) channel-based streaming | (a) | v0.3 deliberate minimalism; sync, single-shot, easy to mock. Streaming + async land at second-runner moment. |
| Mock runner availability | (a) always built; (b) cfg(test)-only; (c) feature-gated | (a) | Always built. `tests/drive_e2e.sh` is a shell script — needs the mock runner accessible from a release binary. |
| Drive loop composition | (a) in-process function calls; (b) shell out to self; (c) mixed | (a) | In-process is testable, atomic, and avoids fork overhead. The CLI verbs are thin wrappers around handler::run anyway. |
| `--max-iters` default | (a) 50; (b) 100; (c) unbounded | (a) | A 3-phase task with full revise budget hits ~12 iters; 50 is generous safety but bounded. |
| `drive` exit code on `blocked` | (a) 0 with hint; (b) non-zero error | (a) | A real `blocked` is a successful drive outcome — surfacing the block to the human is the deliverable. Reserve non-zero for runner failures and bugs. |
| `guide` write-access verbs | (a) only `gate answer`; (b) full task verbs; (c) read-only | (a) for gate; restrictive list documented in prompt | Smallest blast radius. v0.4 expands as the trust model develops. |
| README quickstart vs 13-step walk | (a) replace; (b) keep both with quickstart on top; (c) delete walk | (b) | Quickstart is the headline; the walk remains valuable for users debugging the framework internals. |

### Plan Notes (open items flagged for plan-reviewer / user)

These do not block execution; sensible defaults are documented above. Flagged for visibility:

1. **`tasks` store schema lacks a `priority` column.** The Intent Contract's locked decision #8 is "highest-priority then oldest." The current schema has no priority field. **Default chosen:** in v0.3, `--auto` falls back to "oldest non-complete" (purely `created_at ASC`). Adding a `priority` column requires a schema migration and is out of scope for T003. Plan-reviewer/user: please confirm the `created_at`-only fallback is acceptable, or scope-in a `priority` column addition (would add ~Low complexity to phase 3).

2. **Runner-stub for tests in a release binary.** AC3.3 mentions `--mock` as a flag exposed on the release binary so `tests/drive_e2e.sh` can invoke it. Alternative: gate `--mock` behind a `runner-mock` Cargo feature that is `default = ["runner-mock"]` (always on unless explicitly disabled). **Default chosen:** always available, undocumented in user-facing help, documented in `--help` only via a hidden flag. Plan-reviewer: confirm this is acceptable, or push for a feature gate.

3. **`agents/<name>.md` flat vs nested layout.** Mirrors Claude Code plugin convention but diverges from `skills/<name>/SKILL.md`. `BUNDLED_AGENTS` registry would carry `(&str, &str)` tuples mapping name → markdown content, parallel to `BUNDLED_SKILLS`. Plan-reviewer: confirm the flat layout — alternative would be `agents/<name>/AGENT.md` for symmetry with skills, at the cost of fighting Claude Code's convention.

4. **Guide agent's authorized-verbs surface.** AC6.4 says the prompt forbids direct main.md edits. The system prompt should also enumerate the exact CLI verbs the guide may invoke. **Default chosen:** `stores gate show`, `stores gate answer`, `stores tasks show`, `stores tasks list`, `stores tasks next-action`. Plan-reviewer: confirm this list, or scope adjustments.

5. **Phase ordering for `setup`.** Phase 4 builds `setup` after the agents/runner phases. Alternative: build a thin `setup` early (phase 2) so subsequent phases can dogfood it. **Default chosen:** keep the listed order — `setup` is a thin composer; building it last avoids re-touching it as new pieces land.

---

## Plan Review
_Plan-reviewer agent fills this section._

- **Gate:** READY | NEEDS_WORK | NOT_READY
- **Open Questions Finalized:** —
- **Issues Found:** —

> Details: plan-review.md

---

## Execution Log
_Executor agent fills this section per phase._

### Phase 1: [Title]
- **Status:** PENDING | IN_PROGRESS | COMPLETE | BLOCKED
- **Started:** —
- **Completed:** —
- **Commits:** —
- **Files Modified:** —
- **Notes:** —

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

### Phase 1
- **Gate:** PASS | REVISE | FAIL
- **Issues Found:** —
- **Revision Count:** 0/3

> Details: code-review-phase-1.md

---

## Completion
_Final summary when task is complete._

- **Completed:** [DATE]
- **Summary:** ...
- **Commits:** ...
- **Lessons Learned:** ...
