use std::collections::VecDeque;
use std::env;
use std::fs::{self, DirBuilder};
use std::io::{self, BufWriter, IsTerminal as _, Write as _};
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

use crate::play_attempt::{
    PlayAttemptReducer, PlayAttemptScreen, PlayAttemptState, SelectionSource,
};
use crate::run_event_artifact::{FinishOutcome as RunEventArtifactOutcome, RunEventArtifactWorker};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use scorepeek::catalog::{Difficulty, PlayType, ScorepeekSongId};
use scorepeek::recognition::{
    ParsedResultFields, PreviousBest, PreviousBestValue, ResultChartResolution, ResultJudgments,
    ResultPerformanceResolution, ResultTiming, SupplementalResultValue,
};
use scorepeek::temporal_recognition::{
    MusicSelectTemporalPolicy, MusicSelectTemporalReducer, MusicSelectTemporalState,
    MusicSelectTemporalTransitionReason, ResultTemporalReducer, ResultTemporalState,
    TemporalFieldTransition, TemporalPolicy, TemporalTransitionReason,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const MAX_CLIENTS: usize = 8;
const EVENT_QUEUE_CAPACITY: usize = 64;
const SOCKET_NAME: &str = "observations-v2.sock";
const RESULT_HISTORY_CAPACITY: usize = 32;
const RESULT_TEMPORAL_REQUIRED_OBSERVATIONS: u8 = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunEvent {
    pub schema: String,
    #[serde(flatten)]
    pub kind: RunEventKind,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "the event schema remains flat and values cross an already bounded queue"
)]
pub enum RunEventKind {
    WatcherStarted {
        invocation_id: String,
        profile_sha256: String,
    },
    SessionStarted {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        capture_generation: u64,
        capture_profile_sha256: String,
        normalizer_artifact_sha256: String,
    },
    ScreenChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
    },
    FieldObservation {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
        fields: Value,
        result_song_resolution: Value,
        music_select_song_resolution: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        parsed_result_fields: Option<ParsedResultFields>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result_chart_resolution: Option<ResultChartResolution>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result_performance_resolution: Option<ResultPerformanceResolution>,
        #[serde(skip_serializing_if = "Option::is_none")]
        current_score_ocr_resolution: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        numeric_batch: Option<Value>,
        song_resolution_presentation: Box<SongResolutionPresentation>,
    },
    ResultDetected {
        session_id: String,
        capture_generation: u64,
        source_sequence: u64,
        result: ResultDomainEvent,
    },
    TemporalResultChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_sequence: Option<u64>,
        transitions: Vec<TemporalFieldTransition>,
        state: ResultTemporalState<scorepeek::catalog::ScorepeekSongId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stable_song: Option<SongPresentation>,
    },
    TemporalMusicSelectChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_sequence: Option<u64>,
        reasons: Vec<MusicSelectTemporalTransitionReason>,
        state: MusicSelectTemporalState<scorepeek::catalog::ScorepeekSongId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retained_song: Option<SongPresentation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        candidate_song: Option<SongPresentation>,
    },
    NumericResultChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        source_sequence: u64,
        state: NumericResultTemporalState,
        reason: NumericResultTransitionReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        event_suppression_reason: Option<NumericResultEventSuppressionReason>,
    },
    PlayAttemptChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        source_sequence: Option<u64>,
        state: PlayAttemptState<SongPresentation>,
    },
    SessionFinished {
        session_id: String,
        capture_generation: u64,
        outcome: String,
        report: Value,
    },
    WatcherStopped {
        invocation_id: String,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NumericResultTemporalState {
    Unknown,
    Pending { observations: u8 },
    Accepted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericResultTransitionReason {
    Incomplete,
    CandidateStarted,
    CandidateRepeated,
    Accepted,
    Conflict,
    ChronologyReset,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericResultEventSuppressionReason {
    NumericNotAccepted,
    SessionUnavailable,
    ResultSongNotStable,
    ClearTypeNotStable,
    PlayAttemptNotAccepted,
    LinkageConflict,
    AlreadyEmitted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultDomainEvent {
    pub contract: String,
    pub attempt_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_attempt_id: Option<u64>,
    pub scorepeek_song_id: ScorepeekSongId,
    pub play_side: String,
    pub play_mode: String,
    pub play_type: PlayType,
    pub difficulty: Difficulty,
    pub level: u8,
    pub notes: u32,
    pub current_score: u32,
    pub clear_type: String,
    pub judgments: ResultJudgments,
    pub miss_count: SupplementalResultValue<u32>,
    pub timing: ResultTiming,
    pub combo_break: SupplementalResultValue<u32>,
    pub previous_best: PreviousBest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NumericResultView {
    song_id: ScorepeekSongId,
    clear_type: String,
    chart: scorepeek::catalog::Chart,
    current_score: u32,
    performance: ResultPerformanceResolution,
    source_sequence: u64,
}

#[derive(Clone, Debug)]
struct PendingNumericResult {
    view: NumericResultView,
    observations: u8,
}

fn same_numeric_tuple(left: &NumericResultView, right: &NumericResultView) -> bool {
    left.song_id == right.song_id
        && left.clear_type == right.clear_type
        && left.chart == right.chart
        && left.current_score == right.current_score
        && left.performance == right.performance
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SongPresentation {
    pub scorepeek_song_id: scorepeek::catalog::ScorepeekSongId,
    pub display_titles: Vec<String>,
    pub artist: String,
}

#[derive(Clone, Debug, Serialize)]
struct ResultHistoryEntry {
    ordinal: u64,
    session_id: String,
    capture_generation: u64,
    source_sequence: u64,
    song: Option<SongPresentation>,
    result: ResultDomainEvent,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SongResolutionPresentation {
    Accepted {
        reason: Option<Value>,
        selected: SongPresentation,
        runner_up: SongPresentation,
        evidence_summary: String,
    },
    Unknown {
        reason: Value,
        selected: Option<SongPresentation>,
        runner_up: Option<SongPresentation>,
        evidence_summary: Option<String>,
    },
}

impl RunEvent {
    pub fn to_value(&self) -> Result<Value, String> {
        serde_json::to_value(self)
            .map_err(|error| format!("run event serialization failed: {error}"))
    }

    pub fn from_value(value: Value) -> Result<Self, String> {
        serde_json::from_value(value)
            .map_err(|error| format!("run event contract validation failed: {error}"))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RunViewState {
    invocation_id: String,
    profile_sha256: String,
    recording: &'static str,
    watcher_state: String,
    session_count: u64,
    active_session_id: Option<String>,
    capture_generation: Option<u64>,
    current_screen: Option<String>,
    latest_observation: Option<Value>,
    latest_stabilized_result: Option<Value>,
    latest_temporal_music_select: Option<Value>,
    latest_play_attempt: Option<Value>,
    latest_result_detected: Option<Value>,
    result_history: VecDeque<ResultHistoryEntry>,
    result_count: u64,
    #[serde(skip)]
    stable_result_song: Option<SongPresentation>,
    latest_report: Option<Value>,
    status_recording: &'static str,
    next_channel_sequence: u64,
    message: String,
}

impl RunViewState {
    fn new(invocation_id: String, profile_sha256: String, recording_enabled: bool) -> Self {
        Self {
            invocation_id,
            profile_sha256,
            recording: if recording_enabled {
                "enabled"
            } else {
                "disabled"
            },
            watcher_state: "starting".to_owned(),
            session_count: 0,
            active_session_id: None,
            capture_generation: None,
            current_screen: None,
            latest_observation: None,
            latest_stabilized_result: None,
            latest_temporal_music_select: None,
            latest_play_attempt: None,
            latest_result_detected: None,
            result_history: VecDeque::with_capacity(RESULT_HISTORY_CAPACITY),
            result_count: 0,
            stable_result_song: None,
            latest_report: None,
            status_recording: if recording_enabled {
                "ready"
            } else {
                "disabled"
            },
            next_channel_sequence: 1,
            message: "initializing".to_owned(),
        }
    }

    fn reduce(&mut self, event: &RunEvent, serialized: &Value) {
        match &event.kind {
            RunEventKind::WatcherStarted { .. } => "starting".clone_into(&mut self.watcher_state),
            RunEventKind::SessionStarted {
                session_id,
                capture_generation,
                ..
            } => {
                "session_active".clone_into(&mut self.watcher_state);
                self.session_count = self.session_count.saturating_add(1);
                self.active_session_id.clone_from(session_id);
                self.capture_generation = Some(*capture_generation);
                self.current_screen = None;
                self.latest_observation = None;
                self.latest_stabilized_result = None;
                self.latest_temporal_music_select = None;
                self.latest_play_attempt = None;
                self.latest_result_detected = None;
                self.stable_result_song = None;
                self.latest_report = None;
                "Gamescope session admitted".clone_into(&mut self.message);
            }
            RunEventKind::ScreenChanged { screen, .. } => {
                self.current_screen = Some(screen.clone());
            }
            RunEventKind::FieldObservation { .. } => {
                self.latest_observation = Some(serialized.clone());
            }
            RunEventKind::TemporalResultChanged { stable_song, .. } => {
                self.latest_stabilized_result = Some(serialized.clone());
                self.stable_result_song.clone_from(stable_song);
            }
            RunEventKind::TemporalMusicSelectChanged { .. } => {
                self.latest_temporal_music_select = Some(serialized.clone());
            }
            RunEventKind::NumericResultChanged { .. } => {}
            RunEventKind::PlayAttemptChanged { .. } => {
                self.latest_play_attempt = Some(serialized.clone());
            }
            RunEventKind::ResultDetected {
                session_id,
                capture_generation,
                source_sequence,
                result,
            } => {
                self.latest_result_detected = Some(serialized.clone());
                self.result_count = self.result_count.saturating_add(1);
                if self.result_history.len() == RESULT_HISTORY_CAPACITY {
                    self.result_history.pop_front();
                }
                let song = self
                    .stable_result_song
                    .as_ref()
                    .filter(|song| song.scorepeek_song_id == result.scorepeek_song_id)
                    .cloned();
                self.result_history.push_back(ResultHistoryEntry {
                    ordinal: self.result_count,
                    session_id: session_id.clone(),
                    capture_generation: *capture_generation,
                    source_sequence: *source_sequence,
                    song,
                    result: result.clone(),
                });
            }
            RunEventKind::SessionFinished {
                outcome, report, ..
            } => {
                "session_finished".clone_into(&mut self.watcher_state);
                self.active_session_id = None;
                self.capture_generation = None;
                self.current_screen = None;
                self.latest_report = Some(report.clone());
                self.message = format!("session finished: {outcome}");
            }
            RunEventKind::WatcherStopped { .. } => {
                "stopped".clone_into(&mut self.watcher_state);
                self.active_session_id = None;
                self.capture_generation = None;
                self.current_screen = None;
                self.latest_observation = None;
                self.latest_stabilized_result = None;
                self.latest_temporal_music_select = None;
                self.latest_play_attempt = None;
                self.latest_result_detected = None;
                self.stable_result_song = None;
                "scorepeek stopped by signal".clone_into(&mut self.message);
            }
        }
    }
}

#[derive(Default)]
struct ChannelHealth {
    connected_clients: AtomicUsize,
    dropped_events: AtomicU64,
    disconnected_clients: AtomicU64,
    server_failed: AtomicBool,
}

impl ChannelHealth {
    fn value(&self) -> Value {
        json!({
            "status": if self.server_failed.load(Ordering::Acquire) { "degraded" } else { "ready" },
            "connected_clients": self.connected_clients.load(Ordering::Acquire),
            "dropped_events": self.dropped_events.load(Ordering::Acquire),
            "disconnected_clients": self.disconnected_clients.load(Ordering::Acquire),
        })
    }
}

struct ObservationChannel {
    sender: SyncSender<Vec<u8>>,
    stop: Arc<AtomicBool>,
    health: Arc<ChannelHealth>,
    thread: Option<JoinHandle<()>>,
    socket_path: PathBuf,
    socket_identity: (u64, u64),
}

struct SocketPathGuard {
    path: PathBuf,
    identity: (u64, u64),
    armed: bool,
}

impl SocketPathGuard {
    fn new(path: PathBuf, identity: (u64, u64)) -> Self {
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

impl ObservationChannel {
    fn start(state: Arc<Mutex<RunViewState>>) -> Result<Self, String> {
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && !path.as_os_str().is_empty())
            .ok_or_else(|| {
                "XDG_RUNTIME_DIR must be absolute and non-empty for scorepeek run".to_owned()
            })?;
        Self::start_at(&runtime, state)
    }

    fn start_at(runtime: &Path, state: Arc<Mutex<RunViewState>>) -> Result<Self, String> {
        let directory = runtime.join("scorepeek");
        ensure_private_directory(&directory)?;
        let socket_path = directory.join(SOCKET_NAME);
        remove_stale_socket(&socket_path)?;
        let listener = UnixListener::bind(&socket_path)
            .map_err(|error| format!("observation socket could not be bound: {error}"))?;
        let metadata = socket_path
            .symlink_metadata()
            .map_err(|error| format!("observation socket could not be inspected: {error}"))?;
        let socket_identity = (metadata.dev(), metadata.ino());
        let mut path_guard = SocketPathGuard::new(socket_path.clone(), socket_identity);
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("observation socket permissions could not be set: {error}"))?;
        listener.set_nonblocking(true).map_err(|error| {
            format!("observation socket could not be made nonblocking: {error}")
        })?;
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(EVENT_QUEUE_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let health = Arc::new(ChannelHealth::default());
        let thread_stop = Arc::clone(&stop);
        let thread_health = Arc::clone(&health);
        let thread = thread::Builder::new()
            .name("scorepeek-observation-socket".to_owned())
            .spawn(move || {
                let mut clients = Vec::new();
                loop {
                    accept_clients(&listener, &state, &thread_health, &mut clients);
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(bytes) => broadcast(&bytes, &thread_health, &mut clients),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if thread_stop.load(Ordering::Acquire) {
                                break;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                thread_health.connected_clients.store(0, Ordering::Release);
            })
            .map_err(|error| format!("observation socket worker could not start: {error}"))?;
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

    fn publish(&self, event: &Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(event)
            .map_err(|error| format!("run observation serialization failed: {error}"))?;
        bytes.push(b'\n');
        try_send_event(&self.sender, &self.health, bytes);
        Ok(())
    }
}

fn try_send_event(sender: &SyncSender<Vec<u8>>, health: &ChannelHealth, bytes: Vec<u8>) {
    match sender.try_send(bytes) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            health.dropped_events.fetch_add(1, Ordering::AcqRel);
        }
        Err(TrySendError::Disconnected(_)) => {
            health.server_failed.store(true, Ordering::Release);
        }
    }
}

impl Drop for ObservationChannel {
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
        Ok(_) => Err("observation socket directory is not a directory".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).map_err(|error| {
                format!("observation socket directory could not be created: {error}")
            })
        }
        Err(error) => Err(format!(
            "observation socket directory could not be inspected: {error}"
        )),
    }
}

fn remove_stale_socket(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_socket() => match UnixStream::connect(path) {
            Ok(_) => Err("observation socket is already active".to_owned()),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path)
                .map_err(|error| format!("stale observation socket could not be removed: {error}")),
            Err(error) => Err(format!(
                "observation socket liveness could not be determined: {error}"
            )),
        },
        Ok(_) => Err("observation socket path contains a non-socket entry".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "observation socket path could not be inspected: {error}"
        )),
    }
}

fn accept_clients(
    listener: &UnixListener,
    state: &Arc<Mutex<RunViewState>>,
    health: &ChannelHealth,
    clients: &mut Vec<UnixStream>,
) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if clients.len() >= MAX_CLIENTS || stream.set_nonblocking(true).is_err() {
                    health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
                clients.push(stream);
                health
                    .connected_clients
                    .store(clients.len(), Ordering::Release);
                let Some(snapshot) = snapshot_bytes(state, health) else {
                    clients.pop();
                    health
                        .connected_clients
                        .store(clients.len(), Ordering::Release);
                    health.server_failed.store(true, Ordering::Release);
                    continue;
                };
                if clients.last_mut().unwrap().write_all(&snapshot).is_err() {
                    clients.pop();
                    health
                        .connected_clients
                        .store(clients.len(), Ordering::Release);
                    health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => {
                health.server_failed.store(true, Ordering::Release);
                break;
            }
        }
    }
}

fn snapshot_bytes(state: &Arc<Mutex<RunViewState>>, health: &ChannelHealth) -> Option<Vec<u8>> {
    let state = state.lock().ok()?.clone();
    let mut bytes = serde_json::to_vec(&json!({
        "schema": "scorepeek-run-observation-snapshot-v1",
        "state": state,
        "channel": health.value(),
    }))
    .ok()?;
    bytes.push(b'\n');
    Some(bytes)
}

fn broadcast(bytes: &[u8], health: &ChannelHealth, clients: &mut Vec<UnixStream>) {
    clients.retain_mut(|client| {
        if client.write_all(bytes).is_ok() {
            true
        } else {
            health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
            false
        }
    });
    health
        .connected_clients
        .store(clients.len(), Ordering::Release);
}

enum Display {
    Tui(TerminalGuard),
    Plain {
        output: BufWriter<io::Stdout>,
        last_line: Option<String>,
    },
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self, String> {
        let mut output = io::stdout();
        enter_alternate_screen(&mut output)?;
        let backend = CrosstermBackend::new(output);
        let terminal = Terminal::new(backend).map_err(|error| {
            let mut restore = io::stdout();
            let _ = restore.write_all(b"\x1b[?25h\x1b[?1049l");
            format!("terminal could not initialize TUI rendering: {error}")
        })?;
        Ok(Self { terminal })
    }

    fn draw(
        &mut self,
        state: &RunViewState,
        socket_path: &Path,
        health: &ChannelHealth,
    ) -> Result<(), String> {
        self.terminal
            .draw(|frame| render(frame, state, socket_path, health))
            .map(|_| ())
            .map_err(|error| format!("TUI output failed: {error}"))
    }
}

fn enter_alternate_screen(output: &mut impl io::Write) -> Result<(), String> {
    if let Err(error) = output
        .write_all(b"\x1b[?1049h\x1b[?25l")
        .and_then(|()| output.flush())
    {
        let _ = output.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = output.flush();
        return Err(format!("terminal could not enter TUI mode: {error}"));
    }
    Ok(())
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut output = io::stdout();
        let _ = output.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = output.flush();
    }
}

pub struct RoutineOutput {
    state: Arc<Mutex<RunViewState>>,
    channel: ObservationChannel,
    display: Display,
    next_sequence: u64,
    temporal_result: ResultTemporalReducer<scorepeek::catalog::ScorepeekSongId>,
    stable_result_song: Option<SongPresentation>,
    temporal_music_select: MusicSelectTemporalReducer<scorepeek::catalog::ScorepeekSongId>,
    retained_music_select_song: Option<SongPresentation>,
    candidate_music_select_song: Option<SongPresentation>,
    play_attempt: PlayAttemptReducer<SongPresentation>,
    pending_numeric_result: Option<PendingNumericResult>,
    accepted_numeric_result: Option<NumericResultView>,
    last_numeric_sequence: Option<u64>,
    last_numeric_monotonic_ms: Option<u64>,
    result_event_emitted: bool,
    latest_screen_boundary_sequence: Option<u64>,
    event_store: Option<PathBuf>,
    event_worker: Option<RunEventArtifactWorker>,
    completed_event_artifact: Option<RunEventArtifactOutcome>,
}

impl RoutineOutput {
    pub fn start(
        invocation_id: String,
        profile_sha256: String,
        recording_enabled: bool,
        event_store: Option<PathBuf>,
    ) -> Result<Self, String> {
        let state = Arc::new(Mutex::new(RunViewState::new(
            invocation_id,
            profile_sha256,
            recording_enabled,
        )));
        let channel = ObservationChannel::start(Arc::clone(&state))?;
        let display = if io::stdout().is_terminal() {
            Display::Tui(TerminalGuard::new()?)
        } else {
            Display::Plain {
                output: BufWriter::new(io::stdout()),
                last_line: None,
            }
        };
        let mut output = Self {
            state,
            channel,
            display,
            next_sequence: 1,
            temporal_result: ResultTemporalReducer::new(
                TemporalPolicy::for_episode(RESULT_TEMPORAL_REQUIRED_OBSERVATIONS)
                    .expect("fixed result temporal policy is valid"),
            ),
            stable_result_song: None,
            temporal_music_select: MusicSelectTemporalReducer::new(
                MusicSelectTemporalPolicy::new(200, 200, 250)
                    .expect("fixed music-select temporal policy is valid"),
            ),
            retained_music_select_song: None,
            candidate_music_select_song: None,
            play_attempt: PlayAttemptReducer::default(),
            pending_numeric_result: None,
            accepted_numeric_result: None,
            last_numeric_sequence: None,
            last_numeric_monotonic_ms: None,
            result_event_emitted: false,
            latest_screen_boundary_sequence: None,
            event_store,
            event_worker: None,
            completed_event_artifact: None,
        };
        output.refresh()?;
        Ok(output)
    }

    pub fn publish(&mut self, event: &RunEvent) -> Result<(), String> {
        match &event.kind {
            RunEventKind::SessionStarted { session_id, .. } => {
                self.temporal_result
                    .reset(TemporalTransitionReason::ResetBySessionBoundary);
                self.stable_result_song = None;
                self.temporal_music_select
                    .reset(MusicSelectTemporalTransitionReason::ResetBySessionBoundary);
                self.retained_music_select_song = None;
                self.candidate_music_select_song = None;
                self.play_attempt.reset_session();
                self.reset_numeric_result();
                self.result_event_emitted = false;
                self.latest_screen_boundary_sequence = None;
                self.completed_event_artifact = None;
                self.event_worker =
                    self.event_store.as_deref().zip(session_id.as_deref()).map(
                        |(store, session_id)| RunEventArtifactWorker::start(store, session_id),
                    );
                self.publish_one(event)
            }
            RunEventKind::WatcherStopped { .. } => self.publish_watcher_stopped(event),
            RunEventKind::FieldObservation { .. } => self.publish_field_observation(event),
            RunEventKind::ScreenChanged { .. } => self.publish_screen_change(event),
            RunEventKind::SessionFinished { .. } => self.publish_session_finished(event),
            RunEventKind::WatcherStarted { .. }
            | RunEventKind::TemporalResultChanged { .. }
            | RunEventKind::TemporalMusicSelectChanged { .. }
            | RunEventKind::NumericResultChanged { .. }
            | RunEventKind::PlayAttemptChanged { .. }
            | RunEventKind::ResultDetected { .. } => self.publish_one(event),
        }
    }

    fn publish_watcher_stopped(&mut self, event: &RunEvent) -> Result<(), String> {
        let (session_id, capture_generation) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            (state.active_session_id.clone(), state.capture_generation)
        };
        if let Some(state) = self.play_attempt.finish_session() {
            self.publish_play_attempt_update(session_id.clone(), capture_generation, None, state)?;
        }
        self.publish_one(event)?;
        if let Some(update) = self
            .temporal_result
            .reset(TemporalTransitionReason::ResetBySessionBoundary)
        {
            self.stable_result_song = None;
            self.publish_temporal_update(session_id.clone(), capture_generation, None, update)?;
        }
        if let Some(update) = self
            .temporal_music_select
            .reset(MusicSelectTemporalTransitionReason::ResetBySessionBoundary)
        {
            self.retained_music_select_song = None;
            self.candidate_music_select_song = None;
            self.publish_music_select_temporal_update(
                session_id,
                capture_generation,
                None,
                update,
            )?;
        }
        self.completed_event_artifact =
            self.event_worker.take().map(RunEventArtifactWorker::finish);
        Ok(())
    }

    fn publish_field_observation(&mut self, event: &RunEvent) -> Result<(), String> {
        let RunEventKind::FieldObservation {
            session_id,
            capture_generation,
            sequence,
            monotonic_end_ms,
            screen,
            fields,
            result_chart_resolution,
            result_performance_resolution,
            song_resolution_presentation,
            ..
        } = &event.kind
        else {
            unreachable!("field observation dispatcher preserves event kind");
        };
        self.publish_one(event)?;
        if self
            .latest_screen_boundary_sequence
            .is_some_and(|boundary| *sequence < boundary)
        {
            return Ok(());
        }
        match screen.as_str() {
            "result" => self.reduce_result_observation(
                session_id.as_ref(),
                *capture_generation,
                *sequence,
                *monotonic_end_ms,
                fields,
                result_chart_resolution.as_ref(),
                result_performance_resolution.as_ref(),
                song_resolution_presentation,
            ),
            "music_select" => self.reduce_music_select_observation(
                session_id.as_ref(),
                *capture_generation,
                *sequence,
                *monotonic_end_ms,
                song_resolution_presentation,
            ),
            _ => Ok(()),
        }
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the reducer keeps ordered temporal, attempt, and domain emission in one path"
    )]
    fn reduce_result_observation(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_end_ms: u64,
        fields: &Value,
        result_chart_resolution: Option<&ResultChartResolution>,
        result_performance_resolution: Option<&ResultPerformanceResolution>,
        song_resolution_presentation: &SongResolutionPresentation,
    ) -> Result<(), String> {
        let observed_song = match song_resolution_presentation {
            SongResolutionPresentation::Accepted { selected, .. } => {
                Some(selected.scorepeek_song_id)
            }
            SongResolutionPresentation::Unknown { .. } => None,
        };
        let observed_clear_type = fields
            .get("clear_type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if let Some(update) = self.temporal_result.observe_result(
            sequence,
            monotonic_end_ms,
            observed_song,
            observed_clear_type.clone(),
        ) {
            if let Some(stable_song_id) = update.state.song.stable_value() {
                if let SongResolutionPresentation::Accepted { selected, .. } =
                    song_resolution_presentation
                    && selected.scorepeek_song_id == *stable_song_id
                {
                    self.stable_result_song = Some(selected.clone());
                }
            } else {
                self.stable_result_song = None;
            }
            self.publish_temporal_update(
                session_id.cloned(),
                capture_generation,
                Some(sequence),
                update,
            )?;
            if let Some(song) = self.stable_result_song.clone()
                && let Some(state) = self.play_attempt.observe_stable_result(song)
            {
                self.publish_play_attempt_update(
                    session_id.cloned(),
                    capture_generation,
                    Some(sequence),
                    state,
                )?;
            }
        }
        if let Some((state, reason)) = self.observe_numeric_result(
            sequence,
            monotonic_end_ms,
            observed_song,
            observed_clear_type,
            result_chart_resolution,
            result_performance_resolution,
        ) {
            self.publish_one(&RunEvent {
                schema: "scorepeek-run-event-v2".to_owned(),
                kind: RunEventKind::NumericResultChanged {
                    session_id: session_id.cloned(),
                    capture_generation,
                    source_sequence: sequence,
                    state,
                    reason,
                    event_suppression_reason: self
                        .numeric_event_suppression_reason(session_id, capture_generation),
                },
            })?;
        }
        self.try_emit_result(session_id.cloned(), capture_generation, sequence)?;
        Ok(())
    }

    fn observe_numeric_result(
        &mut self,
        sequence: u64,
        monotonic_end_ms: u64,
        observed_song: Option<ScorepeekSongId>,
        observed_clear_type: Option<String>,
        chart_resolution: Option<&ResultChartResolution>,
        performance_resolution: Option<&ResultPerformanceResolution>,
    ) -> Option<(NumericResultTemporalState, NumericResultTransitionReason)> {
        let chronology_reset = self
            .last_numeric_sequence
            .is_some_and(|last| sequence <= last)
            || self
                .last_numeric_monotonic_ms
                .is_some_and(|last| monotonic_end_ms < last);
        if chronology_reset {
            self.reset_numeric_result();
        }
        self.last_numeric_sequence = Some(sequence);
        self.last_numeric_monotonic_ms = Some(monotonic_end_ms);
        let (
            Some(song_id),
            Some(clear_type),
            Some(ResultChartResolution::Accepted {
                chart,
                current_score,
                ..
            }),
            Some(performance @ ResultPerformanceResolution::Accepted { .. }),
        ) = (
            observed_song,
            observed_clear_type,
            chart_resolution,
            performance_resolution,
        )
        else {
            return self.pending_numeric_result.take().map(|_| {
                (
                    NumericResultTemporalState::Unknown,
                    NumericResultTransitionReason::Incomplete,
                )
            });
        };
        let view = NumericResultView {
            song_id,
            clear_type,
            chart: chart.clone(),
            current_score: *current_score,
            performance: performance.clone(),
            source_sequence: sequence,
        };
        if let Some(accepted) = &self.accepted_numeric_result {
            if same_numeric_tuple(accepted, &view) {
                return None;
            }
            self.accepted_numeric_result = None;
        }
        let had_conflict = self
            .pending_numeric_result
            .as_ref()
            .is_some_and(|pending| !same_numeric_tuple(&pending.view, &view));
        match &mut self.pending_numeric_result {
            Some(pending) if same_numeric_tuple(&pending.view, &view) => {
                pending.observations = pending.observations.saturating_add(1);
                pending.view.source_sequence = sequence;
                if pending.observations >= RESULT_TEMPORAL_REQUIRED_OBSERVATIONS {
                    self.accepted_numeric_result = Some(pending.view.clone());
                    self.pending_numeric_result = None;
                    Some((
                        NumericResultTemporalState::Accepted,
                        NumericResultTransitionReason::Accepted,
                    ))
                } else {
                    Some((
                        NumericResultTemporalState::Pending {
                            observations: pending.observations,
                        },
                        NumericResultTransitionReason::CandidateRepeated,
                    ))
                }
            }
            _ => {
                self.pending_numeric_result = Some(PendingNumericResult {
                    view,
                    observations: 1,
                });
                Some((
                    NumericResultTemporalState::Pending { observations: 1 },
                    if chronology_reset {
                        NumericResultTransitionReason::ChronologyReset
                    } else if had_conflict {
                        NumericResultTransitionReason::Conflict
                    } else {
                        NumericResultTransitionReason::CandidateStarted
                    },
                ))
            }
        }
    }

    fn numeric_event_suppression_reason(
        &self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
    ) -> Option<NumericResultEventSuppressionReason> {
        if self.result_event_emitted {
            return Some(NumericResultEventSuppressionReason::AlreadyEmitted);
        }
        if session_id.is_none() || capture_generation.is_none() {
            return Some(NumericResultEventSuppressionReason::SessionUnavailable);
        }
        let Some(numeric) = self.accepted_numeric_result.as_ref() else {
            return Some(NumericResultEventSuppressionReason::NumericNotAccepted);
        };
        let Some(stable_song) = self.stable_result_song.as_ref() else {
            return Some(NumericResultEventSuppressionReason::ResultSongNotStable);
        };
        let Some(stable_clear_type) = self.temporal_result.state().clear_type.stable_value() else {
            return Some(NumericResultEventSuppressionReason::ClearTypeNotStable);
        };
        let Some(accepted_attempt) = self.play_attempt.accepted_result() else {
            return Some(NumericResultEventSuppressionReason::PlayAttemptNotAccepted);
        };
        (stable_song.scorepeek_song_id != numeric.song_id
            || stable_clear_type != &numeric.clear_type
            || accepted_attempt.song.scorepeek_song_id != numeric.song_id)
            .then_some(NumericResultEventSuppressionReason::LinkageConflict)
    }

    fn try_emit_result(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        fallback_sequence: u64,
    ) -> Result<(), String> {
        if self.result_event_emitted {
            return Ok(());
        }
        let (Some(session_id), Some(capture_generation)) = (session_id, capture_generation) else {
            return Ok(());
        };
        let Some(numeric) = self.accepted_numeric_result.as_ref() else {
            return Ok(());
        };
        let Some(stable_song) = self.stable_result_song.as_ref() else {
            return Ok(());
        };
        let Some(stable_clear_type) = self.temporal_result.state().clear_type.stable_value() else {
            return Ok(());
        };
        let Some(accepted_attempt) = self.play_attempt.accepted_result() else {
            return Ok(());
        };
        if stable_song.scorepeek_song_id != numeric.song_id
            || stable_clear_type != &numeric.clear_type
            || accepted_attempt.song.scorepeek_song_id != numeric.song_id
        {
            return Ok(());
        }
        let ResultPerformanceResolution::Accepted {
            judgments,
            miss_count,
            timing,
            combo_break,
            previous_best,
            ..
        } = &numeric.performance
        else {
            unreachable!("accepted numeric view stores accepted performance");
        };
        let result = ResultDomainEvent {
            contract: "scorepeek-result-detected-v2".to_owned(),
            attempt_id: accepted_attempt.attempt_id,
            parent_attempt_id: accepted_attempt.parent_attempt_id,
            scorepeek_song_id: numeric.song_id,
            play_side: "one_player".to_owned(),
            play_mode: "single_play".to_owned(),
            play_type: numeric.chart.key.play_type,
            difficulty: numeric.chart.key.difficulty,
            level: numeric.chart.level,
            notes: numeric.chart.notes,
            current_score: numeric.current_score,
            clear_type: numeric.clear_type.clone(),
            judgments: judgments.clone(),
            miss_count: miss_count.clone(),
            timing: timing.clone(),
            combo_break: combo_break.clone(),
            previous_best: previous_best.clone(),
        };
        let source_sequence = numeric.source_sequence.max(fallback_sequence);
        self.publish_one(&RunEvent {
            schema: "scorepeek-run-event-v2".to_owned(),
            kind: RunEventKind::ResultDetected {
                session_id,
                capture_generation,
                source_sequence,
                result,
            },
        })?;
        self.result_event_emitted = true;
        Ok(())
    }

    fn reset_numeric_result(&mut self) {
        self.pending_numeric_result = None;
        self.accepted_numeric_result = None;
        self.last_numeric_sequence = None;
        self.last_numeric_monotonic_ms = None;
    }

    fn reduce_music_select_observation(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_end_ms: u64,
        presentation: &SongResolutionPresentation,
    ) -> Result<(), String> {
        let selected = match presentation {
            SongResolutionPresentation::Accepted { selected, .. } => Some(selected),
            SongResolutionPresentation::Unknown { .. } => None,
        };
        let Some(update) = self.temporal_music_select.observe(
            sequence,
            monotonic_end_ms,
            selected.map(|song| song.scorepeek_song_id),
        ) else {
            return Ok(());
        };
        match &update.state {
            MusicSelectTemporalState::Empty => {
                self.retained_music_select_song = None;
                self.candidate_music_select_song = None;
            }
            MusicSelectTemporalState::Pending { candidate, .. } => {
                self.retained_music_select_song = None;
                self.candidate_music_select_song = selected
                    .filter(|song| song.scorepeek_song_id == *candidate)
                    .cloned();
            }
            MusicSelectTemporalState::Stable { value, .. } => {
                if let Some(song) = selected.filter(|song| song.scorepeek_song_id == *value) {
                    self.retained_music_select_song = Some(song.clone());
                }
                self.candidate_music_select_song = None;
            }
            MusicSelectTemporalState::HeldUnknown { .. } => {
                self.candidate_music_select_song = None;
            }
            MusicSelectTemporalState::Changing { candidate, .. } => {
                self.candidate_music_select_song = selected
                    .filter(|song| song.scorepeek_song_id == *candidate)
                    .cloned();
            }
        }
        let temporal_state = update.state.clone();
        let play_attempt_updates = match &temporal_state {
            MusicSelectTemporalState::Stable { .. } => self.play_attempt.observe_selection(
                self.retained_music_select_song.clone(),
                Some(SelectionSource::Stable),
                sequence,
            ),
            MusicSelectTemporalState::HeldUnknown { .. } => self.play_attempt.observe_selection(
                self.retained_music_select_song.clone(),
                Some(SelectionSource::LastConfirmedHeld),
                sequence,
            ),
            MusicSelectTemporalState::Empty => {
                self.play_attempt.observe_selection(None, None, sequence)
            }
            MusicSelectTemporalState::Pending { .. }
            | MusicSelectTemporalState::Changing { .. } => self
                .play_attempt
                .observe_selection_candidate(self.candidate_music_select_song.as_ref())
                .into_iter()
                .collect(),
        };
        self.publish_one(&RunEvent {
            schema: "scorepeek-run-event-v2".to_owned(),
            kind: RunEventKind::TemporalMusicSelectChanged {
                session_id: session_id.cloned(),
                capture_generation,
                source_sequence: Some(sequence),
                reasons: update.reasons,
                state: update.state,
                retained_song: self.retained_music_select_song.clone(),
                candidate_song: self.candidate_music_select_song.clone(),
            },
        })?;
        for state in play_attempt_updates {
            self.publish_play_attempt_update(
                session_id.cloned(),
                capture_generation,
                Some(sequence),
                state,
            )?;
        }
        Ok(())
    }

    fn publish_screen_change(&mut self, event: &RunEvent) -> Result<(), String> {
        let RunEventKind::ScreenChanged {
            session_id,
            capture_generation,
            sequence,
            screen,
            ..
        } = &event.kind
        else {
            unreachable!("screen change dispatcher preserves event kind");
        };
        self.latest_screen_boundary_sequence = Some(*sequence);
        self.publish_one(event)?;
        if let Some(attempt_screen) = play_attempt_screen(screen)
            && let Some(state) = self.play_attempt.observe_screen(attempt_screen, *sequence)
        {
            self.publish_play_attempt_update(
                session_id.clone(),
                *capture_generation,
                Some(*sequence),
                state,
            )?;
        }
        if screen != "result"
            && let Some(update) = self
                .temporal_result
                .reset(TemporalTransitionReason::ResetByScreenChange)
        {
            self.stable_result_song = None;
            self.reset_numeric_result();
            self.result_event_emitted = false;
            self.publish_temporal_update(
                session_id.clone(),
                *capture_generation,
                Some(*sequence),
                update,
            )?;
        }
        if screen != "music_select"
            && let Some(update) = self
                .temporal_music_select
                .reset(MusicSelectTemporalTransitionReason::ResetByScreenChange)
        {
            self.retained_music_select_song = None;
            self.candidate_music_select_song = None;
            self.publish_music_select_temporal_update(
                session_id.clone(),
                *capture_generation,
                Some(*sequence),
                update,
            )?;
        }
        Ok(())
    }

    fn publish_session_finished(&mut self, event: &RunEvent) -> Result<(), String> {
        let RunEventKind::SessionFinished {
            session_id,
            capture_generation,
            ..
        } = &event.kind
        else {
            unreachable!("session-finished dispatcher preserves event kind");
        };
        self.publish_one(event)?;
        if let Some(state) = self.play_attempt.finish_session() {
            self.publish_play_attempt_update(
                Some(session_id.clone()),
                Some(*capture_generation),
                None,
                state,
            )?;
        }
        if let Some(update) = self
            .temporal_result
            .reset(TemporalTransitionReason::ResetBySessionBoundary)
        {
            self.stable_result_song = None;
            self.publish_temporal_update(
                Some(session_id.clone()),
                Some(*capture_generation),
                None,
                update,
            )?;
        }
        if let Some(update) = self
            .temporal_music_select
            .reset(MusicSelectTemporalTransitionReason::ResetBySessionBoundary)
        {
            self.retained_music_select_song = None;
            self.candidate_music_select_song = None;
            self.publish_music_select_temporal_update(
                Some(session_id.clone()),
                Some(*capture_generation),
                None,
                update,
            )?;
        }
        self.completed_event_artifact =
            self.event_worker.take().map(RunEventArtifactWorker::finish);
        Ok(())
    }

    fn publish_temporal_update(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        source_sequence: Option<u64>,
        update: scorepeek::temporal_recognition::ResultTemporalUpdate<
            scorepeek::catalog::ScorepeekSongId,
        >,
    ) -> Result<(), String> {
        self.publish_one(&RunEvent {
            schema: "scorepeek-run-event-v2".to_owned(),
            kind: RunEventKind::TemporalResultChanged {
                session_id,
                capture_generation,
                source_sequence,
                transitions: update.transitions,
                state: update.state,
                stable_song: self.stable_result_song.clone(),
            },
        })
    }

    fn publish_music_select_temporal_update(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        source_sequence: Option<u64>,
        update: scorepeek::temporal_recognition::MusicSelectTemporalUpdate<
            scorepeek::catalog::ScorepeekSongId,
        >,
    ) -> Result<(), String> {
        self.publish_one(&RunEvent {
            schema: "scorepeek-run-event-v2".to_owned(),
            kind: RunEventKind::TemporalMusicSelectChanged {
                session_id,
                capture_generation,
                source_sequence,
                reasons: update.reasons,
                state: update.state,
                retained_song: self.retained_music_select_song.clone(),
                candidate_song: self.candidate_music_select_song.clone(),
            },
        })
    }

    fn publish_play_attempt_update(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        source_sequence: Option<u64>,
        state: PlayAttemptState<SongPresentation>,
    ) -> Result<(), String> {
        self.publish_one(&RunEvent {
            schema: "scorepeek-run-event-v2".to_owned(),
            kind: RunEventKind::PlayAttemptChanged {
                session_id: session_id.clone(),
                capture_generation,
                source_sequence,
                state,
            },
        })?;
        if let Some(sequence) = source_sequence {
            self.try_emit_result(session_id, capture_generation, sequence)?;
        }
        Ok(())
    }

    fn publish_one(&mut self, event: &RunEvent) -> Result<(), String> {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let mut value = event.to_value()?;
        if let Some(object) = value.as_object_mut() {
            object.insert("channel_sequence".to_owned(), sequence.into());
        }
        if let Some(worker) = &mut self.event_worker {
            worker.try_record(&value);
        }
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            state.reduce(event, &value);
            state.next_channel_sequence = self.next_sequence;
        }
        self.channel.publish(&value)?;
        self.refresh()
    }

    pub fn take_completed_event_artifact(&mut self) -> Option<RunEventArtifactOutcome> {
        self.completed_event_artifact.take()
    }

    pub fn watcher_state(
        &mut self,
        state_name: &str,
        session_id: Option<&str>,
        generation: Option<u64>,
        message: &str,
    ) -> Result<(), String> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            state_name.clone_into(&mut state.watcher_state);
            state.active_session_id = session_id.map(ToOwned::to_owned);
            state.capture_generation = generation;
            message.clone_into(&mut state.message);
        }
        self.refresh()
    }

    pub fn warning(&mut self, message: impl Into<String>) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .message = message.into();
        self.refresh()
    }

    pub fn status_recording_degraded(&mut self) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .status_recording = "degraded";
        self.refresh()
    }

    fn refresh(&mut self) -> Result<(), String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .clone();
        match &mut self.display {
            Display::Tui(terminal) => {
                terminal.draw(&state, &self.channel.socket_path, &self.channel.health)
            }
            Display::Plain { output, last_line } => {
                let line = plain_status_line(&state, &self.channel.health);
                if last_line.as_deref() != Some(&line) {
                    writeln!(output, "{line}")
                        .and_then(|()| output.flush())
                        .map_err(|error| format!("plain run output failed: {error}"))?;
                    *last_line = Some(line);
                }
                Ok(())
            }
        }
    }
}

fn play_attempt_screen(screen: &str) -> Option<PlayAttemptScreen> {
    match screen {
        "music_select" => Some(PlayAttemptScreen::MusicSelect),
        "decide_transition" => Some(PlayAttemptScreen::DecideTransition),
        "play" => Some(PlayAttemptScreen::Play),
        "result" => Some(PlayAttemptScreen::Result),
        "unknown" => Some(PlayAttemptScreen::Unknown),
        _ => None,
    }
}

fn plain_status_line(state: &RunViewState, health: &ChannelHealth) -> String {
    let channel = health.value();
    format!(
        "scorepeek: state={} sessions={} session={} generation={} channel={} clients={} dropped={} disconnected={} message={}",
        state.watcher_state,
        state.session_count,
        state.active_session_id.as_deref().unwrap_or("-"),
        state
            .capture_generation
            .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        channel["status"].as_str().unwrap_or("degraded"),
        channel["connected_clients"].as_u64().unwrap_or(0),
        channel["dropped_events"].as_u64().unwrap_or(0),
        channel["disconnected_clients"].as_u64().unwrap_or(0),
        state.message,
    )
}

fn render(
    frame: &mut ratatui::Frame<'_>,
    state: &RunViewState,
    socket_path: &Path,
    health: &ChannelHealth,
) {
    let area = frame.area();
    let compact = area.width < 80 || area.height < 32;
    let show_attempt = !compact || state.latest_play_attempt.is_some();
    let has_results = !state.result_history.is_empty();
    if compact && has_results {
        let results = result_history_lines(
            state,
            true,
            area.width.saturating_sub(2) as usize,
            area.height.saturating_sub(2) as usize,
        );
        frame.render_widget(
            Paragraph::new(results).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Accepted play events"),
            ),
            area,
        );
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(result_panel_constraints(compact, show_attempt, has_results))
        .split(area);
    let results = result_history_lines(
        state,
        compact,
        rows[0].width.saturating_sub(2) as usize,
        rows[0].height.saturating_sub(2) as usize,
    );
    frame.render_widget(
        Paragraph::new(results).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Accepted play events"),
        ),
        rows[0],
    );

    let header = watcher_lines(state, compact, rows[1].width.saturating_sub(2) as usize);
    frame.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Debug: Watcher"),
        ),
        rows[1],
    );

    let mut row = 2;
    if show_attempt {
        let attempt = play_attempt_lines(
            state.latest_play_attempt.as_ref(),
            compact,
            rows[row].width.saturating_sub(2) as usize,
        );
        frame.render_widget(
            Paragraph::new(attempt).wrap(Wrap { trim: false }).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Debug: Play attempt"),
            ),
            rows[row],
        );
        row += 1;
    }

    let observation = observation_lines(
        state.current_screen.as_deref(),
        state.latest_observation.as_ref(),
        state.latest_stabilized_result.as_ref(),
        state.latest_temporal_music_select.as_ref(),
        compact,
        rows[row].width.saturating_sub(2) as usize,
    );
    frame.render_widget(
        Paragraph::new(observation)
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Debug: Latest recognition"),
            ),
        rows[row],
    );
    row += 1;

    let footer = channel_lines(state, socket_path, health, compact);
    frame.render_widget(
        Paragraph::new(footer).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Debug: Observation channel"),
        ),
        rows[row],
    );
}

fn result_panel_constraints(
    compact: bool,
    show_attempt: bool,
    has_results: bool,
) -> Vec<Constraint> {
    if !has_results && compact && show_attempt {
        vec![
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Min(6),
            Constraint::Length(3),
        ]
    } else if !has_results && compact {
        vec![
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(3),
        ]
    } else if !has_results && show_attempt {
        vec![
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(9),
            Constraint::Min(13),
            Constraint::Length(5),
        ]
    } else if !has_results {
        vec![
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Min(14),
            Constraint::Length(5),
        ]
    } else if compact && show_attempt {
        vec![
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Length(6),
            Constraint::Min(8),
            Constraint::Length(3),
        ]
    } else if compact {
        vec![
            Constraint::Min(8),
            Constraint::Length(5),
            Constraint::Min(6),
            Constraint::Length(3),
        ]
    } else if show_attempt {
        vec![
            Constraint::Min(14),
            Constraint::Length(5),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(5),
        ]
    } else {
        vec![
            Constraint::Min(14),
            Constraint::Length(5),
            Constraint::Length(8),
            Constraint::Length(5),
        ]
    }
}

fn result_history_lines(
    state: &RunViewState,
    compact: bool,
    available_width: usize,
    available_height: usize,
) -> Vec<Line<'static>> {
    if state.result_history.is_empty() {
        return vec![
            Line::from(Span::styled(
                "No accepted play event yet",
                Style::default().fg(Color::DarkGray),
            )),
            Line::from("Only scorepeek-result-detected-v2 events appear here."),
        ];
    }

    let mut entries = Vec::new();
    let mut used_lines = 1_usize;
    for entry in state.result_history.iter().rev() {
        let entry_lines = result_history_entry_lines(entry, compact, available_width);
        if !entries.is_empty() && used_lines.saturating_add(entry_lines.len()) > available_height {
            break;
        }
        used_lines = used_lines.saturating_add(entry_lines.len());
        entries.push(entry_lines);
    }
    let shown = entries.len();
    let mut lines = vec![Line::from(format!(
        "{} completed  |  newest first  |  showing {}  |  retained {}/{}",
        state.result_count,
        shown,
        state.result_history.len(),
        RESULT_HISTORY_CAPACITY
    ))];
    lines.extend(entries.into_iter().flatten());
    lines
}

fn result_history_entry_lines(
    entry: &ResultHistoryEntry,
    compact: bool,
    available_width: usize,
) -> Vec<Line<'static>> {
    if compact {
        compact_result_history_entry_lines(entry, available_width)
    } else {
        expanded_result_history_entry_lines(entry, available_width)
    }
}

fn compact_result_history_entry_lines(
    entry: &ResultHistoryEntry,
    available_width: usize,
) -> Vec<Line<'static>> {
    let result = &entry.result;
    let maximum_score = u64::from(result.notes) * 2;
    let percentage_tenths = u64::from(result.current_score)
        .checked_mul(1_000)
        .and_then(|value| value.checked_div(maximum_score))
        .unwrap_or(0);
    let title = entry
        .song
        .as_ref()
        .and_then(|song| song.display_titles.first())
        .map_or_else(
            || result.scorepeek_song_id.as_uuid().to_string(),
            ToOwned::to_owned,
        );
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("#{} {}", entry.ordinal, result.clear_type),
            Style::default()
                .fg(clear_type_color(&result.clear_type))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  {} {} Lv{}",
            play_type_label(result.play_type),
            difficulty_label(result.difficulty),
            result.level,
        )),
    ])];
    lines.extend(packed_token_lines(
        [
            format!("EX {}", grouped_u32(result.current_score)),
            format!("/ {}", grouped_u64(maximum_score)),
            format!(
                "({}.{:01}%)",
                percentage_tenths / 10,
                percentage_tenths % 10
            ),
        ],
        available_width,
    ));
    lines.push(Line::from(fitted_value("Title: ", &title, available_width)));
    lines.extend(packed_token_lines(
        [
            format!("PG {}", grouped_u32(result.judgments.pgreat)),
            format!("GR {}", grouped_u32(result.judgments.great)),
            format!("GD {}", grouped_u32(result.judgments.good)),
            format!("BD {}", grouped_u32(result.judgments.bad)),
            format!("PR {}", grouped_u32(result.judgments.poor)),
        ],
        available_width,
    ));
    lines.extend(packed_token_lines(
        [
            format!("MISS {}", supplemental_u32(&result.miss_count)),
            format!("F {}", supplemental_u32(&result.timing.fast)),
            format!("S {}", supplemental_u32(&result.timing.slow)),
            format!("CB {}", supplemental_u32(&result.combo_break)),
        ],
        available_width,
    ));
    let previous_clear = previous_text(&result.previous_best.clear_type);
    let previous_score = previous_u32(&result.previous_best.score);
    let previous_miss = previous_u32(&result.previous_best.miss_count);
    lines.extend(packed_token_lines(
        [
            "PREV".to_owned(),
            format!("clear={previous_clear}"),
            format!("EX={previous_score}"),
            format!("MISS={previous_miss}"),
        ],
        available_width,
    ));
    lines
}

fn packed_token_lines<const N: usize>(
    tokens: [String; N],
    available_width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for token in tokens {
        let separator = usize::from(!current.is_empty());
        if !current.is_empty()
            && Line::raw(&current)
                .width()
                .saturating_add(separator)
                .saturating_add(Line::raw(&token).width())
                > available_width
        {
            lines.push(Line::from(current));
            current = token;
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(&token);
        }
    }
    if !current.is_empty() {
        lines.push(Line::from(current));
    }
    lines
}

fn expanded_result_history_entry_lines(
    entry: &ResultHistoryEntry,
    available_width: usize,
) -> Vec<Line<'static>> {
    let result = &entry.result;
    let maximum_score = u64::from(result.notes) * 2;
    let percentage_tenths = u64::from(result.current_score)
        .checked_mul(1_000)
        .and_then(|value| value.checked_div(maximum_score))
        .unwrap_or(0);
    let title = entry
        .song
        .as_ref()
        .and_then(|song| song.display_titles.first())
        .map_or_else(
            || result.scorepeek_song_id.as_uuid().to_string(),
            ToOwned::to_owned,
        );
    vec![
        Line::from(vec![
            Span::styled(
                format!("#{} {}", entry.ordinal, result.clear_type),
                Style::default()
                    .fg(clear_type_color(&result.clear_type))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  {} {} Lv{}  EX SCORE {} / {} ({}.{:01}%)",
                play_type_label(result.play_type),
                difficulty_label(result.difficulty),
                result.level,
                grouped_u32(result.current_score),
                grouped_u64(maximum_score),
                percentage_tenths / 10,
                percentage_tenths % 10,
            )),
        ]),
        Line::from(fitted_value("Title: ", &title, available_width)),
        Line::from(format!(
            "PGREAT {}  GREAT {}  GOOD {}  BAD {}  POOR {}",
            grouped_u32(result.judgments.pgreat),
            grouped_u32(result.judgments.great),
            grouped_u32(result.judgments.good),
            grouped_u32(result.judgments.bad),
            grouped_u32(result.judgments.poor),
        )),
        Line::from(format!(
            "MISS {}  FAST {}  SLOW {}  COMBO BREAK {}",
            supplemental_u32(&result.miss_count),
            supplemental_u32(&result.timing.fast),
            supplemental_u32(&result.timing.slow),
            supplemental_u32(&result.combo_break),
        )),
        Line::from(format!(
            "Previous: clear={}  EX SCORE {}  MISS {}",
            previous_text(&result.previous_best.clear_type),
            previous_u32(&result.previous_best.score),
            previous_u32(&result.previous_best.miss_count),
        )),
        Line::from(fitted_value(
            "Artist: ",
            entry.song.as_ref().map_or("-", |song| song.artist.as_str()),
            available_width,
        )),
        Line::from(format!(
            "attempt=#{} parent=#{}  notes={} side={} mode={} sequence={}",
            result.attempt_id,
            result
                .parent_attempt_id
                .map_or_else(|| "-".to_owned(), |value| format!("{value}")),
            grouped_u32(result.notes),
            result.play_side,
            result.play_mode,
            entry.source_sequence,
        )),
    ]
}

const fn clear_type_color(clear_type: &str) -> Color {
    match clear_type.as_bytes() {
        b"FAILED" => Color::Red,
        b"ASSIST CLEAR" | b"EASY CLEAR" => Color::Yellow,
        b"CLEAR" | b"HARD CLEAR" | b"EXH-CLEAR" | b"F-COMBO" => Color::Green,
        _ => Color::White,
    }
}

const fn play_type_label(play_type: PlayType) -> &'static str {
    match play_type {
        PlayType::Single => "SP",
        PlayType::Double => "DP",
    }
}

const fn difficulty_label(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Beginner => "BEGINNER",
        Difficulty::Normal => "NORMAL",
        Difficulty::Hyper => "HYPER",
        Difficulty::Another => "ANOTHER",
        Difficulty::Leggendaria => "LEGGENDARIA",
    }
}

fn grouped_u32(value: u32) -> String {
    grouped_u64(u64::from(value))
}

fn grouped_u64(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn supplemental_u32(value: &SupplementalResultValue<u32>) -> String {
    match value {
        SupplementalResultValue::Known { value } => grouped_u32(*value),
        SupplementalResultValue::NotDisplayed => "--".to_owned(),
        SupplementalResultValue::Unknown { .. } => "?".to_owned(),
    }
}

fn previous_u32(value: &PreviousBestValue<u32>) -> String {
    match value {
        PreviousBestValue::Known { value } => grouped_u32(*value),
        PreviousBestValue::NotPlayed => "NO PLAY".to_owned(),
        PreviousBestValue::NotDisplayed => "--".to_owned(),
        PreviousBestValue::Unknown { .. } => "?".to_owned(),
    }
}

fn previous_text(value: &PreviousBestValue<String>) -> String {
    match value {
        PreviousBestValue::Known { value } => value.clone(),
        PreviousBestValue::NotPlayed => "NO PLAY".to_owned(),
        PreviousBestValue::NotDisplayed => "--".to_owned(),
        PreviousBestValue::Unknown { .. } => "?".to_owned(),
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the bounded TUI panel keeps all play-attempt display priorities in one formatter"
)]
fn play_attempt_lines(
    event: Option<&Value>,
    compact: bool,
    available_width: usize,
) -> Vec<Line<'static>> {
    let Some(event) = event else {
        return vec![Line::from("No play attempt yet")];
    };
    let state = event.get("state").unwrap_or(&Value::Null);
    match text_at(state, "/status").as_str() {
        "armed" => {
            let selected = state.get("selected_song").unwrap_or(&Value::Null);
            let mut lines = vec![Line::from(Span::styled(
                "SELECTION ARMED",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ))];
            lines.push(Line::from(fitted_value(
                "Song: ",
                &joined_at(selected, "/display_titles"),
                available_width,
            )));
            lines.push(Line::from(fitted_value(
                "Artist: ",
                &text_at(selected, "/artist"),
                available_width,
            )));
            if !compact {
                lines.push(Line::from(format!(
                    "source={}  sequence={}  song-id={}",
                    text_at(state, "/selection_source"),
                    text_at(state, "/source_sequence"),
                    text_at(selected, "/scorepeek_song_id")
                )));
            }
            lines
        }
        "unlinked_result" => {
            let mut lines = vec![Line::from(Span::styled(
                "UNLINKED RESULT",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ))];
            lines.push(Line::from(format!(
                "reason={}  sequence={}",
                text_at(state, "/reason"),
                text_at(state, "/source_sequence")
            )));
            if !state["result_song"].is_null() {
                lines.push(Line::from(fitted_value(
                    "Result title: ",
                    &joined_at(state, "/result_song/display_titles"),
                    available_width,
                )));
                lines.push(Line::from(fitted_value(
                    "Result artist: ",
                    &text_at(state, "/result_song/artist"),
                    available_width,
                )));
            }
            lines
        }
        "attempt" => {
            let attempt = state.get("attempt").unwrap_or(&Value::Null);
            let phase = text_at(attempt, "/phase");
            let relation = text_at(attempt, "/result_relation");
            let reasons = joined_at(attempt, "/reasons");
            let (label, color) = match relation.as_str() {
                "confirmed" => ("CONFIRMED", Color::Green),
                "conflict" => ("CONFLICT", Color::Red),
                "pending" => ("RESULT PENDING", Color::Yellow),
                _ if phase == "playing" => ("PLAYING", Color::Cyan),
                _ if phase == "decided" && attempt["selected_song"].is_object() => {
                    ("SELECTED", Color::Cyan)
                }
                _ => ("INCOMPLETE", Color::Yellow),
            };
            let mut lines = vec![Line::from(vec![
                Span::styled(
                    label,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!("  phase={phase}")),
            ])];
            let selected = attempt.get("selected_song").unwrap_or(&Value::Null);
            if selected.is_object() {
                lines.push(Line::from(fitted_value(
                    "Song: ",
                    &joined_at(selected, "/display_titles"),
                    available_width,
                )));
                lines.push(Line::from(fitted_value(
                    "Artist: ",
                    &text_at(selected, "/artist"),
                    available_width,
                )));
            } else {
                lines.push(Line::from("Song: unknown"));
            }
            if relation == "conflict" {
                lines.push(Line::from(fitted_value(
                    "Result: ",
                    &joined_at(attempt, "/result_song/display_titles"),
                    available_width,
                )));
            }
            if !compact {
                lines.push(Line::from(format!(
                    "attempt=#{}  parent=#{}  source={}  song-id={}",
                    text_at(attempt, "/attempt_id"),
                    text_at(attempt, "/parent_attempt_id"),
                    text_at(attempt, "/selection_source"),
                    text_at(selected, "/scorepeek_song_id")
                )));
                lines.push(Line::from(format!(
                    "path select={} decide={} play={} result={}",
                    text_at(attempt, "/path/select_observed"),
                    text_at(attempt, "/path/decide_observed"),
                    text_at(attempt, "/path/play_observed"),
                    text_at(attempt, "/path/result_observed")
                )));
            }
            if !reasons.is_empty() && reasons != "-" {
                lines.push(Line::from(fitted_value(
                    "reason=",
                    &reasons,
                    available_width,
                )));
            }
            lines
        }
        _ => vec![Line::from("No play attempt yet")],
    }
}

fn watcher_lines(
    state: &RunViewState,
    compact: bool,
    available_width: usize,
) -> Vec<Line<'static>> {
    if compact {
        return vec![
            Line::from(vec![
                Span::styled(
                    "scorepeek run",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(
                    "  state={} sessions={} generation={}",
                    state.watcher_state,
                    state.session_count,
                    state
                        .capture_generation
                        .map_or_else(|| "-".to_owned(), |value| value.to_string())
                )),
            ]),
            Line::from(fitted_value(
                "invocation=",
                &state.invocation_id,
                available_width,
            )),
            Line::from(format!(
                "session={}  recording={}  status={}",
                state.active_session_id.as_deref().unwrap_or("-"),
                state.recording,
                state.status_recording
            )),
        ];
    }
    vec![
        Line::from(vec![
            Span::styled(
                "scorepeek run",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  invocation={}  state={}",
                state.invocation_id, state.watcher_state
            )),
        ]),
        Line::from(format!(
            "sessions={}  session={}  generation={}",
            state.session_count,
            state.active_session_id.as_deref().unwrap_or("-"),
            state
                .capture_generation
                .map_or_else(|| "-".to_owned(), |value| value.to_string())
        )),
        Line::from(format!("profile={}", state.profile_sha256)),
        Line::from(format!(
            "recording={}  status-recording={}",
            state.recording, state.status_recording
        )),
        Line::from(format!("message={}", state.message)),
    ]
}

fn channel_lines(
    state: &RunViewState,
    socket_path: &Path,
    health: &ChannelHealth,
    compact: bool,
) -> Vec<Line<'static>> {
    let channel = health.value();
    let mut lines = vec![
        Line::from(format!("socket={}", socket_path.display())),
        Line::from(format!(
            "channel={} clients={} dropped={} disconnected={}",
            channel["status"].as_str().unwrap_or("degraded"),
            channel["connected_clients"],
            channel["dropped_events"],
            channel["disconnected_clients"]
        )),
    ];
    if !compact {
        lines.push(Line::from(format!(
            "diagnostic={}  artifact={}  field-worker={}",
            state.latest_report.as_ref().map_or_else(
                || "-".to_owned(),
                |report| text_at(report, "/diagnostic_completeness")
            ),
            state.latest_report.as_ref().map_or_else(
                || "-".to_owned(),
                |report| text_at(report, "/recognition_artifact_status")
            ),
            state.latest_report.as_ref().map_or_else(
                || "-".to_owned(),
                |report| text_at(report, "/field_worker_status")
            ),
        )));
        lines.push(Line::from(format!(
            "screen ticks={} field-busy-skips={} max-consecutive={} dropped-diagnostic-facts={}",
            state.latest_report.as_ref().map_or_else(
                || "-".to_owned(),
                |report| text_at(report, "/recognition_ticks")
            ),
            state.latest_report.as_ref().map_or_else(
                || "-".to_owned(),
                |report| text_at(report, "/field_observation_busy_skips")
            ),
            state.latest_report.as_ref().map_or_else(
                || "-".to_owned(),
                |report| text_at(report, "/maximum_consecutive_field_observation_busy_skips")
            ),
            state.latest_report.as_ref().map_or_else(
                || "-".to_owned(),
                |report| text_at(report, "/dropped_capture_diagnostic_facts")
            ),
        )));
    }
    lines.push(Line::from("Ctrl-C / SIGTERM: stop scorepeek"));
    lines
}

fn observation_lines(
    current_screen: Option<&str>,
    observation: Option<&Value>,
    stabilized_result: Option<&Value>,
    temporal_music_select: Option<&Value>,
    compact: bool,
    available_width: usize,
) -> Vec<Line<'static>> {
    let Some(observation) = observation else {
        return vec![Line::from(format!(
            "screen={}  No field observation yet",
            current_screen.unwrap_or("-")
        ))];
    };
    let raw_screen = text_at(observation, "/screen");
    if current_screen == Some("mode_select") && raw_screen != "mode_select" {
        return vec![Line::from(format!(
            "screen={}  No field observation for this screen",
            current_screen.unwrap_or("-")
        ))];
    }
    let mut lines = vec![Line::from(format!(
        "current screen={}  raw screen={}  sequence={}  interval={}..{} ms",
        current_screen.unwrap_or(&raw_screen),
        raw_screen,
        text_at(observation, "/sequence"),
        text_at(observation, "/monotonic_start_ms"),
        text_at(observation, "/monotonic_end_ms"),
    ))];
    lines.extend(stabilized_result_lines(stabilized_result, available_width));
    if raw_screen == "music_select" {
        lines.extend(temporal_music_select_lines(
            temporal_music_select,
            available_width,
        ));
    }
    let ocr_lines = ocr_lines(observation, available_width);
    let resolution = observation
        .get("song_resolution_presentation")
        .unwrap_or(&Value::Null);
    let mut resolution_lines = vec![Line::from(vec![
        Span::styled(
            format!("{} ", text_at(resolution, "/status").to_uppercase()),
            Style::default()
                .fg(if resolution["status"] == "accepted" {
                    Color::Green
                } else {
                    Color::Yellow
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(text_at(resolution, "/reason")),
    ])];
    let unknown = resolution["status"] == "unknown";
    let title_label = if unknown {
        "Candidate title: "
    } else {
        "Catalog title: "
    };
    let artist_label = if unknown {
        "Candidate artist: "
    } else {
        "Catalog artist: "
    };
    resolution_lines.push(Line::from(fitted_value(
        title_label,
        &joined_at(resolution, "/selected/display_titles"),
        available_width,
    )));
    resolution_lines.push(Line::from(fitted_value(
        artist_label,
        &text_at(resolution, "/selected/artist"),
        available_width,
    )));
    if compact {
        lines.extend(resolution_lines);
        lines.extend(ocr_lines);
        return lines;
    }
    lines.extend(ocr_lines);
    lines.extend(resolution_lines);
    lines.push(Line::from(format!(
        "Song ID: {}",
        text_at(resolution, "/selected/scorepeek_song_id")
    )));
    lines.push(Line::from(format!(
        "Evidence: {}",
        text_at(resolution, "/evidence_summary")
    )));
    lines.push(Line::from(format!(
        "Runner-up: {}",
        joined_at(resolution, "/runner_up/display_titles")
    )));
    lines
}

fn temporal_music_select_lines(
    temporal: Option<&Value>,
    available_width: usize,
) -> Vec<Line<'static>> {
    let Some(temporal) = temporal else {
        return Vec::new();
    };
    let status = text_at(temporal, "/state/status");
    let label = match status.as_str() {
        "stable" => "STABLE",
        "held_unknown" => "HELD",
        "changing" => "CHANGING",
        "pending" => "PENDING",
        _ => "EMPTY",
    };
    let color = match label {
        "STABLE" => Color::Green,
        "HELD" | "CHANGING" | "PENDING" => Color::Yellow,
        _ => Color::DarkGray,
    };
    let mut lines = vec![Line::from(Span::styled(
        format!("Temporal selection: {label}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))];
    if matches!(label, "STABLE" | "HELD" | "CHANGING") {
        lines.push(Line::from(fitted_value(
            if label == "HELD" {
                "Last confirmed title: "
            } else {
                "Stable title: "
            },
            &joined_at(temporal, "/retained_song/display_titles"),
            available_width,
        )));
        lines.push(Line::from(fitted_value(
            if label == "HELD" {
                "Last confirmed artist: "
            } else {
                "Stable artist: "
            },
            &text_at(temporal, "/retained_song/artist"),
            available_width,
        )));
    }
    if matches!(label, "PENDING" | "CHANGING") {
        lines.push(Line::from(fitted_value(
            "Pending title: ",
            &joined_at(temporal, "/candidate_song/display_titles"),
            available_width,
        )));
        lines.push(Line::from(fitted_value(
            "Pending artist: ",
            &text_at(temporal, "/candidate_song/artist"),
            available_width,
        )));
    }
    lines
}

fn stabilized_result_lines(
    stabilized: Option<&Value>,
    available_width: usize,
) -> Vec<Line<'static>> {
    let Some(stabilized) = stabilized else {
        return Vec::new();
    };
    let song_status = text_at(stabilized, "/state/song/status");
    let clear_status = text_at(stabilized, "/state/clear_type/status");
    let status = if song_status == "conflict" || clear_status == "conflict" {
        "CONFLICT"
    } else if song_status == "stable" && clear_status == "stable" {
        "STABLE"
    } else if song_status == "pending" || clear_status == "pending" {
        "PENDING"
    } else {
        "EMPTY"
    };
    let color = match status {
        "STABLE" => Color::Green,
        "CONFLICT" => Color::Red,
        _ => Color::Yellow,
    };
    let song_progress = if song_status == "pending" || song_status == "stable" {
        format!(
            "{}/{}",
            text_at(stabilized, "/state/song/evidence/count"),
            text_at(stabilized, "/state/song/evidence/required")
        )
    } else {
        song_status.clone()
    };
    let clear_progress = if clear_status == "pending" || clear_status == "stable" {
        format!(
            "{}/{}",
            text_at(stabilized, "/state/clear_type/evidence/count"),
            text_at(stabilized, "/state/clear_type/evidence/required")
        )
    } else {
        clear_status.clone()
    };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("Stabilized result: {status}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  song={song_progress} clear={clear_progress}")),
    ])];
    if status == "STABLE" {
        lines.push(Line::from(fitted_value(
            "Stable title: ",
            &joined_at(stabilized, "/stable_song/display_titles"),
            available_width,
        )));
        lines.push(Line::from(fitted_value(
            "Stable artist: ",
            &text_at(stabilized, "/stable_song/artist"),
            available_width,
        )));
        lines.push(Line::from(format!(
            "Stable clear: {}",
            text_at(stabilized, "/state/clear_type/value")
        )));
    }
    lines
}

fn ocr_lines(observation: &Value, available_width: usize) -> Vec<Line<'static>> {
    match observation.get("screen").and_then(Value::as_str) {
        Some("result") => vec![
            Line::from(fitted_value(
                "OCR title: ",
                &text_at(observation, "/fields/title"),
                available_width,
            )),
            Line::from(fitted_value(
                "OCR artist: ",
                &text_at(observation, "/fields/artist"),
                available_width,
            )),
            Line::from(format!(
                "OCR clear: {}",
                text_at(observation, "/fields/clear_type_ocr")
            )),
        ],
        Some("music_select") => vec![
            Line::from(fitted_value(
                "OCR active title: ",
                &text_at(observation, "/fields/active_list_title"),
                available_width,
            )),
            Line::from(fitted_value(
                "OCR central title: ",
                &text_at(observation, "/fields/central_title"),
                available_width,
            )),
            Line::from(fitted_value(
                "OCR artist: ",
                &text_at(observation, "/fields/artist"),
                available_width,
            )),
        ],
        _ => Vec::new(),
    }
}

fn fitted_value(prefix: &str, value: &str, available_width: usize) -> String {
    let prefix_width = Line::raw(prefix).width();
    if prefix_width >= available_width {
        return prefix.to_owned();
    }
    let maximum = available_width - prefix_width;
    if Line::raw(value).width() <= maximum {
        return format!("{prefix}{value}");
    }
    let ellipsis_width = Line::raw("…").width();
    let mut truncated = String::new();
    for character in value.chars() {
        let mut candidate = truncated.clone();
        candidate.push(character);
        if Line::raw(&candidate).width().saturating_add(ellipsis_width) > maximum {
            break;
        }
        truncated.push(character);
    }
    format!("{prefix}{truncated}…")
}

fn text_at(value: &Value, pointer: &str) -> String {
    match value.pointer(pointer) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => "-".to_owned(),
    }
}

fn joined_at(value: &Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" / ")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "-".to_owned())
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Write};
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use ratatui::backend::TestBackend;

    use super::*;

    fn state() -> Arc<Mutex<RunViewState>> {
        Arc::new(Mutex::new(RunViewState::new(
            "invocation-1".to_owned(),
            "a".repeat(64),
            true,
        )))
    }

    fn test_output(state: Arc<Mutex<RunViewState>>, channel: ObservationChannel) -> RoutineOutput {
        RoutineOutput {
            state,
            channel,
            display: Display::Plain {
                output: BufWriter::new(io::stdout()),
                last_line: None,
            },
            next_sequence: 1,
            temporal_result: ResultTemporalReducer::new(
                TemporalPolicy::for_episode(RESULT_TEMPORAL_REQUIRED_OBSERVATIONS).unwrap(),
            ),
            stable_result_song: None,
            temporal_music_select: MusicSelectTemporalReducer::new(
                MusicSelectTemporalPolicy::new(200, 200, 250).unwrap(),
            ),
            retained_music_select_song: None,
            candidate_music_select_song: None,
            play_attempt: PlayAttemptReducer::default(),
            pending_numeric_result: None,
            accepted_numeric_result: None,
            last_numeric_sequence: None,
            last_numeric_monotonic_ms: None,
            result_event_emitted: false,
            latest_screen_boundary_sequence: None,
            event_store: None,
            event_worker: None,
            completed_event_artifact: None,
        }
    }

    fn accepted_result_event(sequence: u64) -> RunEvent {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        RunEvent {
            schema: "scorepeek-run-event-v2".to_owned(),
            kind: RunEventKind::FieldObservation {
                session_id: Some("invocation-1-session-1".to_owned()),
                capture_generation: Some(1),
                sequence,
                monotonic_start_ms: sequence.saturating_mul(100),
                monotonic_end_ms: sequence.saturating_mul(100).saturating_add(25),
                screen: "result".to_owned(),
                fields: json!({
                    "title": "OCR TITLE",
                    "artist": "OCR ARTIST",
                    "clear_type": "CLEAR",
                    "clear_type_ocr": "CLEAR"
                }),
                result_song_resolution: json!({ "status": "accepted" }),
                music_select_song_resolution: Value::Null,
                parsed_result_fields: None,
                result_chart_resolution: Some(ResultChartResolution::Accepted {
                    resolver_id: "scorepeek-result-fields-catalog-constrained-v4".to_owned(),
                    chart: scorepeek::catalog::Chart {
                        key: scorepeek::catalog::ChartKey {
                            play_type: PlayType::Single,
                            difficulty: Difficulty::Hyper,
                        },
                        level: 8,
                        notes: 764,
                    },
                    current_score: 1_286,
                }),
                result_performance_resolution: Some(ResultPerformanceResolution::Accepted {
                    resolver_id: "scorepeek-result-performance-v1".to_owned(),
                    judgments: ResultJudgments {
                        pgreat: 600,
                        great: 86,
                        good: 10,
                        bad: 5,
                        poor: 3,
                    },
                    miss_count: SupplementalResultValue::Known { value: 3 },
                    timing: ResultTiming {
                        fast: SupplementalResultValue::Known { value: 20 },
                        slow: SupplementalResultValue::Known { value: 21 },
                    },
                    combo_break: SupplementalResultValue::Known { value: 2 },
                    previous_best: PreviousBest {
                        clear_type: scorepeek::recognition::PreviousBestValue::Known {
                            value: "CLEAR".to_owned(),
                        },
                        score: scorepeek::recognition::PreviousBestValue::Known { value: 1_200 },
                        miss_count: scorepeek::recognition::PreviousBestValue::Known { value: 4 },
                    },
                }),
                current_score_ocr_resolution: None,
                numeric_batch: None,
                song_resolution_presentation: Box::new(SongResolutionPresentation::Accepted {
                    reason: None,
                    selected: SongPresentation {
                        scorepeek_song_id: song_id,
                        display_titles: vec!["CATALOG TITLE".to_owned()],
                        artist: "CATALOG ARTIST".to_owned(),
                    },
                    runner_up: SongPresentation {
                        scorepeek_song_id: serde_json::from_str(
                            "\"00000000-0000-0000-0000-000000000002\"",
                        )
                        .unwrap(),
                        display_titles: vec!["RUNNER UP".to_owned()],
                        artist: "RUNNER ARTIST".to_owned(),
                    },
                    evidence_summary: "title edit=0; runner-up margin=4".to_owned(),
                }),
            },
        }
    }

    fn detected_result_event(
        session_id: &str,
        capture_generation: u64,
        source_sequence: u64,
        result: ResultDomainEvent,
    ) -> RunEvent {
        RunEvent {
            schema: "scorepeek-run-event-v2".to_owned(),
            kind: RunEventKind::ResultDetected {
                session_id: session_id.to_owned(),
                capture_generation,
                source_sequence,
                result,
            },
        }
    }

    fn prepare_accepted_attempt(output: &mut RoutineOutput) {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        let song = SongPresentation {
            scorepeek_song_id: song_id,
            display_titles: vec!["CATALOG TITLE".to_owned()],
            artist: "CATALOG ARTIST".to_owned(),
        };
        output
            .play_attempt
            .observe_selection(Some(song), Some(SelectionSource::Stable), 0);
        output
            .play_attempt
            .observe_screen(PlayAttemptScreen::Play, 0);
        output
            .play_attempt
            .observe_screen(PlayAttemptScreen::Result, 0);
    }

    fn unknown_result_event(sequence: u64) -> RunEvent {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation {
            fields,
            result_song_resolution,
            song_resolution_presentation,
            ..
        } = &mut event.kind
        else {
            unreachable!();
        };
        fields["clear_type"] = Value::Null;
        *result_song_resolution = json!({ "status": "unknown" });
        **song_resolution_presentation = SongResolutionPresentation::Unknown {
            reason: json!("artist_similarity_too_low"),
            selected: None,
            runner_up: None,
            evidence_summary: None,
        };
        event
    }

    fn accepted_result_without_chart(sequence: u64) -> RunEvent {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation {
            result_chart_resolution,
            ..
        } = &mut event.kind
        else {
            unreachable!();
        };
        *result_chart_resolution = None;
        event
    }

    fn result_with_rejected_performance(sequence: u64) -> RunEvent {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation {
            result_performance_resolution,
            ..
        } = &mut event.kind
        else {
            unreachable!();
        };
        *result_performance_resolution = Some(ResultPerformanceResolution::Unknown {
            resolver_id: "scorepeek-result-performance-v1".to_owned(),
            reason: scorepeek::recognition::ResultPerformanceUnknownReason::ScoreBreakdownMismatch,
        });
        event
    }

    fn accepted_music_select_event(sequence: u64) -> RunEvent {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation {
            screen,
            fields,
            result_song_resolution,
            music_select_song_resolution,
            ..
        } = &mut event.kind
        else {
            unreachable!();
        };
        *screen = "music_select".to_owned();
        *fields = json!({
            "active_list_title": "OCR ACTIVE",
            "central_title": "OCR CENTRAL",
            "artist": "OCR ARTIST"
        });
        *result_song_resolution = Value::Null;
        *music_select_song_resolution = json!({ "status": "accepted" });
        event
    }

    fn unknown_music_select_event(sequence: u64) -> RunEvent {
        let mut event = accepted_music_select_event(sequence);
        let RunEventKind::FieldObservation {
            music_select_song_resolution,
            song_resolution_presentation,
            ..
        } = &mut event.kind
        else {
            unreachable!();
        };
        *music_select_song_resolution = json!({ "status": "unknown" });
        **song_resolution_presentation = SongResolutionPresentation::Unknown {
            reason: json!("active_prefix_too_weak"),
            selected: None,
            runner_up: None,
            evidence_summary: None,
        };
        event
    }

    fn screen_event(sequence: u64, screen: &str) -> RunEvent {
        RunEvent {
            schema: "scorepeek-run-event-v2".to_owned(),
            kind: RunEventKind::ScreenChanged {
                session_id: Some("invocation-1-session-1".to_owned()),
                capture_generation: Some(1),
                sequence,
                monotonic_start_ms: sequence.saturating_mul(100),
                monotonic_end_ms: sequence.saturating_mul(100).saturating_add(25),
                screen: screen.to_owned(),
            },
        }
    }

    #[test]
    fn mode_select_is_displayed_without_stale_field_data_or_attempt_evidence() {
        assert_eq!(play_attempt_screen("mode_select"), None);
        let stale = json!({
            "screen": "music_select",
            "sequence": 7,
            "monotonic_start_ms": 700,
            "monotonic_end_ms": 725,
            "fields": { "active_list_title": "STALE TITLE" }
        });
        let lines = observation_lines(Some("mode_select"), Some(&stale), None, None, false, 80);
        let rendered = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(rendered.contains("screen=mode_select"));
        assert!(rendered.contains("No field observation for this screen"));
        assert!(!rendered.contains("STALE TITLE"));
    }

    fn read_events(reader: &mut BufReader<UnixStream>, count: usize) -> Vec<Value> {
        (0..count)
            .map(|_| {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                serde_json::from_str::<Value>(&line).unwrap()
            })
            .collect()
    }

    fn publish_transitional_music_select_false_positive(
        output: &mut RoutineOutput,
        reader: &mut BufReader<UnixStream>,
    ) {
        output.publish(&screen_event(4, "unknown")).unwrap();
        let transition_start = read_events(reader, 2);
        assert_eq!(transition_start[0]["event"], "screen_changed");
        assert_eq!(
            transition_start[1]["event"],
            "temporal_music_select_changed"
        );

        output.publish(&screen_event(5, "music_select")).unwrap();
        let false_positive = read_events(reader, 1);
        assert_eq!(false_positive[0]["event"], "screen_changed");
        output.publish(&unknown_music_select_event(5)).unwrap();
        let empty_ocr = read_events(reader, 1);
        assert_eq!(empty_ocr[0]["event"], "field_observation");

        output.publish(&screen_event(6, "unknown")).unwrap();
        let transition_continues = read_events(reader, 1);
        assert_eq!(transition_continues[0]["event"], "screen_changed");
    }

    fn publish_transitional_result_false_positive(
        output: &mut RoutineOutput,
        reader: &mut BufReader<UnixStream>,
    ) {
        output.publish(&screen_event(9, "unknown")).unwrap();
        assert_eq!(read_events(reader, 1)[0]["event"], "screen_changed");
        output.publish(&screen_event(10, "music_select")).unwrap();
        assert_eq!(read_events(reader, 1)[0]["event"], "screen_changed");
        output.publish(&unknown_music_select_event(10)).unwrap();
        assert_eq!(read_events(reader, 1)[0]["event"], "field_observation");
        output.publish(&screen_event(11, "unknown")).unwrap();
        assert_eq!(read_events(reader, 1)[0]["event"], "screen_changed");
    }

    fn read_snapshot(socket_path: &Path) -> Value {
        let stream = UnixStream::connect(socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        serde_json::from_str(&snapshot).unwrap()
    }

    fn assert_stabilized_fields(state: &RunViewState, expected: &str) {
        let stabilized = state.latest_stabilized_result.as_ref().unwrap();
        assert_eq!(stabilized.pointer("/state/song/status").unwrap(), expected);
        assert_eq!(
            stabilized.pointer("/state/clear_type/status").unwrap(),
            expected
        );
    }

    #[derive(Default)]
    struct FailFirstWrite {
        failed: bool,
        recovered_bytes: Vec<u8>,
    }

    impl Write for FailFirstWrite {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.failed {
                self.failed = true;
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "injected"));
            }
            self.recovered_bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_terminal_entry_attempts_to_restore_screen_and_cursor() {
        let mut output = FailFirstWrite::default();
        assert!(enter_alternate_screen(&mut output).is_err());
        assert_eq!(output.recovered_bytes, b"\x1b[?25h\x1b[?1049l");
    }

    #[test]
    fn socket_sends_snapshot_before_live_events_and_removes_its_own_path() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let socket_path = channel.socket_path.clone();
        let stream = UnixStream::connect(&socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let snapshot: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(snapshot["schema"], "scorepeek-run-observation-snapshot-v1");
        assert_eq!(snapshot["state"]["invocation_id"], "invocation-1");
        assert_eq!(snapshot["state"]["next_channel_sequence"], 1);
        assert_eq!(snapshot["channel"]["connected_clients"], 1);

        channel
            .publish(&json!({
                "schema": "scorepeek-run-event-v2",
                "event": "watcher_started",
                "channel_sequence": 1,
            }))
            .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let event: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(event["event"], "watcher_started");
        drop(channel);
        assert!(!socket_path.exists());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn raw_result_events_are_followed_by_bounded_temporal_transitions() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let stream = UnixStream::connect(&channel.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        let mut output = test_output(state, channel);
        prepare_accepted_attempt(&mut output);

        output.publish(&accepted_result_event(1)).unwrap();
        output.publish(&accepted_result_event(2)).unwrap();
        let events = read_events(&mut reader, 8);
        assert_eq!(events[0]["event"], "field_observation");
        assert_eq!(events[1]["event"], "temporal_result_changed");
        assert_eq!(events[1]["state"]["song"]["status"], "pending");
        assert_eq!(events[2]["event"], "numeric_result_changed");
        assert_eq!(events[3]["event"], "field_observation");
        assert_eq!(events[4]["event"], "temporal_result_changed");
        assert_eq!(events[4]["state"]["song"]["status"], "stable");
        assert_eq!(events[4]["state"]["clear_type"]["status"], "stable");
        assert_eq!(events[5]["event"], "play_attempt_changed");
        assert_eq!(
            events[5]["state"]["attempt"]["result_relation"],
            "confirmed"
        );
        assert_eq!(events[6]["event"], "numeric_result_changed");
        assert_eq!(events[6]["state"]["status"], "accepted");
        assert!(events[6].get("event_suppression_reason").is_none());
        assert_eq!(events[7]["event"], "result_detected");
        assert_eq!(
            events[7]["result"]["contract"],
            "scorepeek-result-detected-v2"
        );
        assert_eq!(events[7]["result"]["attempt_id"], 1);
        assert!(events[7]["result"].get("savable").is_none());
        assert_eq!(events[7]["result"]["play_side"], "one_player");
        assert_eq!(events[7]["result"]["play_mode"], "single_play");
        assert_eq!(events[7]["result"]["difficulty"], "hyper");
        assert_eq!(events[7]["result"]["current_score"], 1_286);
        assert_eq!(events[7]["result"]["judgments"]["pgreat"], 600);
        assert_eq!(
            events[4]["stable_song"]["display_titles"][0],
            "CATALOG TITLE"
        );
        assert_eq!(
            events
                .iter()
                .map(|event| event["channel_sequence"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7, 8]
        );

        output.publish(&unknown_result_event(3)).unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let raw_unknown: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(raw_unknown["event"], "field_observation");
        assert_eq!(raw_unknown["channel_sequence"], 9);
        let state = output.state.lock().unwrap();
        assert_stabilized_fields(&state, "stable");
        drop(state);

        output
            .publish(&RunEvent {
                schema: "scorepeek-run-event-v2".to_owned(),
                kind: RunEventKind::ScreenChanged {
                    session_id: Some("invocation-1-session-1".to_owned()),
                    capture_generation: Some(1),
                    sequence: 4,
                    monotonic_start_ms: 400,
                    monotonic_end_ms: 425,
                    screen: "unknown".to_owned(),
                },
            })
            .unwrap();
        let boundary_events = read_events(&mut reader, 2);
        assert_eq!(boundary_events[0]["event"], "screen_changed");
        assert_eq!(boundary_events[1]["event"], "temporal_result_changed");
        assert_eq!(boundary_events[1]["state"]["song"]["status"], "empty");
        assert_eq!(boundary_events[1]["channel_sequence"], 11);

        output.publish(&accepted_result_event(5)).unwrap();
        let after_boundary = read_events(&mut reader, 3);
        assert_eq!(after_boundary[0]["event"], "field_observation");
        assert_eq!(after_boundary[1]["state"]["song"]["status"], "pending");
        assert_eq!(after_boundary[2]["event"], "numeric_result_changed");

        output
            .publish(&RunEvent {
                schema: "scorepeek-run-event-v2".to_owned(),
                kind: RunEventKind::SessionFinished {
                    session_id: "invocation-1-session-1".to_owned(),
                    capture_generation: 1,
                    outcome: "stopped".to_owned(),
                    report: json!({}),
                },
            })
            .unwrap();
        let session_events = read_events(&mut reader, 2);
        assert_eq!(session_events[0]["event"], "session_finished");
        assert_eq!(session_events[1]["event"], "temporal_result_changed");
        assert_eq!(session_events[1]["channel_sequence"], 16);
    }

    #[test]
    fn production_result_policy_stabilizes_across_long_field_ocr_spacing() {
        let mut reducer = ResultTemporalReducer::new(
            TemporalPolicy::for_episode(RESULT_TEMPORAL_REQUIRED_OBSERVATIONS).unwrap(),
        );
        reducer.observe_result(1, 100, Some(7), Some("CLEAR".to_owned()));
        let update = reducer
            .observe_result(8, 800, Some(7), Some("CLEAR".to_owned()))
            .unwrap();
        assert_eq!(update.state.song.stable_value(), Some(&7));
        assert_eq!(
            update.state.clear_type.stable_value().map(String::as_str),
            Some("CLEAR")
        );
    }

    #[test]
    fn field_observation_older_than_an_observed_screen_boundary_cannot_reenter_temporal_state() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let mut output = test_output(state, channel);

        output.publish(&screen_event(1, "result")).unwrap();
        output.publish(&accepted_result_event(1)).unwrap();
        assert!(matches!(
            output.temporal_result.state().song,
            scorepeek::temporal_recognition::TemporalFieldState::Pending { .. }
        ));
        output.publish(&screen_event(2, "unknown")).unwrap();
        assert_eq!(
            output.temporal_result.state(),
            &ResultTemporalState::default()
        );

        output.publish(&accepted_result_event(1)).unwrap();
        assert_eq!(
            output.temporal_result.state(),
            &ResultTemporalState::default()
        );

        output.publish(&screen_event(3, "result")).unwrap();
        output.publish(&accepted_result_event(3)).unwrap();
        assert!(matches!(
            output.temporal_result.state().song,
            scorepeek::temporal_recognition::TemporalFieldState::Pending { .. }
        ));
    }

    #[test]
    fn complete_context_after_temporal_stability_emits_once() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let stream = UnixStream::connect(&channel.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        let mut output = test_output(state, channel);
        prepare_accepted_attempt(&mut output);

        output.publish(&accepted_result_without_chart(1)).unwrap();
        output.publish(&accepted_result_without_chart(2)).unwrap();
        let stabilization = read_events(&mut reader, 5);
        assert!(
            stabilization
                .iter()
                .all(|event| event["event"] != "result_detected")
        );
        assert_eq!(stabilization[3]["state"]["song"]["status"], "stable");
        assert_eq!(stabilization[4]["event"], "play_attempt_changed");

        output
            .publish(&result_with_rejected_performance(3))
            .unwrap();
        let rejected = read_events(&mut reader, 1);
        assert_eq!(rejected[0]["event"], "field_observation");

        output.publish(&accepted_result_event(4)).unwrap();
        let candidate = read_events(&mut reader, 2);
        assert_eq!(candidate[0]["event"], "field_observation");
        assert_eq!(candidate[1]["event"], "numeric_result_changed");

        output.publish(&accepted_result_event(5)).unwrap();
        let completed = read_events(&mut reader, 3);
        assert_eq!(completed[0]["event"], "field_observation");
        assert_eq!(completed[1]["event"], "numeric_result_changed");
        assert_eq!(completed[2]["event"], "result_detected");
        assert_eq!(completed[2]["source_sequence"], 5);

        output.publish(&accepted_result_event(6)).unwrap();
        assert_eq!(read_events(&mut reader, 1)[0]["event"], "field_observation");
    }

    #[test]
    fn music_select_temporal_events_follow_raw_observations_and_hold_unknown() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let stream = UnixStream::connect(&channel.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        let mut output = test_output(state, channel);

        for sequence in 1..=3 {
            output
                .publish(&accepted_music_select_event(sequence))
                .unwrap();
        }
        let events = read_events(&mut reader, 7);
        for pair in events[..6].chunks_exact(2) {
            assert_eq!(pair[0]["event"], "field_observation");
            assert_eq!(pair[1]["event"], "temporal_music_select_changed");
        }
        assert_eq!(events[5]["state"]["status"], "stable");
        assert_eq!(
            events[5]["retained_song"]["display_titles"][0],
            "CATALOG TITLE"
        );
        assert_eq!(events[6]["event"], "play_attempt_changed");
        assert_eq!(events[6]["state"]["status"], "armed");
        assert_eq!(events[6]["state"]["selection_source"], "stable");

        output.publish(&unknown_music_select_event(4)).unwrap();
        let held = read_events(&mut reader, 3);
        assert_eq!(held[0]["event"], "field_observation");
        assert_eq!(held[1]["state"]["status"], "held_unknown");
        assert_eq!(held[1]["retained_song"]["artist"], "CATALOG ARTIST");
        assert!(held[1].get("candidate_song").is_none());
        assert_eq!(held[2]["state"]["selection_source"], "last_confirmed_held");

        output.publish(&accepted_music_select_event(5)).unwrap();
        let recovered = read_events(&mut reader, 3);
        assert_eq!(recovered[1]["state"]["status"], "stable");
        assert_eq!(recovered[1]["reasons"][0], "change_cancelled");
        assert_eq!(recovered[2]["state"]["selection_source"], "stable");

        output
            .publish(&RunEvent {
                schema: "scorepeek-run-event-v2".to_owned(),
                kind: RunEventKind::WatcherStopped {
                    invocation_id: "invocation-1".to_owned(),
                    reason: "signal".to_owned(),
                },
            })
            .unwrap();
        let stopped = read_events(&mut reader, 3);
        assert_eq!(stopped[0]["event"], "play_attempt_changed");
        assert_eq!(stopped[0]["state"]["status"], "idle");
        assert_eq!(stopped[1]["event"], "watcher_stopped");
        assert_eq!(stopped[2]["event"], "temporal_music_select_changed");
        assert_eq!(stopped[2]["state"]["status"], "empty");
        let state = output.state.lock().unwrap();
        assert!(state.latest_observation.is_none());
        assert_eq!(
            state.latest_temporal_music_select.as_ref().unwrap()["state"]["status"],
            "empty"
        );
    }

    #[test]
    fn play_attempt_events_follow_raw_causes_and_snapshot_retains_confirmation() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let stream = UnixStream::connect(&channel.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        let mut output = test_output(state, channel);

        for sequence in 1..=3 {
            output
                .publish(&accepted_music_select_event(sequence))
                .unwrap();
        }
        let armed = read_events(&mut reader, 7);
        assert_eq!(armed[6]["event"], "play_attempt_changed");
        assert_eq!(armed[6]["state"]["status"], "armed");

        publish_transitional_music_select_false_positive(&mut output, &mut reader);

        output
            .publish(&screen_event(7, "decide_transition"))
            .unwrap();
        let decide = read_events(&mut reader, 2);
        assert_eq!(decide[0]["event"], "screen_changed");
        assert_eq!(decide[1]["event"], "play_attempt_changed");
        assert_eq!(decide[1]["state"]["attempt"]["phase"], "decided");
        assert_eq!(decide[1]["state"]["attempt"]["selection_source"], "stable");
        assert_eq!(
            decide[1]["state"]["attempt"]["path"]["select_observed"],
            true
        );

        output.publish(&screen_event(8, "play")).unwrap();
        let play = read_events(&mut reader, 2);
        assert_eq!(play[0]["event"], "screen_changed");
        assert_eq!(play[1]["state"]["attempt"]["phase"], "playing");

        publish_transitional_result_false_positive(&mut output, &mut reader);

        output.publish(&screen_event(12, "result")).unwrap();
        let result_screen = read_events(&mut reader, 2);
        assert_eq!(result_screen[0]["event"], "screen_changed");
        assert_eq!(
            result_screen[1]["state"]["attempt"]["result_relation"],
            "pending"
        );

        output.publish(&accepted_result_event(13)).unwrap();
        output.publish(&accepted_result_event(14)).unwrap();
        let result = read_events(&mut reader, 7);
        assert_eq!(result[0]["event"], "field_observation");
        assert_eq!(result[1]["event"], "temporal_result_changed");
        assert_eq!(result[2]["event"], "numeric_result_changed");
        assert_eq!(result[3]["event"], "field_observation");
        assert_eq!(result[4]["event"], "temporal_result_changed");
        assert_eq!(result[5]["event"], "play_attempt_changed");
        assert_eq!(result[6]["event"], "numeric_result_changed");
        assert_eq!(
            result[5]["state"]["attempt"]["result_relation"],
            "confirmed"
        );

        let snapshot = read_snapshot(&output.channel.socket_path);
        assert_eq!(
            snapshot["state"]["latest_play_attempt"]["state"]["attempt"]["result_relation"],
            "confirmed"
        );

        output.publish(&screen_event(15, "music_select")).unwrap();
        let snapshot = read_snapshot(&output.channel.socket_path);
        assert_eq!(
            snapshot["state"]["latest_play_attempt"]["state"]["attempt"]["result_relation"],
            "confirmed"
        );

        for sequence in 16..=18 {
            output
                .publish(&accepted_music_select_event(sequence))
                .unwrap();
        }
        let snapshot = read_snapshot(&output.channel.socket_path);
        assert_eq!(
            snapshot["state"]["latest_play_attempt"]["state"]["status"],
            "armed"
        );
        assert_eq!(
            snapshot["state"]["latest_play_attempt"]["state"]["source_sequence"],
            18
        );
    }

    #[test]
    fn socket_broadcasts_one_live_event_to_multiple_clients() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let mut readers = (0..2)
            .map(|_| {
                let stream = UnixStream::connect(&channel.socket_path).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                BufReader::new(stream)
            })
            .collect::<Vec<_>>();
        for reader in &mut readers {
            let mut snapshot = String::new();
            reader.read_line(&mut snapshot).unwrap();
            assert!(snapshot.contains("scorepeek-run-observation-snapshot-v1"));
        }
        channel
            .publish(&json!({
                "schema": "scorepeek-run-event-v2",
                "event": "watcher_started",
                "channel_sequence": 1,
            }))
            .unwrap();
        for reader in &mut readers {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["channel_sequence"],
                1
            );
        }
    }

    #[test]
    fn publishing_without_clients_is_healthy() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        channel
            .publish(&json!({
                "schema": "scorepeek-run-event-v2",
                "event": "watcher_started",
                "channel_sequence": 1,
            }))
            .unwrap();
        std::thread::sleep(Duration::from_millis(40));
        assert!(!channel.health.server_failed.load(Ordering::Acquire));
        assert_eq!(channel.health.connected_clients.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_slow_client_is_disconnected_without_degrading_the_server() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let stream = UnixStream::connect(&channel.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        channel
            .publish(&json!({
                "schema": "scorepeek-run-event-v2",
                "event": "field_observation",
                "channel_sequence": 1,
                "payload": "x".repeat(2 * 1024 * 1024),
            }))
            .unwrap();
        for _ in 0..50 {
            if channel.health.disconnected_clients.load(Ordering::Acquire) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(channel.health.connected_clients.load(Ordering::Acquire), 0);
        assert_eq!(
            channel.health.disconnected_clients.load(Ordering::Acquire),
            1
        );
        assert!(!channel.health.server_failed.load(Ordering::Acquire));
    }

    #[test]
    fn stale_socket_is_replaced_but_other_entries_are_preserved() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("scorepeek");
        fs::create_dir(&directory).unwrap();
        let socket_path = directory.join(SOCKET_NAME);
        let stale = UnixListener::bind(&socket_path).unwrap();
        drop(stale);
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        drop(channel);

        fs::write(&socket_path, b"owned by operator").unwrap();
        let Err(error) = ObservationChannel::start_at(temporary.path(), state()) else {
            panic!("non-socket entry must not be replaced");
        };
        assert!(error.contains("non-socket"));
        assert_eq!(fs::read(&socket_path).unwrap(), b"owned by operator");

        fs::remove_file(&socket_path).unwrap();
        let target = directory.join("target");
        fs::write(&target, b"target").unwrap();
        symlink(&target, &socket_path).unwrap();
        let Err(error) = ObservationChannel::start_at(temporary.path(), state()) else {
            panic!("symlink must not be replaced");
        };
        assert!(error.contains("non-socket"));
        assert!(
            socket_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn active_socket_is_not_unlinked_or_rebound() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("scorepeek");
        fs::create_dir(&directory).unwrap();
        let socket_path = directory.join(SOCKET_NAME);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let Err(error) = ObservationChannel::start_at(temporary.path(), state()) else {
            panic!("active socket must not be replaced");
        };
        assert!(error.contains("already active"));
        assert!(
            socket_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_socket()
        );
        drop(listener);
    }

    #[test]
    fn initialization_guard_removes_only_the_socket_it_owns() {
        let temporary = tempfile::tempdir().unwrap();
        let socket_path = temporary.path().join("initializing.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let metadata = socket_path.symlink_metadata().unwrap();
        let identity = (metadata.dev(), metadata.ino());
        drop(SocketPathGuard::new(socket_path.clone(), identity));
        assert!(!socket_path.exists());
        drop(listener);
    }

    #[test]
    fn cleanup_preserves_an_entry_that_replaced_the_owned_socket() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let socket_path = channel.socket_path.clone();
        fs::remove_file(&socket_path).unwrap();
        fs::write(&socket_path, b"replacement").unwrap();
        drop(channel);
        assert_eq!(fs::read(&socket_path).unwrap(), b"replacement");
    }

    #[test]
    fn cleanup_preserves_a_socket_with_a_different_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let socket_path = channel.socket_path.clone();
        fs::remove_file(&socket_path).unwrap();
        let replacement = UnixListener::bind(&socket_path).unwrap();
        drop(channel);
        assert!(
            socket_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_socket()
        );
        drop(replacement);
    }

    #[test]
    fn full_event_queue_is_counted_without_blocking_the_producer() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
        let health = ChannelHealth::default();
        try_send_event(&sender, &health, vec![1]);
        try_send_event(&sender, &health, vec![2]);
        assert_eq!(health.dropped_events.load(Ordering::Acquire), 1);
        assert!(!health.server_failed.load(Ordering::Acquire));
    }

    #[test]
    fn plain_status_does_not_change_for_a_field_observation() {
        let mut state = RunViewState::new("invocation-1".to_owned(), "e".repeat(64), true);
        let health = ChannelHealth::default();
        let before = plain_status_line(&state, &health);
        state.latest_observation = Some(json!({
            "event": "field_observation",
            "fields": { "title": "OCR VALUE" }
        }));
        assert_eq!(plain_status_line(&state, &health), before);
        assert!(!before.contains("OCR VALUE"));
    }

    #[test]
    fn typed_reducer_tracks_session_report_and_stop_transitions() {
        let mut state = RunViewState::new("invocation-1".to_owned(), "d".repeat(64), true);
        let started = RunEvent::from_value(json!({
            "schema": "scorepeek-run-event-v2",
            "event": "session_started",
            "session_id": "invocation-1-session-1",
            "capture_generation": 1,
            "capture_profile_sha256": "profile",
            "normalizer_artifact_sha256": "normalizer"
        }))
        .unwrap();
        state.reduce(&started, &started.to_value().unwrap());
        assert_eq!(state.watcher_state, "session_active");
        assert_eq!(state.session_count, 1);
        assert_eq!(
            state.active_session_id.as_deref(),
            Some("invocation-1-session-1")
        );

        let finished = RunEvent::from_value(json!({
            "schema": "scorepeek-run-event-v2",
            "event": "session_finished",
            "session_id": "invocation-1-session-1",
            "capture_generation": 1,
            "outcome": "source_ended",
            "report": { "recognition_ticks": 3 }
        }))
        .unwrap();
        state.reduce(&finished, &finished.to_value().unwrap());
        assert_eq!(state.watcher_state, "session_finished");
        assert_eq!(
            state.latest_report.as_ref().unwrap()["recognition_ticks"],
            3
        );

        state.latest_observation = Some(json!({ "sequence": 9 }));
        state.latest_stabilized_result = Some(json!({ "state": { "song": "stable" } }));
        state.latest_temporal_music_select = Some(json!({ "state": { "status": "changing" } }));
        let next_started = RunEvent::from_value(json!({
            "schema": "scorepeek-run-event-v2",
            "event": "session_started",
            "session_id": "invocation-1-session-2",
            "capture_generation": 2,
            "capture_profile_sha256": "profile",
            "normalizer_artifact_sha256": "normalizer"
        }))
        .unwrap();
        state.reduce(&next_started, &next_started.to_value().unwrap());
        assert_eq!(
            state.active_session_id.as_deref(),
            Some("invocation-1-session-2")
        );
        assert!(state.latest_observation.is_none());
        assert!(state.latest_report.is_none());

        let stopped = RunEvent::from_value(json!({
            "schema": "scorepeek-run-event-v2",
            "event": "watcher_stopped",
            "invocation_id": "invocation-1",
            "reason": "signal"
        }))
        .unwrap();
        state.reduce(&stopped, &stopped.to_value().unwrap());
        assert_eq!(state.watcher_state, "stopped");
        assert!(state.latest_observation.is_none());
        assert!(state.latest_stabilized_result.is_none());
        assert!(state.latest_temporal_music_select.is_none());
    }

    #[test]
    fn tui_keeps_catalog_presentation_separate_from_ocr_values() {
        let mut state = RunViewState::new("invocation-1".to_owned(), "b".repeat(64), true);
        state.watcher_state = "session_active".to_owned();
        state.current_screen = Some("unknown".to_owned());
        state.latest_observation = Some(json!({
            "event": "field_observation",
            "screen": "result",
            "sequence": 42,
            "monotonic_start_ms": 100,
            "monotonic_end_ms": 125,
            "fields": {
                "title": "OCR TITLE",
                "artist": "OCR ARTIST",
                "clear_type_ocr": "CLEAR"
            },
            "song_resolution_presentation": {
                "status": "accepted",
                "reason": null,
                "selected": {
                    "scorepeek_song_id": "00000000-0000-0000-0000-000000000001",
                    "display_titles": ["CATALOG TITLE", "CATALOG ALT"],
                    "artist": "CATALOG ARTIST"
                },
                "runner_up": {
                    "display_titles": ["RUNNER UP"]
                },
                "evidence_summary": "title edit=0; runner-up margin=4"
            }
        }));
        state.latest_stabilized_result = Some(json!({
            "event": "temporal_result_changed",
            "state": {
                "song": {
                    "status": "stable",
                    "value": "00000000-0000-0000-0000-000000000001",
                    "evidence": {
                        "count": 2,
                        "required": 2,
                        "first_sequence": 41,
                        "last_sequence": 42,
                        "first_monotonic_ms": 25,
                        "last_monotonic_ms": 125
                    }
                },
                "clear_type": {
                    "status": "stable",
                    "value": "CLEAR",
                    "evidence": {
                        "count": 2,
                        "required": 2,
                        "first_sequence": 41,
                        "last_sequence": 42,
                        "first_monotonic_ms": 25,
                        "last_monotonic_ms": 125
                    }
                }
            },
            "stable_song": {
                "scorepeek_song_id": "00000000-0000-0000-0000-000000000001",
                "display_titles": ["STABLE CATALOG TITLE"],
                "artist": "STABLE CATALOG ARTIST"
            }
        }));
        let health = ChannelHealth::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &state, Path::new("/run/scorepeek.sock"), &health))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("OCR title: OCR TITLE"));
        assert!(rendered.contains("Accepted play events"));
        assert!(rendered.contains("No accepted play event yet"));
        assert!(rendered.contains("Debug: Watcher"));
        assert!(rendered.contains("Debug: Latest recognition"));
        assert!(rendered.contains("Debug: Observation channel"));
        assert!(rendered.contains("current screen=unknown"));
        assert!(rendered.contains("raw screen=result"));
        assert!(rendered.contains("invocation=invocation-1"));
        assert!(rendered.contains("Catalog title: CATALOG TITLE / CATALOG ALT"));
        assert!(rendered.contains("Catalog artist: CATALOG ARTIST"));
        assert!(rendered.contains("ACCEPTED"));
        assert!(rendered.contains("Stabilized result: STABLE"));
        assert!(rendered.contains("Stable title: STABLE CATALOG TITLE"));
        assert!(rendered.contains("Stable artist: STABLE CATALOG ARTIST"));
        assert!(rendered.contains("Stable clear: CLEAR"));
    }

    #[test]
    fn compact_tui_prioritizes_title_and_artist_over_song_id() {
        let mut state = RunViewState::new(
            "run-1787877123-123456789-424242".to_owned(),
            "c".repeat(64),
            false,
        );
        state.watcher_state = "session_active".to_owned();
        state.active_session_id = Some("run-1787877123-123456789-424242-session-12".to_owned());
        state.capture_generation = Some(12);
        state.latest_observation = Some(json!({
            "screen": "music_select",
            "sequence": 1,
            "monotonic_start_ms": 1,
            "monotonic_end_ms": 2,
            "fields": {
                "active_list_title": "OCR ACTIVE",
                "central_title": "OCR CENTRAL",
                "artist": "OCR ARTIST"
            },
            "song_resolution_presentation": {
                "status": "unknown",
                "reason": "ambiguous",
                "selected": {
                    "scorepeek_song_id": "00000000-0000-0000-0000-000000000001",
                    "display_titles": ["IMPORTANT TITLE"],
                    "artist": "IMPORTANT ARTIST"
                }
            }
        }));
        let health = ChannelHealth::default();
        let backend = TestBackend::new(70, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &state, Path::new("/run/scorepeek.sock"), &health))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("IMPORTANT TITLE"));
        assert!(rendered.contains("IMPORTANT ARTIST"));
        assert!(rendered.contains("Candidate title:"));
        assert!(rendered.contains("state=session_active"));
        assert!(rendered.contains("generation=12"));
        assert!(rendered.contains("invocation=run-1787877123"));
        assert!(!rendered.contains("00000000-0000-0000-0000-000000000001"));
    }

    #[test]
    fn music_select_tui_labels_retained_catalog_identity_as_held() {
        let mut state = RunViewState::new("invocation-1".to_owned(), "e".repeat(64), true);
        state.current_screen = Some("music_select".to_owned());
        state.latest_observation = Some(json!({
            "screen": "music_select",
            "sequence": 9,
            "monotonic_start_ms": 900,
            "monotonic_end_ms": 925,
            "fields": {
                "active_list_title": "OCR ACTIVE JITTER",
                "central_title": "OCR CENTRAL JITTER",
                "artist": "OCR ARTIST JITTER"
            },
            "song_resolution_presentation": {
                "status": "unknown",
                "reason": "active_prefix_too_weak",
                "selected": null,
                "runner_up": null
            }
        }));
        state.latest_temporal_music_select = Some(json!({
            "event": "temporal_music_select_changed",
            "state": {
                "status": "held_unknown",
                "value": "00000000-0000-0000-0000-000000000001"
            },
            "retained_song": {
                "scorepeek_song_id": "00000000-0000-0000-0000-000000000001",
                "display_titles": ["STABLE CATALOG TITLE"],
                "artist": "STABLE CATALOG ARTIST"
            }
        }));
        let health = ChannelHealth::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &state, Path::new("/run/scorepeek.sock"), &health))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("Temporal selection: HELD"));
        assert!(rendered.contains("Last confirmed title: STABLE CATALOG TITLE"));
        assert!(rendered.contains("Last confirmed artist: STABLE CATALOG ARTIST"));
        assert!(rendered.contains("OCR central title: OCR CENTRAL JITTER"));
    }

    #[test]
    fn play_attempt_tui_prioritizes_song_and_exposes_conflict_path() {
        let mut state = RunViewState::new("invocation-1".to_owned(), "f".repeat(64), true);
        state.latest_play_attempt = Some(json!({
            "event": "play_attempt_changed",
            "state": {
                "status": "attempt",
                "attempt": {
                    "attempt_id": 2,
                    "parent_attempt_id": 1,
                    "phase": "result",
                    "path": {
                        "select_observed": true,
                        "decide_observed": true,
                        "play_observed": true,
                        "result_observed": true
                    },
                    "selection_source": "stable",
                    "selected_song": {
                        "scorepeek_song_id": "00000000-0000-0000-0000-000000000001",
                        "display_titles": ["SELECTED TITLE"],
                        "artist": "SELECTED ARTIST"
                    },
                    "result_song": {
                        "scorepeek_song_id": "00000000-0000-0000-0000-000000000002",
                        "display_titles": ["RESULT TITLE"],
                        "artist": "RESULT ARTIST"
                    },
                    "result_relation": "conflict",
                    "reasons": []
                }
            }
        }));
        let health = ChannelHealth::default();
        for (width, height, compact) in [(100, 36, false), (60, 24, true)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render(frame, &state, Path::new("/run/scorepeek.sock"), &health))
                .unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            assert!(rendered.contains("Play attempt"));
            assert!(rendered.contains("CONFLICT"));
            assert!(rendered.contains("Song: SELECTED TITLE"));
            assert!(rendered.contains("Artist: SELECTED ARTIST"));
            assert!(rendered.contains("Result: RESULT TITLE"));
            if compact {
                assert!(!rendered.contains("00000000-0000-0000-0000-000000000001"));
            } else {
                assert!(rendered.contains("attempt=#2"));
                assert!(rendered.contains("parent=#1"));
                assert!(rendered.contains("path select=true decide=true play=true result=true"));
            }
        }
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one rendering test covers both retained result entries and layout sizes"
    )]
    fn result_history_tui_prioritizes_song_and_play_result_without_json() {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        let song = SongPresentation {
            scorepeek_song_id: song_id,
            display_titles: vec!["HISTORY TITLE".to_owned(), "HISTORY ALIAS".to_owned()],
            artist: "HISTORY ARTIST".to_owned(),
        };
        let mut state = RunViewState::new("invocation-1".to_owned(), "a".repeat(64), true);
        state.watcher_state = "session_active".to_owned();
        state.latest_play_attempt = Some(json!({"state": {"status": "attempt"}}));
        state.stable_result_song = Some(song);
        let result = detected_result_event(
            "session-1",
            1,
            42,
            ResultDomainEvent {
                contract: "scorepeek-result-detected-v2".to_owned(),
                attempt_id: 1,
                parent_attempt_id: None,
                scorepeek_song_id: song_id,
                play_side: "one_player".to_owned(),
                play_mode: "single_play".to_owned(),
                play_type: PlayType::Single,
                difficulty: Difficulty::Hyper,
                level: 8,
                notes: 764,
                current_score: 1_286,
                clear_type: "HARD CLEAR".to_owned(),
                judgments: ResultJudgments {
                    pgreat: 600,
                    great: 86,
                    good: 10,
                    bad: 5,
                    poor: 3,
                },
                miss_count: SupplementalResultValue::Known { value: 3 },
                timing: ResultTiming {
                    fast: SupplementalResultValue::Known { value: 20 },
                    slow: SupplementalResultValue::Known { value: 21 },
                },
                combo_break: SupplementalResultValue::Known { value: 2 },
                previous_best: PreviousBest {
                    clear_type: scorepeek::recognition::PreviousBestValue::Known {
                        value: "CLEAR".to_owned(),
                    },
                    score: scorepeek::recognition::PreviousBestValue::Known { value: 1_200 },
                    miss_count: scorepeek::recognition::PreviousBestValue::Known { value: 4 },
                },
            },
        );
        state.reduce(&result, &result.to_value().unwrap());

        let health = ChannelHealth::default();
        for (width, height, compact) in [
            (100, 40, false),
            (70, 30, true),
            (100, 30, true),
            (70, 12, true),
            (40, 30, true),
        ] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render(frame, &state, Path::new("/run/scorepeek.sock"), &health))
                .unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>();
            assert!(rendered.contains("Accepted play events"));
            assert!(rendered.contains("#1 HARD CLEAR"));
            assert!(rendered.contains("SP HYPER Lv8"));
            assert!(rendered.contains("Title: HISTORY TITLE"));
            if compact {
                assert!(rendered.contains("EX 1,286 / 1,528 (84.1%)"));
                assert!(rendered.contains("PG 600 GR 86 GD 10 BD 5 PR 3"));
                assert!(rendered.contains("MISS 3 F 20 S 21 CB 2"));
                assert!(rendered.contains("PREV clear=CLEAR EX=1,200 MISS=4"));
                assert!(!rendered.contains("Artist: HISTORY ARTIST"));
                assert!(!rendered.contains("attempt=#1"));
                assert!(!rendered.contains("Debug:"));
            } else {
                assert!(rendered.contains("EX SCORE 1,286 / 1,528 (84.1%)"));
                assert!(rendered.contains("PGREAT 600  GREAT 86  GOOD 10  BAD 5  POOR 3"));
                assert!(rendered.contains("MISS 3  FAST 20  SLOW 21  COMBO BREAK 2"));
                assert!(rendered.contains("Previous: clear=CLEAR  EX SCORE 1,200  MISS 4"));
                assert!(rendered.contains("Artist: HISTORY ARTIST"));
                assert!(rendered.contains(
                    "attempt=#1 parent=#-  notes=764 side=one_player mode=single_play sequence=42"
                ));
            }
            assert!(!rendered.contains("scorepeek_song_id"));
            assert!(!rendered.contains("current_score"));
            assert!(!rendered.contains("HISTORY ALIAS"));
        }

        let mut upper_bound_entry = state.result_history.back().unwrap().clone();
        upper_bound_entry.result.notes = 2_500;
        upper_bound_entry.result.current_score = 2_500;
        upper_bound_entry.result.judgments = ResultJudgments {
            pgreat: 1_000,
            great: 500,
            good: 2_500,
            bad: 2_500,
            poor: 2_500,
        };
        upper_bound_entry.result.miss_count = SupplementalResultValue::Known { value: 2_500 };
        upper_bound_entry.result.timing = ResultTiming {
            fast: SupplementalResultValue::Known { value: 2_500 },
            slow: SupplementalResultValue::Known { value: 2_500 },
        };
        upper_bound_entry.result.combo_break = SupplementalResultValue::Known { value: 2_500 };
        upper_bound_entry.result.previous_best = PreviousBest {
            clear_type: PreviousBestValue::Known {
                value: "F-COMBO".to_owned(),
            },
            score: PreviousBestValue::Known { value: 5_000 },
            miss_count: PreviousBestValue::Known { value: 2_500 },
        };
        let packed = compact_result_history_entry_lines(&upper_bound_entry, 38);
        assert!(packed.iter().all(|line| line.width() <= 38));
        let packed_text = packed
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "PG 1,000",
            "GR 500",
            "GD 2,500",
            "BD 2,500",
            "PR 2,500",
            "MISS 2,500",
            "F 2,500",
            "S 2,500",
            "CB 2,500",
            "clear=F-COMBO",
            "EX=5,000",
            "MISS=2,500",
        ] {
            assert!(packed_text.contains(expected));
        }

        let second_song_id =
            serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
        state.stable_result_song = Some(SongPresentation {
            scorepeek_song_id: second_song_id,
            display_titles: vec!["SECOND TITLE".to_owned()],
            artist: "SECOND ARTIST".to_owned(),
        });
        let second = detected_result_event(
            "session-2",
            2,
            84,
            ResultDomainEvent {
                contract: "scorepeek-result-detected-v2".to_owned(),
                attempt_id: 2,
                parent_attempt_id: None,
                scorepeek_song_id: second_song_id,
                play_side: "one_player".to_owned(),
                play_mode: "single_play".to_owned(),
                play_type: PlayType::Single,
                difficulty: Difficulty::Another,
                level: 10,
                notes: 1_000,
                current_score: 1_500,
                clear_type: "FAILED".to_owned(),
                judgments: ResultJudgments {
                    pgreat: 700,
                    great: 100,
                    good: 20,
                    bad: 10,
                    poor: 5,
                },
                miss_count: SupplementalResultValue::NotDisplayed,
                timing: ResultTiming {
                    fast: SupplementalResultValue::Known { value: 30 },
                    slow: SupplementalResultValue::Known { value: 31 },
                },
                combo_break: SupplementalResultValue::Known { value: 5 },
                previous_best: PreviousBest {
                    clear_type: scorepeek::recognition::PreviousBestValue::NotPlayed,
                    score: scorepeek::recognition::PreviousBestValue::NotPlayed,
                    miss_count: scorepeek::recognition::PreviousBestValue::NotPlayed,
                },
            },
        );
        state.reduce(&second, &second.to_value().unwrap());
        let backend = TestBackend::new(100, 60);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, &state, Path::new("/run/scorepeek.sock"), &health))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("2 completed"));
        assert!(rendered.contains("#2 FAILED"));
        assert!(rendered.contains("Title: SECOND TITLE"));
        assert!(rendered.contains("MISS --  FAST 30  SLOW 31  COMBO BREAK 5"));
        assert!(rendered.contains("Previous: clear=NO PLAY  EX SCORE NO PLAY  MISS NO PLAY"));
        assert!(rendered.contains("#1 HARD CLEAR"));
        assert!(rendered.contains("Title: HISTORY TITLE"));
        assert!(rendered.find("#2 FAILED") < rendered.find("#1 HARD CLEAR"));
    }

    #[test]
    fn result_history_is_bounded_and_survives_session_changes() {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        let mut state = RunViewState::new("invocation-1".to_owned(), "a".repeat(64), true);
        state.stable_result_song = Some(SongPresentation {
            scorepeek_song_id: song_id,
            display_titles: vec!["TITLE".to_owned()],
            artist: "ARTIST".to_owned(),
        });
        for source_sequence in 1..=(RESULT_HISTORY_CAPACITY as u64 + 3) {
            let result = detected_result_event(
                "session-1",
                1,
                source_sequence,
                ResultDomainEvent {
                    contract: "scorepeek-result-detected-v2".to_owned(),
                    attempt_id: source_sequence,
                    parent_attempt_id: None,
                    scorepeek_song_id: song_id,
                    play_side: "one_player".to_owned(),
                    play_mode: "single_play".to_owned(),
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Normal,
                    level: 5,
                    notes: 100,
                    current_score: 150,
                    clear_type: "CLEAR".to_owned(),
                    judgments: ResultJudgments {
                        pgreat: 70,
                        great: 10,
                        good: 5,
                        bad: 3,
                        poor: 2,
                    },
                    miss_count: SupplementalResultValue::Known { value: 2 },
                    timing: ResultTiming {
                        fast: SupplementalResultValue::Known { value: 4 },
                        slow: SupplementalResultValue::Known { value: 5 },
                    },
                    combo_break: SupplementalResultValue::Known { value: 1 },
                    previous_best: PreviousBest {
                        clear_type: scorepeek::recognition::PreviousBestValue::NotPlayed,
                        score: scorepeek::recognition::PreviousBestValue::NotPlayed,
                        miss_count: scorepeek::recognition::PreviousBestValue::NotPlayed,
                    },
                },
            );
            state.reduce(&result, &result.to_value().unwrap());
        }
        assert_eq!(state.result_count, RESULT_HISTORY_CAPACITY as u64 + 3);
        assert_eq!(state.result_history.len(), RESULT_HISTORY_CAPACITY);
        assert_eq!(state.result_history.front().unwrap().ordinal, 4);
        assert_eq!(
            state.result_history.back().unwrap().ordinal,
            RESULT_HISTORY_CAPACITY as u64 + 3
        );

        let next_session = RunEvent {
            schema: "scorepeek-run-event-v2".to_owned(),
            kind: RunEventKind::SessionStarted {
                session_id: Some("session-2".to_owned()),
                capture_generation: 2,
                capture_profile_sha256: "b".repeat(64),
                normalizer_artifact_sha256: "c".repeat(64),
            },
        };
        state.reduce(&next_session, &next_session.to_value().unwrap());
        assert_eq!(state.result_history.len(), RESULT_HISTORY_CAPACITY);
        assert_eq!(state.result_count, RESULT_HISTORY_CAPACITY as u64 + 3);
        assert!(state.stable_result_song.is_none());
    }

    #[test]
    fn result_value_labels_preserve_domain_states_without_debug_reasons() {
        use scorepeek::recognition::ResultFieldUnknownReason;

        assert_eq!(
            supplemental_u32(&SupplementalResultValue::Known { value: 1_234 }),
            "1,234"
        );
        assert_eq!(
            supplemental_u32(&SupplementalResultValue::NotDisplayed),
            "--"
        );
        assert_eq!(
            supplemental_u32(&SupplementalResultValue::Unknown {
                reason: ResultFieldUnknownReason::InvalidFormat,
            }),
            "?"
        );
        assert_eq!(previous_text(&PreviousBestValue::NotPlayed), "NO PLAY");
        assert_eq!(previous_u32(&PreviousBestValue::NotDisplayed), "--");
        assert_eq!(
            previous_u32(&PreviousBestValue::Unknown {
                reason: ResultFieldUnknownReason::OutOfRange,
            }),
            "?"
        );
    }

    #[test]
    fn fitted_song_text_uses_an_ellipsis_without_mutating_the_value() {
        let value = "非常に長い曲名を完全な状態で保持する";
        let rendered = fitted_value("Catalog title: ", value, 24);
        assert!(rendered.starts_with("Catalog title: "));
        assert!(rendered.ends_with('…'));
        assert!(Line::raw(rendered).width() <= 24);
        assert_eq!(value, "非常に長い曲名を完全な状態で保持する");
    }
}
