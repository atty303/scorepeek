use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use scorepeek::recognition::CanonicalFrame;

use crate::diagnostic_live::BoundCanonicalFrame;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionFrameSelection {
    pub sequence: u64,
    pub frame_id: String,
    pub source_pts_ms: u64,
}

/// An immutable recording-derived canonical source.
///
/// The selected extraction is a cache derived from the corpus-owned MKV. Its profile author binds
/// the recording, probe, extraction, normalizer, and selected source timestamps before this adapter
/// is constructed.
pub struct RecordingCanonicalFrameSource {
    extraction_directory: PathBuf,
    extraction_sha256: String,
    capture_generation: u64,
    selections: Vec<ExtractionFrameSelection>,
    next: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecordingCanonicalSourceError {
    CanonicalFrameInvalid,
    FrameBindingMismatch,
}

impl RecordingCanonicalFrameSource {
    #[must_use]
    pub fn new(
        extraction_directory: &Path,
        extraction_sha256: String,
        capture_generation: u64,
        selections: Vec<ExtractionFrameSelection>,
    ) -> Self {
        Self {
            extraction_directory: extraction_directory.to_owned(),
            extraction_sha256,
            capture_generation,
            selections,
            next: 0,
        }
    }
}

impl CanonicalFrameSource for RecordingCanonicalFrameSource {
    type Error = RecordingCanonicalSourceError;

    fn next_frame(
        &mut self,
        maximum_wait: Duration,
    ) -> Result<Option<BoundCanonicalFrame>, Self::Error> {
        let Some(selection) = self.selections.get(self.next) else {
            return Ok(None);
        };
        if self.next != 0 && !maximum_wait.is_zero() {
            thread::sleep(maximum_wait);
        }
        let frame = CanonicalFrame::read_extraction(
            &self.extraction_directory,
            &selection.frame_id,
            &self.extraction_sha256,
        )
        .map_err(|_| RecordingCanonicalSourceError::CanonicalFrameInvalid)?;
        if u64::try_from(frame.source_pts_ms()).ok() != Some(selection.source_pts_ms) {
            return Err(RecordingCanonicalSourceError::FrameBindingMismatch);
        }
        let bound = BoundCanonicalFrame::from_extraction(
            frame,
            self.capture_generation,
            selection.sequence,
            selection.source_pts_ms,
            selection.source_pts_ms,
        );
        self.next += 1;
        Ok(Some(bound))
    }
}
