# Real World Workflow Takeover Analysis

**Date:** 2026-05-02
**Type:** note

## Summary

Discussion-as-design exercise: can the `stores` substrate take over Blake's real-world client `/task:open <L-id>` workflow (currently driven by a 12.5k-LOC `./dev` bash CLI in client repos)? Walked through a real transcript (L310 → T281). Confirmed two pieces of philosophy alignment, one piece I had wrong, and surfaced four substantive tensions worth scoping. New tension raised mid-discussion: **working directory propagation** — agents are spawned headlessly by stores, but T3 work happens in a worktree that the project script provisioned, so stores must somehow know where to point them.

No code written. This is the analytical pre-step before any implementation task is filed.

## Details

### Source material

- Workflow transcript: client-side `/task:open L310` run that produced T281 (`./dev` CLI ergonomics task). Captured inline in chat — terminal-redrawn duplicates are noise, not real re-runs.
- Philosophy doc: `docs/philosophy.md` (re-read mid-discussion to sanity-check the framing).

### What the transcript exposes

The `/task:open` workflow runs roughly:

1. `./dev observation show L310` — load the raw observation
2. AI drafts an `intent_contract` in chat (objective / scope / acceptance / tier_hint / known_solution)
3. Human approves
4. AI runs `./dev observation contract --draft --from-stdin` to ratify (state → `ready`)
5. AI picks next TID by scanning `tasks/{active,planning,...}/`
6. AI runs `./dev new <tid>-<slug> --links L310` → provisions worktree + branch + preflight
7. AI seeds `tasks/active/T281-.../main.md`
8. AI spawns: `task-workflow:planner` → `task-workflow:plan-reviewer` → `task-workflow:executor` (carrying notes between them)
9. (Loop continues phase by phase to wrap.)

### Mapping: transcript step → stores capability

| Transcript step | Stores has it? | Notes |
|---|---|---|
| Load observation | ✅ `stores observations show` | Direct parity |
| Draft intent_contract in chat | ⚠️ Partial — schema exists, skill does not | `required_when` enforces shape; no skill yet drafts pre-capture |
| Ratify contract (state=ready) | ✅ `required_when` on `intent_contract.contract_state == 'ready'` | Philosophy §3.1 — already the design |
| Pick next TID | ❌ `./dev` does it via `ls + sort -u` | Need `stores tasks next-id` verb |
| Provision worktree + branch | ❌ Out of scope (project-side, by design) | Inversion: project script wraps stores, not vice versa |
| Seed `main.md` | ✅ Render layer | Already deterministic from row |
| Spawn planner / reviewer / executor | ✅ `drive` loop + `guide --claude-code` | T010 closed this |
| Pass plan_review notes → executor | ⚠️ Schema gap | Precedent: `wrap_log[]` from T010; need `plan_review_notes`, `phase_review_notes`, etc. |
| Wrap (human GO/NO_GO) | ✅ T010 lifecycle extension | `in_review → accepted | rejected` |

### Two pieces of my initial framing — corrected after philosophy reread

**1. "Contract-at-capture vs contract-at-open" is not a workflow choice.** Philosophy §3.1 already mandates it via `required_when`. Flipping `contract_state: ready` *is* the moment intent gets bottled. The captor cannot opt out. So the gap is not schema — it's that the capture *skill* must drive the draft-and-ratify dialog at lodge time, not at `/task:open` time.

**2. `provision_workspace` should not be a stores hook.** Philosophy: "exactly one write path: the CLI." A project hook called from inside stores becomes an unverified write path. Inversion: **the project script wraps stores**, not the other way around. `./dev new t281 --links L310` provisions the worktree, then calls `stores tasks add` inside it. Stores never learns about worktrees.

### Tensions still open

| # | Tension | Verdict |
|---|---|---|
| A | Mode-2 (Claude Code wrapping the CLI) — does it need stores changes? | **Confirmed no.** Mode-2 is a wrapper problem, not a substrate problem. Outer Claude Code has no schema role (no `actor: ai_autonomous`, no row writes). It reads stdout; if it wants to act, it uses the CLI same as any other client. The trap to actively resist: don't give the outer agent "special powers" (e.g. "can pause drive") — that pushes orchestration up a level and breaks T010's atomicity. Worth one paragraph in `docs/philosophy.md` or new `docs/wrappers.md`. |
| B | Inter-agent note propagation (plan-review notes → executor) | **Worked through — see dedicated section below.** Notes live on reviewer envelopes (cycle-resident). Severity is binary: blocker = REQUEST_CHANGES, otherwise either a cycle-local note or — for tech debt / future work — lodged as a first-class observation via the existing observations write path. |
| C | Recursive agent spawning — outer Claude Code wraps a CLI that itself spawns Claude Codes | Not wrong, but needs explicit framing: outer instance has **no schema role**, no row writes. It is a UI. If it wants to act, it uses the CLI like anyone else. |
| D | TID picking (next-id discovery) | Tiny but real. Project scripts wrapping stores need `stores tasks next-id`. |
| **E** | **Working directory propagation (NEW)** | See dedicated section below — this is the most interesting new question raised today. |

### Tension B — Note propagation (worked through)

**Pattern.** Notes live on the reviewer envelope, written onto the cycle record (`cycles[N].plan_review.notes`, `cycles[N].phase_reviews[].notes`, `cycles[N].code_review.notes`), assembled into downstream briefs by the same overlay layer that already handles `git_diff_summary`. Not a separate top-level `review_notes[]` list_record — that would split each reviewer envelope across two writes and force denormalization of `source_phase` / `source_cycle` into every note.

**Why envelope-resident, not list_record.** `wrap_log[]` is task-level because there's only ever one wrap per task. Reviews happen N times per cycle; the cycle record is the natural history layer. The reviewer returns one validated envelope, the framework writes it once, brief assembly walks `cycles[]` (which it already does for prior plan/execution context).

**Five design calls.**

| # | Question | Decision |
|---|---|---|
| 1 | Structured or freeform? | Structured. Start with `{topic, body}`. Freeform bullets degrade — agents drift to longer prose, inconsistent shape. |
| 2 | Track resolved/open status? | No. Fire-and-forget. Adding "did the executor address this?" forces another envelope field on every executor return — overhead for marginal value. If a note keeps reappearing across cycles, that's a human signal to spot in the brief. |
| 3 | Can notes gate transitions? | No. Verdict (`APPROVED`, `REQUEST_CHANGES`) is the gate; notes are the parenthetical. Conflating them corrodes the verdict's meaning. |
| 4 | Propagation rule | All notes from prior reviews where `source_phase ≤ current_phase`, chronological. Old notes might be obsolete — executor uses judgment. Don't try to filter by relevance heuristics. |
| 5 | Brief surfacing | Grouped by source agent, chronological within group. |

**Critical refinement (Blake) — severity is binary, debt goes to observations.**

This is the move that makes the design clean. Three cases, three buckets:

| Reviewer finding | Bucket | Mechanism |
|---|---|---|
| "This stops deployment — must fix" | Blocker | `verdict: REQUEST_CHANGES` + notes explaining what |
| "Address this while you're in here" | Current-cycle concern | `verdict: APPROVED` + notes (cycle-local) |
| "Should refactor X later" / "future task to revisit Y" / tech debt | Future work | Reviewer envelope includes `observations: [...]`; framework writes them via the existing observations write path with a source pointer back to (task, cycle, reviewer_role) |

The reviewer agent picks the bucket; the schema enforces the shape; the substrate handles the routing.

**Why this is elegant.**
- Reuses existing substrate. Observations store is already the queue of "stuff to triage into future tasks." Making it the natural sink for reviewer-discovered debt means everything funnels through one triage path — no new "debt log" concept.
- Notes stay tight: current-cycle context only. They don't accumulate cruft across cycles; cycle-local concerns die with the cycle (rendered into the brief once, then become history).
- Future work becomes first-class: a tech-debt observation can be triaged into its own contract with the same `intent_contract` machinery as any other observation.
- No severity enum needed — there are only two real cases (stops shipping = REQUEST_CHANGES, doesn't = note or observation).

**Side benefit.** Observations get a new `source: review` type with pointer fields (`task_id`, `cycle`, `reviewer_role`). Trivially queryable: "show me everything plan-review surfaced across my T3 tasks last week." Long-term this is a feedback signal on the reviewer agents themselves — if one of them is consistently producing observations that get triaged to "won't fix," its prompt needs tuning.

**Schema delta (revised).**

| File | Change |
|---|---|
| `agents/schemas/{plan_review,phase_review,code_review}.schema.json` | Add `notes: [{topic, body}]` and `observations: [{title, body, ...observation fields}]` |
| `src/render/context.rs` (or brief overlay site) | Walk cycles, gather notes (filtered by `source_phase ≤ current_phase`), surface in template |
| Brief templates (executor briefs) | Add `{{review_notes_section}}` block |
| Framework envelope-write path | When reviewer envelope contains `observations`, call observations.add for each, with source pointer |
| Observations schema | Add `source: review` enum value; add optional source-pointer fields (`source_task_id`, `source_cycle`, `source_reviewer_role`) |

**What to verify before scoping — verified 2026-05-02.**

| # | Question | Finding |
|---|---|---|
| 1 | `agents/schemas/plan-reviewer.schema.json` envelope shape | Fields: `gate` (string, "READY, NEEDS_WORK, etc." — open-ended), `summary`, `open_questions: [string]`, `reasoning`. No structured notes, no observations sink. `open_questions` is the closest thing to "notes" today and probably wants to be replaced rather than left alongside. |
| 2 | `agents/schemas/code-reviewer.schema.json` envelope shape | **Surprise: already carries graded severity.** `details` is freeform "one per line, tagged `[CRITICAL] [MAJOR] [MINOR]`" + `counts: {critical, major, minor}` summary. This is exactly the graded-severity pattern the new design kills. See "Open design call" below. |
| 3 | `src/render/context.rs` (brief overlay) | Pure `(schema, entry) → Value`. Already derives `current_cycle_for_phase`, `plan_phases_count`, `current_phase_idx`, **`cycles_have_reviews`** (so templates already conditionally surface review presence — precedent slot exists). Does NOT yet walk cycles to gather notes for brief. Adding `review_notes_for_brief` here is ~30 LOC, no I/O needed. Cleanly separates from drive.rs overlay layer (which is for I/O additions like `git_diff_summary`). |
| 4 | `stores/observations/schema.yaml` source enum + back-pointer | `source` enum is `[dashboard, qa, dev, sentry, intake, converge, wrap]` — needs `review` added. `task_id` already exists as soft-FK back to a task (T010). But `task_id` alone is too coarse — need cycle/phase/reviewer_role disambiguation. Add a `source_context` record (cycle_number, phase_number, reviewer_role). Top-level `notes: json` exists but is a catch-all for unrelated metadata — don't conflate. |

**Design call — DECIDED 2026-05-02 (Blake): path (a), migrate to binary.** Code-reviewer's existing `[CRITICAL]/[MAJOR]/[MINOR]` tri-grade conflicts with the binary-severity decision. Two paths considered:

- **(a) Apply binary rule consistently.** Code-reviewer redesigns: `[CRITICAL]` → blocker → `REQUEST_CHANGES`; `[MAJOR]` → cycle-local note; `[MINOR]` → future-work observation. Drops `counts` + freeform `details`. Cleaner; a real change to a working agent.
- **(b) Code-reviewer keeps tri-grade.** Plan/phase reviewers adopt new binary shape; code-reviewer stays as-is. Inconsistent but defensible if code-quality findings genuinely warrant more granularity.

**Recommendation: (a).** The whole reason for binary is that MINOR is where intent-to-fix goes to die. Forcing every MINOR into the observations queue means triage or explicit `wont_fix` — no more "low-priority bullets nobody returns to."

**Verification (Explore subagent, 2026-05-02) — reinforces (a):**
- `[CRITICAL]/[MAJOR]/[MINOR]` tags in `details` are **never parsed by any Rust code** — stored verbatim, rendered verbatim. Tri-grade does no structural work today; it's visual hinting in a freeform blob. Migrating to structured `notes` + `observations` loses no typed data.
- BUT `critical/major/minor` are persisted as **separate integer columns** in `cycles[].review`. So path (a) is a real schema migration, not just an envelope reshape.
- File-level impact (~10 files): `agents/schemas/code-reviewer.schema.json`, `stores/tasks/schema.yaml` (cycles.review record), `src/codegen/ddl.rs`, `src/handlers/drive.rs`, `src/handlers/submit.rs` (`compute_submit_review` signature), `src/cli/dispatch.rs` (drop `--critical/--major/--minor` flags), `code-reviewer-brief.md.tpl`, `main.md.tpl`, 5+ integration tests, `tests/fixtures/agent_outputs/code-reviewer.json`, `agents/code-reviewer.md` prompt.
- Verdict: **medium task**. Splittable: schema + codegen + DDL first, then handlers + CLI + templates + tests as a follow-on.

**Revised scope (assuming path a).**

| Piece | Size | Notes |
|---|---|---|
| plan-reviewer envelope: replace `open_questions` with `notes` + `observations`; verdict to enum | Small | Greenfield |
| code-reviewer envelope: drop `details`/`counts`, add `notes` + `observations`, verdict to enum | Medium | Breaking; needs migration of any existing cycle records with old shape |
| Add `review` to observations.source enum; add `source_context` record (cycle_number, phase_number, reviewer_role) | Small | 1 enum value, 1 record (3 fields) |
| Framework write-path: when envelope has `observations[]`, call observations.add for each (with source_context populated) | Medium | New code in drive (or envelope-write site); needs to handle partial failure |
| `context.rs`: add `review_notes_for_brief` derived key (gather from cycles[].plan_review.notes etc., filter by source_phase ≤ current_phase, sort chronologically) | Small | ~30 LOC pure derivation |
| Brief templates: `{{#if review_notes_for_brief}}` section in executor + planner-on-revise briefs | Small | Per template |

Total: 1–2 days as one task, or two small tasks split at the framework-write-path boundary.

### Tension E — Working directory propagation

**Context:** Blake's tmux session always runs from the main worktree. `pwd` is always main. But T3 work happens in a feature worktree that `./dev new` provisions. Headlessly-spawned agents need to know where to `cd` before doing anything.

**Why this matters:** Stores' `drive` spawns agents. Those agents need a `cwd`. If stores doesn't know the worktree path, it can only spawn agents in its own `cwd` — which is wrong for any task that has a feature branch.

**Options on the table:**

1. **Store `workspace_path` on the task row.** Project script writes it during `stores tasks add` (it just provisioned the worktree, so it knows the path). Drive reads it when spawning. This is the cleanest schema fit — workspace location becomes a typed, validated field like everything else.
2. **Store a `setup_script` reference instead of a path.** More flexible (script can do extra setup), but adds a project-side execution dependency that stores has to invoke — risks the same "unverified write path" issue from the worktree-hook discussion.
3. **Make it the spawning skill's job.** The outer wrapper (project-side) tells stores where to spawn. But stores spawns autonomously in `drive` — there's no outer wrapper at that point. So this only works for mode-2.

**DECIDED 2026-05-02 (Blake): Option 1.** Add `workspace_path` to the task schema, written by whoever creates the task row (typically the project script, which just provisioned the worktree). Drive uses it as `cwd` for every spawned agent. Stores never *creates* worktrees, never *invokes* setup scripts — it just records and respects the path.

**Edge case to think through:** What if the workspace_path doesn't exist when drive goes to spawn? (Worktree was deleted, machine moved, etc.) Probably: drive errors out with a clear "workspace_path missing" — does not silently fall back to `pwd`, because that would silently put work in the wrong place.

**Verification (Explore subagent, 2026-05-02):**
- **Spawn site:** `src/runner/claude_code.rs:308-309` — `cmd.current_dir(&cwd)` where `cwd = std::env::current_dir()?.canonicalize()?`. Today cwd is inherited from parent process and locked at spawn.
- **Critical existing footgun (already guarded):** the Anthropic SDK silently mints a fresh session if cwd differs between spawn and resume calls (lines 33-38 explain this). So `workspace_path` MUST be canonicalized once at spawn and locked the same way, or session continuity silently breaks. Worth a comment in the new code.
- **No path-typed fields exist in any stores schema today.** Convention: paths are plain `text`. The only existing path-shaped data: `render_target_path` (template string in workflow declaration, not a field) and `files: list of text` (plan phases). So a `workspace_path: text` field is consistent with convention.
- **Tasks schema already has `branch`** in the relationship category. `workspace_path` fits naturally next to it — same conceptual cluster (where this task's work physically lives).
- **Scope: medium, not small.** ~30-50 LOC of code, BUT the `Runner::spawn` trait signature change touches 4-5 call sites (2 runner implementations + drive + tests). Half-day task.
- Schema: 1 line. Trait change: breaks `ClaudeCodeRunner` and `MockRunner`. Drive: extract field from entry, thread to spawn (~5 lines). Tests: workspace_path used / absent / non-existent path errors (~30 lines).

### What stores can already do (no changes needed)

- Capture observations with full intent_contract
- Enforce `required_when` on contract ratification
- Drive the full lifecycle (planning → wrap)
- Spawn agents via `guide --claude-code`
- Render `main.md` deterministically from rows
- Audit-trail every transition with actor + timestamp
- Wrap-mode synthesis brief (T010)

### What stores would need to add (estimated scope)

| Item | Type | Estimated size |
|---|---|---|
| `workspace_path` field on tasks (Tension E) | Schema | Small — 1 field + drive plumbing |
| `notes[]` + `observations[]` on reviewer envelopes (Tension B) | Schema + brief render + write-path | Small-medium — envelope schemas + brief overlay + framework call to `observations.add` for debt-lodging + observations source-pointer fields |
| `stores tasks next-id` verb (Tension D) | CLI | Small — read-only scan |
| Capture-time draft skill (Tension §"Two corrected") | Skill, not stores | Small — lives in project, calls stores CLI |
| Documentation: project-script-wraps-stores pattern (Tension §"Two corrected") | Docs | Small but load-bearing |

### Decisions implicitly made today

- Worktree provisioning stays project-side. Stores never knows about worktrees beyond `workspace_path`.
- Agent spawning stays inside drive (per T010 design and per the user's stated preference).
- Contract-at-capture is the target workflow shape — confirmed already supported by schema.
- Mode-2 (observing Claude Code) is a wrapper problem, not a substrate problem.
- Reviewer findings split into three buckets — blocker (REQUEST_CHANGES), cycle-local note, future-work observation. No graded severity enum; the bucket *is* the severity. Tech debt is never a "low-priority note" — it's its own observation, triable like any other.

## Ship plan — CONFIRMED 2026-05-02

After verification of all four open items, ship order confirmed by Blake:

| # | Task | Type | Size | Why this slot |
|---|---|---|---|---|
| 1 | Add wrapper-boundary section to `docs/philosophy.md` (Tension A) | Doc PR | Trivial | No deps; prevents misframing while later work is in flight. Add as "What's outside the substrate" section. |
| 2 | `workspace_path` field + `tasks next-id` verb (Tensions D + E bundled) | One task | Medium | Both serve the project-script-wraps-stores pattern. Both unblock anyone using stores from a feature worktree. Half-day each. |
| 3a | Reviewer-notes envelope + storage migration (plan-reviewer + code-reviewer + DDL + observations source enum) | Schema task | Medium | New shape is the contract. Done before write-path so writes have something to land in. |
| 3b | Framework write-path (envelope `observations[]` → `observations.add`) + brief overlay + templates | Substrate task | Medium | Builds on 3a; surfaces notes in briefs and routes debt to observations store. |

**Why this order:** (1) is free and prevents anyone giving the outer agent special powers while (2)/(3) are in flight. (2) immediately unblocks real-world client-repo use — once shipped, `./dev new t281` can pass `--workspace-path` and drive spawns agents in the right place. (3) is the highest-value but largest delta; doing it last means simpler pieces are battle-tested first.

## Follow-ups

All three confirmation calls answered by Blake (2026-05-02): path (a) on code-reviewer, Option 1 on workspace_path, recommended ship order. Outstanding actions:

- File task #1 (doc PR: add wrapper-boundary section to `docs/philosophy.md`) — trivial, ship first.
- File task #2 (workspace_path field + tasks next-id verb) once #1 lands.
- File tasks #3a (envelope + storage schema migration) and #3b (framework write-path + brief overlay + templates) in sequence once #2 lands.
- Re-test the original L310 → T281 transcript shape against the new substrate once all four ship — confirm the workflow Blake described in chat actually flows end-to-end. Probably warrants its own fixture (or repurposed e2e test).
