# T001: approval-token mechanism for chat-mediated human assent

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T05:38:41Z
- **Last Updated:** 2026-05-03T05:44:30Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —

## Task

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** Schema actor relaxation; encrypted at-rest token storage (age, leveraging the user&#x27;s existing age key + sops/sb baseline); stores auth init/show CLI verbs; --approve-token flag plumbing on all relevant write verbs; CLAUDE.md doctrine update (tier-A/tier-B + threat model); tests covering encryption-at-rest, age-key-protection validation, valid/invalid token paths, and the ai_autonomous-rejected case.
- **Out:** MCP server / non-CLI transport (deferred per user instruction); daemon-mediated signing model (token never leaves a long-running daemon&#x27;s memory); per-row nonces and replay protection (single shared secret + ask-first discipline is sufficient this iteration); subagent context-redaction (orchestrator&#x27;s discipline covers); custom KMS / non-age encryption schemes; token rotation UI / external sync.

### Done When
(1) Approval token storage — encrypted at rest: (a) random 32-byte secret generated at &#x60;stores auth init&#x60;; (b) persisted to disk age-encrypted at ~/.config/stores/approve.token.age (gitignored, 0600); (c) encrypted to the user&#x27;s age recipient (auto-discovered or --recipient-specified); (d) stores auth init validates the user&#x27;s age key is passphrase-protected or hardware-backed; REFUSES init if the age key is raw plaintext; (e) &#x60;stores auth show&#x60; decrypts interactively (age prompts the user for passphrase / hardware tap) and prints to stdout — AI cannot decrypt without user-presence.
(2) Substrate accepts --approve-token &lt;T&gt; on writes currently gated by actor: human or actor: ai_with_human. Token verified by constant-time hash-equality against the stored expected hash. Invalid token → clear rejection.
(3) Schema&#x27;s &#x60;actor: human&#x60; semantics relaxed: such fields/transitions accept EITHER --invoker human OR (--invoker ai_with_human + valid --approve-token). ai_autonomous still rejected — token does NOT relax the AI-only case.
(4) observations.investigate transition actor → ai_autonomous (closes L007).
(5) observations.confirm + intent_contract.approved_by/at fields eligible for the token-mediated path (closes L008).
(6) CLAUDE.md root + stores/observations/CLAUDE.md gain a tier-A / tier-B doctrine paragraph + a section explicitly naming the threat model: AI cannot fabricate assent because the token requires user-presence to decrypt; once decrypted into chat context, the AI&#x27;s ask-first discipline is the runtime protection until the session ends.
(7) Tests: at-rest encryption verified (file does NOT contain raw secret bytes); init-refuses-unprotected-age-key; valid-token path; invalid-token path; token-missing-with-invoker-human; token-present-with-invoker-autonomous-rejected.
(8) cargo build + cargo test green.

### Assumptions
User has an age key configured that requires user-presence to decrypt (passphrase or hardware) — stores does NOT roll its own crypto, it leverages age. The user&#x27;s existing sops/age/sb workflow is the security baseline. Once the user runs &#x60;stores auth show&#x60; and pastes the token into chat, the AI possesses the token for the remainder of the chat session; the ask-first behavioral discipline is the runtime protection during that window. Session-end → token leaves AI context.

### Phases

#### Phase 1: Phase 1: auth CLI + age-encrypted token storage
- **Objective:** Add &#x60;stores auth init&#x60; and &#x60;stores auth show&#x60;; generate, encrypt, persist a 32-byte secret + its SHA-256 hash on the user&#x27;s filesystem, refusing init when the user&#x27;s age identity is raw plaintext.
- **Tasks:**
  - Task 1.1: Add &#x60;sha2&#x60; and &#x60;subtle&#x60; (constant-time-eq) crates to Cargo.toml
  - Task 1.2: Create src/cli/auth.rs with &#x60;AuthCmd::{Init {recipient: Option&lt;String&gt;, identity: Option&lt;PathBuf&gt;, force: bool}, Show}&#x60; and a &#x60;token_dir()&#x60; helper honoring &#x60;STORES_TOKEN_DIR&#x60; env override (defaults to &#x60;~/.config/stores/&#x60;)
  - Task 1.3: Implement &#x60;auth init&#x60;: (a) generate 32 random bytes via getrandom; (b) auto-discover recipient from &#x60;~/.config/sops/age/keys.txt&#x60; (parse &#x60;# public key: age1...&#x60; comment) or use &#x60;--recipient&#x60;; (c) validate identity file is NOT raw &#x60;AGE-SECRET-KEY-&#x60; plaintext (accept &#x60;-----BEGIN AGE ENCRYPTED FILE-----&#x60; armored / &#x60;AGE-PLUGIN-&#x60; plugin identities); (d) shell out to &#x60;age -r &lt;recipient&gt; -o approve.token.age&#x60; writing 0600; (e) write hex SHA-256 to &#x60;approve.token.hash&#x60; 0644; (f) refuse if files exist unless &#x60;--force&#x60;
  - Task 1.4: Implement &#x60;auth show&#x60;: shell out to &#x60;age -d -i &lt;identity&gt;&#x60; (passes through tty for passphrase/hardware prompt) and print decrypted token to stdout
  - Task 1.5: Register &#x60;auth&#x60; subcommand tree in src/cli/dynamic.rs build_root and dispatch in src/main.rs
  - Task 1.6: Add ~/.config/stores/ to .gitignore docs (project doesn&#x27;t gitignore home, but documented as-such in &#x60;auth init&#x60; stdout)
  - Task 1.7: Tests in cli/auth.rs: (a) raw-bytes-not-on-disk (open .age file, assert no 32-byte plaintext substring); (b) refuses-init-on-raw-plaintext-identity (write fake &#x60;AGE-SECRET-KEY-...&#x60; file, expect error); (c) hash file contents &#x3D;&#x3D; sha256(plaintext_bytes); (d) round-trip via mock age (use &#x60;STORES_AGE_BIN&#x60; env override pointing at a passthrough script for CI)
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo build&#x60; succeeds with new crates
  - [ ] AC1.2: &#x60;STORES_TOKEN_DIR&#x3D;/tmp/tok stores auth init --recipient age1...&#x60; produces &#x60;approve.token.age&#x60; (0600) + &#x60;approve.token.hash&#x60; (0644)
  - [ ] AC1.3: &#x60;stores auth init&#x60; refuses when identity file begins &#x60;AGE-SECRET-KEY-&#x60; and emits a remedy mentioning &#x60;passphrase-encrypt your age key&#x60;
  - [ ] AC1.4: encrypted file content does NOT contain raw 32 random bytes (sliding-window check)
  - [ ] AC1.5: hash file is exactly 64 hex chars and equals sha256 of the plaintext token
  - [ ] AC1.6: &#x60;cargo test cli::auth&#x60; passes (≥4 tests)
- **Files:** `Cargo.toml`, `src/cli/auth.rs`, `src/cli/mod.rs`, `src/cli/dynamic.rs`, `src/main.rs`
#### Phase 2: Phase 2: --approve-token plumbing + verifier
- **Objective:** Plumb &#x60;--approve-token&#x60; from CLI through dispatch to validators, with a constant-time hash verifier that returns a boolean used by actor checks downstream.
- **Tasks:**
  - Task 2.1: Add &#x60;--approve-token &lt;T&gt;&#x60; global Arg in src/cli/dynamic.rs::build_root (parallel to &#x60;--invoker&#x60;)
  - Task 2.2: Introduce &#x60;pub struct InvokerCtx { pub actor: Actor, pub token_valid: bool }&#x60; in src/schema/actor.rs (or new src/validate/invoker_ctx.rs); add &#x60;pub fn verify_approve_token(token: &amp;str) -&gt; bool&#x60; that reads &#x60;${STORES_TOKEN_DIR:-~/.config/stores}/approve.token.hash&#x60;, sha256s the input, constant-time-compares via &#x60;subtle::ConstantTimeEq&#x60;. Returns false if hash file missing.
  - Task 2.3: In src/cli/dispatch.rs::detect_invoker → return &#x60;InvokerCtx&#x60;; read &#x60;--approve-token&#x60; and call verify; on token-supplied-but-invalid emit clear error and exit non-zero (do NOT silently drop)
  - Task 2.4: Thread &#x60;InvokerCtx&#x60; (or &#x60;Actor&#x60; + &#x60;bool&#x60;) through every handler signature: add.rs, update.rs, transition.rs, submit.rs (all variants), drive.rs. Handlers pass ctx into validators.
  - Task 2.5: Tests in validate/invoker_ctx.rs (or auth.rs): (a) verify_approve_token returns true on matching token; (b) returns false on mismatch; (c) returns false on missing hash file; (d) constant-time path uses subtle (smoke-test by checking equal-prefix-different-suffix returns false)
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;stores tasks add ... --approve-token wrong&#x60; exits non-zero with message containing &#x60;invalid approval token&#x60;
  - [ ] AC2.2: &#x60;stores tasks add ... --approve-token &lt;correct&gt;&#x60; proceeds to validation (does not error on the token check)
  - [ ] AC2.3: All handler signatures compile with the new ctx parameter; cargo build green
  - [ ] AC2.4: &#x60;cargo test validate::invoker_ctx&#x60; passes (≥3 tests)
  - [ ] AC2.5: --approve-token absent behaves identically to today (token_valid&#x3D;false, no actor relaxation)
- **Files:** `src/cli/dynamic.rs`, `src/cli/dispatch.rs`, `src/schema/actor.rs`, `src/validate/mod.rs`, `src/handlers/add.rs`, `src/handlers/update.rs`, `src/handlers/transition.rs`, `src/handlers/submit.rs`
- **Dependencies:** Phase 1 (hash file format + token_dir helper exist)
#### Phase 3: Phase 3: Schema actor relaxation in validators
- **Objective:** Modify actor_allowed to honor a valid token: actor:human AND actor:ai_with_human now satisfied by (invoker&#x3D;ai_with_human + token_valid). actor:ai_autonomous and actor:framework remain unchanged — token does NOT unlock autonomous.
- **Tasks:**
  - Task 3.1: Update &#x60;actor_allowed(invoker, required)&#x60; → &#x60;actor_allowed(invoker, required, token_valid: bool)&#x60; in src/validate/actor.rs
  - Task 3.2: Branch logic: when required&#x3D;Human, return true if invoker&#x3D;Human OR (invoker&#x3D;AiWithHuman AND token_valid); when required&#x3D;AiWithHuman, current logic (no change unless Q1 says make token mandatory); AiAutonomous + Framework unchanged
  - Task 3.3: Update &#x60;invoker_remedy&#x60; to mention &#x60;--approve-token&#x60; as an alternative remedy when required&#x3D;Human and invoker&#x3D;AiWithHuman/AiAutonomous
  - Task 3.4: Update both call sites (check_actor, check_transition_actor) signatures and all callers in handlers
  - Task 3.5: Tests: (a) actor:human + invoker&#x3D;ai_with_human + token_valid&#x3D;true → allowed; (b) actor:human + invoker&#x3D;ai_with_human + token_valid&#x3D;false → rejected; (c) actor:human + invoker&#x3D;ai_autonomous + token_valid&#x3D;true → STILL rejected; (d) actor:human + invoker&#x3D;human + token_valid&#x3D;false → allowed (preserved); (e) actor:ai_autonomous + token_valid&#x3D;true → still rejected (token does not unlock autonomous); (f) remedy message for actor:human mentions both &#x60;--invoker human&#x60; AND &#x60;--approve-token&#x60;
- **Acceptance Criteria:**
  - [ ] AC3.1: &#x60;cargo test validate::actor&#x60; passes with ≥6 new tests covering the matrix above
  - [ ] AC3.2: e2e: &#x60;stores tasks add --invoker ai_with_human --approve-token &lt;correct&gt; ...&#x60; succeeds on actor:human field
  - [ ] AC3.3: e2e: same call without &#x60;--approve-token&#x60; fails with remedy message naming &#x60;--approve-token&#x60;
  - [ ] AC3.4: e2e: &#x60;--invoker ai_autonomous --approve-token &lt;correct&gt;&#x60; STILL fails on actor:human (regression-guard for the AI-only case)
- **Files:** `src/validate/actor.rs`, `src/handlers/add.rs`, `src/handlers/update.rs`, `src/handlers/transition.rs`, `src/handlers/submit.rs`
- **Dependencies:** Phase 2 (InvokerCtx exists; handlers thread token_valid)
#### Phase 4: Phase 4: Observations schema updates (close L007 + L008)
- **Objective:** Apply the actor changes the relaxation enables: investigate becomes ai_autonomous; intent_contract.approved_by/at remain actor:human and become token-mediated automatically via Phase 3.
- **Tasks:**
  - Task 4.1: In stores/observations/schema.yaml change &#x60;investigate&#x60; transition &#x60;actor: ai_with_human&#x60; → &#x60;actor: ai_autonomous&#x60; (closes L007)
  - Task 4.2: Verify intent_contract.approved_by and approved_at remain &#x60;actor: human&#x60; — no schema change needed; document inline that they are now reachable via &#x60;--invoker ai_with_human --approve-token &lt;T&gt;&#x60; (closes L008 via Phase 3 relaxation)
  - Task 4.3: Add new e2e tests in tests/observations_e2e.sh: (a) &#x60;observations investigate&#x60; works with &#x60;--invoker ai_autonomous&#x60;; (b) writing &#x60;intent_contract.approved_by&#x60; with &#x60;--invoker ai_with_human --approve-token &lt;correct&gt;&#x60; succeeds; (c) same write without token fails
  - Task 4.4: Run existing tests/observations_e2e.sh to confirm no regression
- **Acceptance Criteria:**
  - [ ] AC4.1: &#x60;grep -A1 &#x27;verb: investigate&#x27; stores/observations/schema.yaml&#x60; shows &#x60;actor: ai_autonomous&#x60;
  - [ ] AC4.2: tests/observations_e2e.sh exits 0 (with new test cases added)
  - [ ] AC4.3: &#x60;stores observations investigate &lt;id&gt;&#x60; succeeds without &#x60;--invoker ai_with_human&#x60; (auto-detected ai_autonomous from CLAUDECODE works)
  - [ ] AC4.4: &#x60;stores observations update &lt;id&gt; --intent-contract.approved-by ... --invoker ai_with_human --approve-token &lt;wrong&gt;&#x60; fails; with correct token succeeds
- **Files:** `stores/observations/schema.yaml`, `tests/observations_e2e.sh`
- **Dependencies:** Phase 3 (token-mediated actor:human path live)
#### Phase 5: Phase 5: Doctrine — CLAUDE.md tier-A / tier-B + threat model
- **Objective:** Document the new doctrine: tier-A writes (token-required) vs tier-B (ai_with_human honor-system); explicitly state the threat model and the runtime-protection window.
- **Tasks:**
  - Task 5.1: Add a new &#x60;## Approval-token doctrine (tier-A / tier-B)&#x60; section to CLAUDE.md root after the existing &#x60;--invoker discipline&#x60; section. Explain: tier-A &#x3D; &#x60;actor: human&#x60; gates (token-mediated); tier-B &#x3D; &#x60;actor: ai_with_human&#x60; (honor-system, no token); ai_autonomous remains the autonomous default
  - Task 5.2: Add a &#x60;### Threat model&#x60; subsection: AI cannot fabricate assent because token decryption requires user-presence (passphrase or hardware tap); once decrypted into chat, the AI possesses it for the session — the ask-first behavioral discipline is the runtime protection until session-end clears the AI&#x27;s context window
  - Task 5.3: Update U1–U4 listing in CLAUDE.md: each U-moment now has two equivalent grounding paths — (a) &#x60;--invoker human&#x60; (user types the verb) or (b) &#x60;--invoker ai_with_human --approve-token &lt;T&gt;&#x60; (user pre-authorized via token decryption + AI executes)
  - Task 5.4: Update stores/observations/CLAUDE.md: investigate is now &#x60;(auto)&#x60; not &#x60;(U)&#x60;; confirm + approved_by are tier-A token-mediated
  - Task 5.5: Confirm &#x60;cargo build &amp;&amp; cargo test&#x60; pass cleanly across the full suite
- **Acceptance Criteria:**
  - [ ] AC5.1: CLAUDE.md contains the literal headers &#x60;Approval-token doctrine (tier-A / tier-B)&#x60; and &#x60;Threat model&#x60;
  - [ ] AC5.2: stores/observations/CLAUDE.md no longer marks &#x60;investigate&#x60; as &#x60;(U)&#x60; — it is &#x60;(auto)&#x60;
  - [ ] AC5.3: &#x60;cargo build&#x60; succeeds
  - [ ] AC5.4: &#x60;cargo test&#x60; exits 0 (full suite, including all phases&#x27; tests)
  - [ ] AC5.5: &#x60;tests/observations_e2e.sh &amp;&amp; tests/tasks_e2e.sh&#x60; both exit 0
- **Files:** `CLAUDE.md`, `stores/observations/CLAUDE.md`
- **Dependencies:** Phase 4 (schema changes the docs describe are live)

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

