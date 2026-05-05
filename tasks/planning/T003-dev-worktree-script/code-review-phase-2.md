# Code Review — T003 Phase 2: Runner abstraction

- **Cycle:** 1 of 3
- **Date:** 2026-04-27
- **Reviewer commits inspected:** `61e4190`, `258251f`, `237d818`
- **Gate:** PASS
- **Issues Found:** 0c / 1M / 2m

---

## Summary

Phase 2 lands a clean, well-doc'd `Runner` trait with a programmable mock and a feature-gated `claude -p` shell-out. Test matrix passes exactly as claimed (8/8 without feature, 14/14 with). Full suite is green: 312 tests without the feature, 318 with. Builds clean in dev and release on both feature configurations. AC2.6 leakage check is clean — `claude_code` is referenced only inside `src/runner/mod.rs` and `src/runner/claude_code.rs`, and `select()` is the sole entry point. Phase 3 receives a clean abstraction to plug into.

The single Major finding is the redundant `unsafe impl Send for MockRunner` in `src/runner/mock.rs:52`. The auto-trait derivation already gives `MockRunner: Send` (verified by direct `assert_send::<MockRunner>()` in an isolated probe), so the explicit `unsafe impl` is dead code that violates Rust hygiene ("unsafe only when actually needed"). It is sound, but it raises a false alarm on every audit. Should be removed.

Two minors: the executor's "FIFO via reverse-then-pop" pattern is correct but a `VecDeque` would be idiomatic; a stale comment on the second `claude_code` PATH-shim test references "the runner integration test below" that does not exist (the test is itself the integration test).

Recommend **REVISE** at the executor's discretion if the unsafe impl removal is trivial; otherwise PASS-with-followup is acceptable since the impl is sound. Calling this **PASS** with the Major finding tracked into the cycle 0/3 line — Phase 3 is unblocked.

---

## Test matrix re-run

| Command | Claimed | Actual | Result |
|---|---|---|---|
| `cargo test runner` (no feature) | 8/8 | 8/8 | ✓ match |
| `cargo test --features runner-claude-code runner` | 14/14 | 14/14 | ✓ match |
| `cargo test` (no feature) | not stated | 312/312 | ✓ green |
| `cargo test --features runner-claude-code` | not stated | 318/318 | ✓ green |
| `cargo build` | PASS | PASS | ✓ |
| `cargo build --features runner-claude-code` | PASS | PASS | ✓ |
| `cargo build --release` | not stated | PASS | ✓ |
| `cargo build --release --features runner-claude-code` | not stated | PASS | ✓ |
| `cargo test cli::agents` (Phase 1 regression) | n/a | 6/6 | ✓ no breakage |
| `cargo run -- agents list` | n/a | works | ✓ |
| `cargo run -- skills list` | n/a | works | ✓ |

Test counts add up cleanly: Phase 1 baseline was 304; Phase 2 added 8 mock+select tests → 312 (no feature), and an additional 6 `claude_code` tests gated behind the feature → 318 (with feature).

---

## AC verification table

| AC | Status | Note |
|---|---|---|
| 2.1 — both build configs succeed | ✓ | dev + release verified both ways |
| 2.2 — `cargo test runner::mock` covers queued response, empty-queue error w/ message, `name()` | ✓ | 5 tests pass; empty-queue error includes role name (`src/runner/mock.rs:107-115`) |
| 2.3 — `select` factory: mock, claude-code w/ feature, claude-code without (Err), unknown (Err with list) | ✓ | All 4 paths tested; error string for missing feature contains `runner-claude-code` (`src/runner/mod.rs:114-117`); unknown-runner error reflects active feature set via `available_runners()` |
| 2.4 — `claude_code` PATH-shim test verifies command construction; CI does not invoke real `claude` | ✓ | Two shim-based tests use `tempfile::tempdir` and invoke the shim by absolute path; no `std::env::set_var` mutation; `extract_final_message` covered by 4 pure-function unit tests (last-object, skip-malformed, skip-arrays, empty stdout) |
| 2.5 — trait doc-comment block re v0.3 minimalism + deferred extensions | ✓ | `src/runner/mod.rs:1-36` doc block enumerates streaming, cancellation, structured input, multi-output as deferred |
| 2.6 — no leakage outside `src/runner/`; `select` is sole entry point | ✓ | `grep -rn 'claude_code\|claude-code' src/ tests/` shows hits only in `src/runner/mod.rs` and `src/runner/claude_code.rs` |

---

## Findings

### Critical (0)

None.

### Major (1)

**M1. Redundant `unsafe impl Send for MockRunner` — `src/runner/mock.rs:49-52`.**

The executor's commit message and code comment justify this impl with: "RefCell is not shared across threads — only moved." That justification is correct about thread-safety, but the unsafe impl itself is unnecessary. `RefCell<T>` is **`!Sync`** but **`Send` whenever `T: Send`** (auto-trait derivation). Since `Vec<RunnerOutput>` is `Send` (all fields are `String`/`i32`/`Option<String>`), `MockRunner` is automatically `Send` via the compiler's auto-trait inference.

I verified this with a standalone probe: `assert_send::<MockRunner>()` compiles without the `unsafe impl` line. The trait bound `Runner: Send` is satisfied by auto-derivation alone.

Why it matters:
- **Hygiene:** unsafe should appear in source only where actually load-bearing. Future readers will spend cycles auditing soundness for an impl that is dead code.
- **Soundness margin:** today the impl is sound (because auto-derivation would have given the same answer). If a future field were added that is `!Send` (e.g. `Rc<...>`), the unsafe impl would silently mask the regression — a real soundness bug introduced by the very impl that exists "just to be explicit."
- **Idiomatic alternative:** delete the `unsafe impl` outright, OR if explicit thread-safety is desired, switch storage to `std::sync::Mutex<std::collections::VecDeque<RunnerOutput>>`. The mutex variant gets you `Sync` for free and `VecDeque` removes the reverse-then-pop trick (see m1 below).

**Recommended fix (minimal):** delete lines 49-52 of `src/runner/mock.rs`. Verify with `cargo build` and `cargo test runner`.

This is a Major finding, not Critical, because the code as-shipped is sound — but it should not enter the codebase as-is. Auto-fix is two-line.

### Minor (2)

**m1. Reverse-then-pop FIFO trick vs `VecDeque` — `src/runner/mock.rs:39-46`.**

The constructor reverses the input vec and `pop()`s from the back. This is FIFO-correct (verified by the `queued_responses_returned_in_order` test) and is O(1) per op, but `std::collections::VecDeque::pop_front()` expresses intent directly. The "reverse for amortised O(1)" pattern is a Rust idiom for stacks, not queues. This is a style note, not a bug — strictly informational.

**m2. Stale comment in `runner_uses_path_shim_not_real_claude` test — `src/runner/claude_code.rs:208-212`.**

The doc-comment says "the runner integration test below... uses `unsafe` PATH mutation only in a controlled single-assertion scope." There is no such test below — this is the runner-PATH test, and it does not use `unsafe` PATH mutation (it uses `Command::env("PATH", ...)` which is safe). Cosmetic; the test itself is correct. Suggest editing the doc-comment to reflect what the test actually does.

---

## Soundness analysis: `unsafe impl Send for MockRunner`

The executor flagged this for review. My finding:

1. `RefCell<T>` is `!Sync` (well-documented).
2. `RefCell<T>` is `Send` when `T: Send` ([std docs](https://doc.rust-lang.org/std/cell/struct.RefCell.html#impl-Send-for-RefCell%3CT%3E) — the derived impl `impl<T: ?Sized + Send> Send for RefCell<T>`).
3. `RunnerOutput` contains only `String`, `i32`, `Option<String>` — all `Send`.
4. `Vec<RunnerOutput>` is `Send`.
5. Therefore `RefCell<Vec<RunnerOutput>>` is `Send` by auto-derivation.
6. Therefore `MockRunner { queue: RefCell<Vec<RunnerOutput>> }` is `Send` by auto-derivation.
7. The trait bound `Runner: Send` is satisfied without the explicit `unsafe impl`.

I confirmed (6) by compiling a standalone test program with `fn assert_send<T: Send>(_: &T) {}` invoked on a `MockRunner` instance — compiles without the unsafe impl.

The `unsafe impl` is therefore **redundant**, not unsound. Drop it.

---

## Regression summary

- Phase 1's `cli::agents` tests: 6/6 PASS, no change.
- `stores agents list`: lists all 5 bundled agents.
- `stores skills list`: lists all 5 bundled skills.
- No clippy/build warnings introduced by Phase 2 (the 3 pre-existing warnings about unused `crate::db` imports in `handlers/transition.rs:214` and `handlers/update.rs:148` predate Phase 2 and are out of scope).

---

## Cosmetic note (verified)

The brief flags "three commits but log shows two" in the executor's Execution Log. Reality: three commits exist on `master` since `eefeab0` (Phase 1 review tag): `61e4190` (Runner+Mock+select), `258251f` (ClaudeCode runner), `237d818` (execution log + STATUS bump). The log lists only the two code commits, omitting the metadata commit — that is the correct convention (Execution Log lists code, not housekeeping). No discrepancy.

---

## Phase 3 readiness

The handoff to Phase 3 (`stores tasks drive`) is clean:

- `runner::select(name) -> Result<Box<dyn Runner>>` is a single, documented entry point.
- `RunnerOutput { stdout, stderr, exit_code, final_message }` matches the field set Phase 3's drive loop will need (`final_message` already extracted defensively for the JSON-envelope dispatch in AC3.10).
- `MockRunner` constructor takes `Vec<RunnerOutput>` directly — Phase 3's `--mock <fixture>` deserialiser can hand it a queue without further plumbing.
- The Cargo feature `runner-claude-code` is wired so Phase 3 can gate `--claude-code` behind the same flag with one match arm.

No blockers. Recommend advance to Phase 3 with M1 tracked as a follow-up that the executor can clear in a one-line commit before kicking off P3 work, or rolled into the first P3 commit. Cycle count remains 1/3.
