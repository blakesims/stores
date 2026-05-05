# Study: 10.06 `task:open` end-to-end, mapped to the `stores` substrate

**Author:** subagent dispatched 2026-05-05
**Status:** Comprehensive read-only study. Supersedes the narrow scope of `docs/poc-1006-autoscaffold.md` (which is preserved and cross-referenced in §8).
**Headline finding (TL;DR):** Blake's intuition that "stores task workflow is mostly the same workflow as 10.06" is **correct for the orchestration backbone (Phase 0 → Stage 1.5 → Stage 5 phase-loop → Stage 8)** but **wrong about the scope of remaining 10.06-specific work**. The 10.06 substrate beneath `task:open` (worktree creation, Docker stack, postgres, port slots, alembic `_feat_` migrations, Sentry, fly deploy, capability YAML reconcile, ready-to-merge stamp, gate inbox) is roughly **~70% of the line count** of the workflow's runtime weight. It is project-internal — not workflow-internal — and the right move is to leave it in `./dev` and call it from substrate-declared subscribers, NOT to port it into builtins. The orchestration backbone, by contrast, is highly substrate-shaped already: ratify → promote → scaffold → drive → wrap maps to the existing `auto-promote / auto-scaffold / auto-drive / accept-merge` chain with one missing builtin (a "deploy ceremony" check primitive) and a small schema extension on `tasks` (capability, sub_item, ledger[], stamp) and `observations` (already aligned).

---

## §1. Glossary & cast of characters

Names + one-liners. Anchored to file paths so a reader can drill in.

- **`/task:open`** — 10.06 client-side Claude Code skill (`/home/blake/repos/clients/10.06-wt/10.06-main/.claude/skills/task:open/SKILL.md`, 842 lines). Universal "do work on an observation/task" entry; runs Phase 0 (contract ratification) through Stage 8 (completion). Three invocation modes: observation-driven, autonomous, ad-hoc.
- **`/task:wrap`** — sibling skill (`task:wrap/SKILL.md`, 588 lines). Continues from "code shipped on feat branch" through merge → deploy → close-out; stamp-validates the gate result `task:open` Stage 7 emitted.
- **`./dev`** — 12,463-line Bash CLI at the 10.06 repo root. The runtime substrate `task:open` invokes for every concrete side-effect (worktree, db, migration, gate, deploy, observation).
- **Intent contract** — schemafied YAML/JSON brief attached to an observation. Required string fields: `objective, type, in_scope, out_of_scope, acceptance, tier_hint, contract_state`. State machine: `null → draft → ready`. Defined in `research/refs/intent-contract.md`. Maps 1:1 to the bundled `observations` store's contract fields in stores.
- **Observation** — a typed row in `issues/ledger.json` (10.06) — captured friction. ID format `LNNN`. Lifecycle: `open → investigating → confirmed → resolved | wont_fix`. Maps to bundled `observations` store with the same lifecycle in stores.
- **Task** — work artifact `tasks/active/TXXX-<slug>/main.md` (10.06). Has YAML frontmatter (`task_id`, `capability`, `sub_item`, `ledger`, `infra`, `qa_items`, `linked_ledger_ids`) plus prose body with `## Meta`, `## Plan`, `## Phase N` sections. Maps to bundled `tasks` store row + the `render` projection.
- **Worktree** — `git worktree`-mounted feature branch at `~/repos/clients/10.06-wt/10.06-feat-<slug>/`. Created by `./dev new`, destroyed by `./dev destroy` / `./dev cleanup`. Each has its own postgres DB (`unified_feat_<slug>`), its own backend/frontend/fileserver containers (`1006-<slug>-{backend,frontend,fileserver}`) and its own port slot.
- **Capability (C01–C20)** — Phase 1 launch unit declared in `app/backend/config/phase-1-capabilities.yaml` (the canonical YAML) and surfaced in `docs/phase-1-launch.md` (the readable narrative). Every Phase-1 task references one. The sub-list is `C01..C20` plus `C03b`.
- **`sub_item`** — a labelled row inside a capability's `sub_items` list in the YAML. Tasks declare `capability: CXX` + `sub_item: "<exact label>"`. The rollup script flips `status` (NOT_STARTED / IN_PROGRESS / QA / PARTIAL / DONE) on this row when a task completes.
- **Gate** — Blake-only execution baton in `issues/gate.json`. ID `GNNN`. Two types: `script` (a command Blake must run with sudo / fly secrets / prod) and `decision` (a question only Blake can answer). Seven categories (1=Prod DB write, 2=Critical biz decision, 3=External info, 4=Inaccessible action, 5=Fly secret/non-deploy ops, 6=Coordination wait, 7=Live-env policy). Filed via `./dev gate add`. Run via `./dev gate run` (refuses if `$CLAUDECODE` set; `dev:8369-8375`).
- **Ledger** — `issues/ledger.json` — the observation store. Distinct from the `gate` ledger (`issues/gate.json`). 10.06 historical naming: "ledger" mostly means "observation ledger." `cmd_observation` dispatches to `cmd_ledger_*` handlers (`dev:3806`).
- **Frontmatter** — YAML block at the top of `tasks/active/TXXX-*/main.md`. Written ONLY via `./dev task frontmatter <TID>` (`dev:9919`) — never hand-rolled. The CLI runs it through Pydantic `TaskFrontmatter` validation before atomic write.
- **Fleet** — not used in this codebase. (Avoiding accidental jargon.)
- **Ready-to-merge stamp** — `<bare>/worktrees/<feat-name>/ready-to-merge` — JSON file `{feat_sha, main_sha, gate_sha, captured_at}` written by `task:open` Stage 7d after the gate passes on the feat branch. Validated by `task:wrap` Phase 4. Stale if `feat_sha` or `main_sha` advanced since capture.
- **CodeRabbit gate** — `cr review --type all --base main --plain`. ONE-shot per task in `task:open` Stage 6 (`task:open/SKILL.md:633`). Bug-confirmed: `--type committed` and `--agent` modes drop findings.
- **Phase 1 capability YAML** — `app/backend/config/phase-1-capabilities.yaml` (the canonical machine-readable). The sister doc `docs/phase-1-launch.md` is rendered from it by `launch_readiness_sync_md.py`.
- **`launch_readiness_*.py`** — three Python scripts at `scripts/launch_readiness_{rollup,validate,sync_md}.py` (1,138 LOC total). Rollup writes status into the YAML; validate runs the YAML through a Pydantic schema and cross-refs ledger + qa-items; sync_md re-renders the markdown projection.
- **`ntfy`** — push-notification service. 10.06 uses `https://ntfy.sh/blake-carli-monitor-9c90d57c6a2d` (`task:wrap/SKILL.md:106`) for blocking-failure alerts.
- **Blake-only categories** — see Gate above. Categories 1, 5, 7 are operational; 2 is decision; 3/4/6 are coordination waits.
- **`_feat_` migration prefix** — alembic migration files generated on a feat branch get filename `<rev>__feat__<task>_<desc>.py` so `./dev pr prep` can delete them pre-merge (`dev:1283-1289`, `dev:1382-1393`). The clean-named migration is regenerated post-merge in `task:wrap` Phase 3.5.
- **`unified_main`** / **`unified_feat_<slug>`** — postgres database names. Main worktree's DB is `unified_main`; each feat worktree's is `unified_feat_<slug_with_dashes_to_underscores>`.
- **Sentry** — `./dev logs issues / detail / resolve` (`dev:3215-3503`) wraps the Sentry HTTP API.
- **Auto-promote / auto-scaffold / auto-drive / accept-merge** — stores builtins (`src/flow/builtins/*.rs`). Already wire ratify → promote → scaffold → drive → merge.

---

## §2. The full `task:open` workflow, step-by-step

I trace every phase named in `task:open/SKILL.md` plus the wrap continuation. For each step: trigger, inputs, side-effects, outputs, actor, failure modes, and the linkage chain.

### Phase 0a — Resolve the target observation

(Modes: observation-driven, autonomous; skipped in ad-hoc.)

- **Trigger:** `/task:open LNNN` (explicit) or `/task:open` (autonomous → query the ready pool).
- **Inputs:** `./dev observation show LNNN` (reads `issues/ledger.json`); for autonomous mode, `./dev observation list --bucket 1 --scheduled-today` then fallback `--bucket 1 --sort rank` (SKILL.md:135).
- **Side-effects:** `./dev observation lock LNNN "in-session via /task:open"` writes `locked_by`, `locked_at`, `lock_reason` into the ledger row. The lock is the **cross-session anti-collision primitive** (SKILL.md:121, prevents L275-style double-pickup). Autonomous mode acquires the lock only AFTER candidate selection, before printing to Blake; declines release the lock.
- **Outputs:** ledger row mutated with lock fields; in-memory `entry.intent_contract` carried forward.
- **Actor:** orchestrator (Claude main thread).
- **Failure modes:**
  - Entry not found → abort.
  - Status `resolved` / `wont_fix` → abort (must `--status open` first).
  - `BLOCKED:` from `lock` (another holder, fresh <2h) → abort, no `--force`. Three retries on autonomous lock-race, then abort.
  - User declines candidate (autonomous) → unlock + clean exit.
- **Linkage:** the lock row encodes `locked_by ≈ "session via /task:open" or branch_slug`. The branch link is set later (Stage 1.5 via `./dev observation update LNNN --task-id TXXX`).

### Phase 0a.5 — Specialised-skill check

- **Trigger:** Always runs after 0a (cheap routing pass).
- **Inputs:** entry's `summary`, `field_name`, `capability`, `intent_contract.touches`. Routing table (SKILL.md:159-167) maps signals → `/converge`, `/qa:walk`, `/logs:get`, `/doc:answer`, `/intake`.
- **Side-effects:** none unless Blake re-routes (then exit cleanly; specialised skill takes ownership end-to-end).
- **Actor:** orchestrator.
- **Failure modes:** Ambiguous / multi-match → skip silently. The L264 worked example (truth-engine signal → `/converge`) is in SKILL.md:179-189.

### Phase 0b — Route by (status × contract) state

Five paths (SKILL.md:199-271):

1. **Path 1 (no contract):** premise check (NOT_A_BUG → `wont_fix`; ALREADY_RESOLVED → `resolved`); optional ≤10-tool investigation (deeper goes to investigator subagent); draft contract; print as YAML; loop `Approve | amend (max 3) | abort`.
2. **Path 2 (draft):** print existing draft + drafted_by/at; same approve/amend/abort loop.
3. **Path 3 (ready):** print one-line; proceed straight to Stage 0.
4. **Path 4 (`needs_info`):** print `Gaps:` block; one round of Q&A with Blake; update `investigation_note`; flip status open via `./dev observation update LNNN --status open --note "Gaps resolved: …"`.
5. **Path 5 (`in_progress`):** observation already has `task_id=TXXX`; abort and resume that task.

- **Side-effects:** ledger writes via `./dev observation update`. NEW: each `--status confirmed` is rejected if no draft contract (SKILL.md:132 + intent-contract.md:132).
- **Actor:** orchestrator + human (amend/approve loop).
- **Linkage:** the contract is held in working memory (Phase 0d carry-forward, SKILL.md:328-330).

### Phase 0c — Persist ratified contract

- **Trigger:** Blake just approved a draft (Paths 1, 2 only).
- **Side-effect:** `./dev observation contract LNNN --approve` flips `intent_contract.contract_state: draft → ready` and stamps `approved_by, approved_at, approval_invoker` (`dev:6215`+).
- **Actor:** orchestrator records Blake's verbal assent (`approval_invoker=agent`); auditable per CLAUDE.md §39.

### Phase 0d — Tier routing + contract carry-forward

Algorithm (SKILL.md:282-286, intent-contract.md:79-90):

```
final_tier = MAX(contract.tier_hint, touches_floor)
touches_floor:
  any of {migration, secret, capability, cross_system} → T3
  webhook → T2
  empty → T1
```

- **T1 — inline fix on main:** print fix + edit + commit + `./dev observation update LNNN --status resolved --note "<resolution>"` + `./dev observation unlock LNNN`. **No worktree, no wrap, no skill chain.**
- **T2 — in-session mini-loop on main:** spawn `task-workflow:executor` → `task-workflow:code-reviewer`; max 3 REVISE cycles → BLOCKED; on PASS: commit, resolve observation, unlock. **Still no worktree.**
- **T3 — full task on feat branch:** keep the lock; advance to Stage 0.

Linkage at this point: `LNNN` has the ratified contract; if T3, the lock survives into worktree creation where `./dev new` re-locks with `--force`.

### Stage 0 / 0a / 0b — Size check (triage preamble)

- **Trigger:** post-Phase 0 (or first stage in ad-hoc mode).
- **Inputs:** the ratified `intent_contract.touches`; `research/refs/triage-rubric.md` rubric.
- **Side-effects:** prints the four-fact pre-check block (Files / LOC / Subsystem / Forced step-ups), then `Triage: T<N> — <plan>`.
- **Actor:** orchestrator + human (final green-light pause).
- **Failure modes:** if the work fits T1/T2, the orchestrator MUST propose the lighter path before continuing. T3-by-default is explicitly named the **expensive** default (triage-rubric.md:28-30). Override is allowed but explicit.

### Stage 0.4 — TID pick

- **Trigger:** T3 confirmed.
- **Inputs:** four sources (SKILL.md:391-395):
  - `tasks/active/T*` directories
  - `tasks/planning/T*` (deprecated primitive but still scanned for collisions)
  - `tasks/completed/T*`
  - `git log --all --oneline --grep -E 'T\d{3}'` across every commit
- **Output:** `TID = max(...) + 1`. Print TID + max-seen so user can override.
- **Failure modes:** **Mid-task collision is mechanical, not a halt** (SKILL.md:399-400). `task:wrap` Phase 8.6 step 17 re-numbers branch + worktree dir + task dir + `task_id` frontmatter to the next free TID. No observation, no gate, no ntfy.

### Stage 0.5 — Context gate

Confirm intended outcome / constraints / scope / unknowns are sufficient. Ask minimal clarifying questions. Pure orchestrator gate; no side-effects.

### Stage 1 — Intent contract

Two paths.

- **1a (hydrate from ratified contract):** map fields directly (table at SKILL.md:421-433):
  - `objective` → Executive intent + DONE_WHEN seed
  - `acceptance` → DONE_WHEN
  - `inputs` → planner prompt context
  - `affects_capability` → pre-fills Stage 1.5
  - `known_solution` non-null → seed for executor Phase 1; null → planner does discovery first
  - `type=investigation` → planner's first phase MUST be RCA before any code phase
  - Confirm DONE_WHEN phrasing only if `acceptance` doesn't reduce cleanly.
- **1b (build from scratch — ad-hoc only):** interactive Q&A to derive Executive intent / DONE_WHEN / scope / approach / risks / open decisions.

DONE_WHEN is **propagated verbatim** in every subsequent agent prompt (SKILL.md:832). This is the single thread of intent.

### Stage 1.5 — Declare Phase 1 capability (seven steps)

Steps (SKILL.md:456-516):

1. **Kill-switch:** if `app/backend/config/phase-1-capabilities.yaml` does not exist, log "[1.5] YAML not found, skipping" to stderr and skip to Stage 2. (Lets the workflow run on repos that don't have the YAML — partial portability.)
2. **Read context:** the YAML + `docs/phase-1-launch.md`.
3. **Infer + confirm:** if contract has `affects_capability`, prefill; else infer + offer one-shot confirmation.
4. **On confirm:** `./dev task frontmatter <TID> --capability CXX --sub-item "<label>"` (`dev:9919`).
5. **On `infra: <description>`:** `./dev task frontmatter <TID> --infra "<description>"` (escape hatch for refactors / dev tooling).
6. **On decline:** append "Should we propose adding a new Phase 1 capability for this work? Ask Blake." to Open Questions; proceed without frontmatter (wrap kill-switch handles missing).
7. **Link observations to task (CRITICAL — early link, NOT at wrap):**
   ```
   ./dev observation update <LNNN> --task-id <TXXX>
   ```
   Run for the driving `LNNN` AND any `L\d{3,}` in the contract's `inputs` or frontmatter `ledger`. Idempotent. **This is the moment the observation→task linkage chain is established.** It's read by `/day:open` and `/observation:triage` to detect items already being worked on.

- **Side-effects:** writes frontmatter (atomic via `tempfile + os.replace` in `cmd_task_frontmatter`); writes `task_id` onto each linked observation row.
- **Actor:** orchestrator + human (step 3 confirmation).
- **Linkage delivered here:** observation.task_id = TXXX; task.frontmatter.ledger = [LNNN, …]; task.frontmatter.capability = CXX; task.frontmatter.sub_item = label. Note: the `ledger:` frontmatter list is *separate* from `linked_ledger_ids:` written by `./dev new` step 9. Both exist, both are soft FKs, neither is enforced — see §5.

### Stage 2 — Planning (subagent)

- **Trigger:** post-Stage 1.5.
- **Inputs:** Intent contract + DONE_WHEN + `intent_contract.{inputs, touches, known_solution, type}` (SKILL.md:528-533).
- **Side-effects:** spawns `Task(subagent_type="task-workflow:planner", ...)`; planner writes plan to `tasks/active/TXXX-*/main.md` `## Plan` section.
- **Actor:** planner subagent (NOT orchestrator).
- **Failure modes:** plan drifts outside `out_of_scope` → Stage 3 returns FAIL with cited violation.

### Stage 3 — Plan review (subagent)

- **Trigger:** planner returned.
- **Inputs:** the plan + Intent contract + DONE_WHEN.
- **Side-effects:** spawns `Task(subagent_type="task-workflow:plan-reviewer", ...)`; verdict written to `main.md`.

### Stage 4 — Plan gate (orchestrator)

- **Trigger:** plan-reviewer returned.
- **Outcomes:** misaligned with DONE_WHEN → return to planner. High-impact decisions → ask Blake. Aligned → move task `tasks/planning/TXXX/ → tasks/active/TXXX/` and continue.
- **Side-effect:** `git mv` of the task directory; this is *also* where `./dev new` is typically invoked (creating the worktree, locking the linked ledger items, generating the docker stack, seeding the sandbox). That step is the **heaviest single block of side-effects in the entire workflow** — see §3.

### Stage 5 — Phase loop (5a / 5b / 5c)

Per phase in the plan:

- **5a Execute:** spawn `task-workflow:execution` with current phase scope only + DONE_WHEN. Executor writes code on the feat branch.
- **5b Code review:** spawn `task-workflow:code-reviewer` with diff + DONE_WHEN.
- **5c Gate:**
  - PASS → next phase.
  - REVISE (minor) → orchestrator may make ≤30-line safe fixes itself, re-review.
  - REVISE (substantial) → return to executor with revision scope.
  - FAIL after 3 REVISE cycles → BLOCKED.

This mirrors the bundled stores `tasks` workflow (`planning → plan_review → ready → executing → code_review → executing | complete | blocked`) **almost exactly**. The 30-line orchestrator-fix carve-out is one delta vs the substrate's "executor-only writes code" doctrine; see §6.

### Stage 6 — CodeRabbit final review (6a / 6b / 6c)

- **6a Clean diff:** `git fetch origin main` + `git rebase origin/main`; if `./dev pr prep` exists, prefer it (handles `_feat_` migration cleanup + tests). Force-push with `--force-with-lease` (SKILL.md:609-625).
- **6b CodeRabbit local CLI (mandatory, ONCE per task):**
  ```
  cr review --type all --base main --plain
  ```
  Bug-noted flags: `--type committed` shallow; `--agent` drops findings + returns 0 (SKILL.md:638-640).
- **6c Process findings:** one round only — fix actionable, dismiss out-of-scope, commit. **Do NOT re-run CodeRabbit** (loops + quota burn). `task:wrap` enforces this (`task:wrap/SKILL.md:11`, line 562).

### Stage 7 — Rebase + Capability YAML reconcile + Test gate + Stamp (7a/7b/7c/7d)

This is the **canonical gate point**. It moved here from `task:wrap` Phase 4 deliberately, "to the moment context is hottest" (SKILL.md:655-659).

- **7a Rebase onto current main:** fetch + rebase + run integration smoke (`pytest tests/ -x -q --timeout=60` + `npx tsc --noEmit`); force-push. Non-trivial conflict → BLOCKED.
- **7b Capability YAML reconcile:** kill-switches first (no YAML → skip; no `capability` field → skip; `infra:` set → skip). Otherwise:
  ```
  python scripts/launch_readiness_rollup.py --task TXXX --sub-item "<label>" --status DONE --capability CXX
  python scripts/launch_readiness_validate.py
  python scripts/launch_readiness_sync_md.py
  git add app/backend/config/phase-1-capabilities.yaml docs/phase-1-launch.md
  git commit -m "docs: TXXX reconcile capability status — CXX/<sub_item_label>"
  git push
  ```
  Non-zero from any of the three Python scripts → BLOCKED. (See §3 for what each script does.)
- **7c Test gate:** `./dev test gate` — the 11-step gate (containers / alembic / dockerfile / vitest / e2e / pytest / pyhealth / health / sandbox-seed / orphan-seed / bundle-scan; `dev:2441`). Per-worktree lock at `/tmp/carli-gate-${wt}.lock`.
- **7d Emit ready-to-merge stamp:** `<bare>/worktrees/<feat-name>/ready-to-merge` JSON — `feat_sha`, `main_sha`, `gate_sha = sha256(feat_sha+main_sha+captured_at)`, `captured_at`. Stays in `.git/`, never committed (auto-invalidated when HEAD or origin/main moves).

### Stage 8 — Completion

Print exec summary / deeper dive / technical considerations / "To understand" (verifiable actions Blake can run). Hand control back to user.

### `task:wrap` continuation (Phases 1–9)

Per `task:wrap/SKILL.md`:

- **Phase 1 — Identify:** read `tasks/active/TXXX/main.md`.
- **Phase 2 — Commit docs (current branch):** STOP-category (`.env*`, `app/backend/alembic/versions/**`, `fly.toml`, `app/backend/**/*.py`, `app/frontend/**/*.tsx`, `app/backend/scripts/**/*.py`) → On Failure protocol; AUTO-category (`research/`, `docs/`, `tasks/`, `issues/`, `*.md`, `.claude/`, `dev`) → silent commit (`task:wrap/SKILL.md:127-150`).
- **Phase 2.5 — Pre-merge gate on feat worktree:** resolve worktree path via `git worktree list --porcelain`; `cd "$feat_wt" && ./dev test gate`. Per-worktree lock = parallel-safe.
- **Phase 3 — Merge to main + push:** `git merge feat/tXXX-slug` (no `--force-with-lease`), `git push origin main` (rebase if origin ahead).
- **Phase 3.5 — Regenerate `_feat_` migration under clean name:** detect via `git log -1 --name-status --diff-filter=D | grep '__feat__'`; if found, `./dev migrate gen "<description>"` on main, manual review, `alembic upgrade head` locally, commit, **push (mandatory — L159 incident)**.
- **Phase 4 — Stamp validation:** read `<bare>/worktrees/<feat-name>/ready-to-merge` JSON; compare `feat_sha` against current `cd $feat_wt && git rev-parse HEAD` and `main_sha` against `git rev-parse origin/main`. VALID → skip Phase 4.5; MISSING/CORRUPT/STALE_FEAT/STALE_MAIN → fall through.
- **Phase 4.5 — Fallback gate on main:** only when stamp invalid.
- **Phase 5 — Deploy preflight:** `docker compose exec -T backend alembic upgrade head` + `curl -sf http://localhost:6005/health`. NOT a gate re-run; alembic + healthcheck only.
- **Phase 6 — Deploy:** `cd 10.06-prod`, `git fetch origin main`, `git merge origin/main`, `export DEPLOY_FROM_WRAP=1`, `./dev deploy prod --from-wrap` (`dev:3161`+; T289 extracted body to `dev-lib/deploy.sh`). Pre-checks (8 preflight steps, dev:3169-3174): `lock_acquire`, `branch_and_force_check`, `worktree_check`, `main_merged`, `code_divergence`, `main_clean`, `test_gate_wait`, `truth_engine`. L159 verification step confirms Phase 3.5 migration is in the prod tree before invoking fly deploy.
- **Phase 7 — Worktree cleanup:** `./dev cleanup --dry-run` then `./dev cleanup`. Walks `~/repos/clients/10.06-wt/${PROJECT_PREFIX}-feat-*`, calls `cmd_destroy <slug> --yes` for any branch `merged --to main` (HOTFIX excluded). Drops orphan `unified_feat_*` databases without a matching worktree (`dev:10395-10461`). The `cmd_destroy` cleanup is symmetric to `cmd_new` and includes: stop containers, force-remove, drop `docker-compose.override.yml` + `nginx-local.conf`, terminate postgres connections + `DROP DATABASE`, `git worktree remove`, `git branch -D`, `remove_port_slot`, **resolve every locked ledger item with templated note `closed via ./dev destroy <slug> on <date>; branch_merged=<bool>; merge_sha=<sha>`** (dev:817-883).
- **Phase 8 — Post-deploy docs:** `./research/new-note.sh <task-slug>` → daily note; update daily summary (`wXX-dY-00-summary.md`); close source items (`./dev observation update LNNN --status resolved --resolution "TXXX: <summary>" --task-id TXXX`); also search ledger for unresolved entries with `task_id == TXXX` and resolve them.
- **Phase 8.5 — Blake-only follow-ups → gate entries + tomorrow checklist:** `./dev gate add` for every Blake-only step surfaced; `./research/new-note.sh --tomorrow` for verification checklist with `GNNN` references.
- **Phase 8.6 — Close out task record (mandatory):** flip `## Meta - **Status:**` to COMPLETE; `git mv tasks/active/TXXX tasks/completed/TXXX`; update `tasks/global-task-manager.md` (remove from Active, add to Completed). Commit `task(TXXX): COMPLETE — moved active → completed, GTM updated`.
- **Phase 9 — Report:** print stamp result, capability YAML reconcile result, close-out lines, gate IDs filed, observation IDs resolved.

---

## §3. The runtime substrate beneath `task:open`

Most of `task:open`'s real weight is in `./dev`, not in the skill. Each subsystem documented at file:line.

### 3.1 Worktree creation (`./dev new`, `dev:333-685`)

Steps (in order):

1. Validate `--links` (mandatory; either an `LNNN[,LNNN…]` list or `none`). Slug validation.
2. **Port slot allocation** — `get_port_slot "$slug"` (around `dev:458-465`). Backend / frontend / fileserver ports derived from the slot.
3. **`git worktree add` — branch `feat/$slug` from `main`** (`dev:469`).
4. **Wire `core.hooksPath` to `app/backend/tools/git-hooks`** (`dev:474` — installs the pre-commit hook per CLAUDE.md rule 38).
5. **Generate `docker-compose.override.yml`** from `docker-compose.override.yml.template` via `envsubst` with `SLUG, SLUG_UNDERSCORE, BACKEND_PORT, FRONTEND_PORT, FILESERVER_PORT` substituted (`dev:486-497`). The override pins each container name to `1006-<slug>-{backend,frontend,fileserver}` and (per L076 fix) sets `aliases: [1006-<slug>-backend]` on the main network.
6. **Generate `nginx-local.conf`** (no substitution needed — direct copy from template, `dev:501`).
7. **Generate `.env`** from `.env.feat.template` via `sed` substitution of `${DB_PORT:-…}`, `${FRONTEND_PORT:-…}`, `${BRANCH_SLUG}` (`dev:505-513`). The feat-internal postgres auto-creates `unified_feat_<slug>` via `POSTGRES_DB` env var when `./dev up` first runs.
8. **Register ports with `ports` CLI** (`ports add`, `dev:516-524`).
9. **Set up Claude Code safety hooks** (`dev:527-538`): create `~/.claude/projects/-home-blake-repos-clients-10-06-wt-10-06-feat-<slug>/`, symlink hook scripts from the main project's hooks dir, copy `settings.json`.
10. **Auto-lock linked observation IDs** (`dev:540-577`) — `cmd_ledger_lock LXXX "auto-locked by ./dev new $slug" --force` for each linked ID. Build YAML list `[LNNN, LNNN]` for frontmatter.
11. **Inject `linked_ledger_ids: [LXXX, …]` into task `main.md` frontmatter** via inline Python (atomic tempfile + `os.replace`, `dev:599-628`). Note: this is a SECOND linkage primitive — distinct from frontmatter `ledger:` written later by `./dev task frontmatter`.
12. **Apply profile** — default = "sandbox-local" (multi-minute step): `cd "$worktree_dir" && ./dev sandbox local` runs `./dev up --build` (containers come up), alembic migrations, `stripe_configure.py` (sandbox products), `sandbox_seed.py` (scrubbed test data), `doctor` (health checks). `--profile qa-walk` instead dumps Neon sandbox branch over feat DB. `--no-seed` skips entirely (rare, opt-in).
13. **Run `scripts/preflight.py`** in the new worktree to surface any missing config (advisory only, non-fatal).

**Time budget:** the sandbox seed step alone is several minutes. This is why the POC's auto-scaffold subcommand (`./dev scaffold`) deliberately skips steps 5-12 and only does the cheap git worktree + branch slice (POC §5).

### 3.2 `cmd_destroy` cleanup symmetry (`dev:691-888`)

Reverse of `cmd_new`:

1. Capture branch state (merged-to-main? merge SHA?) before deletion.
2. `docker compose down` + `docker rm -f 1006-<slug>-{backend,frontend,fileserver}`.
3. Remove `docker-compose.override.yml` + `nginx-local.conf`.
4. **Drop database** (terminate connections + `DROP DATABASE "$db_name"` after PROTECTED_DBS check, `dev:766-782`).
5. **Pre-clean docker-owned `node_modules`** via one-shot alpine container as UID 0 (L279 fix, `dev:792-797`) — frontend container builds node_modules as container UID, host rm trips Permission denied.
6. `git worktree remove --force` (with rm -rf fallback + `git worktree prune`).
7. `git branch -D feat/$slug` (local only).
8. `remove_port_slot "$slug"`.
9. **Resolve all locked ledger IDs** with templated note `closed via ./dev destroy <slug> on <date>; branch_merged=<bool>; merge_sha=<sha>` (`dev:820-883`). Already-terminal entries are unlocked-only. Inline Python writes via tempfile.

### 3.3 Migrations (`dev:1245-1338`)

- **`./dev migrate gen <message>`**: `alembic revision --autogenerate -m "<message>"`. On feat branches, auto-prepends `_feat_ ` prefix to the message; alembic uses the prefix in the filename (`<rev>__feat__<task>_<desc>.py`, `dev:1283-1289`).
- **`./dev migrate run`**: `alembic upgrade head`.
- **`./dev migrate check`**: `alembic heads` — fails if multiple heads.

The feat-prefix is the gating mechanic. `./dev pr prep` finds `*__feat__*.py` files (filename match — `dev:1393`), runs the **phantom-revision preflight** (`dev:1404-1442`): for each `_feat_` revision, queries every reachable DB (`unified_main`, `unified_feat_<slug>`) for `SELECT version_num FROM alembic_version` and refuses to delete if any DB is still stamped at that revision (the L057/L055 incident — without this guard, the next `alembic upgrade head` errors with "Can't locate revision"). `pr prep` then deletes the `_feat_` files, leaving a clean diff. `task:wrap` Phase 3.5 regenerates the migration under a clean name on `main` after merge so the schema change rides the same prod deploy as the code (the L159 5,322-Sentry-event incident — `task:wrap/SKILL.md:248`).

### 3.4 Test gate (`./dev test gate`, `dev:2335`+)

11 steps in order:

| # | Step | What |
|---|---|---|
| 0 | up --build | prod-shape build (NOT --dev) — catches T265 cross-project tsc errors |
| 1 | docker | containers running (backend + frontend) |
| 2 | alembic | `alembic upgrade head` clean |
| 3 | dockerfile | frontend Dockerfile build = `tsc -b && vite build` (catches `tsc -b` failures that pass `tsc --noEmit`) |
| 4 | vitest | component tests |
| 5 | e2e | playwright |
| 6 | pytest | backend tests |
| 7 | pyhealth | code-health pre-commit hook |
| 8 | health | `/health` endpoint |
| 9 | sandbox-seed | seed assertion suite |
| 10 | orphan-seed | inventory of orphaned seed rows |
| 11 | bundle-scan | frontend bundle scan (L158 regression guard) |

Pre-gate smoke: `[GATE-SMOKE] cmd_test_smoke` (file/config/binary assertions, <5s) + `[GATE-SELF-TEST] cmd_test_gate_of_the_gate` (verifies the gate parser surfaces failures). Lock at `/tmp/carli-gate-${wt}.lock` is per-worktree (parallel-safe across feats; serialized within one). Multi-worktree DNS hijack preflight (`dev:2412-2430`) detects containers other than `carli-backend` claiming the bare `backend` alias on `1006-main_default` (the L076 hijack).

Wall-clock: ~6 minutes per run (`task:wrap/SKILL.md:188`).

### 3.5 CodeRabbit local CLI

Single invocation per task in Stage 6: `cr review --type all --base main --plain`. The `--agent` mode has a confirmed bug that returns 0 with empty findings (`task:open/SKILL.md:638-640`). One-shot only — no second pass per task (line 653). `task:wrap` rule line 562: "CodeRabbit does NOT run here."

### 3.6 Capability YAML reconcile (`scripts/launch_readiness_*.py`, 1,138 LOC total)

- **`launch_readiness_rollup.py`** (463 LOC, head read): updates `app/backend/config/phase-1-capabilities.yaml` in place. Inputs: `--task TXXX --sub-item "<label>" --status DONE --capability CXX [--dry-run]`. Uses `ruamel.yaml` round-trip (preserves comments, key order, quote style, folded scalars). Atomic write: `tempfile.mkstemp(dir=same)+ os.replace`. Capability-level rollup rules: `all DONE → DONE`; `any PARTIAL → PARTIAL`; `mix DONE+QA → QA`; `any IN_PROGRESS or NOT_STARTED-with-DONE → PARTIAL`; `all NOT_STARTED → NOT_STARTED`. Percentage `DONE=100, QA=75, PARTIAL=50, IN_PROGRESS=25, NOT_STARTED=0`, averaged. **Revision history overwrite-with-date-preservation**: same `(task_id, normalized sub_item_label)` overwrites status_to + note, preserves first `date`. Importable Python API (`apply_rollup` returning `RollupError` with `exit_code`).
- **`launch_readiness_validate.py`** (144 LOC): runs YAML through `app.schemas.launch.PhaseOneCapabilities` Pydantic model + cross-refs `qa_items` against `app/backend/config/qa-items.yaml`, every `LNNN` against `issues/ledger.json`, every `task_id` shape `T\d+`, per-capability `open_questions` against root `open_questions`. Exit 0/non-zero.
- **`launch_readiness_sync_md.py`** (531 LOC): re-renders `docs/phase-1-launch.md` status tables from the YAML; writes a Revision History line "status tables synced from … by launch_readiness_sync_md.py".

These three scripts run as a chain in `task:open` Stage 7b. Failure of any → BLOCKED (no stamp emitted). The order matters: rollup writes, validate confirms, sync_md re-renders. Recovery (`task:wrap/SKILL.md:577-588`): `git restore` the YAML + md, fix the underlying issue (mismatched label is the most common — normalized match handles whitespace+case but not different words), re-run Stage 7b idempotently.

### 3.7 Sentry integration (`dev:3215-3503`)

- `./dev logs issues` — list unresolved Sentry issues (filterable by project / level / sort).
- `./dev logs detail <issue-id>` — fetch latest event with stack trace.
- `./dev logs resolve <issue-id>` — mark resolved via Sentry API.
- These don't fire automatically. Sentry → observation flow is a **manual orchestrator step**: agent reads Sentry, files an observation with `--evidence '{"external_refs":[{"system":"sentry","kind":"issue","id":"<id>"}]}'` (observation:log/SKILL.md:74-82). There is no auto-promote-from-Sentry today.

### 3.8 Deploy chain (`dev:3122-3194`, body extracted to `dev-lib/deploy.sh`)

`./dev deploy prod --from-wrap` (DEPLOY_FROM_WRAP=1 set by `task:wrap` Phase 6) runs 8 preflight steps (`dev:3168-3174`) — `lock_acquire`, `branch_and_force_check`, `worktree_check` (must be run from `10.06-prod`), `main_merged`, `code_divergence`, `main_clean`, `test_gate_wait`, `truth_engine`. Then mutating steps: `prod.deploy_backend`, post-deploy `invariant_precheck`, `secrets_drift_check`. Failure of any pre-gate → halt before any mutation. `--check` runs preflight only (read-only parity mode).

`./dev deploy merge <feat-branch>` is the alternative — does sync-feat-with-main, Phase 4 YAML reconcile, gate-on-feat, fast-forward-main-via-`git update-ref`-without-touching-10.06-main-working-tree, deploy prod. Used when 10.06-main has ambient dirty state.

### 3.9 Observation lifecycle (`dev:3806-7295` for `cmd_observation` and `cmd_ledger_*`)

- `./dev observation add` — `cmd_ledger_add` (5151+).
- `./dev observation contract LNNN [--show|--draft|--ready|--amend|--approve|--clear]` (`cmd_ledger_contract`, 6215+). `--ready` and `--approve` flip `intent_contract.contract_state: draft → ready`. Records `approval_invoker = agent | $USER` based on `$CLAUDECODE` (cmd_ledger_contract:6258+).
- `./dev observation lock LNNN "<reason>" [--force]` — writes `locked_by`, `locked_at`, `lock_reason` (`cmd_ledger_lock`, 6702). 2-hour aging.
- `./dev observation update LNNN --task-id TXXX` — sets observation→task soft FK.
- `./dev observation reconcile-tasks` — detects mismatches: terminal observation pointing at a missing task folder (orphan ref); non-terminal observation pointing at a task in completed/archived (close-out missed).
- `./dev observation gc-locks --apply --age-hours N` — clear stale locks on non-terminal entries (>24h default).

### 3.10 Gate inbox (`dev:7299`+)

`./dev gate add "<one-liner>" --type <script|decision> --category <1..7> --why-not-autonomous "<reason>" [...] ` (`cmd_gate_add`, 7673). Required: `--category`, `--why-not-autonomous`. `--api-checked` required for category 4. `./dev gate run GNNN` is **HUMAN-ONLY** — explicitly refuses if `$CLAUDECODE` is set (`dev:8369-8375`); requires sudo + the broker socket at `/run/secrets-broker/broker.sock` (`dev:8466-8478`). `./dev gate answer GNNN "<option>"` resolves a decision entry. `./dev gate cancel GNNN --reason-category <wrong-category|api-doable|stale|duplicate|other>` with structured reasons.

### 3.11 The ratification ↔ task ↔ worktree linking chain

This is the deepest answer to Blake's specific concern. Tracing one end-to-end:

```
issues/ledger.json::L091.intent_contract.contract_state = ready
                       ↓ (./dev observation contract L091 --approve)
issues/ledger.json::L091.task_id = T001
                       ↓ (./dev observation update L091 --task-id T001 — Stage 1.5 step 7)
tasks/active/T001-deprecate-alias/main.md.frontmatter.ledger = [L091]
                       ↓ (./dev task frontmatter T001 --ledger L091 — Stage 1.5)
tasks/active/T001-deprecate-alias/main.md.frontmatter.linked_ledger_ids = [L091]
                       ↓ (./dev new t001-deprecate-alias --links L091 — Stage 4 worktree create)
issues/ledger.json::L091.locked_by = "auto-locked by ./dev new t001-deprecate-alias"
                       ↓
git branch feat/t001-deprecate-alias  (--from-main)
git worktree at ~/repos/clients/10.06-wt/10.06-feat-t001-deprecate-alias
                       ↓ (Stage 1.5 step 4 — capability declaration)
tasks/active/T001/main.md.frontmatter.capability = C18
tasks/active/T001/main.md.frontmatter.sub_item = "Cutover runbook"
                       ↓ (Stage 7b)
app/backend/config/phase-1-capabilities.yaml::C18.sub_items[label=="Cutover runbook"].status = DONE
                                            .task_id = T001
                                            .revision_history[].task_id = T001
                       ↓ (commit on feat branch)
git commit -m "docs: T001 reconcile capability status — C18/Cutover runbook"
                       ↓ (Stage 7d)
<bare>/worktrees/10.06-feat-t001-deprecate-alias/ready-to-merge JSON {feat_sha, main_sha, gate_sha, captured_at}
                       ↓ (task:wrap Phase 3 — merge)
main now contains the feat branch's history; commit messages reference T001
                       ↓ (task:wrap Phase 6 — deploy)
fly v<N> backend deploy logs reference the merge SHA
                       ↓ (task:wrap Phase 8)
issues/ledger.json::L091.status = resolved
                                .resolution = "T001: Removed deprecated is_over_one_month alias"
                                .task_id = T001  (idempotent re-set)
                                .resolved_at = <ISO>
                       ↓ (task:wrap Phase 8.6)
tasks/completed/T001-deprecate-alias/main.md  (git mv from active/)
tasks/global-task-manager.md updated
                       ↓ (task:wrap Phase 7 — cleanup)
./dev cleanup destroys the worktree, branch, DB, container, ports
issues/ledger.json::L091.locked_by = null  (already resolved by Phase 8; cleanup is no-op for terminal)
```

**Ten distinct rows on five distinct surfaces** (`ledger.json`, frontmatter YAML, `phase-1-capabilities.yaml`, git refs, `<bare>/worktrees/.../ready-to-merge`) all encode the same logical fact ("T001 was driven by L091 and shipped via C18/Cutover-runbook"). All ten are soft FKs today; none are validated by the substrate. Drift modes:

- Phase 8 misses the resolution → orphan observation (`./dev observation reconcile-tasks` is the rescue).
- Phase 8.6 misses the close-out → 22 known cases historically (the Phase 8.6 mandatory note).
- Phase 7b YAML reconcile fails → feat branch has uncommitted YAML state (recovery in `task:wrap/SKILL.md:577-588`).
- Phase 3.5 missed migration → the L159 incident (5,322 Sentry events, "phase 3.5 commits not pushed before phase 6 prod merge").
- TID collision → mechanical rename (Phase 8.6 step 17, NOT a halt).
- Worktree-name vs branch-name mismatch → `git worktree move` inline (NOT a halt).

---

## §4. Map onto stores primitives

Tabular classification. Citations are in form `path:LINE` where applicable.

| 10.06 mechanism | Substrate fit | Citation / proposed wiring |
|---|---|---|
| Observation `open → investigating → confirmed` | **Already native** | bundled `observations` store at `src/handlers/*` + `bundles/observations/schema.yaml`. Field mapping is exact. |
| Observation `intent_contract` schema | **Already native (≈ 1:1)** | `observations.intent_contract.{objective, type, in_scope, out_of_scope, acceptance, tier_hint, contract_state, drafted_by, drafted_at, approved_by, approved_at}` matches the bundled schema. |
| Phase 0c `./dev observation contract --approve` | **Already native** | `stores observations update LXXX --approved-by blake --approved-at <now> --contract-state ready --invoker ai_with_human --approve-token <T>` (CLAUDE.md tier-A token doctrine). |
| Phase 0d T1/T2 inline-fix path | **Stays in 10.06** | These never reach the substrate's `tasks` store — they commit on main directly. The substrate cannot model them without leaking project-internal git operations into the schema. Substrate can RECORD them via `observations.resolution` but should not own the commit. |
| Phase 0d T3 → Stage 0.4 TID pick + Stage 4 worktree create | **Already native (auto-scaffold)** + **substrate refactor needed** for TID picker | `auto_scaffold.rs` writes `workspace_path`. Stores' `tasks next-id` already exists (CLAUDE.md). The 4-source TID scan (active/planning/completed/git-log) is project-internal — move it INTO the project's `./dev scaffold` shim, not the builtin. The builtin gives the row a `display_id` from the DB sequence; project scaffold maps that to the correct on-disk T### if collisions exist. |
| Stage 1.5 step 7 — observation `task_id = TXXX` back-link | **Already native** (`auto-promote` does this) | `auto_promote.rs:7-12` — "back-links observation.task_id to the new task." |
| Stage 1.5 step 4 — frontmatter writer | **Project-declared subscriber** | `agents.yaml` entry: subscribe to `tasks: ''→planning`, `command: "./dev task frontmatter {display_id} --capability {capability} --sub-item '{sub_item}' --ledger {ledger}"`. Requires schema extension on `tasks` row (capability, sub_item, ledger[], infra) — see §5. |
| Stage 2/3/5 planner / plan-reviewer / executor / code-reviewer chain | **Already native** | `agents/planner.md`, `plan-reviewer.md`, `executor.md`, `code-reviewer.md`; `tasks drive` is the runner. The 30-line orchestrator carve-out (Stage 5c minor REVISE) is the only delta — substrate's "executor only" doctrine is stricter, which is arguably an improvement. |
| Stage 5 phase loop with N phases | **Substrate refactor — Loop primitive missing** | `docs/primitives.md:25` lists Loop as missing. Today the phase loop is encoded via `current_phase < plan.phases.length` guard (primitives.md:31 example). Re-modelling phase iteration as a typed Loop primitive would clean up the workflow. |
| Stage 6 CodeRabbit one-shot | **New primitive — Check** | `primitives.md:31` already names this. Wire as: `agents.yaml` subscriber on the post-code-review transition, `command: "cr review --type all --base main --plain"`, parsed exit→pass/cr_blocked. **One-shot constraint** is a project rule, not a substrate property — encoded by NOT subscribing it to a re-fire transition. |
| Stage 7a rebase + integration smoke | **Stays in 10.06 OR project subscriber** | Project subscriber: subscribes to `tasks: code_review→ready_to_stamp` (new state), `command: "./dev rebase-and-smoke {workspace_path}"`. Project script runs git fetch + rebase + tsc + pytest. |
| Stage 7b capability YAML reconcile (`launch_readiness_*.py` chain) | **Project-declared subscriber + new Check primitive** | One subscriber (project-side `command: "./dev capability-reconcile {display_id}"`) running the three Python scripts in sequence. Check primitive routes pass→`stamp_ready`, fail→`yaml_reconcile_blocked` recovery state. The kill-switch behaviour (no YAML / no capability / infra:) is preserved by the script returning exit 0 with a "skipped" marker. |
| Stage 7c `./dev test gate` (11-step) | **New primitive — Check (deterministic external gate)** | `primitives.md:31` lists this exact use case. Subscriber `command: "./dev test gate"`. Pass→stamp; fail→`gate_blocked`. The 6-minute wall-clock argues for `claim_window_secs: 900` and `retry_policy: max_attempts: 1`. |
| Stage 7d ready-to-merge stamp | **Substrate refactor — schema field** | Stamp content (`feat_sha, main_sha, gate_sha, captured_at`) becomes a row column on `tasks` (`gate_stamp` JSON or four scalar columns). Stale-detection logic (compare current feat_sha + origin/main against stamp) becomes a guard on the `accepted` transition. The `.git/`-file mechanism is project-internal; replace with DB columns and the staleness guard. |
| `task:wrap` Phase 2 STOP/AUTO doc commit | **Stays in 10.06** | The classification rule is project-policy; keep in `./dev wrap-docs`. |
| `task:wrap` Phase 3 merge | **Already native (accept-merge)** | `accept_merge.rs` does the fast-merge on `in_review→accepted`. The `task:wrap` Phase 3 sequence becomes substrate-driven. |
| `task:wrap` Phase 3.5 regen `_feat_` migration on main | **Project subscriber** | New subscriber on `tasks: in_review→accepted` (peer with accept-merge, sequenced AFTER): `command: "./dev migrate regen-postmerge {display_id}"`. Project script does the L159-incident-safe sequence. Failure → `merge_blocked` recovery. |
| `task:wrap` Phase 4/4.5 stamp validation | **Substrate refactor** | Becomes a guard on the `in_review→accepted` transition that re-evaluates the stamp's freshness. If valid, fast-path; if stale, route through a re-stamp subscriber. |
| `task:wrap` Phase 5 deploy preflight | **Project subscriber** | `command: "./dev deploy preflight {display_id}"`. |
| `task:wrap` Phase 6 deploy prod | **Project subscriber + new Notification primitive** | `command: "./dev deploy prod --from-wrap"` on `accepted→deployed` (NEW transition / state). `primitives.md:29` notes Notification is partial today (ntfy on `deploy_blocked`); failure routing to `deploy_blocked` with ntfy + dispatch to `deployment_specialist` already works (`accept_merge.rs:5-7`, `schema_migrate.rs:7-10`). Extend to the deploy step. |
| `task:wrap` Phase 7 cleanup (`./dev cleanup`) | **Project subscriber + missing Decay primitive option** | Today: subscribe to `tasks: accepted→cleaned` (or to whatever terminal-after-deploy state is). Substrate-shaped alternative: a Decay-driven sweeper (primitives.md:28) on `tasks` rows that are `accepted` + N days old + branch merged. Either works; project subscriber is simpler. |
| `task:wrap` Phase 8 docs (daily note + summary) | **Stays in 10.06** | Project-internal; the note system is configured via `.notes-config.yml`. |
| `task:wrap` Phase 8 close source items | **Already native (causality)** | `auto-promote` already does the inverse direction (observation→task). The reverse closure (task complete → observation resolved) wants a built-in subscriber on `tasks: deployed→closed_out` that reads `linked_observations` and writes `observations.status=resolved` + `resolution`. Small new builtin OR project subscriber. |
| `task:wrap` Phase 8.5 gate filing | **New primitive — separate `gate` buffer** | `primitives.md:13` already names `gate` as a typed Buffer. Bundle a `gate` store alongside `observations` and `tasks`. Schema mirrors `issues/gate.json` (id, type=script\|decision, category 1-7, command/question/options/implications, status pending\|executed\|cancelled\|answered, audit_log[]). Tier-A on the `pending→executed` transition (Blake-only). |
| `task:wrap` Phase 8.6 close out (Meta status, git mv to completed/) | **Substrate refactor** | The `tasks/active/ → tasks/completed/` filesystem move is the FS projection of `tasks.status: deployed→closed_out`. `tasks render` already projects rows to markdown — extend to write to the right directory based on status. The Meta-Status flip is just the projection re-rendering. |
| `tasks/global-task-manager.md` update | **Already a render artifact** | Same idea as Phase 8.6 — derived from the stores DB, written by a `tasks render-gtm` verb. |
| `./dev new` port slot allocation, docker-compose.override generation, .env templating, sandbox seed | **Stays in 10.06** | Project-internal. Auto-scaffold's contract is "row arrives → worktree path written"; the ~12 steps in `cmd_new` past `git worktree add` are explicitly the heavy tail that should NOT live inside `auto_scaffold.rs`'s budget (POC §5). The shim approach already proven. |
| `./dev` postgres database (`unified_main`, `unified_feat_<slug>`) | **Stays in 10.06** | DB schema is the application's, not the substrate's. |
| `./dev` Sentry integration | **Stays in 10.06** | Until a Sentry→observation auto-feed subscriber is wanted. |
| Secrets broker (sb / si, fly secrets) | **Stays in 10.06** | Already gated by category 5 in the gate store. |
| Approval-token mechanism | **Already native** | CLAUDE.md tier-A doctrine + `~/.config/stores/approve.token.age`. The 10.06 `--approve` invoker check is honor-system; stores' is cryptographically gated — substrate STRICTLY STRONGER. |
| Phase 1 capability YAML structure | **Stays in 10.06 schema-wise; substrate sees only `tasks.capability + sub_item`** | The capability YAML IS the project's domain model. Substrate stores the fact "task affects C18 / Cutover runbook"; reconcile script lives project-side. |
| Ledger lock (`./dev observation lock`) | **Substrate refactor — Capacity primitive (rate=1, scope=row)** | Today the lock is a soft FK + 2h aging. In substrate terms it's a per-row Capacity primitive (`primitives.md:30`) — at-most-one active session per row. The `ai_autonomous` claim window in `agents.yaml` already exists (`claim_window_secs`); extend that primitive to also serve the user-facing "I'm working on this" semantic, not just the agent's claim. |

---

## §5. The linking-chain question (deep dive)

**The chain in 10.06 today (re-stating compactly):**

1. `issues/ledger.json::LNNN.intent_contract.contract_state` (substrate-tracked field on observation)
2. `issues/ledger.json::LNNN.task_id` (set Stage 1.5 step 7 — soft FK to a task ID, never validated)
3. `tasks/active/TXXX-<slug>/main.md` frontmatter `ledger:` (set by `./dev task frontmatter --ledger`)
4. `tasks/active/TXXX-<slug>/main.md` frontmatter `linked_ledger_ids:` (set by `./dev new --links` — DUPLICATE of #3 today, separate code path)
5. `tasks/active/TXXX-<slug>/main.md` frontmatter `capability + sub_item`
6. `git branch feat/<slug>` (named with the slug; TID is derivable but not enforced)
7. `~/repos/clients/10.06-wt/10.06-feat-<slug>/` (worktree directory; slug-named, not TID-named)
8. `<bare>/worktrees/10.06-feat-<slug>/ready-to-merge` (the gate stamp; not committed, per-worktree)
9. `app/backend/config/phase-1-capabilities.yaml::CXX.sub_items[label==sub_item].task_id` (set by rollup script Stage 7b)
10. `app/backend/config/phase-1-capabilities.yaml::CXX.sub_items[…].revision_history[].task_id`
11. Git commit messages: `task(TXXX): COMPLETE`, `feat(TXXX): clean-named migration`, `docs: TXXX reconcile capability status`
12. Fly deploy stamps (`./dev prod stamp` writes a versioned record on each deploy)

**The chain in stores today:**

- `observations.intent_contract.*` — schema-enforced.
- `observations.task_id` (soft FK, plain text — same as 10.06).
- `tasks.linked_observations` (already present in bundled `tasks` schema, set by `auto_promote.rs`).
- `tasks.workspace_path` (set by `auto_scaffold`).
- `tasks.branch` (set by `auto_scaffold` shim).
- `tasks.display_id` — DB-managed.
- `tasks.status` lifecycle.
- `tasks.drive_pid`, `tasks.drive_started_at` (auto_drive bookkeeping).
- `transition_history` — every state change with actor.
- `agents_run.claim_window_secs` lock — analog of `./dev observation lock`.

**Gaps in stores' schema vs the 10.06 chain:**

| 10.06 field/link | Stores has? | What's needed |
|---|---|---|
| `observations.task_id` back-link | YES | None |
| `tasks.linked_observations[]` | YES | None |
| Frontmatter `capability` | NO | Add `tasks.capability TEXT` (CXX shape — pattern `^C\d{2}[a-z]?$`) |
| Frontmatter `sub_item` | NO | Add `tasks.sub_item TEXT` |
| Frontmatter `infra:` (escape hatch) | NO | Add `tasks.infra TEXT` (mutually exclusive with capability — schema validator) |
| Frontmatter `ledger[]` (separate from linked_observations?) | DUPLICATE | Resolve: keep `tasks.linked_observations` as the canonical FK; render frontmatter `ledger:` from it. Drop `linked_ledger_ids` as a separate field — it's the same data. |
| Capability YAML row reference (`CXX.sub_items[].task_id`) | NO (and shouldn't) | This is project-domain data. The YAML is the project's. Stores doesn't need a row for it; the project's reconcile subscriber writes it. |
| Ready-to-merge stamp `feat_sha + main_sha + gate_sha + captured_at` | NO | Add `tasks.gate_stamp JSON` OR four columns. Add a guard on `in_review→accepted` that compares stamped SHAs against current branch + origin/main — if stale, transition `in_review→stamp_stale` (recovery state) which re-fires the gate subscriber. |
| Branch / worktree dir name vs TID | Stored separately | Today fine. The TID-collision rename (`task:wrap` Phase 8.6 step 17) is a 10.06-specific operation — substrate's `display_id` is DB-allocated, no collision at that layer. |
| Sentry / fly deploy stamps | NO | These are project-domain; no substrate change. |

**Soft FK → hard FK candidates:**

- `observations.task_id`: today plain text; promote to a hard FK to `tasks.display_id`. Validator on `observations.update --task-id` must reject if no such task exists. **Worth it** — orphan observations are a known historical drift.
- `tasks.linked_observations[]`: each entry must be a valid `observations.display_id`. Validator on `tasks add --linked-observations` and `tasks update --linked-observations`. **Worth it** — auto_promote already enforces this on insert; extend to update.
- `tasks.capability` (proposed new field): validator that the value matches a Phase 1 capability in `phase-1-capabilities.yaml`. **Tradeoff** — couples substrate to a project-specific YAML. Better: validate the *shape* (`^C\d{2}[a-z]?$`) and let the project's reconcile subscriber catch unknown capabilities at Stage 7b. The substrate stays generic.

**Specific subscribers to add (concrete YAML):**

```yaml
# .stores/agents.yaml additions

  # (1) Stage 1.5 step 4: write 10.06 frontmatter when scaffold completes
  - name: write-frontmatter
    subscribes_to:
      - store: tasks
        transition: { from: planning, to: planning }   # or new: scaffolded → ready_for_planner
        predicate: { op: "!=", left: "$workspace_path", right: "" }
    command: "cd {workspace_path} && ./dev task frontmatter {display_id} --capability {capability} --sub-item '{sub_item}' --ledger {linked_observations_csv}"
    claim_window_secs: 60
    retry_policy: { max_attempts: 1, backoff: linear }

  # (2) Stage 7b: capability YAML reconcile
  - name: capability-reconcile
    subscribes_to:
      - store: tasks
        transition: { from: code_review, to: ready_to_stamp }   # new state
    command: "cd {workspace_path} && ./dev capability-reconcile {display_id}"
    claim_window_secs: 120
    retry_policy: { max_attempts: 1, backoff: linear }
    # exit non-zero → deploy_blocked recovery (existing pattern)

  # (3) Stage 7c: test gate Check
  - name: gate-check
    subscribes_to:
      - store: tasks
        transition: { from: ready_to_stamp, to: stamp_pending }
    command: "cd {workspace_path} && ./dev test gate"
    claim_window_secs: 900
    retry_policy: { max_attempts: 1, backoff: linear }

  # (4) Stage 7d: emit stamp (substrate writes the cols, but project reads SHAs)
  - name: emit-stamp
    subscribes_to:
      - store: tasks
        transition: { from: stamp_pending, to: in_review }
    command: "cd {workspace_path} && ./dev stamp-emit {display_id}"
    claim_window_secs: 30
    retry_policy: { max_attempts: 1, backoff: linear }

  # (5) task:wrap Phase 3.5: regen _feat_ migration post-merge
  - name: regen-feat-migration
    subscribes_to:
      - store: tasks
        transition: { from: in_review, to: accepted }   # peer with accept-merge, sequenced after
    command: "cd {workspace_path} && ./dev migrate regen-postmerge {display_id}"
    claim_window_secs: 180
    retry_policy: { max_attempts: 1, backoff: linear }

  # (6) task:wrap Phase 6: deploy prod
  - name: deploy-prod
    subscribes_to:
      - store: tasks
        transition: { from: accepted, to: deployed }
    command: "cd {workspace_path} && ./dev deploy prod --from-wrap"
    claim_window_secs: 1800
    retry_policy: { max_attempts: 1, backoff: linear }

  # (7) task:wrap Phase 8: close source observations
  - name: close-source-observations
    subscribes_to:
      - store: tasks
        transition: { from: deployed, to: closed_out }
    command: "builtin:close-linked-observations"   # NEW builtin, ~30 lines of Rust
    claim_window_secs: 60

  # (8) task:wrap Phase 7: cleanup worktree
  - name: cleanup-worktree
    subscribes_to:
      - store: tasks
        transition: { from: closed_out, to: cleaned }
    command: "cd {workspace_path}/.. && ./dev destroy {slug} --yes"
    claim_window_secs: 300
```

This wiring keeps every project-side mechanic (port slots, docker, alembic, fly, capability YAML) in `./dev` while the substrate enforces transitions, schema, lifecycle, and FK linkage.

---

## §6. Implementation plan

Phases are dependency-ordered. P# numbers are local to this plan, not stores task IDs.

### P1 — Dual-channel taste-test (no commits to truth yet)

**Deliverable:** A throwaway 10.06 worktree (like the POC's `experiment/stores-poc-autoscaffold`) configured with stores side-by-side. Run one full real task end-to-end on the new path; mirror to the old path. Diff outputs.

- Files: `.stores/agents.yaml` (subscribers 1-2 only — auto-promote + auto-scaffold + the frontmatter writer); `.stores/config.yaml` (`scaffold.command: ./dev scaffold {display_id}`); `./dev scaffold` subcommand (per POC §5).
- Test approach: pick L091-equivalent (small T1/T2 for quick cycle); run twice (once via stores, once via skill); compare ledger row, frontmatter, branch, worktree, capability YAML.
- Unblocks: P2.
- Deferred: deploy chain, capability reconcile, gate filing.
- Precondition: none.

### P2 — Schema extension on `tasks` for 10.06 fields

**Deliverable:** Add `capability`, `sub_item`, `infra` columns to bundled `tasks` schema. Validation rules: `(capability AND sub_item) XOR infra` (mutually exclusive); pattern guards. Run `stores migrate` against the test DB.

- Files: `bundles/tasks/schema.yaml`; `bundles/tasks/migrations/<rev>__add_10_06_fields.sql`.
- Test approach: round-trip test — `tasks add --capability C18 --sub-item "Cutover runbook"`, `tasks show`, verify schema rejects mismatched shape.
- Unblocks: P3.
- Deferred: enforcing capability against the YAML — that's the project's reconcile subscriber's job.
- Precondition: P1 surfaced any schema friction.

### P3 — Gate buffer (new bundled store)

**Deliverable:** Bundle a `gate` store. Schema mirrors `issues/gate.json`: `id (G\d{3,})`, `type (script|decision)`, `category (1-7)`, `one_liner`, `business_reason`, `technical_detail`, `command|question`, `options[]`, `implications`, `status (pending|executed|cancelled|answered|deferred)`, `task_ref`, `priority`, `audit_log[]`, `why_not_autonomous`, `api_checked`. Tier-A on `pending→executed` (token-mediated) and on `pending→answered`.

- Files: `bundles/gate/schema.yaml`, `bundles/gate/CLAUDE.md`.
- CLI: `stores gate add | list | show | run | answer | cancel | defer`.
- Test approach: file-fix-resolve cycle for one each of script + decision.
- Unblocks: P4.
- Deferred: integration with the `dev gate run` broker socket — keep the project's existing implementation as the runner; substrate just records the row state.
- Precondition: P2.

### P4 — Check primitive (formalised) + capability-reconcile + test-gate subscribers

**Deliverable:** Encode `Check` as a typed primitive (`primitives.md:31`). Each Check subscriber: command, exit-0=pass, non-zero=fail. Pass routes to forward state; fail routes to `<check>_blocked` with stderr captured in `blocked_reason`. The three concrete checks: `cr review`, `./dev capability-reconcile`, `./dev test gate`.

- Files: `src/flow/primitives/check.rs` (new module); update `src/flow/dispatcher.rs` to recognize Check semantics; `agents.yaml` subscribers 2, 3, 4 from §5.
- Test approach: each Check fired against a fixture. Pass + fail paths. Verify `blocked_reason` captured and a `<check>_blocked → ready_to_<check>` recovery transition is generated.
- Unblocks: P5.
- Deferred: Loop primitive (phase iteration) — existing guard pattern is acceptable for now.
- Precondition: P3.

### P5 — Stamp schema + staleness guard

**Deliverable:** Add `tasks.gate_stamp JSON` (or four cols `feat_sha, main_sha, gate_sha, captured_at`). Add a guard on `in_review→accepted` that compares stamp against current branch + origin/main; on mismatch, transitions to `stamp_stale` (re-fires gate-check subscriber).

- Files: `bundles/tasks/schema.yaml`; `bundles/tasks/migrations/<rev>__add_gate_stamp.sql`; `src/flow/transitions/in_review_to_accepted.rs` guard.
- Test approach: stamp valid → fast accept; staleness → loops back through P4 gate-check. Confirm stamp re-emit on retry.
- Unblocks: P6.
- Precondition: P4.

### P6 — Post-merge ceremony chain (regen-feat-migration + deploy-prod + close-observations + cleanup)

**Deliverable:** Wire subscribers 5-8 from §5. Add new `tasks` states `accepted → deployed → closed_out → cleaned`. Add `builtin:close-linked-observations` (~30 lines Rust): read `tasks.linked_observations[]`, write `observations.status=resolved` + `resolution = "<TID>: <auto-derived from tasks.title>"` for each. Failure routing reuses `deploy_blocked` pattern.

- Files: `src/flow/builtins/close_linked_observations.rs`; `agents.yaml`; bundled `tasks` schema state machine.
- Test approach: full ceremony on a fixture row (mocking `./dev deploy prod` exit). Confirm causality: `tasks.deployed` → all `linked_observations` resolved → `cleanup-worktree` fires last.
- Unblocks: P7.
- Precondition: P5.

### P7 — Truth-flip on a dedicated worktree, observe for one week

**Deliverable:** Designate one ongoing 10.06 task (a small one) and run it ENTIRELY through stores. Keep `./dev observation` + `tasks/` filesystem as fallback only. Observe drift over 7 days.

- Files: none new — operational milestone.
- Test approach: at end of week, count drift events. < 3 → ready for P8. >= 3 → file observations, fix, retry.
- Unblocks: P8.
- Deferred: full repo migration.
- Precondition: P6.

### P8 — Migrate `issues/ledger.json` + `issues/gate.json` historical data

**Deliverable:** One-shot importer. Read both JSON files, write `observations` + `gate` rows. Preserve `LNNN`, `GNNN` IDs. Audit-trail-preserving (every entry's audit_log → `transition_history` rows).

- Files: `scripts/migrate-1006-ledger.py` (project-side, NOT a substrate verb — it's project-specific data).
- Test approach: import on a copy DB; spot-check 20 random entries; confirm `transition_history` count matches sum of audit_log lengths.
- Unblocks: P9.
- Deferred: deleting the JSON files — keep them read-only for one month.
- Precondition: P7.

### P9 — Deprecate `./dev observation` + `./dev gate` mutating verbs; rewire to `stores` CLI

**Deliverable:** `./dev observation add` etc. become thin wrappers calling `stores observations add`. Read verbs (`./dev observation list`, `gate list`) survive as convenience views over the substrate.

- Files: `dev` script changes (about 200 LOC of replacement).
- Test approach: every `./dev observation` mutating verb has the same effect as before but writes go through stores. Confirm with substrate `tasks list`, `observations list`.
- Unblocks: nothing — this IS the migration.
- Precondition: P8.

### Deliberately deferred (do NOT smuggle into the plan)

- `auto-drive` integration with 10.06's existing `task-workflow:planner` etc. subagents. The bundled `agents/*.md` system prompts are stores-internal; matching them to 10.06's plugin system is a separate design.
- The dual-write Sentry → observation auto-feed.
- The capability YAML treated as a substrate Buffer rather than a project artifact.
- Fleet/multi-repo concerns. (Out of scope today.)

---

## §7. Confidence + risks + open questions

### High confidence (cite the evidence)

- **Phase 0 → Stage 1.5 maps cleanly to `auto-promote + auto-scaffold + a frontmatter subscriber`.** Evidence: POC §3 already proved auto-promote + auto-scaffold end-to-end; observation→task_id back-link is in `auto_promote.rs:7-12`.
- **Stage 5 phase loop maps to bundled `tasks drive` cycle.** Evidence: `target/release/stores tasks --help` shows `submit-plan / submit-plan-review / submit-execute / submit-review / submit-wrap` — 1:1 with 10.06 planner/plan-reviewer/executor/code-reviewer/wrap.
- **Test gate, CodeRabbit, capability YAML reconcile are Check primitives** as named in `docs/primitives.md:31`.
- **`./dev new`'s ~12 steps past `git worktree add` are project-internal** and should NOT live in `auto_scaffold.rs`. Already concluded by POC §5 and re-confirmed here by reading `dev:333-685` cover to cover.
- **The L159 phase-3.5-migration-not-pushed incident is a real failure mode** that needs explicit subscriber wiring (P6 subscriber 5).
- **Approval-token doctrine in stores is strictly stronger** than 10.06's `--approval-invoker = agent | $USER` honor system. Both approve/ratify moments (Phase 0c, U3 accept, U4 amend) gain cryptographic enforcement.

### Lower confidence

- **The 30-line orchestrator carve-out at Stage 5c (minor REVISE)** doesn't map cleanly into a substrate that's strict about "executor only writes code." This is a behavioural delta, not a schema delta — could be left as a project rule layered above the substrate.
- **The `./dev test gate` 6-minute wall-clock + per-worktree lock semantics** vs `claim_window_secs`. Need to verify: does `stores agents run` poll cadence interact with a 900-sec claim window in a way that fires the gate twice on a slow run? The `auto_drive.rs` `pid_is_alive` watchdog suggests the answer is yes if PIDs aren't tracked — and the gate doesn't naturally have a long-running PID. Probably wants `gate-check` to spawn detached and write its PID, like `auto-drive`.
- **The capability YAML's `revision_history` "overwrite-with-date-preservation"** behaviour vs git history. The script preserves the original date per `(task_id, sub_item)` — but a substrate Causality query (`primitives.md:32`) would prefer all revisions captured. This is a design question, not a coding one.
- **Phase 8.6 close-out invariant** ("every wrap that reaches Phase 9 must leave `tasks/active/TXXX/` non-existent and `tasks/completed/TXXX/main.md` with Status: COMPLETE") — historically failed 22 times. The substrate fix is to make `tasks render` write to the right directory based on `tasks.status`. But what about manually-edited markdown content drifting from the DB row? Needs a render-on-status-change discipline that's idempotent and non-destructive.

### Open questions for Blake

1. **Capability YAML treatment** — stays project-side as a domain artifact (cleanest), OR gets bundled as a typed Buffer? My recommendation: stay project-side. It's not the substrate's job to model launch-readiness reporting.
2. **Bundled `gate` store** — desirable to bundle alongside `observations` + `tasks`? Or do we keep `gate` 10.06-specific until a second project demands it? The seven-category classification feels project-specific (category 7 "live-env policy" is very 10.06-shaped).
3. **The 30-line orchestrator carve-out at Stage 5c** — kill it (substrate purist path: only executor writes code) or preserve it (pragmatic path: project rule)? The substrate doesn't enforce either way; the project's `agents/` system prompts can.
4. **`linked_ledger_ids` vs `ledger:` frontmatter duplication** — confirmed today as the same data through two write paths. Acceptable to drop one in stores (recommend: drop `linked_ledger_ids`, render frontmatter `ledger:` from `tasks.linked_observations`)?
5. **Promote `observations.task_id` to a hard FK?** Trade-off: enforces correctness but breaks the resolve-then-task-deleted historical pattern. My recommendation: enforce on insert/update, allow null, allow stale references after the task is moved (don't cascade).
6. **Sentry → observation feed** — is this in scope for v1, or deferred? POC didn't touch it; this study deferred it; but it's the dominant friction surface today.
7. **Phase 0a.5 specialised-skill check (re-route to `/converge`, `/qa:walk`, etc.)** — this is fundamentally project-specific. Stays in the orchestrator agent's prompt, not the substrate?
8. **Auto-drive's interaction with 10.06's `task-workflow:*` subagent registration** — POC §7 deliberately deferred. Does the substrate's bundled `agents/*.md` replace 10.06's `.claude/skills/`-managed subagents, or coexist with them?

### Items deliberately deferred to follow-up studies

- The dual-channel cohabitation period — what triggers, what alerts, what does P7 actually look like operationally?
- Sentry → observation auto-feed.
- Fleet / multi-project concerns (today: 10.06 only).
- Fly deploy stamp ingestion as a substrate Causality node.
- The `tasks render`/`render-gtm` behaviour when frontmatter has been hand-edited (drift detection / repair).

---

## §8. Cross-reference to `docs/poc-1006-autoscaffold.md`

I read `docs/poc-1006-autoscaffold.md` AFTER writing §1-§7. Cross-reference now:

### Where this study confirms the POC

- **`./dev scaffold` subcommand recommendation** (POC §5) is correct and identical to my P1 / P2 wiring. The POC's 60-line `cmd_scaffold` skeleton is the exact shape of the proposed integration.
- **Heavy `cmd_new` tail (port slot, docker-compose, .env, sandbox seed) MUST stay outside `auto_scaffold.rs`** (POC §3 verdict + §5 "What about ports, docker, seed?"). Re-confirmed by my §3.1 + §4 table.
- **The `--links` mandatory + slug positional + mixed stdout shape are dealbreakers** for direct `./dev new` invocation (POC §3 table). Re-confirmed.
- **Substrate-side friction listed in POC §3.4** (3 items: `add` flag inconsistency, in-scope JSON-string flattening, opaque `confirm` guard error). All re-observable today.

### Where this study extends the POC

- POC §1 spec'd auto-scaffold's contract; this study extends to **the full Phase 0 → Stage 8 + task:wrap workflow**, not just scaffold.
- POC §7 listed 6 "out-of-scope follow-ups." This study addresses each:
  - **Auto-drive integration** (POC §7.1) — addressed in §7 Open Q8.
  - **Dual-channel question** (POC §7.2) — addressed in P7 of the implementation plan.
  - **Schema fit** (POC §7.3) — addressed in P2 (schema extension for `capability, sub_item, infra`) + §5's hard FK discussion.
  - **`stores` on PATH** (POC §7.4) — out of scope, hygiene only.
  - **Auto-locking linked observations** (POC §7.5) — addressed: my P1's frontmatter writer subscriber subsumes it (the `--ledger` flag also locks per `./dev new` semantics, OR a separate auto-lock subscriber on `tasks: planning→executing`).
  - **Cleanup symmetry on `tasks accept`** (POC §7.6) — addressed in P6 subscriber 8 (`cleanup-worktree`).

### Where this study disagrees / refines

- POC §6 said "confidence: HIGH for the tested slice." That holds for the worktree-creation slice. But the study's broader scope shows the **stamp+gate+YAML reconcile chain at Stages 7a–7d is the single highest-stakes integration point** — confidence there is medium, not high, until P4 + P5 ship and are tested against a real task. Specifically: the stamp's freshness invalidation logic on rebase is not yet substrate-modeled.
- POC §3's friction list is focused on `add`/`update` flag asymmetry. This study surfaces **structural** friction too: the duplicated `linked_ledger_ids` vs `ledger:` frontmatter (§5 table), the soft FK approach to `observations.task_id` (§5 hard-FK candidates).
- POC §5 recommends `./dev scaffold` as the integration shim. This study agrees — **and** recommends extending it to a small family of `./dev` verbs called by substrate subscribers: `./dev capability-reconcile`, `./dev migrate regen-postmerge`, `./dev stamp-emit`, `./dev rebase-and-smoke`. Same shape, same boundary discipline.

### Net read

The POC was correct in scope and conclusions for what it tested. Its judgment that "no changes to stores or to 10.06's `./dev new`" hold for the auto-scaffold slice. The broader workflow integration **does** require schema extension (P2), Check primitive (P4), and stamp schema (P5), but each is small (<~150 LOC of Rust + bundled SQL each). The 10.06-specific weight (~70% of `./dev`'s `task:open`-relevant code) stays in `./dev` as small wrapper subcommands called by subscribers — Blake's intuition that "this is mostly refactoring" is correct **for the orchestration backbone**, but underestimates the integration surface (six new subscribers + three new substrate primitives + one new bundled store) needed to retire `task:open / task:wrap` as the source of truth.

---

## End of study
