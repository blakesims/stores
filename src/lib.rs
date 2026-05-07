//! Library entry point so integration tests in `tests/` can import the
//! same modules the `stores` binary uses.

pub mod cli;
pub mod codegen;
pub mod db;
pub mod flow;
pub mod handlers;
pub mod id_format;
pub mod install;
pub mod manifest;
pub mod output;
pub mod paths;
pub mod render;
pub mod runner;
pub mod schema;
pub mod tui;
pub mod validate;
pub mod version;
