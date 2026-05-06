# Pi Runner E2e Dogfood

**Date:** 2026-05-06
**Type:** note

## Summary

Implemented and dogfooded a Pi SDK-backed stores runner. The full E2E eventually succeeded: a stores task (`T035`) was driven by Pi SDK agents through executor → code-reviewer → wrap and reached `in_review`, with transcripts showing `openai-codex/gpt-5.5` and `final_output` tool events.

## Details

- Started from `docs/worklog/2026-05-05/04-pi-runner-handover.md`.
- Committed the dirty incoming worktree, branched `pi-runner`, and added a feature-gated `runner-pi` implementation.
- Runner shape:
  - Rust `PiRunner` spawns `node agents/sidecar/pi_runner.mjs`.
  - System prompt, brief, and schema are passed as temp files.
  - Helper uses Pi SDK headlessly with in-memory session/settings.
  - Helper registers `final_output` and Rust extracts `RunnerOutput.structured_output` with source `pi-tool`.
- Confirmed Pi config in this environment:
  - provider: `openai-codex`
  - model: `gpt-5.5`
  - thinking: `low`
- Filed and ratified observation `L110`, auto-promoted to `T034`, and used it to exercise the runner. That exposed two issues:
  1. Pi SDK tool allowlist expects tool names, not tool instances. Fixed helper to pass `tools: ['read', 'bash', 'edit', 'write', 'final_output']`.
  2. `final_output` needed stronger prompting / reprompt loop after normal tool use. Added a bounded reminder loop.
- The first E2E attempt also exposed stores substrate friction: resumed tasks could retain stale `auto-drive` `drive_pid` / `drive_started_at` / dispatch lock state and be immediately re-blocked by the watchdog as `silent_zombie_pid_dead`.
- Filed and ratified `L113`, auto-promoted to `T035`, then fixed `tasks resume` to clear stale auto-drive bookkeeping. Added regression coverage in `handlers::submit::tests::ac5_14_blocked_to_ready_recovery`.
- After the resume fix, `T035` was successfully driven with:
  - `cargo run --features runner-pi -- tasks drive T035 --pi --max-iters 3`
  - final status: `in_review`
  - transcripts under `.stores/runs/` contain `final_output` and `provider/model` evidence for Pi SDK.

Commits made:

- `db3d15a add pi sdk runner`
- `56002c2 clear stale auto-drive state on resume`
- `f267c82 render pi runner dogfood tasks`

Validation run:

- `cargo test --features runner-pi --lib runner::pi::tests` — passed (6 tests)
- `cargo test --features runner-pi --lib handlers::submit::tests::ac5_14_blocked_to_ready_recovery` — passed

## Follow-ups

- Clean up duplicated rendered task projections for `T034` / `T035` if desired; render warned about both active and planning paths existing.
- Consider a focused follow-up for runner flag UX:
  - `--claude-code --pi` currently chooses Claude silently.
  - no-runner error text mentions `--pi` even in non-`runner-pi` builds.
- Investigate T1/contract-is-plan interaction: `T035` reached code review with `plan = null`, and submit-review PASS needed a manually seeded one-phase plan to satisfy existing guard logic.
- Decide whether live Pi E2E evidence should become a formal smoke command/script or remain operator-run due to credentials/model cost.
