# Stores Framework — Handoff v0.1 → v0.2

**Date:** 2026-04-26
**From:** Claude Opus 4.7 + Blake Sims (initial build session)
**To:** Whoever picks up v0.2 (likely a fresh Claude Code agent in this repo)
**Repo:** https://github.com/blakesims/stores

---

## ⚠️ Changelog — read before relying on this document

The architectural direction for the **tasks store** has shifted substantially since this handoff was written. Treat the original tasks-store sketch in this file as **superseded historical context**, not a specification.

### 2026-04-26 (later same day) — β architecture ratified

The marquee v0.2 task (this document called it "the tasks store") expanded into something broader: **DB-as-truth + framework-as-engine** ("β"). The framework now grows generic workflow CLI verbs (`next-action`, `brief`, `submit-execute`, `submit-review`, `submit-plan`, `submit-plan-review`, `render`) driven by per-store lifecycle metadata. Agents become thin workers receiving scoped briefings; main.md is rendered from DB rows on demand. This is the foundation for any future workflow-shaped store, not just tasks.

**The authoritative source for the in-flight work is now:** `tasks/active/T002-tasks-store-v02/main.md` (Intent Contract in `## Task`; ratified plan in `## Plan`).

Specifically superseded sections of this document:

| Section in this file | Status | What replaces it |
|---|---|---|
| "v0.2 priority list" → item #2 (`tasks` store) | **DELIVERED — see T002 main.md for the audit trail** | T002 main.md `## Plan` (10 phases, β architecture) |
| Sub-document arrays as CLI surface — three options sketched | **Superseded** | β chose option (b)+ : briefing/submission verbs (`stores tasks brief`, `stores tasks submit-execute`, etc.) bypass flat-flag flattening for workflow stores |
| Section ownership as per-section actors | **Superseded** | Agents are thin; framework owns sections via render. Plus: `actor: framework` is a new actor variant for engine-fired transitions |
| Parameterized `EXECUTING_PHASE_N` states | **Superseded** | Dropped — single `executing` state + `current_phase: integer` field with `auto_increment*` |
| Lifecycle sketch with `phase_review`, `merge_review`, `merge_ready` | **Superseded** | 7 states only: `[planning, plan_review, ready, executing, code_review, blocked, complete]`. Phase-reviewer dropped entirely. Merge-reviewer / CodeRabbit moved to a separate (out-of-scope) `tasks:wrap` skill. |
| `list_record` / `list_fk` field types deferred to Phase 7 | **Superseded** | Promoted to Phase 1 of T002 (per code-review C1) — they're foundational, not tasks-specific |
| `task_ref`-style soft FK by string | **Still valid** but generalized via `FieldType::ListFk { ref_store }` |
| Open question on storage scope (per-cwd vs `repo`) | **Resolved** | `scope: repo \| worktree \| user` field on schema; tasks/observations/gate share `repo` scope; resolves via `git rev-parse --git-common-dir` |
| Open question: sub-doc CLI surface | **Resolved** | option (b)+ via briefing/submission verbs |
| Open question: skill format convergence with pi-extension | **Out of scope for T002** | Defer to a future cross-runtime task |

Sections of this document that are **still authoritative**:

- "What you're picking up" — current repo state, test count, e2e. (Numbers move; the structural claims hold.)
- "Locked design decisions from v0.1" — all 9 still locked. β extends them, doesn't relitigate them.
- "Known issues / open work — Bugs deferred" — the 6 deferred bugs (`--tags` pipe-split, `--summary-from-file` masking, per-verb `--help` filtering, `required_when` OR/AND substring rejection, reserved-column-name collision, lock primitive) are still on the v0.2 backlog. Some may be picked up incidentally during T002 (e.g., the lock primitive is now subsumed by T002's `claimed_by`/`claimed_at` lock pattern).
- "How to develop" — workflow conventions still apply.
- "What NOT to do" — all six guardrails still apply, with one carve-out: guardrail #4 ("don't add a process layer to the framework") IS being modified by β. Workflow-shaped stores opt in via an explicit `workflow:` block; non-opt-in stores keep v0.1 "data + enforcement only" behavior. The framework gains a process layer that is generic across opt-in stores.

### 2026-04-26 (final, post-T002 ship) — `tasks` store DELIVERED

T002 shipped via β architecture. The 10-phase plan executed cleanly with two cycle-2 plan revisions and several phase-level cycle-2 code reviews. Key landed pieces:
- New schema features: `actor: framework`, `guard:` predicates, `auto_increment` / `auto_increment_within`, `workflow:` opt-in block, `scope: repo | worktree | user` storage resolution, `FieldType::ListRecord`, `FieldType::ListFk`, `requires_gate` on Transition.
- Generic workflow CLI verbs: `next-action`, `brief`, `submit-execute`, `submit-review`, `submit-plan`, `submit-plan-review`, `render`, `resume`.
- `tasks` bundled store + 4 briefing templates + main.md render template.
- `tasks:start` bundled skill.
- Final test count: ~298 unit tests + 13 e2e steps (original) + 16 tasks_e2e steps (new). All green.
- Marquee DONE_WHEN — "4th REVISE attempt rejected by schema-level guard with status auto-set to BLOCKED" — verified end-to-end at 1.1s.

**Legacy boundary:** filesystem tasks T001 (stores framework v0.1) and T002 (this work) stay as legacy filesystem-only. T003 onwards are DB rows rendered to filesystem via `stores tasks render <id>`. No automated migration of T001/T002. Do not attempt to import legacy tasks — the schema and render template are built for T003+; the legacy main.md layout differs enough that mechanical import would corrupt intent contract fields.

**v0.3 candidates** (deferred per Intent Contract scope-out):
- Phase-reviewer agent + lifecycle state
- Merge-reviewer agent + MERGE_REVIEW / MERGE_READY states
- `tasks:wrap` skill (Stage 6 CodeRabbit + Stage 7 Completion summary)
- `runs` event log store (provenance audit trail)
- `notes` store (10.06 worklog port)
- HTTP/JSON API for tasks store
- 10.06 capability YAML reconciliation in a project-specific wrapper skill (`/task:open` port)
- The 6 deferred bugs from the original v0.2 handoff (`--tags` pipe-split, `--summary-from-file` masking, per-verb `--help` filtering, `required_when` OR/AND substring rejection, reserved-column-name collision, the lock primitive — partially subsumed by tasks's `claimed_by`/`claimed_at`)
- Documentation polish from Phase 8 (skill gate enumeration, task-workflow plugin dependency declaration in skill frontmatter)

### Why the shift

A second-opinion review of the original handoff surfaced a fundamental ambiguity: source of truth. The original sketch implicitly mixed "DB has rows; main.md is the agents' working document" — leading to two write paths and inevitable drift. β commits to one write path (the CLI), with main.md as a deterministic render. The pi-extension's graph-engine had already pioneered this pattern; β lifts it into the framework so any orchestrator (Claude Code, pi, future runtimes) can drive the same state machine.

The cost: T002 is ~3-4 weeks elapsed and ~4000-6000 LOC, vs the original handoff's ~2 weeks and 1500-2500 LOC. The benefit: every future workflow-shaped store gets the engine for free, and the marquee 4th-revise-→-BLOCKED enforcement happens at schema level (untestable in markdown, mechanical in DB).

### Ratified decisions (locked at T002 Intent Contract; do not relitigate)

| # | Decision | Locked value |
|---|---|---|
| 1 | Architecture | β: DB-as-truth + workflow engine in framework |
| 2 | Workflow opt-in | Explicit `workflow:` block in schema (not implicit) |
| 3 | CLI verb shape | Split verbs: `brief` returns prompt; `submit-execute`/`submit-review`/`submit-plan`/`submit-plan-review` are explicit submission verbs (no overloaded args) |
| 4 | Smoke-test target | Expand `observations` lifecycle to 10.06's full set: `investigating`, `confirmed`, `needs_info`, `in_progress` |
| 5 | ID prefix | `T{:03d}` shared with filesystem; T001-T002 stay legacy filesystem-only; T003 onwards are DB rows rendered to filesystem |
| 6 | `guard:` expression scope | Equality (`==`, `!=`) + `.length <`, `<=`, `>`, `>=`, `==` only; defer full AND/OR/inequalities for non-length comparisons |
| 7 | Capability fields | Bake `capability`/`sub_item`/`infra` into bundled tasks schema as optional. 10.06 YAML check stays in project-specific wrapper skill, not framework. |
| 8 | Concurrency lock | `claimed_by` / `claimed_at` with 5-min default timeout (releases on submit or expiry) |

### Read order if you've just opened this repo

1. `tasks/active/T002-tasks-store-v02/main.md` — the **current** Intent Contract + 10-phase plan (cycle-2 ratified). Authoritative.
2. `tasks/active/T002-tasks-store-v02/plan-review.md` — review history + plan-strengthening decisions.
3. `tasks/active/T002-tasks-store-v02/code-review-phase-{N}.md` — phase-by-phase audit trail (read in phase order).
4. `tasks/completed/T001-stores-framework-v01/main.md` — full v0.1 audit trail.
5. **This document** — original v0.2 framing + still-valid decisions and deferred bugs. Skip the superseded sections marked above.

---

## TL;DR

`stores` is a schema-driven store framework written in Rust. v0.1 just shipped: single binary CLI, two bundled stores (`observations` + `gate`), schema → CLI → SQLite → insert-time enforcement chain works end-to-end. **94 tests pass; 13-step DONE_WHEN demo passes from a fresh shell** via `tests/e2e.sh`.

The motivating problem: in 10.06, T3 observations throw away user context and require a fresh Claude Code session to do `/task:open`'s Intent Contract work. The framework's central enforcement pattern — `required_when: triage.verdict == 'T3'` on the contract record — captures the contract at the moment the user has context, so downstream work can drain from a queue without re-blocking.

**v0.2's marquee task is the `tasks` store** — the biggest port enabler. Once it ships, 10.06's `task:open`, `task:wrap`, `task:next` skills can move onto the framework, replacing the bespoke `tasks/` directory layout with schema-enforced typed entries.

---

## What you're picking up

**Repo state:** https://github.com/blakesims/stores (private, branch `master`).

**Test count:** 94 (as of `f70506b`). All passing.

**E2E:** `bash tests/e2e.sh` walks all 13 DONE_WHEN steps in a fresh `mktemp -d` and exits 0.

**Bundled stores:**
- `stores/observations/` — minimal lifecycle (open → triaged → resolved | wont_fix), Record-typed `triage` and `contract` fields, `contract` sub-fields gated by `required_when: triage.verdict == 'T3'`.
- `stores/gate/` — async question routing. Three states (pending → answered | cancelled). `answer` field is `actor: human`; everything else is unguarded (anyone can file a gate item).

**Bundled skill drafts** at `skills/` (NOT auto-installed; copy to `.claude/skills/` manually OR via `stores skills install --all`):
- `observation:log` — fast capture
- `observation:triage` — classify + (for T3) capture contract
- `gate:walk` — walk pending gate items with human at the helm
- `task:next` — drain queued tasks (forward-looking; tasks store doesn't exist yet)

**Status of skill drafts: known fictional in places** (see "Known issues" below). They assume `stores X schema --json` and `stores X list --status open --limit 1` which currently don't exist (T2B is in flight to add them).

**Top-level commands today:**
```
stores init                     # initialize .stores/db.sqlite + .stores/manifest.yaml in cwd
stores install <path>           # install a store from a folder containing schema.yaml
stores skills list              # list bundled skills (4)
stores skills install <name> [--all] [--global]
stores skills uninstall <name>
stores <store> add ...          # auto-generated per installed store
stores <store> show <id>
stores <store> list
stores <store> update <id> ...
stores <store> <transition-verb> <id> ...   # e.g. triage, resolve, answer, cancel
```

**Per-store flags include `--invoker {human|ai_autonomous|ai_with_human}` and `--json`.** Default invoker = env-detect (`$CLAUDECODE` set → `ai_autonomous`, else `human`). Invalid `--invoker` values are now rejected with a clear error (per `f70506b`).

---

## Read these files first (in this order)

1. **`README.md`** — install + 13-step demo path. The user-facing contract.
2. **`tests/e2e.sh`** — the byte-identical script that exercises the README. If anything ever drifts between these, e2e is the truth.
3. **`tasks/completed/T001-stores-framework-v01/main.md`** — full v0.1 task document: Intent Contract, 8-phase plan, every code review (cycles 1 + 2 where they happened), execution log per phase, completion summary, lessons learned. Decision Matrix at the bottom is the most concentrated source of "why this design, not that one."
4. **`tasks/completed/T001-stores-framework-v01/code-review-phase-{4,7,8}.md`** — the three reviews that surfaced real bugs. Phase 4 cycle 1 (Record sub-field destructive update), Phase 7 (transition actor scoping bug; `Op::TransitionWithDiff` introduction), Phase 8 (Op::Update carry-forward of the same fix).
5. **`findings/cli-smoke-2026-04-26.md`** — Tester A's empirical pass against v0.1 binary. **2 critical bugs surfaced.**
6. **`findings/skill-walkthrough-2026-04-26.md`** — Tester B simulated each bundled skill literally. **All 4 skills have fictional discovery prose.**
7. **`docs/handoff-v0.2.md`** — this file.

The `tasks/CLAUDE.md` document explains the task-workflow conventions used during the v0.1 build. v0.2 likely uses the same approach (it's effective; cycle-2 plan-review caught real architectural drift).

---

## Locked design decisions from v0.1 (do NOT relitigate)

These are signed-off; honor them in v0.2. Read the full Decision Matrix in `tasks/completed/T001-stores-framework-v01/main.md` for rationale.

| # | Decision | Choice |
|---|---|---|
| 1 | Actor model | Per-field tag + env detect + `--invoker` override |
| 2 | Identity | Hybrid: integer PK + display name (`OBS001`, `G001`) |
| 3 | DB topology | Single SQLite at `.stores/db.sqlite` |
| 4 | CLI shape | Verb-noun (`stores observations add`) |
| 5 | Async-first questions | Gate store for cross-context blockers; sync ask is a future UX wrapper |
| 6 | Language | Rust, single binary, `cargo install --path .` |
| 7 | Schema declaration | YAML at top, pydantic-style internal model |
| 8 | Two stores in v0.1 | `observations` + `gate` both ship |
| 9 | Observations lifecycle | Minimal (open|triaged|resolved|wont_fix) — expansion is v0.2 |

Plus from in-flight design conversations (record in main.md if you act on these):

- **Per-store storage scope** — currently per-cwd. v0.2 should support `scope: repo | worktree | user` with `repo` as the new default (resolves `.stores/` to the canonical `.git/` location via `git rev-parse --git-common-dir`). ~10 LOC in `paths.rs`. Discussed but not implemented.
- **Skills are NOT bundled into store-install** — they ship as separate suggestions. A skill declares `requires_stores: [...]` in frontmatter; framework can verify on load (not yet implemented).
- **`required_when` syntax** — locked at `lhs.dotted.path == 'literal'`. AND/OR/inequalities are deferred. **Note bug below in T2-FIX1's wake.**

---

## What just landed (post-v0.1)

All three fixers landed before this handoff was committed. Verify with `git log --oneline` in the repo.

| Commit | Description |
|---|---|
| `369a33c` | Skill drafts seeded under `skills/` (4 of them + README) |
| `60d2064` | T2A: removed over-restrictive `default_actor` from gate + observations; observations id_format → `OBS{:03d}` |
| `165c7c6` | T2C: `stores skills` subcommand family (list/install/uninstall, --all, --global) |
| `f70506b` | T2-FIX2: `--invoker` value validation; rejects unknown values |
| `c419a73` | T2-FIX1: transition actor enforcement — `ai_autonomous` correctly rejected for `ai_with_human` transitions |
| `3052df4` | T2B: `stores X schema [--json]`, list filters (`--status --sort --reverse --limit --since`), `stores list-installable`, `stores install <bundled-name>` shortcut |

**Test count at handoff:** 110 (was 90 at T001 PASS; +20 across the five post-v0.1 fixes).

**Bugs surfaced by Tester A and Tester B that are NOW CLOSED:** transition actor enforcement (was decorative); `--invoker zorblax` silent acceptance; `stores X schema` doesn't exist; `stores X list --filter` doesn't exist. All four resolved.

The skill drafts under `skills/` are still partially fictional — they reference correct commands but the prose may not match the actual help text or flag conventions. Revise them as the v0.2 first quick-win (~30 min).

---

## Known issues / open work

### Bugs deferred (not in flight; pick up if relevant to your work)

3. **`--tags "a,b"` doesn't pipe-split.** Per Tester A. Comma is treated as a literal value — result: `["a,b"]` not `["a", "b"]`. Pipe-split (`|`) works for `--options` (gate.options is a list<text>); the issue is whether the same pipe-split is applied to other list<text> fields. Tester says: not consistently. Fix: apply the pipe-split universally to `list<text>` flags. ~5 LOC.

4. **`--summary-from-file /missing/path` masks I/O error as `summary: required`.** Per Tester A. Should error explicitly with the missing path. ~10 LOC in handlers/{add,update}.rs.

5. **Per-verb `--help` lists every store field.** Doesn't filter by verb (e.g., `stores observations add --help` shows `--answer` because some other verb uses it). Cosmetic but confusing. Fix: filter `Arg::new(...)` to those relevant for the verb's `Op` shape.

6. **`required_when` parser substring-rejection of `OR`/`AND` keywords inside enum literals.** Earlier review (Phase 2 cycle 1) flagged this. The current heuristic `s.contains("OR")` would falsely reject `value == 'NORTH'`. Tester B didn't trip it (no enum values with `OR`/`AND` substrings in v0.1 schemas) but it's lurking. Fix: tokenize properly with whitespace-bounded keyword detection. ~15 LOC.

7. **Reserved-column-name leaf collision** (a user field named `status`/`id`/`display_id`/`created_at`) is caught only by SQLite's `duplicate column name` error at install time, not framework-level. Confusing UX; not a correctness bug. ~20 LOC to add a friendly check in `flatten.rs::leaf_args`.

8. **Lock primitive (`stores observations lock <id>`).** Doesn't exist yet. Concurrent triage agents could step on each other. SQLite WAL handles transactional writes; a row-level lock primitive would be additive. ~50 LOC + lock-table column. Defer until concurrent-agent triage actually exists.

### Skill drafts need revision (after T2-FIX1 + T2B)

All 4 bundled skills under `skills/` reference fictional CLI surfaces:
- Every skill's "discover via `stores X schema --json`" — works only after T2B.
- `observation:triage` says `--invoker ai_with_human` for the triage transition; reality (after T2-FIX1) should require `ai_with_human` exactly — confirm and update if the actor model changed.
- `gate:walk` mentions a `defer` verb; gate's lifecycle has only `cancel` today. Either add `defer` to the gate schema (transition pending → deferred?) or drop it from the skill prose.
- `task:next` is heavily forward-looking against the unbuilt tasks store. Once tasks ships, this skill becomes meaningful; until then keep it as a stub with a clear "tasks store not installed" branch.

Each skill is ~5 min revise once the surfaces it depends on are real.

---

## v0.2 priority list (suggested)

In rough order of value-per-effort. The user asked specifically about "the larger v0.2 task for the task workflow" — that's #2 below.

### 1. Quick wins (~1–2 hours total)

1a. Revise the 4 skill drafts to match the now-real surface (post T2-FIX1 + T2B).
1b. Expand observations lifecycle to match 10.06's: add `investigating`, `confirmed`, `needs_info`, `in_progress` states with appropriate transitions. ~30 LOC YAML + corresponding skill prose.
1c. Add `priority` (enum: high|normal|low) + `priority_rank` (int, nullable) + `priority_rank_at` (timestamp, nullable) fields to both observations and gate schemas. Required for `focus` skill. ~10 LOC each schema.

### 2. **`tasks` store** — the marquee v0.2 task

This is **a T3** by 10.06's rubric (capability change, multiple subsystems). **Plan it via `task:start` task-workflow**, don't just dive in. The plan-review cycle on T001 caught real architectural drift; same will happen here.

**What it enables:** porting `task:open`, `task:wrap`, `task:next` from 10.06. After this lands, the entire 10.06 task workflow can run on the framework.

**What's hard:**

- **Sub-document arrays as CLI surface.** Tasks have `phases: list<record>`, `execution_log: list<record>`, `code_review_log: list<record>`. The framework supports `list<record>` in storage (TEXT JSON column), but the dynamic CLI codegen flattens leaves only one level deep — you can do `--verdict T3` for a Record sub-field, but you can't address `phases[1].status = COMPLETE` via flat flags. Either:
  - (a) Introduce a `--phase 1 --phase-status COMPLETE` syntax (parameterized leaf addressing)
  - (b) Have a separate `stores tasks log <id> phase --name X --status COMPLETE` verb that targets a specific list<record> entry
  - (c) Treat sub-documents as opaque JSON blobs accepted via `--phases-from-file` (gives up validation per-sub-record)
  - This is the central design choice for the tasks store. Spend some time here.
- **Section ownership.** Tasks have section-owner semantics (planner writes Plan; executor writes Execution Log; code-reviewer writes Code Review Log). The current actor model is field-level. Either extend to per-section (named groups of fields) OR enforce via per-field actor declarations (verbose but works).
- **Parameterized states.** `EXECUTING_PHASE_N` where N is variable. The framework's `Lifecycle.states` is a static `Vec<String>`. Either (a) collapse to a single `EXECUTING` state with a `current_phase: int` field, or (b) add support for parameterized states. (a) is much simpler; recommend (a).
- **`list<fk>` field type.** `linked_observations: list<fk-to-observations>`. The framework's `FieldType::List` accepts `list<text>`, but typed-FK list isn't implemented. ~50 LOC framework addition.
- **Soft-FK validation.** When `task_ref` or `linked_observations` references something, validate the referent exists. Tester for v0.2 should drive this.

**Schema sketch (start point, refine in planning):**

```yaml
name: tasks
id_format: "T{:03d}"
fields:
  - name: title
    type: text
    required: true
  - name: slug
    type: text
    pattern: "^[a-z0-9-]+$"
  - name: capability
    type: text             # FK to capabilities store, but capabilities doesn't exist yet — treat as text for v0.2
  - name: linked_observations
    type: list_fk          # NEW field type; see "What's hard" above
    ref: observations
  - name: contract
    type: record
    fields:
      - {name: executive_intent, type: text}
      - {name: done_when, type: text, required: true}
      - {name: scope_in, type: text, required: true}
      - {name: scope_out, type: text, required: true}
      - {name: assumptions, type: text}
  - name: phases
    type: list_record      # see "What's hard"
    fields: [...]
  - name: execution_log
    type: list_record
    fields: [...]
  - name: code_review_log
    type: list_record
    fields: [...]
  - name: current_phase
    type: integer
    default: 0
lifecycle:
  states: [planning, plan_review, ready, executing, code_review, blocked, complete]
  initial_state: planning
  transitions:
    - {from: planning, to: plan_review, verb: submit_plan, actor: ai_autonomous}
    - {from: plan_review, to: ready, verb: approve_plan, actor: ai_with_human}
    - {from: plan_review, to: planning, verb: revise_plan, actor: ai_autonomous}
    - {from: ready, to: executing, verb: start, actor: ai_autonomous}
    - {from: executing, to: code_review, verb: submit_for_review, actor: ai_autonomous}
    - {from: code_review, to: executing, verb: revise, actor: ai_autonomous}     # next phase or revision; phases tracked in current_phase
    - {from: code_review, to: complete, verb: finish, actor: ai_autonomous}      # last phase passed
    - {from: any, to: blocked, verb: block, actor: ai_autonomous}
    - {from: blocked, to: ready, verb: resume, actor: ai_with_human}
```

(Don't implement this verbatim — let plan-review tear into it. Several rough edges: "from: any" not currently supported; whether `executing → code_review → executing` cycles correctly track phase transitions; etc.)

**Estimated effort:** 1500–2500 LOC framework + schema + skill ports. Multi-day. Plan it carefully.

### 3. `notes` store (after tasks or alongside)

Smaller than tasks. Replaces 10.06's `research/wXX-phase*/...md` worklog convention. Schema sketch:

```yaml
name: notes
id_format: "N{:03d}"
fields:
  - {name: slug, type: text, pattern: "^[a-z0-9-]+$", required: true}
  - {name: title, type: text, required: true}
  - {name: type, type: enum, enum_values: [note, summary, weekly, finding]}
  - {name: body, type: text}                          # supports --body-from-file
  - {name: linked_tasks, type: list_fk, ref: tasks}   # if tasks ships first
  - {name: linked_observations, type: list_fk, ref: observations}
lifecycle:
  states: [draft, published, archived]
  initial_state: draft
  transitions:
    - {from: draft, to: published, verb: publish, actor: ai_with_human}
    - {from: published, to: archived, verb: archive, actor: ai_autonomous}
```

Unblocks `note:new`, `intake`, `day:close`'s summary write, `observation:investigate`'s research note. ~80 LOC schema + minor framework if `list_fk` is needed.

### 4. `runs` store (provenance event log)

Append-only, immutable. Every CLI invocation writes one entry: `(timestamp, invoker, command, args, model, duration_ms, store, entry_id, outcome)`. Auto-instrumented by the CLI — no skill needs to call this; it just happens.

Schema sketch:

```yaml
name: runs
id_format: "R{:06d}"     # higher cardinality
append_only: true        # NEW property: no update/delete; only add
fields: [...]
```

Enables every audit / metric / analytics question across all other stores. JOINs against runs answer "how often does triage need a second pass," "average time-to-contract for T3s," etc.

### 5. `capabilities` store (lower priority)

The 10.06 `phase-1-capabilities.yaml` could become a capabilities store. But: the YAML-canonical form works fine today, and the read-side users (`focus`, `task:open` Stage 1.5) can read YAML directly. **Defer until either (a) there's a real benefit beyond what YAML gives, or (b) capabilities want their own lifecycle (status changes, audit trail).**

### 6. Per-store storage scope

`scope: repo | worktree | user` with `repo` as the new default. Resolves `.stores/` to canonical `.git/` location via `git rev-parse --git-common-dir`. ~10 LOC in `paths.rs` + schema field. Per-store opt-in. **Important for multi-worktree workflows.**

### 7. Soft-FK validation + `list_fk` field type

Required by tasks store (`linked_observations`). Could land before tasks if you want to test the FK approach in isolation; or fold it into the tasks PR.

---

## How to develop

```bash
# Clone:
git clone git@github.com:blakesims/stores.git ~/repos/experiments/stores
cd ~/repos/experiments/stores

# Build:
cargo build

# Test:
cargo test                        # unit tests
bash tests/e2e.sh                 # end-to-end demo from fresh tmp dir

# Install locally for hand-testing:
cargo install --path .            # puts `stores` on PATH

# Reinstall after a change:
cargo install --path .            # re-runs; replaces binary

# Quick interactive test:
TMP=$(mktemp -d) && cd $TMP
stores init
stores list-installable           # (after T2B lands)
stores install observations       # (after T2B lands; or use a path)
stores observations schema        # (after T2B lands)
```

Conventions:

- **Don't auto-commit until tests pass.** v0.1 was developed with the `task-workflow` plugin which has explicit cycle-tracked reviews. Even on smaller changes, `cargo build && cargo test` before commit.
- **Author lines:** `Blake Sims <blake.sims27@gmail.com>` + `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.
- **The orchestrator (you) doesn't write implementation code directly** if working through `/task:start`. Spawn `task-workflow:executor` and `task-workflow:code-reviewer`. For T2 work, executor + in-session review is fine; for T3, full plan + plan-review + execute → review per phase.

---

## What NOT to do (lessons from v0.1)

1. **Don't add v0.2 features without explicit user signoff** beyond the deferred list above. The user's energy is finite; design churn is expensive.
2. **Don't relitigate the locked decisions** — actor model, identity scheme, single SQLite, Rust, etc. They're locked for a reason.
3. **Don't over-engineer error handling for scenarios that can't happen.** Trust internal types; validate at system boundaries (CLI input, schema parsing, SQL).
4. **Don't add a process layer to the framework.** Skills (Claude Code) and personas (pi-agents) are external; the framework is data + enforcement only. The graph engine in pi-extension is orchestration; that doesn't migrate into the stores binary.
5. **Don't create new markdown documentation files unless asked.** This handoff is a deliberate exception.
6. **Don't commit findings/ in fresh test runs.** The directory is a scratchpad for tester reports; commit only when the user explicitly asks.

---

## Open questions to surface back to the user (don't decide alone)

- **Tasks store sub-document CLI surface** — three options sketched above (parameterized flags, dedicated `log` verb, opaque JSON). User has good design instincts on this kind of question; ask before committing.
- **Skill format convergence with pi-extension personas** — are skills and personas meant to share a file format eventually (one frontmatter key chooses runtime), or stay distinct? Discussed in v0.1; not decided.
- **Which 10.06 skill is the immediate target after tasks-store ships?** Probably `task:open`. Confirm.
- **`/run task-workflow` v.s. just `stores tasks ...`** — is the framework supposed to have its own task-workflow orchestration, or stay strictly data-only and let pi-extension/Claude Code handle orchestration?

---

## Pointers to all the artifacts

```
stores/
├── README.md                                       # user-facing demo
├── HANDOFF moved to docs/handoff-v0.2.md           # this file
├── Cargo.toml
├── src/                                            # ~4400 LOC Rust
│   ├── main.rs
│   ├── cli/                                        # dispatch, dynamic, init, install, skills
│   ├── handlers/                                   # add, show, list, update, transition, row
│   ├── schema/                                     # mod, types, parse, flatten, lifecycle, required_when, actor
│   ├── validate/                                   # mod, required, enum_check, regex_check, actor, error
│   ├── codegen/ddl.rs                              # SQLite DDL from Schema
│   ├── manifest.rs
│   ├── db.rs                                       # WAL + connection
│   ├── paths.rs                                    # .stores/ resolution
│   ├── output.rs                                   # text + --json formatters
│   └── id_format.rs                                # parse + render
├── stores/                                         # bundled stores
│   ├── observations/{schema.yaml, README.md}
│   └── gate/{schema.yaml, README.md}
├── skills/                                         # bundled skill drafts
│   ├── README.md
│   ├── observation:log/SKILL.md
│   ├── observation:triage/SKILL.md
│   ├── gate:walk/SKILL.md
│   └── task:next/SKILL.md
├── tests/
│   ├── e2e.sh                                      # full DONE_WHEN walk
│   └── fixtures/all_types_store/                   # exercises every FieldType
├── tasks/
│   ├── CLAUDE.md                                   # task-workflow conventions
│   ├── global-task-manager.md
│   ├── main-template.md
│   └── completed/T001-stores-framework-v01/        # full v0.1 audit trail
│       ├── main.md
│       ├── plan-review.md
│       └── code-review-phase-{1..8}.md
├── findings/                                       # tester reports — committed as audit-trail snapshots
│   ├── cli-smoke-2026-04-26.md
│   └── skill-walkthrough-2026-04-26.md
└── docs/handoff-v0.2.md                            # this file
```

---

## Final words

v0.1 works. The architecture holds up under real testing — the marquee enforcement moments (T3 contract gating; gate.answer human-only) sharply reject violations and cite the schema rule. The bugs surfaced by testing are real but addressable. The framework is small (~4400 LOC) and the schema language is genuinely expressive.

The user (Blake) has been through a long session and is conserving energy. **Be calm, write less, ship working code, don't pile on design.** When in doubt, ask.

Good luck.

— prior Claude

---

**P.S.** If you're using `task-workflow:start` for v0.2 work, expect plan-review cycle 1 to surface architectural drift on the tasks store sub-document CLI design — that's the load-bearing question. Don't try to slip it past the reviewer.
