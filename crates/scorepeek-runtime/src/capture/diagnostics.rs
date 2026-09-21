//! Backend-neutral capture diagnostic contract.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSourceKind {
    Pipewire,
    VulkanLayer,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDiagnosticOperation {
    SourceAcquisition,
    RegistryDiscovery,
    SourceLifetime,
    StreamNegotiation,
    FirstFrame,
    ProfileBindingAdmission,
    FrameNormalization,
    SteadyReception,
    ReceiverShutdown,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureDiagnosticStatus {
    Success,
    Error,
    Timeout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureErrorType {
    RemoteConnectionFailed,
    RegistryUnavailable,
    RegistryTimedOut,
    RegistryLimitExceeded,
    SourceUnavailable,
    SourceAmbiguous,
    SourceLost,
    NegotiationTimedOut,
    FirstFrameTimedOut,
    UnsupportedFormat,
    SourceContractChanged,
    UnsupportedMemoryType,
    FrameMalformed,
    StreamLost,
    ReceiverFailed,
    ProfileSessionProvenanceMissing,
    ProfileEnvironmentMismatch,
    ProfileGamescopeVersionMismatch,
    ProfileBackendMismatch,
    ProfileOutputDimensionsMismatch,
    ProfileNestedDimensionsMismatch,
    ProfileNestedRefreshMismatch,
    ProfileScalerMismatch,
    ProfileFilterMismatch,
    ProfileVideoContractMismatch,
    ProfileMemoryTypeMismatch,
    ProfileStrideMismatch,
    FrameLeaseMismatch,
    FrameGenerationMismatch,
    FrameProfileMismatch,
    FrameNormalizerMismatch,
    FrameNormalizationFailed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureDiagnosticDetail {
    SourceAcquisition {
        source: CaptureSourceKind,
        candidate_count: u32,
        selected_node_id: Option<u32>,
    },
    RegistryDiscovery {
        global_count: u32,
        candidate_count: u32,
    },
    SourceLifetime {
        source: CaptureSourceKind,
        selected_node_id: u32,
        failure_origin: CaptureDiagnosticOperation,
    },
    StreamNegotiation {
        format: &'static str,
        requested_framerate_num: u32,
        requested_framerate_denom: u32,
        width: u32,
        height: u32,
        framerate_num: u32,
        framerate_denom: u32,
        maximum_framerate_num: u32,
        maximum_framerate_denom: u32,
        pixel_aspect_num: u32,
        pixel_aspect_denom: u32,
        chroma_site: u32,
        color_range: u32,
        color_matrix: u32,
        transfer_function: u32,
        color_primaries: u32,
    },
    FirstFrame {
        memory_type: &'static str,
        stride: u32,
        byte_count: u32,
    },
    ProfileBindingAdmission,
    FrameNormalization {
        source_sequence: u64,
    },
    SteadyReception {
        received_frames: u64,
        overwritten_frames: u64,
        last_sequence: Option<u64>,
        maximum_gap_ns: u64,
    },
    PerformanceSummary {
        count: u64,
        p50_ns: u64,
        p95_ns: u64,
        p99_ns: u64,
        max_ns: u64,
        dropped: u64,
    },
    VulkanPerformanceSummary {
        count: u64,
        request_to_present_ns: VulkanTimingDistribution,
        producer_submit_ns: VulkanTimingDistribution,
        present_call_ns: VulkanTimingDistribution,
        producer_fence_ns: VulkanTimingDistribution,
        consumer_readback_ns: VulkanTimingDistribution,
        total_ns: VulkanTimingDistribution,
        dropped: u64,
        requests: u64,
        captures: u64,
        busy_drops: u64,
        coalesced_drops: u64,
        consumer_queue_family: u32,
        consumer_queue_flags: u32,
        consumer_global_priority_low: bool,
        consumer_commands_prerecorded: bool,
        consumer_staging_persistently_mapped: bool,
    },
    VulkanFailure {
        category: &'static str,
        producer_status: Option<i32>,
    },
    ReceiverShutdown {
        received_frames: u64,
        overwritten_frames: u64,
    },
    Shutdown {
        source: CaptureSourceKind,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct VulkanTimingDistribution {
    pub p50: u64,
    pub p95: u64,
    pub p99: u64,
    pub max: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CaptureDiagnosticFact {
    pub sequence: u64,
    pub monotonic_start_ms: u64,
    pub monotonic_end_ms: u64,
    pub operation: CaptureDiagnosticOperation,
    pub status: CaptureDiagnosticStatus,
    pub error_type: Option<CaptureErrorType>,
    pub detail: CaptureDiagnosticDetail,
}

/// Receives capture observations inside a diagnostic run owned by the host application.
///
/// Implementations must remain bounded and must not change the capture result when recording
/// fails. The capture library does not configure a global provider, storage, or exporter.
pub trait CaptureDiagnosticSink {
    fn record(&mut self, fact: CaptureDiagnosticFact);
}

impl CaptureDiagnosticSink for () {
    fn record(&mut self, _fact: CaptureDiagnosticFact) {}
}
