//! Bounded decoding of recognition observations used by corpus evaluation.

use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::Path;

use scorepeek_core::replay::ScorepeekSongId;
use scorepeek_core::replay::ScreenClass;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{CorpusError, encode_digest};

use super::session::CaptureSession;

pub(super) const OBSERVATION_SCHEMA: &str = "scorepeek-private-corpus-observation-v1";
const MAX_OBSERVATION_BYTES: u64 = 512 * 1024 * 1024;
const MAX_OBSERVATION_RECORD_BYTES: usize = 1024 * 1024;
const MAX_OBSERVATIONS: usize = 250_000;

#[derive(Clone, Debug)]
pub(super) struct TemporalRecord {
    pub(super) sequence: u64,
    pub(super) timestamp_ms: u64,
    pub(super) screen: ScreenClass,
    pub(super) song: Option<ScorepeekSongId>,
    pub(super) clear_type: Option<String>,
    pub(super) has_result_observation: bool,
}

pub(super) fn read_observations(
    store: &Path,
    session: &CaptureSession,
) -> Result<Vec<TemporalRecord>, CorpusError> {
    let artifact = session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == "analysis/observations.ndjson")
        .ok_or_else(|| {
            CorpusError::InvalidReplay(
                "temporal evaluation observation artifact is unavailable".to_owned(),
            )
        })?;
    if artifact.bytes == 0
        || artifact.bytes > MAX_OBSERVATION_BYTES
        || !valid_sha256(&artifact.sha256)
    {
        return invalid("temporal evaluation observation artifact binding is invalid");
    }
    let path = store.join("objects").join(&artifact.sha256);
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() != artifact.bytes {
        return invalid("temporal evaluation observation artifact size differs");
    }
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    let mut records = Vec::new();
    let mut hasher = Sha256::new();
    while records.len() < MAX_OBSERVATIONS && read_line(&mut reader, &mut line)? {
        hasher.update(&line);
        let value: Value = serde_json::from_slice(&line)?;
        records.push(parse_record(&value)?);
    }
    if read_line(&mut reader, &mut line)? {
        return invalid("temporal evaluation observation count exceeds its bound");
    }
    if encode_digest(hasher.finalize()) != artifact.sha256 {
        return invalid("temporal evaluation observation artifact digest differs");
    }
    if records.is_empty()
        || records
            .windows(2)
            .any(|pair| pair[0].sequence >= pair[1].sequence)
    {
        return invalid("temporal evaluation observation order is invalid");
    }
    Ok(records)
}

fn read_line(reader: &mut BufReader<File>, line: &mut Vec<u8>) -> Result<bool, CorpusError> {
    line.clear();
    let read = reader
        .take(u64::try_from(MAX_OBSERVATION_RECORD_BYTES).unwrap_or(u64::MAX) + 1)
        .read_until(b'\n', line)?;
    if read == 0 {
        return Ok(false);
    }
    if read > MAX_OBSERVATION_RECORD_BYTES || line.last() != Some(&b'\n') {
        return invalid("temporal evaluation observation record exceeds its bound");
    }
    Ok(true)
}

pub(super) fn parse_record(value: &Value) -> Result<TemporalRecord, CorpusError> {
    if value["schema"] != OBSERVATION_SCHEMA {
        return invalid("temporal evaluation observation schema differs");
    }
    let sequence = value["tick_sequence"].as_u64().ok_or_else(|| {
        CorpusError::InvalidReplay("temporal observation sequence is invalid".to_owned())
    })?;
    let timestamp_ms = value["source_timestamp_ms"].as_u64().ok_or_else(|| {
        CorpusError::InvalidReplay("temporal observation timestamp is invalid".to_owned())
    })?;
    let screen_name = value["screen"].as_str().ok_or_else(|| {
        CorpusError::InvalidReplay("temporal observation screen is invalid".to_owned())
    })?;
    let screen = match screen_name {
        "result" => ScreenClass::Result,
        "music_select" => ScreenClass::MusicSelect,
        "mode_select" => ScreenClass::ModeSelect,
        "decide_transition" => ScreenClass::DecideTransition,
        "play" => ScreenClass::Play,
        "unknown" => ScreenClass::Unknown,
        _ => return invalid("temporal observation screen is unsupported"),
    };
    let has_result_observation = screen == ScreenClass::Result
        && (value.get("song_id").and_then(Value::as_str).is_some()
            || value.pointer("/fields/clear_type").is_some());
    let song = if has_result_observation {
        accepted_song(value)?
    } else {
        None
    };
    let clear_type = if has_result_observation {
        observed_clear_type(value)
    } else {
        None
    };
    Ok(TemporalRecord {
        sequence,
        timestamp_ms,
        screen,
        song,
        clear_type,
        has_result_observation,
    })
}

fn accepted_song(value: &Value) -> Result<Option<ScorepeekSongId>, CorpusError> {
    let accepted = value
        .pointer("/decision/resolution/status")
        .and_then(Value::as_str);
    let song = if accepted == Some("accepted") {
        value.pointer("/decision/resolution/selected/song_id")
    } else if value.get("decision").is_none() {
        value.get("song_id")
    } else {
        None
    };
    let Some(song) = song.and_then(Value::as_str) else {
        return Ok(None);
    };
    serde_json::from_value(Value::String(song.to_owned()))
        .map(Some)
        .map_err(|_| CorpusError::InvalidReplay("observed song ID is invalid".to_owned()))
}

fn observed_clear_type(value: &Value) -> Option<String> {
    value
        .pointer("/fields/clear_type")
        .and_then(|clear| {
            clear
                .as_str()
                .or_else(|| clear.get("open_text").and_then(Value::as_str))
        })
        .and_then(scorepeek_core::replay::resolve_clear_type)
        .map(ToOwned::to_owned)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn invalid<T>(detail: &str) -> Result<T, CorpusError> {
    Err(CorpusError::InvalidReplay(detail.to_owned()))
}
