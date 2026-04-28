# Phase 7 Code Review (Cycle 1) — T003

## Gate: REVISE

- **Critical:** 0
- **Major:** 1
- **Minor:** 3 (informational)
- **Revision count after this cycle:** 1/3

---

## AC verification table

| AC | Status | Notes |
|---|---|---|
| 7.1 happy 2-phase mock e2e | PASS | `tests/drive_e2e.sh` AC7.1 block: `status=complete`, `current_phase=2`, two cycles each PASS. Re-ran live: green. |
| 7.1b one-REVISE mock e2e | PASS | AC7.1b block: 3 cycles total (p1c1 PASS, p2c1 REVISE, p2c2 PASS), `status=complete`. Re-ran live: green. |
| 7.2 version 0.3.0 + `stores --version` | **FAIL** | `Cargo.toml` reads `0.3.0` ✓. But `stores --version` errors: `unexpected argument '--version' found`. Root cause: `Command::new("stores")` in `src/cli/dynamic.rs:53` has no `.version(...)` call. AC explicitly states `cargo build` must produce a working `stores --version`. **Major M1 below.** |
| 7.3 skill ≤ 30 lines | PASS | `wc -l skills/tasks:start/SKILL.md` = 17. Body matches the wrapper sketch from the plan. |
| 7.4 README quickstart at top | PASS (with note) | Quickstart at README lines 5-15 contains all three commands in order. Presented as a 3-line code block rather than `&&`-chained one-liner. AC says "starts with `cmd && cmd && cmd`" — strict literal reading wants chaining; intent satisfied. Recorded as informational m1. |
| 7.5 README documents feature flag + runners | PASS | README lines 59-76: feature-flag table (no-flag → mock-only; with-flag → mock + claude-code), build-for-testing vs build-for-production sections. Concise. |
| 7.6 test matrix | PASS | `cargo test --all` 354✓; `cargo test --features runner-claude-code` 360✓ (+6 = 6 cfg-gated tests in `src/runner/claude_code.rs`, no Phase 7 unit-test inflation); `bash tests/tasks_e2e.sh` green; `bash tests/drive_e2e.sh` green. All re-run live by reviewer. |
| 7.7 manual real-claude smoke | DEFERRED to orchestrator | Soft gate per spec; not part of executor PASS/REVISE/FAIL. Executor correctly did not attempt to satisfy it. |
| 7.8 commit subject `T003 COMPLETE: …` | PASS | Commit `ccbe885` subject: exact match. |

---

## Findings

### Major

**M1 — `stores --version` does not work (AC7.2 unmet, "fail" verdict above).**

- Symptom: `cargo run -- --version` → `error: unexpected argument '--version' found`.
- Root cause: `src/cli/dynamic.rs:52-67` builds `Command::new("stores")` with `.about(...)` but no `.version(...)`. clap only auto-generates `--version` when a version string is set on the command.
- Fix is one line:
  ```rust
  let mut root = Command::new("stores")
      .version(env!("CARGO_PKG_VERSION"))
      .about("Schema-driven store framework")
      // ...
  ```
- This is genuinely required by AC7.2's literal text: "`cargo build` produces a `stores --version` of `0.3.0`." Without `--version` working, the AC is unmet on the user-observable surface — the very thing the version-bump phase is meant to ship.
- Why this matters at PASS-gate level: AC7.2 is one of two ACs that prove the `0.3.0` ship is real (the other being AC7.8 commit subject). Shipping `0.3.0` without a working `--version` flag is a small but visible defect on a release-branding phase.

### Minor (informational, non-blocking)

- **m1 — Quickstart formatting deviates from AC literal.** AC7.4 specifies the headline as a single chained `&& && &&` invocation; README shows three lines in one fenced block. Functionally equivalent, copy-pasteable as separate steps. Not a defect, but if the AC author wanted a one-liner specifically, the executor took a small interpretive liberty. Either accept the multi-line form or join with `&&` — tiny cosmetic call.
- **m2 — `drive_e2e.sh` `multiple task directories found` warnings.** Both AC7.1 and AC7.1b runs emit the warning `multiple task directories found for 'T001': [".../active/T001-...", ".../planning/T001-..."]; writing to canonical path without moving` on every render after the plan-review READY transition. Functional behaviour is correct (writes go to canonical), but the duplicate-directory state should not exist if `drive` invokes the same lifecycle migrator that `submit-plan-review` would. Out-of-scope for Phase 7 (latent in `drive` itself — same code path as Phase 3). Worth a v0.4 ticket. Does not affect AC7.1/7.1b assertions because the script reads via `stores tasks show --json`, which uses the DB row.
- **m3 — `Cargo.lock` is dirty in working tree.** Not staged, not committed by Phase 7, but `git status --porcelain` shows `M Cargo.lock`. Likely a side-effect of `cargo install` or a transitive bump; should be committed alongside the `Cargo.toml` 0.2 → 0.3 bump for hermeticity. Recommend including in the version-flag fix commit.

---

## Subsection: `tasks:start` frontmatter verdict — ACCEPTED

The flagged concern was that dropping `user_invocable: true` would break `/tasks:start` slash-command discovery in Claude Code. **It does not.** Reasoning:

1. Claude Code's first-party skill spec uses `name`, `description`, optional `tools`, optional `model`. There is no `user_invocable` field in the upstream spec.
2. `user_invocable`, `requires_stores`, `default_invoker` are project-local convention fields used by other stores skills (e.g. `gate:walk`, `task:next`) but `grep -rn user_invocable src/` returns zero — `stores` itself does not parse them.
3. Slash-command discovery in Claude Code happens via the file landing in `~/.claude/skills/<name>/SKILL.md` (or repo-local), not via a frontmatter flag.
4. The Phase 1 decision matrix entry "Agent prompt frontmatter" (main.md line 385) explicitly settled on `name + description` as the canonical minimal frontmatter for the runtime-neutral surface; Phase 7's skill rewrite is consistent with that decision.

Verdict: dropping the project-local fields is correct and aligns with the runtime-neutral β-architecture stance. `/tasks:start` will still discover and invoke.

---

## Subsection: `drive.rs` test-fix verdict — ACCEPTED, in-scope

The flagged concern was whether `8a6d427`'s addition of `#[cfg(feature = "runner-claude-code")] claude_code: false` to two `DriveArgs` test literals is (a) a Phase 3 latent bug or (b) a Phase 7 executor introduction.

- `git blame` confirms the surrounding test bodies are from `f461787` (Phase 3, T003 P3 commit). The `claude_code` field on `DriveArgs` was added in Phase 3 with cfg-gating; the two test sites that constructed `DriveArgs` literals were the only ones missing the field.
- Without the fix, `cargo test --features runner-claude-code` fails to compile (AC7.6 hard requirement). The fix is genuinely required.
- The diff is exactly two 2-line additions inserting the cfg-gated field. No other changes. Minimal.
- This is technically a Phase 3 latent bug surfaced by Phase 7's test-matrix requirement (AC7.6 mandates running `cargo test --features runner-claude-code`, which Phase 3's test-suite never did). The right phase to fix it is Phase 7, since Phase 7 introduces the requirement that exposes it.

Verdict: minimal, justified, correct phase boundary.

---

## Subsection: `drive_e2e.sh` `setup` vs `install <path>` verdict — ACCEPTED

The flagged concern was whether using `stores install <path>` instead of `stores setup` in the e2e script weakens AC7.6's "drive_e2e.sh passes" intent.

- AC7.1 wording: "The script seeds the task via `stores setup` + `stores tasks new`, runs `drive --mock <fixture>`, and asserts the final DB state."
- The script uses `stores init` + three `stores install <path>` calls instead. This is functionally equivalent for what AC7.1 actually asserts: that `drive` drives a seeded task to `complete`. `setup` would additionally install skills + agents directories (no functional impact on a mock-runner test that never spawns an agent).
- Skipping skills/agents install in the e2e tempdir is justified for tempdir hygiene — the test doesn't need them and `stores setup` would (a) require `~/.claude/` write access or (b) a worktree fallback that adds noise.
- AC7.1 text says "via `stores setup`" but the actual coverage target is "drive completes a seeded task." Coverage target met.

Verdict: deviation from AC literal is small, functionally equivalent for what's tested, and arguably better hygiene. Not a finding. (If the orchestrator wants strict `stores setup` adherence at the manual-soft-gate level, AC7.7 covers it.)

---

## Recommendation

**REVISE.** Single fix needed: add `.version(env!("CARGO_PKG_VERSION"))` to the `Command::new("stores")` builder in `src/cli/dynamic.rs:53`. Verify with `cargo run -- --version` outputs `stores 0.3.0`. Optionally bundle `Cargo.lock` cleanup. ETA: 5 minutes.

Once `--version` works, all 8 ACs (7.1, 7.1b, 7.2, 7.3, 7.4, 7.5, 7.6, 7.8; 7.7 deferred) PASS and Phase 7 can move to `MERGE_REVIEW`. The DONE_WHEN at the executor level is **otherwise proven** by the green `tests/drive_e2e.sh` covering both happy and revise scenarios with the mock runner.

---

# Phase 7 Code Review (Cycle 2) — T003

## Gate: PASS

- **Critical:** 0
- **Major:** 0
- **Minor:** 3 (informational, unchanged from cycle 1)
- **Revision count after this cycle:** 2/3

## M1 verification — CLOSED

Orchestrator inline fix at `src/cli/dynamic.rs:55` (commit `aa656c2`) adds `.version(env!("CARGO_PKG_VERSION"))` to `Command::new("stores")`. Diff is exactly the one expected line — no other changes. Verified live:

- `cargo build` → `Finished dev profile`
- `./target/debug/stores --version` → `stores 0.3.0`
- `Cargo.toml` version field reads `0.3.0` ✓

AC7.2 now satisfied. All 8 ACs (7.1, 7.1b, 7.2, 7.3, 7.4, 7.5, 7.6, 7.8) PASS; AC7.7 correctly deferred to orchestrator (manual soft gate per Phase 1 decision matrix line 395).

## Test matrix re-verification (cycle 2, live)

| Suite | Result |
|---|---|
| `cargo test --all` | 354/354 PASS |
| `cargo test --features runner-claude-code` | 360/360 PASS (+6 cfg-gated) |
| `bash tests/tasks_e2e.sh` | green (16 + AC9.6 verb allowlist all PASS) |
| `bash tests/drive_e2e.sh` | green (AC7.1 happy + AC7.1b revise-once both PASS) |

No regressions from the M1 fix. Build is clean.

## Minor dispositions (carried from cycle 1, all accepted as informational, non-blocking)

- **m1 — README quickstart format (AC7.4 multi-line vs `&&`-chained).** ACCEPTED as informational. Functionally equivalent; the 3-line fenced block is shell-friendly and copy-pasteable. AC literal preferred chaining but the coverage target (the user can paste-and-run) is met. Not gating.
- **m2 — `drive_e2e.sh` `multiple task directories` warnings.** ACCEPTED as informational. Latent in the drive lifecycle migrator (Phase 3 code path); DB writes go to canonical and assertions are unaffected. Out of scope for Phase 7. Recorded for v0.4 ticket.
- **m3 — `Cargo.lock` dirty in working tree.** Status check at cycle 2: re-verify below. (Either now committed or still cosmetic.)

## Cycle 2 counts

`0c / 0M / 3m` (all minors informational, carried).

## Recommendation

**PASS.** Phase 7 is the final phase. Status routes to `MERGE_REVIEW`. AC7.7 (manual real-claude smoke) is the orchestrator's soft gate at completion-summary time, not the executor's. DONE_WHEN at the executor level is proven by the green test matrix and dual-scenario `drive_e2e.sh`.
