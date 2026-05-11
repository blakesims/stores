# Live Runner Streaming Plan

**Date:** 2026-05-11
**Type:** note / implementation plan
**Context:** Direct-on-main substrate rescue work. This does not overlap T146's feature branch; it is engine observability infrastructure needed to diagnose current runner wedges.

## Summary

Implement live, runner-agnostic streaming for stores task runners so operators can see what an executor/reviewer/planner is doing while it runs, instead of waiting for post-exit transcripts. The target experience is similar to `pi-subagents`: live JSONL/event logs, durable progress snapshots, and a tail/view command that shows assistant text, tool starts/ends, retries, usage, and last activity.

The first slice should be intentionally narrow but must still make live paths discoverable: preserve current runner behavior, allocate a pre-spawn invocation context, and change transcript writing from "buffer until child exits" to "append and flush as stdout/stderr lines arrive". Then layer normalized events and UI/TUI affordances on top.

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

### 1. Add a pre-spawn runner invocation context and event sink

Live streaming needs one architectural change up front: `drive` must know the session id and live path before it blocks in `Runner::spawn`. Today each runner mints `session_id` internally and returns only after completion, so raw live files could exist without any stable way for `stores tasks status` or `stores runs tail` to find them.

Introduce a small common layer, not a giant rewrite:

```rust
pub struct RunnerInvocationContext {
    pub session_id: String,
    pub live_dir: PathBuf,
    pub flat_transcript_path: PathBuf,
    pub runner: String,
    pub display_id: String,
    pub phase: i64,
    pub cycle: i64,
    pub role: String,
}

pub enum RunnerEvent {
    Spawned { pid: u32 },
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

`drive` should allocate `RunnerInvocationContext` immediately before spawn and pass it into the runner/sink. The runner emits `Spawned { pid }` after `cmd.spawn()` so PID recording does not rely on post-hoc process inspection. Keep the sink append-only and best-effort where possible. The sink should never hide runner failures, but observability write failures should be explicit and diagnosable.

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

Compatibility decision: keep the existing flat transcript path (`.stores/runs/<session_id>.jsonl`) as the canonical raw stdout transcript and write it live instead of post-exit. Add the per-session directory for normalized events and snapshots. Existing telemetry/tests can continue to point at the flat path while new live viewers use `<session_id>/events.jsonl` and `<session_id>/status.json`.

### 3. Persist running-attempt metadata before completion

Current `agent_runs` rows are inserted after the runner returns, so operators cannot discover live paths from the DB while a run is active.

Avoid relaxing `agent_runs` in the first pass: existing code assumes `agent_runs` is post-completion telemetry with non-null `ended_at` and `exit_code`.

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

Insert the `runner_invocations` row before calling `Runner::spawn`, update it on `Spawned`, line/event heartbeat, current tool changes, and completion. Then later fold or relate this to `agent_runs` once stable.

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

### Slice 1 — pre-spawn context + live raw transcript for Pi and Claude Code

Goal: tailers can discover and see raw output before process exit.

- Add `RunnerInvocationContext` allocation in `drive` immediately before the runner spawn.
- Keep `.stores/runs/<session_id>.jsonl` as the flat raw stdout transcript and create `<session_id>/` for live status/events.
- Insert a `runner_invocations` row before `Runner::spawn` blocks.
- Update `liveness::run_streaming_with_liveness` to accept a streaming sink/callback policy, or introduce a `StreamingSink` wrapper around it.
- Callback semantics:
  - all stdout/stderr lines, including post-`wait()` drained lines, must pass through the same sink path;
  - sink write failures should surface as runner infrastructure errors unless explicitly downgraded with a warning;
  - heartbeat/status updates should happen after durable append succeeds when possible.
- Pi runner: write stdout/stderr lines to live files immediately and flush.
- Claude Code runner: replace `.output()` with streaming spawn and write raw JSONL live.
- Preserve returned `RunnerOutput.stdout` so existing parsers/tests keep working.
- Do **not** enable Claude `--include-partial-messages` by default in Slice 1.
- Defer Codex JSON mode to a dedicated compatibility slice.

Validation:

- Unit test with a shim that emits one line, sleeps, emits second line, then exits; assert file contains first line before exit and `runner_invocations` resolves before exit.
- Unit test that late drained lines after child exit are written through the same sink.
- Regression for long silent child: no-output timeout still kills and writes a terminal status.

### Slice 2 — normalized event mapping for Pi and Claude Code

Goal: raw lines become useful semantic events without changing downstream final-output behavior.

- Add `RunnerEvent` and sink.
- Implement mappers:
  - `map_pi_event`
  - `map_claude_stream_json_event`
- Append `events.jsonl` and update `status.json` on every semantic event.
- Include API retry/rate-limit events as first-class events.
- Track current tool by id where available, and render unknown tool end events without failing.

Validation:

- Fixture-based mapper tests for Pi and Claude Code.
- Tool start/end matching by id where available.
- Claude `system.api_retry` fixture extends liveness and renders as retry, not as silence.

### Slice 3 — discoverability from stores

Goal: operator can find the current live log without spelunking `/proc`.

- Complete the `runner_invocations` table/query layer if Slice 1 used a minimal internal form.
- Insert/update at spawn start, PID observation, line event, semantic event, and completion.
- Surface in `tasks status`.

Validation:

- Start a shim runner and assert `stores runs current <task>` resolves before child exits.
- Assert `tasks status` reports live path, last event age, runner, role, and PID when known.

### Slice 4 — `stores runs tail`

Goal: human-readable live viewer.

- Implement latest/current invocation lookup.
- Follow `events.jsonl` until completion by default; add `--raw` for raw stdout.
- Render normalized events concisely.

Validation:

- CLI smoke test against a temp runs dir and fixture event stream.

### Slice 5 — Codex JSON compatibility

Goal: add Codex semantic streaming without breaking existing Codex consumers.

- Audit current Codex downstream parsing, especially external-review verdict/final-message handling.
- Add `--json` only after preserving or deriving the existing final human text expected by callers.
- Map Codex JSONL events:
  - `item.started` / `item.completed` with `command_execution` → tool start/end
  - `item.completed` with `agent_message` → assistant text
  - `turn.completed.usage` → usage
  - `turn.failed` / `error` → error events
- Keep raw stdout live transcript compatibility.

Validation:

- Existing external-review parsing remains green.
- Codex JSON fixtures map to normalized events.

### Slice 6 — Pi/TUI extension integration

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
- `--include-partial-messages` for Claude Code is opt-in/config-gated, not default, until event volume and UI rendering are proven.
- Codex `--json` is opt-in/deferred until compatibility with existing final-message/verdict consumers is proven.

## Resolved decisions from plan review

1. Claude `--include-partial-messages` is config-gated/off by default.
2. Keep the existing flat `.stores/runs/<session_id>.jsonl` raw transcript path for compatibility; add per-session live directory alongside it.
3. Use a new `runner_invocations` table rather than relaxing `agent_runs` in the first pass.
4. Drive must allocate/pass pre-spawn invocation context so live paths are discoverable while the runner is active.
5. Runner implementations must emit `Spawned { pid }` or equivalent through the sink; PID is not available from the current post-completion `RunnerOutput` API.

## Open questions for review

1. Should Slice 1 include both `stores runs current` and `tasks status` live path rendering, or is one enough for the first debugging pass?
2. Should sink write failure abort the runner as infrastructure failure in all cases, or warn-and-continue for secondary files while requiring the flat transcript append to succeed?
3. What retention/cleanup policy should apply to per-session live directories once flat transcript compatibility remains?

## Recommended first commit

Implement Slice 1 for Pi + Claude Code first:

- Add pre-spawn invocation context and `runner_invocations` row creation.
- Keep the existing flat raw transcript path and live-write it.
- Pi first because T146 is currently exposing the blind spot.
- Claude Code second because it already emits canonical stream-json and the change is mechanical.

Leave Codex JSON mode for the compatibility slice, not the first commit.
