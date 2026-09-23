//! Accepted public score-event contract and decoding.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::error::Error;
use super::facts::PlaySide;

pub const EVENT_SCHEMA: &str = "scorepeek-event-v5";

pub(super) fn validate_v5_envelope(raw: &Value) -> Result<(), Error> {
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
    if raw["schema"].as_str() != Some(super::event::EVENT_SCHEMA)
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
    if capture.len() == 1
        && capture
            .get("session_id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.is_empty())
    {
        Ok(())
    } else {
        Err(Error::UnsupportedContract)
    }
}
#[derive(Deserialize)]
pub(super) struct Envelope {
    pub(super) schema: String,
    pub(super) invocation_id: String,
    pub(super) sequence: u64,
    pub(super) event_id: String,
    pub(super) emitted_monotonic_ms: u64,
    pub(super) emitted_unix_ms: i64,
    pub(super) capture: Value,
    #[serde(flatten)]
    pub(super) event: Event,
}
#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(super) enum Event {
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
pub(super) enum ResultChange {
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
pub(super) struct SongPresentation {
    pub(super) scorepeek_song_id: String,
    pub(super) display_titles: Vec<String>,
    pub(super) artist: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ResultRetractionReason {
    EvidenceUnresolved,
    PanelSideConflict,
    AttemptRejected,
    SessionEnded,
}

pub(super) fn validate_result_context(event: &Event, capture: &Value) -> Result<(), Error> {
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
pub(super) struct Chart {
    pub(super) scorepeek_song_id: String,
    pub(super) play_type: PlayType,
    pub(super) difficulty: Difficulty,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PlayType {
    Single,
    Double,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Difficulty {
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
pub(super) struct ResultData {
    pub(super) contract: String,
    pub(super) attempt_id: u64,
    #[serde(flatten)]
    pub(super) chart: Chart,
    pub(super) play_side: PlaySide,
    pub(super) play_mode: String,
    pub(super) level: u8,
    pub(super) notes: u32,
    pub(super) current_score: u32,
    pub(super) clear_type: String,
    pub(super) judgments: ResultJudgments,
    pub(super) miss_count: Field<u32>,
    pub(super) timing: ResultTiming,
    pub(super) combo_break: Supplemental<u32>,
    pub(super) previous_best: Previous,
    pub(super) play_options: PlayOptions,
}
#[allow(
    dead_code,
    reason = "fields make the complete public result shape mandatory"
)]
#[derive(Deserialize)]
pub(super) struct ResultJudgments {
    pub(super) pgreat: u32,
    pub(super) great: u32,
    pub(super) good: u32,
    pub(super) bad: u32,
    pub(super) poor: u32,
}
#[allow(
    dead_code,
    reason = "fields make the complete public result shape mandatory"
)]
#[derive(Deserialize)]
pub(super) struct ResultTiming {
    pub(super) fast: Supplemental<u32>,
    pub(super) slow: Supplemental<u32>,
}
#[allow(
    dead_code,
    reason = "variants validate the complete public field shape"
)]
#[derive(Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(super) enum Supplemental<T> {
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
pub(super) enum PlayOptions {
    Known { values: Vec<String> },
    Unknown { reason: String },
}
#[derive(Deserialize)]
pub(super) struct Previous {
    pub(super) score: Field<u32>,
    pub(super) miss_count: Field<u32>,
    pub(super) clear_type: Field<String>,
}
#[derive(Deserialize)]
pub(super) struct SelectData {
    pub(super) contract: String,
    pub(super) revision: u64,
    pub(super) observation_id: String,
    pub(super) chart: SelectChart,
    pub(super) values: SelectValues,
}
#[derive(Deserialize)]
pub(super) struct SelectChart {
    #[serde(flatten)]
    pub(super) chart: Chart,
    pub(super) play_side: PlaySide,
    pub(super) presentation: Value,
}
#[derive(Deserialize)]
pub(super) struct SelectValues {
    pub(super) score: Field<u32>,
    pub(super) miss_count: Field<u32>,
    pub(super) clear_type: Field<SelectClear>,
}
#[derive(Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub(super) enum Field<T> {
    Known(T),
    Unknown,
    NotDisplayed,
    NoRecord,
    NotPlayed,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SelectClear {
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
    pub(super) fn rank(&self) -> i64 {
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
pub(super) fn result_clear(value: &str) -> Result<i64, Error> {
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
pub(super) const STORED_RESULT_SCHEMA: &str = "scorepeek-stored-result-v2";

pub(super) fn stored_result_from_event(raw: &Value) -> Result<Value, Error> {
    if raw["schema"].as_str() != Some(super::event::EVENT_SCHEMA)
        || raw["event"].as_str() != Some("result_changed")
    {
        return Err(Error::UnsupportedContract);
    }
    let result = raw["state"]["result"].clone();
    serde_json::from_value::<ResultData>(result.clone())?;
    Ok(serde_json::json!({
        "schema": STORED_RESULT_SCHEMA,
        "event_id": raw["event_id"],
        "invocation_id": raw["invocation_id"],
        "sequence": raw["sequence"],
        "emitted_unix_ms": raw["emitted_unix_ms"],
        "capture": raw["capture"],
        "result": result,
    }))
}
