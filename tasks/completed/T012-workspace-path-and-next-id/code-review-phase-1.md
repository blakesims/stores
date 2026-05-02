# Code Review — T012 Phase 1

**Reviewer:** code-reviewer agent
**Date:** 2026-05-02
**Gate:** REVISE

## Verdict

**REVISE.** The implementation is structurally correct on every load-bearing axis: the SDK guard is preserved, validation lives in drive with a contextualised error, the trait signature ripples cleanly through every call site, MockRunner reuses the existing `unsafe impl Send`, and scope is exactly the seven files the plan promised. However, the two new runner-level tests (`workspace_path_canonicalised_when_some` and `workspace_path_falls_back_to_inherited_when_none`) introduce a real test-suite flake at roughly 50% under the default parallel-test harness, violating AC1.8 ("`cargo test --all-features` passes with no skips and no new warnings beyond what existed pre-task").

This is a **minor REVISE** — orchestrator-fixable in <30 lines via a single test pattern change, mirroring the existing `command_construction_and_final_message_parsing` pattern at `src/runner/claude_code.rs:471–486`.

---

## Independent Test Counts

- `cargo build --all-features` — clean, no new warnings (3 pre-existing `unused import: crate::db` warnings unchanged).
- `cargo test --all-features` — **flaky**: 5 runs total, observed 487/487 pass three times and 485/487 pass two times. Failures are always on `runner::claude_code::tests::workspace_path_canonicalised_when_some` and/or `workspace_path_falls_back_to_inherited_when_none` (1 or 2 of them).
- `cargo test --all-features -- --test-threads=1` (serial) — 487/487 pass deterministically.
- Pre-task baseline (`git stash` + `cargo test --all-features`) — 5/5 runs clean at 481/481. **The flake is new.**
- `bash tests/tasks_e2e.sh` — passes.
- `bash tests/drive_e2e.sh` — passes.

---

## AC Coverage

| AC | Status | Notes |
|---|---|---|
| AC1.1 — schema delta | PASS | Exact one-line addition adjacent to `branch` (line 9). |
| AC1.2 — trait signature + doc | PASS | `workspace_path: Option<&str>` is the last positional. Trait doc at `src/runner/mod.rs:113-118` references the SDK guard at `claude_code.rs:305-306`. |
| AC1.3 — canonicalize-once + inline comment | PASS | `claude_code.rs:308-317`. Single `match` expression at function entry; comment at line 310 references file-top doc lines 33-38. The plan asked for "references `claude_code.rs:305-306` (or the equivalent post-edit lines) and the SDK footgun" — file-top doc lines 33-41 are the equivalent post-edit lines and explicitly describe the footgun. Acceptable. |
| AC1.4 — None branch falls through | PASS | `None => resolve_cwd()?`; existing `cwd_canonicalised_before_spawn` unchanged. |
| AC1.5 — set+missing → loud error | PASS | `drive.rs:605-613`. Error message includes both display_id (`[T001]`) and the missing path. Pre-spawn check; runner queue undrained (verified by AC1.6 test). |
| AC1.6 — four drive tests | PASS | All four tests exist with the right shape: |
| | | • `workspace_path_unset_uses_inherited_cwd` — checks `paths.iter().all(|p| p.is_none())`. |
| | | • `workspace_path_set_and_exists_canonicalizes` — note in test correctly explains MockRunner records the raw string (canonicalization verified at runner level). |
| | | • `workspace_path_set_but_missing_errors_at_spawn` — asserts both `T001` and the missing path are in the error message AND `runner.remaining_count() == 1`. |
| | | • `workspace_path_canonicalize_stable_across_spawns` — drives through 5 spawns and asserts byte-equality. |
| AC1.7 — runner-level tests | **FAIL** | Tests exist and follow the existing shim pattern, but they assert on shim **output** (the cwd from `pwd`), so when the parallel-test PATH race fires and the real `claude` CLI is invoked instead of the shim, the tests fail with `"Not logged in · Please run /login"`. See "Flake details" below. |
| AC1.8 — full test suite passes | **FAIL** | Suite is flaky at ~50% under parallel execution (the default). Pre-task baseline was 5/5 clean. |
| AC1.9 — e2e scripts | PASS | Both pass. |

---

## Flake Details (the blocker)

The two new tests at `src/runner/claude_code.rs:840-908` use this pattern:

```rust
let original_path = std::env::var("PATH").unwrap_or_default();
let new_path = format!("{}:{}", dir.path().display(), original_path);
let runner = ClaudeCodeRunner::new();
unsafe { std::env::set_var("PATH", &new_path); }
let result = runner.spawn("planner", "sys", "brief", None, Some(&workspace));
unsafe { std::env::set_var("PATH", &original_path); }
```

This is the same pattern as the pre-existing `session_id_is_valid_uuid_v4_propagated_to_output` (lines 666-679) and `json_schema_arg_is_passed_inline` (lines 720-731). Those existing tests tolerate the race because they only assert on `result.expect(...)` (and check session_id, which the runner generates regardless of whether the shim or real claude was called).

The new tests are **stricter** — they assert on the cwd printed by the shim's `pwd`. When another parallel test does `std::env::set_var("PATH", &original_path)` in the brief window between this test's `set_var(new_path)` and `runner.spawn(...)`, the real `claude` binary on `PATH` is invoked instead of the shim. The real claude prints `"Not logged in · Please run /login"` to its result event, and the cwd assertion fails.

The pre-task code already documented this hazard at line 471-473:
> "We cannot mutate PATH for the current process safely in a parallel test environment, so we invoke via Command directly with PATH set."

The `command_construction_and_final_message_parsing` test at line 452 follows that safer pattern. The new tests should too.

**Suggested fix (≤30 lines, orchestrator-inline):** Refactor both new tests to use `Command::new(shim_path).env("PATH", &new_path)` directly, replicating the relevant `ClaudeCodeRunner::spawn` command-construction logic, OR — better — add a test-only constructor or method to `ClaudeCodeRunner` that allows overriding the binary path / PATH per-call, eliminating the global-state mutation entirely. The first option is mechanical and preserves the existing test scope; the second is cleaner but expands the runner API for testability and is closer to a substantial revision. Either way, the canonicalize-once contract being tested is small enough that a `Command::new(shim_path)`-style replication is the lower-risk path.

If the orchestrator prefers a shorter detour: marking the two new tests `#[ignore]` and routing AC1.7 verification to the existing `cwd_canonicalised_before_spawn` test (which already verifies the `None` branch's `resolve_cwd()` path deterministically) — plus a manually-run note that the `Some(p)` branch is exercised by the 4 drive tests at AC1.6 — would land Phase 1 cleanly. AC1.7 wording would need to be relaxed accordingly. **Not recommended** — the runner-level canonicalization assertion is real value to keep; better to fix the test pattern.

---

## SDK Guard Preservation — Verified

Decision Matrix row 5 + AC1.3 are the load-bearing requirements. Verified structurally at `claude_code.rs:308-317`:

- Single `let cwd = match workspace_path { ... }` expression at function entry (line 312).
- `Some(p)` branch: `PathBuf::from(p).canonicalize().with_context(...)?` — single canonicalize call.
- `None` branch: `resolve_cwd()?` — unchanged from pre-task.
- `cwd` is then passed once to `cmd.current_dir(&cwd)` at line 320 and once to `write_transcript(&cwd, ...)` at line 376.
- No inner site re-canonicalizes or re-resolves cwd. The canonicalize-once contract is honored mechanically, not just by convention.
- Inline comment at line 310: `// Canonicalize once at spawn entry; the SDK silently mints a fresh session if cwd differs across resume calls (see lines 33-38).` — references the file-top doc that explains the SDK footgun. Reads correctly at the point of risk.
- File-top doc at lines 33-41 was updated to describe both branches.

This is good. The SDK guard is preserved both structurally and by inline annotation.

---

## Validation Location — Verified

Decision Matrix row 5 puts validation in drive. Confirmed at `drive.rs:605-613`:

```rust
if let Some(p) = workspace_path {
    if !std::path::Path::new(p).exists() {
        anyhow::bail!(
            "[{display_id}] workspace_path '{p}' does not exist; \
             set a valid path or remove the field"
        );
    }
}
```

- Pre-spawn: the runner is never invoked when the path is missing, confirmed by `workspace_path_set_but_missing_errors_at_spawn` asserting `runner.remaining_count() == 1` (queue undrained).
- Error message includes display_id and the missing path. Helpful suggestion in the error tail ("set a valid path or remove the field").
- No silent fallback to inherited cwd.

The runner's own `canonicalize().with_context(...)?` at `claude_code.rs:314-315` is the implicit second line of defence for callers that bypass drive — its message ("workspace_path canonicalize failed: '<path>'") is less contextual than drive's but still surfaces the failure.

---

## Trait Signature Ripple — Verified

`grep -rn "\.spawn(" src/ --include="*.rs"`:

- `src/handlers/drive.rs:624` — passes the extracted `workspace_path`.
- `src/handlers/guide.rs:274,347` — both pass `None` (Decision Matrix row 10).
- `src/runner/mock.rs:30,113,118,126,141,142,149,174` — all pass `None` (mechanical).
- `src/runner/claude_code.rs:676,728,898` — pass `None`. Line 863 passes `Some(&workspace)` (the new `workspace_path_canonicalised_when_some` test).
- No call site missed; no commented-out spawn.

---

## MockRunner Discipline — Verified

- `workspace_paths_seen: RefCell<Vec<Option<String>>>` — correct type (`Option<String>` mirrors the parameter type, allowing the test to distinguish "set" from "unset").
- Single `unsafe impl Send` at `src/runner/mock.rs:68`. Q3 from plan-review resolved: no second `unsafe` block added.
- `workspace_paths_seen()` accessor at line 60-62 returns `self.workspace_paths_seen.borrow().clone()` — clones out of the RefCell, so callers cannot trigger borrow panics by holding a reference across a subsequent `spawn`.

---

## Scope Discipline — Verified

`git diff --name-only HEAD`:
```
src/handlers/drive.rs
src/handlers/guide.rs
src/runner/claude_code.rs
src/runner/mock.rs
src/runner/mod.rs
stores/tasks/schema.yaml
tasks/active/T012-workspace-path-and-next-id/main.md
```

Exactly the 7 files the plan promised. No `src/cli/dynamic.rs`, `src/cli/dispatch.rs`, `src/handlers/next_id.rs`. No `WORKFLOW_VERBS` change. No edits to `tasks/CLAUDE.md`, `docs/philosophy.md`, or unrelated handlers. Phase 2 territory is untouched.

---

## Symmetry with `branch` — Verified

`drive.rs:557-560` (existing branch extraction):
```rust
let branch = entry
    .get("branch")
    .and_then(|v| v.as_str())
    .map(|s| s.to_string());
```

`drive.rs:598-601` (new workspace_path extraction):
```rust
let workspace_path = entry
    .get("workspace_path")
    .and_then(|v| v.as_str());
```

Same shape, same idiom. The workspace_path extraction omits the trailing `.map(|s| s.to_string())` because it's used directly as `Option<&str>` in the spawn call (no `String` ownership needed), which is correct.

---

## Minor Observations (non-blocking, non-actionable)

- The `workspace_path_set_and_exists_canonicalizes` test introduces a `let _ = canonical_str;` at the end (line 2295 area). Slightly awkward — the variable was computed for documentation but never asserted. Could be removed; could also be left as documentation. Not worth a revision.
- The error message in `drive.rs:608` is very good (display_id + path + suggestion). Consider also surfacing the failure mode in the runner's own `canonicalize().with_context(...)?` message (line 315) — currently it says "workspace_path canonicalize failed: '<path>'" which doesn't differentiate "missing" from "permission denied" from "broken symlink." Not a Phase 1 concern; the underlying IO error is wrapped as the source via `with_context`.
- The execution log notes "Pre-task warning baseline: 3x `unused import: crate::db` in drive.rs tests (pre-existing). No new warnings added." — confirmed, no new warnings.

---

## Recommendation

**REVISE.** The single blocker is the test flake (AC1.7 + AC1.8). Fix is mechanical and orchestrator-inline-able in <30 lines using the `Command::new(shim_path).env("PATH", ...)` pattern already established at `src/runner/claude_code.rs:471-486`.

Everything else — the load-bearing pieces — is clean.

---

## Cycle 2

**Reviewer:** code-reviewer agent
**Date:** 2026-05-02
**Gate:** REVISE (cycle-1 minor → cycle-2 still flaky; root cause partially addressed)

### What the orchestrator changed (cycle 2)

Per the brief, exactly:
- Added `static PATH_MUTEX: std::sync::Mutex<()>` at the top of the `tests` mod (line 449).
- Wrapped the `unsafe { std::env::set_var("PATH", ...) }` blocks in **all four** PATH-mutating tests (`session_id_is_valid_uuid_v4_propagated_to_output`, `json_schema_arg_is_passed_inline`, `workspace_path_canonicalised_when_some`, `workspace_path_falls_back_to_inherited_when_none`) with `let _guard = PATH_MUTEX.lock()...` acquired before `set_var(new)` and dropped after `set_var(original)`.
- Added `crate::paths::test_cwd_lock()` acquisition in `workspace_path_falls_back_to_inherited_when_none` (line 913) — held BEFORE `resolve_cwd()` is called and through the spawn — to serialize against `paths.rs` and `cli/setup.rs` tests that mutate process-global cwd via `set_current_dir`.
- Lock acquisition order is consistent (PATH first, then cwd) — no deadlock risk introduced.
- Updated one stale comment ("test is single-threaded by cargo test's default behaviour" → "PATH mutation is process-global; PATH_MUTEX serializes...") on `session_id_is_valid_uuid_v4_propagated_to_output`. The pre-cycle-2 comment was factually wrong (cargo test defaults to multi-threaded); the new comment is accurate.

Total cycle-2 diff: ~21 LOC in `src/runner/claude_code.rs` only. Verified scope:
- File mtimes: `claude_code.rs` last modified 17:10:49; all other staged files last modified 17:04:53 → cycle 2 touched only `claude_code.rs`.
- The cycle-1 canonicalize-once block at `claude_code.rs:308-317` (the `let cwd = match workspace_path { Some(p) => ... canonicalize() .., None => resolve_cwd()? }` expression) is byte-identical to cycle 1.
- `runner.spawn(...)` is still called by both new tests — end-to-end coverage preserved.
- No assertion weakened or removed; no test deleted; no source semantics changed; pre-existing test logic unchanged (only lock guards added).

The cycle-2 change is well-scoped, well-commented, defensively consistent, and structurally correct.

### Determinism — the blocker

`cargo test --all-features` was run **75 times** back-to-back (10 + 15 + 50 in three batches) under default parallel harness:

- **Batch 1 (10 runs):** 9 pass / 1 fail. The fail was `json_schema_arg_is_passed_inline` (one of the pre-existing tests the orchestrator added the mutex to).
- **Batch 2 (15 runs):** 13 pass / 2 fail. Failures: `runner_uses_path_shim_not_real_claude` and `json_schema_arg_is_passed_inline`.
- **Batch 3 (50 runs):** 47 pass / 3 fail. Failures: `runner_uses_path_shim_not_real_claude`, `json_schema_arg_is_passed_inline`, `session_id_is_valid_uuid_v4_propagated_to_output`.

**Aggregate: 69 pass / 6 fail across 75 runs (~8% flake rate).**

`cargo test --all-features -- --test-threads=1` (serial): 487/487 deterministic pass. (Confirms the failures are races, not logic bugs.)

The cycle-2 fix **reduced** the flake rate (cycle 1 was ~50%) but did **not** eliminate it. AC1.8 still fails.

### Root cause — two distinct races, only one addressed

The orchestrator's diagnosis identified ONE race (parallel PATH mutation) and added one synchronization primitive. There are at least TWO independent races in this test pattern:

**Race A — process-global env mutation (PARTIALLY addressed by PATH_MUTEX).**
Two failure modes observed: `failed to launch claude ... No such file or directory (os error 2)` from `runner.spawn(...)` at lines 691 and 746. The kernel's PATH lookup at exec time finds neither the shim nor the real `claude`. PATH_MUTEX serializes the four participating tests' writes against EACH OTHER, but `unsafe std::env::set_var` is unsound in Rust 2024 specifically because **glibc's `setenv`/`getenv` are not thread-safe at the libc level** — any other thread in the process that reads PATH (libstd's `Command::output` itself does an exec-prep environment snapshot, and arbitrary parallel tests may indirectly call into libc functions that touch the env) can observe a torn or stale PATH value. The mutex doesn't fix this because the race is between the protected writer and unprotected libc readers. This is documented in [Rust RFC 3458](https://github.com/rust-lang/rust/issues/124866) and is exactly why the function became `unsafe`. Lower-but-nonzero flake rate is the expected outcome of a mutex-based fix.

**Race B — `ETXTBSY` / "Text file busy" on shim exec (NOT addressed at all).**
Observed in `runner_uses_path_shim_not_real_claude` failure: `Os { code: 26, kind: ExecutableFileBusy, message: "Text file busy" }`. This test does **not** mutate PATH and does **not** acquire PATH_MUTEX — so the cycle-2 fix is irrelevant to it. The race is at the kernel/inode level: `fs::write(&shim_path, script)` returns, but the file's write-side fd or the inotify/write-cache state is not yet flushed when `Command::new(shim_path).output()` runs. Linux refuses to exec a file that is open for writing (ETXTBSY). This race exists for ALL three shim-writing tests but only fires for the one without PATH mutation, because the others fail Race A first (they're more likely to take the PATH-error path before reaching exec). With PATH_MUTEX serializing the PATH-mutating tests, the ETXTBSY surface area on `runner_uses_path_shim_not_real_claude` is *unchanged* from cycle 1 but is now visible because the louder Race A noise is reduced.

### AC re-check

| AC | Cycle-1 | Cycle-2 | Notes |
|----|---------|---------|-------|
| AC1.1–AC1.6, AC1.9 | PASS | PASS (unchanged) | Load-bearing pieces verified byte-identical to cycle 1; `tests/tasks_e2e.sh` and `tests/drive_e2e.sh` re-run, both pass. |
| AC1.7 | FAIL | **STILL FAIL** | `workspace_path_canonicalised_when_some` and `workspace_path_falls_back_to_inherited_when_none` did NOT appear in any of the 6 failures across 75 runs — the cycle-2 fix DID make them deterministic in the observed window. **However**, the cycle-2 fix introduced a regression-by-side-effect: the same mutex was applied to two pre-existing tests that previously did NOT flake, and now they do. Net flake set has shifted (from "the two new tests" to "session_id, json_schema, runner_uses_path_shim"), but flake rate is still nonzero. |
| AC1.8 | FAIL | **STILL FAIL** | 6/75 (~8%) parallel runs failed. AC1.8 requires "passes with no skips and no new warnings" — flake is observable in <10 runs of `cargo test`, well within what a developer or CI would notice. |

### Scope discipline (cycle 2)

PASS. The orchestrator did not touch:
- `src/handlers/drive.rs` — confirmed by mtime + `git diff` shows only the cycle-1 changes.
- `src/handlers/guide.rs` — same.
- `src/runner/mod.rs` — same.
- `src/runner/mock.rs` — same.
- `stores/tasks/schema.yaml` — same.
- `ClaudeCodeRunner::spawn` body (`claude_code.rs:295-435`) — the canonicalize-once block at lines 308-317 is byte-identical to cycle 1; only the `tests` mod (lines 437-end) was modified.

No scope creep. No load-bearing regression.

### Lock discipline (cycle 2)

PASS. Verified per-test:
- `_path_guard` acquired before `unsafe set_var(PATH, new)` and dropped after `unsafe set_var(PATH, original)` in all four sites (lines 681-689, 735-743, 874-878, 912-921).
- `_cwd_guard` (only in `workspace_path_falls_back_to_inherited_when_none`) acquired before `resolve_cwd()` and held through spawn — correct.
- Acquisition order: PATH first, then cwd. Consistent across the only test that takes both.
- No other test acquires both → no deadlock cycle.
- Mutex doc comment at lines 443-448 is clear and explains the purpose. Future readers will understand WHY the lock exists. (Note: the comment is technically *optimistic* about the fix's effectiveness; see Race A above.)

The discipline is correct. The mechanism is just insufficient for the underlying race.

### Pre-existing tests (cycle-2 inclusion check)

In cycle 1 I did NOT flag `session_id_is_valid_uuid_v4_propagated_to_output` or `json_schema_arg_is_passed_inline` as flaky. Adding the mutex to them was a defensive consistency move (apply the same lock to all PATH-mutating tests in the file). That instinct was sound — they have the same theoretical race; cycle 1 just didn't observe it. Lock guards added; assertions and shim logic byte-identical. **No scope creep.**

However, the cycle-2 outcome makes one of those tests (`json_schema_arg_is_passed_inline`) appear in the observed failure set. This is not because the cycle-2 fix introduced a regression — the race was always there — but it does mean the orchestrator's defensive consistency exposed it. The honest framing: cycle 2 redistributed observable flake from "two new tests" to "three different tests", at a lower aggregate rate.

### Verdict

**REVISE.** The cycle-2 mutex is a **partial** fix: it reduced flake from ~50% to ~8% but did not eliminate it. The core unsoundness — `unsafe std::env::set_var("PATH", ...)` from a parallel test thread — is unfixable by adding a mutex around the WRITER side, because the readers (libc env machinery, libstd `Command` exec-prep) do not participate in any user-space mutex. Additionally, the ETXTBSY race in `runner_uses_path_shim_not_real_claude` was never addressed and remains.

The fix the cycle-1 review recommended is still the correct one: **stop mutating process-global PATH from the test, and use `Command::new(shim_path).env("PATH", new_path)` directly** in the four PATH-using tests. That pattern eliminates Race A entirely (no global mutation = no race). For Race B (ETXTBSY), the canonical mitigation is to drop the file write in a separate scope, call `fs::File::sync_all()` after the write, OR retry the exec with backoff (3-5 attempts) — even a 100ms sleep between `fs::write` and `Command::output` is usually enough on Linux.

Concretely, the recommended cycle-3 change is to refactor the four tests to:

1. Drop `PATH_MUTEX` and the `unsafe set_var` blocks entirely.
2. For the two tests that exercise `ClaudeCodeRunner::spawn` (which calls `Command::new("claude")` internally and thus needs PATH lookup), the cleanest refactor is to add a test-only constructor `ClaudeCodeRunner::with_binary_path(PathBuf)` that overrides the binary lookup. This is a small surface expansion (~5 lines of production code) but eliminates the entire class of PATH races. Alternative: build the `Command` manually in the test and replicate the small set of args the runner adds — keeps production code untouched but loses the end-to-end "what does spawn actually pass?" assertion.
3. For Race B, add an `fs::File::sync_all()` call after writing the shim, and consider opening it explicitly: `let f = fs::File::create(&shim)?; f.write_all(script)?; f.sync_all()?; drop(f);` — explicit drop of the write fd before exec attempts.

Estimated diff size: 20–40 LOC. Same surface area as cycle 2; different mechanism.

If the orchestrator wants the absolute minimum-discipline path forward and is willing to relax AC1.7 wording: mark the four flaky tests `#[ignore]` (or move them behind a `#[cfg(feature = "serial-tests")]` gate), document the rationale in `tests/CLAUDE.md` or similar, and rely on `cwd_canonicalised_before_spawn` (already passes deterministically) plus the AC1.6 drive-level tests for the workspace_path coverage. **Not recommended** — same reason as cycle 1: the runner-level canonicalization assertion is real value to keep, and ignoring tests masks rather than fixes the race.

This is the **2nd of 3 allowed revisions** (Revision Count: 2/3). One more cycle is available before BLOCKED.

---

## Cycle 3

**Reviewer:** code-reviewer agent
**Date:** 2026-05-02
**Gate:** PASS

### What changed in cycle 3 (commit `0687e9a`)

Single-file fix in `src/runner/claude_code.rs` (335 lines diff per `git show --stat`). Three race fixes layered together:

1. **Production:** new `bin: PathBuf` field on `ClaudeCodeRunner`, defaulted to `PathBuf::from("claude")`. `Command::new(&self.bin)` replaces the previous `Command::new("claude")`. Functionally identical for production callers (PATH lookup of bare name `"claude"` is what `Command::new` does for non-absolute paths).
2. **Test API:** `#[cfg(test)] pub(crate) fn with_bin(mut self, bin: PathBuf) -> Self` builder. Both attributes confirmed present at `claude_code.rs:94-98`.
3. **Test infrastructure:** `static SHIM_DIR: OnceLock<ShimDir>` written once on first access via `init_shims()`. Four named shim scripts (`silent`, `planner`, `executor`, `cwd_printer`) written into a `tempfile::Builder`-allocated subdir under `target/test-shims/`. The `TempDir` is held by the `OnceLock` for the test-binary lifetime — leaked at process exit (per the doc comment, intentional, to avoid drop-vs-running-shim races).
4. **PATH machinery removed:** `PATH_MUTEX` static is gone. All four previously PATH-mutating tests now invoke `ClaudeCodeRunner::new().with_bin(shims().XXX.clone())` and pass an explicit `workspace_path` where needed.
5. **cwd_lock for execution-relevant tests:** both `workspace_path_canonicalised_when_some` (`:890`) and `workspace_path_falls_back_to_inherited_when_none` (`:922`) acquire `crate::paths::test_cwd_lock()` to serialize against the `paths::tests` `set_current_dir` writers that produced ETXTBSY under high concurrency on Linux 6.17.
6. **Race-A bypass for `resolve_cwd()`-sensitive tests:** `session_id_is_valid_uuid_v4_propagated_to_output` (`:728`) and `json_schema_arg_is_passed_inline` (`:763`) pass `Some(env!("CARGO_MANIFEST_DIR"))` as `workspace_path`, bypassing `resolve_cwd()` entirely. This eliminates the cwd-dangling failure mode where a parallel `paths::tests` `set_current_dir(&tmp); drop(tmp);` left the process cwd pointing at a deleted directory.

### Determinism — INDEPENDENTLY VERIFIED

`cargo test --all-features` was run **12 times back-to-back** under default parallel harness:

```
Run 1-12: PASS (489 tests each, 0 failures, 0 skips)
Summary: 12 pass / 0 fail (of 12)
```

This is **stronger** than the orchestrator's reported 15/15 in scope but still well above the noise floor where a real flake would have shown up (cycle 2 was 6 fails / 75 runs ≈ 8% — at 12 trials a 5% flake has ~46% probability of being observed at least once; we observed zero). Combined with the orchestrator's 15/15, the empirical sample is now 27 consecutive clean parallel runs. The `runner_uses_path_shim_not_real_claude` test that was the cycle-2 ETXTBSY repeat-offender no longer mutates state at all (it just execs the stable `executor` shim directly).

`bash tests/tasks_e2e.sh`: PASS. `bash tests/drive_e2e.sh`: PASS.

### Production API surface — verified clean

- `bin: PathBuf` is private (no `pub`). External callers cannot reach it.
- `with_bin` is `#[cfg(test)] pub(crate)` — confirmed both attributes at line 94-95. Compiles out of release builds entirely; even within the crate, only test code can call it.
- `ClaudeCodeRunner::new()` and `with_model()` signatures unchanged — verified by inspection at lines 75-89.
- `Command::new(&self.bin)` with `bin = PathBuf::from("claude")` is functionally identical to `Command::new("claude")`: `Command::new` performs PATH resolution when the program path contains no separator, regardless of whether the argument is `&str` or `&PathBuf`. The OS lookup behaviour is determined by the path's components, not its Rust type.

### Canonicalize-once contract — preserved byte-identically

The `let cwd = match workspace_path { ... }` block at `claude_code.rs:329-334`:

```rust
let cwd = match workspace_path {
    Some(p) => std::path::PathBuf::from(p)
        .canonicalize()
        .with_context(|| format!("workspace_path canonicalize failed: '{p}'"))?,
    None => resolve_cwd()?,
};
```

This is byte-identical to cycle 1 / cycle 2 (verified via `git diff 9e90a82..0687e9a -- src/runner/claude_code.rs`). The cycle-3 diff in this region of the file is limited to (a) the `Command::new(&self.bin)` substitution at line 336 and (b) the file-top doc comment update lines 33-41. The single-canonicalize, `current_dir(&cwd)` once at line 337, `write_transcript(&cwd, ...)` once at line 393 invariants are all preserved. Inline comment at lines 327-328 still references the SDK guard (file-top lines 33-38 doc).

DONE_WHEN clause 2 ("preserving the SDK session-fresh-on-cwd-mismatch guard at `src/runner/claude_code.rs:305-306`") is satisfied — the guard is structural (single canonicalize at function entry) and remains in place.

### `OnceLock<ShimDir>` review — sound

The riskiest cycle-3 addition. Audit:

- **Where declared:** `src/runner/claude_code.rs:491` — `static SHIM_DIR: OnceLock<ShimDir> = OnceLock::new();` inside the `tests` mod, so it is `#[cfg(test)]`-gated implicitly.
- **`ShimDir` shape:** struct holding the live `tempfile::TempDir` (kept for inode-pinning) plus four named `PathBuf` fields (`silent`, `planner`, `executor`, `cwd_printer`) — one per shim script.
- **Initialised once:** `shims()` accessor (line 550) calls `SHIM_DIR.get_or_init(init_shims)`. Standard `OnceLock` semantics — exactly one initialisation, all subsequent calls return the same `&'static ShimDir`. Concurrent first-access calls are safe (`OnceLock` blocks contenders until init completes).
- **Different-script-per-test concern:** addressed by giving each test its OWN named shim. Tests use `&shims().planner`, `&shims().executor`, `&shims().silent`, or `&shims().cwd_printer` according to need. There is **no test mutation** of any shim script after `init_shims` returns — the `PathBuf`s are read-only handles. No test rewrites a shim. No test deletes a shim. So parallel tests using the same `&'static ShimDir` cannot corrupt each other's view.
- **Inode-recycling defence:** the doc comment at lines 462-476 explains why the directory lives under `target/` (stable mount, low /tmp churn) and why the `TempDir` is held forever (avoids the drop race against still-running shim execs). Reasoning is sound.
- **File-handle leak:** `init_shims` opens each shim file via `fs::File::create`, calls `sync_all()`, drops the handle, then `chmod`s. No write fd is held open across the exec — that was the ETXTBSY trigger in cycle 2's `runner_uses_path_shim_not_real_claude`. Cycle 3's design eliminates the surface area entirely.
- **No `tempfile` cleanup at process exit:** intentional, documented. The OS reclaims the inodes on process exit; the tradeoff (a few KB of leaked target dir on test crash) is well worth eliminating the drop race.

Verdict: the OnceLock pattern is correct and the lifetime discipline is defensible.

### cwd-lock discipline — appropriate

- `workspace_path_canonicalised_when_some` (`:884-910`) — holds `test_cwd_lock` though it does not itself call `set_current_dir`. The doc comment at 887-889 correctly explains: holding the lock serialises against `paths::tests` writers whose concurrent `set_current_dir` + git-process invocations were the empirical ETXTBSY trigger on the cwd_printer shim exec under 20+ parallel threads. Defensive lock that empirically eliminates the race — not strictly tied to the test's own cwd reads, but justified by the observed failure mode.
- `workspace_path_falls_back_to_inherited_when_none` (`:917-939`) — must hold the cwd_lock because it READS `resolve_cwd()` (which calls `current_dir().canonicalize()`) and then expects the same value to be observed by the spawned shim. Without the lock, a parallel `paths::tests` `set_current_dir(&tmp); drop(tmp)` between the test's `resolve_cwd()` and the shim's `pwd` would produce divergent values OR the dangling-cwd ENOENT. Lock is acquired BEFORE `resolve_cwd()` (line 922) — correct ordering.
- `session_id_is_valid_uuid_v4_propagated_to_output` (`:728`) and `json_schema_arg_is_passed_inline` (`:763`) — do NOT acquire `cwd_lock` and do NOT need to: they pass `workspace_path: Some(CARGO_MANIFEST_DIR)`, which routes through the `Some(p) => canonicalize(p)` branch and bypasses `resolve_cwd()` entirely. The dangling-cwd race cannot reach them. Their assertions still match cycle 1 (session_id parses as UUID v4; source-grep against `/tmp/stores-schema-` does not match and `--json-schema=` does match) — assertions byte-equivalent to cycle 1, just routed around the racing dependency.

### Scope discipline — clean

- `git show --stat 0687e9a`: cycle-3 commit touched `src/runner/claude_code.rs` (335 lines) and `tasks/active/T012-workspace-path-and-next-id/main.md` only. Plus the cumulative T012 diff (`git diff 5f42496..HEAD --stat`, where `5f42496` is the last pre-T012 commit) shows exactly the 7 in-scope source files plus task docs:
  - `src/handlers/drive.rs`
  - `src/handlers/guide.rs`
  - `src/runner/claude_code.rs`
  - `src/runner/mock.rs`
  - `src/runner/mod.rs`
  - `stores/tasks/schema.yaml`
  - `tasks/global-task-manager.md`
  - plus `tasks/active/T012-workspace-path-and-next-id/{main.md,plan-review.md}`.
- No edits to `src/cli/dynamic.rs`, `src/cli/dispatch.rs`, `src/handlers/next_id.rs`, `WORKFLOW_VERBS`, `tasks/CLAUDE.md`, or any unrelated handler. Phase 2 territory is untouched. Other handlers/agents/skills shown in the broader branch diff are T010/T011 (already on this branch pre-T012, not Phase 1's responsibility).

### Assertions preserved

- `runner_uses_path_shim_not_real_claude` (`:653`): still calls the executor shim, still asserts `final_message` contains `"executor"` and `"abc"`. Removed the comment that previously promised "actual ClaudeCodeRunner spawn call happens inside the runner integration test below" — true now: the spawn-based tests at `:728` and `:763` exercise that path via `with_bin`. No assertion weakened.
- `command_construction_and_final_message_parsing` (`:561`): still calls the planner shim, still asserts `"planner"` in `final_message`. Cycle-3 just removed the per-test shim write and the PATH-env wrapper; the JSON-shape assertion is identical.
- `session_id_is_valid_uuid_v4_propagated_to_output` (`:728`): still asserts UUID v4 parse and version, still goes through `runner.spawn(...)` end-to-end. Now passes `with_bin(shims().silent.clone())` and `Some(workspace)` instead of mutating PATH. Same shape, same assertion.
- `json_schema_arg_is_passed_inline` (`:763`): still source-greps for the negative needle (no `/tmp/stores-schema-` path) and the positive needle (`--json-schema=`). Same assertions, runs `runner.spawn(...)` to satisfy the smoke check.
- `workspace_path_canonicalised_when_some` (`:884`) and `workspace_path_falls_back_to_inherited_when_none` (`:917`): both still drive the spawned cwd_printer shim and assert byte-equal `printed_cwd` vs `expected_cwd`. End-to-end runner-level coverage preserved.
- `extract_*` unit tests, `extract_tools_*` tests, `cwd_canonicalised_before_spawn`, AC1.5/AC1.8/AC1.9 fixtures: unchanged.

### Process observation — non-blocking, surfaced for orchestrator

The cycle-3 commit `0687e9a` was authored by `Blake Sims <blake.sims27@gmail.com>` with `Co-Authored-By: Claude Sonnet 4.6` — i.e. the executor pair (Sonnet under the user's git identity), not the orchestrator. The task/CLAUDE.md workflow assigns commit responsibility to the orchestrator at the boundary between phases, not to the executor. Cycle-3 work was ready and the resulting commit is correct (clean message, accurate co-author trailer, well-scoped diff), but the workflow intent of "executor proposes, orchestrator commits" was bypassed.

This is informational for the orchestrator's process-improvement loop. It does not change the gate. The work itself is sound.

### Verdict

**PASS.** The flake is gone (12/12 independently, 27/27 with orchestrator's runs). The production API surface picked up only a private `PathBuf` field — no externally visible breaking change. The canonicalize-once contract block is byte-identical to cycle 1 and the SDK-guard inline comment is intact at the point of risk. The `OnceLock<ShimDir>` test pattern is sound: shims are immutable after init, named per script, lifetime-pinned via the held `TempDir`. The `cwd_lock` use is appropriate to each test's race profile. Scope is exactly the seven in-scope files. All four DONE_WHEN spawn-time cases (set+exists, set+missing, unset, set+canonicalize-stable) are exercised by the AC1.6 drive tests; the `Some`-branch and `None`-branch are additionally exercised end-to-end at runner level by AC1.7 tests with the cwd_printer shim.

Phase 1 is complete. Route to Phase 2 (next-id scan).

This is the **3rd of 3 allowed revisions** (Revision Count: 3/3). Gate fires PASS — Phase 2 begins.
