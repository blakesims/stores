# Combined Fake Runner Implementation Plan

**Date:** 2026-05-12
**Type:** note

## Summary

Build no-LLM dogfood mode as a **first-class Stores `FakeRunner` backed by a real `stores-fake-agent` subprocess**. Runner selection stays explicit and Stores-native; execution still has real process/PID/stdout/stderr/exit/signal/timing behavior.

This combines the two earlier proposals:

- **Subprocess realism:** watchdogs, process death, partial transcripts, hangs, and commits are real enough to pressure the broken control plane.
- **Operational discipline:** telemetry/provenance split, scripted scenarios, failure taxonomy, external-review coverage, executor modes, and loud fake-mode operator labeling.

Implementation should happen outside the substrate as direct code phases, each completed by a worker agent and reviewed. Keep **4 phases**; compressing to 3 would overload either external-review/telemetry or failure-scenario work.

## Guiding decisions

### Architecture

Use this shape:

```text
Stores runner selection
  -> FakeRunner
  -> launches stores-fake-agent subprocess
  -> subprocess honors preallocated RunnerInvocationContext paths
  -> subprocess writes transcript/events/status and exits/crashes/hangs
  -> FakeRunner maps artifacts to normal RunnerOutput
  -> existing drive/external-review lifecycle continues unchanged
```

Do **not** globally shadow `claude`, `node`, `pi_runner.mjs`, or `codex` as the primary mechanism. Adapter-level shims can be later parser tests, not the main dogfood architecture.

`FakeRunner` must pass env to child commands with command-local env, not process-global mutation, except for reading `STORES_LLM_OFF` and config. This avoids daemon/test concurrency flakes.

### Master switch

Use one primary env var:

```bash
STORES_LLM_OFF=1
```

When set, drive roles and external review default to fake. This should mean **no LLM calls by default**. Later config may opt specific lanes back to real, but the default no-LLM guarantee must be mechanically testable.

### Binary resolution

`FakeRunner` should resolve `stores-fake-agent` robustly:

1. `STORES_FAKE_AGENT_BIN` override.
2. Sibling of `std::env::current_exe()` for installed/private daemon layouts.
3. PATH fallback only for development.

Tests must not depend on global PATH shadowing.

### Artifact ownership

The drive layer already allocates `RunnerInvocationContext`. Fake mode must honor those exact paths:

- `session_id`
- flat transcript path
- stderr log path
- events path
- status path

The subprocess must not mint a separate session id or alternate run directory. This keeps `runs current`, status selection, watchdogs, and cleanup pointed at the same artifacts as real runners.

### Machine-readable context, not brief parsing

Pass role/task/cycle context explicitly where possible:

- `STORES_FAKE_ROLE`
- `STORES_FAKE_TASK_ID`
- `STORES_FAKE_PHASE`
- `STORES_FAKE_CYCLE`
- `STORES_FAKE_ATTEMPT`
- `STORES_FAKE_SESSION_ID`
- `STORES_FAKE_TRANSCRIPT_PATH`
- `STORES_FAKE_EVENTS_PATH`
- `STORES_FAKE_STATUS_PATH`

Use existing `spawn_with_invocation_and_env` plumbing where possible. Do not parse role/task/cycle from prose briefs except as a last-resort compatibility fallback.

### Fake runs are real engine data, not model-quality data

Fake runs should count for engine-reliability analysis and should be excluded from model-quality analysis. Preserve configured-vs-effective truth:

```text
configured_harness_id = pi
configured_model_id = gpt-5.5
configured_thinking_effort = medium

harness_id = fake
model_id = fake-random-v1
effective_model_id = fake-random-v1
effective_thinking_effort = none
thinking_effort_source = fake
provider_id = stores-fake
api_id = stores-fake-agent-v1
```

Required by Phase 2 for new fake runs: preserve requested configured harness/model/thinking before override, and store effective fake harness/model/provider/api after override. If an existing insertion path cannot store a configured field, log it in fake decision events and file a follow-up rather than expanding into broad telemetry schema work.

### Operator trust / fake PASS safety

Fake PASS must be loud. Fake external-review PASS should be visibly fake in:

- `external_reviews.runner` / equivalent persisted metadata
- transcript/events
- telemetry
- status/runs output where feasible

Acceptance/integration tests may use fake PASS for test rows, but operator-facing output must not imply real Codex/Pi/Claude review occurred. A later hardening option is an explicit `--allow-fake-review` or test-mode marker before accepting fake-reviewed production rows; do not force that into the MVP unless it is trivial.

### Reproducibility before randomness

Scripted deterministic scenarios come before random soak. Every fake decision should be replayable from stable inputs:

- global seed
- scenario id
- task id
- role
- phase
- cycle
- attempt number
- policy hash

Each decision should be logged as a transcript/live event with seed, roll, threshold, and outcome.

### Scope control

Do not block the MVP on broad telemetry hardening. Required now:

- fake provenance in telemetry/transcript
- configured/effective runner/model distinction where fields already exist
- session/transcript/status paths
- seed/scenario/decision events

Defer unless cheap:

- prompt hash
- schema hash
- `.stores/config.yaml` hash
- start/end base/head SHA DB columns
- planner/plan-reviewer downstream backlink schema changes
- long-term clean-data projections

## Phase 1 — Subprocess-backed fake runner MVP

**Goal:** One happy-path no-LLM task drive can run through the normal runner seam with real subprocess lifecycle and valid artifacts.

### Scope in

- Add `src/runner/fake.rs` implementing `Runner`.
- Add `src/bin/stores-fake-agent.rs` or equivalent binary target.
- Add `runner::select("fake")`.
- Implement robust fake-agent binary resolution.
- `FakeRunner` launches `stores-fake-agent` with command-local env and the preallocated invocation paths.
- Support fixed delay with heartbeat, default 5 seconds.
- Generate schema-valid structured outputs for normal drive roles:
  - planner
  - plan-reviewer
  - executor
  - code-reviewer
  - wrap
- Emit real-ish artifacts at the provided paths:
  - flat transcript JSONL
  - live events JSONL
  - status JSON
  - stderr log if present
- Fill required `AgentRunTelemetry` fields:
  - `harness_id=fake`
  - `model_id=fake-random-v1` or `fake-scripted-v1`
  - `started_at`, `ended_at`
  - `transcript_path`, `stderr_log_path`
  - `session_id`, `workspace_path`
  - `runner_exit_kind=ok`
  - `payload_valid=true`
  - provider/api fake labels
- Include loud fake labeling in transcript/events and telemetry.
- Validate fake output against the same parser/schema path used by real runner output. If sharing `AgentEnvelope` types requires a large refactor, use current schema/parser tests first and defer type extraction.
- Tests:
  - unit/fixture test validates each fake role payload against the existing parser/schema path
  - integration-style test drives a minimal task with `runner=fake` and reaches at least wrap/complete without LLM calls

### Scope out

- `STORES_LLM_OFF` global override.
- Random probabilities.
- Failure modes beyond success.
- External review fake path.
- Marker-file commits.
- Broad telemetry schema changes.
- Claude/Pi/Codex adapter shims.

### Done when

A worker can run a task drive with explicit `runner=fake`; it creates real run artifacts at the invocation paths, inserts valid `agent_runs`, and advances through the standard submit handlers without bypassing lifecycle code.

## Phase 2 — LLM_OFF selection, provenance, and external-review fake

**Goal:** `STORES_LLM_OFF=1` reliably prevents LLM calls across both drive and external review while preserving truthful metadata.

### Scope in

- Add `STORES_LLM_OFF=1` selection override.
- Capture requested runner/model/thinking before overriding to fake.
- Force drive roles to `FakeRunner` under `STORES_LLM_OFF`.
- Locate and cover the actual external-review dispatch path, not only drive runner selection.
- Force external-review runner to fake under `STORES_LLM_OFF`.
- Add minimal `fake_runner` config support:
  - `delay_ms`
  - `seed`
  - `scenario`
  - optional `fake_external_review` override, default true under `STORES_LLM_OFF`
- Preserve requested config in `configured_*` telemetry fields where available, while effective/harness/provider/api fields show fake.
- Add fake decision events:
  - seed
  - scenario
  - role
  - phase/cycle/attempt where available
  - roll/threshold/outcome for deterministic decisions, even if only success is implemented initially
- Generate external-review structured output:
  - `PASS`
  - counts zero
  - findings empty or absent according to schema
- Add operator loudness:
  - runner/status clearly says fake where feasible
  - transcripts and telemetry clearly say fake
  - no fake run can be mistaken for real Codex/Pi/Claude output
- Add no-real-LLM negative test:
  - under `STORES_LLM_OFF=1`, use sentinel binaries, command logging, or equivalent to assert no `claude`, `codex`, `node pi_runner.mjs`, or Pi runner process is invoked
- Tests:
  - env override chooses fake even when config requests Pi/Claude/Codex
  - external-review path uses fake under `STORES_LLM_OFF`
  - configured/effective telemetry split is asserted

### Scope out

- Probabilistic REVISE/FAIL.
- Crash/timeout/payload-invalid simulation.
- Marker commits.
- Real external-review opt-back-in, unless trivial.
- Accept/integrate hard gate for fake-reviewed production rows.

### Done when

Setting `STORES_LLM_OFF=1` makes a normal dogfood run token-free across drive and external review, with fake provenance visible in artifacts and telemetry, and a test proves no real LLM subprocess was launched.

## Phase 3 — Scripted scenarios and failure taxonomy

**Goal:** Deterministic non-happy-path scenarios exercise the substrate's separate failure/recovery paths.

### Scope in

- Add scenario/script support before random soak.
- Script outcomes by role and attempt/cycle, e.g.:
  - all-pass
  - plan-reviewer rejects once then passes
  - code-reviewer REVISE once then PASS
  - external-review REVISE once then PASS
  - external-review TOOLING_FAILURE
  - payload-invalid with exit 0
  - nonzero runner exit
  - long delay with heartbeat
  - long delay without heartbeat / controlled stall
- Distinguish outcome classes in `RunnerOutput` and telemetry:
  - semantic failure: valid payload, review gate/ER verdict says REVISE/NEEDS_WORK
  - tooling-held: valid external-review TOOLING_FAILURE
  - payload failure: exit 0 but invalid/missing structured output
  - infra failure: nonzero exit/crash
  - liveness failure: no heartbeat/stall long enough for watchdog path
- Ensure fake decisions are replayable and logged.
- Tests for each failure class hitting the intended substrate path.

### Scope out

- Git/integration realism beyond no-op executor.
- Large random soak runner.
- Real adapter/provider shims.
- New model-quality metrics.
- Seeded probability mode unless it is small; scripted scenarios are the required deliverable.

### Done when

A reviewer can run named fake scenarios and see the expected task/external-review states without manual LLM use or ambiguous failure classification.

## Phase 4 — Executor realism, integration pressure, and fidelity guardrails

**Goal:** Fake dogfood can pressure git/integration paths and stay aligned with real envelope contracts over time.

### Scope in

- Add executor modes:

```yaml
fake_runner:
  executor_mode: no_op | marker_file | scripted_patch
```

- `marker_file` mode:
  - writes deterministic file(s)
  - commits with fake provenance in commit message
  - reports changed files in executor payload
- `scripted_patch` mode:
  - applies a configured patch or fixture for targeted integration tests
- Capture start/end git SHAs in transcript/fake metadata; use existing DB fields if available, otherwise do not force schema churn in this phase unless needed.
- Add optional real external-review opt-back-in for targeted runs, while preserving `STORES_LLM_OFF=1` default as all-fake/no-token.
- Add envelope/fidelity guardrails:
  - fake payloads use shared Rust types where practical, or schema fixtures if type refactor would sprawl
  - tests validate fake outputs against current agent schemas and `AgentEnvelope` parsing
  - add captured real transcript fixtures if available, but do not require live LLM calls in CI
- Add targeted integration-lane smoke using marker commits.

### Scope out

- Weekly live-LLM drift CI.
- Full provider/API proxying.
- Comprehensive model-quality reporting.
- Large synthetic diff generator unless marker commits prove insufficient.

### Done when

Fake runs can create real commits and push accepted test tasks through integration/pre-land surfaces, and fake output shape is guarded against silent drift.

## Open decisions

1. Whether `stores-fake-agent` should live as `src/bin/stores-fake-agent.rs` in the same crate or in a small workspace crate. Default: same crate for speed and shared types.
2. Whether to refactor `AgentEnvelope` out of `drive.rs` before Phase 1 or defer and validate via schemas first. Default: schema validation first, type refactor in Phase 4 if Phase 1 would sprawl.
3. Whether real external-review opt-back-in belongs in Phase 2 or Phase 4. Default: Phase 4 unless trivial.
4. Whether fake provenance needs first-class DB columns immediately. Default: no; use existing telemetry fields plus transcript/events metadata first.
5. Whether fake-reviewed production rows should require an explicit accept/integrate allow flag. Default: not MVP; allow for test rows while making provenance loud.

## Why this plan is intentionally four phases

Three phases would either combine selection/external-review with all failure modes, or combine failure taxonomy with git/integration realism. Those are review-heavy seams. Four phases keeps each worker/review cycle focused:

1. Can fake run happily through the standard runner seam?
2. Can fake mode replace all LLM calls and tell the truth in telemetry?
3. Can fake mode exercise failure/recovery paths deterministically?
4. Can fake mode pressure git/integration and avoid drifting from real contracts?

## Follow-ups

- Use this note as the worker handoff basis for Phase 1.
- Each phase should be implemented by a worker and then reviewed before the next phase starts.
- Keep broad telemetry/schema hardening out of Phase 1 unless directly required for fake provenance correctness.
