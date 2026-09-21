//! Portable model-session capabilities used by live and replay recognition.

pub use crate::recognition::{OnnxParityError, RecognitionError};

mod text_observer_pool;

pub use text_observer_pool::{
    PendingTextRecognition, RecognitionExecutionMode, RegisteredTextRecognitionSession,
    TextRecognitionResult, recommended_text_worker_count,
};
