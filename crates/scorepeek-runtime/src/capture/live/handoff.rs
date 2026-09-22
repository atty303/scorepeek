use super::*;

pub(super) fn run_gamescope_handoff_gate(
    mut config: GamescopeDiagnosticHandoffGateConfig<'_>,
    inspect_screen: bool,
) -> HandoffGateRun {
    let capture_generation = config.capture_generation;
    let mut recognition = RecognitionHandoffCounters::default();
    if !valid_handoff_descriptor(&config.descriptor, capture_generation, inspect_screen) {
        return invalid_handoff_configuration(
            capture_generation,
            BoundedDiagnosticSink::default(),
            recognition,
        );
    }
    let binding = match read_diagnostic_handoff_binding(
        config.binding_path,
        config.expected_binding_sha256,
    ) {
        Ok(binding) => binding,
        Err(error_type) => {
            return handoff_gate_run(
                diagnostic_handoff_report(
                    error_type,
                    None,
                    capture_generation,
                    HandoffCounters::default(),
                    None,
                    BoundedDiagnosticSink::default(),
                ),
                recognition,
            );
        }
    };
    bind_diagnostic_descriptor(&mut config.descriptor, &binding);
    let mut sink = BoundedDiagnosticSink::default();
    let mut lease = match start_diagnostic_handoff_capture(
        binding,
        capture_generation,
        config.expected_source_node_id,
        &mut sink,
    ) {
        Ok(lease) => lease,
        Err((error_type, capture_error_type)) => {
            return handoff_gate_run(
                diagnostic_handoff_report(
                    error_type,
                    capture_error_type,
                    capture_generation,
                    HandoffCounters::default(),
                    None,
                    sink,
                ),
                recognition,
            );
        }
    };
    if config.descriptor.binding.capture_profile_sha256 != lease.capture_profile_sha256()
        || config.descriptor.binding.normalizer_sha256 != lease.normalizer_artifact_sha256()
    {
        let _ = lease.shutdown(&mut sink);
        return handoff_gate_run(
            diagnostic_handoff_report(
                DiagnosticHandoffGateErrorType::DiagnosticBindingMismatch,
                None,
                capture_generation,
                HandoffCounters::default(),
                None,
                sink,
            ),
            recognition,
        );
    }
    let Ok(mut session) = start_handoff_session(
        config.diagnostic_root,
        config.descriptor,
        config.policy,
        inspect_screen,
    ) else {
        let _ = lease.shutdown(&mut sink);
        return invalid_handoff_configuration(capture_generation, sink, recognition);
    };
    let mut counters = HandoffCounters::default();
    let mut terminal = offer_diagnostic_handoff_frames(
        &mut lease,
        &mut session,
        Duration::from_millis(config.duration_ms),
        &mut counters,
        inspect_screen,
        &mut recognition,
        &mut sink,
    );
    finish_handoff_gate(
        lease,
        session,
        &mut terminal,
        capture_generation,
        counters,
        recognition,
        sink,
    )
}

pub(super) fn valid_handoff_descriptor(
    descriptor: &DiagnosticRunDescriptor,
    capture_generation: CaptureGeneration,
    inspect_screen: bool,
) -> bool {
    descriptor.binding.capture_generation == capture_generation.get()
        && descriptor.binding.replay.is_none()
        && (!inspect_screen
            || descriptor.binding.canonical_layout_sha256 == CanonicalLayout::sha256())
}

pub(super) fn start_handoff_session(
    root: &std::path::Path,
    descriptor: DiagnosticRunDescriptor,
    policy: DiagnosticPolicy,
    inspect_screen: bool,
) -> Result<HandoffSession, ()> {
    if inspect_screen {
        RecognitionSession::start(root, descriptor, policy)
            .map(HandoffSession::Recognition)
            .map_err(|_| ())
    } else {
        Ok(HandoffSession::Diagnostic(DiagnosticBridge::start(
            root, descriptor, policy,
        )))
    }
}

pub(super) fn invalid_handoff_configuration(
    capture_generation: CaptureGeneration,
    sink: BoundedDiagnosticSink,
    recognition: RecognitionHandoffCounters,
) -> HandoffGateRun {
    handoff_gate_run(
        diagnostic_handoff_report(
            DiagnosticHandoffGateErrorType::DiagnosticConfigurationInvalid,
            None,
            capture_generation,
            HandoffCounters::default(),
            None,
            sink,
        ),
        recognition,
    )
}

pub(super) fn finish_handoff_gate(
    lease: CaptureLease,
    session: HandoffSession,
    terminal: &mut Option<(DiagnosticHandoffGateErrorType, Option<CaptureErrorType>)>,
    capture_generation: CaptureGeneration,
    counters: HandoffCounters,
    recognition: RecognitionHandoffCounters,
    mut sink: BoundedDiagnosticSink,
) -> HandoffGateRun {
    if counters.normalized_frames == 0 && terminal.is_none() {
        *terminal = Some((DiagnosticHandoffGateErrorType::FrameUnavailable, None));
    }
    let (shutdown_result, finish_time) = lease.shutdown_with_elapsed(&mut sink);
    if let Err(error) = shutdown_result {
        terminal.get_or_insert((
            DiagnosticHandoffGateErrorType::ShutdownFailed,
            Some(error.error_type()),
        ));
    }
    let finish_status = if terminal.is_some() {
        DiagnosticRunStatus::Error
    } else {
        DiagnosticRunStatus::Success
    };
    let diagnostic_outcome = session.finish(finish_status, finish_time);
    let diagnostic = match *terminal {
        Some((error_type, capture_error_type)) => diagnostic_handoff_report(
            error_type,
            capture_error_type,
            capture_generation,
            counters,
            Some(diagnostic_outcome),
            sink,
        ),
        None => diagnostic_handoff_success(capture_generation, counters, diagnostic_outcome, sink),
    };
    handoff_gate_run(diagnostic, recognition)
}

pub(super) fn handoff_gate_run(
    diagnostic: GamescopeDiagnosticHandoffGateReport,
    recognition: RecognitionHandoffCounters,
) -> HandoffGateRun {
    HandoffGateRun {
        diagnostic,
        recognition,
    }
}

pub(super) fn read_diagnostic_handoff_binding(
    path: &std::path::Path,
    expected_sha256: &str,
) -> Result<GamescopeProfileBinding, DiagnosticHandoffGateErrorType> {
    read_binding(path, expected_sha256).map_err(|error| match error {
        BindingAdmissionGateErrorType::BindingUnavailable => {
            DiagnosticHandoffGateErrorType::BindingUnavailable
        }
        _ => DiagnosticHandoffGateErrorType::BindingInvalid,
    })
}

pub(super) fn bind_diagnostic_descriptor(
    descriptor: &mut DiagnosticRunDescriptor,
    binding: &GamescopeProfileBinding,
) {
    binding
        .capture_profile_sha256()
        .clone_into(&mut descriptor.binding.capture_profile_sha256);
    binding
        .normalizer_artifact_sha256()
        .clone_into(&mut descriptor.binding.normalizer_sha256);
}

pub(super) fn start_diagnostic_handoff_capture(
    binding: GamescopeProfileBinding,
    capture_generation: CaptureGeneration,
    expected_source_node_id: Option<u32>,
    sink: &mut BoundedDiagnosticSink,
) -> Result<CaptureLease, (DiagnosticHandoffGateErrorType, Option<CaptureErrorType>)> {
    let lease = acquire_gamescope_source(DISCOVERY_TIMEOUT, sink).map_err(|error| {
        (
            DiagnosticHandoffGateErrorType::CaptureFailed,
            Some(error.error_type()),
        )
    })?;
    if expected_source_node_id.is_some_and(|expected| expected != lease.node_id()) {
        lease.shutdown(sink);
        return Err((
            DiagnosticHandoffGateErrorType::CaptureFailed,
            Some(CaptureErrorType::SourceLost),
        ));
    }
    let receiver = start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, sink)
        .map_err(|error| {
            (
                DiagnosticHandoffGateErrorType::CaptureFailed,
                Some(error.error_type()),
            )
        })?;
    admit_gamescope_profile(receiver, binding, capture_generation, sink)
        .map(CaptureLease::Pipewire)
        .map_err(|failure| {
            let error_type = failure.error_type();
            let _ = failure.shutdown(sink);
            (
                DiagnosticHandoffGateErrorType::AdmissionRejected,
                Some(error_type),
            )
        })
}

pub(super) fn offer_diagnostic_handoff_frames(
    lease: &mut CaptureLease,
    session: &mut HandoffSession,
    duration: Duration,
    counters: &mut HandoffCounters,
    inspect_screen: bool,
    recognition: &mut RecognitionHandoffCounters,
    sink: &mut BoundedDiagnosticSink,
) -> Option<(DiagnosticHandoffGateErrorType, Option<CaptureErrorType>)> {
    let started = Instant::now();
    loop {
        if let Some(observed) = lease.take_latest_observed_frame() {
            counters.observed_frames = counters.observed_frames.saturating_add(1);
            let (normalized, source) =
                match lease.normalize_observed_frame_with_source(observed, sink) {
                    Ok(pair) => pair,
                    Err(error) => {
                        return Some((
                            DiagnosticHandoffGateErrorType::NormalizationFailed,
                            Some(error.error_type()),
                        ));
                    }
                };
            let live = BoundCanonicalFrame::from_normalized_with_source(normalized, source);
            counters.normalized_frames = counters.normalized_frames.saturating_add(1);
            counters.first_sequence.get_or_insert(live.sequence());
            counters.last_sequence = Some(live.sequence());
            if inspect_screen {
                let HandoffSession::Recognition(recognition_session) = session else {
                    unreachable!("recognition gate owns a recognition session");
                };
                let Ok(result) = recognition_session.inspect(&live) else {
                    recognition.recognition_failures =
                        recognition.recognition_failures.saturating_add(1);
                    return Some((DiagnosticHandoffGateErrorType::RecognitionFailed, None));
                };
                counters.record_offer(result.diagnostic_frame);
                recognition.inspected_frames = recognition.inspected_frames.saturating_add(1);
                let screen_counter = match result.observation.screen() {
                    ScreenClass::Title => &mut recognition.title_frames,
                    ScreenClass::Result => &mut recognition.result_frames,
                    ScreenClass::MusicSelect => &mut recognition.music_select_frames,
                    ScreenClass::ModeSelect => &mut recognition.mode_select_frames,
                    ScreenClass::DecideTransition => &mut recognition.decide_transition_frames,
                    ScreenClass::Play => &mut recognition.play_frames,
                    ScreenClass::Unknown => &mut recognition.unknown_frames,
                };
                *screen_counter = screen_counter.saturating_add(1);
                recognition.fact_outcomes.record(result.diagnostic_fact);
            } else {
                let HandoffSession::Diagnostic(bridge) = session else {
                    unreachable!("diagnostic gate owns a diagnostic bridge");
                };
                counters.record_offer(bridge.offer(&live));
            }
        }
        if started.elapsed() >= duration {
            return None;
        }
        let remaining = duration.saturating_sub(started.elapsed());
        if let Err(error) = lease.poll(remaining, sink) {
            return Some((
                DiagnosticHandoffGateErrorType::CaptureFailed,
                Some(error.error_type()),
            ));
        }
    }
}

pub(super) fn diagnostic_handoff_success(
    capture_generation: CaptureGeneration,
    counters: HandoffCounters,
    diagnostic_outcome: crate::diagnostics::writer::DiagnosticFinishOutcome,
    sink: BoundedDiagnosticSink,
) -> GamescopeDiagnosticHandoffGateReport {
    diagnostic_handoff_report_inner(
        LiveGateStatus::Success,
        None,
        None,
        capture_generation,
        counters,
        Some(diagnostic_outcome),
        sink,
    )
}

pub(super) fn diagnostic_handoff_report(
    error_type: DiagnosticHandoffGateErrorType,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: CaptureGeneration,
    counters: HandoffCounters,
    diagnostic_outcome: Option<crate::diagnostics::writer::DiagnosticFinishOutcome>,
    sink: BoundedDiagnosticSink,
) -> GamescopeDiagnosticHandoffGateReport {
    diagnostic_handoff_report_inner(
        LiveGateStatus::Error,
        Some(error_type),
        capture_error_type,
        capture_generation,
        counters,
        diagnostic_outcome,
        sink,
    )
}

pub(super) fn diagnostic_handoff_report_inner(
    status: LiveGateStatus,
    error_type: Option<DiagnosticHandoffGateErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    capture_generation: CaptureGeneration,
    counters: HandoffCounters,
    diagnostic_outcome: Option<crate::diagnostics::writer::DiagnosticFinishOutcome>,
    sink: BoundedDiagnosticSink,
) -> GamescopeDiagnosticHandoffGateReport {
    let (diagnostic_completeness, diagnostic_error_type, diagnostic_manifest_sha256) =
        diagnostic_outcome.map_or((None, None, None), |outcome| {
            (
                outcome.completeness,
                outcome.error_type,
                outcome.manifest_sha256,
            )
        });
    GamescopeDiagnosticHandoffGateReport {
        schema: "scorepeek-gamescope-diagnostic-handoff-gate-v1",
        status,
        error_type,
        capture_error_type,
        capture_generation: capture_generation.get(),
        observed_frames: counters.observed_frames,
        normalized_frames: counters.normalized_frames,
        first_sequence: counters.first_sequence,
        last_sequence: counters.last_sequence,
        enqueued_frames: counters.enqueued_frames,
        skipped_cadence_frames: counters.skipped_cadence_frames,
        rejected_frames: counters.rejected_frames,
        disabled_frames: counters.disabled_frames,
        queue_full_frames: counters.queue_full_frames,
        worker_unavailable_frames: counters.worker_unavailable_frames,
        diagnostic_completeness,
        diagnostic_error_type,
        diagnostic_manifest_sha256,
        capture_diagnostic_facts: sink.facts,
        dropped_capture_diagnostic_facts: sink.dropped,
    }
}

pub(super) fn recognition_handoff_report(
    run: HandoffGateRun,
) -> GamescopeRecognitionHandoffGateReport {
    let diagnostic = run.diagnostic;
    let recognition = run.recognition;
    GamescopeRecognitionHandoffGateReport {
        schema: "scorepeek-gamescope-recognition-handoff-gate-v1",
        status: diagnostic.status,
        error_type: diagnostic.error_type,
        capture_error_type: diagnostic.capture_error_type,
        capture_generation: diagnostic.capture_generation,
        observed_frames: diagnostic.observed_frames,
        normalized_frames: diagnostic.normalized_frames,
        first_sequence: diagnostic.first_sequence,
        last_sequence: diagnostic.last_sequence,
        diagnostic_frame_enqueued: diagnostic.enqueued_frames,
        diagnostic_frame_skipped_cadence: diagnostic.skipped_cadence_frames,
        diagnostic_frame_rejected: diagnostic.rejected_frames,
        diagnostic_frame_disabled: diagnostic.disabled_frames,
        diagnostic_frame_queue_full: diagnostic.queue_full_frames,
        diagnostic_frame_worker_unavailable: diagnostic.worker_unavailable_frames,
        inspected_frames: recognition.inspected_frames,
        title_frames: recognition.title_frames,
        result_frames: recognition.result_frames,
        music_select_frames: recognition.music_select_frames,
        mode_select_frames: recognition.mode_select_frames,
        decide_transition_frames: recognition.decide_transition_frames,
        play_frames: recognition.play_frames,
        unknown_frames: recognition.unknown_frames,
        recognition_failures: recognition.recognition_failures,
        diagnostic_fact_enqueued: recognition.fact_outcomes.enqueued,
        diagnostic_fact_skipped_cadence: recognition.fact_outcomes.skipped_cadence,
        diagnostic_fact_rejected: recognition.fact_outcomes.rejected,
        diagnostic_fact_disabled: recognition.fact_outcomes.disabled,
        diagnostic_fact_queue_full: recognition.fact_outcomes.queue_full,
        diagnostic_fact_worker_unavailable: recognition.fact_outcomes.worker_unavailable,
        diagnostic_completeness: diagnostic.diagnostic_completeness,
        diagnostic_error_type: diagnostic.diagnostic_error_type,
        diagnostic_manifest_sha256: diagnostic.diagnostic_manifest_sha256,
        capture_diagnostic_facts: diagnostic.capture_diagnostic_facts,
        dropped_capture_diagnostic_facts: diagnostic.dropped_capture_diagnostic_facts,
    }
}

pub(super) fn canonical_frame_success(
    capture_generation: CaptureGeneration,
    capture_profile_sha256: String,
    normalizer_artifact_sha256: String,
    source_sequence: u64,
    canonical_rgb8_sha256: String,
    sink: BoundedDiagnosticSink,
) -> GamescopeCanonicalFrameGateReport {
    GamescopeCanonicalFrameGateReport {
        schema: "scorepeek-gamescope-canonical-frame-gate-v1",
        status: LiveGateStatus::Success,
        error_type: None,
        capture_error_type: None,
        capture_generation: capture_generation.get(),
        capture_profile_sha256: Some(capture_profile_sha256),
        normalizer_artifact_sha256: Some(normalizer_artifact_sha256),
        source_sequence: Some(source_sequence),
        canonical_rgb8_sha256: Some(canonical_rgb8_sha256),
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}
