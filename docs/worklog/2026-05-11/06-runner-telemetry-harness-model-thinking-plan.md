# Runner Telemetry Harness Model Thinking Plan

**Date:** 2026-05-11
**Type:** note

## Summary

Blake wants the runner layer to answer, with clean reconstructable data, which **harness × model × thinking-effort** combination should be used for each task stage: planner, plan reviewer, executor, code reviewer, and external/task review.

Immediate operator decision: **turn wrap off**. It is not buying enough value for its cost. The schema on-state dispatch for `in_review` should be empty, and `.stores/config.yaml` should not configure a wrap runner.

Current finding: stores has useful outcome data, but not enough causal telemetry. Pi runs are stored as `pi:default` even though transcripts show `gpt-5.5`; thinking level is not persisted for stores-spawned Pi sessions; Pi token usage is currently missed because the runner extracts only top-level `model`/`usage` instead of nested `message.model`/`message.usage`.

## Details

### Current evidence

- `agent_runs` has: `display_id`, `phase`, `cycle`, `role`, `model_id`, `harness_id`, timestamps, exit code, token fields, transcript path, brief text.
- `external_reviews` has runner/model/verdict/duration/counts/findings, but its historical rows are polluted by convergence loops and repeated stale reviews.
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

Back-compat can accept `runner:` as an alias for `harness:` during migration.

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

Add or derive a `run_outcomes` view/table:

- `run_id`
- `display_id`
- `role`
- `phase`
- `cycle`
- `attempt_key` = deterministic semantic attempt id.
- `semantic_outcome`:
  - planner: `submitted_plan|payload_fail|runner_fail`
  - plan_reviewer: `READY|NEEDS_WORK|NOT_READY|runner_fail`
  - executor: `submitted_execution|runner_fail|payload_fail`
  - code_reviewer: `PASS|REVISE|FAIL|runner_fail`
  - external_review: `PASS|REVISE|TOOLING_FAILURE|superseded|stale`
- `caused_transition_id` — FK-ish link to `transition_history.id`.
- `superseded_by_run_id` where later fresh runs replace stale output.

### Metrics to support

Core per-role questions:

- Success / reject / revise / fail rate by `role × harness × model × thinking`.
- Time-to-valid-output by `role × harness × model × thinking`.
- Token and cost distributions by `role × harness × model × thinking`.
- Plan-review rejection rate by planner combo and by plan-reviewer combo.
- Code-review rejection rate by executor combo and by code-reviewer combo.
- External-review revise rate by executor combo and by external reviewer combo.
- End-to-end task cycle time by combo sequence, not just individual runs.

Data-quality / dedupe questions:

- How many runs are stale-base, superseded, duplicate-dispatch, convergence-loop, or tooling failures?
- For each semantic attempt, which run is authoritative?
- How many tasks have repeated identical findings? By reviewer/model?
- How many failures are actual model quality failures vs substrate runner/liveness failures?
- Which rows should be excluded from statistical model comparison?

Time-based questions:

- Average wall time per role/combo.
- P50/P90/P95 wall time per role/combo.
- Queue wait vs active runner time.
- Time from `planning → ready`, `ready → code_review`, `code_review → complete`, `complete/in_review → integrated`.
- Calendar drift: does performance change before/after engine fixes or model default changes?

### Clean-data rules

Historical data must be queryable but not blindly counted.

Mark runs/attempts with exclusion flags rather than deleting them:

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

Separate exclusion reasons for model-quality, harness/tooling, substrate orchestration, stale-base/stale-review, duplicate dispatch, convergence-loop, and manual/out-of-band recovery.

### Oracle feedback incorporated

The oracle agreed with the direction but flagged two plan holes: wrap removal has lifecycle fallout, and model-comparison statistics will be confounded unless we capture difficulty/config covariates.

Corrections:

- `in_review` with no next agent must become a legitimate held/review state, not a drive failure.
- `active_step=wrapping` should not remain on new `complete → in_review` or `in_review → accepted` transitions if wrap is gone. Use `active_step=none` for now; a future architecture pass can introduce `task_review` / `acceptance_waiting` if needed.
- Watchdog, postcondition, TUI/progress, and tests/comments that assume `in_review → wrap` need cleanup.
- External review must use contract + plan + cycles + diff as primary inputs. `wrap_log_ref` is optional/historical only.
- Current code rejects `model` for non-Claude runners; this must change for Pi if role config gains model/thinking.
- If Pi SDK cannot actually force model/thinking, the runner must persist `configured_*` separately from `effective_* = unknown` and emit a warning. Never compare labels as if they were observed facts.
- Statistical analysis needs covariates: tier, phase/cycle, changed-file count, LOC changed, prompt/brief hash, schema version, binary git SHA/version, config hash, base/head SHA, and before/after known engine fixes.
- Outcome data needs two layers: direct model output outcome and downstream semantic outcome. Example: executor `submitted_execution` can later become `failed_code_review`; both are real metrics.

### Implementation plan sketch

1. **Wrap off now**
   - Change `stores/tasks/schema.yaml` so `workflow.on_state.in_review` has no `dispatch_agent: wrap`.
   - Change `complete → in_review` and `in_review → accepted` lifecycle projections to `active_step=none`, not `wrapping`.
   - Remove `wrap` from `.stores/config.yaml` to avoid suggesting it is active.
   - Leave `wrap_log` schema in place for historical compatibility.
   - Follow-up cleanup: update watchdog/postcondition comments/tests, TUI labels, and any strings like "wrap pre-deploy" so operators are not told wrap is still part of the live lifecycle.

2. **Pi sidecar config axes**
   - Extend drive config parsing from role `{runner, model}` to `{harness|runner, model, thinking}`.
   - Pass configured model/thinking into `agents/sidecar/pi_runner.mjs`.
   - Use Pi SDK settings/session APIs to set effective model and thinking for that session, or emit explicit stores-owned config events if SDK cannot force one.

3. **Pi telemetry extraction**
   - Update `src/runner/pi.rs::extract_pi_telemetry` to parse nested `message.model`, `message.provider`, `message.api`, and `message.usage`.
   - Capture thinking-level events if present.
   - If no provider event exists, persist configured/effective thinking as `unknown` or config-derived, not silently absent.

4. **DB telemetry migration**
   - Add additive columns to `agent_runs` for configured/effective harness/model/thinking and run-quality metadata.
   - Preserve existing `model_id` as back-compat, probably equal to effective model when known.
   - Backfill historical Pi rows from transcript JSONL where possible: actual `gpt-5.5`, provider/api, tokens/cost; thinking remains `unknown` unless recoverable.

5. **Outcome projection**
   - Build a read-only `runner_outcomes` CLI/report first, using current `plan_review_log`, `cycles`, `external_reviews`, and `transition_history`.
   - Promote to durable table only if query cost or ambiguity requires it.

6. **Dedupe/cleanliness projection**
   - Add deterministic attempt keys and freshness fields.
   - Flag convergence-loop clusters by repeated identical findings/verdicts against same task/head/base.
   - Flag duplicate dispatches by same task/role/phase/cycle overlapping in time.

7. **Operator metrics surface**
   - Extend `stores runner-stats` or add `stores runner-metrics` with filters:
     - `--role`
     - `--harness`
     - `--model`
     - `--thinking`
     - `--since/--until`
     - `--include-dirty-data`
     - `--json`
   - Include confidence intervals where rates are shown.

## Follow-ups

- File/promote the telemetry observability work from I043 into a concrete task if not already routed.
- File/promote follow-up cleanup for wrap-off lifecycle labels/tests if the narrow schema/config change lands first.
- Decide whether `thinking_effort` vocabulary should mirror Pi's native values exactly or normalize into a cross-harness enum plus raw field. Oracle preference: normalized cross-harness enum plus raw provider value.
- Decide if external-review metrics belong in the same runner table or a parallel `review_runs` table joined through a shared semantic attempt id.
- Decide whether model assignment experiments should use randomized/alternating role configs; otherwise adaptive routing will bias model comparisons.
