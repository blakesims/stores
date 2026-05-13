# Improved phase plan: `stores watch` top-bar UI iteration

## Inherited decisions / constraints to preserve

- Keep the five store cards separate: Intake, Observations, Tasks, External Reviews, Engine.
- Preserve the shared six-slot flow grammar per card: front/work/gate/exit/wait/fault.
- Do not abbreviate semantic labels into cockpit codes (`cand`, `inv`, `wrk`, `dn`, `w`, `err`, etc.). The top bar must be readable without decoding.
- Use only subtle internal dividers, not nested boxes inside boxes.
- Color means attention/severity, not decoration. Success/exhaust counts must stay quiet even when high.
- The existing TUI code currently uses a fixed `TOP_STRIP_HEIGHT`, equal horizontal card widths, `lane_card_lines(...) -> [String; 3]`, and mostly `Paragraph` rendering. This likely needs to become a real top-card renderer rather than more string concatenation.

## Phase 1 — Readable 3x2 top-card grid layout

### Goal
Replace compressed top-card strings with a readable, full-word 3-column × 2-row grid inside each existing store card.

### Implementation shape

Likely files:

- `src/tui/render.rs` — main rendering changes, top-strip height, custom card drawing/tests.
- `src/tui/data.rs` — only if current flow counters need clearer slot naming.
- `src/tui/semantics.rs` — preferable home for `TopCardSlot` / label metadata if not purely render-local.
- `tests/tui_watch_cockpit.rs`, `tests/tui_watch_semantic_regression.rs` — rendered-buffer regression coverage.

Recommended sequencing:

1. Replace `lane_card_lines(...) -> [String; 3]` with a structured slot model, e.g.:

   ```rust
   struct TopCardSlot {
       glyph: &'static str,
       label: &'static str,
       count: Option<usize>,
       role: SlotRole, // front/work/gate/exit/wait/fault
       attention: AttentionKind, // exhaust/flow/fault/neutral/manual
   }
   ```

2. Add one function per lane that returns exactly six slots in canonical order:

   ```text
   row 1: front | work | gate
   row 2: exit  | wait | fault
   ```

   Suggested full labels:

   - Intake: `new`, `triage`, `needs info`, `routed`, `waiting`, `errors`
   - Observations: `candidates`, `investigate`, `contract gate`, `closed`, `waiting`, `errors`
   - Tasks: `queued`, `working`, `gate`, `done`, `waiting`, `failed`
   - External Reviews: `pending`, `running`, `revise`, `passed`, `waiting`, `tool fault`
   - Engine: `dispatch`, `runners`, `locks`, `clear`, `manual`/`waiting`, `daemon down`/`errors`

3. Render cards with direct buffer drawing or a small custom widget, not a single `Paragraph`, so inner separators and per-slot styles are reliable.

4. Keep only:

   - outer card border;
   - two subtle vertical dividers between the three columns;
   - one subtle horizontal divider between the two rows;
   - no nested inner border rectangle.

5. Increase top-strip height if needed. Prefer enough vertical space for each slot to show `glyph + full label` and a visually separated count. For example:

   ```text
   ┌ OBSERVATIONS ───────────────────────────────────────────────┐
   │ ◌ candidates        │ ◆ investigate       │ ◇ contract gate  │
   │ 8                   │ 0                   │ 0                │
   ├─────────────────────┼─────────────────────┼──────────────────┤
   │ ✓ closed            │ △ waiting           │ ▲ errors         │
   │ 0                   │ 8                   │ 0                │
   └─────────────────────┴─────────────────────┴──────────────────┘
   ```

   If the executor chooses a one-line-per-slot layout, acceptance must still prove labels and counts are visually separated and not glued.

6. Add responsive fallback before implementing color:

   - Wide terminals: five cards side by side with full labels.
   - Narrow terminals where full labels cannot fit: do **not** abbreviate into codes. Prefer one of:
     - wrap cards into two rows, or
     - use a compact but still full-word vertical layout, or
     - show fewer slots with an explicit `+ more` affordance.
   - Do not silently return to `◌cand8`-style compression.

### Phase 1 acceptance criteria

- Top bar still renders five separate store cards.
- Each store card exposes the same six semantic slots in stable position: front/work/gate on row 1, exit/wait/fault on row 2.
- Labels are full words at the normal/wide cockpit size; no glued strings like `◌cand8`, `◆inv0`, `◌q2`, `◆wrk2`, `✓dn0`, `△w8`, `▲err0`, `▲tool0`.
- Glyph, label, and count are visually separated. A count must be distinguishable as a count without parsing a concatenated token.
- Only the outer border plus subtle internal dividers are drawn; there are no nested cell boxes.
- Inner dividers align with the outer card edges and do not create double-heavy line noise.
- `TOP_STRIP_HEIGHT` and downstream table/detail layout are updated consistently; focus, scrolling, and detail panes still start at the correct row.
- Existing semantic row/detail behavior from the prior watch implementation remains unchanged.
- Rendered-buffer tests assert actual visible output, not just raw slot strings.
- Tests cover at least:
  - observations showing `candidates 8`, `investigate 0`, `contract gate 0`, `closed 0`, `waiting 8`, `errors 0`;
  - tasks showing `queued`, `working`, `gate`, `done`, `waiting`, `failed`;
  - external reviews showing `tool fault`, not `tool`;
  - engine showing `manual` when daemon-off is non-actionable and `daemon down` when actionable.
- Narrow-width behavior is explicitly tested enough to prove it remains readable and does not reintroduce abbreviations.

## Phase 2 — Catppuccin-style severity styling and threshold seam

### Goal
Apply dark-mode-friendly attention color to top-card slots based on slot meaning and count, while keeping success/exhaust slots quiet.

### Implementation shape

Likely files:

- `src/tui/semantics.rs` — `AttentionKind`, `SeverityGrade`, threshold classification tests.
- `src/tui/render.rs` — map severity grades to `ratatui::style::Style` and apply styles per slot/divider/border.
- `src/flow/config.rs` or a new narrow watch-config module — only if existing config loading can be reused without broad plumbing.
- TUI tests that inspect buffer cell foreground colors/styles for representative slots.

Recommended sequencing:

1. Introduce semantic attention kinds:

   ```rust
   enum AttentionKind {
       Exhaust, // success/closed/done/passed/clear; never alarming
       Flow,    // pile-up pressure
       Fault,   // escalates quickly
       Neutral, // static/manual/unknown informational state
   }
   ```

2. Introduce severity grades independent of concrete colors:

   ```rust
   enum SeverityGrade { Dim, Normal, Warning, High, Critical, SuccessQuiet }
   ```

3. Centralize default thresholds:

   ```text
   flow:  0 dim, 1 normal, 3 warning, 6 high, 10 critical
   fault: 0 dim, 1 warning, 2 high, 4 critical
   exhaust: always SuccessQuiet/Dim regardless of count
   neutral/manual: Dim or Normal, never Warning/Critical by count alone
   ```

4. Use a Catppuccin Mocha-ish palette with `ratatui::style::Color` approximations:

   ```text
   dim/subtext: DarkGray / gray-ish
   normal:     Gray or White depending focus
   success:    Green
   warning:    Yellow
   high:       light orange/peach approximation
   critical:   Red
   focus:      existing Cyan can remain for selected outer border unless replaced deliberately
   dividers:   DarkGray, not severity-colored
   ```

   If exact Catppuccin RGB colors are easy and supported in the current ratatui version, use `Color::Rgb`. Otherwise use terminal-safe approximations and keep palette centralized.

5. Do not build a large new config system in this pass. Check existing `.stores/config.yaml` plumbing first:

   - If it is straightforward to reuse, add optional config fields under a narrow key such as `watch.theme` / `watch.severity` with serde defaults.
   - If not straightforward, implement `WatchTheme::default_catppuccin_mocha()` and `SeverityThresholds::default()` as centralized structs, plus a clear TODO/future seam. Do not block the UI fix on config plumbing.

6. Apply style per slot, not by painting the whole card aggressively:

   - slot glyph/label/count: severity style;
   - success/exhaust: dim or quiet green;
   - fault: escalates quickly and may use bold at critical;
   - outer border: focused-state border remains obvious; optionally tint unfocused border by max slot severity, but keep this subtle;
   - inner dividers: always dim/subtle.

### Phase 2 acceptance criteria

- Each top-card slot has an explicit `AttentionKind`; no count is colored without knowing whether it is exhaust, flow, fault, or neutral.
- Exhaust/success slots do not alarm regardless of count:
  - `closed 100`, `done 100`, `passed 100`, and `clear 1` render quiet/success, not warning/high/critical.
- Flow-pressure slots grade by pile-up:
  - `waiting 0` dim;
  - `waiting 1` normal;
  - `waiting 3` warning;
  - `waiting 8` high;
  - `waiting 10+` critical.
- Fault slots escalate faster:
  - `errors 0` dim;
  - `errors 1` warning;
  - `failed/errors/tool fault 2` high;
  - `failed/errors/tool fault 4+` critical.
- Manual/neutral engine states do not become alarming merely because the daemon is off; actionable daemon failure still renders as a fault.
- Colors are dark-mode friendly and centralized in a named watch theme/palette function or config-backed struct.
- If config support is added:
  - missing config uses safe defaults;
  - malformed config fails loudly in existing config-load style;
  - tests cover default and one overridden threshold.
- If config support is deferred:
  - thresholds/palette are centralized in structs/constants with unit tests;
  - the code shape leaves a clear future config seam and does not scatter magic numbers.
- Tests verify severity classification independently of terminal rendering and also inspect at least a few rendered buffer cell styles for representative slots.
- Color styling must not break monochrome readability: labels/counts remain full and spaced even if terminal color is unavailable.

## Recommended total chain

Use two worker/review cycles:

1. **Worker/review 1:** layout-only 3x2 grid + responsive fallback + rendered-output tests.
2. **Worker/review 2:** attention semantics + Catppuccin-style palette/default thresholds + color/style tests.

Do not combine both unless the first worker proves the layout is small. The layout change is the higher UX risk because it touches top-strip height, buffer coordinates, focus borders, and responsive behavior.
