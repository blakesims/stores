# Code Review — Phase 4: `stores setup` quickstart

- **Cycle:** 1 of 3
- **Reviewer:** code-reviewer
- **Date:** 2026-04-27
- **Commits under review:** 718f5e3, e429027 (log-only)
- **Diff scope:** `git diff 04cc95c..HEAD -- src/` → 5 files, +224 / -5

## Gate: PASS

Counts: **0 Critical / 0 Major / 3 minor (cosmetic)**

## AC verification table

| AC   | Statement                                                                                                                                  | Status | Evidence                                                                                                                                                                                                                                                                       |
|------|--------------------------------------------------------------------------------------------------------------------------------------------|--------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 4.1  | Fresh `stores setup` creates `.stores/db.sqlite`, `.stores/manifest.yaml`, all 3 stores, all 5 skills, all 5 agents.                       | PASS   | Manual run in `/tmp/stores-review.vQc9Cy`: db + manifest created; manifest.yaml lists `observations`, `gate`, `tasks` (with correct scopes `worktree/worktree/repo`); 5 skills + 5 agents on disk. Test `fresh_bootstrap_creates_all_artifacts` (setup.rs:134) covers the same. |
| 4.2  | Re-run is idempotent, exits 0, prints idempotency notes per layer.                                                                         | PASS\* | Re-run exits 0; init prints `Already initialized at …`; stores layer prints `Already installed: <name>`; skills/agents layers print banner only (silent — see m1). Test `idempotent_rerun_succeeds` (setup.rs:181) covers exit-0.                                              |
| 4.3  | `--global` writes skills+agents to `~/.claude/`; store DB local.                                                                           | PASS   | Manual run with `HOME=tmp` + `--global` lands skills in `$HOME/.claude/skills/` and agents in `$HOME/.claude/agents/`; `.stores/` stays in CWD. Real `~/.claude/agents/` not polluted (no `planner.md` etc. there).                                                            |
| 4.4  | Partial-state recovery: missing layer is restored without erroring or wiping others.                                                       | PASS   | Manual: deleted `.claude/agents/`, re-ran `setup`; init/stores/skills layers said already-installed, agents reinstalled fully; exit 0. Idempotency at each layer is the mechanism (no special composer logic — and that's fine).                                              |
| 4.5  | `cargo test cli::setup` covers fresh-bootstrap and idempotent-rerun in tempdir.                                                            | PASS   | `cargo test cli::setup` → 2/2. Both tests use `with_isolated_env` to set CWD+HOME into a unique tempdir under the shared `paths::test_cwd_lock` mutex; HOME isolation prevents real `~/.claude/` pollution.                                                                   |
| 4.6  | Layer error aborts subsequent layers and surfaces the underlying error.                                                                    | PASS   | setup.rs:23/40/54/64 all use `?` for short-circuit; the bundled-stores loop only swallows `"already installed"` substrings (line 37) and re-raises everything else. No half-installed silent state. (Code-read evidence; no test simulates a forced layer failure but logic is straightforward.) |

\* m1 below: skills/agents layers print no per-item idempotency message on re-run.

## paths.rs deviation verdict — ACCEPT

The executor promoted `paths.rs::tests::cwd_lock` (previously private) to `pub(crate) fn paths::test_cwd_lock()` under `#[cfg(test)]`, so `cli::setup::tests` can share the same process-wide mutex. Every callsite in `paths::tests::*` now goes through the shared form (paths.rs:141 `fn cwd_lock() -> _ { test_cwd_lock() }`). No divergence.

**Why this is the minimum exposure:**

1. **Existing pattern check** — `cli::skills::tests` and `cli::agents::tests` *don't* mutate CWD. They use a `make_tmp_base()` + `install_to(name, &base, …)` helper that takes the base dir explicitly, so the production `current_dir()` calls in `skills::skills_dir()` / `agents::agents_dir()` are bypassed. That trick won't work for setup, which must drive the production `init::run` / `skills_run` / `agents_run` end-to-end — all three resolve their target via `current_dir()` internally. Setup tests **have to** mutate CWD.
2. **`paths::tests` already had this exact mutex** (private). The change is a one-liner exposing it for cross-module tests via `pub(crate)` + `#[cfg(test)]` — invisible in release builds, invisible to external crates, and the smallest possible widening.
3. **Alternatives considered & rejected:**
   - `serial_test` crate: adds a dev-dep and an attribute on every CWD-mutating test. Heavier than a 5-line shared mutex.
   - `--test-threads=1`: pollutes the dev workflow and slows CI for no real win.
   - Co-locating in `cli::skills::tests`: doesn't help — setup needs the *production* path, not the explicit-base helpers.
   - Refactoring `init::run`/`skills_run`/`agents_run` to take an explicit base: deeply invasive for a test-only need.

**Public-API check:** the only exposed widenings in the diff are `pub mod setup`, `pub fn setup::run(global)`, and `pub(crate) fn paths::test_cwd_lock` (cfg-test). No release-build surface is widened. Confirmed via `git diff 04cc95c..HEAD -- src/ | grep "^[+-].*pub"`.

## Findings

### Critical (0)
*None.*

### Major (0)
*None.*

### Minor (3, all cosmetic — non-blocking)

**m1. Skills/agents layers are silent on idempotent re-runs** — `src/cli/setup.rs:50,60` calls `skills_run`/`agents_run` with `Install { all: true, … }`. The downstream `install_all` (`src/cli/skills.rs:135-140`, `src/cli/agents.rs:148-153`) passes `silent_if_same: true`, which suppresses the `Already installed: <path>` notice. Result: a re-run prints
```
=== skills ===
=== agents ===
Setup complete.
```
with zero per-item feedback. The spec says "prints idempotency notes per layer" — init and stores comply, skills and agents don't. Not a correctness bug (exit 0, same artifacts). Could be addressed by either (a) plumbing a `silent: bool` argument into `SkillsCmd::Install`/`AgentsCmd::Install` so setup forces non-silent, or (b) printing "All 5 skills already installed" once when nothing wrote.

**m2. `with_isolated_env` is not panic-safe** — `src/cli/setup.rs:94-123`. CWD/HOME are restored *after* the closure body, so an `assert!` panic inside the body skips restoration; the next test (and any later CWD-using test in the binary) inherits a CWD pointing into a leaked tempdir. The shared `test_cwd_lock` is poison-recovered (`unwrap_or_else(|e| e.into_inner())`), so the suite won't deadlock, but state across tests can be inconsistent. Wrap the save-restore in a `Drop` guard struct, or use `std::panic::catch_unwind`. Low priority — current tests don't fail.

**m3. Substring match `"already installed"` is brittle but currently safe** — `src/cli/setup.rs:37`. The check accepts any `anyhow::Error` whose Display contains the literal substring. Today only `install_bundled` produces such messages and only for genuine idempotency cases (install.rs:135, 143). If `install_bundled` ever grows a non-idempotent error mentioning "already installed" (e.g. "manifest already installed an incompatible version") it would be silently swallowed. A typed enum (`InstallError::AlreadyInstalled`) or a sentinel return (`Result<InstallOutcome>` with `Installed | AlreadyInstalled`) would be more robust. Punch-list item, not a defect.

## Test results

```
cargo build                  → clean
cargo build --release        → clean (no warnings)
cargo test cli::setup        → 2/2 PASSED
cargo test                   → 326 passed; 0 failed; 0 ignored
```

Pre-Phase-4 was 324 passing. New tests: `fresh_bootstrap_creates_all_artifacts`, `idempotent_rerun_succeeds`. Diff: +2, no regressions. Real `~/.claude/agents/` confirmed clean of bundled names (planner.md/executor.md/etc. absent — `--global` test isolation works).

## Banner-format check

setup.rs prints `=== init ===`, `=== stores ===`, `=== skills ===`, `=== agents ===` in order, with a final `Setup complete.`. Manual run confirms ordering and consistency. Cosmetic only.

## Public-API surface delta

```
+pub mod setup;                                       // src/cli/mod.rs
+pub fn run(global: bool) -> Result<()>               // src/cli/setup.rs
+pub(crate) fn test_cwd_lock() -> ...                 // src/paths.rs (cfg-test only)
```

Three additions. The first two are required to wire the new feature; the third is the discussed test-only mutex. Nothing else widened. `BUNDLED_STORE_NAMES` was already `pub` pre-Phase-4 (`src/cli/dynamic.rs:13`).

## Phase-3 punch-list carry

The Phase 3 reviewer flagged `LOCK_WINDOW_SECS = 300` as a redefinition vs. `submit.rs`. Out of scope for Phase 4 (no change to either file in this commit). Defer to a later cleanup phase.

## Recommendation

**PASS.** Set Status → `EXECUTING_PHASE_5`. The composer is small, the tests pass, the deviation is justified, and the AC table is fully green (with one cosmetic asterisk on AC4.2 that the executor's own notes already acknowledge). Phase 5 (status --follow + gate guide) can begin.
