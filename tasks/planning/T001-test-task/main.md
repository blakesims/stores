# T001: Test Task

## Meta
- **Status:** plan_review
- **Created:** 2026-01-01T00:00:00Z
- **Last Updated:** 2026-05-03T14:52:57Z
- **Current Phase:** 0
- **Current Cycle:** 0
- **Blocked Reason:** —

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** Everything
- **Out:** Nothing

### Done When
It works

### Phases

#### Phase 1: Phase 1: CLI agents module + wiring
- **Objective:** Ship src/cli/agents.rs as a flat-file install clone of cli/skills.rs with BUNDLED_AGENTS registry, registered in mod.rs, dynamic.rs, and main.rs.
- **Tasks:**
  - Task 1.1: Create src/cli/agents.rs cloning cli/skills.rs with BUNDLED_AGENTS (5 entries, include_str!)
  - Task 1.2: Add pub mod agents to src/cli/mod.rs
  - Task 1.3: Register agents subcommand in src/cli/dynamic.rs build_root parallel to skills
  - Task 1.4: Dispatch agents matches in src/main.rs parallel to skills block
- **Acceptance Criteria:**
  - [ ] AC1.1: cargo build succeeds with the new include_str! paths
  - [ ] AC1.2: stores agents list prints exactly 5 entries with installed/uninstalled annotations
  - [ ] AC1.3: cargo test cli::agents passes; BUNDLED_AGENTS.len() &#x3D;&#x3D; 5 asserted
  - [ ] AC1.4: stores agents install --all writes 5 flat .md files to .claude/agents/
- **Files:** `src/cli/agents.rs`, `src/cli/mod.rs`, `src/cli/dynamic.rs`, `src/main.rs`

---

## Plan Review

_No plan reviews yet._


---

## Execution Log

_No execution cycles yet._

---

## Code Review Log

_No code reviews yet._

---

## Completion
_Not yet complete._

