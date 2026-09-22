//! `SQLite` schema migration authority.

use rusqlite::{Transaction, params};
use serde_json::Value;

use super::error::Error;
use super::event::{ResultData, STORED_RESULT_SCHEMA};
use super::facts::PlaySide;

pub const CURRENT_SCHEMA_VERSION: i64 = 4;

struct LegacyPlayRow {
    event_id: String,
    session_id: Option<String>,
    attempt_id: Option<i64>,
    state: String,
    latest_event_id: String,
    recovery_confirmed: i64,
    song_id: String,
    play_type: String,
    difficulty: String,
    emitted_unix_ms: i64,
    received_unix_ms: i64,
    score: i64,
    miss: Option<i64>,
    clear: i64,
    event_json: String,
}

fn migrate_stored_result_v1(json: &str, row_play_type: &str) -> Result<(PlaySide, String), Error> {
    let mut raw: Value = serde_json::from_str(json)?;
    if raw["schema"].as_str() != Some("scorepeek-stored-result-v1")
        || raw["result"]["contract"].as_str() != Some("scorepeek-result-detected-v3")
        || raw["result"]["play_type"].as_str() != Some(row_play_type)
    {
        return Err(Error::UnsupportedContract);
    }
    let legacy = &raw["result"]["play_side"];
    let play_side = match (row_play_type, legacy["status"].as_str()) {
        ("single", Some("known")) => {
            PlaySide::parse(legacy["value"].as_str().ok_or(Error::UnsupportedContract)?)?
        }
        ("double", Some("not_applicable")) if legacy.get("value").is_none() => PlaySide::OnePlayer,
        _ => return Err(Error::UnsupportedContract),
    };
    raw["schema"] = Value::String(STORED_RESULT_SCHEMA.to_owned());
    raw["result"]["contract"] = Value::String("scorepeek-result-detected-v4".to_owned());
    raw["result"]["play_side"] = serde_json::to_value(play_side)?;
    serde_json::from_value::<ResultData>(raw["result"].clone())?;
    Ok((play_side, serde_json::to_string(&raw)?))
}

pub(super) fn migrate_database_v3_to_v4(tx: &Transaction<'_>) -> Result<(), Error> {
    let rows = {
        let mut statement = tx.prepare(
            "SELECT event_id,session_id,attempt_id,state,latest_event_id,recovery_confirmed,song_id,play_type,difficulty,emitted_unix_ms,received_unix_ms,score,miss,clear,event_json FROM play_results ORDER BY rowid",
        )?;
        statement
            .query_map([], |row| {
                Ok(LegacyPlayRow {
                    event_id: row.get(0)?,
                    session_id: row.get(1)?,
                    attempt_id: row.get(2)?,
                    state: row.get(3)?,
                    latest_event_id: row.get(4)?,
                    recovery_confirmed: row.get(5)?,
                    song_id: row.get(6)?,
                    play_type: row.get(7)?,
                    difficulty: row.get(8)?,
                    emitted_unix_ms: row.get(9)?,
                    received_unix_ms: row.get(10)?,
                    score: row.get(11)?,
                    miss: row.get(12)?,
                    clear: row.get(13)?,
                    event_json: row.get(14)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let rows = rows
        .into_iter()
        .map(|row| {
            let (play_side, event_json) =
                migrate_stored_result_v1(&row.event_json, &row.play_type)?;
            Ok((row, play_side, event_json))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    tx.execute_batch(
        "DROP INDEX plays_chart;
         DROP INDEX plays_attempt;
         ALTER TABLE play_results RENAME TO play_results_v3;
         CREATE TABLE play_results (event_id TEXT PRIMARY KEY, session_id TEXT, attempt_id INTEGER, state TEXT NOT NULL, latest_event_id TEXT NOT NULL, recovery_confirmed INTEGER NOT NULL DEFAULT 0, song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, play_side TEXT NOT NULL, emitted_unix_ms INTEGER NOT NULL, received_unix_ms INTEGER NOT NULL, score INTEGER NOT NULL, miss INTEGER, clear INTEGER NOT NULL, event_json TEXT NOT NULL);
         CREATE INDEX plays_chart ON play_results(song_id,play_type,difficulty);
         CREATE UNIQUE INDEX plays_attempt ON play_results(session_id,attempt_id);",
    )?;
    for (row, play_side, event_json) in rows {
        tx.execute(
            "INSERT INTO play_results(event_id,session_id,attempt_id,state,latest_event_id,recovery_confirmed,song_id,play_type,difficulty,play_side,emitted_unix_ms,received_unix_ms,score,miss,clear,event_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                row.event_id,
                row.session_id,
                row.attempt_id,
                row.state,
                row.latest_event_id,
                row.recovery_confirmed,
                row.song_id,
                row.play_type,
                row.difficulty,
                play_side.as_str(),
                row.emitted_unix_ms,
                row.received_unix_ms,
                row.score,
                row.miss,
                row.clear,
                event_json,
            ],
        )?;
    }
    tx.execute_batch("DROP TABLE play_results_v3; PRAGMA user_version=4;")?;
    Ok(())
}
