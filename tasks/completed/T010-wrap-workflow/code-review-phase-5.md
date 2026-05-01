# Code Review — Phase 5

**Reviewer:** code-reviewer (Opus 4.7 1M)
**Date:** 2026-05-01
**Cycle:** 1/3
**Verdict:** PASS
**Phase 5 commits reviewed:** `b0fcd7c` (impl), `e50951d` (docs)
**Branch:** `feat/T010-wrap-workflow`

---

## Build + test outcomes

- `cargo build --features runner-claude-code` — clean (warnings: 0 new).
- `cargo test --features runner-claude-code` — **472 unit + 2 integration = 474 passed**, 0 failed. Up from 470 at end of Phase 4 (+4, exactly as Phase 5 commit message claims).
- `bash tests/drive_e2e.sh` — exit 0; AC7.1 (happy 2-phase) and AC7.1b (revise-once) both pass.
- `bash tests/tasks_e2e.sh` — exit 0; all 16+ assertions pass.

## Out-of-scope check

`git show b0fcd7c --name-only` returns exactly:
- `agents/guide.md`
- `skills/task:wrap/SKILL.md`
- `src/handlers/guide.rs`

No drift into Phase 6 (`tests/drive_e2e.sh`), Phase 4 (`agents/wrap.md`, `wrap-brief.md.tpl`), Phase 3 (`compute_submit_wrap`), or `src/render/context.rs`. ✓

## AC verification

### AC5.1 — status-keyed dispatch ✓
`run_tasks_guide_with_runner` (guide.rs:298-358) reads `task_entry.status` once at the top, then branches at line 322:

```rust
let brief = if status == "in_review" {
    ...
    build_wrap_mode_brief(...)
} else {
    ...
    build_tasks_brief(...)
};
```

Single check at the entry point — no buried conditionals. Existing gate-mode dispatch (`run_gate_guide_with_runner`) is untouched.

### AC5.2 — wrap brief content ✓
`build_wrap_mode_brief` (guide.rs:477-562):
- Latest wrap_log entry via `extract_latest_wrap_log_entry` → `arr.last().cloned()` (correct LIFO).
- Falls back gracefully when `wrap_log` is absent / empty / non-array (returns `None`, renders "_No wrap_log entry found..._" placeholder via the `match wrap_log_latest` arm).
- Promise: dumps the contract sub-record as pretty JSON. The reviewer sees `executive_intent`, `done_when`, `scope_in`, `scope_out`, `assumptions` if present. Acceptable — AC5.2 says "contract block" without specifying field-by-field rendering.
- Reality: cycles[] table built by `extract_cycles_table` — same columns as the Phase 4 wrap-brief.md.tpl (Phase | Cycle | Executor Summary | Review Gate | Review Summary).
- Synthesis: renders `executive_summary` (string), `deviations[]`, `residual_risks[]`, `recommended_sanity_checks[]` via `format_string_list`.

### AC5.3 — authorized verbs in wrap-mode ✓
`WRAP_MODE_VERBS` const (guide.rs:54-61) contains exactly the 6 verbs:
- `stores tasks show`
- `stores tasks list`
- `stores tasks next-action`
- `stores tasks accept`
- `stores tasks reject`
- `stores gate add`

No more, no less. Brief includes a forbid clause ("All other CLI verbs are FORBIDDEN in wrap mode.") at line 560.

### AC5.4 — three-mode prompt structure ✓
`agents/guide.md`:
- Workflow Position section (lines 38-71) shows three modes with ASCII branches.
- Three numbered modes (lines 53-66) describe gate / task / wrap.
- Explicit framework-layer claim: "**The brief header tells you which mode you are in.** You do NOT inspect row state to determine your mode" (lines 69-71). Decision (f) compliance — agent does NOT derive mode from row state.
- Wrap Mode Protocol (lines 178-241) parallel to Gate Mode Protocol and Task Mode Protocol.
- Schema-enforced explanation: "This is a **schema-enforced** restriction — not a prompt-enforced one" (lines 211-214).
- Dual listing of `accept`/`reject`:
  - Wrap-mode "human-only" section (lines 344-351) — narrated for the human.
  - FORBIDDEN list (lines 363-364) — explicitly forbidden under AI context.
  This dual structure is correct per the review brief: AI can never write these regardless of mode; the wrap brief just has the guide narrate them for the human.

### AC5.5 — unit tests ✓
Four new tests in `guide.rs::tests`:
1. `ac5_5_in_review_status_triggers_wrap_mode_brief` — full handler path with `in_review` row + mock runner; runner exits 0 → handler Ok.
2. `ac5_5_wrap_mode_brief_contains_executive_summary` — pure brief-content test; asserts executive_summary token, contract done_when, cycles executor summary, all 6 verbs, schema-enforced note, FORBIDDEN clause.
3. `ac5_5_wrap_mode_brief_without_wrap_log` — defensive: `task_entry` lacks `wrap_log` field; brief still renders mode header, task ID, all 6 verbs, plus graceful "no wrap_log" placeholder.
4. `ac5_5_non_in_review_status_gets_tasks_brief` — negative case: `executing` status + `build_tasks_brief` produces "Task Mode" header and explicitly NOT "Wrap Mode".

### AC5.6 — slim skill ✓
`skills/task:wrap/SKILL.md`:
- Frontmatter: `name: task:wrap`, description present.
- Body: 2 prose paragraphs, ~3 lines total — fits the "≤10 lines" spec.
- Points at `stores tasks <id> guide --claude-code`.
- Path matches existing convention (`skills/task:next/SKILL.md`, `skills/gate:walk/SKILL.md`, etc., all use `<verb>:<noun>/SKILL.md`).
- No Q&A persistence, no rendering, no routing — pure entry point.

## Specific concerns from the review brief

1. **`compute_git_diff_summary` not called from guide.rs** — verified correct. The wrap-mode brief is for the human reading after the wrap agent has synthesised. The synthesis is already in `wrap_log[]`. The wrap agent saw the diff at synthesis time (Phase 4 overlay). The human-facing brief reflects the synthesis, not a fresh diff. No promotion of `compute_git_diff_summary` was required for Phase 5; if a future feature wants a fresh diff at human-review time, that's a forward concern, not a Phase 5 blocker.

2. **`EntryMap = BTreeMap<String, Value>`** — verified consistent. All test fixtures construct `BTreeMap<String, serde_json::Value>` directly (lines 1295, 1363, 1390 in `guide.rs::tests`). No `HashMap` introduced.

3. **Schema-enforced restriction story** — verified. Schema has `actor: human` on `accept` and `reject` (`stores/tasks/schema.yaml:118-119`). Validate layer enforces via `validate::actor::check_transition_actor` (`src/validate/actor.rs:50-65`). CLI verbs `accept`, `reject`, `amend` exist as auto-generated subcommands (verified via `stores tasks --help` against a fresh setup). Existing tests `transition_actor_ai_autonomous_rejected_for_ai_with_human` and `transition_actor_human_accepted_for_ai_with_human` (validate/mod.rs:468/482) cover the unit-level actor enforcement. CLI-level subprocess test is explicitly Phase 6's AC7.6 — not a Phase 5 blocker.

4. **Two separate briefs vs. template reuse (Phase 4 wrap-brief.md.tpl vs. Phase 5 build_wrap_mode_brief)** — read both side-by-side. Justified separation:
   - Phase 4 template's audience is the **wrap agent** producing a synthesis — has "Your Job" instructions and JSON envelope template.
   - Phase 5 brief's audience is the **guide agent** narrating a finished synthesis — has wrap_log entry rendering and authorized-verbs list.
   - Promise / Reality scaffold overlap is ~30 LOC. Sharing would require either parametrizing the handlebars template (`{{#if for_human}}…`) or adding a render dependency in `guide.rs` (purposely absent — the executor noted this). Acceptable duplication; future refactor could extract a `cycles_table_text` helper if it grows.

## Findings

### MINOR: wrap_log multi-field render not explicitly asserted
`build_wrap_mode_brief` renders four fields from the wrap_log entry: `executive_summary`, `deviations`, `residual_risks`, `recommended_sanity_checks`. The test `ac5_5_wrap_mode_brief_contains_executive_summary` asserts only the `UNIQUE_EXEC_SUMMARY_TOKEN`. The other three are exercised by the rendering code path but not directly asserted.

A tighter test would seed each list with a unique sentinel string (e.g. `"DEVIATION_TOKEN"`, `"RISK_TOKEN"`, `"CHECK_TOKEN"`) and assert each appears in the brief. Code is straightforward `format_string_list(entry.get("…"))` for each so the risk of regression is low, but explicit coverage for AC5.2's "deviations, residual_risks, recommended_sanity_checks" enumeration would close the gap.

Not a blocker.

### MINOR: tools frontmatter grants the verbs the prompt forbids (under AI context)
`agents/guide.md` lines 19-20 grant `Bash(stores tasks accept:*)` and `Bash(stores tasks reject:*)` to the Claude Code permission system. Lines 363-364 (FORBIDDEN list) tell the AI it MUST NOT call them. The schema rejection at write time is the load-bearing fence — that's the design intent and it works.

The contradiction is ergonomic, not safety-relevant. The agent prompt is self-consistent (it tells the AI to surface accept/reject for the human, not call them); the tool grant exists so that if a *human* is ever in a hybrid session that drives this same agent prompt, the verb is callable. The schema enforces the actor rule regardless of tool grant.

If we ever want to make the prompt-level intent match the tool-level intent, we'd remove the `Bash(stores tasks accept:*)` and `Bash(stores tasks reject:*)` grants from the frontmatter. The schema would still reject AI-invoker writes — but the AI would also lack tool permission to even try. Currently belt-and-suspenders is just suspenders (schema), not belt (tool grant). Acceptable per the "trust the schema" footer at line 376; flagged for future tightening.

Not a blocker.

### TRIVIAL: Cycles table format duplication
`extract_cycles_table` (guide.rs:577-595) renders cycles[] as a markdown table with columns identical to `wrap-brief.md.tpl:34-36`. Same logical content, different presentation engines (Rust format!() vs. handlebars). ~12 LOC of duplication.

Future-direction: could extract a `cycles_table_text` helper in a shared location. Phase 5 doesn't require it; the executor explicitly noted the decision not to share template logic between Phase 4 and Phase 5 briefs.

Not a blocker.

### TRIVIAL: Test-count claim drift from Phase 4 review's count
Phase 4 review noted "470 tests green" (b48e8d6 / fdf3509). Phase 5 commit message claims "All 472 unit tests pass" — meaning Phase 5 added 2 net tests by that count. Actually Phase 5 added 4 new tests (`ac5_5_*` × 4) — so the +2 implies 2 existing tests were renamed or absorbed. Spot-check: `cargo test` reports 472 unit and 2 integration = 474 total. Main.md execution log claims "472 unit + 2 integration = **474 total** (+4 new guide tests)" which matches exactly. Commit message slightly under-states (says "472 unit"); main.md is accurate. Counting nit only.

Not a blocker.

## Summary

Phase 5 cleanly implements status-keyed mode dispatch in `run_tasks_guide_with_runner`, adds the wrap-mode brief with the right content (contract / cycles / wrap_log entry / authorized verbs / schema-enforced note), updates `agents/guide.md` to describe three modes without putting row-state inspection in the agent prompt (Decision (f) compliance), and ships a slim `task:wrap` skill at the verified path.

All four ACs that called for code (AC5.1, AC5.2, AC5.3, AC5.5) are verified. AC5.4 (prompt structure) reads correctly. AC5.6 (skill file) matches convention.

Build clean, all 474 tests pass, both shell e2e scripts exit 0, no out-of-scope drift.

Findings are MINOR (test rigor improvement opportunity; tool-frontmatter ergonomics) and TRIVIAL (rendering duplication; counting). None block PASS.

**Verdict: PASS.** Advance status to `EXECUTING_PHASE_6`.
