# Fake Runner No-LLM Dogfood Proposal

**Date:** 2026-05-12
**Type:** note

## Summary

Shift dogfooding to a first-class Stores `fake` runner: the workflow remains real, but the model boundary is replaced by a configurable, seeded, delayed simulator that emits the same structured outputs, transcripts, run markers, telemetry, and external-review verdicts as the live agent harnesses.

This is the best next move because the current pain is mostly substrate/control-plane reliability, not model intelligence. A fake runner lets us rerun broken workflows cheaply and repeatedly while still exercising dispatch, lifecycle transitions, liveness, telemetry, review loops, external review, integration, and stale-metadata behavior.

## Recommendation

Build a new runner harness, tentatively `fake`, at the existing `Runner` trait seam rather than proxying provider APIs or directly mutating task state.

The fake runner should be selectable in two ways:

- explicit config: `drive.roles.*.runner: fake` and `review.runner: fake`
- global kill switch: `LLM_OFF=true` or `STORES_LLM_OFF=1`, which forces drive and review runner selection to fake unless explicitly opted out

It should support:

- configurable delay, defaulting to about 5 seconds
- deterministic seeded pseudo-random outcomes
- per-role pass/fail probabilities
- scripted scenarios for exact reproduction
- valid role-shaped structured outputs
- deliberate payload/infra/liveness failure modes
- real `.stores/runs` transcript, event, status, and current-run marker behavior
- complete `agent_runs` telemetry with fake provenance made explicit
- external-review PASS/REVISE/TOOLING_FAILURE output through the existing external-review path

## Why this is the best option

The system failures we need to see are downstream of the LLM boundary:

- duplicate dispatch
- runner liveness and stale marker truth
- task lifecycle loops
- code-review and external-review convergence behavior
- payload validation and parser error routing
- `agent_runs` telemetry persistence
- status/watch/runs-current display correctness
- integration lane and post-integration side effects
- schema/live-DB drift surfacing under repeated runs

A first-class fake runner exercises those paths with the least distortion. The runner abstraction already normalizes agent execution into `RunnerOutput`: structured output, final message, transcript path, session id, telemetry, exit code, and payload errors. If fake mode produces a faithful `RunnerOutput`, everything after that stays real.

This is better than a dummy provider proxy because Claude Code, Pi, and Codex do not share a single clean HTTP/provider boundary in this repo. Proxying would require emulating multiple CLIs/SDK formats while still leaving Stores metadata and external-review handling to be solved separately.

This is also better than a direct simulator that fires task transitions, because direct transition simulation bypasses the recent failure surfaces: live markers, dispatch locks, runner status files, transcript backlinks, payload parsing, watchdogs, and telemetry insertion.

## Metadata plan

Fake mode must preserve requested-vs-effective truth. If config says a role would normally use Pi/GPT, but `LLM_OFF` forces fake, telemetry should show both facts:

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
api_id = stores-fake-runner-v1
```

Fake runs must be included in engine-reliability analysis but excluded from model-quality analysis. They should never masquerade as real Claude/Pi/Codex runs.

Every fake decision should be replayable. Do not use process-global unseeded randomness. Derive a per-decision seed from stable inputs such as:

- global fake seed
- task id
- role
- phase
- cycle
- attempt number
- base/head SHA when available
- scenario id
- fake policy/config hash

Log each decision into the transcript/events stream, for example:

```json
{
  "type": "fake_decision",
  "role": "code-reviewer",
  "decision": "REVISE",
  "probability": 0.30,
  "roll": 0.2171,
  "seed": "...",
  "policy_hash": "..."
}
```

## Metadata details we are missing or should add

Telemetry was hardened recently, but fake dogfood will be more useful if the following details are captured or backfilled where possible:

1. **Prompt/config identity**
   - system prompt hash
   - rendered brief hash
   - role schema hash
   - fake policy/config hash
   - `.stores/config.yaml` hash
   - Stores binary version/git SHA/build timestamp

2. **Git/workspace identity**
   - base SHA at runner start
   - head SHA at runner start and end
   - workspace path
   - optional dirty-status summary

3. **Stable downstream backlinks**
   - executor and code-reviewer already have better transcript/session linkage through `tasks.cycles`
   - planner and plan-reviewer still need stable `agent_runs.id`, `session_id`, or transcript-path backlinks in `tasks.plan` and `plan_review_log[]` if we want clean outcome analysis

4. **Clean-data labels**
   - fake_run
   - payload_schema_failure
   - runner_infra_failure
   - duplicate_dispatch
   - stale_base
   - stale_external_review
   - convergence_loop_member
   - manual_out_of_band

Some of this can start as transcript events or metadata JSON before it becomes schema. The immediate requirement is not perfect analytics; it is not losing the provenance needed to interpret fake runs later.

## Realism requirements

The fake runner should not merely return final JSON. It should create realistic runtime artifacts:

- flat transcript JSONL under `.stores/runs/<session>.jsonl`
- live event stream under `.stores/runs/<session>/events.jsonl`
- live status JSON under `.stores/runs/<session>/status.json`
- current-run marker transitions from `running` to `completed` or `failed`
- heartbeat events during delay, so liveness/watchdog behavior is tested
- final output event with the same payload shape expected from real runners
- stderr/log files when simulating warnings or infra failures

It should generate outputs from the same role contracts used by real agents:

- planner: non-empty `phases`, respecting T2 single-phase constraints where possible
- plan-reviewer: `READY`, `NEEDS_WORK`, or `NOT_READY`
- executor: summary, optional commit, changed files
- code-reviewer: `PASS`, `REVISE`, or `FAIL` plus count fields
- wrap: executive summary, deviations, residual risks, sanity checks
- external review: `PASS`, `REVISE`, or `TOOLING_FAILURE` plus findings/counts

## Failure modes to simulate

Do not collapse all negative outcomes into review `REVISE`. The substrate needs separate pressure on separate paths:

- semantic review failure: valid payload with code-review `REVISE`
- plan-review failure: valid payload with `NEEDS_WORK`
- external-review failure: valid payload with verdict `REVISE`
- tooling-held review: valid external-review `TOOLING_FAILURE`
- payload failure: exit 0 but malformed/missing structured output
- runner infra failure: nonzero exit
- liveness failure: delayed or absent heartbeat
- timeout/stall behavior
- slow-run overlap/duplicate-dispatch pressure
- optional dirty-main/stale-base integration pressure

## Scripted scenarios before random soak

Random mode is useful for long soak runs, but initial debugging needs deterministic scripts. Recommended scenarios:

1. all roles pass
2. plan-reviewer rejects once, then passes
3. code-reviewer revises once, then passes
4. external-review revises once, then passes
5. payload-invalid after successful child exit
6. nonzero runner exit
7. long delay with heartbeat
8. long delay without heartbeat, if safe in a test workspace
9. external-review repeated REVISE until convergence guard trips

Only after these are stable should we run probabilistic soak.

## Executor realism

A no-op fake executor is enough to test lifecycle and review loops, but not enough to test integration. Add executor modes:

```yaml
executor_mode: no_op | marker_file | scripted_patch
```

- `no_op`: fastest lifecycle churn
- `marker_file`: writes and commits a deterministic marker file, useful for integration/pre-land testing
- `scripted_patch`: applies a configured patch for targeted tests

Marker commits should include fake provenance in the commit message so they cannot be mistaken for model-authored implementation work.

## What this will miss

Fake mode will not test actual model quality:

- plan quality
- code quality
- reviewer judgment
- instruction following
- hallucination behavior
- real reasoning-effort differences

It will not test real adapter/provider drift unless we add separate adapter shims:

- Claude Code stream-json changes
- Pi SDK/tool-call changes
- Codex output format changes
- auth/model config failures
- provider rate limits
- provider retry/timeout behavior
- real token/cost extraction bugs

It may under-test messy natural-language output if fake responses are always perfect structured JSON. Mitigation: include payload-fuzz modes and malformed-output scenarios.

It may under-test git complexity unless marker/scripted patch modes are used. No-op fake execution will not naturally produce merge conflicts, rebase conflicts, dirty working trees, or pre-land failures.

Most importantly: a fake PASS means the substrate moved correctly, not that a task is shippable.

## What makes this hard

The hard part is not producing fake JSON; it is preserving all the operational surfaces that real runs touch:

- current-run marker selection must remain truthful
- status/watch/runs-current must show fake runs clearly and live
- liveness should see heartbeat behavior during the delay
- `agent_runs` insert must have all required fields
- executor/code-reviewer session backlinks must be present
- external review uses a separate persistence path and must also be covered
- fake randomness must be reproducible or it will create un-debuggable flakes
- fake telemetry must not pollute future model-quality metrics
- fake executor commits must not create misleading implementation history
- `LLM_OFF` must be loud enough that operators do not accidentally trust fake outcomes as real review outcomes

## Implementation order

1. **Fake runner MVP**
   - implement `src/runner/fake.rs`
   - add `runner::select("fake")`
   - emit valid structured outputs
   - write transcript/events/status files
   - fill required telemetry
   - support delay and heartbeat
   - cover external-review structured output

2. **Selection and config**
   - add `fake_runner` config
   - add `LLM_OFF` / `STORES_LLM_OFF` override
   - force both drive and review paths through fake when enabled
   - make fake mode visible in status/telemetry

3. **Deterministic scenarios**
   - scripted outcomes by role/attempt
   - all-pass, revise-once, payload-invalid, nonzero, liveness scenarios

4. **Provenance and analysis hygiene**
   - decision events
   - seed/policy hash
   - prompt/schema/config hashes where cheap
   - clear fake/model-quality exclusion labels

5. **Executor realism**
   - marker-file commit mode
   - start/end SHAs
   - dirty/stale integration scenarios where safe

6. **Adapter-level fake tests later**
   - only after the first-class fake runner exists
   - add targeted shims for Claude/Pi/Codex parser behavior if needed

## Follow-ups

- File/promote an implementation task for the fake runner MVP.
- Decide the final env var name: prefer `STORES_LLM_OFF=1`, accept `LLM_OFF=true` as alias.
- Decide whether fake provenance starts in transcript metadata only or gets first-class schema columns immediately.
- Add a rule to metrics/reporting: fake runs are valid engine-reliability data and invalid model-quality data.
