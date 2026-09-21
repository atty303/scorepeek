use std::sync::OnceLock;

use imageproc::image::GrayImage;
use imageproc::template_matching::{MatchTemplateMethod, match_template};

use super::{RecognitionError, Roi, encode_sha256};

const MUSIC_REFERENCE_QOI: &[u8] =
    include_bytes!("../../assets/screen-references-v1/music-select.qoi");
const MODE_REFERENCE_QOI: &[u8] =
    include_bytes!("../../assets/screen-references-v1/mode-select.qoi");
const SCORE_SCALE: f32 = 1_000_000.0;

static REFERENCES: OnceLock<Option<ScreenReferences>> = OnceLock::new();

struct ScreenReferences {
    music: GrayImage,
    mode_select: GrayImage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReferenceScores {
    pub music_ppm: u32,
    pub mode_select_ppm: u32,
}

pub(super) struct ReferenceContract<'a> {
    pub search_roi: Roi,
    pub template_width: u32,
    pub template_height: u32,
    pub music_asset_sha256: &'a str,
    pub mode_asset_sha256: &'a str,
}

pub(super) fn score(
    canonical_rgb8: &[u8],
    contract: &ReferenceContract<'_>,
) -> Result<ReferenceScores, RecognitionError> {
    let references = REFERENCES
        .get_or_init(|| ScreenReferences::decode().ok())
        .as_ref()
        .ok_or(RecognitionError::InvalidCanonicalLayout)?;
    if references.music.width() != contract.template_width
        || references.music.height() != contract.template_height
        || references.mode_select.width() != contract.template_width
        || references.mode_select.height() != contract.template_height
        || encode_sha256(MUSIC_REFERENCE_QOI) != contract.music_asset_sha256
        || encode_sha256(MODE_REFERENCE_QOI) != contract.mode_asset_sha256
    {
        return Err(RecognitionError::InvalidCanonicalLayout);
    }
    let search = gray_crop(canonical_rgb8, contract.search_roi)?;
    Ok(ReferenceScores {
        music_ppm: maximum_score_ppm(&search, &references.music),
        mode_select_ppm: maximum_score_ppm(&search, &references.mode_select),
    })
}

impl ScreenReferences {
    fn decode() -> Result<Self, RecognitionError> {
        Ok(Self {
            music: decode_qoi_gray(MUSIC_REFERENCE_QOI)?,
            mode_select: decode_qoi_gray(MODE_REFERENCE_QOI)?,
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

fn gray_crop(canonical_rgb8: &[u8], roi: Roi) -> Result<GrayImage, RecognitionError> {
    let rgb = super::crop_canonical_pixels(canonical_rgb8, roi)?;
    GrayImage::from_raw(roi.width, roi.height, rgb_to_gray(&rgb))
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
    reason = "the rounded normalized score is explicitly clamped to the complete u32 ppm domain"
)]
fn maximum_score_ppm(search: &GrayImage, template: &GrayImage) -> u32 {
    match_template(
        search,
        template,
        MatchTemplateMethod::CrossCorrelationNormalized,
    )
    .pixels()
    .map(|pixel| pixel[0])
    .fold(0.0_f32, f32::max)
    .mul_add(SCORE_SCALE, 0.5)
    .clamp(0.0, SCORE_SCALE) as u32
}
