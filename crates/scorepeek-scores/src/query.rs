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
    pub received_unix_ms: i64,
    pub score: i64,
    pub miss: Option<i64>,
    pub clear: i64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct Dashboard {
    pub recorded: bool,
    pub best: Best,
    pub representative: Option<serde_json::Value>,
    pub recent: Vec<Play>,
    pub graph: Vec<Play>,
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
            "SELECT event_id,emitted_unix_ms,received_unix_ms,score,miss,clear FROM play_results WHERE song_id=?1 AND play_type=?2 AND difficulty=?3 ORDER BY received_unix_ms DESC, emitted_unix_ms DESC, event_id DESC LIMIT 5",
        )?;
        statement
            .query_map(params![song_id, play_type, difficulty], |row| {
                Ok(Play {
                    event_id: row.get(0)?,
                    emitted_unix_ms: row.get(1)?,
                    received_unix_ms: row.get(2)?,
                    score: row.get(3)?,
                    miss: row.get(4)?,
                    clear: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    transaction.commit()?;
    Ok(ChartHistory { best, plays })
}

/// Reads the committed widget projection for one selected chart.
/// Reads the committed best, representative result, list and graph rows for one chart.
/// # Errors
/// Returns an open, schema, query, or decode error without creating or migrating the database.
pub fn chart_dashboard(
    path: &Path,
    song_id: &str,
    play_type: &str,
    difficulty: &str,
    recent_limit: usize,
    since_unix_ms: i64,
) -> Result<Dashboard, Error> {
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_millis(250))?;
    let tx = connection.transaction()?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != 1 {
        return Err(Error::UnsupportedDatabase(version));
    }
    let row:Option<(Option<i64>,Option<i64>,Option<i64>)>=tx.query_row("SELECT score,miss,clear FROM chart_bests WHERE song_id=?1 AND play_type=?2 AND difficulty=?3",params![song_id,play_type,difficulty],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let recorded = row.is_some();
    let best = row.map_or_else(Best::default, |(score, miss, clear)| Best {
        score,
        miss,
        clear,
    });
    let representative=tx.query_row("SELECT event_json FROM play_results WHERE song_id=?1 AND play_type=?2 AND difficulty=?3 ORDER BY score DESC, miss IS NULL, miss ASC, received_unix_ms DESC, emitted_unix_ms DESC, event_id DESC LIMIT 1",params![song_id,play_type,difficulty],|r|r.get::<_,String>(0)).optional()?.map(|v|serde_json::from_str(&v)).transpose()?;
    let read = |sql: &str, extra: Option<i64>| -> Result<Vec<Play>, Error> {
        let mut stmt = tx.prepare(sql)?;
        let map = |r: &rusqlite::Row<'_>| {
            Ok(Play {
                event_id: r.get(0)?,
                emitted_unix_ms: r.get(1)?,
                received_unix_ms: r.get(2)?,
                score: r.get(3)?,
                miss: r.get(4)?,
                clear: r.get(5)?,
            })
        };
        if let Some(since) = extra {
            Ok(stmt
                .query_map(params![song_id, play_type, difficulty, since], map)?
                .collect::<Result<Vec<_>, _>>()?)
        } else {
            Ok(stmt
                .query_map(
                    params![
                        song_id,
                        play_type,
                        difficulty,
                        i64::try_from(recent_limit).unwrap_or(50)
                    ],
                    map,
                )?
                .collect::<Result<Vec<_>, _>>()?)
        }
    };
    let recent = read(
        "SELECT event_id,emitted_unix_ms,received_unix_ms,score,miss,clear FROM play_results WHERE song_id=?1 AND play_type=?2 AND difficulty=?3 ORDER BY received_unix_ms DESC, emitted_unix_ms DESC, event_id DESC LIMIT ?4",
        None,
    )?;
    let graph = read(
        "SELECT event_id,emitted_unix_ms,received_unix_ms,score,miss,clear FROM (SELECT event_id,emitted_unix_ms,received_unix_ms,score,miss,clear FROM play_results WHERE song_id=?1 AND play_type=?2 AND difficulty=?3 AND received_unix_ms>=?4 ORDER BY received_unix_ms DESC, emitted_unix_ms DESC, event_id DESC LIMIT 4096) ORDER BY received_unix_ms, emitted_unix_ms, event_id",
        Some(since_unix_ms),
    )?;
    tx.commit()?;
    Ok(Dashboard {
        recorded,
        best,
        representative,
        recent,
        graph,
    })
}

#[cfg(test)]
mod tests {
    use super::chart_dashboard;
    use rusqlite::{Connection, params};

    #[test]
    fn graph_keeps_the_newest_bounded_points_in_timestamp_order() {
        let path = std::env::temp_dir().join(format!(
            "scorepeek-dashboard-{}-{}.sqlite3",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let mut connection = Connection::open(&path).unwrap();
        connection.pragma_update(None, "user_version", 1).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE chart_bests(song_id TEXT,play_type TEXT,difficulty TEXT,score INTEGER,miss INTEGER,clear INTEGER);
                 CREATE TABLE play_results(event_id TEXT,emitted_unix_ms INTEGER,received_unix_ms INTEGER,score INTEGER,miss INTEGER,clear INTEGER,event_json TEXT,song_id TEXT,play_type TEXT,difficulty TEXT);",
            )
            .unwrap();
        let tx = connection.transaction().unwrap();
        for sequence in 0_i64..4_100 {
            tx.execute(
                "INSERT INTO play_results VALUES(?1,?2,?2,?2,NULL,0,'{}','song','single','hyper')",
                params![format!("event-{sequence:04}"), sequence],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(connection);

        let dashboard = chart_dashboard(&path, "song", "single", "hyper", 5, 0).unwrap();
        assert_eq!(dashboard.graph.len(), 4_096);
        assert_eq!(dashboard.graph.first().unwrap().received_unix_ms, 4);
        assert_eq!(dashboard.graph.last().unwrap().received_unix_ms, 4_099);
        assert!(
            dashboard
                .graph
                .windows(2)
                .all(|pair| pair[0].received_unix_ms < pair[1].received_unix_ms)
        );
        std::fs::remove_file(path).unwrap();
    }
}
