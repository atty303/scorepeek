use std::collections::VecDeque;
use std::fs;
use std::io::IoSliceMut;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, poll};
use rustix::fs::{Mode, chmod};
use rustix::io::dup;
use rustix::net::{
    AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendFlags, SocketAddrUnix,
    SocketFlags, SocketType, accept_with, bind, listen, recv, recvmsg, send, socket_with,
};
use rustix::time::{ClockId, Timespec, clock_gettime};
use scorepeek_vulkan_capture::{ImageContract, ImportedImage, MAX_PLANES, PlaneLayout};

const MAGIC: u32 = 0x4b56_5053;
const VERSION: u16 = 5;
const HELLO: u16 = 1;
const HELLO_ACK: u16 = 2;
const REQUEST: u16 = 3;
const READY: u16 = 4;
const ACK: u16 = 5;
const ERROR: u16 = 6;
const ADMIT: u16 = 7;
const ADMIT_ACK: u16 = 8;
const STATUS: u16 = 9;
const HELLO_BYTES: usize = 232;
const PACKET_BYTES: usize = 96;
const REQUEST_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VulkanPixelOrder {
    Bgra,
    Rgba,
}

#[derive(Debug)]
pub struct VulkanFrameData {
    pub width: u32,
    pub height: u32,
    pub sequence: u64,
    pub received_monotonic_ns: u64,
    pub bytes: Box<[u8]>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VulkanTimingStats {
    pub requests: u64,
    pub captures: u64,
    pub busy_drops: u64,
    pub coalesced_drops: u64,
    pub request_to_present_ns: u64,
    pub producer_submit_ns: u64,
    pub present_call_ns: u64,
    pub producer_fence_ns: u64,
    pub consumer_readback_ns: u64,
    pub total_ns: u64,
}

#[derive(Debug)]
enum WorkerTerminal {
    Disconnected,
    Producer { status: i32 },
    Readback,
    Protocol,
}

#[derive(Debug, Eq, PartialEq)]
enum TransportError {
    Disconnected,
    Producer { status: i32 },
    Import(String),
    Protocol(String),
    Transport(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VulkanAcceptFailure {
    Producer { status: i32 },
    Import,
    Protocol,
    Transport,
}

impl VulkanAcceptFailure {
    #[must_use]
    pub const fn diagnostic(self) -> (&'static str, Option<i32>) {
        match self {
            Self::Producer { status } => ("producer", Some(status)),
            Self::Import => ("import", None),
            Self::Protocol => ("protocol", None),
            Self::Transport => ("transport", None),
        }
    }
}

trait CaptureImage: Send {
    fn readback(&mut self) -> Result<scorepeek_vulkan_capture::Readback, String>;

    fn profile(&self) -> scorepeek_vulkan_capture::ReadbackProfile {
        scorepeek_vulkan_capture::ReadbackProfile::default()
    }
}

impl CaptureImage for ImportedImage {
    fn readback(&mut self) -> Result<scorepeek_vulkan_capture::Readback, String> {
        ImportedImage::readback(self)
    }

    fn profile(&self) -> scorepeek_vulkan_capture::ReadbackProfile {
        ImportedImage::profile(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VulkanSessionFailure {
    Disconnected,
    Producer { status: i32 },
    Readback,
    Protocol,
}

#[derive(Debug, Default)]
struct SessionState {
    latest: Option<VulkanFrameData>,
    timing: VulkanTimingStats,
    readback_profile: scorepeek_vulkan_capture::ReadbackProfile,
    terminal: Option<WorkerTerminal>,
    timing_samples: VecDeque<VulkanTimingStats>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VulkanTimingDistribution {
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VulkanPerformanceSummary {
    pub count: u64,
    pub request_to_present: VulkanTimingDistribution,
    pub producer_submit: VulkanTimingDistribution,
    pub present_call: VulkanTimingDistribution,
    pub producer_fence: VulkanTimingDistribution,
    pub consumer_readback: VulkanTimingDistribution,
    pub total: VulkanTimingDistribution,
    pub dropped: u64,
    pub requests: u64,
    pub captures: u64,
    pub busy_drops: u64,
    pub coalesced_drops: u64,
    pub readback_profile: scorepeek_vulkan_capture::ReadbackProfile,
}

pub struct VulkanListener {
    fd: OwnedFd,
    path: PathBuf,
}

impl VulkanListener {
    /// Creates the private fixed per-user capture endpoint.
    ///
    /// # Errors
    /// Returns a stable setup failure when the runtime directory is absent or the socket cannot
    /// be created and secured.
    pub fn bind_default() -> Result<Self, String> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "vulkan-layer capture requires XDG_RUNTIME_DIR".to_owned())?;
        Self::bind_at(PathBuf::from(runtime).join("scorepeek/vulkan-capture.sock"))
    }

    /// Creates a private listener at an explicit path, primarily for hardware-independent tests.
    ///
    /// # Errors
    /// Returns a stable filesystem or socket operation failure.
    pub fn bind_at(path: PathBuf) -> Result<Self, String> {
        let parent = path
            .parent()
            .ok_or_else(|| "Vulkan capture socket has no parent".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("create Vulkan socket directory: {error}"))?;
        fs::set_permissions(parent, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .map_err(|error| format!("secure Vulkan socket directory: {error}"))?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("remove stale Vulkan socket: {error}")),
        }
        let fd = socket_with(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
            None,
        )
        .map_err(|error| format!("create Vulkan socket: {error}"))?;
        let address = SocketAddrUnix::new(&path)
            .map_err(|error| format!("address Vulkan socket: {error}"))?;
        bind(&fd, &address).map_err(|error| format!("bind Vulkan socket: {error}"))?;
        chmod(&path, Mode::RUSR | Mode::WUSR)
            .map_err(|error| format!("secure Vulkan socket: {error}"))?;
        listen(&fd, 8).map_err(|error| format!("listen Vulkan socket: {error}"))?;
        Ok(Self { fd, path })
    }

    /// Accepts at most one producer within the bounded timeout.
    ///
    /// # Errors
    /// Returns a stable handshake, protocol, or Vulkan import failure.
    pub fn accept(&self, timeout: Duration) -> Result<Option<VulkanSession>, VulkanAcceptFailure> {
        if !wait_readable(&self.fd, timeout).map_err(|_| VulkanAcceptFailure::Transport)? {
            return Ok(None);
        }
        let socket = accept_with(&self.fd, SocketFlags::CLOEXEC)
            .map_err(|_| VulkanAcceptFailure::Transport)?;
        let admission_listener = dup(&self.fd).map_err(|_| VulkanAcceptFailure::Transport)?;
        match VulkanSession::start(socket, admission_listener) {
            Ok(session) => Ok(Some(session)),
            Err(TransportError::Disconnected) => Ok(None),
            Err(TransportError::Producer { status }) => {
                Err(VulkanAcceptFailure::Producer { status })
            }
            Err(TransportError::Import(_)) => Err(VulkanAcceptFailure::Import),
            Err(TransportError::Protocol(_)) => Err(VulkanAcceptFailure::Protocol),
            Err(TransportError::Transport(_)) => Err(VulkanAcceptFailure::Transport),
        }
    }

    /// Rejects already queued producers while one generation owns capture admission.
    ///
    /// # Errors
    /// Returns a socket failure if the pending-producer queue cannot be inspected.
    pub fn reject_pending(&self) -> Result<u64, String> {
        let mut rejected = 0_u64;
        loop {
            match accept_with(&self.fd, SocketFlags::CLOEXEC | SocketFlags::NONBLOCK) {
                Ok(socket) => {
                    let packet = encode_packet(Packet {
                        message_type: ERROR,
                        status: 9,
                        ..Packet::default()
                    });
                    let _ = send(&socket, &packet, SendFlags::NOSIGNAL);
                    rejected = rejected.saturating_add(1);
                }
                Err(error) if error == rustix::io::Errno::AGAIN => break,
                Err(error) => return Err(format!("reject busy Vulkan producer: {error}")),
            }
        }
        Ok(rejected)
    }
}

impl Drop for VulkanListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub struct VulkanSession {
    contract: ImageContract,
    state: Arc<Mutex<SessionState>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    last_summary: Instant,
}

impl VulkanSession {
    fn start(socket: OwnedFd, admission_listener: OwnedFd) -> Result<Self, TransportError> {
        Self::start_with_importer(socket, admission_listener, |contract, dma_buf| {
            ImportedImage::import(contract, dma_buf)
                .map(|image| Box::new(image) as Box<dyn CaptureImage>)
        })
    }

    fn start_with_importer(
        socket: OwnedFd,
        admission_listener: OwnedFd,
        importer: impl FnOnce(ImageContract, OwnedFd) -> Result<Box<dyn CaptureImage>, String>,
    ) -> Result<Self, TransportError> {
        if !wait_readable(&socket, Duration::from_secs(2)).map_err(TransportError::Transport)? {
            return Err(TransportError::Protocol(
                "Vulkan producer admission timed out".to_owned(),
            ));
        }
        let admission = receive_packet(&socket, Duration::ZERO)?.ok_or_else(|| {
            TransportError::Protocol("Vulkan producer admission packet missing".to_owned())
        })?;
        if admission.message_type != ADMIT {
            return Err(TransportError::Protocol(
                "invalid Vulkan admission request".to_owned(),
            ));
        }
        send_packet(
            &socket,
            Packet {
                message_type: ADMIT_ACK,
                ..Packet::default()
            },
        )?;
        if !wait_readable(&socket, Duration::from_secs(2)).map_err(TransportError::Transport)? {
            return Err(TransportError::Protocol(
                "Vulkan producer handshake timed out".to_owned(),
            ));
        }
        let (contract, order, dma_buf) = receive_hello(&socket)?;
        let width = contract.width;
        let height = contract.height;
        let capture_image = importer(contract, dma_buf).map_err(TransportError::Import)?;
        send_packet(
            &socket,
            Packet {
                message_type: HELLO_ACK,
                ..Packet::default()
            },
        )?;
        let state = Arc::new(Mutex::new(SessionState {
            readback_profile: capture_image.profile(),
            ..SessionState::default()
        }));
        let worker_state = Arc::clone(&state);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("scorepeek-vulkan-capture".to_owned())
            .spawn(move || {
                run_worker(
                    socket,
                    admission_listener,
                    capture_image,
                    order,
                    width,
                    height,
                    worker_state,
                    worker_stop,
                );
            })
            .map_err(|error| {
                TransportError::Transport(format!("start Vulkan capture worker: {error}"))
            })?;
        Ok(Self {
            contract,
            state,
            stop,
            worker: Some(worker),
            last_summary: Instant::now(),
        })
    }

    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.contract.width, self.contract.height)
    }

    #[must_use]
    pub const fn contract(&self) -> ImageContract {
        self.contract
    }

    pub fn take_latest(&mut self) -> Option<VulkanFrameData> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .latest
            .take()
    }

    #[must_use]
    pub fn timing(&self) -> VulkanTimingStats {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .timing
    }

    /// Checks whether the producer and asynchronous readback worker remain usable.
    ///
    /// # Errors
    /// Returns the terminal disconnect, protocol, or Vulkan readback failure.
    pub fn poll(&self) -> Result<(), VulkanSessionFailure> {
        match self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .terminal
            .as_ref()
        {
            None => Ok(()),
            Some(WorkerTerminal::Disconnected) => Err(VulkanSessionFailure::Disconnected),
            Some(WorkerTerminal::Producer { status }) => {
                Err(VulkanSessionFailure::Producer { status: *status })
            }
            Some(WorkerTerminal::Readback) => Err(VulkanSessionFailure::Readback),
            Some(WorkerTerminal::Protocol) => Err(VulkanSessionFailure::Protocol),
        }
    }

    pub fn periodic_summary(&mut self, interval: Duration) -> Option<VulkanPerformanceSummary> {
        if self.last_summary.elapsed() < interval {
            return None;
        }
        self.last_summary = Instant::now();
        Some(performance_summary(&self.state))
    }

    #[must_use]
    pub fn final_summary(&self) -> VulkanPerformanceSummary {
        performance_summary(&self.state)
    }
}

impl Drop for VulkanSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[allow(
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the capture thread must own every resource and keep its protocol loop together"
)]
fn run_worker(
    socket: OwnedFd,
    admission_listener: OwnedFd,
    mut imported: Box<dyn CaptureImage>,
    order: VulkanPixelOrder,
    width: u32,
    height: u32,
    state: Arc<Mutex<SessionState>>,
    stop: Arc<AtomicBool>,
) {
    let mut next_request = Instant::now();
    let mut sequence = 0_u64;
    while !stop.load(Ordering::Acquire) {
        if reject_busy_producers(&admission_listener).is_err() {
            set_terminal(&state, WorkerTerminal::Protocol);
            return;
        }
        let now = Instant::now();
        if now >= next_request {
            sequence = sequence.saturating_add(1);
            let request = Packet {
                message_type: REQUEST,
                sequence,
                request_ns: monotonic_ns(),
                ..Packet::default()
            };
            if let Err(error) = send_packet(&socket, request) {
                set_terminal(
                    &state,
                    match error {
                        TransportError::Disconnected => WorkerTerminal::Disconnected,
                        TransportError::Producer { status } => WorkerTerminal::Producer { status },
                        TransportError::Import(_)
                        | TransportError::Protocol(_)
                        | TransportError::Transport(_) => WorkerTerminal::Protocol,
                    },
                );
                return;
            }
            next_request = now + REQUEST_INTERVAL;
        }
        match receive_packet(&socket, Duration::from_millis(10)) {
            Ok(None) => {}
            Err(TransportError::Disconnected) => {
                set_terminal(&state, WorkerTerminal::Disconnected);
                return;
            }
            Ok(Some(packet)) if packet.message_type == READY => {
                {
                    let mut current = state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    merge_producer_counters(&mut current.timing, packet);
                }
                let Ok(readback) = imported.readback() else {
                    set_terminal(&state, WorkerTerminal::Readback);
                    return;
                };
                let consumer_done_ns = monotonic_ns();
                let mut bytes = readback.bytes;
                if order == VulkanPixelOrder::Rgba {
                    for pixel in bytes.chunks_exact_mut(4) {
                        pixel.swap(0, 2);
                    }
                }
                {
                    let mut current = state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let requests = current.timing.requests.max(packet.requests);
                    let captures = current.timing.captures.max(packet.captures);
                    let busy_drops = current.timing.busy_drops.max(packet.busy_drops);
                    let coalesced_drops =
                        current.timing.coalesced_drops.max(packet.coalesced_drops);
                    current.latest = Some(VulkanFrameData {
                        width,
                        height,
                        sequence: packet.sequence,
                        received_monotonic_ns: consumer_done_ns,
                        bytes,
                    });
                    let timing = VulkanTimingStats {
                        requests,
                        captures,
                        busy_drops,
                        coalesced_drops,
                        request_to_present_ns: packet.present_ns.saturating_sub(packet.request_ns),
                        producer_submit_ns: packet.submit_done_ns.saturating_sub(packet.present_ns),
                        present_call_ns: packet.present_call_ns,
                        producer_fence_ns: packet.producer_fence_ns,
                        consumer_readback_ns: readback.fence_ns.saturating_sub(readback.submit_ns),
                        total_ns: consumer_done_ns.saturating_sub(packet.request_ns),
                    };
                    current.timing = timing;
                    if current.timing_samples.len() == 600 {
                        current.timing_samples.pop_front();
                    }
                    current.timing_samples.push_back(timing);
                }
                if let Err(error) = send_packet(
                    &socket,
                    Packet {
                        message_type: ACK,
                        sequence: packet.sequence,
                        ..Packet::default()
                    },
                ) {
                    set_terminal(
                        &state,
                        match error {
                            TransportError::Disconnected => WorkerTerminal::Disconnected,
                            TransportError::Producer { status } => {
                                WorkerTerminal::Producer { status }
                            }
                            TransportError::Import(_)
                            | TransportError::Protocol(_)
                            | TransportError::Transport(_) => WorkerTerminal::Protocol,
                        },
                    );
                    return;
                }
            }
            Ok(Some(packet)) if packet.message_type == STATUS => {
                let mut current = state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                merge_producer_counters(&mut current.timing, packet);
            }
            Ok(Some(packet)) if packet.message_type == ERROR => {
                let mut current = state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                merge_producer_counters(&mut current.timing, packet);
                drop(current);
                set_terminal(
                    &state,
                    WorkerTerminal::Producer {
                        status: packet.status,
                    },
                );
                return;
            }
            Err(TransportError::Producer { status }) => {
                set_terminal(&state, WorkerTerminal::Producer { status });
                return;
            }
            Err(
                TransportError::Import(_)
                | TransportError::Protocol(_)
                | TransportError::Transport(_),
            )
            | Ok(Some(_)) => {
                set_terminal(&state, WorkerTerminal::Protocol);
                return;
            }
        }
    }
}

fn merge_producer_counters(timing: &mut VulkanTimingStats, packet: Packet) {
    timing.requests = timing.requests.max(packet.requests);
    timing.captures = timing.captures.max(packet.captures);
    timing.busy_drops = timing.busy_drops.max(packet.busy_drops);
    timing.coalesced_drops = timing.coalesced_drops.max(packet.coalesced_drops);
}

fn reject_busy_producers(listener: &OwnedFd) -> Result<(), String> {
    loop {
        match accept_with(listener, SocketFlags::CLOEXEC | SocketFlags::NONBLOCK) {
            Ok(socket) => {
                let packet = encode_packet(Packet {
                    message_type: ERROR,
                    status: 9,
                    ..Packet::default()
                });
                let _ = send(&socket, &packet, SendFlags::NOSIGNAL);
            }
            Err(error) if error == rustix::io::Errno::AGAIN => return Ok(()),
            Err(error) => return Err(format!("reject busy Vulkan producer: {error}")),
        }
    }
}

fn receive_hello(
    socket: &OwnedFd,
) -> Result<(ImageContract, VulkanPixelOrder, OwnedFd), TransportError> {
    let mut bytes = [0_u8; HELLO_BYTES];
    let mut io = [IoSliceMut::new(&mut bytes)];
    let mut control_space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = RecvAncillaryBuffer::new(&mut control_space);
    let received = recvmsg(socket, &mut io, &mut control, RecvFlags::CMSG_CLOEXEC)
        .map_err(|error| transport_error("receive Vulkan hello", error))?;
    if received.bytes == 0 {
        return Err(TransportError::Disconnected);
    }
    if received.bytes == PACKET_BYTES
        && u32_at(&bytes, 0) == MAGIC
        && u16_at(&bytes, 4) == VERSION
        && u16_at(&bytes, 6) == ERROR
    {
        return Err(TransportError::Producer {
            status: i32::from_ne_bytes(bytes[88..92].try_into().expect("fixed slice")),
        });
    }
    if received.bytes != HELLO_BYTES
        || u32_at(&bytes, 0) != MAGIC
        || u16_at(&bytes, 4) != VERSION
        || u16_at(&bytes, 6) != HELLO
        || u32_at(&bytes, 8) as usize != HELLO_BYTES
    {
        return Err(TransportError::Protocol("invalid Vulkan hello".to_owned()));
    }
    let dma_buf = control
        .drain()
        .find_map(|message| match message {
            RecvAncillaryMessage::ScmRights(mut rights) => rights.next(),
            _ => None,
        })
        .ok_or_else(|| {
            TransportError::Protocol("Vulkan hello did not include one DMA-BUF".to_owned())
        })?;
    let plane_count = u32_at(&bytes, 28);
    if plane_count == 0 || plane_count as usize > MAX_PLANES {
        return Err(TransportError::Protocol(
            "invalid Vulkan plane count".to_owned(),
        ));
    }
    let vk_format = u32_at(&bytes, 20);
    let order = match vk_format {
        37 | 43 => VulkanPixelOrder::Rgba,
        44 | 50 => VulkanPixelOrder::Bgra,
        _ => {
            return Err(TransportError::Protocol(format!(
                "unsupported Vulkan format {vk_format}"
            )));
        }
    };
    let mut device_uuid = [0_u8; 16];
    device_uuid.copy_from_slice(&bytes[56..72]);
    let mut planes = [PlaneLayout::default(); MAX_PLANES];
    for (index, plane) in planes.iter_mut().enumerate().take(plane_count as usize) {
        let offset = 72 + index * 40;
        *plane = PlaneLayout {
            offset: u64_at(&bytes, offset),
            size: u64_at(&bytes, offset + 8),
            row_pitch: u64_at(&bytes, offset + 16),
            array_pitch: u64_at(&bytes, offset + 24),
            depth_pitch: u64_at(&bytes, offset + 32),
        };
    }
    Ok((
        ImageContract {
            width: u32_at(&bytes, 12),
            height: u32_at(&bytes, 16),
            vk_format,
            drm_fourcc: u32_at(&bytes, 24),
            modifier: u64_at(&bytes, 40),
            allocation_size: u64_at(&bytes, 48),
            plane_count,
            device_uuid,
            planes,
        },
        order,
        dma_buf,
    ))
}

#[derive(Clone, Copy, Debug, Default)]
struct Packet {
    message_type: u16,
    sequence: u64,
    request_ns: u64,
    present_ns: u64,
    submit_done_ns: u64,
    present_call_ns: u64,
    producer_fence_ns: u64,
    requests: u64,
    captures: u64,
    busy_drops: u64,
    coalesced_drops: u64,
    status: i32,
}

fn receive_packet(socket: &OwnedFd, timeout: Duration) -> Result<Option<Packet>, TransportError> {
    if !wait_readable(socket, timeout).map_err(TransportError::Transport)? {
        return Ok(None);
    }
    let mut bytes = [0_u8; PACKET_BYTES];
    let (count, _) = recv(socket, &mut bytes, RecvFlags::empty())
        .map_err(|error| transport_error("receive Vulkan packet", error))?;
    if count == 0 {
        return Err(TransportError::Disconnected);
    }
    if count != PACKET_BYTES || u32_at(&bytes, 0) != MAGIC || u16_at(&bytes, 4) != VERSION {
        return Err(TransportError::Protocol("invalid Vulkan packet".to_owned()));
    }
    Ok(Some(Packet {
        message_type: u16_at(&bytes, 6),
        sequence: u64_at(&bytes, 8),
        request_ns: u64_at(&bytes, 16),
        present_ns: u64_at(&bytes, 24),
        submit_done_ns: u64_at(&bytes, 32),
        present_call_ns: u64_at(&bytes, 40),
        producer_fence_ns: u64_at(&bytes, 48),
        requests: u64_at(&bytes, 56),
        captures: u64_at(&bytes, 64),
        busy_drops: u64_at(&bytes, 72),
        coalesced_drops: u64_at(&bytes, 80),
        status: i32::from_ne_bytes(bytes[88..92].try_into().expect("fixed slice")),
    }))
}

fn performance_summary(state: &Mutex<SessionState>) -> VulkanPerformanceSummary {
    let current = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    VulkanPerformanceSummary {
        count: current.timing_samples.len() as u64,
        request_to_present: timing_distribution(&current.timing_samples, |timing| {
            timing.request_to_present_ns
        }),
        producer_submit: timing_distribution(&current.timing_samples, |timing| {
            timing.producer_submit_ns
        }),
        present_call: timing_distribution(&current.timing_samples, |timing| timing.present_call_ns),
        producer_fence: timing_distribution(&current.timing_samples, |timing| {
            timing.producer_fence_ns
        }),
        consumer_readback: timing_distribution(&current.timing_samples, |timing| {
            timing.consumer_readback_ns
        }),
        total: timing_distribution(&current.timing_samples, |timing| timing.total_ns),
        dropped: current
            .timing
            .busy_drops
            .saturating_add(current.timing.coalesced_drops),
        requests: current.timing.requests,
        captures: current.timing.captures,
        busy_drops: current.timing.busy_drops,
        coalesced_drops: current.timing.coalesced_drops,
        readback_profile: current.readback_profile,
    }
}

fn timing_distribution(
    samples: &VecDeque<VulkanTimingStats>,
    value: impl Fn(&VulkanTimingStats) -> u64,
) -> VulkanTimingDistribution {
    let mut values = samples.iter().map(value).collect::<Vec<_>>();
    values.sort_unstable();
    let percentile = |numerator: usize| {
        if values.is_empty() {
            0
        } else {
            values[(values.len().saturating_sub(1) * numerator) / 100]
        }
    };
    VulkanTimingDistribution {
        p50_ns: percentile(50),
        p95_ns: percentile(95),
        p99_ns: percentile(99),
        max_ns: values.last().copied().unwrap_or(0),
    }
}

fn send_packet(socket: &OwnedFd, packet: Packet) -> Result<(), TransportError> {
    let bytes = encode_packet(packet);
    let count = send(socket, &bytes, SendFlags::NOSIGNAL)
        .map_err(|error| transport_error("send Vulkan packet", error))?;
    if count != PACKET_BYTES {
        return Err(TransportError::Protocol(
            "short Vulkan packet send".to_owned(),
        ));
    }
    Ok(())
}

fn transport_error(operation: &str, error: rustix::io::Errno) -> TransportError {
    if matches!(
        error,
        rustix::io::Errno::PIPE | rustix::io::Errno::CONNRESET | rustix::io::Errno::NOTCONN
    ) {
        TransportError::Disconnected
    } else {
        TransportError::Transport(format!("{operation}: {error}"))
    }
}

fn encode_packet(packet: Packet) -> [u8; PACKET_BYTES] {
    let mut bytes = [0_u8; PACKET_BYTES];
    put(&mut bytes, 0, &MAGIC.to_ne_bytes());
    put(&mut bytes, 4, &VERSION.to_ne_bytes());
    put(&mut bytes, 6, &packet.message_type.to_ne_bytes());
    put(&mut bytes, 8, &packet.sequence.to_ne_bytes());
    put(&mut bytes, 16, &packet.request_ns.to_ne_bytes());
    put(&mut bytes, 24, &packet.present_ns.to_ne_bytes());
    put(&mut bytes, 32, &packet.submit_done_ns.to_ne_bytes());
    put(&mut bytes, 40, &packet.present_call_ns.to_ne_bytes());
    put(&mut bytes, 48, &packet.producer_fence_ns.to_ne_bytes());
    put(&mut bytes, 56, &packet.requests.to_ne_bytes());
    put(&mut bytes, 64, &packet.captures.to_ne_bytes());
    put(&mut bytes, 72, &packet.busy_drops.to_ne_bytes());
    put(&mut bytes, 80, &packet.coalesced_drops.to_ne_bytes());
    put(&mut bytes, 88, &packet.status.to_ne_bytes());
    bytes
}

fn wait_readable(fd: &OwnedFd, timeout: Duration) -> Result<bool, String> {
    let mut fds = [PollFd::new(fd, PollFlags::IN)];
    let timeout = Timespec {
        tv_sec: i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX),
        tv_nsec: i64::from(timeout.subsec_nanos()),
    };
    match poll(&mut fds, Some(&timeout)) {
        Ok(ready) => Ok(ready != 0),
        Err(error) if error == rustix::io::Errno::INTR => Ok(false),
        Err(error) => Err(format!("poll Vulkan socket: {error}")),
    }
}

fn set_terminal(state: &Mutex<SessionState>, terminal: WorkerTerminal) {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .terminal = Some(terminal);
}

fn monotonic_ns() -> u64 {
    let now = clock_gettime(ClockId::Monotonic);
    u64::try_from(now.tv_sec)
        .unwrap_or(0)
        .saturating_mul(1_000_000_000)
        .saturating_add(u64::try_from(now.tv_nsec).unwrap_or(0))
}

fn put(target: &mut [u8], offset: usize, bytes: &[u8]) {
    target[offset..offset + bytes.len()].copy_from_slice(bytes);
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_ne_bytes(bytes[offset..offset + 2].try_into().expect("fixed slice"))
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("fixed slice"))
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().expect("fixed slice"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, sendmsg, socketpair};
    use std::io::IoSlice;
    use std::os::fd::AsFd as _;
    use std::os::unix::fs::PermissionsExt as _;

    struct FakeImage;

    impl CaptureImage for FakeImage {
        fn readback(&mut self) -> Result<scorepeek_vulkan_capture::Readback, String> {
            Ok(scorepeek_vulkan_capture::Readback {
                bytes: vec![1; 8].into_boxed_slice(),
                submit_ns: 1,
                fence_ns: 2,
            })
        }
    }

    struct FailingImage;

    impl CaptureImage for FailingImage {
        fn readback(&mut self) -> Result<scorepeek_vulkan_capture::Readback, String> {
            Err("readback failed".to_owned())
        }
    }

    #[test]
    fn packet_wire_size_and_offsets_match_layer_contract() {
        let bytes = encode_packet(Packet {
            message_type: REQUEST,
            sequence: 42,
            request_ns: 99,
            present_ns: 100,
            submit_done_ns: 101,
            present_call_ns: 102,
            producer_fence_ns: 103,
            requests: 104,
            captures: 105,
            busy_drops: 106,
            coalesced_drops: 107,
            status: 108,
        });
        assert_eq!(bytes.len(), 96);
        assert_eq!(u16_at(&bytes, 4), VERSION);
        assert_eq!(u16_at(&bytes, 6), REQUEST);
        assert_eq!(u64_at(&bytes, 8), 42);
        assert_eq!(u64_at(&bytes, 16), 99);
        assert_eq!(u64_at(&bytes, 24), 100);
        assert_eq!(u64_at(&bytes, 32), 101);
        assert_eq!(u64_at(&bytes, 40), 102);
        assert_eq!(u64_at(&bytes, 48), 103);
        assert_eq!(u64_at(&bytes, 56), 104);
        assert_eq!(u64_at(&bytes, 64), 105);
        assert_eq!(u64_at(&bytes, 72), 106);
        assert_eq!(u64_at(&bytes, 80), 107);
        assert_eq!(i32::from_ne_bytes(bytes[88..92].try_into().unwrap()), 108);

        let admission = encode_packet(Packet {
            message_type: ADMIT,
            ..Packet::default()
        });
        assert_eq!(u16_at(&admission, 6), ADMIT);
    }

    #[test]
    fn performance_summary_keeps_stage_distributions_and_real_drop_counters_separate() {
        let samples = [1_u64, 2, 3, 4]
            .into_iter()
            .map(|value| VulkanTimingStats {
                request_to_present_ns: value,
                producer_submit_ns: value * 10,
                present_call_ns: value * 100,
                producer_fence_ns: value * 1_000,
                consumer_readback_ns: value * 10_000,
                total_ns: value * 100_000,
                ..VulkanTimingStats::default()
            })
            .collect();
        let state = Mutex::new(SessionState {
            timing: VulkanTimingStats {
                requests: 11,
                captures: 4,
                busy_drops: 2,
                coalesced_drops: 5,
                ..VulkanTimingStats::default()
            },
            timing_samples: samples,
            ..SessionState::default()
        });

        let summary = performance_summary(&state);
        assert_eq!(summary.count, 4);
        assert_eq!(summary.request_to_present.p50_ns, 2);
        assert_eq!(summary.producer_submit.p95_ns, 30);
        assert_eq!(summary.present_call.p99_ns, 300);
        assert_eq!(summary.producer_fence.max_ns, 4_000);
        assert_eq!(summary.consumer_readback.max_ns, 40_000);
        assert_eq!(summary.total.max_ns, 400_000);
        assert_eq!(summary.dropped, 7);
        assert_eq!(summary.requests, 11);
        assert_eq!(summary.captures, 4);
    }

    #[test]
    fn explicit_layer_manifest_is_valid_and_points_to_the_installed_library() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../native/vulkan-capture/layer/VkLayer_SCOREPEEK_capture.json"
        ))
        .unwrap();
        assert_eq!(manifest["file_format_version"], "1.2.0");
        assert_eq!(manifest["layer"]["name"], "VK_LAYER_SCOREPEEK_capture");
        assert_eq!(manifest["layer"]["type"], "GLOBAL");
        assert_eq!(
            manifest["layer"]["library_path"],
            "../../scorepeek/vulkan-layer/libscorepeek_vulkan_capture.so"
        );
        assert_eq!(manifest["layer"]["implementation_version"], "1");
    }

    #[test]
    fn disconnect_errno_is_typed_without_string_matching() {
        assert_eq!(
            transport_error("send Vulkan packet", rustix::io::Errno::PIPE),
            TransportError::Disconnected
        );
        assert_eq!(
            transport_error("receive Vulkan packet", rustix::io::Errno::CONNRESET),
            TransportError::Disconnected
        );
        assert!(matches!(
            transport_error("receive Vulkan packet", rustix::io::Errno::INVAL),
            TransportError::Transport(_)
        ));
    }

    #[test]
    fn producer_error_before_hello_remains_typed() {
        let (consumer, producer) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        send_packet(
            &producer,
            Packet {
                message_type: ERROR,
                status: 6,
                ..Packet::default()
            },
        )
        .unwrap();
        assert_eq!(
            receive_hello(&consumer).unwrap_err(),
            TransportError::Producer { status: 6 }
        );
    }

    #[test]
    fn send_after_peer_exit_is_a_normal_disconnect() {
        let (consumer, producer) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        drop(producer);
        assert_eq!(
            send_packet(
                &consumer,
                Packet {
                    message_type: REQUEST,
                    ..Packet::default()
                }
            ),
            Err(TransportError::Disconnected)
        );
    }

    #[test]
    fn worker_preserves_producer_error_status() {
        let root = tempfile::tempdir().expect("tempdir");
        let listener = VulkanListener::bind_at(root.path().join("capture.sock")).unwrap();
        let admission_listener = dup(&listener.fd).unwrap();
        let (consumer, producer) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let state = Arc::new(Mutex::new(SessionState::default()));
        let worker_state = Arc::clone(&state);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            run_worker(
                consumer,
                admission_listener,
                Box::new(FakeImage),
                VulkanPixelOrder::Bgra,
                2,
                1,
                worker_state,
                worker_stop,
            );
        });
        let request = receive_packet(&producer, Duration::from_secs(1))
            .unwrap()
            .unwrap();
        send_packet(
            &producer,
            Packet {
                message_type: STATUS,
                sequence: request.sequence,
                requests: 1,
                captures: 0,
                busy_drops: 0,
                coalesced_drops: 0,
                ..Packet::default()
            },
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let timing = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .timing;
            if timing.requests == 1 {
                assert_eq!(timing.captures, 0);
                break;
            }
            if Instant::now() >= deadline {
                let terminal = state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .terminal
                    .as_ref()
                    .map(|value| format!("{value:?}"));
                panic!("producer status was not observed; terminal={terminal:?}");
            }
            thread::sleep(Duration::from_millis(5));
        }
        send_packet(
            &producer,
            Packet {
                message_type: ERROR,
                status: 6,
                sequence: request.sequence,
                requests: 1,
                captures: 1,
                ..Packet::default()
            },
        )
        .unwrap();
        worker.join().unwrap();
        assert!(matches!(
            state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .terminal
                .as_ref(),
            Some(WorkerTerminal::Producer { status: 6 })
        ));
        assert_eq!(performance_summary(&state).captures, 1);
    }

    #[test]
    fn worker_preserves_ready_counters_when_readback_fails() {
        let root = tempfile::tempdir().expect("tempdir");
        let listener = VulkanListener::bind_at(root.path().join("capture.sock")).unwrap();
        let admission_listener = dup(&listener.fd).unwrap();
        let (consumer, producer) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let state = Arc::new(Mutex::new(SessionState::default()));
        let worker_state = Arc::clone(&state);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            run_worker(
                consumer,
                admission_listener,
                Box::new(FailingImage),
                VulkanPixelOrder::Bgra,
                2,
                1,
                worker_state,
                worker_stop,
            );
        });
        let request = receive_packet(&producer, Duration::from_secs(1))
            .unwrap()
            .unwrap();
        send_packet(
            &producer,
            Packet {
                message_type: READY,
                sequence: request.sequence,
                requests: 1,
                captures: 1,
                ..Packet::default()
            },
        )
        .unwrap();
        worker.join().unwrap();
        let current = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(matches!(current.terminal, Some(WorkerTerminal::Readback)));
        assert_eq!(current.timing.requests, 1);
        assert_eq!(current.timing.captures, 1);
    }

    #[test]
    fn fixed_socket_is_private_and_recreated() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("scorepeek/vulkan-capture.sock");
        let listener = VulkanListener::bind_at(path.clone()).expect("listener");
        let mode = fs::metadata(&path).expect("metadata").permissions();
        assert_eq!(mode.mode() & 0o777, 0o600);
        drop(listener);
        assert!(!path.exists());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn hardware_independent_session_exercises_handshake_request_ready_and_ack() {
        let root = tempfile::tempdir().expect("tempdir");
        let listener = VulkanListener::bind_at(root.path().join("capture.sock")).unwrap();
        let admission_listener = dup(&listener.fd).unwrap();
        let (consumer, producer) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let producer_thread = thread::spawn(move || {
            send_packet(
                &producer,
                Packet {
                    message_type: ADMIT,
                    ..Packet::default()
                },
            )
            .unwrap();
            assert_eq!(
                receive_packet(&producer, Duration::from_secs(1))
                    .unwrap()
                    .unwrap()
                    .message_type,
                ADMIT_ACK
            );

            let mut hello = [0_u8; HELLO_BYTES];
            put(&mut hello, 0, &MAGIC.to_ne_bytes());
            put(&mut hello, 4, &VERSION.to_ne_bytes());
            put(&mut hello, 6, &HELLO.to_ne_bytes());
            put(
                &mut hello,
                8,
                &u32::try_from(HELLO_BYTES).unwrap().to_ne_bytes(),
            );
            put(&mut hello, 12, &2_u32.to_ne_bytes());
            put(&mut hello, 16, &1_u32.to_ne_bytes());
            put(&mut hello, 20, &44_u32.to_ne_bytes());
            put(&mut hello, 24, &0x3432_5241_u32.to_ne_bytes());
            put(&mut hello, 28, &1_u32.to_ne_bytes());
            put(&mut hello, 48, &8_u64.to_ne_bytes());
            put(&mut hello, 80, &8_u64.to_ne_bytes());
            put(&mut hello, 88, &8_u64.to_ne_bytes());
            let file = tempfile::tempfile().unwrap();
            let rights = [file.as_fd()];
            let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
            let mut ancillary = SendAncillaryBuffer::new(&mut space);
            assert!(ancillary.push(SendAncillaryMessage::ScmRights(&rights)));
            assert_eq!(
                sendmsg(
                    &producer,
                    &[IoSlice::new(&hello)],
                    &mut ancillary,
                    SendFlags::NOSIGNAL,
                )
                .unwrap(),
                HELLO_BYTES
            );
            assert_eq!(
                receive_packet(&producer, Duration::from_secs(1))
                    .unwrap()
                    .unwrap()
                    .message_type,
                HELLO_ACK
            );
            let request = receive_packet(&producer, Duration::from_secs(1))
                .unwrap()
                .unwrap();
            assert_eq!(request.message_type, REQUEST);
            send_packet(
                &producer,
                Packet {
                    message_type: STATUS,
                    sequence: request.sequence,
                    requests: 3,
                    captures: 2,
                    busy_drops: 1,
                    coalesced_drops: 4,
                    ..Packet::default()
                },
            )
            .unwrap();
            send_packet(
                &producer,
                Packet {
                    message_type: READY,
                    sequence: request.sequence,
                    request_ns: request.request_ns,
                    ..Packet::default()
                },
            )
            .unwrap();
            let ack = receive_packet(&producer, Duration::from_secs(1))
                .unwrap()
                .unwrap();
            assert_eq!(ack.message_type, ACK);
            assert_eq!(ack.sequence, request.sequence);
            send_packet(
                &producer,
                Packet {
                    message_type: STATUS,
                    sequence: request.sequence,
                    ..Packet::default()
                },
            )
            .unwrap();
        });

        let mut session =
            VulkanSession::start_with_importer(consumer, admission_listener, |contract, _| {
                assert_eq!(contract.width, 2);
                assert_eq!(contract.drm_fourcc, 0x3432_5241);
                assert_eq!(contract.allocation_size, 8);
                Ok(Box::new(FakeImage))
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let frame = loop {
            if let Some(frame) = session.take_latest() {
                break frame;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(frame.bytes.as_ref(), &[1; 8]);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match session.poll() {
                Err(VulkanSessionFailure::Disconnected) => break,
                Ok(()) => {}
                Err(error) => panic!("unexpected session terminal: {error:?}"),
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        let timing = session.timing();
        assert_eq!(timing.requests, 3);
        assert_eq!(timing.captures, 2);
        assert_eq!(timing.busy_drops, 1);
        assert_eq!(timing.coalesced_drops, 4);
        drop(session);
        producer_thread.join().unwrap();
    }
}
pub mod lifecycle;
pub mod listener;
pub mod protocol;
pub mod receiver;
