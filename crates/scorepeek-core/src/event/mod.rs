//! Stable domain input, transition, and state authority.

pub mod coordinator;
pub mod domain;
pub mod input;
mod reducer;
pub mod transition;

pub use domain::{
    BestChart, BestOutputState, CurrentSelectionDifficulty, EvidenceContribution,
    MusicSelectBestSnapshot, MusicSelectResolverState, MusicSelectionState,
    MusicSelectionUnresolvedReason, NumericResultEventSuppressionReason,
    NumericResultTemporalState, NumericResultTransitionReason, ResolverHypothesisKey,
    ResolverResolutionState, ResolverScope, ResultDomainEvent, ResultPanelSideEpisodeState,
    ResultPanelSideTransitionReason, ResultRetractionReason, ResultState, SelectFrameIdentity,
    SelectIdentityStatus, SelectionDifficultyTarget, SelectionDifficultyTransitionReason,
    SongPresentation,
};
pub use input::{DomainFieldObservation, DomainInput};
pub use reducer::{
    AttemptNodeSnapshot, DomainEffect, DomainSnapshot, GateDecision, GateKind, GateSnapshot,
    GateState, PlayOptionsDebugSnapshot, ReducedDomainTransitions, ResolverNodeSnapshot,
};
pub use transition::DomainTransitionKind;
