#![allow(clippy::missing_errors_doc, clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::ops::{Deref, DerefMut};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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
use crate::segment_remote::{RemoteSegment, SegmentRemote};

const DIAGNOSTIC_SCHEMA: &str = "scorepeek-private-diagnostic-session-v5";
const SESSION_SCHEMA: &str = "scorepeek-private-capture-session-v2";
const DRAFT_SCHEMA: &str = "scorepeek-private-session-review-draft-v2";
const LABEL_SCHEMA: &str = "scorepeek-private-session-regression-label-v5";
const SUITE_SCHEMA: &str = "scorepeek-private-regression-suite-v1";
const ACTIVE_SCHEMA: &str = "scorepeek-private-regression-suite-active-v1";
const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ARTIFACTS: usize = 20_000;
const MAX_NDJSON_RECORDS: usize = 250_000;
const MAX_NDJSON_RECORD_BYTES: usize = 1024 * 1024;
const MAX_EVIDENCE_FRAMES: usize = 1_024;
const MAX_EVIDENCE_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_QOI_BYTES: u64 = 16 * 1024 * 1024;
const CANONICAL_DECODE_TIMEOUT: Duration = Duration::from_mins(2);
const CANONICAL_DECODE_STDERR_BYTES: usize = 64 * 1024;
const DEFAULT_REPLAY_MEMORY_MIB: usize = 2_048;
const MINIMUM_REPLAY_MEMORY_MIB: usize = 256;
const MAXIMUM_REPLAY_MEMORY_MIB: usize = 8_192;
const DECODER_RESERVATION_BYTES: usize = 16 * 1024 * 1024;
const SESSION_STATE_RESERVATION_BYTES: usize = 64 * 1024 * 1024;
const PENDING_FIELD_FRAME_RESERVATION_BYTES: usize = 16 * 1024 * 1024;
const REPLAY_SEGMENT_PREFETCH: usize = 4;
const NUMERIC_DATASET_SCHEMA: &str = "scorepeek-private-numeric-ctc-dataset-v1";

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

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
    #[serde(default)]
    canonical_manifest_sha256: Option<String>,
    #[serde(default)]
    canonical_completeness: Option<String>,
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

#[derive(Clone, Debug, Deserialize)]
struct CanonicalRecordingManifest {
    schema: String,
    completeness: String,
    ffmpeg_sha256: String,
    ffmpeg_version: String,
    tick_index_sha256: String,
    tick_count: usize,
    segments: Vec<CanonicalSegment>,
    dropped_frames: u64,
    completeness_reasons: Vec<String>,
    memory_limit_bytes: u64,
    memory_high_water_bytes: u64,
    integrity_verification: String,
}

#[derive(Clone, Debug, Deserialize)]
struct CanonicalSegment {
    path: String,
    first_sequence: u64,
    last_sequence: u64,
    frames: usize,
    raw_rgb24_sha256: String,
    encoded_sha256: String,
    bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct CanonicalTick {
    sequence: u64,
    #[allow(dead_code)]
    source_sequence: u64,
    monotonic_ms: u64,
    screen: ScreenClass,
    semantic_episode_id: Option<u64>,
    disposition: String,
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
    RecordingHealthChanged {
        session_id: Option<String>,
        capture_generation: Option<u64>,
        state: String,
        memory_limit_bytes: u64,
        memory_used_bytes: u64,
        memory_high_water_bytes: u64,
        dropped_frames: u64,
    },
    RecordingFinalizing {
        session_id: Option<String>,
        capture_generation: Option<u64>,
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
        song: Option<Value>,
        result: Value,
    },
    ResultProvisionalChanged {
        session_id: String,
        capture_generation: u64,
        screen_episode_id: u64,
        source_sequence: u64,
        revision: u64,
        state: Value,
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
    decide_span: Option<SequenceSpan>,
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

struct NumericEpisodePlan<'a> {
    episode: &'a RegressionEpisode,
    field_labels: BTreeMap<NumericField, String>,
    requested: BTreeSet<u64>,
    observed: BTreeSet<u64>,
    crops: BTreeSet<(NumericField, String)>,
    field_counts: BTreeMap<NumericField, usize>,
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
    local_segment_objects: u64,
    remote_segment_objects: u64,
    remote_transferred_objects: u64,
    remote_reused_objects: u64,
    remote_segment_bytes: u64,
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
    text_workers: usize,
    preprocess_workers: usize,
    decode_workers: usize,
    maximum_active_sessions: usize,
    maximum_concurrent_decoders: usize,
    decoder_children: usize,
    maximum_blocked_sessions: usize,
    completed_sessions: usize,
    memory_limit_bytes: u64,
    tracked_memory_peak_bytes: u64,
    process_rss_peak_bytes: u64,
    ffmpeg_rss_peak_total_bytes: u64,
    decoder_details: Vec<CorpusReplayDecoderSummary>,
    decode_consumer_wait_us: u64,
    preprocess_queue_wait_us: u64,
    preprocess_wall_us: u64,
    screen_classification_us: u64,
    crop_prepare_us: u64,
    field_queue_wait_us: u64,
    text_batch_wall_us: u64,
    maximum_text_worker_inference_us: u64,
    text_worker_busy_us: u64,
    numeric_inference_us: u64,
    field_join_us: u64,
    catalog_projection_us: u64,
    field_frame_wall_us: u64,
    ordered_commit_wait_us: u64,
    decoder_slot_wait_us: u64,
    memory_wait_us: u64,
    sessions: Vec<CorpusReplaySessionSummary>,
    corpus_wall_us: u64,
    local_segment_decodes: u64,
    remote_segment_downloads: u64,
    remote_downloaded_bytes: u64,
}

#[derive(Debug, Serialize)]
struct CorpusReplaySessionSummary {
    session_key: String,
    wall_us: u64,
    canonical_frames: usize,
}

#[derive(Clone, Debug, Serialize)]
struct CorpusReplayDecoderSummary {
    decoder_id: usize,
    wall_us: u64,
    rss_peak_bytes: u64,
}

#[derive(Clone)]
struct SegmentResolver {
    remote: Option<SegmentRemote>,
    local_segment_decodes: Arc<AtomicU64>,
}

enum ResolvedSegment {
    Local(PathBuf),
    Remote(RemoteSegment),
}

struct PrefetchedReplaySegment {
    segment_index: usize,
    handle: Option<JoinHandle<Result<ResolvedSegment, CorpusError>>>,
}

impl PrefetchedReplaySegment {
    fn start(
        segment_index: usize,
        store: PathBuf,
        session: CaptureSession,
        source_path: String,
        resolver: SegmentResolver,
    ) -> Self {
        Self {
            segment_index,
            handle: Some(thread::spawn(move || {
                resolver.resolve(&store, &session, &source_path)
            })),
        }
    }

    fn finish(mut self) -> Result<ResolvedSegment, CorpusError> {
        self.join()
    }

    fn join(&mut self) -> Result<ResolvedSegment, CorpusError> {
        self.handle
            .take()
            .expect("prefetched segment handle is present")
            .join()
            .map_err(|_| {
                CorpusError::InvalidReplay("canonical segment prefetch panicked".to_owned())
            })?
    }
}

impl Drop for PrefetchedReplaySegment {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct CanonicalReplayEnvironment<'a> {
    bundle: &'a Path,
    catalog_root: &'a Path,
    diagnostic_root: &'a Path,
    segment_resolver: &'a SegmentResolver,
}

struct ReplayStepContext<'a> {
    store: &'a Path,
    diagnostic_root: &'a Path,
    shared: &'a Arc<
        scorepeek::recognition_live::screen_field_observer::SharedRegisteredScreenFieldResources,
    >,
    decode_activity: &'a Arc<ReplayDecodeActivity>,
    preprocess_pool: &'a ReplayPreprocessPool,
    outstanding_limit: usize,
    segment_resolver: &'a SegmentResolver,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CorpusReplayOptions {
    pub text_workers: Option<usize>,
    pub memory_mib: usize,
}

impl Default for CorpusReplayOptions {
    fn default() -> Self {
        Self {
            text_workers: None,
            memory_mib: DEFAULT_REPLAY_MEMORY_MIB,
        }
    }
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
        canonical_manifest_sha256: None,
        canonical_completeness: None,
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
        canonical_manifest_sha256: None,
        canonical_completeness: None,
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
    if manifest.schema == DIAGNOSTIC_SCHEMA {
        return verify_canonical_diagnostic(path, manifest, &bytes);
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

fn verify_canonical_diagnostic(
    path: &Path,
    manifest: DiagnosticManifest,
    manifest_bytes: &[u8],
) -> Result<DiagnosticVerificationSummary, CorpusError> {
    let canonical_artifact = manifest_artifact(&manifest, "recognition/canonical-manifest.json")?;
    if Some(canonical_artifact.sha256.as_str()) != manifest.canonical_manifest_sha256.as_deref() {
        return invalid("canonical manifest binding differs");
    }
    let (canonical, _) =
        read_json::<CanonicalRecordingManifest>(&path.join("recognition/canonical-manifest.json"))?;
    if canonical.schema != "scorepeek-canonical-session-recording-v2"
        || canonical.completeness
            != manifest
                .canonical_completeness
                .as_deref()
                .unwrap_or_default()
        || !valid_sha256(&canonical.ffmpeg_sha256)
        || canonical.ffmpeg_version.is_empty()
        || !valid_sha256(&canonical.tick_index_sha256)
        || canonical.segments.len() > MAX_ARTIFACTS
        || !(128 * 1024 * 1024..=16 * 1024 * 1024 * 1024).contains(&canonical.memory_limit_bytes)
        || canonical.memory_high_water_bytes > canonical.memory_limit_bytes
        || canonical.integrity_verification != "deferred_to_import"
        || (canonical.completeness == "complete"
            && (canonical.dropped_frames != 0 || !canonical.completeness_reasons.is_empty()))
    {
        return invalid("canonical recording manifest is invalid");
    }
    let tick_artifact = manifest_artifact(&manifest, "recognition/canonical-ticks.ndjson")?;
    let tick_path = path.join("recognition/canonical-ticks.ndjson");
    if tick_artifact.sha256 != canonical.tick_index_sha256
        || verify_file(
            &tick_path,
            &canonical.tick_index_sha256,
            tick_artifact.bytes,
        )
        .is_err()
    {
        return invalid("canonical tick index binding differs");
    }
    let ticks = read_canonical_ticks(&tick_path)?;
    if ticks.len() != canonical.tick_count || ticks.is_empty() {
        return invalid("canonical tick index count differs");
    }
    let mut retained = Vec::new();
    let mut previous = None;
    for tick in &ticks {
        if !canonical_tick_follows(previous, tick) {
            return invalid("canonical tick chronology is invalid");
        }
        previous = Some((tick.sequence, tick.monotonic_ms));
        let must_retain = matches!(
            tick.screen,
            ScreenClass::MusicSelect | ScreenClass::DecideTransition | ScreenClass::Result
        );
        match tick.disposition.as_str() {
            "retained" => retained.push(tick.sequence),
            "play_interior" if tick.screen == ScreenClass::Play => {}
            "mode_select_interior" if tick.screen == ScreenClass::ModeSelect => {}
            "unknown_interior" if tick.screen == ScreenClass::Unknown => {}
            _ => return invalid("canonical tick disposition is invalid"),
        }
        if must_retain && tick.disposition != "retained" {
            return invalid("required semantic evidence was elided");
        }
        let _ = tick.semantic_episode_id;
    }
    let mut segment_frames = 0usize;
    let mut retained_offset = 0usize;
    for segment in &canonical.segments {
        if segment.frames == 0
            || segment.frames > 600
            || segment.last_sequence < segment.first_sequence
            || !valid_sha256(&segment.raw_rgb24_sha256)
            || !valid_sha256(&segment.encoded_sha256)
            || segment.bytes == 0
            || segment.bytes > MAX_ARTIFACT_BYTES
        {
            return invalid("canonical segment reference is invalid");
        }
        safe_relative(&segment.path)?;
        let artifact = manifest_artifact(&manifest, &format!("recognition/{}", segment.path))?;
        if artifact.sha256 != segment.encoded_sha256 || artifact.bytes != segment.bytes {
            return invalid("canonical segment artifact binding differs");
        }
        let expected = retained
            .get(retained_offset..retained_offset.saturating_add(segment.frames))
            .ok_or_else(|| {
                CorpusError::InvalidRequest(
                    "canonical segment exceeds retained tick index".to_owned(),
                )
            })?;
        if expected.first() != Some(&segment.first_sequence)
            || expected.last() != Some(&segment.last_sequence)
        {
            return invalid("canonical segment chronology differs from tick index");
        }
        let (decoded_sha256, decoded_frames) = decode_canonical_segment(
            &path.join("recognition").join(&segment.path),
            segment.frames,
        )?;
        if decoded_sha256 != segment.raw_rgb24_sha256 || decoded_frames != segment.frames {
            return invalid("canonical segment lossless decode differs");
        }
        retained_offset = retained_offset.saturating_add(segment.frames);
        segment_frames = segment_frames.saturating_add(segment.frames);
    }
    if retained_offset != retained.len() || segment_frames != retained.len() {
        return invalid("canonical retained tick coverage differs");
    }
    Ok(DiagnosticVerificationSummary {
        schema: "scorepeek-private-diagnostic-verification-v2",
        diagnostic_sha256: digest(manifest_bytes),
        session_id: manifest.session_id,
        artifact_count: manifest.artifacts.len(),
        canonical_frame_count: retained.len(),
        observation_count: 0,
    })
}

fn canonical_tick_follows(previous: Option<(u64, u64)>, tick: &CanonicalTick) -> bool {
    previous.is_none_or(|(sequence, monotonic)| {
        tick.sequence > sequence && tick.monotonic_ms >= monotonic
    })
}

fn decode_canonical_segment(path: &Path, frames: usize) -> Result<(String, usize), CorpusError> {
    let digest = decode_canonical_frames(path, frames, DecodeContext::Verify, |_, _| Ok(()))?;
    Ok((digest, frames))
}

#[derive(Clone, Copy)]
enum DecodeContext {
    Verify,
    Replay,
}

enum DecodeSource<'a> {
    Path(&'a Path),
    File(File),
}

fn decode_canonical_frames(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    observe: impl FnMut(usize, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_frames_with_activity(path, expected_frames, context, None, observe)
}

fn decode_canonical_frames_with_activity(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    mut observe: impl FnMut(usize, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_frames_with_program_and_timing(
        path,
        expected_frames,
        context,
        activity,
        OsStr::new("ffmpeg"),
        move |index, pixels, _| observe(index, pixels),
    )
}

fn decode_canonical_frames_with_activity_and_timing(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    observe: impl FnMut(usize, Box<[u8]>, u64) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_frames_with_program_and_timing(
        path,
        expected_frames,
        context,
        activity,
        OsStr::new("ffmpeg"),
        observe,
    )
}

fn decode_canonical_frames_with_program(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    program: &OsStr,
    mut observe: impl FnMut(usize, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_frames_with_program_and_timing(
        path,
        expected_frames,
        context,
        activity,
        program,
        move |index, pixels, _| observe(index, pixels),
    )
}

fn decode_canonical_frames_with_program_and_timing(
    path: &Path,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    program: &OsStr,
    observe: impl FnMut(usize, Box<[u8]>, u64) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    decode_canonical_source_with_program_and_timing(
        DecodeSource::Path(path),
        expected_frames,
        context,
        activity,
        program,
        observe,
    )
}

fn decode_resolved_canonical_frames(
    source: &ResolvedSegment,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    observe: impl FnMut(usize, Box<[u8]>, u64) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    let source = match source {
        ResolvedSegment::Local(path) => DecodeSource::Path(path),
        ResolvedSegment::Remote(segment) => DecodeSource::File(segment.input()?),
    };
    decode_canonical_source_with_program_and_timing(
        source,
        expected_frames,
        context,
        activity,
        OsStr::new("ffmpeg"),
        observe,
    )
}

fn decode_canonical_source_with_program_and_timing(
    source: DecodeSource<'_>,
    expected_frames: usize,
    context: DecodeContext,
    activity: Option<&ReplayDecodeActivity>,
    program: &OsStr,
    mut observe: impl FnMut(usize, Box<[u8]>, u64) -> Result<(), CorpusError> + Send,
) -> Result<String, CorpusError> {
    let decoder_memory = activity.map(ReplayDecodeActivity::reserve_decoder);
    let mut command = Command::new(program);
    command.args(["-hide_banner", "-loglevel", "error", "-threads", "1", "-i"]);
    match source {
        DecodeSource::Path(path) => {
            command.arg(path).stdin(Stdio::null());
        }
        DecodeSource::File(file) => {
            command.arg("pipe:0").stdin(Stdio::from(file));
        }
    }
    let child = command
        .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| decode_error(context, format!("ffmpeg decode failed: {error}")))?;
    let mut child = ReapedChild(child);
    let activity_guard = activity
        .zip(decoder_memory)
        .map(|(activity, memory)| activity.enter(child.id(), memory));
    let Some(mut stdout) = child.stdout.take() else {
        kill_and_reap(&mut child);
        return Err(decode_error(
            context,
            "ffmpeg decoder stdout is unavailable".to_owned(),
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        kill_and_reap(&mut child);
        return Err(decode_error(
            context,
            "ffmpeg decoder stderr is unavailable".to_owned(),
        ));
    };
    let stderr = bounded_decode_stderr(stderr);
    if let Some(activity) = &activity_guard {
        activity.sample_rss(child.id());
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let reader = thread::spawn(move || -> Result<String, String> {
        let mut digest = Sha256::new();
        for _ in 0..expected_frames {
            let mut pixels = vec![0u8; 1920 * 1080 * 3].into_boxed_slice();
            stdout
                .read_exact(&mut pixels)
                .map_err(|error| format!("canonical RGB frame read failed: {error}"))?;
            digest.update(&pixels);
            sender
                .send((Instant::now(), pixels))
                .map_err(|_| "canonical decoder consumer stopped".to_owned())?;
        }
        let mut extra = [0_u8; 1];
        if stdout
            .read(&mut extra)
            .map_err(|error| format!("canonical RGB trailer read failed: {error}"))?
            != 0
        {
            return Err("canonical segment decoded more frames than declared".to_owned());
        }
        Ok(hex_digest(digest.finalize().as_slice()))
    });
    let mut last_progress = Instant::now();
    for index in 0..expected_frames {
        let pixels = loop {
            if let Some(activity) = &activity_guard {
                activity.sample_rss(child.id());
            }
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(decoded) => break decoded,
                Err(RecvTimeoutError::Timeout)
                    if last_progress.elapsed() < CANONICAL_DECODE_TIMEOUT => {}
                Err(RecvTimeoutError::Timeout) => {
                    drop(receiver);
                    abort_decoder(&mut child, reader, stderr);
                    return Err(decode_error(
                        context,
                        "canonical decode timed out".to_owned(),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let detail = finish_failed_decoder(&mut child, reader, stderr);
                    return Err(decode_error(context, detail));
                }
            }
        };
        let (decoded_at, pixels) = pixels;
        let decode_consumer_wait_us = duration_us(decoded_at.elapsed());
        let mut callback_timed_out = false;
        let callback_started = Instant::now();
        let callback = thread::scope(|scope| {
            let (callback_sender, callback_receiver) = mpsc::sync_channel(1);
            let observer = &mut observe;
            let callback = scope.spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    observer(index, pixels, decode_consumer_wait_us)
                }));
                let _ = callback_sender.send(result);
            });
            let result = loop {
                if let Some(activity) = &activity_guard {
                    activity.sample_rss(child.id());
                }
                match callback_receiver.recv_timeout(Duration::from_millis(10)) {
                    Ok(result) => break Some(result),
                    Err(RecvTimeoutError::Timeout)
                        if callback_started.elapsed() < CANONICAL_DECODE_TIMEOUT => {}
                    Err(RecvTimeoutError::Timeout) => {
                        kill_and_reap(&mut child);
                        callback_timed_out = true;
                        break None;
                    }
                    Err(RecvTimeoutError::Disconnected) => break None,
                }
            };
            let _ = callback.join();
            result
        });
        if callback_timed_out {
            drop(receiver);
            abort_decoder(&mut child, reader, stderr);
            return Err(decode_error(
                context,
                "canonical decode timed out while consuming a frame".to_owned(),
            ));
        }
        match callback {
            Some(Ok(Ok(()))) => last_progress = Instant::now(),
            Some(Ok(Err(error))) => {
                drop(receiver);
                abort_decoder(&mut child, reader, stderr);
                return Err(error);
            }
            Some(Err(_)) | None => {
                drop(receiver);
                abort_decoder(&mut child, reader, stderr);
                return Err(decode_error(
                    context,
                    "canonical decoder consumer panicked".to_owned(),
                ));
            }
        }
    }
    let status = loop {
        if let Some(activity) = &activity_guard {
            activity.sample_rss(child.id());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                abort_decoder(&mut child, reader, stderr);
                return Err(decode_error(
                    context,
                    format!("ffmpeg wait failed: {error}"),
                ));
            }
        }
        if last_progress.elapsed() >= CANONICAL_DECODE_TIMEOUT {
            drop(receiver);
            abort_decoder(&mut child, reader, stderr);
            return Err(decode_error(
                context,
                "canonical decode timed out".to_owned(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    if let Some(activity) = &activity_guard {
        activity.finish();
    }
    drop(receiver);
    let stderr_bytes = stderr.join().unwrap_or_default();
    let digest = reader
        .join()
        .map_err(|_| decode_error(context, "canonical decoder reader panicked".to_owned()))?
        .map_err(|detail| decode_error(context, detail))?;
    if !status.success() {
        return Err(decode_error(
            context,
            format!(
                "ffmpeg canonical decode failed: {}",
                String::from_utf8_lossy(&stderr_bytes)
            ),
        ));
    }
    Ok(digest)
}

fn decode_error(context: DecodeContext, detail: String) -> CorpusError {
    match context {
        DecodeContext::Verify => CorpusError::InvalidRequest(detail),
        DecodeContext::Replay => CorpusError::InvalidReplay(detail),
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

struct ReapedChild(Child);

impl Deref for ReapedChild {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for ReapedChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ReapedChild {
    fn drop(&mut self) {
        kill_and_reap(&mut self.0);
    }
}

fn abort_decoder(
    child: &mut Child,
    reader: JoinHandle<Result<String, String>>,
    stderr: JoinHandle<Vec<u8>>,
) {
    kill_and_reap(child);
    let _ = reader.join();
    let _ = stderr.join();
}

fn finish_failed_decoder(
    child: &mut Child,
    reader: JoinHandle<Result<String, String>>,
    stderr: JoinHandle<Vec<u8>>,
) -> String {
    kill_and_reap(child);
    let reader = reader.join();
    let stderr = stderr.join().unwrap_or_default();
    match reader {
        Ok(Err(detail)) => detail,
        Err(_) => "canonical decoder reader panicked".to_owned(),
        Ok(Ok(_)) => format!(
            "ffmpeg canonical decode ended before all frames: stderr={}",
            String::from_utf8_lossy(&stderr)
        ),
    }
}

fn bounded_decode_stderr(mut stderr: impl std::io::Read + Send + 'static) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut retained = Vec::new();
        let mut buffer = [0_u8; 4096];
        while let Ok(read) = stderr.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let available = CANONICAL_DECODE_STDERR_BYTES.saturating_sub(retained.len());
            retained.extend_from_slice(&buffer[..read.min(available)]);
        }
        retained
    })
}

fn read_canonical_ticks(path: &Path) -> Result<Vec<CanonicalTick>, CorpusError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut ticks = Vec::new();
    while read_bounded_ndjson_line(&mut reader, &mut line)? {
        if ticks.len() == MAX_NDJSON_RECORDS {
            return invalid("canonical tick index exceeds its capacity");
        }
        ticks.push(serde_json::from_slice(&line)?);
    }
    Ok(ticks)
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a string cannot fail");
    }
    encoded
}

pub fn import_diagnostic(
    store: &Path,
    diagnostic: &Path,
    review_draft: &Path,
) -> Result<DiagnosticImportSummary, CorpusError> {
    let remote = SegmentRemote::from_environment()?;
    let verified = verify_diagnostic(diagnostic)?;
    let (manifest, _) = read_json::<DiagnosticManifest>(&diagnostic.join("manifest.json"))?;
    ensure_store(store)?;
    if manifest.schema == DIAGNOSTIC_SCHEMA {
        return import_canonical_diagnostic(
            store,
            diagnostic,
            review_draft,
            verified,
            manifest,
            remote.as_ref(),
        );
    }
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
        schema: "scorepeek-private-diagnostic-import-v3",
        session_sha256,
        diagnostic_sha256: verified.diagnostic_sha256,
        review_draft: review_draft.to_owned(),
        canonical_frame_count: draft.canonical_frames.len(),
        local_segment_objects: 0,
        remote_segment_objects: 0,
        remote_transferred_objects: 0,
        remote_reused_objects: 0,
        remote_segment_bytes: 0,
    })
}

fn import_canonical_diagnostic(
    store: &Path,
    diagnostic: &Path,
    review_draft: &Path,
    verified: DiagnosticVerificationSummary,
    manifest: DiagnosticManifest,
    remote: Option<&SegmentRemote>,
) -> Result<DiagnosticImportSummary, CorpusError> {
    if manifest.completeness != "complete"
        || manifest.canonical_completeness.as_deref() != Some("complete")
    {
        return invalid("only complete canonical diagnostic sessions can be imported");
    }
    let (canonical, _) = read_json::<CanonicalRecordingManifest>(
        &diagnostic.join("recognition/canonical-manifest.json"),
    )?;
    let ticks = read_canonical_ticks(&diagnostic.join("recognition/canonical-ticks.ndjson"))?;
    let retained = ticks
        .iter()
        .filter(|tick| tick.disposition == "retained")
        .collect::<Vec<_>>();
    let mut frames = Vec::with_capacity(retained.len());
    let mut offset = 0usize;
    for segment in &canonical.segments {
        let expected = retained
            .get(offset..offset.saturating_add(segment.frames))
            .ok_or_else(|| {
                CorpusError::InvalidRequest(
                    "canonical segment exceeds retained tick index".to_owned(),
                )
            })?;
        for tick in expected {
            frames.push(ReviewFrame {
                sequence: tick.sequence,
                artifact_sha256: segment.encoded_sha256.clone(),
            });
        }
        offset = offset.saturating_add(segment.frames);
    }
    if offset != retained.len() {
        return invalid("canonical retained tick coverage differs");
    }
    let segment_paths = canonical
        .segments
        .iter()
        .map(|segment| format!("recognition/{}", segment.path))
        .collect::<BTreeSet<_>>();
    let mut local_segment_objects = 0_u64;
    let mut remote_segment_objects = 0_u64;
    let mut remote_segment_bytes = 0_u64;
    let mut artifacts = Vec::with_capacity(manifest.artifacts.len());
    for artifact in &manifest.artifacts {
        let source = diagnostic.join(safe_relative(&artifact.path)?);
        if segment_paths.contains(&artifact.path) {
            if let Some(remote) = remote {
                remote.upload_verified(File::open(&source)?, &artifact.sha256, artifact.bytes)?;
                remote_segment_objects = remote_segment_objects.saturating_add(1);
                remote_segment_bytes = remote_segment_bytes.saturating_add(artifact.bytes);
            } else {
                publish_object(store, &source, &artifact.sha256, artifact.bytes)?;
                local_segment_objects = local_segment_objects.saturating_add(1);
            }
        } else {
            publish_object(store, &source, &artifact.sha256, artifact.bytes)?;
        }
        artifacts.push(CorpusArtifact {
            kind: artifact.kind.clone(),
            source_path: artifact.path.clone(),
            sha256: artifact.sha256.clone(),
            bytes: artifact.bytes,
        });
    }
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
        normalization_pairs: Vec::new(),
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
            schema: "scorepeek-private-capture-session-identity-v2",
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
        schema: "scorepeek-private-diagnostic-import-v3",
        session_sha256,
        diagnostic_sha256: verified.diagnostic_sha256,
        review_draft: review_draft.to_owned(),
        canonical_frame_count: draft.canonical_frames.len(),
        local_segment_objects,
        remote_segment_objects,
        remote_transferred_objects: remote.map_or(0, |remote| remote.metrics().transferred_objects),
        remote_reused_objects: remote.map_or(0, |remote| remote.metrics().reused_objects),
        remote_segment_bytes,
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
    validate_label_timeline(store, &draft, &label)?;
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
        if label.schema != LABEL_SCHEMA {
            return invalid("numeric dataset requires v5 labels for every active session");
        }
        let screen_sequences = numeric_screen_sequences(store, &session)?;
        let mut plans = Vec::with_capacity(label.episodes.len());
        let mut requested_sequences = BTreeSet::new();
        for episode in &label.episodes {
            episode_count += 1;
            let field_labels = numeric_field_labels(&episode.expected_result)?;
            let requested = numeric_episode_sequences(
                &screen_sequences,
                &episode.stable_sequences,
                usize::MAX,
            )?
            .into_iter()
            .collect::<BTreeSet<_>>();
            requested_sequences.extend(requested.iter().copied());
            plans.push(NumericEpisodePlan {
                episode,
                field_labels,
                requested,
                observed: BTreeSet::new(),
                crops: BTreeSet::new(),
                field_counts: BTreeMap::new(),
            });
        }
        for_each_session_canonical_frame(store, &session, |sequence, pixels| {
            if !requested_sequences.contains(&sequence) {
                return Ok(());
            }
            if inspect_canonical_rgb8(&pixels)
                .map_err(|_| {
                    CorpusError::InvalidRequest("numeric dataset screen predicate failed".into())
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
            for plan in plans
                .iter_mut()
                .filter(|plan| plan.requested.contains(&sequence))
            {
                plan.observed.insert(sequence);
                for (field, label, crop) in numeric_crops(&crops, &plan.field_labels) {
                    if !numeric_field_uses_sequence(field, sequence, &plan.episode.stable_sequences)
                    {
                        continue;
                    }
                    let bytes = ppm_bytes(crop)?;
                    let crop_sha256 = digest(&bytes);
                    if !plan.crops.insert((field, crop_sha256.clone())) {
                        continue;
                    }
                    let field_count = plan.field_counts.entry(field).or_default();
                    if *field_count >= 32 {
                        continue;
                    }
                    *field_count += 1;
                    let filename = format!("images/{crop_sha256}.ppm");
                    let sample = NumericDatasetSample {
                        session_sha256: entry.session_sha256.clone(),
                        episode_id: plan.episode.episode_id.clone(),
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
            Ok(())
        })?;
        for plan in plans {
            if plan.observed != plan.requested {
                return invalid("numeric dataset frame is unavailable");
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
    for_each_session_canonical_frame(store, session, |sequence, pixels| {
        let is_result = inspect_canonical_rgb8(&pixels)
            .map_err(|_| {
                CorpusError::InvalidRequest("numeric dataset screen predicate failed".into())
            })?
            .screen
            == ScreenClass::Result;
        sequences.push((sequence, is_result));
        Ok(())
    })?;
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
    replay_corpus_with_options(store, CorpusReplayOptions::default())
}

pub fn replay_corpus_with_options(
    store: &Path,
    options: CorpusReplayOptions,
) -> Result<CorpusReplaySummary, CorpusError> {
    let segment_resolver = SegmentResolver {
        remote: SegmentRemote::from_environment()?,
        local_segment_decodes: Arc::new(AtomicU64::new(0)),
    };
    let available_parallelism = thread::available_parallelism().map_or(1, usize::from);
    if !(MINIMUM_REPLAY_MEMORY_MIB..=MAXIMUM_REPLAY_MEMORY_MIB).contains(&options.memory_mib)
        || options
            .text_workers
            .is_some_and(|workers| workers == 0 || workers > available_parallelism)
    {
        return invalid("corpus replay worker or memory configuration is invalid");
    }
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
    if suite.schema == SUITE_SCHEMA {
        return replay_canonical_suite(
            store,
            generation_sha256,
            &suite,
            options,
            &CanonicalReplayEnvironment {
                bundle: &bundle,
                catalog_root: &catalog_root,
                diagnostic_root: diagnostic_root.path(),
                segment_resolver: &segment_resolver,
            },
        );
    }
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
                let output = observation.output().as_ref().map_err(|error| {
                    CorpusError::InvalidReplay(format!("production OCR failed: {error}"))
                })?;
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
        schema: "scorepeek-private-corpus-replay-v4",
        generation_sha256,
        session_count: suite.entries.len(),
        episode_count: episodes,
        canonical_frames,
        negative_frames: negatives,
        text_workers: 0,
        preprocess_workers: 0,
        decode_workers: 0,
        maximum_active_sessions: 0,
        maximum_concurrent_decoders: 0,
        decoder_children: 0,
        maximum_blocked_sessions: 0,
        completed_sessions: suite.entries.len(),
        memory_limit_bytes: 0,
        tracked_memory_peak_bytes: 0,
        process_rss_peak_bytes: 0,
        ffmpeg_rss_peak_total_bytes: 0,
        decoder_details: Vec::new(),
        decode_consumer_wait_us: 0,
        preprocess_queue_wait_us: 0,
        preprocess_wall_us: 0,
        screen_classification_us: 0,
        crop_prepare_us: 0,
        field_queue_wait_us: 0,
        text_batch_wall_us: 0,
        maximum_text_worker_inference_us: 0,
        text_worker_busy_us: 0,
        numeric_inference_us: 0,
        field_join_us: 0,
        catalog_projection_us: 0,
        field_frame_wall_us: 0,
        ordered_commit_wait_us: 0,
        decoder_slot_wait_us: 0,
        memory_wait_us: 0,
        sessions: Vec::new(),
        corpus_wall_us: 0,
        local_segment_decodes: 0,
        remote_segment_downloads: 0,
        remote_downloaded_bytes: 0,
    })
}

fn optional_supplemental_matches<T: PartialEq>(
    observed: &SupplementalResultValue<T>,
    expected: &SupplementalResultValue<T>,
) -> bool {
    observed == expected || matches!(observed, SupplementalResultValue::Unknown { .. })
}

type ReplayFieldOutput = Result<
    scorepeek::recognition_live::screen_field_observer::RegisteredScreenFieldObservation,
    scorepeek::recognition::ScreenFieldObservationError<scorepeek::recognition::OnnxParityError>,
>;

struct ReplayPending {
    pending: scorepeek::recognition_live::field_session::PendingSessionFieldObservation<
        ReplayFieldOutput,
    >,
    _memory: ReplayPendingMemory,
}

struct PreparedReplayFrame {
    pixels: Box<[u8]>,
    recognition: scorepeek::recognition_live::PreparedRecognitionFrame,
    memory: ReplayPendingMemory,
    queue_wait_us: u64,
    wall_us: u64,
}

struct PendingReplayPreprocess {
    tick: CanonicalTick,
    receiver: mpsc::Receiver<Result<PreparedReplayFrame, String>>,
}

struct ReplayPreprocessJob {
    pixels: Box<[u8]>,
    memory: ReplayPendingMemory,
    queued_at: Instant,
    output: mpsc::SyncSender<Result<PreparedReplayFrame, String>>,
}

struct ReplayPreprocessPoolInner {
    senders: Vec<mpsc::Sender<Option<ReplayPreprocessJob>>>,
    cursor: AtomicUsize,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Clone)]
struct ReplayPreprocessPool {
    inner: Arc<ReplayPreprocessPoolInner>,
}

impl ReplayPreprocessPool {
    fn start(workers: usize) -> Self {
        let mut senders = Vec::with_capacity(workers);
        let mut handles = Vec::with_capacity(workers);
        for _worker_id in 0..workers {
            let (sender, receiver) = mpsc::channel::<Option<ReplayPreprocessJob>>();
            senders.push(sender);
            handles.push(thread::spawn(move || {
                while let Ok(Some(job)) = receiver.recv() {
                    let queue_wait_us = duration_us(job.queued_at.elapsed());
                    let started = Instant::now();
                    let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        scorepeek::recognition_live::PreparedRecognitionFrame::prepare_since(
                            &job.pixels,
                            job.queued_at,
                        )
                    }));
                    let result = match prepared {
                        Ok(Ok(recognition)) => Ok(PreparedReplayFrame {
                            pixels: job.pixels,
                            recognition,
                            memory: job.memory,
                            queue_wait_us,
                            wall_us: duration_us(started.elapsed()),
                        }),
                        Ok(Err(error)) => {
                            Err(format!("canonical replay preprocessing failed: {error:?}"))
                        }
                        Err(_) => Err("canonical replay preprocessing panicked".to_owned()),
                    };
                    let _ = job.output.send(result);
                }
            }));
        }
        Self {
            inner: Arc::new(ReplayPreprocessPoolInner {
                senders,
                cursor: AtomicUsize::new(0),
                handles: Mutex::new(handles),
            }),
        }
    }

    fn submit(
        &self,
        tick: CanonicalTick,
        pixels: Box<[u8]>,
        memory: ReplayPendingMemory,
    ) -> Result<PendingReplayPreprocess, CorpusError> {
        let (output, receiver) = mpsc::sync_channel(1);
        let index = self.inner.cursor.fetch_add(1, Ordering::Relaxed) % self.inner.senders.len();
        self.inner.senders[index]
            .send(Some(ReplayPreprocessJob {
                pixels,
                memory,
                queued_at: Instant::now(),
                output,
            }))
            .map_err(|_| {
                CorpusError::InvalidReplay("canonical replay preprocessing stopped".to_owned())
            })?;
        Ok(PendingReplayPreprocess { tick, receiver })
    }
}

impl Drop for ReplayPreprocessPoolInner {
    fn drop(&mut self) {
        for sender in &self.senders {
            let _ = sender.send(None);
        }
        for handle in self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            let _ = handle.join();
        }
    }
}

#[derive(Default)]
#[allow(
    clippy::struct_field_names,
    reason = "every replay duration field includes its serialized microsecond unit"
)]
struct ReplayMeasurements {
    decode_consumer_wait_us: u64,
    preprocess_queue_wait_us: u64,
    preprocess_wall_us: u64,
    screen_classification_us: u64,
    crop_prepare_us: u64,
    field_queue_wait_us: u64,
    text_batch_wall_us: u64,
    maximum_text_worker_inference_us: u64,
    text_worker_busy_us: u64,
    numeric_inference_us: u64,
    field_join_us: u64,
    catalog_projection_us: u64,
    field_frame_wall_us: u64,
    ordered_commit_wait_us: u64,
}

struct PreparedReplaySession {
    index: usize,
    session: CaptureSession,
    label: RegressionLabel,
    binding: SessionBinding,
}

#[derive(Clone)]
struct QueuedReplaySession {
    index: usize,
    session_sha256: String,
    label_sha256: String,
    memory_wait_started: Instant,
    memory_wait_us: u64,
}

type ReplayRecognitionSession = scorepeek::recognition_live::field_session::FieldObservationSession<
    scorepeek::recognition_live::screen_field_observer::RegisteredScreenFieldObserver,
>;

struct ReplaySessionRuntime {
    index: usize,
    session: CaptureSession,
    label: RegressionLabel,
    binding: SessionBinding,
    recognition: Option<ReplayRecognitionSession>,
    output: scorepeek::routine_output::RoutineOutput,
    timeline: scorepeek::timeline_driver::TimelineDriver,
    pending: VecDeque<ReplayPending>,
    measurements: ReplayMeasurements,
    failures: Vec<String>,
    canonical: CanonicalRecordingManifest,
    retained: Vec<CanonicalTick>,
    segment_index: usize,
    prefetched_segments: VecDeque<PrefetchedReplaySegment>,
    retained_offset: usize,
    canonical_frames: usize,
    last_sequence: u64,
    last_monotonic_ms: u64,
    session_id: String,
    session_started: Instant,
    decoder_slot_wait_us: u64,
    memory_wait_us: u64,
    _memory: ReplaySessionMemory,
}

impl Drop for ReplaySessionRuntime {
    fn drop(&mut self) {
        if let Some(recognition) = self.recognition.take() {
            let _ = recognition.finish_offline(
                scorepeek::diagnostic_recording::DiagnosticRunStatus::Error,
                self.last_monotonic_ms,
            );
        }
    }
}

enum ReplayWork {
    Queued(QueuedReplaySession),
    Prepared(Box<PreparedReplaySession>, u64),
    Active(Box<ReplaySessionRuntime>),
}

struct ScheduledReplayWork {
    index: usize,
    queued_at: Instant,
    work: ReplayWork,
}

enum ReplayStep {
    Continue(Box<ReplaySessionRuntime>),
    Finalize(Box<ReplaySessionRuntime>),
}

enum ReplayWorkerResult {
    Step {
        index: usize,
        session_key: String,
        result: Result<ReplayStep, CorpusError>,
    },
    Finalized {
        index: usize,
        session_key: String,
        result: Result<ReplaySessionOutcome, CorpusError>,
    },
}

struct ReplaySessionOutcome {
    session_key: String,
    episode_count: usize,
    canonical_frames: usize,
    negative_frames: usize,
    measurements: ReplayMeasurements,
    failures: Vec<String>,
    wall_us: u64,
    decoder_slot_wait_us: u64,
    memory_wait_us: u64,
}

#[derive(Default)]
struct ReplayDecodeActivity {
    active: AtomicUsize,
    maximum_active: AtomicUsize,
    children: AtomicUsize,
    tracked_bytes: AtomicU64,
    tracked_peak_bytes: AtomicU64,
    next_decoder_id: AtomicUsize,
    process_rss_peak_bytes: AtomicU64,
    ffmpeg_rss_peak_total_bytes: AtomicU64,
    ffmpeg_current_rss: Mutex<BTreeMap<usize, u64>>,
    live_pids: Mutex<BTreeMap<usize, u32>>,
    decoder_details: Mutex<Vec<CorpusReplayDecoderSummary>>,
}

impl ReplayDecodeActivity {
    fn reserve_decoder(&self) -> ReplayDecoderMemory<'_> {
        let bytes = self
            .tracked_bytes
            .fetch_add(DECODER_RESERVATION_BYTES as u64, Ordering::AcqRel)
            + DECODER_RESERVATION_BYTES as u64;
        self.tracked_peak_bytes.fetch_max(bytes, Ordering::AcqRel);
        ReplayDecoderMemory { activity: self }
    }

    fn enter<'a>(
        &'a self,
        process_id: u32,
        memory: ReplayDecoderMemory<'a>,
    ) -> ReplayDecodeGuard<'a> {
        let decoder_id = self.next_decoder_id.fetch_add(1, Ordering::AcqRel);
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.maximum_active.fetch_max(active, Ordering::AcqRel);
        self.children.fetch_add(1, Ordering::AcqRel);
        self.live_pids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(decoder_id, process_id);
        ReplayDecodeGuard {
            activity: self,
            decoder_id,
            started: Instant::now(),
            rss_peak_bytes: AtomicU64::new(0),
            finished: std::sync::atomic::AtomicBool::new(false),
            _memory: memory,
        }
    }

    fn reserve_pending(self: &Arc<Self>) -> ReplayPendingMemory {
        let bytes = self.tracked_bytes.fetch_add(
            PENDING_FIELD_FRAME_RESERVATION_BYTES as u64,
            Ordering::AcqRel,
        ) + PENDING_FIELD_FRAME_RESERVATION_BYTES as u64;
        self.tracked_peak_bytes.fetch_max(bytes, Ordering::AcqRel);
        ReplayPendingMemory {
            activity: Arc::clone(self),
        }
    }

    fn reserve_session(self: &Arc<Self>) -> ReplaySessionMemory {
        let bytes = self
            .tracked_bytes
            .fetch_add(SESSION_STATE_RESERVATION_BYTES as u64, Ordering::AcqRel)
            + SESSION_STATE_RESERVATION_BYTES as u64;
        self.tracked_peak_bytes.fetch_max(bytes, Ordering::AcqRel);
        ReplaySessionMemory {
            activity: Arc::clone(self),
        }
    }
}

struct ReplayDecoderMemory<'a> {
    activity: &'a ReplayDecodeActivity,
}

impl Drop for ReplayDecoderMemory<'_> {
    fn drop(&mut self) {
        self.activity
            .tracked_bytes
            .fetch_sub(DECODER_RESERVATION_BYTES as u64, Ordering::AcqRel);
    }
}

struct ReplayDecodeGuard<'a> {
    activity: &'a ReplayDecodeActivity,
    decoder_id: usize,
    started: Instant,
    rss_peak_bytes: AtomicU64,
    finished: std::sync::atomic::AtomicBool,
    _memory: ReplayDecoderMemory<'a>,
}

impl ReplayDecodeGuard<'_> {
    fn sample_rss(&self, child_id: u32) {
        if let Some(bytes) = process_rss_bytes(child_id) {
            self.rss_peak_bytes.fetch_max(bytes, Ordering::AcqRel);
            let mut current = self
                .activity
                .ffmpeg_current_rss
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            current.insert(self.decoder_id, bytes);
            self.activity
                .ffmpeg_rss_peak_total_bytes
                .fetch_max(current.values().copied().sum(), Ordering::AcqRel);
        } else {
            self.activity
                .ffmpeg_current_rss
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&self.decoder_id);
        }
        if let Some(bytes) = process_rss_bytes(std::process::id()) {
            self.activity
                .process_rss_peak_bytes
                .fetch_max(bytes, Ordering::AcqRel);
        }
    }

    fn finish(&self) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        self.activity
            .live_pids
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.decoder_id);
        self.activity
            .ffmpeg_current_rss
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.decoder_id);
        self.activity
            .decoder_details
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(CorpusReplayDecoderSummary {
                decoder_id: self.decoder_id,
                wall_us: u64::try_from(self.started.elapsed().as_micros()).unwrap_or(u64::MAX),
                rss_peak_bytes: self.rss_peak_bytes.load(Ordering::Acquire),
            });
        self.activity.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for ReplayDecodeGuard<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

fn process_rss_bytes(process_id: u32) -> Option<u64> {
    let status = fs::read_to_string(format!("/proc/{process_id}/status")).ok()?;
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_ascii_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    kib.checked_mul(1024)
}

struct ReplayPendingMemory {
    activity: Arc<ReplayDecodeActivity>,
}

struct ReplaySessionMemory {
    activity: Arc<ReplayDecodeActivity>,
}

impl Drop for ReplaySessionMemory {
    fn drop(&mut self) {
        self.activity
            .tracked_bytes
            .fetch_sub(SESSION_STATE_RESERVATION_BYTES as u64, Ordering::AcqRel);
    }
}

impl Drop for ReplayPendingMemory {
    fn drop(&mut self) {
        self.activity.tracked_bytes.fetch_sub(
            PENDING_FIELD_FRAME_RESERVATION_BYTES as u64,
            Ordering::AcqRel,
        );
    }
}

fn replay_canonical_suite(
    store: &Path,
    generation_sha256: String,
    suite: &RegressionSuite,
    options: CorpusReplayOptions,
    environment: &CanonicalReplayEnvironment<'_>,
) -> Result<CorpusReplaySummary, CorpusError> {
    let replay_started = std::time::Instant::now();
    let available_parallelism = std::thread::available_parallelism().map_or(1, usize::from);
    let text_workers = options.text_workers.unwrap_or_else(|| {
        scorepeek::recognition_live::text_observer_pool::select_text_worker_count(
            scorepeek::recognition_live::text_observer_pool::RecognitionExecutionMode::Offline,
            available_parallelism,
        )
    });
    let preprocess_workers = (available_parallelism / 4).clamp(1, 8);
    let memory_limit_bytes = options.memory_mib.saturating_mul(1024 * 1024);
    let memory_decode_slots = (memory_limit_bytes
        / (DECODER_RESERVATION_BYTES
            + 2 * (SESSION_STATE_RESERVATION_BYTES + PENDING_FIELD_FRAME_RESERVATION_BYTES)))
        .max(1);
    let decode_workers = suite
        .entries
        .len()
        .min((available_parallelism / 4).max(1))
        .min(memory_decode_slots);
    if decode_workers == 0 {
        return invalid_replay("canonical replay suite is empty");
    }
    let queue_epoch = Instant::now();
    let mut queued = suite
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| QueuedReplaySession {
            index,
            session_sha256: entry.session_sha256.clone(),
            label_sha256: entry.label_sha256.clone(),
            memory_wait_started: queue_epoch,
            memory_wait_us: 0,
        })
        .collect::<VecDeque<_>>();
    let mut bootstrap_failures: Vec<(usize, String, CorpusError)> = Vec::new();
    let (first_source, first, shared) = loop {
        let Some(source) = queued.pop_front() else {
            bootstrap_failures.sort_by_key(|(index, _, _)| *index);
            return Err(CorpusError::InvalidReplay(
                bootstrap_failures
                    .into_iter()
                    .map(|(index, key, error)| format!("session[{index}] {key}: {error}"))
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        };
        let prepared = match load_prepared_replay_session(store, &source) {
            Ok(prepared) => prepared,
            Err(error) => {
                bootstrap_failures.push((source.index, source.session_sha256.clone(), error));
                continue;
            }
        };
        let descriptor = replay_descriptor(prepared.index, &prepared.session, &prepared.binding);
        match scorepeek::recognition_live::screen_field_observer::SharedRegisteredScreenFieldResources::load(
            &descriptor,
            environment.catalog_root,
            environment.bundle,
            text_workers,
        ) {
            Ok(shared) => break (source, prepared, Arc::new(shared)),
            Err(error) => bootstrap_failures.push((
                source.index,
                source.session_sha256.clone(),
                CorpusError::InvalidReplay(format!(
                    "shared production recognizer could not start: {error}"
                )),
            )),
        }
    };
    let maximum_active_sessions = suite
        .entries
        .len()
        .min(decode_workers.saturating_mul(2).max(1));
    let pending_slots = memory_limit_bytes
        .saturating_sub(decode_workers.saturating_mul(DECODER_RESERVATION_BYTES))
        .saturating_sub(maximum_active_sessions.saturating_mul(SESSION_STATE_RESERVATION_BYTES))
        / PENDING_FIELD_FRAME_RESERVATION_BYTES;
    let per_session_pending_limit = pending_slots
        .checked_div(maximum_active_sessions)
        .unwrap_or(1)
        .max(1)
        .min(text_workers.saturating_mul(2));
    let decode_activity = Arc::new(ReplayDecodeActivity::default());
    let preprocess_pool = ReplayPreprocessPool::start(preprocess_workers);
    let memory_wait_epoch = Instant::now();
    for source in &mut queued {
        source.memory_wait_started = memory_wait_epoch;
    }
    let mut ready = VecDeque::new();
    ready.push_back(ScheduledReplayWork {
        index: first.index,
        queued_at: Instant::now(),
        work: ReplayWork::Prepared(Box::new(first), first_source.memory_wait_us),
    });
    while ready.len() < maximum_active_sessions {
        let Some(source) = queued.pop_front() else {
            break;
        };
        ready.push_back(ScheduledReplayWork {
            index: source.index,
            queued_at: Instant::now(),
            work: ReplayWork::Queued(source),
        });
    }
    let (work_sender, work_receiver) = mpsc::channel::<Option<ScheduledReplayWork>>();
    let work_receiver = Arc::new(Mutex::new(work_receiver));
    let (result_sender, result_receiver) = mpsc::channel::<ReplayWorkerResult>();
    let (finalize_sender, finalize_receiver) =
        mpsc::channel::<Option<(usize, String, ReplaySessionRuntime)>>();
    let finalize_receiver = Arc::new(Mutex::new(finalize_receiver));
    let mut handles = Vec::with_capacity(decode_workers);
    for _ in 0..decode_workers {
        let receiver = Arc::clone(&work_receiver);
        let result_sender = result_sender.clone();
        let shared = Arc::clone(&shared);
        let activity = Arc::clone(&decode_activity);
        let preprocess_pool = preprocess_pool.clone();
        let store = store.to_owned();
        let diagnostic_root = environment.diagnostic_root.to_owned();
        let segment_resolver = environment.segment_resolver.clone();
        handles.push(thread::spawn(move || {
            loop {
                let message = receiver
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv();
                let Ok(Some(work)) = message else { break };
                let index = work.index;
                let session_key = match &work.work {
                    ReplayWork::Queued(source) => source.session_sha256.clone(),
                    ReplayWork::Prepared(prepared, _) => prepared.session.source_session_id.clone(),
                    ReplayWork::Active(runtime) => runtime.session_id.clone(),
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let context = ReplayStepContext {
                        store: &store,
                        diagnostic_root: &diagnostic_root,
                        shared: &shared,
                        decode_activity: &activity,
                        preprocess_pool: &preprocess_pool,
                        outstanding_limit: per_session_pending_limit,
                        segment_resolver: &segment_resolver,
                    };
                    execute_replay_step(&context, work)
                }))
                .unwrap_or_else(|_| {
                    Err(CorpusError::InvalidReplay(
                        "canonical replay worker panicked".to_owned(),
                    ))
                });
                if result_sender
                    .send(ReplayWorkerResult::Step {
                        index,
                        session_key,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    let mut finalizer_handles = Vec::with_capacity(decode_workers);
    for _ in 0..decode_workers {
        let receiver = Arc::clone(&finalize_receiver);
        let result_sender = result_sender.clone();
        finalizer_handles.push(thread::spawn(move || {
            loop {
                let message = receiver
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv();
                let Ok(Some((index, session_key, runtime))) = message else {
                    break;
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    finalize_replay_session(runtime)
                }))
                .unwrap_or_else(|_| {
                    Err(CorpusError::InvalidReplay(
                        "canonical replay finalizer panicked".to_owned(),
                    ))
                });
                if result_sender
                    .send(ReplayWorkerResult::Finalized {
                        index,
                        session_key,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
    drop(result_sender);
    let mut inflight = 0usize;
    let mut active_sessions = ready.len();
    let mut maximum_blocked_sessions = 0usize;
    let mut completed = bootstrap_failures.len();
    let mut results = Vec::with_capacity(suite.entries.len());
    results.extend(
        bootstrap_failures
            .drain(..)
            .map(|(index, key, error)| (index, key, Err(error))),
    );
    while completed < suite.entries.len() {
        while inflight < decode_workers {
            let Some(work) = ready.pop_front() else { break };
            work_sender.send(Some(work)).map_err(|_| {
                CorpusError::InvalidReplay("canonical replay scheduler stopped".to_owned())
            })?;
            inflight = inflight.saturating_add(1);
        }
        maximum_blocked_sessions = maximum_blocked_sessions.max(
            queued
                .len()
                .saturating_add(ready.len())
                .saturating_add(inflight.saturating_sub(decode_workers)),
        );
        let worker_result = result_receiver.recv().map_err(|_| {
            CorpusError::InvalidReplay("canonical replay worker stopped".to_owned())
        })?;
        match worker_result {
            ReplayWorkerResult::Step {
                index,
                session_key,
                result,
            } => {
                inflight = inflight.saturating_sub(1);
                match result {
                    Ok(ReplayStep::Continue(runtime)) => ready.push_back(ScheduledReplayWork {
                        index,
                        queued_at: Instant::now(),
                        work: ReplayWork::Active(runtime),
                    }),
                    Ok(ReplayStep::Finalize(runtime)) => finalize_sender
                        .send(Some((index, session_key, *runtime)))
                        .map_err(|_| {
                            CorpusError::InvalidReplay(
                                "canonical replay finalizer stopped".to_owned(),
                            )
                        })?,
                    Err(error) => {
                        results.push((index, session_key, Err(error)));
                        active_sessions = active_sessions.saturating_sub(1);
                        completed = completed.saturating_add(1);
                    }
                }
            }
            ReplayWorkerResult::Finalized {
                index,
                session_key,
                result,
            } => {
                results.push((index, session_key, result));
                active_sessions = active_sessions.saturating_sub(1);
                completed = completed.saturating_add(1);
            }
        }
        while active_sessions < maximum_active_sessions {
            let Some(mut source) = queued.pop_front() else {
                break;
            };
            source.memory_wait_us = source.memory_wait_us.saturating_add(
                u64::try_from(source.memory_wait_started.elapsed().as_micros()).unwrap_or(u64::MAX),
            );
            ready.push_back(ScheduledReplayWork {
                index: source.index,
                queued_at: Instant::now(),
                work: ReplayWork::Queued(source),
            });
            active_sessions = active_sessions.saturating_add(1);
        }
    }
    for _ in 0..decode_workers {
        let _ = work_sender.send(None);
        let _ = finalize_sender.send(None);
    }
    drop(work_sender);
    for handle in handles {
        if handle.join().is_err() {
            results.push((
                usize::MAX,
                "worker".to_owned(),
                Err(CorpusError::InvalidReplay(
                    "canonical replay worker panicked".to_owned(),
                )),
            ));
        }
    }
    drop(finalize_sender);
    for handle in finalizer_handles {
        if handle.join().is_err() {
            results.push((
                usize::MAX,
                "finalizer".to_owned(),
                Err(CorpusError::InvalidReplay(
                    "canonical replay finalizer panicked".to_owned(),
                )),
            ));
        }
    }
    results.sort_by_key(|(index, _, _)| *index);
    let mut measurements = ReplayMeasurements::default();
    let mut episode_count = 0usize;
    let mut canonical_frames = 0usize;
    let mut negative_frames = 0usize;
    let mut failures = Vec::new();
    let mut sessions = Vec::with_capacity(suite.entries.len());
    let mut decoder_slot_wait_us = 0_u64;
    let mut memory_wait_us = 0_u64;
    for (index, session_key, result) in results {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                failures.push(format!("session[{index}] {session_key}: {error}"));
                continue;
            }
        };
        episode_count = episode_count.saturating_add(outcome.episode_count);
        canonical_frames = canonical_frames.saturating_add(outcome.canonical_frames);
        negative_frames = negative_frames.saturating_add(outcome.negative_frames);
        measurements.text_batch_wall_us = measurements
            .text_batch_wall_us
            .saturating_add(outcome.measurements.text_batch_wall_us);
        measurements.field_queue_wait_us = measurements
            .field_queue_wait_us
            .saturating_add(outcome.measurements.field_queue_wait_us);
        measurements.maximum_text_worker_inference_us = measurements
            .maximum_text_worker_inference_us
            .max(outcome.measurements.maximum_text_worker_inference_us);
        measurements.numeric_inference_us = measurements
            .numeric_inference_us
            .saturating_add(outcome.measurements.numeric_inference_us);
        measurements.field_join_us = measurements
            .field_join_us
            .saturating_add(outcome.measurements.field_join_us);
        measurements.catalog_projection_us = measurements
            .catalog_projection_us
            .saturating_add(outcome.measurements.catalog_projection_us);
        measurements.decode_consumer_wait_us = measurements
            .decode_consumer_wait_us
            .saturating_add(outcome.measurements.decode_consumer_wait_us);
        measurements.preprocess_queue_wait_us = measurements
            .preprocess_queue_wait_us
            .saturating_add(outcome.measurements.preprocess_queue_wait_us);
        measurements.preprocess_wall_us = measurements
            .preprocess_wall_us
            .saturating_add(outcome.measurements.preprocess_wall_us);
        measurements.screen_classification_us = measurements
            .screen_classification_us
            .saturating_add(outcome.measurements.screen_classification_us);
        measurements.crop_prepare_us = measurements
            .crop_prepare_us
            .saturating_add(outcome.measurements.crop_prepare_us);
        measurements.text_worker_busy_us = measurements
            .text_worker_busy_us
            .saturating_add(outcome.measurements.text_worker_busy_us);
        measurements.field_frame_wall_us = measurements
            .field_frame_wall_us
            .saturating_add(outcome.measurements.field_frame_wall_us);
        measurements.ordered_commit_wait_us = measurements
            .ordered_commit_wait_us
            .saturating_add(outcome.measurements.ordered_commit_wait_us);
        failures.extend(
            outcome
                .failures
                .iter()
                .map(|failure| format!("{}: {failure}", outcome.session_key)),
        );
        decoder_slot_wait_us = decoder_slot_wait_us.saturating_add(outcome.decoder_slot_wait_us);
        memory_wait_us = memory_wait_us.saturating_add(outcome.memory_wait_us);
        sessions.push(CorpusReplaySessionSummary {
            session_key: outcome.session_key,
            wall_us: outcome.wall_us,
            canonical_frames: outcome.canonical_frames,
        });
    }
    if !failures.is_empty() {
        return Err(CorpusError::InvalidReplay(failures.join("; ")));
    }
    let mut decoder_details = decode_activity
        .decoder_details
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    decoder_details.sort_by_key(|decoder| decoder.decoder_id);
    Ok(CorpusReplaySummary {
        schema: "scorepeek-private-corpus-replay-v4",
        generation_sha256,
        session_count: suite.entries.len(),
        episode_count,
        canonical_frames,
        negative_frames,
        text_workers,
        preprocess_workers,
        decode_workers,
        maximum_active_sessions,
        maximum_concurrent_decoders: decode_activity.maximum_active.load(Ordering::Acquire),
        decoder_children: decode_activity.children.load(Ordering::Acquire),
        maximum_blocked_sessions,
        completed_sessions: suite.entries.len(),
        memory_limit_bytes: u64::try_from(memory_limit_bytes).unwrap_or(u64::MAX),
        tracked_memory_peak_bytes: decode_activity.tracked_peak_bytes.load(Ordering::Acquire),
        process_rss_peak_bytes: decode_activity
            .process_rss_peak_bytes
            .load(Ordering::Acquire),
        ffmpeg_rss_peak_total_bytes: decode_activity
            .ffmpeg_rss_peak_total_bytes
            .load(Ordering::Acquire),
        decoder_details,
        decode_consumer_wait_us: measurements.decode_consumer_wait_us,
        preprocess_queue_wait_us: measurements.preprocess_queue_wait_us,
        preprocess_wall_us: measurements.preprocess_wall_us,
        screen_classification_us: measurements.screen_classification_us,
        crop_prepare_us: measurements.crop_prepare_us,
        field_queue_wait_us: measurements.field_queue_wait_us,
        text_batch_wall_us: measurements.text_batch_wall_us,
        maximum_text_worker_inference_us: measurements.maximum_text_worker_inference_us,
        text_worker_busy_us: measurements.text_worker_busy_us,
        numeric_inference_us: measurements.numeric_inference_us,
        field_join_us: measurements.field_join_us,
        catalog_projection_us: measurements.catalog_projection_us,
        field_frame_wall_us: measurements.field_frame_wall_us,
        ordered_commit_wait_us: measurements.ordered_commit_wait_us,
        decoder_slot_wait_us,
        memory_wait_us,
        sessions,
        corpus_wall_us: u64::try_from(replay_started.elapsed().as_micros()).unwrap_or(u64::MAX),
        local_segment_decodes: environment
            .segment_resolver
            .local_segment_decodes
            .load(Ordering::Acquire),
        remote_segment_downloads: environment
            .segment_resolver
            .remote
            .as_ref()
            .map_or(0, |remote| remote.metrics().downloaded_segments),
        remote_downloaded_bytes: environment
            .segment_resolver
            .remote
            .as_ref()
            .map_or(0, |remote| remote.metrics().downloaded_bytes),
    })
}

fn replay_descriptor(
    session_index: usize,
    session: &CaptureSession,
    binding: &SessionBinding,
) -> scorepeek::diagnostic_recording::DiagnosticRunDescriptor {
    scorepeek::diagnostic_recording::DiagnosticRunDescriptor {
        run_id: format!("canonical-corpus-replay-{session_index}"),
        monotonic_start_ms: 0,
        resource: scorepeek::diagnostic_recording::DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: "0".repeat(64),
        },
        binding: scorepeek::diagnostic_recording::DiagnosticBinding {
            capture_generation: session.capture_generation,
            capture_profile_sha256: binding.capture_profile_sha256.clone(),
            normalizer_sha256: binding.normalizer_sha256.clone(),
            canonical_layout_sha256: scorepeek::recognition::CanonicalLayout::sha256(),
            catalog_sha256: session.catalog_sha256.clone(),
            model_sha256: scorepeek::recognition::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: scorepeek::recognition::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    }
}

fn load_prepared_replay_session(
    store: &Path,
    source: &QueuedReplaySession,
) -> Result<PreparedReplaySession, CorpusError> {
    let (session, session_bytes) = read_json::<CaptureSession>(
        &store
            .join("sessions")
            .join(format!("{}.json", source.session_sha256)),
    )?;
    let (label, label_bytes) = read_regression_label(
        &store
            .join("labels")
            .join(format!("{}.json", source.label_sha256)),
    )?;
    if session.schema != SESSION_SCHEMA
        || session.completeness != "complete"
        || digest(&session_bytes) != source.session_sha256
        || digest(&label_bytes) != source.label_sha256
        || label.session_sha256 != source.session_sha256
    {
        return invalid("canonical suite entry binding is invalid");
    }
    let binding = session_binding(store, &session)?;
    Ok(PreparedReplaySession {
        index: source.index,
        session,
        label,
        binding,
    })
}

fn start_replay_session(
    store: &Path,
    diagnostic_root: &Path,
    prepared: PreparedReplaySession,
    shared: Arc<
        scorepeek::recognition_live::screen_field_observer::SharedRegisteredScreenFieldResources,
    >,
    decode_activity: &Arc<ReplayDecodeActivity>,
    memory_wait_us: u64,
) -> Result<ReplaySessionRuntime, CorpusError> {
    let PreparedReplaySession {
        index: session_index,
        session,
        label,
        binding,
    } = prepared;
    let session_id = session.source_session_id.clone();
    let canonical_manifest_object =
        session_object_for_source(store, &session, "recognition/canonical-manifest.json")?;
    let (canonical, _) = read_json::<CanonicalRecordingManifest>(&canonical_manifest_object)?;
    if canonical.completeness != "complete" || canonical.dropped_frames != 0 {
        return invalid_replay("canonical session is incomplete");
    }
    let tick_object =
        session_object_for_source(store, &session, "recognition/canonical-ticks.ndjson")?;
    let retained = read_canonical_ticks(&tick_object)?
        .into_iter()
        .filter(|tick| tick.disposition == "retained")
        .collect::<Vec<_>>();
    let mut output = scorepeek::routine_output::RoutineOutput::start_headless(
        format!("corpus-{session_index}"),
        session.profile_sha256.clone(),
    );
    output
        .publish(&scorepeek::routine_output::RunEvent {
            schema: "scorepeek-run-event-v8".to_owned(),
            kind: scorepeek::routine_output::RunEventKind::SessionStarted {
                session_id: Some(session_id.clone()),
                capture_generation: session.capture_generation,
                capture_profile_sha256: binding.capture_profile_sha256.clone(),
                normalizer_artifact_sha256: binding.normalizer_sha256.clone(),
            },
        })
        .map_err(CorpusError::InvalidReplay)?;
    let descriptor = replay_descriptor(session_index, &session, &binding);
    let recognition =
        scorepeek::recognition_live::field_session::FieldObservationSession::start_registered_shared(
            diagnostic_root,
            descriptor,
            scorepeek::diagnostic_recording::DiagnosticPolicy {
                enabled: false,
                ..scorepeek::diagnostic_recording::DiagnosticPolicy::default()
            },
            shared,
        )
        .map_err(|error| {
            CorpusError::InvalidReplay(format!(
                "production recognizer could not start: {error:?}"
            ))
        })?;
    Ok(ReplaySessionRuntime {
        index: session_index,
        session,
        label,
        binding,
        recognition: Some(recognition),
        output,
        timeline: scorepeek::timeline_driver::TimelineDriver::default(),
        pending: VecDeque::new(),
        measurements: ReplayMeasurements::default(),
        failures: Vec::new(),
        canonical,
        retained,
        segment_index: 0,
        prefetched_segments: VecDeque::new(),
        retained_offset: 0,
        canonical_frames: 0,
        last_sequence: 0,
        last_monotonic_ms: 0,
        session_id,
        session_started: Instant::now(),
        decoder_slot_wait_us: 0,
        memory_wait_us,
        _memory: decode_activity.reserve_session(),
    })
}

fn execute_replay_step(
    context: &ReplayStepContext<'_>,
    scheduled: ScheduledReplayWork,
) -> Result<ReplayStep, CorpusError> {
    let slot_wait_us = u64::try_from(scheduled.queued_at.elapsed().as_micros()).unwrap_or(u64::MAX);
    match scheduled.work {
        ReplayWork::Queued(source) => {
            let prepared = load_prepared_replay_session(context.store, &source)?;
            let mut runtime = start_replay_session(
                context.store,
                context.diagnostic_root,
                prepared,
                Arc::clone(context.shared),
                context.decode_activity,
                source.memory_wait_us,
            )?;
            runtime.decoder_slot_wait_us =
                runtime.decoder_slot_wait_us.saturating_add(slot_wait_us);
            process_replay_segment(
                context.store,
                &mut runtime,
                context.decode_activity,
                context.preprocess_pool,
                context.outstanding_limit,
                context.segment_resolver,
            )?;
            Ok(
                if runtime.segment_index == runtime.canonical.segments.len() {
                    ReplayStep::Finalize(Box::new(runtime))
                } else {
                    ReplayStep::Continue(Box::new(runtime))
                },
            )
        }
        ReplayWork::Prepared(prepared, memory_wait_us) => {
            let mut runtime = start_replay_session(
                context.store,
                context.diagnostic_root,
                *prepared,
                Arc::clone(context.shared),
                context.decode_activity,
                memory_wait_us,
            )?;
            runtime.decoder_slot_wait_us =
                runtime.decoder_slot_wait_us.saturating_add(slot_wait_us);
            process_replay_segment(
                context.store,
                &mut runtime,
                context.decode_activity,
                context.preprocess_pool,
                context.outstanding_limit,
                context.segment_resolver,
            )?;
            Ok(
                if runtime.segment_index == runtime.canonical.segments.len() {
                    ReplayStep::Finalize(Box::new(runtime))
                } else {
                    ReplayStep::Continue(Box::new(runtime))
                },
            )
        }
        ReplayWork::Active(mut runtime) => {
            runtime.decoder_slot_wait_us =
                runtime.decoder_slot_wait_us.saturating_add(slot_wait_us);
            process_replay_segment(
                context.store,
                &mut runtime,
                context.decode_activity,
                context.preprocess_pool,
                context.outstanding_limit,
                context.segment_resolver,
            )?;
            Ok(
                if runtime.segment_index == runtime.canonical.segments.len() {
                    ReplayStep::Finalize(runtime)
                } else {
                    ReplayStep::Continue(runtime)
                },
            )
        }
    }
}

fn process_replay_segment(
    store: &Path,
    runtime: &mut ReplaySessionRuntime,
    decode_activity: &Arc<ReplayDecodeActivity>,
    preprocess_pool: &ReplayPreprocessPool,
    outstanding_limit: usize,
    segment_resolver: &SegmentResolver,
) -> Result<(), CorpusError> {
    let segment = runtime
        .canonical
        .segments
        .get(runtime.segment_index)
        .cloned()
        .ok_or_else(|| CorpusError::InvalidReplay("canonical segment is unavailable".to_owned()))?;
    let expected = runtime
        .retained
        .get(runtime.retained_offset..runtime.retained_offset.saturating_add(segment.frames))
        .ok_or_else(|| {
            CorpusError::InvalidReplay("canonical segment exceeds retained tick index".to_owned())
        })?
        .to_vec();
    let object = match runtime.prefetched_segments.pop_front() {
        Some(prefetched) if prefetched.segment_index == runtime.segment_index => {
            prefetched.finish()?
        }
        Some(_) => return invalid_replay("canonical segment prefetch order differs"),
        None => segment_resolver.resolve(
            store,
            &runtime.session,
            &format!("recognition/{}", segment.path),
        )?,
    };
    fill_replay_segment_prefetch(store, runtime, segment_resolver);
    let mut preprocessing = VecDeque::new();
    let decoded_digest = decode_resolved_canonical_frames(
        &object,
        segment.frames,
        DecodeContext::Replay,
        Some(decode_activity),
        |index, pixels, decode_consumer_wait_us| {
            runtime.measurements.decode_consumer_wait_us = runtime
                .measurements
                .decode_consumer_wait_us
                .saturating_add(decode_consumer_wait_us);
            let tick = expected.get(index).ok_or_else(|| {
                CorpusError::InvalidReplay("canonical decoded frame exceeds tick index".to_owned())
            })?;
            preprocessing.push_back(preprocess_pool.submit(
                tick.clone(),
                pixels,
                decode_activity.reserve_pending(),
            )?);
            while preprocessing.len() >= outstanding_limit {
                commit_replay_preprocessed(runtime, &mut preprocessing, true, outstanding_limit)?;
            }
            commit_replay_preprocessed(runtime, &mut preprocessing, false, outstanding_limit)
        },
    )?;
    while !preprocessing.is_empty() {
        commit_replay_preprocessed(runtime, &mut preprocessing, true, outstanding_limit)?;
    }
    if decoded_digest != segment.raw_rgb24_sha256 {
        return invalid_replay("canonical segment decoded pixel digest differs");
    }
    runtime.retained_offset = runtime.retained_offset.saturating_add(segment.frames);
    runtime.segment_index = runtime.segment_index.saturating_add(1);
    if runtime.segment_index == runtime.canonical.segments.len()
        && runtime.retained_offset != runtime.retained.len()
    {
        return invalid_replay("canonical segment coverage differs");
    }
    Ok(())
}

fn fill_replay_segment_prefetch(
    store: &Path,
    runtime: &mut ReplaySessionRuntime,
    segment_resolver: &SegmentResolver,
) {
    let mut next_segment_index = runtime.prefetched_segments.back().map_or_else(
        || runtime.segment_index.saturating_add(1),
        |item| item.segment_index.saturating_add(1),
    );
    while runtime.prefetched_segments.len() < REPLAY_SEGMENT_PREFETCH {
        let Some(next_segment) = runtime.canonical.segments.get(next_segment_index) else {
            break;
        };
        runtime
            .prefetched_segments
            .push_back(PrefetchedReplaySegment::start(
                next_segment_index,
                store.to_owned(),
                runtime.session.clone(),
                format!("recognition/{}", next_segment.path),
                segment_resolver.clone(),
            ));
        next_segment_index = next_segment_index.saturating_add(1);
    }
}

fn commit_replay_preprocessed(
    runtime: &mut ReplaySessionRuntime,
    preprocessing: &mut VecDeque<PendingReplayPreprocess>,
    wait: bool,
    outstanding_limit: usize,
) -> Result<(), CorpusError> {
    let Some(front) = preprocessing.front() else {
        return Ok(());
    };
    let prepared = if wait {
        front
            .receiver
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| {
                CorpusError::InvalidReplay("canonical replay preprocessing timed out".to_owned())
            })?
    } else {
        match front.receiver.try_recv() {
            Ok(prepared) => prepared,
            Err(mpsc::TryRecvError::Empty) => return Ok(()),
            Err(mpsc::TryRecvError::Disconnected) => {
                return invalid_replay("canonical replay preprocessing stopped");
            }
        }
    };
    let pending = preprocessing
        .pop_front()
        .expect("prepared replay queue has a front");
    let prepared = prepared.map_err(CorpusError::InvalidReplay)?;
    process_replay_frame(runtime, &pending.tick, prepared, outstanding_limit)
}

fn process_replay_frame(
    runtime: &mut ReplaySessionRuntime,
    tick: &CanonicalTick,
    prepared: PreparedReplayFrame,
    outstanding_limit: usize,
) -> Result<(), CorpusError> {
    while runtime.pending.len() >= outstanding_limit {
        commit_replay_pending(
            runtime.recognition.as_mut().expect("recognizer is active"),
            &mut runtime.pending,
            &mut runtime.output,
            true,
            &runtime.session_id,
            runtime.session.capture_generation,
            &mut runtime.measurements,
        )?;
    }
    commit_replay_pending(
        runtime.recognition.as_mut().expect("recognizer is active"),
        &mut runtime.pending,
        &mut runtime.output,
        false,
        &runtime.session_id,
        runtime.session.capture_generation,
        &mut runtime.measurements,
    )?;
    let PreparedReplayFrame {
        pixels,
        recognition: prepared_recognition,
        memory,
        queue_wait_us,
        wall_us,
    } = prepared;
    runtime.measurements.preprocess_queue_wait_us = runtime
        .measurements
        .preprocess_queue_wait_us
        .saturating_add(queue_wait_us);
    runtime.measurements.preprocess_wall_us = runtime
        .measurements
        .preprocess_wall_us
        .saturating_add(wall_us);
    runtime.measurements.screen_classification_us = runtime
        .measurements
        .screen_classification_us
        .saturating_add(prepared_recognition.screen_classification_us());
    runtime.measurements.crop_prepare_us = runtime
        .measurements
        .crop_prepare_us
        .saturating_add(prepared_recognition.crop_prepare_us().unwrap_or(0));
    let frame = scorepeek::diagnostic_live::BoundCanonicalFrame::for_replay(
        runtime.session.capture_generation,
        tick.sequence,
        tick.monotonic_ms,
        runtime.binding.capture_profile_sha256.clone(),
        runtime.binding.normalizer_sha256.clone(),
        pixels,
    )
    .map_err(|_| CorpusError::InvalidReplay("canonical replay frame is invalid".to_owned()))?;
    let inspected = runtime
        .recognition
        .as_mut()
        .expect("recognizer is active")
        .inspect_prepared(&frame, prepared_recognition)
        .map_err(|_| CorpusError::InvalidReplay("production frame inspection failed".to_owned()))?;
    let screen = inspected.observation.screen();
    let timeline_step = runtime
        .timeline
        .observe(screen.into(), tick.sequence, tick.monotonic_ms);
    runtime
        .output
        .publish(&scorepeek::routine_output::RunEvent {
            schema: "scorepeek-run-event-v8".to_owned(),
            kind: scorepeek::routine_output::RunEventKind::RawScreenObserved {
                session_id: Some(runtime.session_id.clone()),
                capture_generation: Some(runtime.session.capture_generation),
                semantic_episode_id: timeline_step.active_episode_id,
                sequence: tick.sequence,
                monotonic_start_ms: tick.monotonic_ms,
                monotonic_end_ms: tick.monotonic_ms,
                screen: replay_screen_name(screen).to_owned(),
                unknown_reason: (screen == ScreenClass::Unknown)
                    .then(|| "predicate_not_matched".to_owned()),
            },
        })
        .map_err(CorpusError::InvalidReplay)?;
    apply_replay_timeline_actions(
        timeline_step.actions,
        runtime.recognition.as_mut().expect("recognizer is active"),
        &mut runtime.pending,
        &mut runtime.output,
        &runtime.session_id,
        runtime.session.capture_generation,
        tick.sequence,
        tick.monotonic_ms,
        &mut runtime.measurements,
    )?;
    match inspected.field_submission {
        scorepeek::recognition_live::field_session::FieldObservationSubmission::NotApplicable => {}
        scorepeek::recognition_live::field_session::FieldObservationSubmission::Submitted(
            mut field,
        ) => {
            let episode_id = runtime.timeline.active_episode_id().ok_or_else(|| {
                CorpusError::InvalidReplay("field observation has no semantic episode".to_owned())
            })?;
            field.bind_screen_episode(episode_id);
            runtime.pending.push_back(ReplayPending {
                pending: field,
                _memory: memory,
            });
        }
        scorepeek::recognition_live::field_session::FieldObservationSubmission::BusySkipped => {
            return invalid_replay("offline replay skipped field OCR as busy");
        }
        scorepeek::recognition_live::field_session::FieldObservationSubmission::Rejected(error) => {
            return Err(CorpusError::InvalidReplay(format!(
                "offline replay rejected field OCR: {error:?}"
            )));
        }
    }
    runtime.canonical_frames = runtime.canonical_frames.saturating_add(1);
    runtime.last_sequence = tick.sequence;
    runtime.last_monotonic_ms = tick.monotonic_ms;
    Ok(())
}

fn finalize_replay_session(
    mut runtime: ReplaySessionRuntime,
) -> Result<ReplaySessionOutcome, CorpusError> {
    let finish_actions = runtime.timeline.finish();
    if finish_actions.is_empty() {
        drain_replay_pending(
            runtime.recognition.as_mut().expect("recognizer is active"),
            &mut runtime.pending,
            &mut runtime.output,
            &runtime.session_id,
            runtime.session.capture_generation,
            &mut runtime.measurements,
        )?;
    } else {
        apply_replay_timeline_actions(
            finish_actions,
            runtime.recognition.as_mut().expect("recognizer is active"),
            &mut runtime.pending,
            &mut runtime.output,
            &runtime.session_id,
            runtime.session.capture_generation,
            runtime.last_sequence,
            runtime.last_monotonic_ms,
            &mut runtime.measurements,
        )?;
    }
    runtime
        .output
        .publish(&scorepeek::routine_output::RunEvent {
            schema: "scorepeek-run-event-v8".to_owned(),
            kind: scorepeek::routine_output::RunEventKind::SessionFinished {
                session_id: runtime.session_id.clone(),
                capture_generation: runtime.session.capture_generation,
                outcome: "replayed".to_owned(),
                report: serde_json::json!({}),
            },
        })
        .map_err(CorpusError::InvalidReplay)?;
    let recognition = runtime.recognition.take().expect("recognizer is active");
    let finish = recognition.finish(
        scorepeek::diagnostic_recording::DiagnosticRunStatus::Success,
        runtime.last_monotonic_ms,
        Duration::from_secs(30),
    );
    if finish.field_observer.status
        != scorepeek::recognition_live::field_observer::FieldObserverFinishStatus::Complete
    {
        return invalid_replay("production recognizer did not finish cleanly");
    }
    let emitted = runtime
        .output
        .take_headless_events()
        .into_iter()
        .filter_map(|event| match event.kind {
            scorepeek::routine_output::RunEventKind::ResultDetected { result, .. } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    validate_semantic_oracle(&runtime.label, &emitted, &mut runtime.failures);
    Ok(ReplaySessionOutcome {
        session_key: runtime.session_id.clone(),
        episode_count: runtime.label.episodes.len(),
        canonical_frames: runtime.canonical_frames,
        negative_frames: runtime.label.negative_frames.len(),
        measurements: std::mem::take(&mut runtime.measurements),
        failures: std::mem::take(&mut runtime.failures),
        wall_us: u64::try_from(runtime.session_started.elapsed().as_micros()).unwrap_or(u64::MAX),
        decoder_slot_wait_us: runtime.decoder_slot_wait_us,
        memory_wait_us: runtime.memory_wait_us,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "the replay adapter executes one shared timeline action against bound session state"
)]
fn apply_replay_timeline_actions(
    actions: Vec<scorepeek::timeline_driver::TimelineAction>,
    recognition: &mut scorepeek::recognition_live::field_session::FieldObservationSession<
        scorepeek::recognition_live::screen_field_observer::RegisteredScreenFieldObserver,
    >,
    pending: &mut VecDeque<ReplayPending>,
    output: &mut scorepeek::routine_output::RoutineOutput,
    session_id: &str,
    generation: u64,
    sequence: u64,
    monotonic_ms: u64,
    measurements: &mut ReplayMeasurements,
) -> Result<(), CorpusError> {
    for action in actions {
        match action {
            scorepeek::timeline_driver::TimelineAction::Semantic { episode, phase } => {
                publish_replay_semantic(
                    output,
                    session_id,
                    generation,
                    episode,
                    sequence,
                    monotonic_ms,
                    replay_semantic_phase(phase),
                )?;
            }
            scorepeek::timeline_driver::TimelineAction::DrainAdmitted { .. } => {
                drain_replay_pending(
                    recognition,
                    pending,
                    output,
                    session_id,
                    generation,
                    measurements,
                )?;
            }
        }
    }
    Ok(())
}

const fn replay_semantic_phase(
    phase: scorepeek::timeline_driver::SemanticEpisodePhase,
) -> scorepeek::routine_output::SemanticEpisodePhase {
    use scorepeek::timeline_driver::SemanticEpisodePhase;
    match phase {
        SemanticEpisodePhase::Started => scorepeek::routine_output::SemanticEpisodePhase::Started,
        SemanticEpisodePhase::Suspended => {
            scorepeek::routine_output::SemanticEpisodePhase::Suspended
        }
        SemanticEpisodePhase::Resumed => scorepeek::routine_output::SemanticEpisodePhase::Resumed,
        SemanticEpisodePhase::Closing => scorepeek::routine_output::SemanticEpisodePhase::Closing,
        SemanticEpisodePhase::Finalized => {
            scorepeek::routine_output::SemanticEpisodePhase::Finalized
        }
    }
}

fn publish_replay_semantic(
    output: &mut scorepeek::routine_output::RoutineOutput,
    session_id: &str,
    generation: u64,
    episode: scorepeek::screen_episode::SemanticScreenEpisode,
    sequence: u64,
    monotonic_ms: u64,
    phase: scorepeek::routine_output::SemanticEpisodePhase,
) -> Result<(), CorpusError> {
    output
        .publish(&scorepeek::routine_output::RunEvent {
            schema: "scorepeek-run-event-v8".to_owned(),
            kind: scorepeek::routine_output::RunEventKind::SemanticScreenEpisodeChanged {
                session_id: Some(session_id.to_owned()),
                capture_generation: Some(generation),
                screen_episode_id: episode.id,
                sequence,
                monotonic_end_ms: monotonic_ms,
                screen: replay_screen_name(episode.screen).to_owned(),
                phase,
            },
        })
        .map_err(CorpusError::InvalidReplay)
}

fn drain_replay_pending(
    recognition: &mut scorepeek::recognition_live::field_session::FieldObservationSession<
        scorepeek::recognition_live::screen_field_observer::RegisteredScreenFieldObserver,
    >,
    pending: &mut VecDeque<ReplayPending>,
    output: &mut scorepeek::routine_output::RoutineOutput,
    session_id: &str,
    generation: u64,
    measurements: &mut ReplayMeasurements,
) -> Result<(), CorpusError> {
    while !pending.is_empty() {
        commit_replay_pending(
            recognition,
            pending,
            output,
            true,
            session_id,
            generation,
            measurements,
        )?;
    }
    Ok(())
}

fn commit_replay_pending(
    recognition: &mut scorepeek::recognition_live::field_session::FieldObservationSession<
        scorepeek::recognition_live::screen_field_observer::RegisteredScreenFieldObserver,
    >,
    pending: &mut VecDeque<ReplayPending>,
    output: &mut scorepeek::routine_output::RoutineOutput,
    wait: bool,
    session_id: &str,
    generation: u64,
    measurements: &mut ReplayMeasurements,
) -> Result<(), CorpusError> {
    use scorepeek::recognition_live::field_session::FieldObservationSessionPoll;
    let Some(front) = pending.front() else {
        return Ok(());
    };
    let wait_started = Instant::now();
    let poll = if wait {
        recognition.wait_field_observation(&front.pending, Duration::from_secs(30))
    } else {
        recognition.poll_field_observation(&front.pending)
    };
    if wait {
        measurements.ordered_commit_wait_us = measurements
            .ordered_commit_wait_us
            .saturating_add(u64::try_from(wait_started.elapsed().as_micros()).unwrap_or(u64::MAX));
    }
    let FieldObservationSessionPoll::Ready {
        observation,
        timing,
        screen_episode_id,
        ..
    } = poll
    else {
        return if !wait && matches!(poll, FieldObservationSessionPoll::Pending) {
            Ok(())
        } else {
            invalid_replay("production OCR did not complete in sequence order")
        };
    };
    pending.pop_front();
    let sequence = observation.sequence();
    let monotonic_start_ms = observation.monotonic_start_ms();
    let monotonic_end_ms = observation.monotonic_end_ms();
    let observation = observation
        .into_output()
        .map_err(|error| CorpusError::InvalidReplay(format!("production OCR failed: {error}")))?;
    let processing = observation.processing_timing();
    measurements.field_queue_wait_us = measurements
        .field_queue_wait_us
        .saturating_add(processing.field_queue_wait_us);
    measurements.text_batch_wall_us = measurements
        .text_batch_wall_us
        .saturating_add(processing.text_batch_wall_us);
    measurements.maximum_text_worker_inference_us = measurements
        .maximum_text_worker_inference_us
        .max(processing.maximum_text_worker_inference_us);
    measurements.text_worker_busy_us = measurements
        .text_worker_busy_us
        .saturating_add(processing.text_worker_busy_us);
    measurements.numeric_inference_us = measurements
        .numeric_inference_us
        .saturating_add(processing.numeric_recognition_us.unwrap_or(0));
    measurements.field_join_us = measurements
        .field_join_us
        .saturating_add(processing.join_us);
    measurements.catalog_projection_us = measurements
        .catalog_projection_us
        .saturating_add(processing.catalog_evidence_us);
    measurements.field_frame_wall_us = measurements
        .field_frame_wall_us
        .saturating_add(timing.frame_processing_wall_us);
    output
        .publish(
            &scorepeek::routine_output::RunEvent::from_field_observation(
                session_id,
                generation,
                screen_episode_id,
                sequence,
                monotonic_start_ms,
                monotonic_end_ms,
                &observation,
            )
            .map_err(CorpusError::InvalidReplay)?,
        )
        .map_err(CorpusError::InvalidReplay)
}

fn replay_screen_name(screen: ScreenClass) -> &'static str {
    match screen {
        ScreenClass::Result => "result",
        ScreenClass::MusicSelect => "music_select",
        ScreenClass::ModeSelect => "mode_select",
        ScreenClass::DecideTransition => "decide_transition",
        ScreenClass::Play => "play",
        ScreenClass::Unknown => "unknown",
    }
}

fn for_each_canonical_session_frame(
    store: &Path,
    session: &CaptureSession,
    observe: impl FnMut(&CanonicalTick, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<(), CorpusError> {
    let resolver = SegmentResolver {
        remote: SegmentRemote::from_environment()?,
        local_segment_decodes: Arc::new(AtomicU64::new(0)),
    };
    for_each_canonical_session_frame_with_activity(store, session, None, &resolver, observe)
}

fn for_each_canonical_session_frame_with_activity(
    store: &Path,
    session: &CaptureSession,
    activity: Option<&ReplayDecodeActivity>,
    resolver: &SegmentResolver,
    mut observe: impl FnMut(&CanonicalTick, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<(), CorpusError> {
    let canonical_manifest_object =
        session_object_for_source(store, session, "recognition/canonical-manifest.json")?;
    let (canonical, _) = read_json::<CanonicalRecordingManifest>(&canonical_manifest_object)?;
    if canonical.completeness != "complete" || canonical.dropped_frames != 0 {
        return invalid_replay("canonical session is incomplete");
    }
    let tick_object =
        session_object_for_source(store, session, "recognition/canonical-ticks.ndjson")?;
    let ticks = read_canonical_ticks(&tick_object)?;
    let retained = ticks
        .iter()
        .filter(|tick| tick.disposition == "retained")
        .collect::<Vec<_>>();
    let mut offset = 0usize;
    for segment in &canonical.segments {
        let object = resolver.resolve(store, session, &format!("recognition/{}", segment.path))?;
        let expected = retained
            .get(offset..offset.saturating_add(segment.frames))
            .ok_or_else(|| {
                CorpusError::InvalidReplay(
                    "canonical segment exceeds retained tick index".to_owned(),
                )
            })?;
        let decoded_digest = decode_resolved_canonical_frames(
            &object,
            segment.frames,
            DecodeContext::Replay,
            activity,
            |index, pixels, _| {
                let tick = expected.get(index).ok_or_else(|| {
                    CorpusError::InvalidReplay(
                        "canonical decoded frame exceeds tick index".to_owned(),
                    )
                })?;
                observe(tick, pixels)
            },
        )?;
        if decoded_digest != segment.raw_rgb24_sha256 {
            return invalid_replay("canonical segment decoded pixel digest differs");
        }
        offset = offset.saturating_add(segment.frames);
    }
    if offset != retained.len() {
        return invalid_replay("canonical segment coverage differs");
    }
    Ok(())
}

fn for_each_session_canonical_frame(
    store: &Path,
    session: &CaptureSession,
    mut observe: impl FnMut(u64, Box<[u8]>) -> Result<(), CorpusError> + Send,
) -> Result<(), CorpusError> {
    if session
        .artifacts
        .iter()
        .any(|artifact| artifact.source_path == "recognition/canonical-manifest.json")
    {
        return for_each_canonical_session_frame(store, session, |tick, pixels| {
            observe(tick.sequence, pixels)
        });
    }
    for frame in &session.canonical_frames {
        let encoded = fs::read(store.join("objects").join(&frame.artifact_sha256))?;
        if digest(&encoded) != frame.artifact_sha256 {
            return invalid("canonical session frame digest differs");
        }
        let (header, pixels) = qoi::decode_to_vec(encoded)
            .map_err(|_| CorpusError::InvalidRequest("canonical session QOI is invalid".into()))?;
        if header.width != 1_920 || header.height != 1_080 || pixels.len() != 1_920 * 1_080 * 3 {
            return invalid("canonical session frame is not canonical RGB8");
        }
        observe(frame.sequence, pixels.into_boxed_slice())?;
    }
    Ok(())
}

fn session_object_for_source(
    store: &Path,
    session: &CaptureSession,
    source_path: &str,
) -> Result<PathBuf, CorpusError> {
    let artifact = session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == source_path)
        .ok_or_else(|| {
            CorpusError::InvalidReplay(format!(
                "canonical session artifact is unavailable: {source_path}"
            ))
        })?;
    let object = store.join("objects").join(&artifact.sha256);
    verify_file(&object, &artifact.sha256, artifact.bytes)?;
    Ok(object)
}

impl SegmentResolver {
    fn resolve(
        &self,
        store: &Path,
        session: &CaptureSession,
        source_path: &str,
    ) -> Result<ResolvedSegment, CorpusError> {
        let artifact = session
            .artifacts
            .iter()
            .find(|artifact| artifact.source_path == source_path)
            .ok_or_else(|| {
                CorpusError::InvalidReplay(format!(
                    "canonical session artifact is unavailable: {source_path}"
                ))
            })?;
        let object = store.join("objects").join(&artifact.sha256);
        match object.symlink_metadata() {
            Ok(_) => {
                verify_file(&object, &artifact.sha256, artifact.bytes)?;
                self.local_segment_decodes.fetch_add(1, Ordering::AcqRel);
                Ok(ResolvedSegment::Local(object))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let remote = self.remote.as_ref().ok_or_else(|| {
                    CorpusError::InvalidReplay(
                        "remote remote_not_configured: canonical segment is not local".to_owned(),
                    )
                })?;
                remote
                    .materialize(&artifact.sha256, artifact.bytes)
                    .map(ResolvedSegment::Remote)
                    .map_err(|error| CorpusError::InvalidReplay(error.to_string()))
            }
            Err(error) => Err(error.into()),
        }
    }
}

fn validate_semantic_oracle(
    label: &RegressionLabel,
    emitted: &[scorepeek::routine_output::ResultDomainEvent],
    failures: &mut Vec<String>,
) {
    let accepted = label
        .episodes
        .iter()
        .filter(|episode| {
            episode
                .attempt
                .as_ref()
                .is_some_and(|attempt| matches!(attempt.outcome, AttemptOutcome::Accepted))
        })
        .collect::<Vec<_>>();
    if emitted.len() != accepted.len() {
        failures.push(format!(
            "session {} event count differs: expected {}, observed {}",
            label.session_sha256,
            accepted.len(),
            emitted.len()
        ));
    }
    let mut actual_by_key = BTreeMap::<String, u64>::new();
    for (index, episode) in accepted.iter().enumerate() {
        let attempt = episode.attempt.as_ref().expect("accepted attempt exists");
        let Some(event) = emitted.get(index) else {
            failures.push(format!(
                "attempt {} is missing its ordered result event",
                attempt.attempt_key
            ));
            continue;
        };
        if !result_event_matches(event, episode) {
            failures.push(format!(
                "attempt {} result payload or play-options order differs",
                attempt.attempt_key
            ));
        }
        let expected_parent = attempt
            .parent_attempt_key
            .as_ref()
            .and_then(|key| actual_by_key.get(key))
            .copied();
        if event.parent_attempt_id != expected_parent {
            failures.push(format!(
                "attempt {} parent relation differs",
                attempt.attempt_key
            ));
        }
        actual_by_key.insert(attempt.attempt_key.clone(), event.attempt_id);
    }
}

fn result_event_matches(
    event: &scorepeek::routine_output::ResultDomainEvent,
    episode: &RegressionEpisode,
) -> bool {
    let expected = &episode.expected_result;
    let expected_song = serde_json::from_value::<scorepeek::catalog::ScorepeekSongId>(
        Value::String(episode.expected_song_id.clone()),
    )
    .ok();
    event.contract == "scorepeek-result-detected-v2"
        && Some(event.scorepeek_song_id) == expected_song
        && event.clear_type == episode.expected_clear_type
        && event.play_side == expected.play_side
        && event.play_mode == expected.play_mode
        && event.play_type == expected.play_type
        && event.difficulty == expected.difficulty
        && event.level == expected.level
        && event.notes == expected.notes
        && event.current_score == expected.current_score
        && expected.judgments.as_ref() == Some(&event.judgments)
        && expected.miss_count.as_ref() == Some(&event.miss_count)
        && expected.timing.as_ref() == Some(&event.timing)
        && expected.combo_break.as_ref() == Some(&event.combo_break)
        && expected.previous_best.as_ref() == Some(&event.previous_best)
        && matches!(
            (&event.play_options, expected.play_options.as_deref()),
            (PlayOptions::Known { values }, Some(expected)) if values == expected
        )
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
    if manifest.schema != DIAGNOSTIC_SCHEMA
        || manifest.session_id.is_empty()
        || manifest.capture_generation == 0
        || !valid_sha256(&manifest.profile_sha256)
        || !valid_sha256(&manifest.catalog_sha256)
        || !valid_sha256(&manifest.capture_manifest_sha256)
        || !valid_sha256(&manifest.recognition_manifest_sha256)
        || !valid_sha256(&manifest.event_manifest_sha256)
        || manifest
            .canonical_manifest_sha256
            .as_deref()
            .is_none_or(|digest| !valid_sha256(digest))
        || !matches!(
            manifest.canonical_completeness.as_deref(),
            Some("complete" | "partial")
        )
        || manifest.artifacts.is_empty()
        || manifest.artifacts.len() > MAX_ARTIFACTS
        || manifest.recognition_interval_ms != 100
        || manifest
            .field_observation_busy_skips
            .zip(manifest.maximum_consecutive_field_observation_busy_skips)
            .is_none_or(|(total, maximum)| maximum > total)
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
        || label.schema != LABEL_SCHEMA
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
    let mut attempt_keys = BTreeSet::new();
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
            || !valid_play_options(episode.expected_result.play_options.as_deref())
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
            || episode.attempt.as_ref().is_none_or(|attempt| {
                attempt.attempt_key.is_empty()
                    || !attempt_keys.insert(attempt.attempt_key.clone())
                    || attempt.parent_attempt_key.as_deref() == Some("")
                    || attempt.parent_attempt_key.as_deref() == Some(&attempt.attempt_key)
                    || attempt
                        .parent_attempt_key
                        .as_ref()
                        .is_some_and(|parent| !attempt_keys.contains(parent))
                    || !valid_span(attempt.result_span, &available)
                    || attempt
                        .select_span
                        .is_none_or(|span| !valid_span(span, &available))
                    || attempt
                        .decide_span
                        .is_none_or(|span| !valid_span(span, &available))
                    || attempt
                        .play_span
                        .is_none_or(|span| !valid_span(span, &available))
                    || !spans_are_ordered(attempt)
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

fn spans_are_ordered(attempt: &AttemptTruth) -> bool {
    let (Some(select), Some(decide), Some(play)) =
        (attempt.select_span, attempt.decide_span, attempt.play_span)
    else {
        return false;
    };
    select.last_sequence < decide.first_sequence
        && decide.last_sequence < play.first_sequence
        && play.last_sequence < attempt.result_span.first_sequence
}

fn validate_label_timeline(
    store: &Path,
    draft: &ReviewDraft,
    label: &RegressionLabel,
) -> Result<(), CorpusError> {
    let session_path = store
        .join("sessions")
        .join(format!("{}.json", draft.session_sha256));
    let (session, session_bytes) = read_json::<CaptureSession>(&session_path)?;
    if session.schema != SESSION_SCHEMA
        || digest(&session_bytes) != draft.session_sha256
        || session.completeness != "complete"
    {
        return invalid("review session is not a complete canonical session");
    }
    let ticks = read_canonical_ticks(&session_object_for_source(
        store,
        &session,
        "recognition/canonical-ticks.ndjson",
    )?)?;
    let mut by_sequence = BTreeMap::new();
    for tick in &ticks {
        if by_sequence.insert(tick.sequence, tick).is_some() {
            return invalid("canonical tick index contains duplicate sequences");
        }
    }
    for episode in &label.episodes {
        let attempt = episode
            .attempt
            .as_ref()
            .ok_or_else(|| CorpusError::InvalidRequest("attempt truth is required".to_owned()))?;
        validate_screen_span(
            &by_sequence,
            attempt.select_span.expect("validated select span"),
            ScreenClass::MusicSelect,
            false,
        )?;
        validate_screen_span(
            &by_sequence,
            attempt.decide_span.expect("validated decide span"),
            ScreenClass::DecideTransition,
            true,
        )?;
        validate_screen_span(
            &by_sequence,
            attempt.play_span.expect("validated play span"),
            ScreenClass::Play,
            false,
        )?;
        validate_screen_span(&by_sequence, attempt.result_span, ScreenClass::Result, true)?;
    }
    Ok(())
}

fn validate_screen_span(
    ticks: &BTreeMap<u64, &CanonicalTick>,
    span: SequenceSpan,
    expected_screen: ScreenClass,
    require_complete_interior: bool,
) -> Result<(), CorpusError> {
    for endpoint in [span.first_sequence, span.last_sequence] {
        let tick = ticks
            .get(&endpoint)
            .ok_or_else(|| CorpusError::InvalidRequest("span endpoint is absent".to_owned()))?;
        if tick.disposition != "retained" || tick.screen != expected_screen {
            return invalid("span endpoint is not retained on the expected raw screen");
        }
    }
    if require_complete_interior {
        for sequence in span.first_sequence..=span.last_sequence {
            let tick = ticks.get(&sequence).ok_or_else(|| {
                CorpusError::InvalidRequest("required span contains a missing tick".to_owned())
            })?;
            if tick.disposition != "retained" || tick.screen != expected_screen {
                return invalid("required span contains elision or another raw screen");
            }
        }
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
                | "scorepeek-run-event-v7"
                | "scorepeek-run-event-v8"
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
                | "scorepeek-run-event-v7"
                | "scorepeek-run-event-v8"
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
    let label = serde_json::from_slice::<RegressionLabel>(&bytes)?;
    if label.schema != LABEL_SCHEMA {
        return invalid("regression label schema is unsupported");
    }
    Ok((label, bytes))
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
    use object_store::ObjectStore;
    use object_store::memory::InMemory;
    use std::io::Seek as _;

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
            canonical_manifest_sha256: Some("6".repeat(64)),
            canonical_completeness: Some("complete".to_owned()),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn diagnostic_manifest_requires_v5_canonical_and_field_busy_bindings() {
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

        manifest.schema = "scorepeek-private-diagnostic-session-v4".to_owned();
        manifest.field_observation_busy_skips = None;
        manifest.maximum_consecutive_field_observation_busy_skips = None;
        assert!(validate_diagnostic_manifest(&manifest).is_err());
    }

    #[test]
    fn decimal_video_timestamps_are_converted_without_float_rounding() {
        assert_eq!(parse_timestamp_ms("0.000000").unwrap(), 0);
        assert_eq!(parse_timestamp_ms("12.345678").unwrap(), 12_345);
        assert_eq!(parse_timestamp_ms("1.5").unwrap(), 1_500);
        assert!(parse_timestamp_ms("-0.1").is_err());
    }

    #[test]
    fn canonical_tick_chronology_rejects_cross_segment_sequence_or_time_reset() {
        let tick = CanonicalTick {
            sequence: 11,
            source_sequence: 11,
            monotonic_ms: 1_000,
            screen: ScreenClass::Result,
            semantic_episode_id: Some(1),
            disposition: "retained".to_owned(),
        };
        assert!(canonical_tick_follows(Some((10, 1_000)), &tick));
        assert!(!canonical_tick_follows(Some((11, 900)), &tick));
        assert!(!canonical_tick_follows(Some((12, 1_000)), &tick));
        assert!(!canonical_tick_follows(Some((10, 1_001)), &tick));
    }

    #[test]
    fn replay_decoder_activity_tracks_four_independent_sessions() {
        let activity = Arc::new(ReplayDecodeActivity::default());
        let entered = Arc::new(std::sync::Barrier::new(5));
        let release = Arc::new(std::sync::Barrier::new(5));
        thread::scope(|scope| {
            for _ in 0..4 {
                let activity = Arc::clone(&activity);
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                scope.spawn(move || {
                    let memory = activity.reserve_decoder();
                    let _decoder = activity.enter(std::process::id(), memory);
                    entered.wait();
                    release.wait();
                });
            }
            entered.wait();
            assert_eq!(activity.active.load(Ordering::Acquire), 4);
            assert_eq!(activity.maximum_active.load(Ordering::Acquire), 4);
            assert_eq!(activity.children.load(Ordering::Acquire), 4);
            assert_eq!(
                activity.tracked_peak_bytes.load(Ordering::Acquire),
                u64::try_from(4 * DECODER_RESERVATION_BYTES).unwrap()
            );
            release.wait();
        });
        assert_eq!(activity.active.load(Ordering::Acquire), 0);
        assert_eq!(activity.tracked_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn four_ffmpeg_children_decode_the_same_immutable_segment_concurrently() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("fixture.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=1920x1080:r=10:d=1",
                "-frames:v",
                "10",
                "-c:v",
                "ffv1",
            ])
            .arg(&segment)
            .status()
            .unwrap();
        assert!(status.success());

        let activity = Arc::new(ReplayDecodeActivity::default());
        let decoded = Arc::new(std::sync::Barrier::new(5));
        let release = Arc::new(std::sync::Barrier::new(5));
        thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..4 {
                let activity = Arc::clone(&activity);
                let decoded = Arc::clone(&decoded);
                let release = Arc::clone(&release);
                let segment = segment.clone();
                handles.push(scope.spawn(move || {
                    decode_canonical_frames_with_activity(
                        &segment,
                        10,
                        DecodeContext::Replay,
                        Some(&activity),
                        |index, _| {
                            if index == 0 {
                                decoded.wait();
                                release.wait();
                            }
                            Ok(())
                        },
                    )
                }));
            }
            decoded.wait();
            let pids = activity
                .live_pids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .copied()
                .collect::<Vec<_>>();
            assert_eq!(pids.len(), 4);
            assert!(pids.into_iter().all(|pid| process_rss_bytes(pid).is_some()));
            release.wait();
            for handle in handles {
                handle.join().unwrap().unwrap();
            }
        });
        assert_eq!(activity.children.load(Ordering::Acquire), 4);
        assert_eq!(activity.maximum_active.load(Ordering::Acquire), 4);
        let details = activity
            .decoder_details
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(details.len(), 4);
        assert!(details.iter().all(|detail| detail.rss_peak_bytes > 0));
        assert!(activity.ffmpeg_rss_peak_total_bytes.load(Ordering::Acquire) > 0);
        assert!(activity.process_rss_peak_bytes.load(Ordering::Acquire) > 0);
    }

    #[test]
    fn verified_temporary_segment_decodes_through_ffmpeg_stdin() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("fixture.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=1920x1080:r=10:d=0.1",
                "-frames:v",
                "1",
                "-c:v",
                "ffv1",
            ])
            .arg(&segment)
            .status()
            .unwrap();
        assert!(status.success());
        let digest = decode_canonical_source_with_program_and_timing(
            DecodeSource::File(File::open(segment).unwrap()),
            1,
            DecodeContext::Replay,
            None,
            OsStr::new("ffmpeg"),
            |_, pixels, _| {
                assert_eq!(pixels.len(), 1920 * 1080 * 3);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(digest, crate::digest_bytes(&vec![0_u8; 1920 * 1080 * 3]));
    }

    #[test]
    fn segment_resolver_gets_a_missing_local_object_from_remote() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        ensure_store(&store).unwrap();
        let bytes = b"remote segment";
        let sha256 = digest(bytes);
        let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let remote = SegmentRemote::new(object_store, "test".to_owned()).unwrap();
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(bytes).unwrap();
        source.rewind().unwrap();
        remote
            .upload_verified(source, &sha256, bytes.len() as u64)
            .unwrap();
        let resolver = SegmentResolver {
            remote: Some(remote),
            local_segment_decodes: Arc::new(AtomicU64::new(0)),
        };
        let session = CaptureSession {
            schema: SESSION_SCHEMA.to_owned(),
            diagnostic_sha256: "1".repeat(64),
            source_kind: SourceKind::LiveRun,
            source_session_id: "session".to_owned(),
            capture_generation: 1,
            profile_sha256: "2".repeat(64),
            catalog_sha256: "3".repeat(64),
            recognition_interval_ms: 100,
            processed_ticks: 1,
            busy_skips: 0,
            maximum_consecutive_busy_skips: 0,
            completeness: "complete".to_owned(),
            canonical_frames: Vec::new(),
            normalization_pairs: Vec::new(),
            artifacts: vec![CorpusArtifact {
                kind: "canonical_segment".to_owned(),
                source_path: "recognition/segment-0000.mkv".to_owned(),
                sha256,
                bytes: bytes.len() as u64,
            }],
        };
        let ResolvedSegment::Remote(segment) = resolver
            .resolve(&store, &session, "recognition/segment-0000.mkv")
            .unwrap()
        else {
            panic!("missing local segment must resolve remotely");
        };
        let mut actual = Vec::new();
        segment.input().unwrap().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
        assert!(
            !store
                .join("objects")
                .join(&session.artifacts[0].sha256)
                .exists()
        );
    }

    #[test]
    fn replay_segment_prefetch_resolves_exactly_once() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        ensure_store(&store).unwrap();
        let bytes = b"prefetched remote segment";
        let sha256 = digest(bytes);
        let object_store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let remote = SegmentRemote::new(object_store, "test".to_owned()).unwrap();
        let mut source = tempfile::tempfile().unwrap();
        source.write_all(bytes).unwrap();
        source.rewind().unwrap();
        remote
            .upload_verified(source, &sha256, bytes.len() as u64)
            .unwrap();
        let resolver = SegmentResolver {
            remote: Some(remote.clone()),
            local_segment_decodes: Arc::new(AtomicU64::new(0)),
        };
        let session = CaptureSession {
            schema: SESSION_SCHEMA.to_owned(),
            diagnostic_sha256: "1".repeat(64),
            source_kind: SourceKind::LiveRun,
            source_session_id: "session".to_owned(),
            capture_generation: 1,
            profile_sha256: "2".repeat(64),
            catalog_sha256: "3".repeat(64),
            recognition_interval_ms: 100,
            processed_ticks: 1,
            busy_skips: 0,
            maximum_consecutive_busy_skips: 0,
            completeness: "complete".to_owned(),
            canonical_frames: Vec::new(),
            normalization_pairs: Vec::new(),
            artifacts: vec![CorpusArtifact {
                kind: "canonical_segment".to_owned(),
                source_path: "recognition/segment-0001.mkv".to_owned(),
                sha256,
                bytes: bytes.len() as u64,
            }],
        };
        let prefetched = PrefetchedReplaySegment::start(
            1,
            store,
            session,
            "recognition/segment-0001.mkv".to_owned(),
            resolver,
        );
        assert_eq!(prefetched.segment_index, 1);
        assert!(matches!(
            prefetched.finish().unwrap(),
            ResolvedSegment::Remote(_)
        ));
        assert_eq!(remote.metrics().downloaded_segments, 1);
        assert_eq!(remote.metrics().downloaded_bytes, bytes.len() as u64);
    }

    #[test]
    fn local_segment_resolution_does_not_touch_the_configured_remote() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        ensure_store(&store).unwrap();
        let bytes = b"local segment";
        let sha256 = digest(bytes);
        fs::write(store.join("objects").join(&sha256), bytes).unwrap();
        let remote = SegmentRemote::new(Arc::new(InMemory::new()), "test".to_owned()).unwrap();
        let resolver = SegmentResolver {
            remote: Some(remote.clone()),
            local_segment_decodes: Arc::new(AtomicU64::new(0)),
        };
        let session = CaptureSession {
            schema: SESSION_SCHEMA.to_owned(),
            diagnostic_sha256: "1".repeat(64),
            source_kind: SourceKind::LiveRun,
            source_session_id: "session".to_owned(),
            capture_generation: 1,
            profile_sha256: "2".repeat(64),
            catalog_sha256: "3".repeat(64),
            recognition_interval_ms: 100,
            processed_ticks: 1,
            busy_skips: 0,
            maximum_consecutive_busy_skips: 0,
            completeness: "complete".to_owned(),
            canonical_frames: Vec::new(),
            normalization_pairs: Vec::new(),
            artifacts: vec![CorpusArtifact {
                kind: "canonical_segment".to_owned(),
                source_path: "recognition/segment-0000.mkv".to_owned(),
                sha256: sha256.clone(),
                bytes: bytes.len() as u64,
            }],
        };
        assert!(matches!(
            resolver
                .resolve(&store, &session, "recognition/segment-0000.mkv")
                .unwrap(),
            ResolvedSegment::Local(_)
        ));
        assert_eq!(
            remote.metrics(),
            crate::segment_remote::RemoteMetrics::default()
        );
    }

    #[test]
    fn failed_decoder_spawn_releases_memory_without_counting_a_child() {
        let activity = ReplayDecodeActivity::default();
        let result = decode_canonical_frames_with_program(
            Path::new("/does/not/matter.mkv"),
            1,
            DecodeContext::Replay,
            Some(&activity),
            OsStr::new("/scorepeek/missing-ffmpeg"),
            |_, _| Ok(()),
        );
        assert!(result.is_err());
        assert_eq!(activity.children.load(Ordering::Acquire), 0);
        assert_eq!(activity.active.load(Ordering::Acquire), 0);
        assert_eq!(activity.tracked_bytes.load(Ordering::Acquire), 0);
        assert!(
            activity
                .decoder_details
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
    }

    #[test]
    fn panicking_decoder_consumer_kills_reaps_and_releases_the_child() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("fixture.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=1920x1080:r=10:d=1",
                "-frames:v",
                "10",
                "-c:v",
                "ffv1",
            ])
            .arg(&segment)
            .status()
            .unwrap();
        assert!(status.success());

        let activity = ReplayDecodeActivity::default();
        let result = decode_canonical_frames_with_activity(
            &segment,
            10,
            DecodeContext::Replay,
            Some(&activity),
            |_, _| panic!("consumer failure"),
        );
        assert!(matches!(result, Err(CorpusError::InvalidReplay(_))));
        assert_eq!(activity.active.load(Ordering::Acquire), 0);
        assert_eq!(activity.tracked_bytes.load(Ordering::Acquire), 0);
        assert!(
            activity
                .live_pids
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty()
        );
        assert_eq!(
            activity
                .decoder_details
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1
        );
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
                "{\"schema\":\"scorepeek-run-event-v6\",\"event\":\"recording_health_changed\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"state\":\"active\",\"memory_limit_bytes\":1073741824,\"memory_used_bytes\":6220800,\"memory_high_water_bytes\":6220800,\"dropped_frames\":0,\"channel_sequence\":2}\n",
                "{\"schema\":\"scorepeek-run-event-v6\",\"event\":\"recording_finalizing\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"channel_sequence\":3}\n",
                "{\"schema\":\"scorepeek-run-event-v6\",\"event\":\"session_finished\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"outcome\":\"ok\",\"report\":{},\"channel_sequence\":4}\n",
            ),
        )
        .unwrap();
        assert_eq!(verify_session_events(&path, &manifest).unwrap(), 4);
    }

    #[test]
    fn v8_run_event_stream_accepts_provisional_lifecycle_and_rejects_v9() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("events.ndjson");
        let manifest = diagnostic_manifest();
        fs::write(
            &path,
            concat!(
                "{\"schema\":\"scorepeek-run-event-v8\",\"event\":\"session_started\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"capture_profile_sha256\":\"profile\",\"normalizer_artifact_sha256\":\"normalizer\",\"channel_sequence\":1}\n",
                "{\"schema\":\"scorepeek-run-event-v8\",\"event\":\"result_provisional_changed\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"screen_episode_id\":22,\"source_sequence\":3255,\"revision\":1,\"state\":{\"status\":\"withdrawn\",\"reason\":\"evidence_unresolved\"},\"channel_sequence\":2}\n",
                "{\"schema\":\"scorepeek-run-event-v8\",\"event\":\"session_finished\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"outcome\":\"ok\",\"report\":{},\"channel_sequence\":3}\n",
            ),
        )
        .unwrap();
        assert_eq!(verify_session_events(&path, &manifest).unwrap(), 3);

        fs::write(
            &path,
            concat!(
                "{\"schema\":\"scorepeek-run-event-v9\",\"event\":\"session_started\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"capture_profile_sha256\":\"profile\",\"normalizer_artifact_sha256\":\"normalizer\",\"channel_sequence\":1}\n",
                "{\"schema\":\"scorepeek-run-event-v9\",\"event\":\"session_finished\",\"session_id\":\"run-1-session-1\",\"capture_generation\":1,\"outcome\":\"ok\",\"report\":{},\"channel_sequence\":2}\n",
            ),
        )
        .unwrap();
        assert!(verify_session_events(&path, &manifest).is_err());
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
    fn numeric_dataset_authors_from_segment_backed_canonical_frames() {
        let root = tempfile::tempdir().unwrap();
        let store = root.path().join("store");
        ensure_store(&store).unwrap();

        let expected = expected_result();
        let labels = numeric_field_labels(&expected).unwrap();
        let mut pixels = [200_u8, 100, 20].repeat(1_920 * 1_080);
        for y in [451, 655] {
            for x in 0..518 {
                pixels[(y * 1_920 + x) * 3..][..3].copy_from_slice(&[0, 0, 0]);
            }
        }
        let ScreenRgb8Crops::Result(crops) =
            route_screen_rgb8_crops(&pixels, ScreenClass::Result).unwrap()
        else {
            unreachable!();
        };
        let rois = numeric_crops(&crops, &labels)
            .into_iter()
            .map(|(_, _, crop)| crop.roi)
            .collect::<Vec<_>>();
        for (index, roi) in rois.iter().enumerate() {
            let offset = (roi.y as usize * 1_920 + roi.x as usize) * 3;
            pixels[offset..offset + 3].copy_from_slice(&[
                u8::try_from(index + 1).unwrap(),
                u8::try_from(index + 2).unwrap(),
                u8::try_from(index + 3).unwrap(),
            ]);
        }
        assert_eq!(
            inspect_canonical_rgb8(&pixels).unwrap().screen,
            ScreenClass::Result
        );

        let segment_path = root.path().join("segment.mkv");
        let output = File::create(&segment_path).unwrap();
        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-video_size",
                "1920x1080",
                "-framerate",
                "10",
                "-i",
                "pipe:0",
                "-an",
                "-c:v",
                "libx264rgb",
                "-crf",
                "0",
                "-preset",
                "ultrafast",
                "-frames:v",
                "1",
                "-f",
                "matroska",
                "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::from(output))
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(&pixels).unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let segment_bytes = fs::read(&segment_path).unwrap();
        let segment_sha256 = digest(&segment_bytes);
        fs::write(store.join("objects").join(&segment_sha256), &segment_bytes).unwrap();
        let raw_sha256 = digest(&pixels);
        let tick_bytes = b"{\"sequence\":1,\"source_sequence\":1,\"monotonic_ms\":100,\"screen\":\"result\",\"semantic_episode_id\":1,\"disposition\":\"retained\"}\n";
        let tick_sha256 = digest(tick_bytes);
        fs::write(store.join("objects").join(&tick_sha256), tick_bytes).unwrap();
        let canonical_bytes = canonical_json(&serde_json::json!({
            "schema": "scorepeek-canonical-session-recording-v2",
            "completeness": "complete",
            "ffmpeg_sha256": "1".repeat(64),
            "ffmpeg_version": "test",
            "tick_index_sha256": tick_sha256,
            "tick_count": 1,
            "segments": [{
                "path": "segment-0000.mkv",
                "first_sequence": 1,
                "last_sequence": 1,
                "frames": 1,
                "raw_rgb24_sha256": raw_sha256,
                "encoded_sha256": segment_sha256,
                "bytes": segment_bytes.len(),
            }],
            "dropped_frames": 0,
            "completeness_reasons": [],
            "memory_limit_bytes": 1_073_741_824_u64,
            "memory_high_water_bytes": 6_220_800_u64,
            "integrity_verification": "deferred_to_import",
        }))
        .unwrap();
        let canonical_sha256 = digest(&canonical_bytes);
        let canonical_bytes_len = u64::try_from(canonical_bytes.len()).unwrap();
        fs::write(
            store.join("objects").join(&canonical_sha256),
            canonical_bytes,
        )
        .unwrap();

        let session = CaptureSession {
            schema: SESSION_SCHEMA.to_owned(),
            diagnostic_sha256: "2".repeat(64),
            source_kind: SourceKind::LiveRun,
            source_session_id: "segment-backed".to_owned(),
            capture_generation: 1,
            profile_sha256: "3".repeat(64),
            catalog_sha256: "4".repeat(64),
            recognition_interval_ms: 100,
            processed_ticks: 1,
            busy_skips: 0,
            maximum_consecutive_busy_skips: 0,
            completeness: "complete".to_owned(),
            canonical_frames: vec![ReviewFrame {
                sequence: 1,
                artifact_sha256: segment_sha256.clone(),
            }],
            normalization_pairs: Vec::new(),
            artifacts: vec![
                CorpusArtifact {
                    kind: "canonical_manifest".to_owned(),
                    source_path: "recognition/canonical-manifest.json".to_owned(),
                    sha256: canonical_sha256,
                    bytes: canonical_bytes_len,
                },
                CorpusArtifact {
                    kind: "canonical_ticks".to_owned(),
                    source_path: "recognition/canonical-ticks.ndjson".to_owned(),
                    sha256: tick_sha256,
                    bytes: u64::try_from(tick_bytes.len()).unwrap(),
                },
                CorpusArtifact {
                    kind: "canonical_segment".to_owned(),
                    source_path: "recognition/segment-0000.mkv".to_owned(),
                    sha256: segment_sha256,
                    bytes: u64::try_from(segment_bytes.len()).unwrap(),
                },
            ],
        };
        let session_bytes = canonical_json(&session).unwrap();
        let session_sha256 = digest(&session_bytes);
        publish_document(
            &store
                .join("sessions")
                .join(format!("{session_sha256}.json")),
            &session_bytes,
        )
        .unwrap();
        let label = RegressionLabel {
            schema: LABEL_SCHEMA.to_owned(),
            session_sha256: session_sha256.clone(),
            disposition: LabelDisposition::Include,
            episodes: vec![RegressionEpisode {
                episode_id: "result-1".to_owned(),
                expected_song_id: "00000000-0000-0000-0000-000000000001".to_owned(),
                expected_clear_type: "CLEAR".to_owned(),
                expected_result: expected,
                stable_sequences: vec![1],
                attempt: None,
            }],
            negative_frames: Vec::new(),
        };
        let label_bytes = canonical_json(&label).unwrap();
        let label_sha256 = digest(&label_bytes);
        publish_document(
            &store.join("labels").join(format!("{label_sha256}.json")),
            &label_bytes,
        )
        .unwrap();
        let suite = RegressionSuite {
            schema: SUITE_SCHEMA.to_owned(),
            previous_generation_sha256: None,
            entries: vec![SuiteEntry {
                session_sha256,
                label_sha256,
            }],
        };
        let suite_bytes = canonical_json(&suite).unwrap();
        let suite_sha256 = digest(&suite_bytes);
        publish_document(
            &store.join("suites").join(format!("{suite_sha256}.json")),
            &suite_bytes,
        )
        .unwrap();
        publish_active(&store, &suite_sha256).unwrap();

        let summary = author_numeric_dataset(&store, &root.path().join("dataset")).unwrap();
        assert_eq!(summary.sessions, 1);
        assert_eq!(summary.episodes, 1);
        assert!(summary.samples > 0);
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
    fn v4_labels_are_rejected_by_the_clean_cut_reader() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("label.json");
        let mut value = serde_json::to_value(RegressionLabel {
            schema: "scorepeek-private-session-regression-label-v4".to_owned(),
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
        assert!(read_regression_label(&path).is_err());
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
