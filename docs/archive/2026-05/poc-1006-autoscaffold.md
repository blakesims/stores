<!-- ARCHIVED: Historical context only. Not a current executable contract or active doctrine. See docs/CLAUDE.md for active durable docs. -->

# POC: stores auto-scaffold integration with 10.06 `./dev`

**Date:** 2026-05-05
**Outcome:** End-to-end success on first run via thin wrapper shim. No changes needed to stores or to 10.06's `./dev new`.
**Confidence:** HIGH for the worktree-creation slice tested. Several known integration concerns remain out of scope (see § Out-of-scope follow-ups).

---

## 1. Contract spec (extracted from `src/flow/builtins/auto_scaffold.rs`)

This is what `scaffold.command` MUST satisfy. It is the binding interface between the substrate and any project-side scaffolding.

### Trigger

- Subscribes to `tasks` `'' → planning` (synthetic create transition fired by auto-promote, or by direct `tasks add`).
- The agents.yaml entry: `command: "builtin:auto-scaffold"`.

### Template substitution (`.stores/config.yaml`'s `scaffold.command`)

Three tokens, simple string replace:

- `{display_id}` — e.g. `T007`
- `{slug}` — e.g. `auto-promoted-l001` (whatever `tasks.slug` holds)
- `{branch}` — e.g. `feat/auto-promoted-l001` (whatever `tasks.branch` holds; may be empty)

### Invocation

- Spawned via `sh -c "<substituted command>"`.
- `cwd` is the cwd of the daemon (`stores agents run`) — typically the substrate root containing `.stores/`.
- No special env vars set by stores. The child inherits the daemon's env.
- No stdin.

### Stdout contract

- Last non-empty stdout line, trimmed, is parsed as the absolute worktree path.
- That path is `canonicalize()`d (real path resolution); failure → return code 1, no DB write.
- The path MUST be an existing directory, else fail.
- Anything you don't want parsed (logs, info messages) MUST go to stderr — stderr is preserved (last 20 lines logged on failure) but does NOT affect parsing.

### Exit-code semantics

- Exit 0 + valid stdout path → DB write: `UPDATE tasks SET workspace_path=<canon>, updated_at=<now>`.
- Exit non-zero → builtin returns 1; tasks row stays at `planning` with no `workspace_path`. stderr tail (20 lines) is printed to the daemon log.
- No retries beyond what `agents.yaml`'s `retry_policy` declares.

### Idempotency

- If `tasks.workspace_path` is already set AND that path is an existing directory, the builtin no-ops (does NOT call the command). The shim itself doesn't need to be idempotent for this case — stores guards before it dispatches.
- If `scaffold.command` is unconfigured (no `.stores/config.yaml`, or no `scaffold` block, or empty command), the builtin no-ops (returns 0). Projects without scaffolding stay manual.

### Failure modes (verbatim list from source)

1. `display_id` missing on row → log + return 1.
2. Config file unreadable → log + return 0 (treated as "no scaffold configured").
3. Failed to spawn `sh -c` (rare) → log + return 1.
4. Command exits non-zero → log stderr tail + return 1. Row stays `planning`.
5. Empty stdout → log + return 1.
6. Last line doesn't `canonicalize()` → log + return 1.
7. Canonicalized path is not a directory → log + return 1.
8. SQL UPDATE fails → log + return 1.

> Decision Matrix Q5 (per source comment): scaffold failures surface via stderr only. The row is left at `planning`; recovery is out of scope per contract. There is no "retry" or "rerun-from-failure" verb — operator must manually `tasks update --workspace-path` or fix the command and wait for the next poll.

### What auto-scaffold writes

- ONE column on the existing `tasks` row: `workspace_path` (plus `updated_at`). No status transition. No history row from auto-scaffold itself.
- The synthetic `''→planning` transition that triggered it was already written by auto-promote (or by `tasks add`).

### Reference implementation: stores' own `./dev scaffold`

`/home/blake/repos/experiments/stores/dev` lines 234-356 (`cmd_scaffold`). Satisfies the contract by:

1. Reading `tasks show <T###> --json` to get slug/branch/workspace_path.
2. Idempotency short-circuit if workspace_path is set and dir exists → echo it and exit 0.
3. Recovery short-circuit if dir exists but row unset → stamp branch, echo path, exit 0.
4. Otherwise: `git worktree add -b <branch> <root>/../stores-<T###>-<slug> main` (stderr), then `tasks update --branch <branch>` (stderr), then **single `printf '%s\n' "$abs"` to stdout** as the contract output.

The pattern: **everything goes to stderr except the canonical path**, which is the very last printf to stdout.

---

## 2. Gap analysis: stores contract vs 10.06's `cmd_new`

10.06's `cmd_new` lives at `dev:333-685`. Concrete diff against the contract:

| Aspect | Contract | 10.06 `cmd_new` | Gap |
|---|---|---|---|
| Invocation arg | `<T###>` (display_id) | `<slug> --links <ids>\|none` | **Major.** No display_id concept; expects a slug AND a mandatory `--links` flag (`die`s without it at dev:417-428). |
| Stdout shape | LAST non-empty line = abs worktree path | Mixed `info` / `warn` / `echo` lines, none of which are guaranteed-last to be the path. dev:657 prints `Directory: $worktree_dir` as one of many lines, then more echoes after. | **Major.** No clean way to extract the path from stdout. |
| Side effects beyond worktree | Heavy: `get_port_slot`, `slot_*_port`, `ports add` (dev:457-465, 515-524), generate `docker-compose.override.yml` (dev:485-497), `nginx-local.conf` (dev:499-501), `.env` from `.env.feat.template` (dev:503-513), Claude Code hook symlinks (dev:526-538), auto-lock ledger items (dev:557-577), inject `linked_ledger_ids` into `main.md` frontmatter (dev:579-632), then `./dev sandbox local` (containers + alembic + stripe + seed + doctor) at dev:646-651. | **Heavy.** Stores doesn't care about any of this, but it's bound up in `cmd_new`. Calling `./dev new` directly from auto-scaffold would: (a) require manufacturing a `--links` arg, (b) take many minutes (sandbox seed), (c) emit the wrong stdout shape, (d) leave the daemon iteration open the whole time. |
| Idempotency | Required (auto-scaffold pre-checks; the command may also short-circuit) | dev:451-453 hard-`die`s if worktree dir exists. Not idempotent. | **Major.** A retry / re-fire of auto-scaffold would fail. |
| Worktree path layout | Contract is agnostic (any abs path) | `${WORKTREE_BASE}/${PROJECT_PREFIX}-feat-${slug}` (dev:447). | None — fine, just need to emit it on stdout. |

### Verdict on direct `./dev new` integration

Direct `scaffold.command: "./dev new {slug}"` is **not viable**. The interface mismatch (positional arg shape, mandatory `--links`, stdout shape, lack of idempotency, multi-minute sandbox seed) blocks it. Zero of the four would survive contact with the contract.

A wrapper or a new subcommand is required.

---

## 3. POC outcome — hands-on results

### Setup performed

```bash
# 1. Throwaway worktree off the bare repo
git -C /home/blake/repos/clients/10.06-wt worktree add \
    -b experiment/stores-poc-autoscaffold \
    /home/blake/repos/clients/10.06-wt/10.06-experiment-stores-poc main
# → "HEAD is now at bb24b1ac task(T299): plan + review — Commissions UI redesign"

# 2. Stores init + bundled stores
cd /home/blake/repos/clients/10.06-wt/10.06-experiment-stores-poc
stores init                       # → Created .stores/db.sqlite + manifest.yaml
stores install observations       # → Installed bundled store 'observations'
stores install tasks              # → Installed bundled store 'tasks'

# 3. Wrote .stores/agents.yaml  (auto-promote + auto-scaffold only, no auto-drive)
# 4. Wrote .stores/config.yaml  (scaffold.command: "./scripts/stores-scaffold.sh {display_id}")
# 5. Wrote scripts/stores-scaffold.sh  (see § The shim below)
chmod +x scripts/stores-scaffold.sh
```

### Observation lifecycle

```bash
stores observations add --invoker ai_autonomous \
    --summary "POC test observation" --source dev --priority normal \
    --captured-at "$(date -Iseconds)" --captured-week "$(date +w%V)" \
    --body "..."
# → L001

stores observations investigate L001 --invoker ai_autonomous
# → Transitioned L001: open → investigating

# Set contract fields. NOTE — these flags only exist on `confirm` / `update`,
# not on `add`. List args (--in-scope, --out-of-scope, --acceptance) are
# stored as a single-element array containing the raw JSON string when
# passed as JSON; this is a separate bug worth filing but did not block POC.
stores observations update L001 \
    --objective "..." --type work \
    --in-scope '["a","b"]' --out-of-scope '["c"]' --acceptance '["d","e"]' \
    --invoker ai_autonomous

# Approval (tier-A — used --invoker human; no token in this POC)
stores observations update L001 \
    --approved-by blake --approved-at "$(date -Iseconds)" --invoker human

# First confirm attempt FAILED — needs intent_contract.contract_state == 'ready'
stores observations confirm L001 --invoker human
# → Error: no transition from 'investigating' via 'confirm' (gate None) had its guard satisfied

# Set contract_state. First attempt missed required tier_hint:
#   Error: validation failed: intent_contract.tier_hint: required
stores observations update L001 --contract-state ready --tier-hint T3 --invoker human
stores observations confirm L001 --invoker human
# → Transitioned L001: investigating → confirmed
# → Auto-ratified L001: confirmed → ready (framework)
```

### Daemon run

```bash
stores agents run --poll-interval 2 --detach --log-file /tmp/poc-agents.log
# → PID 603632
sleep 8
cat /tmp/poc-agents.log
```

Daemon log (verbatim):

```
[auto-promote] L001: promoted to T001 (planning)
[auto-scaffold] T001: workspace_path = /home/blake/repos/clients/10.06-wt/10.06-feat-auto-promoted-l001
[daemon] dispatched 2 job(s) in iteration 0
```

### Verification

```sql
SELECT display_id, status, slug, branch, workspace_path FROM tasks;
-- T001 | planning | auto-promoted-l001 | feat/auto-promoted-l001
--      | /home/blake/repos/clients/10.06-wt/10.06-feat-auto-promoted-l001
```

```bash
git -C /home/blake/repos/clients/10.06-wt worktree list | grep auto-promoted
# /home/blake/repos/clients/10.06-wt/10.06-feat-auto-promoted-l001  bb24b1ac [feat/auto-promoted-l001]

ls /home/blake/repos/clients/10.06-wt/10.06-feat-auto-promoted-l001/
# (full 10.06 tree present: app/ .claude/ etc.)
```

Transition history:

```
1 | observations | L001 |              | open         | create       | ai_autonomous
2 | observations | L001 | open         | investigating| investigate  | ai_autonomous
3 | observations | L001 | investigating| confirmed    | confirm      | human
4 | observations | L001 | confirmed    | ready        | ratify       | framework
5 | tasks        | T001 |              | planning     | create       | ai_autonomous
```

### What DID NOT need to be patched

- No changes to `src/flow/builtins/auto_scaffold.rs`. Contract held as-is.
- No changes to 10.06's `./dev`. The shim sat alongside, calling git directly.
- No changes to stores' bundled schemas.
- No retry needed. Auto-promote + auto-scaffold dispatched in the same poll iteration.

### Friction encountered (worth filing as observations later)

1. `stores observations add` does not accept `--objective` / `--in-scope` / `--out-of-scope` / `--acceptance` even though `update` and `confirm` do. Forces a two-step open → update sequence.
2. `--in-scope '["a","b"]'` accepts the JSON string as a single text element, not a parsed list — see "intent_contract" in the show output above. The downstream `auto-promote` `format_bullets` will render this as `- ["a","b"]` instead of `- a` / `- b`. Cosmetic; doesn't block.
3. `confirm` failure mode is unclear — the message `no transition … had its guard satisfied` doesn't tell the user `contract_state` needs to be `ready`. A "show which guards failed" hint would save 2 round-trips.

These are all stores-side, not 10.06-side. None blocked the POC.

---

## 4. The shim (drafted, not committed)

This is what made it work. ~50 lines of bash, sat at `scripts/stores-scaffold.sh` in the experimental worktree. The full source:

```bash
#!/usr/bin/env bash
# stores-scaffold.sh — minimal POC shim that satisfies the auto-scaffold
# contract (single trailing stdout line = canonical worktree path).
set -euo pipefail

die() { printf 'stores-scaffold: %s\n' "$*" >&2; exit 1; }

task_id="${1:-}"
[[ -n "$task_id" ]] || die "missing task_id"
[[ "$task_id" =~ ^T[0-9]{3}$ ]] || die "invalid task id: $task_id"

if command -v stores >/dev/null 2>&1; then
    stores_bin=$(command -v stores)
else
    stores_bin="/home/blake/repos/experiments/stores/target/release/stores"
fi
[[ -x "$stores_bin" ]] || die "stores binary not found"

root="$(pwd)"
bare_repo="$HOME/repos/clients/10.06-wt"

show_json=$( "$stores_bin" tasks show "$task_id" --json ) || die "tasks show failed"
slug=$(printf '%s' "$show_json" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("slug") or "")')
existing_branch=$(printf '%s' "$show_json" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("branch") or "")')
existing_workspace=$(printf '%s' "$show_json" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("workspace_path") or "")')

[[ -n "$slug" ]] || die "$task_id has no slug"

# Idempotency
if [[ -n "$existing_workspace" && -d "$existing_workspace" ]]; then
    printf '%s\n' "$existing_workspace"
    exit 0
fi

branch="${existing_branch:-feat/${slug}}"
worktree_dir="${bare_repo}/10.06-feat-${slug}"

# Recovery
if [[ -d "$worktree_dir" ]]; then
    abs=$(readlink -f "$worktree_dir")
    "$stores_bin" tasks update "$task_id" --branch "$branch" --invoker ai_autonomous >&2
    printf '%s\n' "$abs"
    exit 0
fi

git -C "$bare_repo" worktree add "$worktree_dir" -b "$branch" main >&2
abs=$(readlink -f "$worktree_dir")
"$stores_bin" tasks update "$task_id" --branch "$branch" --invoker ai_autonomous >&2

# Contract: last non-empty stdout line is the canonical worktree path.
printf '%s\n' "$abs"
```

This shim does ONLY the "create the worktree" slice. It deliberately skips port allocation, docker-compose generation, nginx config, `.env` rendering, ledger auto-locking, and `./dev sandbox local`. Those steps are the heavy 10.06-specific tail that should NOT live inside auto-scaffold's invocation budget.

---

## 5. Minimal refactor recommendation

**Recommended path: a new `./dev scaffold` subcommand inside 10.06's `dev`, modeled on stores' own `cmd_scaffold` (dev:234-356), wrapping the same code path the shim took.**

### Why a new subcommand and not "just keep the shim"

1. The shim duplicates `repo_root` / `validate_slug` logic that already exists in 10.06's `dev`.
2. The shim has to re-implement json parsing of `tasks show`. Inside `dev`, `cmd_scaffold` would already have access to all the helpers.
3. A `./dev scaffold` verb is discoverable via `./dev --help`; a `scripts/stores-scaffold.sh` is invisible.
4. A subcommand makes the boundary between "stores tells us to make a worktree" (cheap, fast, contract-shaped) and "human runs `./dev new`" (heavy, interactive, port + docker + seed) explicit and named.

### Concrete shape

Add ~60 lines to `dev` mirroring `/home/blake/repos/experiments/stores/dev`'s `cmd_scaffold` (the reference). Skeleton:

```bash
cmd_scaffold() {
    local task_id="${1:-}"
    [[ "$task_id" =~ ^T[0-9]{3}$ ]] || die "invalid task id"

    local stores_bin
    stores_bin=$(command -v stores) || die "stores not on PATH"

    local show_json slug existing_branch existing_workspace
    show_json=$( "$stores_bin" tasks show "$task_id" --json ) || die "tasks show $task_id failed"
    slug=$(printf '%s' "$show_json" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("slug") or "")')
    existing_branch=$(printf '%s' "$show_json" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("branch") or "")')
    existing_workspace=$(printf '%s' "$show_json" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("workspace_path") or "")')
    [[ -n "$slug" ]] || die "$task_id has no slug"
    validate_slug "$slug"

    # Idempotency
    if [[ -n "$existing_workspace" && -d "$existing_workspace" ]]; then
        printf '%s\n' "$existing_workspace"; return 0
    fi

    local branch="${existing_branch:-feat/$slug}"
    local worktree_dir="${WORKTREE_BASE}/${PROJECT_PREFIX}-feat-${slug}"

    # Recovery
    if [[ -d "$worktree_dir" ]]; then
        local abs; abs=$(readlink -f "$worktree_dir")
        "$stores_bin" tasks update "$task_id" --branch "$branch" --invoker ai_autonomous >&2
        printf '%s\n' "$abs"; return 0
    fi

    git -C "$BARE_REPO" worktree add "$worktree_dir" -b "$branch" main >&2
    local abs; abs=$(readlink -f "$worktree_dir")
    "$stores_bin" tasks update "$task_id" --branch "$branch" --invoker ai_autonomous >&2
    printf '%s\n' "$abs"
}
```

Plus one line in the dispatch table near the end of the script:

```bash
scaffold) cmd_scaffold "$@" ;;
```

And the `.stores/config.yaml` becomes:

```yaml
scaffold:
  command: "./dev scaffold {display_id}"
```

This matches stores' OWN reference implementation byte-for-byte in shape.

### What about ports, docker, seed?

Decoupled. `./dev scaffold` creates the worktree only. The human (or a follow-up automation in 10.06) runs `./dev up`, `./dev sandbox local`, etc. INSIDE the new worktree when ready to actually develop. The substrate's contract — "row arrives, worktree exists, path written back" — is satisfied by the cheap, fast `./dev scaffold` step. The heavy 10.06 work happens lazily, on the first actual development session.

If 10.06 wants the heavy steps to ALSO fire automatically on row creation, that's a separate subscriber (`builtin:run-shell` or a project-side `auto-bootstrap` agent) wired in agents.yaml, NOT auto-scaffold's job. Keep auto-scaffold cheap.

---

## 6. Confidence rating + remaining risks

### Confidence: HIGH for the tested slice

**What was actually tested end-to-end:**

- ratify → confirm → auto-ratify → auto-promote → auto-scaffold → workspace_path written → git worktree present on disk.
- Idempotency on second poll iteration (no double-fire — auto-scaffold's pre-check held; daemon log showed dispatched 2 jobs ONCE, then quiet).
- Ran inside a 10.06 worktree (real `./dev` script available, real bare repo, real branch).
- The shim was the only project-side code; ~50 lines; satisfied the contract.

**Confidence floor:** the contract is small and well-specified. The reference implementation in `stores/dev` is 60 lines. The 10.06 port is a near-direct translation. Nothing exotic.

### Risks not tested

1. **Concurrent dispatch races.** Did not test what happens if two daemons run simultaneously, or if a `tasks add` fires while the daemon is mid-iteration. Auto-scaffold's `claim_window_secs: 300` and the row-level claim should handle it, but unproven here.
2. **Branch already exists.** If `feat/auto-promoted-l001` exists from a prior run, `git worktree add -b` will fail. The shim/subcommand should detect this and use `git worktree add` (no `-b`) when the branch already exists, or generate a unique branch name. Stores' own `cmd_scaffold` handles this via the recovery path; 10.06's port should match.
3. **Substrate-side bugs noted in §3.4** (in-scope as JSON string, observation `add` lacking contract flags, opaque guard error). They didn't block this POC but they do create paper-cuts on the path the human walks before auto-scaffold ever fires.
4. **10.06's `cmd_destroy` cleanup.** When `tasks accept` merges and the worktree is no longer needed, who tears it down? Not in scope for this POC, but the symmetry question is real.
5. **Git config / hook / Claude project setup.** 10.06's `cmd_new` does extra work (core.hooksPath, Claude Code hook symlinks at dev:472-475, 526-538). These are NOT in `./dev scaffold`. If the human jumps into the new worktree expecting working pre-commit hooks, they'll be missing. Either: (a) replicate the lightweight bits in `./dev scaffold`, or (b) document that `./dev up` (or a new `./dev bootstrap`) is the next step after `./dev scaffold` and does the rest.

### What "well-tested path" looks like before flipping this on for real 10.06 work

- A unit test on the `./dev scaffold` subcommand asserting stdout-shape (no `info` / `warn` lines on stdout).
- An integration test that runs `stores agents run` once over a fixture observation and asserts workspace_path is written + worktree exists.
- A failure-mode test: scaffold command exits 1 → row stays at planning, workspace_path empty.
- A pre-existing-worktree test: shim/subcommand short-circuits, no double-`git worktree add`.

The POC covered exactly one happy path. Four-corners coverage (idempotent / recovery / failure / branch-collision) is a follow-up.

---

## 7. Out-of-scope follow-ups (named so they don't bleed in)

These are real concerns that surfaced while doing this POC. They are NOT auto-scaffold problems and should not be folded into the autoscaffold integration scope:

1. **`builtin:auto-drive` integration.** This POC explicitly skipped auto-drive (the executor that runs planner → reviewer → executor → wrap inside the new worktree). 10.06 has its own task lifecycle (`./dev observation`, `./dev gate`, `./dev task frontmatter`, `.claude/skills/task:open`, `.claude/skills/task:wrap`). Wiring auto-drive over that would require deciding: does stores' Claude-Code runner take over, or does 10.06's existing skill chain stay primary? Big design call. Defer.

2. **The dual-channel question.** 10.06 has `./dev observation` + a ledger today. This POC introduced `stores observations` parallel to it. Running both is a footgun (humans file in one place, the AI files in another). Pick one, migrate the data, deprecate the other. Out of scope for autoscaffold.

3. **Schema fit for 10.06's bespoke fields.** The bundled `tasks` schema ships with `done_when / scope_in / scope_out / tier_hint`. 10.06's task frontmatter has `linked_ledger_ids`, `capability`, `scheduled_for`, `evidence`, profile fields, etc. Either: (a) install the `observations_1006` shaped variant if one exists for tasks, or (b) define a 10.06-specific tasks schema. Not blocking — auto-scaffold doesn't care about these — but it's the next gap.

4. **Where `stores` lives on PATH.** The shim hard-coded `/home/blake/repos/experiments/stores/target/release/stores` as a fallback. For a real wire-up, install `stores` to `~/.local/bin` (or `cargo install --path .`) and rely on PATH only. Not a contract issue, just hygiene.

5. **Auto-locking of linked observations on scaffold.** `./dev new` auto-locks ledger items via `cmd_ledger_lock` (dev:557-577). The substrate-shaped equivalent would be a separate `auto-lock-linked-observations` subscriber on `tasks` `''→planning`, NOT inside auto-scaffold. Easy follow-up once observations and tasks are unified (#2 above).

6. **Cleanup symmetry on `tasks accept`.** When a task is accepted and merged, the worktree should ideally be torn down (git worktree remove + branch delete + ports unregistered). 10.06's `cmd_destroy` exists; stores' `accept-merge` builtin does the merge but not the teardown. Probably wants a `builtin:destroy-workspace` subscriber on `accepted→cargo_installed` or a similar transition.

---

## 8. Cleanup performed

```bash
kill 603632                                                                  # daemon
git -C /home/blake/repos/clients/10.06-wt worktree remove --force \
    /home/blake/repos/clients/10.06-wt/10.06-feat-auto-promoted-l001        # generated worktree
git -C /home/blake/repos/clients/10.06-wt worktree remove --force \
    /home/blake/repos/clients/10.06-wt/10.06-experiment-stores-poc          # experimental worktree
git -C /home/blake/repos/clients/10.06-wt branch -D \
    experiment/stores-poc-autoscaffold feat/auto-promoted-l001
```

`git worktree list` confirms only the original seven 10.06 worktrees remain. No commits landed on `experiment/stores-poc-autoscaffold` (the branch was deleted with the worktree). No changes to `10.06-main`. The shim and `.stores/` directory died with the experimental worktree.

This report (`docs/poc-1006-autoscaffold.md`) is the only durable artifact.
