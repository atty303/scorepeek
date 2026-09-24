use super::*;

fn screen_name(screen: recognition::ScreenClass) -> &'static str {
    match screen {
        recognition::ScreenClass::Title => "title",
        recognition::ScreenClass::Result => "result",
        recognition::ScreenClass::MusicSelect => "music_select",
        recognition::ScreenClass::ModeSelect => "mode_select",
        recognition::ScreenClass::DecideTransition => "decide_transition",
        recognition::ScreenClass::Play => "play",
        recognition::ScreenClass::Unknown => "unknown",
    }
}

fn semantic_phase(
    phase: capture_live::SemanticScreenEpisodePhase,
) -> scorepeek_core::session::timeline::SemanticEpisodePhase {
    use capture_live::SemanticScreenEpisodePhase as Source;
    use scorepeek_core::session::timeline::SemanticEpisodePhase as Target;
    match phase {
        Source::Started => Target::Started,
        Source::Suspended => Target::Suspended,
        Source::Resumed => Target::Resumed,
        Source::Closing => Target::Closing,
        Source::Finalized => Target::Finalized,
    }
}

pub(super) struct TypedLiveSessionEmission {
    pub(super) event: Option<RunEvent>,
    pub(super) domain_input: Option<scorepeek_core::event::DomainInput>,
    pub(super) diagnostic_identity: Option<serde_json::Value>,
    pub(super) diagnostic_capture_fact: Option<serde_json::Value>,
}

#[allow(
    clippy::too_many_lines,
    reason = "one exhaustive mapping preserves typed capture event order"
)]
pub(super) fn typed_live_session_emission(
    session_id: &str,
    capture: capture_live::CaptureSessionEvent<'_>,
    diagnostic_identity: Option<serde_json::Value>,
    diagnostic_capture_fact: Option<serde_json::Value>,
) -> Result<TypedLiveSessionEmission, String> {
    use scorepeek_core::event::DomainInput;
    let (kind, domain_input) = match capture {
        capture_live::CaptureSessionEvent::Started { .. } => (
            Some(RunEventKind::SessionStarted {
                session_id: Some(session_id.to_owned()),
            }),
            Some(DomainInput::SessionStarted {
                session_id: session_id.to_owned(),
            }),
        ),
        capture_live::CaptureSessionEvent::RecordingHealth { snapshot } => (
            Some(RunEventKind::RecordingHealthChanged {
                session_id: Some(session_id.to_owned()),
                state: match snapshot.state {
                    canonical_recording::RecordingHealthState::Active => "active",
                    canonical_recording::RecordingHealthState::Pressured => "pressured",
                    canonical_recording::RecordingHealthState::Degraded => "degraded",
                }
                .to_owned(),
                memory_limit_bytes: snapshot.memory_limit_bytes,
                memory_used_bytes: snapshot.memory_used_bytes,
                memory_high_water_bytes: snapshot.memory_high_water_bytes,
                dropped_frames: snapshot.dropped_frames,
            }),
            None,
        ),
        capture_live::CaptureSessionEvent::RecordingFinalizing => (
            Some(RunEventKind::RecordingFinalizing {
                session_id: Some(session_id.to_owned()),
            }),
            None,
        ),
        capture_live::CaptureSessionEvent::CaptureDiagnostic { .. } => (None, None),
        capture_live::CaptureSessionEvent::RawScreenObserved {
            semantic_episode_id,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            screen,
            result_presence,
            play_presence,
        } => (
            Some(RunEventKind::RawScreenObserved {
                session_id: Some(session_id.to_owned()),
                semantic_episode_id,
                sequence,
                monotonic_start_ms,
                monotonic_end_ms,
                screen: screen_name(screen).to_owned(),
                result_presence: Some(result_presence),
                play_presence: Some(play_presence),
                unknown_reason: (screen == recognition::ScreenClass::Unknown)
                    .then(|| "predicate_not_matched".to_owned()),
            }),
            Some(DomainInput::RawScreenObserved {
                session_id: Some(session_id.to_owned()),
                semantic_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
                result_panel_side: result_presence.panel_side.known(),
            }),
        ),
        capture_live::CaptureSessionEvent::SemanticScreenEpisode {
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            phase,
        } => {
            let phase = semantic_phase(phase);
            (
                Some(RunEventKind::SemanticScreenEpisodeChanged {
                    session_id: Some(session_id.to_owned()),
                    screen_episode_id,
                    sequence,
                    monotonic_end_ms,
                    screen: screen_name(screen).to_owned(),
                    phase,
                }),
                Some(DomainInput::SemanticScreenEpisodeChanged {
                    session_id: Some(session_id.to_owned()),
                    screen_episode_id,
                    sequence,
                    monotonic_end_ms,
                    screen,
                    phase,
                }),
            )
        }
        capture_live::CaptureSessionEvent::GameVersionIdentified {
            source_sequence,
            version,
        } => (
            Some(RunEventKind::GameVersionChanged {
                session_id: session_id.to_owned(),
                source_sequence,
                version: version.to_owned(),
            }),
            Some(DomainInput::GameVersionState(
                scorepeek_core::game_version::GameVersionState::Identified(version.to_owned()),
            )),
        ),
        capture_live::CaptureSessionEvent::Observation {
            screen_episode_id,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            output,
        } => {
            let event = crate::events::run_event_from_field_observation(
                session_id,
                screen_episode_id,
                sequence,
                monotonic_start_ms,
                monotonic_end_ms,
                output,
            )?;
            let input = DomainInput::from_registered_field(
                session_id,
                screen_episode_id,
                sequence,
                monotonic_end_ms,
                output,
            );
            return Ok(TypedLiveSessionEmission {
                event: Some(event),
                domain_input: Some(input),
                diagnostic_identity,
                diagnostic_capture_fact,
            });
        }
    };
    Ok(TypedLiveSessionEmission {
        event: kind.map(|kind| RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind,
        }),
        domain_input,
        diagnostic_identity,
        diagnostic_capture_fact,
    })
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

#[cfg(test)]
pub(super) fn live_session_event_value(
    session_id: Option<&str>,
    _routine_generation: Option<u64>,
    event: capture_live::CaptureSessionEvent<'_>,
) -> Result<serde_json::Value, String> {
    if let capture_live::CaptureSessionEvent::CaptureDiagnostic { fact } = event {
        return Ok(serde_json::json!({
            "schema": CAPTURE_DIAGNOSTIC_SCHEMA,
            "event": "capture_diagnostic",
            "session_id": session_id,
            "fact": fact,
        }));
    }
    let session_id = session_id.ok_or_else(|| "session ID is required".to_owned())?;
    let emission = typed_live_session_emission(session_id, event, None, None)?;
    let event = emission
        .event
        .ok_or_else(|| "capture event is not a run event".to_owned())?;
    crate::events::diagnostic_run_event_value(&event)
}
