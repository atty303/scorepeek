//! Self-contained canonical input contract shared by the live writer and corpus reader.

use serde::{Deserialize, Serialize};

use crate::frame::{CANONICAL_FRAME_CONTRACT_ID, CANONICAL_HEIGHT, CANONICAL_WIDTH};
use crate::game_version::GameVersionState;
use crate::recognition::screen::ScreenClass;

pub const RECORDING_SCHEMA: &str = "scorepeek-canonical-session-recording-v5";
pub const TICK_INDEX_NAME: &str = "canonical-ticks.ndjson";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalShape {
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
}

impl CanonicalShape {
    #[must_use]
    pub const fn fixed() -> Self {
        Self {
            width: CANONICAL_WIDTH,
            height: CANONICAL_HEIGHT,
            pixel_format: PixelFormat::Rgb8,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PixelFormat {
    Rgb8,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Completion {
    Complete,
    Partial,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncompleteReason {
    FrameLoss,
    EncoderFailure,
    ShutdownTimeout,
    MemoryLimit,
    TickIndexFailure,
    NoCanonicalTicks,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TickIndexArtifact {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub count: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentArtifact {
    pub path: String,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub frames: u64,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalRecordingManifest {
    pub schema: String,
    pub frame_contract: String,
    pub session_id: String,
    pub shape: CanonicalShape,
    pub tick_index: TickIndexArtifact,
    pub tick_count: u64,
    pub segments: Vec<SegmentArtifact>,
    pub completeness: Completion,
    pub completeness_reasons: Vec<IncompleteReason>,
    pub game_version: GameVersionState,
}

impl CanonicalRecordingManifest {
    /// Checks the immutable contract before a reader opens any referenced byte stream.
    ///
    /// # Errors
    /// Returns the first invalid structural field. Byte integrity and tick chronology are
    /// verified by the consuming artifact reader.
    pub fn validate_structure(&self) -> Result<(), &'static str> {
        if self.schema != RECORDING_SCHEMA || self.frame_contract != CANONICAL_FRAME_CONTRACT_ID {
            return Err("unsupported canonical recording contract");
        }
        if self.shape != CanonicalShape::fixed() {
            return Err("invalid canonical frame shape");
        }
        if self.session_id.is_empty()
            || self.session_id.len() > 128
            || !self
                .session_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("invalid session-local ID");
        }
        if self.completeness != Completion::Complete || !self.completeness_reasons.is_empty() {
            return Err("canonical recording is incomplete");
        }
        if self.tick_count == 0
            || self.tick_index.count != self.tick_count
            || self.tick_index.path != TICK_INDEX_NAME
            || self.tick_index.bytes == 0
            || !valid_sha256(&self.tick_index.sha256)
        {
            return Err("invalid canonical tick index");
        }
        if self.segments.is_empty() || self.segments.len() > 20_000 {
            return Err("invalid canonical segment count");
        }
        let mut previous_last = None;
        for (index, segment) in self.segments.iter().enumerate() {
            if segment.path != format!("segment-{index:04}.mkv")
                || segment.frames == 0
                || segment.frames > 600
                || segment.bytes == 0
                || segment.first_sequence > segment.last_sequence
                || previous_last.is_some_and(|last| segment.first_sequence <= last)
                || !valid_sha256(&segment.sha256)
            {
                return Err("invalid canonical segment");
            }
            previous_last = Some(segment.last_sequence);
        }
        Ok(())
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalTick {
    pub sequence: u64,
    pub source_sequence: u64,
    pub source_timestamp_ms: u64,
    pub screen: ScreenClass,
    pub semantic_episode_id: Option<u64>,
    pub disposition: TickDisposition,
}

impl CanonicalTick {
    /// # Errors
    /// Rejects an intentional elision reason that contradicts the observed screen.
    pub fn validate(&self) -> Result<(), &'static str> {
        let matches_screen = match self.disposition {
            TickDisposition::Retained
            | TickDisposition::Elided(ElisionReason::RecordingFailure) => true,
            TickDisposition::Elided(ElisionReason::Title) => self.screen == ScreenClass::Title,
            TickDisposition::Elided(ElisionReason::PlayInterior) => {
                self.screen == ScreenClass::Play
            }
            TickDisposition::Elided(ElisionReason::ModeSelectInterior) => {
                self.screen == ScreenClass::ModeSelect
            }
            TickDisposition::Elided(ElisionReason::UnknownInterior) => {
                self.screen == ScreenClass::Unknown
            }
        };
        matches_screen
            .then_some(())
            .ok_or("canonical elision reason contradicts screen")
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "reason", rename_all = "snake_case")]
pub enum TickDisposition {
    Retained,
    Elided(ElisionReason),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ElisionReason {
    Title,
    PlayInterior,
    ModeSelectInterior,
    UnknownInterior,
    RecordingFailure,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_recording_has_only_canonical_input_fields() {
        let manifest = CanonicalRecordingManifest {
            schema: RECORDING_SCHEMA.into(),
            frame_contract: CANONICAL_FRAME_CONTRACT_ID.into(),
            session_id: "session-1".into(),
            shape: CanonicalShape::fixed(),
            tick_index: TickIndexArtifact {
                path: TICK_INDEX_NAME.into(),
                sha256: "a".repeat(64),
                bytes: 100,
                count: 1,
            },
            tick_count: 1,
            segments: vec![SegmentArtifact {
                path: "segment-0000.mkv".into(),
                first_sequence: 7,
                last_sequence: 7,
                frames: 1,
                bytes: 100,
                sha256: "b".repeat(64),
            }],
            completeness: Completion::Complete,
            completeness_reasons: vec![],
            game_version: GameVersionState::NotObserved,
        };
        manifest.validate_structure().unwrap();
        let value = serde_json::to_value(&manifest).unwrap();
        for forbidden in [
            "capture_profile",
            "normalizer",
            "ffmpeg",
            "catalog",
            "model",
            "run_id",
        ] {
            assert!(value.get(forbidden).is_none(), "{forbidden}");
        }
        let mut invalid = manifest;
        invalid.tick_index.count = 2;
        assert!(invalid.validate_structure().is_err());
    }

    #[test]
    fn elision_reason_is_bound_to_the_observed_screen() {
        let mut tick = CanonicalTick {
            sequence: 1,
            source_sequence: 1,
            source_timestamp_ms: 100,
            screen: ScreenClass::Play,
            semantic_episode_id: None,
            disposition: TickDisposition::Elided(ElisionReason::PlayInterior),
        };
        tick.validate().unwrap();
        tick.screen = ScreenClass::Result;
        assert!(tick.validate().is_err());
        tick.disposition = TickDisposition::Retained;
        tick.validate().unwrap();
    }
}
