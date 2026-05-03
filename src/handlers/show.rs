use anyhow::Result;
use clap::ArgMatches;
use rusqlite::Connection;

use crate::schema::{actor::InvokerCtx, Schema};

use super::row::read_row;
use crate::output;

pub fn run(
    schema: &Schema,
    conn: &Connection,
    matches: &ArgMatches,
    _invoker: InvokerCtx,
) -> Result<()> {
    let display_id = matches
        .get_one::<String>("display_id")
        .map(|s| s.as_str())
        .unwrap_or("");

    let json_flag = matches.get_flag("json");

    let (_id, entry) = read_row(schema, conn, display_id)?;

    if json_flag {
        output::print_entry_json(&entry);
    } else {
        output::print_entry_text(&entry);
    }

    Ok(())
}
