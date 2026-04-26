# Code Review — Phase 3, Cycle 1

- **Gate:** REVISE
- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer agent
- **Cycle:** 1 of max 3
- **Status next:** EXECUTING_PHASE_3

## Summary

Phase 3 lands a working `render_template` + `build_context` pair with 21 new tests, 228 total green, e2e all 13 steps pass. The four ACs that were directly-tested (3.1, 3.2, 3.3, 3.5) hold up under direct verification of fixture and helper code paths. The executor's call_inner / `ScopedJson<Derived(bool)>` design for `eq`/`gt`/`lt` is correct and well-justified — `if_eq_helper_false_branch` does verify the BLOCKED-conditional skip on non-BLOCKED status. `current_cycle_for_phase` derivation is sound across all four shapes tested (basic, empty, short alias, JSON-string). `set_strict_mode(false)` is set explicitly. handlebars 5 confirmed in Cargo.toml.

However, **one critical issue** breaks the universal contract that plan task 3.4 explicitly enumerates ("missing key returns empty string (NOT error — render must never crash on partial DB rows)"): `gt` and `lt` helpers return `RenderError` when a referenced key is missing or non-numeric. This was confirmed by direct probe of the binary. Given the contract is universal and Phase 6 (`render` for main.md) has rows in PLANNING state where `current_phase`, `current_cycle`, etc. may be NULL — and the plan explicitly notes "occasionally needed in render templates for 'latest cycle' arithmetic" — this WILL crash on partial rows in production unless every render template defensively guards every numeric comparison.

The `eq` subtlety the executor caught (returning `ScopedJson<Derived(bool)>` rather than a string) is exactly the same class of bug they then re-introduced in `gt`/`lt` via `ok_or_else(...)` returning a hard error instead of `Ok(ScopedJson::Derived(json!(false)))`. Fixing the `gt`/`lt` bug consistently brings them into line with the contract `eq` already honors.

## Files reviewed

- `/home/blake/repos/experiments/stores/src/render/engine.rs` (lines 1-278)
- `/home/blake/repos/experiments/stores/src/render/context.rs` (lines 1-293)
- `/home/blake/repos/experiments/stores/src/render/mod.rs` (lines 1-7)
- `/home/blake/repos/experiments/stores/Cargo.toml` (line 19 — `handlebars = "5"`)
- `/home/blake/repos/experiments/stores/tests/fixtures/workflow_minimal/templates/planner-brief.md.tpl` (lines 1-21)
- `/home/blake/repos/experiments/stores/tests/fixtures/workflow_minimal/schema.yaml` (verified `cycles` is a list_record)

## Verification of executor claims

- 4 commits present and on `master`: `074ffa8`, `fb9b1a3`, `854d028`, `9561fce`. **CONFIRMED.**
- `git diff --stat HEAD~4..HEAD`: 8 files / +752 / -1 — matches scope. **CONFIRMED.**
- `cargo test`: `228 passed; 0 failed`. **CONFIRMED.** 21 new tests under `render::*::tests::*`. Counted manually.
- `env -u CLAUDECODE bash tests/e2e.sh`: All 13 DONE_WHEN steps PASS. **CONFIRMED.**
- `eq` uses `call_inner` returning `ScopedJson<Derived(bool)>`: `engine.rs:30-34`. **CONFIRMED.**
- `eq`, `gt`, `lt`, `default` all registered: `engine.rs:133-136`. **CONFIRMED.**
- Strict mode disabled: `engine.rs:131` — `hbs.set_strict_mode(false)`. **CONFIRMED.**
- `handlebars = "5"`: Cargo.toml:19. **CONFIRMED** (major version 5).
- `if_eq_helper_false_branch` exercises the false branch with `status: "executing"` against the BLOCKED conditional: `engine.rs:209-214`. **CONFIRMED** — it asserts `out == "not blocked"`, proving the BLOCKED branch is correctly skipped.

## AC Verification (per plan)

| AC | Test | Status |
|----|------|--------|
| 3.1 | `static_template_roundtrips` (engine.rs:155-160) | PASS |
| 3.2 | `variable_substitution`, `missing_variable_renders_empty`, `null_variable_renders_empty` | PASS |
| 3.3 | `each_iterates_list` (engine.rs:191-196) | PASS |
| 3.4 | `if_eq_helper_true_branch`, `if_eq_helper_false_branch` | PASS |
| 3.5 | `context_top_level_keys_match_schema_plus_engine_key` + fixture byte-for-byte | PASS |

All five ACs as written pass. The critical issue below is on the **universal contract** in task 3.4 ("render must never crash on partial DB rows"), not on a specific AC bullet.

## Findings

### Critical

#### C1. `gt` and `lt` helpers return RenderError on missing or non-numeric keys, breaking the universal "never crash on partial DB rows" contract

**File:** `src/render/engine.rs:46-54` (GtHelper) and `src/render/engine.rs:67-75` (LtHelper).

The plan's task 3.4 states the contract explicitly:

> "missing key returns empty string (NOT error — matches Handlebars default; our render must never crash on partial DB rows)"

The executor's own engine.rs:127 docstring repeats it:

> "Missing keys render as empty string (Handlebars default, strict mode off). Returns `Err` only on template syntax errors, not on missing data."

But `GtHelper`/`LtHelper` violate this:

```rust
let a = h
    .param(0)
    .and_then(|p| p.value().as_f64())
    .ok_or_else(|| RenderError::from(RenderErrorReason::ParamNotFoundForIndex("gt", 0)))?;
```

When the param resolves to `Null` (missing key in non-strict mode) or to a non-numeric value, `.as_f64()` returns `None` and the helper returns `Err`. I verified this directly by injecting a probe test:

```rust
let tpl = "{{#if (gt missing_key 3)}}yes{{else}}no{{/if}}";
let result = render_template(tpl, &json!({}));
// result == Err("Helper/Decorator gt param at index 0 required but not found")
```

This is the same class of bug the executor correctly diagnosed and fixed in `EqHelper` (helper subexpression returning a string `"false"` is truthy under `{{#if}}`). The fix for `gt`/`lt` is symmetric: when a param is missing or non-numeric, return `Ok(ScopedJson::Derived(json!(false)))` rather than `Err`. That treats "missing/non-numeric" as "comparison is false" — the same lenient semantics `eq` uses, the same semantics the universal contract demands.

**Why it matters for Phase 6:** Phase 6's `render` verb runs against rows that may be in PLANNING state with `current_phase` and `current_cycle` NULL. Any `main.md.tpl` that does `{{#if (gt current_phase 1)}}…{{/if}}` will crash the render. The executor's commit message for `854d028` claims AC verification, but the byte-for-byte fixture template happens to use only `{{#if (eq …)}}`, not `{{#if (gt …)}}` — so this lurking bug isn't exercised by any current test.

**Required fix:** In both `GtHelper::call_inner` and `LtHelper::call_inner`, replace the `ok_or_else(...)` chain with: if either param is missing / not a number, return `Ok(ScopedJson::Derived(json!(false)))`. Add two test cases to `engine.rs#tests`:

```rust
#[test]
fn gt_helper_missing_key_returns_false() {
    let out = render_template(
        "{{#if (gt missing_key 3)}}yes{{else}}no{{/if}}",
        &json!({}),
    ).unwrap();
    assert_eq!(out, "no");
}

#[test]
fn lt_helper_missing_key_returns_false() {
    let out = render_template(
        "{{#if (lt missing_key 3)}}yes{{else}}no{{/if}}",
        &json!({}),
    ).unwrap();
    assert_eq!(out, "no");
}
```

### Minor

#### m1. Test coverage gap: missing-key behavior tested for variables and `eq` but NOT for `gt`/`lt`

Same code path family, same failure mode, but the executor wrote both `missing_variable_renders_empty` (engine.rs:172-178) and `null_variable_renders_empty` (engine.rs:181-187) to lock down the contract for plain-variable expansion, then registered two more helpers without parallel coverage. The C1 finding above is the direct consequence — without a missing-key test for `gt`/`lt`, the bug slipped past local CI.

After the C1 fix, the two probe tests above (or equivalents) should land permanently as part of the regression suite, named to make the contract obvious.

#### m2. `default` helper passes through `0`, `false`, and empty arrays as their JSON renderings instead of treating them as "empty"

**File:** `src/render/engine.rs:80-118`.

The doc comment at engine.rs:79 promises:

> "`{{default value "fallback"}}` — emits fallback when value is missing / null / empty."

I probed:

| Input | Output |
|-------|--------|
| missing | fallback (correct) |
| `null` | fallback (correct) |
| `""` | fallback (correct) |
| `[]` | `"[]"` (NOT fallback — debatable) |
| `{}` | passes through render |
| `0` | `"0"` (NOT fallback — debatable) |
| `false` | `"false"` (NOT fallback — debatable) |

Whether `0`/`false`/`[]` should fall back is a contract choice — none of these are explicitly nailed down by an AC, but the implementation is asymmetric: empty string YES, empty array NO. Since Phase 6 uses `default` for things like `{{default blocked_reason "—"}}` (text-shaped fields), the current behavior is probably fine in practice, but **either** the doc comment should be tightened (`"missing / null / empty STRING"`) **or** the helper should also treat empty arrays/objects/zero-length as empty. Cheapest fix: tighten the comment.

This is not a gate-blocker; flagging for clarity so Phase 6 templates know the contract.

#### m3. `Handlebars` instance + 4 helpers rebuilt on every `render_template` call

**File:** `src/render/engine.rs:128-142`.

`render_template` constructs a fresh `Handlebars` registry, registers four helpers, and renders. Phase 6 `render` may call this once per row; Phase 4 `brief` calls it once per invocation; orchestrator-driven loops will do many renders per minute. Helper registration on a Handlebars instance is not free (boxes four heap helpers each call). For Phase 3 unit-test correctness this is fine, but Phase 6 should consider one of:

- A `OnceLock<Handlebars<'static>>` constructed once and reused (helpers do not hold per-render state),
- A `RenderEngine` struct that wraps the Handlebars instance and exposes `render_template`,
- Or accept the cost as negligible if Phase 6 measurement says so.

Not a gate-blocker. Mark as a note for Phase 6 design.

## Things looked at and found OK

- `current_cycle_for_phase` derivation: latest cycle per phase via max scan; tested for basic / empty / short-alias / JSON-string-encoded shapes (context.rs:163-215). Algorithm is correct.
- `current_cycle_for_phase` map keys are stringified phase numbers (`ph.to_string()` at context.rs:88) — matches JSON spec (object keys must be strings) and the test at context.rs:175 (`ccfp["1"] == 2`). Plan's `{ 1: 2, ... }` shorthand was always JSON.
- `build_context`'s missing-field-as-null behavior is tested explicitly (context.rs:218-225) and integrates correctly with the `null_variable_renders_empty` engine test, end-to-end giving the universal contract.
- `EqHelper` truthiness via `ScopedJson<Derived(bool)>` is correct; `if_eq_helper_false_branch` verifies the BLOCKED-conditional false path.
- Strict mode is explicitly off (engine.rs:131).
- `handlebars = "5"`: confirmed in Cargo.toml; Cargo.lock shows `handlebars 5.x` resolved.
- The fixture template exercises text passthrough, variable substitution, `{{#each}}` over a list_record schema field, and `{{#if (eq …)}}` — all four substitution patterns called for in plan task 3.5.
- `description` field in `tests/fixtures/workflow_minimal/schema.yaml` is `text` (not the `record` shape used by the production tasks schema). For Phase 3 validation that's fine — substitution-pattern coverage is what matters.
- Helper-registration order: `eq`, `gt`, `lt`, `default` registered in that order (engine.rs:133-136). None shadow handlebars built-ins (handlebars-rust 5 does not register comparison helpers by default).
- All git claims verified: 4 commits on master, clean working tree apart from `tasks/active/T002-tasks-store-v02/plan-review.md` (untracked, unrelated to phase 3).

## Decision rationale

The **C1 bug is a true contract violation** with concrete blast radius (Phase 6 `render` against PLANNING-state rows), explicitly called out by plan task 3.4 and the engine.rs:127 docstring. It is small to fix (~10 LOC of code + 2 tests). Fixing it is faster than deferring it as a Phase 6 carry-forward, and deferring would leave the universal contract in a broken state at the boundary of Phase 4 (which depends on Phase 3 per the plan's `Dependencies:` line).

Gate: **REVISE**, cycle 1 of max 3. Status next: `EXECUTING_PHASE_3`.

## Required actions for cycle 2

1. **C1 fix:** In `src/render/engine.rs`, change `GtHelper::call_inner` and `LtHelper::call_inner` so missing or non-numeric params yield `Ok(ScopedJson::Derived(json!(false)))` instead of `Err`. Mirror the lenient semantics of `EqHelper`.
2. **m1 fix:** Add two regression tests in the same file: `gt_helper_missing_key_returns_false` and `lt_helper_missing_key_returns_false`. The names should make the contract obvious to anyone reading the test list.
3. **m2 (optional, recommended):** Tighten the `default` helper's doc comment to reflect actual behavior (e.g., `"missing / null / empty string"`), or extend the `is_empty()` arm to also treat empty arrays/objects/zero-length as fallback. Acceptable to defer with a comment in the source naming the choice.
4. **m3 (optional, defer to Phase 6):** Add a TODO comment in `engine.rs` near `render_template` noting the per-call rebuild cost; Phase 6 (or a later perf pass) decides whether to switch to a cached engine.

After fixes land, run `cargo test` (expect 230 pass: 228 baseline + 2 new) and `env -u CLAUDECODE bash tests/e2e.sh`. No re-execution of any other phase needed.
