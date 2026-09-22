//! Stable domain and run event authority.

pub mod domain;
pub mod projection;
mod reducer;
pub mod run;
pub mod schema;

pub use domain::{
    BestChart, BestOutputState, CurrentSelectionDifficulty, EvidenceContribution,
    MusicSelectBestSnapshot, MusicSelectResolverState, MusicSelectionState,
    MusicSelectionUnresolvedReason, NumericResultEventSuppressionReason,
    NumericResultTemporalState, NumericResultTransitionReason, ResolverHypothesisKey,
    ResolverResolutionState, ResolverScope, ResultDomainEvent, ResultPanelSideEpisodeState,
    ResultPanelSideTransitionReason, ResultRetractionReason, ResultState, SelectFrameIdentity,
    SelectIdentityStatus, SelectionDifficultyTarget, SelectionDifficultyTransitionReason,
    SongPresentation, SongResolutionPresentation,
};
pub use projection::{
    ProjectionCursor, diagnostic_run_event_value, run_event_from_field_observation,
};
pub use reducer::{
    AttemptNodeSnapshot, GateSnapshot, GateState, PlayOptionsDebugSnapshot, ReducedRunEvents,
    ResolverNodeSnapshot, RunEventReducer, RunEventReductionError, RunReducerEffect,
    RunReducerSnapshot,
};
#[cfg(feature = "reducer-test-support")]
#[doc(hidden)]
pub use reducer::{
    EVIDENCE_FAMILY_CAP, HypothesisAccumulator, JointKey, MusicSelectResolver,
    PlayOptionsEpisodeAccumulator, ResultChartFactor, ResultPanelSideAccumulator,
    SelectionEpochTracker, candidate_song_presentation, selected_difficulty, selected_play_type,
    test_result_play_side, test_selected_play_side,
};
pub use run::{RunEvent, RunEventEnvelope, RunEventKind};
pub use schema::RUN_EVENT_SCHEMA;
