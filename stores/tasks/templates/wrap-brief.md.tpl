{{!-- STUB — Phase 4 owns the full brief template. This file exists for Phase 1 happy-path test
     (drive's briefing_templates lookup needs a 'wrap' entry). DO NOT use as production template. --}}
# Wrap Brief — {{display_id}}: {{title}}

> **Mode:** wrap — synthesise the completed task for human GO/NO_GO review.

## Promise (Contract)

{{#if contract.executive_intent}}**Executive Intent:** {{contract.executive_intent}}{{/if}}

**Done When:** {{contract.done_when}}

**Scope In:** {{contract.scope_in}}

**Scope Out:** {{contract.scope_out}}

## Reality (Cycles)

{{#each cycles}}
- Phase {{this.phase}} / Cycle {{this.cycle}}: executor: {{this.executor.summary}} | review: {{this.review.gate}} — {{this.review.summary}}
{{/each}}

## Critical Actions

1. Read the Promise and Reality sections above.
2. Produce a concise executive summary (≤ 150 words) stating what was promised, what was delivered, and any notable gaps or surprises.
3. List deviations (plan divergences), residual risks, and recommended sanity checks.
4. Output the JSON envelope below — drive will submit it via `submit-wrap`.

## Output Envelope

```json
{
  "role": "wrap",
  "executive_summary": "<150-word summary>",
  "deviations": [],
  "residual_risks": [],
  "recommended_sanity_checks": []
}
```
