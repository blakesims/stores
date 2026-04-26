/// Template rendering module — Handlebars-backed engine for briefing and
/// main.md templates.
pub mod context;
pub mod engine;
pub mod path;

pub use context::build_context;
pub use engine::render_template;
