# {{display_id}}: {{title}}

## Meta
- **Status:** {{status}}
- **Created:** {{created_at}}
- **Last Updated:** {{updated_at}}
- **Current Phase:** {{current_phase}}
- **Current Cycle:** {{current_cycle}}
{{#if blocked_reason}}- **Blocked Reason:** {{blocked_reason}}
{{else}}- **Blocked Reason:** —
{{/if}}{{#if branch}}- **Branch:** {{branch}}
{{/if}}{{#if capability}}- **Capability:** {{capability}}
{{/if}}

## Task

{{#if contract.executive_intent}}{{contract.executive_intent}}

{{/if}}---

## Plan

### Objective
{{#if plan.objective}}{{plan.objective}}{{else}}_No objective set._{{/if}}

### Scope
- **In:** {{#if contract.scope_in}}{{contract.scope_in}}{{else}}—{{/if}}
- **Out:** {{#if contract.scope_out}}{{contract.scope_out}}{{else}}—{{/if}}

### Done When
{{#if contract.done_when}}{{contract.done_when}}{{else}}—{{/if}}

{{#if contract.assumptions}}
### Assumptions
{{contract.assumptions}}

{{/if}}### Phases

{{#each plan.phases}}
#### Phase {{add @index 1}}: {{this.name}}
- **Objective:** {{this.objective}}
- **Tasks:**
{{#each this.tasks}}
  - {{this}}
{{/each}}
- **Acceptance Criteria:**
{{#each this.acceptance_criteria}}
  - [ ] {{this}}
{{/each}}
{{#if this.files}}- **Files:** {{#each this.files}}`{{this}}`{{#unless @last}}, {{/unless}}{{/each}}
{{/if}}{{#if this.dependencies}}- **Dependencies:** {{#each this.dependencies}}{{this}}{{#unless @last}}, {{/unless}}{{/each}}
{{/if}}
{{/each}}{{#unless plan.phases}}_Plan not yet submitted._
{{/unless}}

---

## Plan Review

{{#if plan_review_log}}
{{#each plan_review_log}}
### Review {{add @index 1}}
- **Gate:** {{this.gate}}
- **Summary:** {{#if this.summary}}{{this.summary}}{{else}}—{{/if}}
{{#if this.open_questions}}- **Open Questions:**
{{#each this.open_questions}}
  - {{this}}
{{/each}}
{{/if}}{{#if this.at}}- **At:** {{this.at}}
{{/if}}
{{/each}}
{{else}}
_No plan reviews yet._

{{/if}}

---

## Execution Log

{{#each cycles}}
### Phase {{this.phase}} / Cycle {{this.cycle}}
- **Status:** {{#if this.review}}{{this.review.gate}}{{else}}Submitted — awaiting review{{/if}}
- **Summary:** {{#if this.executor.summary}}{{this.executor.summary}}{{else}}—{{/if}}
{{#if this.executor.commit}}- **Commit:** `{{this.executor.commit}}`
{{/if}}{{#if this.executor.files_changed}}- **Files:**
{{#each this.executor.files_changed}}
  - `{{this}}`
{{/each}}
{{/if}}{{#if this.executor.notes}}- **Notes:** {{this.executor.notes}}
{{/if}}{{#if this.executor.at}}- **At:** {{this.executor.at}}
{{/if}}
{{/each}}{{#unless cycles}}_No execution cycles yet._
{{/unless}}

---

## Code Review Log

{{#each cycles}}{{#if this.review}}
### Phase {{this.phase}} / Cycle {{this.cycle}}
- **Gate:** {{this.review.gate}}
- **Summary:** {{#if this.review.summary}}{{this.review.summary}}{{else}}—{{/if}}
- **Findings:** {{#if this.review.critical}}{{this.review.critical}}{{else}}0{{/if}} critical, {{#if this.review.major}}{{this.review.major}}{{else}}0{{/if}} major, {{#if this.review.minor}}{{this.review.minor}}{{else}}0{{/if}} minor
{{#if this.review.details}}
**Details:**
{{this.review.details}}
{{/if}}{{#if this.review.at}}- **At:** {{this.review.at}}
{{/if}}
{{/if}}{{/each}}{{#unless cycles_have_reviews}}_No code reviews yet._
{{/unless}}

---

## Completion
{{#if (eq status "accepted")}}- **Accepted:** {{updated_at}}
- **Branch:** {{#if branch}}{{branch}}{{else}}—{{/if}}
{{else}}{{#if (eq status "rejected")}}- **Rejected:** {{updated_at}}
- **Branch:** {{#if branch}}{{branch}}{{else}}—{{/if}}
{{else}}{{#if (eq status "in_review")}}- **In Review:** {{updated_at}} — awaiting human GO/NO_GO
{{else}}_Not yet complete._
{{/if}}{{/if}}{{/if}}
