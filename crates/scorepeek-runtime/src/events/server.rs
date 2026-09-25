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
use super::snapshot as event_api;
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

#[cfg(test)]
use super::SongResolutionPresentation;
use super::{RUN_EVENT_SCHEMA, RunEvent, RunEventKind, diagnostic_run_event_value};
use crate::diagnostics::inspect::{DiagnosticSink, RunDiagnostics};
#[cfg(test)]
use scorepeek_core::catalog::{Difficulty, PlayType};
use scorepeek_core::event::DomainEffect;
use scorepeek_core::event::coordinator::{CoordinatorPolicy, DomainCoordinator};
use scorepeek_core::event::{
    DomainInput, MusicSelectResolverState, MusicSelectionState, ResultDomainEvent, ResultState,
    SongPresentation,
};
#[cfg(test)]
use scorepeek_core::event::{
    MusicSelectionUnresolvedReason, ResolverResolutionState, ResultRetractionReason,
};
#[cfg(test)]
use scorepeek_core::recognition::music_select::PlaySide;
#[cfg(test)]
use scorepeek_core::recognition::result::{
    ParsedResultFields, PlayOptions, PlayOptionsObservation, ResultPerformanceResolution,
};
#[cfg(test)]
use scorepeek_core::recognition::result::{
    PlayOption, PreviousBest, PreviousBestValue, ResultChartResolution, ResultJudgments,
    ResultTiming, SupplementalResultValue,
};
#[cfg(test)]
use scorepeek_core::recognition::screen::ResultPanelSide;
#[cfg(test)]
use scorepeek_core::recognition::shared::{
    EvidenceFamily, JointEvidenceCandidate, JointEvidenceObservation,
};
#[cfg(test)]
use scorepeek_core::session::attempt::{PlayAttemptReason, PlayAttemptState};
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
    source_sequence: u64,
    song: Option<SongPresentation>,
    result: ResultDomainEvent,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunViewState {
    #[serde(skip)]
    pub(super) public: event_api::PublicState,
    invocation_id: String,
    recording: &'static str,
    watcher_state: String,
    scores_summary: Option<String>,
    channel_start_failure: Option<String>,
    session_count: u64,
    active_session_id: Option<String>,
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
    resolver: Value,
}

impl RunViewState {
    fn new(invocation_id: String, recording_enabled: bool) -> Self {
        let mut public = event_api::PublicState::new(invocation_id.clone());
        if recording_enabled {
            public.enable_recording();
        }
        Self {
            public,
            invocation_id,
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
            resolver: super::debug_projection::snapshot(
                &scorepeek_core::event::DomainSnapshot::default(),
            ),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn reduce(&mut self, event: &RunEvent, serialized: &Value) {
        match &event.kind {
            RunEventKind::MusicSelectResolverChanged { state, .. } => {
                self.music_select = state.clone();
            }
            RunEventKind::WatcherStarted { .. } => "starting".clone_into(&mut self.watcher_state),
            RunEventKind::SessionStarted { session_id, .. } => {
                "session_active".clone_into(&mut self.watcher_state);
                self.session_count = self.session_count.saturating_add(1);
                self.active_session_id.clone_from(session_id);
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
            // Standalone canonical replay is not dispatched through the live UI projection.
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
    next_core_input_sequence: u64,
    active_core_input_sequence: Option<u64>,
    timing_active: bool,
    output_us: u64,
    #[cfg(test)]
    headless_events: Vec<RunEvent>,
    core_reducer: DomainCoordinator,
    trace_counters: TraceCounters,
    diagnostics: Option<RunDiagnostics>,
}

#[derive(Default)]
struct TraceCounters {
    inputs: u64,
    no_op_inputs: u64,
    domain_transitions: u64,
    output_events: u64,
    core_errors: u64,
    admitted_frames: u64,
    completed_field_observations: u64,
    source_sequence_gaps: u64,
    last_source_sequence: Option<u64>,
    screen_counts: [u64; 7],
    output_kinds: BTreeMap<&'static str, u64>,
    field_total_timing: TimingAccumulator,
    field_end_to_end_timing: TimingAccumulator,
    field_queue_wait_timing: TimingAccumulator,
}

#[derive(Default)]
struct TimingAccumulator {
    count: u64,
    total_us: u64,
    max_us: u64,
}

impl TimingAccumulator {
    fn observe(&mut self, value: &Value) {
        if let Some(micros) = value.as_u64() {
            self.count = self.count.saturating_add(1);
            self.total_us = self.total_us.saturating_add(micros);
            self.max_us = self.max_us.max(micros);
        }
    }
}

impl TraceCounters {
    fn observe_input(&mut self, kind: &RunEventKind) {
        match kind {
            RunEventKind::SessionStarted { .. } => self.last_source_sequence = None,
            RunEventKind::RawScreenObserved {
                sequence, screen, ..
            } => {
                self.admitted_frames = self.admitted_frames.saturating_add(1);
                self.screen_counts[screen_bucket(screen)] =
                    self.screen_counts[screen_bucket(screen)].saturating_add(1);
                if let Some(previous) = self.last_source_sequence {
                    self.source_sequence_gaps = self
                        .source_sequence_gaps
                        .saturating_add(sequence.saturating_sub(previous.saturating_add(1)));
                }
                self.last_source_sequence = Some(*sequence);
            }
            RunEventKind::FieldObservation {
                processing_timing, ..
            } => {
                self.completed_field_observations =
                    self.completed_field_observations.saturating_add(1);
                self.field_total_timing
                    .observe(&processing_timing["frame_total_us"]);
                self.field_end_to_end_timing
                    .observe(&processing_timing["frame_end_to_end_wall_us"]);
                self.field_queue_wait_timing
                    .observe(&processing_timing["field_queue_wait_us"]);
            }
            _ => {}
        }
    }
}

fn recording_publication(report: &Value, enabled: bool) -> &'static str {
    if !enabled {
        "disabled"
    } else if report["canonical_recording_manifest_published"] == true
        && report["canonical_recording_completeness"] == "complete"
    {
        "published"
    } else if report["canonical_recording_completeness"] == "partial" {
        "partial"
    } else {
        "failed"
    }
}

fn annotate_session_start(
    object: &mut serde_json::Map<String, Value>,
    event: &RunEvent,
    recording_enabled: bool,
) {
    if let RunEventKind::SessionStarted { .. } = &event.kind {
        object.insert(
            "canonical_frame_contract".to_owned(),
            scorepeek_core::frame::CANONICAL_FRAME_CONTRACT_ID.into(),
        );
        object.insert(
            "canonical_recording_schema".to_owned(),
            scorepeek_core::canonical_recording::RECORDING_SCHEMA.into(),
        );
        object.insert(
            "runtime_config".to_owned(),
            json!({
                "recording_enabled": recording_enabled,
            }),
        );
    }
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
            "runtime_event",
            None,
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
        self.emit_diagnostic_warnings();
        self.record_trace_summary(self.next_core_input_sequence.saturating_sub(1), None);
        if let Ok(state) = self.state.lock() {
            self.record_diagnostic("runtime_run_summary", &json!({
                "session_count": state.session_count,
                "result_count": state.result_count,
                "recording_status": state.status_recording,
                "recording_memory_high_water_bytes": state.recording_memory_high_water_bytes,
                "recording_dropped_frames": state.recording_dropped_frames,
                "diagnostic_health": self.diagnostics.as_ref().map(|diagnostics| diagnostics.sink().health()),
            }), true);
        }
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.finish(operation_status);
        }
        self.emit_diagnostic_warnings();
    }

    fn emit_diagnostic_warnings(&self) {
        if let Some(diagnostics) = &self.diagnostics {
            for warning in diagnostics.take_warnings() {
                let _ = crate::service::dispatch::frontend_event(
                    scorepeek_frontend_api::FrontendEvent::Warning { warning },
                );
            }
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
        recording_enabled: bool,
        diagnostics: RunDiagnostics,
    ) -> Result<Self, RoutineOutputStartError> {
        let state = Arc::new(Mutex::new(RunViewState::new(
            invocation_id,
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
            next_core_input_sequence: 1,
            active_core_input_sequence: None,
            timing_active: false,
            output_us: 0,
            #[cfg(test)]
            headless_events: Vec::new(),
            core_reducer: DomainCoordinator::new(CoordinatorPolicy::default())
                .expect("fixed core coordinator policy"),
            trace_counters: TraceCounters::default(),
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
    pub fn start_headless(invocation_id: String, _profile_sha256: String) -> Self {
        Self::start_headless_inner(invocation_id, None)
    }

    #[must_use]
    #[cfg(test)]
    pub fn start_headless_with_diagnostics(
        invocation_id: String,
        _profile_sha256: String,
        diagnostics: RunDiagnostics,
    ) -> Self {
        Self::start_headless_inner(invocation_id, Some(diagnostics))
    }

    #[cfg(test)]
    fn start_headless_inner(invocation_id: String, diagnostics: Option<RunDiagnostics>) -> Self {
        Self {
            state: Arc::new(Mutex::new(RunViewState::new(invocation_id, false))),
            channel: None,
            scores: None,
            publish_frontend_snapshots: false,
            next_sequence: 1,
            next_core_input_sequence: 1,
            active_core_input_sequence: None,
            timing_active: false,
            output_us: 0,
            #[cfg(test)]
            headless_events: Vec::new(),
            core_reducer: DomainCoordinator::new(CoordinatorPolicy::default())
                .expect("fixed core coordinator policy"),
            trace_counters: TraceCounters::default(),
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
        self.refresh()?;
        if let Some(health) = self.scores_health()
            && let Some(failure) = health.failure
        {
            let cause = health.cause.unwrap_or_default();
            self.record_diagnostic(
                "scores_failure",
                &json!({
                    "error_type": failure,
                    "cause": cause,
                    "failed": health.failed,
                    "rejected": health.rejected,
                    "pending": health.pending,
                    "committed": health.committed,
                }),
                true,
            );
            return Err(format!(
                "score saving failed ({failure}): {cause}; failed={}, rejected={}, pending={}, committed={}",
                health.failed, health.rejected, health.pending, health.committed
            ));
        }
        Ok(())
    }

    pub fn enable_scores(&mut self, path: &Path) -> Result<(), String> {
        self.scores = Some(crate::scores::Worker::start(path)?);
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

    pub fn publish(&mut self, event: &RunEvent) -> Result<(), String> {
        self.publish_timed(event).map(|_| ())
    }

    pub fn publish_timed(
        &mut self,
        event: &RunEvent,
    ) -> Result<RoutineEventProcessingTiming, String> {
        self.publish_timed_core(event, None)
    }

    pub fn publish_timed_domain(
        &mut self,
        event: &RunEvent,
        input: DomainInput,
    ) -> Result<RoutineEventProcessingTiming, String> {
        self.publish_timed_core(event, Some(input))
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the ordered core step and runtime projection share one timing scope"
    )]
    fn publish_timed_core(
        &mut self,
        event: &RunEvent,
        typed_input: Option<DomainInput>,
    ) -> Result<RoutineEventProcessingTiming, String> {
        let input = match typed_input {
            Some(input) => Some(input),
            None => super::input::domain_input(event)?,
        };
        let Some(input) = input else {
            self.publish_one(event)?;
            return Ok(RoutineEventProcessingTiming::default());
        };
        self.timing_active = true;
        self.output_us = 0;
        let started = Instant::now();
        let input_sequence = self.next_core_input_sequence;
        let reduced = match self.core_reducer.step(input_sequence, &input) {
            Ok(reduced) => reduced,
            Err(error) => return Err(self.record_core_error(input_sequence, error)),
        };
        self.next_core_input_sequence = input_sequence.saturating_add(1);
        self.active_core_input_sequence = Some(input_sequence);
        let transition_count = reduced
            .effects()
            .iter()
            .filter(|effect| matches!(effect, DomainEffect::Event(_)))
            .count() as u64;
        let output_count = reduced
            .effects()
            .iter()
            .filter(|effect| matches!(effect, DomainEffect::Event(_)))
            .count() as u64;
        self.trace_counters.inputs = self.trace_counters.inputs.saturating_add(1);
        self.trace_counters.domain_transitions = self
            .trace_counters
            .domain_transitions
            .saturating_add(transition_count);
        self.trace_counters.output_events = self
            .trace_counters
            .output_events
            .saturating_add(output_count);
        for effect in reduced.effects() {
            if let DomainEffect::Event(output) = effect {
                let count = self
                    .trace_counters
                    .output_kinds
                    .entry(output_kind(
                        &super::domain_projection::run_event(output).kind,
                    ))
                    .or_default();
                *count = count.saturating_add(1);
            }
        }
        self.trace_counters.observe_input(&event.kind);
        if transition_count == 0 {
            self.trace_counters.no_op_inputs = self.trace_counters.no_op_inputs.saturating_add(1);
        }
        if trace_ephemeral_input(&event.kind) {
            self.consume_omitted_input()?;
        } else if !matches!(
            input,
            DomainInput::SessionFinished { .. } | DomainInput::WatcherFinished
        ) {
            self.publish_one(event)?;
        }
        self.publish_domain_outputs(reduced.into_effects())?;
        if matches!(input, DomainInput::WatcherFinished) {
            if let Some(scores) = &mut self.scores {
                scores.finish();
            }
            self.refresh_scores()?;
        }
        if matches!(
            input,
            DomainInput::SessionFinished { .. } | DomainInput::WatcherFinished
        ) {
            self.publish_one(event)?;
        }
        self.refresh()?;
        self.active_core_input_sequence = None;
        if self.trace_counters.inputs.is_multiple_of(256)
            || matches!(
                event.kind,
                RunEventKind::SessionFinished { .. } | RunEventKind::WatcherStopped { .. }
            )
        {
            let report = match &event.kind {
                RunEventKind::SessionFinished { report, .. } => Some(report),
                _ => None,
            };
            self.record_trace_summary(input_sequence, report);
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

    fn publish_domain_outputs(&mut self, effects: Vec<DomainEffect>) -> Result<(), String> {
        for effect in effects {
            match effect {
                DomainEffect::Event(output) => {
                    let event = super::domain_projection::run_event(&output);
                    self.publish_domain_transition(&output, &event)?;
                }
                DomainEffect::SessionEnded => {}
                DomainEffect::Snapshot(snapshot) => {
                    self.state
                        .lock()
                        .map_err(|_| "run view state lock was poisoned".to_owned())?
                        .resolver = super::debug_projection::snapshot(&snapshot);
                }
            }
        }
        Ok(())
    }

    fn record_core_error(
        &mut self,
        input_sequence: u64,
        error: scorepeek_core::event::coordinator::CoordinatorError,
    ) -> String {
        self.trace_counters.core_errors = self.trace_counters.core_errors.saturating_add(1);
        self.record_diagnostic(
            "core_error",
            &json!({"input_sequence": input_sequence, "error_type": format!("{error:?}")}),
            true,
        );
        self.record_trace_summary(input_sequence, None);
        self.timing_active = false;
        format!("core domain transition failed: {error:?}")
    }

    fn record_trace_summary(&self, input_sequence: u64, terminal_report: Option<&Value>) {
        let snapshot = self.core_reducer.state().snapshot();
        self.record_diagnostic(
            "domain_summary",
            &json!({
                "input_sequence": input_sequence,
                "inputs": self.trace_counters.inputs,
                "no_op_inputs": self.trace_counters.no_op_inputs,
                "domain_transitions": self.trace_counters.domain_transitions,
                "output_events": self.trace_counters.output_events,
                "core_errors": self.trace_counters.core_errors,
                "admitted_frames": self.trace_counters.admitted_frames,
                "completed_field_observations": self.trace_counters.completed_field_observations,
                "recognition_field_timing_us": {
                    "frame_total": {
                        "count": self.trace_counters.field_total_timing.count,
                        "sum": self.trace_counters.field_total_timing.total_us,
                        "max": self.trace_counters.field_total_timing.max_us,
                    },
                    "frame_end_to_end": {
                        "count": self.trace_counters.field_end_to_end_timing.count,
                        "sum": self.trace_counters.field_end_to_end_timing.total_us,
                        "max": self.trace_counters.field_end_to_end_timing.max_us,
                    },
                    "field_queue_wait": {
                        "count": self.trace_counters.field_queue_wait_timing.count,
                        "sum": self.trace_counters.field_queue_wait_timing.total_us,
                        "max": self.trace_counters.field_queue_wait_timing.max_us,
                    },
                },
                "source_sequence_gaps": self.trace_counters.source_sequence_gaps,
                "screen_counts": {
                    "title": self.trace_counters.screen_counts[0],
                    "result": self.trace_counters.screen_counts[1],
                    "music_select": self.trace_counters.screen_counts[2],
                    "mode_select": self.trace_counters.screen_counts[3],
                    "decide_transition": self.trace_counters.screen_counts[4],
                    "play": self.trace_counters.screen_counts[5],
                    "unknown": self.trace_counters.screen_counts[6],
                },
                "output_kinds": self.trace_counters.output_kinds,
                "runtime_scheduling": terminal_report.map(|report| json!({
                    "recognition_ticks": report["recognition_ticks"],
                    "recognition_busy_skips": report["recognition_busy_skips"],
                    "field_observation_busy_skips": report["field_observation_busy_skips"],
                    "field_submitted": report["field_submitted"],
                    "field_rejected": report["field_rejected"],
                    "field_ready_failure": report["field_ready_failure"],
                    "field_worker_abandoned": report["field_worker_abandoned"],
                    "diagnostic_fact_queue_full": report["diagnostic_fact_queue_full"],
                    "dropped_capture_diagnostic_facts": report["dropped_capture_diagnostic_facts"],
                })),
                "final_state": {
                    "screen": snapshot.screen,
                    "screen_episode_id": snapshot.screen_episode_id,
                    "gate": super::debug_projection::snapshot(&snapshot)["gate"],
                    "attempt": super::debug_projection::snapshot(&snapshot)["attempt"],
                },
            }),
            true,
        );
    }

    #[cfg(test)]
    #[allow(
        clippy::too_many_arguments,
        reason = "the test adapter mirrors one complete music-select observation contract"
    )]
    fn reduce_music_select_observation(
        &mut self,
        session_id: Option<&String>,
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
                screen_episode_id: self.core_reducer.state().snapshot().screen_episode_id,
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
        self.publish_one_inner(event, true, "runtime_event", None)
    }

    fn publish_domain_transition(
        &mut self,
        transition: &scorepeek_core::event::DomainTransitionKind,
        event: &RunEvent,
    ) -> Result<(), String> {
        let mut value = serde_json::to_value(transition)
            .map_err(|error| format!("domain transition serialization failed: {error}"))?;
        value["schema"] = super::schema::DOMAIN_TRANSITION_SCHEMA.into();
        self.publish_one_inner(event, true, "domain_transition", Some(value))
    }

    fn consume_omitted_input(&mut self) -> Result<(), String> {
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .next_channel_sequence = self.next_sequence;
        Ok(())
    }

    fn publish_one_inner(
        &mut self,
        event: &RunEvent,
        refresh: bool,
        record_kind: &'static str,
        diagnostic_value: Option<Value>,
    ) -> Result<(), String> {
        let output_started = Instant::now();
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let mut value = match diagnostic_value {
            Some(value) => value,
            None => diagnostic_run_event_value(event)?,
        };
        let recording_enabled = self
            .state
            .lock()
            .is_ok_and(|state| state.recording == "enabled");
        if let Some(object) = value.as_object_mut() {
            object.insert("channel_sequence".to_owned(), sequence.into());
            if let Some(input_sequence) = self.active_core_input_sequence {
                object.insert("input_sequence".to_owned(), input_sequence.into());
            }
            annotate_session_start(object, event, recording_enabled);
            if recording_enabled
                && let RunEventKind::SessionStarted {
                    session_id: Some(session_id),
                    ..
                }
                | RunEventKind::SessionFinished { session_id, .. }
                | RunEventKind::RecordingCompleted { session_id, .. } = &event.kind
            {
                object.insert(
                    "recording_locator".to_owned(),
                    format!("sessions/{session_id}/canonical").into(),
                );
            }
            if let RunEventKind::SessionFinished { report, .. } = &event.kind {
                let snapshot = self.core_reducer.state().snapshot();
                object.insert(
                    "final_domain_state".to_owned(),
                    json!({
                        "screen": snapshot.screen,
                        "screen_episode_id": snapshot.screen_episode_id,
                        "gate": super::debug_projection::snapshot(&snapshot)["gate"],
                        "attempt": super::debug_projection::snapshot(&snapshot)["attempt"],
                    }),
                );
                let publication = recording_publication(report, recording_enabled);
                object.insert("recording_publication".to_owned(), publication.into());
            } else if matches!(event.kind, RunEventKind::RecordingCompleted { .. }) {
                object.insert("recording_publication".to_owned(), "published".into());
            }
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
            if !trace_ephemeral_input(&event.kind) {
                sink.record(record_kind, &value, important_run_event(event));
            }
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
                "scores={} failed={} rejected={} pending={} committed={} db={path}",
                if health.failure.is_some() {
                    "degraded"
                } else if health.flush.is_some() {
                    "saved"
                } else {
                    "active"
                },
                health.failed,
                health.rejected,
                health.pending,
                health.committed
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
                raw_screen: state.raw_screen.clone(),
                semantic_screen: state.current_screen.clone(),
                recording_status: state.status_recording.to_owned(),
                recording_memory_used_bytes: state.recording_memory_used_bytes,
                recording_memory_limit_bytes: state.recording_memory_limit_bytes,
                recording_memory_high_water_bytes: state.recording_memory_high_water_bytes,
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
        self.emit_diagnostic_warnings();
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

fn trace_ephemeral_input(kind: &RunEventKind) -> bool {
    matches!(
        kind,
        RunEventKind::RawScreenObserved { .. }
            | RunEventKind::ScreenTick { .. }
            | RunEventKind::FieldObservation { .. }
    )
}

fn screen_bucket(screen: &str) -> usize {
    match screen {
        "title" => 0,
        "result" => 1,
        "music_select" => 2,
        "mode_select" => 3,
        "decide_transition" => 4,
        "play" => 5,
        _ => 6,
    }
}

const fn output_kind(kind: &RunEventKind) -> &'static str {
    match kind {
        RunEventKind::OverlayObserved { .. } => "overlay_observed",
        RunEventKind::MusicSelectBestObserved { .. } => "music_select_best_observed",
        RunEventKind::MusicSelectResolverChanged { .. } => "music_select_resolver_changed",
        RunEventKind::WatcherStarted { .. } => "watcher_started",
        RunEventKind::SessionStarted { .. } => "session_started",
        RunEventKind::RecordingHealthChanged { .. } => "recording_health_changed",
        RunEventKind::RecordingFinalizing { .. } => "recording_finalizing",
        RunEventKind::RecordingCompleted { .. } => "recording_completed",
        RunEventKind::GameVersionChanged { .. } => "game_version_changed",
        RunEventKind::RawScreenObserved { .. } => "raw_screen_observed",
        RunEventKind::SemanticScreenEpisodeChanged { .. } => "semantic_screen_episode_changed",
        RunEventKind::ScreenChanged { .. } => "screen_changed",
        RunEventKind::ScreenTick { .. } => "screen_tick",
        RunEventKind::FieldObservation { .. } => "field_observation",
        RunEventKind::ResultChanged { .. } => "result_changed",
        RunEventKind::ResultPanelSideChanged { .. } => "result_panel_side_changed",
        RunEventKind::ResultSelectContextMismatch { .. } => "result_select_context_mismatch",
        RunEventKind::MusicSelectionChanged { .. } => "music_selection_changed",
        RunEventKind::TemporalResultChanged { .. } => "temporal_result_changed",
        RunEventKind::TemporalMusicSelectChanged { .. } => "temporal_music_select_changed",
        RunEventKind::NumericResultChanged { .. } => "numeric_result_changed",
        RunEventKind::PlayAttemptChanged { .. } => "play_attempt_changed",
        RunEventKind::ResolverStateChanged { .. } => "resolver_state_changed",
        RunEventKind::SelectionDifficultyChanged { .. } => "selection_difficulty_changed",
        RunEventKind::SessionFinished { .. } => "session_finished",
        RunEventKind::WatcherStopped { .. } => "watcher_stopped",
    }
}

impl Drop for RoutineOutput {
    fn drop(&mut self) {
        if let Some(scores) = &mut self.scores {
            scores.finish();
            let _ = self.refresh();
        }
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.finish("error");
        }
        self.emit_diagnostic_warnings();
    }
}

#[cfg(test)]
fn plain_status_line(state: &RunViewState, health: &ChannelHealth) -> String {
    let channel = health.value();
    format!(
        "scorepeek: state={} sessions={} session={} channel={} clients={} dropped={} disconnected={} message={} {}{}",
        state.watcher_state,
        state.session_count,
        state.active_session_id.as_deref().unwrap_or("-"),
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
#[path = "server/tests.rs"]
mod tests;
