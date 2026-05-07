# Heart Constitution Architect Thesis

**Date:** 2026-05-07
**Type:** note

## Summary

Raw capture of Blake's architectural thesis: stores is bootstrapping, by hand and agent convention, a constitutional/architectural governance layer that stores should eventually provide as a first-class primitive. Each system built through stores should have a **Heart**: the core architecture, constitution, principles, primitives, boundaries, and direction of that system. The Architect's responsibility is to keep the system aligned to the Heart. The Human's responsibility is to hold the Heart itself to human intent, desire, and direction.

## Details

Blake's thesis, captured close to raw:

> Each system that stores is building (including stores itself) should have a heart. The essence. The core architecture and constitution of the system itself. These are guiding principles, something like `docs/primitives.md` and `docs/philosophy.md` codified in some way. Then there is you — the architect — whose sole responsibility is to keep the system aligned to the constitution.
>
> Occasionally of course the system evolves. That is when the human should come in. The architect holds the system to the constitution, but the human holds the constitution to their intent and desires and direction. The architect therefore facilitates the evolution of the constitution by interfacing with the human.

Pi's immediate interpretation:

- The current system already behaves this way informally:
  - `docs/philosophy.md` / `docs/primitives.md` are the proto-constitution.
  - `docs/engine-health.md` is living strategic state.
  - Pi architect is the constitutional interpreter.
  - Blake is the sovereign source of intent.
  - substrate-agent is the executive/operations branch.
  - reviewer-runner/codex provides technical review evidence.
  - observations/tasks/intake are the change pipeline.
- The missing piece is that stores does not yet know this is the structure. The governance layer lives in docs, chat rulings, handoff notes, and agent discipline rather than typed substrate state.

Proposed conceptual model:

1. **Heart / Constitution**
   - The explicit constitutional bundle of a project/system.
   - Contains principles, primitives, non-negotiable boundaries, authority model, architectural commitments, strategic priorities, prohibited patterns, precedents, and open constitutional questions.

2. **Architect**
   - Not the smartest planner or a general executor.
   - A constitutional governor/interpreter.
   - Keeps proposed work coherent with the Heart.
   - Detects when local fixes violate global shape.
   - Decides whether a proposed change is ordinary execution or constitutional evolution.
   - Asks the human when the constitution itself must change.

3. **Human**
   - The source of intent and constitutional authority.
   - The only actor who can approve changes to the Heart's direction.
   - Does not need to adjudicate every local implementation question; does need to ratify changes in system values, boundaries, and core direction.

Important distinction:

- The architect can interpret the constitution autonomously:
  - e.g. "this codex finding touches dispatch lifecycle; use terminal-ok only, do not sweep failed locks."
- The architect cannot silently amend the constitution:
  - e.g. "stores should introduce a Heart primitive," "fast-track execution should exist," or "private install path replaces global binary doctrine."
- Human approval is required when the question becomes what the system should become, not merely how current doctrine applies.

Potential future stores primitives named in discussion:

- `heart` / constitution bundle.
- `constitutional_rulings` / `architecture_rulings` capturing architect decisions with question, decision, rationale, scope, citations, and whether the ruling interprets or amends the Heart.
- `amendments` capturing proposed constitutional changes, architect recommendation, human decision, effective date, and migration implications.

Potential implementation sequence sketched, not yet chosen:

1. Doctrine first: document Heart / Constitution / Architect / Human role model.
2. Ruling capture: lightweight architecture-rulings store before a full Heart store.
3. Task/risk integration: gatekeeper marks `requires_architect_review`; rulings attach to tasks/observations.
4. Heart bundle: promote docs/primitives/philosophy/engine-health into a typed/queryable bundle with docs as projections.
5. Amendment ceremony: human-approved constitutional amendments become first-class substrate events.

## Follow-ups

- Decide whether to create a doctrine task for the Heart / Architect / Human model.
- Decide whether first implementation should be a lightweight `architecture_rulings` store or a richer `heart` primitive.
- Keep this separate from the current operational binary-corruption recovery unless Blake explicitly prioritizes constitutional governance work next.
