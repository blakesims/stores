# T010: Wrap Workflow + GO/NO_GO (last 10%)

## Meta
- **Status:** PLAN_REVIEW
- **Created:** 2026-05-01
- **Last Updated:** 2026-05-01 (planner: revisions for plan-review round 2)
- **Blocked Reason:** —

## Task

Implement the **last 10%** of the task lifecycle — the moment a task transitions from "agent says complete" to "human reviewer says GO/NO_GO." Closes the human-in-the-loop story that the substrate's first-10%/middle-80% already enforce.

**Source design:** `docs/worklog/2026-05-01/02-wrap-workflow-and-go-nogo-design.md` (full architectural rationale + Option A/B comparison + sequencing argument).

**Why this task:** The substrate already enforces `required_when` at intent-contract ratification (first 10%) and guarded transitions through the phase loop (middle 80%). The completion side currently has neither a synthesis artifact nor an actor-gated state transition — `complete` is the terminal state, with no mechanism for a human to record GO/NO_GO and no executive summary for the reviewer to read against the contract. This task closes that gap.

### Intent Contract

**Executive intent:** Extend the tasks lifecycle so that `complete` is no longer terminal. A new wrap agent produces an executive summary at task completion; the row enters `in_review`; the human reviewer reads a synthesis brief (promise + reality + synthesis + receipts) and issues GO or NO_GO via actor-gated transitions. The result is that GO/NO_GO becomes a first-class fact in the DB, not vibes recorded in chat.

**DONE_WHEN:**
1. New `wrap` agent at `agents/wrap.md` + JSON schema at `agents/schemas/wrap.schema.json`. Output envelope includes `{role: "wrap", executive_summary, deviations[], residual_risks[], recommended_sanity_checks[]}`. Agent brief input: ratified contract + cycles[] + git diff summary.
2. `tasks/schema.yaml` lifecycle extended: states gain `in_review`, `accepted`, `rejected`. Transitions added:
   - `complete → in_review` — verb `request_review`, actor `ai_autonomous`, fires automatically on drive's "all phases done" hand-off.
   - `in_review → accepted` — verb `accept`, actor `human` (terminal for v0.5).
   - `in_review → rejected` — verb `reject`, actor `human`, requires `--reason`.
   - `rejected → executing` — verb `resume`, actor `ai_with_human` (semantics resolved at planner-time per open question (c)).
3. `executive_summary` persisted on the row. Location to be decided at planner-time (open question (a)) — column on tasks vs. sub-field of a new `wrap_log` record (latter scales better if Q&A persistence is added later).
4. `agents/guide.md` graduates from v0.3 stub to a third mode: when row status is `in_review`, guide spawns in **wrap-mode** with write-back permission for the two terminal transitions + the executive_summary slot, read-only otherwise. Existing gate-mode and read-only task-mode preserved.
5. `/task:wrap` skill (in `~/.claude/skills/task:wrap` or equivalent) rewritten: spawn `stores tasks <id> guide` in wrap-mode, render the wrap brief, drop user into chat. The `accept`/`reject` decision is the natural exit.
6. Verifiable end-to-end: drive a real workflow-shaped task to PASS-on-last-phase; framework auto-fires `request_review`; row enters `in_review`; `stores tasks <id> guide` produces a brief; the human can `accept` or `reject --reason "..."` and the row lands in the corresponding terminal state.

**Scope boundaries:**

In scope:
- Wrap agent definition + schema.
- Lifecycle extension on `tasks/schema.yaml` (states + transitions + actor enforcement).
- `executive_summary` persistence (location decided at planner-time).
- Guide agent's wrap-mode (third mode alongside existing gate-mode and task-mode).
- `/task:wrap` skill rewrite (or equivalent CLI verb if planner argues for it).
- Drive integration: auto-fire `request_review` on PASS-on-last-phase.
- Tests covering the new transitions, actor enforcement, and the wrap envelope round-trip.

Out of scope:
- **Ship-as-separate-task** (build vs. ship distinction). Filed for later, after ~5 manual ships have happened by hand and patterns are visible. v0.5 terminal is `accepted`; a future `shipped` state can be added when the first ship task is filed.
- **`accepted-pending-ship` parent state.** Same reason — defer until ship-task workflow exists.
- **Wrap_log Q&A persistence.** Start ephemeral; only persist the GO/NO_GO note + flagged Q&A pairs if the planner picks the wrap_log record location. Don't over-engineer audit before knowing what gets re-read.
- **Building a generic post-completion workflow engine.** Stay manual on ship until patterns are visible.
- **Auto-running the wrap agent on every `complete`** vs lazy on `/task:wrap` invocation — open question (b), planner decides.

Should remain unchanged:
- All existing transitions in the planning → plan_review → ready → executing → code_review → complete arc.
- Existing planner / plan-reviewer / executor / code-reviewer agents and their schemas.
- Existing gate-mode of the guide agent.
- The `cycles[]` record structure.

**Proposed approach (high level):**

1. Define the wrap envelope schema first (JSON schema). Drives the agent prompt and the submit handler.
2. Extend `tasks/schema.yaml` lifecycle. The schema is the contract — get this right before writing any code.
3. Pick `executive_summary` location (column vs wrap_log record). Recommend wrap_log record if Q&A persistence is on the roadmap; column if simplicity wins for v0.5. Planner decides with rationale.
4. Add submit verb (`submit-wrap` or fold into existing pattern) and target.
5. Wire drive: on PASS-on-last-phase, instead of landing on `complete` as terminal, dispatch wrap agent → submit envelope → `request_review` → `in_review`. Decide auto vs lazy at planner-time.
6. Add wrap-mode to `agents/guide.md` keyed on row status.
7. Rewrite `/task:wrap` skill to spawn guide in wrap-mode.
8. Tests: schema validation, transition guards, actor enforcement (AI rejected from `accept`/`reject`), wrap envelope round-trip, end-to-end drive of a fixture task through the new lifecycle.

**Risks / assumptions:**

- **Drive integration risk.** The current `complete` is terminal. Inserting `in_review` between PASS-on-last-phase and the previous terminal changes the "drive done" signal. Risk: existing fixtures or callers that expected `complete` as the post-`drive` end state break. Mitigation: enumerate callers; the new "drive ends at in_review" is correct per the design but should be explicit.
- **Actor enforcement assumption.** The schema's `actor: human` on transitions is honored at write time per `philosophy.md`. Assumes T009 actor-detection (auto from `$CLAUDECODE`) is reliable. Verify behavior on both AI-invoker and human-invoker paths.
- **Guide agent multi-mode complexity.** Adding a third mode to `agents/guide.md` is at risk of becoming a routing-by-status state machine inside the agent (the very pattern philosophy.md warns against). Mitigation: keep mode selection at the framework layer (briefing template differs by status), not inside the agent prompt.
- **`rejected → executing` semantics** are unresolved (open question (c)). If "require contract amendment" is chosen, the row may need a way to re-enter contract drafting; if "same-contract re-run" is chosen, executor risks doing the same thing. Recommend the former (philosophically cleaner) but planner should think this through.
- **Schema state additions are mostly additive**, but `accepted` and `rejected` need to interact correctly with whatever queries / views exist for "active tasks" and "complete tasks." Audit downstream consumers.

**Open decisions (for planner to resolve):**

- (a) **`executive_summary` location:** top-level text column on tasks vs. sub-field of a new `wrap_log` record. Trade-off: column is simpler for v0.5; record scales better if Q&A persistence is added later. **Recommendation: planner picks with rationale; default to wrap_log record if a stub is cheap.**
- (b) **Wrap agent timing:** auto-fire on PASS-on-last-phase (eager — brief ready when human shows up) vs. lazy on `/task:wrap` invocation (saves agent calls on rubber-stamped accepts). **Recommendation: eager — the human-in-the-loop story is "the brief is waiting for you," not "you ask the agent to think first."** Planner confirms or argues otherwise.
- (c) **`rejected → executing` semantics:** require contract amendment first (philosophically cleaner — the contract changed, so the work changed) vs. same-contract re-run (faster — executor missed a case). **Recommendation: former.** Planner confirms or argues for a hybrid (e.g., `--reason "scope was wrong"` requires amendment; `--reason "executor bug"` allows re-run).
- (d) **Terminal state for v0.5:** simple `accepted` terminal vs. `accepted-pending-ship` parent state now. **Recommendation: simple `accepted` terminal for v0.5.** Add `shipped` later when the first ship task is filed. Don't enforce structure that doesn't have data behind it yet.

---

## Plan

### Objective

Close the **last 10%** of the task lifecycle by extending `tasks/schema.yaml` with a guarded GO/NO_GO gate (`complete → in_review → accepted | rejected`), introducing a new **wrap** agent that produces a synthesis brief at completion, persisting that synthesis as a `wrap_log[]` list-record on the row, and graduating the existing `stores tasks <id> guide` stub into a status-keyed third mode (wrap-mode) with write-back permission for the two terminal transitions only. The drive loop auto-fires `request_review` when the last phase passes, so by the time the human shows up the brief is already waiting. After v0.5 lands, GO/NO_GO is a typed, actor-attributed row event — not chat vibes.

### Scope

**In Scope:**
- New `wrap` agent: `agents/wrap.md` (system prompt) + `agents/schemas/wrap.schema.json` (envelope schema) + entries in `BUNDLED_AGENTS` and `BUNDLED_AGENT_SCHEMAS` (`src/cli/agents.rs`).
- Lifecycle extension on `stores/tasks/schema.yaml`: states `in_review`, `accepted`, `rejected`; transitions `complete→in_review` (`request_review`, `ai_autonomous`), `in_review→accepted` (`accept`, `human`), `in_review→rejected` (`reject`, `human`, `--reason` required), `rejected→planning` (`amend`, `ai_with_human`). Existing `submit-review PASS-on-last-phase` lands on **`complete` (kept as a transient state)** — `request_review` fires from `complete` via the on-entry follow-on machinery. The `complete` row never sits idle; it advances to `in_review` inside the same drive iteration that produced PASS.
- New `wrap_log` `list_record` field on tasks (parallel to `plan_review_log`) with sub-fields `executive_summary`, `deviations[]`, `residual_risks[]`, `recommended_sanity_checks[]`, `at`. The "current" synthesis is the most recent entry; persisting as a list lets us re-wrap on `rejected→planning→…→complete` round-trips without overwriting history.
- New CLI submit verb `submit-wrap` (mirrors `submit-plan-review` shape; routes through `compute_submit_wrap`) producing the `complete → in_review` transition.
- `accept` / `reject` are **plain transitions** (declared in `lifecycle.transitions`) handled by the existing `handlers::transition::run` path. `reject` requires `--reason` (writes to `blocked_reason`-style sub-field on the latest `wrap_log` entry, NOT `blocked_reason` proper, since the row is not blocked — it's in review and the human said no).
- `amend` from `rejected` returns the row to `planning` so the contract authoring round can be re-opened (Decision (c)). It is `ai_with_human` to mirror `resume`.
- Wire `complete → in_review` auto-fire: add `on_state.complete: [transition_to: in_review]` and a new `request_review` `framework`-actor transition. The follow-on machinery in `submit.rs::fire_on_entry_follow_ons` already recurses; we extend `compute_on_entry_framework_fields` to handle the new state if needed (likely no fields to set — `wrap_log` is appended by the wrap agent's submit, not by the framework).
- Drive integration: when the row enters `in_review` after `submit-review PASS-on-last-phase` follow-on chain, the loop's next `next-action` returns `next_agent: wrap`. Drive spawns the wrap agent with a brief built from contract + cycles + `git diff` summary, parses the envelope, calls `compute_submit_wrap`, which appends to `wrap_log[]` AND fires the same follow-on chain — except `in_review` has no `transition_to` follow-on (it's the human-gated wait state). The row sits in `in_review` until a human invokes `accept` or `reject`. Drive's loop sees `in_review` with no `DispatchAgent` action → exits 0 with a "waiting for human review" hint (parallel to the `blocked` exit message).
- Guide wrap-mode: `handlers::guide::run_tasks_guide_with_runner` branches on `task_entry.status`. When status is `in_review`, the brief is built by a new `build_wrap_mode_brief` function (instead of `build_tasks_brief`); when status is anything else the existing v0.3 stub brief is built. The guide agent prompt is updated to recognise three modes (gate / task / wrap), but **mode dispatch lives in the briefing template selection at the framework layer**, not in the agent prompt — the agent's mode is told to it via the brief header, not derived by the agent from row state. Guide's authorized verbs in wrap-mode are extended to include `stores tasks accept` and `stores tasks reject --reason`.
- `/task:wrap` skill rewrite: the project's bundled skill at `skills/task:wrap/SKILL.md` (new directory) instructs the user to run `stores tasks <id> guide --claude-code` against an `in_review` row and explains the accept/reject loop. Since skills are slim per the cli-vs-skill-split design, the skill is essentially a one-liner pointing at the CLI verb; the heavy lifting (brief render, CLI invocation) is the framework's job.
- Tests: schema migration validity (state set, transition table, ambiguity check); transition guards (correct `requires_gate` partitioning on `submit-wrap`-less plain transitions; `amend` from `rejected`); actor enforcement (AI rejected from `accept` and `reject`; human rejected from `request_review`); wrap envelope round-trip via mock runner; drive end-to-end fixture (3-phase task PASSes through to `in_review`, wrap envelope written, human accepts → `accepted`).
- Touch the bundled-store fixture YAML in `submit.rs` tests (`WF_SCHEMA_YAML` in `mod tests`) so existing tests still compile and assert the new terminal is `accepted` (not `complete`).

**Out of Scope:**
- Ship-as-separate-task / `accepted-pending-ship` parent state (Decision (d) — punt to v0.6 after first manual ship).
- Ephemeral Q&A chat persistence beyond what `wrap_log` already captures.
- Parent/child task linking semantics for build→ship.
- A standalone `stores tasks accepted-list` reporting verb (just a query against `status='accepted'`).
- Auto-creating gate-store rows for "orthogonal questions during review" — that's existing `stores gate add` territory; wrap-mode just surfaces the option.
- Replacing or rewriting the existing v0.3 task-mode stub for non-`in_review` rows; it stays as-is.

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | Schema-first: lifecycle states + transitions + `wrap_log` field on `stores/tasks/schema.yaml`; ambiguity validator passes; existing fixtures updated | Medium |
| 2 | Wrap envelope contract: `agents/schemas/wrap.schema.json` + register in `BUNDLED_AGENT_SCHEMAS`; `AgentEnvelope::Wrap` variant in `drive.rs`; planner-style "shape gate" via serde tag | Low |
| 3 | Wrap submit handler: `compute_submit_wrap` in `submit.rs` (mirrors `compute_submit_plan_review`); `submit-wrap` CLI verb wiring in `cli/dispatch.rs`; on-entry follow-on machinery handles `complete → in_review` | Medium |
| 4 | Wrap agent prompt: `agents/wrap.md` + `BUNDLED_AGENTS` registration; wrap-brief template at `stores/tasks/templates/wrap-brief.md.tpl`; `next-action` returns `next_agent=wrap` for `in_review` (via `on_state.in_review: [dispatch_agent: wrap]`); drive loop dispatches and submits | Medium |
| 5 | Guide wrap-mode: status-keyed brief selection in `handlers/guide.rs`; `agents/guide.md` updated to describe the three modes; authorized-verbs list expanded for wrap-mode; `/task:wrap` skill at `skills/task:wrap/SKILL.md` | Medium |
| 6 | Tests + e2e fixture: unit tests for new transitions, actor enforcement, wrap envelope round-trip; `tests/drive_e2e.sh` AC7.5 — drive a 2-phase task through to `in_review`, wrap-mode guide accepts via `--mock` | High |
| 7 | Worklog note + global-task-manager update | Low |

### Phase Details

#### Phase 1: Lifecycle schema extension

**Objective:** Land the schema contract before any code. Extend `stores/tasks/schema.yaml` with the new states, transitions, and `wrap_log` field; verify the install-time ambiguity validator accepts the result; update the in-test `WF_SCHEMA_YAML` fixture so existing tests still pass.

**Files to modify:**
- `stores/tasks/schema.yaml`: add `in_review`, `accepted`, `rejected` to `lifecycle.states`; add four transitions (see Decision Matrix); add `wrap_log` field; add `on_state.complete: [transition_to: in_review]` and `on_state.in_review: [dispatch_agent: wrap]`; add `submit-wrap: wrap_log` to `submit_targets`; add `wrap` agent role.
- `src/handlers/submit.rs` (test fixture `WF_SCHEMA_YAML` only): mirror the schema changes so existing unit tests don't break. PASS-on-last-phase tests assert final status of `complete` — those must update to walk through `in_review → accepted` OR the assertion target must change to `complete` as the **immediate** post-submit-review state with the follow-on tested separately.

**Acceptance Criteria:**
- [ ] AC1.1: `stores tasks schema` (or equivalent inspect) loads the new schema without error.
- [ ] AC1.2: `cargo test schema::lifecycle` passes — including a new test asserting `validate_transition_ambiguity()` accepts the four new transitions (no two unguarded transitions share `(from, verb)`).
- [ ] AC1.3: `lifecycle.states` contains exactly: `[planning, plan_review, ready, executing, code_review, blocked, complete, in_review, accepted, rejected]` (10 states; existing 7 + 3 new).
- [ ] AC1.4: `wrap_log` is a `list_record` field with sub-fields `{executive_summary, deviations, residual_risks, recommended_sanity_checks, reject_reason, at}` where the four list-typed sub-fields use `{list: text}` shape, mirroring `plan_review_log.open_questions`.
- [ ] AC1.5: `on_state.complete` contains exactly `[transition_to: in_review]` and `on_state.in_review` contains exactly `[dispatch_agent: wrap]`.
- [ ] AC1.6: `submit_targets.submit-wrap == wrap_log`.
- [ ] AC1.7: Existing tests with hard-coded `"complete"` terminal assertions are migrated. **Specific call-outs** (verified by grep on the working tree at planning time):
  - `src/handlers/submit.rs::ac5_3_submit_review_pass_last_phase_completes` (currently asserts `out.new_status == "complete"` and `read_status == "complete"` at lines ~1495–1496). Under the new schema, PASS-on-last-phase still lands at `complete` for one tx-step but the on-entry follow-on chains to `in_review` in the same tx. The assertion target becomes `"in_review"` (post-follow-on observable status) — confirming Decision (e) at the unit level.
  - `src/handlers/drive.rs` happy-path test at line ~968 (`assert_eq!(na.status, "complete", "task should be complete after drive")`). Under the new schema, drive will advance through `complete → in_review` and either dispatch the wrap agent (if the mock runner returns one) or exit with "waiting for human". The fix: extend the `MockRunner` sequence at line ~966 to include a 5th `make_run_output(wrap_fixture_json(), 0)` and update the assertion to `na.status == "in_review"` (drive exits awaiting human accept/reject), OR `"accepted"` if the test is extended further to invoke `accept` afterwards. Recommended: assert `"in_review"` and add a separate test that drives all the way through `accept`.
  - `src/handlers/drive.rs` line ~1152 (the `"complete"` literal in a fixture insert — this is a setup line, not an assertion; verify it's still a valid starting state after Phase 1 lands. It should be — `complete` remains a real lifecycle state, just no longer terminal).
  - **Sweep**: before claiming Phase 1 done, the executor runs `grep -n '"complete"' src/handlers/submit.rs src/handlers/drive.rs src/handlers/transition.rs` and confirms every remaining hit is either (a) a setup/insert line (still valid), (b) a control-flow check on `na.status == "complete"` in `drive.rs::339` (still valid — `complete` is reachable mid-loop), or (c) a migrated assertion. No `"complete"` literal in a terminal-status assertion may remain.
  - The `WF_SCHEMA_YAML` fixture in `submit.rs::tests` (the embedded test schema string) must mirror the new lifecycle so the migrated tests have a schema to resolve against. Specifically: add `in_review`, `accepted`, `rejected` states; add the four new transitions; add `on_state.complete: [transition_to: in_review]` so the tx-time follow-on fires. Without this fixture update, `find_transition` calls in the test will fail with "no transition" errors.
- [ ] AC1.8: `cargo build --features runner-claude-code` succeeds.
- [ ] AC1.9: Full sweep clean: `grep -rn '"complete"' src/handlers/ tests/ | grep -v ':[0-9]*://' | grep -v '\.html\|\.json'` reports no terminal-status assertions still pinned to `complete`.

**Decisions made within the phase:**
- The PASS-on-last-phase transition's `to` field stays `complete`; the new `request_review` transition is a separate framework-actor edge fired by the on-entry follow-on. Rationale: keeps the existing `submit-review` algorithm unchanged; the `complete` state is briefly entered on every successful task and immediately advanced; `complete` is no longer terminal but stays in the lifecycle as the natural "build done, awaiting wrap" gate. Alternative (rewriting PASS-on-last-phase to land on `in_review` directly) would push the wrap dispatch logic into `submit-review`, conflating two concerns.
- `rejected → planning` (verb `amend`) re-uses the planning state rather than introducing a new `amending` state. Rationale: the contract authoring round IS planning; the planner agent already handles the case where prior `plan_review_log` is present (it appends, doesn't overwrite). When `cycles[]` is non-empty on re-planning, the planner brief surfaces the prior cycles as historical context. No new state shape needed.
- `wrap_log` carries `reject_reason` so a reject doesn't pollute `blocked_reason` (which is reserved for `status='blocked'` rows per the canonical predicate in `handlers::is_blocked`).

**Dependencies:** None (this is the foundation).

#### Phase 2: Wrap envelope schema

**Objective:** Define the wrap-agent JSON envelope as a Draft 2020-12 schema, register it in the bundled-schemas registry, and add an `AgentEnvelope::Wrap` variant in `drive.rs` so the role-tagged dispatch table recognises it.

**Files to modify:**
- `agents/schemas/wrap.schema.json`: new file. Required: `executive_summary`. Optional: `reasoning`, `deviations[]`, `residual_risks[]`, `recommended_sanity_checks[]`. `role: const "wrap"`. `additionalProperties: false`. Mirrors planner schema's recovery-pattern conventions (reasoning slot first).
- `src/cli/agents.rs`: add `("wrap", include_str!("../../agents/schemas/wrap.schema.json"))` to `BUNDLED_AGENT_SCHEMAS`. Update the `bundled_schemas_count_matches_agents` test to expect 6.
- `src/handlers/drive.rs`: extend `enum AgentEnvelope` with `Wrap { executive_summary: String, deviations: Vec<String>, residual_risks: Vec<String>, recommended_sanity_checks: Vec<String> }`; extend `dispatch_submit` to call `compute_submit_wrap` when the role matches.

**Acceptance Criteria:**
- [ ] AC2.1: `cargo test cli::agents::tests::bundled_schemas_count_matches_agents` passes with `len() == 6`.
- [ ] AC2.2: `tests/schemas_validate_fixtures.rs` (existing) extended to validate a sample wrap envelope against `wrap.schema.json`; fixture file `tests/fixtures/agent_outputs/wrap.json` added.
- [ ] AC2.3: `parse_envelope` in `drive.rs` correctly routes a wrap envelope to `AgentEnvelope::Wrap`; new unit test `parse_envelope_from_wrap_fixture` mirrors the existing per-role parse tests.
- [ ] AC2.4: Role-mismatch detection (the existing `check_role_mismatch` path) covers wrap — adding a unit test where the runner returns `role: "wrap"` while drive expects `executor` errors with the existing message format.
- [ ] AC2.5: `additionalProperties: false` is asserted in the schema text.

**Decisions made within the phase:**
- Envelope sub-field shape mirrors the brief's "what to surface for review" buckets: `deviations`, `residual_risks`, `recommended_sanity_checks` are all `string[]` (one item per finding). Free-text `executive_summary` is the load-bearing slot.
- No `gate` field on the wrap envelope. The wrap agent does NOT decide GO/NO_GO — it produces the synthesis; the human decides via `accept` / `reject`. Putting a `gate` on the envelope would conflate AI synthesis with human decision authority.

**Dependencies:** Phase 1 (the schema must exist before drive can route to a wrap submit).

#### Phase 3: `submit-wrap` handler + auto-fire wiring

**Objective:** Implement `compute_submit_wrap` in `submit.rs`, mirroring `compute_submit_plan_review`'s 11-step pattern. Wire the CLI verb in `cli/dispatch.rs`. Verify the on-entry follow-on machinery already handles `complete → in_review` correctly (it should — `fire_on_entry_follow_ons` recurses on the new state).

**Files to modify:**
- `src/handlers/submit.rs`: add `compute_submit_wrap(schema, conn, display_id, wrap_log_entry: Value, invoker: Actor) -> Result<SubmitOutput>`. Reads the row in state `complete` (must equal `complete`), appends to `wrap_log[]`, transitions `complete → in_review` via `find_transition` with verb `request_review` and actor `ai_autonomous`. The transition fires the existing follow-on chain (which is empty for `in_review` since its only `on_state` action is `dispatch_agent: wrap` — that's not a `TransitionTo`). Add `pub fn run_submit_wrap(...)` printer wrapper.
- `src/cli/dispatch.rs`: add `Some(("submit-wrap", sub)) => ...` arm wiring `--summary-from-file` (the executive summary) plus `--deviations-from-file`, `--residual-risks-from-file`, `--sanity-checks-from-file` (one-per-line). Mirror `submit-plan-review`'s arg style.
- `src/handlers/submit.rs::compute_on_entry_framework_fields`: extend to handle `target_state == "in_review"` — likely no-op (no framework fields to set), but explicit comment saying so prevents future drift.

**Acceptance Criteria:**
- [ ] AC3.1: `compute_submit_wrap` rejects the call when row status ≠ `complete` with the existing error format ("cannot submit-wrap: row is in state 'X', expected 'complete'").
- [ ] AC3.2: After successful `compute_submit_wrap`, row.status == `in_review`, `wrap_log[]` length is bumped by 1, and the appended entry has `at` set to `now_iso8601()`.
- [ ] AC3.3: The lock is acquired and released within the tx (no leaks); test mirrors `ac5_1_submit_execute_writes_cycle_and_transitions`'s lock-released assertion.
- [ ] AC3.4: `find_transition` correctly resolves `complete → in_review` via verb `request_review` actor `ai_autonomous` (no gate, no guard).
- [ ] AC3.5: AI invoker is accepted (the transition is `ai_autonomous`); human invoker is rejected by the actor validator.
- [ ] AC3.6: A second call to `submit-wrap` on the same row (now in `in_review`) errors with "expected 'complete'".
- [ ] AC3.7: `cli/dispatch.rs` correctly reads the four `--*-from-file` args and forwards them to `compute_submit_wrap` as a `Value::Object` matching the wrap_log entry shape.
- [ ] AC3.8: `cargo test handlers::submit` all pass; ≥ 6 new tests covering the above.

**Decisions made within the phase:**
- Pass the wrap log entry as a single `Value::Object` (mirrors how `compute_submit_plan` takes `plan_json: Value`). The dispatch layer assembles the object from the four CLI args; the handler is decoupled from CLI shape.
- No gate on `submit-wrap`. The wrap envelope has no decision; the framework handle is purely synthesis-write + status-bump. Reject and accept are plain transitions, NOT submit verbs, so they do NOT route through `submit.rs`. They use `handlers::transition::run` (which already enforces actor + transition resolution).

**Dependencies:** Phases 1–2.

#### Phase 4: Wrap agent prompt + briefing template + drive integration

**Objective:** Author `agents/wrap.md` (system prompt, persona, output protocol). Author `stores/tasks/templates/wrap-brief.md.tpl` (the brief input). Register the wrap agent in `BUNDLED_AGENTS` and the template in the bundled-stores registry. Drive's existing dispatch loop already handles arbitrary `next_agent` values via the brief-template and bundled-agent lookups — no drive logic change beyond `parse_envelope` (Phase 2).

**Files to modify:**
- `agents/wrap.md`: new file. Persona: "Senior reviewer's sherpa — read the contract, read what was delivered, write the 150-word executive summary the reviewer will read first." Includes: Brief shape, output envelope schema, examples of good/bad summaries (e.g. concrete delta callouts vs vague "looks good"). Tools: `Read`, `Glob`, `Grep`, `Bash(git diff:*)`, `Bash(git log:*)`, `Bash(stores tasks show:*)`. Forbidden: any write verb, any `submit-*` verb (drive submits in-process). Failure modes mirror planner's BLOCKED/MALFORMED-brief handling.
- `agents/schemas/wrap.schema.json`: already added in Phase 2.
- `src/cli/agents.rs::BUNDLED_AGENTS`: add `("wrap", include_str!("../../agents/wrap.md"))`. Update `all_agents_bundled` test to expect 6.
- `stores/tasks/templates/wrap-brief.md.tpl`: new template. Sections: Header (display_id, title), Promise (full `contract` block), Reality (`cycles[]` rendered as a compact table — phase, cycle, executor.summary, review.gate, review.summary), Diff (`{{git_diff_summary}}` — the value comes from a context overlay assembled by drive, NOT from `src/render/context.rs`), Critical Actions (synthesis instructions). Uses Handlebars per the existing template engine in `src/render/`.
- `src/handlers/drive.rs`: **drive is the natural place for the git shell-out**, not render. Before invoking `render_template` for the wrap brief, drive computes `git_diff_summary` locally (`git log --oneline <since-ref>..HEAD` + `git diff --stat <since-ref>..HEAD`) and passes it as a **context overlay** to `render_template`. The "since-ref" formula is documented in the Decision Matrix row (j) below. If the formula yields nothing computable (no git binary, not a repo, no master branch resolvable), the overlay value is `"<git diff unavailable>"` — the wrap agent gracefully degrades.
- `src/render/`: **no change to `src/render/context.rs`.** Render must stay pure `(schema, entry) → Value`; adding shell-out would regress determinism and couple render to working-tree state. The only change in `src/render/` is to extend `render_template`'s signature (or its caller) to accept a small **context overlay map** (`HashMap<String, Value>` or equivalent) that gets merged into the `ctx` Value before template evaluation. Non-wrap agents pass an empty overlay → behavior unchanged.
- `src/cli/dynamic.rs::BUNDLED_STORE_TEMPLATES`: register the new wrap-brief template under the `tasks` store entry.

**Acceptance Criteria:**
- [ ] AC4.1: `next-action` on a row with status=`in_review` returns `next_agent: "wrap"`.
- [ ] AC4.2: Drive successfully spawns the wrap agent via mock runner using a fixture envelope; the row transitions to `in_review` (verifying drive→submit→follow-on→wrap-mode loop) within one drive iteration after `submit-review PASS-on-last-phase`.
- [ ] AC4.3: **Dispatch idempotency via state-local flag (NOT a wrap_log timestamp heuristic).** The `drive_loop` function maintains a state-local boolean (e.g. `dispatched_wrap_this_run`) initialized `false`. The iteration that produces a wrap envelope from `dispatch_submit` (i.e. `submit_out.from_role == "wrap"`) sets the flag to `true` and immediately exits the loop with status code 0 and the message `[<id>] in_review; brief written; awaiting `stores tasks accept | reject`` (parallel to the existing `blocked` exit-0 message). Rationale: the previous heuristic (`wrap_log.length > 0 AND latest entry.at > row.updated_at - epsilon`) breaks under reject → amend → re-complete cycles, where a stale `wrap_log` entry from a previous review cycle would fool the predicate into "already wrapped" or "not yet wrapped" depending on tx timing. State-local flag is unambiguous: this iteration of drive, did *I* just submit a wrap? Yes → exit. No → continue. This violates no philosophy invariant — the schema is still the truth (the row's `wrap_log` and `status` reflect everything); the flag is only about loop control inside one drive run.
- [ ] AC4.3a: **Re-entry safety.** If drive is invoked while the row is already at status `in_review` (e.g. user retypes `stores tasks drive T001` after a reject → amend → re-complete cycle that landed back in `in_review`), the **first** iteration's `next-action` returns `next_agent: wrap`. Drive dispatches wrap → submit-wrap appends a new (correct, current-cycle) `wrap_log` entry → the state-local flag flips → drive exits. The previous cycle's `wrap_log` entry is preserved (list_record never overwrites) and the new entry is the latest. This is the desired behavior and is exercised in test `wrap_dispatch_re_entry_after_amend`.
- [ ] AC4.4: Wrap brief template renders without error against a fixture row populated with contract + 3 cycles.
- [ ] AC4.5: **`git_diff_summary` is assembled in `drive.rs` (not `src/render/context.rs`).** Drive computes the diff using the formula in Decision Matrix row (j) and passes the value to `render_template` via the new context-overlay parameter. Render code remains pure `(schema, entry) → Value` with the overlay merged at template-evaluation time. A unit test in `drive.rs::tests` (`wrap_brief_includes_git_diff_summary`) verifies the overlay reaches the rendered output; a unit test in `render::tests` (`render_template_with_overlay_merges_correctly`) verifies the overlay-merge plumbing in isolation.
- [ ] AC4.6: `git_diff_summary` degrades gracefully: when `git merge-base HEAD master` fails (e.g. detached HEAD, no master ref, no `.git` directory), the value is `"<git diff unavailable>"` and drive logs a warning but does not abort.
- [ ] AC4.7: `cargo test handlers::drive` all pass; new tests `wrap_dispatch_on_in_review_status` and `wrap_dispatch_re_entry_after_amend` cover the auto-dispatch path with mock runner.
- [ ] AC4.8: `BUNDLED_AGENTS` count test expects 6.

**Decisions made within the phase:**
- **Drive's "waiting for human" exit uses a state-local flag, not a `wrap_log` timestamp heuristic.** New control-flow branch in `drive_loop`: a local `dispatched_wrap_this_run: bool` is set when the iteration's `submit_out` originates from a wrap envelope (visible via the `AgentEnvelope::Wrap` variant or `submit_out.target_state == "in_review"`). On the iteration that flips the flag, drive prints the exit message and returns `Ok(())`. **Rationale:** the heuristic version (compare `wrap_log[-1].at` against `row.updated_at - epsilon`) was rejected by the plan reviewer and the planner agrees — under reject → amend → re-complete cycles, a stale `wrap_log` entry confuses the predicate (the row's `updated_at` gets bumped on `accept`/`reject`/`amend` transitions in between, making the timestamp comparison meaningless). The philosophy invariant is "schema is the truth" — a process heuristic over the truth is the wrong shape. The state-local flag is loop-control plumbing, not a substitute for row state, so it doesn't violate the invariant. (Defense-in-depth option: a future `last_request_review_at` row field could backstop this if drive ever gets restarted mid-flight by an external trigger; not required for v0.5.)
- **`git_diff_summary` is assembled in `drive.rs`, not `src/render/context.rs`.** Render must stay pure `(schema, entry) → Value`. Drive shelling out to `git` is fine — drive already does I/O (it spawns subprocesses, reads files). The overlay-merge mechanism is a small extension to `render_template` (or its caller) that takes a `HashMap<String, Value>` of extra context and merges it into the `ctx` Value before template evaluation. Wrap is the only current consumer; non-wrap agents pass an empty overlay and observe no change.
- The wrap-brief template lives in the **bundled-stores templates**, not in `agents/`. Same as planner-brief, executor-brief, etc. — the template is per-store, the agent is per-role.

**Dependencies:** Phases 1–3, **plus the AC4.3 state-local-flag mechanism is a prerequisite for the drive-loop wrap-dispatch implementation in this phase.** Without it, an `in_review` row with unconditional `dispatch_agent: wrap` re-dispatches forever (every iteration's `next-action` returns `next_agent: wrap` because there is no schema-level guard and the row stays at `in_review` until a human invokes `accept`/`reject`). The flag must be designed and built before — or as the first sub-task of — Phase 4's drive integration; not bolted on after.

#### Phase 5: Guide wrap-mode + `/task:wrap` skill

**Objective:** Graduate `stores tasks <id> guide` into status-keyed mode dispatch. When status is `in_review`, build a wrap-mode brief that includes the wrap_log entry the framework just wrote, plus the same Promise/Reality/Receipts shape from Phase 4's wrap-brief template (for the human's reading), plus an authorized-verbs section that includes `accept` and `reject --reason`. The guide agent prompt updates describe the three modes; the agent is told its mode by the brief header, never derives it.

**Files to modify:**
- `src/handlers/guide.rs`: in `run_tasks_guide_with_runner`, branch on `task_entry.status`. When `in_review`, call new `build_wrap_mode_brief(task_entry, wrap_log_latest)` instead of `build_tasks_brief`. Add `build_wrap_mode_brief` adjacent to the existing `build_tasks_brief`. Verbs list for wrap-mode: existing read-only set + `stores tasks accept` + `stores tasks reject --reason`.
- `agents/guide.md`: update the "Workflow Position" + "How to Read Your Brief" + "Authorized CLI Verbs" sections to describe three modes (gate / task / wrap). Add a "Wrap Mode Protocol" section parallel to "Gate Mode Protocol" / "Task Mode Protocol". Critical: the agent is **told** its mode in the brief header (already true for gate/task); wrap-mode adds nothing the agent must derive from row state. Mode dispatch is at the framework layer.
- `src/handlers/guide.rs::AUTHORIZED_VERBS`: existing const stays gate-mode-only (`stores gate answer`). Add `WRAP_MODE_VERBS` const containing `stores tasks accept` + `stores tasks reject` plus the read-only set. The brief builder picks the right list.
- `skills/task:wrap/SKILL.md`: new skill. **Path verified at planning time** by `ls /home/blake/repos/experiments/stores/skills/gate:walk/` — confirmed that the convention is `skills/<verb>:<noun>/SKILL.md` (the colon is part of the on-disk directory name, not just a slash-command rendering). Existing examples: `skills/gate:walk/SKILL.md`, `skills/observation:log/SKILL.md`, `skills/observation:triage/SKILL.md`, `skills/task:next/SKILL.md`, `skills/tasks:start/SKILL.md`. New `skills/task:wrap/SKILL.md` matches this convention. Per the cli-vs-skill-split design, the body is slim:
  ```
  ---
  name: task:wrap
  description: Wrap a completed task — read the synthesis brief and approve or reject.
  ---

  Run: `stores tasks <id> guide --claude-code`

  The task must be in status `in_review` (drive auto-fires `request_review`
  after PASS-on-last-phase). The guide agent will spawn in wrap-mode, render
  the synthesis brief (promise vs reality vs deviations), and accept your
  GO/NO_GO decision via `stores tasks accept` or `stores tasks reject --reason "..."`.
  ```
  No prose-rendering logic; no Q&A persistence logic; the framework owns it all.

**Acceptance Criteria:**
- [ ] AC5.1: `run_tasks_guide_with_runner` correctly branches: `in_review` rows get `build_wrap_mode_brief`; all other statuses get `build_tasks_brief`.
- [ ] AC5.2: `build_wrap_mode_brief` includes the latest wrap_log entry (executive_summary, deviations, residual_risks, recommended_sanity_checks), the contract block, and the cycles[] table — all from the row, no extra DB reads beyond what `read_row` already produced.
- [ ] AC5.3: Authorized-verbs section in the wrap-mode brief lists exactly: `stores tasks show`, `stores tasks list`, `stores tasks next-action`, `stores tasks accept`, `stores tasks reject`, `stores gate add` (for orthogonal questions). All other verbs forbidden.
- [ ] AC5.4: `agents/guide.md` describes three modes; the wrap-mode protocol section explains: "your authorized writes are `stores tasks accept` and `stores tasks reject --reason` — both are `actor: human`, so when invoked under `$CLAUDECODE` the framework refuses; the human is doing this, you are reading the brief and proposing the decision". This is a **schema-enforced** restriction, not a prompt-enforced one — the agent prompt simply describes the effect.
- [ ] AC5.5: A unit test in `guide.rs::tests` covering the in_review→wrap-mode-brief branch using a synthetic task row + wrap_log entry; assertion: brief content includes the executive_summary text and the accept/reject verbs.
- [ ] AC5.6: `skills/task:wrap/SKILL.md` exists; its YAML front-matter loads via the existing skill loader.

**Decisions made within the phase:**
- Guide stays a **single agent prompt** (`agents/guide.md`) describing three modes. We do NOT split into `agents/guide-gate.md` + `agents/guide-task.md` + `agents/guide-wrap.md`. Mode dispatch is the framework's job (briefing-template selection); the agent's prompt explains all three so it can correctly act on any brief it receives. This is consistent with the existing two-mode pattern.
- Skill is intentionally one-screen. The cli-vs-skill-split design says skills are slim entry points; the framework owns the heavy lifting.

**Dependencies:** Phases 1–4.

#### Phase 6: Tests + e2e fixture

**Objective:** Comprehensive test coverage. Unit tests where the existing test files already live; an e2e shell test that drives a fresh task all the way through `accepted`.

**Files to modify:**
- `src/handlers/submit.rs::tests`: add tests for `compute_submit_wrap` (happy path; wrong-state error; lock contention; appended entry has correct shape; framework follow-on fires complete→in_review correctly; idempotent guard against double-wrap). Also **migrate** `ac5_3_submit_review_pass_last_phase_completes` (lines ~1479–1496) to assert that PASS-on-last-phase observably lands at `in_review` (the on-entry follow-on chain advances from `complete` to `in_review` in the same tx). Sweep for any other `"complete"` literal terminal-status assertions per AC1.7.
- `src/handlers/transition.rs::tests`: add tests for `accept` (human invoker accepted, AI rejected via actor enforcement); `reject` (human invoker accepted, requires `--reason` non-empty); `amend` (`ai_with_human` invoker accepted from `rejected` state, lands on `planning`); state-machine illegality (`accept` from `executing` rejected with the existing "no transition" error format).
- `src/handlers/drive.rs::tests`: add `wrap_dispatch_on_in_review_status` covering: insert row at `complete`, run drive, assert it fires the on-entry follow-on to `in_review`, dispatches wrap agent, submits the envelope, exits with "waiting for human" message. Add `wrap_dispatch_re_entry_after_amend` covering: insert row at `in_review` with a stale `wrap_log` entry (simulating reject → amend → re-complete), run drive, assert state-local-flag dispatch logic correctly produces a fresh wrap entry and exits cleanly. **Migrate** the existing happy-path test at line ~968 (`assert_eq!(na.status, "complete", "task should be complete after drive")`): extend the `MockRunner` sequence at line ~966 to include a 5th `make_run_output(wrap_fixture_json(), 0)` for the wrap dispatch; update the assertion to `"in_review"` (drive exits awaiting human accept/reject). The test name may need to be updated to reflect that "complete" no longer means the end of drive.
- `tests/drive_e2e.sh`: add AC7.5 — drive a 2-phase task to `in_review` via `--mock` (extends existing happy_2phase fixture with a wrap envelope as the 5th item); then `stores tasks accept T001`; assert final status is `accepted`.
- `tests/drive_e2e.sh`: add AC7.6 — **CLI-level actor enforcement subprocess test.** Two-step assertion against the real CLI binary:
  1. With `CLAUDECODE=1` set in the environment, run `stores tasks accept T001` against an `in_review` row. Assert exit code is non-zero and stderr contains the actor-rejection error message format (the existing `transition_actor_rejects_*` pattern says "actor mismatch: AI not allowed for transition" or similar — confirm the exact string against `handlers::transition::run`'s error path during execution). The test must exercise `detect_invoker` resolution at the CLI dispatch layer, not just the lower-level `Actor::AiAutonomous` rejection inside `transition::run`. This is the integration path introduced by T009 and is what users actually experience.
  2. Without `CLAUDECODE` (env unset), run `stores tasks accept T001` against the same `in_review` row. Assert exit code 0 and final status `accepted`.
  Mirror the same two-step pattern for `reject --reason "test"`. The test is a subprocess-shape test (using `bash` constructs in `tests/drive_e2e.sh`, mirroring AC7.1 / AC7.5), not a Rust unit test.
- `tests/fixtures/agent_outputs/wrap.json`: a representative wrap envelope for the schema validation fixture suite.
- `tests/fixtures/drive_e2e/happy_2phase_with_wrap.jsonl`: extends the existing fixture with a 5th item (the wrap envelope).

**Acceptance Criteria:**
- [ ] AC6.1: All new unit tests pass under `cargo test`.
- [ ] AC6.2: `bash tests/drive_e2e.sh` exits 0 with all ACs (existing AC7.1, AC7.1b, plus new AC7.5 wrap-then-accept and AC7.6 CLI-level actor enforcement).
- [ ] AC6.3: Test naming convention: `acN_M_short_name` matches the existing AC tagging in submit.rs tests.
- [ ] AC6.4: Coverage spans: schema migration, transition guards, actor enforcement (unit AND CLI-subprocess level), wrap envelope round-trip, drive integration, accept/reject CLI invocation, end-to-end happy path.
- [ ] AC6.5: `tests/schemas_validate_fixtures.rs` validates the wrap fixture against `wrap.schema.json`.
- [ ] AC6.6: **Migrated tests pass.** `ac5_3_submit_review_pass_last_phase_completes` (renamed if helpful) asserts post-follow-on status `in_review`. The drive happy-path test at `drive.rs:~968` asserts `in_review` (drive exits awaiting human) after consuming an extended 5-item MockRunner sequence including a wrap envelope. No `"complete"` literal terminal-status assertion remains in the codebase (per AC1.9 sweep).
- [ ] AC6.7: **CLI-level actor enforcement subprocess test passes.** `CLAUDECODE=1 stores tasks accept T001` exits non-zero with an actor-mismatch error string; `stores tasks accept T001` (env unset) exits 0 and lands the row at `accepted`. Symmetric pair for `reject --reason "test"`. Test path: `tests/drive_e2e.sh` AC7.6, exercised in subprocess shape (real CLI binary, real env-var probing through `detect_invoker`).

**Decisions made within the phase:**
- E2E test asserts `accepted` as the terminal — confirms Decision (d) at the integration level.
- No e2e for `reject` path; it's unit-tested in `transition.rs::tests` because the rejection path is mechanically symmetric to the accept path and adding a second e2e variant doesn't catch additional integration bugs.

**Dependencies:** Phases 1–5.

#### Phase 7: Worklog + GTM update

**Objective:** Capture the shipped feature in the worklog system; update `tasks/global-task-manager.md` to move T010 from active to completed.

**Files to modify:**
- Run `docs/worklog/new-note.sh t010-wrap-workflow` (per `tasks/CLAUDE.md` completion procedure) to create the worklog note.
- `tasks/global-task-manager.md`: move T010 row from "Current Tasks" to "Recently Completed".
- `tasks/planning/T010-wrap-workflow/main.md` → `tasks/completed/T010-wrap-workflow/main.md` (orchestrator move per `tasks/CLAUDE.md`).

**Acceptance Criteria:**
- [ ] AC7.1: Worklog note exists at `docs/worklog/2026-MM-DD/NN-t010-wrap-workflow.md` with sections: Summary, Decisions ratified (the four decision-matrix rows below), Surprises, Follow-ups (ship-as-task at v0.6).
- [ ] AC7.2: `tasks/global-task-manager.md` updated; T010 in "Recently Completed".
- [ ] AC7.3: `main.md` for T010 ends in `/completed/`.
- [ ] AC7.4: `## Completion` section of `main.md` filled with: Completed date, Summary, Commits list, Lessons Learned (especially: anything surprising about how the existing follow-on machinery handled `complete→in_review`).

**Dependencies:** Phases 1–6.

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| (a) `executive_summary` location | (i) Top-level text column on `tasks`. (ii) Sub-field of a new `wrap_log` `list_record` parallel to `plan_review_log`. | **(ii) `wrap_log` list_record** with sub-fields `{executive_summary, deviations, residual_risks, recommended_sanity_checks, reject_reason, at}`. | A reject can re-open the task; on re-completion a new wrap is produced. A list record preserves history without overwrites and matches the existing `plan_review_log` pattern, so reviewers and renderers already know how to read it. The cost is one more JSON field on the row and one more list-record DDL line — trivial. The column option would force a "do we overwrite or version" decision later; we make the right call now. Brief production cost is identical (the framework reads `wrap_log[wrap_log.length - 1]`). |
| (b) Wrap agent timing | (i) Eager — auto-fire on PASS-on-last-phase; the brief is waiting when the human shows up. (ii) Lazy — only run on `/task:wrap` invocation; saves Claude calls on rubber-stamps. | **(i) Eager.** | The morning-queue case is load-bearing: Blake opens his queue and expects `in_review` rows to be ready to wrap. Lazy would force the human to invoke a verb that immediately spawns an agent and waits 30–90s before showing the brief — friction at exactly the moment the human is most context-loaded (deciding which of N tasks to close). Cost of an extra Claude call per task is ~$0.05 — irrelevant against the human-attention savings. Lazy also conflicts with the philosophy thesis: agents fill schema-demanded slots, so the row should reach `in_review` already populated, not require a second invocation to populate it. |
| (c) `rejected → executing` semantics | (i) Reject goes back to `planning` (contract amendment first — rationale: a NO_GO means the contract was wrong). (ii) Reject goes back to `executing` with same contract (rationale: maybe just executor missed a case). (iii) Hybrid keyed on `--reason-kind {scope, bug}`. | **(i) Reject → `planning` via verb `amend`** (ai_with_human actor). | Philosophically cleaner per the architecture document recommendation: the substrate's contract is the boundary; if work was rejected, the contract was insufficient — the human should re-state intent before more work happens. The pragmatic case (ii) — "executor missed a test case" — is already handled by the REVISE cycle WITHIN code-review; reject is the gate AFTER code-review approved, so the executor was "done" by that yardstick — what changed is the human's standard, which is a contract-level fact. Hybrid (iii) introduces an enum on `--reason-kind` whose values would be debated forever; better to add `amend-resume` (rejected→executing direct) as a future option if a real "executor bug, no contract change" reject ever occurs in practice. v0.5 starts with the strict version. |
| (d) Terminal state for v0.5 | (i) Simple `accepted` terminal. (ii) `accepted-pending-ship` parent state now, with `shipped` later. | **(i) Simple `accepted` terminal.** | Per the architecture document's "build vs ship" discussion: don't enforce structure that doesn't have data behind it yet. The first 5 ship cycles will happen by hand; only after that will we know what `shipped` should mean schema-wise. Adding `accepted-pending-ship` now would either be a placeholder that does nothing (clutter) or commit to a parent/child task structure we haven't designed. v0.5 ships with `accepted` as terminal; v0.6 adds `shipped` when the first ship-task is filed and the parent/child shape is observable. |
| (e) PASS-on-last-phase land state | (i) Modify the existing `submit-review PASS-last-phase` transition to land on `in_review` directly. (ii) Keep `complete` as the immediate post-PASS state; add `complete → in_review` as an on-entry follow-on. | **(ii) Keep `complete`; add follow-on.** | Per the hard constraint to wire drive integration explicitly: option (i) conflates two concerns (review-result + wrap-readiness) into one transition's `to`. Option (ii) lets the existing `submit-review` algorithm stay byte-identical and uses the existing `fire_on_entry_follow_ons` recursion (which already handles `ready→executing` the same way). `complete` is no longer terminal but stays in the lifecycle as the natural "all phases passed; awaiting wrap" momentary state — the row never sits in it (the follow-on fires within the same tx) but it remains a meaningful intermediate fact in the audit trail (`updated_by = framework` rows in the log). |
| (f) Mode dispatch for guide | (i) Mode in the agent prompt — guide reads row.status and decides which protocol to follow. (ii) Mode in the brief header — framework picks the briefing template based on row.status; agent reads its mode from the brief. | **(ii) Mode in the brief header.** | Hard constraint requirement: schema is the contract; mode dispatch belongs at the framework layer. Option (i) would push routing logic into the agent prompt — the exact pattern philosophy.md warns against. Option (ii) is the existing pattern (gate-mode and task-mode briefs already differ; the agent reads `Mode: gate` / `Mode: task` from the header). Adding `Mode: wrap` is a one-line extension of the existing pattern. |
| (g) Reject re-loop start | (i) `rejected → executing` direct. (ii) `rejected → planning` via verb `amend`. | **(ii) `rejected → planning`** consistent with Decision (c). | Same rationale: re-state intent before re-doing work. Verb-name choice (`amend` vs `resume`) is promoted to its own row — see Decision (i). |
| (h) Wrap envelope schema strictness | (i) Permissive — only require `executive_summary`, allow extra fields. (ii) Strict — `additionalProperties: false`, all four optional fields explicitly typed. | **(ii) Strict.** | Mirrors planner.schema.json convention. Strictness catches typos in the wrap agent's output before they silently land in the DB. The reasoning slot is opt-in but typed as `string`. |
| (i) Verb name for `rejected → planning` | (i) Reuse `resume` (parallel to `blocked → ready`). (ii) New verb `amend` distinct from `resume`. | **(ii) `amend`.** | The two verbs have meaningfully different semantics that must not be conflated. `resume` (existing, `blocked → ready`) preserves `current_phase` and `current_cycle` — the work picks up where it paused. `amend` (new, `rejected → planning`) **resets the row to phase 0** and re-opens the contract authoring round, because a rejection means the contract was wrong; the executor's prior phase progress is no longer the "right" thing to resume. Reusing `resume` would create silent ambiguity — a future reader of the schema sees one verb on two different transitions and has to guess which semantics applies. Distinct verb names make the schema self-documenting. The existing `find_transition` ambiguity validator already requires distinct (from, verb) pairs, so this is also the path of least resistance for the lifecycle plumbing. |
| (j) `git_diff_summary` since-ref formula | (i) Use the row's `branch` field directly, fail if unset. (ii) `git merge-base HEAD master`, falling back to first cycle commit if `branch` is unset, falling back to `<git diff unavailable>` if both fail. | **(ii) `git merge-base HEAD master`** with documented fallbacks. | The row's `branch` field is `required: false` (per `tasks/schema.yaml` audited at planning time), so option (i) would fail on rows without one — and we observably have such rows. Option (ii) yields a sensible diff in the common case (`HEAD` is a branch off `master`, the diff is "everything since divergence"), gracefully degrades when the merge-base cannot be computed (detached HEAD, no master, no `.git`), and never blocks brief rendering. Fallback chain: (a) try `git merge-base HEAD master`, use that as `<since-ref>`; (b) if that fails, try the commit hash from `cycles[0].executor.commit` (the first executor commit on this task) as `<since-ref>`; (c) if that fails too, value becomes `"<git diff unavailable>"` and a warning is logged to drive's stderr. The diff body itself is `git log --oneline <since-ref>..HEAD` joined with `git diff --stat <since-ref>..HEAD`, both shelled out from `drive.rs`, never from `src/render/`. |
| (k) Dispatch idempotency mechanism (drive ↔ `in_review`) | (i) Wrap_log timestamp heuristic (`wrap_log[-1].at > row.updated_at - epsilon`). (ii) State-local flag inside `drive_loop` (sub-bullet of (k): the iteration that produces a wrap envelope flips the flag and exits). (iii) Schema-level guard on `on_state.in_review.dispatch_agent` (e.g. `last_request_review_at` row field gates the action). | **(ii) State-local flag inside `drive_loop`.** | Option (i) was the planner's first pass and was correctly rejected by the plan reviewer — under reject → amend → re-complete cycles, a stale wrap_log entry from a previous review cycle confuses the predicate (`row.updated_at` gets bumped on `accept`/`reject`/`amend` transitions in between, making the timestamp comparison meaningless or outright wrong). It also violates the philosophy invariant: schema is the truth, not "did this row change recently". Option (iii) is the substrate-clean version and would compose better against external triggers, but requires schema work and a new row field that v0.5 doesn't otherwise need. Option (ii) is the smallest correct fix: the flag is loop-control plumbing for one drive run, not a substitute for row state, so it doesn't violate the invariant. Drive currently only enters `in_review` from one of two paths (PASS-on-last-phase follow-on chain inside the same iteration; or human re-invocation of `stores tasks drive T001` against an already-`in_review` row), and both are handled correctly by the flag (see AC4.3a re-entry safety). If a future external-trigger restart of drive becomes a thing, option (iii) can be added as defense-in-depth without breaking option (ii)'s correctness. |

---

## Plan Review

### Round 2 — Planner revision (2026-05-01)

- **Gate (planner self-attest):** READY-FOR-REVIEW (round 2)
- **Status:** awaiting plan-reviewer round-2 verdict.
- **Round-1 outcome:** NEEDS_WORK with seven specific corrections (full review at `plan-review.md`).
- **Revisions applied (mapping each correction to where it landed):**
  1. **AC4.3 dispatch idempotency** — heuristic replaced with state-local flag inside `drive_loop`. New AC4.3 + AC4.3a (re-entry safety after amend) added. Phase 4 "Decisions made" rewritten. Decision Matrix row (k) added with full rationale (philosophy invariant, why option (i) breaks under reject→amend→re-complete, why state-local flag is correct).
  2. **`git_diff_summary` location** — moved out of `src/render/context.rs` (which stays pure `(schema, entry) → Value`) into `drive.rs` via a new context-overlay parameter on `render_template`. Phase 4 file list updated. AC4.5 / AC4.6 rewritten. Decision Matrix row (j) added documenting the since-ref formula (`git merge-base HEAD master`, falling back to `cycles[0].executor.commit`, falling back to `<git diff unavailable>`).
  3. **Test fixture migration enumeration** — Phase 1 AC1.7 now enumerates `submit.rs::ac5_3_submit_review_pass_last_phase_completes` (lines ~1479–1496), `drive.rs:~968` happy-path test, and `drive.rs:~1152` setup line, plus a sweep requirement. New AC1.9 requires a clean `grep` sweep. Phase 6 file list documents both migrations explicitly. New AC6.6 covers migrated tests passing.
  4. **`amend` vs `resume` verb naming** — promoted from passing mention in Decision (g) to first-class Decision Matrix row (i) with full semantic rationale (resume preserves current_phase; amend resets to phase 0). Decision (g) now points at (i).
  5. **Skill path verification** — verified at planning time via `ls /home/blake/repos/experiments/stores/skills/gate:walk/`. Convention is `skills/<verb>:<noun>/SKILL.md` with the colon as part of the on-disk directory name (NOT a slash-command rendering artifact). Phase 5 file list updated to note the verification with examples.
  6. **CLI-level actor enforcement integration test** — new AC6.7 added; `tests/drive_e2e.sh::AC7.6` documented as a subprocess-shape test exercising real CLI binary with `CLAUDECODE=1` set vs unset, asserting both `accept` and `reject --reason` paths.
  7. **Phase 4 dependency on AC4.3 fix** — Phase 4 Dependencies line now explicitly names the state-local-flag mechanism as a prerequisite (not a post-hoc tweak), with rationale about the infinite-redispatch failure mode if it lands later.

- **Architecture unchanged:** Decisions (a)–(h) remain ratified. No phase reshuffling. No DONE_WHEN scope change. The corrections are spec-tightening only.

### Round 1 — Plan-reviewer (2026-05-01)

- **Gate:** **NEEDS_WORK**
- **Reviewed:** 2026-05-01
- **Open Questions Finalized:** None — Decisions (a)–(h) are all argued and ratified in the plan.

### Summary

The plan is substantively sound. Decisions (a)–(h) are correct, the philosophy alignment is solid (schema as contract, framework-layer mode dispatch, no gate on the wrap envelope, `accept`/`reject` as plain transitions through the existing actor-enforced path), and Phase 1 schema-first ordering is right. All six DONE_WHEN bullets are addressed in the right phases.

Three concrete implementation corrections must land before execution begins. None requires rethinking architecture; they tighten the spec.

### Issues Found (must address before READY)

1. **AC4.3 dispatch idempotency — REJECT the `wrap_log[].at vs row.updated_at - epsilon` heuristic.** Re-entry after a reject→amend→re-complete cycle would mis-classify a stale entry as "wrap not yet written" and re-dispatch, duplicating the row's wrap_log. Replace with a state-local flag inside `drive_loop` (the iteration that just submitted a wrap envelope flips a bool and exits cleanly), optionally backstopped by a schema-level guard on `on_state.in_review.dispatch_agent`. Update Phase 4 "Decisions made" accordingly.

2. **`git_diff_summary` — REMOVE from `src/render/context.rs`.** Render must stay deterministic from row state; a git shell-out from `build_context` regresses determinism and couples render to working-tree environment. Move the diff computation to `drive.rs` (drive already does I/O), assemble `git_diff_summary` there, and pass it to `render_template` as a context overlay. Document the "since-ref" formula (suggested: `git merge-base HEAD master`) in the Decision Matrix.

3. **Test fixture migration — enumerate the affected tests.** Phase 1 AC1.7 and Phase 6 must explicitly call out:
   - `submit.rs::tests` PASS-last assertions (around lines 1638, 1656) that currently land at `complete`.
   - `drive.rs::tests` happy-path test at line ~968 that asserts `na.status == "complete"` after drive — this test breaks the moment Phase 1 lands and must be either rewritten with a wrap envelope in the mock runner or split into two variants.

### Smaller corrections

4. **Decision Matrix row for `amend` verb naming** — explicit "verb name `amend` (NOT `resume`); rationale: amend resets phase 0, resume preserves current_phase." Promote from passing mention to ratified decision.

5. **Skill path verification** — `skills/task:wrap/SKILL.md` is a guess. `ls` an existing skill (`skills/gate:walk/`) and commit to the actual filename convention before Phase 5.

6. **Phase 6 CLI-level actor enforcement test** — add an AC6.X bullet asserting subprocess-shape: `CLAUDECODE=1 stores tasks accept T001` exits non-zero; `stores tasks accept T001` (no env) succeeds. The unit-level `Actor::AiAutonomous` rejection is already well-covered; the integration path is what's new and load-bearing.

7. **Phase 4 dependency on Hot-spot A fix** — make explicit that the dispatch idempotency mechanism is a *prerequisite* for Phase 4's drive loop, not a post-hoc tweak. Without it, an `in_review` row with unconditional `dispatch_agent: wrap` re-dispatches forever.

### Things the plan got right

- `wrap_log` as `list_record` (Decision (a)) — preserves history across reject→amend→re-wrap.
- No `gate` field on the wrap envelope (Phase 2 sub-decision) — synthesis ≠ decision authority.
- Framework-layer mode dispatch for guide (Decision (f)) — agent prompt describes all three modes, told which via brief header, no row-state inspection in-prompt.
- `accept`/`reject` as plain transitions (Phase 3 sub-decision) — flows through existing `handlers::transition::run`, no new write path.
- `amend → planning` (Decision (c)) — philosophically clean: contract changed, restate intent.

> Full detail: `plan-review.md`

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
