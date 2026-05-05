# T005: stores topology static schematic command

## Meta
- **Status:** plan_review
- **Created:** 2026-05-03T09:46:39Z
- **Last Updated:** 2026-05-03T09:50:57Z
- **Current Phase:** 
- **Current Cycle:** 
- **Blocked Reason:** —
- **Branch:** feat/T005-stores-topology-schematic
- **Capability:** observability

## Task

Add a &#x60;stores topology&#x60; command that prints a static, colored, in-terminal schematic of the substrate&#x27;s data-flow topology — soft-FK system block diagram, per-store state machines with actor-coloring on every transition, and the tasks-workflow firing order. Renders via &#x60;dot -Tutf8&#x60; (graphviz) by default, with mermaid and dot-source as alternative formats. The actor coloring is the central visual signal: it answers &quot;where does the engine fire on its own vs. where does the human have to throw a switch.&quot;

---

## Plan

### Objective
_No objective set._

### Scope
- **In:** - New top-level subcommand &#x60;topology&#x60; registered in src/cli/dynamic.rs and dispatched in src/main.rs (sibling to init/setup/auth/watch).
- New module src/cli/topology.rs containing the dot/mermaid emitters and the shell-out logic.
- Schema reader: walk &#x60;lifecycle.transitions&#x60; for each installed store from manifest+schemas to produce nodes and edges.
- Cross-store edge discovery: identify ListFk/Fk-typed fields whose target_store is another installed store; render those as Z0 edges.
- Dot emitter with actor color attributes on edges and Nerd Font icon prefixes on edge labels.
- Mermaid stateDiagram-v2 emitter with the equivalent actor markers (Mermaid does not support per-edge color reliably, so use icon-only there).
- Shell-out wrapper: &#x60;Command::new(&quot;dot&quot;).args([&quot;-Tutf8&quot;]).stdin(piped).output()&#x60; with explicit fallback when the binary is absent or the call fails.
- README &quot;Topology&quot; subsection.
- Tests: golden snapshot tests for dot+mermaid output against the bundled tasks/observations/gate schemas; a fallback-path test that simulates &#x60;dot&#x60; missing.
- **Out:** - Live data — no DB reads for state-occupancy counts. Topology is deliberately static; live counts belong in a future enhancement to &#x60;stores watch&#x60;.
- Sound, notifications, or any push-based observability.
- Watch-style refresh loop or any TTY clearing.
- Cross-worktree aggregation.
- Hand-rolled ASCII layout engine — graphviz dot does layout.
- SVG/PNG output (would require an additional renderer crate; stick to text-only for v0).
- Configurable color themes — one fixed palette tuned for dark terminals.
- Per-state row counts overlaid on Z1 nodes — that crosses into live-data territory.

### Done When
- &#x60;stores topology&#x60; (no args) prints three sections to stdout, top-to-bottom: (Z0) cross-store soft-FK block diagram, (Z1) per-store state machine for each installed store, (Z2) tasks-workflow firing order if a workflow is declared.
- Every transition edge is labeled with the verb and a colored actor marker. Four actor classes have distinct colors and Nerd Font icons: ai_autonomous (green), ai_with_human (yellow), human (red), framework (dim/gray). Text-code fallback (A / H+ / H! / F) when --no-icons is passed or NO_COLOR is set.
- Default rendering shells out to &#x60;dot -Tutf8&#x60;. If &#x60;dot&#x60; is not on PATH, prints the dot source verbatim with a one-line note pointing the user at &#x60;apt install graphviz&#x60; (or equivalent) and the &#x60;--format mermaid&#x60; alternative.
- &#x60;--format dot&#x60; emits graphviz dot source to stdout, no shellout.
- &#x60;--format mermaid&#x60; emits a mermaid stateDiagram-v2 to stdout suitable for pasting into a markdown doc.
- &#x60;--store &lt;name&gt;&#x60; filters Z1 (and Z2 if applicable) to a single store; Z0 still shows the whole graph for context.
- Tests cover: dot emission shape (golden snapshot), mermaid emission shape (golden snapshot), graceful fallback when &#x60;dot&#x60; is missing on PATH.
- README gains a short &quot;Topology&quot; subsection under Usage with a screenshot-style example output.

### Assumptions
- &#x60;dot&#x60; (graphviz) is available on the user&#x27;s machines; if not, the fallback prints dot source and a clear pointer. We do not vendor a Rust dot-renderer crate.
- The user&#x27;s terminal supports Nerd Font glyphs by default; --no-icons flag and NO_COLOR env var both fall back to text-code markers (A / H+ / H! / F).
- The &#x60;Schema&#x60; struct already exposes &#x60;lifecycle.transitions&#x60; with &#x60;from_state&#x60;, &#x60;to_state&#x60;, &#x60;verb&#x60;, and &#x60;actor&#x60;; the topology emitter only reads these and does not require schema changes.
- ListFk field-type metadata exposes the target store name (verify in scope-in pass; if not, the cross-store edges section is best-effort).

### Phases

#### Phase 1: Phase 1: CLI scaffolding + topology module skeleton
- **Objective:** Register &#x60;stores topology&#x60; as a top-level subcommand, wire dispatch in main.rs, and add an empty src/cli/topology.rs with public &#x60;run(opts)&#x60; plus the actor→{color,icon,code} styling table.
- **Tasks:**
  - Task 1.1: Register &#x60;topology&#x60; subcommand in src/cli/dynamic.rs::build_root with args &#x60;--format &lt;auto|dot|mermaid&gt;&#x60; (default &#x60;auto&#x60;), &#x60;--store &lt;name&gt;&#x60;, &#x60;--no-icons&#x60; (ArgAction::SetTrue). Place it as a sibling to &#x60;watch&#x60; so the build_root section reads top-to-bottom in feature order.
  - Task 1.2: Add &#x60;pub mod topology;&#x60; to src/cli/mod.rs.
  - Task 1.3: Create src/cli/topology.rs with &#x60;pub struct Opts { format: Format, store_filter: Option&lt;String&gt;, no_icons: bool }&#x60;, &#x60;pub enum Format { Auto, Dot, Mermaid }&#x60;, and &#x60;pub fn run(manifest: &amp;Manifest, schemas: &amp;HashMap&lt;String, Schema&gt;, opts: Opts) -&gt; Result&lt;()&gt;&#x60; (initially returns Ok(()), prints nothing — emitters land in Phase 2).
  - Task 1.4: Define &#x60;actor_style(actor: Option&lt;Actor&gt;, no_icons: bool, color_disabled: bool) -&gt; ActorStyle&#x60; returning &#x60;{ dot_color: &amp;&#x27;static str, icon: &amp;&#x27;static str, text_code: &amp;&#x27;static str, label_prefix: String }&#x60; mapping: ai_autonomous→(green, nf-fa-robot, A), ai_with_human→(gold, nf-fa-handshake, H+), human→(red, nf-fa-user, H!), framework→(gray, nf-fa-cog, F). Color-disabled means &#x60;NO_COLOR&#x60; env set OR mermaid emitter (no per-edge color). No-icons means render text_code only (no glyph).
  - Task 1.5: Add dispatch arm in src/main.rs &#x60;Some((&quot;topology&quot;, sub)) &#x3D;&gt; { … cli::topology::run(&amp;manifest, &amp;schemas, opts)?; }&#x60; parallel to the &#x60;watch&#x60; arm. Parse &#x60;--format&#x60; via match on the string, default to Format::Auto.
- **Acceptance Criteria:**
  - [ ] AC1.1: &#x60;cargo build&#x60; succeeds.
  - [ ] AC1.2: &#x60;stores topology --help&#x60; lists the three flags &#x60;--format&#x60;, &#x60;--store&#x60;, &#x60;--no-icons&#x60; with descriptions.
  - [ ] AC1.3: &#x60;stores topology&#x60; runs without panic and exits 0 on a freshly &#x60;stores setup&#x60; repo (no rendered output yet is fine).
  - [ ] AC1.4: &#x60;cargo test cli::topology::actor_style&#x60; covers all four actors × {color_on,color_off} × {icons_on,icons_off} (≥4 tests).
- **Files:** `src/cli/topology.rs`, `src/cli/mod.rs`, `src/cli/dynamic.rs`, `src/main.rs`
#### Phase 2: Phase 2: Dot + Mermaid emitters (Z0, Z1, Z2)
- **Objective:** Implement the three-zone schematic in both dot and mermaid output formats, walking schemas + manifest + workflow declarations to produce nodes and edges.
- **Tasks:**
  - Task 2.1: In src/cli/topology.rs add &#x60;fn emit_dot(manifest, schemas, opts) -&gt; String&#x60;. Emit a single &#x60;digraph stores_topology { rankdir&#x3D;TB; compound&#x3D;true; … }&#x60; containing three subgraphs in order: &#x60;cluster_z0_cross_store&#x60;, one &#x60;cluster_z1_&lt;store&gt;&#x60; per installed (and filter-passing) store, and &#x60;cluster_z2_workflow&#x60; for any schema whose &#x60;workflow.is_some()&#x60;.
  - Task 2.2: Z0 cross-store FK edges — walk every installed schema&#x27;s &#x60;fields[]&#x60;; for each field whose &#x60;ty &#x3D;&#x3D; FieldType::ListFk { ref_store }&#x60; AND &#x60;ref_store&#x60; names another installed store, emit a node per store (box label &#x3D; store name, color by store) and an edge &#x60;&lt;src&gt; -&gt; &lt;ref&gt; [label&#x3D;&quot;&lt;field&gt;&quot;]&#x60;. Z0 always shows ALL installed stores regardless of &#x60;--store&#x60; filter (per contract).
  - Task 2.3: Z1 per-store state machine — for each installed store passing the &#x60;--store&#x60; filter, emit one node per &#x60;lifecycle.states[]&#x60; entry and one edge per &#x60;lifecycle.transitions[]&#x60; with label &#x60;&quot;{icon} {verb}&quot;&#x60; (or &#x60;&quot;{text_code} {verb}&quot;&#x60; when --no-icons), &#x60;color&#x3D;&lt;actor_dot_color&gt;&#x60;, and &#x60;fontcolor&#x3D;&lt;same&gt;&#x60;. Mark the &#x60;lifecycle.resolved_initial_state()&#x60; node with &#x60;style&#x3D;bold,peripheries&#x3D;2&#x60;.
  - Task 2.4: Z2 tasks-workflow firing order — for each store with &#x60;schema.workflow.is_some()&#x60; (and passing --store filter), build an ordered chain from &#x60;workflow.on_state&#x60; entries: nodes are state names, edges are &#x60;dispatch_agent: &lt;role&gt;&#x60; (label &#x3D; &#x60;&quot;→ {role}&quot;&#x60;) or &#x60;transition_to: &lt;state&gt;&#x60; (label &#x3D; &#x60;&quot;⇒ auto&quot;&#x60;, framework actor coloring). Render in a separate cluster labeled &#x60;&quot;Z2: {store} workflow firing order&quot;&#x60;.
  - Task 2.5: Add &#x60;fn emit_mermaid(manifest, schemas, opts) -&gt; String&#x60;. One top-level &#x60;stateDiagram-v2&#x60; block per zone, separated by &#x60;---&#x60; and a markdown heading line (&#x60;## Z0 …&#x60;). Mermaid does not support per-edge color; use icon-only (or text_code-only) prefixes. Initial state via &#x60;[*] --&gt; &lt;initial&gt;&#x60;.
  - Task 2.6: Both emitters must be deterministic — sort stores by manifest order, states by lifecycle declaration order, transitions by their declared index. No HashMap iteration in output paths.
- **Acceptance Criteria:**
  - [ ] AC2.1: &#x60;stores topology --format dot&#x60; prints a single &#x60;digraph&#x60; block containing exactly three top-level cluster subgraphs (Z0, Z1×N, Z2×workflow-stores) and exits 0.
  - [ ] AC2.2: Output of &#x60;--format dot&#x60; parses cleanly with &#x60;dot -Tsvg /dev/null&#x60; (graphviz syntax check) when &#x60;dot&#x60; is on PATH; covered as an integration test gated on &#x60;which dot&#x60;.
  - [ ] AC2.3: &#x60;--format mermaid&#x60; output begins with &#x60;## Z0&#x60; heading and contains at least one &#x60;stateDiagram-v2&#x60; block per workflow store.
  - [ ] AC2.4: Golden-snapshot test &#x60;tests/topology_dot_snapshot.rs&#x60; compares &#x60;emit_dot()&#x60; against &#x60;tests/fixtures/topology/expected.dot&#x60; for the bundled tasks+observations+gate trio; updates require explicit fixture commit.
  - [ ] AC2.5: Golden-snapshot test &#x60;tests/topology_mermaid_snapshot.rs&#x60; compares &#x60;emit_mermaid()&#x60; against &#x60;tests/fixtures/topology/expected.md&#x60;.
  - [ ] AC2.6: With &#x60;--store tasks&#x60;, Z1 and Z2 contain only the tasks store but Z0 still references all installed stores (verified by string-contains assertion on observations/gate appearing in Z0 but not Z1).
  - [ ] AC2.7: With &#x60;NO_COLOR&#x3D;1&#x60;, dot output contains no &#x60;color&#x3D;&#x60; attribute lines on edges (color is suppressed; icons may remain).
  - [ ] AC2.8: With &#x60;--no-icons&#x60;, edge labels contain &#x60;A&#x60;, &#x60;H+&#x60;, &#x60;H!&#x60;, or &#x60;F&#x60; text codes and contain none of the Nerd Font glyph code points.
- **Files:** `src/cli/topology.rs`, `tests/topology_dot_snapshot.rs`, `tests/topology_mermaid_snapshot.rs`, `tests/fixtures/topology/expected.dot`, `tests/fixtures/topology/expected.md`
- **Dependencies:** Phase 1 must be complete
#### Phase 3: Phase 3: Auto-format shell-out to &#x60;dot -Tutf8&#x60; with graceful fallback
- **Objective:** Default &#x60;--format auto&#x60; pipes the dot source into &#x60;dot -Tutf8&#x60; and prints the rendered ASCII; if &#x60;dot&#x60; is missing or fails, prints the dot source verbatim with a one-line install hint.
- **Tasks:**
  - Task 3.1: Add &#x60;fn render_via_dot(dot_source: &amp;str) -&gt; Result&lt;RenderOutcome&gt;&#x60; where &#x60;enum RenderOutcome { Rendered(String), Fallback { source: String, reason: FallbackReason } }&#x60; and &#x60;enum FallbackReason { DotMissing, DotFailed(String) }&#x60;.
  - Task 3.2: Use &#x60;std::process::Command::new(&quot;dot&quot;).args([&quot;-Tutf8&quot;]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()&#x60;; on &#x60;ErrorKind::NotFound&#x60; return &#x60;Fallback { source, reason: DotMissing }&#x60;. On non-zero exit return &#x60;Fallback { reason: DotFailed(stderr) }&#x60;.
  - Task 3.3: Wire &#x60;Format::Auto&#x60; in &#x60;run()&#x60; to call &#x60;render_via_dot&#x60;. On Rendered: print to stdout. On Fallback: print dot source, then a one-line stderr note: &#x60;note: &#x27;dot&#x27; not found on PATH — install graphviz (e.g. apt install graphviz) or use --format mermaid&#x60;.
  - Task 3.4: Inject the spawn function via a function pointer or trait object so tests can simulate dot-missing without mutating PATH (&#x60;type DotSpawner &#x3D; fn(&amp;str) -&gt; std::io::Result&lt;...&gt;&#x60;); production calls Command::new, tests pass a stub returning NotFound.
- **Acceptance Criteria:**
  - [ ] AC3.1: When &#x60;dot&#x60; is on PATH, &#x60;stores topology&#x60; prints non-empty UTF-8 ASCII output and exits 0 (integration test gated on &#x60;which dot&#x60; succeeding).
  - [ ] AC3.2: When &#x60;dot&#x60; is absent (simulated via spawner stub), output begins with &#x60;digraph&#x60; (raw dot source) and stderr contains the substring &#x60;apt install graphviz&#x60; and &#x60;--format mermaid&#x60;. Exit code is 0 (fallback is not an error).
  - [ ] AC3.3: &#x60;cargo test cli::topology::render_via_dot_falls_back_when_missing&#x60; passes.
- **Files:** `src/cli/topology.rs`
- **Dependencies:** Phase 2 must be complete
#### Phase 4: Phase 4: README + final polish
- **Objective:** Document the new command in README under a Topology subsection with an example, and add a smoke test that the command appears in &#x60;stores --help&#x60;.
- **Tasks:**
  - Task 4.1: Add a &#x60;### Topology&#x60; subsection under the README&#x27;s Usage area with a 6–10 line example showing &#x60;stores topology&#x60;, &#x60;stores topology --format mermaid&#x60;, and a screenshot-style fenced block of representative output (dot source for the bundled trio, trimmed to ~30 lines).
  - Task 4.2: Add an integration test &#x60;tests/topology_help.rs&#x60; that invokes the binary with &#x60;topology --help&#x60; via &#x60;assert_cmd&#x60; (or the existing test harness pattern — verify by reading &#x60;tests/&#x60; first) and asserts the three flags appear.
  - Task 4.3: Run &#x60;cargo fmt&#x60; and &#x60;cargo clippy --all-targets&#x60; and resolve any warnings introduced by Phases 1–3.
- **Acceptance Criteria:**
  - [ ] AC4.1: README contains the literal string &#x60;### Topology&#x60; under a Usage heading and a fenced code block beginning with &#x60;digraph&#x60; or &#x60;stateDiagram-v2&#x60;.
  - [ ] AC4.2: &#x60;cargo test --test topology_help&#x60; passes and asserts &#x60;--format&#x60;, &#x60;--store&#x60;, &#x60;--no-icons&#x60; appear in help output.
  - [ ] AC4.3: &#x60;cargo clippy --all-targets -- -D warnings&#x60; exits 0 (or matches the repo&#x27;s pre-existing clippy baseline if not -D warnings).
- **Files:** `README.md`, `tests/topology_help.rs`
- **Dependencies:** Phase 3 must be complete

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

