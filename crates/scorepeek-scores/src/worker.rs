use crate::{Error, Store};
use serde::Serialize;
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};

const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_QUEUE_BYTES: usize = 8 * 1024 * 1024;
const QUEUE_RECORDS: usize = 64;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// A host-owned diagnostic sample; no exporter or output sink is installed by this crate.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Health {
    pub accepted: u64,
    pub committed: u64,
    pub duplicates: u64,
    pub rejected: u64,
    pub pending: u64,
    pub queued_bytes: usize,
    pub last_committed_event_id: Option<String>,
    pub failure: Option<String>,
    pub cause: Option<String>,
    pub flush: Option<String>,
}
impl Health {
    fn fail(&mut self, kind: &str, cause: &(impl ToString + ?Sized)) {
        if self.failure.is_none() {
            self.failure = Some(kind.to_owned());
            self.cause = Some(cause.to_string());
        }
    }
}
struct Message {
    bytes: Vec<u8>,
    received_unix_ms: u64,
    event_id: String,
}

/// Bounded, non-blocking consumer. Dropping it attempts a bounded drain.
pub struct Worker {
    sender: Option<SyncSender<Message>>,
    health: Arc<Mutex<Health>>,
    done: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    /// Starts initialization on the worker; initialization failure is reported through health.
    #[must_use]
    pub fn start(path: &Path) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Message>(QUEUE_RECORDS);
        let (done_sender, done) = mpsc::channel();
        let health = Arc::new(Mutex::new(Health::default()));
        let worker_health = Arc::clone(&health);
        let path = path.to_owned();
        let spawn = thread::Builder::new()
            .name("scorepeek-scores".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(|| run(&path, &receiver, &worker_health));
                if outcome.is_err() {
                    worker_health
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .fail("worker_panicked", "scores worker panicked");
                }
                let _ = done_sender.send(());
            });
        let thread = match spawn {
            Ok(thread) => Some(thread),
            Err(error) => {
                health
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .fail("worker_start", &error);
                None
            }
        };
        Self {
            sender: Some(sender),
            health,
            done,
            thread,
        }
    }

    /// Offers a public event without waiting for `SQLite`. Unrelated event kinds are ignored.
    pub fn offer(&self, bytes: &[u8]) {
        if bytes.len() > MAX_RECORD_BYTES {
            self.reject("record_limit", "event exceeds 1 MiB");
            return;
        }
        let header: serde_json::Value = match serde_json::from_slice(bytes) {
            Ok(value) => value,
            Err(error) => {
                self.reject("event_contract", &error);
                return;
            }
        };
        if !matches!(
            header["event"].as_str(),
            Some("result_detected" | "music_select_best_observed")
        ) || (header["event"] == "music_select_best_observed" && header["snapshot"].is_null())
        {
            return;
        }
        let mut health = self
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if health.failure.is_some() {
            health.rejected += 1;
            return;
        }
        let Some(sender) = &self.sender else {
            health.rejected += 1;
            return;
        };
        if health.queued_bytes.saturating_add(bytes.len()) > MAX_QUEUE_BYTES {
            health.rejected += 1;
            health.fail("queue_limit", "scores queue byte limit reached");
            return;
        }
        let Some(received_unix_ms) = unix_ms() else {
            health.rejected += 1;
            health.fail("clock", "system clock cannot represent Unix milliseconds");
            return;
        };
        let message = Message {
            bytes: bytes.to_vec(),
            received_unix_ms,
            event_id: header["event_id"].as_str().unwrap_or_default().to_owned(),
        };
        match sender.try_send(message) {
            Ok(()) => {
                health.accepted += 1;
                health.pending += 1;
                health.queued_bytes += bytes.len();
            }
            Err(TrySendError::Full(_)) => {
                health.rejected += 1;
                health.fail("queue_limit", "scores queue record limit reached");
            }
            Err(TrySendError::Disconnected(_)) => {
                health.rejected += 1;
                health.fail("worker_stopped", "scores worker disconnected");
            }
        }
    }
    /// Marks an event that could not reach the consumer as unsaved.
    pub fn reject(&self, kind: &str, cause: &(impl ToString + ?Sized)) {
        let mut health = self
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        health.rejected += 1;
        health.fail(kind, cause);
    }
    #[must_use]
    pub fn health(&self) -> Health {
        self.health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Stops admission and waits at most two seconds. Pending commits are not claimed as saved.
    pub fn finish(&mut self) -> Health {
        self.sender.take();
        if self.thread.is_some() {
            match self.done.recv_timeout(FLUSH_TIMEOUT) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if let Some(thread) = self.thread.take() {
                        let _ = thread.join();
                    }
                    let mut health = self
                        .health
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    health.flush = Some(
                        if health.pending == 0 {
                            "drained"
                        } else {
                            "incomplete"
                        }
                        .to_owned(),
                    );
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.thread.take();
                    let mut health = self
                        .health
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    health.fail("flush_timeout", "scores drain exceeded two seconds");
                    health.flush = Some("timeout".into());
                }
            }
        }
        self.health()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.finish();
    }
}

fn run(path: &Path, receiver: &Receiver<Message>, health: &Mutex<Health>) {
    let mut store = match Store::open(path) {
        Ok(store) => store,
        Err(error) => {
            health
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .fail("database_open", &error);
            return;
        }
    };
    while let Ok(message) = receiver.recv() {
        {
            let mut health = health
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            health.queued_bytes = health.queued_bytes.saturating_sub(message.bytes.len());
            if health.flush.as_deref() == Some("timeout") {
                return;
            }
        }
        let outcome = store.consume(&message.bytes, message.received_unix_ms);
        let mut health = health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match outcome {
            Ok(changed) => {
                health.pending -= 1;
                if changed {
                    health.committed += 1;
                    health.last_committed_event_id = Some(message.event_id);
                } else {
                    health.duplicates += 1;
                }
            }
            Err(error) => {
                let kind = match error {
                    Error::Json(_) | Error::UnsupportedContract => "event_contract",
                    _ => "database_write",
                };
                health.fail(kind, &error);
                return;
            }
        }
    }
}
pub(crate) fn unix_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|v| u64::try_from(v.as_millis()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event() -> Vec<u8> {
        serde_json::to_vec(&json!({"event":"result_detected","event_id":"run:1"})).unwrap()
    }
    #[test]
    fn queue_limits_stop_admission_without_blocking() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let (_done_sender, done) = mpsc::channel();
        let worker = Worker {
            sender: Some(sender),
            health: Arc::new(Mutex::new(Health::default())),
            done,
            thread: None,
        };
        worker.offer(&event());
        worker.offer(&event());
        worker.offer(&event());
        let health = worker.health();
        assert_eq!(health.accepted, 1);
        assert_eq!(health.pending, 1);
        assert_eq!(health.rejected, 2);
        assert_eq!(health.failure.as_deref(), Some("queue_limit"));
        assert!(health.queued_bytes <= MAX_QUEUE_BYTES);
    }
    #[test]
    fn drain_timeout_reports_pending_work_and_does_not_wait_for_worker_forever() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::channel();
        let (done_sender, done) = mpsc::channel();
        let handle = thread::spawn(move || {
            wait.recv().unwrap();
            done_sender.send(()).unwrap();
        });
        let mut worker = Worker {
            sender: Some(sender),
            health: Arc::new(Mutex::new(Health::default())),
            done,
            thread: Some(handle),
        };
        worker.offer(&event());
        let health = worker.finish();
        assert_eq!(health.flush.as_deref(), Some("timeout"));
        assert_eq!(health.pending, 1);
        release.send(()).unwrap();
        worker.done.recv_timeout(Duration::from_secs(1)).unwrap();
    }
}
