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
    blocked_reason_class, cockpit_model, recent_exhaust, store_flow_model, ExternalReviewState,
    Row, Section, StoreFlowModel, StoreLane,
};

/// Height (in rows) of the cockpit's top store-flow strip (5 cards drawn
/// inside a bordered block ⇒ 4 lines: top border + 2 body rows + bottom
/// border). Exposed so integration tests can derive the focused-table
/// region's vertical span without re-encoding the literal.
pub const TOP_STRIP_HEIGHT: u16 = 4;

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

/// Render the 5-card store-flow strip across the top of the cockpit. Each
/// card shows the lane label, a primary count, and a one-line status
/// breakdown drawn from [`StoreFlowModel`]. The focused lane gets a cyan
/// border; unfocused lanes are dim.
fn draw_store_strip(f: &mut Frame, app: &App, area: Rect) {
    let model = store_flow_model(
        &app.rows,
        &app.system_health,
        &app.status_bar.daemon_liveness,
        &app.external_review,
    );
    let cells = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 5); 5])
        .split(area);
    for (i, lane) in StoreLane::ALL.iter().enumerate() {
        draw_store_card(f, *lane, app.focused_store == *lane, &model, cells[i]);
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
            Style::default().fg(Color::DarkGray),
            Style::default().fg(Color::DarkGray),
        )
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(format!(" {} ", lane.label()), title_style));
    let (primary, breakdown) = lane_card_lines(lane, model);
    let primary_style = if focused {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Gray)
    };
    let body = Paragraph::new(vec![
        Line::from(Span::styled(primary, primary_style)),
        Line::from(Span::styled(
            breakdown,
            Style::default().fg(Color::DarkGray),
        )),
    ])
    .block(block);
    f.render_widget(body, area);
}

fn lane_card_lines(lane: StoreLane, model: &StoreFlowModel) -> (String, String) {
    match lane {
        StoreLane::Intake => {
            let i = &model.intake;
            let open = i.draft + i.triaging + i.needs_info;
            (
                format!("open: {open}"),
                format!("needs_info {} · routed {}", i.needs_info, i.routed),
            )
        }
        StoreLane::Observations => {
            let o = &model.observations;
            (
                format!("open: {}", o.open),
                format!("invest {} · ready {}", o.investigating, o.ready),
            )
        }
        StoreLane::Tasks => {
            let t = &model.tasks;
            let review = t.plan_review + t.code_review + t.in_review;
            (
                format!("active: {}", t.active),
                format!("held {} · review {}", t.held, review),
            )
        }
        StoreLane::ExternalReviews => {
            let r = &model.external_reviews;
            (
                format!("running: {}", r.running),
                format!(
                    "pending {} · revise {} · held {}",
                    r.pending, r.revise, r.tooling_held
                ),
            )
        }
        StoreLane::EngineHealth => {
            let e = &model.engine;
            let primary = if e.daemon_live {
                "daemon LIVE".to_string()
            } else {
                "daemon DEAD ⚠".to_string()
            };
            let age = match e.oldest_lock_age_secs {
                Some(secs) if secs >= 3600 => format!("oldest {}h", secs / 3600),
                Some(secs) if secs >= 60 => format!("oldest {}m", secs / 60),
                Some(_) => "oldest <1m".to_string(),
                None => "oldest —".to_string(),
            };
            (primary, format!("locks {} · {}", e.unfinished_locks, age))
        }
    }
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
    let mut last_section: Option<usize> = None;
    let cursor = app.current_flat();
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
                Row::Task(t) => Some(format!(
                    "{} {} lifecycle={} active_step={} integration_step={}",
                    t.display_id,
                    t.status,
                    super::data::task_lifecycle(t),
                    super::data::task_active_step(t),
                    super::data::task_integration_step(t)
                )),
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
    if selected {
        let mut spans = base;
        for s in spans.iter_mut() {
            s.style = s.style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
        }
        Line::from(spans)
    } else {
        Line::from(base)
    }
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
    let runner = t
        .claimed_by
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("none");
    let age = age_label(super::data::parse_epoch(&t.updated_at));
    vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<6}", t.display_id),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            format!("{:<24}", task_status_label(t)),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" "),
        Span::raw(task_progress_text(t, external_review)),
        Span::raw(format!("runner:{runner} ")),
        Span::raw(format!("{age} ")),
        Span::raw(truncate(&task_snippet(t), 60)),
    ]
}

fn format_obs_line(
    o: &super::data::ObsRow,
    collapsed: Option<&super::data::CollapsedObsRow>,
) -> Vec<Span<'static>> {
    let status = if o.status == "investigation_failed" {
        match o.investigation_failure_reason.as_deref() {
            Some(reason) if !reason.trim().is_empty() => {
                format!("investigation_failed:{}", truncate(reason.trim(), 40))
            }
            _ => "investigation_failed:unknown".to_string(),
        }
    } else {
        o.status.clone()
    };
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
    let contract = o.contract_state.as_deref().filter(|s| !s.is_empty());
    let linked = collapsed
        .is_none()
        .then(|| o.task_id.as_deref().filter(|s| !s.is_empty()))
        .flatten();
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<6}", display_id),
            Style::default().fg(Color::Magenta),
        ),
        Span::styled(
            format!("{:<24}", status),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" "),
        Span::raw(format!("priority:{}{} ", o.priority, badge)),
    ];
    if let Some(t) = tier {
        spans.push(Span::raw(format!("tier:{t} ")));
    }
    if let Some(c) = contract {
        spans.push(Span::raw(format!("contract:{c} ")));
    }
    if let Some(l) = linked {
        spans.push(Span::raw(format!("linked:{l} ")));
    }
    spans.push(Span::raw(truncate(
        &format!("{}{}", summary_prefix, obs_snippet(o)),
        60,
    )));
    spans
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
            format!("{:<24}", i.status),
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
        .filter(|s| !s.is_empty())
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
    let runner = if r.runner.is_empty() {
        "unknown"
    } else {
        r.runner.as_str()
    };
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<6}", r.display_id),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(
            format!("{:<24}", format!("review:{}", r.status)),
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
    if t.status == "rejected" {
        return format!(
            "terminal-unless-amended lifecycle={} active_step={} integration_step={}",
            super::data::task_lifecycle(t),
            super::data::task_active_step(t),
            super::data::task_integration_step(t)
        );
    }
    if super::data::task_is_terminal_primary(t) {
        return format!(
            "recovered/done lifecycle={} active_step={} integration_step={}",
            super::data::task_lifecycle(t),
            super::data::task_active_step(t),
            super::data::task_integration_step(t)
        );
    }
    if super::data::task_is_blocked(t) {
        let kind = t
            .blocker_kind
            .as_deref()
            .filter(|s| !s.is_empty() && *s != "none")
            .or(t.blocked_reason_class.as_deref())
            .unwrap_or_else(|| blocked_reason_class(t.blocked_reason.as_deref()));
        return format!("blocked:{kind}");
    }
    let primary = format!(
        "lifecycle={} active_step={} integration_step={}",
        super::data::task_lifecycle(t),
        super::data::task_active_step(t),
        super::data::task_integration_step(t)
    );
    if t.status == "legacy_unknown" {
        primary
    } else {
        format!("{} {primary}", t.status)
    }
}

fn task_progress_text(t: &super::data::TaskRow, external_review: &ExternalReviewState) -> String {
    let progress = super::progress::task_progress(t, external_review);
    if progress.text == t.status {
        format!(
            "{}:{}:{} ",
            super::data::task_lifecycle(t),
            super::data::task_active_step(t),
            super::data::task_integration_step(t)
        )
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
    use crate::tui::data::{IntakeRow, ObsRow, Row, SystemHealth, TaskRow};
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
        assert!(closed.contains("recovered/done"));
        assert!(!closed.contains("in flight"));

        let rejected = line_text(format_row_line(
            &task_row("rejected", None),
            false,
            &ExternalReviewState::default(),
        ));
        assert!(rejected.contains("terminal-unless-amended"));
        assert!(!rejected.contains("in flight"));

        let blocked = line_text(format_row_line(
            &task_row("blocked", Some("rate limit 429")),
            false,
            &ExternalReviewState::default(),
        ));
        assert!(blocked.contains("blocked:rate_limit"));
        let unknown = line_text(format_row_line(
            &task_row("blocked", Some("opaque")),
            false,
            &ExternalReviewState::default(),
        ));
        assert!(unknown.contains("blocked:unknown"));
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
        assert!(obs_text.contains("priority:high"), "{obs_text}");
        assert!(obs_text.contains("tier:T2"), "{obs_text}");
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
        assert!(text.contains("investigation_failed:rate_limit"), "{text}");
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

        for label in ["INTEGRATION (2)", "INTEGRATED (1)", "HELD-INTEGRATION (1)"] {
            assert!(
                painted.contains(label),
                "expected section header '{label}' in painted frame:\n{painted}"
            );
        }
        for id in ["T800", "T801", "T802", "T803"] {
            assert!(
                painted.contains(id),
                "expected row id {id} painted:\n{painted}"
            );
        }
        // Sections must appear in canonical order: INTEGRATION before
        // INTEGRATED before HELD-INTEGRATION.
        let pos_int = painted.find("INTEGRATION (2)").unwrap();
        let pos_integrated = painted.find("INTEGRATED (1)").unwrap();
        let pos_held = painted.find("HELD-INTEGRATION (1)").unwrap();
        assert!(pos_int < pos_integrated && pos_integrated < pos_held);
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

    use crate::tui::data::{classify, ReviewRow, StoreLane};
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

    /// AC3.1(a) / AC3.2: top strip paints all five lane labels.
    #[test]
    fn cockpit_top_strip_paints_all_five_lane_labels() {
        let mut app = cockpit_fixture_app();
        let buf = paint(&mut app, 120, 30);
        let painted = buffer_to_string(&buf);
        // Top region is rows 0..4 (the 5-card strip).
        let top_region: String = painted.lines().take(4).collect::<Vec<_>>().join("\n");
        for label in ["INTAKE", "OBSERVATIONS", "TASKS", "EXTERNAL", "ENGINE"] {
            assert!(
                top_region.contains(label),
                "missing top-strip label {label:?} in top region:\n{top_region}\n\nfull buffer:\n{painted}"
            );
        }
    }

    /// AC3.1(b): focused-lane card border is cyan; unfocused are dim.
    #[test]
    fn cockpit_focused_card_has_distinct_border_style() {
        let mut app = cockpit_fixture_app();
        // Default focus = Tasks (index 2 in StoreLane::ALL).
        let buf = paint(&mut app, 120, 30);
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
            exhaust_line.contains("accepted"),
            "exhaust strip must show terminal status; got: {exhaust_line}"
        );
        // Focused-table region (between top strip y=4 and exhaust y=h-3).
        let middle: String = lines[4..lines.len() - 3].join("\n");
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
        let middle: String = lines[4..lines.len() - 3].join("\n");
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
        let buf = paint(&mut app, 120, 30);
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

        let buf2 = paint(&mut app, 120, 30);
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

    /// Phase 3 (T141): per-row column renderers surface store-specific cues.
    #[test]
    fn format_obs_line_surfaces_tier_and_contract() {
        let row = Row::Obs(ObsRow {
            display_id: "L200".to_string(),
            status: "open".to_string(),
            priority: "normal".to_string(),
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
        assert!(text.contains("tier:T2"), "{text}");
        assert!(text.contains("contract:ready"), "{text}");
        assert!(text.contains("linked:T555"), "{text}");

        // None/empty tier/contract/task_id are simply omitted — keeps the
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
    fn format_intake_line_surfaces_source_and_cluster() {
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
        assert!(text.contains("source:executor"), "{text}");
        assert!(text.contains("cluster:watch-ux"), "{text}");
        assert!(text.contains("age:"), "{text}");
    }

    #[test]
    fn format_review_line_surfaces_verdict_and_attempts() {
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
        assert!(text.contains("verdict:PASS"), "{text}");
        assert!(text.contains("attempts:2"), "{text}");
        assert!(text.contains("sha:abcdef0"), "{text}");
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
        let middle: String = lines[4..lines.len() - 3].join("\n");
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
