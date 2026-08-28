use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use scorepeek::capture::GamescopeSourceSnapshot;
use serde::Serialize;

const MAX_TRANSITIONS: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchDecision {
    WaitAbsent,
    WaitAmbiguous,
    WaitConsumed,
    Admit { node_id: u32, generation: u64 },
}

#[derive(Default)]
pub struct SourceLifetimes {
    consumed_node: Option<u32>,
    next_generation: u64,
}

impl SourceLifetimes {
    pub fn new() -> Self {
        Self {
            consumed_node: None,
            next_generation: 1,
        }
    }

    pub fn observe(&mut self, snapshot: GamescopeSourceSnapshot) -> WatchDecision {
        match snapshot {
            GamescopeSourceSnapshot::Absent => {
                self.consumed_node = None;
                WatchDecision::WaitAbsent
            }
            GamescopeSourceSnapshot::Ambiguous { .. } => WatchDecision::WaitAmbiguous,
            GamescopeSourceSnapshot::Unique { node_id } if self.consumed_node == Some(node_id) => {
                WatchDecision::WaitConsumed
            }
            GamescopeSourceSnapshot::Unique { node_id } => WatchDecision::Admit {
                node_id,
                generation: self.next_generation,
            },
        }
    }

    pub fn admitted(&mut self, node_id: u32) {
        self.consumed_node = Some(node_id);
        self.next_generation = self.next_generation.saturating_add(1);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WatcherState {
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

impl WatcherState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::WaitingForSource => "waiting_for_source",
            Self::AmbiguousSources => "ambiguous_sources",
            Self::RemoteUnavailable => "remote_unavailable",
            Self::CatalogUnavailable => "catalog_unavailable",
            Self::AdmissionRejected => "admission_rejected",
            Self::SessionActive => "session_active",
            Self::SessionFinished => "session_finished",
            Self::Stopped => "stopped",
        }
    }
}

#[derive(Serialize)]
struct Transition {
    sequence: u64,
    state: WatcherState,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'static str>,
}

#[derive(Serialize)]
struct WatcherStatus<'a> {
    schema: &'static str,
    invocation_id: &'a str,
    current_state: WatcherState,
    session_count: u64,
    active_session_id: Option<&'a str>,
    transitions: &'a VecDeque<Transition>,
    dropped_transitions: u64,
}

pub struct StatusRecorder {
    path: Option<PathBuf>,
    invocation_id: String,
    state: WatcherState,
    session_count: u64,
    active_session_id: Option<String>,
    transitions: VecDeque<Transition>,
    next_sequence: u64,
    dropped: u64,
}

impl StatusRecorder {
    pub fn new(path: Option<PathBuf>, invocation_id: String) -> Self {
        Self {
            path,
            invocation_id,
            state: WatcherState::Starting,
            session_count: 0,
            active_session_id: None,
            transitions: VecDeque::new(),
            next_sequence: 1,
            dropped: 0,
        }
    }

    pub fn transition(
        &mut self,
        state: WatcherState,
        active_session_id: Option<&str>,
        outcome: Option<&'static str>,
    ) -> Result<(), String> {
        if self.state == state
            && self.active_session_id.as_deref() == active_session_id
            && self
                .transitions
                .back()
                .is_some_and(|transition| transition.outcome == outcome)
        {
            return Ok(());
        }
        self.state = state;
        self.active_session_id = active_session_id.map(ToOwned::to_owned);
        if state == WatcherState::SessionActive {
            self.session_count = self.session_count.saturating_add(1);
        }
        if self.transitions.len() == MAX_TRANSITIONS {
            self.transitions.pop_front();
            self.dropped = self.dropped.saturating_add(1);
        }
        self.transitions.push_back(Transition {
            sequence: self.next_sequence,
            state,
            outcome,
        });
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.publish()
    }

    fn publish(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let parent = path
            .parent()
            .ok_or_else(|| "watcher status path has no parent".to_owned())?;
        let staging = staging_path(path);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&staging)
            .map_err(|error| format!("watcher status staging could not be opened: {error}"))?;
        serde_json::to_writer(
            &mut file,
            &WatcherStatus {
                schema: "scorepeek-watcher-status-v1",
                invocation_id: &self.invocation_id,
                current_state: self.state,
                session_count: self.session_count,
                active_session_id: self.active_session_id.as_deref(),
                transitions: &self.transitions,
                dropped_transitions: self.dropped,
            },
        )
        .map_err(|error| format!("watcher status serialization failed: {error}"))?;
        file.write_all(b"\n")
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("watcher status staging write failed: {error}"))?;
        fs::rename(&staging, path)
            .map_err(|error| format!("watcher status publication failed: {error}"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("watcher status directory sync failed: {error}"))
    }
}

fn staging_path(path: &Path) -> PathBuf {
    let mut staging = path.as_os_str().to_owned();
    staging.push(".staging");
    PathBuf::from(staging)
}

#[cfg(test)]
mod tests {
    use scorepeek::capture::GamescopeSourceSnapshot;

    use super::{SourceLifetimes, StatusRecorder, WatchDecision, WatcherState};

    #[test]
    fn one_attempt_per_node_lifetime_and_new_generation_after_absence() {
        let mut lifetimes = SourceLifetimes::new();
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 41 }),
            WatchDecision::Admit {
                node_id: 41,
                generation: 1
            }
        );
        lifetimes.admitted(41);
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 41 }),
            WatchDecision::WaitConsumed
        );
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Absent),
            WatchDecision::WaitAbsent
        );
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 41 }),
            WatchDecision::Admit {
                node_id: 41,
                generation: 2
            }
        );
    }

    #[test]
    fn rejected_lifetime_does_not_consume_a_generation() {
        let mut lifetimes = SourceLifetimes::new();
        assert!(matches!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 1 }),
            WatchDecision::Admit { generation: 1, .. }
        ));
        assert!(matches!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 1 }),
            WatchDecision::Admit { generation: 1, .. }
        ));
        assert!(matches!(
            lifetimes.observe(GamescopeSourceSnapshot::Unique { node_id: 2 }),
            WatchDecision::Admit { generation: 1, .. }
        ));
    }

    #[test]
    fn ambiguous_candidates_are_never_admitted() {
        let mut lifetimes = SourceLifetimes::new();
        assert_eq!(
            lifetimes.observe(GamescopeSourceSnapshot::Ambiguous { candidate_count: 2 }),
            WatchDecision::WaitAmbiguous
        );
    }

    #[test]
    fn status_record_is_bounded_and_atomically_replaced() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("watcher-status.json");
        let mut recorder = StatusRecorder::new(Some(path.clone()), "invocation-1".to_owned());
        for index in 0..40 {
            let state = if index % 2 == 0 {
                WatcherState::WaitingForSource
            } else {
                WatcherState::RemoteUnavailable
            };
            recorder.transition(state, None, None).unwrap();
        }
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["transitions"].as_array().unwrap().len(), 32);
        assert_eq!(value["dropped_transitions"], 8);
        assert!(
            !temporary
                .path()
                .join("watcher-status.json.staging")
                .exists()
        );
    }

    #[test]
    fn disabled_status_recorder_does_not_write() {
        let temporary = tempfile::tempdir().unwrap();
        let mut recorder = StatusRecorder::new(None, "invocation-1".to_owned());
        recorder
            .transition(WatcherState::WaitingForSource, None, None)
            .unwrap();
        assert_eq!(temporary.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn repeated_retry_state_is_recorded_once() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("watcher-status.json");
        let mut recorder = StatusRecorder::new(Some(path.clone()), "invocation-1".to_owned());
        recorder
            .transition(
                WatcherState::AdmissionRejected,
                None,
                Some("capture_admission_failed"),
            )
            .unwrap();
        recorder
            .transition(
                WatcherState::AdmissionRejected,
                None,
                Some("capture_admission_failed"),
            )
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["transitions"].as_array().unwrap().len(), 1);
    }
}
