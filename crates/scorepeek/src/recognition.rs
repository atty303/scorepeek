use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

mod catalog_candidates;
mod music_select_resolver;
mod result_fields;
mod result_resolver;
mod title;
mod title_decoder;
mod title_onnx;
mod title_preprocessor;

pub use catalog_candidates::{
    CatalogCandidateDomain, CatalogCandidateDomainError, CatalogCandidateEvidenceTable,
    CatalogCandidateSongEvidence, CatalogCandidateTextEvidence, CatalogNormalizedSimilarity,
    CatalogPrefixCandidateScore, CatalogTextCandidateScore, MusicSelectSongCandidateObservation,
    ResultSongCandidateObservation, ScreenCatalogCandidateObservations,
};
pub use music_select_resolver::{
    MUSIC_SELECT_SONG_RESOLVER_ID, MusicSelectCorroboration, MusicSelectSongResolution,
    MusicSelectSongUnknownReason, RankedMusicSelectSongCandidate, resolve_music_select_song,
};
pub use result_fields::{
    ParsedResultFields, RESULT_FIELD_RESOLVER_ID, ResultChartResolution, ResultChartUnknownReason,
    ResultFieldUnknownReason, ResultFieldValue, matching_single_play_songs, resolve_result_chart,
};
pub use result_resolver::{
    RESULT_SONG_CHART_ASSISTED_RESOLVER_ID, RESULT_SONG_RESOLVER_ID, RankedResultSongCandidate,
    ResultSongResolution, ResultSongUnknownReason, assist_unknown_result_song_with_chart,
    resolve_result_song,
};
pub use title::{
    DIAGNOSTIC_TITLE_COMPARISON_KEY_ID, DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
    DiagnosticTitleCandidate, DiagnosticTitleError, DiagnosticTitleUnknownReason,
    ProvisionalTitleCandidate, ProvisionalTitleCandidateDomain, ProvisionalTitleCandidateSet,
    diagnostic_title_candidate, provisional_title_candidates,
};
pub use title_decoder::{
    CatalogTitleDecision, CatalogTitleDecoderError, CatalogTitleDictionaryAudit,
    CatalogTitleUnknownReason, DiagnosticTitleThresholds, TITLE_DICTIONARY_SHA256,
    TitleDictionaryVariantKindAudit, TitleModelExportRequirements, audit_catalog_title_dictionary,
    score_catalog_titles, title_model_export_requirements,
};
pub use title_onnx::{
    DynamicOfficialOnnxDecodeSummary, DynamicTextObservation, ExportContractParityRequest,
    ExportContractParitySummary, LIVE_MODEL_BUNDLE_MANIFEST_SHA256, LIVE_MODEL_ID,
    LIVE_MODEL_SHA256, LIVE_RUNTIME_SHA256, OfficialOnnxDecodeSummary, OnnxParityError,
    OnnxParitySummary, OnnxTitleDiagnosticRequest, RegisteredDynamicTitleRuntime,
    RegisteredLiveModelFile, RegisteredRecognitionResources, RegisteredResourceLoadError,
    RegisteredResourceLoadErrorType, compare_export_contract, compare_paddle_onnx,
    decode_dynamic_official_onnx_crops, decode_official_onnx_crops, registered_live_model_files,
    verify_registered_live_model_bundle,
};
pub use title_preprocessor::{TITLE_PREPROCESSOR_ID, preprocess_title_crop};

const CANONICAL_WIDTH: u32 = 1_920;
const CANONICAL_HEIGHT: u32 = 1_080;
const CANONICAL_BYTES: usize = CANONICAL_WIDTH as usize * CANONICAL_HEIGHT as usize * 3;
const CANONICAL_FRAME_CONTRACT_ID: &str = "scorepeek-canonical-rgb8-1920x1080-v1";
const LAYOUT_SCHEMA: &str = "scorepeek-canonical-layout-v1";
const SCREEN_PATH_LAYOUT_SCHEMA: &str = "scorepeek-screen-path-layout-v1";
const NORMALIZER_SCHEMA: &str = "scorepeek-domain-normalizer-artifact-v1";
const EXTRACTION_SCHEMA: &str = "scorepeek-private-canonical-frame-extraction-v1";
const NORMALIZER_IMPLEMENTATION: &str = "ffmpeg-swscale-bt709-limited-to-rgb24-v1";
const NORMALIZER_FILTER: &str = "scale=1920:1080:flags=bitexact:in_color_matrix=bt709:out_color_matrix=bt709:in_range=tv:out_range=pc,format=rgb24";
const CALIBRATED_CAPTURE_PROFILE_SHA256: &str =
    "d5809dc9b2acc19837260053f4df59a454c9178ae2ac6a0602982effc9da4704";
const CALIBRATED_GAMESCOPE_VKCAPTURE_PROFILE_SHA256: &str =
    "f5f0c5a86b5edba6a8fd014ad85b3873be8f745c0b531d2b5b77f203770b046a";
const CALIBRATED_FFMPEG_SHA256: &str =
    "9eac5b2b5076db5ff853a6fa0dcd6b8de7d0cac8481eadda6c47cd935825f1ee";
const FFMPEG_VERSION: &str = "8.1.2";
const MAX_EXTRACTION_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_NORMALIZER_BYTES: u64 = 64 * 1024;
const PPM_HEADER: &[u8] = b"P6\n1920 1080\n255\n";
const CANONICAL_FILE_BYTES: u64 = CANONICAL_BYTES as u64 + PPM_HEADER.len() as u64;
const LAYOUT_BYTES: &[u8] = include_bytes!("canonical-layout-v1.json");
const SCREEN_PATH_LAYOUT_BYTES: &[u8] = include_bytes!("screen-path-layout-v1.json");
const INTEGRATED_CONTEXT_LAYOUT_BYTES: &[u8] = include_bytes!("integrated-context-layout-v2.json");
const INTEGRATED_CONTEXT_MODEL_ID: &str = "pp-ocrv6-small-rec-onnx-v1";

fn calibrated_capture_profile(profile: &str) -> bool {
    matches!(
        profile,
        CALIBRATED_CAPTURE_PROFILE_SHA256 | CALIBRATED_GAMESCOPE_VKCAPTURE_PROFILE_SHA256
    )
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalFrame {
    pixels: Box<[u8]>,
    source_pts_ms: i64,
    decode_index: u64,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    frame_extraction_sha256: String,
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

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn into_pixels(self) -> Box<[u8]> {
        self.pixels
    }

    #[must_use]
    pub fn capture_profile_id(&self) -> &str {
        &self.capture_profile_id
    }

    #[must_use]
    pub fn source_pts_ms(&self) -> i64 {
        self.source_pts_ms
    }

    #[must_use]
    pub fn decode_index(&self) -> u64 {
        self.decode_index
    }

    #[must_use]
    pub fn normalizer_artifact_sha256(&self) -> &str {
        &self.normalizer_artifact_sha256
    }

    #[must_use]
    pub fn frame_extraction_sha256(&self) -> &str {
        &self.frame_extraction_sha256
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

fn encode_sha256(bytes: &[u8]) -> String {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Roi {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
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
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultLayout {
    presence: ResultPresencePredicate,
    pub header: Roi,
    pub upper_panel_edge: Roi,
    pub lower_panel_edge: Roi,
    pub title: Roi,
    pub artist: Roi,
    pub clear_type: Roi,
    pub difficulty: Roi,
    pub level: Roi,
    pub notes: Roi,
    pub current_score: Roi,
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
    pub lane_edge: Roi,
    pub header: Roi,
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
    cyan_lane_edge_pixels_min: u32,
    warm_header_pixels_min: u32,
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
    Result,
    MusicSelect,
    DecideTransition,
    Play,
    Unknown,
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
    pub result_presence: ResultPresenceEvidence,
    pub music_select_presence: MusicSelectPresenceEvidence,
    pub decide_transition_presence: DecideTransitionPresenceEvidence,
    pub play_presence: PlayPresenceEvidence,
}

/// A pure canonical-RGB8 screen-predicate result without capture or extraction provenance.
///
/// This value is not an accepted live recognition input. The application must bind it to its
/// profile- and generation-bearing live frame before recording or accepting the observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScreenPredicateObservation {
    pub screen_path_layout_sha256: String,
    pub screen: ScreenClass,
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
struct IntegratedContextLayout {
    schema: String,
    canonical_frame_contract_id: String,
    canonical_layout_sha256: String,
    result: ResultContextLayout,
    music_select: MusicSelectContextLayout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ResultContextLayout {
    artist: Roi,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct MusicSelectContextLayout {
    artist: Roi,
    selected_chart: Roi,
    active_list_title: Roi,
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
    fn load() -> Result<Self, RecognitionError> {
        let layout: Self = serde_json::from_slice(INTEGRATED_CONTEXT_LAYOUT_BYTES)?;
        let canonical = CanonicalLayout::load()?;
        let active_list_slot = canonical
            .music_select
            .list_titles
            .rois()
            .nth(10)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if layout.schema != "scorepeek-integrated-context-layout-v2"
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
            layout.music_select.artist,
            layout.music_select.selected_chart,
            layout.music_select.active_list_title,
        ] {
            roi.validate(CANONICAL_WIDTH, CANONICAL_HEIGHT)?;
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
    pixels: Vec<u8>,
}

impl Rgb8Crop {
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
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
    pub title: Rgb8Crop,
    pub artist: Rgb8Crop,
    pub clear_type: Rgb8Crop,
    pub difficulty: Rgb8Crop,
    pub level: Rgb8Crop,
    pub notes: Rgb8Crop,
    pub current_score: Rgb8Crop,
}

/// Every currently measured music-select field crop used by one selection observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectScreenRgb8Crops {
    pub canonical_layout_sha256: String,
    pub integrated_context_layout_sha256: String,
    pub central_title: Rgb8Crop,
    pub artist: Rgb8Crop,
    pub selected_chart: Rgb8Crop,
    pub active_list_title: Rgb8Crop,
}

/// Measured field crops for exactly one classified screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScreenRgb8Crops {
    Result(ResultScreenRgb8Crops),
    MusicSelect(MusicSelectScreenRgb8Crops),
}

/// One text field that can fail without fabricating a partial screen observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScreenTextField {
    ResultTitle,
    ResultArtist,
    ResultClearType,
    ResultDifficulty,
    ResultLevel,
    ResultNotes,
    ResultCurrentScore,
    MusicSelectCentralTitle,
    MusicSelectArtist,
    MusicSelectSelectedChart,
    MusicSelectActiveListTitle,
}

impl ScreenTextField {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResultTitle => "result_title",
            Self::ResultArtist => "result_artist",
            Self::ResultClearType => "result_clear_type",
            Self::ResultDifficulty => "result_difficulty",
            Self::ResultLevel => "result_level",
            Self::ResultNotes => "result_notes",
            Self::ResultCurrentScore => "result_current_score",
            Self::MusicSelectCentralTitle => "music_select_central_title",
            Self::MusicSelectArtist => "music_select_artist",
            Self::MusicSelectSelectedChart => "music_select_selected_chart",
            Self::MusicSelectActiveListTitle => "music_select_active_list_title",
        }
    }
}

/// Complete result-screen field observations from the currently registered observers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResultScreenFieldObservations {
    pub title: DynamicTextObservation,
    pub artist: DynamicTextObservation,
    pub clear_type: DynamicTextObservation,
    pub difficulty: DynamicTextObservation,
    pub level: DynamicTextObservation,
    pub notes: DynamicTextObservation,
    pub current_score: DynamicTextObservation,
}

/// Complete music-select field observations from the currently registered observers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectScreenFieldObservations {
    pub central_title: DynamicTextObservation,
    pub artist: DynamicTextObservation,
    pub selected_chart: DynamicTextObservation,
    pub active_list_title: DynamicTextObservation,
}

/// Complete field-observer output for exactly one classified screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScreenFieldObservations {
    Result(ResultScreenFieldObservations),
    MusicSelect(MusicSelectScreenFieldObservations),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "screen", content = "resolution", rename_all = "snake_case")]
pub enum ScreenSongResolution {
    Result(ResultSongResolution),
    MusicSelect(MusicSelectSongResolution),
}

impl ScreenFieldObservations {
    #[must_use]
    pub const fn screen(&self) -> ScreenClass {
        match self {
            Self::Result(_) => ScreenClass::Result,
            Self::MusicSelect(_) => ScreenClass::MusicSelect,
        }
    }

    #[must_use]
    pub const fn diagnostic_field_counts(&self) -> (u8, u8) {
        match self {
            Self::Result(_) => (3, 4),
            Self::MusicSelect(_) => (3, 1),
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
pub fn observe_screen_fields<E>(
    crops: &ScreenRgb8Crops,
    mut observe_text: impl FnMut(ScreenTextField, &Rgb8Crop) -> Result<DynamicTextObservation, E>,
) -> Result<ScreenFieldObservations, ScreenFieldObservationError<E>> {
    let mut observe = |field, crop| {
        observe_text(field, crop).map_err(|source| ScreenFieldObservationError::new(field, source))
    };
    Ok(match crops {
        ScreenRgb8Crops::Result(crops) => {
            ScreenFieldObservations::Result(ResultScreenFieldObservations {
                title: observe(ScreenTextField::ResultTitle, &crops.title)?,
                artist: observe(ScreenTextField::ResultArtist, &crops.artist)?,
                clear_type: observe(ScreenTextField::ResultClearType, &crops.clear_type)?,
                difficulty: observe(ScreenTextField::ResultDifficulty, &crops.difficulty)?,
                level: observe(ScreenTextField::ResultLevel, &crops.level)?,
                notes: observe(ScreenTextField::ResultNotes, &crops.notes)?,
                current_score: observe(ScreenTextField::ResultCurrentScore, &crops.current_score)?,
            })
        }
        ScreenRgb8Crops::MusicSelect(crops) => {
            ScreenFieldObservations::MusicSelect(MusicSelectScreenFieldObservations {
                central_title: observe(
                    ScreenTextField::MusicSelectCentralTitle,
                    &crops.central_title,
                )?,
                artist: observe(ScreenTextField::MusicSelectArtist, &crops.artist)?,
                selected_chart: observe(
                    ScreenTextField::MusicSelectSelectedChart,
                    &crops.selected_chart,
                )?,
                active_list_title: observe(
                    ScreenTextField::MusicSelectActiveListTitle,
                    &crops.active_list_title,
                )?,
            })
        }
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResultPresenceEvidence {
    pub warm_pixels: u32,
    pub warm_pixels_min: u32,
    pub upper_panel_edge_pixels: u32,
    pub lower_panel_edge_pixels: u32,
    pub horizontal_edge_pixels_min: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectPresenceEvidence {
    pub cyan_header_pixels: u32,
    pub cyan_header_pixels_min: u32,
    pub colored_level_pixels: u32,
    pub colored_level_pixels_min: u32,
    pub bright_label_pixels: u32,
    pub bright_label_pixels_min: u32,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct PlayPresenceEvidence {
    pub cyan_lane_edge_pixels: u32,
    pub cyan_lane_edge_pixels_min: u32,
    pub warm_header_pixels: u32,
    pub warm_header_pixels_min: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalLayout {
    schema: String,
    canonical_frame_contract_id: String,
    width: u32,
    height: u32,
    pub result: ResultLayout,
    pub music_select: MusicSelectLayout,
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
        for roi in [
            layout.result.header,
            layout.result.upper_panel_edge,
            layout.result.lower_panel_edge,
            layout.result.title,
            layout.result.artist,
            layout.result.clear_type,
            layout.result.difficulty,
            layout.result.level,
            layout.result.notes,
            layout.result.current_score,
            layout.music_select.header,
            layout.music_select.label,
            layout.music_select.level_column,
            layout.music_select.selected_title,
        ] {
            roi.validate(layout.width, layout.height)?;
        }
        for roi in layout.music_select.list_titles.rois() {
            roi.validate(layout.width, layout.height)?;
        }
        let header_pixels = layout
            .result
            .header
            .width
            .checked_mul(layout.result.header.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if layout.result.presence.warm_pixels_min == 0
            || layout.result.presence.horizontal_edge_pixels_min == 0
            || layout.result.presence.warm_pixels_min > header_pixels
            || layout.result.upper_panel_edge.height != 2
            || layout.result.lower_panel_edge.height != 2
            || layout.result.presence.horizontal_edge_pixels_min
                > layout.result.upper_panel_edge.width
            || layout.result.presence.horizontal_edge_pixels_min
                > layout.result.lower_panel_edge.width
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        let header_pixels = layout
            .music_select
            .header
            .width
            .checked_mul(layout.music_select.header.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let level_pixels = layout
            .music_select
            .level_column
            .width
            .checked_mul(layout.music_select.level_column.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let label_pixels = layout
            .music_select
            .label
            .width
            .checked_mul(layout.music_select.label.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if layout.music_select.list_titles.slots == 0
            || layout.music_select.list_titles.stride_y == 0
            || layout.music_select.presence.cyan_header_pixels_min == 0
            || layout.music_select.presence.colored_level_pixels_min == 0
            || layout.music_select.presence.bright_label_pixels_min == 0
            || layout.music_select.presence.cyan_header_pixels_min > header_pixels
            || layout.music_select.presence.colored_level_pixels_min > level_pixels
            || layout.music_select.presence.bright_label_pixels_min > label_pixels
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(layout)
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
    decide_transition: DecideTransitionLayout,
    play: PlayLayout,
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
            layout.decide_transition.splash,
            layout.play.lane_edge,
            layout.play.header,
        ] {
            roi.validate(layout.width, layout.height)?;
        }
        let decide_pixels = layout
            .decide_transition
            .splash
            .width
            .checked_mul(layout.decide_transition.splash.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let lane_edge_pixels = layout
            .play
            .lane_edge
            .width
            .checked_mul(layout.play.lane_edge.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let play_header_pixels = layout
            .play
            .header
            .width
            .checked_mul(layout.play.header.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if layout.decide_transition.presence.cyan_pixels_min == 0
            || layout.decide_transition.presence.bright_pixels_min == 0
            || layout.decide_transition.presence.saturated_pixels_min == 0
            || layout.decide_transition.presence.cyan_pixels_min > decide_pixels
            || layout.decide_transition.presence.bright_pixels_min > decide_pixels
            || layout.decide_transition.presence.saturated_pixels_min > decide_pixels
            || layout.play.presence.cyan_lane_edge_pixels_min == 0
            || layout.play.presence.warm_header_pixels_min == 0
            || layout.play.presence.cyan_lane_edge_pixels_min > lane_edge_pixels
            || layout.play.presence.warm_header_pixels_min > play_header_pixels
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(layout)
    }

    fn sha256() -> String {
        encode_sha256(SCREEN_PATH_LAYOUT_BYTES)
    }
}

/// Inspects one canonical frame without accepting an observed-frame representation.
///
/// # Errors
/// Returns an error when the committed layout or its crop is invalid.
pub fn inspect(frame: &CanonicalFrame) -> Result<RecognitionSnapshot, RecognitionError> {
    let observation = inspect_canonical_rgb8(frame.pixels())?;
    Ok(RecognitionSnapshot {
        schema: "scorepeek-recognition-spike-v3".to_owned(),
        canonical_frame_sha256: encode_sha256(frame.pixels()),
        normalizer_artifact_sha256: frame.normalizer_artifact_sha256.clone(),
        frame_extraction_sha256: frame.frame_extraction_sha256.clone(),
        canonical_layout_sha256: CanonicalLayout::sha256(),
        screen_path_layout_sha256: observation.screen_path_layout_sha256,
        screen: observation.screen,
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
    reason = "all four independent predicates remain together so classification is based on one measurement pass"
)]
pub fn inspect_canonical_rgb8(
    pixels: &[u8],
) -> Result<ScreenPredicateObservation, RecognitionError> {
    if pixels.len() != CANONICAL_BYTES {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let layout = CanonicalLayout::load()?;
    let screen_path_layout = ScreenPathLayout::load()?;
    let header = crop_canonical_pixels(pixels, layout.result.header)?;
    let mut warm = 0_u32;
    for pixel in header.chunks_exact(3) {
        let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
        if r > 100 && g > 70 && b < 170 && r >= g && g >= b {
            warm += 1;
        }
    }
    let upper_panel_edge_pixels = horizontal_edge_pixels(
        &crop_canonical_pixels(pixels, layout.result.upper_panel_edge)?,
        layout.result.upper_panel_edge.width,
    );
    let lower_panel_edge_pixels = horizontal_edge_pixels(
        &crop_canonical_pixels(pixels, layout.result.lower_panel_edge)?,
        layout.result.lower_panel_edge.width,
    );
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
    let play_lane_edge = crop_canonical_pixels(pixels, screen_path_layout.play.lane_edge)?;
    let cyan_lane_edge_pixels = play_lane_edge
        .chunks_exact(3)
        .filter(|pixel| {
            let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
            g > 120 && b > 150 && u16::from(b) * 2 > u16::from(r) * 3
        })
        .fold(0_u32, |count, _| count + 1);
    let play_header = crop_canonical_pixels(pixels, screen_path_layout.play.header)?;
    let warm_header_pixels = play_header
        .chunks_exact(3)
        .filter(|pixel| {
            let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
            r > 120 && g > 70 && b < 100 && r > g && g > b
        })
        .fold(0_u32, |count, _| count + 1);
    let result_present = warm >= layout.result.presence.warm_pixels_min
        && upper_panel_edge_pixels >= layout.result.presence.horizontal_edge_pixels_min;
    let music_select_present = cyan_header_pixels
        >= layout.music_select.presence.cyan_header_pixels_min
        && colored_level_pixels >= layout.music_select.presence.colored_level_pixels_min
        && bright_label_pixels >= layout.music_select.presence.bright_label_pixels_min;
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
    let play_present = cyan_lane_edge_pixels
        >= screen_path_layout.play.presence.cyan_lane_edge_pixels_min
        && warm_header_pixels >= screen_path_layout.play.presence.warm_header_pixels_min;
    let screen = match [
        (result_present, ScreenClass::Result),
        (music_select_present, ScreenClass::MusicSelect),
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
        result_presence: ResultPresenceEvidence {
            warm_pixels: warm,
            warm_pixels_min: layout.result.presence.warm_pixels_min,
            upper_panel_edge_pixels,
            lower_panel_edge_pixels,
            horizontal_edge_pixels_min: layout.result.presence.horizontal_edge_pixels_min,
        },
        music_select_presence: MusicSelectPresenceEvidence {
            cyan_header_pixels,
            cyan_header_pixels_min: layout.music_select.presence.cyan_header_pixels_min,
            colored_level_pixels,
            colored_level_pixels_min: layout.music_select.presence.colored_level_pixels_min,
            bright_label_pixels,
            bright_label_pixels_min: layout.music_select.presence.bright_label_pixels_min,
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
        play_presence: PlayPresenceEvidence {
            cyan_lane_edge_pixels,
            cyan_lane_edge_pixels_min: screen_path_layout.play.presence.cyan_lane_edge_pixels_min,
            warm_header_pixels,
            warm_header_pixels_min: screen_path_layout.play.presence.warm_header_pixels_min,
        },
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

fn crop_canonical_pixels(pixels: &[u8], roi: Roi) -> Result<Vec<u8>, RecognitionError> {
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

    let ScreenRgb8Crops::Result(routed) = route_screen_rgb8_crops(frame.pixels(), snapshot.screen)?
    else {
        return Err(RecognitionError::NotResultScreen);
    };
    let selections = [
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
    ];
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
        schema: "scorepeek-private-canonical-result-crops-v1".to_owned(),
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

    let ScreenRgb8Crops::MusicSelect(routed) =
        route_screen_rgb8_crops(frame.pixels(), snapshot.screen)?
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
    let routed = route_screen_rgb8_crops(frame.pixels(), snapshot.screen)?;
    let output = output.as_ref();
    fs::create_dir(output)?;
    let (integrated_context_layout_sha256, selections) = match routed {
        ScreenRgb8Crops::Result(crops) => (
            IntegratedContextLayout::sha256(),
            vec![(IntegratedContextField::ResultArtist, crops.artist)],
        ),
        ScreenRgb8Crops::MusicSelect(crops) => (
            crops.integrated_context_layout_sha256,
            vec![
                (IntegratedContextField::MusicSelectArtist, crops.artist),
                (
                    IntegratedContextField::MusicSelectSelectedChart,
                    crops.selected_chart,
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
    screen: ScreenClass,
) -> Result<ScreenRgb8Crops, RecognitionError> {
    fn crop(pixels: &[u8], roi: Roi) -> Result<Rgb8Crop, RecognitionError> {
        Ok(Rgb8Crop {
            roi,
            pixels: crop_canonical_pixels(pixels, roi)?,
        })
    }

    let canonical = CanonicalLayout::load()?;
    let context = IntegratedContextLayout::load()?;
    match screen {
        ScreenClass::Result => Ok(ScreenRgb8Crops::Result(ResultScreenRgb8Crops {
            canonical_layout_sha256: CanonicalLayout::sha256(),
            title: crop(pixels, canonical.result.title)?,
            artist: crop(pixels, context.result.artist)?,
            clear_type: crop(pixels, canonical.result.clear_type)?,
            difficulty: crop(pixels, canonical.result.difficulty)?,
            level: crop(pixels, canonical.result.level)?,
            notes: crop(pixels, canonical.result.notes)?,
            current_score: crop(pixels, canonical.result.current_score)?,
        })),
        ScreenClass::MusicSelect => Ok(ScreenRgb8Crops::MusicSelect(MusicSelectScreenRgb8Crops {
            canonical_layout_sha256: CanonicalLayout::sha256(),
            integrated_context_layout_sha256: IntegratedContextLayout::sha256(),
            central_title: crop(pixels, canonical.music_select.selected_title)?,
            artist: crop(pixels, context.music_select.artist)?,
            selected_chart: crop(pixels, context.music_select.selected_chart)?,
            active_list_title: crop(pixels, context.music_select.active_list_title)?,
        })),
        ScreenClass::DecideTransition | ScreenClass::Play | ScreenClass::Unknown => {
            Err(RecognitionError::InvalidCanonicalFrame)
        }
    }
}

const fn integrated_context_filename(field: IntegratedContextField) -> &'static str {
    match field {
        IntegratedContextField::ResultArtist => "result-artist.ppm",
        IntegratedContextField::MusicSelectArtist => "music-select-artist.ppm",
        IntegratedContextField::MusicSelectSelectedChart => "music-select-selected-chart.ppm",
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
        .filter(|crop| crop.field != IntegratedContextField::MusicSelectSelectedChart)
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
                layout.music_select.selected_chart,
            ),
            (
                IntegratedContextField::MusicSelectActiveListTitle,
                "music-select-active-list-title.ppm",
                layout.music_select.active_list_title,
            ),
        ],
        ScreenClass::DecideTransition | ScreenClass::Play | ScreenClass::Unknown => {
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
        || artifact.schema != "scorepeek-private-canonical-result-crops-v1"
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

    fn paint_result_presence(pixels: &mut [u8], layout: &CanonicalLayout) {
        for index in 0..layout.result.presence.warm_pixels_min as usize {
            let x = layout.result.header.x as usize + index % layout.result.header.width as usize;
            let y = layout.result.header.y as usize + index / layout.result.header.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[140, 100, 60]);
        }
        for edge in [
            layout.result.upper_panel_edge,
            layout.result.lower_panel_edge,
        ] {
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
        for index in 0..layout.play.presence.cyan_lane_edge_pixels_min as usize {
            let x = layout.play.lane_edge.x as usize + index % layout.play.lane_edge.width as usize;
            let y = layout.play.lane_edge.y as usize + index / layout.play.lane_edge.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[20, 160, 220]);
        }
        for index in 0..layout.play.presence.warm_header_pixels_min as usize {
            let x = layout.play.header.x as usize + index % layout.play.header.width as usize;
            let y = layout.play.header.y as usize + index / layout.play.header.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&[200, 120, 40]);
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
    fn screen_field_observations_keep_complete_screen_specific_shapes() {
        let result_crops =
            route_screen_rgb8_crops(&vec![0; CANONICAL_BYTES], ScreenClass::Result).unwrap();
        let mut result_calls = 0;
        let result = observe_screen_fields(&result_crops, |_, crop| {
            result_calls += 1;
            Ok::<_, ()>(DynamicTextObservation {
                input_width: crop.roi.width as usize,
                output_timesteps: result_calls,
                open_text: format!("result-{result_calls}"),
            })
        })
        .unwrap();
        let ScreenFieldObservations::Result(result) = result else {
            panic!("result crops produced another screen output");
        };
        assert_eq!(result_calls, 7);
        assert_eq!(result.title.open_text, "result-1");
        assert_eq!(result.artist.open_text, "result-2");
        assert_eq!(result.clear_type.open_text, "result-3");
        assert_eq!(result.difficulty.open_text, "result-4");
        assert_eq!(result.level.open_text, "result-5");
        assert_eq!(result.notes.open_text, "result-6");
        assert_eq!(result.current_score.open_text, "result-7");

        let music_crops =
            route_screen_rgb8_crops(&vec![0; CANONICAL_BYTES], ScreenClass::MusicSelect).unwrap();
        let mut music_calls = 0;
        let music = observe_screen_fields(&music_crops, |_, crop| {
            music_calls += 1;
            Ok::<_, ()>(DynamicTextObservation {
                input_width: crop.roi.width as usize,
                output_timesteps: music_calls,
                open_text: format!("music-{music_calls}"),
            })
        })
        .unwrap();
        let ScreenFieldObservations::MusicSelect(music) = music else {
            panic!("music-select crops produced another screen output");
        };
        assert_eq!(music_calls, 4);
        assert_eq!(music.central_title.open_text, "music-1");
        assert_eq!(music.artist.open_text, "music-2");
        assert_eq!(music.selected_chart.open_text, "music-3");
        assert_eq!(music.active_list_title.open_text, "music-4");
    }

    #[test]
    fn failed_text_field_does_not_construct_a_partial_screen_observation() {
        let crops =
            route_screen_rgb8_crops(&vec![0; CANONICAL_BYTES], ScreenClass::Result).unwrap();
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
                })
            }
        })
        .unwrap_err();
        assert_eq!(calls, 2);
        assert_eq!(error.field, ScreenTextField::ResultArtist);
        assert_eq!(error.source_error(), &"runtime-failed");
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
        assert_eq!(snapshot.result_presence.upper_panel_edge_pixels, 518);
        assert_eq!(snapshot.result_presence.lower_panel_edge_pixels, 518);

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
        assert_eq!(
            play.play_presence.cyan_lane_edge_pixels,
            layout.play.presence.cyan_lane_edge_pixels_min
        );
        assert_eq!(
            play.play_presence.warm_header_pixels,
            layout.play.presence.warm_header_pixels_min
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
        let frame = test_frame(pixels);
        let snapshot = inspect(&frame).unwrap();
        assert_eq!(snapshot.screen, ScreenClass::MusicSelect);
        assert_eq!(snapshot.music_select_presence.cyan_header_pixels, 7_000);
        assert_eq!(snapshot.music_select_presence.colored_level_pixels, 1_000);
        assert_eq!(snapshot.music_select_presence.bright_label_pixels, 4_000);

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
        assert_eq!(music_artifact.crops.len(), 3);
        assert_eq!(
            music_artifact.crops[0].field,
            IntegratedContextField::MusicSelectArtist
        );
        assert_eq!(
            music_artifact.crops[1].field,
            IntegratedContextField::MusicSelectSelectedChart
        );
        assert_eq!(
            music_artifact.crops[2].field,
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
            "scorepeek-private-canonical-result-crops-v1"
        );
        assert_eq!(artifact.crops.len(), 7);
        assert_eq!(artifact.crops[0].field, "title");
        assert_eq!(artifact.crops[0].roi, layout.result.title);
        assert_eq!(artifact.crops[0].bytes, 600 * 50 * 3 + 14);
        assert_eq!(artifact.crops[0].file_sha256.len(), 64);
        assert_eq!(artifact.crops[0].pixel_sha256.len(), 64);
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
        let unsupported_manifest_sha256 =
            write_unsupported_profile_evidence(directory.path(), normalizer.clone(), manifest);
        assert!(
            CanonicalFrame::read_extraction(
                directory.path(),
                "result-001",
                &unsupported_manifest_sha256,
            )
            .is_err(),
            "an uncalibrated capture profile must fail even with self-consistent evidence"
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
    fn canonical_frame_accepts_only_registered_calibrated_profiles() {
        assert!(calibrated_capture_profile(
            CALIBRATED_CAPTURE_PROFILE_SHA256
        ));
        assert!(calibrated_capture_profile(
            CALIBRATED_GAMESCOPE_VKCAPTURE_PROFILE_SHA256
        ));
        assert!(!calibrated_capture_profile(&"e".repeat(64)));
    }

    fn write_unsupported_profile_evidence(
        directory: &Path,
        normalizer: DomainNormalizerEvidence,
        manifest: CanonicalExtractionEvidence,
    ) -> String {
        write_profile_evidence(directory, normalizer, manifest, &"e".repeat(64))
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
