use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::catalog::Difficulty;
use crate::frame::{
    CANONICAL_BYTES, CANONICAL_FRAME_CONTRACT_ID, CANONICAL_HEIGHT, CANONICAL_WIDTH,
    CanonicalFrame, CanonicalLayout, FrameError, Roi, crop_pixels as crop_canonical_pixels,
};

#[path = "screen_reference.rs"]
pub(super) mod screen_reference;

use super::music_select::observe_music_select_play_type;
use super::music_select::{
    MusicSelectBestCrops, MusicSelectBestObservation, MusicSelectDifficultyMarkerCrops,
    MusicSelectMotionRegions, MusicSelectPlaySideCrops, MusicSelectScreenFieldObservations,
    MusicSelectScreenRgb8Crops, MusicSelectSongResolution, observe_music_select_difficulty,
    observe_music_select_play_side,
};
use super::result::numeric::NumericBatchInference;
use super::result::{PlayOptionsObservation, ResultSongResolution, observe_play_options};
use super::shared::NumericField;
use super::title::{
    CtcCharacterSet, DynamicTextObservation, OnnxParityError, decode_dynamic_official_onnx_crops,
};

#[cfg(test)]
use super::music_select::*;

const LAYOUT_SCHEMA: &str = "scorepeek-canonical-layout-v2";
const SCREEN_PATH_LAYOUT_SCHEMA: &str = "scorepeek-screen-path-layout-v7";
const NORMALIZER_SCHEMA: &str = "scorepeek-domain-normalizer-artifact-v1";
const EXTRACTION_SCHEMA: &str = "scorepeek-private-canonical-frame-extraction-v1";
const NORMALIZER_IMPLEMENTATION: &str = "ffmpeg-swscale-bt709-limited-to-rgb24-v1";
const NORMALIZER_FILTER: &str = "scale=1920:1080:flags=bitexact:in_color_matrix=bt709:out_color_matrix=bt709:in_range=tv:out_range=pc,format=rgb24";
const CALIBRATED_FFMPEG_SHA256: &str =
    "9eac5b2b5076db5ff853a6fa0dcd6b8de7d0cac8481eadda6c47cd935825f1ee";
const FFMPEG_VERSION: &str = "8.1.2";
const MAX_EXTRACTION_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_NORMALIZER_BYTES: u64 = 64 * 1024;
const PPM_HEADER: &[u8] = b"P6\n1920 1080\n255\n";
const CANONICAL_FILE_BYTES: u64 = CANONICAL_BYTES as u64 + PPM_HEADER.len() as u64;
const LAYOUT_BYTES: &[u8] = include_bytes!("../canonical-layout-v2.json");
const SCREEN_PATH_LAYOUT_BYTES: &[u8] = include_bytes!("../screen-path-layout-v7.json");
const INTEGRATED_CONTEXT_LAYOUT_BYTES: &[u8] =
    include_bytes!("../integrated-context-layout-v8.json");
const INTEGRATED_CONTEXT_MODEL_ID: &str = "pp-ocrv6-small-rec-onnx-v1";
#[cfg(test)]
const CALIBRATED_CAPTURE_PROFILE_SHA256: &str =
    "d5809dc9b2acc19837260053f4df59a454c9178ae2ac6a0602982effc9da4704";

fn calibrated_capture_profile(profile: &str) -> bool {
    profile.len() == 64 && profile.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug)]
pub enum RecognitionError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidCanonicalFrame,
    InvalidCanonicalLayout,
    NotResultScreen,
    NotMusicSelectScreen,
    Onnx(Box<OnnxParityError>),
}

impl std::fmt::Display for RecognitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "canonical frame I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "canonical layout JSON failed: {error}"),
            Self::InvalidCanonicalFrame => formatter.write_str("canonical frame is invalid"),
            Self::InvalidCanonicalLayout => formatter.write_str("canonical layout is invalid"),
            Self::NotResultScreen => formatter.write_str("canonical frame is not a result screen"),
            Self::NotMusicSelectScreen => {
                formatter.write_str("canonical frame is not a music-select screen")
            }
            Self::Onnx(error) => write!(
                formatter,
                "integrated context ONNX observation failed: {error}"
            ),
        }
    }
}

impl std::error::Error for RecognitionError {}

impl From<std::io::Error> for RecognitionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for RecognitionError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<FrameError> for RecognitionError {
    fn from(error: FrameError) -> Self {
        match error {
            FrameError::InvalidCanonicalFrame => Self::InvalidCanonicalFrame,
            FrameError::InvalidCanonicalLayout => Self::InvalidCanonicalLayout,
        }
    }
}

impl From<OnnxParityError> for RecognitionError {
    fn from(error: OnnxParityError) -> Self {
        Self::Onnx(Box::new(error))
    }
}

impl CanonicalFrame {
    /// Copies one layout-bound RGB8 crop in row-major order.
    ///
    /// # Errors
    /// Returns a recognition error when the ROI is outside the canonical frame.
    pub fn crop(&self, roi: Roi) -> Result<Vec<u8>, RecognitionError> {
        self.crop_region(roi).map_err(Into::into)
    }

    /// Reads one P6 frame only after validating its canonical extraction and normalizer evidence.
    ///
    /// # Errors
    /// Returns an error for an unknown frame ID, invalid or mismatched evidence, or bytes outside
    /// the fixed canonical RGB8 contract.
    pub fn read_extraction(
        directory: impl AsRef<Path>,
        frame_id: &str,
        expected_extraction_sha256: &str,
    ) -> Result<Self, RecognitionError> {
        if !valid_sha256(expected_extraction_sha256) {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let directory = directory.as_ref();
        if !directory.metadata()?.is_dir() {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let manifest_path = directory.join("manifest.json");
        let normalizer_path = directory.join("normalizer.json");
        for path in [&manifest_path, &normalizer_path] {
            if !path.metadata()?.is_file() {
                return Err(RecognitionError::InvalidCanonicalFrame);
            }
        }
        let manifest_bytes =
            read_bounded_regular(&manifest_path, MAX_EXTRACTION_MANIFEST_BYTES, None)?;
        if encode_sha256(&manifest_bytes) != expected_extraction_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let manifest: CanonicalExtractionEvidence = serde_json::from_slice(&manifest_bytes)?;
        if canonical_evidence_json(&manifest)? != manifest_bytes {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let normalizer_bytes = read_bounded_regular(&normalizer_path, MAX_NORMALIZER_BYTES, None)?;
        let normalizer: DomainNormalizerEvidence = serde_json::from_slice(&normalizer_bytes)?;
        if canonical_evidence_json(&normalizer)? != normalizer_bytes {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        manifest.validate(&normalizer, &normalizer_bytes)?;
        let frame = manifest.frame(frame_id)?;
        let frame_path = directory.join(&frame.filename);
        let bytes = read_bounded_regular(&frame_path, CANONICAL_FILE_BYTES, Some(frame.bytes))?;
        if encode_sha256(&bytes) != frame.file_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let pixels = bytes
            .strip_prefix(PPM_HEADER)
            .ok_or(RecognitionError::InvalidCanonicalFrame)?;
        if pixels.len() != CANONICAL_BYTES || encode_sha256(pixels) != frame.frame_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        Self::from_validated_parts(
            pixels.into(),
            frame.source_pts,
            frame.decode_index,
            manifest.capture_profile_id,
            manifest.normalizer_artifact_sha256,
            expected_extraction_sha256.to_owned(),
        )
        .map_err(Into::into)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalExtractionEvidence {
    schema: String,
    fixture_id: String,
    source_manifest_sha256: String,
    media_probe_sha256: String,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    canonical_frame_contract_id: String,
    extractor: ExtractorEvidence,
    source_time_base: TimeBaseEvidence,
    video_stream_index: u32,
    frames: Vec<CanonicalExtractedFrameEvidence>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalExtractedFrameEvidence {
    frame_id: String,
    source_pts: i64,
    decode_index: u64,
    filename: String,
    frame_sha256: String,
    file_sha256: String,
    bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DomainNormalizerEvidence {
    schema: String,
    capture_profile_id: String,
    observed: ObservedMediaEvidence,
    canonical_frame_contract_id: String,
    implementation: String,
    ffmpeg_sha256: String,
    filter: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ObservedMediaEvidence {
    input_format: String,
    codec_name: String,
    pixel_format: String,
    width: u32,
    height: u32,
    source_time_base: TimeBaseEvidence,
    color_range: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TimeBaseEvidence {
    numerator: u32,
    denominator: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExtractorEvidence {
    tool_id: String,
    tool_version: String,
    extractor_manifest_sha256: String,
    parameters_sha256: String,
}

impl CanonicalExtractionEvidence {
    fn validate(
        &self,
        normalizer: &DomainNormalizerEvidence,
        normalizer_bytes: &[u8],
    ) -> Result<(), RecognitionError> {
        if self.schema != EXTRACTION_SCHEMA
            || self.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
            || self.capture_profile_id != normalizer.capture_profile_id
            || self.fixture_id.is_empty()
            || !valid_sha256(&self.source_manifest_sha256)
            || !valid_sha256(&self.media_probe_sha256)
            || !valid_sha256(&self.capture_profile_id)
            || !valid_sha256(&self.normalizer_artifact_sha256)
            || encode_sha256(normalizer_bytes) != self.normalizer_artifact_sha256
            || normalizer.schema != NORMALIZER_SCHEMA
            || normalizer.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
            || normalizer.implementation != NORMALIZER_IMPLEMENTATION
            || normalizer.filter != NORMALIZER_FILTER
            || !calibrated_capture_profile(&normalizer.capture_profile_id)
            || normalizer.ffmpeg_sha256 != CALIBRATED_FFMPEG_SHA256
            || !normalizer.observed.is_supported()
            || normalizer.observed.source_time_base != self.source_time_base
            || self.extractor.tool_id != "ffmpeg"
            || self.extractor.tool_version != FFMPEG_VERSION
            || self.extractor.extractor_manifest_sha256 != self.media_probe_sha256
            || !valid_sha256(&self.extractor.parameters_sha256)
            || self.video_stream_index > 255
            || self.frames.is_empty()
            || self.frames.len() > 512
        {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let mut frame_ids = BTreeSet::new();
        let mut previous_decode_index = None;
        for (index, frame) in self.frames.iter().enumerate() {
            if frame.frame_id.is_empty()
                || !frame_ids.insert(frame.frame_id.as_str())
                || frame.filename != format!("frame-{index:06}.ppm")
                || !valid_sha256(&frame.frame_sha256)
                || !valid_sha256(&frame.file_sha256)
                || frame.bytes != CANONICAL_FILE_BYTES
                || previous_decode_index.is_some_and(|previous| previous >= frame.decode_index)
            {
                return Err(RecognitionError::InvalidCanonicalFrame);
            }
            previous_decode_index = Some(frame.decode_index);
        }
        Ok(())
    }

    fn frame(&self, frame_id: &str) -> Result<&CanonicalExtractedFrameEvidence, RecognitionError> {
        let mut matching = self
            .frames
            .iter()
            .filter(|frame| frame.frame_id == frame_id);
        let frame = matching
            .next()
            .ok_or(RecognitionError::InvalidCanonicalFrame)?;
        if matching.next().is_some() {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        Ok(frame)
    }
}

impl ObservedMediaEvidence {
    fn is_supported(&self) -> bool {
        self.input_format == "matroska"
            && self.codec_name == "ffv1"
            && self.pixel_format == "yuv420p"
            && self.width == CANONICAL_WIDTH
            && self.height == CANONICAL_HEIGHT
            && self.source_time_base.numerator == 1
            && self.source_time_base.denominator == 1_000
            && self.color_range.as_deref() == Some("tv")
            && self.color_space.as_deref() == Some("bt709")
            && self.color_transfer.as_deref() == Some("bt709")
            && self.color_primaries.as_deref() == Some("bt709")
    }
}

fn read_bounded_regular(
    path: &Path,
    maximum: u64,
    exact: Option<u64>,
) -> Result<Vec<u8>, RecognitionError> {
    let metadata = path.metadata()?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum
        || exact.is_some_and(|expected| metadata.len() != expected)
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let capacity =
        usize::try_from(metadata.len()).map_err(|_| RecognitionError::InvalidCanonicalFrame)?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    Ok(bytes)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub(in crate::recognition) fn encode_sha256(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn canonical_evidence_json(value: &impl Serialize) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultPanelOrigins {
    left: u32,
    right: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultNumericFieldOrigins {
    score: u32,
    judgment: u32,
    timing: u32,
    combo_break: u32,
}

impl ResultNumericFieldOrigins {
    const fn get(self, field: NumericField) -> u32 {
        match field {
            NumericField::CurrentScore
            | NumericField::PreviousScore
            | NumericField::PreviousMissCount
            | NumericField::MissCount => self.score,
            NumericField::Pgreat
            | NumericField::Great
            | NumericField::Good
            | NumericField::Bad
            | NumericField::Poor => self.judgment,
            NumericField::Fast | NumericField::Slow => self.timing,
            NumericField::ComboBreak => self.combo_break,
            NumericField::Level | NumericField::Notes => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::recognition) struct ResultNumericPanelOrigins {
    left: ResultNumericFieldOrigins,
    right: ResultNumericFieldOrigins,
}

impl ResultNumericPanelOrigins {
    pub(in crate::recognition) const fn get(
        self,
        side: ResultPanelSide,
        field: NumericField,
    ) -> u32 {
        match side {
            ResultPanelSide::Left => self.left,
            ResultPanelSide::Right => self.right,
        }
        .get(field)
    }
}

impl ResultPanelOrigins {
    const fn get(self, side: ResultPanelSide) -> u32 {
        match side {
            ResultPanelSide::Left => self.left,
            ResultPanelSide::Right => self.right,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultLayout {
    presence: ResultPresencePredicate,
    pub header: Roi,
    panel_origins: ResultPanelOrigins,
    pub(in crate::recognition) numeric_panel_origins: ResultNumericPanelOrigins,
    pub upper_panel_edge: Roi,
    pub lower_panel_edge: Roi,
    pub title: Roi,
    pub artist: Roi,
    pub clear_type: Roi,
    pub difficulty: Roi,
    pub level: Roi,
    pub notes: Roi,
    pub current_score: Roi,
    pub previous_clear_type: Roi,
    pub previous_score: Roi,
    pub previous_miss_count: Roi,
    pub miss_count: Roi,
    pub pgreat: Roi,
    pub great: Roi,
    pub good: Roi,
    pub bad: Roi,
    pub poor: Roi,
    pub fast: Roi,
    pub slow: Roi,
    pub combo_break: Roi,
    pub play_options: Roi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MusicSelectLayout {
    presence: MusicSelectPresencePredicate,
    pub header: Roi,
    pub label: Roi,
    pub level_column: Roi,
    pub selected_title: Roi,
    pub list_titles: RepeatedRoi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideTransitionLayout {
    presence: DecideTransitionPresencePredicate,
    pub splash: Roi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayLayout {
    presence: PlayPresencePredicate,
    pub bpm_outline_search: Roi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_field_names,
    reason = "field names intentionally match the canonical layout contract"
)]
struct MusicSelectPresencePredicate {
    cyan_header_pixels_min: u32,
    colored_level_pixels_min: u32,
    bright_label_pixels_min: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    clippy::struct_field_names,
    reason = "field names intentionally match the screen-path layout contract"
)]
struct DecideTransitionPresencePredicate {
    cyan_pixels_min: u32,
    bright_pixels_min: u32,
    saturated_pixels_min: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlayPresencePredicate {
    top_edge_pixels_min: u32,
    top_edge_pixels_max: u32,
    bottom_edge_pixels_min: u32,
    bottom_edge_pixels_max: u32,
    vertical_distance_min: u32,
    vertical_distance_max: u32,
    edge_center_delta_x2_max: u32,
    candidate_cluster_delta_x2_max: u32,
    candidate_cluster_delta_y_max: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct TitlePresencePredicate {
    bright_channel_min: u8,
    bbox_x_min: u32,
    bbox_x_max: u32,
    bbox_y_min: u32,
    bbox_y_max: u32,
    bbox_width_min: u32,
    bbox_width_max: u32,
    bbox_height_min: u32,
    bbox_height_max: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepeatedRoi {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    stride_y: u32,
    slots: u32,
}

impl RepeatedRoi {
    fn rois(self) -> impl Iterator<Item = Roi> {
        (0..self.slots).map(move |slot| Roi {
            x: self.x,
            y: self.y + slot * self.stride_y,
            width: self.width,
            height: self.height,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultPresencePredicate {
    warm_pixels_min: u32,
    horizontal_edge_pixels_min: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenClass {
    Title,
    Result,
    MusicSelect,
    ModeSelect,
    DecideTransition,
    Play,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenCropRoute {
    Title,
    Result(ResultPanelSide),
    MusicSelect,
}

impl ScreenPredicateObservation {
    #[must_use]
    pub const fn crop_route(&self) -> Option<ScreenCropRoute> {
        match self.screen {
            ScreenClass::Title => Some(ScreenCropRoute::Title),
            ScreenClass::Result => match self.result_presence.panel_side.known() {
                Some(side) => Some(ScreenCropRoute::Result(side)),
                None => None,
            },
            ScreenClass::MusicSelect => Some(ScreenCropRoute::MusicSelect),
            ScreenClass::ModeSelect
            | ScreenClass::DecideTransition
            | ScreenClass::Play
            | ScreenClass::Unknown => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultPanelSide {
    #[default]
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultPanelSideUnknownReason {
    NoCandidate,
    MultipleCandidates,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum ResultPanelSideState {
    Known(ResultPanelSide),
    Unknown(ResultPanelSideUnknownReason),
}

impl ResultPanelSideState {
    #[must_use]
    pub const fn known(self) -> Option<ResultPanelSide> {
        match self {
            Self::Known(side) => Some(side),
            Self::Unknown(_) => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecognitionSnapshot {
    pub schema: String,
    pub canonical_frame_sha256: String,
    pub normalizer_artifact_sha256: String,
    pub frame_extraction_sha256: String,
    pub canonical_layout_sha256: String,
    pub screen_path_layout_sha256: String,
    pub screen: ScreenClass,
    pub title_presence: TitlePresenceEvidence,
    pub result_presence: ResultPresenceEvidence,
    pub music_select_presence: MusicSelectPresenceEvidence,
    pub decide_transition_presence: DecideTransitionPresenceEvidence,
    pub play_presence: PlayPresenceEvidence,
}

impl RecognitionSnapshot {
    #[must_use]
    pub const fn crop_route(&self) -> Option<ScreenCropRoute> {
        match self.screen {
            ScreenClass::Title => Some(ScreenCropRoute::Title),
            ScreenClass::Result => match self.result_presence.panel_side.known() {
                Some(side) => Some(ScreenCropRoute::Result(side)),
                None => None,
            },
            ScreenClass::MusicSelect => Some(ScreenCropRoute::MusicSelect),
            ScreenClass::ModeSelect
            | ScreenClass::DecideTransition
            | ScreenClass::Play
            | ScreenClass::Unknown => None,
        }
    }
}

/// A pure canonical-RGB8 screen-predicate result without capture or extraction provenance.
///
/// This value is not an accepted live recognition input. The application must bind it to its
/// profile- and generation-bearing live frame before recording or accepting the observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScreenPredicateObservation {
    pub screen_path_layout_sha256: String,
    pub screen: ScreenClass,
    pub title_presence: TitlePresenceEvidence,
    pub result_presence: ResultPresenceEvidence,
    pub music_select_presence: MusicSelectPresenceEvidence,
    pub decide_transition_presence: DecideTransitionPresenceEvidence,
    pub play_presence: PlayPresenceEvidence,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResultCropArtifact {
    pub schema: String,
    pub frame_id: String,
    pub frame_extraction_sha256: String,
    pub canonical_frame_sha256: String,
    pub normalizer_artifact_sha256: String,
    pub canonical_layout_sha256: String,
    pub crops: Vec<ResultCropEvidence>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResultCropEvidence {
    pub field: ResultCropField,
    pub filename: String,
    pub roi: Roi,
    pub pixel_sha256: String,
    pub file_sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultCropField {
    Title,
    Artist,
    ClearType,
    Difficulty,
    Level,
    Notes,
    CurrentScore,
    PreviousClearType,
    PreviousScore,
    PreviousMissCount,
    MissCount,
    Pgreat,
    Great,
    Good,
    Bad,
    Poor,
    Fast,
    Slow,
    ComboBreak,
    PlayOptions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResultCropExportSummary {
    pub schema: String,
    pub output: PathBuf,
    pub manifest_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MusicSelectCropArtifact {
    pub schema: String,
    pub frame_id: String,
    pub frame_extraction_sha256: String,
    pub canonical_frame_sha256: String,
    pub normalizer_artifact_sha256: String,
    pub canonical_layout_sha256: String,
    pub crops: Vec<MusicSelectCropEvidence>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MusicSelectCropEvidence {
    pub field: String,
    pub filename: String,
    pub roi: Roi,
    pub pixel_sha256: String,
    pub file_sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectCropExportSummary {
    pub schema: String,
    pub output: PathBuf,
    pub manifest_sha256: String,
    pub list_slot_count: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(in crate::recognition) struct IntegratedContextLayout {
    schema: String,
    canonical_frame_contract_id: String,
    canonical_layout_sha256: String,
    result: ResultContextLayout,
    pub(in crate::recognition) music_select: MusicSelectContextLayout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ResultContextLayout {
    artist: Roi,
    play_type: Roi,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(in crate::recognition) struct MusicSelectContextLayout {
    artist: Roi,
    legacy_selected_chart: Roi,
    pub(in crate::recognition) play_type: MusicSelectPlayTypeLayout,
    pub(in crate::recognition) selected_difficulty: MusicSelectDifficultyLayout,
    pub(in crate::recognition) play_side: MusicSelectPlaySideLayout,
    active_list_title: Roi,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(in crate::recognition) struct MusicSelectPlayTypeLayout {
    pub(in crate::recognition) algorithm_id: String,
    pub(in crate::recognition) roi: Roi,
    pub(in crate::recognition) template_width: u32,
    pub(in crate::recognition) template_height: u32,
    pub(in crate::recognition) single_asset_sha256: String,
    pub(in crate::recognition) double_asset_sha256: String,
    pub(in crate::recognition) score_min_ppm: u32,
    pub(in crate::recognition) winner_margin_min_ppm: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(in crate::recognition) struct MusicSelectDifficultyLayout {
    pub(in crate::recognition) predicate_id: String,
    pub(in crate::recognition) score_min_ppm: u32,
    pub(in crate::recognition) winner_margin_min_ppm: u32,
    beginner: Roi,
    normal: Roi,
    hyper: Roi,
    another: Roi,
    leggendaria: Roi,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub(in crate::recognition) struct MusicSelectPlaySideLayout {
    pub(in crate::recognition) predicate_id: String,
    pub(in crate::recognition) luma_min: u8,
    pub(in crate::recognition) bright_pixel_min: u32,
    pub(in crate::recognition) winner_margin_min: u32,
    one_player: Roi,
    two_player: Roi,
}

impl MusicSelectDifficultyLayout {
    const fn slots(&self) -> [(Difficulty, Roi); 5] {
        [
            (Difficulty::Beginner, self.beginner),
            (Difficulty::Normal, self.normal),
            (Difficulty::Hyper, self.hyper),
            (Difficulty::Another, self.another),
            (Difficulty::Leggendaria, self.leggendaria),
        ]
    }
}

impl IntegratedContextLayout {
    pub(in crate::recognition) fn load() -> Result<Self, RecognitionError> {
        let layout: Self = serde_json::from_slice(INTEGRATED_CONTEXT_LAYOUT_BYTES)?;
        let canonical = CanonicalLayout::load()?;
        let active_list_slot = canonical
            .music_select
            .list_titles
            .rois()
            .nth(10)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if layout.schema != "scorepeek-integrated-context-layout-v8"
            || layout.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
            || layout.canonical_layout_sha256 != CanonicalLayout::sha256()
            || layout.result.artist != canonical.result.artist
            || layout.music_select.active_list_title.y < active_list_slot.y
            || layout.music_select.active_list_title.y
                + layout.music_select.active_list_title.height
                > active_list_slot.y + active_list_slot.height
            || layout.music_select.active_list_title.x >= active_list_slot.x
            || layout.music_select.active_list_title.x + layout.music_select.active_list_title.width
                != active_list_slot.x + active_list_slot.width
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        for roi in [
            layout.result.artist,
            layout.result.play_type,
            layout.music_select.artist,
            layout.music_select.legacy_selected_chart,
            layout.music_select.play_type.roi,
            layout.music_select.play_side.one_player,
            layout.music_select.play_side.two_player,
            layout.music_select.active_list_title,
        ] {
            roi.validate(CANONICAL_WIDTH, CANONICAL_HEIGHT)?;
        }
        let play_type = &layout.music_select.play_type;
        if play_type.algorithm_id != "imageproc-cross-correlation-normalized-gray8-v1"
            || play_type.roi.width != play_type.template_width
            || play_type.roi.height != play_type.template_height
            || !valid_sha256(&play_type.single_asset_sha256)
            || !valid_sha256(&play_type.double_asset_sha256)
            || play_type.score_min_ppm == 0
            || play_type.score_min_ppm > 1_000_000
            || play_type.winner_margin_min_ppm == 0
            || play_type.winner_margin_min_ppm > 1_000_000
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        if layout.music_select.selected_difficulty.predicate_id
            != "scorepeek-player-marker-outline-v2"
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        for (_, roi) in layout.music_select.selected_difficulty.slots() {
            roi.validate(CANONICAL_WIDTH, CANONICAL_HEIGHT)?;
            if roi.width != 128 || roi.height != 30 {
                return Err(RecognitionError::InvalidCanonicalLayout);
            }
        }
        let play_side = &layout.music_select.play_side;
        if play_side.predicate_id != "scorepeek-music-select-footer-brightness-v1"
            || play_side.luma_min == 0
            || play_side.bright_pixel_min == 0
            || play_side.winner_margin_min == 0
            || play_side.one_player.width != play_side.two_player.width
            || play_side.one_player.height != play_side.two_player.height
            || play_side.one_player.y != play_side.two_player.y
            || play_side.bright_pixel_min
                > play_side
                    .one_player
                    .width
                    .saturating_mul(play_side.one_player.height)
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(layout)
    }

    fn sha256() -> String {
        encode_sha256(INTEGRATED_CONTEXT_LAYOUT_BYTES)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegratedContextField {
    ResultArtist,
    MusicSelectArtist,
    MusicSelectSelectedChart,
    MusicSelectPlayType,
    MusicSelectActiveListTitle,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegratedContextCropEvidence {
    pub field: IntegratedContextField,
    pub filename: String,
    pub roi: Roi,
    pub pixel_sha256: String,
    pub file_sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IntegratedContextCropArtifact {
    pub schema: String,
    pub frame_id: String,
    pub frame_extraction_sha256: String,
    pub canonical_frame_sha256: String,
    pub normalizer_artifact_sha256: String,
    pub canonical_layout_sha256: String,
    pub integrated_context_layout_sha256: String,
    pub screen: ScreenClass,
    pub crops: Vec<IntegratedContextCropEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntegratedContextCropExportSummary {
    pub schema: String,
    pub output: PathBuf,
    pub manifest_sha256: String,
    pub screen: ScreenClass,
}

/// One in-memory RGB8 crop from the scorepeek-owned canonical layouts.
///
/// This pure value carries no capture provenance or accepted-field authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rgb8Crop {
    pub roi: Roi,
    pub(in crate::recognition) pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TitleForegroundGeometry {
    pub bbox: Roi,
    pub occupancy_width_ppm: u32,
    pub touches_left_edge: bool,
    pub touches_right_edge: bool,
}

/// The single registered active-list title view used by production recognition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TitleEvidenceExtractor {
    grayscale_threshold: u8,
    horizontal_margin: u32,
}

impl TitleEvidenceExtractor {
    pub const REGISTERED: Self = Self {
        grayscale_threshold: 80,
        horizontal_margin: 4,
    };

    #[must_use]
    pub fn extract(self, source: &Rgb8Crop) -> Option<(Rgb8Crop, TitleForegroundGeometry)> {
        source.extract_title_foreground(self.grayscale_threshold, self.horizontal_margin)
    }
}

impl Rgb8Crop {
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Extracts the registered active-title foreground view without interpreting its text.
    #[must_use]
    pub fn title_foreground_crop(&self) -> Option<(Self, TitleForegroundGeometry)> {
        TitleEvidenceExtractor::REGISTERED.extract(self)
    }

    fn extract_title_foreground(
        &self,
        grayscale_threshold: u8,
        horizontal_margin: u32,
    ) -> Option<(Self, TitleForegroundGeometry)> {
        let width = usize::try_from(self.roi.width).ok()?;
        let height = usize::try_from(self.roi.height).ok()?;
        let mut minimum_x = width;
        let mut minimum_y = height;
        let mut maximum_x = 0_usize;
        let mut maximum_y = 0_usize;
        let mut observed = false;
        for y in 0..height {
            for x in 0..width {
                let offset = (y * width + x) * 3;
                let [red, green, blue] = self.pixels.get(offset..offset + 3)? else {
                    return None;
                };
                let grayscale =
                    (u32::from(*red) * 77 + u32::from(*green) * 150 + u32::from(*blue) * 29) / 256;
                if grayscale > u32::from(grayscale_threshold) {
                    observed = true;
                    minimum_x = minimum_x.min(x);
                    minimum_y = minimum_y.min(y);
                    maximum_x = maximum_x.max(x);
                    maximum_y = maximum_y.max(y);
                }
            }
        }
        if !observed {
            return None;
        }
        let horizontal_margin = usize::try_from(horizontal_margin).ok()?;
        let crop_minimum_x = minimum_x.saturating_sub(horizontal_margin);
        let crop_maximum_x = maximum_x.saturating_add(horizontal_margin).min(width - 1);
        let crop_width = crop_maximum_x - crop_minimum_x + 1;
        let mut pixels = Vec::with_capacity(crop_width * height * 3);
        for y in 0..height {
            let start = (y * width + crop_minimum_x) * 3;
            let end = start + crop_width * 3;
            pixels.extend_from_slice(self.pixels.get(start..end)?);
        }
        let foreground_width = maximum_x - minimum_x + 1;
        let geometry = TitleForegroundGeometry {
            bbox: Roi {
                x: self.roi.x + u32::try_from(minimum_x).ok()?,
                y: self.roi.y + u32::try_from(minimum_y).ok()?,
                width: u32::try_from(foreground_width).ok()?,
                height: u32::try_from(maximum_y - minimum_y + 1).ok()?,
            },
            occupancy_width_ppm: u32::try_from(
                foreground_width.saturating_mul(1_000_000) / width.max(1),
            )
            .ok()?,
            touches_left_edge: minimum_x == 0,
            touches_right_edge: maximum_x + 1 == width,
        };
        Some((
            Self {
                roi: Roi {
                    x: self.roi.x + u32::try_from(crop_minimum_x).ok()?,
                    y: self.roi.y,
                    width: u32::try_from(crop_width).ok()?,
                    height: self.roi.height,
                },
                pixels,
            },
            geometry,
        ))
    }

    /// Returns the tight score-colored content crop used by the numeric result recognizer.
    #[must_use]
    pub fn cyan_content_crop(&self) -> Option<Self> {
        let width = usize::try_from(self.roi.width).ok()?;
        let height = usize::try_from(self.roi.height).ok()?;
        let mut bounds: Option<(usize, usize, usize, usize)> = None;
        for y in 0..height {
            for x in 0..width {
                let offset = (y * width + x) * 3;
                let [r, g, b] = self.pixels.get(offset..offset + 3)? else {
                    return None;
                };
                if *g > 120 && *b > 150 && u16::from(*b) * 2 > u16::from(*r) * 3 {
                    bounds = Some(bounds.map_or((x, y, x, y), |(min_x, min_y, max_x, max_y)| {
                        (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
                    }));
                }
            }
        }
        let (min_x, min_y, max_x, max_y) = bounds?;
        let min_x = min_x.saturating_sub(2);
        let min_y = min_y.saturating_sub(2);
        let max_x = max_x.saturating_add(2).min(width - 1);
        let max_y = max_y.saturating_add(2).min(height - 1);
        let crop_width = max_x - min_x + 1;
        let crop_height = max_y - min_y + 1;
        let mut pixels = Vec::with_capacity(crop_width * crop_height * 3);
        for y in min_y..=max_y {
            let start = (y * width + min_x) * 3;
            let end = start + crop_width * 3;
            pixels.extend_from_slice(self.pixels.get(start..end)?);
        }
        Some(Self {
            roi: Roi {
                x: self.roi.x + u32::try_from(min_x).ok()?,
                y: self.roi.y + u32::try_from(min_y).ok()?,
                width: u32::try_from(crop_width).ok()?,
                height: u32::try_from(crop_height).ok()?,
            },
            pixels,
        })
    }
}

/// Every currently measured result-screen field crop.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResultScreenRgb8Crops {
    pub canonical_layout_sha256: String,
    pub panel_side: ResultPanelSide,
    pub title: Rgb8Crop,
    pub artist: Rgb8Crop,
    pub clear_type: Rgb8Crop,
    pub difficulty: Rgb8Crop,
    pub play_type: Rgb8Crop,
    pub level: Rgb8Crop,
    pub notes: Rgb8Crop,
    pub current_score: Rgb8Crop,
    pub previous_clear_type: Rgb8Crop,
    pub previous_score: Rgb8Crop,
    pub previous_miss_count: Rgb8Crop,
    pub miss_count: Rgb8Crop,
    pub pgreat: Rgb8Crop,
    pub great: Rgb8Crop,
    pub good: Rgb8Crop,
    pub bad: Rgb8Crop,
    pub poor: Rgb8Crop,
    pub fast: Rgb8Crop,
    pub slow: Rgb8Crop,
    pub combo_break: Rgb8Crop,
    pub play_options: Rgb8Crop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TitleScreenRgb8Crops {
    pub canonical_layout_sha256: String,
    pub game_version: Rgb8Crop,
}

/// Measured field crops for exactly one classified screen.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "both variants are short-lived fixed-layout crop views and boxing would add allocation"
)]
pub enum ScreenRgb8Crops {
    Title(TitleScreenRgb8Crops),
    Result(ResultScreenRgb8Crops),
    MusicSelect(MusicSelectScreenRgb8Crops),
}

/// One text field that can fail without fabricating a partial screen observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenTextField {
    TitleGameVersion,
    MusicSelectBestHeader,
    MusicSelectBestClearType,
    ResultNumericBatch,
    ResultTitle,
    ResultArtist,
    ResultClearType,
    ResultDifficulty,
    ResultPlayType,
    ResultLevel,
    ResultNotes,
    ResultCurrentScore,
    ResultPreviousClearType,
    ResultPreviousScore,
    ResultPreviousMissCount,
    ResultMissCount,
    ResultPgreat,
    ResultGreat,
    ResultGood,
    ResultBad,
    ResultPoor,
    ResultFast,
    ResultSlow,
    ResultComboBreak,
    ResultPlayOptions,
    MusicSelectCentralTitle,
    MusicSelectArtist,
    MusicSelectActiveListTitle,
}

impl ScreenTextField {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TitleGameVersion => "title_game_version",
            Self::MusicSelectBestHeader => "music_select_best_header",
            Self::MusicSelectBestClearType => "music_select_best_clear_type",
            Self::ResultNumericBatch => "result_numeric_batch",
            Self::ResultTitle => "result_title",
            Self::ResultArtist => "result_artist",
            Self::ResultClearType => "result_clear_type",
            Self::ResultDifficulty => "result_difficulty",
            Self::ResultPlayType => "result_play_type",
            Self::ResultLevel => "result_level",
            Self::ResultNotes => "result_notes",
            Self::ResultCurrentScore => "result_current_score",
            Self::ResultPreviousClearType => "result_previous_clear_type",
            Self::ResultPreviousScore => "result_previous_score",
            Self::ResultPreviousMissCount => "result_previous_miss_count",
            Self::ResultMissCount => "result_miss_count",
            Self::ResultPgreat => "result_pgreat",
            Self::ResultGreat => "result_great",
            Self::ResultGood => "result_good",
            Self::ResultBad => "result_bad",
            Self::ResultPoor => "result_poor",
            Self::ResultFast => "result_fast",
            Self::ResultSlow => "result_slow",
            Self::ResultComboBreak => "result_combo_break",
            Self::ResultPlayOptions => "result_play_options",
            Self::MusicSelectCentralTitle => "music_select_central_title",
            Self::MusicSelectArtist => "music_select_artist",
            Self::MusicSelectActiveListTitle => "music_select_active_list_title",
        }
    }

    #[must_use]
    pub const fn ctc_character_set(self) -> Option<CtcCharacterSet> {
        match self {
            Self::ResultLevel => Some(CtcCharacterSet::DigitsUpToTwo),
            Self::ResultNotes
            | Self::ResultCurrentScore
            | Self::ResultPgreat
            | Self::ResultGreat
            | Self::ResultGood
            | Self::ResultBad
            | Self::ResultPoor => Some(CtcCharacterSet::Digits),
            Self::ResultPreviousScore
            | Self::ResultPreviousMissCount
            | Self::ResultMissCount
            | Self::ResultFast
            | Self::ResultSlow => Some(CtcCharacterSet::DigitsAndDashes),
            Self::ResultComboBreak => Some(CtcCharacterSet::DigitsAndDashesUpToThree),
            Self::TitleGameVersion
            | Self::ResultNumericBatch
            | Self::ResultTitle
            | Self::ResultArtist
            | Self::ResultClearType
            | Self::ResultDifficulty
            | Self::ResultPlayType
            | Self::ResultPreviousClearType
            | Self::ResultPlayOptions
            | Self::MusicSelectCentralTitle
            | Self::MusicSelectArtist
            | Self::MusicSelectActiveListTitle
            | Self::MusicSelectBestHeader
            | Self::MusicSelectBestClearType => None,
        }
    }
}

/// Complete result-screen field observations from the currently registered observers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResultScreenFieldObservations {
    pub panel_side: ResultPanelSide,
    pub title: DynamicTextObservation,
    pub artist: DynamicTextObservation,
    pub clear_type: DynamicTextObservation,
    pub difficulty: DynamicTextObservation,
    pub play_type: DynamicTextObservation,
    pub level: DynamicTextObservation,
    pub notes: DynamicTextObservation,
    pub current_score: DynamicTextObservation,
    pub previous_clear_type: DynamicTextObservation,
    pub previous_score: DynamicTextObservation,
    pub previous_miss_count: DynamicTextObservation,
    pub miss_count: DynamicTextObservation,
    pub pgreat: DynamicTextObservation,
    pub great: DynamicTextObservation,
    pub good: DynamicTextObservation,
    pub bad: DynamicTextObservation,
    pub poor: DynamicTextObservation,
    pub fast: DynamicTextObservation,
    pub slow: DynamicTextObservation,
    pub combo_break: DynamicTextObservation,
    pub play_options: PlayOptionsObservation,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TitleScreenFieldObservations {
    pub game_version: DynamicTextObservation,
}

/// Complete field-observer output for exactly one classified screen.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the bounded worker output retains a flat screen-specific observation schema"
)]
pub enum ScreenFieldObservations {
    Title(TitleScreenFieldObservations),
    Result(ResultScreenFieldObservations),
    MusicSelect(MusicSelectScreenFieldObservations),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "screen", content = "resolution", rename_all = "snake_case")]
pub enum ScreenSongResolution {
    Title,
    Result(ResultSongResolution),
    MusicSelect(MusicSelectSongResolution),
}

impl ScreenFieldObservations {
    #[must_use]
    pub const fn screen(&self) -> ScreenClass {
        match self {
            Self::Title(_) => ScreenClass::Title,
            Self::Result(_) => ScreenClass::Result,
            Self::MusicSelect(_) => ScreenClass::MusicSelect,
        }
    }

    #[must_use]
    pub const fn diagnostic_field_counts(&self) -> (u8, u8) {
        match self {
            Self::Title(_) => (1, 0),
            Self::Result(_) => (20, 0),
            Self::MusicSelect(_) => (8, 1),
        }
    }
}

/// One failed text inference with the exact screen-local field and original cause.
#[derive(Debug)]
pub struct ScreenFieldObservationError<E> {
    pub field: ScreenTextField,
    source: E,
}

impl<E> ScreenFieldObservationError<E> {
    #[must_use]
    pub const fn new(field: ScreenTextField, source: E) -> Self {
        Self { field, source }
    }

    #[must_use]
    pub const fn source_error(&self) -> &E {
        &self.source
    }
}

impl<E: std::fmt::Display> std::fmt::Display for ScreenFieldObservationError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "field observation failed for {}: {}",
            self.field.as_str(),
            self.source
        )
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ScreenFieldObservationError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// Applies one text observer to every registered text field in a complete screen crop set.
///
/// # Errors
/// Returns the exact failed field and observer error without constructing a partial screen output.
///
/// # Panics
/// Panics only if the embedded MUSIC SELECT play-type layout or templates fail their static
/// contract after the crop was constructed from that same layout.
pub fn observe_screen_fields<E>(
    crops: &ScreenRgb8Crops,
    mut observe_text: impl FnMut(ScreenTextField, &Rgb8Crop) -> Result<DynamicTextObservation, E>,
) -> Result<ScreenFieldObservations, ScreenFieldObservationError<E>> {
    let mut observe = |field, crop| {
        observe_text(field, crop).map_err(|source| ScreenFieldObservationError::new(field, source))
    };
    Ok(match crops {
        ScreenRgb8Crops::Title(crops) => {
            ScreenFieldObservations::Title(TitleScreenFieldObservations {
                game_version: observe(ScreenTextField::TitleGameVersion, &crops.game_version)?,
            })
        }
        ScreenRgb8Crops::Result(crops) => {
            ScreenFieldObservations::Result(ResultScreenFieldObservations {
                panel_side: crops.panel_side,
                title: observe(ScreenTextField::ResultTitle, &crops.title)?,
                artist: observe(ScreenTextField::ResultArtist, &crops.artist)?,
                clear_type: observe(ScreenTextField::ResultClearType, &crops.clear_type)?,
                difficulty: observe(ScreenTextField::ResultDifficulty, &crops.difficulty)?,
                play_type: observe(ScreenTextField::ResultPlayType, &crops.play_type)?,
                level: observe(ScreenTextField::ResultLevel, &crops.level)?,
                notes: observe(ScreenTextField::ResultNotes, &crops.notes)?,
                current_score: observe(ScreenTextField::ResultCurrentScore, &crops.current_score)?,
                previous_clear_type: observe(
                    ScreenTextField::ResultPreviousClearType,
                    &crops.previous_clear_type,
                )?,
                previous_score: observe(
                    ScreenTextField::ResultPreviousScore,
                    &crops.previous_score,
                )?,
                previous_miss_count: observe(
                    ScreenTextField::ResultPreviousMissCount,
                    &crops.previous_miss_count,
                )?,
                miss_count: observe(ScreenTextField::ResultMissCount, &crops.miss_count)?,
                pgreat: observe(ScreenTextField::ResultPgreat, &crops.pgreat)?,
                great: observe(ScreenTextField::ResultGreat, &crops.great)?,
                good: observe(ScreenTextField::ResultGood, &crops.good)?,
                bad: observe(ScreenTextField::ResultBad, &crops.bad)?,
                poor: observe(ScreenTextField::ResultPoor, &crops.poor)?,
                fast: observe(ScreenTextField::ResultFast, &crops.fast)?,
                slow: observe(ScreenTextField::ResultSlow, &crops.slow)?,
                combo_break: observe(ScreenTextField::ResultComboBreak, &crops.combo_break)?,
                play_options: {
                    let raw = observe(ScreenTextField::ResultPlayOptions, &crops.play_options)?;
                    observe_play_options(&crops.play_options, &raw)
                },
            })
        }
        ScreenRgb8Crops::MusicSelect(crops) => {
            ScreenFieldObservations::MusicSelect(MusicSelectScreenFieldObservations {
                best: MusicSelectBestObservation::default(),
                central_title: observe(
                    ScreenTextField::MusicSelectCentralTitle,
                    &crops.central_title,
                )?,
                artist: observe(ScreenTextField::MusicSelectArtist, &crops.artist)?,
                play_type: observe_music_select_play_type(&crops.play_type)
                    .expect("the embedded music-select play-type contract is statically valid"),
                selected_difficulty: observe_music_select_difficulty(&crops.difficulty_markers),
                play_side: observe_music_select_play_side(&crops.play_side),
                active_list_title: observe(
                    ScreenTextField::MusicSelectActiveListTitle,
                    &crops.active_list_title,
                )?,
            })
        }
    })
}

/// Combines one specialist numeric batch with the independently registered result text fields.
///
/// # Errors
/// Returns the exact failed text field without running PP-OCR for any numeric ROI.
pub fn observe_result_fields_with_numeric<E>(
    crops: &ResultScreenRgb8Crops,
    numeric: &NumericBatchInference,
    mut observe_text: impl FnMut(ScreenTextField, &Rgb8Crop) -> Result<DynamicTextObservation, E>,
) -> Result<ResultScreenFieldObservations, ScreenFieldObservationError<E>> {
    let mut observe = |field, crop| {
        observe_text(field, crop).map_err(|source| ScreenFieldObservationError::new(field, source))
    };
    Ok(ResultScreenFieldObservations {
        panel_side: crops.panel_side,
        title: observe(ScreenTextField::ResultTitle, &crops.title)?,
        artist: observe(ScreenTextField::ResultArtist, &crops.artist)?,
        clear_type: observe(ScreenTextField::ResultClearType, &crops.clear_type)?,
        difficulty: observe(ScreenTextField::ResultDifficulty, &crops.difficulty)?,
        play_type: observe(ScreenTextField::ResultPlayType, &crops.play_type)?,
        level: numeric.text_observation(NumericField::Level),
        notes: numeric.text_observation(NumericField::Notes),
        current_score: numeric.text_observation(NumericField::CurrentScore),
        previous_clear_type: observe(
            ScreenTextField::ResultPreviousClearType,
            &crops.previous_clear_type,
        )?,
        previous_score: numeric.text_observation(NumericField::PreviousScore),
        previous_miss_count: numeric.text_observation(NumericField::PreviousMissCount),
        miss_count: numeric.text_observation(NumericField::MissCount),
        pgreat: numeric.text_observation(NumericField::Pgreat),
        great: numeric.text_observation(NumericField::Great),
        good: numeric.text_observation(NumericField::Good),
        bad: numeric.text_observation(NumericField::Bad),
        poor: numeric.text_observation(NumericField::Poor),
        fast: numeric.text_observation(NumericField::Fast),
        slow: numeric.text_observation(NumericField::Slow),
        combo_break: numeric.text_observation(NumericField::ComboBreak),
        play_options: PlayOptionsObservation::default(),
    })
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntegratedContextTextObservation {
    pub field: IntegratedContextField,
    pub crop_file_sha256: String,
    pub input_width: usize,
    pub input_tensor_sha256: String,
    pub output_timesteps: usize,
    pub open_text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegratedChartContextState {
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegratedChartContextUnknownReason {
    ObserverNotImplemented,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntegratedChartContextEvidence {
    pub field: IntegratedContextField,
    pub crop_file_sha256: String,
    pub pixel_sha256: String,
    pub state: IntegratedChartContextState,
    pub reason: IntegratedChartContextUnknownReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegratedContextRecordingCompleteness {
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntegratedContextObservationArtifact {
    pub schema: &'static str,
    pub recording_completeness: IntegratedContextRecordingCompleteness,
    pub source_manifest_sha256: String,
    pub frame_id: String,
    pub frame_extraction_sha256: String,
    pub canonical_frame_sha256: String,
    pub normalizer_artifact_sha256: String,
    pub canonical_layout_sha256: String,
    pub integrated_context_layout_sha256: String,
    pub screen: ScreenClass,
    pub model_id: String,
    pub model_sha256: String,
    pub dictionary_sha256: String,
    pub preprocessor_id: &'static str,
    pub request_sha256: String,
    pub elapsed_ms: u128,
    pub text_observations: Vec<IntegratedContextTextObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chart_context: Option<IntegratedChartContextEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntegratedContextObservationSummary {
    pub schema: &'static str,
    pub output: PathBuf,
    pub manifest_sha256: String,
    pub screen: ScreenClass,
    pub text_observation_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chart_context_state: Option<IntegratedChartContextState>,
}

#[derive(Serialize)]
struct IntegratedContextDecodeRequest<'a> {
    schema: &'static str,
    rows: Vec<IntegratedContextDecodeRequestRow<'a>>,
}

#[derive(Serialize)]
struct IntegratedContextDecodeRequestRow<'a> {
    path: &'a Path,
    file_sha256: &'a str,
}

#[path = "screen/predicate.rs"]
mod predicate;

use predicate::ScreenPathLayout;
pub use predicate::{
    DecideTransitionPresenceEvidence, MusicSelectPresenceEvidence, PlayBpmEdgePairEvidence,
    PlayPresenceEvidence, ResultPanelPresenceEvidence, ResultPresenceEvidence,
    TitlePresenceEvidence, inspect, inspect_canonical_rgb8,
};

#[path = "screen/export.rs"]
mod export;

use export::horizontal_edge_pixels;
pub(super) use export::read_title_crop_artifact;
pub use export::{
    export_integrated_context_crops, export_music_select_crops, export_result_crops,
    observe_integrated_context, route_screen_rgb8_crops,
};
#[cfg(test)]
use export::{publish_private_manifest, read_integrated_context_crop_artifact};

#[cfg(test)]
#[path = "screen/tests.rs"]
mod tests;
