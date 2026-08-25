use super::{RecognitionError, Roi};

pub const TITLE_PREPROCESSOR_ID: &str = "paddlex-3.7.0-bgr-rec-resize-3x48x320-v1";
pub(super) const DYNAMIC_TITLE_PREPROCESSOR_ID: &str =
    "paddleocr-3.7.0-bgr-dynamic-rec-resize-3x48x320-3200-v1";
pub(super) const TITLE_INPUT_SHAPE: [usize; 4] = [1, 3, 48, 320];
pub(super) const TITLE_INPUT_VALUES: usize = 3 * 48 * 320;
pub(super) const DYNAMIC_TITLE_INPUT_HEIGHT: usize = 48;
pub(super) const DYNAMIC_TITLE_MINIMUM_WIDTH: usize = 320;
pub(super) const DYNAMIC_TITLE_MAXIMUM_WIDTH: usize = 3_200;

pub(super) struct DynamicTitleInput {
    pub width: usize,
    pub values: Vec<f32>,
}

/// Applies the registered `PaddleX` recognition resize and normalization contract.
///
/// The input is the RGB8 title ROI from a validated canonical frame. The output is BGR,
/// channel-first `float32`, normalized to `[-1, 1]`, and zero-padded on the right.
///
/// # Errors
/// Returns an error unless the crop is exactly the shared-layout title ROI.
pub fn preprocess_title_crop(rgb: &[u8], roi: Roi) -> Result<Vec<f32>, RecognitionError> {
    if roi != super::CanonicalLayout::load()?.result.title
        || rgb.len() != roi.width as usize * roi.height as usize * 3
    {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    preprocess_title_image(rgb, roi.width as usize, roi.height as usize)
}

pub(super) fn preprocess_title_image(
    rgb: &[u8],
    source_width: usize,
    source_height: usize,
) -> Result<Vec<f32>, RecognitionError> {
    if source_width == 0 || source_height == 0 || rgb.len() != source_width * source_height * 3 {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let resized_height = TITLE_INPUT_SHAPE[2];
    let resized_width = (resized_height * source_width)
        .div_ceil(source_height)
        .min(TITLE_INPUT_SHAPE[3]);

    let resized = resize_linear_rgb(
        rgb,
        source_width,
        source_height,
        resized_width,
        resized_height,
    );
    let mut tensor = vec![0.0_f32; TITLE_INPUT_VALUES];
    let plane = TITLE_INPUT_SHAPE[2] * TITLE_INPUT_SHAPE[3];
    for y in 0..resized_height {
        for x in 0..resized_width {
            let pixel = (y * resized_width + x) * 3;
            for (channel, rgb_channel) in [2_usize, 1, 0].into_iter().enumerate() {
                tensor[channel * plane + y * TITLE_INPUT_SHAPE[3] + x] =
                    f32::from(resized[pixel + rgb_channel]) / 127.5 - 1.0;
            }
        }
    }
    Ok(tensor)
}

pub(super) fn preprocess_dynamic_title_image(
    rgb: &[u8],
    source_width: usize,
    source_height: usize,
) -> Result<DynamicTitleInput, RecognitionError> {
    if source_width == 0 || source_height == 0 || rgb.len() != source_width * source_height * 3 {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let proportional_width = DYNAMIC_TITLE_INPUT_HEIGHT
        .checked_mul(source_width)
        .ok_or(RecognitionError::InvalidCanonicalFrame)?
        / source_height;
    let width = proportional_width.clamp(DYNAMIC_TITLE_MINIMUM_WIDTH, DYNAMIC_TITLE_MAXIMUM_WIDTH);
    let resized_width = (DYNAMIC_TITLE_INPUT_HEIGHT * source_width)
        .div_ceil(source_height)
        .min(width);
    let resized = resize_linear_rgb(
        rgb,
        source_width,
        source_height,
        resized_width,
        DYNAMIC_TITLE_INPUT_HEIGHT,
    );
    let plane = DYNAMIC_TITLE_INPUT_HEIGHT * width;
    let mut values = vec![0.0_f32; 3 * plane];
    for y in 0..DYNAMIC_TITLE_INPUT_HEIGHT {
        for x in 0..resized_width {
            let pixel = (y * resized_width + x) * 3;
            for (channel, rgb_channel) in [2_usize, 1, 0].into_iter().enumerate() {
                values[channel * plane + y * width + x] =
                    f32::from(resized[pixel + rgb_channel]) / 127.5 - 1.0;
            }
        }
    }
    Ok(DynamicTitleInput { width, values })
}

fn resize_linear_rgb(
    source: &[u8],
    source_width: usize,
    source_height: usize,
    target_width: usize,
    target_height: usize,
) -> Vec<u8> {
    let mut output = vec![0_u8; target_width * target_height * 3];
    let horizontal: Vec<_> = (0..target_width)
        .map(|x| interpolation_horizontal_axis(x, source_width, target_width))
        .collect();
    let vertical: Vec<_> = (0..target_height)
        .map(|y| interpolation_vertical_axis(y, source_height, target_height))
        .collect();
    for (target_y, &(source_y, vertical_weights)) in vertical.iter().enumerate() {
        let source_y = clamped_source_index(source_y, source_height);
        let next_y = clamped_source_index(vertical[target_y].0 + 1, source_height);
        for (target_x, &(source_x, horizontal_weights)) in horizontal.iter().enumerate() {
            let next_x = (source_x + 1).min(source_width - 1);
            for channel in 0..3 {
                let top = i32::from(source[(source_y * source_width + source_x) * 3 + channel])
                    * horizontal_weights[0]
                    + i32::from(source[(source_y * source_width + next_x) * 3 + channel])
                        * horizontal_weights[1];
                let bottom = i32::from(source[(next_y * source_width + source_x) * 3 + channel])
                    * horizontal_weights[0]
                    + i32::from(source[(next_y * source_width + next_x) * 3 + channel])
                        * horizontal_weights[1];
                let top_term = (vertical_weights[0] * (top >> 4)) >> 16;
                let bottom_term = (vertical_weights[1] * (bottom >> 4)) >> 16;
                let value = ((top_term + bottom_term + 2) >> 2).clamp(0, 255);
                output[(target_y * target_width + target_x) * 3 + channel] =
                    u8::try_from(value).expect("clamped resize output must fit in u8");
            }
        }
    }
    output
}

fn clamped_source_index(index: isize, source_size: usize) -> usize {
    usize::try_from(index)
        .unwrap_or(0)
        .min(source_size.saturating_sub(1))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn interpolation_horizontal_axis(
    target: usize,
    source_size: usize,
    target_size: usize,
) -> (usize, [i32; 2]) {
    let (base, weights) = interpolation_vertical_axis(target, source_size, target_size);
    let Ok(base) = usize::try_from(base) else {
        return (0, [2048, 0]);
    };
    if base >= source_size - 1 {
        (source_size - 1, [2048, 0])
    } else {
        (base, weights)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn interpolation_vertical_axis(
    target: usize,
    source_size: usize,
    target_size: usize,
) -> (isize, [i32; 2]) {
    // These casts reproduce OpenCV's registered double-to-float and float-to-fixed-point path.
    // The fixed-point path matches the registered OpenCV linear resize for result and list crops.
    const COEFFICIENT_SCALE: f32 = 2048.0;
    let coordinate = ((target as f64 + 0.5) * source_size as f64 / target_size as f64 - 0.5) as f32;
    let base = coordinate.floor() as isize;
    let fraction = coordinate - base as f32;
    (
        base,
        [
            ((1.0 - fraction) * COEFFICIENT_SCALE).round_ties_even() as i32,
            (fraction * COEFFICIENT_SCALE).round_ties_even() as i32,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocessor_is_bgr_chw_normalized_at_full_width() {
        let roi = super::super::CanonicalLayout::load().unwrap().result.title;
        let mut rgb = vec![0_u8; roi.width as usize * roi.height as usize * 3];
        rgb.chunks_exact_mut(3)
            .for_each(|pixel| pixel.copy_from_slice(&[255, 128, 0]));
        let tensor = preprocess_title_crop(&rgb, roi).unwrap();
        let plane = 48 * 320;
        assert_eq!(tensor.len(), TITLE_INPUT_VALUES);
        assert_eq!(tensor[0].to_bits(), (-1.0_f32).to_bits());
        assert!((tensor[plane] - (128.0 / 127.5 - 1.0)).abs() < f32::EPSILON);
        assert_eq!(tensor[plane * 2].to_bits(), 1.0_f32.to_bits());
        assert_eq!(tensor[319].to_bits(), (-1.0_f32).to_bits());
    }

    #[test]
    fn preprocessor_reproduces_registered_opencv_linear_resize() {
        let roi = super::super::CanonicalLayout::load().unwrap().result.title;
        let mut rgb = Vec::with_capacity(roi.width as usize * roi.height as usize * 3);
        for y in 0..roi.height {
            for x in 0..roi.width {
                rgb.extend_from_slice(&[
                    u8::try_from((x * 17 + y * 29) % 256).unwrap(),
                    u8::try_from((x * 7 + y * 31) % 256).unwrap(),
                    u8::try_from((x * 13 + y * 11) % 256).unwrap(),
                ]);
            }
        }
        let tensor = preprocess_title_crop(&rgb, roi).unwrap();
        let resized = resize_linear_rgb(&rgb, 600, 50, 320, 48);
        assert_eq!(&resized[..6], &[8, 3, 6, 40, 17, 30]);
        assert_eq!(&resized[resized.len() - 3..], &[76, 76, 128]);
        let bytes: Vec<_> = tensor.into_iter().flat_map(f32::to_le_bytes).collect();
        assert_eq!(
            super::super::encode_sha256(&bytes),
            "856899b96510ffc8450a78328bb2527b3cacd8c886a4c58a54f41e5ed73f867d"
        );
    }

    #[test]
    fn wide_music_list_crop_uses_the_complete_registered_input_width() {
        let rgb = vec![255_u8; 475 * 45 * 3];
        let tensor = preprocess_title_image(&rgb, 475, 45).unwrap();
        let plane = 48 * 320;
        assert_eq!(tensor.len(), TITLE_INPUT_VALUES);
        assert_eq!(tensor[319].to_bits(), 1.0_f32.to_bits());
        assert_eq!(tensor[plane + 319].to_bits(), 1.0_f32.to_bits());
        assert_eq!(tensor[plane * 2 + 319].to_bits(), 1.0_f32.to_bits());
    }

    #[test]
    fn dynamic_music_list_upscale_matches_registered_opencv_linear_resize() {
        let mut rgb = Vec::with_capacity(475 * 45 * 3);
        for y in 0..45_u32 {
            for x in 0..475_u32 {
                rgb.extend_from_slice(&[
                    u8::try_from((x * 17 + y * 29) % 256).unwrap(),
                    u8::try_from((x * 7 + y * 31) % 256).unwrap(),
                    u8::try_from((x * 13 + y * 11) % 256).unwrap(),
                ]);
            }
        }
        let resized = resize_linear_rgb(&rgb, 475, 45, 506, 48);
        assert_eq!(
            super::super::encode_sha256(&resized),
            "3517280a382663fa282e91240319f1550895dadbcddfa42213af6c2d0b0bccbd"
        );
        let input = preprocess_dynamic_title_image(&rgb, 475, 45).unwrap();
        assert_eq!(input.width, 506);
        assert_eq!(input.values.len(), 3 * 48 * 506);
        let bytes: Vec<_> = input
            .values
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        assert_eq!(
            super::super::encode_sha256(&bytes),
            "a0c0e995661b0aeec61288ff0b97a42ec73223bce4d5a42a8bf13bbd640e78a1"
        );
    }

    #[test]
    fn dynamic_preprocessor_retains_the_registered_minimum_width() {
        let input = preprocess_dynamic_title_image(&vec![255; 100 * 48 * 3], 100, 48).unwrap();
        assert_eq!(input.width, 320);
        let plane = 48 * 320;
        assert_eq!(input.values[99].to_bits(), 1.0_f32.to_bits());
        assert_eq!(input.values[100].to_bits(), 0.0_f32.to_bits());
        assert_eq!(input.values[plane + 99].to_bits(), 1.0_f32.to_bits());
        assert_eq!(input.values[plane * 2 + 99].to_bits(), 1.0_f32.to_bits());
    }
}
