use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

mod title;
mod title_onnx;

pub use title::{
    DIAGNOSTIC_TITLE_COMPARISON_KEY_ID, DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
    DiagnosticTitleCandidate, DiagnosticTitleError, DiagnosticTitleUnknownReason,
    diagnostic_title_candidate,
};
pub use title_onnx::{OnnxParityError, OnnxParitySummary, compare_paddle_onnx};

const CANONICAL_WIDTH: u32 = 1_920;
const CANONICAL_HEIGHT: u32 = 1_080;
const CANONICAL_BYTES: usize = CANONICAL_WIDTH as usize * CANONICAL_HEIGHT as usize * 3;
const CANONICAL_FRAME_CONTRACT_ID: &str = "scorepeek-canonical-rgb8-1920x1080-v1";
const LAYOUT_SCHEMA: &str = "scorepeek-canonical-layout-v1";
const NORMALIZER_SCHEMA: &str = "scorepeek-domain-normalizer-artifact-v1";
const EXTRACTION_SCHEMA: &str = "scorepeek-private-canonical-frame-extraction-v1";
const NORMALIZER_IMPLEMENTATION: &str = "ffmpeg-swscale-bt709-limited-to-rgb24-v1";
const NORMALIZER_FILTER: &str = "scale=1920:1080:flags=bitexact:in_color_matrix=bt709:out_color_matrix=bt709:in_range=tv:out_range=pc,format=rgb24";
const CALIBRATED_CAPTURE_PROFILE_SHA256: &str =
    "d5809dc9b2acc19837260053f4df59a454c9178ae2ac6a0602982effc9da4704";
const CALIBRATED_FFMPEG_SHA256: &str =
    "9eac5b2b5076db5ff853a6fa0dcd6b8de7d0cac8481eadda6c47cd935825f1ee";
const FFMPEG_VERSION: &str = "8.1.2";
const MAX_EXTRACTION_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_NORMALIZER_BYTES: u64 = 64 * 1024;
const PPM_HEADER: &[u8] = b"P6\n1920 1080\n255\n";
const CANONICAL_FILE_BYTES: u64 = CANONICAL_BYTES as u64 + PPM_HEADER.len() as u64;
const LAYOUT_BYTES: &[u8] = include_bytes!("canonical-layout-v1.json");

#[derive(Debug)]
pub enum RecognitionError {
    Io(std::io::Error),
    Json(serde_json::Error),
    InvalidCanonicalFrame,
    InvalidCanonicalLayout,
    NotResultScreen,
}

impl std::fmt::Display for RecognitionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "canonical frame I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "canonical layout JSON failed: {error}"),
            Self::InvalidCanonicalFrame => formatter.write_str("canonical frame is invalid"),
            Self::InvalidCanonicalLayout => formatter.write_str("canonical layout is invalid"),
            Self::NotResultScreen => formatter.write_str("canonical frame is not a result screen"),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalFrame {
    pixels: Box<[u8]>,
    normalizer_artifact_sha256: String,
    frame_extraction_sha256: String,
}

impl CanonicalFrame {
    /// Reads one P6 frame only after validating its canonical extraction and normalizer evidence.
    ///
    /// # Errors
    /// Returns an error for an unknown frame ID, invalid or mismatched evidence, a symlinked
    /// artifact, or bytes outside the fixed canonical RGB8 contract.
    pub fn read_extraction(
        directory: impl AsRef<Path>,
        frame_id: &str,
        expected_extraction_sha256: &str,
    ) -> Result<Self, RecognitionError> {
        if !valid_sha256(expected_extraction_sha256) {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let directory = directory.as_ref();
        if !directory.symlink_metadata()?.is_dir() {
            return Err(RecognitionError::InvalidCanonicalFrame);
        }
        let manifest_path = directory.join("manifest.json");
        let normalizer_path = directory.join("normalizer.json");
        for path in [&manifest_path, &normalizer_path] {
            if !path.symlink_metadata()?.is_file() {
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
            normalizer_artifact_sha256: manifest.normalizer_artifact_sha256,
            frame_extraction_sha256: expected_extraction_sha256.to_owned(),
        })
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Copies one layout-bound RGB8 crop in row-major order.
    ///
    /// # Errors
    /// Returns an error when the ROI is outside the canonical frame.
    pub fn crop(&self, roi: Roi) -> Result<Vec<u8>, RecognitionError> {
        roi.validate(CANONICAL_WIDTH, CANONICAL_HEIGHT)?;
        let row_bytes = roi.width as usize * 3;
        let mut crop = Vec::with_capacity(row_bytes * roi.height as usize);
        for y in roi.y..roi.y + roi.height {
            let start = (y as usize * CANONICAL_WIDTH as usize + roi.x as usize) * 3;
            crop.extend_from_slice(&self.pixels[start..start + row_bytes]);
        }
        Ok(crop)
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
            || normalizer.capture_profile_id != CALIBRATED_CAPTURE_PROFILE_SHA256
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
    let metadata = path.symlink_metadata()?;
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
    pub title: Roi,
    pub artist: Roi,
    pub difficulty: Roi,
    pub level: Roi,
    pub notes: Roi,
    pub current_score: Roi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultPresencePredicate {
    warm_pixels_min: u32,
    red_pixels_min: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenClass {
    Result,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecognitionSnapshot {
    pub schema: String,
    pub canonical_frame_sha256: String,
    pub normalizer_artifact_sha256: String,
    pub frame_extraction_sha256: String,
    pub canonical_layout_sha256: String,
    pub screen: ScreenClass,
    pub result_presence: ResultPresenceEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResultCropArtifact {
    pub schema: String,
    pub frame_id: String,
    pub frame_extraction_sha256: String,
    pub canonical_frame_sha256: String,
    pub normalizer_artifact_sha256: String,
    pub canonical_layout_sha256: String,
    pub crops: Vec<ResultCropEvidence>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResultCropEvidence {
    pub field: ResultCropField,
    pub filename: String,
    pub roi: Roi,
    pub pixel_sha256: String,
    pub file_sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultCropField {
    Title,
    Artist,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResultPresenceEvidence {
    pub warm_pixels: u32,
    pub warm_pixels_min: u32,
    pub red_pixels: u32,
    pub red_pixels_min: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalLayout {
    schema: String,
    canonical_frame_contract_id: String,
    width: u32,
    height: u32,
    pub result: ResultLayout,
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
            layout.result.title,
            layout.result.artist,
            layout.result.difficulty,
            layout.result.level,
            layout.result.notes,
            layout.result.current_score,
        ] {
            roi.validate(layout.width, layout.height)?;
        }
        let header_pixels = layout
            .result
            .header
            .width
            .checked_mul(layout.result.header.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if layout.result.presence.warm_pixels_min == 0
            || layout.result.presence.red_pixels_min == 0
            || layout.result.presence.warm_pixels_min > header_pixels
            || layout.result.presence.red_pixels_min > header_pixels
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(layout)
    }

    #[must_use]
    pub fn sha256() -> String {
        encode_sha256(LAYOUT_BYTES)
    }
}

/// Inspects one canonical frame without accepting an observed-frame representation.
///
/// # Errors
/// Returns an error when the committed layout or its crop is invalid.
pub fn inspect(frame: &CanonicalFrame) -> Result<RecognitionSnapshot, RecognitionError> {
    let layout = CanonicalLayout::load()?;
    let header = frame.crop(layout.result.header)?;
    let mut warm = 0_u32;
    let mut red = 0_u32;
    for pixel in header.chunks_exact(3) {
        let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
        if r > 100 && g > 70 && b < 170 && r >= g && g >= b {
            warm += 1;
        }
        if r > 30 && u16::from(r) * 2 > u16::from(g) * 3 && u16::from(r) * 2 > u16::from(b) * 3 {
            red += 1;
        }
    }
    let screen = if warm >= layout.result.presence.warm_pixels_min
        && red >= layout.result.presence.red_pixels_min
    {
        ScreenClass::Result
    } else {
        ScreenClass::Unknown
    };
    Ok(RecognitionSnapshot {
        schema: "scorepeek-recognition-spike-v1".to_owned(),
        canonical_frame_sha256: encode_sha256(frame.pixels()),
        normalizer_artifact_sha256: frame.normalizer_artifact_sha256.clone(),
        frame_extraction_sha256: frame.frame_extraction_sha256.clone(),
        canonical_layout_sha256: CanonicalLayout::sha256(),
        screen,
        result_presence: ResultPresenceEvidence {
            warm_pixels: warm,
            warm_pixels_min: layout.result.presence.warm_pixels_min,
            red_pixels: red,
            red_pixels_min: layout.result.presence.red_pixels_min,
        },
    })
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

    let layout = CanonicalLayout::load()?;
    let selections = [
        (ResultCropField::Title, "title.ppm", layout.result.title),
        (ResultCropField::Artist, "artist.ppm", layout.result.artist),
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
    let mut crops = Vec::with_capacity(selections.len());
    for (field, filename, roi) in selections {
        let pixels = frame.crop(roi)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn test_frame(pixels: Vec<u8>) -> CanonicalFrame {
        CanonicalFrame {
            pixels: pixels.into(),
            normalizer_artifact_sha256: "1".repeat(64),
            frame_extraction_sha256: "2".repeat(64),
        }
    }

    #[test]
    fn canonical_layout_is_bounded_and_hash_stable() {
        let layout = CanonicalLayout::load().unwrap();
        assert_eq!(
            layout.result.title,
            Roi {
                x: 660,
                y: 900,
                width: 600,
                height: 100
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
    fn result_presence_is_fail_closed() {
        let layout = CanonicalLayout::load().unwrap();
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        let warm = [140, 100, 60];
        let red = [90, 20, 20];
        for index in 0..layout.result.presence.warm_pixels_min as usize {
            let x = layout.result.header.x as usize + index % layout.result.header.width as usize;
            let y = layout.result.header.y as usize + index / layout.result.header.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&warm);
        }
        for index in 0..layout.result.presence.red_pixels_min as usize {
            let x = layout.result.header.x as usize + index % layout.result.header.width as usize;
            let y = layout.result.header.y as usize + layout.result.header.height as usize
                - 1
                - index / layout.result.header.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&red);
        }
        let frame = test_frame(pixels);
        let snapshot = inspect(&frame).unwrap();
        assert_eq!(snapshot.screen, ScreenClass::Result);
        assert_eq!(snapshot.result_presence.warm_pixels, 3_000);
        assert_eq!(snapshot.result_presence.red_pixels, 12_000);

        let empty = test_frame(vec![0_u8; CANONICAL_BYTES]);
        assert_eq!(inspect(&empty).unwrap().screen, ScreenClass::Unknown);
    }

    #[test]
    fn result_crops_are_layout_bound_and_digest_bound() {
        let layout = CanonicalLayout::load().unwrap();
        let mut pixels = vec![0_u8; CANONICAL_BYTES];
        let warm = [140, 100, 60];
        let red = [90, 20, 20];
        for index in 0..layout.result.presence.warm_pixels_min as usize {
            let x = layout.result.header.x as usize + index % layout.result.header.width as usize;
            let y = layout.result.header.y as usize + index / layout.result.header.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&warm);
        }
        for index in 0..layout.result.presence.red_pixels_min as usize {
            let x = layout.result.header.x as usize + index % layout.result.header.width as usize;
            let y = layout.result.header.y as usize + layout.result.header.height as usize
                - 1
                - index / layout.result.header.width as usize;
            pixels[(y * CANONICAL_WIDTH as usize + x) * 3..][..3].copy_from_slice(&red);
        }
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
        assert_eq!(artifact.crops.len(), 6);
        assert_eq!(artifact.crops[0].field, "title");
        assert_eq!(artifact.crops[0].roi, layout.result.title);
        assert_eq!(artifact.crops[0].bytes, 600 * 100 * 3 + 15);
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

    fn write_unsupported_profile_evidence(
        directory: &Path,
        mut normalizer: DomainNormalizerEvidence,
        mut manifest: CanonicalExtractionEvidence,
    ) -> String {
        normalizer.capture_profile_id = "e".repeat(64);
        let normalizer_bytes = canonical_evidence_json(&normalizer).unwrap();
        manifest.capture_profile_id = "e".repeat(64);
        manifest.normalizer_artifact_sha256 = encode_sha256(&normalizer_bytes);
        let manifest_bytes = canonical_evidence_json(&manifest).unwrap();
        let manifest_sha256 = encode_sha256(&manifest_bytes);
        fs::write(directory.join("normalizer.json"), normalizer_bytes).unwrap();
        fs::write(directory.join("manifest.json"), manifest_bytes).unwrap();
        manifest_sha256
    }
}
