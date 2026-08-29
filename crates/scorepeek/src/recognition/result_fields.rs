use serde::{Deserialize, Serialize};

use crate::catalog::{Catalog, Chart, Difficulty, PlayType, ScorepeekSongId};

use super::{DynamicTextObservation, ResultScreenFieldObservations};

pub const RESULT_FIELD_RESOLVER_ID: &str = "scorepeek-result-fields-catalog-constrained-v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultFieldUnknownReason {
    Empty,
    InvalidFormat,
    OutOfRange,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResultFieldValue<T> {
    Known { value: T },
    Unknown { reason: ResultFieldUnknownReason },
}

impl<T> ResultFieldValue<T> {
    #[must_use]
    pub const fn known(&self) -> Option<&T> {
        match self {
            Self::Known { value } => Some(value),
            Self::Unknown { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ParsedResultFields {
    pub resolver_id: String,
    pub difficulty: ResultFieldValue<Difficulty>,
    pub level: ResultFieldValue<u8>,
    pub notes: ResultFieldValue<u32>,
    pub current_score: ResultFieldValue<u32>,
}

impl ParsedResultFields {
    #[must_use]
    pub fn from_observations(fields: &ResultScreenFieldObservations) -> Self {
        Self {
            resolver_id: RESULT_FIELD_RESOLVER_ID.to_owned(),
            difficulty: parse_difficulty(&fields.difficulty),
            level: parse_decimal(&fields.level, 1, 12),
            notes: parse_decimal(&fields.notes, 1, u32::MAX),
            current_score: parse_decimal(&fields.current_score, 0, u32::MAX),
        }
    }

    #[must_use]
    pub const fn complete_chart_tuple(&self) -> Option<(Difficulty, u8, u32)> {
        let ResultFieldValue::Known { value: difficulty } = self.difficulty else {
            return None;
        };
        let ResultFieldValue::Known { value: level } = self.level else {
            return None;
        };
        let ResultFieldValue::Known { value: notes } = self.notes else {
            return None;
        };
        Some((difficulty, level, notes))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultChartUnknownReason {
    IncompleteObservation,
    SongMissingFromCatalog,
    NoMatchingChart,
    MultipleMatchingCharts,
    CurrentScoreUnknown,
    CurrentScoreExceedsMaximum,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResultChartResolution {
    Accepted {
        resolver_id: String,
        chart: Chart,
        current_score: u32,
    },
    Unknown {
        resolver_id: String,
        reason: ResultChartUnknownReason,
        matching_charts: Vec<Chart>,
    },
}

impl ResultChartResolution {
    #[must_use]
    pub const fn accepted(&self) -> Option<(&Chart, u32)> {
        match self {
            Self::Accepted {
                chart,
                current_score,
                ..
            } => Some((chart, *current_score)),
            Self::Unknown { .. } => None,
        }
    }
}

#[must_use]
pub fn resolve_result_chart(
    catalog: &Catalog,
    song_id: ScorepeekSongId,
    fields: &ParsedResultFields,
) -> ResultChartResolution {
    let Some(difficulty) = fields.difficulty.known().copied() else {
        return unknown(ResultChartUnknownReason::IncompleteObservation, Vec::new());
    };
    let Some(notes) = fields.notes.known().copied() else {
        return unknown(ResultChartUnknownReason::IncompleteObservation, Vec::new());
    };
    let level = fields.level.known().copied();
    let Some(song) = catalog.songs().get(&song_id) else {
        return unknown(ResultChartUnknownReason::SongMissingFromCatalog, Vec::new());
    };
    let matching_charts = song
        .charts()
        .values()
        .filter(|chart| {
            chart.key.play_type == PlayType::Single
                && chart.key.difficulty == difficulty
                && level.is_none_or(|level| chart.level == level)
                && chart.notes == notes
        })
        .cloned()
        .collect::<Vec<_>>();
    if matching_charts.is_empty() {
        return unknown(ResultChartUnknownReason::NoMatchingChart, matching_charts);
    }
    if matching_charts.len() != 1 {
        return unknown(
            ResultChartUnknownReason::MultipleMatchingCharts,
            matching_charts,
        );
    }
    let Some(current_score) = fields.current_score.known().copied() else {
        return unknown(
            ResultChartUnknownReason::CurrentScoreUnknown,
            matching_charts,
        );
    };
    if current_score > notes.saturating_mul(2) {
        return unknown(
            ResultChartUnknownReason::CurrentScoreExceedsMaximum,
            matching_charts,
        );
    }
    let Some(chart) = matching_charts.into_iter().next() else {
        return unknown(ResultChartUnknownReason::NoMatchingChart, Vec::new());
    };
    ResultChartResolution::Accepted {
        resolver_id: RESULT_FIELD_RESOLVER_ID.to_owned(),
        chart,
        current_score,
    }
}

#[must_use]
pub fn matching_single_play_songs(
    catalog: &Catalog,
    fields: &ParsedResultFields,
) -> Vec<ScorepeekSongId> {
    let Some(difficulty) = fields.difficulty.known().copied() else {
        return Vec::new();
    };
    let Some(notes) = fields.notes.known().copied() else {
        return Vec::new();
    };
    let level = fields.level.known().copied();
    catalog
        .songs()
        .iter()
        .filter(|(_, song)| {
            song.charts().values().any(|chart| {
                chart.key.play_type == PlayType::Single
                    && chart.key.difficulty == difficulty
                    && level.is_none_or(|level| chart.level == level)
                    && chart.notes == notes
            })
        })
        .map(|(song_id, _)| *song_id)
        .collect()
}

fn parse_difficulty(observation: &DynamicTextObservation) -> ResultFieldValue<Difficulty> {
    let value = observation.open_text.trim().to_ascii_uppercase();
    if value.is_empty() {
        return ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::Empty,
        };
    }
    let mut matches = [
        ("BEGINNER", Difficulty::Beginner),
        ("NORMAL", Difficulty::Normal),
        ("HYPER", Difficulty::Hyper),
        ("ANOTHER", Difficulty::Another),
        ("LEGGENDARIA", Difficulty::Leggendaria),
    ]
    .into_iter()
    .filter(|(candidate, _)| ascii_edit_distance_at_most_one(&value, candidate));
    let Some((_, selected)) = matches.next() else {
        return ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::InvalidFormat,
        };
    };
    if matches.next().is_some() {
        return ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::InvalidFormat,
        };
    }
    ResultFieldValue::Known { value: selected }
}

fn parse_decimal<T>(
    observation: &DynamicTextObservation,
    minimum: T,
    maximum: T,
) -> ResultFieldValue<T>
where
    T: Copy + Ord + std::str::FromStr,
{
    let raw = observation.open_text.trim();
    let normalized_one;
    let value = if matches!(raw, "I" | "l" | "|") {
        normalized_one = "1";
        normalized_one
    } else {
        raw
    };
    if value.is_empty() {
        return ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::Empty,
        };
    }
    if !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::InvalidFormat,
        };
    }
    let Ok(value) = value.parse::<T>() else {
        return ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::OutOfRange,
        };
    };
    if value < minimum || value > maximum {
        return ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::OutOfRange,
        };
    }
    ResultFieldValue::Known { value }
}

fn ascii_edit_distance_at_most_one(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len().abs_diff(right.len()) > 1 {
        return false;
    }
    let (shorter, longer) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    let mut short = 0;
    let mut long = 0;
    let mut edits = 0;
    while short < shorter.len() && long < longer.len() {
        if shorter[short] == longer[long] {
            short += 1;
            long += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            if shorter.len() == longer.len() {
                short += 1;
            }
            long += 1;
        }
    }
    edits + usize::from(long < longer.len()) <= 1
}

fn unknown(reason: ResultChartUnknownReason, matching_charts: Vec<Chart>) -> ResultChartResolution {
    ResultChartResolution::Unknown {
        resolver_id: RESULT_FIELD_RESOLVER_ID.to_owned(),
        reason,
        matching_charts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> DynamicTextObservation {
        DynamicTextObservation {
            input_width: 1,
            output_timesteps: 1,
            open_text: value.to_owned(),
        }
    }

    #[test]
    fn parses_exact_closed_difficulty_and_decimal_fields() {
        assert_eq!(
            parse_difficulty(&text(" HYPER ")),
            ResultFieldValue::Known {
                value: Difficulty::Hyper
            }
        );
        assert_eq!(
            parse_decimal(&text("0764"), 1_u32, u32::MAX),
            ResultFieldValue::Known { value: 764 }
        );
        assert!(matches!(
            parse_decimal(&text("76A"), 1_u32, u32::MAX),
            ResultFieldValue::Unknown {
                reason: ResultFieldUnknownReason::InvalidFormat
            }
        ));
    }
}
