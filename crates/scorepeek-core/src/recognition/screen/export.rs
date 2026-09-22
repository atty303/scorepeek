use super::{
    CanonicalLayout, IntegratedContextLayout, MusicSelectBestCrops,
    MusicSelectDifficultyMarkerCrops, MusicSelectPlaySideCrops, MusicSelectScreenRgb8Crops,
    NumericField, RecognitionError, ResultPanelSide, ResultScreenRgb8Crops, Rgb8Crop, Roi,
    ScreenCropRoute, ScreenPathLayout, ScreenRgb8Crops, TitleScreenRgb8Crops,
    crop_canonical_pixels,
};

pub(super) fn horizontal_edge_pixels(pixels: &[u8], width: u32) -> u32 {
    let row_bytes = width as usize * 3;
    pixels[..row_bytes]
        .chunks_exact(3)
        .zip(pixels[row_bytes..].chunks_exact(3))
        .filter(|(upper, lower)| {
            let luma = |pixel: &[u8]| {
                (u32::from(pixel[0]) * 77 + u32::from(pixel[1]) * 150 + u32::from(pixel[2]) * 29)
                    / 256
            };
            luma(upper).abs_diff(luma(lower)) > 45
        })
        .fold(0, |count, _| count + 1)
}

fn crop_rgb8(pixels: &[u8], roi: Roi) -> Result<Rgb8Crop, RecognitionError> {
    Ok(Rgb8Crop {
        roi,
        pixels: crop_canonical_pixels(pixels, roi)?,
    })
}

fn route_result_rgb8_crops(
    pixels: &[u8],
    canonical: &CanonicalLayout,
    context: &IntegratedContextLayout,
    panel_side: ResultPanelSide,
) -> Result<ResultScreenRgb8Crops, RecognitionError> {
    let origin_x = canonical.result.panel_origins.get(panel_side);
    let panel = |roi: Roi| roi.translated_x(origin_x);
    let numeric = |field, roi: Roi| {
        roi.translated_x(
            canonical
                .result
                .numeric_panel_origins
                .get(panel_side, field),
        )
    };
    Ok(ResultScreenRgb8Crops {
        canonical_layout_sha256: CanonicalLayout::sha256(),
        panel_side,
        title: crop_rgb8(pixels, canonical.result.title)?,
        artist: crop_rgb8(pixels, context.result.artist)?,
        clear_type: crop_rgb8(pixels, panel(canonical.result.clear_type)?)?,
        difficulty: crop_rgb8(pixels, canonical.result.difficulty)?,
        play_type: crop_rgb8(pixels, context.result.play_type)?,
        level: crop_rgb8(pixels, canonical.result.level)?,
        notes: crop_rgb8(pixels, canonical.result.notes)?,
        current_score: crop_rgb8(
            pixels,
            numeric(NumericField::CurrentScore, canonical.result.current_score)?,
        )?,
        previous_clear_type: crop_rgb8(pixels, panel(canonical.result.previous_clear_type)?)?,
        previous_score: crop_rgb8(
            pixels,
            numeric(NumericField::PreviousScore, canonical.result.previous_score)?,
        )?,
        previous_miss_count: crop_rgb8(
            pixels,
            numeric(
                NumericField::PreviousMissCount,
                canonical.result.previous_miss_count,
            )?,
        )?,
        miss_count: crop_rgb8(
            pixels,
            numeric(NumericField::MissCount, canonical.result.miss_count)?,
        )?,
        pgreat: crop_rgb8(
            pixels,
            numeric(NumericField::Pgreat, canonical.result.pgreat)?,
        )?,
        great: crop_rgb8(
            pixels,
            numeric(NumericField::Great, canonical.result.great)?,
        )?,
        good: crop_rgb8(pixels, numeric(NumericField::Good, canonical.result.good)?)?,
        bad: crop_rgb8(pixels, numeric(NumericField::Bad, canonical.result.bad)?)?,
        poor: crop_rgb8(pixels, numeric(NumericField::Poor, canonical.result.poor)?)?,
        fast: crop_rgb8(pixels, numeric(NumericField::Fast, canonical.result.fast)?)?,
        slow: crop_rgb8(pixels, numeric(NumericField::Slow, canonical.result.slow)?)?,
        combo_break: crop_rgb8(
            pixels,
            numeric(NumericField::ComboBreak, canonical.result.combo_break)?,
        )?,
        play_options: crop_rgb8(pixels, panel(canonical.result.play_options)?)?,
    })
}

/// Routes one already-classified canonical RGB8 frame to all currently measured field crops for
/// that screen.
///
/// This function is synchronous, deterministic, and filesystem-free. Callers retain responsibility
/// for binding the result to capture provenance and for preventing `Unknown` from entering field
/// observation.
///
/// # Errors
/// Returns an error for an unknown screen, invalid canonical pixels, or layout drift.
pub fn route_screen_rgb8_crops(
    pixels: &[u8],
    route: ScreenCropRoute,
) -> Result<ScreenRgb8Crops, RecognitionError> {
    let canonical = CanonicalLayout::load()?;
    let context = IntegratedContextLayout::load()?;
    match route {
        ScreenCropRoute::Title => {
            let path = ScreenPathLayout::load()?;
            Ok(ScreenRgb8Crops::Title(TitleScreenRgb8Crops {
                canonical_layout_sha256: CanonicalLayout::sha256(),
                game_version: crop_rgb8(pixels, path.title.version)?,
            }))
        }
        ScreenCropRoute::Result(panel_side) => Ok(ScreenRgb8Crops::Result(
            route_result_rgb8_crops(pixels, &canonical, &context, panel_side)?,
        )),
        ScreenCropRoute::MusicSelect => {
            Ok(ScreenRgb8Crops::MusicSelect(MusicSelectScreenRgb8Crops {
                best: MusicSelectBestCrops::extract(pixels)?,
                canonical_layout_sha256: CanonicalLayout::sha256(),
                integrated_context_layout_sha256: IntegratedContextLayout::sha256(),
                central_title: crop_rgb8(pixels, canonical.music_select.selected_title)?,
                artist: crop_rgb8(pixels, context.music_select.artist)?,
                play_type: crop_rgb8(pixels, context.music_select.play_type.roi)?,
                difficulty_markers: MusicSelectDifficultyMarkerCrops {
                    beginner: crop_rgb8(pixels, context.music_select.selected_difficulty.beginner)?,
                    normal: crop_rgb8(pixels, context.music_select.selected_difficulty.normal)?,
                    hyper: crop_rgb8(pixels, context.music_select.selected_difficulty.hyper)?,
                    another: crop_rgb8(pixels, context.music_select.selected_difficulty.another)?,
                    leggendaria: crop_rgb8(
                        pixels,
                        context.music_select.selected_difficulty.leggendaria,
                    )?,
                },
                play_side: MusicSelectPlaySideCrops {
                    one_player: crop_rgb8(pixels, context.music_select.play_side.one_player)?,
                    two_player: crop_rgb8(pixels, context.music_select.play_side.two_player)?,
                },
                active_list_title: crop_rgb8(pixels, context.music_select.active_list_title)?,
            }))
        }
    }
}
