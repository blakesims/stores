---
description: Reference documentation — ADRs, PRDs, architecture notes, domain maps, runbooks.
---

## References

Curated durable docs. Updated manually — no script-managed naming. Promote here when a worklog insight becomes something future-you or a teammate will want to revisit.

## Important durable notes index

- `docs/philosophy.md` — first-principles substrate doctrine and what stays outside the substrate.
- `docs/architecture-coherence.md` — local correctness vs architectural coherence; includes the client-adapter boundary for cross-repo dogfood work.
- `docs/primitives.md` — typed primitives the substrate composes from; read before proposing schema-shape changes.
- `docs/engine-health.md` — current priority/state-of-engine snapshot; keep concise and priority-honest.
- `docs/agents-and-policies.md` — operational agent/subscriber policy surface.
- `docs/gatekeeper-design.md` — intake/gatekeeper design and front-of-engine routing direction.

## Agents daemon stale executable note

After `cargo install`, `stores agents run` detects when its launch-path executable identity no longer matches the in-memory daemon binary and self-reexecs into the launch-path binary, preserving the original daemon argv (including `--invoker`, `--detach`, `--log-file`, and poll flags). Operators and wrappers should not perform an additional manual restart unless the fallback fail-loud log appears: `daemon binary stale after cargo install; restart required`.
