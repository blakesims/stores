# Handover — watch cockpit landed, daemon truth, triage lane, and next picks

**Time:** 2026-05-10 11:33 +07

## What shipped / changed this session

- **T139 watch cockpit landed and installed.** The cockpit now makes `stores watch` much more legible: store-flow top strip, focused lanes, details, hidden historical exhaust by default. T139 reached `schema_migrated`; merge commit `ac88192`; release bumped to **stores 0.7.0** and installed to both `/home/blake/.cargo/bin/stores` and the private daemon path `/home/blake/.local/share/stores/bin/stores`.
- **Engine health was refreshed and compressed.** `docs/engine-health.md` now reflects the current picture, adds the ~10-rows-per-table rule, and compresses old shipped history into an archive-batch line instead of drowning current priorities.
- **T141 is in flight.** User chose `docs/ui-ux.md` Phase 3 next: rich store-specific drilldowns for the cockpit. T141 is active and currently executing.
- **L557 was triaged into T142.** The narrow immediate fix is: auto-drive redispatch must honor `agents.yaml` subscriber configuration; no implicit auto-drive when a project disables it. T142 is active and currently executing.
- **Task-B follow-up was captured as L558.** This preserves the separate daemon/binary-spawn hardening gap: preflight the child `stores` binary before recording `drive_pid`, so stale/broken child binaries fail loud instead of becoming silent zombies.

## Current live state

- `T141 watch cockpit rich store drilldowns`: `executing`, phase `1/4`, cycle `1`, active.
- `T142 auto-drive redispatch must honor agents.yaml`: `executing`, phase `1/1`, cycle `1`, active.
- `stores engine plan-start` currently has no blocked rows. Inactive historical accepted rows remain: `T002`, `T005`, `T015`, `T018`. Operator residue remains: `T081`, `T122`.
- A foreground/nohup daemon is running (`stores agents run --log-file logs/agents-daemon.log --poll-interval 2`, pid observed `417045`). It is functionally dispatching work, but `stores watch` reports `daemon DEAD` because the TUI only checks the detached pidfile `.stores/agents.pid`.

## Important boundary / work ethic

Use the substrate for real work that should exercise or repair the engine. But do **not** push every small main-lane cleanup through the full task ceremony.

Good candidates to push through the substrate:
- lifecycle / dispatch / subscriber behavior;
- watch/UI tasks with meaningful plan/review value;
- changes that should prove the dogfood path;
- user-ratified observations with durable contracts.

Good candidates for direct manual-main work with us:
- small obvious meta-substrate fixes needed to make the operator surface tell the truth;
- focused docs/worklog/engine-health updates;
- tiny diagnostics where the full ceremony would be more noise than signal.

Concrete example: **fixing watch's daemon liveness wording/detection is probably a small manual-main fix**. It should not wait behind a full T3. The bug is: foreground daemons can be live while watch says `daemon DEAD` because only the detached pidfile is probed. Fix options: detect foreground project daemon too, or say `detached pidfile missing` / `foreground daemon live` instead of absolute `DEAD`.

## UI/UX chain after T141

`docs/ui-ux.md` still gives the good ladder:

1. **T141 / Phase 3 — rich drilldowns** (in flight): store-specific focused table/detail renderers for intake, observations, tasks, external reviews, and engine health.
2. **Phase 2 — flow and pressure:** rolling rates from `transition_history`, queue ages, sparklines, review-loop badges, pressure coloring.
3. **Phase 4 — logs and liveness:** active agent transcript/log pointers, runner/model display, manual-engine vs daemon distinction, stale lock impact explanation.
4. **Phase 5 — guarded actions:** only after read-only trust is established.
5. **L529 generic flowtop architecture:** larger schema-aware flow graph / row-packet model. Do not rush this until cockpit surfaces prove what operators actually use.

## User scratch priorities consolidated

From `docs/human-dev/2026-05-10.md` and discussion:

- Simplify the overall flow/state/change scheme. The system may be too complex; consider whether `flow-diagrams.md` / lifecycle semantics can be compressed into a simpler unified model.
- Build a real triage workflow/agent inside the system: duplicate detection, `needs_info`, `architecture_review`, `confirmed`, `wont_fix`, route-to-task, and cluster handling. The plumbing exists (`intake_items`, `gatekeeper_router_drain`, validator, `architecture_reviews`), but the routing brain is not yet trustworthy enough.
- Right-size fast-track fixes / T1. Too much ceremony for tiny deterministic repairs slows learning and encourages bypassing the substrate.
- Rework external review timing. The authoritative ER probably belongs at integration time after refresh/rebase, immediately before merge, or needs a way to reason about what changed since the reviewed head.
- Agent feedback subscriptions: when an external/client agent files an obs, it should be able to subscribe/monitor it so `wont_fix`, duplicate, or routed status feeds back to the filer in real time.
- Improve `stores watch` daemon truth immediately: do not show `daemon DEAD` when a foreground daemon is live.

## L557 / T142 / L558 split

L557 contained enough detail to skip a full investigation cycle. It named concrete repros, code pointers, and two separate gaps.

We split it deliberately:

- **T142 (active now):** auto-drive redispatch must honor `agents.yaml`. This restores operator/project control. If a project disables `builtin:auto-drive`, engine-runner must not redispatch orphaned auto-drive work behind its back.
- **L558 (new follow-up):** spawn-side binary preflight. Before recording a `drive_pid`, preflight the child `stores` binary (`--version`, `tasks --help`, or equivalent) and classify failures loudly. This avoids stale/broken child binary failures degrading into silent zombies. Related to L543, but distinct from T142.

## Suggested first next manual fix

Fix watch daemon liveness truth on main:

- Current `src/tui/daemon.rs` only probes `.stores/agents.pid`.
- Foreground/nohup daemon does not write that pidfile.
- `stores watch` therefore reports `daemon DEAD` while the daemon is dispatching T141/T142.

A minimal fix could be:
- add fallback process detection scoped to this repo/cwd/DB path, or
- change the label to `detached daemon: DEAD` and add a separate `foreground daemon: LIVE` if detected, or
- make `stores agents run` write a pidfile even in foreground mode and teach `stores agents stop` to stop it safely.

This is small, visible, and improves trust immediately.

## Handover instruction

Start by checking:

```bash
stores tasks status T141
stores tasks status T142
stores engine plan-start
pgrep -af 'stores agents run'
stores watch
```

Then decide with Blake whether to continue watching T141/T142, fix daemon-liveness display directly on main, or promote one of the next priorities above.
