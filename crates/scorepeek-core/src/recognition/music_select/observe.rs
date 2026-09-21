//! Music-select screen observation contract and fixed-layout predicates.

use serde::{Deserialize, Serialize};

use crate::catalog::Difficulty;
use crate::recognition::title::DynamicTextObservation;

use super::super::screen::{IntegratedContextLayout, Rgb8Crop};
use super::{MusicSelectBestCrops, MusicSelectBestObservation, MusicSelectPlayTypeObservation};

/// Canonical regions whose motion must be reviewed separately before music-select dwell is
/// calibrated.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MusicSelectMotionRegions {
    pub list_titles: crate::frame::Roi,
    pub active_list_title: crate::frame::Roi,
    pub central_title: crate::frame::Roi,
}

/// Every currently measured music-select field crop used by one selection observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectScreenRgb8Crops {
    pub best: MusicSelectBestCrops,
    pub canonical_layout_sha256: String,
    pub integrated_context_layout_sha256: String,
    pub central_title: Rgb8Crop,
    pub artist: Rgb8Crop,
    pub play_type: Rgb8Crop,
    pub difficulty_markers: MusicSelectDifficultyMarkerCrops,
    pub play_side: MusicSelectPlaySideCrops,
    pub active_list_title: Rgb8Crop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectPlaySideCrops {
    pub(in crate::recognition) one_player: Rgb8Crop,
    pub(in crate::recognition) two_player: Rgb8Crop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectDifficultyMarkerCrops {
    pub(in crate::recognition) beginner: Rgb8Crop,
    pub(in crate::recognition) normal: Rgb8Crop,
    pub(in crate::recognition) hyper: Rgb8Crop,
    pub(in crate::recognition) another: Rgb8Crop,
    pub(in crate::recognition) leggendaria: Rgb8Crop,
}

impl MusicSelectDifficultyMarkerCrops {
    #[must_use]
    pub fn as_slots(&self) -> [(Difficulty, &Rgb8Crop); 5] {
        [
            (Difficulty::Beginner, &self.beginner),
            (Difficulty::Normal, &self.normal),
            (Difficulty::Hyper, &self.hyper),
            (Difficulty::Another, &self.another),
            (Difficulty::Leggendaria, &self.leggendaria),
        ]
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicSelectDifficultyUnknownReason {
    NoCandidate,
    MultipleCandidates,
    InsufficientMargin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum MusicSelectDifficultyState {
    Known(Difficulty),
    Unknown(MusicSelectDifficultyUnknownReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectDifficultyMarkerEvidence {
    pub difficulty: Difficulty,
    pub top_edge_ppm: u32,
    pub bottom_edge_ppm: u32,
    pub score_ppm: u32,
    pub qualifies: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectDifficultyObservation {
    pub predicate_id: &'static str,
    pub state: MusicSelectDifficultyState,
    pub winner_score_ppm: u32,
    pub runner_up_score_ppm: u32,
    pub margin_ppm: u32,
    pub slots: [MusicSelectDifficultyMarkerEvidence; 5],
}

impl MusicSelectDifficultyObservation {
    #[must_use]
    pub const fn known(&self) -> Option<Difficulty> {
        match self.state {
            MusicSelectDifficultyState::Known(value) => Some(value),
            MusicSelectDifficultyState::Unknown(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaySide {
    OnePlayer,
    TwoPlayer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicSelectPlaySideUnknownReason {
    NoCandidate,
    MultipleCandidates,
    InsufficientMargin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum MusicSelectPlaySideState {
    Known(PlaySide),
    Unknown(MusicSelectPlaySideUnknownReason),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectPlaySideEvidence {
    pub play_side: PlaySide,
    pub bright_pixels: u32,
    pub qualifies: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectPlaySideObservation {
    pub predicate_id: &'static str,
    pub state: MusicSelectPlaySideState,
    pub winner_bright_pixels: u32,
    pub runner_up_bright_pixels: u32,
    pub margin: u32,
    pub sides: [MusicSelectPlaySideEvidence; 2],
}

impl Default for MusicSelectPlaySideObservation {
    fn default() -> Self {
        let sides =
            [PlaySide::OnePlayer, PlaySide::TwoPlayer].map(|value| MusicSelectPlaySideEvidence {
                play_side: value,
                bright_pixels: 0,
                qualifies: false,
            });
        Self {
            predicate_id: "scorepeek-music-select-footer-brightness-v1",
            state: MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate),
            winner_bright_pixels: 0,
            runner_up_bright_pixels: 0,
            margin: 0,
            sides,
        }
    }
}

impl MusicSelectPlaySideObservation {
    #[must_use]
    pub const fn known(&self) -> Option<PlaySide> {
        match self.state {
            MusicSelectPlaySideState::Known(value) => Some(value),
            MusicSelectPlaySideState::Unknown(_) => None,
        }
    }
}

#[doc(hidden)]
pub fn test_music_select_play_side(play_side: Option<PlaySide>) -> MusicSelectPlaySideObservation {
    MusicSelectPlaySideObservation {
        state: play_side.map_or(
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate),
            MusicSelectPlaySideState::Known,
        ),
        ..MusicSelectPlaySideObservation::default()
    }
}

#[cfg(test)]
pub(crate) fn test_music_select_difficulty(
    difficulty: Option<Difficulty>,
) -> MusicSelectDifficultyObservation {
    let slots = [
        Difficulty::Beginner,
        Difficulty::Normal,
        Difficulty::Hyper,
        Difficulty::Another,
        Difficulty::Leggendaria,
    ]
    .map(|value| MusicSelectDifficultyMarkerEvidence {
        difficulty: value,
        top_edge_ppm: 0,
        bottom_edge_ppm: 0,
        score_ppm: 0,
        qualifies: false,
    });
    MusicSelectDifficultyObservation {
        predicate_id: "scorepeek-player-marker-outline-v2",
        state: difficulty.map_or(
            MusicSelectDifficultyState::Unknown(MusicSelectDifficultyUnknownReason::NoCandidate),
            MusicSelectDifficultyState::Known,
        ),
        winner_score_ppm: 0,
        runner_up_score_ppm: 0,
        margin_ppm: 0,
        slots,
    }
}

/// Complete music-select field observations from the currently registered observers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectScreenFieldObservations {
    pub best: MusicSelectBestObservation,
    pub central_title: DynamicTextObservation,
    pub artist: DynamicTextObservation,
    pub play_type: MusicSelectPlayTypeObservation,
    pub selected_difficulty: MusicSelectDifficultyObservation,
    pub play_side: MusicSelectPlaySideObservation,
    pub active_list_title: DynamicTextObservation,
}

#[must_use]
/// Evaluates the five fixed `PLAYER 01` marker slots without invoking OCR.
///
/// # Panics
/// Panics only if the embedded layout, which is validated by the build's layout contract tests,
/// can no longer be decoded.
pub fn observe_music_select_difficulty(
    crops: &MusicSelectDifficultyMarkerCrops,
) -> MusicSelectDifficultyObservation {
    let layout = IntegratedContextLayout::load()
        .expect("the embedded integrated context layout is statically validated");
    let policy = &layout.music_select.selected_difficulty;
    let slots = crops.as_slots().map(|(difficulty, crop)| {
        let top_edge_ppm = marker_edge_ppm(crop, 36..114, 8, 11);
        let bottom_edge_ppm = marker_edge_ppm(crop, 10..114, 26, 23);
        let score_ppm = top_edge_ppm.min(bottom_edge_ppm);
        MusicSelectDifficultyMarkerEvidence {
            difficulty,
            top_edge_ppm,
            bottom_edge_ppm,
            score_ppm,
            qualifies: score_ppm >= policy.score_min_ppm,
        }
    });
    let mut ranked = slots;
    ranked.sort_by(|left, right| {
        right
            .score_ppm
            .cmp(&left.score_ppm)
            .then_with(|| left.difficulty.cmp(&right.difficulty))
    });
    let winner = ranked[0];
    let runner_up = ranked[1];
    let margin_ppm = winner.score_ppm.saturating_sub(runner_up.score_ppm);
    let qualifying = slots.iter().filter(|slot| slot.qualifies).count();
    let state = match qualifying {
        0 => MusicSelectDifficultyState::Unknown(MusicSelectDifficultyUnknownReason::NoCandidate),
        1 if margin_ppm < policy.winner_margin_min_ppm => MusicSelectDifficultyState::Unknown(
            MusicSelectDifficultyUnknownReason::InsufficientMargin,
        ),
        1 => MusicSelectDifficultyState::Known(winner.difficulty),
        _ => MusicSelectDifficultyState::Unknown(
            MusicSelectDifficultyUnknownReason::MultipleCandidates,
        ),
    };
    MusicSelectDifficultyObservation {
        predicate_id: "scorepeek-player-marker-outline-v2",
        state,
        winner_score_ppm: winner.score_ppm,
        runner_up_score_ppm: runner_up.score_ppm,
        margin_ppm,
        slots,
    }
}

#[must_use]
/// Distinguishes 1P and 2P from the mutually exclusive footer labels on MUSIC SELECT.
///
/// # Panics
/// Panics only if the embedded, statically validated recognition layout cannot be loaded.
pub fn observe_music_select_play_side(
    crops: &MusicSelectPlaySideCrops,
) -> MusicSelectPlaySideObservation {
    let layout = IntegratedContextLayout::load()
        .expect("the embedded integrated context layout is statically validated");
    let policy = &layout.music_select.play_side;
    let mut sides = [
        MusicSelectPlaySideEvidence {
            play_side: PlaySide::OnePlayer,
            bright_pixels: bright_luma_pixels(&crops.one_player, policy.luma_min),
            qualifies: false,
        },
        MusicSelectPlaySideEvidence {
            play_side: PlaySide::TwoPlayer,
            bright_pixels: bright_luma_pixels(&crops.two_player, policy.luma_min),
            qualifies: false,
        },
    ];
    for side in &mut sides {
        side.qualifies = side.bright_pixels >= policy.bright_pixel_min;
    }
    let mut ranked = sides;
    ranked.sort_by(|left, right| {
        right
            .bright_pixels
            .cmp(&left.bright_pixels)
            .then_with(|| left.play_side.cmp(&right.play_side))
    });
    let winner = ranked[0];
    let runner_up = ranked[1];
    let margin = winner.bright_pixels.saturating_sub(runner_up.bright_pixels);
    let qualifying = sides.iter().filter(|side| side.qualifies).count();
    let state = match qualifying {
        0 => MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::NoCandidate),
        1 if margin < policy.winner_margin_min => {
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::InsufficientMargin)
        }
        1 => MusicSelectPlaySideState::Known(winner.play_side),
        _ => {
            MusicSelectPlaySideState::Unknown(MusicSelectPlaySideUnknownReason::MultipleCandidates)
        }
    };
    MusicSelectPlaySideObservation {
        predicate_id: "scorepeek-music-select-footer-brightness-v1",
        state,
        winner_bright_pixels: winner.bright_pixels,
        runner_up_bright_pixels: runner_up.bright_pixels,
        margin,
        sides,
    }
}

fn bright_luma_pixels(crop: &Rgb8Crop, luma_min: u8) -> u32 {
    u32::try_from(
        crop.pixels
            .chunks_exact(3)
            .filter(|pixel| {
                let luma = u32::from(pixel[0]) * 2_126
                    + u32::from(pixel[1]) * 7_152
                    + u32::from(pixel[2]) * 722;
                luma >= u32::from(luma_min) * 10_000
            })
            .count(),
    )
    .expect("a canonical crop pixel count fits u32")
}

fn marker_edge_ppm(
    crop: &Rgb8Crop,
    columns: std::ops::Range<usize>,
    edge_y: usize,
    inside_y: usize,
) -> u32 {
    let white = |x: usize, y: usize| {
        let offset = (y * crop.roi.width as usize + x) * 3;
        let [r, g, b] = [
            crop.pixels[offset],
            crop.pixels[offset + 1],
            crop.pixels[offset + 2],
        ];
        r.min(g).min(b) >= 180 && r.max(g).max(b) - r.min(g).min(b) <= 45
    };
    let count = columns.len();
    let matched = columns
        .filter(|&x| {
            (white(x, edge_y) || white(x, edge_y + 1))
                && !white(x, inside_y)
                && !white(x, inside_y + 1)
        })
        .count();
    u32::try_from(matched * 1_000_000 / count).expect("a matched fraction is at most one million")
}
