use std::fmt;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::buffer::{ChunkFlags, DataType};
use pw::spa::param::video::{VideoFormat, VideoInfoRaw, VideoInterlaceMode};
use pw::spa::pod::{Pod, Value};
use serde::{Deserialize, Serialize};

use super::{
    CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
    CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureError, CaptureErrorType,
    CaptureGeneration, GamescopeProfileBinding, ITERATION_SLICE, NormalizedCanonicalFrame,
    ObservedContractMismatch, UncalibratedGamescopeSourceLease, elapsed_ms,
};

const MAX_WIDTH: u32 = 7_680;
const MAX_HEIGHT: u32 = 4_320;
const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;
const MAX_BUFFERS_PER_PROCESS: usize = 64;
const REQUESTED_FRAMERATE_NUM: u32 = 10;
const REQUESTED_FRAMERATE_DENOM: u32 = 1;
const WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
static RECEIVER_WORKER_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UncalibratedVideoContract {
    pub width: u32,
    pub height: u32,
    pub framerate_num: u32,
    pub framerate_denom: u32,
    pub maximum_framerate_num: u32,
    pub maximum_framerate_denom: u32,
    pub pixel_aspect_num: u32,
    pub pixel_aspect_denom: u32,
    pub chroma_site: u32,
    pub color_range: u32,
    pub color_matrix: u32,
    pub transfer_function: u32,
    pub color_primaries: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UncalibratedMemoryType {
    MemoryPointer,
    MemoryFileDescriptor,
    DmaBuf,
}

impl UncalibratedMemoryType {
    const fn diagnostic_name(self) -> &'static str {
        match self {
            Self::MemoryPointer => "memory_pointer",
            Self::MemoryFileDescriptor => "memory_file_descriptor",
            Self::DmaBuf => "dma_buf",
        }
    }
}

/// Raw `BGRx` evidence retained only for calibration and receiver diagnostics.
///
/// This type is not an `ObservedFrame`: it has no capture-profile or normalizer binding and must
/// not enter recognition. Its debug representation deliberately omits pixel bytes.
pub struct UncalibratedFrame {
    contract: UncalibratedVideoContract,
    memory_type: UncalibratedMemoryType,
    stride: u32,
    sequence: u64,
    received_monotonic_ns: u64,
    bytes: Vec<u8>,
}

impl fmt::Debug for UncalibratedFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UncalibratedFrame")
            .field("contract", &self.contract)
            .field("memory_type", &self.memory_type)
            .field("stride", &self.stride)
            .field("sequence", &self.sequence)
            .field("received_monotonic_ns", &self.received_monotonic_ns)
            .field("byte_count", &self.bytes.len())
            .finish()
    }
}

impl UncalibratedFrame {
    #[must_use]
    pub const fn contract(&self) -> UncalibratedVideoContract {
        self.contract
    }

    #[must_use]
    pub const fn memory_type(&self) -> UncalibratedMemoryType {
        self.memory_type
    }

    #[must_use]
    pub const fn stride(&self) -> u32 {
        self.stride
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub const fn received_monotonic_ns(&self) -> u64 {
        self.received_monotonic_ns
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[cfg(test)]
    pub(super) fn for_normalizer_test(
        contract: UncalibratedVideoContract,
        stride: u32,
        sequence: u64,
        received_monotonic_ns: u64,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            contract,
            memory_type: UncalibratedMemoryType::MemoryFileDescriptor,
            stride,
            sequence,
            received_monotonic_ns,
            bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ReceiverTerminal {
    error_type: CaptureErrorType,
    operation: CaptureDiagnosticOperation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupSnapshot {
    Pending,
    Ready,
    Terminal(ReceiverTerminal),
}

struct ReceiverState {
    started: Instant,
    contract: Option<UncalibratedVideoContract>,
    memory_type: Option<UncalibratedMemoryType>,
    stride: Option<u32>,
    latest: Option<UncalibratedFrame>,
    next_sequence: u64,
    received_frames: u64,
    overwritten_frames: u64,
    last_received_ns: Option<u64>,
    maximum_gap_ns: u64,
    contract_received_ns: Option<u64>,
    first_received_ns: Option<u64>,
    active_seen: bool,
    shutting_down: bool,
    terminal: Option<ReceiverTerminal>,
}

impl ReceiverState {
    fn new(started: Instant) -> Self {
        Self {
            started,
            contract: None,
            memory_type: None,
            stride: None,
            latest: None,
            next_sequence: 1,
            received_frames: 0,
            overwritten_frames: 0,
            last_received_ns: None,
            maximum_gap_ns: 0,
            contract_received_ns: None,
            first_received_ns: None,
            active_seen: false,
            shutting_down: false,
            terminal: None,
        }
    }

    fn fail(&mut self, error_type: CaptureErrorType, operation: CaptureDiagnosticOperation) {
        if self.terminal.is_none() {
            self.terminal = Some(ReceiverTerminal {
                error_type,
                operation,
            });
        }
    }

    fn reception_operation(&self) -> CaptureDiagnosticOperation {
        if self.contract.is_none() {
            CaptureDiagnosticOperation::StreamNegotiation
        } else if self.received_frames == 0 {
            CaptureDiagnosticOperation::FirstFrame
        } else {
            CaptureDiagnosticOperation::SteadyReception
        }
    }

    fn latch_startup_deadline(
        &mut self,
    ) -> Result<(), (ReceiverTerminal, CaptureDiagnosticStatus)> {
        if let Some(terminal) = self.terminal {
            return Err((terminal, CaptureDiagnosticStatus::Error));
        }
        if self.contract.is_some() && self.received_frames > 0 {
            return Ok(());
        }
        let operation = self.reception_operation();
        let error_type = if operation == CaptureDiagnosticOperation::FirstFrame {
            CaptureErrorType::FirstFrameTimedOut
        } else {
            CaptureErrorType::NegotiationTimedOut
        };
        self.fail(error_type, operation);
        Err((
            ReceiverTerminal {
                error_type,
                operation,
            },
            CaptureDiagnosticStatus::Timeout,
        ))
    }

    fn startup_snapshot(&self) -> StartupSnapshot {
        if let Some(terminal) = self.terminal {
            StartupSnapshot::Terminal(terminal)
        } else if self.contract.is_some() && self.received_frames > 0 {
            StartupSnapshot::Ready
        } else {
            StartupSnapshot::Pending
        }
    }

    fn validated_contract(&self, info: VideoInfoRaw) -> Result<UncalibratedVideoContract, ()> {
        let size = info.size();
        let framerate = info.framerate();
        let maximum_framerate = info.max_framerate();
        let pixel_aspect = info.pixel_aspect_ratio();
        if info.format() != VideoFormat::BGRx
            || size.width == 0
            || size.height == 0
            || size.width > MAX_WIDTH
            || size.height > MAX_HEIGHT
            || framerate.denom == 0
            || !info.flags().is_empty()
            || info.modifier() != 0
            || info.views() > 1
            || info.interlace_mode() != VideoInterlaceMode::Progressive
            || info.multiview_mode() != 0
            || info.multiview_flags() != 0
        {
            return Err(());
        }
        let contract = UncalibratedVideoContract {
            width: size.width,
            height: size.height,
            framerate_num: framerate.num,
            framerate_denom: framerate.denom,
            maximum_framerate_num: maximum_framerate.num,
            maximum_framerate_denom: maximum_framerate.denom,
            pixel_aspect_num: pixel_aspect.num,
            pixel_aspect_denom: pixel_aspect.denom,
            chroma_site: info.chroma_site(),
            color_range: info.color_range(),
            color_matrix: info.color_matrix(),
            transfer_function: info.transfer_function(),
            color_primaries: info.color_primaries(),
        };
        if self.contract.is_some_and(|existing| existing != contract) {
            return Err(());
        }
        Ok(contract)
    }

    fn commit_contract(&mut self, contract: UncalibratedVideoContract) {
        if self.terminal.is_none() && self.contract.is_none() {
            self.contract = Some(contract);
            self.contract_received_ns =
                Some(u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
    }

    #[cfg(test)]
    fn negotiate(&mut self, info: VideoInfoRaw) {
        if self.terminal.is_some() {
            return;
        }
        if let Ok(contract) = self.validated_contract(info) {
            self.commit_contract(contract);
        } else {
            let operation = self.reception_operation();
            self.fail(CaptureErrorType::UnsupportedFormat, operation);
        }
    }

    fn accept_frame(
        &mut self,
        memory_type: UncalibratedMemoryType,
        stride: u32,
        bytes: &[u8],
        received_ns: u64,
    ) {
        if self.terminal.is_some() {
            return;
        }
        let Some(contract) = self.contract else {
            self.fail(
                CaptureErrorType::FrameMalformed,
                CaptureDiagnosticOperation::FirstFrame,
            );
            return;
        };
        let minimum_stride = contract.width.checked_mul(4);
        let required_bytes = usize::try_from(stride)
            .ok()
            .and_then(|stride| stride.checked_mul(contract.height as usize));
        if minimum_stride.is_none_or(|minimum| stride < minimum)
            || required_bytes
                .is_none_or(|required| required > MAX_FRAME_BYTES || bytes.len() < required)
        {
            self.fail(
                CaptureErrorType::FrameMalformed,
                CaptureDiagnosticOperation::FirstFrame,
            );
            return;
        }
        if self
            .memory_type
            .is_some_and(|existing| existing != memory_type)
        {
            self.fail(
                CaptureErrorType::UnsupportedMemoryType,
                CaptureDiagnosticOperation::SteadyReception,
            );
            return;
        }
        if self.stride.is_some_and(|existing| existing != stride) {
            self.fail(
                CaptureErrorType::FrameMalformed,
                CaptureDiagnosticOperation::SteadyReception,
            );
            return;
        }

        let required_bytes = required_bytes.expect("validated frame size");
        let previous = self.latest.take();
        let replaced_latest = previous.is_some();
        let mut owned = previous.map_or_else(Vec::new, |frame| frame.bytes);
        owned.clear();
        if owned.try_reserve_exact(required_bytes).is_err() {
            self.fail(
                CaptureErrorType::ReceiverFailed,
                CaptureDiagnosticOperation::SteadyReception,
            );
            return;
        }
        owned.extend_from_slice(&bytes[..required_bytes]);

        self.memory_type = Some(memory_type);
        self.stride = Some(stride);
        if replaced_latest {
            self.overwritten_frames = self.overwritten_frames.saturating_add(1);
        }
        if let Some(previous) = self.last_received_ns {
            self.maximum_gap_ns = self
                .maximum_gap_ns
                .max(received_ns.saturating_sub(previous));
        }
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.received_frames = self.received_frames.saturating_add(1);
        if self.first_received_ns.is_none() {
            self.first_received_ns = Some(received_ns);
        }
        self.last_received_ns = Some(received_ns);
        self.latest = Some(UncalibratedFrame {
            contract,
            memory_type,
            stride,
            sequence,
            received_monotonic_ns: received_ns,
            bytes: owned,
        });
    }
}

/// Common `PipeWire` receiver bound to one uncalibrated Gamescope provider lease.
///
/// The receiver owns stream negotiation, buffer lifetime, latest-frame replacement, sequencing,
/// and receive timing. Provider lifetime remains owned by the enclosed source lease.
pub struct UncalibratedPipeWireReceiver {
    worker: Option<ReceiverWorker>,
    lease: Option<UncalibratedGamescopeSourceLease>,
    state: Arc<Mutex<ReceiverState>>,
    receiver_started_ms: u64,
    shutdown_started_ms: Option<u64>,
    negotiation_recorded: bool,
    first_frame_recorded: bool,
    terminal_recorded: Option<CaptureDiagnosticOperation>,
}

enum ReceiverCommand {
    Shutdown(mpsc::SyncSender<Result<(), ()>>),
}

struct ReceiverWorker {
    commands: mpsc::SyncSender<ReceiverCommand>,
    join: Option<JoinHandle<()>>,
}

struct ReceiverWorkerSupervisor;

impl ReceiverWorkerSupervisor {
    fn acquire() -> Result<Self, CaptureError> {
        RECEIVER_WORKER_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| CaptureError::without_source(CaptureErrorType::ReceiverFailed))
    }
}

impl Drop for ReceiverWorkerSupervisor {
    fn drop(&mut self) {
        RECEIVER_WORKER_ACTIVE.store(false, Ordering::Release);
    }
}

fn lock_state(state: &Mutex<ReceiverState>) -> MutexGuard<'_, ReceiverState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

trait LockedReceiverState {
    fn borrow(&self) -> MutexGuard<'_, ReceiverState>;
    fn borrow_mut(&self) -> MutexGuard<'_, ReceiverState>;
}

impl LockedReceiverState for Arc<Mutex<ReceiverState>> {
    fn borrow(&self) -> MutexGuard<'_, ReceiverState> {
        lock_state(self)
    }

    fn borrow_mut(&self) -> MutexGuard<'_, ReceiverState> {
        lock_state(self)
    }
}

impl ReceiverWorker {
    fn start(node_id: u32, state: Arc<Mutex<ReceiverState>>) -> Result<Self, CaptureError> {
        let supervisor = ReceiverWorkerSupervisor::acquire()?;
        let (commands, command_rx) = mpsc::sync_channel(1);
        let join = thread::Builder::new()
            .name("scorepeek-pipewire".to_owned())
            .spawn(move || {
                let _supervisor = supervisor;
                if run_receiver_worker(node_id, &state, &command_rx).is_err() {
                    let mut state = state.borrow_mut();
                    let operation = state.reception_operation();
                    state.fail(CaptureErrorType::ReceiverFailed, operation);
                }
            })
            .map_err(|_| CaptureError::without_source(CaptureErrorType::ReceiverFailed))?;
        Ok(Self {
            commands,
            join: Some(join),
        })
    }

    fn shutdown(mut self) -> Result<(), ()> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        if self
            .commands
            .send(ReceiverCommand::Shutdown(response_tx))
            .is_err()
        {
            if let Some(join) = self.join.take() {
                let _ = join.join();
            }
            return Err(());
        }
        let result = response_rx
            .recv_timeout(WORKER_SHUTDOWN_TIMEOUT)
            .map_err(|_| ())?;
        let joined = self.join.take().is_none_or(|join| join.join().is_ok());
        if result.is_ok() && joined {
            Ok(())
        } else {
            Err(())
        }
    }
}

fn run_receiver_worker(
    node_id: u32,
    state: &Arc<Mutex<ReceiverState>>,
    commands: &mpsc::Receiver<ReceiverCommand>,
) -> Result<(), pw::Error> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main_loop, None)?;
    let core = context.connect_rc(None)?;
    let stream = pw::stream::StreamRc::new(
        core,
        "scorepeek-uncalibrated-video-receiver",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;
    let listener = register_stream_listener(&stream, Arc::clone(state))?;
    let values = format_offer().map_err(|_| pw::Error::CreationFailed)?;
    let param = Pod::from_bytes(&values).ok_or(pw::Error::CreationFailed)?;
    let mut params = [param];
    stream.connect(
        spa::utils::Direction::Input,
        Some(node_id),
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::NO_CONVERT
            | pw::stream::StreamFlags::DONT_RECONNECT,
        &mut params,
    )?;

    loop {
        match commands.try_recv() {
            Ok(ReceiverCommand::Shutdown(response)) => {
                let result = stream.disconnect().map_err(|_| ());
                drop(listener);
                drop(stream);
                let _ = response.send(result);
                return Ok(());
            }
            Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if main_loop
            .loop_()
            .iterate(pw::loop_::Timeout::Finite(ITERATION_SLICE))
            < 0
        {
            return Err(pw::Error::CreationFailed);
        }
    }
}

/// A live Gamescope receiver admitted by one explicit session and immutable profile binding.
///
/// Only this admitted lease can attach its generation/profile/normalizer identities to an
/// `ObservedFrame`, and only the same lease can apply the binding-selected normalizer to that frame.
pub struct CalibratedGamescopeLease {
    receiver: UncalibratedPipeWireReceiver,
    capture_profile_sha256: Arc<str>,
    normalizer_artifact_sha256: Arc<str>,
    geometry: super::FractionalLinearGeometry,
    capture_generation: CaptureGeneration,
    frame_domain: Rc<()>,
    normalization_success_recorded: bool,
    normalization_failure_recorded: bool,
}

/// One raw receiver frame carrying the identities granted only by calibrated admission.
pub struct ObservedFrame {
    frame: UncalibratedFrame,
    capture_generation: CaptureGeneration,
    capture_profile_sha256: Arc<str>,
    normalizer_artifact_sha256: Arc<str>,
    frame_domain: Rc<()>,
}

/// Exact raw `BGRx` source bytes paired with a successfully normalized live frame.
///
/// The evidence remains outside recognition. It exists only so a retained diagnostic frame can
/// replay the bound source-to-canonical transform without another game session.
pub struct CalibratedSourceFrameEvidence {
    frame: UncalibratedFrame,
}

impl CalibratedSourceFrameEvidence {
    #[must_use]
    pub const fn contract(&self) -> UncalibratedVideoContract {
        self.frame.contract()
    }

    #[must_use]
    pub const fn memory_type(&self) -> UncalibratedMemoryType {
        self.frame.memory_type()
    }

    #[must_use]
    pub const fn stride(&self) -> u32 {
        self.frame.stride()
    }

    #[must_use]
    pub const fn source_sequence(&self) -> u64 {
        self.frame.sequence()
    }

    #[must_use]
    pub const fn received_monotonic_ns(&self) -> u64 {
        self.frame.received_monotonic_ns()
    }

    #[must_use]
    pub fn into_bytes(self) -> Box<[u8]> {
        self.frame.bytes.into_boxed_slice()
    }
}

impl fmt::Debug for ObservedFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservedFrame")
            .field("capture_generation", &self.capture_generation)
            .field("capture_profile_sha256", &self.capture_profile_sha256)
            .field(
                "normalizer_artifact_sha256",
                &self.normalizer_artifact_sha256,
            )
            .field("source_sequence", &self.frame.sequence())
            .field("received_monotonic_ns", &self.frame.received_monotonic_ns())
            .finish_non_exhaustive()
    }
}

impl ObservedFrame {
    #[must_use]
    pub const fn capture_generation(&self) -> CaptureGeneration {
        self.capture_generation
    }

    #[must_use]
    pub fn capture_profile_sha256(&self) -> &str {
        &self.capture_profile_sha256
    }

    #[must_use]
    pub fn normalizer_artifact_sha256(&self) -> &str {
        &self.normalizer_artifact_sha256
    }

    #[must_use]
    pub const fn source_sequence(&self) -> u64 {
        self.frame.sequence()
    }

    #[must_use]
    pub const fn received_monotonic_ns(&self) -> u64 {
        self.frame.received_monotonic_ns()
    }
}

impl fmt::Debug for CalibratedGamescopeLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CalibratedGamescopeLease")
            .field("capture_profile_sha256", &self.capture_profile_sha256)
            .field(
                "normalizer_artifact_sha256",
                &self.normalizer_artifact_sha256,
            )
            .field("capture_generation", &self.capture_generation)
            .finish_non_exhaustive()
    }
}

impl CalibratedGamescopeLease {
    #[must_use]
    pub fn capture_profile_sha256(&self) -> &str {
        &self.capture_profile_sha256
    }

    #[must_use]
    pub fn normalizer_artifact_sha256(&self) -> &str {
        &self.normalizer_artifact_sha256
    }

    #[must_use]
    pub const fn capture_generation(&self) -> CaptureGeneration {
        self.capture_generation
    }

    /// Takes the newest raw frame and binds it to this admitted capture lifetime.
    #[must_use]
    pub fn take_latest_observed_frame(&mut self) -> Option<ObservedFrame> {
        self.receiver
            .take_latest_frame()
            .map(|frame| ObservedFrame {
                frame,
                capture_generation: self.capture_generation,
                capture_profile_sha256: Arc::clone(&self.capture_profile_sha256),
                normalizer_artifact_sha256: Arc::clone(&self.normalizer_artifact_sha256),
                frame_domain: Rc::clone(&self.frame_domain),
            })
    }

    /// Applies this lease's immutable normalizer to one matching observed frame.
    ///
    /// The first success and first failure are recorded at most once each. Later frames do not
    /// create per-frame diagnostic traffic.
    ///
    /// # Errors
    /// Returns a stable typed error for generation/profile/normalizer mixing or normalization
    /// failure. The observed frame is consumed and never enters recognition directly.
    pub fn normalize_observed_frame(
        &mut self,
        observed: ObservedFrame,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> Result<NormalizedCanonicalFrame, CaptureError> {
        self.normalize_observed_frame_inner(observed, sink)
            .map(|(canonical, _)| canonical)
    }

    /// Applies the immutable normalizer and returns the exact consumed source bytes as diagnostic
    /// evidence alongside the canonical frame.
    ///
    /// # Errors
    /// Returns the same typed errors as [`Self::normalize_observed_frame`].
    pub fn normalize_observed_frame_with_source(
        &mut self,
        observed: ObservedFrame,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> Result<(NormalizedCanonicalFrame, CalibratedSourceFrameEvidence), CaptureError> {
        self.normalize_observed_frame_inner(observed, sink)
            .map(|(canonical, observed)| {
                (
                    canonical,
                    CalibratedSourceFrameEvidence {
                        frame: observed.frame,
                    },
                )
            })
    }

    fn normalize_observed_frame_inner(
        &mut self,
        observed: ObservedFrame,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> Result<(NormalizedCanonicalFrame, ObservedFrame), CaptureError> {
        let started_ms = self.receiver.elapsed_ms();
        let source_sequence = observed.source_sequence();
        let result = if !Rc::ptr_eq(&observed.frame_domain, &self.frame_domain) {
            Err(CaptureErrorType::FrameLeaseMismatch)
        } else if observed.capture_generation != self.capture_generation {
            Err(CaptureErrorType::FrameGenerationMismatch)
        } else if observed.capture_profile_sha256 != self.capture_profile_sha256 {
            Err(CaptureErrorType::FrameProfileMismatch)
        } else if observed.normalizer_artifact_sha256 != self.normalizer_artifact_sha256 {
            Err(CaptureErrorType::FrameNormalizerMismatch)
        } else {
            self.geometry
                .normalize(&observed.frame)
                .map(|frame| {
                    (
                        NormalizedCanonicalFrame::bind(
                            frame,
                            self.capture_generation,
                            Arc::clone(&observed.capture_profile_sha256),
                            Arc::clone(&observed.normalizer_artifact_sha256),
                        ),
                        observed,
                    )
                })
                .map_err(|_| CaptureErrorType::FrameNormalizationFailed)
        };
        let error_type = result.as_ref().err().copied();
        let should_record = if error_type.is_some() {
            !std::mem::replace(&mut self.normalization_failure_recorded, true)
        } else {
            !std::mem::replace(&mut self.normalization_success_recorded, true)
        };
        if should_record {
            let ended_ms = self.receiver.elapsed_ms();
            self.receiver.record_with_bounds(
                sink,
                CaptureDiagnosticOperation::FrameNormalization,
                if error_type.is_some() {
                    CaptureDiagnosticStatus::Error
                } else {
                    CaptureDiagnosticStatus::Success
                },
                error_type,
                CaptureDiagnosticDetail::FrameNormalization { source_sequence },
                (started_ms, ended_ms),
            );
        }
        result.map_err(CaptureError::without_source)
    }

    /// Advances the admitted provider and receiver without exposing an unbound frame.
    ///
    /// # Errors
    /// Returns the first typed provider or receiver lifetime failure.
    pub fn poll(
        &mut self,
        timeout: Duration,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> Result<(), CaptureError> {
        self.receiver.poll(timeout, sink)
    }

    /// Shuts down the receiver before its provider lease.
    ///
    /// # Errors
    /// Returns the receiver shutdown failure after all owned resources are released.
    pub fn shutdown(self, sink: &mut impl CaptureDiagnosticSink) -> Result<(), CaptureError> {
        self.shutdown_with_elapsed(sink).0
    }

    /// Shuts down the receiver and provider and returns the final lease-relative monotonic time.
    pub fn shutdown_with_elapsed(
        self,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> (Result<(), CaptureError>, u64) {
        let started = self.receiver.lease.as_ref().map(|lease| lease.started);
        let result = self.receiver.shutdown(sink);
        let elapsed = started.map_or(0, elapsed_ms);
        (result, elapsed)
    }
}

/// A rejected admission that retains ownership of the live receiver for explicit shutdown.
pub struct GamescopeLeaseAdmissionFailure {
    error_type: CaptureErrorType,
    receiver: UncalibratedPipeWireReceiver,
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

impl fmt::Debug for UncalibratedPipeWireReceiver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.state.borrow();
        formatter
            .debug_struct("UncalibratedPipeWireReceiver")
            .field("node_id", &self.lease.as_ref().map(|lease| lease.node_id))
            .field("contract", &state.contract)
            .field("received_frames", &state.received_frames)
            .field("overwritten_frames", &state.overwritten_frames)
            .finish_non_exhaustive()
    }
}

impl UncalibratedPipeWireReceiver {
    /// Advances provider and stream callbacks for at most one bounded event-loop slice.
    ///
    /// # Errors
    /// Fails closed on provider loss, stream loss, contract drift, unsupported memory, or malformed
    /// buffers. The first terminal cause is retained.
    pub fn poll(
        &mut self,
        timeout: Duration,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> Result<(), CaptureError> {
        let Some(lease) = self.lease.as_mut() else {
            return Err(CaptureError::without_source(CaptureErrorType::StreamLost));
        };
        lease.poll(timeout.min(ITERATION_SLICE), sink)?;
        self.flush_observations(sink);
        let terminal = self.state.borrow().terminal;
        if let Some(terminal) = terminal {
            self.record_terminal(terminal, CaptureDiagnosticStatus::Error, sink);
            return Err(CaptureError::without_source(terminal.error_type));
        }
        Ok(())
    }

    #[must_use]
    pub fn take_latest_frame(&mut self) -> Option<UncalibratedFrame> {
        if self.worker.is_some()
            && let Some(lease) = self.lease.as_mut()
            && let Err(terminal) = lease.observe_lifetime(Duration::ZERO)
        {
            self.state
                .borrow_mut()
                .fail(terminal.error_type, terminal.operation);
            return None;
        }
        self.state.borrow_mut().latest.take()
    }

    /// Disconnects and drops the receiver before releasing its provider lease.
    ///
    /// # Errors
    /// Returns `ReceiverFailed` if the explicit stream disconnect fails. Receiver and provider
    /// resources are still released and their shutdown facts are still attempted.
    pub fn shutdown(mut self, sink: &mut impl CaptureDiagnosticSink) -> Result<(), CaptureError> {
        self.shutdown_started_ms = self.lease.as_ref().map(|lease| elapsed_ms(lease.started));
        let provider_error = self.lease.as_mut().and_then(|lease| {
            lease.runtime.as_ref()?;
            lease
                .poll(Duration::ZERO, sink)
                .err()
                .map(|error| error.error_type())
        });
        if let Some(error_type) = provider_error {
            latch_source_terminal(&self.state, error_type);
        }
        self.state.borrow_mut().shutting_down = true;
        let disconnect_error = self
            .worker
            .take()
            .is_some_and(|worker| worker.shutdown().is_err());
        self.flush_observations(sink);
        let terminal = self.state.borrow().terminal;
        if let Some(terminal) = terminal {
            if terminal.operation == CaptureDiagnosticOperation::SourceLifetime {
                if let Some(lease) = self.lease.as_mut() {
                    let _ = lease.poll(Duration::ZERO, sink);
                }
            } else {
                self.record_terminal(terminal, CaptureDiagnosticStatus::Error, sink);
            }
        }
        let received_frames = self.state.borrow().received_frames;
        if should_record_steady_summary(received_frames, self.terminal_recorded) {
            let summary = self.summary_detail();
            self.record(
                sink,
                CaptureDiagnosticOperation::SteadyReception,
                CaptureDiagnosticStatus::Success,
                None,
                summary,
            );
        }
        let shutdown_status = if disconnect_error {
            CaptureDiagnosticStatus::Error
        } else {
            CaptureDiagnosticStatus::Success
        };
        let shutdown_error = disconnect_error.then_some(CaptureErrorType::ReceiverFailed);
        let state = self.state.borrow();
        let detail = CaptureDiagnosticDetail::ReceiverShutdown {
            received_frames: state.received_frames,
            overwritten_frames: state.overwritten_frames,
        };
        drop(state);
        self.record(
            sink,
            CaptureDiagnosticOperation::ReceiverShutdown,
            shutdown_status,
            shutdown_error,
            detail,
        );
        if let Some(lease) = self.lease.take() {
            lease.shutdown(sink);
        }
        if disconnect_error {
            Err(CaptureError::without_source(
                CaptureErrorType::ReceiverFailed,
            ))
        } else {
            Ok(())
        }
    }

    fn flush_observations(&mut self, sink: &mut impl CaptureDiagnosticSink) {
        let state = self.state.borrow();
        let negotiation = (!self.negotiation_recorded)
            .then_some(state.contract)
            .flatten();
        let first = if self.first_frame_recorded {
            None
        } else {
            state.latest.as_ref().map(|frame| {
                (
                    frame.memory_type,
                    frame.stride,
                    u32::try_from(frame.bytes.len()).unwrap_or(u32::MAX),
                )
            })
        };
        drop(state);
        if let Some(contract) = negotiation {
            self.record(
                sink,
                CaptureDiagnosticOperation::StreamNegotiation,
                CaptureDiagnosticStatus::Success,
                None,
                CaptureDiagnosticDetail::StreamNegotiation {
                    format: "bgrx",
                    requested_framerate_num: REQUESTED_FRAMERATE_NUM,
                    requested_framerate_denom: REQUESTED_FRAMERATE_DENOM,
                    width: contract.width,
                    height: contract.height,
                    framerate_num: contract.framerate_num,
                    framerate_denom: contract.framerate_denom,
                    maximum_framerate_num: contract.maximum_framerate_num,
                    maximum_framerate_denom: contract.maximum_framerate_denom,
                    pixel_aspect_num: contract.pixel_aspect_num,
                    pixel_aspect_denom: contract.pixel_aspect_denom,
                    chroma_site: contract.chroma_site,
                    color_range: contract.color_range,
                    color_matrix: contract.color_matrix,
                    transfer_function: contract.transfer_function,
                    color_primaries: contract.color_primaries,
                },
            );
            self.negotiation_recorded = true;
        }
        if let Some((memory_type, stride, byte_count)) = first {
            self.record(
                sink,
                CaptureDiagnosticOperation::FirstFrame,
                CaptureDiagnosticStatus::Success,
                None,
                CaptureDiagnosticDetail::FirstFrame {
                    memory_type: memory_type.diagnostic_name(),
                    stride,
                    byte_count,
                },
            );
            self.first_frame_recorded = true;
        }
    }

    fn record_terminal(
        &mut self,
        terminal: ReceiverTerminal,
        status: CaptureDiagnosticStatus,
        sink: &mut impl CaptureDiagnosticSink,
    ) {
        if self.terminal_recorded.is_some() {
            return;
        }
        let detail = match terminal.operation {
            CaptureDiagnosticOperation::StreamNegotiation => {
                CaptureDiagnosticDetail::StreamNegotiation {
                    format: "bgrx",
                    requested_framerate_num: REQUESTED_FRAMERATE_NUM,
                    requested_framerate_denom: REQUESTED_FRAMERATE_DENOM,
                    width: 0,
                    height: 0,
                    framerate_num: 0,
                    framerate_denom: 0,
                    maximum_framerate_num: 0,
                    maximum_framerate_denom: 0,
                    pixel_aspect_num: 0,
                    pixel_aspect_denom: 0,
                    chroma_site: 0,
                    color_range: 0,
                    color_matrix: 0,
                    transfer_function: 0,
                    color_primaries: 0,
                }
            }
            CaptureDiagnosticOperation::FirstFrame => CaptureDiagnosticDetail::FirstFrame {
                memory_type: "unknown",
                stride: 0,
                byte_count: 0,
            },
            _ => self.summary_detail(),
        };
        self.record(
            sink,
            terminal.operation,
            status,
            Some(terminal.error_type),
            detail,
        );
        self.terminal_recorded = Some(terminal.operation);
    }

    fn summary_detail(&self) -> CaptureDiagnosticDetail {
        let state = self.state.borrow();
        CaptureDiagnosticDetail::SteadyReception {
            received_frames: state.received_frames,
            overwritten_frames: state.overwritten_frames,
            last_sequence: state
                .next_sequence
                .checked_sub(1)
                .filter(|_| state.received_frames > 0),
            maximum_gap_ns: state.maximum_gap_ns,
        }
    }

    fn record(
        &mut self,
        sink: &mut impl CaptureDiagnosticSink,
        operation: CaptureDiagnosticOperation,
        status: CaptureDiagnosticStatus,
        error_type: Option<CaptureErrorType>,
        detail: CaptureDiagnosticDetail,
    ) {
        let state = self.state.borrow();
        let contract_received_ns = state.contract_received_ns;
        let first_received_ns = state.first_received_ns;
        drop(state);
        let Some(lease) = self.lease.as_mut() else {
            return;
        };
        let now = elapsed_ms(lease.started);
        let (start, end) = receiver_fact_bounds(
            operation,
            self.receiver_started_ms,
            contract_received_ns,
            first_received_ns,
            self.shutdown_started_ms,
            now,
        );
        sink.record(CaptureDiagnosticFact {
            sequence: lease.next_diagnostic_sequence,
            monotonic_start_ms: start,
            monotonic_end_ms: end,
            operation,
            status,
            error_type,
            detail,
        });
        lease.next_diagnostic_sequence = lease.next_diagnostic_sequence.saturating_add(1);
    }

    fn elapsed_ms(&self) -> u64 {
        self.lease
            .as_ref()
            .map_or(0, |lease| elapsed_ms(lease.started))
    }

    fn record_with_bounds(
        &mut self,
        sink: &mut impl CaptureDiagnosticSink,
        operation: CaptureDiagnosticOperation,
        status: CaptureDiagnosticStatus,
        error_type: Option<CaptureErrorType>,
        detail: CaptureDiagnosticDetail,
        monotonic_bounds_ms: (u64, u64),
    ) {
        let Some(lease) = self.lease.as_mut() else {
            return;
        };
        sink.record(CaptureDiagnosticFact {
            sequence: lease.next_diagnostic_sequence,
            monotonic_start_ms: monotonic_bounds_ms.0,
            monotonic_end_ms: monotonic_bounds_ms.1,
            operation,
            status,
            error_type,
            detail,
        });
        lease.next_diagnostic_sequence = lease.next_diagnostic_sequence.saturating_add(1);
    }
}

/// Admits a started receiver only when its explicit session and negotiated contract match.
///
/// Exactly one value-free admission fact is offered to the host sink. Sink absence or capacity does
/// not change the returned result. Rejection retains the receiver for explicit ordered shutdown.
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
    let error_type = classify_profile_admission(&binding, &receiver.state.borrow()).err();
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
        frame_domain: Rc::new(()),
        normalization_success_recorded: false,
        normalization_failure_recorded: false,
    })
}

fn classify_profile_admission(
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

fn should_record_steady_summary(
    received_frames: u64,
    terminal_recorded: Option<CaptureDiagnosticOperation>,
) -> bool {
    received_frames > 0 && terminal_recorded != Some(CaptureDiagnosticOperation::SteadyReception)
}

fn receiver_fact_bounds(
    operation: CaptureDiagnosticOperation,
    receiver_started_ms: u64,
    contract_received_ns: Option<u64>,
    first_received_ns: Option<u64>,
    shutdown_started_ms: Option<u64>,
    now: u64,
) -> (u64, u64) {
    let contract_ms = contract_received_ns.map(|value| value / 1_000_000);
    let first_ms = first_received_ns.map(|value| value / 1_000_000);
    match operation {
        CaptureDiagnosticOperation::StreamNegotiation => {
            (receiver_started_ms, contract_ms.unwrap_or(now))
        }
        CaptureDiagnosticOperation::FirstFrame => (
            contract_ms.unwrap_or(receiver_started_ms),
            first_ms.unwrap_or(now),
        ),
        CaptureDiagnosticOperation::SteadyReception => (first_ms.unwrap_or(now), now),
        CaptureDiagnosticOperation::ReceiverShutdown => (shutdown_started_ms.unwrap_or(now), now),
        _ => (now, now),
    }
}

/// Starts the common receiver and waits for negotiated `BGRx` plus its first bounded frame.
///
/// The selected dimensions and rate are observations, not a profile identifier. Only `BGRx` is
/// offered in this minimal slice; there is no conversion or automatic fallback.
///
/// # Errors
/// Returns a typed timeout or the first provider/stream terminal error. Failure cleanup still
/// disconnects the stream before releasing the provider lease.
pub fn start_uncalibrated_gamescope_receiver(
    lease: UncalibratedGamescopeSourceLease,
    timeout: Duration,
    sink: &mut impl CaptureDiagnosticSink,
) -> Result<UncalibratedPipeWireReceiver, CaptureError> {
    if lease.runtime.is_none() {
        return Err(CaptureError::without_source(CaptureErrorType::SourceLost));
    }
    let receiver_started_ms = elapsed_ms(lease.started);
    let state = Arc::new(Mutex::new(ReceiverState::new(lease.started)));
    let worker = match ReceiverWorker::start(lease.node_id, Arc::clone(&state)) {
        Ok(worker) => worker,
        Err(error) => return Err(fail_before_receiver(lease, error, sink)),
    };
    let receiver = UncalibratedPipeWireReceiver {
        worker: Some(worker),
        lease: Some(lease),
        state,
        receiver_started_ms,
        shutdown_started_ms: None,
        negotiation_recorded: false,
        first_frame_recorded: false,
        terminal_recorded: None,
    };
    wait_for_first_frame(receiver, timeout, sink)
}

fn wait_for_first_frame(
    mut receiver: UncalibratedPipeWireReceiver,
    timeout: Duration,
    sink: &mut impl CaptureDiagnosticSink,
) -> Result<UncalibratedPipeWireReceiver, CaptureError> {
    let startup_started = Instant::now();
    loop {
        if let Err(error) = receiver.poll(ITERATION_SLICE, sink) {
            let _ = receiver.shutdown(sink);
            return Err(error);
        }
        let startup = receiver.state.borrow().startup_snapshot();
        match startup {
            StartupSnapshot::Terminal(terminal) => {
                receiver.record_terminal(terminal, CaptureDiagnosticStatus::Error, sink);
                let _ = receiver.shutdown(sink);
                return Err(CaptureError::without_source(terminal.error_type));
            }
            StartupSnapshot::Ready => {
                receiver.flush_observations(sink);
                return Ok(receiver);
            }
            StartupSnapshot::Pending => {}
        }
        if startup_started.elapsed() >= timeout {
            let deadline = {
                let mut state = receiver.state.borrow_mut();
                state.latch_startup_deadline()
            };
            let Err((terminal, status)) = deadline else {
                receiver.flush_observations(sink);
                return Ok(receiver);
            };
            receiver.record_terminal(terminal, status, sink);
            let _ = receiver.shutdown(sink);
            return Err(CaptureError::without_source(terminal.error_type));
        }
    }
}

fn fail_before_receiver(
    mut lease: UncalibratedGamescopeSourceLease,
    error: CaptureError,
    sink: &mut impl CaptureDiagnosticSink,
) -> CaptureError {
    let end = elapsed_ms(lease.started);
    sink.record(CaptureDiagnosticFact {
        sequence: lease.next_diagnostic_sequence,
        monotonic_start_ms: end,
        monotonic_end_ms: end,
        operation: CaptureDiagnosticOperation::StreamNegotiation,
        status: CaptureDiagnosticStatus::Error,
        error_type: Some(error.error_type()),
        detail: CaptureDiagnosticDetail::StreamNegotiation {
            format: "bgrx",
            requested_framerate_num: REQUESTED_FRAMERATE_NUM,
            requested_framerate_denom: REQUESTED_FRAMERATE_DENOM,
            width: 0,
            height: 0,
            framerate_num: 0,
            framerate_denom: 0,
            maximum_framerate_num: 0,
            maximum_framerate_denom: 0,
            pixel_aspect_num: 0,
            pixel_aspect_denom: 0,
            chroma_site: 0,
            color_range: 0,
            color_matrix: 0,
            transfer_function: 0,
            color_primaries: 0,
        },
    });
    lease.next_diagnostic_sequence = lease.next_diagnostic_sequence.saturating_add(1);
    lease.shutdown(sink);
    error
}

fn register_stream_listener(
    stream: &pw::stream::StreamRc,
    state: Arc<Mutex<ReceiverState>>,
) -> Result<pw::stream::StreamListener<Arc<Mutex<ReceiverState>>>, pw::Error> {
    stream
        .add_local_listener_with_user_data(state)
        .state_changed(|_, state, _, new| handle_stream_state(state, &new))
        .param_changed(|stream, state, id, param| handle_format(Some(stream), state, id, param))
        .process(handle_process)
        .register()
}

fn handle_process(stream: &pw::stream::Stream, state: &mut Arc<Mutex<ReceiverState>>) {
    let mut newest = None;
    for _ in 0..MAX_BUFFERS_PER_PROCESS {
        let Some(buffer) = stream.dequeue_buffer() else {
            break;
        };
        newest = Some(buffer);
    }
    let Some(mut buffer) = newest else { return };
    let received_ns =
        u64::try_from(state.borrow().started.elapsed().as_nanos()).unwrap_or(u64::MAX);
    let datas = buffer.datas_mut();
    if datas.len() != 1 {
        fail_frame(state, CaptureErrorType::FrameMalformed);
        return;
    }
    let data = &mut datas[0];
    let memory_type = match data.type_() {
        DataType::MemPtr => UncalibratedMemoryType::MemoryPointer,
        DataType::MemFd => UncalibratedMemoryType::MemoryFileDescriptor,
        DataType::DmaBuf => UncalibratedMemoryType::DmaBuf,
        _ => {
            fail_frame(state, CaptureErrorType::UnsupportedMemoryType);
            return;
        }
    };
    if data.as_raw().chunk.is_null() {
        fail_frame(state, CaptureErrorType::FrameMalformed);
        return;
    }
    let chunk = data.chunk();
    if chunk.flags().contains(ChunkFlags::CORRUPTED) || chunk.stride() <= 0 {
        fail_frame(state, CaptureErrorType::FrameMalformed);
        return;
    }
    let offset = chunk.offset() as usize;
    let size = chunk.size() as usize;
    let stride = chunk.stride().cast_unsigned();
    let Some(mapped) = data.data() else {
        fail_frame(state, CaptureErrorType::UnsupportedMemoryType);
        return;
    };
    let Some(end) = offset.checked_add(size) else {
        fail_frame(state, CaptureErrorType::FrameMalformed);
        return;
    };
    let Some(bytes) = mapped.get(offset..end) else {
        fail_frame(state, CaptureErrorType::FrameMalformed);
        return;
    };
    state
        .borrow_mut()
        .accept_frame(memory_type, stride, bytes, received_ns);
}

fn handle_stream_state(state: &Arc<Mutex<ReceiverState>>, new: &pw::stream::StreamState) {
    let mut state = state.borrow_mut();
    match new {
        pw::stream::StreamState::Connecting
        | pw::stream::StreamState::Paused
        | pw::stream::StreamState::Streaming => state.active_seen = true,
        pw::stream::StreamState::Error(_) => {
            let operation = state.reception_operation();
            state.fail(CaptureErrorType::ReceiverFailed, operation);
        }
        pw::stream::StreamState::Unconnected if state.active_seen && !state.shutting_down => {
            let operation = state.reception_operation();
            state.fail(CaptureErrorType::StreamLost, operation);
        }
        pw::stream::StreamState::Unconnected => {}
    }
}

fn handle_format(
    stream: Option<&pw::stream::Stream>,
    state: &Arc<Mutex<ReceiverState>>,
    id: u32,
    param: Option<&Pod>,
) {
    if id != spa::param::ParamType::Format.as_raw() {
        return;
    }
    let Some(param) = param else { return };
    let Ok((media_type, media_subtype)) = spa::param::format_utils::parse_format(param) else {
        fail_format(state);
        return;
    };
    if media_type != spa::param::format::MediaType::Video
        || media_subtype != spa::param::format::MediaSubtype::Raw
    {
        fail_format(state);
        return;
    }
    let mut info = VideoInfoRaw::new();
    if info.parse(param).is_err() {
        fail_format(state);
        return;
    }
    negotiate_format(state, info, |width, height| {
        let values = buffer_offer(width, height).ok_or(())?;
        let param = Pod::from_bytes(&values).ok_or(())?;
        stream
            .ok_or(())?
            .update_params(&mut [param])
            .map_err(|_| ())
    });
}

fn negotiate_format(
    state: &Arc<Mutex<ReceiverState>>,
    info: VideoInfoRaw,
    update_buffers: impl FnOnce(u32, u32) -> Result<(), ()>,
) {
    let contract = {
        let mut state = state.borrow_mut();
        if state.terminal.is_some() {
            return;
        }
        if let Ok(contract) = state.validated_contract(info) {
            contract
        } else {
            let operation = state.reception_operation();
            state.fail(CaptureErrorType::UnsupportedFormat, operation);
            return;
        }
    };
    if update_buffers(contract.width, contract.height).is_err() {
        fail_receiver(state);
        return;
    }
    state.borrow_mut().commit_contract(contract);
}

fn fail_format(state: &Arc<Mutex<ReceiverState>>) {
    let mut state = state.borrow_mut();
    let operation = state.reception_operation();
    state.fail(CaptureErrorType::UnsupportedFormat, operation);
}

fn fail_frame(state: &Arc<Mutex<ReceiverState>>, error_type: CaptureErrorType) {
    let mut state = state.borrow_mut();
    let operation = state.reception_operation();
    state.fail(error_type, operation);
}

fn fail_receiver(state: &Arc<Mutex<ReceiverState>>) {
    let mut state = state.borrow_mut();
    let operation = state.reception_operation();
    state.fail(CaptureErrorType::ReceiverFailed, operation);
}

fn latch_source_terminal(state: &Arc<Mutex<ReceiverState>>, error_type: CaptureErrorType) {
    state
        .borrow_mut()
        .fail(error_type, CaptureDiagnosticOperation::SourceLifetime);
}

fn buffer_offer(width: u32, height: u32) -> Option<Vec<u8>> {
    let stride = width.checked_mul(4)?;
    let size = stride.checked_mul(height)?;
    let stride = i32::try_from(stride).ok()?;
    let size = i32::try_from(size).ok()?;
    let memfd = 1_i32.checked_shl(DataType::MemFd.as_raw())?;
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_BUFFERS_buffers,
                Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
                    spa::utils::ChoiceFlags::empty(),
                    spa::utils::ChoiceEnum::Range {
                        default: 4,
                        min: 1,
                        max: 8,
                    },
                ))),
            ),
            spa::pod::Property::new(spa::sys::SPA_PARAM_BUFFERS_blocks, Value::Int(1)),
            spa::pod::Property::new(spa::sys::SPA_PARAM_BUFFERS_size, Value::Int(size)),
            spa::pod::Property::new(spa::sys::SPA_PARAM_BUFFERS_stride, Value::Int(stride)),
            spa::pod::Property::new(
                spa::sys::SPA_PARAM_BUFFERS_dataType,
                Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
                    spa::utils::ChoiceFlags::empty(),
                    spa::utils::ChoiceEnum::Flags {
                        default: memfd,
                        flags: vec![memfd],
                    },
                ))),
            ),
        ],
    };
    spa::pod::serialize::PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(object))
        .ok()
        .map(|serialized| serialized.0.into_inner())
}

fn format_offer() -> Result<Vec<u8>, spa::pod::serialize::GenError> {
    let object = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Id,
            VideoFormat::BGRx
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle {
                width: 1_920,
                height: 1_080
            },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: MAX_WIDTH,
                height: MAX_HEIGHT
            }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction {
                num: REQUESTED_FRAMERATE_NUM,
                denom: REQUESTED_FRAMERATE_DENOM
            },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 240, denom: 1 }
        ),
    );
    Ok(spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &Value::Object(object),
    )?
    .0
    .into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{
        FractionalRectangle, GamescopeProfileBindingAuthoringInput, RationalCoordinate,
    };

    fn video_info() -> VideoInfoRaw {
        let mut info = VideoInfoRaw::new();
        info.set_format(VideoFormat::BGRx);
        info.set_size(spa::utils::Rectangle {
            width: 4,
            height: 2,
        });
        info.set_framerate(spa::utils::Fraction { num: 60, denom: 1 });
        info
    }

    fn negotiated_state() -> ReceiverState {
        let mut state = ReceiverState::new(Instant::now());
        let info = video_info();
        state.negotiate(info);
        state
    }

    fn profile_binding() -> GamescopeProfileBinding {
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
            observed_video_contract: negotiated_state().contract.expect("contract"),
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
        GamescopeProfileBinding::parse(&authored.bytes, &authored.artifact_sha256).unwrap()
    }

    fn receiver_for_admission() -> UncalibratedPipeWireReceiver {
        let mut state = negotiated_state();
        state.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[0; 32], 1);
        UncalibratedPipeWireReceiver {
            worker: None,
            lease: Some(UncalibratedGamescopeSourceLease {
                runtime: None,
                node_id: 7,
                registry_global_count: 1,
                started: Instant::now(),
                next_diagnostic_sequence: 3,
                terminal_recorded: false,
            }),
            state: Arc::new(Mutex::new(state)),
            receiver_started_ms: 0,
            shutdown_started_ms: None,
            negotiation_recorded: false,
            first_frame_recorded: false,
            terminal_recorded: None,
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
    fn unspecified_producer_framerate_is_preserved() {
        let mut state = ReceiverState::new(Instant::now());
        let mut info = video_info();
        info.set_framerate(spa::utils::Fraction { num: 0, denom: 1 });

        state.negotiate(info);

        assert_eq!(state.terminal, None);
        assert_eq!(state.contract.expect("contract").framerate_num, 0);
    }

    #[test]
    fn stream_offer_prefers_the_sampling_rate_and_accepts_unspecified_producers() {
        let values = format_offer().unwrap();
        let (_, value) = spa::pod::deserialize::PodDeserializer::deserialize_any_from(&values)
            .expect("format offer");
        let Value::Object(object) = value else {
            panic!("expected object");
        };
        let framerate = object
            .properties
            .into_iter()
            .find(|property| {
                property.key == spa::param::format::FormatProperties::VideoFramerate.as_raw()
            })
            .expect("framerate");
        let Value::Choice(spa::pod::ChoiceValue::Fraction(spa::utils::Choice(
            _,
            spa::utils::ChoiceEnum::Range { default, min, max },
        ))) = framerate.value
        else {
            panic!("expected framerate range");
        };
        assert_eq!(
            default,
            spa::utils::Fraction {
                num: REQUESTED_FRAMERATE_NUM,
                denom: REQUESTED_FRAMERATE_DENOM,
            }
        );
        assert_eq!(min, spa::utils::Fraction { num: 0, denom: 1 });
        assert_eq!(max, spa::utils::Fraction { num: 240, denom: 1 });
    }

    #[test]
    fn buffer_offer_matches_the_mapped_bgrx_contract() {
        let values = buffer_offer(3_840, 2_160).expect("buffer offer");
        let (_, value) = spa::pod::deserialize::PodDeserializer::deserialize_any_from(&values)
            .expect("buffer offer pod");
        let Value::Object(object) = value else {
            panic!("expected object");
        };
        assert_eq!(
            object.type_,
            spa::utils::SpaTypes::ObjectParamBuffers.as_raw()
        );
        assert_eq!(object.id, spa::param::ParamType::Buffers.as_raw());
        let property = |key| {
            object
                .properties
                .iter()
                .find(|property| property.key == key)
                .expect("buffer property")
        };
        assert_eq!(
            property(spa::sys::SPA_PARAM_BUFFERS_size).value,
            Value::Int(3_840 * 2_160 * 4)
        );
        assert_eq!(
            property(spa::sys::SPA_PARAM_BUFFERS_stride).value,
            Value::Int(3_840 * 4)
        );
        let Value::Choice(spa::pod::ChoiceValue::Int(spa::utils::Choice(
            _,
            spa::utils::ChoiceEnum::Range { default, min, max },
        ))) = &property(spa::sys::SPA_PARAM_BUFFERS_buffers).value
        else {
            panic!("expected buffer-count range");
        };
        assert_eq!((*default, *min, *max), (4, 1, 8));
    }

    #[test]
    fn startup_deadline_latches_one_terminal_cause_before_late_frames() {
        let mut state = ReceiverState::new(Instant::now());
        state.negotiate(video_info());

        let (terminal, status) = state.latch_startup_deadline().unwrap_err();
        assert_eq!(terminal.error_type, CaptureErrorType::FirstFrameTimedOut);
        assert_eq!(status, CaptureDiagnosticStatus::Timeout);
        state.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[0; 32], 1);
        assert_eq!(state.received_frames, 0);
        assert_eq!(state.terminal, Some(terminal));
    }

    #[test]
    fn startup_terminal_wins_over_an_already_received_frame() {
        let mut state = negotiated_state();
        state.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[0; 32], 1);
        state.fail(
            CaptureErrorType::StreamLost,
            CaptureDiagnosticOperation::SteadyReception,
        );

        assert_eq!(
            state.startup_snapshot(),
            StartupSnapshot::Terminal(ReceiverTerminal {
                error_type: CaptureErrorType::StreamLost,
                operation: CaptureDiagnosticOperation::SteadyReception,
            })
        );
        assert_eq!(
            state.latch_startup_deadline(),
            Err((
                ReceiverTerminal {
                    error_type: CaptureErrorType::StreamLost,
                    operation: CaptureDiagnosticOperation::SteadyReception,
                },
                CaptureDiagnosticStatus::Error,
            ))
        );
    }

    #[test]
    fn buffer_parameter_failure_does_not_commit_negotiation() {
        let state = Arc::new(Mutex::new(ReceiverState::new(Instant::now())));

        negotiate_format(&state, video_info(), |_, _| Err(()));

        let state = state.borrow();
        assert_eq!(state.contract, None);
        assert_eq!(state.contract_received_ns, None);
        assert_eq!(
            state.terminal,
            Some(ReceiverTerminal {
                error_type: CaptureErrorType::ReceiverFailed,
                operation: CaptureDiagnosticOperation::StreamNegotiation,
            })
        );
    }

    #[test]
    fn startup_success_facts_are_flushed_before_the_frame_can_be_taken() {
        let mut receiver = receiver_for_admission();
        let mut facts = Facts::default();

        assert_eq!(
            receiver.state.borrow().startup_snapshot(),
            StartupSnapshot::Ready
        );
        receiver.flush_observations(&mut facts);
        let frame = receiver.take_latest_frame().expect("latest frame");

        assert_eq!(frame.sequence(), 1);
        assert!(facts.0.iter().any(|fact| {
            fact.operation == CaptureDiagnosticOperation::FirstFrame
                && fact.status == CaptureDiagnosticStatus::Success
        }));
    }

    #[test]
    fn receiver_supervisor_rejects_overlap_until_the_worker_exits() {
        let first = ReceiverWorkerSupervisor::acquire().unwrap();
        assert!(ReceiverWorkerSupervisor::acquire().is_err());
        drop(first);
        assert!(ReceiverWorkerSupervisor::acquire().is_ok());
    }

    #[test]
    fn profile_admission_requires_dimensions_and_current_byte_layout_only() {
        let binding = profile_binding();
        let mut state = negotiated_state();
        state.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[0; 32], 1);

        assert_eq!(classify_profile_admission(&binding, &state), Ok(()));

        let mut absent = ReceiverState::new(Instant::now());
        assert_eq!(
            classify_profile_admission(&binding, &absent),
            Err(CaptureErrorType::ProfileVideoContractMismatch)
        );
        absent.contract = negotiated_state().contract;
        assert_eq!(
            classify_profile_admission(&binding, &absent),
            Err(CaptureErrorType::ProfileMemoryTypeMismatch)
        );
        absent.memory_type = Some(UncalibratedMemoryType::MemoryPointer);
        assert_eq!(
            classify_profile_admission(&binding, &absent),
            Err(CaptureErrorType::ProfileStrideMismatch)
        );

        let mut video_mismatch = state.contract.expect("contract");
        video_mismatch.color_primaries = 1;
        state.contract = Some(video_mismatch);
        assert_eq!(classify_profile_admission(&binding, &state), Ok(()));
        state.contract = Some(negotiated_state().contract.expect("contract"));
        state.memory_type = Some(UncalibratedMemoryType::DmaBuf);
        assert_eq!(classify_profile_admission(&binding, &state), Ok(()));
        state.memory_type = Some(UncalibratedMemoryType::MemoryPointer);
        state.stride = Some(20);
        assert_eq!(classify_profile_admission(&binding, &state), Ok(()));
        let mut dimension_mismatch = state.contract.expect("contract");
        dimension_mismatch.width += 1;
        state.contract = Some(dimension_mismatch);
        assert_eq!(
            classify_profile_admission(&binding, &state),
            Err(CaptureErrorType::ProfileVideoContractMismatch)
        );
    }

    #[test]
    fn admission_records_one_value_free_result_and_retains_rejected_receiver() {
        let binding = profile_binding();
        let receiver = receiver_for_admission();
        let mut facts = Facts::default();
        let admitted = admit_gamescope_profile(
            receiver,
            binding,
            CaptureGeneration::new(1).unwrap(),
            &mut facts,
        )
        .unwrap();
        assert_eq!(facts.0.len(), 3);
        let acceptance = facts.0.last().expect("acceptance fact");
        assert_eq!(acceptance.sequence, 5);
        assert_eq!(
            acceptance.operation,
            CaptureDiagnosticOperation::ProfileBindingAdmission
        );
        assert_eq!(acceptance.status, CaptureDiagnosticStatus::Success);
        assert_eq!(acceptance.error_type, None);
        assert_eq!(
            acceptance.detail,
            CaptureDiagnosticDetail::ProfileBindingAdmission
        );
        admitted.shutdown(&mut ()).unwrap();

        let receiver = receiver_for_admission();
        receiver.state.borrow_mut().contract.as_mut().unwrap().width += 1;
        let failure = admit_gamescope_profile(
            receiver,
            profile_binding(),
            CaptureGeneration::new(1).unwrap(),
            &mut facts,
        )
        .unwrap_err();
        assert_eq!(
            failure.error_type(),
            CaptureErrorType::ProfileVideoContractMismatch
        );
        let rejection = facts.0.last().expect("rejection fact");
        assert_eq!(
            rejection.operation,
            CaptureDiagnosticOperation::ProfileBindingAdmission
        );
        assert_eq!(rejection.status, CaptureDiagnosticStatus::Error);
        assert_eq!(
            rejection.error_type,
            Some(CaptureErrorType::ProfileVideoContractMismatch)
        );
        assert_eq!(
            rejection.detail,
            CaptureDiagnosticDetail::ProfileBindingAdmission
        );
        failure.shutdown(&mut ()).unwrap();
    }

    #[test]
    fn admitted_lease_alone_binds_and_normalizes_observed_frames() {
        let binding = profile_binding();
        let expected_profile = binding.capture_profile_sha256().to_owned();
        let expected_normalizer = binding.normalizer_artifact_sha256().to_owned();
        let receiver = receiver_for_admission();
        let mut facts = Facts::default();
        let mut admitted = admit_gamescope_profile(
            receiver,
            binding,
            CaptureGeneration::new(7).unwrap(),
            &mut facts,
        )
        .unwrap();

        let observed = admitted.take_latest_observed_frame().unwrap();
        assert_eq!(observed.capture_generation().get(), 7);
        assert_eq!(observed.capture_profile_sha256(), expected_profile);
        assert_eq!(observed.normalizer_artifact_sha256(), expected_normalizer);
        let canonical = admitted
            .normalize_observed_frame(observed, &mut facts)
            .unwrap();
        assert_eq!(canonical.capture_generation().get(), 7);
        assert_eq!(canonical.capture_profile_sha256(), expected_profile);
        assert_eq!(canonical.normalizer_artifact_sha256(), expected_normalizer);
        assert_eq!(canonical.source_sequence(), 1);
        assert_eq!(canonical.pixels().len(), 1_920 * 1_080 * 3);
        assert!(canonical.pixels().iter().all(|byte| *byte == 0));
        let fact = facts.0.last().unwrap();
        assert_eq!(
            fact.operation,
            CaptureDiagnosticOperation::FrameNormalization
        );
        assert_eq!(fact.status, CaptureDiagnosticStatus::Success);
        assert_eq!(fact.error_type, None);
        assert_eq!(
            fact.detail,
            CaptureDiagnosticDetail::FrameNormalization { source_sequence: 1 }
        );

        admitted.receiver.state.borrow_mut().accept_frame(
            UncalibratedMemoryType::MemoryPointer,
            16,
            &[0; 32],
            2,
        );
        let observed = admitted.take_latest_observed_frame().unwrap();
        let fact_count = facts.0.len();
        let (canonical, source) = admitted
            .normalize_observed_frame_with_source(observed, &mut facts)
            .unwrap();
        assert_eq!(canonical.source_sequence(), 2);
        assert_eq!(source.source_sequence(), 2);
        assert_eq!(source.contract().width, 4);
        assert_eq!(source.stride(), 16);
        assert_eq!(&*source.into_bytes(), &[0; 32]);
        assert_eq!(facts.0.len(), fact_count);
        admitted.shutdown(&mut ()).unwrap();
    }

    #[test]
    fn generation_profile_and_normalizer_mixing_fail_closed() {
        let cases = [
            CaptureErrorType::FrameLeaseMismatch,
            CaptureErrorType::FrameGenerationMismatch,
            CaptureErrorType::FrameProfileMismatch,
            CaptureErrorType::FrameNormalizerMismatch,
        ];
        for expected in cases {
            let binding = profile_binding();
            let receiver = receiver_for_admission();
            let mut source = Facts::default();
            let mut admitted = admit_gamescope_profile(
                receiver,
                binding,
                CaptureGeneration::new(1).unwrap(),
                &mut source,
            )
            .unwrap();
            let mut observed = admitted.take_latest_observed_frame().unwrap();
            match expected {
                CaptureErrorType::FrameLeaseMismatch => {
                    observed.frame_domain = Rc::new(());
                }
                CaptureErrorType::FrameGenerationMismatch => {
                    observed.capture_generation = CaptureGeneration::new(2).unwrap();
                }
                CaptureErrorType::FrameProfileMismatch => {
                    observed.capture_profile_sha256 = Arc::from("f".repeat(64));
                }
                CaptureErrorType::FrameNormalizerMismatch => {
                    observed.normalizer_artifact_sha256 = Arc::from("e".repeat(64));
                }
                _ => unreachable!(),
            }
            let error = admitted
                .normalize_observed_frame(observed, &mut source)
                .unwrap_err();
            assert_eq!(error.error_type(), expected);
            let fact = source.0.last().unwrap();
            assert_eq!(
                fact.operation,
                CaptureDiagnosticOperation::FrameNormalization
            );
            assert_eq!(fact.status, CaptureDiagnosticStatus::Error);
            assert_eq!(fact.error_type, Some(expected));
            admitted.shutdown(&mut ()).unwrap();
        }
    }

    #[test]
    fn latest_frame_is_bounded_and_receiver_sequence_is_monotonic() {
        let mut state = negotiated_state();
        state.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[1; 32], 10);
        state.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[2; 32], 25);

        assert_eq!(state.received_frames, 2);
        assert_eq!(state.overwritten_frames, 1);
        assert_eq!(state.maximum_gap_ns, 15);
        let frame = state.latest.expect("latest frame");
        assert_eq!(frame.sequence(), 2);
        assert_eq!(frame.received_monotonic_ns(), 25);
        assert_eq!(frame.bytes(), &[2; 32]);
    }

    #[test]
    fn caps_memory_and_stride_drift_fail_closed() {
        let mut memory = negotiated_state();
        memory.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[0; 32], 1);
        memory.accept_frame(UncalibratedMemoryType::DmaBuf, 16, &[0; 32], 2);
        assert_eq!(
            memory.terminal.map(|terminal| terminal.error_type),
            Some(CaptureErrorType::UnsupportedMemoryType)
        );

        let mut stride = negotiated_state();
        stride.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[0; 32], 1);
        stride.accept_frame(UncalibratedMemoryType::MemoryPointer, 20, &[0; 40], 2);
        assert_eq!(
            stride.terminal.map(|terminal| terminal.error_type),
            Some(CaptureErrorType::FrameMalformed)
        );

        let mut caps = negotiated_state();
        let mut changed = VideoInfoRaw::new();
        changed.set_format(VideoFormat::BGRx);
        changed.set_size(spa::utils::Rectangle {
            width: 8,
            height: 2,
        });
        changed.set_framerate(spa::utils::Fraction { num: 60, denom: 1 });
        caps.negotiate(changed);
        assert_eq!(
            caps.terminal.map(|terminal| terminal.error_type),
            Some(CaptureErrorType::UnsupportedFormat)
        );
    }

    #[test]
    fn malformed_frames_are_rejected_before_copying() {
        let mut state = negotiated_state();
        state.accept_frame(UncalibratedMemoryType::MemoryPointer, 12, &[0; 32], 1);
        assert_eq!(state.received_frames, 0);
        assert_eq!(
            state.terminal.map(|terminal| terminal.error_type),
            Some(CaptureErrorType::FrameMalformed)
        );
    }

    #[test]
    fn debug_output_omits_pixel_content() {
        let mut state = negotiated_state();
        state.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[0xabu8; 32], 1);
        let output = format!("{:?}", state.latest.as_ref().expect("frame"));

        assert!(output.contains("byte_count"));
        assert!(!output.contains("171"));
        assert!(!output.contains("ab"));
    }

    #[test]
    fn null_format_clear_does_not_replace_stream_loss_classification() {
        let state = Arc::new(Mutex::new(negotiated_state()));
        state.borrow_mut().active_seen = true;

        handle_format(None, &state, spa::param::ParamType::Format.as_raw(), None);
        assert_eq!(state.borrow().terminal, None);

        handle_stream_state(&state, &pw::stream::StreamState::Unconnected);
        assert_eq!(
            state.borrow().terminal,
            Some(ReceiverTerminal {
                error_type: CaptureErrorType::StreamLost,
                operation: CaptureDiagnosticOperation::FirstFrame,
            })
        );
    }

    #[test]
    fn intentional_shutdown_does_not_latch_stream_loss() {
        let state = Arc::new(Mutex::new(negotiated_state()));
        {
            let mut state = state.borrow_mut();
            state.active_seen = true;
            state.shutting_down = true;
        }

        handle_stream_state(&state, &pw::stream::StreamState::Unconnected);

        assert_eq!(state.borrow().terminal, None);
    }

    #[test]
    fn pixel_semantics_drift_and_non_linear_layout_fail_closed() {
        let mut drift = negotiated_state();
        drift.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[0; 32], 1);
        let mut changed = video_info();
        changed.set_color_range(1);
        drift.negotiate(changed);
        assert_eq!(
            drift.terminal,
            Some(ReceiverTerminal {
                error_type: CaptureErrorType::UnsupportedFormat,
                operation: CaptureDiagnosticOperation::SteadyReception,
            })
        );

        let mut modifier_state = negotiated_state();
        modifier_state.accept_frame(UncalibratedMemoryType::MemoryPointer, 16, &[0; 32], 1);
        let mut non_linear = video_info();
        non_linear.set_modifier(1);
        modifier_state.negotiate(non_linear);
        assert_eq!(
            modifier_state.terminal,
            Some(ReceiverTerminal {
                error_type: CaptureErrorType::UnsupportedFormat,
                operation: CaptureDiagnosticOperation::SteadyReception,
            })
        );

        let mut interlaced = ReceiverState::new(Instant::now());
        let mut non_progressive = video_info();
        non_progressive.set_interlace_mode(VideoInterlaceMode::Interleaved);
        interlaced.negotiate(non_progressive);
        assert_eq!(
            interlaced.terminal.map(|terminal| terminal.error_type),
            Some(CaptureErrorType::UnsupportedFormat)
        );
    }

    #[test]
    fn terminal_steady_reception_is_not_summarized_twice() {
        assert!(!should_record_steady_summary(0, None));
        assert!(should_record_steady_summary(2, None));
        assert!(!should_record_steady_summary(
            2,
            Some(CaptureDiagnosticOperation::SteadyReception)
        ));
        assert!(should_record_steady_summary(
            2,
            Some(CaptureDiagnosticOperation::StreamNegotiation)
        ));
    }

    #[test]
    fn shutdown_records_a_latched_stream_terminal() {
        let receiver = receiver_for_admission();
        receiver.state.borrow_mut().fail(
            CaptureErrorType::StreamLost,
            CaptureDiagnosticOperation::SteadyReception,
        );
        let mut facts = Facts::default();

        receiver.shutdown(&mut facts).unwrap();

        assert!(facts.0.iter().any(|fact| {
            fact.operation == CaptureDiagnosticOperation::SteadyReception
                && fact.status == CaptureDiagnosticStatus::Error
                && fact.error_type == Some(CaptureErrorType::StreamLost)
        }));
    }

    #[test]
    fn shutdown_routes_a_source_terminal_through_the_provider_fact() {
        let receiver = receiver_for_admission();
        receiver.state.borrow_mut().fail(
            CaptureErrorType::SourceLost,
            CaptureDiagnosticOperation::SourceLifetime,
        );
        let mut facts = Facts::default();

        receiver.shutdown(&mut facts).unwrap();

        let lifetime: Vec<_> = facts
            .0
            .iter()
            .filter(|fact| fact.operation == CaptureDiagnosticOperation::SourceLifetime)
            .collect();
        assert_eq!(lifetime.len(), 1);
        assert_eq!(lifetime[0].status, CaptureDiagnosticStatus::Error);
        assert_eq!(lifetime[0].error_type, Some(CaptureErrorType::SourceLost));
    }

    #[test]
    fn provider_error_at_shutdown_latches_source_lifetime_before_receiver_teardown() {
        let state = Arc::new(Mutex::new(ReceiverState::new(Instant::now())));

        latch_source_terminal(&state, CaptureErrorType::SourceLost);
        state.borrow_mut().shutting_down = true;

        assert_eq!(
            state.borrow().terminal,
            Some(ReceiverTerminal {
                error_type: CaptureErrorType::SourceLost,
                operation: CaptureDiagnosticOperation::SourceLifetime,
            })
        );
    }

    #[test]
    fn receiver_fact_bounds_retain_phase_durations() {
        let arguments = (10, Some(12_000_000), Some(17_000_000), Some(29), 30);
        assert_eq!(
            receiver_fact_bounds(
                CaptureDiagnosticOperation::StreamNegotiation,
                arguments.0,
                arguments.1,
                arguments.2,
                arguments.3,
                arguments.4,
            ),
            (10, 12)
        );
        assert_eq!(
            receiver_fact_bounds(
                CaptureDiagnosticOperation::FirstFrame,
                arguments.0,
                arguments.1,
                arguments.2,
                arguments.3,
                arguments.4,
            ),
            (12, 17)
        );
        assert_eq!(
            receiver_fact_bounds(
                CaptureDiagnosticOperation::SteadyReception,
                arguments.0,
                arguments.1,
                arguments.2,
                arguments.3,
                arguments.4,
            ),
            (17, 30)
        );
        assert_eq!(
            receiver_fact_bounds(
                CaptureDiagnosticOperation::ReceiverShutdown,
                arguments.0,
                arguments.1,
                arguments.2,
                arguments.3,
                arguments.4,
            ),
            (29, 30)
        );
    }
}
