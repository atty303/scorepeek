//! Public live API projection. Internal observations never become wire records implicitly.
use super::{
    MusicSelectBestSnapshot, MusicSelectionState, ResultDomainEvent, ResultProvisionalState,
    RunEvent, RunEventKind, SongPresentation,
};
use serde::Serialize;
use std::io::{self, Write};
use std::time::{Instant, SystemTime};

pub(super) const MAX_RECORD_BYTES: usize = 1024 * 1024;
pub(super) const EVENT_SCHEMA: &str = "scorepeek-event-v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Binding {
    #[serde(rename = "capture_profile_sha256")]
    pub capture_profile: String,
    #[serde(rename = "normalizer_sha256")]
    pub normalizer: String,
    #[serde(rename = "canonical_layout_sha256")]
    pub canonical_layout: String,
    #[serde(rename = "catalog_sha256")]
    pub catalog: String,
    #[serde(rename = "model_sha256")]
    pub model: String,
    #[serde(rename = "runtime_sha256")]
    pub runtime: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct CaptureContext {
    session_id: Option<String>,
    capture_generation: u64,
    binding: Option<Binding>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum WatcherStatus {
    Starting,
    WaitingForSource,
    AmbiguousSources,
    RemoteUnavailable,
    CatalogUnavailable,
    AdmissionRejected,
    SessionActive,
    SessionFinished,
    Stopped,
}

impl WatcherStatus {
    pub(super) fn from_internal(value: &str) -> Option<Self> {
        Some(match value {
            "starting" => Self::Starting,
            "waiting_for_source" => Self::WaitingForSource,
            "ambiguous_sources" => Self::AmbiguousSources,
            "remote_unavailable" => Self::RemoteUnavailable,
            "catalog_unavailable" => Self::CatalogUnavailable,
            "session_active" => Self::SessionActive,
            "session_finished" => Self::SessionFinished,
            "stopped" => Self::Stopped,
            "admission_rejected" => Self::AdmissionRejected,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Readiness {
    NotReady,
    Ready,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum SessionOutcome {
    Stopped,
    SourceEnded,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct Status {
    watcher: WatcherStatus,
    capture: Option<CaptureContext>,
    catalog: Readiness,
    model: Readiness,
    last_session_outcome: Option<SessionOutcome>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct PublicRecord {
    schema: &'static str,
    invocation_id: String,
    pub(super) sequence: u64,
    event_id: String,
    emitted_monotonic_ms: u64,
    emitted_unix_ms: i64,
    capture: Option<CaptureContext>,
    #[serde(flatten)]
    kind: EventKind,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum EventKind {
    ResultDetected {
        source_sequence: u64,
        song: Option<SongPresentation>,
        result: Box<ResultDomainEvent>,
    },
    ResultProvisionalChanged {
        screen_episode_id: u64,
        source_sequence: u64,
        revision: u64,
        state: ResultProvisionalState,
    },
    MusicSelectionChanged {
        screen_episode_id: u64,
        source_sequence: u64,
        revision: u64,
        state: MusicSelectionState,
    },
    MusicSelectBestObserved {
        snapshot: Option<Box<MusicSelectBestSnapshot>>,
    },
    StatusChanged {
        status: Status,
    },
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct PublicState {
    schema: &'static str,
    invocation_id: String,
    pub(super) next_sequence: u64,
    status: Status,
    latest_result: Option<PublicRecord>,
    provisional_result: Option<PublicRecord>,
    music_selection: Option<PublicRecord>,
    music_select_best: Option<PublicRecord>,
    #[serde(skip)]
    started: Instant,
    #[serde(skip)]
    pub(super) pending_binding: Option<Binding>,
}

impl PublicState {
    pub(super) fn new(invocation_id: String) -> Self {
        Self {
            schema: "scorepeek-event-snapshot-v1",
            invocation_id,
            next_sequence: 1,
            status: Status {
                watcher: WatcherStatus::Starting,
                capture: None,
                catalog: Readiness::NotReady,
                model: Readiness::NotReady,
                last_session_outcome: None,
            },
            latest_result: None,
            provisional_result: None,
            music_selection: None,
            music_select_best: None,
            started: Instant::now(),
            pending_binding: None,
        }
    }

    fn capture(
        &self,
        session_id: Option<&String>,
        generation: Option<u64>,
    ) -> Option<CaptureContext> {
        generation.map(|capture_generation| CaptureContext {
            session_id: session_id.cloned(),
            capture_generation,
            binding: self
                .status
                .capture
                .as_ref()
                .filter(|context| {
                    context.session_id.as_ref() == session_id
                        && context.capture_generation == capture_generation
                })
                .and_then(|context| context.binding.clone()),
        })
    }

    fn event(&mut self, kind: EventKind, capture: Option<CaptureContext>) -> PublicRecord {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        PublicRecord {
            schema: EVENT_SCHEMA,
            invocation_id: self.invocation_id.clone(),
            sequence,
            event_id: format!("{}:{sequence}", self.invocation_id),
            emitted_unix_ms: match SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
                Ok(duration) => i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
                Err(error) => -i64::try_from(error.duration().as_millis()).unwrap_or(i64::MAX),
            },
            emitted_monotonic_ms: u64::try_from(self.started.elapsed().as_millis())
                .unwrap_or(u64::MAX),
            capture,
            kind,
        }
    }

    pub(super) fn watcher(&mut self, watcher: WatcherStatus) -> Option<PublicRecord> {
        let before = self.status.clone();
        self.status.watcher = watcher;
        if watcher == WatcherStatus::CatalogUnavailable {
            self.status.catalog = Readiness::Unavailable;
        } else if self.status.capture.is_none() {
            self.status.catalog = Readiness::NotReady;
        }
        (before != self.status).then(|| {
            self.event(
                EventKind::StatusChanged {
                    status: self.status.clone(),
                },
                self.status.capture.clone(),
            )
        })
    }

    pub(super) fn observes(event: &RunEvent) -> bool {
        matches!(
            event.kind,
            RunEventKind::ResultDetected { .. }
                | RunEventKind::ResultProvisionalChanged { .. }
                | RunEventKind::MusicSelectionChanged { .. }
                | RunEventKind::MusicSelectBestObserved { .. }
                | RunEventKind::MusicSelectResolverChanged { .. }
                | RunEventKind::SessionStarted { .. }
                | RunEventKind::SessionFinished { .. }
                | RunEventKind::WatcherStarted { .. }
                | RunEventKind::WatcherStopped { .. }
        )
    }

    // This state is updated only by the same ordered publications used by live recognition/replay.
    pub(super) fn project(&mut self, event: &RunEvent) -> Vec<PublicRecord> {
        let mut records = Vec::new();
        let (kind, capture) = match &event.kind {
            RunEventKind::ResultDetected {
                session_id,
                capture_generation,
                source_sequence,
                song,
                result,
            } => (
                EventKind::ResultDetected {
                    source_sequence: *source_sequence,
                    song: song.clone(),
                    result: Box::new(result.clone()),
                },
                self.capture(Some(session_id), Some(*capture_generation)),
            ),
            RunEventKind::ResultProvisionalChanged {
                session_id,
                capture_generation,
                screen_episode_id,
                source_sequence,
                revision,
                state,
            } => (
                EventKind::ResultProvisionalChanged {
                    screen_episode_id: *screen_episode_id,
                    source_sequence: *source_sequence,
                    revision: *revision,
                    state: state.clone(),
                },
                self.capture(Some(session_id), Some(*capture_generation)),
            ),
            RunEventKind::MusicSelectionChanged {
                session_id,
                capture_generation,
                screen_episode_id,
                source_sequence,
                revision,
                state,
            } => (
                EventKind::MusicSelectionChanged {
                    screen_episode_id: *screen_episode_id,
                    source_sequence: *source_sequence,
                    revision: *revision,
                    state: state.clone(),
                },
                self.capture(session_id.as_ref(), *capture_generation),
            ),
            RunEventKind::MusicSelectBestObserved {
                session_id,
                capture_generation,
                snapshot,
            } => (
                EventKind::MusicSelectBestObserved {
                    snapshot: Some(Box::new(snapshot.clone())),
                },
                self.capture(Some(session_id), Some(*capture_generation)),
            ),
            RunEventKind::MusicSelectResolverChanged {
                session_id,
                capture_generation,
                state,
            } => {
                if state.snapshot.is_some() || self.music_select_best.is_none() {
                    return records;
                }
                (
                    EventKind::MusicSelectBestObserved { snapshot: None },
                    self.capture(session_id.as_ref(), *capture_generation),
                )
            }
            _ => match self.lifecycle(event) {
                Some(value) => value,
                None => return records,
            },
        };
        let record = self.event(kind, capture);
        self.retain(&record);
        records.push(record);
        records
    }

    fn retain(&mut self, record: &PublicRecord) {
        match &record.kind {
            EventKind::ResultDetected { .. } => {
                self.latest_result = Some(record.clone());
                self.provisional_result = None;
            }
            EventKind::ResultProvisionalChanged { state, .. } => {
                self.provisional_result = matches!(state, ResultProvisionalState::Resolved { .. })
                    .then(|| record.clone());
            }
            EventKind::MusicSelectionChanged { .. } => self.music_selection = Some(record.clone()),
            EventKind::MusicSelectBestObserved { snapshot } => {
                self.music_select_best = snapshot.as_ref().map(|_| record.clone());
            }
            EventKind::StatusChanged { .. } => {}
        }
    }

    fn lifecycle(&mut self, event: &RunEvent) -> Option<(EventKind, Option<CaptureContext>)> {
        let value = match &event.kind {
            RunEventKind::SessionStarted {
                session_id,
                capture_generation,
                ..
            } => {
                let binding = self.pending_binding.take();
                let readiness = if binding.is_some() {
                    Readiness::Ready
                } else {
                    Readiness::NotReady
                };
                self.status = Status {
                    watcher: WatcherStatus::SessionActive,
                    capture: Some(CaptureContext {
                        session_id: session_id.clone(),
                        capture_generation: *capture_generation,
                        binding,
                    }),
                    catalog: readiness,
                    model: readiness,
                    last_session_outcome: None,
                };
                self.clear_current();
                (
                    EventKind::StatusChanged {
                        status: self.status.clone(),
                    },
                    self.status.capture.clone(),
                )
            }
            RunEventKind::SessionFinished { .. } | RunEventKind::WatcherStopped { .. } => {
                let capture = self.status.capture.take();
                if let RunEventKind::SessionFinished { outcome, .. } = &event.kind {
                    self.status.last_session_outcome = Some(match outcome.as_str() {
                        "stopped" => SessionOutcome::Stopped,
                        "source_ended" => SessionOutcome::SourceEnded,
                        _ => SessionOutcome::Error,
                    });
                }
                self.clear_current();
                self.status.watcher = if matches!(event.kind, RunEventKind::WatcherStopped { .. }) {
                    WatcherStatus::Stopped
                } else {
                    WatcherStatus::SessionFinished
                };
                self.status.catalog = Readiness::NotReady;
                self.status.model = Readiness::NotReady;
                (
                    EventKind::StatusChanged {
                        status: self.status.clone(),
                    },
                    capture,
                )
            }
            RunEventKind::WatcherStarted { .. } => (
                EventKind::StatusChanged {
                    status: self.status.clone(),
                },
                None,
            ),
            _ => return None,
        };
        Some(value)
    }

    fn clear_current(&mut self) {
        self.provisional_result = None;
        self.music_selection = None;
        self.music_select_best = None;
    }
}

struct BoundedRecord(Vec<u8>);
impl Write for BoundedRecord {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_RECORD_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("public record exceeds 1 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn encode(value: &impl Serialize) -> Result<Vec<u8>, serde_json::Error> {
    let mut writer = BoundedRecord(Vec::new());
    serde_json::to_writer(&mut writer, value)?;
    writer.write_all(b"\n").map_err(serde_json::Error::io)?;
    Ok(writer.0)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn run(kind: RunEventKind) -> RunEvent {
        RunEvent {
            schema: super::super::RUN_EVENT_SCHEMA.into(),
            kind,
        }
    }

    fn session() -> RunEvent {
        run(RunEventKind::SessionStarted {
            session_id: Some("session".into()),
            capture_generation: 7,
            capture_profile_sha256: "a".repeat(64),
            normalizer_artifact_sha256: "b".repeat(64),
        })
    }

    fn binding() -> Binding {
        Binding {
            capture_profile: "a".repeat(64),
            normalizer: "b".repeat(64),
            canonical_layout: "c".repeat(64),
            catalog: "d".repeat(64),
            model: "e".repeat(64),
            runtime: "f".repeat(64),
        }
    }

    // Consumer model intentionally operates on the wire, independently of the producer reducer.
    pub(crate) fn fold(snapshot: &mut Value, event: &Value) {
        assert_eq!(snapshot["next_sequence"], event["sequence"]);
        snapshot["next_sequence"] = (event["sequence"].as_u64().unwrap() + 1).into();
        match event["event"].as_str().unwrap() {
            "status_changed" => {
                snapshot["status"] = event["status"].clone();
                if ["session_active", "session_finished", "stopped"]
                    .contains(&event["status"]["watcher"].as_str().unwrap())
                {
                    for slot in ["provisional_result", "music_selection", "music_select_best"] {
                        snapshot[slot] = Value::Null;
                    }
                }
            }
            "result_detected" => {
                snapshot["latest_result"] = event.clone();
                snapshot["provisional_result"] = Value::Null;
            }
            "result_provisional_changed" => {
                snapshot["provisional_result"] = if event["state"]["status"] == "resolved" {
                    event.clone()
                } else {
                    Value::Null
                };
            }
            "music_selection_changed" => snapshot["music_selection"] = event.clone(),
            "music_select_best_observed" => {
                snapshot["music_select_best"] = if event["snapshot"].is_null() {
                    Value::Null
                } else {
                    event.clone()
                }
            }
            kind => panic!("unexpected public kind: {kind}"),
        }
    }

    #[test]
    fn lifecycle_projection_matches_consumer_state_and_does_not_publish_reports() {
        let mut state = PublicState::new("run".into());
        let mut consumer = serde_json::to_value(&state).unwrap();
        state.pending_binding = Some(binding());
        let events = [
            session(),
            run(RunEventKind::MusicSelectionChanged {
                session_id: Some("session".into()),
                capture_generation: Some(7),
                screen_episode_id: 1,
                source_sequence: 10,
                revision: 1,
                state: MusicSelectionState::Unresolved {
                    reason: super::super::MusicSelectionUnresolvedReason::EvidenceUnresolved,
                },
            }),
            run(RunEventKind::SessionFinished {
                session_id: "session".into(),
                capture_generation: 7,
                outcome: "error".into(),
                report: json!({ "raw_ocr": "MUST_NOT_APPEAR", "directory": "/private/path" }),
            }),
            run(RunEventKind::WatcherStopped {
                invocation_id: "run".into(),
                reason: "diagnostic detail".into(),
            }),
        ];
        for internal in events {
            for event in state.project(&internal) {
                let wire = serde_json::to_value(event).unwrap();
                if wire["status"]["watcher"] == "session_active" {
                    assert_eq!(wire["capture"]["binding"]["catalog_sha256"], "d".repeat(64));
                    assert_eq!(wire["status"]["model"], "ready");
                }
                fold(&mut consumer, &wire);
                assert_eq!(consumer, serde_json::to_value(&state).unwrap());
                let bytes = wire.to_string();
                assert!(!bytes.contains("MUST_NOT_APPEAR"));
                assert!(!bytes.contains("/private/path"));
                assert!(!bytes.contains("diagnostic detail"));
            }
        }
        assert!(consumer["status"]["capture"].is_null());
    }

    #[test]
    fn raw_observations_do_not_advance_public_sequence() {
        let mut state = PublicState::new("run".into());
        let raw = run(RunEventKind::RawScreenObserved {
            session_id: Some("session".into()),
            capture_generation: Some(7),
            semantic_episode_id: Some(1),
            sequence: 9,
            monotonic_start_ms: 0,
            monotonic_end_ms: 1,
            screen: "raw frame label".into(),
            unknown_reason: None,
        });
        assert!(state.project(&raw).is_empty());
        assert_eq!(state.next_sequence, 1);
        let mut consumer = serde_json::to_value(&state).unwrap();
        let wire = serde_json::to_value(state.watcher(WatcherStatus::CatalogUnavailable).unwrap())
            .unwrap();
        fold(&mut consumer, &wire);
        assert_eq!(consumer, serde_json::to_value(&state).unwrap());
        assert!(state.watcher(WatcherStatus::CatalogUnavailable).is_none());
    }

    #[test]
    fn record_limit_includes_newline_and_never_returns_partial_json() {
        let at_limit = "x".repeat(MAX_RECORD_BYTES - 3);
        assert_eq!(encode(&at_limit).unwrap().len(), MAX_RECORD_BYTES);
        assert!(encode(&(at_limit + "x")).is_err());
    }
}
