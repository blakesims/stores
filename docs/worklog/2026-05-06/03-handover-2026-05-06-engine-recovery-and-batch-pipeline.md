# Handover — engine recovery + batch pipeline (2026-05-06 → next agent)

**Date:** 2026-05-06
**Type:** handover / operating-procedure

## ⚠ NEXT STEPS — restore throughput to 5 active drives (do this FIRST)

Pipeline is currently 0 actively driving. That's the critical state. Fastest restoration:

1. **Ratify L107, L039, L087** (already drafted contracts). For each: `observations update LXXX --contract-state ready --approved-by blake --approved-at "$(date -Iseconds)" --invoker ai_with_human --approve-token <T>`, then `investigate` → `confirm`.
2. **Run `./target/debug/stores agents run --once`** → fires 3 auto-promote → 3 auto-scaffold → 3 auto-drive (claude-code). Now driving 3.
3. **Fix L126 directly** (template change in `stores/tasks/templates/executor-brief.md.tpl` + `code-reviewer-brief.md.tpl` — tier-conditional Phase Being Reviewed section, mirror T039's planner-brief pattern). `cargo build` → resume T036 + T029 (`tasks resume <id> --invoker ai_with_human --approve-token <T>`). They'll cascade to executing; manually `tasks drive <id> --claude-code &`. Now driving 5.
4. **If you can't complete L126 fix quickly**: draft + ratify 2 more contracts instead (L122 manual/auto drive collision, or L061 pre-promotion precheck). Same approach as step 1.

Don't wait, don't observe, don't diagnose — *fill the pipeline first*, then handle individual review/accept moments as they arrive.

## Summary

Today's session: fixed three interlocking substrate bugs (L116, L117, L123) that were silently breaking the dogfood path; ran a 3-task parallel batch through the engine using claude-code; shipped 3 of 6 cleanly via verbs-only (T037 + T039 + T033); 3 stuck on newly-surfaced bugs (T036/T029 → L126, T038 → unfiled-T2-resume-with-null-plan). User established a session doctrine for pragmatic-escape-with-no-raw-SQL (CLAUDE.md § "Session doctrine — 2026-05-06") and a streamlined operating loop. **You are coming in mid-flight.** T037's deploy is the last action; everything else is either shipped, blocked-with-filed-obs, or stale.

## Details

### Standing operating procedure (the user's instructions today)

1. **Semi-autonomous mode.** The user pre-authorized this session with the approval token below. Use `--invoker ai_with_human --approve-token <T>` for U-moments (ratify, accept, resume) without re-asking. Token: `<redacted-approval-token>` (the user pastes it; treat as session-scoped — do NOT persist anywhere on disk).

2. **Don't stop unless you must.** Halt only for: (a) genuine cost/architectural trade-off you can't resolve, (b) something you flag as uncertain. Otherwise drive forward through ratify → promote → drive → review → accept → deploy.

3. **Never raw-SQL the substrate DB.** Reads via `sqlite3 ... SELECT` are fine; UPDATE / DELETE / INSERT are forbidden. If you reach for raw SQL, the right move is to fix the broken handler in code instead. See CLAUDE.md § "Session doctrine — 2026-05-06". This rule was earned today by me hand-editing dispatch_locks; do not repeat.

4. **Pragmatic escape.** When two or more substrate bugs interlock and block the dogfood path, escape to direct code edits (Edit/Write, subagents, normal cargo + git). File the friction as observations either way; the pain is the data. Name the friction in the commit message ("Couldn't dogfood because L### + L### interlock; direct fix per session doctrine").

5. **Concurrency: aim for 5 concurrent tasks** going through the engine at any one time. The substrate's hard cap is `drive.max_parallel` in `.stores/config.yaml` (set to 5 today, was 3). The cap only applies to auto-drive; manual drives are invisible (L122).

6. **Reviews via `/codex:review`.** The user installed `codex@openai-codex` plugin today. The new procedure:
   - Tasks reach `in_review` (after wrap).
   - Run `/codex:review` (or `codex:rescue` with a "review this PR" prompt — confirm with user which slash command/skill is canonical) against the task's branch + diff.
   - **If codex finds a genuine issue** (use your judgment — substantive bugs / regressions / contract violations, NOT cosmetic nits), LEAVE the task `in_review` and surface the finding to the user for discussion.
   - **Otherwise** auto-accept via the token: `tasks accept <id> --invoker ai_with_human --approve-token <T>`.

7. **Status-update format the user wants.** When asked or at meaningful inflection points, output **3-5 lines max** in this shape:
   1. how many tasks in the pipeline
   2. how many being reviewed right now
   3. overall engine health — where is the biggest issue
   4. anything else to flag / discuss
   No long prose updates. The user wants signal.

8. **Stale tasks ≠ broken tasks.** If a row is blocked because the work is duplicate / shipped-out-of-band, that's not a bug to fix — it's L124 territory (need a `tasks abandon` verb; doesn't exist yet, so leave them blocked-as-cruft). Don't burn a drive cycle re-doing work that's already on main. Concrete examples today: T034 (Pi runner smoke test, already shipped via `db3d15a`) + T032 (auto-scaffold, already shipped per engine-health.md ✓).

9. **Worklog hygiene.** When writing notes, use `./docs/worklog/new-note.sh <slug>` (slugs are kebab-case). Never hand-create note files. Read before Edit.

10. **`fast` mode toggle.** Claude Code has a `/fast` mode that uses Opus 4.6 for faster output (no model downgrade). User hasn't requested it; use Opus 4.7 (current) unless asked.

### Engine state at handover

**Shipped today (deployed: merged to main + cargo-installed + schema-migrated):**

| Task | Linked obs | What landed |
|---|---|---|
| T039 | L093 | Tier-aware planner brief (T1/T2/T3/unset Tier Guidance section + 4 snapshot tests) |
| T033 | L038 | Pre-flight `depends_on` guard in `drive_loop` (refuses to start when any dep not accepted) |

**Accepted, deploying as of writing (waiting on the daemon's final poll for accept-merge + cargo-install + schema-migrate; check `/tmp/stores-dogfood/deploy-T037.log`):**

| Task | Linked obs | What's landing |
|---|---|---|
| T037 | L049 | `auto-resolve-observation` builtin — closes step 10 of the auto-pipeline (linked obs → resolved when task hits schema_migrated). 457 LOC new file + 6 framework transitions on observations schema. |

**Direct-code fixes I shipped to main today (not via the dogfood loop, per session doctrine — these unblocked the dogfood):**

| Commit | Bug | What it fixes |
|---|---|---|
| `6f869fb` | L117 | `auto-promote` now calls `fire_on_entry_follow_ons`; T1 rows cascade `planning → ready → executing` inside the auto-promote txn. |
| `7703608` | L116 | Starting-line seeder uses per-agent presence check; new transitions firing between `agents run --once` calls are no longer claimed as `skip-historical`. |
| `49c5129` | L123 | Added a T1-aware PASS transition `code_review → complete` (guard `tier_hint == 'T1'`); T1 rows can PASS without a phase count. |
| (CLAUDE.md) | doctrine | Session doctrine added forbidding raw-SQL writes; defines the pragmatic escape. |

**Blocked, filed-bugs, awaiting fix:**

| Task | Linked obs | Reason | Filed |
|---|---|---|---|
| T036 | L020 (render canonicalize) | T1 brief defect on executor + code-reviewer briefs ("Phase 1 of 0" malformed brief; executor refuses commit; reviewer FAILs) | **L126** |
| T029 | L071 (drive exit=1 substrate notify) | Same — L126 | **L126** |
| T038 | L043 (orchestrator inline investigation) | T2 with `plan=null` after planner crash; `tasks resume` cascaded to executing past plan_review where it shouldn't (T2 needs a plan). **Not yet filed — file as T128-ish.** | **unfiled** |

**Stale, won't ship via substrate (need L124 abandon verb):**

| Task | Reason |
|---|---|
| T034 (L110 Pi runner smoke test) | Pi runner already shipped via `db3d15a` (different lineage) |
| T032 (L032 auto-scaffold) | Per engine-health.md "Recently shipped" — work landed out-of-band |

### Today's filed observations (most → least relevant)

- **L116** — starting-line seeder claims new transitions as skip-historical (✅ shipped above)
- **L117** — auto-promote skips on-entry hooks (✅ shipped above)
- **L121** — Pi runner has no timeout / liveness; quota exhaustion stalls drive silently. Three fix shapes sketched (helper wall-clock budget; Rust-side timeout; substrate-side liveness watchdog).
- **L122** — manual `tasks drive` doesn't set `drive_pid`; auto-drive can race-spawn a duplicate drive. Fix shape: drive.rs sets/clears drive_pid like auto-drive does.
- **L123** — T1 PASS submit-review missing transition (✅ shipped above)
- **L124** — need a `tasks abandon` verb for stale rows (compounds with L002, L092). T2.
- **L125** — escalation noise (user-escalation subscriber misclassifies plain `blocked` as `deploy_blocked`).
- **L126** — T1 brief defect on executor + code-reviewer briefs (sibling of L093 on different templates). T1 fix.
- **(unfiled)** — `tasks resume` on a non-T1 row with `plan=null` cascades to executing where it can't proceed. Should hold at `ready` (or revert to `planning`) until plan exists.

### Pre-drafted contracts ready for the next batch

| Obs | Tier | Contract state | What it ships |
|---|---|---|---|
| **L107** | T2 | draft (full content) | Watchdog scope: don't reap pre-existing dead drive_pids from prior daemon lifetimes (false-positive shape); add daemon-epoch / lock-recency check |
| **L039** | T2 | draft (full content) | Daemon retry-on-failure: `retry_policy.max_attempts` + backoff actually respected |
| **L087** | T2 | draft (full content) | Auto-promote silent-fail: lock-marked-ok decoupled from "did we actually create the task row" |
| **L093** | T1 | ratified, shipped via T039 | (already done) |

To ratify a drafted contract: `observations update LXXX --contract-state ready --approved-by blake --approved-at "$(date -Iseconds)" --invoker ai_with_human --approve-token <T>` then walk it `investigate` → `confirm`.

### Recommended next batch (5 candidates)

1. **L126** — fix the T1 brief defect first; unblocks T036 + T029 immediately. Small T1 (template + brief.rs).
2. **L107** — watchdog scope. Closes the false-positive blocked-on-restart class. T2.
3. **L039** — daemon retry. Engine resilience. T2.
4. **L087** — auto-promote silent-fail. Same dispatch-lock-shape as L107; consider folding both into a single dispatch_lock primitive refactor. T2.
5. **L122** — manual/auto drive collision. Cheap T1; touches drive.rs. (Could also bundle into T037-style work.)

After L126 ships: resume T036 + T029 (will work this time). After L107 ships: stop manually-spawning drives that get watchdog-reaped on restart. After whatever fixes the T2-resume-null-plan: resume T038.

### What NOT to do (lessons)

- **Don't raw-SQL.** Period. CLAUDE.md § "Session doctrine — 2026-05-06". Read it before doing anything.
- **Don't kill manually-spawned drives without a reason.** They're real progress. Do kill stalled Pi runner subprocesses (L121).
- **Don't re-drive rows whose work is already on main from a different lineage.** Look at `git log main..HEAD` on the worktree branch first (T034 / T032 pattern).
- **Don't mix manual `tasks drive` with auto-drive on the same row.** L122. If you manually drive, disable auto-drive in agents.yaml temporarily (or wait until L122 ships).
- **Don't accept a task whose wrap_log lies about what changed.** Read `git diff main..HEAD --stat` on the worktree first; the diff's negatives are usually because the branch is BEHIND main, not because the executor deleted things — but verify.

## Follow-ups

- T037 deploy in flight. Confirm `tasks status T037` shows `schema_migrated` after the daemon poll completes; if it stalls at `accepted`, run `agents run --once` again.
- File the unfiled T2-resume-with-null-plan observation. Suggested shape: "tasks resume on non-T1 rows with plan=null cascades through ready→executing where the executor can't proceed; resume should detect plan-required-but-missing and route to planning instead."
- Restore/keep `drive.max_parallel: 5` in `.stores/config.yaml` (was 3, bumped today).
- The `auto-drive` subscriber in `.stores/agents.yaml` — confirm it's NOT commented out (I disabled then restored it during the Pi-runner pivot; should be live now).
- Engine-health.md needs a refresh at session end: move L116/L117/L123/L038/L093/L049 to ✅; add L107/L121/L122/L123/L124/L125/L126 to the relevant Layers; update the "Highest-leverage next picks" rank given today's shifts.
- Confirm whether `/codex:review` is a slash command from the freshly-installed codex plugin or whether the right invocation is `codex:rescue` with a review-prompt. The user said `/codex:review` — it should resolve via Skill, but I didn't get to test it.
- Pi runner is on main but the user's openai-codex quota is exhausted (L121 demonstrated). For now operate purely on claude-code. Don't try to restart Pi-runner dogfood until quota replenishes AND L121 (timeout/liveness) is fixed.
