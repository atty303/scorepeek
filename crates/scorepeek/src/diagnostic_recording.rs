use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{BufWriter, Write as _};
use std::os::unix::fs::DirBuilderExt as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use scorepeek::capture::{UncalibratedMemoryType, UncalibratedVideoContract};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::diagnostic_control::DiagnosticStoreLease;
use crate::publish_private_file;

pub const DEFAULT_SAMPLE_INTERVAL_MS: u64 = 100;
pub const DEFAULT_AGGREGATE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const NORMAL_RETENTION_HOURS: u32 = 24;
pub const PRIORITY_RETENTION_HOURS: u32 = 7 * 24;

const CANONICAL_WIDTH: u32 = 1_920;
const CANONICAL_HEIGHT: u32 = 1_080;
pub(crate) const CANONICAL_BYTES: usize = CANONICAL_WIDTH as usize * CANONICAL_HEIGHT as usize * 3;
const MAX_SOURCE_FRAME_BYTES: usize = 128 * 1024 * 1024;
pub(crate) const MANIFEST_RESERVE_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_FRAMES_PER_RUN: usize = 8_192;
pub(crate) const MAX_FACTS_PER_RUN: usize = 250_000;
pub(crate) const MAX_FACT_BYTES: usize = 64 * 1024;
pub(crate) const MAX_DEGRADATIONS_PER_RUN: usize = 4_096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRunStatus {
    Success,
    Error,
    Cancel,
    Timeout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCompleteness {
    Complete,
    Partial,
    Dropped,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticErrorType {
    InvalidConfiguration,
    StoreUnavailable,
    SequenceNonmonotonic,
    TimingNonmonotonic,
    CaptureSequenceGap,
    CapacityExceeded,
    FrameLimitExceeded,
    FactLimitExceeded,
    EncodeFailed,
    WriteFailed,
    FinalizeFailed,
    QueueFull,
    WorkerUnavailable,
    FlushTimeout,
    FieldObserverOutstandingLimit,
    FieldObserverQueueFull,
    FieldObserverUnavailable,
    FieldObserverFinishTimeout,
    FieldObservationAbandoned,
}

impl DiagnosticErrorType {
    pub(crate) const ALL: [Self; 19] = [
        Self::InvalidConfiguration,
        Self::StoreUnavailable,
        Self::SequenceNonmonotonic,
        Self::TimingNonmonotonic,
        Self::CaptureSequenceGap,
        Self::CapacityExceeded,
        Self::FrameLimitExceeded,
        Self::FactLimitExceeded,
        Self::EncodeFailed,
        Self::WriteFailed,
        Self::FinalizeFailed,
        Self::QueueFull,
        Self::WorkerUnavailable,
        Self::FlushTimeout,
        Self::FieldObserverOutstandingLimit,
        Self::FieldObserverQueueFull,
        Self::FieldObserverUnavailable,
        Self::FieldObserverFinishTimeout,
        Self::FieldObservationAbandoned,
    ];
    pub(crate) const COUNT: usize = Self::ALL.len();

    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticRecordOutcome {
    Recorded,
    SkippedCadence,
    Disabled,
    Dropped(DiagnosticErrorType),
}

pub enum DiagnosticExternalDegradation {
    Drop(DiagnosticErrorType, u64),
    SequenceGap(u64, u64),
}

#[derive(Clone, Debug)]
pub struct DiagnosticPolicy {
    pub enabled: bool,
    pub sample_interval_ms: u64,
    pub maximum_run_bytes: u64,
    pub retention: DiagnosticRetention,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRetention {
    CompleteCadence,
    ForegroundFailureWindowV1,
    FactsOnly,
}

impl Default for DiagnosticPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            sample_interval_ms: DEFAULT_SAMPLE_INTERVAL_MS,
            maximum_run_bytes: DEFAULT_AGGREGATE_BYTES,
            retention: DiagnosticRetention::CompleteCadence,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticResource {
    pub program: &'static str,
    pub version: &'static str,
    pub build_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticBinding {
    pub capture_generation: u64,
    pub capture_profile_sha256: String,
    pub normalizer_sha256: String,
    pub canonical_layout_sha256: String,
    pub catalog_sha256: String,
    pub model_sha256: String,
    pub runtime_sha256: String,
    pub replay: Option<DiagnosticReplayBinding>,
}

impl DiagnosticBinding {
    /// Returns the stable identity of the immutable inputs owned by one diagnostic run.
    ///
    /// Invalid bindings have no identity and are rejected before a live recognition session
    /// starts or changes binding.
    #[must_use]
    pub fn identity_sha256(&self) -> Option<String> {
        if !valid_binding(self) {
            return None;
        }
        canonical_json(&DiagnosticBindingIdentity {
            schema: "scorepeek-diagnostic-binding-identity-v1",
            binding: self,
        })
        .ok()
        .map(|bytes| encode_sha256(&bytes))
    }
}

#[derive(Serialize)]
struct DiagnosticBindingIdentity<'a> {
    schema: &'static str,
    binding: &'a DiagnosticBinding,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticReplayBinding {
    pub request_sha256: String,
    pub extraction_sha256: String,
}

#[derive(Clone, Debug)]
pub struct DiagnosticRunDescriptor {
    pub run_id: String,
    pub monotonic_start_ms: u64,
    pub resource: DiagnosticResource,
    pub binding: DiagnosticBinding,
}

impl DiagnosticRunDescriptor {
    #[must_use]
    pub fn is_valid(&self) -> bool {
        valid_descriptor(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOperation {
    CaptureFrame,
    NormalizeFrame,
    SampleRecognition,
    InspectRecognition,
    ObserveFields,
    ReduceSongContext,
    DeliverEvent,
    ChangeBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOperationStatus {
    Success,
    Error,
    Cancel,
    Timeout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticFactErrorType {
    CaptureUnavailable,
    NormalizeFailed,
    RecognitionFailed,
    FieldObservationFailed,
    SelectionConflict,
    EventDeliveryFailed,
    ConsumerUnavailable,
    OperationTimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticScreen {
    Unknown,
    Title,
    MusicSelection,
    ModeSelection,
    DecideTransition,
    Gameplay,
    Result,
    ConfirmedNonState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticContextChange {
    Replaced,
    Preserved,
    Cleared,
    AlreadyEmpty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticDecisionDomain {
    MusicSelection,
    Result,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticDecisionOutcome {
    Accepted,
    Unknown,
    Suppressed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticTextField {
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
    MusicSelectCentralTitle,
    MusicSelectArtist,
    MusicSelectActiveListTitle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticEventKind {
    MusicSelectDetected,
    ResultDetected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticEventOutcome {
    Emitted,
    Suppressed,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameFieldStatus {
    Completed,
    BusySkip,
    NotApplicable,
    Failed,
    LateEpisode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecognitionSamplingSummary {
    pub processed_ticks: u64,
    pub busy_skips: u64,
    pub maximum_consecutive_busy_skips: u64,
    pub field_observation_busy_skips: u64,
    pub maximum_consecutive_field_observation_busy_skips: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiagnosticDetail {
    Operation,
    SamplingSummary {
        processed_ticks: u64,
        busy_skips: u64,
        maximum_consecutive_busy_skips: u64,
        field_observation_busy_skips: u64,
        maximum_consecutive_field_observation_busy_skips: u64,
    },
    RecognitionBusySkip,
    FieldObservationBusySkip {
        screen: DiagnosticScreen,
    },
    FrameProcessingTiming {
        screen: DiagnosticScreen,
        screen_classification_us: u64,
        crop_prepare_us: Option<u64>,
        field_queue_wait_us: Option<u64>,
        text_batch_wall_us: Option<u64>,
        maximum_text_worker_queue_wait_us: Option<u64>,
        maximum_text_worker_inference_us: Option<u64>,
        text_worker_busy_us: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_worker_ids: Option<Vec<usize>>,
        numeric_ocr_us: Option<u64>,
        field_join_us: Option<u64>,
        catalog_evidence_us: Option<u64>,
        screen_resolver_us: Option<u64>,
        attempt_resolver_us: Option<u64>,
        output_us: Option<u64>,
        frame_processing_wall_us: u64,
        field_status: FrameFieldStatus,
    },
    ScreenObservation {
        screen: DiagnosticScreen,
    },
    ScreenPredicateObservation {
        screen: DiagnosticScreen,
        screen_path_layout_sha256: String,
        result_warm_pixels: u32,
        result_warm_pixels_min: u32,
        result_upper_panel_edge_pixels: u32,
        result_lower_panel_edge_pixels: u32,
        result_horizontal_edge_pixels_min: u32,
        music_select_cyan_header_pixels: u32,
        music_select_cyan_header_pixels_min: u32,
        music_select_colored_level_pixels: u32,
        music_select_colored_level_pixels_min: u32,
        music_select_bright_label_pixels: u32,
        music_select_bright_label_pixels_min: u32,
        music_select_reference_evaluated: bool,
        music_select_music_reference_score_ppm: u32,
        music_select_mode_reference_score_ppm: u32,
        music_select_reference_score_min_ppm: u32,
        music_select_reference_winner_margin_min_ppm: u32,
        decide_transition_cyan_pixels: u32,
        decide_transition_cyan_pixels_min: u32,
        decide_transition_bright_pixels: u32,
        decide_transition_bright_pixels_min: u32,
        decide_transition_saturated_pixels: u32,
        decide_transition_saturated_pixels_min: u32,
        play_cyan_lane_edge_pixels: u32,
        play_cyan_lane_edge_pixels_min: u32,
        play_warm_header_pixels: u32,
        play_warm_header_pixels_min: u32,
    },
    FieldObservation {
        screen: DiagnosticScreen,
        observed_fields: u8,
        unimplemented_fields: u8,
        failed_field: Option<DiagnosticTextField>,
    },
    SongContextObservation {
        change: DiagnosticContextChange,
        candidate_set_sha256: Option<String>,
    },
    SongDecision {
        domain: DiagnosticDecisionDomain,
        outcome: DiagnosticDecisionOutcome,
        song_id: Option<String>,
    },
    EventDelivery {
        event: DiagnosticEventKind,
        outcome: DiagnosticEventOutcome,
    },
    BindingChange {
        next_binding_sha256: String,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticFact {
    #[serde(rename = "tick_sequence")]
    pub sequence: u64,
    pub monotonic_start_ms: u64,
    pub monotonic_end_ms: u64,
    pub operation: DiagnosticOperation,
    pub status: DiagnosticOperationStatus,
    pub error_type: Option<DiagnosticFactErrorType>,
    pub detail: DiagnosticDetail,
}

#[derive(Clone, Copy)]
pub struct DiagnosticFrameInput<'a> {
    pub sequence: u64,
    pub monotonic_start_ms: u64,
    pub monotonic_end_ms: u64,
    pub pixels: &'a [u8],
    pub source: Option<DiagnosticSourceFrameInput<'a>>,
}

#[derive(Clone, Copy)]
pub struct DiagnosticSourceFrameInput<'a> {
    pub source_sequence: u64,
    pub contract: UncalibratedVideoContract,
    pub memory_type: UncalibratedMemoryType,
    pub stride: u32,
    pub received_monotonic_ns: u64,
    pub bytes: &'a [u8],
}

pub enum DiagnosticRecorder {
    Disabled,
    Degraded(DiagnosticDegradation),
    Active(Box<ActiveDiagnosticRecorder>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticDegradation {
    pub error_type: DiagnosticErrorType,
}

pub struct ActiveDiagnosticRecorder {
    directory: PathBuf,
    store_lease: DiagnosticCapacityLease,
    policy: DiagnosticPolicy,
    frames: Vec<DiagnosticFrameArtifact>,
    facts: Option<BufWriter<File>>,
    facts_hasher: Sha256,
    facts_bytes: u64,
    facts_count: u64,
    facts_first_sequence: Option<u64>,
    facts_last_sequence: Option<u64>,
    degradations: Vec<DiagnosticDegradationArtifact>,
    degradation_entries_dropped: u64,
    degradation_reason_counts: [u64; DiagnosticErrorType::COUNT],
    bytes: u64,
    dropped_count: u64,
    last_error_type: Option<DiagnosticErrorType>,
    start: DiagnosticStartArtifact,
    run_monotonic_start_ms: u64,
    last_offered_sequence: Option<u64>,
    last_offered_monotonic_ms: Option<u64>,
    last_offered_monotonic_end_ms: Option<u64>,
    last_recorded_monotonic_ms: Option<u64>,
    maximum_artifact_end_ms: Option<u64>,
    maximum_frame_coverage_end_ms: Option<u64>,
    maximum_observation_gap_ms: Option<u64>,
}

enum DiagnosticCapacityLease {
    Store(DiagnosticStoreLease),
    Isolated {
        managed_bytes: u64,
        maximum_bytes: u64,
    },
}

impl DiagnosticCapacityLease {
    fn reserve(&mut self, additional_bytes: u64) -> Result<(), DiagnosticErrorType> {
        match self {
            Self::Store(lease) => lease.reserve(additional_bytes),
            Self::Isolated {
                managed_bytes,
                maximum_bytes,
            } => {
                let total = managed_bytes
                    .checked_add(additional_bytes)
                    .ok_or(DiagnosticErrorType::CapacityExceeded)?;
                if total > *maximum_bytes {
                    return Err(DiagnosticErrorType::CapacityExceeded);
                }
                *managed_bytes = total;
                Ok(())
            }
        }
    }

    fn release(&mut self, bytes: u64) {
        match self {
            Self::Store(lease) => lease.release(bytes),
            Self::Isolated { managed_bytes, .. } => {
                *managed_bytes = managed_bytes
                    .checked_sub(bytes)
                    .expect("only a successful reservation can be released");
            }
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct DiagnosticFinishOutcome {
    pub completeness: Option<DiagnosticCompleteness>,
    pub error_type: Option<DiagnosticErrorType>,
    pub manifest_sha256: Option<String>,
}

#[derive(Serialize)]
struct DiagnosticRunStart<'a> {
    schema: &'static str,
    run_id: &'a str,
    monotonic_start_ms: u64,
    resource: &'a DiagnosticResource,
    binding: &'a DiagnosticBinding,
    policy: DiagnosticPolicyArtifact,
}

#[derive(Serialize)]
struct DiagnosticPolicyArtifact {
    sample_interval_ms: u64,
    maximum_run_bytes: u64,
    aggregate_retention_bytes: u64,
    normal_retention_hours: u32,
    priority_retention_hours: u32,
    remote_export_enabled: bool,
    retention: DiagnosticRetention,
}

#[derive(Serialize)]
struct DiagnosticStartArtifact {
    schema: &'static str,
    filename: &'static str,
    file_sha256: String,
    bytes: u64,
}

#[derive(Serialize)]
struct DiagnosticFactArtifactDocument<'a> {
    schema: &'static str,
    fact: &'a DiagnosticFact,
}

#[derive(Serialize)]
struct DiagnosticRunManifest<'a> {
    schema: &'static str,
    monotonic_end_ms: u64,
    status: DiagnosticRunStatus,
    completeness: DiagnosticCompleteness,
    dropped_count: u64,
    last_error_type: Option<DiagnosticErrorType>,
    maximum_observation_gap_ms: Option<u64>,
    result_miss_denominator_eligible: bool,
    artifact_bytes: u64,
    manifest_bytes: u64,
    total_bytes: u64,
    start: &'a DiagnosticStartArtifact,
    frames: &'a [DiagnosticFrameArtifact],
    facts: Option<DiagnosticNdjsonArtifact>,
    degradations: &'a [DiagnosticDegradationArtifact],
    degradation_entries_dropped: u64,
    degradation_reason_counts: &'a [DiagnosticDegradationReasonCount],
}

#[derive(Serialize)]
struct DiagnosticDegradationArtifact {
    reason: DiagnosticErrorType,
    affected_sequence: Option<u64>,
    first_missing_sequence: Option<u64>,
    last_missing_sequence: Option<u64>,
    known_missing_count: u64,
}

#[derive(Serialize)]
struct DiagnosticDegradationReasonCount {
    reason: DiagnosticErrorType,
    count: u64,
}

#[derive(Serialize)]
struct DiagnosticFrameArtifact {
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    filename: String,
    canonical_pixel_sha256: String,
    file_sha256: String,
    bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<DiagnosticSourceFrameArtifact>,
}

#[derive(Serialize)]
struct DiagnosticSourceFrameArtifact {
    filename: String,
    source_sequence: u64,
    observed_pixel_format: &'static str,
    encoded_pixel_format: &'static str,
    video: UncalibratedVideoContract,
    memory_type: UncalibratedMemoryType,
    stride: u32,
    received_monotonic_ns: u64,
    file_sha256: String,
    bytes: u64,
}

fn encode_source_qoi(source: DiagnosticSourceFrameInput<'_>) -> Result<Vec<u8>, ()> {
    let width = usize::try_from(source.contract.width).map_err(|_| ())?;
    let height = usize::try_from(source.contract.height).map_err(|_| ())?;
    let stride = usize::try_from(source.stride).map_err(|_| ())?;
    let rgb_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or(())?;
    let mut rgb = Vec::with_capacity(rgb_bytes);
    for row in source.bytes.chunks_exact(stride).take(height) {
        for pixel in row[..width.checked_mul(4).ok_or(())?].chunks_exact(4) {
            rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
        }
    }
    if rgb.len() != rgb_bytes {
        return Err(());
    }
    qoi::encode_to_vec(&rgb, source.contract.width, source.contract.height).map_err(|_| ())
}

#[derive(Serialize)]
struct DiagnosticNdjsonArtifact {
    filename: &'static str,
    record_count: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    file_sha256: String,
    bytes: u64,
}

impl DiagnosticRecorder {
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn start(
        root: &Path,
        descriptor: &DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Self {
        let directory_name = descriptor.run_id.clone();
        Self::start_inner(root, &directory_name, descriptor, policy, true)
    }

    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub(crate) fn start_named(
        root: &Path,
        directory_name: &str,
        descriptor: &DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Self {
        Self::start_inner(root, directory_name, descriptor, policy, false)
    }

    #[allow(clippy::too_many_lines)]
    fn start_inner(
        root: &Path,
        directory_name: &str,
        descriptor: &DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        managed_store: bool,
    ) -> Self {
        if !policy.enabled {
            return Self::Disabled;
        }
        if !valid_policy(&policy)
            || !valid_descriptor(descriptor)
            || !valid_run_directory_name(directory_name)
        {
            return Self::Degraded(DiagnosticDegradation {
                error_type: DiagnosticErrorType::InvalidConfiguration,
            });
        }
        let root_metadata = match root.metadata() {
            Ok(metadata) if root.is_absolute() && metadata.is_dir() => metadata,
            _ => {
                return Self::Degraded(DiagnosticDegradation {
                    error_type: DiagnosticErrorType::StoreUnavailable,
                });
            }
        };
        let _ = root_metadata;
        let start = DiagnosticRunStart {
            schema: "scorepeek-private-diagnostic-capture-start-v4",
            run_id: &descriptor.run_id,
            monotonic_start_ms: descriptor.monotonic_start_ms,
            resource: &descriptor.resource,
            binding: &descriptor.binding,
            policy: DiagnosticPolicyArtifact::from(&policy),
        };
        let Ok(start_bytes) = canonical_json(&start) else {
            return Self::Degraded(DiagnosticDegradation {
                error_type: DiagnosticErrorType::InvalidConfiguration,
            });
        };
        let store_lease = if managed_store {
            match DiagnosticStoreLease::acquire_for_run(
                root,
                &descriptor.run_id,
                start_bytes.len() as u64,
            ) {
                Ok(lease) => DiagnosticCapacityLease::Store(lease),
                Err(error_type) => {
                    return Self::Degraded(DiagnosticDegradation { error_type });
                }
            }
        } else {
            DiagnosticCapacityLease::Isolated {
                managed_bytes: start_bytes.len() as u64,
                maximum_bytes: policy.maximum_run_bytes,
            }
        };
        let directory = root.join(directory_name);
        let mut builder = DirBuilder::new();
        builder.mode(0o700);
        if builder.create(&directory).is_err() {
            return Self::Degraded(DiagnosticDegradation {
                error_type: DiagnosticErrorType::StoreUnavailable,
            });
        }
        if File::open(root).and_then(|root| root.sync_all()).is_err() {
            let _ = fs::remove_dir(&directory);
            let _ = File::open(root).and_then(|root| root.sync_all());
            return Self::Degraded(DiagnosticDegradation {
                error_type: DiagnosticErrorType::StoreUnavailable,
            });
        }
        if publish_private_file(&directory.join("run.json"), &start_bytes).is_err() {
            let _ = fs::remove_dir(&directory);
            let _ = File::open(root).and_then(|root| root.sync_all());
            return Self::Degraded(DiagnosticDegradation {
                error_type: DiagnosticErrorType::WriteFailed,
            });
        }
        let facts_path = directory.join("facts.ndjson");
        let Ok(facts) = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&facts_path)
        else {
            let _ = fs::remove_file(directory.join("run.json"));
            let _ = fs::remove_dir(&directory);
            let _ = File::open(root).and_then(|root| root.sync_all());
            return Self::Degraded(DiagnosticDegradation {
                error_type: DiagnosticErrorType::WriteFailed,
            });
        };
        if File::open(&directory)
            .and_then(|directory| directory.sync_all())
            .is_err()
        {
            let _ = fs::remove_file(facts_path);
            let _ = fs::remove_file(directory.join("run.json"));
            let _ = fs::remove_dir(&directory);
            let _ = File::open(root).and_then(|root| root.sync_all());
            return Self::Degraded(DiagnosticDegradation {
                error_type: DiagnosticErrorType::WriteFailed,
            });
        }
        Self::Active(Box::new(ActiveDiagnosticRecorder {
            directory,
            store_lease,
            policy,
            frames: Vec::new(),
            facts: Some(BufWriter::new(facts)),
            facts_hasher: Sha256::new(),
            facts_bytes: 0,
            facts_count: 0,
            facts_first_sequence: None,
            facts_last_sequence: None,
            degradations: Vec::new(),
            degradation_entries_dropped: 0,
            degradation_reason_counts: [0; DiagnosticErrorType::COUNT],
            bytes: start_bytes.len() as u64,
            dropped_count: 0,
            last_error_type: None,
            start: DiagnosticStartArtifact {
                schema: "scorepeek-private-diagnostic-artifact-v1",
                filename: "run.json",
                file_sha256: encode_sha256(&start_bytes),
                bytes: start_bytes.len() as u64,
            },
            run_monotonic_start_ms: descriptor.monotonic_start_ms,
            last_offered_sequence: None,
            last_offered_monotonic_ms: None,
            last_offered_monotonic_end_ms: None,
            last_recorded_monotonic_ms: None,
            maximum_artifact_end_ms: None,
            maximum_frame_coverage_end_ms: None,
            maximum_observation_gap_ms: None,
        }))
    }

    pub fn record_frame(&mut self, frame: DiagnosticFrameInput<'_>) -> DiagnosticRecordOutcome {
        match self {
            Self::Disabled => DiagnosticRecordOutcome::Disabled,
            Self::Degraded(degradation) => DiagnosticRecordOutcome::Dropped(degradation.error_type),
            Self::Active(recorder) => recorder.record_frame(frame, true),
        }
    }

    pub fn record_sampled_frame(
        &mut self,
        frame: DiagnosticFrameInput<'_>,
    ) -> DiagnosticRecordOutcome {
        match self {
            Self::Disabled => DiagnosticRecordOutcome::Disabled,
            Self::Degraded(degradation) => DiagnosticRecordOutcome::Dropped(degradation.error_type),
            Self::Active(recorder) => recorder.record_frame(frame, false),
        }
    }

    pub fn record_fact(&mut self, fact: &DiagnosticFact) -> DiagnosticRecordOutcome {
        match self {
            Self::Disabled => DiagnosticRecordOutcome::Disabled,
            Self::Degraded(degradation) => DiagnosticRecordOutcome::Dropped(degradation.error_type),
            Self::Active(recorder) => recorder.record_fact(fact),
        }
    }

    pub fn record_external_degradations(
        &mut self,
        degradations: &[DiagnosticExternalDegradation],
        unbound_drops: &[(DiagnosticErrorType, u64, u64)],
        last_error_type: Option<DiagnosticErrorType>,
    ) {
        let Self::Active(recorder) = self else {
            return;
        };
        for degradation in degradations {
            match *degradation {
                DiagnosticExternalDegradation::Drop(error_type, sequence) => {
                    recorder.mark_drop_for_sequence(error_type, Some(sequence));
                }
                DiagnosticExternalDegradation::SequenceGap(first, last) => {
                    recorder.mark_sequence_gap(first, last);
                }
            }
        }
        for &(reason, count, omitted_entries) in unbound_drops {
            recorder.mark_unbound_drops(reason, count, omitted_entries);
        }
        if let Some(error_type) = last_error_type {
            recorder.last_error_type = Some(error_type);
        }
    }

    #[must_use]
    pub fn finish(
        self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
    ) -> DiagnosticFinishOutcome {
        match self {
            Self::Disabled => DiagnosticFinishOutcome {
                completeness: None,
                error_type: None,
                manifest_sha256: None,
            },
            Self::Degraded(degradation) => DiagnosticFinishOutcome {
                completeness: Some(DiagnosticCompleteness::Dropped),
                error_type: Some(degradation.error_type),
                manifest_sha256: None,
            },
            Self::Active(recorder) => recorder.finish(status, monotonic_end_ms, None),
        }
    }

    #[must_use]
    pub fn finish_cancellable(
        self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
        cancellation: &AtomicBool,
    ) -> DiagnosticFinishOutcome {
        match self {
            Self::Disabled => DiagnosticFinishOutcome {
                completeness: None,
                error_type: None,
                manifest_sha256: None,
            },
            Self::Degraded(degradation) => DiagnosticFinishOutcome {
                completeness: Some(DiagnosticCompleteness::Dropped),
                error_type: Some(degradation.error_type),
                manifest_sha256: None,
            },
            Self::Active(recorder) => recorder.finish(status, monotonic_end_ms, Some(cancellation)),
        }
    }
}

impl ActiveDiagnosticRecorder {
    #[allow(clippy::too_many_lines)]
    fn record_frame(
        &mut self,
        frame: DiagnosticFrameInput<'_>,
        detect_sequence_gaps: bool,
    ) -> DiagnosticRecordOutcome {
        if frame.pixels.len() != CANONICAL_BYTES
            || frame.monotonic_end_ms < frame.monotonic_start_ms
            || frame.monotonic_start_ms < self.run_monotonic_start_ms
        {
            return self.drop(DiagnosticErrorType::InvalidConfiguration, frame.sequence);
        }
        if frame.source.is_some_and(|source| {
            let minimum_stride = source.contract.width.checked_mul(4);
            let expected_bytes = usize::try_from(source.stride)
                .ok()
                .and_then(|stride| stride.checked_mul(source.contract.height as usize));
            source.contract.width == 0
                || source.contract.height == 0
                || minimum_stride.is_none_or(|minimum| source.stride < minimum)
                || expected_bytes != Some(source.bytes.len())
                || source.bytes.len() > MAX_SOURCE_FRAME_BYTES
        }) {
            return self.drop(DiagnosticErrorType::InvalidConfiguration, frame.sequence);
        }
        if self
            .last_offered_sequence
            .is_some_and(|previous| frame.sequence <= previous)
        {
            return self.drop(DiagnosticErrorType::SequenceNonmonotonic, frame.sequence);
        }
        if detect_sequence_gaps
            && self
                .last_offered_sequence
                .is_some_and(|previous| frame.sequence > previous + 1)
        {
            let previous = self.last_offered_sequence.expect("checked as present");
            self.mark_sequence_gap(previous + 1, frame.sequence - 1);
        }
        if self
            .last_offered_monotonic_ms
            .is_some_and(|previous| frame.monotonic_start_ms <= previous)
            || self
                .last_offered_monotonic_end_ms
                .is_some_and(|previous| frame.monotonic_end_ms < previous)
        {
            return self.drop(DiagnosticErrorType::TimingNonmonotonic, frame.sequence);
        }
        self.last_offered_sequence = Some(frame.sequence);
        self.last_offered_monotonic_ms = Some(frame.monotonic_start_ms);
        self.last_offered_monotonic_end_ms = Some(frame.monotonic_end_ms);
        if self.last_recorded_monotonic_ms.is_some_and(|previous| {
            frame.monotonic_start_ms.saturating_sub(previous) < self.policy.sample_interval_ms
        }) {
            return DiagnosticRecordOutcome::SkippedCadence;
        }
        if self.frames.len() >= MAX_FRAMES_PER_RUN {
            return self.drop(DiagnosticErrorType::FrameLimitExceeded, frame.sequence);
        }
        let Ok(encoded) = qoi::encode_to_vec(frame.pixels, CANONICAL_WIDTH, CANONICAL_HEIGHT)
        else {
            return self.drop(DiagnosticErrorType::EncodeFailed, frame.sequence);
        };
        let encoded_bytes = encoded.len() as u64;
        let encoded_source = match frame.source {
            Some(source) => match encode_source_qoi(source) {
                Ok(encoded) => Some(encoded),
                Err(()) => return self.drop(DiagnosticErrorType::EncodeFailed, frame.sequence),
            },
            None => None,
        };
        let source_bytes = encoded_source
            .as_ref()
            .map_or(0, |source| source.len() as u64);
        let reserved_bytes = encoded_bytes.saturating_add(source_bytes);
        if self
            .bytes
            .checked_add(reserved_bytes)
            .and_then(|bytes| bytes.checked_add(MANIFEST_RESERVE_BYTES))
            .is_none_or(|bytes| bytes > self.policy.maximum_run_bytes)
        {
            return self.drop(DiagnosticErrorType::CapacityExceeded, frame.sequence);
        }
        if let Err(error_type) = self.store_lease.reserve(reserved_bytes) {
            return self.drop(error_type, frame.sequence);
        }
        let filename = format!("frame-{:020}.qoi", frame.sequence);
        if !self.publish_reserved(&filename, &encoded) {
            self.store_lease.release(source_bytes);
            return self.drop(DiagnosticErrorType::WriteFailed, frame.sequence);
        }
        let mut source_write_failed = false;
        let source = if let Some(source) = frame.source {
            let encoded_source = encoded_source.as_ref().expect("source was encoded above");
            let filename = format!("source-{:020}.qoi", frame.sequence);
            if self.publish_reserved(&filename, encoded_source) {
                Some(DiagnosticSourceFrameArtifact {
                    filename,
                    source_sequence: source.source_sequence,
                    observed_pixel_format: "bgrx",
                    encoded_pixel_format: "rgb8",
                    video: source.contract,
                    memory_type: source.memory_type,
                    stride: source.stride,
                    received_monotonic_ns: source.received_monotonic_ns,
                    file_sha256: encode_sha256(encoded_source),
                    bytes: source_bytes,
                })
            } else {
                source_write_failed = true;
                None
            }
        } else {
            None
        };
        if self.last_recorded_monotonic_ms.is_some() {
            let previous_end = self
                .maximum_frame_coverage_end_ms
                .expect("recorded start and end are updated together");
            let gap = frame.monotonic_start_ms.saturating_sub(previous_end);
            self.maximum_observation_gap_ms = Some(
                self.maximum_observation_gap_ms
                    .map_or(gap, |maximum| maximum.max(gap)),
            );
        } else {
            self.maximum_observation_gap_ms = Some(
                frame
                    .monotonic_start_ms
                    .saturating_sub(self.run_monotonic_start_ms),
            );
        }
        self.last_recorded_monotonic_ms = Some(frame.monotonic_start_ms);
        self.maximum_artifact_end_ms = Some(
            self.maximum_artifact_end_ms
                .map_or(frame.monotonic_end_ms, |end| {
                    end.max(frame.monotonic_end_ms)
                }),
        );
        self.maximum_frame_coverage_end_ms = Some(
            self.maximum_frame_coverage_end_ms
                .map_or(frame.monotonic_end_ms, |end| {
                    end.max(frame.monotonic_end_ms)
                }),
        );
        self.bytes += encoded_bytes + source.as_ref().map_or(0, |source| source.bytes);
        self.frames.push(DiagnosticFrameArtifact {
            sequence: frame.sequence,
            monotonic_start_ms: frame.monotonic_start_ms,
            monotonic_end_ms: frame.monotonic_end_ms,
            filename,
            canonical_pixel_sha256: encode_sha256(frame.pixels),
            file_sha256: encode_sha256(&encoded),
            bytes: encoded_bytes,
            source,
        });
        if source_write_failed {
            self.drop(DiagnosticErrorType::WriteFailed, frame.sequence)
        } else {
            DiagnosticRecordOutcome::Recorded
        }
    }

    fn record_fact(&mut self, fact: &DiagnosticFact) -> DiagnosticRecordOutcome {
        if self.facts_count >= MAX_FACTS_PER_RUN as u64 {
            return self.drop(DiagnosticErrorType::FactLimitExceeded, fact.sequence);
        }
        if fact.monotonic_start_ms < self.run_monotonic_start_ms || !valid_fact(fact) {
            return self.drop(DiagnosticErrorType::InvalidConfiguration, fact.sequence);
        }
        let document = DiagnosticFactArtifactDocument {
            schema: "scorepeek-private-diagnostic-fact-v1",
            fact,
        };
        let bytes = match canonical_json(&document) {
            Ok(bytes) if bytes.len() <= MAX_FACT_BYTES => bytes,
            _ => return self.drop(DiagnosticErrorType::InvalidConfiguration, fact.sequence),
        };
        if self
            .bytes
            .checked_add(bytes.len() as u64)
            .and_then(|bytes| bytes.checked_add(MANIFEST_RESERVE_BYTES))
            .is_none_or(|bytes| bytes > self.policy.maximum_run_bytes)
        {
            return self.drop(DiagnosticErrorType::CapacityExceeded, fact.sequence);
        }
        if let Err(error_type) = self.store_lease.reserve(bytes.len() as u64) {
            return self.drop(error_type, fact.sequence);
        }
        let Some(writer) = self.facts.as_mut() else {
            self.store_lease.release(bytes.len() as u64);
            return self.drop(DiagnosticErrorType::WriteFailed, fact.sequence);
        };
        if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
            self.store_lease.release(bytes.len() as u64);
            self.facts = None;
            return self.drop(DiagnosticErrorType::WriteFailed, fact.sequence);
        }
        self.bytes += bytes.len() as u64;
        self.facts_bytes += bytes.len() as u64;
        self.facts_hasher.update(&bytes);
        self.facts_count += 1;
        self.facts_first_sequence.get_or_insert(fact.sequence);
        self.facts_last_sequence = Some(fact.sequence);
        self.maximum_artifact_end_ms = Some(
            self.maximum_artifact_end_ms
                .map_or(fact.monotonic_end_ms, |end| end.max(fact.monotonic_end_ms)),
        );
        DiagnosticRecordOutcome::Recorded
    }

    fn finish(
        mut self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
        cancellation: Option<&AtomicBool>,
    ) -> DiagnosticFinishOutcome {
        if cancellation.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            return cancelled_finish();
        }
        if let Some(mut facts) = self.facts.take()
            && (facts.flush().is_err() || facts.get_ref().sync_all().is_err())
        {
            self.mark_drop_for_sequence(DiagnosticErrorType::WriteFailed, None);
        }
        if monotonic_end_ms < self.run_monotonic_start_ms
            || self
                .maximum_artifact_end_ms
                .is_some_and(|last| monotonic_end_ms < last)
        {
            self.mark_drop_for_sequence(DiagnosticErrorType::TimingNonmonotonic, None);
        }
        let trailing_gap = self.maximum_frame_coverage_end_ms.map_or_else(
            || monotonic_end_ms.saturating_sub(self.run_monotonic_start_ms),
            |last| monotonic_end_ms.saturating_sub(last),
        );
        self.maximum_observation_gap_ms = Some(
            self.maximum_observation_gap_ms
                .map_or(trailing_gap, |maximum| maximum.max(trailing_gap)),
        );
        let completeness = if self.dropped_count == 0 {
            DiagnosticCompleteness::Complete
        } else {
            DiagnosticCompleteness::Partial
        };
        let Some((bytes, manifest_bytes)) =
            self.encode_manifest(status, completeness, monotonic_end_ms)
        else {
            return DiagnosticFinishOutcome {
                completeness: Some(DiagnosticCompleteness::Partial),
                error_type: Some(DiagnosticErrorType::FinalizeFailed),
                manifest_sha256: None,
            };
        };
        if self
            .bytes
            .checked_add(manifest_bytes)
            .is_none_or(|bytes| bytes > self.policy.maximum_run_bytes)
            || File::open(&self.directory)
                .and_then(|directory| directory.sync_all())
                .is_err()
            || self
                .directory
                .parent()
                .and_then(|parent| File::open(parent).ok())
                .and_then(|parent| parent.sync_all().ok())
                .is_none()
        {
            self.mark_drop_for_sequence(DiagnosticErrorType::FinalizeFailed, None);
            return DiagnosticFinishOutcome {
                completeness: Some(DiagnosticCompleteness::Partial),
                error_type: Some(DiagnosticErrorType::FinalizeFailed),
                manifest_sha256: None,
            };
        }
        if let Err(error_type) = self.store_lease.reserve(manifest_bytes) {
            self.mark_drop_for_sequence(error_type, None);
            return DiagnosticFinishOutcome {
                completeness: Some(DiagnosticCompleteness::Partial),
                error_type: Some(error_type),
                manifest_sha256: None,
            };
        }
        if cancellation.is_some_and(|flag| flag.load(Ordering::Acquire)) {
            self.store_lease.release(manifest_bytes);
            return cancelled_finish();
        }
        // Publication is the final commit point. The helper removes a linked output again
        // if its containing-directory fsync fails, so no fallible finalize step follows it.
        if !self.publish_reserved("manifest.json", &bytes) {
            return DiagnosticFinishOutcome {
                completeness: Some(DiagnosticCompleteness::Partial),
                error_type: Some(DiagnosticErrorType::FinalizeFailed),
                manifest_sha256: None,
            };
        }
        DiagnosticFinishOutcome {
            completeness: Some(completeness),
            error_type: self.last_error_type,
            manifest_sha256: Some(encode_sha256(&bytes)),
        }
    }

    fn publish_reserved(&mut self, filename: &str, bytes: &[u8]) -> bool {
        if publish_private_file(&self.directory.join(filename), bytes).is_ok() {
            true
        } else {
            self.store_lease.release(bytes.len() as u64);
            false
        }
    }

    fn encode_manifest(
        &self,
        status: DiagnosticRunStatus,
        completeness: DiagnosticCompleteness,
        monotonic_end_ms: u64,
    ) -> Option<(Vec<u8>, u64)> {
        let reason_counts = DiagnosticErrorType::ALL
            .into_iter()
            .filter_map(|reason| {
                let count = self.degradation_reason_counts[reason.index()];
                (count > 0).then_some(DiagnosticDegradationReasonCount { reason, count })
            })
            .collect::<Vec<_>>();
        let mut manifest_bytes = 0_u64;
        let mut total_bytes = self.bytes;
        for _ in 0..8 {
            let manifest = DiagnosticRunManifest {
                schema: "scorepeek-private-diagnostic-capture-v4",
                monotonic_end_ms,
                status,
                completeness,
                dropped_count: self.dropped_count,
                last_error_type: self.last_error_type,
                maximum_observation_gap_ms: self.maximum_observation_gap_ms,
                // No create-only, digest-bound multi-recording calibration artifact exists yet.
                result_miss_denominator_eligible: false,
                artifact_bytes: self.bytes,
                manifest_bytes,
                total_bytes,
                start: &self.start,
                frames: &self.frames,
                facts: Some(DiagnosticNdjsonArtifact {
                    filename: "facts.ndjson",
                    record_count: self.facts_count,
                    first_sequence: self.facts_first_sequence,
                    last_sequence: self.facts_last_sequence,
                    file_sha256: encode_digest(self.facts_hasher.clone().finalize()),
                    bytes: self.facts_bytes,
                }),
                degradations: &self.degradations,
                degradation_entries_dropped: self.degradation_entries_dropped,
                degradation_reason_counts: &reason_counts,
            };
            let bytes = canonical_json(&manifest).ok()?;
            let length = u64::try_from(bytes.len()).ok()?;
            let next_total = self.bytes.checked_add(length)?;
            if manifest_bytes == length && total_bytes == next_total {
                return Some((bytes, length));
            }
            manifest_bytes = length;
            total_bytes = next_total;
        }
        None
    }

    fn mark_drop_for_sequence(
        &mut self,
        error_type: DiagnosticErrorType,
        affected_sequence: Option<u64>,
    ) {
        self.dropped_count += 1;
        self.last_error_type = Some(error_type);
        self.degradation_reason_counts[error_type.index()] += 1;
        if self.degradations.len() < MAX_DEGRADATIONS_PER_RUN {
            self.degradations.push(DiagnosticDegradationArtifact {
                reason: error_type,
                affected_sequence,
                first_missing_sequence: None,
                last_missing_sequence: None,
                known_missing_count: 0,
            });
        } else {
            self.degradation_entries_dropped += 1;
        }
    }

    fn mark_sequence_gap(&mut self, first: u64, last: u64) {
        let count = last.saturating_sub(first).saturating_add(1);
        self.dropped_count = self.dropped_count.saturating_add(count);
        self.last_error_type = Some(DiagnosticErrorType::CaptureSequenceGap);
        self.degradation_reason_counts[DiagnosticErrorType::CaptureSequenceGap.index()] = self
            .degradation_reason_counts[DiagnosticErrorType::CaptureSequenceGap.index()]
        .saturating_add(count);
        if self.degradations.len() < MAX_DEGRADATIONS_PER_RUN {
            self.degradations.push(DiagnosticDegradationArtifact {
                reason: DiagnosticErrorType::CaptureSequenceGap,
                affected_sequence: None,
                first_missing_sequence: Some(first),
                last_missing_sequence: Some(last),
                known_missing_count: count,
            });
        } else {
            self.degradation_entries_dropped += 1;
        }
    }

    fn mark_unbound_drops(
        &mut self,
        error_type: DiagnosticErrorType,
        count: u64,
        omitted_entries: u64,
    ) {
        if count == 0 {
            return;
        }
        self.dropped_count = self.dropped_count.saturating_add(count);
        self.last_error_type = Some(error_type);
        self.degradation_reason_counts[error_type.index()] =
            self.degradation_reason_counts[error_type.index()].saturating_add(count);
        self.degradation_entries_dropped = self
            .degradation_entries_dropped
            .saturating_add(omitted_entries);
    }

    fn drop(
        &mut self,
        error_type: DiagnosticErrorType,
        affected_sequence: u64,
    ) -> DiagnosticRecordOutcome {
        self.mark_drop_for_sequence(error_type, Some(affected_sequence));
        DiagnosticRecordOutcome::Dropped(error_type)
    }
}

fn cancelled_finish() -> DiagnosticFinishOutcome {
    DiagnosticFinishOutcome {
        completeness: Some(DiagnosticCompleteness::Partial),
        error_type: Some(DiagnosticErrorType::FlushTimeout),
        manifest_sha256: None,
    }
}

impl From<&DiagnosticPolicy> for DiagnosticPolicyArtifact {
    fn from(policy: &DiagnosticPolicy) -> Self {
        Self {
            sample_interval_ms: policy.sample_interval_ms,
            maximum_run_bytes: policy.maximum_run_bytes,
            aggregate_retention_bytes: DEFAULT_AGGREGATE_BYTES,
            normal_retention_hours: NORMAL_RETENTION_HOURS,
            priority_retention_hours: PRIORITY_RETENTION_HOURS,
            remote_export_enabled: false,
            retention: policy.retention,
        }
    }
}

fn valid_policy(policy: &DiagnosticPolicy) -> bool {
    policy.sample_interval_ms > 0
        && policy.maximum_run_bytes > MANIFEST_RESERVE_BYTES
        && policy.maximum_run_bytes <= DEFAULT_AGGREGATE_BYTES
}

fn valid_descriptor(descriptor: &DiagnosticRunDescriptor) -> bool {
    !descriptor.run_id.is_empty()
        && descriptor.run_id.len() <= 64
        && descriptor
            .run_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && descriptor.resource.program == "scorepeek"
        && descriptor.resource.version == env!("CARGO_PKG_VERSION")
        && valid_sha256(&descriptor.resource.build_sha256)
        && valid_binding(&descriptor.binding)
}

fn valid_run_directory_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_binding(binding: &DiagnosticBinding) -> bool {
    binding.capture_generation > 0
        && [
            &binding.capture_profile_sha256,
            &binding.normalizer_sha256,
            &binding.canonical_layout_sha256,
            &binding.catalog_sha256,
            &binding.model_sha256,
            &binding.runtime_sha256,
        ]
        .into_iter()
        .all(|value| valid_sha256(value))
        && binding.replay.as_ref().is_none_or(|replay| {
            valid_sha256(&replay.request_sha256) && valid_sha256(&replay.extraction_sha256)
        })
}

#[allow(
    clippy::too_many_lines,
    reason = "the strict diagnostic fact validator keeps the complete schema match in one place"
)]
fn valid_fact(fact: &DiagnosticFact) -> bool {
    if fact.monotonic_end_ms < fact.monotonic_start_ms || !valid_fact_status_error(fact) {
        return false;
    }
    let operation_matches = matches!(
        (&fact.operation, &fact.detail),
        (
            DiagnosticOperation::CaptureFrame | DiagnosticOperation::NormalizeFrame,
            DiagnosticDetail::Operation
        ) | (
            DiagnosticOperation::SampleRecognition,
            DiagnosticDetail::SamplingSummary { .. } | DiagnosticDetail::RecognitionBusySkip
        ) | (
            DiagnosticOperation::InspectRecognition,
            DiagnosticDetail::ScreenObservation { .. }
                | DiagnosticDetail::ScreenPredicateObservation { .. }
                | DiagnosticDetail::SongDecision { .. }
                | DiagnosticDetail::FrameProcessingTiming { .. }
        ) | (
            DiagnosticOperation::ObserveFields,
            DiagnosticDetail::FieldObservation { .. }
                | DiagnosticDetail::FieldObservationBusySkip { .. }
        ) | (
            DiagnosticOperation::ReduceSongContext,
            DiagnosticDetail::SongContextObservation { .. }
        ) | (
            DiagnosticOperation::DeliverEvent,
            DiagnosticDetail::EventDelivery { .. }
        ) | (
            DiagnosticOperation::ChangeBinding,
            DiagnosticDetail::BindingChange { .. }
        )
    );
    if !operation_matches {
        return false;
    }
    match &fact.detail {
        DiagnosticDetail::SongContextObservation {
            change,
            candidate_set_sha256,
        } => match change {
            DiagnosticContextChange::Replaced | DiagnosticContextChange::Preserved => {
                candidate_set_sha256.as_deref().is_some_and(valid_sha256)
            }
            DiagnosticContextChange::Cleared | DiagnosticContextChange::AlreadyEmpty => {
                candidate_set_sha256.is_none()
            }
        },
        DiagnosticDetail::SongDecision {
            outcome, song_id, ..
        } => match outcome {
            DiagnosticDecisionOutcome::Accepted => song_id
                .as_deref()
                .is_some_and(|value| valid_bounded_text(value, 128)),
            DiagnosticDecisionOutcome::Unknown | DiagnosticDecisionOutcome::Suppressed => {
                song_id.is_none()
            }
        },
        DiagnosticDetail::BindingChange {
            next_binding_sha256,
        } => valid_sha256(next_binding_sha256),
        DiagnosticDetail::FieldObservation {
            screen,
            observed_fields,
            unimplemented_fields,
            failed_field,
        } => match failed_field {
            None => {
                fact.status == DiagnosticOperationStatus::Success
                    && fact.error_type.is_none()
                    && matches!(
                        (screen, observed_fields, unimplemented_fields),
                        (DiagnosticScreen::Result, 20, 0)
                            | (DiagnosticScreen::MusicSelection, 3, 1)
                    )
            }
            Some(field) => {
                fact.status == DiagnosticOperationStatus::Error
                    && fact.error_type == Some(DiagnosticFactErrorType::FieldObservationFailed)
                    && *observed_fields == 0
                    && matches!(
                        (screen, unimplemented_fields, field),
                        (
                            DiagnosticScreen::Result,
                            0,
                            DiagnosticTextField::ResultTitle
                                | DiagnosticTextField::ResultArtist
                                | DiagnosticTextField::ResultClearType
                                | DiagnosticTextField::ResultDifficulty
                                | DiagnosticTextField::ResultPlayType
                                | DiagnosticTextField::ResultLevel
                                | DiagnosticTextField::ResultNotes
                                | DiagnosticTextField::ResultCurrentScore
                                | DiagnosticTextField::ResultPreviousClearType
                                | DiagnosticTextField::ResultPreviousScore
                                | DiagnosticTextField::ResultPreviousMissCount
                                | DiagnosticTextField::ResultMissCount
                                | DiagnosticTextField::ResultPgreat
                                | DiagnosticTextField::ResultGreat
                                | DiagnosticTextField::ResultGood
                                | DiagnosticTextField::ResultBad
                                | DiagnosticTextField::ResultPoor
                                | DiagnosticTextField::ResultFast
                                | DiagnosticTextField::ResultSlow
                                | DiagnosticTextField::ResultComboBreak
                        ) | (
                            DiagnosticScreen::MusicSelection,
                            1,
                            DiagnosticTextField::MusicSelectCentralTitle
                                | DiagnosticTextField::MusicSelectArtist
                                | DiagnosticTextField::MusicSelectActiveListTitle
                        )
                    )
            }
        },
        _ => true,
    }
}

fn valid_fact_status_error(fact: &DiagnosticFact) -> bool {
    match (fact.status, fact.error_type) {
        (DiagnosticOperationStatus::Success | DiagnosticOperationStatus::Cancel, None)
        | (DiagnosticOperationStatus::Timeout, Some(DiagnosticFactErrorType::OperationTimedOut)) => {
            true
        }
        (DiagnosticOperationStatus::Error, Some(error)) => match fact.operation {
            DiagnosticOperation::CaptureFrame => {
                error == DiagnosticFactErrorType::CaptureUnavailable
            }
            DiagnosticOperation::NormalizeFrame => {
                error == DiagnosticFactErrorType::NormalizeFailed
            }
            DiagnosticOperation::SampleRecognition | DiagnosticOperation::ChangeBinding => false,
            DiagnosticOperation::InspectRecognition => matches!(
                error,
                DiagnosticFactErrorType::RecognitionFailed
                    | DiagnosticFactErrorType::SelectionConflict
            ),
            DiagnosticOperation::ObserveFields => {
                error == DiagnosticFactErrorType::FieldObservationFailed
            }
            DiagnosticOperation::ReduceSongContext => {
                error == DiagnosticFactErrorType::SelectionConflict
            }
            DiagnosticOperation::DeliverEvent => matches!(
                error,
                DiagnosticFactErrorType::EventDeliveryFailed
                    | DiagnosticFactErrorType::ConsumerUnavailable
            ),
        },
        _ => false,
    }
}

fn valid_bounded_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, ()> {
    let mut bytes = serde_json::to_vec(value).map_err(|_| ())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn encode_sha256(bytes: &[u8]) -> String {
    encode_digest(Sha256::digest(bytes))
}

fn encode_digest(bytes: impl AsRef<[u8]>) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in bytes.as_ref() {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

#[derive(Deserialize)]
struct DiagnosticManifestStartEnvelope {
    schema: String,
    start: DiagnosticManifestStartReference,
}

#[derive(Deserialize)]
struct DiagnosticManifestStartReference {
    schema: String,
    filename: String,
    file_sha256: String,
    bytes: u64,
}

/// Checks that a completed run still has the exact start document bound by its manifest.
#[must_use]
pub fn completed_run_start_is_intact(directory: &Path) -> bool {
    let Ok(metadata) = directory.metadata() else {
        return false;
    };
    if !directory.is_absolute() || !metadata.is_dir() {
        return false;
    }
    let Ok(manifest_bytes) = std::fs::read(directory.join("manifest.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<DiagnosticManifestStartEnvelope>(&manifest_bytes)
    else {
        return false;
    };
    if !matches!(
        manifest.schema.as_str(),
        "scorepeek-private-diagnostic-run-v1"
            | "scorepeek-private-diagnostic-run-v2"
            | "scorepeek-private-diagnostic-capture-v3"
            | "scorepeek-private-diagnostic-capture-v4"
    ) || manifest.start.schema != "scorepeek-private-diagnostic-artifact-v1"
        || manifest.start.filename != "run.json"
        || !valid_sha256(&manifest.start.file_sha256)
    {
        return false;
    }
    let start_path = directory.join("run.json");
    let Ok(start_metadata) = start_path.metadata() else {
        return false;
    };
    if !start_metadata.is_file() {
        return false;
    }
    let Ok(start_bytes) = std::fs::read(start_path) else {
        return false;
    };
    start_bytes.len() as u64 == manifest.start.bytes
        && encode_sha256(&start_bytes) == manifest.start.file_sha256
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn descriptor(run_id: &str) -> DiagnosticRunDescriptor {
        DiagnosticRunDescriptor {
            run_id: run_id.to_owned(),
            monotonic_start_ms: 0,
            resource: DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "1".repeat(64),
            },
            binding: DiagnosticBinding {
                capture_generation: 1,
                capture_profile_sha256: "2".repeat(64),
                normalizer_sha256: "3".repeat(64),
                canonical_layout_sha256: "4".repeat(64),
                catalog_sha256: "5".repeat(64),
                model_sha256: "6".repeat(64),
                runtime_sha256: "7".repeat(64),
                replay: None,
            },
        }
    }

    fn pixels(value: u8) -> Vec<u8> {
        vec![value; CANONICAL_BYTES]
    }

    fn source_contract() -> UncalibratedVideoContract {
        UncalibratedVideoContract {
            width: 4,
            height: 2,
            framerate_num: 60,
            framerate_denom: 1,
            maximum_framerate_num: 60,
            maximum_framerate_denom: 1,
            pixel_aspect_num: 1,
            pixel_aspect_denom: 1,
            chroma_site: 0,
            color_range: 0,
            color_matrix: 0,
            transfer_function: 0,
            color_primaries: 0,
        }
    }

    fn frame(sequence: u64, time_ms: u64, pixels: &[u8]) -> DiagnosticFrameInput<'_> {
        DiagnosticFrameInput {
            sequence,
            monotonic_start_ms: time_ms,
            monotonic_end_ms: time_ms + 16,
            pixels,
            source: None,
        }
    }

    #[test]
    fn disabled_recording_has_no_files_and_does_not_change_the_call_result() {
        let root = tempfile::tempdir().unwrap();
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("disabled-run"),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        );
        let expected_result = Result::<_, &'static str>::Ok("recognition-result");
        assert_eq!(
            recorder.record_frame(frame(1, 0, &pixels(1))),
            DiagnosticRecordOutcome::Disabled
        );
        assert_eq!(expected_result, Ok("recognition-result"));
        assert_eq!(
            recorder
                .finish(DiagnosticRunStatus::Success, 0)
                .completeness,
            None
        );
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn named_recording_directory_keeps_the_session_run_id() {
        let root = tempfile::tempdir().unwrap();
        let run = descriptor("session-1");
        let recorder = DiagnosticRecorder::start_named(
            root.path(),
            "capture",
            &run,
            DiagnosticPolicy::default(),
        );
        let outcome = recorder.finish(DiagnosticRunStatus::Success, 1);

        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Complete));
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("capture/run.json")).unwrap())
                .unwrap();
        assert_eq!(value["run_id"], "session-1");
        assert!(!root.path().join("session-1").exists());
    }

    #[test]
    fn paired_source_bytes_are_exact_and_bound_to_the_same_frame_entry() {
        let root = tempfile::tempdir().unwrap();
        let canonical = pixels(31);
        let source = vec![17_u8; 32];
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("paired-source-run"),
            DiagnosticPolicy::default(),
        );
        let outcome = recorder.record_frame(DiagnosticFrameInput {
            sequence: 7,
            monotonic_start_ms: 1_000,
            monotonic_end_ms: 1_016,
            pixels: &canonical,
            source: Some(DiagnosticSourceFrameInput {
                source_sequence: 70,
                contract: source_contract(),
                memory_type: UncalibratedMemoryType::MemoryFileDescriptor,
                stride: 16,
                received_monotonic_ns: 1_000_000_000,
                bytes: &source,
            }),
        });
        assert_eq!(outcome, DiagnosticRecordOutcome::Recorded);
        let finished = recorder.finish(DiagnosticRunStatus::Success, 2_000);
        assert_eq!(
            finished.completeness,
            Some(DiagnosticCompleteness::Complete)
        );

        let directory = root.path().join("paired-source-run");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
        let entry = &manifest["frames"][0];
        assert_eq!(entry["sequence"], 7);
        assert_eq!(
            entry["source"]["filename"],
            "source-00000000000000000007.qoi"
        );
        assert_eq!(entry["source"]["observed_pixel_format"], "bgrx");
        assert_eq!(entry["source"]["encoded_pixel_format"], "rgb8");
        assert_eq!(entry["source"]["stride"], 16);
        let encoded = fs::read(directory.join("source-00000000000000000007.qoi")).unwrap();
        assert_eq!(entry["source"]["bytes"], encoded.len());
        let (header, decoded) = qoi::decode_to_vec(&encoded).unwrap();
        assert_eq!((header.width, header.height), (4, 2));
        assert_eq!(decoded, vec![17_u8; 24]);
        let runs = crate::diagnostic_control::inspect_store(root.path()).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].run_id, "paired-source-run");
    }

    #[test]
    fn unavailable_store_degrades_without_changing_the_call_result() {
        let root = tempfile::tempdir().unwrap();
        let mut recorder = DiagnosticRecorder::start(
            &root.path().join("missing"),
            &descriptor("degraded-run"),
            DiagnosticPolicy::default(),
        );
        let expected_result = Result::<_, &'static str>::Ok("recognition-result");
        assert_eq!(
            recorder.record_frame(frame(1, 0, &pixels(2))),
            DiagnosticRecordOutcome::Dropped(DiagnosticErrorType::StoreUnavailable)
        );
        assert_eq!(expected_result, Ok("recognition-result"));
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn records_qoi_frames_and_manifest_last_with_exact_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let input = pixels(17);
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("complete-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            recorder.record_frame(frame(10, 100, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        assert!(!root.path().join("complete-run/manifest.json").exists());
        let outcome = recorder.finish(DiagnosticRunStatus::Success, 116);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Complete));
        assert!(outcome.manifest_sha256.is_some());

        let encoded = fs::read(
            root.path()
                .join("complete-run/frame-00000000000000000010.qoi"),
        )
        .unwrap();
        let (header, decoded) = qoi::decode_to_vec(encoded).unwrap();
        assert_eq!(header.width, CANONICAL_WIDTH);
        assert_eq!(header.height, CANONICAL_HEIGHT);
        assert_eq!(decoded, input);
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("complete-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["completeness"], "complete");
        assert_eq!(manifest["result_miss_denominator_eligible"], false);
        let actual_total = fs::read_dir(root.path().join("complete-run"))
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum::<u64>();
        assert_eq!(manifest["total_bytes"], actual_total);
        assert_eq!(
            manifest["manifest_bytes"],
            fs::metadata(root.path().join("complete-run/manifest.json"))
                .unwrap()
                .len()
        );
        assert!(completed_run_start_is_intact(
            &root.path().join("complete-run")
        ));
        fs::write(root.path().join("complete-run/run.json"), b"corrupt\n").unwrap();
        assert!(!completed_run_start_is_intact(
            &root.path().join("complete-run")
        ));
    }

    #[test]
    fn cadence_is_independent_of_recognition_facts() {
        let root = tempfile::tempdir().unwrap();
        let input = pixels(23);
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("cadence-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            recorder.record_frame(frame(1, 0, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        assert_eq!(
            recorder.record_frame(frame(2, 50, &input)),
            DiagnosticRecordOutcome::SkippedCadence
        );
        let fact = DiagnosticFact {
            sequence: 2,
            monotonic_start_ms: 500,
            monotonic_end_ms: 501,
            operation: DiagnosticOperation::InspectRecognition,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::ScreenObservation {
                screen: DiagnosticScreen::Unknown,
            },
        };
        assert_eq!(
            recorder.record_fact(&fact),
            DiagnosticRecordOutcome::Recorded
        );
        assert_eq!(
            recorder.record_frame(frame(3, 1_000, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        let outcome = recorder.finish(DiagnosticRunStatus::Success, 1_016);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Complete));
    }

    #[test]
    fn recognition_facts_never_reduce_canonical_frame_coverage_gaps() {
        let root = tempfile::tempdir().unwrap();
        let fact = DiagnosticFact {
            sequence: 1,
            monotonic_start_ms: 0,
            monotonic_end_ms: 10_000,
            operation: DiagnosticOperation::InspectRecognition,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::ScreenObservation {
                screen: DiagnosticScreen::Unknown,
            },
        };
        let mut fact_only = DiagnosticRecorder::start(
            root.path(),
            &descriptor("fact-only-coverage-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            fact_only.record_fact(&fact),
            DiagnosticRecordOutcome::Recorded
        );
        let _ = fact_only.finish(DiagnosticRunStatus::Success, 10_000);
        let fact_only_manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("fact-only-coverage-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(fact_only_manifest["maximum_observation_gap_ms"], 10_000);

        let input = pixels(24);
        let mut with_frames = DiagnosticRecorder::start(
            root.path(),
            &descriptor("fact-between-frames-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            with_frames.record_frame(frame(1, 0, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        assert_eq!(
            with_frames.record_fact(&fact),
            DiagnosticRecordOutcome::Recorded
        );
        assert_eq!(
            with_frames.record_frame(frame(2, 5_000, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        let _ = with_frames.finish(DiagnosticRunStatus::Success, 10_000);
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("fact-between-frames-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["maximum_observation_gap_ms"], 4_984);
    }

    #[test]
    fn nonmonotonic_timing_is_partial_even_when_sequences_increase() {
        let root = tempfile::tempdir().unwrap();
        let input = pixels(24);
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("timing-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            recorder.record_frame(frame(1, 1_000, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        assert_eq!(
            recorder.record_frame(frame(2, 900, &input)),
            DiagnosticRecordOutcome::Dropped(DiagnosticErrorType::TimingNonmonotonic)
        );
        assert_eq!(
            recorder
                .finish(DiagnosticRunStatus::Success, 1_016)
                .completeness,
            Some(DiagnosticCompleteness::Partial)
        );
    }

    #[test]
    fn frame_end_regression_is_partial_and_overlap_keeps_the_coverage_frontier() {
        let root = tempfile::tempdir().unwrap();
        let overlap_root = tempfile::tempdir().unwrap();
        let input = pixels(25);
        let mut regressing = DiagnosticRecorder::start(
            root.path(),
            &descriptor("end-regression-run"),
            DiagnosticPolicy::default(),
        );
        let first = DiagnosticFrameInput {
            sequence: 1,
            monotonic_start_ms: 0,
            monotonic_end_ms: 5_000,
            pixels: &input,
            source: None,
        };
        let regressed = DiagnosticFrameInput {
            sequence: 2,
            monotonic_start_ms: 1_000,
            monotonic_end_ms: 1_016,
            pixels: &input,
            source: None,
        };
        assert_eq!(
            regressing.record_frame(first),
            DiagnosticRecordOutcome::Recorded
        );
        assert_eq!(
            regressing.record_frame(regressed),
            DiagnosticRecordOutcome::Dropped(DiagnosticErrorType::TimingNonmonotonic)
        );
        assert_eq!(
            regressing
                .finish(DiagnosticRunStatus::Success, 5_000)
                .completeness,
            Some(DiagnosticCompleteness::Partial)
        );

        let mut overlapping = DiagnosticRecorder::start(
            overlap_root.path(),
            &descriptor("overlap-run"),
            DiagnosticPolicy::default(),
        );
        let extended = DiagnosticFrameInput {
            monotonic_end_ms: 6_000,
            ..regressed
        };
        assert_eq!(
            overlapping.record_frame(first),
            DiagnosticRecordOutcome::Recorded
        );
        assert_eq!(
            overlapping.record_frame(extended),
            DiagnosticRecordOutcome::Recorded
        );
        let outcome = overlapping.finish(DiagnosticRunStatus::Success, 6_000);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Complete));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(overlap_root.path().join("overlap-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["maximum_observation_gap_ms"], 0);
    }

    #[test]
    fn capacity_and_sequence_gaps_downgrade_without_replacing_the_result() {
        let root = tempfile::tempdir().unwrap();
        let input = pixels(29);
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("partial-run"),
            DiagnosticPolicy {
                maximum_run_bytes: MANIFEST_RESERVE_BYTES + 2_048,
                ..DiagnosticPolicy::default()
            },
        );
        let expected_result = Result::<_, &'static str>::Ok(42);
        assert_eq!(
            recorder.record_frame(frame(1, 0, &input)),
            DiagnosticRecordOutcome::Dropped(DiagnosticErrorType::CapacityExceeded)
        );
        assert_eq!(
            recorder.record_frame(frame(3, 1_000, &input)),
            DiagnosticRecordOutcome::Dropped(DiagnosticErrorType::CapacityExceeded)
        );
        assert_eq!(expected_result, Ok(42));
        let outcome = recorder.finish(DiagnosticRunStatus::Success, 1_016);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Partial));
        assert!(outcome.error_type.is_some());
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("partial-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["dropped_count"], 3);
        assert_eq!(manifest["degradations"][1]["first_missing_sequence"], 2);
        assert_eq!(manifest["degradations"][1]["last_missing_sequence"], 2);
        assert_eq!(manifest["degradations"][1]["known_missing_count"], 1);
    }

    #[test]
    fn uncalibrated_slice_never_enables_the_result_miss_denominator() {
        let root = tempfile::tempdir().unwrap();
        let input = pixels(31);
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("denominator-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            recorder.record_frame(frame(1, 0, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        assert_eq!(
            recorder.record_frame(frame(2, 1_000, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 3_000);
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("denominator-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["maximum_observation_gap_ms"], 1_984);
        assert_eq!(manifest["result_miss_denominator_eligible"], false);
    }

    #[test]
    fn facts_are_strict_bounded_and_do_not_put_pixels_in_public_output() {
        let root = tempfile::tempdir().unwrap();
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("fact-run"),
            DiagnosticPolicy::default(),
        );
        let fact = DiagnosticFact {
            sequence: 4,
            monotonic_start_ms: 4_000,
            monotonic_end_ms: 4_010,
            operation: DiagnosticOperation::ReduceSongContext,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::SongContextObservation {
                change: DiagnosticContextChange::Preserved,
                candidate_set_sha256: Some("8".repeat(64)),
            },
        };
        assert_eq!(
            recorder.record_fact(&fact),
            DiagnosticRecordOutcome::Recorded
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 4_010);
        let bytes = fs::read(root.path().join("fact-run/facts.ndjson")).unwrap();
        assert!(!bytes.windows(4).any(|window| window == b"RGB8"));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["fact"]["detail"]["kind"],
            "song_context_observation"
        );
    }

    #[test]
    fn inconsistent_fact_variants_and_song_decisions_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("invalid-fact-run"),
            DiagnosticPolicy::default(),
        );
        let mismatched = DiagnosticFact {
            sequence: 1,
            monotonic_start_ms: 0,
            monotonic_end_ms: 1,
            operation: DiagnosticOperation::DeliverEvent,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::ScreenObservation {
                screen: DiagnosticScreen::Unknown,
            },
        };
        assert_eq!(
            recorder.record_fact(&mismatched),
            DiagnosticRecordOutcome::Dropped(DiagnosticErrorType::InvalidConfiguration)
        );
        let missing_song = DiagnosticFact {
            sequence: 2,
            monotonic_start_ms: 2,
            monotonic_end_ms: 3,
            operation: DiagnosticOperation::InspectRecognition,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::SongDecision {
                domain: DiagnosticDecisionDomain::Result,
                outcome: DiagnosticDecisionOutcome::Accepted,
                song_id: None,
            },
        };
        assert_eq!(
            recorder.record_fact(&missing_song),
            DiagnosticRecordOutcome::Dropped(DiagnosticErrorType::InvalidConfiguration)
        );
        let _ = recorder.finish(DiagnosticRunStatus::Success, 3);
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("invalid-fact-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["degradations"][0]["affected_sequence"], 1);
        assert_eq!(manifest["degradations"][1]["affected_sequence"], 2);
    }

    #[test]
    fn fact_errors_are_operation_scoped_and_timeout_is_typed() {
        let mut fact = DiagnosticFact {
            sequence: 1,
            monotonic_start_ms: 0,
            monotonic_end_ms: 1,
            operation: DiagnosticOperation::CaptureFrame,
            status: DiagnosticOperationStatus::Error,
            error_type: Some(DiagnosticFactErrorType::CaptureUnavailable),
            detail: DiagnosticDetail::Operation,
        };
        assert!(valid_fact(&fact));
        fact.error_type = Some(DiagnosticFactErrorType::SelectionConflict);
        assert!(!valid_fact(&fact));
        fact.status = DiagnosticOperationStatus::Timeout;
        fact.error_type = Some(DiagnosticFactErrorType::OperationTimedOut);
        assert!(valid_fact(&fact));
        fact.error_type = None;
        assert!(!valid_fact(&fact));
    }

    #[test]
    fn run_boundaries_are_persisted_and_out_of_boundary_artifacts_are_partial() {
        let root = tempfile::tempdir().unwrap();
        let mut bound = descriptor("boundary-run");
        bound.monotonic_start_ms = 1_000;
        let input = pixels(43);
        let mut recorder =
            DiagnosticRecorder::start(root.path(), &bound, DiagnosticPolicy::default());
        assert_eq!(
            recorder.record_frame(frame(7, 999, &input)),
            DiagnosticRecordOutcome::Dropped(DiagnosticErrorType::InvalidConfiguration)
        );
        assert_eq!(
            recorder.record_frame(frame(8, 1_000, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        let outcome = recorder.finish(DiagnosticRunStatus::Success, 1_000);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Partial));
        let run: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("boundary-run/run.json")).unwrap())
                .unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("boundary-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(run["monotonic_start_ms"], 1_000);
        assert_eq!(manifest["monotonic_end_ms"], 1_000);
        assert_eq!(manifest["last_error_type"], "timing_nonmonotonic");
    }

    #[test]
    fn degradation_log_truncation_is_explicit_and_reason_counted() {
        let root = tempfile::tempdir().unwrap();
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("truncated-degradation-run"),
            DiagnosticPolicy::default(),
        );
        for sequence in 0..=MAX_DEGRADATIONS_PER_RUN as u64 {
            let fact = DiagnosticFact {
                sequence,
                monotonic_start_ms: 0,
                monotonic_end_ms: 0,
                operation: DiagnosticOperation::DeliverEvent,
                status: DiagnosticOperationStatus::Success,
                error_type: None,
                detail: DiagnosticDetail::ScreenObservation {
                    screen: DiagnosticScreen::Unknown,
                },
            };
            assert_eq!(
                recorder.record_fact(&fact),
                DiagnosticRecordOutcome::Dropped(DiagnosticErrorType::InvalidConfiguration)
            );
        }
        let _ = recorder.finish(DiagnosticRunStatus::Success, 0);
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("truncated-degradation-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["degradations"].as_array().unwrap().len(), 4_096);
        assert_eq!(manifest["degradation_entries_dropped"], 1);
        assert_eq!(manifest["degradation_reason_counts"][0]["count"], 4_097);
    }

    #[test]
    fn absent_completion_manifest_is_observable_as_partial() {
        let root = tempfile::tempdir().unwrap();
        let input = pixels(37);
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("crashed-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            recorder.record_frame(frame(1, 0, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        drop(recorder);
        assert!(root.path().join("crashed-run/run.json").is_file());
        assert!(!root.path().join("crashed-run/manifest.json").exists());
    }

    #[test]
    fn final_manifest_collision_is_partial_and_never_clobbers() {
        let root = tempfile::tempdir().unwrap();
        let input = pixels(41);
        let mut recorder = DiagnosticRecorder::start(
            root.path(),
            &descriptor("collision-run"),
            DiagnosticPolicy::default(),
        );
        assert_eq!(
            recorder.record_frame(frame(1, 0, &input)),
            DiagnosticRecordOutcome::Recorded
        );
        let manifest = root.path().join("collision-run/manifest.json");
        fs::write(&manifest, b"existing\n").unwrap();
        let outcome = recorder.finish(DiagnosticRunStatus::Success, 16);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Partial));
        assert_eq!(
            outcome.error_type,
            Some(DiagnosticErrorType::FinalizeFailed)
        );
        assert_eq!(fs::read(manifest).unwrap(), b"existing\n");
    }
}
