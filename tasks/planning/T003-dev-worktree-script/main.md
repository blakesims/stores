# T003: dev worktree script for substrate task scaffolding

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T06:07:06Z
- **Last Updated:** 2026-05-03T06:11:13Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —
- **Branch:** feat/T003-dev-worktree-script

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** ./dev shell script; smoke test (bats / shell test / minimal cargo test invoking the script); brief README note; integration via existing &#x60;stores tasks next-id&#x60;, &#x60;stores tasks add&#x60; (with --display-id, --workspace-path, --branch flags), and &#x60;git worktree&#x60;.
- **Out:** Language-specific scaffolding (no Rust/Python/etc. project skeletons — just shell + git); IDE integration (auto-launch worktree in VSCode etc.); auto-launching &#x60;stores tasks drive&#x60; after &#x60;./dev new&#x60; (user runs drive when ready); Windows / non-POSIX shells (bash assumption); replacing or wrapping git commands beyond worktree.

### Done When
(1) &#x60;./dev&#x60; script exists at repo root, executable (chmod +x), bash (or equivalent shell — planner can choose).
(2) &#x60;./dev new&#x60; (interactive, or via flags &#x60;--slug&#x60;, &#x60;--title&#x60;, &#x60;--done-when&#x60;, &#x60;--scope-in&#x60;, &#x60;--scope-out&#x60;, &#x60;--assumptions&#x60;): runs &#x60;stores tasks next-id&#x60; to resolve T###; creates worktree at &#x60;../stores-T###-&lt;slug&gt;&#x60; on a new branch &#x60;feat/T###-&lt;slug&gt;&#x60; (stacked on the current branch by default; allow &#x60;--base &lt;branch&gt;&#x60; override); runs &#x60;stores tasks add --invoker ai_with_human --display-id T### --workspace-path &lt;abs-canonical&gt; --branch feat/T###-&lt;slug&gt; --slug &lt;slug&gt; --title &lt;title&gt; ...&#x60;; prints the worktree path on success.
(3) &#x60;./dev done &lt;T###&gt;&#x60;: removes the worktree (&#x60;git worktree remove&#x60;); refuses if the substrate row&#x27;s status is not in {accepted, rejected} (or with &#x60;--force&#x60;); does NOT delete the branch (let the user / merge-back-to-master decide).
(4) Edge cases handled with clear errors: target worktree path already exists; substrate row with that display_id already exists; user aborts mid-prompt (interactive mode); base branch has uncommitted changes (warn but proceed).
(5) Smoke test: &#x60;./dev new --slug&#x3D;test --title&#x3D;test --done-when&#x3D;x --scope-in&#x3D;x --scope-out&#x3D;x&#x60; from this repo creates the worktree, adds the substrate row, prints the path, exits 0. Inspecting the substrate confirms &#x60;workspace_path&#x60; matches the canonical path of the worktree.
(6) README usage note (very brief — 5-10 lines) added to repo root or docs/ explaining the two-verb flow.

### Assumptions
git worktree is available (git &gt;&#x3D; 2.5). &#x60;stores&#x60; binary is on PATH or invokable as &#x60;./target/debug/stores&#x60; (script can probe). Substrate&#x27;s tasks store is repo-scoped (per .stores/manifest.yaml — confirmed) so the worktree-spawned agents will find the canonical .stores/db.sqlite via the existing repo-walk-up logic. The new --display-id flag from L001&#x27;s fix is the key plumbing this script depends on.

### Phases

#### Phase 1: Phase 1: Script skeleton + dispatch + helpers
- **Objective:** Land an executable bash &#x60;./dev&#x60; at repo root with subcommand dispatch (&#x60;new&#x60;, &#x60;done&#x60;, &#x60;help&#x60;), shared helpers (locate &#x60;stores&#x60;, resolve repo root, validate slug, canonicalize path), and &#x60;--help&#x60; output. No worktree/substrate side effects yet.
- **Tasks:**
  - Task 1.1: Create &#x60;./dev&#x60; at repo root with bash shebang &#x60;#!/usr/bin/env bash&#x60;, &#x60;set -euo pipefail&#x60;, and chmod +x.
  - Task 1.2: Implement top-level dispatch on &#x60;$1&#x60;: &#x60;new&#x60;, &#x60;done&#x60;, &#x60;-h|--help|help&#x60; print usage; unknown verb exits 2 with usage-to-stderr.
  - Task 1.3: Add helper &#x60;locate_stores()&#x60;: prefers &#x60;command -v stores&#x60;; falls back to &#x60;./target/debug/stores&#x60; and &#x60;./target/release/stores&#x60;; errors with clear message if none found.
  - Task 1.4: Add helper &#x60;repo_root()&#x60;: &#x60;git rev-parse --show-toplevel&#x60;; errors if not in a git repo.
  - Task 1.5: Add helper &#x60;validate_slug()&#x60;: enforces &#x60;^[a-z0-9-]+$&#x60; (matches schema pattern); errors with the offending value on mismatch.
  - Task 1.6: Add helper &#x60;canonical_path()&#x60;: portable &#x60;realpath -m&#x60; or &#x60;python3 -c &#x27;import os,sys;print(os.path.realpath(sys.argv[1]))&#x27;&#x60; fallback for macOS-without-coreutils-realpath compatibility (POSIX-ish; bash assumption per scope).
  - Task 1.7: Stub &#x60;cmd_new()&#x60; and &#x60;cmd_done()&#x60; to print &#x27;not implemented&#x27; and exit 0 (to be filled in later phases).
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;./dev&#x60; is executable (&#x60;test -x ./dev&#x60;).
  - [ ] AC1.2: &#x60;./dev help&#x60; exits 0 and prints both &#x60;new&#x60; and &#x60;done&#x60; verbs with their flags.
  - [ ] AC1.3: &#x60;./dev bogus&#x60; exits 2 with usage on stderr.
  - [ ] AC1.4: &#x60;bash -n ./dev&#x60; passes (syntax check).
  - [ ] AC1.5: &#x60;./dev new&#x60; and &#x60;./dev done T001&#x60; both run without error (stubs).
- **Files:** `dev`
#### Phase 2: Phase 2: &#x60;./dev new&#x60; — worktree + substrate row creation
- **Objective:** Implement the full &#x60;new&#x60; verb: parse interactive prompts and/or flags (&#x60;--slug&#x60;, &#x60;--title&#x60;, &#x60;--done-when&#x60;, &#x60;--scope-in&#x60;, &#x60;--scope-out&#x60;, &#x60;--assumptions&#x60;, &#x60;--base&#x60;), resolve T### via &#x60;stores tasks next-id&#x60;, create the worktree, then create the substrate row.
- **Tasks:**
  - Task 2.1: Parse flags for &#x60;new&#x60; via a &#x60;while [[ $# -gt 0 ]]&#x60; loop accepting &#x60;--slug&#x60;, &#x60;--title&#x60;, &#x60;--done-when&#x60;, &#x60;--scope-in&#x60;, &#x60;--scope-out&#x60;, &#x60;--assumptions&#x60;, &#x60;--base&#x60;. Unknown flags → exit 2 with usage.
  - Task 2.2: For each missing required field (&#x60;slug&#x60;, &#x60;title&#x60;, &#x60;done-when&#x60;, &#x60;scope-in&#x60;, &#x60;scope-out&#x60;), prompt interactively via &#x60;read -r -p&#x60;. If stdin is not a TTY (&#x60;! [ -t 0 ]&#x60;) and a required field is missing, exit 2.
  - Task 2.3: Trap SIGINT (Ctrl-C) during prompts and exit 130 with a message; ensure no worktree/row was created (mid-prompt abort safe by ordering: prompt FIRST, then side-effect).
  - Task 2.4: Validate slug against &#x60;^[a-z0-9-]+$&#x60;; error if invalid.
  - Task 2.5: Resolve T### by &#x60;cd $(repo_root) &amp;&amp; stores tasks next-id&#x60; capturing stdout. Validate output matches &#x60;^T[0-9]{3}$&#x60;.
  - Task 2.6: Compute &#x60;WORKTREE_PATH&#x3D;$(repo_root)/../stores-T###-&lt;slug&gt;&#x60; and &#x60;BRANCH&#x3D;feat/T###-&lt;slug&gt;&#x60;. Refuse if &#x60;WORKTREE_PATH&#x60; already exists with a clear error referencing the path.
  - Task 2.7: Determine base branch: &#x60;--base&#x60; flag if supplied, else current branch via &#x60;git rev-parse --abbrev-ref HEAD&#x60;. If base has uncommitted changes (&#x60;git status --porcelain&#x60; non-empty in repo_root), print a warning to stderr but proceed.
  - Task 2.8: Create the worktree: &#x60;git worktree add -b $BRANCH $WORKTREE_PATH $BASE&#x60;. On failure, exit non-zero (do NOT call &#x60;stores tasks add&#x60; if worktree creation failed).
  - Task 2.9: Compute canonical absolute path of &#x60;$WORKTREE_PATH&#x60; and run &#x60;stores tasks add --invoker ai_with_human --display-id &lt;T###&gt; --workspace-path &lt;abs&gt; --branch &lt;branch&gt; --slug &lt;slug&gt; --title &lt;title&gt; --done-when &lt;...&gt; --scope-in &lt;...&gt; --scope-out &lt;...&gt; [--assumptions &lt;...&gt;]&#x60;. If the substrate add fails (e.g. duplicate display-id), remove the just-created worktree (&#x60;git worktree remove --force $WORKTREE_PATH&#x60;) and exit non-zero — do not leave half-created state. Forward the substrate&#x27;s stderr.
  - Task 2.10: On success, print the worktree absolute path to stdout (and only the path on the final line, so callers can &#x60;WORKTREE&#x3D;$(./dev new ...)&#x60;). Exit 0.
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;./dev new --slug&#x3D;test --title&#x3D;test --done-when&#x3D;x --scope-in&#x3D;x --scope-out&#x3D;x&#x60; from this repo creates &#x60;../stores-T&lt;NNN&gt;-test&#x60;, creates a substrate row with status&#x3D;planning, prints the absolute worktree path, and exits 0.
  - [ ] AC2.2: &#x60;stores tasks show T&lt;NNN&gt; --json&#x60; after AC2.1 contains &#x60;&quot;workspace_path&quot;&#x60; equal to the canonical realpath of the worktree directory.
  - [ ] AC2.3: Re-running the same command (so &#x60;WORKTREE_PATH&#x60; already exists) exits non-zero with a stderr message naming the existing path; no substrate row created.
  - [ ] AC2.4: If the substrate &#x60;add&#x60; fails (simulate by passing &#x60;--display-id T001&#x60; when T001 already exists), the worktree is rolled back (&#x60;git worktree list&#x60; does not show it).
  - [ ] AC2.5: Running &#x60;./dev new&#x60; with no flags and stdin closed (&#x60;&lt; /dev/null&#x60;) exits 2 with a usage message naming the missing fields.
  - [ ] AC2.6: Running with an invalid slug (&#x60;--slug&#x3D;Bad_Slug&#x60;) exits non-zero with an error naming the slug pattern.
- **Files:** `dev`
- **Dependencies:** Phase 1 complete
#### Phase 3: Phase 3: &#x60;./dev done&#x60; — guarded worktree teardown
- **Objective:** Implement &#x60;done &lt;T###&gt;&#x60;: refuse unless substrate status ∈ {accepted, rejected} (or &#x60;--force&#x60;), then &#x60;git worktree remove&#x60;. Branch is left intact.
- **Tasks:**
  - Task 3.1: Parse &#x60;done&#x60; args: positional &#x60;&lt;T###&gt;&#x60; (required), optional &#x60;--force&#x60;. Validate ID matches &#x60;^T[0-9]{3}$&#x60;.
  - Task 3.2: Read substrate status via &#x60;stores tasks show &lt;id&gt; --json&#x60; parsed with one of: &#x60;python3 -c &#x27;import json,sys;print(json.load(sys.stdin)[&quot;status&quot;])&#x27;&#x60; or &#x60;jq -r .status&#x60; (probe both; require at least one). If neither tool is on PATH, error clearly.
  - Task 3.3: If status is not in {accepted, rejected} and &#x60;--force&#x60; is not set, exit non-zero with a message naming the current status and suggesting &#x60;--force&#x60;.
  - Task 3.4: Read &#x60;workspace_path&#x60; from the same JSON; if missing or empty, error (the substrate row was not created by &#x60;./dev new&#x60;).
  - Task 3.5: Run &#x60;git worktree remove $workspace_path&#x60; (no &#x60;--force&#x60; unless &#x60;--force&#x60; was passed to &#x60;./dev done&#x60;). Print confirmation to stdout. Do NOT delete the branch.
  - Task 3.6: If &#x60;git worktree remove&#x60; fails (e.g., dirty worktree without &#x60;--force&#x60;), forward stderr verbatim and exit non-zero.
- **Acceptance Criteria:**
  - [ ] AC3.1: &#x60;./dev done T&lt;NNN&gt;&#x60; against a row in status&#x3D;planning exits non-zero with a message naming the current status.
  - [ ] AC3.2: After manually flipping status to accepted (via direct substrate verbs in test setup), &#x60;./dev done T&lt;NNN&gt;&#x60; removes the worktree and exits 0.
  - [ ] AC3.3: After AC3.2, &#x60;git branch --list feat/T&lt;NNN&gt;-*&#x60; still shows the branch (branch NOT deleted).
  - [ ] AC3.4: &#x60;./dev done T&lt;NNN&gt; --force&#x60; against a planning row removes the worktree.
  - [ ] AC3.5: &#x60;./dev done&#x60; (no positional) exits 2 with usage.
- **Files:** `dev`
- **Dependencies:** Phase 2 complete
#### Phase 4: Phase 4: Smoke test + README note
- **Objective:** Add a shell-based smoke test exercising the happy path of &#x60;./dev new&#x60; and a brief README note documenting the two-verb flow.
- **Tasks:**
  - Task 4.1: Create &#x60;tests/dev_script_smoke.sh&#x60; (bash, &#x60;set -euo pipefail&#x60;): create a tempdir, &#x60;git init&#x60;, copy/symlink the &#x60;dev&#x60; script and minimal &#x60;.stores/&#x60; skeleton or run from this repo root via &#x60;cd&#x60; (planner&#x27;s choice; document in script header). Invoke &#x60;./dev new --slug&#x3D;smoke --title&#x3D;smoke --done-when&#x3D;x --scope-in&#x3D;x --scope-out&#x3D;x&#x60;, assert exit 0, capture printed path, assert path exists, assert substrate row exists with matching &#x60;workspace_path&#x60;. Tear down with &#x60;./dev done &lt;id&gt; --force&#x60; and assert worktree removed.
  - Task 4.2: Make the smoke script executable and self-contained (no external bats dependency); document invocation in its header (&#x60;bash tests/dev_script_smoke.sh&#x60;).
  - Task 4.3: Add a brief 5-10 line section to &#x60;README.md&#x60; titled &#x27;Spawning a task worktree&#x27; describing &#x60;./dev new&#x60; and &#x60;./dev done&#x60; with one example each. Do NOT duplicate the full contract — link to &#x60;CLAUDE.md&#x60; for the dogfood doctrine.
- **Acceptance Criteria:**
  - [ ] AC4.1: &#x60;bash tests/dev_script_smoke.sh&#x60; exits 0 from a clean repo state.
  - [ ] AC4.2: &#x60;tests/dev_script_smoke.sh&#x60; is executable (&#x60;test -x&#x60;).
  - [ ] AC4.3: &#x60;README.md&#x60; contains a section header matching &#x60;Spawning a task worktree&#x60; (or equivalent) of 5-10 lines, linking to &#x60;CLAUDE.md&#x60;.
  - [ ] AC4.4: After the smoke test runs, no orphan worktrees remain (&#x60;git worktree list&#x60; shows only the two pre-existing entries: master + this T003 worktree).
- **Files:** `tests/dev_script_smoke.sh`, `README.md`
- **Dependencies:** Phase 3 complete

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

