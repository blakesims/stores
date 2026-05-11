# Runner Telemetry Dogfood Handoff

**Date:** 2026-05-11
**Context:** Telemetry branch merged to `main` as `e60373b`, with follow-up bounded backfill commit `9ab824e`.

## What works

- Fresh `agent_runs` schema/migration applied on DB open; new telemetry columns exist.
- Historical Pi telemetry backfill now completes quickly enough for the local DB after `9ab824e`:
  - `stores agents telemetry-backfill` result observed: `scanned=1757 updated=632 skipped=1121 parse_failed=4`.
  - The backfill is bounded/streaming and no longer tries to read huge transcripts wholesale.
- `stores runner-stats` works as a raw operational telemetry surface and is explicitly caveated.
- T147 dogfood produced Pi reviewer rows that prove observed/effective Pi metadata is captured:
  - `effective_model_id = gpt-5.5`
  - `provider_id = openai-codex`
  - `api_id = openai-codex-responses`
  - `runner_exit_kind = ok`
  - `payload_valid = 1`
- The run config was present in both main and T147 worktrees:
  - planner: `claude-code` / `opus`
  - executor: `claude-code` / `opus`
  - plan/code reviewers: `pi` / `gpt-5.5` / `thinking: medium`

## What does not work / not proven

- Configured Pi axes did **not** persist on the observed T147 Pi reviewer rows:
  - `configured_model_id` was blank.
  - `configured_thinking_effort` was blank.
  - `effective_thinking_effort` was blank.
- That means the observed/provider side works, but the requested/configured side still needs investigation.
- Likely place to inspect next:
  - `ConfigRoleRunner::name_for_role` returns `pi:gpt-5.5`, but `build_choice` creates `PiRunner::with_config(...)` only from the parsed role config path.
  - The live marker displayed `runner=pi`, not `pi:gpt-5.5`, and the sidecar transcripts did not show `stores_config` events for the T147 reviewer rows.
  - Check whether the engine/worktree config loader is actually using `.stores/config.yaml` from the intended stores root/workspace when spawning child runners.
- Thinking is not observed from provider telemetry yet; until configured events are persisted, `thinking: medium` cannot be verified.

## T147 cleanup / duplicate status

- T148 is the task that actually shipped ADR 0002 upstream lifecycle/read-model work:
  - `T148 status=integrated lifecycle=done`
  - linked observation `L568` is resolved.
- T147 is a duplicate/closure/verification remnant for `L565`, not the primary ADR 0002 implementation anymore:
  - `T147 status=executing lifecycle=active current_phase=1 current_cycle=2`
  - `T145` was abandoned; `T147` replaced it but has now become redundant because T148 integrated.
- I stopped the `stores agents run` daemon I had started and moved stale `current-T147-*` run markers into `.stores/pi-trash/stale-run-markers/` so it stops wasting runner attention locally.
- I could **not** abandon T147 in the substrate because this host has no approval token:
  - `stores auth show` failed: no `~/.config/stores/approve.token`.
  - `tasks abandon` requires actor `human` or `ai_with_human --approve-token <T>`.

## Human cleanup command needed

To close T147 properly, run one of:

```bash
stores tasks abandon T147 \
  --reason "Duplicate closure/verification task. ADR 0002 upstream lifecycle/read-model work shipped through T148 (integrated/done, L568 resolved); keeping T147 active only wastes runner resources." \
  --invoker human
```

or initialize/show an approval token and let the agent run the `ai_with_human --approve-token` form.

## Do not do next

- Do not manually `stores tasks drive T147` again. If any future task is used for telemetry dogfood, let the engine move it.
- Do not judge telemetry by whether T147 finishes; T147 is duplicate work.
- Do not raw-SQL the task row to abandon it.

## Recommended next telemetry investigation

1. Start a fresh tiny non-duplicate task or observation specifically for telemetry dogfood.
2. Let `stores agents run` move it, not manual `tasks drive`.
3. After the Pi reviewer runs, inspect:

```sql
SELECT role, harness_id, model_id,
       configured_model_id, configured_thinking_effort,
       effective_model_id, effective_thinking_effort,
       provider_id, api_id, runner_exit_kind, payload_valid
FROM agent_runs
WHERE display_id='<TASK>'
ORDER BY id;
```

4. If configured fields are still blank, inspect config loading/root routing and whether `PiRunner::with_config` arguments reach `agents/sidecar/pi_runner.mjs`.
