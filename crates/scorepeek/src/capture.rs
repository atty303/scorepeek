use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fmt;
use std::rc::Rc;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa::utils::result::AsyncSeq;
use serde::{Deserialize, Serialize};

mod binding;
mod normalizer;
mod receiver;

pub use binding::{
    AuthoredGamescopeProfileBinding, GamescopeProfileBinding,
    GamescopeProfileBindingAuthoringInput, GamescopeProfileBindingError,
    GamescopeSessionProvenance, GamescopeSessionProvenanceInput,
    GamescopeSessionProvenanceMismatch, MeasuredGamescopeProfileBindingAuthoringInput,
    ObservedContractMismatch,
};
pub use normalizer::{
    CanonicalRegion, FractionalLinearGeometry, FractionalRectangle, NormalizedCanonicalFrame,
    RationalCoordinate, UnboundCanonicalFrame, UnboundNormalizationError,
};
pub use receiver::{
    CalibratedGamescopeLease, CalibratedSourceFrameEvidence, GamescopeLeaseAdmissionFailure,
    ObservedFrame, UncalibratedFrame, UncalibratedMemoryType, UncalibratedPipeWireReceiver,
    UncalibratedVideoContract, admit_gamescope_profile, start_uncalibrated_gamescope_receiver,
};

const MAX_REGISTRY_GLOBALS: u32 = 4_096;
const ITERATION_SLICE: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSourceKind {
    GamescopeDefaultRemote,
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
    ReceiverShutdown {
        received_frames: u64,
        overwritten_frames: u64,
    },
    Shutdown {
        source: CaptureSourceKind,
    },
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

/// Application-owned identity for one uninterrupted capture lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureGeneration(u64);

impl CaptureGeneration {
    /// Creates one nonzero capture generation.
    ///
    /// # Errors
    /// Returns `InvalidCaptureGeneration` when the application supplies zero.
    pub const fn new(value: u64) -> Result<Self, InvalidCaptureGeneration> {
        if value == 0 {
            return Err(InvalidCaptureGeneration);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCaptureGeneration;

impl fmt::Display for InvalidCaptureGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("capture generation must be nonzero")
    }
}

impl std::error::Error for InvalidCaptureGeneration {}

impl CaptureDiagnosticSink for () {
    fn record(&mut self, _fact: CaptureDiagnosticFact) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct GamescopeSourceProbe {
    pub node_id: u32,
    pub registry_global_count: u32,
}

/// One bounded observation of Gamescope video sources on the default `PipeWire` remote.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GamescopeSourceSnapshot {
    Absent,
    Unique { node_id: u32 },
    Ambiguous { candidate_count: u32 },
}

/// A default-remote Gamescope source whose lifetime remains bound to the selected node.
///
/// This lease is deliberately uncalibrated: it contains no capture-profile identifier and cannot
/// produce an `ObservedFrame`. A later calibration boundary must bind an opaque profile explicitly;
/// negotiated caps are not profile identity.
pub struct UncalibratedGamescopeSourceLease {
    runtime: Option<DefaultRemoteRuntime>,
    node_id: u32,
    registry_global_count: u32,
    started: Instant,
    next_diagnostic_sequence: u64,
    terminal_recorded: bool,
}

impl fmt::Debug for UncalibratedGamescopeSourceLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UncalibratedGamescopeSourceLease")
            .field("node_id", &self.node_id)
            .field("registry_global_count", &self.registry_global_count)
            .field("is_active", &self.runtime.is_some())
            .finish_non_exhaustive()
    }
}

impl UncalibratedGamescopeSourceLease {
    #[must_use]
    pub const fn node_id(&self) -> u32 {
        self.node_id
    }

    #[must_use]
    pub const fn registry_global_count(&self) -> u32 {
        self.registry_global_count
    }

    /// Advances the default-remote event loop for at most `timeout` and checks that the selected
    /// node still exists.
    ///
    /// # Errors
    /// Returns `SourceLost` after removal of the selected node, or the stable transport error owned
    /// by the provider lifetime operation. A replacement node is never selected implicitly.
    pub fn poll(
        &mut self,
        timeout: Duration,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> Result<(), CaptureError> {
        let Some(runtime) = self.runtime.as_ref() else {
            return Err(CaptureError::without_source(CaptureErrorType::SourceLost));
        };

        if runtime
            .main_loop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(timeout.min(ITERATION_SLICE)))
            < 0
        {
            runtime.terminal_error.set(Some(TerminalError {
                error_type: CaptureErrorType::ReceiverFailed,
                operation: CaptureDiagnosticOperation::SourceLifetime,
            }));
        }

        let terminal = lease_terminal_error(
            &runtime.state.borrow(),
            runtime.terminal_error.get(),
            runtime.selected_removed.get(),
            self.node_id,
        );
        let Some(terminal) = terminal else {
            return Ok(());
        };

        if !self.terminal_recorded {
            sink.record(lifetime_fact(
                self.next_diagnostic_sequence,
                elapsed_ms(self.started),
                self.node_id,
                terminal.operation,
                CaptureDiagnosticStatus::Error,
                Some(terminal.error_type),
            ));
            self.next_diagnostic_sequence = self.next_diagnostic_sequence.saturating_add(1);
            self.terminal_recorded = true;
        }
        Err(CaptureError::without_source(terminal.error_type))
    }

    /// Releases the provider-owned registry, remote, context, and loop in deterministic order.
    pub fn shutdown(mut self, sink: &mut impl CaptureDiagnosticSink) {
        self.runtime.take();
        sink.record(shutdown_fact(
            self.next_diagnostic_sequence,
            elapsed_ms(self.started),
        ));
    }
}

fn lease_terminal_error(
    state: &RegistryState,
    transport_error: Option<TerminalError>,
    selected_removed: bool,
    selected_node_id: u32,
) -> Option<TerminalError> {
    transport_error.or_else(|| {
        (selected_removed || !state.candidate_ids.contains(&selected_node_id)).then_some(
            TerminalError {
                error_type: CaptureErrorType::SourceLost,
                operation: CaptureDiagnosticOperation::SourceLifetime,
            },
        )
    })
}

#[derive(Debug)]
pub struct CaptureError {
    error_type: CaptureErrorType,
    source: Option<pw::Error>,
}

impl CaptureError {
    #[must_use]
    pub const fn error_type(&self) -> CaptureErrorType {
        self.error_type
    }

    const fn without_source(error_type: CaptureErrorType) -> Self {
        Self {
            error_type,
            source: None,
        }
    }

    fn with_source(error_type: CaptureErrorType, source: pw::Error) -> Self {
        Self {
            error_type,
            source: Some(source),
        }
    }
}

impl fmt::Display for CaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.error_type {
            CaptureErrorType::RemoteConnectionFailed => "PipeWire default remote is unavailable",
            CaptureErrorType::RegistryUnavailable => "PipeWire registry is unavailable",
            CaptureErrorType::RegistryTimedOut => "PipeWire registry discovery timed out",
            CaptureErrorType::RegistryLimitExceeded => "PipeWire registry exceeded the probe limit",
            CaptureErrorType::SourceUnavailable => "Gamescope PipeWire source is unavailable",
            CaptureErrorType::SourceAmbiguous => "Gamescope PipeWire source is ambiguous",
            CaptureErrorType::SourceLost => "Gamescope PipeWire source was lost",
            CaptureErrorType::NegotiationTimedOut => "PipeWire stream negotiation timed out",
            CaptureErrorType::FirstFrameTimedOut => "PipeWire first frame timed out",
            CaptureErrorType::UnsupportedFormat => "PipeWire stream format is unsupported",
            CaptureErrorType::UnsupportedMemoryType => "PipeWire stream memory type is unsupported",
            CaptureErrorType::FrameMalformed => "PipeWire frame is malformed",
            CaptureErrorType::StreamLost => "PipeWire stream was lost",
            CaptureErrorType::ReceiverFailed => "PipeWire receiver failed",
            CaptureErrorType::ProfileSessionProvenanceMissing => {
                "Gamescope session provenance is missing"
            }
            CaptureErrorType::ProfileEnvironmentMismatch => {
                "Gamescope profile environment does not match the session"
            }
            CaptureErrorType::ProfileGamescopeVersionMismatch => {
                "Gamescope profile version does not match the session"
            }
            CaptureErrorType::ProfileBackendMismatch => {
                "Gamescope profile backend does not match the session"
            }
            CaptureErrorType::ProfileOutputDimensionsMismatch => {
                "Gamescope profile output dimensions do not match the session"
            }
            CaptureErrorType::ProfileNestedDimensionsMismatch => {
                "Gamescope profile nested dimensions do not match the session"
            }
            CaptureErrorType::ProfileNestedRefreshMismatch => {
                "Gamescope profile nested refresh does not match the session"
            }
            CaptureErrorType::ProfileScalerMismatch => {
                "Gamescope profile scaler does not match the session"
            }
            CaptureErrorType::ProfileFilterMismatch => {
                "Gamescope profile filter does not match the session"
            }
            CaptureErrorType::ProfileVideoContractMismatch => {
                "Gamescope profile video contract does not match the receiver"
            }
            CaptureErrorType::ProfileMemoryTypeMismatch => {
                "Gamescope profile memory type does not match the receiver"
            }
            CaptureErrorType::ProfileStrideMismatch => {
                "Gamescope profile stride does not match the receiver"
            }
            CaptureErrorType::FrameGenerationMismatch => {
                "observed frame belongs to another capture generation"
            }
            CaptureErrorType::FrameLeaseMismatch => {
                "observed frame belongs to another admitted lease"
            }
            CaptureErrorType::FrameProfileMismatch => {
                "observed frame belongs to another capture profile"
            }
            CaptureErrorType::FrameNormalizerMismatch => {
                "observed frame is bound to another normalizer"
            }
            CaptureErrorType::FrameNormalizationFailed => "observed frame normalization failed",
        })
    }
}

impl std::error::Error for CaptureError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RegistrySnapshot {
    global_count: u32,
    candidate_count: u32,
    first_candidate: Option<u32>,
}

#[derive(Debug, Default)]
struct RegistryState {
    snapshot: RegistrySnapshot,
    candidate_ids: BTreeSet<u32>,
}

impl RegistryState {
    fn observe_global(&mut self) -> bool {
        self.snapshot.global_count = self.snapshot.global_count.saturating_add(1);
        self.snapshot.global_count <= MAX_REGISTRY_GLOBALS
    }

    fn add_candidate(&mut self, node_id: u32) {
        self.candidate_ids.insert(node_id);
        self.update_candidates();
    }

    fn remove_global(&mut self, global_id: u32) {
        self.candidate_ids.remove(&global_id);
        self.update_candidates();
    }

    fn update_candidates(&mut self) {
        self.snapshot.candidate_count = u32::try_from(self.candidate_ids.len()).unwrap_or(u32::MAX);
        self.snapshot.first_candidate = self.candidate_ids.first().copied();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TerminalError {
    error_type: CaptureErrorType,
    operation: CaptureDiagnosticOperation,
}

#[derive(Debug)]
struct RegistryFailure {
    error: CaptureError,
    snapshot: RegistrySnapshot,
    operation: CaptureDiagnosticOperation,
}

#[cfg(test)]
trait RegistryBackend {
    fn discover(&self, timeout: Duration) -> Result<RegistrySnapshot, RegistryFailure>;
}

struct DefaultRemoteRuntime {
    // Listeners must be dropped before the proxies that own their hooks.
    _registry_listener: pw::registry::Listener,
    _core_listener: pw::core::Listener,
    _registry: pw::registry::RegistryRc,
    core: pw::core::CoreRc,
    _context: pw::context::ContextRc,
    main_loop: pw::main_loop::MainLoopRc,
    state: Rc<RefCell<RegistryState>>,
    terminal_error: Rc<Cell<Option<TerminalError>>>,
    selected_removed: Rc<Cell<bool>>,
}

/// Discovers exactly one Gamescope video source on the default `PipeWire` remote.
///
/// Only registry object count, exact candidate count, and the selected numeric node ID cross the
/// boundary. Arbitrary node properties and `PipeWire` error messages are not sent to the diagnostic
/// sink. Discovery does not create a source lease, assign a capture profile, negotiate a stream,
/// or establish capture support.
///
/// # Errors
/// Returns a typed error when the default remote or registry cannot be used, the bounded initial
/// registry round trip times out, or the exact Gamescope source selector finds zero or multiple
/// candidates.
pub fn probe_gamescope_source(
    timeout: Duration,
    sink: &mut impl CaptureDiagnosticSink,
) -> Result<GamescopeSourceProbe, CaptureError> {
    let lease = acquire_gamescope_source(timeout, sink)?;
    let probe = GamescopeSourceProbe {
        node_id: lease.node_id(),
        registry_global_count: lease.registry_global_count(),
    };
    lease.shutdown(sink);
    Ok(probe)
}

/// Observes the current candidate set without acquiring a capture stream.
///
/// This operation owns only one bounded default-remote registry round trip. It does not choose a
/// profile or apply reconnect policy; those remain application responsibilities.
///
/// # Errors
/// Returns a typed transport or registry error when the bounded observation cannot complete.
pub fn snapshot_gamescope_sources(
    timeout: Duration,
) -> Result<GamescopeSourceSnapshot, CaptureError> {
    let (_runtime, snapshot) = acquire_default_remote(timeout).map_err(|failure| failure.error)?;
    Ok(match (snapshot.candidate_count, snapshot.first_candidate) {
        (0, None) => GamescopeSourceSnapshot::Absent,
        (1, Some(node_id)) => GamescopeSourceSnapshot::Unique { node_id },
        _ => GamescopeSourceSnapshot::Ambiguous {
            candidate_count: snapshot.candidate_count,
        },
    })
}

/// Acquires exactly one Gamescope video source and retains its default-remote lifetime.
///
/// The returned lease is intentionally uncalibrated and therefore is not a capture profile or a
/// supported source. Callers must poll it while in use and explicitly shut it down to record the
/// provider lifecycle in the host-owned diagnostic run.
///
/// # Errors
/// Returns a typed error under the same fail-closed discovery contract as
/// [`probe_gamescope_source`].
pub fn acquire_gamescope_source(
    timeout: Duration,
    sink: &mut impl CaptureDiagnosticSink,
) -> Result<UncalibratedGamescopeSourceLease, CaptureError> {
    let started = Instant::now();
    let (runtime, snapshot) = match acquire_default_remote(timeout) {
        Ok(value) => value,
        Err(failure) => {
            record_discovery_failure(started, &failure, sink);
            return Err(failure.error);
        }
    };
    sink.record(registry_fact(
        1,
        0,
        elapsed_ms(started),
        snapshot,
        CaptureDiagnosticStatus::Success,
        None,
    ));
    let selected = match select_gamescope_source(snapshot) {
        Ok(node_id) => node_id,
        Err(error) => {
            sink.record(acquisition_fact(
                2,
                0,
                elapsed_ms(started),
                snapshot,
                CaptureDiagnosticStatus::Error,
                Some(error.error_type),
                None,
            ));
            return Err(error);
        }
    };
    sink.record(acquisition_fact(
        2,
        0,
        elapsed_ms(started),
        snapshot,
        CaptureDiagnosticStatus::Success,
        None,
        Some(selected),
    ));
    Ok(UncalibratedGamescopeSourceLease {
        runtime: Some(runtime),
        node_id: selected,
        registry_global_count: snapshot.global_count,
        started,
        next_diagnostic_sequence: 3,
        terminal_recorded: false,
    })
}

fn record_discovery_failure(
    started: Instant,
    failure: &RegistryFailure,
    sink: &mut impl CaptureDiagnosticSink,
) {
    let status = if failure.error.error_type == CaptureErrorType::RegistryTimedOut {
        CaptureDiagnosticStatus::Timeout
    } else {
        CaptureDiagnosticStatus::Error
    };
    let acquisition_sequence = if failure.operation == CaptureDiagnosticOperation::RegistryDiscovery
    {
        sink.record(registry_fact(
            1,
            0,
            elapsed_ms(started),
            failure.snapshot,
            status,
            Some(failure.error.error_type),
        ));
        2
    } else {
        1
    };
    sink.record(acquisition_fact(
        acquisition_sequence,
        0,
        elapsed_ms(started),
        failure.snapshot,
        CaptureDiagnosticStatus::Error,
        Some(failure.error.error_type),
        None,
    ));
}

#[cfg(test)]
fn probe_gamescope_source_with(
    backend: &impl RegistryBackend,
    timeout: Duration,
    sink: &mut impl CaptureDiagnosticSink,
) -> Result<GamescopeSourceProbe, CaptureError> {
    let started = Instant::now();
    let registry_started_ms = elapsed_ms(started);
    let snapshot = match backend.discover(timeout) {
        Ok(snapshot) => {
            sink.record(registry_fact(
                1,
                registry_started_ms,
                elapsed_ms(started),
                snapshot,
                CaptureDiagnosticStatus::Success,
                None,
            ));
            snapshot
        }
        Err(failure) => {
            let status = if failure.error.error_type == CaptureErrorType::RegistryTimedOut {
                CaptureDiagnosticStatus::Timeout
            } else {
                CaptureDiagnosticStatus::Error
            };
            let acquisition_sequence =
                if failure.operation == CaptureDiagnosticOperation::RegistryDiscovery {
                    sink.record(registry_fact(
                        1,
                        registry_started_ms,
                        elapsed_ms(started),
                        failure.snapshot,
                        status,
                        Some(failure.error.error_type),
                    ));
                    2
                } else {
                    1
                };
            sink.record(acquisition_fact(
                acquisition_sequence,
                0,
                elapsed_ms(started),
                failure.snapshot,
                CaptureDiagnosticStatus::Error,
                Some(failure.error.error_type),
                None,
            ));
            return Err(failure.error);
        }
    };

    let selected = match select_gamescope_source(snapshot) {
        Ok(node_id) => node_id,
        Err(error) => {
            sink.record(acquisition_fact(
                2,
                0,
                elapsed_ms(started),
                snapshot,
                CaptureDiagnosticStatus::Error,
                Some(error.error_type),
                None,
            ));
            return Err(error);
        }
    };
    sink.record(acquisition_fact(
        2,
        0,
        elapsed_ms(started),
        snapshot,
        CaptureDiagnosticStatus::Success,
        None,
        Some(selected),
    ));
    Ok(GamescopeSourceProbe {
        node_id: selected,
        registry_global_count: snapshot.global_count,
    })
}

fn select_gamescope_source(snapshot: RegistrySnapshot) -> Result<u32, CaptureError> {
    match (snapshot.candidate_count, snapshot.first_candidate) {
        (1, Some(node_id)) => Ok(node_id),
        (0, None) => Err(CaptureError::without_source(
            CaptureErrorType::SourceUnavailable,
        )),
        _ => Err(CaptureError::without_source(
            CaptureErrorType::SourceAmbiguous,
        )),
    }
}

fn acquire_default_remote(
    timeout: Duration,
) -> Result<(DefaultRemoteRuntime, RegistrySnapshot), RegistryFailure> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(|error| RegistryFailure {
        error: CaptureError::with_source(CaptureErrorType::ReceiverFailed, error),
        snapshot: RegistrySnapshot::default(),
        operation: CaptureDiagnosticOperation::SourceAcquisition,
    })?;
    let context =
        pw::context::ContextRc::new(&main_loop, None).map_err(|error| RegistryFailure {
            error: CaptureError::with_source(CaptureErrorType::ReceiverFailed, error),
            snapshot: RegistrySnapshot::default(),
            operation: CaptureDiagnosticOperation::SourceAcquisition,
        })?;
    let core = context.connect_rc(None).map_err(|error| RegistryFailure {
        error: CaptureError::with_source(CaptureErrorType::RemoteConnectionFailed, error),
        snapshot: RegistrySnapshot::default(),
        operation: CaptureDiagnosticOperation::SourceAcquisition,
    })?;
    let registry = core.get_registry_rc().map_err(|error| RegistryFailure {
        error: CaptureError::with_source(CaptureErrorType::RegistryUnavailable, error),
        snapshot: RegistrySnapshot::default(),
        operation: CaptureDiagnosticOperation::RegistryDiscovery,
    })?;

    let state = Rc::new(RefCell::new(RegistryState::default()));
    let terminal_error = Rc::new(Cell::new(None));
    let selected_node_id = Rc::new(Cell::new(None::<u32>));
    let selected_removed = Rc::new(Cell::new(false));
    let complete = Rc::new(Cell::new(false));
    let pending = Rc::new(Cell::new(None::<AsyncSeq>));

    let registry_listener = register_registry_listener(
        &registry,
        Rc::clone(&state),
        Rc::clone(&terminal_error),
        Rc::clone(&selected_node_id),
        Rc::clone(&selected_removed),
    );

    let callback_complete = Rc::clone(&complete);
    let callback_pending = Rc::clone(&pending);
    let callback_error = Rc::clone(&terminal_error);
    let core_listener = core
        .add_listener_local()
        .done(move |id, sequence| {
            if id == pw::core::PW_ID_CORE && callback_pending.get() == Some(sequence) {
                callback_complete.set(true);
            }
        })
        .error(move |id, _sequence, result, _message| {
            callback_error.set(Some(classify_core_error(id, result)));
        })
        .register();

    let sync = core.sync(0).map_err(|error| RegistryFailure {
        error: CaptureError::with_source(CaptureErrorType::RegistryUnavailable, error),
        snapshot: state.borrow().snapshot,
        operation: CaptureDiagnosticOperation::RegistryDiscovery,
    })?;
    pending.set(Some(sync));

    let snapshot = complete_initial_registry_roundtrip(
        &main_loop,
        &state,
        &terminal_error,
        &complete,
        timeout,
    )?;
    selected_node_id.set(snapshot.first_candidate);
    Ok((
        DefaultRemoteRuntime {
            _registry_listener: registry_listener,
            _core_listener: core_listener,
            _registry: registry,
            core,
            _context: context,
            main_loop,
            state,
            terminal_error,
            selected_removed,
        },
        snapshot,
    ))
}

fn register_registry_listener(
    registry: &pw::registry::RegistryRc,
    state: Rc<RefCell<RegistryState>>,
    terminal_error: Rc<Cell<Option<TerminalError>>>,
    selected_node_id: Rc<Cell<Option<u32>>>,
    selected_removed: Rc<Cell<bool>>,
) -> pw::registry::Listener {
    let global_state = Rc::clone(&state);
    registry
        .add_listener_local()
        .global(move |global| {
            let mut state = global_state.borrow_mut();
            if !state.observe_global() {
                terminal_error.set(Some(TerminalError {
                    error_type: CaptureErrorType::RegistryLimitExceeded,
                    operation: CaptureDiagnosticOperation::RegistryDiscovery,
                }));
                return;
            }
            if global.type_ != pw::types::ObjectType::Node {
                return;
            }
            let Some(properties) = global.props.as_ref() else {
                return;
            };
            if properties.get(*pw::keys::NODE_NAME) == Some("gamescope")
                && properties.get(*pw::keys::MEDIA_CLASS) == Some("Video/Source")
            {
                state.add_candidate(global.id);
            }
        })
        .global_remove({
            move |global_id| {
                if selected_node_id.get() == Some(global_id) {
                    selected_removed.set(true);
                }
                state.borrow_mut().remove_global(global_id);
            }
        })
        .register()
}

fn complete_initial_registry_roundtrip(
    main_loop: &pw::main_loop::MainLoopRc,
    state: &RefCell<RegistryState>,
    terminal_error: &Cell<Option<TerminalError>>,
    complete: &Cell<bool>,
    timeout: Duration,
) -> Result<RegistrySnapshot, RegistryFailure> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| RegistryFailure {
            error: CaptureError::without_source(CaptureErrorType::RegistryTimedOut),
            snapshot: state.borrow().snapshot,
            operation: CaptureDiagnosticOperation::RegistryDiscovery,
        })?;
    while !complete.get() && terminal_error.get().is_none() {
        let now = Instant::now();
        if now >= deadline {
            return Err(RegistryFailure {
                error: CaptureError::without_source(CaptureErrorType::RegistryTimedOut),
                snapshot: state.borrow().snapshot,
                operation: CaptureDiagnosticOperation::RegistryDiscovery,
            });
        }
        let wait = deadline.saturating_duration_since(now).min(ITERATION_SLICE);
        if main_loop.loop_().iterate(pw::loop_::Timeout::Finite(wait)) < 0 {
            terminal_error.set(Some(TerminalError {
                error_type: CaptureErrorType::ReceiverFailed,
                operation: CaptureDiagnosticOperation::RegistryDiscovery,
            }));
        }
    }

    let snapshot = state.borrow().snapshot;
    if let Some(terminal_error) = terminal_error.get() {
        return Err(RegistryFailure {
            error: CaptureError::without_source(terminal_error.error_type),
            snapshot,
            operation: terminal_error.operation,
        });
    }
    Ok(snapshot)
}

fn classify_core_error(id: u32, result: i32) -> TerminalError {
    if id != pw::core::PW_ID_CORE {
        return TerminalError {
            error_type: CaptureErrorType::RegistryUnavailable,
            operation: CaptureDiagnosticOperation::RegistryDiscovery,
        };
    }

    let error_kind = result
        .checked_neg()
        .map(std::io::Error::from_raw_os_error)
        .map(|error| error.kind());
    if matches!(
        error_kind,
        Some(
            std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::TimedOut
        )
    ) {
        TerminalError {
            error_type: CaptureErrorType::RemoteConnectionFailed,
            operation: CaptureDiagnosticOperation::SourceAcquisition,
        }
    } else {
        TerminalError {
            error_type: CaptureErrorType::ReceiverFailed,
            operation: CaptureDiagnosticOperation::RegistryDiscovery,
        }
    }
}

fn registry_fact(
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    snapshot: RegistrySnapshot,
    status: CaptureDiagnosticStatus,
    error_type: Option<CaptureErrorType>,
) -> CaptureDiagnosticFact {
    CaptureDiagnosticFact {
        sequence,
        monotonic_start_ms,
        monotonic_end_ms,
        operation: CaptureDiagnosticOperation::RegistryDiscovery,
        status,
        error_type,
        detail: CaptureDiagnosticDetail::RegistryDiscovery {
            global_count: snapshot.global_count.min(MAX_REGISTRY_GLOBALS),
            candidate_count: snapshot.candidate_count,
        },
    }
}

fn acquisition_fact(
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    snapshot: RegistrySnapshot,
    status: CaptureDiagnosticStatus,
    error_type: Option<CaptureErrorType>,
    selected_node_id: Option<u32>,
) -> CaptureDiagnosticFact {
    CaptureDiagnosticFact {
        sequence,
        monotonic_start_ms,
        monotonic_end_ms,
        operation: CaptureDiagnosticOperation::SourceAcquisition,
        status,
        error_type,
        detail: CaptureDiagnosticDetail::SourceAcquisition {
            source: CaptureSourceKind::GamescopeDefaultRemote,
            candidate_count: snapshot.candidate_count,
            selected_node_id,
        },
    }
}

fn lifetime_fact(
    sequence: u64,
    monotonic_end_ms: u64,
    selected_node_id: u32,
    failure_origin: CaptureDiagnosticOperation,
    status: CaptureDiagnosticStatus,
    error_type: Option<CaptureErrorType>,
) -> CaptureDiagnosticFact {
    CaptureDiagnosticFact {
        sequence,
        monotonic_start_ms: 0,
        monotonic_end_ms,
        operation: CaptureDiagnosticOperation::SourceLifetime,
        status,
        error_type,
        detail: CaptureDiagnosticDetail::SourceLifetime {
            source: CaptureSourceKind::GamescopeDefaultRemote,
            selected_node_id,
            failure_origin,
        },
    }
}

fn shutdown_fact(sequence: u64, monotonic_end_ms: u64) -> CaptureDiagnosticFact {
    CaptureDiagnosticFact {
        sequence,
        monotonic_start_ms: monotonic_end_ms,
        monotonic_end_ms,
        operation: CaptureDiagnosticOperation::Shutdown,
        status: CaptureDiagnosticStatus::Success,
        error_type: None,
        detail: CaptureDiagnosticDetail::Shutdown {
            source: CaptureSourceKind::GamescopeDefaultRemote,
        },
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_generation_is_explicit_and_nonzero() {
        assert_eq!(CaptureGeneration::new(0), Err(InvalidCaptureGeneration));
        assert_eq!(CaptureGeneration::new(9).unwrap().get(), 9);
    }

    struct FakeRegistry(Result<RegistrySnapshot, RegistryFailure>);

    impl RegistryBackend for FakeRegistry {
        fn discover(&self, _timeout: Duration) -> Result<RegistrySnapshot, RegistryFailure> {
            match &self.0 {
                Ok(snapshot) => Ok(*snapshot),
                Err(failure) => Err(RegistryFailure {
                    error: CaptureError::without_source(failure.error.error_type),
                    snapshot: failure.snapshot,
                    operation: failure.operation,
                }),
            }
        }
    }

    #[derive(Default)]
    struct Facts(Vec<CaptureDiagnosticFact>);

    impl CaptureDiagnosticSink for Facts {
        fn record(&mut self, fact: CaptureDiagnosticFact) {
            self.0.push(fact);
        }
    }

    #[test]
    fn exact_gamescope_candidate_is_selected_with_bounded_facts() {
        let backend = FakeRegistry(Ok(RegistrySnapshot {
            global_count: 17,
            candidate_count: 1,
            first_candidate: Some(42),
        }));
        let mut facts = Facts::default();

        let probe = probe_gamescope_source_with(&backend, Duration::from_secs(1), &mut facts)
            .expect("exact source");

        assert_eq!(probe.node_id, 42);
        assert_eq!(probe.registry_global_count, 17);
        assert_eq!(facts.0.len(), 2);
        assert_eq!(
            facts.0[0].operation,
            CaptureDiagnosticOperation::RegistryDiscovery
        );
        assert_eq!(facts.0[0].status, CaptureDiagnosticStatus::Success);
        assert_eq!(
            facts.0[1].detail,
            CaptureDiagnosticDetail::SourceAcquisition {
                source: CaptureSourceKind::GamescopeDefaultRemote,
                candidate_count: 1,
                selected_node_id: Some(42),
            }
        );
    }

    #[test]
    fn zero_or_multiple_candidates_fail_closed() {
        for (candidate_count, first_candidate, expected) in [
            (0, None, CaptureErrorType::SourceUnavailable),
            (2, Some(10), CaptureErrorType::SourceAmbiguous),
        ] {
            let backend = FakeRegistry(Ok(RegistrySnapshot {
                global_count: 20,
                candidate_count,
                first_candidate,
            }));
            let mut facts = Facts::default();

            let error = probe_gamescope_source_with(&backend, Duration::from_secs(1), &mut facts)
                .expect_err("source selection must fail");

            assert_eq!(error.error_type(), expected);
            assert_eq!(facts.0.len(), 2);
            assert_eq!(facts.0[1].status, CaptureDiagnosticStatus::Error);
            assert_eq!(facts.0[1].error_type, Some(expected));
        }
    }

    #[test]
    fn registry_timeout_is_distinct_and_preserves_partial_counts() {
        let backend = FakeRegistry(Err(RegistryFailure {
            error: CaptureError::without_source(CaptureErrorType::RegistryTimedOut),
            snapshot: RegistrySnapshot {
                global_count: 9,
                candidate_count: 1,
                first_candidate: Some(7),
            },
            operation: CaptureDiagnosticOperation::RegistryDiscovery,
        }));
        let mut facts = Facts::default();

        let error = probe_gamescope_source_with(&backend, Duration::from_millis(1), &mut facts)
            .expect_err("timeout");

        assert_eq!(error.error_type(), CaptureErrorType::RegistryTimedOut);
        assert_eq!(facts.0.len(), 2);
        assert_eq!(facts.0[0].status, CaptureDiagnosticStatus::Timeout);
        assert_eq!(
            facts.0[0].error_type,
            Some(CaptureErrorType::RegistryTimedOut)
        );
        assert_eq!(
            facts.0[0].detail,
            CaptureDiagnosticDetail::RegistryDiscovery {
                global_count: 9,
                candidate_count: 1,
            }
        );
        assert_eq!(facts.0[1].status, CaptureDiagnosticStatus::Error);
    }

    #[test]
    fn remote_failure_is_owned_by_source_acquisition() {
        let backend = FakeRegistry(Err(RegistryFailure {
            error: CaptureError::without_source(CaptureErrorType::RemoteConnectionFailed),
            snapshot: RegistrySnapshot::default(),
            operation: CaptureDiagnosticOperation::SourceAcquisition,
        }));
        let mut facts = Facts::default();

        let error = probe_gamescope_source_with(&backend, Duration::from_secs(1), &mut facts)
            .expect_err("remote failure");

        assert_eq!(error.error_type(), CaptureErrorType::RemoteConnectionFailed);
        assert_eq!(facts.0.len(), 1);
        assert_eq!(facts.0[0].sequence, 1);
        assert_eq!(
            facts.0[0].operation,
            CaptureDiagnosticOperation::SourceAcquisition
        );
        assert_eq!(facts.0[0].status, CaptureDiagnosticStatus::Error);
    }

    #[test]
    fn removed_candidate_is_absent_at_round_trip_completion() {
        let mut state = RegistryState::default();

        assert!(state.observe_global());
        state.add_candidate(10);
        state.remove_global(10);

        assert_eq!(state.snapshot.global_count, 1);
        assert_eq!(state.snapshot.candidate_count, 0);
        assert_eq!(state.snapshot.first_candidate, None);
    }

    #[test]
    fn candidate_replacement_retains_only_the_current_node() {
        let mut state = RegistryState::default();

        assert!(state.observe_global());
        state.add_candidate(10);
        state.remove_global(10);
        assert!(state.observe_global());
        state.add_candidate(20);

        assert_eq!(state.snapshot.global_count, 2);
        assert_eq!(state.snapshot.candidate_count, 1);
        assert_eq!(state.snapshot.first_candidate, Some(20));
    }

    #[test]
    fn lease_is_bound_to_the_selected_node_without_replacement() {
        let mut state = RegistryState::default();
        state.add_candidate(10);

        assert_eq!(lease_terminal_error(&state, None, false, 10), None);

        state.remove_global(10);
        state.add_candidate(20);

        assert_eq!(
            lease_terminal_error(&state, None, false, 10),
            Some(TerminalError {
                error_type: CaptureErrorType::SourceLost,
                operation: CaptureDiagnosticOperation::SourceLifetime,
            })
        );
    }

    #[test]
    fn same_numeric_id_replacement_cannot_revive_a_removed_lease() {
        let mut state = RegistryState::default();
        state.add_candidate(10);

        state.remove_global(10);
        state.add_candidate(10);

        assert_eq!(
            lease_terminal_error(&state, None, true, 10),
            Some(TerminalError {
                error_type: CaptureErrorType::SourceLost,
                operation: CaptureDiagnosticOperation::SourceLifetime,
            })
        );
    }

    #[test]
    fn transport_failure_precedes_source_removal_classification() {
        let state = RegistryState::default();
        let transport_error = TerminalError {
            error_type: CaptureErrorType::RemoteConnectionFailed,
            operation: CaptureDiagnosticOperation::SourceAcquisition,
        };

        assert_eq!(
            lease_terminal_error(&state, Some(transport_error), false, 10),
            Some(transport_error)
        );
    }

    #[test]
    fn lifetime_fact_retains_the_terminal_failure_origin() {
        assert_eq!(
            lifetime_fact(
                3,
                12,
                10,
                CaptureDiagnosticOperation::SourceAcquisition,
                CaptureDiagnosticStatus::Error,
                Some(CaptureErrorType::RemoteConnectionFailed),
            ),
            CaptureDiagnosticFact {
                sequence: 3,
                monotonic_start_ms: 0,
                monotonic_end_ms: 12,
                operation: CaptureDiagnosticOperation::SourceLifetime,
                status: CaptureDiagnosticStatus::Error,
                error_type: Some(CaptureErrorType::RemoteConnectionFailed),
                detail: CaptureDiagnosticDetail::SourceLifetime {
                    source: CaptureSourceKind::GamescopeDefaultRemote,
                    selected_node_id: 10,
                    failure_origin: CaptureDiagnosticOperation::SourceAcquisition,
                },
            }
        );
    }

    #[test]
    fn explicit_shutdown_is_a_bounded_typed_fact() {
        assert_eq!(
            shutdown_fact(3, 12),
            CaptureDiagnosticFact {
                sequence: 3,
                monotonic_start_ms: 12,
                monotonic_end_ms: 12,
                operation: CaptureDiagnosticOperation::Shutdown,
                status: CaptureDiagnosticStatus::Success,
                error_type: None,
                detail: CaptureDiagnosticDetail::Shutdown {
                    source: CaptureSourceKind::GamescopeDefaultRemote,
                },
            }
        );
    }

    #[test]
    fn asynchronous_core_errors_keep_stable_type_and_owner() {
        let broken_pipe = (1..=255)
            .find(|code| {
                std::io::Error::from_raw_os_error(*code).kind() == std::io::ErrorKind::BrokenPipe
            })
            .expect("BrokenPipe has an OS error code");

        assert_eq!(
            classify_core_error(pw::core::PW_ID_CORE, -broken_pipe),
            TerminalError {
                error_type: CaptureErrorType::RemoteConnectionFailed,
                operation: CaptureDiagnosticOperation::SourceAcquisition,
            }
        );
        assert_eq!(
            classify_core_error(pw::core::PW_ID_CORE.saturating_add(1), -broken_pipe),
            TerminalError {
                error_type: CaptureErrorType::RegistryUnavailable,
                operation: CaptureDiagnosticOperation::RegistryDiscovery,
            }
        );
        assert_eq!(
            classify_core_error(pw::core::PW_ID_CORE, -1),
            TerminalError {
                error_type: CaptureErrorType::ReceiverFailed,
                operation: CaptureDiagnosticOperation::RegistryDiscovery,
            }
        );
    }
}
