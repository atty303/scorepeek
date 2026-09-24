//! Runtime-owned event envelope used by diagnostics and public projections.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use scorepeek_core::catalog::{PlayType, ScorepeekSongId};
use scorepeek_core::recognition::music_select::PlaySide;
use scorepeek_core::recognition::result::{
    ParsedResultFields, ResultChartResolution, ResultPerformanceResolution,
};
use scorepeek_core::recognition::screen::{PlayPresenceEvidence, ResultPresenceEvidence};
use scorepeek_core::recognition::shared::{EvidenceFamily, JointEvidenceObservation};
use scorepeek_core::session::{
    MusicSelectTemporalState, MusicSelectTemporalTransitionReason, PlayAttemptState,
    ResultTemporalState, SemanticEpisodePhase, TemporalFieldTransition,
};

use scorepeek_core::event::{
    CurrentSelectionDifficulty, EvidenceContribution, MusicSelectBestSnapshot,
    MusicSelectResolverState, MusicSelectionState, NumericResultEventSuppressionReason,
    NumericResultTemporalState, NumericResultTransitionReason, ResolverHypothesisKey,
    ResolverResolutionState, ResolverScope, ResultPanelSideEpisodeState,
    ResultPanelSideTransitionReason, ResultState, SelectionDifficultyTarget,
    SelectionDifficultyTransitionReason, SongPresentation,
};

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

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct RunEventEnvelope<T> {
    pub schema: String,
    #[serde(flatten)]
    pub kind: T,
}

impl<T> RunEventEnvelope<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    /// Serializes this envelope to its transport-neutral JSON representation.
    ///
    /// # Errors
    /// Returns an error when the event payload cannot be represented as JSON.
    pub fn to_value(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self)
            .map_err(|error| format!("run event serialization failed: {error}"))
    }

    /// Validates and decodes a transport-neutral JSON run event.
    ///
    /// # Errors
    /// Returns an error for malformed payloads or an unsupported event schema.
    pub fn from_value(value: serde_json::Value) -> Result<Self, String> {
        let event: Self = serde_json::from_value(value)
            .map_err(|error| format!("run event contract validation failed: {error}"))?;
        if event.schema != super::schema::RUN_EVENT_SCHEMA {
            return Err("run event schema is unsupported".to_owned());
        }
        Ok(event)
    }
}

pub type RunEvent = RunEventEnvelope<RunEventKind>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "the event schema remains flat and values cross an already bounded queue"
)]
pub enum RunEventKind {
    OverlayObserved {
        observation: Value,
    },
    MusicSelectBestObserved {
        session_id: String,
        snapshot: MusicSelectBestSnapshot,
    },
    MusicSelectResolverChanged {
        session_id: Option<String>,
        state: MusicSelectResolverState,
    },
    WatcherStarted {
        invocation_id: String,
    },
    SessionStarted {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    RecordingHealthChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        state: String,
        memory_limit_bytes: u64,
        memory_used_bytes: u64,
        memory_high_water_bytes: u64,
        dropped_frames: u64,
    },
    RecordingFinalizing {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    RecordingCompleted {
        session_id: String,
        directory: String,
    },
    GameVersionChanged {
        session_id: String,
        source_sequence: u64,
        version: String,
    },
    RawScreenObserved {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        semantic_episode_id: Option<u64>,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_presence: Option<ResultPresenceEvidence>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        play_presence: Option<PlayPresenceEvidence>,
        #[serde(skip_serializing_if = "Option::is_none")]
        unknown_reason: Option<String>,
    },
    SemanticScreenEpisodeChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: String,
        phase: SemanticEpisodePhase,
    },
    ScreenChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default)]
        screen_episode_id: u64,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
    },
    ScreenTick {
        #[serde(default)]
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: String,
    },
    FieldObservation {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default)]
        screen_episode_id: u64,
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
        joint_evidence: JointEvidenceObservation,
        processing_timing: Value,
        song_resolution_presentation: Box<SongResolutionPresentation>,
    },
    ResultChanged {
        session_id: String,
        source_sequence: u64,
        state: ResultState,
    },
    ResultPanelSideChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        state: ResultPanelSideEpisodeState,
        reason: ResultPanelSideTransitionReason,
    },
    ResultSelectContextMismatch {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        select_play_side: PlaySide,
        result_play_side: PlaySide,
    },
    MusicSelectionChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        revision: u64,
        state: MusicSelectionState,
    },
    TemporalResultChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_sequence: Option<u64>,
        transitions: Vec<TemporalFieldTransition>,
        state: ResultTemporalState<ScorepeekSongId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stable_song: Option<SongPresentation>,
    },
    TemporalMusicSelectChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_sequence: Option<u64>,
        reasons: Vec<MusicSelectTemporalTransitionReason>,
        state: MusicSelectTemporalState<ScorepeekSongId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retained_song: Option<SongPresentation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        candidate_song: Option<SongPresentation>,
    },
    NumericResultChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
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
        source_sequence: Option<u64>,
        state: PlayAttemptState,
    },
    ResolverStateChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        scope: ResolverScope,
        state: ResolverResolutionState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        select_play_type: Option<PlayType>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_play_type: Option<PlayType>,
        #[serde(default)]
        play_type_mismatch: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        top: Option<ResolverHypothesisKey>,
        #[serde(skip_serializing_if = "Option::is_none")]
        runner_up: Option<ResolverHypothesisKey>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runner_song: Option<ResolverHypothesisKey>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runner_chart: Option<ResolverHypothesisKey>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        top_candidates: Vec<ResolverHypothesisKey>,
        support: u16,
        margin: u16,
        #[serde(default)]
        song_margin: u16,
        #[serde(default)]
        chart_margin: u16,
        selected_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
        runner_up_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
        observation_count: u32,
    },
    SelectionDifficultyChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        target: SelectionDifficultyTarget,
        reason: SelectionDifficultyTransitionReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        current: Option<CurrentSelectionDifficulty>,
    },
    SessionFinished {
        session_id: String,
        outcome: String,
        report: Value,
    },
    WatcherStopped {
        invocation_id: String,
        reason: String,
    },
}
