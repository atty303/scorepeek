use std::cell::RefCell;
use std::fmt;
use std::io::Cursor;
use std::rc::Rc;
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::buffer::{ChunkFlags, DataType};
use pw::spa::param::video::{VideoFormat, VideoInfoRaw, VideoInterlaceMode};
use pw::spa::pod::{Pod, Value};
use serde::Serialize;

use super::{
    CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
    CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureError, CaptureErrorType,
    ITERATION_SLICE, UncalibratedGamescopeSourceLease, elapsed_ms,
};

const MAX_WIDTH: u32 = 7_680;
const MAX_HEIGHT: u32 = 4_320;
const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;
const MAX_BUFFERS_PER_CALLBACK: usize = 64;
const REQUESTED_FRAMERATE_NUM: u32 = 60;
const REQUESTED_FRAMERATE_DENOM: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
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
    fn new() -> Self {
        Self {
            started: Instant::now(),
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

    fn negotiate(&mut self, info: VideoInfoRaw) {
        if self.terminal.is_some() {
            return;
        }
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
            let operation = self.reception_operation();
            self.fail(CaptureErrorType::UnsupportedFormat, operation);
            return;
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
        match self.contract {
            None => {
                self.contract = Some(contract);
                self.contract_received_ns =
                    Some(u64::try_from(self.started.elapsed().as_nanos()).unwrap_or(u64::MAX));
            }
            Some(existing) if existing == contract => {}
            Some(_) => {
                let operation = self.reception_operation();
                self.fail(CaptureErrorType::UnsupportedFormat, operation);
            }
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
    listener: Option<pw::stream::StreamListener<Rc<RefCell<ReceiverState>>>>,
    stream: Option<pw::stream::StreamRc>,
    lease: Option<UncalibratedGamescopeSourceLease>,
    state: Rc<RefCell<ReceiverState>>,
    receiver_started_ms: u64,
    shutdown_started_ms: Option<u64>,
    negotiation_recorded: bool,
    first_frame_recorded: bool,
    terminal_recorded: Option<CaptureDiagnosticOperation>,
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
        self.state.borrow_mut().latest.take()
    }

    /// Disconnects and drops the receiver before releasing its provider lease.
    ///
    /// # Errors
    /// Returns `ReceiverFailed` if the explicit stream disconnect fails. Receiver and provider
    /// resources are still released and their shutdown facts are still attempted.
    pub fn shutdown(mut self, sink: &mut impl CaptureDiagnosticSink) -> Result<(), CaptureError> {
        self.flush_observations(sink);
        self.shutdown_started_ms = self.lease.as_ref().map(|lease| elapsed_ms(lease.started));
        self.state.borrow_mut().shutting_down = true;
        let disconnect_error = self
            .stream
            .as_ref()
            .and_then(|stream| stream.disconnect().err());
        self.listener.take();
        self.stream.take();
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
        let shutdown_status = if disconnect_error.is_some() {
            CaptureDiagnosticStatus::Error
        } else {
            CaptureDiagnosticStatus::Success
        };
        let shutdown_error = disconnect_error
            .as_ref()
            .map(|_| CaptureErrorType::ReceiverFailed);
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
        disconnect_error.map_or(Ok(()), |source| {
            Err(CaptureError::with_source(
                CaptureErrorType::ReceiverFailed,
                source,
            ))
        })
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

    fn fail_start(
        mut self,
        error: CaptureError,
        sink: &mut impl CaptureDiagnosticSink,
    ) -> CaptureError {
        self.record_terminal(
            ReceiverTerminal {
                error_type: error.error_type(),
                operation: CaptureDiagnosticOperation::StreamNegotiation,
            },
            CaptureDiagnosticStatus::Error,
            sink,
        );
        let _ = self.shutdown(sink);
        error
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
    let contract_ms =
        contract_received_ns.map(|value| receiver_started_ms.saturating_add(value / 1_000_000));
    let first_ms =
        first_received_ns.map(|value| receiver_started_ms.saturating_add(value / 1_000_000));
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
    let core = lease
        .runtime
        .as_ref()
        .map(|runtime| runtime.core.clone())
        .ok_or_else(|| CaptureError::without_source(CaptureErrorType::SourceLost))?;
    let stream = match pw::stream::StreamRc::new(
        core,
        "scorepeek-uncalibrated-video-receiver",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
            *pw::keys::VIDEO_RATE => "60/1",
        },
    ) {
        Ok(stream) => stream,
        Err(source) => {
            let error = CaptureError::with_source(CaptureErrorType::ReceiverFailed, source);
            return Err(fail_before_receiver(lease, error, sink));
        }
    };
    let receiver_started_ms = elapsed_ms(lease.started);
    let state = Rc::new(RefCell::new(ReceiverState::new()));
    let listener = match register_stream_listener(&stream, Rc::clone(&state)) {
        Ok(listener) => listener,
        Err(source) => {
            drop(stream);
            let error = CaptureError::with_source(CaptureErrorType::ReceiverFailed, source);
            return Err(fail_before_receiver(lease, error, sink));
        }
    };
    let receiver = UncalibratedPipeWireReceiver {
        listener: Some(listener),
        stream: Some(stream),
        lease: Some(lease),
        state,
        receiver_started_ms,
        shutdown_started_ms: None,
        negotiation_recorded: false,
        first_frame_recorded: false,
        terminal_recorded: None,
    };
    let Ok(values) = format_offer() else {
        let error = CaptureError::without_source(CaptureErrorType::ReceiverFailed);
        return Err(receiver.fail_start(error, sink));
    };
    let Some(param) = Pod::from_bytes(&values) else {
        let error = CaptureError::without_source(CaptureErrorType::ReceiverFailed);
        return Err(receiver.fail_start(error, sink));
    };
    let mut params = [param];
    let Some(stream) = receiver.stream.as_ref() else {
        let error = CaptureError::without_source(CaptureErrorType::ReceiverFailed);
        return Err(receiver.fail_start(error, sink));
    };
    let connect_result = stream.connect(
        spa::utils::Direction::Input,
        receiver.lease.as_ref().map(|lease| lease.node_id),
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::NO_CONVERT
            | pw::stream::StreamFlags::DONT_RECONNECT,
        &mut params,
    );
    if let Err(source) = connect_result {
        let error = CaptureError::with_source(CaptureErrorType::ReceiverFailed, source);
        return Err(receiver.fail_start(error, sink));
    }

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
        let ready = {
            let state = receiver.state.borrow();
            state.contract.is_some() && state.received_frames > 0
        };
        if ready {
            return Ok(receiver);
        }
        if startup_started.elapsed() >= timeout {
            let operation = if receiver.state.borrow().contract.is_some() {
                CaptureDiagnosticOperation::FirstFrame
            } else {
                CaptureDiagnosticOperation::StreamNegotiation
            };
            let error_type = if operation == CaptureDiagnosticOperation::FirstFrame {
                CaptureErrorType::FirstFrameTimedOut
            } else {
                CaptureErrorType::NegotiationTimedOut
            };
            receiver.record_terminal(
                ReceiverTerminal {
                    error_type,
                    operation,
                },
                CaptureDiagnosticStatus::Timeout,
                sink,
            );
            let _ = receiver.shutdown(sink);
            return Err(CaptureError::without_source(error_type));
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
    state: Rc<RefCell<ReceiverState>>,
) -> Result<pw::stream::StreamListener<Rc<RefCell<ReceiverState>>>, pw::Error> {
    stream
        .add_local_listener_with_user_data(state)
        .state_changed(|_, state, _, new| handle_stream_state(state, &new))
        .param_changed(|_, state, id, param| handle_format(state, id, param))
        .process(process_buffers)
        .register()
}

fn handle_stream_state(state: &Rc<RefCell<ReceiverState>>, new: &pw::stream::StreamState) {
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

fn handle_format(state: &Rc<RefCell<ReceiverState>>, id: u32, param: Option<&Pod>) {
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
    state.borrow_mut().negotiate(info);
}

fn fail_format(state: &Rc<RefCell<ReceiverState>>) {
    let mut state = state.borrow_mut();
    let operation = state.reception_operation();
    state.fail(CaptureErrorType::UnsupportedFormat, operation);
}

fn process_buffers(stream: &pw::stream::Stream, state: &mut Rc<RefCell<ReceiverState>>) {
    for _ in 0..MAX_BUFFERS_PER_CALLBACK {
        let Some(mut buffer) = stream.dequeue_buffer() else {
            return;
        };
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
    state.borrow_mut().fail(
        CaptureErrorType::ReceiverFailed,
        CaptureDiagnosticOperation::SteadyReception,
    );
}

fn fail_frame(state: &Rc<RefCell<ReceiverState>>, error_type: CaptureErrorType) {
    let mut state = state.borrow_mut();
    let operation = state.reception_operation();
    state.fail(error_type, operation);
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
        let mut state = ReceiverState::new();
        let info = video_info();
        state.negotiate(info);
        state
    }

    #[test]
    fn unspecified_producer_framerate_is_preserved() {
        let mut state = ReceiverState::new();
        let mut info = video_info();
        info.set_framerate(spa::utils::Fraction { num: 0, denom: 1 });

        state.negotiate(info);

        assert_eq!(state.terminal, None);
        assert_eq!(state.contract.expect("contract").framerate_num, 0);
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
        let state = Rc::new(RefCell::new(negotiated_state()));
        state.borrow_mut().active_seen = true;

        handle_format(&state, spa::param::ParamType::Format.as_raw(), None);
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
        let state = Rc::new(RefCell::new(negotiated_state()));
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

        let mut interlaced = ReceiverState::new();
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
    fn receiver_fact_bounds_retain_phase_durations() {
        let arguments = (10, Some(2_000_000), Some(7_000_000), Some(29), 30);
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
