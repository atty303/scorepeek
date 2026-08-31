use serde::{Deserialize, Serialize};

use crate::catalog::{Catalog, Chart, Difficulty, PlayType, ScorepeekSongId};

use super::{DynamicTextObservation, ResultScreenFieldObservations};

pub const RESULT_FIELD_RESOLVER_ID: &str = "scorepeek-result-fields-catalog-constrained-v5";
pub const RESULT_PERFORMANCE_RESOLVER_ID: &str = "scorepeek-result-performance-v1";

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SupplementalResultValue<T> {
    Known { value: T },
    NotDisplayed,
    Unknown { reason: ResultFieldUnknownReason },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PreviousBestValue<T> {
    Known { value: T },
    NotPlayed,
    NotDisplayed,
    Unknown { reason: ResultFieldUnknownReason },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultJudgments {
    pub pgreat: u32,
    pub great: u32,
    pub good: u32,
    pub bad: u32,
    pub poor: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultTiming {
    pub fast: SupplementalResultValue<u32>,
    pub slow: SupplementalResultValue<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreviousBest {
    pub clear_type: PreviousBestValue<String>,
    pub score: PreviousBestValue<u32>,
    pub miss_count: PreviousBestValue<u32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultPerformanceUnknownReason {
    IncompleteJudgments,
    JudgmentExceedsNotes,
    ScoreBreakdownMismatch,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResultPerformanceResolution {
    Accepted {
        resolver_id: String,
        judgments: ResultJudgments,
        miss_count: SupplementalResultValue<u32>,
        timing: ResultTiming,
        combo_break: SupplementalResultValue<u32>,
        previous_best: PreviousBest,
    },
    Unknown {
        resolver_id: String,
        reason: ResultPerformanceUnknownReason,
    },
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
    pub previous_clear_type: PreviousBestValue<String>,
    pub previous_score: PreviousBestValue<u32>,
    pub previous_miss_count: PreviousBestValue<u32>,
    pub miss_count: SupplementalResultValue<u32>,
    pub pgreat: ResultFieldValue<u32>,
    pub great: ResultFieldValue<u32>,
    pub good: ResultFieldValue<u32>,
    pub bad: ResultFieldValue<u32>,
    pub poor: ResultFieldValue<u32>,
    pub fast: SupplementalResultValue<u32>,
    pub slow: SupplementalResultValue<u32>,
    pub combo_break: SupplementalResultValue<u32>,
}

impl ParsedResultFields {
    #[must_use]
    pub fn from_observations(fields: &ResultScreenFieldObservations) -> Self {
        let previous_not_played =
            unique_ascii_match(&fields.previous_clear_type.open_text, &["NO PLAY"]).is_some();
        let previous_clear_type = if previous_not_played {
            PreviousBestValue::NotPlayed
        } else {
            resolve_clear_type(&fields.previous_clear_type.open_text).map_or(
                PreviousBestValue::Unknown {
                    reason: field_unknown_reason(&fields.previous_clear_type.open_text),
                },
                |value| PreviousBestValue::Known {
                    value: value.to_owned(),
                },
            )
        };
        let previous_score = parse_previous_decimal(&fields.previous_score, previous_not_played);
        let previous_miss_count = if previous_not_played {
            PreviousBestValue::NotPlayed
        } else if fields
            .previous_miss_count
            .constrained_text()
            .is_some_and(is_displayed_dash)
        {
            PreviousBestValue::NotDisplayed
        } else {
            previous_value(parse_decimal(&fields.previous_miss_count, 0, u32::MAX))
        };
        Self {
            resolver_id: RESULT_FIELD_RESOLVER_ID.to_owned(),
            difficulty: parse_difficulty(&fields.difficulty),
            level: parse_decimal(&fields.level, 1, 12),
            notes: parse_decimal(&fields.notes, 1, u32::MAX),
            current_score: parse_decimal(&fields.current_score, 0, u32::MAX),
            previous_clear_type,
            previous_score,
            previous_miss_count,
            miss_count: parse_supplemental_decimal(&fields.miss_count, true),
            pgreat: parse_decimal(&fields.pgreat, 0, u32::MAX),
            great: parse_decimal(&fields.great, 0, u32::MAX),
            good: parse_decimal(&fields.good, 0, u32::MAX),
            bad: parse_decimal(&fields.bad, 0, u32::MAX),
            poor: parse_decimal(&fields.poor, 0, u32::MAX),
            fast: parse_supplemental_decimal(&fields.fast, true),
            slow: parse_supplemental_decimal(&fields.slow, true),
            combo_break: parse_supplemental_decimal(&fields.combo_break, true),
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

#[must_use]
pub fn resolve_result_performance(
    fields: &ParsedResultFields,
    notes: u32,
    current_score: u32,
) -> ResultPerformanceResolution {
    let (
        ResultFieldValue::Known { value: pgreat },
        ResultFieldValue::Known { value: great },
        ResultFieldValue::Known { value: good },
        ResultFieldValue::Known { value: bad },
        ResultFieldValue::Known { value: poor },
    ) = (
        &fields.pgreat,
        &fields.great,
        &fields.good,
        &fields.bad,
        &fields.poor,
    )
    else {
        return performance_unknown(ResultPerformanceUnknownReason::IncompleteJudgments);
    };
    if [pgreat, great, good, bad]
        .into_iter()
        .any(|value| *value > notes)
    {
        return performance_unknown(ResultPerformanceUnknownReason::JudgmentExceedsNotes);
    }
    if pgreat
        .checked_mul(2)
        .and_then(|value| value.checked_add(*great))
        != Some(current_score)
    {
        return performance_unknown(ResultPerformanceUnknownReason::ScoreBreakdownMismatch);
    }
    ResultPerformanceResolution::Accepted {
        resolver_id: RESULT_PERFORMANCE_RESOLVER_ID.to_owned(),
        judgments: ResultJudgments {
            pgreat: *pgreat,
            great: *great,
            good: *good,
            bad: *bad,
            poor: *poor,
        },
        miss_count: fields.miss_count.clone(),
        timing: ResultTiming {
            fast: fields.fast.clone(),
            slow: fields.slow.clone(),
        },
        combo_break: fields.combo_break.clone(),
        previous_best: PreviousBest {
            clear_type: fields.previous_clear_type.clone(),
            score: bound_previous(&fields.previous_score, notes.saturating_mul(2)),
            miss_count: fields.previous_miss_count.clone(),
        },
    }
}

#[must_use]
pub fn resolve_clear_type(observed: &str) -> Option<&'static str> {
    if observed == "A-CLEAR" {
        return Some("ASSIST CLEAR");
    }
    if observed == "H-CLEAR" {
        return Some("HARD CLEAR");
    }
    unique_ascii_match(
        observed,
        &[
            "FAILED",
            "ASSIST CLEAR",
            "EASY CLEAR",
            "CLEAR",
            "HARD CLEAR",
            "EXH-CLEAR",
            "F-COMBO",
        ],
    )
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
    let notes = fields.notes.known().copied();
    let Some(song) = catalog.songs().get(&song_id) else {
        return unknown(ResultChartUnknownReason::SongMissingFromCatalog, Vec::new());
    };
    let matching_charts = song
        .charts()
        .values()
        .filter(|chart| {
            chart.key.play_type == PlayType::Single
                && chart.key.difficulty == difficulty
                && notes.is_none_or(|notes| chart.notes == notes)
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
    let Some(chart) = matching_charts.into_iter().next() else {
        return unknown(ResultChartUnknownReason::NoMatchingChart, Vec::new());
    };
    if current_score > chart.notes.saturating_mul(2) {
        return unknown(
            ResultChartUnknownReason::CurrentScoreExceedsMaximum,
            vec![chart],
        );
    }
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

#[must_use]
pub fn observed_result_difficulty(observation: &DynamicTextObservation) -> Option<Difficulty> {
    parse_difficulty(observation).known().copied()
}

fn parse_decimal<T>(
    observation: &DynamicTextObservation,
    minimum: T,
    maximum: T,
) -> ResultFieldValue<T>
where
    T: Copy + Ord + std::str::FromStr,
{
    let Some(value) = observation.constrained_text().map(str::trim) else {
        return ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::Empty,
        };
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

fn unique_ascii_match<'a>(observed: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let value = observed.trim().to_ascii_uppercase();
    let mut matches = candidates
        .iter()
        .copied()
        .filter(|candidate| ascii_edit_distance_at_most_one(&value, candidate));
    let selected = matches.next()?;
    matches.next().is_none().then_some(selected)
}

fn field_unknown_reason(raw: &str) -> ResultFieldUnknownReason {
    if raw.trim().is_empty() {
        ResultFieldUnknownReason::Empty
    } else {
        ResultFieldUnknownReason::InvalidFormat
    }
}

fn is_displayed_dash(raw: &str) -> bool {
    let value = raw.trim();
    !value.is_empty()
        && value
            .chars()
            .all(|character| matches!(character, '-' | '―' | 'ー' | '—'))
}

fn parse_supplemental_decimal(
    observation: &DynamicTextObservation,
    dash_is_not_displayed: bool,
) -> SupplementalResultValue<u32> {
    if dash_is_not_displayed
        && observation
            .constrained_text()
            .is_some_and(is_displayed_dash)
    {
        return SupplementalResultValue::NotDisplayed;
    }
    match parse_decimal(observation, 0, u32::MAX) {
        ResultFieldValue::Known { value } => SupplementalResultValue::Known { value },
        ResultFieldValue::Unknown { reason } => SupplementalResultValue::Unknown { reason },
    }
}

fn parse_previous_decimal(
    observation: &DynamicTextObservation,
    not_played: bool,
) -> PreviousBestValue<u32> {
    if not_played {
        return PreviousBestValue::NotPlayed;
    }
    if observation
        .constrained_text()
        .is_some_and(is_displayed_dash)
    {
        return PreviousBestValue::NotDisplayed;
    }
    previous_value(parse_decimal(observation, 0, u32::MAX))
}

fn previous_value<T>(value: ResultFieldValue<T>) -> PreviousBestValue<T> {
    match value {
        ResultFieldValue::Known { value } => PreviousBestValue::Known { value },
        ResultFieldValue::Unknown { reason } => PreviousBestValue::Unknown { reason },
    }
}

fn performance_unknown(reason: ResultPerformanceUnknownReason) -> ResultPerformanceResolution {
    ResultPerformanceResolution::Unknown {
        resolver_id: RESULT_PERFORMANCE_RESOLVER_ID.to_owned(),
        reason,
    }
}

fn bound_previous(value: &PreviousBestValue<u32>, maximum: u32) -> PreviousBestValue<u32> {
    match value {
        PreviousBestValue::Known { value } if *value <= maximum => {
            PreviousBestValue::Known { value: *value }
        }
        PreviousBestValue::Known { .. } => PreviousBestValue::Unknown {
            reason: ResultFieldUnknownReason::OutOfRange,
        },
        PreviousBestValue::NotPlayed => PreviousBestValue::NotPlayed,
        PreviousBestValue::NotDisplayed => PreviousBestValue::NotDisplayed,
        PreviousBestValue::Unknown { reason } => PreviousBestValue::Unknown { reason: *reason },
    }
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
    use serde_json::json;

    use super::*;
    use crate::catalog::{FederationInput, SourceRevision, TachiFixtureAdapter};

    fn text(value: &str) -> DynamicTextObservation {
        DynamicTextObservation {
            input_width: 1,
            output_timesteps: 1,
            open_text: value.to_owned(),
            constrained_text: Some(value.to_owned()),
        }
    }

    fn result_fields() -> ResultScreenFieldObservations {
        ResultScreenFieldObservations {
            clear_type: text("CLEAR"),
            difficulty: text("HYPER"),
            level: text("8"),
            notes: text("764"),
            current_score: text("1286"),
            previous_clear_type: text("HARD CLEAR"),
            previous_score: text("1200"),
            previous_miss_count: text("12"),
            miss_count: text("9"),
            pgreat: text("600"),
            great: text("86"),
            good: text("20"),
            bad: text("5"),
            poor: text("3"),
            fast: text("40"),
            slow: text("41"),
            combo_break: text("7"),
            ..Default::default()
        }
    }

    fn single_chart_catalog() -> Catalog {
        let bytes = serde_json::to_vec(&json!({
            "schema": "scorepeek-tachi-fixture-v1",
            "records": [{
                "source_song_id": "result-chart",
                "title": "RESULT CHART",
                "title_kind": "in_game_display",
                "artist": "ARTIST",
                "version": "SYNTHETIC",
                "charts": [{
                    "play_type": "single",
                    "difficulty": "hyper",
                    "level": 8,
                    "notes": 764,
                    "source_chart_id": "sph",
                    "product_versions": ["synthetic-v1"],
                    "primary": true
                }],
                "primary_infinitas": true
            }]
        }))
        .unwrap();
        let snapshot = TachiFixtureAdapter::parse(
            &bytes,
            SourceRevision::git_commit("0123456789abcdef0123456789abcdef01234567").unwrap(),
        )
        .unwrap();
        Catalog::default()
            .federate(FederationInput {
                tachi: Some(snapshot),
                ..FederationInput::default()
            })
            .catalog
    }

    #[test]
    fn unique_catalog_chart_resolves_when_level_and_notes_are_not_displayed() {
        let catalog = single_chart_catalog();
        let song_id = *catalog.songs().keys().next().unwrap();
        let mut parsed = ParsedResultFields::from_observations(&result_fields());
        parsed.level = ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::Empty,
        };
        parsed.notes = ResultFieldValue::Unknown {
            reason: ResultFieldUnknownReason::Empty,
        };
        assert!(matches!(
            resolve_result_chart(&catalog, song_id, &parsed),
            ResultChartResolution::Accepted {
                chart: Chart {
                    level: 8,
                    notes: 764,
                    ..
                },
                current_score: 1_286,
                ..
            }
        ));
    }

    #[test]
    fn observed_level_never_vetoes_a_confirmed_song_chart() {
        let catalog = single_chart_catalog();
        let song_id = *catalog.songs().keys().next().unwrap();
        let mut parsed = ParsedResultFields::from_observations(&result_fields());
        parsed.level = ResultFieldValue::Known { value: 11 };
        assert!(matches!(
            resolve_result_chart(&catalog, song_id, &parsed),
            ResultChartResolution::Accepted {
                chart: Chart {
                    level: 8,
                    notes: 764,
                    ..
                },
                ..
            }
        ));
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

    #[test]
    fn numeric_parser_uses_constrained_decode_and_preserves_raw_evidence() {
        let mut observations = result_fields();
        observations.bad = DynamicTextObservation {
            input_width: 1,
            output_timesteps: 1,
            open_text: "只".to_owned(),
            constrained_text: Some("0".to_owned()),
        };
        let parsed = ParsedResultFields::from_observations(&observations);
        assert_eq!(observations.bad.open_text, "只");
        assert!(matches!(
            resolve_result_performance(&parsed, 764, 1_286),
            ResultPerformanceResolution::Accepted {
                judgments: ResultJudgments { bad: 0, .. },
                ..
            }
        ));
    }

    #[test]
    fn accepts_complete_judgments_and_keeps_supplemental_values_typed() {
        let parsed = ParsedResultFields::from_observations(&result_fields());
        assert!(matches!(
            resolve_result_performance(&parsed, 764, 1_286),
            ResultPerformanceResolution::Accepted {
                judgments: ResultJudgments {
                    pgreat: 600,
                    great: 86,
                    ..
                },
                miss_count: SupplementalResultValue::Known { value: 9 },
                ..
            }
        ));
    }

    #[test]
    fn rejects_unknown_or_mismatched_judgments() {
        let mut observations = result_fields();
        observations.good = text("2O");
        let parsed = ParsedResultFields::from_observations(&observations);
        assert!(matches!(
            resolve_result_performance(&parsed, 764, 1_286),
            ResultPerformanceResolution::Unknown {
                reason: ResultPerformanceUnknownReason::IncompleteJudgments,
                ..
            }
        ));

        observations.good = text("20");
        observations.great = text("85");
        let parsed = ParsedResultFields::from_observations(&observations);
        assert!(matches!(
            resolve_result_performance(&parsed, 764, 1_286),
            ResultPerformanceResolution::Unknown {
                reason: ResultPerformanceUnknownReason::ScoreBreakdownMismatch,
                ..
            }
        ));
    }

    #[test]
    fn failed_miss_is_not_displayed_and_supplemental_values_are_not_notes_bounded() {
        let mut observations = result_fields();
        observations.clear_type = text("FAILED");
        observations.miss_count = text("--");
        observations.fast = text("9999");
        let parsed = ParsedResultFields::from_observations(&observations);
        let ResultPerformanceResolution::Accepted {
            miss_count, timing, ..
        } = resolve_result_performance(&parsed, 764, 1_286)
        else {
            panic!("valid judgments must keep the result accepted");
        };
        assert_eq!(miss_count, SupplementalResultValue::NotDisplayed);
        assert_eq!(timing.fast, SupplementalResultValue::Known { value: 9999 });
    }

    #[test]
    fn poor_and_previous_miss_are_not_notes_bounded() {
        let mut observations = result_fields();
        observations.poor = text("9999");
        observations.previous_miss_count = text("9999");
        let parsed = ParsedResultFields::from_observations(&observations);
        let ResultPerformanceResolution::Accepted {
            judgments,
            previous_best,
            ..
        } = resolve_result_performance(&parsed, 764, 1_286)
        else {
            panic!("POOR and previous miss must not be constrained by notes");
        };
        assert_eq!(judgments.poor, 9999);
        assert_eq!(
            previous_best.miss_count,
            PreviousBestValue::Known { value: 9999 }
        );
    }

    #[test]
    fn note_judgments_are_individually_notes_bounded() {
        for field in ["pgreat", "great", "good", "bad"] {
            let mut observations = result_fields();
            *match field {
                "pgreat" => &mut observations.pgreat,
                "great" => &mut observations.great,
                "good" => &mut observations.good,
                "bad" => &mut observations.bad,
                _ => unreachable!(),
            } = text("765");
            let parsed = ParsedResultFields::from_observations(&observations);
            assert!(matches!(
                resolve_result_performance(&parsed, 764, 1_286),
                ResultPerformanceResolution::Unknown {
                    reason: ResultPerformanceUnknownReason::JudgmentExceedsNotes,
                    ..
                }
            ));
        }
    }

    #[test]
    fn no_play_normalizes_all_previous_best_fields() {
        let mut observations = result_fields();
        observations.previous_clear_type = text("NO PLAY");
        observations.previous_score = text("0");
        observations.previous_miss_count = text("--");
        let parsed = ParsedResultFields::from_observations(&observations);
        assert_eq!(parsed.previous_clear_type, PreviousBestValue::NotPlayed);
        assert_eq!(parsed.previous_score, PreviousBestValue::NotPlayed);
        assert_eq!(parsed.previous_miss_count, PreviousBestValue::NotPlayed);
    }

    #[test]
    fn previous_best_preserves_field_specific_missing_states() {
        let mut observations = result_fields();
        observations.previous_score = text("SCORE");
        observations.previous_miss_count = text("--");
        let parsed = ParsedResultFields::from_observations(&observations);
        assert_eq!(
            parsed.previous_score,
            PreviousBestValue::Unknown {
                reason: ResultFieldUnknownReason::InvalidFormat
            }
        );
        assert_eq!(parsed.previous_miss_count, PreviousBestValue::NotDisplayed);
    }

    #[test]
    fn score_breakdown_overflow_fails_closed() {
        let mut observations = result_fields();
        observations.pgreat = text(&u32::MAX.to_string());
        observations.great = text("1");
        let parsed = ParsedResultFields::from_observations(&observations);
        assert!(matches!(
            resolve_result_performance(&parsed, u32::MAX, u32::MAX),
            ResultPerformanceResolution::Unknown {
                reason: ResultPerformanceUnknownReason::ScoreBreakdownMismatch,
                ..
            }
        ));
    }
}
