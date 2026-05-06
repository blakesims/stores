# intake_items under dogfood

The `intake_items` store is the first layer in the gatekeeper pipeline. Local agents file friction here; the gatekeeper classifies it before it becomes an observation. For the full design, see `docs/gatekeeper-design.md`.

### When to file

File via `stores intake add --invoker ai_autonomous` **whenever a local agent (planner, executor, code_reviewer, orchestrator) encounters substrate friction**. This is the primary path from P1 onward. The gatekeeper (not implemented in P1) will triage the item and route it.

`stores observations add` remains valid as a human-grounded escape hatch or for AI-with-human filings that should bypass the gatekeeper. It is NOT deprecated.

### The filing verb

```bash
stores intake add --invoker ai_autonomous \
  --summary "<one-line>" \
  --source-agent <planner|executor|code_reviewer|orchestrator|...> \
  --captured-at "$(date -Iseconds)" \
  --captured-week "w$(date +%V)-d$(date +%u)" \
  [--body-from-file <path>] \
  [--source-task T###] \
  [--suggested-fix "<brief proposal>"]
```

Required: `summary`, `source_agent`, `captured_at`, `captured_week`.

### The lifecycle (invoker discipline)

All transitions except `reopen` are `ai_autonomous`:

- `claim-triage` — gatekeeper picks up the item (draft → triaging)
- `route --decision <D>` — gatekeeper routes; D ∈ {duplicate, needs_info, fast_track, normal_observation, arch_review_candidate, reject_noise}. `arch_review_candidate` routes to `routed` and creates a tagged observation (tag `arch-review-candidate`); the dedicated `architecture_reviews` store / `escalated` lifecycle is a P3 follow-up (L171), not present in P1.
- `recon-return` — recon agent returns evidence (needs_info → triaging)

`reopen` is `ai_with_human` — reversing a `reject_noise` decision requires human presence (the human is overriding a machine judgment).

### Cross-row guards enforced at write-time

The schema enforces two cross-row invariants via `required_when`:

1. `route --decision duplicate` requires `--duplicate-of I###` (soft-FK to intake_items)
2. `route --decision normal_observation` requires `--routed-to-observation L###` (soft-FK to observations)

Attempting either route variant without the required FK fails loud with a validation error.

### Triage tier hint

The `decision_metadata` field (json) should carry `tier_hint` and `rationale` from the gatekeeper's structured output. When the gatekeeper agent ships (post-P1), it will write `gatekeeper_decision_json` with the full validated payload; `decision_metadata` is the top-level extract for human-readable context.

### Substrate-down escape

If `stores intake add` fails: write a worklog note at `docs/worklog/<date>/NN-substrate-down-<slug>.md` following the observations escape pattern. File the intake item once the substrate recovers.

### What this file does NOT contain

- CLI flag reference: `stores intake <verb> --help`
- Schema field listings: `schema.yaml`
- Full routing decision semantics: `docs/gatekeeper-design.md`
- Generic Claude orientation: `/CLAUDE.md`
