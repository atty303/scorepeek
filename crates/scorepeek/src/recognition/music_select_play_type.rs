use std::sync::OnceLock;

use imageproc::image::GrayImage;
use imageproc::template_matching::{MatchTemplateMethod, match_template};
use serde::Serialize;

use crate::catalog::PlayType;

use super::{IntegratedContextLayout, RecognitionError, Rgb8Crop, encode_sha256};

const SINGLE_REFERENCE_QOI: &[u8] =
    include_bytes!("../../assets/music-select-play-type-v1/single.qoi");
const DOUBLE_REFERENCE_QOI: &[u8] =
    include_bytes!("../../assets/music-select-play-type-v1/double.qoi");
const SCORE_SCALE: f32 = 1_000_000.0;

static REFERENCES: OnceLock<Option<PlayTypeReferences>> = OnceLock::new();

struct PlayTypeReferences {
    single: GrayImage,
    double: GrayImage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicSelectPlayTypeUnknownReason {
    ScoreTooLow,
    InsufficientMargin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum MusicSelectPlayTypeState {
    Known(PlayType),
    Unknown(MusicSelectPlayTypeUnknownReason),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectPlayTypeObservation {
    pub algorithm_id: &'static str,
    pub state: MusicSelectPlayTypeState,
    pub single_score_ppm: u32,
    pub double_score_ppm: u32,
    pub score_min_ppm: u32,
    pub winner_margin_min_ppm: u32,
}

impl Default for MusicSelectPlayTypeObservation {
    fn default() -> Self {
        Self {
            algorithm_id: "imageproc-cross-correlation-normalized-gray8-v1",
            state: MusicSelectPlayTypeState::Unknown(MusicSelectPlayTypeUnknownReason::ScoreTooLow),
            single_score_ppm: 0,
            double_score_ppm: 0,
            score_min_ppm: 980_000,
            winner_margin_min_ppm: 20_000,
        }
    }
}

impl MusicSelectPlayTypeObservation {
    #[must_use]
    pub const fn known(&self) -> Option<PlayType> {
        match self.state {
            MusicSelectPlayTypeState::Known(value) => Some(value),
            MusicSelectPlayTypeState::Unknown(_) => None,
        }
    }
}

/// Classifies the fixed MUSIC SELECT mode badge using only registered templates.
///
/// # Errors
/// Returns an error if the embedded layout or template assets violate their immutable contract.
pub fn observe_music_select_play_type(
    crop: &Rgb8Crop,
) -> Result<MusicSelectPlayTypeObservation, RecognitionError> {
    let layout = IntegratedContextLayout::load()?;
    let contract = &layout.music_select.play_type;
    if crop.roi != contract.roi {
        return Err(RecognitionError::InvalidCanonicalLayout);
    }
    let references = REFERENCES
        .get_or_init(|| PlayTypeReferences::decode().ok())
        .as_ref()
        .ok_or(RecognitionError::InvalidCanonicalLayout)?;
    if references.single.width() != contract.template_width
        || references.single.height() != contract.template_height
        || references.double.width() != contract.template_width
        || references.double.height() != contract.template_height
        || encode_sha256(SINGLE_REFERENCE_QOI) != contract.single_asset_sha256
        || encode_sha256(DOUBLE_REFERENCE_QOI) != contract.double_asset_sha256
    {
        return Err(RecognitionError::InvalidCanonicalLayout);
    }
    let observed = GrayImage::from_raw(crop.roi.width, crop.roi.height, rgb_to_gray(crop.pixels()))
        .ok_or(RecognitionError::InvalidCanonicalLayout)?;
    let single_score_ppm = score_ppm(&observed, &references.single);
    let double_score_ppm = score_ppm(&observed, &references.double);
    let winner = single_score_ppm.max(double_score_ppm);
    let margin = single_score_ppm.abs_diff(double_score_ppm);
    let state = if winner < contract.score_min_ppm {
        MusicSelectPlayTypeState::Unknown(MusicSelectPlayTypeUnknownReason::ScoreTooLow)
    } else if margin < contract.winner_margin_min_ppm {
        MusicSelectPlayTypeState::Unknown(MusicSelectPlayTypeUnknownReason::InsufficientMargin)
    } else if single_score_ppm > double_score_ppm {
        MusicSelectPlayTypeState::Known(PlayType::Single)
    } else {
        MusicSelectPlayTypeState::Known(PlayType::Double)
    };
    Ok(MusicSelectPlayTypeObservation {
        algorithm_id: "imageproc-cross-correlation-normalized-gray8-v1",
        state,
        single_score_ppm,
        double_score_ppm,
        score_min_ppm: contract.score_min_ppm,
        winner_margin_min_ppm: contract.winner_margin_min_ppm,
    })
}

impl PlayTypeReferences {
    fn decode() -> Result<Self, RecognitionError> {
        Ok(Self {
            single: decode_qoi_gray(SINGLE_REFERENCE_QOI)?,
            double: decode_qoi_gray(DOUBLE_REFERENCE_QOI)?,
        })
    }
}

fn decode_qoi_gray(encoded: &[u8]) -> Result<GrayImage, RecognitionError> {
    let (header, rgb) =
        qoi::decode_to_vec(encoded).map_err(|_| RecognitionError::InvalidCanonicalLayout)?;
    if rgb.len() != header.width as usize * header.height as usize * 3 {
        return Err(RecognitionError::InvalidCanonicalLayout);
    }
    GrayImage::from_raw(header.width, header.height, rgb_to_gray(&rgb))
        .ok_or(RecognitionError::InvalidCanonicalLayout)
}

fn rgb_to_gray(rgb: &[u8]) -> Vec<u8> {
    rgb.chunks_exact(3)
        .map(|pixel| {
            let weighted =
                u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29;
            u8::try_from(weighted >> 8).unwrap_or(u8::MAX)
        })
        .collect()
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the rounded normalized score is clamped to the complete u32 ppm domain"
)]
fn score_ppm(observed: &GrayImage, template: &GrayImage) -> u32 {
    match_template(
        observed,
        template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    )
    .pixels()
    .next()
    .map_or(0.0, |pixel| pixel[0])
    .mul_add(SCORE_SCALE, 0.5)
    .clamp(0.0, SCORE_SCALE) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_crop(encoded: &[u8]) -> Rgb8Crop {
        let (header, pixels) = qoi::decode_to_vec(encoded).unwrap();
        Rgb8Crop {
            roi: super::IntegratedContextLayout::load()
                .unwrap()
                .music_select
                .play_type
                .roi,
            pixels: {
                assert_eq!(header.width, 100);
                assert_eq!(header.height, 80);
                pixels
            },
        }
    }

    #[test]
    fn registered_select_badges_resolve_both_play_types() {
        let single = observe_music_select_play_type(&reference_crop(SINGLE_REFERENCE_QOI)).unwrap();
        let double = observe_music_select_play_type(&reference_crop(DOUBLE_REFERENCE_QOI)).unwrap();
        assert_eq!(single.known(), Some(PlayType::Single));
        assert_eq!(double.known(), Some(PlayType::Double));
    }

    #[test]
    fn unrelated_badge_pixels_fail_closed() {
        let roi = IntegratedContextLayout::load()
            .unwrap()
            .music_select
            .play_type
            .roi;
        let crop = Rgb8Crop {
            roi,
            pixels: vec![0; roi.width as usize * roi.height as usize * 3],
        };
        assert!(matches!(
            observe_music_select_play_type(&crop).unwrap().state,
            MusicSelectPlayTypeState::Unknown(_)
        ));
    }
}
