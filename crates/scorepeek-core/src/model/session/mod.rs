//! Portable projected field-observation values.

mod screen_observation;

pub use screen_observation::{
    CurrentScoreOcrAttempt, CurrentScoreOcrResolution, CurrentScoreOcrSelection,
    FrameTimedScreenFieldObservation, ProjectedScreenFieldObservation, RecognitionFrameTiming,
    RecognitionProcessingTiming, RegisteredScreenFieldObservation, TitleEvidenceObservation,
};
