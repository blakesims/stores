# Take Off Handover

**Date:** 2026-05-03
**Type:** note

## Summary

**Read first**: `/CLAUDE.md` — dogfood doctrine, U-moments, token mechanism. This handover assumes you've internalized that.

**Where we are**: ~75% to autonomous-propulsion GO. Five tasks shipped clean today (T004, T013, T016, T017, T014). T019 (the last critical-path task before GO) is in flight — `executing phase=1/5`. After T019 ships and one acute regression is hot-fixed, the engine is ready to run autonomously.

**The bar for GO**: file an observation → user approves contract → watcher promotes to task → drive runs → user accepts → branch auto-merges → binary auto-rebuilds → DB schema auto-migrates → engine consumes engine. Front gate (contract approval) and back gate (acceptance) are the only halts.

## Details

### What is shipped on main

- **T013** (filing primitives): `intent_contract.*` settable at `observations.add`; `--lock-contract` shorthand; `tasks.tier_hint` enum field with inheritance from linked observations.
- **T016** (brief plumbing): plan_reviewer brief renders the planner's `decision_matrix` (sits between Current Plan and Prior Plan Reviews).
- **T017** (schema migrations): `stores migrate` verb (additive-only; dry-run default; `--apply` mutates inside a transaction). Idempotent. Detects orphaned columns + type mismatches as warnings.
- **T014** (autonomous flow engine): the big one. Ships:
  - `src/flow/` module: `agents.yaml` + `policies.yaml` parsers, predicate evaluator (5 operators, default-ALLOW, NEVER-sacrosanct, SHA-256 hashed for audit), config.yaml reader, ntfy notifier
  - `src/handlers/agents_run.rs` (902 LOC): polling daemon, SQLite UNIQUE-claim idempotency, SIGTERM graceful shutdown, policy gating with `STORES_POLICY_REF` env passthrough
  - `src/flow/builtins/`: `accept-merge` + `user-escalation`
  - `stores agents backfill` one-off verb
  - Schema: `deploy_blocked` state + `transition_history` audit table

### What's in flight RIGHT NOW

- **T019** — post-accept ceremony. Adds `builtin:cargo-install` + `builtin:schema-migrate` chained after `builtin:accept-merge`. After this lands, accept → merge → install → migrate is fully autonomous.
- Drive ID: see `pgrep -af "tasks drive T019"`. Output stream: `/tmp/claude-1000/.../<id>.output`.
- Status check: `stores tasks status T019 --invoker ai_autonomous`.
- One known landmine: T019's first attempt produced an empty plan (transient SDK flake). Retry succeeded. If you see another flake, retry once before deeper investigation.

### What's still BETWEEN you and GO

After T019 lands:

1. **Accept T019** (back gate U3 — needs token: see "Operating environment" below).
2. **Merge** `feat/T019-post-accept-ceremony` into main. Expect 1-2 small conflicts on `src/handlers/mod.rs` and possibly `src/flow/builtins/mod.rs` (handle by preserving both sides). The branch was forked pre-T013/T014; 3-way merge handles most cleanly per recent precedent.
3. **`cargo install --path . --features runner-claude-code --quiet`** to ship the new binary.
4. **`stores migrate --apply`** to sync DB schema (T017's verb).
5. **Hot-fix L042 regression** (see Known Landmines below) — until that lands, every drive is a black box for debugging.
6. **(Optional) ship L038 Layer 1** (depends_on enforcement, ~30-50 LOC, T1) — guards against the drive-without-deps-met footgun we hit twice today.
7. **Start the daemon**: `stores agents run --poll-interval 5` (foreground) or `--detach`. Engine begins consuming the queue.

### Operating environment

- **Branch**: `main` (ahead of remote `origin/main`).
- **Worktrees**: `../stores-T019-post-accept-ceremony` is in flight; teardown after accept via `./dev done T019 --force`.
- **Token**: the user's pre-decrypted approval token is in this conversation context. If you're a fresh agent, you don't have it yet — ask the user to paste it (`stores auth show` decrypts; needs passphrase or hardware tap; user-presence-bound).
- **Binary**: `~/.cargo/bin/stores` post-T014 reinstall. Verify with `stores --version` and `stores tasks drive --help | grep claude-code` (the latter must show — if missing, **L009 fired again**, run `cargo install --path . --features runner-claude-code --quiet` to fix).
- **DB**: `.stores/db.sqlite` has columns through T013's `tier_hint` and T014's `transition_history` table (auto-created via SUBSTRATE_DDL on first connection post-T014).

### Known landmines (in order of urgency)

1. **L042 — run-log transcript capture is BROKEN** (regressed by one of T013-T014 today). Files in `.stores/runs/` are 0 or 66 bytes since ~16:09 UTC. Drives still functionally work (final submission lands on the row), but **debugging any flake is now black-box**. Hot-fix is small but mandatory before serious autonomous propulsion. See L042 body for the bisect/inspect plan.

2. **L009 — `runner-claude-code` is not a default Cargo feature**. Any `cargo install` without `--features runner-claude-code` silently drops the `--claude-code` flag from `stores tasks drive --help`. The substrate then refuses to spawn agents. Fix is one Cargo.toml line. Until then, every reinstall must explicitly pass the feature flag.

3. **L020 — stale state directories**. Drive renders create entries in `tasks/active/`, `tasks/planning/`, `tasks/paused/` over time. Render warns ("multiple task directories found") but writes to canonical path anyway. Cosmetic; not blocking.

4. **L032 — worktree has no `.stores/` visibility**. `./dev new` creates worktrees, but `.stores/` is gitignored so substrate verbs fail from inside the worktree. Workaround: always run `stores tasks drive` and other substrate verbs from the **main worktree** (`/home/blake/repos/experiments/stores`). Drive routes agent spawns into the worktree via `workspace_path`.

5. **`./dev new` next-id collision** when scaffolding two tasks back-to-back. After `./dev new` succeeds, run `stores tasks render T###` to update filesystem before the next `./dev new`. Otherwise filesystem-based `next-id` returns the same ID twice and the second creation rolls back.

6. **L038** (filed but un-promoted) — `tasks.depends_on` is stored but unused. Drive happily fires tasks whose deps aren't accepted. Two layers in the body; Layer 1 (passive guard, ~30-50 LOC, T1) is the high-leverage immediate fix.

7. **Empty planner submission** (T019's first attempt) — appears to be transient SDK/API flake. If you see another, retry once before investigating.

### Doctrine the next agent must internalize

- **Two-gate model**: U-moments are FRONT GATE (contract lock at observations.add or task add) and BACK GATE (`tasks accept`). Everything else flows. Don't add gates that aren't already in the schema.
- **Filing carries drafted contracts** (L029, shipped via T013): all observations land with at least a draft `intent_contract`. AI-autonomous filings stay draft; user-present filings can `--lock-contract` to land ready at birth.
- **Uniform task-branch dispatch** (L030, pending): every task gets a branch via `./dev new`. Tier modulates AGENT brief content (planner sees tier; produces 1-phase or multi-phase plan accordingly), NOT pipeline shape. There is no T1Runner / T3Runner.
- **Sandbox deferral** (L031, pending): worktree + Claude Code `permissions.deny` is the substrate-recommended isolation. Container/VM sandboxing deferred indefinitely until a real incident.
- **Default-allow policy** (T014 shipped): when no policy in `policies.yaml` matches, the daemon FLOWS. Policies are exceptions (NEVER + conditional halts). Per-field `actor` enforcement protects U-moments at the schema level.
- **Executor scope is intentional**: executor is a narrow code-writer; it sees only its current phase + contract. Don't widen its brief; if context is missing, that means the planner's plan should have inlined what the executor needs (or L035's schema-enforced refs should ship).
- **Schema-enforced context flow** (L035, pending T3): inter-agent context flow should be type-checked via template references at write-time, not discovered at runtime. Compile-error vs runtime-error analogy.

### Substrate verbs cheat-sheet (the verbs you'll actually use)

```bash
# Filing
stores observations add --invoker ai_autonomous --summary ... --body-from-file ... \
  --source dev --priority normal --tier-hint T2 --task-id T0XX
stores observations close_as_addressed L0XX --resolution T0XX --invoker ai_autonomous

# Task scaffolding (uses ./dev wrapper which does the worktree dance)
./dev new --slug=foo --title=... --done-when=... --scope-in=... --scope-out=...
stores tasks update T0XX --linked-observations L0XX,L0YY --tier-hint T2 --invoker ai_autonomous
stores tasks update T0XX --depends-on T0YY --invoker ai_autonomous   # depends_on stored but unused

# Driving
stores tasks drive T0XX --claude-code --invoker ai_autonomous   # always run from MAIN worktree
stores tasks status T0XX --invoker ai_autonomous
stores tasks brief T0XX --for planner --invoker ai_autonomous   # debug; doesn't spawn

# U-moments (token in session — paste it OR user types)
stores tasks accept T0XX --invoker ai_with_human --approve-token <T>
stores tasks resume T0XX --invoker ai_with_human --approve-token <T> --summary "reason"

# Recovery
stores tasks update T0XX --done-when "..." --invoker ai_with_human --approve-token <T>   # amend after blocked
sqlite3 .stores/db.sqlite "UPDATE tasks SET plan=NULL, status='planning' WHERE display_id='T0XX';"
                                                # ^ ONLY when nothing else works; bypasses transition rules
```

### What NOT to do (this session's hard-won lessons)

- **Don't skip `stores tasks render T###` between back-to-back `./dev new` calls.** Filesystem-based next-id collides.
- **Don't run `cargo install` without `--features runner-claude-code`.** Silently drops the feature.
- **Don't run substrate verbs from inside a worktree.** Always from main; drive routes agents via workspace_path.
- **Don't try to amend a `blocked` task** (`amend` requires `rejected` state). Use `tasks update --done-when ...` instead, then `tasks resume`. The plan field gets stale; substrate doesn't auto-replan after amend (this is L038-flavored). For radical replans, do the SQL surgery to reset plan + status.
- **Don't accept a task without checking the wrap brief's `deviations[]` and `residual_risks[]`** sections. Both can flag honest disclosures the executor wants you to know about.
- **Don't give the orchestrator privileged channels into the substrate.** Wrapper boundary doctrine. Re-read `docs/philosophy.md` § *What's outside the substrate* if tempted.

## Follow-ups

For the next agent, in priority order:

1. **Wait for T019 to land**, review wrap brief, accept (token), merge (expect minor conflicts), reinstall binary, run `stores migrate --apply`.
2. **Fix L042 regression** (run-log capture). Bisect or read the diff; small patch task. Mandatory before serious propulsion.
3. **Optionally ship L038 Layer 1** (depends_on guard) as a small T1 task. ~30-50 LOC. Removes a footgun we hit repeatedly today.
4. **Start the daemon** — `stores agents run --poll-interval 5`. Engine begins.
5. **Watch what flows and what doesn't.** Default-allow policy means most non-U transitions auto-fire. ntfy-on-halt tells you when policy DIDN'T apply (signals "I expected this to flow, why didn't it?").
6. **L009 default-feature fix** (Cargo.toml) — one-line change; eliminates the silent feature-drop on every reinstall.
7. **L030** (tier-as-planner-input briefs) — separate medium task; the agent-side of the tier system.
8. **L035** (schema-enforced context flow) — T3, biggest architectural follow-up; finishes the schema-as-engine doctrine for inter-agent boundaries.

### What's deferred / low priority

- **Auth UX cluster** (L013/L014/L015) — drains 3 observations; T1 bundle when queue has bandwidth.
- **L020 / L021 / L023** papercuts — render hygiene + observations next-id; small individually, accumulating annoyance, defer until engine is running.
- **L019 DockerRunner** — deferred indefinitely per L031 unless real incident demands it.
- **L012 inspector** — T3 observability uplift; useful eventually but not blocking GO.

### One last thing

The substrate's quality ceiling is set by what surfaces when actual work runs through it. The ratio is: real use surfaces what real use surfaces; planning, code review, and tests do not replace it. **Run the engine. File observations as friction surfaces. Promote them. Drive them. Ship them. Repeat.** The engine consumes itself.
