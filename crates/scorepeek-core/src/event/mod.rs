//! Stable domain and run event authority.

pub mod coordinator;
pub mod domain;
pub mod input;
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
pub use input::DomainInput;
pub use projection::{
    ProjectionCursor, diagnostic_run_event_value, run_event_from_field_observation,
};
pub use reducer::{
    AttemptNodeSnapshot, GateSnapshot, GateState, PlayOptionsDebugSnapshot, ReducedRunEvents,
    ResolverNodeSnapshot, RunReducerEffect, RunReducerSnapshot,
};
pub use run::{RunEvent, RunEventEnvelope, RunEventKind};
pub use schema::RUN_EVENT_SCHEMA;
