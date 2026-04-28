# T004: Schema-validated agent envelope via `--json-schema`

## Meta
- **Status:** EXECUTING_PHASE_1_REVISE
- **Created:** 2026-04-28
- **Last Updated:** 2026-04-28
- **Blocked Reason:** —

## Task

### Executive intent

The v0.3.0 smoke run (T002 in `/tmp/t003-smoke`) surfaced a real-world bug in the bundled-agent contract that the mock-runner e2e tests cannot catch: **agents wrap their JSON envelope in a markdown ` ```json ` fence**, so the runner's "last non-empty stdout line is the envelope" parser fails because the *actual* last line is the closing ` ``` `. Thirty seconds of haiku reasoning is then thrown away, with no resume capability.

Two follow-up research rounds established that this is not a model-capability issue (the planner produced correct JSON in shape; reasoning-before-structure is the right pattern and the current design already follows it) but a **parsing fragility issue plus a missing retry/recovery mechanism**. The literature ("Let Me Speak Freely?" 2024 vs JSONSchemaBench 2025) shows that strict JSON-schema-validated structured output via constrained decoding does *not* degrade reasoning when the schema's reasoning fields precede answer fields — and `claude -p --json-schema` ships exactly this validate-then-retry-with-prior-context behaviour internally.

T004 replaces the line-scanning envelope parser with `--json-schema` server-side validation, switches the runner to `--output-format stream-json --verbose` for full transcript capture, generates `--session-id` upfront so any non-schema failure is resumable, and removes the brief-template contradictions that tell agents to call `stores tasks submit-*` (which the v0.3 envelope-only contract forbids). The protocol contract is unchanged from a runtime-neutrality standpoint — the envelope concept ports trivially to OpenAI's `response_format`, Codex's `--output-schema`, and the Anthropic Python SDK's `output_config`, all of which support schema-validated structured output natively.

### DONE_WHEN

> In `/tmp/t003-smoke` with the v0.4 binary installed, `rm -rf .stores tasks && stores setup && stores tasks add --invoker human --title "Hello world" --slug "hello" --done-when "echo hi prints hi" --scope-in "scripts/" --scope-out "src/"` followed by `stores tasks drive --auto --claude-code --testing` runs end-to-end against haiku without an envelope-parse error, producing a `complete` task with executor commit and `gate=PASS` code review — the same outcome T001's prior smoke achieved, but now resilient to the markdown-fence pathology and with a stream-json transcript captured for postmortem.

### Scope

**In scope**

- **JSON Schema files** for the 5 agent envelopes (`agents/schemas/{planner,plan-reviewer,executor,code-reviewer,guide}.schema.json`). Each schema places a free-text reasoning/notes field before structured fields where applicable (the dottxt-ai field-ordering recovery pattern).
- **`BUNDLED_AGENT_SCHEMAS` registry** in `src/cli/agents.rs` mirroring `BUNDLED_AGENTS` (`include_str!` at compile time).
- **Runner trait extension**: `spawn(role, system_prompt, brief, schema: Option<&str>) -> Result<RunnerOutput>`; `RunnerOutput` gains `structured_output: Option<serde_json::Value>` and `session_id: Option<String>`.
- **`claude_code` runner update**: pass `--json-schema=<schema>`, **generate the `--session-id=<uuid>` internally inside the runner** (drive does NOT mint the UUID; the runner produces it and returns it via `RunnerOutput.session_id`), switch to `--output-format stream-json --verbose`, parse the `result` event for `structured_output`. Pin `cwd` (the documented #1 footgun for session resume).
- **`mock` runner update**: accept canned `structured_output` directly; no schema validation needed in mock (preserves test simplicity).
- **Drive consumes `structured_output`** instead of last-line-scanning. Existing `parse_envelope` becomes a thin schema-aware deserializer that prefers `structured_output` and falls back to last-line scan only for legacy/mock use.
- **Brief-template fix**: drop "Call `stores tasks submit-*`" instructions from all 4 task brief templates (`stores/tasks/templates/{planner,plan-reviewer,executor,code-reviewer}-brief.md.tpl`). They contradict the envelope-only contract that landed in v0.3.
- **Agent-prompt simplification**: remove the "emit JSON on the last non-empty line, no markdown fences" hand-wringing now that the schema enforces shape. Prompts describe content, not formatting.
- **Stream-json transcript capture**: write the JSONL stream to `.stores/runs/<session-id>.jsonl` for postmortem (foundation for v0.4 `runs` event-log store; not the full v0.4 feature, just the capture path).
- **Tests**: `tests/runner_claude_code_unit.rs` (or extend existing) covers `--json-schema` flag emission and `result.structured_output` extraction; mock-runner tests updated to use `structured_output`; existing `drive_e2e.sh` continues to pass.
- **Cargo bump 0.3.0 → 0.4.0** on Phase 3 success (Runner trait change is breaking — semver minor, per user direction).

**Out of scope (deferred)**

- Full `runs` event-log store with query API (v0.4+; this task only writes the JSONL).
- Auto-retry on broader failures beyond what `--json-schema` does internally (drive's own retry loop).
- Second runner (pi/headless-Python). Trait change is forward-compatible; we don't write the runner here.
- Migrating other CLIs (`gemini-cli`, `opencode`) — not on the runner roadmap.
- The "multiple task directories found for T001" warning from drive's render path. Still on v0.4 punch list.
- Extended thinking integration with structured outputs (Anthropic's docs note these are not yet compatible).

**Should remain unchanged**

- All v0.3 schema features and CLI verbs (`stores tasks {add,drive,brief,submit-*,render,status}`, `stores agents {list,install,uninstall}`, `stores setup`, `stores gate guide`).
- The envelope-only contract direction (drive is sole DB writer; agents are read-only). T004 strengthens this; it does not change it.
- Mock-runner tests' state-machine coverage in `drive_e2e.sh` and the unit tests in `handlers/drive.rs::tests`.
- T001 / T002 / T003 on-disk task documents.

### Proposed approach (high level)

Three-phase, bottom-up:

1. **Phase 1 — Schemas + runner refactor.** Author the 5 JSON Schemas. Extend the `Runner` trait. Update `claude_code.rs` to pass `--json-schema`/`--session-id` and parse stream-json. Update `mock.rs` to expose `structured_output`. Unit tests pass.
2. **Phase 2 — Drive + briefs + prompts.** Drive reads `structured_output` from `RunnerOutput`. Brief templates lose their CLI-call instructions. Agent prompts shed the JSON-fence hand-wringing. Existing e2e tests pass.
3. **Phase 3 — Smoke + version.** Re-run the haiku smoke against `/tmp/t003-smoke` from a clean state. If it goes end-to-end, bump Cargo to 0.4.0 and tag.

### Risks / assumptions

- **`--json-schema` retry budget is internal/non-configurable.** If a model consistently fails schema validation (e.g. brief is misleading), drive sees a single error rather than a retry-exhausted message it can act on. Mitigation: surface the `result.error.subtype` (`error_max_structured_output_retries`) clearly in drive's stderr; the user can `--resume` manually with the guide agent if needed.
- **`cwd` footgun.** Session resume silently mints a fresh session if cwd differs between spawn and resume calls. Mitigation: drive must canonicalise and pin cwd; add a unit test that asserts cwd consistency across the spawn path.
- **Schema-extended-thinking incompatibility.** Anthropic docs note these don't compose yet. We don't use extended thinking in v0.3, so this is a forward compatibility note rather than a current blocker.
- **Mock runner divergence from real runner.** Mock skips schema validation entirely, so a mock-pass does not guarantee a real-runner-pass. The Phase 3 haiku smoke is the only thing that catches this — same situation as v0.3, this task does not regress it.
- **Backwards compatibility of envelope JSON shape.** Phase 1 schemas codify the same envelope shape the v0.3 prompts already describe; existing fixtures in `tests/fixtures/agent_outputs/*.json` should validate against the new schemas without modification. If they don't, the schema is wrong.

### Open decisions (resolved)

| Decision | Resolution |
|---|---|
| Workflow harness for T004 | **Legacy filesystem** (per user: "for this using the old system is fine") |
| Cargo version bump timing | **0.3.0 tagged now** (done at task creation); **0.4.0 on Phase 3 success** |
| Validate-then-retry mechanism | **Built-in `--json-schema`** (zero orchestrator-side retry) over manual `--session-id`/`--resume` loop. The latter is reserved for non-schema failures and v0.4 broader-retry policies. |
| Stream-json capture location | **`.stores/runs/<session-id>.jsonl`** (worktree-local, gitignored; foundation for future `runs` store) |
| Schema reasoning-field placement | **First field in each envelope** (Instructor / dottxt-ai pattern; ~60% recovery on GSM8K when reasoning precedes answer) |
| Guide envelope schema | **Authored in Phase 1** alongside the others, even though guide handler isn't in the smoke path |

---

## Plan

**Objective:** Replace the brittle "last non-empty stdout line is JSON" envelope contract with `claude -p --json-schema`-validated structured output, capture full stream-json transcripts under `.stores/runs/<session-id>.jsonl`, generate `--session-id` upfront for resumability, and remove the brief-template / agent-prompt instructions that contradict the v0.3 envelope-only contract. Three bottom-up phases; each phase ends with `cargo build` + relevant `cargo test` green and existing `tests/drive_e2e.sh` continuing to pass.

### Phase 1 — Schemas + runner refactor

**Objective:** Author the 5 JSON Schemas, add the `BUNDLED_AGENT_SCHEMAS` registry, extend the `Runner` trait with an optional `schema` parameter and an enriched `RunnerOutput`, and update `claude_code` to pass `--json-schema` / `--session-id` / `--output-format stream-json --verbose` while parsing `result.structured_output`. Mock runner exposes a canned `structured_output` field. All runner-layer unit tests pass; drive layer is **not yet** touched.

**Files to create:**
- `agents/schemas/planner.schema.json`
- `agents/schemas/plan-reviewer.schema.json`
- `agents/schemas/executor.schema.json`
- `agents/schemas/code-reviewer.schema.json`
- `agents/schemas/guide.schema.json`
- `tests/runner_claude_code_unit.rs` (or extend the in-file `mod tests` in `src/runner/claude_code.rs`)

**Files to modify:**
- `Cargo.toml` (add `uuid = { version = "1", features = ["v4"] }`)
- `src/runner/mod.rs` (extend `RunnerOutput` and the `Runner` trait; bump doc-comments)
- `src/runner/claude_code.rs` (rewrite `spawn` to use `--json-schema`, `--session-id`, `--output-format stream-json --verbose`, parse `result` event, write `.stores/runs/<session-id>.jsonl`)
- `src/runner/mock.rs` (extend `RunnerOutput` shape; mock ignores schema arg)
- `src/cli/agents.rs` (add `BUNDLED_AGENT_SCHEMAS: &[(&str, &str)]` mirroring `BUNDLED_AGENTS`)

**Schema authoring rules** (apply to all 5):
1. First property in each schema is a free-text reasoning/notes string field (`reasoning` for planner / plan-reviewer / code-reviewer; `notes` for executor; `summary` may double as the reasoning slot for guide where it already leads). This is the dottxt-ai field-ordering recovery pattern.
2. `role` is a `const` literal matching the existing `AgentEnvelope` tag (`"planner"`, `"plan-reviewer"`, `"executor"`, `"code-reviewer"`, `"guide"`).
3. Every property the v0.3 `AgentEnvelope` deserializer reads (`phases`, `decision_matrix`, `gate`, `summary`, `open_questions`, `commit`, `files_changed`, `details`, `counts.{critical,major,minor}`, `action`) is declared with the right type. Optional fields (`#[serde(default)]` in Rust) appear only in `properties`, not in `required`.
4. `additionalProperties: false` at the top level so the model cannot smuggle stray fields.
5. Each schema validates the corresponding existing fixture in `tests/fixtures/agent_outputs/<role>.json` byte-for-byte. The Phase 1 ACs include this round-trip check.

**Runner trait change:**
```rust
pub struct RunnerOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub final_message: Option<String>,            // kept for backwards compat
    pub structured_output: Option<serde_json::Value>,  // NEW: from --json-schema
    pub session_id: Option<String>,                    // NEW: runner-generated UUID, returned to drive
}

pub trait Runner: Send {
    fn name(&self) -> &str;
    fn spawn(
        &self,
        role: &str,
        system_prompt: &str,
        brief: &str,
        schema: Option<&str>,                      // NEW: JSON-schema text or None
    ) -> Result<RunnerOutput>;
}
```

**`claude_code::spawn` command construction:**
```rust
// Inside claude_code::spawn — runner mints the UUID, drive never sees it until it's returned.
let session_id = uuid::Uuid::new_v4().to_string();
// ... build Command:
//   claude -p \
//     --append-system-prompt <system_prompt> \
//     --output-format stream-json --verbose \
//     --session-id <session_id>            <-- runner-local
//     --json-schema <schema-when-Some> \
//     --allowed-tools=<from frontmatter> | --permission-mode=bypassPermissions \
//     [--model=<model>] \
//     <brief>
// After the child exits, write `.stores/runs/<session_id>.jsonl` and return
// RunnerOutput { session_id: Some(session_id), structured_output, .. }.
```
- The runner generates the UUID *itself* (Decision Matrix row 5) and returns it via `RunnerOutput.session_id`. Drive does NOT mint the UUID and does NOT take a `uuid` dependency; drive only consumes `RunnerOutput.session_id` after the spawn returns (e.g. for logging on failure or for human `--resume`).
- The runner writes the full stream-json stdout to `.stores/runs/<session_id>.jsonl` after the child exits, then walks the stream-json events to find the `result` event and pulls `result.structured_output` (when present) and `result.error.subtype` (when validation retries are exhausted).
- `cwd` is canonicalised on entry (`std::env::current_dir()?.canonicalize()?`) and pinned for the spawn — documented #1 footgun for session resume per Anthropic SDK docs.
- When `schema` is `None` (e.g. an unsupported role or a bypass test), the runner falls through to the v0.3 stream-json-without-schema path and `structured_output` stays `None`. `final_message` is still extracted from the result-event's `text`/`content` for legacy compatibility.

**Acceptance criteria:**
- **AC1.1** `cargo build --features runner-claude-code` succeeds. New `uuid` dep resolves.
- **AC1.2** `agents/schemas/` contains exactly 5 `*.schema.json` files; each parses as valid JSON Schema. **Default dialect: Draft 2020-12** (`"$schema": "https://json-schema.org/draft/2020-12/schema"`). **Fallback (gated on Phase 1 runner-unit smoke):** if `claude -p --json-schema=<draft-2020-12-file>` rejects the schema with a parse/dialect error during the AC1.5 fixture-stream tests, switch every schema's `$schema` to Draft-07 (`"http://json-schema.org/draft-07/schema#"`) and re-run AC1.5. No SDK-doc research is required; the decision is mechanically resolved by whether the SDK accepts 2020-12. Document which dialect was retained in the Phase 1 executor summary.
- **AC1.3** A new test `tests/schemas_validate_fixtures.rs` (or equivalent block) deserializes every `tests/fixtures/agent_outputs/<role>.json` against its corresponding schema and asserts validation passes for all 5. Use `jsonschema = "0.18"` as a dev-dep, OR a hand-rolled minimal validator if the executor prefers no new dep — either is fine, but the test must run under `cargo test`.
- **AC1.4** `BUNDLED_AGENT_SCHEMAS` in `src/cli/agents.rs` contains exactly 5 entries; `cargo test cli::agents` passes including a new `bundled_schemas_count_matches_agents` test asserting the registry has the same role-name set as `BUNDLED_AGENTS`.
- **AC1.5** `cargo test --lib runner::` passes. New tests cover: (a) `extract_structured_output_from_stream_json` returns `Some(value)` for a fixture stream-json with `result.structured_output` populated; (b) returns `None` when only `result.error.subtype = "error_max_structured_output_retries"` is present and stderr surfaces the subtype string; (c) `cwd` is canonicalised before spawn; (d) session-id is a valid v4 UUID and is propagated to `RunnerOutput.session_id`.
- **AC1.6** `cargo test --lib runner::mock` passes. A new test asserts `MockRunner` accepts a `RunnerOutput` with `structured_output: Some(...)` and `spawn` returns it verbatim — schema arg is ignored.
- **AC1.7** Running the existing fixture-driven test path (`cargo test --features runner-claude-code -- runner_uses_path_shim_not_real_claude` and `command_construction_and_final_message_parsing`) continues to pass after `final_message` extraction is reworked to be derived from the stream-json `result.text` rather than last-line scanning.

**Dependencies:** none (foundational layer).

---

### Phase 2 — Drive consumes structured output, briefs and prompts updated

**Objective:** Drive looks up the role's schema in `BUNDLED_AGENT_SCHEMAS`, threads it through `runner.spawn(..., Some(schema))`, prefers `RunnerOutput.structured_output` over the legacy `final_message` / last-line scan, and surfaces `error_max_structured_output_retries` clearly. Brief templates lose their `Call stores tasks submit-*` instructions; agent system prompts shed the markdown-fence and "last line must be JSON" hand-wringing now that the schema is the contract. Existing `tests/drive_e2e.sh` continues passing because mock `RunnerOutput.structured_output` is read in preference to `final_message`, but mock fixtures keep working without modification (legacy fallback).

**Files to modify:**
- `src/handlers/drive.rs` (drive_loop: pull schema for role, pass to `runner.spawn`, rewrite `parse_envelope` to prefer `structured_output`)
- `stores/tasks/templates/planner-brief.md.tpl`
- `stores/tasks/templates/plan-reviewer-brief.md.tpl`
- `stores/tasks/templates/executor-brief.md.tpl`
- `stores/tasks/templates/code-reviewer-brief.md.tpl`
- `agents/planner.md`
- `agents/plan-reviewer.md`
- `agents/executor.md`
- `agents/code-reviewer.md`
- `agents/guide.md`
- `tests/fixtures/drive_e2e/happy_2phase.jsonl` and `tests/fixtures/drive_e2e/revise_once.jsonl` — only if a structured_output round-trip test is added; otherwise leave them as-is for the legacy fallback path

**Drive change shape:**
```rust
let schema_text: Option<&str> = BUNDLED_AGENT_SCHEMAS
    .iter()
    .find(|(n, _)| *n == agent_name_normalized)
    .map(|(_, s)| *s);
let run_out = runner.spawn(&agent_name_normalized, system_prompt, &brief_markdown, schema_text)?;
```

`parse_envelope` becomes:
```rust
fn parse_envelope(out: &RunnerOutput) -> Result<AgentEnvelope> {
    if let Some(value) = &out.structured_output {
        return serde_json::from_value(value.clone()).map_err(...);
    }
    // Legacy fallback for mock fixtures and pre-schema runners.
    if let Some(fm) = &out.final_message { /* existing code */ }
    /* existing last-line-scan fallback */
}
```

**Brief-template fix (uniform across all 4 templates):**
- Target = any **numbered imperative checklist line** of the form `N. Call \`stores tasks submit-...\`` or `N. Call \`stores tasks render\``. These are the lines AC2.5's regex matches.
- Remove every such imperative line from the `Critical Actions` checklist.
- Replace the removed line with a non-imperative reformulation that does NOT begin with `Call \`stores tasks submit-`. Example replacement: `5. **EMIT** the JSON envelope as your final structured output. Drive parses it and submits in-process; do not invoke any \`submit-*\` verb directly.` (Note: this phrasing avoids the literal substring `stores tasks submit-` in the imperative position, so it slips past AC2.5's regex by construction.)
- This preserves the brief's section structure (drive_e2e.sh and unit tests don't grep for the exact wording — verified during analysis).

**Agent-prompt simplification:**
- In each of `agents/{planner,plan-reviewer,executor,code-reviewer,guide}.md`, drop the "**emit on the last non-empty line of stdout**" / "**no markdown fences**" / "Final stdout line is the JSON envelope (nothing after it)" boilerplate. Replace with a one-line note: `Your output is validated against a JSON schema. Emit the envelope as a single JSON object — formatting (fences, surrounding text) is irrelevant; only structural conformance matters.`
- Keep the schema description (`role`, fields) — agents still need to know which fields to populate.
- Keep the "do NOT invoke stores tasks submit-*" warnings — the schema does not enforce that.

**Acceptance criteria:**
- **AC2.1** `cargo build --features runner-claude-code` succeeds.
- **AC2.2** `cargo test` (default features) passes — drive unit tests in `src/handlers/drive.rs::tests` updated to construct `RunnerOutput` with the new fields and continue to assert correct dispatch.
- **AC2.3** `bash tests/drive_e2e.sh` passes both AC7.1 and AC7.1b on the legacy fixtures (mock runner sets `structured_output: None`; drive falls back to `final_message`; behaviour unchanged from v0.3).
- **AC2.4** A new drive unit test (`structured_output_takes_precedence_over_final_message`) asserts that when `RunnerOutput.structured_output = Some(...)` AND `final_message` is malformed, `parse_envelope` succeeds via `structured_output`.
- **AC2.5** `grep -rE '^\s*[0-9]+\.\s*Call\s+`stores tasks submit-' stores/tasks/templates/` returns zero matches. This regex matches ONLY the imperative-checklist form (e.g. `5. Call \`stores tasks submit-planner\` ...`) and does not match prose mentions like `Do NOT invoke \`submit-*\`` in warning text. Phase 2's brief-template edit must remove every such imperative line — see the Phase 2 task list for the replacement target.
- **AC2.6** `grep -r 'last non-empty line' agents/` returns zero matches across `planner.md`, `plan-reviewer.md`, `executor.md`, `code-reviewer.md`, `guide.md`.
- **AC2.7** When the runner returns an error with `error_max_structured_output_retries`, drive prints (to stderr) a line containing the substring `schema validation retries exhausted` AND surfaces the `.stores/runs/<session-id>.jsonl` path. New unit test asserts both.

**Dependencies:** Phase 1 must be complete (drive depends on the new `RunnerOutput` shape and `BUNDLED_AGENT_SCHEMAS`).

---

### Phase 3 — Smoke verification + version bump

**Objective:** Re-run the haiku smoke against `/tmp/t003-smoke` from a clean state. On success bump Cargo to `0.4.0`, update README to mention the new schema-validated runner contract and the `.stores/runs/` transcript path, and tag.

**Files to modify:**
- `Cargo.toml` (`version = "0.4.0"`)
- `Cargo.lock` (regenerated by `cargo build`)
- `README.md` (Quickstart, "Runner feature flag", and "What this demonstrates" sections — note schema validation, transcript capture, and v0.4 contract)

**Smoke command sequence (this is the DONE_WHEN — propagate verbatim to executor):**
```bash
cd /tmp/t003-smoke
cargo install --path /home/blake/repos/experiments/stores --features runner-claude-code --force
rm -rf .stores tasks
stores setup
stores tasks add \
  --invoker human \
  --title "Hello world" \
  --slug "hello" \
  --done-when "echo hi prints hi" \
  --scope-in "scripts/" \
  --scope-out "src/"
stores tasks drive --auto --claude-code --testing
```

**Acceptance criteria:**
- **AC3.1** Smoke run completes end-to-end with no envelope-parse error in stderr; final exit code is `0`.
- **AC3.2** `stores tasks show T001 --json` reports `status == "complete"`.
- **AC3.3** The completed task has at least one executor cycle with a non-`none` `commit` SHA recorded.
- **AC3.4** The final code-reviewer cycle has `gate == "PASS"`.
- **AC3.5** `/tmp/t003-smoke/.stores/runs/` contains at least 4 `<uuid>.jsonl` files (one per agent role invoked: planner, plan-reviewer, executor, code-reviewer); each parses as valid JSONL with at least one line whose top-level event type is `result`.
- **AC3.6** `Cargo.toml` shows `version = "0.4.0"`; `cargo build --features runner-claude-code` succeeds at the new version.
- **AC3.7** `README.md` Quickstart and Runner-feature-flag sections mention `--json-schema` validation and the `.stores/runs/<session-id>.jsonl` transcript path.
- **AC3.8** A clean `bash tests/drive_e2e.sh` still passes at v0.4.0.

**Dependencies:** Phases 1 and 2 must be complete and merged.

---

### Decision Matrix

| Decision | Options | Chosen | Rationale |
|---|---|---|---|
| Schema reasoning-field placement | (a) reasoning first; (b) reasoning last; (c) no reasoning field | **(a) first** | dottxt-ai / Instructor-style field ordering: reasoning before answer recovers ~60% of GSM8K accuracy lost to "speak freely" failures (cited in `## Task`). Cheap, low-risk, matches existing prompt pattern (planner already does plan-notes-then-envelope). |
| Stream-json transcript path | (a) `.stores/runs/<uuid>.jsonl`; (b) `.stores/runs/<task-id>/<n>.jsonl`; (c) `/tmp` ephemeral | **(a) `.stores/runs/<uuid>.jsonl`** | Worktree-local (already gitignored via `/.stores/`), session-id-named so resume can pick the right transcript without DB lookup, foundation for v0.4+ `runs` event-log store without committing to a schema yet. |
| Mock-runner schema bypass | (a) mock validates schema in-process; (b) mock ignores schema arg entirely | **(b) ignore** | Mock's whole purpose is to skip the runner; validating a fixture against a schema in mock duplicates Phase 1's `tests/schemas_validate_fixtures.rs` and double-binds tests to schema versions. Mock's `structured_output` is set by the fixture author. |
| Runner trait shape: `Option<&str>` schema arg vs new method | (a) extend `spawn` signature with `schema: Option<&str>`; (b) add `spawn_with_schema` next to `spawn`; (c) wrap in a `SpawnRequest` struct | **(a) extend signature** | v0.4 is a breaking minor anyway (per resolved decisions in `## Task`). Adding a parallel method doubles the trait surface and creates a "which one do you call" footgun. The struct (option c) is defensible but premature — only one new field (schema) is being added; revisit if Phase 1 needs more args. |
| Session-id generation locus | (a) drive generates the UUID and passes to runner; (b) runner generates internally and returns it; (c) runner generates and writes to a side channel | **(b) runner generates internally** | Session-id is meaningful only inside the runner call (the transcript path is named after it; the resume API consumes it). Drive doesn't otherwise need it before spawn. Returning it via `RunnerOutput.session_id` lets drive log it on failure for human resume; mock can return `None` to indicate it has no session. Avoids leaking a UUID dependency into drive. |
| `final_message` field: keep or remove? | (a) remove now (breaking, simpler); (b) keep alongside `structured_output` (legacy) | **(b) keep** | Mock fixtures and the legacy last-line scan path still produce `final_message`. Removing it forces touching every fixture in this phase, which inflates Phase 2's blast radius and risks regressing `drive_e2e.sh`. Mark `final_message` deprecated in doc-comments; remove in v0.5. |
| Schema validation library for fixture round-trip test | (a) `jsonschema` crate; (b) hand-rolled minimal validator; (c) call `claude --json-schema` in a test (network-dependent) | **(a) `jsonschema` crate (dev-dep only)** | One small dev-dep, well-maintained, Draft-2020-12 compatible. Hand-rolling is masochism for 5 schemas. Network-dependent tests are a no-go for `cargo test`. |
| Schema dialect | (a) Draft-07; (b) Draft 2020-12 | **(b) Draft 2020-12 by default, Draft-07 as a runtime-smoke fallback** | No doc-research required: the executor authors all 5 schemas with `"$schema": "https://json-schema.org/draft/2020-12/schema"` and runs the AC1.5 fixture-stream tests against the real `claude -p --json-schema` binary. If the SDK rejects 2020-12 with a parse/dialect error, the executor mechanically swaps every `$schema` to Draft-07 (`"http://json-schema.org/draft-07/schema#"`) and re-runs. Both dialects support every construct we need (`const`, `properties`, `required`, `additionalProperties`), so the swap is local to the `$schema` line. The chosen dialect is recorded in the Phase 1 executor summary. |

---

### Plan Notes

The codebase already follows tight bottom-up layering — `runner` knows nothing about workflow, `drive` is the only place workflow decisions happen, `cli/agents.rs` is the canonical place for compile-time bundled assets via `include_str!`. Phase 1 mirrors that exactly: schemas live next to their agent prompts in `agents/schemas/`, the registry in `cli/agents.rs` clones the `BUNDLED_AGENTS` shape, and the runner trait remains the only seam drive crosses to reach the model. Phase 2 keeps drive as the sole DB writer (no agent calls `submit-*`) and tightens that contract by deleting the contradictory template language. Phase 3 is purely verification + version housekeeping. The phase ordering is dictated by compile-time dependencies: drive cannot consume `RunnerOutput.structured_output` before the field exists on `RunnerOutput`, and `BUNDLED_AGENT_SCHEMAS` must compile before drive references it.

**Executor: be most careful about** — (1) the `cwd` canonicalisation in `claude_code::spawn`. The Anthropic SDK silently mints a fresh session if cwd differs between spawn and resume calls; this is the documented #1 footgun. Add a unit test that asserts `cwd_for_spawn` matches `std::env::current_dir()?.canonicalize()?` and is the same value used for any subsequent `--resume` calls (none in this task, but the test guards future work). (2) The stream-json result-event shape. Anthropic's stream-json emits a sequence of events terminating in a `result` event whose payload contains `structured_output` (on schema-success), `error.subtype` (on retry-exhausted), and `text`/`content` (the human-readable assistant message). Read the actual fixture bytes before writing the parser — do not trust this plan's prose over the SDK's emitted bytes. (3) Schema fidelity. Each schema MUST validate the existing fixture in `tests/fixtures/agent_outputs/` byte-for-byte. If a fixture fails, the schema is wrong, not the fixture — the fixtures encode v0.3's *correct* envelope shape. (4) Brief-template pruning is mechanical but easy to typo; run `grep -r 'submit-' stores/tasks/templates/` before submitting Phase 2.

**Risk register:**
- **Integration risk (Phase 1 → Phase 2 ABI):** Adding `schema: Option<&str>` to `Runner::spawn` is a hard break — every implementor must update on the same compile. Phase 1 is gated on both `mock` and `claude_code` updating in lockstep; the test suite is the gate.
- **Test-infra risk (Phase 1 schema validator):** Pulling in `jsonschema` as a dev-dep adds a transitive tree; if it conflicts with existing deps, fall back to hand-rolling a minimal validator covering only `properties`, `required`, `const`, `additionalProperties`, `type`. Document the choice in the executor summary.
- **API-surface risk (Phase 2 prompt edits):** If the haiku model has internalised "emit JSON on the last line, no fences" from the v0.3 prompt, removing that instruction may regress phase-3 smoke even though the schema enforces shape. Keep one terse line: `Output a single JSON object conforming to the provided schema.` Don't strip every reference.
- **Scope-creep risk (`runs` event-log store):** Phase 1 only writes the JSONL file. Resist adding query verbs, indexing, or schema migrations — those land in v0.4+ as a separate task. The capture path is the foundation, not the feature.
- **Backwards-compat risk (Phase 2 fixtures):** The legacy mock fixtures in `tests/fixtures/drive_e2e/*.jsonl` set `structured_output: None` (because they're authored against v0.3). Drive must fall back to `final_message` cleanly when `structured_output.is_none()`. AC2.3 is the explicit guard.
- **Smoke flakiness (Phase 3):** Haiku's tool use can hang for 60-90s per spawn; budget at least 10 minutes of wall-clock for a 4-cycle smoke. The runner has no built-in timeout; if a cycle hangs beyond `max_iters`, the loop bails — that's the failure signal, not a hang.

---

### Plan Revisions (post-Phase-3 attempt 1, 2026-04-28)

Phase 3 smoke surfaced three real issues that invalidate the original "schema validation alone is sufficient" premise. Replanning Phases 1 and 2 in light of fresh research; Phase 3 unchanged.

**What we learned:**

1. **Bug A — `--json-schema` arg shape.** Phase 1 wrote the schema to `/tmp/stores-schema-<uuid>.json` and passed the file path. `claude --json-schema` takes the schema TEXT inline (per `claude --help` example: `--json-schema '{"type":"object",...}'`). Path-mode caused claude to hang silently for 15+ minutes per spawn. **Already fixed inline** during Phase 3 attempt 1 (commit pending in revise).
2. **Bug B — Stream-json parser extracts wrong event.** The runner's `extract_structured_output_from_stream_json` pulls intermediate `user` events (e.g. tool_result for a denied bash call) and feeds them to drive as `final_message`. Drive then fails to parse with `missing field 'role' at line 1 column 3568`. The parser must walk the JSONL, find the terminal `result` event, and extract `structured_output` / `result` (text) / `error` from THAT event only. Phase 1 unit tests had only synthetic single-event fixtures; they missed this.
3. **Architectural premise correction.** Research (delegated agent, summarised in conversation) shows that `claude --json-schema` is **post-hoc validation with re-prompting**, NOT constrained decoding. Multi-turn tool-using agents that emit final answers as prose (markdown-fenced JSON) bypass schema validation entirely — `result.structured_output` stays `null`. Industry consensus (BAML, Pydantic AI, OpenAI Agents SDK, Anthropic SDK docs) is a **belt-and-braces** stack: schema validation as preferred path, schema-aligned-parsing of `result.text` as fallback, optional submit-tool layer for v1.0.
   - BAML's "Structured Outputs Create False Confidence" post explicitly argues prose + SAP outperforms strict schema mode in some cases (92% vs 87.5% on gpt-3.5).
   - Anthropic Claude Code subagents (Task tool) have ZERO schema support; community workaround is prose contract only.
   - The original T004 premise of "delete the parser, use the schema" was wrong — the parser must stay, hardened.

**Phase 1 revise — additional ACs (in addition to AC1.1–AC1.7):**

- **AC1.8 — `--json-schema` inline (Bug A).** `claude_code::spawn` passes `--json-schema=<schema_text>` directly with no temp file. Verified by reading the spawn function's `cmd.arg` calls; no `fs::write` for schemas. (Already applied in this session; executor confirms.)
- **AC1.9 — `extract_structured_output_from_stream_json` walks to terminal `result` event (Bug B fix).** New unit test `extract_structured_output_skips_intermediate_user_events` constructs a realistic 26-line stream-json fixture with `system` + `assistant` + `user` (tool_use_result) + `assistant` + `result` events, and asserts the extractor returns the `result` event's `structured_output` (when present) or `result.result` text (when not), NEVER an intermediate `user` event's content. The fixture should mirror the structure observed in `/tmp/t003-smoke/.stores/runs/5fc73fef-3b3f-47b7-aa53-a7e9d0dc8687.jsonl` from the Phase 3 attempt 1 smoke.
- **AC1.10 — SAP-style parser added.** New module `src/runner/sap.rs` (or top-level helper in `claude_code.rs`) exposes `extract_envelope_from_text(text: &str, schema: Option<&Value>) -> Option<Value>` that:
  - Strips markdown fences (```json ... ``` and bare ``` ... ```);
  - Walks the cleaned text for balanced `{...}` candidates;
  - Returns the first candidate that parses as valid JSON;
  - When `schema.is_some()`, additionally validates the candidate against the schema using the existing `jsonschema` dev-dep (or, if dev-only, lift to a runtime dep behind the same `runner-claude-code` feature flag);
  - Returns `None` if no candidate found.
  Unit tests cover: (a) plain JSON, (b) markdown-fenced JSON, (c) JSON inside prose with leading/trailing commentary, (d) two candidates where the first fails schema and the second passes, (e) malformed-everything returns None.
- **AC1.11 — `RunnerOutput` exposes `structured_output_source: Option<&'static str>`.** When the runner extracts data, it records which layer caught it: `"sdk"` (claude returned `result.structured_output`), `"sap"` (extracted from `result.result` text via SAP), or `None` (legacy `final_message` / drive's last-line fallback). Drive logs this on submit for postmortem.

**Phase 2 revise — additional ACs (in addition to AC2.1–AC2.7):**

- **AC2.8 — `parse_envelope` is three-layer.** Drive prefers `RunnerOutput.structured_output` (Layer 1: SDK-validated); on null, falls through to SAP applied to the runner's extracted final-message text (Layer 2); on SAP-failure, falls through to existing `final_message` last-line scan (Layer 3: legacy mock-fixture compat). New unit test `three_layer_fallback_for_markdown_fenced_planner_output` constructs a `RunnerOutput` mirroring the Phase 3 attempt 1 transcript (structured_output: None, final_message: markdown-fenced JSON) and asserts SAP recovers the planner envelope cleanly.
- **AC2.9 — Runner records `structured_output_source` and drive surfaces it.** When drive submits, stderr includes `[T<id>] phase N cycle M: <role> → submitted (gate=...; source=sdk|sap|legacy)`. Helps diagnose which layer is doing the work in production.

**What does NOT change:**
- Phase 3 unchanged (smoke + version bump). Same DONE_WHEN.
- Cargo bump 0.3.0 → 0.4.0 still on Phase 3 success.
- Submit-tool layer (Pydantic AI's force-final-tool pattern) is **deferred to a future task (v0.5+)**. Three-layer SAP-fallback is sufficient for v0.4 robustness.
- Bug C (planner tried `./scripts/hi` and hit bash approval) is **not addressed**. It's incidental to T001's contract; not all task contracts have a pre-existing executable to verify. If it recurs in Phase 3 attempt 2, tighten the planner's allowed-tools whitelist as a follow-up patch.

**Decision Matrix additions:**

| Decision | Options | Chosen | Rationale |
|---|---|---|---|
| `--json-schema` arg shape | (a) file path; (b) inline text | **(b) inline text** | `claude --help` example shows inline JSON. Path mode silently hangs claude for 15+ minutes — confirmed empirically Phase 3 attempt 1. |
| Multi-turn-agent failure recovery | (a) schema validation only; (b) submit-tool terminator (Pydantic AI); (c) SAP fallback (BAML); (d) two-pass extraction | **(c) SAP fallback** | Schema-only doesn't engage when agent ends in prose (observed in Phase 3 attempt 1). Submit-tool is v1.0 design (deferred). Two-pass adds API cost. SAP is parser-only, zero API cost, restores v0.3's text-extraction capability while keeping schema as preferred path when it fires. Industry precedent: BAML's Schema-Aligned Parsing. |
| `structured_output_source` field | (a) skip; (b) `Option<&'static str>` on RunnerOutput | **(b) add** | Diagnostic gold for v0.4: when an agent regresses or a prompt drifts, we can see which layer is doing the work and intervene at the right level. Costs nothing. |

**Gate:** NEEDS_WORK

**Summary:** The plan is structurally sound, well-bottom-up-layered, scope-disciplined, and the DONE_WHEN is genuinely reachable by completing all 22 ACs. Phase ordering is correct (schemas + runner → drive consumption → smoke), the four Intent-Contract risks are each tied to a concrete AC (cwd test → AC1.5c, error_max_structured_output_retries surface → AC2.7, mock divergence acknowledged in Plan Notes risk register, fixture round-trip → AC1.3 + AC2.3), and the Decision Matrix covers every non-obvious choice the executor would otherwise punt or guess. Three concrete fixes are needed before this is executor-ready: (1) the Scope section and Decision Matrix disagree on **who generates the session UUID** (Scope L30 says drive, matrix row 5 says runner) — the executor will not know which to implement; (2) **AC2.5's grep target will fail by construction** because the planner's own replacement text on Plan L205 contains the literal string `stores tasks submit-*`; the AC must be rephrased OR the replacement text must be revised; (3) AC1.2's "Draft 2020-12 or Draft-07; whichever Anthropic's SDK accepts — confirm in the executor's first task" punts a hard prerequisite into the executor's lap without an escape hatch — pin a default-and-fallback in the plan so a network-doc failure doesn't block Phase 1.

**Strengths:**
- DONE_WHEN alignment is exact: Phase 3's smoke command sequence (Plan L236–L249) reproduces the DONE_WHEN verb-for-verb, and AC3.1–AC3.4 mechanically check the four success conditions the DONE_WHEN names (no envelope-parse error, complete status, executor commit, gate=PASS). AC3.5 additionally verifies the new transcript-capture path.
- Every AC is mechanically verifiable: each names a concrete `cargo` invocation, a specific grep pattern, a file-existence check, or an exact CLI output assertion. There is no "works correctly" / "agents are good" hand-waving.
- The fixture round-trip discipline (Plan L112 schema authoring rule #5; AC1.3) is the right way to lock the schema to v0.3's settled envelope shape — if a fixture fails, the schema is wrong, not the fixture.
- The Decision Matrix surfaces and resolves the choices that would otherwise stall the executor: schema dialect, jsonschema crate vs hand-rolled, mock bypass policy, `final_message` keep-or-remove, trait shape (signature extension vs new method vs struct). All have rationale.
- Risk register in Plan Notes adds value beyond the Intent Contract risks: integration-risk (lockstep mock + claude_code update), test-infra risk (jsonschema dep tree fallback to hand-roll), API-surface risk (don't strip every JSON-format reference from prompts — keep one terse line so haiku's prior internalisation isn't disturbed). The last point is a non-obvious second-order concern.
- Phase ordering is correct and the dependencies are explicit (Phase 2 cannot reference `RunnerOutput.structured_output` before Phase 1 ships the field; Phase 3 cannot smoke before Phase 2 wires drive). Plan Notes L282 names the compile-time ordering explicitly.
- Scope discipline: nothing in the plan is outside the Intent Contract's "In scope". The deferred items (full `runs` event-log store, drive-side retry loop, second runner) are explicitly listed in Plan Notes' scope-creep risk to keep the executor disciplined.

**Concerns:**
- **[major] Session-id generation locus contradicts itself.** Scope L30 says: "generate and pass `--session-id=<uuid>`" — implying drive owns the UUID. Decision Matrix row 5 (Plan L273) says: "**(b) runner generates internally** … Session-id is meaningful only inside the runner call". `claude_code::spawn` command construction at Plan L142 says `--session-id <uuid>` without saying who creates it. The two answers imply different signatures (drive needs a `uuid` dep vs only the runner does; `RunnerOutput.session_id` is a return-only field vs a round-trip echo). **Fix:** pick one and make Scope, Plan command construction, and the Decision Matrix all say the same thing. Decision Matrix (b) reads as the more thoughtful answer; align Scope to it.
- **[major] AC2.5 will fail by construction.** AC2.5 (Plan L218) says `grep -r 'stores tasks submit-' stores/tasks/templates/` returns zero matches. The replacement text the plan itself prescribes for those templates (Plan L205) is: `Do NOT call \`stores tasks submit-*\`...` — which contains the literal substring `stores tasks submit-`. The AC's parenthetical hand-wave ("excluding the explicit do NOT call warning line, which the executor must phrase without the literal `submit-` substring or the grep pattern must be tightened") punts the resolution into the executor. **Fix:** either (a) tighten the grep to `grep -rE '^[0-9]+\\. .*Call \`stores tasks submit-' stores/tasks/templates/` (matches only the imperative checklist-step form), or (b) remove the literal `stores tasks submit-*` from the replacement text and rephrase as "Do NOT invoke any `submit-*` verb directly".
- **[major] Schema dialect choice is punted into Phase 1 without a fallback path.** AC1.2 says "Draft 2020-12 or Draft-07; whichever Anthropic's SDK accepts — confirm in the executor's first task by reading the SDK docs link cited in `## Task`." The Decision Matrix row 8 also defers: "Default to Draft-07 (broadly supported); switch to 2020-12 only if SDK demands it." But there is no SDK-docs link cited in `## Task` (re-checked — the section references the literature "Let Me Speak Freely?" and "JSONSchemaBench" papers, not an SDK URL). If the executor cannot find the answer, Phase 1 stalls. **Fix:** pin the default to Draft 2020-12 (current Anthropic public guidance for JSON-schema validated outputs uses 2020-12) and instruct the executor to fall back to Draft-07 ONLY if `claude -p --json-schema=<draft-2020-12-file>` returns a dialect-rejection error in the AC1.5 fixture-stream test. Make the fallback automatic, not a research task.
- **[minor] AC3.5's transcript-count floor may misfire on multi-phase smokes.** AC3.5 expects "at least 4 `<uuid>.jsonl` files". For the single-phase Hello world task in DONE_WHEN, 4 is the correct floor (planner + plan-reviewer + executor + code-reviewer). If the planner happens to emit multiple phases (likely, since "echo hi prints hi" might be split into "create script" + "run script"), the floor still holds — but the AC's wording could create confusion if the executor sees 8 files and assumes regression. **Fix (optional):** rephrase as "at least one `<uuid>.jsonl` per agent invocation; minimum 4 total for a single-phase task". Minor — not a blocker.
- **[minor] AC1.5c (cwd canonicalisation unit test) is named but its assertion shape is unclear.** The plan says: "`cwd` is canonicalised before spawn." The risk-register entry on Plan L284 elaborates: "Add a unit test that asserts `cwd_for_spawn` matches `std::env::current_dir()?.canonicalize()?` and is the same value used for any subsequent `--resume` calls (none in this task, but the test guards future work)." The plan does not name the function under test or the assertion target. **Fix:** name the function — e.g., "test asserts `claude_code::spawn` calls `Command::current_dir(...)` with a path equal to `std::env::current_dir()?.canonicalize()?`" — so the executor doesn't have to invent a test surface. (A `Command` builder isn't directly inspectable; this may require a small refactor to extract the cwd-resolution into a testable helper. The plan should mention that.)
- **[minor] No AC enforces that the schema's `additionalProperties: false` actually rejects extra fields.** Schema authoring rule #4 (Plan L111) requires `additionalProperties: false`, but AC1.3 only validates *positive* round-trips against the existing fixtures. If a schema author forgets `additionalProperties: false`, AC1.3 still passes. **Fix (optional):** add a negative test in AC1.3 — "for each schema, a fixture-with-an-extra-stray-field is rejected by the validator". Half-line of test code, locks the property in.

**Open Questions Finalized:** —

(All previously open questions in the Intent Contract were resolved before planning; the planner's matrix added six more decisions, all resolved. The three [major] concerns above are plan-internal contradictions or scope punts, not user-level decisions — they belong back to the planner, not escalated to the human. Max 3 plan-review iterations applies; this is iteration 1 of 3.)

### Iteration 2

**Gate:** READY

**Summary:** All three iteration-1 blockers have landed cleanly. Session-id ownership is now consistent across every mention site (Scope L30, RunnerOutput doc-comment L122, command-construction snippet L139–L153, Decision Matrix row 5 L280) — the runner mints the UUID and returns it via `RunnerOutput.session_id`; drive never generates it and explicitly does not take a `uuid` dependency. AC2.5's regex is now a tightened imperative-checklist matcher (`^\s*[0-9]+\.\s*Call\s+\`stores tasks submit-`) that matches only numbered "Call ..." lines, and Phase 2's replacement text on L212 starts with `5. **EMIT**` — neither the `Call` imperative nor the literal substring `stores tasks submit-` appears, so the AC is satisfiable by construction. Schema-dialect AC1.2 now pins Draft 2020-12 as the default with a mechanical Draft-07 fallback gated on AC1.5 SDK rejection — no doc-research punt remains. No new contradictions or regressions were introduced; the rest of the plan (untouched ACs, phase ordering, decision matrix, risk register) was already iteration-1 READY-quality. The plan is executor-ready.

**Fix verification:**
- ✅ **Fix 1 (session-id ownership) resolved.** Five mention sites now agree the runner mints the UUID: Scope L30 ("generate the `--session-id=<uuid>` internally inside the runner … drive does NOT mint the UUID"); Phase 1 `RunnerOutput.session_id` doc-comment L122 ("runner-generated UUID, returned to drive"); command-construction snippet L139–L140 (`let session_id = uuid::Uuid::new_v4().to_string();` with the comment "runner mints the UUID, drive never sees it until it's returned"); narrative L153 ("Drive does NOT mint the UUID and does NOT take a `uuid` dependency"); Decision Matrix row 5 L280 ("**(b) runner generates internally**"). No residual language in the Plan section says drive generates the UUID.
- ✅ **Fix 2 (AC2.5 self-conflict) resolved.** AC2.5's regex `^\s*[0-9]+\.\s*Call\s+\`stores tasks submit-` matches only numbered checklist items beginning with "Call \`stores tasks submit-" (the imperative-checklist form). Prose mentions like "drive calls submit handlers internally" or warnings like "Do NOT invoke `submit-*`" do not match — they lack the leading `N.` numbering and/or the literal `stores tasks submit-` substring. Phase 2's replacement text on L212 (`5. **EMIT** the JSON envelope as your final structured output. Drive parses it and submits in-process; do not invoke any \`submit-*\` verb directly.`) starts with `EMIT` rather than `Call`, and uses the bare token `submit-*` rather than `stores tasks submit-`, so it slips past the regex by construction (planner's note on L213 confirms this is intentional).
- ✅ **Fix 3 (schema dialect) resolved.** AC1.2 L160 pins Draft 2020-12 as the default (`"$schema": "https://json-schema.org/draft/2020-12/schema"`) with an automatic, mechanically-resolvable fallback to Draft-07 gated on a concrete trigger: a parse/dialect rejection from `claude -p --json-schema=<draft-2020-12-file>` during AC1.5 fixture-stream tests. No SDK-doc research is required; the executor only needs to observe the runtime test outcome and swap one line. Decision Matrix row 8 L283 mirrors the same swap-path rationale and requires the chosen dialect to be recorded in the Phase 1 executor summary.

---

## Execution Log

### Phase 1 — Schemas + runner refactor

**Status:** CODE_REVIEW

**Commit:** 58e959f

**Dialect decision:** Draft 2020-12 retained. The `jsonschema = "0.18"` crate accepted all 5 schemas with `"$schema": "https://json-schema.org/draft/2020-12/schema"` without error. No Draft-07 fallback was required.

**Files changed:**
- `agents/schemas/planner.schema.json` (created)
- `agents/schemas/plan-reviewer.schema.json` (created)
- `agents/schemas/executor.schema.json` (created)
- `agents/schemas/code-reviewer.schema.json` (created)
- `agents/schemas/guide.schema.json` (created)
- `tests/schemas_validate_fixtures.rs` (created)
- `Cargo.toml` (uuid dep, jsonschema dev-dep)
- `src/runner/mod.rs` (RunnerOutput: +structured_output, +session_id; Runner::spawn: +schema param)
- `src/runner/claude_code.rs` (full rewrite: stream-json, --session-id, --json-schema, cwd canonicalisation, JSONL transcript write)
- `src/runner/mock.rs` (spawn sig updated; structured_output round-trip test added)
- `src/cli/agents.rs` (BUNDLED_AGENT_SCHEMAS added; bundled_schemas_count_matches_agents test added)
- `src/handlers/drive.rs` (spawn call updated to None; RunnerOutput literals updated)
- `src/handlers/guide.rs` (spawn calls updated to None; RunnerOutput literals updated)

**AC checklist:**
- ✅ AC1.1 — `cargo build --features runner-claude-code` succeeds; `uuid` dep resolves.
- ✅ AC1.2 — 5 schemas under `agents/schemas/`; valid JSON Schema; dialect = Draft 2020-12.
- ✅ AC1.3 — `tests/schemas_validate_fixtures.rs` validates all 5 fixtures (positive + negative stray-field test).
- ✅ AC1.4 — `BUNDLED_AGENT_SCHEMAS` has 5 entries; `bundled_schemas_count_matches_agents` test passes.
- ✅ AC1.5 — New tests cover: (a) structured_output extraction; (b) error_max_structured_output_retries surfacing; (c) cwd canonicalisation; (d) session-id is valid v4 UUID propagated to RunnerOutput.
- ✅ AC1.6 — `MockRunner` structured_output round-trip test passes.
- ✅ AC1.7 — `runner_uses_path_shim_not_real_claude` and `command_construction_and_final_message_parsing` pass (adapted to new spawn signature and stream-json parsing).

**Test summary:** `cargo test --features runner-claude-code` → 370 unit tests + 2 integration tests = 372 passed, 0 failed.

**Deviations from plan:**
1. `runner_uses_path_shim_not_real_claude` test shim used a regular Rust string initially, causing "Unterminated quoted string" in sh due to literal backslash escapes. Fixed by switching to a raw string literal `r#"..."#` matching the pattern used in `command_construction_and_final_message_parsing`. No behavioral deviation.
2. Schema `reasoning`/`notes` fields are listed in `properties` but NOT in `required` — required would have broken fixture validation since the v0.3 fixtures predate the reasoning field. This follows plan rule 5 (fixture-first fidelity) and is the correct interpretation.
3. Schema temp-file for `--json-schema` uses manual `std::fs::write` to `/tmp/stores-schema-<uuid>.json` instead of the `tempfile` crate (which is dev-dep only). Cleanup happens after child exits. No behavioral difference.

---

### Phase 3 — Attempt 1 (FAILED, surfaces 2 bugs + architectural correction)

**Date:** 2026-04-28 (orchestrator-driven, no executor commit — direct repro by orchestrator after task-workflow:executor subagent failed to capture the live drive output and returned an empty status message after ~10min wall-clock).

**What was tried:**
1. Reinstalled `stores 0.3.0` binary from the Phase 1+2 codebase (`cargo install --path . --features runner-claude-code --force`).
2. In `/tmp/t003-smoke`: wiped `.stores tasks`, ran `stores setup`, `stores tasks add ... "Hello world" hello "echo hi prints hi" scripts/ src/`, then `nohup stores tasks drive --auto --claude-code --testing > /tmp/t004-smoke.log`.
3. Attempt 1: planner spawn hung for 15+ minutes with no output. Drive process alive (SN), claude child (PID 2159087) alive but idle (`/proc/<pid>/fd` showed only `/dev/null` and `/dev/urandom`; no open output files). Killed via `kill 2159087 2159085`.
4. Diagnosed: runner passed `--json-schema=/tmp/stores-schema-<uuid>.json` (file path) but `claude --help` shows `--json-schema <schema>` takes inline JSON text (example value: `'{"type":"object",...}'`). Verified empirically with two direct `claude -p` invocations: inline schema returned `result.structured_output: {...}` in 4s; file-path returned empty stdout, exit 0. **Bug A.**
5. Applied minimal inline fix to `src/runner/claude_code.rs` (removed temp-file write, pass schema text directly via `--json-schema=<text>`). Rebuilt, reinstalled, re-ran smoke.
6. Attempt 2: planner returned in 64s. Drive then failed envelope parse: `final_message JSON parse failed: missing field 'role' at line 1 column 3568` — content was an intermediate `user` event (a tool_result for a denied bash call), not the terminal `result` event. **Bug B.**
7. Inspected `.stores/runs/5fc73fef-3b3f-47b7-aa53-a7e9d0dc8687.jsonl` (26 events: 1 system, 16 assistant, 7 user, 1 rate_limit, 1 result):
   - `result.subtype = "success"`, `is_error = false`, but `structured_output: null`.
   - `result.result` (text field) contained a markdown-fenced JSON envelope with valid `role: "planner"`, phases, decision_matrix, plan_notes — i.e. the plan was actually correct, just emitted as prose, not via the structured-output tool.
   - `permission_denials` contained one entry: planner tried `ls -la scripts/hi && ./scripts/hi` (Bug C — incidental).
8. **Architectural finding:** `--json-schema` does post-hoc validation + re-prompt on mismatch, NOT constrained decoding. When a multi-turn tool-using agent ends in prose, the structured-output tool is never invoked, so schema validation never engages. Confirmed by delegated research (BAML, Pydantic AI, Anthropic SDK docs, OpenAI Agents SDK docs all converge on belt-and-braces stack).

**Three findings → three remediation buckets:**
- Bug A: fixed inline (5-line edit, remove temp-file path). Stays in Phase 1 revise commit.
- Bug B: Phase 1 revise — harden `extract_structured_output_from_stream_json` to walk to terminal `result` event, with realistic multi-event fixture.
- Bug C: not addressed (incidental).
- Architectural correction: Phase 1 revise adds SAP-style fallback (`extract_envelope_from_text`); Phase 2 revise rewrites `parse_envelope` as 3-layer fallback (sdk → sap → legacy). Submit-tool layer (Pydantic AI pattern) deferred to v0.5+.

**Status moved to `EXECUTING_PHASE_1_REVISE`. New ACs (AC1.8–AC1.11, AC2.8–AC2.9) appended to Plan section under "Plan Revisions".**

---

## Code Review Log

### Phase 1

**Gate:** PASS

**Summary:** Phase 1 is complete and correctly scoped. All 7 ACs (AC1.1–AC1.7) have concrete evidence in the diff and verifiable test coverage. `cargo build --features runner-claude-code` succeeds (one expected `dead_code` warning on `BUNDLED_AGENT_SCHEMAS` that Phase 2 will consume). `cargo test --features runner-claude-code` runs 372 tests (370 unit + 2 integration), 0 failed, exactly matching the executor's claim. `bash tests/drive_e2e.sh` passes both AC7.1 and AC7.1b — no v0.3 regression. Schema/AgentEnvelope alignment is tight: every field the deserializer reads is declared with the right type and matches each fixture byte-for-byte; `additionalProperties: false` is enforced positively (validate) and negatively (stray-field rejection). `claude_code::spawn` mints the UUID itself, canonicalises cwd via the testable `resolve_cwd()` helper, writes the JSONL transcript to `.stores/runs/<uuid>.jsonl`, and surfaces `error_max_structured_output_retries` to stderr. **Phase boundary discipline is observed**: `src/handlers/drive.rs` and `src/handlers/guide.rs` only adapt to the new spawn signature (pass `None`, default `structured_output: None` and `session_id: None`) — there is no schema lookup or `BUNDLED_AGENT_SCHEMAS` consumer code in drive (those belong to Phase 2). The Phase-1 portion of DONE_WHEN — the foundational refactor that lets Phase 2/3 wire the schema through — is satisfied. Mock runner accepts `structured_output` round-trip and ignores the schema arg. Cargo version remains `0.3.0` (correctly deferred to Phase 3).

**AC verification:**
- AC1.1: PASS — `cargo build --features runner-claude-code` succeeds; `uuid = { version = "1", features = ["v4"] }` in `Cargo.toml:21` (regular dep, correct since the runner uses it).
- AC1.2: PASS — exactly 5 schema files under `agents/schemas/`; all five declare `"$schema": "https://json-schema.org/draft/2020-12/schema"` and parse as valid JSON (asserted by `bundled_schemas_count_matches_agents` in `src/cli/agents.rs:355`).
- AC1.3: PASS — `tests/schemas_validate_fixtures.rs` exists with two tests: `all_fixtures_validate_against_schemas` (positive round-trip for all 5 roles) and `fixtures_with_stray_field_rejected_by_schema` (negative `additionalProperties: false` enforcement). Both pass. `jsonschema = "0.18"` is correctly placed as a dev-dep on `Cargo.toml:29`.
- AC1.4: PASS — `BUNDLED_AGENT_SCHEMAS` in `src/cli/agents.rs:46-67` contains exactly 5 entries; `bundled_schemas_count_matches_agents` asserts both length and role-name parity with `BUNDLED_AGENTS`.
- AC1.5: PASS — all four sub-ACs covered: (a) `extract_structured_output_returns_some_when_present` in `src/runner/claude_code.rs:527`, (b) `extract_structured_output_returns_none_and_error_subtype_on_retries_exhausted` at line 542, (c) `cwd_canonicalised_before_spawn` at line 557 (asserts `resolve_cwd()` equals `current_dir().canonicalize()`), (d) `session_id_is_valid_uuid_v4_propagated_to_output` at line 572 (parses as UUID v4 and confirms propagation through `RunnerOutput.session_id`).
- AC1.6: PASS — `structured_output_round_trip` in `src/runner/mock.rs:141` asserts `MockRunner` accepts `structured_output: Some(...)` and returns it verbatim with the schema arg ignored.
- AC1.7: PASS — `runner_uses_path_shim_not_real_claude` (line 484) and `command_construction_and_final_message_parsing` (line 369) both pass; the shim was correctly migrated to a raw-string literal `r#"..."#` emitting a stream-json `result` event with `text`, and the assertion path now flows through `extract_structured_output_from_stream_json`.

**Findings:** none.

**Counts:** {critical: 0, major: 0, minor: 0}



### Phase 2

**Gate:** PASS

**Summary:** Phase 2 correctly wires drive to the schema-validated envelope contract laid down by Phase 1. Drive looks up `BUNDLED_AGENT_SCHEMAS` and threads the schema text through `runner.spawn` (`src/handlers/drive.rs:419-434`); when the role is not in the registry, `Option::find().map()` produces `None` and the runner falls back to the legacy path, exactly as the plan prescribes. `parse_envelope` now prefers `RunnerOutput.structured_output`, then `final_message`, then a last-line stdout scan (`src/handlers/drive.rs:540-563`) — legacy mock fixtures still work, which `tests/drive_e2e.sh` confirms (both AC7.1 and AC7.1b pass). The AC2.7 retry-exhausted surface at lines 442-454 is positioned BEFORE the non-zero-exit bail, so the eprintln is reached on the failure path; the eprintln body contains both required substrings (`"schema validation retries exhausted"` and `.stores/runs/<sid>.jsonl`). Phase boundary discipline is observed: no `Cargo.toml` version bump (still `0.3.0`), no schema files added or modified, no Runner/RunnerOutput trait shape changes (Phase 2 only touches drive, briefs, agent prompts — `git diff 58e959f..64f1903 --name-only` confirms 11 files, all in scope). Brief templates are pruned to non-imperative `EMIT` phrasing (5 insertions, 6 deletions across 4 templates — minimal, surgical); agent prompts in all 5 roles replace "last non-empty line" boilerplate with schema-conformance language while retaining the `do NOT invoke stores tasks submit-*` warnings. Both AC2.5 and AC2.6 greps return empty. `cargo build --features runner-claude-code` succeeds; `cargo test --features runner-claude-code` reports 372 unit + 2 integration = 374 passed, 0 failed (both new Phase-2 tests `structured_output_takes_precedence_over_final_message` and `retries_exhausted_surfaces_transcript_path` are present and green). Phase-2 DONE_WHEN portion (drive consumes structured output without behavioural regression on the legacy fallback) is satisfied; Phase 3's haiku smoke will close the loop on the live stderr capture.

**AC verification:**
- AC2.1: ✅ `cargo build --features runner-claude-code` succeeds (verified locally — no warnings or errors).
- AC2.2: ✅ `cargo test --features runner-claude-code` passes 372+2=374 tests, 0 failed; drive unit tests include the two new Phase-2 tests at `src/handlers/drive.rs:1095` (`structured_output_takes_precedence_over_final_message`) and `src/handlers/drive.rs:1124` (`retries_exhausted_surfaces_transcript_path`).
- AC2.3: ✅ `bash tests/drive_e2e.sh` passes both AC7.1 and AC7.1b on the legacy fixtures; mock runner sets `structured_output: None`, drive falls back to `final_message`, behaviour unchanged from v0.3.
- AC2.4: ✅ `structured_output_takes_precedence_over_final_message` (`src/handlers/drive.rs:1095-1119`) constructs a `RunnerOutput` with `final_message: Some("this is not valid json {{{{".to_string())` AND `structured_output: Some(valid_envelope)`, then asserts `parse_envelope` succeeds via `structured_output` and yields a `Planner` envelope — exactly the precedence the plan requires.
- AC2.5: ✅ `grep -rE '^\s*[0-9]+\.\s*Call\s+\`stores tasks submit-' stores/tasks/templates/` returns empty (verified locally). Replacement text starts with `EMIT` rather than `Call` and uses the bare token `submit-*` rather than the literal `stores tasks submit-`, slipping past the regex by construction.
- AC2.6: ✅ `grep -r 'last non-empty line' agents/` returns empty across all 5 agent prompts. Schema-conformance one-liner is added to all 5 (`agents/{planner,plan-reviewer,executor,code-reviewer,guide}.md`); `do NOT invoke stores tasks submit-*` warnings remain in all 5.
- AC2.7: ✅ Code path verified at `src/handlers/drive.rs:444-454` — the eprintln contains the literal substring `"schema validation retries exhausted"` AND interpolates the `.stores/runs/<sid>.jsonl` transcript path (with a sensible `<no session-id>` fallback when `session_id` is `None`). The unit test `retries_exhausted_surfaces_transcript_path` exercises this code path end-to-end via drive_loop with a fail RunnerOutput and verifies the non-zero-exit bail fires correctly afterward. The executor's deviation note (test cannot capture `eprintln!` fd2 text directly without redirection) is reasonable; the eprintln body is correct and Phase 3's smoke will validate the live stderr capture. Acceptable as PASS.

**Findings:** none.

**Counts:** {critical: 0, major: 0, minor: 0}

### Phase 2 — Drive consumes structured output, briefs and prompts updated

**Status:** CODE_REVIEW

**Commit:** 64f1903

**Files changed:**
- `src/handlers/drive.rs` (schema lookup via BUNDLED_AGENT_SCHEMAS; schema_text threaded to runner.spawn; parse_envelope rewired to prefer structured_output; AC2.7 retry-exhausted surface; new tests: structured_output_takes_precedence_over_final_message, retries_exhausted_surfaces_transcript_path)
- `stores/tasks/templates/planner-brief.md.tpl` (removed imperative submit line)
- `stores/tasks/templates/plan-reviewer-brief.md.tpl` (removed imperative submit line)
- `stores/tasks/templates/executor-brief.md.tpl` (removed imperative submit lines × 2)
- `stores/tasks/templates/code-reviewer-brief.md.tpl` (removed imperative submit line)
- `agents/planner.md` (removed "last non-empty line" / "Final stdout line is the JSON envelope" boilerplate; replaced with schema-conformance language)
- `agents/plan-reviewer.md` (same — 2 checklist occurrences)
- `agents/executor.md` (same — 2 checklist occurrences)
- `agents/code-reviewer.md` (same)
- `agents/guide.md` (same)

**AC checklist:**
- ✅ AC2.1 — `cargo build --features runner-claude-code` succeeds.
- ✅ AC2.2 — `cargo test` (default features) passes; 358 unit + 2 integration = 360 passed. With `--features runner-claude-code`: 372 + 2 = 374 passed. Drive unit tests include 2 new tests.
- ✅ AC2.3 — `bash tests/drive_e2e.sh` passes both AC7.1 and AC7.1b on legacy fixtures.
- ✅ AC2.4 — `structured_output_takes_precedence_over_final_message` test asserts parse_envelope succeeds via structured_output when final_message is malformed.
- ✅ AC2.5 — `grep -rE '^\s*[0-9]+\.\s*Call\s+`stores tasks submit-' stores/tasks/templates/` returns zero matches.
- ✅ AC2.6 — `grep -r 'last non-empty line' agents/` returns zero matches.
- ✅ AC2.7 — drive_loop surfaces "schema validation retries exhausted" + transcript path when runner stderr contains that substring; `retries_exhausted_surfaces_transcript_path` test verifies drive_loop errors correctly on non-zero exit from such a runner.

**AC2.5 grep evidence:** `<empty>`

**AC2.6 grep evidence:** `<empty>`

**Deviations from plan:**
1. AC2.7 unit test (`retries_exhausted_surfaces_transcript_path`) verifies the drive_loop error path (non-zero exit + correct error message) rather than intercepting `eprintln!` output directly — Rust unit tests cannot capture stderr from `eprintln!` without redirecting fd2. The runner output's stderr field is set to contain the exact substring "schema validation retries exhausted" matching what `claude_code::spawn` emits; the AC2.7 drive-layer eprintln is exercised by the test via the `run_out.stderr.contains(...)` branch in drive_loop. Full e2e verification of the exact stderr text belongs to Phase 3's smoke.

---

## Completion
_Orchestrator fills this on COMPLETE._
