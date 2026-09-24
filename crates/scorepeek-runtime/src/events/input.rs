//! Runtime adaptation of diagnostic event fixtures to typed domain input.

use super::{RunEvent, RunEventKind};
use scorepeek_core::catalog::{Difficulty, PlayType};
use scorepeek_core::event::{DomainFieldObservation, DomainInput};
use scorepeek_core::game_version::GameVersionState;
use scorepeek_core::recognition::music_select::{MusicSelectBestObservation, PlaySide};
use scorepeek_core::recognition::result::PlayOptionsObservation;
use scorepeek_core::recognition::screen::{ResultPanelSide, ScreenClass};
use serde_json::Value;

fn screen_class(name: &str) -> Result<ScreenClass, String> {
    match name {
        "title" => Ok(ScreenClass::Title),
        "result" => Ok(ScreenClass::Result),
        "music_select" => Ok(ScreenClass::MusicSelect),
        "mode_select" => Ok(ScreenClass::ModeSelect),
        "decide_transition" => Ok(ScreenClass::DecideTransition),
        "play" => Ok(ScreenClass::Play),
        "unknown" => Ok(ScreenClass::Unknown),
        _ => Err(format!("unknown domain screen: {name}")),
    }
}

fn optional_field<T: serde::de::DeserializeOwned>(
    fields: &Value,
    name: &str,
) -> Result<Option<T>, String> {
    fields
        .get(name)
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|error| format!("invalid {name} field: {error}"))
        })
        .transpose()
}

fn known<T>(
    fields: &Value,
    name: &str,
    parse: impl FnOnce(&str) -> Option<T>,
) -> Result<Option<T>, String> {
    let Some(field) = fields.get(name) else {
        return Ok(None);
    };
    let state = field
        .get("state")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("invalid {name} state"))?;
    let status = state
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("invalid {name} status"))?;
    if status == "unknown" {
        return Ok(None);
    }
    if status != "known" {
        return Err(format!("invalid {name} status"));
    }
    let value = state
        .get("value")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("invalid {name} value"))?;
    parse(value)
        .map(Some)
        .ok_or_else(|| format!("invalid {name} value"))
}

fn fields(
    screen: &str,
    fields: &Value,
    parsed: Option<&scorepeek_core::recognition::result::ParsedResultFields>,
) -> Result<DomainFieldObservation, String> {
    Ok(match screen {
        "title" => DomainFieldObservation::Title,
        "result" => DomainFieldObservation::Result {
            panel_side: optional_field::<ResultPanelSide>(fields, "panel_side")?,
            play_options: optional_field::<PlayOptionsObservation>(fields, "play_options")?,
            clear_type: fields
                .get("clear_type")
                .and_then(Value::as_str)
                .map(str::to_owned),
            parsed_fields: parsed.cloned().map(Box::new),
        },
        "music_select" => DomainFieldObservation::MusicSelect {
            selected_difficulty: known(fields, "selected_difficulty", |value| match value {
                "beginner" => Some(Difficulty::Beginner),
                "normal" => Some(Difficulty::Normal),
                "hyper" => Some(Difficulty::Hyper),
                "another" => Some(Difficulty::Another),
                "leggendaria" => Some(Difficulty::Leggendaria),
                _ => None,
            })?,
            play_type: known(fields, "play_type", |value| match value {
                "single" => Some(PlayType::Single),
                "double" => Some(PlayType::Double),
                _ => None,
            })?,
            play_side: known(fields, "play_side", |value| match value {
                "one_player" => Some(PlaySide::OnePlayer),
                "two_player" => Some(PlaySide::TwoPlayer),
                _ => None,
            })?,
            best: optional_field::<MusicSelectBestObservation>(fields, "best")?.unwrap_or_default(),
        },
        _ => return Err(format!("field observation on unsupported screen: {screen}")),
    })
}

pub(super) fn domain_input(event: &RunEvent) -> Result<Option<DomainInput>, String> {
    Ok(Some(match &event.kind {
        RunEventKind::SessionStarted {
            session_id: Some(session_id),
        } => DomainInput::SessionStarted {
            session_id: session_id.clone(),
        },
        RunEventKind::SessionFinished { session_id, .. } => DomainInput::SessionFinished {
            session_id: session_id.clone(),
        },
        RunEventKind::WatcherStopped { .. } => DomainInput::WatcherFinished,
        RunEventKind::GameVersionChanged { version, .. } => {
            DomainInput::GameVersionState(GameVersionState::Identified(version.clone()))
        }
        RunEventKind::RawScreenObserved {
            session_id,
            semantic_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            result_presence,
            ..
        } => DomainInput::RawScreenObserved {
            session_id: session_id.clone(),
            semantic_episode_id: *semantic_episode_id,
            sequence: *sequence,
            monotonic_end_ms: *monotonic_end_ms,
            screen: screen_class(screen)?,
            result_panel_side: result_presence
                .as_ref()
                .and_then(|presence| presence.panel_side.known()),
        },
        RunEventKind::SemanticScreenEpisodeChanged {
            session_id,
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            phase,
        } => DomainInput::SemanticScreenEpisodeChanged {
            session_id: session_id.clone(),
            screen_episode_id: *screen_episode_id,
            sequence: *sequence,
            monotonic_end_ms: *monotonic_end_ms,
            screen: screen_class(screen)?,
            phase: *phase,
        },
        RunEventKind::ScreenChanged {
            session_id,
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            ..
        } => DomainInput::ScreenChanged {
            session_id: session_id.clone(),
            screen_episode_id: *screen_episode_id,
            sequence: *sequence,
            monotonic_end_ms: *monotonic_end_ms,
            screen: screen_class(screen)?,
        },
        RunEventKind::ScreenTick {
            sequence,
            monotonic_end_ms,
            ..
        } => DomainInput::ScreenTick {
            sequence: *sequence,
            monotonic_end_ms: *monotonic_end_ms,
        },
        RunEventKind::FieldObservation {
            session_id,
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            fields: value,
            parsed_result_fields,
            joint_evidence,
            ..
        } => DomainInput::FieldObservation {
            session_id: session_id.clone(),
            screen_episode_id: *screen_episode_id,
            sequence: *sequence,
            monotonic_end_ms: *monotonic_end_ms,
            observation: fields(screen, value, parsed_result_fields.as_ref())?,
            joint_evidence: joint_evidence.clone(),
        },
        _ => return Ok(None),
    }))
}
