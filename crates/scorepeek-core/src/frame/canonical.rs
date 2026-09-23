use super::{FrameError, Roi};

pub const CANONICAL_WIDTH: u32 = 1_920;
pub const CANONICAL_HEIGHT: u32 = 1_080;
pub const CANONICAL_BYTES: usize = CANONICAL_WIDTH as usize * CANONICAL_HEIGHT as usize * 3;
pub const CANONICAL_FRAME_CONTRACT_ID: &str = "scorepeek-canonical-rgb8-1920x1080-v1";

/// Borrowed pixels admitted to the fixed canonical recognition contract.
#[derive(Clone, Copy, Debug)]
pub struct CanonicalFrameView<'a> {
    pixels: &'a [u8],
}

impl<'a> CanonicalFrameView<'a> {
    /// Validates the fixed RGB8 byte count without taking ownership of the pixels.
    ///
    /// # Errors
    /// Returns an error when the frame does not have the canonical shape.
    pub fn new(pixels: &'a [u8]) -> Result<Self, FrameError> {
        if pixels.len() != CANONICAL_BYTES {
            return Err(FrameError::InvalidCanonicalFrame);
        }
        Ok(Self { pixels })
    }

    #[must_use]
    pub const fn pixels(self) -> &'a [u8] {
        self.pixels
    }

    /// Copies one layout-bound RGB8 crop in row-major order.
    ///
    /// # Errors
    /// Returns an error when the ROI is outside the canonical frame.
    pub fn crop_region(self, roi: Roi) -> Result<Vec<u8>, FrameError> {
        crop_pixels(self.pixels, roi)
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
