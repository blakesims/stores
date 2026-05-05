# Autonomous Flow Foundation Handover

**Date:** 2026-05-03
**Type:** note (handover for fresh agent)

## Summary

> **Note on observation IDs:** This note was drafted before two mid-handover audit observations were filed. **L020** in the substrate is *not* the policy observation described here — it's the substrate-stale-state-folders bug. **L021** is the wrap_log-not-in-Completion-section gap. The policy observation, when filed by the next agent, will be **L022**. References to L022 throughout this note refer to the *yet-to-be-filed* policy observation.

End-of-session design discussion on what un-clogs the substrate so observations flow autonomously through to fixes, with human gates only at codified U-moments. The current session shipped three substrate tasks (T001/T002/T003) and filed 13 friction observations (L007–L019) — but most of those observations sit `open` waiting for individual U-moment ratification. The user's prompt: *"how can we un-clog this? what's at the top of the blockage so we can essentially have autonomous updates? eg. we file an observation and the data just moves through it except and only if there is a human gate?"*

The answer: **two paired layers** are the foundation. Without both, no autonomous flow.

1. **Event / watcher infrastructure** — substrate as event bus; agents subscribe to state transitions; daemon mode dispatches. Already filed as **L018** (CI/CD-watcher brief draft).
2. **Policy-based pre-authorization** — declarative `.stores/policies.yaml` codifies what's pre-authorized to flow autonomously vs. what halts at U-moments. **NOT YET FILED** — proposed as L022 in this note. The new agent's first action.

The next agent picks up from here. L022 needs filing (autonomous), then the user's `go` to promote a unified "Autonomous Flow Foundation" task cluster bundling **L018 + L022 + L017** (lifecycle papercut: no close-from-open path) as one substrate task.

## Details

### The 5-layer blockage analysis

```
Layer 1 — Event / watcher infrastructure (L018, filed)
            without it, nothing autonomously fires anything
                         ↓
Layer 2 — Policy-based pre-authorization (L022, NOT YET FILED)
            without it, every U-moment still needs the user mid-flow
                         ↓
Layer 3 — Lifecycle papercuts
            L017 (no close-from-open path on observations)
            L009 (runner-claude-code should be default feature)
            L010 (no automated release step on accept)
            L011 (rows don't record stores binary version)
                         ↓
Layer 4 — Project-side `./dev ship` + linked-obs auto-close
            FIRST watcher subscriber; closes the deploy loop
            depends on L018 + L017 landed first
                         ↓
Layer 5 — Review surface (L012, presidential-brief)
            makes the U-moments that DO fire efficient (1-min decision)
```

**Top of blockage = Layers 1+2 paired.** Layer 1 makes flow *possible*; Layer 2 makes U-moments *not gate every step*. The L018 brief I drafted only captured Layer 1 — it assumed U-moments stay one-by-one. The genuinely-autonomous picture needs Layer 2 alongside.

### Why policy is the missing half (the L022 design)

Without policy: even with watchers, every confirm / claim / resolve / accept / resume requires the user mid-flow. The user becomes the dispatcher of every transition, just at higher latency than today.

With policy: the user types policy ONCE; watcher fires individual writes against policy; user is the source of authority (they wrote the rules) but not in the loop of each write. Hard human gates stay (e.g. `auto_accept: NEVER`) — the user can keep any knob set to NEVER forever.

### Proposed `.stores/policies.yaml` shape

```yaml
# Codified pre-authorization rules. The substrate verifies each
# automatic write against policy, records `policy_ref: <rule-id>`
# on the row's audit trail, fires ntfy when policy DIDN'T apply
# (so the user sees what's NOT auto-flowing).

observations:
  auto_confirm:
    when: tier_hint == 'T1' AND scope_in is bounded AND priority != 'high'
    rationale: "T1 obs with bounded scope are mechanical; ratify-on-default."
  auto_resolve:
    when: linked_task.status == 'accepted' AND status == 'open'
    rationale: "Closes L017's gap — fixed-elsewhere obs land resolved."

tasks:
  auto_accept:
    when: NEVER     # acceptance is the last stop; never auto.
  auto_resume:
    when: blocked_reason matches /flaky test/i AND retry_count < 2
    rationale: "Flake retries are mechanical."

deploy:
  auto_ship:
    when: tier_hint <= T2 AND linted_changes_only AND no_schema_migration
    rationale: "Mechanical merges only; T3+ needs review."
```

### Key design properties

- **Audit trail records the policy.** Every auto-flow write carries `policy_ref` referencing the specific rule that fired. Future debugging answers "why did this auto-confirm?" by quoting the policy.
- **NEVER policies are sacrosanct.** The user keeps `auto_accept: NEVER` permanently if they want; nothing in the substrate or watcher can override it.
- **ntfy on what DIDN'T flow.** Equally important: when policy DIDN'T apply (the row went to U-moment instead), the user gets a signal. This surfaces "I expected this to auto-flow, why didn't it?" early.
- **Pairs with L018.** L018's CI/CD watcher consults `.stores/policies.yaml` at every transition. Without policy, the watcher halts at every U-moment. With policy, it flows.
- **Doesn't break tier-A/tier-B doctrine.** Pre-authorization IS user authority — typed once, enforced many times. Different mechanism from per-row token but same principle: human is the source of authority, the AI/watcher is the executor.

### What the new agent should know about today's session

1. **The substrate dogfood is real and producing data.** 13 observations in one session, almost all from real-use friction, not from review. The doctrine "real use surfaces what real use surfaces" verified hard.

2. **The token mechanism works end-to-end** (post-L016 hex-encoding hotfix). `~/.cargo/bin/stores` was bumped twice; current state has L016 fix shipped.

3. **The repo is now public-shaped** — local `master` renamed to `main`, pushed to private `github.com/blakesims/stores`. 57 commits ahead of where master was when session started. Remote default branch is `main`; stale remote `master` deleted.

4. **U-moment doctrine is now layered.**
   - **CLAUDE.md root** updated mid-session to reflect tier-A two-paths: `--invoker human` (literal typing) OR `--invoker ai_with_human --approve-token <T>` (chat-paste). Both equally valid.
   - **The `actor: human` field gate** still exists — relaxed by tier-A token-mediation per T001 P3+P4.
   - **L017** captured the matrix contradiction: investigate transition wants ai_autonomous post-T001, but contract sub-fields want ai_with_human + token. Same row, two transitions, contradictory invokers.

5. **L018 is the watcher brief.** Read it first. Architecture is solid; the gap it doesn't address is policy (this note's L022 proposal).

6. **The wrapper boundary holds.** Stores stays pure; project-side `./dev` does deploy ceremony. The watcher (L018) lives in stores as a Runner-trait-symmetry concept; deploy steps it invokes are project-side scripts.

### L022 — proposed observation body (for the new agent to file)

When the new agent files L022, the body should be:

```
What surfaced 2026-05-03 during end-of-session reflection on the
13 observations filed in this dogfood session:

  Most of them sit `open`, awaiting individual U-moment ratification.
  Even with L018's watcher infrastructure shipped, every confirm /
  claim / resolve / accept transition would still require the user
  mid-flow. The user becomes the dispatcher of every transition, just
  at higher latency than today.

The user's framing (verbatim):
  "how can we un-clog this? what's at the top of the blockage so we
  can essentially have autonomous updates? eg. we file an observation
  and the data just moves through it except and only if there is a
  human gate?"

The observation:
  L018 captured Layer 1 (event/watcher infrastructure) but assumed
  U-moments stay one-by-one. The genuinely-autonomous picture needs
  Layer 2: policy-based pre-authorization codified once by the user,
  enforced on every transition by the substrate.

Proposed shape:
  .stores/policies.yaml declarative rules:
    observations.auto_confirm.when: <predicate>
    observations.auto_resolve.when: <predicate>
    tasks.auto_accept.when: NEVER  (or other policy)
    tasks.auto_resume.when: <predicate>
    deploy.auto_ship.when: <predicate>

  Substrate verifies each automatic write against the rules,
  records policy_ref: <rule-id> on the row's audit trail, fires
  ntfy when policy DIDN'T apply (signals "I expected this to flow,
  why didn't it?").

  NEVER rules are sacrosanct. The user keeps auto_accept: NEVER
  forever if they want.

Pairs with L018:
  L018 = events fire transitions
  L022 = policy decides which transitions skip the U-moment
  Together = autonomous flow with codified human gates

Doesn't break doctrine:
  Pre-authorization IS user authority — typed once, enforced many
  times. Same principle as per-row token: human is the source of
  authority, the AI/watcher is the executor. Different mechanism
  for different cardinality.

Triage tier (suggested): T3 — schema additions, predicate-DSL
parser, watcher integration, audit trail extensions, ntfy hooks.
~3-5 phases. Pairs with L018 in a unified "Autonomous Flow
Foundation" task cluster.

Why high priority:
  This unblocks ALL of the 13 obs filed today. Without it, even
  with watchers shipped, those obs require 13 individual U-moment
  ratifications. The user wants propulsion; this is what makes
  propulsion possible without abdicating authority.
```

### The promotion strategy (after L022 is filed)

The user proposed bundling **L018 + L022 + L017** as one "Autonomous Flow Foundation" substrate task. Multi-phase. The contract sketch:

```
Title:  Autonomous Flow Foundation: events + policy + lifecycle
Slug:   autonomous-flow-foundation
Linked: L018, L022, L017

Done when:
  (1) `.stores/agents.yaml` schema; agent registry; declarative
      subscription to state transitions.
  (2) `stores agents run` daemon mode (poll OR subscribe);
      claim with 5-min lock; idempotent dispatch.
  (3) `.stores/policies.yaml` schema; predicate-DSL parser;
      substrate verifies auto-writes against rules; records
      policy_ref on audit trail; ntfy when policy DIDN'T apply.
  (4) `open → resolved` transition added (verb: close-as-addressed
      with --resolution) closing L017's gap.
  (5) Field-actor-vs-transition-actor matrix reconciled: when a
      transition is ai_autonomous, contract sub-fields written
      during that transition are also ai_autonomous-friendly
      (tier-A relaxation extends to fields).
  (6) Tests, docs, the new ./dev ship verb integrated via the
      daemon's first subscriber.
  (7) Existing observations L007/L008 backfill-resolved using the
      new close-as-addressed verb.

Scope_in: schema, daemon, policy DSL, ntfy hooks, tests, docs,
          the close-as-addressed verb. (LOTS — this is the
          foundational task.)

Scope_out:
  - Specific deploy ceremonies (./dev ship details — project-side)
  - Web UI / TUI for policy editing (yaml-edit is sufficient)
  - L009/L010/L011 stability cluster (separate task)
  - L012 inspector / presidential brief (separate task)

Tier_hint: T3 (3-5 phases, multi-component)
```

## Follow-ups

**For the next agent (in priority order):**

1. **File L022 autonomously** using the body above (or refined). Filing friction is autonomous; no U-moment needed.

2. **Surface the unified-task promotion** — show the user the L022 body + the "Autonomous Flow Foundation" contract sketch. Get user `go` for the U2 promotion. Then `tasks add --invoker ai_with_human --linked-observations L017,L018,L022 ...`.

3. **Drive the new task** via `tasks drive T### --claude-code`. Substrate's runner spawns planner / plan-reviewer / executor / code-reviewer / wrap. The recursion: this very task fixes the substrate's autonomous-flow gap, using the substrate's existing autonomous-flow capability.

4. **Pre-existing residuals to be aware of:**
   - L004 still in `investigating` with drafted contract (flow-principle docs); now unblockable via token.
   - Auth UX cluster (L013/L014/L015 + L016 informational) is one ~50 LOC follow-up patch task.
   - Substrate stability cluster (L009/L010/L011) is the build/release/audit triad.
   - L012 inspector (T3, three sub-tiers) is the observability uplift.
   - L019 DockerRunner (T3) is the sandboxing pattern.
   - L002, L003, L005, L006 are pre-existing open obs from earlier in the session.

**Operational state for the next agent:**

- **Branch:** `main` (renamed from master). Remote: `git@github.com:blakesims/stores.git` (private).
- **Stable binary:** `~/.cargo/bin/stores` carries L016 hotfix; was `cargo install`-ed twice this session.
- **Token state:** `~/.config/stores/identity.age` (passphrase: `accept`), `~/.config/stores/approve.token.age` (encrypted), `~/.config/stores/approve.token.hash` (sha256). The `~/.config/sops/age/keys.txt` symlink workaround for L015 still in place.
- **Worktrees:** all cleaned up (T002 + T003 worktrees removed; T001 had none).
- **Local feat branches:** `feat/T002-...`, `feat/T003-...`, `feat/T010-...`, `feat/T011-...`, `feat/T012-...`, `feat/T013-...` all merged into main but not deleted (archeology preserved; user can clean when ready).

**Critical context for fresh-agent orientation:**

- Read `/CLAUDE.md` first (the dogfood rule, U-moments, --invoker discipline).
- Read `/docs/worklog/2026-05-03/01-dogfood-recursion-first-session.md` second (today's session in 3 chapters).
- Read this note third.
- Then `stores observations show L018 --invoker ai_autonomous` to see the watcher brief.
- L022 will be the next observation ID. *(Originally this note proposed the policy observation as L020, but two mid-handover audit filings claimed L020 [substrate stale state-folders] and L021 [wrap_log not rendered into Completion section] before the policy obs landed. L021 is now the highest filed; the policy obs becomes L022 when filed.)*

**The recursion the new agent steps into:**

This task's drive will be the LARGEST autonomous-flow demonstration to date. It builds the autonomous-flow capability USING the existing autonomous-flow capability. Every friction the drive encounters becomes an observation that — once L022 ships — flows automatically. The first run won't fully self-sustain (L022 isn't shipped at start), but each successive task gets closer. The propulsion the user wants will manifest after this task lands.
