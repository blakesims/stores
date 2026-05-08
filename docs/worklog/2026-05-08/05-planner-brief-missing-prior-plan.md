# Planner Brief Missing Prior Plan

**Date:** 2026-05-08
**Type:** note

## Summary

When a task's planner cycle is re-spawned on a `submit-plan-review NEEDS_WORK` (planning ↔ plan_review iteration), the substrate's planner brief includes the prior reviewer's `Gate` + `Summary` + `Open Questions` for each round — but **NOT the actual plan that was reviewed**. The planner is asked to revise text it has never seen.

This is a sibling to I022 (executor brief missing `external_reviews.findings` text on REVISE-respawn). Same family of bug: substrate's review-relay-on-respawn carries the *commentary* but not the *artifact under review*. Each round the planner reconstructs what the broken plan must have looked like from negative feedback alone, then produces a new plan — which drifts in different ways every cycle.

Concrete forcing function: T106 (L485 broad gatekeeper-router-drain) and T108 (L499 narrowed Slice 1) **both hit substrate's plan-review cycle limit** (4 rounds × NEEDS_WORK → auto-blocked). Each round's plan_reviewer feedback was substantive and different; opus planner kept producing plans that drifted from the contract's literal invariants (e.g. "rows that error MUST stay in `status='draft'`"). Initial diagnosis (Pi msg_b171ada2) was *planner literal-invariant drift* (filed as I026), but on closer audit the more proximate cause is that **the planner has no visibility into its own prior plan**, so it cannot do a targeted revision — it has to imagine the prior plan from the reviewer's complaints.

## Details

### Reproducible diagnosis (confirm first-hand)

Run these `stores` CLI commands to verify the gap. T108 is currently blocked on main with 4 rounds of plan_review history, making it a clean reproduction case (substitute any other task that's iterated in plan_review for further confirmation).

**1. Confirm the planner brief contains reviewer feedback but not the plan content:**

```bash
stores tasks brief T108 --for planner --invoker ai_autonomous > /tmp/t108-brief.txt
wc -l /tmp/t108-brief.txt
grep -nE "^## |^### " /tmp/t108-brief.txt
```

Expected output (current state):

```
129 /tmp/t108-brief.txt
3:## Persona
6:## Workflow Context
14:## Task
22:## Tier Guidance
27:## Contract
66:## Prior Plan Reviews
67:### Review 1
76:### Review 2
84:### Review 3
92:### Review 4
100:## Critical Actions (Checklist)
107:## Output Format (plan JSON)
124:## Success Criteria
```

There is **no `## Prior Plan` section.** Each `### Review N` subsection contains only the reviewer's `Gate`, `Summary`, and `Open Questions`:

```bash
sed -n '66,100p' /tmp/t108-brief.txt
```

**2. Confirm the prior plan content actually exists in the substrate (so it COULD be relayed):**

```bash
sqlite3 .stores/db.sqlite "SELECT json_array_length(json_extract(plan,'\$.phases')) FROM tasks WHERE display_id='T108';"
sqlite3 .stores/db.sqlite "SELECT substr(plan, 1, 200) FROM tasks WHERE display_id='T108';"
sqlite3 .stores/db.sqlite "SELECT json_array_length(plan_review_log) FROM tasks WHERE display_id='T108';"
```

Expected: `phases` count ≥ 1 (the most recent rejected plan is on the row), `plan_review_log` length 4 (all four reviewer envelopes preserved). The data is there; the brief generator just doesn't read it into the brief.

**3. Confirm what the planner ACTUALLY received at run time (limit of post-hoc audit):**

```bash
sqlite3 .stores/db.sqlite "SELECT id, role, started_at, transcript_path FROM agent_runs WHERE display_id='T108' AND role='planner' ORDER BY id;"
```

For each transcript path, note that the `.jsonl` files **do NOT preserve the system/user input prompt** (claude-code transcript format only persists assistant turns + tool calls). Direct post-hoc audit of "what the planner saw" is not possible from the transcript alone — operators must trust that `stores tasks brief --for planner` (re-generated on demand) matches what was sent at spawn time. This is itself an audit gap and may warrant a separate substrate observation if not already filed.

**4. Confirm the planner DOES acknowledge the relay limitation in its first turn:**

```bash
P=$(sqlite3 .stores/db.sqlite "SELECT transcript_path FROM agent_runs WHERE display_id='T108' AND role='planner' ORDER BY id DESC LIMIT 1;")
python3 -c "
import json
with open('$P') as f:
    for line in f:
        line=line.strip()
        if not line: continue
        d=json.loads(line)
        if d.get('type')!='assistant': continue
        c=d.get('message',{}).get('content',[])
        if isinstance(c,list):
            for item in c:
                if isinstance(item,dict) and item.get('type')=='text':
                    print(item['text'][:400])
                    raise SystemExit
"
```

Expected: the planner's first assistant turn typically says something like *"I'll start by exploring the codebase to understand the existing Router abstraction, the prior plan structure, and the test/schema constraints raised in the three NEEDS_WORK reviews."* — note the language **"the prior plan structure"** as something it has to *reconstruct* by exploring the codebase, not something it received directly.

### Where the brief is generated

The planner brief is constructed by the substrate's brief-generation handler. Locate the source:

```bash
grep -rn "Prior Plan Reviews\|## Prior Plan\|## Contract" src/handlers/brief.rs src/handlers/ 2>/dev/null | head
```

The fix should add a `## Prior Plan` section (or equivalent) immediately above or below `## Prior Plan Reviews` that emits the most recent rejected plan from `tasks.plan` (JSON), with sensible truncation/formatting. Consider whether to emit only the most recent plan or a per-round history (per-round risks brief size blowup; most-recent is probably right since the reviewer's `Open Questions` reference the most recent plan structure).

### Suggested fix shape (small + mechanical)

- One file change in `src/handlers/brief.rs` (or wherever the planner brief is constructed).
- Read the current `tasks.plan` JSON; if non-empty, render a `## Prior Plan (rejected)` section with phase/task summaries (or pretty-printed JSON if that's simpler).
- Update the brief template's intro language so the planner is explicitly directed to revise *this* plan against *those* reviews, not reconstruct.
- Regression test: a unit test that calls the brief generator on a fixture task with a non-empty `plan` and a non-empty `plan_review_log`, asserts the prior plan is in the rendered brief.
- Reference T108 / I022 sibling status in the commit message; doc-only update is NOT sufficient (this is real handler code).

### Why ship on `main` directly (substrate-repair lane)

Per `.claude/skills/engine-controller/SKILL.md` § *Substrate repair lane* and `CLAUDE.md` § *Session doctrine — 2026-05-08*: the substrate is currently blocking forward motion on T108 (and previously T106) because of this exact relay gap. The fix is narrow, mechanical, testable, and restores convergence for any future planning loop that needs to iterate. Pre-blessing for substrate-repair-lane shipments of narrow review-feedback-relay fixes is in the SKILL.

The substrate-repair lane is appropriate even though the broken thing is *handler code* not lifecycle/schema, because:
1. The substrate workflow (planner ↔ plan_reviewer iteration) IS structurally non-convergent without this fix.
2. The fix is one file + one test.
3. It does NOT touch lifecycle, schema, authority, security, or task acceptance semantics.

### Sibling observations (do not duplicate)

- **I022** — REVISE-respawn drive cycle does not inject `external_reviews.findings` into executor brief. Same family of bug, different lane (external_review → executor instead of plan_review → planner). Should be co-fixed or at least cross-referenced.
- **I026** — planner literal-invariant drift. Filed by queue-curator after T106/T108 cycle-limit blocks. **The diagnosis in I026 is partially correct but secondary** — drift is what you get when the planner has to imagine the prior plan. Once this prior-plan-relay fix lands, re-evaluate whether I026 is still independently necessary or whether it folds into the same observation family. If you ship this fix and a re-attempted T108-shape task still drifts, I026 stands; if drift disappears, I026 should be marked superseded.
- **Audit gap** — claude-code transcript files don't persist input prompts. Mentioned in step 3 above but not filed yet; may want a fresh observation if not already covered.

## Fix shipped

Commit `8bf4d18` (`Fix revision agent briefs`) addressed the immediate prompt-relay gap without changing schema/persistence:

- `planner-brief.md.tpl` now renders a distinct `## Revision Context` when `plan_review_log` is non-empty, including the current rejected `tasks.plan` and all prior plan-review feedback. This fixes the T108-class failure where a respawned planner saw reviewer commentary but not the artifact being revised.
- `executor-brief.md.tpl` now treats code-review backpressure as revision context, showing prior executor submissions plus their review feedback for the current phase.
- `code-reviewer-brief.md.tpl` now calls out re-review cycles when `current_cycle > 1` so reviewers explicitly compare the latest executor submission against prior findings.
- `src/handlers/brief.rs` includes regression tests for planner rejected-plan relay and executor/code-reviewer backpressure prompt rendering.
- Follow-up amendment: planner briefs now also receive compact source-observation context for linked observations (`type`, `inputs`, `known_solution`, `touches`, `affects_capability`, and bounded harden decisions) via a render overlay used by both `tasks brief` and `tasks drive`. This restores planner visibility into observation intent fields that auto-promote does not copy into `tasks.contract`.

Validation performed:

```bash
cargo test handlers::brief::tests -- --nocapture
cargo run --quiet -- tasks brief T108 --for planner --invoker ai_autonomous > /tmp/t108-new-brief.txt
```

The regenerated T108 planner brief now contains `## Revision Context`, `### Rejected Plan To Revise`, the current rejected plan, and the prior `NEEDS_WORK` reviews.

Separate follow-up filed as **L502** for prompt observability: persist `prompt_i` / generated brief revisions per agent iteration in the database for analytics and audit. That is intentionally out of scope for this narrow template fix.

## Follow-ups

- Confirm the diagnosis with the four commands above before opening a PR / ratifying anything. ✅ Done before fix.
- Check whether the executor brief has the same shape gap (the executor is ALSO re-spawned on REVISE; does its brief carry the prior wrap_log / prior code_review feedback / prior implementation diff? If not, it's another sibling).
- After landing this fix, retry T108 by either: (a) abandoning T108 and re-ratifying L499 once the auto-promote subscriber gap (I025) is addressed, OR (b) shipping the related `tasks resume --reset-plan` repair primitive Pi blessed in msg_7cef2d5e Path 2.
- Cross-reference engine-controller SKILL: this finding strengthens the case that "different feedback each round" does NOT prove the relay is healthy — the relay can be healthy on the *commentary* side and broken on the *artifact* side. Update the convergence-stall recognition table to reflect this.
