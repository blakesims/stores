use ratatui::backend::TestBackend;
use ratatui::Terminal;
use stores::tui::app::{App, TuiOpts};
use stores::tui::data::{classify, Row, Section, StoreLane, TaskRow};

fn primary_task() -> TaskRow {
    TaskRow {
        display_id: "T900".to_string(),
        status: "legacy_unknown".to_string(),
        title: "primary tuple task".to_string(),
        updated_at: "2026-05-11T00:00:00Z".to_string(),
        lifecycle: Some("integration".to_string()),
        active_step: Some("none".to_string()),
        integration_step: Some("testing".to_string()),
        blocked: Some(false),
        ..Default::default()
    }
}

#[test]
fn tui_primary_columns() {
    let row = Row::Task(primary_task());
    let sections = classify(&[row.clone()]);
    let integration_bucket = sections
        .iter()
        .find(|(s, _)| *s == Section::TasksIntegration)
        .unwrap();
    assert_eq!(integration_bucket.1, vec![0]);

    let mut app = App::new(TuiOpts::default());
    app.rows = vec![row];
    app.sections = sections;
    app.focused_store = StoreLane::Tasks;

    let backend = TestBackend::new(200, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| stores::tui::render::draw(f, &mut app))
        .unwrap();
    let painted = format!("{}", terminal.backend());

    assert!(painted.contains("INTEGRATION"), "{painted}");
    assert!(painted.contains("lifecycle=integration"), "{painted}");
    assert!(painted.contains("integration_step=testing"), "{painted}");
    assert!(
        !painted.contains("legacy_unknown"),
        "status must not drive the rendered task badge: {painted}"
    );
}
