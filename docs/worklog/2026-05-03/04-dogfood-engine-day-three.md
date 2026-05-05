# Dogfood Engine Day Three

**Date:** 2026-05-03
**Type:** note

## Summary

The day the engine started building itself. Three substrate tasks shipped clean (T013 filing primitives, T016 plan-reviewer brief plumbing, T017 schema migrations); one mega-task in flight (T014 autonomous flow engine, on Phase 7/7 at end of session); one pending task filed but waiting on T014 (T019 post-accept ceremony — cargo install + schema migrate as L018 watcher subscribers). 16 new observations filed across the session — most surfacing live during real work, exactly as the dogfood doctrine predicts.

The conversation resolved several architectural questions that had been hanging since 2026-05-02: filing happens with at-least-drafted contracts (no auto-confirm), tasks all run on their own task-branch (uniform — tier modulates agent behavior, not pipeline shape), sandboxing is deferred indefinitely (worktree + Claude Code permissions.deny is enough), policy default-action is ALLOW (matches "everything flows between the gates" doctrine), and inter-agent context flow needs schema-enforced templating (compile-error vs runtime-error analogy).

By session end, **autonomous propulsion is ~50% to GO**. T014 + L010 (in-flight as T019) are the remaining critical-path items.

## Details

### What shipped

| task | what | tier | phases | cycles | wall time |
|---|---|---|---|---|---|
| T004 | observations close-from-open verb | T1 | 3/3 | 1c each, 0 REVISE | ~10 min |
| T013 | filing primitives (intent_contract.* at add; --lock-contract; tasks.tier_hint + inheritance) | T2 | 4/4 | 1c each, 0 REVISE (after one amend) | ~25 min agent + amend cycle |
| T016 | plan-reviewer brief renders decision_matrix | T1 | 2/2 | 1c each, 0 REVISE | ~5 min |
| T017 | `stores migrate` verb (additive-only schema sync, dry-run + --apply) | T2 | 3/3 | 1c each, 0 REVISE | ~10 min |

Plus: T003 (./dev worktree script) was discovered unmerged on disk (the prior session's handover overstated "all merged"); merged inline into main as commit `8f6d451`. T013's hex token round-trip fix from 2026-05-02 (commit `82501d3`) propagated through all subsequent installs.

**Net code shipped today**: ~2400 lines (T004 ~30, T013 ~890, T016 ~170, T017 ~855, plus inline observation-cleanup verbs).

### What's in flight

- **T014** — autonomous flow engine (L018 + L022 + L026). 7 phases. P1-P6 all PASS first-cycle. P7/7 executor running at session end. Estimated ~10-15 min remaining when last checked. This is the engine task: agents.yaml registry, daemon (`stores agents run`, polling 5s), policies.yaml predicate evaluator, builtin:accept-merge subscriber, deploy_blocked state, builtin:user-escalation routing, `stores agents backfill` one-off verb.

- **T019** — post-accept ceremony. Filed but not driven. Depends on T014. Two new builtins (`builtin:cargo-install`, `builtin:schema-migrate`) chained after accept-merge, so binary + DB schema auto-update on every accept.

### Doctrine resolved this session

1. **Two-gate model** (front gate + back gate). User articulated explicitly that the only halts should be (a) intent contract lock at filing and (b) tasks accept. Everything between flows magnetically. U2 (promotion) and U4 (resume) collapse into policy-mediated transitions.

2. **Filing carries drafted contracts** (L029 — shipped via T013). All filings produce at minimum a draft intent_contract; approval (`contract_state: draft → ready`) is the U-moment. ai_autonomous filings stay draft, queue for review. ai_with_human + `--lock-contract` lands ready at birth.

3. **Uniform task-branch dispatch** (L030 — pending). Drop the T1=direct-on-main idea; every task gets its own branch. Tier modulates AGENT behavior via brief content (T1 planner produces 1-phase plan; T3 produces multi-phase), not pipeline shape (no T1Runner / T3Runner trait variants).

4. **Sandbox deferral** (L031 — pending). Worktree + `.claude/settings.json` permissions.deny is the substrate-recommended isolation pattern. Container/VM sandboxing deferred indefinitely; revisit only on a real incident. Agents run on host; project-side docker (e.g. 10.06 client work) accessed normally.

5. **Default-allow policy** (revision mid-session). When daemon sees a transition with no policy match, FLOWS (default ALLOW). Matches "everything flows between the gates" doctrine. The U-moments are already protected by per-field actor enforcement; policy governs non-U transitions; default-allow + explicit halt rules works correctly.

6. **Schema-enforced context flow** (L035 — pending, T3). Inter-agent context should be typed with template references (`${decision_matrix.X.chosen}`); substrate validates resolution at write-time, brief generator substitutes at render-time. Compile-error vs runtime-error. The deepest architectural insight of the day; pairs with L018+L022 to make the engine type-safe.

7. **Executor scope is intentional** (correction to L033). Executor is a narrow code-writer; not seeing decision_matrix is by design. The fix is plan-reviewer brief plumbing (which T016 just shipped) and schema-enforced refs (L035) so plans don't write "see X" pointers consumers can't follow.

### Frictions captured (16 new observations)

L022 — Policy-based pre-authorization (refined mid-session per default-allow doctrine; partially shipped via T013/T014 architecture)
L023 — observations missing next-id verb; --json shape inconsistent
L024 — tasks have no tier_hint field — **SHIPPED via T013, closed**
L025 — `./dev` not tier-aware (superseded by L030 — uniform task-branch)
L026 — accept doesn't merge feature branch (engine work via T014)
L027 — tier-driven execution architecture (superseded by L030)
L028 — drive-spawned agents lack /observe affordance + provenance
L029 — drafted-contract-at-filing schema relaxation — **SHIPPED via T013, closed**
L030 — uniform task-branch + tier-as-planner-input briefs (pending)
L031 — defer sandbox; worktree + permissions.deny pattern (pending)
L032 — `./dev new` worktree has no `.stores/` visibility — substrate verbs fail from inside
L033 — plan_reviewer brief drops decision_matrix — **SHIPPED via T016, closed**
L034 — wrap-agent misreading git diff direction (filed by parallel agent session)
L035 — schema-enforced context flow (architectural T3, pending)
L036 — (filed by parallel agent session)
L037 — schema migrations needed on binary upgrade — **SHIPPED via T017, closed**
L038 — task dependency enforcement + chains; depends_on field exists but unused

Plus L020 fired LIVE ~10x during drives (stale state directories accumulating in tasks/active/, tasks/planning/, tasks/paused/ simultaneously). Annoying but not blocking; render warns and writes to canonical path anyway.

### Substrate behaviors observed

- **Resume-from-killed-state works correctly.** When T004's drive was killed mid-execution then re-fired, it picked up at the right phase/cycle without re-planning. Same for T013 after server restart.
- **Three concurrent drives ran without contention** (T014 + T016 + T017). 5-min claim lock per row prevents same-row collisions; different-row drives are independent. Sweet spot is 2-3 parallel; 10 would create real merge pain.
- **Plan-review NEEDS_WORK auto-blocks at first non-PASS** despite blocked_reason saying "cycle limit ≥ 3" (only 1 entry visible). Either the threshold is misnamed or the substrate auto-blocks regardless of count. Behavior is correct (don't waste cycles re-planning against bad contract) but the message is misleading.
- **Auto-merge handled the T017-branch-behind-main case correctly.** T017 was forked pre-T013/T016; standard 3-way merge preserved both sides' contributions. The wrap brief over-cautioned about "reversions"; in practice no manual conflict resolution needed.
- **Dogfood test: T017's `stores migrate` correctly identified live DB as in-sync** after we'd manually `ALTER TABLE`'d tier_hint post-T013. The very feature T017 ships, used to verify the substrate state. Idempotency clause holds.

### Substrate frictions hit live

- **Binary rebuild during drive drops feature flags** (L009 fired). T004 P2's executor ran `cargo install` without `--features runner-claude-code`; the live binary lost its `--claude-code` flag entirely. Reinstalled inline. Will keep happening until L009 ships (Cargo.toml default-feature flip).
- **Schema migration needed manual ALTER TABLE** (L037 surfaced). T013's tier_hint addition compiled into the new binary, but the live DB wasn't migrated. Every `stores tasks ...` SELECT broke. Fixed via `sqlite3 .stores/db.sqlite "ALTER TABLE tasks ADD COLUMN tier_hint TEXT;"`. T017 now ships the verb that does this automatically.
- **Worktree has no `.stores/` directory** (L032 surfaced). `./dev new` creates a worktree, but `.stores/` is gitignored so the worktree can't run substrate verbs from inside. Worked around by running drives from main (workspace_path routes agent spawns into worktree). Real fix is delegation manifest.
- **`./dev new` next-id collision** when scaffolding two tasks back-to-back. Filesystem-based `tasks next-id` returns the same ID twice if the first task's render hasn't materialized yet. Worked around by `tasks render T###` between scaffolds. Worth a small substrate fix.

### Where we are vs. North Star

```
[██████████████████░░░░░░░░░░░░░░░░░░░░░░] ~50% to autonomous propulsion GO

Critical-path:
  ✅ T013  filing primitives (typed contracts at add)
  ✅ T016  plan-reviewer brief plumbing (bonus, not critical-path)
  ✅ T017  schema migrations
  🔄 T014  engine (P7/7 — last phase + wrap)
  ⚪ T019  post-accept ceremony (filed, depends on T014)
  ⚪ L038  task dependency guard (Layer 1, T1 — could ship between T014 and T019)
```

After T014 + T019 land + (optionally) L038's Layer 1 ships, the loop closes:

```
file → drafted contract → user approves → watcher promotes →
  drive runs → user accepts → branch merges → schema migrates →
  binary installs → linked obs auto-resolve → engine consumes engine
```

## Follow-ups

- **T014 wrap review** — when it lands. Will be the largest task acceptance to date (7 phases, multi-component). Pay close attention to: (a) builtin:accept-merge tested behavior; (b) deploy_blocked transition correctness; (c) policies.yaml predicate evaluator (default-allow semantics); (d) `stores agents run` daemon polling lifecycle.
- **Drive T019** — once T014 accepts + merges + cargo installs (which we'd still do manually pre-T019). T019 then ships the post-accept ceremony so subsequent accepts auto-update binary + DB schema.
- **Promote L038 Layer 1** — small T1 task to make `tasks drive` honor `depends_on`. Could ship between T014 and T019 so T019's depends_on=T014 actually gets enforced. Recursive: the dep-check feature ships between the two tasks that hit the dep-gap.
- **Auth UX cluster** (L013/L014/L015) — deferred today; still worth a tight T1 bundle when the queue has bandwidth.
- **L020/L021/L023 papercuts** — defer until engine lands; render hygiene + observations next-id flow naturally as cleanup work post-engine.
- **L030 + L035** — the next big architectural bets. L030 (tier-as-planner-input briefs) is T2; L035 (schema-enforced context flow) is T3 and the deepest. Both pair with the engine.

### Stats

- Substrate tasks shipped: 4 (T004, T013, T016, T017) + T003 merge-recovery
- Substrate tasks in flight: 1 (T014, P7/7) + 1 filed-not-driven (T019)
- Friction observations filed: 16 (L020 already filed; L022-L038 minus parallel-session L034/L036)
- Substrate hotfixes: 0 (one ALTER TABLE manual workaround for L037; none against shipping code)
- REVISE cycles across 4 shipped tasks: **0**
- Concurrent drives ran simultaneously: 3 at peak (T014 + T016 + T017)
- Doctrine clarifications captured: 7 (two-gate model, drafted contracts at filing, uniform task-branch, sandbox deferral, default-allow policy, schema-enforced context flow, executor scope intentional)
- Token-mediated U3 acceptances: 4 (T004, T013, T016, T017)
