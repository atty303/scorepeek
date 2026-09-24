//! Runtime projection from registered observations into diagnostic events.

use serde_json::{Value, json};

use scorepeek_core::model::session::RegisteredScreenFieldObservation;
use scorepeek_core::recognition::music_select::MusicSelectSongResolution;
use scorepeek_core::recognition::result::ResultSongResolution;
use scorepeek_core::recognition::screen::{ScreenFieldObservations, ScreenSongResolution};

use super::{RunEvent, RunEventKind, SongResolutionPresentation, schema::RUN_EVENT_SCHEMA};

/// Converts one registered field observation into its runtime event.
///
/// # Errors
/// Returns an error when a typed observation cannot be represented by the run-event contract.
pub fn run_event_from_field_observation(
    session_id: &str,
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
    observation: &scorepeek_core::model::session::RegisteredScreenFieldObservation,
) -> Result<SongResolutionPresentation, String> {
    match observation.song_resolution() {
        ScreenSongResolution::Title => {
            Ok(SongResolutionPresentation::Unknown {
                reason: serde_json::Value::String("not_applicable".to_owned()),
                selected: None,
                runner_up: None,
                evidence_summary: None,
            })
        }
        ScreenSongResolution::Result(resolution) => match resolution {
            ResultSongResolution::Accepted {
                selected,
                runner_up,
                title_edit_margin,
                ..
            } => Ok(SongResolutionPresentation::Accepted {
                reason: None,
                selected: observed_song_presentation(observation, selected.song_id)?,
                runner_up: observed_song_presentation(observation, runner_up.song_id)?,
                evidence_summary: format!(
                    "title edit={} similarity={}/{}; artist similarity={}/{}; runner-up margin={}",
                    selected.title.minimum_edit_distance,
                    selected.title.maximum_normalized_similarity.matching_units,
                    selected.title.maximum_normalized_similarity.compared_units,
                    selected.artist.maximum_normalized_similarity.matching_units,
                    selected.artist.maximum_normalized_similarity.compared_units,
                    title_edit_margin,
                ),
            }),
            ResultSongResolution::Unknown {
                reason,
                selected,
                runner_up,
                title_edit_margin,
                ..
            } => Ok(SongResolutionPresentation::Unknown {
                reason: serde_json::to_value(reason).map_err(|error| format!("result resolution reason serialization failed: {error}"))?,
                selected: selected.as_ref().map(|candidate| observed_song_presentation(observation, candidate.song_id)).transpose()?,
                runner_up: runner_up.as_ref().map(|candidate| observed_song_presentation(observation, candidate.song_id)).transpose()?,
                evidence_summary: selected.as_ref().map(|candidate| format!(
                    "title edit={} similarity={}/{}; artist similarity={}/{}; runner-up margin={}",
                    candidate.title.minimum_edit_distance,
                    candidate.title.maximum_normalized_similarity.matching_units,
                    candidate.title.maximum_normalized_similarity.compared_units,
                    candidate.artist.maximum_normalized_similarity.matching_units,
                    candidate.artist.maximum_normalized_similarity.compared_units,
                    title_edit_margin.map_or_else(|| "-".to_owned(), |margin| margin.to_string()),
                )),
            }),
        },
        ScreenSongResolution::MusicSelect(resolution) => match resolution {
            MusicSelectSongResolution::Accepted {
                selected,
                runner_up,
                active_prefix_edit_margin,
                corroboration,
                ..
            } => Ok(SongResolutionPresentation::Accepted {
                reason: None,
                selected: observed_song_presentation(observation, selected.song_id)?,
                runner_up: observed_song_presentation(observation, runner_up.song_id)?,
                evidence_summary: format!(
                    "active-prefix edit={} similarity={}/{}; runner-up margin={}; corroboration central-title={} artist={}",
                    selected.active_list_title_prefix.minimum_edit_distance,
                    selected.active_list_title_prefix.maximum_normalized_similarity.matching_units,
                    selected.active_list_title_prefix.maximum_normalized_similarity.compared_units,
                    active_prefix_edit_margin,
                    corroboration.central_title,
                    corroboration.artist,
                ),
            }),
            MusicSelectSongResolution::Unknown {
                reason,
                selected,
                runner_up,
                active_prefix_edit_margin,
                ..
            } => Ok(SongResolutionPresentation::Unknown {
                reason: serde_json::to_value(reason).map_err(|error| format!("music-select resolution reason serialization failed: {error}"))?,
                selected: selected.as_ref().map(|candidate| observed_song_presentation(observation, candidate.song_id)).transpose()?,
                runner_up: runner_up.as_ref().map(|candidate| observed_song_presentation(observation, candidate.song_id)).transpose()?,
                evidence_summary: selected.as_ref().map(|candidate| format!(
                    "active-prefix edit={} similarity={}/{}; runner-up margin={}",
                    candidate.active_list_title_prefix.minimum_edit_distance,
                    candidate.active_list_title_prefix.maximum_normalized_similarity.matching_units,
                    candidate.active_list_title_prefix.maximum_normalized_similarity.compared_units,
                    active_prefix_edit_margin.map_or_else(|| "-".to_owned(), |margin| margin.to_string()),
                )),
            }),
        },
    }
}

fn observed_song_presentation(
    observation: &scorepeek_core::model::session::RegisteredScreenFieldObservation,
    song_id: scorepeek_core::catalog::ScorepeekSongId,
) -> Result<scorepeek_core::event::SongPresentation, String> {
    let evidence = observation
        .candidates()
        .catalog_evidence()
        .songs
        .iter()
        .find(|song| song.song_id == song_id)
        .ok_or_else(|| {
            format!("resolved song {song_id:?} is absent from the session catalog evidence")
        })?;
    let artists = &evidence.artist.display;
    let [artist] = artists.as_slice() else {
        return Err(format!(
            "resolved song {song_id:?} does not have exactly one display artist"
        ));
    };
    Ok(scorepeek_core::event::SongPresentation {
        scorepeek_song_id: song_id,
        display_titles: evidence.title.display.clone(),
        artist: artist.clone(),
    })
}

/// Projects a run event into the bounded diagnostic representation shared by live and replay.
///
/// # Errors
/// Returns an error when the projected event cannot be represented as JSON.
pub fn diagnostic_run_event_value(event: &RunEvent) -> Result<Value, String> {
    let RunEventKind::FieldObservation {
        session_id,
        screen_episode_id,
        sequence,
        monotonic_start_ms,
        monotonic_end_ms,
        screen,
        fields,
        result_song_resolution,
        music_select_song_resolution,
        parsed_result_fields,
        result_chart_resolution,
        result_performance_resolution,
        current_score_ocr_resolution,
        numeric_batch,
        joint_evidence,
        processing_timing,
        song_resolution_presentation,
    } = &event.kind
    else {
        return event.to_value();
    };
    RunEvent {
        schema: event.schema.clone(),
        kind: RunEventKind::FieldObservation {
            session_id: session_id.clone(),
            screen_episode_id: *screen_episode_id,
            sequence: *sequence,
            monotonic_start_ms: *monotonic_start_ms,
            monotonic_end_ms: *monotonic_end_ms,
            screen: screen.clone(),
            fields: fields.clone(),
            result_song_resolution: result_song_resolution.clone(),
            music_select_song_resolution: music_select_song_resolution.clone(),
            parsed_result_fields: parsed_result_fields.clone(),
            result_chart_resolution: result_chart_resolution.clone(),
            result_performance_resolution: result_performance_resolution.clone(),
            current_score_ocr_resolution: current_score_ocr_resolution.clone(),
            numeric_batch: numeric_batch.clone(),
            joint_evidence: joint_evidence.diagnostic_top(),
            processing_timing: processing_timing.clone(),
            song_resolution_presentation: song_resolution_presentation.clone(),
        },
    }
    .to_value()
}
