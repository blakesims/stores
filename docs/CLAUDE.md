---
description: Reference documentation — ADRs, PRDs, architecture notes, domain maps, runbooks.
---

## References

Curated durable docs. Updated manually — no script-managed naming. Promote here when a worklog insight becomes something future-you or a teammate will want to revisit.

## Root docs rule

Keep root `docs/` sparse. A root-level durable doc must be one of:

1. **Constitutional doctrine** — changes what stores is or what primitives it composes from.
2. **Current architecture contract** — defines a live boundary, lifecycle, taxonomy, or operating policy.
3. **Current engine snapshot** — intentionally mutable state-of-engine summary.
4. **Index / navigation** — points to durable surfaces.

Everything else belongs in `docs/worklog/` for session records, `docs/archive/YYYY-MM/` for historical studies/handoffs, or `examples/` for examples. Archived docs are not updated except to add archive headers or cross-links.

## Important durable notes index

- `docs/philosophy.md` — first-principles substrate doctrine and what stays outside the substrate. Owner: human + Pi; substantive changes require human ratification.
- `docs/primitives.md` — typed primitives the substrate composes from. Owner: human + Pi; substantive primitive changes require human ratification.
- `docs/architecture-coherence.md` — local correctness vs architectural coherence; includes the client-adapter boundary for cross-repo dogfood work. Owner: Pi.
- `docs/risk-and-cluster-taxonomy.md` — current risk flags, cluster-key conventions, and tier/risk/policy matrix. Owner: Pi.
- `docs/gatekeeper-design.md` — intake/gatekeeper design and front-of-engine routing direction. Owner: Pi.
- `docs/engine-health.md` — current priority/state-of-engine snapshot. Owner: Pi + engine-controller; update only at significant inflection points.
- `docs/agents-and-policies.md` — operational agent/subscriber policy surface. Owner: engine-controller + Pi.

Update active docs only when a shipped change alters lifecycle/schema/subscriber semantics, priority doctrine changes, a repeated worklog insight graduates into durable doctrine, or live behavior proves a doc stale. Do not update active docs for session narration, one-off debugging notes, speculative plans, stale handovers, or client-specific studies unless they define a reusable substrate boundary.

## Archived / example material

- `docs/archive/2026-05/` — historical handoffs, 10.06 studies/POCs, and deferred constitutional-governance thesis material. Not current contract.
- `examples/agents-yaml-example.yaml` — historical/reference example, not executable config.

## Human intent inlet

Use `bin/stores-human` for Blake-first freeform intent capture while the substrate inlet remains agent-shaped:

```bash
bin/stores-human add        # opens $EDITOR
bin/stores-human add "freeform gripe/request"
bin/stores-human ls
bin/stores-human bump H001
bin/stores-human show H001
bin/stores-human done H001
```

It writes JSONL to `.stores/human-inbox.jsonl`, which is intentionally outside the schema-enforced SQLite stores and ignored by git. Tags/classification are deferred to substrate triage; the human-facing input stays freeform.

## Agents daemon stale executable note

After `cargo install`, `stores agents run` detects when its launch-path executable identity no longer matches the in-memory daemon binary and self-reexecs into the launch-path binary, preserving the original daemon argv (including `--invoker`, `--detach`, `--log-file`, and poll flags). Operators and wrappers should not perform an additional manual restart unless the fallback fail-loud log appears: `daemon binary stale after cargo install; restart required`.
