# Topology and Watch shipped, with residual friction

**Date:** 2026-05-03
**Type:** note

## Summary

Continuation of `03-stores-watch-poc-and-topology-discussion.md`. From discussion → three tasks driven autonomously through the substrate (T005, T015, T018), three observations filed (L036, L040, L041), one bug fixed in-place (L036). The session validated the dogfood loop end-to-end (file → drive → review → accept → merge) and surfaced the substrate's first three "tests-skipped-as-passes" failure modes — a pattern worth naming.

What ships:
- `stores topology` — three-zone schematic (Z0 cross-store soft-FKs, Z1 per-store state machines, Z2 tasks workflow firing order). `--format dot|mermaid|auto`. Auto-mode renders in-terminal via `graph-easy --as=boxart` (libgraph-easy-perl).
- `stores watch` upgraded — phase-box / cycle-dot Design A glyphs (`▰▮◐▱` + `●·`) replace the text `execute P3/4 R1/3` form. Three independent visual channels (position / state / pressure).
- L036 fixed — the original `dot -Tutf8` (no such graphviz format) replaced with `graph-easy --as=boxart`; reason-aware fallback messages.

What still bites:
- L040 — test gate uses `graph-easy --version` which exits non-zero on success, so the width-checking integration test silently skips on hosts that have graph-easy installed. Same class of bug as the original L036 (a check that nominally exists but never fires).
- L041 — `Z1: tasks state machine` zone is still 136 cols (16 over the contracted 120). graph-easy's layout choices on the wider state names. Either tweak `--width` or relax the AC.

## Details

### What got done autonomously

| Task | What | Time | Phases | Cycles |
|---|---|---|---|---|
| T005 | `stores topology` static schematic command | ~24 min | 4 | 1 REVISE on P3 |
| T015 | watch dashboard phase boxes + cycle dots | ~10 min | 2 | 1 REVISE on P1 |
| T018 | topology renders zones separately (rethink after 876-col blow-up) | ~16 min | 3 | clean PASS each |

All three: substrate-driven (`stores tasks add --invoker ai_with_human` → `stores tasks drive --claude-code` → `stores tasks accept --invoker ai_with_human --approve-token`), merged via `git merge --no-ff`, worktrees torn down via `./dev done T### --force`, branches deleted via `git branch -d` (the `-d` form refuses unmerged — extra safety).

### Friction surfaced

- **L036** (resolved by me in `cdabd54`) — T005 contract said `dot -Tutf8` but graphviz has no `utf8` format. Implementation always fell back; misleading "not found on PATH" message even when dot was installed and merely failed for format reasons. Fixed by switching to `graph-easy --as=boxart` (Perl libgraph-easy-perl) and splitting `FALLBACK_NOTE_MISSING` from `FALLBACK_NOTE_FAILED`.
- **L040** (open) — T018's width + zone-header tests gate on `graph-easy --version` which exits 2 on the user's graph-easy v0.76 even though it prints version info correctly. So `Command::status().success()` returns false, both tests skip silently, "test result: ok" reported. Same anti-pattern as L036.
- **L041** (open) — T018 brought max width 876 → 136 cols. Better but still 16 over the contracted ≤120. The Z1 tasks zone has the longest state names + most edges, and graph-easy lays out wider node boxes than necessary. Probably one CLI flag away.
- **wrap-agent attribution drift** (filed earlier as L034) — wrap brief on T005 misattributed a commit on main as "rides on this branch" by reading diff stat without `git log <range>`. Caught at U3 review, no harm, but a confused brief raises the cost of every accept decision.

### Pattern: tests-skipped-as-passes

Three instances now: L036 (live-render gated on `dot -V`, then `dot -Tutf8` always failed but the gate never tripped), L040 (`graph-easy --version` exits non-zero so the width gate skips silently). The shape: a test exists, gates on a precondition, gate fails on the actual expected environment, test prints "skipping…" but reports "ok". The wrap brief reads "ok" and reports "all tests pass" — semantically meaningless. Worth a sweep across `tests/` for similar patterns; the gate functions deserve a uniform helper that asserts the precondition holds in CI and only skips with a `tester::skip!()` (or equivalent) that surfaces in test output as something other than "ok".

### Substrate behavior that worked well

- The U-moment discipline held cleanly. T015 and T018 were ratified with `--invoker ai_with_human` and accepted with `--approve-token` — no autonomous drift. The token mechanism made the U3 grounding mechanical (paste, fire, done) without me having to ask the user to type the verb.
- Drive cycles auto-resumed correctly after the server restart: `claimed_by` was cleared by the parent process exit, so re-running `stores tasks drive T###` would have picked up from the current substrate state. (T014 and T017 were left dangling at the user's request — not my call.)
- The watch dashboard upgrade made parallel drive observation legible: T014 (autonomous flow engine), T017 (schema migrations), T018 (topology zones) all visible at-a-glance with phase position + cycle pressure encoded in the glyph row.

### Substrate behavior that didn't

- T015 and T018 both produced wrap briefs that confidently described "ok" outcomes from skipped tests. The wrap can't tell the difference between a test that actually exercised the assertion and one that early-returned at a gate. This makes wrap briefs a less reliable signal than they look — readers should still spot-check.
- Snapshot tests (T005 and T018) needed a manual `UPDATE_TOPOLOGY_FIXTURES=1` re-run after T014's merge added `deploy_blocked` to the tasks lifecycle. Schema evolution + golden snapshots inherently couple. Worth a worklog note next time anyone changes lifecycle.transitions.
- The merge of T018 hit a real conflict because another agent was concurrently merging T014 in a different shell. The user paused us both manually. Worth thinking about whether the substrate should have a softer mutex on the working tree (or at least a "merge in progress" status hint).

## Follow-ups

- **L040** (high) — fix `graph_easy_on_path()` test gate to do a real spawn-and-pipe instead of `--version`. Sweep `tests/` for sibling patterns (tests that gate on `Command::status().success()` against tools known to be quirky).
- **L041** (normal) — try `graph-easy --width=120` or `rankdir=LR` for Z1 tasks; failing that, accept 140 as the realistic target.
- **L034** (normal, from earlier in session) — tighten wrap-agent prompt to read git log direction explicitly when describing diff-stat changes.
- T014 (autonomous flow engine, mid-phase 4) and T017 (schema migrations, mid-code-review) drives were killed by the server restart and never restarted. User's call.
- Consider promoting "tests-skipped-as-passes" pattern observation into a refs doc (`docs/test-hygiene.md`) and add a substrate-level lint that reports skip count alongside pass count when running `cargo test`.
- The dogfood loop is now battle-tested across three real tasks. Could write up the canonical `add → drive → review → accept → merge → fmt → teardown → reinstall → file-followups` recipe as a refs doc — it's not in CLAUDE.md and I had to muscle-memory it three times today.
