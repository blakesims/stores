use anyhow::{Context, Result};
use rusqlite::Connection;

use crate::flow::{builtins::DispatchCtx, AgentsYaml};
use crate::handlers::row::read_row;
use crate::schema::Schema;

/// Run exactly one `external_reviews` row through the external-review builtin.
///
/// This is the manual-control alternative to `stores agents run --once`: it does
/// not run daemon startup sweeps, the task watchdog, auto-drive, auto-promote,
/// or the engine-runner actionability loop.
pub fn run_external_review_row(schema: &Schema, conn: &Connection, display_id: &str) -> Result<()> {
    let (_row_id, row) = read_row(schema, conn, display_id)
        .with_context(|| format!("loading external_review row {display_id}"))?;

    let stores_dir = crate::paths::stores_dir()?;
    let config_path = stores_dir.join("config.yaml");
    let agents = AgentsYaml::default_empty();
    let ctx = DispatchCtx {
        conn,
        agents: &agents,
        config_path: &config_path,
        policies_hash: "",
    };

    let row_json = serde_json::Value::Object(row.into_iter().collect());
    let outcome = crate::flow::builtins::external_review::run(&row_json, &ctx)?;

    let status: String = conn.query_row(
        "SELECT status FROM external_reviews WHERE display_id=?1",
        rusqlite::params![display_id],
        |r| r.get(0),
    )?;
    println!("external_reviews run {display_id}: outcome={outcome:?} status={status}");
    Ok(())
}
