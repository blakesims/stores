# Flow: observation → task → done

**Date:** 2026-05-03
**Type:** note

## Summary

The full lifecycle of a substrate item — from a friction observation being filed to the resulting work shipping to production — is a 10-step pipeline with **2 human U-moments** and **8 autonomous edges**. Today, only the post-accept ceremony (steps 7–9) is wired through the daemon (T014 + T019, shipped). Steps 4–5 are in flight (T020: auto-promote + auto-scaffold). Steps 2, 3, 6, 10 are unfiled or filed-but-unpromoted. Until those land, the orchestrator-on-main is the carrier for everything between U1 (contract ratification) and U3 (task acceptance) — by hand-crank, sequentially, blocking the user's main thread.

The point of capturing this now: the autonomous-flow doctrine (per philosophy.md v1.2) commits to "two-gate model — front gate is contract ratification, back gate is task acceptance, everything else flows." That's the design endpoint. Today's reality is "two-gate plus eight orchestrator-driven edges in between." The gap names the remaining work.

## Details

### The 10-step pipeline

```
OBSERVATION                                     TASK
┌─ 1. file (autonomous; orchestrator) ─┐
│      observations.add                 │
│      status=open                      │
└─────────────┬─────────────────────────┘
              │
┌─ 2. triage (autonomous; orchestrator;
│      L043 routing rule: ≤3 cheap tool
│      calls; if root-cause-not-obvious,
│      mark needs_investigation) ───────┐
│   ┌─→ wont_fix    (premise wrong)
│   ├─→ resolved    (already done)
│   └─→ needs_investigation OR investigating
└─────────────┬─────────────────────────┘
              │
┌─ 3. investigate (autonomous; investigator
│      subagent — L043, NOT YET SHIPPED)│
│      produces investigation_note +    │
│      drafts intent_contract           │
│      → status=confirmed,              │
│        contract_state=draft           │
└─────────────┬─────────────────────────┘
              │
   ╔══════════╪══════════════╗
   ║ U1: RATIFY              ║ ← human moment + token
   ║ contract_state=ready,   ║
   ║ approved_by, approved_at║
   ╚══════════╪══════════════╝
              │ framework auto-transition
              ▼
   status=ready (on observation)
              │
┌─ 4. auto-promote (T020, IN FLIGHT) ───┐
│      reads intent_contract            │
│      creates task row at planning     │
│      writes linked_observations       │
│      writes observation.task_id back  │     ┌──────────────────┐
└─────────────────────────────────────────────│ task: planning   │
                                              └────────┬─────────┘
┌─ 5. auto-scaffold (T020, IN FLIGHT) ────────────────┤
│      project-configurable scaffold cmd              │
│      provisions feat branch + worktree              │
│      writes task.workspace_path                     │
└──────────────────────────────────────────────────────┤
                                                       │
┌─ 6. auto-drive (NOT YET FILED) ─────────────────────┤
│      spawns planner → reviewer →                    │
│      executor (per phase) → reviewer →              │
│      wrap                                            │
│      → status=in_review with envelope               │
└──────────────────────────────────────────────────────┤
                                                       │
                                              ┌────────▼──────────┐
                                              │ task: in_review   │ wrap envelope
                                              └────────┬──────────┘
                                                       │
                                          ╔════════════╪═══════════════╗
                                          ║ U3: ACCEPT                 ║ ← human moment + token
                                          ║ tasks accept TXXX          ║
                                          ╚════════════╪═══════════════╝
                                                       │
┌─ 7-9. post-accept ceremony (T014+T019, SHIPPED) ────┤
│      accept-merge → cargo-install →                 │
│      schema-migrate                                  │
└──────────────────────────────────────────────────────┤
                                                       │
                                              ┌────────▼──────────┐
                                              │ schema_migrated   │
                                              └────────┬──────────┘
                                                       │
┌─ 10. auto-resolve-observation                        │
│        (NOT YET FILED) ──────────────────────────────┤
│      fires on tasks → schema_migrated                │
│      sets observations.status = resolved             │
│      resolution = task.commit_sha                    │
│      for every linked_observations entry             │
└──────────────────────────────────────────────────────┘
```

**Human time per item: U1 (~30s with token) + U3 (~30s with token).** Everything else is supposed to be autonomous.

### What's where

| Step | Status | Carrier |
|---|---|---|
| 1. File | shipped | `observations.add` |
| 2. Triage | partial | orchestrator discipline; L043 unfiled-as-task adds `needs_investigation` state |
| 3. Investigate | missing | L043 filed (investigator subagent); needs T020 to ship first to flow through |
| 4. Auto-promote | in flight | T020 (the bootstrap) |
| 5. Auto-scaffold | in flight | T020 sibling, same task |
| 6. Auto-drive | unfiled | next observation to file once T020 lands |
| 7-9. Post-accept | shipped | T014 + T019 |
| 10. Auto-resolve | unfiled | small builtin; closes the loop back to observations |

### Design seams worth deciding

1. **Tier modulates path or just brief?** A T1 fix could skip plan-review (one phase, deterministic). Auto-drive could detect tier_hint=T1 and call a leaner cycle (executor + code-reviewer, no planner). Or tier just changes brief content and the cycle is uniform. **Lean: uniform cycle, lean brief.** Surprise paths are bug factories; the planner producing a 1-phase plan vs a 5-phase plan is enough modulation.

2. **What's the failure state for each autonomous edge?** Today there's `deploy_blocked` for post-accept failures. For pre-accept failures (auto-scaffold can't get a branch, auto-drive's planner crashes 3×, plan-review NEEDS_WORK ≥ N times), do we want `scaffold_failed`/`drive_failed`/etc., or one umbrella `blocked` with `blocked_reason`? **Lean: one `blocked` state with `blocked_reason` + `last_failure_step` field.** Simpler schema; reason field disambiguates; user-escalation subscribes to `→ blocked` once and routes by reason.

3. **U-moment collapse: U1 == U2.** Old doctrine said U1 = ratify contract, U2 = promote to task. With auto-promote in T020, ratifying IS the act that produces a task. U2 disappears as a distinct moment. **CLAUDE.md needs revising** to "U1=ratify (auto-promotes), U3=accept, U4=resume." Two steady U-moments per item.

4. **Should triage become its own state?** L043 proposes `needs_investigation`. We could also formalize a `triaged` state separate from `confirmed`. **Lean: don't.** `open → confirmed → ready → in_progress → resolved` is enough; `wont_fix` and `needs_investigation` are special-purpose dead-ends/branches. Adding more granular states without earning them creates a state-explosion problem.

5. **Cross-store edges are first-class.** Auto-promote (observation→task) and auto-resolve (task→observation) cross store boundaries. The substrate's subscription model already supports this (subscribers declare `store: tasks` or `store: observations`); the predicate layer in `policies.yaml` should be store-aware too. **Lean: existing model handles it; document it.**

6. **The investigator's output is U1-precursor, not U1.** The investigator drafts the contract; the human still ratifies. Don't let the investigator land contracts at `ready` even with a token (it shouldn't have one). **Lean: investigator writes draft only; human ratification is the only path to `ready`.** The schema already enforces this via `actor: human` on `approved_by`/`approved_at`.

7. **Multi-observation tasks** (linked_observations as list). T020 carries L046+L047. Auto-resolve should resolve both with the same commit sha when T020 lands. **Lean: straight loop over `linked_observations`; each one resolves with the same resolution value.** No multi-issue logic.

8. **U-moment grounding is transitive through framework edges.** When auto-promote runs as `ai_autonomous` (the daemon) and creates a task row, the resulting row is "born of human assent" via the chain — U1 was the human-grounded write of `approved_by`/`approved_at` on the observation, and the substrate trusts the chain after that. The schema enforces this at write-time: only after U1 is the observation in a state the auto-promote subscription matches. This is a useful generalization: framework-fired edges propagate authorization downstream without re-asking.

### The orchestrator-on-main's autonomy budget

In the destination state (after T020 + L043 + auto-drive ship):

- **Triage** (3-tool-call discipline): is this real, scoped, actionable? Route to wont_fix / resolved / needs_investigation / inline-draft.
- **Surface** wrap briefs and halts to the user for U-moments.
- **Talk design** (this conversation; meta-discussions; cross-cutting decisions).
- **Stay out of**: investigation (L043), driving (auto-drive), scaffolding (T020), promotion (T020), deploy (T019).

Today's reality:

- **Hand-crank** every item between U1 and U3. Re-key contract content into `./dev new` flags. Backfill linked_observations via separate update. Sequentially type `tasks drive` per task. Wait for each cycle.
- **Investigate inline** when something surfaces (the L043 anti-pattern). Today's cost: ~15 tool calls + several minutes of user-thread blocking on the L042 misdiagnosis.
- **Re-derive context** every conversation turn. Without auto-promote/drive/resolve, the orchestrator carries the lifecycle in conversational memory, not in the substrate.

### Filing rubric for the missing steps

Mapping the destination-state design back to the filing rubric:

| Step | Observation already filed? | Promotable now? |
|---|---|---|
| 2 (triage routing rule) | partial: L043 covers needs_investigation routing | yes, after T020 |
| 3 (investigator subagent) | L043 (filed; ratified contract pending) | yes, after T020 |
| 4 (auto-promote) | L046 (filed; contract ratified; in T020) | n/a, in flight |
| 5 (auto-scaffold) | L047 (filed; linked to T020 via inputs) | n/a, in flight |
| 6 (auto-drive) | NOT FILED | needs filing → ratify → promote |
| 10 (auto-resolve) | NOT FILED | needs filing → ratify → promote |

So the upstream-flow batch beyond T020 is: file 2 more observations (auto-drive, auto-resolve), promote L043 (investigator), and watch them ship in some order through the increasingly autonomous pipeline.

## Follow-ups

1. **Watch T020 land.** Currently in flight; once accepted + deployed, the next ratification of any observation triggers steps 4-5 autonomously.
2. **File "auto-drive subscriber" observation** — the next missing edge. Trigger: task at status=planning with workspace_path set; spawns the existing `tasks drive` flow as a daemon-driven action. Pairs with auto-promote + auto-scaffold to close steps 4-6.
3. **File "auto-resolve-observation subscriber" observation** — closes the loop. Trigger: task → schema_migrated; updates each entry in linked_observations with status=resolved, resolution=<task.commit_sha>.
4. **Promote L043 (investigator subagent)** once T020 ships — fills the gap at step 3 (drafting contracts autonomously).
5. **Revise CLAUDE.md U-moment doctrine** to collapse U1+U2 into "U1: ratify (auto-promotes)" once T020 ships. Keep U3 (accept), U4 (resume/amend). Two steady U-moments per item.
6. **Decide design seam #2** (failure state shape: one `blocked` with reason vs many specific blocked-states) before auto-drive ships, since auto-drive will be the first non-deploy edge with rich failure modes.
7. **Audit the observation lifecycle for the framework auto-transition** that maps `intent_contract.contract_state=ready + approved_by/at set` → `observations.status=ready`. T020's planner has to design this; if it lands cleanly, it's the prototype for any future field-level-write→state-transition pattern (a generalization L035 will eventually formalize).
