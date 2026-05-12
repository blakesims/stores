# Combined Fake Runner Implementation Plan

**Date:** 2026-05-12
**Type:** note

## Summary

Build no-LLM dogfood mode as a **first-class Stores `FakeRunner` backed by a real `stores-fake-agent` subprocess**. Runner selection stays explicit and Stores-native; execution still has real process/PID/stdout/stderr/exit/signal/timing behavior.

This combines the two earlier proposals:

- **Subprocess realism:** watchdogs, process death, partial transcripts, hangs, and commits are real enough to pressure the broken control plane.
- **Operational discipline:** telemetry/provenance split, scripted scenarios, failure taxonomy, external-review coverage, executor modes, and loud fake-mode operator labeling.

Implementation should happen outside the substrate as direct code phases, each completed by a worker agent and reviewed. Keep **4 phases**; compressing to 3 would overload either external-review/telemetry or failure-scenario work.

Worker/reviewer test discipline: all cargo test commands should use at least `-- --test-threads=8` unless a specific test documents why it must serialize. If a test mutates process-wide env, guard it with the existing env lock rather than lowering the whole run to one thread.

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

Do **not** globally shadow `claude`, `node`, `pi_runner.mjs`, or `codex` as the primary mechanism. Adapter-level shims can be later parser tests, not the main dogfood architecture. `FakeRunner` constructs its own command and argument contract for `stores-fake-agent`; it does not mimic Claude/Pi/Codex CLI arg conventions except in optional adapter-specific tests.

`FakeRunner` must pass env to child commands with command-local env, not process-global mutation, except for reading `STORES_LLM_OFF` and config. This avoids daemon/test concurrency flakes.

### Master switch

Use one primary env var:

```bash
STORES_LLM_OFF=1
```

When set, drive roles and external review default to fake. This should mean **no LLM calls by default**. Later config may opt specific lanes back to real, but the default no-LLM guarantee must be mechanically testable.

Boolean parsing convention: enabled means any non-empty value except `0`, `false`, `no`, or `off` (case-insensitive). Unset or empty is disabled. Read this at runner construction / external-review dispatch time, not only daemon startup, so a long-running daemon can be toggled without restart. The consequence is per-cycle behavior: a task may contain real-run and fake-run cycles if the env changes mid-task; provenance must make that mixture visible. Subscriber-fired drives and in-process dispatchers must consult the same runner-construction path.

### Binary resolution

`FakeRunner` should resolve `stores-fake-agent` robustly:

1. `STORES_FAKE_AGENT_BIN` override.
2. Sibling of `std::env::current_exe()` for installed/private daemon layouts.
3. PATH fallback only for development.

Tests must not depend on global PATH shadowing. Per-test scoped `PATH=...` or explicit `STORES_FAKE_AGENT_BIN=...` is fine for sentinel/negative tests; global shell shadowing is not.

Shipping requirement: add an explicit `[[bin]]` entry for `stores-fake-agent` in `Cargo.toml` (or otherwise prove `cargo install --path ...` installs it beside `stores`). Phase 1 is not done until an installed/private-daemon layout can resolve the fake binary without manual copying.

### Artifact ownership

The drive layer already allocates `RunnerInvocationContext`. Fake mode must honor those exact paths, either by having the child write them directly or by streaming child stdout/stderr through `FakeRunner` into those paths:

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

Acceptance/integration tests may use fake PASS for test rows, but operator-facing output must not imply real Codex/Pi/Claude review occurred. Phase 2 should add a minimal acceptance safety gate: rows whose latest required review/external review is fake cannot be accepted as production-reviewed unless an explicit allow flag or test-mode marker is present.

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

Each decision should be logged as a transcript/live event with seed, roll, threshold, and outcome. `policy_hash` means a stable hash of the fake policy inputs that affected the decision: fake_runner config block, scenario id/script, role defaults, delay/jitter settings, and fake-agent version.

### Scope control

Do not block the MVP on broad telemetry hardening. Current code already has optional `configured_harness_id`, `configured_model_id`, `configured_thinking_effort`, `effective_model_id`, `provider_id`, `api_id`, `runner_exit_kind`, and `payload_valid` fields on `AgentRunTelemetry`; Phase 2 should assert them for fake runs rather than add new schema unless implementation discovers a write-path gap. Required now:

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
- Add/verify `Cargo.toml` binary shipping so `cargo install --path ...` installs `stores-fake-agent` beside `stores`.
- Add `runner::select("fake")`.
- Implement robust fake-agent binary resolution.
- `FakeRunner` launches `stores-fake-agent` with command-local env and the preallocated invocation paths.
- Support fixed delay with heartbeat, default 5 seconds; allow `delay_ms=0` for fast unit/integration tests.
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
- Fill `AgentRunTelemetry` with the full fake-run set (required insert fields plus provenance):
  - `model_id=fake-random-v1` or `fake-scripted:<scenario>-v1` when scenario is known
  - `harness_id=fake`
  - `started_at`, `ended_at`
  - `transcript_path`, `stderr_log_path`
  - `configured_harness_id`, `configured_model_id`, `configured_thinking_effort` when the caller provides requested config
  - `effective_model_id=fake-*`, `effective_thinking_effort=none`, `thinking_effort_source=fake`
  - `provider_id=stores-fake`, `api_id=stores-fake-agent-v1`
  - `session_id`, `workspace_path`
  - `runner_exit_kind=ok`
  - `payload_valid=true`, `payload_error=null`
  - token/cache/cost fields zero or null consistently
- Include loud fake labeling in transcript/events and telemetry.
- Validate fake output against the same parser/schema path used by real runner output. If sharing `AgentEnvelope` types requires a large refactor, use current schema/parser tests first and defer type extraction.
- Tests:
  - unit/fixture test validates each fake role payload against the existing parser/schema path
  - integration-style test drives a minimal task with `runner=fake` and reaches at least wrap/complete without LLM calls
  - binary-resolution test covers `STORES_FAKE_AGENT_BIN` and installed sibling resolution where practical
  - tests that render synthetic task projections must isolate output to a temp workspace or clean/move generated projections afterward so main is not left with untracked `tasks/<state>/TFAKE-*` files

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
- Force external-review runner to fake under `STORES_LLM_OFF`. External review must call `FakeRunner`/`stores-fake-agent` through the Stores runner contract, not try to mimic Codex's `codex exec ...` CLI shape. The acceptance criterion is no Codex/Pi/Claude subprocess for review under `STORES_LLM_OFF`.
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
- Add fake-review acceptance safety: `tasks accept` (or equivalent acceptance precheck) refuses rows whose latest required review/external review is fake unless an explicit allow flag or test-mode marker is present. If the exact gate is more complex than expected, Phase 2 must at least make fake-reviewed production acceptance fail loud and document the remaining edge.
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
- Full production policy beyond the minimal fake-review acceptance safety gate.

### Done when

Setting `STORES_LLM_OFF=1` makes a normal dogfood run token-free across drive and external review, with fake provenance visible in artifacts and telemetry, a test proves no real LLM subprocess was launched, and fake-reviewed rows cannot be accepted as if they had real review without an explicit allow/test marker.

## Phase 3 — Scripted scenarios and failure taxonomy

**Goal:** Deterministic non-happy-path scenarios exercise the substrate's separate failure/recovery paths.

### Scope in

- Add scenario/script support before random soak, with a small named registry addressable by config/env (for example `fake_runner.scenario` or `STORES_FAKE_SCENARIO=code-review-revise-once`). Avoid copy-pasted ad hoc test-only scenario logic.
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
  - SIGTERM-ignoring stall that requires watchdog/SIGKILL where safe to test
  - messy prose/legacy-output scenario that exercises SAP or last-line parsing rather than only clean structured output
- Distinguish outcome classes in `RunnerOutput` and telemetry:
  - semantic failure: valid payload, review gate/ER verdict says REVISE/NEEDS_WORK
  - tooling-held: valid external-review TOOLING_FAILURE
  - payload failure: exit 0 but invalid/missing structured output
  - infra failure: nonzero exit/crash
  - liveness failure: no heartbeat/stall long enough for watchdog path
  - signal behavior: cooperative SIGTERM exit and SIGTERM-ignore/SIGKILL-required modes where safe
- Ensure fake decisions are replayable and logged.
- Add determinism test: run the same scenario twice with the same seed/context and diff the fake decision-event streams.
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
  - fixture patches must target stable marker/test paths or be generated against current HEAD; do not rely on fragile source hunks unless the test owns the fixture repo
- Capture start/end git SHAs in transcript/fake metadata; use existing DB fields if available, otherwise do not force schema churn in this phase unless needed.
- Add optional real external-review opt-back-in for targeted runs, while preserving `STORES_LLM_OFF=1` default as all-fake/no-token.
- Fake external review should still `stat()`/read the expected wrap/review brief path where available and fail if absent, so double-fake mode exercises the wrap-to-review contract.
- Confirm fake transcripts are subject to the same run-artifact GC as real transcripts; add a keep/retention note only if existing GC would destroy needed forensic replay artifacts too aggressively.
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
5. Exact spelling of the explicit fake-review acceptance allow flag/test marker. Default: decide in Phase 2 while implementing the minimal safety gate.

## Why this plan is intentionally four phases

Three phases would either combine selection/external-review with all failure modes, or combine failure taxonomy with git/integration realism. Those are review-heavy seams. Four phases keeps each worker/review cycle focused:

1. Can fake run happily through the standard runner seam?
2. Can fake mode replace all LLM calls and tell the truth in telemetry?
3. Can fake mode exercise failure/recovery paths deterministically?
4. Can fake mode pressure git/integration and avoid drifting from real contracts?

## Progress / learnings

### Phase 1 — shipped

Implemented and reviewed as PASS in commits `49b9fc3`, `979395b`, and `a2eac1d`.

Notable learnings carried forward:

- Drive-loop tests must exercise the real `stores-fake-agent` binary, not an inline shim, or the fake runner and fake binary can drift independently.
- Fake provenance should use `structured_output_source="fake"`, not `sdk`.
- Generated projection artifacts from fake task tests need explicit ignore/isolation (`/tasks/{active,planning}/TFAKE*/`, `/tmp/`) so main stays clean after repeated test runs.
- Tests should run with `-- --test-threads=8`; tests that mutate env must lock rather than forcing serial test runs.

### Phase 2 — shipped, external-review env-race fixed

Implemented and reviewed as PASS in commits `dce5879`, `b16e3b1`, and `956b6d9`.

What landed:

- `STORES_LLM_OFF` boolean parsing and runner-construction-time override.
- Drive roles and external-review dispatch select `fake` under LLM_OFF.
- Minimal `fake_runner` config: `delay_ms`, `seed`, `scenario`, `fake_external_review`.
- Requested runner/model/thinking provenance flows through `STORES_FAKE_CONFIGURED_*` without changing the `Runner` trait.
- Fake decision events include role/task/phase/cycle/attempt/seed/scenario/policy hash/roll/threshold/outcome.
- Fake external-review PASS uses the normal external-review persistence path and records fake provenance.
- Fake-reviewed rows are refused by the acceptance transition unless the explicit allow/test marker is present.

Learning for later phases: `STORES_LLM_OFF` is a process-global test hazard. Any test in any binary that mutates or assumes it must take that binary's env lock and restore/unset the variable. The Phase 2 parallel failure was not product logic; it was `llm_off_external_review_uses_fake_runner_not_codex_command` racing with `codex_shim_invocation_persists_runner_metadata` under `--test-threads=8`. Phase 3 scenario tests must be designed with this in mind.

## Follow-ups

- Use this note as the worker handoff basis for Phase 3.
- Each phase should be implemented by a worker and then reviewed before the next phase starts.
- Keep broad telemetry/schema hardening out of future phases unless directly required for fake provenance correctness.
