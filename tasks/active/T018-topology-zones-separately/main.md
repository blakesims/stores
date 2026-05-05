# T018: topology: render zones separately to fix multi-cluster layout blow-up

## Meta
- **Status:** in_review
- **Created:** 2026-05-03T11:59:16Z
- **Last Updated:** 2026-05-03T12:11:22Z
- **Current Phase:** 3
- **Current Cycle:** 1
- **Blocked Reason:** —
- **Branch:** feat/T018-topology-zones-separately
- **Capability:** observability

## Task

Refactor &#x60;stores topology --format auto&#x60; to render each zone (Z0 cross-store, Z1 per-store state machines, Z2 tasks workflow) as an independent dot graph piped through &#x60;graph-easy&#x60; separately, with section headers between. Replaces the current single-graph-with-subgraphs approach, which produces an unusable 876-column × 171-row layout because graph-easy cannot handle multi-cluster layouts. Width should drop to ~100 cols (fits any terminal); height grows but stays contiguous.

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - src/cli/topology.rs:
  - Split &#x60;emit_dot&#x60; into per-zone emitters: &#x60;emit_zone_z0_dot(manifest, schemas, opts) -&gt; String&#x60;, &#x60;emit_zone_z1_dot(store_name, schema, opts) -&gt; String&#x60;, &#x60;emit_zone_z2_dot(store_name, schema, opts) -&gt; String&#x60;. Each returns a complete standalone &#x60;digraph G { ... }&#x60; document (no &#x60;subgraph cluster_*&#x60; wrapper).
  - Keep &#x60;emit_dot&#x60; for &#x60;--format dot&#x60; callers (combined output with cluster subgraphs, unchanged).
  - Refactor &#x60;run()&#x60; so the &#x60;Format::Auto&#x60; arm iterates zones, spawns &#x60;graph-easy&#x60; per zone via the existing &#x60;render_via_dot&#x60; plumbing, prints &#x60;## &lt;zone-header&gt;\n\n&lt;rendered&gt;\n\n&#x60; per zone, and falls back to combined-dot-source on the FIRST graph-easy failure (preserve L036 fallback semantics).
  - The &#x60;--store &lt;name&gt;&#x60; filter applies to which zones get emitted (Z1 + Z2 for the named store; Z0 always shown).
- tests/topology_dot_render.rs gains:
  - &#x60;ac_max_line_width_under_120&#x60; asserting &#x60;--format auto&#x60; output never exceeds 120 cols (gated on &#x60;graph-easy --version&#x60;).
  - &#x60;ac_zone_headers_present&#x60; asserting all expected &#x60;## Z0&#x60;, &#x60;## Z1: ...&#x60;, &#x60;## Z2: ...&#x60; headers appear (gated on graph-easy).
- tests/topology_dot_snapshot.rs and tests/topology_mermaid_snapshot.rs: update goldens if and only if the per-zone refactor changes their byte-for-byte output (it should not for &#x60;--format dot&#x60; and &#x60;--format mermaid&#x60; if scope-out is honored).
- README.md &#x60;## Usage / ### Topology&#x60; subsection: update the example to show a representative trimmed &#x60;--format auto&#x60; boxart sample.
- **Out:** - Hand-rolled layered DAG ASCII layout engine (the &quot;option C&quot; from the discussion). Out of scope; out-source layout to &#x60;graph-easy&#x60; per zone.
- Switching &#x60;--format auto&#x60;&#x27;s default to mermaid (the &quot;option B&quot;). Out of scope; in-terminal rendering remains the goal of &#x60;--format auto&#x60;.
- New formats (e.g. &#x60;--format svg&#x60;, &#x60;--format png&#x60;). Out of scope; users can &#x60;--format dot | dot -Tsvg&#x60;.
- Theming, light-terminal palettes, glyph customization.
- Live-data overlays (per-state row counts, etc.) — that&#x27;s a watch-dashboard concern.
- Cross-worktree aggregation; &#x60;--workspace&#x60; flag.
- Schema migration of any kind; this task only touches src/cli/topology.rs and its tests/README.
- Refactoring &#x60;--format dot&#x60;&#x27;s combined-cluster output. Stays as-is to avoid breaking external graphviz pipelines.

### Done When
- &#x60;stores topology&#x60; (default &#x60;--format auto&#x60;) emits one rendered graph per zone, each produced by piping a standalone dot graph through &#x60;graph-easy --as&#x3D;boxart&#x60;, separated by section headers (e.g. &#x60;## Z0: cross-store soft-FKs&#x60;, &#x60;## Z1: tasks state machine&#x60;, &#x60;## Z1: observations state machine&#x60;, &#x60;## Z2: tasks workflow firing order&#x60;).
- Maximum line width of &#x60;--format auto&#x60; output is ≤ 120 columns on the bundled &#x60;tasks&#x60; + &#x60;observations&#x60; + &#x60;gate&#x60; schema trio. Asserted by an integration test gated on &#x60;graph-easy --version&#x60; succeeding.
- Total content height grows linearly with zone count (each zone ~10–25 rows tall); no whitespace explosion artefacts; no cluster-boundary overlaps.
- &#x60;--format dot&#x60; continues to emit a single combined &#x60;digraph stores_topology { ... }&#x60; document with &#x60;subgraph cluster_*&#x60; blocks, unchanged from today (preserves backward compatibility for users piping to &#x60;dot -Tsvg&#x60;/&#x60;-Tpng&#x60;).
- &#x60;--format mermaid&#x60; continues to emit one document with multiple stateDiagram-v2 blocks, one per zone, unchanged from today (already correctly zone-separated).
- &#x60;--store &lt;name&gt;&#x60; filter behavior unchanged: filters Z1 and Z2 to a single store; Z0 still shows the whole cross-store graph.
- Fallback path unchanged: if &#x60;graph-easy&#x60; is missing on PATH, &#x60;--format auto&#x60; prints the combined dot source plus the install hint to stderr (the L036 behavior is preserved).
- Tests cover: (a) max-line-width assertion on &#x60;--format auto&#x60; output (gated on graph-easy), (b) presence of all expected zone headers in &#x60;--format auto&#x60; output (gated on graph-easy), (c) &#x60;--format dot&#x60; golden snapshot unchanged, (d) &#x60;--format mermaid&#x60; golden snapshot unchanged.
- README&#x27;s &#x60;## Usage / ### Topology&#x60; subsection updated with a representative trimmed &#x60;--format auto&#x60; rendered example (a few lines of boxart per zone) replacing or complementing the current &#x60;--format dot&#x60; example.

### Assumptions
- &#x60;graph-easy&#x60; (Debian/Ubuntu pkg &#x60;libgraph-easy-perl&#x60;) is on the developer&#x27;s PATH for testing &#x60;--format auto&#x60;. CI / hosts without it gracefully degrade via L036&#x27;s fallback (combined dot source + install hint).
- Each zone, rendered by graph-easy as an independent graph, fits within 120 columns: empirically Z0 (3 nodes / 2 edges) and Z2 (5 nodes / 4 edges) are very small; Z1 per store (7–10 nodes, 9–17 edges) lays out at ~70–90 cols when rendered alone. The 120-col guard provides headroom.
- The actor-color attributes in dot edges (&#x60;color&#x3D;green|gold|red|gray&#x60;) are honored by graph-easy when rendering boxart. If they are not, the colors degrade silently to plain glyphs — this is acceptable; the box-shape and text-code markers carry the actor signal.
- Splitting &#x60;emit_dot&#x60; into per-zone functions does not require changes to the schema reader or the actor styling table — both are pure functions of the zone fragment being emitted.

### Phases

#### Phase 1: Phase 1: Per-zone dot emitters + Auto-format zone iteration
- **Objective:** Introduce standalone per-zone dot emitters and rewire the Format::Auto arm of &#x60;run()&#x60; to iterate zones, render each via graph-easy, and emit &#x60;## …&#x60; headers between, while preserving emit_dot (combined) untouched for Format::Dot and preserving the L036 fallback semantics on the first graph-easy failure.
- **Tasks:**
  - Task 1.1: In src/cli/topology.rs add &#x60;pub fn emit_zone_z0_dot(manifest: &amp;Manifest, schemas: &amp;HashMap&lt;String, Schema&gt;) -&gt; String&#x60; returning a complete standalone &#x60;digraph G { rankdir&#x3D;TB; … }&#x60; document with the Z0 cross-store FK nodes/edges (no &#x60;subgraph cluster_*&#x60; wrapper, no &#x60;compound&#x3D;true&#x60;).
  - Task 1.2: Add &#x60;pub fn emit_zone_z1_dot(store_name: &amp;str, schema: &amp;Schema, opts: &amp;Opts) -&gt; String&#x60; returning a standalone digraph for the per-store lifecycle (mirrors current write_z1_dot body, wrapped as a fresh digraph instead of a subgraph cluster).
  - Task 1.3: Add &#x60;pub fn emit_zone_z2_dot(store_name: &amp;str, schema: &amp;Schema, opts: &amp;Opts) -&gt; String&#x60; returning a standalone digraph for the workflow firing order (mirrors current write_z2_dot body, wrapped as a fresh digraph).
  - Task 1.4: Refactor existing private &#x60;write_z0_dot&#x60; / &#x60;write_z1_dot&#x60; / &#x60;write_z2_dot&#x60; so they delegate to the new standalone emitters&#x27; inner-body logic via a small shared helper, ensuring &#x60;emit_dot()&#x60; produces byte-for-byte identical combined output (subgraph clusters preserved).
  - Task 1.5: Add a &#x60;zones_for_auto(manifest, schemas, opts) -&gt; Vec&lt;(String header, String dot_source)&gt;&#x60; helper that walks Z0, then per-store Z1 (respecting &#x60;opts.store_filter&#x60;), then per-workflow-store Z2 (respecting filter), returning headers exactly: &#x60;Z0: cross-store soft-FKs&#x60;, &#x60;Z1: &lt;store&gt; state machine&#x60;, &#x60;Z2: &lt;store&gt; workflow firing order&#x60; (matching mermaid emitter wording).
  - Task 1.6: Rewrite the &#x60;Format::Auto&#x60; arm of &#x60;run()&#x60; to call &#x60;zones_for_auto&#x60;, render each zone via &#x60;render_via_dot&#x60;, print &#x60;## &lt;header&gt;\n\n&lt;rendered&gt;\n\n&#x60; per zone on success; on the FIRST zone whose render returns &#x60;Fallback&#x60;, abort the per-zone loop and instead print the combined &#x60;emit_dot(...)&#x60; source plus the matching FALLBACK_NOTE_MISSING / FALLBACK_NOTE_FAILED to stderr (preserve L036 semantics).
  - Task 1.7: Run &#x60;cargo build&#x60; and &#x60;cargo test --lib&#x60; to confirm the refactor compiles and existing unit tests (actor_style, render_via_dot stubs, NO_COLOR, etc.) still pass.
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo build&#x60; succeeds with no new warnings.
  - [ ] AC1.2: &#x60;cargo test --lib cli::topology&#x60; passes (all existing unit tests green).
  - [ ] AC1.3: &#x60;cargo test --test topology_dot_snapshot&#x60; passes WITHOUT updating the golden — &#x60;emit_dot()&#x60; output is byte-for-byte unchanged for the bundled trio.
  - [ ] AC1.4: &#x60;cargo test --test topology_mermaid_snapshot&#x60; passes WITHOUT updating the golden — &#x60;emit_mermaid()&#x60; is untouched.
  - [ ] AC1.5: Each of &#x60;emit_zone_z0_dot&#x60;, &#x60;emit_zone_z1_dot&#x60;, &#x60;emit_zone_z2_dot&#x60; returns a string that begins with &#x60;digraph &#x60; and ends with &#x60;}\n&#x60;, contains no &#x60;subgraph cluster_&#x60; token, and contains no &#x60;compound&#x3D;true&#x60; directive (verified by smoke test or by reviewer reading the code).
- **Files:** `src/cli/topology.rs`
#### Phase 2: Phase 2: Integration tests for zone-separated auto output
- **Objective:** Add tests that lock in the two key outcomes of the refactor — bounded line width and presence of the expected &#x60;## Zk:&#x60; zone headers in &#x60;--format auto&#x60; output — gated on &#x60;graph-easy --version&#x60; succeeding so CI without it still passes.
- **Tasks:**
  - Task 2.1: In tests/topology_dot_render.rs (or a new tests/topology_auto_zones.rs sibling) add a helper that builds the bundled tasks+observations+gate trio (mirror tests/topology_dot_snapshot.rs::build_trio) and a &#x60;graph_easy_on_path()&#x60; gate (mirror existing helper).
  - Task 2.2: Add &#x60;ac_max_line_width_under_120&#x60; test: gated on graph-easy; calls a new public test-friendly helper (e.g. &#x60;render_auto_to_string(manifest, schemas, opts) -&gt; String&#x60;) OR reproduces the run() loop in-test by calling &#x60;zones_for_auto&#x60; + &#x60;render_via_dot&#x60; per zone; asserts every line of the concatenated rendered output has &#x60;chars().count() &lt;&#x3D; 120&#x60;. Failure message includes the offending line and its width.
  - Task 2.3: Add &#x60;ac_zone_headers_present&#x60; test: gated on graph-easy; asserts the rendered auto output contains &#x60;## Z0: cross-store soft-FKs&#x60;, &#x60;## Z1: tasks state machine&#x60;, &#x60;## Z1: observations state machine&#x60;, &#x60;## Z1: gate state machine&#x60;, and &#x60;## Z2: tasks workflow firing order&#x60; (gate/observations have no workflow).
  - Task 2.4: To make tests possible without invoking the binary, expose a small &#x60;pub fn render_auto(manifest, schemas, opts) -&gt; String&#x60; in src/cli/topology.rs that returns the same string &#x60;Format::Auto&#x60; would print to stdout (used by &#x60;run()&#x60; internally and by tests). Stderr fallback notes remain in &#x60;run()&#x60;.
  - Task 2.5: Confirm &#x60;cargo test --test topology_dot_render&#x60; (or the new test file) passes locally where graph-easy is installed; both new tests print a &#x60;skipping:&#x60; line and pass on hosts without graph-easy.
- **Acceptance Criteria:**
  - [ ] AC2.1: New test &#x60;ac_max_line_width_under_120&#x60; exists, is gated on &#x60;graph-easy --version&#x60;, and passes on this dev host (graph-easy is on PATH per assumptions).
  - [ ] AC2.2: New test &#x60;ac_zone_headers_present&#x60; exists, gated identically, and passes — checks all five expected &#x60;## …&#x60; headers are substrings of the rendered output.
  - [ ] AC2.3: On a host WITHOUT graph-easy, both new tests skip cleanly (return early after eprintln) — verified by temporary &#x60;PATH&#x3D;&#x60; override or by code review of the gate.
  - [ ] AC2.4: &#x60;cargo test&#x60; overall is green (no regressions in dot/mermaid snapshots, render_via_dot stub tests, or any previously passing test).
  - [ ] AC2.5: &#x60;render_auto&#x60; is the only new public surface added; no other public API of src/cli/topology.rs changes signature.
- **Files:** `src/cli/topology.rs`, `tests/topology_dot_render.rs`
- **Dependencies:** Phase 1 must be complete (zones_for_auto and render_auto exist).
#### Phase 3: Phase 3: README Topology subsection update
- **Objective:** Replace the current dot-source example in README.md &#x60;## Usage / ### Topology&#x60; with a representative trimmed &#x60;--format auto&#x60; boxart example so users see the actual in-terminal experience, complementing (not removing) the brief mention of &#x60;--format dot&#x60; for graphviz pipelines.
- **Tasks:**
  - Task 3.1: Run &#x60;stores topology --format auto&#x60; locally on the bundled trio; capture a trimmed sample (header + a few boxart lines per zone, ellipses where truncated) suitable for inline display.
  - Task 3.2: In README.md, update the &#x60;### Topology&#x60; subsection (lines ~98–140): keep the intro paragraph and the bash command block; replace the long &#x60;--format dot&#x60; source example with a trimmed &#x60;--format auto&#x60; boxart sample under a fenced code block (no language tag, since boxart is not a recognised highlighter). Mention briefly that &#x60;--format dot&#x60; still emits the combined graphviz source for &#x60;dot -Tsvg&#x60; pipelines.
  - Task 3.3: Verify rendered markdown looks reasonable (&#x60;grip&#x60; or just visual inspection) and &#x60;cargo build&#x60; still succeeds (no doc-test regressions).
- **Acceptance Criteria:**
  - [ ] AC3.1: README.md &#x60;### Topology&#x60; subsection now contains a fenced code block whose contents include at least the substrings &#x60;## Z0: cross-store soft-FKs&#x60;, &#x60;## Z1: tasks state machine&#x60;, and at least one boxart glyph (e.g. &#x60;┌&#x60;, &#x60;─&#x60;, &#x60;→&#x60;, &#x60;┐&#x60;).
  - [ ] AC3.2: The bash usage block (&#x60;stores topology …&#x60; examples) is preserved unchanged.
  - [ ] AC3.3: &#x60;cargo build&#x60; succeeds; no doc-tests fail.
  - [ ] AC3.4: README diff is bounded to the &#x60;### Topology&#x60; subsection — no other sections touched.
- **Files:** `README.md`
- **Dependencies:** Phase 1 must be complete (so &#x60;--format auto&#x60; produces the new layout to capture).

---

## Plan Review

### Review 1
- **Gate:** READY
- **Summary:** Plan is executable. Three phases with mechanical ACs (cargo build/test, byte-for-byte snapshot preservation, line-width and header substring assertions, README diff bounds). Decision matrix covers the consequential choices (fallback semantics, compound&#x3D;true, test surface, header wording, empty-lifecycle handling). done_when traces cleanly through ACs across all three phases.
- **At:** 2026-05-03T12:01:13Z

---

## Execution Log

### Phase 1 / Cycle 1
- **Status:** PASS
- **Summary:** Phase 1 complete. Extracted Z0/Z1/Z2 body emission into write_z*_body(indent) helpers; existing write_z*_dot delegate so emit_dot output is byte-for-byte unchanged. Added pub emit_zone_z0_dot / emit_zone_z1_dot / emit_zone_z2_dot returning standalone digraphs (no subgraph cluster_*, no compound&#x3D;true). Added zones_for_auto walking Z0, per-store Z1 (filter-aware), per-workflow Z2 (filter-aware) with headers matching mermaid emitter. Rewired Format::Auto to render zones one at a time, printing &#x27;## &lt;header&gt;&#x27; + boxart per zone; on first RenderOutcome::Fallback aborts and prints combined emit_dot source plus FALLBACK_NOTE_MISSING/FAILED to stderr (L036 preserved). Added #[allow(dead_code)] on Fallback.source (now read only by tests). cargo build clean (no new warnings); cli::topology lib tests 13/13 pass; topology_dot_snapshot 5/5 and topology_mermaid_snapshot 2/2 pass without fixture update.
- **Commit:** `5ab7e33ca2690df6b074de804da1168436784646`
- **Files:**
  - `src/cli/topology.rs`
- **At:** 2026-05-03T12:04:09Z
### Phase 2 / Cycle 1
- **Status:** PASS
- **Summary:** Added pub fn render_auto(manifest, schemas, opts) -&gt; String to src/cli/topology.rs (delegates to a private render_auto_with_reason that also returns the FallbackReason for run()&#x27;s stderr hint). Added two graph-easy-gated integration tests in tests/topology_dot_render.rs: ac_max_line_width_under_120 (every line &lt;&#x3D;120 cols) and ac_zone_headers_present (asserts all five ## Zk: headers — Z0 cross-store, Z1 tasks/observations/gate state machines, Z2 tasks workflow). Both pass on this host; PATH&#x3D;/usr/bin:/bin run confirmed they skip cleanly when graph-easy is absent. Full cargo test green (no regressions). No other public API changes.
- **Commit:** `300a6d1b039de16ebd441c4ec9cb4d8da28efa9a`
- **Files:**
  - `src/cli/topology.rs`
  - `tests/topology_dot_render.rs`
- **At:** 2026-05-03T12:07:58Z
### Phase 3 / Cycle 1
- **Status:** PASS
- **Summary:** Replaced the long --format dot example in README.md ### Topology with a trimmed --format auto boxart sample captured by running stores topology --format auto against a scratch workspace with observations/gate/tasks installed. Sample includes ## Z0: cross-store soft-FKs and ## Z1: tasks state machine headers plus boxart glyphs (┌ ─ → ┐ ╔ ║); added a one-line note that --format dot still emits the combined digraph for dot -Tsvg/-Tpng pipelines. Bash usage block (lines 102–108) preserved unchanged; diff bounded to ### Topology (32 insertions, 26 deletions); cargo build succeeds.
- **Commit:** `c0f2a66585d730f2566edaf454fa1025f92101be`
- **Files:**
  - `README.md`
- **At:** 2026-05-03T12:10:15Z

---

## Code Review Log


### Phase 1 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 5 ACs verified: cargo build clean (no new warnings in topology.rs), cli::topology lib tests 13/13 pass, topology_dot_snapshot 5/5 pass byte-for-byte unchanged, topology_mermaid_snapshot 2/2 pass unchanged, and emit_zone_z{0,1,2}_dot all start with &#x60;digraph &#x60;, end with &#x60;}\n&#x60;, and contain no &#x60;subgraph cluster_&#x60; or &#x60;compound&#x3D;true&#x60; (verified by grep — those tokens appear only in the combined emit_dot path). 0 critical, 0 major, 3 minor.
- **Findings:** 0 critical, 0 major, 3 minor
**Details:**
Git reality check: HEAD&#x3D;5ab7e33, status clean, only src/cli/topology.rs touched as claimed (+150/-22). Diff matches summary: write_z*_body helpers extracted with &#x60;indent: &amp;str&#x60; parameter; existing write_z*_dot delegate preserving combined output; new pub emit_zone_z*_dot wrap bodies in standalone &#x60;digraph z0/z1_&lt;store&gt;/z2_&lt;store&gt;&#x60; blocks with &#x60;rankdir&#x3D;TB&#x60;; zones_for_auto walks Z0 + filter-aware per-store Z1 + filter-aware per-store Z2 (only when schema.workflow.is_some()); Format::Auto buffers Rendered outcomes and on first Fallback aborts to combined emit_dot + L036 stderr note.

AC verification:
- AC1.1 PASS: &#x60;cargo build&#x60; finishes clean; clippy --lib produces 29 pre-existing warnings, zero in topology.rs.
- AC1.2 PASS: &#x60;cargo test --lib cli::topology&#x60; → 13 passed / 0 failed.
- AC1.3 PASS: &#x60;cargo test --test topology_dot_snapshot&#x60; → 5 passed; ac2_4_dot_snapshot_matches green without fixture update.
- AC1.4 PASS: &#x60;cargo test --test topology_mermaid_snapshot&#x60; → 2 passed; ac2_5_mermaid_snapshot_matches green without fixture update.
- AC1.5 PASS: read code at src/cli/topology.rs:166-176 (Z0), 200-209 (Z1), 314-322 (Z2). Each writes only &#x60;digraph &lt;name&gt; {\n&#x60; + &#x60;  rankdir&#x3D;TB;\n&#x60; + body + &#x60;}\n&#x60;. &#x60;grep -n &#x27;subgraph cluster_\|compound&#x3D;true&#x27; src/cli/topology.rs&#x60; shows hits only at lines 87/121/180/243 — all inside emit_dot/write_z*_dot combined paths, none inside the new per-zone emitters. Doc comment on emit_zone_z0_dot explicitly notes the contract; Z1/Z2 doc comments are thinner but the code is symmetric.

Minor findings (non-blocking; document for awareness):

[MINOR] emit_zone_z1_dot and emit_zone_z2_dot each recompute &#x60;no_color_env()&#x60; independently per call.
File: src/cli/topology.rs:200, 314
Evidence: both functions open with &#x60;let color_disabled &#x3D; no_color_env();&#x60; rather than accepting it from the caller.
Expected: harmless — zones_for_auto invokes each at most once per store.
Suggestion: leave as-is for now; if a future caller renders many zones in a hot loop, hoist into zones_for_auto and pass through.

[MINOR] Doc-comment asymmetry between Z0 emitter and Z1/Z2 emitters.
File: src/cli/topology.rs:160-165 vs 198, 312
Evidence: emit_zone_z0_dot has a 4-line contract doc-comment (&quot;Output begins with &#x60;digraph&#x60; and ends with &#x60;}\n&#x60;; no &#x60;subgraph cluster_*&#x60; wrapper, no &#x60;compound&#x3D;true&#x60;.&quot;); emit_zone_z1_dot and emit_zone_z2_dot only have a one-line summary.
Expected: AC1.5 contract applies equally to all three.
Suggestion: in a future cycle, copy the contract sentence onto Z1 and Z2 emitters so a reader of either function alone can confirm the no-cluster/no-compound invariant.

[MINOR] zones_for_auto iterates manifest.stores twice.
File: src/cli/topology.rs:340-365
Evidence: one loop appends Z1 zones, a separate loop appends Z2 zones — both apply the same &#x60;--store&#x60; filter logic.
Expected: produces correct ordering (all Z1 before all Z2) which matches the mermaid emitter convention; readability is fine.
Suggestion: leave as-is; combining would either change zone ordering or require a temporary buffer. The two-pass form is the clearer expression.

[INFORMATIONAL] The Format::Auto rewrite buffers all rendered zones into &#x60;Vec&lt;(String, String)&gt;&#x60; before printing so a Fallback on zone N aborts cleanly without partial output. This is the right shape for L036 preservation; just noting the memory tradeoff is intentional.

[INFORMATIONAL] The &#x60;#[allow(dead_code)]&#x60; on RenderOutcome::Fallback.source is justified by the surviving render_via_dot unit tests that pattern-match on the variant; the inline comment explains this. Reasonable carry-cost; reconsider in P3 if those tests change shape.
- **At:** 2026-05-03T12:05:41Z

### Phase 2 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 5 ACs verified: render_auto + render_auto_with_reason added (only render_auto pub), AC2.1 ac_max_line_width_under_120 + AC2.2 ac_zone_headers_present pass on this host, both gated on graph_easy_on_path() returning early with eprintln (AC2.3 verified by code review of the gate at tests/topology_dot_render.rs:55-59 and 86-89), full cargo test green (570+ tests, 0 failures, no regressions in dot/mermaid snapshots or render_via_dot stub tests). 0 critical, 0 major, 2 minor.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
AC2.1 PASS — &#x60;ac_max_line_width_under_120&#x60; (tests/topology_dot_render.rs:107-127) gated on graph_easy_on_path(), passes locally.
AC2.2 PASS — &#x60;ac_zone_headers_present&#x60; (tests/topology_dot_render.rs:131-156) gated identically, asserts five headers including &#x27;## Z0: cross-store soft-FKs&#x27;, &#x27;## Z1: tasks state machine&#x27;, &#x27;## Z1: observations state machine&#x27;, &#x27;## Z1: gate state machine&#x27;, &#x27;## Z2: tasks workflow firing order&#x27;. Note this includes &#x27;gate&#x27; which is bundled in the trio but not listed in the Done-When &#x27;e.g.&#x27; enumeration — correct: &#x27;e.g.&#x27; is non-exhaustive and gate has a state machine block.
AC2.3 PASS — gate is &#x60;if !graph_easy_on_path() { eprintln!(...); return; }&#x60; at the top of each new test; on PATH without graph-easy the Command::new(&#x27;graph-easy&#x27;).arg(&#x27;--version&#x27;).output() returns Err / non-zero status, so .map(|o| o.status.success()).unwrap_or(false) yields false and the test returns cleanly. Verified by code review.
AC2.4 PASS — &#x60;cargo test&#x60; whole-workspace: all suites green (multiple 570-test runs, 0 failed, 0 ignored).
AC2.5 PASS — only &#x60;pub fn render_auto(...)&#x60; was added to src/cli/topology.rs&#x27;s public API. &#x60;render_auto_with_reason&#x60; is private (no pub). Existing public surface (emit_dot, render_via_dot, Format, Opts, RenderOutcome) unchanged.

[MINOR] &#x60;#[allow(dead_code)]&#x60; on &#x60;pub fn render_auto&#x60; (src/cli/topology.rs:626) is unnecessary — pub functions in a library crate are not subject to dead_code lints from the lib&#x27;s own check, and the integration test in tests/topology_dot_render.rs imports it. The attribute is harmless but misleading; consider removing in a follow-up.
[MINOR] &#x60;build_trio()&#x60; helper in tests/topology_dot_render.rs duplicates manifest fixture construction inline rather than reusing existing fixture setup (if any) — acceptable for an integration test, but if a shared fixture module emerges in a future test, consider de-duplicating. Informational only.
- **At:** 2026-05-03T12:09:04Z

### Phase 3 / Cycle 1
- **Gate:** PASS
- **Summary:** PASS. All 4 ACs verified: README ### Topology now contains the required Z0/Z1 headers and ample boxart glyphs (┌ ─ ┐ ╔ ║ ▶ ▼); bash usage block (lines 102–108) preserved unchanged; cargo build succeeds; diff bounded strictly to the ### Topology subsection (32 insertions / 26 deletions, single hunk). 0 critical, 0 major, 2 minor.
- **Findings:** 0 critical, 0 major, 2 minor
**Details:**
AC3.1 PASS — README.md:113 contains &#x27;## Z0: cross-store soft-FKs&#x27;, README.md:129 contains &#x27;## Z1: tasks state machine&#x27;, and the fenced block contains boxart glyphs ┌ ─ ┐ ╔ ║ ▶ ▼ (the AC&#x27;s &#x27;e.g.&#x27; list is illustrative; ┌/─/┐ are all present).
AC3.2 PASS — bash code block at README.md:102–108 is byte-identical pre/post (verified via git diff scope).
AC3.3 PASS — &#x60;cargo build&#x60; finished clean (no warnings, no doc-test failures).
AC3.4 PASS — &#x60;git show c0f2a66 --stat&#x60; shows only README.md modified; the single diff hunk is contained within the ### Topology subsection (lines 110–142). No other sections touched.

Minor findings (non-blocking):
[MINOR] Glyph subset vs ACs.
File: README.md:110-140
Evidence: AC3.1 lists &#x27;→&#x27; as an example boxart glyph; the rendered sample uses &#x27;▶&#x27; and &#x27;▼&#x27; (graph-easy&#x27;s actual boxart output) instead of &#x27;→&#x27;. The AC says &#x27;e.g.&#x27; so this is fine — but a future reader scanning for the literal &#x27;→&#x27; won&#x27;t find it. Suggestion: none required; flagging only because the AC literal differs from the chosen rendering.

[MINOR] Trailing-line ellipsis &#x27;… (Z1: observations …, Z2: …) follow&#x27; is informational prose inside the fenced &#x60;&#x60;&#x60; block.
File: README.md:139
Evidence: line 139 inside the code fence reads &#x60;… (Z1: observations state machine, Z1: gate state machine, Z2: tasks workflow firing order follow)&#x60;. This is documentation prose, not actual program output, embedded in a code-fenced sample. A reader copy-pasting the fence to compare against &#x60;stores topology --format auto&#x60; would see a divergence on this line. Suggestion: either move that sentence below the code fence as plain prose, or prefix with a clear marker like &#x60;# ...&#x60; so it reads as a comment within the sample.

[INFORMATIONAL] Phase 3 is the final phase of T018 — PASS completes the task per the workflow contract.
- **At:** 2026-05-03T12:10:55Z

---

## Completion
- **In Review:** 2026-05-03T12:11:22Z — awaiting human GO/NO_GO

