use std::fmt;
use std::sync::Arc;

use serde::Serialize;

use super::{CaptureGeneration, UncalibratedFrame};

const CANONICAL_WIDTH: usize = 1_920;
const CANONICAL_HEIGHT: usize = 1_080;
const CANONICAL_BYTES: usize = CANONICAL_WIDTH * CANONICAL_HEIGHT * 3;
const BGRX_BYTES_PER_PIXEL: usize = 4;
const COEFFICIENT_SCALE: f32 = 2_048.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RationalCoordinate {
    numerator: i64,
    denominator: u32,
}

impl RationalCoordinate {
    /// Creates one signed rational coordinate.
    ///
    /// # Errors
    /// Returns an error when the denominator is zero.
    pub const fn new(numerator: i64, denominator: u32) -> Result<Self, UnboundNormalizationError> {
        if denominator == 0 {
            return Err(UnboundNormalizationError::InvalidGeometry);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    #[allow(clippy::cast_precision_loss)]
    const fn as_f64(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
    }

    #[must_use]
    pub const fn numerator(self) -> i64 {
        self.numerator
    }

    #[must_use]
    pub const fn denominator(self) -> u32 {
        self.denominator
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct FractionalRectangle {
    left: RationalCoordinate,
    top: RationalCoordinate,
    width: RationalCoordinate,
    height: RationalCoordinate,
}

impl FractionalRectangle {
    #[must_use]
    pub const fn new(
        left: RationalCoordinate,
        top: RationalCoordinate,
        width: RationalCoordinate,
        height: RationalCoordinate,
    ) -> Self {
        Self {
            left,
            top,
            width,
            height,
        }
    }

    #[must_use]
    pub const fn left(self) -> RationalCoordinate {
        self.left
    }

    #[must_use]
    pub const fn top(self) -> RationalCoordinate {
        self.top
    }

    #[must_use]
    pub const fn width(self) -> RationalCoordinate {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> RationalCoordinate {
        self.height
    }
}

/// Explicit fractional source geometry for an unbound linear calibration candidate.
///
/// This type never detects borders or assigns a capture-profile identity. A separately reviewed
/// immutable binding must associate it with an exact observed contract before runtime recognition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct FractionalLinearGeometry {
    observed_width: u32,
    observed_height: u32,
    source: FractionalRectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanonicalRegion {
    pub left: u32,
    pub top: u32,
    pub width: u32,
    pub height: u32,
}

impl FractionalLinearGeometry {
    #[must_use]
    pub const fn source_rectangle(self) -> FractionalRectangle {
        self.source
    }

    /// Validates that every canonical pixel-center sample lies within the observed frame.
    ///
    /// # Errors
    /// Returns an error for a non-positive extent or an out-of-bounds sampling footprint.
    pub fn new(
        observed_width: u32,
        observed_height: u32,
        source: FractionalRectangle,
    ) -> Result<Self, UnboundNormalizationError> {
        if observed_width == 0
            || observed_height == 0
            || source.width.numerator <= 0
            || source.height.numerator <= 0
            || !sampling_axis_within(source.left, source.width, CANONICAL_WIDTH, observed_width)
            || !sampling_axis_within(source.top, source.height, CANONICAL_HEIGHT, observed_height)
        {
            return Err(UnboundNormalizationError::InvalidGeometry);
        }
        Ok(Self {
            observed_width,
            observed_height,
            source,
        })
    }

    /// Applies the explicit linear transform without creating a profile-bound `CanonicalFrame`.
    ///
    /// # Errors
    /// Returns a typed error when the frame dimensions, stride, or byte length do not match the
    /// geometry's observed `BGRx` contract.
    pub fn normalize(
        &self,
        frame: &UncalibratedFrame,
    ) -> Result<UnboundCanonicalFrame, UnboundNormalizationError> {
        let contract = frame.contract();
        if contract.width != self.observed_width || contract.height != self.observed_height {
            return Err(UnboundNormalizationError::ObservedContractMismatch);
        }
        let stride = usize::try_from(frame.stride())
            .map_err(|_| UnboundNormalizationError::StrideMismatch)?;
        let observed_width = usize::try_from(self.observed_width)
            .map_err(|_| UnboundNormalizationError::InvalidGeometry)?;
        let observed_height = usize::try_from(self.observed_height)
            .map_err(|_| UnboundNormalizationError::InvalidGeometry)?;
        let minimum_stride = observed_width
            .checked_mul(BGRX_BYTES_PER_PIXEL)
            .ok_or(UnboundNormalizationError::StrideMismatch)?;
        if stride < minimum_stride {
            return Err(UnboundNormalizationError::StrideMismatch);
        }
        let expected_bytes = stride
            .checked_mul(observed_height)
            .ok_or(UnboundNormalizationError::FrameLengthMismatch)?;
        if frame.bytes().len() != expected_bytes {
            return Err(UnboundNormalizationError::FrameLengthMismatch);
        }
        let pixels = normalize_bgrx(
            frame.bytes(),
            stride,
            observed_width,
            observed_height,
            self.source,
        );
        Ok(UnboundCanonicalFrame {
            pixels: pixels.into_boxed_slice(),
            source_sequence: frame.sequence(),
            received_monotonic_ns: frame.received_monotonic_ns(),
        })
    }

    /// Applies the production transform to one packed or padded `BGRx` frame.
    ///
    /// This is the filesystem-free replay boundary used by offline diagnostic generation. It
    /// creates no capture admission or profile authority.
    ///
    /// # Errors
    /// Returns a typed error when dimensions, stride, byte length, or saved geometry differ from
    /// the measured profile contract.
    pub fn normalize_bgrx_bytes(
        &self,
        bytes: &[u8],
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<Box<[u8]>, UnboundNormalizationError> {
        if width != self.observed_width || height != self.observed_height {
            return Err(UnboundNormalizationError::ObservedContractMismatch);
        }
        let stride =
            usize::try_from(stride).map_err(|_| UnboundNormalizationError::StrideMismatch)?;
        let observed_width =
            usize::try_from(width).map_err(|_| UnboundNormalizationError::InvalidGeometry)?;
        let observed_height =
            usize::try_from(height).map_err(|_| UnboundNormalizationError::InvalidGeometry)?;
        if stride < observed_width.saturating_mul(BGRX_BYTES_PER_PIXEL)
            || stride.checked_mul(observed_height) != Some(bytes.len())
        {
            return Err(UnboundNormalizationError::FrameLengthMismatch);
        }
        Ok(
            normalize_bgrx(bytes, stride, observed_width, observed_height, self.source)
                .into_boxed_slice(),
        )
    }

    /// Applies the production transform only within one canonical rectangle.
    ///
    /// This preserves the exact interpolation used by full-frame normalization while avoiding
    /// unrelated pixels in offline measurement tools.
    ///
    /// # Errors
    /// Returns a typed error when the observed frame contract or canonical region is invalid.
    #[allow(clippy::too_many_arguments)]
    pub fn normalize_bgrx_region(
        &self,
        bytes: &[u8],
        width: u32,
        height: u32,
        stride: u32,
        region: CanonicalRegion,
    ) -> Result<Box<[u8]>, UnboundNormalizationError> {
        if width != self.observed_width || height != self.observed_height {
            return Err(UnboundNormalizationError::ObservedContractMismatch);
        }
        let stride =
            usize::try_from(stride).map_err(|_| UnboundNormalizationError::StrideMismatch)?;
        let observed_width =
            usize::try_from(width).map_err(|_| UnboundNormalizationError::InvalidGeometry)?;
        let observed_height =
            usize::try_from(height).map_err(|_| UnboundNormalizationError::InvalidGeometry)?;
        if stride < observed_width.saturating_mul(BGRX_BYTES_PER_PIXEL)
            || stride.checked_mul(observed_height) != Some(bytes.len())
            || region.width == 0
            || region.height == 0
            || region
                .left
                .checked_add(region.width)
                .is_none_or(|right| right > 1_920)
            || region
                .top
                .checked_add(region.height)
                .is_none_or(|bottom| bottom > 1_080)
        {
            return Err(UnboundNormalizationError::InvalidGeometry);
        }
        Ok(normalize_bgrx_region(
            bytes,
            stride,
            observed_width,
            observed_height,
            self.source,
            region,
        )
        .into_boxed_slice())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnboundNormalizationError {
    InvalidGeometry,
    ObservedContractMismatch,
    StrideMismatch,
    FrameLengthMismatch,
}

/// RGB8 1920x1080 calibration output without capture-profile or normalizer identity.
///
/// It is deliberately distinct from recognition's `CanonicalFrame` and cannot enter recognition.
pub struct UnboundCanonicalFrame {
    pixels: Box<[u8]>,
    source_sequence: u64,
    received_monotonic_ns: u64,
}

impl fmt::Debug for UnboundCanonicalFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UnboundCanonicalFrame")
            .field("width", &CANONICAL_WIDTH)
            .field("height", &CANONICAL_HEIGHT)
            .field("byte_count", &self.pixels.len())
            .field("source_sequence", &self.source_sequence)
            .field("received_monotonic_ns", &self.received_monotonic_ns)
            .finish()
    }
}

impl UnboundCanonicalFrame {
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub const fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    #[must_use]
    pub const fn received_monotonic_ns(&self) -> u64 {
        self.received_monotonic_ns
    }

    #[must_use]
    pub fn into_pixels(self) -> Box<[u8]> {
        self.pixels
    }
}

/// Canonical RGB8 frame bound to one admitted capture generation, profile, and normalizer.
pub struct NormalizedCanonicalFrame {
    pixels: Box<[u8]>,
    capture_generation: CaptureGeneration,
    capture_profile_sha256: Arc<str>,
    normalizer_artifact_sha256: Arc<str>,
    source_sequence: u64,
    received_monotonic_ns: u64,
}

impl fmt::Debug for NormalizedCanonicalFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NormalizedCanonicalFrame")
            .field("capture_generation", &self.capture_generation)
            .field("capture_profile_sha256", &self.capture_profile_sha256)
            .field(
                "normalizer_artifact_sha256",
                &self.normalizer_artifact_sha256,
            )
            .field("source_sequence", &self.source_sequence)
            .field("received_monotonic_ns", &self.received_monotonic_ns)
            .field("byte_count", &self.pixels.len())
            .finish()
    }
}

impl NormalizedCanonicalFrame {
    pub(super) fn bind(
        frame: UnboundCanonicalFrame,
        capture_generation: CaptureGeneration,
        capture_profile_sha256: Arc<str>,
        normalizer_artifact_sha256: Arc<str>,
    ) -> Self {
        Self {
            source_sequence: frame.source_sequence,
            received_monotonic_ns: frame.received_monotonic_ns,
            pixels: frame.pixels,
            capture_generation,
            capture_profile_sha256,
            normalizer_artifact_sha256,
        }
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn into_pixels(self) -> Box<[u8]> {
        self.pixels
    }

    #[must_use]
    pub const fn capture_generation(&self) -> CaptureGeneration {
        self.capture_generation
    }

    #[must_use]
    pub fn capture_profile_sha256(&self) -> &str {
        &self.capture_profile_sha256
    }

    #[must_use]
    pub fn normalizer_artifact_sha256(&self) -> &str {
        &self.normalizer_artifact_sha256
    }

    #[must_use]
    pub const fn source_sequence(&self) -> u64 {
        self.source_sequence
    }

    #[must_use]
    pub const fn received_monotonic_ns(&self) -> u64 {
        self.received_monotonic_ns
    }
}

fn sampling_axis_within(
    start: RationalCoordinate,
    extent: RationalCoordinate,
    target_size: usize,
    source_size: u32,
) -> bool {
    let start_numerator = i128::from(start.numerator);
    let start_denominator = i128::from(start.denominator);
    let extent_numerator = i128::from(extent.numerator);
    let extent_denominator = i128::from(extent.denominator);
    let target_size = i128::try_from(target_size).expect("canonical size fits i128");
    let source_size = i128::from(source_size);
    let doubled_target = 2 * target_size;
    let first_numerator = start_numerator * extent_denominator * doubled_target
        + extent_numerator * start_denominator;
    let last_numerator = start_numerator * extent_denominator * doubled_target
        + extent_numerator * start_denominator * (doubled_target - 1);
    let common_denominator = start_denominator * extent_denominator * doubled_target;
    first_numerator >= 0 && last_numerator <= source_size * common_denominator
}

fn normalize_bgrx(
    source: &[u8],
    stride: usize,
    observed_width: usize,
    observed_height: usize,
    rectangle: FractionalRectangle,
) -> Vec<u8> {
    let horizontal = interpolation_axis(
        rectangle.left,
        rectangle.width,
        CANONICAL_WIDTH,
        observed_width,
    );
    let vertical = interpolation_axis(
        rectangle.top,
        rectangle.height,
        CANONICAL_HEIGHT,
        observed_height,
    );
    let mut output = vec![0; CANONICAL_BYTES];
    for (target_y, &(source_y, vertical_weights)) in vertical.iter().enumerate() {
        let next_y = (source_y + 1).min(observed_height - 1);
        for (target_x, &(source_x, horizontal_weights)) in horizontal.iter().enumerate() {
            let next_x = (source_x + 1).min(observed_width - 1);
            for (target_channel, source_channel) in [2, 1, 0].into_iter().enumerate() {
                let top = i32::from(
                    source[source_y * stride + source_x * BGRX_BYTES_PER_PIXEL + source_channel],
                ) * horizontal_weights[0]
                    + i32::from(
                        source[source_y * stride + next_x * BGRX_BYTES_PER_PIXEL + source_channel],
                    ) * horizontal_weights[1];
                let bottom = i32::from(
                    source[next_y * stride + source_x * BGRX_BYTES_PER_PIXEL + source_channel],
                ) * horizontal_weights[0]
                    + i32::from(
                        source[next_y * stride + next_x * BGRX_BYTES_PER_PIXEL + source_channel],
                    ) * horizontal_weights[1];
                let top_term = (vertical_weights[0] * (top >> 4)) >> 16;
                let bottom_term = (vertical_weights[1] * (bottom >> 4)) >> 16;
                let value = ((top_term + bottom_term + 2) >> 2).clamp(0, 255);
                output[(target_y * CANONICAL_WIDTH + target_x) * 3 + target_channel] =
                    u8::try_from(value).expect("clamped normalization output must fit in u8");
            }
        }
    }
    output
}

fn normalize_bgrx_region(
    source: &[u8],
    stride: usize,
    observed_width: usize,
    observed_height: usize,
    rectangle: FractionalRectangle,
    region: CanonicalRegion,
) -> Vec<u8> {
    let horizontal = interpolation_axis(
        rectangle.left,
        rectangle.width,
        CANONICAL_WIDTH,
        observed_width,
    );
    let vertical = interpolation_axis(
        rectangle.top,
        rectangle.height,
        CANONICAL_HEIGHT,
        observed_height,
    );
    let mut output = Vec::with_capacity(region.width as usize * region.height as usize * 3);
    for &(source_y, vertical_weights) in vertical
        .iter()
        .take((region.top + region.height) as usize)
        .skip(region.top as usize)
    {
        let next_y = (source_y + 1).min(observed_height - 1);
        for &(source_x, horizontal_weights) in horizontal
            .iter()
            .take((region.left + region.width) as usize)
            .skip(region.left as usize)
        {
            let next_x = (source_x + 1).min(observed_width - 1);
            for source_channel in [2, 1, 0] {
                let top = i32::from(
                    source[source_y * stride + source_x * BGRX_BYTES_PER_PIXEL + source_channel],
                ) * horizontal_weights[0]
                    + i32::from(
                        source[source_y * stride + next_x * BGRX_BYTES_PER_PIXEL + source_channel],
                    ) * horizontal_weights[1];
                let bottom = i32::from(
                    source[next_y * stride + source_x * BGRX_BYTES_PER_PIXEL + source_channel],
                ) * horizontal_weights[0]
                    + i32::from(
                        source[next_y * stride + next_x * BGRX_BYTES_PER_PIXEL + source_channel],
                    ) * horizontal_weights[1];
                let top_term = (vertical_weights[0] * (top >> 4)) >> 16;
                let bottom_term = (vertical_weights[1] * (bottom >> 4)) >> 16;
                output.push(
                    u8::try_from(((top_term + bottom_term + 2) >> 2).clamp(0, 255))
                        .expect("clamped normalization output must fit in u8"),
                );
            }
        }
    }
    output
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn interpolation_axis(
    start: RationalCoordinate,
    extent: RationalCoordinate,
    target_size: usize,
    source_size: usize,
) -> Vec<(usize, [i32; 2])> {
    let start = start.as_f64();
    let extent = extent.as_f64();
    (0..target_size)
        .map(|target| {
            let coordinate =
                (start + (target as f64 + 0.5) * extent / target_size as f64 - 0.5) as f32;
            let base = coordinate.floor() as isize;
            let fraction = coordinate - base as f32;
            let weights = [
                ((1.0 - fraction) * COEFFICIENT_SCALE).round_ties_even() as i32,
                (fraction * COEFFICIENT_SCALE).round_ties_even() as i32,
            ];
            let Ok(base) = usize::try_from(base) else {
                return (0, [2_048, 0]);
            };
            let base = base.min(source_size - 1);
            if base == source_size - 1 {
                (base, [2_048, 0])
            } else {
                (base, weights)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::UncalibratedVideoContract;

    fn rational(numerator: i64, denominator: u32) -> RationalCoordinate {
        RationalCoordinate::new(numerator, denominator).unwrap()
    }

    fn full_frame_geometry() -> FractionalLinearGeometry {
        FractionalLinearGeometry::new(
            1_920,
            1_080,
            FractionalRectangle::new(
                rational(0, 1),
                rational(0, 1),
                rational(1_920, 1),
                rational(1_080, 1),
            ),
        )
        .unwrap()
    }

    fn video_contract(width: u32, height: u32) -> UncalibratedVideoContract {
        UncalibratedVideoContract {
            width,
            height,
            framerate_num: 0,
            framerate_denom: 1,
            maximum_framerate_num: 0,
            maximum_framerate_denom: 0,
            pixel_aspect_num: 0,
            pixel_aspect_denom: 0,
            chroma_site: 0,
            color_range: 0,
            color_matrix: 0,
            transfer_function: 0,
            color_primaries: 0,
        }
    }

    #[test]
    fn rational_geometry_is_exact_bounded_and_nonzero() {
        assert!(RationalCoordinate::new(1, 0).is_err());
        assert!(
            FractionalLinearGeometry::new(
                100,
                100,
                FractionalRectangle::new(
                    rational(1, 3),
                    rational(0, 1),
                    rational(299, 3),
                    rational(100, 1),
                ),
            )
            .is_ok()
        );
        assert!(
            FractionalLinearGeometry::new(
                100,
                100,
                FractionalRectangle::new(
                    rational(1, 3),
                    rational(0, 1),
                    rational(300, 3),
                    rational(100, 1),
                ),
            )
            .is_err()
        );
        assert!(
            FractionalLinearGeometry::new(
                3_840,
                2_160,
                FractionalRectangle::new(
                    rational(-1, 2),
                    rational(-1, 2),
                    rational(3_840, 1),
                    rational(2_160, 1),
                ),
            )
            .is_ok()
        );
        assert!(
            FractionalLinearGeometry::new(
                3_840,
                2_160,
                FractionalRectangle::new(
                    rational(-2_049, 2_048),
                    rational(0, 1),
                    rational(3_840, 1),
                    rational(2_160, 1),
                ),
            )
            .is_err()
        );
        assert!(
            FractionalLinearGeometry::new(
                100,
                100,
                FractionalRectangle::new(
                    rational(0, 1),
                    rational(0, 1),
                    rational(0, 1),
                    rational(100, 1),
                ),
            )
            .is_err()
        );
    }

    #[test]
    fn region_normalization_matches_the_same_full_frame_pixels() {
        let geometry = full_frame_geometry();
        let mut source = vec![0_u8; 1_920 * 1_080 * 4];
        for (index, pixel) in source.chunks_exact_mut(4).enumerate() {
            pixel[0] = u8::try_from(index % 251).unwrap();
            pixel[1] = u8::try_from(index % 241).unwrap();
            pixel[2] = u8::try_from(index % 239).unwrap();
        }
        let region = CanonicalRegion {
            left: 100,
            top: 200,
            width: 3,
            height: 2,
        };
        let full = geometry
            .normalize_bgrx_bytes(&source, 1_920, 1_080, 1_920 * 4)
            .unwrap();
        let selected = geometry
            .normalize_bgrx_region(&source, 1_920, 1_080, 1_920 * 4, region)
            .unwrap();
        let expected = (region.top..region.top + region.height)
            .flat_map(|y| {
                let start = ((y * 1_920 + region.left) * 3) as usize;
                full[start..start + (region.width * 3) as usize].to_vec()
            })
            .collect::<Vec<_>>();
        assert_eq!(selected.as_ref(), expected);
    }

    #[test]
    fn gamescope_fit_coordinates_retain_fractional_sampling_phase() {
        let horizontal =
            interpolation_axis(rational(26, 3), rational(7_616, 3), CANONICAL_WIDTH, 2_556);
        let vertical =
            interpolation_axis(rational(0, 1), rational(1_428, 1), CANONICAL_HEIGHT, 1_428);
        assert_eq!(horizontal[0], (8, [353, 1_695]));
        assert_eq!(horizontal[CANONICAL_WIDTH - 1], (2_546, [1_696, 352]));
        assert_eq!(vertical[0], (0, [1_718, 330]));
        assert_eq!(vertical[CANONICAL_HEIGHT - 1], (1_426, [330, 1_718]));
    }

    #[test]
    fn full_frame_linear_normalization_preserves_rgb_and_ignores_padding() {
        let geometry = full_frame_geometry();
        let stride = 1_920 * 4 + 8;
        let mut bgrx = vec![0x7f; stride * 1_080];
        for y in 0..1_080 {
            for x in 0..1_920 {
                let offset = y * stride + x * 4;
                bgrx[offset..offset + 4].copy_from_slice(&[11, 22, 33, 0]);
            }
        }
        let frame = UncalibratedFrame::for_normalizer_test(
            video_contract(1_920, 1_080),
            u32::try_from(stride).unwrap(),
            7,
            11,
            bgrx,
        );
        let normalized = geometry.normalize(&frame).unwrap();
        assert_eq!(normalized.pixels().len(), CANONICAL_BYTES);
        assert_eq!(&normalized.pixels()[..3], &[33, 22, 11]);
        assert_eq!(
            &normalized.pixels()[normalized.pixels().len() - 3..],
            &[33, 22, 11]
        );
        assert_eq!(normalized.source_sequence(), 7);
        assert_eq!(normalized.received_monotonic_ns(), 11);
    }

    #[test]
    fn frame_contract_and_length_mismatch_fail_closed() {
        let geometry = full_frame_geometry();
        let wrong_contract = UncalibratedFrame::for_normalizer_test(
            video_contract(1_919, 1_080),
            1_919 * 4,
            1,
            2,
            vec![0; 1_919 * 1_080 * 4],
        );
        assert_eq!(
            geometry.normalize(&wrong_contract).unwrap_err(),
            UnboundNormalizationError::ObservedContractMismatch
        );

        let short_frame = UncalibratedFrame::for_normalizer_test(
            video_contract(1_920, 1_080),
            1_920 * 4,
            1,
            2,
            vec![0; 1_920 * 1_080 * 4 - 1],
        );
        assert_eq!(
            geometry.normalize(&short_frame).unwrap_err(),
            UnboundNormalizationError::FrameLengthMismatch
        );
    }

    #[test]
    fn upscaling_replicates_the_top_left_border() {
        let geometry = FractionalLinearGeometry::new(
            1_920,
            1_080,
            FractionalRectangle::new(
                rational(0, 1),
                rational(0, 1),
                rational(100, 1),
                rational(100, 1),
            ),
        )
        .unwrap();
        let stride = 1_920 * 4;
        let mut bgrx = vec![0; stride * 1_080];
        bgrx[..4].copy_from_slice(&[11, 22, 33, 0]);
        bgrx[4..8].copy_from_slice(&[101, 102, 103, 0]);
        bgrx[stride..stride + 4].copy_from_slice(&[201, 202, 203, 0]);
        let pixels = normalize_bgrx(&bgrx, stride, 1_920, 1_080, geometry.source);
        assert_eq!(&pixels[..3], &[33, 22, 11]);
    }

    #[test]
    fn unbound_frame_debug_omits_pixels() {
        let frame = UnboundCanonicalFrame {
            pixels: vec![1; CANONICAL_BYTES].into_boxed_slice(),
            source_sequence: 7,
            received_monotonic_ns: 11,
        };
        let debug = format!("{frame:?}");
        assert!(debug.contains("byte_count"));
        assert!(!debug.contains("[1, 1"));
    }
}
