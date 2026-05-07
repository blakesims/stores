---
description: Reference documentation — ADRs, PRDs, architecture notes, domain maps, runbooks.
---

## References

Curated durable docs. Updated manually — no script-managed naming. Promote here when a worklog insight becomes something future-you or a teammate will want to revisit.

## Agents daemon stale executable note

After `cargo install`, `stores agents run` detects when its launch-path executable identity no longer matches the in-memory daemon binary and self-reexecs into the launch-path binary, preserving the original daemon argv (including `--invoker`, `--detach`, `--log-file`, and poll flags). Operators and wrappers should not perform an additional manual restart unless the fallback fail-loud log appears: `daemon binary stale after cargo install; restart required`.
