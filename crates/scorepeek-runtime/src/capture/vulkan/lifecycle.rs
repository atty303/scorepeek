//! Public Vulkan capture session lifecycle around the asynchronous receiver.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use scorepeek_vulkan_capture::{ImageContract, ImportedImage};

use super::protocol::{
    ADMIT, ADMIT_ACK, HELLO_ACK, Packet, TransportError, receive_hello, receive_packet,
    send_packet, wait_readable,
};
use super::receiver::{
    CaptureImage, SessionState, VulkanFrameData, VulkanPerformanceSummary, VulkanTimingStats,
    WorkerTerminal, performance_summary, run_worker,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VulkanSessionFailure {
    Disconnected,
    Producer { status: i32 },
    Readback,
    Protocol,
}

pub struct VulkanSession {
    contract: ImageContract,
    state: Arc<Mutex<SessionState>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    last_summary: Instant,
}

impl VulkanSession {
    pub(super) fn start(
        socket: OwnedFd,
        admission_listener: OwnedFd,
    ) -> Result<Self, TransportError> {
        Self::start_with_importer(socket, admission_listener, |contract, dma_buf| {
            ImportedImage::import(contract, dma_buf)
                .map(|image| Box::new(image) as Box<dyn CaptureImage>)
        })
    }

    pub(super) fn start_with_importer(
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
