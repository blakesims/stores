# Code Review — Phase 8, Cycle 1

- **Reviewed:** 2026-04-26
- **Reviewer:** code-reviewer agent
- **Cycle:** 1 of max 3
- **Gate:** PASS
- **Status next:** EXECUTING_PHASE_9
- **Issues found this cycle:** 0 critical / 0 major / 3 minor (informational only)

## Summary

Phase 8 ships `skills/tasks:start/SKILL.md` (347 lines) as a clean
orchestrator skill that drives the full workflow via the new `stores tasks`
CLI. All four ACs verified at the source/test/install layers. The skill is
correctly registered in `BUNDLED_SKILLS`, installs byte-identical, and the
sibling `task:next` skill was rewritten to point at the new tasks store
verbs. Tests: 298 pass (296 prior + 2 new). e2e: green.

PASS recommended with three minor (informational) carry-forwards. None block
the gate.

## AC verification

| AC | Result | Evidence |
|----|--------|----------|
| AC8.1 | PASS | `stores skills list` lists `tasks:start`; live install in fresh tmp dir produces a file `diff -q` byte-identical to bundled source. Rust unit `tasks_start_install_byte_identical` covers the same assertion. |
| AC8.2 | PASS | grep for the 10 allowed verbs returns 22 hits across SKILL.md; grep for forbidden `transition`, `update`, `init`, `install`, `resume`, `start` (as a `stores tasks <verb>`) returns 0. grep for forbidden references (CodeRabbit, merge-review*, phase-review*, pi-extension, MERGE_READY, MERGE_REVIEW) returns 0. |
| AC8.3 | PASS | `python3 -c "yaml.safe_load(...)"` on the frontmatter returns a dict with all 5 expected keys: `name='tasks:start'`, `description=<non-empty>`, `user_invocable=True`, `requires_stores=['tasks']`, `effort='medium'`. |
| AC8.4 | PASS | The literal heading `## DONE_WHEN propagation rule` appears at line 340; DONE_WHEN itself appears 10 times in the file (4 stage prompts × "Always include … DONE_WHEN", plus the propagation section, plus the routing/intent-contract sections). |

## Independent live verification

```bash
cargo build --release
TMP=$(mktemp -d) && cd $TMP && git init -q
$BIN init
$BIN skills list  # → "tasks:start" listed
$BIN skills install tasks:start
diff -q .claude/skills/tasks:start/SKILL.md \
        ~/repos/experiments/stores/skills/tasks:start/SKILL.md
# → no output (byte-identical)
```

```bash
cargo test --quiet  # → 298 passed; 0 failed
env -u CLAUDECODE bash tests/e2e.sh  # → all 13 steps PASS
```

## Structural review

The skill body matches the plan spec point-for-point:

- Stage 0: Context gate — present (line 67)
- Stage 1: Intent Contract + DONE_WHEN — present (line 80); explicit "Must be
  confirmed by the user" anchor and "Every downstream agent receives it
  verbatim"
- Stage 2: Planning (next-action assert + brief + planner subagent +
  submit-plan + render) — present (lines 128-159)
- Stage 3: Plan review — present (lines 161-192)
- Stage 4: Plan gate (NEEDS_WORK loop / READY proceed) — present (lines 194-204)
- Stage 5: Phase loop (5a execute, 5b code review, 5c gate table) — present
  (lines 206-300); table covers PASS-not-last / PASS-last /
  REVISE / 4th-REVISE-auto-BLOCKED
- Routing rule — present (lines 304-315)
- Blockers section (technical vs. business/scope) — present (lines 317-336)
- DONE_WHEN propagation rule — present (lines 340-347)
- Stage 6 / 7 explicitly absent ✓

## Minor (informational) carry-forwards — Phase 9 or later

### m1 — incomplete gate enumeration in skill prose

The actual `stores tasks submit-plan-review --gate` flag accepts
`READY | NEEDS_WORK | NOT_READY` (per `dynamic.rs:277` and the schema's
plan-review transitions). The skill at line 182 and 185 documents only
`READY | NEEDS_WORK`. Similarly `stores tasks submit-review --gate` accepts
`PASS | REVISE | FAIL` (per `dynamic.rs:345` and schema), but the skill at
lines 272 and 277 documents only `PASS | REVISE`. The 5c gate table
(lines 290-295) likewise omits FAIL.

**Why minor:** an orchestrator running `tasks:start` against a normally-
proceeding task will never need NOT_READY or FAIL. They are abandon-plan /
abandon-phase exits the framework supports for unrecoverable cases. AC8.2
doesn't require enumerating all legal gate values, only that no
out-of-allowlist verbs appear — which is satisfied. But the skill is the
single source of truth for an LLM orchestrator deciding what gate to submit,
and an orchestrator following it can't trigger NOT_READY/FAIL even when the
underlying agent recommends them. Suggest Phase 9 or 10 add one
clarifying line in each section: "Gate values are `READY | NEEDS_WORK | NOT_READY`
(NOT_READY for unrecoverable / abandon-plan)" and "`PASS | REVISE | FAIL`
(FAIL for unrecoverable / abandon-phase)".

### m2 — Plan task 8.4 literal verb deviation not recorded

Plan task 8.4 (line 610 of main.md) specified that `task:next` should use
"`stores tasks start <id>` to invoke `tasks:start`". The shipped
`skills/task:next/SKILL.md` instead uses
`Task(subagent_type="tasks:start", ...)` and `/tasks:start <id>`.

The deviation is **correct** — `stores tasks start` is not a real verb (the
schema only defines `start` as a `framework`-actor transition that the
engine fires internally; it is filtered out of `stores tasks --help` per
the Phase 7 cycle-2 m3 fix). And per AC8.2, `start` is not on the
allowlist. So the executor was right to ignore the literal plan wording.

**Why minor:** the Execution Log "Deviations / Notes" section says
"None. Plan spec was followed exactly." That's incorrect — a literal-verb
deviation occurred, the executor handled it correctly, but it should be
documented so future reviewers don't get confused. Suggest a one-line
amendment: "Task 8.4's literal `stores tasks start <id>` is replaced with
`Task(subagent_type='tasks:start', …)`/`/tasks:start <id>` because
`stores tasks start` is a framework-internal verb hidden from the public
CLI surface (per Phase 7 m3)."

### m3 — `task-workflow` plugin dependency implicit

The skill spawns four subagents under the `task-workflow:` namespace
(`task-workflow:planner`, `task-workflow:plan-reviewer`,
`task-workflow:executor`, `task-workflow:code-reviewer`). This implies the
user has the `task-workflow` Claude Code plugin installed. Nothing in the
skill frontmatter declares this. The bundled briefing templates avoid
hardcoding plugin names (they describe the agent role but never the
namespace), so this is the only place it surfaces.

**Why minor:** the v0.2 framework field `requires_stores` is
documentation-only ("future framework versions can verify" per task
description 8.2). A future `requires_plugins` field could be added in v0.3.
For v0.2, the dependency is real but documenting it is a polish item, not
a blocker. Suggest a single line under "Non-negotiable rules" point 1:
"Requires the `task-workflow` plugin to be installed (provides the four
subagent personas)."

## What's good

- The verb hygiene is exact: 10 allowed verbs, 0 forbidden verbs, 0 forbidden
  references — no over-shoot into `transition`/`update`/`init`/`install`
  even in passing prose.
- The DONE_WHEN propagation discipline is real — every Task spawn block
  literally has a "**Always include in the … prompt:** > **DONE_WHEN:**"
  callout (4 occurrences, lines 146-148, 179-181, 232-234, 269-271). An LLM
  orchestrator following the skill literally cannot forget to forward
  DONE_WHEN.
- "Do not edit main.md directly" is explicit in the Non-negotiable rules
  (rule 2c, lines 29-30) — `tasks tasks render <id>` is the single write
  path for main.md, matching the v0.2 contract.
- Render is called after **every** submit (4 explicit `stores tasks render`
  lines: 158, 191, 248, 285) plus the "After every submit, run render"
  reminder at line 297 — main.md never goes stale during the loop.
- The 5c gate table covers the 4th-REVISE → BLOCKED path with the right
  language ("Framework sets status `blocked` automatically — surface
  `blocked_reason` to user"), matching the schema-level guard the framework
  enforces.
- `task:next` was correctly de-stubbed, drops the v0.1 observations
  fallback, and uses only the four allowed `stores tasks` verbs (`list`,
  `show`). It avoids inventing the non-existent `start` verb and instead
  spawns `tasks:start` via the proper subagent or slash-command pattern.
- The new `tasks_start_install_byte_identical` test directly mirrors
  AC8.1's wording — not just "did the install run" but "is the bytes
  identical".
- The renamed `all_skills_bundled` test is honest about the count (5
  instead of 4) and asserts each name explicitly — a future addition that
  forgets to register a skill will fail this test loud.
- 4 commits, one task per commit (8.1 / 8.3 / 8.4 / log), no amends.

## Carry-forward to Phase 9 (binding)

None. The three m-items above are non-binding polish suggestions; Phase 9
proceeds. If Phase 9's smoke test surfaces an actual orchestrator gap from
the missing NOT_READY/FAIL gate enumeration, m1 graduates to a real fix.
