## Review
- Correct: `apply_obs_to_flow` now makes visible observation flow slots mutually exclusive for waiting overlays: an `open` observation with `waiting_kind=info_needed` increments only the waiting slot and no longer also increments `candidate` (`src/tui/data.rs:639-659`, test at `src/tui/data.rs:3568-3586`).
- Correct: contract-gated observations are counted in the contract-gate/ready slot instead of candidate (`src/tui/data.rs:646-647`, `src/tui/data.rs:665-675`; test at `src/tui/data.rs:3589-3608`).
- Correct: row rendering removes the repeated presentation signal and hides raw `tier:` / `contract:` vocabulary; it now renders semantic state, priority/tier, next action, linked task, and summary (`src/tui/render.rs:1152-1189`). The updated row assertions cover `high/T2`, `next:triage`, and absence of `contract:`/`tier:` (`src/tui/render.rs:1920-1921`, `src/tui/render.rs:2631-2636`).
- Correct: no unrelated files are staged: `git diff --cached --name-only` returned empty. There are many unrelated unstaged modifications in the worktree, but only `src/tui/data.rs` and `src/tui/render.rs` are part of this focused diff.
- Correct: targeted tests I ran passed:
  - `cargo test -q observation_flow_slots_are_mutually_exclusive --lib` → 2 passed
  - `cargo test -q format_obs_line_surfaces_one_state_next_action_and_hides_raw_contract --lib` → 1 passed
  - `cargo test -q row_line_exposes_priority_tier_and_held_reason_snippets --lib` → 1 passed

- Blocker: `contract_state=ready` is internally treated as a contract gate in the top-card model but rendered as ordinary investigation work in rows. `apply_obs_to_flow` maps `Some("ready")` to `flow.ready`/contract gate (`src/tui/data.rs:671-674`), and sectioning treats `contract_state == "ready"` as ratifiable (`src/tui/data.rs:2391-2392`). But the row test for a `contract_state: Some("ready")` observation currently expects `◆ investigate` and `next:gather evidence` (`src/tui/render.rs:2619-2633`). That violates the Option A intent of one semantic state + next action for contract-gated observations: the same row appears as contract-gated in the top card but as investigation work in the row.
  - Exact fix: update observation presentation/row behavior so `contract_state=ready` renders as a contract/ratification gate with an appropriate next action (for example `contract-ready` + `next:approve/revise` or `next:ratify`), and update the row assertion at `src/tui/render.rs:2631-2633` accordingly. Add/extend a test that covers `contract_state=ready`, not only `draft`/`approved`.

- Note: The comment says raw waiting maps remain available for drilldown/debug (`src/tui/data.rs:640-643`), but the implementation filters `human_ratification` out of `waiting_kinds` (`src/tui/data.rs:682-687`) and the contract-gate test asserts the waiting map sum is zero (`src/tui/data.rs:3606-3608`). If `waiting_kinds` is intended to be the raw drilldown map, split raw waiting counts from visible top-card waiting counts; if it is intended to be the visible waiting slot only, adjust the comment/requirement wording.

Gate: REVISE
