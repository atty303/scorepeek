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

#[cfg(test)]
use scorepeek::capture::acquire_gamescope_source;
use scorepeek::capture::{
    AdmittedFrameNormalizer, CalibratedVulkanLease, CaptureDiagnosticDetail, CaptureDiagnosticFact,
    CaptureDiagnosticOperation, CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType,
    EdgeCrop, NormalizedCanonicalFrame, PipewireCaptureLease, RuntimeCaptureEvidence,
    acquire_pipewire_source, admit_pipewire_source, admit_vulkan_session, start_pipewire_receiver,
};
use serde::Serialize;

use crate::canonical_source::CanonicalFrameSource;
use crate::diagnostics::contract::{
    DiagnosticCompleteness, DiagnosticErrorType, DiagnosticPolicy, DiagnosticRunDescriptor,
    DiagnosticRunStatus,
};
use crate::recording::writer::{CanonicalRecordingCompleteness, CanonicalRecordingWorker};
use crate::service::session::recognition::BoundCanonicalFrame;
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
use scorepeek_core::frame::CanonicalLayout;
use scorepeek_core::game_version::GameVersionResolver;
use scorepeek_core::model::session::RegisteredScreenFieldObservation;
use scorepeek_core::recognition::screen::{ScreenClass, ScreenFieldObservationError};
use scorepeek_core::recognition::title::OnnxParityError;
use scorepeek_core::session::episode::{RawScreenState, SemanticScreenEpisode};
use scorepeek_core::session::result::{CadenceDecision, RecognitionCadence};
use scorepeek_core::session::timeline::{TimelineAction, TimelineDriver};
use scorepeek_resources::recognition::RegisteredResourceLoadErrorType;

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
pub enum CaptureSessionEvent<'a> {
    Started {
        source_evidence: &'a RuntimeCaptureEvidence,
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

type LiveEventEmitter<'e> =
    dyn for<'a> FnMut(CaptureSessionEvent<'a>) -> Result<LiveEventProcessingTiming, String> + 'e;

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
pub struct CaptureSessionReport {
    schema: &'static str,
    status: LiveGateStatus,
    error_type: Option<FieldObservationGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
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

impl CaptureSessionReport {
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
}

pub struct LiveCaptureSessionConfig<'a> {
    pub execution_context: crate::service::session::recognition::RecognitionExecutionContext,
    pub descriptor: DiagnosticRunDescriptor,
    pub diagnostic_policy: DiagnosticPolicy,
    pub diagnostic_root: &'a std::path::Path,
    pub diagnostic_directory_name: Option<&'a str>,
    pub catalog_root: &'a std::path::Path,
    pub bundle_root: &'a std::path::Path,
    pub canonical_recording_root: Option<&'a std::path::Path>,
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
}

type CaptureLease = Box<dyn ConnectedCaptureAdapter>;

trait ConnectedCaptureAdapter {
    fn take_latest_observed_frame(&mut self) -> Option<scorepeek::capture::ObservedFrame>;
    fn frame_normalizer(&self) -> AdmittedFrameNormalizer;
    fn record_worker_normalization(
        &mut self,
        source_sequence: u64,
        error_type: Option<CaptureErrorType>,
        sink: &mut BoundedDiagnosticSink,
    );
    fn poll(
        &mut self,
        timeout: Duration,
        sink: &mut BoundedDiagnosticSink,
    ) -> Result<(), scorepeek::capture::CaptureError>;
    fn shutdown_with_elapsed(
        self: Box<Self>,
        sink: &mut BoundedDiagnosticSink,
    ) -> (Result<(), scorepeek::capture::CaptureError>, u64);
    fn shutdown(
        self: Box<Self>,
        sink: &mut BoundedDiagnosticSink,
    ) -> Result<(), scorepeek::capture::CaptureError> {
        self.shutdown_with_elapsed(sink).0
    }
}

macro_rules! impl_connected_capture_adapter {
    ($lease:ty) => {
        impl ConnectedCaptureAdapter for $lease {
            fn take_latest_observed_frame(&mut self) -> Option<scorepeek::capture::ObservedFrame> {
                self.take_latest_observed_frame()
            }
            fn frame_normalizer(&self) -> AdmittedFrameNormalizer {
                self.frame_normalizer()
            }
            fn record_worker_normalization(
                &mut self,
                source_sequence: u64,
                error_type: Option<CaptureErrorType>,
                sink: &mut BoundedDiagnosticSink,
            ) {
                self.record_worker_normalization(source_sequence, error_type, sink);
            }
            fn poll(
                &mut self,
                timeout: Duration,
                sink: &mut BoundedDiagnosticSink,
            ) -> Result<(), scorepeek::capture::CaptureError> {
                self.poll(timeout, sink)
            }
            fn shutdown_with_elapsed(
                self: Box<Self>,
                sink: &mut BoundedDiagnosticSink,
            ) -> (Result<(), scorepeek::capture::CaptureError>, u64) {
                (*self).shutdown_with_elapsed(sink)
            }
        }
    };
}

impl_connected_capture_adapter!(PipewireCaptureLease);
impl_connected_capture_adapter!(CalibratedVulkanLease);

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
        emit(CaptureSessionEvent::CaptureDiagnostic { fact: &fact })
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

#[path = "live/field_observation.rs"]
mod field_observation;

pub(crate) use field_observation::run_runtime_live_session;
#[cfg(test)]
use field_observation::{
    FieldObservationFinishOutcomes, field_observation_report, field_resource_error,
    field_start_error, reconnectable_stop_reason,
};

#[cfg(test)]
#[path = "live/tests.rs"]
mod tests;

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
