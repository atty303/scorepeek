use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Read as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use scorepeek::capture::{
    CalibratedGamescopeLease, CaptureDiagnosticDetail, CaptureDiagnosticFact,
    CaptureDiagnosticOperation, CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType,
    CaptureGeneration, GamescopeProfileBinding, acquire_gamescope_source, admit_gamescope_profile,
    start_uncalibrated_gamescope_receiver,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::canonical_source::CanonicalFrameSource;
use crate::diagnostic_live::{BoundCanonicalFrame, DiagnosticBridge};
use crate::diagnostic_recording::{
    DiagnosticCompleteness, DiagnosticErrorType, DiagnosticPolicy, DiagnosticRunDescriptor,
    DiagnosticRunStatus,
};
use crate::diagnostic_worker::DiagnosticEnqueueOutcome;
use crate::recognition_artifact::{
    RecognitionArtifactEnqueueOutcome, RecognitionArtifactFinishOutcome,
    RecognitionArtifactFinishStatus, RecognitionArtifactRetention, RecognitionArtifactWorker,
};
use crate::recognition_live::RecognitionSession;
use crate::recognition_live::field_observer::{
    DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT, FieldObserverFinishStatus, FieldObserverOfferError,
};
use crate::recognition_live::field_session::{
    FieldObservationSession, FieldObservationSessionPoll, FieldObservationStartError,
    FieldObservationSubmission, PendingSessionFieldObservation,
};
use crate::recognition_live::screen_field_observer::{
    RegisteredScreenFieldObservation, RegisteredScreenFieldObserver,
    RegisteredScreenFieldObserverLoadError,
};
use scorepeek::recognition::{
    CanonicalLayout, OnnxParityError, RegisteredResourceLoadErrorType, ScreenClass,
    ScreenFieldObservationError,
};
use scorepeek::recognition_cadence::{CadenceDecision, RecognitionCadence};

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
    BindingUnavailable,
    BindingInvalid,
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
    TerminalFailure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveSessionStartupRetry {
    Admission,
    Catalog,
}

#[derive(Clone, Copy)]
pub enum GamescopeLiveSessionEvent<'a> {
    Started {
        capture_generation: u64,
        capture_profile_sha256: &'a str,
        normalizer_artifact_sha256: &'a str,
    },
    ScreenChanged {
        screen_episode_id: u64,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
    },
    ScreenTick {
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
        timing: crate::recognition_live::FrameProcessingTiming,
    },
    Observation {
        screen_episode_id: u64,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        output: &'a RegisteredScreenFieldObservation,
    },
}

pub use crate::recognition_live::LiveEventProcessingTiming;

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

    pub fn startup_failure_summary(&self) -> String {
        self.failure_detail.clone().unwrap_or_else(|| {
            format!(
                "Gamescope live session startup failed: {:?}",
                self.error_type
            )
        })
    }

    pub fn diagnostic_manifest_sha256(&self) -> Option<&str> {
        self.diagnostic_manifest_sha256.as_deref()
    }

    pub fn recognition_artifact_manifest_sha256(&self) -> Option<&str> {
        self.recognition_artifact_manifest_sha256.as_deref()
    }

    pub const fn recognition_sampling(&self) -> (u64, u64, u64) {
        (
            self.recognition_ticks,
            self.recognition_busy_skips,
            self.maximum_consecutive_busy_skips,
        )
    }

    pub const fn field_busy_sampling(&self) -> (u64, u64) {
        (
            self.field_observation_busy_skips,
            self.maximum_consecutive_field_observation_busy_skips,
        )
    }

    pub const fn diagnostic_completeness_name(&self) -> &'static str {
        match self.diagnostic_completeness {
            Some(DiagnosticCompleteness::Complete) => "complete",
            Some(DiagnosticCompleteness::Partial) => "partial",
            Some(DiagnosticCompleteness::Dropped) | None => "dropped",
        }
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
    ) -> crate::diagnostic_recording::DiagnosticFinishOutcome {
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
    pub expected_source_node_id: Option<u32>,
}

pub struct GamescopeFieldObservationGateConfig<'a> {
    pub handoff: GamescopeDiagnosticHandoffGateConfig<'a>,
    pub catalog_root: &'a std::path::Path,
    pub bundle_root: &'a std::path::Path,
    pub recognition_artifact_root: Option<&'a std::path::Path>,
    pub recognition_artifact_retention: RecognitionArtifactRetention,
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
    dropped: u64,
}

impl CaptureDiagnosticSink for BoundedDiagnosticSink {
    fn record(&mut self, fact: CaptureDiagnosticFact) {
        if self.facts.len() < MAX_DIAGNOSTIC_FACTS {
            self.facts.push(fact);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
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

pub fn run_gamescope_diagnostic_handoff_gate(
    config: GamescopeDiagnosticHandoffGateConfig<'_>,
) -> GamescopeDiagnosticHandoffGateReport {
    run_gamescope_handoff_gate(config, false).diagnostic
}

pub fn run_gamescope_recognition_handoff_gate(
    config: GamescopeDiagnosticHandoffGateConfig<'_>,
) -> GamescopeRecognitionHandoffGateReport {
    recognition_handoff_report(run_gamescope_handoff_gate(config, true))
}

type RegisteredFieldOutput =
    Result<RegisteredScreenFieldObservation, ScreenFieldObservationError<OnnxParityError>>;

pub fn run_gamescope_field_observation_gate(
    config: GamescopeFieldObservationGateConfig<'_>,
) -> GamescopeFieldObservationGateReport {
    let capture_generation = config.handoff.capture_generation;
    let duration = Duration::from_millis(config.handoff.duration_ms);
    let StartedFieldObservationGate {
        mut lease,
        mut session,
        mut artifact_worker,
        artifact_requested,
        mut sink,
    } = match start_field_observation_gate(config) {
        Ok(started) => started,
        Err(report) => return *report,
    };

    let mut counters = FieldObservationCounters::default();
    let mut pending = Vec::<PendingSessionFieldObservation<RegisteredFieldOutput>>::new();
    let mut terminal = offer_field_observation_frames(
        &mut lease,
        &mut session,
        duration,
        &mut pending,
        &mut counters,
        &mut artifact_worker,
        &mut sink,
    );
    if counters.normalized_frames == 0 && terminal.is_none() {
        terminal = Some((FieldObservationGateErrorType::FrameUnavailable, None));
    }
    let (shutdown, finish_time) = lease.shutdown_with_elapsed(&mut sink);
    let post_capture_started = Instant::now();
    if let Err(error) = shutdown {
        terminal.get_or_insert((
            FieldObservationGateErrorType::ShutdownFailed,
            Some(error.error_type()),
        ));
    }
    if terminal.is_none() {
        terminal = wait_field_observations(
            &mut session,
            &mut pending,
            &mut counters,
            &mut artifact_worker,
        );
    }
    if counters.candidate_sets == 0 && terminal.is_none() {
        terminal = Some((
            FieldObservationGateErrorType::FieldObservationUnavailable,
            None,
        ));
    }
    if artifact_requested && terminal.is_none() {
        terminal = result_evidence_error(&counters).map(|error| (error, None));
    }
    let finish_status = if terminal.is_none() {
        DiagnosticRunStatus::Success
    } else {
        DiagnosticRunStatus::Error
    };
    let outcome = session.finish_after_capture(
        finish_status,
        finish_time,
        post_capture_started.elapsed(),
        DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT,
    );
    if outcome.field_observer.status != FieldObserverFinishStatus::Complete {
        terminal.get_or_insert((
            FieldObservationGateErrorType::FieldObserverFinishFailed,
            None,
        ));
    }
    let artifact_outcome = artifact_worker.map(|worker| worker.finish(terminal.is_none()));
    if artifact_requested && terminal.is_none() {
        terminal = recognition_artifact_error(&counters, artifact_outcome.as_ref())
            .map(|error| (error, None));
    }
    let (error_type, capture_error_type) = terminal.unzip();
    field_observation_report(
        error_type,
        capture_error_type.flatten(),
        capture_generation,
        counters,
        FieldObservationFinishOutcomes {
            field_observer: Some(outcome.field_observer),
            diagnostic: Some(outcome.diagnostic),
            recognition_artifact: artifact_outcome,
            artifact_requested,
        },
        sink,
    )
}

#[allow(clippy::too_many_lines)]
pub fn run_gamescope_live_session(
    config: GamescopeFieldObservationGateConfig<'_>,
    stop: &AtomicBool,
    emit: &mut LiveEventEmitter<'_>,
) -> GamescopeFieldObservationGateReport {
    let capture_generation = config.handoff.capture_generation;
    let StartedFieldObservationGate {
        mut lease,
        mut session,
        mut artifact_worker,
        artifact_requested,
        mut sink,
    } = match start_field_observation_gate(config) {
        Ok(started) => started,
        Err(mut report) => {
            report.schema = "scorepeek-gamescope-live-session-v1";
            report.session_stop_reason = Some(LiveSessionStopReason::TerminalFailure);
            return *report;
        }
    };

    let mut terminal = emit(GamescopeLiveSessionEvent::Started {
        capture_generation: capture_generation.get(),
        capture_profile_sha256: lease.capture_profile_sha256(),
        normalizer_artifact_sha256: lease.normalizer_artifact_sha256(),
    })
    .err()
    .map(|_| (FieldObservationGateErrorType::ResultOutputFailed, None));
    let mut counters = FieldObservationCounters::default();
    let mut pending = Vec::<PendingSessionFieldObservation<RegisteredFieldOutput>>::new();
    let mut minimum_event_sequence = None;
    if terminal.is_none() {
        terminal = offer_live_field_observation_frames(
            &mut lease,
            &mut session,
            stop,
            &mut pending,
            &mut counters,
            &mut artifact_worker,
            &mut sink,
            emit,
            &mut minimum_event_sequence,
        );
    }
    let (shutdown, finish_time) = lease.shutdown_with_elapsed(&mut sink);
    let post_capture_started = Instant::now();
    if let Err(error) = shutdown {
        terminal.get_or_insert((
            FieldObservationGateErrorType::ShutdownFailed,
            Some(error.error_type()),
        ));
    }
    let output_available = !matches!(
        terminal,
        Some((FieldObservationGateErrorType::ResultOutputFailed, _))
    );
    let drain_error = wait_live_field_observations(
        &mut session,
        &mut pending,
        &mut counters,
        &mut artifact_worker,
        output_available.then_some(emit),
        minimum_event_sequence,
    );
    if let Some(error) = drain_error {
        if matches!(
            terminal,
            Some((
                FieldObservationGateErrorType::CaptureFailed,
                Some(CaptureErrorType::SourceLost)
            ))
        ) {
            terminal = Some(error);
        } else {
            terminal.get_or_insert(error);
        }
    }
    session.record_sampling_summary(
        counters.last_recognition_sequence.unwrap_or(0),
        finish_time,
        crate::diagnostic_recording::RecognitionSamplingSummary {
            processed_ticks: counters.recognition_ticks,
            busy_skips: counters.recognition_busy_skips,
            maximum_consecutive_busy_skips: counters.maximum_consecutive_busy_skips,
            field_observation_busy_skips: counters.field_observation_busy_skips,
            maximum_consecutive_field_observation_busy_skips: counters
                .maximum_consecutive_field_observation_busy_skips,
        },
    );
    let mut source_ended = matches!(
        terminal,
        Some((
            FieldObservationGateErrorType::CaptureFailed,
            Some(CaptureErrorType::SourceLost)
        ))
    );
    let finish_status = if terminal.is_none() || source_ended {
        DiagnosticRunStatus::Success
    } else {
        DiagnosticRunStatus::Error
    };
    let outcome = session.finish_after_capture(
        finish_status,
        finish_time,
        post_capture_started.elapsed(),
        DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT,
    );
    if outcome.field_observer.status != FieldObserverFinishStatus::Complete {
        terminal = Some((
            FieldObservationGateErrorType::FieldObserverFinishFailed,
            None,
        ));
        source_ended = false;
    }
    let artifact_outcome =
        artifact_worker.map(|worker| worker.finish(terminal.is_none() || source_ended));
    let stop_reason = if source_ended {
        LiveSessionStopReason::SourceEnded
    } else if terminal.is_none() {
        LiveSessionStopReason::RequestedSignal
    } else {
        LiveSessionStopReason::TerminalFailure
    };
    let (error_type, capture_error_type) = if source_ended {
        (None, Some(Some(CaptureErrorType::SourceLost)))
    } else {
        terminal.unzip()
    };
    let mut report = field_observation_report(
        error_type,
        capture_error_type.flatten(),
        capture_generation,
        counters,
        FieldObservationFinishOutcomes {
            field_observer: Some(outcome.field_observer),
            diagnostic: Some(outcome.diagnostic),
            recognition_artifact: artifact_outcome,
            artifact_requested,
        },
        sink,
    );
    report.schema = "scorepeek-gamescope-live-session-v1";
    report.session_stop_reason = Some(stop_reason);
    report
}

struct StartedFieldObservationGate {
    lease: CalibratedGamescopeLease,
    session: FieldObservationSession<RegisteredScreenFieldObserver>,
    artifact_worker: Option<RecognitionArtifactWorker>,
    artifact_requested: bool,
    sink: BoundedDiagnosticSink,
}

#[allow(clippy::too_many_lines)]
fn start_field_observation_gate(
    mut config: GamescopeFieldObservationGateConfig<'_>,
) -> Result<StartedFieldObservationGate, Box<GamescopeFieldObservationGateReport>> {
    let capture_generation = config.handoff.capture_generation;
    let artifact_requested = config.recognition_artifact_root.is_some();
    let sink = BoundedDiagnosticSink::default();
    if !valid_handoff_descriptor(&config.handoff.descriptor, capture_generation, true) {
        return Err(Box::new(empty_field_observation_report(
            FieldObservationGateErrorType::DiagnosticConfigurationInvalid,
            None,
            capture_generation,
            None,
            artifact_requested,
            sink,
        )));
    }
    let binding = read_diagnostic_handoff_binding(
        config.handoff.binding_path,
        config.handoff.expected_binding_sha256,
    )
    .map_err(|error| {
        let error = match error {
            DiagnosticHandoffGateErrorType::BindingUnavailable => {
                FieldObservationGateErrorType::BindingUnavailable
            }
            _ => FieldObservationGateErrorType::BindingInvalid,
        };
        Box::new(empty_field_observation_report(
            error,
            None,
            capture_generation,
            None,
            artifact_requested,
            sink.clone(),
        ))
    })?;
    bind_diagnostic_descriptor(&mut config.handoff.descriptor, &binding);
    let expected_profile = config
        .handoff
        .descriptor
        .binding
        .capture_profile_sha256
        .clone();
    let expected_normalizer = config.handoff.descriptor.binding.normalizer_sha256.clone();
    let artifact_run_id = config.handoff.descriptor.run_id.clone();
    let mut sink = sink;
    let lease = match start_diagnostic_handoff_capture(
        binding,
        capture_generation,
        config.handoff.expected_source_node_id,
        &mut sink,
    ) {
        Ok(lease) => lease,
        Err((error, capture_error)) => {
            let error = match error {
                DiagnosticHandoffGateErrorType::AdmissionRejected => {
                    FieldObservationGateErrorType::AdmissionRejected
                }
                _ => FieldObservationGateErrorType::CaptureFailed,
            };
            return Err(Box::new(empty_field_observation_report(
                error,
                capture_error,
                capture_generation,
                None,
                artifact_requested,
                sink,
            )));
        }
    };
    if expected_profile != lease.capture_profile_sha256()
        || expected_normalizer != lease.normalizer_artifact_sha256()
    {
        let lease = lease;
        let (shutdown, _) = lease.shutdown_with_elapsed(&mut sink);
        return Err(Box::new(empty_field_observation_report(
            FieldObservationGateErrorType::DiagnosticBindingMismatch,
            shutdown.err().map(|error| error.error_type()),
            capture_generation,
            None,
            artifact_requested,
            sink,
        )));
    }
    let session = match FieldObservationSession::start_registered(
        config.handoff.diagnostic_root,
        config.handoff.descriptor,
        config.handoff.policy,
        config.catalog_root,
        config.bundle_root,
        crate::recognition_live::text_observer_pool::RecognitionExecutionMode::Live,
    ) {
        Ok(session) => session,
        Err(error) => {
            let lease = lease;
            let (shutdown, _) = lease.shutdown_with_elapsed(&mut sink);
            let (error_type, field_finish, detail) = field_start_error(error);
            let mut report = empty_field_observation_report(
                error_type,
                shutdown.err().map(|error| error.error_type()),
                capture_generation,
                field_finish,
                artifact_requested,
                sink,
            );
            report.failure_detail = detail;
            return Err(Box::new(report));
        }
    };
    let artifact_worker =
        config
            .recognition_artifact_root
            .map(|root| match config.recognition_artifact_retention {
                RecognitionArtifactRetention::Complete => RecognitionArtifactWorker::start(
                    root.to_owned(),
                    artifact_run_id,
                    expected_profile.clone(),
                ),
                RecognitionArtifactRetention::ForegroundCompactedV1 => {
                    RecognitionArtifactWorker::start_foreground(
                        root.to_owned(),
                        artifact_run_id,
                        expected_profile.clone(),
                    )
                }
            });
    Ok(StartedFieldObservationGate {
        lease,
        session,
        artifact_worker,
        artifact_requested,
        sink,
    })
}

fn empty_field_observation_report(
    error_type: FieldObservationGateErrorType,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: CaptureGeneration,
    field_observer: Option<crate::recognition_live::field_observer::FieldObserverFinishOutcome>,
    artifact_requested: bool,
    sink: BoundedDiagnosticSink,
) -> GamescopeFieldObservationGateReport {
    field_observation_report(
        Some(error_type),
        capture_error_type,
        capture_generation,
        FieldObservationCounters::default(),
        FieldObservationFinishOutcomes {
            field_observer,
            diagnostic: None,
            recognition_artifact: None,
            artifact_requested,
        },
        sink,
    )
}

fn field_start_error(
    error: FieldObservationStartError<RegisteredScreenFieldObserverLoadError>,
) -> (
    FieldObservationGateErrorType,
    Option<crate::recognition_live::field_observer::FieldObserverFinishOutcome>,
    Option<String>,
) {
    use crate::recognition_live::field_observer::FieldObserverStartError;
    match error {
        FieldObservationStartError::FieldObserver(error) => match error {
            FieldObserverStartError::InvalidBinding => (
                FieldObservationGateErrorType::DiagnosticConfigurationInvalid,
                None,
                None,
            ),
            FieldObserverStartError::Load(RegisteredScreenFieldObserverLoadError::Resources(
                error,
            )) => (
                field_resource_error(error.error_type()),
                None,
                Some(error.to_string()),
            ),
            FieldObserverStartError::Load(
                RegisteredScreenFieldObserverLoadError::CandidateDomain(error),
            ) => (
                FieldObservationGateErrorType::CandidateDomainInvalid,
                None,
                Some(format!(
                    "catalog candidate domain is invalid for song {}: {error}",
                    error.song_id.as_uuid()
                )),
            ),
            FieldObserverStartError::Load(
                RegisteredScreenFieldObserverLoadError::NumericModel(error),
            ) => (
                FieldObservationGateErrorType::NumericModelUnavailable,
                None,
                Some(error.to_string()),
            ),
            FieldObserverStartError::Load(RegisteredScreenFieldObserverLoadError::TextRuntime(
                error,
            )) => (
                FieldObservationGateErrorType::FieldObserverUnavailable,
                None,
                Some(error.to_string()),
            ),
            FieldObserverStartError::WorkerUnavailable => (
                FieldObservationGateErrorType::FieldObserverUnavailable,
                None,
                None,
            ),
        },
        FieldObservationStartError::Recognition {
            field_observer_finish,
            ..
        } => (
            FieldObservationGateErrorType::DiagnosticConfigurationInvalid,
            Some(field_observer_finish),
            None,
        ),
    }
}

const fn field_resource_error(
    error: RegisteredResourceLoadErrorType,
) -> FieldObservationGateErrorType {
    match error {
        RegisteredResourceLoadErrorType::InvalidLocation => {
            FieldObservationGateErrorType::InvalidResourceLocation
        }
        RegisteredResourceLoadErrorType::ModelBindingMismatch => {
            FieldObservationGateErrorType::ModelBindingMismatch
        }
        RegisteredResourceLoadErrorType::RuntimeBindingMismatch => {
            FieldObservationGateErrorType::RuntimeBindingMismatch
        }
        RegisteredResourceLoadErrorType::CatalogUnavailable => {
            FieldObservationGateErrorType::CatalogUnavailable
        }
        RegisteredResourceLoadErrorType::CatalogBindingMismatch => {
            FieldObservationGateErrorType::CatalogBindingMismatch
        }
        RegisteredResourceLoadErrorType::CatalogLoadFailed => {
            FieldObservationGateErrorType::CatalogLoadFailed
        }
        RegisteredResourceLoadErrorType::ModelBundleInvalid => {
            FieldObservationGateErrorType::ModelBundleInvalid
        }
        RegisteredResourceLoadErrorType::RuntimeInitializationFailed => {
            FieldObservationGateErrorType::RuntimeInitializationFailed
        }
    }
}

fn offer_field_observation_frames(
    lease: &mut CalibratedGamescopeLease,
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    duration: Duration,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
    sink: &mut BoundedDiagnosticSink,
) -> Option<(FieldObservationGateErrorType, Option<CaptureErrorType>)> {
    let mut source = GamescopeCanonicalFrameSource {
        lease,
        counters,
        sink,
    };
    let started = Instant::now();
    loop {
        let remaining = duration.saturating_sub(started.elapsed());
        let frame = match source.next_frame(remaining) {
            Ok(frame) => frame,
            Err(error) => return Some(error),
        };
        if let Some(frame) = frame {
            let Ok(result) = session.inspect(&frame) else {
                return Some((FieldObservationGateErrorType::RecognitionFailed, None));
            };
            source.counters.inspected_frames = source.counters.inspected_frames.saturating_add(1);
            let screen_counter = match result.observation.screen() {
                ScreenClass::Result => &mut source.counters.result_frames,
                ScreenClass::MusicSelect => &mut source.counters.music_select_frames,
                ScreenClass::ModeSelect => &mut source.counters.mode_select_frames,
                ScreenClass::DecideTransition => &mut source.counters.decide_transition_frames,
                ScreenClass::Play => &mut source.counters.play_frames,
                ScreenClass::Unknown => &mut source.counters.unknown_frames,
            };
            *screen_counter = screen_counter.saturating_add(1);
            match result.field_submission {
                FieldObservationSubmission::BusySkipped => {
                    let _ = session.record_frame_processing_timing(
                        result.timing,
                        crate::diagnostic_recording::FrameFieldStatus::BusySkip,
                        None,
                    );
                    unreachable!("offline gate has no pending OCR policy")
                }
                FieldObservationSubmission::NotApplicable => {
                    let _ = session.record_frame_processing_timing(
                        result.timing,
                        crate::diagnostic_recording::FrameFieldStatus::NotApplicable,
                        None,
                    );
                    source.counters.field_not_applicable =
                        source.counters.field_not_applicable.saturating_add(1);
                }
                FieldObservationSubmission::Submitted(observation) => {
                    source.counters.field_submitted =
                        source.counters.field_submitted.saturating_add(1);
                    pending.push(observation);
                }
                FieldObservationSubmission::Rejected(error) => {
                    let _ = session.record_frame_processing_timing(
                        result.timing,
                        crate::diagnostic_recording::FrameFieldStatus::Failed,
                        None,
                    );
                    source.counters.field_rejected =
                        source.counters.field_rejected.saturating_add(1);
                    if matches!(
                        error,
                        FieldObserverOfferError::BindingMismatch
                            | FieldObserverOfferError::WorkerUnavailable
                    ) {
                        return Some((
                            FieldObservationGateErrorType::FieldObserverUnavailable,
                            None,
                        ));
                    }
                }
            }
            if let Some(error) = poll_field_observations(
                session,
                pending,
                source.counters,
                artifact_worker,
                None,
                None,
                None,
            ) {
                return Some((error, None));
            }
        }
        if started.elapsed() >= duration {
            return None;
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the live loop keeps screen cadence, field admission, event ordering, and counters in one owner"
)]
fn offer_live_field_observation_frames(
    lease: &mut CalibratedGamescopeLease,
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    stop: &AtomicBool,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
    sink: &mut BoundedDiagnosticSink,
    emit: &mut LiveEventEmitter<'_>,
    minimum_event_sequence: &mut Option<u64>,
) -> Option<(FieldObservationGateErrorType, Option<CaptureErrorType>)> {
    let mut cadence = RecognitionCadence::default();
    let mut last_emitted_screen = None;
    let mut screen_episode_id = 0_u64;
    let mut source = GamescopeCanonicalFrameSource {
        lease,
        counters,
        sink,
    };
    while !stop.load(Ordering::Acquire) {
        if let Some(error) = poll_field_observations(
            session,
            pending,
            source.counters,
            artifact_worker,
            None,
            Some(emit),
            *minimum_event_sequence,
        ) {
            return Some((error, None));
        }
        let frame = match source.next_frame(LIVE_SESSION_POLL_INTERVAL) {
            Ok(frame) => frame,
            Err(error) => return Some(error),
        };
        if let Some(mut frame) = frame {
            match cadence.observe(frame.monotonic_end_ms()) {
                CadenceDecision::SkipCadence => continue,
                CadenceDecision::Process { tick_sequence } => {
                    source.counters.recognition_ticks = cadence.processed();
                    source.counters.last_recognition_sequence = Some(tick_sequence);
                    frame.assign_tick_sequence(tick_sequence);
                }
            }
            let field_busy = !pending.is_empty();
            let inspected = if field_busy {
                session.inspect_while_field_busy(&frame)
            } else {
                session.inspect(&frame)
            };
            let Ok(result) = inspected else {
                return Some((FieldObservationGateErrorType::RecognitionFailed, None));
            };
            source.counters.inspected_frames = source.counters.inspected_frames.saturating_add(1);
            let screen_counter = match result.observation.screen() {
                ScreenClass::Result => &mut source.counters.result_frames,
                ScreenClass::MusicSelect => &mut source.counters.music_select_frames,
                ScreenClass::ModeSelect => &mut source.counters.mode_select_frames,
                ScreenClass::DecideTransition => &mut source.counters.decide_transition_frames,
                ScreenClass::Play => &mut source.counters.play_frames,
                ScreenClass::Unknown => &mut source.counters.unknown_frames,
            };
            *screen_counter = screen_counter.saturating_add(1);
            let screen = result.observation.screen();
            let screen_changed = last_emitted_screen != Some(screen);
            if screen_changed {
                screen_episode_id = screen_episode_id.saturating_add(1);
                if last_emitted_screen.is_some() {
                    *minimum_event_sequence = Some(frame.sequence());
                }
            }
            let mut live_timing = LiveEventProcessingTiming::default();
            let mut output_failed = false;
            if screen_changed {
                match emit(GamescopeLiveSessionEvent::ScreenChanged {
                    screen_episode_id,
                    sequence: frame.sequence(),
                    monotonic_start_ms: frame.monotonic_start_ms(),
                    monotonic_end_ms: frame.monotonic_end_ms(),
                    screen,
                }) {
                    Ok(timing) => {
                        live_timing.add(timing);
                        last_emitted_screen = Some(screen);
                    }
                    Err(_) => output_failed = true,
                }
            }
            if !output_failed {
                match emit(GamescopeLiveSessionEvent::ScreenTick {
                    screen_episode_id,
                    sequence: frame.sequence(),
                    monotonic_end_ms: frame.monotonic_end_ms(),
                    screen,
                    timing: result.timing,
                }) {
                    Ok(timing) => live_timing.add(timing),
                    Err(_) => output_failed = true,
                }
            }
            let mut frame_timing = result.timing;
            frame_timing.add_live_processing(live_timing);
            let mut field_terminal = None;
            match result.field_submission {
                FieldObservationSubmission::BusySkipped => {
                    let _ = session.record_frame_processing_timing(
                        frame_timing,
                        crate::diagnostic_recording::FrameFieldStatus::BusySkip,
                        None,
                    );
                    source.counters.field_observation_busy_skips = source
                        .counters
                        .field_observation_busy_skips
                        .saturating_add(1);
                    source.counters.consecutive_field_observation_busy_skips = source
                        .counters
                        .consecutive_field_observation_busy_skips
                        .saturating_add(1);
                    source
                        .counters
                        .maximum_consecutive_field_observation_busy_skips = source
                        .counters
                        .maximum_consecutive_field_observation_busy_skips
                        .max(source.counters.consecutive_field_observation_busy_skips);
                }
                FieldObservationSubmission::NotApplicable => {
                    let _ = session.record_frame_processing_timing(
                        frame_timing,
                        crate::diagnostic_recording::FrameFieldStatus::NotApplicable,
                        None,
                    );
                    source.counters.consecutive_field_observation_busy_skips = 0;
                    source.counters.field_not_applicable =
                        source.counters.field_not_applicable.saturating_add(1);
                }
                FieldObservationSubmission::Submitted(mut observation) => {
                    observation.bind_screen_episode(screen_episode_id);
                    observation.add_live_processing(live_timing);
                    source.counters.consecutive_field_observation_busy_skips = 0;
                    source.counters.field_submitted =
                        source.counters.field_submitted.saturating_add(1);
                    pending.push(observation);
                }
                FieldObservationSubmission::Rejected(error) => {
                    let _ = session.record_frame_processing_timing(
                        frame_timing,
                        crate::diagnostic_recording::FrameFieldStatus::Failed,
                        None,
                    );
                    source.counters.consecutive_field_observation_busy_skips = 0;
                    source.counters.field_rejected =
                        source.counters.field_rejected.saturating_add(1);
                    if matches!(
                        error,
                        FieldObserverOfferError::BindingMismatch
                            | FieldObserverOfferError::WorkerUnavailable
                    ) {
                        field_terminal = Some((
                            FieldObservationGateErrorType::FieldObserverUnavailable,
                            None,
                        ));
                    }
                }
            }
            if output_failed {
                return Some((FieldObservationGateErrorType::ResultOutputFailed, None));
            }
            if let Some(error) = field_terminal {
                return Some(error);
            }
        }
    }
    None
}

struct GamescopeCanonicalFrameSource<'a> {
    lease: &'a mut CalibratedGamescopeLease,
    counters: &'a mut FieldObservationCounters,
    sink: &'a mut BoundedDiagnosticSink,
}

impl CanonicalFrameSource for GamescopeCanonicalFrameSource<'_> {
    type Error = (FieldObservationGateErrorType, Option<CaptureErrorType>);

    fn next_frame(
        &mut self,
        maximum_wait: Duration,
    ) -> Result<Option<BoundCanonicalFrame>, Self::Error> {
        if let Some(observed) = self.lease.take_latest_observed_frame() {
            self.counters.observed_frames = self.counters.observed_frames.saturating_add(1);
            let (normalized, source) = self
                .lease
                .normalize_observed_frame_with_source(observed, self.sink)
                .map_err(|error| {
                    (
                        FieldObservationGateErrorType::NormalizationFailed,
                        Some(error.error_type()),
                    )
                })?;
            self.counters.normalized_frames = self.counters.normalized_frames.saturating_add(1);
            return Ok(Some(BoundCanonicalFrame::from_normalized_with_source(
                normalized, source,
            )));
        }
        self.lease.poll(maximum_wait, self.sink).map_err(|error| {
            (
                FieldObservationGateErrorType::CaptureFailed,
                Some(error.error_type()),
            )
        })?;
        Ok(None)
    }
}

fn wait_field_observations(
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
) -> Option<(FieldObservationGateErrorType, Option<CaptureErrorType>)> {
    let started = Instant::now();
    while !pending.is_empty() {
        let remaining = DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        if let Some(error) = poll_field_observations(
            session,
            pending,
            counters,
            artifact_worker,
            Some(remaining),
            None,
            None,
        ) {
            return Some((error, None));
        }
    }
    None
}

#[allow(
    clippy::too_many_lines,
    reason = "one polling boundary preserves ordered completion, timing, and diagnostic outcomes"
)]
fn poll_field_observations(
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
    wait: Option<Duration>,
    mut emit: Option<&mut LiveEventEmitter<'_>>,
    minimum_event_sequence: Option<u64>,
) -> Option<FieldObservationGateErrorType> {
    let mut index = 0;
    while index < pending.len() {
        let poll = match wait {
            Some(timeout) => session.wait_field_observation(&pending[index], timeout),
            None => session.poll_field_observation(&pending[index]),
        };
        match poll {
            FieldObservationSessionPoll::Pending => {
                if wait.is_some() {
                    return None;
                }
                index += 1;
            }
            FieldObservationSessionPoll::Ready {
                observation,
                mut timing,
                screen_episode_id,
                ..
            } => {
                pending.swap_remove(index);
                let sequence = observation.sequence();
                let monotonic_start_ms = observation.monotonic_start_ms();
                let monotonic_end_ms = observation.monotonic_end_ms();
                if let Ok(output) = observation.into_output() {
                    let late = minimum_event_sequence.is_some_and(|minimum| sequence < minimum);
                    counters.field_ready_success = counters.field_ready_success.saturating_add(1);
                    counters.candidate_sets = counters.candidate_sets.saturating_add(1);
                    counters.scored_candidates = counters.scored_candidates.saturating_add(
                        u64::try_from(output.candidates().candidate_count()).unwrap_or(u64::MAX),
                    );
                    if matches!(
                        output.fields(),
                        scorepeek::recognition::ScreenFieldObservations::Result(_)
                    ) {
                        counters.result_observations =
                            counters.result_observations.saturating_add(1);
                    }
                    let mut output_failed = false;
                    let output_timing = if minimum_event_sequence
                        .is_none_or(|minimum| sequence >= minimum)
                        && let Some(emit) = emit.as_deref_mut()
                    {
                        if let Ok(timing) = emit(GamescopeLiveSessionEvent::Observation {
                            screen_episode_id,
                            sequence,
                            monotonic_start_ms,
                            monotonic_end_ms,
                            output: &output,
                        }) {
                            Some(timing)
                        } else {
                            output_failed = true;
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(output_timing) = output_timing {
                        timing.add_live_processing(output_timing);
                    }
                    let _ = session.record_frame_processing_timing(
                        timing,
                        if late {
                            crate::diagnostic_recording::FrameFieldStatus::LateEpisode
                        } else {
                            crate::diagnostic_recording::FrameFieldStatus::Completed
                        },
                        Some(output.processing_timing()),
                    );
                    if let Some(worker) = artifact_worker {
                        let counter = match worker.try_record_in_episode(
                            sequence,
                            screen_episode_id,
                            monotonic_start_ms,
                            monotonic_end_ms,
                            if late {
                                crate::diagnostic_recording::FrameFieldStatus::LateEpisode
                            } else {
                                crate::diagnostic_recording::FrameFieldStatus::Completed
                            },
                            output,
                        ) {
                            RecognitionArtifactEnqueueOutcome::Enqueued => {
                                &mut counters.recognition_artifact_enqueued
                            }
                            RecognitionArtifactEnqueueOutcome::QueueFull => {
                                &mut counters.recognition_artifact_queue_full
                            }
                            RecognitionArtifactEnqueueOutcome::WorkerUnavailable => {
                                &mut counters.recognition_artifact_worker_unavailable
                            }
                        };
                        *counter = counter.saturating_add(1);
                    }
                    if output_failed {
                        return Some(FieldObservationGateErrorType::ResultOutputFailed);
                    }
                } else {
                    let _ = session.record_frame_processing_timing(
                        timing,
                        crate::diagnostic_recording::FrameFieldStatus::Failed,
                        None,
                    );
                    counters.field_ready_failure = counters.field_ready_failure.saturating_add(1);
                    return Some(FieldObservationGateErrorType::FieldObservationFailed);
                }
                if wait.is_some() {
                    return None;
                }
            }
            FieldObservationSessionPoll::Consumed
            | FieldObservationSessionPoll::BindingMismatch
            | FieldObservationSessionPoll::Terminal => {
                pending.swap_remove(index);
                return Some(FieldObservationGateErrorType::DiagnosticConfigurationInvalid);
            }
            FieldObservationSessionPoll::WorkerUnavailable => {
                pending.swap_remove(index);
                return Some(FieldObservationGateErrorType::FieldObserverUnavailable);
            }
        }
    }
    None
}

fn wait_live_field_observations(
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
    mut emit: Option<&mut LiveEventEmitter<'_>>,
    minimum_event_sequence: Option<u64>,
) -> Option<(FieldObservationGateErrorType, Option<CaptureErrorType>)> {
    let started = Instant::now();
    while !pending.is_empty() {
        let remaining = DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        if let Some(error) = poll_field_observations(
            session,
            pending,
            counters,
            artifact_worker,
            Some(remaining),
            emit.as_deref_mut(),
            minimum_event_sequence,
        ) {
            return Some((error, None));
        }
    }
    None
}

struct FieldObservationFinishOutcomes {
    field_observer: Option<crate::recognition_live::field_observer::FieldObserverFinishOutcome>,
    diagnostic: Option<crate::diagnostic_recording::DiagnosticFinishOutcome>,
    recognition_artifact: Option<RecognitionArtifactFinishOutcome>,
    artifact_requested: bool,
}

fn result_evidence_error(
    counters: &FieldObservationCounters,
) -> Option<FieldObservationGateErrorType> {
    (counters.result_observations == 0)
        .then_some(FieldObservationGateErrorType::ResultObservationUnavailable)
}

fn recognition_artifact_error(
    counters: &FieldObservationCounters,
    outcome: Option<&RecognitionArtifactFinishOutcome>,
) -> Option<FieldObservationGateErrorType> {
    let complete = matches!(
        outcome,
        Some(RecognitionArtifactFinishOutcome {
            status: RecognitionArtifactFinishStatus::Complete,
            manifest_sha256: Some(_),
            ..
        })
    ) && counters.recognition_artifact_queue_full == 0
        && counters.recognition_artifact_worker_unavailable == 0
        && counters.recognition_artifact_enqueued == counters.field_ready_success
        && outcome.is_some_and(|outcome| {
            outcome.input_observations == outcome.retained_observations
                && u64::try_from(outcome.input_observations).ok()
                    == Some(counters.recognition_artifact_enqueued)
        });
    (!complete).then_some(FieldObservationGateErrorType::RecognitionArtifactIncomplete)
}

#[allow(clippy::too_many_lines)]
fn field_observation_report(
    error_type: Option<FieldObservationGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: CaptureGeneration,
    counters: FieldObservationCounters,
    finishes: FieldObservationFinishOutcomes,
    sink: BoundedDiagnosticSink,
) -> GamescopeFieldObservationGateReport {
    let (
        field_worker_status,
        field_worker_submitted,
        field_worker_completed,
        field_worker_abandoned,
    ) = finishes
        .field_observer
        .map_or((None, None, None, None), |outcome| {
            let status = match outcome.status {
                FieldObserverFinishStatus::Complete => FieldWorkerStatus::Complete,
                FieldObserverFinishStatus::Timeout => FieldWorkerStatus::Timeout,
                FieldObserverFinishStatus::WorkerUnavailable => {
                    FieldWorkerStatus::WorkerUnavailable
                }
            };
            (
                Some(status),
                Some(outcome.submitted),
                outcome.completed,
                outcome.abandoned,
            )
        });
    let (diagnostic_completeness, diagnostic_error_type, diagnostic_manifest_sha256) =
        finishes.diagnostic.map_or((None, None, None), |outcome| {
            (
                outcome.completeness,
                outcome.error_type,
                outcome.manifest_sha256,
            )
        });
    let (
        recognition_artifact_status,
        recognition_artifact_manifest_sha256,
        recognition_artifact_input_observations,
        recognition_artifact_retained_observations,
    ) = finishes
        .recognition_artifact
        .map_or((None, None, None, None), |outcome| {
            (
                Some(outcome.status),
                outcome.manifest_sha256,
                Some(outcome.input_observations),
                Some(outcome.retained_observations),
            )
        });
    GamescopeFieldObservationGateReport {
        schema: if finishes.artifact_requested {
            "scorepeek-gamescope-result-recognition-gate-v1"
        } else {
            "scorepeek-gamescope-field-observation-gate-v1"
        },
        status: if error_type.is_some() {
            LiveGateStatus::Error
        } else {
            LiveGateStatus::Success
        },
        error_type,
        capture_error_type,
        capture_generation: capture_generation.get(),
        observed_frames: counters.observed_frames,
        normalized_frames: counters.normalized_frames,
        recognition_ticks: counters.recognition_ticks,
        recognition_busy_skips: counters.recognition_busy_skips,
        maximum_consecutive_busy_skips: counters.maximum_consecutive_busy_skips,
        field_observation_busy_skips: counters.field_observation_busy_skips,
        maximum_consecutive_field_observation_busy_skips: counters
            .maximum_consecutive_field_observation_busy_skips,
        last_recognition_sequence: counters.last_recognition_sequence,
        inspected_frames: counters.inspected_frames,
        result_frames: counters.result_frames,
        music_select_frames: counters.music_select_frames,
        mode_select_frames: counters.mode_select_frames,
        decide_transition_frames: counters.decide_transition_frames,
        play_frames: counters.play_frames,
        unknown_frames: counters.unknown_frames,
        field_not_applicable: counters.field_not_applicable,
        field_submitted: counters.field_submitted,
        field_rejected: counters.field_rejected,
        field_ready_success: counters.field_ready_success,
        field_ready_failure: counters.field_ready_failure,
        candidate_sets: counters.candidate_sets,
        scored_candidates: counters.scored_candidates,
        result_observations: finishes
            .artifact_requested
            .then_some(counters.result_observations),
        recognition_artifact_enqueued: finishes
            .artifact_requested
            .then_some(counters.recognition_artifact_enqueued),
        recognition_artifact_queue_full: finishes
            .artifact_requested
            .then_some(counters.recognition_artifact_queue_full),
        recognition_artifact_worker_unavailable: finishes
            .artifact_requested
            .then_some(counters.recognition_artifact_worker_unavailable),
        recognition_artifact_status: finishes
            .artifact_requested
            .then_some(recognition_artifact_status)
            .flatten(),
        recognition_artifact_manifest_sha256: finishes
            .artifact_requested
            .then_some(recognition_artifact_manifest_sha256)
            .flatten(),
        recognition_artifact_input_observations: finishes
            .artifact_requested
            .then_some(recognition_artifact_input_observations)
            .flatten(),
        recognition_artifact_retained_observations: finishes
            .artifact_requested
            .then_some(recognition_artifact_retained_observations)
            .flatten(),
        field_worker_status,
        field_worker_submitted,
        field_worker_completed,
        field_worker_abandoned,
        diagnostic_completeness,
        diagnostic_error_type,
        diagnostic_manifest_sha256,
        capture_diagnostic_facts: sink.facts,
        dropped_capture_diagnostic_facts: sink.dropped,
        session_stop_reason: None,
        failure_detail: None,
    }
}

fn run_gamescope_handoff_gate(
    mut config: GamescopeDiagnosticHandoffGateConfig<'_>,
    inspect_screen: bool,
) -> HandoffGateRun {
    let capture_generation = config.capture_generation;
    let mut recognition = RecognitionHandoffCounters::default();
    if !valid_handoff_descriptor(&config.descriptor, capture_generation, inspect_screen) {
        return invalid_handoff_configuration(
            capture_generation,
            BoundedDiagnosticSink::default(),
            recognition,
        );
    }
    let binding = match read_diagnostic_handoff_binding(
        config.binding_path,
        config.expected_binding_sha256,
    ) {
        Ok(binding) => binding,
        Err(error_type) => {
            return handoff_gate_run(
                diagnostic_handoff_report(
                    error_type,
                    None,
                    capture_generation,
                    HandoffCounters::default(),
                    None,
                    BoundedDiagnosticSink::default(),
                ),
                recognition,
            );
        }
    };
    bind_diagnostic_descriptor(&mut config.descriptor, &binding);
    let mut sink = BoundedDiagnosticSink::default();
    let mut lease = match start_diagnostic_handoff_capture(
        binding,
        capture_generation,
        config.expected_source_node_id,
        &mut sink,
    ) {
        Ok(lease) => lease,
        Err((error_type, capture_error_type)) => {
            return handoff_gate_run(
                diagnostic_handoff_report(
                    error_type,
                    capture_error_type,
                    capture_generation,
                    HandoffCounters::default(),
                    None,
                    sink,
                ),
                recognition,
            );
        }
    };
    if config.descriptor.binding.capture_profile_sha256 != lease.capture_profile_sha256()
        || config.descriptor.binding.normalizer_sha256 != lease.normalizer_artifact_sha256()
    {
        let _ = lease.shutdown(&mut sink);
        return handoff_gate_run(
            diagnostic_handoff_report(
                DiagnosticHandoffGateErrorType::DiagnosticBindingMismatch,
                None,
                capture_generation,
                HandoffCounters::default(),
                None,
                sink,
            ),
            recognition,
        );
    }
    let Ok(mut session) = start_handoff_session(
        config.diagnostic_root,
        config.descriptor,
        config.policy,
        inspect_screen,
    ) else {
        let _ = lease.shutdown(&mut sink);
        return invalid_handoff_configuration(capture_generation, sink, recognition);
    };
    let mut counters = HandoffCounters::default();
    let mut terminal = offer_diagnostic_handoff_frames(
        &mut lease,
        &mut session,
        Duration::from_millis(config.duration_ms),
        &mut counters,
        inspect_screen,
        &mut recognition,
        &mut sink,
    );
    finish_handoff_gate(
        lease,
        session,
        &mut terminal,
        capture_generation,
        counters,
        recognition,
        sink,
    )
}

fn valid_handoff_descriptor(
    descriptor: &DiagnosticRunDescriptor,
    capture_generation: CaptureGeneration,
    inspect_screen: bool,
) -> bool {
    descriptor.binding.capture_generation == capture_generation.get()
        && descriptor.binding.replay.is_none()
        && (!inspect_screen
            || descriptor.binding.canonical_layout_sha256 == CanonicalLayout::sha256())
}

fn start_handoff_session(
    root: &std::path::Path,
    descriptor: DiagnosticRunDescriptor,
    policy: DiagnosticPolicy,
    inspect_screen: bool,
) -> Result<HandoffSession, ()> {
    if inspect_screen {
        RecognitionSession::start(root, descriptor, policy)
            .map(HandoffSession::Recognition)
            .map_err(|_| ())
    } else {
        Ok(HandoffSession::Diagnostic(DiagnosticBridge::start(
            root, descriptor, policy,
        )))
    }
}

fn invalid_handoff_configuration(
    capture_generation: CaptureGeneration,
    sink: BoundedDiagnosticSink,
    recognition: RecognitionHandoffCounters,
) -> HandoffGateRun {
    handoff_gate_run(
        diagnostic_handoff_report(
            DiagnosticHandoffGateErrorType::DiagnosticConfigurationInvalid,
            None,
            capture_generation,
            HandoffCounters::default(),
            None,
            sink,
        ),
        recognition,
    )
}

fn finish_handoff_gate(
    lease: CalibratedGamescopeLease,
    session: HandoffSession,
    terminal: &mut Option<(DiagnosticHandoffGateErrorType, Option<CaptureErrorType>)>,
    capture_generation: CaptureGeneration,
    counters: HandoffCounters,
    recognition: RecognitionHandoffCounters,
    mut sink: BoundedDiagnosticSink,
) -> HandoffGateRun {
    if counters.normalized_frames == 0 && terminal.is_none() {
        *terminal = Some((DiagnosticHandoffGateErrorType::FrameUnavailable, None));
    }
    let (shutdown_result, finish_time) = lease.shutdown_with_elapsed(&mut sink);
    if let Err(error) = shutdown_result {
        terminal.get_or_insert((
            DiagnosticHandoffGateErrorType::ShutdownFailed,
            Some(error.error_type()),
        ));
    }
    let finish_status = if terminal.is_some() {
        DiagnosticRunStatus::Error
    } else {
        DiagnosticRunStatus::Success
    };
    let diagnostic_outcome = session.finish(finish_status, finish_time);
    let diagnostic = match *terminal {
        Some((error_type, capture_error_type)) => diagnostic_handoff_report(
            error_type,
            capture_error_type,
            capture_generation,
            counters,
            Some(diagnostic_outcome),
            sink,
        ),
        None => diagnostic_handoff_success(capture_generation, counters, diagnostic_outcome, sink),
    };
    handoff_gate_run(diagnostic, recognition)
}

fn handoff_gate_run(
    diagnostic: GamescopeDiagnosticHandoffGateReport,
    recognition: RecognitionHandoffCounters,
) -> HandoffGateRun {
    HandoffGateRun {
        diagnostic,
        recognition,
    }
}

fn read_diagnostic_handoff_binding(
    path: &std::path::Path,
    expected_sha256: &str,
) -> Result<GamescopeProfileBinding, DiagnosticHandoffGateErrorType> {
    read_binding(path, expected_sha256).map_err(|error| match error {
        BindingAdmissionGateErrorType::BindingUnavailable => {
            DiagnosticHandoffGateErrorType::BindingUnavailable
        }
        _ => DiagnosticHandoffGateErrorType::BindingInvalid,
    })
}

fn bind_diagnostic_descriptor(
    descriptor: &mut DiagnosticRunDescriptor,
    binding: &GamescopeProfileBinding,
) {
    binding
        .capture_profile_sha256()
        .clone_into(&mut descriptor.binding.capture_profile_sha256);
    binding
        .normalizer_artifact_sha256()
        .clone_into(&mut descriptor.binding.normalizer_sha256);
}

fn start_diagnostic_handoff_capture(
    binding: GamescopeProfileBinding,
    capture_generation: CaptureGeneration,
    expected_source_node_id: Option<u32>,
    sink: &mut BoundedDiagnosticSink,
) -> Result<CalibratedGamescopeLease, (DiagnosticHandoffGateErrorType, Option<CaptureErrorType>)> {
    let lease = acquire_gamescope_source(DISCOVERY_TIMEOUT, sink).map_err(|error| {
        (
            DiagnosticHandoffGateErrorType::CaptureFailed,
            Some(error.error_type()),
        )
    })?;
    if expected_source_node_id.is_some_and(|expected| expected != lease.node_id()) {
        lease.shutdown(sink);
        return Err((
            DiagnosticHandoffGateErrorType::CaptureFailed,
            Some(CaptureErrorType::SourceLost),
        ));
    }
    let receiver = start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, sink)
        .map_err(|error| {
            (
                DiagnosticHandoffGateErrorType::CaptureFailed,
                Some(error.error_type()),
            )
        })?;
    admit_gamescope_profile(receiver, binding, capture_generation, sink).map_err(|failure| {
        let error_type = failure.error_type();
        let _ = failure.shutdown(sink);
        (
            DiagnosticHandoffGateErrorType::AdmissionRejected,
            Some(error_type),
        )
    })
}

fn offer_diagnostic_handoff_frames(
    lease: &mut CalibratedGamescopeLease,
    session: &mut HandoffSession,
    duration: Duration,
    counters: &mut HandoffCounters,
    inspect_screen: bool,
    recognition: &mut RecognitionHandoffCounters,
    sink: &mut BoundedDiagnosticSink,
) -> Option<(DiagnosticHandoffGateErrorType, Option<CaptureErrorType>)> {
    let started = Instant::now();
    loop {
        if let Some(observed) = lease.take_latest_observed_frame() {
            counters.observed_frames = counters.observed_frames.saturating_add(1);
            let (normalized, source) =
                match lease.normalize_observed_frame_with_source(observed, sink) {
                    Ok(pair) => pair,
                    Err(error) => {
                        return Some((
                            DiagnosticHandoffGateErrorType::NormalizationFailed,
                            Some(error.error_type()),
                        ));
                    }
                };
            let live = BoundCanonicalFrame::from_normalized_with_source(normalized, source);
            counters.normalized_frames = counters.normalized_frames.saturating_add(1);
            counters.first_sequence.get_or_insert(live.sequence());
            counters.last_sequence = Some(live.sequence());
            if inspect_screen {
                let HandoffSession::Recognition(recognition_session) = session else {
                    unreachable!("recognition gate owns a recognition session");
                };
                let Ok(result) = recognition_session.inspect(&live) else {
                    recognition.recognition_failures =
                        recognition.recognition_failures.saturating_add(1);
                    return Some((DiagnosticHandoffGateErrorType::RecognitionFailed, None));
                };
                counters.record_offer(result.diagnostic_frame);
                recognition.inspected_frames = recognition.inspected_frames.saturating_add(1);
                let screen_counter = match result.observation.screen() {
                    ScreenClass::Result => &mut recognition.result_frames,
                    ScreenClass::MusicSelect => &mut recognition.music_select_frames,
                    ScreenClass::ModeSelect => &mut recognition.mode_select_frames,
                    ScreenClass::DecideTransition => &mut recognition.decide_transition_frames,
                    ScreenClass::Play => &mut recognition.play_frames,
                    ScreenClass::Unknown => &mut recognition.unknown_frames,
                };
                *screen_counter = screen_counter.saturating_add(1);
                recognition.fact_outcomes.record(result.diagnostic_fact);
            } else {
                let HandoffSession::Diagnostic(bridge) = session else {
                    unreachable!("diagnostic gate owns a diagnostic bridge");
                };
                counters.record_offer(bridge.offer(&live));
            }
        }
        if started.elapsed() >= duration {
            return None;
        }
        let remaining = duration.saturating_sub(started.elapsed());
        if let Err(error) = lease.poll(remaining, sink) {
            return Some((
                DiagnosticHandoffGateErrorType::CaptureFailed,
                Some(error.error_type()),
            ));
        }
    }
}

fn diagnostic_handoff_success(
    capture_generation: CaptureGeneration,
    counters: HandoffCounters,
    diagnostic_outcome: crate::diagnostic_recording::DiagnosticFinishOutcome,
    sink: BoundedDiagnosticSink,
) -> GamescopeDiagnosticHandoffGateReport {
    diagnostic_handoff_report_inner(
        LiveGateStatus::Success,
        None,
        None,
        capture_generation,
        counters,
        Some(diagnostic_outcome),
        sink,
    )
}

fn diagnostic_handoff_report(
    error_type: DiagnosticHandoffGateErrorType,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: CaptureGeneration,
    counters: HandoffCounters,
    diagnostic_outcome: Option<crate::diagnostic_recording::DiagnosticFinishOutcome>,
    sink: BoundedDiagnosticSink,
) -> GamescopeDiagnosticHandoffGateReport {
    diagnostic_handoff_report_inner(
        LiveGateStatus::Error,
        Some(error_type),
        capture_error_type,
        capture_generation,
        counters,
        diagnostic_outcome,
        sink,
    )
}

fn diagnostic_handoff_report_inner(
    status: LiveGateStatus,
    error_type: Option<DiagnosticHandoffGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: CaptureGeneration,
    counters: HandoffCounters,
    diagnostic_outcome: Option<crate::diagnostic_recording::DiagnosticFinishOutcome>,
    sink: BoundedDiagnosticSink,
) -> GamescopeDiagnosticHandoffGateReport {
    let (diagnostic_completeness, diagnostic_error_type, diagnostic_manifest_sha256) =
        diagnostic_outcome.map_or((None, None, None), |outcome| {
            (
                outcome.completeness,
                outcome.error_type,
                outcome.manifest_sha256,
            )
        });
    GamescopeDiagnosticHandoffGateReport {
        schema: "scorepeek-gamescope-diagnostic-handoff-gate-v1",
        status,
        error_type,
        capture_error_type,
        capture_generation: capture_generation.get(),
        observed_frames: counters.observed_frames,
        normalized_frames: counters.normalized_frames,
        first_sequence: counters.first_sequence,
        last_sequence: counters.last_sequence,
        enqueued_frames: counters.enqueued_frames,
        skipped_cadence_frames: counters.skipped_cadence_frames,
        rejected_frames: counters.rejected_frames,
        disabled_frames: counters.disabled_frames,
        queue_full_frames: counters.queue_full_frames,
        worker_unavailable_frames: counters.worker_unavailable_frames,
        diagnostic_completeness,
        diagnostic_error_type,
        diagnostic_manifest_sha256,
        capture_diagnostic_facts: sink.facts,
        dropped_capture_diagnostic_facts: sink.dropped,
    }
}

fn recognition_handoff_report(run: HandoffGateRun) -> GamescopeRecognitionHandoffGateReport {
    let diagnostic = run.diagnostic;
    let recognition = run.recognition;
    GamescopeRecognitionHandoffGateReport {
        schema: "scorepeek-gamescope-recognition-handoff-gate-v1",
        status: diagnostic.status,
        error_type: diagnostic.error_type,
        capture_error_type: diagnostic.capture_error_type,
        capture_generation: diagnostic.capture_generation,
        observed_frames: diagnostic.observed_frames,
        normalized_frames: diagnostic.normalized_frames,
        first_sequence: diagnostic.first_sequence,
        last_sequence: diagnostic.last_sequence,
        diagnostic_frame_enqueued: diagnostic.enqueued_frames,
        diagnostic_frame_skipped_cadence: diagnostic.skipped_cadence_frames,
        diagnostic_frame_rejected: diagnostic.rejected_frames,
        diagnostic_frame_disabled: diagnostic.disabled_frames,
        diagnostic_frame_queue_full: diagnostic.queue_full_frames,
        diagnostic_frame_worker_unavailable: diagnostic.worker_unavailable_frames,
        inspected_frames: recognition.inspected_frames,
        result_frames: recognition.result_frames,
        music_select_frames: recognition.music_select_frames,
        mode_select_frames: recognition.mode_select_frames,
        decide_transition_frames: recognition.decide_transition_frames,
        play_frames: recognition.play_frames,
        unknown_frames: recognition.unknown_frames,
        recognition_failures: recognition.recognition_failures,
        diagnostic_fact_enqueued: recognition.fact_outcomes.enqueued,
        diagnostic_fact_skipped_cadence: recognition.fact_outcomes.skipped_cadence,
        diagnostic_fact_rejected: recognition.fact_outcomes.rejected,
        diagnostic_fact_disabled: recognition.fact_outcomes.disabled,
        diagnostic_fact_queue_full: recognition.fact_outcomes.queue_full,
        diagnostic_fact_worker_unavailable: recognition.fact_outcomes.worker_unavailable,
        diagnostic_completeness: diagnostic.diagnostic_completeness,
        diagnostic_error_type: diagnostic.diagnostic_error_type,
        diagnostic_manifest_sha256: diagnostic.diagnostic_manifest_sha256,
        capture_diagnostic_facts: diagnostic.capture_diagnostic_facts,
        dropped_capture_diagnostic_facts: diagnostic.dropped_capture_diagnostic_facts,
    }
}

fn canonical_frame_success(
    capture_generation: CaptureGeneration,
    capture_profile_sha256: String,
    normalizer_artifact_sha256: String,
    source_sequence: u64,
    canonical_rgb8_sha256: String,
    sink: BoundedDiagnosticSink,
) -> GamescopeCanonicalFrameGateReport {
    GamescopeCanonicalFrameGateReport {
        schema: "scorepeek-gamescope-canonical-frame-gate-v1",
        status: LiveGateStatus::Success,
        error_type: None,
        capture_error_type: None,
        capture_generation: capture_generation.get(),
        capture_profile_sha256: Some(capture_profile_sha256),
        normalizer_artifact_sha256: Some(normalizer_artifact_sha256),
        source_sequence: Some(source_sequence),
        canonical_rgb8_sha256: Some(canonical_rgb8_sha256),
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}

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

pub fn parse_lifecycle_runs(value: &OsStr) -> Result<u32, String> {
    let runs = parse_u64(value, "capture lifecycle run count")?;
    if !(u64::from(MIN_LIFECYCLE_RUNS)..=u64::from(MAX_LIFECYCLE_RUNS)).contains(&runs) {
        return Err(format!(
            "capture lifecycle run count must be between {MIN_LIFECYCLE_RUNS} and {MAX_LIFECYCLE_RUNS}"
        ));
    }
    u32::try_from(runs).map_err(|_| "capture lifecycle run count is too large".to_owned())
}

fn parse_u64(value: &OsStr, label: &str) -> Result<u64, String> {
    let value = value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?;
    value
        .parse::<u64>()
        .map_err(|_| format!("{label} must be an integer"))
}

pub fn run_gamescope_live_gate(duration_ms: u64) -> GamescopeLiveGateReport {
    run_gamescope_live_gate_with_interval(duration_ms, 0)
}

pub fn run_gamescope_live_gate_with_interval(
    duration_ms: u64,
    consumer_interval_ms: u64,
) -> GamescopeLiveGateReport {
    let mut sink = BoundedDiagnosticSink::default();
    let mut consumed_frames = 0_u64;
    let mut first_sequence = None;
    let mut last_sequence = None;

    let lease = match acquire_gamescope_source(DISCOVERY_TIMEOUT, &mut sink) {
        Ok(lease) => lease,
        Err(error) => {
            return report(
                duration_ms,
                consumer_interval_ms,
                consumed_frames,
                first_sequence,
                last_sequence,
                Some(error.error_type()),
                sink,
            );
        }
    };
    let mut receiver =
        match start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, &mut sink) {
            Ok(receiver) => receiver,
            Err(error) => {
                return report(
                    duration_ms,
                    consumer_interval_ms,
                    consumed_frames,
                    first_sequence,
                    last_sequence,
                    Some(error.error_type()),
                    sink,
                );
            }
        };

    consume_latest(
        &mut receiver,
        &mut consumed_frames,
        &mut first_sequence,
        &mut last_sequence,
    );
    let steady_started = Instant::now();
    let mut last_consumed = steady_started;
    let requested_duration = Duration::from_millis(duration_ms);
    let consumer_interval = Duration::from_millis(consumer_interval_ms);
    let mut terminal = None;
    while steady_started.elapsed() < requested_duration {
        let remaining = requested_duration.saturating_sub(steady_started.elapsed());
        if let Err(error) = receiver.poll(remaining, &mut sink) {
            terminal = Some(error.error_type());
            break;
        }
        if last_consumed.elapsed() >= consumer_interval {
            consume_latest(
                &mut receiver,
                &mut consumed_frames,
                &mut first_sequence,
                &mut last_sequence,
            );
            last_consumed = Instant::now();
        }
    }

    consume_latest(
        &mut receiver,
        &mut consumed_frames,
        &mut first_sequence,
        &mut last_sequence,
    );

    if let Err(error) = receiver.shutdown(&mut sink) {
        terminal.get_or_insert(error.error_type());
    }
    report(
        duration_ms,
        consumer_interval_ms,
        consumed_frames,
        first_sequence,
        last_sequence,
        terminal,
        sink,
    )
}

pub fn run_gamescope_lifecycle_gate(
    duration_ms: u64,
    requested_runs: u32,
    consumer_interval_ms: u64,
) -> GamescopeLifecycleGateReport {
    let resources_before_first_run = process_resource_snapshot().ok();
    let mut resources_after_warmup = None;
    let mut maximum_resources_after_run = None;
    let mut resources_after_final_run = None;
    let mut runs = Vec::with_capacity(requested_runs as usize);
    let mut capture_failed = false;
    let mut resource_unavailable = resources_before_first_run.is_none();
    let mut overwrite_observed = false;

    for run in 1..=requested_runs {
        let report = run_gamescope_live_gate_with_interval(duration_ms, consumer_interval_ms);
        let summary = summarize_run(run, &report);
        overwrite_observed |= summary.overwritten_frames > 0;
        capture_failed |= !report.succeeded();
        runs.push(summary);

        match process_resource_snapshot() {
            Ok(resources) => {
                if run == 1 {
                    resources_after_warmup = Some(resources);
                }
                maximum_resources_after_run
                    .get_or_insert(resources)
                    .update_maximum(resources);
                resources_after_final_run = Some(resources);
            }
            Err(()) => resource_unavailable = true,
        }
        if capture_failed {
            break;
        }
    }

    let error_type = lifecycle_error_type(
        capture_failed,
        resource_unavailable,
        consumer_interval_ms > 0 && !overwrite_observed,
    );
    GamescopeLifecycleGateReport {
        schema: "scorepeek-gamescope-lifecycle-gate-v1",
        status: if error_type.is_some() {
            LiveGateStatus::Error
        } else {
            LiveGateStatus::Success
        },
        error_type,
        requested_duration_ms: duration_ms,
        consumer_interval_ms,
        requested_runs,
        completed_runs: u32::try_from(runs.len()).unwrap_or(u32::MAX),
        overwrite_observed,
        resources_before_first_run,
        resources_after_warmup,
        maximum_resources_after_run,
        resources_after_final_run,
        runs,
    }
}

fn lifecycle_error_type(
    capture_failed: bool,
    resource_unavailable: bool,
    expected_overwrite_missing: bool,
) -> Option<LifecycleGateErrorType> {
    if capture_failed {
        Some(LifecycleGateErrorType::CaptureRunFailed)
    } else if resource_unavailable {
        Some(LifecycleGateErrorType::ProcessResourceUnavailable)
    } else if expected_overwrite_missing {
        Some(LifecycleGateErrorType::ExpectedOverwriteMissing)
    } else {
        None
    }
}

fn summarize_run(run: u32, report: &GamescopeLiveGateReport) -> LifecycleRunSummary {
    let mut summary = LifecycleRunSummary {
        run,
        status: report.status,
        error_type: report.error_type,
        consumed_frames: report.consumed_frames,
        received_frames: 0,
        overwritten_frames: 0,
        last_sequence: report.last_sequence,
        maximum_gap_ns: 0,
        diagnostic_fact_count: u32::try_from(report.diagnostic_facts.len()).unwrap_or(u32::MAX),
        dropped_diagnostic_facts: report.dropped_diagnostic_facts,
        phases: LifecyclePhaseSummary::default(),
    };
    for fact in &report.diagnostic_facts {
        let succeeded = fact.status == CaptureDiagnosticStatus::Success;
        match (&fact.operation, &fact.detail) {
            (
                CaptureDiagnosticOperation::SteadyReception,
                CaptureDiagnosticDetail::SteadyReception {
                    received_frames,
                    overwritten_frames,
                    last_sequence,
                    maximum_gap_ns,
                },
            ) => {
                summary.received_frames = *received_frames;
                summary.overwritten_frames = *overwritten_frames;
                summary.last_sequence = *last_sequence;
                summary.maximum_gap_ns = *maximum_gap_ns;
            }
            (CaptureDiagnosticOperation::StreamNegotiation, _) if succeeded => {
                summary.phases.negotiation = LifecyclePhaseStatus::Success;
            }
            (CaptureDiagnosticOperation::FirstFrame, _) if succeeded => {
                summary.phases.first_frame = LifecyclePhaseStatus::Success;
            }
            (CaptureDiagnosticOperation::ReceiverShutdown, _) if succeeded => {
                summary.phases.receiver_shutdown = LifecyclePhaseStatus::Success;
            }
            (CaptureDiagnosticOperation::Shutdown, _) if succeeded => {
                summary.phases.provider_shutdown = LifecyclePhaseStatus::Success;
            }
            _ => {}
        }
    }
    summary
}

fn process_resource_snapshot() -> Result<ProcessResourceSnapshot, ()> {
    Ok(ProcessResourceSnapshot {
        open_file_descriptors: count_proc_entries("/proc/self/fd")?,
        threads: count_proc_entries("/proc/self/task")?,
        resident_bytes: resident_bytes()?,
    })
}

fn count_proc_entries(path: &str) -> Result<u64, ()> {
    let entries = fs::read_dir(path).map_err(|_| ())?;
    let mut count = 0_u64;
    for entry in entries {
        entry.map_err(|_| ())?;
        count = count.checked_add(1).ok_or(())?;
    }
    Ok(count)
}

fn resident_bytes() -> Result<u64, ()> {
    let file = File::open("/proc/self/status").map_err(|_| ())?;
    let mut status = String::new();
    file.take(MAX_PROC_STATUS_BYTES)
        .read_to_string(&mut status)
        .map_err(|_| ())?;
    let line = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .ok_or(())?;
    let mut fields = line.split_ascii_whitespace();
    if fields.next() != Some("VmRSS:") {
        return Err(());
    }
    let kibibytes = fields.next().ok_or(())?.parse::<u64>().map_err(|_| ())?;
    if fields.next() != Some("kB") || fields.next().is_some() {
        return Err(());
    }
    kibibytes.checked_mul(1024).ok_or(())
}

fn consume_latest(
    receiver: &mut scorepeek::capture::UncalibratedPipeWireReceiver,
    consumed_frames: &mut u64,
    first_sequence: &mut Option<u64>,
    last_sequence: &mut Option<u64>,
) {
    let Some(frame) = receiver.take_latest_frame() else {
        return;
    };
    *consumed_frames = consumed_frames.saturating_add(1);
    first_sequence.get_or_insert(frame.sequence());
    *last_sequence = Some(frame.sequence());
}

fn report(
    duration_ms: u64,
    consumer_interval_ms: u64,
    consumed_frames: u64,
    first_sequence: Option<u64>,
    last_sequence: Option<u64>,
    error_type: Option<CaptureErrorType>,
    sink: BoundedDiagnosticSink,
) -> GamescopeLiveGateReport {
    GamescopeLiveGateReport {
        schema: "scorepeek-gamescope-live-gate-v2",
        status: if error_type.is_some() {
            LiveGateStatus::Error
        } else {
            LiveGateStatus::Success
        },
        requested_duration_ms: duration_ms,
        consumer_interval_ms,
        consumed_frames,
        first_sequence,
        last_sequence,
        error_type,
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;

    use scorepeek::capture::{
        CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
        CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType, CaptureGeneration,
        CaptureSourceKind, FractionalRectangle, GamescopeProfileBinding,
        GamescopeProfileBindingAuthoringInput, RationalCoordinate, UncalibratedMemoryType,
        UncalibratedVideoContract,
    };
    use scorepeek::recognition::{
        CanonicalLayout, RegisteredResourceLoadError, RegisteredResourceLoadErrorType,
    };

    use super::{
        BoundedDiagnosticSink, DiagnosticPolicy, DiagnosticRunDescriptor, FieldObservationCounters,
        FieldObservationFinishOutcomes, FieldObservationGateErrorType, GamescopeLiveGateReport,
        LifecycleGateErrorType, LifecyclePhaseStatus, LiveGateStatus, MAX_DIAGNOSTIC_FACTS,
        RecognitionArtifactFinishOutcome, RecognitionArtifactFinishStatus,
        field_observation_report, field_resource_error, field_start_error, lifecycle_error_type,
        parse_consumer_interval_ms, parse_duration_ms, parse_lifecycle_runs,
        process_resource_snapshot, read_binding, recognition_artifact_error, result_evidence_error,
        run_gamescope_binding_admission_gate, run_gamescope_canonical_frame_gate,
        run_gamescope_diagnostic_handoff_gate, run_gamescope_recognition_handoff_gate,
        summarize_run,
    };

    fn diagnostic_descriptor(generation: u64) -> DiagnosticRunDescriptor {
        DiagnosticRunDescriptor {
            run_id: "handoff-test".to_owned(),
            monotonic_start_ms: 0,
            resource: crate::diagnostic_recording::DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "1".repeat(64),
            },
            binding: crate::diagnostic_recording::DiagnosticBinding {
                capture_generation: generation,
                capture_profile_sha256: String::new(),
                normalizer_sha256: String::new(),
                canonical_layout_sha256: CanonicalLayout::sha256(),
                catalog_sha256: "3".repeat(64),
                model_sha256: "4".repeat(64),
                runtime_sha256: "5".repeat(64),
                replay: None,
            },
        }
    }

    #[test]
    fn duration_is_explicitly_bounded() {
        assert_eq!(parse_duration_ms(OsStr::new("1")).unwrap(), 1);
        assert_eq!(parse_duration_ms(OsStr::new("60000")).unwrap(), 60_000);
        assert!(parse_duration_ms(OsStr::new("0")).is_err());
        assert!(parse_duration_ms(OsStr::new("60001")).is_err());
        assert!(parse_duration_ms(OsStr::new("forever")).is_err());
    }

    #[test]
    fn consumer_interval_is_explicitly_bounded() {
        assert_eq!(parse_consumer_interval_ms(OsStr::new("0")).unwrap(), 0);
        assert_eq!(
            parse_consumer_interval_ms(OsStr::new("60000")).unwrap(),
            60_000
        );
        assert!(parse_consumer_interval_ms(OsStr::new("60001")).is_err());
        assert!(parse_consumer_interval_ms(OsStr::new("sometimes")).is_err());
    }

    #[test]
    fn binding_gate_reads_only_digest_selected_bounded_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("binding.json");
        let video = UncalibratedVideoContract {
            width: 4,
            height: 2,
            framerate_num: 60,
            framerate_denom: 1,
            maximum_framerate_num: 0,
            maximum_framerate_denom: 0,
            pixel_aspect_num: 0,
            pixel_aspect_denom: 0,
            chroma_site: 0,
            color_range: 0,
            color_matrix: 0,
            transfer_function: 0,
            color_primaries: 0,
        };
        let authored = GamescopeProfileBinding::author(GamescopeProfileBindingAuthoringInput {
            calibration_evidence_sha256: "1".repeat(64),
            environment_id: "test-machine".to_owned(),
            gamescope_version: "3.16.19".to_owned(),
            backend_id: "sdl".to_owned(),
            output_width: 4,
            output_height: 2,
            nested_width: 4,
            nested_height: 2,
            nested_refresh_hz: 60,
            scaler: "auto".to_owned(),
            filter: "linear".to_owned(),
            observed_video_contract: video,
            memory_type: UncalibratedMemoryType::MemoryPointer,
            stride: 16,
            geometry: FractionalRectangle::new(
                RationalCoordinate::new(0, 1).unwrap(),
                RationalCoordinate::new(0, 1).unwrap(),
                RationalCoordinate::new(4, 1).unwrap(),
                RationalCoordinate::new(2, 1).unwrap(),
            ),
        })
        .unwrap();
        fs::write(&path, &authored.bytes).unwrap();

        assert!(read_binding(&path, &authored.artifact_sha256).is_ok());
        assert!(read_binding(&path, &"f".repeat(64)).is_err());
        fs::write(&path, vec![0; super::MAX_BINDING_BYTES + 1]).unwrap();
        assert!(read_binding(&path, &authored.artifact_sha256).is_err());
    }

    #[test]
    fn binding_gate_failure_report_omits_paths_and_session_values() {
        let path = std::path::Path::new("/PRIVATE/BINDING/PATH");
        let report = run_gamescope_binding_admission_gate(path, &"f".repeat(64));
        let encoded = serde_json::to_string(&report).unwrap();

        assert!(!encoded.contains("PRIVATE"));
        assert!(encoded.contains("binding_unavailable"));
    }

    #[test]
    fn canonical_gate_failure_report_omits_paths_and_session_values() {
        let report = run_gamescope_canonical_frame_gate(
            std::path::Path::new("/PRIVATE/BINDING/PATH"),
            &"f".repeat(64),
            CaptureGeneration::new(7).unwrap(),
        );
        let encoded = serde_json::to_string(&report).unwrap();

        assert!(!encoded.contains("PRIVATE"));
        assert!(encoded.contains("binding_unavailable"));
        assert!(encoded.contains("\"capture_generation\":7"));
        assert!(encoded.contains("\"canonical_rgb8_sha256\":null"));
    }

    #[test]
    fn diagnostic_handoff_failure_report_omits_paths_and_session_values() {
        let _session = scorepeek::capture::GamescopeSessionProvenance::new(
            scorepeek::capture::GamescopeSessionProvenanceInput {
                environment_id: "PRIVATE-ENVIRONMENT".to_owned(),
                gamescope_version: "PRIVATE-VERSION".to_owned(),
                backend_id: "PRIVATE-BACKEND".to_owned(),
                output_width: 4,
                output_height: 2,
                nested_width: 4,
                nested_height: 2,
                nested_refresh_hz: 60,
                scaler: "auto".to_owned(),
                filter: "linear".to_owned(),
            },
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let generation = CaptureGeneration::new(9).unwrap();
        let report =
            run_gamescope_diagnostic_handoff_gate(super::GamescopeDiagnosticHandoffGateConfig {
                binding_path: std::path::Path::new("/PRIVATE/BINDING/PATH"),
                expected_binding_sha256: &"f".repeat(64),
                capture_generation: generation,
                descriptor: diagnostic_descriptor(generation.get()),
                policy: DiagnosticPolicy::default(),
                duration_ms: 1_000,
                diagnostic_root: root.path(),
                expected_source_node_id: None,
            });
        let encoded = serde_json::to_string(&report).unwrap();

        assert!(!encoded.contains("PRIVATE"));
        assert!(encoded.contains("binding_unavailable"));
        assert!(encoded.contains("\"capture_generation\":9"));
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn recognition_handoff_failure_is_typed_without_private_values() {
        let _session = scorepeek::capture::GamescopeSessionProvenance::new(
            scorepeek::capture::GamescopeSessionProvenanceInput {
                environment_id: "PRIVATE-ENVIRONMENT".to_owned(),
                gamescope_version: "PRIVATE-VERSION".to_owned(),
                backend_id: "PRIVATE-BACKEND".to_owned(),
                output_width: 4,
                output_height: 2,
                nested_width: 4,
                nested_height: 2,
                nested_refresh_hz: 60,
                scaler: "auto".to_owned(),
                filter: "linear".to_owned(),
            },
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let generation = CaptureGeneration::new(10).unwrap();
        let mut descriptor = diagnostic_descriptor(generation.get());
        descriptor.binding.canonical_layout_sha256 = "2".repeat(64);
        let report =
            run_gamescope_recognition_handoff_gate(super::GamescopeDiagnosticHandoffGateConfig {
                binding_path: std::path::Path::new("/PRIVATE/BINDING/PATH"),
                expected_binding_sha256: &"f".repeat(64),
                capture_generation: generation,
                descriptor,
                policy: DiagnosticPolicy::default(),
                duration_ms: 1_000,
                diagnostic_root: root.path(),
                expected_source_node_id: None,
            });
        let encoded = serde_json::to_string(&report).unwrap();

        assert!(!encoded.contains("PRIVATE"));
        assert!(encoded.contains("diagnostic_configuration_invalid"));
        assert!(encoded.contains("\"capture_generation\":10"));
        assert!(encoded.contains("\"inspected_frames\":0"));
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn lifecycle_run_count_is_explicitly_bounded() {
        assert_eq!(parse_lifecycle_runs(OsStr::new("2")).unwrap(), 2);
        assert_eq!(parse_lifecycle_runs(OsStr::new("100")).unwrap(), 100);
        assert!(parse_lifecycle_runs(OsStr::new("1")).is_err());
        assert!(parse_lifecycle_runs(OsStr::new("101")).is_err());
        assert!(parse_lifecycle_runs(OsStr::new("many")).is_err());
    }

    #[test]
    fn lifecycle_error_precedence_is_stable() {
        assert_eq!(
            lifecycle_error_type(true, true, true),
            Some(LifecycleGateErrorType::CaptureRunFailed)
        );
        assert_eq!(
            lifecycle_error_type(false, true, true),
            Some(LifecycleGateErrorType::ProcessResourceUnavailable)
        );
        assert_eq!(
            lifecycle_error_type(false, false, true),
            Some(LifecycleGateErrorType::ExpectedOverwriteMissing)
        );
        assert_eq!(lifecycle_error_type(false, false, false), None);
    }

    #[test]
    fn lifecycle_summary_extracts_only_typed_receiver_facts() {
        let report = GamescopeLiveGateReport {
            schema: "test",
            status: LiveGateStatus::Success,
            requested_duration_ms: 100,
            consumer_interval_ms: 25,
            consumed_frames: 2,
            first_sequence: Some(0),
            last_sequence: Some(4),
            error_type: None,
            diagnostic_facts: vec![
                CaptureDiagnosticFact {
                    sequence: 0,
                    monotonic_start_ms: 0,
                    monotonic_end_ms: 1,
                    operation: CaptureDiagnosticOperation::StreamNegotiation,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::StreamNegotiation {
                        format: "BGRx",
                        requested_framerate_num: 60,
                        requested_framerate_denom: 1,
                        width: 1280,
                        height: 720,
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
                    },
                },
                CaptureDiagnosticFact {
                    sequence: 1,
                    monotonic_start_ms: 1,
                    monotonic_end_ms: 2,
                    operation: CaptureDiagnosticOperation::FirstFrame,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::FirstFrame {
                        memory_type: "mem_fd",
                        stride: 5120,
                        byte_count: 3_686_400,
                    },
                },
                CaptureDiagnosticFact {
                    sequence: 2,
                    monotonic_start_ms: 2,
                    monotonic_end_ms: 100,
                    operation: CaptureDiagnosticOperation::SteadyReception,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::SteadyReception {
                        received_frames: 5,
                        overwritten_frames: 3,
                        last_sequence: Some(4),
                        maximum_gap_ns: 17_000_000,
                    },
                },
                CaptureDiagnosticFact {
                    sequence: 3,
                    monotonic_start_ms: 100,
                    monotonic_end_ms: 101,
                    operation: CaptureDiagnosticOperation::ReceiverShutdown,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::ReceiverShutdown {
                        received_frames: 5,
                        overwritten_frames: 3,
                    },
                },
                CaptureDiagnosticFact {
                    sequence: 4,
                    monotonic_start_ms: 101,
                    monotonic_end_ms: 102,
                    operation: CaptureDiagnosticOperation::Shutdown,
                    status: CaptureDiagnosticStatus::Success,
                    error_type: None,
                    detail: CaptureDiagnosticDetail::Shutdown {
                        source: CaptureSourceKind::GamescopeDefaultRemote,
                    },
                },
            ],
            dropped_diagnostic_facts: 0,
        };

        let summary = summarize_run(7, &report);
        assert_eq!(summary.run, 7);
        assert_eq!(summary.received_frames, 5);
        assert_eq!(summary.overwritten_frames, 3);
        assert_eq!(summary.last_sequence, Some(4));
        assert_eq!(summary.maximum_gap_ns, 17_000_000);
        assert_lifecycle_phases_succeeded(&summary);
        assert_eq!(summary.error_type, None::<CaptureErrorType>);
    }

    fn assert_lifecycle_phases_succeeded(summary: &super::LifecycleRunSummary) {
        assert_eq!(summary.phases.negotiation, LifecyclePhaseStatus::Success);
        assert_eq!(summary.phases.first_frame, LifecyclePhaseStatus::Success);
        assert_eq!(
            summary.phases.receiver_shutdown,
            LifecyclePhaseStatus::Success
        );
        assert_eq!(
            summary.phases.provider_shutdown,
            LifecyclePhaseStatus::Success
        );
    }

    #[test]
    fn process_resources_are_available_on_linux() {
        let snapshot = process_resource_snapshot().unwrap();
        assert!(snapshot.open_file_descriptors > 0);
        assert!(snapshot.threads > 0);
        assert!(snapshot.resident_bytes > 0);
    }

    #[test]
    fn diagnostic_sink_drops_new_facts_at_capacity() {
        let mut sink = BoundedDiagnosticSink::default();
        for sequence in 0..=MAX_DIAGNOSTIC_FACTS as u64 {
            sink.record(CaptureDiagnosticFact {
                sequence,
                monotonic_start_ms: 0,
                monotonic_end_ms: 0,
                operation: CaptureDiagnosticOperation::Shutdown,
                status: CaptureDiagnosticStatus::Success,
                error_type: None,
                detail: CaptureDiagnosticDetail::Shutdown {
                    source: CaptureSourceKind::GamescopeDefaultRemote,
                },
            });
        }
        assert_eq!(sink.facts.len(), MAX_DIAGNOSTIC_FACTS);
        assert_eq!(sink.dropped, 1);
    }

    #[test]
    fn compact_field_report_links_to_value_bearing_artifact_without_duplicating_it() {
        let mut report = field_observation_report(
            None,
            None,
            CaptureGeneration::new(7).unwrap(),
            FieldObservationCounters {
                observed_frames: 3,
                normalized_frames: 3,
                inspected_frames: 3,
                result_frames: 1,
                music_select_frames: 1,
                unknown_frames: 1,
                field_not_applicable: 1,
                field_submitted: 2,
                field_ready_success: 2,
                candidate_sets: 2,
                scored_candidates: 10,
                result_observations: 2,
                recognition_artifact_enqueued: 2,
                ..FieldObservationCounters::default()
            },
            FieldObservationFinishOutcomes {
                field_observer: None,
                diagnostic: None,
                recognition_artifact: Some(RecognitionArtifactFinishOutcome {
                    status: RecognitionArtifactFinishStatus::Complete,
                    manifest_sha256: Some("e".repeat(64)),
                    input_observations: 2,
                    retained_observations: 2,
                }),
                artifact_requested: true,
            },
            BoundedDiagnosticSink::default(),
        );
        report.failure_detail = Some("operator-only resource cause".to_owned());
        let encoded = serde_json::to_string(&report).unwrap();

        assert!(encoded.contains("\"candidate_sets\":2"));
        assert!(encoded.contains("\"scored_candidates\":10"));
        assert!(encoded.contains("scorepeek-gamescope-result-recognition-gate-v1"));
        assert!(encoded.contains("\"recognition_artifact_status\":\"complete\""));
        assert!(encoded.contains(&"e".repeat(64)));
        assert!(report.succeeded());
        for forbidden in ["open_text", "song_id", "pixels", "environment_id"] {
            assert!(!encoded.contains(forbidden));
        }
        assert!(!encoded.contains("operator-only resource cause"));
    }

    #[test]
    fn counts_only_field_report_retains_its_v1_shape() {
        let report = field_observation_report(
            None,
            None,
            CaptureGeneration::new(1).unwrap(),
            FieldObservationCounters::default(),
            FieldObservationFinishOutcomes {
                field_observer: None,
                diagnostic: None,
                recognition_artifact: None,
                artifact_requested: false,
            },
            BoundedDiagnosticSink::default(),
        );
        let encoded = serde_json::to_string(&report).unwrap();

        assert!(encoded.contains("scorepeek-gamescope-field-observation-gate-v1"));
        assert!(!encoded.contains("result_observations"));
        assert!(!encoded.contains("recognition_artifact"));
    }

    #[test]
    fn startup_retry_classification_distinguishes_transient_boundaries() {
        for error_type in [
            FieldObservationGateErrorType::CaptureFailed,
            FieldObservationGateErrorType::AdmissionRejected,
        ] {
            let report = field_observation_report(
                Some(error_type),
                None,
                CaptureGeneration::new(1).unwrap(),
                FieldObservationCounters::default(),
                FieldObservationFinishOutcomes {
                    field_observer: None,
                    diagnostic: None,
                    recognition_artifact: None,
                    artifact_requested: false,
                },
                BoundedDiagnosticSink::default(),
            );
            assert_eq!(
                report.startup_retry(),
                Some(super::LiveSessionStartupRetry::Admission)
            );
        }

        for error_type in [
            FieldObservationGateErrorType::CatalogUnavailable,
            FieldObservationGateErrorType::CatalogBindingMismatch,
            FieldObservationGateErrorType::CatalogLoadFailed,
        ] {
            let report = field_observation_report(
                Some(error_type),
                None,
                CaptureGeneration::new(1).unwrap(),
                FieldObservationCounters::default(),
                FieldObservationFinishOutcomes {
                    field_observer: None,
                    diagnostic: None,
                    recognition_artifact: None,
                    artifact_requested: false,
                },
                BoundedDiagnosticSink::default(),
            );
            assert_eq!(
                report.startup_retry(),
                Some(super::LiveSessionStartupRetry::Catalog)
            );
        }

        let mut report = field_observation_report(
            Some(FieldObservationGateErrorType::DiagnosticConfigurationInvalid),
            None,
            CaptureGeneration::new(1).unwrap(),
            FieldObservationCounters::default(),
            FieldObservationFinishOutcomes {
                field_observer: None,
                diagnostic: None,
                recognition_artifact: None,
                artifact_requested: false,
            },
            BoundedDiagnosticSink::default(),
        );
        report.failure_detail = Some("catalog binding does not match".to_owned());

        assert_eq!(report.startup_retry(), None);
        assert_eq!(
            report.startup_failure_summary(),
            "catalog binding does not match"
        );
    }

    #[test]
    fn result_evidence_requires_a_completed_result_observation() {
        let counters = FieldObservationCounters {
            field_ready_success: 1,
            candidate_sets: 1,
            recognition_artifact_enqueued: 1,
            ..FieldObservationCounters::default()
        };
        assert_eq!(
            result_evidence_error(&counters),
            Some(FieldObservationGateErrorType::ResultObservationUnavailable)
        );
    }

    #[test]
    fn artifact_failures_produce_the_same_error_report_status_as_cli_failure() {
        let complete = RecognitionArtifactFinishOutcome {
            status: RecognitionArtifactFinishStatus::Complete,
            manifest_sha256: Some("f".repeat(64)),
            input_observations: 1,
            retained_observations: 1,
        };
        let cases = [
            (
                FieldObservationCounters {
                    field_ready_success: 1,
                    result_observations: 1,
                    recognition_artifact_enqueued: 1,
                    recognition_artifact_queue_full: 1,
                    ..FieldObservationCounters::default()
                },
                RecognitionArtifactFinishOutcome {
                    status: RecognitionArtifactFinishStatus::Complete,
                    manifest_sha256: Some("f".repeat(64)),
                    input_observations: 1,
                    retained_observations: 1,
                },
            ),
            (
                FieldObservationCounters {
                    field_ready_success: 1,
                    result_observations: 1,
                    recognition_artifact_enqueued: 1,
                    ..FieldObservationCounters::default()
                },
                RecognitionArtifactFinishOutcome {
                    status: RecognitionArtifactFinishStatus::WriteFailed,
                    manifest_sha256: None,
                    input_observations: 1,
                    retained_observations: 0,
                },
            ),
            (
                FieldObservationCounters {
                    field_ready_success: 1,
                    result_observations: 1,
                    recognition_artifact_enqueued: 1,
                    ..FieldObservationCounters::default()
                },
                RecognitionArtifactFinishOutcome {
                    status: RecognitionArtifactFinishStatus::Timeout,
                    manifest_sha256: None,
                    input_observations: 1,
                    retained_observations: 0,
                },
            ),
        ];

        for (counters, outcome) in cases {
            let error = recognition_artifact_error(&counters, Some(&outcome));
            assert_eq!(
                error,
                Some(FieldObservationGateErrorType::RecognitionArtifactIncomplete)
            );
            let report = field_observation_report(
                error,
                None,
                CaptureGeneration::new(1).unwrap(),
                counters,
                FieldObservationFinishOutcomes {
                    field_observer: None,
                    diagnostic: None,
                    recognition_artifact: Some(outcome),
                    artifact_requested: true,
                },
                BoundedDiagnosticSink::default(),
            );
            let encoded = serde_json::to_string(&report).unwrap();
            assert!(!report.succeeded());
            assert!(encoded.contains("\"status\":\"error\""));
            assert!(encoded.contains("\"error_type\":\"recognition_artifact_incomplete\""));
        }

        let counters = FieldObservationCounters {
            field_ready_success: 1,
            result_observations: 1,
            recognition_artifact_enqueued: 1,
            ..FieldObservationCounters::default()
        };
        assert_eq!(recognition_artifact_error(&counters, Some(&complete)), None);
    }

    #[test]
    fn field_gate_keeps_registered_resource_failures_actionable() {
        assert_eq!(
            field_resource_error(RegisteredResourceLoadErrorType::CatalogBindingMismatch),
            FieldObservationGateErrorType::CatalogBindingMismatch
        );
        assert_eq!(
            field_resource_error(RegisteredResourceLoadErrorType::RuntimeInitializationFailed),
            FieldObservationGateErrorType::RuntimeInitializationFailed
        );
        let (error_type, finish, detail) = field_start_error(
            crate::recognition_live::field_session::FieldObservationStartError::FieldObserver(
                crate::recognition_live::field_observer::FieldObserverStartError::Load(
                    crate::recognition_live::screen_field_observer::RegisteredScreenFieldObserverLoadError::Resources(
                        RegisteredResourceLoadError::InvalidLocation {
                            role: "model bundle",
                            source: Some(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
                        },
                    ),
                ),
            ),
        );
        assert_eq!(
            error_type,
            FieldObservationGateErrorType::InvalidResourceLocation
        );
        assert!(finish.is_none());
        let detail = detail.unwrap();
        assert!(detail.contains("model bundle"));
        assert!(detail.contains("permission denied"));
    }
}
