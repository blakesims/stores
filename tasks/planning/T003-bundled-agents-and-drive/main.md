# T003: Framework-bundled workflow agents + runtime-agnostic orchestrator

## Meta
- **Status:** PLANNING
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
_Planner agent fills this section._

### Objective
_What we're trying to achieve._

### Scope
- **In Scope:** ...
- **Out of Scope:** ...

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | ... | Low/Medium/High |

### Phase Details

#### Phase 1: [Title]
- **Objective:** ...
- **Files to modify:** ...
- **Acceptance Criteria:**
  - [ ] ...

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| ... | ... | ... | ... |

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
