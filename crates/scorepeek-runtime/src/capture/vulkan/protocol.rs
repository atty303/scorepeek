//! Vulkan producer wire protocol and DMA-BUF admission codec.

use std::io::IoSliceMut;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, poll};
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendFlags, recv, recvmsg, send,
};
use rustix::time::Timespec;
use scorepeek_vulkan_capture::{ImageContract, MAX_PLANES, PlaneLayout};

pub(super) const MAGIC: u32 = 0x4b56_5053;
pub(super) const VERSION: u16 = 5;
pub(super) const HELLO: u16 = 1;
pub(super) const HELLO_ACK: u16 = 2;
pub(super) const REQUEST: u16 = 3;
pub(super) const READY: u16 = 4;
pub(super) const ACK: u16 = 5;
pub(super) const ERROR: u16 = 6;
pub(super) const ADMIT: u16 = 7;
pub(super) const ADMIT_ACK: u16 = 8;
pub(super) const STATUS: u16 = 9;
pub(super) const HELLO_BYTES: usize = 232;
pub(super) const PACKET_BYTES: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VulkanPixelOrder {
    Bgra,
    Rgba,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum TransportError {
    Disconnected,
    Producer { status: i32 },
    Import(String),
    Protocol(String),
    Transport(String),
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Packet {
    pub(super) message_type: u16,
    pub(super) sequence: u64,
    pub(super) request_ns: u64,
    pub(super) present_ns: u64,
    pub(super) submit_done_ns: u64,
    pub(super) present_call_ns: u64,
    pub(super) producer_fence_ns: u64,
    pub(super) requests: u64,
    pub(super) captures: u64,
    pub(super) busy_drops: u64,
    pub(super) coalesced_drops: u64,
    pub(super) status: i32,
}

pub(super) fn receive_hello(
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

pub(super) fn receive_packet(
    socket: &OwnedFd,
    timeout: Duration,
) -> Result<Option<Packet>, TransportError> {
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

pub(super) fn send_packet(socket: &OwnedFd, packet: Packet) -> Result<(), TransportError> {
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

pub(super) fn transport_error(operation: &str, error: rustix::io::Errno) -> TransportError {
    if matches!(
        error,
        rustix::io::Errno::PIPE | rustix::io::Errno::CONNRESET | rustix::io::Errno::NOTCONN
    ) {
        TransportError::Disconnected
    } else {
        TransportError::Transport(format!("{operation}: {error}"))
    }
}

pub(super) fn encode_packet(packet: Packet) -> [u8; PACKET_BYTES] {
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

pub(super) fn wait_readable(fd: &OwnedFd, timeout: Duration) -> Result<bool, String> {
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

pub(super) fn put(target: &mut [u8], offset: usize, bytes: &[u8]) {
    target[offset..offset + bytes.len()].copy_from_slice(bytes);
}

pub(super) fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_ne_bytes(bytes[offset..offset + 2].try_into().expect("fixed slice"))
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("fixed slice"))
}

pub(super) fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().expect("fixed slice"))
}
