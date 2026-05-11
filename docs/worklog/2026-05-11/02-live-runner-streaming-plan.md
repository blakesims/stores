# Live Runner Streaming Plan

**Date:** 2026-05-11
**Type:** note / implementation plan
**Context:** Direct-on-main substrate rescue work. This does not overlap T146's feature branch; it is engine observability infrastructure needed to diagnose current runner wedges.

## Summary

Implement live, runner-agnostic streaming for stores task runners so operators can see what an executor/reviewer/planner is doing while it runs, instead of waiting for post-exit transcripts. The target experience is similar to `pi-subagents`: live JSONL/event logs, durable progress snapshots, and a tail/view command that shows assistant text, tool starts/ends, retries, usage, and last activity.

The first slice should be intentionally narrow: preserve current runner behavior, but change transcript writing from "buffer until child exits" to "append and flush as stdout/stderr lines arrive". Then layer normalized events and UI/TUI affordances on top.

## Problem

Current stores runner telemetry is insufficient for long-running or wedged tasks:

- Pi runner stdout is captured in memory and written to `.stores/runs/<session_id>.jsonl` only after helper exit.
- Claude Code runner already uses `--output-format stream-json --verbose`, but currently blocks on `.output()` and parses stdout post-hoc.
- Codex can emit JSONL with `--json`, but we should make the runner consume it incrementally rather than wait for completion.
- `dispatch_locks.heartbeat_at` only proves a line was received at some point; it does not expose what happened.
- PID liveness is a weak signal: alive socket/process does not prove useful work.

Desired liveness hierarchy:

1. last normalized semantic event: assistant text, tool start/end, retry, usage, final output
2. last raw stdout/stderr line
3. current tool and duration
4. child PID alive
5. secondary evidence: filesystem/process I/O

## Evidence / runner capabilities

### Pi runner

`agents/sidecar/pi_runner.mjs` already subscribes to Pi session events and writes JSONL lines to stdout:

```js
session.subscribe((event) => process.stdout.write(`${JSON.stringify(event)}\n`));
```

The Rust `PiRunner` currently calls `liveness::run_streaming_with_liveness(...)`, so the line-reading primitive already exists. The missing piece is a live sink that writes each line as it arrives and maps events into durable progress.

### Claude Code runner

Claude Code headless mode supports real-time JSONL on stdout using the flags we already pass:

```text
--output-format stream-json --verbose
```

Add optional:

```text
--include-partial-messages
```

for token-level deltas/typewriter output. Without it, tool-level liveness is still available.

Important stream event types:

- `system`, subtype `init`
- `system`, subtype `api_retry` — critical to distinguish rate-limit/retry from a hang
- `assistant` — text plus `tool_use` blocks
- `user` — `tool_result` blocks
- `stream_event` — partial deltas when `--include-partial-messages` is enabled
- `result` — final event with result text, structured output, usage, and optional error subtype

Current `src/runner/claude_code.rs` should replace blocking `.output()` with piped `spawn()` plus line readers. Existing post-hoc parsers can continue to operate on the same accumulated/live-written JSONL.

### Codex runner

Codex non-interactive mode supports JSONL with:

```text
--json
```

Useful documented events:

- `thread.started`
- `turn.started`
- `item.started`
- `item.completed`
- `turn.completed`
- `turn.failed`
- `error`

Useful `item.type` values include:

- `agent_message`
- `reasoning`
- `command_execution`
- `file_change`
- `mcp_tool_call`
- `web_search`
- `plan_update`

Map `command_execution` start/completion to tool events; map `agent_message` to assistant text; map `turn.completed.usage` to usage.

## Proposed architecture

### 1. Add a runner event sink abstraction

Introduce a small common layer, not a giant rewrite:

```rust
pub enum RunnerEvent {
    RawStdout { line: String },
    RawStderr { line: String },
    AssistantText { text: String },
    ToolStart { id: Option<String>, name: String, args_preview: Option<String>, path: Option<String> },
    ToolEnd { id: Option<String>, name: Option<String>, ok: Option<bool>, summary: Option<String> },
    Usage { input_tokens: Option<u64>, output_tokens: Option<u64>, cache_read_tokens: Option<u64> },
    Retry { attempt: Option<u64>, max_retries: Option<u64>, delay_ms: Option<u64>, reason: Option<String> },
    Heartbeat,
    FinalOutput { payload: serde_json::Value },
    Error { message: String },
}

pub trait RunnerEventSink {
    fn on_event(&mut self, event: RunnerEvent) -> anyhow::Result<()>;
}
```

Keep it append-only and best-effort where possible. The sink should never hide runner failures, but observability write failures should be explicit and diagnosable.

### 2. Create live run directories at spawn start

For every runner invocation, create a stable live directory immediately:

```text
<workspace>/.stores/runs/<session_id>/
  events.jsonl        # normalized events, append+flush live
  raw.stdout.jsonl    # raw stdout lines, append+flush live
  raw.stderr.log      # stderr lines, append+flush live
  status.json         # overwritten snapshot: current tool, last activity, tokens, pid
  final.json          # written on completion
```

Compatibility option: also maintain the existing flat transcript path (`.stores/runs/<session_id>.jsonl`) as the raw stdout transcript, but write it live instead of post-exit.

### 3. Persist running-attempt metadata before completion

Current `agent_runs` rows are inserted after the runner returns, so operators cannot discover live paths from the DB while a run is active.

Minimal first option:

- Add running fields to task row / dispatch lock only:
  - current runner session id
  - live run path
  - last event at
  - current tool summary

Better option:

- Add `agent_run_attempts` or allow `agent_runs` start rows with nullable `ended_at`/`exit_code`.

Recommended initial implementation: add a small `runner_invocations` table to avoid destabilizing `agent_runs` invariants:

```text
runner_invocations
  id
  display_id
  task_row_id
  phase
  cycle
  role
  runner
  pid
  session_id
  live_dir
  started_at
  last_event_at
  current_tool
  current_path
  status: running|completed|failed|killed
  exit_code nullable
  ended_at nullable
```

Then later fold or relate this to `agent_runs` once stable.

### 4. Add a CLI viewer

Add commands:

```bash
stores runs tail T146
stores runs tail T146 --role executor
stores runs show T146 --json
stores runs current T146
```

Initial `tail` can read `runner_invocations` for the latest running invocation and follow `events.jsonl`.

Human rendering examples:

```text
[09:05:47] assistant: Running cargo test for runner liveness regression...
[09:05:48] tool_start bash path=- args="cargo test ..."
[09:06:08] tool_end bash ok
[09:06:09] retry upstream attempt=2 delay=1200ms reason=rate_limit
[09:06:20] usage input=123456 output=7890 cache_read=120000
```

### 5. Update status/disposition

`stores tasks status T146` should include:

```text
Runner: executor via pi
PID: 3554327
Live: .stores/runs/<session_id>/events.jsonl
Last event: 73s ago
Current tool: bash cargo test (running 41s)
```

Use normalized `last_event_at`, not just PID, as the primary live signal.

## Implementation slices

### Slice 1 — live raw transcript, no schema changes if possible

Goal: tailers can see raw output before process exit.

- Update `liveness::run_streaming_with_liveness` to accept callbacks that can fail or introduce a `StreamingSink` wrapper.
- Pi runner: write stdout/stderr lines to live files immediately and flush.
- Claude Code runner: replace `.output()` with streaming spawn and write raw JSONL live.
- Codex runner: ensure JSONL mode (`--json`) and write raw JSONL live.
- Preserve returned `RunnerOutput.stdout` so existing parsers/tests keep working.

Validation:

- Unit test with a shim that emits one line, sleeps, emits second line, then exits; assert file contains first line before exit.
- Regression for long silent child: no-output timeout still kills.

### Slice 2 — normalized event mapping

Goal: raw lines become useful semantic events.

- Add `RunnerEvent` and sink.
- Implement mappers:
  - `map_pi_event`
  - `map_claude_stream_json_event`
  - `map_codex_json_event`
- Append `events.jsonl` and update `status.json` on every semantic event.
- Include API retry/rate-limit events as first-class events.

Validation:

- Fixture-based mapper tests for each runner.
- Tool start/end matching by id where available.

### Slice 3 — discoverability from stores

Goal: operator can find the current live log without spelunking `/proc`.

- Add `runner_invocations` table or equivalent running metadata.
- Insert/update at spawn start, line event, and completion.
- Surface in `tasks status`.

Validation:

- Start a shim runner and assert `stores runs current <task>` resolves before child exits.

### Slice 4 — `stores runs tail`

Goal: human-readable live viewer.

- Implement latest/current invocation lookup.
- Follow `events.jsonl` until completion by default; add `--raw` for raw stdout.
- Render normalized events concisely.

Validation:

- CLI smoke test against a temp runs dir and fixture event stream.

### Slice 5 — Pi/TUI extension integration

Goal: optional live dashboard like `pi-subagents`.

- The extension can watch `runner_invocations`/`events.jsonl`.
- Show active task runners, last activity, current tool, and recent output.
- This is downstream of the durable CLI/filesystem substrate, not the first dependency.

## Design constraints

- Do not break existing post-hoc parsing. Keep accumulating stdout/stderr in memory for now, even while live-writing.
- Never rely on token-delta streams for correctness; they are high-volume and optional.
- Use line buffering. JSON events may straddle byte chunks; parse only complete newline-delimited records.
- Flush live files after each line/event, at least initially. Optimize later if needed.
- Observability must not mask child exit status or payload errors.
- The same abstraction must cover Pi, Claude Code, and Codex; no Pi-only special path.
- Retry/rate-limit events should extend liveness and render explicitly.

## Open questions for review

1. Should `--include-partial-messages` be enabled by default for Claude Code, or gated by config because it increases event volume?
2. Should live raw stdout keep the existing flat `.stores/runs/<session_id>.jsonl` path, or should we migrate all consumers to per-run directories?
3. Should running metadata be a new `runner_invocations` table, or should `agent_runs` be relaxed to allow in-flight rows?
4. What is the minimum viewer needed now: `stores runs tail`, `stores tasks status` enhancement, or both?

## Recommended first commit

Implement Slice 1 for Pi + Claude Code first:

- Pi because T146 is currently exposing the blind spot.
- Claude Code because it already emits canonical stream-json and the change is mechanical.

Leave Codex for the next commit unless its current runner is equally small.
