# Wrap Brief — {{display_id}}: {{title}}

**Status:** in_review
{{#if capability}}**Capability:** {{capability}}{{/if}}
{{#if branch}}**Branch:** {{branch}}{{/if}}

---

## Promise (contract — what we said we'd do)

{{#if contract.executive_intent}}
**Executive Intent:**
{{contract.executive_intent}}

{{/if}}
**Done When:**
{{contract.done_when}}

**Scope In:**
{{contract.scope_in}}

**Scope Out:**
{{contract.scope_out}}

{{#if contract.assumptions}}
**Assumptions:**
{{contract.assumptions}}
{{/if}}

---

## Reality (what actually happened)

| Phase | Cycle | Executor Summary | Review Gate | Review Summary |
|-------|-------|-----------------|-------------|----------------|
{{#each cycles}}| {{this.phase}} | {{this.cycle}} | {{default this.executor.summary "—"}} | {{default this.review.gate "—"}} | {{default this.review.summary "—"}} |
{{/each}}
{{#unless cycles}}_No execution cycles recorded._{{/unless}}

---

## Diff (direction-aware)

Two labeled sections below. Attribute only commits from "On this branch" to
this task. Commits listed under "On base" rode in on main and must NOT be
described as deliverables of this task.

{{{git_diff_summary}}}

---

## Your Job

Produce the JSON envelope below. Drive submits it via `compute_submit_wrap` —
do NOT call `stores tasks submit-wrap` yourself.

**Requirements:**

1. **`executive_summary`** (REQUIRED, ≤ 150 words): Concrete delta. Name the
   files/modules that changed and what they now do. Call out any REVISE cycles
   that surfaced a defect. Avoid "the work was completed as specified."

2. **`deviations[]`**: Changes that diverged from the contract scope — both
   over-deliveries (work beyond `scope_in`) and under-deliveries (items in
   `scope_in` not delivered). Empty list is correct if none.

3. **`residual_risks[]`**: Forward-looking concerns the reviewer should think
   about before approving. Empty list is correct if none.

4. **`recommended_sanity_checks[]`**: Specific commands, files, or manual
   behaviors the reviewer should verify before issuing GO. Be concrete:
   name the test command, not just "run tests."

5. **`reasoning`** (optional): your working notes — how you synthesised the
   summary. Not shown to the reviewer; useful for audit.

```json
{
  "role": "wrap",
  "reasoning": "optional working notes",
  "executive_summary": "≤150-word concrete delta summary...",
  "deviations": [],
  "residual_risks": [],
  "recommended_sanity_checks": []
}
```
