use super::*;

type RegisteredFieldOutput =
    Result<RegisteredScreenFieldObservation, ScreenFieldObservationError<OnnxParityError>>;

#[cfg(test)]
pub fn run_gamescope_field_observation_gate(
    config: GamescopeFieldObservationGateConfig<'_>,
) -> GamescopeFieldObservationGateReport {
    let capture_generation = config.handoff.capture_generation;
    let duration = Duration::from_millis(config.handoff.duration_ms);
    let StartedFieldObservationGate {
        mut lease,
        mut session,
        mut artifact_worker,
        artifact_requested,
        canonical_recorder,
        canonical_recording_start_failed: _,
        recording_memory_limit: _,
        mut sink,
        capture_profile_document: _,
        normalizer_document: _,
    } = match start_field_observation_gate(config) {
        Ok(started) => started,
        Err(report) => return *report,
    };

    let mut counters = FieldObservationCounters::default();
    let mut pending = Vec::<PendingSessionFieldObservation<RegisteredFieldOutput>>::new();
    let game_version = GameVersionResolver::default();
    let mut terminal = offer_field_observation_frames(
        &mut lease,
        &mut session,
        duration,
        &mut pending,
        &mut counters,
        &mut artifact_worker,
        &mut sink,
    );
    if counters.normalized_frames == 0 && terminal.is_none() {
        terminal = Some((FieldObservationGateErrorType::FrameUnavailable, None));
    }
    let (shutdown, finish_time) = lease.shutdown_with_elapsed(&mut sink);
    let post_capture_started = Instant::now();
    if let Err(error) = shutdown {
        terminal.get_or_insert((
            FieldObservationGateErrorType::ShutdownFailed,
            Some(error.error_type()),
        ));
    }
    if terminal.is_none() {
        terminal = wait_field_observations(
            &mut session,
            &mut pending,
            &mut counters,
            &mut artifact_worker,
        );
    }
    if counters.candidate_sets == 0 && terminal.is_none() {
        terminal = Some((
            FieldObservationGateErrorType::FieldObservationUnavailable,
            None,
        ));
    }
    if artifact_requested && terminal.is_none() {
        terminal = result_evidence_error(&counters).map(|error| (error, None));
    }
    let finish_status = if terminal.is_none() {
        DiagnosticRunStatus::Success
    } else {
        DiagnosticRunStatus::Error
    };
    let outcome = session.finish_after_capture(
        finish_status,
        finish_time,
        post_capture_started.elapsed(),
        DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT,
    );
    if outcome.field_observer.status != FieldObserverFinishStatus::Complete {
        terminal.get_or_insert((
            FieldObservationGateErrorType::FieldObserverFinishFailed,
            None,
        ));
    }
    let artifact_outcome = artifact_worker.map(|worker| worker.finish(terminal.is_none()));
    if let Some(recorder) = canonical_recorder {
        let _ = recorder.finish(game_version.state());
    }
    if artifact_requested && terminal.is_none() {
        terminal = recognition_artifact_error(&counters, artifact_outcome.as_ref())
            .map(|error| (error, None));
    }
    let (error_type, capture_error_type) = terminal.unzip();
    field_observation_report(
        error_type,
        capture_error_type.flatten(),
        capture_generation,
        counters,
        FieldObservationFinishOutcomes {
            field_observer: Some(outcome.field_observer),
            diagnostic: Some(outcome.diagnostic),
            recognition_artifact: artifact_outcome,
            artifact_requested,
        },
        sink,
    )
}

#[allow(clippy::too_many_lines)]
pub fn run_runtime_live_session(
    config: GamescopeFieldObservationGateConfig<'_>,
    stop: &AtomicBool,
    emit: &mut LiveEventEmitter<'_>,
) -> GamescopeFieldObservationGateReport {
    let capture_generation = config.handoff.capture_generation;
    let StartedFieldObservationGate {
        mut lease,
        mut session,
        mut artifact_worker,
        artifact_requested,
        canonical_recorder,
        canonical_recording_start_failed,
        recording_memory_limit,
        mut sink,
        capture_profile_document,
        normalizer_document,
    } = match start_field_observation_gate(config) {
        Ok(started) => started,
        Err(mut report) => {
            report.schema = "scorepeek-gamescope-live-session-v1";
            report.session_stop_reason = Some(LiveSessionStopReason::TerminalFailure);
            return *report;
        }
    };

    let mut terminal = emit(GamescopeLiveSessionEvent::Started {
        capture_generation: capture_generation.get(),
        capture_profile_sha256: lease.capture_profile_sha256(),
        normalizer_artifact_sha256: lease.normalizer_artifact_sha256(),
        capture_profile_document: capture_profile_document.as_deref(),
        normalizer_document: normalizer_document.as_deref(),
    })
    .err()
    .map(|_| (FieldObservationGateErrorType::ResultOutputFailed, None));
    if !matches!(
        terminal,
        Some((FieldObservationGateErrorType::ResultOutputFailed, _))
    ) && let Err(error) = emit_capture_diagnostics(&mut sink, emit)
    {
        terminal = Some((error, None));
    }
    if terminal.is_none()
        && canonical_recording_start_failed
        && emit(GamescopeLiveSessionEvent::RecordingHealth {
            snapshot: crate::recording::writer::RecordingHealthSnapshot {
                state: crate::recording::writer::RecordingHealthState::Degraded,
                memory_limit_bytes: recording_memory_limit.bytes(),
                memory_used_bytes: 0,
                memory_high_water_bytes: 0,
                dropped_frames: 0,
            },
        })
        .is_err()
    {
        terminal = Some((FieldObservationGateErrorType::ResultOutputFailed, None));
    }
    let mut counters = FieldObservationCounters::default();
    let mut pending = Vec::<PendingSessionFieldObservation<RegisteredFieldOutput>>::new();
    let mut minimum_event_sequence = None;
    let mut game_version = GameVersionResolver::default();
    if terminal.is_none() {
        terminal = offer_live_field_observation_frames(
            &mut lease,
            &mut session,
            stop,
            &mut pending,
            &mut counters,
            &mut artifact_worker,
            canonical_recorder.as_ref(),
            &mut sink,
            emit,
            &mut minimum_event_sequence,
            &mut game_version,
        );
    }
    let (shutdown, finish_time) = lease.shutdown_with_elapsed(&mut sink);
    if !matches!(
        terminal,
        Some((FieldObservationGateErrorType::ResultOutputFailed, _))
    ) && let Err(error) = emit_capture_diagnostics(&mut sink, emit)
    {
        terminal = Some((error, None));
    }
    let post_capture_started = Instant::now();
    if let Err(error) = shutdown {
        terminal.get_or_insert((
            FieldObservationGateErrorType::ShutdownFailed,
            Some(error.error_type()),
        ));
    }
    let output_available = !matches!(
        terminal,
        Some((FieldObservationGateErrorType::ResultOutputFailed, _))
    );
    let drain_error = wait_live_field_observations(
        &mut session,
        &mut pending,
        &mut counters,
        &mut artifact_worker,
        output_available.then_some(emit),
        minimum_event_sequence,
        Some(&mut game_version),
    );
    if let Some(error) = drain_error {
        if reconnectable_stop_reason(terminal).is_some() {
            terminal = Some(error);
        } else {
            terminal.get_or_insert(error);
        }
    }
    session.record_sampling_summary(
        counters.last_recognition_sequence.unwrap_or(0),
        finish_time,
        scorepeek_core::diagnostics::RecognitionSamplingSummary {
            processed_ticks: counters.recognition_ticks,
            busy_skips: counters.recognition_busy_skips,
            maximum_consecutive_busy_skips: counters.maximum_consecutive_busy_skips,
            field_observation_busy_skips: counters.field_observation_busy_skips,
            maximum_consecutive_field_observation_busy_skips: counters
                .maximum_consecutive_field_observation_busy_skips,
        },
    );
    let mut reconnectable = reconnectable_stop_reason(terminal);
    let finish_status = if terminal.is_none() || reconnectable.is_some() {
        DiagnosticRunStatus::Success
    } else {
        DiagnosticRunStatus::Error
    };
    let outcome = session.finish_after_capture(
        finish_status,
        finish_time,
        post_capture_started.elapsed(),
        DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT,
    );
    if outcome.field_observer.status != FieldObserverFinishStatus::Complete {
        terminal = Some((
            FieldObservationGateErrorType::FieldObserverFinishFailed,
            None,
        ));
        reconnectable = None;
    }
    let artifact_outcome =
        artifact_worker.map(|worker| worker.finish(terminal.is_none() || reconnectable.is_some()));
    let mut canonical_recording_completeness =
        canonical_recording_start_failed.then_some(CanonicalRecordingCompleteness::Partial);
    let mut canonical_recording_manifest_published = false;
    if let Some(recorder) = canonical_recorder {
        if emit(GamescopeLiveSessionEvent::RecordingFinalizing).is_err() {
            terminal = Some((FieldObservationGateErrorType::ResultOutputFailed, None));
        }
        let outcome = recorder.finish(game_version.state());
        if emit(GamescopeLiveSessionEvent::RecordingHealth {
            snapshot: outcome.final_health,
        })
        .is_err()
        {
            terminal = Some((FieldObservationGateErrorType::ResultOutputFailed, None));
        }
        canonical_recording_completeness = Some(outcome.completeness);
        canonical_recording_manifest_published = outcome.manifest_published;
    }
    let stop_reason = if let Some(reason) = reconnectable {
        reason
    } else if terminal.is_none() {
        LiveSessionStopReason::RequestedSignal
    } else {
        LiveSessionStopReason::TerminalFailure
    };
    let (error_type, capture_error_type) = if reconnectable.is_some() {
        (None, terminal.and_then(|(_, capture)| capture).map(Some))
    } else {
        terminal.unzip()
    };
    let mut report = field_observation_report(
        error_type,
        capture_error_type.flatten(),
        capture_generation,
        counters,
        FieldObservationFinishOutcomes {
            field_observer: Some(outcome.field_observer),
            diagnostic: Some(outcome.diagnostic),
            recognition_artifact: artifact_outcome,
            artifact_requested,
        },
        sink,
    );
    report.schema = "scorepeek-gamescope-live-session-v1";
    report.session_stop_reason = Some(stop_reason);
    report.canonical_recording_completeness = canonical_recording_completeness;
    report.canonical_recording_manifest_published = canonical_recording_manifest_published;
    report
}

pub(super) fn reconnectable_stop_reason(
    terminal: Option<(FieldObservationGateErrorType, Option<CaptureErrorType>)>,
) -> Option<LiveSessionStopReason> {
    match terminal {
        Some((
            FieldObservationGateErrorType::CaptureFailed,
            Some(CaptureErrorType::SourceLost | CaptureErrorType::StreamLost),
        )) => Some(LiveSessionStopReason::SourceEnded),
        Some((
            FieldObservationGateErrorType::CaptureFailed,
            Some(CaptureErrorType::SourceContractChanged),
        )) => Some(LiveSessionStopReason::SourceContractChanged),
        _ => None,
    }
}

struct StartedFieldObservationGate {
    lease: CaptureLease,
    session: FieldObservationSession<RegisteredScreenFieldObserver>,
    artifact_worker: Option<RecognitionArtifactWorker>,
    artifact_requested: bool,
    canonical_recorder: Option<CanonicalRecordingWorker>,
    canonical_recording_start_failed: bool,
    recording_memory_limit: crate::recording::policy::RecordingMemoryLimit,
    sink: BoundedDiagnosticSink,
    capture_profile_document: Option<String>,
    normalizer_document: Option<String>,
}

#[allow(clippy::too_many_lines)]
fn start_field_observation_gate(
    mut config: GamescopeFieldObservationGateConfig<'_>,
) -> Result<StartedFieldObservationGate, Box<GamescopeFieldObservationGateReport>> {
    let capture_generation = config.handoff.capture_generation;
    let artifact_requested = config.recognition_artifact_root.is_some();
    let sink = BoundedDiagnosticSink::default();
    if !valid_handoff_descriptor(&config.handoff.descriptor, capture_generation, true) {
        return Err(Box::new(empty_field_observation_report(
            FieldObservationGateErrorType::DiagnosticConfigurationInvalid,
            None,
            capture_generation,
            None,
            artifact_requested,
            sink,
        )));
    }
    let artifact_run_id = config.handoff.descriptor.run_id.clone();
    let mut sink = sink;
    let started_capture =
        start_runtime_capture(config.runtime_capture, capture_generation, &mut sink);
    let (lease, authored) = match started_capture {
        Ok(lease) => lease,
        Err((error, capture_error)) => {
            let error = match error {
                DiagnosticHandoffGateErrorType::AdmissionRejected => {
                    FieldObservationGateErrorType::AdmissionRejected
                }
                _ => FieldObservationGateErrorType::CaptureFailed,
            };
            return Err(Box::new(empty_field_observation_report(
                error,
                capture_error,
                capture_generation,
                None,
                artifact_requested,
                sink,
            )));
        }
    };
    lease
        .capture_profile_sha256()
        .clone_into(&mut config.handoff.descriptor.binding.capture_profile_sha256);
    lease
        .normalizer_artifact_sha256()
        .clone_into(&mut config.handoff.descriptor.binding.normalizer_sha256);
    let expected_profile = lease.capture_profile_sha256().to_owned();
    let expected_normalizer = lease.normalizer_artifact_sha256().to_owned();
    if expected_profile != lease.capture_profile_sha256()
        || expected_normalizer != lease.normalizer_artifact_sha256()
    {
        let lease = lease;
        let (shutdown, _) = lease.shutdown_with_elapsed(&mut sink);
        return Err(Box::new(empty_field_observation_report(
            FieldObservationGateErrorType::DiagnosticBindingMismatch,
            shutdown.err().map(|error| error.error_type()),
            capture_generation,
            None,
            artifact_requested,
            sink,
        )));
    }
    let session = match config.handoff.diagnostic_directory_name.map_or_else(
        || {
            FieldObservationSession::start_registered(
                config.handoff.diagnostic_root,
                config.handoff.descriptor.clone(),
                config.handoff.policy.clone(),
                config.catalog_root,
                config.bundle_root,
                scorepeek_core::model::session::RecognitionExecutionMode::Live,
            )
        },
        |directory_name| {
            FieldObservationSession::start_registered_named(
                config.handoff.diagnostic_root,
                directory_name,
                config.handoff.descriptor.clone(),
                config.handoff.policy.clone(),
                config.catalog_root,
                config.bundle_root,
                scorepeek_core::model::session::RecognitionExecutionMode::Live,
            )
        },
    ) {
        Ok(session) => session,
        Err(error) => {
            let lease = lease;
            let (shutdown, _) = lease.shutdown_with_elapsed(&mut sink);
            let (error_type, field_finish, detail) = field_start_error(error);
            let mut report = empty_field_observation_report(
                error_type,
                shutdown.err().map(|error| error.error_type()),
                capture_generation,
                field_finish,
                artifact_requested,
                sink,
            );
            report.failure_detail = detail;
            return Err(Box::new(report));
        }
    };
    let artifact_worker =
        config
            .recognition_artifact_root
            .map(|root| match config.recognition_artifact_retention {
                RecognitionArtifactRetention::Complete => RecognitionArtifactWorker::start(
                    root.to_owned(),
                    artifact_run_id,
                    expected_profile.clone(),
                ),
                RecognitionArtifactRetention::ForegroundCompactedV1 => {
                    RecognitionArtifactWorker::start_foreground(
                        root.to_owned(),
                        artifact_run_id,
                        expected_profile.clone(),
                    )
                }
            });
    let recording_capture_identity =
        authored
            .as_ref()
            .map(|value| crate::recording::writer::RecordingCaptureIdentity {
                capture_profile_sha256: value.capture_profile_sha256.clone(),
                capture_profile_document: String::from_utf8(value.capture_profile_bytes.clone())
                    .expect("canonical profile JSON is UTF-8"),
                normalizer_sha256: value.normalizer_sha256.clone(),
                normalizer_document: String::from_utf8(value.normalizer_bytes.clone())
                    .expect("canonical normalizer JSON is UTF-8"),
            });
    let (canonical_recorder, canonical_recording_start_failed) =
        if let Some(recording_root) = config.canonical_recording_root {
            match CanonicalRecordingWorker::start_named(
                recording_root,
                "canonical",
                config.recording_memory_limit,
                config.recording_retention,
                recording_capture_identity,
            ) {
                Ok(recorder) => (Some(recorder), false),
                Err(_) => (None, true),
            }
        } else {
            (None, artifact_requested)
        };
    Ok(StartedFieldObservationGate {
        lease,
        session,
        artifact_worker,
        artifact_requested,
        canonical_recorder,
        canonical_recording_start_failed,
        recording_memory_limit: config.recording_memory_limit,
        sink,
        capture_profile_document: authored.as_ref().map(|value| {
            String::from_utf8(value.capture_profile_bytes.clone())
                .expect("canonical profile JSON is UTF-8")
        }),
        normalizer_document: authored.as_ref().map(|value| {
            String::from_utf8(value.normalizer_bytes.clone())
                .expect("canonical normalizer JSON is UTF-8")
        }),
    })
}

pub(super) fn start_runtime_capture(
    input: RuntimeCaptureInput<'_>,
    generation: CaptureGeneration,
    sink: &mut BoundedDiagnosticSink,
) -> Result<
    (CaptureLease, Option<AuthoredGamescopeProfileBinding>),
    (DiagnosticHandoffGateErrorType, Option<CaptureErrorType>),
> {
    match input {
        RuntimeCaptureInput::Pipewire {
            node_name,
            crop,
            expected_node_id,
        } => {
            let lease =
                acquire_pipewire_source(node_name, DISCOVERY_TIMEOUT, sink).map_err(|error| {
                    (
                        DiagnosticHandoffGateErrorType::CaptureFailed,
                        Some(error.error_type()),
                    )
                })?;
            if expected_node_id.is_some_and(|expected| expected != lease.node_id()) {
                lease.shutdown(sink);
                return Err((
                    DiagnosticHandoffGateErrorType::CaptureFailed,
                    Some(CaptureErrorType::SourceLost),
                ));
            }
            let receiver =
                start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, sink)
                    .map_err(|error| {
                        (
                            DiagnosticHandoffGateErrorType::CaptureFailed,
                            Some(error.error_type()),
                        )
                    })?;
            admit_runtime_profile(
                receiver,
                RuntimeCaptureBackend::Pipewire,
                format!("node.name={node_name};media.class=Video/Source"),
                crop,
                generation,
                sink,
            )
            .map(|(lease, authored)| (CaptureLease::Pipewire(lease), Some(authored)))
            .map_err(|failure| {
                let error = failure.error_type();
                let _ = failure.shutdown(sink);
                (
                    DiagnosticHandoffGateErrorType::AdmissionRejected,
                    Some(error),
                )
            })
        }
        RuntimeCaptureInput::VulkanLayer { session, crop } => {
            admit_vulkan_session(*session, crop, generation, sink)
                .map(|(lease, authored)| (CaptureLease::Vulkan(lease), Some(authored)))
                .map_err(|error| {
                    (
                        DiagnosticHandoffGateErrorType::AdmissionRejected,
                        Some(error.error_type()),
                    )
                })
        }
        #[cfg(test)]
        RuntimeCaptureInput::LegacyGamescope {
            binding_path,
            expected_binding_sha256,
            expected_source_node_id,
        } => {
            let binding = read_diagnostic_handoff_binding(binding_path, expected_binding_sha256)
                .map_err(|error| (error, None))?;
            start_diagnostic_handoff_capture(binding, generation, expected_source_node_id, sink)
                .map(|lease| (lease, None))
        }
    }
}

pub(super) fn empty_field_observation_report(
    error_type: FieldObservationGateErrorType,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: CaptureGeneration,
    field_observer: Option<
        crate::service::session::recognition::field_observer::FieldObserverFinishOutcome,
    >,
    artifact_requested: bool,
    sink: BoundedDiagnosticSink,
) -> GamescopeFieldObservationGateReport {
    field_observation_report(
        Some(error_type),
        capture_error_type,
        capture_generation,
        FieldObservationCounters::default(),
        FieldObservationFinishOutcomes {
            field_observer,
            diagnostic: None,
            recognition_artifact: None,
            artifact_requested,
        },
        sink,
    )
}

pub(super) fn field_start_error(
    error: FieldObservationStartError<RegisteredScreenFieldObserverLoadError>,
) -> (
    FieldObservationGateErrorType,
    Option<crate::service::session::recognition::field_observer::FieldObserverFinishOutcome>,
    Option<String>,
) {
    use crate::service::session::recognition::field_observer::FieldObserverStartError;
    match error {
        FieldObservationStartError::FieldObserver(error) => match error {
            FieldObserverStartError::InvalidBinding => (
                FieldObservationGateErrorType::DiagnosticConfigurationInvalid,
                None,
                None,
            ),
            FieldObserverStartError::Load(RegisteredScreenFieldObserverLoadError::Resources(
                error,
            )) => (
                field_resource_error(error.error_type()),
                None,
                Some(error.to_string()),
            ),
            FieldObserverStartError::Load(
                RegisteredScreenFieldObserverLoadError::CandidateDomain(error),
            ) => (
                FieldObservationGateErrorType::CandidateDomainInvalid,
                None,
                Some(format!(
                    "catalog candidate domain is invalid for song {}: {error}",
                    error.song_id.as_uuid()
                )),
            ),
            FieldObserverStartError::Load(
                RegisteredScreenFieldObserverLoadError::NumericModel(error),
            ) => (
                FieldObservationGateErrorType::NumericModelUnavailable,
                None,
                Some(error.to_string()),
            ),
            FieldObserverStartError::Load(RegisteredScreenFieldObserverLoadError::TextRuntime(
                error,
            )) => (
                FieldObservationGateErrorType::FieldObserverUnavailable,
                None,
                Some(error.to_string()),
            ),
            FieldObserverStartError::WorkerUnavailable => (
                FieldObservationGateErrorType::FieldObserverUnavailable,
                None,
                None,
            ),
        },
        FieldObservationStartError::Recognition {
            field_observer_finish,
            ..
        } => (
            FieldObservationGateErrorType::DiagnosticConfigurationInvalid,
            Some(field_observer_finish),
            None,
        ),
    }
}

pub(super) const fn field_resource_error(
    error: RegisteredResourceLoadErrorType,
) -> FieldObservationGateErrorType {
    match error {
        RegisteredResourceLoadErrorType::InvalidLocation => {
            FieldObservationGateErrorType::InvalidResourceLocation
        }
        RegisteredResourceLoadErrorType::ModelBindingMismatch => {
            FieldObservationGateErrorType::ModelBindingMismatch
        }
        RegisteredResourceLoadErrorType::RuntimeBindingMismatch => {
            FieldObservationGateErrorType::RuntimeBindingMismatch
        }
        RegisteredResourceLoadErrorType::CatalogUnavailable => {
            FieldObservationGateErrorType::CatalogUnavailable
        }
        RegisteredResourceLoadErrorType::CatalogBindingMismatch => {
            FieldObservationGateErrorType::CatalogBindingMismatch
        }
        RegisteredResourceLoadErrorType::CatalogLoadFailed => {
            FieldObservationGateErrorType::CatalogLoadFailed
        }
        RegisteredResourceLoadErrorType::ModelBundleInvalid => {
            FieldObservationGateErrorType::ModelBundleInvalid
        }
        RegisteredResourceLoadErrorType::RuntimeInitializationFailed => {
            FieldObservationGateErrorType::RuntimeInitializationFailed
        }
    }
}

pub(super) fn offer_field_observation_frames(
    lease: &mut CaptureLease,
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    duration: Duration,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
    sink: &mut BoundedDiagnosticSink,
) -> Option<(FieldObservationGateErrorType, Option<CaptureErrorType>)> {
    let normalizer = match NormalizationWorker::start(lease.frame_normalizer()) {
        Ok(worker) => worker,
        Err(error) => return Some(error),
    };
    let mut source = GamescopeCanonicalFrameSource {
        lease,
        counters,
        sink,
        normalizer,
    };
    let started = Instant::now();
    loop {
        let remaining = duration.saturating_sub(started.elapsed());
        let frame = match source.next_frame(remaining) {
            Ok(frame) => frame,
            Err(error) => return Some(error),
        };
        if let Some(frame) = frame {
            let Ok(result) = session.inspect(&frame) else {
                return Some((FieldObservationGateErrorType::RecognitionFailed, None));
            };
            source.counters.inspected_frames = source.counters.inspected_frames.saturating_add(1);
            let screen_counter = match result.observation.screen() {
                ScreenClass::Title => &mut source.counters.title_frames,
                ScreenClass::Result => &mut source.counters.result_frames,
                ScreenClass::MusicSelect => &mut source.counters.music_select_frames,
                ScreenClass::ModeSelect => &mut source.counters.mode_select_frames,
                ScreenClass::DecideTransition => &mut source.counters.decide_transition_frames,
                ScreenClass::Play => &mut source.counters.play_frames,
                ScreenClass::Unknown => &mut source.counters.unknown_frames,
            };
            *screen_counter = screen_counter.saturating_add(1);
            match result.field_submission {
                FieldObservationSubmission::BusySkipped => {
                    let _ = session.record_frame_processing_timing(
                        result.timing,
                        scorepeek_core::diagnostics::FrameFieldStatus::BusySkip,
                        None,
                    );
                    unreachable!("offline gate has no pending OCR policy")
                }
                FieldObservationSubmission::NotApplicable => {
                    let _ = session.record_frame_processing_timing(
                        result.timing,
                        scorepeek_core::diagnostics::FrameFieldStatus::NotApplicable,
                        None,
                    );
                    source.counters.field_not_applicable =
                        source.counters.field_not_applicable.saturating_add(1);
                }
                FieldObservationSubmission::Submitted(observation) => {
                    source.counters.field_submitted =
                        source.counters.field_submitted.saturating_add(1);
                    pending.push(observation);
                }
                FieldObservationSubmission::Rejected(error) => {
                    let _ = session.record_frame_processing_timing(
                        result.timing,
                        scorepeek_core::diagnostics::FrameFieldStatus::Failed,
                        None,
                    );
                    source.counters.field_rejected =
                        source.counters.field_rejected.saturating_add(1);
                    if matches!(
                        error,
                        FieldObserverOfferError::BindingMismatch
                            | FieldObserverOfferError::WorkerUnavailable
                    ) {
                        return Some((
                            FieldObservationGateErrorType::FieldObserverUnavailable,
                            None,
                        ));
                    }
                }
            }
            if let Some(error) = poll_field_observations(
                session,
                pending,
                source.counters,
                artifact_worker,
                None,
                None,
                None,
                None,
            ) {
                return Some((error, None));
            }
        }
        if started.elapsed() >= duration {
            return None;
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "the live loop keeps screen cadence, field admission, event ordering, and counters in one owner"
)]
pub(super) fn offer_live_field_observation_frames(
    lease: &mut CaptureLease,
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    stop: &AtomicBool,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
    canonical_recorder: Option<&CanonicalRecordingWorker>,
    sink: &mut BoundedDiagnosticSink,
    emit: &mut LiveEventEmitter<'_>,
    _minimum_event_sequence: &mut Option<u64>,
    game_version: &mut GameVersionResolver,
) -> Option<(FieldObservationGateErrorType, Option<CaptureErrorType>)> {
    let mut cadence = RecognitionCadence::default();
    let mut episodes = TimelineDriver::default();
    let normalizer = match NormalizationWorker::start(lease.frame_normalizer()) {
        Ok(worker) => worker,
        Err(error) => return Some(error),
    };
    let mut source = GamescopeCanonicalFrameSource {
        lease,
        counters,
        sink,
        normalizer,
    };
    let mut terminal = None;
    let mut semantic_close_failed = false;
    let mut last_recording_health = None;
    let mut last_recording_health_emit = None;
    'capture: while !stop.load(Ordering::Acquire) {
        if let Some(error) = poll_field_observations(
            session,
            pending,
            source.counters,
            artifact_worker,
            None,
            Some(emit),
            None,
            Some(&mut *game_version),
        ) {
            terminal = Some((error, None));
            break 'capture;
        }
        if let Some(recorder) = canonical_recorder {
            let snapshot = recorder.health();
            let changed = last_recording_health.is_none_or(
                |previous: crate::recording::writer::RecordingHealthSnapshot| {
                    previous.state != snapshot.state
                        || previous.dropped_frames != snapshot.dropped_frames
                },
            );
            let due = last_recording_health_emit
                .is_none_or(|previous: Instant| previous.elapsed() >= Duration::from_secs(1));
            if (changed || due)
                && emit(GamescopeLiveSessionEvent::RecordingHealth { snapshot }).is_err()
            {
                terminal = Some((FieldObservationGateErrorType::ResultOutputFailed, None));
                break 'capture;
            }
            if changed || due {
                last_recording_health_emit = Some(Instant::now());
            }
            last_recording_health = Some(snapshot);
        }
        let next_frame = source.next_frame(LIVE_SESSION_POLL_INTERVAL);
        if let Err(error) = emit_capture_diagnostics(source.sink, emit) {
            terminal = Some((error, None));
            break 'capture;
        }
        let frame = match next_frame {
            Ok(frame) => frame,
            Err(error) => {
                terminal = Some(error);
                break 'capture;
            }
        };
        if let Some(mut frame) = frame {
            match cadence.observe(frame.monotonic_end_ms()) {
                CadenceDecision::SkipCadence => continue,
                CadenceDecision::Process { tick_sequence } => {
                    source.counters.recognition_ticks = cadence.processed();
                    source.counters.last_recognition_sequence = Some(tick_sequence);
                    frame.assign_tick_sequence(tick_sequence);
                }
            }
            let field_busy = pending.len() >= 2;
            let inspected = if game_version.identified().is_some() && field_busy {
                session.inspect_with_field_policy(
                    &frame,
                    crate::service::session::recognition::FieldInputPolicy::SkipBusyAndTitle,
                )
            } else if game_version.identified().is_some() {
                session.inspect_with_field_policy(
                    &frame,
                    crate::service::session::recognition::FieldInputPolicy::SkipTitle,
                )
            } else if field_busy {
                session.inspect_while_field_busy(&frame)
            } else {
                session.inspect(&frame)
            };
            let Ok(result) = inspected else {
                terminal = Some((FieldObservationGateErrorType::RecognitionFailed, None));
                break 'capture;
            };
            source.counters.inspected_frames = source.counters.inspected_frames.saturating_add(1);
            let screen_counter = match result.observation.screen() {
                ScreenClass::Title => &mut source.counters.title_frames,
                ScreenClass::Result => &mut source.counters.result_frames,
                ScreenClass::MusicSelect => &mut source.counters.music_select_frames,
                ScreenClass::ModeSelect => &mut source.counters.mode_select_frames,
                ScreenClass::DecideTransition => &mut source.counters.decide_transition_frames,
                ScreenClass::Play => &mut source.counters.play_frames,
                ScreenClass::Unknown => &mut source.counters.unknown_frames,
            };
            *screen_counter = screen_counter.saturating_add(1);
            let screen = result.observation.screen();
            let timeline_step = episodes.observe(
                RawScreenState::from(screen),
                frame.sequence(),
                frame.monotonic_end_ms(),
            );
            if let Some(recorder) = canonical_recorder {
                let _ = recorder.offer(
                    &frame,
                    screen,
                    result.observation.predicate().title_presence.qualifies,
                    timeline_step.active_episode_id,
                );
            }
            let mut live_timing = LiveEventProcessingTiming::default();
            let mut output_failed = false;
            match emit(GamescopeLiveSessionEvent::RawScreenObserved {
                semantic_episode_id: timeline_step.active_episode_id,
                sequence: frame.sequence(),
                monotonic_start_ms: frame.monotonic_start_ms(),
                monotonic_end_ms: frame.monotonic_end_ms(),
                screen,
                result_presence: result.observation.result_presence(),
                play_presence: result.observation.play_presence(),
            }) {
                Ok(timing) => live_timing.add(timing),
                Err(_) => output_failed = true,
            }

            let transition_result: Result<(), FieldObservationGateErrorType> = (|| {
                for action in timeline_step.actions {
                    match action {
                        TimelineAction::Semantic { episode, phase } => {
                            live_timing.add(emit_semantic_episode(
                                emit,
                                episode,
                                frame.sequence(),
                                frame.monotonic_end_ms(),
                                live_semantic_phase(phase),
                            )?);
                        }
                        TimelineAction::DrainAdmitted { .. } => {
                            if let Some((error, _)) = wait_live_field_observations(
                                session,
                                pending,
                                source.counters,
                                artifact_worker,
                                Some(emit),
                                None,
                                Some(&mut *game_version),
                            ) {
                                return Err(error);
                            }
                        }
                    }
                }
                Ok(())
            })();
            let mut frame_timing = result.timing;
            frame_timing.add_live_processing(live_timing);
            if let Err(error) = transition_result {
                let _ = session.record_frame_processing_timing(
                    frame_timing,
                    scorepeek_core::diagnostics::FrameFieldStatus::Failed,
                    None,
                );
                semantic_close_failed = true;
                terminal = Some((error, None));
                break 'capture;
            }
            let mut field_terminal = None;
            match result.field_submission {
                FieldObservationSubmission::BusySkipped => {
                    let _ = session.record_frame_processing_timing(
                        frame_timing,
                        scorepeek_core::diagnostics::FrameFieldStatus::BusySkip,
                        None,
                    );
                    source.counters.field_observation_busy_skips = source
                        .counters
                        .field_observation_busy_skips
                        .saturating_add(1);
                    source.counters.consecutive_field_observation_busy_skips = source
                        .counters
                        .consecutive_field_observation_busy_skips
                        .saturating_add(1);
                    source
                        .counters
                        .maximum_consecutive_field_observation_busy_skips = source
                        .counters
                        .maximum_consecutive_field_observation_busy_skips
                        .max(source.counters.consecutive_field_observation_busy_skips);
                }
                FieldObservationSubmission::NotApplicable => {
                    let _ = session.record_frame_processing_timing(
                        frame_timing,
                        scorepeek_core::diagnostics::FrameFieldStatus::NotApplicable,
                        None,
                    );
                    source.counters.consecutive_field_observation_busy_skips = 0;
                    source.counters.field_not_applicable =
                        source.counters.field_not_applicable.saturating_add(1);
                }
                FieldObservationSubmission::Submitted(mut observation) => {
                    let Some(screen_episode_id) = episodes.active_episode_id() else {
                        let _ = session.record_frame_processing_timing(
                            frame_timing,
                            scorepeek_core::diagnostics::FrameFieldStatus::NotApplicable,
                            None,
                        );
                        continue;
                    };
                    observation.bind_screen_episode(screen_episode_id);
                    observation.add_live_processing(live_timing);
                    source.counters.consecutive_field_observation_busy_skips = 0;
                    source.counters.field_submitted =
                        source.counters.field_submitted.saturating_add(1);
                    pending.push(observation);
                }
                FieldObservationSubmission::Rejected(error) => {
                    let _ = session.record_frame_processing_timing(
                        frame_timing,
                        scorepeek_core::diagnostics::FrameFieldStatus::Failed,
                        None,
                    );
                    source.counters.consecutive_field_observation_busy_skips = 0;
                    source.counters.field_rejected =
                        source.counters.field_rejected.saturating_add(1);
                    if matches!(
                        error,
                        FieldObserverOfferError::BindingMismatch
                            | FieldObserverOfferError::WorkerUnavailable
                    ) {
                        field_terminal = Some((
                            FieldObservationGateErrorType::FieldObserverUnavailable,
                            None,
                        ));
                    }
                }
            }
            if output_failed {
                terminal = Some((FieldObservationGateErrorType::ResultOutputFailed, None));
                break 'capture;
            }
            if let Some(error) = field_terminal {
                terminal = Some(error);
                break 'capture;
            }
        }
    }
    if !semantic_close_failed {
        for action in episodes.finish() {
            match action {
                TimelineAction::Semantic { episode, phase } => {
                    if emit_semantic_episode(
                        emit,
                        episode,
                        episode.last_sequence,
                        episode.last_ms,
                        live_semantic_phase(phase),
                    )
                    .is_err()
                    {
                        return Some((FieldObservationGateErrorType::ResultOutputFailed, None));
                    }
                }
                TimelineAction::DrainAdmitted { .. } => {
                    if let Some(error) = wait_live_field_observations(
                        session,
                        pending,
                        source.counters,
                        artifact_worker,
                        Some(emit),
                        None,
                        Some(&mut *game_version),
                    ) {
                        return Some(error);
                    }
                }
            }
        }
    }
    terminal
}

const fn live_semantic_phase(
    phase: scorepeek_core::session::timeline::SemanticEpisodePhase,
) -> SemanticScreenEpisodePhase {
    use scorepeek_core::session::timeline::SemanticEpisodePhase;
    match phase {
        SemanticEpisodePhase::Started => SemanticScreenEpisodePhase::Started,
        SemanticEpisodePhase::Suspended => SemanticScreenEpisodePhase::Suspended,
        SemanticEpisodePhase::Resumed => SemanticScreenEpisodePhase::Resumed,
        SemanticEpisodePhase::Closing => SemanticScreenEpisodePhase::Closing,
        SemanticEpisodePhase::Finalized => SemanticScreenEpisodePhase::Finalized,
    }
}

pub(super) fn emit_semantic_episode(
    emit: &mut LiveEventEmitter<'_>,
    episode: SemanticScreenEpisode,
    sequence: u64,
    monotonic_end_ms: u64,
    phase: SemanticScreenEpisodePhase,
) -> Result<LiveEventProcessingTiming, FieldObservationGateErrorType> {
    emit(GamescopeLiveSessionEvent::SemanticScreenEpisode {
        screen_episode_id: episode.id,
        sequence,
        monotonic_end_ms,
        screen: episode.screen,
        phase,
    })
    .map_err(|_| FieldObservationGateErrorType::ResultOutputFailed)
}

pub(super) fn wait_field_observations(
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
) -> Option<(FieldObservationGateErrorType, Option<CaptureErrorType>)> {
    let started = Instant::now();
    while !pending.is_empty() {
        let remaining = DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        if let Some(error) = poll_field_observations(
            session,
            pending,
            counters,
            artifact_worker,
            Some(remaining),
            None,
            None,
            None,
        ) {
            return Some((error, None));
        }
    }
    (!pending.is_empty()).then_some((
        FieldObservationGateErrorType::FieldObserverFinishFailed,
        None,
    ))
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one polling boundary keeps its ordered session, queues, diagnostics, event sink, and optional resolvers explicit"
)]
pub(super) fn poll_field_observations(
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
    wait: Option<Duration>,
    mut emit: Option<&mut LiveEventEmitter<'_>>,
    minimum_event_sequence: Option<u64>,
    mut game_version: Option<&mut GameVersionResolver>,
) -> Option<FieldObservationGateErrorType> {
    let index = 0;
    while !pending.is_empty() {
        let poll = match wait {
            Some(timeout) => session.wait_field_observation(&pending[index], timeout),
            None => session.poll_field_observation(&pending[index]),
        };
        match poll {
            FieldObservationSessionPoll::Pending => {
                return None;
            }
            FieldObservationSessionPoll::Ready {
                observation,
                mut timing,
                screen_episode_id,
                ..
            } => {
                pending.remove(index);
                let sequence = observation.sequence();
                let monotonic_start_ms = observation.monotonic_start_ms();
                let monotonic_end_ms = observation.monotonic_end_ms();
                let observation_screen = observation.screen();
                if let Ok(output) = observation.into_output() {
                    let late = minimum_event_sequence.is_some_and(|minimum| sequence < minimum);
                    counters.field_ready_success = counters.field_ready_success.saturating_add(1);
                    counters.candidate_sets = counters.candidate_sets.saturating_add(1);
                    counters.scored_candidates = counters.scored_candidates.saturating_add(
                        u64::try_from(output.candidates().candidate_count()).unwrap_or(u64::MAX),
                    );
                    if matches!(
                        output.fields(),
                        scorepeek_core::recognition::screen::ScreenFieldObservations::Result(_)
                    ) {
                        counters.result_observations =
                            counters.result_observations.saturating_add(1);
                    }
                    let identified_version = match (output.fields(), game_version.as_deref_mut()) {
                        (
                            scorepeek_core::recognition::screen::ScreenFieldObservations::Title(
                                fields,
                            ),
                            Some(resolver),
                        ) => resolver
                            .observe_candidate(sequence, &fields.game_version.open_text)
                            .map(str::to_owned),
                        _ => None,
                    };
                    let mut output_failed = false;
                    let output_timing = if minimum_event_sequence
                        .is_none_or(|minimum| sequence >= minimum)
                        && let Some(emit) = emit.as_deref_mut()
                    {
                        if let Ok(timing) = emit(GamescopeLiveSessionEvent::Observation {
                            screen_episode_id,
                            sequence,
                            monotonic_start_ms,
                            monotonic_end_ms,
                            output: &output,
                        }) {
                            Some(timing)
                        } else {
                            output_failed = true;
                            None
                        }
                    } else {
                        None
                    };
                    if let Some(output_timing) = output_timing {
                        timing.add_live_processing(output_timing);
                    }
                    if let (Some(version), Some(emit)) =
                        (identified_version.as_deref(), emit.as_deref_mut())
                    {
                        match emit(GamescopeLiveSessionEvent::GameVersionIdentified {
                            source_sequence: sequence,
                            version,
                        }) {
                            Ok(output_timing) => timing.add_live_processing(output_timing),
                            Err(_) => output_failed = true,
                        }
                    }
                    timing.finish_wall();
                    let output = output.with_frame_timing(
                        scorepeek_core::model::session::RecognitionFrameTiming {
                            screen_classification_us: timing.screen_classification_us,
                            crop_prepare_us: timing.crop_prepare_us,
                            screen_resolver_us: timing.screen_resolver_us,
                            attempt_resolver_us: timing.attempt_resolver_us,
                            output_us: timing.output_us,
                            frame_processing_wall_us: timing.frame_processing_wall_us,
                        },
                    );
                    let _ = session.record_frame_processing_timing(
                        timing,
                        if late {
                            scorepeek_core::diagnostics::FrameFieldStatus::LateEpisode
                        } else {
                            scorepeek_core::diagnostics::FrameFieldStatus::Completed
                        },
                        Some(output.processing_timing()),
                    );
                    if observation_screen != ScreenClass::Title
                        && let Some(worker) = artifact_worker
                    {
                        let counter = match worker.try_record_in_episode(
                            sequence,
                            screen_episode_id,
                            monotonic_start_ms,
                            monotonic_end_ms,
                            if late {
                                scorepeek_core::diagnostics::FrameFieldStatus::LateEpisode
                            } else {
                                scorepeek_core::diagnostics::FrameFieldStatus::Completed
                            },
                            output,
                        ) {
                            RecognitionArtifactEnqueueOutcome::Enqueued => {
                                &mut counters.recognition_artifact_enqueued
                            }
                            RecognitionArtifactEnqueueOutcome::QueueFull => {
                                &mut counters.recognition_artifact_queue_full
                            }
                            RecognitionArtifactEnqueueOutcome::WorkerUnavailable => {
                                &mut counters.recognition_artifact_worker_unavailable
                            }
                        };
                        *counter = counter.saturating_add(1);
                    }
                    if output_failed {
                        return Some(FieldObservationGateErrorType::ResultOutputFailed);
                    }
                } else {
                    let _ = session.record_frame_processing_timing(
                        timing,
                        scorepeek_core::diagnostics::FrameFieldStatus::Failed,
                        None,
                    );
                    counters.field_ready_failure = counters.field_ready_failure.saturating_add(1);
                    if observation_screen == ScreenClass::Title {
                        if let Some(resolver) = game_version.as_deref_mut() {
                            resolver.observe_failure(sequence);
                        }
                    } else {
                        return Some(FieldObservationGateErrorType::FieldObservationFailed);
                    }
                }
                if wait.is_some() {
                    return None;
                }
            }
            FieldObservationSessionPoll::Consumed
            | FieldObservationSessionPoll::BindingMismatch
            | FieldObservationSessionPoll::Terminal => {
                pending.remove(index);
                return Some(FieldObservationGateErrorType::DiagnosticConfigurationInvalid);
            }
            FieldObservationSessionPoll::WorkerUnavailable => {
                pending.remove(index);
                return Some(FieldObservationGateErrorType::FieldObserverUnavailable);
            }
        }
    }
    None
}

pub(super) fn wait_live_field_observations(
    session: &mut FieldObservationSession<RegisteredScreenFieldObserver>,
    pending: &mut Vec<PendingSessionFieldObservation<RegisteredFieldOutput>>,
    counters: &mut FieldObservationCounters,
    artifact_worker: &mut Option<RecognitionArtifactWorker>,
    mut emit: Option<&mut LiveEventEmitter<'_>>,
    minimum_event_sequence: Option<u64>,
    game_version: Option<&mut GameVersionResolver>,
) -> Option<(FieldObservationGateErrorType, Option<CaptureErrorType>)> {
    let mut game_version = game_version;
    let started = Instant::now();
    while !pending.is_empty() {
        let remaining = DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        if let Some(error) = poll_field_observations(
            session,
            pending,
            counters,
            artifact_worker,
            Some(remaining),
            emit.as_deref_mut(),
            minimum_event_sequence,
            game_version.as_deref_mut(),
        ) {
            return Some((error, None));
        }
    }
    (!pending.is_empty()).then_some((
        FieldObservationGateErrorType::FieldObserverFinishFailed,
        None,
    ))
}

pub(super) struct FieldObservationFinishOutcomes {
    pub(super) field_observer:
        Option<crate::service::session::recognition::field_observer::FieldObserverFinishOutcome>,
    pub(super) diagnostic: Option<crate::diagnostics::writer::DiagnosticFinishOutcome>,
    pub(super) recognition_artifact: Option<RecognitionArtifactFinishOutcome>,
    pub(super) artifact_requested: bool,
}

pub(super) fn result_evidence_error(
    counters: &FieldObservationCounters,
) -> Option<FieldObservationGateErrorType> {
    (counters.result_observations == 0)
        .then_some(FieldObservationGateErrorType::ResultObservationUnavailable)
}

pub(super) fn recognition_artifact_error(
    counters: &FieldObservationCounters,
    outcome: Option<&RecognitionArtifactFinishOutcome>,
) -> Option<FieldObservationGateErrorType> {
    let complete = matches!(
        outcome,
        Some(RecognitionArtifactFinishOutcome {
            status: RecognitionArtifactFinishStatus::Complete,
            manifest_sha256: Some(_),
            ..
        })
    ) && counters.recognition_artifact_queue_full == 0
        && counters.recognition_artifact_worker_unavailable == 0
        && counters.recognition_artifact_enqueued == counters.field_ready_success
        && outcome.is_some_and(|outcome| {
            outcome.input_observations == outcome.retained_observations
                && u64::try_from(outcome.input_observations).ok()
                    == Some(counters.recognition_artifact_enqueued)
        });
    (!complete).then_some(FieldObservationGateErrorType::RecognitionArtifactIncomplete)
}

#[allow(clippy::too_many_lines)]
pub(super) fn field_observation_report(
    error_type: Option<FieldObservationGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: CaptureGeneration,
    counters: FieldObservationCounters,
    finishes: FieldObservationFinishOutcomes,
    sink: BoundedDiagnosticSink,
) -> GamescopeFieldObservationGateReport {
    let (
        field_worker_status,
        field_worker_submitted,
        field_worker_completed,
        field_worker_abandoned,
    ) = finishes
        .field_observer
        .map_or((None, None, None, None), |outcome| {
            let status = match outcome.status {
                FieldObserverFinishStatus::Complete => FieldWorkerStatus::Complete,
                FieldObserverFinishStatus::Timeout => FieldWorkerStatus::Timeout,
                FieldObserverFinishStatus::WorkerUnavailable => {
                    FieldWorkerStatus::WorkerUnavailable
                }
            };
            (
                Some(status),
                Some(outcome.submitted),
                outcome.completed,
                outcome.abandoned,
            )
        });
    let (diagnostic_completeness, diagnostic_error_type, diagnostic_manifest_sha256) =
        finishes.diagnostic.map_or((None, None, None), |outcome| {
            (
                outcome.completeness,
                outcome.error_type,
                outcome.manifest_sha256,
            )
        });
    let (
        recognition_artifact_status,
        recognition_artifact_manifest_sha256,
        recognition_artifact_input_observations,
        recognition_artifact_retained_observations,
    ) = finishes
        .recognition_artifact
        .map_or((None, None, None, None), |outcome| {
            (
                Some(outcome.status),
                outcome.manifest_sha256,
                Some(outcome.input_observations),
                Some(outcome.retained_observations),
            )
        });
    GamescopeFieldObservationGateReport {
        schema: if finishes.artifact_requested {
            "scorepeek-gamescope-result-recognition-gate-v1"
        } else {
            "scorepeek-gamescope-field-observation-gate-v1"
        },
        status: if error_type.is_some() {
            LiveGateStatus::Error
        } else {
            LiveGateStatus::Success
        },
        error_type,
        capture_error_type,
        capture_generation: capture_generation.get(),
        observed_frames: counters.observed_frames,
        normalized_frames: counters.normalized_frames,
        recognition_ticks: counters.recognition_ticks,
        recognition_busy_skips: counters.recognition_busy_skips,
        maximum_consecutive_busy_skips: counters.maximum_consecutive_busy_skips,
        field_observation_busy_skips: counters.field_observation_busy_skips,
        maximum_consecutive_field_observation_busy_skips: counters
            .maximum_consecutive_field_observation_busy_skips,
        last_recognition_sequence: counters.last_recognition_sequence,
        inspected_frames: counters.inspected_frames,
        title_frames: counters.title_frames,
        result_frames: counters.result_frames,
        music_select_frames: counters.music_select_frames,
        mode_select_frames: counters.mode_select_frames,
        decide_transition_frames: counters.decide_transition_frames,
        play_frames: counters.play_frames,
        unknown_frames: counters.unknown_frames,
        field_not_applicable: counters.field_not_applicable,
        field_submitted: counters.field_submitted,
        field_rejected: counters.field_rejected,
        field_ready_success: counters.field_ready_success,
        field_ready_failure: counters.field_ready_failure,
        candidate_sets: counters.candidate_sets,
        scored_candidates: counters.scored_candidates,
        result_observations: finishes
            .artifact_requested
            .then_some(counters.result_observations),
        recognition_artifact_enqueued: finishes
            .artifact_requested
            .then_some(counters.recognition_artifact_enqueued),
        recognition_artifact_queue_full: finishes
            .artifact_requested
            .then_some(counters.recognition_artifact_queue_full),
        recognition_artifact_worker_unavailable: finishes
            .artifact_requested
            .then_some(counters.recognition_artifact_worker_unavailable),
        recognition_artifact_status: finishes
            .artifact_requested
            .then_some(recognition_artifact_status)
            .flatten(),
        recognition_artifact_manifest_sha256: finishes
            .artifact_requested
            .then_some(recognition_artifact_manifest_sha256)
            .flatten(),
        recognition_artifact_input_observations: finishes
            .artifact_requested
            .then_some(recognition_artifact_input_observations)
            .flatten(),
        recognition_artifact_retained_observations: finishes
            .artifact_requested
            .then_some(recognition_artifact_retained_observations)
            .flatten(),
        canonical_recording_completeness: None,
        canonical_recording_manifest_published: false,
        field_worker_status,
        field_worker_submitted,
        field_worker_completed,
        field_worker_abandoned,
        diagnostic_completeness,
        diagnostic_error_type,
        diagnostic_manifest_sha256,
        capture_diagnostic_facts: sink.facts,
        dropped_capture_diagnostic_facts: sink.dropped,
        session_stop_reason: None,
        failure_detail: None,
    }
}
