# Pi Runner Handover

**Author:** OpenAI coding assistant via Pi
**Date:** 2026-05-05
**Type:** handover / done_when

## Summary

Explore adding a Pi-backed runner to `stores` so autonomous/headless task drive can run non-Claude workers while preserving the existing `RunnerOutput` contract.

## Acceptance Criteria

- Decide runner shape: Rust `PiRunner` spawning a Node/TS helper that uses `@mariozechner/pi-coding-agent` SDK.
- Helper accepts role, cwd/workspace, system prompt, brief, and role JSON schema as files/args.
- Helper runs headlessly with controlled resources: deterministic cwd; no accidental global context/skills/extensions unless explicitly enabled.
- Helper registers a generated terminating `final_output` tool from the role schema, or documents why schema-to-tool generation is deferred.
- Runner extracts a validated final payload from Pi events/tool result and populates `RunnerOutput.structured_output` with `structured_output_source = "pi-tool"` or equivalent.
- Preserve Claude runner unchanged; add feature/flag such as `runner-pi` / `--pi` without regressing `--claude-code` or `--mock`.
- Add tests for success, missing final tool call, malformed payload, non-zero helper exit, and transcript writing.

## Key Notes

- Pi `--mode json` is JSON event output, not Claude-style `--json-schema` validated final output.
- The example Pi structured-output extension is not plug-and-play; it has a fixed demo schema and must be generated/parameterized for `stores` role schemas.
- Best first implementation is a narrow Node helper using Pi SDK, not a raw `pi -p` wrapper.

## Follow-ups

- Read Pi SDK docs in `/home/blake/repos/harnesses/pi-mono/packages/coding-agent/docs/sdk.md`.
- Inspect `src/runner/mod.rs` and `src/runner/claude_code.rs` before implementing.
