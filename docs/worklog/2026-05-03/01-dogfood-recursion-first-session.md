# Dogfood Recursion First Session

**Date:** 2026-05-03
**Type:** note

## Summary

The first session where the `stores` substrate was actually used to build itself. Started as an orientation handover, ended with three substrate-driven tasks accepted (T001 token mechanism, T002 per-role models, T003 dev worktree script), 13 friction observations filed (L007–L019), one inline hotfix shipped during deploy (L016 — token round-trip bug), and the repo published privately to `github.com/blakesims/stores` on a renamed `main` branch. The dogfood produced its quality-ceiling-raising signal exactly as the doctrine predicted: real use surfaced what design review didn't.

Three chapters: doctrine clarification on Day-1 friction → building infrastructure for the substrate to drive itself → first real end-to-end deploy under the new ceremony.

## Chapter 1 — Doctrine: agency, magnetism, and the substrate as engine

The session opened mid-decision. The previous shepherd had a working diff for L001's `--display-id` flag on `tasks add` (the canonical first observation — substrate-IDs and filesystem-IDs diverging on the very first attempt) and was asking whether to commit it (option 1) or rollback and re-do via the symmetric `tasks drive` path (option 2'). I picked it up, read the orientation, recommended option 1, and asked the user to choose.

The user redirected to the meta-point: **"I shouldn't have to direct this. The analogy is that we are building an engine. I should not have to push oil or fuel through it. I am not a flight controller telling pilots where to fly. I am merely an observer watching an engine run itself."**

That reframing reshaped the rest of the session. The operational translation:

- **Autonomous moments** (commit finished work, file observation, render projection, run `submit-*`) — the AI just does them. No asking.
- **U-moments (U1–U4)** — the AI halts and proposes; the user types `go`. That's the only place the user sits in the chair.
- **Substrate gaps that block propulsion** — the AI notices, files an observation, fixes it (or proposes a fix), continues. Kick-starting *is* autonomous work.

This produced the first two real-use observations almost immediately:

- **L007** — schema's `actor: ai_with_human` on `observations.investigate` over-strict; CLAUDE.md doctrine says only `confirm` is the U-moment. Drafting the contract is autonomous.
- **L008** — `intent_contract.approved_by/at` fields are `actor: human` (literal); chat-`go` qualifies as `ai_with_human` per doctrine; the field gating doesn't honor that.

Both were filed as `ai_autonomous` work (filing friction is autonomous) — the substrate's rejection-on-mismatch was the fail-loud signal we wanted, not an error to recover from.

The token-mediated tier-A path was sketched right there: encrypted token at `~/.config/stores/approve.token.age`, AI presents `--approve-token <T>` on writes that are gated by `actor: human`, the substrate verifies via constant-time hash compare. The user's question — *"can't you just read the secret? it has to be encrypted"* — locked in the encrypted-at-rest property and the ask-first behavioral discipline as separable layers.

Late in this chapter the user articulated the **flow principle** explicitly (the magnetic-pull-through-pipes framing) which became L004 — observation lifecycle currently `investigating`, drafted contract awaits confirmation.

## Chapter 2 — Building the infrastructure for dogfood propulsion

L007 + L008 promoted to **substrate-T001 — approval-token mechanism** (U2 promotion, ai_with_human + drafted contract, user `go`). The decision NOT to ship MCP transport — "no MCP, just the token. let's get this done" — kept the scope tight. The contract grew to include encryption-at-rest after the user pushed back on plaintext storage; final form mandated age-encrypted token + sha256 hash sidecar + constant-time verify + tier-A/tier-B doctrine in CLAUDE.md.

Two parallel companion tasks were promoted to keep T001 unblocked while it ran:

- **T002 — per-role model configuration** (planner=opus, plan_reviewer=opus, executor=sonnet, code_reviewer=opus, wrap=opus). Closes the model-invisibility gap noticed during T001's drive (`drive` output showed exit codes + durations but never the model used). Added `(model=<m>)` to every spawn line.
- **T003 — `./dev` worktree script**. Resolves the bootstrap dependency: `tasks add --workspace-path` plumbing existed (T012) but no project-side wrapper to actually create worktrees. T003's `./dev new` runs `tasks next-id`, creates the worktree, runs `tasks add` with the path. `./dev done` tears down accepted/rejected worktrees.

Both T002 and T003 ran in their own worktrees (created manually since `./dev new` is what T003 builds — chicken/egg footnote captured in the brief). Three drives in parallel with claude-code runner, all status-queryable via `stores tasks status` in <50ms. While they ran, two threads of substrate-stability work surfaced:

- **L009** — `runner-claude-code` should be a default Cargo feature; bare `cargo build` drops it and breaks orchestration mid-session. Hit twice in this session.
- **L010** — no automated release step on `tasks accept`; `cargo install` should bump the stable binary at `~/.cargo/bin/stores` when work ships.
- **L011** — rows don't record the binary version that wrote them; audit gap acute during a self-modifying-substrate dogfood.
- **L012** — no inspector / visual surface for what each agent receives at each stage. The user invoked the n8n analogy. T3-tier with three sub-tiers (CLI text → edit-and-rerun → graph view).

The **review surface design** crystallized: the wrap agent already produces a structured envelope (`executive_summary`, `deviations[]`, `residual_risks[]`, `recommended_sanity_checks[]`) stored on the row's `wrap_log` — but no `tasks review T###` verb renders it for a human's 1-minute decision. The presidential-brief pattern was sketched: structured fields + verdict suggestion + AC-verification table + optional `--guide` spawn (a fresh claude session with task context as primer for "I need more info"). Filed as a future observation; not promoted yet.

By end-of-chapter, all three drives reached `in_review` (T001 5 phases / 1 REVISE in P2; T002 4 phases all PASS first-cycle; T003 4 phases all PASS first-cycle). 511+528+? unit tests green, plus integration. Acceptance pending.

## Chapter 3 — First real deploy: friction surface and the watcher horizon

The user accepted all three tasks and asked the deeper question: *"how do we 'deploy' the task? does stores handle this? is this a blind spot?"* — and yes, it is. Stores handles the state transition `in_review → accepted` and re-renders the projection to `tasks/completed/`. **Branch merges, `cargo install`, linked-observation closure, worktree cleanup, worklog notes — all unowned.** Per the wrapper-boundary doctrine (T011 § What's outside the substrate), that's correctly *not* in stores; it belongs in `./dev ship` or equivalent, which doesn't exist yet.

The user proposed a **CI/CD-watcher agent**: always-running, subscribed to `tasks accept` transitions, runs the deploy ceremony autonomously, triages failures via the filing rubric. After studying the client-project's `task:wrap` skill (`/home/blake/repos/clients/10.06-wt/10.06-main/.claude/skills/task:wrap/SKILL.md` — a battle-tested 9-phase ceremony with explicit Q1/Q2/Q3 filing rubric, gates store, ntfy push for stuck deploys), I drafted the architectural answer:

- **Stores becomes an event bus** (state transitions fire watcher subscriptions)
- **Agents register** in `.stores/agents.yaml` declaratively
- **Daemon mode** (`stores agents run`) polls/subscribes and dispatches
- **A gates store** (parallel to observations) for "Blake-only follow-ups"
- **State extension**: `accepted → deploying → deployed → closed` with `deploy_blocked` for stuck

The full brief was filed as **L018**, drafted live as we ran the deploy by hand. The session became its own data: every step we took, every friction we hit, would be the watcher's primer.

The deploy itself surfaced **a real bug in T001 that the test suite missed**:

- **L013** — `auth init` defaults `--identity` to `~/.config/sops/age/keys.txt`, squatting on SOPS convention. Stores should be stand-alone (user explicitly: *"i don't want stores to be tangled up to my custom built 'sb' system"*).
- **L014** — `auth init` UX gaps: opaque "stream did not contain valid UTF-8" when given a binary-format file (need `age -p -a`), no recipient discovery on encrypted files, 7-line shell ritual to bootstrap.
- **L015** — `auth show` is missing `--identity` flag entirely (asymmetric with init). Worked around with a symlink at the SOPS-shaped path.
- **L016** — **the real defect**: `init` encrypts and hashes 32 raw random bytes; `verify_token(token: &str)` hashes UTF-8 string bytes. **Random bytes ≠ UTF-8.** Token round-trip mathematically broken for almost every generated token. Tests had passed because fixtures used `"test-secret-..."`-shape valid UTF-8.

L016 was the Q1 fix-in-turn case from the client's filing rubric: small (3-line patch), obvious (hex-encode the secret), critical (whole tier-A path non-functional without it). Hotfixed inline as commit `82501d3` on the deploy branch. The token mechanism then worked end-to-end.

Auth UX bootstrap was the friction the user named most directly: *"there is too much friction copy and pasting the commands. cna you just run these? the passphrase can be 'accept' forall i care."* The pragmatic answer was a Python `pty`-module wrapper to drive `age -p` and `age -d` programmatically with a fixed passphrase. The cleaner answer is an `auth init --generate` mode that does the whole bootstrap in one verb (folded with L013/L014/L015 into one ~50 LOC patch task).

Two more observations landed during the deploy:

- **L017** — no clean close-from-open path on observations. L007 + L008 are functionally closed by T001 but stuck `open` because resolving requires walking the full 4-hop lifecycle (investigate → confirm → claim → resolve), and the field-actor-vs-transition-actor matrix has a contradiction (transition wants ai_autonomous post-T001-P4, fields still want ai_with_human + token).
- **L019** — `DockerRunner` impl: standardized agent sandboxing in the substrate. Architectural decision locked in: **stores, not `./dev`** — the Runner trait is the dispatch boundary, T002 just added per-role config there, DockerRunner is symmetric work. Beyond "sandbox": reproducibility, resource caps, network policy, blast-radius bound.

End of session: 57 commits fast-forwarded from `feat/T013-reviewer-envelope-substrate-dogfood` to `master`, master renamed to `main`, push to private `github.com/blakesims/stores` (which already existed from an earlier project setup — pushed to it; deleted stale remote `master` branch since main contained all its history). Default branch updated to `main`. Worktrees cleaned up. Stable binary at `~/.cargo/bin/stores` carries L016 hotfix.

## Follow-ups

The natural next clusters (each a candidate U2 promotion to a substrate task):

| cluster | observations | shape | priority |
|---|---|---|---|
| Auth-UX patch | L013, L014, L015 (+ L016 informational) | one ~50 LOC patch | high |
| Substrate stability | L009, L010, L011 | release/build/audit triad | high |
| Observation lifecycle | L017 | small schema task (close-from-open path + field/transition actor reconciliation) | normal |
| Inspector | L012 | T3 with three sub-tiers; ship Tier-1 CLI first | normal |
| Sandboxing | L019 | T3 (DockerRunner + Dockerfile + runner.yaml additions) | normal |
| CI/CD watcher | L018 | T3, depends on gates store + agents registry + daemon mode | normal |
| Mid-flight | L004 | now unblockable via token-mediated confirm (modulo L017's path) | normal |

Pre-existing open observations carried into next session: **L002** (no `tasks delete` verb), **L003** (list output unscannable), **L005** (list-field input parsing), **L006** (T1/T2 asymmetry — substantially closed by tier-A but worth re-reading).

Local feat branches (`feat/T002-…`, `feat/T003-…`, `feat/T010-…`, `feat/T011-…`, `feat/T012-…`, `feat/T013-…`) all merged into `main` but not yet deleted.

## Stats

- **Substrate tasks accepted:** 3 (T001 / T002 / T003 — substrate-T001 is the first task done the new way; the great divide on IDs holds)
- **Friction observations filed:** 13 (L007 → L019)
- **Substrate hotfix commits:** 1 (L016 — `82501d3` on deploy branch)
- **Cargo installs:** 2 (mid-T001, post-L016 hotfix)
- **Lines of executor code shipped:** ~1900 (T001 ~700 + T002 ~600 + T003 ~500 + L001/L016 patches)
- **Test-result delta:** 511 → 528 unit tests, all green
- **Cycles needed:** 12 of 13 phases shipped first-cycle PASS (one REVISE in T001 P2)
- **Time elapsed:** several hours of conversation; substrate spent ~50 minutes in `executing` state across the three drives running concurrently

