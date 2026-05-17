PASS

The revisions materially address the prior blockers:
- task-specific `MapCell` reuse is constrained to shared primitives only, with lane-specific cells/source enums required;
- observation checkpoint ambiguity is resolved by defining `signal/evidence │ contract │ arch? │ resolution`, plus explicit collapsed-row handling;
- intake priority is called out as a Phase 1 load/omit caveat, avoiding guessed `normal` priority;
- review lane scope is made an explicit mixed-lane decision with data-query widening/provenance required;
- engine source rules now distinguish live locks/liveness from historical agent-run aggregates;
- per-lane sorting/render integration is included in shared notes and lane acceptance criteria.

Ready for implementation guidance.
