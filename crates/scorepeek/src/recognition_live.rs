use scorepeek::recognition::{
    CanonicalLayout, RecognitionError, ScreenClass, ScreenPredicateObservation,
    inspect_canonical_rgb8,
};

use crate::diagnostic_live::LiveCanonicalFrame;

/// One screen-predicate result that borrows its immutable live capture evidence.
///
/// The result cannot outlive or detach from the profile- and generation-bearing frame that was
/// inspected. It carries no accepted field or event authority.
#[derive(Debug)]
pub struct LiveRecognitionObservation<'a> {
    frame: &'a LiveCanonicalFrame,
    canonical_layout_sha256: String,
    predicate: ScreenPredicateObservation,
}

impl<'a> LiveRecognitionObservation<'a> {
    /// Applies the embedded screen predicate to one admitted live canonical owner.
    ///
    /// # Errors
    /// Returns an error when the fixed canonical pixel or embedded layout contract is invalid.
    pub fn inspect(frame: &'a LiveCanonicalFrame) -> Result<Self, RecognitionError> {
        Ok(Self {
            frame,
            canonical_layout_sha256: CanonicalLayout::sha256(),
            predicate: inspect_canonical_rgb8(frame.pixels())?,
        })
    }

    #[must_use]
    pub const fn screen(&self) -> ScreenClass {
        self.predicate.screen
    }

    #[must_use]
    pub(crate) const fn frame(&self) -> &LiveCanonicalFrame {
        self.frame
    }

    #[must_use]
    pub(crate) fn canonical_layout_sha256(&self) -> &str {
        &self.canonical_layout_sha256
    }
}
