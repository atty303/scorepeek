use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};
use scorepeek_scores::Store;
use serde::Serialize;

#[derive(Serialize)]
struct Summary {
    schema: &'static str,
    database_version: i64,
    play_results: i64,
    stored_results: i64,
    migrated_results: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: migrate DATABASE")?;
    let migrated_results = Store::migrate(&path)?;
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let database_version = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let play_results =
        connection.query_row("SELECT count(*) FROM play_results", [], |row| row.get(0))?;
    let stored_results = connection.query_row(
        "SELECT count(*) FROM play_results WHERE json_extract(event_json,'$.schema')='scorepeek-stored-result-v1'",
        [],
        |row| row.get(0),
    )?;
    println!(
        "{}",
        serde_json::to_string(&Summary {
            schema: "scorepeek-scores-migration-summary-v1",
            database_version,
            play_results,
            stored_results,
            migrated_results,
        })?
    );
    Ok(())
}
