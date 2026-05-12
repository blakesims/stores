# LLM-Off Fake Agent Proposal

**Date:** 2026-05-12
**Type:** note (design proposal)

## Summary

Add a token-free testing mode to `stores` so the full drive cycle (planner → plan-reviewer → executor → code-reviewer → wrap, plus pi-runner equivalents) can be exercised end-to-end **without making LLM calls**. The aim is "modulo real LLM use" — identical envelopes, identical subprocess lifecycle, identical transcripts, identical git/worktree state — so we can re-run the same broken substrate many times per minute, cheaply, and watch the bugs we already know about (watchdog races, REVISE-loop convergence stalls, stale-base rebase loops, capacity-1 integration interlocks) actually fire under controllable timing.

**Recommendation: build a real fake CLI binary (`stores-fake-agent`) and substitute it for `claude`/`node pi_runner.mjs` when `STORES_LLM_OFF=1`. Configurable per-role delay, configurable per-role pass / revise / fail / timeout / crash probabilities, deterministic seed.** Highest realism of the four options considered; the substrate cannot tell it is fake because the seam being swapped is just the binary name on a `Command::new(...)`.

## Details

### Why this matters now

The user-facing pain: the substrate is in a state where the only way to surface remaining bugs is to actually drive it, and driving it costs tokens and minutes per cycle. The bugs in `docs/engine-health.md` and the recent worklog (L116 seeder race, L117, watchdog kill-path, T098 convergence stall) are all timing/lifecycle bugs, not LLM-reasoning bugs. Real LLM calls add token cost and entropy that hide the very races we're trying to flush out. A faithful fake unblocks fast iteration on the substrate.

### What recon found (the seam is small)

All LLM work in `stores` flows through a **single subprocess seam**:

- `src/runner/claude_code.rs:611` — `Command::new(&self.bin)` (default `"claude"`)
- `src/runner/pi.rs:341` — `Command::new(...)` for `node agents/sidecar/pi_runner.mjs`

The substrate **never** calls the Anthropic API in-process. Everything is mediated by a subprocess that emits a tagged-union envelope (`AgentEnvelope` in `src/handlers/drive.rs:475–531`) parsed at three layers (SDK structured output → SAP prose extraction → legacy last-line JSON) by `parse_envelope` (drive.rs:2451–2558). Submit handlers (`compute_submit_plan`, `compute_submit_execute`, etc.) live in `src/handlers/submit.rs`.

Pleasant surprise: `MockRunner` and a `--mock <fixture.json>` flag **already exist** (`src/runner/mock.rs`, drive.rs:923–986). The Runner trait is already plumbed for substitution. We are extending an existing seam, not punching a new one.

### The recommended design

**Master switch:** `STORES_LLM_OFF=1`, read once in `drive.rs` at runner construction (~line 922, before CLI flag / config dispatch). When set, runner construction returns a `FakeRunner` whose `self.bin` points to `stores-fake-agent` (or `stores-fake-pi-agent` for the pi path).

**The fake binary** is a small Rust binary in this repo (or a `bin/` script if we want to ship it shell-fast first):

1. Parses claude-CLI-shaped args: `--append-system-prompt`, `--session-id`, `--output-format`, trailing brief.
2. Reads role from the brief / env / system-prompt (planner / plan-reviewer / executor / code-reviewer / wrap).
3. Sleeps `STORES_FAKE_DELAY_<ROLE>_SECS` (or `STORES_FAKE_DELAY_SECS`, default 5).
4. Rolls a weighted outcome from `STORES_FAKE_OUTCOME_<ROLE>` (defaults: planner 0.9 ready / 0.1 needs-work; reviewer 0.8 pass / 0.15 revise / 0.05 fail; executor 0.85 pass / 0.1 revise / 0.05 crash; wrap 0.95 GO / 0.05 NO_GO). Seedable via `STORES_FAKE_SEED`.
5. Streams a `stream-json` transcript line-by-line to `.stores/runs/<session_id>.jsonl` (the path passed by `--output-format stream-json` is already in the args; the substrate will read it back as a real transcript).
6. For the executor role, makes a **real git commit** on the current worktree branch — `git commit --allow-empty -m "<TID> fake: phase N"` or writes a sentinel file under `tasks/<id>/fake/` and commits it. This is non-negotiable: downstream codex review, rebase, capacity-1 integration lane all assume a real commit exists.
7. Emits the role-keyed envelope as the final stdout message in the exact `AgentEnvelope` shape, exits 0 (or the configured failure exit code).

**Env-var surface (final):**

```
STORES_LLM_OFF=1                              # master switch
STORES_FAKE_DELAY_SECS=5                      # default per-role delay
STORES_FAKE_DELAY_<ROLE>_SECS=...             # PLANNER, PLAN_REVIEWER, EXECUTOR, CODE_REVIEWER, WRAP
STORES_FAKE_DELAY_JITTER_PCT=20               # ±% noise on the delay so timing isn't perfectly uniform
STORES_FAKE_OUTCOME_<ROLE>="pass:0.85,revise:0.10,fail:0.03,timeout:0.01,crash:0.01"
STORES_FAKE_SEED=42                           # deterministic randomness for repro
STORES_FAKE_BIN=stores-fake-agent             # override binary name (escape hatch)
```

`timeout` and `crash` matter as much as `revise` — half the bugs we want to surface are "executor hangs and watchdog fires" or "child exits 137 mid-transcript", and a binary pass/fail won't reproduce them.

### What this might miss

Be honest about what a fake will NOT exercise faithfully:

1. **LLM reasoning quality** — obviously. We are not testing that the planner produces a good plan; we are testing that the substrate handles a plan-shaped envelope correctly. Any bug that depends on real plan content (e.g. submit-plan's T2 phase-count enforcement rejecting a real-but-bad plan) needs a fixture variant on top of the fake, not random pass/fail.
2. **Brief-quality regressions** — if we change the brief format and the real LLM stops understanding it, the fake won't notice; it doesn't read the brief semantically.
3. **Tool-use sequences inside `claude -p`** — the fake doesn't emulate intra-call tool calls (file reads, bash, etc.). For executor specifically this means our `git commit --allow-empty` is a stand-in for "executor edited files and committed"; tests that depend on a specific files_changed payload need fixture overlays.
4. **Real Anthropic API failure modes** — 429s, 529s, deserialization edge cases on streamed JSON, partial response truncation. We can approximate (e.g. truncate the transcript mid-line for one role) but it's an approximation.
5. **Codex / external review** — codex still runs against real diffs. If executor's fake commit is too trivial (one-line file), codex may have nothing to say and the review gate becomes degenerate. Mitigation: fake executor writes a moderately-sized synthetic diff (10–50 lines across 2–3 files) per phase.
6. **Pi runtime parity** — pi has a different subprocess shape; we need a parallel `stores-fake-pi-agent` (or one binary that detects which mode it's invoked in). Easy in principle, easy to forget in practice. Track explicitly.
7. **Drift between fake envelope shape and real envelope shape** — if a future change to `AgentEnvelope` adds a required field, the fake will emit yesterday's shape and pass tests that production won't. Mitigation: the fake imports `AgentEnvelope` from the substrate crate (not a hand-rolled JSON template), so any struct change breaks the fake at compile time. **This is the single most important design decision in the whole proposal.**

### What makes this hard

1. **Role detection from a brief.** The brief is free-form prose. We need a reliable signal — preferably a `STORES_FAKE_ROLE` env var the runner sets on the subprocess, parallel to any existing role-passing mechanism. (Recon shows extra env vars are already plumbed at claude_code.rs:613–615; this is a one-liner.)
2. **Transcript stream-json fidelity.** Real `claude -p --output-format stream-json` emits a specific event sequence (`system`, `assistant`, `result`). The substrate reads this in `claude_code.rs:359` via `append_live_line`. The fake must emit events the same parser accepts. Risk: under-faithful events make some watchdog/health-monitor code path inert. Mitigation: capture one real transcript per role, distill the minimum shape that round-trips through `parse_envelope`, ship that as the fake's template.
3. **Executor's commit + worktree contract.** The executor runs *inside* the task's worktree (drive.rs:1925–1937). The fake needs to know which worktree path it's in (already its `cwd`), what branch it's on (already checked out), and that any commit it makes will be picked up by codex and the integration lane. There is no plan/no-plan mode toggle here — for T1 tasks (contract-is-plan, no planner) the executor is the first agent to run, so its commit must be the diff against base. Manageable but a real edge to handle.
4. **Deterministic randomness across multi-cycle runs.** A REVISE loop fires the same role multiple times. If we just hash (session_id, role), reruns of the same task get the same outcome and the loop never converges. We need (seed, task_id, cycle_count, role) so each cycle gets fresh randomness while the run as a whole is reproducible.
5. **Watchdog-friendly hangs.** To simulate a hang we can't just `sleep` forever — the watchdog timeout might be 10 minutes. The fake should support `outcome=timeout` by sleeping just past the configured watchdog window, which means the fake needs to know the watchdog window. Easier: emit a partial transcript, then sleep, let the watchdog kill us; verify the substrate handles the kill correctly. This is exactly the kind of bug we want to surface.
6. **Multi-runner parity.** Pi and claude-code are two runners today; if we add `stores-fake-agent` we should add it as a third runner variant (`FakeRunner` impl of the `Runner` trait, alongside the existing `MockRunner`), with the fake binary being `FakeRunner`'s subprocess just like `claude` is `ClaudeCodeRunner`'s. Keeps the Runner trait surface symmetric.
7. **Drive code paths that read transcript content semantically.** Anywhere the substrate parses transcript bodies beyond the envelope (e.g. extracting tool calls for review counts, codex prompts) is a place the fake needs to emit plausible content, not just a final envelope. The recon didn't fully map these; an implementation pass will need to grep for `transcript_path` readers.

### Why I recommend this over the alternatives

Four options were considered:

- **A (recommended) — fake CLI binary** substituted for `claude`/`node`. Highest realism: full subprocess lifecycle, real PIDs, real signals, real transcript streaming, real exit codes, real worktree commits. **Surfaces the exact class of bugs we know about.** Cost: ~300–500 LOC for the fake binary + ~50 LOC of runner-selection plumbing.
- **B — in-process `FakeRunner` impl.** Cheaper (~200 LOC), but skips the subprocess entirely. The watchdog races, OS process tree, signal handling, and live transcript streaming are all the surfaces today's bugs live on. An in-process fake makes the substrate look healthier than it is. False confidence is worse than no test.
- **C — cassette/fixture replay.** Extend the existing `--mock` path with recorded envelopes. Deterministic, fast, useful for *parser* regression tests, but kills the randomness and variance we explicitly want for timing-sensitive bug surfaces. Wrong tool for "re-run the broken system".
- **D — A plus B as a fast-path.** Tempting, but adds two backends to keep aligned. Defer until A is working and we feel actual pain from A's per-cycle cost. (Current estimate: A's 5s × 5 roles × 1–3 cycles = 25–75s per full task, which is already a 10×+ speedup over real LLM cycles. We probably don't need B.)

The clinching argument for A is the **subprocess seam is already there and trivially swappable**. We are not building new architecture; we are wiring an alternate binary into an existing trait impl. The realism cost of B is the entire reason we'd build this in the first place — to flush out the timing/lifecycle bugs that an in-process fake will hide.

### Filing path

This proposal is the kind of thing that should be filed as an **observation with tier_hint=T3** and a draft `intent_contract`, then ratified (U1) and auto-promoted into a task. Two reasons:

1. It changes substrate runtime behavior (new env var family, new runner variant, new binary shipped) — exactly the boundary CLAUDE.md says to route through the substrate, not direct-edit.
2. The contract needs scope_in/out — explicitly: scope_in is the fake-binary + runner-selection + env-var surface; scope_out is changing `AgentEnvelope` itself, changing the real claude/pi dispatch, and adding any LLM-quality assertions. That separation prevents scope creep when this lands.

The escape hatch in `CLAUDE.md` § *Session doctrine — 2026-05-06* (direct code edits when substrate is too broken to drive its own fix) probably does NOT apply here: the substrate can still file observations and probably still drive simple tasks. Use the proper path.

## Follow-ups

- [ ] Draft the `intent_contract` (done_when, scope_in, scope_out, decision_matrix) for filing as an observation.
- [ ] Confirm with Blake before filing — this is a non-trivial T3 and worth a sanity check on the contract shape.
- [ ] File via `stores observations add` (or `stores intake add` then route) with tier_hint=T3.
- [ ] Once promoted, the task's plan should include: (a) capture one real transcript per role for the fake template, (b) wire `FakeRunner` as a third Runner trait impl, (c) ship `stores-fake-agent` binary, (d) wire `STORES_LLM_OFF` env switch, (e) end-to-end smoke test driving a real task with `STORES_LLM_OFF=1` and confirming a clean cycle.
- [ ] Decide whether the fake binary lives in this repo (`src/bin/stores-fake-agent.rs`) or as a separate crate. Default: this repo, single crate; importing `AgentEnvelope` directly is the compile-time drift guard and that wants same-crate or workspace.
- [ ] After A ships, decide whether B (in-process fast path) is worth the additional surface, or whether A at ~25–75s per task is already fast enough.
