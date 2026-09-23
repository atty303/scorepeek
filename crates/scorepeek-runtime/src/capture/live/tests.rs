use scorepeek::capture::{
    CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
    CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType, CaptureSourceKind,
};
use scorepeek_resources::recognition::{
    RegisteredResourceLoadError, RegisteredResourceLoadErrorType,
};
use std::ffi::OsStr;

use super::{
    BoundedDiagnosticSink, FieldObservationCounters, FieldObservationFinishOutcomes,
    FieldObservationGateErrorType, GamescopeLiveGateReport, LifecycleGateErrorType,
    LifecyclePhaseStatus, LiveGateStatus, MAX_DIAGNOSTIC_FACTS, field_observation_report,
    field_resource_error, field_start_error, lifecycle_error_type, parse_consumer_interval_ms,
    parse_duration_ms, parse_lifecycle_runs, process_resource_snapshot, reconnectable_stop_reason,
    summarize_run,
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
fn counts_only_field_report_retains_its_v1_shape() {
    let report = field_observation_report(
        None,
        None,
        FieldObservationCounters::default(),
        FieldObservationFinishOutcomes {
            field_observer: None,
            diagnostic: None,
        },
        BoundedDiagnosticSink::default(),
    );
    let encoded = serde_json::to_string(&report).unwrap();

    assert!(encoded.contains("scorepeek-capture-field-observation-v2"));
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
            FieldObservationCounters::default(),
            FieldObservationFinishOutcomes {
                field_observer: None,
                diagnostic: None,
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
            FieldObservationCounters::default(),
            FieldObservationFinishOutcomes {
                field_observer: None,
                diagnostic: None,
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
        FieldObservationCounters::default(),
        FieldObservationFinishOutcomes {
            field_observer: None,
            diagnostic: None,
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
fn field_gate_keeps_registered_resource_failures_actionable() {
    assert_eq!(
        field_resource_error(RegisteredResourceLoadErrorType::CatalogBindingMismatch),
        FieldObservationGateErrorType::CatalogBindingMismatch
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
