# {{title}}

## Meta

- **Status:** {{status}}
- **Display ID:** {{display_id}}
- **Phase:** {{default current_phase "—"}}
- **Cycle:** {{default current_cycle "—"}}
- **Created:** {{default created_at "—"}}
- **Updated:** {{default updated_at "—"}}
- **Blocked Reason:** {{default blocked_reason "—"}}

---

## Task

{{default description "_No description provided._"}}

---

## Plan

### Objective

{{default plan.objective "_Planner agent fills this section._"}}

### Phases

{{#each plan.phases}}- **{{this.name}}**{{#if this.objective}} — {{this.objective}}{{/if}}
{{else}}_Planner agent fills this section._
{{/each}}
---

## Plan Review

{{#each plan_review_log}}- **[{{this.gate}}]** {{default this.summary "—"}}
{{else}}_Plan-reviewer agent fills this section per phase._
{{/each}}
---

## Execution Log

{{#each cycles}}- **Phase {{this.phase}} / Cycle {{this.cycle}}:** {{#if this.executor}}{{default this.executor.summary "—"}}{{else}}—{{/if}}
{{else}}_Executor agent fills this section per phase._
{{/each}}
---

## Code Review Log

{{#each cycles}}{{#if this.review}}- **Phase {{this.phase}} / Cycle {{this.cycle}} [{{this.review.gate}}]:** {{default this.review.summary "—"}}
{{/if}}{{else}}_Code-reviewer agent fills this section per phase._
{{/each}}
---

## Completion

{{#if (eq status "complete")}}_Task complete._{{else}}_Orchestrator fills this section on completion._{{/if}}
