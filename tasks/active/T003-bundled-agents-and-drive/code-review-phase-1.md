# Code Review — Phase 1, Cycle 1

## Gate: PASS

## Counts: 0 critical / 0 major / 3 minor

---

## Findings

### Critical (0)

None.

### Major (0)

None.

### Minor (3)

#### [MINOR-1] Guide authorized read-only verb list adds `stores gate list` beyond Q4 lockdown
- **File:** `agents/guide.md:231`
- **Evidence:** The Read-only verbs section enumerates 5 verbs as locked in plan-notes Q4 (`stores gate show`, `stores gate answer`, `stores tasks show`, `stores tasks list`, `stores tasks next-action`) **plus a sixth — `stores gate list`**.
- **Plan ref:** Plan Notes Q4: "exactly five verbs listed in `agents/guide.md`". AC6.4 enumerates the same five verbs the parser-level test asserts must be present.
- **Impact:** Read-only blast radius; AC6.4 will still pass because all 5 mandated verbs are present and the forbid-everything-else clause is intact (`agents/guide.md:238-260`). The deviation is a small over-permission, not a contract break.
- **Suggestion:** Either (a) drop `stores gate list` from the read-only list to exactly match Q4, or (b) note in the guide prompt body that `gate list` was added as a read-only convenience and AC6.4's 5-verb assertion remains satisfied. Either is acceptable; I lean (a) for strict adherence.

#### [MINOR-2] Planner prompt has Stage 0 — not in the Stage 1-7 lockdown but reasonable
- **File:** `agents/planner.md:53-65`
- **Evidence:** Planner adds a "Stage 0: Context Gate" before Stage 1, total Stage 0-7 (8 stages). AC1.7c specified mirroring the Stage 1-7 structure.
- **Impact:** Stage 0 is an implementation safeguard ("emit BLOCKED if context is insufficient before producing speculative plan") — useful behavior. Stages 1-7 are all present and correctly ordered. The addition does not violate AC1.7c's "mirror without copying verbatim" — it expands.
- **Suggestion:** Informational — no change needed. If pedantic adherence to Stage 1-7 is desired, fold Stage 0's content into Stage 1 ("Intent Contract Verification" with a context-gate check at the top). I'd leave as-is.

#### [MINOR-3] Plan-reviewer prompt is the shortest at 415 lines (just above the 400-line floor)
- **File:** `agents/plan-reviewer.md` (whole file, 415 lines)
- **Evidence:** AC1.7d requires 400-1200 lines per prompt; plan-reviewer is 15 lines above the floor. Read-through confirms substantive content (review protocol with 7 steps, gate-decision guide, failure modes, re-review discipline, summary drafting, decision-matrix review heuristics) — not padded.
- **Impact:** Within bounds. Informational only.
- **Suggestion:** None. Note for future iterations if the agent needs to grow.

---

## Acceptance Criteria Verification Table

| AC | Status | Notes |
|----|--------|-------|
| AC1.1 | ✓ | `stores agents list` prints exactly 5 entries (planner, plan-reviewer, executor, code-reviewer, guide) with installed/uninstalled annotations. Verified live in /tmp/agents-test. |
| AC1.2 | ✓ | `stores agents install --all` writes 5 files; re-running is idempotent (silent on `--all`, `Already installed:` on single-name). Verified live. |
| AC1.3 | ✓ | `stores agents install <name>` writes a single file. `--global` codepath wires through `agents_dir(true)` to `~/.claude/agents/` (verified in code at `src/cli/agents.rs:57-66`; not exercised live to avoid touching `$HOME`). |
| AC1.4 | ✓ | `stores agents uninstall <name>` removes the file. Re-uninstalling a non-existent agent prints `Not installed: <name>` and exits 0 — non-fatal. Verified live. |
| AC1.5 | ✓ | Conflict detection — different content yields `Error: Agent exists with different content; remove or use --force` and exit 1. Verified live. Message format matches skills.rs verbatim except "Agent" vs "Skill". |
| AC1.6 | ✓ | All 5 prompts have `name` + `description` + `tools` frontmatter; **zero `effort` fields** anywhere (`grep -c effort agents/*.md` → 0/0/0/0/0). C2 from cycle-1 plan review satisfied. |
| AC1.7 | ✓ | Each prompt body specifies CLI-native protocol (read brief from stdin/argv → do work → submit via verb → emit JSON envelope on last line of stdout). Each "Output Protocol" section spells out the verb + JSON shape. |
| AC1.7a | ✓ | Each prompt names its exact verb: planner → `submit-plan`; plan-reviewer → `submit-plan-review`; executor → `submit-execute`; code-reviewer → `submit-review`; guide → `gate answer`. Each prompt also includes the matching JSON envelope shape. |
| AC1.7b | ✓ | Each prompt has a `## Failure Modes` section (verified `grep -c "^## Failure Modes" agents/*.md` → 1/1/1/1/1). Guide additionally enumerates authorized vs forbidden CLI verbs (`agents/guide.md:222-260`). |
| AC1.7c | ✓ | Planner prompt mirrors Stage 1-7 (objective verification → codebase analysis → phase design → decision matrix → open questions → plan notes → review handoff) with an added Stage 0 context gate. No verbatim copy from `task-workflow` plugin (no telltale plugin-specific markers found via grep). |
| AC1.7d | ✓ | Line counts: planner 502 / plan-reviewer 415 / executor 479 / code-reviewer 481 / guide 487. All within 400-1200 floor/ceiling. Read-through confirms substantive content; no padding. |
| AC1.8 | ✓ | `cargo build` succeeds (release + dev). `cargo test cli::agents` passes 6 tests (executor claimed "5 tests" in summary but the actual count is 6 — `fresh_install_writes_file`, `idempotent_reinstall_ok`, `conflict_different_content_errors`, `uninstall_removes_file`, `all_agents_bundled`, `flat_layout_not_nested`). The `all_agents_bundled` test asserts `BUNDLED_AGENTS.len() == 5`. Full suite: 304 tests pass. |
| AC1.9 | ✓ | `src/cli/agents.rs` is a near-mechanical clone of `src/cli/skills.rs`. Differences limited to: (a) `BUNDLED_AGENTS` registry contents, (b) target dir `agents/` vs `skills/`, (c) flat `<name>.md` vs nested `<name>/SKILL.md`, (d) doc-comment header on `agents.rs` notes the platform-driven asymmetry. The `flat_layout_not_nested` test was added (extra coverage beyond skills.rs's parallel `tasks_start_install_byte_identical`). Symbol diff between skills.rs and agents.rs (after substitution) is structurally identical. |

---

## Deep checks

### Git reality check
- `git diff --name-only HEAD~2 HEAD`: 16 files. Source code touches limited to the 4 claimed (`src/cli/agents.rs`, `src/cli/mod.rs`, `src/cli/dynamic.rs`, `src/main.rs`). `src/cli/dispatch.rs` is **not** modified (correct — Phase 1 doesn't dispatch new verbs).
- Commit `ae306cf`: 2783 insertions, 9 deletions. `agents/*.md` adds 2364 lines total; `src/cli/agents.rs` adds 338 lines.
- Follow-up commit `a471299` is exec-log only — no source code (correct).

### Test results (re-run)
- `cargo build`: clean (3 pre-existing unused-import warnings in handlers/{add,transition,update}.rs unrelated to Phase 1)
- `cargo build --release`: clean
- `cargo build --features runner-claude-code`: errors (feature not declared) — **expected**, feature lands in Phase 2
- `cargo test cli::agents`: 6 passed, 0 failed
- `cargo test`: 304 passed, 0 failed
- Live regression test: `stores skills list` still prints all 5 skills correctly — no regression from wire-up

### Live binary verification
- `stores agents list` (fresh dir): correct 5 entries, no annotations
- `stores agents install --all`: writes 5 flat `.claude/agents/<name>.md` files
- `stores agents list` (after install): 5 entries with `(installed)` annotation
- `stores agents install planner` (already installed): `Already installed: ...`
- `stores agents install <bogus>`: error 1, message names the bogus agent
- `stores agents uninstall planner`: removes file
- `stores agents uninstall planner` (twice): `Not installed: planner`, exit 0
- `stores agents uninstall bogus`: error 1, names the bogus agent

### Fixtures
- All 5 JSON files at `tests/fixtures/agent_outputs/<role>.json`
- Each parses cleanly as a JSON object
- Each has the correct `role` field matching the file basename
- Each contains all schema-required keys per AC1.7
- All are canonical examples, not stubs (planner has a full phase + decision matrix; code-reviewer has populated counts, summary, details)

---

## DONE_WHEN trace (Phase 1 contribution)

Phase 1 contributes the bundling story to DONE_WHEN: a fresh `stores` install with no `task-workflow` plugin can run `stores agents install --all` and have 5 prompt files written to `.claude/agents/`. The orchestrator (Phase 3+) will load them via `BUNDLED_AGENTS` (compile-time embedded). The agent prompts themselves specify the CLI-native protocol so the runner (Phase 2) can shell out and parse the role-keyed JSON envelope.

DONE_WHEN trace for Phase 1 is **intact**: install surface works, all 5 prompts are substantive, JSON envelope schemas are locked + match canonical fixtures, and the Phase 3 parser has a clear contract to assert against.

---

## Gate routing recommendation

**PASS** — all 13 acceptance criteria pass under inspection. Three minor findings documented above are non-blocking (read-only over-permission in guide, planner Stage 0 addition, plan-reviewer at the line-count floor). Phase advances; orchestrator should set Status → EXECUTING_PHASE_2.
