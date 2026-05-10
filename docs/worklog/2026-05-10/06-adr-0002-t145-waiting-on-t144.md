# ADR 0002 / T145 Waiting On T144

**Date:** 2026-05-10
**Type:** note

## Summary

ADR 0002 is committed at `docs/adr/0002-inlet-triage-and-observation-routing.md` (`28ab674`). Its implementation observation was ratified and auto-promoted to `T145`.

`T145` is intentionally stopped until `T144` lands on main. It overlaps the same watch/read-model/lifecycle surfaces that `T144` is changing for ADR 0001, so starting it before `T144` integrates risks planning/executing against stale architecture.

## Details

Current intended dependency:

```text
T144 integrated / schema_migrated on main
  -> resume T145
  -> activate T145
  -> let auto-drive handle planning/execution
```

Current T145 state after stopping the accidental planner dispatch:

```bash
stores tasks status T145
# expected: status=blocked, activation=inactive, recoverable
```

Check T144 first:

```bash
stores tasks status T144
stores engine plan-start
```

Only unblock T145 after T144 has reached a terminal/integrated state on main, preferably `schema_migrated` for this repo.

## Follow-ups

When T144 is done and merged/integrated, unblock T145 with:

```bash
TOKEN=$(stores auth show)
stores tasks resume T145 \
  --invoker ai_with_human \
  --approve-token "$TOKEN" \
  --summary "T144 has landed; resume ADR 0002 implementation."

stores tasks activate T145 \
  --invoker ai_with_human \
  --approve-token "$TOKEN" \
  --reason "T144 ADR 0001 lifecycle slice has landed; start ADR 0002 upstream read-model slice."

stores engine plan-start
```

Do not install token-bearing unattended automation for this handoff. The safer handoff is an explicit operator action after verifying T144 is integrated.
