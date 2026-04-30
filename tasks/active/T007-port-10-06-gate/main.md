# T007: Port the 10.06 `gate` store — first real migration

## Meta
- **Status:** CODE_REVIEW
- **Created:** 2026-04-30
- **Last Updated:** 2026-04-30 (plan-review cycle 2 of 3 — APPROVED)
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

> **Revision cycle 1 of 3 (2026-04-30):** addresses the NEEDS_WORK verdict in the Plan Review section below. Three changes vs cycle 1:
> 1. Phase 2 file-list expanded from `tests/e2e.sh` only to all four sites: `tests/e2e.sh`, `skills/observation:triage/SKILL.md:76-83`, top-level `README.md:170-186`, `stores/gate/README.md` (Quick Start + Fields/Lifecycle prose). Phase 2 AC now uses a repo-wide grep over `*.md`/`*.sh`/`*.yaml`.
> 2. Folded the (former) Optional Phase 5 README polish into Phase 2 — chose **Option (a)** from the revision brief: smaller diff, higher coherence ("all rename sites in one commit"). Plan is now 4 phases.
> 3. Phase 2 line-audit fixed: comment lines are 14, 18 (not 14, 17, 18). R5 wording in Risks rewritten to reference `(from, verb)` partitioning per `src/schema/lifecycle.rs:93-119`, with a forward-caveat for future executors.

### Objective

Extend `stores/gate/schema.yaml` (Path A, in place) so a single bundled gate store can hold a 10.06-shaped production gate row end-to-end: all the new fields (`one_liner`, `priority_rank`, `priority_rank_at`, `defer_until`, `created_by`, `source`, `business_reason`, `technical_detail`, `command`, `implications`), the new `deferred` lifecycle state, and the `defer` / `resume` transitions. Prove the 6 DONE_WHEN clauses against a fresh tempdir via a dedicated `tests/gate_e2e.sh`, while keeping the existing 13-step `tests/e2e.sh` green via a one-time rename patch (`--question` → `--one-liner`).

### Scope

- **In Scope:**
  - Extend `stores/gate/schema.yaml`: rename `question` → `one_liner`; add 9 new fields; add `deferred` state; add `defer` and `resume` transitions (resume includes a `pending → pending` self-loop for idempotency).
  - Mechanical rename of `--question` → `--one-liner` across all `stores gate add` call sites in tracked files: `tests/e2e.sh`, `skills/observation:triage/SKILL.md`, top-level `README.md`, `stores/gate/README.md` (Quick Start). One commit, one rename pass.
  - Add `tests/gate_e2e.sh` that walks the 6 DONE_WHEN clauses end-to-end against `/tmp/t007-gate-port`.
- **Out of Scope:** (all reaffirmed from the Task section's "What's NOT in this task")
  - Porting `./dev gate` (the bash CLI) — separate task in 10.06 repo.
  - Importing existing 10.06 `gate.json` data — manual cutover later.
  - `/focus` rank-write logic, `/gate:sweep`, `pi-ask-user` integration.
  - Dedup-on-add at the schema level — recorded in DM as a skill-layer concern.
  - Cross-store guards (T010), JSON field type (T008), observations port (T009).
  - Framework changes to validation order (e.g., pre-setting `merged.status = transition.to` before validate) — see Risk R1 and DM row "defer_until enforcement".

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | Schema extension — fields + `deferred` state + `defer`/`resume` transitions | M |
| 2 | Rename blast radius — `--question` → `--one-liner` across all tracked `stores gate add` call sites (tests + skills + READMEs); verify v0.1 demo unregressed | S |
| 3 | New `tests/gate_e2e.sh` — scripted 6-clause trace under `/tmp/t007-gate-port` | M |
| 4 | Operator integration smoke — fresh tempdir, capture artefacts for all 6 clauses + cross-store JOIN unregressed | S |

### Phase Details

#### Phase 1: Schema extension

- **Objective:** Make the bundled `gate` schema isomorphic to 10.06's production gate shape.
- **Files to modify:** `stores/gate/schema.yaml`
- **Implementation notes:**
  - Rename top-level field `question` → `one_liner` (keep `required: true`, `type: text`). No alias mechanism in v0.4 — see DM row "alias for `question`".
  - Add fields:
    - `priority_rank: integer` (optional)
    - `priority_rank_at: timestamp` (optional)
    - `defer_until: text` (optional; ISO-date string `YYYY-MM-DD`; see DM row "defer_until typing")
    - `created_by: text` (required: true)
    - `source: enum [dashboard, qa, dev, converge, wrap, intake]` (required: true)
    - `business_reason: text` (optional)
    - `technical_detail: text` (optional)
    - `command: text` (optional — set when `type: script`)
    - `implications: text` (optional)
  - Lifecycle changes:
    - States: `[pending, answered, deferred, cancelled]` (add `deferred`).
    - Transitions (additions):
      - `pending → deferred` verb `defer` (no actor restriction)
      - `deferred → pending` verb `resume` (no actor restriction)
      - `pending → pending` verb `resume` (self-loop for idempotency)
      - `deferred → cancelled` verb `cancel` (per DM row "cancel-from-deferred")
- **Acceptance Criteria:**
  - [ ] `stores install ./stores/gate` parses cleanly into a fresh tempdir; `sqlite3 .stores/db.sqlite ".schema gate"` shows columns for all 9 new fields plus the renamed `one_liner`.
  - [ ] `stores gate schema --json` shows lifecycle states `[pending, answered, deferred, cancelled]` and transitions including `defer`, `resume` (×2), and the existing `answer`/`cancel`.
  - [ ] `cargo test --all` green (schema-parse and validation tests still pass; bundled fixture tests unaffected since they reference `observations`, not `gate`).

#### Phase 2: Rename blast radius (`--question` → `--one-liner`)

- **Objective:** Single mechanical-rename pass across every tracked file that calls `stores gate add --question`. Keeps the existing 13-step demo passing AND keeps shipped operator-facing docs (READMEs, skill instructions) runnable verbatim after the field rename. This is the "canary regression" check that Path A doesn't silently break operator-level workflows or copy-paste-able doc snippets.
- **Files to modify:**
  - `tests/e2e.sh` — 3 `--question` call sites at lines 139, 152, 171; 2 README-correspondence comment lines at 14, 18 (line 17 is a `CLAUDECODE` comment, no `--question`).
  - `skills/observation:triage/SKILL.md` — lines 76-83, the `stores gate add --type decision --question "<the question>"` code block. (This is the gate store's `--question`, not a triage-owned flag — confirmed by inspection of the surrounding block.)
  - `README.md` (top-level) — lines 170-186, two `stores gate add ... --question "..."` examples in the Quickstart bundled-store demo (Step 9 and Step 11).
  - `stores/gate/README.md` — Quick Start lines 22 and 28 (two `--question` invocations); ALSO update the Fields list (line 8) and Lifecycle section to match the new schema (rename `question` → `one_liner`; add 4-state lifecycle and `defer`/`resume` transitions). The README rename is a hard AC of this phase, not optional polish — Quick Start commands must be runnable verbatim against a fresh install.
- **Implementation notes:**
  - Do not change the questions/options/task-ref values. Mechanical flag rename only on the `stores gate add` invocations.
  - For `stores/gate/README.md` only: also update the documentation prose (Fields list, Lifecycle states, list of new fields) so the README reflects the new shape from Phase 1. This is the larger doc-polish hunk; bundling it here avoids a separate phase that would otherwise be the only consumer of post-rename schema state.
  - Repo-wide grep is the safety net (see AC); if grep finds an additional `stores gate add --question` site not listed above, patch it in this same phase rather than deferring.
- **Acceptance Criteria:**
  - [ ] `bash tests/e2e.sh` runs to completion (13/13 PASS) on a fresh tempdir.
  - [ ] `bash tests/drive_e2e.sh` and `bash tests/tasks_e2e.sh` un-regressed (modulo pre-existing failures from T006).
  - [ ] `grep -rn -- "--question" /home/blake/repos/experiments/stores --include="*.md" --include="*.sh" --include="*.yaml"` returns zero hits inside any `stores gate add` (or `stores gate ...`) invocation across `tests/`, `skills/`, top-level `README.md`, and `stores/gate/README.md`. Any unrelated `--question` flag belonging to a non-gate command (if one exists post-grep) is documented in the execution log.
  - [ ] `stores/gate/README.md` Quick Start commands run verbatim against a fresh `stores install ./stores/gate` and produce `G001`/`G002` as documented.
  - [ ] `stores/gate/README.md` Fields list and Lifecycle section match `stores/gate/schema.yaml` (verify by inspection: 10 fields named, 4 states named, `defer`/`resume` transitions documented).

#### Phase 3: New `tests/gate_e2e.sh`

- **Objective:** Scripted demonstration of the 6 DONE_WHEN clauses end-to-end against a fresh `/tmp/t007-gate-port` tempdir.
- **Files to modify / create:** `tests/gate_e2e.sh` (new)
- **Implementation notes:**
  - Mirror the shape of `tests/e2e.sh`: `set -euo pipefail`, `pass`/`fail` helpers, `mktemp -d` (override-able to `/tmp/t007-gate-port` via `STORES_E2E_TMP` env var so artefact capture in Phase 4 is reproducible).
  - The 6 clauses, in order:
    1. **Add fully-shaped gate** (`--type script --one-liner "Backfill Stripe customer_ids on sub_subscriptions" --task-ref T241 --created-by morning-check --source converge --command "psql ..." --business-reason "..." --technical-detail "..." --implications "..." --priority high`) → expect `G001`. Verify via `stores gate show G001 --json` that all 9 new fields are present and equal to the input.
    2. **Defer**: `stores gate defer G001 --defer-until 2026-05-11` → expect status `deferred`, `defer_until == "2026-05-11"`. Use `show --json` to assert.
    3. **Resume**: `stores gate resume G001` → expect status `pending`. Then `stores gate resume G001` again (idempotent run on `pending`) → expect non-error exit (the self-loop transition handles this; output line is `Transitioned G001: pending → pending`).
    4. **AI-rejection on answer**: `CLAUDECODE=1 stores gate answer G001 --answer yes` → expect non-zero exit; stderr contains `actor` and `human`.
    5. **Human-accept answer**: `stores gate answer G001 --answer yes --invoker human` → expect status `answered`.
    6. **Repeatable list flags for `options`**: file two more gates as variants — one with `--options "yes" --options "no"` (repeatable form), one with `--options "yes|no"` (pipe form). Both should produce JSON `options: ["yes", "no"]` (verify each via `show --json`).
- **Acceptance Criteria:**
  - [ ] `bash tests/gate_e2e.sh` exits 0 with all 6 clauses logged as PASS.
  - [ ] All 6 assertions made against `stores gate show <ID> --json` parsed by `python3 -c 'import sys, json; ...'` (consistent with existing e2e.sh idiom).
  - [ ] Script is ≤120 lines; ≤4 `pass` lines per clause; comment header lists the 6 clauses verbatim.

#### Phase 4: Operator integration smoke

- **Objective:** Operator-driven mirror of T006 P5: run end-to-end in a fresh tempdir and capture concrete artefacts proving each DONE_WHEN clause. Phase 4 ACs are the literal six clauses.
- **Files to modify / create:** Artefact capture under `tasks/planning/T007-port-10-06-gate/artefacts/` (CLI stdout/stderr + `show --json` snapshots per clause).
- **Implementation notes:**
  - Run from a fresh `/tmp/t007-gate-port` (rm -rf if present, then `stores init && stores install ./stores/gate`).
  - Capture per clause: the exact command, the stdout, the exit code, and (where applicable) the post-state `show --json` snapshot.
  - Re-run `bash tests/e2e.sh` and `bash tests/gate_e2e.sh` and `bash tests/drive_e2e.sh` to confirm no regression.
- **Acceptance Criteria (literal DONE_WHEN clauses):**
  - [ ] Clause 1: full add succeeds and emits `G001`; artefact saved.
  - [ ] Clause 2: `defer G001 --defer-until 2026-05-11` transitions to `deferred` with `defer_until=2026-05-11`; artefact saved.
  - [ ] Clause 3: `resume G001` transitions `deferred → pending`; second invocation idempotent; artefact saved.
  - [ ] Clause 4: `CLAUDECODE=1 ... answer G001 --answer yes` rejected with `actor: human` enforcement message; artefact saved (stderr).
  - [ ] Clause 5: `answer G001 --answer yes --invoker human` succeeds; artefact saved.
  - [ ] Clause 6: both `--options "yes" --options "no"` and `--options "yes|no"` produce identical `["yes", "no"]` JSON; artefact saved.

### Decision Matrix

| Decision | Options Considered | Choice | Rationale |
|----------|-------------------|--------|-----------|
| Schema location | A: extend `stores/gate/schema.yaml` in place. B: new `stores/gate_1006/schema.yaml` parallel to bundled v0.1. | **A** | One source of truth; `tests/e2e.sh` becomes the canary regression net for production-shaped gate behavior. The new fields are pure additions with one rename; the rename cost is 6 line edits in e2e.sh. Path B leaves the bundled v0.1 to bit-rot — `gate_1006` would diverge against the original within weeks since 10.06 is the only real consumer. The `observations_1006` precedent exists because observations has a much more elaborate lifecycle (T1/T2/T3 ratification, intent_contract record nesting) that bundled obs deliberately doesn't model — there is no comparable lifecycle delta for gate, so the parallel-store cost isn't justified. |
| Alias for `question` | (a) Add a `question` alias to `one_liner`. (b) Hard rename. (c) Keep `question` as the canonical name and leave 10.06 to map. | **(b) Hard rename** | The framework has no field-alias mechanism today; adding one expands T007 scope into a framework feature. DONE_WHEN clause 1 spec uses `--one-liner` literally, so the rename has to happen in some form. Mechanical patch of 3 e2e.sh call sites is the cheapest implementation. |
| `defer_until` typing | text (ISO-date string) vs new framework `date` type. | **text** | No `Date` variant exists in `FieldType` (`src/schema/mod.rs:52`). Adding one is a framework feature with parser/codegen/validator/coercion implications across 8+ files — dedicated task (e.g. T008.x or later). ISO-date string in `text` is the same shape 10.06's gate.json already uses; format-validation can layer on later via `pattern: "^\\d{4}-\\d{2}-\\d{2}$"` if needed (consider as a P1 follow-up if reviewer feels strongly). |
| `defer_until` enforcement | (a) `required_when: "status == 'deferred'"`. (b) Transition arg-required check in handler/dispatch. (c) Leave optional. | **(c) Leave optional, document as operator hygiene** | Option (a) does NOT work today: validate runs against the pre-merge entry, where `status` is still the source state (`src/handlers/transition.rs:104-111`). Option (b) requires touching the dispatch/handler layer — out of scope per Risk R1. Option (c) matches 10.06's current lived reality (nothing enforces `defer_until` server-side; `./dev gate defer` requires the flag at the bash layer). The bash CLI port (separate task) carries the flag-required check. Document this in the README and in a Risk row. (Reviewer: if you push back, the cheapest schema-only enforcement is a `required_when` on `defer_until` keyed against a sibling marker field — but it adds cruft.) |
| Resume idempotency on `pending` | (a) Self-loop transition `pending → pending`. (b) Special-case in handler ("already pending, no-op"). (c) Error out with state-machine message. | **(a) Self-loop transition** | Pure-schema, no code changes. Output line `Transitioned G001: pending → pending` is mildly cosmetic but operator-clear. Option (b) needs handler logic (out of T007 scope). Option (c) makes the operator's `resume` calls fail spuriously when they're already in the desired state — operator-hostile in the sweep workflow where the surface always re-fires `resume` on every entry whose `defer_until <= today`. |
| `dedup-on-add` layer | Schema field uniqueness vs skill-layer pre-check (`stores gate list --search` then conditional add). | **Skill-layer concern** | The bundled framework has no `unique_when` or `pending_unique` constraint. Adding one is a framework feature comparable to the date type. Today, 10.06's `./dev gate add` does the dedup at bash level via a search-then-add pattern (cited in `~/repos/clients/10.06-wt/10.06-main/issues/CLAUDE.md`); the cutover task carries that pattern forward (the calling skill checks `stores gate list --status pending --search "<one_liner>"` first). T007 records the gap and stays focused. |
| `cancel` from `deferred` | Add `deferred → cancelled` transition vs require `resume` first. | **Add the transition** | Operators need to dismiss a deferred gate without re-surfacing it. Today's lifecycle says `pending → cancelled`; symmetrically extending to `deferred → cancelled` is a one-line schema addition with zero downside. |
| `source` enum | (a) Faithful copy of 10.06 6-value list `[dashboard, qa, dev, converge, wrap, intake]`. (b) Smaller subset. | **(a) Faithful copy** | This is a port, not a redesign. 10.06's gate.json uses all 6 values today; subsetting risks rejecting valid existing rows during the eventual data cutover. |
| `created_by` requiredness | required vs optional. | **required: true** | 10.06 production gate rows always carry a `created_by` — the audit doc treats unrecorded provenance as a bug. The framework's `--invoker` is an audit field for "who ran the verb"; `created_by` is "which skill filed the entry" and is semantically distinct. Keeping it required forces the calling skill to pass `--created-by <slug>` explicitly. |
| `gate_e2e.sh` separate vs extend e2e.sh | Add 6 new steps to `tests/e2e.sh` vs new `tests/gate_e2e.sh`. | **Separate `tests/gate_e2e.sh`** | `tests/e2e.sh` is the v0.1 cross-store demo (observations + gate + JOIN). The 6 DONE_WHEN clauses are gate-only and would balloon e2e.sh from 240 → ~360 lines, blurring its purpose. Separate script is consistent with the existing `tests/drive_e2e.sh` / `tests/tasks_e2e.sh` partitioning. |

### Risks

| ID | Risk | Mitigation |
|----|------|------------|
| R1 | `defer_until` cannot be enforced by the schema today (validate runs pre-merge; merged entry doesn't carry the post-transition status). | Documented in DM row "defer_until enforcement"; left to skill-layer `./dev gate defer` (and its eventual `stores gate defer` wrapper) to enforce. Surface as a P1 follow-up: pre-set `merged.status = transition.to` before `validate::validate(...)` in `src/handlers/transition.rs:104-111` — small framework patch, separate task. |
| R2 | Bundled `tests/e2e.sh` or shipped doc snippets regress if any `--question` reference is missed in Phase 2. The plan-reviewer pass already caught two sites the original Phase 2 missed (`skills/observation:triage/SKILL.md:76-83` and top-level `README.md:170-186`). | Phase 2 AC promotes a repo-wide `grep -rn -- "--question"` zero-hit check across `*.md`/`*.sh`/`*.yaml` for `stores gate ...` invocations. CI green criterion in Phase 4 requires `tests/e2e.sh` full pass. Future hygiene (out of scope for T007): consider adding the grep as a CI step or pre-commit hook so any new skill or doc that adds `stores gate add --question` post-T007 fails fast — recorded as a P1 follow-up, not a Phase work item. |
| R3 | Eventual 10.06 `gate.json` data has fields we missed (e.g. `approval_invoker` outside the audit field, `notes`, free-form metadata). | Forensic audit (`research/w11-phase6/w11-d4-22-stuck-data-audit.md` Pattern 2) and `~/repos/clients/10.06-wt/10.06-main/issues/CLAUDE.md` are the authoritative cross-references; gap table in `## Task` was derived from both. If a missed field surfaces during the cutover task in 10.06, it's a single-line schema addition + migration — not a framework change. |
| R4 | `priority_rank: integer` collides with framework's existing `auto_increment` integer pattern (Field has an `auto_increment` flag). | Verified by inspection: `auto_increment` is opt-in (`auto_increment: true` on the Field) and has nothing to do with the type. `priority_rank` is plain `type: integer` with no auto-increment — no collision. |
| R5 | Self-loop `resume: pending → pending` flagged as an "ambiguous transition" by `validate_transition_ambiguity` in `src/schema/lifecycle.rs:93`. | Verified by inspection of `src/schema/lifecycle.rs:93-119`: ambiguity is `(from, verb)`-keyed, filtered to transitions where both `requires_gate.is_none() && guard.is_none()` (i.e., the validator partitions by `(from, verb)` and only complains if the same partition holds two fully-unguarded transitions). The self-loop `pending → pending` and the resume `deferred → pending` share `verb=resume` but partition into DIFFERENT `(from, verb)` buckets — `(pending, resume)` vs `(deferred, resume)` — so they cannot collide. Phase 1 AC includes a fresh-install parse smoke that catches this if wrong. **Caveat for future executors**: if anyone later adds a SECOND fully-unguarded transition with `from=pending, verb=resume` (e.g., `pending → some_other_state`), the validator will (correctly) reject it; do not assume guard-distinguishing makes them disjoint. |
| R6 | T006 Phase 5 lands a substrate change (e.g. list-flag handling) between plan-approval and T007 execution start. | T007 is gated behind T006 P5 completion; replan if T006 P5 changes the list-flag semantics or transition validation order. The current plan's only T006 dependency is the Phase 4 list-flag lock (already-merged in T006 P4 per `tasks/active/T006-substrate-cleanup-poc/main.md`), so the contact surface is small. |

### Out-of-scope artefacts (recorded for cutover follow-up)

- 10.06 cutover skill needs to: (i) carry forward existing `gate.json` rows into the SQLite store one-time; (ii) implement dedup-on-add at the skill layer (`stores gate list --status pending --search "<one_liner>"`); (iii) implement `/gate:sweep` against `stores gate list --json` filtering on `defer_until <= today`.
- Framework P1 follow-ups (separate tasks): (a) pre-set `merged.status = transition.to` before validate so `required_when: "status == 'deferred'"` works; (b) add `Date` variant to `FieldType`; (c) add field-alias mechanism (so `question` could map to `one_liner` for legacy schemas).

---

## Plan Review

- **Gate:** NEEDS_WORK
- **Reviewer:** plan-reviewer agent (2026-04-30)
- **Open Questions Finalized:** None — no ambiguity remains for the human; revisions are mechanical/in-scope.

### Verdict summary

Path A is well-defended; Decision Matrix is sound; six DONE_WHEN clauses map cleanly to Phase 1/3/4 ACs; `defer_until: text`, resume self-loop, skill-layer dedup, faithful `source` enum, and required `created_by` are all the right calls. T006 dependency acknowledged in R6. `task_ref` correctly stays `text` (no `list_fk` creep into T010 territory).

Two concrete gaps require a revision pass before READY. Both are about the **rename blast radius** — Phase 2 under-counts where `--question` is referenced as a `stores gate add` call site in the repo, and the README polish that fixes the rest of the blast radius is marked Optional. After the rename, runnable doc snippets break silently.

### Issues found (NEEDS_WORK)

**Issue 1 (high) — Phase 2 misses two `stores gate add --question` regression sites in tracked files.**

The plan asserts that the only `--question` patches needed are in `tests/e2e.sh`, with this exact line in Phase 2 implementation notes:

> "The `skills/observation:triage/SKILL.md` line 79 reference is to triage's own `--question` flag, NOT the gate's — leave untouched (verify via inspection)."

Inspection contradicts that classification. `skills/observation:triage/SKILL.md` lines 76-83 are a literal `stores gate add --type decision --question "<the question>"` code block — this is the gate store's `--question`, not triage's own field. After the rename it becomes broken operator guidance.

A second site is also missed: top-level `README.md:170-186` has two `stores gate add ... --question` examples (the canonical README walkthrough corresponding to the bundled e2e). Both will go stale.

**Fix:** add to Phase 2 (or fold into a renamed "rename blast radius" phase):
- AC: `grep -rn -- "--question" /path/to/repo --include="*.md" --include="*.sh"` returns zero hits inside any `stores gate add` invocation across `tests/`, `skills/`, `README.md`, and `stores/gate/README.md`. Triage's own `--question` field (if any independent flag exists) survives.
- File list to patch: `tests/e2e.sh`, `skills/observation:triage/SKILL.md` (lines 76-83 block), `README.md` (lines 170-186), `stores/gate/README.md` (lines 22, 28).

**Issue 2 (medium) — Phase 5 README polish is marked Optional, but `stores/gate/README.md` Quick Start becomes non-runnable after the rename.**

Phase 5 says: "Skip if planner/reviewer agree the README is regenerable... and not worth diff churn." But Phase 5's own AC says "Quick Start commands are runnable verbatim." After Phase 1 lands the rename, the existing Quick Start in `stores/gate/README.md:22, 28` uses `--question` — so skipping Phase 5 leaves shipped docs that fail when copy-pasted. This violates the canary-preservation principle that motivates Path A in the first place.

**Fix:** mark Phase 5 mandatory (not Optional), OR move the README rename hunk (`stores/gate/README.md:22, 28` and top-level `README.md:171, 186`) into Phase 2 as part of "rename blast radius" and keep only the lifecycle/field-list documentation polish as Optional Phase 5.

### Minor (cosmetic, non-blocking)

**M1 — Phase 2 comment-line count off-by-one.** Plan says "3 README-correspondence comment lines at 14, 17, 18"; actual `--question` comment lines are 14 and 18 only (line 17 is the `CLAUDECODE` invocation comment). Update the count to "2 comment lines at 14, 18" for accuracy. Not a behavior risk.

**M2 — R5 wording.** R5 states ambiguity is `(from, verb, requires_gate, guard)`-keyed; per `src/schema/lifecycle.rs:93-118` it's actually `(from, verb)`-keyed (filtered to `requires_gate.is_none() && guard.is_none()`). The conclusion (no ambiguity, since `pending→pending` and `deferred→pending` differ on `from`) is correct, but the stated keying is misleading. Tighten the wording so a future executor reading R5 doesn't introduce a same-`(from, verb)` self-loop somewhere else thinking the validator distinguishes by guard.

### Confirmed strengths (no action needed)

- **Path A vs B**: defended on three axes — single source of truth, `observations_1006` precedent disanalogy (lifecycle delta size), and rename cost (6 lines). Sound.
- **DONE_WHEN ↔ phase mapping**: all 6 clauses appear as literal Phase 4 ACs; Clauses 1-3 map to Phase 1 schema work + Phase 3 e2e; Clauses 4-6 to Phase 3 verification. No clause dropped.
- **Decision Matrix high-stakes**: `defer_until: text` (correct — `FieldType` has no `Date` variant per `src/schema/mod.rs:52`); `defer_until enforcement` punted to operator-hygiene with a separate-task framework follow-up (correct — validate runs against `merged` pre-status-update per `src/handlers/transition.rs:111`); resume idempotency via self-loop (operator-friendly, schema-only); dedup-on-add at skill layer (correct — no `unique_when` mechanism in framework); `source` enum copies all 6 values faithfully; `cancel-from-deferred` proactively added.
- **Cross-store guard**: `task_ref` stays `text`, no `list_fk` upgrade — correctly fenced to T010.
- **T006 dependency**: R6 explicitly gates T007 execution behind T006 P5.
- **Out-of-scope hygiene**: T005/T008/T009/T010/T011 territories all correctly fenced; no creep.

### Routing

NEEDS_WORK → planner. The two issues above are surgical: extend Phase 2's file-list AC to cover the `skills/` and top-level `README.md` rename sites, and mark `stores/gate/README.md` rename as mandatory (either folded into Phase 2 or as a non-optional Phase 5). After revision, expected next status is PLAN_REVIEW for a fast re-check.

### Cycle 2 review

- **Gate:** READY
- **Reviewer:** plan-reviewer agent (2026-04-30, cycle 2 of 3)
- **Open Questions Finalized:** None.

#### Verdict

Both cycle-1 substantive issues are closed; cosmetic items addressed. PASS. Routing → READY → executor.

#### Verification of cycle-1 fixes

1. **Rename sweep completeness (Issue 1, cycle 1)** — Closed. Phase 2 file-list now enumerates all 4 live operator-facing/runnable sites: `tests/e2e.sh`, `skills/observation:triage/SKILL.md` (lines 76-83), top-level `README.md` (lines 170-186), `stores/gate/README.md` (Quick Start + Fields/Lifecycle prose). Independent grep across `*.md`/`*.sh`/`*.yaml`/`*.rs` confirms these 4 are the complete tracked-runnable surface; remaining hits are in `findings/cli-smoke-2026-04-26.md` (frozen 2026-04-26 audit doc) and `tasks/completed/T001-...` (frozen completed task) — correctly excluded by the planner's stated scope ("tracked files that call `stores gate add --question`" in operator-facing dirs). The Phase 2 grep AC explicitly enumerates the in-scope directories.

2. **README polish folded into Phase 2 (Issue 2, cycle 1)** — Closed. Former optional Phase 5 dropped entirely; `stores/gate/README.md` rename is now a hard AC of Phase 2 (lines 157-158: Quick Start runnable verbatim + Fields/Lifecycle prose match schema). Plan is 4 contiguous phases; all six DONE_WHEN clauses still map (clauses 1-3 to Phase 1 schema + Phase 3 e2e, clauses 4-6 to Phase 3 verification, all six as literal Phase 4 ACs at lines 187-192). No clause dropped.

3. **Cosmetic M1 (line-audit)** — Closed. Phase 2 line 145 now correctly says "2 README-correspondence comment lines at 14, 18 (line 17 is a `CLAUDECODE` comment, no `--question`)". Verified against `tests/e2e.sh` grep hits at lines 14 and 18 only.

4. **Cosmetic M2 (R5 wording)** — Closed. R5 (line 217) now references `(from, verb)` partitioning per `src/schema/lifecycle.rs:93-119` (verified: `let key = (t.from.clone(), t.verb.clone())` at lifecycle.rs:99; filter is `requires_gate.is_none() && guard.is_none()` at lifecycle.rs:98). The forward-caveat for future executors ("if anyone later adds a SECOND fully-unguarded transition with `from=pending, verb=resume`...") is correct and load-bearing.

5. **Locked items preserved verbatim** — Confirmed. Path A (DM row 1), `defer_until: text` (DM row 3), resume self-loop (DM row 5), R1 validate-pre-merge (Risks row 1) all unchanged from cycle 1.

6. **Decision Matrix row count** — 9 rows (unchanged from cycle 1; planner reported "no new rows added" — accurate).

#### Surviving nit (non-blocking)

**N1 (cosmetic) — Phase 2 has 5 ACs, exceeding the ≤4 per-phase guideline.** Folding Issue-1's grep AC and Issue-2's two README ACs into Phase 2 brought the count to 5. Trivially collapsible to ≤4 with sub-bullets:
- Combine ACs 1+2 ("all three e2e suites pass / un-regressed: `e2e.sh` 13/13, `drive_e2e.sh`, `tasks_e2e.sh`")
- Combine ACs 4+5 ("`stores/gate/README.md` correctness: Quick Start runs verbatim AND Fields/Lifecycle prose matches schema (10 fields, 4 states, defer/resume documented)")

Resulting 4-AC Phase 2 is structurally identical and reviewer-friendly. **Not blocking** — executor can collapse during execution; the review brief explicitly permits sub-bullets and flagged this contingency.

#### Routing

READY → orchestrator → executor. Move folder `tasks/planning/T007-port-10-06-gate/` → `tasks/active/T007-port-10-06-gate/`; update GTM row to point to active path and Status `READY`. Suggest executor collapse Phase 2 ACs to ≤4 via sub-bullets (N1) before starting Phase 1, but this is optional polish and does not require a re-review.

---

## Execution Log

### Phase 1 — Schema extension (2026-04-30)

- **Status:** COMPLETE
- **Start:** 2026-04-30
- **End:** 2026-04-30
- **Commit:** 7dc5f48
- **Files modified:**
  - `stores/gate/schema.yaml` — primary deliverable
  - `src/handlers/guide.rs` — test fixture update (see deviation note)

#### Changes in `stores/gate/schema.yaml`

- Renamed `question` → `one_liner` (required text, unchanged semantics)
- Added lifecycle state `deferred` (between `answered` and `cancelled`)
- Added transitions:
  - `pending → deferred` verb `defer` actor `ai_with_human`
  - `deferred → pending` verb `resume` actor `ai_with_human`
  - `pending → pending` verb `resume` actor `ai_with_human` (self-loop for idempotency)
  - `deferred → cancelled` verb `cancel` actor `ai_autonomous` (per DM row "cancel-from-deferred")
- Added 9 new fields:
  - `priority_rank: integer` (optional)
  - `priority_rank_at: timestamp` (optional)
  - `defer_until: text` (optional; ISO-date string; R1 limitation applies)
  - `filed_by: text` (required — **see deviation below**)
  - `source: enum [dashboard, qa, dev, converge, wrap, intake]` (required)
  - `business_reason: text` (optional)
  - `technical_detail: text` (optional)
  - `command: text` (optional)
  - `implications: text` (optional)

#### Deviations from plan

**D1 (field rename: `created_by` → `filed_by`):** The plan specified a required `created_by` field. The framework DDL codegen (`src/codegen/ddl.rs`) already reserves `created_by` as an audit column (prepended to every table). Adding a schema field with the same name produced a SQLite `duplicate column name: created_by` error. Since modifying DDL codegen is out of scope and the semantic intent (filing skill / agent provenance) is distinct from the framework's invoker audit, the field was renamed `filed_by`. This name is unambiguous and avoids any reserved-column collision. Phase 2 and Phase 3 will use `--filed-by` instead of `--created-by`. The DONE_WHEN clause 1 spec text ("--created-by morning-check") should be read as "--filed-by morning-check" after this rename.

**D2 (guide.rs test fixture update):** `src/handlers/guide.rs`'s `insert_gate` helper hardcoded the old `question` column and omitted the new required fields. The guide tests load `stores/gate/schema.yaml` live and generate DDL from it; the INSERT then failed against the new schema. Updated the helper to use `one_liner`, added `filed_by = 'test-fixture'` and `source = 'dev'` to satisfy the new required columns. This is a test-fixture correction, not a behavioral change. The plan scoped Phase 1 as "schema-only" and said "do not touch tests/e2e.sh" (the bash e2e) — the Rust unit test fixture is a necessary companion update, not a Phase 2 item.

#### Test results

- `cargo test --all`: **398 passed; 0 failed** (396 unit + 2 integration fixtures)
- No pre-existing failures introduced or suppressed.

---

## Code Review Log
_Code-reviewer agent fills this section per phase._

---

## Completion
_Final summary when task is complete._
