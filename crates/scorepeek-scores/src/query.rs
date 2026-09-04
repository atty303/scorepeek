//! Read-only views of committed scores. Opening a reader never creates a database.
use crate::Error;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Best {
    pub score: Option<i64>,
    pub miss: Option<i64>,
    pub clear: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Play {
    pub event_id: String,
    pub emitted_unix_ms: i64,
    pub score: i64,
    pub miss: Option<i64>,
    pub clear: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ChartHistory {
    pub best: Best,
    pub plays: Vec<Play>,
}

/// One short transaction keeps best and history on the same committed snapshot.
/// # Errors
/// Returns an error for missing, unreadable or unsupported databases.
pub fn chart_history(
    path: &Path,
    song_id: &str,
    play_type: &str,
    difficulty: &str,
) -> Result<ChartHistory, Error> {
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_millis(250))?;
    let transaction = connection.transaction()?;
    let version: i64 = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 1 {
        return Err(Error::UnsupportedDatabase(version));
    }
    let best = transaction.query_row(
        "SELECT score,miss,clear FROM chart_bests WHERE song_id=?1 AND play_type=?2 AND difficulty=?3",
        params![song_id, play_type, difficulty],
        |row| Ok(Best { score: row.get(0)?, miss: row.get(1)?, clear: row.get(2)? }),
    ).optional()?.unwrap_or_default();
    let plays = {
        let mut statement = transaction.prepare(
            "SELECT event_id,emitted_unix_ms,score,miss,clear FROM play_results WHERE song_id=?1 AND play_type=?2 AND difficulty=?3 ORDER BY rowid DESC LIMIT 5",
        )?;
        statement
            .query_map(params![song_id, play_type, difficulty], |row| {
                Ok(Play {
                    event_id: row.get(0)?,
                    emitted_unix_ms: row.get(1)?,
                    score: row.get(2)?,
                    miss: row.get(3)?,
                    clear: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    transaction.commit()?;
    Ok(ChartHistory { best, plays })
}
