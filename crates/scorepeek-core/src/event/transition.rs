//! Ordered decisions made by the domain coordinator.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::catalog::{PlayType, ScorepeekSongId};
use crate::recognition::music_select::PlaySide;
use crate::session::{
    MusicSelectTemporalState, MusicSelectTemporalTransitionReason, PlayAttemptState,
    ResultTemporalState, TemporalFieldTransition,
};

use super::{
    CurrentSelectionDifficulty, EvidenceContribution, MusicSelectBestSnapshot,
    MusicSelectResolverState, MusicSelectionState, NumericResultEventSuppressionReason,
    NumericResultTemporalState, NumericResultTransitionReason, ResolverHypothesisKey,
    ResolverResolutionState, ResolverScope, ResultPanelSideEpisodeState,
    ResultPanelSideTransitionReason, ResultState, SelectionDifficultyTarget,
    SelectionDifficultyTransitionReason, SongPresentation,
};
use crate::recognition::shared::EvidenceFamily;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "ordered domain outputs keep owned typed values"
)]
pub enum DomainTransitionKind {
    MusicSelectBestObserved {
        session_id: String,
        snapshot: MusicSelectBestSnapshot,
    },
    MusicSelectResolverChanged {
        session_id: Option<String>,
        state: MusicSelectResolverState,
    },
    ResultChanged {
        session_id: String,
        source_sequence: u64,
        state: ResultState,
    },
    ResultPanelSideChanged {
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        state: ResultPanelSideEpisodeState,
        reason: ResultPanelSideTransitionReason,
    },
    ResultSelectContextMismatch {
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        select_play_side: PlaySide,
        result_play_side: PlaySide,
    },
    MusicSelectionChanged {
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        revision: u64,
        state: MusicSelectionState,
    },
    TemporalResultChanged {
        session_id: Option<String>,
        source_sequence: Option<u64>,
        transitions: Vec<TemporalFieldTransition>,
        state: ResultTemporalState<ScorepeekSongId>,
        stable_song: Option<SongPresentation>,
    },
    TemporalMusicSelectChanged {
        session_id: Option<String>,
        source_sequence: Option<u64>,
        reasons: Vec<MusicSelectTemporalTransitionReason>,
        state: MusicSelectTemporalState<ScorepeekSongId>,
        retained_song: Option<SongPresentation>,
        candidate_song: Option<SongPresentation>,
    },
    NumericResultChanged {
        session_id: Option<String>,
        source_sequence: u64,
        state: NumericResultTemporalState,
        reason: NumericResultTransitionReason,
        event_suppression_reason: Option<NumericResultEventSuppressionReason>,
    },
    PlayAttemptChanged {
        session_id: Option<String>,
        source_sequence: Option<u64>,
        state: PlayAttemptState,
    },
    ResolverStateChanged {
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        scope: ResolverScope,
        state: ResolverResolutionState,
        select_play_type: Option<PlayType>,
        result_play_type: Option<PlayType>,
        play_type_mismatch: bool,
        top: Option<ResolverHypothesisKey>,
        runner_up: Option<ResolverHypothesisKey>,
        runner_song: Option<ResolverHypothesisKey>,
        runner_chart: Option<ResolverHypothesisKey>,
        top_candidates: Vec<ResolverHypothesisKey>,
        support: u16,
        margin: u16,
        song_margin: u16,
        chart_margin: u16,
        selected_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
        runner_up_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
        observation_count: u32,
    },
    SelectionDifficultyChanged {
        session_id: Option<String>,
        screen_episode_id: u64,
        source_sequence: u64,
        target: SelectionDifficultyTarget,
        reason: SelectionDifficultyTransitionReason,
        current: Option<CurrentSelectionDifficulty>,
    },
}
