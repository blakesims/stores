# T018: topology: render zones separately to fix multi-cluster layout blow-up

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T11:59:16Z
- **Last Updated:** 2026-05-03T12:01:05Z
- **Current Phase:** 
- **Current Cycle:** 
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

