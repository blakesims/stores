# Pi Handoff Senior Architect Runner Gatekeeper

**Date:** 2026-05-06
**Type:** note

## Summary

Handoff for the next Pi agent. This Pi session acted as senior architectural reviewer/coordinator for the `stores` substrate while Claude Code subagents handled local engine fixes. Main contributions: architectural observability/gatekeeper doctrine, loops-vs-forks / Router primitive, gatekeeper phased rollout, review of L142/L143/L134/L133, and per-role runner config work to load-balance Claude Code vs Pi.

## Details

### Coordination thread

Shared agent-comm thread with the Claude Code substrate agent:

`/home/blake/repos/.agent-comm/threads/2026-05-06-01-stores-thread.md`

Use the `agent-comm` skill if continuing coordination. The prior substrate-agent wound down and future Claude Code agents may rejoin under the same name. Important messages in that thread:

- Pi filed L138 and T045 became the gatekeeper design task.
- Pi supplied design artifact `docs/worklog/2026-05-06/07-gatekeeper-design.md`.
- Substrate-agent shipped T045 design docs and used Pi as architecture reviewer.
- Pi reviewed and amended L142/L143/L134/L133 design directions.
- Pi later implemented per-role runner config after Claude Code usage limits became the blocker.

### Architecture documents / worklog references

Core docs changed or produced today:

- `docs/philosophy.md` — added **Loops vs forks: when a state is not enough**.
- `docs/primitives.md` — added missing primitive **Router** and loop/fork composition rule.
- `docs/engine-health.md` — refreshed snapshot at commit `b1b3fc9`; picture now says runtime mostly works, next risk is typed control-plane observability.
- `docs/architecture-coherence.md` — T045 design doc: local correctness is not architectural coherence.
- `docs/gatekeeper-design.md` — T045 design doc: intake_items/gatekeeper lifecycle and routing.
- `docs/risk-and-cluster-taxonomy.md` — T045 taxonomy: risk_flags, cluster_key, and `(size_tier, risk_class, approval_policy)` triple.

Important worklog notes from this Pi session:

- `docs/worklog/2026-05-06/04-architecture-oversight-findings.md`
- `docs/worklog/2026-05-06/05-deep-architecture-checks.md`
- `docs/worklog/2026-05-06/06-gatekeeper-architecture-observability.md`
- `docs/worklog/2026-05-06/07-gatekeeper-design.md`
- `docs/worklog/2026-05-06/09-pi-handoff-senior-architect-runner-gatekeeper.md` (this note)

### Senior architecture stance established

Key doctrine decisions made today:

1. **Local correctness is not architectural coherence.** Local agents file local pain; repeated local fixes can drift the architecture unless a global/gatekeeper layer clusters and risk-classifies them.
2. **New store vs states rule.** Loop inside a buffer when the row remains the same semantic object on the same journey; route across buffers when classification changes identity/schema/terminal family. This justified a new `intake_items` store over observation tags.
3. **Router primitive.** Gatekeeper/intake is an active classification point, not just metadata.
4. **Tier is not risk.** `size_tier`, `risk_class`, and `approval_policy` are orthogonal.
5. **Gatekeeper rollout must be phased.** Do not let L142 become a monolith.

### Gatekeeper phased rollout filed as substrate observations

Blake approved this sequence; Pi communicated it to the substrate-agent and filed observations:

- `L154` — phase boundaries so L142 proves only the Router seam.
- `L155` — dedicated `architecture_reviews` store is Phase 3 after tagged stand-in proves insufficient.
- `L156` — fast-track execution waits for L135 Check primitive / deterministic audit.
- `L157` — cluster-key registry and watch observability are Phase 5.

Approved phases:

1. Phase 1 / narrowed L142: `intake_items` Router seam only. Preserve direct `observations add`. Fast-track classification only; no execution/auto-close. Use tagged observation stand-in for architecture-review candidates.
2. Phase 2 / L143: observations risk metadata with canonical enums and conservative defaults.
3. Phase 3: dedicated `architecture_reviews` store.
4. Phase 4: fast-track execution only after Check primitive.
5. Phase 5: cluster registry + watch/observability.

### Reviews / recommendations already given

- **L142**: do not ratify as originally broad. Narrow to Router seam. Cut direct-observation removal, dedicated architecture_reviews store, fast-track execution.
- **L143**: enum mismatch caught. Must use canonical `risk_class ∈ {low, normal, architecture, security, authority}` and `approval_policy ∈ {auto, human, architecture}`. JSON array for `risk_flags` is OK for P1; remove SQL-membership-without-parsing acceptance. Defaults: `normal`, `human`, `[]`, `cluster_key=NULL`.
- **L134**: Path A accepted: type existing `dispatch_locks` buffer first, defer split into `dispatch_attempts`. Use `postcondition_id + postcondition_args`, closed `terminal_reason` enum, `claim_source=legacy`, `next_retry_at` not stored computed bool.
- **L133**: Path B accepted: synthesize canonical one-phase plan during T1 skip-plan; preserve provenance (`contract_synthesized`), do not add `execution_shape` branch axis.

### Per-role runner/model config changes

User asked to load-balance away from Claude Code while Claude Code subscription is rate-limited until ~18:00 local. Desired split:

- planner: Claude Code Opus
- executor: Claude Code Sonnet
- plan_reviewer/code_reviewer/wrap: Pi

Because Claude Code is currently unavailable, Pi tested all-Pi mode.

Code shipped directly on main (user explicitly gave server to Pi):

- `b03ee6b T055: add per-role runner config`
- `bff3c34 merge: per-role runner config`

Changed files:

- `Cargo.toml` — default features now include `runner-pi` as well as `runner-claude-code`.
- `src/flow/config.rs` — parse `drive.default_runner` and `drive.roles.<role>.runner/model` from `.stores/config.yaml`.
- `src/cli/dynamic.rs` / `src/cli/dispatch.rs` — added `--claude-code-model <model>`.
- `src/handlers/drive.rs` — runner selected per role at spawn time when no CLI all-role override is passed; `--pi` / `--claude-code` still force all roles.
- `src/flow/builtins/auto_drive.rs` — auto-drive now omits hardcoded `--claude-code` when drive runner config exists, so config can apply. It falls back to old `--claude-code` if no config.

Installed and restarted daemon:

```bash
cargo install --path . --features runner-claude-code,runner-pi
kill <stores-daemon-pid>
stores agents run --detach --invoker ai_autonomous --log-file /home/blake/repos/experiments/stores/logs/agents-daemon.log
```

`.stores/config.yaml` was edited (untracked operational config) to all-Pi during test:

```yaml
drive:
  max_parallel: 5
  default_runner: pi
  roles:
    planner: { runner: pi }
    plan_reviewer: { runner: pi }
    executor: { runner: pi }
    code_reviewer: { runner: pi }
    wrap: { runner: pi }
```

Original config was backed up at `/tmp/stores-config-before-pi-test.yaml`. Restore when done if desired.

Tests run after merge:

```bash
cargo test -q --lib --features runner-claude-code,runner-pi
cargo test -q --test flow_promote_scaffold_drive_e2e --features runner-claude-code,runner-pi
```

Both passed on second run. One transient Pi unit test failed once, then passed when rerun.

### Pi all-runner smoke attempt

Task chosen: `T034` (Pi runner E2E smoke test), already existed and was blocked. Pi resumed it:

```bash
stores tasks resume T034 --invoker ai_with_human --json
```

Then manually drove with no runner flags so config would choose Pi:

```bash
stores tasks drive T034 --max-iters 1 --invoker ai_autonomous 2>&1 | tee logs/t034-all-pi-drive.log
```

Observed:

1. Executor spawned via Pi:
   `spawning executor via pi runner`
2. Executor returned success after ~73s.
3. Drive then errored:
   `cannot submit-execute: row is in state 'code_review', expected 'executing'`
   This suggests a concurrent drive/daemon transition raced the manual drive, or the row was already advanced by another process. Status became `code_review`.
4. Ran one more iteration:
   `spawning code_reviewer via pi runner`
   Code reviewer returned success after ~117s and submitted PASS.
5. Status later became blocked again:
   `drive_failed:silent_zombie_pid_dead`.

Current `T034` status at handoff: blocked. Do not keep pushing without understanding concurrency / watchdog behavior.

Logs: `logs/t034-all-pi-drive.log`.
Recent Pi transcripts in `.stores/runs/` include large JSONLs around 17:16 local; inspect newest files for `final_output` if continuing smoke proof.

### Current task status snapshot at time of handoff

From DB shortly before note:

- `T034` — blocked, Pi smoke; partially proved Pi executor + code_reviewer spawn, but race/watchdog blocked it again.
- `T050` — blocked, L134 typed dispatch_locks lifecycle.
- `T052` — blocked, L143 risk metadata.
- `T053` — blocked, L142 intake/gatekeeper; should remain blocked until L143 dependency or amendments resolve.
- `T054` — blocked, L133 T1 synthesized plan.
- `T055` — blocked in substrate ledger, but code shipped out-of-band on main (`bff3c34`). Attempt to close-out-of-band requires tier-A human/token:
  `stores tasks close-out-of-band T055 --commit bff3c34 --invoker human`
  or with approval token via `ai_with_human`.

### Git / workspace warnings

- Main has many unrelated dirty/untracked generated task projection files and unrelated template edits. Do not `git add -A`.
- `.stores/config.yaml` and `.stores/agents.yaml` are untracked operational config. They were edited during runner rollout/test. Preserve intentionally.
- A separate spike worktree exists: `/home/blake/repos/experiments/stores-pi-per-role-runner-config-spike`, branch `pi/per-role-runner-config-spike`. Main already merged the work; this can be left or removed later.
- T055 worktree exists: `/home/blake/repos/experiments/stores-T055-per-role-runner-config-phase-a`. It may contain task-branch state from earlier seed. Be careful; main has shipped the code separately.

## Follow-ups

1. **Do not continue T034 blindly.** First inspect for concurrent drive/watchdog state and recent `.stores/runs/*.jsonl` final_output events. The proof partially succeeded but row ended blocked.
2. **Close T055 out-of-band** when Blake/human token is available: `stores tasks close-out-of-band T055 --commit bff3c34 ...`.
3. **Restore or decide `.stores/config.yaml` runner split.** Currently set all roles to Pi for testing. Desired eventual mixed config after Claude Code limits lift: planner=claude-code opus, executor=claude-code sonnet, plan_reviewer/code_reviewer/wrap=pi.
4. **Continue architecture reviewer role** for L142/L143/L134/L133 if substrate-agent returns. Senior stance: keep P1s narrow, prevent monoliths, favor typed control-plane observability.
5. **Watch for auto-drive config behavior.** After T050/L134 settles, ensure auto-drive config + dispatch_locks typed lifecycle do not conflict. The new auto_drive behavior deliberately falls back to `--claude-code` only when no drive runner config exists.
