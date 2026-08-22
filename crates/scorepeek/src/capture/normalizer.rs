use std::fmt;

use serde::Serialize;

use super::UncalibratedFrame;

const CANONICAL_WIDTH: usize = 1_920;
const CANONICAL_HEIGHT: usize = 1_080;
const CANONICAL_BYTES: usize = CANONICAL_WIDTH * CANONICAL_HEIGHT * 3;
const BGRX_BYTES_PER_PIXEL: usize = 4;
const COEFFICIENT_SCALE: f32 = 2_048.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RationalCoordinate {
    numerator: u32,
    denominator: u32,
}

impl RationalCoordinate {
    /// Creates one non-negative rational coordinate.
    ///
    /// # Errors
    /// Returns an error when the denominator is zero.
    pub const fn new(numerator: u32, denominator: u32) -> Result<Self, UnboundNormalizationError> {
        if denominator == 0 {
            return Err(UnboundNormalizationError::InvalidGeometry);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    const fn as_f64(self) -> f64 {
        self.numerator as f64 / self.denominator as f64
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

impl FractionalLinearGeometry {
    /// Validates an explicit fractional source rectangle within the observed frame.
    ///
    /// # Errors
    /// Returns an error for a zero-sized or out-of-bounds rectangle.
    pub fn new(
        observed_width: u32,
        observed_height: u32,
        source: FractionalRectangle,
    ) -> Result<Self, UnboundNormalizationError> {
        if observed_width == 0
            || observed_height == 0
            || source.width.numerator == 0
            || source.height.numerator == 0
            || !coordinate_sum_within(source.left, source.width, observed_width)
            || !coordinate_sum_within(source.top, source.height, observed_height)
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
}

fn coordinate_sum_within(
    start: RationalCoordinate,
    extent: RationalCoordinate,
    boundary: u32,
) -> bool {
    let left = u128::from(start.numerator) * u128::from(extent.denominator);
    let width = u128::from(extent.numerator) * u128::from(start.denominator);
    let limit =
        u128::from(boundary) * u128::from(start.denominator) * u128::from(extent.denominator);
    left + width <= limit
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

    fn rational(numerator: u32, denominator: u32) -> RationalCoordinate {
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
