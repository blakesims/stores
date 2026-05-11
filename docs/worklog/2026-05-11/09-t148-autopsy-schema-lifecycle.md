# T148 Autopsy Schema Lifecycle

**Date:** 2026-05-11
**Type:** note

## Summary

Schema/lifecycle friction in T148 clustered around source-of-truth drift: task `status` remains the dispatch key in several places while ADR0001 columns (`lifecycle`, `active_step`, `integration_step`) and activation gates were added later; external-review schema says manual runners are legal but live DB CHECK constraints and runtime code still treat only executable runners as valid; ADR0002 upstream columns are added/backfilled by a framework side path, not by the normal store schema migrator.

## Findings

### 1. `complete -> in_review` immediately becomes a wrap dispatch state, but `active_step` stays `wrapping`

Relevant code:

- `stores/tasks/schema.yaml` lines 250-252: final code-review PASS moves `code_review -> complete` with `active_step: wrapping`.
- `stores/tasks/schema.yaml` lines 335-341: `complete -> in_review` via `request_review`, still `active_step: wrapping`; `accept` from `in_review` also leaves `active_step: wrapping`.
- `stores/tasks/schema.yaml` lines 376-391: `on_state.complete` immediately transitions to `in_review`; `on_state.in_review` dispatches `wrap`.
- `src/handlers/submit.rs` lines 1772-1776 document that `submit-wrap` is **not** a lifecycle transition; it only appends `wrap_log` while the row is already `in_review`.

Failure mode: readers that interpret `in_review` as “external review pending” miss that the first action in `in_review` is actually the wrap agent. Conversely readers that interpret `active_step='wrapping'` as “not reviewable yet” will misclassify accepted/in_review rows, because the schema intentionally keeps wrapping active through `in_review -> accepted`.

### 2. Engine-runner backfills external-review rows for every T2/T3 `in_review` task, independent of wrap/activation readiness

Relevant code:

- `src/flow/engine_runner.rs` lines 303-329 selects `tasks WHERE status='in_review' AND tier_hint IN ('T2','T3')` and checks active ER rows.
- `src/flow/engine_runner.rs` lines 359-379 inserts a pending `external_reviews` row, deriving `wrap_log_ref` from current `wrap_log` length; if no wrap exists it still uses `tasks:T###:wrap_log`.
- `stores/tasks/schema.yaml` lines 376-391 also dispatches `wrap` on `in_review`.

Failure mode: `in_review` is overloaded. A T2/T3 row entering `in_review` can have a pending external review minted before `wrap` has completed, because Layer 1 checks only status/tier. If `wrap_log` is empty, the ER record points at an unindexed `wrap_log` ref. This can create stale or content-poor authoritative-review attempts and confuse the later release gate.

### 3. `release-to-integration` can bypass human acceptance from `complete`/`in_review` if policy considers acceptance possible

Relevant code:

- `stores/tasks/schema.yaml` lines 339-341 declares `release-to-integration` from `complete`, `in_review`, and `accepted` to `integration_queued`.
- `src/flow/builtins/release_to_integration.rs` lines 11-19 treats `lifecycle=active && active_step=wrapping` or status `complete|in_review|accepted` as source states, and `human_acceptance_policy` values `required|optional|delegated_by_policy` as “acceptance possible”.
- `src/flow/builtins/release_to_integration.rs` lines 47-67 fires `release-to-integration` after optionally writing a policy delegate when authoritative task review passed.

Failure mode: the schema contains direct `complete/in_review -> integration_queued` edges, so a subscriber on those states can move a row into integration without the human `accept` transition. That may be intended for delegated policy, but the code’s `acceptance_possible` name is broad: `required` is included even when `acceptance_decided_by` is absent. If this builtin is subscribed too broadly, it turns “required human acceptance” into “integration can start”.

### 4. Activation gating is split across schema guards, agents predicates, scanner classification, and migration backfill

Relevant code:

- `stores/tasks/schema.yaml` lines 42-60 defines `activation` default `inactive` and states combustion subscribers should gate on `activation='active'`.
- `stores/tasks/schema.yaml` lines 290-296 guards `start-integration` with `activation == 'active'`.
- `.stores/agents.yaml` lines 103-121 gates `auto-drive` on both non-empty `workspace_path` and `activation == active`; lines 124-150 gate `integrate` subscriptions on activation.
- `src/flow/engine_runner.rs` lines 850-853 scans active-ish statuses using `COALESCE(activation, 'active')`; lines 913-918 hold rows as `inactive` after checking `in_review` special-case first.
- `src/handlers/framework_migrate.rs` lines 17-25 says only `executing`, `code_review`, and `integrating` backfill active; lines 213-229 perform that backfill.

Failure modes:

1. Legacy DBs with no activation column are treated as active by the scanner (`COALESCE(...,'active')`), while fresh/migrated rows default inactive. That is a deliberate compatibility choice but a semantic split.
2. `in_review` classification returns `no_autonomous_reviewer_runner` before checking activation, so inactive T2/T3 review rows may not surface as activation-held.
3. `integration_queued` is not in `IN_FLIGHT_STATES`, so migrated queued integration work becomes inactive and cannot pass `start-integration` until manually activated.

### 5. Live external_reviews runner CHECK is still older than the schema enum

Relevant code/data:

- `stores/external_reviews/schema.yaml` lines 22-23 allows `runner: [codex, pi, claude-code, manual-codex, manual]`.
- Live `.stores/db.sqlite` table SQL currently has `runner TEXT CHECK (runner IN ('codex', 'pi', 'claude-code'))` (read-only sqlite inspection during this autopsy).
- `src/handlers/external_reviews.rs` lines 426-435 explicitly stores manual imports as `runner='codex'` to survive old DB constraints.
- `src/handlers/external_reviews.rs` lines 649-659 only builds executable runners for `codex`, `pi`, `claude-code`.
- `src/handlers/migrate.rs` lines 54-86 only detects column presence/type mismatches, not CHECK enum drift; lines 379-529 contain special rebuilds only for observations source_id / cluster_key.

Failure mode: schema says manual runner values are valid, but existing DBs may reject them. The manual import handler works around this by lying in the row (`runner='codex'`) and preserving the real manual label only in transition history. This is concrete enum schema drift that normal `stores migrate --apply` will not repair because it does not compare/rebuild CHECK constraints generally.

### 6. Manual import-pass creates a passed ER row but does not advance the task lifecycle

Relevant code:

- `src/handlers/external_reviews.rs` lines 369-453 validates transcript/base/head, inserts an `external_reviews` row directly at `status='passed'`, and records transition history `verb='import-pass'`.
- `stores/external_reviews/schema.yaml` lines 8-17 has normal lifecycle transitions `pending -> running -> passed` and supersede/retry, but no import-pass transition.
- `stores/tasks/schema.yaml` lines 330-336 has `submit-external-review` transitions from `in_review`/`blocked` for `REVISE`; there is no explicit PASS transition because PASS is consumed by release-to-integration policy.

Failure mode: importing a PASS is not itself a task transition. Something else must run `release-to-integration` or human `accept`. If that subscriber/gate is stale or inactive, T148-like rows can show a passed external review while the task remains `in_review`, leading operators to manually push other lifecycle verbs.

### 7. External-review supersede semantics count superseded rows as inactive, permitting new attempts, but old passed rows remain authoritative unless explicitly superseded

Relevant code:

- `stores/external_reviews/schema.yaml` lines 13-16 permits `pending|running|passed|tooling_held -> superseded`; no `revise -> superseded` transition is declared.
- `src/flow/engine_runner.rs` lines 327-329 counts active ER rows only in `pending,running,passed,revise,tooling_held`, so `superseded` allows a new attempt.
- `src/flow/builtins/release_to_integration.rs` lines 22-33 accepts any `external_reviews WHERE task_id=? AND status='passed' AND verdict='PASS'`.

Failure modes:

1. A stale `passed` ER remains authoritative for release unless a supersede transition marks it `superseded`.
2. `revise` rows cannot be superseded per schema, despite being included in active-row counting; a REVISE result can block new automatic ER backfill until another path changes it.
3. Supersede has no guard tying `superseded_by` to the new attempt, so provenance is optional rather than invariant.

### 8. ADR0002 upstream fields exist in store schemas and projection code, but framework_migrate adds/backfills them outside normal store migration

Relevant code:

- `stores/intake_items/schema.yaml` lines 38-59 defines ADR0002 `lifecycle`, `waiting_kind`, `outcome`.
- `stores/observations/schema.yaml` lines 42-76 defines ADR0002 observation `lifecycle`, `contract_state`, `waiting`, `waiting_kind`, `outcome`; lines 292-296 defines `superseded_by_id`.
- `stores/architecture_reviews/schema.yaml` lines 23-37 defines ADR0002 architecture-review `lifecycle` and `outcome`; lines 122-127 defines `superseded_by_id`.
- `src/handlers/framework_migrate.rs` lines 334-428 adds these columns with plain TEXT/INTEGER DDL (mostly without CHECK constraints) and calls backfills on every framework drift run.
- `src/handlers/framework_migrate.rs` lines 461-538 backfills intake projections; lines 621-656 handles observation superseded references only when `resolution_kind='superseded'`; lines 691-780 backfills architecture-review projections.
- `src/flow/adr0002_projection.rs` lines 467-472 explicitly says legacy `addressed_by_observation` prose is ambiguous and only `resolution_kind='superseded'` is authoritative.

Failure modes:

1. Fresh schema DDL may have CHECKs from store schema, while framework-migrated older DBs get plain TEXT columns. That is schema-shape drift by migration path.
2. Supersede semantics for observations are intentionally narrow after T148: rows with old `resolution_kind='addressed_by_observation'` default to duplicate/other semantics unless a new `superseded_by_id` exists. Any T148-era data relying on prose “addressed by observation” as supersede will not round-trip.
3. `intake_items` schema file maps to the live `intake` table through manifest conventions; framework_migrate hardcodes table name `intake`. That is fine but brittle if store/table naming diverges again.

### 9. Lifecycle overlay defaults disagree across schema, framework DDL, and live DB history

Relevant code/data:

- `stores/tasks/schema.yaml` lines 64-75 defaults `lifecycle='queued'`, `active_step='none'`.
- `src/codegen/ddl.rs` lines 65-78 also defines framework-added `lifecycle TEXT NOT NULL DEFAULT 'queued'` and `active_step TEXT NOT NULL DEFAULT 'none'`.
- Live `.stores/db.sqlite` table SQL currently shows `lifecycle TEXT NOT NULL DEFAULT 'active' ...` from an earlier migration generation (read-only sqlite inspection during this autopsy).
- `src/handlers/framework_migrate.rs` lines 231-332 backfills lifecycle overlay only when one of the overlay columns is newly added; it does not rebuild changed defaults/checks later.

Failure mode: rows inserted on DBs with historical defaults can start with `lifecycle='active'` while schema and current DDL expect new rows to start `queued`. Normal migrate detects type only, not default/CHECK/default-value drift, so this remains latent until a row’s `status`/overlay disagree.

## Follow-ups

- Add a CHECK/default drift detector or explicit rebuild migration for `external_reviews.runner` so live DBs accept `manual-codex`/`manual` if the schema intends that.
- Decide whether ER backfill should require non-empty/latest `wrap_log` before minting attempts, or whether `in_review` should split into `wrapping` vs `external_review_pending` statuses/steps.
- Tighten `release-to-integration` so `human_acceptance_policy='required'` cannot release without `acceptance_decided_by`.
- Add invariant tests for `status` + (`lifecycle`, `active_step`, `integration_step`, `activation`) tuples, especially `integration_queued`, `in_review`, and migrated legacy rows.
- Make supersede provenance required for ER supersede and add `revise -> superseded` if REVISE attempts should not block fresh attempts.
