#![allow(
    clippy::missing_errors_doc,
    reason = "internal event service methods retain their existing operation-specific errors"
)]

use super::client::{ChannelHealth, EventChannel, QueuedEvent};
#[cfg(test)]
use super::client::{
    EVENT_QUEUE_CAPACITY, EventClient, MAX_CLIENTS, SOCKET_NAME, SocketPathGuard, accept_clients,
    broadcast, snapshot_bytes, try_send_event,
};
#[cfg(test)]
use super::music_select_best;
use super::snapshot as event_api;
#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::VecDeque;
#[cfg(test)]
use std::fs;
#[cfg(test)]
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
#[cfg(test)]
use std::os::unix::net::UnixListener;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::diagnostics::inspect::{DiagnosticSink, RunDiagnostics};
#[cfg(test)]
use ratatui::layout::{Constraint, Direction, Layout};
#[cfg(test)]
use ratatui::style::{Color, Modifier, Style};
#[cfg(test)]
use ratatui::text::{Line, Span};
#[cfg(test)]
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
#[cfg(test)]
use scorepeek_core::catalog::{Difficulty, PlayType};
#[cfg(test)]
use scorepeek_core::event::{
    AttemptNodeSnapshot, EVIDENCE_FAMILY_CAP, GateSnapshot, GateState, HypothesisAccumulator,
    JointKey, MusicSelectResolver, PlayOptionsDebugSnapshot, PlayOptionsEpisodeAccumulator,
    ResolverNodeSnapshot, ResultChartFactor, ResultPanelSideAccumulator, SelectionEpochTracker,
    candidate_song_presentation, selected_difficulty, selected_play_type,
    test_result_play_side as result_play_side, test_selected_play_side as selected_play_side,
};
#[cfg(test)]
use scorepeek_core::event::{BestChart, ResultPanelSideEpisodeState};
#[cfg(test)]
use scorepeek_core::event::{
    CurrentSelectionDifficulty, EvidenceContribution, MusicSelectionUnresolvedReason,
    ResolverResolutionState, ResultPanelSideTransitionReason, ResultRetractionReason,
    SelectIdentityStatus, SelectionDifficultyTarget, SelectionDifficultyTransitionReason,
    SongResolutionPresentation,
};
use scorepeek_core::event::{
    MusicSelectResolverState, MusicSelectionState, RUN_EVENT_SCHEMA, ResultDomainEvent,
    ResultState, RunEvent, RunEventKind, SongPresentation, diagnostic_run_event_value,
};
use scorepeek_core::event::{
    RunEventReducer, RunReducerEffect, RunReducerSnapshot as ResolverDebugSnapshot,
};
#[cfg(test)]
use scorepeek_core::recognition::music_select::PlaySide;
#[cfg(test)]
use scorepeek_core::recognition::result::{
    ParsedResultFields, PlayOptions, PlayOptionsObservation, ResultPerformanceResolution,
};
#[cfg(test)]
use scorepeek_core::recognition::result::{
    PlayOption, PlayOptionsUnknownReason, PreviousBest, PreviousBestValue, ResultChartResolution,
    ResultJudgments, ResultTiming, SupplementalResultValue,
};
#[cfg(test)]
use scorepeek_core::recognition::screen::ResultPanelSide;
#[cfg(test)]
use scorepeek_core::recognition::shared::{
    EvidenceFamily, JointEvidenceCandidate, JointEvidenceObservation,
};
#[cfg(test)]
use scorepeek_core::session::attempt::{PlayAttemptReason, PlayAttemptScreen, PlayAttemptState};
use scorepeek_core::session::timeline::SemanticEpisodePhase;
use serde::Serialize;
use serde_json::{Value, json};

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

const RESULT_HISTORY_CAPACITY: usize = 32;

#[derive(Clone, Debug, Serialize)]
struct ResultHistoryEntry {
    ordinal: u64,
    session_id: String,
    capture_generation: u64,
    source_sequence: u64,
    song: Option<SongPresentation>,
    result: ResultDomainEvent,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunViewState {
    #[serde(skip)]
    pub(super) public: event_api::PublicState,
    invocation_id: String,
    profile_sha256: String,
    recording: &'static str,
    watcher_state: String,
    scores_summary: Option<String>,
    channel_start_failure: Option<String>,
    session_count: u64,
    active_session_id: Option<String>,
    capture_generation: Option<u64>,
    current_screen: Option<String>,
    raw_screen: Option<String>,
    #[serde(skip)]
    latest_observation: Option<Value>,
    #[serde(skip)]
    latest_stabilized_result: Option<Value>,
    #[serde(skip)]
    latest_temporal_music_select: Option<Value>,
    #[serde(skip)]
    latest_play_attempt: Option<Value>,
    #[serde(skip)]
    latest_numeric_result: Option<Value>,
    latest_result_detected: Option<Value>,
    latest_provisional_result: Option<ResultHistoryEntry>,
    latest_result_label: Option<&'static str>,
    latest_music_selection: Option<MusicSelectionState>,
    music_select: MusicSelectResolverState,
    result_history: VecDeque<ResultHistoryEntry>,
    result_count: u64,
    #[serde(skip)]
    stable_result_song: Option<SongPresentation>,
    latest_report: Option<Value>,
    status_recording: &'static str,
    recording_memory_limit_bytes: u64,
    recording_memory_used_bytes: u64,
    recording_memory_high_water_bytes: u64,
    recording_dropped_frames: u64,
    next_channel_sequence: u64,
    #[serde(skip)]
    overlay_summary: String,
    message: String,
    resolver: ResolverDebugSnapshot,
}

impl RunViewState {
    fn new(invocation_id: String, profile_sha256: String, recording_enabled: bool) -> Self {
        let mut public = event_api::PublicState::new(invocation_id.clone());
        if recording_enabled {
            public.enable_recording();
        }
        Self {
            public,
            invocation_id,
            profile_sha256,
            recording: if recording_enabled {
                "enabled"
            } else {
                "disabled"
            },
            watcher_state: "starting".to_owned(),
            scores_summary: None,
            overlay_summary: String::new(),
            channel_start_failure: None,
            session_count: 0,
            active_session_id: None,
            capture_generation: None,
            current_screen: None,
            raw_screen: None,
            latest_observation: None,
            latest_stabilized_result: None,
            latest_temporal_music_select: None,
            latest_play_attempt: None,
            latest_numeric_result: None,
            latest_result_detected: None,
            latest_provisional_result: None,
            latest_result_label: None,
            latest_music_selection: None,
            music_select: MusicSelectResolverState::default(),
            result_history: VecDeque::with_capacity(RESULT_HISTORY_CAPACITY),
            result_count: 0,
            stable_result_song: None,
            latest_report: None,
            status_recording: if recording_enabled {
                "armed"
            } else {
                "disabled"
            },
            recording_memory_limit_bytes: 0,
            recording_memory_used_bytes: 0,
            recording_memory_high_water_bytes: 0,
            recording_dropped_frames: 0,
            next_channel_sequence: 1,
            message: "initializing".to_owned(),
            resolver: ResolverDebugSnapshot::default(),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn reduce(&mut self, event: &RunEvent, serialized: &Value) {
        match &event.kind {
            RunEventKind::MusicSelectResolverChanged { state, .. } => {
                self.music_select = state.clone();
            }
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
                self.music_select = MusicSelectResolverState::default();
                self.raw_screen = None;
                self.latest_observation = None;
                self.latest_stabilized_result = None;
                self.latest_temporal_music_select = None;
                self.latest_play_attempt = None;
                self.latest_numeric_result = None;
                self.latest_provisional_result = None;
                self.latest_result_label = None;
                self.latest_music_selection = None;
                self.music_select = MusicSelectResolverState::default();
                self.stable_result_song = None;
                self.latest_report = None;
                if self.recording == "enabled" && self.status_recording != "degraded" {
                    self.status_recording = "armed";
                }
                "capture session admitted".clone_into(&mut self.message);
            }
            RunEventKind::RecordingHealthChanged {
                state,
                memory_limit_bytes,
                memory_used_bytes,
                memory_high_water_bytes,
                dropped_frames,
                ..
            } => {
                self.status_recording = match state.as_str() {
                    "active" => "active",
                    "pressured" => "pressured",
                    _ => "degraded",
                };
                self.recording_memory_limit_bytes = *memory_limit_bytes;
                self.recording_memory_used_bytes = *memory_used_bytes;
                self.recording_memory_high_water_bytes = *memory_high_water_bytes;
                self.recording_dropped_frames = *dropped_frames;
            }
            RunEventKind::RecordingFinalizing { .. } => {
                if self.status_recording != "degraded" {
                    self.status_recording = "finalizing";
                }
                "session recording finalizing".clone_into(&mut self.message);
            }
            RunEventKind::RecordingCompleted { session_id, .. } => {
                self.status_recording = "ready";
                self.message = format!("session recording ready: {session_id}");
            }
            RunEventKind::ScreenChanged { screen, .. } => {
                if screen == "result" {
                    self.latest_numeric_result = None;
                }
                self.current_screen = Some(screen.clone());
            }
            RunEventKind::RawScreenObserved { screen, .. } => {
                self.raw_screen = Some(screen.clone());
            }
            RunEventKind::SemanticScreenEpisodeChanged { screen, phase, .. } => match phase {
                SemanticEpisodePhase::Started | SemanticEpisodePhase::Resumed => {
                    self.current_screen = Some(screen.clone());
                }
                SemanticEpisodePhase::Finalized => self.current_screen = None,
                SemanticEpisodePhase::Suspended | SemanticEpisodePhase::Closing => {}
            },
            RunEventKind::GameVersionChanged { .. }
            | RunEventKind::OverlayObserved { .. }
            | RunEventKind::MusicSelectBestObserved { .. }
            | RunEventKind::ScreenTick { .. }
            | RunEventKind::ResolverStateChanged { .. }
            | RunEventKind::SelectionDifficultyChanged { .. }
            | RunEventKind::ResultPanelSideChanged { .. }
            | RunEventKind::ResultSelectContextMismatch { .. } => {}
            RunEventKind::MusicSelectionChanged { state, .. } => {
                self.latest_music_selection = Some(state.clone());
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
            RunEventKind::NumericResultChanged { .. } => {
                self.latest_numeric_result = Some(serialized.clone());
            }
            RunEventKind::PlayAttemptChanged { .. } => {
                self.latest_play_attempt = Some(serialized.clone());
            }
            RunEventKind::ResultChanged {
                session_id,
                capture_generation,
                source_sequence,
                state,
            } => match state {
                ResultState::Inactive => {
                    self.latest_provisional_result = None;
                    self.latest_result_label = Some("INACTIVE");
                }
                ResultState::Provisional { song, result } => {
                    self.latest_result_label = Some("PROVISIONAL");
                    self.latest_provisional_result = Some(ResultHistoryEntry {
                        ordinal: self.result_count.saturating_add(1),
                        session_id: session_id.clone(),
                        capture_generation: *capture_generation,
                        source_sequence: *source_sequence,
                        song: song
                            .as_ref()
                            .filter(|song| song.scorepeek_song_id == result.scorepeek_song_id)
                            .cloned(),
                        result: result.as_ref().clone(),
                    });
                }
                ResultState::Retracted { song, result, .. } => {
                    self.latest_result_label = Some("RETRACTED");
                    self.latest_provisional_result = Some(ResultHistoryEntry {
                        ordinal: self.result_count.saturating_add(1),
                        session_id: session_id.clone(),
                        capture_generation: *capture_generation,
                        source_sequence: *source_sequence,
                        song: song.clone(),
                        result: result.as_ref().clone(),
                    });
                }
                ResultState::Confirmed { song, result } => {
                    self.latest_provisional_result = None;
                    self.latest_result_label = None;
                    self.latest_result_detected = Some(serialized.clone());
                    self.result_count = self.result_count.saturating_add(1);
                    if self.result_history.len() == RESULT_HISTORY_CAPACITY {
                        self.result_history.pop_front();
                    }
                    self.result_history.push_back(ResultHistoryEntry {
                        ordinal: self.result_count,
                        session_id: session_id.clone(),
                        capture_generation: *capture_generation,
                        source_sequence: *source_sequence,
                        song: song.clone(),
                        result: result.as_ref().clone(),
                    });
                }
            },
            RunEventKind::SessionFinished {
                outcome, report, ..
            } => {
                "session_finished".clone_into(&mut self.watcher_state);
                self.active_session_id = None;
                self.capture_generation = None;
                self.current_screen = None;
                self.music_select = MusicSelectResolverState::default();
                self.raw_screen = None;
                self.latest_report = Some(report.clone());
                self.message = format!("session finished: {outcome}");
                if self.recording != "disabled" && self.status_recording != "degraded" {
                    self.status_recording = "finalizing";
                }
            }
            RunEventKind::WatcherStopped { .. } => {
                "stopped".clone_into(&mut self.watcher_state);
                self.active_session_id = None;
                self.capture_generation = None;
                self.current_screen = None;
                self.music_select = MusicSelectResolverState::default();
                self.raw_screen = None;
                self.latest_observation = None;
                self.latest_stabilized_result = None;
                self.latest_temporal_music_select = None;
                self.latest_play_attempt = None;
                self.latest_numeric_result = None;
                self.stable_result_song = None;
                "scorepeek stopped by signal".clone_into(&mut self.message);
            }
        }
    }
}

fn commit_public_projection(
    current: &mut event_api::PublicState,
    projected: event_api::PublicState,
    events: &[event_api::PublicRecord],
    channel: Option<&EventChannel>,
    scores: Option<&crate::scores::Worker>,
) -> Vec<Value> {
    if events.is_empty() {
        return Vec::new();
    }
    *current = projected;
    let records = events
        .iter()
        .map(event_api::encode)
        .collect::<Result<Vec<_>, _>>();
    if let Some(scores) = scores {
        match &records {
            Ok(records) => {
                for bytes in records {
                    scores.offer(bytes);
                }
            }
            Err(error) => scores.reject("event_encoding", error),
        }
    }
    let mut observations = Vec::with_capacity(events.len());
    let Some(channel) = channel else {
        for event in events {
            observations.push(json!({"public_event":event, "public_event_sequence":event.sequence, "enqueue":"channel_unavailable"}));
        }
        return observations;
    };
    if channel.health.server_failed.load(Ordering::Acquire) {
        for event in events {
            observations.push(json!({"public_event":event, "public_event_sequence":event.sequence, "enqueue":"worker_unavailable"}));
        }
        return observations;
    }
    if let Ok(records) = records.and_then(|records| event_api::encode(current).map(|_| records)) {
        for (event, bytes) in events.iter().zip(records) {
            let enqueue = channel.publish(QueuedEvent {
                sequence: event.sequence,
                bytes,
            });
            observations.push(json!({"public_event":event, "public_event_sequence":event.sequence, "enqueue":enqueue}));
        }
    } else {
        channel
            .health
            .oversized_records
            .fetch_add(1, Ordering::AcqRel);
        channel.health.server_failed.store(true, Ordering::Release);
        for event in events {
            observations.push(json!({"public_event":event, "public_event_sequence":event.sequence, "enqueue":"encoding_failed"}));
        }
    }
    observations
}

#[allow(clippy::struct_excessive_bools)]
pub struct RoutineOutput {
    state: Arc<Mutex<RunViewState>>,
    channel: Option<EventChannel>,
    scores: Option<crate::scores::Worker>,
    publish_frontend_snapshots: bool,
    next_sequence: u64,
    timing_active: bool,
    output_us: u64,
    #[cfg(test)]
    headless_events: Vec<RunEvent>,
    core_reducer: RunEventReducer,
    diagnostics: Option<RunDiagnostics>,
}

pub struct RoutineOutputStartError {
    message: String,
    diagnostics: RunDiagnostics,
}

impl RoutineOutputStartError {
    #[must_use]
    pub fn into_parts(self) -> (String, RunDiagnostics) {
        (self.message, self.diagnostics)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(
    clippy::struct_field_names,
    reason = "the unit suffix is part of the explicit frame-timing contract"
)]
pub struct RoutineEventProcessingTiming {
    pub screen_resolver_us: Option<u64>,
    pub attempt_resolver_us: Option<u64>,
    pub output_us: Option<u64>,
}

impl RoutineOutput {
    pub fn refresh_overlays(
        &mut self,
        children: &mut crate::overlay::supervisor::Children,
        controller: Option<&crate::config::control::Controller>,
    ) -> Result<(), String> {
        for message in children.poll() {
            self.warning(message)?;
        }
        self.state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .overlay_summary = children.summary();
        for observation in children.take_observations() {
            self.publish_overlay_observation(observation)?;
        }
        if let Some(controller) = controller {
            for record in controller.take_observations() {
                self.publish_overlay_observation(serde_json::json!({
                    "source":"controller", "record":record
                }))?;
            }
        }
        Ok(())
    }

    fn publish_overlay_observation(&mut self, observation: Value) -> Result<(), String> {
        self.publish_one_inner(
            &RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::OverlayObserved { observation },
            },
            false,
        )
    }
    #[must_use]
    pub fn event_socket_path(&self) -> Option<&Path> {
        self.channel
            .as_ref()
            .map(|channel| channel.socket_path.as_path())
    }

    pub fn diagnostic_run_root(&self) -> Option<&Path> {
        self.diagnostics.as_ref().and_then(RunDiagnostics::run_root)
    }

    pub fn finish_diagnostics(&mut self, operation_status: &str) {
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.finish(operation_status);
        }
    }

    pub fn record_diagnostic(&self, resource: &str, detail: &Value, critical: bool) {
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.sink().record(resource, detail, critical);
        }
    }

    /// Starts the public run output while retaining diagnostic ownership on failure.
    ///
    /// # Panics
    /// Panics only if the internal constructor loses diagnostics that this entry point supplied.
    pub fn start(
        invocation_id: String,
        profile_sha256: String,
        recording_enabled: bool,
        diagnostics: RunDiagnostics,
    ) -> Result<Self, RoutineOutputStartError> {
        let state = Arc::new(Mutex::new(RunViewState::new(
            invocation_id,
            profile_sha256,
            recording_enabled,
        )));
        let channel = EventChannel::start(Arc::clone(&state));
        Self::from_channel(state, channel, true, Some(diagnostics)).map_err(
            |(message, diagnostics)| RoutineOutputStartError {
                message,
                diagnostics: diagnostics.expect("start supplied diagnostic ownership"),
            },
        )
    }

    fn from_channel(
        state: Arc<Mutex<RunViewState>>,
        channel: Result<EventChannel, String>,
        publish_frontend_snapshots: bool,
        diagnostics: Option<RunDiagnostics>,
    ) -> Result<Self, (String, Option<RunDiagnostics>)> {
        let channel = match channel {
            Ok(channel) => Some(channel),
            Err(error) => {
                let Ok(mut state) = state.lock() else {
                    return Err(("run view state lock was poisoned".to_owned(), diagnostics));
                };
                state.channel_start_failure = Some(error);
                None
            }
        };
        let mut output = Self {
            state,
            channel,
            scores: None,
            publish_frontend_snapshots,
            next_sequence: 1,
            timing_active: false,
            output_us: 0,
            #[cfg(test)]
            headless_events: Vec::new(),
            core_reducer: RunEventReducer::new(),
            diagnostics,
        };
        match output.refresh() {
            Ok(()) => Ok(output),
            Err(message) => {
                let diagnostics = output.diagnostics.take();
                Err((message, diagnostics))
            }
        }
    }

    #[must_use]
    #[cfg(test)]
    pub fn start_headless(invocation_id: String, profile_sha256: String) -> Self {
        Self::start_headless_inner(invocation_id, profile_sha256, None)
    }

    #[must_use]
    #[allow(
        dead_code,
        reason = "the library entry point is consumed by corpus replay"
    )]
    pub fn start_headless_with_diagnostics(
        invocation_id: String,
        profile_sha256: String,
        diagnostics: RunDiagnostics,
    ) -> Self {
        Self::start_headless_inner(invocation_id, profile_sha256, Some(diagnostics))
    }

    #[allow(
        dead_code,
        reason = "the library entry point is consumed by corpus replay"
    )]
    fn start_headless_inner(
        invocation_id: String,
        profile_sha256: String,
        diagnostics: Option<RunDiagnostics>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(RunViewState::new(
                invocation_id,
                profile_sha256,
                false,
            ))),
            channel: None,
            scores: None,
            publish_frontend_snapshots: false,
            next_sequence: 1,
            timing_active: false,
            output_us: 0,
            #[cfg(test)]
            headless_events: Vec::new(),
            core_reducer: RunEventReducer::new(),
            diagnostics,
        }
    }

    #[cfg(test)]
    pub fn take_headless_events(&mut self) -> Vec<RunEvent> {
        std::mem::take(&mut self.headless_events)
    }

    pub fn refresh_scores(&mut self) -> Result<(), String> {
        let diagnostic_sink = self.diagnostics.as_ref().map(RunDiagnostics::sink);
        let completions = self
            .scores
            .as_ref()
            .map(crate::scores::Worker::take_completions)
            .unwrap_or_default();
        for completion in completions {
            let persisted = completion.outcome == crate::scores::CompletionOutcome::Persisted;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            let mut projected = state.public.clone();
            let mut events = Vec::new();
            if persisted && let Some(chart) = completion.chart {
                events.push(projected.score_store_changed(chart));
            }
            let observations = commit_public_projection(
                &mut state.public,
                projected,
                &events,
                self.channel.as_ref(),
                None,
            );
            if let Some(sink) = &diagnostic_sink {
                for observation in observations {
                    sink.record("public_event", &observation, false);
                }
            }
        }
        let persistence_failed = self
            .scores_health()
            .is_some_and(|health| health.failure.is_some());
        let mut state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?;
        let mut projected = state.public.clone();
        let health_events = projected
            .scores_health(!persistence_failed)
            .into_iter()
            .collect::<Vec<_>>();
        let observations = commit_public_projection(
            &mut state.public,
            projected,
            &health_events,
            self.channel.as_ref(),
            None,
        );
        drop(state);
        if let Some(sink) = &diagnostic_sink {
            for observation in observations {
                sink.record("public_event", &observation, false);
            }
        }
        self.refresh()
    }

    pub fn enable_scores(&mut self, path: &Path) -> Result<(), String> {
        self.scores = Some(crate::scores::Worker::start(path));
        let mut state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?;
        state.scores_summary = Some(format!("{}", path.display()));
        state.public.enable_scores();
        drop(state);
        self.refresh()
    }

    pub fn scores_health(&self) -> Option<crate::scores::Health> {
        self.scores.as_ref().map(crate::scores::Worker::health)
    }

    pub fn bind_public_session(&mut self, binding: event_api::Binding) {
        if let Ok(mut state) = self.state.lock() {
            state.public.pending_binding = Some(binding);
        }
    }

    pub fn publish(&mut self, event: &RunEvent) -> Result<(), String> {
        self.publish_timed(event).map(|_| ())
    }

    pub fn publish_timed(
        &mut self,
        event: &RunEvent,
    ) -> Result<RoutineEventProcessingTiming, String> {
        self.publish_timed_core(event)
    }

    fn publish_timed_core(
        &mut self,
        event: &RunEvent,
    ) -> Result<RoutineEventProcessingTiming, String> {
        self.timing_active = true;
        self.output_us = 0;
        let started = Instant::now();
        let reduced = match self.core_reducer.reduce(event) {
            Ok(reduced) => reduced,
            Err(error) => match error {},
        };
        for effect in reduced.into_effects() {
            match effect {
                RunReducerEffect::Event(event) => self.publish_one(&event)?,
                RunReducerEffect::ClearFieldObservation => {
                    let mut state = self
                        .state
                        .lock()
                        .map_err(|_| "run view state lock was poisoned".to_owned())?;
                    state.resolver.raw_fields.clear();
                    state.resolver.latest_field_sequence = None;
                    state.resolver.latest_field_ms = None;
                }
                RunReducerEffect::Snapshot(snapshot) => {
                    self.state
                        .lock()
                        .map_err(|_| "run view state lock was poisoned".to_owned())?
                        .resolver = snapshot;
                }
                RunReducerEffect::Refresh => self.refresh()?,
                RunReducerEffect::FinishScores => {
                    if let Some(scores) = &mut self.scores {
                        scores.finish();
                    }
                    self.refresh_scores()?;
                }
            }
        }
        let total_us = duration_us(started.elapsed());
        self.timing_active = false;
        let output_us = self.output_us.min(total_us);
        let resolver_us = total_us.saturating_sub(output_us);
        let (screen_resolver_us, attempt_resolver_us) = match &event.kind {
            RunEventKind::RawScreenObserved { .. }
            | RunEventKind::SemanticScreenEpisodeChanged { .. }
            | RunEventKind::ScreenChanged { .. }
            | RunEventKind::ScreenTick { .. } => (Some(resolver_us), None),
            RunEventKind::FieldObservation { .. } => (None, Some(resolver_us)),
            _ => (None, None),
        };
        Ok(RoutineEventProcessingTiming {
            screen_resolver_us,
            attempt_resolver_us,
            output_us: Some(output_us),
        })
    }

    #[cfg(test)]
    fn reduce_music_select_observation(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_end_ms: u64,
        fields: &Value,
        joint_evidence: &JointEvidenceObservation,
        presentation: &SongResolutionPresentation,
    ) -> Result<(), String> {
        self.publish(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::FieldObservation {
                session_id: session_id.cloned(),
                capture_generation,
                screen_episode_id: self.core_reducer.snapshot().screen_episode_id,
                sequence,
                monotonic_start_ms: monotonic_end_ms,
                monotonic_end_ms,
                screen: "music_select".to_owned(),
                fields: fields.clone(),
                result_song_resolution: Value::Null,
                music_select_song_resolution: Value::Null,
                parsed_result_fields: None,
                result_chart_resolution: None,
                result_performance_resolution: None,
                current_score_ocr_resolution: None,
                numeric_batch: None,
                joint_evidence: joint_evidence.clone(),
                processing_timing: Value::Null,
                song_resolution_presentation: Box::new(presentation.clone()),
            },
        })
    }

    #[cfg(test)]
    fn publish_screen_change(
        &mut self,
        event: &RunEvent,
        publish_event: bool,
    ) -> Result<(), String> {
        assert!(
            publish_event,
            "test adapter only accepts published screen changes"
        );
        self.publish(event)
    }
    fn publish_one(&mut self, event: &RunEvent) -> Result<(), String> {
        self.publish_one_inner(event, true)
    }

    fn publish_one_inner(&mut self, event: &RunEvent, refresh: bool) -> Result<(), String> {
        let output_started = Instant::now();
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let mut value = diagnostic_run_event_value(event)?;
        if let Some(object) = value.as_object_mut() {
            object.insert("channel_sequence".to_owned(), sequence.into());
        }
        let mut public_observations = Vec::new();
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            state.reduce(event, &value);
            state.next_channel_sequence = self.next_sequence;
            if event_api::PublicState::observes(event) {
                let mut projected = state.public.clone();
                let events = projected.project(event);
                public_observations = commit_public_projection(
                    &mut state.public,
                    projected,
                    &events,
                    self.channel.as_ref(),
                    self.scores.as_ref(),
                );
            }
        }
        if let Some(channel) = &self.channel {
            value["event_api_health"] = channel.health.value();
            if let Ok(state) = self.state.lock() {
                value["event_api_health"]["invocation_id"] = state.invocation_id.clone().into();
            }
        }
        if self.channel.is_none()
            && let Ok(state) = self.state.lock()
            && let Some(cause) = &state.channel_start_failure
        {
            value["event_api_health"] = json!({"status":"degraded", "error_type":"startup_failed", "cause":cause, "invocation_id":state.invocation_id});
        }
        if let Some(health) = self.scores_health() {
            value["scores_health"] = serde_json::to_value(health).unwrap_or(Value::Null);
        }
        if let Some(diagnostics) = &self.diagnostics {
            let sink: DiagnosticSink = diagnostics.sink();
            for observation in public_observations {
                sink.record("public_event", &observation, false);
            }
            value["diagnostic_health"] = sink.health();
            sink.record("run_event", &value, important_run_event(event));
        }
        #[cfg(test)]
        self.headless_events.push(event.clone());
        if self.timing_active {
            self.output_us = self
                .output_us
                .saturating_add(duration_us(output_started.elapsed()));
        }
        if refresh { self.refresh() } else { Ok(()) }
    }

    pub fn watcher_state(
        &mut self,
        state_name: &str,
        session_id: Option<&str>,
        generation: Option<u64>,
        message: &str,
    ) -> Result<(), String> {
        let diagnostic_sink = self.diagnostics.as_ref().map(RunDiagnostics::sink);
        let observations = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            state_name.clone_into(&mut state.watcher_state);
            state.active_session_id = session_id.map(ToOwned::to_owned);
            state.capture_generation = generation;
            message.clone_into(&mut state.message);
            let mut projected = state.public.clone();
            let events = projected
                .watcher(
                    event_api::WatcherStatus::from_internal(state_name)
                        .ok_or_else(|| "unknown public watcher state".to_owned())?,
                )
                .into_iter()
                .collect::<Vec<_>>();
            commit_public_projection(
                &mut state.public,
                projected,
                &events,
                self.channel.as_ref(),
                self.scores.as_ref(),
            )
        };
        if let Some(sink) = &diagnostic_sink {
            for observation in observations {
                sink.record("public_event", &observation, false);
            }
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
        let output_started = Instant::now();
        let mut state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .clone();
        if let Some(health) = self.scores_health()
            && let Some(path) = &state.scores_summary
        {
            state.scores_summary = Some(format!(
                "scores={} unsaved={} db={path}",
                if health.failure.is_some() {
                    "degraded"
                } else if health.flush.is_some() {
                    "saved"
                } else {
                    "active"
                },
                health.pending + health.rejected
            ));
        }
        if !self.publish_frontend_snapshots {
            return Ok(());
        }
        let unavailable = ChannelHealth {
            server_failed: AtomicBool::new(true),
            ..ChannelHealth::default()
        };
        let health = self
            .channel
            .as_ref()
            .map_or(&unavailable, |channel| channel.health.as_ref());
        let channel = health.value();
        let snapshot = scorepeek_frontend_api::ApplicationSnapshot {
            revision: scorepeek_frontend_api::Revision(state.next_channel_sequence),
            running: state.watcher_state != "stopped",
            diagnostic_run_id: self
                .diagnostics
                .as_ref()
                .and_then(RunDiagnostics::run_root)
                .and_then(Path::file_name)
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_owned),
            run: Some(scorepeek_frontend_api::RunSnapshot {
                watcher_state: state.watcher_state.clone(),
                session_count: state.session_count,
                active_session_id: state.active_session_id.clone(),
                capture_generation: state.capture_generation,
                raw_screen: state.raw_screen.clone(),
                semantic_screen: state.current_screen.clone(),
                recording_status: state.status_recording.to_owned(),
                recording_memory_used_bytes: state.recording_memory_used_bytes,
                recording_memory_limit_bytes: state.recording_memory_limit_bytes,
                recording_dropped_frames: state.recording_dropped_frames,
                event_stream_status: channel["status"].as_str().unwrap_or("degraded").to_owned(),
                connected_clients: channel["connected_clients"].as_u64().unwrap_or(0),
                dropped_events: channel["dropped_events"].as_u64().unwrap_or(0),
                disconnected_clients: channel["disconnected_clients"].as_u64().unwrap_or(0),
                result_count: state.result_count,
                latest_result_label: state.latest_result_label.map(str::to_owned),
                scores_summary: state.scores_summary.clone(),
                overlay_summary: state.overlay_summary.clone(),
                message: state.message.clone(),
            }),
        };
        let _ = crate::service::dispatch::frontend_event(
            scorepeek_frontend_api::FrontendEvent::Snapshot {
                snapshot: Box::new(snapshot),
            },
        );
        let result = Ok(());
        if self.timing_active {
            self.output_us = self
                .output_us
                .saturating_add(duration_us(output_started.elapsed()));
        }
        result
    }
}

fn important_run_event(event: &RunEvent) -> bool {
    matches!(
        event.kind,
        RunEventKind::WatcherStarted { .. }
            | RunEventKind::WatcherStopped { .. }
            | RunEventKind::SessionStarted { .. }
            | RunEventKind::SessionFinished { .. }
            | RunEventKind::RecordingHealthChanged { .. }
            | RunEventKind::RecordingFinalizing { .. }
            | RunEventKind::RecordingCompleted { .. }
    )
}

impl Drop for RoutineOutput {
    fn drop(&mut self) {
        if let Some(scores) = &mut self.scores {
            scores.finish();
            let _ = self.refresh();
        }
    }
}

#[cfg(test)]
fn elapsed_seconds(now_ms: u64, started_ms: Option<u64>) -> u64 {
    started_ms.map_or(0, |started| now_ms.saturating_sub(started) / 1_000)
}

#[cfg(test)]
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!(
            "{}.{:02}GiB",
            bytes / GIB,
            (bytes % GIB).saturating_mul(100) / GIB
        )
    } else if bytes >= MIB {
        format!(
            "{}.{}MiB",
            bytes / MIB,
            (bytes % MIB).saturating_mul(10) / MIB
        )
    } else if bytes >= KIB {
        format!(
            "{}.{}KiB",
            bytes / KIB,
            (bytes % KIB).saturating_mul(10) / KIB
        )
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
fn plain_status_line(state: &RunViewState, health: &ChannelHealth) -> String {
    let channel = health.value();
    format!(
        "scorepeek: state={} sessions={} session={} generation={} channel={} clients={} dropped={} disconnected={} message={} {}{}",
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
        state
            .channel_start_failure
            .as_deref()
            .unwrap_or(&state.message),
        state.scores_summary.as_deref().unwrap_or("scores=disabled"),
        state.overlay_summary,
    )
}

#[cfg(test)]
fn render(
    frame: &mut ratatui::Frame<'_>,
    state: &RunViewState,
    _socket_path: &Path,
    health: &ChannelHealth,
) {
    let area = frame.area();
    let available_width = area.width.saturating_sub(2) as usize;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(fixed_watcher_lines(state, health)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(watcher_color(state, health)))
                .title(state.scores_summary.as_ref().map_or_else(
                    || "Watcher".to_owned(),
                    |scores| format!("Watcher {scores}"),
                )),
        ),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new(fixed_domain_lines(state, available_width)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(match state.latest_result_label {
                    Some("RETRACTED") => Color::Red,
                    Some("PROVISIONAL") => Color::Yellow,
                    Some("INACTIVE") => Color::DarkGray,
                    _ if state.latest_result_detected.is_some() => Color::Green,
                    _ => Color::DarkGray,
                }))
                .title("Latest result"),
        ),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(if rows[3].height < 12 {
            compact_resolver_lines(&state.resolver, available_width)
        } else {
            resolver_lines(&state.resolver, available_width)
        })
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(resolver_color(&state.resolver)))
                .title("Resolver"),
        ),
        rows[3],
    );
    frame.render_widget(
        Paragraph::new(music_select_best::lines(
            &state.music_select,
            available_width,
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Music Select Resolver")
                .border_style(Style::default().fg(if state.music_select.suspended {
                    Color::Yellow
                } else if state.music_select.chart.is_some() {
                    Color::Cyan
                } else {
                    Color::DarkGray
                })),
        ),
        rows[2],
    );
}

#[cfg(test)]
fn fixed_watcher_lines(state: &RunViewState, health: &ChannelHealth) -> Vec<Line<'static>> {
    let channel = health.value();
    let resolver = &state.resolver;
    vec![
        Line::from(vec![
            Span::styled(
                state.watcher_state.clone(),
                Style::default().fg(watcher_color(state, health)),
            ),
            Span::raw(format!(
                "  raw={} semantic=",
                resolver.raw_screen.as_deref().unwrap_or("-"),
            )),
            Span::styled(
                resolver.screen.clone().unwrap_or_else(|| "-".to_owned()),
                Style::default().fg(if resolver.suspended {
                    Color::Yellow
                } else {
                    Color::Cyan
                }),
            ),
            Span::raw(format!(
                "{} episode=#{} {}s  sessions={} gen={}",
                if resolver.suspended { " suspended" } else { "" },
                resolver.screen_episode_id,
                elapsed_seconds(resolver.now_ms, resolver.screen_episode_started_ms),
                state.session_count,
                state
                    .capture_generation
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            )),
        ]),
        Line::from(format!(
            "recording={} mem={}/{} high={} frame_drop={}  channel={} clients={} drop={}{}  {}",
            state.status_recording,
            human_bytes(state.recording_memory_used_bytes),
            human_bytes(state.recording_memory_limit_bytes),
            human_bytes(state.recording_memory_high_water_bytes),
            state.recording_dropped_frames,
            channel["status"].as_str().unwrap_or("degraded"),
            channel["connected_clients"].as_u64().unwrap_or(0),
            channel["dropped_events"].as_u64().unwrap_or(0),
            state.overlay_summary,
            state.message,
        )),
    ]
}

#[cfg(test)]
fn watcher_color(state: &RunViewState, health: &ChannelHealth) -> Color {
    let channel = health.value();
    if channel["status"].as_str() == Some("degraded")
        || channel["dropped_events"].as_u64().unwrap_or(0) > 0
        || state.status_recording == "degraded"
    {
        Color::Red
    } else if state.watcher_state == "stopped" {
        Color::DarkGray
    } else if state.watcher_state == "starting" || state.resolver.suspended {
        Color::Yellow
    } else {
        Color::Green
    }
}

#[cfg(test)]
fn resolution_color(state: ResolverResolutionState) -> Color {
    match state {
        ResolverResolutionState::AcceptedJoint => Color::Green,
        ResolverResolutionState::Conflict => Color::Red,
        ResolverResolutionState::SongProjected | ResolverResolutionState::JointCandidate => {
            Color::Cyan
        }
        ResolverResolutionState::Unresolved => Color::Yellow,
    }
}

#[cfg(test)]
fn resolver_color(snapshot: &ResolverDebugSnapshot) -> Color {
    if snapshot
        .gates
        .iter()
        .any(|gate| gate.state == GateState::Failed)
    {
        Color::Red
    } else if snapshot
        .gates
        .iter()
        .any(|gate| gate.label == "emit" && gate.state == GateState::Accepted)
    {
        Color::Green
    } else if snapshot.suspended || snapshot.finalizing {
        Color::Yellow
    } else if snapshot.local.is_some() {
        Color::Cyan
    } else {
        Color::DarkGray
    }
}

#[cfg(test)]
const fn gate_color(state: GateState) -> Color {
    match state {
        GateState::Accepted => Color::Green,
        GateState::Pending => Color::Yellow,
        GateState::Failed => Color::Red,
        GateState::Inactive => Color::DarkGray,
    }
}

#[cfg(test)]
const fn gate_suffix(state: GateState) -> &'static str {
    match state {
        GateState::Accepted => "✓",
        GateState::Pending => "…",
        GateState::Failed => "✗",
        GateState::Inactive => "–",
    }
}

#[cfg(test)]
fn fixed_domain_lines(state: &RunViewState, available_width: usize) -> Vec<Line<'static>> {
    if state.latest_result_label == Some("INACTIVE") {
        return vec![Line::from(Span::styled(
            "No active result",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    if let Some(entry) = state.latest_provisional_result.as_ref() {
        let mut lines = expanded_result_history_entry_lines(
            entry,
            available_width,
            state.latest_result_label.unwrap_or("PROVISIONAL"),
        );
        lines.truncate(6);
        return lines;
    }
    let Some(entry) = state.result_history.back() else {
        return vec![Line::from(Span::styled(
            "No result payload yet",
            Style::default().fg(Color::DarkGray),
        ))];
    };
    let mut lines = expanded_result_history_entry_lines(
        entry,
        available_width,
        &format!("CONFIRMED #{}", entry.ordinal),
    );
    lines.truncate(6);
    lines
}

#[cfg(test)]
fn compact_resolver_lines(snapshot: &ResolverDebugSnapshot, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if snapshot.screen.as_deref() == Some("result")
        && let Some(local) = &snapshot.local
    {
        lines.push(Line::from(fitted_value(
            &format!(
                "{:?} support={} songΔ={} chartΔ={} ",
                local.state, local.support, local.song_margin, local.chart_margin
            ),
            local.top.as_deref().unwrap_or("-"),
            width,
        )));
    } else {
        lines.push(Line::from("No active RESULT resolution"));
    }
    lines.push(Line::from(snapshot.attempt.as_ref().map_or_else(
        || "ATTEMPT -".to_owned(),
        |a| {
            format!(
                "ATTEMPT #{} {} {}",
                a.attempt_id
                    .map_or_else(|| "-".to_owned(), |id| id.to_string()),
                a.phase,
                a.path
            )
        },
    )));
    lines.push(Line::from(fitted_value("", &snapshot.gate, width)));
    lines.push(Line::from(
        snapshot
            .gates
            .iter()
            .map(|g| {
                Span::styled(
                    format!("{}{} ", g.label, gate_suffix(g.state)),
                    Style::default().fg(gate_color(g.state)),
                )
            })
            .collect::<Vec<_>>(),
    ));
    lines
}

#[allow(
    clippy::too_many_lines,
    reason = "the fixed ten-line debug tree keeps semantic styles adjacent to their typed values"
)]
#[cfg(test)]
fn resolver_lines(snapshot: &ResolverDebugSnapshot, available_width: usize) -> Vec<Line<'static>> {
    if snapshot.screen.as_deref() != Some("result") {
        return compact_resolver_lines(snapshot, available_width);
    }
    let semantic_color = if snapshot.suspended {
        Color::Yellow
    } else {
        Color::Cyan
    };
    let mut lines = vec![Line::from(vec![
        Span::raw(format!(
            "SCREEN raw={} semantic=",
            snapshot.raw_screen.as_deref().unwrap_or("-")
        )),
        Span::styled(
            snapshot.screen.clone().unwrap_or_else(|| "-".to_owned()),
            Style::default()
                .fg(semantic_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if snapshot.suspended { " suspended" } else { "" },
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(format!(
            "  episode=#{} {}s seq={}",
            snapshot.screen_episode_id,
            elapsed_seconds(snapshot.now_ms, snapshot.screen_episode_started_ms),
            snapshot
                .source_sequence
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        )),
    ])];
    lines.push(Line::from(vec![
        Span::styled("├─ FIELD ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            if snapshot.finalizing {
                "draining"
            } else {
                "completed"
            },
            Style::default().fg(if snapshot.finalizing {
                Color::Yellow
            } else if snapshot.latest_field_sequence.is_some() {
                Color::Green
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw(format!(
            " seq={} age={}s",
            snapshot
                .latest_field_sequence
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            elapsed_seconds(snapshot.now_ms, snapshot.latest_field_ms),
        )),
    ]));
    let raw = snapshot
        .raw_fields
        .iter()
        .map(|(key, value)| format!("{key}=\"{value}\""))
        .collect::<Vec<_>>();
    lines.push(Line::from(Span::styled(
        fitted_value(
            "│  OCR ",
            &raw.iter().take(3).cloned().collect::<Vec<_>>().join(" "),
            available_width,
        ),
        Style::default().fg(Color::White),
    )));
    if snapshot.screen.as_deref() == Some("result") {
        let (text, color) = snapshot.play_options.as_ref().map_or_else(
            || {
                (
                    "raw=- marker=- typed=unknown(not_observed)".to_owned(),
                    Color::DarkGray,
                )
            },
            |options| {
                let raw = options.latest.raw_text.as_deref().unwrap_or("-");
                let marker = format!(
                    "{:?}:{}",
                    options.latest.marker.state, options.latest.marker.orange_pixels
                )
                .to_ascii_lowercase();
                let typed = play_options_text(&options.resolved);
                let color = match options.resolved {
                    PlayOptions::Known { .. } => Color::Green,
                    PlayOptions::Unknown {
                        reason: PlayOptionsUnknownReason::ConflictingObservations,
                    } => Color::Red,
                    PlayOptions::Unknown { .. } => Color::Yellow,
                };
                (
                    format!(
                        "raw=\"{raw}\" marker={marker} typed={typed} stable={}{}",
                        options.observations,
                        if options.conflicting { " conflict" } else { "" },
                    ),
                    color,
                )
            },
        );
        lines.push(Line::from(Span::styled(
            fitted_value("│  OPT ", &text, available_width),
            Style::default().fg(color),
        )));
    }
    if let Some(local) = &snapshot.local {
        lines.push(Line::from(vec![
            Span::styled("├─ LOCAL ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:?}", local.state),
                Style::default().fg(resolution_color(local.state)),
            ),
            Span::raw(format!(
                " obs={} age={}s support={} songΔ={} chartΔ={}",
                local.observations,
                elapsed_seconds(snapshot.now_ms, local.last_observation_ms),
                local.support,
                local.song_margin,
                local.chart_margin,
            )),
        ]));
        lines.push(Line::from(Span::styled(
            fitted_value(
                "│  TOP ",
                local.top.as_deref().unwrap_or("-"),
                available_width,
            ),
            Style::default().fg(resolution_color(local.state)),
        )));
        lines.push(Line::from(fitted_value(
            "│  RUN song=",
            &format!(
                "{} chart={} {}={}",
                local.runner_song.as_deref().unwrap_or("-"),
                local.runner_chart.as_deref().unwrap_or("-"),
                if snapshot.successor.is_some() {
                    "successor"
                } else {
                    "#3"
                },
                snapshot
                    .successor
                    .as_ref()
                    .and_then(|value| value.top.as_deref())
                    .unwrap_or_else(|| local.top_candidates.get(2).map_or("-", String::as_str)),
            ),
            available_width,
        )));
    }
    if let Some(attempt) = &snapshot.attempt {
        lines.push(Line::from(vec![
            Span::styled("└─ ATTEMPT ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "{} phase={} path={}",
                    attempt
                        .attempt_id
                        .map_or_else(|| "-".to_owned(), |value| format!("#{value}")),
                    attempt.phase,
                    attempt.path,
                ),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(format!(
                " {}s",
                elapsed_seconds(snapshot.now_ms, attempt.started_ms)
            )),
        ]));
        lines.push(Line::from(fitted_value(
            "   FACTORS ",
            &attempt.family_contributions.join(" "),
            available_width,
        )));
    } else if let Some(local) = &snapshot.local {
        lines.push(Line::from(fitted_value(
            "│  FACTORS ",
            &local.family_contributions.join(" "),
            available_width,
        )));
    }
    let mut gate_spans = vec![Span::styled(
        "   GATES ",
        Style::default().fg(Color::DarkGray),
    )];
    for gate in &snapshot.gates {
        gate_spans.push(Span::styled(
            format!("{}{} ", gate.label, gate_suffix(gate.state)),
            Style::default().fg(gate_color(gate.state)),
        ));
    }
    lines.push(Line::from(gate_spans));
    lines
}

#[cfg(test)]
fn expanded_result_history_entry_lines(
    entry: &ResultHistoryEntry,
    available_width: usize,
    authority: &str,
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
                format!("{authority} {}", result.clear_type),
                Style::default()
                    .fg(clear_type_color(&result.clear_type))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {} ", play_type_label(result.play_type))),
            Span::styled(
                difficulty_label(result.difficulty),
                Style::default().fg(difficulty_color(result.difficulty)),
            ),
            Span::raw(format!(
                " Lv{}  EX SCORE {} / {} ({}.{:01}%)",
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
            &format!(
                "{}  Options: {}",
                entry.song.as_ref().map_or("-", |song| song.artist.as_str()),
                play_options_text(&result.play_options),
            ),
            available_width,
        )),
        Line::from(format!(
            "attempt=#{} parent=#{}  notes={} side={} mode={} sequence={}",
            result.attempt_id,
            result
                .parent_attempt_id
                .map_or_else(|| "-".to_owned(), |value| format!("{value}")),
            grouped_u32(result.notes),
            play_side_label(result.play_side),
            result.play_mode,
            entry.source_sequence,
        )),
    ]
}

#[cfg(test)]
const fn clear_type_color(clear_type: &str) -> Color {
    match clear_type.as_bytes() {
        b"FAILED" => Color::Red,
        b"ASSIST CLEAR" | b"EASY CLEAR" => Color::Yellow,
        b"CLEAR" | b"HARD CLEAR" | b"EXH-CLEAR" | b"F-COMBO" => Color::Green,
        _ => Color::White,
    }
}

#[cfg(test)]
const fn difficulty_color(difficulty: Difficulty) -> Color {
    match difficulty {
        Difficulty::Beginner => Color::Green,
        Difficulty::Normal => Color::Blue,
        Difficulty::Hyper => Color::Yellow,
        Difficulty::Another => Color::Red,
        Difficulty::Leggendaria => Color::Magenta,
    }
}

#[cfg(test)]
pub(super) const fn play_type_label(play_type: PlayType) -> &'static str {
    match play_type {
        PlayType::Single => "SP",
        PlayType::Double => "DP",
    }
}

#[cfg(test)]
const fn play_side_label(play_side: PlaySide) -> &'static str {
    match play_side {
        PlaySide::OnePlayer => "1P",
        PlaySide::TwoPlayer => "2P",
    }
}

#[cfg(test)]
pub(super) const fn difficulty_label(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Beginner => "BEGINNER",
        Difficulty::Normal => "NORMAL",
        Difficulty::Hyper => "HYPER",
        Difficulty::Another => "ANOTHER",
        Difficulty::Leggendaria => "LEGGENDARIA",
    }
}

#[cfg(test)]
fn grouped_u32(value: u32) -> String {
    grouped_u64(u64::from(value))
}

#[cfg(test)]
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

#[cfg(test)]
fn supplemental_u32(value: &SupplementalResultValue<u32>) -> String {
    match value {
        SupplementalResultValue::Known { value } => grouped_u32(*value),
        SupplementalResultValue::NotDisplayed => "--".to_owned(),
        SupplementalResultValue::Unknown { .. } => "?".to_owned(),
    }
}

#[cfg(test)]
fn play_options_text(value: &PlayOptions) -> String {
    match value {
        PlayOptions::Known { values } if values.is_empty() => "none".to_owned(),
        PlayOptions::Known { values } => values
            .iter()
            .map(|value| match value {
                PlayOption::Random => "RANDOM",
                PlayOption::RRandom => "R-RANDOM",
                PlayOption::SRandom => "S-RANDOM",
                PlayOption::Mirror => "MIRROR",
                PlayOption::AutoScratch => "A-SCR",
                PlayOption::Legacy => "LEGACY",
            })
            .collect::<Vec<_>>()
            .join(","),
        PlayOptions::Unknown { reason } => {
            format!("unknown({})", format!("{reason:?}").to_ascii_lowercase())
        }
    }
}

#[cfg(test)]
fn previous_u32(value: &PreviousBestValue<u32>) -> String {
    match value {
        PreviousBestValue::Known { value } => grouped_u32(*value),
        PreviousBestValue::NotPlayed => "NO PLAY".to_owned(),
        PreviousBestValue::NotDisplayed => "--".to_owned(),
        PreviousBestValue::Unknown { .. } => "?".to_owned(),
    }
}

#[cfg(test)]
fn previous_text(value: &PreviousBestValue<String>) -> String {
    match value {
        PreviousBestValue::Known { value } => value.clone(),
        PreviousBestValue::NotPlayed => "NO PLAY".to_owned(),
        PreviousBestValue::NotDisplayed => "--".to_owned(),
        PreviousBestValue::Unknown { .. } => "?".to_owned(),
    }
}

#[cfg(test)]
pub(super) fn fitted_value(prefix: &str, value: &str, available_width: usize) -> String {
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

#[cfg(test)]
#[path = "server/tests.rs"]
mod tests;
