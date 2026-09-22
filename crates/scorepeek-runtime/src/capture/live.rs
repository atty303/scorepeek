#![cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "legacy capture gates are compiled only as private test harness support"
    )
)]

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Read as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use scorepeek::capture::{
    AdmittedFrameNormalizer, AuthoredGamescopeProfileBinding, CalibratedGamescopeLease,
    CalibratedSourceFrameEvidence, CalibratedVulkanLease, CaptureDiagnosticDetail,
    CaptureDiagnosticFact, CaptureDiagnosticOperation, CaptureDiagnosticSink,
    CaptureDiagnosticStatus, CaptureErrorType, CaptureGeneration, EdgeCrop,
    GamescopeProfileBinding, NormalizedCanonicalFrame, RuntimeCaptureBackend,
    acquire_gamescope_source, acquire_pipewire_source, admit_gamescope_profile,
    admit_runtime_profile, admit_vulkan_session, start_uncalibrated_gamescope_receiver,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::canonical_source::CanonicalFrameSource;
use crate::diagnostics::live::{BoundCanonicalFrame, DiagnosticBridge};
use crate::diagnostics::ring::DiagnosticEnqueueOutcome;
use crate::recognition_artifact::{
    RecognitionArtifactEnqueueOutcome, RecognitionArtifactFinishOutcome,
    RecognitionArtifactFinishStatus, RecognitionArtifactRetention, RecognitionArtifactWorker,
};
use crate::recording::writer::{CanonicalRecordingCompleteness, CanonicalRecordingWorker};
use crate::service::session::recognition::RecognitionSession;
use crate::service::session::recognition::field_observer::{
    DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT, FieldObserverFinishStatus, FieldObserverOfferError,
};
use crate::service::session::recognition::field_session::{
    FieldObservationSession, FieldObservationSessionPoll, FieldObservationStartError,
    FieldObservationSubmission, PendingSessionFieldObservation,
};
use crate::service::session::recognition::screen_field_observer::{
    RegisteredScreenFieldObserver, RegisteredScreenFieldObserverLoadError,
};
use scorepeek_core::diagnostics::{
    DiagnosticCompleteness, DiagnosticErrorType, DiagnosticPolicy, DiagnosticRunDescriptor,
    DiagnosticRunStatus,
};
use scorepeek_core::frame::CanonicalLayout;
use scorepeek_core::game_version::GameVersionResolver;
use scorepeek_core::model::session::RegisteredScreenFieldObservation;
use scorepeek_core::recognition::screen::{ScreenClass, ScreenFieldObservationError};
use scorepeek_core::recognition::title::{OnnxParityError, RegisteredResourceLoadErrorType};
use scorepeek_core::session::episode::{RawScreenState, SemanticScreenEpisode};
use scorepeek_core::session::result::{CadenceDecision, RecognitionCadence};
use scorepeek_core::session::timeline::{TimelineAction, TimelineDriver};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const RECEIVER_START_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_GATE_DURATION_MS: u64 = 60_000;
const MAX_CONSUMER_INTERVAL_MS: u64 = 60_000;
const MIN_LIFECYCLE_RUNS: u32 = 2;
const MAX_LIFECYCLE_RUNS: u32 = 100;
const MAX_DIAGNOSTIC_FACTS: usize = 32;
const MAX_PROC_STATUS_BYTES: u64 = 64 * 1024;
const MAX_BINDING_BYTES: usize = 64 * 1024;
const LIVE_SESSION_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LiveGateStatus {
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecycleGateErrorType {
    CaptureRunFailed,
    ProcessResourceUnavailable,
    ExpectedOverwriteMissing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum BindingAdmissionGateErrorType {
    BindingUnavailable,
    BindingInvalid,
    CaptureFailed,
    AdmissionRejected,
    ShutdownFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CanonicalFrameGateErrorType {
    BindingUnavailable,
    BindingInvalid,
    CaptureFailed,
    AdmissionRejected,
    FrameUnavailable,
    NormalizationFailed,
    ShutdownFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DiagnosticHandoffGateErrorType {
    BindingUnavailable,
    BindingInvalid,
    CaptureFailed,
    AdmissionRejected,
    DiagnosticBindingMismatch,
    DiagnosticConfigurationInvalid,
    FrameUnavailable,
    NormalizationFailed,
    RecognitionFailed,
    ShutdownFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum FieldObservationGateErrorType {
    CaptureFailed,
    AdmissionRejected,
    DiagnosticBindingMismatch,
    DiagnosticConfigurationInvalid,
    InvalidResourceLocation,
    ModelBindingMismatch,
    RuntimeBindingMismatch,
    CatalogUnavailable,
    CatalogBindingMismatch,
    CatalogLoadFailed,
    ModelBundleInvalid,
    RuntimeInitializationFailed,
    NumericModelUnavailable,
    CandidateDomainInvalid,
    FieldObserverUnavailable,
    FrameUnavailable,
    NormalizationFailed,
    RecognitionFailed,
    FieldObservationFailed,
    FieldObservationUnavailable,
    ResultObservationUnavailable,
    RecognitionArtifactIncomplete,
    ShutdownFailed,
    FieldObserverFinishFailed,
    ResultOutputFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveSessionStopReason {
    RequestedSignal,
    SourceEnded,
    SourceContractChanged,
    TerminalFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveSessionStartupRetry {
    Admission,
    Catalog,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticScreenEpisodePhase {
    Started,
    Suspended,
    Resumed,
    Closing,
    Finalized,
}

#[derive(Clone, Copy)]
pub enum GamescopeLiveSessionEvent<'a> {
    Started {
        capture_generation: u64,
        capture_profile_sha256: &'a str,
        normalizer_artifact_sha256: &'a str,
        capture_profile_document: Option<&'a str>,
        normalizer_document: Option<&'a str>,
    },
    RecordingHealth {
        snapshot: crate::recording::writer::RecordingHealthSnapshot,
    },
    RecordingFinalizing,
    CaptureDiagnostic {
        fact: &'a CaptureDiagnosticFact,
    },
    RawScreenObserved {
        semantic_episode_id: Option<u64>,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
        result_presence: scorepeek_core::recognition::screen::ResultPresenceEvidence,
        play_presence: scorepeek_core::recognition::screen::PlayPresenceEvidence,
    },
    SemanticScreenEpisode {
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
        phase: SemanticScreenEpisodePhase,
    },
    GameVersionIdentified {
        source_sequence: u64,
        version: &'a str,
    },
    Observation {
        screen_episode_id: u64,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        output: &'a RegisteredScreenFieldObservation,
    },
}

pub use crate::service::session::recognition::LiveEventProcessingTiming;

type LiveEventEmitter<'e> = dyn for<'a> FnMut(GamescopeLiveSessionEvent<'a>) -> Result<LiveEventProcessingTiming, String>
    + 'e;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
struct ProcessResourceSnapshot {
    open_file_descriptors: u64,
    threads: u64,
    resident_bytes: u64,
}

impl ProcessResourceSnapshot {
    fn update_maximum(&mut self, observed: Self) {
        self.open_file_descriptors = self
            .open_file_descriptors
            .max(observed.open_file_descriptors);
        self.threads = self.threads.max(observed.threads);
        self.resident_bytes = self.resident_bytes.max(observed.resident_bytes);
    }
}

#[derive(Debug, Serialize)]
pub struct GamescopeLiveGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    requested_duration_ms: u64,
    consumer_interval_ms: u64,
    consumed_frames: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    error_type: Option<CaptureErrorType>,
    diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_diagnostic_facts: u64,
}

#[derive(Debug, Serialize)]
struct LifecycleRunSummary {
    run: u32,
    status: LiveGateStatus,
    error_type: Option<CaptureErrorType>,
    consumed_frames: u64,
    received_frames: u64,
    overwritten_frames: u64,
    last_sequence: Option<u64>,
    maximum_gap_ns: u64,
    diagnostic_fact_count: u32,
    dropped_diagnostic_facts: u64,
    phases: LifecyclePhaseSummary,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LifecyclePhaseStatus {
    #[default]
    NotObserved,
    Success,
}

#[derive(Debug, Default, Serialize)]
struct LifecyclePhaseSummary {
    negotiation: LifecyclePhaseStatus,
    first_frame: LifecyclePhaseStatus,
    receiver_shutdown: LifecyclePhaseStatus,
    provider_shutdown: LifecyclePhaseStatus,
}

#[derive(Debug, Serialize)]
pub struct GamescopeLifecycleGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    error_type: Option<LifecycleGateErrorType>,
    requested_duration_ms: u64,
    consumer_interval_ms: u64,
    requested_runs: u32,
    completed_runs: u32,
    overwrite_observed: bool,
    resources_before_first_run: Option<ProcessResourceSnapshot>,
    resources_after_warmup: Option<ProcessResourceSnapshot>,
    maximum_resources_after_run: Option<ProcessResourceSnapshot>,
    resources_after_final_run: Option<ProcessResourceSnapshot>,
    runs: Vec<LifecycleRunSummary>,
}

#[derive(Debug, Serialize)]
pub struct GamescopeBindingAdmissionGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    error_type: Option<BindingAdmissionGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_profile_sha256: Option<String>,
    normalizer_artifact_sha256: Option<String>,
    diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_diagnostic_facts: u64,
}

#[derive(Debug, Serialize)]
pub struct GamescopeCanonicalFrameGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    error_type: Option<CanonicalFrameGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: u64,
    capture_profile_sha256: Option<String>,
    normalizer_artifact_sha256: Option<String>,
    source_sequence: Option<u64>,
    canonical_rgb8_sha256: Option<String>,
    diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_diagnostic_facts: u64,
}

#[derive(Debug, Serialize)]
pub struct GamescopeDiagnosticHandoffGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    error_type: Option<DiagnosticHandoffGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: u64,
    observed_frames: u64,
    normalized_frames: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    enqueued_frames: u64,
    skipped_cadence_frames: u64,
    rejected_frames: u64,
    disabled_frames: u64,
    queue_full_frames: u64,
    worker_unavailable_frames: u64,
    diagnostic_completeness: Option<DiagnosticCompleteness>,
    diagnostic_error_type: Option<DiagnosticErrorType>,
    diagnostic_manifest_sha256: Option<String>,
    capture_diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_capture_diagnostic_facts: u64,
}

#[derive(Debug, Serialize)]
pub struct GamescopeRecognitionHandoffGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    error_type: Option<DiagnosticHandoffGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: u64,
    observed_frames: u64,
    normalized_frames: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    diagnostic_frame_enqueued: u64,
    diagnostic_frame_skipped_cadence: u64,
    diagnostic_frame_rejected: u64,
    diagnostic_frame_disabled: u64,
    diagnostic_frame_queue_full: u64,
    diagnostic_frame_worker_unavailable: u64,
    inspected_frames: u64,
    title_frames: u64,
    result_frames: u64,
    music_select_frames: u64,
    mode_select_frames: u64,
    decide_transition_frames: u64,
    play_frames: u64,
    unknown_frames: u64,
    recognition_failures: u64,
    diagnostic_fact_enqueued: u64,
    diagnostic_fact_skipped_cadence: u64,
    diagnostic_fact_rejected: u64,
    diagnostic_fact_disabled: u64,
    diagnostic_fact_queue_full: u64,
    diagnostic_fact_worker_unavailable: u64,
    diagnostic_completeness: Option<DiagnosticCompleteness>,
    diagnostic_error_type: Option<DiagnosticErrorType>,
    diagnostic_manifest_sha256: Option<String>,
    capture_diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_capture_diagnostic_facts: u64,
}

#[derive(Debug, Serialize)]
pub struct GamescopeFieldObservationGateReport {
    schema: &'static str,
    status: LiveGateStatus,
    error_type: Option<FieldObservationGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: u64,
    observed_frames: u64,
    normalized_frames: u64,
    recognition_ticks: u64,
    recognition_busy_skips: u64,
    maximum_consecutive_busy_skips: u64,
    field_observation_busy_skips: u64,
    maximum_consecutive_field_observation_busy_skips: u64,
    last_recognition_sequence: Option<u64>,
    inspected_frames: u64,
    title_frames: u64,
    result_frames: u64,
    music_select_frames: u64,
    mode_select_frames: u64,
    decide_transition_frames: u64,
    play_frames: u64,
    unknown_frames: u64,
    field_not_applicable: u64,
    field_submitted: u64,
    field_rejected: u64,
    field_ready_success: u64,
    field_ready_failure: u64,
    candidate_sets: u64,
    scored_candidates: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_observations: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_artifact_enqueued: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_artifact_queue_full: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_artifact_worker_unavailable: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_artifact_status: Option<RecognitionArtifactFinishStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_artifact_manifest_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_artifact_input_observations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recognition_artifact_retained_observations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    canonical_recording_completeness: Option<CanonicalRecordingCompleteness>,
    canonical_recording_manifest_published: bool,
    field_worker_status: Option<FieldWorkerStatus>,
    field_worker_submitted: Option<u64>,
    field_worker_completed: Option<u64>,
    field_worker_abandoned: Option<u64>,
    diagnostic_completeness: Option<DiagnosticCompleteness>,
    diagnostic_error_type: Option<DiagnosticErrorType>,
    diagnostic_manifest_sha256: Option<String>,
    capture_diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_capture_diagnostic_facts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_stop_reason: Option<LiveSessionStopReason>,
    #[serde(skip)]
    failure_detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum FieldWorkerStatus {
    Complete,
    Timeout,
    WorkerUnavailable,
}

impl GamescopeLiveGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

impl GamescopeLifecycleGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

impl GamescopeBindingAdmissionGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

impl GamescopeCanonicalFrameGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

impl GamescopeDiagnosticHandoffGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

impl GamescopeRecognitionHandoffGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }
}

impl GamescopeFieldObservationGateReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, LiveGateStatus::Success)
    }

    pub fn failure_detail(&self) -> Option<&str> {
        self.failure_detail.as_deref()
    }

    pub const fn stop_reason(&self) -> Option<LiveSessionStopReason> {
        self.session_stop_reason
    }

    pub const fn output_failed(&self) -> bool {
        matches!(
            self.error_type,
            Some(FieldObservationGateErrorType::ResultOutputFailed)
        )
    }

    pub const fn startup_retry(&self) -> Option<LiveSessionStartupRetry> {
        match self.error_type {
            Some(
                FieldObservationGateErrorType::CaptureFailed
                | FieldObservationGateErrorType::AdmissionRejected,
            ) => Some(LiveSessionStartupRetry::Admission),
            Some(
                FieldObservationGateErrorType::CatalogUnavailable
                | FieldObservationGateErrorType::CatalogBindingMismatch
                | FieldObservationGateErrorType::CatalogLoadFailed,
            ) => Some(LiveSessionStartupRetry::Catalog),
            _ => None,
        }
    }

    pub const fn capture_error_type(&self) -> Option<CaptureErrorType> {
        self.capture_error_type
    }

    pub fn startup_failure_summary(&self) -> String {
        self.failure_detail.clone().unwrap_or_else(|| {
            format!("capture live session startup failed: {:?}", self.error_type)
        })
    }

    pub const fn canonical_recording_is_complete(&self) -> bool {
        matches!(
            self.canonical_recording_completeness,
            Some(CanonicalRecordingCompleteness::Complete)
        ) && self.canonical_recording_manifest_published
    }
}

#[derive(Clone, Copy, Default)]
struct HandoffCounters {
    observed_frames: u64,
    normalized_frames: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    enqueued_frames: u64,
    skipped_cadence_frames: u64,
    rejected_frames: u64,
    disabled_frames: u64,
    queue_full_frames: u64,
    worker_unavailable_frames: u64,
}

#[derive(Clone, Copy, Default)]
struct RecognitionHandoffCounters {
    inspected_frames: u64,
    title_frames: u64,
    result_frames: u64,
    music_select_frames: u64,
    mode_select_frames: u64,
    decide_transition_frames: u64,
    play_frames: u64,
    unknown_frames: u64,
    recognition_failures: u64,
    fact_outcomes: EnqueueOutcomeCounters,
}

#[derive(Clone, Copy, Default)]
struct FieldObservationCounters {
    observed_frames: u64,
    normalized_frames: u64,
    recognition_ticks: u64,
    recognition_busy_skips: u64,
    maximum_consecutive_busy_skips: u64,
    field_observation_busy_skips: u64,
    maximum_consecutive_field_observation_busy_skips: u64,
    consecutive_field_observation_busy_skips: u64,
    last_recognition_sequence: Option<u64>,
    inspected_frames: u64,
    title_frames: u64,
    result_frames: u64,
    music_select_frames: u64,
    mode_select_frames: u64,
    decide_transition_frames: u64,
    play_frames: u64,
    unknown_frames: u64,
    field_not_applicable: u64,
    field_submitted: u64,
    field_rejected: u64,
    field_ready_success: u64,
    field_ready_failure: u64,
    candidate_sets: u64,
    scored_candidates: u64,
    result_observations: u64,
    recognition_artifact_enqueued: u64,
    recognition_artifact_queue_full: u64,
    recognition_artifact_worker_unavailable: u64,
}

#[derive(Clone, Copy, Default)]
struct EnqueueOutcomeCounters {
    enqueued: u64,
    skipped_cadence: u64,
    rejected: u64,
    disabled: u64,
    queue_full: u64,
    worker_unavailable: u64,
}

struct HandoffGateRun {
    diagnostic: GamescopeDiagnosticHandoffGateReport,
    recognition: RecognitionHandoffCounters,
}

enum HandoffSession {
    Diagnostic(DiagnosticBridge),
    Recognition(RecognitionSession),
}

impl HandoffSession {
    fn finish(
        self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
    ) -> crate::diagnostics::writer::DiagnosticFinishOutcome {
        match self {
            Self::Diagnostic(bridge) => bridge.finish(status, monotonic_end_ms),
            Self::Recognition(session) => session.finish(status, monotonic_end_ms),
        }
    }
}

pub struct GamescopeDiagnosticHandoffGateConfig<'a> {
    pub binding_path: &'a std::path::Path,
    pub expected_binding_sha256: &'a str,
    pub capture_generation: CaptureGeneration,
    pub descriptor: DiagnosticRunDescriptor,
    pub policy: DiagnosticPolicy,
    pub duration_ms: u64,
    pub diagnostic_root: &'a std::path::Path,
    pub diagnostic_directory_name: Option<&'a str>,
    pub expected_source_node_id: Option<u32>,
}

pub struct GamescopeFieldObservationGateConfig<'a> {
    pub handoff: GamescopeDiagnosticHandoffGateConfig<'a>,
    pub catalog_root: &'a std::path::Path,
    pub bundle_root: &'a std::path::Path,
    pub recognition_artifact_root: Option<&'a std::path::Path>,
    pub canonical_recording_root: Option<&'a std::path::Path>,
    pub recognition_artifact_retention: RecognitionArtifactRetention,
    pub recording_memory_limit: crate::recording::policy::RecordingMemoryLimit,
    pub recording_retention: crate::recording::retention::RecordingRetention,
    pub runtime_capture: RuntimeCaptureInput<'a>,
}

pub enum RuntimeCaptureInput<'a> {
    Pipewire {
        node_name: &'a str,
        crop: EdgeCrop,
        expected_node_id: Option<u32>,
    },
    VulkanLayer {
        session: Box<scorepeek::capture::vulkan::VulkanSession>,
        crop: EdgeCrop,
    },
    #[cfg(test)]
    LegacyGamescope {
        binding_path: &'a std::path::Path,
        expected_binding_sha256: &'a str,
        expected_source_node_id: Option<u32>,
    },
}

enum CaptureLease {
    Pipewire(CalibratedGamescopeLease),
    Vulkan(CalibratedVulkanLease),
}

impl CaptureLease {
    fn capture_profile_sha256(&self) -> &str {
        match self {
            Self::Pipewire(lease) => lease.capture_profile_sha256(),
            Self::Vulkan(lease) => lease.capture_profile_sha256(),
        }
    }

    fn normalizer_artifact_sha256(&self) -> &str {
        match self {
            Self::Pipewire(lease) => lease.normalizer_artifact_sha256(),
            Self::Vulkan(lease) => lease.normalizer_artifact_sha256(),
        }
    }

    fn take_latest_observed_frame(&mut self) -> Option<scorepeek::capture::ObservedFrame> {
        match self {
            Self::Pipewire(lease) => lease.take_latest_observed_frame(),
            Self::Vulkan(lease) => lease.take_latest_observed_frame(),
        }
    }

    fn frame_normalizer(&self) -> AdmittedFrameNormalizer {
        match self {
            Self::Pipewire(lease) => lease.frame_normalizer(),
            Self::Vulkan(lease) => lease.frame_normalizer(),
        }
    }

    fn record_worker_normalization(
        &mut self,
        source_sequence: u64,
        error_type: Option<CaptureErrorType>,
        sink: &mut impl CaptureDiagnosticSink,
    ) {
        match self {
            Self::Pipewire(lease) => {
                lease.record_worker_normalization(source_sequence, error_type, sink);
            }
            Self::Vulkan(lease) => {
                lease.record_worker_normalization(source_sequence, error_type, sink);
            }
        }
    }

    fn normalize_observed_frame_with_source(
        &mut self,
        observed: scorepeek::capture::ObservedFrame,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> Result<
        (NormalizedCanonicalFrame, CalibratedSourceFrameEvidence),
        scorepeek::capture::CaptureError,
    > {
        match self {
            Self::Pipewire(lease) => lease.normalize_observed_frame_with_source(observed, sink),
            Self::Vulkan(lease) => lease.normalize_observed_frame_with_source(observed, sink),
        }
    }

    fn poll(
        &mut self,
        timeout: Duration,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> Result<(), scorepeek::capture::CaptureError> {
        match self {
            Self::Pipewire(lease) => lease.poll(timeout, sink),
            Self::Vulkan(lease) => lease.poll(timeout, sink),
        }
    }

    fn shutdown_with_elapsed(
        self,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> (Result<(), scorepeek::capture::CaptureError>, u64) {
        match self {
            Self::Pipewire(lease) => lease.shutdown_with_elapsed(sink),
            Self::Vulkan(lease) => lease.shutdown_with_elapsed(sink),
        }
    }

    fn shutdown(
        self,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> Result<(), scorepeek::capture::CaptureError> {
        self.shutdown_with_elapsed(sink).0
    }
}

impl HandoffCounters {
    fn record_offer(&mut self, outcome: DiagnosticEnqueueOutcome) {
        let counter = match outcome {
            DiagnosticEnqueueOutcome::Enqueued => &mut self.enqueued_frames,
            DiagnosticEnqueueOutcome::SkippedCadence => &mut self.skipped_cadence_frames,
            DiagnosticEnqueueOutcome::Rejected => &mut self.rejected_frames,
            DiagnosticEnqueueOutcome::Disabled => &mut self.disabled_frames,
            DiagnosticEnqueueOutcome::QueueFull => &mut self.queue_full_frames,
            DiagnosticEnqueueOutcome::WorkerUnavailable => &mut self.worker_unavailable_frames,
        };
        *counter = counter.saturating_add(1);
    }
}

impl EnqueueOutcomeCounters {
    fn record(&mut self, outcome: DiagnosticEnqueueOutcome) {
        let counter = match outcome {
            DiagnosticEnqueueOutcome::Enqueued => &mut self.enqueued,
            DiagnosticEnqueueOutcome::SkippedCadence => &mut self.skipped_cadence,
            DiagnosticEnqueueOutcome::Rejected => &mut self.rejected,
            DiagnosticEnqueueOutcome::Disabled => &mut self.disabled,
            DiagnosticEnqueueOutcome::QueueFull => &mut self.queue_full,
            DiagnosticEnqueueOutcome::WorkerUnavailable => &mut self.worker_unavailable,
        };
        *counter = counter.saturating_add(1);
    }
}

#[derive(Clone, Default)]
struct BoundedDiagnosticSink {
    facts: Vec<CaptureDiagnosticFact>,
    pending: Vec<CaptureDiagnosticFact>,
    dropped: u64,
}

impl CaptureDiagnosticSink for BoundedDiagnosticSink {
    fn record(&mut self, fact: CaptureDiagnosticFact) {
        if self.pending.len() < MAX_DIAGNOSTIC_FACTS {
            self.pending.push(fact.clone());
        }
        if self.facts.len() < MAX_DIAGNOSTIC_FACTS {
            self.facts.push(fact);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

impl BoundedDiagnosticSink {
    fn take_pending(&mut self) -> Vec<CaptureDiagnosticFact> {
        std::mem::take(&mut self.pending)
    }
}

fn emit_capture_diagnostics(
    sink: &mut BoundedDiagnosticSink,
    emit: &mut LiveEventEmitter<'_>,
) -> Result<(), FieldObservationGateErrorType> {
    for fact in sink.take_pending() {
        emit(GamescopeLiveSessionEvent::CaptureDiagnostic { fact: &fact })
            .map_err(|_| FieldObservationGateErrorType::ResultOutputFailed)?;
    }
    Ok(())
}

pub fn parse_duration_ms(value: &OsStr) -> Result<u64, String> {
    let duration = parse_u64(value, "capture live gate duration")?;
    if !(1..=MAX_GATE_DURATION_MS).contains(&duration) {
        return Err(format!(
            "capture live gate duration must be between 1 and {MAX_GATE_DURATION_MS} ms"
        ));
    }
    Ok(duration)
}

pub fn parse_consumer_interval_ms(value: &OsStr) -> Result<u64, String> {
    let interval = parse_u64(value, "capture consumer interval")?;
    if interval > MAX_CONSUMER_INTERVAL_MS {
        return Err(format!(
            "capture consumer interval must be between 0 and {MAX_CONSUMER_INTERVAL_MS} ms"
        ));
    }
    Ok(interval)
}

#[cfg(test)]
pub fn run_gamescope_binding_admission_gate(
    binding_path: &std::path::Path,
    expected_binding_sha256: &str,
) -> GamescopeBindingAdmissionGateReport {
    let binding = match read_binding(binding_path, expected_binding_sha256) {
        Ok(binding) => binding,
        Err(error_type) => {
            return binding_admission_report(error_type, None, BoundedDiagnosticSink::default());
        }
    };
    let mut sink = BoundedDiagnosticSink::default();
    let lease = match acquire_gamescope_source(DISCOVERY_TIMEOUT, &mut sink) {
        Ok(lease) => lease,
        Err(error) => {
            return binding_admission_report(
                BindingAdmissionGateErrorType::CaptureFailed,
                Some(error.error_type()),
                sink,
            );
        }
    };
    let receiver =
        match start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, &mut sink) {
            Ok(receiver) => receiver,
            Err(error) => {
                return binding_admission_report(
                    BindingAdmissionGateErrorType::CaptureFailed,
                    Some(error.error_type()),
                    sink,
                );
            }
        };
    match admit_gamescope_profile(
        receiver,
        binding,
        CaptureGeneration::new(1).expect("fixed nonzero capture generation"),
        &mut sink,
    ) {
        Ok(lease) => {
            let digests = (
                lease.capture_profile_sha256().to_owned(),
                lease.normalizer_artifact_sha256().to_owned(),
            );
            if let Err(error) = lease.shutdown(&mut sink) {
                return binding_admission_report(
                    BindingAdmissionGateErrorType::ShutdownFailed,
                    Some(error.error_type()),
                    sink,
                );
            }
            GamescopeBindingAdmissionGateReport {
                schema: "scorepeek-gamescope-binding-admission-gate-v1",
                status: LiveGateStatus::Success,
                error_type: None,
                capture_error_type: None,
                capture_profile_sha256: Some(digests.0),
                normalizer_artifact_sha256: Some(digests.1),
                diagnostic_facts: sink.facts,
                dropped_diagnostic_facts: sink.dropped,
            }
        }
        Err(failure) => {
            let error_type = failure.error_type();
            let _ = failure.shutdown(&mut sink);
            binding_admission_report(
                BindingAdmissionGateErrorType::AdmissionRejected,
                Some(error_type),
                sink,
            )
        }
    }
}

#[cfg(test)]
pub fn run_gamescope_canonical_frame_gate(
    binding_path: &std::path::Path,
    expected_binding_sha256: &str,
    capture_generation: CaptureGeneration,
) -> GamescopeCanonicalFrameGateReport {
    let binding = match read_binding(binding_path, expected_binding_sha256) {
        Ok(binding) => binding,
        Err(BindingAdmissionGateErrorType::BindingUnavailable) => {
            return canonical_frame_report(
                CanonicalFrameGateErrorType::BindingUnavailable,
                None,
                capture_generation,
                BoundedDiagnosticSink::default(),
            );
        }
        Err(_) => {
            return canonical_frame_report(
                CanonicalFrameGateErrorType::BindingInvalid,
                None,
                capture_generation,
                BoundedDiagnosticSink::default(),
            );
        }
    };
    let mut sink = BoundedDiagnosticSink::default();
    let lease = match acquire_gamescope_source(DISCOVERY_TIMEOUT, &mut sink) {
        Ok(lease) => lease,
        Err(error) => {
            return canonical_frame_report(
                CanonicalFrameGateErrorType::CaptureFailed,
                Some(error.error_type()),
                capture_generation,
                sink,
            );
        }
    };
    let receiver =
        match start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, &mut sink) {
            Ok(receiver) => receiver,
            Err(error) => {
                return canonical_frame_report(
                    CanonicalFrameGateErrorType::CaptureFailed,
                    Some(error.error_type()),
                    capture_generation,
                    sink,
                );
            }
        };
    let mut lease = match admit_gamescope_profile(receiver, binding, capture_generation, &mut sink)
    {
        Ok(lease) => lease,
        Err(failure) => {
            let error_type = failure.error_type();
            let _ = failure.shutdown(&mut sink);
            return canonical_frame_report(
                CanonicalFrameGateErrorType::AdmissionRejected,
                Some(error_type),
                capture_generation,
                sink,
            );
        }
    };
    let Some(observed) = lease.take_latest_observed_frame() else {
        let _ = lease.shutdown(&mut sink);
        return canonical_frame_report(
            CanonicalFrameGateErrorType::FrameUnavailable,
            None,
            capture_generation,
            sink,
        );
    };
    let canonical = match lease.normalize_observed_frame(observed, &mut sink) {
        Ok(frame) => frame,
        Err(error) => {
            let error_type = error.error_type();
            let _ = lease.shutdown(&mut sink);
            return canonical_frame_report(
                CanonicalFrameGateErrorType::NormalizationFailed,
                Some(error_type),
                capture_generation,
                sink,
            );
        }
    };
    let capture_profile_sha256 = canonical.capture_profile_sha256().to_owned();
    let normalizer_artifact_sha256 = canonical.normalizer_artifact_sha256().to_owned();
    let source_sequence = canonical.source_sequence();
    let canonical_rgb8_sha256 = encode_sha256(canonical.pixels());
    if let Err(error) = lease.shutdown(&mut sink) {
        return canonical_frame_report(
            CanonicalFrameGateErrorType::ShutdownFailed,
            Some(error.error_type()),
            capture_generation,
            sink,
        );
    }
    canonical_frame_success(
        capture_generation,
        capture_profile_sha256,
        normalizer_artifact_sha256,
        source_sequence,
        canonical_rgb8_sha256,
        sink,
    )
}

#[cfg(test)]
pub fn run_gamescope_diagnostic_handoff_gate(
    config: GamescopeDiagnosticHandoffGateConfig<'_>,
) -> GamescopeDiagnosticHandoffGateReport {
    run_gamescope_handoff_gate(config, false).diagnostic
}

#[cfg(test)]
pub fn run_gamescope_recognition_handoff_gate(
    config: GamescopeDiagnosticHandoffGateConfig<'_>,
) -> GamescopeRecognitionHandoffGateReport {
    recognition_handoff_report(run_gamescope_handoff_gate(config, true))
}

#[path = "live/field_observation.rs"]
mod field_observation;

#[cfg(test)]
pub(crate) use field_observation::run_gamescope_field_observation_gate;
pub(crate) use field_observation::run_runtime_live_session;
#[cfg(test)]
use field_observation::{
    FieldObservationFinishOutcomes, field_observation_report, field_resource_error,
    field_start_error, recognition_artifact_error, reconnectable_stop_reason,
    result_evidence_error,
};

#[path = "live/handoff.rs"]
mod handoff;

#[allow(
    clippy::wildcard_imports,
    reason = "handoff is an implementation partition shared with live capture child modules"
)]
use handoff::*;

#[cfg(test)]
#[path = "live/tests.rs"]
mod tests;

fn canonical_frame_report(
    error_type: CanonicalFrameGateErrorType,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: CaptureGeneration,
    sink: BoundedDiagnosticSink,
) -> GamescopeCanonicalFrameGateReport {
    GamescopeCanonicalFrameGateReport {
        schema: "scorepeek-gamescope-canonical-frame-gate-v1",
        status: LiveGateStatus::Error,
        error_type: Some(error_type),
        capture_error_type,
        capture_generation: capture_generation.get(),
        capture_profile_sha256: None,
        normalizer_artifact_sha256: None,
        source_sequence: None,
        canonical_rgb8_sha256: None,
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}

fn encode_sha256(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn read_binding(
    path: &std::path::Path,
    expected_sha256: &str,
) -> Result<GamescopeProfileBinding, BindingAdmissionGateErrorType> {
    let file = File::open(path).map_err(|_| BindingAdmissionGateErrorType::BindingUnavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| BindingAdmissionGateErrorType::BindingUnavailable)?;
    let maximum_bytes = u64::try_from(MAX_BINDING_BYTES).unwrap_or(u64::MAX);
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(BindingAdmissionGateErrorType::BindingInvalid);
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| BindingAdmissionGateErrorType::BindingInvalid)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| BindingAdmissionGateErrorType::BindingUnavailable)?;
    if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
        return Err(BindingAdmissionGateErrorType::BindingInvalid);
    }
    GamescopeProfileBinding::parse(&bytes, expected_sha256)
        .map_err(|_| BindingAdmissionGateErrorType::BindingInvalid)
}

fn binding_admission_report(
    error_type: BindingAdmissionGateErrorType,
    capture_error_type: Option<CaptureErrorType>,
    sink: BoundedDiagnosticSink,
) -> GamescopeBindingAdmissionGateReport {
    GamescopeBindingAdmissionGateReport {
        schema: "scorepeek-gamescope-binding-admission-gate-v1",
        status: LiveGateStatus::Error,
        error_type: Some(error_type),
        capture_error_type,
        capture_profile_sha256: None,
        normalizer_artifact_sha256: None,
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}

#[path = "live/lifecycle_gate.rs"]
mod lifecycle_gate;

pub(crate) use lifecycle_gate::*;
#[path = "live/normalization_worker.rs"]
mod normalization_worker;

#[allow(
    clippy::wildcard_imports,
    reason = "the worker is an implementation partition of the live capture authority"
)]
use normalization_worker::*;
