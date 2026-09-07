//! Persistence consumer for the public event v2 contract, independent of recognition.
pub mod query;
mod worker;
pub use worker::{ChartIdentity, Completion, CompletionOutcome, Health, Worker};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fmt, fs,
    path::{Path, PathBuf},
    time::Duration,
};

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

fn validate_v2_envelope(raw: &Value) -> Result<(), Error> {
    let object = raw.as_object().ok_or(Error::UnsupportedContract)?;
    for key in [
        "schema",
        "invocation_id",
        "sequence",
        "event_id",
        "emitted_monotonic_ms",
        "emitted_unix_ms",
        "capture",
        "event",
    ] {
        if !object.contains_key(key) {
            return Err(Error::UnsupportedContract);
        }
    }
    if raw["schema"].as_str() != Some("scorepeek-event-v2")
        || raw["invocation_id"].as_str().is_none()
        || raw["sequence"].as_u64().is_none()
        || raw["event_id"].as_str().is_none_or(str::is_empty)
        || raw["emitted_monotonic_ms"].as_u64().is_none()
        || raw["emitted_unix_ms"].as_i64().is_none()
        || raw["event"].as_str().is_none()
    {
        return Err(Error::UnsupportedContract);
    }
    let capture = &raw["capture"];
    if capture.is_null() {
        return Ok(());
    }
    let capture = capture.as_object().ok_or(Error::UnsupportedContract)?;
    if capture
        .get("session_id")
        .is_none_or(|value| !(value.is_null() || value.is_string()))
        || capture
            .get("capture_generation")
            .and_then(Value::as_u64)
            .is_none()
    {
        return Err(Error::UnsupportedContract);
    }
    let binding = capture.get("binding").ok_or(Error::UnsupportedContract)?;
    if binding.is_null() {
        return Ok(());
    }
    let binding = binding.as_object().ok_or(Error::UnsupportedContract)?;
    for key in [
        "capture_profile_sha256",
        "normalizer_sha256",
        "canonical_layout_sha256",
        "catalog_sha256",
        "model_sha256",
        "runtime_sha256",
    ] {
        if binding.get(key).and_then(Value::as_str).is_none() {
            return Err(Error::UnsupportedContract);
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Envelope {
    schema: String,
    invocation_id: String,
    sequence: u64,
    event_id: String,
    emitted_monotonic_ms: u64,
    emitted_unix_ms: i64,
    capture: Value,
    #[serde(flatten)]
    event: Event,
}
#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event {
    ResultChanged {
        #[serde(rename = "source_sequence")]
        _source_sequence: u64,
        state: ResultChange,
    },
    MusicSelectBestObserved {
        snapshot: Option<SelectData>,
    },
    MusicSelectionChanged,
    StatusChanged,
    #[serde(other)]
    Unknown,
}
#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ResultChange {
    Inactive,
    Provisional {
        result: Box<ResultData>,
        song: SongPresentation,
    },
    Retracted {
        result: Box<ResultData>,
        song: SongPresentation,
        reason: ResultRetractionReason,
    },
    Confirmed {
        result: Box<ResultData>,
        song: SongPresentation,
    },
}
#[derive(Deserialize, Serialize)]
struct SongPresentation {
    scorepeek_song_id: String,
    display_titles: Vec<String>,
    artist: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResultRetractionReason {
    EvidenceUnresolved,
    AttemptRejected,
    SessionEnded,
}

fn validate_result_context(event: &Event, capture: &Value) -> Result<(), Error> {
    let (result, song) = match event {
        Event::ResultChanged {
            state:
                ResultChange::Provisional { result, song }
                | ResultChange::Retracted { result, song, .. }
                | ResultChange::Confirmed { result, song },
            ..
        } => (result.as_ref(), song),
        _ => return Ok(()),
    };
    let _session_id = capture
        .as_object()
        .and_then(|value| value.get("session_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(Error::UnsupportedContract)?;
    if song.scorepeek_song_id != result.chart.scorepeek_song_id
        || song.display_titles.is_empty()
        || song.display_titles.iter().any(String::is_empty)
    {
        return Err(Error::UnsupportedContract);
    }
    Ok(())
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
#[allow(
    dead_code,
    reason = "fields make the complete public result shape mandatory"
)]
#[derive(Deserialize)]
struct ResultData {
    contract: String,
    attempt_id: u64,
    #[serde(flatten)]
    chart: Chart,
    play_side: String,
    play_mode: String,
    level: u8,
    notes: u32,
    current_score: u32,
    clear_type: String,
    judgments: ResultJudgments,
    miss_count: Field<u32>,
    timing: ResultTiming,
    combo_break: Supplemental<u32>,
    previous_best: Previous,
    play_options: PlayOptions,
}
#[allow(
    dead_code,
    reason = "fields make the complete public result shape mandatory"
)]
#[derive(Deserialize)]
struct ResultJudgments {
    pgreat: u32,
    great: u32,
    good: u32,
    bad: u32,
    poor: u32,
}
#[allow(
    dead_code,
    reason = "fields make the complete public result shape mandatory"
)]
#[derive(Deserialize)]
struct ResultTiming {
    fast: Supplemental<u32>,
    slow: Supplemental<u32>,
}
#[allow(
    dead_code,
    reason = "variants validate the complete public field shape"
)]
#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Supplemental<T> {
    Known { value: T },
    NotDisplayed,
    Unknown { reason: String },
}
#[allow(
    dead_code,
    reason = "variants validate the complete public option shape"
)]
#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum PlayOptions {
    Known { values: Vec<String> },
    Unknown { reason: String },
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResultMutation {
    Upsert(&'static str),
    Retract,
}
type Prepared<'a> = (
    &'a Chart,
    Option<Value>,
    [Fields; 2],
    Option<ResultMutation>,
    Option<u64>,
);
fn prepare_result<'a>(
    result: &'a ResultData,
    song: Option<&SongPresentation>,
    origin: &Origin,
    mutation: ResultMutation,
) -> Result<Prepared<'a>, Error> {
    if result.contract != "scorepeek-result-detected-v2" {
        return Err(Error::UnsupportedContract);
    }
    if let Some(song) = song
        && (song.scorepeek_song_id != result.chart.scorepeek_song_id
            || song.display_titles.is_empty()
            || song.display_titles.iter().any(String::is_empty))
    {
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
    Ok((
        &result.chart,
        song.map(serde_json::to_value).transpose()?,
        [
            current.map(|v| fact(v, Source::Result, origin)),
            previous.map(|v| fact(v, Source::PreviousBest, origin)),
        ],
        Some(mutation),
        Some(result.attempt_id),
    ))
}
fn prepare<'a>(event: &'a Event, origin: &mut Origin) -> Result<Option<Prepared<'a>>, Error> {
    let prepared = match event {
        Event::ResultChanged {
            state: ResultChange::Provisional { result, song },
            ..
        } => prepare_result(
            result,
            Some(song),
            origin,
            ResultMutation::Upsert("provisional"),
        )?,
        Event::ResultChanged {
            state: ResultChange::Confirmed { result, song },
            ..
        } => prepare_result(
            result,
            Some(song),
            origin,
            ResultMutation::Upsert("confirmed"),
        )?,
        Event::ResultChanged {
            state:
                ResultChange::Retracted {
                    result,
                    song,
                    reason,
                },
            ..
        } => {
            let _ = reason;
            prepare_result(result, Some(song), origin, ResultMutation::Retract)?
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
                None,
                None,
            )
        }
        _ => return Ok(None),
    };
    Ok(Some(prepared))
}

/// Synchronous database core. The host chooses the path and owns diagnostics.
pub struct Store {
    connection: Connection,
    _writer_lock: fs::File,
    cursor: Option<(String, u64)>,
    recovered_provisional_count: u64,
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
        let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if !(0..=2).contains(&version) {
            return Err(Error::UnsupportedDatabase(version));
        }
        if version == 0 {
            tx.execute_batch(&format!(
                "CREATE TABLE play_results (event_id TEXT PRIMARY KEY, session_id TEXT, attempt_id INTEGER, state TEXT NOT NULL, latest_event_id TEXT NOT NULL, recovery_confirmed INTEGER NOT NULL DEFAULT 0, song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, emitted_unix_ms INTEGER NOT NULL, received_unix_ms INTEGER NOT NULL, score INTEGER NOT NULL, miss INTEGER, clear INTEGER NOT NULL, event_json TEXT NOT NULL);\n\
                 CREATE INDEX plays_chart ON play_results(song_id,play_type,difficulty);\n\
                 CREATE UNIQUE INDEX plays_attempt ON play_results(session_id,attempt_id);\n\
                 CREATE TABLE result_attempt_origins(session_id TEXT NOT NULL, attempt_id INTEGER NOT NULL, event_id TEXT NOT NULL, emitted_unix_ms INTEGER NOT NULL, received_unix_ms INTEGER NOT NULL, PRIMARY KEY(session_id,attempt_id));\n\
                 CREATE TABLE chart_bests (song_id TEXT NOT NULL, play_type TEXT NOT NULL, difficulty TEXT NOT NULL, presentation TEXT, {}, score INTEGER, miss INTEGER, clear INTEGER, PRIMARY KEY(song_id,play_type,difficulty));\n\
                 PRAGMA user_version=2;",
                COLUMNS.iter().map(|column| format!("{column} TEXT")).collect::<Vec<_>>().join(",")
            ))?;
        }
        if version == 1 {
            tx.execute_batch(
                "ALTER TABLE play_results ADD COLUMN session_id TEXT;
                 ALTER TABLE play_results ADD COLUMN attempt_id INTEGER;
                 ALTER TABLE play_results ADD COLUMN state TEXT NOT NULL DEFAULT 'confirmed';
                 ALTER TABLE play_results ADD COLUMN latest_event_id TEXT;
                 ALTER TABLE play_results ADD COLUMN recovery_confirmed INTEGER NOT NULL DEFAULT 0;
                 UPDATE play_results SET latest_event_id=event_id WHERE latest_event_id IS NULL;
                 CREATE UNIQUE INDEX plays_attempt ON play_results(session_id,attempt_id);
                 CREATE TABLE result_attempt_origins(session_id TEXT NOT NULL, attempt_id INTEGER NOT NULL, event_id TEXT NOT NULL, emitted_unix_ms INTEGER NOT NULL, received_unix_ms INTEGER NOT NULL, PRIMARY KEY(session_id,attempt_id));
                 PRAGMA user_version=2;",
            )?;
        }
        let recovered_provisional_count = tx.execute(
            "UPDATE play_results SET state='confirmed', recovery_confirmed=1 WHERE state='provisional'",
            [],
        )? as u64;
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
            _writer_lock: writer_lock,
            cursor: None,
            recovered_provisional_count,
        })
    }

    #[must_use]
    pub fn recovered_provisional_count(&self) -> u64 {
        self.recovered_provisional_count
    }

    /// Applies one live event in producer order. Returns whether data was committed.
    /// # Errors
    /// Returns unsupported contract, parsing or transaction errors. No partial event is saved.
    pub fn consume(&mut self, bytes: &[u8], received_unix_ms: u64) -> Result<bool, Error> {
        let raw: Value = serde_json::from_slice(bytes)?;
        validate_v2_envelope(&raw)?;
        let envelope: Envelope = serde_json::from_slice(bytes)?;
        if envelope.schema != "scorepeek-event-v2" {
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
        let Some((chart, presentation, incoming, result_mutation, attempt_id)) =
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
                    received_unix_ms,
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

struct ResultWrite<'a> {
    envelope: &'a Envelope,
    raw: &'a Value,
    chart: &'a Chart,
    play_type: &'a str,
    difficulty: &'a str,
    incoming: &'a [Fields; 2],
    attempt_id: u64,
    received_unix_ms: u64,
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
            tx.execute(
                "INSERT INTO play_results(event_id,session_id,attempt_id,state,latest_event_id,recovery_confirmed,song_id,play_type,difficulty,emitted_unix_ms,received_unix_ms,score,miss,clear,event_json) VALUES (?1,?2,?3,?4,?14,0,?5,?6,?7,?8,?9,?10,?11,?12,?13) ON CONFLICT(session_id,attempt_id) DO UPDATE SET state=excluded.state,latest_event_id=excluded.latest_event_id,recovery_confirmed=0,song_id=excluded.song_id,play_type=excluded.play_type,difficulty=excluded.difficulty,score=excluded.score,miss=excluded.miss,clear=excluded.clear,event_json=excluded.event_json WHERE play_results.state != 'confirmed' OR excluded.state = 'confirmed'",
                params![first_event_id, session_id, attempt_id, state, write.chart.scorepeek_song_id, write.play_type, write.difficulty, first_emitted_unix_ms, first_received_unix_ms, write.incoming[0][0].as_ref().and_then(|f| f.value), write.incoming[0][1].as_ref().and_then(|f| f.value), write.incoming[0][2].as_ref().and_then(|f| f.value), serde_json::to_string(write.raw)?, write.envelope.event_id],
            )?;
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
        "SELECT event_json,received_unix_ms FROM play_results WHERE song_id=?1 AND play_type=?2 AND difficulty=?3 ORDER BY emitted_unix_ms,event_id",
    )?;
    let rows = statement.query_map(
        params![chart.scorepeek_song_id, play_type, difficulty],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    for row in rows {
        let (json, stored_received_ms) = row?;
        let [current, previous] = stored_result_facts(&json, stored_received_ms)?;
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

fn stored_result_facts(json: &str, received_unix_ms: i64) -> Result<[Fields; 2], Error> {
    let raw: Value = serde_json::from_str(json)?;
    let result = match (
        raw["schema"].as_str(),
        raw["event"].as_str(),
        raw["state"]["status"].as_str(),
    ) {
        (Some("scorepeek-event-v2"), Some("result_changed"), Some("provisional" | "confirmed")) => {
            serde_json::from_value::<ResultData>(raw["state"]["result"].clone())?
        }
        (Some("scorepeek-event-v1"), Some("result_detected"), _) => {
            serde_json::from_value::<ResultData>(raw["result"].clone())?
        }
        _ => return Err(Error::UnsupportedContract),
    };
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
    let (_, _, facts, _, _) =
        prepare_result(&result, None, &origin, ResultMutation::Upsert("confirmed"))?;
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
