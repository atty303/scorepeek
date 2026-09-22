//! Narrow runtime capability used by canonical offline replay.

use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use scorepeek_core::diagnostics::{DiagnosticPolicy, DiagnosticRunDescriptor, DiagnosticRunStatus};
use scorepeek_core::model::session::{RecognitionExecutionMode, RegisteredScreenFieldObservation};
use scorepeek_core::recognition::screen::{
    RecognitionError, ScreenClass, ScreenFieldObservationError,
};
use scorepeek_core::recognition::title::OnnxParityError;
use sha2::{Digest as _, Sha256};

use crate::diagnostics::live::BoundCanonicalFrame;
use crate::service::session::recognition::field_observer::{
    BoundFieldObservation, FieldObserverFinishStatus, FieldObserverOfferError,
};
use crate::service::session::recognition::field_session::{
    FieldObservationFrameResult, FieldObservationSession, FieldObservationSessionPoll,
    FieldObservationSubmission, PendingSessionFieldObservation,
};
use crate::service::session::recognition::screen_field_observer::{
    RegisteredScreenFieldObserver, SharedRegisteredScreenFieldResources,
};
use crate::service::session::recognition::{
    PreparedRecognitionFrame, RecognitionObservation, RecognitionSessionError,
};

type ReplayFieldOutput =
    Result<RegisteredScreenFieldObservation, ScreenFieldObservationError<OnnxParityError>>;
type InnerSession = FieldObservationSession<RegisteredScreenFieldObserver>;

const RUNTIME_REPLAY_SOURCES: &[&[u8]] = &[
    include_bytes!("replay.rs"),
    include_bytes!("capture/profile.rs"),
    include_bytes!("diagnostics/live.rs"),
    include_bytes!("resources/model/acquire.rs"),
    include_bytes!("service/session/recognition/mod.rs"),
    include_bytes!("service/session/recognition/field_observer.rs"),
    include_bytes!("service/session/recognition/field_session.rs"),
    include_bytes!("service/session/recognition/screen_field_observer.rs"),
];

/// Returns the implementation identity of the production replay path.
#[must_use]
pub fn implementation_sha256() -> String {
    let core = scorepeek_core::replay::production_semantics_sha256();
    let mut digest = Sha256::new();
    digest.update(u64::try_from(core.len()).unwrap_or(u64::MAX).to_le_bytes());
    digest.update(core.as_bytes());
    for source in RUNTIME_REPLAY_SOURCES {
        digest.update(
            u64::try_from(source.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        digest.update(source);
    }
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn operation_main(operation: &'static str) -> ExitCode {
    crate::service::dispatch::development_operation_main(operation)
}

/// Runs the standalone canonical-frame inspector.
#[must_use]
pub fn recognition_inspect_main() -> ExitCode {
    operation_main("inspect")
}

/// Runs the standalone RESULT crop exporter.
#[must_use]
pub fn recognition_crop_main() -> ExitCode {
    operation_main("crop")
}

/// Runs the standalone MUSIC SELECT crop exporter.
#[must_use]
pub fn music_select_crop_main() -> ExitCode {
    operation_main("music-select-crop")
}

/// Runs the standalone integrated-context crop exporter.
#[must_use]
pub fn integrated_context_crop_main() -> ExitCode {
    operation_main("integrated-context-crop")
}

/// Runs the standalone provisional title candidate exporter.
#[must_use]
pub fn provisional_title_candidates_main() -> ExitCode {
    operation_main("provisional-title-candidates")
}

/// Runs the standalone title resolution probe.
#[must_use]
pub fn title_spike_main() -> ExitCode {
    operation_main("title-spike")
}

/// Immutable registered resources shared by offline replay sessions.
pub struct ReplaySharedResources(Arc<SharedRegisteredScreenFieldResources>);

impl ReplaySharedResources {
    /// Loads one registered catalog and a fixed-size offline text worker pool.
    ///
    /// # Errors
    /// Returns an error when the descriptor binding or registered resources are invalid.
    pub fn load(
        descriptor: &DiagnosticRunDescriptor,
        catalog_root: &Path,
        bundle_root: &Path,
        text_workers: usize,
    ) -> Result<Self, ReplayStartError> {
        SharedRegisteredScreenFieldResources::load(
            descriptor,
            catalog_root,
            bundle_root,
            text_workers,
        )
        .map(|resources| Self(Arc::new(resources)))
        .map_err(|error| ReplayStartError(error.to_string()))
    }

    /// Loads another catalog while retaining the registered text worker pool.
    ///
    /// # Errors
    /// Returns an error when the new descriptor is incompatible with the shared runtime.
    pub fn load_sharing_text_pool(
        descriptor: &DiagnosticRunDescriptor,
        catalog_root: &Path,
        bundle_root: &Path,
        shared: &Self,
    ) -> Result<Self, ReplayStartError> {
        SharedRegisteredScreenFieldResources::load_sharing_text_pool(
            descriptor,
            catalog_root,
            bundle_root,
            &shared.0,
        )
        .map(|resources| Self(Arc::new(resources)))
        .map_err(|error| ReplayStartError(error.to_string()))
    }
}

/// Pure classification and crop preparation retained for ordered replay consumption.
pub struct ReplayPreparedFrame(PreparedRecognitionFrame);

impl ReplayPreparedFrame {
    /// Prepares one canonical frame while retaining the scheduler admission time.
    ///
    /// # Errors
    /// Returns an error when the canonical pixels or embedded layouts are invalid.
    pub fn prepare_since(pixels: &[u8], started: Instant) -> Result<Self, RecognitionError> {
        PreparedRecognitionFrame::prepare_since(pixels, started).map(Self)
    }

    #[must_use]
    pub const fn screen_classification_us(&self) -> u64 {
        self.0.screen_classification_us()
    }

    #[must_use]
    pub const fn crop_prepare_us(&self) -> Option<u64> {
        self.0.crop_prepare_us()
    }
}

/// Offline-only production recognition session used by canonical corpus replay.
pub struct ReplayRecognitionSession(InnerSession);

impl ReplayRecognitionSession {
    /// Starts an offline session with independently loaded registered resources.
    ///
    /// # Errors
    /// Returns an error when the binding, resources, worker, or diagnostic session is invalid.
    pub fn start_registered(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        catalog_root: &Path,
        bundle_root: &Path,
    ) -> Result<Self, ReplayStartError> {
        InnerSession::start_registered(
            root,
            descriptor,
            policy,
            catalog_root,
            bundle_root,
            RecognitionExecutionMode::Offline,
        )
        .map(Self)
        .map_err(|error| ReplayStartError(format!("{error:?}")))
    }

    /// Starts an offline session using a shared registered text worker pool.
    ///
    /// # Errors
    /// Returns an error when the binding, numeric runtime, worker, or diagnostic session fails.
    pub fn start_registered_shared(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        shared: &ReplaySharedResources,
    ) -> Result<Self, ReplayStartError> {
        InnerSession::start_registered_shared(root, descriptor, policy, Arc::clone(&shared.0))
            .map(Self)
            .map_err(|error| ReplayStartError(format!("{error:?}")))
    }

    /// Inspects and submits one canonical frame to the registered field observer.
    ///
    /// # Errors
    /// Returns an error when the frame does not belong to this replay binding.
    pub fn inspect<'a>(
        &mut self,
        frame: &'a BoundCanonicalFrame,
    ) -> Result<ReplayFrameInspection<'a>, ReplayInspectError> {
        self.0
            .inspect(frame)
            .map(ReplayFrameInspection::from_inner)
            .map_err(ReplayInspectError)
    }

    /// Consumes one previously prepared frame in source order.
    ///
    /// # Errors
    /// Returns an error when the prepared value or frame does not match this binding.
    pub fn inspect_prepared<'a>(
        &mut self,
        frame: &'a BoundCanonicalFrame,
        prepared: ReplayPreparedFrame,
    ) -> Result<ReplayFrameInspection<'a>, ReplayInspectError> {
        self.0
            .inspect_prepared(frame, prepared.0)
            .map(ReplayFrameInspection::from_inner)
            .map_err(ReplayInspectError)
    }

    #[must_use]
    pub fn poll_field_observation(
        &mut self,
        pending: &ReplayPendingObservation,
    ) -> ReplayFieldPoll {
        ReplayFieldPoll::from_inner(self.0.poll_field_observation(&pending.0))
    }

    #[must_use]
    pub fn wait_field_observation(
        &mut self,
        pending: &ReplayPendingObservation,
        timeout: Duration,
    ) -> ReplayFieldPoll {
        ReplayFieldPoll::from_inner(self.0.wait_field_observation(&pending.0, timeout))
    }

    #[must_use]
    pub fn finish(
        self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
        field_observer_timeout: Duration,
    ) -> ReplayFinishOutcome {
        let outcome = self
            .0
            .finish(status, monotonic_end_ms, field_observer_timeout);
        ReplayFinishOutcome {
            field_observer_complete: outcome.field_observer.status
                == FieldObserverFinishStatus::Complete,
        }
    }

    /// Joins the field worker without a timeout during offline failure cleanup.
    #[must_use]
    pub fn finish_offline(
        self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
    ) -> ReplayFinishOutcome {
        let outcome = self.0.finish_offline(status, monotonic_end_ms);
        ReplayFinishOutcome {
            field_observer_complete: outcome.field_observer.status
                == FieldObserverFinishStatus::Complete,
        }
    }
}

/// Screen classification plus the bounded outcome of field submission.
pub struct ReplayFrameInspection<'a> {
    observation: RecognitionObservation<'a>,
    pub field_submission: ReplayFieldSubmission,
}

impl<'a> ReplayFrameInspection<'a> {
    fn from_inner(value: FieldObservationFrameResult<'a, ReplayFieldOutput>) -> Self {
        Self {
            observation: value.observation,
            field_submission: ReplayFieldSubmission::from_inner(value.field_submission),
        }
    }

    #[must_use]
    pub const fn screen(&self) -> ScreenClass {
        self.observation.screen()
    }

    #[must_use]
    pub const fn result_presence(
        &self,
    ) -> scorepeek_core::recognition::screen::ResultPresenceEvidence {
        self.observation.result_presence()
    }

    #[must_use]
    pub const fn play_presence(&self) -> scorepeek_core::recognition::screen::PlayPresenceEvidence {
        self.observation.play_presence()
    }
}

#[derive(Debug)]
pub enum ReplayFieldSubmission {
    NotApplicable,
    BusySkipped,
    Submitted(ReplayPendingObservation),
    Rejected(ReplayOfferError),
}

impl ReplayFieldSubmission {
    fn from_inner(value: FieldObservationSubmission<ReplayFieldOutput>) -> Self {
        match value {
            FieldObservationSubmission::NotApplicable => Self::NotApplicable,
            FieldObservationSubmission::BusySkipped => Self::BusySkipped,
            FieldObservationSubmission::Submitted(pending) => {
                Self::Submitted(ReplayPendingObservation(pending))
            }
            FieldObservationSubmission::Rejected(error) => Self::Rejected(ReplayOfferError(error)),
        }
    }
}

#[derive(Debug)]
pub struct ReplayPendingObservation(PendingSessionFieldObservation<ReplayFieldOutput>);

impl ReplayPendingObservation {
    pub fn bind_screen_episode(&mut self, screen_episode_id: u64) {
        self.0.bind_screen_episode(screen_episode_id);
    }
}

#[derive(Debug)]
// The ready observation stays inline to avoid adding an allocation to every replay field result.
#[allow(clippy::large_enum_variant)]
pub enum ReplayFieldPoll {
    Pending,
    Ready {
        observation: ReplayFieldObservation,
        frame_processing_wall_us: u64,
        screen_episode_id: u64,
    },
    Consumed,
    BindingMismatch,
    Terminal,
    WorkerUnavailable,
}

impl ReplayFieldPoll {
    fn from_inner(value: FieldObservationSessionPoll<ReplayFieldOutput>) -> Self {
        match value {
            FieldObservationSessionPoll::Pending => Self::Pending,
            FieldObservationSessionPoll::Ready {
                observation,
                timing,
                screen_episode_id,
                ..
            } => Self::Ready {
                observation: ReplayFieldObservation(observation),
                frame_processing_wall_us: timing.frame_processing_wall_us,
                screen_episode_id,
            },
            FieldObservationSessionPoll::Consumed => Self::Consumed,
            FieldObservationSessionPoll::BindingMismatch => Self::BindingMismatch,
            FieldObservationSessionPoll::Terminal => Self::Terminal,
            FieldObservationSessionPoll::WorkerUnavailable => Self::WorkerUnavailable,
        }
    }
}

#[derive(Debug)]
pub struct ReplayFieldObservation(BoundFieldObservation<ReplayFieldOutput>);

impl ReplayFieldObservation {
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.0.sequence()
    }

    #[must_use]
    pub const fn monotonic_start_ms(&self) -> u64 {
        self.0.monotonic_start_ms()
    }

    #[must_use]
    pub const fn monotonic_end_ms(&self) -> u64 {
        self.0.monotonic_end_ms()
    }

    pub const fn output(&self) -> &ReplayFieldOutput {
        self.0.output()
    }

    /// Consumes the observation and returns its field-recognition result.
    ///
    /// # Errors
    /// Returns the retained field-observation or ONNX inference error.
    pub fn into_output(self) -> ReplayFieldOutput {
        self.0.into_output()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayFinishOutcome {
    pub field_observer_complete: bool,
}

#[derive(Debug)]
pub struct ReplayStartError(String);

impl fmt::Display for ReplayStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ReplayStartError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayInspectError(RecognitionSessionError);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayOfferError(FieldObserverOfferError);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn implementation_digest_is_sha256() {
        let digest = implementation_sha256();
        assert_eq!(digest.len(), 64);
        assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }
}
