//! Portable model-session capabilities used by live and replay recognition.

mod screen_observation;
mod text_observer_pool;

pub use screen_observation::{
    CurrentScoreOcrAttempt, CurrentScoreOcrResolution, CurrentScoreOcrSelection,
    FrameTimedScreenFieldObservation, ProjectedScreenFieldObservation, RecognitionFrameTiming,
    RecognitionProcessingTiming, RegisteredScreenFieldObservation, TitleEvidenceObservation,
};

pub use text_observer_pool::{
    PendingTextRecognition, RecognitionExecutionMode, RegisteredTextRecognitionSession,
    TextRecognitionResult, recommended_text_worker_count,
};
