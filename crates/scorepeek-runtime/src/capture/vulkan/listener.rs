//! Private Vulkan producer endpoint and admission lifecycle.

use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::time::Duration;

use rustix::fs::{Mode, chmod};
use rustix::io::dup;
use rustix::net::{
    AddressFamily, SendFlags, SocketAddrUnix, SocketFlags, SocketType, accept_with, bind, listen,
    send, socket_with,
};

use super::lifecycle::VulkanSession;
use super::protocol::{ERROR, Packet, TransportError, encode_packet, wait_readable};

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

pub struct VulkanListener {
    pub(super) fd: OwnedFd,
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
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
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
}

pub(super) fn reject_pending_fd(listener: &OwnedFd) -> Result<u64, String> {
    let mut rejected = 0_u64;
    loop {
        match accept_with(listener, SocketFlags::CLOEXEC | SocketFlags::NONBLOCK) {
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

impl Drop for VulkanListener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
