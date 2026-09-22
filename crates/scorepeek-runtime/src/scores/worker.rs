use scorepeek_scores::{Error, Store};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};

use super::health::{ChartIdentity, Completion, CompletionOutcome, Health};

const MAX_RECORD_BYTES: usize = 1024 * 1024;
const MAX_QUEUE_BYTES: usize = 8 * 1024 * 1024;
const QUEUE_RECORDS: usize = 64;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

struct Message {
    bytes: Vec<u8>,
    received_unix_ms: u64,
    event_id: String,
    chart: Option<ChartIdentity>,
}

/// Bounded, non-blocking consumer. Dropping it attempts a bounded drain.
pub struct Worker {
    sender: Option<SyncSender<Message>>,
    health: Arc<Mutex<Health>>,
    done: Receiver<()>,
    completions: Receiver<Completion>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    /// Starts initialization on the worker; initialization failure is reported through health.
    #[must_use]
    pub fn start(path: &Path) -> Self {
        let (sender, receiver) = mpsc::sync_channel::<Message>(QUEUE_RECORDS);
        let (done_sender, done) = mpsc::channel();
        let (completion_sender, completions) = mpsc::channel();
        let health = Arc::new(Mutex::new(Health::default()));
        let worker_health = Arc::clone(&health);
        let path = path.to_owned();
        let spawn = thread::Builder::new()
            .name("scorepeek-scores".into())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(|| {
                    run(&path, &receiver, &worker_health, &completion_sender);
                });
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
            completions,
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
            Some("result_changed" | "music_select_best_observed")
        ) || (header["event"] == "music_select_best_observed" && header["snapshot"].is_null())
            || (header["event"] == "result_changed" && header["state"]["status"] == "inactive")
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
            chart: chart_identity(&header),
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

    #[must_use]
    pub fn take_completions(&self) -> Vec<Completion> {
        self.completions.try_iter().collect()
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

fn run(
    path: &Path,
    receiver: &Receiver<Message>,
    health: &Mutex<Health>,
    completions: &mpsc::Sender<Completion>,
) {
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
    health
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .recovered_provisional = store.recovered_provisional_count();
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
                    health.last_committed_event_id = Some(message.event_id.clone());
                } else {
                    health.duplicates += 1;
                }
                let _ = completions.send(Completion {
                    event_id: message.event_id,
                    outcome: CompletionOutcome::Persisted,
                    chart: message.chart,
                });
            }
            Err(error) => {
                let kind = match error {
                    Error::Json(_) | Error::UnsupportedContract => "event_contract",
                    _ => "database_write",
                };
                health.fail(kind, &error);
                let _ = completions.send(Completion {
                    event_id: message.event_id,
                    outcome: CompletionOutcome::Failed,
                    chart: message.chart,
                });
                return;
            }
        }
    }
}

fn chart_identity(event: &serde_json::Value) -> Option<ChartIdentity> {
    let chart = match event["event"].as_str()? {
        "result_changed" => &event["state"]["result"],
        "music_select_best_observed" => &event["snapshot"]["chart"],
        _ => return None,
    };
    Some(ChartIdentity {
        scorepeek_song_id: chart["scorepeek_song_id"].as_str()?.to_owned(),
        play_type: chart["play_type"].as_str()?.to_owned(),
        difficulty: chart["difficulty"].as_str()?.to_owned(),
    })
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
        serde_json::to_vec(&json!({"event":"result_changed","event_id":"run:1","state":{"status":"provisional","result":{}}})).unwrap()
    }
    fn result(sequence: u64, score: u32) -> Vec<u8> {
        serde_json::to_vec(&json!({"schema":"scorepeek-event-v4","invocation_id":"run-a","sequence":sequence,"event_id":format!("run-a:{sequence}"),"emitted_monotonic_ms":sequence,"emitted_unix_ms":1000+sequence,"capture":{"session_id":"session","capture_generation":1,"binding":null},"event":"result_changed","source_sequence":sequence,"state":{"status":"provisional","song":{"scorepeek_song_id":"song-a","display_titles":["Synthetic song"],"artist":"Synthetic artist"},"result":{"contract":"scorepeek-result-detected-v4","attempt_id":sequence,"scorepeek_song_id":"song-a","play_side":"one_player","play_mode":"sp","play_type":"single","difficulty":"hyper","level":10,"notes":1000,"current_score":score,"clear_type":"EXH-CLEAR","judgments":{"pgreat":50,"great":20,"good":3,"bad":2,"poor":1},"miss_count":{"status":"known","value":20},"timing":{"fast":{"status":"known","value":4},"slow":{"status":"known","value":5}},"combo_break":{"status":"known","value":6},"previous_best":{"score":{"status":"known","value":180},"miss_count":{"status":"unknown","reason":"empty"},"clear_type":{"status":"not_played"}},"play_options":{"status":"known","values":[]}}}})).unwrap()
    }
    #[test]
    fn worker_drains_and_database_instances_are_separate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.db");
        let other = dir.path().join("b.db");
        let mut worker = Worker::start(&path);
        worker.offer(&result(1, 100));
        let health = worker.finish();
        assert!(health.failure.is_none(), "{health:?}");
        assert_eq!(health.committed, 1);
        assert_eq!(health.pending, 0);
        assert_eq!(
            worker.take_completions()[0].chart,
            Some(ChartIdentity {
                scorepeek_song_id: "song-a".into(),
                play_type: "single".into(),
                difficulty: "hyper".into(),
            })
        );
        assert!(Store::open(&path).is_ok());
        assert!(Store::open(&other).is_ok());
        let mut worker = Worker::start(dir.path());
        worker.offer(&result(2, 200));
        assert_eq!(worker.finish().failure.as_deref(), Some("database_open"));
    }
    #[test]
    fn queue_limits_stop_admission_without_blocking() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let (_done_sender, done) = mpsc::channel();
        let (_completion_sender, completions) = mpsc::channel();
        let worker = Worker {
            sender: Some(sender),
            health: Arc::new(Mutex::new(Health::default())),
            done,
            completions,
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
        let (_completion_sender, completions) = mpsc::channel();
        let handle = thread::spawn(move || {
            wait.recv().unwrap();
            done_sender.send(()).unwrap();
        });
        let mut worker = Worker {
            sender: Some(sender),
            health: Arc::new(Mutex::new(Health::default())),
            done,
            completions,
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
