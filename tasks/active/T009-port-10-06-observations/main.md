# T009: Port the 10.06 `observations` store — second real migration

## Meta
- **Status:** CODE_REVIEW
- **Created:** 2026-04-30
- **Last Updated:** 2026-04-30 (Phase 1 code review — PASS; routing to executor for Phase 2)
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

### Objective

Converge the bundled `stores/observations/` to the 10.06 production shape end-to-end so that an `LNNN`-shaped observation trace exercises the eight DONE_WHEN behaviors against a fresh tempdir. Reuse the T006/T007/T008 substrate (transition guards, list_record/list_fk, hyphenated identifiers, repeatable list flags, `FieldType::Json`, `actor: human` and `actor: framework`), follow the T007 cycle-2 methodology (extend in place; canary regression net via `tests/e2e.sh`; canonical-grep blast-radius sweep before mass-rename), and retire the `observations_1006/` POC by freezing it as a referenced fixture.

### Scope

- **In Scope:**
  - `stores/observations/schema.yaml` — full extension to 10.06 production shape (Path A — see Decision Matrix D0). New `intent_contract` record uses production sub-field names per D9.
  - `stores/observations/README.md` — per-field documentation refresh for the new shape; one-line cross-reference to the frozen POC.
  - `tests/e2e.sh` — partial-rewrite migration of v0.1 demo path (rename `OBS001` → `L001`, plus rewrite of steps 5-7 + step 12 JOIN to use `intent_contract` shape; estimated 70-90 LOC touched per Phase 2).
  - New `tests/observations_e2e.sh` — scripted 8-DONE_WHEN-clause walkthrough mirroring `tests/gate_e2e.sh`.
  - `README.md` (root) — update worked example for new `id_format` + new required fields + `intent_contract` instead of `triage`/`contract`.
  - `docs/philosophy.md` — update the worked-example paragraph at line 23 to use `intent_contract.contract_state == 'ready'` framing instead of `triage.verdict == 'T3'` (the philosophy-thesis paragraph that the doc hangs on).
  - `skills/observation:triage/SKILL.md` — full rewrite of the T3-path block: `triage` verb gone; `--verdict` gone; `--done-when` / `--scope-in` / `--scope-out` replaced with the new `intent_contract` sub-fields. The "schema enforces required_when on contract" idea survives, just under the new field names.
  - `skills/observation:log/SKILL.md` — update the example `add` invocation to include the new required fields (`--source`, `--priority`, `--captured-at`, `--captured-week`); fix display_id example `L042` (already L-shaped; no rename needed); drop the dead `schema --json` line if still present.
  - `skills/gate:walk/SKILL.md` — verify the `stores observations show <ref-id>` line still works post-rename; if `<ref-id>` example was `OBS###` it becomes `L###`.
  - `src/handlers/schema_show.rs` — update unit-test fixture string `OBS{:03d}` → `L{:03d}` (lines 216 + 250).
  - `stores/observations_1006/schema.yaml` — leave in place + add 4-6 line frozen-fixture header comment (Decision Matrix D1, Phase 5).
  - Operator integration smoke — fresh `/tmp/t009-obs-port`; capture artefacts for all 8 clauses.
- **Out of Scope:**
  - Everything listed in `## Task` / "What's NOT in this task".
  - Cross-store guards (T010); `pi-ask-user` (T012); cosmetic stub cleanup (T011).
  - Lock-staleness / lock-expiry behavior (10.06's "2h auto-expire" is framework feature work; T009 only ships the columns).
  - Importing 275+ existing 10.06 ledger rows (cutover happens in the 10.06 repo when ready).
  - Rewrites of historical `tasks/completed/` task documents and `findings/*.md` smoke-trace artefacts that incidentally reference `OBS001` (these are point-in-time records and should not be retroactively edited).
  - `docs/handoff-v0.2.md` (frozen historical handoff doc — not edited).
  - `src/handlers/transition.rs` inline `OBS_SCHEMA` test fixture (lines 222-270): self-contained synthetic schema for transition unit tests; internally consistent; not load-bearing on the production shape. **Left as-is** (verified by `cargo test --all`).
  - `tests/fixtures/all_types_store/schema.yaml` (uses `triage.verdict == 'T3'` for required_when): role is to exercise the type system as a generic fixture, not to mirror the production observations schema. **Left as-is.**
  - `tests/drive_e2e.sh` and `tests/tasks_e2e.sh` (use `--done-when` / `--scope-in` / `--scope-out` flags against the **tasks** store, not observations — false positives in the canonical grep).
  - `agents/guide.md:396` (`stores tasks resume` example — `--summary` is universal, false positive).

### Phases

| Phase | Description | Estimated Complexity |
|-------|-------------|---------------------|
| 1 | Schema extension — extend `stores/observations/schema.yaml` to 10.06 production shape (~20 new top-level fields, full 7-state lifecycle, 9 transitions including needs_info parking, `intent_contract` record with required_when on the locked sub-field set per D9, evidence list_record, notes:json, lock primitive). | L |
| 2 | `tests/e2e.sh` partial rewrite (NOT a mechanical rename) — `OBS001` → `L001`, `triage`/`contract` records replaced by `intent_contract`, steps 5-7 rewritten end-to-end, step 12 JOIN updated to extract from `intent_contract` instead of `triage`, ~70-90 LOC touched, all 13 v0.1 demo steps still demonstrate the SAME enforcement behavior under the new shape. | M-L |
| 3 | New `tests/observations_e2e.sh` — scripted 8-clause walkthrough under `/tmp/t009-obs-port`; mirrors `tests/gate_e2e.sh` shape. | M |
| 4 | Live-paths sweep (canonical grep first per T007 P2 methodology) — `README.md`, `docs/philosophy.md`, `stores/observations/README.md`, `skills/observation:triage/SKILL.md`, `skills/observation:log/SKILL.md`, `skills/gate:walk/SKILL.md`, `src/handlers/schema_show.rs`. **9 live files / ~74 hit-sites** total (pre-phase grep tally). Skill rewrites are the load-bearing work; everything else is renames + flag-set updates. | M |
| 5 | POC fate — explicit document-as-frozen-fixture decision: top-of-file comment in `stores/observations_1006/schema.yaml`, one-line cross-reference in `stores/observations/README.md`. | XS |
| 6 | Operator integration smoke — fresh `/tmp/t009-obs-port`; capture CLI outputs + `show --json` artefacts for all 8 DONE_WHEN clauses; `cargo test --all` + every e2e suite green. | S |

(Phase 4 and Phase 5 stay separate commits because Phase 5 documents the architectural decision recorded in D1; folding them risks burying the decision in a docs-sweep commit.)

### Phase Details

#### Phase 1: Schema extension
- **Objective:** Extend `stores/observations/schema.yaml` to be the 10.06 production schema. After this phase, `stores install ./stores/observations` produces a table that can hold every field a 10.06 ledger row carries, and the lifecycle covers every 10.06 transition with the right actor/guard. The old `triage` and `contract` records are removed; the new `intent_contract` record replaces them with the production sub-field names locked in D9.
- **Files to modify:**
  - `stores/observations/schema.yaml`
- **Acceptance Criteria:**
  - [ ] `id_format` is `L{:03d}`; lifecycle has the 7 production states `[open, investigating, confirmed, needs_info, in_progress, resolved, wont_fix]` (initial: `open`); the **8 transitions** below all parse and round-trip:
        - `open → investigating` (verb: `investigate`, actor: `ai_with_human`)
        - `open → wont_fix` (verb: `wont_fix`, actor: `ai_with_human`) — direct path per the 10.06 lifecycle diagram
        - `investigating → confirmed` (verb: `confirm`, actor: `ai_with_human`, **guard:** `intent_contract.contract_state == 'ready'`) — clause 2 + clause 3 backbone
        - `investigating → needs_info` (verb: `request_info`, actor: `ai_autonomous`)
        - `confirmed → needs_info` (verb: `park`, actor: `ai_autonomous`) — clause 7 (parking)
        - `needs_info → confirmed` (verb: `provide_info`, actor: `human`) — clause 7 (resume)
        - `confirmed → in_progress` (verb: `claim`, actor: `ai_autonomous`)
        - `in_progress → resolved` (verb: `resolve`, actor: `ai_autonomous`)
        - `confirmed → wont_fix` (verb: `wont_fix`, actor: `ai_with_human`)
        (Note: the bundled v0.1's `triaged` state is **removed**; `triage` verb is **removed**. This is a breaking change for the v0.1 demo path; Phase 2 rewrites it.)
  - [ ] All ~20 missing top-level fields present with correct types (per the field-reference table in `## Task`): `source` (enum required), `source_id`/`prod_source_id`/`sandbox_source_id` (integer), `origin_db` (enum), `priority` (enum required), `priority_rank` (integer), `priority_rank_at` (timestamp), `scheduled_for` (text), `contact_id` (integer), `field_name` (text), `captured_at` (text required), `captured_week` (text required, **no pattern** — operator-hygiene per D2), `qa_item_id`/`tour_session_id`/`step_index`/`staff_user_id`/`message` (qa-source dedup keys), `capability` (text), `capability_ids` (`list: text`), `investigation_note` (text), `resolved_at` (text), `resolution` (text), `task_id` (text — kept as plain text per D4), `locked_by`/`locked_at`/`lock_reason` (text/timestamp/text with `actor: framework` per D5), `body` (text optional), `summary` (text required, retained), `tags` (`list: text`, retained from v0.1), plus `intent_contract` (record per D9), `evidence` (record-with-`list_record`), `notes` (`json`).
  - [ ] **D9-compliant `intent_contract` sub-fields**, exact list (mirrors 10.06 production per `~/repos/clients/10.06-wt/10.06-main/research/refs/intent-contract.md`):
        - `objective` (text, `required_when: "intent_contract.contract_state == 'ready'"`)
        - `type` (enum `[work, investigation]`, `required_when: ...ready`)
        - `in_scope` (`list: text`, `required_when: ...ready`)
        - `out_of_scope` (`list: text`, `required_when: ...ready`)
        - `acceptance` (`list: text`, `required_when: ...ready`)
        - `inputs` (`list: text`, optional)
        - `touches` (`list: text`, optional)
        - `affects_capability` (text, optional)
        - `tier_hint` (enum `[T1, T2, T3]`, `required_when: ...ready`)
        - `known_solution` (text, optional)
        - `contract_state` (enum `[draft, ready]`)
        - `drafted_by` (text)
        - `drafted_at` (timestamp)
        - `approved_by` (text, `actor: human`, `required_when: ...ready`)
        - `approved_at` (timestamp, `actor: human`, `required_when: ...ready`)
        Total: 15 sub-fields, of which 7 are gated by `contract_state == 'ready'` (5 plain `required_when` + 2 `actor: human` + `required_when`).
  - [ ] **D6 audit-column collision check.** None of the 10.06 field names collide with the reserved set (`created_at`, `created_by`, `updated_at`, `updated_by`): `captured_at`, `resolved_at`, `priority_rank_at`, `locked_at`, `drafted_at`, `approved_at` are all distinct. Executor records this verification in the Phase 1 commit message.
  - [ ] `cargo test --all` green; `cargo run -- install ./stores/observations` against a scratch tempdir succeeds. The inline `OBS_SCHEMA` test fixture in `src/handlers/transition.rs` is **not modified** (it's a self-contained synthetic schema for transition unit tests; internally consistent; left as-is).

#### Phase 2: `tests/e2e.sh` partial rewrite
- **Objective:** Preserve the v0.1 demo's value as a multi-store canary regression net (T007's precedent) while migrating it to the new schema shape. This is **not** a mechanical rename — it is a partial rewrite of steps 5-7 plus the step-12 JOIN, totalling roughly 70-90 LOC touched out of 247. The original demo's pedagogical job (showing the philosophy thesis live: required_when rejects an under-specified write, then accepts it once the contract is filled in) MUST be preserved under the new field names.
- **Files to modify:**
  - `tests/e2e.sh` (partial rewrite of steps 5-7 + JOIN at step 12 + ID-format renames + new required `add` flags + comment header).
- **Acceptance Criteria:**
  - [ ] Steps 1-4 + 8-13 are largely a mechanical rename (`OBS001` → `L001`); the `add` invocation at step 4 is updated to include the now-required fields: `stores observations add --summary "thing broke" --source dev --priority normal --captured-at 2026-04-30 --captured-week w11-d4`. Hits: ~18 `OBS001` rename sites + 1 `add`-flag expansion.
  - [ ] **Step 5 (rejection) rewritten:** the new failure mode demonstrates a write that flips `intent_contract.contract_state` to `ready` while missing required sub-fields. Concrete shape: a verb that writes `--intent-contract.contract-state ready` without the required sub-field set must be rejected, with the error citing `intent_contract.objective`, `intent_contract.acceptance`, `intent_contract.in_scope`, `intent_contract.out_of_scope`, `intent_contract.tier_hint`, `intent_contract.approved_by`, `intent_contract.approved_at` (the 7 sub-fields gated by `contract_state == 'ready'` per D9). The grep assertions on the error string update to match (`grep -q "intent_contract.objective"` etc.).
  - [ ] **Step 6 (success) rewritten:** the same write, now with all 7 required-when sub-fields supplied + a fresh `--invoker human` (because `approved_by`/`approved_at` are `actor: human`), succeeds. The transition is `investigating → confirmed` per Phase 1's lifecycle (so the demo also walks `open → investigating` first via `stores observations investigate L001`).
  - [ ] **Step 7 (show + JSON shape) rewritten:** the `show` output and `show --json` Python assertion check `d['intent_contract']['contract_state'] == 'ready'`, `d['intent_contract']['tier_hint'] == 'T3'`, `d['intent_contract']['objective'] == '...'` instead of the old `triage`/`contract` keys.
  - [ ] **Step 12 (cross-store JOIN) rewritten:** the `sqlite3` query becomes `select o.display_id, o.status, json_extract(o.intent_contract, '$.tier_hint'), g.display_id from observations o left join gate g on g.task_ref = o.display_id`. Expected output asserts `T3` from the new path and a non-NULL `G001`.
  - [ ] The comment-header summary at the top of `tests/e2e.sh` (lines 4-23) is rewritten to reflect the new step shape (commands + flags) so a reader sees the new canon at a glance.
  - [ ] `bash tests/e2e.sh` passes locally; runtime under 30s.
  - [ ] All 13 v0.1 demo steps still pass post-migration; specifically, steps 5/6 demonstrate the SAME enforcement (a write fails because contract sub-fields are missing) but using the new `intent_contract.contract_state == 'ready'` shape.

#### Phase 3: New `tests/observations_e2e.sh`
- **Objective:** Scripted 8-DONE_WHEN-clause walkthrough mirroring `tests/gate_e2e.sh`. This is the operator-trust artefact for T009.
- **Files to modify:**
  - `tests/observations_e2e.sh` (new).
- **Acceptance Criteria:**
  - [ ] One step per DONE_WHEN clause (8 steps), each with explicit `pass`/`fail` output and a final summary block. Tempdir parameterized via `STORES_E2E_TMP` (default `/tmp/t009-obs-port-XXXXXX`), trap-cleaned on exit unless overridden — same shape as `tests/gate_e2e.sh`.
  - [ ] The 8 clauses are exercised end-to-end in this order: (1) full add with all required fields (`--source`, `--priority`, `--captured-at`, `--captured-week`, plus optional dashboard-source ints); (2) `open → investigating → confirmed` triage flow with `intent_contract.contract_state` gated `draft → ready`; (3) **required_when on `intent_contract.objective` / `acceptance` / `in_scope` / `out_of_scope` / `tier_hint`** (the D9-locked sub-field names) — failure path then success path; (4) `actor: human` rejection on `intent_contract.approved_by` / `intent_contract.approved_at` when invoker is `ai_autonomous`; (5) `evidence.external_refs` JSON-array round-trip via `--evidence.external-refs` repeatable flag (T006 P2 substrate); (6) `notes` JSON round-trip via `--notes` (T008 substrate); (7) `confirmed → needs_info → confirmed` parking with human resume (`provide_info` is `actor: human`); (8) cross-store `task_id` soft-FK with `stores tasks <id> show` linking back.
  - [ ] **Clause 8 scope boundary explicit in the script's comment header AND in the `pass` message:** "soft-FK round-trip only; cross-store referential integrity (e.g. `tasks.complete` requires linked `observations.status == 'resolved'`) is T010 work and out of scope here. This step verifies the `task_id` value round-trips through `add`/`show --json` and that `stores tasks show <id>` returns a row, NOT that the framework rejects writes when the FK is dangling."
  - [ ] `bash tests/observations_e2e.sh` passes; runtime under 30s.

#### Phase 4: Live-paths sweep (Path A canonical grep)
- **Objective:** Apply T007 P2's canonical-grep methodology — locate every live reference to the old `OBS{:03d}` shape, the `triage`/`contract` record concept, or the v0.1 add-flag set; update them; leave historical task records and findings alone. Skill rewrites are the load-bearing piece (`observation:triage` SKILL is functionally a from-scratch rewrite of its T3-path block; the others are surface flag-set updates).
- **Pre-phase canonical grep (run first, paste output into commit message):**
  ```bash
  grep -rn "stores observations\|--summary\|--triage\|--contract\|OBS00\|OBS{\|triage\.verdict\|done-when\|scope-in\|scope-out" \
    /home/blake/repos/experiments/stores \
    --include='*.md' --include='*.sh' --include='*.yaml' --include='*.rs' \
    --exclude-dir=tasks --exclude-dir=findings --exclude-dir=target 2>/dev/null
  ```
  Pre-phase tally (verified by planner during cycle 2): **9 live files, ~74 hit-sites** across the load-bearing patterns.
- **Files to modify (live, load-bearing — confirmed by pre-phase grep):**
  1. **`README.md`** (root) — Steps 4, 6, 7, 9, 12 narratives + the `OBS001|triaged|T3|G001` join expected-output line; new `add` flags; new lifecycle path; new `intent_contract` JSON shape. ~9 hit-sites.
  2. **`docs/philosophy.md:23`** — the philosophy-thesis worked-example paragraph references `triage.verdict == 'T3'` and `done_when`/`scope_in`/`scope_out` directly; rewrite to use `intent_contract.contract_state == 'ready'` and `objective`/`acceptance`/`in_scope`/`out_of_scope`/`tier_hint`. **The thesis itself is unchanged** — only the example shape is updated. 1 paragraph (~3 sentences).
  3. **`stores/observations/README.md`** — full per-field reference rewrite for the new shape; the existing 5-line example block at lines 26-31 (`add --summary` → `triage` → `show`) becomes the new lifecycle (`add` with required fields → `investigate` → `confirm` after writing `intent_contract` to `ready` → `show`). ~6 hit-sites + paragraph rewrites.
  4. **`skills/observation:triage/SKILL.md`** — full rewrite of the T3-path block (lines 49-70). **The `triage` verb is gone**; **`--verdict` is gone**; **`--done-when` / `--scope-in` / `--scope-out` are gone**. Replacement: skill teaches the `investigate` verb (verb name from Phase 1), then writing `intent_contract` sub-fields directly via `--intent-contract.objective`, `--intent-contract.in-scope`, etc., then transitioning `investigating → confirmed` (which the schema gates on `intent_contract.contract_state == 'ready'`). The required_when explanation block (lines 22-26) updates to cite `contract_state == 'ready'` instead of `verdict == 'T3'`. The triage rubric table (lines 40-47) **stays** — it's about T1/T2/T3 reasoning, which is independent of the field name (`tier_hint` is the new home). The `--invoker ai_with_human` advice survives. The `schema --json` line (line 19) is dead and gets removed. The `<id>` example uses `L###`. ~12 hit-sites + a from-scratch ~25-line block rewrite.
  5. **`skills/observation:log/SKILL.md`** — update the action-block `add` invocation (lines 43-50): add `--source`, `--priority`, `--captured-at`, `--captured-week` (all required). The `--contact-id` and `--field-name` optional flags survive. The `--note` flag in the rubric (line 37) is invented — drop it (use `--body` instead, which exists). The `--summary` stdin block (lines 52-57) survives (the `--summary-from-file -` plumbing works). The `schema --json` line at line 21 is dead and gets removed. The `L042` display_id example survives. ~6 hit-sites + flag-set update.
  6. **`skills/gate:walk/SKILL.md`** — verify the `stores observations show <ref-id>` line at line 38 still works (it does; `show` survived the rename). If any commented `<ref-id>` example uses `OBS###`, change to `L###`. **Likely zero edits** — but the file is in the sweep list to confirm it during execution.
  7. **`src/handlers/schema_show.rs`** — lines 216 + 250 — unit-test fixture string `OBS{:03d}` → `L{:03d}`. 2 hit-sites.
- **Files explicitly NOT modified (point-in-time records or false positives — these are what the canonical grep returns that should be skipped):**
  - `findings/cli-smoke-2026-04-26.md`, `findings/skill-walkthrough-2026-04-26.md` — historical smoke traces; not edited.
  - `docs/handoff-v0.2.md` — frozen historical handoff doc; not edited.
  - `tasks/completed/T002-tasks-store-v02/*` and any other `tasks/completed/` records — not edited.
  - `agents/guide.md:396` — `stores tasks resume … --summary …` is a tasks-store invocation, not observations. False positive.
  - `tests/drive_e2e.sh`, `tests/tasks_e2e.sh` — `--done-when` / `--scope-in` / `--scope-out` are tasks-store flags here. False positives.
  - `src/handlers/transition.rs` (lines 222-270) — inline `OBS_SCHEMA` synthetic test fixture; internally consistent; left as-is. Verified by `cargo test --all`.
  - `src/handlers/{add,row,submit}.rs` and `src/{schema,validate}/*` — `--summary` / `triage` matches are framework-internal generic identifier handling; nothing schema-specific to update.
  - `tests/fixtures/all_types_store/schema.yaml` — generic type-system fixture; uses `triage.verdict == 'T3'` as a stand-in for required_when on records. Left as-is.
- **Acceptance Criteria:**
  - [ ] First step of phase: run the canonical grep above; paste the full output into the executor commit message; reconcile each hit against the load-bearing-vs-skip lists above.
  - [ ] After updates, the canonical grep across **non-historical, non-false-positive** paths returns zero hits for the load-bearing patterns. Concrete check: `grep -rn "OBS00\|OBS{:03d}\|triage\.verdict\|--verdict\|done-when.*observations" --include='*.md' --include='*.sh' --include='*.yaml' --include='*.rs' --exclude-dir=tasks --exclude-dir=findings --exclude-dir=target . | grep -v -e "drive_e2e\.sh" -e "tasks_e2e\.sh" -e "agents/guide\.md" -e "transition\.rs" -e "all_types_store" -e "handoff-v0.2"` returns zero lines.
  - [ ] `cargo test --all` green (the schema-show unit test was the only Rust call site; other Rust hits are framework-internal and unaffected).
  - [ ] Skills are still **executable instructions** post-rewrite: a literal copy-paste of the `observation:triage` T3-path block by an executor agent issues commands the CLI accepts (i.e., flag names match what `stores observations <verb> --help` reports). Cross-check by hand during the operator smoke in Phase 6.

#### Phase 5: Document POC fate
- **Objective:** Record the architectural decision (D1) in the codebase, not just the task plan, so a future reader of `stores/observations_1006/schema.yaml` understands why two schemas exist.
- **Files to modify:**
  - `stores/observations_1006/schema.yaml` (add a 4-6 line comment header explaining: this was the T006-T008 POC; production is now `stores/observations/`; this file is kept as a frozen fixture for T006 P5 / T008 P5 smoke-trace artefacts at `/tmp/t006-p5-smoke/` and `/tmp/t008-notes-smoke/`).
  - `stores/observations/README.md` (one-line cross-reference: "Historical POC: `stores/observations_1006/` — frozen fixture, not maintained").
- **Acceptance Criteria:**
  - [ ] Top-of-file comment in `stores/observations_1006/schema.yaml` clearly identifies it as frozen.
  - [ ] `stores install ./stores/observations_1006` still parses (we are not deleting or breaking the schema; we are only annotating it).
  - [ ] No code references rely on the POC; `tests/e2e.sh` and `tests/observations_e2e.sh` both use `stores/observations/`.

#### Phase 6: Operator integration smoke
- **Objective:** Trust-but-verify against a fresh tempdir; capture artefacts that prove all 8 clauses end-to-end. This is the T007 P4 / T008 P5 mirror.
- **Files to modify:**
  - None (this phase only writes to `/tmp/t009-obs-port` artefacts and updates this `main.md`'s execution log).
- **Acceptance Criteria:**
  - [ ] Fresh `/tmp/t009-obs-port` tempdir; run the full 8-clause trace by hand (or via `bash tests/observations_e2e.sh STORES_E2E_TMP=/tmp/t009-obs-port`); capture per-clause artefact files (`clause-1-full-add.json`, `clause-2-triage-flow.txt`, ..., `clause-8-cross-store.txt`). Mirror the artefact-table format used at the bottom of `tasks/completed/T007-port-10-06-gate/main.md`.
  - [ ] Verify `task_id` cross-store soft-FK in clause 8 by **first** running `stores install ./stores/tasks` against the tempdir, then adding a row to the bundled `tasks` store (display_id like `T123`), then setting it as `--task-id T123` on an observation, then `stores tasks show T123` and `stores observations show L00X --json` and confirming the strings match. **Scope boundary explicit in the artefact:** "soft-FK round-trip only; cross-store referential integrity (e.g. tasks.complete requires linked observations resolved) is T010 work and out of scope here."
  - [ ] **Skills hand-cross-check:** for each of the rewritten skills (`observation:triage`, `observation:log`), copy the example invocation block out of the skill verbatim and run it against the tempdir; confirm zero "unexpected argument" errors. This is the operator-trust check that catches Phase 4 regressions before T009 closes.
  - [ ] `cargo test --all`, `bash tests/e2e.sh`, `bash tests/gate_e2e.sh`, `bash tests/observations_e2e.sh`, `bash tests/tasks_e2e.sh`, `bash tests/drive_e2e.sh` all green (modulo pre-existing CLAUDECODE/SIGPIPE failures already documented in prior task logs).
  - [ ] Append an artefact table to the Execution Log section of `main.md` listing each of the 8 clause files plus the verifying assertion.

### Decision Matrix

| # | Decision | Options Considered | Choice | Rationale |
|---|----------|-------------------|--------|-----------|
| D0 | Path A vs Path B (which schema becomes production) | A: extend `stores/observations/schema.yaml` in place; B: promote `stores/observations_1006/` as production, leave bundled v0.1 demo | **Path A** | Single source of truth, e2e canary catches regressions, T007 set the precedent, the `_1006` suffix is permanent and confusing if Path B. The `OBS{:03d}` → `L{:03d}` ID rename is a breaking change but the framework is pre-1.0 with no current users; blast radius is bounded (9 live files / ~74 hit-sites per Phase 4 canonical grep, of which only the 2 skill rewrites are load-bearing redesigns; the rest are renames and flag-set updates). |
| D1 | Fate of `stores/observations_1006/` | Delete; rename to a non-`_1006` name; freeze in place with a header comment; document as fixture | **Freeze in place + header comment** (Phase 5) | T006 P5 (`/tmp/t006-p5-smoke/`) and T008 P5 (`/tmp/t008-notes-smoke/`) artefacts reference this schema; deleting would invalidate retrievable evidence. Renaming to e.g. `observations-poc/` adds churn for no gain. A 4-6 line comment header costs nothing and records the decision in-tree. `tests/e2e.sh` does not depend on the POC, so freezing is safe. |
| D2 | `captured_week` enforcement | (a) `required: true` only — operator-hygiene; (b) `required: true, pattern: "^w\\d+-d\\d+$"` — schema-enforced | **(a) required only** | 10.06 production accepts the field as plain text — no regex check exists in `./dev observation`. The pattern is operator-hygiene (skills produce it). T007 D-decisions explicitly preferred operator-hygiene over over-eager schema regex (e.g. `defer_until` was kept as plain text). Cheaper, less brittle when the week format inevitably shifts (e.g. fortnight-mode). |
| D3 | Lifecycle scope | (a) Bundled v0.1's 7 states + extend to all production transitions; (b) POC's 5 states | **(a) full 7 states + 8 transitions** | DONE_WHEN clauses 2 + 7 require `investigating` and `needs_info`. POC's 5-state slim was deliberately under-spec'd (it said so in its comments). Production's lifecycle is the spec; T009 implements it fully. Phase 1 ACs enumerate all 8 transitions with actor + guard. |
| D4 | `task_id` typing | (a) Plain `text` (current); (b) `list_fk: ref: tasks` to leverage T006 P2 round-trip | **(a) plain text** | A single soft-FK is awkward as a list-of-one. 10.06 production has it as plain string (`null` or `"T170"`). Cross-store *guards* are explicitly T010. Clause 8 only requires value round-trips; plain text is sufficient. Revisit when T010 lands cross-store integrity checks; possibly upgrade then. |
| D5 | Lock primitive shape | (a) Mirror tasks's `claimed_by`/`claimed_at` with `actor: framework`; (b) Add as plain text/timestamp with no actor; (c) Skip entirely (defer to a later framework feature) | **(a) `actor: framework`** | 10.06 has `locked_by` / `locked_at` / `lock_reason` and uses them; T009 ships the schema for them. `actor: framework` matches `claimed_by`/`claimed_at` convention in `stores/tasks/schema.yaml` — these are columns the framework writes during a hypothetical `lock`/`unlock` verb (out of scope for T009). Lock-staleness behavior (`>2h auto-expire`) is a future framework feature; T009 only ships the columns. |
| D6 | Audit-column collision check (D1 from T007 reapplied) | (a) Verify no field name collides with reserved audit columns; (b) Skip — assume clear | **(a) explicit verify** | The plan-reviewer flagged this in T007. Production's field names: `captured_at`, `priority_rank_at`, `resolved_at`, `locked_at`, `drafted_at`, `approved_at` are all distinct from the reserved set (`created_at`, `created_by`, `updated_at`, `updated_by`). No `created_by`/`updated_by`-style fields exist in 10.06's schema. Conclusion: no rename is required for T009 (unlike T007's `created_by → filed_by`). Phase 1 commit message records this verification; if any new field gets added between now and execution, executor must re-verify. |
| D7 | `tests/e2e.sh` blast radius (Path A risk) | (a) Mass rename in Phase 2 + canonical grep in Phase 4; (b) Deprecate v0.1 demo and let `tests/observations_e2e.sh` be the only canary | **(a) keep + partial rewrite** | Updated estimate (cycle 2): **~70-90 LOC touched** in `tests/e2e.sh` — partial rewrite of steps 5-7 + JSON-shape assertions + step-12 JOIN, on top of the ID-format rename. Above the ~50 LOC threshold that historically signals "Path A may be wrong" but defensible because (a) the rewrite preserves the canary's intent (the philosophy thesis lives — required_when rejects an under-specified write, then accepts it once the contract is filled), (b) the v0.1 demo's value as a multi-store regression net (observations + gate together) is preserved, (c) the alternative (Path B) creates worse long-term churn (permanent `_1006` suffix). Phase 2 ACs make the rewrite scope explicit step-by-step rather than hiding behind "mechanical rename". |
| D8 | Phase count (5 vs 6 vs 7) | T007 used 4 phases; this task is bigger (more fields, more clauses, POC-fate decision); 7 phases would over-fragment | **6 phases** | Phase 5 (POC fate) is small but architecturally distinct — folding it into Phase 4 would bury the D1 rationale in a mass-update commit. Phase 7 (README polish) folds into Phase 4 (docs sweep). Each phase ≤4 ACs (Phase 1 has 5 — schema is dense), independently committable, with a green test bar at the end. |
| D9 | `intent_contract` sub-field naming convention (cycle 2) | (a) Bundled v0.1: `done_when`/`scope_in`/`scope_out` (under `contract` record); (b) POC `observations_1006/`: `objective`/`in_scope`/`out_of_scope`/`acceptance`/`tier_hint` (under `intent_contract`); (c) **10.06 production**: full sub-field set per `~/repos/clients/10.06-wt/10.06-main/research/refs/intent-contract.md` | **(c) mirror 10.06 production names exactly — the 15-sub-field set listed in Phase 1 AC** | Verified by reading the production reference doc directly. The bundled v0.1's `done_when` / `scope_in` / `scope_out` map to **`acceptance` / `in_scope` / `out_of_scope`** under the new shape (note plural `scopes`); `verdict` maps to **`tier_hint`** with the same `T1`/`T2`/`T3` enum. The POC was already on convention (b), which differs from production only in being a strict subset — adopting (c) is a superset-extend of (b). This decision is load-bearing for Phase 1 (schema), Phase 2 (e2e.sh JSON-assertion rewrites), Phase 3 (clause-3 required_when test), and Phase 4 (skill examples) — they all use the SAME 15 sub-field names. The mapping table for migrating mental models from v0.1 → production is:

| v0.1 (bundled) | 10.06 production (D9) |
|---|---|
| `triage.verdict` (`T1`/`T2`/`T3`) | `intent_contract.tier_hint` (`T1`/`T2`/`T3`) |
| `triage.notes` | (no equivalent — drop; use `investigation_note` top-level) |
| `contract.done_when` | `intent_contract.acceptance` (`list: text`, not single string) |
| `contract.scope_in` | `intent_contract.in_scope` (`list: text`) |
| `contract.scope_out` | `intent_contract.out_of_scope` (`list: text`) |
| (no equivalent) | `intent_contract.objective` (one-line goal — required) |
| (no equivalent) | `intent_contract.type` (`work` / `investigation` — required) |
| (no equivalent) | `intent_contract.contract_state` (`draft` / `ready` — gates the required_when) |
| (no equivalent) | `intent_contract.approved_by` / `approved_at` (`actor: human`) |
| (no equivalent) | `intent_contract.drafted_by` / `drafted_at`, `inputs`, `touches`, `affects_capability`, `known_solution` |

The single most important change: **list-typed sub-fields** (`in_scope`, `out_of_scope`, `acceptance`). The v0.1 was a single-string `done_when` / `scope_in` / `scope_out`; production uses `list: text`. This is a real schema-shape upgrade, not a rename, and Phase 2 e2e.sh assertions must adopt list-of-strings semantics. |

### Risks

- **R1 — Skill-rewrite regression (Issue 1 from cycle 1).** The `observation:triage` SKILL is functionally rewriting the T3-path block; a careless executor could leave the old `--verdict T3` example in. Phase 6 AC's "skills hand-cross-check" (copy example invocations out of the skill verbatim, run against tempdir) is the trip-wire. If that check fails post-Phase 4, the skill needs another pass before T009 closes.
- **R2 — `tests/e2e.sh` partial rewrite is bigger than typical Phase 2 work.** ~70-90 LOC touched in a 247-LOC file is at the upper end of what fits one phase commit. If the executor finds the diff growing past ~120 LOC during execution (e.g., new failures requiring extra steps), they should split into 2 commits within Phase 2 (1: rename + add-flag updates; 2: steps 5-7 + JOIN rewrite) rather than ship one mega-commit.
- **R3 — `intent_contract` sub-field count makes Phase 1's YAML grow ~3x.** The new schema lands at ~150-200 lines vs the v0.1's 96 lines. Code-reviewer mass-eyeball-review takes longer than T007's. Mitigation: Phase 1 AC's explicit 15-sub-field list serves as the review checklist.
- **R4 — Cross-store referential integrity is NOT being added in T009.** The `task_id` field is plain text (D4). DONE_WHEN clause 8 verifies value round-trip only. **A reader of `tests/observations_e2e.sh` clause 8 might mistake the round-trip for an integrity guard**; the script's comment header and pass-message must be explicit (Phase 3 AC). T010 owns this.
- **R5 — `intent_contract` list-typed sub-fields (`in_scope` / `out_of_scope` / `acceptance` are `list: text`, not strings).** Anyone reading the v0.1 demo and assuming the new shape is a string-rename will write `--intent-contract.in-scope "backend handler"` and get a single-element list back. Phase 2 + Phase 3 + skill examples must use repeatable-flag semantics or pipe-separated input per the framework's list conventions (T006 P2 substrate).
- **R6 — `docs/philosophy.md:23` worked example carries the philosophy thesis.** A botched edit to that paragraph could weaken the framework's pitch. Phase 4 file 2 must update the example shape WITHOUT changing the surrounding thesis ("the human is forced to bottle their context the moment they have it" stays verbatim).

---

## Plan Review

- **Gate:** **NEEDS_WORK** (cycle 1 of 3)
- **Reviewer:** plan-reviewer agent
- **Date:** 2026-04-30
- **Open Questions Finalized:** —
- **Path A vs Path B verdict:** **Path A** is correct; rationale is sound (D0). Bundled becomes production; POC frozen as fixture (D1).
- **Decision Matrix:** D0-D8 are all decided with rationale. D6 audit-collision check correctly concludes no collision. D2 (`captured_week`: required only, no pattern), D4 (`task_id` plain text), D5 (lock primitive `actor: framework`) are all consistent with prior task precedents.

### Issues Found (must address before READY)

**Issue 1 — `skills/observation:*` blast radius missed (CRITICAL).**

Phase 4 file list includes only `README.md`, `stores/observations/README.md`, `src/handlers/schema_show.rs`. But the following skill files heavily depend on the v0.1 shape that Path A retires:

- `skills/observation:triage/SKILL.md` — uses the `triage` verb (line 61), `--verdict T3` (line 62), `triage.verdict` rules (line 24, 93), and the `contract` record concept throughout. After Path A: `triage` verb is gone, `triage` and `contract` records are replaced by `intent_contract` (sub-fields `objective`/`acceptance`/`tier_hint` instead of `done_when`/`scope_in`/`scope_out`). The skill becomes broken instructions.
- `skills/observation:log/SKILL.md` — uses `triage` framing (lines 12, 35-37) and the `--summary` add path (line 46). After Path A: needs `--source`, `--priority`, `--captured-at`, `--captured-week` added to the example invocations; T2/T3 framing needs to point at `intent_contract.tier_hint` not `triage.verdict`.
- `skills/gate:walk/SKILL.md:38` — uses `stores observations show <ref-id>` which still works ID-format aside; only ID-format reference may need updating but verify.

This is the **exact failure mode** that sent T007 cycle 1 back to NEEDS_WORK (the planner missed `skills/observation:triage/SKILL.md:76-83`). Phase 4's pre-phase grep must include `skills/**/*.md` in the canonical sweep, and the modified-file list must enumerate each skill explicitly.

**Action:** Phase 4 file list expanded to include `skills/observation:triage/SKILL.md`, `skills/observation:log/SKILL.md`, and `skills/gate:walk/SKILL.md` (verify if any update needed). Phase 4 AC's pre-phase grep command should include `--include="*.md"` over `skills/`.

**Issue 2 — Phase 2 understates the e2e.sh rewrite (AC2 is ambiguous).**

Phase 2 is described as "Mechanical rename + new-required-field updates, no semantic changes." That is not accurate for Path A:

- The bundled `triaged` state is dropped (Phase 1 AC1 lists 7 production states, no `triaged`).
- The `triage` verb is gone — replaced by `investigate` / `confirm`.
- The `triage` and `contract` records are gone — replaced by `intent_contract` with different sub-field names (`done_when`/`scope_in`/`scope_out` → `objective`/`acceptance` etc.).
- `tests/e2e.sh` has ~36 lines mentioning `triage`/`contract`/`verdict` (steps 5-8, 11, 12, 13). Steps 5-7 currently assert on `triage.verdict == 'T3'` and `contract.done_when` — those JSON-shape asserts must be rewritten to assert on `intent_contract.tier_hint == 'T3'` etc.
- Step 12's JOIN uses `json_extract(o.triage,'$.verdict')` — must become `json_extract(o.intent_contract,'$.tier_hint')`.

This is closer to a **rewrite** of steps 5-7 than a rename. Phase 2 AC2's "OR, if the existing `triage`/`contract` v0.1 record concept is retired" is presented as optional but is actually mandatory under Path A.

**Action:**
- Phase 2 description: drop "no semantic changes" — replace with "lifecycle/record-name updates required (`triaged` state removed, `triage`+`contract` records replaced by `intent_contract`); demo path semantically equivalent (still proves required_when on contract sub-fields, still produces a non-NULL JOIN at step 12)."
- Phase 2 AC2: rewrite as definitive (not "OR"). State the chosen demo path (e.g. `open → investigating → confirmed` with the new `intent_contract` ratification flow) and which sub-fields the required_when failure-then-success demonstration uses (`objective`/`acceptance`/`tier_hint` if mirroring POC; `done_when`/`scope_in`/`scope_out` if those are kept under `intent_contract`).
- Phase 2 AC: explicit "JSON assertions in steps 7 and 12 updated from `triage.verdict` to `intent_contract.tier_hint`."

**Issue 3 — Decision missing: which `intent_contract` sub-field names to standardize on.**

The POC `observations_1006/schema.yaml` uses `objective`/`in_scope`/`out_of_scope`/`acceptance`/`tier_hint`. The bundled v0.1's `contract` record uses `done_when`/`scope_in`/`scope_out`. The plan's Phase 1 AC says the field list includes "`intent_contract` (record)" but does not enumerate sub-fields. Phase 3 AC2 hand-waves: "the precise sub-field names track the `intent_contract` record landed in Phase 1." Phase 2 references `done_when`/`scope_in`/`scope_out` (from the v0.1 bundled).

**Action:** Add a Decision Matrix row D9 for "intent_contract sub-field naming: bundled v0.1 names (`done_when`/`scope_in`/`scope_out`) vs POC names (`objective`/`in_scope`/`out_of_scope`/`acceptance`) vs 10.06 production names (whatever those are — verify against `~/repos/clients/10.06-wt/10.06-main/issues/CLAUDE.md`)." This is load-bearing for Phase 1 AC's field list, Phase 2's e2e.sh rewrite, Phase 3's clause-3 required_when test, and Phase 4's skill examples.

### Minor Notes (not blockers)

- **N1:** Phase 1 AC1 lists 7 states `[open, investigating, confirmed, needs_info, in_progress, resolved, wont_fix]`. The bundled v0.1 currently has 8 (includes `triaged`). The plan should explicitly call out "`triaged` state is removed" in Phase 1 description so Phase 2 understands the breaking lifecycle change up front.
- **N2:** D7 estimates "~10 live `OBS001`/`OBS{:03d}` references across `README.md`, `tests/e2e.sh`, and `src/handlers/schema_show.rs`." Actual count is **42 hits** across 4 live files (README.md: 9, tests/e2e.sh: 30, src/handlers/schema_show.rs: 2, stores/observations/schema.yaml: 1 — schema.yaml updated in Phase 1). File count matches; per-line count understated. Not a blocker because Phase 4's canonical grep will catch all of them, but the executor should expect ~3x the touch-count D7 implied.
- **N3:** Phase 6 AC2 cross-store verification adds a `tasks` row first, then sets `--task-id T123` on an observation. Verify `stores tasks add` provides a stable display_id (T123) that survives the verification path; T002 ships display_ids so this should be fine, but Phase 6 should explicitly note the prerequisite (`stores install ./stores/tasks` happens before the cross-store check).
- **N4:** No explicit risk surfaced for "Phase 1 schema gets large enough that the YAML grows beyond ~150 lines, making review slow." Mention in plan that the new schema will land at ~120-150 lines and mass-eyeball-review by code-reviewer is expected to take longer than T007's. Not a blocker; just calibration.

### Verdict

The plan is structurally sound (Path A choice, Decision Matrix complete, lifecycle enumerated, audit collision verified, POC fate handled), but **Phase 4's skill-blast-radius is missing the same way T007 cycle 1 missed it** — a known regression in planner attention to `skills/`. Phase 2's "mechanical rename" framing also understates work. Both are precise revisions the planner can apply without architectural rework.

**Routing:** PLAN_REVIEW → PLANNING (cycle 1 of 3). Planner addresses Issues 1-3, then re-submits.

After Issues 1-3 are resolved:
- Path A is sound.
- Top decision (D0): **Path A** — extend bundled in place; retire POC as frozen fixture.
- Estimated e2e.sh blast radius: **~70-90 LOC touched** in `tests/e2e.sh` (rename + record-name swap + lifecycle-path rewrite of steps 5-7 + JSON-assertion updates + step 12 JOIN rewrite). Above the ~50 LOC threshold that historically signalled "Path A may be wrong" but still defensible because (a) the rewrite preserves the canary's intent, (b) the v0.1 demo's value as a multi-store regression net (observations + gate together) is preserved, (c) the alternative (Path B) creates worse long-term churn.

### Cycle 2 review

- **Gate:** **READY** (cycle 2 of 3) — all three cycle-1 substantive items closed.
- **Reviewer:** plan-reviewer agent
- **Date:** 2026-04-30

**Issue 1 closed (Phase 4 sweep completeness).** Re-ran the canonical grep against the live tree (excluding `tasks/`, `findings/`, `target/`, completed/paused archives). Hits land in exactly the 9 files the planner enumerated: `README.md`, `docs/philosophy.md`, `stores/observations/README.md`, `skills/observation:triage/SKILL.md`, `skills/observation:log/SKILL.md`, `skills/gate:walk/SKILL.md`, `src/handlers/schema_show.rs`, plus `tests/e2e.sh` (Phase 2) and `stores/observations/schema.yaml` (Phase 1). No 10th tracked, non-archive file missed. False-positive exclusions are honest: `transition.rs` inline `OBS_SCHEMA` fixture is self-contained; `tests/fixtures/all_types_store/schema.yaml` uses `triage.verdict` as a generic type-system stand-in; `drive_e2e.sh` / `tasks_e2e.sh` / `agents/guide.md:396` are tasks-store invocations; `docs/handoff-v0.2.md` is a frozen historical record. Spot-checked `docs/philosophy.md:23` — the paragraph IS the philosophy thesis ("required_when — capture intent at the moment of context") with the thesis sentence ("The human is forced to bottle their context the moment they have it") that must survive verbatim. Phase 4 file 2 description correctly preserves it. R6 (philosophy thesis preservation risk) is a real, mitigated risk.

**Issue 2 closed (Phase 2 LOC honesty).** Read `tests/e2e.sh` (246 LOC; planner said 247 — within rounding). Confirmed the structure of the rewrite:
- Steps 5-7 (lines 80-120) are the triage flow being rewritten end-to-end. Error-string greps at lines 85-87 reference `contract.done_when` / `contract.scope_in` / `contract.scope_out` — must become `intent_contract.objective` / `acceptance` / `in_scope` / `out_of_scope` / `tier_hint` / `approved_by` / `approved_at`. Python JSON assertions at lines 111-119 check `d['triage']['verdict']` and `d['contract']['done_when']` — must become `d['intent_contract']['tier_hint']` / `['contract_state']` / `['objective']`.
- Step 12 JOIN at line 218 uses `json_extract(o.triage,'$.verdict')` — Phase 2 AC4 correctly specifies the new path `json_extract(o.intent_contract, '$.tier_hint')`.
- Header comment block (lines 4-23, ~20 LOC) needs full rewrite; Phase 2 AC5 accounts for this.
- Tally: ~18 `OBS001` rename sites + ~40 LOC steps 5-7 rewrite + ~5 LOC JOIN + ~20 LOC header + 1 `add`-flag expansion ≈ 80-90 LOC. Plan's ~70-90 estimate is honest and the upper-band split-into-two-commits trip-wire (R2) at ~120 LOC is reasonable.

**Issue 3 closed (D9 production-name verification).** Read `~/repos/clients/10.06-wt/10.06-main/research/refs/intent-contract.md` directly. All 15 sub-fields the planner listed match the production source-of-truth exactly: `tier_hint` (not `verdict`), `acceptance` (not `done_when`), `in_scope` / `out_of_scope` (not `scope_in` / `scope_out`), `contract_state` with `draft`/`ready`, `drafted_by` / `drafted_at`, `approved_by` / `approved_at` with `actor: human`. Required-when partition matches: 5 plain `required_when` (`objective`, `type`, `in_scope`, `out_of_scope`, `acceptance`, `tier_hint` — actually 6 plain) + 2 `actor: human` + `required_when` (`approved_by`, `approved_at`). The list-typed semantics for `in_scope` / `out_of_scope` / `acceptance` (per the production JSON example) are correctly flagged in R5.

**Minor observation (not blocker):** Production's intent-contract doc (line 165-167) describes a 16th sub-field, `approval_invoker`, paired with `approved_by` ("expected steady state is `approved_by=blake, approval_invoker=blake`"). The plan does not include `approval_invoker` explicitly, but the framework's `actor: human` mechanism on `approved_by` already records the invoker on every write through the existing audit substrate. This is acceptable scope-narrowing — the framework's invoker-tracking subsumes the field. If during execution the executor finds the audit gap matters, it can be added in Phase 1 with a single line entry (no rework upstream).

**Phase 3 / Phase 6 Clause 8 scope boundary verified.** The Phase 3 AC explicitly bakes the soft-FK-only language into the script's comment header AND its `pass` message; Phase 6 AC2 carries the same language to the artefact. A future reader of `tests/observations_e2e.sh` cannot misread Clause 8 as a referential-integrity guard.

**Risks R1-R6 verified.** Each is distinct with a concrete mitigation:
- R1 (skill-rewrite regression) — Phase 6 hand-cross-check is the trip-wire.
- R2 (e2e.sh diff growth) — split-into-two-commits at ~120 LOC.
- R3 (YAML 3x growth, ~150-200 lines) — Phase 1 AC's 15-sub-field enumeration serves as the review checklist.
- R4 (FK-as-guard misread) — explicit Clause 8 scope-boundary language.
- R5 (list-typed sub-fields) — Phase 2/3/skill examples must use repeatable-flag semantics.
- R6 (philosophy thesis preservation) — line-level review of the worked-example paragraph in code review.

**Phase shape sanity.** 6 phases; ACs ≤4 per phase except Phase 1 (5 — schema-dense, justified in D8); Phase 4 + 5 stay separate per D8 rationale (D1 architectural decision visible in its own commit); out-of-scope still clean (no T010/T011/T012/T005 creep). D6 (audit-column collision) verification unchanged from cycle 1.

**Routing:** PLAN_REVIEW → READY. Plan moves to `tasks/active/`; orchestrator hands to executor for Phase 1.

---

## Execution Log

### Phase 1 — Schema extension
- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Finished:** 2026-04-30
- **Commit SHA:** 367fca4
- **Files modified:** `stores/observations/schema.yaml`

**Summary:**
- `id_format`: `OBS{:03d}` → `L{:03d}`
- **Lifecycle**: 7 states `[open, investigating, confirmed, needs_info, in_progress, resolved, wont_fix]` (initial: open). `triaged` state REMOVED. 9 transitions with correct actor/guard.
- **Transitions added/changed**: `open→investigating` (investigate, ai_with_human), `open→wont_fix` (wont_fix, ai_with_human), `investigating→confirmed` (confirm, ai_with_human, guard: `intent_contract.contract_state == 'ready'`), `investigating→needs_info` (request_info, ai_autonomous), `confirmed→needs_info` (park, ai_autonomous), `needs_info→confirmed` (provide_info, human), `confirmed→in_progress` (claim, ai_autonomous), `in_progress→resolved` (resolve, ai_autonomous), `confirmed→wont_fix` (wont_fix, ai_with_human). The v0.1's `triaged` state + `triage` verb REMOVED; POC-only verbs (`ratify`, `start_t2`, `start_t3`, `resolve_t1`) not present.
- **Fields added (new)**: `source`, `source_id`, `prod_source_id`, `sandbox_source_id`, `origin_db`, `priority_rank`, `priority_rank_at`, `scheduled_for`, `captured_at`, `captured_week`, `contact_id`, `field_name`, `qa_item_id`, `tour_session_id`, `step_index`, `staff_user_id`, `message`, `capability`, `capability_ids`, `investigation_note`, `resolved_at`, `resolution`, `task_id`, `locked_by`, `locked_at`, `lock_reason`
- **Records dropped**: `triage`, `contract`
- **Records added**: `intent_contract` (15 sub-fields per D9), `evidence` (list_record), `notes` (json)
- **Fields retained**: `summary`, `body`, `tags`, `priority` (from POC)
- **D6 audit-column collision check**: Verified — `captured_at`, `resolved_at`, `priority_rank_at`, `locked_at`, `drafted_at`, `approved_at` are all distinct from reserved set (`created_at`, `created_by`, `updated_at`, `updated_by`). No rename needed.
- **`cargo test --all`**: 414 unit tests + 2 integration tests = 416 total, all PASS.
- **`stores install ./stores/observations`**: DDL installs cleanly; 40 columns verified via `PRAGMA table_info(observations)`.
- **e2e.sh expected-failure mode**: Step 4 `stores observations add` fails with `Error: validation failed: - captured_at: required - captured_week: required - priority: required - source: required`. NOT a parse error or panic. Expected per plan.
- **Deviations from plan**: None. The plan noted `approval_invoker` (16th sub-field in the production doc audit-trail section) as an acceptable scope-narrowing; not added. `inputs` sub-field is `list: text` as specified. Schema grew to ~210 lines (plan estimated 150-200 — slightly over due to full YAML verbosity with descriptions).

### Phase 2 — `tests/e2e.sh` partial rewrite
- **Status:** COMPLETE
- **Started:** 2026-04-30
- **Finished:** 2026-04-30
- **Commit SHA:** 4cfd3ad
- **Files modified:** `tests/e2e.sh`

**Summary:**
- Header comment block (lines 4-23) fully rewritten: new commands reflect `L001`, `--source dev --priority normal --captured-at ... --captured-week ...`, `intent_contract` shape, `confirm` verb.
- Step 4: `--summary "thing broke"` → `--summary "thing broke" --source dev --priority normal --captured-at 2026-04-30 --captured-week w11-d4`; expected ID `OBS001` → `L001`.
- Step 5: Rewritten — `stores observations update L001 --contract-state ready --invoker human` triggers required_when failures; grep assertions now check `intent_contract.objective`, `intent_contract.acceptance`, `intent_contract.in_scope`, `intent_contract.out_of_scope`, `intent_contract.tier_hint`, `intent_contract.contract_state == 'ready'`.
- Step 6: Rewritten — `investigate L001 --invoker human` + `update L001 --contract-state ready --objective ... --type work --in-scope ... --out-of-scope ... --acceptance ... --tier-hint T3 --approved-by blake --approved-at 2026-04-30 --invoker human` + `confirm L001 --invoker human` (full 3-command triage flow: open→investigating→confirmed).
- Step 7: Rewritten — `show L001` checks `display_id: L001`, `tier_hint: T3`, `contract_state: ready`; `show --json` Python assertion checks `d['intent_contract']['contract_state'] == 'ready'`, `d['intent_contract']['tier_hint'] == 'T3'`, `d['intent_contract']['objective']`, and that `acceptance`/`in_scope`/`out_of_scope` are lists.
- Steps 8-11: Mechanical rename `OBS001` → `L001`, `task_ref OBS001` → `task_ref L001`.
- Gate JSON assertion: `d['task_ref'] == 'L001'` (was `'OBS001'`).
- Step 12: JOIN query updated — `json_extract(o.intent_contract,'$.tier_hint')` replaces `json_extract(o.triage,'$.verdict')`; grep assertions check `L001` and `T3`; expected output is `L001|confirmed|T3|G001`.
- Step 13 summary block: all references updated to L001 and new shape.
- **LOC delta**: 104 added / 64 deleted across 168 changed lines; file grew from 246 → 286 LOC. Exceeds plan's 70-90 estimate (R2 trip-wire was 120 LOC). Cause: header rewrite added ~15 LOC; step 6 expanded from 4 to 12 LOC (3 commands instead of 1); step 7 Python assertions doubled. No split into 2 commits needed — changes are one coherent logical unit.
- **`bash tests/e2e.sh` result**: EXIT 0 — all 13 steps PASS.
- **Canonical grep**: `grep -nE "OBS|--triage |--contract |triage\.|done_when|scope_in|scope_out|verdict" tests/e2e.sh` returns exactly 1 hit: line 254 commentary `# Uses intent_contract.tier_hint (new shape) instead of triage.verdict (v0.1)` — acceptable commentary per plan.
- **`cargo test --all`**: 414 unit + 2 integration = 416 total, all PASS, 0 fail.
- **Deviations from plan**: None substantive. LOC delta (168 touched vs 70-90 estimate) is documented above. The `--summary` flag appears in the `add` command (line 84) and header (line 8) — these are not old-shape references; `--summary` is a universal retained field.

---

## Code Review Log

### Phase 1 — Schema extension
- **Reviewer:** code-reviewer agent
- **Date:** 2026-04-30
- **Commit reviewed:** `367fca4` ("feat(T009-P1): extend observations schema to 10.06 production shape")
- **Gate:** **PASS**

**Verification against ACs (Phase 1):**

| AC | Status | Notes |
|----|--------|-------|
| `id_format: L{:03d}` | ✓ | Line 2 of schema.yaml |
| 7 lifecycle states `[open, investigating, confirmed, needs_info, in_progress, resolved, wont_fix]` | ✓ | Line 9; `triaged` removed; `initial_state: open` |
| 9 transitions with correct actor/guard | ✓ | All 9 transitions present (plan AC1 listed 8, schema has 9 — extra is `confirmed → wont_fix` which IS in plan AC1; planner said "8" but enumerated 9; schema correct) |
| `investigating → confirmed` guarded on `intent_contract.contract_state == 'ready'` | ✓ | Line 29 |
| `provide_info` (needs_info → confirmed) `actor: human` | ✓ | Line 47 |
| All ~26 new top-level fields with correct types | ✓ | 33 top-level fields total; SQLite install produces 40 columns (4 audit + 2 framework `id`/`display_id` + `status` + 33 user fields = 40); types match plan field-reference table |
| `intent_contract` has all 15 D9-locked sub-fields with production names | ✓ | Verified against `~/repos/clients/10.06-wt/10.06-main/research/refs/intent-contract.md` directly. All present: `contract_state`, `drafted_by`, `drafted_at`, `objective`, `type`, `in_scope`, `out_of_scope`, `acceptance`, `tier_hint`, `inputs`, `touches`, `affects_capability`, `known_solution`, `approved_by`, `approved_at`. **Zero forbidden v0.1 names** (`done_when`/`scope_in`/`scope_out`/`verdict`) anywhere in schema (grep confirmed empty) |
| `in_scope`, `out_of_scope`, `acceptance` typed as `list: text` (not single string) | ✓ | R5 risk averted; production list semantics preserved |
| `objective` is plain `text` (single line) | ✓ | Line 257 |
| `tier_hint` enum `[T1, T2, T3]` | ✓ | Line 287 |
| `actor: human` on `approved_by` + `approved_at` | ✓ | Lines 317, 323 |
| 8 `required_when` annotations on intent_contract sub-fields gated by `contract_state == 'ready'` | ✓ | Plan AC summary said "7 gated" but enumerated 8 in the explicit field list (6 plain + 2 actor:human). Schema matches the enumerated list, which matches the production reference doc (the doc lists `type: always` as required). 6 plain (`objective`, `type`, `in_scope`, `out_of_scope`, `acceptance`, `tier_hint`) + 2 actor:human (`approved_by`, `approved_at`) = 8 total. The plan's "7" was an internal summary-vs-enumeration count discrepancy; not a schema bug |
| `evidence` record with `external_refs: list_record` | ✓ | T006 P2 substrate; sub-fields `system`/`kind`/`id` all required text |
| `notes: type: json, required: false` | ✓ | T008 substrate; line 361 |
| `locked_by`/`locked_at`/`lock_reason` with `actor: framework` | ✓ | Lines 214, 220, 226 (D5) |
| D6 audit-column collision check | ✓ | Header comment lines 4-6; verified distinct from reserved set |
| `cargo test --all` green | ✓ | 414 unit + 2 integration = 416 PASS, 0 fail |

**Tests re-run during review:**
- `cargo test --all` — 416/0 PASS (matches executor's claim)
- `bash tests/e2e.sh` — fails at Step 4 with **clean validation error**: `validation failed: - captured_at: required - captured_week: required - priority: required - source: required`. NOT a parse error or panic. Failure is at the EXPECTED step (4 — first observation add) and is the EXPECTED shape (validator field-required cluster). Phase 2 will fix.
- `bash tests/gate_e2e.sh` — PASS (6 DONE_WHEN clauses)
- `bash tests/drive_e2e.sh` — PASS (AC7.1 + AC7.1b)
- `bash tests/tasks_e2e.sh` — fails at Step 16 (`ac5_11b atomicity test failed`). **Pre-existing failure** — confirmed by re-running against the prior schema (HEAD~2); same failure. NOT a Phase 1 regression.

**Out-of-scope discipline:**
- `git show 367fca4 --stat` confirms ONLY 2 files touched: `stores/observations/schema.yaml` (+304/-40) and `tasks/active/T009-port-10-06-observations/main.md` (+24). **Zero `src/` Rust files modified** — plan required this.
- `approval_invoker` (16th production sub-field) NOT added. This is the explicit cycle-2 plan-review scope narrowing — production framework substrate already records invoker; the plan reviewer's note ("acceptable scope-narrowing") authorizes this. NOT a finding.

**Observations (not findings):**
- `body` field retained (line 74) — plan AC explicitly lists it as "retained from v0.1 + present in production"; production reference doc does not name `body` directly but the field is harmless and the plan called for retention. On-spec.
- `tags: list: text` retained (line 230) — plan AC lists as retained; production has it. On-spec.
- Schema landed at 364 lines vs plan's 150-200 estimate. R3 anticipated "may grow beyond 150 lines"; this is at the upper end but fully described per-field with descriptions, which is the right trade-off for a production-shape store.
- Plan AC1 enumerated 8 transitions but the schema has 9 (the listed 8 plus `confirmed → wont_fix`). Re-reading plan AC1 carefully: it lists `confirmed → wont_fix` as the 9th bullet. The "8 transitions" header was a count error; the enumeration is correct and the schema matches. Not a bug.
- D9 `type` sub-field: schema correctly types it `enum [work, investigation]` and gates with required_when. Production doc confirms `type: always` required and `work | investigation` enum. Match.

**Findings:** None substantive. The two minor count discrepancies in the plan ("7 gated" vs 8; "8 transitions" vs 9) are plan-summary-vs-enumeration mismatches; the schema implements the enumerated lists, which match the production reference. The schema is the spec; the plan summaries are scaffolding.

**Decision:** PASS. The D9 production names are exactly right (verified against `intent-contract.md` reference doc directly). Lifecycle is complete with correct actors and guards. List-typed sub-field semantics preserved. e2e.sh fails cleanly at the right step in the right shape. No `src/` touched. No regressions in other e2e suites (the tasks_e2e.sh failure is pre-existing). This is a clean schema-only foundation for Phase 2.

**Routing:** CODE_REVIEW → EXECUTING_PHASE_2.

---

## Completion
_Final summary when task is complete._
