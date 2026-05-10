## Z0: cross-store soft-FKs

```mermaid
stateDiagram-v2
  state "tasks" as tasks
  state "observations" as observations
  state "gate" as gate
  tasks --> tasks : depends_on
  tasks --> observations : linked_observations
```

---

## Z1: tasks state machine

```mermaid
stateDiagram-v2
  [*] --> planning
  planning --> closed_out_of_band :  H! close-out-of-band
  plan_review --> closed_out_of_band :  H! close-out-of-band
  ready --> closed_out_of_band :  H! close-out-of-band
  executing --> closed_out_of_band :  H! close-out-of-band
  code_review --> closed_out_of_band :  H! close-out-of-band
  blocked --> closed_out_of_band :  H! close-out-of-band
  complete --> closed_out_of_band :  H! close-out-of-band
  in_review --> closed_out_of_band :  H! close-out-of-band
  rejected --> closed_out_of_band :  H! close-out-of-band
  deploy_blocked --> closed_out_of_band :  H! close-out-of-band
  integration_queued --> closed_out_of_band :  H! close-out-of-band
  integrating --> closed_out_of_band :  H! close-out-of-band
  integration_blocked --> closed_out_of_band :  H! close-out-of-band
  integrated --> closed_out_of_band :  H! close-out-of-band
  planning --> planning :  F activate-task
  planning --> planning :  F release-from-queued
  planning --> plan_review :  A submit-plan
  planning --> ready :  F skip-plan
  plan_review --> ready :  A submit-plan-review [READY]
  plan_review --> planning :  A submit-plan-review [NEEDS_WORK]
  plan_review --> blocked :  A submit-plan-review [NEEDS_WORK]
  plan_review --> blocked :  A submit-plan-review [NOT_READY]
  ready --> executing :  F start
  executing --> code_review :  A submit-execute
  code_review --> executing :  A submit-review [PASS]
  code_review --> complete :  A submit-review [PASS]
  code_review --> executing :  A submit-review [REVISE]
  code_review --> blocked :  A submit-review [REVISE]
  code_review --> blocked :  A submit-review [FAIL]
  blocked --> planning :  H+ resume
  planning --> blocked :  F mark_drive_failed
  plan_review --> blocked :  F mark_drive_failed
  ready --> blocked :  F mark_drive_failed
  executing --> blocked :  F mark_drive_failed
  code_review --> blocked :  F mark_drive_failed
  accepted --> integration_queued :  F enqueue-integration
  integration_queued --> integrating :  F start-integration
  integrating --> integrating :  F mark_refresh_done
  integrating --> integrating :  F mark_task_review_done
  integrating --> integrating :  F mark_testing_done
  integrating --> integrating :  F mark_merge_done
  integrating --> integrating :  F mark_deploy_done
  integrating --> integrated :  F mark_verify_done
  integrating --> integrated :  F mark_integrated
  integrating --> integration_blocked :  F mark_integration_blocked
  integration_blocked --> integration_queued :  H+ retry-integration
  integrated --> cargo_installed :  F mark_cargo_installed
  integrated --> deploy_blocked :  F mark_deploy_blocked
  cargo_installed --> schema_migrated :  F mark_schema_migrated
  cargo_installed --> deploy_blocked :  F mark_deploy_blocked
  deploy_blocked --> accepted :  H+ retry-deploy
  complete --> in_review :  F request_review
  in_review --> executing :  F submit-external-review [REVISE]
  in_review --> accepted :  H! accept
  in_review --> rejected :  H! reject
  rejected --> planning :  H+ amend
  planning --> abandoned :  H! abandon
  plan_review --> abandoned :  H! abandon
  ready --> abandoned :  H! abandon
  executing --> abandoned :  H! abandon
  code_review --> abandoned :  H! abandon
  blocked --> abandoned :  H! abandon
  in_review --> abandoned :  H! abandon
  deploy_blocked --> abandoned :  H! abandon
  complete --> abandoned :  H! abandon
  integration_queued --> abandoned :  H! abandon
  integrating --> abandoned :  H! abandon
  integration_blocked --> abandoned :  H! abandon
  integrated --> abandoned :  H! abandon
```

---

## Z1: observations state machine

```mermaid
stateDiagram-v2
  [*] --> open
  open --> investigating :  A investigate
  open --> needs_investigation :  H+ request-investigation
  needs_investigation --> investigating :  F investigation-started
  investigating --> investigated :  F investigation-succeeded
  investigating --> investigation_failed :  F investigation-failed
  open --> wont_fix :  H+ wont_fix
  open --> resolved :  A close_as_addressed
  ready --> resolved :  A close_as_addressed
  investigating --> confirmed :  H+ confirm
  confirmed --> ready :  F ratify
  investigating --> needs_info :  A request_info
  confirmed --> needs_info :  A park
  needs_info --> confirmed :  H! provide_info
  confirmed --> in_progress :  A claim
  in_progress --> resolved :  A resolve
  confirmed --> wont_fix :  H+ wont_fix
  ready --> wont_fix :  H+ wont_fix
  open --> resolved :  F auto_resolve
  investigating --> resolved :  F auto_resolve
  confirmed --> resolved :  F auto_resolve
  ready --> resolved :  F auto_resolve
  needs_info --> resolved :  F auto_resolve
  in_progress --> resolved :  F auto_resolve
```

---

## Z1: gate state machine

```mermaid
stateDiagram-v2
  [*] --> pending
  pending --> answered :  H! answer
  pending --> cancelled :  A cancel
  deferred --> cancelled :  A cancel
  pending --> deferred :  H+ defer
  deferred --> pending :  H+ resume
  pending --> pending :  H+ resume
```

---

## Z2: tasks workflow firing order

```mermaid
stateDiagram-v2
  state "planner" as planning_role_0_planner
  planning --> planning_role_0_planner :  A → planner
  planning --> ready :  F ⇒ auto
  state "plan_reviewer" as plan_review_role_0_plan_reviewer
  plan_review --> plan_review_role_0_plan_reviewer :  A → plan_reviewer
  ready --> executing :  F ⇒ auto
  state "executor" as executing_role_0_executor
  executing --> executing_role_0_executor :  A → executor
  state "code_reviewer" as code_review_role_0_code_reviewer
  code_review --> code_review_role_0_code_reviewer :  A → code_reviewer
  complete --> in_review :  F ⇒ auto
  state "wrap" as in_review_role_0_wrap
  in_review --> in_review_role_0_wrap :  A → wrap
  accepted --> integration_queued :  F ⇒ auto
```

