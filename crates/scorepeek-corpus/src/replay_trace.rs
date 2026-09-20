use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use scorepeek::routine_output::{RunEvent, RunEventKind};
use serde::Serialize;

fn retained(event: &RunEvent) -> bool {
    !matches!(event.kind, RunEventKind::FieldObservation { .. })
}

const MAX_BYTES: u64 = 256 * 1024 * 1024;
const WRITER_QUEUE_EVENTS: usize = 64;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TraceStatus {
    path: PathBuf,
    written_events: usize,
    total_events: usize,
    bytes: u64,
    error: Option<String>,
}

pub(crate) struct ReplayTrace {
    root: PathBuf,
    generation: String,
    remaining: Arc<AtomicU64>,
    initialization_error: Option<String>,
    executable_sha256: Option<String>,
    sessions: BTreeMap<usize, SessionTrace>,
    #[cfg(test)]
    writer_gate: Option<(usize, TestWriterGate)>,
}

struct SessionTrace {
    sender: Option<SyncSender<RunEvent>>,
    status: Arc<Mutex<TraceStatus>>,
    writer: Option<JoinHandle<()>>,
}

pub(crate) struct TraceFinisher {
    sender: Option<SyncSender<RunEvent>>,
    status: Arc<Mutex<TraceStatus>>,
    writer: Option<JoinHandle<()>>,
}

impl TraceFinisher {
    pub(crate) fn finish(mut self) -> TraceStatus {
        self.sender.take();
        if let Some(writer) = self.writer.take()
            && writer.join().is_err()
        {
            set_error(&self.status, "replay trace writer panicked".to_owned());
        }
        self.status
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl ReplayTrace {
    pub(crate) fn new(root: PathBuf, generation: &str) -> Self {
        let setup = fs::create_dir(&root)
            .map_err(|error| error.to_string())
            .and_then(|()| {
                #[cfg(target_os = "linux")]
                let executable = Ok(PathBuf::from("/proc/self/exe"));
                #[cfg(not(target_os = "linux"))]
                let executable = std::env::current_exe().map_err(|error| error.to_string());
                executable.and_then(|path| {
                    crate::frame_corpus::digest_file(&path).map_err(|error| error.to_string())
                })
            });
        let (executable_sha256, initialization_error) = match setup {
            Ok(digest) => (Some(digest), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            root,
            generation: generation.to_owned(),
            remaining: Arc::new(AtomicU64::new(MAX_BYTES)),
            initialization_error,
            executable_sha256,
            sessions: BTreeMap::new(),
            #[cfg(test)]
            writer_gate: None,
        }
    }

    pub(crate) fn start_session(&mut self, index: usize, session: &str) -> Result<(), String> {
        if self.sessions.contains_key(&index) {
            return Err("replay trace session is already active".to_owned());
        }
        let status = Arc::new(Mutex::new(TraceStatus {
            path: self.root.join(format!("session-{index}.ndjson")),
            written_events: 0,
            total_events: 0,
            bytes: 0,
            error: self.initialization_error.clone(),
        }));
        let mut sender = None;
        let mut writer = None;
        if self.initialization_error.is_none() {
            let (candidate_sender, receiver) = mpsc::sync_channel(WRITER_QUEUE_EVENTS);
            let writer_status = Arc::clone(&status);
            let writer_remaining = Arc::clone(&self.remaining);
            let generation = self.generation.clone();
            let session = session.to_owned();
            let executable_sha256 = self.executable_sha256.clone();
            #[cfg(test)]
            let gate = self
                .writer_gate
                .as_ref()
                .and_then(|(selected, gate)| (*selected == index).then(|| gate.clone()));
            let spawn = thread::Builder::new()
                .name(format!("scorepeek-replay-trace-{index}"))
                .spawn(move || {
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        gate.wait();
                    }
                    write_session(
                        receiver,
                        &writer_status,
                        &writer_remaining,
                        &generation,
                        &session,
                        executable_sha256.as_deref(),
                    );
                });
            match spawn {
                Ok(handle) => {
                    sender = Some(candidate_sender);
                    writer = Some(handle);
                }
                Err(error) => set_error(&status, error.to_string()),
            }
        }
        self.sessions.insert(
            index,
            SessionTrace {
                sender,
                status,
                writer,
            },
        );
        Ok(())
    }

    pub(crate) fn observe(&mut self, index: usize, event: &RunEvent) -> Result<(), String> {
        if !retained(event) {
            return Ok(());
        }
        let session = self
            .sessions
            .get_mut(&index)
            .ok_or_else(|| "replay trace session is not active".to_owned())?;
        {
            let mut status = session
                .status
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            status.total_events = status.total_events.saturating_add(1);
            if status.error.is_some() {
                return Ok(());
            }
        }
        let Some(sender) = session.sender.as_ref() else {
            return Ok(());
        };
        let error = match sender.try_send(event.clone()) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(_)) => "replay trace writer queue capacity exceeded",
            Err(TrySendError::Disconnected(_)) => "replay trace writer disconnected",
        };
        set_error(&session.status, error.to_owned());
        session.sender.take();
        Ok(())
    }

    pub(crate) fn finish_session(&mut self, index: usize) -> Result<TraceFinisher, String> {
        let session = self
            .sessions
            .remove(&index)
            .ok_or_else(|| "replay trace session is not active".to_owned())?;
        Ok(TraceFinisher {
            sender: session.sender,
            status: session.status,
            writer: session.writer,
        })
    }

    #[cfg(test)]
    pub(crate) fn block_writer(&mut self, index: usize) -> TestWriterGate {
        let gate = TestWriterGate::new();
        self.writer_gate = Some((index, gate.clone()));
        gate
    }

    #[cfg(test)]
    pub(crate) fn active_session_count(&self) -> usize {
        self.sessions.len()
    }
}

fn write_session(
    receiver: Receiver<RunEvent>,
    status: &Mutex<TraceStatus>,
    remaining: &AtomicU64,
    generation: &str,
    session: &str,
    executable_sha256: Option<&str>,
) {
    let path = status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .path
        .clone();
    let mut file = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(file) => Some(file),
        Err(error) => {
            set_error(status, error.to_string());
            None
        }
    };
    if let Some(opened) = file.as_mut() {
        let metadata = serde_json::json!({
            "schema": "scorepeek-private-replay-trace-v1",
            "generation_sha256": generation,
            "session_id": session,
            "executable_sha256": executable_sha256,
            "selected_sources_sha256": crate::frame_corpus::digest(concat!(
                include_str!("../../scorepeek/src/routine_output.rs"),
                include_str!("../../scorepeek/src/routine_output/music_select_best.rs"),
                include_str!("../../scorepeek/src/recognition.rs")
            ).as_bytes()),
            "integrated_layout_sha256": crate::frame_corpus::digest(include_bytes!("../../scorepeek/src/integrated-context-layout-v8.json")),
            "best_layout_sha256": crate::frame_corpus::digest(include_bytes!("../../scorepeek/src/music-select-best-layout-v1.json")),
            "numeric_manifest_sha256": scorepeek::recognition::NUMERIC_MODEL_MANIFEST_SHA256,
            "text_manifest_sha256": scorepeek::recognition::LIVE_MODEL_BUNDLE_MANIFEST_SHA256,
            "run_event_schema": scorepeek::routine_output::RUN_EVENT_SCHEMA,
        });
        match write_line(remaining, opened, &metadata) {
            Ok(bytes) => add_bytes(status, bytes),
            Err(error) => {
                set_error(status, error.to_string());
                file = None;
            }
        }
    }
    for event in receiver {
        if has_error(status) {
            continue;
        }
        let Some(opened) = file.as_mut() else {
            continue;
        };
        match write_line(remaining, opened, &event) {
            Ok(bytes) => {
                let mut current = status
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                current.bytes = current.bytes.saturating_add(bytes);
                current.written_events = current.written_events.saturating_add(1);
            }
            Err(error) => {
                set_error(status, error.to_string());
                file = None;
            }
        }
    }
    if let Some(file) = file
        && let Err(error) = file.sync_all()
    {
        set_error(status, error.to_string());
    }
}

fn write_line(
    remaining: &AtomicU64,
    file: &mut impl Write,
    value: &impl Serialize,
) -> io::Result<u64> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    let size = bytes.len() as u64;
    remaining
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |available| {
            available.checked_sub(size)
        })
        .map_err(|_| io::Error::other("replay trace capacity exceeded"))?;
    // Reserve before writing: a partial write still consumes the shared run budget.
    file.write_all(&bytes)?;
    Ok(size)
}

fn set_error(status: &Mutex<TraceStatus>, error: String) {
    let mut status = status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if status.error.is_none() {
        status.error = Some(error);
    }
}

fn has_error(status: &Mutex<TraceStatus>) -> bool {
    status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .error
        .is_some()
}

fn add_bytes(status: &Mutex<TraceStatus>, bytes: u64) {
    let mut status = status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    status.bytes = status.bytes.saturating_add(bytes);
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestWriterGate(Arc<(Mutex<bool>, std::sync::Condvar)>);

#[cfg(test)]
impl TestWriterGate {
    fn new() -> Self {
        Self(Arc::new((Mutex::new(false), std::sync::Condvar::new())))
    }

    fn wait(&self) {
        let (released, changed) = &*self.0;
        let mut released = released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*released {
            released = changed
                .wait(released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub(crate) fn release(&self) {
        let (released, changed) = &*self.0;
        *released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watcher_started(sequence: usize) -> RunEvent {
        RunEvent {
            schema: scorepeek::routine_output::RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::WatcherStarted {
                invocation_id: format!("replay-{sequence}"),
            },
        }
    }

    #[test]
    fn trace_never_overwrites_and_capacity_is_shared() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("trace");
        let mut trace = ReplayTrace::new(root.clone(), "generation");
        trace.start_session(0, "session").unwrap();
        trace.observe(0, &watcher_started(0)).unwrap();
        let first = trace.finish_session(0).unwrap().finish();
        assert!(first.error.is_none());
        assert_eq!(first.written_events, 1);
        assert_eq!(first.total_events, 1);
        let original = fs::read(&first.path).unwrap();
        let header: serde_json::Value = serde_json::from_slice(
            original
                .split(|byte| *byte == b'\n')
                .next()
                .expect("trace header exists"),
        )
        .unwrap();
        assert_eq!(header["executable_sha256"].as_str().unwrap().len(), 64);
        assert_eq!(header["best_layout_sha256"].as_str().unwrap().len(), 64);
        trace.start_session(0, "session").unwrap();
        assert!(trace.finish_session(0).unwrap().finish().error.is_some());
        assert_eq!(original, fs::read(&first.path).unwrap());
        trace.remaining.store(0, Ordering::Relaxed);
        trace.start_session(1, "session").unwrap();
        assert!(
            trace
                .finish_session(1)
                .unwrap()
                .finish()
                .error
                .unwrap()
                .contains("capacity")
        );
        let mut duplicate = ReplayTrace::new(root, "generation");
        duplicate.start_session(2, "session").unwrap();
        assert!(
            duplicate
                .finish_session(2)
                .unwrap()
                .finish()
                .error
                .is_some()
        );
    }

    #[test]
    fn stalled_writer_is_bounded_and_does_not_block_other_sessions() {
        let temp = tempfile::tempdir().unwrap();
        let mut trace = ReplayTrace::new(temp.path().join("trace"), "generation");
        let gate = trace.block_writer(0);
        trace.start_session(0, "stalled").unwrap();
        for sequence in 0..=WRITER_QUEUE_EVENTS {
            trace.observe(0, &watcher_started(sequence)).unwrap();
        }

        trace.start_session(1, "independent").unwrap();
        trace.observe(1, &watcher_started(0)).unwrap();
        let independent = trace.finish_session(1).unwrap().finish();
        assert!(independent.error.is_none());
        assert_eq!(independent.written_events, 1);

        gate.release();
        let stalled = trace.finish_session(0).unwrap().finish();
        assert_eq!(stalled.total_events, WRITER_QUEUE_EVENTS + 1);
        assert!(stalled.error.unwrap().contains("writer queue capacity"));
    }
}
