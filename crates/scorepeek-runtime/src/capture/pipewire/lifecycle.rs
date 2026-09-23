//! `PipeWire` admission failure lifecycle with explicit receiver ownership.

use std::fmt;
use std::sync::Arc;

use super::receiver::{PipewireCaptureLease, UncalibratedPipeWireReceiver};
use crate::capture::{
    CaptureDiagnosticDetail, CaptureDiagnosticOperation, CaptureDiagnosticSink,
    CaptureDiagnosticStatus, CaptureError, CaptureErrorType, EdgeCrop, RuntimeCaptureEvidence,
};

/// A rejected admission that retains ownership of the live receiver for explicit shutdown.
pub struct PipewireLeaseAdmissionFailure {
    pub(super) error_type: CaptureErrorType,
    pub(super) receiver: UncalibratedPipeWireReceiver,
}

impl fmt::Debug for PipewireLeaseAdmissionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PipewireLeaseAdmissionFailure")
            .field("error_type", &self.error_type)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for PipewireLeaseAdmissionFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        CaptureError::without_source(self.error_type).fmt(formatter)
    }
}

impl std::error::Error for PipewireLeaseAdmissionFailure {}

impl PipewireLeaseAdmissionFailure {
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

/// Creates and admits an immutable runtime binding from the negotiated source contract.
///
/// # Errors
/// Returns ownership of the receiver with a typed admission failure when its negotiated contract
/// is incomplete, the crop is invalid, or the authored identity cannot be admitted.
///
/// # Panics
/// Panics only if serializing the fixed in-memory video contract fails.
pub fn admit_pipewire_source(
    mut receiver: UncalibratedPipeWireReceiver,
    crop: EdgeCrop,
    sink: &mut impl CaptureDiagnosticSink,
) -> Result<(PipewireCaptureLease, RuntimeCaptureEvidence), Box<PipewireLeaseAdmissionFailure>> {
    receiver.flush_observations(sink);
    let observed = {
        let state = receiver
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.contract, state.memory_type, state.stride)
    };
    let (Some(video), Some(memory_type), Some(stride)) = observed else {
        return Err(Box::new(PipewireLeaseAdmissionFailure {
            error_type: CaptureErrorType::SourceContractIncomplete,
            receiver,
        }));
    };
    let Ok((evidence, geometry)) = RuntimeCaptureEvidence::new(
        "pipewire",
        video.width,
        video.height,
        serde_json::to_value(video).expect("video contract is serializable"),
        memory_type,
        stride,
        crop,
    ) else {
        return Err(Box::new(PipewireLeaseAdmissionFailure {
            error_type: CaptureErrorType::FrameNormalizationFailed,
            receiver,
        }));
    };
    receiver.record(
        sink,
        CaptureDiagnosticOperation::SourceAdmission,
        CaptureDiagnosticStatus::Success,
        None,
        CaptureDiagnosticDetail::SourceAdmission,
    );
    Ok((
        PipewireCaptureLease {
            receiver,
            geometry,
            frame_domain: Arc::new(()),
            normalization_success_recorded: false,
            normalization_failure_recorded: false,
        },
        evidence,
    ))
}
