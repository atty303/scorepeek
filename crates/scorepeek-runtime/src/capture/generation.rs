//! Capture-generation identity and lifecycle counters.

/// Application-owned identity for one uninterrupted capture lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CaptureGeneration(u64);

impl CaptureGeneration {
    /// Creates one nonzero capture generation.
    ///
    /// # Errors
    /// Returns `InvalidCaptureGeneration` when the application supplies zero.
    pub const fn new(value: u64) -> Result<Self, InvalidCaptureGeneration> {
        if value == 0 {
            return Err(InvalidCaptureGeneration);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCaptureGeneration;

impl std::fmt::Display for InvalidCaptureGeneration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("capture generation must be nonzero")
    }
}

impl std::error::Error for InvalidCaptureGeneration {}
