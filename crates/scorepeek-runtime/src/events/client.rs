use super::server::RunViewState;
use super::snapshot as event_api;
use std::env;
use std::fs::{self, DirBuilder};
use std::io::{self, Write as _};
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, PermissionsExt as _,
};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

pub(super) const MAX_CLIENTS: usize = 8;
pub(super) const EVENT_QUEUE_CAPACITY: usize = 64;
pub(super) const SOCKET_NAME: &str = "events.sock";

#[derive(Default)]
pub(super) struct ChannelHealth {
    pub(super) connected_clients: AtomicUsize,
    pub(super) dropped_events: AtomicU64,
    pub(super) disconnected_clients: AtomicU64,
    pub(super) server_failed: AtomicBool,
    pub(super) oversized_records: AtomicU64,
}

impl ChannelHealth {
    pub(super) fn value(&self) -> Value {
        json!({
            "status": if self.server_failed.load(Ordering::Acquire) { "degraded" } else { "ready" },
            "connected_clients": self.connected_clients.load(Ordering::Acquire),
            "dropped_events": self.dropped_events.load(Ordering::Acquire),
            "oversized_records": self.oversized_records.load(Ordering::Acquire),
            "error_type": if self.oversized_records.load(Ordering::Acquire) > 0 { Some("record_too_large") } else if self.server_failed.load(Ordering::Acquire) { Some("worker_unavailable") } else if self.dropped_events.load(Ordering::Acquire) > 0 { Some("queue_overflow") } else { None },
            "disconnected_clients": self.disconnected_clients.load(Ordering::Acquire),
        })
    }
}

pub(super) struct EventChannel {
    pub(super) sender: SyncSender<QueuedEvent>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) health: Arc<ChannelHealth>,
    pub(super) thread: Option<JoinHandle<()>>,
    pub(super) socket_path: PathBuf,
    pub(super) socket_identity: (u64, u64),
}

pub(super) struct SocketPathGuard {
    path: PathBuf,
    identity: (u64, u64),
    armed: bool,
}

impl SocketPathGuard {
    pub(super) fn new(path: PathBuf, identity: (u64, u64)) -> Self {
        Self {
            path,
            identity,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SocketPathGuard {
    fn drop(&mut self) {
        if self.armed {
            remove_owned_socket(&self.path, self.identity);
        }
    }
}

impl EventChannel {
    pub(super) fn start(state: Arc<Mutex<RunViewState>>) -> Result<Self, String> {
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && !path.as_os_str().is_empty())
            .ok_or_else(|| {
                "XDG_RUNTIME_DIR must be absolute and non-empty for scorepeek run".to_owned()
            })?;
        Self::start_at(&runtime, state)
    }

    pub(super) fn start_at(
        runtime: &Path,
        state: Arc<Mutex<RunViewState>>,
    ) -> Result<Self, String> {
        let directory = runtime.join("scorepeek");
        ensure_private_directory(&directory)?;
        let socket_path = directory.join(SOCKET_NAME);
        remove_stale_socket(&socket_path)?;
        let listener = UnixListener::bind(&socket_path)
            .map_err(|error| format!("event socket could not be bound: {error}"))?;
        let metadata = socket_path
            .symlink_metadata()
            .map_err(|error| format!("event socket could not be inspected: {error}"))?;
        let socket_identity = (metadata.dev(), metadata.ino());
        let mut path_guard = SocketPathGuard::new(socket_path.clone(), socket_identity);
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("event socket permissions could not be set: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("event socket could not be made nonblocking: {error}"))?;
        let (sender, receiver) = std::sync::mpsc::sync_channel::<QueuedEvent>(EVENT_QUEUE_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let health = Arc::new(ChannelHealth::default());
        let thread_stop = Arc::clone(&stop);
        let thread_health = Arc::clone(&health);
        let thread = thread::Builder::new()
            .name("scorepeek-event-socket".to_owned())
            .spawn(move || {
                let mut clients = Vec::new();
                loop {
                    if thread_health.server_failed.load(Ordering::Acquire) {
                        break;
                    }
                    prune_clients(&thread_health, &mut clients);
                    accept_clients(&listener, &state, &thread_health, &mut clients);
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(record) => broadcast(&record, &thread_health, &mut clients),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if thread_stop.load(Ordering::Acquire) {
                                break;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                retain_clients(&thread_health, &mut clients, |_| false);
            })
            .map_err(|error| format!("event socket worker could not start: {error}"))?;
        let channel = Self {
            sender,
            stop,
            health,
            thread: Some(thread),
            socket_path,
            socket_identity,
        };
        path_guard.disarm();
        Ok(channel)
    }

    pub(super) fn publish(&self, event: QueuedEvent) -> &'static str {
        try_send_event(&self.sender, &self.health, event)
    }
}

pub(super) struct QueuedEvent {
    pub(super) sequence: u64,
    pub(super) bytes: Vec<u8>,
}

pub(super) fn try_send_event(
    sender: &SyncSender<QueuedEvent>,
    health: &ChannelHealth,
    event: QueuedEvent,
) -> &'static str {
    match sender.try_send(event) {
        Ok(()) => "enqueued",
        Err(TrySendError::Full(_)) => {
            health.dropped_events.fetch_add(1, Ordering::AcqRel);
            "queue_full"
        }
        Err(TrySendError::Disconnected(_)) => {
            health.server_failed.store(true, Ordering::Release);
            "worker_unavailable"
        }
    }
}

impl Drop for EventChannel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            self.health.server_failed.store(true, Ordering::Release);
        }
        remove_owned_socket(&self.socket_path, self.socket_identity);
    }
}

fn remove_owned_socket(path: &Path, identity: (u64, u64)) {
    if let Ok(metadata) = path.symlink_metadata()
        && metadata.file_type().is_socket()
        && (metadata.dev(), metadata.ino()) == identity
    {
        let _ = fs::remove_file(path);
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err("event socket directory is not a directory".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(path)
                .map_err(|error| format!("event socket directory could not be created: {error}"))
        }
        Err(error) => Err(format!(
            "event socket directory could not be inspected: {error}"
        )),
    }
}

fn remove_stale_socket(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_socket() => match UnixStream::connect(path) {
            Ok(_) => Err("event socket is already active".to_owned()),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path)
                .map_err(|error| format!("stale event socket could not be removed: {error}")),
            Err(error) => Err(format!(
                "event socket liveness could not be determined: {error}"
            )),
        },
        Ok(_) => Err("event socket path contains a non-socket entry".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("event socket path could not be inspected: {error}")),
    }
}

pub(super) struct EventClient {
    pub(super) stream: UnixStream,
    pub(super) next_sequence: u64,
    pub(super) epoch: u64,
}

fn prune_clients(health: &ChannelHealth, clients: &mut Vec<EventClient>) {
    let epoch = health.dropped_events.load(Ordering::Acquire);
    retain_clients(health, clients, |client| {
        // On Linux AF_UNIX, a zero-byte send detects a closed peer without consuming requests
        // or mistaking a client's write-half shutdown for loss of its receiving side.
        client.epoch == epoch
            && match client.stream.write(&[]) {
                Ok(_) => true,
                Err(error) => matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ),
            }
    });
}

fn retain_clients(
    health: &ChannelHealth,
    clients: &mut Vec<EventClient>,
    mut keep: impl FnMut(&mut EventClient) -> bool,
) {
    let before = clients.len();
    clients.retain_mut(|client| keep(client));
    health
        .disconnected_clients
        .fetch_add((before - clients.len()) as u64, Ordering::AcqRel);
    health
        .connected_clients
        .store(clients.len(), Ordering::Release);
}

pub(super) fn accept_clients(
    listener: &UnixListener,
    state: &Arc<Mutex<RunViewState>>,
    health: &ChannelHealth,
    clients: &mut Vec<EventClient>,
) {
    // Bound admission work as well as established clients so a reconnect loop cannot starve delivery.
    for _ in 0..MAX_CLIENTS {
        match listener.accept() {
            Ok((mut stream, _)) => {
                prune_clients(health, clients);
                if clients.len() >= MAX_CLIENTS || stream.set_nonblocking(true).is_err() {
                    health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
                let Ok(state) = state.lock() else {
                    health.server_failed.store(true, Ordering::Release);
                    return;
                };
                if health.server_failed.load(Ordering::Acquire) {
                    return;
                }
                let Ok(bytes) = event_api::encode(&state.public) else {
                    health.oversized_records.fetch_add(1, Ordering::AcqRel);
                    health.server_failed.store(true, Ordering::Release);
                    return;
                };
                let next_sequence = state.public.next_sequence;
                let epoch = health.dropped_events.load(Ordering::Acquire);
                if stream.write_all(&bytes).is_err() {
                    health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
                clients.push(EventClient {
                    stream,
                    next_sequence,
                    epoch,
                });
                health
                    .connected_clients
                    .store(clients.len(), Ordering::Release);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => {
                health.server_failed.store(true, Ordering::Release);
                break;
            }
        }
    }
}

#[cfg(test)]
pub(super) fn snapshot_bytes(
    state: &Arc<Mutex<RunViewState>>,
    _health: &ChannelHealth,
) -> Option<Vec<u8>> {
    event_api::encode(&state.lock().ok()?.public).ok()
}

pub(super) fn broadcast(
    record: &QueuedEvent,
    health: &ChannelHealth,
    clients: &mut Vec<EventClient>,
) {
    let epoch = health.dropped_events.load(Ordering::Acquire);
    retain_clients(health, clients, |client| {
        if client.epoch != epoch {
            return false;
        }
        if record.sequence < client.next_sequence {
            return true;
        }
        if record.sequence != client.next_sequence {
            return false;
        }
        if client.stream.write_all(&record.bytes).is_err() {
            return false;
        }
        client.next_sequence += 1;
        true
    });
}
