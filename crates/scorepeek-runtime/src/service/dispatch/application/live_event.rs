use super::*;

pub(super) struct LiveSessionEmission {
    pub(super) public_binding: Option<crate::events::snapshot::Binding>,
    pub(super) value: serde_json::Value,
    pub(super) authority_joint_evidence:
        Option<scorepeek_core::recognition::shared::JointEvidenceObservation>,
    pub(super) diagnostic_identity: Option<serde_json::Value>,
    pub(super) diagnostic_capture_fact: Option<serde_json::Value>,
}

pub(super) fn run_event_from_live_emission(
    emission: LiveSessionEmission,
) -> Result<RunEvent, String> {
    let mut event = RunEvent::from_value(emission.value)?;
    if let Some(authority_joint_evidence) = emission.authority_joint_evidence {
        let RunEventKind::FieldObservation { joint_evidence, .. } = &mut event.kind else {
            return Err("full joint evidence was attached to a non-field event".to_owned());
        };
        *joint_evidence = authority_joint_evidence;
    }
    Ok(event)
}

pub(super) fn optional_recognition_root(enabled: bool, root: &Path) -> Option<&Path> {
    enabled.then_some(root)
}

pub(super) fn current_executable_sha256() -> Result<String, String> {
    let mut file = File::open("/proc/self/exe")
        .map_err(|error| format!("current scorepeek executable could not be opened: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("current scorepeek executable could not be read: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(encoded)
}

#[derive(Serialize)]
pub(super) struct LiveDiagnosticPreflight<'a> {
    schema: &'static str,
    event: &'static str,
    pub(super) status: &'static str,
    root: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error_type: Option<&'static str>,
}

pub(super) fn prepare_live_diagnostic_root<'a>(
    root: &'a Path,
    policy: &DiagnosticPolicy,
) -> LiveDiagnosticPreflight<'a> {
    let ready = if policy.enabled {
        Some(prepare_private_directory(root))
    } else {
        None
    };
    let (status, error_type) = match ready {
        None => ("disabled", None),
        Some(true) => ("ready", None),
        Some(false) => ("degraded", Some("store_unavailable")),
    };
    LiveDiagnosticPreflight {
        schema: "scorepeek-live-session-event-v1",
        event: "diagnostic_status",
        status,
        root,
        error_type,
    }
}

pub(super) fn prepare_private_directory(path: &Path) -> bool {
    match path.metadata() {
        Ok(metadata) => path.is_absolute() && metadata.is_dir(),
        Err(error) if error.kind() == io::ErrorKind::NotFound && path.is_absolute() => {
            let Some(parent) = path.parent() else {
                return false;
            };
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).is_ok()
                && File::open(parent)
                    .and_then(|directory| directory.sync_all())
                    .is_ok()
        }
        Err(_) => false,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the serializer keeps the complete versioned live event mapping together"
)]
pub(super) fn live_session_event_value(
    session_id: Option<&str>,
    routine_generation: Option<u64>,
    event: capture_live::GamescopeLiveSessionEvent<'_>,
) -> Result<serde_json::Value, String> {
    let schema = if session_id.is_some() {
        RUN_EVENT_SCHEMA
    } else {
        "scorepeek-live-session-event-v1"
    };
    let value = match event {
        capture_live::GamescopeLiveSessionEvent::Started {
            capture_generation,
            capture_profile_sha256,
            normalizer_artifact_sha256,
            ..
        } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "session_started",
                "capture_generation": capture_generation,
                "capture_profile_sha256": capture_profile_sha256,
                "normalizer_artifact_sha256": normalizer_artifact_sha256,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
            }
            if let Some(capture_generation) = routine_generation {
                value["capture_generation"] = capture_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::RecordingHealth { snapshot } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "recording_health_changed",
                "state": snapshot.state,
                "memory_limit_bytes": snapshot.memory_limit_bytes,
                "memory_used_bytes": snapshot.memory_used_bytes,
                "memory_high_water_bytes": snapshot.memory_high_water_bytes,
                "dropped_frames": snapshot.dropped_frames,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
            }
            if let Some(capture_generation) = routine_generation {
                value["capture_generation"] = capture_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::RecordingFinalizing => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "recording_finalizing",
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
            }
            if let Some(capture_generation) = routine_generation {
                value["capture_generation"] = capture_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::CaptureDiagnostic { fact } => {
            let mut value = serde_json::json!({
                "schema": CAPTURE_DIAGNOSTIC_SCHEMA,
                "event": "capture_diagnostic",
                "fact": fact,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
            }
            if let Some(capture_generation) = routine_generation {
                value["capture_generation"] = capture_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::RawScreenObserved {
            semantic_episode_id,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            screen,
            result_presence,
            play_presence,
        } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "raw_screen_observed",
                "semantic_episode_id": semantic_episode_id,
                "sequence": sequence,
                "monotonic_start_ms": monotonic_start_ms,
                "monotonic_end_ms": monotonic_end_ms,
                "screen": screen,
                "result_presence": result_presence,
                "play_presence": play_presence,
                "unknown_reason": (screen == recognition::ScreenClass::Unknown)
                    .then_some("predicate_not_matched"),
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
                value["capture_generation"] = routine_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::SemanticScreenEpisode {
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            phase,
        } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "semantic_screen_episode_changed",
                "screen_episode_id": screen_episode_id,
                "sequence": sequence,
                "monotonic_end_ms": monotonic_end_ms,
                "screen": screen,
                "phase": phase,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
                value["capture_generation"] = routine_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::GameVersionIdentified {
            source_sequence,
            version,
        } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "game_version_changed",
                "source_sequence": source_sequence,
                "version": version,
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
                value["capture_generation"] = routine_generation.into();
            }
            value
        }
        capture_live::GamescopeLiveSessionEvent::Observation {
            screen_episode_id,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            output: observation,
        } => {
            let (screen, fields) = match observation.fields() {
                recognition::ScreenFieldObservations::Title(fields) => (
                    "title",
                    serde_json::json!({
                        "game_version": fields.game_version.open_text,
                    }),
                ),
                recognition::ScreenFieldObservations::Result(fields) => (
                    "result",
                    serde_json::json!({
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
                recognition::ScreenFieldObservations::MusicSelect(fields) => (
                    "music_select",
                    serde_json::json!({
                        "best": fields.best,
                        "play_type": fields.play_type,
                        "central_title": fields.central_title.open_text,
                        "artist": fields.artist.open_text,
                        "selected_difficulty": fields.selected_difficulty,
                        "play_side": fields.play_side,
                        "active_list_title": fields.active_list_title.open_text,
                        "title_evidence": observation.title_evidence(),
                    }),
                ),
            };
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "field_observation",
                "screen_episode_id": screen_episode_id,
                "sequence": sequence,
                "monotonic_start_ms": monotonic_start_ms,
                "monotonic_end_ms": monotonic_end_ms,
                "screen": screen,
                "fields": fields,
                "result_song_resolution": observation.result_resolution(),
                "music_select_song_resolution": observation.music_select_resolution(),
                "parsed_result_fields": observation.parsed_result_fields(),
                "result_chart_resolution": observation.result_chart_resolution(),
                "result_performance_resolution": observation.result_performance_resolution(),
                "current_score_ocr_resolution": observation.current_score_ocr_resolution(),
                "numeric_batch": observation.numeric_batch(),
                "joint_evidence": observation.joint_evidence().diagnostic_top(),
                "processing_timing": observation.processing_timing(),
            });
            if let Some(session_id) = session_id {
                value["session_id"] = session_id.into();
                value["capture_generation"] = routine_generation.into();
            }
            value["song_resolution_presentation"] =
                serde_json::to_value(song_resolution_presentation(observation)?)
                    .map_err(|error| format!("song presentation serialization failed: {error}"))?;
            value
        }
    };
    Ok(value)
}

pub(super) fn song_resolution_presentation(
    observation: &scorepeek_core::model::session::RegisteredScreenFieldObservation,
) -> Result<scorepeek_core::event::SongResolutionPresentation, String> {
    use scorepeek_core::recognition::music_select::MusicSelectSongResolution;
    use scorepeek_core::recognition::result::ResultSongResolution;

    match observation.song_resolution() {
        recognition::ScreenSongResolution::Title => {
            Ok(scorepeek_core::event::SongResolutionPresentation::Unknown {
                reason: serde_json::Value::String("not_applicable".to_owned()),
                selected: None,
                runner_up: None,
                evidence_summary: None,
            })
        }
        recognition::ScreenSongResolution::Result(resolution) => match resolution {
            ResultSongResolution::Accepted {
                selected,
                runner_up,
                title_edit_margin,
                ..
            } => Ok(scorepeek_core::event::SongResolutionPresentation::Accepted {
                reason: None,
                selected: song_presentation(observation, selected.song_id)?,
                runner_up: song_presentation(observation, runner_up.song_id)?,
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
            } => Ok(scorepeek_core::event::SongResolutionPresentation::Unknown {
                reason: serde_json::to_value(reason).map_err(|error| format!("result resolution reason serialization failed: {error}"))?,
                selected: selected.as_ref().map(|candidate| song_presentation(observation, candidate.song_id)).transpose()?,
                runner_up: runner_up.as_ref().map(|candidate| song_presentation(observation, candidate.song_id)).transpose()?,
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
        recognition::ScreenSongResolution::MusicSelect(resolution) => match resolution {
            MusicSelectSongResolution::Accepted {
                selected,
                runner_up,
                active_prefix_edit_margin,
                corroboration,
                ..
            } => Ok(scorepeek_core::event::SongResolutionPresentation::Accepted {
                reason: None,
                selected: song_presentation(observation, selected.song_id)?,
                runner_up: song_presentation(observation, runner_up.song_id)?,
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
            } => Ok(scorepeek_core::event::SongResolutionPresentation::Unknown {
                reason: serde_json::to_value(reason).map_err(|error| format!("music-select resolution reason serialization failed: {error}"))?,
                selected: selected.as_ref().map(|candidate| song_presentation(observation, candidate.song_id)).transpose()?,
                runner_up: runner_up.as_ref().map(|candidate| song_presentation(observation, candidate.song_id)).transpose()?,
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

pub(super) fn song_presentation(
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
