#![allow(clippy::missing_errors_doc, clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use scorepeek::catalog::{Difficulty, PlayType};
use scorepeek::recognition::{
    NumericField, PlayOption, PlayOptions, PreviousBest, PreviousBestValue, ResultChartResolution,
    ResultJudgments, ResultPerformanceResolution, ResultTiming, Rgb8Crop, ScreenClass,
    ScreenRgb8Crops, SupplementalResultValue, inspect_canonical_rgb8, resolve_clear_type,
    route_screen_rgb8_crops,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::CorpusError;

const DIAGNOSTIC_SCHEMA: &str = "scorepeek-private-diagnostic-session-v4";
const LEGACY_DIAGNOSTIC_SCHEMA: &str = "scorepeek-private-diagnostic-session-v3";
const SESSION_SCHEMA: &str = "scorepeek-private-capture-session-v1";
const DRAFT_SCHEMA: &str = "scorepeek-private-session-review-draft-v1";
const LABEL_SCHEMA: &str = "scorepeek-private-session-regression-label-v5";
const PREVIOUS_LABEL_SCHEMA: &str = "scorepeek-private-session-regression-label-v4";
const PREVIOUS_LABEL_SCHEMA_V3: &str = "scorepeek-private-session-regression-label-v3";
const LEGACY_LABEL_SCHEMA: &str = "scorepeek-private-session-regression-label-v2";
const SUITE_SCHEMA: &str = "scorepeek-private-regression-suite-v1";
const ACTIVE_SCHEMA: &str = "scorepeek-private-regression-suite-active-v1";
const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ARTIFACTS: usize = 20_000;
const MAX_NDJSON_RECORDS: usize = 250_000;
const MAX_NDJSON_RECORD_BYTES: usize = 1024 * 1024;
const MAX_EVIDENCE_FRAMES: usize = 1_024;
const MAX_EVIDENCE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_QOI_BYTES: u64 = 16 * 1024 * 1024;
const NUMERIC_DATASET_SCHEMA: &str = "scorepeek-private-numeric-ctc-dataset-v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticManifest {
    schema: String,
    source_kind: SourceKind,
    session_id: String,
    capture_generation: u64,
    profile_sha256: String,
    catalog_sha256: String,
    recognition_interval_ms: u64,
    processed_ticks: u64,
    busy_skips: u64,
    maximum_consecutive_busy_skips: u64,
    #[serde(default)]
    field_observation_busy_skips: Option<u64>,
    #[serde(default)]
    maximum_consecutive_field_observation_busy_skips: Option<u64>,
    completeness: String,
    capture_manifest_sha256: String,
    recognition_manifest_sha256: String,
    event_manifest_sha256: String,
    artifacts: Vec<DiagnosticArtifact>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SourceKind {
    LiveRun,
    VideoReplay,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticArtifact {
    kind: String,
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
struct CaptureComponentManifest {
    schema: String,
    status: String,
    completeness: String,
    start: CaptureStartReference,
    #[serde(default)]
    recognition_interval_ms: Option<u64>,
    #[serde(default)]
    processed_ticks: Option<u64>,
    #[serde(default)]
    busy_skips: Option<u64>,
    facts: NdjsonComponent,
    frames: Vec<Value>,
}

#[derive(Debug, Deserialize)]
struct CaptureStartReference {
    schema: String,
    filename: String,
    file_sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
struct CaptureRun {
    schema: String,
    run_id: String,
    binding: CaptureRunBinding,
}

#[derive(Debug, Deserialize)]
struct CaptureRunBinding {
    capture_generation: u64,
    capture_profile_sha256: String,
    catalog_sha256: String,
}

#[derive(Debug, Deserialize)]
struct NdjsonComponent {
    filename: String,
    record_count: u64,
    #[serde(default)]
    first_sequence: Option<u64>,
    #[serde(default)]
    last_sequence: Option<u64>,
    file_sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
struct RecognitionComponentManifest {
    schema: String,
    run_id: String,
    profile_sha256: String,
    catalog_sha256: String,
    status: String,
    observations_sha256: String,
    #[serde(default)]
    observation_count: Option<u64>,
    #[serde(default)]
    retained_observation_count: Option<u64>,
    #[serde(default)]
    observation_bytes: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EventComponentManifest {
    schema: String,
    run_id: String,
    status: String,
    events_sha256: String,
    event_count: u64,
    event_bytes: u64,
    dropped_events: u64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum StoredRunEventPayload {
    WatcherStarted {
        invocation_id: String,
        profile_sha256: String,
    },
    SessionStarted {
        session_id: Option<String>,
        capture_generation: u64,
        capture_profile_sha256: String,
        normalizer_artifact_sha256: String,
    },
    ScreenChanged {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
    },
    RawScreenObserved {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        semantic_episode_id: Option<u64>,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
        unknown_reason: Option<String>,
    },
    SemanticScreenEpisodeChanged {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: String,
        phase: String,
    },
    ScreenTick {
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: String,
    },
    FieldObservation {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
        fields: Value,
        result_song_resolution: Value,
        music_select_song_resolution: Value,
        parsed_result_fields: Option<Value>,
        result_chart_resolution: Option<Value>,
        current_score_ocr_resolution: Option<Value>,
        song_resolution_presentation: Value,
    },
    ResultDetected {
        session_id: String,
        capture_generation: u64,
        source_sequence: u64,
        result: Value,
    },
    TemporalResultChanged {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        source_sequence: Option<u64>,
        transitions: Vec<Value>,
        state: Value,
        stable_song: Option<Value>,
    },
    TemporalMusicSelectChanged {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        source_sequence: Option<u64>,
        reasons: Vec<Value>,
        state: Value,
        retained_song: Option<Value>,
        candidate_song: Option<Value>,
    },
    NumericResultChanged {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        source_sequence: u64,
        state: Value,
        reason: String,
        event_suppression_reason: Option<String>,
    },
    PlayAttemptChanged {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        source_sequence: Option<u64>,
        state: Value,
    },
    ResolverStateChanged {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        screen_episode_id: u64,
        source_sequence: u64,
        scope: String,
        state: String,
        top: Option<Value>,
        runner_up: Option<Value>,
        support: u16,
        margin: u16,
        selected_family_support: Value,
        runner_up_family_support: Value,
        observation_count: u32,
    },
    SelectionDifficultyChanged {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        screen_episode_id: u64,
        source_sequence: u64,
        target: String,
        reason: String,
        current: Option<Value>,
    },
    SessionFinished {
        session_id: String,
        capture_generation: u64,
        outcome: String,
        report: Value,
    },
    WatcherStopped {
        invocation_id: String,
        reason: String,
    },
}

#[derive(Debug, Serialize)]
struct SessionIdentity<'a> {
    schema: &'static str,
    source_session_id: &'a str,
    capture_generation: u64,
    session_sha256: &'a str,
}

#[derive(Debug, Deserialize)]
struct VideoProbe {
    streams: Vec<VideoStreamProbe>,
    frames: Vec<VideoFrameProbe>,
}

#[derive(Debug, Deserialize)]
struct VideoStreamProbe {
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
struct VideoFrameProbe {
    best_effort_timestamp_time: Option<String>,
}

struct OwnedStaging {
    path: PathBuf,
    published: bool,
}

impl OwnedStaging {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            published: false,
        }
    }

    fn disarm(&mut self) {
        self.published = true;
    }
}

impl Drop for OwnedStaging {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CaptureSession {
    schema: String,
    diagnostic_sha256: String,
    source_kind: SourceKind,
    source_session_id: String,
    capture_generation: u64,
    profile_sha256: String,
    catalog_sha256: String,
    recognition_interval_ms: u64,
    processed_ticks: u64,
    busy_skips: u64,
    maximum_consecutive_busy_skips: u64,
    completeness: String,
    canonical_frames: Vec<ReviewFrame>,
    normalization_pairs: Vec<NormalizationPair>,
    artifacts: Vec<CorpusArtifact>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NormalizationPair {
    sequence: u64,
    canonical_sha256: String,
    observed_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CorpusArtifact {
    kind: String,
    source_path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReviewDraft {
    schema: String,
    session_sha256: String,
    diagnostic_sha256: String,
    source_session_id: String,
    canonical_frames: Vec<ReviewFrame>,
    observation_count: u64,
    completeness: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReviewFrame {
    sequence: u64,
    artifact_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegressionLabel {
    schema: String,
    session_sha256: String,
    disposition: LabelDisposition,
    episodes: Vec<RegressionEpisode>,
    negative_frames: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LabelDisposition {
    Include,
    Exclude,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegressionEpisode {
    episode_id: String,
    expected_song_id: String,
    expected_clear_type: String,
    expected_result: ExpectedResult,
    stable_sequences: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempt: Option<AttemptTruth>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttemptTruth {
    attempt_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent_attempt_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    select_span: Option<SequenceSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    play_span: Option<SequenceSpan>,
    result_span: SequenceSpan,
    outcome: AttemptOutcome,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SequenceSpan {
    first_sequence: u64,
    last_sequence: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum AttemptOutcome {
    Accepted,
    Abandoned,
    Unlinked,
    NoResult,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExpectedResult {
    play_side: String,
    play_mode: String,
    play_type: PlayType,
    difficulty: Difficulty,
    level: u8,
    notes: u32,
    current_score: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    judgments: Option<ResultJudgments>,
    #[serde(skip_serializing_if = "Option::is_none")]
    miss_count: Option<SupplementalResultValue<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timing: Option<ResultTiming>,
    #[serde(skip_serializing_if = "Option::is_none")]
    combo_break: Option<SupplementalResultValue<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_best: Option<PreviousBest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    play_options: Option<Vec<PlayOption>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct NumericDatasetAuthoringSummary {
    pub schema: &'static str,
    pub suite_sha256: String,
    pub sessions: usize,
    pub episodes: usize,
    pub samples: usize,
    pub unique_crops: usize,
    pub output: PathBuf,
    pub manifest_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NumericSentinelRequest {
    schema: String,
    sentinel_id: String,
    labels: BTreeMap<NumericField, String>,
}

#[derive(Debug, Serialize)]
pub struct NumericSentinelAuthoringSummary {
    pub schema: &'static str,
    pub sentinel_id: String,
    pub frame_sha256: String,
    pub labels_sha256: String,
    pub samples: usize,
    pub output: PathBuf,
    pub manifest_sha256: String,
}

#[derive(Serialize)]
struct NumericSentinelManifest {
    schema: &'static str,
    sentinel_id: String,
    frame_sha256: String,
    labels_sha256: String,
    dictionary: &'static str,
    maximum_text_length: usize,
    samples: Vec<NumericSentinelSample>,
}

#[derive(Serialize)]
struct NumericSentinelSample {
    field: NumericField,
    label: String,
    crop_sha256: String,
    filename: String,
    roi: scorepeek::recognition::Roi,
}

#[derive(Serialize)]
struct NumericDatasetManifest {
    schema: &'static str,
    suite_sha256: String,
    dictionary: &'static str,
    maximum_text_length: usize,
    samples: Vec<NumericDatasetSample>,
}

#[derive(Debug, Serialize)]
struct NumericDatasetSample {
    session_sha256: String,
    episode_id: String,
    split: String,
    sequence: u64,
    field: NumericField,
    label: String,
    crop_sha256: String,
    filename: String,
    roi: scorepeek::recognition::Roi,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRegressionLabel {
    schema: String,
    session_sha256: String,
    disposition: LabelDisposition,
    episodes: Vec<LegacyRegressionEpisode>,
    negative_frames: Vec<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRegressionEpisode {
    episode_id: String,
    expected_song_id: String,
    expected_clear_type: String,
    expected_result: LegacyExpectedResult,
    stable_sequences: Vec<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyExpectedResult {
    savable: bool,
    play_side: String,
    play_mode: String,
    play_type: PlayType,
    difficulty: Difficulty,
    level: u8,
    notes: u32,
    current_score: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegressionSuite {
    schema: String,
    previous_generation_sha256: Option<String>,
    entries: Vec<SuiteEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SuiteEntry {
    session_sha256: String,
    label_sha256: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveSuite {
    schema: String,
    generation_sha256: String,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticVerificationSummary {
    schema: &'static str,
    diagnostic_sha256: String,
    session_id: String,
    artifact_count: usize,
    canonical_frame_count: usize,
    observation_count: u64,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticImportSummary {
    schema: &'static str,
    session_sha256: String,
    diagnostic_sha256: String,
    review_draft: PathBuf,
    canonical_frame_count: usize,
}

#[derive(Debug, Serialize)]
pub struct ReviewApplySummary {
    schema: &'static str,
    session_sha256: String,
    label_sha256: String,
    generation_sha256: String,
    active_entries: usize,
}

#[derive(Debug, Serialize)]
pub struct CorpusReplaySummary {
    schema: &'static str,
    generation_sha256: String,
    session_count: usize,
    episode_count: usize,
    canonical_frames: usize,
    negative_frames: usize,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticConversionSummary {
    schema: &'static str,
    output: PathBuf,
    session_id: String,
    canonical_frame_count: usize,
    fact_count: usize,
    observation_count: u64,
}

#[derive(Debug, Serialize)]
pub struct VideoReplaySummary {
    schema: &'static str,
    output: PathBuf,
    session_id: String,
    processed_ticks: u64,
    evidence_frames: usize,
    observation_count: u64,
}

/// Replays a video deterministically at 10 Hz through the production normalizer and recognizer.
pub fn replay_video(
    video: &Path,
    profile_name: &str,
    output: &Path,
) -> Result<VideoReplaySummary, CorpusError> {
    if !video.is_absolute() || !video.is_file() || !output.is_absolute() || output.exists() {
        return invalid("video replay requires an absolute input file and absent absolute output");
    }
    if profile_name.is_empty()
        || profile_name.len() > 128
        || !profile_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return invalid("video replay profile name is invalid");
    }
    let profile_path = profile_root()?.join(format!("{profile_name}.json"));
    let profile_bytes = fs::read(&profile_path)?;
    let profile_sha256 = digest(&profile_bytes);
    let profile =
        scorepeek::capture::GamescopeProfileBinding::parse(&profile_bytes, &profile_sha256)
            .map_err(|_| CorpusError::InvalidRequest("capture profile is invalid".to_owned()))?;
    let catalog_root = default_catalog_root()?;
    let active = scorepeek::catalog::CatalogStore::new(&catalog_root)
        .load_active()
        .map_err(|_| CorpusError::InvalidRequest("active catalog could not be loaded".to_owned()))?
        .ok_or_else(|| CorpusError::InvalidRequest("active catalog is unavailable".to_owned()))?;
    let bundle = scorepeek::model_cache::ensure_small_model(None, |_| {})
        .map_err(|error| CorpusError::InvalidRequest(format!("model cache failed: {error}")))?;
    let video_sha256 = digest_file(video)?;
    let session_id = format!("video-{}", &video_sha256[..24]);
    let parent = output
        .parent()
        .ok_or_else(|| CorpusError::InvalidRequest("output has no parent".to_owned()))?;
    let staging = parent.join(format!(
        ".{}-scorepeek-staging",
        output
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("video")
    ));
    if staging.exists() {
        return invalid("video replay staging already exists");
    }
    create_private_directory(&staging)?;
    let mut staging_guard = OwnedStaging::new(&staging);
    create_private_directory(&staging.join("capture"))?;
    create_private_directory(&staging.join("recognition"))?;
    write_new(&staging.join("capture/profile.json"), &profile_bytes)?;
    let run = serde_json::json!({
        "schema": "scorepeek-private-diagnostic-capture-start-v3",
        "run_id": session_id,
        "binding": {
            "capture_generation": 1,
            "capture_profile_sha256": profile.capture_profile_sha256(),
            "normalizer_sha256": profile.normalizer_artifact_sha256(),
            "canonical_layout_sha256": scorepeek::recognition::CanonicalLayout::sha256(),
            "catalog_sha256": active.digest,
            "model_sha256": scorepeek::recognition::LIVE_MODEL_SHA256,
            "runtime_sha256": scorepeek::recognition::LIVE_RUNTIME_SHA256
        },
        "source": {"kind": "video_replay", "video_sha256": video_sha256}
    });
    let run_bytes = canonical_json(&run)?;
    write_new(&staging.join("capture/run.json"), &run_bytes)?;
    let descriptor = scorepeek::diagnostic_recording::DiagnosticRunDescriptor {
        run_id: session_id.clone(),
        monotonic_start_ms: 0,
        resource: scorepeek::diagnostic_recording::DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: "0".repeat(64),
        },
        binding: scorepeek::diagnostic_recording::DiagnosticBinding {
            capture_generation: 1,
            capture_profile_sha256: profile.capture_profile_sha256().to_owned(),
            normalizer_sha256: profile.normalizer_artifact_sha256().to_owned(),
            canonical_layout_sha256: scorepeek::recognition::CanonicalLayout::sha256(),
            catalog_sha256: active.digest.clone(),
            model_sha256: scorepeek::recognition::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: scorepeek::recognition::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    };
    let diagnostic_root = tempfile::tempdir()?;
    let mut recognition =
        scorepeek::recognition_live::field_session::FieldObservationSession::start_registered(
            diagnostic_root.path(),
            descriptor,
            scorepeek::diagnostic_recording::DiagnosticPolicy {
                enabled: false,
                ..Default::default()
            },
            &catalog_root,
            &bundle,
            scorepeek::recognition_live::text_observer_pool::RecognitionExecutionMode::Offline,
        )
        .map_err(|error| {
            CorpusError::InvalidRequest(format!("production recognizer could not start: {error:?}"))
        })?;

    let ffprobe = super::media::find_executable("ffprobe")?;
    let probed = Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_streams",
            "-show_frames",
            "-show_entries",
            "stream=width,height:frame=best_effort_timestamp_time",
            "-of",
            "json",
        ])
        .arg(video)
        .stdin(Stdio::null())
        .output()?;
    if !probed.status.success() {
        return invalid("video metadata could not be read");
    }
    let probe: VideoProbe = serde_json::from_slice(&probed.stdout)?;
    if probe.streams.len() != 1
        || probe.streams[0].width != profile.observed_width()
        || probe.streams[0].height != profile.observed_height()
        || probe.frames.is_empty()
    {
        return invalid("video dimensions differ from the selected profile");
    }
    let timestamps = probe
        .frames
        .iter()
        .map(|frame| {
            frame
                .best_effort_timestamp_time
                .as_deref()
                .ok_or_else(|| {
                    CorpusError::InvalidRequest("decoded video frame lacks a timestamp".to_owned())
                })
                .and_then(parse_timestamp_ms)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let ffmpeg = super::media::find_executable("ffmpeg")?;
    let mut child = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(video)
        .args([
            "-map", "0:v:0", "-pix_fmt", "bgr0", "-f", "rawvideo", "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| CorpusError::InvalidRequest("ffmpeg stdout is unavailable".to_owned()))?;
    let frame_bytes = usize::try_from(profile.observed_width())
        .ok()
        .and_then(|width| {
            usize::try_from(profile.observed_height())
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| CorpusError::InvalidRequest("profile dimensions overflow".to_owned()))?;
    let stride = profile
        .observed_width()
        .checked_mul(4)
        .ok_or_else(|| CorpusError::InvalidRequest("profile stride overflows".to_owned()))?;
    let mut sequence = 0_u64;
    let mut facts = Vec::new();
    let mut observations = Vec::new();
    let mut frames = Vec::new();
    let mut saved = BTreeMap::<String, String>::new();
    let mut evidence_bytes = 0_u64;
    let mut evidence_stopped = false;
    let mut previous_screen = None;
    let mut events = vec![serde_json::json!({
        "schema":"scorepeek-private-diagnostic-event-v1", "event":"session_started",
        "session_id":session_id, "capture_generation":1
    })];
    let mut process_sample = |raw: &[u8],
                              source_timestamp_ms: u64,
                              sequence: u64|
     -> Result<Option<Value>, CorpusError> {
        let mut evidence_event = None;
        let canonical = profile
            .geometry()
            .normalize_bgrx_bytes(
                raw,
                profile.observed_width(),
                profile.observed_height(),
                stride,
            )
            .map_err(|_| {
                CorpusError::InvalidRequest("video frame normalization failed".to_owned())
            })?;
        let snapshot = scorepeek::recognition::inspect_canonical_rgb8(&canonical)
            .map_err(|_| CorpusError::InvalidRequest("video scene inspection failed".to_owned()))?;
        let screen = match snapshot.screen {
            scorepeek::recognition::ScreenClass::Result => "result",
            scorepeek::recognition::ScreenClass::MusicSelect => "music_select",
            scorepeek::recognition::ScreenClass::ModeSelect => "mode_select",
            scorepeek::recognition::ScreenClass::DecideTransition => "decide_transition",
            scorepeek::recognition::ScreenClass::Play => "play",
            scorepeek::recognition::ScreenClass::Unknown => "unknown",
        };
        facts.extend_from_slice(&canonical_json(&serde_json::json!({
            "schema":"scorepeek-private-diagnostic-fact-v3", "tick_sequence":sequence,
            "source_timestamp_ms":source_timestamp_ms, "screen":screen,
            "screen_path_layout_sha256":snapshot.screen_path_layout_sha256,
            "result_presence":snapshot.result_presence,
            "music_select_presence":snapshot.music_select_presence,
            "decide_transition_presence":snapshot.decide_transition_presence,
            "play_presence":snapshot.play_presence
        }))?);
        let should_save = previous_screen != Some(snapshot.screen)
            || matches!(snapshot.screen, scorepeek::recognition::ScreenClass::Result);
        if should_save && !evidence_stopped && frames.len() >= MAX_EVIDENCE_FRAMES {
            evidence_stopped = true;
            evidence_event = Some(serde_json::json!({
                "schema":"scorepeek-private-diagnostic-event-v1",
                "event":"evidence_capacity_reached", "tick_sequence":sequence,
                "reason":"frame_count", "session_id":session_id,
                "capture_generation":1
            }));
        }
        if should_save && !evidence_stopped {
            let pixel_sha256 = digest(&canonical);
            let filename = if let Some(filename) = saved.get(&pixel_sha256).cloned() {
                Some(filename)
            } else {
                let filename = format!("frame-{sequence:020}.qoi");
                let encoded = qoi::encode_to_vec(&canonical, 1920, 1080).map_err(|_| {
                    CorpusError::InvalidRequest("canonical QOI encoding failed".to_owned())
                })?;
                let next_bytes =
                    evidence_bytes.saturating_add(u64::try_from(encoded.len()).unwrap_or(u64::MAX));
                if next_bytes > MAX_EVIDENCE_BYTES {
                    evidence_stopped = true;
                    evidence_event = Some(serde_json::json!({
                        "schema":"scorepeek-private-diagnostic-event-v1",
                        "event":"evidence_capacity_reached", "tick_sequence":sequence,
                        "reason":"aggregate_bytes", "session_id":session_id,
                        "capture_generation":1
                    }));
                    None
                } else {
                    write_new(&staging.join("capture").join(&filename), &encoded)?;
                    evidence_bytes = next_bytes;
                    saved.insert(pixel_sha256.clone(), filename.clone());
                    Some(filename)
                }
            };
            if let Some(filename) = filename {
                frames.push(serde_json::json!({
                    "sequence":sequence, "source_timestamp_ms":source_timestamp_ms,
                    "filename":filename, "canonical_pixel_sha256":pixel_sha256
                }));
            }
        }
        let bound = scorepeek::diagnostic_live::BoundCanonicalFrame::for_replay(
            1,
            sequence,
            source_timestamp_ms,
            profile.capture_profile_sha256().to_owned(),
            profile.normalizer_artifact_sha256().to_owned(),
            canonical,
        )
        .map_err(|_| CorpusError::InvalidRequest("canonical replay binding failed".to_owned()))?;
        let inspected = recognition
            .inspect(&bound)
            .map_err(|_| CorpusError::InvalidRequest("production recognition failed".to_owned()))?;
        let mut record = serde_json::json!({
            "schema":"scorepeek-recognition-observation-v5", "tick_sequence":sequence,
            "source_timestamp_ms":source_timestamp_ms, "screen":screen,
            "fields":null, "song_id":null
        });
        if let scorepeek::recognition_live::field_session::FieldObservationSubmission::Submitted(
            pending,
        ) = inspected.field_submission
        {
            match recognition.wait_field_observation(&pending, Duration::from_secs(10)) {
                scorepeek::recognition_live::field_session::FieldObservationSessionPoll::Ready { observation, .. } => {
                    if let Ok(output) = observation.output() {
                        record["fields"] = field_json(output.fields());
                        record["song_id"] = output.result_resolution()
                            .and_then(scorepeek::recognition::ResultSongResolution::accepted_song_id)
                            .map_or(Value::Null, |song| Value::String(song.as_uuid().to_string()));
                    } else {
                        record["failure"] = Value::String("ocr_failed".to_owned());
                    }
                }
                _ => record["failure"] = Value::String("ocr_unavailable".to_owned()),
            }
        }
        observations.extend_from_slice(&canonical_json(&record)?);
        previous_screen = Some(snapshot.screen);
        Ok(evidence_event)
    };
    let mut raw = vec![0_u8; frame_bytes];
    let mut latest: Option<(u64, Vec<u8>)> = None;
    let mut next_tick_ms = 0_u64;
    let mut previous_pts = None;
    let mut decoded = 0_usize;
    for timestamp_ms in timestamps {
        match stdout.read_exact(&mut raw) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return invalid("video decode ended before its timestamp inventory");
            }
            Err(error) => return Err(error.into()),
        }
        decoded += 1;
        if let Some(previous) = previous_pts {
            if timestamp_ms < previous {
                events.push(video_timeline_event(
                    "timestamp_rewind",
                    &session_id,
                    timestamp_ms,
                    previous,
                ));
                continue;
            }
            if timestamp_ms == previous {
                events.push(video_timeline_event(
                    "duplicate_timestamp",
                    &session_id,
                    timestamp_ms,
                    previous,
                ));
            } else if timestamp_ms.saturating_sub(previous) > 200 {
                events.push(video_timeline_event(
                    "decode_gap",
                    &session_id,
                    timestamp_ms,
                    previous,
                ));
            }
        }
        while next_tick_ms < timestamp_ms {
            if let Some((source_timestamp_ms, latest_raw)) = latest.as_ref() {
                if let Some(event) = process_sample(latest_raw, *source_timestamp_ms, sequence)? {
                    events.push(event);
                }
                sequence = sequence.saturating_add(1);
            }
            next_tick_ms = next_tick_ms.saturating_add(100);
        }
        latest = Some((timestamp_ms, raw.clone()));
        previous_pts = Some(timestamp_ms);
    }
    if decoded == 0 {
        return invalid("video produced no decoded frames");
    }
    match stdout.read_exact(&mut raw) {
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {}
        Ok(()) => return invalid("video decode produced frames without timestamp inventory"),
        Err(error) => return Err(error.into()),
    }
    if let Some((source_timestamp_ms, latest_raw)) = latest.as_ref() {
        while next_tick_ms <= *source_timestamp_ms {
            if let Some(event) = process_sample(latest_raw, *source_timestamp_ms, sequence)? {
                events.push(event);
            }
            sequence = sequence.saturating_add(1);
            next_tick_ms = next_tick_ms.saturating_add(100);
        }
    }
    drop(stdout);
    let output_status = child.wait_with_output()?;
    if !output_status.status.success() {
        return Err(CorpusError::InvalidRequest(format!(
            "ffmpeg video replay failed: {}",
            String::from_utf8_lossy(&output_status.stderr)
        )));
    }
    let finish = recognition.finish(
        scorepeek::diagnostic_recording::DiagnosticRunStatus::Success,
        sequence,
        Duration::from_secs(10),
    );
    if finish.field_observer.status
        != scorepeek::recognition_live::field_observer::FieldObserverFinishStatus::Complete
    {
        return invalid("production recognizer did not finish cleanly");
    }
    if sequence == 0 {
        return invalid("video produced no 10 Hz samples");
    }
    write_new(&staging.join("capture/facts.ndjson"), &facts)?;
    write_new(
        &staging.join("recognition/observations.ndjson"),
        &observations,
    )?;
    write_new(
        &staging.join("recognition/catalog.json"),
        &canonical_json(
            &serde_json::json!({"schema":"scorepeek-recognition-catalog-binding-v1","catalog_sha256":active.digest}),
        )?,
    )?;
    let capture_manifest = serde_json::json!({
        "schema":"scorepeek-private-diagnostic-capture-v3", "run_id":session_id,
        "status":"success",
        "recognition_interval_ms":100, "processed_ticks":sequence, "busy_skips":0,
        "completeness":"complete",
        "start":{"schema":"scorepeek-private-diagnostic-artifact-v1","filename":"run.json",
            "file_sha256":digest(&run_bytes),"bytes":run_bytes.len()},
        "frames":frames, "facts":{"filename":"facts.ndjson","record_count":sequence,
            "first_sequence":0,"last_sequence":sequence.saturating_sub(1),
            "file_sha256":digest(&facts),"bytes":facts.len()}
    });
    let capture_bytes = canonical_json(&capture_manifest)?;
    write_new(&staging.join("capture/manifest.json"), &capture_bytes)?;
    let recognition_manifest = serde_json::json!({
        "schema":"scorepeek-recognition-evidence-manifest-v3", "run_id":session_id,
        "profile_sha256":profile.capture_profile_sha256(), "status":"complete",
        "observation_count":sequence, "observations_sha256":digest(&observations),
        "catalog_sha256":active.digest
    });
    let recognition_bytes = canonical_json(&recognition_manifest)?;
    write_new(
        &staging.join("recognition/manifest.json"),
        &recognition_bytes,
    )?;
    events.push(serde_json::json!({
        "schema":"scorepeek-private-diagnostic-event-v1", "event":"session_finished",
        "session_id":session_id, "capture_generation":1,
        "processed_ticks":sequence, "busy_skips":0
    }));
    let mut event_bytes = Vec::new();
    for event in &events {
        event_bytes.extend_from_slice(&canonical_json(event)?);
    }
    write_new(&staging.join("events.ndjson"), &event_bytes)?;
    let event_manifest_bytes =
        write_event_manifest(&staging, &session_id, &event_bytes, events.len() as u64)?;
    let artifacts = enumerate_component_artifacts(&staging)?;
    let top = DiagnosticManifest {
        schema: DIAGNOSTIC_SCHEMA.to_owned(),
        source_kind: SourceKind::VideoReplay,
        session_id: session_id.clone(),
        capture_generation: 1,
        profile_sha256: profile.capture_profile_sha256().to_owned(),
        catalog_sha256: active.digest,
        recognition_interval_ms: 100,
        processed_ticks: sequence,
        busy_skips: 0,
        maximum_consecutive_busy_skips: 0,
        field_observation_busy_skips: Some(0),
        maximum_consecutive_field_observation_busy_skips: Some(0),
        completeness: "complete".to_owned(),
        capture_manifest_sha256: digest(&capture_bytes),
        recognition_manifest_sha256: digest(&recognition_bytes),
        event_manifest_sha256: digest(&event_manifest_bytes),
        artifacts,
    };
    write_new(&staging.join("manifest.json"), &canonical_json(&top)?)?;
    verify_diagnostic(&staging)?;
    fs::rename(&staging, output)?;
    staging_guard.disarm();
    File::open(parent)?.sync_all()?;
    Ok(VideoReplaySummary {
        schema: "scorepeek-private-video-replay-v1",
        output: output.to_owned(),
        session_id,
        processed_ticks: sequence,
        evidence_frames: frames.len(),
        observation_count: sequence,
    })
}

pub fn convert_v2_diagnostic(
    diagnostic: &Path,
    recognition: &Path,
    output: &Path,
) -> Result<DiagnosticConversionSummary, CorpusError> {
    if output.exists() || !output.is_absolute() {
        return invalid("v3 diagnostic output must be an absent absolute path");
    }
    let diagnostic_manifest_path = if diagnostic.join("diagnostic-manifest.json").is_file() {
        diagnostic.join("diagnostic-manifest.json")
    } else {
        diagnostic.join("manifest.json")
    };
    let (_, diagnostic_bytes) = read_json::<Value>(&diagnostic_manifest_path)?;
    let diagnostic_value: Value = serde_json::from_slice(&diagnostic_bytes)?;
    if diagnostic_value["schema"] != "scorepeek-private-diagnostic-run-v2" {
        return invalid("legacy diagnostic must use v2 schema");
    }
    let session_id = diagnostic_value["start"]["filename"]
        .as_str()
        .and_then(|_| find_run_id(diagnostic, recognition))
        .ok_or_else(|| {
            CorpusError::InvalidRequest("legacy session directory is ambiguous".to_owned())
        })?;
    let source_directory = if diagnostic.join(&session_id).is_dir() {
        diagnostic.join(&session_id)
    } else {
        diagnostic.to_owned()
    };
    let recognition_directory = if recognition.join(&session_id).is_dir() {
        recognition.join(&session_id)
    } else {
        recognition.to_owned()
    };
    let parent = output
        .parent()
        .ok_or_else(|| CorpusError::InvalidRequest("v3 output has no parent".to_owned()))?;
    let staging = parent.join(format!(
        ".{}.scorepeek-staging",
        output
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("diagnostic")
    ));
    if staging.exists() {
        return invalid("v3 diagnostic staging already exists");
    }
    create_private_directory(&staging)?;
    let mut staging_guard = OwnedStaging::new(&staging);
    create_private_directory(&staging.join("capture"))?;
    create_private_directory(&staging.join("recognition"))?;

    let capture_run_bytes = fs::read(source_directory.join("run.json"))?;
    write_new(&staging.join("capture/run.json"), &capture_run_bytes)?;
    let frames = diagnostic_value["frames"]
        .as_array()
        .ok_or_else(|| {
            CorpusError::InvalidRequest("legacy diagnostic frames are invalid".to_owned())
        })?
        .clone();
    let fact_refs = diagnostic_value["facts"]
        .as_array()
        .ok_or_else(|| {
            CorpusError::InvalidRequest("legacy diagnostic facts are invalid".to_owned())
        })?
        .clone();
    let mut base_monotonic_ms = frames
        .iter()
        .filter_map(|frame| frame["monotonic_start_ms"].as_u64())
        .min();
    for fact in &fact_refs {
        let filename = fact["filename"].as_str().ok_or_else(|| {
            CorpusError::InvalidRequest("legacy fact filename is invalid".to_owned())
        })?;
        let document: Value = serde_json::from_slice(&fs::read(source_directory.join(filename))?)?;
        if let Some(timestamp) = document["fact"]["monotonic_start_ms"].as_u64() {
            base_monotonic_ms =
                Some(base_monotonic_ms.map_or(timestamp, |base| base.min(timestamp)));
        }
    }
    let base_monotonic_ms = base_monotonic_ms.ok_or_else(|| {
        CorpusError::InvalidRequest("legacy diagnostic has no timestamped evidence".to_owned())
    })?;
    let mut converted_frame_map = BTreeMap::new();
    for frame in &frames {
        let mut converted = frame.clone();
        let timestamp = frame["monotonic_start_ms"].as_u64().ok_or_else(|| {
            CorpusError::InvalidRequest("legacy frame timestamp is invalid".to_owned())
        })?;
        let tick = timestamp.saturating_sub(base_monotonic_ms) / 100;
        converted["sequence"] = Value::from(tick);
        let filename = frame["filename"].as_str().ok_or_else(|| {
            CorpusError::InvalidRequest("legacy frame filename is invalid".to_owned())
        })?;
        copy_file(
            &source_directory.join(filename),
            &staging.join("capture").join(filename),
        )?;
        if let Some(source) = frame.get("source").filter(|value| !value.is_null()) {
            let source_name = source["filename"].as_str().ok_or_else(|| {
                CorpusError::InvalidRequest("legacy source filename is invalid".to_owned())
            })?;
            let width =
                u32::try_from(source["video"]["width"].as_u64().unwrap_or(0)).map_err(|_| {
                    CorpusError::InvalidRequest("legacy source width is invalid".to_owned())
                })?;
            let height =
                u32::try_from(source["video"]["height"].as_u64().unwrap_or(0)).map_err(|_| {
                    CorpusError::InvalidRequest("legacy source height is invalid".to_owned())
                })?;
            let stride = usize::try_from(source["stride"].as_u64().unwrap_or(0)).map_err(|_| {
                CorpusError::InvalidRequest("legacy source stride is invalid".to_owned())
            })?;
            let raw = fs::read(source_directory.join(source_name))?;
            let rgb = bgrx_to_rgb(&raw, width, height, stride)?;
            let encoded = qoi::encode_to_vec(&rgb, width, height).map_err(|_| {
                CorpusError::InvalidRequest("legacy source QOI encoding failed".to_owned())
            })?;
            let qoi_name = source_name
                .strip_suffix(".bgrx")
                .unwrap_or(source_name)
                .to_owned()
                + ".qoi";
            write_new(&staging.join("capture").join(&qoi_name), &encoded)?;
            let target = converted
                .get_mut("source")
                .and_then(Value::as_object_mut)
                .ok_or_else(|| {
                    CorpusError::InvalidRequest("legacy source document is invalid".to_owned())
                })?;
            target.remove("pixel_format");
            target.insert("filename".to_owned(), Value::String(qoi_name));
            target.insert(
                "observed_pixel_format".to_owned(),
                Value::String("bgrx".to_owned()),
            );
            target.insert(
                "encoded_pixel_format".to_owned(),
                Value::String("rgb8".to_owned()),
            );
            target.insert("file_sha256".to_owned(), Value::String(digest(&encoded)));
            target.insert("bytes".to_owned(), Value::from(encoded.len() as u64));
        }
        converted_frame_map.insert(tick, converted);
    }
    let converted_frames = converted_frame_map.into_values().collect::<Vec<_>>();
    let mut facts_bytes = Vec::new();
    let mut first_sequence = None;
    let mut last_sequence = None;
    let mut fact_documents = Vec::with_capacity(fact_refs.len());
    for fact in &fact_refs {
        let filename = fact["filename"].as_str().ok_or_else(|| {
            CorpusError::InvalidRequest("legacy fact filename is invalid".to_owned())
        })?;
        let bytes = fs::read(source_directory.join(filename))?;
        let document = serde_json::from_slice::<Value>(&bytes)?;
        let timestamp = document["fact"]["monotonic_start_ms"]
            .as_u64()
            .ok_or_else(|| {
                CorpusError::InvalidRequest("legacy fact timestamp is invalid".to_owned())
            })?;
        fact_documents.push((timestamp, document));
    }
    fact_documents.sort_by_key(|(timestamp, _)| *timestamp);
    for (timestamp, mut document) in fact_documents {
        let sequence = timestamp.saturating_sub(base_monotonic_ms) / 100;
        if let Some(object) = document.as_object_mut() {
            object.remove("sequence");
            object.insert(
                "schema".to_owned(),
                Value::String("scorepeek-private-diagnostic-fact-v3".to_owned()),
            );
            object.insert("tick_sequence".to_owned(), Value::from(sequence));
            object.insert("source_timestamp_ms".to_owned(), Value::from(timestamp));
        }
        first_sequence.get_or_insert(sequence);
        last_sequence = Some(sequence);
        facts_bytes.extend_from_slice(&canonical_json(&document)?);
    }
    write_new(&staging.join("capture/facts.ndjson"), &facts_bytes)?;
    let processed_ticks = converted_frames
        .last()
        .and_then(|frame| frame["sequence"].as_u64())
        .into_iter()
        .chain(last_sequence)
        .max()
        .map_or(0, |tick| tick.saturating_add(1));
    let mut capture_manifest = diagnostic_value;
    capture_manifest["schema"] =
        Value::String("scorepeek-private-diagnostic-capture-v3".to_owned());
    capture_manifest["frames"] = Value::Array(converted_frames.clone());
    capture_manifest["recognition_interval_ms"] = Value::from(100);
    capture_manifest["busy_skips"] = Value::from(0);
    capture_manifest["processed_ticks"] = Value::from(processed_ticks);
    capture_manifest["facts"] = serde_json::json!({
        "filename": "facts.ndjson",
        "record_count": fact_refs.len(),
        "first_sequence": first_sequence,
        "last_sequence": last_sequence,
        "file_sha256": digest(&facts_bytes),
        "bytes": facts_bytes.len(),
    });
    capture_manifest["start"] = serde_json::json!({
        "schema":"scorepeek-private-diagnostic-artifact-v1", "filename":"run.json",
        "file_sha256":digest(&capture_run_bytes), "bytes":capture_run_bytes.len()
    });
    stabilize_capture_manifest(&mut capture_manifest, &staging.join("capture"))?;
    let capture_manifest_bytes = canonical_json(&capture_manifest)?;
    write_new(
        &staging.join("capture/manifest.json"),
        &capture_manifest_bytes,
    )?;

    copy_file(
        &recognition_directory.join("catalog.json"),
        &staging.join("recognition/catalog.json"),
    )?;
    let legacy_observations = File::open(recognition_directory.join("observations.ndjson"))?;
    let mut converted_observation_map = BTreeMap::new();
    for line in BufReader::new(legacy_observations).lines() {
        let mut observation: Value = serde_json::from_str(&line?)?;
        let object = observation.as_object_mut().ok_or_else(|| {
            CorpusError::InvalidRequest("legacy recognition observation is invalid".to_owned())
        })?;
        object
            .remove("sequence")
            .and_then(|value| value.as_u64())
            .ok_or_else(|| {
                CorpusError::InvalidRequest("legacy observation sequence is invalid".to_owned())
            })?;
        let timestamp = object["timing"]["monotonic_start_ms"]
            .as_u64()
            .ok_or_else(|| {
                CorpusError::InvalidRequest("legacy observation timestamp is invalid".to_owned())
            })?;
        let sequence = timestamp.saturating_sub(base_monotonic_ms) / 100;
        object.insert("tick_sequence".to_owned(), Value::from(sequence));
        object.insert("source_timestamp_ms".to_owned(), Value::from(timestamp));
        object.insert(
            "schema".to_owned(),
            Value::String("scorepeek-recognition-observation-v5".to_owned()),
        );
        converted_observation_map.insert(sequence, observation);
    }
    let mut converted_observations = Vec::new();
    for observation in converted_observation_map.values() {
        converted_observations.extend_from_slice(&canonical_json(observation)?);
    }
    write_new(
        &staging.join("recognition/observations.ndjson"),
        &converted_observations,
    )?;
    let recognition_manifest_source = if recognition.join("recognition-manifest.json").is_file() {
        recognition.join("recognition-manifest.json")
    } else {
        recognition_directory.join("manifest.json")
    };
    let mut recognition_manifest: Value =
        serde_json::from_slice(&fs::read(recognition_manifest_source)?)?;
    recognition_manifest["schema"] =
        Value::String("scorepeek-recognition-evidence-manifest-v3".to_owned());
    recognition_manifest["observations_sha256"] = Value::String(digest(&converted_observations));
    recognition_manifest["observation_bytes"] = Value::from(converted_observations.len() as u64);
    recognition_manifest["input_observation_count"] =
        Value::from(converted_observation_map.len() as u64);
    recognition_manifest["retained_observation_count"] =
        Value::from(converted_observation_map.len() as u64);
    let events = format!(
        "{{\"schema\":\"scorepeek-private-diagnostic-event-v1\",\"event\":\"session_started\",\"session_id\":{session_id:?},\"capture_generation\":1,\"conversion\":\"legacy_v2\"}}\n{{\"schema\":\"scorepeek-private-diagnostic-event-v1\",\"event\":\"session_finished\",\"session_id\":{session_id:?},\"capture_generation\":1,\"conversion\":\"legacy_v2\"}}\n",
    );
    write_new(&staging.join("events.ndjson"), events.as_bytes())?;
    let event_manifest_bytes = write_event_manifest(&staging, &session_id, events.as_bytes(), 2)?;
    let observation_count = verify_ndjson(&staging.join("recognition/observations.ndjson"))?;
    if converted_observation_map
        .keys()
        .next_back()
        .is_some_and(|tick| *tick >= processed_ticks)
    {
        return invalid("legacy recognition falls outside retained diagnostic timing");
    }
    let run: Value = serde_json::from_slice(&fs::read(source_directory.join("run.json"))?)?;
    let profile_sha256 = run["binding"]["capture_profile_sha256"]
        .as_str()
        .ok_or_else(|| {
            CorpusError::InvalidRequest("legacy profile binding is unavailable".to_owned())
        })?;
    let profile_bytes = find_profile_bytes(profile_sha256)?;
    write_new(&staging.join("capture/profile.json"), &profile_bytes)?;
    recognition_manifest["run_id"] = Value::String(session_id.clone());
    recognition_manifest["profile_sha256"] = Value::String(profile_sha256.to_owned());
    recognition_manifest["catalog_sha256"] = run["binding"]["catalog_sha256"].clone();
    recognition_manifest["status"] = capture_manifest["completeness"].clone();
    write_new(
        &staging.join("recognition/manifest.json"),
        &canonical_json(&recognition_manifest)?,
    )?;
    let artifacts = enumerate_component_artifacts(&staging)?;
    let manifest = DiagnosticManifest {
        schema: DIAGNOSTIC_SCHEMA.to_owned(),
        source_kind: SourceKind::LiveRun,
        session_id: session_id.clone(),
        capture_generation: run["binding"]["capture_generation"].as_u64().unwrap_or(1),
        profile_sha256: profile_sha256.to_owned(),
        catalog_sha256: run["binding"]["catalog_sha256"]
            .as_str()
            .unwrap_or(&"0".repeat(64))
            .to_owned(),
        recognition_interval_ms: 100,
        processed_ticks,
        busy_skips: 0,
        maximum_consecutive_busy_skips: 0,
        field_observation_busy_skips: Some(0),
        maximum_consecutive_field_observation_busy_skips: Some(0),
        completeness: capture_manifest["completeness"]
            .as_str()
            .unwrap_or("partial")
            .to_owned(),
        capture_manifest_sha256: digest(&capture_manifest_bytes),
        recognition_manifest_sha256: digest_file(&staging.join("recognition/manifest.json"))?,
        event_manifest_sha256: digest(&event_manifest_bytes),
        artifacts,
    };
    write_new(&staging.join("manifest.json"), &canonical_json(&manifest)?)?;
    verify_diagnostic(&staging)?;
    fs::rename(&staging, output)?;
    staging_guard.disarm();
    File::open(parent)?.sync_all()?;
    Ok(DiagnosticConversionSummary {
        schema: "scorepeek-private-diagnostic-conversion-v1",
        output: output.to_owned(),
        session_id,
        canonical_frame_count: frames.len(),
        fact_count: fact_refs.len(),
        observation_count,
    })
}

pub fn verify_diagnostic(path: &Path) -> Result<DiagnosticVerificationSummary, CorpusError> {
    let (manifest, bytes) = read_json::<DiagnosticManifest>(&path.join("manifest.json"))?;
    validate_diagnostic_manifest(&manifest)?;
    let mut seen = BTreeSet::new();
    let mut canonical_frames = 0;
    for artifact in &manifest.artifacts {
        if !seen.insert(&artifact.path) {
            return invalid("diagnostic contains duplicate artifact paths");
        }
        let relative = safe_relative(&artifact.path)?;
        let file = path.join(relative);
        verify_file(&file, &artifact.sha256, artifact.bytes)?;
        if artifact.path.starts_with("capture/frame-")
            && Path::new(&artifact.path)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("qoi"))
        {
            verify_canonical_qoi(&file)?;
            canonical_frames += 1;
        }
    }
    let capture_manifest = manifest_artifact(&manifest, "capture/manifest.json")?;
    let recognition_manifest = manifest_artifact(&manifest, "recognition/manifest.json")?;
    let event_manifest = manifest_artifact(&manifest, "event-manifest.json")?;
    let profile_artifact = manifest_artifact(&manifest, "capture/profile.json")?;
    let run_artifact = manifest_artifact(&manifest, "capture/run.json")?;
    let events = manifest_artifact(&manifest, "events.ndjson")?;
    if capture_manifest.sha256 != manifest.capture_manifest_sha256
        || recognition_manifest.sha256 != manifest.recognition_manifest_sha256
        || event_manifest.sha256 != manifest.event_manifest_sha256
    {
        return invalid("diagnostic component manifest binding differs");
    }
    let profile_bytes = fs::read(path.join("capture/profile.json"))?;
    let profile = scorepeek::capture::GamescopeProfileBinding::parse(
        &profile_bytes,
        &profile_artifact.sha256,
    )
    .map_err(|_| CorpusError::InvalidRequest("diagnostic capture profile is invalid".to_owned()))?;
    if profile.capture_profile_sha256() != manifest.profile_sha256 {
        return invalid("diagnostic capture profile binding differs");
    }
    let (capture, _) = read_json::<CaptureComponentManifest>(&path.join("capture/manifest.json"))?;
    let (run, _) = read_json::<CaptureRun>(&path.join("capture/run.json"))?;
    let (recognition, _) =
        read_json::<RecognitionComponentManifest>(&path.join("recognition/manifest.json"))?;
    let (event, _) = read_json::<EventComponentManifest>(&path.join("event-manifest.json"))?;
    if !matches!(
        capture.schema.as_str(),
        "scorepeek-private-diagnostic-capture-v3" | "scorepeek-private-diagnostic-capture-v4"
    ) || recognition.schema != "scorepeek-recognition-evidence-manifest-v3"
        || capture.start.schema != "scorepeek-private-diagnostic-artifact-v1"
        || capture.start.filename != "run.json"
        || capture.start.file_sha256 != run_artifact.sha256
        || capture.start.bytes != run_artifact.bytes
        || !matches!(
            run.schema.as_str(),
            "scorepeek-private-diagnostic-run-start-v2"
                | "scorepeek-private-diagnostic-capture-start-v3"
                | "scorepeek-private-diagnostic-capture-start-v4"
        )
        || run.run_id != manifest.session_id
        || run.binding.capture_generation != manifest.capture_generation
        || run.binding.capture_profile_sha256 != manifest.profile_sha256
        || run.binding.catalog_sha256 != manifest.catalog_sha256
        || recognition.run_id != manifest.session_id
        || recognition.profile_sha256 != manifest.profile_sha256
        || recognition.catalog_sha256 != manifest.catalog_sha256
        || recognition.status != manifest.completeness
        || capture.completeness != manifest.completeness
        || !matches!(
            capture.status.as_str(),
            "success" | "error" | "cancel" | "timeout"
        )
        || !matches!(
            capture.completeness.as_str(),
            "complete" | "partial" | "dropped"
        )
        || capture.facts.filename != "facts.ndjson"
        || capture.facts.file_sha256 != manifest_artifact(&manifest, "capture/facts.ndjson")?.sha256
        || capture.facts.bytes != manifest_artifact(&manifest, "capture/facts.ndjson")?.bytes
        || recognition.observations_sha256
            != manifest_artifact(&manifest, "recognition/observations.ndjson")?.sha256
    {
        return invalid("diagnostic component contract differs");
    }
    if let Some(interval) = capture.recognition_interval_ms
        && interval != manifest.recognition_interval_ms
    {
        return invalid("diagnostic sampling interval differs");
    }
    if capture
        .processed_ticks
        .is_some_and(|count| count != manifest.processed_ticks)
        || capture
            .busy_skips
            .is_some_and(|count| count != manifest.busy_skips)
    {
        return invalid("diagnostic sampling summary differs");
    }
    let facts = verify_tick_ndjson(&path.join("capture/facts.ndjson"), false)?;
    let observations = verify_tick_ndjson(&path.join("recognition/observations.ndjson"), true)?.0;
    if facts.0 != capture.facts.record_count
        || facts.1 != capture.facts.first_sequence
        || facts.2 != capture.facts.last_sequence
        || recognition
            .observation_count
            .or(recognition.retained_observation_count)
            != Some(observations)
        || recognition.observation_bytes.is_some_and(|bytes| {
            bytes
                != manifest_artifact(&manifest, "recognition/observations.ndjson")
                    .map_or(u64::MAX, |artifact| artifact.bytes)
        })
    {
        return invalid("diagnostic NDJSON summary differs");
    }
    let event_count = verify_session_events(&path.join("events.ndjson"), &manifest)?;
    if event.schema != "scorepeek-run-event-artifact-v1"
        || event.run_id != manifest.session_id
        || !matches!(event.status.as_str(), "complete" | "partial")
        || (event.status == "complete" && event.dropped_events != 0)
        || event.events_sha256 != events.sha256
        || event.event_bytes != events.bytes
        || event.event_count != event_count
        || (manifest.completeness == "complete"
            && (capture.status != "success"
                || recognition.status != "complete"
                || event.status != "complete"))
    {
        return invalid("diagnostic event component contract differs");
    }
    if manifest.completeness == "complete" && observations != manifest.processed_ticks {
        return invalid("diagnostic processed tick count differs from observations");
    }
    if capture.frames.iter().any(|frame| {
        frame
            .get("sequence")
            .and_then(Value::as_u64)
            .is_none_or(|sequence| {
                sequence >= manifest.processed_ticks.saturating_add(manifest.busy_skips)
            })
    }) {
        return invalid("diagnostic evidence frame tick is outside the session summary");
    }
    Ok(DiagnosticVerificationSummary {
        schema: "scorepeek-private-diagnostic-verification-v1",
        diagnostic_sha256: digest(&bytes),
        session_id: manifest.session_id,
        artifact_count: manifest.artifacts.len(),
        canonical_frame_count: canonical_frames,
        observation_count: observations,
    })
}

pub fn import_diagnostic(
    store: &Path,
    diagnostic: &Path,
    review_draft: &Path,
) -> Result<DiagnosticImportSummary, CorpusError> {
    let verified = verify_diagnostic(diagnostic)?;
    let (manifest, _) = read_json::<DiagnosticManifest>(&diagnostic.join("manifest.json"))?;
    ensure_store(store)?;
    let mut artifacts = Vec::with_capacity(manifest.artifacts.len());
    let capture: Value =
        serde_json::from_slice(&fs::read(diagnostic.join("capture/manifest.json"))?)?;
    let frame_records = capture["frames"].as_array().ok_or_else(|| {
        CorpusError::InvalidRequest("diagnostic frame index is invalid".to_owned())
    })?;
    let mut frames = Vec::with_capacity(frame_records.len());
    let mut normalization_pairs = Vec::new();
    for frame in frame_records {
        let sequence = frame["sequence"].as_u64().ok_or_else(|| {
            CorpusError::InvalidRequest("diagnostic frame sequence is invalid".to_owned())
        })?;
        let filename = frame["filename"].as_str().ok_or_else(|| {
            CorpusError::InvalidRequest("diagnostic frame filename is invalid".to_owned())
        })?;
        let artifact = manifest_artifact(&manifest, &format!("capture/{filename}"))?;
        frames.push(ReviewFrame {
            sequence,
            artifact_sha256: artifact.sha256.clone(),
        });
        if let Some(source_filename) = frame["source"]["filename"].as_str() {
            let observed = manifest_artifact(&manifest, &format!("capture/{source_filename}"))?;
            normalization_pairs.push(NormalizationPair {
                sequence,
                canonical_sha256: artifact.sha256.clone(),
                observed_sha256: observed.sha256.clone(),
            });
        }
    }
    frames.sort_by_key(|frame| frame.sequence);
    if frames
        .windows(2)
        .any(|pair| pair[0].sequence >= pair[1].sequence)
    {
        return invalid("diagnostic frame sequence is not strictly ordered");
    }
    for artifact in &manifest.artifacts {
        let source = diagnostic.join(safe_relative(&artifact.path)?);
        publish_object(store, &source, &artifact.sha256, artifact.bytes)?;
        artifacts.push(CorpusArtifact {
            kind: artifact.kind.clone(),
            source_path: artifact.path.clone(),
            sha256: artifact.sha256.clone(),
            bytes: artifact.bytes,
        });
    }
    frames.sort_by_key(|frame| frame.sequence);
    let session = CaptureSession {
        schema: SESSION_SCHEMA.to_owned(),
        diagnostic_sha256: verified.diagnostic_sha256.clone(),
        source_kind: manifest.source_kind,
        source_session_id: manifest.session_id.clone(),
        capture_generation: manifest.capture_generation,
        profile_sha256: manifest.profile_sha256,
        catalog_sha256: manifest.catalog_sha256,
        recognition_interval_ms: manifest.recognition_interval_ms,
        processed_ticks: manifest.processed_ticks,
        busy_skips: manifest.busy_skips,
        maximum_consecutive_busy_skips: manifest.maximum_consecutive_busy_skips,
        completeness: manifest.completeness.clone(),
        canonical_frames: frames.clone(),
        normalization_pairs,
        artifacts,
    };
    let session_bytes = canonical_json(&session)?;
    let session_sha256 = digest(&session_bytes);
    let identity_key = canonical_json(&serde_json::json!({
        "source_session_id": session.source_session_id,
        "capture_generation": session.capture_generation,
    }))?;
    let identity_sha256 = digest(&identity_key);
    publish_document(
        &store
            .join("identities")
            .join(format!("{identity_sha256}.json")),
        &canonical_json(&SessionIdentity {
            schema: "scorepeek-private-capture-session-identity-v1",
            source_session_id: &session.source_session_id,
            capture_generation: session.capture_generation,
            session_sha256: &session_sha256,
        })?,
    )?;
    publish_document(
        &store
            .join("sessions")
            .join(format!("{session_sha256}.json")),
        &session_bytes,
    )?;
    let draft = ReviewDraft {
        schema: DRAFT_SCHEMA.to_owned(),
        session_sha256: session_sha256.clone(),
        diagnostic_sha256: verified.diagnostic_sha256.clone(),
        source_session_id: manifest.session_id,
        canonical_frames: frames,
        observation_count: verified.observation_count,
        completeness: manifest.completeness,
    };
    publish_document(review_draft, &canonical_json(&draft)?)?;
    Ok(DiagnosticImportSummary {
        schema: "scorepeek-private-diagnostic-import-v1",
        session_sha256,
        diagnostic_sha256: verified.diagnostic_sha256,
        review_draft: review_draft.to_owned(),
        canonical_frame_count: draft.canonical_frames.len(),
    })
}

pub fn inspect_review(path: &Path) -> Result<Value, CorpusError> {
    let (draft, _) = read_json::<ReviewDraft>(path)?;
    if draft.schema != DRAFT_SCHEMA || !valid_sha256(&draft.session_sha256) {
        return invalid("review draft is invalid");
    }
    serde_json::to_value(draft).map_err(CorpusError::Json)
}

pub fn apply_review(
    store: &Path,
    draft_path: &Path,
    labels_path: &Path,
) -> Result<ReviewApplySummary, CorpusError> {
    ensure_store(store)?;
    let (draft, _) = read_json::<ReviewDraft>(draft_path)?;
    let (label, label_bytes) = read_json::<RegressionLabel>(labels_path)?;
    validate_label(&draft, &label)?;
    let label_sha256 = digest(&label_bytes);
    publish_document(
        &store.join("labels").join(format!("{label_sha256}.json")),
        &label_bytes,
    )?;
    let previous = load_active_suite(store)?;
    let mut entries = previous
        .as_ref()
        .map_or_else(Vec::new, |(_, suite)| suite.entries.clone());
    entries.retain(|entry| entry.session_sha256 != label.session_sha256);
    if label.disposition == LabelDisposition::Include {
        entries.push(SuiteEntry {
            session_sha256: label.session_sha256.clone(),
            label_sha256: label_sha256.clone(),
        });
    }
    entries.sort_by(|left, right| left.session_sha256.cmp(&right.session_sha256));
    let suite = RegressionSuite {
        schema: SUITE_SCHEMA.to_owned(),
        previous_generation_sha256: previous.map(|(digest, _)| digest),
        entries,
    };
    let suite_bytes = canonical_json(&suite)?;
    let generation_sha256 = digest(&suite_bytes);
    publish_document(
        &store
            .join("suites")
            .join(format!("{generation_sha256}.json")),
        &suite_bytes,
    )?;
    publish_active(store, &generation_sha256)?;
    Ok(ReviewApplySummary {
        schema: "scorepeek-private-review-apply-v1",
        session_sha256: label.session_sha256,
        label_sha256,
        generation_sha256,
        active_entries: suite.entries.len(),
    })
}

pub fn author_numeric_dataset(
    store: &Path,
    output: &Path,
) -> Result<NumericDatasetAuthoringSummary, CorpusError> {
    if !store.is_absolute() || !output.is_absolute() || output.exists() {
        return invalid("numeric dataset paths must be absolute and output must not exist");
    }
    let Some((suite_sha256, suite)) = load_active_suite(store)? else {
        return invalid("active regression suite is unavailable");
    };
    let parent = output.parent().ok_or_else(|| {
        CorpusError::InvalidRequest("numeric dataset output has no parent".into())
    })?;
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".scorepeek-numeric-dataset-")
        .tempdir_in(parent)?;
    let images = staging.path().join("images");
    fs::create_dir(&images)?;
    let mut crop_candidates = BTreeMap::<String, (Vec<NumericDatasetSample>, Vec<u8>)>::new();
    let mut episode_count = 0;
    for entry in &suite.entries {
        let (session, _) = read_json::<CaptureSession>(
            &store
                .join("sessions")
                .join(format!("{}.json", entry.session_sha256)),
        )?;
        let (label, _) = read_json::<RegressionLabel>(
            &store
                .join("labels")
                .join(format!("{}.json", entry.label_sha256)),
        )?;
        if !matches!(
            label.schema.as_str(),
            LABEL_SCHEMA | PREVIOUS_LABEL_SCHEMA | PREVIOUS_LABEL_SCHEMA_V3
        ) {
            return invalid("numeric dataset requires v3-v5 labels for every active session");
        }
        let screen_sequences = numeric_screen_sequences(store, &session)?;
        for episode in &label.episodes {
            episode_count += 1;
            let field_labels = numeric_field_labels(&episode.expected_result)?;
            let mut episode_crops = BTreeSet::new();
            let mut episode_field_counts = BTreeMap::<NumericField, usize>::new();
            for sequence in
                numeric_episode_sequences(&screen_sequences, &episode.stable_sequences, usize::MAX)?
            {
                let artifact_sha256 = session
                    .canonical_frames
                    .iter()
                    .find(|frame| frame.sequence == sequence)
                    .map(|frame| frame.artifact_sha256.as_str())
                    .ok_or_else(|| {
                        CorpusError::InvalidRequest("numeric dataset frame is unavailable".into())
                    })?;
                let encoded = fs::read(store.join("objects").join(artifact_sha256))?;
                if digest(&encoded) != *artifact_sha256 {
                    return invalid("numeric dataset frame digest differs");
                }
                let (header, pixels) = qoi::decode_to_vec(encoded).map_err(|_| {
                    CorpusError::InvalidRequest("numeric dataset QOI is invalid".into())
                })?;
                if header.width != 1_920
                    || header.height != 1_080
                    || pixels.len() != 1_920 * 1_080 * 3
                    || inspect_canonical_rgb8(&pixels)
                        .map_err(|_| {
                            CorpusError::InvalidRequest(
                                "numeric dataset screen predicate failed".into(),
                            )
                        })?
                        .screen
                        != ScreenClass::Result
                {
                    return invalid("numeric dataset frame is not a canonical result frame");
                }
                let ScreenRgb8Crops::Result(crops) =
                    route_screen_rgb8_crops(&pixels, ScreenClass::Result).map_err(|_| {
                        CorpusError::InvalidRequest("numeric dataset crop routing failed".into())
                    })?
                else {
                    unreachable!("result routing returns result crops");
                };
                for (field, label, crop) in numeric_crops(&crops, &field_labels) {
                    if !numeric_field_uses_sequence(field, sequence, &episode.stable_sequences) {
                        continue;
                    }
                    let bytes = ppm_bytes(crop)?;
                    let crop_sha256 = digest(&bytes);
                    if !episode_crops.insert((field, crop_sha256.clone())) {
                        continue;
                    }
                    let field_count = episode_field_counts.entry(field).or_default();
                    if *field_count >= 32 {
                        continue;
                    }
                    *field_count += 1;
                    let filename = format!("images/{crop_sha256}.ppm");
                    let sample = NumericDatasetSample {
                        session_sha256: entry.session_sha256.clone(),
                        episode_id: episode.episode_id.clone(),
                        split: entry.session_sha256.clone(),
                        sequence,
                        field,
                        label,
                        crop_sha256: crop_sha256.clone(),
                        filename,
                        roi: crop.roi,
                    };
                    let candidates = crop_candidates
                        .entry(crop_sha256)
                        .or_insert_with(|| (Vec::new(), bytes));
                    if let Some(existing) = candidates.0.first().filter(|existing| {
                        existing.field != sample.field || existing.label != sample.label
                    }) {
                        return invalid(&format!(
                            "numeric crop {} conflicts: {}:{}:{:?}={} versus {}:{}:{:?}={}",
                            sample.crop_sha256,
                            existing.session_sha256,
                            existing.episode_id,
                            existing.field,
                            existing.label,
                            sample.session_sha256,
                            sample.episode_id,
                            sample.field,
                            sample.label,
                        ));
                    }
                    candidates.0.push(sample);
                }
            }
        }
    }
    let mut samples = Vec::new();
    for (_, (mut candidates, bytes)) in crop_candidates {
        let sessions = candidates
            .iter()
            .map(|sample| sample.session_sha256.as_str())
            .collect::<BTreeSet<_>>();
        if sessions.len() != 1 {
            continue;
        }
        let sample = candidates.remove(0);
        fs::write(staging.path().join(&sample.filename), bytes)?;
        samples.push(sample);
    }
    samples.sort_by(|left, right| {
        (
            &left.session_sha256,
            &left.episode_id,
            left.sequence,
            left.field,
            &left.crop_sha256,
        )
            .cmp(&(
                &right.session_sha256,
                &right.episode_id,
                right.sequence,
                right.field,
                &right.crop_sha256,
            ))
    });
    let manifest = NumericDatasetManifest {
        schema: NUMERIC_DATASET_SCHEMA,
        suite_sha256: suite_sha256.clone(),
        dictionary: scorepeek::recognition::NUMERIC_DICTIONARY,
        maximum_text_length: 4,
        samples,
    };
    let manifest_bytes = canonical_json(&manifest)?;
    let manifest_sha256 = digest(&manifest_bytes);
    fs::write(staging.path().join("manifest.json"), manifest_bytes)?;
    let samples = manifest.samples.len();
    let unique_crop_count = samples;
    let staging_path = staging.keep();
    fs::rename(staging_path, output)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(NumericDatasetAuthoringSummary {
        schema: "scorepeek-private-numeric-ctc-dataset-authoring-v1",
        suite_sha256,
        sessions: suite.entries.len(),
        episodes: episode_count,
        samples,
        unique_crops: unique_crop_count,
        output: output.to_owned(),
        manifest_sha256,
    })
}

pub fn author_numeric_sentinel(
    frame: &Path,
    frame_sha256: &str,
    labels: &Path,
    labels_sha256: &str,
    output: &Path,
) -> Result<NumericSentinelAuthoringSummary, CorpusError> {
    if !frame.is_absolute()
        || !labels.is_absolute()
        || !output.is_absolute()
        || output.exists()
        || !valid_sha256(frame_sha256)
        || !valid_sha256(labels_sha256)
    {
        return invalid("numeric sentinel inputs must be absolute, digest-bound, and create-only");
    }
    let encoded = fs::read(frame)?;
    if digest(&encoded) != frame_sha256 {
        return invalid("numeric sentinel frame digest differs");
    }
    let label_bytes = fs::read(labels)?;
    if digest(&label_bytes) != labels_sha256 {
        return invalid("numeric sentinel labels digest differs");
    }
    let request: NumericSentinelRequest = serde_json::from_slice(&label_bytes)?;
    if request.schema != "scorepeek-private-numeric-ctc-sentinel-request-v1"
        || request.sentinel_id.is_empty()
        || request.sentinel_id.len() > 128
        || request.labels.len() != NumericField::ALL.len()
        || NumericField::ALL
            .iter()
            .any(|field| !valid_numeric_label(*field, request.labels.get(field)))
    {
        return invalid("numeric sentinel labels are invalid");
    }
    let (header, pixels) = qoi::decode_to_vec(encoded)
        .map_err(|_| CorpusError::InvalidRequest("numeric sentinel QOI is invalid".into()))?;
    if header.width != 1_920
        || header.height != 1_080
        || pixels.len() != 1_920 * 1_080 * 3
        || inspect_canonical_rgb8(&pixels)
            .map_err(|_| {
                CorpusError::InvalidRequest("numeric sentinel screen predicate failed".into())
            })?
            .screen
            != ScreenClass::Result
    {
        return invalid("numeric sentinel frame is not a canonical result frame");
    }
    let ScreenRgb8Crops::Result(crops) = route_screen_rgb8_crops(&pixels, ScreenClass::Result)
        .map_err(|_| CorpusError::InvalidRequest("numeric sentinel crop routing failed".into()))?
    else {
        unreachable!("result routing returns result crops");
    };
    let parent = output.parent().ok_or_else(|| {
        CorpusError::InvalidRequest("numeric sentinel output has no parent".into())
    })?;
    fs::create_dir_all(parent)?;
    let staging = tempfile::Builder::new()
        .prefix(".scorepeek-numeric-sentinel-")
        .tempdir_in(parent)?;
    let images = staging.path().join("images");
    fs::create_dir(&images)?;
    let mut samples = Vec::new();
    for (field, label, crop) in numeric_crops(&crops, &request.labels) {
        let bytes = ppm_bytes(crop)?;
        let crop_sha256 = digest(&bytes);
        let filename = format!("images/{crop_sha256}.ppm");
        let target = staging.path().join(&filename);
        if !target.exists() {
            fs::write(&target, bytes)?;
        }
        samples.push(NumericSentinelSample {
            field,
            label,
            crop_sha256,
            filename,
            roi: crop.roi,
        });
    }
    let manifest = NumericSentinelManifest {
        schema: "scorepeek-private-numeric-ctc-sentinel-v1",
        sentinel_id: request.sentinel_id.clone(),
        frame_sha256: frame_sha256.to_owned(),
        labels_sha256: labels_sha256.to_owned(),
        dictionary: scorepeek::recognition::NUMERIC_DICTIONARY,
        maximum_text_length: 4,
        samples,
    };
    let manifest_bytes = canonical_json(&manifest)?;
    let manifest_sha256 = digest(&manifest_bytes);
    fs::write(staging.path().join("manifest.json"), manifest_bytes)?;
    let sample_count = manifest.samples.len();
    let staging_path = staging.keep();
    fs::rename(staging_path, output)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(NumericSentinelAuthoringSummary {
        schema: "scorepeek-private-numeric-ctc-sentinel-authoring-v1",
        sentinel_id: request.sentinel_id,
        frame_sha256: frame_sha256.to_owned(),
        labels_sha256: labels_sha256.to_owned(),
        samples: sample_count,
        output: output.to_owned(),
        manifest_sha256,
    })
}

fn valid_numeric_label(field: NumericField, label: Option<&String>) -> bool {
    let Some(label) = label else {
        return false;
    };
    if field.allows_dash() && label == "--" {
        return true;
    }
    !label.is_empty()
        && label.len() <= field.maximum_digits()
        && label.bytes().all(|byte| byte.is_ascii_digit())
}

fn numeric_screen_sequences(
    store: &Path,
    session: &CaptureSession,
) -> Result<Vec<(u64, bool)>, CorpusError> {
    let mut sequences = Vec::with_capacity(session.canonical_frames.len());
    for frame in &session.canonical_frames {
        let encoded = fs::read(store.join("objects").join(&frame.artifact_sha256))?;
        if digest(&encoded) != frame.artifact_sha256 {
            return invalid("numeric dataset frame digest differs");
        }
        let (header, pixels) = qoi::decode_to_vec(encoded)
            .map_err(|_| CorpusError::InvalidRequest("numeric dataset QOI is invalid".into()))?;
        if header.width != 1_920 || header.height != 1_080 || pixels.len() != 1_920 * 1_080 * 3 {
            return invalid("numeric dataset frame is not canonical RGB8");
        }
        let is_result = inspect_canonical_rgb8(&pixels)
            .map_err(|_| {
                CorpusError::InvalidRequest("numeric dataset screen predicate failed".into())
            })?
            .screen
            == ScreenClass::Result;
        sequences.push((frame.sequence, is_result));
    }
    Ok(sequences)
}

fn numeric_episode_sequences(
    screen_sequences: &[(u64, bool)],
    stable_sequences: &[u64],
    limit: usize,
) -> Result<Vec<u64>, CorpusError> {
    if stable_sequences.is_empty() || limit == 0 {
        return invalid("numeric dataset episode has no stable sequence");
    }
    let mut episode_indices = BTreeSet::new();
    for stable in stable_sequences {
        let Some(index) = screen_sequences
            .iter()
            .position(|(sequence, is_result)| sequence == stable && *is_result)
        else {
            return invalid("numeric dataset stable frame is not a result frame");
        };
        episode_indices.insert(index);
        let mut before = index;
        while before > 0 && screen_sequences[before - 1].1 {
            before -= 1;
            episode_indices.insert(before);
        }
        let mut after = index;
        while after + 1 < screen_sequences.len() && screen_sequences[after + 1].1 {
            after += 1;
            episode_indices.insert(after);
        }
    }

    let mut candidates = episode_indices.into_iter().collect::<Vec<_>>();
    candidates.sort_by_key(|index| {
        let sequence = screen_sequences[*index].0;
        (
            stable_sequences
                .iter()
                .map(|stable| sequence.abs_diff(*stable))
                .min()
                .unwrap_or(u64::MAX),
            sequence,
        )
    });
    candidates.truncate(limit);
    Ok(candidates
        .into_iter()
        .map(|index| screen_sequences[index].0)
        .collect())
}

fn numeric_field_labels(
    expected: &ExpectedResult,
) -> Result<BTreeMap<NumericField, String>, CorpusError> {
    let judgments = expected.judgments.as_ref().ok_or_else(|| {
        CorpusError::InvalidRequest("numeric dataset judgments are absent".into())
    })?;
    let timing = expected
        .timing
        .as_ref()
        .ok_or_else(|| CorpusError::InvalidRequest("numeric dataset timing is absent".into()))?;
    let previous = expected.previous_best.as_ref().ok_or_else(|| {
        CorpusError::InvalidRequest("numeric dataset previous best is absent".into())
    })?;
    let mut labels = BTreeMap::new();
    labels.insert(NumericField::Level, expected.level.to_string());
    labels.insert(NumericField::Notes, format!("{:04}", expected.notes));
    labels.insert(
        NumericField::CurrentScore,
        expected.current_score.to_string(),
    );
    labels.insert(
        NumericField::PreviousScore,
        previous_numeric_label(&previous.score, true)?,
    );
    labels.insert(
        NumericField::PreviousMissCount,
        previous_numeric_label(&previous.miss_count, false)?,
    );
    labels.insert(
        NumericField::MissCount,
        supplemental_label(expected.miss_count.as_ref())?,
    );
    labels.insert(NumericField::Pgreat, judgments.pgreat.to_string());
    labels.insert(NumericField::Great, judgments.great.to_string());
    labels.insert(NumericField::Good, judgments.good.to_string());
    labels.insert(NumericField::Bad, judgments.bad.to_string());
    labels.insert(NumericField::Poor, judgments.poor.to_string());
    labels.insert(NumericField::Fast, supplemental_label(Some(&timing.fast))?);
    labels.insert(NumericField::Slow, supplemental_label(Some(&timing.slow))?);
    labels.insert(
        NumericField::ComboBreak,
        supplemental_label(expected.combo_break.as_ref())?,
    );
    Ok(labels)
}

fn numeric_field_uses_sequence(
    field: NumericField,
    sequence: u64,
    stable_sequences: &[u64],
) -> bool {
    !matches!(field, NumericField::Level | NumericField::Notes)
        || stable_sequences.contains(&sequence)
}

fn supplemental_label(value: Option<&SupplementalResultValue<u32>>) -> Result<String, CorpusError> {
    match value {
        Some(SupplementalResultValue::Known { value }) => Ok(value.to_string()),
        Some(SupplementalResultValue::NotDisplayed) => Ok("--".to_owned()),
        Some(SupplementalResultValue::Unknown { .. }) | None => {
            invalid("numeric dataset cannot train an unknown supplemental value")
        }
    }
}

fn previous_numeric_label(
    value: &PreviousBestValue<u32>,
    zero_when_not_played: bool,
) -> Result<String, CorpusError> {
    match value {
        PreviousBestValue::Known { value } => Ok(value.to_string()),
        PreviousBestValue::NotPlayed if zero_when_not_played => Ok("0".to_owned()),
        PreviousBestValue::NotPlayed | PreviousBestValue::NotDisplayed => Ok("--".to_owned()),
        PreviousBestValue::Unknown { .. } => {
            invalid("numeric dataset cannot train an unknown previous value")
        }
    }
}

fn numeric_crops<'a>(
    crops: &'a scorepeek::recognition::ResultScreenRgb8Crops,
    labels: &BTreeMap<NumericField, String>,
) -> Vec<(NumericField, String, &'a Rgb8Crop)> {
    [
        (NumericField::Level, &crops.level),
        (NumericField::Notes, &crops.notes),
        (NumericField::CurrentScore, &crops.current_score),
        (NumericField::PreviousScore, &crops.previous_score),
        (NumericField::PreviousMissCount, &crops.previous_miss_count),
        (NumericField::MissCount, &crops.miss_count),
        (NumericField::Pgreat, &crops.pgreat),
        (NumericField::Great, &crops.great),
        (NumericField::Good, &crops.good),
        (NumericField::Bad, &crops.bad),
        (NumericField::Poor, &crops.poor),
        (NumericField::Fast, &crops.fast),
        (NumericField::Slow, &crops.slow),
        (NumericField::ComboBreak, &crops.combo_break),
    ]
    .into_iter()
    .filter_map(|(field, crop)| labels.get(&field).map(|label| (field, label.clone(), crop)))
    .collect()
}

fn ppm_bytes(crop: &Rgb8Crop) -> Result<Vec<u8>, CorpusError> {
    let mut bytes = format!("P6\n{} {}\n255\n", crop.roi.width, crop.roi.height).into_bytes();
    bytes.extend_from_slice(crop.pixels());
    if bytes.len() > 1024 * 1024 {
        return invalid("numeric dataset crop exceeds its bound");
    }
    Ok(bytes)
}

pub fn replay_corpus(store: &Path) -> Result<CorpusReplaySummary, CorpusError> {
    let Some((generation_sha256, suite)) = load_active_suite(store)? else {
        return invalid("active regression suite is unavailable");
    };
    let mut episodes = 0;
    let mut canonical_frames = 0;
    let mut negatives = 0;
    let bundle = scorepeek::model_cache::ensure_small_model(None, |_| {})
        .map_err(|error| CorpusError::InvalidReplay(format!("model cache failed: {error}")))?;
    let catalog_root = default_catalog_root()?;
    let diagnostic_root = tempfile::tempdir()?;
    let mut replay_failures = Vec::new();
    for (session_index, entry) in suite.entries.iter().enumerate() {
        let (session, session_bytes) = read_json::<CaptureSession>(
            &store
                .join("sessions")
                .join(format!("{}.json", entry.session_sha256)),
        )?;
        let (label, label_bytes) = read_regression_label(
            &store
                .join("labels")
                .join(format!("{}.json", entry.label_sha256)),
        )?;
        if session.schema != SESSION_SCHEMA
            || digest(&session_bytes) != entry.session_sha256
            || digest(&label_bytes) != entry.label_sha256
            || label.session_sha256 != entry.session_sha256
            || !matches!(
                session.completeness.as_str(),
                "complete" | "partial" | "dropped"
            )
            || session.processed_ticks.saturating_add(session.busy_skips) == 0
            || label.episodes.windows(2).any(|pair| {
                pair[0]
                    .stable_sequences
                    .last()
                    .zip(pair[1].stable_sequences.first())
                    .is_none_or(|(left, right)| left >= right)
            })
        {
            return invalid("suite entry binding is invalid");
        }
        let frame_map = session_frame_map(&session);
        let binding = session_binding(store, &session)?;
        replay_normalization_pairs(store, &session)?;
        let descriptor = scorepeek::diagnostic_recording::DiagnosticRunDescriptor {
            run_id: format!("corpus-replay-{session_index}"),
            monotonic_start_ms: 0,
            resource: scorepeek::diagnostic_recording::DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "0".repeat(64),
            },
            binding: scorepeek::diagnostic_recording::DiagnosticBinding {
                capture_generation: 1,
                capture_profile_sha256: binding.capture_profile_sha256.clone(),
                normalizer_sha256: binding.normalizer_sha256.clone(),
                canonical_layout_sha256: scorepeek::recognition::CanonicalLayout::sha256(),
                catalog_sha256: session.catalog_sha256.clone(),
                model_sha256: scorepeek::recognition::LIVE_MODEL_SHA256.to_owned(),
                runtime_sha256: scorepeek::recognition::LIVE_RUNTIME_SHA256.to_owned(),
                replay: None,
            },
        };
        let mut recognition =
            scorepeek::recognition_live::field_session::FieldObservationSession::start_registered(
                diagnostic_root.path(),
                descriptor,
                scorepeek::diagnostic_recording::DiagnosticPolicy {
                    enabled: false,
                    ..scorepeek::diagnostic_recording::DiagnosticPolicy::default()
                },
                &catalog_root,
                &bundle,
                scorepeek::recognition_live::text_observer_pool::RecognitionExecutionMode::Offline,
            )
            .map_err(|error| {
                CorpusError::InvalidReplay(format!(
                    "production recognizer could not start: {error:?}"
                ))
            })?;
        for artifact_sha256 in frame_map.values() {
            let pixels = read_canonical_object(store, artifact_sha256)?;
            scorepeek::recognition::inspect_canonical_rgb8(&pixels)
                .map_err(|_| CorpusError::InvalidReplay("scene predicate failed".to_owned()))?;
            canonical_frames += 1;
        }
        for sequence in &label.negative_frames {
            let pixels = read_canonical_object(
                store,
                frame_map.get(sequence).ok_or_else(|| {
                    CorpusError::InvalidReplay("labeled negative frame is unavailable".to_owned())
                })?,
            )?;
            if scorepeek::recognition::inspect_canonical_rgb8(&pixels)
                .map_err(|_| CorpusError::InvalidReplay("scene predicate failed".to_owned()))?
                .screen
                != scorepeek::recognition::ScreenClass::Unknown
            {
                return invalid_replay("negative frame is no longer unknown");
            }
            negatives += 1;
        }
        for episode in &label.episodes {
            if episode.stable_sequences.is_empty() {
                return invalid_replay("episode has no stable frame");
            }
            for sequence in &episode.stable_sequences {
                let pixels = read_canonical_object(
                    store,
                    frame_map.get(sequence).ok_or_else(|| {
                        CorpusError::InvalidReplay("stable frame is unavailable".to_owned())
                    })?,
                )?;
                if scorepeek::recognition::inspect_canonical_rgb8(&pixels)
                    .map_err(|_| CorpusError::InvalidReplay("scene predicate failed".to_owned()))?
                    .screen
                    != scorepeek::recognition::ScreenClass::Result
                {
                    return invalid_replay("stable result frame is no longer a result");
                }
                let frame = scorepeek::diagnostic_live::BoundCanonicalFrame::for_replay(
                    1,
                    *sequence,
                    sequence.saturating_mul(100),
                    binding.capture_profile_sha256.clone(),
                    binding.normalizer_sha256.clone(),
                    pixels.into_boxed_slice(),
                )
                .map_err(|_| {
                    CorpusError::InvalidReplay("canonical replay frame is invalid".to_owned())
                })?;
                let inspected = recognition.inspect(&frame).map_err(|_| {
                    CorpusError::InvalidReplay("production frame inspection failed".to_owned())
                })?;
                let scorepeek::recognition_live::field_session::FieldObservationSubmission::Submitted(pending) = inspected.field_submission else {
                    return invalid_replay("stable result frame was not submitted for OCR");
                };
                let scorepeek::recognition_live::field_session::FieldObservationSessionPoll::Ready { observation, .. } = recognition.wait_field_observation(
                    &pending,
                    Duration::from_secs(5),
                ) else {
                    return invalid_replay("production OCR did not complete");
                };
                let output = observation
                    .output()
                    .as_ref()
                    .map_err(|_| CorpusError::InvalidReplay("production OCR failed".to_owned()))?;
                let scorepeek::recognition::ScreenFieldObservations::Result(fields) =
                    output.fields()
                else {
                    return invalid_replay("stable frame produced non-result fields");
                };
                let observed_song = output
                    .result_resolution()
                    .and_then(scorepeek::recognition::ResultSongResolution::accepted_song_id)
                    .map(|song| song.as_uuid().to_string());
                if output.clear_type() != Some(episode.expected_clear_type.as_str())
                    || observed_song.as_deref() != Some(&episode.expected_song_id)
                {
                    replay_failures.push(format!(
                        "episode {} tick {} differs: expected song={} clear_type={:?}, observed song={} clear_type={:?} title_ocr={:?} artist_ocr={:?} resolution={:?}",
                        episode.episode_id,
                        sequence,
                        episode.expected_song_id,
                        episode.expected_clear_type,
                        observed_song.as_deref().unwrap_or("unresolved"),
                        output.clear_type().unwrap_or("unresolved"),
                        fields.title.open_text,
                        fields.artist.open_text,
                        output.result_resolution(),
                    ));
                }
                let expected = &episode.expected_result;
                if !expected_play_options_match(
                    &fields.play_options.parsed,
                    expected.play_options.as_deref(),
                ) {
                    replay_failures.push(format!(
                        "episode {} tick {} play options differ: expected={:?}, observed={:?}",
                        episode.episode_id,
                        sequence,
                        expected.play_options,
                        fields.play_options.parsed,
                    ));
                }
                match output.result_chart_resolution() {
                    Some(ResultChartResolution::Accepted {
                        chart,
                        current_score,
                        ..
                    }) => {
                        if expected.play_side != "one_player"
                            || expected.play_mode != "single_play"
                            || chart.key.play_type != expected.play_type
                            || chart.key.difficulty != expected.difficulty
                            || chart.level != expected.level
                            || chart.notes != expected.notes
                            || *current_score != expected.current_score
                        {
                            replay_failures.push(format!(
                                "episode {} tick {} result context differs: expected={:?}, observed_chart={:?}, observed_score={}",
                                episode.episode_id, sequence, expected, chart, current_score,
                            ));
                        }
                    }
                    resolution => replay_failures.push(format!(
                        "episode {} tick {} result context unresolved: expected={:?}, raw difficulty={:?} level={:?} notes={:?} score={:?}, parsed={:?}, resolution={:?}, numeric={:?}",
                        episode.episode_id,
                        sequence,
                        expected,
                        fields.difficulty.open_text,
                        fields.level.open_text,
                        fields.notes.open_text,
                        fields.current_score.open_text,
                        output.parsed_result_fields(),
                        resolution,
                        output.numeric_batch(),
                    )),
                }
                if let (
                    Some(expected_judgments),
                    Some(expected_miss_count),
                    Some(expected_timing),
                    Some(expected_combo_break),
                    Some(expected_previous_best),
                ) = (
                    expected.judgments.as_ref(),
                    expected.miss_count.as_ref(),
                    expected.timing.as_ref(),
                    expected.combo_break.as_ref(),
                    expected.previous_best.as_ref(),
                ) {
                    match output.result_performance_resolution() {
                        Some(ResultPerformanceResolution::Accepted {
                            judgments,
                            miss_count,
                            timing,
                            combo_break,
                            previous_best,
                            ..
                        }) if judgments == expected_judgments
                            && optional_supplemental_matches(miss_count, expected_miss_count)
                            && optional_supplemental_matches(&timing.fast, &expected_timing.fast)
                            && optional_supplemental_matches(&timing.slow, &expected_timing.slow)
                            && optional_supplemental_matches(combo_break, expected_combo_break)
                            && optional_previous_matches(
                                &previous_best.clear_type,
                                &expected_previous_best.clear_type,
                            )
                            && optional_previous_matches(
                                &previous_best.score,
                                &expected_previous_best.score,
                            )
                            && optional_previous_matches(
                                &previous_best.miss_count,
                                &expected_previous_best.miss_count,
                            ) => {}
                        resolution => replay_failures.push(format!(
                            "episode {} tick {} performance differs: expected judgments={:?} miss_count={:?} timing={:?} combo_break={:?} previous_best={:?}, observed={:?}, numeric={:?}",
                            episode.episode_id,
                            sequence,
                            expected_judgments,
                            expected_miss_count,
                            expected_timing,
                            expected_combo_break,
                            expected_previous_best,
                            resolution,
                            output.numeric_batch(),
                        )),
                    }
                }
            }
            episodes += 1;
        }
        let finish = recognition.finish(
            scorepeek::diagnostic_recording::DiagnosticRunStatus::Success,
            1,
            Duration::from_secs(5),
        );
        if finish.field_observer.status
            != scorepeek::recognition_live::field_observer::FieldObserverFinishStatus::Complete
        {
            return invalid_replay("production recognizer did not finish cleanly");
        }
    }
    if !replay_failures.is_empty() {
        return Err(CorpusError::InvalidReplay(replay_failures.join("; ")));
    }
    Ok(CorpusReplaySummary {
        schema: "scorepeek-private-corpus-replay-v1",
        generation_sha256,
        session_count: suite.entries.len(),
        episode_count: episodes,
        canonical_frames,
        negative_frames: negatives,
    })
}

fn optional_supplemental_matches<T: PartialEq>(
    observed: &SupplementalResultValue<T>,
    expected: &SupplementalResultValue<T>,
) -> bool {
    observed == expected || matches!(observed, SupplementalResultValue::Unknown { .. })
}

fn optional_previous_matches<T: PartialEq>(
    observed: &PreviousBestValue<T>,
    expected: &PreviousBestValue<T>,
) -> bool {
    observed == expected || matches!(observed, PreviousBestValue::Unknown { .. })
}

fn expected_play_options_match(observed: &PlayOptions, expected: Option<&[PlayOption]>) -> bool {
    expected.is_none_or(
        |expected| matches!(observed, PlayOptions::Known { values } if values == expected),
    )
}

#[derive(Deserialize)]
struct SessionBinding {
    capture_profile_sha256: String,
    normalizer_sha256: String,
}

#[derive(Deserialize)]
struct SessionRunDocument {
    binding: SessionBinding,
}

fn session_binding(store: &Path, session: &CaptureSession) -> Result<SessionBinding, CorpusError> {
    let run = session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == "capture/run.json")
        .ok_or_else(|| {
            CorpusError::InvalidReplay("session run binding is unavailable".to_owned())
        })?;
    let (document, _) = read_json::<SessionRunDocument>(&store.join("objects").join(&run.sha256))?;
    if !valid_sha256(&document.binding.capture_profile_sha256)
        || !valid_sha256(&document.binding.normalizer_sha256)
    {
        return invalid_replay("session run binding is invalid");
    }
    Ok(document.binding)
}

fn replay_normalization_pairs(store: &Path, session: &CaptureSession) -> Result<(), CorpusError> {
    if session.normalization_pairs.is_empty() {
        return Ok(());
    }
    let profile_artifact = session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == "capture/profile.json")
        .ok_or_else(|| {
            CorpusError::InvalidReplay("normalization profile is unavailable".to_owned())
        })?;
    let profile_bytes = fs::read(store.join("objects").join(&profile_artifact.sha256))?;
    let profile = scorepeek::capture::GamescopeProfileBinding::parse(
        &profile_bytes,
        &profile_artifact.sha256,
    )
    .map_err(|_| CorpusError::InvalidReplay("normalization profile is invalid".to_owned()))?;
    if profile.capture_profile_sha256() != session.profile_sha256 {
        return invalid_replay("normalization profile binding differs");
    }
    let frame_map = session_frame_map(session);
    for pair in &session.normalization_pairs {
        if frame_map.get(&pair.sequence) != Some(&pair.canonical_sha256) {
            return invalid_replay("normalization pair canonical binding differs");
        }
        let observed_path = store.join("objects").join(&pair.observed_sha256);
        let observed_pixels = u64::from(profile.observed_width())
            .checked_mul(u64::from(profile.observed_height()))
            .ok_or_else(|| {
                CorpusError::InvalidReplay("observed QOI dimensions overflow".to_owned())
            })?;
        let encoded_bound = observed_pixels
            .checked_mul(5)
            .and_then(|bytes| bytes.checked_add(22))
            .map_or(MAX_ARTIFACT_BYTES, |bytes| bytes.min(MAX_ARTIFACT_BYTES));
        let encoded = read_bounded_qoi_with_limit(&observed_path, encoded_bound)?;
        let header = qoi::decode_header(&encoded)
            .map_err(|_| CorpusError::InvalidReplay("observed QOI header is invalid".to_owned()))?;
        if header.width != profile.observed_width() || header.height != profile.observed_height() {
            return invalid_replay("observed QOI dimensions differ from the profile");
        }
        let (header, rgb) = qoi::decode_to_vec(&encoded)
            .map_err(|_| CorpusError::InvalidReplay("observed QOI is invalid".to_owned()))?;
        if rgb.len()
            != usize::try_from(header.width)
                .unwrap_or(usize::MAX)
                .saturating_mul(usize::try_from(header.height).unwrap_or(usize::MAX))
                .saturating_mul(3)
        {
            return invalid_replay("observed QOI dimensions differ from the profile");
        }
        let mut bgrx = Vec::with_capacity((rgb.len() / 3).saturating_mul(4));
        for pixel in rgb.chunks_exact(3) {
            bgrx.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0]);
        }
        let stride = profile
            .observed_width()
            .checked_mul(4)
            .ok_or_else(|| CorpusError::InvalidReplay("observed stride overflows".to_owned()))?;
        let normalized = profile
            .geometry()
            .normalize_bgrx_bytes(
                &bgrx,
                profile.observed_width(),
                profile.observed_height(),
                stride,
            )
            .map_err(|_| {
                CorpusError::InvalidReplay("production normalization failed".to_owned())
            })?;
        let canonical = read_canonical_object(store, &pair.canonical_sha256)?;
        if normalized.as_ref() != canonical {
            return invalid_replay("observed-to-canonical normalization regressed");
        }
    }
    Ok(())
}

fn default_catalog_root() -> Result<PathBuf, CorpusError> {
    if let Some(data) = env::var_os("XDG_DATA_HOME") {
        let path = PathBuf::from(data);
        if path.is_absolute() {
            return Ok(path.join("scorepeek/catalog"));
        }
        return invalid_replay("XDG_DATA_HOME must be absolute");
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| CorpusError::InvalidReplay("HOME is required".to_owned()))?;
    Ok(home.join(".local/share/scorepeek/catalog"))
}

fn parse_timestamp_ms(value: &str) -> Result<u64, CorpusError> {
    let (seconds, fraction) = value.split_once('.').unwrap_or((value, ""));
    if seconds.starts_with('-')
        || seconds.is_empty()
        || !seconds.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return invalid("video frame timestamp is invalid");
    }
    let seconds = seconds
        .parse::<u64>()
        .map_err(|_| CorpusError::InvalidRequest("video frame timestamp overflows".to_owned()))?;
    let mut milliseconds = 0_u64;
    for (index, byte) in fraction.bytes().take(3).enumerate() {
        milliseconds =
            milliseconds.saturating_add(u64::from(byte - b'0') * [100_u64, 10, 1][index]);
    }
    seconds
        .checked_mul(1_000)
        .and_then(|whole| whole.checked_add(milliseconds))
        .ok_or_else(|| CorpusError::InvalidRequest("video frame timestamp overflows".to_owned()))
}

fn video_timeline_event(
    event: &str,
    session_id: &str,
    source_timestamp_ms: u64,
    previous_source_timestamp_ms: u64,
) -> Value {
    serde_json::json!({
        "schema":"scorepeek-private-diagnostic-event-v1", "event":event,
        "session_id":session_id, "capture_generation":1,
        "source_timestamp_ms":source_timestamp_ms,
        "previous_source_timestamp_ms":previous_source_timestamp_ms,
    })
}

fn profile_root() -> Result<PathBuf, CorpusError> {
    if let Some(config) = env::var_os("XDG_CONFIG_HOME") {
        let path = PathBuf::from(config);
        if path.is_absolute() {
            return Ok(path.join("scorepeek/profiles"));
        }
        return invalid("XDG_CONFIG_HOME must be absolute");
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| CorpusError::InvalidRequest("HOME is required".to_owned()))?;
    Ok(home.join(".config/scorepeek/profiles"))
}

fn find_profile_bytes(expected_sha256: &str) -> Result<Vec<u8>, CorpusError> {
    if !valid_sha256(expected_sha256) {
        return invalid("capture profile digest is invalid");
    }
    for entry in fs::read_dir(profile_root()?)? {
        let entry = entry?;
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let bytes = fs::read(entry.path())?;
            let file_sha256 = digest(&bytes);
            if scorepeek::capture::GamescopeProfileBinding::parse(&bytes, &file_sha256)
                .is_ok_and(|profile| profile.capture_profile_sha256() == expected_sha256)
            {
                return Ok(bytes);
            }
        }
    }
    invalid("capture profile bound by the diagnostic is unavailable")
}

fn field_json(fields: &scorepeek::recognition::ScreenFieldObservations) -> Value {
    match fields {
        scorepeek::recognition::ScreenFieldObservations::Result(fields) => serde_json::json!({
            "screen":"result", "title":fields.title.open_text, "artist":fields.artist.open_text,
            "clear_type":fields.clear_type.open_text
        }),
        scorepeek::recognition::ScreenFieldObservations::MusicSelect(fields) => serde_json::json!({
            "screen":"music_select", "central_title":fields.central_title.open_text,
            "artist":fields.artist.open_text, "active_list_title":fields.active_list_title.open_text
        }),
    }
}

fn validate_diagnostic_manifest(manifest: &DiagnosticManifest) -> Result<(), CorpusError> {
    if !matches!(
        manifest.schema.as_str(),
        DIAGNOSTIC_SCHEMA | LEGACY_DIAGNOSTIC_SCHEMA
    ) || manifest.session_id.is_empty()
        || manifest.capture_generation == 0
        || !valid_sha256(&manifest.profile_sha256)
        || !valid_sha256(&manifest.catalog_sha256)
        || !valid_sha256(&manifest.capture_manifest_sha256)
        || !valid_sha256(&manifest.recognition_manifest_sha256)
        || !valid_sha256(&manifest.event_manifest_sha256)
        || manifest.artifacts.is_empty()
        || manifest.artifacts.len() > MAX_ARTIFACTS
        || manifest.recognition_interval_ms != 100
        || match manifest.schema.as_str() {
            DIAGNOSTIC_SCHEMA => manifest
                .field_observation_busy_skips
                .zip(manifest.maximum_consecutive_field_observation_busy_skips)
                .is_none_or(|(total, maximum)| maximum > total),
            LEGACY_DIAGNOSTIC_SCHEMA => {
                manifest.field_observation_busy_skips.is_some()
                    || manifest
                        .maximum_consecutive_field_observation_busy_skips
                        .is_some()
            }
            _ => true,
        }
    {
        return invalid("diagnostic manifest is invalid");
    }
    for artifact in &manifest.artifacts {
        safe_relative(&artifact.path)?;
        if !valid_sha256(&artifact.sha256)
            || artifact.bytes == 0
            || artifact.bytes > MAX_ARTIFACT_BYTES
        {
            return invalid("diagnostic artifact reference is invalid");
        }
    }
    Ok(())
}

fn find_run_id(diagnostic: &Path, recognition: &Path) -> Option<String> {
    for root in [diagnostic, recognition] {
        let entries = fs::read_dir(root).ok()?;
        let candidates = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with("run-") && name.contains("-session-"))
            .collect::<Vec<_>>();
        if candidates.len() == 1 {
            return candidates.into_iter().next();
        }
    }
    diagnostic
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|name| name.starts_with("run-") && name.contains("-session-"))
        .map(ToOwned::to_owned)
}

fn bgrx_to_rgb(
    bytes: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<u8>, CorpusError> {
    let width = usize::try_from(width)
        .map_err(|_| CorpusError::InvalidRequest("source width is invalid".to_owned()))?;
    let height = usize::try_from(height)
        .map_err(|_| CorpusError::InvalidRequest("source height is invalid".to_owned()))?;
    if stride < width.saturating_mul(4) || stride.checked_mul(height) != Some(bytes.len()) {
        return invalid("legacy BGRx source contract differs");
    }
    let mut rgb = Vec::with_capacity(width.saturating_mul(height).saturating_mul(3));
    for row in bytes.chunks_exact(stride) {
        for pixel in row[..width * 4].chunks_exact(4) {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    Ok(rgb)
}

fn stabilize_capture_manifest(manifest: &mut Value, directory: &Path) -> Result<(), CorpusError> {
    let run_bytes = fs::metadata(directory.join("run.json"))?.len();
    let facts_bytes = fs::metadata(directory.join("facts.ndjson"))?.len();
    let frame_bytes = manifest["frames"]
        .as_array()
        .ok_or_else(|| CorpusError::InvalidRequest("converted frames are invalid".to_owned()))?
        .iter()
        .try_fold(0_u64, |total, frame| {
            total
                .checked_add(frame["bytes"].as_u64().unwrap_or(0))
                .and_then(|value| value.checked_add(frame["source"]["bytes"].as_u64().unwrap_or(0)))
                .ok_or_else(|| {
                    CorpusError::InvalidRequest("converted byte count overflowed".to_owned())
                })
        })?;
    let artifact_bytes = run_bytes
        .checked_add(facts_bytes)
        .and_then(|value| value.checked_add(frame_bytes))
        .ok_or_else(|| CorpusError::InvalidRequest("converted byte count overflowed".to_owned()))?;
    manifest["artifact_bytes"] = Value::from(artifact_bytes);
    let mut manifest_bytes = 0_u64;
    for _ in 0..8 {
        manifest["manifest_bytes"] = Value::from(manifest_bytes);
        manifest["total_bytes"] = Value::from(artifact_bytes.saturating_add(manifest_bytes));
        let encoded = canonical_json(manifest)?;
        let next = encoded.len() as u64;
        if next == manifest_bytes {
            return Ok(());
        }
        manifest_bytes = next;
    }
    invalid("converted manifest size did not stabilize")
}

fn write_event_manifest(
    root: &Path,
    session_id: &str,
    events: &[u8],
    event_count: u64,
) -> Result<Vec<u8>, CorpusError> {
    let bytes = canonical_json(&EventComponentManifest {
        schema: "scorepeek-run-event-artifact-v1".to_owned(),
        run_id: session_id.to_owned(),
        status: "complete".to_owned(),
        events_sha256: digest(events),
        event_count,
        event_bytes: events.len() as u64,
        dropped_events: 0,
    })?;
    write_new(&root.join("event-manifest.json"), &bytes)?;
    Ok(bytes)
}

fn enumerate_component_artifacts(root: &Path) -> Result<Vec<DiagnosticArtifact>, CorpusError> {
    let mut artifacts = Vec::new();
    for kind in ["capture", "recognition"] {
        for entry in fs::read_dir(root.join(kind))? {
            let entry = entry?;
            let metadata = entry.path().metadata()?;
            if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_ARTIFACT_BYTES {
                return invalid("converted diagnostic artifact is invalid");
            }
            let name = entry.file_name().into_string().map_err(|_| {
                CorpusError::InvalidRequest("artifact filename must be UTF-8".to_owned())
            })?;
            artifacts.push(DiagnosticArtifact {
                kind: kind.to_owned(),
                path: format!("{kind}/{name}"),
                sha256: digest_file(&entry.path())?,
                bytes: metadata.len(),
            });
        }
    }
    for name in ["event-manifest.json", "events.ndjson"] {
        let event_artifact = root.join(name);
        if !event_artifact.is_file() {
            continue;
        }
        let metadata = event_artifact.metadata()?;
        artifacts.push(DiagnosticArtifact {
            kind: "events".to_owned(),
            path: name.to_owned(),
            sha256: digest_file(&event_artifact)?,
            bytes: metadata.len(),
        });
    }
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(artifacts)
}

fn create_private_directory(path: &Path) -> Result<(), CorpusError> {
    DirBuilder::new().mode(0o700).create(path)?;
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), CorpusError> {
    let bytes = fs::read(source)?;
    write_new(destination, &bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), CorpusError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn validate_label(draft: &ReviewDraft, label: &RegressionLabel) -> Result<(), CorpusError> {
    if draft.schema != DRAFT_SCHEMA
        || !matches!(
            label.schema.as_str(),
            LABEL_SCHEMA | PREVIOUS_LABEL_SCHEMA | PREVIOUS_LABEL_SCHEMA_V3
        )
        || label.session_sha256 != draft.session_sha256
    {
        return invalid("review label does not bind the draft session");
    }
    let available = draft
        .canonical_frames
        .iter()
        .map(|frame| frame.sequence)
        .collect::<BTreeSet<_>>();
    let mut used = BTreeSet::new();
    let mut previous_episode_end = None;
    for episode in &label.episodes {
        if episode.episode_id.is_empty()
            || episode.expected_song_id.is_empty()
            || episode.expected_clear_type.is_empty()
            || episode.expected_result.play_side != "one_player"
            || episode.expected_result.play_mode != "single_play"
            || episode.expected_result.play_type != PlayType::Single
            || !(1..=12).contains(&episode.expected_result.level)
            || !valid_expected_result(&episode.expected_result)
            || (label.schema == LABEL_SCHEMA
                && !valid_play_options(episode.expected_result.play_options.as_deref()))
            || episode.stable_sequences.is_empty()
            || episode
                .stable_sequences
                .iter()
                .any(|sequence| !available.contains(sequence) || !used.insert(*sequence))
            || episode
                .stable_sequences
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || previous_episode_end.is_some_and(|previous| {
                episode
                    .stable_sequences
                    .first()
                    .is_some_and(|first| *first <= previous)
            })
            || episode.attempt.as_ref().is_some_and(|attempt| {
                attempt.attempt_key.is_empty()
                    || attempt.parent_attempt_key.as_deref() == Some("")
                    || !valid_span(attempt.result_span, &available)
                    || attempt
                        .select_span
                        .is_some_and(|span| !valid_span(span, &available))
                    || attempt
                        .play_span
                        .is_some_and(|span| !valid_span(span, &available))
            })
        {
            return invalid("review episode is invalid");
        }
        previous_episode_end = episode.stable_sequences.last().copied();
    }
    if label
        .negative_frames
        .iter()
        .any(|sequence| !available.contains(sequence) || !used.insert(*sequence))
    {
        return invalid("review negative frame is invalid");
    }
    Ok(())
}

fn valid_span(span: SequenceSpan, available: &BTreeSet<u64>) -> bool {
    span.first_sequence <= span.last_sequence
        && available.contains(&span.first_sequence)
        && available.contains(&span.last_sequence)
}

fn valid_play_options(options: Option<&[PlayOption]>) -> bool {
    let Some(options) = options else {
        return false;
    };
    options.len() <= PlayOption::ALL.len()
        && options.iter().copied().collect::<BTreeSet<_>>().len() == options.len()
}

fn valid_expected_result(expected: &ExpectedResult) -> bool {
    let notes = expected.notes;
    let Some(judgments) = expected.judgments.as_ref() else {
        return false;
    };
    if expected.miss_count.is_none() {
        return false;
    }
    if expected.timing.is_none() {
        return false;
    }
    if expected.combo_break.is_none() {
        return false;
    }
    let Some(previous_best) = expected.previous_best.as_ref() else {
        return false;
    };
    let previous_not_played = [
        matches!(previous_best.clear_type, PreviousBestValue::NotPlayed),
        matches!(previous_best.score, PreviousBestValue::NotPlayed),
        matches!(previous_best.miss_count, PreviousBestValue::NotPlayed),
    ];
    notes > 0
        && u64::from(expected.current_score) <= u64::from(notes) * 2
        && judgments
            .pgreat
            .checked_mul(2)
            .and_then(|value| value.checked_add(judgments.great))
            == Some(expected.current_score)
        && [
            judgments.pgreat,
            judgments.great,
            judgments.good,
            judgments.bad,
        ]
        .into_iter()
        .all(|value| value <= notes)
        && valid_previous_clear(&previous_best.clear_type)
        && valid_previous_numeric(&previous_best.score, notes.saturating_mul(2))
        && (previous_not_played.into_iter().all(|value| value)
            || previous_not_played.into_iter().all(|value| !value))
}

fn valid_previous_clear(value: &PreviousBestValue<String>) -> bool {
    match value {
        PreviousBestValue::Known { value } => resolve_clear_type(value) == Some(value.as_str()),
        PreviousBestValue::NotPlayed
        | PreviousBestValue::NotDisplayed
        | PreviousBestValue::Unknown { .. } => true,
    }
}

const fn valid_previous_numeric(value: &PreviousBestValue<u32>, maximum: u32) -> bool {
    match value {
        PreviousBestValue::Known { value } => *value <= maximum,
        PreviousBestValue::NotPlayed
        | PreviousBestValue::NotDisplayed
        | PreviousBestValue::Unknown { .. } => true,
    }
}

fn ensure_store(store: &Path) -> Result<(), CorpusError> {
    if !store.is_absolute() {
        return invalid("frame corpus store must be absolute");
    }
    for directory in ["objects", "sessions", "identities", "labels", "suites"] {
        let path = store.join(directory);
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&path)?;
    }
    Ok(())
}

fn publish_object(
    store: &Path,
    source: &Path,
    sha256: &str,
    bytes: u64,
) -> Result<(), CorpusError> {
    let destination = store.join("objects").join(sha256);
    if destination.exists() {
        return verify_file(&destination, sha256, bytes);
    }
    let staging = store.join("objects").join(format!(".{sha256}.staging"));
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    verify_file(&staging, sha256, bytes)?;
    fs::rename(&staging, &destination)?;
    File::open(store.join("objects"))?.sync_all()?;
    Ok(())
}

fn publish_document(path: &Path, bytes: &[u8]) -> Result<(), CorpusError> {
    if path.exists() {
        let existing = fs::read(path)?;
        return (existing == bytes)
            .then_some(())
            .ok_or(CorpusError::FixtureConflict);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    File::open(
        path.parent()
            .ok_or_else(|| CorpusError::InvalidRequest("document path has no parent".to_owned()))?,
    )?
    .sync_all()?;
    Ok(())
}

fn publish_active(store: &Path, generation_sha256: &str) -> Result<(), CorpusError> {
    let path = store.join("active-suite.json");
    let staging = store.join(".active-suite.staging");
    let bytes = canonical_json(&ActiveSuite {
        schema: ACTIVE_SCHEMA.to_owned(),
        generation_sha256: generation_sha256.to_owned(),
    })?;
    if staging.exists() {
        fs::remove_file(&staging)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(staging, path)?;
    File::open(store)?.sync_all()?;
    Ok(())
}

fn load_active_suite(store: &Path) -> Result<Option<(String, RegressionSuite)>, CorpusError> {
    let active_path = store.join("active-suite.json");
    if !active_path.exists() {
        return Ok(None);
    }
    let (active, _) = read_json::<ActiveSuite>(&active_path)?;
    if active.schema != ACTIVE_SCHEMA || !valid_sha256(&active.generation_sha256) {
        return invalid("active suite pointer is invalid");
    }
    let (suite, bytes) = read_json::<RegressionSuite>(
        &store
            .join("suites")
            .join(format!("{}.json", active.generation_sha256)),
    )?;
    if suite.schema != SUITE_SCHEMA || digest(&bytes) != active.generation_sha256 {
        return invalid("active suite generation is invalid");
    }
    Ok(Some((active.generation_sha256, suite)))
}

fn session_frame_map(session: &CaptureSession) -> BTreeMap<u64, String> {
    session
        .canonical_frames
        .iter()
        .map(|frame| (frame.sequence, frame.artifact_sha256.clone()))
        .collect()
}

fn read_bounded_qoi(path: &Path) -> Result<Vec<u8>, CorpusError> {
    read_bounded_qoi_with_limit(path, MAX_QOI_BYTES)
}

fn read_bounded_qoi_with_limit(path: &Path, limit: u64) -> Result<Vec<u8>, CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > limit {
        return invalid("QOI artifact exceeds the canonical encoded bound");
    }
    Ok(fs::read(path)?)
}

fn read_bounded_ndjson_line(
    reader: &mut BufReader<File>,
    line: &mut Vec<u8>,
) -> Result<bool, CorpusError> {
    line.clear();
    let read = reader
        .take(u64::try_from(MAX_NDJSON_RECORD_BYTES).unwrap_or(u64::MAX) + 1)
        .read_until(b'\n', line)?;
    if read == 0 {
        return Ok(false);
    }
    if read > MAX_NDJSON_RECORD_BYTES || line.last() != Some(&b'\n') {
        return invalid("diagnostic NDJSON record exceeds its byte bound");
    }
    Ok(true)
}

fn read_canonical_object(store: &Path, sha256: &str) -> Result<Vec<u8>, CorpusError> {
    let bytes = read_bounded_qoi(&store.join("objects").join(sha256))?;
    let header = qoi::decode_header(&bytes)
        .map_err(|_| CorpusError::InvalidReplay("canonical QOI header is invalid".to_owned()))?;
    if header.width != 1_920 || header.height != 1_080 {
        return invalid_replay("canonical QOI contract differs");
    }
    let (header, pixels) = qoi::decode_to_vec(&bytes)
        .map_err(|_| CorpusError::InvalidReplay("canonical QOI decoding failed".to_owned()))?;
    if header.width != 1_920 || header.height != 1_080 || pixels.len() != 1_920 * 1_080 * 3 {
        return invalid_replay("canonical QOI contract differs");
    }
    Ok(pixels)
}

fn verify_canonical_qoi(path: &Path) -> Result<(), CorpusError> {
    let bytes = read_bounded_qoi(path)?;
    let header = qoi::decode_header(&bytes)
        .map_err(|_| CorpusError::InvalidRequest("diagnostic QOI header is invalid".to_owned()))?;
    if header.width != 1_920 || header.height != 1_080 {
        return invalid("diagnostic canonical QOI contract differs");
    }
    let (header, pixels) = qoi::decode_to_vec(&bytes)
        .map_err(|_| CorpusError::InvalidRequest("diagnostic QOI is invalid".to_owned()))?;
    if header.width != 1_920 || header.height != 1_080 || pixels.len() != 1_920 * 1_080 * 3 {
        return invalid("diagnostic canonical QOI contract differs");
    }
    Ok(())
}

fn verify_ndjson(path: &Path) -> Result<u64, CorpusError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut count = 0_u64;
    while read_bounded_ndjson_line(&mut reader, &mut line)? {
        if serde_json::from_slice::<Value>(&line).is_err() {
            return invalid("diagnostic NDJSON is invalid");
        }
        count = count.saturating_add(1);
        if count > MAX_NDJSON_RECORDS as u64 {
            return invalid("diagnostic NDJSON record capacity exceeded");
        }
    }
    Ok(count)
}

fn verify_tick_ndjson(
    path: &Path,
    require_strict_order: bool,
) -> Result<(u64, Option<u64>, Option<u64>), CorpusError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut count = 0_u64;
    let mut first = None;
    let mut last = None;
    while read_bounded_ndjson_line(&mut reader, &mut line)? {
        let record: Value = serde_json::from_slice(&line)
            .map_err(|_| CorpusError::InvalidRequest("diagnostic NDJSON is invalid".to_owned()))?;
        let tick = record["tick_sequence"]
            .as_u64()
            .or_else(|| record["fact"]["tick_sequence"].as_u64())
            .ok_or_else(|| {
                CorpusError::InvalidRequest("diagnostic NDJSON lacks a tick sequence".to_owned())
            })?;
        if require_strict_order && last.is_some_and(|previous| tick <= previous) {
            return invalid("diagnostic NDJSON tick ordering is invalid");
        }
        first.get_or_insert(tick);
        last = Some(tick);
        count = count.saturating_add(1);
        if count > MAX_NDJSON_RECORDS as u64 {
            return invalid("diagnostic NDJSON record capacity exceeded");
        }
    }
    Ok((count, first, last))
}

fn verify_session_events(path: &Path, manifest: &DiagnosticManifest) -> Result<u64, CorpusError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut count = 0_usize;
    let mut first_event = None;
    let mut last_event = None;
    let mut event_schema = None;
    let mut last_channel_sequence = None;
    while read_bounded_ndjson_line(&mut reader, &mut line)? {
        let record = serde_json::from_slice::<Value>(&line).map_err(|_| {
            CorpusError::InvalidRequest("diagnostic event NDJSON is invalid".to_owned())
        })?;
        if record["session_id"].as_str() != Some(manifest.session_id.as_str())
            || record["capture_generation"].as_u64() != Some(manifest.capture_generation)
        {
            return invalid("diagnostic session event binding is invalid");
        }
        let schema = record["schema"].as_str().ok_or_else(|| {
            CorpusError::InvalidRequest("diagnostic event lacks its schema".to_owned())
        })?;
        if !matches!(
            schema,
            "scorepeek-private-diagnostic-event-v1"
                | "scorepeek-run-event-v2"
                | "scorepeek-run-event-v3"
                | "scorepeek-run-event-v4"
                | "scorepeek-run-event-v5"
                | "scorepeek-run-event-v6"
        ) || event_schema
            .as_deref()
            .is_some_and(|expected| expected != schema)
        {
            return invalid("diagnostic session event schema is invalid");
        }
        event_schema.get_or_insert(schema.to_owned());
        if matches!(
            schema,
            "scorepeek-run-event-v2"
                | "scorepeek-run-event-v3"
                | "scorepeek-run-event-v4"
                | "scorepeek-run-event-v5"
                | "scorepeek-run-event-v6"
        ) {
            serde_json::from_value::<StoredRunEventPayload>(record.clone()).map_err(|_| {
                CorpusError::InvalidRequest("diagnostic run event payload is invalid".to_owned())
            })?;
            let channel_sequence = record["channel_sequence"].as_u64().ok_or_else(|| {
                CorpusError::InvalidRequest(
                    "diagnostic run event lacks its channel sequence".to_owned(),
                )
            })?;
            if last_channel_sequence.is_some_and(|previous| channel_sequence <= previous) {
                return invalid("diagnostic run event ordering is invalid");
            }
            last_channel_sequence = Some(channel_sequence);
        }
        let event = record["event"]
            .as_str()
            .ok_or_else(|| {
                CorpusError::InvalidRequest("diagnostic event lacks its type".to_owned())
            })?
            .to_owned();
        first_event.get_or_insert_with(|| event.clone());
        last_event = Some(event);
        count = count.saturating_add(1);
        if count > MAX_NDJSON_RECORDS {
            return invalid("diagnostic event record capacity exceeded");
        }
    }
    if count == 0
        || first_event.as_deref() != Some("session_started")
        || (event_schema.as_deref() == Some("scorepeek-private-diagnostic-event-v1")
            && (count < 2 || last_event.as_deref() != Some("session_finished")))
    {
        return invalid("diagnostic session event ordering or binding is invalid");
    }
    Ok(count as u64)
}

fn manifest_artifact<'a>(
    manifest: &'a DiagnosticManifest,
    path: &str,
) -> Result<&'a DiagnosticArtifact, CorpusError> {
    manifest
        .artifacts
        .iter()
        .find(|artifact| artifact.path == path)
        .ok_or_else(|| CorpusError::InvalidRequest(format!("diagnostic is missing {path}")))
}

fn safe_relative(value: &str) -> Result<PathBuf, CorpusError> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return invalid("diagnostic artifact path is invalid");
    }
    Ok(path.to_owned())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<(T, Vec<u8>), CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_DOCUMENT_BYTES {
        return invalid("document size is invalid");
    }
    let bytes = fs::read(path)?;
    let value = serde_json::from_slice(&bytes)?;
    Ok((value, bytes))
}

fn read_regression_label(path: &Path) -> Result<(RegressionLabel, Vec<u8>), CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_DOCUMENT_BYTES {
        return invalid("document size is invalid");
    }
    let bytes = fs::read(path)?;
    let value = serde_json::from_slice::<Value>(&bytes)?;
    match value.get("schema").and_then(Value::as_str) {
        Some(LABEL_SCHEMA | PREVIOUS_LABEL_SCHEMA | PREVIOUS_LABEL_SCHEMA_V3) => {
            Ok((serde_json::from_value(value)?, bytes))
        }
        Some(LEGACY_LABEL_SCHEMA) => {
            let legacy = serde_json::from_value::<LegacyRegressionLabel>(value)?;
            if legacy
                .episodes
                .iter()
                .any(|episode| !episode.expected_result.savable)
            {
                return invalid("legacy regression label contains an unsavable result");
            }
            let label = RegressionLabel {
                schema: legacy.schema,
                session_sha256: legacy.session_sha256,
                disposition: legacy.disposition,
                episodes: legacy
                    .episodes
                    .into_iter()
                    .map(|episode| RegressionEpisode {
                        episode_id: episode.episode_id,
                        expected_song_id: episode.expected_song_id,
                        expected_clear_type: episode.expected_clear_type,
                        expected_result: ExpectedResult {
                            play_side: episode.expected_result.play_side,
                            play_mode: episode.expected_result.play_mode,
                            play_type: episode.expected_result.play_type,
                            difficulty: episode.expected_result.difficulty,
                            level: episode.expected_result.level,
                            notes: episode.expected_result.notes,
                            current_score: episode.expected_result.current_score,
                            judgments: None,
                            miss_count: None,
                            timing: None,
                            combo_break: None,
                            previous_best: None,
                            play_options: None,
                        },
                        stable_sequences: episode.stable_sequences,
                        attempt: None,
                    })
                    .collect(),
                negative_frames: legacy.negative_frames,
            };
            Ok((label, bytes))
        }
        _ => invalid("regression label schema is unsupported"),
    }
}

fn verify_file(path: &Path, expected_sha256: &str, expected_bytes: u64) -> Result<(), CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_file()
        || metadata.len() != expected_bytes
        || digest_file(path)? != expected_sha256
    {
        return invalid("artifact content differs from its reference");
    }
    Ok(())
}

fn digest_file(path: &Path) -> Result<String, CorpusError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(hasher.finalize()))
}

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, CorpusError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn digest(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes))
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    bytes.as_ref().iter().fold(
        String::with_capacity(bytes.as_ref().len().saturating_mul(2)),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
            output
        },
    )
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn invalid<T>(detail: &str) -> Result<T, CorpusError> {
    Err(CorpusError::InvalidRequest(detail.to_owned()))
}

fn invalid_replay<T>(detail: &str) -> Result<T, CorpusError> {
    Err(CorpusError::InvalidReplay(detail.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic_manifest() -> DiagnosticManifest {
        DiagnosticManifest {
            schema: DIAGNOSTIC_SCHEMA.to_owned(),
            source_kind: SourceKind::LiveRun,
            session_id: "run-1-session-1".to_owned(),
            capture_generation: 1,
            profile_sha256: "1".repeat(64),
            catalog_sha256: "2".repeat(64),
            recognition_interval_ms: 100,
            processed_ticks: 1,
            busy_skips: 0,
            maximum_consecutive_busy_skips: 0,
            field_observation_busy_skips: Some(0),
            maximum_consecutive_field_observation_busy_skips: Some(0),
            completeness: "complete".to_owned(),
            capture_manifest_sha256: "3".repeat(64),
            recognition_manifest_sha256: "4".repeat(64),
            event_manifest_sha256: "5".repeat(64),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn diagnostic_manifest_requires_v4_field_busy_summary_and_preserves_v3() {
        let mut manifest = diagnostic_manifest();
        manifest.artifacts.push(DiagnosticArtifact {
            kind: "capture_manifest".to_owned(),
            path: "capture/manifest.json".to_owned(),
            sha256: "6".repeat(64),
            bytes: 1,
        });
        manifest.field_observation_busy_skips = Some(17);
        manifest.maximum_consecutive_field_observation_busy_skips = Some(3);
        assert!(validate_diagnostic_manifest(&manifest).is_ok());

        manifest.maximum_consecutive_field_observation_busy_skips = Some(18);
        assert!(validate_diagnostic_manifest(&manifest).is_err());

        manifest.schema = LEGACY_DIAGNOSTIC_SCHEMA.to_owned();
        manifest.field_observation_busy_skips = None;
        manifest.maximum_consecutive_field_observation_busy_skips = None;
        assert!(validate_diagnostic_manifest(&manifest).is_ok());
    }

    #[test]
    fn decimal_video_timestamps_are_converted_without_float_rounding() {
        assert_eq!(parse_timestamp_ms("0.000000").unwrap(), 0);
        assert_eq!(parse_timestamp_ms("12.345678").unwrap(), 12_345);
        assert_eq!(parse_timestamp_ms("1.5").unwrap(), 1_500);
        assert!(parse_timestamp_ms("-0.1").is_err());
    }

    #[test]
    fn fact_stream_allows_async_completion_order_but_observations_require_order() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("records.ndjson");
        fs::write(
            &path,
            b"{\"tick_sequence\":1}\n{\"tick_sequence\":1}\n{\"tick_sequence\":2}\n",
        )
        .unwrap();
        assert_eq!(
            verify_tick_ndjson(&path, false).unwrap(),
            (3, Some(1), Some(2))
        );
        assert!(verify_tick_ndjson(&path, true).is_err());
        fs::write(&path, b"{\"tick_sequence\":2}\n{\"tick_sequence\":1}\n").unwrap();
        assert_eq!(
            verify_tick_ndjson(&path, false).unwrap(),
            (2, Some(2), Some(1))
        );
        assert!(verify_tick_ndjson(&path, true).is_err());

        fs::write(
            &path,
            b"{\"fact\":{\"tick_sequence\":1}}\n{\"fact\":{\"tick_sequence\":2}}\n",
        )
        .unwrap();
        assert_eq!(
            verify_tick_ndjson(&path, false).unwrap(),
            (2, Some(1), Some(2))
        );
    }

    #[test]
    fn ndjson_reader_rejects_an_oversized_record_before_parsing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("records.ndjson");
        fs::write(&path, vec![b' '; MAX_NDJSON_RECORD_BYTES + 1]).unwrap();
        assert!(verify_ndjson(&path).is_err());
    }

    #[test]
    fn run_event_stream_requires_a_bound_start_and_increasing_channel_order() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.ndjson");
        let manifest = diagnostic_manifest();
        fs::write(
            &path,
            concat!(
                "{\"schema\":\"scorepeek-run-event-v2\",\"event\":\"session_started\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"capture_profile_sha256\":\"profile\",\"normalizer_artifact_sha256\":\"normalizer\",\"channel_sequence\":2}\n",
                "{\"schema\":\"scorepeek-run-event-v2\",\"event\":\"screen_changed\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"sequence\":0,\"monotonic_start_ms\":0,\"monotonic_end_ms\":1,\"screen\":\"unknown\",\"channel_sequence\":3}\n",
                "{\"schema\":\"scorepeek-run-event-v2\",\"event\":\"numeric_result_changed\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"source_sequence\":2,\"state\":{\"observations\":1,\"status\":\"pending\"},\"reason\":\"candidate_started\",\"event_suppression_reason\":\"numeric_not_accepted\",\"channel_sequence\":4}\n",
            ),
        )
        .unwrap();
        assert_eq!(verify_session_events(&path, &manifest).unwrap(), 3);

        fs::write(
            &path,
            concat!(
                "{\"schema\":\"scorepeek-run-event-v2\",\"event\":\"session_started\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"capture_profile_sha256\":\"profile\",\"normalizer_artifact_sha256\":\"normalizer\",\"channel_sequence\":3}\n",
                "{\"schema\":\"scorepeek-run-event-v2\",\"event\":\"screen_changed\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"sequence\":0,\"monotonic_start_ms\":0,\"monotonic_end_ms\":1,\"screen\":\"unknown\",\"channel_sequence\":2}\n",
            ),
        )
        .unwrap();
        assert!(verify_session_events(&path, &manifest).is_err());

        fs::write(
            &path,
            concat!(
                "{\"schema\":\"scorepeek-run-event-v2\",\"event\":\"session_started\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"capture_profile_sha256\":\"profile\",\"normalizer_artifact_sha256\":\"normalizer\",\"channel_sequence\":2}\n",
                "{\"schema\":\"scorepeek-run-event-v2\",\"event\":\"screen_changed\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"channel_sequence\":3}\n",
            ),
        )
        .unwrap();
        assert!(verify_session_events(&path, &manifest).is_err());
    }

    #[test]
    fn v5_run_event_stream_accepts_typed_selection_difficulty_transitions() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.ndjson");
        let manifest = diagnostic_manifest();
        fs::write(
            &path,
            concat!(
                "{\"schema\":\"scorepeek-run-event-v5\",\"event\":\"session_started\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"capture_profile_sha256\":\"profile\",\"normalizer_artifact_sha256\":\"normalizer\",\"channel_sequence\":1}\n",
                "{\"schema\":\"scorepeek-run-event-v5\",\"event\":\"selection_difficulty_changed\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"screen_episode_id\":22,\"source_sequence\":3255,\"target\":\"incumbent\",\"reason\":\"changed\",\"current\":{\"difficulty\":\"another\",\"consecutive_known\":1,\"first_sequence\":3255,\"last_sequence\":3255,\"first_monotonic_ms\":325500,\"last_monotonic_ms\":325500},\"channel_sequence\":2}\n",
                "{\"schema\":\"scorepeek-run-event-v5\",\"event\":\"session_finished\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"outcome\":\"ok\",\"report\":{},\"channel_sequence\":3}\n",
            ),
        )
        .unwrap();
        assert_eq!(verify_session_events(&path, &manifest).unwrap(), 3);
    }

    #[test]
    fn v6_run_event_stream_is_current_and_v5_remains_readable() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.ndjson");
        let manifest = diagnostic_manifest();
        fs::write(
            &path,
            concat!(
                "{\"schema\":\"scorepeek-run-event-v6\",\"event\":\"session_started\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"capture_profile_sha256\":\"profile\",\"normalizer_artifact_sha256\":\"normalizer\",\"channel_sequence\":1}\n",
                "{\"schema\":\"scorepeek-run-event-v6\",\"event\":\"session_finished\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"outcome\":\"ok\",\"report\":{},\"channel_sequence\":2}\n",
            ),
        )
        .unwrap();
        assert_eq!(verify_session_events(&path, &manifest).unwrap(), 2);
    }

    #[test]
    fn legacy_diagnostic_event_stream_still_requires_a_finished_session() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.ndjson");
        let manifest = diagnostic_manifest();
        let started = "{\"schema\":\"scorepeek-private-diagnostic-event-v1\",\"event\":\"session_started\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1}\n";
        fs::write(&path, started).unwrap();
        assert!(verify_session_events(&path, &manifest).is_err());
        fs::write(
            &path,
            format!(
                "{started}{{\"schema\":\"scorepeek-private-diagnostic-event-v1\",\"event\":\"session_finished\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1}}\n"
            ),
        )
        .unwrap();
        assert_eq!(verify_session_events(&path, &manifest).unwrap(), 2);
    }

    fn expected_result() -> ExpectedResult {
        ExpectedResult {
            play_side: "one_player".to_owned(),
            play_mode: "single_play".to_owned(),
            play_type: PlayType::Single,
            difficulty: Difficulty::Hyper,
            level: 8,
            notes: 100,
            current_score: 150,
            judgments: Some(ResultJudgments {
                pgreat: 70,
                great: 10,
                good: 5,
                bad: 3,
                poor: 2,
            }),
            miss_count: Some(SupplementalResultValue::Known { value: 2 }),
            timing: Some(ResultTiming {
                fast: SupplementalResultValue::Known { value: 4 },
                slow: SupplementalResultValue::Known { value: 5 },
            }),
            combo_break: Some(SupplementalResultValue::Known { value: 1 }),
            previous_best: Some(PreviousBest {
                clear_type: PreviousBestValue::Known {
                    value: "CLEAR".to_owned(),
                },
                score: PreviousBestValue::Known { value: 140 },
                miss_count: PreviousBestValue::Known { value: 3 },
            }),
            play_options: Some(vec![PlayOption::Random, PlayOption::Legacy]),
        }
    }

    #[test]
    fn numeric_dataset_selects_only_the_stable_result_episode() {
        let screen_sequences = [
            (8, true),
            (18, true),
            (30, true),
            (41, false),
            (50, true),
            (61, true),
        ];
        assert_eq!(
            numeric_episode_sequences(&screen_sequences, &[18], 32).unwrap(),
            vec![18, 8, 30]
        );
        assert!(numeric_episode_sequences(&screen_sequences, &[41], 32).is_err());
    }

    #[test]
    fn numeric_dataset_caps_frames_nearest_to_stable_evidence() {
        let screen_sequences = (1..=10)
            .map(|sequence| (sequence, true))
            .collect::<Vec<_>>();
        assert_eq!(
            numeric_episode_sequences(&screen_sequences, &[6], 4).unwrap(),
            vec![6, 5, 7, 4]
        );
    }

    #[test]
    fn numeric_dataset_collects_visible_level_and_notes_truth() {
        let labels = numeric_field_labels(&expected_result()).unwrap();
        assert_eq!(
            labels.get(&NumericField::Level).map(String::as_str),
            Some("8")
        );
        assert_eq!(
            labels.get(&NumericField::Notes).map(String::as_str),
            Some("0100")
        );
        assert_eq!(labels.len(), 14);
        assert!(numeric_field_uses_sequence(NumericField::Level, 20, &[20]));
        assert!(!numeric_field_uses_sequence(NumericField::Notes, 19, &[20]));
        assert!(numeric_field_uses_sequence(NumericField::Good, 19, &[20]));
    }

    #[test]
    fn optional_numeric_unknown_is_safe_but_wrong_known_is_not() {
        let expected = SupplementalResultValue::Known { value: 7_u32 };
        assert!(optional_supplemental_matches(
            &SupplementalResultValue::Unknown {
                reason: scorepeek::recognition::ResultFieldUnknownReason::Empty,
            },
            &expected,
        ));
        assert!(!optional_supplemental_matches(
            &SupplementalResultValue::Known { value: 8 },
            &expected,
        ));
        assert!(optional_previous_matches(
            &PreviousBestValue::Unknown {
                reason: scorepeek::recognition::ResultFieldUnknownReason::Empty,
            },
            &PreviousBestValue::Known { value: 7_u32 },
        ));
    }

    #[test]
    fn v5_result_validation_rejects_unreplayable_typed_values() {
        assert!(valid_expected_result(&expected_result()));

        let mut unbounded = expected_result();
        unbounded.miss_count = Some(SupplementalResultValue::Known { value: 101 });
        unbounded.timing = Some(ResultTiming {
            fast: SupplementalResultValue::Known { value: 102 },
            slow: SupplementalResultValue::Known { value: 103 },
        });
        unbounded.combo_break = Some(SupplementalResultValue::Known { value: 104 });
        unbounded.judgments.as_mut().unwrap().poor = 105;
        unbounded.previous_best.as_mut().unwrap().miss_count =
            PreviousBestValue::Known { value: 106 };
        assert!(valid_expected_result(&unbounded));

        let mut note_judgment_overflow = expected_result();
        note_judgment_overflow.judgments.as_mut().unwrap().bad = 101;
        assert!(!valid_expected_result(&note_judgment_overflow));

        let mut inconsistent_no_play = expected_result();
        inconsistent_no_play
            .previous_best
            .as_mut()
            .unwrap()
            .clear_type = PreviousBestValue::NotPlayed;
        assert!(!valid_expected_result(&inconsistent_no_play));

        let mut invalid_clear = expected_result();
        invalid_clear.previous_best.as_mut().unwrap().clear_type = PreviousBestValue::Known {
            value: "CLEER".to_owned(),
        };
        assert!(!valid_expected_result(&invalid_clear));

        let mut score_overflow = expected_result();
        score_overflow.notes = u32::MAX;
        score_overflow.current_score = u32::MAX;
        score_overflow.judgments = Some(ResultJudgments {
            pgreat: u32::MAX,
            great: 1,
            good: 0,
            bad: 0,
            poor: 0,
        });
        assert!(!valid_expected_result(&score_overflow));
    }

    #[test]
    fn v5_play_options_require_an_ordered_distinct_list() {
        assert!(!valid_play_options(None));
        assert!(valid_play_options(Some(&[])));
        assert!(valid_play_options(Some(&[
            PlayOption::Random,
            PlayOption::Legacy,
        ])));
        assert!(!valid_play_options(Some(&[
            PlayOption::Random,
            PlayOption::Random,
        ])));
    }

    #[test]
    fn v5_replay_requires_exact_known_play_options_in_display_order() {
        let expected = [PlayOption::Random, PlayOption::Legacy];
        assert!(expected_play_options_match(
            &PlayOptions::Known {
                values: expected.to_vec(),
            },
            Some(&expected),
        ));
        assert!(!expected_play_options_match(
            &PlayOptions::Known {
                values: vec![PlayOption::Legacy, PlayOption::Random],
            },
            Some(&expected),
        ));
        assert!(!expected_play_options_match(
            &PlayOptions::Unknown {
                reason: scorepeek::recognition::PlayOptionsUnknownReason::Unrecognized,
            },
            Some(&expected),
        ));
    }

    #[test]
    fn legacy_replay_ignores_play_options_without_truth() {
        assert!(expected_play_options_match(
            &PlayOptions::Unknown {
                reason: scorepeek::recognition::PlayOptionsUnknownReason::NotObserved,
            },
            None,
        ));
    }

    #[test]
    fn v4_labels_remain_readable_without_play_options() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("label.json");
        let mut value = serde_json::to_value(RegressionLabel {
            schema: PREVIOUS_LABEL_SCHEMA.to_owned(),
            session_sha256: "1".repeat(64),
            disposition: LabelDisposition::Include,
            episodes: vec![RegressionEpisode {
                episode_id: "episode-1".to_owned(),
                expected_song_id: "song-1".to_owned(),
                expected_clear_type: "CLEAR".to_owned(),
                expected_result: expected_result(),
                stable_sequences: vec![1],
                attempt: None,
            }],
            negative_frames: Vec::new(),
        })
        .unwrap();
        value["episodes"][0]["expected_result"]
            .as_object_mut()
            .unwrap()
            .remove("play_options");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let (label, _) = read_regression_label(&path).unwrap();
        assert!(label.episodes[0].expected_result.play_options.is_none());
    }

    #[test]
    fn invalid_v5_result_does_not_publish_an_active_suite() {
        let temporary = tempfile::tempdir().unwrap();
        let store = temporary.path().join("store");
        let draft_path = temporary.path().join("draft.json");
        let label_path = temporary.path().join("label.json");
        let session_sha256 = "1".repeat(64);
        let draft = ReviewDraft {
            schema: DRAFT_SCHEMA.to_owned(),
            session_sha256: session_sha256.clone(),
            diagnostic_sha256: "2".repeat(64),
            source_session_id: "session".to_owned(),
            canonical_frames: vec![ReviewFrame {
                sequence: 1,
                artifact_sha256: "3".repeat(64),
            }],
            observation_count: 1,
            completeness: "complete".to_owned(),
        };
        let mut invalid_result = expected_result();
        invalid_result.judgments.as_mut().unwrap().bad = 101;
        let label = RegressionLabel {
            schema: LABEL_SCHEMA.to_owned(),
            session_sha256,
            disposition: LabelDisposition::Include,
            episodes: vec![RegressionEpisode {
                episode_id: "episode-1".to_owned(),
                expected_song_id: "song-1".to_owned(),
                expected_clear_type: "CLEAR".to_owned(),
                expected_result: invalid_result,
                stable_sequences: vec![1],
                attempt: None,
            }],
            negative_frames: Vec::new(),
        };
        fs::write(&draft_path, canonical_json(&draft).unwrap()).unwrap();
        fs::write(&label_path, canonical_json(&label).unwrap()).unwrap();

        assert!(apply_review(&store, &draft_path, &label_path).is_err());
        assert!(!store.join("active-suite.json").exists());
    }

    #[test]
    fn partial_review_accepts_only_explicitly_retained_negative_frames() {
        let digest = "1".repeat(64);
        let draft = ReviewDraft {
            schema: DRAFT_SCHEMA.to_owned(),
            session_sha256: digest.clone(),
            diagnostic_sha256: "2".repeat(64),
            source_session_id: "session".to_owned(),
            canonical_frames: vec![ReviewFrame {
                sequence: 1,
                artifact_sha256: "3".repeat(64),
            }],
            observation_count: 0,
            completeness: "partial".to_owned(),
        };
        let label = RegressionLabel {
            schema: LABEL_SCHEMA.to_owned(),
            session_sha256: digest,
            disposition: LabelDisposition::Include,
            episodes: Vec::new(),
            negative_frames: vec![1],
        };
        assert!(validate_label(&draft, &label).is_ok());
        let missing = RegressionLabel {
            negative_frames: vec![2],
            ..label
        };
        assert!(validate_label(&draft, &missing).is_err());
    }
}
