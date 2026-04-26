# Skill walkthrough — 2026-04-26

Setup: fresh `mktemp -d`, `git init`, `stores init`, installed `observations` + `gate` stores, then `stores skills install --all`. Test repo: `/tmp/tmp.Qe7by8BUOL`. Walked each skill literally, executing every command it instructs.

## Summary

- 4 skills walked, ~17 distinct gaps found.
- **Worst-affected:** `task:next` — virtually every command in it is wrong (already self-flags as forward-looking, but its fallback path also fails).
- **Best-affected:** `gate:walk` — core flow (answer + actor enforcement) actually works; only `defer` verb and discovery commands are broken.
- **Headline finding:** Every skill points at two discovery commands that do not exist: `stores <store> schema --json` and `stores <store> list --status <x>`. The skills lean hard on "discover via the CLI"; the CLI does not implement the discovery surface they describe. There is no `schema` subcommand on any store, and `list` accepts no filter flags whatsoever (no `--status`, `--limit`, `--sort`, `--has-contract`, etc.).

## Per-skill findings

### observation:log

**What worked:**
- `stores observations --help` prints verbs as expected.
- The minimal `add --summary "..." --invoker ai_with_human` does work and returns a display_id.
- The store accepts the entry; `show` round-trips.

**Gaps / breaks:**
- **`stores observations schema --json`**: skill instructs running this once per session. Subcommand does not exist (`error: unrecognized subcommand 'schema'`). All "the schema tells you what fields add accepts" guidance is dead.
- **Flag invention**: skill's example block lists `--priority`, `--contact-id`, `--field-name`, `--source`, `--note`. **None of these exist.** Real `add` takes: `--summary`, `--body`, `--verdict`, `--notes`, `--done-when`, `--scope-in`, `--scope-out`, `--tags`. A literal executor copying the skill's example will be hit with "unexpected argument" five different ways.
- **Display ID format**: skill says "CLI returns the new display_id (e.g. `L042`)". Reality: `OBS001`. Cosmetic but misleading.
- **Stdin example syntax**: `stores observations add --summary - <<< "$(cat <<'TXT'…)"` doesn't match the actual flag — real flag is `--summary-from-file -` (a separate flag, not `--summary -`). Will fail.

**Score:** Needs Rewrite

### observation:triage

**What worked:**
- T3 contract enforcement works exactly as described: omitting `--done-when` / `--scope-in` / `--scope-out` produces clear `required (because triage.verdict == 'T3')` errors. This is the skill's central claim and it holds.
- The `triage <id>` verb exists with all four contract flags.

**Gaps / breaks:**
- **Wrong invoker**: skill (and frontmatter `default_invoker: ai_with_human`) tells the executor to pass `--invoker ai_with_human`. Reality: the `triage` transition rejects `ai_with_human` and **requires `--invoker human`**. So the skill's literal example fails, even with all contract fields filled. Only `--invoker human` succeeds.
- The error message printed when invoker is wrong is itself confusing: `transition 'triage' requires actor 'ai_with_human'; invoker is 'ai_with_human'` — same value on both sides of the inequality. Looks like a bug in the error formatter, not the skill, but a literal AI executor would loop on this.
- **`stores observations schema --json`**: not a real subcommand (same as above).
- **`stores gate schema --json`**: not a real subcommand.
- **`list --status open --limit 1`**: list takes no filter flags. The "pick + lock" step's literal command fails on `--status open`. There is no way to filter to open observations from the CLI; you'd have to dump everything as JSON and grep.
- **Lock**: skill admits no lock verb exists ("backlog"). Confirmed — fine, just noting.

**Score:** C (the central T3 enforcement claim is true; everything around it is wrong, especially the actor)

### gate:walk

**What worked:**
- `stores gate --help` lists verbs.
- `stores gate add` works for both `--invoker human` and `--invoker ai_autonomous` (so the per-field actor change in T2A is real — non-`answer` writes accept any invoker).
- `stores gate answer G001 --answer ... --invoker human` succeeds and transitions pending → answered.
- Without `--invoker human` and with `CLAUDECODE=1` set, `answer` rejects with the exact actor-mismatch error the skill promises ("requires actor 'human'; invoker is 'ai_autonomous' (auto-detected from $CLAUDECODE; pass --invoker human to override)"). This is a clean, correct flow.

**Gaps / breaks:**
- **`stores gate defer` does not exist.** Skill literally lists it as one of the three step-4 actions. Running `stores gate defer G002 --until ... --reason ... --invoker human` returns `error: unrecognized subcommand 'defer'`. There's no way to defer a gate item.
- **`stores gate cancel`**: this one does exist (skill correct).
- **`stores gate schema --json`**: doesn't exist.
- **`stores gate list --status pending --json`**: `--status` flag doesn't exist; bare `list` works but returns everything.

**Score:** B (core actor-enforcement story works; missing `defer` verb and broken discovery)

### task:next

**What worked:**
- The skill openly flags itself as forward-looking and notes the `tasks` store isn't shipped yet. That self-awareness is fair.

**Gaps / breaks:**
- **`stores tasks` doesn't exist** — confirmed, but skill says so.
- **The fallback path is also broken**: `stores observations list --status triaged --has-contract --sort priority_rank --json --limit 1` — every single one of those flags is invented. `--status`, `--has-contract`, `--sort`, `--limit` all rejected. The fallback to `observations` is the operative path today and it doesn't work.
- **`update --status EXECUTING_PHASE_1`**: not tested (no tasks store), but `observations update` doesn't have arbitrary status transitions; the observations FSM is `open → triaged → resolved/wont_fix`.
- **`--execution-log-phase-1-status COMPLETE` etc.**: invented flag names. No such fields on observations.
- The skill references calling `ask_user`-style prompts as a thing to avoid, but mentions they're available; for an `ai_autonomous` skill with no human in loop, the `ask_user` reference is just confusing.

**Score:** Needs Rewrite (or deletion until tasks store ships; the fallback prose is fictional)

## Cross-skill flow findings

Walked the pipeline: `observation:log` files → `observation:triage` triages T3 with contract → would `task:next` pick it up?

- **log → triage handoff**: works *data-wise* — the observation is written, the contract column is empty, ready for triage to populate. But: log skill's frontmatter and example use `ai_with_human`; triage's `triage` transition rejects that and requires `human`. So the second skill's command fails as written. A literal pipeline crashes here.
- **triage → task:next handoff**: triage successfully sets `status=triaged` with a populated `contract`. But `task:next`'s pick query (`list --status triaged --has-contract --sort priority_rank`) fails — those flags don't exist. So even though the data shape is correct, the next skill cannot find the row it should pick up. The data handoff is fine; the discovery handoff is broken.
- No "priority" or "priority_rank" exists anywhere in the observations schema. Multiple skills assume priority ordering that the schema does not encode.

## Pattern observations

1. **Phantom `schema` subcommand.** All four skills assume `stores <store> schema --json`. None have it. If the framework intends this as the canonical discovery surface, it needs to be implemented; if not, all skills need to drop the reference.
2. **Phantom `list` filter flags.** Every skill assumes `list` accepts `--status`, `--limit`, plus various subject-specific filters (`--has-contract`, `--sort`). `list` accepts none of these. This blocks every "pick the next one" pattern.
3. **Invoker drift between skill frontmatter and CLI reality.** `observation:log` says `default_invoker: ai_with_human` and triage's example uses the same — but the `triage` transition requires `human`. The skills' default-invoker hints contradict the underlying schema actor enforcement on transitions.
4. **Invented flag clusters.** Each skill has a block of example commands with a half-dozen flags that the real CLI doesn't expose (`--priority`, `--contact-id`, `--field-name`, `--source`, `--note`, `--has-contract`, `--sort`, `--limit`, `--execution-log-phase-1-*`, `--until`, `--reason`). They look plausible but none are wired in.
5. **One bright spot**: actor-mismatch enforcement on `gate.answer` (the only verb tested with strict actor) does work, with a clear error message that even hints at the fix. That pattern, extended to other transitions, is the right shape — but error wording on the `triage` transition (`requires actor X; invoker is X`) seems to misformat.

## Recommended skill rewrites

- **observation:log**: drop the `schema --json` discovery line and the invented flag block; rewrite the action example using the real `add` flags (`--summary`, `--body`, `--verdict`, `--tags`, `--invoker`); fix the display_id example to `OBS###`; either delete the stdin block or rewrite to use `--summary-from-file -`.
- **observation:triage**: change the `--invoker` value in the T3 example from `ai_with_human` to `human` (and update frontmatter); drop `schema --json`; replace the "pick" query with a real filter mechanism (or `list --json | jq` until filters land); fix the gate-add example's `--invoker` similarly if needed.
- **gate:walk**: drop `gate schema --json`; drop `list --status pending` (or wait for filter flags); remove the `defer` verb entirely (or implement it); leave the `answer` flow as-is — it's the one part that works.
- **task:next**: either delete pending the `tasks` store, or rewrite the fallback to use only flags `observations list` actually accepts (currently: `--json` and `--invoker`, period); drop all invented `--execution-log-phase-N-*` flags; drop `--status`, `--sort`, `--has-contract`, `--limit`.

Across all skills: stop pointing at `schema --json` until the framework ships it, or implement that subcommand and let the skills' "discover the surface" pattern actually work — that pattern is good, but its anchor command doesn't exist.
