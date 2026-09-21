//! Test-only TUI projection for the portable music-select best state.

#![cfg(test)]

use std::fmt::Write as _;

use ratatui::text::Line;
use scorepeek_core::event::{
    BestOutputState, MusicSelectResolverState, SelectIdentityStatus, SelectionDifficultyTarget,
};
use scorepeek_core::recognition::{BestValue, PlaySide, StableBestField};

use super::server::{difficulty_label, fitted_value, play_type_label};

fn field_label<T>(field: &StableBestField<T>, show: impl FnOnce(&T) -> String) -> String {
    let value = match &field.observed {
        BestValue::Known(value) => show(value),
        BestValue::NoRecord => "no record".to_owned(),
        BestValue::NotDisplayed => "not displayed".to_owned(),
        BestValue::Unknown => if field.observed_once {
            "unknown"
        } else {
            "waiting"
        }
        .to_owned(),
    };
    if field.consecutive == 1 {
        format!("{value} (1/2)")
    } else {
        value
    }
}

fn best_value_label<T>(value: &BestValue<T>, show: impl FnOnce(&T) -> String) -> String {
    match value {
        BestValue::Known(value) => show(value),
        BestValue::NoRecord => "no record".to_owned(),
        BestValue::NotDisplayed => "not displayed".to_owned(),
        BestValue::Unknown => "unknown".to_owned(),
    }
}

fn output_line(state: &MusicSelectResolverState) -> String {
    let output = match state.output {
        BestOutputState::IdentityUnresolved => "waiting: identity",
        BestOutputState::Stabilizing => "waiting: values",
        BestOutputState::Partial => "partial snapshot emitted",
        BestOutputState::Complete => "snapshot emitted",
    };
    if matches!(
        state.output,
        BestOutputState::IdentityUnresolved | BestOutputState::Stabilizing
    ) {
        let reason = if state.suspended {
            "suspended"
        } else if state.output == BestOutputState::Stabilizing {
            "values"
        } else {
            match state.identity_status {
                SelectIdentityStatus::AwaitingDifficulty => "difficulty",
                SelectIdentityStatus::AwaitingPlayType => "SP/DP",
                SelectIdentityStatus::AwaitingEvidence => "evidence",
                SelectIdentityStatus::CurrentFrameConflict => "identity conflict",
                SelectIdentityStatus::Stabilizing | SelectIdentityStatus::Resolved => "identity",
            }
        };
        state.snapshot.as_ref().map_or_else(
            || format!("waiting: {reason}"),
            |snapshot| {
                format!(
                    "waiting: {reason}; last r{} S={} M={} {}",
                    snapshot.revision,
                    best_value_label(&snapshot.values.score, u32::to_string),
                    best_value_label(&snapshot.values.miss_count, u32::to_string),
                    best_value_label(&snapshot.values.clear_type, |value| format!("{value:?}"))
                )
            },
        )
    } else {
        format!("{output} revision={}", state.revision)
    }
}

const fn play_side_label(play_side: PlaySide) -> &'static str {
    match play_side {
        PlaySide::OnePlayer => "1P",
        PlaySide::TwoPlayer => "2P",
    }
}

pub fn lines(state: &MusicSelectResolverState, width: usize) -> Vec<Line<'static>> {
    if !state.active {
        return vec![Line::from("inactive")];
    }
    let phase = if state.suspended {
        "suspended"
    } else {
        "active"
    };
    let mut heading = format!(
        "{phase} ep#{} selection#{}",
        state.screen_episode_id, state.selection_interval
    );
    if state.chart.is_none()
        && let Some(current) = state.current_difficulty
    {
        let _ = write!(
            heading,
            " {} streak={} target={}",
            difficulty_label(current.difficulty),
            current.consecutive_known,
            match state.difficulty_target {
                Some(SelectionDifficultyTarget::Pending) => "pending",
                Some(SelectionDifficultyTarget::Incumbent) => "incumbent",
                Some(SelectionDifficultyTarget::Successor) => "successor",
                None => "-",
            }
        );
    }
    let chart = state.chart.as_ref().map_or_else(
        || format!("identity unresolved: {:?}", state.identity_status),
        |chart| {
            fitted_value(
                &format!(
                    "{}{} {} {} / ",
                    if state.identity_status != SelectIdentityStatus::Resolved || state.suspended {
                        "held "
                    } else {
                        ""
                    },
                    play_side_label(chart.play_side),
                    play_type_label(chart.play_type),
                    difficulty_label(chart.difficulty)
                ),
                chart
                    .presentation
                    .display_titles
                    .first()
                    .map_or("?", String::as_str),
                width,
            )
        },
    );
    let values = format!(
        "SCORE {}  MISS {}",
        field_label(&state.score, ToString::to_string),
        field_label(&state.miss_count, ToString::to_string)
    );
    let clear = field_label(&state.clear_type, |clear| format!("{clear:?}"));
    let rank = state
        .snapshot
        .as_ref()
        .filter(|_| {
            matches!(
                state.output,
                BestOutputState::Partial | BestOutputState::Complete
            )
        })
        .and_then(|snapshot| snapshot.derived_dj_rank.as_deref())
        .unwrap_or("?");
    vec![
        Line::from(heading),
        Line::from(chart),
        Line::from(values),
        Line::from(format!("{clear}  DJ {rank} (derived)")),
        Line::from(output_line(state)),
    ]
}

#[test]
fn field_labels_distinguish_wait_unknown_pending_and_explicit_absence() {
    let mut field = StableBestField::default();
    assert_eq!(field_label(&field, u32::to_string), "waiting");
    field.observe(BestValue::Unknown);
    assert_eq!(field_label(&field, u32::to_string), "unknown");
    field.observe(BestValue::Known(12));
    assert_eq!(field_label(&field, u32::to_string), "12 (1/2)");
    field.observe(BestValue::Known(12));
    assert_eq!(field_label(&field, u32::to_string), "12");
    for _ in 0..2 {
        field.observe(BestValue::NoRecord);
    }
    assert_eq!(field_label(&field, u32::to_string), "no record");
    for _ in 0..2 {
        field.observe(BestValue::NotDisplayed);
    }
    assert_eq!(field_label(&field, u32::to_string), "not displayed");
}
