use std::ffi::OsStr;
use std::fs;

use crate::diagnostics::contract::{DiagnosticBinding, DiagnosticResource};
use scorepeek::capture::{
    CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
    CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType, CaptureGeneration,
    CaptureSourceKind, FractionalRectangle, GamescopeProfileBinding,
    GamescopeProfileBindingAuthoringInput, RationalCoordinate, UncalibratedMemoryType,
    UncalibratedVideoContract,
};
use scorepeek_core::frame::CanonicalLayout;
use scorepeek_resources::recognition::{
    RegisteredResourceLoadError, RegisteredResourceLoadErrorType,
};

use super::{
    BoundedDiagnosticSink, DiagnosticPolicy, DiagnosticRunDescriptor, FieldObservationCounters,
    FieldObservationFinishOutcomes, FieldObservationGateErrorType, GamescopeLiveGateReport,
    LifecycleGateErrorType, LifecyclePhaseStatus, LiveGateStatus, MAX_DIAGNOSTIC_FACTS,
    RecognitionArtifactFinishOutcome, RecognitionArtifactFinishStatus, field_observation_report,
    field_resource_error, field_start_error, lifecycle_error_type, parse_consumer_interval_ms,
    parse_duration_ms, parse_lifecycle_runs, process_resource_snapshot, read_binding,
    recognition_artifact_error, reconnectable_stop_reason, result_evidence_error,
    run_gamescope_binding_admission_gate, run_gamescope_canonical_frame_gate,
    run_gamescope_diagnostic_handoff_gate, run_gamescope_recognition_handoff_gate, summarize_run,
};

#[test]
fn transport_disappearance_and_contract_change_are_reconnectable() {
    for error in [CaptureErrorType::SourceLost, CaptureErrorType::StreamLost] {
        assert_eq!(
            reconnectable_stop_reason(Some((
                FieldObservationGateErrorType::CaptureFailed,
                Some(error),
            ))),
            Some(super::LiveSessionStopReason::SourceEnded)
        );
    }
    assert_eq!(
        reconnectable_stop_reason(Some((
            FieldObservationGateErrorType::CaptureFailed,
            Some(CaptureErrorType::SourceContractChanged),
        ))),
        Some(super::LiveSessionStopReason::SourceContractChanged)
    );
    for error in [
        CaptureErrorType::UnsupportedFormat,
        CaptureErrorType::UnsupportedMemoryType,
        CaptureErrorType::ReceiverFailed,
    ] {
        assert!(
            reconnectable_stop_reason(Some((
                FieldObservationGateErrorType::CaptureFailed,
                Some(error),
            )))
            .is_none()
        );
    }
}

fn diagnostic_descriptor(generation: u64) -> DiagnosticRunDescriptor {
    DiagnosticRunDescriptor {
        run_id: "handoff-test".to_owned(),
        monotonic_start_ms: 0,
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: "1".repeat(64),
        },
        binding: DiagnosticBinding {
            capture_generation: generation,
            capture_profile_sha256: String::new(),
            normalizer_sha256: String::new(),
            canonical_layout_sha256: CanonicalLayout::sha256(),
            catalog_sha256: "3".repeat(64),
            model_sha256: "4".repeat(64),
            runtime_sha256: "5".repeat(64),
            replay: None,
        },
    }
}

#[test]
fn duration_is_explicitly_bounded() {
    assert_eq!(parse_duration_ms(OsStr::new("1")).unwrap(), 1);
    assert_eq!(parse_duration_ms(OsStr::new("60000")).unwrap(), 60_000);
    assert!(parse_duration_ms(OsStr::new("0")).is_err());
    assert!(parse_duration_ms(OsStr::new("60001")).is_err());
    assert!(parse_duration_ms(OsStr::new("forever")).is_err());
}

#[test]
fn consumer_interval_is_explicitly_bounded() {
    assert_eq!(parse_consumer_interval_ms(OsStr::new("0")).unwrap(), 0);
    assert_eq!(
        parse_consumer_interval_ms(OsStr::new("60000")).unwrap(),
        60_000
    );
    assert!(parse_consumer_interval_ms(OsStr::new("60001")).is_err());
    assert!(parse_consumer_interval_ms(OsStr::new("sometimes")).is_err());
}

#[test]
fn binding_gate_reads_only_digest_selected_bounded_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("binding.json");
    let video = UncalibratedVideoContract {
        width: 4,
        height: 2,
        framerate_num: 60,
        framerate_denom: 1,
        maximum_framerate_num: 0,
        maximum_framerate_denom: 0,
        pixel_aspect_num: 0,
        pixel_aspect_denom: 0,
        chroma_site: 0,
        color_range: 0,
        color_matrix: 0,
        transfer_function: 0,
        color_primaries: 0,
    };
    let authored = GamescopeProfileBinding::author(GamescopeProfileBindingAuthoringInput {
        calibration_evidence_sha256: "1".repeat(64),
        environment_id: "test-machine".to_owned(),
        gamescope_version: "3.16.19".to_owned(),
        backend_id: "sdl".to_owned(),
        output_width: 4,
        output_height: 2,
        nested_width: 4,
        nested_height: 2,
        nested_refresh_hz: 60,
        scaler: "auto".to_owned(),
        filter: "linear".to_owned(),
        observed_video_contract: video,
        memory_type: UncalibratedMemoryType::MemoryPointer,
        stride: 16,
        geometry: FractionalRectangle::new(
            RationalCoordinate::new(0, 1).unwrap(),
            RationalCoordinate::new(0, 1).unwrap(),
            RationalCoordinate::new(4, 1).unwrap(),
            RationalCoordinate::new(2, 1).unwrap(),
        ),
    })
    .unwrap();
    fs::write(&path, &authored.bytes).unwrap();

    assert!(read_binding(&path, &authored.artifact_sha256).is_ok());
    assert!(read_binding(&path, &"f".repeat(64)).is_err());
    fs::write(&path, vec![0; super::MAX_BINDING_BYTES + 1]).unwrap();
    assert!(read_binding(&path, &authored.artifact_sha256).is_err());
}

#[test]
fn binding_gate_failure_report_omits_paths_and_session_values() {
    let path = std::path::Path::new("/PRIVATE/BINDING/PATH");
    let report = run_gamescope_binding_admission_gate(path, &"f".repeat(64));
    let encoded = serde_json::to_string(&report).unwrap();

    assert!(!encoded.contains("PRIVATE"));
    assert!(encoded.contains("binding_unavailable"));
}

#[test]
fn canonical_gate_failure_report_omits_paths_and_session_values() {
    let report = run_gamescope_canonical_frame_gate(
        std::path::Path::new("/PRIVATE/BINDING/PATH"),
        &"f".repeat(64),
        CaptureGeneration::new(7).unwrap(),
    );
    let encoded = serde_json::to_string(&report).unwrap();

    assert!(!encoded.contains("PRIVATE"));
    assert!(encoded.contains("binding_unavailable"));
    assert!(encoded.contains("\"capture_generation\":7"));
    assert!(encoded.contains("\"canonical_rgb8_sha256\":null"));
}

#[test]
fn diagnostic_handoff_failure_report_omits_paths_and_session_values() {
    let _session = scorepeek::capture::GamescopeSessionProvenance::new(
        scorepeek::capture::GamescopeSessionProvenanceInput {
            environment_id: "PRIVATE-ENVIRONMENT".to_owned(),
            gamescope_version: "PRIVATE-VERSION".to_owned(),
            backend_id: "PRIVATE-BACKEND".to_owned(),
            output_width: 4,
            output_height: 2,
            nested_width: 4,
            nested_height: 2,
            nested_refresh_hz: 60,
            scaler: "auto".to_owned(),
            filter: "linear".to_owned(),
        },
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let generation = CaptureGeneration::new(9).unwrap();
    let report =
        run_gamescope_diagnostic_handoff_gate(super::GamescopeDiagnosticHandoffGateConfig {
            binding_path: std::path::Path::new("/PRIVATE/BINDING/PATH"),
            expected_binding_sha256: &"f".repeat(64),
            capture_generation: generation,
            descriptor: diagnostic_descriptor(generation.get()),
            policy: DiagnosticPolicy::default(),
            duration_ms: 1_000,
            diagnostic_root: root.path(),
            diagnostic_directory_name: None,
            expected_source_node_id: None,
        });
    let encoded = serde_json::to_string(&report).unwrap();

    assert!(!encoded.contains("PRIVATE"));
    assert!(encoded.contains("binding_unavailable"));
    assert!(encoded.contains("\"capture_generation\":9"));
    assert_eq!(root.path().read_dir().unwrap().count(), 0);
}

#[test]
fn recognition_handoff_failure_is_typed_without_private_values() {
    let _session = scorepeek::capture::GamescopeSessionProvenance::new(
        scorepeek::capture::GamescopeSessionProvenanceInput {
            environment_id: "PRIVATE-ENVIRONMENT".to_owned(),
            gamescope_version: "PRIVATE-VERSION".to_owned(),
            backend_id: "PRIVATE-BACKEND".to_owned(),
            output_width: 4,
            output_height: 2,
            nested_width: 4,
            nested_height: 2,
            nested_refresh_hz: 60,
            scaler: "auto".to_owned(),
            filter: "linear".to_owned(),
        },
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let generation = CaptureGeneration::new(10).unwrap();
    let mut descriptor = diagnostic_descriptor(generation.get());
    descriptor.binding.canonical_layout_sha256 = "2".repeat(64);
    let report =
        run_gamescope_recognition_handoff_gate(super::GamescopeDiagnosticHandoffGateConfig {
            binding_path: std::path::Path::new("/PRIVATE/BINDING/PATH"),
            expected_binding_sha256: &"f".repeat(64),
            capture_generation: generation,
            descriptor,
            policy: DiagnosticPolicy::default(),
            duration_ms: 1_000,
            diagnostic_root: root.path(),
            diagnostic_directory_name: None,
            expected_source_node_id: None,
        });
    let encoded = serde_json::to_string(&report).unwrap();

    assert!(!encoded.contains("PRIVATE"));
    assert!(encoded.contains("diagnostic_configuration_invalid"));
    assert!(encoded.contains("\"capture_generation\":10"));
    assert!(encoded.contains("\"inspected_frames\":0"));
    assert_eq!(root.path().read_dir().unwrap().count(), 0);
}

#[test]
fn lifecycle_run_count_is_explicitly_bounded() {
    assert_eq!(parse_lifecycle_runs(OsStr::new("2")).unwrap(), 2);
    assert_eq!(parse_lifecycle_runs(OsStr::new("100")).unwrap(), 100);
    assert!(parse_lifecycle_runs(OsStr::new("1")).is_err());
    assert!(parse_lifecycle_runs(OsStr::new("101")).is_err());
    assert!(parse_lifecycle_runs(OsStr::new("many")).is_err());
}

#[test]
fn lifecycle_error_precedence_is_stable() {
    assert_eq!(
        lifecycle_error_type(true, true, true),
        Some(LifecycleGateErrorType::CaptureRunFailed)
    );
    assert_eq!(
        lifecycle_error_type(false, true, true),
        Some(LifecycleGateErrorType::ProcessResourceUnavailable)
    );
    assert_eq!(
        lifecycle_error_type(false, false, true),
        Some(LifecycleGateErrorType::ExpectedOverwriteMissing)
    );
    assert_eq!(lifecycle_error_type(false, false, false), None);
}

#[test]
fn lifecycle_summary_extracts_only_typed_receiver_facts() {
    let report = GamescopeLiveGateReport {
        schema: "test",
        status: LiveGateStatus::Success,
        requested_duration_ms: 100,
        consumer_interval_ms: 25,
        consumed_frames: 2,
        first_sequence: Some(0),
        last_sequence: Some(4),
        error_type: None,
        diagnostic_facts: vec![
            CaptureDiagnosticFact {
                sequence: 0,
                monotonic_start_ms: 0,
                monotonic_end_ms: 1,
                operation: CaptureDiagnosticOperation::StreamNegotiation,
                status: CaptureDiagnosticStatus::Success,
                error_type: None,
                detail: CaptureDiagnosticDetail::StreamNegotiation {
                    format: "BGRx",
                    requested_framerate_num: 60,
                    requested_framerate_denom: 1,
                    width: 1280,
                    height: 720,
                    framerate_num: 60,
                    framerate_denom: 1,
                    maximum_framerate_num: 60,
                    maximum_framerate_denom: 1,
                    pixel_aspect_num: 1,
                    pixel_aspect_denom: 1,
                    chroma_site: 0,
                    color_range: 0,
                    color_matrix: 0,
                    transfer_function: 0,
                    color_primaries: 0,
                },
            },
            CaptureDiagnosticFact {
                sequence: 1,
                monotonic_start_ms: 1,
                monotonic_end_ms: 2,
                operation: CaptureDiagnosticOperation::FirstFrame,
                status: CaptureDiagnosticStatus::Success,
                error_type: None,
                detail: CaptureDiagnosticDetail::FirstFrame {
                    memory_type: "mem_fd",
                    stride: 5120,
                    byte_count: 3_686_400,
                },
            },
            CaptureDiagnosticFact {
                sequence: 2,
                monotonic_start_ms: 2,
                monotonic_end_ms: 100,
                operation: CaptureDiagnosticOperation::SteadyReception,
                status: CaptureDiagnosticStatus::Success,
                error_type: None,
                detail: CaptureDiagnosticDetail::SteadyReception {
                    received_frames: 5,
                    overwritten_frames: 3,
                    last_sequence: Some(4),
                    maximum_gap_ns: 17_000_000,
                },
            },
            CaptureDiagnosticFact {
                sequence: 3,
                monotonic_start_ms: 100,
                monotonic_end_ms: 101,
                operation: CaptureDiagnosticOperation::ReceiverShutdown,
                status: CaptureDiagnosticStatus::Success,
                error_type: None,
                detail: CaptureDiagnosticDetail::ReceiverShutdown {
                    received_frames: 5,
                    overwritten_frames: 3,
                },
            },
            CaptureDiagnosticFact {
                sequence: 4,
                monotonic_start_ms: 101,
                monotonic_end_ms: 102,
                operation: CaptureDiagnosticOperation::Shutdown,
                status: CaptureDiagnosticStatus::Success,
                error_type: None,
                detail: CaptureDiagnosticDetail::Shutdown {
                    source: CaptureSourceKind::Pipewire,
                },
            },
        ],
        dropped_diagnostic_facts: 0,
    };

    let summary = summarize_run(7, &report);
    assert_eq!(summary.run, 7);
    assert_eq!(summary.received_frames, 5);
    assert_eq!(summary.overwritten_frames, 3);
    assert_eq!(summary.last_sequence, Some(4));
    assert_eq!(summary.maximum_gap_ns, 17_000_000);
    assert_lifecycle_phases_succeeded(&summary);
    assert_eq!(summary.error_type, None::<CaptureErrorType>);
}

fn assert_lifecycle_phases_succeeded(summary: &super::LifecycleRunSummary) {
    assert_eq!(summary.phases.negotiation, LifecyclePhaseStatus::Success);
    assert_eq!(summary.phases.first_frame, LifecyclePhaseStatus::Success);
    assert_eq!(
        summary.phases.receiver_shutdown,
        LifecyclePhaseStatus::Success
    );
    assert_eq!(
        summary.phases.provider_shutdown,
        LifecyclePhaseStatus::Success
    );
}

#[test]
fn process_resources_are_available_on_linux() {
    let snapshot = process_resource_snapshot().unwrap();
    assert!(snapshot.open_file_descriptors > 0);
    assert!(snapshot.threads > 0);
    assert!(snapshot.resident_bytes > 0);
}

#[test]
fn diagnostic_sink_drops_new_facts_at_capacity() {
    let mut sink = BoundedDiagnosticSink::default();
    for sequence in 0..=MAX_DIAGNOSTIC_FACTS as u64 {
        sink.record(CaptureDiagnosticFact {
            sequence,
            monotonic_start_ms: 0,
            monotonic_end_ms: 0,
            operation: CaptureDiagnosticOperation::Shutdown,
            status: CaptureDiagnosticStatus::Success,
            error_type: None,
            detail: CaptureDiagnosticDetail::Shutdown {
                source: CaptureSourceKind::Pipewire,
            },
        });
    }
    assert_eq!(sink.facts.len(), MAX_DIAGNOSTIC_FACTS);
    assert_eq!(sink.dropped, 1);
    assert_eq!(sink.take_pending().len(), MAX_DIAGNOSTIC_FACTS);
    assert!(sink.take_pending().is_empty());
    assert_eq!(sink.facts.len(), MAX_DIAGNOSTIC_FACTS);
}

#[test]
fn compact_field_report_links_to_value_bearing_artifact_without_duplicating_it() {
    let mut report = field_observation_report(
        None,
        None,
        CaptureGeneration::new(7).unwrap(),
        FieldObservationCounters {
            observed_frames: 3,
            normalized_frames: 3,
            inspected_frames: 3,
            title_frames: 0,
            result_frames: 1,
            music_select_frames: 1,
            unknown_frames: 1,
            field_not_applicable: 1,
            field_submitted: 2,
            field_ready_success: 2,
            candidate_sets: 2,
            scored_candidates: 10,
            result_observations: 2,
            recognition_artifact_enqueued: 2,
            ..FieldObservationCounters::default()
        },
        FieldObservationFinishOutcomes {
            field_observer: None,
            diagnostic: None,
            recognition_artifact: Some(RecognitionArtifactFinishOutcome {
                status: RecognitionArtifactFinishStatus::Complete,
                manifest_sha256: Some("e".repeat(64)),
                input_observations: 2,
                retained_observations: 2,
            }),
            artifact_requested: true,
        },
        BoundedDiagnosticSink::default(),
    );
    report.failure_detail = Some("operator-only resource cause".to_owned());
    let encoded = serde_json::to_string(&report).unwrap();

    assert!(encoded.contains("\"candidate_sets\":2"));
    assert!(encoded.contains("\"scored_candidates\":10"));
    assert!(encoded.contains("scorepeek-gamescope-result-recognition-gate-v1"));
    assert!(encoded.contains("\"recognition_artifact_status\":\"complete\""));
    assert!(encoded.contains(&"e".repeat(64)));
    assert!(report.succeeded());
    for forbidden in ["open_text", "song_id", "pixels", "environment_id"] {
        assert!(!encoded.contains(forbidden));
    }
    assert!(!encoded.contains("operator-only resource cause"));
}

#[test]
fn counts_only_field_report_retains_its_v1_shape() {
    let report = field_observation_report(
        None,
        None,
        CaptureGeneration::new(1).unwrap(),
        FieldObservationCounters::default(),
        FieldObservationFinishOutcomes {
            field_observer: None,
            diagnostic: None,
            recognition_artifact: None,
            artifact_requested: false,
        },
        BoundedDiagnosticSink::default(),
    );
    let encoded = serde_json::to_string(&report).unwrap();

    assert!(encoded.contains("scorepeek-gamescope-field-observation-gate-v1"));
    assert!(!encoded.contains("result_observations"));
    assert!(!encoded.contains("recognition_artifact"));
}

#[test]
fn startup_retry_classification_distinguishes_transient_boundaries() {
    for error_type in [
        FieldObservationGateErrorType::CaptureFailed,
        FieldObservationGateErrorType::AdmissionRejected,
    ] {
        let report = field_observation_report(
            Some(error_type),
            None,
            CaptureGeneration::new(1).unwrap(),
            FieldObservationCounters::default(),
            FieldObservationFinishOutcomes {
                field_observer: None,
                diagnostic: None,
                recognition_artifact: None,
                artifact_requested: false,
            },
            BoundedDiagnosticSink::default(),
        );
        assert_eq!(
            report.startup_retry(),
            Some(super::LiveSessionStartupRetry::Admission)
        );
    }

    for error_type in [
        FieldObservationGateErrorType::CatalogUnavailable,
        FieldObservationGateErrorType::CatalogBindingMismatch,
        FieldObservationGateErrorType::CatalogLoadFailed,
    ] {
        let report = field_observation_report(
            Some(error_type),
            None,
            CaptureGeneration::new(1).unwrap(),
            FieldObservationCounters::default(),
            FieldObservationFinishOutcomes {
                field_observer: None,
                diagnostic: None,
                recognition_artifact: None,
                artifact_requested: false,
            },
            BoundedDiagnosticSink::default(),
        );
        assert_eq!(
            report.startup_retry(),
            Some(super::LiveSessionStartupRetry::Catalog)
        );
    }

    let mut report = field_observation_report(
        Some(FieldObservationGateErrorType::DiagnosticConfigurationInvalid),
        None,
        CaptureGeneration::new(1).unwrap(),
        FieldObservationCounters::default(),
        FieldObservationFinishOutcomes {
            field_observer: None,
            diagnostic: None,
            recognition_artifact: None,
            artifact_requested: false,
        },
        BoundedDiagnosticSink::default(),
    );
    report.failure_detail = Some("catalog binding does not match".to_owned());

    assert_eq!(report.startup_retry(), None);
    assert_eq!(
        report.startup_failure_summary(),
        "catalog binding does not match"
    );
}

#[test]
fn result_evidence_requires_a_completed_result_observation() {
    let counters = FieldObservationCounters {
        field_ready_success: 1,
        candidate_sets: 1,
        recognition_artifact_enqueued: 1,
        ..FieldObservationCounters::default()
    };
    assert_eq!(
        result_evidence_error(&counters),
        Some(FieldObservationGateErrorType::ResultObservationUnavailable)
    );
}

#[test]
fn artifact_failures_produce_the_same_error_report_status_as_cli_failure() {
    let complete = RecognitionArtifactFinishOutcome {
        status: RecognitionArtifactFinishStatus::Complete,
        manifest_sha256: Some("f".repeat(64)),
        input_observations: 1,
        retained_observations: 1,
    };
    let cases = [
        (
            FieldObservationCounters {
                field_ready_success: 1,
                result_observations: 1,
                recognition_artifact_enqueued: 1,
                recognition_artifact_queue_full: 1,
                ..FieldObservationCounters::default()
            },
            RecognitionArtifactFinishOutcome {
                status: RecognitionArtifactFinishStatus::Complete,
                manifest_sha256: Some("f".repeat(64)),
                input_observations: 1,
                retained_observations: 1,
            },
        ),
        (
            FieldObservationCounters {
                field_ready_success: 1,
                result_observations: 1,
                recognition_artifact_enqueued: 1,
                ..FieldObservationCounters::default()
            },
            RecognitionArtifactFinishOutcome {
                status: RecognitionArtifactFinishStatus::WriteFailed,
                manifest_sha256: None,
                input_observations: 1,
                retained_observations: 0,
            },
        ),
        (
            FieldObservationCounters {
                field_ready_success: 1,
                result_observations: 1,
                recognition_artifact_enqueued: 1,
                ..FieldObservationCounters::default()
            },
            RecognitionArtifactFinishOutcome {
                status: RecognitionArtifactFinishStatus::Timeout,
                manifest_sha256: None,
                input_observations: 1,
                retained_observations: 0,
            },
        ),
    ];

    for (counters, outcome) in cases {
        let error = recognition_artifact_error(&counters, Some(&outcome));
        assert_eq!(
            error,
            Some(FieldObservationGateErrorType::RecognitionArtifactIncomplete)
        );
        let report = field_observation_report(
            error,
            None,
            CaptureGeneration::new(1).unwrap(),
            counters,
            FieldObservationFinishOutcomes {
                field_observer: None,
                diagnostic: None,
                recognition_artifact: Some(outcome),
                artifact_requested: true,
            },
            BoundedDiagnosticSink::default(),
        );
        let encoded = serde_json::to_string(&report).unwrap();
        assert!(!report.succeeded());
        assert!(encoded.contains("\"status\":\"error\""));
        assert!(encoded.contains("\"error_type\":\"recognition_artifact_incomplete\""));
    }

    let counters = FieldObservationCounters {
        field_ready_success: 1,
        result_observations: 1,
        recognition_artifact_enqueued: 1,
        ..FieldObservationCounters::default()
    };
    assert_eq!(recognition_artifact_error(&counters, Some(&complete)), None);
}

#[test]
fn field_gate_keeps_registered_resource_failures_actionable() {
    assert_eq!(
        field_resource_error(RegisteredResourceLoadErrorType::CatalogBindingMismatch),
        FieldObservationGateErrorType::CatalogBindingMismatch
    );
    assert_eq!(
        field_resource_error(RegisteredResourceLoadErrorType::RuntimeInitializationFailed),
        FieldObservationGateErrorType::RuntimeInitializationFailed
    );
    let (error_type, finish, detail) = field_start_error(
            crate::service::session::recognition::field_session::FieldObservationStartError::FieldObserver(
                crate::service::session::recognition::field_observer::FieldObserverStartError::Load(
                    crate::service::session::recognition::screen_field_observer::RegisteredScreenFieldObserverLoadError::Resources(
                        RegisteredResourceLoadError::InvalidLocation {
                            role: "model bundle",
                            source: Some(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
                        },
                    ),
                ),
            ),
        );
    assert_eq!(
        error_type,
        FieldObservationGateErrorType::InvalidResourceLocation
    );
    assert!(finish.is_none());
    let detail = detail.unwrap();
    assert!(detail.contains("model bundle"));
    assert!(detail.contains("permission denied"));
}
