# Code Review — Phase 9, Cycle 1

- **Reviewed:** 2026-04-27
- **Reviewer:** code-reviewer agent
- **Cycle:** 1 of max 3
- **Gate:** PASS
- **Status next:** EXECUTING_PHASE_10
- **Issues found this cycle:** 0 critical / 0 major / 2 minor (informational)

## Summary

Phase 9 is the marquee end-to-end proof. It ships `tests/tasks_e2e.sh` (16
steps, 21 assertions, runs in 0.81s on test hardware), the
`observations` lifecycle expansion (4 new states, 7 new transitions, original
`triaged → resolved` preserved for v0.1 e2e back-compat), the `gate.priority`
optional enum field, and the `run_submit_review`-on-blocked non-zero-exit
behaviour required by AC9.4. All six ACs are met and all 298 unit tests pass.
Both e2e scripts (`tests/e2e.sh` and `tests/tasks_e2e.sh`) exit 0 from a
clean environment. The DONE_WHEN bolded clause about the 4th-REVISE
schema-level guard with auto-BLOCKED is airtight: step 11 asserts non-zero
exit code, error containing "guard" + "current_cycle", row status reading
back as "blocked", and `blocked_reason` populated with guard + phase + cycle
context — verified by independent live re-run.

PASS recommended with two minor (informational) carry-forwards, neither
gating.

## AC verification

| AC | Result | Evidence |
|----|--------|----------|
| AC9.1 | PASS | `cargo test --quiet` → 298 passed; 0 failed; 0 ignored. |
| AC9.2 | PASS | `env -u CLAUDECODE bash tests/e2e.sh` exits 0 with all 13 steps green; original `triaged → resolved` direct-path transition preserved. |
| AC9.3 | PASS | `bash tests/tasks_e2e.sh` exits 0 in 0.81s (well under the 30s cap); 16 steps, 21 PASS markers. |
| AC9.4 | PASS | Step 11 asserts: (a) `$?` non-zero (got 1), (b) error contains "guard", (c) error contains "current_cycle", (d) row status == "blocked", (e) `blocked_reason` non-empty and contains "guard", "current_cycle", and "1" (phase context). All five assertions independently re-verified. |
| AC9.5 | PASS | Step 14 runs `stores tasks render T001` twice; sha1sums of `tasks/completed/T001-smoke-test/main.md` are byte-identical (the `diff` invocation returns empty). |
| AC9.6 | PASS | The script-internal allowlist grep matches no forbidden verbs; an independent grep run by this reviewer also returned empty. Verbs used: `add`, `next-action`, `brief`, `submit-plan`, `submit-plan-review`, `submit-execute`, `submit-review`, `resume`, `show`, `render` — all in DONE_WHEN's allowed set. |

## Independent live verification

```bash
cargo install --path . --quiet
cd $(mktemp -d)
time bash /home/blake/repos/experiments/stores/tests/tasks_e2e.sh
# → 0.81s elapsed; exit 0; 21 PASS markers; final state "complete|2"
env -u CLAUDECODE bash /home/blake/repos/experiments/stores/tests/e2e.sh
# → exit 0; all 13 demo steps green
cargo test --quiet
# → 298 passed; 0 failed
```

The 4th-REVISE marquee was probed end-to-end:
- Steps 8-10 produced `current_cycle=2,3,4` after each REVISE (post-increment
  guard `current_cycle <= 4` evaluates 2,3,4 — all true, all routed
  back to `executing`).
- Step 11 issued the 4th REVISE → `current_cycle=5` would-be → guard
  `5 <= 4` false → unguarded REVISE→blocked transition fired →
  `blocked_reason = "4th revise rejected by guard current_cycle <= 4 on
  phase 1 cycle 4: still broken after 3 revises"` → `compute_submit_review`
  returns `Ok(SubmitOutput{new_status: "blocked", blocked_reason: Some(...)})`
  → `run_submit_review` `bail!`s after the commit, producing CLI exit 1.

This is the exact shape promised by the DONE_WHEN bold clause.

## Structural review

### Deviation 1: `run_submit_review` returns `Err` on blocked routing

**Verdict: correct, well-bounded.**

`run_submit_review` (submit.rs:1018-1044) calls `compute_submit_review`,
prints the summary, then bails if `out.new_status == "blocked"`. The
compute path commits the tx FIRST (line 999), so `blocked_reason` is durable
in the DB before the bail fires — the row is genuinely blocked even though
the CLI exits non-zero. The compute layer's contract (`Ok(SubmitOutput)` for
both happy and blocked) is preserved, and the existing
`ac5_4_fourth_revise_blocked` marquee unit test (submit.rs:1549) continues
to call `compute_submit_review` directly and asserts on `Ok(...)` —
unaffected by the run-layer bail. The added
`blocked_reason: Option<String>` field on `SubmitOutput` is initialized to
`None` in the four other compute fns (submit-plan, submit-plan-review,
submit-execute, resume) and to `Some(reason)` only in submit-review's
blocked branch. Clean.

### Deviation 2: Step 16 uses `cargo test ac5_*` filters instead of a separate `tests/submit_atomicity.rs`

**Verdict: acceptable but with a minor robustness gap (m1 below).**

Plan 9.3 step 16 said "the bash e2e references the Rust suite by running
`cargo test --test submit_atomicity`". The integration test file does not
exist and was not authored — instead the AC5.11/13/14 tests were authored
inline in `src/handlers/submit.rs::tests`. The bash script runs them via
`cargo test ac5_11b`, `cargo test ac5_13`, and `cargo test ac5_14`, each
gated by `grep -q "test result: ok"`. The same coverage exists, and full
`cargo test` runs everything regardless. No behavioural difference at the
covered ACs. (See m1 for the filter narrowness concern.)

### `observations` lifecycle expansion

`stores/observations/schema.yaml` adds states
`investigating, confirmed, needs_info, in_progress` and 7 transitions
matching the plan task 9.1 list verbatim. The original
`triaged → resolved` transition is preserved (line 13) so the v0.1 e2e
script's path `OBS001 open → triaged → resolved` continues to work. The
v0.1 e2e was re-run in this review and exited 0 with all 13 steps green.

### `gate.priority` field

`stores/gate/schema.yaml` adds `priority: enum [high, normal, low]`,
required: false, no default. Optional addition; existing rows unaffected
(SQLite stores NULL for the new column). The `--priority` CLI flag is
auto-generated via clap from the schema field. Existing gate tests in
the v0.1 e2e all pass.

### README updates

The "How to test" section (README.md:168-180) lists `cargo test`,
`bash tests/e2e.sh`, and `bash tests/tasks_e2e.sh`. The "Workflow stores"
section (README.md:183-199) shows the tasks CLI surface and the
`tasks:start` skill. `stores skills list` was probed and confirms
`tasks:start` is registered. Both sections are accurate and minimal.

## Findings

### m1 (minor, robustness): step 16 cargo-test filter is too narrow and silently passes on typos

**Severity:** minor (information).
**Location:** `tests/tasks_e2e.sh:50-57`.

Issue 1 — `cargo test ac5_11b` matches only one test
(`ac5_11b_handler_path_validator_failure_rolls_back`) but NOT the original
`ac5_11_atomic_boundary_rollback_leaves_db_unchanged` (also under AC5.11).
Probe: `cargo test ac5_11b` reports "1 passed", which leaves the
non-`b` AC5.11 test out of step 16's coverage. The full suite still runs it
via the unfiltered `cargo test`, so AC9.1 isn't affected — but the smoke
test's intent of "AC5.11 covered" is partially missed.

Issue 2 — `cargo test <typo>` returns "test result: ok. 0 passed" and the
script's `grep -q "test result: ok"` would PASS on that. A future rename or
typo of any of the three filter strings would silently degrade coverage.

**Suggested fix (Phase 10 or later):** either (a) widen filter to `ac5_11`
which catches both ac5_11 and ac5_11b, or (b) add an extra
`grep -q "[1-9][0-9]* passed"` guard so 0-passed runs fail loudly, or (c)
materialize `tests/submit_atomicity.rs` per the original plan and include a
single named test that re-exports the assertions.

Not gating: the relevant tests exist and pass; the full
`cargo test` invocation in AC9.1 covers everything.

### m2 (minor, weak assertion): step 11's "phase context" check uses `'1' in br`

**Severity:** minor (information).
**Location:** `tests/tasks_e2e.sh:223`.

The Python check `assert '1' in br` would be satisfied by ANY '1'
substring in the `blocked_reason` text — including the `<=` operand "4",
"3 revises", an unrelated "1 critical", etc. The actual error format
emitted by `compute_submit_review` (submit.rs:955-957) is
`"4th revise rejected by guard current_cycle <= 4 on phase {N} cycle {M}: {summary}"`
where `{N}=1` and `{M}=4` — so "1" is genuinely in the phase position
today, but a renumber from "phase 1" → "p1" or similar would not break the
assertion despite being a regression. Recommend tightening to
`'phase 1' in br` or `'phase 1 cycle 4' in br` for a contract-faithful
check.

Not gating: today's behaviour is correct and the assertion passes for the
right reason in the field.

## Verdict

PASS — the marquee DONE_WHEN clause about "4th REVISE attempt on any phase is
rejected by schema-level guard with status auto-set to BLOCKED" is proven
end-to-end through the bash e2e: a real T001 row is created, planned,
plan-reviewed, executed, hits the cycle limit, gets BLOCKED, gets resumed,
completes, and the rendered main.md is byte-identical across two
invocations. No changes required for cycle 2.

Two minor findings (m1, m2) are informational carry-forwards for Phase 10
or later; neither blocks the gate. Advance to `EXECUTING_PHASE_10`.
