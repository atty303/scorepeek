//! Errors at the canonical frame and region boundary.

#[derive(Debug)]
pub enum FrameError {
    InvalidCanonicalFrame,
    InvalidCanonicalLayout,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCanonicalFrame => formatter.write_str("canonical frame is invalid"),
            Self::InvalidCanonicalLayout => formatter.write_str("canonical layout is invalid"),
        }
    }
}

impl std::error::Error for FrameError {}
