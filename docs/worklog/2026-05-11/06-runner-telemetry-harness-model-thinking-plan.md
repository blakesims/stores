# Runner Telemetry Harness Model Thinking Plan

**Date:** 2026-05-11
**Type:** note

## Summary

Blake wants the runner layer to answer, with clean reconstructable data, which **harness × model × thinking-effort** combination should be used for each task stage: planner, plan reviewer, executor, code reviewer, and later external/task review.

This note is now scoped **only** to runner telemetry, data collection, and role-level runner configuration axes. It intentionally does **not** own `in_review`, wrap removal, external-review currentness/dedupe, acceptance prechecks, or review-repair lifecycle semantics.

Those lifecycle/review concerns are owned by:

- `docs/worklog/2026-05-11/07-review-lifecycle-stabilization-plan.md`

Current finding: stores has useful outcome data, but not enough causal telemetry. Pi runs are stored as `pi:default` even though transcripts show `gpt-5.5`; thinking level is not persisted for stores-spawned Pi sessions; Pi token usage is currently missed because the runner extracts only top-level `model`/`usage` instead of nested `message.model`/`message.usage`.

## Scope boundaries

### In scope

- Role config axes: `runner|harness`, `model`, and `thinking`.
- Pi sidecar wiring for configured model/thinking where the SDK supports it.
- Pi transcript telemetry extraction from nested assistant metadata.
- Additive `agent_runs` telemetry columns and insert/update plumbing.
- Historical backfill where transcript artifacts make it safe and reconstructable.
- Future metrics/reporting design after clean telemetry exists.

### Out of scope / dependency

- Turning wrap on/off and defining no-wrap `in_review` behavior.
- `active_step=wrapping` lifecycle projection cleanup.
- Watchdog/postcondition/TUI semantics for `in_review`.
- External-review current-head selection, stale/superseded review handling, acceptance prechecks, and review-repair loops.
- Any broad ADR-001/ADR-002 lifecycle or stage migration.

The dependency is explicit: telemetry work can proceed in parallel only if it does not change lifecycle/review behavior. Any code or test touching `in_review`, wrap dispatch, external-review acceptance, or review-repair belongs to the review lifecycle stabilization lane.

## Details

### Current evidence

- `agent_runs` has: `display_id`, `phase`, `cycle`, `role`, `model_id`, `harness_id`, timestamps, exit code, token fields, transcript path, brief text.
- Pi transcript JSONL files under `.stores/runs/` include assistant metadata like `message.model = "gpt-5.5"` and `message.usage`, but `src/runner/pi.rs` currently looks only for top-level `model` / `usage`.
- Stores-spawned Pi agents use `SettingsManager.inMemory(...)` and `SessionManager.inMemory()` in `agents/sidecar/pi_runner.mjs`, so they do not write normal Pi session files under `~/.pi/agent/sessions/...`.
- The sidecar creates a temporary `agentDir` under `/tmp/stores-pi-agent-*`; this is not durable session state.
- The only durable stores-owned runner artifacts are under the driven workspace's `.stores/runs/`, plus DB rows.
- Pi global auth/model defaults are still read from `~/.pi/agent/auth.json` / `~/.pi/agent/models.json` through `AuthStorage.create()` / `ModelRegistry.create(...)`, but global settings are bypassed by `SettingsManager.inMemory(...)`.

### Three-axis selection model

Do not treat "model" as the whole decision. Every run should persist all three independent axes:

1. `harness_id`: `pi`, `claude-code`, `codex`, future harnesses.
2. `model_id`: actual provider model, e.g. `gpt-5.5`, `claude-opus-4-7`, `claude-sonnet-4-6`.
3. `thinking_effort`: effective reasoning/thinking setting, e.g. `none|low|medium|high|max|unknown`, plus raw provider value.

A config row should support role-specific choices across all three axes:

```yaml
drive:
  roles:
    planner:
      harness: claude-code
      model: opus
      thinking: high
    plan_reviewer:
      harness: pi
      model: gpt-5.5
      thinking: high
    executor:
      harness: pi
      model: gpt-5.5
      thinking: medium
    code_reviewer:
      harness: pi
      model: gpt-5.5
      thinking: high
```

Back-compat should accept `runner:` as an alias for `harness:` during migration.

### Proposed telemetry schema

Prefer additive columns first; normalize later if needed.

Add to `agent_runs`:

- `harness_id` already exists; keep it as source harness.
- `configured_harness_id` — requested harness from config/CLI.
- `configured_model_id` — requested model from config/CLI.
- `configured_thinking_effort` — requested thinking level from config/CLI.
- `effective_model_id` — observed model from transcript/provider metadata.
- `effective_thinking_effort` — observed thinking level from transcript/provider metadata or sidecar settings.
- `thinking_effort_source` — `config|pi-settings|provider-event|default|unknown`.
- `provider_id` — e.g. `openai-codex`, `anthropic`, `google`.
- `api_id` — e.g. `openai-codex-responses`, for provider API drift.
- `session_id` — already implicit in transcript path; make explicit for joins.
- `workspace_path` — absolute workspace used for the run.
- `base_sha`, `head_sha` — branch state at runner start/end where meaningful.
- `runner_exit_kind` — `ok|nonzero|timeout|stalled_no_output|payload_invalid|spawn_failed|killed`.
- `payload_valid` / `payload_error` — distinguish model did work from structured-output parser failure.
- `input_tokens`, `output_tokens`, `cache_read_tokens`, `cache_write_tokens`, `cost_total` — use actual transcript usage where available.

Outcome/dedupe data is important, but should be deferred until clean telemetry exists. When implemented, prefer a read-only `runner_outcomes` projection/report first; promote to a durable table only if query cost or ambiguity requires it.

### Metrics to support later

Core per-role questions:

- Success / reject / revise / fail rate by `role × harness × model × thinking`.
- Time-to-valid-output by `role × harness × model × thinking`.
- Token and cost distributions by `role × harness × model × thinking`.
- Plan-review rejection rate by planner combo and by plan-reviewer combo.
- Code-review rejection rate by executor combo and by code-reviewer combo.
- End-to-end task cycle time by combo sequence, not just individual runs.

These metrics should not be treated as model-quality evidence until runner telemetry includes enough covariates and clean-data flags to separate model behavior from substrate/lifecycle failures.

### Clean-data rules for later

Historical data must be queryable but not blindly counted.

Eventually mark runs/attempts with exclusion flags rather than deleting them:

- `duplicate_dispatch`
- `stale_base`
- `stale_external_review`
- `convergence_loop_member`
- `tooling_failure`
- `runner_infra_failure`
- `payload_schema_failure`
- `manual_out_of_band`
- `superseded`

Default model-quality analysis should exclude infra/tooling/stale/duplicate/convergence-loop members, while operational reliability analysis should include them.

Deterministic dedupe keys should not rely only on identical findings text. Use at least:

- `task_id + role + phase + cycle + attempt_kind`
- `base_sha + head_sha`
- `prompt_hash + role_schema_hash + config_hash`
- runner session id / transcript path for provenance

This note does not implement those projections in the first telemetry slice; it preserves them as requirements for the metrics/reporting phase.

### Oracle feedback incorporated

The oracle agreed with the telemetry direction but flagged three risks:

1. Lifecycle/wrap/review behavior is a separate stabilization lane and should not be mixed into telemetry implementation.
2. Model-comparison statistics will be confounded unless we capture difficulty/config covariates.
3. The recovered stash is a salvage source, not an implementation authority; applying it wholesale would reintroduce unrelated lifecycle, projection, and test noise.

Corrections for this telemetry plan:

- Treat `docs/worklog/2026-05-11/07-review-lifecycle-stabilization-plan.md` as the owner for `in_review`, wrap/no-wrap behavior, external-review currentness, acceptance prechecks, and review-repair loops.
- Do not add a new wrap configuration surface in this telemetry slice.
- Partition the recovered stash before committing: keep only telemetry hunks and discard unrelated lifecycle/test/projection/doc noise.
- Current code rejects `model` for non-Claude runners; this must change for Pi if role config gains model/thinking.
- If Pi SDK cannot actually force model/thinking through a known-safe API, the runner must persist `configured_*` separately from observed `effective_* = unknown` and emit a stores-owned config telemetry event. Never compare configured labels as if they were observed facts.
- Keep legacy `agent_runs.model_id` compatibility explicit: leave existing labels untouched when no effective model is observed; when an effective model is observed, populate `model_id` with the effective value only for the new run while also writing `effective_model_id`.
- Statistical analysis needs covariates: tier, phase/cycle, changed-file count, LOC changed, prompt/brief hash, schema version, binary git SHA/version, config hash, base/head SHA, and before/after known engine fixes.
- Outcome data needs two layers eventually: direct model output outcome and downstream semantic outcome. Example: executor `submitted_execution` can later become `failed_code_review`; both are real metrics. Defer durable outcome projection until base telemetry is reliable and a stable join key exists.

## Implementation plan sketch

0. **Recovery hygiene / partition**
   - Work only in `../stores-telemetry` on `feat/runner-telemetry-recovery`.
   - Keep only telemetry hunks from the recovered stash.
   - Exclude unrelated lifecycle, generated task projections, broad tests from other lanes, and review-lifecycle docs unless directly required by telemetry.
   - Commit nothing until the diff is partitioned to telemetry scope.

1. **Telemetry capture MVP: role config and Pi configured axes**
   - Extend drive config parsing from role `{runner, model}` to `{harness|runner, model, thinking}`.
   - Accept `runner:` as a back-compatible alias for `harness:` and reject conflicting values.
   - Allow `model`/`thinking` for Pi without claiming enforcement.
   - Pass configured model/thinking into `agents/sidecar/pi_runner.mjs`.
   - Attempt Pi SDK model/thinking enforcement only if a known-safe per-session API exists; otherwise emit a stores-owned config telemetry event and persist observed effective values as `unknown` unless provider telemetry proves otherwise.

2. **Telemetry capture MVP: Pi extraction and persistence**
   - Update `src/runner/pi.rs::extract_pi_telemetry` to parse nested `message.model`, `message.provider`, `message.api`, and `message.usage`, while tolerating absent/string/object provider fields.
   - Add real Pi-style JSONL fixture tests for nested model/usage/provider/api and stores-owned config events.
   - Add nullable additive `agent_runs` columns for configured/effective harness/model/thinking, provider/api, session/workspace, token/cost fields, and carefully scoped payload/exit metadata.
   - Update fresh schema and additive migration path together so new and existing DBs match.
   - Persist configured/effective/provider/api/session/workspace/token/cost fields on new runs.

3. **Run-quality fields**
   - Populate `runner_exit_kind`, `payload_valid`, and `payload_error` only for mapped paths: success, nonzero, spawn failure, timeout/stall, and payload/schema parse failure.
   - Do not add partially-authoritative run-quality labels for paths that are not mapped.

4. **Backfill as a separate phase**
   - Backfill historical Pi rows from transcript JSONL only after current capture works.
   - Treat thinking as `unknown` unless recoverable.
   - Make backfill idempotent and report counts: scanned, updated, skipped, parse_failed.

5. **Outcome projection deferred until join strategy exists**
   - Before implementing `runner_outcomes`, define the primary join key: prefer `agent_runs.id` recorded downstream or deterministic `session_id`/`transcript_path`; do not use timestamp heuristics as the primary linkage.
   - Current schema finding (2026-05-11): executor and code-reviewer have a stable deterministic link because drive persists `agent_runs.session_id` and embeds `.stores/runs/<session_id>.jsonl` into the downstream `tasks.cycles` sub-record in the same submit transaction. Planner and plan-reviewer do **not** have an equivalent downstream backlink (`agent_runs.id`, `session_id`, or transcript path), so they must remain excluded from outcome projection until schema adds one.
   - Implemented first slice as a read-only `runner_outcomes` view for direct executor/code-reviewer outputs only; no durable outcome table, timestamp heuristic, dedupe, or metrics.
   - Recommended next schema link: record `agent_runs.id` (preferred) or the deterministic `session_id`/transcript path in `tasks.plan` and each `plan_review_log[]` entry when those submissions are committed.

6. **Dedupe/cleanliness deferred until capture fields exist**
   - Current inspection (2026-05-11, `feat/runner-telemetry-recovery`): the telemetry branch is **not sufficient** to implement convergence-loop, duplicate-dispatch, stale-base, or general clean-data flags without heuristics.
   - Present on `agent_runs`: required `started_at` / `ended_at`, `session_id`, `transcript_path`, `workspace_path`, `runner_exit_kind`, `payload_valid`, and `payload_error`.
   - Missing on `agent_runs`: `base_sha`, `head_sha`, `brief_hash`/`prompt_hash`, `role_schema_hash`, and `config_hash`.
   - Join blocker remains: executor and code-reviewer outputs have a deterministic transcript/session backlink through `tasks.cycles`; planner and plan-reviewer submissions still lack an `agent_runs.id`, `session_id`, or transcript-path backlink in `tasks.plan` / `plan_review_log[]`.
   - Required prerequisites before implementing read-only flags: capture runner start/end SHA window (`base_sha`, `head_sha`), persisted brief/prompt hash, role schema hash, effective role config hash, and downstream submission backlinks for planner and plan-reviewer.
   - Do **not** implement `convergence_loop_member`, `duplicate_dispatch`, `stale_base`, or `stale_external_review` for runner telemetry until those fields exist. Do not rely on identical findings text except as a weak signal.
   - No safe minimal read-only cleanliness indicator was added in this phase: `runner_exit_kind` / `payload_valid` can distinguish runner/payload failures, but they do not establish duplicate, stale, or convergence-loop cleanliness.

7. **Metrics surface last**
   - Start with `--json` raw grouped summaries and dirty-data flags.
   - Defer confidence intervals until sample size and exclusion policy are stable.
   - Decide whether external-review metrics belong in the same runner table or a parallel `review_runs` table joined through a shared semantic attempt id.

## Follow-ups

- File/promote the telemetry observability work from I043 into a concrete task if not already routed.
- Keep wrap/no-wrap lifecycle cleanup in `docs/worklog/2026-05-11/07-review-lifecycle-stabilization-plan.md`; do not duplicate it here.
- Decide whether `thinking_effort` vocabulary should mirror Pi's native values exactly or normalize into a cross-harness enum plus raw field. Oracle preference: normalized cross-harness enum plus raw provider value.
- Decide if external-review metrics belong in the same runner table or a parallel `review_runs` table joined through a shared semantic attempt id, after review lifecycle stabilization lands.
- Decide whether model assignment experiments should use randomized/alternating role configs; otherwise adaptive routing will bias model comparisons.
