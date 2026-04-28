# Code Review — Phase 5: `stores tasks status --follow`

- **Cycle:** 1 of 3
- **Reviewer:** code-reviewer
- **Date:** 2026-04-27
- **Commits under review:** 5ee1809 (impl), b5681b0 (log-only)
- **Diff scope:** `git diff 718f5e3..HEAD -- src/ Cargo.toml` → 6 files, +721 / -1

## Gate: PASS

Counts: **0 Critical / 0 Major / 4 minor (cosmetic / coverage)**

## AC verification table

| AC   | Statement                                                                                                                  | Status | Evidence                                                                                                                                                                                                              |
|------|----------------------------------------------------------------------------------------------------------------------------|--------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 5.1  | `stores tasks status <id>` (no `--follow`) prints a single frame and exits 0.                                              | PASS   | Live: `[05:16:56] T001 status=executing phase=2/3 cycle=1 next=executor blocked=false` followed by exit 0 in `/tmp/p5-e2e`. `single_frame_contains_required_fields` + `single_frame_blocked_task` cover formatter.   |
| 5.2  | `stores tasks status --follow <id>` re-prints frames every interval; exits 0 on `complete`/`blocked`.                     | PASS   | Live: setting `T001` to `complete` → first iteration prints frame, terminal-detector returns Ok, exit 0. `bounded_follow_loop_exits_on_complete` covers terminal-state early-exit deterministically.                  |
| 5.3  | `stores tasks status --follow` (no id) prints multi-task frames; exits when none remain or Ctrl-C.                         | PASS   | Live single-shot multi: `[05:17:00] / T001 …/ T002 …` indented as spec. Multi follow with all-terminal exits 0. `multi_frame_contains_both_tasks` + `bounded_follow_loop_multi_task_exits_when_all_terminal` cover.    |
| 5.4  | Ctrl-C is caught cleanly — last frame on screen, exit code 130.                                                            | PASS   | Live: spawn `--follow T001`, `kill -INT $PID`, exit code = 130. Last frame remains visible. SIGINT handler is the only `unsafe` block (see verdict below).                                                            |
| 5.5  | Frames suppress duplicate consecutive lines (same state → no spam); on state change, prints immediately.                  | PASS   | `should_print(prev, new)` is pure and tested across first-frame (None), same-key, status-change, phase-change, cycle-change. `--max-iters 3` live run printed 1 frame (3 iters, no state churn → 2 suppressed).        |
| 5.6  | `cargo test handlers::status` covers single-frame mode + change detection. Follow-loop tests bounded by `--max-iters`.    | PASS   | `cargo test handlers::status` 12/12 (re-run twice — no flakes). `--max-iters` flag is `.hide(true)` on the CLI, defaults to `usize::MAX`. `run_follow_loop(&Path, args)` separation enables tempdir DB injection.     |

Full suite: 338/338 passed (was 326 → +12 new). Re-run twice: no flakes. `cargo build` clean.

## libc + signal handling verdict

**Soundness: OK.**

- `libc` was already pulled in transitively via `tempfile` (dev) + `getrandom` (prod). Promoting it to a direct dep is honest about the new prod usage — `cargo tree -i libc` confirms `stores v0.2.0` is now a direct parent. No new transitive crates added; lockfile drift is one line. Minimal-dep choice (vs `ctrlc` / `signal-hook`) is appropriate.
- `static INTERRUPTED: AtomicBool = AtomicBool::new(false)` is `const`-initialized at compile time — no UB risk from reading before init.
- `extern "C" fn sigint_handler(_: libc::c_int)` does only `INTERRUPTED.store(true, Ordering::SeqCst)`. `AtomicBool::store` compiles to a single relaxed/seq-cst write on x86/aarch64 (no allocation, no locks) → **async-signal-safe**. No prints, no `Mutex`, no allocator calls inside the handler. Soundness gate clears.
- Re-installing the handler on every `run_status` entry is fine — `signal()` replaces; idempotent. Tests calling `install_sigint_handler()` repeatedly do not race because the install is single-threaded per process.

**Portability: OK for advertised targets.**

- `Cargo.toml` declares no platform targets and the README does not advertise Windows. `libc::SIGINT` and `libc::signal` exist on Linux + macOS with consistent semantics → matches stores' actual deployment surface (server linux + macOS dev). On Windows, `libc::signal`'s ABI matches but `SIGINT` semantics differ (delivered on a separate thread); not a Phase-5 concern. No `#[cfg(unix)]` gate added — defensible given no Windows target, but a one-line gate would be cheap insurance for v0.4 (recorded as m1).
- `signal()` is technically deprecated in POSIX in favor of `sigaction()` for richer semantics (race-free re-arm, custom flags). For a "set a flag and return" handler, `signal()` is functionally adequate. Not a finding.

**Loop math (50ms chunk inside interval): OK.**

- The chunk loop on lines 360-372 sleeps `min(remaining, 50ms)` then subtracts a full `chunk` (50ms) from remaining via `saturating_sub`. With `interval = 75ms`: iter 1 sleeps 50, remaining=25; iter 2 sleeps 25 (`min(25,50)`), remaining saturates to 0; loop exits. With `interval = 1500ms`: 30 iterations of 50ms. No busy-spin. SIGINT polled every 50ms during the wait — responsiveness budget is tight enough for AC5.4. The subtraction uses `chunk` rather than `sleep_for` (m4) — functionally equivalent because `remaining < chunk` implies `sleep_for == remaining` and saturating-sub clamps to 0, but it reads sloppily.

## Findings

### Minor (non-blocking)

- **m1 — empty plan.phases array prints `phase=N/0` instead of `N/-`.** Live: a task with `plan = '{"phases":[]}'` and `current_phase=1` renders `phase=1/0`. The reviewer prompt explicitly flagged this edge case (review item 9). The fix is a one-liner in `format_task_line` (or in `total_phases` derivation): treat `Some(0)` as `None`. Recorded for v0.4 polish; does not block — bundled tasks always seed at least one phase.
- **m2 — no test covers `should_print` on `blocked_reason` change.** The `StateKey` includes `blocked_reason` and the docstring at line 24 lists it as a key field, but no test mutates `blocked_reason` while keeping other fields constant. Five `should_print_*` tests exist; a sixth would close the matrix. Coverage gap, not a correctness bug.
- **m3 — `prev_keys.get(id).map(|k| k)` is `clippy::map_identity`.** Two call sites (lines 331 and 344). Cosmetic; `cargo clippy` would catch.
- **m4 — `remaining = remaining.saturating_sub(chunk)` should be `saturating_sub(sleep_for)`.** As described above, equivalent in practice (saturating clamp) but the explicit form reads correctly and would survive future refactors.

### Public API surface

`git diff 5ee1809^..5ee1809 -- src/handlers/status.rs | grep '^+pub'` → 10 new pub items: 3 structs (`StatusArgs`, `TaskState`, `StateKey`), 7 fns. All live in `handlers::status`, which is a binary-internal module — these are visible to tests and to `dispatch.rs` only. No widening of existing surface, no leakage into the install-facing CLI. Acceptable.

### Phase-spec compliance details

- **`status` vs `show` doc comment (review item 8):** Present at the top of `status.rs` lines 1-9 — explicitly contrasts the verbs. Spec from `main.md:279` matches.
- **Frame format (review item 7):** Live output matches `[HH:MM:SS] T001 status=executing phase=2/3 cycle=1 next=executor blocked=false` exactly. Multi-task indented under a leading timestamp line.
- **`--max-iters` hidden:** `cargo run -- tasks status --help` does not list `--max-iters`. `.hide(true)` works.
- **`run_follow_loop(&Path, StatusArgs)` test seam:** Tests inject temp DB path via this fn directly, bypassing `db_path()` which would resolve to project `.stores/`. Confirmed in three loop-tests.

## Phase 6 readiness

`status --follow` is independent of Phase 6 (`guide` handlers). No coupling. Phase 6 can begin with no carry-over from Phase 5.

## Recommendation

**Gate: PASS → advance to Phase 6.** All 6 ACs verified by live e2e + unit tests; signal-handler soundness clears; libc promotion is justified and minimal. Findings are cosmetic; none warrant a revision cycle.
