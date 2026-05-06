---
description: Reference documentation — ADRs, PRDs, architecture notes, domain maps, runbooks.
---

## References

Curated durable docs. Updated manually — no script-managed naming. Promote here when a worklog insight becomes something future-you or a teammate will want to revisit.

## Agents daemon stale executable note

After `cargo install`, `stores agents run` detects when its launch-path executable identity no longer matches the in-memory daemon binary and exits loudly with `daemon binary stale after cargo install; restart required`. First-ship fallback is fail-loud exit; self-reexec is not implemented, so a wrapper/operator should restart the daemon deterministically.
