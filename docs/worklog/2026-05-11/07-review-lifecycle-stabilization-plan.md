## 2026-05-11 T148 status note

T149 completed earlier. T148 was stuck in an external-review revise loop, not because the original review-lifecycle bug was still open, but because each review exposed additional ADR0002 edge-case gaps while validating phase 6.

Shipped/committed on the T148 worktree before final recovery:
- `9b103da` — TUI reads primary observation `contract_state` and ADR intake waiting kinds.
- `9c28378` — restored wrap/in_review schema invariants (`active_step=wrapping`, wrap dispatch).
- `a416f8b` — linked observations can clear architecture-review U1 gates; superseded reviews record `superseded_by_id` when superseded by a successor review.
- `39a55e8` — architecture-review verdict effects are deferred until a review is actually closed/ratified.
- `23ab20f` — explicit `supersede A###` without a successor is rejected; legacy `status='resolved'` observations are treated as already resolved by auto-resolve.

Final recovery completed:
- `d8a89b4` — applied the interrupted executor WIP: route-isolated temp-repo tests and successor-verdict supersede regression.
- `244af10` — kept `external_reviews import-pass` usable against the live DB runner CHECK by storing manual imports as `codex` rows while preserving the manual label in transition history.
- `55caf51` — merged `feat/T148-auto-promoted-l568` to `main`.

Final state:
- T148 accepted and integrated.
- `cargo install --path /home/blake/repos/experiments/stores --features runner-claude-code,runner-pi --locked` completed; `stores --version` reports `git_sha=55caf516e724f8267d65996204ec27a2ae6682d1`.
- `stores migrate --apply --invoker ai_autonomous` completed with only the pre-existing `daemon_starts.daemon_epoch` orphan-column warning.

Targeted checks run during final recovery:
- `AS=/usr/bin/as cargo test --test external_review_acceptance manual_import_pass_creates_auditable_pass_row -- --test-threads=10`
- `AS=/usr/bin/as cargo test --test activation_gating --test agents_daemon_pidfile_e2e --test auth_help --test cli_engine_plan_start -- --test-threads=10`
- `AS=/usr/bin/as cargo test --test architecture_reviews_cardinality -- --test-threads=10`

Operational notes:
- Full workspace tests were intentionally not rerun in final recovery because prior full-suite executor runs were the source of the convergence stall.
- The old stale runner metadata still appears in task status, but no matching live process was found.
