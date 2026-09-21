use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::catalog::Difficulty;
use crate::frame::{CanonicalFrame, CanonicalLayout, Roi};

#[path = "result/panel.rs"]
pub(super) mod numeric_character_layout;
#[path = "result/numeric/fixed_slot.rs"]
pub(super) mod numeric_fixed_slot;
#[path = "result/numeric/onnx.rs"]
pub(super) mod numeric_onnx;
#[path = "result/play_options.rs"]
pub(super) mod play_options;
#[path = "result/observe.rs"]
pub(super) mod result_fields;
#[path = "result/resolve.rs"]
pub(super) mod result_resolver;
#[path = "screen_reference.rs"]
pub(super) mod screen_reference;

pub use super::music_select::{
    BestClearType, BestNumericObservation, BestValue, MUSIC_SELECT_BEST_LAYOUT,
    MusicSelectBestCrops, MusicSelectBestLayout, MusicSelectBestObservation, MusicSelectBestValues,
    StableBestField, dj_rank, resolve_music_select_best,
};
pub use super::music_select::{
    MUSIC_SELECT_SONG_RESOLVER_ID, MusicSelectCorroboration, MusicSelectSongResolution,
    MusicSelectSongUnknownReason, RankedMusicSelectSongCandidate, resolve_music_select_song,
};
pub use super::music_select::{
    MusicSelectPlayTypeObservation, MusicSelectPlayTypeState, MusicSelectPlayTypeUnknownReason,
    observe_music_select_play_type,
};
pub use super::shared::{
    CatalogCandidateDomain, CatalogCandidateDomainError, CatalogCandidateEvidenceTable,
    CatalogCandidateSongEvidence, CatalogCandidateTextEvidence, CatalogNormalizedSimilarity,
    CatalogPrefixCandidateScore, CatalogTextCandidateScore, EvidenceFamily, JointEvidenceCandidate,
    JointEvidenceObservation, MusicSelectSongCandidateObservation, ResultSongCandidateObservation,
    ScreenCatalogCandidateObservations,
};
pub use super::shared::{
    NUMERIC_BLANK_INDEX, NUMERIC_DICTIONARY, NUMERIC_TOP_CANDIDATES, NumericCalibration,
    NumericCandidate, NumericField, NumericFieldInference, ScoreBreakdownCandidate,
    ScoreBreakdownDecision, rank_numeric_probabilities, rank_numeric_sequences,
    select_score_breakdown,
};
pub use super::title::{
    DIAGNOSTIC_TITLE_COMPARISON_KEY_ID, DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
    DiagnosticTitleCandidate, DiagnosticTitleError, DiagnosticTitleUnknownReason,
    ProvisionalTitleCandidate, ProvisionalTitleCandidateDomain, ProvisionalTitleCandidateSet,
    diagnostic_title_candidate, provisional_title_candidates,
};
pub use numeric_character_layout::{
    NumericCharacterFieldLayout, NumericCharacterLayoutVariant, ResultNumericCharacterLayout,
};
pub use numeric_fixed_slot::{FIXED_SLOT_FEATURE_DIMENSIONS, FIXED_SLOT_PREPROCESSOR_ID};
pub use numeric_onnx::{
    NUMERIC_MODEL_MANIFEST_BYTES, NUMERIC_MODEL_MANIFEST_SHA256, NUMERIC_PREPROCESSOR_ID,
    NumericBatchInference, NumericCellCandidate, NumericCellInference, NumericModelCalibrations,
    NumericModelContract, RegisteredNumericRuntime,
};
pub use play_options::{
    PlayOption, PlayOptionMarkerObservation, PlayOptionMarkerState, PlayOptions,
    PlayOptionsObservation, PlayOptionsUnknownReason, observe_play_options,
};
pub use result_fields::{
    ParsedResultFields, PreviousBest, PreviousBestValue, RESULT_FIELD_RESOLVER_ID,
    RESULT_PERFORMANCE_RESOLVER_ID, ResultChartResolution, ResultChartUnknownReason,
    ResultFieldUnknownReason, ResultFieldValue, ResultJudgments, ResultPerformanceResolution,
    ResultPerformanceUnknownReason, ResultTiming, SupplementalResultValue,
    matching_observed_chart_songs, observed_result_difficulty, resolve_clear_type,
    resolve_result_chart, resolve_result_performance,
};
pub use result_resolver::{
    RESULT_SONG_CHART_ASSISTED_RESOLVER_ID, RESULT_SONG_RESOLVER_ID, RankedResultSongCandidate,
    ResultSongResolution, ResultSongUnknownReason, assist_unknown_result_song_with_chart,
    resolve_result_song,
};

#[must_use]
pub fn normalized_title_key(value: &str) -> String {
    super::title::observe::folded_comparison_key(value)
}
pub use super::title::{
    CatalogTitleDecision, CatalogTitleDecoderError, CatalogTitleDictionaryAudit,
    CatalogTitleUnknownReason, DiagnosticTitleThresholds, TITLE_DICTIONARY_SHA256,
    TitleDictionaryVariantKindAudit, TitleModelExportRequirements, audit_catalog_title_dictionary,
    score_catalog_titles, title_model_export_requirements,
};
pub use super::title::{
    CtcCharacterSet, DynamicOfficialOnnxDecodeSummary, DynamicTextObservation,
    ExportContractParityRequest, ExportContractParitySummary, LIVE_MODEL_BUNDLE_MANIFEST_SHA256,
    LIVE_MODEL_ID, LIVE_MODEL_SHA256, LIVE_RUNTIME_SHA256, OfficialOnnxDecodeSummary,
    OnnxParityError, OnnxParitySummary, OnnxTitleDiagnosticRequest, RegisteredDynamicTitleRuntime,
    RegisteredLiveModelFile, RegisteredRecognitionResources, RegisteredResourceLoadError,
    RegisteredResourceLoadErrorType, compare_export_contract, compare_paddle_onnx,
    decode_dynamic_official_onnx_crops, decode_official_onnx_crops, registered_live_model_files,
    verify_registered_live_model_bundle,
};
pub use super::title::{TITLE_PREPROCESSOR_ID, preprocess_title_crop};

const CANONICAL_WIDTH: u32 = 1_920;
const CANONICAL_HEIGHT: u32 = 1_080;
pub(crate) const CANONICAL_BYTES: usize = CANONICAL_WIDTH as usize * CANONICAL_HEIGHT as usize * 3;
const CANONICAL_FRAME_CONTRACT_ID: &str = "scorepeek-canonical-rgb8-1920x1080-v1";
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

impl From<OnnxParityError> for RecognitionError {
    fn from(error: OnnxParityError) -> Self {
        Self::Onnx(Box::new(error))
    }
}

impl CanonicalFrame {
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
        Ok(Self {
            pixels: pixels.into(),
            source_pts_ms: frame.source_pts,
            decode_index: frame.decode_index,
            capture_profile_id: manifest.capture_profile_id,
            normalizer_artifact_sha256: manifest.normalizer_artifact_sha256,
            frame_extraction_sha256: expected_extraction_sha256.to_owned(),
        })
    }

    /// Copies one layout-bound RGB8 crop in row-major order.
    ///
    /// # Errors
    /// Returns an error when the ROI is outside the canonical frame.
    pub fn crop(&self, roi: Roi) -> Result<Vec<u8>, RecognitionError> {
        crop_canonical_pixels(&self.pixels, roi)
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

impl Roi {
    fn validate(self, width: u32, height: u32) -> Result<(), RecognitionError> {
        if self.width == 0
            || self.height == 0
            || self
                .x
                .checked_add(self.width)
                .is_none_or(|right| right > width)
            || self
                .y
                .checked_add(self.height)
                .is_none_or(|bottom| bottom > height)
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(())
    }

    fn translated_x(self, origin_x: u32) -> Result<Self, RecognitionError> {
        Ok(Self {
            x: self
                .x
                .checked_add(origin_x)
                .ok_or(RecognitionError::InvalidCanonicalLayout)?,
            ..self
        })
    }
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
struct ResultNumericPanelOrigins {
    left: ResultNumericFieldOrigins,
    right: ResultNumericFieldOrigins,
}

impl ResultNumericPanelOrigins {
    const fn get(self, side: ResultPanelSide, field: NumericField) -> u32 {
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
    numeric_panel_origins: ResultNumericPanelOrigins,
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
    selected_difficulty: MusicSelectDifficultyLayout,
    play_side: MusicSelectPlaySideLayout,
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
struct MusicSelectDifficultyLayout {
    predicate_id: String,
    score_min_ppm: u32,
    winner_margin_min_ppm: u32,
    beginner: Roi,
    normal: Roi,
    hyper: Roi,
    another: Roi,
    leggendaria: Roi,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct MusicSelectPlaySideLayout {
    predicate_id: String,
    luma_min: u8,
    bright_pixel_min: u32,
    winner_margin_min: u32,
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

/// Canonical regions whose motion must be reviewed separately before music-select dwell is
/// calibrated.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MusicSelectMotionRegions {
    pub list_titles: Roi,
    pub active_list_title: Roi,
    pub central_title: Roi,
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

/// Every currently measured music-select field crop used by one selection observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectScreenRgb8Crops {
    pub best: MusicSelectBestCrops,
    pub canonical_layout_sha256: String,
    pub integrated_context_layout_sha256: String,
    pub central_title: Rgb8Crop,
    pub artist: Rgb8Crop,
    pub play_type: Rgb8Crop,
    pub difficulty_markers: MusicSelectDifficultyMarkerCrops,
    pub play_side: MusicSelectPlaySideCrops,
    pub active_list_title: Rgb8Crop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectPlaySideCrops {
    one_player: Rgb8Crop,
    two_player: Rgb8Crop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectDifficultyMarkerCrops {
    beginner: Rgb8Crop,
    normal: Rgb8Crop,
    hyper: Rgb8Crop,
    another: Rgb8Crop,
    leggendaria: Rgb8Crop,
}

impl MusicSelectDifficultyMarkerCrops {
    #[must_use]
    pub fn as_slots(&self) -> [(Difficulty, &Rgb8Crop); 5] {
        [
            (Difficulty::Beginner, &self.beginner),
            (Difficulty::Normal, &self.normal),
            (Difficulty::Hyper, &self.hyper),
            (Difficulty::Another, &self.another),
            (Difficulty::Leggendaria, &self.leggendaria),
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicSelectDifficultyUnknownReason {
    NoCandidate,
    MultipleCandidates,
    InsufficientMargin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum MusicSelectDifficultyState {
    Known(Difficulty),
    Unknown(MusicSelectDifficultyUnknownReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectDifficultyMarkerEvidence {
    pub difficulty: Difficulty,
    pub top_edge_ppm: u32,
    pub bottom_edge_ppm: u32,
    pub score_ppm: u32,
    pub qualifies: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectDifficultyObservation {
    pub predicate_id: &'static str,
    pub state: MusicSelectDifficultyState,
    pub winner_score_ppm: u32,
    pub runner_up_score_ppm: u32,
    pub margin_ppm: u32,
    pub slots: [MusicSelectDifficultyMarkerEvidence; 5],
}

impl MusicSelectDifficultyObservation {
    #[must_use]
    pub const fn known(&self) -> Option<Difficulty> {
        match self.state {
            MusicSelectDifficultyState::Known(value) => Some(value),
            MusicSelectDifficultyState::Unknown(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaySide {
    OnePlayer,
    TwoPlayer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicSelectPlaySideUnknownReason {
    NoCandidate,
    MultipleCandidates,
    InsufficientMargin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum MusicSelectPlaySideState {
    Known(PlaySide),
    Unknown(MusicSelectPlaySideUnknownReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectPlaySideEvidence {
    pub play_side: PlaySide,
    pub bright_pixels: u32,
    pub qualifies: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectPlaySideObservation {
    pub predicate_id: &'static str,
    pub state: MusicSelectPlaySideState,
    pub winner_bright_pixels: u32,
    pub runner_up_bright_pixels: u32,
    pub margin: u32,
    pub sides: [MusicSelectPlaySideEvidence; 2],
}

impl Default for MusicSelectPlaySideObservation {
    fn default() -> Self {
        let sides =
            [PlaySide::OnePlayer, PlaySide::TwoPlayer].map(|value| MusicSelectPlaySideEvidence {
                play_side: value,
                bright_pixels: 0,
                qualifies: false,
            });
        Self {
            predicate_id: "scorepeek-music-select-footer-brightness-v1",
            state: MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate),
            winner_bright_pixels: 0,
            runner_up_bright_pixels: 0,
            margin: 0,
            sides,
        }
    }
}

impl MusicSelectPlaySideObservation {
    #[must_use]
    pub const fn known(&self) -> Option<PlaySide> {
        match self.state {
            MusicSelectPlaySideState::Known(value) => Some(value),
            MusicSelectPlaySideState::Unknown(_) => None,
        }
    }
}

#[doc(hidden)]
pub fn test_music_select_play_side(play_side: Option<PlaySide>) -> MusicSelectPlaySideObservation {
    MusicSelectPlaySideObservation {
        state: play_side.map_or(
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate),
            MusicSelectPlaySideState::Known,
        ),
        ..MusicSelectPlaySideObservation::default()
    }
}

#[cfg(test)]
pub(crate) fn test_music_select_difficulty(
    difficulty: Option<Difficulty>,
) -> MusicSelectDifficultyObservation {
    let slots = [
        Difficulty::Beginner,
        Difficulty::Normal,
        Difficulty::Hyper,
        Difficulty::Another,
        Difficulty::Leggendaria,
    ]
    .map(|value| MusicSelectDifficultyMarkerEvidence {
        difficulty: value,
        top_edge_ppm: 0,
        bottom_edge_ppm: 0,
        score_ppm: 0,
        qualifies: false,
    });
    MusicSelectDifficultyObservation {
        predicate_id: "scorepeek-player-marker-outline-v2",
        state: difficulty.map_or(
            MusicSelectDifficultyState::Unknown(MusicSelectDifficultyUnknownReason::NoCandidate),
            MusicSelectDifficultyState::Known,
        ),
        winner_score_ppm: 0,
        runner_up_score_ppm: 0,
        margin_ppm: 0,
        slots,
    }
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

/// Complete music-select field observations from the currently registered observers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectScreenFieldObservations {
    pub best: MusicSelectBestObservation,
    pub central_title: DynamicTextObservation,
    pub artist: DynamicTextObservation,
    pub play_type: MusicSelectPlayTypeObservation,
    pub selected_difficulty: MusicSelectDifficultyObservation,
    pub play_side: MusicSelectPlaySideObservation,
    pub active_list_title: DynamicTextObservation,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultPresenceEvidence {
    pub warm_pixels: u32,
    pub warm_pixels_min: u32,
    pub panel_side: ResultPanelSideState,
    pub panels: [ResultPanelPresenceEvidence; 2],
    pub horizontal_edge_pixels_min: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TitlePresenceEvidence {
    pub bright_bbox: Option<Roi>,
    pub bright_channel_min: u8,
    pub qualifies: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultPanelPresenceEvidence {
    pub panel_side: ResultPanelSide,
    pub upper_panel_edge_pixels: u32,
    pub lower_panel_edge_pixels: u32,
    pub qualifies: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectPresenceEvidence {
    pub cyan_header_pixels: u32,
    pub cyan_header_pixels_min: u32,
    pub colored_level_pixels: u32,
    pub colored_level_pixels_min: u32,
    pub bright_label_pixels: u32,
    pub bright_label_pixels_min: u32,
    pub reference_evaluated: bool,
    pub music_reference_score_ppm: u32,
    pub mode_select_reference_score_ppm: u32,
    pub reference_score_min_ppm: u32,
    pub reference_winner_margin_min_ppm: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct DecideTransitionPresenceEvidence {
    pub cyan_pixels: u32,
    pub cyan_pixels_min: u32,
    pub bright_pixels: u32,
    pub bright_pixels_min: u32,
    pub saturated_pixels: u32,
    pub saturated_pixels_min: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayPresenceEvidence {
    pub qualifying_candidates: u8,
    pub top_edge_runs: u8,
    pub bottom_edge_runs: u8,
    pub candidates: [Option<PlayBpmEdgePairEvidence>; 2],
    pub top_edge_pixels_min: u32,
    pub top_edge_pixels_max: u32,
    pub bottom_edge_pixels_min: u32,
    pub bottom_edge_pixels_max: u32,
    pub vertical_distance_min: u32,
    pub vertical_distance_max: u32,
    pub edge_center_delta_x2_max: u32,
    pub candidate_cluster_delta_x2_max: u32,
    pub candidate_cluster_delta_y_max: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayBpmEdgePairEvidence {
    pub center_x2: u32,
    pub top_y: u32,
    pub top_edge_pixels: u32,
    pub bottom_y: u32,
    pub bottom_edge_pixels: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PlayCyanRun {
    x: u32,
    y: u32,
    pixels: u32,
}

impl CanonicalLayout {
    /// Loads the scorepeek-owned shared game layout embedded in the runtime.
    ///
    /// # Errors
    /// Returns an error when the committed artifact is malformed or outside the canonical frame.
    pub fn load() -> Result<Self, RecognitionError> {
        let layout: Self = serde_json::from_slice(LAYOUT_BYTES)?;
        if layout.schema != LAYOUT_SCHEMA
            || layout.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
            || layout.width != CANONICAL_WIDTH
            || layout.height != CANONICAL_HEIGHT
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        layout.validate_result()?;
        layout.validate_music_select()?;
        Ok(layout)
    }

    fn validate_result(&self) -> Result<(), RecognitionError> {
        for roi in [
            self.result.header,
            self.result.title,
            self.result.artist,
            self.result.difficulty,
            self.result.level,
            self.result.notes,
        ] {
            roi.validate(self.width, self.height)?;
        }
        for side in [ResultPanelSide::Left, ResultPanelSide::Right] {
            let origin_x = self.result.panel_origins.get(side);
            for roi in [
                self.result.upper_panel_edge,
                self.result.lower_panel_edge,
                self.result.clear_type,
                self.result.previous_clear_type,
                self.result.play_options,
            ] {
                roi.translated_x(origin_x)?
                    .validate(self.width, self.height)?;
            }
            for (field, roi) in [
                (NumericField::CurrentScore, self.result.current_score),
                (NumericField::PreviousScore, self.result.previous_score),
                (
                    NumericField::PreviousMissCount,
                    self.result.previous_miss_count,
                ),
                (NumericField::MissCount, self.result.miss_count),
                (NumericField::Pgreat, self.result.pgreat),
                (NumericField::Great, self.result.great),
                (NumericField::Good, self.result.good),
                (NumericField::Bad, self.result.bad),
                (NumericField::Poor, self.result.poor),
                (NumericField::Fast, self.result.fast),
                (NumericField::Slow, self.result.slow),
                (NumericField::ComboBreak, self.result.combo_break),
            ] {
                roi.translated_x(self.result.numeric_panel_origins.get(side, field))?
                    .validate(self.width, self.height)?;
            }
        }
        let header_pixels = self
            .result
            .header
            .width
            .checked_mul(self.result.header.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if self.result.presence.warm_pixels_min == 0
            || self.result.presence.horizontal_edge_pixels_min == 0
            || self.result.presence.warm_pixels_min > header_pixels
            || self.result.panel_origins.left != 0
            || self.result.panel_origins.right != 1_360
            || self.result.numeric_panel_origins.left.score != 0
            || self.result.numeric_panel_origins.left.judgment != 0
            || self.result.numeric_panel_origins.left.timing != 0
            || self.result.numeric_panel_origins.left.combo_break != 0
            || self.result.upper_panel_edge.height != 2
            || self.result.lower_panel_edge.height != 2
            || self.result.presence.horizontal_edge_pixels_min > self.result.upper_panel_edge.width
            || self.result.presence.horizontal_edge_pixels_min > self.result.lower_panel_edge.width
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(())
    }

    fn validate_music_select(&self) -> Result<(), RecognitionError> {
        for roi in [
            self.music_select.header,
            self.music_select.label,
            self.music_select.level_column,
            self.music_select.selected_title,
        ] {
            roi.validate(self.width, self.height)?;
        }
        for roi in self.music_select.list_titles.rois() {
            roi.validate(self.width, self.height)?;
        }
        let header_pixels = self
            .music_select
            .header
            .width
            .checked_mul(self.music_select.header.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let level_pixels = self
            .music_select
            .level_column
            .width
            .checked_mul(self.music_select.level_column.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let label_pixels = self
            .music_select
            .label
            .width
            .checked_mul(self.music_select.label.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if self.music_select.list_titles.slots == 0
            || self.music_select.list_titles.stride_y == 0
            || self.music_select.presence.cyan_header_pixels_min == 0
            || self.music_select.presence.colored_level_pixels_min == 0
            || self.music_select.presence.bright_label_pixels_min == 0
            || self.music_select.presence.cyan_header_pixels_min > header_pixels
            || self.music_select.presence.colored_level_pixels_min > level_pixels
            || self.music_select.presence.bright_label_pixels_min > label_pixels
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(())
    }

    /// Returns the bounded text-presentation regions used by offline music-select motion review.
    ///
    /// # Errors
    /// Returns an error when either committed layout artifact is invalid or their identities do
    /// not agree.
    pub fn music_select_motion_regions() -> Result<MusicSelectMotionRegions, RecognitionError> {
        let canonical = Self::load()?;
        let context = IntegratedContextLayout::load()?;
        let list = canonical.music_select.list_titles;
        let height = list
            .stride_y
            .checked_mul(list.slots.saturating_sub(1))
            .and_then(|offset| offset.checked_add(list.height))
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let list_titles = Roi {
            x: list.x,
            y: list.y,
            width: list.width,
            height,
        };
        list_titles.validate(canonical.width, canonical.height)?;
        Ok(MusicSelectMotionRegions {
            list_titles,
            active_list_title: context.music_select.active_list_title,
            central_title: canonical.music_select.selected_title,
        })
    }

    #[must_use]
    pub fn sha256() -> String {
        encode_sha256(LAYOUT_BYTES)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScreenPathLayout {
    schema: String,
    canonical_frame_contract_id: String,
    width: u32,
    height: u32,
    title: TitleLayout,
    music_select_reference: MusicSelectReferenceLayout,
    decide_transition: DecideTransitionLayout,
    play: PlayLayout,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleLayout {
    version: Roi,
    presence: TitlePresencePredicate,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct MusicSelectReferenceLayout {
    algorithm_id: String,
    search_roi: Roi,
    template_width: u32,
    template_height: u32,
    music_asset_sha256: String,
    mode_asset_sha256: String,
    score_min_ppm: u32,
    winner_margin_min_ppm: u32,
}

impl ScreenPathLayout {
    fn load() -> Result<Self, RecognitionError> {
        let layout: Self = serde_json::from_slice(SCREEN_PATH_LAYOUT_BYTES)?;
        if layout.schema != SCREEN_PATH_LAYOUT_SCHEMA
            || layout.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
            || layout.width != CANONICAL_WIDTH
            || layout.height != CANONICAL_HEIGHT
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        for roi in [
            layout.title.version,
            layout.music_select_reference.search_roi,
            layout.decide_transition.splash,
            layout.play.bpm_outline_search,
        ] {
            roi.validate(layout.width, layout.height)?;
        }
        let title = layout.title.presence;
        if title.bright_channel_min == 0
            || title.bbox_x_min > title.bbox_x_max
            || title.bbox_y_min > title.bbox_y_max
            || title.bbox_width_min == 0
            || title.bbox_width_min > title.bbox_width_max
            || title.bbox_height_min == 0
            || title.bbox_height_min > title.bbox_height_max
            || title.bbox_x_max >= layout.title.version.width
            || title.bbox_y_max >= layout.title.version.height
            || title.bbox_width_max > layout.title.version.width
            || title.bbox_height_max > layout.title.version.height
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        let decide_pixels = layout
            .decide_transition
            .splash
            .width
            .checked_mul(layout.decide_transition.splash.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let reference = &layout.music_select_reference;
        if reference.algorithm_id != "imageproc-cross-correlation-normalized-gray8-v1"
            || reference.template_width == 0
            || reference.template_height == 0
            || reference.template_width > reference.search_roi.width
            || reference.template_height > reference.search_roi.height
            || reference.music_asset_sha256.len() != 64
            || reference.mode_asset_sha256.len() != 64
            || reference.score_min_ppm == 0
            || reference.score_min_ppm > 1_000_000
            || reference.winner_margin_min_ppm == 0
            || reference.winner_margin_min_ppm > 1_000_000
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        let play = layout.play.presence;
        if layout.decide_transition.presence.cyan_pixels_min == 0
            || layout.decide_transition.presence.bright_pixels_min == 0
            || layout.decide_transition.presence.saturated_pixels_min == 0
            || layout.decide_transition.presence.cyan_pixels_min > decide_pixels
            || layout.decide_transition.presence.bright_pixels_min > decide_pixels
            || layout.decide_transition.presence.saturated_pixels_min > decide_pixels
            || play.top_edge_pixels_min == 0
            || play.top_edge_pixels_min > play.top_edge_pixels_max
            || play.bottom_edge_pixels_min == 0
            || play.bottom_edge_pixels_min > play.bottom_edge_pixels_max
            || play.vertical_distance_min == 0
            || play.vertical_distance_min > play.vertical_distance_max
            || play.edge_center_delta_x2_max == 0
            || play.candidate_cluster_delta_x2_max < play.edge_center_delta_x2_max
            || play.candidate_cluster_delta_y_max == 0
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        let play_search = layout.play.bpm_outline_search;
        let pixels = play_search
            .width
            .checked_mul(play_search.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if pixels > 256_000
            || play.top_edge_pixels_max > play_search.width
            || play.bottom_edge_pixels_max > play_search.width
            || play.vertical_distance_max >= play_search.height
            || play.edge_center_delta_x2_max > play_search.width * 2
            || play.candidate_cluster_delta_x2_max > play_search.width * 2
            || play.candidate_cluster_delta_y_max > play_search.height
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(layout)
    }

    fn sha256() -> String {
        encode_sha256(SCREEN_PATH_LAYOUT_BYTES)
    }
}

fn is_play_cyan(pixel: &[u8]) -> bool {
    let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
    g >= 30 && b >= 40 && u16::from(b) * 2 > u16::from(r) * 3 && b > g
}

fn play_cyan_runs(pixels: &[u8], roi: Roi) -> Vec<PlayCyanRun> {
    let mut runs = Vec::new();
    for y in roi.y..roi.y + roi.height {
        let mut x = roi.x;
        while x < roi.x + roi.width {
            let index = (y as usize * CANONICAL_WIDTH as usize + x as usize) * 3;
            if !is_play_cyan(&pixels[index..index + 3]) {
                x += 1;
                continue;
            }
            let start = x;
            x += 1;
            while x < roi.x + roi.width {
                let index = (y as usize * CANONICAL_WIDTH as usize + x as usize) * 3;
                if !is_play_cyan(&pixels[index..index + 3]) {
                    break;
                }
                x += 1;
            }
            runs.push(PlayCyanRun {
                x: start,
                y,
                pixels: x - start,
            });
        }
    }
    runs
}

impl PlayCyanRun {
    const fn center_x2(self) -> u32 {
        self.x * 2 + self.pixels - 1
    }
}

fn play_presence_evidence(
    pixels: &[u8],
    roi: Roi,
    predicate: PlayPresencePredicate,
) -> PlayPresenceEvidence {
    let runs = play_cyan_runs(pixels, roi);
    let top_runs = runs
        .iter()
        .copied()
        .filter(|run| {
            (predicate.top_edge_pixels_min..=predicate.top_edge_pixels_max).contains(&run.pixels)
        })
        .collect::<Vec<_>>();
    let bottom_runs = runs
        .iter()
        .copied()
        .filter(|run| {
            (predicate.bottom_edge_pixels_min..=predicate.bottom_edge_pixels_max)
                .contains(&run.pixels)
        })
        .collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for top in &top_runs {
        for bottom in &bottom_runs {
            let Some(vertical_distance) = bottom.y.checked_sub(top.y) else {
                continue;
            };
            if !(predicate.vertical_distance_min..=predicate.vertical_distance_max)
                .contains(&vertical_distance)
                || top.center_x2().abs_diff(bottom.center_x2()) > predicate.edge_center_delta_x2_max
            {
                continue;
            }
            pairs.push(PlayBpmEdgePairEvidence {
                center_x2: u32::midpoint(top.center_x2(), bottom.center_x2()),
                top_y: top.y,
                top_edge_pixels: top.pixels,
                bottom_y: bottom.y,
                bottom_edge_pixels: bottom.pixels,
            });
        }
    }
    pairs.sort_unstable_by_key(|pair| (pair.center_x2, pair.top_y, pair.bottom_y));
    let mut candidates = [None, None];
    let mut candidate_count = 0_u8;
    let mut cluster_anchor = None;
    for pair in pairs {
        let same_cluster = cluster_anchor.is_some_and(|anchor: PlayBpmEdgePairEvidence| {
            pair.center_x2.abs_diff(anchor.center_x2) <= predicate.candidate_cluster_delta_x2_max
                && pair.top_y.abs_diff(anchor.top_y) <= predicate.candidate_cluster_delta_y_max
                && pair.bottom_y.abs_diff(anchor.bottom_y)
                    <= predicate.candidate_cluster_delta_y_max
        });
        if same_cluster {
            continue;
        }
        if let Some(slot) = candidates.get_mut(usize::from(candidate_count)) {
            *slot = Some(pair);
        }
        candidate_count = candidate_count.saturating_add(1);
        cluster_anchor = Some(pair);
    }
    PlayPresenceEvidence {
        qualifying_candidates: candidate_count,
        top_edge_runs: u8::try_from(top_runs.len()).unwrap_or(u8::MAX),
        bottom_edge_runs: u8::try_from(bottom_runs.len()).unwrap_or(u8::MAX),
        candidates,
        top_edge_pixels_min: predicate.top_edge_pixels_min,
        top_edge_pixels_max: predicate.top_edge_pixels_max,
        bottom_edge_pixels_min: predicate.bottom_edge_pixels_min,
        bottom_edge_pixels_max: predicate.bottom_edge_pixels_max,
        vertical_distance_min: predicate.vertical_distance_min,
        vertical_distance_max: predicate.vertical_distance_max,
        edge_center_delta_x2_max: predicate.edge_center_delta_x2_max,
        candidate_cluster_delta_x2_max: predicate.candidate_cluster_delta_x2_max,
        candidate_cluster_delta_y_max: predicate.candidate_cluster_delta_y_max,
    }
}

fn result_panel_presence(
    pixels: &[u8],
    layout: &ResultLayout,
    side: ResultPanelSide,
) -> Result<ResultPanelPresenceEvidence, RecognitionError> {
    let origin_x = layout.panel_origins.get(side);
    let upper_panel_edge_pixels = horizontal_edge_pixels(
        &crop_canonical_pixels(pixels, layout.upper_panel_edge.translated_x(origin_x)?)?,
        layout.upper_panel_edge.width,
    );
    let lower_panel_edge_pixels = horizontal_edge_pixels(
        &crop_canonical_pixels(pixels, layout.lower_panel_edge.translated_x(origin_x)?)?,
        layout.lower_panel_edge.width,
    );
    Ok(ResultPanelPresenceEvidence {
        panel_side: side,
        upper_panel_edge_pixels,
        lower_panel_edge_pixels,
        qualifies: upper_panel_edge_pixels >= layout.presence.horizontal_edge_pixels_min
            && lower_panel_edge_pixels >= layout.presence.horizontal_edge_pixels_min,
    })
}

fn title_presence_evidence(
    pixels: &[u8],
    layout: &TitleLayout,
) -> Result<TitlePresenceEvidence, RecognitionError> {
    let crop = crop_canonical_pixels(pixels, layout.version)?;
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (index, pixel) in crop.chunks_exact(3).enumerate() {
        if pixel
            .iter()
            .all(|channel| *channel > layout.presence.bright_channel_min)
        {
            let index =
                u32::try_from(index).map_err(|_| RecognitionError::InvalidCanonicalFrame)?;
            let x = index % layout.version.width;
            let y = index / layout.version.width;
            bounds = Some(bounds.map_or((x, y, x, y), |(min_x, min_y, max_x, max_y)| {
                (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
            }));
        }
    }
    let bright_bbox = bounds.map(|(min_x, min_y, max_x, max_y)| Roi {
        x: min_x,
        y: min_y,
        width: max_x - min_x + 1,
        height: max_y - min_y + 1,
    });
    let qualifies = bright_bbox.is_some_and(|bbox| {
        (layout.presence.bbox_x_min..=layout.presence.bbox_x_max).contains(&bbox.x)
            && (layout.presence.bbox_y_min..=layout.presence.bbox_y_max).contains(&bbox.y)
            && (layout.presence.bbox_width_min..=layout.presence.bbox_width_max)
                .contains(&bbox.width)
            && (layout.presence.bbox_height_min..=layout.presence.bbox_height_max)
                .contains(&bbox.height)
    });
    Ok(TitlePresenceEvidence {
        bright_bbox,
        bright_channel_min: layout.presence.bright_channel_min,
        qualifies,
    })
}

/// Inspects one canonical frame without accepting an observed-frame representation.
///
/// # Errors
/// Returns an error when the committed layout or its crop is invalid.
pub fn inspect(frame: &CanonicalFrame) -> Result<RecognitionSnapshot, RecognitionError> {
    let observation = inspect_canonical_rgb8(frame.pixels())?;
    Ok(RecognitionSnapshot {
        schema: "scorepeek-recognition-spike-v4".to_owned(),
        canonical_frame_sha256: encode_sha256(frame.pixels()),
        normalizer_artifact_sha256: frame.normalizer_artifact_sha256.clone(),
        frame_extraction_sha256: frame.frame_extraction_sha256.clone(),
        canonical_layout_sha256: CanonicalLayout::sha256(),
        screen_path_layout_sha256: observation.screen_path_layout_sha256,
        screen: observation.screen,
        title_presence: observation.title_presence,
        result_presence: observation.result_presence,
        music_select_presence: observation.music_select_presence,
        decide_transition_presence: observation.decide_transition_presence,
        play_presence: observation.play_presence,
    })
}

/// Applies only the embedded screen predicates to one fixed-contract canonical RGB8 slice.
///
/// This pure primitive deliberately carries no capture, generation, normalizer, extraction, or
/// model authority. Application code must combine it with a source-bound canonical owner before
/// the result can enter a diagnostic run or later acceptance logic.
///
/// # Errors
/// Returns an error when the pixels do not satisfy the fixed canonical byte contract or the
/// committed layout is invalid.
#[allow(
    clippy::too_many_lines,
    reason = "all independent predicates remain together so classification is based on one measurement pass"
)]
pub fn inspect_canonical_rgb8(
    pixels: &[u8],
) -> Result<ScreenPredicateObservation, RecognitionError> {
    if pixels.len() != CANONICAL_BYTES {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let layout = CanonicalLayout::load()?;
    let screen_path_layout = ScreenPathLayout::load()?;
    let title_presence = title_presence_evidence(pixels, &screen_path_layout.title)?;
    let header = crop_canonical_pixels(pixels, layout.result.header)?;
    let mut warm = 0_u32;
    for pixel in header.chunks_exact(3) {
        let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
        if r > 100 && g > 70 && b < 170 && r >= g && g >= b {
            warm += 1;
        }
    }
    let result_panels = [
        result_panel_presence(pixels, &layout.result, ResultPanelSide::Left)?,
        result_panel_presence(pixels, &layout.result, ResultPanelSide::Right)?,
    ];
    let qualifying_result_panels = result_panels
        .iter()
        .filter(|panel| panel.qualifies)
        .collect::<Vec<_>>();
    let result_panel_side = match qualifying_result_panels.as_slice() {
        [panel] => ResultPanelSideState::Known(panel.panel_side),
        [] => ResultPanelSideState::Unknown(ResultPanelSideUnknownReason::NoCandidate),
        [_, _, ..] => {
            ResultPanelSideState::Unknown(ResultPanelSideUnknownReason::MultipleCandidates)
        }
    };
    let music_header = crop_canonical_pixels(pixels, layout.music_select.header)?;
    let cyan_header_pixels = music_header
        .chunks_exact(3)
        .filter(|pixel| {
            let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
            g > 120 && b > 150 && u16::from(b) * 2 > u16::from(r) * 3
        })
        .fold(0_u32, |count, _| count + 1);
    let level_column = crop_canonical_pixels(pixels, layout.music_select.level_column)?;
    let colored_level_pixels = level_column
        .chunks_exact(3)
        .filter(|pixel| {
            let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
            let maximum = r.max(g).max(b);
            let minimum = r.min(g).min(b);
            maximum > 130 && maximum - minimum > 60
        })
        .fold(0_u32, |count, _| count + 1);
    let music_label = crop_canonical_pixels(pixels, layout.music_select.label)?;
    let bright_label_pixels = music_label
        .chunks_exact(3)
        .filter(|pixel| pixel[0] > 178 && pixel[1] > 178 && pixel[2] > 178)
        .fold(0_u32, |count, _| count + 1);
    let decide_splash = crop_canonical_pixels(pixels, screen_path_layout.decide_transition.splash)?;
    let mut decide_cyan_pixels = 0_u32;
    let mut decide_bright_pixels = 0_u32;
    let mut decide_saturated_pixels = 0_u32;
    for pixel in decide_splash.chunks_exact(3) {
        let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
        if g > 120 && b > 150 && u16::from(b) * 2 > u16::from(r) * 3 {
            decide_cyan_pixels += 1;
        }
        if r > 178 && g > 178 && b > 178 {
            decide_bright_pixels += 1;
        }
        if r.max(g).max(b) > 130 && r.max(g).max(b) - r.min(g).min(b) > 60 {
            decide_saturated_pixels += 1;
        }
    }
    let play_presence = play_presence_evidence(
        pixels,
        screen_path_layout.play.bpm_outline_search,
        screen_path_layout.play.presence,
    );
    let result_present =
        warm >= layout.result.presence.warm_pixels_min && result_panel_side.known().is_some();
    let aggregate_music_select_present = cyan_header_pixels
        >= layout.music_select.presence.cyan_header_pixels_min
        && colored_level_pixels >= layout.music_select.presence.colored_level_pixels_min
        && bright_label_pixels >= layout.music_select.presence.bright_label_pixels_min;
    let reference_scores = aggregate_music_select_present
        .then(|| {
            screen_reference::score(
                pixels,
                &screen_reference::ReferenceContract {
                    search_roi: screen_path_layout.music_select_reference.search_roi,
                    template_width: screen_path_layout.music_select_reference.template_width,
                    template_height: screen_path_layout.music_select_reference.template_height,
                    music_asset_sha256: &screen_path_layout
                        .music_select_reference
                        .music_asset_sha256,
                    mode_asset_sha256: &screen_path_layout.music_select_reference.mode_asset_sha256,
                },
            )
        })
        .transpose()?;
    let music_select_present = reference_scores.is_some_and(|scores| {
        scores.music_ppm >= screen_path_layout.music_select_reference.score_min_ppm
            && scores.music_ppm.saturating_sub(scores.mode_select_ppm)
                >= screen_path_layout
                    .music_select_reference
                    .winner_margin_min_ppm
    });
    let mode_select_present = reference_scores.is_some_and(|scores| {
        scores.mode_select_ppm >= screen_path_layout.music_select_reference.score_min_ppm
            && scores.mode_select_ppm.saturating_sub(scores.music_ppm)
                >= screen_path_layout
                    .music_select_reference
                    .winner_margin_min_ppm
    });
    let decide_transition_present = decide_cyan_pixels
        >= screen_path_layout
            .decide_transition
            .presence
            .cyan_pixels_min
        && decide_bright_pixels
            >= screen_path_layout
                .decide_transition
                .presence
                .bright_pixels_min
        && decide_saturated_pixels
            >= screen_path_layout
                .decide_transition
                .presence
                .saturated_pixels_min;
    let play_present = play_presence.qualifying_candidates == 1;
    let screen = match [
        (result_present, ScreenClass::Result),
        (music_select_present, ScreenClass::MusicSelect),
        (mode_select_present, ScreenClass::ModeSelect),
        (decide_transition_present, ScreenClass::DecideTransition),
        (play_present, ScreenClass::Play),
    ]
    .into_iter()
    .filter_map(|(present, screen)| present.then_some(screen))
    .collect::<Vec<_>>()
    .as_slice()
    {
        [screen] => *screen,
        [] | [_, _, ..] => ScreenClass::Unknown,
    };
    Ok(ScreenPredicateObservation {
        screen_path_layout_sha256: ScreenPathLayout::sha256(),
        screen,
        title_presence,
        result_presence: ResultPresenceEvidence {
            warm_pixels: warm,
            warm_pixels_min: layout.result.presence.warm_pixels_min,
            panel_side: result_panel_side,
            panels: result_panels,
            horizontal_edge_pixels_min: layout.result.presence.horizontal_edge_pixels_min,
        },
        music_select_presence: MusicSelectPresenceEvidence {
            cyan_header_pixels,
            cyan_header_pixels_min: layout.music_select.presence.cyan_header_pixels_min,
            colored_level_pixels,
            colored_level_pixels_min: layout.music_select.presence.colored_level_pixels_min,
            bright_label_pixels,
            bright_label_pixels_min: layout.music_select.presence.bright_label_pixels_min,
            reference_evaluated: reference_scores.is_some(),
            music_reference_score_ppm: reference_scores.map_or(0, |scores| scores.music_ppm),
            mode_select_reference_score_ppm: reference_scores
                .map_or(0, |scores| scores.mode_select_ppm),
            reference_score_min_ppm: screen_path_layout.music_select_reference.score_min_ppm,
            reference_winner_margin_min_ppm: screen_path_layout
                .music_select_reference
                .winner_margin_min_ppm,
        },
        decide_transition_presence: DecideTransitionPresenceEvidence {
            cyan_pixels: decide_cyan_pixels,
            cyan_pixels_min: screen_path_layout
                .decide_transition
                .presence
                .cyan_pixels_min,
            bright_pixels: decide_bright_pixels,
            bright_pixels_min: screen_path_layout
                .decide_transition
                .presence
                .bright_pixels_min,
            saturated_pixels: decide_saturated_pixels,
            saturated_pixels_min: screen_path_layout
                .decide_transition
                .presence
                .saturated_pixels_min,
        },
        play_presence,
    })
}

fn horizontal_edge_pixels(pixels: &[u8], width: u32) -> u32 {
    let row_bytes = width as usize * 3;
    pixels[..row_bytes]
        .chunks_exact(3)
        .zip(pixels[row_bytes..].chunks_exact(3))
        .filter(|(upper, lower)| {
            let luma = |pixel: &[u8]| {
                (u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29)
                    / 256
            };
            luma(upper).abs_diff(luma(lower)) > 45
        })
        .fold(0, |count, _| count + 1)
}

pub(in crate::recognition) fn crop_canonical_pixels(
    pixels: &[u8],
    roi: Roi,
) -> Result<Vec<u8>, RecognitionError> {
    roi.validate(CANONICAL_WIDTH, CANONICAL_HEIGHT)?;
    if pixels.len() != CANONICAL_BYTES {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let row_bytes = roi.width as usize * 3;
    let mut crop = Vec::with_capacity(row_bytes * roi.height as usize);
    for y in roi.y..roi.y + roi.height {
        let start = (y as usize * CANONICAL_WIDTH as usize + roi.x as usize) * 3;
        crop.extend_from_slice(&pixels[start..start + row_bytes]);
    }
    Ok(crop)
}

fn result_crop_selections(
    routed: ResultScreenRgb8Crops,
) -> [(ResultCropField, &'static str, Rgb8Crop); 20] {
    [
        (ResultCropField::Title, "title.ppm", routed.title),
        (ResultCropField::Artist, "artist.ppm", routed.artist),
        (
            ResultCropField::ClearType,
            "clear-type.ppm",
            routed.clear_type,
        ),
        (
            ResultCropField::Difficulty,
            "difficulty.ppm",
            routed.difficulty,
        ),
        (ResultCropField::Level, "level.ppm", routed.level),
        (ResultCropField::Notes, "notes.ppm", routed.notes),
        (
            ResultCropField::CurrentScore,
            "current-score.ppm",
            routed.current_score,
        ),
        (
            ResultCropField::PreviousClearType,
            "previous-clear-type.ppm",
            routed.previous_clear_type,
        ),
        (
            ResultCropField::PreviousScore,
            "previous-score.ppm",
            routed.previous_score,
        ),
        (
            ResultCropField::PreviousMissCount,
            "previous-miss-count.ppm",
            routed.previous_miss_count,
        ),
        (
            ResultCropField::MissCount,
            "miss-count.ppm",
            routed.miss_count,
        ),
        (ResultCropField::Pgreat, "pgreat.ppm", routed.pgreat),
        (ResultCropField::Great, "great.ppm", routed.great),
        (ResultCropField::Good, "good.ppm", routed.good),
        (ResultCropField::Bad, "bad.ppm", routed.bad),
        (ResultCropField::Poor, "poor.ppm", routed.poor),
        (ResultCropField::Fast, "fast.ppm", routed.fast),
        (ResultCropField::Slow, "slow.ppm", routed.slow),
        (
            ResultCropField::ComboBreak,
            "combo-break.ppm",
            routed.combo_break,
        ),
        (
            ResultCropField::PlayOptions,
            "play-options.ppm",
            routed.play_options,
        ),
    ]
}

/// Exports the fixed result-layout crops from a validated canonical frame.
///
/// The output directory must not exist. `manifest.json` is written last, so a partial export is
/// never accepted as a complete crop artifact.
///
/// # Errors
/// Returns an error for a non-result screen, an invalid layout, or any output I/O failure.
pub fn export_result_crops(
    frame: &CanonicalFrame,
    frame_id: &str,
    output: impl AsRef<Path>,
) -> Result<ResultCropExportSummary, RecognitionError> {
    if frame_id.is_empty() || frame_id.len() > 256 || frame_id.chars().any(char::is_control) {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let snapshot = inspect(frame)?;
    if snapshot.screen != ScreenClass::Result {
        return Err(RecognitionError::NotResultScreen);
    }
    let output = output.as_ref();
    fs::create_dir(output)?;

    let ScreenRgb8Crops::Result(routed) = route_screen_rgb8_crops(
        frame.pixels(),
        snapshot
            .crop_route()
            .ok_or(RecognitionError::NotResultScreen)?,
    )?
    else {
        return Err(RecognitionError::NotResultScreen);
    };
    let selections = result_crop_selections(routed);
    let mut crops = Vec::with_capacity(selections.len());
    for (field, filename, crop) in selections {
        let roi = crop.roi;
        let pixels = crop.pixels;
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let mut bytes = Vec::with_capacity(header.len() + pixels.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&pixels);
        write_private_file(&output.join(filename), &bytes)?;
        crops.push(ResultCropEvidence {
            field,
            filename: filename.to_owned(),
            roi,
            pixel_sha256: encode_sha256(&pixels),
            file_sha256: encode_sha256(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    let artifact = ResultCropArtifact {
        schema: "scorepeek-private-canonical-result-crops-v2".to_owned(),
        frame_id: frame_id.to_owned(),
        frame_extraction_sha256: snapshot.frame_extraction_sha256,
        canonical_frame_sha256: snapshot.canonical_frame_sha256,
        normalizer_artifact_sha256: snapshot.normalizer_artifact_sha256,
        canonical_layout_sha256: snapshot.canonical_layout_sha256,
        crops,
    };
    let manifest = canonical_evidence_json(&artifact)?;
    write_private_file(&output.join("manifest.json"), &manifest)?;
    Ok(ResultCropExportSummary {
        schema: "scorepeek-result-crop-export-summary-v1".to_owned(),
        output: output.to_path_buf(),
        manifest_sha256: encode_sha256(&manifest),
    })
}

/// Exports the selected title and every visible music-list title slot from a validated frame.
///
/// List slots are geometric observations. Separators and partially visible rows remain in the
/// artifact and must be rejected by downstream recognition rather than silently omitted.
///
/// # Errors
/// Returns an error for a non-music-select screen, invalid layout, or any output I/O failure.
pub fn export_music_select_crops(
    frame: &CanonicalFrame,
    frame_id: &str,
    output: impl AsRef<Path>,
) -> Result<MusicSelectCropExportSummary, RecognitionError> {
    if frame_id.is_empty() || frame_id.len() > 256 || frame_id.chars().any(char::is_control) {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let snapshot = inspect(frame)?;
    if snapshot.screen != ScreenClass::MusicSelect {
        return Err(RecognitionError::NotMusicSelectScreen);
    }
    let output = output.as_ref();
    fs::create_dir(output)?;

    let ScreenRgb8Crops::MusicSelect(routed) = route_screen_rgb8_crops(
        frame.pixels(),
        snapshot
            .crop_route()
            .ok_or(RecognitionError::NotMusicSelectScreen)?,
    )?
    else {
        return Err(RecognitionError::NotMusicSelectScreen);
    };
    let layout = CanonicalLayout::load()?;
    let mut selections = Vec::with_capacity(layout.music_select.list_titles.slots as usize + 1);
    selections.push((
        "selected_title".to_owned(),
        "selected-title.ppm".to_owned(),
        routed.central_title,
    ));
    for (slot, roi) in layout.music_select.list_titles.rois().enumerate() {
        selections.push((
            format!("list_title_{slot:02}"),
            format!("list-title-{slot:02}.ppm"),
            Rgb8Crop {
                roi,
                pixels: crop_canonical_pixels(frame.pixels(), roi)?,
            },
        ));
    }
    let mut crops = Vec::with_capacity(selections.len());
    for (field, filename, crop) in selections {
        let roi = crop.roi;
        let pixels = crop.pixels;
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let mut bytes = Vec::with_capacity(header.len() + pixels.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&pixels);
        write_private_file(&output.join(&filename), &bytes)?;
        crops.push(MusicSelectCropEvidence {
            field,
            filename,
            roi,
            pixel_sha256: encode_sha256(&pixels),
            file_sha256: encode_sha256(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    let artifact = MusicSelectCropArtifact {
        schema: "scorepeek-private-canonical-music-select-crops-v1".to_owned(),
        frame_id: frame_id.to_owned(),
        frame_extraction_sha256: snapshot.frame_extraction_sha256,
        canonical_frame_sha256: snapshot.canonical_frame_sha256,
        normalizer_artifact_sha256: snapshot.normalizer_artifact_sha256,
        canonical_layout_sha256: snapshot.canonical_layout_sha256,
        crops,
    };
    let manifest = canonical_evidence_json(&artifact)?;
    write_private_file(&output.join("manifest.json"), &manifest)?;
    Ok(MusicSelectCropExportSummary {
        schema: "scorepeek-music-select-crop-export-summary-v1".to_owned(),
        output: output.to_path_buf(),
        manifest_sha256: encode_sha256(&manifest),
        list_slot_count: layout.music_select.list_titles.slots,
    })
}

/// Exports only the independently measured fields needed by the first integrated-context slice.
///
/// The context layout is versioned separately so adding these observations does not invalidate the
/// existing result-title and music-list diagnostic artifacts. The output directory must not exist;
/// `manifest.json` is written last.
///
/// # Errors
/// Returns an error for an unknown screen, invalid evidence or layout, or output I/O failure.
pub fn export_integrated_context_crops(
    frame: &CanonicalFrame,
    frame_id: &str,
    output: impl AsRef<Path>,
) -> Result<IntegratedContextCropExportSummary, RecognitionError> {
    if frame_id.is_empty() || frame_id.len() > 256 || frame_id.chars().any(char::is_control) {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let snapshot = inspect(frame)?;
    let routed = route_screen_rgb8_crops(
        frame.pixels(),
        snapshot
            .crop_route()
            .ok_or(RecognitionError::InvalidCanonicalFrame)?,
    )?;
    let output = output.as_ref();
    fs::create_dir(output)?;
    let (integrated_context_layout_sha256, selections) = match routed {
        ScreenRgb8Crops::Title(_) => return Err(RecognitionError::InvalidCanonicalFrame),
        ScreenRgb8Crops::Result(crops) => (
            IntegratedContextLayout::sha256(),
            vec![(IntegratedContextField::ResultArtist, crops.artist)],
        ),
        ScreenRgb8Crops::MusicSelect(crops) => (
            crops.integrated_context_layout_sha256,
            vec![
                (IntegratedContextField::MusicSelectArtist, crops.artist),
                (IntegratedContextField::MusicSelectPlayType, crops.play_type),
                (
                    IntegratedContextField::MusicSelectSelectedChart,
                    Rgb8Crop {
                        roi: IntegratedContextLayout::load()?
                            .music_select
                            .legacy_selected_chart,
                        pixels: crop_canonical_pixels(
                            frame.pixels(),
                            IntegratedContextLayout::load()?
                                .music_select
                                .legacy_selected_chart,
                        )?,
                    },
                ),
                (
                    IntegratedContextField::MusicSelectActiveListTitle,
                    crops.active_list_title,
                ),
            ],
        ),
    };
    let mut crops = Vec::with_capacity(selections.len());
    for (field, crop) in selections {
        let filename = integrated_context_filename(field);
        let pixels = crop.pixels;
        let roi = crop.roi;
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let mut bytes = Vec::with_capacity(header.len() + pixels.len());
        bytes.extend_from_slice(header.as_bytes());
        bytes.extend_from_slice(&pixels);
        write_private_file(&output.join(filename), &bytes)?;
        crops.push(IntegratedContextCropEvidence {
            field,
            filename: filename.to_owned(),
            roi,
            pixel_sha256: encode_sha256(&pixels),
            file_sha256: encode_sha256(&bytes),
            bytes: bytes.len() as u64,
        });
    }
    let screen = snapshot.screen;
    let artifact = IntegratedContextCropArtifact {
        schema: "scorepeek-private-integrated-context-crops-v1".to_owned(),
        frame_id: frame_id.to_owned(),
        frame_extraction_sha256: snapshot.frame_extraction_sha256,
        canonical_frame_sha256: snapshot.canonical_frame_sha256,
        normalizer_artifact_sha256: snapshot.normalizer_artifact_sha256,
        canonical_layout_sha256: snapshot.canonical_layout_sha256,
        integrated_context_layout_sha256,
        screen,
        crops,
    };
    let manifest = canonical_evidence_json(&artifact)?;
    write_private_file(&output.join("manifest.json"), &manifest)?;
    Ok(IntegratedContextCropExportSummary {
        schema: "scorepeek-integrated-context-crop-export-summary-v1".to_owned(),
        output: output.to_path_buf(),
        manifest_sha256: encode_sha256(&manifest),
        screen,
    })
}

fn crop_rgb8(pixels: &[u8], roi: Roi) -> Result<Rgb8Crop, RecognitionError> {
    Ok(Rgb8Crop {
        roi,
        pixels: crop_canonical_pixels(pixels, roi)?,
    })
}

fn route_result_rgb8_crops(
    pixels: &[u8],
    canonical: &CanonicalLayout,
    context: &IntegratedContextLayout,
    panel_side: ResultPanelSide,
) -> Result<ResultScreenRgb8Crops, RecognitionError> {
    let origin_x = canonical.result.panel_origins.get(panel_side);
    let panel = |roi: Roi| roi.translated_x(origin_x);
    let numeric = |field, roi: Roi| {
        roi.translated_x(
            canonical
                .result
                .numeric_panel_origins
                .get(panel_side, field),
        )
    };
    Ok(ResultScreenRgb8Crops {
        canonical_layout_sha256: CanonicalLayout::sha256(),
        panel_side,
        title: crop_rgb8(pixels, canonical.result.title)?,
        artist: crop_rgb8(pixels, context.result.artist)?,
        clear_type: crop_rgb8(pixels, panel(canonical.result.clear_type)?)?,
        difficulty: crop_rgb8(pixels, canonical.result.difficulty)?,
        play_type: crop_rgb8(pixels, context.result.play_type)?,
        level: crop_rgb8(pixels, canonical.result.level)?,
        notes: crop_rgb8(pixels, canonical.result.notes)?,
        current_score: crop_rgb8(
            pixels,
            numeric(NumericField::CurrentScore, canonical.result.current_score)?,
        )?,
        previous_clear_type: crop_rgb8(pixels, panel(canonical.result.previous_clear_type)?)?,
        previous_score: crop_rgb8(
            pixels,
            numeric(NumericField::PreviousScore, canonical.result.previous_score)?,
        )?,
        previous_miss_count: crop_rgb8(
            pixels,
            numeric(
                NumericField::PreviousMissCount,
                canonical.result.previous_miss_count,
            )?,
        )?,
        miss_count: crop_rgb8(
            pixels,
            numeric(NumericField::MissCount, canonical.result.miss_count)?,
        )?,
        pgreat: crop_rgb8(
            pixels,
            numeric(NumericField::Pgreat, canonical.result.pgreat)?,
        )?,
        great: crop_rgb8(
            pixels,
            numeric(NumericField::Great, canonical.result.great)?,
        )?,
        good: crop_rgb8(pixels, numeric(NumericField::Good, canonical.result.good)?)?,
        bad: crop_rgb8(pixels, numeric(NumericField::Bad, canonical.result.bad)?)?,
        poor: crop_rgb8(pixels, numeric(NumericField::Poor, canonical.result.poor)?)?,
        fast: crop_rgb8(pixels, numeric(NumericField::Fast, canonical.result.fast)?)?,
        slow: crop_rgb8(pixels, numeric(NumericField::Slow, canonical.result.slow)?)?,
        combo_break: crop_rgb8(
            pixels,
            numeric(NumericField::ComboBreak, canonical.result.combo_break)?,
        )?,
        play_options: crop_rgb8(pixels, panel(canonical.result.play_options)?)?,
    })
}

/// Routes one already-classified canonical RGB8 frame to all currently measured field crops for
/// that screen.
///
/// This function is synchronous, deterministic, and filesystem-free. Callers retain responsibility
/// for binding the result to capture provenance and for preventing `Unknown` from entering field
/// observation.
///
/// # Errors
/// Returns an error for an unknown screen, invalid canonical pixels, or layout drift.
pub fn route_screen_rgb8_crops(
    pixels: &[u8],
    route: ScreenCropRoute,
) -> Result<ScreenRgb8Crops, RecognitionError> {
    let canonical = CanonicalLayout::load()?;
    let context = IntegratedContextLayout::load()?;
    match route {
        ScreenCropRoute::Title => {
            let path = ScreenPathLayout::load()?;
            Ok(ScreenRgb8Crops::Title(TitleScreenRgb8Crops {
                canonical_layout_sha256: CanonicalLayout::sha256(),
                game_version: crop_rgb8(pixels, path.title.version)?,
            }))
        }
        ScreenCropRoute::Result(panel_side) => Ok(ScreenRgb8Crops::Result(
            route_result_rgb8_crops(pixels, &canonical, &context, panel_side)?,
        )),
        ScreenCropRoute::MusicSelect => {
            Ok(ScreenRgb8Crops::MusicSelect(MusicSelectScreenRgb8Crops {
                best: MusicSelectBestCrops::extract(pixels)?,
                canonical_layout_sha256: CanonicalLayout::sha256(),
                integrated_context_layout_sha256: IntegratedContextLayout::sha256(),
                central_title: crop_rgb8(pixels, canonical.music_select.selected_title)?,
                artist: crop_rgb8(pixels, context.music_select.artist)?,
                play_type: crop_rgb8(pixels, context.music_select.play_type.roi)?,
                difficulty_markers: MusicSelectDifficultyMarkerCrops {
                    beginner: crop_rgb8(pixels, context.music_select.selected_difficulty.beginner)?,
                    normal: crop_rgb8(pixels, context.music_select.selected_difficulty.normal)?,
                    hyper: crop_rgb8(pixels, context.music_select.selected_difficulty.hyper)?,
                    another: crop_rgb8(pixels, context.music_select.selected_difficulty.another)?,
                    leggendaria: crop_rgb8(
                        pixels,
                        context.music_select.selected_difficulty.leggendaria,
                    )?,
                },
                play_side: MusicSelectPlaySideCrops {
                    one_player: crop_rgb8(pixels, context.music_select.play_side.one_player)?,
                    two_player: crop_rgb8(pixels, context.music_select.play_side.two_player)?,
                },
                active_list_title: crop_rgb8(pixels, context.music_select.active_list_title)?,
            }))
        }
    }
}

#[must_use]
/// Evaluates the five fixed `PLAYER 01` marker slots without invoking OCR.
///
/// # Panics
///
/// Panics only if the embedded layout, which is validated by the build's layout contract tests,
/// can no longer be decoded.
pub fn observe_music_select_difficulty(
    crops: &MusicSelectDifficultyMarkerCrops,
) -> MusicSelectDifficultyObservation {
    let layout = IntegratedContextLayout::load()
        .expect("the embedded integrated context layout is statically validated");
    let policy = &layout.music_select.selected_difficulty;
    let slots = crops.as_slots().map(|(difficulty, crop)| {
        let top_edge_ppm = marker_edge_ppm(crop, 36..114, 8, 11);
        let bottom_edge_ppm = marker_edge_ppm(crop, 10..114, 26, 23);
        let score_ppm = top_edge_ppm.min(bottom_edge_ppm);
        MusicSelectDifficultyMarkerEvidence {
            difficulty,
            top_edge_ppm,
            bottom_edge_ppm,
            score_ppm,
            qualifies: score_ppm >= policy.score_min_ppm,
        }
    });
    let mut ranked = slots;
    ranked.sort_by(|left, right| {
        right
            .score_ppm
            .cmp(&left.score_ppm)
            .then_with(|| left.difficulty.cmp(&right.difficulty))
    });
    let winner = ranked[0];
    let runner_up = ranked[1];
    let margin_ppm = winner.score_ppm.saturating_sub(runner_up.score_ppm);
    let qualifying = slots.iter().filter(|slot| slot.qualifies).count();
    let state = match qualifying {
        0 => MusicSelectDifficultyState::Unknown(MusicSelectDifficultyUnknownReason::NoCandidate),
        1 if margin_ppm < policy.winner_margin_min_ppm => MusicSelectDifficultyState::Unknown(
            MusicSelectDifficultyUnknownReason::InsufficientMargin,
        ),
        1 => MusicSelectDifficultyState::Known(winner.difficulty),
        _ => MusicSelectDifficultyState::Unknown(
            MusicSelectDifficultyUnknownReason::MultipleCandidates,
        ),
    };
    MusicSelectDifficultyObservation {
        predicate_id: "scorepeek-player-marker-outline-v2",
        state,
        winner_score_ppm: winner.score_ppm,
        runner_up_score_ppm: runner_up.score_ppm,
        margin_ppm,
        slots,
    }
}

#[must_use]
/// Distinguishes 1P and 2P from the mutually exclusive footer labels on MUSIC SELECT.
///
/// # Panics
/// Panics only if the embedded, statically validated recognition layout cannot be loaded.
pub fn observe_music_select_play_side(
    crops: &MusicSelectPlaySideCrops,
) -> MusicSelectPlaySideObservation {
    let layout = IntegratedContextLayout::load()
        .expect("the embedded integrated context layout is statically validated");
    let policy = &layout.music_select.play_side;
    let mut sides = [
        MusicSelectPlaySideEvidence {
            play_side: PlaySide::OnePlayer,
            bright_pixels: bright_luma_pixels(&crops.one_player, policy.luma_min),
            qualifies: false,
        },
        MusicSelectPlaySideEvidence {
            play_side: PlaySide::TwoPlayer,
            bright_pixels: bright_luma_pixels(&crops.two_player, policy.luma_min),
            qualifies: false,
        },
    ];
    for side in &mut sides {
        side.qualifies = side.bright_pixels >= policy.bright_pixel_min;
    }
    let mut ranked = sides;
    ranked.sort_by(|left, right| {
        right
            .bright_pixels
            .cmp(&left.bright_pixels)
            .then_with(|| left.play_side.cmp(&right.play_side))
    });
    let winner = ranked[0];
    let runner_up = ranked[1];
    let margin = winner.bright_pixels.saturating_sub(runner_up.bright_pixels);
    let qualifying = sides.iter().filter(|side| side.qualifies).count();
    let state = match qualifying {
        0 => MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate),
        1 if margin < policy.winner_margin_min => {
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::InsufficientMargin)
        }
        1 => MusicSelectPlaySideState::Known(winner.play_side),
        _ => {
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::MultipleCandidates)
        }
    };
    MusicSelectPlaySideObservation {
        predicate_id: "scorepeek-music-select-footer-brightness-v1",
        state,
        winner_bright_pixels: winner.bright_pixels,
        runner_up_bright_pixels: runner_up.bright_pixels,
        margin,
        sides,
    }
}

fn bright_luma_pixels(crop: &Rgb8Crop, luma_min: u8) -> u32 {
    u32::try_from(
        crop.pixels
            .chunks_exact(3)
            .filter(|pixel| {
                let luma = u32::from(pixel[0]) * 2_126
                    + u32::from(pixel[1]) * 7_152
                    + u32::from(pixel[2]) * 722;
                luma >= u32::from(luma_min) * 10_000
            })
            .count(),
    )
    .expect("a canonical crop pixel count fits u32")
}

fn marker_edge_ppm(
    crop: &Rgb8Crop,
    columns: std::ops::Range<usize>,
    edge_y: usize,
    inside_y: usize,
) -> u32 {
    let white = |x: usize, y: usize| {
        let offset = (y * crop.roi.width as usize + x) * 3;
        let [r, g, b] = [
            crop.pixels[offset],
            crop.pixels[offset + 1],
            crop.pixels[offset + 2],
        ];
        r.min(g).min(b) >= 180 && r.max(g).max(b) - r.min(g).min(b) <= 45
    };
    let count = columns.len();
    let matched = columns
        .filter(|&x| {
            (white(x, edge_y) || white(x, edge_y + 1))
                && !white(x, inside_y)
                && !white(x, inside_y + 1)
        })
        .count();
    u32::try_from(matched * 1_000_000 / count).expect("a matched fraction is at most one million")
}

const fn integrated_context_filename(field: IntegratedContextField) -> &'static str {
    match field {
        IntegratedContextField::ResultArtist => "result-artist.ppm",
        IntegratedContextField::MusicSelectArtist => "music-select-artist.ppm",
        IntegratedContextField::MusicSelectSelectedChart => "music-select-selected-chart.ppm",
        IntegratedContextField::MusicSelectPlayType => "music-select-play-type.ppm",
        IntegratedContextField::MusicSelectActiveListTitle => "music-select-active-list-title.ppm",
    }
}

/// Runs the selected native dynamic recognizer over text-only integrated-context crops.
///
/// The combined selected-chart crop is deliberately excluded from OCR. Its digest-bound evidence
/// is recorded as unknown until a dedicated chart observer is implemented. The output directory
/// must not exist; `manifest.json` is written last, so its presence denotes a complete run.
///
/// # Errors
/// Returns an error for an unregistered model choice, invalid crop evidence, incomplete model
/// bundle, unexpected ONNX output, or output I/O failure.
#[allow(
    clippy::too_many_lines,
    reason = "the strict integrated-context artifact reader keeps all field bindings together"
)]
pub fn observe_integrated_context(
    crop_directory: impl AsRef<Path>,
    expected_manifest_sha256: &str,
    model_id: &str,
    bundle_path: impl AsRef<Path>,
    output: impl AsRef<Path>,
) -> Result<IntegratedContextObservationSummary, RecognitionError> {
    if model_id != INTEGRATED_CONTEXT_MODEL_ID {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let crop_directory = crop_directory.as_ref();
    let artifact = read_integrated_context_crop_artifact(crop_directory, expected_manifest_sha256)?;
    let text_crops: Vec<_> = artifact
        .crops
        .iter()
        .filter(|crop| {
            !matches!(
                crop.field,
                IntegratedContextField::MusicSelectSelectedChart
                    | IntegratedContextField::MusicSelectPlayType
            )
        })
        .collect();
    // Own the joined paths before serializing references into the decoder request.
    let crop_paths: Vec<_> = text_crops
        .iter()
        .map(|crop| crop_directory.join(&crop.filename))
        .collect();
    let request = IntegratedContextDecodeRequest {
        schema: "scorepeek-private-official-onnx-decode-request-v1",
        rows: text_crops
            .iter()
            .zip(&crop_paths)
            .map(|(crop, path)| IntegratedContextDecodeRequestRow {
                path,
                file_sha256: &crop.file_sha256,
            })
            .collect(),
    };
    let output = output.as_ref();
    if !crop_directory.is_absolute() || !bundle_path.as_ref().is_absolute() || !output.is_absolute()
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    fs::create_dir(output)?;
    let request_bytes = canonical_evidence_json(&request)?;
    let request_path = output.join("decode-request.json");
    write_private_file(&request_path, &request_bytes)?;
    let decoded =
        decode_dynamic_official_onnx_crops(model_id, bundle_path.as_ref(), &request_path)?;
    let row_count = text_crops.len();
    if decoded.input_widths.len() != row_count
        || decoded.input_tensor_sha256s.len() != row_count
        || decoded.output_timesteps.len() != row_count
        || decoded.decoded_text.len() != row_count
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let text_observations = text_crops
        .iter()
        .enumerate()
        .map(|(index, crop)| IntegratedContextTextObservation {
            field: crop.field,
            crop_file_sha256: crop.file_sha256.clone(),
            input_width: decoded.input_widths[index],
            input_tensor_sha256: decoded.input_tensor_sha256s[index].clone(),
            output_timesteps: decoded.output_timesteps[index],
            open_text: decoded.decoded_text[index].clone(),
        })
        .collect();
    let chart_context = artifact
        .crops
        .iter()
        .find(|crop| crop.field == IntegratedContextField::MusicSelectSelectedChart)
        .map(|crop| IntegratedChartContextEvidence {
            field: crop.field,
            crop_file_sha256: crop.file_sha256.clone(),
            pixel_sha256: crop.pixel_sha256.clone(),
            state: IntegratedChartContextState::Unknown,
            reason: IntegratedChartContextUnknownReason::ObserverNotImplemented,
        });
    let chart_context_state = chart_context.as_ref().map(|evidence| evidence.state);
    let observation = IntegratedContextObservationArtifact {
        schema: "scorepeek-private-integrated-context-observation-v1",
        recording_completeness: IntegratedContextRecordingCompleteness::Complete,
        source_manifest_sha256: expected_manifest_sha256.to_owned(),
        frame_id: artifact.frame_id,
        frame_extraction_sha256: artifact.frame_extraction_sha256,
        canonical_frame_sha256: artifact.canonical_frame_sha256,
        normalizer_artifact_sha256: artifact.normalizer_artifact_sha256,
        canonical_layout_sha256: artifact.canonical_layout_sha256,
        integrated_context_layout_sha256: artifact.integrated_context_layout_sha256,
        screen: artifact.screen,
        model_id: decoded.model_id,
        model_sha256: decoded.model_sha256,
        dictionary_sha256: decoded.dictionary_sha256,
        preprocessor_id: decoded.preprocessor_id,
        request_sha256: decoded.request_sha256,
        elapsed_ms: decoded.elapsed_ms,
        text_observations,
        chart_context,
    };
    let manifest = canonical_evidence_json(&observation)?;
    publish_private_manifest(output, &manifest)?;
    Ok(IntegratedContextObservationSummary {
        schema: "scorepeek-integrated-context-observation-summary-v1",
        output: output.to_path_buf(),
        manifest_sha256: encode_sha256(&manifest),
        screen: observation.screen,
        text_observation_count: observation.text_observations.len(),
        chart_context_state,
    })
}

fn read_integrated_context_crop_artifact(
    directory: &Path,
    expected_manifest_sha256: &str,
) -> Result<IntegratedContextCropArtifact, RecognitionError> {
    if !directory.is_absolute()
        || !directory.metadata()?.is_dir()
        || !valid_sha256(expected_manifest_sha256)
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let manifest_bytes = read_bounded_regular(
        &directory.join("manifest.json"),
        MAX_EXTRACTION_MANIFEST_BYTES,
        None,
    )?;
    if encode_sha256(&manifest_bytes) != expected_manifest_sha256 {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let artifact: IntegratedContextCropArtifact = serde_json::from_slice(&manifest_bytes)?;
    if canonical_evidence_json(&artifact)? != manifest_bytes
        || artifact.schema != "scorepeek-private-integrated-context-crops-v1"
        || artifact.frame_id.is_empty()
        || !valid_sha256(&artifact.frame_extraction_sha256)
        || !valid_sha256(&artifact.canonical_frame_sha256)
        || !valid_sha256(&artifact.normalizer_artifact_sha256)
        || artifact.canonical_layout_sha256 != CanonicalLayout::sha256()
        || artifact.integrated_context_layout_sha256 != IntegratedContextLayout::sha256()
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let layout = IntegratedContextLayout::load()?;
    let expected: Vec<_> = match artifact.screen {
        ScreenClass::Result => vec![(
            IntegratedContextField::ResultArtist,
            "result-artist.ppm",
            layout.result.artist,
        )],
        ScreenClass::MusicSelect => vec![
            (
                IntegratedContextField::MusicSelectArtist,
                "music-select-artist.ppm",
                layout.music_select.artist,
            ),
            (
                IntegratedContextField::MusicSelectSelectedChart,
                "music-select-selected-chart.ppm",
                layout.music_select.legacy_selected_chart,
            ),
            (
                IntegratedContextField::MusicSelectPlayType,
                "music-select-play-type.ppm",
                layout.music_select.play_type.roi,
            ),
            (
                IntegratedContextField::MusicSelectActiveListTitle,
                "music-select-active-list-title.ppm",
                layout.music_select.active_list_title,
            ),
        ],
        ScreenClass::Title
        | ScreenClass::ModeSelect
        | ScreenClass::DecideTransition
        | ScreenClass::Play
        | ScreenClass::Unknown => {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
    };
    if artifact.crops.len() != expected.len() {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    for (crop, (field, filename, roi)) in artifact.crops.iter().zip(expected) {
        let expected_bytes = u64::from(roi.width) * u64::from(roi.height) * 3
            + format!("P6\n{} {}\n255\n", roi.width, roi.height).len() as u64;
        if crop.field != field
            || crop.filename != filename
            || crop.roi != roi
            || crop.bytes != expected_bytes
            || !valid_sha256(&crop.file_sha256)
            || !valid_sha256(&crop.pixel_sha256)
        {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let bytes = read_bounded_regular(
            &directory.join(filename),
            expected_bytes,
            Some(expected_bytes),
        )?;
        if encode_sha256(&bytes) != crop.file_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let pixels = bytes
            .strip_prefix(header.as_bytes())
            .ok_or(RecognitionError::InvalidCanonicalFrame)?;
        if encode_sha256(pixels) != crop.pixel_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
    }
    Ok(artifact)
}

#[allow(
    clippy::too_many_lines,
    reason = "strict admission validates every ordered result crop in one versioned contract"
)]
pub(super) fn read_title_crop_artifact(
    directory: &Path,
    expected_manifest_sha256: &str,
) -> Result<(Roi, Vec<u8>), RecognitionError> {
    if !directory.is_absolute()
        || !directory.metadata()?.is_dir()
        || !valid_sha256(expected_manifest_sha256)
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let manifest_bytes = read_bounded_regular(
        &directory.join("manifest.json"),
        MAX_EXTRACTION_MANIFEST_BYTES,
        None,
    )?;
    if encode_sha256(&manifest_bytes) != expected_manifest_sha256 {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let artifact: ResultCropArtifact = serde_json::from_slice(&manifest_bytes)?;
    if canonical_evidence_json(&artifact)? != manifest_bytes
        || artifact.schema != "scorepeek-private-canonical-result-crops-v2"
        || artifact.frame_id.is_empty()
        || !valid_sha256(&artifact.frame_extraction_sha256)
        || !valid_sha256(&artifact.canonical_frame_sha256)
        || !valid_sha256(&artifact.normalizer_artifact_sha256)
        || artifact.canonical_layout_sha256 != CanonicalLayout::sha256()
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let layout = CanonicalLayout::load()?;
    let expected = [
        (ResultCropField::Title, "title.ppm", layout.result.title),
        (ResultCropField::Artist, "artist.ppm", layout.result.artist),
        (
            ResultCropField::ClearType,
            "clear-type.ppm",
            layout.result.clear_type,
        ),
        (
            ResultCropField::Difficulty,
            "difficulty.ppm",
            layout.result.difficulty,
        ),
        (ResultCropField::Level, "level.ppm", layout.result.level),
        (ResultCropField::Notes, "notes.ppm", layout.result.notes),
        (
            ResultCropField::CurrentScore,
            "current-score.ppm",
            layout.result.current_score,
        ),
        (
            ResultCropField::PreviousClearType,
            "previous-clear-type.ppm",
            layout.result.previous_clear_type,
        ),
        (
            ResultCropField::PreviousScore,
            "previous-score.ppm",
            layout.result.previous_score,
        ),
        (
            ResultCropField::PreviousMissCount,
            "previous-miss-count.ppm",
            layout.result.previous_miss_count,
        ),
        (
            ResultCropField::MissCount,
            "miss-count.ppm",
            layout.result.miss_count,
        ),
        (ResultCropField::Pgreat, "pgreat.ppm", layout.result.pgreat),
        (ResultCropField::Great, "great.ppm", layout.result.great),
        (ResultCropField::Good, "good.ppm", layout.result.good),
        (ResultCropField::Bad, "bad.ppm", layout.result.bad),
        (ResultCropField::Poor, "poor.ppm", layout.result.poor),
        (ResultCropField::Fast, "fast.ppm", layout.result.fast),
        (ResultCropField::Slow, "slow.ppm", layout.result.slow),
        (
            ResultCropField::ComboBreak,
            "combo-break.ppm",
            layout.result.combo_break,
        ),
        (
            ResultCropField::PlayOptions,
            "play-options.ppm",
            layout.result.play_options,
        ),
    ];
    if artifact.crops.len() != expected.len() {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let mut title = None;
    for (crop, (field, filename, roi)) in artifact.crops.iter().zip(expected) {
        let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
        let expected_bytes = header.len() as u64 + u64::from(roi.width) * u64::from(roi.height) * 3;
        if crop.field != field
            || crop.filename != filename
            || crop.roi != roi
            || crop.bytes != expected_bytes
            || !valid_sha256(&crop.pixel_sha256)
            || !valid_sha256(&crop.file_sha256)
        {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let bytes = read_bounded_regular(
            &directory.join(filename),
            expected_bytes,
            Some(expected_bytes),
        )?;
        if encode_sha256(&bytes) != crop.file_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let pixels = bytes
            .strip_prefix(header.as_bytes())
            .ok_or(RecognitionError::InvalidCanonicalFrame)?;
        if encode_sha256(pixels) != crop.pixel_sha256 {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        if field == ResultCropField::Title {
            title = Some((roi, pixels.to_vec()));
        }
    }
    title.ok_or(RecognitionError::InvalidCanonicalFrame)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), RecognitionError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn publish_private_manifest(directory: &Path, bytes: &[u8]) -> Result<(), RecognitionError> {
    let manifest = directory.join("manifest.json");
    let staging = directory.join(".manifest.json.scorepeek-staging");
    if let Err(error) = write_private_file(&staging, bytes) {
        let _ = fs::remove_file(&staging);
        return Err(error);
    }
    if let Err(error) = fs::hard_link(&staging, &manifest) {
        let _ = fs::remove_file(&staging);
        return Err(error.into());
    }
    fs::remove_file(&staging)?;
    File::open(directory)?.sync_all()?;
    File::open(
        directory
            .parent()
            .ok_or(RecognitionError::InvalidCanonicalFrame)?,
    )?
    .sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn paint_screen_reference(pixels: &mut [u8], encoded: &[u8]) {
        let (header, reference) = qoi::decode_to_vec(encoded).unwrap();
        assert_eq!((header.width, header.height), (410, 60));
        for y in 0..header.height as usize {
            let source_start = y * header.width as usize * 3;
            let target_start = ((50 + y) * CANONICAL_WIDTH as usize + 50) * 3;
            pixels[target_start..target_start + header.width as usize * 3].copy_from_slice(
                &reference[source_start..source_start + header.width as usize * 3],
            );
        }
    }

    fn paint_music_reference(pixels: &mut [u8]) {
        paint_screen_reference(
            pixels,
            include_bytes!("../../assets/screen-references-v1/music-select.qoi"),
        );
    }

    fn test_frame(pixels: Vec<u8>) -> CanonicalFrame {
        CanonicalFrame {
            pixels: pixels.into(),
            source_pts_ms: 0,
            decode_index: 0,
            capture_profile_id: "0".repeat(64),
            normalizer_artifact_sha256: "1".repeat(64),
            frame_extraction_sha256: "2".repeat(64),
        }
    }

    fn paint_title_bbox(pixels: &mut [u8], filled: bool) {
        let roi = ScreenPathLayout::load().unwrap().title.version;
        let bbox = Roi {
            x: 10,
            y: 9,
            width: 232,
            height: 14,
        };
        for y in bbox.y..bbox.y + bbox.height {
            for x in bbox.x..bbox.x + bbox.width {
                if filled
                    || x == bbox.x
                    || x == bbox.x + bbox.width - 1
                    || y == bbox.y
                    || y == bbox.y + bbox.height - 1
                {
                    let frame_x = usize::try_from(roi.x + x).unwrap();
                    let frame_y = usize::try_from(roi.y + y).unwrap();
                    let offset = (frame_y * CANONICAL_WIDTH as usize + frame_x) * 3;
                    pixels[offset..offset + 3].copy_from_slice(&[255, 255, 255]);
                }
            }
        }
    }

    #[test]
    fn title_presence_uses_the_bright_bounding_box_not_bright_pixel_count() {
        for filled in [false, true] {
            let mut pixels = vec![0; CANONICAL_BYTES];
            paint_title_bbox(&mut pixels, filled);
            let observation = inspect_canonical_rgb8(&pixels).unwrap();
            assert_eq!(observation.screen, ScreenClass::Unknown);
            assert_eq!(
                observation.title_presence.bright_bbox,
                Some(Roi {
                    x: 10,
                    y: 9,
                    width: 232,
                    height: 14,
                })
            );
            assert!(observation.title_presence.qualifies);
        }

        let mut pixels = vec![0; CANONICAL_BYTES];
        paint_title_bbox(&mut pixels, false);
        paint_play_presence(&mut pixels, &ScreenPathLayout::load().unwrap());
        let overlapping = inspect_canonical_rgb8(&pixels).unwrap();
        assert_eq!(overlapping.screen, ScreenClass::Play);
        assert!(overlapping.title_presence.qualifies);

        let mut pixels = vec![0; CANONICAL_BYTES];
        paint_title_bbox(&mut pixels, false);
        let roi = ScreenPathLayout::load().unwrap().title.version;
        let offset = (usize::try_from(roi.y + 8).unwrap() * CANONICAL_WIDTH as usize
            + usize::try_from(roi.x + 10).unwrap())
            * 3;
        pixels[offset..offset + 3].copy_from_slice(&[255, 255, 255]);
        assert!(
            !inspect_canonical_rgb8(&pixels)
                .unwrap()
                .title_presence
                .qualifies
        );
    }

    fn paint_result_presence(pixels: &mut [u8], layout: &CanonicalLayout) {
        paint_result_presence_on(pixels, layout, ResultPanelSide::Left);
    }

    fn paint_result_presence_on(
        pixels: &mut [u8],
        layout: &CanonicalLayout,
        side: ResultPanelSide,
    ) {
        for index in 0..layout.result.presence.warm_pixels_min as usize {
            let x = layout.result.header.x as usize + index % layout.result.header.width as usize;
            let y = layout.result.header.y as usize + index / layout.result.header.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[140, 100, 60]);
        }
        for edge in [
            layout.result.upper_panel_edge,
            layout.result.lower_panel_edge,
        ] {
            let edge = edge
                .translated_x(layout.result.panel_origins.get(side))
                .unwrap();
            for x in 0..layout.result.presence.horizontal_edge_pixels_min as usize {
                let upper = (edge.y as usize * CANONICAL_WIDTH as usize + edge.x as usize + x) * 3;
                let lower = upper + CANONICAL_WIDTH as usize * 3;
                pixels[upper..upper + 3].copy_from_slice(&[0, 0, 0]);
                pixels[lower..lower + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
    }

    fn paint_decide_transition_presence(pixels: &mut [u8], layout: &ScreenPathLayout) {
        let roi = layout.decide_transition.splash;
        let paint = |pixels: &mut [u8], index: usize, color: [u8; 3]| {
            let x = roi.x as usize + index % roi.width as usize;
            let y = roi.y as usize + index / roi.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&color);
        };
        for index in 0..layout.decide_transition.presence.cyan_pixels_min as usize {
            paint(pixels, index, [20, 160, 220]);
        }
        let saturated_remainder = layout
            .decide_transition
            .presence
            .saturated_pixels_min
            .saturating_sub(layout.decide_transition.presence.cyan_pixels_min);
        for index in 0..saturated_remainder as usize {
            paint(
                pixels,
                layout.decide_transition.presence.cyan_pixels_min as usize + index,
                [220, 40, 40],
            );
        }
        for index in 0..layout.decide_transition.presence.bright_pixels_min as usize {
            paint(
                pixels,
                layout.decide_transition.presence.saturated_pixels_min as usize + index,
                [220, 220, 220],
            );
        }
    }

    fn paint_play_presence(pixels: &mut [u8], layout: &ScreenPathLayout) {
        let roi = layout.play.bpm_outline_search;
        paint_play_outline(pixels, roi.x as usize + 606, roi.y as usize + 12);
    }

    fn paint_play_outline(pixels: &mut [u8], origin_x: usize, origin_y: usize) {
        for y in 0..71_usize {
            let ranges = if y < 9 {
                let start = 31 - y;
                [(start, start + 287 + y * 2), (0, 0)]
            } else if y >= 64 {
                let row = y - 64;
                let start = 17 + row;
                [(start, start + 315 - row * 2), (0, 0)]
            } else {
                [(31, 39), (310, 318)]
            };
            for (start, end) in ranges {
                for x in start..end {
                    let frame_x = origin_x + x;
                    let frame_y = origin_y + y;
                    pixels[(frame_y * CANONICAL_WIDTH as usize + frame_x) * 3..][..3]
                        .copy_from_slice(&[20, 100, 150]);
                }
            }
        }
    }

    fn paint_play_edge_pair(pixels: &mut [u8], origin_x: usize, origin_y: usize) {
        for y in 0..6_usize {
            for x in 31..318_usize {
                let index = ((origin_y + y) * CANONICAL_WIDTH as usize + origin_x + x) * 3;
                pixels[index..index + 3].copy_from_slice(&[20, 100, 150]);
            }
        }
        for y in 64..70_usize {
            for x in 17..332_usize {
                let index = ((origin_y + y) * CANONICAL_WIDTH as usize + origin_x + x) * 3;
                pixels[index..index + 3].copy_from_slice(&[20, 100, 150]);
            }
        }
    }

    fn assert_integrated_artifact_is_strict(
        directory: &Path,
        summary: &IntegratedContextCropExportSummary,
        expected: &IntegratedContextCropArtifact,
    ) {
        assert_eq!(
            read_integrated_context_crop_artifact(directory, &summary.manifest_sha256).unwrap(),
            *expected
        );
        let rejected_output = directory.parent().unwrap().join("rejected-observation");
        assert!(
            observe_integrated_context(
                directory,
                &summary.manifest_sha256,
                "unregistered-model",
                directory.parent().unwrap(),
                &rejected_output,
            )
            .is_err()
        );
        assert!(!rejected_output.exists());
        fs::write(directory.join("result-artist.ppm"), b"tampered").unwrap();
        assert!(
            read_integrated_context_crop_artifact(directory, &summary.manifest_sha256).is_err()
        );
    }

    fn assert_selected_active_title_layout(
        canonical: &CanonicalLayout,
        context: &IntegratedContextLayout,
    ) {
        let active_list_slot = canonical.music_select.list_titles.rois().nth(10).unwrap();
        assert_eq!(
            context.music_select.active_list_title,
            Roi {
                x: 1305,
                y: 525,
                width: 505,
                height: 30,
            }
        );
        assert!(context.music_select.active_list_title.x < active_list_slot.x);
        assert!(context.music_select.active_list_title.y >= active_list_slot.y);
        assert!(
            context.music_select.active_list_title.y
                + context.music_select.active_list_title.height
                <= active_list_slot.y + active_list_slot.height
        );
        assert_eq!(
            context.music_select.active_list_title.x + context.music_select.active_list_title.width,
            active_list_slot.x + active_list_slot.width
        );
    }

    #[test]
    fn private_manifest_publication_is_atomic_and_no_clobber() {
        let root = tempdir().unwrap();
        let output = root.path().join("observation");
        fs::create_dir(&output).unwrap();
        publish_private_manifest(&output, b"complete\n").unwrap();
        assert_eq!(
            fs::read(output.join("manifest.json")).unwrap(),
            b"complete\n"
        );
        assert!(publish_private_manifest(&output, b"replacement\n").is_err());
        assert_eq!(
            fs::read(output.join("manifest.json")).unwrap(),
            b"complete\n"
        );
        assert!(!output.join(".manifest.json.scorepeek-staging").exists());
    }

    #[test]
    fn canonical_layout_is_bounded_and_hash_stable() {
        let layout = CanonicalLayout::load().unwrap();
        assert_eq!(
            layout.result.title,
            Roi {
                x: 660,
                y: 950,
                width: 600,
                height: 50
            }
        );
        assert_eq!(
            layout.result.artist,
            Roi {
                x: 650,
                y: 990,
                width: 650,
                height: 40
            }
        );
        assert_eq!(CanonicalLayout::sha256().len(), 64);
    }

    #[test]
    fn crop_uses_canonical_row_major_coordinates() {
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        let offset = (10 * CANONICAL_WIDTH as usize + 20) * 3;
        pixels[offset..offset + 3].copy_from_slice(&[1, 2, 3]);
        let frame = test_frame(pixels);
        assert_eq!(
            frame
                .crop(Roi {
                    x: 20,
                    y: 10,
                    width: 1,
                    height: 1
                })
                .unwrap(),
            [1, 2, 3]
        );
    }

    #[test]
    fn active_title_foreground_uses_fixed_gray_bbox_and_horizontal_margin() {
        let mut pixels = vec![0_u8; 20 * 4 * 3];
        for y in 1..=2 {
            for x in 8..=10 {
                let offset = (y * 20 + x) * 3;
                pixels[offset..offset + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let crop = Rgb8Crop {
            roi: Roi {
                x: 100,
                y: 200,
                width: 20,
                height: 4,
            },
            pixels,
        };
        let (foreground, geometry) = crop.title_foreground_crop().unwrap();
        assert_eq!(
            foreground.roi,
            Roi {
                x: 104,
                y: 200,
                width: 11,
                height: 4
            }
        );
        assert_eq!(
            geometry.bbox,
            Roi {
                x: 108,
                y: 201,
                width: 3,
                height: 2
            }
        );
        assert_eq!(geometry.occupancy_width_ppm, 150_000);
        assert!(!geometry.touches_left_edge);
        assert!(!geometry.touches_right_edge);

        let empty = Rgb8Crop {
            roi: Roi {
                x: 0,
                y: 0,
                width: 2,
                height: 2,
            },
            pixels: vec![0; 12],
        };
        assert!(empty.title_foreground_crop().is_none());
    }

    #[test]
    fn screen_field_observations_keep_complete_screen_specific_shapes() {
        let result_crops = route_screen_rgb8_crops(
            &vec![0; CANONICAL_BYTES],
            ScreenCropRoute::Result(ResultPanelSide::Left),
        )
        .unwrap();
        let mut result_calls = 0;
        let result = observe_screen_fields(&result_crops, |_, crop| {
            result_calls += 1;
            Ok::<_, ()>(DynamicTextObservation {
                input_width: crop.roi.width as usize,
                output_timesteps: result_calls,
                open_text: format!("result-{result_calls}"),
                constrained_text: None,
            })
        })
        .unwrap();
        let ScreenFieldObservations::Result(result) = result else {
            panic!("result crops produced another screen output");
        };
        assert_eq!(result_calls, 21);
        assert_eq!(result.title.open_text, "result-1");
        assert_eq!(result.artist.open_text, "result-2");
        assert_eq!(result.clear_type.open_text, "result-3");
        assert_eq!(result.difficulty.open_text, "result-4");
        assert_eq!(result.play_type.open_text, "result-5");
        assert_eq!(result.level.open_text, "result-6");
        assert_eq!(result.notes.open_text, "result-7");
        assert_eq!(result.current_score.open_text, "result-8");
        assert_eq!(result.previous_clear_type.open_text, "result-9");
        assert_eq!(result.previous_score.open_text, "result-10");
        assert_eq!(result.previous_miss_count.open_text, "result-11");
        assert_eq!(result.miss_count.open_text, "result-12");
        assert_eq!(result.pgreat.open_text, "result-13");
        assert_eq!(result.great.open_text, "result-14");
        assert_eq!(result.good.open_text, "result-15");
        assert_eq!(result.bad.open_text, "result-16");
        assert_eq!(result.poor.open_text, "result-17");
        assert_eq!(result.fast.open_text, "result-18");
        assert_eq!(result.slow.open_text, "result-19");
        assert_eq!(result.combo_break.open_text, "result-20");

        let music_crops =
            route_screen_rgb8_crops(&vec![0; CANONICAL_BYTES], ScreenCropRoute::MusicSelect)
                .unwrap();
        let mut music_calls = 0;
        let music = observe_screen_fields(&music_crops, |_, crop| {
            music_calls += 1;
            Ok::<_, ()>(DynamicTextObservation {
                input_width: crop.roi.width as usize,
                output_timesteps: music_calls,
                open_text: format!("music-{music_calls}"),
                constrained_text: None,
            })
        })
        .unwrap();
        let ScreenFieldObservations::MusicSelect(music) = music else {
            panic!("music-select crops produced another screen output");
        };
        assert_eq!(music_calls, 3);
        assert_eq!(music.central_title.open_text, "music-1");
        assert_eq!(music.artist.open_text, "music-2");
        assert_eq!(
            music.selected_difficulty.state,
            MusicSelectDifficultyState::Unknown(MusicSelectDifficultyUnknownReason::NoCandidate)
        );
        assert_eq!(
            music.play_side.state,
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate)
        );
        assert_eq!(music.active_list_title.open_text, "music-3");
    }

    #[test]
    fn result_crop_router_translates_only_panel_local_fields() {
        let pixels = vec![0; CANONICAL_BYTES];
        let ScreenRgb8Crops::Result(left) =
            route_screen_rgb8_crops(&pixels, ScreenCropRoute::Result(ResultPanelSide::Left))
                .unwrap()
        else {
            unreachable!();
        };
        let ScreenRgb8Crops::Result(right) =
            route_screen_rgb8_crops(&pixels, ScreenCropRoute::Result(ResultPanelSide::Right))
                .unwrap()
        else {
            unreachable!();
        };
        for (left, right) in [
            (&left.title, &right.title),
            (&left.artist, &right.artist),
            (&left.difficulty, &right.difficulty),
            (&left.play_type, &right.play_type),
            (&left.level, &right.level),
            (&left.notes, &right.notes),
        ] {
            assert_eq!(left.roi, right.roi);
        }
        for (left, right) in [
            (&left.clear_type, &right.clear_type),
            (&left.previous_clear_type, &right.previous_clear_type),
            (&left.play_options, &right.play_options),
        ] {
            assert_eq!(right.roi.x, left.roi.x + 1_360);
            assert_eq!(right.roi.y, left.roi.y);
            assert_eq!(right.roi.width, left.roi.width);
            assert_eq!(right.roi.height, left.roi.height);
        }
        for (left, right, origin) in [
            (&left.current_score, &right.current_score, 1_350),
            (&left.previous_score, &right.previous_score, 1_350),
            (&left.previous_miss_count, &right.previous_miss_count, 1_350),
            (&left.miss_count, &right.miss_count, 1_350),
            (&left.pgreat, &right.pgreat, 1_349),
            (&left.great, &right.great, 1_349),
            (&left.good, &right.good, 1_349),
            (&left.bad, &right.bad, 1_349),
            (&left.poor, &right.poor, 1_349),
            (&left.fast, &right.fast, 1_344),
            (&left.slow, &right.slow, 1_344),
            (&left.combo_break, &right.combo_break, 1_350),
        ] {
            assert_eq!(right.roi.x, left.roi.x + origin);
            assert_eq!(right.roi.y, left.roi.y);
            assert_eq!(right.roi.width, left.roi.width);
            assert_eq!(right.roi.height, left.roi.height);
        }
    }

    fn side_crop(bright_pixels: usize) -> Rgb8Crop {
        let roi = Roi {
            x: 0,
            y: 0,
            width: 140,
            height: 18,
        };
        let mut pixels = vec![0; 140 * 18 * 3];
        for pixel in pixels.chunks_exact_mut(3).take(bright_pixels) {
            pixel.fill(200);
        }
        Rgb8Crop { roi, pixels }
    }

    fn side_crops(one_player: usize, two_player: usize) -> MusicSelectPlaySideCrops {
        MusicSelectPlaySideCrops {
            one_player: side_crop(one_player),
            two_player: side_crop(two_player),
        }
    }

    #[test]
    fn music_select_footer_resolves_each_play_side() {
        assert_eq!(
            observe_music_select_play_side(&side_crops(380, 250)).known(),
            Some(PlaySide::OnePlayer)
        );
        assert_eq!(
            observe_music_select_play_side(&side_crops(250, 380)).known(),
            Some(PlaySide::TwoPlayer)
        );
    }

    #[test]
    fn music_select_footer_fails_closed_for_absence_multiple_and_small_margin() {
        assert_eq!(
            observe_music_select_play_side(&side_crops(0, 0)).state,
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate)
        );
        assert_eq!(
            observe_music_select_play_side(&side_crops(330, 330)).state,
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::MultipleCandidates)
        );
        assert_eq!(
            observe_music_select_play_side(&side_crops(330, 290)).state,
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::InsufficientMargin)
        );
    }

    fn marker_crop(columns: usize) -> Rgb8Crop {
        let roi = Roi {
            x: 0,
            y: 0,
            width: 128,
            height: 30,
        };
        let mut pixels = vec![0; 128 * 30 * 3];
        for (y, start, width) in [(8, 36, 78), (26, 10, 104)] {
            for x in start..start + width * columns / 100 {
                let offset = (y * 128 + x) * 3;
                pixels[offset..offset + 3].fill(220);
            }
        }
        Rgb8Crop { roi, pixels }
    }

    fn marker_crops(selected: &[Difficulty]) -> MusicSelectDifficultyMarkerCrops {
        let crop = |difficulty| {
            marker_crop(if selected.contains(&difficulty) {
                100
            } else {
                0
            })
        };
        MusicSelectDifficultyMarkerCrops {
            beginner: crop(Difficulty::Beginner),
            normal: crop(Difficulty::Normal),
            hyper: crop(Difficulty::Hyper),
            another: crop(Difficulty::Another),
            leggendaria: crop(Difficulty::Leggendaria),
        }
    }

    #[test]
    fn fixed_music_select_marker_resolves_each_single_slot() {
        for difficulty in [
            Difficulty::Beginner,
            Difficulty::Normal,
            Difficulty::Hyper,
            Difficulty::Another,
            Difficulty::Leggendaria,
        ] {
            assert_eq!(
                observe_music_select_difficulty(&marker_crops(&[difficulty])).known(),
                Some(difficulty)
            );
        }
    }

    #[test]
    fn fixed_music_select_marker_rejects_absence_multiple_and_broad_background_bands() {
        assert_eq!(
            observe_music_select_difficulty(&marker_crops(&[])).known(),
            None
        );
        assert_eq!(
            observe_music_select_difficulty(&marker_crops(&[
                Difficulty::Normal,
                Difficulty::Hyper
            ]))
            .state,
            MusicSelectDifficultyState::Unknown(
                MusicSelectDifficultyUnknownReason::MultipleCandidates
            )
        );
        for rows in [0..30, 8..13, 23..28] {
            let mut crops = marker_crops(&[]);
            for y in rows {
                crops.normal.pixels[y * 128 * 3..(y + 1) * 128 * 3].fill(220);
            }
            assert_eq!(observe_music_select_difficulty(&crops).known(), None);
        }
        let mut crops = marker_crops(&[Difficulty::Hyper]);
        crops.normal.pixels.fill(220);
        assert_eq!(
            observe_music_select_difficulty(&crops).known(),
            Some(Difficulty::Hyper)
        );
    }

    #[test]
    fn fixed_music_select_marker_rejects_insufficient_winner_margin() {
        let mut crops = marker_crops(&[]);
        crops.hyper = marker_crop(85);
        crops.normal = marker_crop(79);
        assert_eq!(
            observe_music_select_difficulty(&crops).state,
            MusicSelectDifficultyState::Unknown(
                MusicSelectDifficultyUnknownReason::InsufficientMargin
            )
        );
    }

    #[test]
    fn failed_text_field_does_not_construct_a_partial_screen_observation() {
        let crops = route_screen_rgb8_crops(
            &vec![0; CANONICAL_BYTES],
            ScreenCropRoute::Result(ResultPanelSide::Left),
        )
        .unwrap();
        let mut calls = 0;
        let error = observe_screen_fields(&crops, |_, _| {
            calls += 1;
            if calls == 2 {
                Err("runtime-failed")
            } else {
                Ok(DynamicTextObservation {
                    input_width: 1,
                    output_timesteps: 1,
                    open_text: "discarded".to_owned(),
                    constrained_text: None,
                })
            }
        })
        .unwrap_err();
        assert_eq!(calls, 2);
        assert_eq!(error.field, ScreenTextField::ResultArtist);
        assert_eq!(error.source_error(), &"runtime-failed");
    }

    #[test]
    fn general_text_numeric_comparison_keeps_its_fixed_character_sets() {
        for field in [
            ScreenTextField::ResultNotes,
            ScreenTextField::ResultCurrentScore,
            ScreenTextField::ResultPgreat,
            ScreenTextField::ResultGreat,
            ScreenTextField::ResultGood,
            ScreenTextField::ResultBad,
            ScreenTextField::ResultPoor,
        ] {
            assert_eq!(field.ctc_character_set(), Some(CtcCharacterSet::Digits));
        }
        assert_eq!(
            ScreenTextField::ResultLevel.ctc_character_set(),
            Some(CtcCharacterSet::DigitsUpToTwo)
        );
        for field in [
            ScreenTextField::ResultPreviousScore,
            ScreenTextField::ResultPreviousMissCount,
            ScreenTextField::ResultMissCount,
            ScreenTextField::ResultFast,
            ScreenTextField::ResultSlow,
        ] {
            assert_eq!(
                field.ctc_character_set(),
                Some(CtcCharacterSet::DigitsAndDashes)
            );
        }
        assert_eq!(
            ScreenTextField::ResultComboBreak.ctc_character_set(),
            Some(CtcCharacterSet::DigitsAndDashesUpToThree)
        );
        assert_eq!(ScreenTextField::ResultTitle.ctc_character_set(), None);
        assert_eq!(
            ScreenTextField::MusicSelectCentralTitle.ctc_character_set(),
            None
        );
    }

    #[test]
    fn result_presence_is_fail_closed() {
        let layout = CanonicalLayout::load().unwrap();
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        paint_result_presence(&mut pixels, &layout);
        let frame = test_frame(pixels);
        let snapshot = inspect(&frame).unwrap();
        assert_eq!(snapshot.screen, ScreenClass::Result);
        assert_eq!(snapshot.result_presence.warm_pixels, 3_000);
        assert_eq!(
            snapshot.result_presence.panel_side.known(),
            Some(ResultPanelSide::Left)
        );
        assert_eq!(
            snapshot.result_presence.panels[0].upper_panel_edge_pixels,
            490
        );
        assert_eq!(
            snapshot.result_presence.panels[0].lower_panel_edge_pixels,
            490
        );

        let mut right = vec![0_u8; CANONICAL_BYTES];
        paint_result_presence_on(&mut right, &layout, ResultPanelSide::Right);
        let right = inspect(&test_frame(right)).unwrap();
        assert_eq!(right.screen, ScreenClass::Result);
        assert_eq!(
            right.result_presence.panel_side.known(),
            Some(ResultPanelSide::Right)
        );

        let mut both = frame.pixels.to_vec();
        paint_result_presence_on(&mut both, &layout, ResultPanelSide::Right);
        let both = inspect(&test_frame(both)).unwrap();
        assert_eq!(both.screen, ScreenClass::Unknown);
        assert_eq!(
            both.result_presence.panel_side,
            ResultPanelSideState::Unknown(ResultPanelSideUnknownReason::MultipleCandidates)
        );

        let empty = test_frame(vec![0_u8; CANONICAL_BYTES]);
        assert_eq!(inspect(&empty).unwrap().screen, ScreenClass::Unknown);

        let mut ambiguous = frame.pixels.to_vec();
        for index in 0..layout.music_select.presence.cyan_header_pixels_min as usize {
            let x = index % 600;
            let y = index / 600;
            ambiguous[(y * CANONICAL_WIDTH as usize + x) * 3..][..3]
                .copy_from_slice(&[20, 160, 220]);
        }
        for index in 0..layout.music_select.presence.colored_level_pixels_min as usize {
            let x = 1_320 + index % 30;
            let y = index / 30;
            ambiguous[(y * CANONICAL_WIDTH as usize + x) * 3..][..3]
                .copy_from_slice(&[20, 180, 40]);
        }
        for index in 0..layout.music_select.presence.bright_label_pixels_min as usize {
            let x = layout.music_select.label.x as usize
                + index % layout.music_select.label.width as usize;
            let y = layout.music_select.label.y as usize
                + index / layout.music_select.label.width as usize;
            ambiguous[(y * CANONICAL_WIDTH as usize + x) * 3..][..3]
                .copy_from_slice(&[220, 220, 220]);
        }
        paint_music_reference(&mut ambiguous);
        assert_eq!(
            inspect(&test_frame(ambiguous)).unwrap().screen,
            ScreenClass::Unknown
        );
    }

    #[test]
    fn result_presence_does_not_depend_on_the_background_palette() {
        let layout = CanonicalLayout::load().unwrap();
        for background in [[0, 0, 0], [170, 20, 20], [20, 80, 190], [220, 220, 220]] {
            let mut pixels = Vec::with_capacity(CANONICAL_BYTES);
            for _ in 0..CANONICAL_BYTES / 3 {
                pixels.extend_from_slice(&background);
            }
            paint_result_presence(&mut pixels, &layout);
            assert_eq!(
                inspect(&test_frame(pixels)).unwrap().screen,
                ScreenClass::Result
            );
        }
    }

    #[test]
    fn decide_transition_and_play_presence_are_exactly_one_fail_closed() {
        let layout = ScreenPathLayout::load().unwrap();
        let mut decide = vec![0_u8; CANONICAL_BYTES];
        paint_decide_transition_presence(&mut decide, &layout);
        let decide = inspect(&test_frame(decide)).unwrap();
        assert_eq!(decide.screen, ScreenClass::DecideTransition);
        assert_eq!(
            decide.decide_transition_presence.cyan_pixels,
            layout.decide_transition.presence.cyan_pixels_min
        );
        assert_eq!(
            decide.decide_transition_presence.bright_pixels,
            layout.decide_transition.presence.bright_pixels_min
        );

        let mut play = vec![0_u8; CANONICAL_BYTES];
        paint_play_presence(&mut play, &layout);
        let play = inspect(&test_frame(play)).unwrap();
        assert_eq!(play.screen, ScreenClass::Play);
        assert_eq!(play.play_presence.qualifying_candidates, 1);
        let outline = play
            .play_presence
            .candidates
            .iter()
            .flatten()
            .next()
            .unwrap();
        assert!((280..=305).contains(&outline.top_edge_pixels));
        assert!((300..=320).contains(&outline.bottom_edge_pixels));
        assert!((59..=70).contains(&(outline.bottom_y - outline.top_y)));

        let mut color_area_only = vec![0_u8; CANONICAL_BYTES];
        let roi = layout.play.bpm_outline_search;
        for y in 0..20_usize {
            for x in 0..220_usize {
                let index =
                    ((roi.y as usize + y) * CANONICAL_WIDTH as usize + roi.x as usize + x) * 3;
                color_area_only[index..index + 3].copy_from_slice(&[20, 100, 150]);
            }
        }
        assert_eq!(
            inspect(&test_frame(color_area_only)).unwrap().screen,
            ScreenClass::Unknown
        );

        let mut former_graph_panel = vec![0_u8; CANONICAL_BYTES];
        for roi in [
            Roi {
                x: 1_505,
                y: 0,
                width: 410,
                height: 24,
            },
            Roi {
                x: 1_508,
                y: 160,
                width: 140,
                height: 22,
            },
        ] {
            for y in roi.y..roi.y + roi.height {
                for x in roi.x..roi.x + roi.width {
                    let index = (y as usize * CANONICAL_WIDTH as usize + x as usize) * 3;
                    former_graph_panel[index..index + 3].copy_from_slice(&[210, 150, 20]);
                }
            }
        }
        assert_eq!(
            inspect(&test_frame(former_graph_panel)).unwrap().screen,
            ScreenClass::Unknown
        );

        let mut overlap = vec![0_u8; CANONICAL_BYTES];
        paint_decide_transition_presence(&mut overlap, &layout);
        paint_play_presence(&mut overlap, &layout);
        assert_eq!(
            inspect(&test_frame(overlap)).unwrap().screen,
            ScreenClass::Unknown
        );
    }

    #[test]
    fn bpm_outline_accepts_all_measured_positions_and_rejects_solid_panels() {
        // Positions measured independently from canonical captures, not derived from the ROI.
        for origin_x in [298, 715, 778, 866, 1283] {
            let mut pixels = vec![0_u8; CANONICAL_BYTES];
            paint_play_outline(&mut pixels, origin_x, 952);
            assert_eq!(
                inspect(&test_frame(pixels)).unwrap().screen,
                ScreenClass::Play
            );

            let mut solid = vec![0_u8; CANONICAL_BYTES];
            for y in 952..1023 {
                for x in origin_x..origin_x + 340 {
                    let index = (y * CANONICAL_WIDTH as usize + x) * 3;
                    solid[index..index + 3].copy_from_slice(&[20, 100, 150]);
                }
            }
            assert_eq!(
                inspect(&test_frame(solid)).unwrap().screen,
                ScreenClass::Unknown
            );
        }
    }

    #[test]
    fn bpm_outline_ignores_connected_interior_judge_pixels() {
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        paint_play_outline(&mut pixels, 715, 952);
        for y in 970..1010 {
            for x in 900..1450 {
                let index = (y * CANONICAL_WIDTH as usize + x) * 3;
                pixels[index..index + 3].copy_from_slice(&[20, 100, 150]);
            }
        }
        let observation = inspect(&test_frame(pixels)).unwrap();
        assert_eq!(observation.screen, ScreenClass::Play);
        assert_eq!(observation.play_presence.qualifying_candidates, 1);
    }

    #[test]
    fn bpm_outline_rejects_simultaneous_left_and_center_right_candidates() {
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        paint_play_outline(&mut pixels, 298, 952);
        paint_play_outline(&mut pixels, 866, 952);
        let observation = inspect(&test_frame(pixels)).unwrap();
        assert_eq!(observation.screen, ScreenClass::Unknown);
        assert_eq!(observation.play_presence.qualifying_candidates, 2);
    }

    #[test]
    fn bpm_outline_rejects_vertically_separated_candidates_at_the_same_center() {
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        paint_play_edge_pair(&mut pixels, 715, 940);
        paint_play_edge_pair(&mut pixels, 715, 1010);
        let observation = inspect(&test_frame(pixels)).unwrap();
        assert_eq!(observation.screen, ScreenClass::Unknown);
        assert_eq!(observation.play_presence.qualifying_candidates, 2);
    }

    #[test]
    fn bpm_outline_ignores_loading_and_variable_tempo_interior() {
        let layout = ScreenPathLayout::load().unwrap();
        let mut loading = vec![0_u8; CANONICAL_BYTES];
        paint_play_presence(&mut loading, &layout);
        assert_eq!(
            inspect(&test_frame(loading.clone())).unwrap().screen,
            ScreenClass::Play
        );

        let roi = layout.play.bpm_outline_search;
        let mut variable_tempo = loading;
        for (x, width) in [(105_u32, 28_u32), (185, 45), (275, 28)] {
            for y in 47..57_u32 {
                for local_x in x..x + width {
                    let frame_x = roi.x + 606 + local_x;
                    let frame_y = roi.y + 12 + y;
                    let index =
                        (frame_y as usize * CANONICAL_WIDTH as usize + frame_x as usize) * 3;
                    variable_tempo[index..index + 3].copy_from_slice(&[210, 210, 210]);
                }
            }
        }
        assert_eq!(
            inspect(&test_frame(variable_tempo)).unwrap().screen,
            ScreenClass::Play
        );
    }

    #[test]
    fn music_select_presence_and_crops_are_fail_closed_and_layout_bound() {
        let layout = CanonicalLayout::load().unwrap();
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        for index in 0..layout.music_select.presence.cyan_header_pixels_min as usize {
            let x = layout.music_select.header.x as usize
                + index % layout.music_select.header.width as usize;
            let y = layout.music_select.header.y as usize
                + index / layout.music_select.header.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[20, 160, 220]);
        }
        for index in 0..layout.music_select.presence.colored_level_pixels_min as usize {
            let x = layout.music_select.level_column.x as usize
                + index % layout.music_select.level_column.width as usize;
            let y = layout.music_select.level_column.y as usize
                + index / layout.music_select.level_column.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[20, 180, 40]);
        }
        assert_eq!(
            inspect(&test_frame(pixels.clone())).unwrap().screen,
            ScreenClass::Unknown
        );
        for index in 0..layout.music_select.presence.bright_label_pixels_min as usize {
            let x = layout.music_select.label.x as usize
                + index % layout.music_select.label.width as usize;
            let y = layout.music_select.label.y as usize
                + index / layout.music_select.label.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[220, 220, 220]);
        }
        let mut mode_select = pixels.clone();
        paint_screen_reference(
            &mut mode_select,
            include_bytes!("../../assets/screen-references-v1/mode-select.qoi"),
        );
        let mode_snapshot = inspect(&test_frame(mode_select)).unwrap();
        assert_eq!(mode_snapshot.screen, ScreenClass::ModeSelect);
        assert!(mode_snapshot.music_select_presence.reference_evaluated);
        assert!(
            mode_snapshot
                .music_select_presence
                .mode_select_reference_score_ppm
                > mode_snapshot
                    .music_select_presence
                    .music_reference_score_ppm
        );
        paint_music_reference(&mut pixels);
        let frame = test_frame(pixels);
        let snapshot = inspect(&frame).unwrap();
        assert_eq!(
            snapshot.screen,
            ScreenClass::MusicSelect,
            "{:?}",
            snapshot.music_select_presence
        );
        assert!(snapshot.music_select_presence.cyan_header_pixels >= 7_000);
        assert_eq!(snapshot.music_select_presence.colored_level_pixels, 1_000);
        assert!(snapshot.music_select_presence.bright_label_pixels >= 4_000);
        assert!(snapshot.music_select_presence.reference_evaluated);
        assert_eq!(
            snapshot.music_select_presence.music_reference_score_ppm,
            1_000_000
        );
        assert!(
            snapshot.music_select_presence.music_reference_score_ppm
                > snapshot
                    .music_select_presence
                    .mode_select_reference_score_ppm
        );

        let directory = tempdir().unwrap();
        let output = directory.path().join("music-select-crops");
        let summary = export_music_select_crops(&frame, "select-001", &output).unwrap();
        let manifest = fs::read(output.join("manifest.json")).unwrap();
        assert_eq!(summary.manifest_sha256, encode_sha256(&manifest));
        assert_eq!(summary.list_slot_count, 20);
        let artifact: MusicSelectCropArtifact = serde_json::from_slice(&manifest).unwrap();
        assert_eq!(
            artifact.schema,
            "scorepeek-private-canonical-music-select-crops-v1"
        );
        assert_eq!(artifact.crops.len(), 21);
        assert_eq!(artifact.crops[0].field, "selected_title");
        assert_eq!(artifact.crops[0].roi, layout.music_select.selected_title);
        assert_eq!(artifact.crops[1].field, "list_title_00");
        assert_eq!(artifact.crops[20].field, "list_title_19");
        assert!(
            export_music_select_crops(
                &test_frame(vec![0; CANONICAL_BYTES]),
                "empty",
                directory.path().join("unknown")
            )
            .is_err()
        );
    }

    #[test]
    fn integrated_context_crops_keep_the_base_layout_stable() {
        let canonical = CanonicalLayout::load().unwrap();
        let context = IntegratedContextLayout::load().unwrap();
        assert_eq!(context.result.artist, canonical.result.artist);
        assert_selected_active_title_layout(&canonical, &context);

        let mut music_select_pixels = vec![0_u8; CANONICAL_BYTES];
        for index in 0..canonical.music_select.presence.cyan_header_pixels_min as usize {
            let x = canonical.music_select.header.x as usize
                + index % canonical.music_select.header.width as usize;
            let y = canonical.music_select.header.y as usize
                + index / canonical.music_select.header.width as usize;
            music_select_pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3]
                .copy_from_slice(&[20, 160, 220]);
        }
        for index in 0..canonical.music_select.presence.colored_level_pixels_min as usize {
            let x = canonical.music_select.level_column.x as usize
                + index % canonical.music_select.level_column.width as usize;
            let y = canonical.music_select.level_column.y as usize
                + index / canonical.music_select.level_column.width as usize;
            music_select_pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3]
                .copy_from_slice(&[20, 180, 40]);
        }
        for index in 0..canonical.music_select.presence.bright_label_pixels_min as usize {
            let x = canonical.music_select.label.x as usize
                + index % canonical.music_select.label.width as usize;
            let y = canonical.music_select.label.y as usize
                + index / canonical.music_select.label.width as usize;
            music_select_pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3]
                .copy_from_slice(&[220, 220, 220]);
        }
        paint_music_reference(&mut music_select_pixels);
        let directory = tempdir().unwrap();
        let music_output = directory.path().join("music-context");
        let music_summary = export_integrated_context_crops(
            &test_frame(music_select_pixels),
            "music-001",
            &music_output,
        )
        .unwrap();
        let music_manifest = fs::read(music_output.join("manifest.json")).unwrap();
        let music_artifact: IntegratedContextCropArtifact =
            serde_json::from_slice(&music_manifest).unwrap();
        assert_eq!(music_summary.screen, ScreenClass::MusicSelect);
        assert_eq!(
            music_summary.manifest_sha256,
            encode_sha256(&music_manifest)
        );
        assert_eq!(music_artifact.screen, ScreenClass::MusicSelect);
        assert_eq!(music_artifact.crops.len(), 4);
        assert_eq!(
            music_artifact.crops[0].field,
            IntegratedContextField::MusicSelectArtist
        );
        assert_eq!(
            music_artifact.crops[1].field,
            IntegratedContextField::MusicSelectPlayType
        );
        assert_eq!(
            music_artifact.crops[2].field,
            IntegratedContextField::MusicSelectSelectedChart
        );
        assert_eq!(
            music_artifact.crops[3].field,
            IntegratedContextField::MusicSelectActiveListTitle
        );

        let mut result_pixels = vec![0_u8; CANONICAL_BYTES];
        paint_result_presence(&mut result_pixels, &canonical);
        let result_output = directory.path().join("result-context");
        let result_summary = export_integrated_context_crops(
            &test_frame(result_pixels),
            "result-001",
            &result_output,
        )
        .unwrap();
        let result_manifest = fs::read(result_output.join("manifest.json")).unwrap();
        let result_artifact: IntegratedContextCropArtifact =
            serde_json::from_slice(&result_manifest).unwrap();
        assert_eq!(result_summary.screen, ScreenClass::Result);
        assert_eq!(result_artifact.crops.len(), 1);
        assert_eq!(
            result_artifact.crops[0].field,
            IntegratedContextField::ResultArtist
        );
        assert_eq!(result_artifact.crops[0].roi, canonical.result.artist);
        assert_integrated_artifact_is_strict(&result_output, &result_summary, &result_artifact);

        assert!(
            export_integrated_context_crops(
                &test_frame(vec![0; CANONICAL_BYTES]),
                "unknown",
                directory.path().join("unknown")
            )
            .is_err()
        );
    }

    #[test]
    fn result_crops_are_layout_bound_and_digest_bound() {
        let layout = CanonicalLayout::load().unwrap();
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        paint_result_presence(&mut pixels, &layout);
        let directory = tempdir().unwrap();
        let output = directory.path().join("crops");
        let summary = export_result_crops(&test_frame(pixels), "result-001", &output).unwrap();
        let manifest = fs::read(output.join("manifest.json")).unwrap();
        assert_eq!(summary.manifest_sha256, encode_sha256(&manifest));
        let artifact: ResultCropArtifactForTest = serde_json::from_slice(&manifest).unwrap();
        assert_eq!(
            artifact.schema,
            "scorepeek-private-canonical-result-crops-v2"
        );
        assert_eq!(artifact.crops.len(), 20);
        assert_eq!(artifact.crops[0].field, "title");
        assert_eq!(artifact.crops[0].roi, layout.result.title);
        assert_eq!(artifact.crops[0].bytes, 600 * 50 * 3 + 14);
        assert_eq!(artifact.crops[0].file_sha256.len(), 64);
        assert_eq!(artifact.crops[0].pixel_sha256.len(), 64);
        assert_eq!(artifact.crops[19].field, "play_options");
        assert_eq!(artifact.crops[19].roi, layout.result.play_options);
        assert!(
            export_result_crops(
                &test_frame(vec![0; CANONICAL_BYTES]),
                "empty",
                directory.path().join("unknown")
            )
            .is_err()
        );
    }

    #[derive(Deserialize)]
    struct ResultCropArtifactForTest {
        schema: String,
        crops: Vec<ResultCropEvidenceForTest>,
    }

    #[derive(Deserialize)]
    struct ResultCropEvidenceForTest {
        field: String,
        roi: Roi,
        pixel_sha256: String,
        file_sha256: String,
        bytes: u64,
    }

    #[test]
    fn canonical_frame_requires_bound_normalizer_evidence() {
        let directory = tempdir().unwrap();
        let time_base = TimeBaseEvidence {
            numerator: 1,
            denominator: 1_000,
        };
        let normalizer = DomainNormalizerEvidence {
            schema: NORMALIZER_SCHEMA.to_owned(),
            capture_profile_id: CALIBRATED_CAPTURE_PROFILE_SHA256.to_owned(),
            observed: ObservedMediaEvidence {
                input_format: "matroska".to_owned(),
                codec_name: "ffv1".to_owned(),
                pixel_format: "yuv420p".to_owned(),
                width: CANONICAL_WIDTH,
                height: CANONICAL_HEIGHT,
                source_time_base: time_base,
                color_range: Some("tv".to_owned()),
                color_space: Some("bt709".to_owned()),
                color_transfer: Some("bt709".to_owned()),
                color_primaries: Some("bt709".to_owned()),
            },
            canonical_frame_contract_id: CANONICAL_FRAME_CONTRACT_ID.to_owned(),
            implementation: NORMALIZER_IMPLEMENTATION.to_owned(),
            ffmpeg_sha256: CALIBRATED_FFMPEG_SHA256.to_owned(),
            filter: NORMALIZER_FILTER.to_owned(),
        };
        let normalizer_bytes = canonical_evidence_json(&normalizer).unwrap();
        fs::write(directory.path().join("normalizer.json"), &normalizer_bytes).unwrap();

        let pixels = vec![0_u8; CANONICAL_BYTES];
        let mut ppm = PPM_HEADER.to_vec();
        ppm.extend_from_slice(&pixels);
        fs::write(directory.path().join("frame-000000.ppm"), &ppm).unwrap();
        let manifest = CanonicalExtractionEvidence {
            schema: EXTRACTION_SCHEMA.to_owned(),
            fixture_id: "fixture-001".to_owned(),
            source_manifest_sha256: "3".repeat(64),
            media_probe_sha256: "4".repeat(64),
            capture_profile_id: CALIBRATED_CAPTURE_PROFILE_SHA256.to_owned(),
            normalizer_artifact_sha256: encode_sha256(&normalizer_bytes),
            canonical_frame_contract_id: CANONICAL_FRAME_CONTRACT_ID.to_owned(),
            extractor: ExtractorEvidence {
                tool_id: "ffmpeg".to_owned(),
                tool_version: FFMPEG_VERSION.to_owned(),
                extractor_manifest_sha256: "4".repeat(64),
                parameters_sha256: "5".repeat(64),
            },
            source_time_base: time_base,
            video_stream_index: 0,
            frames: vec![CanonicalExtractedFrameEvidence {
                frame_id: "result-001".to_owned(),
                source_pts: 1,
                decode_index: 1,
                filename: "frame-000000.ppm".to_owned(),
                frame_sha256: encode_sha256(&pixels),
                file_sha256: encode_sha256(&ppm),
                bytes: ppm.len() as u64,
            }],
        };
        let manifest_bytes = canonical_evidence_json(&manifest).unwrap();
        let manifest_sha256 = encode_sha256(&manifest_bytes);
        fs::write(directory.path().join("manifest.json"), &manifest_bytes).unwrap();

        assert_eq!(
            CanonicalFrame::read_extraction(directory.path(), "result-001", &manifest_sha256)
                .unwrap()
                .pixels(),
            pixels
        );
        assert!(
            CanonicalFrame::read_extraction(directory.path(), "result-001", &"9".repeat(64))
                .is_err(),
            "self-reported evidence without the expected extraction digest must fail"
        );
        let invalid_manifest_sha256 =
            write_invalid_profile_evidence(directory.path(), normalizer.clone(), manifest);
        assert!(
            CanonicalFrame::read_extraction(
                directory.path(),
                "result-001",
                &invalid_manifest_sha256,
            )
            .is_err(),
            "a malformed runtime profile digest must fail even with self-consistent evidence"
        );
        fs::write(directory.path().join("manifest.json"), &manifest_bytes).unwrap();
        fs::write(directory.path().join("normalizer.json"), &normalizer_bytes).unwrap();
        fs::remove_file(directory.path().join("normalizer.json")).unwrap();
        assert!(
            CanonicalFrame::read_extraction(directory.path(), "result-001", &manifest_sha256)
                .is_err(),
            "a bare PPM must not enter recognition"
        );
        fs::write(directory.path().join("normalizer.json"), normalizer_bytes).unwrap();
        ppm.push(0);
        fs::write(directory.path().join("frame-000000.ppm"), ppm).unwrap();
        assert!(
            CanonicalFrame::read_extraction(directory.path(), "result-001", &manifest_sha256)
                .is_err(),
            "an oversized canonical frame must fail before recognition"
        );
    }

    #[test]
    fn canonical_frame_accepts_runtime_admitted_digest_identity() {
        assert!(calibrated_capture_profile(
            CALIBRATED_CAPTURE_PROFILE_SHA256
        ));
        assert!(calibrated_capture_profile(&"e".repeat(64)));
        assert!(!calibrated_capture_profile(&"g".repeat(64)));
    }

    fn write_invalid_profile_evidence(
        directory: &Path,
        normalizer: DomainNormalizerEvidence,
        manifest: CanonicalExtractionEvidence,
    ) -> String {
        write_profile_evidence(directory, normalizer, manifest, &"g".repeat(64))
    }

    fn write_profile_evidence(
        directory: &Path,
        mut normalizer: DomainNormalizerEvidence,
        mut manifest: CanonicalExtractionEvidence,
        capture_profile_id: &str,
    ) -> String {
        normalizer.capture_profile_id = capture_profile_id.to_owned();
        let normalizer_bytes = canonical_evidence_json(&normalizer).unwrap();
        manifest.capture_profile_id = capture_profile_id.to_owned();
        manifest.normalizer_artifact_sha256 = encode_sha256(&normalizer_bytes);
        let manifest_bytes = canonical_evidence_json(&manifest).unwrap();
        let manifest_sha256 = encode_sha256(&manifest_bytes);
        fs::write(directory.join("normalizer.json"), normalizer_bytes).unwrap();
        fs::write(directory.join("manifest.json"), manifest_bytes).unwrap();
        manifest_sha256
    }
}
