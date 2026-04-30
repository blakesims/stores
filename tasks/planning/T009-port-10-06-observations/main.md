# T009: Port the 10.06 `observations` store — second real migration

## Meta
- **Status:** PLANNING
- **Created:** 2026-04-30
- **Last Updated:** 2026-04-30
- **Blocked Reason:** —

## Task

10.06's production observations primitive lives in `~/repos/clients/10.06-wt/10.06-main/issues/ledger.json` — 275+ rows, ~30 fields, the busiest store in the 10.06 workflow. The bundled `stores/observations/schema.yaml` in this repo is a v0.1 minimal version (4 fields + triage/contract records, 7-state lifecycle). The L275 POC schema (`stores/observations_1006/`) is a closer-to-production approximation but was always named with the `_1006` suffix as a proof-of-concept, not the production schema.

T009 is the second real migration (T007 was `gate`): converge the bundled `observations/` to match 10.06 production end-to-end, validate against a real L-id-shaped trace, and either retire the `observations_1006/` POC schema or document why it stays as a separate fixture. This exercises everything T006 / T007 / T008 built (transition guards, list_record write/read parity, hyphenated identifier escaping, repeatable list flags, Json field type) on the busiest store.

### What 10.06 production has

Cross-referenced from `~/repos/clients/10.06-wt/10.06-main/issues/CLAUDE.md` and the audit's stuck-data patterns. The 10.06 observation has:

| Field / behaviour | 10.06 production | Bundled v0.1 | POC `observations_1006/` | Gap (production vs latest in-repo) |
|---|---|---|---|---|
| `id` | `LNNN` | `OBS{:03d}` | `LNNN` | bundled is wrong shape; POC is right |
| Lifecycle states | `[open, investigating, confirmed, needs_info, in_progress, resolved, wont_fix]` (7) | full 7-state | slimmed 5-state | extend POC OR converge from bundled |
| `summary` | required text | required text | required text | ✓ |
| `body` | optional text | optional text | — | add to POC if Path B |
| `source` | enum `[dashboard, qa, dev, sentry, intake, converge, wrap]` | — | enum (same) | bundled lacks |
| `source_id`, `prod_source_id`, `sandbox_source_id` | int (dashboard sync) | — | — | add |
| `origin_db` | enum `[prod, sandbox]` | — | — | add |
| `priority`, `priority_rank`, `priority_rank_at`, `scheduled_for` | high/normal/low + int + timestamps + date | — | partial | add (priority_rank, scheduled_for, priority_rank_at) |
| `contact_id`, `field_name` | int, text | — | int, text | ✓ on POC |
| `captured_at`, `captured_week` | text, text | — | text | add captured_week |
| `qa_item_id`, `tour_session_id`, `step_index`, `staff_user_id`, `message` | qa-source dedup keys | — | — | add (qa-source fields) |
| `capability`, `capability_ids` | text, list[text] | — | — | add (Phase 1 dashboard surface) |
| `investigation_note`, `resolved_at`, `resolution` | text, text, text | — | text, text, text | ✓ on POC |
| `task_id` | text (soft FK to `TNNN`) | — | text | ✓ on POC |
| `locked_by`, `locked_at`, `lock_reason` | text + timestamp + text | — | — | add (concurrency primitive) |
| `intent_contract` (record, ~14 sub-fields) | full | minimal `triage`/`contract` | full (matches T006 work) | ✓ on POC |
| `evidence` (record with `external_refs: list_record`) | full | — | full | ✓ on POC |
| `notes` (Json) | full | — | T008 added | ✓ on POC |

Lifecycle transitions are richer in 10.06 than in either the bundled v0.1 or the POC — full state machine has paths like `confirmed → in_progress` (claim), `in_progress → resolved` (resolve), `confirmed → wont_fix` (wont_fix), `needs_info → confirmed` (provide_info, actor=human). 10.06 also has `confirmed → needs_info` parking. Bundled v0.1 has the 7 states but not all transitions; POC has only 5 states.

### Path A vs Path B (planner decides)

- **Path A — extend `stores/observations/schema.yaml` in place.** Bundled becomes production. `tests/e2e.sh` (the v0.1 demo) becomes the canary regression net. The POC `observations_1006/` is retired or documented as a fixture. Pros: single source of truth; e2e catches regressions; converges naming. Cons: e2e.sh demo needs updates (new required fields). The `OBS{:03d}` ID format must change to `L{:03d}` — this IS a breaking change for anyone who's stored data under the old format (no current users; framework is pre-1.0).
- **Path B — make `stores/observations_1006/` the production schema; bundled `observations/` stays as the v0.1 demo.** Pros: bundled e2e stays untouched; POC becomes the production shape with no rename. Cons: long-term maintenance of two parallel schemas; the `_1006` suffix is permanent and confusing.

Recommended default: **Path A**. The convention set by T007 (extend bundled gate in place) is the right precedent. The POC was always meant to be a stepping stone; T009's job is to retire it.

### DONE_WHEN

A 10.06-shaped observation trace runs end-to-end through `stores observations` and demonstrates **eight production-shaped behaviors**:

1. **Add a fully-shaped observation** (dashboard-sourced T3 with full intent contract): `--type work` style content with all production fields populated → `L001` returned.
2. **Triage flow**: `open → investigating → confirmed` with the `intent_contract` record gradually filled. The `confirmed` transition's guard requires the contract is `ready`.
3. **Required_when on contract sub-fields** (T006 P1 substrate): tier_hint=T3 contract fails to flip to `ready` until `done_when` / `scope_in` / `scope_out` are filled.
4. **Per-field `actor: human`** (T006 P1): `approved_by`/`approved_at` reject AI invokers.
5. **`evidence.external_refs` round-trips as JSON array** (T006 P2): structured array, not a quoted blob.
6. **`notes` round-trips as structured JSON** (T008): operator-readable subkeys, not a quoted blob.
7. **`needs_info` parking + `provide_info` resume**: `confirmed → needs_info → confirmed` with `actor: human` on the resume (operator answers the gap question).
8. **Cross-store `task_id` reference** (Phase 1 of T002 / T006): observations soft-FK to a `tasks` row by display_id; `stores tasks <id> show` finds the task linked to the observation.

All eight demonstrated against a fresh `/tmp/t009-obs-port` tempdir; artefacts captured (CLI outputs + `show --json` snapshots). `cargo test --all`, all e2e suites green (modulo pre-existing CLAUDECODE/SIGPIPE failures already documented).

### What's NOT in this task

- **Importing 275+ existing 10.06 ledger rows** — no automated migration. T009 ships the schema; cutover is a separate task in the 10.06 repo when ready.
- **The dashboard sync logic** (`./dev observation sync` pulls from prod/sandbox APIs) — that's a 10.06-specific external integration.
- **`/focus` rank-writing** — `priority_rank` is a plain int field; the rank-write skill stays in 10.06.
- **`/observation:log` / `/observation:triage` / `/observation:investigate` skills** — these are 10.06-side; T009 only ships the schema they'd write to.
- **Cross-store guards** (T010) — `task_id` stays as plain text soft-FK; referential integrity check is T010 work.
- **`pi-ask-user`** — `actor: human` rejects AI writes today; turning that into a synchronous human-pause is T012-ish.

### DONE_WHEN clauses to phase mapping (planner fills the rest)

- Clause 1 (full add): Phase ?? schema extension
- Clause 2 (triage flow): Phase ?? lifecycle transitions
- Clause 3 (required_when): existing T006 P1 substrate covers; verify in integration
- Clause 4 (actor:human): existing T006 P1 substrate covers; verify in integration
- Clause 5 (evidence list_record): existing T006 P2 substrate covers; verify in integration
- Clause 6 (notes Json): existing T008 substrate covers; verify in integration
- Clause 7 (needs_info parking): Phase ?? lifecycle (additional transitions)
- Clause 8 (cross-store task_id): existing soft-FK; verify in integration

---

## Plan
_Planner agent fills this section._

### Objective
_What we're trying to achieve._

### Scope
- **In Scope:**
  - `stores/observations/schema.yaml` — full extension to 10.06 production shape (Path A) OR
  - `stores/observations_1006/schema.yaml` — promote to production (Path B); decide in Decision Matrix
  - `tests/e2e.sh` — extend or update for new required fields (Path A only)
  - New `tests/observations_e2e.sh` — scripted 8-DONE_WHEN-clause walkthrough
  - Operator integration phase (artefact capture)
  - Decide on the POC schema's fate: retire / rename / document-as-fixture
- **Out of Scope:**
  - Anything listed in `## Task` / "What's NOT in this task"
  - Cross-store guards (T010); pi-ask-user (T012); cosmetic stub cleanup (T011)

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
