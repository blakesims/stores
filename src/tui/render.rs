//! Widget tree: rows pane (top), 1-line selected-row footer, 1-line
//! status bar, plus optional filter palette / search bar overlays.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;
use std::time::{SystemTime, UNIX_EPOCH};

use super::app::{App, FlatRow, Mode};
use super::data::{
    cockpit_model, recent_exhaust, store_flow_model, ExternalReviewState, Row, Section,
    StoreFlowModel, StoreLane,
};
use super::semantics::{
    engine_presentation, external_review_presentation, external_review_runner_label,
    intake_presentation, observation_presentation, observation_watch_projection,
    observation_watch_slot_label, task_map_projection, task_presentation, task_watch_projection,
    task_watch_slot_label, MapCell, MapColor, PresentationSeverity, TaskMapProjection,
    WatchProjection, WatchSlotId,
};

/// Height (in rows) of the cockpit's top store-flow strip (5 cards drawn
/// inside a bordered block ⇒ 9 lines: top border + two 3-column slot rows
/// with wrapped full-word labels + one internal divider + bottom border).
/// Exposed so integration tests can derive the focused-table region's
/// vertical span without re-encoding the literal.
pub const TOP_STRIP_HEIGHT: u16 = 9;

const STORE_STRIP_MORE_WIDTH: u16 = 12;

const WATCH_OVERLAY0: Color = Color::Rgb(0x6c, 0x70, 0x86);
const WATCH_OVERLAY2: Color = Color::Rgb(0x93, 0x99, 0xb2);
const WATCH_SURFACE1: Color = Color::Rgb(0x45, 0x47, 0x5a);
const WATCH_TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
const WATCH_GREEN: Color = Color::Rgb(0xa6, 0xe3, 0xa1);
const WATCH_YELLOW: Color = Color::Rgb(0xf9, 0xe2, 0xaf);
const WATCH_PEACH: Color = Color::Rgb(0xfa, 0xb3, 0x87);
const WATCH_RED: Color = Color::Rgb(0xf3, 0x8b, 0xa8);

// Five-card mode uses lane-specific minimum widths. The previous single
// worst-case width made fullscreen-ish terminals collapse to one focused card
// even though most lanes have much shorter labels. Compute the minimum from the
// actual six slots so all cards appear as soon as each lane can keep full words
// readable.
const MIN_STORE_CARD_TITLE_PADDING: u16 = 4;

/// Height (in rows) of the bottom chrome painted below the focused-table
/// region: recent-exhaust strip (1) + hint line (1) + status bar (1) = 3.
/// Excludes the optional search bar (only present in `Mode::Search`).
pub const BOTTOM_CHROME_HEIGHT: u16 = 3;

pub fn draw(f: &mut Frame, app: &mut App) {
    if app.mode == Mode::Detail {
        draw_detail(f, app);
        return;
    }

    // Search-mode adds an extra 1-line input bar above the status bar.
    let search_bar = if app.mode == Mode::Search { 1 } else { 0 };

    if mission_compact_mode(app) {
        app.viewport_height = f.area().height.saturating_sub(1).max(1) as usize;
        let flat = app.flat_rows();
        clamp_scroll(app, flat.len());
        draw_rows(f, app, &flat, f.area());
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(TOP_STRIP_HEIGHT), // store-flow top strip (5 cards)
                Constraint::Min(1),                   // focused table | side detail
                Constraint::Length(1),                // recent-exhaust strip
                Constraint::Length(search_bar),       // search input
                Constraint::Length(1),                // hint line
                Constraint::Length(1),                // status bar
            ])
            .split(f.area());

        draw_store_strip(f, app, chunks[0]);

        let middle = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Ratio(13, 23), Constraint::Ratio(10, 23)])
            .split(chunks[1]);
        let focused_area = middle[0];
        let detail_area = middle[1];

        app.viewport_height = (focused_area.height as usize).max(1);
        let flat = app.flat_rows();
        clamp_scroll(app, flat.len());

        draw_focused_table(f, app, focused_area);
        draw_selected_detail(f, app, detail_area);
        draw_recent_exhaust(f, app, chunks[2]);

        if app.mode == Mode::Search {
            draw_search_bar(f, app, chunks[3]);
        }

        let hint = super::help::hint_for(app.mode);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint,
                Style::default().fg(Color::DarkGray),
            ))),
            chunks[4],
        );

        super::status_bar::render(f, app, chunks[5]);
    }

    if app.mode == Mode::Filter {
        draw_filter_palette(f, app);
    }
    if app.mode == Mode::ObsDraftConfirm {
        draw_obs_draft_popup(f, app);
    }
    if app.show_help {
        super::help::render_popup(f, app);
    }
}

fn draw_detail(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    let title = app
        .detail
        .as_ref()
        .map(|d| format!("stores watch · detail · {:?} {}", d.kind, d.display_id))
        .unwrap_or_else(|| "stores watch · detail".to_string());
    f.render_widget(
        Paragraph::new(title).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        chunks[0],
    );

    let body = Paragraph::new(super::detail::selected_detail_lines(app))
        .block(Block::default().borders(Borders::NONE));
    f.render_widget(body, chunks[1]);

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            super::help::hint_for(app.mode),
            Style::default().fg(Color::DarkGray),
        ))),
        chunks[2],
    );
    super::status_bar::render(f, app, chunks[3]);
}

fn draw_obs_draft_popup(f: &mut Frame, app: &App) {
    let draft = match &app.obs_draft_pending {
        Some(d) => d,
        None => return,
    };
    let area = centered_rect(70, 10, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" obs-draft (y to file, n/Esc to discard) ")
        .style(Style::default().fg(Color::Yellow));
    let body = Paragraph::new(vec![
        Line::from(format!("summary: {}", draft.summary)),
        Line::from(""),
        Line::from(truncate(&draft.body, 200)),
    ])
    .block(block);
    f.render_widget(body, area);
}

/// Slice the flat-row list to the current viewport window.
pub fn visible_window<'a>(app: &App, flat: &'a [FlatRow]) -> &'a [FlatRow] {
    let start = app.scroll_offset.min(flat.len());
    let end = (start + app.viewport_height).min(flat.len());
    &flat[start..end]
}

fn clamp_scroll(app: &mut App, total: usize) {
    if total == 0 {
        app.scroll_offset = 0;
        return;
    }
    let max_offset = total.saturating_sub(app.viewport_height.max(1));
    if app.scroll_offset > max_offset {
        app.scroll_offset = max_offset;
    }
}

fn draw_rows(f: &mut Frame, app: &App, flat: &[FlatRow], area: Rect) {
    // Mission-compact emergency view: keep the legacy header so the operator
    // sees lanes + daemon liveness in the same buffer as the system alert.
    let compact_window = mission_compact_window(app, flat);
    let window = compact_window.as_slice();
    let mut items: Vec<ListItem> = cockpit_header_items(app);
    if let Some(alert) = system_alert_item(app) {
        items.push(alert);
    }
    let mut last_section: Option<usize> = None;

    // Highlight the row currently under the cursor.
    let cursor = app.current_flat();

    for (i, fr) in window.iter().enumerate() {
        if last_section != Some(fr.section) {
            if let Some((sec, indices)) = app.sections.get(fr.section) {
                let collapsed = app.collapsed.contains(sec);
                let glyph = if collapsed { "▸" } else { "▾" };
                items.push(ListItem::new(Line::from(Span::styled(
                    format!("{glyph} {} ({})", sec.label(), indices.len()),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ))));
            }
            last_section = Some(fr.section);
        }
        let absolute_idx = app.scroll_offset + i;
        let selected = cursor == Some(absolute_idx);
        items.push(ListItem::new(format_row_line(
            &app.rows[fr.abs],
            selected,
            &app.external_review,
        )));
    }

    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no rows)",
            Style::default().fg(Color::DarkGray),
        ))));
    }

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::NONE)
            .title("stores watch · cockpit"),
    );
    f.render_widget(list, area);
}

/// Render the store-flow strip across the top of the cockpit. At common wide
/// widths all five cards are visible; below the readable-card threshold the
/// focused lane gets the available width and the hidden lanes collapse behind
/// an explicit `+ more` affordance.
fn draw_store_strip(f: &mut Frame, app: &App, area: Rect) {
    let model = store_flow_model(
        &app.rows,
        &app.system_health,
        &app.status_bar.daemon_liveness,
        &app.external_review,
    );
    let min_widths = lane_min_card_widths(&model);
    let all_cards_min_width: u16 = min_widths.iter().sum();
    if area.width >= all_cards_min_width {
        let widths = distribute_store_card_widths(area.width, min_widths);
        let cells = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(widths.map(Constraint::Length))
            .split(area);
        for (i, lane) in StoreLane::ALL.iter().enumerate() {
            draw_store_card(f, *lane, app.focused_store == *lane, &model, cells[i]);
        }
    } else {
        let focused_min_width = lane_min_card_width(app.focused_store, &model);
        let more_width = STORE_STRIP_MORE_WIDTH.min(area.width.saturating_sub(focused_min_width));
        let focused_width = area.width.saturating_sub(more_width);
        let cells = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(focused_width),
                Constraint::Length(more_width),
            ])
            .split(area);
        draw_store_card(f, app.focused_store, true, &model, cells[0]);
        draw_store_more_affordance(f, app.focused_store, cells[1]);
    }
}

fn lane_min_card_widths(model: &StoreFlowModel) -> [u16; 5] {
    StoreLane::ALL.map(|lane| lane_min_card_width(lane, model))
}

fn lane_min_card_width(lane: StoreLane, model: &StoreFlowModel) -> u16 {
    let slots = lane_card_slots(lane, model);
    let mut col_widths = [1_u16; 3];
    for (idx, slot) in slots.iter().enumerate() {
        let col = idx % 3;
        col_widths[col] = col_widths[col].max(slot_min_cell_width(*slot));
    }
    // Non-first cells lose one display column to the vertical separator.
    let inner = col_widths[0] + (col_widths[1] + 1) + (col_widths[2] + 1);
    let title = lane.label().chars().count() as u16 + MIN_STORE_CARD_TITLE_PADDING;
    inner.saturating_add(2).max(title)
}

fn slot_min_cell_width(slot: FlowSlot) -> u16 {
    let label_word = slot
        .label
        .split_whitespace()
        .map(|word| word.chars().count() as u16)
        .max()
        .unwrap_or(1);
    label_word.max(slot.meta().chars().count() as u16)
}

fn distribute_store_card_widths(total_width: u16, mut widths: [u16; 5]) -> [u16; 5] {
    let mut extra = total_width.saturating_sub(widths.iter().sum());
    let mut idx = 0;
    while extra > 0 {
        widths[idx] = widths[idx].saturating_add(1);
        extra -= 1;
        idx = (idx + 1) % widths.len();
    }
    widths
}

fn draw_store_more_affordance(f: &mut Frame, focused: StoreLane, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let hidden = StoreLane::ALL
        .iter()
        .filter(|lane| **lane != focused)
        .count();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            format!("+{hidden} more"),
            Style::default()
                .fg(Color::Gray)
                .add_modifier(Modifier::BOLD),
        ));
    f.render_widget(block, area);
    if area.width > 2 && area.height > 2 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "+ more",
                Style::default().fg(Color::Gray),
            ))),
            Rect {
                x: area.x + 1,
                y: area.y + 1,
                width: area.width.saturating_sub(2),
                height: 1,
            },
        );
    }
}

fn draw_store_card(
    f: &mut Frame,
    lane: StoreLane,
    focused: bool,
    model: &StoreFlowModel,
    area: Rect,
) {
    let (border_style, title_style) = if focused {
        (
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Style::default().fg(WATCH_SURFACE1),
            Style::default().fg(WATCH_OVERLAY2),
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(format!(" {} ", lane.label()), title_style));
    f.render_widget(block, area);

    if area.width < 5 || area.height < TOP_STRIP_HEIGHT {
        return;
    }

    let divider_style = watch_divider_style();
    let slots = lane_card_slots(lane, model);
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let col_widths = split_store_card_widths(lane, model, inner.width);
    let col_starts = [
        inner.x,
        inner.x + col_widths[0],
        inner.x + col_widths[0] + col_widths[1],
    ];
    let row_ys = [inner.y, inner.y + 4];
    let divider_y = inner.y + 3;
    let buf = f.buffer_mut();

    for sep_x in [col_starts[1], col_starts[2]] {
        for y in inner.y..inner.y + inner.height {
            buf[(sep_x, y)].set_symbol("│").set_style(divider_style);
        }
    }
    for x in inner.x..inner.x + inner.width {
        buf[(x, divider_y)].set_symbol("─").set_style(divider_style);
    }
    for sep_x in [col_starts[1], col_starts[2]] {
        buf[(sep_x, divider_y)]
            .set_symbol("┼")
            .set_style(divider_style);
    }

    for (idx, slot) in slots.iter().enumerate() {
        let row = idx / 3;
        let col = idx % 3;
        let cell_x = col_starts[col] + usize::from(col > 0) as u16;
        let cell_w = col_widths[col].saturating_sub(usize::from(col > 0) as u16);
        if cell_w == 0 {
            continue;
        }
        let slot_style = watch_slot_style(slot.severity());
        let meta = slot.meta();
        set_clipped(buf, cell_x, row_ys[row], cell_w, &meta, slot_style);
        let wrapped = wrap_label(slot.label, cell_w as usize, 2);
        if let Some(line) = wrapped.first() {
            set_clipped(buf, cell_x, row_ys[row] + 1, cell_w, line, slot_style);
        }
        if let Some(line) = wrapped.get(1) {
            set_clipped(buf, cell_x, row_ys[row] + 2, cell_w, line, slot_style);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WatchSeverityThresholds {
    flow_warning: usize,
    flow_high: usize,
    flow_critical: usize,
    fault_high: usize,
    fault_critical: usize,
}

impl Default for WatchSeverityThresholds {
    fn default() -> Self {
        Self {
            flow_warning: 3,
            flow_high: 6,
            flow_critical: 10,
            fault_high: 2,
            fault_critical: 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopSlotAttention {
    Exhaust,
    Flow,
    Fault,
    Neutral,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchSeverity {
    Dim,
    Normal,
    Warning,
    High,
    Critical,
    SuccessQuiet,
}

#[derive(Debug, Clone, Copy)]
struct FlowSlot {
    glyph: &'static str,
    label: &'static str,
    count: Option<usize>,
    attention: TopSlotAttention,
}

impl FlowSlot {
    fn new(
        glyph: &'static str,
        label: &'static str,
        count: usize,
        attention: TopSlotAttention,
    ) -> Self {
        Self {
            glyph,
            label,
            count: Some(count),
            attention,
        }
    }

    fn flag(glyph: &'static str, label: &'static str, attention: TopSlotAttention) -> Self {
        Self {
            glyph,
            label,
            count: None,
            attention,
        }
    }

    fn meta(self) -> String {
        match self.count {
            Some(count) => format!("{} {}", self.glyph, count),
            None => self.glyph.to_string(),
        }
    }

    fn severity(self) -> WatchSeverity {
        classify_watch_severity(self.attention, self.count.unwrap_or(1))
    }
}

fn classify_watch_severity(attention: TopSlotAttention, count: usize) -> WatchSeverity {
    classify_watch_severity_with_thresholds(attention, count, WatchSeverityThresholds::default())
}

fn classify_watch_severity_with_thresholds(
    attention: TopSlotAttention,
    count: usize,
    thresholds: WatchSeverityThresholds,
) -> WatchSeverity {
    match attention {
        TopSlotAttention::Exhaust => WatchSeverity::SuccessQuiet,
        TopSlotAttention::Flow => {
            if count == 0 {
                WatchSeverity::Dim
            } else if count >= thresholds.flow_critical {
                WatchSeverity::Critical
            } else if count >= thresholds.flow_high {
                WatchSeverity::High
            } else if count >= thresholds.flow_warning {
                WatchSeverity::Warning
            } else {
                WatchSeverity::Normal
            }
        }
        TopSlotAttention::Fault => {
            if count == 0 {
                WatchSeverity::Dim
            } else if count >= thresholds.fault_critical {
                WatchSeverity::Critical
            } else if count >= thresholds.fault_high {
                WatchSeverity::High
            } else {
                WatchSeverity::Warning
            }
        }
        TopSlotAttention::Neutral => {
            if count == 0 {
                WatchSeverity::Dim
            } else {
                WatchSeverity::Normal
            }
        }
    }
}

fn watch_slot_style(severity: WatchSeverity) -> Style {
    let style = match severity {
        WatchSeverity::Dim => Style::default().fg(WATCH_OVERLAY0),
        WatchSeverity::Normal => Style::default().fg(WATCH_TEXT),
        WatchSeverity::Warning => Style::default().fg(WATCH_YELLOW),
        WatchSeverity::High => Style::default().fg(WATCH_PEACH),
        WatchSeverity::Critical => Style::default().fg(WATCH_RED),
        WatchSeverity::SuccessQuiet => Style::default().fg(WATCH_GREEN),
    };
    if severity == WatchSeverity::Critical {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn watch_divider_style() -> Style {
    Style::default().fg(WATCH_SURFACE1)
}

fn lane_card_slots(lane: StoreLane, model: &StoreFlowModel) -> [FlowSlot; 6] {
    match lane {
        StoreLane::Intake => {
            let i = &model.intake;
            [
                FlowSlot::new("◌", "new", i.new, TopSlotAttention::Flow),
                FlowSlot::new("◆", "triage", i.triaging, TopSlotAttention::Flow),
                FlowSlot::new("◇", "needs info", i.waiting, TopSlotAttention::Flow),
                FlowSlot::new("✓", "routed", i.closed, TopSlotAttention::Exhaust),
                FlowSlot::new("△", "waiting", 0, TopSlotAttention::Flow),
                FlowSlot::new("▲", "errors", 0, TopSlotAttention::Fault),
            ]
        }
        StoreLane::Observations => {
            let o = &model.observations;
            [
                FlowSlot::new(
                    "◌",
                    observation_watch_slot_label(WatchSlotId::Front),
                    o.candidate,
                    TopSlotAttention::Flow,
                ),
                FlowSlot::new(
                    "◆",
                    observation_watch_slot_label(WatchSlotId::Work),
                    o.in_progress,
                    TopSlotAttention::Flow,
                ),
                FlowSlot::new(
                    "◇",
                    observation_watch_slot_label(WatchSlotId::Gate),
                    o.ready,
                    TopSlotAttention::Flow,
                ),
                FlowSlot::new(
                    "✓",
                    observation_watch_slot_label(WatchSlotId::Exit),
                    o.closed,
                    TopSlotAttention::Exhaust,
                ),
                FlowSlot::new(
                    "△",
                    observation_watch_slot_label(WatchSlotId::Wait),
                    o.waiting_kinds.values().sum::<usize>(),
                    TopSlotAttention::Flow,
                ),
                FlowSlot::new(
                    "▲",
                    observation_watch_slot_label(WatchSlotId::Fault),
                    o.errors,
                    TopSlotAttention::Fault,
                ),
            ]
        }
        StoreLane::Tasks => {
            let t = &model.tasks;
            [
                FlowSlot::new(
                    "◌",
                    task_watch_slot_label(WatchSlotId::Front),
                    t.queued,
                    TopSlotAttention::Flow,
                ),
                FlowSlot::new(
                    "◆",
                    task_watch_slot_label(WatchSlotId::Work),
                    t.work,
                    TopSlotAttention::Flow,
                ),
                FlowSlot::new(
                    "◇",
                    task_watch_slot_label(WatchSlotId::Gate),
                    t.gate,
                    TopSlotAttention::Flow,
                ),
                FlowSlot::new(
                    "✓",
                    task_watch_slot_label(WatchSlotId::Exit),
                    t.recently_terminal,
                    TopSlotAttention::Exhaust,
                ),
                FlowSlot::new(
                    "△",
                    task_watch_slot_label(WatchSlotId::Wait),
                    t.wait,
                    TopSlotAttention::Flow,
                ),
                FlowSlot::new(
                    "▲",
                    task_watch_slot_label(WatchSlotId::Fault),
                    t.fail,
                    TopSlotAttention::Fault,
                ),
            ]
        }
        StoreLane::ExternalReviews => {
            let r = &model.external_reviews;
            [
                FlowSlot::new("◌", "pending", r.pending, TopSlotAttention::Flow),
                FlowSlot::new("◆", "running", r.running, TopSlotAttention::Flow),
                FlowSlot::new("◇", "revise", r.revise, TopSlotAttention::Flow),
                FlowSlot::new("✓", "passed", r.passed, TopSlotAttention::Exhaust),
                FlowSlot::new("△", "waiting", r.wait, TopSlotAttention::Flow),
                FlowSlot::new("▲", "tool fault", r.tooling_held, TopSlotAttention::Fault),
            ]
        }
        StoreLane::EngineHealth => {
            let e = &model.engine;
            let health = super::data::SystemHealth {
                unfinished_dispatch_locks: e.unfinished_locks,
                oldest_claimed_at_epoch: e.oldest_lock_age_secs,
            };
            let daemon = if e.daemon_live {
                super::daemon::Liveness::Live { pid: 0 }
            } else {
                super::daemon::Liveness::Dead
            };
            let state = engine_presentation(&health, &daemon);
            let clear = usize::from(state.label == "clear" || state.label == "manual");
            let wait_slot = if state.label == "manual" {
                FlowSlot::flag("△", "manual", TopSlotAttention::Neutral)
            } else {
                FlowSlot::new("△", "waiting", 0, TopSlotAttention::Flow)
            };
            let fault_slot = if state.severity == PresentationSeverity::Fault {
                FlowSlot::flag("▲", "daemon down", TopSlotAttention::Fault)
            } else {
                FlowSlot::new("▲", "errors", 0, TopSlotAttention::Fault)
            };
            [
                FlowSlot::new("◌", "dispatch", 0, TopSlotAttention::Neutral),
                FlowSlot::new("◆", "runners", 0, TopSlotAttention::Neutral),
                FlowSlot::new("◇", "locks", e.unfinished_locks, TopSlotAttention::Fault),
                FlowSlot::new("✓", "clear", clear, TopSlotAttention::Exhaust),
                wait_slot,
                fault_slot,
            ]
        }
    }
}

fn split_store_card_widths(lane: StoreLane, model: &StoreFlowModel, width: u16) -> [u16; 3] {
    let slots = lane_card_slots(lane, model);
    let mut widths = [1_u16; 3];
    for (idx, slot) in slots.iter().enumerate() {
        let col = idx % 3;
        widths[col] = widths[col].max(slot_min_cell_width(*slot) + u16::from(col > 0));
    }
    distribute_three_widths(width, widths)
}

fn distribute_three_widths(total_width: u16, mut widths: [u16; 3]) -> [u16; 3] {
    let min_total: u16 = widths.iter().sum();
    if total_width < min_total {
        let base = total_width / 3;
        let rem = total_width % 3;
        return [base + u16::from(rem > 0), base + u16::from(rem > 1), base];
    }
    let mut extra = total_width - min_total;
    let mut idx = 0;
    while extra > 0 {
        widths[idx] = widths[idx].saturating_add(1);
        extra -= 1;
        idx = (idx + 1) % widths.len();
    }
    widths
}

fn set_clipped(
    buf: &mut ratatui::buffer::Buffer,
    x: u16,
    y: u16,
    width: u16,
    text: &str,
    style: Style,
) {
    let clipped: String = text.chars().take(width as usize).collect();
    buf.set_string(x, y, clipped, style);
}

fn wrap_label(label: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }
    let more = "+ more";
    let mut lines: Vec<String> = Vec::new();
    let mut hidden = false;
    for word in label.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > width {
            hidden = true;
            break;
        }
        if let Some(last) = lines.last_mut() {
            if last.chars().count() + 1 + word_len <= width {
                last.push(' ');
                last.push_str(word);
                continue;
            }
        }
        if lines.len() < max_lines {
            lines.push(word.to_string());
        } else {
            hidden = true;
            break;
        }
    }
    if hidden && more.chars().count() <= width {
        if lines.len() == max_lines {
            lines.pop();
        }
        lines.push(more.to_string());
    }
    lines
}

/// Render the focused-lane table (or engine panel when `EngineHealth` is
/// focused). Emits the section-grouped row list restricted to the focused
/// lane via `app.flat_rows()`, prefixed by the system-alert when the daemon
/// is dead.
fn draw_focused_table(f: &mut Frame, app: &App, area: Rect) {
    if app.focused_store == StoreLane::EngineHealth {
        draw_engine_panel(f, app, area);
        return;
    }
    let flat = app.flat_rows();
    // Hide TasksRecentlyTerminal from the focused-table view — terminal task
    // history belongs in the recent-exhaust strip, not the main rows.
    let window = visible_window(app, &flat);
    let mut items: Vec<ListItem> = Vec::new();
    if let Some(alert) = system_alert_item(app) {
        items.push(alert);
    }
    let cursor = app.current_flat();
    if app.focused_store == StoreLane::Tasks {
        items.push(ListItem::new(task_table_header(area.width)));
        append_task_projection_items(app, &flat, window, cursor, area.width, &mut items);
    } else if app.focused_store == StoreLane::Observations {
        append_observation_projection_items(app, &flat, window, cursor, &mut items);
    } else {
        let mut last_section: Option<usize> = None;
        for (i, fr) in window.iter().enumerate() {
            let sec_kind = app.sections.get(fr.section).map(|(s, _)| *s);
            if sec_kind == Some(Section::TasksRecentlyTerminal) {
                continue;
            }
            if last_section != Some(fr.section) {
                if let Some((sec, indices)) = app.sections.get(fr.section) {
                    let collapsed = app.collapsed.contains(sec);
                    let glyph = if collapsed { "▸" } else { "▾" };
                    items.push(ListItem::new(Line::from(Span::styled(
                        format!("{glyph} {} ({})", sec.label(), indices.len()),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ))));
                }
                last_section = Some(fr.section);
            }
            let absolute_idx = app.scroll_offset + i;
            let selected = cursor == Some(absolute_idx);
            items.push(ListItem::new(format_row_line(
                &app.rows[fr.abs],
                selected,
                &app.external_review,
            )));
        }
    }
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (no rows in this lane)",
            Style::default().fg(Color::DarkGray),
        ))));
    }
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::NONE)
            .title(format!(" {} ", app.focused_store.label())),
    );
    f.render_widget(list, area);
}

fn append_task_projection_items(
    app: &App,
    full: &[FlatRow],
    window: &[FlatRow],
    cursor: Option<usize>,
    area_width: u16,
    items: &mut Vec<ListItem<'static>>,
) {
    for slot in task_projection_display_order(full, app) {
        let total = task_projection_group_count(full, app, slot);
        let rows: Vec<(usize, &FlatRow, WatchProjection)> = window
            .iter()
            .enumerate()
            .filter_map(|(i, fr)| match &app.rows[fr.abs] {
                Row::Task(t) => {
                    let projection = task_watch_projection(t);
                    (projection.slot == slot).then_some((i, fr, projection))
                }
                _ => None,
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        items.push(ListItem::new(Line::from(Span::styled(
            format!(
                "▾ {} ({})",
                task_watch_slot_label(slot).to_ascii_uppercase(),
                total
            ),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))));
        for (i, fr, projection) in rows {
            let absolute_idx = app.scroll_offset + i;
            let selected = cursor == Some(absolute_idx);
            items.push(ListItem::new(format_row_line_for_task_projection(
                &app.rows[fr.abs],
                selected,
                &projection,
                area_width,
            )));
        }
    }
}

fn task_projection_display_order(full: &[FlatRow], app: &App) -> Vec<WatchSlotId> {
    const ORDER: [WatchSlotId; 6] = [
        WatchSlotId::Front,
        WatchSlotId::Work,
        WatchSlotId::Gate,
        WatchSlotId::Wait,
        WatchSlotId::Fault,
        WatchSlotId::Exit,
    ];
    ORDER
        .into_iter()
        .filter(|slot| task_projection_group_count(full, app, *slot) > 0)
        .collect()
}

fn task_projection_group_count(full: &[FlatRow], app: &App, slot: WatchSlotId) -> usize {
    full.iter()
        .filter(|fr| match &app.rows[fr.abs] {
            Row::Task(t) => task_watch_projection(t).slot == slot,
            _ => false,
        })
        .count()
}

fn append_observation_projection_items(
    app: &App,
    full: &[FlatRow],
    window: &[FlatRow],
    cursor: Option<usize>,
    items: &mut Vec<ListItem<'static>>,
) {
    for slot in observation_projection_display_order(full, app) {
        let total = observation_projection_group_count(full, app, slot);
        let rows: Vec<(usize, &FlatRow, WatchProjection)> = window
            .iter()
            .enumerate()
            .filter_map(|(i, fr)| {
                observation_projection_for_row(&app.rows[fr.abs])
                    .filter(|projection| projection.slot == slot)
                    .map(|projection| (i, fr, projection))
            })
            .collect();
        if rows.is_empty() {
            continue;
        }
        items.push(ListItem::new(Line::from(Span::styled(
            format!(
                "▾ {} ({})",
                observation_watch_slot_label(slot).to_ascii_uppercase(),
                total
            ),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))));
        for (i, fr, projection) in rows {
            let absolute_idx = app.scroll_offset + i;
            let selected = cursor == Some(absolute_idx);
            items.push(ListItem::new(format_row_line_for_observation_projection(
                &app.rows[fr.abs],
                selected,
                &projection,
            )));
        }
    }
}

fn observation_projection_for_row(row: &Row) -> Option<WatchProjection> {
    match row {
        Row::Obs(o) => Some(observation_watch_projection(o)),
        Row::CollapsedObs(c) => Some(observation_watch_projection(&c.representative)),
        _ => None,
    }
}

fn observation_projection_display_order(full: &[FlatRow], app: &App) -> Vec<WatchSlotId> {
    const ORDER: [WatchSlotId; 6] = [
        WatchSlotId::Front,
        WatchSlotId::Work,
        WatchSlotId::Gate,
        WatchSlotId::Wait,
        WatchSlotId::Fault,
        WatchSlotId::Exit,
    ];
    ORDER
        .into_iter()
        .filter(|slot| observation_projection_group_count(full, app, *slot) > 0)
        .collect()
}

fn observation_projection_group_count(full: &[FlatRow], app: &App, slot: WatchSlotId) -> usize {
    full.iter()
        .filter_map(|fr| {
            let row = &app.rows[fr.abs];
            observation_projection_for_row(row)
                .filter(|p| p.slot == slot)
                .map(|_| match row {
                    Row::CollapsedObs(c) => c.count,
                    _ => 1,
                })
        })
        .sum()
}

/// Engine-health panel: daemon liveness, dispatch-lock counts, oldest-lock
/// age, and an agent_runs note. No row list — this lane is a system-state
/// surface, not a row store.
fn draw_engine_panel(f: &mut Frame, app: &App, area: Rect) {
    let model = store_flow_model(
        &app.rows,
        &app.system_health,
        &app.status_bar.daemon_liveness,
        &app.external_review,
    );
    let e = &model.engine;
    let daemon_line = if e.daemon_live {
        Line::from(Span::styled(
            "daemon: LIVE",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        Line::from(Span::styled(
            "daemon: DEAD ⚠",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))
    };
    let locks_line = Line::from(Span::raw(format!(
        "unfinished_locks: {}",
        e.unfinished_locks
    )));
    let oldest_line = Line::from(Span::raw(match e.oldest_lock_age_secs {
        Some(secs) if secs >= 3600 => format!("oldest_lock_age: {}h", secs / 3600),
        Some(secs) if secs >= 60 => format!("oldest_lock_age: {}m", secs / 60),
        Some(_) => "oldest_lock_age: <1m".to_string(),
        None => "oldest_lock_age: —".to_string(),
    }));
    let runs_line = Line::from(Span::styled(
        format!(
            "agent_runs (recent): {} (not yet wired)",
            e.agent_runs_recent
        ),
        Style::default().fg(Color::DarkGray),
    ));
    let alert_lines: Vec<Line<'static>> = if !e.daemon_live && e.unfinished_locks > 0 {
        vec![Line::from(Span::styled(
            format!(
                "system-alert: daemon DEAD; {} dangling locks",
                e.unfinished_locks
            ),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ))]
    } else {
        Vec::new()
    };
    let mut lines = vec![daemon_line, locks_line, oldest_line, runs_line];
    if let Some(start) = app.engine_detail.recent_daemon_starts.first() {
        let started = start.started_at.as_deref().unwrap_or("-");
        lines.push(Line::from(Span::raw(format!(
            "recent_restart: pid={} at {}",
            start.pid, started
        ))));
    }
    for lock in app.engine_detail.unfinished_lock_rows.iter().take(3) {
        let agent = lock.agent_name.as_deref().unwrap_or("-");
        let heartbeat = lock.heartbeat_at.as_deref().unwrap_or("-");
        lines.push(Line::from(Span::raw(format!(
            "lock: {} runner={} last_progress={} {}",
            lock.display_id, agent, heartbeat, lock.liveness_label
        ))));
    }
    if !alert_lines.is_empty() {
        lines.push(Line::from(""));
        lines.extend(alert_lines);
    }
    let body = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::NONE)
            .title(" ENGINE · system health "),
    );
    f.render_widget(body, area);
}

/// Render the side detail pane for the currently selected row. When the
/// focused lane has no selectable row (empty lane / EngineHealth) the pane
/// displays the no-row placeholder.
fn draw_selected_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" detail ");
    let lines: Vec<Line<'static>> = if app.focused_store == StoreLane::EngineHealth {
        let pane_height = area.height.saturating_sub(2).max(1) as usize;
        super::detail::engine_lines(app)
            .into_iter()
            .take(pane_height)
            .map(Line::from)
            .collect()
    } else if let Some(row) = app.current_row() {
        let pane_height = area.height.saturating_sub(2).max(1) as usize;
        super::detail::lines_for_row(row, app)
            .into_iter()
            .take(pane_height)
            .map(Line::from)
            .collect()
    } else {
        vec![Line::from(Span::styled(
            "— no row selected —",
            Style::default().fg(Color::DarkGray),
        ))]
    };
    let body = Paragraph::new(lines).block(block);
    f.render_widget(body, area);
}

/// One-line strip: up to `limit` recent terminal task ids joined by " · ".
/// Renders a placeholder when the recent-exhaust list is empty.
fn draw_recent_exhaust(f: &mut Frame, app: &App, area: Rect) {
    let exhaust = recent_exhaust(&app.rows, 5);
    let text = if exhaust.is_empty() {
        "— no recent exhaust —".to_string()
    } else {
        let parts: Vec<String> = exhaust
            .iter()
            .filter_map(|row| match row {
                Row::Task(t) => Some(format!("{} {}", t.display_id, task_status_label(t))),
                _ => None,
            })
            .collect();
        format!("recent exhaust · {}", parts.join(" · "))
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}

fn mission_compact_mode(app: &App) -> bool {
    matches!(
        app.status_bar.daemon_liveness,
        super::daemon::Liveness::Dead
    ) && app.system_health.unfinished_dispatch_locks > 0
        && app
            .rows
            .iter()
            .any(|row| matches!(row, Row::CollapsedObs(_)))
}

fn mission_compact_window(app: &App, flat: &[FlatRow]) -> Vec<FlatRow> {
    let mut out = Vec::new();
    // Preserve one representative row per operator-actionable section so populated
    // default work isn't hidden when compact mode triggers. TasksRecentlyTerminal
    // (historical noise) and ObsOther (handled via the collapsed-row extend below)
    // are the only intentional exclusions.
    let wanted_single = [
        Section::TasksQueued,
        Section::TasksActionableCurrentWork,
        Section::ObsRatifiable,
        Section::TasksAcceptU3,
        Section::TasksIntegration,
        Section::TasksIntegratedAwaitingPostLand,
        Section::TasksIntegrationBlocked,
        Section::TasksBlockedNeedsAction,
        Section::TasksDeployRecovery,
        Section::TasksNeedsTriage,
        Section::IntakeHeld,
        Section::TasksHeldAiReview,
        Section::TasksHeldZombie,
        Section::ObsOpenNoContract,
        Section::IntakeOpen,
        Section::IntakeRouted,
        Section::ExternalReviewLane,
    ];
    for section in wanted_single {
        if let Some(fr) = flat.iter().find(|fr| app.sections[fr.section].0 == section) {
            out.push(*fr);
        }
    }
    out.extend(flat.iter().copied().filter(|fr| {
        app.sections[fr.section].0 == Section::ObsOther
            && matches!(app.rows.get(fr.abs), Some(Row::CollapsedObs(_)))
    }));
    out
}

fn system_alert_item(app: &App) -> Option<ListItem<'static>> {
    if !matches!(
        app.status_bar.daemon_liveness,
        super::daemon::Liveness::Dead
    ) {
        return None;
    }
    let count = app.system_health.unfinished_dispatch_locks;
    if count == 0 {
        return None;
    }
    let age_display = match app.system_health.oldest_claimed_at_epoch {
        Some(oldest) => format!("{}h", now_epoch().saturating_sub(oldest) / 3600),
        None => "?h".to_string(),
    };
    Some(ListItem::new(Line::from(Span::styled(
        format!(
            "system-alert: daemon DEAD; {count} dangling locks; oldest started {age_display} ago"
        ),
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    ))))
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn cockpit_header_items(app: &App) -> Vec<ListItem<'static>> {
    let model = cockpit_model(&app.rows, app.external_review.clone());
    let daemon = match app.status_bar.daemon_liveness {
        super::daemon::Liveness::Live { pid } => format!("daemon:LIVE pid={pid}"),
        super::daemon::Liveness::Dead => "daemon:DEAD".to_string(),
    };
    let external = match model.external_review {
        ExternalReviewState::Available { rows, lane, status } => {
            format!(
                "external review: lane={} status={} rows={rows}",
                lane.as_deref().unwrap_or("unknown"),
                status.as_deref().unwrap_or("unknown")
            )
        }
        ExternalReviewState::Unavailable { reason } => reason,
    };
    vec![
        ListItem::new(Line::from(Span::styled(
            format!(
                "{daemon} · cap active {}/{}",
                app.status_bar.cap.0, app.status_bar.cap.1
            ),
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))),
        ListItem::new(Line::from(Span::raw(format!(
            "lanes: execution={} review={} accept={} held={} active={} priority={}",
            model.execution, model.review, model.accept, model.held, model.active, model.priority
        )))),
        ListItem::new(Line::from(Span::raw(external))),
        ListItem::new(Line::from(Span::raw(""))),
    ]
}

fn format_row_line(
    row: &Row,
    selected: bool,
    external_review: &ExternalReviewState,
) -> Line<'static> {
    let base = match row {
        Row::Task(t) => format_task_line(t, external_review),
        Row::Obs(o) => format_obs_line(o, None),
        Row::CollapsedObs(c) => format_obs_line(&c.representative, Some(c)),
        Row::Review(r) => format_review_line(r),
        Row::Intake(i) => format_intake_line(i),
    };
    styled_row_line(base, selected)
}

fn format_row_line_for_task_projection(
    row: &Row,
    selected: bool,
    projection: &WatchProjection,
    area_width: u16,
) -> Line<'static> {
    let base = match row {
        Row::Task(t) => format_task_table_line(t, projection, area_width),
        _ => match row {
            Row::Obs(o) => format_obs_line(o, None),
            Row::CollapsedObs(c) => format_obs_line(&c.representative, Some(c)),
            Row::Review(r) => format_review_line(r),
            Row::Intake(i) => format_intake_line(i),
            Row::Task(_) => unreachable!(),
        },
    };
    styled_row_line(base, selected)
}

fn format_row_line_for_observation_projection(
    row: &Row,
    selected: bool,
    projection: &WatchProjection,
) -> Line<'static> {
    let base = match row {
        Row::Obs(o) => format_obs_line_with_projection(o, None, projection),
        Row::CollapsedObs(c) => {
            format_obs_line_with_projection(&c.representative, Some(c), projection)
        }
        _ => match row {
            Row::Task(t) => format_task_line(t, &ExternalReviewState::default()),
            Row::Review(r) => format_review_line(r),
            Row::Intake(i) => format_intake_line(i),
            Row::Obs(_) | Row::CollapsedObs(_) => unreachable!(),
        },
    };
    styled_row_line(base, selected)
}

fn styled_row_line(mut spans: Vec<Span<'static>>, selected: bool) -> Line<'static> {
    if selected {
        for s in spans.iter_mut() {
            s.style = s.style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
        }
    }
    Line::from(spans)
}

const TASK_ID_WIDTH: usize = 6;
const TASK_MAP_WIDTH: usize = 18;
const TASK_REASON_WIDTH: usize = 8;
const TASK_AGE_WIDTH: usize = 5;
const TASK_TIER_WIDTH: usize = 4;
const TASK_TABLE_GAPS: usize = 5;
const TASK_TABLE_PREFIX_WIDTH: usize = 2;

fn task_table_summary_width(area_width: u16) -> usize {
    let fixed = TASK_TABLE_PREFIX_WIDTH
        + TASK_ID_WIDTH
        + TASK_MAP_WIDTH
        + TASK_REASON_WIDTH
        + TASK_AGE_WIDTH
        + TASK_TIER_WIDTH
        + TASK_TABLE_GAPS;
    (area_width as usize).saturating_sub(fixed).max(12)
}

fn task_table_header(area_width: u16) -> Line<'static> {
    let summary_width = task_table_summary_width(area_width);
    Line::from(vec![Span::styled(
        format!(
            "  {:<id_w$} {:<summary_w$} {:<map_w$} {:<reason_w$} {:>age_w$} {:>tier_w$}",
            "ID",
            "SUMMARY",
            "MAP",
            "REASON",
            "AGE",
            "TIER",
            id_w = TASK_ID_WIDTH,
            summary_w = summary_width,
            map_w = TASK_MAP_WIDTH,
            reason_w = TASK_REASON_WIDTH,
            age_w = TASK_AGE_WIDTH,
            tier_w = TASK_TIER_WIDTH,
        ),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )])
}

fn format_task_table_line(
    t: &super::data::TaskRow,
    _projection: &WatchProjection,
    area_width: u16,
) -> Vec<Span<'static>> {
    let summary_width = task_table_summary_width(area_width);
    let map_projection = task_map_projection(t);
    let map_text = task_map_text(&map_projection);
    let reason = map_projection.reason.clone().unwrap_or_default();
    let age = compact_age_label(super::data::parse_epoch(&t.updated_at));
    let tier = t.tier_hint.as_deref().unwrap_or("");
    let title = truncate(&t.title, summary_width);

    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<id_w$}", t.display_id, id_w = TASK_ID_WIDTH),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" "),
        Span::raw(format!("{:<summary_w$}", title, summary_w = summary_width)),
        Span::raw(" "),
    ];
    spans.extend(task_map_spans(&map_projection, TASK_MAP_WIDTH));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(format!(
        "{:<reason_w$}",
        truncate(&reason, TASK_REASON_WIDTH),
        reason_w = TASK_REASON_WIDTH
    )));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(format!(
        "{:>age_w$}",
        age,
        age_w = TASK_AGE_WIDTH
    )));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(format!(
        "{:>tier_w$}",
        tier,
        tier_w = TASK_TIER_WIDTH
    )));

    let rendered_map_chars = map_text.chars().count();
    debug_assert!(rendered_map_chars <= TASK_MAP_WIDTH);
    spans
}

fn task_map_text(projection: &TaskMapProjection) -> String {
    task_map_tokens(projection)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("")
}

fn task_map_spans(projection: &TaskMapProjection, width: usize) -> Vec<Span<'static>> {
    let tokens = task_map_tokens(projection);
    let rendered: usize = tokens.iter().map(|(text, _)| text.chars().count()).sum();
    let mut spans: Vec<Span<'static>> = tokens
        .into_iter()
        .map(|(text, style)| Span::styled(text, style))
        .collect();
    if rendered < width {
        spans.push(Span::raw(" ".repeat(width - rendered)));
    }
    spans
}

fn task_map_tokens(projection: &TaskMapProjection) -> Vec<(String, Style)> {
    if let Some(fallback) = projection.fallback.as_ref() {
        return vec![(map_cell_text(fallback), map_cell_style(fallback))];
    }

    let mut tokens = vec![(
        map_cell_text(&projection.planning),
        map_cell_style(&projection.planning),
    )];
    if !projection.phases.is_empty() {
        tokens.push((" │ ".to_string(), watch_divider_style()));
        for (idx, cell) in projection.phases.iter().enumerate() {
            if idx > 0 {
                tokens.push((" ".to_string(), Style::default()));
            }
            tokens.push((map_cell_text(cell), map_cell_style(cell)));
        }
    }
    if let Some(wrap) = projection.wrap.as_ref() {
        tokens.push((" ".to_string(), Style::default()));
        tokens.push((map_cell_text(wrap), map_cell_style(wrap)));
    }
    tokens
}

fn map_cell_text(cell: &MapCell) -> String {
    match cell.cycle {
        Some(cycle) => format!("{}{}", cell.glyph.symbol(), superscript_number(cycle)),
        None => cell.glyph.symbol().to_string(),
    }
}

fn map_cell_style(cell: &MapCell) -> Style {
    let style = match cell.color_role {
        MapColor::Inactive => Style::default().fg(WATCH_OVERLAY0),
        MapColor::ActiveWork => Style::default().fg(Color::Cyan),
        MapColor::ActiveGate => Style::default().fg(WATCH_YELLOW),
        MapColor::Passed => Style::default().fg(WATCH_GREEN),
        MapColor::Failed => Style::default().fg(WATCH_RED),
        MapColor::Waiting => Style::default().fg(WATCH_PEACH),
        MapColor::Unknown => Style::default().fg(WATCH_OVERLAY2),
    };
    if cell.active {
        style.add_modifier(Modifier::BOLD)
    } else {
        style
    }
}

fn superscript_number(n: i64) -> String {
    n.to_string()
        .chars()
        .map(|ch| match ch {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            '-' => '⁻',
            other => other,
        })
        .collect()
}

fn compact_age_label(epoch: Option<i64>) -> String {
    age_label(epoch)
        .strip_prefix("age:")
        .unwrap_or("-")
        .to_string()
}

fn age_label(epoch: Option<i64>) -> String {
    match epoch {
        Some(e) => {
            let diff = now_epoch().saturating_sub(e);
            if diff >= 3600 {
                format!("age:{}h", diff / 3600)
            } else if diff >= 60 {
                format!("age:{}m", diff / 60)
            } else {
                "age:<1m".to_string()
            }
        }
        None => "age:-".to_string(),
    }
}

fn format_task_line(
    t: &super::data::TaskRow,
    external_review: &ExternalReviewState,
) -> Vec<Span<'static>> {
    format_task_line_with_status(t, external_review, task_status_label(t))
}

fn format_task_line_with_status(
    t: &super::data::TaskRow,
    external_review: &ExternalReviewState,
    status_label: String,
) -> Vec<Span<'static>> {
    let live_runner = t.live_run.as_ref().and_then(|live| {
        let role = live.role.trim();
        if role.is_empty() {
            return None;
        }
        let runner = live
            .runner
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("unknown");
        Some(format!("{role}({runner})"))
    });
    let claimed_runner = t
        .claimed_by
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let runner = live_runner.or(claimed_runner);
    let age = age_label(super::data::parse_epoch(&t.updated_at));
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<6}", t.display_id),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            format!("{:<24}", status_label),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" "),
        Span::raw(task_progress_text(t, external_review)),
    ];
    if let Some(runner) = runner {
        spans.push(Span::raw(format!("runner:{runner} ")));
    }
    spans.push(Span::raw(format!("{age} ")));
    spans.push(Span::raw(truncate(&task_snippet(t), 60)));
    spans
}

fn format_obs_line(
    o: &super::data::ObsRow,
    collapsed: Option<&super::data::CollapsedObsRow>,
) -> Vec<Span<'static>> {
    let presentation = observation_presentation(o);
    let status = format!("{} {}", presentation.glyph, presentation.label);
    let (display_id, badge, summary_prefix) = collapsed
        .map(|c| {
            (
                c.primary_display_id.as_str(),
                format!(" ×{}", c.count),
                format!("{} ", c.summary),
            )
        })
        .unwrap_or((o.display_id.as_str(), String::new(), String::new()));
    let tier = o.tier_hint.as_deref().filter(|s| !s.is_empty());
    let linked = collapsed
        .is_none()
        .then(|| o.task_id.as_deref().filter(|s| !s.is_empty()))
        .flatten();
    let priority = if let Some(t) = tier {
        format!("{}/{}", o.priority, t)
    } else {
        o.priority.clone()
    };
    let next = observation_next_action(o, &presentation.label);
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<6}", display_id),
            Style::default().fg(Color::Magenta),
        ),
        Span::styled(
            format!("{:<20}", status),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" "),
        Span::raw(format!("{:<9}{} ", priority, badge)),
        Span::raw(format!("next:{:<18} ", next)),
    ];
    if let Some(l) = linked {
        spans.push(Span::raw(format!("linked:{l} ")));
    }
    spans.push(Span::raw(truncate(
        &format!("{}{}", summary_prefix, obs_snippet(o)),
        60,
    )));
    spans
}

fn format_obs_line_with_projection(
    o: &super::data::ObsRow,
    collapsed: Option<&super::data::CollapsedObsRow>,
    projection: &WatchProjection,
) -> Vec<Span<'static>> {
    let (display_id, badge, summary_prefix) = collapsed
        .map(|c| {
            (
                c.primary_display_id.as_str(),
                format!(" ×{}", c.count),
                format!("{} ", c.summary),
            )
        })
        .unwrap_or((o.display_id.as_str(), String::new(), String::new()));
    let stage = if projection.slot == WatchSlotId::Front {
        projection.glyph.to_string()
    } else {
        format!("{} {}", projection.glyph, projection.row_stage)
    };
    let next = projection.next_action.unwrap_or("triage");
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<6}", display_id),
            Style::default().fg(Color::Magenta),
        ),
        Span::styled(format!("{:<24}", stage), Style::default().fg(Color::Yellow)),
        Span::raw(" "),
        Span::raw(format!("{:<9}{} ", o.priority, badge)),
        Span::raw(format!("next:{:<18} ", next)),
    ];
    spans.push(Span::raw(truncate(
        &format!("{}{}", summary_prefix, obs_snippet(o)),
        60,
    )));
    spans
}

fn observation_next_action(o: &super::data::ObsRow, label: &str) -> &'static str {
    match label {
        "candidate" => "triage",
        "investigate" => "gather evidence",
        "needs-info" => "answer info",
        "external-dependency" => "check dependency",
        "triage-capacity" => "assign triage",
        "contract-draft" => "approve/revise",
        "contract-approved" => "promote/resolve",
        "arch-gate" => "architecture review",
        "resolving" => "resolve",
        "investigation-failed" => "inspect failure",
        "addressed" | "wont-fix" | "superseded" => "done",
        _ if o
            .waiting_kind
            .as_deref()
            .is_some_and(|s| s == "info_needed") =>
        {
            "answer info"
        }
        _ => "triage",
    }
}

fn format_intake_line(i: &super::data::IntakeRow) -> Vec<Span<'static>> {
    let source = i
        .source_agent
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("-");
    let cluster = i
        .cluster_key
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|c| format!("cluster:{c} "));
    let age_source = i
        .captured_at
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&i.updated_at);
    let age = age_label(super::data::parse_epoch(age_source));
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<6}", i.display_id),
            Style::default().fg(Color::Green),
        ),
        Span::styled(
            format!("{:<24}", intake_status_label(i)),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" "),
        Span::raw(format!(
            "priority:{} ",
            i.priority.as_deref().unwrap_or("normal")
        )),
        Span::raw(format!("source:{source} ")),
    ];
    if let Some(c) = cluster {
        spans.push(Span::raw(c));
    }
    spans.push(Span::raw(format!("{age} ")));
    spans.push(Span::raw(truncate(&intake_snippet(i), 60)));
    spans
}

fn format_review_line(r: &super::data::ReviewRow) -> Vec<Span<'static>> {
    let verdict = format!(
        "verdict:{}",
        r.verdict
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("-")
    );
    let attempts = format!("attempts:{}", r.attempts);
    let held = r
        .held_reason
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "none")
        .map(|h| format!("held:{h} "));
    let age = age_label(
        r.started_at
            .as_deref()
            .filter(|s| !s.is_empty())
            .and_then(super::data::parse_epoch),
    );
    let sha = r
        .base_sha
        .as_deref()
        .filter(|s| s.len() >= 7)
        .map(|s| format!("sha:{} ", &s[..7]));
    let runner = external_review_runner_label(r);
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<6}", r.display_id),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            format!("{:<24}", review_status_label(r)),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" "),
        Span::raw(format!("{verdict} ")),
        Span::raw(format!("{attempts} ")),
    ];
    if let Some(h) = held {
        spans.push(Span::raw(h));
    }
    spans.push(Span::raw(format!("{age} ")));
    if let Some(s) = sha {
        spans.push(Span::raw(s));
    }
    spans.push(Span::raw(truncate(
        &format!("task={} runner={}", r.task_id, runner),
        60,
    )));
    spans
}

fn task_status_label(t: &super::data::TaskRow) -> String {
    let presentation = task_presentation(t);
    presentation_label(presentation)
}

fn intake_status_label(i: &super::data::IntakeRow) -> String {
    presentation_label(intake_presentation(i))
}

fn review_status_label(r: &super::data::ReviewRow) -> String {
    presentation_label(external_review_presentation(r))
}

fn presentation_label(presentation: super::semantics::Presentation) -> String {
    match presentation.signal {
        Some(signal) => format!("{} {} {}", presentation.glyph, presentation.label, signal),
        None => format!("{} {}", presentation.glyph, presentation.label),
    }
}

fn task_progress_text(t: &super::data::TaskRow, external_review: &ExternalReviewState) -> String {
    let progress = super::progress::task_progress(t, external_review);
    if progress.text == t.status
        || progress.text.contains("lifecycle=")
        || progress.text.contains("active_step=")
        || progress.text.contains("integration_step=")
        || progress.text.contains("active:none:none")
    {
        String::new()
    } else {
        format!("{} ", progress.text)
    }
}

fn task_snippet(t: &super::data::TaskRow) -> String {
    let mut parts = Vec::new();
    if let Some(tier) = t.tier_hint.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("tier:{tier}"));
    }
    if super::data::task_lifecycle(t) == "active" && super::data::task_active_step(t) == "planning"
    {
        if let Some(pid) = t.drive_pid {
            parts.push(format!("drive_pid:{pid}"));
        } else if t
            .workspace_path
            .as_deref()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
        {
            parts.push("owner:none".to_string());
        } else {
            parts.push("workspace:none".to_string());
        }
    }
    if super::data::task_is_blocked(t) {
        if let Some(reason) = t.blocked_reason.as_deref().filter(|s| !s.is_empty()) {
            parts.push(format!("reason:{reason}"));
        }
    }
    parts.push(t.title.clone());
    parts.join(" · ")
}

fn obs_snippet(o: &super::data::ObsRow) -> String {
    let mut parts = Vec::new();
    if let Some(reason) = o.lock_reason.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("held:{reason}"));
    }
    parts.push(o.summary.clone());
    parts.join(" · ")
}

fn intake_snippet(i: &super::data::IntakeRow) -> String {
    let mut parts = Vec::new();
    if let Some(reason) = i.held_reason.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("held:{reason}"));
    }
    if let Some(next) = i.next_action.as_deref().filter(|s| !s.is_empty()) {
        parts.push(format!("next:{next}"));
    }
    parts.push(i.summary.clone());
    parts.join(" · ")
}

fn draw_search_bar(f: &mut Frame, app: &App, area: Rect) {
    let line = Line::from(vec![
        Span::styled("/", Style::default().fg(Color::Yellow)),
        Span::raw(app.search.query.clone()),
        Span::raw("  "),
        Span::styled(
            format!("{} hit(s)", app.search.hits.len()),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_filter_palette(f: &mut Frame, app: &App) {
    let palette = match &app.filter_palette {
        Some(p) => p,
        None => return,
    };
    let area = centered_rect(60, 7, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" filter (state= priority= tier= since=, Enter applies, Esc cancels) ")
        .style(Style::default().fg(Color::Yellow));
    let body = Paragraph::new(vec![
        Line::from(format!("> {}", palette.buffer)),
        Line::from(""),
        Line::from(format!(
            "draft: state={:?} priority={:?} tier={:?} since={:?}",
            palette.draft.state, palette.draft.priority, palette.draft.tier, palette.draft.since
        )),
    ])
    .block(block);
    f.render_widget(body, area);
}

fn centered_rect(width_pct: u16, height: u16, area: Rect) -> Rect {
    let popup_w = area.width * width_pct / 100;
    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x: popup_x,
        y: popup_y,
        width: popup_w,
        height,
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{App, StatusBar, TuiOpts};
    use crate::tui::daemon::Liveness;
    use crate::tui::data::{
        CycleReviewGate, IntakeRow, ObsRow, PlanReviewGate, ReviewRow, Row, SystemHealth,
        TaskCycleEntry, TaskPlanReviewEntry, TaskRow,
    };
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn line_text(line: Line<'static>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    fn painted_buffer(app: &mut App) -> String {
        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal.draw(|f| draw(f, app)).expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut painted = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                painted.push_str(buf[(x, y)].symbol());
            }
            painted.push('\n');
        }
        painted
    }

    #[test]
    fn lane_cards_use_shared_flow_glyph_slots_and_task_wait_fail_split() {
        let mut model = StoreFlowModel::default();
        model.intake.new = 1;
        model.intake.triaging = 2;
        model.intake.waiting = 3;
        model.intake.closed = 4;
        model.observations.candidate = 5;
        model.observations.in_progress = 6;
        model.observations.ready = 7;
        model.observations.closed = 8;
        model.tasks.queued = 9;
        model.tasks.work = 10;
        model.tasks.gate = 11;
        model.tasks.wait = 12;
        model.tasks.fail = 13;
        model.tasks.recently_terminal = 14;
        model.external_reviews.pending = 15;
        model.external_reviews.running = 16;
        model.external_reviews.revise = 17;
        model.external_reviews.passed = 18;
        model.external_reviews.tooling_held = 19;
        model.engine.daemon_live = false;

        let lanes = [
            StoreLane::Intake,
            StoreLane::Observations,
            StoreLane::Tasks,
            StoreLane::ExternalReviews,
            StoreLane::EngineHealth,
        ];
        for lane in lanes {
            let slots = lane_card_slots(lane, &model);
            for glyph in ["◌", "◆", "◇", "✓", "△", "▲"] {
                assert!(
                    slots.iter().any(|slot| slot.glyph == glyph),
                    "{lane:?} missing {glyph}: {slots:?}"
                );
            }
            assert_eq!(slots.len(), 6, "{lane:?} must expose six stable slots");
        }

        let tasks = lane_card_slots(StoreLane::Tasks, &model);
        assert_eq!(tasks[4].label, "waiting");
        assert_eq!(tasks[4].count, Some(12));
        assert_eq!(tasks[4].attention, TopSlotAttention::Flow);
        assert_eq!(tasks[5].label, "failed");
        assert_eq!(tasks[5].count, Some(13));
        assert_eq!(tasks[5].attention, TopSlotAttention::Fault);
    }

    #[test]
    fn top_slot_severity_thresholds_are_semantic_not_count_only() {
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Flow, 0),
            WatchSeverity::Dim
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Flow, 1),
            WatchSeverity::Normal
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Flow, 3),
            WatchSeverity::Warning
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Flow, 8),
            WatchSeverity::High
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Flow, 10),
            WatchSeverity::Critical
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Fault, 0),
            WatchSeverity::Dim
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Fault, 1),
            WatchSeverity::Warning
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Fault, 2),
            WatchSeverity::High
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Fault, 4),
            WatchSeverity::Critical
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Exhaust, 100),
            WatchSeverity::SuccessQuiet
        );
        assert_eq!(
            classify_watch_severity(TopSlotAttention::Neutral, 100),
            WatchSeverity::Normal
        );
    }

    #[test]
    fn top_card_render_uses_slot_severity_colors_and_muted_dividers() {
        let mut model = StoreFlowModel::default();
        model.tasks.wait = 8;
        model.tasks.fail = 4;
        model.tasks.recently_terminal = 100;

        let backend = TestBackend::new(80, TOP_STRIP_HEIGHT);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal
            .draw(|f| draw_store_card(f, StoreLane::Tasks, true, &model, f.area()))
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let painted = buffer_to_string(&buf);
        assert!(
            painted.contains("△ 8"),
            "missing waiting pile-up:\n{painted}"
        );
        assert!(painted.contains("▲ 4"), "missing fault pile-up:\n{painted}");
        assert!(
            painted.contains("✓ 100"),
            "missing quiet exhaust count:\n{painted}"
        );

        let cell_with = |needle: &str| -> ratatui::buffer::Cell {
            for y in 0..buf.area.height {
                for x in 0..buf.area.width {
                    if buf[(x, y)].symbol() == needle {
                        return buf[(x, y)].clone();
                    }
                }
            }
            panic!("missing cell {needle:?}:\n{painted}");
        };

        assert_eq!(cell_with("△").fg, WATCH_PEACH);
        assert_eq!(cell_with("▲").fg, WATCH_RED);
        assert_eq!(cell_with("✓").fg, WATCH_GREEN);
        assert!(cell_with("▲").modifier.contains(Modifier::BOLD));

        let divider = (0..buf.area.height)
            .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
            .map(|(x, y)| &buf[(x, y)])
            .find(|cell| cell.symbol() == "│" && cell.fg == WATCH_SURFACE1)
            .unwrap_or_else(|| panic!("missing muted divider:\n{painted}"));
        assert!(!divider.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn top_cards_render_full_word_three_by_two_grid() {
        let mut model = StoreFlowModel::default();
        model.observations.candidate = 8;
        model.observations.in_progress = 0;
        model.observations.ready = 0;
        model.observations.closed = 0;
        model.observations.waiting_kinds.insert("human".into(), 8);
        model.tasks.queued = 2;
        model.tasks.work = 2;
        model.tasks.gate = 1;
        model.tasks.recently_terminal = 3;
        model.tasks.wait = 8;
        model.tasks.fail = 4;
        model.external_reviews.tooling_held = 5;
        model.engine.daemon_live = false;

        let backend = TestBackend::new(260, TOP_STRIP_HEIGHT);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal
            .draw(|f| {
                let cells = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Ratio(1, 5); 5])
                    .split(f.area());
                for (i, lane) in StoreLane::ALL.iter().enumerate() {
                    draw_store_card(f, *lane, *lane == StoreLane::Tasks, &model, cells[i]);
                }
            })
            .expect("draw");
        let buf = terminal.backend().buffer().clone();
        let painted = buffer_to_string(&buf);

        for needle in [
            "candidates",
            "investigate",
            "contract gate",
            "closed",
            "waiting",
            "errors",
            "queued",
            "working",
            "gate",
            "done",
            "failed",
            "tool fault",
            "manual",
        ] {
            assert!(painted.contains(needle), "missing {needle:?}:\n{painted}");
        }
        for glued in ["◌cand", "◆inv", "◌q", "◆wrk", "✓dn", "△w", "▲err", "▲tool"] {
            assert!(
                !painted.contains(glued),
                "found glued token {glued:?}:\n{painted}"
            );
        }
        assert!(
            painted.contains("◌ 8"),
            "missing separated observation count:\n{painted}"
        );
        assert!(
            painted.contains("△ 8"),
            "missing separated waiting count:\n{painted}"
        );
        assert!(
            painted.contains("│"),
            "missing vertical dividers:\n{painted}"
        );
        assert!(
            painted.contains("─"),
            "missing horizontal divider:\n{painted}"
        );

        model.engine.unfinished_locks = 2;
        let backend = TestBackend::new(80, TOP_STRIP_HEIGHT);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal
            .draw(|f| draw_store_card(f, StoreLane::EngineHealth, true, &model, f.area()))
            .expect("draw");
        let fault_painted = buffer_to_string(terminal.backend().buffer());
        assert!(
            fault_painted.contains("daemon down"),
            "missing daemon down in rendered engine fault card:\n{fault_painted}"
        );
    }

    #[test]
    fn engine_card_distinguishes_manual_from_daemon_down_fault() {
        let mut manual = StoreFlowModel::default();
        manual.engine.daemon_live = false;
        let manual_slots = lane_card_slots(StoreLane::EngineHealth, &manual);
        assert_eq!(manual_slots[4].label, "manual");
        assert_eq!(manual_slots[5].label, "errors");
        assert_eq!(manual_slots[5].count, Some(0));

        let mut fault = StoreFlowModel::default();
        fault.engine.daemon_live = false;
        fault.engine.unfinished_locks = 2;
        let fault_slots = lane_card_slots(StoreLane::EngineHealth, &fault);
        assert_eq!(fault_slots[2].label, "locks");
        assert_eq!(fault_slots[2].count, Some(2));
        assert_eq!(fault_slots[5].label, "daemon down");
    }

    fn task_row(status: &str, reason: Option<&str>) -> Row {
        Row::Task(TaskRow {
            display_id: "T999".to_string(),
            status: status.to_string(),
            title: "render status".to_string(),
            blocked_reason: reason.map(str::to_string),
            blocked_reason_class: Some(crate::tui::data::blocked_reason_class(reason).to_string()),
            ..Default::default()
        })
    }

    fn synthetic_rows(n: usize) -> Vec<Row> {
        (0..n)
            .map(|i| {
                Row::Task(TaskRow {
                    display_id: format!("T{:03}", i),
                    status: "executing".to_string(),
                    title: format!("synthetic task {i}"),
                    claimed_by: None,
                    updated_at: format!("2026-05-{:02}", (i % 28) + 1),
                    ..Default::default()
                })
            })
            .collect()
    }

    /// AC2.5: with 200 synthetic rows and viewport=10, only 10 row widgets
    /// are rendered into the list, and selection stays in view on PgDn.
    #[test]
    fn task_terminal_and_blocked_status_labels_render_operator_actionably() {
        let closed = line_text(format_row_line(
            &task_row("closed_out_of_band", None),
            false,
            &ExternalReviewState::default(),
        ));
        assert!(closed.contains("■ done"), "{closed}");
        assert!(!closed.contains("lifecycle="), "{closed}");
        assert!(!closed.contains("active_step="), "{closed}");
        assert!(!closed.contains("integration_step="), "{closed}");

        let blocked = line_text(format_row_line(
            &task_row("blocked", Some("rate limit 429")),
            false,
            &ExternalReviewState::default(),
        ));
        assert!(blocked.contains("△ rate-limited"), "{blocked}");
        assert!(!blocked.contains("runner:none"), "{blocked}");

        let unknown = line_text(format_row_line(
            &task_row("blocked", Some("opaque")),
            false,
            &ExternalReviewState::default(),
        ));
        assert!(unknown.contains("▲ unknown-blocked"), "{unknown}");
    }

    #[test]
    fn task_rows_render_semantic_task_stages_and_blockers() {
        let cases = [
            (
                TaskRow {
                    display_id: "T201".to_string(),
                    status: "blocked".to_string(),
                    title: "runner failed".to_string(),
                    blocked: Some(true),
                    blocker_kind: Some("runner".to_string()),
                    blocked_reason: Some(r#"{"exit_code":42,"kind":"runner_crash"}"#.to_string()),
                    ..Default::default()
                },
                ["▲ runner-failed exit 42", ""],
            ),
            (
                TaskRow {
                    display_id: "T202".to_string(),
                    status: "planning".to_string(),
                    title: "capacity wait".to_string(),
                    blocked: Some(true),
                    blocker_kind: Some("capacity".to_string()),
                    ..Default::default()
                },
                ["△ waiting-capacity", ""],
            ),
            (
                TaskRow {
                    display_id: "T203".to_string(),
                    status: "plan_review".to_string(),
                    title: "plan gate".to_string(),
                    lifecycle: Some("active".to_string()),
                    active_step: Some("planning_review".to_string()),
                    ..Default::default()
                },
                ["◇ plan-gate", ""],
            ),
            (
                TaskRow {
                    display_id: "T204".to_string(),
                    status: "executing".to_string(),
                    title: "exec".to_string(),
                    lifecycle: Some("active".to_string()),
                    active_step: Some("coding".to_string()),
                    workspace_path: Some("/tmp/t204".to_string()),
                    ..Default::default()
                },
                ["▣ exec", ""],
            ),
            (
                TaskRow {
                    display_id: "T205".to_string(),
                    status: "in_review".to_string(),
                    title: "accept".to_string(),
                    lifecycle: Some("active".to_string()),
                    active_step: Some("wrapping".to_string()),
                    workspace_path: Some("/tmp/t205".to_string()),
                    ..Default::default()
                },
                ["▰ accept", ""],
            ),
        ];

        for (task, expected) in cases {
            let text = line_text(format_row_line(
                &Row::Task(task),
                false,
                &ExternalReviewState::default(),
            ));
            assert!(text.contains(expected[0]), "{text}");
            assert!(!text.contains("active:none:none"), "{text}");
            assert!(!text.contains("runner:none"), "{text}");
            assert!(!text.contains("lifecycle="), "{text}");
            assert!(!text.contains("active_step="), "{text}");
            assert!(!text.contains("integration_step="), "{text}");
        }
    }

    #[test]
    fn task_projection_row_labels_render_dense_table_fields() {
        let queued = TaskRow {
            display_id: "T301".to_string(),
            status: "ready".to_string(),
            title: "queued".to_string(),
            lifecycle: Some("queued".to_string()),
            total_phases: Some(2),
            ..Default::default()
        };
        let queued_projection = task_watch_projection(&queued);
        let queued_text = line_text(format_row_line_for_task_projection(
            &Row::Task(queued),
            false,
            &queued_projection,
            120,
        ));
        assert!(queued_text.contains("T301"), "{queued_text}");
        assert!(queued_text.contains("queued"), "{queued_text}");
        assert!(queued_text.contains("◌ │ · ·"), "{queued_text}");
        assert!(!queued_text.contains("workspace:none"), "{queued_text}");

        let waiting = TaskRow {
            display_id: "T302".to_string(),
            status: "blocked".to_string(),
            title: "capacity".to_string(),
            blocked: Some(true),
            blocker_kind: Some("capacity".to_string()),
            ..Default::default()
        };
        let waiting_projection = task_watch_projection(&waiting);
        let waiting_text = line_text(format_row_line_for_task_projection(
            &Row::Task(waiting),
            false,
            &waiting_projection,
            120,
        ));
        assert!(waiting_text.contains("△"), "{waiting_text}");
        assert!(waiting_text.contains("capacity"), "{waiting_text}");
        assert!(!waiting_text.contains("waiting-capacity"), "{waiting_text}");

        let failed = TaskRow {
            display_id: "T303".to_string(),
            status: "blocked".to_string(),
            title: "runner".to_string(),
            blocked: Some(true),
            blocker_kind: Some("runner".to_string()),
            blocked_reason: Some(r#"{"exit_code":42}"#.to_string()),
            ..Default::default()
        };
        let failed_projection = task_watch_projection(&failed);
        let failed_text = line_text(format_row_line_for_task_projection(
            &Row::Task(failed),
            false,
            &failed_projection,
            120,
        ));
        assert!(failed_text.contains("▲"), "{failed_text}");
        assert!(failed_text.contains("runner"), "{failed_text}");
        assert!(!failed_text.contains("exit 42"), "{failed_text}");
    }

    #[test]
    fn task_focused_table_renders_aligned_static_map_columns() {
        let mut app = App::new(TuiOpts::default());
        app.rows = vec![
            Row::Task(TaskRow {
                display_id: "T001".to_string(),
                status: "ready".to_string(),
                title: "synthetic queued inactive plan task".to_string(),
                lifecycle: Some("queued".to_string()),
                total_phases: Some(3),
                tier_hint: Some("T3".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                display_id: "T011".to_string(),
                status: "executing".to_string(),
                title: "retrying phase two after review".to_string(),
                lifecycle: Some("active".to_string()),
                active_step: Some("coding".to_string()),
                current_phase: Some(2),
                current_cycle: Some(2),
                total_phases: Some(3),
                plan_review_entries: vec![TaskPlanReviewEntry {
                    gate: PlanReviewGate::Ready,
                    ..Default::default()
                }],
                cycle_entries: vec![TaskCycleEntry {
                    phase: 1,
                    cycle: 1,
                    review_gate: Some(CycleReviewGate::Pass),
                    ..Default::default()
                }],
                tier_hint: Some("T3".to_string()),
                workspace_path: Some("/tmp/t011".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                display_id: "T010".to_string(),
                status: "blocked".to_string(),
                title: "synthetic fake runner nonzero blocked task".to_string(),
                lifecycle: Some("active".to_string()),
                blocked: Some(true),
                blocker_kind: Some("runner".to_string()),
                total_phases: Some(3),
                tier_hint: Some("T3".to_string()),
                ..Default::default()
            }),
        ];
        app.sections = classify(&app.rows);
        app.apply_sort();
        app.viewport_height = 20;

        let backend = TestBackend::new(150, 14);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal
            .draw(|f| draw_focused_table(f, &app, f.area()))
            .expect("draw focused table");
        let painted = buffer_to_string(terminal.backend().buffer());
        let lines: Vec<&str> = painted.lines().collect();
        let header = lines
            .iter()
            .find(|line| line.contains("ID") && line.contains("SUMMARY") && line.contains("MAP"))
            .expect("header line");
        let queued = lines.iter().find(|line| line.contains("T001")).unwrap();
        let active = lines.iter().find(|line| line.contains("T011")).unwrap();
        let failed = lines.iter().find(|line| line.contains("T010")).unwrap();

        let char_pos = |line: &str, needle: &str| -> Option<usize> {
            line.find(needle).map(|byte| line[..byte].chars().count())
        };
        let char_rpos = |line: &str, needle: &str| -> Option<usize> {
            line.rfind(needle).map(|byte| line[..byte].chars().count())
        };

        assert_eq!(
            char_pos(header, "ID"),
            char_pos(queued, "T001"),
            "{painted}"
        );
        assert_eq!(
            char_pos(header, "SUMMARY"),
            char_pos(queued, "synthetic queued inactive plan task"),
            "{painted}"
        );
        assert_eq!(char_pos(header, "MAP"), char_pos(queued, "◌"), "{painted}");
        assert_eq!(char_pos(header, "MAP"), char_pos(active, "●"), "{painted}");
        assert_eq!(
            char_pos(header, "REASON"),
            char_rpos(failed, "runner"),
            "{painted}"
        );

        assert!(painted.contains("◌ │ · · ·"), "{painted}");
        assert!(painted.contains("● │ ▣ □² ·"), "{painted}");
        assert!(painted.contains("▲"), "{painted}");
        assert!(painted.contains("TIER"), "{painted}");
        for prose_bag in [
            "workspace:none",
            "workspace:",
            "tier:",
            "lifecycle=",
            "active_step=",
        ] {
            assert!(
                !painted.contains(prose_bag),
                "task rows must keep raw/debug prose out of focused table ({prose_bag}):\n{painted}"
            );
        }
    }

    #[test]
    fn observation_projection_row_labels_suppress_broad_group_context_and_raw_schema() {
        let candidate = ObsRow {
            display_id: "L301".to_string(),
            status: "open".to_string(),
            priority: "high".to_string(),
            summary: "fresh signal".to_string(),
            ..Default::default()
        };
        let candidate_projection = observation_watch_projection(&candidate);
        let candidate_text = line_text(format_row_line_for_observation_projection(
            &Row::Obs(candidate),
            false,
            &candidate_projection,
        ));
        assert!(candidate_text.contains("next:triage"), "{candidate_text}");
        assert!(!candidate_text.contains("candidate"), "{candidate_text}");
        assert!(!candidate_text.contains("lifecycle="), "{candidate_text}");

        let contract = ObsRow {
            display_id: "L302".to_string(),
            status: "open".to_string(),
            priority: "normal".to_string(),
            summary: "contract summary".to_string(),
            contract_state: Some("draft".to_string()),
            tier_hint: Some("T2".to_string()),
            ..Default::default()
        };
        let contract_projection = observation_watch_projection(&contract);
        let contract_text = line_text(format_row_line_for_observation_projection(
            &Row::Obs(contract),
            false,
            &contract_projection,
        ));
        assert!(contract_text.contains("◈ draft"), "{contract_text}");
        assert!(
            contract_text.contains("next:approve/revise"),
            "{contract_text}"
        );
        assert!(!contract_text.contains("contract:"), "{contract_text}");
        assert!(!contract_text.contains("tier:"), "{contract_text}");

        let waiting = ObsRow {
            display_id: "L303".to_string(),
            status: "open".to_string(),
            priority: "normal".to_string(),
            summary: "waiting summary".to_string(),
            waiting_kind: Some("info_needed".to_string()),
            ..Default::default()
        };
        let waiting_projection = observation_watch_projection(&waiting);
        let waiting_text = line_text(format_row_line_for_observation_projection(
            &Row::Obs(waiting),
            false,
            &waiting_projection,
        ));
        assert!(waiting_text.contains("⋯ info needed"), "{waiting_text}");
        assert!(waiting_text.contains("next:answer info"), "{waiting_text}");
        assert!(!waiting_text.contains("waiting_kind="), "{waiting_text}");
    }

    #[test]
    fn task_line_prefers_live_runner_over_empty_claimed_by() {
        let row = Row::Task(TaskRow {
            display_id: "T149".to_string(),
            status: "planning".to_string(),
            title: "live planner".to_string(),
            updated_at: "2026-05-11T08:18:19Z".to_string(),
            live_run: Some(crate::tui::data::LiveRunSummary {
                role: "planner".to_string(),
                runner: Some("claude-code:opus".to_string()),
                status: Some("running".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let text = line_text(format_row_line(
            &row,
            false,
            &ExternalReviewState::default(),
        ));
        assert!(text.contains("runner:planner(claude-code:opus)"), "{text}");
        assert!(!text.contains("runner:none"), "{text}");
    }

    #[test]
    fn row_line_exposes_priority_tier_and_held_reason_snippets() {
        let obs = Row::Obs(ObsRow {
            display_id: "L100".to_string(),
            status: "open".to_string(),
            priority: "high".to_string(),
            summary: "priority obs".to_string(),
            tier_hint: Some("T2".to_string()),
            ..Default::default()
        });
        let intake = Row::Intake(IntakeRow {
            display_id: "I100".to_string(),
            status: "needs_info".to_string(),
            priority: Some("high".to_string()),
            summary: "held intake".to_string(),
            held_reason: Some("missing owner".to_string()),
            ..Default::default()
        });
        let obs_text = line_text(format_row_line(
            &obs,
            false,
            &ExternalReviewState::default(),
        ));
        let intake_text = line_text(format_row_line(
            &intake,
            false,
            &ExternalReviewState::default(),
        ));
        assert!(obs_text.contains("high/T2"), "{obs_text}");
        assert!(obs_text.contains("next:triage"), "{obs_text}");
        assert!(intake_text.contains("priority:high"), "{intake_text}");
        assert!(intake_text.contains("held:missing owner"), "{intake_text}");
    }

    #[test]
    fn collapsed_observation_row_renders_summary_count_badge_and_primary_id() {
        let row = Row::CollapsedObs(crate::tui::data::CollapsedObsRow {
            section: crate::tui::data::Section::ObsOther,
            summary: "dupe cluster summary".to_string(),
            count: 76,
            primary_display_id: "L000".to_string(),
            display_ids: (0..76).map(|i| format!("L{:03}", i)).collect(),
            representative: ObsRow {
                display_id: "L000".to_string(),
                status: "open".to_string(),
                priority: "normal".to_string(),
                summary: "dupe cluster summary".to_string(),
                ..Default::default()
            },
        });
        let text = line_text(format_row_line(
            &row,
            false,
            &ExternalReviewState::default(),
        ));
        assert!(text.contains("dupe cluster summary"), "{text}");
        assert!(text.contains("×76"), "{text}");
        assert!(text.contains("L000"), "{text}");
    }

    #[test]
    fn investigation_failed_observation_row_renders_reason() {
        let row = Row::Obs(ObsRow {
            display_id: "L065".to_string(),
            status: "investigation_failed".to_string(),
            priority: "high".to_string(),
            summary: "investigator failed".to_string(),
            investigation_failure_reason: Some("rate_limit: reset later".to_string()),
            ..Default::default()
        });
        let text = line_text(format_row_line(
            &row,
            false,
            &ExternalReviewState::default(),
        ));
        assert!(text.contains("investigation-failed"), "{text}");
        assert!(text.contains("investigator failed"), "{text}");
    }

    #[test]
    fn system_alert_item_renders_after_cockpit_header_for_dead_daemon_with_locks() {
        let mut app = App::new(TuiOpts::default());
        app.status_bar = StatusBar {
            daemon_liveness: Liveness::Dead,
            ..Default::default()
        };
        app.system_health = SystemHealth {
            unfinished_dispatch_locks: 8,
            oldest_claimed_at_epoch: Some(now_epoch() - (3 * 3600 + 10)),
        };
        let mut items = cockpit_header_items(&app);
        if let Some(alert) = system_alert_item(&app) {
            items.push(alert);
        }
        assert_eq!(items.len(), 5);

        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal.draw(|f| draw(f, &mut app)).expect("draw");
        let buf = terminal.backend().buffer().clone();
        let mut painted = String::new();
        let mut alert_y = None;
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            if line.contains("system-alert:") {
                alert_y = Some(y);
            }
            painted.push_str(&line);
            painted.push('\n');
        }
        assert!(
            painted.contains("system-alert: daemon DEAD; 8 dangling locks; oldest started 3h ago"),
            "{painted}"
        );
        let y = alert_y.expect("alert row painted");
        let first_cell = &buf[(0, y)];
        assert_eq!(first_cell.fg, Color::Red);
        assert!(first_cell.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn system_alert_item_absent_for_live_daemon_or_zero_locks() {
        let mut app = App::new(TuiOpts::default());
        app.status_bar.daemon_liveness = Liveness::Live { pid: 1 };
        app.system_health = SystemHealth {
            unfinished_dispatch_locks: 8,
            oldest_claimed_at_epoch: Some(now_epoch() - 3 * 3600),
        };
        assert!(system_alert_item(&app).is_none());
        assert!(!painted_buffer(&mut app).contains("system-alert:"));

        let mut dead_zero = App::new(TuiOpts::default());
        dead_zero.status_bar.daemon_liveness = Liveness::Dead;
        dead_zero.system_health = SystemHealth::default();
        assert!(system_alert_item(&dead_zero).is_none());
        assert!(!painted_buffer(&mut dead_zero).contains("system-alert:"));
    }

    #[test]
    fn system_alert_item_renders_with_placeholder_age_when_oldest_claimed_at_is_null() {
        // When daemon is DEAD + unfinished locks >= 1 BUT claimed_at is NULL
        // (oldest_claimed_at_epoch is None), the alert MUST still render with "?h"
        // placeholder instead of being suppressed.
        let mut app = App::new(TuiOpts::default());
        app.status_bar = StatusBar {
            daemon_liveness: Liveness::Dead,
            ..Default::default()
        };
        app.system_health = SystemHealth {
            unfinished_dispatch_locks: 3,
            oldest_claimed_at_epoch: None,
        };
        let alert = system_alert_item(&app);
        assert!(
            alert.is_some(),
            "alert must render even when claimed_at is NULL"
        );
        let painted = painted_buffer(&mut app);
        assert!(
            painted.contains("system-alert: daemon DEAD; 3 dangling locks; oldest started ?h ago"),
            "expected '?h' placeholder in alert: {painted}"
        );
    }

    #[test]
    fn mission_compact_window_preserves_populated_default_sections() {
        use crate::tui::data::{CollapsedObsRow, Section, StoreLane};

        let mut app = App::new(TuiOpts::default());
        app.status_bar = StatusBar {
            daemon_liveness: Liveness::Dead,
            ..Default::default()
        };
        app.system_health = SystemHealth {
            unfinished_dispatch_locks: 1,
            oldest_claimed_at_epoch: None,
        };

        let mut rows: Vec<Row> = Vec::new();
        let mut sections: Vec<(Section, Vec<usize>)> = Vec::new();
        for sec in Section::ALL {
            let abs = rows.len();
            if sec == Section::ObsOther {
                rows.push(Row::CollapsedObs(CollapsedObsRow {
                    section: Section::ObsOther,
                    summary: "dupe cluster".to_string(),
                    count: 5,
                    primary_display_id: "L100".to_string(),
                    display_ids: vec!["L100".to_string(), "L101".to_string()],
                    representative: ObsRow {
                        display_id: "L100".to_string(),
                        status: "open".to_string(),
                        priority: "normal".to_string(),
                        summary: "dupe cluster".to_string(),
                        ..Default::default()
                    },
                }));
            } else {
                rows.push(Row::Task(TaskRow {
                    display_id: format!("T{:03}", abs),
                    status: "executing".to_string(),
                    title: format!("synthetic for {:?}", sec),
                    ..Default::default()
                }));
            }
            sections.push((sec, vec![abs]));
        }
        app.rows = rows;
        app.sections = sections;

        assert!(
            mission_compact_mode(&app),
            "preconditions for compact mode (DEAD + dangling lock + collapsed obs) should hold"
        );

        // Cockpit lanes filter App::flat_rows by focused_store; collect the
        // compact-window output across every lane to validate the union of
        // populated sections.
        let mut window: Vec<FlatRow> = Vec::new();
        for lane in StoreLane::ALL {
            app.focused_store = lane;
            let flat = app.flat_rows();
            window.extend(mission_compact_window(&app, &flat));
        }

        let section_of = |fr: &FlatRow| app.sections[fr.section].0;
        let present: std::collections::HashSet<Section> = window.iter().map(section_of).collect();

        let must_contain = [
            Section::TasksActionableCurrentWork,
            Section::ObsRatifiable,
            Section::TasksAcceptU3,
            Section::TasksIntegration,
            Section::TasksIntegratedAwaitingPostLand,
            Section::TasksIntegrationBlocked,
            Section::TasksBlockedNeedsAction,
            Section::TasksDeployRecovery,
            Section::TasksNeedsTriage,
            Section::IntakeHeld,
            Section::TasksHeldAiReview,
            Section::TasksHeldZombie,
            Section::ObsOpenNoContract,
            Section::IntakeOpen,
            Section::IntakeRouted,
            Section::ExternalReviewLane,
        ];
        for sec in must_contain {
            assert!(
                present.contains(&sec),
                "compact window missing populated default section {:?}; present={:?}",
                sec,
                present
            );
        }
        assert!(
            window
                .iter()
                .any(|fr| matches!(app.rows.get(fr.abs), Some(Row::CollapsedObs(_)))),
            "compact window must still include collapsed obs rows"
        );
        assert!(
            !present.contains(&Section::TasksRecentlyTerminal),
            "compact window must not surface TERMINAL historical noise"
        );
    }

    #[test]
    fn render_frame_emits_dedicated_headers_for_integration_lane_states() {
        // AC4.3: With one row in each new state, draw the cockpit and assert
        // each row appears under its dedicated section header — never folded
        // into ACTIVE WORK.
        use crate::tui::data::{classify, Row, Section, TaskRow};

        let mut app = App::new(TuiOpts::default());
        app.rows = vec![
            Row::Task(TaskRow {
                display_id: "T800".to_string(),
                status: "integration_queued".to_string(),
                title: "queued candidate".to_string(),
                tier_hint: Some("T2".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                display_id: "T801".to_string(),
                status: "integrating".to_string(),
                title: "currently integrating".to_string(),
                tier_hint: Some("T2".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                display_id: "T802".to_string(),
                status: "integrated".to_string(),
                title: "awaiting post-land".to_string(),
                tier_hint: Some("T2".to_string()),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                display_id: "T803".to_string(),
                status: "integration_blocked".to_string(),
                title: "stale base".to_string(),
                tier_hint: Some("T2".to_string()),
                blocked_reason: Some("stale_base".to_string()),
                blocked_reason_class: Some("stale".to_string()),
                ..Default::default()
            }),
        ];
        app.sections = classify(&app.rows);
        app.apply_sort();

        // Sanity: classifier put each row in the right section.
        let sec_idx =
            |sec: Section| -> usize { app.sections.iter().position(|(s, _)| *s == sec).unwrap() };
        assert_eq!(app.sections[sec_idx(Section::TasksIntegration)].1.len(), 2);
        assert_eq!(
            app.sections[sec_idx(Section::TasksIntegratedAwaitingPostLand)]
                .1
                .len(),
            1
        );
        assert_eq!(
            app.sections[sec_idx(Section::TasksIntegrationBlocked)]
                .1
                .len(),
            1
        );
        assert!(
            app.sections[sec_idx(Section::TasksActionableCurrentWork)]
                .1
                .is_empty(),
            "integration states must not appear in ACTIVE WORK"
        );

        let painted = painted_buffer(&mut app);

        for label in ["GATE (3)", "FAILED (1)"] {
            assert!(
                painted.contains(label),
                "expected projection header '{label}' in painted frame:\n{painted}"
            );
        }
        for legacy in ["INTEGRATION (2)", "INTEGRATED (1)", "HELD-INTEGRATION (1)"] {
            assert!(
                !painted.contains(legacy),
                "task-focused table must not render legacy section header '{legacy}':\n{painted}"
            );
        }
        for id in ["T800", "T801", "T802", "T803"] {
            assert!(
                painted.contains(id),
                "expected row id {id} painted:\n{painted}"
            );
        }
        // Projection groups appear in canonical order: GATE before FAILED.
        let pos_gate = painted.find("GATE (3)").unwrap();
        let pos_failed = painted.find("FAILED (1)").unwrap();
        assert!(pos_gate < pos_failed);
    }

    #[test]
    fn observation_projection_headers_use_projection_buckets_and_collapsed_counts() {
        use crate::tui::data::{CollapsedObsRow, Section};

        let mut app = App::new(TuiOpts::default());
        app.focused_store = StoreLane::Observations;
        app.rows = vec![
            Row::Obs(ObsRow {
                display_id: "L401".to_string(),
                status: "open".to_string(),
                priority: "normal".to_string(),
                summary: "front".to_string(),
                ..Default::default()
            }),
            Row::Obs(ObsRow {
                display_id: "L402".to_string(),
                status: "open".to_string(),
                lifecycle: Some("investigating".to_string()),
                priority: "normal".to_string(),
                summary: "work".to_string(),
                ..Default::default()
            }),
            Row::Obs(ObsRow {
                display_id: "L403".to_string(),
                status: "open".to_string(),
                priority: "normal".to_string(),
                summary: "gate".to_string(),
                contract_state: Some("draft".to_string()),
                ..Default::default()
            }),
            Row::Obs(ObsRow {
                display_id: "L404".to_string(),
                status: "open".to_string(),
                priority: "normal".to_string(),
                summary: "wait".to_string(),
                waiting_kind: Some("info_needed".to_string()),
                ..Default::default()
            }),
            Row::Obs(ObsRow {
                display_id: "L405".to_string(),
                status: "investigation_failed".to_string(),
                priority: "normal".to_string(),
                summary: "fault".to_string(),
                investigation_failure_reason: Some("runner fault".to_string()),
                ..Default::default()
            }),
            Row::CollapsedObs(CollapsedObsRow {
                section: Section::ObsOther,
                summary: "approved cluster".to_string(),
                count: 4,
                primary_display_id: "L406".to_string(),
                display_ids: (0..4).map(|i| format!("L40{i}")).collect(),
                representative: ObsRow {
                    display_id: "L406".to_string(),
                    status: "open".to_string(),
                    priority: "normal".to_string(),
                    summary: "approved cluster".to_string(),
                    contract_state: Some("approved".to_string()),
                    ..Default::default()
                },
            }),
        ];
        app.sections = vec![(Section::ObsOther, (0..app.rows.len()).collect())];
        app.viewport_height = 20;

        let model = store_flow_model(
            &app.rows,
            &SystemHealth::default(),
            &Liveness::Dead,
            &ExternalReviewState::default(),
        );
        assert_eq!(model.observations.candidate, 1);
        assert_eq!(model.observations.in_progress, 1);
        assert_eq!(model.observations.ready, 5);
        assert_eq!(model.observations.waiting_kinds.values().sum::<usize>(), 1);
        assert_eq!(model.observations.errors, 1);

        let backend = TestBackend::new(140, 24);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal
            .draw(|f| draw_focused_table(f, &app, f.area()))
            .expect("draw focused table");
        let painted = buffer_to_string(terminal.backend().buffer());

        for label in [
            "CANDIDATES (1)",
            "INVESTIGATE (1)",
            "CONTRACT GATE (5)",
            "WAITING (1)",
            "ERRORS (1)",
        ] {
            assert!(painted.contains(label), "missing {label}:\n{painted}");
        }
        assert!(
            painted.contains("×4"),
            "collapsed badge missing:\n{painted}"
        );
        assert!(!painted.contains("contract:"), "{painted}");
        assert!(!painted.contains("waiting_kind="), "{painted}");
    }

    #[test]
    fn task_projection_headers_count_full_filtered_rows_not_visible_window() {
        let mut app = App::new(TuiOpts::default());
        app.rows = (0..8)
            .map(|i| {
                Row::Task(TaskRow {
                    display_id: format!("T90{i}"),
                    status: "executing".to_string(),
                    title: format!("viewport hidden work {i}"),
                    lifecycle: Some("active".to_string()),
                    active_step: Some("coding".to_string()),
                    ..Default::default()
                })
            })
            .collect();
        app.sections = classify(&app.rows);
        app.apply_sort();
        app.viewport_height = 3;
        app.scroll_offset = 2;
        app.selection = crate::tui::app::Selection { section: 0, row: 2 };

        let flat = app.flat_rows();
        assert_eq!(flat.len(), 8, "fixture full focused task row set");
        assert_eq!(
            visible_window(&app, &flat).len(),
            3,
            "fixture viewport slice"
        );

        let backend = TestBackend::new(120, 12);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal
            .draw(|f| draw_focused_table(f, &app, f.area()))
            .expect("draw focused table");
        let painted = buffer_to_string(terminal.backend().buffer());

        assert!(
            painted.contains("WORKING (8)"),
            "projection header must count the full focused task set, not the viewport:\n{painted}"
        );
        assert!(!painted.contains("WORKING (3)"), "{painted}");
        for id in ["T902", "T903", "T904"] {
            assert!(painted.contains(id), "visible row {id} missing:\n{painted}");
        }
        for id in ["T900", "T901", "T905", "T906", "T907"] {
            assert!(
                !painted.contains(id),
                "hidden row {id} rendered:\n{painted}"
            );
        }
    }

    #[test]
    fn virtual_scroll() {
        let mut app = App::new(TuiOpts::default());
        app.rows = synthetic_rows(200);
        app.sections = crate::tui::data::classify(&app.rows);
        app.apply_sort();
        app.viewport_height = 10;
        app.selection = crate::tui::app::Selection {
            section: 0, /* TasksActionableCurrentWork */
            row: 0,
        };

        let flat = app.flat_rows();
        assert_eq!(flat.len(), 200);

        let window = visible_window(&app, &flat);
        assert_eq!(
            window.len(),
            10,
            "only viewport-height widgets should be emitted"
        );

        // PgDn moves selection by viewport_height and the scroll offset
        // tracks it (selection always lands within the visible window).
        crate::tui::input::on_key(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::PageDown,
                crossterm::event::KeyModifiers::NONE,
            ),
        );
        let cursor = app.current_flat().expect("cursor in flat list");
        assert!(
            cursor >= app.scroll_offset && cursor < app.scroll_offset + app.viewport_height,
            "selection (idx={cursor}) must stay inside viewport window \
             [{}, {}) after PgDn",
            app.scroll_offset,
            app.scroll_offset + app.viewport_height,
        );
    }

    // ----- Phase 3 cockpit-render tests -------------------------------------

    use crate::tui::data::{classify, StoreLane};
    use ratatui::buffer::Buffer;

    /// Build an app populated across all five lanes (intake / obs / tasks /
    /// reviews + system_health for engine), with one terminal task so the
    /// recent-exhaust strip has content.
    fn cockpit_fixture_app() -> App {
        let mut app = App::new(TuiOpts::default());
        app.rows = vec![
            Row::Intake(IntakeRow {
                display_id: "I001".to_string(),
                status: "draft".to_string(),
                summary: "intake row".to_string(),
                ..Default::default()
            }),
            Row::Obs(ObsRow {
                display_id: "L001".to_string(),
                status: "open".to_string(),
                priority: "normal".to_string(),
                summary: "obs row".to_string(),
                ..Default::default()
            }),
            Row::Task(TaskRow {
                display_id: "T100".to_string(),
                status: "executing".to_string(),
                title: "active task".to_string(),
                ..Default::default()
            }),
            // Terminal task — must appear in recent-exhaust, NOT in tasks lane main rows.
            Row::Task(TaskRow {
                display_id: "T200".to_string(),
                status: "accepted".to_string(),
                title: "done task".to_string(),
                updated_at: "2026-05-09".to_string(),
                ..Default::default()
            }),
            Row::Review(ReviewRow {
                display_id: "E001".to_string(),
                task_id: "T100".to_string(),
                status: "running".to_string(),
                runner: "codex".to_string(),
                ..Default::default()
            }),
        ];
        app.sections = classify(&app.rows);
        app.status_bar = StatusBar {
            daemon_liveness: Liveness::Live { pid: 4242 },
            ..Default::default()
        };
        app.system_health = SystemHealth {
            unfinished_dispatch_locks: 3,
            oldest_claimed_at_epoch: Some(now_epoch() - 7200),
        };
        app.apply_sort();
        app.viewport_height = 10;
        app
    }

    fn paint(app: &mut App, w: u16, h: u16) -> Buffer {
        let backend = TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).expect("test backend");
        terminal.draw(|f| draw(f, app)).expect("draw");
        terminal.backend().buffer().clone()
    }

    fn buffer_to_string(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// Locate the column where a card's left border sits by scanning row 0
    /// (top strip top border) for `┌`. Returns one column per card found.
    fn card_left_borders(buf: &Buffer) -> Vec<u16> {
        let mut cols = Vec::new();
        for x in 0..buf.area.width {
            if buf[(x, 0)].symbol() == "┌" {
                cols.push(x);
            }
        }
        cols
    }

    /// AC3.1(a) / AC3.2: top strip paints all five lane labels once each card
    /// is wide enough for its 3-column grid to preserve semantic label words.
    #[test]
    fn cockpit_top_strip_paints_all_five_lane_labels_at_readable_width() {
        let mut app = cockpit_fixture_app();
        let buf = paint(&mut app, 140, 30);
        let painted = buffer_to_string(&buf);
        // Top region is the card strip.
        let top_region: String = painted
            .lines()
            .take(TOP_STRIP_HEIGHT as usize)
            .collect::<Vec<_>>()
            .join("\n");
        for label in ["INTAKE", "OBSERVATIONS", "TASKS", "EXTERNAL", "ENGINE"] {
            assert!(
                top_region.contains(label),
                "missing top-strip label {label:?} in top region:\n{top_region}\n\nfull buffer:\n{painted}"
            );
        }
        for label in [
            "investigate",
            "contract",
            "gate",
            "working",
            "waiting",
            "tool",
            "fault",
        ] {
            assert!(
                top_region.contains(label),
                "missing readable slot word {label:?} in top region:\n{top_region}"
            );
        }
        assert!(
            !top_region.contains("+ more"),
            "wide readable strip must not replace slot labels with + more:\n{top_region}"
        );
    }

    #[test]
    fn cockpit_top_strip_120_width_uses_focused_fallback_without_slot_more_labels() {
        let mut app = cockpit_fixture_app();
        let buf = paint(&mut app, 120, 30);
        let painted = buffer_to_string(&buf);
        let top_region: String = painted
            .lines()
            .take(TOP_STRIP_HEIGHT as usize)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            top_region.contains("+4 more"),
            "120-column strip must collapse hidden lanes behind +4 more until card labels are readable:\n{top_region}"
        );
        for label in ["queued", "working", "gate", "done", "waiting", "failed"] {
            assert!(
                top_region.contains(label),
                "focused card label {label:?} must remain readable at 120 columns:\n{top_region}"
            );
        }
        assert_eq!(
            top_region.matches("+ more").count(),
            1,
            "only the more-affordance body may say + more; slot labels must stay semantic:\n{top_region}"
        );
    }

    #[test]
    fn cockpit_top_strip_narrow_width_uses_more_fallback_without_label_fragments() {
        let mut app = cockpit_fixture_app();
        let buf = paint(&mut app, 80, 30);
        let painted = buffer_to_string(&buf);
        let top_region: String = painted
            .lines()
            .take(TOP_STRIP_HEIGHT as usize)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            top_region.contains("+4 more") || top_region.contains("+ more"),
            "narrow strip must expose hidden lanes via + more affordance:\n{top_region}"
        );
        for label in ["queued", "working", "gate", "done", "waiting", "failed"] {
            assert!(
                top_region.contains(label),
                "focused card label {label:?} must remain readable under fallback:\n{top_region}"
            );
        }
        for fragment in [
            "cand", "inv", "contr", "wrk", "dn", "err", "◌q", "◆wrk", "✓dn", "△w", "▲err",
        ] {
            assert!(
                !top_region.contains(fragment),
                "narrow fallback must not paint cockpit-code/word fragment {fragment:?}:\n{top_region}"
            );
        }
    }

    /// AC3.1(b): focused-lane card border is cyan; unfocused are dim.
    #[test]
    fn cockpit_focused_card_has_distinct_border_style() {
        let mut app = cockpit_fixture_app();
        // Default focus = Tasks (index 2 in StoreLane::ALL).
        let buf = paint(&mut app, 200, 30);
        let cols = card_left_borders(&buf);
        assert_eq!(cols.len(), 5, "expected 5 cards, found cols {:?}", cols);
        let focused_col = cols[2];
        let other_col = cols[0];
        let focused_cell = &buf[(focused_col, 0)];
        let other_cell = &buf[(other_col, 0)];
        assert_eq!(
            focused_cell.fg,
            Color::Cyan,
            "focused card border must be cyan, got {:?}",
            focused_cell.fg
        );
        assert_ne!(
            other_cell.fg,
            Color::Cyan,
            "non-focused card border must NOT be cyan, got {:?}",
            other_cell.fg
        );
    }

    /// AC3.1(c) / AC3.3: terminal task ids appear in the recent-exhaust strip
    /// and NOT in the focused-table region (which hides terminal history).
    #[test]
    fn cockpit_recent_exhaust_strip_shows_terminal_ids_not_in_main_rows() {
        let mut app = cockpit_fixture_app();
        let buf = paint(&mut app, 120, 30);
        let painted = buffer_to_string(&buf);
        let lines: Vec<&str> = painted.lines().collect();
        // Last lines: status bar (h-1), hint (h-2), exhaust (h-3) (no search bar).
        let exhaust_line = lines[lines.len() - 3];
        assert!(
            exhaust_line.contains("T200"),
            "exhaust strip must include terminal task id T200; got: {exhaust_line}"
        );
        assert!(
            exhaust_line.contains("■ done"),
            "exhaust strip must show terminal status; got: {exhaust_line}"
        );
        // Focused-table region (between top strip and exhaust).
        let middle: String = lines[TOP_STRIP_HEIGHT as usize..lines.len() - 3].join("\n");
        assert!(
            !middle.contains("T200"),
            "terminal task id T200 must NOT appear in focused-table region:\n{middle}"
        );
    }

    /// AC3.1(c)/AC3.3 placeholder branch: empty exhaust shows the placeholder.
    #[test]
    fn cockpit_recent_exhaust_strip_placeholder_when_no_terminal_rows() {
        let mut app = cockpit_fixture_app();
        // Drop the terminal task.
        app.rows
            .retain(|r| !matches!(r, Row::Task(t) if t.status == "accepted"));
        app.sections = classify(&app.rows);
        app.apply_sort();
        let buf = paint(&mut app, 120, 30);
        let painted = buffer_to_string(&buf);
        let lines: Vec<&str> = painted.lines().collect();
        let exhaust_line = lines[lines.len() - 3];
        assert!(
            exhaust_line.contains("— no recent exhaust —"),
            "expected exhaust placeholder; got: {exhaust_line}"
        );
    }

    /// AC3.1(d): EngineHealth focus paints the engine panel — daemon status
    /// text and lock counts in the focused-table region.
    #[test]
    fn cockpit_engine_focus_paints_daemon_status_and_lock_counts() {
        let mut app = cockpit_fixture_app();
        // DEAD daemon to surface the system-alert variant of the engine panel.
        app.status_bar.daemon_liveness = Liveness::Dead;
        app.focused_store = StoreLane::EngineHealth;
        app.selection = crate::tui::app::Selection::default();
        let buf = paint(&mut app, 120, 30);
        let painted = buffer_to_string(&buf);
        let lines: Vec<&str> = painted.lines().collect();
        let middle: String = lines[TOP_STRIP_HEIGHT as usize..lines.len() - 3].join("\n");
        assert!(
            middle.contains("daemon: DEAD"),
            "engine panel must show daemon status; got middle:\n{middle}"
        );
        assert!(
            middle.contains("unfinished_locks: 3"),
            "engine panel must show lock count; got middle:\n{middle}"
        );
        // Side detail pane now renders engine detail (T141 P2): the
        // EngineHealth focus branch calls super::detail::engine_lines.
        assert!(
            middle.contains("Engine detail"),
            "engine focus must paint Engine detail header in side pane; got:\n{middle}"
        );
    }

    /// AC3.1(e): Right-key (l) moves the focused card to the next lane and
    /// the painted cyan border follows.
    #[test]
    fn cockpit_right_key_changes_visibly_highlighted_lane() {
        let mut app = cockpit_fixture_app();
        // Default focus is Tasks (col index 2). Cyan border at cols[2] only.
        let buf = paint(&mut app, 200, 30);
        let cols = card_left_borders(&buf);
        assert_eq!(buf[(cols[2], 0)].fg, Color::Cyan);

        // Press 'l' (right) → focus advances to ExternalReviews (cols[3]).
        crate::tui::input::on_key(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('l'),
                crossterm::event::KeyModifiers::NONE,
            ),
        );
        assert_eq!(app.focused_store, StoreLane::ExternalReviews);

        let buf2 = paint(&mut app, 200, 30);
        let cols2 = card_left_borders(&buf2);
        assert_eq!(
            buf2[(cols2[3], 0)].fg,
            Color::Cyan,
            "focus should follow to col 3"
        );
        assert_ne!(
            buf2[(cols2[2], 0)].fg,
            Color::Cyan,
            "old focus at col 2 must lose cyan border"
        );
    }

    /// Phase 4: per-row renderers use semantic labels for upstream/review stores.
    #[test]
    fn format_obs_line_surfaces_one_state_next_action_and_hides_raw_contract() {
        let row = Row::Obs(ObsRow {
            display_id: "L200".to_string(),
            status: "open".to_string(),
            priority: "normal".to_string(),
            lifecycle: Some("ready".to_string()),
            summary: "obs with contract".to_string(),
            tier_hint: Some("T2".to_string()),
            contract_state: Some("ready".to_string()),
            task_id: Some("T555".to_string()),
            ..Default::default()
        });
        let text = line_text(format_row_line(
            &row,
            false,
            &ExternalReviewState::default(),
        ));
        assert!(text.contains("▰ contract-approved"), "{text}");
        assert!(text.contains("normal/T2"), "{text}");
        assert!(text.contains("next:promote/resolve"), "{text}");
        assert!(text.contains("linked:T555"), "{text}");
        assert!(!text.contains("contract:"), "{text}");
        assert!(!text.contains("tier:"), "{text}");

        // None/empty tier/task_id are simply omitted — keeps the
        // 80-col cockpit budget within reach for the narrow live-realistic
        // snapshot.
        let bare = Row::Obs(ObsRow {
            display_id: "L201".to_string(),
            status: "open".to_string(),
            priority: "normal".to_string(),
            summary: "bare obs".to_string(),
            ..Default::default()
        });
        let bare_text = line_text(format_row_line(
            &bare,
            false,
            &ExternalReviewState::default(),
        ));
        assert!(!bare_text.contains("tier:"), "{bare_text}");
        assert!(!bare_text.contains("contract:"), "{bare_text}");
        assert!(!bare_text.contains("linked:"), "{bare_text}");
    }

    #[test]
    fn format_intake_line_surfaces_semantic_label_source_and_cluster() {
        let row = Row::Intake(IntakeRow {
            display_id: "I200".to_string(),
            status: "draft".to_string(),
            summary: "intake row".to_string(),
            source_agent: Some("executor".to_string()),
            cluster_key: Some("watch-ux".to_string()),
            captured_at: Some("2026-05-09T00:00:00Z".to_string()),
            ..Default::default()
        });
        let text = line_text(format_row_line(
            &row,
            false,
            &ExternalReviewState::default(),
        ));
        assert!(text.contains("◌ new"), "{text}");
        assert!(text.contains("source:executor"), "{text}");
        assert!(text.contains("cluster:watch-ux"), "{text}");
        assert!(text.contains("age:"), "{text}");
    }

    #[test]
    fn format_review_line_surfaces_semantic_label_verdict_and_attempts() {
        let row = Row::Review(ReviewRow {
            display_id: "E200".to_string(),
            task_id: "T100".to_string(),
            status: "running".to_string(),
            runner: "codex".to_string(),
            verdict: Some("PASS".to_string()),
            attempts: 2,
            base_sha: Some("abcdef0123456789".to_string()),
            started_at: Some("2026-05-09T00:00:00Z".to_string()),
            ..Default::default()
        });
        let text = line_text(format_row_line(
            &row,
            false,
            &ExternalReviewState::default(),
        ));
        assert!(text.contains("◆ running"), "{text}");
        assert!(text.contains("verdict:PASS"), "{text}");
        assert!(text.contains("attempts:2"), "{text}");
        assert!(text.contains("sha:abcdef0"), "{text}");
    }

    #[test]
    fn upstream_and_review_rows_cover_semantic_label_cases() {
        let obs_cases = [
            (
                ObsRow {
                    lifecycle: Some("candidate".to_string()),
                    ..Default::default()
                },
                "candidate",
            ),
            (
                ObsRow {
                    waiting_kind: Some("info_needed".to_string()),
                    ..Default::default()
                },
                "needs-info",
            ),
            (
                ObsRow {
                    contract_state: Some("draft".to_string()),
                    ..Default::default()
                },
                "contract-draft",
            ),
            (
                ObsRow {
                    contract_state: Some("approved".to_string()),
                    ..Default::default()
                },
                "contract-approved",
            ),
            (
                ObsRow {
                    pending_architecture_review: Some(true),
                    ..Default::default()
                },
                "arch-gate",
            ),
            (
                ObsRow {
                    lifecycle: Some("in_progress".to_string()),
                    ..Default::default()
                },
                "resolving",
            ),
            (
                ObsRow {
                    lifecycle: Some("closed".to_string()),
                    ..Default::default()
                },
                "addressed",
            ),
            (
                ObsRow {
                    lifecycle: Some("closed".to_string()),
                    outcome: Some("wont_fix".to_string()),
                    ..Default::default()
                },
                "wont-fix",
            ),
            (
                ObsRow {
                    superseded_by_id: Some("L001".to_string()),
                    ..Default::default()
                },
                "superseded",
            ),
            (
                ObsRow {
                    lifecycle: Some("ready".to_string()),
                    ..Default::default()
                },
                "investigate",
            ),
        ];
        for (idx, (mut obs, label)) in obs_cases.into_iter().enumerate() {
            obs.display_id = format!("L{idx:03}");
            obs.priority = "normal".to_string();
            let text = line_text(format_row_line(
                &Row::Obs(obs),
                false,
                &ExternalReviewState::default(),
            ));
            assert!(text.contains(label), "{label}: {text}");
        }

        let intake_cases = [
            (
                IntakeRow {
                    lifecycle: Some("new".to_string()),
                    ..Default::default()
                },
                "new",
            ),
            (
                IntakeRow {
                    lifecycle: Some("triaging".to_string()),
                    ..Default::default()
                },
                "triage",
            ),
            (
                IntakeRow {
                    lifecycle: Some("waiting".to_string()),
                    ..Default::default()
                },
                "needs-info",
            ),
            (
                IntakeRow {
                    lifecycle: Some("closed".to_string()),
                    outcome: Some("routed_to_observation".to_string()),
                    ..Default::default()
                },
                "routed",
            ),
            (
                IntakeRow {
                    lifecycle: Some("closed".to_string()),
                    outcome: Some("marked_duplicate".to_string()),
                    ..Default::default()
                },
                "duplicate",
            ),
            (
                IntakeRow {
                    lifecycle: Some("closed".to_string()),
                    outcome: Some("dropped_as_noise".to_string()),
                    ..Default::default()
                },
                "dropped",
            ),
            (
                IntakeRow {
                    lifecycle: Some("closed".to_string()),
                    outcome: Some("escalated_to_architecture_review".to_string()),
                    ..Default::default()
                },
                "arch-review",
            ),
        ];
        for (idx, (mut intake, label)) in intake_cases.into_iter().enumerate() {
            intake.display_id = format!("I{idx:03}");
            let text = line_text(format_row_line(
                &Row::Intake(intake),
                false,
                &ExternalReviewState::default(),
            ));
            assert!(text.contains(label), "{label}: {text}");
        }

        let review_cases = [
            (
                ReviewRow {
                    status: "pending".to_string(),
                    ..Default::default()
                },
                "pending",
            ),
            (
                ReviewRow {
                    status: "running".to_string(),
                    ..Default::default()
                },
                "running",
            ),
            (
                ReviewRow {
                    status: "passed".to_string(),
                    ..Default::default()
                },
                "passed",
            ),
            (
                ReviewRow {
                    status: "revise".to_string(),
                    ..Default::default()
                },
                "revise",
            ),
            (
                ReviewRow {
                    status: "tooling_held".to_string(),
                    ..Default::default()
                },
                "tool-fault",
            ),
            (
                ReviewRow {
                    status: "superseded".to_string(),
                    ..Default::default()
                },
                "superseded",
            ),
        ];
        for (idx, (mut review, label)) in review_cases.into_iter().enumerate() {
            review.display_id = format!("E{idx:03}");
            review.task_id = "T001".to_string();
            let text = line_text(format_row_line(
                &Row::Review(review),
                false,
                &ExternalReviewState::default(),
            ));
            assert!(text.contains(label), "{label}: {text}");
        }
    }

    #[test]
    fn external_review_default_row_hides_null_none_clutter() {
        let row = Row::Review(ReviewRow {
            display_id: "E999".to_string(),
            task_id: "T999".to_string(),
            status: "pending".to_string(),
            runner: "unknown".to_string(),
            held_reason: Some("none".to_string()),
            next_retry_at: Some("none".to_string()),
            ..Default::default()
        });
        let text = line_text(format_row_line(
            &row,
            false,
            &ExternalReviewState::default(),
        ));
        assert!(!text.contains("runner=unknown"), "{text}");
        assert!(!text.contains("held_reason=none"), "{text}");
        assert!(!text.contains("held:none"), "{text}");
        assert!(!text.contains("next_retry_at=none"), "{text}");
        assert!(!text.contains("liveness=pending"), "{text}");
        assert!(text.contains("runner=—"), "{text}");
    }

    #[test]
    fn engine_panel_renders_recent_daemon_restart_when_present() {
        use crate::tui::data::DaemonStartRow;
        let mut app = cockpit_fixture_app();
        app.focused_store = StoreLane::EngineHealth;
        app.engine_detail.recent_daemon_starts = vec![DaemonStartRow {
            pid: 4242,
            started_at: Some("2026-05-09T12:34:56Z".to_string()),
            ..Default::default()
        }];
        let buf = paint(&mut app, 120, 30);
        let painted = buffer_to_string(&buf);
        assert!(
            painted.contains("recent_restart:"),
            "engine panel must include recent_restart line:\n{painted}"
        );
        assert!(
            painted.contains("pid="),
            "engine panel restart line must include pid=:\n{painted}"
        );

        // Empty recent_daemon_starts → no restart line, no panic.
        let mut empty_app = cockpit_fixture_app();
        empty_app.focused_store = StoreLane::EngineHealth;
        let buf2 = paint(&mut empty_app, 120, 30);
        let painted2 = buffer_to_string(&buf2);
        assert!(
            !painted2.contains("recent_restart:"),
            "engine panel must NOT include recent_restart line when empty:\n{painted2}"
        );
    }

    /// AC3.6 invariant restated as a render-layer assertion: with focused
    /// store = Tasks, terminal task rows do not appear in the main-row region.
    #[test]
    fn cockpit_tasks_lane_excludes_terminal_rows_from_main_table() {
        let mut app = cockpit_fixture_app();
        // Sanity: fixture has T200 (accepted, terminal) + T100 (executing).
        let buf = paint(&mut app, 120, 30);
        let painted = buffer_to_string(&buf);
        let lines: Vec<&str> = painted.lines().collect();
        let middle: String = lines[TOP_STRIP_HEIGHT as usize..lines.len() - 3].join("\n");
        assert!(
            middle.contains("T100"),
            "active task T100 should be in focused-table region:\n{middle}"
        );
        assert!(
            !middle.contains("T200"),
            "terminal task T200 must NOT be in focused-table region (TasksRecentlyTerminal classification hidden):\n{middle}"
        );
    }
}
