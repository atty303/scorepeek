#![allow(clippy::missing_errors_doc, clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
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
use crate::replay_trace::{ReplayTrace, TraceStatus};
use crate::segment_remote::{RemoteSegment, SegmentRemote};

const DIAGNOSTIC_SCHEMA: &str = "scorepeek-private-diagnostic-session-v5";
const SESSION_SCHEMA: &str = "scorepeek-private-capture-session-v3";
const CORPUS_OBSERVATION_SCHEMA: &str = "scorepeek-private-corpus-observation-v1";
const DRAFT_SCHEMA: &str = "scorepeek-private-session-review-draft-v2";
const LABEL_SCHEMA: &str = "scorepeek-private-session-regression-label-v5";
const SUITE_SCHEMA: &str = "scorepeek-private-regression-suite-v1";
const ACTIVE_SCHEMA: &str = "scorepeek-private-regression-suite-active-v1";
const MAX_DOCUMENT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_ARTIFACTS: usize = 20_000;
const MAX_NDJSON_RECORDS: usize = 250_000;
const MAX_CORPUS_OBSERVATION_BYTES: u64 = 512 * 1024 * 1024;
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
const REPLAY_SEGMENT_PREFETCH: usize = 2;
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
    #[serde(default)]
    tick_index_sha256: Option<String>,
    tick_count: usize,
    segments: Vec<CanonicalSegment>,
    dropped_frames: u64,
    completeness_reasons: Vec<String>,
    memory_limit_bytes: u64,
    memory_high_water_bytes: u64,
    #[serde(default)]
    integrity_verification: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct CanonicalRecordingManifestV3 {
    schema: String,
    completeness: String,
    ffmpeg_sha256: String,
    ffmpeg_version: String,
    tick_count: usize,
    segments: Vec<CanonicalSegmentV3>,
    dropped_frames: u64,
    completeness_reasons: Vec<String>,
    memory_limit_bytes: u64,
    memory_high_water_bytes: u64,
}

#[derive(Clone, Debug, Deserialize)]
struct CanonicalSegmentV3 {
    path: String,
    first_sequence: u64,
    last_sequence: u64,
    frames: usize,
    bytes: u64,
}

struct RunSessionDiagnostic {
    run_id: String,
    capture_session_id: String,
    capture_generation: u64,
    profile_sha256: String,
    normalizer_sha256: String,
    catalog_sha256: String,
    canonical_layout_sha256: String,
    model_sha256: String,
    runtime_sha256: String,
    diagnostic_sha256: Option<String>,
    observation_count: u64,
    canonical: CanonicalRecordingManifestV3,
    ticks: Vec<CanonicalTick>,
    segment_digests: Vec<String>,
    observations: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize)]
struct CanonicalSegment {
    path: String,
    first_sequence: u64,
    last_sequence: u64,
    frames: usize,
    #[serde(default)]
    raw_rgb24_sha256: Option<String>,
    #[serde(default)]
    encoded_sha256: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic_sha256: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic_sha256: Option<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_sha256: Option<String>,
    session_id: String,
    artifact_count: usize,
    canonical_frame_count: usize,
    observation_count: u64,
}

#[derive(Debug, Serialize)]
pub struct DiagnosticImportSummary {
    schema: &'static str,
    session_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    diagnostic_sha256: Option<String>,
    review_draft: PathBuf,
    canonical_frame_count: usize,
    local_segment_objects: u64,
    remote_segment_objects: u64,
    remote_transferred_objects: u64,
    remote_reused_objects: u64,
    remote_segment_bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunImportReceipt {
    schema: String,
    store: PathBuf,
    review_draft: PathBuf,
    session_sha256: String,
    canonical_frame_count: usize,
    local_segment_objects: u64,
    remote_segment_objects: u64,
    remote_transferred_objects: u64,
    remote_reused_objects: u64,
    remote_segment_bytes: u64,
    segment_paths: Vec<String>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    trace: Option<Box<TraceStatus>>,
    session_key: String,
    music_select_best_snapshots: usize,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CorpusReplayOptions {
    pub trace_dir: Option<PathBuf>,
    pub text_workers: Option<usize>,
    pub memory_mib: usize,
}

impl Default for CorpusReplayOptions {
    fn default() -> Self {
        Self {
            trace_dir: None,
            text_workers: None,
            memory_mib: DEFAULT_REPLAY_MEMORY_MIB,
        }
    }
}

pub fn verify_diagnostic(path: &Path) -> Result<DiagnosticVerificationSummary, CorpusError> {
    let (manifest, bytes) = read_json::<DiagnosticManifest>(&path.join("manifest.json"))?;
    validate_diagnostic_manifest(&manifest)?;
    let mut seen = BTreeSet::new();
    for artifact in &manifest.artifacts {
        if !seen.insert(&artifact.path) {
            return invalid("diagnostic contains duplicate artifact paths");
        }
        let relative = safe_relative(&artifact.path)?;
        let file = path.join(relative);
        verify_file(&file, &artifact.sha256, artifact.bytes)?;
    }
    verify_canonical_diagnostic(path, manifest, &bytes)
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
        || canonical
            .tick_index_sha256
            .as_deref()
            .is_none_or(|sha256| !valid_sha256(sha256))
        || canonical.segments.len() > MAX_ARTIFACTS
        || !(128 * 1024 * 1024..=16 * 1024 * 1024 * 1024).contains(&canonical.memory_limit_bytes)
        || canonical.memory_high_water_bytes > canonical.memory_limit_bytes
        || canonical.integrity_verification.as_deref() != Some("deferred_to_import")
        || (canonical.completeness == "complete"
            && (canonical.dropped_frames != 0 || !canonical.completeness_reasons.is_empty()))
    {
        return invalid("canonical recording manifest is invalid");
    }
    let tick_artifact = manifest_artifact(&manifest, "recognition/canonical-ticks.ndjson")?;
    let tick_path = path.join("recognition/canonical-ticks.ndjson");
    let tick_index_sha256 = canonical
        .tick_index_sha256
        .as_deref()
        .expect("v2 validation requires a tick digest");
    if tick_artifact.sha256 != tick_index_sha256
        || verify_file(&tick_path, tick_index_sha256, tick_artifact.bytes).is_err()
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
            || segment
                .raw_rgb24_sha256
                .as_deref()
                .is_none_or(|sha256| !valid_sha256(sha256))
            || segment
                .encoded_sha256
                .as_deref()
                .is_none_or(|sha256| !valid_sha256(sha256))
            || segment.bytes == 0
            || segment.bytes > MAX_ARTIFACT_BYTES
        {
            return invalid("canonical segment reference is invalid");
        }
        safe_relative(&segment.path)?;
        let artifact = manifest_artifact(&manifest, &format!("recognition/{}", segment.path))?;
        if Some(artifact.sha256.as_str()) != segment.encoded_sha256.as_deref()
            || artifact.bytes != segment.bytes
        {
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
        if Some(decoded_sha256.as_str()) != segment.raw_rgb24_sha256.as_deref()
            || decoded_frames != segment.frames
        {
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
        diagnostic_sha256: Some(digest(manifest_bytes)),
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

pub fn verify_run_diagnostic(
    run: &Path,
    capture_session_id: &str,
) -> Result<DiagnosticVerificationSummary, CorpusError> {
    let verified = read_run_session_diagnostic(run, capture_session_id)?;
    Ok(DiagnosticVerificationSummary {
        schema: "scorepeek-run-diagnostic-verification-v1",
        diagnostic_sha256: verified.diagnostic_sha256,
        session_id: verified.capture_session_id,
        artifact_count: verified.canonical.segments.len().saturating_add(3),
        canonical_frame_count: verified
            .ticks
            .iter()
            .filter(|tick| tick.disposition == "retained")
            .count(),
        observation_count: verified.observation_count,
    })
}

pub fn import_run_diagnostic(
    store: &Path,
    run: &Path,
    capture_session_id: &str,
    review_draft: &Path,
) -> Result<DiagnosticImportSummary, CorpusError> {
    let remote = SegmentRemote::from_environment()?;
    import_run_diagnostic_with_remote(
        store,
        run,
        capture_session_id,
        review_draft,
        remote.as_ref(),
    )
}

fn import_run_diagnostic_with_remote(
    store: &Path,
    run: &Path,
    capture_session_id: &str,
    review_draft: &Path,
    remote: Option<&SegmentRemote>,
) -> Result<DiagnosticImportSummary, CorpusError> {
    let canonical_root = run
        .join("sessions")
        .join(capture_session_id)
        .join("canonical");
    let receipt_path = canonical_root.join("import-receipt.json");
    if receipt_path.is_file() {
        let (receipt, _) = read_json::<RunImportReceipt>(&receipt_path)?;
        return finish_run_import_cleanup(store, review_draft, &canonical_root, &receipt);
    }
    let verified = read_run_session_diagnostic(run, capture_session_id)?;
    ensure_store(store)?;
    let retained = verified
        .ticks
        .iter()
        .filter(|tick| tick.disposition == "retained")
        .collect::<Vec<_>>();
    let mut frames = Vec::with_capacity(retained.len());
    let mut offset = 0usize;
    let mut artifacts = Vec::new();
    let binding_bytes = canonical_json(&serde_json::json!({
        "schema":"scorepeek-imported-run-binding-v1",
        "binding":{
            "capture_profile_sha256":verified.profile_sha256,
            "normalizer_sha256":verified.normalizer_sha256,
            "canonical_layout_sha256":verified.canonical_layout_sha256,
            "catalog_sha256":verified.catalog_sha256,
            "model_sha256":verified.model_sha256,
            "runtime_sha256":verified.runtime_sha256,
        }
    }))?;
    let binding_sha256 = digest(&binding_bytes);
    publish_object_bytes(store, &binding_sha256, &binding_bytes)?;
    artifacts.push(CorpusArtifact {
        kind: "run_binding".to_owned(),
        source_path: "capture/run.json".to_owned(),
        sha256: binding_sha256,
        bytes: binding_bytes.len() as u64,
    });
    for (kind, source_name, corpus_name) in [
        (
            "canonical_manifest",
            "canonical-manifest.json",
            "recognition/canonical-manifest.json",
        ),
        (
            "canonical_tick_index",
            "canonical-ticks.ndjson",
            "recognition/canonical-ticks.ndjson",
        ),
    ] {
        let source = canonical_root.join(source_name);
        let bytes = source.metadata()?.len();
        if bytes > MAX_ARTIFACT_BYTES {
            return invalid("canonical metadata artifact exceeds the corpus bound");
        }
        let sha256 = digest_file(&source)?;
        publish_object(store, &source, &sha256, bytes)?;
        artifacts.push(CorpusArtifact {
            kind: kind.to_owned(),
            source_path: corpus_name.to_owned(),
            sha256,
            bytes,
        });
    }
    let mut local_segment_objects = 0_u64;
    let mut remote_segment_objects = 0_u64;
    let mut remote_segment_bytes = 0_u64;
    for (segment, encoded_sha256) in verified
        .canonical
        .segments
        .iter()
        .zip(&verified.segment_digests)
    {
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
                artifact_sha256: encoded_sha256.clone(),
            });
        }
        offset = offset.saturating_add(segment.frames);
        let source = canonical_root.join(safe_relative(&segment.path)?);
        if let Some(remote) = remote {
            remote.upload_verified(File::open(&source)?, encoded_sha256, segment.bytes)?;
            remote_segment_objects += 1;
            remote_segment_bytes = remote_segment_bytes.saturating_add(segment.bytes);
        } else {
            publish_object(store, &source, encoded_sha256, segment.bytes)?;
            local_segment_objects += 1;
        }
        artifacts.push(CorpusArtifact {
            kind: "canonical_video_segment".to_owned(),
            source_path: format!("recognition/{}", segment.path),
            sha256: encoded_sha256.clone(),
            bytes: segment.bytes,
        });
    }
    if offset != retained.len() {
        return invalid("canonical retained tick coverage differs");
    }
    if !verified.observations.is_empty() {
        let sha256 = digest(&verified.observations);
        publish_object_bytes(store, &sha256, &verified.observations)?;
        artifacts.push(CorpusArtifact {
            kind: "analysis".to_owned(),
            source_path: "analysis/observations.ndjson".to_owned(),
            sha256,
            bytes: verified.observations.len() as u64,
        });
    }
    let session = CaptureSession {
        schema: SESSION_SCHEMA.to_owned(),
        diagnostic_sha256: verified.diagnostic_sha256.clone(),
        source_kind: SourceKind::LiveRun,
        source_session_id: verified.capture_session_id.clone(),
        capture_generation: verified.capture_generation,
        profile_sha256: verified.profile_sha256,
        catalog_sha256: verified.catalog_sha256,
        recognition_interval_ms: 0,
        processed_ticks: verified.ticks.len() as u64,
        busy_skips: 0,
        maximum_consecutive_busy_skips: 0,
        completeness: "complete".to_owned(),
        canonical_frames: frames.clone(),
        normalization_pairs: Vec::new(),
        artifacts,
    };
    let session_bytes = canonical_json(&session)?;
    let session_sha256 = digest(&session_bytes);
    let identity_key = canonical_json(&serde_json::json!({
        "run_id": verified.run_id,
        "capture_session_id": verified.capture_session_id,
    }))?;
    let identity_sha256 = digest(&identity_key);
    publish_document(
        &store
            .join("identities")
            .join(format!("{identity_sha256}.json")),
        &canonical_json(&SessionIdentity {
            schema: "scorepeek-private-capture-session-identity-v4",
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
        source_session_id: verified.capture_session_id,
        canonical_frames: frames,
        observation_count: verified.observation_count,
        completeness: "complete".to_owned(),
    };
    publish_document(review_draft, &canonical_json(&draft)?)?;
    let receipt = RunImportReceipt {
        schema: "scorepeek-run-import-receipt-v1".to_owned(),
        store: store.to_owned(),
        review_draft: review_draft.to_owned(),
        session_sha256,
        canonical_frame_count: draft.canonical_frames.len(),
        local_segment_objects,
        remote_segment_objects,
        remote_transferred_objects: remote.map_or(0, |remote| remote.metrics().transferred_objects),
        remote_reused_objects: remote.map_or(0, |remote| remote.metrics().reused_objects),
        remote_segment_bytes,
        segment_paths: verified
            .canonical
            .segments
            .iter()
            .map(|segment| segment.path.clone())
            .collect(),
    };
    publish_document(&receipt_path, &canonical_json(&receipt)?)?;
    File::open(&canonical_root)?.sync_all()?;
    finish_run_import_cleanup(store, review_draft, &canonical_root, &receipt)
}

fn finish_run_import_cleanup(
    store: &Path,
    review_draft: &Path,
    canonical_root: &Path,
    receipt: &RunImportReceipt,
) -> Result<DiagnosticImportSummary, CorpusError> {
    let session_path = store
        .join("sessions")
        .join(format!("{}.json", receipt.session_sha256));
    if receipt.schema != "scorepeek-run-import-receipt-v1"
        || receipt.store != store
        || receipt.review_draft != review_draft
        || !valid_sha256(&receipt.session_sha256)
        || !review_draft.is_file()
    {
        return invalid("run import receipt binding is invalid");
    }
    let session_bytes = session_path.metadata()?.len();
    verify_file(&session_path, &receipt.session_sha256, session_bytes)?;
    let (draft, _) = read_json::<ReviewDraft>(review_draft)?;
    if draft.session_sha256 != receipt.session_sha256 || draft.completeness != "complete" {
        return invalid("run import receipt review binding is invalid");
    }
    let mut failures = Vec::new();
    for segment in &receipt.segment_paths {
        let path = canonical_root.join(safe_relative(segment)?);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("{}: {error}", path.display())),
        }
    }
    if let Err(error) = File::open(canonical_root).and_then(|directory| directory.sync_all()) {
        failures.push(format!("{}: {error}", canonical_root.display()));
    }
    if !failures.is_empty() {
        return invalid(&format!(
            "imported video cleanup is incomplete and can be retried: {}",
            failures.join("; ")
        ));
    }
    Ok(DiagnosticImportSummary {
        schema: "scorepeek-private-diagnostic-import-v4",
        session_sha256: receipt.session_sha256.clone(),
        diagnostic_sha256: None,
        review_draft: review_draft.to_owned(),
        canonical_frame_count: receipt.canonical_frame_count,
        local_segment_objects: receipt.local_segment_objects,
        remote_segment_objects: receipt.remote_segment_objects,
        remote_transferred_objects: receipt.remote_transferred_objects,
        remote_reused_objects: receipt.remote_reused_objects,
        remote_segment_bytes: receipt.remote_segment_bytes,
    })
}

fn read_run_session_diagnostic(
    run: &Path,
    capture_session_id: &str,
) -> Result<RunSessionDiagnostic, CorpusError> {
    if capture_session_id.is_empty()
        || capture_session_id.contains('/')
        || capture_session_id.contains("..")
    {
        return invalid("capture session ID is invalid");
    }
    let stream_path = run.join("diagnostics.ndjson");
    let stream_file = File::open(&stream_path)?;
    let mut reader = BufReader::new(stream_file);
    let mut line = Vec::new();
    let mut expected_sequence = 1_u64;
    let mut run_id = None;
    let mut started = None;
    let mut completed = false;
    let mut public_binding = None;
    let mut observation_count = 0_u64;
    let mut observations = Vec::new();
    loop {
        line.clear();
        let read = reader
            .by_ref()
            .take(u64::try_from(MAX_NDJSON_RECORD_BYTES).unwrap_or(u64::MAX) + 1)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        if read > MAX_NDJSON_RECORD_BYTES {
            return invalid("diagnostic NDJSON record exceeds its byte bound");
        }
        if line.last() != Some(&b'\n') {
            break;
        }
        let envelope: Value = serde_json::from_slice(&line)?;
        if envelope["schema"] != "scorepeek-diagnostic-event-v1" {
            return invalid("diagnostic stream schema is invalid");
        }
        if envelope["sequence"].as_u64() != Some(expected_sequence) {
            return invalid("diagnostic stream sequence is not contiguous");
        }
        expected_sequence = expected_sequence.saturating_add(1);
        let envelope_run = envelope["run_id"]
            .as_str()
            .ok_or_else(|| CorpusError::InvalidRequest("diagnostic run ID is absent".to_owned()))?;
        if run_id.as_deref().is_some_and(|value| value != envelope_run) {
            return invalid("diagnostic stream changes run ID");
        }
        run_id.get_or_insert_with(|| envelope_run.to_owned());
        if envelope["operation"] == "public_event"
            && envelope["data"]["public_event"]["capture"]["session_id"].as_str()
                == Some(capture_session_id)
        {
            let binding = &envelope["data"]["public_event"]["capture"]["binding"];
            public_binding = Some((
                binding["capture_profile_sha256"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                binding["normalizer_sha256"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                binding["canonical_layout_sha256"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                binding["catalog_sha256"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                binding["model_sha256"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                binding["runtime_sha256"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            ));
        }
        if envelope["operation"] != "run_event" {
            continue;
        }
        let event = &envelope["data"];
        let matching = event["session_id"].as_str() == Some(capture_session_id);
        match event["event"].as_str() {
            Some("session_started") if matching => {
                started = Some((
                    event["capture_generation"].as_u64().ok_or_else(|| {
                        CorpusError::InvalidRequest("capture generation is absent".to_owned())
                    })?,
                    event["capture_profile_sha256"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    event["normalizer_artifact_sha256"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                ));
            }
            Some("field_observation") if matching => {
                observation_count = observation_count.saturating_add(1);
                observations.extend_from_slice(&normalize_run_observation(event)?);
                if observations.len() as u64 > MAX_CORPUS_OBSERVATION_BYTES {
                    return invalid("diagnostic observation artifact exceeds the corpus bound");
                }
            }
            Some("recording_completed") if matching => completed = true,
            _ => {}
        }
    }
    if !completed {
        return invalid("session has no saved recording_completed terminal record");
    }
    let (capture_generation, profile_sha256, normalizer_sha256) = started.ok_or_else(|| {
        CorpusError::InvalidRequest("session_started record is absent".to_owned())
    })?;
    let (
        public_profile_sha256,
        public_normalizer_sha256,
        canonical_layout_sha256,
        catalog_sha256,
        model_sha256,
        runtime_sha256,
    ) = public_binding.ok_or_else(|| {
        CorpusError::InvalidRequest("session catalog binding is absent".to_owned())
    })?;
    if public_profile_sha256 != profile_sha256
        || public_normalizer_sha256 != normalizer_sha256
        || !valid_sha256(&profile_sha256)
        || !valid_sha256(&normalizer_sha256)
        || !valid_sha256(&canonical_layout_sha256)
        || !valid_sha256(&catalog_sha256)
        || !valid_sha256(&model_sha256)
        || !valid_sha256(&runtime_sha256)
    {
        return invalid("session binding is invalid");
    }
    let canonical_root = run
        .join("sessions")
        .join(capture_session_id)
        .join("canonical");
    let (canonical, _) =
        read_json::<CanonicalRecordingManifestV3>(&canonical_root.join("canonical-manifest.json"))?;
    if canonical.schema != "scorepeek-canonical-session-recording-v3"
        || canonical.completeness != "complete"
        || canonical.dropped_frames != 0
        || !canonical.completeness_reasons.is_empty()
        || !valid_sha256(&canonical.ffmpeg_sha256)
        || canonical.ffmpeg_version.is_empty()
        || canonical.memory_high_water_bytes > canonical.memory_limit_bytes
    {
        return invalid("canonical recording manifest is invalid");
    }
    let ticks = read_canonical_ticks(&canonical_root.join("canonical-ticks.ndjson"))?;
    if ticks.len() != canonical.tick_count || ticks.is_empty() {
        return invalid("canonical tick index count differs");
    }
    let mut segment_digests = Vec::with_capacity(canonical.segments.len());
    let mut previous = None;
    for tick in &ticks {
        if !canonical_tick_follows(previous, tick) {
            return invalid("canonical tick chronology is invalid");
        }
        previous = Some((tick.sequence, tick.monotonic_ms));
    }
    let retained_ticks = ticks
        .iter()
        .filter(|tick| tick.disposition == "retained")
        .collect::<Vec<_>>();
    let retained = retained_ticks.len();
    if retained == 0 || canonical.segments.is_empty() {
        return invalid("canonical recording has no video");
    }
    if canonical
        .segments
        .iter()
        .map(|segment| segment.frames)
        .sum::<usize>()
        != retained
    {
        return invalid("canonical retained tick coverage differs");
    }
    let mut retained_offset = 0usize;
    for segment in &canonical.segments {
        if segment.frames == 0
            || segment.frames > 600
            || segment.last_sequence < segment.first_sequence
        {
            return invalid("canonical segment is invalid");
        }
        let expected = retained_ticks
            .get(retained_offset..retained_offset.saturating_add(segment.frames))
            .ok_or_else(|| {
                CorpusError::InvalidRequest(
                    "canonical segment exceeds retained tick index".to_owned(),
                )
            })?;
        if expected.first().map(|tick| tick.sequence) != Some(segment.first_sequence)
            || expected.last().map(|tick| tick.sequence) != Some(segment.last_sequence)
        {
            return invalid("canonical segment sequence binding differs");
        }
        retained_offset = retained_offset.saturating_add(segment.frames);
        let path = canonical_root.join(safe_relative(&segment.path)?);
        let metadata = path.metadata()?;
        if metadata.len() != segment.bytes {
            return invalid("canonical segment byte count differs");
        }
        let encoded = digest_file(&path)?;
        let (_, decoded_frames) = decode_canonical_segment(&path, segment.frames)?;
        if decoded_frames != segment.frames {
            return invalid("canonical segment decode count differs");
        }
        segment_digests.push(encoded);
    }
    Ok(RunSessionDiagnostic {
        run_id: run_id
            .ok_or_else(|| CorpusError::InvalidRequest("diagnostic stream is empty".to_owned()))?,
        capture_session_id: capture_session_id.to_owned(),
        capture_generation,
        profile_sha256,
        normalizer_sha256,
        catalog_sha256,
        canonical_layout_sha256,
        model_sha256,
        runtime_sha256,
        diagnostic_sha256: None,
        observation_count,
        canonical,
        ticks,
        segment_digests,
        observations,
    })
}

fn normalize_run_observation(event: &Value) -> Result<Vec<u8>, CorpusError> {
    let sequence = event["sequence"].as_u64().ok_or_else(|| {
        CorpusError::InvalidRequest("run observation sequence is invalid".to_owned())
    })?;
    let timestamp_ms = event["monotonic_end_ms"].as_u64().ok_or_else(|| {
        CorpusError::InvalidRequest("run observation timestamp is invalid".to_owned())
    })?;
    let screen = event["screen"].as_str().ok_or_else(|| {
        CorpusError::InvalidRequest("run observation screen is invalid".to_owned())
    })?;
    canonical_json(&serde_json::json!({
        "schema":CORPUS_OBSERVATION_SCHEMA,
        "tick_sequence":sequence,
        "source_timestamp_ms":timestamp_ms,
        "screen":screen,
        "fields":event.get("fields").cloned().unwrap_or(Value::Null),
        "decision":{
            "result_song_resolution":event.get("result_song_resolution").cloned().unwrap_or(Value::Null),
            "music_select_song_resolution":event.get("music_select_song_resolution").cloned().unwrap_or(Value::Null),
            "parsed_result_fields":event.get("parsed_result_fields").cloned().unwrap_or(Value::Null),
            "result_chart_resolution":event.get("result_chart_resolution").cloned().unwrap_or(Value::Null),
            "result_performance_resolution":event.get("result_performance_resolution").cloned().unwrap_or(Value::Null),
        },
        "song_id":event.pointer("/song_resolution_presentation/selected/scorepeek_song_id").cloned().unwrap_or(Value::Null),
    }))
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
                artifact_sha256: segment
                    .encoded_sha256
                    .clone()
                    .expect("verified v2 manifest has an encoded digest"),
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
        if matches!(
            artifact.path.as_str(),
            "recognition/manifest.json" | "events.ndjson" | "event-manifest.json"
        ) {
            continue;
        }
        if artifact.path == "recognition/observations.ndjson" {
            if artifact.bytes > MAX_CORPUS_OBSERVATION_BYTES {
                return invalid("diagnostic observation artifact exceeds the corpus bound");
            }
            let bytes = fs::read(&source)?;
            let (normalized, _) = normalize_observation_stream(&bytes)?;
            let sha256 = digest(&normalized);
            publish_object_bytes(store, &sha256, &normalized)?;
            artifacts.push(CorpusArtifact {
                kind: "analysis".to_owned(),
                source_path: "analysis/observations.ndjson".to_owned(),
                sha256,
                bytes: normalized.len() as u64,
            });
            continue;
        }
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
            schema: "scorepeek-private-capture-session-identity-v3",
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

fn normalize_observation_stream(bytes: &[u8]) -> Result<(Vec<u8>, u64), CorpusError> {
    let mut normalized = Vec::new();
    let mut count = 0_u64;
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if count >= MAX_NDJSON_RECORDS as u64 {
            return invalid("corpus observation count exceeds its bound");
        }
        if line.len() > MAX_NDJSON_RECORD_BYTES || line.last() != Some(&b'\n') {
            return invalid("corpus observation record exceeds its bound");
        }
        let value: Value = serde_json::from_slice(line)?;
        let schema = value["schema"].as_str().ok_or_else(|| {
            CorpusError::InvalidRequest("corpus observation schema is unavailable".into())
        })?;
        if schema != "scorepeek-recognition-observation-v22" {
            return invalid("corpus observation source schema differs");
        }
        let sequence = value["tick_sequence"].as_u64().ok_or_else(|| {
            CorpusError::InvalidRequest("corpus observation sequence is invalid".into())
        })?;
        let timestamp_ms = value["source_timestamp_ms"]
            .as_u64()
            .or_else(|| {
                value
                    .pointer("/timing/source_pts_ms")
                    .and_then(Value::as_u64)
            })
            .or_else(|| {
                value
                    .pointer("/timing/monotonic_end_ms")
                    .and_then(Value::as_u64)
            })
            .ok_or_else(|| {
                CorpusError::InvalidRequest("corpus observation timestamp is invalid".into())
            })?;
        let screen = value["screen"]
            .as_str()
            .or_else(|| value.pointer("/decision/screen").and_then(Value::as_str))
            .or_else(|| value.pointer("/fields/screen").and_then(Value::as_str))
            .ok_or_else(|| {
                CorpusError::InvalidRequest("corpus observation screen is invalid".into())
            })?;
        let record = serde_json::json!({
            "schema": CORPUS_OBSERVATION_SCHEMA,
            "tick_sequence": sequence,
            "source_timestamp_ms": timestamp_ms,
            "screen": screen,
            "fields": value.get("fields").cloned().unwrap_or(Value::Null),
            "decision": value.get("decision").cloned().unwrap_or(Value::Null),
            "song_id": value.get("song_id").cloned().unwrap_or(Value::Null),
        });
        normalized.extend_from_slice(&canonical_json(&record)?);
        if normalized.len() as u64 > MAX_CORPUS_OBSERVATION_BYTES {
            return invalid("normalized corpus observations exceed their byte bound");
        }
        count = count.saturating_add(1);
    }
    if count == 0 {
        return invalid("corpus observation stream is empty");
    }
    Ok((normalized, count))
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
                let Some(expected_play_options) = expected.play_options.as_deref() else {
                    return invalid_replay("validated result label lacks play option truth");
                };
                if !expected_play_options_match(&fields.play_options.parsed, expected_play_options)
                {
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
                            || !play_mode_matches_type(
                                &expected.play_mode,
                                expected.play_type,
                            )
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
    trace: Option<Box<TraceStatus>>,
    session_key: String,
    music_select_best_snapshots: usize,
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
    let trace = options
        .trace_dir
        .map(|path| Arc::new(Mutex::new(ReplayTrace::new(path, &generation_sha256))));
    let mut finalizer_handles = Vec::with_capacity(decode_workers);
    for _ in 0..decode_workers {
        let trace = trace.clone();
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
                    finalize_replay_session(runtime, trace.as_ref())
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
            trace: outcome.trace,
            session_key: outcome.session_key,
            music_select_best_snapshots: outcome.music_select_best_snapshots,
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
            schema: scorepeek::routine_output::RUN_EVENT_SCHEMA.to_owned(),
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
    if let Some(expected) = &segment.raw_rgb24_sha256
        && decoded_digest != *expected
    {
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
    for episode in &runtime.label.episodes {
        if episode
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.play_span)
            .is_some_and(|span| {
                tick.sequence == span.first_sequence || tick.sequence == span.last_sequence
            })
            && screen != ScreenClass::Play
        {
            runtime.failures.push(format!(
                "episode {} PLAY endpoint {} classified as {screen:?}",
                episode.episode_id, tick.sequence,
            ));
        }
    }
    let timeline_step = runtime
        .timeline
        .observe(screen.into(), tick.sequence, tick.monotonic_ms);
    runtime
        .output
        .publish(&scorepeek::routine_output::RunEvent {
            schema: scorepeek::routine_output::RUN_EVENT_SCHEMA.to_owned(),
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
    trace: Option<&Arc<Mutex<ReplayTrace>>>,
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
            schema: scorepeek::routine_output::RUN_EVENT_SCHEMA.to_owned(),
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
    let events = runtime.output.take_headless_events();
    let trace = trace.map(|trace| {
        Box::new({
            trace
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .write_session(runtime.index, &runtime.session_id, &events)
        })
    });
    validate_music_selection_oracle(&runtime.label, &events, &mut runtime.failures);
    let music_select_best_snapshots = events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                scorepeek::routine_output::RunEventKind::MusicSelectBestObserved { .. }
            )
        })
        .count();
    let emitted = events
        .into_iter()
        .filter_map(|event| match event.kind {
            scorepeek::routine_output::RunEventKind::ResultChanged {
                state: scorepeek::routine_output::ResultState::Confirmed { result, .. },
                ..
            } => Some(*result),
            _ => None,
        })
        .collect::<Vec<_>>();
    validate_semantic_oracle(&runtime.label, &emitted, &mut runtime.failures);
    Ok(ReplaySessionOutcome {
        trace,
        session_key: runtime.session_id.clone(),
        music_select_best_snapshots,
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
            schema: scorepeek::routine_output::RUN_EVENT_SCHEMA.to_owned(),
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
        if let Some(expected) = &segment.raw_rgb24_sha256
            && decoded_digest != *expected
        {
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

fn validate_music_selection_oracle(
    label: &RegressionLabel,
    events: &[scorepeek::routine_output::RunEvent],
    failures: &mut Vec<String>,
) {
    use scorepeek::routine_output::{MusicSelectionState, RunEventKind};
    for episode in &label.episodes {
        let Some(span) = episode
            .attempt
            .as_ref()
            .and_then(|attempt| attempt.select_span)
        else {
            continue;
        };
        let latest = events
            .iter()
            .filter_map(|event| match &event.kind {
                RunEventKind::MusicSelectionChanged {
                    source_sequence,
                    state,
                    ..
                } if *source_sequence <= span.last_sequence => Some(state),
                _ => None,
            })
            .next_back();
        let expected = &episode.expected_result;
        let matches = match latest {
            Some(MusicSelectionState::Selected {
                scorepeek_song_id,
                play_type,
                difficulty,
                level,
                notes,
                ..
            }) => {
                serde_json::to_value(scorepeek_song_id).ok()
                    == Some(Value::String(episode.expected_song_id.clone()))
                    && *play_type == expected.play_type
                    && *difficulty == expected.difficulty
                    && *level == expected.level
                    && *notes == expected.notes
            }
            _ => false,
        };
        if !matches {
            failures.push(format!(
                "episode {} SELECT state differs at sequence {}: {latest:?}",
                episode.episode_id, span.last_sequence,
            ));
        }
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

fn expected_play_options_match(observed: &PlayOptions, expected: &[PlayOption]) -> bool {
    matches!(observed, PlayOptions::Known { values } if values == expected)
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
            || !play_mode_matches_type(
                &episode.expected_result.play_mode,
                episode.expected_result.play_type,
            )
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

fn play_mode_matches_type(play_mode: &str, play_type: PlayType) -> bool {
    matches!(
        (play_mode, play_type),
        ("single_play", PlayType::Single) | ("double_play", PlayType::Double)
    )
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
        let calibrating_unknown_play =
            expected_screen == ScreenClass::Play && tick.screen == ScreenClass::Unknown;
        if tick.disposition != "retained"
            || (tick.screen != expected_screen && !calibrating_unknown_play)
        {
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

fn publish_object_bytes(store: &Path, sha256: &str, bytes: &[u8]) -> Result<(), CorpusError> {
    let destination = store.join("objects").join(sha256);
    if destination.exists() {
        return verify_file(&destination, sha256, bytes.len() as u64);
    }
    let staging = store.join("objects").join(format!(".{sha256}.staging"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staging)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    verify_file(&staging, sha256, bytes.len() as u64)?;
    fs::rename(staging, destination)?;
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

pub(crate) fn digest_file(path: &Path) -> Result<String, CorpusError> {
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

pub(crate) fn digest(bytes: &[u8]) -> String {
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

    #[test]
    fn run_import_requires_the_selected_sessions_saved_completion_record() {
        let root = tempfile::tempdir().unwrap();
        let run_id = "run-1-0-1";
        let session_id = "run-1-0-1-session-1";
        let event = serde_json::json!({
            "schema":"scorepeek-diagnostic-event-v1", "run_id":run_id, "sequence":1,
            "observed_unix_us":1, "operation":"run_event", "data":{
                "schema":"scorepeek-run-event-v12", "event":"session_started",
                "session_id":session_id, "capture_generation":1,
                "capture_profile_sha256":"1".repeat(64),
                "normalizer_artifact_sha256":"2".repeat(64)
            }
        });
        std::fs::write(
            root.path().join("diagnostics.ndjson"),
            format!("{}\n", serde_json::to_string(&event).unwrap()),
        )
        .unwrap();
        let error = verify_run_diagnostic(root.path(), session_id).unwrap_err();
        assert!(error.to_string().contains("recording_completed"));
    }

    #[test]
    fn run_import_rejects_a_complete_manifest_without_video() {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("run-1-0-1");
        let session_id = "run-1-0-1-session-1";
        let canonical = run.join("sessions").join(session_id).join("canonical");
        fs::create_dir_all(&canonical).unwrap();
        fs::write(
            canonical.join("canonical-ticks.ndjson"),
            b"{\"sequence\":1,\"source_sequence\":1,\"monotonic_ms\":100,\"screen\":\"unknown\",\"semantic_episode_id\":null,\"disposition\":\"elided\"}\n",
        )
        .unwrap();
        fs::write(
            canonical.join("canonical-manifest.json"),
            canonical_json(&serde_json::json!({
                "schema":"scorepeek-canonical-session-recording-v3",
                "completeness":"complete",
                "ffmpeg_sha256":"4".repeat(64),
                "ffmpeg_version":"test",
                "tick_count":1,
                "segments":[],
                "dropped_frames":0,
                "completeness_reasons":[],
                "memory_limit_bytes":1_073_741_824_u64,
                "memory_high_water_bytes":0
            }))
            .unwrap(),
        )
        .unwrap();
        let records = [
            serde_json::json!({
                "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
                "sequence":1, "observed_unix_us":1, "operation":"run_event", "data":{
                    "schema":"scorepeek-run-event-v12", "event":"session_started",
                    "session_id":session_id, "capture_generation":1,
                    "capture_profile_sha256":"1".repeat(64),
                    "normalizer_artifact_sha256":"2".repeat(64)
                }
            }),
            serde_json::json!({
                "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
                "sequence":2, "observed_unix_us":2, "operation":"public_event", "data":{
                    "public_event":{"capture":{"session_id":session_id,"binding":{
                        "capture_profile_sha256":"1".repeat(64),
                        "normalizer_sha256":"2".repeat(64),
                        "canonical_layout_sha256":"5".repeat(64),
                        "catalog_sha256":"3".repeat(64),
                        "model_sha256":"6".repeat(64),
                        "runtime_sha256":"7".repeat(64)
                    }}}
                }
            }),
            serde_json::json!({
                "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
                "sequence":3, "observed_unix_us":3, "operation":"run_event", "data":{
                    "schema":"scorepeek-run-event-v12", "event":"recording_completed",
                    "session_id":session_id, "directory":canonical.parent().unwrap()
                }
            }),
        ];
        let mut stream = Vec::new();
        for record in records {
            serde_json::to_writer(&mut stream, &record).unwrap();
            stream.push(b'\n');
        }
        fs::write(run.join("diagnostics.ndjson"), stream).unwrap();

        let error = verify_run_diagnostic(&run, session_id).unwrap_err();
        assert!(error.to_string().contains("no video"));
    }

    #[test]
    fn run_import_preserves_video_and_replay_metadata_before_releasing_local_video() {
        let root = tempfile::tempdir().unwrap();
        let run = root.path().join("run-1-0-1");
        let session_id = "run-1-0-1-session-1";
        let canonical = run.join("sessions").join(session_id).join("canonical");
        fs::create_dir_all(&canonical).unwrap();
        let segment = canonical.join("segment-0000.mkv");
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
            .stdout(Stdio::from(File::create(&segment).unwrap()))
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&vec![0; 1_920 * 1_080 * 3])
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let segment_bytes = segment.metadata().unwrap().len();
        fs::write(
            canonical.join("canonical-ticks.ndjson"),
            b"{\"sequence\":1,\"source_sequence\":1,\"monotonic_ms\":100,\"screen\":\"result\",\"semantic_episode_id\":1,\"disposition\":\"retained\"}\n",
        )
        .unwrap();
        fs::write(
            canonical.join("canonical-manifest.json"),
            canonical_json(&serde_json::json!({
                "schema":"scorepeek-canonical-session-recording-v3",
                "completeness":"complete",
                "ffmpeg_sha256":"4".repeat(64),
                "ffmpeg_version":"test",
                "tick_count":1,
                "segments":[{
                    "path":"segment-0000.mkv", "first_sequence":1,
                    "last_sequence":1, "frames":1, "bytes":segment_bytes
                }],
                "dropped_frames":0,
                "completeness_reasons":[],
                "memory_limit_bytes":1_073_741_824_u64,
                "memory_high_water_bytes":6_220_800_u64
            }))
            .unwrap(),
        )
        .unwrap();
        let records = [
            serde_json::json!({
                "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
                "sequence":1, "observed_unix_us":1, "operation":"run_event", "data":{
                    "schema":"scorepeek-run-event-v12", "event":"session_started",
                    "session_id":session_id, "capture_generation":1,
                    "capture_profile_sha256":"1".repeat(64),
                    "normalizer_artifact_sha256":"2".repeat(64)
                }
            }),
            serde_json::json!({
                "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
                "sequence":2, "observed_unix_us":2, "operation":"public_event", "data":{
                    "public_event":{"capture":{"session_id":session_id,"binding":{
                        "capture_profile_sha256":"1".repeat(64),
                        "normalizer_sha256":"2".repeat(64),
                        "canonical_layout_sha256":"5".repeat(64),
                        "catalog_sha256":"3".repeat(64),
                        "model_sha256":"6".repeat(64),
                        "runtime_sha256":"7".repeat(64)
                    }}}
                }
            }),
            serde_json::json!({
                "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
                "sequence":3, "observed_unix_us":3, "operation":"run_event", "data":{
                    "schema":"scorepeek-run-event-v12", "event":"field_observation",
                    "session_id":session_id, "capture_generation":1, "sequence":1,
                    "monotonic_start_ms":90, "monotonic_end_ms":100, "screen":"result",
                    "fields":{"screen":"result"},
                    "result_song_resolution":{"status":"accepted"},
                    "music_select_song_resolution":{"status":"unknown"},
                    "song_resolution_presentation":{"status":"unknown","selected":null}
                }
            }),
            serde_json::json!({
                "schema":"scorepeek-diagnostic-event-v1", "run_id":"run-1-0-1",
                "sequence":4, "observed_unix_us":4, "operation":"run_event", "data":{
                    "schema":"scorepeek-run-event-v12", "event":"recording_completed",
                    "session_id":session_id, "directory":canonical.parent().unwrap()
                }
            }),
        ];
        let mut stream = Vec::new();
        for record in records {
            serde_json::to_writer(&mut stream, &record).unwrap();
            stream.push(b'\n');
        }
        fs::write(run.join("diagnostics.ndjson"), stream).unwrap();

        let store = root.path().join("store");
        let draft = root.path().join("review.json");
        let verification = verify_run_diagnostic(&run, session_id).unwrap();
        assert!(
            serde_json::to_value(verification)
                .unwrap()
                .get("diagnostic_sha256")
                .is_none()
        );
        let summary =
            import_run_diagnostic_with_remote(&store, &run, session_id, &draft, None).unwrap();
        assert!(
            serde_json::to_value(&summary)
                .unwrap()
                .get("diagnostic_sha256")
                .is_none()
        );
        let (session, _) = read_json::<CaptureSession>(
            &store
                .join("sessions")
                .join(format!("{}.json", summary.session_sha256)),
        )
        .unwrap();
        assert!(
            session
                .artifacts
                .iter()
                .any(|artifact| { artifact.source_path == "recognition/canonical-manifest.json" })
        );
        assert!(
            session
                .artifacts
                .iter()
                .any(|artifact| { artifact.source_path == "recognition/canonical-ticks.ndjson" })
        );
        assert!(
            session
                .artifacts
                .iter()
                .any(|artifact| { artifact.source_path == "recognition/segment-0000.mkv" })
        );
        assert!(
            session
                .artifacts
                .iter()
                .any(|artifact| { artifact.source_path == "capture/run.json" })
        );
        let binding = session_binding(&store, &session).unwrap();
        assert_eq!(binding.capture_profile_sha256, "1".repeat(64));
        assert_eq!(binding.normalizer_sha256, "2".repeat(64));
        let (manifest, _) = read_json::<CanonicalRecordingManifest>(
            &session_object_for_source(&store, &session, "recognition/canonical-manifest.json")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.schema, "scorepeek-canonical-session-recording-v3");
        assert!(manifest.segments[0].raw_rgb24_sha256.is_none());
        assert!(
            session_object_for_source(&store, &session, "recognition/segment-0000.mkv")
                .unwrap()
                .is_file()
        );
        let analysis = session
            .artifacts
            .iter()
            .find(|artifact| artifact.source_path == "analysis/observations.ndjson")
            .unwrap();
        let analysis: Value = serde_json::from_slice(
            &fs::read(store.join("objects").join(&analysis.sha256)).unwrap(),
        )
        .unwrap();
        assert_eq!(analysis["schema"], CORPUS_OBSERVATION_SCHEMA);
        assert_eq!(analysis["tick_sequence"], 1);
        assert!(analysis.get("event").is_none());
        assert!(!segment.exists());
        assert!(canonical.join("import-receipt.json").is_file());
        fs::write(&segment, b"leftover already transferred segment").unwrap();
        let repeated =
            import_run_diagnostic_with_remote(&store, &run, session_id, &draft, None).unwrap();
        assert_eq!(repeated.session_sha256, summary.session_sha256);
        assert!(!segment.exists());
        assert!(run.join("diagnostics.ndjson").exists());
    }

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
            diagnostic_sha256: Some("1".repeat(64)),
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
            diagnostic_sha256: Some("1".repeat(64)),
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
            diagnostic_sha256: Some("1".repeat(64)),
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
        for row in pixels.chunks_exact_mut(1_920 * 3) {
            row[1_320 * 3..].fill(0);
        }
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
            diagnostic_sha256: Some("2".repeat(64)),
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
            &expected,
        ));
        assert!(!expected_play_options_match(
            &PlayOptions::Known {
                values: vec![PlayOption::Legacy, PlayOption::Random],
            },
            &expected,
        ));
        assert!(!expected_play_options_match(
            &PlayOptions::Unknown {
                reason: scorepeek::recognition::PlayOptionsUnknownReason::Unrecognized,
            },
            &expected,
        ));
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
            diagnostic_sha256: Some("2".repeat(64)),
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
            diagnostic_sha256: Some("2".repeat(64)),
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

    #[test]
    fn play_mode_truth_requires_matching_sp_or_dp() {
        assert!(play_mode_matches_type("single_play", PlayType::Single));
        assert!(play_mode_matches_type("double_play", PlayType::Double));
        assert!(!play_mode_matches_type("single_play", PlayType::Double));
        assert!(!play_mode_matches_type("double_play", PlayType::Single));
        assert!(!play_mode_matches_type("unknown", PlayType::Single));
    }

    #[test]
    fn only_retained_unknown_play_endpoints_allow_layout_calibration() {
        let tick = CanonicalTick {
            sequence: 1,
            source_sequence: 1,
            monotonic_ms: 100,
            screen: ScreenClass::Unknown,
            semantic_episode_id: None,
            disposition: "retained".to_owned(),
        };
        let ticks = BTreeMap::from([(1, &tick)]);
        let span = SequenceSpan {
            first_sequence: 1,
            last_sequence: 1,
        };
        assert!(validate_screen_span(&ticks, span, ScreenClass::Play, false).is_ok());
        for screen in [
            ScreenClass::MusicSelect,
            ScreenClass::DecideTransition,
            ScreenClass::Result,
        ] {
            assert!(validate_screen_span(&ticks, span, screen, false).is_err());
        }
        let elided = CanonicalTick {
            disposition: "elided".to_owned(),
            ..tick
        };
        assert!(
            validate_screen_span(
                &BTreeMap::from([(1, &elided)]),
                span,
                ScreenClass::Play,
                false
            )
            .is_err()
        );
    }
}
