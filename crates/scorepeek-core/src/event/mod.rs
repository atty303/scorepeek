//! Stable domain and run event authority.

pub mod domain;
pub mod projection;
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
pub use run::{RunEvent, RunEventEnvelope, RunEventKind};
pub use schema::RUN_EVENT_SCHEMA;
