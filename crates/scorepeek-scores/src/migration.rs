//! `SQLite` schema migration authority.

use rusqlite::{OptionalExtension, Transaction, params};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::error::Error;
use super::event::{Field, result_clear};
use super::event::{ResultData, STORED_RESULT_SCHEMA};
use super::facts::PlaySide;
use super::facts::{COLUMNS, Fact, Fields, Origin, Source, cumulative, fact, integrate, known};

pub const CURRENT_SCHEMA_VERSION: i64 = 5;

pub(super) fn backup_database(
    connection: &rusqlite::Connection,
    path: &Path,
    version: i64,
) -> Result<PathBuf, Error> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::UnsupportedContract)?
        .as_nanos();
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".v{version}.backup-{stamp}-{}", std::process::id()));
    let backup = PathBuf::from(name);
    let backup_sql_path = backup.to_str().ok_or(Error::UnsupportedContract)?;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&backup)?;
    let attempt = (|| {
        connection.execute("VACUUM main INTO ?1", [backup_sql_path])?;
        let snapshot = rusqlite::Connection::open_with_flags(
            &backup,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let saved_version: i64 =
            snapshot.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let check: String = snapshot.pragma_query_value(None, "quick_check", |row| row.get(0))?;
        if saved_version != version || check != "ok" {
            return Err(Error::UnsupportedContract);
        }
        let original_counts: (i64, i64) = connection.query_row(
            "SELECT (SELECT count(*) FROM play_results),(SELECT count(*) FROM chart_bests)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let copied_counts: (i64, i64) = snapshot.query_row(
            "SELECT (SELECT count(*) FROM play_results),(SELECT count(*) FROM chart_bests)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if original_counts != copied_counts {
            return Err(Error::UnsupportedContract);
        }
        fs::File::open(&backup)?.sync_all()?;
        let parent = backup.parent().unwrap_or(Path::new("."));
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = attempt {
        let _ = fs::remove_file(&backup);
        return Err(error);
    }
    Ok(backup)
}

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

fn migrate_stored_result_v1(json: &str, row_play_type: &str) -> Result<PlaySide, Error> {
    let raw: Value = serde_json::from_str(json)?;
    legacy_side(&raw, row_play_type)
}

fn legacy_side(raw: &Value, row_play_type: &str) -> Result<PlaySide, Error> {
    if raw["schema"].as_str() != Some("scorepeek-stored-result-v1")
        || raw["result"]["play_type"].as_str() != Some(row_play_type)
    {
        return Err(Error::UnsupportedContract);
    }
    let result = &raw["result"];
    match result["contract"].as_str() {
        Some("scorepeek-result-detected-v2") => PlaySide::parse(
            result["play_side"]
                .as_str()
                .ok_or(Error::UnsupportedContract)?,
        ),
        Some("scorepeek-result-detected-v3") => {
            match (row_play_type, result["play_side"]["status"].as_str()) {
                ("single", Some("known")) => PlaySide::parse(
                    result["play_side"]["value"]
                        .as_str()
                        .ok_or(Error::UnsupportedContract)?,
                ),
                ("double", Some("not_applicable"))
                    if result["play_side"].get("value").is_none() =>
                {
                    Ok(PlaySide::OnePlayer)
                }
                _ => Err(Error::UnsupportedContract),
            }
        }
        _ => Err(Error::UnsupportedContract),
    }
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
            let play_side = migrate_stored_result_v1(&row.event_json, &row.play_type)?;
            Ok((row, play_side))
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
    for (row, play_side) in rows {
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
                row.event_json,
            ],
        )?;
    }
    tx.execute_batch("DROP TABLE play_results_v3; PRAGMA user_version=4;")?;
    Ok(())
}

#[derive(Deserialize)]
struct ScoringResult {
    contract: String,
    scorepeek_song_id: String,
    play_type: String,
    difficulty: String,
    current_score: u32,
    clear_type: String,
    miss_count: Field<u32>,
    previous_best: ScoringPrevious,
}

#[derive(Deserialize)]
struct ScoringPrevious {
    score: Field<u32>,
    miss_count: Field<u32>,
    clear_type: Field<String>,
}

pub(super) fn normalized_result(raw: &Value, play_type: &str) -> Result<Value, Error> {
    let mut result = raw["result"].clone();
    match (raw["schema"].as_str(), result["contract"].as_str()) {
        (
            Some("scorepeek-stored-result-v1"),
            Some("scorepeek-result-detected-v2" | "scorepeek-result-detected-v3"),
        ) => {
            let side = legacy_side(raw, play_type)?;
            result["play_side"] = serde_json::to_value(side)?;
            result["contract"] = Value::String("scorepeek-result-detected-v4".into());
        }
        (Some(STORED_RESULT_SCHEMA), Some("scorepeek-result-detected-v4")) => (),
        _ => return Err(Error::UnsupportedContract),
    }
    Ok(result)
}

fn scoring_facts(
    raw: &Value,
    result: &Value,
    received_unix_ms: i64,
    row: (&str, &str, &str, i64, Option<i64>, i64),
) -> Result<[Fields; 2], Error> {
    let scoring: ScoringResult = serde_json::from_value(result.clone())?;
    if scoring.contract != "scorepeek-result-detected-v4"
        || scoring.scorepeek_song_id != row.0
        || scoring.play_type != row.1
        || scoring.difficulty != row.2
    {
        return Err(Error::UnsupportedContract);
    }
    let current = [
        Some(i64::from(scoring.current_score)),
        known(&scoring.miss_count, |value| Ok(i64::from(*value)))?,
        Some(result_clear(&scoring.clear_type)?),
    ];
    if current != [Some(row.3), row.4, Some(row.5)] {
        return Err(Error::UnsupportedContract);
    }
    let origin = Origin {
        event_id: raw["event_id"]
            .as_str()
            .ok_or(Error::UnsupportedContract)?
            .to_owned(),
        invocation_id: raw["invocation_id"]
            .as_str()
            .ok_or(Error::UnsupportedContract)?
            .to_owned(),
        sequence: raw["sequence"].as_u64().ok_or(Error::UnsupportedContract)?,
        emitted_unix_ms: raw["emitted_unix_ms"]
            .as_i64()
            .ok_or(Error::UnsupportedContract)?,
        received_unix_ms: u64::try_from(received_unix_ms)
            .map_err(|_| Error::UnsupportedContract)?,
        revision: None,
        observation_id: None,
        capture: raw["capture"].clone(),
    };
    let previous = [
        known(&scoring.previous_best.score, |value| Ok(i64::from(*value)))?,
        known(&scoring.previous_best.miss_count, |value| {
            Ok(i64::from(*value))
        })?,
        known(&scoring.previous_best.clear_type, |value| {
            result_clear(value)
        })?,
    ];
    Ok([
        current.map(|value| fact(value, Source::Result, &origin)),
        previous.map(|value| fact(value, Source::PreviousBest, &origin)),
    ])
}

pub(super) fn save_facts(
    tx: &Transaction<'_>,
    event_id: &str,
    facts: &[Fields; 2],
) -> Result<(), Error> {
    tx.execute(
        "DELETE FROM play_result_facts WHERE event_id=?1",
        [event_id],
    )?;
    for (source, fields) in facts.iter().enumerate() {
        for (field, value) in fields.iter().enumerate() {
            if let Some(value) = value {
                tx.execute("INSERT INTO play_result_facts(event_id,source,field,fact) VALUES (?1,?2,?3,?4)", params![event_id, i64::try_from(source).map_err(|_| Error::UnsupportedContract)?, i64::try_from(field).map_err(|_| Error::UnsupportedContract)?, serde_json::to_string(value)?])?;
            }
        }
    }
    Ok(())
}

pub(super) fn migrate_database_v4_to_v5(tx: &Transaction<'_>) -> Result<u64, Error> {
    tx.execute_batch(&format!(
        "ALTER TABLE play_results ADD COLUMN detail_state TEXT NOT NULL DEFAULT 'available' CHECK(detail_state IN ('available','unavailable_at_migration'));
         {}\n
         CREATE TABLE play_result_facts(event_id TEXT NOT NULL,source INTEGER NOT NULL,field INTEGER NOT NULL,fact TEXT NOT NULL,PRIMARY KEY(event_id,source,field),FOREIGN KEY(event_id) REFERENCES play_results(event_id) ON DELETE CASCADE);",
        super::structured::SCHEMA
    ))?;
    let rows = {
        let mut statement = tx.prepare("SELECT event_id,song_id,play_type,difficulty,received_unix_ms,score,miss,clear,event_json FROM play_results ORDER BY rowid")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut unavailable = 0;
    for (event_id, song, play_type, difficulty, received, score, miss, clear, json) in rows {
        let raw: Value = serde_json::from_str(&json)?;
        let result = normalized_result(&raw, &play_type)?;
        let facts = scoring_facts(
            &raw,
            &result,
            received,
            (&song, &play_type, &difficulty, score, miss, clear),
        )?;
        save_facts(tx, &event_id, &facts)?;
        if serde_json::from_value::<ResultData>(result.clone()).is_ok() {
            let expected = result;
            super::structured::save(tx, &event_id, &expected)?;
            if super::structured::load(tx, &event_id)? != Some(expected) {
                return Err(Error::UnsupportedContract);
            }
        } else {
            unavailable += 1;
            tx.execute(
                "UPDATE play_results SET detail_state='unavailable_at_migration' WHERE event_id=?1",
                [&event_id],
            )?;
        }
    }
    validate_bests(tx)?;
    tx.pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)?;
    Ok(unavailable)
}

fn validate_bests(tx: &Transaction<'_>) -> Result<(), Error> {
    let charts = {
        let mut statement = tx.prepare("SELECT song_id,play_type,difficulty FROM chart_bests")?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for (song, play_type, difficulty) in charts {
        let mut facts: [Option<Fact>; 12] = std::array::from_fn(|_| None);
        for (index, column) in COLUMNS.iter().enumerate().skip(6).take(3) {
            let sql = format!(
                "SELECT {column} FROM chart_bests WHERE song_id=?1 AND play_type=?2 AND difficulty=?3"
            );
            let value: Option<String> =
                tx.query_row(&sql, params![song, play_type, difficulty], |row| row.get(0))?;
            facts[index] = value
                .map(|value| serde_json::from_str(&value))
                .transpose()?;
        }
        let mut statement = tx.prepare("SELECT event_id FROM play_results WHERE song_id=?1 AND play_type=?2 AND difficulty=?3 ORDER BY emitted_unix_ms,event_id")?;
        let rows = statement.query_map(params![song, play_type, difficulty], |row| {
            row.get::<_, String>(0)
        })?;
        for event_id in rows {
            let event_id = event_id?;
            let mut current: Fields = std::array::from_fn(|_| None);
            let mut previous: Fields = std::array::from_fn(|_| None);
            let mut fstmt =
                tx.prepare("SELECT source,field,fact FROM play_result_facts WHERE event_id=?1")?;
            let values = fstmt.query_map([&event_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for value in values {
                let (source, field, json) = value?;
                if !(0..=1).contains(&source) || !(0..=2).contains(&field) {
                    return Err(Error::UnsupportedContract);
                }
                (if source == 0 {
                    &mut current
                } else {
                    &mut previous
                })[usize::try_from(field).map_err(|_| Error::UnsupportedContract)?] =
                    Some(serde_json::from_str(&json)?);
            }
            cumulative(&mut facts[0..3], current);
            cumulative(&mut facts[3..6], previous);
        }
        integrate(&mut facts);
        for (index, column) in COLUMNS.iter().enumerate() {
            let sql = format!(
                "SELECT {column} FROM chart_bests WHERE song_id=?1 AND play_type=?2 AND difficulty=?3"
            );
            let saved: Option<String> =
                tx.query_row(&sql, params![song, play_type, difficulty], |row| row.get(0))?;
            let saved: Option<Fact> = saved.map(|v| serde_json::from_str(&v)).transpose()?;
            if saved.as_ref().and_then(|v| v.value) != facts[index].as_ref().and_then(|v| v.value) {
                return Err(Error::UnsupportedContract);
            }
        }
        let saved: (Option<i64>,Option<i64>,Option<i64>) = tx.query_row("SELECT score,miss,clear FROM chart_bests WHERE song_id=?1 AND play_type=?2 AND difficulty=?3", params![song,play_type,difficulty], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        if saved
            != (
                facts[9].as_ref().and_then(|v| v.value),
                facts[10].as_ref().and_then(|v| v.value),
                facts[11].as_ref().and_then(|v| v.value),
            )
        {
            return Err(Error::UnsupportedContract);
        }
    }
    let unrepresented: Option<i64> = tx.query_row("SELECT 1 FROM play_results p LEFT JOIN chart_bests b ON p.song_id=b.song_id AND p.play_type=b.play_type AND p.difficulty=b.difficulty WHERE b.song_id IS NULL LIMIT 1", [], |row| row.get(0)).optional()?;
    if unrepresented.is_some() {
        return Err(Error::UnsupportedContract);
    }
    Ok(())
}
