use super::{FrameError, Roi};

pub const CANONICAL_WIDTH: u32 = 1_920;
pub const CANONICAL_HEIGHT: u32 = 1_080;
pub const CANONICAL_BYTES: usize = CANONICAL_WIDTH as usize * CANONICAL_HEIGHT as usize * 3;
pub const CANONICAL_FRAME_CONTRACT_ID: &str = "scorepeek-canonical-rgb8-1920x1080-v1";

/// One frame admitted at the fixed canonical recognition boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalFrame {
    pub(crate) pixels: Box<[u8]>,
    pub(crate) source_pts_ms: i64,
    pub(crate) decode_index: u64,
    pub(crate) capture_profile_id: String,
    pub(crate) normalizer_artifact_sha256: String,
    pub(crate) frame_extraction_sha256: String,
}

impl CanonicalFrame {
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn into_pixels(self) -> Box<[u8]> {
        self.pixels
    }

    #[must_use]
    pub fn capture_profile_id(&self) -> &str {
        &self.capture_profile_id
    }

    #[must_use]
    pub const fn source_pts_ms(&self) -> i64 {
        self.source_pts_ms
    }

    #[must_use]
    pub const fn decode_index(&self) -> u64 {
        self.decode_index
    }

    #[must_use]
    pub fn normalizer_artifact_sha256(&self) -> &str {
        &self.normalizer_artifact_sha256
    }

    #[must_use]
    pub fn frame_extraction_sha256(&self) -> &str {
        &self.frame_extraction_sha256
    }

    /// Copies one layout-bound RGB8 crop in row-major order.
    ///
    /// # Errors
    /// Returns an error when the ROI is outside the canonical frame.
    pub fn crop_region(&self, roi: Roi) -> Result<Vec<u8>, FrameError> {
        crop_pixels(&self.pixels, roi)
    }
}

pub(crate) fn crop_pixels(pixels: &[u8], roi: Roi) -> Result<Vec<u8>, FrameError> {
    roi.validate(CANONICAL_WIDTH, CANONICAL_HEIGHT)?;
    if pixels.len() != CANONICAL_BYTES {
        return Err(FrameError::InvalidCanonicalFrame);
    }
    let row_bytes = roi.width as usize * 3;
    let mut crop = Vec::with_capacity(row_bytes * roi.height as usize);
    for y in roi.y..roi.y + roi.height {
        let start = (y as usize * CANONICAL_WIDTH as usize + roi.x as usize) * 3;
        crop.extend_from_slice(&pixels[start..start + row_bytes]);
    }
    Ok(crop)
}
