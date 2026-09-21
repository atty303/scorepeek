//! Deterministic replay-facing product logic.

pub mod conformance;
pub mod session;
pub mod trace;

pub use conformance::{
    CanonicalLayout, CatalogCandidateDomain, CatalogStore, Difficulty, DynamicTextObservation,
    GameVersionState, LIVE_MODEL_BUNDLE_MANIFEST_SHA256, LIVE_MODEL_SHA256, LIVE_RUNTIME_SHA256,
    MusicSelectBestObservation, MusicSelectDifficultyMarkerEvidence,
    MusicSelectDifficultyObservation, MusicSelectDifficultyState,
    MusicSelectDifficultyUnknownReason, MusicSelectMotionRegions, MusicSelectPlaySideObservation,
    MusicSelectPlayTypeObservation, MusicSelectScreenFieldObservations, NUMERIC_DICTIONARY,
    NUMERIC_MODEL_MANIFEST_SHA256, NumericField, PlayOption, PlayOptions, PlayOptionsUnknownReason,
    PlayPresenceEvidence, PlaySide, PlayType, PreviousBest, PreviousBestValue, RecognitionError,
    ResultChartResolution, ResultFieldUnknownReason, ResultJudgments, ResultPanelPresenceEvidence,
    ResultPanelSide, ResultPanelSideState, ResultPanelSideUnknownReason,
    ResultPerformanceResolution, ResultPresenceEvidence, ResultScreenRgb8Crops,
    ResultSongResolution, ResultTiming, Rgb8Crop, Roi, ScorepeekSongId,
    ScreenCatalogCandidateObservations, ScreenClass, ScreenCropRoute, ScreenFieldObservations,
    ScreenRgb8Crops, SupplementalResultValue, inspect_canonical_rgb8, resolve_clear_type,
    resolve_music_select_song, route_screen_rgb8_crops,
};
pub use session::{
    MusicSelectTemporalPolicy, MusicSelectTemporalReducer, MusicSelectTemporalState,
    MusicSelectTemporalTransitionReason, MusicSelectTemporalUpdate, ResultTemporalReducer,
    SemanticEpisodePhase, SemanticScreenEpisode, TemporalFieldState, TemporalPolicy,
    TemporalTransitionReason, TimelineAction, TimelineDriver,
};
pub use trace::{RUN_EVENT_SCHEMA, RunEvent, RunEventEnvelope, RunEventKind};
