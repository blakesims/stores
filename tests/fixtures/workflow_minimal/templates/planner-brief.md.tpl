# Planner Briefing — {{title}}

**Status:** {{status}}
**Phase:** {{current_phase}}

## Objective

{{description}}

## Prior Cycles

{{#each cycles}}- Phase {{this.phase}}: {{this.summary}}
{{/each}}
## Blocked Reason

{{#if (eq status "BLOCKED")}}This task is blocked. Human input required.{{else}}Not blocked.{{/if}}

## Instructions

You are the planner. Create an implementation plan.
