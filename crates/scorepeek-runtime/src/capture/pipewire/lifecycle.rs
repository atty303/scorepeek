//! `PipeWire` admission failure lifecycle with explicit receiver ownership.

use std::fmt;
use std::sync::Arc;

use super::receiver::{CalibratedGamescopeLease, ReceiverState, UncalibratedPipeWireReceiver};
use crate::capture::{
    AuthoredGamescopeProfileBinding, CaptureDiagnosticDetail, CaptureDiagnosticOperation,
    CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureError, CaptureErrorType,
    CaptureGeneration, EdgeCrop, GamescopeProfileBinding, ObservedContractMismatch,
    RuntimeCaptureBackend,
};

/// A rejected admission that retains ownership of the live receiver for explicit shutdown.
pub struct GamescopeLeaseAdmissionFailure {
    pub(super) error_type: CaptureErrorType,
    pub(super) receiver: UncalibratedPipeWireReceiver,
}

impl fmt::Debug for GamescopeLeaseAdmissionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GamescopeLeaseAdmissionFailure")
            .field("error_type", &self.error_type)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for GamescopeLeaseAdmissionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        CaptureError::without_source(self.error_type).fmt(formatter)
    }
}

impl std::error::Error for GamescopeLeaseAdmissionFailure {}

impl GamescopeLeaseAdmissionFailure {
    #[must_use]
    pub const fn error_type(&self) -> CaptureErrorType {
        self.error_type
    }

    /// Releases the rejected receiver and provider in the normal order.
    ///
    /// # Errors
    /// Returns a receiver shutdown failure without replacing the admission rejection category.
    pub fn shutdown(self, sink: &mut impl CaptureDiagnosticSink) -> Result<(), CaptureError> {
        self.receiver.shutdown(sink)
    }
}

/// Admits a started receiver only when its explicit session and negotiated contract match.
///
/// Exactly one value-free admission fact is offered to the host sink. Sink absence or capacity
/// does not change the returned result. Rejection retains the receiver for explicit shutdown.
///
/// # Errors
/// Returns a stable provenance or negotiated-contract mismatch category.
pub fn admit_gamescope_profile(
    mut receiver: UncalibratedPipeWireReceiver,
    binding: GamescopeProfileBinding,
    capture_generation: CaptureGeneration,
    sink: &mut impl CaptureDiagnosticSink,
) -> Result<CalibratedGamescopeLease, Box<GamescopeLeaseAdmissionFailure>> {
    receiver.flush_observations(sink);
    let error_type = classify_profile_admission(
        &binding,
        &receiver
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .err();
    receiver.record(
        sink,
        CaptureDiagnosticOperation::ProfileBindingAdmission,
        if error_type.is_some() {
            CaptureDiagnosticStatus::Error
        } else {
            CaptureDiagnosticStatus::Success
        },
        error_type,
        CaptureDiagnosticDetail::ProfileBindingAdmission,
    );
    if let Some(error_type) = error_type {
        return Err(Box::new(GamescopeLeaseAdmissionFailure {
            error_type,
            receiver,
        }));
    }
    let capture_profile_sha256 = Arc::from(binding.capture_profile_sha256());
    let normalizer_artifact_sha256 = Arc::from(binding.normalizer_artifact_sha256());
    let geometry = binding.geometry();
    drop(binding);
    Ok(CalibratedGamescopeLease {
        receiver,
        capture_profile_sha256,
        normalizer_artifact_sha256,
        geometry,
        capture_generation,
        frame_domain: Arc::new(()),
        normalization_success_recorded: false,
        normalization_failure_recorded: false,
    })
}

/// Creates and admits an immutable runtime binding from the negotiated source contract.
///
/// # Errors
/// Returns ownership of the receiver with a typed admission failure when its negotiated contract
/// is incomplete, the crop is invalid, or the authored identity cannot be admitted.
pub fn admit_runtime_profile(
    receiver: UncalibratedPipeWireReceiver,
    backend: RuntimeCaptureBackend,
    selector: String,
    crop: EdgeCrop,
    capture_generation: CaptureGeneration,
    sink: &mut impl CaptureDiagnosticSink,
) -> Result<
    (CalibratedGamescopeLease, AuthoredGamescopeProfileBinding),
    Box<GamescopeLeaseAdmissionFailure>,
> {
    let observed = {
        let state = receiver
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.contract, state.memory_type, state.stride)
    };
    let (Some(video), Some(memory_type), Some(stride)) = observed else {
        return Err(Box::new(GamescopeLeaseAdmissionFailure {
            error_type: CaptureErrorType::ProfileVideoContractMismatch,
            receiver,
        }));
    };
    let authored = GamescopeProfileBinding::author_runtime(
        backend,
        selector,
        video,
        memory_type,
        stride,
        crop,
    );
    let Ok(authored) = authored else {
        return Err(Box::new(GamescopeLeaseAdmissionFailure {
            error_type: CaptureErrorType::FrameNormalizationFailed,
            receiver,
        }));
    };
    let Ok(binding) = GamescopeProfileBinding::parse(&authored.bytes, &authored.artifact_sha256)
    else {
        return Err(Box::new(GamescopeLeaseAdmissionFailure {
            error_type: CaptureErrorType::FrameNormalizationFailed,
            receiver,
        }));
    };
    admit_gamescope_profile(receiver, binding, capture_generation, sink)
        .map(|lease| (lease, authored))
}

pub(super) fn classify_profile_admission(
    binding: &GamescopeProfileBinding,
    state: &ReceiverState,
) -> Result<(), CaptureErrorType> {
    let video = state
        .contract
        .ok_or(CaptureErrorType::ProfileVideoContractMismatch)?;
    let memory_type = state
        .memory_type
        .ok_or(CaptureErrorType::ProfileMemoryTypeMismatch)?;
    let stride = state
        .stride
        .ok_or(CaptureErrorType::ProfileStrideMismatch)?;
    binding
        .verify_observed_contract(video, memory_type, stride)
        .map_err(observed_mismatch_error)
}

const fn observed_mismatch_error(mismatch: ObservedContractMismatch) -> CaptureErrorType {
    match mismatch {
        ObservedContractMismatch::Video => CaptureErrorType::ProfileVideoContractMismatch,
        ObservedContractMismatch::MemoryType => CaptureErrorType::ProfileMemoryTypeMismatch,
        ObservedContractMismatch::Stride => CaptureErrorType::ProfileStrideMismatch,
    }
}
