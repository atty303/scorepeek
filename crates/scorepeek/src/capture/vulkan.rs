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
const VERSION: u16 = 2;
const HELLO: u16 = 1;
const HELLO_ACK: u16 = 2;
const REQUEST: u16 = 3;
const READY: u16 = 4;
const ACK: u16 = 5;
const ERROR: u16 = 6;
const ADMIT: u16 = 7;
const ADMIT_ACK: u16 = 8;
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
    pub producer_fence_ns: u64,
    pub consumer_readback_ns: u64,
    pub total_ns: u64,
}

#[derive(Debug)]
enum WorkerTerminal {
    Disconnected,
    Failed(String),
}

trait CaptureImage: Send {
    fn readback(&mut self) -> Result<scorepeek_vulkan_capture::Readback, String>;
}

impl CaptureImage for ImportedImage {
    fn readback(&mut self) -> Result<scorepeek_vulkan_capture::Readback, String> {
        ImportedImage::readback(self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VulkanSessionFailure {
    Disconnected,
    Failed,
}

#[derive(Debug, Default)]
struct SessionState {
    latest: Option<VulkanFrameData>,
    timing: VulkanTimingStats,
    terminal: Option<WorkerTerminal>,
    latencies_ns: VecDeque<u64>,
    dropped_samples: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VulkanPerformanceSummary {
    pub count: u64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
    pub dropped: u64,
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
    pub fn accept(&self, timeout: Duration) -> Result<Option<VulkanSession>, String> {
        if !wait_readable(&self.fd, timeout)? {
            return Ok(None);
        }
        let socket = accept_with(&self.fd, SocketFlags::CLOEXEC)
            .map_err(|error| format!("accept Vulkan producer: {error}"))?;
        let admission_listener =
            dup(&self.fd).map_err(|error| format!("duplicate Vulkan admission socket: {error}"))?;
        match VulkanSession::start(socket, admission_listener) {
            Ok(session) => Ok(Some(session)),
            Err(error) if source_ended_during_handshake(&error) => Ok(None),
            Err(error) => Err(error),
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

fn source_ended_during_handshake(error: &str) -> bool {
    error.contains("Broken pipe") || error.contains("disconnected")
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
    fn start(socket: OwnedFd, admission_listener: OwnedFd) -> Result<Self, String> {
        Self::start_with_importer(socket, admission_listener, |contract, dma_buf| {
            ImportedImage::import(contract, dma_buf)
                .map(|image| Box::new(image) as Box<dyn CaptureImage>)
        })
    }

    fn start_with_importer(
        socket: OwnedFd,
        admission_listener: OwnedFd,
        importer: impl FnOnce(ImageContract, OwnedFd) -> Result<Box<dyn CaptureImage>, String>,
    ) -> Result<Self, String> {
        if !wait_readable(&socket, Duration::from_secs(2))? {
            return Err("Vulkan producer admission timed out".to_owned());
        }
        let admission = receive_packet(&socket, Duration::ZERO)?
            .ok_or_else(|| "Vulkan producer disconnected during admission".to_owned())?;
        if admission.message_type != ADMIT {
            return Err("invalid Vulkan admission request".to_owned());
        }
        send_packet(
            &socket,
            Packet {
                message_type: ADMIT_ACK,
                ..Packet::default()
            },
        )?;
        if !wait_readable(&socket, Duration::from_secs(2))? {
            return Err("Vulkan producer handshake timed out".to_owned());
        }
        let (contract, order, dma_buf) = receive_hello(&socket)?;
        let width = contract.width;
        let height = contract.height;
        let capture_image = importer(contract, dma_buf)?;
        send_packet(
            &socket,
            Packet {
                message_type: HELLO_ACK,
                ..Packet::default()
            },
        )?;
        let state = Arc::new(Mutex::new(SessionState::default()));
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
            .map_err(|error| format!("start Vulkan capture worker: {error}"))?;
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
            Some(WorkerTerminal::Failed(_)) => Err(VulkanSessionFailure::Failed),
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
        if let Err(error) = reject_busy_producers(&admission_listener) {
            set_terminal(&state, WorkerTerminal::Failed(error));
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
                set_terminal(&state, WorkerTerminal::Failed(error));
                return;
            }
            next_request = now + REQUEST_INTERVAL;
        }
        match receive_packet(&socket, Duration::from_millis(10)) {
            Ok(None) => {}
            Err(error) if error.contains("disconnected") => {
                set_terminal(&state, WorkerTerminal::Disconnected);
                return;
            }
            Err(error) => {
                set_terminal(&state, WorkerTerminal::Failed(error));
                return;
            }
            Ok(Some(packet)) if packet.message_type == READY => {
                let readback = match imported.readback() {
                    Ok(value) => value,
                    Err(error) => {
                        set_terminal(&state, WorkerTerminal::Failed(error));
                        return;
                    }
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
                    current.latest = Some(VulkanFrameData {
                        width,
                        height,
                        sequence: packet.sequence,
                        received_monotonic_ns: consumer_done_ns,
                        bytes,
                    });
                    current.timing = VulkanTimingStats {
                        requests: packet.requests,
                        captures: packet.captures,
                        busy_drops: packet.busy_drops,
                        coalesced_drops: packet.coalesced_drops,
                        request_to_present_ns: packet.present_ns.saturating_sub(packet.request_ns),
                        producer_submit_ns: packet.submit_done_ns.saturating_sub(packet.present_ns),
                        producer_fence_ns: packet
                            .fence_done_ns
                            .saturating_sub(packet.submit_done_ns),
                        consumer_readback_ns: readback.fence_ns.saturating_sub(readback.submit_ns),
                        total_ns: consumer_done_ns.saturating_sub(packet.request_ns),
                    };
                    let latency = consumer_done_ns.saturating_sub(packet.request_ns);
                    if current.latencies_ns.len() == 600 {
                        current.latencies_ns.pop_front();
                        current.dropped_samples = current.dropped_samples.saturating_add(1);
                    }
                    current.latencies_ns.push_back(latency);
                }
                if let Err(error) = send_packet(
                    &socket,
                    Packet {
                        message_type: ACK,
                        sequence: packet.sequence,
                        ..Packet::default()
                    },
                ) {
                    set_terminal(&state, WorkerTerminal::Failed(error));
                    return;
                }
            }
            Ok(Some(packet)) if packet.message_type == ERROR => {
                set_terminal(
                    &state,
                    WorkerTerminal::Failed(format!(
                        "Vulkan producer capture error {}",
                        packet.status
                    )),
                );
                return;
            }
            Ok(Some(_)) => {
                set_terminal(
                    &state,
                    WorkerTerminal::Failed("invalid Vulkan packet".to_owned()),
                );
                return;
            }
        }
    }
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

fn receive_hello(socket: &OwnedFd) -> Result<(ImageContract, VulkanPixelOrder, OwnedFd), String> {
    let mut bytes = [0_u8; HELLO_BYTES];
    let mut io = [IoSliceMut::new(&mut bytes)];
    let mut control_space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = RecvAncillaryBuffer::new(&mut control_space);
    let received = recvmsg(socket, &mut io, &mut control, RecvFlags::CMSG_CLOEXEC)
        .map_err(|error| format!("receive Vulkan hello: {error}"))?;
    if received.bytes == PACKET_BYTES
        && u32_at(&bytes, 0) == MAGIC
        && u16_at(&bytes, 4) == VERSION
        && u16_at(&bytes, 6) == ERROR
    {
        return Err(format!(
            "Vulkan producer capture error {}",
            i32::from_ne_bytes(bytes[88..92].try_into().expect("fixed slice"))
        ));
    }
    if received.bytes != HELLO_BYTES
        || u32_at(&bytes, 0) != MAGIC
        || u16_at(&bytes, 4) != VERSION
        || u16_at(&bytes, 6) != HELLO
        || u32_at(&bytes, 8) as usize != HELLO_BYTES
    {
        return Err("invalid Vulkan hello".to_owned());
    }
    let dma_buf = control
        .drain()
        .find_map(|message| match message {
            RecvAncillaryMessage::ScmRights(mut rights) => rights.next(),
            _ => None,
        })
        .ok_or_else(|| "Vulkan hello did not include one DMA-BUF".to_owned())?;
    let plane_count = u32_at(&bytes, 28);
    if plane_count == 0 || plane_count as usize > MAX_PLANES {
        return Err("invalid Vulkan plane count".to_owned());
    }
    let vk_format = u32_at(&bytes, 20);
    let order = match vk_format {
        37 | 43 => VulkanPixelOrder::Rgba,
        44 | 50 => VulkanPixelOrder::Bgra,
        _ => return Err(format!("unsupported Vulkan format {vk_format}")),
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
    fence_done_ns: u64,
    requests: u64,
    captures: u64,
    busy_drops: u64,
    coalesced_drops: u64,
    status: i32,
}

fn receive_packet(socket: &OwnedFd, timeout: Duration) -> Result<Option<Packet>, String> {
    if !wait_readable(socket, timeout)? {
        return Ok(None);
    }
    let mut bytes = [0_u8; PACKET_BYTES];
    let (count, _) = recv(socket, &mut bytes, RecvFlags::empty())
        .map_err(|error| format!("receive Vulkan packet: {error}"))?;
    if count == 0 {
        return Err("Vulkan producer disconnected".to_owned());
    }
    if count != PACKET_BYTES || u32_at(&bytes, 0) != MAGIC || u16_at(&bytes, 4) != VERSION {
        return Err("invalid Vulkan packet".to_owned());
    }
    Ok(Some(Packet {
        message_type: u16_at(&bytes, 6),
        sequence: u64_at(&bytes, 16),
        request_ns: u64_at(&bytes, 24),
        present_ns: u64_at(&bytes, 32),
        submit_done_ns: u64_at(&bytes, 40),
        fence_done_ns: u64_at(&bytes, 48),
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
    let mut values = current.latencies_ns.iter().copied().collect::<Vec<_>>();
    values.sort_unstable();
    let percentile = |numerator: usize| {
        if values.is_empty() {
            0
        } else {
            values[(values.len().saturating_sub(1) * numerator) / 100]
        }
    };
    VulkanPerformanceSummary {
        count: values.len() as u64,
        p50_ns: percentile(50),
        p95_ns: percentile(95),
        p99_ns: percentile(99),
        max_ns: values.last().copied().unwrap_or(0),
        dropped: current
            .dropped_samples
            .saturating_add(current.timing.busy_drops)
            .saturating_add(current.timing.coalesced_drops),
    }
}

fn send_packet(socket: &OwnedFd, packet: Packet) -> Result<(), String> {
    let bytes = encode_packet(packet);
    let count = send(socket, &bytes, SendFlags::NOSIGNAL)
        .map_err(|error| format!("send Vulkan packet: {error}"))?;
    if count != PACKET_BYTES {
        return Err("short Vulkan packet send".to_owned());
    }
    Ok(())
}

fn encode_packet(packet: Packet) -> [u8; PACKET_BYTES] {
    let mut bytes = [0_u8; PACKET_BYTES];
    put(&mut bytes, 0, &MAGIC.to_ne_bytes());
    put(&mut bytes, 4, &VERSION.to_ne_bytes());
    put(&mut bytes, 6, &packet.message_type.to_ne_bytes());
    put(&mut bytes, 16, &packet.sequence.to_ne_bytes());
    put(&mut bytes, 24, &packet.request_ns.to_ne_bytes());
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

    #[test]
    fn packet_wire_size_and_offsets_match_layer_contract() {
        let bytes = encode_packet(Packet {
            message_type: REQUEST,
            sequence: 42,
            request_ns: 99,
            ..Packet::default()
        });
        assert_eq!(bytes.len(), 96);
        assert_eq!(u16_at(&bytes, 4), VERSION);
        assert_eq!(u16_at(&bytes, 6), REQUEST);
        assert_eq!(u64_at(&bytes, 16), 42);
        assert_eq!(u64_at(&bytes, 24), 99);

        let admission = encode_packet(Packet {
            message_type: ADMIT,
            ..Packet::default()
        });
        assert_eq!(u16_at(&admission, 6), ADMIT);
    }

    #[test]
    fn explicit_layer_manifest_is_valid_and_points_to_the_installed_library() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../native/vulkan-capture/layer/VkLayer_SCOREPEEK_capture.json"
        ))
        .unwrap();
        assert_eq!(manifest["file_format_version"], "1.2.0");
        assert_eq!(manifest["layer"]["name"], "VK_LAYER_SCOREPEEK_capture");
        assert_eq!(manifest["layer"]["type"], "GLOBAL");
        assert_eq!(
            manifest["layer"]["library_path"],
            "../../../lib/libscorepeek_vulkan_capture.so"
        );
        assert_eq!(manifest["layer"]["implementation_version"], "1");
    }

    #[test]
    fn disappearing_pre_admission_producer_is_not_an_incompatible_contract() {
        assert!(source_ended_during_handshake(
            "send Vulkan packet: Broken pipe (os error 32)"
        ));
        assert!(source_ended_during_handshake(
            "Vulkan producer disconnected"
        ));
        assert!(!source_ended_during_handshake(
            "unsupported Vulkan format 99"
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
            "Vulkan producer capture error 6"
        );
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
        drop(session);
        producer_thread.join().unwrap();
    }
}
