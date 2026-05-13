# Watch Topbar Progress

**Date:** 2026-05-13
**Type:** note

## Summary

Implemented the first semantic `stores watch` cockpit pass and then iterated the top bar from compressed glyph codes into readable store cards.

## Details

Completed work:

- Added a semantic presentation layer for tasks, observations, intake, external reviews, and engine state.
- Replaced raw task row tuple text (`active:none:none`, `runner:none`, lifecycle/debug fields) with operator labels such as `plan`, `exec`, `accept`, `runner-failed`, and `waiting-capacity`.
- Reworked top lane cards into a shared six-slot grammar across separate store cards: front/work/gate/exit/wait/fault.
- Added semantic row rendering for observations, intake, and external reviews, hiding null/none clutter.
- Reordered detail panes around operator state and live/latest runner information while preserving raw debug tuples lower down.
- Added clean-DB semantic regression coverage for representative row shapes.
- Planned and implemented a second top-bar UX iteration after Blake reported compressed labels like `◌cand8 ◆inv0` were still too cryptic.
- Replaced compressed top-card text with readable 3-column x 2-row card grids using full labels and separated counts.
- Added Catppuccin Mocha-style severity colors: quiet exhaust/success, graded flow pressure, faster fault escalation.
- Fixed the responsive behavior so all five cards render when the available terminal width can fit lane-specific readable labels; focused-card `+ more` fallback remains only for genuinely narrow widths.
- Installed the updated binary after each implementation pass.

Current observed state from Blake's latest check:

- The card layout is much better and all store cards can show on fullscreen.
- The observations top card still has a semantic problem: `candidates 8` and `waiting 8` appear simultaneously, which feels like double-counting and creates cognitive ambiguity.
- Observation rows remain too heavy, e.g. `contract-draft contract draft ... contract:draft`, repeating the same concept in multiple vocabularies.

## Follow-ups

- Clarify what observation top-card slots should count, especially `candidates` vs `waiting`.
- Redesign the observation focused-row grammar so it tells the operator one clear state, one reason/signal, and the summary, without repeated contract/status/debug vocabulary.
- Keep raw ADR/status/contract fields in detail/debug, not in the row list.
