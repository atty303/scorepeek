use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::{DynamicTextObservation, Rgb8Crop};

const OPTION_PREFIX: &str = "USE OPTION ";
const MARKER_WIDTH: usize = 120;
const MARKER_ACTIVE_MINIMUM: u32 = 1_000;
const MARKER_INACTIVE_MAXIMUM: u32 = 100;
const MAXIMUM_EDIT_DISTANCE: usize = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayOption {
    Random,
    RRandom,
    SRandom,
    Mirror,
    AutoScratch,
    Legacy,
}

impl PlayOption {
    pub const ALL: [Self; 6] = [
        Self::Random,
        Self::RRandom,
        Self::SRandom,
        Self::Mirror,
        Self::AutoScratch,
        Self::Legacy,
    ];

    const fn display_token(self) -> &'static str {
        match self {
            Self::Random => "RANDOM",
            Self::RRandom => "R-RANDOM",
            Self::SRandom => "S-RANDOM",
            Self::Mirror => "MIRROR",
            Self::AutoScratch => "A-SCR",
            Self::Legacy => "LEGACY",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayOptionMarkerState {
    Active,
    Inactive,
    #[default]
    Inconclusive,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayOptionMarkerObservation {
    pub state: PlayOptionMarkerState,
    pub orange_pixels: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayOptionsUnknownReason {
    NotObserved,
    InsufficientObservations,
    Unrecognized,
    Ambiguous,
    ConflictingObservations,
    MarkerInconclusive,
    FieldFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PlayOptions {
    Known { values: Vec<PlayOption> },
    Unknown { reason: PlayOptionsUnknownReason },
}

impl Default for PlayOptions {
    fn default() -> Self {
        Self::Unknown {
            reason: PlayOptionsUnknownReason::NotObserved,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayOptionsObservation {
    pub marker: PlayOptionMarkerObservation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nearest_display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nearest_distance: Option<u8>,
    pub parsed: PlayOptions,
}

impl PlayOptionsObservation {
    #[must_use]
    pub fn failed(crop: &Rgb8Crop) -> Self {
        Self {
            marker: observe_marker(crop),
            parsed: PlayOptions::Unknown {
                reason: PlayOptionsUnknownReason::FieldFailed,
            },
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug)]
struct DisplayCandidate {
    display: String,
    values: Vec<PlayOption>,
}

#[must_use]
pub fn observe_play_options(
    crop: &Rgb8Crop,
    text: &DynamicTextObservation,
) -> PlayOptionsObservation {
    let marker = observe_marker(crop);
    let raw_text = text.open_text.clone();
    let normalized = normalize(&raw_text);
    match marker.state {
        PlayOptionMarkerState::Inactive if normalized.is_empty() => PlayOptionsObservation {
            marker,
            raw_text: Some(raw_text),
            normalized_text: Some(normalized),
            parsed: PlayOptions::Known { values: Vec::new() },
            ..PlayOptionsObservation::default()
        },
        PlayOptionMarkerState::Inactive | PlayOptionMarkerState::Inconclusive => {
            PlayOptionsObservation {
                marker,
                raw_text: Some(raw_text),
                normalized_text: Some(normalized),
                parsed: PlayOptions::Unknown {
                    reason: PlayOptionsUnknownReason::MarkerInconclusive,
                },
                ..PlayOptionsObservation::default()
            }
        }
        PlayOptionMarkerState::Active => parse_active(marker, raw_text, normalized),
    }
}

fn parse_active(
    marker: PlayOptionMarkerObservation,
    raw_text: String,
    normalized: String,
) -> PlayOptionsObservation {
    if normalized.is_empty() {
        return PlayOptionsObservation {
            marker,
            raw_text: Some(raw_text),
            normalized_text: Some(normalized),
            parsed: PlayOptions::Unknown {
                reason: PlayOptionsUnknownReason::Unrecognized,
            },
            ..PlayOptionsObservation::default()
        };
    }
    let mut minimum = usize::MAX;
    let mut nearest = Vec::new();
    for candidate in display_candidates() {
        let distance = levenshtein_distance(&normalized, &candidate.display);
        match distance.cmp(&minimum) {
            std::cmp::Ordering::Less => {
                minimum = distance;
                nearest.clear();
                nearest.push(candidate);
            }
            std::cmp::Ordering::Equal => nearest.push(candidate),
            std::cmp::Ordering::Greater => {}
        }
    }
    let distance = u8::try_from(minimum).unwrap_or(u8::MAX);
    let selected = (minimum <= MAXIMUM_EDIT_DISTANCE && nearest.len() == 1).then(|| nearest[0]);
    PlayOptionsObservation {
        marker,
        raw_text: Some(raw_text),
        normalized_text: Some(normalized),
        nearest_display: (nearest.len() == 1).then(|| nearest[0].display.clone()),
        nearest_distance: Some(distance),
        parsed: selected.map_or_else(
            || PlayOptions::Unknown {
                reason: if minimum <= MAXIMUM_EDIT_DISTANCE {
                    PlayOptionsUnknownReason::Ambiguous
                } else {
                    PlayOptionsUnknownReason::Unrecognized
                },
            },
            |candidate| PlayOptions::Known {
                values: candidate.values.clone(),
            },
        ),
    }
}

fn observe_marker(crop: &Rgb8Crop) -> PlayOptionMarkerObservation {
    let width = usize::try_from(crop.roi.width).unwrap_or(0);
    let height = usize::try_from(crop.roi.height).unwrap_or(0);
    let marker_width = width.min(MARKER_WIDTH);
    let mut orange_pixels = 0_u32;
    for y in 0..height {
        for x in 0..marker_width {
            let offset = (y * width + x) * 3;
            let Some([red, green, blue]) = crop.pixels.get(offset..offset + 3) else {
                return PlayOptionMarkerObservation::default();
            };
            let red = u16::from(*red);
            let green = u16::from(*green);
            let blue = u16::from(*blue);
            if red >= 180
                && (55..=190).contains(&green)
                && blue <= 90
                && red >= green.saturating_add(50)
            {
                orange_pixels = orange_pixels.saturating_add(1);
            }
        }
    }
    let state = if orange_pixels >= MARKER_ACTIVE_MINIMUM {
        PlayOptionMarkerState::Active
    } else if orange_pixels <= MARKER_INACTIVE_MAXIMUM {
        PlayOptionMarkerState::Inactive
    } else {
        PlayOptionMarkerState::Inconclusive
    };
    PlayOptionMarkerObservation {
        state,
        orange_pixels,
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn display_candidates() -> &'static [DisplayCandidate] {
    static CANDIDATES: OnceLock<Vec<DisplayCandidate>> = OnceLock::new();
    CANDIDATES.get_or_init(|| {
        let mut candidates = Vec::new();
        let mut prefix = Vec::new();
        append_permutations(&mut candidates, &mut prefix, &mut BTreeSet::new());
        candidates
    })
}

fn append_permutations(
    candidates: &mut Vec<DisplayCandidate>,
    prefix: &mut Vec<PlayOption>,
    used: &mut BTreeSet<PlayOption>,
) {
    if !prefix.is_empty() {
        let suffix = prefix
            .iter()
            .map(|option| option.display_token())
            .collect::<Vec<_>>()
            .join(",");
        candidates.push(DisplayCandidate {
            display: format!("{OPTION_PREFIX}{suffix}"),
            values: prefix.clone(),
        });
    }
    for option in PlayOption::ALL {
        if used.insert(option) {
            prefix.push(option);
            append_permutations(candidates, prefix, used);
            prefix.pop();
            used.remove(&option);
        }
    }
}

fn levenshtein_distance(left: &str, right: &str) -> usize {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (left_index, left_char) in left.iter().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right.iter().enumerate() {
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + usize::from(left_char != right_char));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognition::Roi;

    fn crop(orange_pixels: usize) -> Rgb8Crop {
        let mut pixels = vec![0_u8; 530 * 50 * 3];
        for marker_pixel in 0..orange_pixels.min(MARKER_WIDTH * 50) {
            let y = marker_pixel / MARKER_WIDTH;
            let x = marker_pixel % MARKER_WIDTH;
            let offset = (y * 530 + x) * 3;
            pixels[offset..offset + 3].copy_from_slice(&[220, 120, 20]);
        }
        Rgb8Crop {
            roi: Roi {
                x: 30,
                y: 318,
                width: 530,
                height: 50,
            },
            pixels,
        }
    }

    fn text(value: &str) -> DynamicTextObservation {
        DynamicTextObservation {
            input_width: 1,
            output_timesteps: 1,
            open_text: value.to_owned(),
            constrained_text: None,
        }
    }

    #[test]
    fn parses_exact_and_one_edit_option_lists() {
        let active = crop(1_100);
        assert_eq!(
            observe_play_options(&active, &text("USE OPTION RANDOM,LEGACY")).parsed,
            PlayOptions::Known {
                values: vec![PlayOption::Random, PlayOption::Legacy]
            }
        );
        assert_eq!(
            observe_play_options(&active, &text("USE OPTION RANDOMLEGACY")).parsed,
            PlayOptions::Known {
                values: vec![PlayOption::Random, PlayOption::Legacy]
            }
        );
        assert_eq!(
            observe_play_options(&active, &text("USE OPTION A-SCR")).parsed,
            PlayOptions::Known {
                values: vec![PlayOption::AutoScratch]
            }
        );
    }

    #[test]
    fn rejects_unknown_or_distant_text() {
        let observed = observe_play_options(&crop(1_100), &text("USE OPTION UNKNOWN"));
        assert!(matches!(
            observed.parsed,
            PlayOptions::Unknown {
                reason: PlayOptionsUnknownReason::Unrecognized
            }
        ));
        let ambiguous = observe_play_options(&crop(1_100), &text("USE OPTION RRANDOM"));
        assert!(matches!(
            ambiguous.parsed,
            PlayOptions::Unknown {
                reason: PlayOptionsUnknownReason::Ambiguous
            }
        ));
    }

    #[test]
    fn marker_requires_positive_inactive_evidence_for_empty_options() {
        assert_eq!(
            observe_play_options(&crop(0), &text("")).parsed,
            PlayOptions::Known { values: Vec::new() }
        );
        assert!(matches!(
            observe_play_options(&crop(500), &text("")).parsed,
            PlayOptions::Unknown {
                reason: PlayOptionsUnknownReason::MarkerInconclusive
            }
        ));
        assert_eq!(
            observe_marker(&crop(100)).state,
            PlayOptionMarkerState::Inactive
        );
        assert_eq!(
            observe_marker(&crop(1_000)).state,
            PlayOptionMarkerState::Active
        );
        assert!(matches!(
            observe_play_options(&crop(0), &text("USE OPTION RANDOM")).parsed,
            PlayOptions::Unknown {
                reason: PlayOptionsUnknownReason::MarkerInconclusive
            }
        ));
    }

    #[test]
    fn generated_candidates_are_bounded_unique_token_sequences() {
        let candidates = display_candidates();
        assert_eq!(candidates.len(), 1_956);
        assert!(candidates.iter().any(|candidate| {
            candidate.values == vec![PlayOption::Legacy, PlayOption::Random]
                && candidate.display == "USE OPTION LEGACY,RANDOM"
        }));
    }
}
