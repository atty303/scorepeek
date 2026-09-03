use serde::{Deserialize, Serialize};

use super::{RecognitionError, Rgb8Crop, Roi};

pub const MUSIC_SELECT_BEST_LAYOUT: &[u8] = include_bytes!("../music-select-best-layout-v1.json");

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum BestValue<T> {
    Known(T),
    NoRecord,
    NotDisplayed,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BestClearType {
    NoPlay,
    Failed,
    AssistClear,
    EasyClear,
    Clear,
    HardClear,
    ExHardClear,
    FullCombo,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MusicSelectBestValues {
    pub score: BestValue<u32>,
    pub miss_count: BestValue<u32>,
    pub clear_type: BestValue<BestClearType>,
}

impl MusicSelectBestValues {
    #[must_use]
    pub fn has_observed_value(&self) -> bool {
        self.score != BestValue::Unknown
            || self.miss_count != BestValue::Unknown
            || self.clear_type != BestValue::Unknown
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MusicSelectBestObservation {
    pub header_text: String,
    pub clear_text: String,
    pub numeric: BestNumericObservation,
    pub values: MusicSelectBestValues,
    pub failures: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct BestNumericObservation {
    pub score: BestValue<u32>,
    pub miss_count: BestValue<u32>,
    pub cell_classes: Vec<String>,
    pub minimum_margins_milli: Vec<i32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MusicSelectBestLayout {
    pub schema: String,
    pub header: Roi,
    pub clear_type: Roi,
    pub score: Roi,
    pub miss_count: Roi,
    pub cell_width: u32,
    pub cell_pitch: u32,
    pub minimum_logit_margin_milli: i32,
    pub numeric_minimum_channel: u8,
    pub numeric_maximum_channel_difference: u8,
}

impl MusicSelectBestLayout {
    /// # Errors
    /// Returns an error if the embedded layout is invalid.
    pub fn load() -> Result<Self, RecognitionError> {
        let layout: Self = serde_json::from_slice(MUSIC_SELECT_BEST_LAYOUT)
            .map_err(|_| RecognitionError::InvalidCanonicalLayout)?;
        if layout.schema != "scorepeek-music-select-best-layout-v1"
            || layout.cell_width == 0
            || layout.cell_pitch < layout.cell_width
            || layout.score.width != layout.cell_pitch * 3 + layout.cell_width
            || layout.miss_count.width != layout.score.width
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(layout)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectBestCrops {
    pub header: Rgb8Crop,
    pub clear_type: Rgb8Crop,
    pub score: Rgb8Crop,
    pub miss_count: Rgb8Crop,
}

impl MusicSelectBestCrops {
    /// # Errors
    /// Rejects pixels outside the canonical RGB8 contract or an invalid layout.
    pub fn extract(pixels: &[u8]) -> Result<Self, RecognitionError> {
        let layout = MusicSelectBestLayout::load()?;
        let crop = |roi| -> Result<Rgb8Crop, RecognitionError> {
            Ok(Rgb8Crop {
                roi,
                pixels: super::crop_canonical_pixels(pixels, roi)?,
            })
        };
        Ok(Self {
            header: crop(layout.header)?,
            clear_type: crop(layout.clear_type)?,
            score: crop(layout.score)?,
            miss_count: crop(layout.miss_count)?,
        })
    }

    pub(super) fn numeric_cells(&self) -> Result<Vec<Rgb8Crop>, RecognitionError> {
        let layout = MusicSelectBestLayout::load()?;
        let mut cells = Vec::with_capacity(8);
        for source in [&self.score, &self.miss_count] {
            for slot in 0..4 {
                let x = slot * layout.cell_pitch;
                let mut pixels = Vec::new();
                for y in 0..source.roi.height {
                    let start = ((y * source.roi.width + x) * 3) as usize;
                    pixels.extend_from_slice(
                        &source.pixels()[start..start + (layout.cell_width * 3) as usize],
                    );
                }
                // SELECT uses dim zero placeholders. They must not become numeric evidence
                // when the RESULT model normalizes contrast within a cell.
                for pixel in pixels.chunks_exact_mut(3) {
                    let low = *pixel.iter().min().unwrap();
                    let high = *pixel.iter().max().unwrap();
                    if low < layout.numeric_minimum_channel
                        || high - low > layout.numeric_maximum_channel_difference
                    {
                        pixel.fill(0);
                    }
                }
                cells.push(Rgb8Crop {
                    roi: Roi {
                        x: source.roi.x + x,
                        width: layout.cell_width,
                        ..source.roi
                    },
                    pixels,
                });
            }
        }
        Ok(cells)
    }

    pub(super) fn miss_dashes(&self) -> bool {
        let crop = &self.miss_count;
        // Four independently measured 16x2 neutral dashes, with no numeral-height foreground.
        let mut rows = vec![0_u32; crop.roi.height as usize];
        let mut slots = [0_u32; 4];
        for y in 0..crop.roi.height {
            for x in 0..crop.roi.width {
                let i = ((y * crop.roi.width + x) * 3) as usize;
                let p = &crop.pixels()[i..i + 3];
                let low = *p.iter().min().unwrap();
                let high = *p.iter().max().unwrap();
                if low >= 180 && high - low <= 50 {
                    rows[y as usize] += 1;
                    slots[(x / 22).min(3) as usize] += 1;
                }
            }
        }
        rows.iter().enumerate().all(|(y, n)| {
            if (7..=10).contains(&y) {
                *n <= 68
            } else {
                *n == 0
            }
        }) && slots.iter().all(|n| (28..=36).contains(n))
    }
}

#[must_use]
pub fn resolve_music_select_best(
    header_text: String,
    clear_text: String,
    numeric: BestNumericObservation,
) -> MusicSelectBestObservation {
    let clear = match clear_text.as_str() {
        "NO PLAY" => Some(BestClearType::NoPlay),
        "FAILED" => Some(BestClearType::Failed),
        "ASSIST CLEAR" => Some(BestClearType::AssistClear),
        "EASY CLEAR" => Some(BestClearType::EasyClear),
        "CLEAR" => Some(BestClearType::Clear),
        "HARD CLEAR" => Some(BestClearType::HardClear),
        "EX HARD CLEAR" => Some(BestClearType::ExHardClear),
        "FULLCOMBO CLEAR" => Some(BestClearType::FullCombo),
        _ => None,
    };
    let values = if header_text == "SCORE DATA" {
        MusicSelectBestValues {
            score: numeric.score.clone(),
            miss_count: numeric.miss_count.clone(),
            clear_type: clear.map_or(BestValue::Unknown, BestValue::Known),
        }
    } else {
        MusicSelectBestValues::default()
    };
    MusicSelectBestObservation {
        header_text,
        clear_text,
        numeric,
        values,
        failures: Vec::new(),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StableBestField<T> {
    pub observed: BestValue<T>,
    pub consecutive: u8,
    pub observed_once: bool,
}

impl<T> Default for StableBestField<T> {
    fn default() -> Self {
        Self {
            observed: BestValue::Unknown,
            consecutive: 0,
            observed_once: false,
        }
    }
}

impl<T: Clone + Eq> StableBestField<T> {
    pub fn observe(&mut self, observed: BestValue<T>) {
        self.observed_once = true;
        self.consecutive = if observed == BestValue::Unknown {
            0
        } else if self.observed == observed {
            self.consecutive.saturating_add(1).min(2)
        } else {
            1
        };
        self.observed = observed;
    }

    #[must_use]
    pub fn accepted(&self) -> BestValue<T> {
        if self.consecutive >= 2 {
            self.observed.clone()
        } else {
            BestValue::Unknown
        }
    }
}

/// Derived from EX SCORE and the resolved chart's maximum EX SCORE; never OCR evidence.
#[must_use]
pub fn dj_rank(score: u32, notes: u32) -> Option<&'static str> {
    if notes == 0 || u64::from(score) > u64::from(notes) * 2 {
        return None;
    }
    let band = u64::from(score) * 9 / (u64::from(notes) * 2);
    Some(match band {
        8.. => "AAA",
        7 => "AA",
        6 => "A",
        5 => "B",
        4 => "C",
        3 => "D",
        2 => "E",
        _ => "F",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_and_value_changes_restart_consecutive_evidence() {
        let mut field = StableBestField::default();
        field.observe(BestValue::Known(1234));
        assert_eq!(field.accepted(), BestValue::Unknown);
        field.observe(BestValue::Known(1234));
        assert_eq!(field.accepted(), BestValue::Known(1234));
        field.observe(BestValue::Unknown);
        field.observe(BestValue::Known(1234));
        assert_eq!(field.accepted(), BestValue::Unknown);
        field.observe(BestValue::Known(999));
        assert_eq!(field.consecutive, 1);
        field.observe(BestValue::NoRecord);
        field.observe(BestValue::NoRecord);
        assert_eq!(field.accepted(), BestValue::NoRecord);
    }

    #[test]
    fn panel_header_is_required_and_blank_ocr_is_not_no_play() {
        let numeric = BestNumericObservation {
            score: BestValue::Known(0),
            miss_count: BestValue::NoRecord,
            ..BestNumericObservation::default()
        };
        assert!(
            !resolve_music_select_best(String::new(), "NO PLAY".into(), numeric.clone())
                .values
                .has_observed_value()
        );
        let blank = resolve_music_select_best("SCORE DATA".into(), String::new(), numeric.clone());
        assert_eq!(blank.values.clear_type, BestValue::Unknown);
        assert_eq!(blank.values.score, BestValue::Known(0));
        assert_eq!(
            resolve_music_select_best("SCORE DATA".into(), "NO PLAY".into(), numeric)
                .values
                .clear_type,
            BestValue::Known(BestClearType::NoPlay)
        );
    }

    #[test]
    fn derived_rank_has_exact_integer_boundaries_and_rejects_impossible_scores() {
        for (band, rank) in [
            (2, "E"),
            (3, "D"),
            (4, "C"),
            (5, "B"),
            (6, "A"),
            (7, "AA"),
            (8, "AAA"),
        ] {
            assert_eq!(dj_rank(band * 200, 900), Some(rank));
            assert_ne!(dj_rank(band * 200 - 1, 900), Some(rank));
        }
        assert_eq!(dj_rank(1800, 900), Some("AAA"));
        assert_eq!(dj_rank(0, 900), Some("F"));
        assert_eq!(dj_rank(1801, 900), None);
        assert_eq!(dj_rank(0, 0), None);
        assert_eq!(dj_rank(u32::MAX, u32::MAX), Some("C"));
    }
}
