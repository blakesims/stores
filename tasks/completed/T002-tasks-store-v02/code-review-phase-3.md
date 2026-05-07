# Code Review — Phase 3, Cycle 2

- **Gate:** PASS
- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer agent
- **Cycle:** 2 of max 3
- **Status next:** EXECUTING_PHASE_4

## Summary

Cycle 2 cleanly resolves the cycle-1 critical (C1) and adopts the recommended actions on m1, m2, and m3. `GtHelper::call_inner` and `LtHelper::call_inner` now use the symmetric `match (a, b)` pattern returning `Ok(ScopedJson::Derived(json!(false)))` on missing/non-numeric params — the universal "render must never crash on partial DB rows" contract is restored. Two new regression tests (`gt_helper_missing_key_returns_false`, `lt_helper_missing_key_returns_false`) lock the contract; each covers four sub-cases (missing first arg, missing second arg, non-numeric string, both missing) so the contract is pinned in four orthogonal places per helper. 230 tests pass (228 baseline + 2 new), 0 failed; e2e all 13 DONE_WHEN steps pass. The cycle-2 diff is tightly scoped: only `src/render/engine.rs` (98 ins / 20 del) and the cycle-2 log block in main.md (+9 / -4). No drift.

## Verification of executor's claimed fixes

| Claim | Verified by | Result |
|-------|-------------|--------|
| C1 `gt`/`lt` use `match (a, b)` returning `false` on missing/non-numeric | `src/render/engine.rs:46-54` (gt) and `:65-74` (lt) | CONFIRMED — both call sites use `let a = h.param(0).and_then(\|p\| p.value().as_f64());` then `match (a,b) { (Some,Some) => Ok(...gt/lt...), _ => Ok(false) }`. No `ok_or_else`, no `?`, no error path remains. |
| `RenderErrorReason` import removed | `src/render/engine.rs:10-13` | CONFIRMED — only `RenderError` and `ScopedJson` remain in the imports row from `handlebars::`. |
| 2 regression tests added with 4 sub-cases each | `src/render/engine.rs:285-355` | CONFIRMED — `gt_helper_missing_key_returns_false` and `lt_helper_missing_key_returns_false` each contain 4 distinct `assert_eq!` calls covering: missing first arg, missing second arg, non-numeric string value, both keys missing. Each test renders the `{{else}}` branch and expects `"no"`. |
| m2 doc comment tightened on `default` helper | `src/render/engine.rs:77-79` | CONFIRMED — comment now reads `"emits fallback when value is missing / null / empty string"` plus an explicit second line: `"Note: 0, false, and empty arrays/objects are NOT treated as empty and pass through as-is. Template authors that need those cases should guard with {{#if}} instead."` Contract is now explicit. |
| m3 TODO note added to `render_template` | `src/render/engine.rs:129-133` | CONFIRMED — `# Performance note (TODO Phase 6)` doc block above `pub fn render_template` describes the per-call rebuild cost and names two candidate fixes (`OnceLock<Handlebars<'static>>` or `RenderEngine` struct). |
| 230 tests pass | `cargo test` | CONFIRMED — `test result: ok. 230 passed; 0 failed; 0 ignored`. |
| e2e green | `env -u CLAUDECODE bash tests/e2e.sh` | CONFIRMED — `=== All 13 DONE_WHEN steps verified ===`, all 13 PASS. |
| 2 commits on master: `23a6442` (C1 fix) + `f4134e4` (main.md update) | `git log --oneline` | CONFIRMED — both present in expected order on `master`. |
| Tight scope (no collateral changes) | `git diff 9b2da0a..HEAD -- ':!tasks/' ':!src/render/engine.rs'` | CONFIRMED — empty diff. The only files changed are `src/render/engine.rs` and `tasks/active/T002-tasks-store-v02/main.md`. |

## Direct test of the C1 fix (re-probe of cycle-1 critical)

The cycle-1 probe was:

```rust
let result = render_template("{{#if (gt missing_key 3)}}yes{{else}}no{{/if}}", &json!({}));
// Cycle 1: result == Err("Helper/Decorator gt param at index 0 required but not found")
```

Cycle 2 lands the identical probe as `gt_helper_missing_key_returns_false` (engine.rs:289-294); the test passes with `out == "no"`. The else-branch renders, no error, contract honored. Same shape verified for `lt`.

## Files reviewed

- `/home/blake/repos/experiments/stores/src/render/engine.rs` (lines 1-356) — full read
- `/home/blake/repos/experiments/stores/tasks/active/T002-tasks-store-v02/main.md` (Phase 3 execution log section, cycle-2 revisions sub-block, lines 1137-1175)
- Diff `9b2da0a..HEAD` for `src/render/engine.rs` (118 lines: 98 ins / 20 del)
- Commits `23a6442` (C1 fix) and `f4134e4` (main.md update)

## Findings — cycle 2

**0 new issues.**

For a cycle-2 fix that is this surgical (one bug class, two helpers, four-case regression tests, ~80 LOC), zero new findings is plausible and honest. The fix is symmetric with the existing `EqHelper` lenient pattern; the import cleanup is the right hygiene; the regression tests are named to lock the contract; the doc tightening on `default` resolves the cycle-1 m2 ambiguity directly; the TODO on `render_template` is parked correctly for Phase 6 to act on. I looked specifically for:

- Unrelated changes piggybacking on the fix commit — none (diff scoped purely to `engine.rs` + `main.md`)
- Test-name drift (e.g., a test that doesn't actually exercise the else branch) — names match behavior; each test asserts `"no"` from a `{{else}}no{{/if}}` template
- Asymmetry between `gt` and `lt` fixes — none; the two helpers are now byte-for-byte parallel with only the operator differing
- Hidden error paths — none; `as_f64()` returns `None` on null/missing/non-numeric and falls into the `_` match arm with a clean `Ok(false)`
- Whether handlebars's `Helper::param(idx)` already collapses missing-key + null to the same `JsonValue::Null` in non-strict mode — yes (verified by reading handlebars-rust 5 docs and by the `null_variable_renders_empty` engine test which already passed in cycle 1)
- Whether the literal-numeric branch (e.g., `{{#if (gt 5 missing_key)}}`) goes through the same code path as the variable-binding branch — yes, `h.param(0).value()` returns the literal as `JsonValue::Number(5)` for which `as_f64()` returns `Some(5.0)`; the test on engine.rs:297-302 exercises exactly this combination

## Things looked at and found OK

- C1 fix is symmetric across `gt` and `lt`: both use the same `match (a, b)` pattern with the same error-free fallback. Nothing diverges between the two helpers beyond the operator.
- The `EqHelper` was unchanged (correct — it was never broken; cycle 1 confirmed it).
- The `helper_default` body was unchanged — only its docstring was tightened. No behavior risk.
- The TODO Phase 6 comment is doc-only (`/// # Performance note...`); does not change the `render_template` signature or behavior.
- `cargo test` runs ~30ms; no perf regression from the new tests.
- e2e runs unchanged from cycle 1; all 13 steps green.
- No new clippy warnings introduced (the 3 warnings from cycle 1 are pre-existing on the test binary, not on the engine.rs changes).
- Git log linear and clean: `23a6442` (fix) → `f4134e4` (log update); no force-push, no amends. Both commits attribute to Sonnet 4.6 with clear cycle-2 prefixes.

## Decision rationale

C1 is fully resolved — the contract violation cycle 1 caught is now closed at the implementation level, the test level (4 sub-case regression tests per helper), and the documentation level (the engine.rs:127 docstring promise is now backed by code). m1 is satisfied directly. m2 is satisfied with the cheaper-and-recommended option (doc tightening, not behavior change) — the contract is now explicit so Phase 6 template authors know what `default` will do for `0`/`false`/`[]`. m3 is satisfied as a parked TODO with named candidate fixes, exactly as cycle 1 requested for Phase 6 to act on (or not).

Phase 3 has now been verified to ship: a working `render_template`, four type-correct subexpression-safe helpers (`eq`/`gt`/`lt`/`default`), a `build_context` that mirrors schema fields plus the engine-only `current_cycle_for_phase` key, 23 new tests + 207 baseline = 230 green, e2e all green, and a fixture template exercising all four substitution patterns at byte-for-byte resolution.

Gate: **PASS**, cycle 2 of max 3. Status next: `EXECUTING_PHASE_4`.

## Carry-forward to Phase 6 (informational, not a gate condition)

- The TODO Phase 6 comment (engine.rs:129-133) names two implementation candidates — `OnceLock<Handlebars<'static>>` or a `RenderEngine` struct. Phase 6 should decide based on profiling, not assume.
- The `default` helper passes through `0`, `false`, and `[]` as-is. Phase 6 templates that need null-shape fallback for those types must use `{{#if}}` guards. This is now documented in the source.
- All three Phase-1/Phase-2 carry-forwards (P1-M1 expr unification, P1-M2 ListRecord walker, P2-M1 WorkflowResolved threading) remain open and must be enumerated in Phase 5's plan before Phase 5 execution begins; Phase 3 did not affect them.
