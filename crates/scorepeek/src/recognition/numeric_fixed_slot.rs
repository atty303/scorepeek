use crate::catalog::Difficulty;

use super::title_preprocessor::resize_linear_gray;
use super::{
    NumericField, RecognitionError, ResultNumericCharacterLayout, ResultScreenRgb8Crops, Rgb8Crop,
    Roi,
};

pub const FIXED_SLOT_PREPROCESSOR_ID: &str = "scorepeek-fixed-slot-hog-hybrid-0p25-v1";
pub const FIXED_SLOT_FEATURE_DIMENSIONS: usize = 2_244;

#[derive(Debug)]
pub struct FixedSlotFieldCells {
    pub field: NumericField,
    pub cells: Vec<FixedSlotCell>,
}

#[derive(Debug)]
pub struct FixedSlotCell {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

pub fn extract_fixed_slot_fields(
    crops: &ResultScreenRgb8Crops,
    difficulty: Option<Difficulty>,
) -> Result<Vec<FixedSlotFieldCells>, RecognitionError> {
    let layout = ResultNumericCharacterLayout::load()?;
    let mut output = Vec::new();
    if let Some(difficulty) = difficulty {
        for variant in layout.level_variants(difficulty) {
            output.push(FixedSlotFieldCells {
                field: NumericField::Level,
                cells: extract_cells(&crops.level, &variant.digit_cells)?,
            });
        }
    }
    for field in NumericField::ALL
        .into_iter()
        .filter(|field| *field != NumericField::Level)
    {
        let owner = numeric_crop(crops, field);
        let cells = layout
            .cells(field, None, None)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        output.push(FixedSlotFieldCells {
            field,
            cells: extract_cells(owner, cells)?,
        });
    }
    Ok(output)
}

#[must_use]
pub fn fixed_not_displayed_fields(crops: &ResultScreenRgb8Crops) -> Vec<NumericField> {
    [
        NumericField::PreviousScore,
        NumericField::PreviousMissCount,
        NumericField::MissCount,
    ]
    .into_iter()
    .filter(|field| has_not_displayed_marker(numeric_crop(crops, *field)))
    .collect()
}

fn has_not_displayed_marker(crop: &Rgb8Crop) -> bool {
    let width = crop.roi.width as usize;
    let height = crop.roi.height as usize;
    if height < 45 || width < 79 {
        return false;
    }
    let white = crop
        .pixels()
        .chunks_exact(3)
        .map(|pixel| {
            let low = pixel[0].min(pixel[1]).min(pixel[2]);
            let high = pixel[0].max(pixel[1]).max(pixel[2]);
            low >= 145 && high - low <= 70
        })
        .collect::<Vec<_>>();
    let mut row_counts = [0_usize; 17];
    let mut occupied_columns = vec![false; width];
    for (band_y, y) in (28..45).enumerate() {
        for (x, occupied) in occupied_columns.iter_mut().enumerate() {
            if white[y * width + x] {
                row_counts[band_y] += 1;
                *occupied = true;
            }
        }
    }
    let maximum_row = row_counts.into_iter().max().unwrap_or(0);
    let long_rows = row_counts.into_iter().filter(|count| *count >= 40).count();
    let occupied = occupied_columns.into_iter().filter(|value| *value).count();
    (70..=78).contains(&maximum_row)
        && (2..=3).contains(&long_rows)
        && (70..=78).contains(&occupied)
}

fn numeric_crop(crops: &ResultScreenRgb8Crops, field: NumericField) -> &Rgb8Crop {
    match field {
        NumericField::Level => &crops.level,
        NumericField::Notes => &crops.notes,
        NumericField::CurrentScore => &crops.current_score,
        NumericField::PreviousScore => &crops.previous_score,
        NumericField::PreviousMissCount => &crops.previous_miss_count,
        NumericField::MissCount => &crops.miss_count,
        NumericField::Pgreat => &crops.pgreat,
        NumericField::Great => &crops.great,
        NumericField::Good => &crops.good,
        NumericField::Bad => &crops.bad,
        NumericField::Poor => &crops.poor,
        NumericField::Fast => &crops.fast,
        NumericField::Slow => &crops.slow,
        NumericField::ComboBreak => &crops.combo_break,
    }
}

fn extract_cells(owner: &Rgb8Crop, cells: &[Roi]) -> Result<Vec<FixedSlotCell>, RecognitionError> {
    cells
        .iter()
        .map(|cell| extract_cell(owner, *cell))
        .collect()
}

fn extract_cell(owner: &Rgb8Crop, cell: Roi) -> Result<FixedSlotCell, RecognitionError> {
    let x = cell
        .x
        .checked_sub(owner.roi.x)
        .ok_or(RecognitionError::InvalidCanonicalLayout)? as usize;
    let y = cell
        .y
        .checked_sub(owner.roi.y)
        .ok_or(RecognitionError::InvalidCanonicalLayout)? as usize;
    let width = cell.width as usize;
    let height = cell.height as usize;
    let owner_width = owner.roi.width as usize;
    let owner_height = owner.roi.height as usize;
    if x + width > owner_width || y + height > owner_height {
        return Err(RecognitionError::InvalidCanonicalLayout);
    }
    let mut output = Vec::with_capacity(width * height * 3);
    for row in y..y + height {
        let start = (row * owner_width + x) * 3;
        output.extend_from_slice(
            owner
                .pixels()
                .get(start..start + width * 3)
                .ok_or(RecognitionError::InvalidCanonicalFrame)?,
        );
    }
    Ok(FixedSlotCell {
        width,
        height,
        pixels: output,
    })
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the registered ONNX tensor deliberately narrows normalized f64 features to f32"
)]
pub fn fixed_slot_feature(
    rgb: &[u8],
    width: usize,
    height: usize,
    field: NumericField,
) -> Result<Vec<f32>, RecognitionError> {
    if width == 0 || height == 0 || rgb.len() != width.saturating_mul(height).saturating_mul(3) {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    let hard = hard_mask(rgb, width, height, field);
    let soft = soft_mask(rgb, width, height, field);
    let hard = resize_linear_gray(&hard, width, height, 24, 32)?;
    let soft = resize_linear_gray(&soft, width, height, 24, 32)?;
    let coarse = normalized_hog(&hard, 24, 32, 8)?;
    let mut fine = normalized_hog(&soft, 24, 32, 4)?;
    let mut pixels = soft
        .iter()
        .map(|value| f64::from(*value) / 255.0)
        .collect::<Vec<_>>();
    normalize(&mut pixels);
    fine.extend(pixels);
    normalize(&mut fine);
    let mut combined = Vec::with_capacity(FIXED_SLOT_FEATURE_DIMENSIONS);
    combined.extend(coarse);
    combined.extend(fine.into_iter().map(|value| value * 0.25));
    normalize(&mut combined);
    if combined.len() != FIXED_SLOT_FEATURE_DIMENSIONS {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    Ok(combined.into_iter().map(|value| value as f32).collect())
}

fn hard_mask(rgb: &[u8], width: usize, height: usize, field: NumericField) -> Vec<u8> {
    let selected = rgb
        .chunks_exact(3)
        .map(|pixel| {
            let red = i32::from(pixel[0]);
            let green = i32::from(pixel[1]);
            let blue = i32::from(pixel[2]);
            let selected = if field == NumericField::Level {
                red >= 180 && green >= 70 && blue <= 120 && red - blue >= 80
            } else if matches!(field, NumericField::CurrentScore | NumericField::MissCount) {
                blue >= 150 && green >= 130 && red <= 170 && blue - red >= 35
            } else {
                let low = red.min(green).min(blue);
                let high = red.max(green).max(blue);
                low >= 145 && high - low <= 70
            };
            u8::from(selected) * 255
        })
        .collect::<Vec<_>>();
    erode_2x2(&dilate_2x2(&selected, width, height), width, height)
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the clamped registered mask contract deliberately quantizes to u8"
)]
fn soft_mask(rgb: &[u8], _width: usize, _height: usize, field: NumericField) -> Vec<u8> {
    rgb.chunks_exact(3)
        .map(|pixel| {
            let red = f64::from(pixel[0]);
            let green = f64::from(pixel[1]);
            let blue = f64::from(pixel[2]);
            let selected = if field == NumericField::Level {
                ((red - blue) / 160.0).clamp(0.0, 1.0) * ((red - 80.0) / 175.0).clamp(0.0, 1.0)
            } else if matches!(field, NumericField::CurrentScore | NumericField::MissCount) {
                ((((blue + green) * 0.5) - red) / 100.0).clamp(0.0, 1.0)
                    * ((blue.min(green) - 70.0) / 185.0).clamp(0.0, 1.0)
            } else {
                let low = red.min(green).min(blue);
                let high = red.max(green).max(blue);
                ((low - 70.0) / 185.0).clamp(0.0, 1.0)
                    * ((110.0 - (high - low)) / 110.0).clamp(0.0, 1.0)
            };
            (selected * 255.0).round_ties_even().clamp(0.0, 255.0) as u8
        })
        .collect()
}

fn dilate_2x2(input: &[u8], width: usize, height: usize) -> Vec<u8> {
    morphology_2x2(input, width, height, true)
}

fn erode_2x2(input: &[u8], width: usize, height: usize) -> Vec<u8> {
    morphology_2x2(input, width, height, false)
}

fn morphology_2x2(input: &[u8], width: usize, height: usize, dilate: bool) -> Vec<u8> {
    let mut output = vec![0; input.len()];
    for y in 0..height {
        for x in 0..width {
            let mut value = if dilate { 0 } else { 255 };
            for dy in 0..2_isize {
                for dx in 0..2_isize {
                    let source_x = isize::try_from(x).unwrap_or(isize::MAX) + dx - 1;
                    let source_y = isize::try_from(y).unwrap_or(isize::MAX) + dy - 1;
                    let sample = usize::try_from(source_x)
                        .ok()
                        .zip(usize::try_from(source_y).ok())
                        .filter(|(source_x, source_y)| *source_x < width && *source_y < height)
                        .map_or(if dilate { 0 } else { 255 }, |(source_x, source_y)| {
                            input[source_y * width + source_x]
                        });
                    value = if dilate {
                        value.max(sample)
                    } else {
                        value.min(sample)
                    };
                }
            }
            output[y * width + x] = value;
        }
    }
    output
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "the reference HOG contract accumulates each bin as float32"
)]
fn normalized_hog(
    image: &[u8],
    width: usize,
    height: usize,
    pixels_per_cell: usize,
) -> Result<Vec<f64>, RecognitionError> {
    let cells_x = width / pixels_per_cell;
    let cells_y = height / pixels_per_cell;
    if cells_x < 2 || cells_y < 2 || image.len() != width.saturating_mul(height) {
        return Err(RecognitionError::InvalidCanonicalFrame);
    }
    // scikit-image 0.26.0 intentionally accumulates each cell/bin total in float32 even when the
    // gradient arrays are float64. Preserve that narrowing and its row-major addition order.
    let mut histogram = vec![0.0_f32; cells_x * cells_y * 9];
    for y in 0..height {
        for x in 0..width {
            let row = if y == 0 || y + 1 == height {
                0.0
            } else {
                f64::from(image[(y + 1) * width + x]) - f64::from(image[(y - 1) * width + x])
            };
            let column = if x == 0 || x + 1 == width {
                0.0
            } else {
                f64::from(image[y * width + x + 1]) - f64::from(image[y * width + x - 1])
            };
            let magnitude = row.hypot(column);
            let column_index = x / pixels_per_cell;
            let row_index = y / pixels_per_cell;
            if column_index < cells_x && row_index < cells_y {
                let orientation = row.atan2(column).to_degrees().rem_euclid(180.0);
                for direction in 0_u8..9 {
                    let lower = 20.0 * f64::from(direction);
                    let upper = lower + 20.0;
                    if lower <= orientation && orientation < upper {
                        let index =
                            (row_index * cells_x + column_index) * 9 + usize::from(direction);
                        histogram[index] = (f64::from(histogram[index]) + magnitude) as f32;
                    }
                }
            }
        }
    }
    let mut output = Vec::with_capacity((cells_x - 1) * (cells_y - 1) * 36);
    for block_y in 0..cells_y - 1 {
        for block_x in 0..cells_x - 1 {
            let mut block = Vec::with_capacity(36);
            for cell_y in block_y..block_y + 2 {
                for cell_x in block_x..block_x + 2 {
                    let start = (cell_y * cells_x + cell_x) * 9;
                    let pixel_count = u32::try_from(pixels_per_cell * pixels_per_cell)
                        .expect("registered HOG cells fit u32");
                    block.extend(
                        histogram[start..start + 9]
                            .iter()
                            .map(|value| f64::from(*value) / f64::from(pixel_count)),
                    );
                }
            }
            l2_hys(&mut block);
            output.extend(block);
        }
    }
    normalize(&mut output);
    Ok(output)
}

fn l2_hys(values: &mut [f64]) {
    const EPSILON: f64 = 1e-5;
    let norm = (values.iter().map(|value| value * value).sum::<f64>() + EPSILON * EPSILON).sqrt();
    for value in values.iter_mut() {
        *value = (*value / norm).min(0.2);
    }
    let norm = (values.iter().map(|value| value * value).sum::<f64>() + EPSILON * EPSILON).sqrt();
    for value in values {
        *value /= norm;
    }
}

fn normalize(values: &mut [f64]) {
    let norm = values.iter().map(|value| value * value).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in values {
            *value /= norm;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_slot_feature_has_registered_shape_and_finite_values() {
        let mut rgb = Vec::new();
        for y in 0..22_usize {
            for x in 0..27_usize {
                let value = if (x / 3 + y / 4) % 2 == 0 { 230 } else { 20 };
                rgb.extend_from_slice(&[value; 3]);
            }
        }
        assert_eq!(
            super::super::encode_sha256(&hard_mask(&rgb, 27, 22, NumericField::Pgreat)),
            "04e2a96647d744fc1b3992c879cfe536f424dd3191dd7600dd0cf1d50d63bac1"
        );
        assert_eq!(
            super::super::encode_sha256(&soft_mask(&rgb, 27, 22, NumericField::Pgreat)),
            "d711599e6e4da839b7bf49b3de9c22d7e090f11d32c5b9b6a9e24edde8fb02bf"
        );
        let hard_resized = resize_linear_gray(
            &hard_mask(&rgb, 27, 22, NumericField::Pgreat),
            27,
            22,
            24,
            32,
        )
        .unwrap();
        let soft_resized = resize_linear_gray(
            &soft_mask(&rgb, 27, 22, NumericField::Pgreat),
            27,
            22,
            24,
            32,
        )
        .unwrap();
        assert_eq!(
            super::super::encode_sha256(&hard_resized),
            "5218a105aabdd46c57388448979d1b81f8b4b5d678c295a3c28203fbcfa9b651"
        );
        assert_eq!(
            super::super::encode_sha256(&soft_resized),
            "59c5e8a9bac8a8703c2660e1a80e857d7cd009df59d4b1c9a456ca64da1abe52"
        );
        let coarse = normalized_hog(&hard_resized, 24, 32, 8).unwrap();
        let coarse_bytes = coarse
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(
            super::super::encode_sha256(&coarse_bytes),
            "1718672f978cbde7280cc09a569bfbbbce9bb24f7e0ed744f3e7917a8750241f"
        );
        let feature = fixed_slot_feature(&rgb, 27, 22, NumericField::Pgreat).unwrap();
        for (observed, expected) in feature.iter().zip([
            0.155_899_58_f32,
            0.014_865_789,
            0.011_239_049,
            0.024_396_664,
            0.075_816_94,
        ]) {
            assert!((*observed - expected).abs() < 1e-7);
        }
        assert_eq!(feature.len(), FIXED_SLOT_FEATURE_DIMENSIONS);
        assert!(feature.iter().all(|value| value.is_finite()));
        let norm = feature
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
        let bytes = feature
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<_>>();
        assert_eq!(
            super::super::encode_sha256(&bytes),
            "c293e0cbf9c4bbe6e3ba4d9d8d7d3a38539bf54b8373c28e266403059a0d1451"
        );
    }
}
