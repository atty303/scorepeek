//! Vulkan producer admission, protocol handling, and canonical readback session.

use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::time::{ClockId, clock_gettime};
use scorepeek_vulkan_capture::ImportedImage;

#[cfg(test)]
use super::lifecycle::{VulkanSession, VulkanSessionFailure};
#[cfg(test)]
use super::listener::VulkanListener;
use super::listener::reject_pending_fd;
#[allow(clippy::wildcard_imports)]
use super::protocol::*;

const REQUEST_INTERVAL: Duration = Duration::from_millis(100);

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
pub(super) enum WorkerTerminal {
    Disconnected,
    Producer { status: i32 },
    Readback,
    Protocol,
}

pub(super) trait CaptureImage: Send {
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

#[derive(Debug, Default)]
pub(super) struct SessionState {
    pub(super) latest: Option<VulkanFrameData>,
    pub(super) timing: VulkanTimingStats,
    pub(super) readback_profile: scorepeek_vulkan_capture::ReadbackProfile,
    pub(super) terminal: Option<WorkerTerminal>,
    pub(super) timing_samples: VecDeque<VulkanTimingStats>,
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

#[allow(
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the capture thread must own every resource and keep its protocol loop together"
)]
pub(super) fn run_worker(
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
        if reject_pending_fd(&admission_listener).is_err() {
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

pub(super) fn performance_summary(state: &Mutex<SessionState>) -> VulkanPerformanceSummary {
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::io::dup;
    use rustix::net::{
        AddressFamily, SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketFlags,
        SocketType, sendmsg, socketpair,
    };
    use std::fs;
    use std::io::IoSlice;
    use std::mem::MaybeUninit;
    use std::os::fd::AsFd as _;
    use std::os::unix::fs::PermissionsExt as _;
    use std::thread;

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
