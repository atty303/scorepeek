use super::{RecognitionError, Roi};

pub const TITLE_PREPROCESSOR_ID: &str = "paddlex-3.7.0-bgr-rec-resize-3x48x320-v1";
pub(super) const TITLE_INPUT_SHAPE: [usize; 4] = [1, 3, 48, 320];
pub(super) const TITLE_INPUT_VALUES: usize = 3 * 48 * 320;

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
    let source_width = roi.width as usize;
    let source_height = roi.height as usize;
    let resized_height = TITLE_INPUT_SHAPE[2];
    let resized_width = (resized_height * source_width).div_ceil(source_height);
    if resized_width > TITLE_INPUT_SHAPE[3] {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }

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

fn resize_linear_rgb(
    source: &[u8],
    source_width: usize,
    source_height: usize,
    target_width: usize,
    target_height: usize,
) -> Vec<u8> {
    let mut output = vec![0_u8; target_width * target_height * 3];
    let horizontal: Vec<_> = (0..target_width)
        .map(|x| interpolation_axis(x, source_width, target_width))
        .collect();
    let vertical: Vec<_> = (0..target_height)
        .map(|y| interpolation_axis(y, source_height, target_height))
        .collect();
    for (target_y, &(source_y, vertical_weights)) in vertical.iter().enumerate() {
        let next_y = (source_y + 1).min(source_height - 1);
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

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn interpolation_axis(target: usize, source_size: usize, target_size: usize) -> (usize, [i32; 2]) {
    // These casts reproduce OpenCV's registered double-to-float and float-to-fixed-point path.
    // All callers use the fixed 600x100 to 288x48 title geometry.
    const COEFFICIENT_SCALE: f32 = 2048.0;
    let coordinate = ((target as f64 + 0.5) * source_size as f64 / target_size as f64 - 0.5) as f32;
    if coordinate <= 0.0 {
        (0, [COEFFICIENT_SCALE as i32, 0])
    } else {
        let base = coordinate.floor() as usize;
        if base >= source_size - 1 {
            (source_size - 1, [COEFFICIENT_SCALE as i32, 0])
        } else {
            let fraction = coordinate - base as f32;
            (
                base,
                [
                    ((1.0 - fraction) * COEFFICIENT_SCALE).round_ties_even() as i32,
                    (fraction * COEFFICIENT_SCALE).round_ties_even() as i32,
                ],
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocessor_is_bgr_chw_normalized_and_right_padded() {
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
        assert_eq!(tensor[287].to_bits(), (-1.0_f32).to_bits());
        assert_eq!(tensor[288].to_bits(), 0.0_f32.to_bits());
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
        let resized = resize_linear_rgb(&rgb, 600, 100, 288, 48);
        assert_eq!(&resized[..6], &[25, 20, 13, 60, 35, 40]);
        assert_eq!(&resized[resized.len() - 3..], &[229, 73, 159]);
        let bytes: Vec<_> = tensor.into_iter().flat_map(f32::to_le_bytes).collect();
        assert_eq!(
            super::super::encode_sha256(&bytes),
            "978a4c52cb1a3644c2904f43ab5252e2fdfc76662eb9ce36ee88aed024649500"
        );
    }
}
