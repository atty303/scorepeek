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

use scorepeek_core::replay::GameVersionState;
use scorepeek_core::replay::{Difficulty, PlayType};
use scorepeek_core::replay::{
    NumericField, PlayOption, PlayOptions, PreviousBest, PreviousBestValue, ResultChartResolution,
    ResultJudgments, ResultPanelSide, ResultPerformanceResolution, ResultTiming, Rgb8Crop,
    ScreenClass, ScreenRgb8Crops, SupplementalResultValue, inspect_canonical_rgb8,
    resolve_clear_type, route_screen_rgb8_crops,
};
use scorepeek_runtime::replay::{
    ReplayFieldPoll, ReplayFieldSubmission, ReplayPendingObservation, ReplayPreparedFrame,
    ReplayRecognitionSession, ReplaySharedResources as RuntimeReplaySharedResources,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::CorpusError;
use crate::ingest::remote::{RemoteSegment, SegmentRemote};
use crate::replay::runner::{ReplayTrace, TraceStatus};

const DIAGNOSTIC_SCHEMA: &str = "scorepeek-private-diagnostic-session-v5";
const SESSION_SCHEMA: &str = "scorepeek-private-capture-session-v4";
const CORPUS_OBSERVATION_SCHEMA: &str = "scorepeek-private-corpus-observation-v1";
const DRAFT_SCHEMA: &str = "scorepeek-private-session-review-draft-v2";
const LABEL_SCHEMA: &str = "scorepeek-private-session-regression-label-v6";
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
const SESSION_STATE_RESERVATION_BYTES: usize =
    64 * 1024 * 1024 + scorepeek_runtime::diagnostics::inspect::ISOLATED_RING_BYTES;
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
struct CanonicalRecordingManifestV4 {
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
    game_version: GameVersionState,
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
    canonical: CanonicalRecordingManifestV4,
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
    game_version: GameVersionState,
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
    roi: scorepeek_core::replay::Roi,
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
    roi: scorepeek_core::replay::Roi,
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

type SharedReplayFields = RuntimeReplaySharedResources;

struct ReplaySharedResources {
    by_catalog: Mutex<BTreeMap<String, Arc<SharedReplayFields>>>,
    catalog_root: PathBuf,
    bundle: PathBuf,
}

impl ReplaySharedResources {
    fn new(
        catalog_sha256: String,
        resources: Arc<SharedReplayFields>,
        catalog_root: &Path,
        bundle: &Path,
    ) -> Self {
        Self {
            by_catalog: Mutex::new(BTreeMap::from([(catalog_sha256, resources)])),
            catalog_root: catalog_root.to_owned(),
            bundle: bundle.to_owned(),
        }
    }

    fn for_session(
        &self,
        session_index: usize,
        session: &CaptureSession,
        binding: &SessionBinding,
    ) -> Result<Arc<SharedReplayFields>, CorpusError> {
        let mut by_catalog = self
            .by_catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(resources) = by_catalog.get(&session.catalog_sha256) {
            return Ok(Arc::clone(resources));
        }
        let shared = by_catalog
            .values()
            .next()
            .expect("replay shared resources are bootstrapped");
        let descriptor = replay_descriptor(session_index, session, binding);
        let resources = SharedReplayFields::load_sharing_text_pool(
            &descriptor,
            &self.catalog_root,
            &self.bundle,
            shared,
        )
        .map_err(|error| {
            CorpusError::InvalidReplay(format!(
                "shared production recognizer could not load catalog {}: {error}",
                session.catalog_sha256
            ))
        })?;
        let resources = Arc::new(resources);
        by_catalog.insert(session.catalog_sha256.clone(), Arc::clone(&resources));
        Ok(resources)
    }
}

struct ReplayStepContext<'a> {
    store: &'a Path,
    diagnostic_root: &'a Path,
    shared: &'a Arc<ReplaySharedResources>,
    decode_activity: &'a Arc<ReplayDecodeActivity>,
    preprocess_pool: &'a ReplayPreprocessPool,
    outstanding_limit: usize,
    segment_resolver: &'a SegmentResolver,
    trace: Option<&'a Arc<Mutex<ReplayTrace>>>,
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

mod decoder;
use decoder::{
    DecodeContext, canonical_tick_follows, decode_canonical_segment,
    decode_resolved_canonical_frames,
};
#[cfg(test)]
use decoder::{
    DecodeSource, decode_canonical_frames_with_activity, decode_canonical_frames_with_program,
    decode_canonical_source_with_program_and_timing,
};

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
        game_version: verified.canonical.game_version.clone(),
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
        read_json::<CanonicalRecordingManifestV4>(&canonical_root.join("canonical-manifest.json"))?;
    if canonical.schema != "scorepeek-canonical-session-recording-v4"
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
        game_version: GameVersionState::NotObserved,
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
        game_version: GameVersionState::NotObserved,
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
            let predicate = inspect_canonical_rgb8(&pixels).map_err(|_| {
                CorpusError::InvalidRequest("numeric dataset screen predicate failed".into())
            })?;
            if predicate.screen != ScreenClass::Result {
                return invalid("numeric dataset frame is not a canonical result frame");
            }
            let ScreenRgb8Crops::Result(crops) = route_screen_rgb8_crops(
                &pixels,
                predicate.crop_route().ok_or_else(|| {
                    CorpusError::InvalidRequest("numeric dataset result panel is unknown".into())
                })?,
            )
            .map_err(|_| {
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
        dictionary: scorepeek_core::replay::NUMERIC_DICTIONARY,
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
    let predicate = inspect_canonical_rgb8(&pixels).map_err(|_| {
        CorpusError::InvalidRequest("numeric sentinel screen predicate failed".into())
    })?;
    if header.width != 1_920
        || header.height != 1_080
        || pixels.len() != 1_920 * 1_080 * 3
        || predicate.screen != ScreenClass::Result
    {
        return invalid("numeric sentinel frame is not a canonical result frame");
    }
    let ScreenRgb8Crops::Result(crops) = route_screen_rgb8_crops(
        &pixels,
        predicate.crop_route().ok_or_else(|| {
            CorpusError::InvalidRequest("numeric sentinel result panel is unknown".into())
        })?,
    )
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
        dictionary: scorepeek_core::replay::NUMERIC_DICTIONARY,
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
    crops: &'a scorepeek_core::replay::ResultScreenRgb8Crops,
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

mod execution;
use execution::{
    ReplayDecodeActivity, ReplayPreprocessPool, SessionBinding, for_each_session_canonical_frame,
    normalize_observation_stream, replay_descriptor, session_object_for_source,
};
#[cfg(test)]
use execution::{
    ReplayEventOutput, ReplayEventStream, episode_expects_result_event,
    episode_requires_clear_type, expected_play_options_match, optional_previous_matches,
    optional_supplemental_matches, parse_timestamp_ms, process_rss_bytes, session_binding,
    start_replay_observer,
};
pub use execution::{replay_corpus, replay_corpus_with_options};

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
            || !valid_expected_play_side(&episode.expected_result.play_side)
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

fn valid_expected_play_side(play_side: &str) -> bool {
    matches!(play_side, "one_player" | "two_player")
}

fn expected_play_side_matches(panel_side: ResultPanelSide, play_side: &str) -> bool {
    matches!(
        (panel_side, play_side),
        (ResultPanelSide::Left, "one_player") | (ResultPanelSide::Right, "two_player")
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
#[path = "oracle/tests.rs"]
mod tests;
