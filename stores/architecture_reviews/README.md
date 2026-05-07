# architecture_reviews store

Dedicated typed store for Heart/Architect phase α rulings. Rows use the `A###` display-id namespace and render to `architecture-reviews/A###/main.md`.

## Lifecycle

```text
pending --claim-review--> in_review --issue-verdict kind=interpret--> verdict_issued
pending --claim-review--> in_review --issue-verdict kind=amend--> awaiting_human_ratification --ratify-amend--> verdict_issued
pending|in_review|awaiting_human_ratification --withdraw--> withdrawn
pending|in_review|awaiting_human_ratification|verdict_issued --supersede--> superseded
```

## Verdicts and authority

- `kind=interpret`: `issue-verdict` is `actor: ai_with_human` and moves `in_review -> verdict_issued`.
- `kind=amend`: `issue-verdict` is `actor: ai_with_human`, requires `verdict=propose_doctrine_update` plus well-formed `cascade_decisions`, and moves to `awaiting_human_ratification`.
- `ratify-amend`: `actor: human`; requires a valid tier-A `--approve-token`; rejects `ai_autonomous` and `ai_with_human` even with a valid token.

Typed verdict outcomes:

```text
allow_local_fix
reframe_contract
merge_with_cluster
create_primitive_task
block_pending_fixes
propose_doctrine_update
request_human_arch_decision
```

## CLI examples

```bash
stores architecture-reviews add \
  --kind interpret \
  --summary "dispatch cluster coherence" \
  --source-observation L123 \
  --cluster-key dispatch-lifecycle \
  --invoker ai_with_human

stores architecture-reviews claim-review A001 --invoker ai_with_human

stores architecture-reviews issue-verdict A001 \
  --kind interpret \
  --verdict allow_local_fix \
  --rationale "Reviewed against architecture-coherence doctrine." \
  --invoker ai_with_human

stores architecture-reviews add \
  --kind amend \
  --summary "amend doctrine wording" \
  --cascade-decisions '[{"target":"docs/heart-and-architect.md","decision":"update"}]' \
  --invoker ai_with_human

stores architecture-reviews claim-review A002 --invoker ai_with_human
stores architecture-reviews issue-verdict A002 \
  --kind amend \
  --verdict propose_doctrine_update \
  --rationale "Doctrine needs to move." \
  --cascade-decisions '[{"target":"docs/heart-and-architect.md","decision":"update"}]' \
  --invoker ai_with_human
stores architecture-reviews ratify-amend A002 \
  --invoker human \
  --approve-token "$STORES_APPROVE_TOKEN"
```

Use `--supersedes A###` on a new ruling or `stores architecture-reviews supersede A### --invoker ai_with_human` to move a prior ruling to the `superseded` terminal.
