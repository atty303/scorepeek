//! Portable text-model observation contracts.

use serde::Serialize;

/// One open-text observation produced without granting field or song authority.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DynamicTextObservation {
    pub input_width: usize,
    pub output_timesteps: usize,
    pub open_text: String,
    pub constrained_text: Option<String>,
}

impl DynamicTextObservation {
    #[must_use]
    pub fn constrained_text(&self) -> Option<&str> {
        self.constrained_text.as_deref()
    }
}
