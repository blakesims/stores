# Operator vs Engine Lanes

**Date:** 2026-05-09
**Type:** SOP draft
**Status:** active

## Context

Manual cleanup restored the queue to a trusted baseline, but exposed a second problem: not all work should pay full engine ceremony. T116 showed the engine can preserve correctness on a tiny fix, but with too much elapsed time. T123 showed lifecycle safety must be mechanical before autonomy.

The goal is to split work into lanes so Blake and the operator agent can move quickly on safe/control work while the engine handles durable, review-worthy implementation in the background.

## Lane A — Operator lane: do here

Use the main thread for work that is local, reversible, operator-facing, and easy to inspect.

Examples:

- queue/status inspection
- process checks and kills
- narrow read-only SQL
- observation/task disposition with Blake grounding
- worklogs, handovers, engine-health updates
- config toggles
- small UX/error-message improvements
- tiny obvious code fixes with focused tests
- emergency safety repairs when the engine itself is unsafe

Rule: if the desired diff/action can be explained in one sentence, inspected directly, and tested in ~5–10 minutes, prefer this lane.

## Lane B — Fast repair lane: do here, but test/commit

Use for small code fixes that are low blast-radius but still change behavior.

Requirements:

- explicit scope
- focused test or focused command validation
- explicit commit
- note if it bypassed engine and why

Avoid this lane for schema/lifecycle/authority/security unless it is emergency containment.

## Lane C — Engine lane: background single-row drive

Use the engine for work that benefits from planning, independent review, durable contract tracking, or background execution.

Examples:

- T2/T3 implementation
- schema/lifecycle/subscriber changes
- runner infrastructure
- integration lane work
- watch/front-door redesign
- telemetry system
- scheduler/file-overlap work

Rules:

- harden/ratify contract first
- drive one explicit task row, not broad daemon
- run as subprocess when possible so the operator thread stays free
- monitor logs/status from the operator thread
- stop on blocked/review/U-moment/weird transitions
- do not accept/reject without Blake grounding

## Lane D — Architect lane: clarify before implementation

Use when the question is about what the substrate should be, not how to patch it.

Examples:

- philosophy/primitives/Heart changes
- cross-project doctrine
- lifecycle semantics disagreements
- architectural gray areas
- contract rewrites like L538 replacing T123/L528

Output may be a revised observation/contract, then Lane C implements it.

## Decision test

Before starting work, ask:

1. Could a wrong change corrupt lifecycle, authority, deploy, main, or future automation? If yes, engine/architect.
2. Is the desired change tiny and obvious? If yes, operator/fast repair.
3. Does it need independent review to trust? If yes, engine or Codex review.
4. Is the main value immediate Blake clarity/control? If yes, operator lane.
5. Is the main value durable substrate capability? If yes, engine lane unless tiny.
6. Would running the engine block the main thread? If yes, subprocess or defer.

## Current application

- `T138` from `L538` is correctly Lane C: background single-row drive, planner/executor on Opus, operator supervises.
- T1 ceremony fast path is a high-priority future design item: preserve correctness without making tiny fixes pay full T2/T3 ceremony.
- Engine-health now names the upstream bottleneck as trust: safe lifecycle transitions, right-sized ceremony, legible control surfaces, and measured runner choices.
