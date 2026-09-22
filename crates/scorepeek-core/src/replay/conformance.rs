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
pub use crate::recognition::music_select::{
    MusicSelectBestObservation, MusicSelectDifficultyMarkerEvidence,
    MusicSelectDifficultyObservation, MusicSelectDifficultyState,
    MusicSelectDifficultyUnknownReason, MusicSelectMotionRegions, MusicSelectPlaySideObservation,
    MusicSelectPlayTypeObservation, MusicSelectScreenFieldObservations, PlaySide,
    resolve_music_select_song,
};
pub use crate::recognition::result::{
    PlayOption, PlayOptions, PlayOptionsUnknownReason, PreviousBest, PreviousBestValue,
    ResultChartResolution, ResultFieldUnknownReason, ResultJudgments, ResultPerformanceResolution,
    ResultSongResolution, ResultTiming, SupplementalResultValue, resolve_clear_type,
};
pub use crate::recognition::screen::{
    PlayPresenceEvidence, RecognitionError, ResultPanelPresenceEvidence, ResultPanelSide,
    ResultPanelSideState, ResultPanelSideUnknownReason, ResultPresenceEvidence,
    ResultScreenRgb8Crops, Rgb8Crop, ScreenClass, ScreenCropRoute, ScreenFieldObservations,
    ScreenRgb8Crops, inspect_canonical_rgb8, route_screen_rgb8_crops,
};
pub use crate::recognition::shared::{CatalogCandidateDomain, ScreenCatalogCandidateObservations};
