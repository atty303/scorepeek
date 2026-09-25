//! Persistence consumer for the public event v5 contract, independent of recognition.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub use super::error::Error;
use super::event::{
    Chart, Envelope, stored_result_from_event, validate_result_context, validate_v5_envelope,
};
use super::facts::{
    COLUMNS, Fact, Fields, Origin, PlaySide, ResultMutation, cumulative, integrate, prepare,
};

const DATABASE_VERSION: i64 = super::migration::CURRENT_SCHEMA_VERSION;
/// Synchronous database core. The host chooses the path and owns diagnostics.
pub struct Store {
    connection: Connection,
    _writer_lock: fs::File,
    cursor: Option<(String, u64)>,
    recovered_provisional_count: u64,
    migration_unavailable_details: u64,
    migration_backup: Option<PathBuf>,
}
impl Store {
    /// Opens or creates a database without deleting existing data.
    /// # Errors
    /// Returns filesystem, `SQLite` or unsupported-version errors.
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            durable_directory(parent)?;
        }
        let writer_lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(writer_lock_path(path))?;
        writer_lock.try_lock().map_err(std::io::Error::from)?;
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(250))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let original_version: i64 =
            connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if !matches!(original_version, 0 | 3 | 4 | DATABASE_VERSION) {
            return Err(Error::UnsupportedDatabase(original_version));
        }
        let migration_backup = if matches!(original_version, 3 | 4) {
            Some(
                super::migration::backup_database(&connection, path, original_version)
                    .map_err(|error| Error::Migration("backup", Box::new(error)))?,
            )
        } else {
            None
        };
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|error| migration_error(original_version, "journal setup", error.into()))?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|error| migration_error(original_version, "durability setup", error.into()))?;
        let tx = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| {
                migration_error(original_version, "transaction start", error.into())
            })?;
        let version: i64 = tx
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|error| migration_error(original_version, "version check", error.into()))?;
        if version != original_version {
            return Err(migration_error(
                original_version,
                "version check",
                Error::UnsupportedDatabase(version),
            ));
        }
        let mut migration_unavailable_details = 0;
        if version == 0 {
            tx.execute_batch(&format!(
                "CREATE TABLE play_results (event_id TEXT PRIMARY KEY, session_id TEXT, attempt_id INTEGER, state TEXT NOT NULL, latest_event_id TEXT NOT NULL, recovery_confirmed INTEGER NOT NULL DEFAULT 0, song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, play_side TEXT NOT NULL, emitted_unix_ms INTEGER NOT NULL, received_unix_ms INTEGER NOT NULL, score INTEGER NOT NULL, miss INTEGER, clear INTEGER NOT NULL, event_json TEXT NOT NULL, detail_state TEXT NOT NULL DEFAULT 'available' CHECK(detail_state IN ('available','unavailable_at_migration')));\n\
                 CREATE INDEX plays_chart ON play_results(song_id,play_type,difficulty);\n\
                 CREATE UNIQUE INDEX plays_attempt ON play_results(session_id,attempt_id);\n\
                 CREATE TABLE result_attempt_origins(session_id TEXT NOT NULL, attempt_id INTEGER NOT NULL, event_id TEXT NOT NULL, emitted_unix_ms INTEGER NOT NULL, received_unix_ms INTEGER NOT NULL, PRIMARY KEY(session_id,attempt_id));\n\
                 CREATE TABLE chart_bests (song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, presentation TEXT, {}, score INTEGER, miss INTEGER, clear INTEGER, PRIMARY KEY(song_id,play_type,difficulty));\n\
                 {}\n\
                 CREATE TABLE play_result_facts(event_id TEXT NOT NULL,source INTEGER NOT NULL,field INTEGER NOT NULL,fact TEXT NOT NULL,PRIMARY KEY(event_id,source,field),FOREIGN KEY(event_id) REFERENCES play_results(event_id) ON DELETE CASCADE);\n\
                 PRAGMA user_version=5;",
                COLUMNS.iter().map(|column| format!("{column} TEXT")).collect::<Vec<_>>().join(","), super::structured::SCHEMA
            ))?;
        } else if matches!(version, 3 | 4) {
            if version == 3 {
                super::migration::migrate_database_v3_to_v4(&tx)
                    .map_err(|error| Error::Migration("v3 conversion", Box::new(error)))?;
            }
            migration_unavailable_details = super::migration::migrate_database_v4_to_v5(&tx)
                .map_err(|error| Error::Migration("structured conversion", Box::new(error)))?;
        }
        let recovered_provisional_count = tx.execute(
            "UPDATE play_results SET state='confirmed', recovery_confirmed=1 WHERE state='provisional'",
            [],
        ).map_err(|error| migration_error(original_version, "provisional recovery", error.into()))? as u64;
        tx.commit()
            .map_err(|error| migration_error(original_version, "commit", error.into()))?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| migration_error(original_version, "directory sync", error.into()))?;
        Ok(Self {
            connection,
            _writer_lock: writer_lock,
            cursor: None,
            recovered_provisional_count,
            migration_unavailable_details,
            migration_backup,
        })
    }

    #[must_use]
    pub fn recovered_provisional_count(&self) -> u64 {
        self.recovered_provisional_count
    }

    #[must_use]
    pub fn migration_unavailable_details(&self) -> u64 {
        self.migration_unavailable_details
    }

    #[must_use]
    pub fn migration_backup(&self) -> Option<&Path> {
        self.migration_backup.as_deref()
    }

    /// Applies one live event in producer order. Returns whether data was committed.
    /// # Errors
    /// Returns unsupported contract, parsing or transaction errors. No partial event is saved.
    pub fn consume(&mut self, bytes: &[u8], received_unix_ms: u64) -> Result<bool, Error> {
        let raw: Value = serde_json::from_slice(bytes)?;
        validate_v5_envelope(&raw)?;
        let envelope: Envelope = serde_json::from_slice(bytes)?;
        if envelope.schema != super::event::EVENT_SCHEMA {
            return Err(Error::UnsupportedContract);
        }
        validate_result_context(&envelope.event, &envelope.capture)?;
        if self.cursor.as_ref().is_some_and(|(id, sequence)| {
            id == &envelope.invocation_id && *sequence >= envelope.sequence
        }) {
            return Ok(false);
        }
        let _ = envelope.emitted_monotonic_ms;
        let mut origin = Origin {
            event_id: envelope.event_id.clone(),
            invocation_id: envelope.invocation_id.clone(),
            sequence: envelope.sequence,
            emitted_unix_ms: envelope.emitted_unix_ms,
            received_unix_ms,
            revision: None,
            observation_id: None,
            capture: envelope.capture.clone(),
        };
        let Some((chart, presentation, incoming, result_mutation, attempt_id, result_play_side)) =
            prepare(&envelope.event, &mut origin)?
        else {
            self.cursor = Some((envelope.invocation_id, envelope.sequence));
            return Ok(false);
        };
        let play_type = serde_json::to_value(&chart.play_type)?
            .as_str()
            .ok_or(Error::UnsupportedContract)?
            .to_owned();
        let difficulty = serde_json::to_value(&chart.difficulty)?
            .as_str()
            .ok_or(Error::UnsupportedContract)?
            .to_owned();
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(mutation) = result_mutation {
            apply_result_mutation(
                &tx,
                mutation,
                &ResultWrite {
                    envelope: &envelope,
                    raw: &raw,
                    chart,
                    play_type: &play_type,
                    difficulty: &difficulty,
                    incoming: &incoming,
                    attempt_id: attempt_id.ok_or(Error::UnsupportedContract)?,
                    play_side: result_play_side.ok_or(Error::UnsupportedContract)?,
                    received_unix_ms,
                    stored_result: stored_result_from_event(&raw)?,
                },
            )?;
        }
        let query = format!(
            "SELECT {} FROM chart_bests WHERE song_id=?1 AND play_type=?2 AND difficulty=?3",
            COLUMNS.join(",")
        );
        let saved: Option<Vec<Option<String>>> = tx
            .query_row(
                &query,
                params![chart.scorepeek_song_id, play_type, difficulty],
                |row| (0..12).map(|index| row.get(index)).collect(),
            )
            .optional()?;
        let mut facts: [Option<Fact>; 12] = std::array::from_fn(|_| None);
        if let Some(saved) = saved {
            for (index, value) in saved.into_iter().enumerate() {
                facts[index] = value.map(|v| serde_json::from_str(&v)).transpose()?;
            }
        }
        let [first, _] = incoming;
        if result_mutation.is_some() {
            facts[0..6].fill(None);
            recompute_result_facts(&tx, chart, &play_type, &difficulty, &mut facts)?;
        } else {
            for (index, value) in first.into_iter().enumerate() {
                if value.is_some() {
                    facts[6 + index] = value;
                }
            }
        }
        integrate(&mut facts);
        tx.execute("INSERT INTO chart_bests(song_id,play_type,difficulty,presentation) VALUES (?1,?2,?3,?4) ON CONFLICT(song_id,play_type,difficulty) DO UPDATE SET presentation=COALESCE(excluded.presentation,chart_bests.presentation)", params![chart.scorepeek_song_id,play_type,difficulty,presentation.map(|v| serde_json::to_string(&v)).transpose()?])?;
        for (column, value) in COLUMNS.iter().zip(&facts) {
            tx.execute(&format!("UPDATE chart_bests SET {column}=?4 WHERE song_id=?1 AND play_type=?2 AND difficulty=?3"), params![chart.scorepeek_song_id,play_type,difficulty,value.as_ref().map(serde_json::to_string).transpose()?])?;
        }
        tx.execute("UPDATE chart_bests SET score=?4,miss=?5,clear=?6 WHERE song_id=?1 AND play_type=?2 AND difficulty=?3",params![chart.scorepeek_song_id,play_type,difficulty,facts[9].as_ref().and_then(|f|f.value),facts[10].as_ref().and_then(|f|f.value),facts[11].as_ref().and_then(|f|f.value)])?;
        tx.commit()?;
        self.cursor = Some((envelope.invocation_id, envelope.sequence));
        Ok(true)
    }
}

fn migration_error(version: i64, stage: &'static str, cause: Error) -> Error {
    if matches!(version, 3 | 4) {
        Error::Migration(stage, Box::new(cause))
    } else {
        cause
    }
}

struct ResultWrite<'a> {
    envelope: &'a Envelope,
    raw: &'a Value,
    chart: &'a Chart,
    play_type: &'a str,
    difficulty: &'a str,
    incoming: &'a [Fields; 2],
    attempt_id: u64,
    play_side: PlaySide,
    received_unix_ms: u64,
    stored_result: Value,
}

fn apply_result_mutation(
    tx: &Transaction<'_>,
    mutation: ResultMutation,
    write: &ResultWrite<'_>,
) -> Result<(), Error> {
    let session_id = write.raw["capture"]["session_id"]
        .as_str()
        .ok_or(Error::UnsupportedContract)?;
    let attempt_id = i64::try_from(write.attempt_id).map_err(|_| Error::UnsupportedContract)?;
    match mutation {
        ResultMutation::Upsert(state) => {
            let received_unix_ms =
                i64::try_from(write.received_unix_ms).map_err(|_| Error::UnsupportedContract)?;
            tx.execute(
                "INSERT OR IGNORE INTO result_attempt_origins VALUES (?1,?2,?3,?4,?5)",
                params![
                    session_id,
                    attempt_id,
                    write.envelope.event_id,
                    write.envelope.emitted_unix_ms,
                    received_unix_ms
                ],
            )?;
            let (first_event_id, first_emitted_unix_ms, first_received_unix_ms):
                (String, i64, i64) = tx.query_row(
                    "SELECT event_id,emitted_unix_ms,received_unix_ms FROM result_attempt_origins WHERE session_id=?1 AND attempt_id=?2",
                    params![session_id, attempt_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            let changed = tx.execute(
                "INSERT INTO play_results(event_id,session_id,attempt_id,state,latest_event_id,recovery_confirmed,song_id,play_type,difficulty,play_side,emitted_unix_ms,received_unix_ms,score,miss,clear,event_json) VALUES (?1,?2,?3,?4,?15,0,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14) ON CONFLICT(session_id,attempt_id) DO UPDATE SET state=excluded.state,latest_event_id=excluded.latest_event_id,recovery_confirmed=0,song_id=excluded.song_id,play_type=excluded.play_type,difficulty=excluded.difficulty,play_side=excluded.play_side,score=excluded.score,miss=excluded.miss,clear=excluded.clear,event_json=excluded.event_json,detail_state='available' WHERE play_results.state != 'confirmed' OR excluded.state = 'confirmed'",
                params![first_event_id, session_id, attempt_id, state, write.chart.scorepeek_song_id, write.play_type, write.difficulty, write.play_side.as_str(), first_emitted_unix_ms, first_received_unix_ms, write.incoming[0][0].as_ref().and_then(|f| f.value), write.incoming[0][1].as_ref().and_then(|f| f.value), write.incoming[0][2].as_ref().and_then(|f| f.value), serde_json::to_string(&write.stored_result)?, write.envelope.event_id],
            )?;
            if changed != 0 {
                super::migration::save_facts(tx, &first_event_id, write.incoming)?;
                super::structured::save(tx, &first_event_id, &write.stored_result["result"])?;
            }
        }
        ResultMutation::Retract => {
            tx.execute(
                "DELETE FROM play_results WHERE session_id=?1 AND attempt_id=?2 AND state!='confirmed'",
                params![session_id, attempt_id],
            )?;
        }
    }
    Ok(())
}

fn recompute_result_facts(
    tx: &Transaction<'_>,
    chart: &Chart,
    play_type: &str,
    difficulty: &str,
    facts: &mut [Option<Fact>; 12],
) -> Result<(), Error> {
    let mut statement = tx.prepare(
        "SELECT event_id FROM play_results WHERE song_id=?1 AND play_type=?2 AND difficulty=?3 ORDER BY emitted_unix_ms,event_id",
    )?;
    let rows = statement.query_map(
        params![chart.scorepeek_song_id, play_type, difficulty],
        |row| row.get::<_, String>(0),
    )?;
    for row in rows {
        let [current, previous] = structured_result_facts(tx, &row?)?;
        cumulative(&mut facts[0..3], current);
        cumulative(&mut facts[3..6], previous);
    }
    Ok(())
}

fn writer_lock_path(path: &Path) -> PathBuf {
    let mut lock_path = path.as_os_str().to_os_string();
    lock_path.push(".writer.lock");
    PathBuf::from(lock_path)
}

fn structured_result_facts(tx: &Transaction<'_>, event_id: &str) -> Result<[Fields; 2], Error> {
    let mut facts: [Fields; 2] = std::array::from_fn(|_| std::array::from_fn(|_| None));
    let mut statement =
        tx.prepare("SELECT source,field,fact FROM play_result_facts WHERE event_id=?1")?;
    let rows = statement.query_map([event_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (source, field, json) = row?;
        if !(0..=1).contains(&source) || !(0..=2).contains(&field) {
            return Err(Error::UnsupportedContract);
        }
        facts[usize::try_from(source).map_err(|_| Error::UnsupportedContract)?]
            [usize::try_from(field).map_err(|_| Error::UnsupportedContract)?] =
            Some(serde_json::from_str(&json)?);
    }
    Ok(facts)
}
fn durable_directory(path: &Path) -> Result<(), std::io::Error> {
    if path.is_dir() {
        return Ok(());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    durable_directory(parent)?;
    match fs::create_dir(path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => (),
        Err(e) => return Err(e),
    }
    fs::File::open(path)?.sync_all()?;
    fs::File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests;
