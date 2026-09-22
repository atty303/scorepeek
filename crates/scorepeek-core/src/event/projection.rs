//! Portable projection from registered observations into typed run events.

use serde_json::{Value, json};

use crate::catalog::ScorepeekSongId;
use crate::model::session::RegisteredScreenFieldObservation;
use crate::recognition::music_select::MusicSelectSongResolution;
use crate::recognition::result::ResultSongResolution;
use crate::recognition::screen::{ScreenFieldObservations, ScreenSongResolution};

use super::{
    RunEvent, RunEventKind, SongPresentation, SongResolutionPresentation, schema::RUN_EVENT_SCHEMA,
};

/// Converts one registered field observation into its transport-neutral run event.
///
/// # Errors
/// Returns an error when a typed observation cannot be represented by the run-event contract.
pub fn run_event_from_field_observation(
    session_id: &str,
    capture_generation: u64,
    screen_episode_id: u64,
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    observation: &RegisteredScreenFieldObservation,
) -> Result<RunEvent, String> {
    let (screen, fields) = match observation.fields() {
        ScreenFieldObservations::Title(fields) => (
            "title",
            json!({ "game_version": fields.game_version.open_text }),
        ),
        ScreenFieldObservations::Result(fields) => (
            "result",
            json!({
                "panel_side": fields.panel_side,
                "title": fields.title.open_text,
                "artist": fields.artist.open_text,
                "clear_type": observation.clear_type(),
                "clear_type_ocr": fields.clear_type.open_text,
                "difficulty": fields.difficulty.open_text,
                "play_type": fields.play_type.open_text,
                "level": fields.level.open_text,
                "notes": fields.notes.open_text,
                "current_score": fields.current_score.open_text,
                "previous_clear_type": fields.previous_clear_type.open_text,
                "previous_score": fields.previous_score.open_text,
                "previous_miss_count": fields.previous_miss_count.open_text,
                "miss_count": fields.miss_count.open_text,
                "pgreat": fields.pgreat.open_text,
                "great": fields.great.open_text,
                "good": fields.good.open_text,
                "bad": fields.bad.open_text,
                "poor": fields.poor.open_text,
                "fast": fields.fast.open_text,
                "slow": fields.slow.open_text,
                "combo_break": fields.combo_break.open_text,
                "play_options": fields.play_options,
            }),
        ),
        ScreenFieldObservations::MusicSelect(fields) => (
            "music_select",
            json!({
                "best": fields.best,
                "central_title": fields.central_title.open_text,
                "artist": fields.artist.open_text,
                "play_type": fields.play_type,
                "selected_difficulty": fields.selected_difficulty,
                "play_side": fields.play_side,
                "active_list_title": fields.active_list_title.open_text,
                "title_evidence": observation.title_evidence(),
            }),
        ),
    };
    Ok(RunEvent {
        schema: RUN_EVENT_SCHEMA.to_owned(),
        kind: RunEventKind::FieldObservation {
            session_id: Some(session_id.to_owned()),
            capture_generation: Some(capture_generation),
            screen_episode_id,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            screen: screen.to_owned(),
            fields,
            result_song_resolution: serde_json::to_value(observation.result_resolution())
                .map_err(|error| error.to_string())?,
            music_select_song_resolution: serde_json::to_value(
                observation.music_select_resolution(),
            )
            .map_err(|error| error.to_string())?,
            parsed_result_fields: observation.parsed_result_fields().cloned(),
            result_chart_resolution: observation.result_chart_resolution().cloned(),
            result_performance_resolution: observation.result_performance_resolution().cloned(),
            current_score_ocr_resolution: observation
                .current_score_ocr_resolution()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|error| error.to_string())?,
            numeric_batch: observation
                .numeric_batch()
                .map(serde_json::to_value)
                .transpose()
                .map_err(|error| error.to_string())?,
            joint_evidence: observation.joint_evidence().clone(),
            processing_timing: serde_json::to_value(observation.processing_timing())
                .map_err(|error| error.to_string())?,
            song_resolution_presentation: Box::new(song_resolution_presentation_from_observation(
                observation,
            )?),
        },
    })
}

fn song_resolution_presentation_from_observation(
    observation: &RegisteredScreenFieldObservation,
) -> Result<SongResolutionPresentation, String> {
    match observation.song_resolution() {
        ScreenSongResolution::Title => Ok(SongResolutionPresentation::Unknown {
            reason: Value::String("not_applicable".to_owned()),
            selected: None,
            runner_up: None,
            evidence_summary: None,
        }),
        ScreenSongResolution::Result(resolution) => match resolution {
            ResultSongResolution::Accepted {
                selected,
                runner_up,
                ..
            } => Ok(SongResolutionPresentation::Accepted {
                reason: None,
                selected: observed_song_presentation(observation, selected.song_id)?,
                runner_up: observed_song_presentation(observation, runner_up.song_id)?,
                evidence_summary: "catalog_constrained_result".to_owned(),
            }),
            ResultSongResolution::Unknown {
                reason,
                selected,
                runner_up,
                ..
            } => Ok(SongResolutionPresentation::Unknown {
                reason: serde_json::to_value(reason).map_err(|error| error.to_string())?,
                selected: selected
                    .as_ref()
                    .map(|candidate| observed_song_presentation(observation, candidate.song_id))
                    .transpose()?,
                runner_up: runner_up
                    .as_ref()
                    .map(|candidate| observed_song_presentation(observation, candidate.song_id))
                    .transpose()?,
                evidence_summary: selected
                    .as_ref()
                    .map(|_| "catalog_constrained_result".to_owned()),
            }),
        },
        ScreenSongResolution::MusicSelect(resolution) => match resolution {
            MusicSelectSongResolution::Accepted {
                selected,
                runner_up,
                ..
            } => Ok(SongResolutionPresentation::Accepted {
                reason: None,
                selected: observed_song_presentation(observation, selected.song_id)?,
                runner_up: observed_song_presentation(observation, runner_up.song_id)?,
                evidence_summary: "catalog_constrained_music_select".to_owned(),
            }),
            MusicSelectSongResolution::Unknown {
                reason,
                selected,
                runner_up,
                ..
            } => Ok(SongResolutionPresentation::Unknown {
                reason: serde_json::to_value(reason).map_err(|error| error.to_string())?,
                selected: selected
                    .as_ref()
                    .map(|candidate| observed_song_presentation(observation, candidate.song_id))
                    .transpose()?,
                runner_up: runner_up
                    .as_ref()
                    .map(|candidate| observed_song_presentation(observation, candidate.song_id))
                    .transpose()?,
                evidence_summary: selected
                    .as_ref()
                    .map(|_| "catalog_constrained_music_select".to_owned()),
            }),
        },
    }
}

fn observed_song_presentation(
    observation: &RegisteredScreenFieldObservation,
    song_id: ScorepeekSongId,
) -> Result<SongPresentation, String> {
    let evidence = observation
        .candidates()
        .catalog_evidence()
        .songs
        .iter()
        .find(|song| song.song_id == song_id)
        .ok_or_else(|| "resolved song is absent from catalog evidence".to_owned())?;
    let [artist] = evidence.artist.display.as_slice() else {
        return Err("resolved song does not have exactly one display artist".to_owned());
    };
    Ok(SongPresentation {
        scorepeek_song_id: song_id,
        display_titles: evidence.title.display.clone(),
        artist: artist.clone(),
    })
}

/// Cursor for deterministic event consumers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProjectionCursor {
    pub next_sequence: u64,
}
