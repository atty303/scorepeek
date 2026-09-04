//! Persistence consumer for the public event v1 contract, independent of recognition.
pub mod query;
mod worker;
pub use worker::{Health, Worker};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, fs, path::Path, time::Duration};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Sql(rusqlite::Error),
    Json(serde_json::Error),
    UnsupportedContract,
    UnsupportedDatabase(i64),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "scores filesystem: {e}"),
            Self::Sql(e) => write!(f, "scores database: {e}"),
            Self::Json(e) => write!(f, "scores event: {e}"),
            Self::UnsupportedContract => f.write_str("unsupported scores event contract"),
            Self::UnsupportedDatabase(v) => write!(f, "unsupported scores database version {v}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sql(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

#[derive(Deserialize)]
struct Envelope {
    schema: String,
    invocation_id: String,
    sequence: u64,
    event_id: String,
    emitted_unix_ms: i64,
    #[serde(flatten)]
    event: Event,
}
#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    ResultDetected {
        result: ResultData,
        song: Option<Value>,
    },
    MusicSelectBestObserved {
        snapshot: Option<SelectData>,
    },
    ResultProvisionalChanged,
    MusicSelectionChanged,
    StatusChanged,
}
#[derive(Deserialize)]
struct Chart {
    scorepeek_song_id: String,
    play_type: PlayType,
    difficulty: Difficulty,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum PlayType {
    Single,
    Double,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Difficulty {
    Beginner,
    Normal,
    Hyper,
    Another,
    Leggendaria,
}
#[derive(Deserialize)]
struct ResultData {
    contract: String,
    #[serde(flatten)]
    chart: Chart,
    current_score: u32,
    clear_type: String,
    miss_count: Field<u32>,
    previous_best: Previous,
}
#[derive(Deserialize)]
struct Previous {
    score: Field<u32>,
    miss_count: Field<u32>,
    clear_type: Field<String>,
}
#[derive(Deserialize)]
struct SelectData {
    contract: String,
    revision: u64,
    observation_id: String,
    chart: SelectChart,
    values: SelectValues,
}
#[derive(Deserialize)]
struct SelectChart {
    #[serde(flatten)]
    chart: Chart,
    presentation: Value,
}
#[derive(Deserialize)]
struct SelectValues {
    score: Field<u32>,
    miss_count: Field<u32>,
    clear_type: Field<SelectClear>,
}
#[derive(Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
enum Field<T> {
    Known(T),
    Unknown,
    NotDisplayed,
    NoRecord,
    NotPlayed,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SelectClear {
    NoPlay,
    Failed,
    AssistClear,
    EasyClear,
    Clear,
    HardClear,
    ExHardClear,
    FullCombo,
}
impl SelectClear {
    fn rank(&self) -> i64 {
        match self {
            Self::NoPlay => 0,
            Self::Failed => 1,
            Self::AssistClear => 2,
            Self::EasyClear => 3,
            Self::Clear => 4,
            Self::HardClear => 5,
            Self::ExHardClear => 6,
            Self::FullCombo => 7,
        }
    }
}
fn result_clear(value: &str) -> Result<i64, Error> {
    match value {
        "NO PLAY" => Ok(0),
        "FAILED" => Ok(1),
        "ASSIST CLEAR" => Ok(2),
        "EASY CLEAR" => Ok(3),
        "CLEAR" => Ok(4),
        "HARD CLEAR" => Ok(5),
        "EXH-CLEAR" => Ok(6),
        "F-COMBO" => Ok(7),
        _ => Err(Error::UnsupportedContract),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Origin {
    event_id: String,
    invocation_id: String,
    sequence: u64,
    emitted_unix_ms: i64,
    received_unix_ms: u64,
    revision: Option<u64>,
    observation_id: Option<String>,
    capture: Value,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Source {
    Result,
    PreviousBest,
    Select,
}
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Fact {
    // None is explicit no_record, distinct from an absent observation.
    value: Option<i64>,
    source: Source,
    origin: Origin,
}
type Fields = [Option<Fact>; 3];
const COLUMNS: [&str; 12] = [
    "result_score",
    "result_miss",
    "result_clear",
    "previous_score",
    "previous_miss",
    "previous_clear",
    "select_score",
    "select_miss",
    "select_clear",
    "score_origin",
    "miss_origin",
    "clear_origin",
];
fn better(field: usize, left: i64, right: i64) -> bool {
    if field == 1 {
        left < right
    } else {
        left > right
    }
}
fn cumulative(existing: &mut [Option<Fact>], incoming: Fields) {
    for (field, fact) in incoming.into_iter().enumerate() {
        if let Some(fact) = fact
            && let Some(value) = fact.value
            && existing[field]
                .as_ref()
                .and_then(|f| f.value)
                .is_none_or(|old| better(field, value, old))
        {
            existing[field] = Some(fact);
        }
    }
}
fn integrate(facts: &mut [Option<Fact>; 12]) {
    for field in 0..3 {
        let mut best: Option<Fact> = None;
        for source in 0..3 {
            if let Some(candidate) = &facts[source * 3 + field]
                && let Some(value) = candidate.value
                && best
                    .as_ref()
                    .and_then(|b| b.value)
                    .is_none_or(|old| better(field, value, old))
            {
                best = Some(candidate.clone());
            }
        }
        if let Some(old) = &facts[9 + field]
            && old.value == best.as_ref().and_then(|f| f.value)
            && (0..3).any(|source| facts[source * 3 + field].as_ref() == Some(old))
        {
            continue;
        }
        facts[9 + field] = best;
    }
}
fn known<T>(
    field: &Field<T>,
    convert: impl FnOnce(&T) -> Result<i64, Error>,
) -> Result<Option<i64>, Error> {
    match field {
        Field::Known(value) => convert(value).map(Some),
        _ => Ok(None),
    }
}
fn fact(value: Option<i64>, source: Source, origin: &Origin) -> Option<Fact> {
    value.map(|value| Fact {
        value: Some(value),
        source,
        origin: origin.clone(),
    })
}
fn select_fact<T>(
    field: &Field<T>,
    convert: impl FnOnce(&T) -> i64,
    origin: &Origin,
) -> Option<Fact> {
    match field {
        Field::Known(value) => fact(Some(convert(value)), Source::Select, origin),
        Field::NoRecord => Some(Fact {
            value: None,
            source: Source::Select,
            origin: origin.clone(),
        }),
        _ => None,
    }
}

type Prepared<'a> = (&'a Chart, Option<Value>, [Fields; 2], bool);
fn prepare<'a>(event: &'a Event, origin: &mut Origin) -> Result<Option<Prepared<'a>>, Error> {
    let prepared = match event {
        Event::ResultDetected { result, song } => {
            if result.contract != "scorepeek-result-detected-v2" {
                return Err(Error::UnsupportedContract);
            }
            let current = [
                Some(i64::from(result.current_score)),
                known(&result.miss_count, |v| Ok(i64::from(*v)))?,
                Some(result_clear(&result.clear_type)?),
            ];
            let previous = [
                known(&result.previous_best.score, |v| Ok(i64::from(*v)))?,
                known(&result.previous_best.miss_count, |v| Ok(i64::from(*v)))?,
                known(&result.previous_best.clear_type, |v| result_clear(v))?,
            ];
            (
                &result.chart,
                song.clone(),
                [
                    current.map(|v| fact(v, Source::Result, origin)),
                    previous.map(|v| fact(v, Source::PreviousBest, origin)),
                ],
                true,
            )
        }
        Event::MusicSelectBestObserved {
            snapshot: Some(snapshot),
        } => {
            if snapshot.contract != "scorepeek-music-select-best-snapshot-v1" {
                return Err(Error::UnsupportedContract);
            }
            origin.revision = Some(snapshot.revision);
            origin.observation_id = Some(snapshot.observation_id.clone());
            let values = &snapshot.values;
            let fields = [
                select_fact(&values.score, |v| i64::from(*v), origin),
                select_fact(&values.miss_count, |v| i64::from(*v), origin),
                select_fact(&values.clear_type, SelectClear::rank, origin),
            ];
            (
                &snapshot.chart.chart,
                Some(snapshot.chart.presentation.clone()),
                [fields, [None, None, None]],
                false,
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(prepared))
}

/// Synchronous database core. The host chooses the path and owns diagnostics.
pub struct Store {
    connection: Connection,
    cursor: Option<(String, u64)>,
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
        let mut connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_millis(250))?;
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != 0 && version != 1 {
            return Err(Error::UnsupportedDatabase(version));
        }
        if version == 0 {
            tx.execute_batch(&format!(
                "CREATE TABLE play_results (event_id TEXT PRIMARY KEY, song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, emitted_unix_ms INTEGER NOT NULL, received_unix_ms INTEGER NOT NULL, score INTEGER NOT NULL, miss INTEGER, clear INTEGER NOT NULL, event_json TEXT NOT NULL);\n\
                 CREATE INDEX plays_chart ON play_results(song_id,play_type,difficulty);\n\
                 CREATE TABLE chart_bests (song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, presentation TEXT, {}, score INTEGER, miss INTEGER, clear INTEGER, PRIMARY KEY(song_id,play_type,difficulty));\n\
                 PRAGMA user_version=1;",
                COLUMNS.iter().map(|column| format!("{column} TEXT")).collect::<Vec<_>>().join(",")
            ))?;
        }
        tx.commit()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        fs::File::open(parent)?.sync_all()?;
        Ok(Self {
            connection,
            cursor: None,
        })
    }

    /// Applies one live event in producer order. Returns whether data was committed.
    /// # Errors
    /// Returns unsupported contract, parsing or transaction errors. No partial event is saved.
    pub fn consume(&mut self, bytes: &[u8], received_unix_ms: u64) -> Result<bool, Error> {
        let envelope: Envelope = serde_json::from_slice(bytes)?;
        if envelope.schema != "scorepeek-event-v1" {
            return Err(Error::UnsupportedContract);
        }
        if self.cursor.as_ref().is_some_and(|(id, sequence)| {
            id == &envelope.invocation_id && *sequence >= envelope.sequence
        }) {
            return Ok(false);
        }
        let raw: Value = serde_json::from_slice(bytes)?;
        let mut origin = Origin {
            event_id: envelope.event_id.clone(),
            invocation_id: envelope.invocation_id.clone(),
            sequence: envelope.sequence,
            emitted_unix_ms: envelope.emitted_unix_ms,
            received_unix_ms,
            revision: None,
            observation_id: None,
            capture: raw["capture"].clone(),
        };
        let Some((chart, presentation, incoming, is_result)) =
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
        if is_result {
            let changed = tx.execute(
                "INSERT OR IGNORE INTO play_results VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    envelope.event_id,
                    chart.scorepeek_song_id,
                    play_type,
                    difficulty,
                    envelope.emitted_unix_ms,
                    i64::try_from(received_unix_ms).map_err(|_| Error::UnsupportedContract)?,
                    incoming[0][0].as_ref().and_then(|f| f.value),
                    incoming[0][1].as_ref().and_then(|f| f.value),
                    incoming[0][2].as_ref().and_then(|f| f.value),
                    serde_json::to_string(&raw)?
                ],
            )?;
            if changed == 0 {
                tx.commit()?;
                self.cursor = Some((envelope.invocation_id, envelope.sequence));
                return Ok(false);
            }
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
        let [first, second] = incoming;
        if is_result {
            cumulative(&mut facts[0..3], first);
            cumulative(&mut facts[3..6], second);
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
