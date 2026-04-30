# T007: Port the 10.06 `gate` store — first real migration

## Meta
- **Status:** PLANNING
- **Created:** 2026-04-30
- **Last Updated:** 2026-04-30
- **Blocked Reason:** —

## Task

10.06 has been hand-rolling its own gate primitive in `~/repos/clients/10.06-wt/10.06-main/issues/gate.json` for months. The bundled `stores/gate/schema.yaml` in this repo is a v0.1 minimal version of the same primitive (3 states, 6 fields). T007 is the first real migration: extend the stores `gate` until it can hold a 10.06 production gate row end-to-end, with all of 10.06's actual behaviors enforced by the schema rather than by `./dev gate`'s bash conventions.

This is the smallest 10.06 port and the first to exercise the v0.4.1 substrate (post-T006 cleanup) on production-shaped data. It's the pattern we'll use to port `observations` (T009) and `tasks` (later).

### What 10.06's production `gate.json` has that the bundled schema doesn't

Cross-referenced from `~/repos/clients/10.06-wt/10.06-main/issues/CLAUDE.md`, the audit's Pattern 2 (27 unranked pending gates), and `research/w11-phase6/w11-d4-22-stuck-data-audit.md`:

| Field / behaviour | 10.06 production | Bundled stores/gate (v0.1) | Gap |
|---|---|---|---|
| `id` | `G001`-style | `G001`-style | ✓ same |
| `type` | `decision \| script` | `decision \| script` | ✓ same |
| `one_liner` (question) | required text | `question: text` (required) | naming-only |
| `task_ref` | soft FK to `LNNN` / `TNNN` | `task_ref: text` | ✓ same |
| `options` | list text | list text | ✓ same |
| `answer` | text, `actor: human` | text, `actor: human` | ✓ same |
| `priority` (high/normal/low) | yes | `priority: enum` | ✓ same |
| **`priority_rank`** | int 1-5, set by `/focus` | — | **add (int)** |
| **`priority_rank_at`** | timestamp when rank written | — | **add (timestamp)** |
| **`defer_until`** | date, set when status transitions to `deferred` | — | **add (date)** |
| **`created_by`** | skill slug or agent name (`morning-check`, `task:wrap`, `investigator`, `phase-1-planner`, etc.) | — | **add (text, required)** |
| **`source`** | `dashboard \| qa \| dev \| converge \| wrap \| intake` | — | **add (enum)** |
| **`business_reason`** | proto-contract free text | — | **add (text)** |
| **`technical_detail`** | proto-contract free text | — | **add (text)** |
| **`command`** | for type=script: the shell command | — | **add (text)** |
| **`implications`** | proto-contract free text | — | **add (text)** |
| Lifecycle states | `pending \| answered \| deferred \| cancelled` (4) | `pending \| answered \| cancelled` (3) | **add `deferred` state** |
| Defer transition | `pending → deferred` with `--defer-until <date>` | — | **add transition** |
| Resume from defer | `deferred → pending` when `defer_until <= today` | — | **add transition** (manual or daily-sweep) |
| Dedup-on-add | `--one-liner X --task-ref Y` reuses existing pending GNNN if exact match; `--force-new` to bypass | — | **out of stores schema scope** (skill-level concern; record in DM) |
| `approval_invoker` | `agent` vs `blake` audit field on `answer` writes | — | already covered by stores' `--invoker` audit (no new field needed) |

### DONE_WHEN

A scripted "G075-shaped" gate trace runs end-to-end through `stores gate` and demonstrates **six production-shaped behaviors**:

1. **Add a fully-shaped gate** with `--type script --one-liner "Backfill Stripe customer_ids on sub_subscriptions" --task-ref T241 --created-by morning-check --source converge --command "psql ..." --business-reason "..." --technical-detail "..." --implications "..." --priority high` succeeds and emits `G001`.
2. **Defer**: `stores gate defer G001 --defer-until 2026-05-11` transitions `pending → deferred` and writes `defer_until=2026-05-11`. Status is now `deferred`.
3. **Resume**: `stores gate resume G001` transitions `deferred → pending` (verb represents "the date arrived, surface it again"). Idempotent — running on `pending` should be a no-op or a clear "already pending" message, not a state-corruption.
4. **Answer with AI invoker is rejected**: `CLAUDECODE=1 stores gate answer G001 --answer "yes" ` fails with the existing `actor: human` enforcement message.
5. **Answer with human invoker succeeds**: `stores gate answer G001 --answer "yes" --invoker human` transitions `pending → answered`.
6. **Repeatable list flags work for `options`** (Phase 4 of T006 verified the framework; this is the second integration check): `--options "yes" --options "no"` produces `["yes", "no"]`; `--options "yes|no"` produces the same.

All six demonstrated against a fresh `/tmp/t007-gate-port` tempdir; artefacts captured (CLI outputs + show --json snapshots). `cargo test --all` and `tests/e2e.sh` and `tests/drive_e2e.sh` and `tests/tasks_e2e.sh` all green (modulo the pre-existing CLAUDECODE / SIGPIPE failures already documented in T006). T005's drive smoke un-regressed.

### What's NOT in this task

- **Porting `./dev gate` itself** (the bash CLI). T007 stops at proving the stores `gate` schema can hold the 10.06 shape; the actual cutover (replacing `./dev gate` with `stores gate` in 10.06) is a separate task in the 10.06 repo.
- **Schema migration of existing `gate.json` data** — there's no automated importer; existing 10.06 gates stay in 10.06's gate.json until the cutover task ports them.
- **`/focus` rank-writing logic** — `priority_rank` is a plain int field set externally. T007 just makes the schema accept it. The rank-write skill stays in 10.06.
- **Dedup-on-add at the schema level** — no schema mechanism yet; record in Decision Matrix as "skill-layer concern" (`stores gate add` would naively create duplicates; the calling skill checks for matches first via `stores gate list --search`).
- **`/gate:sweep`** — separate skill, not stores work.
- **`stores gate guide` / `stores tasks <id> guide`** — already exist; not part of T007.
- **pi-ask-user integration** for `answer` field — listed in README "not in v0.3"; T012-ish.

### DONE_WHEN clauses mapped to phases (planner fills the rest)

- Clause 1 (full add): Phase ?? schema fields + add path
- Clause 2 (defer): Phase ?? lifecycle state + transition
- Clause 3 (resume): Phase ?? lifecycle transition + idempotent semantics
- Clause 4 (AI rejection): existing actor:human covers; just verify in integration
- Clause 5 (human accept): existing covers; verify in integration
- Clause 6 (repeatable options): existing T006-P4 covers; verify in integration

---

## Plan
_Planner agent fills this section._

### Objective
_What we're trying to achieve._

### Scope
- **In Scope:**
  - `stores/gate/schema.yaml` — extend with the new fields and the `deferred` state + transitions, OR
  - `stores/gate_1006/schema.yaml` — alternative POC pattern (mirrors `observations_1006/`); decide in Decision Matrix
  - Update or add to `tests/e2e.sh` — extend to cover the new defer/resume + new fields, OR write a separate `tests/gate_e2e.sh` that runs the 6-clause integration trace
  - Update `stores/gate/README.md` if it exists (or leave to documentation polish)
- **Out of Scope:**
  - Anything listed in `## Task` / "What's NOT in this task"
  - Cross-store guards (T010), Json field type (T008), observations port (T009)

### Phases
| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | _planner fills_ | _planner sets_ |

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

---

## Execution Log
_Executor agent fills this section per phase._

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

---

## Completion
_Final summary when task is complete._
