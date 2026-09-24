//! Human-reviewed semantic truth for canonical corpus replay.

use std::collections::{BTreeMap, BTreeSet};

use scorepeek_core::canonical_recording::{CanonicalTick, TickDisposition};
use scorepeek_core::catalog::{Difficulty, PlayType, ScorepeekSongId};
use scorepeek_core::event::{
    DomainTransitionKind, MusicSelectionState, ResultDomainEvent, ResultState,
};
use scorepeek_core::recognition::music_select::PlaySide;
use scorepeek_core::recognition::result::{
    PlayOption, PlayOptions, PreviousBest, PreviousBestValue, ResultJudgments, ResultTiming,
    SupplementalResultValue, resolve_clear_type,
};
use scorepeek_core::recognition::screen::ScreenClass;
use serde::{Deserialize, Serialize};

use crate::replay::{ReplayError, ReplayObserver};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegressionEpisode {
    pub episode_id: String,
    pub expected_song_id: String,
    pub expected_clear_type: String,
    pub expected_result: ExpectedResult,
    pub stable_sequences: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<AttemptTruth>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AttemptTruth {
    pub attempt_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_attempt_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select_span: Option<SequenceSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decide_span: Option<SequenceSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub play_span: Option<SequenceSpan>,
    pub result_span: SequenceSpan,
    pub outcome: AttemptOutcome,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SequenceSpan {
    pub first_sequence: u64,
    pub last_sequence: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Accepted,
    Abandoned,
    Unlinked,
    NoResult,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedResult {
    pub play_side: String,
    pub play_mode: String,
    pub play_type: PlayType,
    pub difficulty: Difficulty,
    pub level: u8,
    pub notes: u32,
    pub current_score: u32,
    pub judgments: Option<ResultJudgments>,
    pub miss_count: Option<SupplementalResultValue<u32>>,
    pub timing: Option<ResultTiming>,
    pub combo_break: Option<SupplementalResultValue<u32>>,
    pub previous_best: Option<PreviousBest>,
    pub play_options: Option<Vec<PlayOption>>,
}

fn invalid(message: &'static str) -> Result<(), &'static str> {
    Err(message)
}

fn valid_expected_result(expected: &ExpectedResult) -> bool {
    let Some(judgments) = expected.judgments.as_ref() else {
        return false;
    };
    let Some(previous) = expected.previous_best.as_ref() else {
        return false;
    };
    let Some(options) = expected.play_options.as_deref() else {
        return false;
    };
    let not_played = [
        matches!(previous.clear_type, PreviousBestValue::NotPlayed),
        matches!(previous.score, PreviousBestValue::NotPlayed),
        matches!(previous.miss_count, PreviousBestValue::NotPlayed),
    ];
    expected.notes > 0
        && u64::from(expected.current_score) <= u64::from(expected.notes) * 2
        && judgments
            .pgreat
            .checked_mul(2)
            .and_then(|score| score.checked_add(judgments.great))
            == Some(expected.current_score)
        && [
            judgments.pgreat,
            judgments.great,
            judgments.good,
            judgments.bad,
        ]
        .into_iter()
        .all(|value| value <= expected.notes)
        && expected.miss_count.is_some()
        && expected.timing.is_some()
        && expected.combo_break.is_some()
        && options.len() <= PlayOption::ALL.len()
        && options.iter().copied().collect::<BTreeSet<_>>().len() == options.len()
        && match &previous.clear_type {
            PreviousBestValue::Known { value } => resolve_clear_type(value) == Some(value.as_str()),
            _ => true,
        }
        && match &previous.score {
            PreviousBestValue::Known { value } => *value <= expected.notes.saturating_mul(2),
            _ => true,
        }
        && (not_played.into_iter().all(|value| value) || not_played.into_iter().all(|value| !value))
}

fn valid_screen_span(
    ticks: &BTreeMap<u64, &CanonicalTick>,
    span: SequenceSpan,
    screen: ScreenClass,
    complete_interior: bool,
) -> bool {
    if span.first_sequence > span.last_sequence {
        return false;
    }
    let endpoint_matches = |sequence| {
        ticks.get(&sequence).is_some_and(|tick| {
            tick.disposition == TickDisposition::Retained
                && (tick.screen == screen
                    || (screen == ScreenClass::Play && tick.screen == ScreenClass::Unknown))
        })
    };
    if !endpoint_matches(span.first_sequence) || !endpoint_matches(span.last_sequence) {
        return false;
    }
    !complete_interior
        || (span.first_sequence..=span.last_sequence).all(|sequence| {
            ticks.get(&sequence).is_some_and(|tick| {
                tick.disposition == TickDisposition::Retained && tick.screen == screen
            })
        })
}

/// Verifies that the reviewed facts bind to complete canonical input chronology.
///
/// # Errors
/// Rejects missing frames, invalid expected fields, or inconsistent episode spans.
#[allow(
    clippy::too_many_lines,
    reason = "reviews one episode's coupled expected result and ordered screen spans"
)]
pub fn validate_label(
    episodes: &[RegressionEpisode],
    negatives: &[u64],
    ticks: &[CanonicalTick],
) -> Result<(), &'static str> {
    let by_sequence = ticks
        .iter()
        .map(|tick| (tick.sequence, tick))
        .collect::<BTreeMap<_, _>>();
    let retained = ticks
        .iter()
        .filter(|tick| tick.disposition == TickDisposition::Retained)
        .map(|tick| tick.sequence)
        .collect::<BTreeSet<_>>();
    let mut used = BTreeSet::new();
    let mut keys = BTreeSet::new();
    let mut previous = None;
    for episode in episodes {
        let Some(attempt) = &episode.attempt else {
            return invalid("reviewed attempt is missing");
        };
        let expected = &episode.expected_result;
        if episode.episode_id.is_empty()
            || serde_json::from_value::<ScorepeekSongId>(serde_json::Value::String(
                episode.expected_song_id.clone(),
            ))
            .is_err()
            || episode.expected_clear_type.is_empty()
            || !matches!(expected.play_side.as_str(), "one_player" | "two_player")
            || !matches!(
                (expected.play_mode.as_str(), expected.play_type),
                ("single_play", PlayType::Single) | ("double_play", PlayType::Double)
            )
            || !(1..=12).contains(&expected.level)
            || !valid_expected_result(expected)
            || episode.stable_sequences.is_empty()
            || episode
                .stable_sequences
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || previous.is_some_and(|end| episode.stable_sequences[0] <= end)
            || episode
                .stable_sequences
                .iter()
                .any(|seq| !retained.contains(seq) || !used.insert(*seq))
            || attempt.attempt_key.is_empty()
            || !keys.insert(attempt.attempt_key.clone())
            || attempt
                .parent_attempt_key
                .as_ref()
                .is_some_and(|key| !keys.contains(key))
        {
            return invalid("reviewed episode is invalid");
        }
        let spans = [
            attempt.select_span,
            attempt.decide_span,
            attempt.play_span,
            Some(attempt.result_span),
        ];
        let screens = [
            ScreenClass::MusicSelect,
            ScreenClass::DecideTransition,
            ScreenClass::Play,
            ScreenClass::Result,
        ];
        for (index, (span, screen)) in spans.into_iter().zip(screens).enumerate() {
            let Some(span) = span else {
                return invalid("reviewed span is missing");
            };
            if !valid_screen_span(&by_sequence, span, screen, matches!(index, 1 | 3)) {
                return invalid("reviewed span differs");
            }
        }
        let (Some(select), Some(decide), Some(play)) =
            (attempt.select_span, attempt.decide_span, attempt.play_span)
        else {
            return invalid("reviewed span is missing");
        };
        if select.last_sequence >= decide.first_sequence
            || decide.last_sequence >= play.first_sequence
            || play.last_sequence >= attempt.result_span.first_sequence
            || episode.stable_sequences.iter().any(|seq| {
                *seq < attempt.result_span.first_sequence
                    || *seq > attempt.result_span.last_sequence
            })
        {
            return invalid("reviewed span chronology differs");
        }
        previous = episode.stable_sequences.last().copied();
    }
    if negatives
        .iter()
        .any(|seq| !retained.contains(seq) || !used.insert(*seq))
    {
        return invalid("reviewed negative frame is invalid");
    }
    Ok(())
}

fn result_event_matches(event: &ResultDomainEvent, episode: &RegressionEpisode) -> bool {
    let expected = &episode.expected_result;
    event.scorepeek_song_id.as_uuid().to_string() == episode.expected_song_id
        && event.clear_type == episode.expected_clear_type
        && matches!(
            (event.play_side, expected.play_side.as_str()),
            (PlaySide::OnePlayer, "one_player") | (PlaySide::TwoPlayer, "two_player")
        )
        && event.play_type == expected.play_type
        && event.difficulty == expected.difficulty
        && event.level == expected.level
        && event.notes == expected.notes
        && event.current_score == expected.current_score
        && expected.judgments.as_ref() == Some(&event.judgments)
        && expected.miss_count.as_ref() == Some(&event.miss_count)
        && expected.timing.as_ref() == Some(&event.timing)
        && expected.combo_break.as_ref() == Some(&event.combo_break)
        && expected.previous_best.as_ref() == Some(&event.previous_best)
        && matches!((&event.play_options, expected.play_options.as_deref()), (PlayOptions::Known { values }, Some(expected)) if values == expected)
}

pub struct OracleObserver<'a> {
    episodes: &'a [RegressionEpisode],
    negatives: &'a [u64],
    seen_stable: BTreeSet<u64>,
    seen_negative: BTreeSet<u64>,
    latest_select: Vec<Option<MusicSelectionState>>,
    results: Vec<(u64, ResultDomainEvent)>,
}

impl<'a> OracleObserver<'a> {
    #[must_use]
    pub fn new(episodes: &'a [RegressionEpisode], negatives: &'a [u64]) -> Self {
        Self {
            episodes,
            negatives,
            seen_stable: BTreeSet::new(),
            seen_negative: BTreeSet::new(),
            latest_select: vec![None; episodes.len()],
            results: Vec::new(),
        }
    }

    /// Checks that every reviewed observation and domain event was seen.
    ///
    /// # Errors
    /// Reports the first semantic mismatch with its reviewed episode identity.
    pub fn finish(self) -> Result<(), ReplayError> {
        let accepted = self
            .episodes
            .iter()
            .enumerate()
            .filter(|(_, episode)| {
                episode
                    .attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.outcome == AttemptOutcome::Accepted)
            })
            .collect::<Vec<_>>();
        if self.results.len() != accepted.len() {
            return Err(ReplayError::Oracle(format!(
                "reviewed RESULT count differs: expected {}, observed {}",
                accepted.len(),
                self.results.len()
            )));
        }
        let mut ids = BTreeMap::new();
        for ((index, episode), (source_sequence, event)) in accepted.into_iter().zip(&self.results)
        {
            let Some(attempt) = episode.attempt.as_ref() else {
                return Err(ReplayError::Invalid("reviewed attempt is missing"));
            };
            let next_result_start = self
                .episodes
                .get(index + 1)
                .and_then(|next| next.attempt.as_ref())
                .map(|next| next.result_span.first_sequence);
            if *source_sequence < attempt.result_span.first_sequence
                || next_result_start.is_some_and(|next| *source_sequence >= next)
            {
                return Err(ReplayError::Oracle(format!(
                    "episode {} RESULT sequence {} differs from reviewed chronology starting at {} and ending before {:?}",
                    episode.episode_id,
                    source_sequence,
                    attempt.result_span.first_sequence,
                    next_result_start
                )));
            }
            if !result_event_matches(event, episode) {
                return Err(ReplayError::Oracle(format!(
                    "episode {} reviewed RESULT payload differs",
                    episode.episode_id
                )));
            }
            let parent = attempt
                .parent_attempt_key
                .as_ref()
                .and_then(|key| ids.get(key))
                .copied();
            if event.parent_attempt_id != parent {
                return Err(ReplayError::Oracle(format!(
                    "episode {} RESULT parent differs",
                    episode.episode_id
                )));
            }
            ids.insert(attempt.attempt_key.clone(), event.attempt_id);
        }
        for (index, episode) in self.episodes.iter().enumerate() {
            for seq in &episode.stable_sequences {
                if !self.seen_stable.contains(seq) {
                    return Err(ReplayError::Oracle(format!(
                        "episode {} stable frame {} was not checked",
                        episode.episode_id, seq
                    )));
                }
            }
            let expected = &episode.expected_result;
            if !matches!(&self.latest_select[index], Some(MusicSelectionState::Selected { scorepeek_song_id, play_type, difficulty, level, notes, .. })
                if scorepeek_song_id.as_uuid().to_string() == episode.expected_song_id && *play_type == expected.play_type && *difficulty == expected.difficulty && *level == expected.level && *notes == expected.notes)
            {
                return Err(ReplayError::Oracle(format!(
                    "episode {} reviewed SELECT differs",
                    episode.episode_id
                )));
            }
        }
        if self.seen_negative.len() != self.negatives.len() {
            return Err(ReplayError::Oracle(
                "reviewed negative frame was not checked".into(),
            ));
        }
        Ok(())
    }
}

impl ReplayObserver for OracleObserver<'_> {
    fn validate(&mut self, ticks: &[CanonicalTick]) -> Result<(), ReplayError> {
        validate_label(self.episodes, self.negatives, ticks).map_err(ReplayError::Invalid)?;
        if self.episodes.is_empty() && ticks.iter().any(|tick| tick.screen == ScreenClass::Result) {
            return Err(ReplayError::Invalid(
                "included RESULT recording has no reviewed episodes",
            ));
        }
        Ok(())
    }
    fn screen(&mut self, sequence: u64, screen: ScreenClass) -> Result<(), ReplayError> {
        if self.negatives.contains(&sequence) {
            if screen != ScreenClass::Unknown {
                return Err(ReplayError::Oracle(format!(
                    "negative frame {sequence} is no longer unknown"
                )));
            }
            self.seen_negative.insert(sequence);
        }
        for episode in self.episodes {
            if episode.stable_sequences.contains(&sequence) {
                if screen != ScreenClass::Result {
                    return Err(ReplayError::Oracle(format!(
                        "episode {} stable frame {sequence} is not RESULT",
                        episode.episode_id
                    )));
                }
                self.seen_stable.insert(sequence);
            }
            if episode
                .attempt
                .as_ref()
                .and_then(|attempt| attempt.play_span)
                .is_some_and(|span| {
                    sequence == span.first_sequence || sequence == span.last_sequence
                })
                && screen != ScreenClass::Play
            {
                return Err(ReplayError::Oracle(format!(
                    "episode {} PLAY endpoint {sequence} differs",
                    episode.episode_id
                )));
            }
        }
        Ok(())
    }

    fn event(&mut self, event: &DomainTransitionKind) -> Result<(), ReplayError> {
        match event {
            DomainTransitionKind::MusicSelectionChanged {
                source_sequence,
                state,
                ..
            } => {
                for (index, episode) in self.episodes.iter().enumerate() {
                    if episode
                        .attempt
                        .as_ref()
                        .and_then(|attempt| attempt.select_span)
                        .is_some_and(|span| *source_sequence <= span.last_sequence)
                    {
                        self.latest_select[index] = Some(state.clone());
                    }
                }
            }
            DomainTransitionKind::ResultChanged {
                source_sequence,
                state: ResultState::Confirmed { result, .. },
                ..
            } => {
                if self.results.len() >= self.episodes.len() {
                    return Err(ReplayError::Oracle(
                        "too many confirmed RESULT events".into(),
                    ));
                }
                self.results.push((*source_sequence, (**result).clone()));
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reviewed_result_truth_rejects_changed_score_and_missing_span() {
        let song = "123e4567-e89b-12d3-a456-426614174000";
        let expected = json!({
            "play_side":"one_player", "play_mode":"single_play", "play_type":"single",
            "difficulty":"hyper", "level":7, "notes":100, "current_score":200,
            "judgments":{"pgreat":100,"great":0,"good":0,"bad":0,"poor":0},
            "miss_count":{"status":"not_displayed"},
            "timing":{"fast":{"status":"not_displayed"},"slow":{"status":"not_displayed"}},
            "combo_break":{"status":"not_displayed"},
            "previous_best":{"clear_type":{"status":"not_played"},"score":{"status":"not_played"},"miss_count":{"status":"not_played"}},
            "play_options":[]
        });
        let episode: RegressionEpisode = serde_json::from_value(json!({
            "episode_id":"synthetic-episode", "expected_song_id":song,
            "expected_clear_type":"CLEAR", "expected_result":expected,
            "stable_sequences":[4], "attempt":{"attempt_key":"synthetic-attempt",
                "select_span":{"first_sequence":1,"last_sequence":1},
                "decide_span":{"first_sequence":2,"last_sequence":2},
                "play_span":{"first_sequence":3,"last_sequence":3},
                "result_span":{"first_sequence":4,"last_sequence":4},
                "outcome":"accepted"}
        }))
        .unwrap();
        let ticks = ["music_select", "decide_transition", "play", "result"]
            .into_iter()
            .enumerate()
            .map(|(index, screen)| {
                serde_json::from_value::<CanonicalTick>(json!({
                "sequence":index+1, "source_sequence":index+1, "source_timestamp_ms":100*(index+1),
                "screen":screen, "semantic_episode_id":1, "disposition":{"kind":"retained"}
            })).unwrap()
            })
            .collect::<Vec<_>>();
        assert!(validate_label(std::slice::from_ref(&episode), &[], &ticks).is_ok());
        let mut missing = ticks.clone();
        missing.pop();
        assert!(validate_label(std::slice::from_ref(&episode), &[], &missing).is_err());
        let mut event_value = expected;
        event_value["contract"] = json!("scorepeek-result-detected-v4");
        event_value["attempt_id"] = json!(1);
        event_value["scorepeek_song_id"] = json!(song);
        event_value["clear_type"] = json!("CLEAR");
        event_value["play_options"] = json!({"status":"known","values":[]});
        let event: ResultDomainEvent = serde_json::from_value(event_value).unwrap();
        assert!(result_event_matches(&event, &episode));
        let mut changed = event;
        changed.current_score -= 1;
        assert!(!result_event_matches(&changed, &episode));
        changed.current_score += 1;
        let episodes = [episode];
        let mut oracle = OracleObserver::new(&episodes, &[]);
        oracle.results.push((3, changed));
        assert!(
            matches!(oracle.finish(), Err(ReplayError::Oracle(message)) if message.contains("RESULT sequence 3 differs"))
        );
    }
}
