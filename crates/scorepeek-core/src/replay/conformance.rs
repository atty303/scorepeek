//! Portable recognition, model, frame, and catalog contracts used by replay conformance.

pub use crate::catalog::{CatalogStore, Difficulty, PlayType, ScorepeekSongId};
pub use crate::frame::{CanonicalLayout, Roi};
pub use crate::game_version::GameVersionState;
pub use crate::model::{
    numeric::{NUMERIC_DICTIONARY, NumericField},
    registry::{
        LIVE_MODEL_BUNDLE_MANIFEST_SHA256, LIVE_MODEL_SHA256, LIVE_RUNTIME_SHA256,
        NUMERIC_MODEL_MANIFEST_SHA256,
    },
    text::DynamicTextObservation,
};
pub use crate::recognition::{
    CatalogCandidateDomain, MusicSelectBestObservation, MusicSelectDifficultyMarkerEvidence,
    MusicSelectDifficultyObservation, MusicSelectDifficultyState,
    MusicSelectDifficultyUnknownReason, MusicSelectMotionRegions, MusicSelectPlaySideObservation,
    MusicSelectPlayTypeObservation, MusicSelectScreenFieldObservations, PlayOption, PlayOptions,
    PlayOptionsUnknownReason, PlayPresenceEvidence, PlaySide, PreviousBest, PreviousBestValue,
    RecognitionError, ResultChartResolution, ResultFieldUnknownReason, ResultJudgments,
    ResultPanelPresenceEvidence, ResultPanelSide, ResultPanelSideState,
    ResultPanelSideUnknownReason, ResultPerformanceResolution, ResultPresenceEvidence,
    ResultScreenRgb8Crops, ResultSongResolution, ResultTiming, Rgb8Crop,
    ScreenCatalogCandidateObservations, ScreenClass, ScreenCropRoute, ScreenFieldObservations,
    ScreenRgb8Crops, SupplementalResultValue, inspect_canonical_rgb8, resolve_clear_type,
    resolve_music_select_song, route_screen_rgb8_crops,
};
