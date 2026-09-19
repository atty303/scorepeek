use std::collections::VecDeque;
use std::env;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufRead as _, BufReader, Read as _, Seek as _, SeekFrom, Write as _};
use std::net::Shutdown;
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _,
    PermissionsExt as _,
};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use serde_json::{Value, json};

pub const RECORD_SCHEMA: &str = "scorepeek-diagnostic-event-v1";
pub const HEADER_SCHEMA: &str = "scorepeek-diagnostic-stream-header-v1";
pub const REQUEST_SCHEMA: &str = "scorepeek-diagnostic-stream-request-v1";
pub const SOCKET_NAME: &str = "diagnostics.sock";
pub const STREAM_NAME: &str = "diagnostics.ndjson";
const ACTIVE_LOCK_NAME: &str = "active.lock";
pub const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub const RING_BYTES: usize = 128 * 1024 * 1024;
pub const ISOLATED_RING_BYTES: usize = 8 * 1024 * 1024;
pub const RETAINED_RUNS: usize = 10;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectionFormat {
    Human,
    Json,
    Ndjson,
}

const MAX_CLIENTS: usize = 8;
const MAX_REQUEST_BYTES: usize = 1024;
#[derive(Clone)]
pub struct DiagnosticSink {
    shared: Arc<Shared>,
}

pub struct RunDiagnostics {
    sink: DiagnosticSink,
    run_root: Option<PathBuf>,
    writer: Option<JoinHandle<()>>,
    server: Option<JoinHandle<()>>,
    active_lock: Option<File>,
    finished: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    run_id: String,
    records: VecDeque<Record>,
    ring_bytes: usize,
    ring_capacity_bytes: usize,
    next_sequence: u64,
    dropped_before_oldest: u64,
    persistence: Persistence,
    partial: bool,
    active: bool,
    started_at: Instant,
}

#[derive(Clone)]
struct Record {
    sequence: u64,
    bytes: Arc<[u8]>,
    observed_at: Instant,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Persistence {
    Active,
    Unavailable,
    Failed,
    Lagged,
}

impl Persistence {
    const fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::Lagged => "lagged",
        }
    }

    const fn partial(self) -> bool {
        !matches!(self, Self::Active)
    }
}

#[derive(Serialize)]
struct Envelope<'a> {
    schema: &'static str,
    run_id: &'a str,
    sequence: u64,
    observed_unix_us: u128,
    operation: &'a str,
    data: &'a Value,
}

impl RunDiagnostics {
    #[cfg(test)]
    #[must_use]
    pub(crate) fn start(store: &Path, run_id: &str) -> Self {
        Self::from_disk(run_id, prepare_run(store, run_id))
    }

    /// Starts one diagnostic run with an explicitly isolated persistence and runtime root.
    ///
    /// This is used by offline production-path consumers that must observe the same socket
    /// protocol without colliding with an active interactive run.
    #[must_use]
    #[allow(
        dead_code,
        reason = "the library entry point is consumed by corpus replay"
    )]
    pub fn start_at(store: &Path, runtime: &Path, run_id: &str) -> Self {
        Self::from_disk_at(run_id, prepare_run(store, run_id), Some(runtime), true)
    }

    /// Starts an isolated socket-only diagnostic run without persistent output.
    #[must_use]
    #[allow(
        dead_code,
        reason = "the library entry point is consumed by corpus replay"
    )]
    pub fn start_ephemeral_at(runtime: &Path, run_id: &str) -> Self {
        Self::from_disk_at(run_id, Err(String::new()), Some(runtime), false)
    }

    #[must_use]
    pub fn start_default(run_id: &str) -> Self {
        let disk = default_store().and_then(|store| prepare_run(&store, run_id));
        Self::from_disk(run_id, disk)
    }

    fn from_disk(run_id: &str, disk: Result<(PathBuf, File, File), String>) -> Self {
        Self::from_disk_at(run_id, disk, None, true)
    }

    fn from_disk_at(
        run_id: &str,
        disk: Result<(PathBuf, File, File), String>,
        runtime: Option<&Path>,
        report_persistence_error: bool,
    ) -> Self {
        let ring_capacity_bytes = if runtime.is_some() {
            ISOLATED_RING_BYTES
        } else {
            RING_BYTES
        };
        let (run_root, file, active_lock, persistence) = match disk {
            Ok((root, file, active_lock)) => (
                Some(root),
                Some(file),
                Some(active_lock),
                Persistence::Active,
            ),
            Err(error) => {
                if report_persistence_error {
                    eprintln!("scorepeek: diagnostic persistence unavailable: {error}");
                }
                (None, None, None, Persistence::Unavailable)
            }
        };
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                run_id: run_id.to_owned(),
                records: VecDeque::new(),
                ring_bytes: 0,
                ring_capacity_bytes,
                next_sequence: 1,
                dropped_before_oldest: 0,
                persistence,
                partial: persistence.partial(),
                active: true,
                started_at: Instant::now(),
            }),
            changed: Condvar::new(),
        });
        let sink = DiagnosticSink {
            shared: Arc::clone(&shared),
        };
        let writer = file.and_then(|file| match spawn_writer(Arc::clone(&shared), file) {
            Ok(writer) => Some(writer),
            Err(error) => {
                eprintln!("scorepeek: diagnostic persistence unavailable: {error}");
                if let Ok(mut state) = shared.state.lock() {
                    state.persistence = Persistence::Failed;
                    state.partial = true;
                }
                None
            }
        });
        let server = runtime
            .map_or_else(
                || start_server(Arc::clone(&shared)).map(|(_, server)| server),
                |runtime| {
                    start_server_at(Arc::clone(&shared), runtime).map(|(_, server)| Some(server))
                },
            )
            .unwrap_or_else(|error| {
                eprintln!("scorepeek: diagnostic socket unavailable: {error}");
                None
            });
        let diagnostics = Self {
            sink,
            run_root,
            writer,
            server,
            active_lock,
            finished: false,
        };
        diagnostics.sink.record(
            "diagnostic_run_started",
            &json!({
                "resource": {
                    "program": "scorepeek",
                    "version": env!("CARGO_PKG_VERSION"),
                    "process_id": std::process::id()
                },
                "ring_bytes": ring_capacity_bytes,
                "record_bytes": MAX_RECORD_BYTES,
                "retained_runs": RETAINED_RUNS,
                "persistence": persistence.name()
            }),
            true,
        );
        diagnostics
    }

    #[must_use]
    pub fn sink(&self) -> DiagnosticSink {
        self.sink.clone()
    }

    #[must_use]
    pub fn run_root(&self) -> Option<&Path> {
        self.run_root.as_deref()
    }

    pub fn finish(&mut self, operation_status: &str) {
        if self.finished {
            return;
        }
        self.sink.record(
            "diagnostic_run_finished",
            &json!({"status":operation_status}),
            true,
        );
        if let Ok(mut state) = self.sink.shared.state.lock() {
            state.active = false;
            self.sink.shared.changed.notify_all();
        }
        join_thread(&mut self.writer);
        join_thread(&mut self.server);
        if let Some(lock) = self.active_lock.take() {
            let _ = lock.unlock();
        }
        self.finished = true;
    }
}

impl Drop for RunDiagnostics {
    fn drop(&mut self) {
        if !self.finished {
            self.finish("error");
        }
    }
}

impl DiagnosticSink {
    pub fn record(&self, operation: &str, data: &Value, _important: bool) {
        let observed_unix_us = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros();
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sequence = state.next_sequence;
        let envelope = Envelope {
            schema: RECORD_SCHEMA,
            run_id: &state.run_id,
            sequence,
            observed_unix_us,
            operation,
            data,
        };
        let Ok(mut bytes) = serde_json::to_vec(&envelope) else {
            return;
        };
        bytes.push(b'\n');
        if bytes.len() > MAX_RECORD_BYTES {
            append_small_failure(&mut state, operation, bytes.len());
        } else {
            append(&mut state, Arc::from(bytes), Instant::now());
        }
        self.shared.changed.notify_all();
    }

    pub fn health(&self) -> Value {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        json!({
            "status": if state.partial { "degraded" } else { "ready" },
            "error_type": match state.persistence {
                Persistence::Active => None,
                Persistence::Unavailable => Some("startup_failed"),
                Persistence::Failed => Some("write_failed"),
                Persistence::Lagged => Some("writer_lagged"),
            },
            "oldest_sequence": oldest_sequence(&state),
            "next_sequence": state.next_sequence,
            "dropped_before_oldest": state.dropped_before_oldest,
            "ring_bytes": state.ring_bytes,
        })
    }
}

fn append_small_failure(state: &mut State, operation: &str, bytes: usize) {
    state.partial = true;
    let sequence = state.next_sequence;
    let value = json!({
        "schema": RECORD_SCHEMA,
        "run_id": state.run_id,
        "sequence": sequence,
        "observed_unix_us": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_micros(),
        "operation": "diagnostic_record_rejected",
        "data": {"error_type":"record_too_large", "source_operation":operation, "bytes":bytes}
    });
    if let Ok(mut bytes) = serde_json::to_vec(&value) {
        bytes.push(b'\n');
        append(state, Arc::from(bytes), Instant::now());
    }
}

fn append(state: &mut State, bytes: Arc<[u8]>, observed_at: Instant) {
    while state.ring_bytes.saturating_add(bytes.len()) > state.ring_capacity_bytes {
        let Some(removed) = state.records.pop_front() else {
            break;
        };
        state.ring_bytes = state.ring_bytes.saturating_sub(removed.bytes.len());
        state.dropped_before_oldest = removed.sequence.saturating_add(1);
    }
    let sequence = state.next_sequence;
    state.next_sequence = state.next_sequence.saturating_add(1);
    state.ring_bytes = state.ring_bytes.saturating_add(bytes.len());
    state.records.push_back(Record {
        sequence,
        bytes,
        observed_at,
    });
}

fn oldest_sequence(state: &State) -> u64 {
    state
        .records
        .front()
        .map_or(state.next_sequence, |record| record.sequence)
}

fn record_at(state: &State, sequence: u64) -> Option<&Record> {
    let offset = sequence.checked_sub(oldest_sequence(state))?;
    let index = usize::try_from(offset).ok()?;
    state
        .records
        .get(index)
        .filter(|record| record.sequence == sequence)
}

fn prepare_run(store: &Path, run_id: &str) -> Result<(PathBuf, File, File), String> {
    validate_run_id(run_id)?;
    crate::local_profiles::ensure_directory_tree(store)?;
    rotate(store)?;
    let run_root = store.join(run_id);
    DirBuilder::new()
        .mode(0o700)
        .create(&run_root)
        .map_err(|error| format!("diagnostic run directory could not be created: {error}"))?;
    File::open(store)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("diagnostic store could not be synced: {error}"))?;
    let active_lock = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(run_root.join(ACTIVE_LOCK_NAME))
        .and_then(|file| {
            file.try_lock()?;
            Ok(file)
        }) {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_dir(&run_root);
            return Err(format!(
                "diagnostic active lock could not be created: {error}"
            ));
        }
    };
    let file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(run_root.join(STREAM_NAME))
    {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_file(run_root.join(ACTIVE_LOCK_NAME));
            let _ = fs::remove_dir(&run_root);
            let _ = File::open(store).and_then(|directory| directory.sync_all());
            return Err(format!("diagnostic stream could not be created: {error}"));
        }
    };
    if let Err(error) = File::open(&run_root).and_then(|directory| directory.sync_all()) {
        drop(file);
        let _ = fs::remove_file(run_root.join(STREAM_NAME));
        let _ = fs::remove_file(run_root.join(ACTIVE_LOCK_NAME));
        let _ = fs::remove_dir(&run_root);
        let _ = File::open(store).and_then(|directory| directory.sync_all());
        return Err(format!(
            "diagnostic run directory could not be synced: {error}"
        ));
    }
    Ok((run_root, file, active_lock))
}

fn rotate(store: &Path) -> Result<(), String> {
    let mut runs = Vec::new();
    for entry in fs::read_dir(store)
        .map_err(|error| format!("diagnostic store could not be read: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("diagnostic entry could not be read: {error}"))?;
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|error| format!("diagnostic entry could not be inspected: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("diagnostic store contains an unexpected entry".to_owned());
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| "diagnostic run ID is invalid".to_owned())?;
        runs.push((
            run_order(&name)?,
            entry.path(),
            run_is_active(&entry.path())?,
        ));
    }
    runs.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    while runs.len() >= RETAINED_RUNS {
        let completed = runs
            .iter()
            .position(|(_, _, active)| !active)
            .ok_or_else(|| "diagnostic retention contains no completed run".to_owned())?;
        let (_, oldest, _) = runs.remove(completed);
        fs::remove_dir_all(&oldest)
            .map_err(|error| format!("old diagnostic run could not be removed: {error}"))?;
        File::open(store)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("diagnostic store could not be synced: {error}"))?;
    }
    Ok(())
}

fn spawn_writer(shared: Arc<Shared>, mut file: File) -> Result<JoinHandle<()>, String> {
    thread::Builder::new()
        .name("scorepeek-diagnostic-writer".to_owned())
        .spawn(move || {
            let mut cursor = 1_u64;
            loop {
                let record = {
                    let mut state = shared
                        .state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    loop {
                        let oldest = oldest_sequence(&state);
                        if cursor < oldest {
                            mark_persistence_failure(&mut state, Persistence::Lagged, "writer_lagged");
                            shared.changed.notify_all();
                            eprintln!("scorepeek: diagnostic persistence degraded: writer lagged behind the ring");
                            return;
                        }
                        if let Some(record) = record_at(&state, cursor).cloned() {
                            break record;
                        }
                        if !state.active && cursor >= state.next_sequence {
                            let _ = file.flush();
                            return;
                        }
                        state = shared
                            .changed
                            .wait_timeout(state, Duration::from_millis(50))
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .0;
                    }
                };
                let result = file.write_all(&record.bytes).and_then(|()| file.flush());
                if result.is_err() {
                    if let Ok(mut state) = shared.state.lock() {
                        mark_persistence_failure(&mut state, Persistence::Failed, "write_failed");
                        shared.changed.notify_all();
                    }
                    eprintln!("scorepeek: diagnostic persistence degraded: stream write failed");
                    return;
                }
                cursor = cursor.saturating_add(1);
            }
        })
        .map_err(|error| format!("diagnostic writer could not start: {error}"))
}

fn mark_persistence_failure(state: &mut State, persistence: Persistence, error_type: &str) {
    state.persistence = persistence;
    state.partial = true;
    let value = json!({
        "schema": RECORD_SCHEMA,
        "run_id": state.run_id,
        "sequence": state.next_sequence,
        "observed_unix_us": SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_micros(),
        "operation": "diagnostic_persistence_failed",
        "data": {"error_type":error_type}
    });
    if let Ok(mut bytes) = serde_json::to_vec(&value) {
        bytes.push(b'\n');
        append(state, Arc::from(bytes), Instant::now());
    }
}

fn start_server(shared: Arc<Shared>) -> Result<(Option<PathBuf>, Option<JoinHandle<()>>), String> {
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && !path.as_os_str().is_empty())
        .ok_or_else(|| "XDG_RUNTIME_DIR must be absolute and non-empty".to_owned())?;
    start_server_at(shared, &runtime).map(|(path, thread)| (Some(path), Some(thread)))
}

fn start_server_at(
    shared: Arc<Shared>,
    runtime: &Path,
) -> Result<(PathBuf, JoinHandle<()>), String> {
    let directory = runtime.join("scorepeek");
    crate::local_profiles::ensure_directory_tree(&directory)?;
    let path = directory.join(SOCKET_NAME);
    remove_stale_socket(&path)?;
    let listener = UnixListener::bind(&path)
        .map_err(|error| format!("diagnostic socket could not be bound: {error}"))?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("diagnostic socket permissions could not be set: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("diagnostic socket could not be made nonblocking: {error}"))?;
    let metadata = path
        .symlink_metadata()
        .map_err(|error| format!("diagnostic socket could not be inspected: {error}"))?;
    let identity = (metadata.dev(), metadata.ino());
    let thread_path = path.clone();
    let worker = thread::Builder::new()
        .name("scorepeek-diagnostic-socket".to_owned())
        .spawn(move || serve(&listener, &thread_path, identity, &shared))
        .map_err(|error| format!("diagnostic socket worker could not start: {error}"))?;
    Ok((path, worker))
}

fn remove_stale_socket(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_socket() => match UnixStream::connect(path) {
            Ok(_) => Err("diagnostic socket is already active".to_owned()),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path)
                .map_err(|error| format!("stale diagnostic socket could not be removed: {error}")),
            Err(error) => Err(format!(
                "diagnostic socket liveness could not be determined: {error}"
            )),
        },
        Ok(_) => Err("diagnostic socket path contains a non-socket entry".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "diagnostic socket path could not be inspected: {error}"
        )),
    }
}

struct Client {
    stream: UnixStream,
    request: Vec<u8>,
    initialized: bool,
    next_sequence: u64,
    pending: Arc<[u8]>,
    offset: usize,
    pending_record: bool,
}

fn serve(listener: &UnixListener, path: &Path, identity: (u64, u64), shared: &Shared) {
    let mut clients = Vec::new();
    let mut shutdown_started = None;
    loop {
        accept_clients(listener, shared, &mut clients);
        clients.retain_mut(|client| advance_client(client, shared));
        let active = shared.state.lock().is_ok_and(|state| state.active);
        if !active {
            let started = shutdown_started.get_or_insert_with(Instant::now);
            let next = shared.state.lock().map_or(0, |state| state.next_sequence);
            if clients
                .iter()
                .all(|client| client.next_sequence >= next && client.offset >= client.pending.len())
                || started.elapsed() >= Duration::from_millis(250)
            {
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    if let Ok(metadata) = path.symlink_metadata()
        && metadata.file_type().is_socket()
        && (metadata.dev(), metadata.ino()) == identity
    {
        let _ = fs::remove_file(path);
    }
}

fn accept_clients(listener: &UnixListener, shared: &Shared, clients: &mut Vec<Client>) {
    for _ in 0..MAX_CLIENTS {
        match listener.accept() {
            Ok((stream, _)) => {
                if clients.len() >= MAX_CLIENTS || stream.set_nonblocking(true).is_err() {
                    continue;
                }
                let state = shared
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let next_sequence = state.next_sequence;
                drop(state);
                clients.push(Client {
                    stream,
                    request: Vec::new(),
                    initialized: false,
                    next_sequence,
                    pending: Arc::from([]),
                    offset: 0,
                    pending_record: false,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

#[derive(Clone, Copy)]
struct StreamStart {
    sequence: u64,
    replay_seconds: Option<u64>,
    replay_available_us: u64,
    replay_truncated: bool,
}

fn select_stream_start(state: &State, replay_seconds: Option<u64>, now: Instant) -> StreamStart {
    let available = state.records.front().map_or_else(
        || now.saturating_duration_since(state.started_at),
        |record| now.saturating_duration_since(record.observed_at),
    );
    let replay_available_us = u64::try_from(available.as_micros()).unwrap_or(u64::MAX);
    let Some(seconds) = replay_seconds else {
        return StreamStart {
            sequence: state.next_sequence,
            replay_seconds: None,
            replay_available_us,
            replay_truncated: false,
        };
    };
    let cutoff = now.checked_sub(Duration::from_secs(seconds));
    let sequence = cutoff
        .and_then(|cutoff| {
            state
                .records
                .iter()
                .find(|record| record.observed_at >= cutoff)
                .map(|record| record.sequence)
        })
        .unwrap_or_else(|| cutoff.map_or_else(|| oldest_sequence(state), |_| state.next_sequence));
    let replay_truncated = state.dropped_before_oldest > 0
        && state
            .records
            .front()
            .is_some_and(|oldest| cutoff.is_none_or(|cutoff| oldest.observed_at > cutoff));
    StreamStart {
        sequence,
        replay_seconds: Some(seconds),
        replay_available_us,
        replay_truncated,
    }
}

fn encode_header(state: &State, start: StreamStart) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&json!({
        "schema": HEADER_SCHEMA,
        "run_id": state.run_id,
        "oldest_sequence": oldest_sequence(state),
        "stream_start_sequence": start.sequence,
        "next_sequence": state.next_sequence,
        "dropped_before_oldest": state.dropped_before_oldest,
        "gap": false,
        "active": state.active,
        "partial": state.partial,
        "persistence": state.persistence.name(),
        "replay_seconds": start.replay_seconds,
        "replay_available_us": start.replay_available_us,
        "replay_truncated": start.replay_truncated,
    }))
    .expect("diagnostic header is serializable");
    bytes.push(b'\n');
    bytes
}

fn advance_client(client: &mut Client, shared: &Shared) -> bool {
    if !client.initialized && !initialize_client(client, shared) {
        return false;
    }
    if !client.initialized {
        return true;
    }
    if client.offset < client.pending.len() {
        match client.stream.write(&client.pending[client.offset..]) {
            Ok(0) => return false,
            Ok(written) => client.offset += written,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                return true;
            }
            Err(_) => return false,
        }
        if client.offset < client.pending.len() {
            return true;
        }
        if client.pending_record {
            client.next_sequence = client.next_sequence.saturating_add(1);
        }
    }
    let state = shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let oldest = oldest_sequence(&state);
    if client.next_sequence < oldest {
        return false;
    }
    let Some(record) = record_at(&state, client.next_sequence) else {
        client.pending = Arc::from([]);
        client.offset = 0;
        client.pending_record = false;
        return true;
    };
    client.pending = Arc::clone(&record.bytes);
    client.offset = 0;
    client.pending_record = true;
    drop(state);
    advance_client(client, shared)
}

fn initialize_client(client: &mut Client, shared: &Shared) -> bool {
    let mut bytes = [0_u8; MAX_REQUEST_BYTES];
    match client.stream.read(&mut bytes) {
        Ok(0) => return false,
        Ok(read) => client.request.extend_from_slice(&bytes[..read]),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            return true;
        }
        Err(_) => return false,
    }
    if client.request.len() > MAX_REQUEST_BYTES {
        return false;
    }
    let Some(newline) = client.request.iter().position(|byte| *byte == b'\n') else {
        return true;
    };
    if newline + 1 != client.request.len() {
        return false;
    }
    let Ok(replay_seconds) = decode_request(&client.request[..newline]) else {
        return false;
    };
    let state = shared
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let start = select_stream_start(&state, replay_seconds, Instant::now());
    client.next_sequence = start.sequence;
    client.pending = Arc::from(encode_header(&state, start));
    client.offset = 0;
    client.pending_record = false;
    client.initialized = true;
    true
}

fn decode_request(bytes: &[u8]) -> Result<Option<u64>, ()> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let object = value.as_object().ok_or(())?;
    if object.get("schema").and_then(Value::as_str) != Some(REQUEST_SCHEMA) {
        return Err(());
    }
    match object.get("mode").and_then(Value::as_str) {
        Some("live") if object.len() == 2 => Ok(None),
        Some("replay") if object.len() == 3 => object
            .get("seconds")
            .and_then(Value::as_u64)
            .filter(|seconds| *seconds > 0)
            .map(Some)
            .ok_or(()),
        _ => Err(()),
    }
}

fn join_thread(thread: &mut Option<JoinHandle<()>>) {
    let Some(handle) = thread.take() else {
        return;
    };
    let _ = handle.join();
}

/// Prints a finite diagnostic snapshot and returns its semantic exit status.
///
/// # Errors
///
/// Returns an error when the selected run cannot be resolved or its stream is invalid.
pub fn inspect_latest(store: &Path, run_id: Option<&str>) -> Result<i32, String> {
    inspect_latest_with_format(store, run_id, InspectionFormat::Ndjson)
}

/// Prints a finite diagnostic snapshot in the requested presentation format and returns its
/// semantic exit status.
///
/// # Errors
///
/// Returns an error when the selected run cannot be resolved, its stream is invalid, or output
/// fails.
pub fn inspect_latest_with_format(
    store: &Path,
    run_id: Option<&str>,
    format: InspectionFormat,
) -> Result<i32, String> {
    let root = match run_id {
        Some(run_id) => {
            validate_run_id(run_id)?;
            store.join(run_id)
        }
        None => latest_run(store)?,
    };
    inspect_run(&root, format)
}

fn validate_run_id(run_id: &str) -> Result<(), String> {
    if run_id.is_empty()
        || run_id.len() > 128
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("diagnostic run ID is invalid".to_owned());
    }
    Ok(())
}

fn latest_run(store: &Path) -> Result<PathBuf, String> {
    let mut runs = Vec::new();
    for entry in fs::read_dir(store)
        .map_err(|error| format!("diagnostic store could not be read: {error}"))?
    {
        let entry =
            entry.map_err(|error| format!("diagnostic entry could not be read: {error}"))?;
        let metadata = entry
            .path()
            .symlink_metadata()
            .map_err(|error| format!("diagnostic entry could not be inspected: {error}"))?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| "diagnostic run ID is invalid".to_owned())?;
            let active = run_is_active(&entry.path())?;
            runs.push((active, run_order(&name)?, entry.path()));
        }
    }
    runs.into_iter()
        .max_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        })
        .map(|(_, _, path)| path)
        .ok_or_else(|| "no diagnostic run is available".to_owned())
}

fn run_order(run_id: &str) -> Result<(u64, u32, u32), String> {
    let mut parts = run_id
        .strip_prefix("run-")
        .ok_or_else(|| "diagnostic run ID is invalid".to_owned())?
        .split('-');
    let seconds = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "diagnostic run ID is invalid".to_owned())?;
    let nanos = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "diagnostic run ID is invalid".to_owned())?;
    let process = parts
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| "diagnostic run ID is invalid".to_owned())?;
    if parts.next().is_some() {
        return Err("diagnostic run ID is invalid".to_owned());
    }
    Ok((seconds, nanos, process))
}

#[allow(
    clippy::too_many_lines,
    reason = "the scan and exact snapshot copy share one file boundary and validation state"
)]
fn inspect_run(root: &Path, format: InspectionFormat) -> Result<i32, String> {
    let run_id = root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "diagnostic run ID is invalid".to_owned())?;
    let path = root.join(STREAM_NAME);
    validate_run_id(run_id)?;
    let mut file = File::open(&path)
        .map_err(|error| format!("diagnostic stream could not be opened: {error}"))?;
    let active = run_is_active(root)?;
    let snapshot_len = file
        .metadata()
        .map_err(|error| format!("diagnostic stream could not be inspected: {error}"))?
        .len();
    let tail = if snapshot_len == 0 {
        false
    } else {
        file.seek(SeekFrom::Start(snapshot_len - 1))
            .map_err(|error| error.to_string())?;
        let mut byte = [0];
        file.read_exact(&mut byte)
            .map_err(|error| error.to_string())?;
        byte[0] != b'\n'
    };
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut oldest = 1_u64;
    let mut next = 1_u64;
    let mut finished = false;
    let mut partial = false;
    let mut reader = BufReader::new((&mut file).take(snapshot_len));
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("diagnostic stream could not be read: {error}"))?;
        if read == 0 || line.last() != Some(&b'\n') {
            break;
        }
        if line.len() > MAX_RECORD_BYTES {
            return Err("diagnostic stream record is too large".to_owned());
        }
        let value: Value = serde_json::from_slice(&line)
            .map_err(|_| "diagnostic stream contains a malformed interior record".to_owned())?;
        let sequence = value
            .get("sequence")
            .and_then(Value::as_u64)
            .ok_or_else(|| "diagnostic stream record has no sequence".to_owned())?;
        if sequence != next {
            return Err("diagnostic stream sequence is not contiguous".to_owned());
        }
        if next == 1 {
            oldest = sequence;
        }
        finished = value["operation"] == "diagnostic_run_finished";
        partial |= matches!(
            value["operation"].as_str(),
            Some("diagnostic_record_rejected" | "diagnostic_persistence_failed")
        );
        next = next.saturating_add(1);
    }
    let header = json!({
        "schema": HEADER_SCHEMA,
        "run_id": run_id,
        "oldest_sequence": oldest,
        "next_sequence": next,
        "dropped_before_oldest": 0,
        "gap": false,
        "active": active,
        "partial": partial || (!active && (tail || !finished)),
        "tail_in_progress": tail && active,
        "source": "disk"
    });
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let complete_len = if tail {
        let mut scan = BufReader::new((&mut file).take(snapshot_len));
        let mut consumed = 0_u64;
        loop {
            line.clear();
            let read = scan
                .read_until(b'\n', &mut line)
                .map_err(|error| error.to_string())?;
            if read == 0 || line.last() != Some(&b'\n') {
                break;
            }
            consumed = consumed.saturating_add(read as u64);
        }
        consumed
    } else {
        snapshot_len
    };
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    match format {
        InspectionFormat::Ndjson => {
            serde_json::to_writer(&mut output, &header).map_err(|error| error.to_string())?;
            output.write_all(b"\n").map_err(|error| error.to_string())?;
            io::copy(&mut (&mut file).take(complete_len), &mut output)
                .map_err(|error| error.to_string())?;
        }
        InspectionFormat::Json => {
            output
                .write_all(b"{\"schema\":\"scorepeek-diagnostic-inspection-v1\",\"header\":")
                .map_err(|error| error.to_string())?;
            serde_json::to_writer(&mut output, &header).map_err(|error| error.to_string())?;
            output
                .write_all(b",\"records\":[")
                .map_err(|error| error.to_string())?;
            write_json_records(BufReader::new((&mut file).take(complete_len)), &mut output)?;
            output
                .write_all(b"]}\n")
                .map_err(|error| error.to_string())?;
        }
        InspectionFormat::Human => {
            writeln!(output, "scorepeek diagnostic inspection")
                .map_err(|error| error.to_string())?;
            writeln!(output, "  run: {run_id}").map_err(|error| error.to_string())?;
            writeln!(output, "  active: {active}").map_err(|error| error.to_string())?;
            writeln!(output, "  partial: {}", header["partial"])
                .map_err(|error| error.to_string())?;
            writeln!(output, "  records: {}", next.saturating_sub(oldest))
                .map_err(|error| error.to_string())?;
            writeln!(output, "events:").map_err(|error| error.to_string())?;
            write_human_records(BufReader::new((&mut file).take(complete_len)), &mut output)?;
        }
    }
    Ok(i32::from(partial || (!active && (tail || !finished))))
}

fn write_json_records(
    mut reader: impl io::BufRead,
    output: &mut impl io::Write,
) -> Result<(), String> {
    let mut first = true;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        if !first {
            output.write_all(b",").map_err(|error| error.to_string())?;
        }
        first = false;
        output
            .write_all(line.strip_suffix(b"\n").unwrap_or(&line))
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn write_human_records(
    mut reader: impl io::BufRead,
    output: &mut impl io::Write,
) -> Result<(), String> {
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        let value: Value = serde_json::from_slice(&line)
            .map_err(|_| "diagnostic stream contains a malformed interior record".to_owned())?;
        let sequence = value["sequence"].as_u64().unwrap_or_default();
        let operation = value["operation"].as_str().unwrap_or("unknown");
        write!(output, "  {sequence}: {operation}").map_err(|error| error.to_string())?;
        for key in ["stage", "status", "error_type"] {
            if let Some(detail) = value["data"][key].as_str() {
                write!(output, " {key}={detail}").map_err(|error| error.to_string())?;
            }
        }
        writeln!(output).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn run_is_active(root: &Path) -> Result<bool, String> {
    let path = root.join(ACTIVE_LOCK_NAME);
    let lock = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "diagnostic active lock could not be opened: {error}"
            ));
        }
    };
    match lock.try_lock() {
        Ok(()) => {
            let _ = lock.unlock();
            Ok(false)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(error) => Err(format!(
            "diagnostic active lock could not be inspected: {error}"
        )),
    }
}

/// Resolves the persistent diagnostic store from the XDG state convention.
///
/// # Errors
///
/// Returns an error when neither `XDG_STATE_HOME` nor `HOME` is available.
pub fn default_store() -> Result<PathBuf, String> {
    env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map(|state| state.join("scorepeek/diagnostics"))
        .ok_or_else(|| "XDG_STATE_HOME or HOME is required for diagnostics".to_owned())
}

/// Streams live diagnostics to stdout as NDJSON, optionally preceded by a bounded replay window.
///
/// # Errors
///
/// Returns an error for unavailable runtime state, connection failures, gaps, or malformed data.
pub fn observe(replay_seconds: Option<u64>) -> Result<(), String> {
    let runtime = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "XDG_RUNTIME_DIR is unavailable".to_owned())?;
    let mut stream = UnixStream::connect(runtime.join("scorepeek").join(SOCKET_NAME))
        .map_err(|error| format!("diagnostic socket could not be connected: {error}"))?;
    let request = replay_seconds.map_or_else(
        || json!({"schema":REQUEST_SCHEMA,"mode":"live"}),
        |seconds| json!({"schema":REQUEST_SCHEMA,"mode":"replay","seconds":seconds}),
    );
    serde_json::to_writer(&mut stream, &request)
        .map_err(|error| format!("diagnostic socket request failed: {error}"))?;
    stream
        .write_all(b"\n")
        .and_then(|()| stream.shutdown(Shutdown::Write))
        .map_err(|error| format!("diagnostic socket request failed: {error}"))?;
    let stdout = io::stdout();
    let stderr = io::stderr();
    copy_observation_stream(BufReader::new(stream), stdout.lock(), stderr.lock())
}

/// A validated, ordered client of one `diagnostics.sock` stream.
#[allow(
    dead_code,
    reason = "the library entry point is consumed by corpus replay"
)]
pub struct DiagnosticObserver {
    reader: BufReader<UnixStream>,
    expected: u64,
}

#[allow(
    dead_code,
    reason = "the library entry point is consumed by corpus replay"
)]
impl DiagnosticObserver {
    /// Connects to an explicitly selected runtime root and completes the stream handshake.
    ///
    /// Once this returns, live-only delivery is armed before the caller produces more records.
    ///
    /// # Errors
    ///
    /// Returns an error when the socket cannot be connected, the request cannot be sent, or the
    /// server returns an invalid or gapped stream header.
    pub fn connect_at(runtime: &Path, replay_seconds: Option<u64>) -> Result<Self, String> {
        let mut stream = UnixStream::connect(runtime.join("scorepeek").join(SOCKET_NAME))
            .map_err(|error| format!("diagnostic socket could not be connected: {error}"))?;
        let request = replay_seconds.map_or_else(
            || json!({"schema":REQUEST_SCHEMA,"mode":"live"}),
            |seconds| json!({"schema":REQUEST_SCHEMA,"mode":"replay","seconds":seconds}),
        );
        serde_json::to_writer(&mut stream, &request)
            .map_err(|error| format!("diagnostic socket request failed: {error}"))?;
        stream
            .write_all(b"\n")
            .and_then(|()| stream.shutdown(Shutdown::Write))
            .map_err(|error| format!("diagnostic socket request failed: {error}"))?;
        let mut reader = BufReader::new(stream);
        let (header, _) = read_observation_value(&mut reader)?
            .ok_or_else(|| "diagnostic socket returned no header".to_owned())?;
        if header["schema"] != HEADER_SCHEMA {
            return Err("diagnostic socket header schema is unsupported".to_owned());
        }
        if header["gap"] == true {
            return Err("diagnostic socket declared an initial sequence gap".to_owned());
        }
        let expected = header["stream_start_sequence"]
            .as_u64()
            .or_else(|| header["oldest_sequence"].as_u64())
            .ok_or_else(|| "diagnostic socket header has no start sequence".to_owned())?;
        Ok(Self { reader, expected })
    }

    /// Consumes validated records until the producer reports terminal completion.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed records, sequence gaps, early disconnect, or a callback
    /// failure.
    pub fn consume(
        mut self,
        mut observe: impl FnMut(Value) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut finished = false;
        while let Some((value, _)) = read_observation_value(&mut self.reader)? {
            let sequence = value["sequence"]
                .as_u64()
                .ok_or_else(|| "diagnostic socket record has no sequence".to_owned())?;
            if sequence != self.expected {
                return Err("diagnostic socket sequence gap".to_owned());
            }
            self.expected = sequence.saturating_add(1);
            finished = value["operation"] == "diagnostic_run_finished";
            observe(value)?;
        }
        if finished {
            Ok(())
        } else {
            Err(
                "diagnostic socket disconnected before the run finished; records may have been evicted"
                    .to_owned(),
            )
        }
    }
}

#[allow(
    dead_code,
    reason = "the library entry point is consumed by corpus replay"
)]
fn read_observation_value(
    reader: &mut impl std::io::BufRead,
) -> Result<Option<(Value, Vec<u8>)>, String> {
    let mut line = Vec::new();
    let read = reader
        .read_until(b'\n', &mut line)
        .map_err(|error| format!("diagnostic socket read failed: {error}"))?;
    if read == 0 {
        return Ok(None);
    }
    if line.last() != Some(&b'\n') || line.len() > MAX_RECORD_BYTES {
        return Err("diagnostic socket returned a malformed record".to_owned());
    }
    let value: Value = serde_json::from_slice(&line)
        .map_err(|_| "diagnostic socket returned invalid NDJSON".to_owned())?;
    Ok(Some((value, line)))
}

fn copy_observation_stream(
    mut reader: impl std::io::BufRead,
    mut output: impl std::io::Write,
    mut warning: impl std::io::Write,
) -> Result<(), String> {
    let mut line = Vec::new();
    let mut expected = None;
    let mut finished = false;
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("diagnostic socket read failed: {error}"))?;
        if read == 0 {
            break;
        }
        if line.last() != Some(&b'\n') || line.len() > MAX_RECORD_BYTES {
            return Err("diagnostic socket returned a malformed record".to_owned());
        }
        let value: Value = serde_json::from_slice(&line)
            .map_err(|_| "diagnostic socket returned invalid NDJSON".to_owned())?;
        if expected.is_none() {
            if value["schema"] != HEADER_SCHEMA {
                return Err("diagnostic socket header schema is unsupported".to_owned());
            }
            expected = value["stream_start_sequence"]
                .as_u64()
                .or_else(|| value["oldest_sequence"].as_u64());
            output
                .write_all(&line)
                .map_err(|error| format!("diagnostic output failed: {error}"))?;
            if value["replay_truncated"] == true {
                let requested = value["replay_seconds"].as_u64().unwrap_or(0);
                let available_us = value["replay_available_us"].as_u64().unwrap_or(0);
                writeln!(
                    warning,
                    "scorepeek: requested {requested}s diagnostic replay, but the ring retains only {:.3}s; streaming the available suffix",
                    Duration::from_micros(available_us).as_secs_f64()
                )
                .map_err(|error| format!("diagnostic warning output failed: {error}"))?;
            }
            if value["gap"] == true {
                return Err("diagnostic socket declared an initial sequence gap".to_owned());
            }
            continue;
        }
        let sequence = value["sequence"]
            .as_u64()
            .ok_or_else(|| "diagnostic socket record has no sequence".to_owned())?;
        if Some(sequence) != expected {
            return Err("diagnostic socket sequence gap".to_owned());
        }
        expected = Some(sequence.saturating_add(1));
        finished = value["operation"] == "diagnostic_run_finished";
        output
            .write_all(&line)
            .map_err(|error| format!("diagnostic output failed: {error}"))?;
    }
    if finished {
        Ok(())
    } else {
        Err(
            "diagnostic socket disconnected before the run finished; records may have been evicted"
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(run_id: &str, persistence: Persistence) -> Arc<Shared> {
        Arc::new(Shared {
            state: Mutex::new(State {
                run_id: run_id.to_owned(),
                records: VecDeque::new(),
                ring_bytes: 0,
                ring_capacity_bytes: RING_BYTES,
                next_sequence: 1,
                dropped_before_oldest: 0,
                persistence,
                partial: persistence.partial(),
                active: true,
                started_at: Instant::now(),
            }),
            changed: Condvar::new(),
        })
    }

    #[test]
    fn ring_evicts_complete_records_by_bytes() {
        let shared = shared("run-1", Persistence::Active);
        let sink = DiagnosticSink { shared };
        let payload = "x".repeat(MAX_RECORD_BYTES / 2);
        for _ in 0..300 {
            sink.record("test", &json!({"payload":payload}), false);
        }
        let state = sink.shared.state.lock().unwrap();
        assert!(state.ring_bytes <= RING_BYTES);
        assert!(state.dropped_before_oldest > 0);
        assert_eq!(
            state.records.front().unwrap().sequence,
            state.dropped_before_oldest
        );
    }

    #[test]
    fn oversized_record_is_replaced_by_a_bounded_partial_marker() {
        let shared = shared("run-1", Persistence::Active);
        let sink = DiagnosticSink { shared };
        sink.record(
            "test",
            &json!({"payload":"x".repeat(MAX_RECORD_BYTES)}),
            false,
        );
        let state = sink.shared.state.lock().unwrap();
        assert!(state.partial);
        assert_eq!(state.records.len(), 1);
        assert!(state.records[0].bytes.len() <= MAX_RECORD_BYTES);
        assert_eq!(
            serde_json::from_slice::<Value>(&state.records[0].bytes).unwrap()["operation"],
            "diagnostic_record_rejected"
        );
    }

    #[test]
    fn explicit_runtime_observer_arms_live_delivery_before_records() {
        let temporary = tempfile::tempdir().unwrap();
        let store = temporary.path().join("store");
        let runtime = temporary.path().join("runtime");
        let mut diagnostics = RunDiagnostics::start_at(&store, &runtime, "run-1-0-1");
        let observer = DiagnosticObserver::connect_at(&runtime, None).unwrap();
        diagnostics
            .sink()
            .record("run_event", &json!({"event":"test"}), false);
        diagnostics.finish("success");
        let mut operations = Vec::new();
        observer
            .consume(|record| {
                operations.push(record["operation"].as_str().unwrap().to_owned());
                Ok(())
            })
            .unwrap();
        assert_eq!(
            operations,
            ["run_event".to_owned(), "diagnostic_run_finished".to_owned()]
        );
    }

    #[test]
    fn ephemeral_runtime_observer_does_not_create_persistent_output() {
        let temporary = tempfile::tempdir().unwrap();
        let runtime = temporary.path().join("runtime");
        let mut diagnostics = RunDiagnostics::start_ephemeral_at(&runtime, "run-1-0-1");
        assert!(diagnostics.run_root().is_none());
        let observer = DiagnosticObserver::connect_at(&runtime, None).unwrap();
        diagnostics
            .sink()
            .record("run_event", &json!({"event":"test"}), false);
        diagnostics.finish("success");
        let mut operations = Vec::new();
        observer
            .consume(|record| {
                operations.push(record["operation"].as_str().unwrap().to_owned());
                Ok(())
            })
            .unwrap();
        assert_eq!(
            operations,
            ["run_event".to_owned(), "diagnostic_run_finished".to_owned()]
        );
        assert!(!temporary.path().join("store").exists());
    }

    #[test]
    fn sequence_lookup_is_offset_based_across_a_large_ring() {
        let shared = shared("run-1-0-1", Persistence::Active);
        let mut state = shared.state.lock().unwrap();
        for _ in 0..100_000 {
            append(&mut state, Arc::from(&b"{}\n"[..]), Instant::now());
        }
        assert_eq!(record_at(&state, 1).map(|record| record.sequence), Some(1));
        assert_eq!(
            record_at(&state, 50_000).map(|record| record.sequence),
            Some(50_000)
        );
        assert_eq!(
            record_at(&state, 100_000).map(|record| record.sequence),
            Some(100_000)
        );
        assert!(record_at(&state, 100_001).is_none());
    }

    #[test]
    fn observer_rejects_a_gap_declared_by_the_initial_header() {
        let header = serde_json::to_vec(&json!({
            "schema":HEADER_SCHEMA,
            "run_id":"run-1-0-1",
            "oldest_sequence":5,
            "next_sequence":8,
            "dropped_before_oldest":5,
            "gap":true,
            "active":true,
            "partial":false,
        }))
        .unwrap();
        let mut input = header.clone();
        input.push(b'\n');
        let mut output = Vec::new();
        let mut warning = Vec::new();
        let error = copy_observation_stream(std::io::Cursor::new(input), &mut output, &mut warning)
            .unwrap_err();
        assert!(error.contains("initial sequence gap"));
        assert_eq!(output, [header, vec![b'\n']].concat());
        assert!(warning.is_empty());
    }

    #[test]
    fn truncated_replay_is_streamed_with_a_warning() {
        let mut input = serde_json::to_vec(&json!({
            "schema":HEADER_SCHEMA,
            "run_id":"run-1-0-1",
            "oldest_sequence":5,
            "next_sequence":5,
            "dropped_before_oldest":5,
            "gap":false,
            "active":true,
            "partial":false,
            "replay_seconds":30,
            "replay_available_us":12_500_000,
            "replay_truncated":true,
        }))
        .unwrap();
        input.push(b'\n');
        input.extend_from_slice(
            &serde_json::to_vec(&json!({
                "sequence":5,
                "operation":"diagnostic_run_finished"
            }))
            .unwrap(),
        );
        input.push(b'\n');
        let mut output = Vec::new();
        let mut warning = Vec::new();
        copy_observation_stream(std::io::Cursor::new(input), &mut output, &mut warning).unwrap();
        let warning = String::from_utf8(warning).unwrap();
        assert!(warning.contains("requested 30s"));
        assert!(warning.contains("12.500s"));
    }

    #[test]
    fn stream_start_is_live_by_default_and_time_bounded_for_replay() {
        let shared = shared("run-1-0-1", Persistence::Active);
        let mut state = shared.state.lock().unwrap();
        let now = Instant::now();
        append(
            &mut state,
            Arc::from(&b"{}\n"[..]),
            now.checked_sub(Duration::from_secs(20)).unwrap(),
        );
        append(
            &mut state,
            Arc::from(&b"{}\n"[..]),
            now.checked_sub(Duration::from_secs(5)).unwrap(),
        );
        state.dropped_before_oldest = 1;
        let live = select_stream_start(&state, None, now);
        assert_eq!(live.sequence, state.next_sequence);
        assert!(!live.replay_truncated);
        let recent = select_stream_start(&state, Some(10), now);
        assert_eq!(recent.sequence, 2);
        assert!(!recent.replay_truncated);
        let unavailable = select_stream_start(&state, Some(30), now);
        assert_eq!(unavailable.sequence, 1);
        assert!(unavailable.replay_truncated);
    }

    #[test]
    fn inspect_returns_complete_records_before_an_incomplete_tail() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("run-1");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(STREAM_NAME), b"{\"sequence\":1}\n{\"sequence\":2").unwrap();
        assert_eq!(inspect_run(&root, InspectionFormat::Ndjson).unwrap(), 1);
    }

    #[test]
    fn inspect_rejects_an_ended_stream_without_a_terminal_record() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("run-1");
        fs::create_dir(&root).unwrap();
        fs::write(root.join(STREAM_NAME), b"{\"sequence\":1}\n").unwrap();
        assert_eq!(inspect_run(&root, InspectionFormat::Ndjson).unwrap(), 1);
    }

    #[test]
    fn disk_inspect_uses_the_run_lock_when_the_socket_is_unavailable() {
        let temporary = tempfile::tempdir().unwrap();
        let (root, mut stream, active_lock) = prepare_run(temporary.path(), "run-1-0-1").unwrap();
        stream
            .write_all(b"{\"sequence\":1,\"operation\":\"diagnostic_run_started\"}\n")
            .unwrap();
        stream.flush().unwrap();
        assert_eq!(inspect_run(&root, InspectionFormat::Ndjson).unwrap(), 0);
        active_lock.unlock().unwrap();
        assert_eq!(inspect_run(&root, InspectionFormat::Ndjson).unwrap(), 1);
    }

    #[test]
    fn rotation_keeps_nine_before_starting_the_tenth_active_run() {
        let temporary = tempfile::tempdir().unwrap();
        for index in 0..RETAINED_RUNS {
            fs::create_dir(temporary.path().join(format!("run-{index}-0-1"))).unwrap();
        }
        rotate(temporary.path()).unwrap();
        assert_eq!(
            fs::read_dir(temporary.path()).unwrap().count(),
            RETAINED_RUNS - 1
        );
    }

    #[test]
    fn writer_persists_every_record_without_video_recording() {
        let temporary = tempfile::tempdir().unwrap();
        let (root, file, _active_lock) = prepare_run(temporary.path(), "run-1-0-1").unwrap();
        let shared = shared("run-1-0-1", Persistence::Active);
        let sink = DiagnosticSink {
            shared: Arc::clone(&shared),
        };
        let writer = spawn_writer(Arc::clone(&shared), file).unwrap();
        sink.record("first", &json!({"video":false}), false);
        sink.record("last", &json!({}), true);
        {
            let mut state = shared.state.lock().unwrap();
            state.active = false;
            shared.changed.notify_all();
        }
        writer.join().unwrap();
        let bytes = fs::read(root.join(STREAM_NAME)).unwrap();
        let lines = bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            serde_json::from_slice::<Value>(lines[0]).unwrap()["operation"],
            "first"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(lines[1]).unwrap()["operation"],
            "last"
        );
    }

    #[test]
    fn socket_is_live_only_unless_replay_is_requested() {
        let temporary = tempfile::tempdir().unwrap();
        let shared = shared("run-1-0-1", Persistence::Unavailable);
        let sink = DiagnosticSink {
            shared: Arc::clone(&shared),
        };
        sink.record("before_connect", &json!({}), false);
        let (path, server) = start_server_at(Arc::clone(&shared), temporary.path()).unwrap();
        let mut stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .write_all(
                format!("{{\"schema\":\"{REQUEST_SCHEMA}\",\"mode\":\"live\"}}\n").as_bytes(),
            )
            .unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["schema"],
            HEADER_SCHEMA
        );
        sink.record("diagnostic_run_finished", &json!({"status":"test"}), true);
        {
            let mut state = shared.state.lock().unwrap();
            state.active = false;
            shared.changed.notify_all();
        }
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["operation"],
            "diagnostic_run_finished"
        );
        server.join().unwrap();
    }

    #[test]
    fn socket_replays_the_requested_time_window_before_live_records() {
        let temporary = tempfile::tempdir().unwrap();
        let shared = shared("run-1-0-1", Persistence::Unavailable);
        let sink = DiagnosticSink {
            shared: Arc::clone(&shared),
        };
        sink.record("before_connect", &json!({}), false);
        let (path, server) = start_server_at(Arc::clone(&shared), temporary.path()).unwrap();
        let mut stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream
            .write_all(
                format!("{{\"schema\":\"{REQUEST_SCHEMA}\",\"mode\":\"replay\",\"seconds\":30}}\n")
                    .as_bytes(),
            )
            .unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let header: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(header["replay_seconds"], 30);
        assert_eq!(header["replay_truncated"], false);
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["operation"],
            "before_connect"
        );
        sink.record("diagnostic_run_finished", &json!({"status":"test"}), true);
        {
            let mut state = shared.state.lock().unwrap();
            state.active = false;
            shared.changed.notify_all();
        }
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&line).unwrap()["operation"],
            "diagnostic_run_finished"
        );
        server.join().unwrap();
    }

    #[test]
    fn explicit_run_id_cannot_escape_the_store() {
        assert!(validate_run_id("../other").is_err());
        assert!(validate_run_id("run-1-2-3").is_ok());
    }
}
