use super::*;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultPresenceEvidence {
    pub warm_pixels: u32,
    pub warm_pixels_min: u32,
    pub panel_side: ResultPanelSideState,
    pub panels: [ResultPanelPresenceEvidence; 2],
    pub horizontal_edge_pixels_min: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct TitlePresenceEvidence {
    pub bright_bbox: Option<Roi>,
    pub bright_channel_min: u8,
    pub qualifies: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultPanelPresenceEvidence {
    pub panel_side: ResultPanelSide,
    pub upper_panel_edge_pixels: u32,
    pub lower_panel_edge_pixels: u32,
    pub qualifies: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectPresenceEvidence {
    pub cyan_header_pixels: u32,
    pub cyan_header_pixels_min: u32,
    pub colored_level_pixels: u32,
    pub colored_level_pixels_min: u32,
    pub bright_label_pixels: u32,
    pub bright_label_pixels_min: u32,
    pub reference_evaluated: bool,
    pub music_reference_score_ppm: u32,
    pub mode_select_reference_score_ppm: u32,
    pub reference_score_min_ppm: u32,
    pub reference_winner_margin_min_ppm: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct DecideTransitionPresenceEvidence {
    pub cyan_pixels: u32,
    pub cyan_pixels_min: u32,
    pub bright_pixels: u32,
    pub bright_pixels_min: u32,
    pub saturated_pixels: u32,
    pub saturated_pixels_min: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayPresenceEvidence {
    pub qualifying_candidates: u8,
    pub top_edge_runs: u8,
    pub bottom_edge_runs: u8,
    pub candidates: [Option<PlayBpmEdgePairEvidence>; 2],
    pub top_edge_pixels_min: u32,
    pub top_edge_pixels_max: u32,
    pub bottom_edge_pixels_min: u32,
    pub bottom_edge_pixels_max: u32,
    pub vertical_distance_min: u32,
    pub vertical_distance_max: u32,
    pub edge_center_delta_x2_max: u32,
    pub candidate_cluster_delta_x2_max: u32,
    pub candidate_cluster_delta_y_max: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayBpmEdgePairEvidence {
    pub center_x2: u32,
    pub top_y: u32,
    pub top_edge_pixels: u32,
    pub bottom_y: u32,
    pub bottom_edge_pixels: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PlayCyanRun {
    x: u32,
    y: u32,
    pixels: u32,
}

impl CanonicalLayout {
    /// Loads the scorepeek-owned shared game layout embedded in the runtime.
    ///
    /// # Errors
    /// Returns an error when the committed artifact is malformed or outside the canonical frame.
    pub fn load() -> Result<Self, RecognitionError> {
        let layout: Self = serde_json::from_slice(LAYOUT_BYTES)?;
        if layout.schema != LAYOUT_SCHEMA
            || layout.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
            || layout.width != CANONICAL_WIDTH
            || layout.height != CANONICAL_HEIGHT
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        layout.validate_result()?;
        layout.validate_music_select()?;
        Ok(layout)
    }

    fn validate_result(&self) -> Result<(), RecognitionError> {
        for roi in [
            self.result.header,
            self.result.title,
            self.result.artist,
            self.result.difficulty,
            self.result.level,
            self.result.notes,
        ] {
            roi.validate(self.width, self.height)?;
        }
        for side in [ResultPanelSide::Left, ResultPanelSide::Right] {
            let origin_x = self.result.panel_origins.get(side);
            for roi in [
                self.result.upper_panel_edge,
                self.result.lower_panel_edge,
                self.result.clear_type,
                self.result.previous_clear_type,
                self.result.play_options,
            ] {
                roi.translated_x(origin_x)?
                    .validate(self.width, self.height)?;
            }
            for (field, roi) in [
                (NumericField::CurrentScore, self.result.current_score),
                (NumericField::PreviousScore, self.result.previous_score),
                (
                    NumericField::PreviousMissCount,
                    self.result.previous_miss_count,
                ),
                (NumericField::MissCount, self.result.miss_count),
                (NumericField::Pgreat, self.result.pgreat),
                (NumericField::Great, self.result.great),
                (NumericField::Good, self.result.good),
                (NumericField::Bad, self.result.bad),
                (NumericField::Poor, self.result.poor),
                (NumericField::Fast, self.result.fast),
                (NumericField::Slow, self.result.slow),
                (NumericField::ComboBreak, self.result.combo_break),
            ] {
                roi.translated_x(self.result.numeric_panel_origins.get(side, field))?
                    .validate(self.width, self.height)?;
            }
        }
        let header_pixels = self
            .result
            .header
            .width
            .checked_mul(self.result.header.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if self.result.presence.warm_pixels_min == 0
            || self.result.presence.horizontal_edge_pixels_min == 0
            || self.result.presence.warm_pixels_min > header_pixels
            || self.result.panel_origins.left != 0
            || self.result.panel_origins.right != 1_360
            || self.result.numeric_panel_origins.left.score != 0
            || self.result.numeric_panel_origins.left.judgment != 0
            || self.result.numeric_panel_origins.left.timing != 0
            || self.result.numeric_panel_origins.left.combo_break != 0
            || self.result.upper_panel_edge.height != 2
            || self.result.lower_panel_edge.height != 2
            || self.result.presence.horizontal_edge_pixels_min > self.result.upper_panel_edge.width
            || self.result.presence.horizontal_edge_pixels_min > self.result.lower_panel_edge.width
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(())
    }

    fn validate_music_select(&self) -> Result<(), RecognitionError> {
        for roi in [
            self.music_select.header,
            self.music_select.label,
            self.music_select.level_column,
            self.music_select.selected_title,
        ] {
            roi.validate(self.width, self.height)?;
        }
        for roi in self.music_select.list_titles.rois() {
            roi.validate(self.width, self.height)?;
        }
        let header_pixels = self
            .music_select
            .header
            .width
            .checked_mul(self.music_select.header.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let level_pixels = self
            .music_select
            .level_column
            .width
            .checked_mul(self.music_select.level_column.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let label_pixels = self
            .music_select
            .label
            .width
            .checked_mul(self.music_select.label.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if self.music_select.list_titles.slots == 0
            || self.music_select.list_titles.stride_y == 0
            || self.music_select.presence.cyan_header_pixels_min == 0
            || self.music_select.presence.colored_level_pixels_min == 0
            || self.music_select.presence.bright_label_pixels_min == 0
            || self.music_select.presence.cyan_header_pixels_min > header_pixels
            || self.music_select.presence.colored_level_pixels_min > level_pixels
            || self.music_select.presence.bright_label_pixels_min > label_pixels
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(())
    }

    #[must_use]
    pub fn sha256() -> String {
        encode_sha256(LAYOUT_BYTES)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ScreenPathLayout {
    schema: String,
    canonical_frame_contract_id: String,
    width: u32,
    height: u32,
    pub(super) title: TitleLayout,
    music_select_reference: MusicSelectReferenceLayout,
    pub(super) decide_transition: DecideTransitionLayout,
    pub(super) play: PlayLayout,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TitleLayout {
    pub(super) version: Roi,
    presence: TitlePresencePredicate,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct MusicSelectReferenceLayout {
    algorithm_id: String,
    search_roi: Roi,
    template_width: u32,
    template_height: u32,
    music_asset_sha256: String,
    mode_asset_sha256: String,
    score_min_ppm: u32,
    winner_margin_min_ppm: u32,
}

impl ScreenPathLayout {
    pub(super) fn load() -> Result<Self, RecognitionError> {
        let layout: Self = serde_json::from_slice(SCREEN_PATH_LAYOUT_BYTES)?;
        if layout.schema != SCREEN_PATH_LAYOUT_SCHEMA
            || layout.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
            || layout.width != CANONICAL_WIDTH
            || layout.height != CANONICAL_HEIGHT
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        for roi in [
            layout.title.version,
            layout.music_select_reference.search_roi,
            layout.decide_transition.splash,
            layout.play.bpm_outline_search,
        ] {
            roi.validate(layout.width, layout.height)?;
        }
        let title = layout.title.presence;
        if title.bright_channel_min == 0
            || title.bbox_x_min > title.bbox_x_max
            || title.bbox_y_min > title.bbox_y_max
            || title.bbox_width_min == 0
            || title.bbox_width_min > title.bbox_width_max
            || title.bbox_height_min == 0
            || title.bbox_height_min > title.bbox_height_max
            || title.bbox_x_max >= layout.title.version.width
            || title.bbox_y_max >= layout.title.version.height
            || title.bbox_width_max > layout.title.version.width
            || title.bbox_height_max > layout.title.version.height
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        let decide_pixels = layout
            .decide_transition
            .splash
            .width
            .checked_mul(layout.decide_transition.splash.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        let reference = &layout.music_select_reference;
        if reference.algorithm_id != "imageproc-cross-correlation-normalized-gray8-v1"
            || reference.template_width == 0
            || reference.template_height == 0
            || reference.template_width > reference.search_roi.width
            || reference.template_height > reference.search_roi.height
            || reference.music_asset_sha256.len() != 64
            || reference.mode_asset_sha256.len() != 64
            || reference.score_min_ppm == 0
            || reference.score_min_ppm > 1_000_000
            || reference.winner_margin_min_ppm == 0
            || reference.winner_margin_min_ppm > 1_000_000
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        let play = layout.play.presence;
        if layout.decide_transition.presence.cyan_pixels_min == 0
            || layout.decide_transition.presence.bright_pixels_min == 0
            || layout.decide_transition.presence.saturated_pixels_min == 0
            || layout.decide_transition.presence.cyan_pixels_min > decide_pixels
            || layout.decide_transition.presence.bright_pixels_min > decide_pixels
            || layout.decide_transition.presence.saturated_pixels_min > decide_pixels
            || play.top_edge_pixels_min == 0
            || play.top_edge_pixels_min > play.top_edge_pixels_max
            || play.bottom_edge_pixels_min == 0
            || play.bottom_edge_pixels_min > play.bottom_edge_pixels_max
            || play.vertical_distance_min == 0
            || play.vertical_distance_min > play.vertical_distance_max
            || play.edge_center_delta_x2_max == 0
            || play.candidate_cluster_delta_x2_max < play.edge_center_delta_x2_max
            || play.candidate_cluster_delta_y_max == 0
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        let play_search = layout.play.bpm_outline_search;
        let pixels = play_search
            .width
            .checked_mul(play_search.height)
            .ok_or(RecognitionError::InvalidCanonicalLayout)?;
        if pixels > 256_000
            || play.top_edge_pixels_max > play_search.width
            || play.bottom_edge_pixels_max > play_search.width
            || play.vertical_distance_max >= play_search.height
            || play.edge_center_delta_x2_max > play_search.width * 2
            || play.candidate_cluster_delta_x2_max > play_search.width * 2
            || play.candidate_cluster_delta_y_max > play_search.height
        {
            return Err(RecognitionError::InvalidCanonicalLayout);
        }
        Ok(layout)
    }

    fn sha256() -> String {
        encode_sha256(SCREEN_PATH_LAYOUT_BYTES)
    }
}

fn is_play_cyan(pixel: &[u8]) -> bool {
    let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
    g >= 30 && b >= 40 && u16::from(b) * 2 > u16::from(r) * 3 && b > g
}

fn play_cyan_runs(pixels: &[u8], roi: Roi) -> Vec<PlayCyanRun> {
    let mut runs = Vec::new();
    for y in roi.y..roi.y + roi.height {
        let mut x = roi.x;
        while x < roi.x + roi.width {
            let index = (y as usize * CANONICAL_WIDTH as usize + x as usize) * 3;
            if !is_play_cyan(&pixels[index..index + 3]) {
                x += 1;
                continue;
            }
            let start = x;
            x += 1;
            while x < roi.x + roi.width {
                let index = (y as usize * CANONICAL_WIDTH as usize + x as usize) * 3;
                if !is_play_cyan(&pixels[index..index + 3]) {
                    break;
                }
                x += 1;
            }
            runs.push(PlayCyanRun {
                x: start,
                y,
                pixels: x - start,
            });
        }
    }
    runs
}

impl PlayCyanRun {
    const fn center_x2(self) -> u32 {
        self.x * 2 + self.pixels - 1
    }
}

fn play_presence_evidence(
    pixels: &[u8],
    roi: Roi,
    predicate: PlayPresencePredicate,
) -> PlayPresenceEvidence {
    let runs = play_cyan_runs(pixels, roi);
    let top_runs = runs
        .iter()
        .copied()
        .filter(|run| {
            (predicate.top_edge_pixels_min..=predicate.top_edge_pixels_max).contains(&run.pixels)
        })
        .collect::<Vec<_>>();
    let bottom_runs = runs
        .iter()
        .copied()
        .filter(|run| {
            (predicate.bottom_edge_pixels_min..=predicate.bottom_edge_pixels_max)
                .contains(&run.pixels)
        })
        .collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for top in &top_runs {
        for bottom in &bottom_runs {
            let Some(vertical_distance) = bottom.y.checked_sub(top.y) else {
                continue;
            };
            if !(predicate.vertical_distance_min..=predicate.vertical_distance_max)
                .contains(&vertical_distance)
                || top.center_x2().abs_diff(bottom.center_x2()) > predicate.edge_center_delta_x2_max
            {
                continue;
            }
            pairs.push(PlayBpmEdgePairEvidence {
                center_x2: u32::midpoint(top.center_x2(), bottom.center_x2()),
                top_y: top.y,
                top_edge_pixels: top.pixels,
                bottom_y: bottom.y,
                bottom_edge_pixels: bottom.pixels,
            });
        }
    }
    pairs.sort_unstable_by_key(|pair| (pair.center_x2, pair.top_y, pair.bottom_y));
    let mut candidates = [None, None];
    let mut candidate_count = 0_u8;
    let mut cluster_anchor = None;
    for pair in pairs {
        let same_cluster = cluster_anchor.is_some_and(|anchor: PlayBpmEdgePairEvidence| {
            pair.center_x2.abs_diff(anchor.center_x2) <= predicate.candidate_cluster_delta_x2_max
                && pair.top_y.abs_diff(anchor.top_y) <= predicate.candidate_cluster_delta_y_max
                && pair.bottom_y.abs_diff(anchor.bottom_y)
                    <= predicate.candidate_cluster_delta_y_max
        });
        if same_cluster {
            continue;
        }
        if let Some(slot) = candidates.get_mut(usize::from(candidate_count)) {
            *slot = Some(pair);
        }
        candidate_count = candidate_count.saturating_add(1);
        cluster_anchor = Some(pair);
    }
    PlayPresenceEvidence {
        qualifying_candidates: candidate_count,
        top_edge_runs: u8::try_from(top_runs.len()).unwrap_or(u8::MAX),
        bottom_edge_runs: u8::try_from(bottom_runs.len()).unwrap_or(u8::MAX),
        candidates,
        top_edge_pixels_min: predicate.top_edge_pixels_min,
        top_edge_pixels_max: predicate.top_edge_pixels_max,
        bottom_edge_pixels_min: predicate.bottom_edge_pixels_min,
        bottom_edge_pixels_max: predicate.bottom_edge_pixels_max,
        vertical_distance_min: predicate.vertical_distance_min,
        vertical_distance_max: predicate.vertical_distance_max,
        edge_center_delta_x2_max: predicate.edge_center_delta_x2_max,
        candidate_cluster_delta_x2_max: predicate.candidate_cluster_delta_x2_max,
        candidate_cluster_delta_y_max: predicate.candidate_cluster_delta_y_max,
    }
}

fn result_panel_presence(
    pixels: &[u8],
    layout: &ResultLayout,
    side: ResultPanelSide,
) -> Result<ResultPanelPresenceEvidence, RecognitionError> {
    let origin_x = layout.panel_origins.get(side);
    let upper_panel_edge_pixels = horizontal_edge_pixels(
        &crop_canonical_pixels(pixels, layout.upper_panel_edge.translated_x(origin_x)?)?,
        layout.upper_panel_edge.width,
    );
    let lower_panel_edge_pixels = horizontal_edge_pixels(
        &crop_canonical_pixels(pixels, layout.lower_panel_edge.translated_x(origin_x)?)?,
        layout.lower_panel_edge.width,
    );
    Ok(ResultPanelPresenceEvidence {
        panel_side: side,
        upper_panel_edge_pixels,
        lower_panel_edge_pixels,
        qualifies: upper_panel_edge_pixels >= layout.presence.horizontal_edge_pixels_min
            && lower_panel_edge_pixels >= layout.presence.horizontal_edge_pixels_min,
    })
}

fn title_presence_evidence(
    pixels: &[u8],
    layout: &TitleLayout,
) -> Result<TitlePresenceEvidence, RecognitionError> {
    let crop = crop_canonical_pixels(pixels, layout.version)?;
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (index, pixel) in crop.chunks_exact(3).enumerate() {
        if pixel
            .iter()
            .all(|channel| *channel > layout.presence.bright_channel_min)
        {
            let index =
                u32::try_from(index).map_err(|_| RecognitionError::InvalidCanonicalFrame)?;
            let x = index % layout.version.width;
            let y = index / layout.version.width;
            bounds = Some(bounds.map_or((x, y, x, y), |(min_x, min_y, max_x, max_y)| {
                (min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y))
            }));
        }
    }
    let bright_bbox = bounds.map(|(min_x, min_y, max_x, max_y)| Roi {
        x: min_x,
        y: min_y,
        width: max_x - min_x + 1,
        height: max_y - min_y + 1,
    });
    let qualifies = bright_bbox.is_some_and(|bbox| {
        (layout.presence.bbox_x_min..=layout.presence.bbox_x_max).contains(&bbox.x)
            && (layout.presence.bbox_y_min..=layout.presence.bbox_y_max).contains(&bbox.y)
            && (layout.presence.bbox_width_min..=layout.presence.bbox_width_max)
                .contains(&bbox.width)
            && (layout.presence.bbox_height_min..=layout.presence.bbox_height_max)
                .contains(&bbox.height)
    });
    Ok(TitlePresenceEvidence {
        bright_bbox,
        bright_channel_min: layout.presence.bright_channel_min,
        qualifies,
    })
}

/// Applies only the embedded screen predicates to one fixed-contract canonical RGB8 slice.
///
/// This pure primitive deliberately carries no capture, generation, normalizer, extraction, or
/// model authority. Application code must combine it with a source-bound canonical owner before
/// the result can enter a diagnostic run or later acceptance logic.
///
/// # Errors
/// Returns an error when the pixels do not satisfy the fixed canonical byte contract or the
/// committed layout is invalid.
#[allow(
    clippy::too_many_lines,
    reason = "all independent predicates remain together so classification is based on one measurement pass"
)]
pub fn inspect_canonical_rgb8(
    pixels: &[u8],
) -> Result<ScreenPredicateObservation, RecognitionError> {
    let frame = crate::frame::CanonicalFrameView::new(pixels)?;
    inspect_canonical_frame(frame)
}

/// Inspects a frame already admitted to the fixed canonical RGB8 contract.
///
/// # Errors
/// Returns an error when the committed layout or its crop is invalid.
#[allow(clippy::too_many_lines)]
pub fn inspect_canonical_frame(
    frame: crate::frame::CanonicalFrameView<'_>,
) -> Result<ScreenPredicateObservation, RecognitionError> {
    let pixels = frame.pixels();
    let layout = CanonicalLayout::load()?;
    let screen_path_layout = ScreenPathLayout::load()?;
    let title_presence = title_presence_evidence(pixels, &screen_path_layout.title)?;
    let header = crop_canonical_pixels(pixels, layout.result.header)?;
    let mut warm = 0_u32;
    for pixel in header.chunks_exact(3) {
        let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
        if r > 100 && g > 70 && b < 170 && r >= g && g >= b {
            warm += 1;
        }
    }
    let result_panels = [
        result_panel_presence(pixels, &layout.result, ResultPanelSide::Left)?,
        result_panel_presence(pixels, &layout.result, ResultPanelSide::Right)?,
    ];
    let qualifying_result_panels = result_panels
        .iter()
        .filter(|panel| panel.qualifies)
        .collect::<Vec<_>>();
    let result_panel_side = match qualifying_result_panels.as_slice() {
        [panel] => ResultPanelSideState::Known(panel.panel_side),
        [] => ResultPanelSideState::Unknown(ResultPanelSideUnknownReason::NoCandidate),
        [_, _, ..] => {
            ResultPanelSideState::Unknown(ResultPanelSideUnknownReason::MultipleCandidates)
        }
    };
    let music_header = crop_canonical_pixels(pixels, layout.music_select.header)?;
    let cyan_header_pixels = music_header
        .chunks_exact(3)
        .filter(|pixel| {
            let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
            g > 120 && b > 150 && u16::from(b) * 2 > u16::from(r) * 3
        })
        .fold(0_u32, |count, _| count + 1);
    let level_column = crop_canonical_pixels(pixels, layout.music_select.level_column)?;
    let colored_level_pixels = level_column
        .chunks_exact(3)
        .filter(|pixel| {
            let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
            let maximum = r.max(g).max(b);
            let minimum = r.min(g).min(b);
            maximum > 130 && maximum - minimum > 60
        })
        .fold(0_u32, |count, _| count + 1);
    let music_label = crop_canonical_pixels(pixels, layout.music_select.label)?;
    let bright_label_pixels = music_label
        .chunks_exact(3)
        .filter(|pixel| pixel[0] > 178 && pixel[1] > 178 && pixel[2] > 178)
        .fold(0_u32, |count, _| count + 1);
    let decide_splash = crop_canonical_pixels(pixels, screen_path_layout.decide_transition.splash)?;
    let mut decide_cyan_pixels = 0_u32;
    let mut decide_bright_pixels = 0_u32;
    let mut decide_saturated_pixels = 0_u32;
    for pixel in decide_splash.chunks_exact(3) {
        let [r, g, b] = [pixel[0], pixel[1], pixel[2]];
        if g > 120 && b > 150 && u16::from(b) * 2 > u16::from(r) * 3 {
            decide_cyan_pixels += 1;
        }
        if r > 178 && g > 178 && b > 178 {
            decide_bright_pixels += 1;
        }
        if r.max(g).max(b) > 130 && r.max(g).max(b) - r.min(g).min(b) > 60 {
            decide_saturated_pixels += 1;
        }
    }
    let play_presence = play_presence_evidence(
        pixels,
        screen_path_layout.play.bpm_outline_search,
        screen_path_layout.play.presence,
    );
    let result_present =
        warm >= layout.result.presence.warm_pixels_min && result_panel_side.known().is_some();
    let aggregate_music_select_present = cyan_header_pixels
        >= layout.music_select.presence.cyan_header_pixels_min
        && colored_level_pixels >= layout.music_select.presence.colored_level_pixels_min
        && bright_label_pixels >= layout.music_select.presence.bright_label_pixels_min;
    let reference_scores = aggregate_music_select_present
        .then(|| {
            screen_reference::score(
                pixels,
                &screen_reference::ReferenceContract {
                    search_roi: screen_path_layout.music_select_reference.search_roi,
                    template_width: screen_path_layout.music_select_reference.template_width,
                    template_height: screen_path_layout.music_select_reference.template_height,
                    music_asset_sha256: &screen_path_layout
                        .music_select_reference
                        .music_asset_sha256,
                    mode_asset_sha256: &screen_path_layout.music_select_reference.mode_asset_sha256,
                },
            )
        })
        .transpose()?;
    let music_select_present = reference_scores.is_some_and(|scores| {
        scores.music_ppm >= screen_path_layout.music_select_reference.score_min_ppm
            && scores.music_ppm.saturating_sub(scores.mode_select_ppm)
                >= screen_path_layout
                    .music_select_reference
                    .winner_margin_min_ppm
    });
    let mode_select_present = reference_scores.is_some_and(|scores| {
        scores.mode_select_ppm >= screen_path_layout.music_select_reference.score_min_ppm
            && scores.mode_select_ppm.saturating_sub(scores.music_ppm)
                >= screen_path_layout
                    .music_select_reference
                    .winner_margin_min_ppm
    });
    let decide_transition_present = decide_cyan_pixels
        >= screen_path_layout
            .decide_transition
            .presence
            .cyan_pixels_min
        && decide_bright_pixels
            >= screen_path_layout
                .decide_transition
                .presence
                .bright_pixels_min
        && decide_saturated_pixels
            >= screen_path_layout
                .decide_transition
                .presence
                .saturated_pixels_min;
    let play_present = play_presence.qualifying_candidates == 1;
    let screen = match [
        (result_present, ScreenClass::Result),
        (music_select_present, ScreenClass::MusicSelect),
        (mode_select_present, ScreenClass::ModeSelect),
        (decide_transition_present, ScreenClass::DecideTransition),
        (play_present, ScreenClass::Play),
    ]
    .into_iter()
    .filter_map(|(present, screen)| present.then_some(screen))
    .collect::<Vec<_>>()
    .as_slice()
    {
        [screen] => *screen,
        [] | [_, _, ..] => ScreenClass::Unknown,
    };
    Ok(ScreenPredicateObservation {
        screen_path_layout_sha256: ScreenPathLayout::sha256(),
        screen,
        title_presence,
        result_presence: ResultPresenceEvidence {
            warm_pixels: warm,
            warm_pixels_min: layout.result.presence.warm_pixels_min,
            panel_side: result_panel_side,
            panels: result_panels,
            horizontal_edge_pixels_min: layout.result.presence.horizontal_edge_pixels_min,
        },
        music_select_presence: MusicSelectPresenceEvidence {
            cyan_header_pixels,
            cyan_header_pixels_min: layout.music_select.presence.cyan_header_pixels_min,
            colored_level_pixels,
            colored_level_pixels_min: layout.music_select.presence.colored_level_pixels_min,
            bright_label_pixels,
            bright_label_pixels_min: layout.music_select.presence.bright_label_pixels_min,
            reference_evaluated: reference_scores.is_some(),
            music_reference_score_ppm: reference_scores.map_or(0, |scores| scores.music_ppm),
            mode_select_reference_score_ppm: reference_scores
                .map_or(0, |scores| scores.mode_select_ppm),
            reference_score_min_ppm: screen_path_layout.music_select_reference.score_min_ppm,
            reference_winner_margin_min_ppm: screen_path_layout
                .music_select_reference
                .winner_margin_min_ppm,
        },
        decide_transition_presence: DecideTransitionPresenceEvidence {
            cyan_pixels: decide_cyan_pixels,
            cyan_pixels_min: screen_path_layout
                .decide_transition
                .presence
                .cyan_pixels_min,
            bright_pixels: decide_bright_pixels,
            bright_pixels_min: screen_path_layout
                .decide_transition
                .presence
                .bright_pixels_min,
            saturated_pixels: decide_saturated_pixels,
            saturated_pixels_min: screen_path_layout
                .decide_transition
                .presence
                .saturated_pixels_min,
        },
        play_presence,
    })
}
