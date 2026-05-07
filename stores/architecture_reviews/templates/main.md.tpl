# {{display_id}}: {{summary}}

## Meta
- **Status:** {{status}}
- **Kind:** {{kind}}
- **Created:** {{created_at}}
- **Last Updated:** {{updated_at}}
{{#if verdict_issued_at}}- **Verdict Issued At:** {{verdict_issued_at}}
{{/if}}{{#if ratified_at}}- **Ratified At:** {{ratified_at}}
{{/if}}{{#if ratified_by}}- **Ratified By:** {{ratified_by}}
{{/if}}
## Source
- **Observation:** {{#if source_observation}}{{source_observation}}{{else}}—{{/if}}
- **Intake:** {{#if source_intake}}{{source_intake}}{{else}}—{{/if}}
- **Cluster:** {{#if cluster_key}}{{cluster_key}}{{else}}—{{/if}}

## Ruling
- **Verdict:** {{#if verdict}}{{verdict}}{{else}}—{{/if}}
- **Supersedes:** {{#if supersedes}}{{supersedes}}{{else}}—{{/if}}
- **Merge Target:** {{#if merge_target_id}}{{merge_target_id}}{{else}}—{{/if}}
- **Reframe Acknowledged Against:** {{#if reframe_acknowledged_against}}{{reframe_acknowledged_against}}{{else}}—{{/if}}

{{#if rationale}}
## Rationale
{{rationale}}
{{else}}
## Rationale
—
{{/if}}

## Cascade Decisions
{{#each cascade_decisions}}
- **Target:** {{this.target}}
  - **Decision:** {{this.decision}}
  - **Rationale:** {{#if this.rationale}}{{this.rationale}}{{else}}—{{/if}}
{{else}}
—
{{/each}}

## Doctrine References
{{#each doctrine_refs}}
- {{this}}
{{else}}
—
{{/each}}
