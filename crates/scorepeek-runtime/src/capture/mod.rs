//! Linux capture capabilities and canonical normalization.

pub mod diagnostics;
pub(crate) mod live;
pub mod normalization;
pub mod pipewire;
pub mod source;
pub mod vulkan;

pub use diagnostics::{
    CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
    CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType, CaptureSourceKind,
    VulkanTimingDistribution,
};
pub use normalization::{
    CanonicalRegion, FractionalLinearGeometry, FractionalRectangle, NormalizedCanonicalFrame,
    RationalCoordinate, UnboundCanonicalFrame, UnboundNormalizationError,
};
pub use pipewire::discovery::{
    CaptureError, PipewireSourceProbe, PipewireSourceSnapshot, UncalibratedPipewireSourceLease,
    acquire_gamescope_source, acquire_pipewire_source, probe_gamescope_source,
    snapshot_gamescope_sources, snapshot_pipewire_sources,
};
pub(crate) use pipewire::discovery::{ITERATION_SLICE, elapsed_ms};
pub use pipewire::lifecycle::{PipewireLeaseAdmissionFailure, admit_pipewire_source};
pub use pipewire::receiver::{
    AdmittedFrameNormalizer, CalibratedVulkanLease, ObservedFrame, PipewireCaptureLease,
    UncalibratedPipeWireReceiver, admit_vulkan_session, start_pipewire_receiver,
};
pub use source::{
    BgrxFrame, EdgeCrop, RuntimeCaptureEvidence, UncalibratedFrame, UncalibratedMemoryType,
    UncalibratedVideoContract,
};
