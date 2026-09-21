//! Linux capture capabilities and canonical normalization.

pub mod diagnostics;
pub mod generation;
pub(crate) mod live;
pub mod normalization;
pub mod pipewire;
pub mod profile;
pub mod vulkan;

pub use diagnostics::{
    CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
    CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType, CaptureSourceKind,
    VulkanTimingDistribution,
};
pub use generation::{CaptureGeneration, InvalidCaptureGeneration};
pub use normalization::{
    CanonicalRegion, FractionalLinearGeometry, FractionalRectangle, NormalizedCanonicalFrame,
    RationalCoordinate, UnboundCanonicalFrame, UnboundNormalizationError,
};
pub use pipewire::contract::{
    UncalibratedFrame, UncalibratedMemoryType, UncalibratedVideoContract,
};
pub use pipewire::discovery::{
    CaptureError, PipewireSourceProbe, PipewireSourceSnapshot, UncalibratedPipewireSourceLease,
    acquire_gamescope_source, acquire_pipewire_source, probe_gamescope_source,
    snapshot_gamescope_sources, snapshot_pipewire_sources,
};
pub(crate) use pipewire::discovery::{ITERATION_SLICE, elapsed_ms};
pub use pipewire::lifecycle::{
    GamescopeLeaseAdmissionFailure, admit_gamescope_profile, admit_runtime_profile,
};
pub use pipewire::receiver::{
    AdmittedFrameNormalizer, CalibratedGamescopeLease, CalibratedSourceFrameEvidence,
    CalibratedVulkanLease, ObservedFrame, UncalibratedPipeWireReceiver, admit_vulkan_session,
    start_uncalibrated_gamescope_receiver,
};
pub use profile::{
    AuthoredGamescopeProfileBinding, EdgeCrop, GamescopeProfileBinding,
    GamescopeProfileBindingAuthoringInput, GamescopeProfileBindingError,
    GamescopeSessionProvenance, GamescopeSessionProvenanceInput,
    GamescopeSessionProvenanceMismatch, MeasuredGamescopeProfileBindingAuthoringInput,
    ObservedContractMismatch, RuntimeCaptureBackend,
};
