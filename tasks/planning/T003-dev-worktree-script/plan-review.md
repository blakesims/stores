# T003 Plan Review

## Gate: NEEDS_WORK

The plan is structurally sound and the phase decomposition is correct, but four issues block READY: (1) the e2e test plan is incoherent because it tries to mix the `--mock` runner with full live state-machine traversal in a way the current architecture can't deliver without a real `submit-*` driver loop in the mock; (2) the agent prompt frontmatter spec is wrong for Claude Code subagents; (3) AC7.7 mandates a real `claude -p` smoke as a merge gate but T003 lists CI-only mock testing as in-scope and bans real `claude` invocations, contradicting the DONE_WHEN proof; and (4) the auto-selection AC says "highest priority then oldest" while the plan's own Plan Notes #1 acknowledges no `priority` column exists — AC3.2 is therefore literally untestable as written. All four are bounded and the planner can fix them in another pass.

---

## Critical issues (blockers — must fix before READY)

### C1. AC3.2 contradicts the schema (no `priority` column)

AC3.2 reads: "selects the highest-priority non-complete task; with priorities tied, picks the oldest by `created_at`." The `stores/tasks/schema.yaml` has no `priority` field (verified — fields are `title`, `slug`, `branch`, `capability`, `sub_item`, `infra`, `depends_on`, `linked_observations`, `contract`, `plan`, `plan_review_log`, `cycles`, `current_phase`, `current_cycle`, `claimed_by`, `claimed_at`, `blocked_reason`). Plan Notes #1 acknowledges this but Decision Matrix row "`--auto` task selection" still says "priority+oldest" as the locked choice. AC3.2 cannot pass as stated.

**Fix:** Either (a) collapse to `created_at ASC` only and rewrite AC3.2 + Decision Matrix accordingly, or (b) scope-in a schema migration adding `priority` (would require a schema-version bump, migration story, and revisiting v0.2 → v0.3 compatibility — not Low complexity, contrary to Plan Notes #1).

**Adjudicated default (sensible):** Take (a). v0.3 sorts by `created_at ASC` only. Defer `priority` to v0.4 where queueing/fairness gets real treatment. Update AC3.2, the Decision Matrix row, and remove Plan Notes #1.

### C2. AC1.6 prescribes wrong agent-prompt frontmatter

AC1.6 says each agent prompt's YAML frontmatter declares `name`, `description`, `effort`. The current `task-workflow` plugin uses `effort: medium`, but Claude Code's first-party subagent spec (the form the orchestrator will spawn via `claude -p` and via the Task tool) uses `name`, `description`, optionally `tools` (whitelist) and `model`. There is no `effort` field. If the bundled agents land at `.claude/agents/<name>.md` with `effort: medium`, Claude Code will not register them as subagents and the runner will not be able to spawn them by role.

**Fix:** Replace `effort` with the actual Claude Code subagent fields. Each agent should declare:
```yaml
name: <role>
description: <one-line trigger description>
tools: <optional whitelist of tools the agent may call>
```
Also: the runner does not invoke agents via Claude Code's subagent registry — it uses `claude -p '<system-prompt-text>' --append-system-prompt` (or similar) and feeds the prompt directly. That means the frontmatter is only meaningful for the parallel "user types `/tasks:start`" path, where Claude Code's Task tool reads the registry. Both paths matter; the frontmatter must satisfy whichever the orchestrator actually exercises in `drive --auto --claude-code`.

**Adjudicated default:** Use `name` + `description` (drop `effort`). Add a Decision Matrix row clarifying which spawn path Phase 3's `claude_code` runner actually uses (CLI arg vs subagent registry). The runner contract for v0.3 should be: pass the prompt body via `claude -p` stdin or `--append-system-prompt`, NOT rely on the agent being pre-registered. The frontmatter is for the Task-tool path (`/tasks:start` invocation).

### C3. AC7.1 + AC7.7 contradict each other and the in-scope statement

The Intent Contract says: "runner trait should be mockable... CI uses the mock runner, never real `claude`." Risks section says: "CI uses the mock runner, never real `claude`."

AC7.1 says `drive_e2e.sh` runs against the mock runner — fine.

AC7.7 says: "A final manual smoke (run-and-screenshot in the completion summary) confirms `stores setup && stores tasks drive --auto --claude-code` against a fresh test repo with a single seeded task drives to `complete` using a real `claude -p` runner. (This step is the DONE_WHEN proof; it gates the merge.)"

That's a manual gate that requires a working `claude` CLI on the executor's machine, with credentials, executing real model calls. The DONE_WHEN clause "drives it through the full state machine to `complete`" implicitly requires this proof — but making it a hard merge gate inside Phase 7 means the executor cannot complete the task without a successful real-`claude` run, and the bundled agents must produce structured-enough output for the parser. That's a significant, brittle dependency masquerading as a checkbox.

**Fix:** Either (a) explicitly document AC7.7 as a manual smoke that the executor records evidence of (screenshot/transcript) and call it a soft gate — separate from the automated test pass; or (b) make AC7.7 the orchestrator/human's responsibility (not the executor's) and remove it from Phase 7's exit criteria; or (c) accept the brittle dependency and add a Phase 7 sub-AC for "executor can run `claude --version` and has CLAUDE_API_KEY set; otherwise BLOCKED."

**Adjudicated default:** (a). AC7.7 stays as a manual smoke recorded in the completion summary, but it is not a hard gate inside the executor's PASS/REVISE/FAIL cycle. The executor demonstrates DONE_WHEN coverage via the mock-runner e2e plus the inspected agent prompts; the real-claude smoke is captured as evidence in the completion summary and counts as a v0.3 acceptance gate at the merge level. Document this distinction explicitly.

### C4. AC7.1 overstates what `drive_e2e.sh` proves

AC7.1 claims: "All 16 step-equivalents from `tasks_e2e.sh` validated through this single drive call." `tasks_e2e.sh` step 16 is `cargo test ac5_11b` (atomicity unit tests inside Rust); step 14 is render idempotency (file-byte comparison); step 15 is direct SQLite final-state assertion. These are not "drive call" outputs — they're orthogonal harness checks. Drive composing the workflow does not validate atomicity, render byte-identity, or SQLite invariants; those still need their own assertions.

**Fix:** Rewrite AC7.1 to claim only what drive actually proves: the state-machine transitions through every workflow status (`planning → plan_review → ready → executing → code_review → executing → code_review → complete`) using mock-runner outputs that satisfy each `submit-*` schema. Keep `tasks_e2e.sh` running unchanged as a regression (already covered by AC7.6). Drop the "16 step-equivalents" claim.

**Adjudicated default:** AC7.1 reads: "drives a fixture task with N=2 phases and zero REVISE cycles from `planning` to `complete`; final `stores tasks show` reports `status=complete`, `current_phase=2`, both phases have one cycle with PASS gates." Add a separate AC7.1b for "drives a fixture with one REVISE cycle" if blast-radius coverage is wanted.

---

## Major issues (should fix; planner can defer with rationale)

### M1. "Expose `compute(...)` publicly" misreads the codebase

Phase 3's "Files to modify" says: "`src/handlers/brief.rs` — expose `compute(...)` publicly so `drive` can call it without re-shelling. `src/handlers/render.rs`, `src/handlers/submit.rs` — same expose-`compute` treatment if not already public." Inspection shows:
- `brief.rs` already exposes `compute` as `pub(crate)`.
- `submit.rs` already exposes `compute_submit_plan`, `compute_submit_plan_review`, `compute_submit_execute`, `compute_submit_review`, `compute_resume` all as `pub(crate)`.
- `render.rs` exposes `pub fn compute_render` and `pub fn run_render`.

`drive` will be `src/handlers/drive.rs` — same crate. `pub(crate)` is already sufficient. No visibility changes required. The plan's instruction to "expose publicly" is a no-op or worse (widens API surface unnecessarily).

**Fix:** Strike the "expose `compute(...)` publicly" item from Phase 3's Files to modify. Phase 3 calls into the existing `pub(crate) fn compute_*` directly.

### M2. Phase 3 has no story for the agent-output → submit-verb mapping

Phase 3 step 2(e) says: "Parse runner output → invoke the appropriate `submit-*` handler in-process." How? The runner returns a `RunnerOutput { stdout, stderr, exit_code, final_message: Option<String> }`. The handler needs to (a) decide which `submit-*` verb to call based on which agent ran, and (b) extract the structured arguments — the planner's plan JSON, the plan-reviewer's gate + summary + open_questions, the executor's commit + files_changed + summary, the code-reviewer's gate + counts + summary + details.

Today, those arguments are passed as CLI flags (e.g. `submit-plan --plan-json @file`, `submit-plan-review --gate READY --summary '...' --open-questions @file`). The agent prompt would have to emit a parseable structured tail, and `drive` would need a parser per role. This is real design work, not a one-liner.

**Fix:** Add a Phase 3 sub-AC explicitly addressing the contract: "each agent's final message is a single JSON object on the last line of stdout, schema-keyed by role; drive parses that JSON and calls the matching `compute_submit_*`." And: "the agent prompts (Phase 1) must produce that JSON." This couples Phase 1 and Phase 3 in a way that's not currently captured — Phase 1's AC1.7 says "print a structured one-line success/failure summary" but doesn't constrain the schema. Tighten AC1.7 to specify the role-keyed JSON envelope, with a fixture in the repo (`tests/fixtures/agent_outputs/<role>.json`).

### M3. Phase 1 ACs are silent on what the prompts must contain

AC1.7 says the prompt body specifies the CLI-native protocol but doesn't constrain the *content* of the prompts. Authoring 4 system prompts is the long pole (Risks bullet 2). The current ACs would let the executor ship 4 stub markdown files containing only "you are a planner; do plans" and pass Phase 1.

**Fix:** Add ACs requiring (a) each prompt names the schema verbs it submits to and the JSON shape it outputs; (b) each prompt has a section addressing failure modes (open questions, blocked, what to do when context is insufficient); (c) the planner prompt mirrors the Stage-1-7 structure of the current `task-workflow` plugin's planner (without copying verbatim — license/drift); (d) total prompt length bounded (e.g. 400-1200 lines per prompt) so they're substantive.

### M4. `setup` ordering — phase 4 is fine, but missing a sub-AC

Plan Notes #5 considers building `setup` early to dogfood. Reasonable to keep Phase 4 where it is. But: Phase 1, 2, 3 all need a way to install the agents into `.claude/agents/` for end-to-end testing. Phase 1 ships `stores agents install --all`, so manual install works. Just add a note in Phase 3's text confirming "tests use `agents install --all` directly; `setup` arrives in Phase 4 as the user-facing convenience."

**Fix:** Add one line to Phase 3 ACs confirming the manual install path is used until `setup` lands in Phase 4. Cosmetic but reduces ambiguity for the executor.

### M5. AC6.5's exit-code-2-on-escape is unimplementable as stated

AC6.5: "exits 2 if the user escapes (signal — best-effort capture)." Ctrl-C in a child runner process kills the runner, which returns a non-zero exit code through `RunnerOutput.exit_code`. The drive parent process then has to distinguish "runner crashed" from "user pressed Ctrl-C inside the agent session" — generally not possible without specific signal handling on the parent + cooperation from the runner. The "best-effort capture" hedge admits this.

**Fix:** Drop the exit-code-2 distinction. Use exit code 1 for any guide failure (runner error or user escape — both are non-success). If the gate row's status changes from `pending` to `answered`, exit 0. Otherwise exit 1. Simpler, testable, no magic about signals.

### M6. `--mock` flag exposure conflict

Plan Notes #2 leans toward "always available, undocumented." Decision Matrix row "Mock runner availability" says "always built." AC3.3 says "`--mock` is a hidden test-only flag (or always-available; document the choice)." That's two of three reviewer questions self-resolved but contradicting the third. The phrase "hidden test-only flag" implies clap's `.hide(true)`, which is fine — it doesn't gate availability, just visibility.

**Fix:** AC3.3 should read: "`--mock` is always built, hidden from `--help` (clap `.hide(true)`), accepts a path to a queued-response fixture file. `--claude-code` requires the cargo feature." Resolves the contradiction.

### M7. Phase 6's `guide` agent isn't accounted for in Phase 1

Phase 1 authors 4 agents (`planner`, `plan-reviewer`, `executor`, `code-reviewer`). Phase 6 introduces a 5th agent (`guide`) and adds it to `BUNDLED_AGENTS` in Phase 6's "Files to modify." That works, but it means AC1.1 ("`stores agents list` prints exactly 4 entries") becomes wrong after Phase 6 lands (becomes 5). And the all_agents_bundled test (parallel to `all_skills_bundled`) needs to assert 5.

**Fix:** Add a one-line note in Phase 6: "extend the BUNDLED_AGENTS count test to 5; update AC1.1 historically (Phase 1 ships 4; Phase 6 ships the 5th)." Or move guide-agent authoring into Phase 1 and gate the *handler* in Phase 6. The latter is cleaner because Phase 1 already has authoring infrastructure and reviewers.

**Adjudicated default:** Author the guide prompt in Phase 1 (5 agents total), build the handlers in Phase 6. Update AC1.1 to "5 entries" and add `guide` to the Phase 1 file list.

---

## Minor issues (informational; document or punt)

### m1. AC7.3's "≤ 30 lines" budget for `tasks:start` skill

Current `tasks:start/SKILL.md` is ~280 lines. A 30-line wrapper is aggressive but reasonable for "invoke `stores tasks drive --auto --claude-code` and exit." Confirm with a quick sketch in the plan.

### m2. AC5.1/5.2/5.3's `status` flag conflict

`stores tasks status <id>` (without `--follow`) is a one-shot frame; `stores tasks status --follow <id>` re-prints. But `stores tasks show <id>` already prints task state. Make sure the status command's noun choice is intentional — "status" vs "show" — and document the difference (status = workflow telemetry frame; show = full row JSON).

### m3. Decision Matrix has no row for "agent output JSON envelope"

If M2 above is accepted, add a row to the Decision Matrix: "Agent output protocol: (a) trailing JSON object on stdout last line; (b) JSON-only stdout; (c) sentinel-delimited blocks. Choice: (a) for v0.3 (works with both real and mock runners; tolerant of agent commentary)."

### m4. README quickstart should call out the cargo feature

AC7.4 lists the quickstart as `cargo install --path . && stores setup && stores tasks drive --auto --claude-code`. That `cargo install` does NOT include the `runner-claude-code` feature by default. The user would get a remediation message at runtime. Make the quickstart the working form: `cargo install --path . --features runner-claude-code`.

### m5. `drive` should warn (not error) when the task is already claimed

The schema has `claimed_by` / `claimed_at` lock columns. Phase 3's pseudocode doesn't address claim acquisition. Submit handlers already implement claim logic. But `drive --auto` should probably skip claimed tasks (find next candidate) rather than error. Document the policy.

### m6. AC3.6 ("does NOT corrupt task state on runner error") is hard to test

To test this, the test needs a fixture where the runner errors mid-loop AND the task row is asserted unchanged. Add a sub-AC pointing to the test fixture name (`tests/handlers/drive_runner_error.rs` or similar).

---

## Adjudicated open questions (Q1–Q5)

### Q1. `tasks` schema has no `priority` column

Confirmed: `stores/tasks/schema.yaml` has no `priority` field. Adding one is not a Low-complexity addition — it requires a schema-version bump, migration path for existing v0.2 DBs, CLI surface for setting priority, and a doc story. **Adjudication: drop `priority` from v0.3 entirely. `--auto` selects by `created_at ASC` filtered to `status NOT IN ('complete', 'blocked')`.** Plan Notes #1 should be removed; Decision Matrix row corrected; AC3.2 rewritten. This is **Critical issue C1**.

### Q2. `--mock` flag exposed on release binary (hidden) vs feature-gated

`tests/drive_e2e.sh` is a shell script that needs the mock runner accessible from a release-mode binary. Feature-gating `--mock` would require CI to build twice (once for the e2e, once for the release artifact). The hidden-flag-on-release-binary approach has a smaller blast radius (it's not advertised; it's not a stable API; it consumes a fixture file path). **Adjudication: always built, hidden from `--help` via clap `.hide(true)`, takes `--mock <fixture-path>` argument. The mock runner reads queued responses from the file.** This matches Decision Matrix's "Mock runner availability: always built" — make AC3.3 consistent (Major issue M6).

### Q3. Flat `agents/<name>.md` vs nested `agents/<name>/AGENT.md`

The codebase precedent (`cli/skills.rs`) uses nested `<base>/<name>/SKILL.md`. The Claude Code subagent spec (the platform target) uses flat `<base>/<name>.md`. For agents, the platform convention wins because Claude Code's subagent loader scans flat — it would not find `<name>/AGENT.md`. **Adjudication: flat `agents/<name>.md`, BUT note in `cli/agents.rs` doc-comment that this asymmetry with `cli/skills.rs` is intentional and platform-driven.** Phase 1's AC1.9 already reflects this; just keep it explicit in the Decision Matrix (already there).

### Q4. Explicit list of CLI verbs the guide agent may invoke

The guide is read-mostly with one write verb (`gate answer`). Plan's default of `stores gate show`, `stores gate answer`, `stores tasks show`, `stores tasks list`, `stores tasks next-action` is correct. **Adjudication: that exact list goes into the guide agent's system prompt as an "Authorized CLI verbs" section, with all other `stores` verbs explicitly forbidden.** Add a Phase 6 sub-AC that the prompt enumerates this list verbatim. The planner should produce it (not the executor) because it's a security-adjacent decision.

### Q5. `setup` ordering (phase 4 vs earlier for dogfooding)

Building `setup` after agents+runner+drive is ergonomic — `setup` is a thin composer; earlier-positioning would force re-touching it as new pieces land (skills install, agents install, drive registration). **Adjudication: keep Phase 4. Add a one-line note in Phases 1, 2, 3 confirming that until Phase 4 lands, tests use `stores agents install --all` directly.** No real change needed.

---

## DONE_WHEN trace

DONE_WHEN: "In a fresh repo with stores installed (no `task-workflow` plugin, no Claude Code skill manually wired), `stores setup && stores tasks drive --auto --claude-code` picks the highest-priority non-complete task, drives it through the full state machine to `complete` (or surfaces a real `blocked` with `stores gate <id> guide` available to the human), and `stores tasks status --follow <id>` shows live progress throughout."

| Clause | Phase(s) | Status |
|---|---|---|
| Fresh repo with stores installed | Phase 7 (e2e creates tempdir + `cargo install`) | Covered (assuming AC7.1 fixed per C4) |
| No `task-workflow` plugin | Phase 1 (bundled agents replace plugin) | Covered |
| No Claude Code skill manually wired | Phase 4 (`setup` installs skills + agents) | Covered |
| `stores setup` works | Phase 4 (AC4.1–4.6) | Covered |
| `stores tasks drive --auto` selects a task | Phase 3 (AC3.2) | **Blocker C1** — selection criterion conflicts with schema |
| `--claude-code` runner | Phase 2 (AC2.1, AC2.4) + Phase 3 (AC3.3) | **Blocker C2** — frontmatter spec wrong; runner spawn-path unclear |
| Drives full state machine to `complete` | Phase 3 (AC3.1) + Phase 7 (AC7.1, AC7.7) | **Blocker C3+C4** — AC7.1 overstates; AC7.7 contradicts in-scope |
| Surfaces real `blocked` cleanly | Phase 3 (AC3.9) | Covered |
| `stores gate <id> guide` available | Phase 6 (AC6.1, AC6.4–6.5) | Mostly covered (M5: exit-code-2 unimplementable) |
| `stores tasks status --follow <id>` shows live progress | Phase 5 (AC5.1–5.6) | Covered |

Net: 4 of 10 clauses are blocked by Critical issues. Once C1–C4 are fixed, the trace is solid.

---

## Phase ordering & dependencies — assessment

Reviewing the 7-phase cut against the dependency graph:

- **Phase 1 (agents registry)** — independently shippable. No deps. Cargo builds and tests pass with just the registry + 4 markdown files. ✓
- **Phase 2 (runner trait)** — depends on Phase 1 only because Phase 2's docstring references the agent role names. Could ship independently if the doc-comment is generic. ✓
- **Phase 3 (drive)** — depends on Phases 1 + 2. The big phase. ✓
- **Phase 4 (setup)** — depends on Phase 1 (agents install). Could land between any of {Phase 1, Phase 5, Phase 6, Phase 7}; planner's choice (after Phase 3) is fine. ✓
- **Phase 5 (status --follow)** — independently shippable; no hard deps. Could ship alongside Phase 3 to give immediate observability. ✓
- **Phase 6 (guide)** — depends on Phases 1 + 2; needs the 5th agent (`guide`) which Phase 1 should also ship (M7). ⚠ adjust Phase 1 to author the guide prompt.
- **Phase 7 (skill rewrite + version + README + e2e)** — depends on all prior phases. ✓

The cut is right. Each phase leaves the framework working and tested. No ordering swaps required. Adjustments per C1–C4 and M7 are within-phase fixes.

---

## Out-of-scope creep — none detected

Cross-checked Phase Details against the Intent Contract's "Out of scope" list:
- ✓ No `runs` event log work
- ✓ No phase-reviewer / merge-reviewer agents (only the 4 + guide)
- ✓ No `tasks:wrap` skill
- ✓ No second runner
- ✓ No HTTP/JSON API or TUI
- ✓ T001/T002 not migrated
- ✓ `tasks <id> guide` is stub-quality (Phase 6 explicitly stub)

Plan stays inside scope.

---

## Risks coverage check

The Intent Contract listed 5 risks. Coverage:

1. **`claude -p` output format stability** — Phase 2 AC2.4 covers (fixture shim, defensive parsing). ✓ but tightening per M2 (JSON envelope) makes this concrete.
2. **Authoring 4 system prompts is the long pole** — Phase 1 authors them but ACs are silent on content quality (M3). ⚠
3. **Runner trait shape may evolve** — Phase 2 doc-comments cover (AC2.5). ✓
4. **`stores setup` writing to `~/.claude/`** — Phase 4 AC4.3 covers (default local, explicit `--global`). ✓
5. **`drive --auto` task selection policy** — Phase 3 AC3.2 attempts but conflicts with schema (C1). ✗ blocker.

3 of 5 risks well-covered, 2 need work (M3 + C1).

---

# Cycle 2 Review

## Gate: READY

The planner applied all 4 criticals and all 7 majors verbatim per the cycle-1 adjudications. 5 of 6 minors landed; one drifted (cosmetic — see below). No new criticals were introduced during the revision pass. The DONE_WHEN trace is solid: every previously-blocked clause now has a concrete, schema-honest AC. The plan is ready for the executor.

## Cycle-1 finding landing report

| ID | Adjudicated fix | Landed |
|---|---|---|
| C1 | drop priority; AC3.2 FIFO `created_at ASC`; remove Plan Notes #1; Decision Matrix row corrected | **CLEAN** — line 113 explanation, AC3.2 line 247 with full WHERE clause incl. claim-skip, Decision Matrix line 383, Plan Notes #1 line 403 reframed as adjudication record |
| C2 | frontmatter `name`/`description`/optional `tools`; drop `effort`; new Decision Matrix row for `claude_code` runner spawn path | **CLEAN** — AC1.6 line 179 spells out exactly this; two new Decision Matrix rows (Agent frontmatter line 385, Runner spawn path line 386) |
| C3 | AC7.7 reclassified as manual soft gate captured in completion summary, NOT in executor PASS/REVISE/FAIL | **CLEAN** — AC7.7 line 369 explicitly says "Manual soft gate (NOT a hard executor gate)"; Phase 7 prose line 327 confirms; Decision Matrix gains "Real-`claude` smoke gate" row (line 395) |
| C4 | AC7.1 rewritten to assert state-machine traversal with concrete final-state checks; AC7.1b for one-REVISE-cycle | **CLEAN** — AC7.1 line 362 + AC7.1b line 363; fixture list line 332 includes both `happy_2phase.jsonl` and `revise_once.jsonl` |
| M1 | strike "expose `compute(...)` publicly"; reuse existing `pub(crate)` | **CLEAN** — Phase 3 prose line 226 explicitly says "**No `pub` widening is required.**"; AC3.8 line 253 reasserts |
| M2 | Phase 3 sub-AC for JSON envelope; AC1.7 specifies role-keyed JSON; fixture path in Phase 1 file list | **CLEAN** — AC3.10 line 255, AC1.7 line 180 (fully role-keyed schema with examples), fixture list line 168, parser logic in Phase 3 step 2(e) |
| M3 | prompt content quality ACs (verbs+JSON shape, failure modes, planner mirrors Stage-1-7, length 400-1200) | **CLEAN** — AC1.7a/7b/7c/7d at lines 181-184 |
| M4 | one-line note in Phase 3 about manual install path until Phase 4 | **CLEAN** — Phase 3 prose line 224 |
| M5 | AC6.5 collapsed to single-bit DB-state check | **CLEAN** — AC6.5 line 322 ("transitions from `pending` to `answered`... otherwise exits 1") |
| M6 | AC3.3 — always built, hidden via clap `.hide(true)`, accepts fixture path; `--claude-code` requires feature | **CLEAN** — AC3.3 line 248 reads exactly this; Decision Matrix "Mock runner availability" row line 389 |
| M7 | guide prompt authored in Phase 1; AC1.1 says "5 entries"; Phase 6 builds handlers only | **CLEAN** — Phase 1 file list line 166, AC1.1 line 174, AC1.8 line 185 (count == 5), Phase 6 prose line 300 ("no new prompt authoring") |
| m1 | sketch the 30-line wrapper in Phase 7 | **CLEAN** — wrapper sketch at lines 339-359 (~18 lines, budget confirmed) |
| m2 | document `status` vs `show` distinction | **CLEAN** — Phase 5 prose lines 277-279 |
| m3 | Decision Matrix row for agent output protocol | **CLEAN** — line 387 |
| m4 | quickstart includes `--features runner-claude-code` | **CLEAN** — AC7.4 line 366; Decision Matrix "README quickstart command" row line 396 |
| m5 | Phase 3 sub-AC for skip-if-claimed in `--auto` | **CLEAN** — AC3.2 line 247 WHERE clause includes `(claimed_by IS NULL OR claimed_at < now - lock_window)`; AC3.7 line 252 covers via "live-claim skip" test |
| m6 | AC3.6 references the runner-error fixture path | **CLEAN** — AC3.6 line 251 names `tests/handlers/drive_runner_error.rs`; Files-to-create list line 230 includes the path |

**Tally:** 4 critical / 7 major / 6 minor = 17 findings. **17 landed cleanly, 0 partial, 0 missed.**

## Cycle-2-only checks

### DONE_WHEN trace (re-verify)

| Clause | Phase(s) | Cycle 1 | Cycle 2 |
|---|---|---|---|
| Fresh repo with stores installed | Phase 7 | OK (assuming AC7.1 fixed) | **OK** — AC7.1 now asserts via `stores setup` + tempdir |
| No `task-workflow` plugin | Phase 1 | OK | OK |
| No Claude Code skill manually wired | Phase 4 | OK | OK |
| `stores setup` works | Phase 4 (AC4.1–4.6) | OK | OK |
| `--auto` selects a task | Phase 3 (AC3.2) | **C1 blocker** | **FIXED** — FIFO `created_at ASC`, schema-honest |
| `--claude-code` runner | Phase 2 + Phase 3 | **C2 blocker** | **FIXED** — frontmatter spec correct, runner spawn-path documented |
| Drives state machine to `complete` | Phase 3 + Phase 7 | **C3+C4 blockers** | **FIXED** — AC7.1 asserts state transitions; AC7.7 is soft merge gate |
| Surfaces real `blocked` cleanly | Phase 3 (AC3.9) | OK | OK |
| `gate guide` available | Phase 6 | M5 (exit-code-2) | **FIXED** — AC6.5 single-bit gate-status |
| `status --follow` shows progress | Phase 5 (AC5.1–5.6) | OK | OK |

**Net: 10 of 10 clauses now covered. Trace is solid.**

### Internal consistency

- **AC3.2 ↔ Decision Matrix row 1**: identical WHERE clause and ordering. ✓
- **AC1.1 ↔ AC1.8 ↔ AC4.1**: all assert 5 agents with the same names. ✓
- **AC1.6 ↔ Decision Matrix "Agent prompt frontmatter"**: identical fields (`name` + `description` + optional `tools`, no `effort`). ✓
- **AC3.10 ↔ AC1.7 ↔ Decision Matrix "Agent output protocol"**: identical envelope shape and parsing rule (last non-empty stdout line, role-keyed JSON). ✓
- **AC7.7 ↔ Phase 7 prose ↔ Decision Matrix "Real-`claude` smoke gate"**: all consistently call this a manual soft gate at the merge level. ✓

The Intent Contract (`## Task` section, lines 27-92) still uses pre-cycle-1 language ("highest-priority", "4 agent prompts", "highest-priority then oldest" risk mitigation). The planner correctly added a clarifying note at line 113 that explicitly reconciles the DONE_WHEN with the FIFO selection. The Intent Contract is human-ratified and not a planner editing surface; the divergence is acknowledged and bridged. Acceptable.

### 5-prompt consistency

All 5 prompts (`planner`, `plan-reviewer`, `executor`, `code-reviewer`, `guide`) are listed in:
- Phase 1 Files-to-create (line 162-166) ✓
- Phase 1 fixture list (line 168) ✓
- AC1.1 (line 174 — "exactly 5 entries", names enumerated) ✓
- AC1.2 (line 175 — "writes 5 files") ✓
- AC1.7/7a (lines 180-181 — JSON envelope schemas given for each role including guide) ✓
- AC1.8 (line 185 — `BUNDLED_AGENTS.len() == 5`) ✓
- AC4.1 (line 268 — "all 5 bundled agents", names enumerated) ✓
- AC6.4 (line 321 — guide prompt's authorized-verbs list) ✓

No off-by-one drift anywhere.

## Cycle-2 issues

### Critical: 0
### Major: 0
### Minor: 1

**m7 (cycle 2). AC5.6 references a `--max-iters` flag that Phase 5 does not define.** The status handler's Phase 5 ACs only define `--follow`, `<id>`, and `--interval`. AC5.6 line 296 says "Follow-loop tests are bounded by `--max-iters` test-only flag to avoid flakiness" — but `--max-iters` belongs to `drive`, not `status`. Either add a hidden `--max-iters` to `status` for tests, or rephrase AC5.6 to say "tests use a bounded loop via test-only injection (e.g. capping the polling iteration count via a `cfg(test)` constant or env var)." Cosmetic; the executor will figure it out. Not blocking.

## Top 3 remaining concerns (post-READY)

1. **AC5.6 cycle-2 nit** — wording mismatch with Phase 5's flag set. Fix at executor's discretion; not blocking.
2. **Authoring 5 prompts at 400-1200 lines each is real work** — AC1.7d locks the floor/ceiling; reviewer must enforce at code-review time, not waved past. Risk #2 in the Intent Contract; mitigated by AC1.7a-d but execution-risky.
3. **AC7.7's manual soft gate depends on a working `claude -p` + credentials at the executor's machine** — now a soft gate (good), but the executor should still record evidence (transcript or screenshot) in the completion summary as AC7.7 specifies. Watch for "I ran it locally and it worked" hand-waves at PR time.

## Confirmation

DONE_WHEN trace is now solid: all 10 clauses covered, all 4 prior blockers resolved, no new criticals introduced.
