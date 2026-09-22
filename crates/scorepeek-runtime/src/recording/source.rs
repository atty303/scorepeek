use std::time::Duration;

use crate::diagnostics::live::BoundCanonicalFrame;

/// A source adapter that yields frames at the shared canonical recognition boundary.
///
/// Capture, decoding, and normalization remain source-owned. Recognition receives only this
/// profile-bound frame shape and therefore cannot select a source-specific downstream path.
pub trait CanonicalFrameSource {
    type Error;

    fn next_frame(
        &mut self,
        maximum_wait: Duration,
    ) -> Result<Option<BoundCanonicalFrame>, Self::Error>;
}
