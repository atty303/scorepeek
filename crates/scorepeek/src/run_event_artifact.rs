use std::fs::{DirBuilder, File, OpenOptions};
use std::io::{BufWriter, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest as _, Sha256};

const QUEUE_CAPACITY: usize = 256;
const MAX_RECORDS: usize = 250_000;
const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BYTES: u64 = 512 * 1024 * 1024;
const FINISH_TIMEOUT: Duration = Duration::from_secs(5);

enum Message {
    Record(Vec<u8>),
    Finish {
        dropped: u64,
        response: SyncSender<FinishOutcome>,
    },
}

pub struct RunEventArtifactWorker {
    sender: Option<SyncSender<Message>>,
    root: PathBuf,
    dropped: u64,
    startup_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct FinishOutcome {
    pub root: PathBuf,
    pub manifest_sha256: Option<String>,
    pub complete: bool,
    pub dropped: u64,
    pub error: Option<String>,
}

#[derive(Serialize)]
struct Manifest<'a> {
    schema: &'static str,
    run_id: &'a str,
    status: &'static str,
    events_sha256: String,
    event_count: usize,
    event_bytes: u64,
    dropped_events: u64,
}

impl RunEventArtifactWorker {
    pub(crate) fn start_at(root: PathBuf, run_id: &str) -> Self {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
        let worker_root = root.clone();
        let worker_run_id = run_id.to_owned();
        let spawned = thread::Builder::new()
            .name("scorepeek-run-event-writer".to_owned())
            .spawn(move || run_worker(&receiver, &worker_root, &worker_run_id, &startup_sender));
        let startup = match spawned {
            Ok(_) => startup_receiver
                .recv_timeout(FINISH_TIMEOUT)
                .unwrap_or_else(|_| Err("run event writer startup timed out".to_owned())),
            Err(error) => Err(format!("run event writer could not start: {error}")),
        };
        let (sender, startup_error) = match startup {
            Ok(()) => (Some(sender), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            sender,
            root,
            dropped: 0,
            startup_error,
        }
    }

    pub fn try_record<T: Serialize>(&mut self, event: &T) {
        let Some(sender) = &self.sender else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        let Ok(mut bytes) = serde_json::to_vec(event) else {
            self.dropped = self.dropped.saturating_add(1);
            return;
        };
        bytes.push(b'\n');
        if bytes.len() > MAX_RECORD_BYTES {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        match sender.try_send(Message::Record(bytes)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => self.dropped = self.dropped.saturating_add(1),
            Err(TrySendError::Disconnected(_)) => {
                self.sender = None;
                self.dropped = self.dropped.saturating_add(1);
            }
        }
    }

    pub fn finish(mut self) -> FinishOutcome {
        let Some(sender) = self.sender.take() else {
            return unavailable(self.root, self.dropped, self.startup_error.take());
        };
        let (response_sender, response_receiver) = mpsc::sync_channel(1);
        let deadline = Instant::now() + FINISH_TIMEOUT;
        let mut message = Message::Finish {
            dropped: self.dropped,
            response: response_sender,
        };
        loop {
            match sender.try_send(message) {
                Ok(()) => break,
                Err(TrySendError::Full(returned)) if Instant::now() < deadline => {
                    message = returned;
                    thread::yield_now();
                }
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                    return unavailable(
                        self.root,
                        self.dropped,
                        Some("run event writer finish could not be queued".to_owned()),
                    );
                }
            }
        }
        drop(sender);
        response_receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|_| {
                unavailable(
                    self.root,
                    self.dropped,
                    Some("run event writer finish timed out".to_owned()),
                )
            })
    }
}

fn unavailable(root: PathBuf, dropped: u64, error: Option<String>) -> FinishOutcome {
    FinishOutcome {
        root,
        manifest_sha256: None,
        complete: false,
        dropped,
        error,
    }
}

fn run_worker(
    receiver: &Receiver<Message>,
    root: &Path,
    run_id: &str,
    startup: &SyncSender<Result<(), String>>,
) {
    let file = match create_root(root).and_then(|()| open_create_only(&root.join("events.ndjson")))
    {
        Ok(file) => file,
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    let _ = startup.send(Ok(()));
    let mut writer = BufWriter::new(file);
    let mut hasher = Sha256::new();
    let mut count = 0usize;
    let mut bytes = 0u64;
    let mut failed = false;
    let mut worker_dropped = 0u64;
    while let Ok(message) = receiver.recv() {
        match message {
            Message::Record(record) => {
                let next = bytes.saturating_add(u64::try_from(record.len()).unwrap_or(u64::MAX));
                if failed || count >= MAX_RECORDS || next > MAX_STREAM_BYTES {
                    failed = true;
                    worker_dropped = worker_dropped.saturating_add(1);
                    continue;
                }
                if writer.write_all(&record).is_err() {
                    failed = true;
                    worker_dropped = worker_dropped.saturating_add(1);
                    continue;
                }
                hasher.update(&record);
                count += 1;
                bytes = next;
            }
            Message::Finish { dropped, response } => {
                let dropped = dropped.saturating_add(worker_dropped);
                let stream_ok =
                    !failed && writer.flush().is_ok() && writer.get_ref().sync_all().is_ok();
                let digest = encode_digest(hasher.finalize());
                let complete = stream_ok && dropped == 0;
                let manifest = Manifest {
                    schema: "scorepeek-run-event-artifact-v1",
                    run_id,
                    status: if complete { "complete" } else { "partial" },
                    events_sha256: digest,
                    event_count: count,
                    event_bytes: bytes,
                    dropped_events: dropped,
                };
                let manifest_sha256 = write_manifest(root, &manifest).ok();
                let _ = response.send(FinishOutcome {
                    root: root.to_owned(),
                    complete: complete && manifest_sha256.is_some(),
                    manifest_sha256,
                    dropped,
                    error: None,
                });
                return;
            }
        }
    }
}

fn create_root(root: &Path) -> Result<(), String> {
    let parent = root
        .parent()
        .ok_or_else(|| "event root has no parent".to_owned())?;
    DirBuilder::new()
        .mode(0o700)
        .create(root)
        .map_err(|error| format!("event root creation failed: {error}"))?;
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("event root parent sync failed: {error}"))
}

fn open_create_only(path: &Path) -> Result<File, String> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("event stream creation failed: {error}"))
}

fn write_manifest(root: &Path, manifest: &Manifest<'_>) -> Result<String, String> {
    let mut bytes = serde_json::to_vec(manifest)
        .map_err(|error| format!("event manifest encode failed: {error}"))?;
    bytes.push(b'\n');
    let digest = encode_digest(Sha256::digest(&bytes));
    let mut file = open_create_only(&root.join("manifest.json"))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("event manifest publication failed: {error}"))?;
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("event root sync failed: {error}"))?;
    Ok(digest)
}

fn encode_digest(digest: impl AsRef<[u8]>) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in digest.as_ref() {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::RunEventArtifactWorker;

    #[test]
    fn retains_ordered_events_and_publishes_a_complete_manifest() {
        let temporary = tempfile::tempdir().unwrap();
        let mut worker = RunEventArtifactWorker::start_at(temporary.path().join("events"), "run-1");
        worker.try_record(&serde_json::json!({"sequence":1}));
        worker.try_record(&serde_json::json!({"sequence":2}));
        let outcome = worker.finish();
        assert!(outcome.complete);
        assert_eq!(outcome.dropped, 0);
        assert!(outcome.manifest_sha256.is_some());
        assert_eq!(
            std::fs::read_to_string(outcome.root.join("events.ndjson")).unwrap(),
            "{\"sequence\":1}\n{\"sequence\":2}\n"
        );
    }
}
