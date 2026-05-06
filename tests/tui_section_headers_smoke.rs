//! T028 P6 / AC6.3: TUI smoke — boots the renderer against a `TestBackend`
//! with synthetic rows for ~200ms-equivalent and asserts the section
//! headers are painted into the buffer. Acts as the "dev smoke" entry
//! requested by the phase.

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::time::{Duration, Instant};

use stores::tui::app::{App, TuiOpts};
use stores::tui::data::{classify, ObsRow, Row, Section, TaskRow};
use stores::tui::render;

fn build_app() -> App {
    let mut app = App::new(TuiOpts::default());
    app.rows = vec![
        Row::Task(TaskRow {
            display_id: "T001".into(),
            status: "plan_review".into(),
            title: "needs review".into(),
            updated_at: "2026-05-05".into(),
            ..Default::default()
        }),
        Row::Task(TaskRow {
            display_id: "T002".into(),
            status: "executing".into(),
            title: "in flight".into(),
            updated_at: "2026-05-04".into(),
            ..Default::default()
        }),
        Row::Obs(ObsRow {
            display_id: "L001".into(),
            status: "investigating".into(),
            priority: "high".into(),
            summary: "ratifiable obs".into(),
            updated_at: "2026-05-05".into(),
            contract_state: Some("ready".into()),
            ..Default::default()
        }),
    ];
    app.sections = classify(&app.rows);
    app.apply_sort();
    app.viewport_height = 20;
    app
}

#[test]
fn tui_smoke_paints_section_headers_within_200ms() {
    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test backend");
    let mut app = build_app();

    let deadline = Instant::now() + Duration::from_millis(200);
    let mut frames = 0u32;
    while Instant::now() < deadline {
        terminal.draw(|f| render::draw(f, &mut app)).expect("draw");
        frames += 1;
    }
    assert!(frames > 0, "expected at least one frame within 200ms");

    let buf = terminal.backend().buffer().clone();
    let mut painted = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            painted.push_str(buf[(x, y)].symbol());
        }
        painted.push('\n');
    }

    // At least one task and one obs section header must be visible.
    let task_label_present = painted.contains(Section::TasksActionableCurrentWork.label())
        || painted.contains(Section::TasksBlockedNeedsAction.label())
        || painted.contains(Section::TasksDeployRecovery.label())
        || painted.contains(Section::TasksRecentlyTerminal.label());
    let obs_label_present = painted.contains(Section::ObsRatifiable.label())
        || painted.contains(Section::ObsOpenNoContract.label())
        || painted.contains(Section::ObsOther.label());

    assert!(
        task_label_present,
        "expected a TASKS section header in the painted buffer; got:\n{painted}"
    );
    assert!(
        obs_label_present,
        "expected an OBS section header in the painted buffer; got:\n{painted}"
    );
}
