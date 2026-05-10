//! `builtin:activate-queued` — release armed queued tasks when blockers clear.

use anyhow::Context;
use serde_json::Value;

use crate::flow::builtins::{BuiltinResult, DispatchCtx};

pub fn run(row: &Value, ctx: &DispatchCtx) -> BuiltinResult {
    let schema =
        crate::flow::builtins::load_tasks_schema().context("activate-queued: load tasks schema")?;
    let tx = ctx.conn.unchecked_transaction()?;

    let display_ids: Vec<String> = match row.get("display_id").and_then(|v| v.as_str()) {
        Some(id) if !id.is_empty() => vec![id.to_string()],
        _ => {
            let mut stmt = tx.prepare(
                "SELECT display_id FROM tasks \
                 WHERE activation='active' AND lifecycle='queued' \
                 ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row?);
            }
            ids
        }
    };

    let mut promoted_count = 0;
    for display_id in display_ids {
        let (row_id, existing) = crate::handlers::row::read_row(&schema, &tx, &display_id)?;
        let promoted = crate::handlers::activate::try_release_queued_in_tx(
            &tx,
            &schema,
            row_id,
            &display_id,
            &existing,
            "release-from-queued",
            Some("activate_queued"),
        )?;
        if promoted {
            promoted_count += 1;
            eprintln!("[activate-queued] {display_id}: promoted queued→active");
        } else {
            eprintln!("[activate-queued] {display_id}: still queued");
        }
    }
    tx.commit()?;

    Ok(promoted_count)
}
