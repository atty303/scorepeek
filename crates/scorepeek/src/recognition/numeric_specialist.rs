use std::cmp::Ordering;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::ctc_sequence::CtcSequenceTrie;

pub const NUMERIC_DICTIONARY: &str = "0123456789-";
pub const NUMERIC_BLANK_INDEX: usize = 11;
pub const NUMERIC_TOP_CANDIDATES: usize = 8;
pub const FIXED_SLOT_CLASSES: &str = "_0123456789";
pub const FIXED_SLOT_CLASS_COUNT: usize = 11;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericField {
    Level,
    Notes,
    CurrentScore,
    PreviousScore,
    PreviousMissCount,
    MissCount,
    Pgreat,
    Great,
    Good,
    Bad,
    Poor,
    Fast,
    Slow,
    ComboBreak,
}

impl NumericField {
    pub const ALL: [Self; 14] = [
        Self::Level,
        Self::Notes,
        Self::CurrentScore,
        Self::PreviousScore,
        Self::PreviousMissCount,
        Self::MissCount,
        Self::Pgreat,
        Self::Great,
        Self::Good,
        Self::Bad,
        Self::Poor,
        Self::Fast,
        Self::Slow,
        Self::ComboBreak,
    ];

    #[must_use]
    pub const fn maximum_digits(self) -> usize {
        match self {
            Self::Level => 2,
            Self::ComboBreak => 3,
            _ => 4,
        }
    }

    #[must_use]
    pub const fn allows_dash(self) -> bool {
        matches!(
            self,
            Self::PreviousScore
                | Self::PreviousMissCount
                | Self::MissCount
                | Self::Fast
                | Self::Slow
                | Self::ComboBreak
        )
    }

    #[must_use]
    pub const fn allows_leading_zeroes(self) -> bool {
        matches!(self, Self::Notes)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NumericCandidate {
    pub text: String,
    pub log_probability: f32,
    pub calibrated_probability: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct NumericFieldInference {
    pub field: NumericField,
    pub calibration: NumericCalibration,
    pub accepted: bool,
    #[serde(default)]
    pub raw_text: String,
    pub candidates: Vec<NumericCandidate>,
    pub all_blank_log_probability: f32,
    pub runner_up_margin: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScoreBreakdownCandidate {
    pub current_score: u32,
    pub pgreat: u32,
    pub great: u32,
    pub joint_log_probability: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ScoreBreakdownDecision {
    pub accepted: Option<ScoreBreakdownCandidate>,
    pub candidates: Vec<ScoreBreakdownCandidate>,
    pub runner_up_margin: Option<f32>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct NumericCalibration {
    pub enabled: bool,
    pub temperature: f32,
    pub minimum_probability: f32,
    pub minimum_runner_up_margin: f32,
}

impl NumericCalibration {
    #[must_use]
    pub fn accepts(self, inference: &NumericFieldInference) -> bool {
        if !self.enabled {
            return false;
        }
        let Some(best) = inference.candidates.first() else {
            return false;
        };
        best.log_probability > inference.all_blank_log_probability
            && best.calibrated_probability >= self.minimum_probability
            && inference
                .runner_up_margin
                .is_some_and(|margin| margin >= self.minimum_runner_up_margin)
    }
}

/// Scores every sequence admitted by the field grammar with exact CTC forward probability.
/// `logits` is row-major `[timesteps, 12]`: dictionary order followed by blank.
///
/// # Errors
///
/// Returns an error for an invalid tensor shape or calibration temperature.
pub fn rank_numeric_sequences(
    field: NumericField,
    logits: &[f32],
    timesteps: usize,
    calibration: NumericCalibration,
) -> Result<NumericFieldInference, &'static str> {
    let classes = NUMERIC_BLANK_INDEX + 1;
    if timesteps == 0 || logits.len() != timesteps.saturating_mul(classes) {
        return Err("numeric logits have an invalid shape");
    }
    if !calibration.temperature.is_finite() || calibration.temperature <= 0.0 {
        return Err("numeric calibration temperature must be positive");
    }
    let probabilities =
        softmax_ctc_rows_blank_first(logits, timesteps, classes, calibration.temperature);
    let raw_text = greedy_decode_dictionary_first(logits, classes);
    rank_numeric_probabilities_inner(field, &probabilities, calibration, raw_text)
}

/// Scores model probabilities in Paddle CTC order: blank followed by the registered dictionary.
///
/// # Errors
///
/// Returns an error for an invalid tensor shape, calibration, probability, or CTC score.
pub fn rank_numeric_probabilities(
    field: NumericField,
    probabilities: &[f32],
    timesteps: usize,
    calibration: NumericCalibration,
) -> Result<NumericFieldInference, &'static str> {
    let classes = NUMERIC_BLANK_INDEX + 1;
    if timesteps == 0 || probabilities.len() != timesteps.saturating_mul(classes) {
        return Err("numeric probabilities have an invalid shape");
    }
    if !calibration.temperature.is_finite() || calibration.temperature <= 0.0 {
        return Err("numeric calibration temperature must be positive");
    }
    let mut calibrated = Vec::with_capacity(probabilities.len());
    for row in probabilities.chunks_exact(classes) {
        if row.iter().any(|value| !value.is_finite() || *value < 0.0) {
            return Err("numeric probabilities contain an invalid value");
        }
        let sum = row.iter().sum::<f32>();
        if (sum - 1.0).abs() > 1e-3 {
            return Err("numeric probability row is not normalized");
        }
        let scaled = row
            .iter()
            .map(|value| {
                if *value == 0.0 {
                    f32::NEG_INFINITY
                } else {
                    value.ln() / calibration.temperature
                }
            })
            .collect::<Vec<_>>();
        let maximum = scaled.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let normalizer = scaled
            .iter()
            .map(|value| (*value - maximum).exp())
            .sum::<f32>();
        calibrated.extend(
            scaled
                .into_iter()
                .map(|value| (value - maximum).exp() / normalizer),
        );
    }
    let raw_text = greedy_decode_blank_first(probabilities, classes);
    rank_numeric_probabilities_inner(field, &calibrated, calibration, raw_text)
}

/// Scores a fixed number of independently classified character cells.
///
/// `logits` is row-major `[slots, 11]` in `_0123456789` order. The grammar admits only leading
/// blank cells followed by contiguous decimal digits; notes always retain all four displayed
/// digits. There is no CTC collapse and no dash class.
///
/// # Errors
/// Returns an error for an invalid tensor shape or calibration.
pub fn rank_fixed_slot_logits(
    field: NumericField,
    logits: &[f32],
    slots: usize,
    calibration: NumericCalibration,
) -> Result<NumericFieldInference, &'static str> {
    if slots == 0 || logits.len() != slots.saturating_mul(FIXED_SLOT_CLASS_COUNT) {
        return Err("fixed-slot logits have an invalid shape");
    }
    if !calibration.temperature.is_finite() || calibration.temperature <= 0.0 {
        return Err("fixed-slot calibration temperature must be positive");
    }
    let probabilities = softmax_rows(
        logits,
        slots,
        FIXED_SLOT_CLASS_COUNT,
        calibration.temperature,
    );
    let mut ranked = fixed_slot_sequences(field, slots)
        .into_iter()
        .map(|(text, tokens)| {
            let log_probability = tokens
                .into_iter()
                .enumerate()
                .map(|(slot, token)| probabilities[slot * FIXED_SLOT_CLASS_COUNT + token].ln())
                .sum::<f32>();
            (text, log_probability)
        })
        .collect::<Vec<_>>();
    if ranked.len() < 2 {
        return Err("fixed-slot grammar has fewer than two sequences");
    }
    let normalizer = log_sum_exp(&ranked.iter().map(|(_, score)| *score).collect::<Vec<_>>());
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    ranked.truncate(NUMERIC_TOP_CANDIDATES);
    let candidates = ranked
        .into_iter()
        .map(|(text, log_probability)| NumericCandidate {
            text,
            log_probability,
            calibrated_probability: (log_probability - normalizer).exp(),
        })
        .collect::<Vec<_>>();
    let all_blank_log_probability = (0..slots)
        .map(|slot| probabilities[slot * FIXED_SLOT_CLASS_COUNT].ln())
        .sum();
    let runner_up_margin = candidates
        .first()
        .zip(candidates.get(1))
        .map(|(best, runner_up)| best.log_probability - runner_up.log_probability);
    let raw_text = probabilities
        .chunks_exact(FIXED_SLOT_CLASS_COUNT)
        .map(|row| {
            let selected = row
                .iter()
                .enumerate()
                .max_by(|left, right| left.1.partial_cmp(right.1).unwrap_or(Ordering::Equal))
                .map_or(0, |(index, _)| index);
            char::from(FIXED_SLOT_CLASSES.as_bytes()[selected])
        })
        .collect();
    let mut inference = NumericFieldInference {
        field,
        calibration,
        accepted: false,
        raw_text,
        candidates,
        all_blank_log_probability,
        runner_up_margin,
    };
    inference.accepted = calibration.accepts(&inference);
    Ok(inference)
}

fn fixed_slot_sequences(field: NumericField, slots: usize) -> Vec<(String, Vec<usize>)> {
    if field == NumericField::Notes {
        return (0..10_u32.pow(u32::try_from(slots).unwrap_or(u32::MAX)))
            .map(|value| {
                let text = format!("{value:0slots$}");
                let tokens = text
                    .bytes()
                    .map(|byte| usize::from(byte - b'0') + 1)
                    .collect();
                (text, tokens)
            })
            .collect();
    }
    let (minimum, maximum) = if field == NumericField::Level {
        if slots == 1 { (1, 9) } else { (10, 12) }
    } else {
        (0, 10_u32.pow(u32::try_from(slots).unwrap_or(u32::MAX)) - 1)
    };
    (minimum..=maximum)
        .map(|value| {
            let text = value.to_string();
            let mut tokens = vec![0; slots - text.len()];
            tokens.extend(text.bytes().map(|byte| usize::from(byte - b'0') + 1));
            (text, tokens)
        })
        .collect()
}

#[allow(clippy::cast_possible_truncation)]
fn rank_numeric_probabilities_inner(
    field: NumericField,
    probabilities_blank_first: &[f32],
    calibration: NumericCalibration,
    raw_text: String,
) -> Result<NumericFieldInference, &'static str> {
    let classes = NUMERIC_BLANK_INDEX + 1;
    let ctc_scores = numeric_trie(field)
        .score(probabilities_blank_first, classes)
        .ok_or("numeric CTC trie rejected model probabilities")?;
    let all_blank_log_probability = ctc_scores.blank_log_probability as f32;
    let mut ranked = ctc_scores
        .values
        .into_iter()
        .map(|(text, score)| (text.clone(), score as f32))
        .collect::<Vec<_>>();
    let normalizer = log_sum_exp(
        &ranked
            .iter()
            .map(|(_, score)| *score)
            .chain(std::iter::once(all_blank_log_probability))
            .collect::<Vec<_>>(),
    );
    ranked.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
    ranked.truncate(NUMERIC_TOP_CANDIDATES);
    let candidates = ranked
        .into_iter()
        .map(|(text, log_probability)| NumericCandidate {
            text,
            log_probability,
            calibrated_probability: (log_probability - normalizer).exp(),
        })
        .collect::<Vec<_>>();
    let runner_up_margin = candidates
        .first()
        .zip(candidates.get(1))
        .map(|(best, runner_up)| best.log_probability - runner_up.log_probability);
    let mut inference = NumericFieldInference {
        field,
        calibration,
        accepted: false,
        raw_text,
        candidates,
        all_blank_log_probability,
        runner_up_margin,
    };
    inference.accepted = calibration.accepts(&inference);
    Ok(inference)
}

fn greedy_decode_blank_first(probabilities: &[f32], classes: usize) -> String {
    let mut output = String::new();
    let mut previous = None;
    for row in probabilities.chunks_exact(classes) {
        let mut selected = 0;
        for (index, probability) in row.iter().copied().enumerate().skip(1) {
            if probability > row[selected] {
                selected = index;
            }
        }
        if selected != 0 && previous != Some(selected) {
            output.push(char::from(NUMERIC_DICTIONARY.as_bytes()[selected - 1]));
        }
        previous = Some(selected);
    }
    output
}

fn greedy_decode_dictionary_first(values: &[f32], classes: usize) -> String {
    let mut output = String::new();
    let mut previous = None;
    for row in values.chunks_exact(classes) {
        let mut selected = NUMERIC_BLANK_INDEX;
        for (index, value) in row[..NUMERIC_BLANK_INDEX].iter().copied().enumerate() {
            if value > row[selected] {
                selected = index;
            }
        }
        if selected != NUMERIC_BLANK_INDEX && previous != Some(selected) {
            output.push(char::from(NUMERIC_DICTIONARY.as_bytes()[selected]));
        }
        previous = Some(selected);
    }
    output
}

#[must_use]
pub fn select_score_breakdown(
    notes: Option<u32>,
    current_score: &NumericFieldInference,
    pgreat: &NumericFieldInference,
    great: &NumericFieldInference,
    minimum_joint_margin: f32,
) -> ScoreBreakdownDecision {
    let mut candidates = Vec::new();
    for score in &current_score.candidates {
        let Ok(score_value) = score.text.parse::<u32>() else {
            continue;
        };
        if notes.is_some_and(|notes| score_value > notes.saturating_mul(2)) {
            continue;
        }
        for pgreat in &pgreat.candidates {
            let Ok(pgreat_value) = pgreat.text.parse::<u32>() else {
                continue;
            };
            if notes.is_some_and(|notes| pgreat_value > notes) {
                continue;
            }
            for great in &great.candidates {
                let Ok(great_value) = great.text.parse::<u32>() else {
                    continue;
                };
                if notes.is_some_and(|notes| great_value > notes)
                    || pgreat_value
                        .checked_mul(2)
                        .and_then(|value| value.checked_add(great_value))
                        != Some(score_value)
                {
                    continue;
                }
                candidates.push(ScoreBreakdownCandidate {
                    current_score: score_value,
                    pgreat: pgreat_value,
                    great: great_value,
                    joint_log_probability: score.log_probability
                        + pgreat.log_probability
                        + great.log_probability,
                });
            }
        }
    }
    candidates.sort_by(|left, right| {
        right
            .joint_log_probability
            .partial_cmp(&left.joint_log_probability)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                (left.current_score, left.pgreat, left.great).cmp(&(
                    right.current_score,
                    right.pgreat,
                    right.great,
                ))
            })
    });
    let runner_up_margin = candidates
        .first()
        .zip(candidates.get(1))
        .map(|(best, runner_up)| best.joint_log_probability - runner_up.joint_log_probability);
    let accepted = candidates
        .first()
        .filter(|_| runner_up_margin.is_none_or(|margin| margin >= minimum_joint_margin))
        .cloned();
    candidates.truncate(NUMERIC_TOP_CANDIDATES);
    ScoreBreakdownDecision {
        accepted,
        candidates,
        runner_up_margin,
    }
}

fn for_each_numeric_sequence(
    maximum_digits: usize,
    allows_leading_zeroes: bool,
    mut visit: impl FnMut(String, Vec<usize>),
) {
    for length in 1..=maximum_digits {
        let count = 10_usize.pow(u32::try_from(length).expect("numeric length is bounded"));
        let first = if length == 1 || allows_leading_zeroes {
            0
        } else {
            10_usize.pow(u32::try_from(length - 1).expect("numeric length is bounded"))
        };
        for value in first..count {
            let text = format!("{value:0length$}");
            let tokens = text.bytes().map(|byte| usize::from(byte - b'0')).collect();
            visit(text, tokens);
        }
    }
}

fn numeric_trie(field: NumericField) -> &'static CtcSequenceTrie<String> {
    static LEVEL: OnceLock<CtcSequenceTrie<String>> = OnceLock::new();
    static NOTES: OnceLock<CtcSequenceTrie<String>> = OnceLock::new();
    static DIGITS: OnceLock<CtcSequenceTrie<String>> = OnceLock::new();
    static DISPLAY: OnceLock<CtcSequenceTrie<String>> = OnceLock::new();
    static COMBO: OnceLock<CtcSequenceTrie<String>> = OnceLock::new();
    let selected = match (
        field.maximum_digits(),
        field.allows_dash(),
        field.allows_leading_zeroes(),
    ) {
        (2, false, false) => &LEVEL,
        (4, false, true) => &NOTES,
        (4, false, false) => &DIGITS,
        (4, true, false) => &DISPLAY,
        (3, true, false) => &COMBO,
        _ => unreachable!("all numeric field grammars are registered"),
    };
    selected.get_or_init(|| {
        let mut trie = CtcSequenceTrie::default();
        for_each_numeric_sequence(
            field.maximum_digits(),
            field.allows_leading_zeroes(),
            |text, tokens| {
                let tokens = tokens
                    .into_iter()
                    .map(|token| u32::try_from(token + 1).expect("numeric token is bounded"))
                    .collect::<Vec<_>>();
                assert!(trie.insert(&tokens, text));
            },
        );
        if field.allows_dash() {
            assert!(trie.insert(&[11, 11], "--".to_owned()));
        }
        trie
    })
}

fn softmax_ctc_rows_blank_first(
    logits: &[f32],
    rows: usize,
    columns: usize,
    temperature: f32,
) -> Vec<f32> {
    let mut output = Vec::with_capacity(logits.len());
    for row in logits.chunks_exact(columns).take(rows) {
        let maximum = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let normalizer = row
            .iter()
            .map(|value| ((*value - maximum) / temperature).exp())
            .sum::<f32>();
        output.push(((row[NUMERIC_BLANK_INDEX] - maximum) / temperature).exp() / normalizer);
        output.extend(
            row[..NUMERIC_BLANK_INDEX]
                .iter()
                .map(|value| ((*value - maximum) / temperature).exp() / normalizer),
        );
    }
    output
}

fn softmax_rows(logits: &[f32], rows: usize, columns: usize, temperature: f32) -> Vec<f32> {
    let mut output = Vec::with_capacity(logits.len());
    for row in logits.chunks_exact(columns).take(rows) {
        let maximum = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let normalizer = row
            .iter()
            .map(|value| ((*value - maximum) / temperature).exp())
            .sum::<f32>();
        output.extend(
            row.iter()
                .map(|value| ((*value - maximum) / temperature).exp() / normalizer),
        );
    }
    output
}

fn log_sum_exp(values: &[f32]) -> f32 {
    let maximum = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if maximum == f32::NEG_INFINITY {
        return maximum;
    }
    maximum
        + values
            .iter()
            .map(|value| (*value - maximum).exp())
            .sum::<f32>()
            .ln()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logits(path: &[usize]) -> Vec<f32> {
        let mut logits = vec![-8.0; path.len() * (NUMERIC_BLANK_INDEX + 1)];
        for (time, class) in path.iter().copied().enumerate() {
            logits[time * (NUMERIC_BLANK_INDEX + 1) + class] = 8.0;
        }
        logits
    }

    fn fixed_slot_logits(path: &[usize]) -> Vec<f32> {
        let mut logits = vec![-8.0; path.len() * FIXED_SLOT_CLASS_COUNT];
        for (slot, class) in path.iter().copied().enumerate() {
            logits[slot * FIXED_SLOT_CLASS_COUNT + class] = 8.0;
        }
        logits
    }

    const fn calibration() -> NumericCalibration {
        NumericCalibration {
            enabled: true,
            temperature: 1.0,
            minimum_probability: 0.0,
            minimum_runner_up_margin: 0.0,
        }
    }

    #[test]
    fn ranks_zero_over_blank_without_forcing_a_blank_crop() {
        let zero = logits(&[0, NUMERIC_BLANK_INDEX]);
        let ranked = rank_numeric_sequences(NumericField::Bad, &zero, 2, calibration()).unwrap();
        assert_eq!(ranked.candidates[0].text, "0");
        assert!(calibration().accepts(&ranked));
        assert!(
            !NumericCalibration {
                enabled: false,
                ..calibration()
            }
            .accepts(&ranked)
        );
        let blank = logits(&[NUMERIC_BLANK_INDEX, NUMERIC_BLANK_INDEX]);
        let ranked = rank_numeric_sequences(NumericField::Bad, &blank, 2, calibration()).unwrap();
        assert!(!calibration().accepts(&ranked));
    }

    #[test]
    fn fixed_slots_use_blank_first_eleven_class_logits() {
        let ranked = rank_fixed_slot_logits(
            NumericField::Bad,
            &fixed_slot_logits(&[0, 1]),
            2,
            calibration(),
        )
        .unwrap();
        assert_eq!(ranked.raw_text, "_0");
        assert_eq!(ranked.candidates[0].text, "0");

        let repeated = rank_fixed_slot_logits(
            NumericField::ComboBreak,
            &fixed_slot_logits(&[0, 4, 4]),
            3,
            calibration(),
        )
        .unwrap();
        assert_eq!(repeated.candidates[0].text, "33");
    }

    #[test]
    fn fixed_slot_grammar_keeps_notes_padding_and_level_range() {
        let notes = rank_fixed_slot_logits(
            NumericField::Notes,
            &fixed_slot_logits(&[1, 1, 8, 7]),
            4,
            calibration(),
        )
        .unwrap();
        assert_eq!(notes.candidates[0].text, "0076");

        let level = rank_fixed_slot_logits(
            NumericField::Level,
            &fixed_slot_logits(&[2, 3]),
            2,
            calibration(),
        )
        .unwrap();
        assert_eq!(level.candidates[0].text, "12");
        assert!(
            level
                .candidates
                .iter()
                .all(|candidate| candidate.text != "13")
        );
    }

    #[test]
    fn scores_paddle_blank_first_probabilities() {
        let mut probabilities = vec![0.0; 2 * (NUMERIC_BLANK_INDEX + 1)];
        probabilities[1] = 1.0;
        probabilities[NUMERIC_BLANK_INDEX + 1] = 1.0;
        let ranked =
            rank_numeric_probabilities(NumericField::Bad, &probabilities, 2, calibration())
                .unwrap();
        assert_eq!(ranked.candidates[0].text, "0");
    }

    #[test]
    fn preserves_repeated_digits_and_leading_zeroes() {
        let path = logits(&[0, NUMERIC_BLANK_INDEX, 0, 1]);
        let ranked = rank_numeric_sequences(NumericField::Notes, &path, 4, calibration()).unwrap();
        assert_eq!(ranked.candidates[0].text, "001");
        assert_eq!(ranked.raw_text, "001");
    }

    #[test]
    fn unrestricted_greedy_decode_preserves_grammar_rejected_text() {
        let path = logits(&[0, NUMERIC_BLANK_INDEX, 7, 7, NUMERIC_BLANK_INDEX, 10]);
        let ranked = rank_numeric_sequences(NumericField::Bad, &path, 6, calibration()).unwrap();
        assert_eq!(ranked.raw_text, "07-");
        assert!(
            ranked
                .candidates
                .iter()
                .all(|candidate| candidate.text != "07-")
        );
    }

    #[test]
    fn unrestricted_greedy_decode_uses_blank_first_tie_order() {
        let probabilities = vec![0.5, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let ranked =
            rank_numeric_probabilities(NumericField::Bad, &probabilities, 1, calibration())
                .unwrap();
        assert!(ranked.raw_text.is_empty());
    }

    #[test]
    fn unrestricted_raw_uses_uncalibrated_probability_argmax() {
        let blank = 0.499_f32;
        let digit = f32::from_bits(blank.to_bits() + 1);
        let mut probabilities = vec![0.0; NUMERIC_BLANK_INDEX + 1];
        probabilities[0] = blank;
        probabilities[1] = digit;
        probabilities[2] = 1.0 - blank - digit;
        let ranked = rank_numeric_probabilities(
            NumericField::CurrentScore,
            &probabilities,
            1,
            NumericCalibration {
                temperature: 2.0,
                ..calibration()
            },
        )
        .unwrap();
        assert_eq!(ranked.raw_text, "0");
    }

    #[test]
    fn excludes_leading_zeroes_from_judgment_grammar() {
        let path = logits(&[0, NUMERIC_BLANK_INDEX, 7]);
        let notes = rank_numeric_sequences(NumericField::Notes, &path, 3, calibration()).unwrap();
        assert!(
            notes
                .candidates
                .iter()
                .any(|candidate| candidate.text == "07")
        );
        let bad = rank_numeric_sequences(NumericField::Bad, &path, 3, calibration()).unwrap();
        assert!(
            bad.candidates
                .iter()
                .all(|candidate| candidate.text != "07")
        );
    }

    #[test]
    fn dash_is_only_admitted_by_display_grammar() {
        let path = logits(&[10, NUMERIC_BLANK_INDEX, 10]);
        let displayed =
            rank_numeric_sequences(NumericField::MissCount, &path, 3, calibration()).unwrap();
        assert_eq!(displayed.candidates[0].text, "--");
        let judgment = rank_numeric_sequences(NumericField::Bad, &path, 3, calibration()).unwrap();
        assert_ne!(judgment.candidates[0].text, "--");
    }

    #[test]
    fn field_grammars_cover_all_fourteen_inputs() {
        assert_eq!(NumericField::ALL.len(), 14);
        assert_eq!(NumericField::Level.maximum_digits(), 2);
        assert_eq!(NumericField::ComboBreak.maximum_digits(), 3);
        assert!(NumericField::PreviousScore.allows_dash());
        assert!(!NumericField::CurrentScore.allows_dash());
    }

    fn inference(field: NumericField, values: &[(&str, f32)]) -> NumericFieldInference {
        NumericFieldInference {
            field,
            calibration: calibration(),
            accepted: true,
            raw_text: values
                .first()
                .map_or(String::new(), |(text, _)| (*text).to_owned()),
            candidates: values
                .iter()
                .map(|(text, score)| NumericCandidate {
                    text: (*text).to_owned(),
                    log_probability: *score,
                    calibrated_probability: 1.0,
                })
                .collect(),
            all_blank_log_probability: -100.0,
            runner_up_margin: Some(1.0),
        }
    }

    #[test]
    fn joint_score_breakdown_rejects_1303_and_selects_1383() {
        let decision = select_score_breakdown(
            Some(764),
            &inference(
                NumericField::CurrentScore,
                &[("1303", -1.0), ("1383", -2.0)],
            ),
            &inference(NumericField::Pgreat, &[("630", -0.1)]),
            &inference(NumericField::Great, &[("123", -0.1)]),
            0.0,
        );
        assert_eq!(decision.accepted.unwrap().current_score, 1383);
        assert!(
            decision
                .candidates
                .iter()
                .all(|value| value.current_score != 1303)
        );
    }

    #[test]
    fn score_breakdown_can_be_selected_before_catalog_notes_are_joined() {
        let decision = select_score_breakdown(
            None,
            &inference(NumericField::CurrentScore, &[("1383", -0.2)]),
            &inference(NumericField::Pgreat, &[("630", -0.1)]),
            &inference(NumericField::Great, &[("123", -0.1)]),
            0.0,
        );
        assert_eq!(decision.accepted.unwrap().current_score, 1383);
    }
}
