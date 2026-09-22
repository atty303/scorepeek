use super::{
    CAPTURE_FIELD_OBSERVATION_FLAGS, CAPTURE_HANDOFF_FLAGS, CAPTURE_RESULT_RECOGNITION_FLAGS,
    ConfigFile, LIVE_SESSION_FLAGS, PrivatePublicationPoint, RecordingRetention, RunArgs,
    catalog_paths, command_flag_values, initialize_routine_model, live_session_event_value,
    load_run_options, optional_recognition_root, parse_diagnostic_recording_policy,
    parse_routine_run_options, prepare_live_diagnostic_root, publish_private_file,
    publish_private_file_with, routine_session_disposition, run_command, run_config_command,
    run_startup_stage, run_with_model_initializer, transient_admission_capture_error,
};
use super::{LiveSessionEmission, run_event_from_live_emission};
use crate::capture_live::GamescopeLiveSessionEvent;
use crate::config::document::validate as validate_config_file;
use crate::config::effective as config_effective;
use scorepeek::capture::{
    CaptureDiagnosticDetail, CaptureDiagnosticFact, CaptureDiagnosticOperation,
    CaptureDiagnosticStatus,
};
use scorepeek_core::catalog::Catalog;
use scorepeek_core::catalog::FederationInput;
use scorepeek_core::catalog::{
    Chart, ChartKey, Difficulty, DisplayVariantKind, LineageId, PlayType, RevisionStrategy,
    SourceChartObservation, SourceEvidence, SourceId, SourceObservation, SourcePolicy,
    SourceSnapshot, SourceTitleObservation, TachiObservation,
};
use scorepeek_core::diagnostics::{DiagnosticPolicy, DiagnosticRetention};
use scorepeek_core::event::{RunEvent, RunEventKind};
use scorepeek_core::model::session::RegisteredScreenFieldObservation;
use scorepeek_core::recognition::screen as recognition;
use scorepeek_core::recognition::screen::{ResultScreenFieldObservations, ScreenFieldObservations};
use scorepeek_core::recognition::shared::CatalogCandidateDomain;
use scorepeek_core::recognition::title::DynamicTextObservation;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};

fn result_presence(
    panel_side: recognition::ResultPanelSideState,
) -> recognition::ResultPresenceEvidence {
    let known = panel_side.known();
    recognition::ResultPresenceEvidence {
        warm_pixels: if known.is_some() { 3_100 } else { 2_900 },
        warm_pixels_min: 3_000,
        panel_side,
        panels: [
            recognition::ResultPanelPresenceEvidence {
                panel_side: recognition::ResultPanelSide::Left,
                upper_panel_edge_pixels: if known == Some(recognition::ResultPanelSide::Left) {
                    520
                } else {
                    0
                },
                lower_panel_edge_pixels: if known == Some(recognition::ResultPanelSide::Left) {
                    520
                } else {
                    0
                },
                qualifies: known == Some(recognition::ResultPanelSide::Left),
            },
            recognition::ResultPanelPresenceEvidence {
                panel_side: recognition::ResultPanelSide::Right,
                upper_panel_edge_pixels: if known == Some(recognition::ResultPanelSide::Right) {
                    520
                } else {
                    0
                },
                lower_panel_edge_pixels: if known == Some(recognition::ResultPanelSide::Right) {
                    520
                } else {
                    0
                },
                qualifies: known == Some(recognition::ResultPanelSide::Right),
            },
        ],
        horizontal_edge_pixels_min: 518,
    }
}

fn play_presence() -> recognition::PlayPresenceEvidence {
    recognition::PlayPresenceEvidence {
        qualifying_candidates: 0,
        top_edge_runs: 0,
        bottom_edge_runs: 0,
        candidates: [None, None],
        top_edge_pixels_min: 280,
        top_edge_pixels_max: 305,
        bottom_edge_pixels_min: 300,
        bottom_edge_pixels_max: 320,
        vertical_distance_min: 59,
        vertical_distance_max: 70,
        edge_center_delta_x2_max: 2,
        candidate_cluster_delta_x2_max: 4,
        candidate_cluster_delta_y_max: 12,
    }
}

fn resolved_two_player_result_observation() -> RegisteredScreenFieldObservation {
    let catalog = catalog_from_records(&[
        tachi_record("song-1", "SYNTHETIC SONG", "SYNTHETIC ARTIST"),
        tachi_record("song-2", "DISTANT RUNNER UP", "OTHER ARTIST"),
    ]);
    let domain = CatalogCandidateDomain::from_catalog(&catalog).unwrap();
    let numeric = |value: &str| DynamicTextObservation {
        input_width: 1,
        output_timesteps: 1,
        open_text: value.to_owned(),
        constrained_text: Some(value.to_owned()),
    };
    let observation = project_fields_with_catalog(
        &domain,
        &catalog,
        ScreenFieldObservations::Result(ResultScreenFieldObservations {
            panel_side: recognition::ResultPanelSide::Right,
            title: text("SYNTHETIC SONG"),
            artist: text("SYNTHETIC ARTIST"),
            clear_type: text("CLEAR"),
            difficulty: text("NORMAL"),
            play_type: text("SP"),
            level: numeric("1"),
            notes: numeric("1"),
            current_score: numeric("2"),
            previous_clear_type: text("NO PLAY"),
            previous_score: numeric("0"),
            previous_miss_count: numeric("0"),
            miss_count: numeric("0"),
            pgreat: numeric("1"),
            great: numeric("0"),
            good: numeric("0"),
            bad: numeric("0"),
            poor: numeric("0"),
            fast: numeric("0"),
            slow: numeric("0"),
            combo_break: numeric("0"),
            ..Default::default()
        }),
    );
    assert!(observation.result_chart_resolution().is_some());
    assert!(observation.result_performance_resolution().is_some());
    observation
}

fn publish_headless_live_event(
    routine: &mut crate::events::server::RoutineOutput,
    event: GamescopeLiveSessionEvent<'_>,
) {
    let value = live_session_event_value(Some("invocation-session-1"), Some(1), event).unwrap();
    routine
        .publish(&RunEvent::from_value(value).unwrap())
        .unwrap();
}

fn publish_two_player_result_episode(
    routine: &mut crate::events::server::RoutineOutput,
    observation: &RegisteredScreenFieldObservation,
) {
    for (screen_episode_id, sequence, screen, phase) in [
        (
            1,
            1,
            recognition::ScreenClass::MusicSelect,
            crate::capture_live::SemanticScreenEpisodePhase::Started,
        ),
        (
            1,
            2,
            recognition::ScreenClass::MusicSelect,
            crate::capture_live::SemanticScreenEpisodePhase::Finalized,
        ),
        (
            2,
            3,
            recognition::ScreenClass::Play,
            crate::capture_live::SemanticScreenEpisodePhase::Started,
        ),
        (
            2,
            4,
            recognition::ScreenClass::Play,
            crate::capture_live::SemanticScreenEpisodePhase::Finalized,
        ),
        (
            3,
            5,
            recognition::ScreenClass::Result,
            crate::capture_live::SemanticScreenEpisodePhase::Started,
        ),
    ] {
        publish_headless_live_event(
            routine,
            GamescopeLiveSessionEvent::SemanticScreenEpisode {
                screen_episode_id,
                sequence,
                monotonic_end_ms: sequence * 100,
                screen,
                phase,
            },
        );
    }
    for sequence in [6, 7] {
        publish_headless_live_event(
            routine,
            GamescopeLiveSessionEvent::RawScreenObserved {
                semantic_episode_id: Some(3),
                sequence,
                monotonic_start_ms: sequence * 100,
                monotonic_end_ms: sequence * 100 + 25,
                screen: recognition::ScreenClass::Result,
                result_presence: result_presence(recognition::ResultPanelSideState::Known(
                    recognition::ResultPanelSide::Right,
                )),
                play_presence: play_presence(),
            },
        );
    }
    for sequence in [8, 9] {
        publish_headless_live_event(
            routine,
            GamescopeLiveSessionEvent::Observation {
                screen_episode_id: 3,
                sequence,
                monotonic_start_ms: sequence * 100,
                monotonic_end_ms: sequence * 100 + 25,
                output: observation,
            },
        );
    }
}

#[test]
fn config_uses_top_level_sections_and_rejects_cross_backend_values() {
    let config: ConfigFile = toml::from_str(
        r#"
[capture]
backend = "pipewire"
node_name = "gamescope"
[crop]
left = 8
[scores]
enabled = true
[overlay]
obs = true
[recording]
enabled = true
memory_mib = 64
[catalog]
url = "https://example.invalid/catalog.zip"
"#,
    )
    .unwrap();
    validate_config_file(&config).unwrap();

    let partial_pipewire: ConfigFile = toml::from_str("[capture]\nbackend = 'pipewire'\n").unwrap();
    validate_config_file(&partial_pipewire).unwrap();

    let invalid: ConfigFile = toml::from_str(
        r#"
[capture]
backend = "vulkan-layer"
node_name = "must-not-be-inherited"
"#,
    )
    .unwrap();
    assert!(validate_config_file(&invalid).is_err());
    assert!(toml::from_str::<ConfigFile>("[run]\ncapture = 'pipewire'\n").is_err());
}

#[test]
fn config_wayland_editor_enables_the_wayland_overlay() {
    let config: ConfigFile =
        toml::from_str("[capture]\nbackend = 'vulkan-layer'\n[overlay]\nwayland_edit = true\n")
            .unwrap();
    let options = config_effective::merge_run_options(config, RunArgs::default()).unwrap();
    assert!(options.overlays.wayland);
    assert!(options.overlays.wayland_edit);
}

#[test]
fn environment_can_disable_configured_recording_and_its_memory_limit() {
    use std::process::Command;

    const CHILD: &str = "SCOREPEEK_RECORDING_ENV_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let config: ConfigFile = toml::from_str(
            "[capture]\nbackend = 'vulkan-layer'\n[recording]\nenabled = true\nmemory_mib = 64\n",
        )
        .unwrap();
        let options = config_effective::merge_run_options(config, RunArgs::default()).unwrap();
        assert!(!options.recording);
        return;
    }

    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "application::tests::environment_can_disable_configured_recording_and_its_memory_limit",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("SCOREPEEK_RECORDING_ENABLED", "false")
        .env_remove("SCOREPEEK_RECORDING_MEMORY_MIB")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn run_diagnostics_start_before_config_loading_and_keep_the_failure_stage() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("invalid.toml");
    let diagnostic_store = root.path().join("diagnostics");
    fs::write(&config, "[capture\n").unwrap();
    let run_id = "run-2-0-1";
    let mut diagnostics =
        crate::diagnostics::inspect::RunDiagnostics::start(&diagnostic_store, run_id);
    let sink = diagnostics.sink();
    let monitor = crate::platform::signal::SignalStopMonitor::start().unwrap();
    let error = load_run_options(&mut diagnostics, &monitor, &config, RunArgs::default())
        .err()
        .expect("invalid config must fail");
    assert!(error.starts_with("config file is invalid:"));
    drop(sink);
    drop(diagnostics);

    let stream =
        fs::read_to_string(diagnostic_store.join(run_id).join("diagnostics.ndjson")).unwrap();
    assert!(stream.contains("\"stage\":\"config_load\""));
    assert!(stream.contains("\"status\":\"error\""));
    assert!(!stream.contains("model_initialization"));
}

#[test]
fn config_json_rejects_non_utf8_paths_without_panicking() {
    use std::os::unix::ffi::OsStringExt as _;

    let path = PathBuf::from(OsString::from_vec(b"/tmp/scorepeek-\xff".to_vec()));
    let error = run_config_command(
        super::ConfigCommand::Path(super::FormatArgs {
            format: super::OutputFormat::Json,
        }),
        &path,
    )
    .unwrap_err();
    assert_eq!(error, "config path must be UTF-8 for JSON output");
}

#[test]
fn public_run_interrupt_prioritizes_cancel_over_startup_success_and_failure() {
    use std::process::Command;
    use std::time::Duration;

    const CHILD_MODE: &str = "SCOREPEEK_STARTUP_INTERRUPT_TEST_MODE";
    if let Some(mode) = std::env::var_os(CHILD_MODE) {
        let args = RunArgs {
            capture: Some(super::CaptureKind::VulkanLayer),
            no_scores: true,
            ..RunArgs::default()
        };
        let result = super::run_public_with_model_initializer(args, None, |_| {
            signal_hook::low_level::raise(signal_hook::consts::SIGINT).unwrap();
            std::thread::sleep(Duration::from_millis(25));
            if mode == "success" {
                Ok(PathBuf::from("/unused-after-interrupt"))
            } else {
                Err("synthetic concurrent model failure".to_owned())
            }
        });
        assert_eq!(result.as_ref().unwrap_err(), super::INTERRUPTED_ERROR);
        assert_eq!(super::result_exit_status(&result), 130);
        return;
    }

    let root = tempfile::tempdir().unwrap();
    for mode in ["success", "failure"] {
        let state = root.path().join(mode).join("state");
        let runtime = root.path().join(mode).join("runtime");
        fs::create_dir_all(&runtime).unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "service::dispatch::application::tests::public_run_interrupt_prioritizes_cancel_over_startup_success_and_failure",
                    "--nocapture",
                ])
                .env(CHILD_MODE, mode)
                .env("XDG_STATE_HOME", &state)
                .env("XDG_RUNTIME_DIR", &runtime)
                .env("XDG_CONFIG_HOME", root.path().join(mode).join("config"))
                .env("HOME", root.path().join(mode).join("home"))
                .env_remove("SCOREPEEK_CONFIG")
                .status()
                .unwrap();
        assert!(status.success());
        let store = state.join("scorepeek/diagnostics");
        let run = fs::read_dir(&store)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.is_dir())
            .unwrap();
        let stream = fs::read_to_string(run.join("diagnostics.ndjson")).unwrap();
        assert!(stream.contains("\"stage\":\"interrupt\""));
        assert!(stream.contains("\"status\":\"cancel\""));
        assert_eq!(stream.matches("diagnostic_run_finished").count(), 1);
        assert!(!stream.contains("\"stage\":\"catalog_paths\""));
    }
}

#[test]
fn output_owned_diagnostics_prioritize_interrupt_over_startup_failure() {
    use std::process::Command;
    use std::time::Duration;

    const CHILD_ROOT: &str = "SCOREPEEK_OUTPUT_INTERRUPT_TEST_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let diagnostics = crate::diagnostics::inspect::RunDiagnostics::start(
            Path::new(&root),
            "run-output-interrupt-test",
        );
        let mut output = crate::events::server::RoutineOutput::start_headless_with_diagnostics(
            "invocation-output-interrupt".to_owned(),
            "0".repeat(64),
            diagnostics,
        );
        let monitor = crate::platform::signal::SignalStopMonitor::start().unwrap();
        signal_hook::low_level::raise(signal_hook::consts::SIGINT).unwrap();
        std::thread::sleep(Duration::from_millis(25));
        let result = super::settle_output_startup_result::<()>(
            &mut output,
            &monitor,
            Err("synthetic output startup failure".to_owned()),
        );
        assert_eq!(result.as_ref().unwrap_err(), super::INTERRUPTED_ERROR);
        assert_eq!(super::result_exit_status(&result), 130);
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "service::dispatch::application::tests::output_owned_diagnostics_prioritize_interrupt_over_startup_failure",
                "--nocapture",
            ])
            .env(CHILD_ROOT, root.path())
            .status()
            .unwrap();
    assert!(status.success());
    let stream = fs::read_to_string(
        root.path()
            .join("run-output-interrupt-test")
            .join("diagnostics.ndjson"),
    )
    .unwrap();
    assert!(stream.contains("\"stage\":\"interrupt\""));
    assert!(stream.contains("\"status\":\"cancel\""));
    assert_eq!(stream.matches("diagnostic_run_finished").count(), 1);
}

#[test]
fn capture_diagnostic_events_use_the_stage_timing_schema() {
    let fact = CaptureDiagnosticFact {
        sequence: 1,
        monotonic_start_ms: 2,
        monotonic_end_ms: 3,
        operation: CaptureDiagnosticOperation::ProfileBindingAdmission,
        status: CaptureDiagnosticStatus::Success,
        error_type: None,
        detail: CaptureDiagnosticDetail::ProfileBindingAdmission,
    };
    let value = live_session_event_value(
        Some("session-1"),
        Some(1),
        GamescopeLiveSessionEvent::CaptureDiagnostic { fact: &fact },
    )
    .unwrap();
    assert_eq!(value["schema"], "scorepeek-capture-diagnostic-v2");
    assert_eq!(value["event"], "capture_diagnostic");
}

#[test]
fn failed_startup_stage_is_saved_in_a_zero_session_run() {
    let root = tempfile::tempdir().unwrap();
    let run_id = "run-1-0-1";
    let diagnostics = crate::diagnostics::inspect::RunDiagnostics::start(root.path(), run_id);
    let sink = diagnostics.sink();
    let error = run_startup_stage::<()>(&sink, "synthetic_preflight", || {
        Err("synthetic failure".to_owned())
    })
    .unwrap_err();
    assert_eq!(error, "synthetic failure");
    drop(sink);
    drop(diagnostics);

    let stream = fs::read_to_string(root.path().join(run_id).join("diagnostics.ndjson")).unwrap();
    assert!(stream.contains("synthetic_preflight"));
    assert!(stream.contains("synthetic failure"));
    assert!(stream.contains("diagnostic_run_finished"));
    assert!(!stream.contains("session_started"));
}

#[test]
fn failed_model_initialization_is_saved_in_a_zero_session_run() {
    let root = tempfile::tempdir().unwrap();
    let run_id = "run-1-0-2";
    let diagnostics = crate::diagnostics::inspect::RunDiagnostics::start(root.path(), run_id);
    let sink = diagnostics.sink();
    let error =
        initialize_routine_model(&sink, None, |_| Err("synthetic model failure".to_owned()))
            .unwrap_err();
    assert_eq!(error, "synthetic model failure");
    drop(sink);
    drop(diagnostics);

    let stream = fs::read_to_string(root.path().join(run_id).join("diagnostics.ndjson")).unwrap();
    assert!(stream.contains("model_initialization"));
    assert!(stream.contains("synthetic model failure"));
    assert!(stream.contains("diagnostic_run_finished"));
    assert!(!stream.contains("session_started"));
}

#[test]
fn scores_options_are_independent_of_recording_and_reject_conflicts() {
    let options = ["--capture", "vulkan-layer", "--scores-db", "guest.sqlite3"].map(OsString::from);
    let parsed = parse_routine_run_options(&options).unwrap();
    assert_eq!(parsed.scores_db, Some(PathBuf::from("guest.sqlite3")));
    assert!(!parsed.recording);
    assert!(!parsed.no_scores);
    let base = ["--capture", "vulkan-layer"].map(OsString::from);
    assert!(!parse_routine_run_options(&base).unwrap().no_scores);
    assert!(
        parse_routine_run_options(
            &["--capture", "vulkan-layer", "--no-scores"].map(OsString::from)
        )
        .unwrap()
        .no_scores
    );
    for options in [
        vec!["--scores-db"],
        vec!["--scores-db", ""],
        vec!["--no-scores", "--scores-db", "guest.db"],
        vec!["--no-scores", "--no-scores"],
    ] {
        let options = ["--capture", "vulkan-layer"]
            .into_iter()
            .chain(options)
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert!(parse_routine_run_options(&options).is_err());
    }
}

#[test]
fn unrecorded_run_disables_the_recognition_artifact_root() {
    let root = Path::new("/tmp/recognition");
    assert_eq!(optional_recognition_root(false, root), None);
    assert_eq!(optional_recognition_root(true, root), Some(root));
}

#[test]
fn overlay_options_allow_independent_and_simultaneous_consumers() {
    for options in [
        vec!["--overlay-wayland"],
        vec!["--overlay-wayland-edit"],
        vec!["--overlay-obs"],
        vec![
            "--overlay-wayland",
            "--overlay-obs",
            "--overlay-config",
            "overlay.toml",
            "--no-scores",
        ],
    ] {
        let values = ["--capture", "vulkan-layer"]
            .into_iter()
            .chain(options)
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert!(parse_routine_run_options(&values).is_ok());
    }
    for options in [
        vec!["--overlay-wayland", "--overlay-wayland"],
        vec!["--overlay-obs", "--overlay-obs"],
        vec!["--overlay-config"],
        vec!["--overlay-config", "a", "--overlay-config", "b"],
    ] {
        let values = ["--capture", "vulkan-layer"]
            .into_iter()
            .chain(options)
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert!(parse_routine_run_options(&values).is_err());
    }
    let base = ["--capture", "vulkan-layer"].map(OsString::from);
    let parsed = parse_routine_run_options(&base).unwrap();
    assert!(!parsed.overlays.wayland && !parsed.overlays.obs);
    let edit_args = ["--capture", "vulkan-layer", "--overlay-wayland-edit"].map(OsString::from);
    let edit = parse_routine_run_options(&edit_args).unwrap();
    assert!(edit.overlays.wayland && edit.overlays.wayland_edit);
}

#[test]
fn overlay_config_path_is_preserved() {
    let args = [
        "--capture",
        "vulkan-layer",
        "--overlay-obs",
        "--overlay-config",
        "custom.toml",
    ]
    .map(OsString::from);
    let parsed = parse_routine_run_options(&args).unwrap();
    assert_eq!(
        parsed.overlays.config_path.as_deref(),
        Some(Path::new("custom.toml"))
    );
}

#[test]
fn ordinary_run_options_are_order_independent_and_record_is_opt_in() {
    assert!(parse_routine_run_options(&[]).is_err());
    let base = ["--capture", "vulkan-layer"].map(OsString::from);
    let parsed = parse_routine_run_options(&base).unwrap();
    assert!(!parsed.recording);
    let record = ["--capture", "vulkan-layer", "--record"].map(OsString::from);
    let parsed = parse_routine_run_options(&record).unwrap();
    assert!(parsed.recording);
    assert_eq!(parsed.recording_retention, RecordingRetention::Selective);
    assert_eq!(
        parsed.recording_memory_limit.bytes(),
        1024_u64 * 1024 * 1024
    );
    let configured = [
        OsString::from("--capture"),
        OsString::from("vulkan-layer"),
        OsString::from("--record-memory-mib"),
        OsString::from("2048"),
        OsString::from("--record"),
    ];
    assert_eq!(
        parse_routine_run_options(&configured)
            .unwrap()
            .recording_memory_limit
            .bytes(),
        2048_u64 * 1024 * 1024
    );
    let record_all = ["--capture", "vulkan-layer", "--record-all"].map(OsString::from);
    let parsed = parse_routine_run_options(&record_all).unwrap();
    assert!(parsed.recording);
    assert_eq!(parsed.recording_retention, RecordingRetention::All);
}

#[test]
fn removed_or_duplicate_recording_options_are_rejected() {
    for options in [
        vec![OsString::from("--no-recording")],
        vec![OsString::from("--record-attempts")],
        vec![OsString::from("--record"), OsString::from("--record")],
        vec![OsString::from("--record"), OsString::from("--record-all")],
        vec![
            OsString::from("--record-all"),
            OsString::from("--record-all"),
        ],
        vec![
            OsString::from("--record-memory-mib"),
            OsString::from("1024"),
        ],
    ] {
        assert!(parse_routine_run_options(&options).is_err());
    }
}

#[test]
fn help_version_and_doctor_skip_model_initialization() {
    for args in [["--help"], ["--version"], ["doctor"]] {
        let initialized = Cell::new(false);
        run_with_model_initializer(&args.map(OsString::from), |_| {
            initialized.set(true);
            Err("must not initialize".to_owned())
        })
        .unwrap();
        assert!(!initialized.get());
    }
}

#[test]
fn every_other_command_initializes_before_dispatch() {
    let initialized = Cell::new(false);
    let error = run_with_model_initializer(&[OsString::from("unknown")], |_| {
        initialized.set(true);
        Ok(PathBuf::from("/unused-model-bundle"))
    })
    .unwrap_err();
    assert!(initialized.get());
    assert_eq!(error, "usage: scorepeek --help");
}

#[test]
fn short_information_aliases_initialize_before_dispatch() {
    for flag in ["-h", "-V"] {
        let initialized = Cell::new(false);
        run_with_model_initializer(&[OsString::from(flag)], |_| {
            initialized.set(true);
            Ok(PathBuf::from("/unused-model-bundle"))
        })
        .unwrap();
        assert!(initialized.get());
    }
}

#[test]
fn global_model_bundle_is_forwarded_to_initialization() {
    let selected = Cell::new(false);
    let args = [
        OsString::from("--model-bundle"),
        OsString::from("/development/small"),
        OsString::from("unknown"),
    ];
    let _ = run_with_model_initializer(&args, |bundle| {
        selected.set(bundle == Some(Path::new("/development/small")));
        Ok(PathBuf::from("/development/small"))
    });
    assert!(selected.get());
}

#[test]
fn private_file_publication_is_no_clobber_and_cleans_every_failed_checkpoint() {
    for failed_point in [
        PrivatePublicationPoint::FileSynced,
        PrivatePublicationPoint::Linked,
        PrivatePublicationPoint::StagingRemoved,
    ] {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("artifact.json");
        let error = publish_private_file_with(&output, b"complete\n", |point| {
            if point == failed_point {
                Err(std::io::Error::other("checkpoint failure"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert!(!output.exists());
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }

    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("artifact.json");
    publish_private_file(&output, b"first\n").unwrap();
    assert_eq!(fs::read(&output).unwrap(), b"first\n");
    assert!(publish_private_file(&output, b"second\n").is_err());
    assert_eq!(fs::read(&output).unwrap(), b"first\n");
}

#[test]
fn catalog_paths_use_absolute_xdg_directories() {
    let paths = catalog_paths(Some(OsStr::new("/data")), Some(OsStr::new("/cache")), None).unwrap();
    assert_eq!(paths.0, PathBuf::from("/data/scorepeek/catalog"));
    assert_eq!(paths.1, PathBuf::from("/cache/scorepeek/catalog/sources"));
}

#[test]
fn catalog_paths_fall_back_to_home_and_reject_relative_values() {
    let paths = catalog_paths(None, None, Some(OsStr::new("/home/test"))).unwrap();
    assert_eq!(
        paths.0,
        PathBuf::from("/home/test/.local/share/scorepeek/catalog")
    );
    assert_eq!(
        paths.1,
        PathBuf::from("/home/test/.cache/scorepeek/catalog/sources")
    );
    assert!(
        catalog_paths(
            Some(OsStr::new("relative")),
            Some(OsStr::new("/cache")),
            None,
        )
        .is_err()
    );
}

#[test]
fn live_session_command_requires_the_exact_ordered_contract() {
    let mut args = vec!["run".into(), "gamescope".into()];
    for (index, flag) in LIVE_SESSION_FLAGS.iter().enumerate() {
        args.push((*flag).into());
        args.push(format!("value-{index}").into());
    }
    let values = command_flag_values(&args, "run", "gamescope", LIVE_SESSION_FLAGS).unwrap();
    assert_eq!(values.len(), LIVE_SESSION_FLAGS.len());
    assert_eq!(values[0], OsStr::new("value-0"));

    args[0] = "capture".into();
    assert!(command_flag_values(&args, "run", "gamescope", LIVE_SESSION_FLAGS).is_none());
    args[0] = "run".into();
    args.pop();
    assert!(command_flag_values(&args, "run", "gamescope", LIVE_SESSION_FLAGS).is_none());
}

#[test]
fn runtime_gate_contracts_have_no_launch_metadata_arguments() {
    for flags in [
        LIVE_SESSION_FLAGS,
        CAPTURE_HANDOFF_FLAGS,
        CAPTURE_FIELD_OBSERVATION_FLAGS,
        CAPTURE_RESULT_RECOGNITION_FLAGS,
    ] {
        for removed in [
            "--environment-id",
            "--gamescope-version",
            "--backend",
            "--output-width",
            "--output-height",
            "--nested-width",
            "--nested-height",
            "--nested-refresh",
            "--scaler",
            "--filter",
        ] {
            assert!(!flags.contains(&removed));
        }
    }

    let mut args = vec!["capture".into(), "gamescope-field-observation-gate".into()];
    for (index, flag) in CAPTURE_FIELD_OBSERVATION_FLAGS.iter().enumerate() {
        args.push((*flag).into());
        args.push(format!("value-{index}").into());
    }
    assert!(
        command_flag_values(
            &args,
            "capture",
            "gamescope-field-observation-gate",
            CAPTURE_FIELD_OBSERVATION_FLAGS,
        )
        .is_some()
    );
}

#[test]
fn live_session_prepares_an_absent_private_diagnostic_root() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("diagnostics");
    let preflight = prepare_live_diagnostic_root(&root, &DiagnosticPolicy::default());
    assert_eq!(preflight.status, "ready");
    assert_eq!(preflight.error_type, None);
    assert!(root.is_dir());
}

#[test]
fn internal_capture_cli_never_enables_runtime_frame_artifacts() {
    let policy = parse_diagnostic_recording_policy(OsStr::new("enabled")).unwrap();
    assert!(policy.enabled);
    assert_eq!(policy.retention, DiagnosticRetention::FactsOnly);
}

#[test]
fn removed_gamescope_capture_commands_are_not_dispatched() {
    for args in [
        vec!["run", "gamescope"],
        vec!["capture", "gamescope-live-gate", "--duration-ms", "100"],
        vec![
            "capture",
            "gamescope-binding-admission-gate",
            "--binding",
            "/tmp/ignored",
            "--binding-sha256",
            "0",
        ],
    ] {
        let args = args.into_iter().map(OsString::from).collect::<Vec<_>>();
        assert!(run_command(&args, Path::new("/tmp/unused-model-bundle")).is_err());
    }
}

#[test]
fn capture_terminal_failure_is_fatal_but_source_endings_are_readmitted() {
    assert_eq!(
        routine_session_disposition(Some(
            crate::capture_live::LiveSessionStopReason::SourceEnded
        )),
        ("source_ended", true, false)
    );
    assert_eq!(
        routine_session_disposition(Some(
            crate::capture_live::LiveSessionStopReason::TerminalFailure
        )),
        ("error", false, true)
    );
    assert_eq!(
        routine_session_disposition(Some(
            crate::capture_live::LiveSessionStopReason::SourceContractChanged
        )),
        ("source_ended", true, false)
    );
}

#[test]
fn only_source_disappearance_is_retried_during_admission() {
    assert!(transient_admission_capture_error(
        scorepeek::capture::CaptureErrorType::SourceLost
    ));
    assert!(transient_admission_capture_error(
        scorepeek::capture::CaptureErrorType::StreamLost
    ));
    assert!(!transient_admission_capture_error(
        scorepeek::capture::CaptureErrorType::UnsupportedFormat
    ));
    assert!(!transient_admission_capture_error(
        scorepeek::capture::CaptureErrorType::FrameNormalizationFailed
    ));
}

#[test]
fn live_serializer_and_reducer_keep_one_recording_schema() {
    use crate::events::server::RoutineOutput;
    let mut output = RoutineOutput::start_headless("invocation".into(), "a".repeat(64));
    for event in [
        GamescopeLiveSessionEvent::Started {
            capture_generation: 1,
            capture_profile_sha256: "profile",
            normalizer_artifact_sha256: "normalizer",
            capture_profile_document: None,
            normalizer_document: None,
        },
        GamescopeLiveSessionEvent::SemanticScreenEpisode {
            screen_episode_id: 1,
            sequence: 1,
            monotonic_end_ms: 100,
            screen: recognition::ScreenClass::MusicSelect,
            phase: crate::capture_live::SemanticScreenEpisodePhase::Started,
        },
    ] {
        let value = live_session_event_value(Some("invocation-session-1"), Some(1), event).unwrap();
        output
            .publish(&RunEvent::from_value(value).unwrap())
            .unwrap();
    }
    let events = output.take_headless_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, RunEventKind::MusicSelectResolverChanged { .. }))
    );
    let schemas: std::collections::BTreeSet<_> =
        events.iter().map(|event| event.schema.as_str()).collect();
    assert_eq!(
        schemas.len(),
        1,
        "the corpus reader rejects mixed-schema sessions"
    );
    assert_eq!(schemas.first().copied(), Some("scorepeek-run-event-v17"));
}

#[test]
fn routine_screen_events_separate_raw_observation_and_semantic_episode() {
    let value = live_session_event_value(
        Some("invocation-session-2"),
        Some(2),
        GamescopeLiveSessionEvent::RawScreenObserved {
            semantic_episode_id: Some(1),
            sequence: 41,
            monotonic_start_ms: 100,
            monotonic_end_ms: 125,
            screen: recognition::ScreenClass::Unknown,
            result_presence: result_presence(recognition::ResultPanelSideState::Unknown(
                recognition::ResultPanelSideUnknownReason::NoCandidate,
            )),
            play_presence: play_presence(),
        },
    )
    .unwrap();
    assert_eq!(value["schema"], "scorepeek-run-event-v17");
    assert_eq!(value["event"], "raw_screen_observed");
    assert_eq!(value["semantic_episode_id"], 1);
    assert_eq!(value["session_id"], "invocation-session-2");
    assert_eq!(value["capture_generation"], 2);
    assert_eq!(value["sequence"], 41);
    assert_eq!(value["screen"], "unknown");
    assert_eq!(value["result_presence"]["warm_pixels"], 2_900);
    assert_eq!(value["play_presence"]["qualifying_candidates"], 0);
    assert_eq!(
        value["result_presence"]["panel_side"]["value"],
        "no_candidate"
    );

    let mode = live_session_event_value(
        Some("invocation-session-2"),
        Some(2),
        GamescopeLiveSessionEvent::SemanticScreenEpisode {
            screen_episode_id: 1,
            sequence: 42,
            monotonic_end_ms: 150,
            screen: recognition::ScreenClass::ModeSelect,
            phase: crate::capture_live::SemanticScreenEpisodePhase::Started,
        },
    )
    .unwrap();
    assert_eq!(mode["screen"], "mode_select");
    assert_eq!(mode["phase"], "started");
}

#[test]
fn live_result_output_retains_exact_ocr_and_typed_resolution() {
    let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
    let output = project_fields(
        &domain,
        ScreenFieldObservations::Result(ResultScreenFieldObservations {
            panel_side: recognition::ResultPanelSide::Right,
            title: text("TITLE EXACT"),
            artist: text("ARTIST EXACT"),
            clear_type: text("FAILED"),
            difficulty: text("HYPER"),
            play_type: text("SP"),
            level: text("8"),
            notes: text("800"),
            current_score: text("1200"),
            ..Default::default()
        }),
    );
    let value = live_session_event_value(
        Some("invocation-session-1"),
        Some(1),
        GamescopeLiveSessionEvent::Observation {
            screen_episode_id: 0,
            sequence: 42,
            monotonic_start_ms: 100,
            monotonic_end_ms: 125,
            output: &output,
        },
    )
    .unwrap();
    assert_eq!(value["event"], "field_observation");
    assert_eq!(value["sequence"], 42);
    assert_eq!(value["fields"]["panel_side"], "right");
    assert_eq!(value["fields"]["title"], "TITLE EXACT");
    assert_eq!(value["fields"]["artist"], "ARTIST EXACT");
    assert_eq!(value["fields"]["clear_type"], "FAILED");
    assert_eq!(value["fields"]["play_type"], "SP");
    assert_eq!(
        value["result_song_resolution"]["reason"],
        "no_catalog_candidates"
    );
    let event = RunEvent::from_value(value).unwrap();
    let RunEventKind::FieldObservation { fields, .. } = event.kind else {
        panic!("live result observation changed event kind");
    };
    assert_eq!(fields["panel_side"], "right");
}

#[test]
fn production_result_serializer_reaches_provisional_and_confirmed_output() {
    use crate::events::server::RoutineOutput;
    use scorepeek_core::event::ResultState;

    let observation = resolved_two_player_result_observation();
    let mut routine = RoutineOutput::start_headless("invocation".into(), "a".repeat(64));
    publish_two_player_result_episode(&mut routine, &observation);
    assert!(routine.take_headless_events().iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Provisional { ref result, .. },
            ..
        } if result.play_side
            == scorepeek_core::recognition::music_select::PlaySide::TwoPlayer
    )));

    publish_headless_live_event(
        &mut routine,
        GamescopeLiveSessionEvent::SemanticScreenEpisode {
            screen_episode_id: 3,
            sequence: 10,
            monotonic_end_ms: 1_000,
            screen: recognition::ScreenClass::Result,
            phase: crate::capture_live::SemanticScreenEpisodePhase::Finalized,
        },
    );
    assert!(routine.take_headless_events().iter().any(|event| matches!(
        event.kind,
        RunEventKind::ResultChanged {
            state: ResultState::Confirmed { ref result, .. },
            ..
        } if result.play_side
            == scorepeek_core::recognition::music_select::PlaySide::TwoPlayer
    )));
}

#[test]
fn routine_observation_binds_session_and_generation() {
    let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
    let output = project_fields(
        &domain,
        ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text("TITLE"),
            artist: text("ARTIST"),
            clear_type: text("CLEAR"),
            difficulty: text("HYPER"),
            level: text("8"),
            notes: text("800"),
            current_score: text("1200"),
            ..Default::default()
        }),
    );
    let value = live_session_event_value(
        Some("invocation-session-2"),
        Some(2),
        GamescopeLiveSessionEvent::Observation {
            screen_episode_id: 0,
            sequence: 1,
            monotonic_start_ms: 10,
            monotonic_end_ms: 20,
            output: &output,
        },
    )
    .unwrap();
    assert_eq!(value["schema"], "scorepeek-run-event-v17");
    assert_eq!(value["session_id"], "invocation-session-2");
    assert_eq!(value["capture_generation"], 2);
    assert_eq!(value["sequence"], 1);
}

#[test]
fn routine_live_emission_bounds_json_without_truncating_authority() {
    let records = (0..9)
        .map(|index| {
            tachi_record(
                &format!("song-{index}"),
                &format!("COMMON TITLE {index}"),
                &format!("COMMON ARTIST {index}"),
            )
        })
        .collect::<Vec<_>>();
    let catalog = catalog_from_records(&records);
    let domain = CatalogCandidateDomain::from_catalog(&catalog).unwrap();
    let output = project_fields_with_catalog(
        &domain,
        &catalog,
        ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text("COMMON TITLE"),
            artist: text("COMMON ARTIST"),
            ..Default::default()
        }),
    );
    let authority = output.joint_evidence().clone();
    assert!(authority.candidates.len() > 8);
    let value = live_session_event_value(
        Some("invocation-session-2"),
        Some(2),
        GamescopeLiveSessionEvent::Observation {
            screen_episode_id: 7,
            sequence: 8,
            monotonic_start_ms: 10,
            monotonic_end_ms: 20,
            output: &output,
        },
    )
    .unwrap();
    assert_eq!(
        value["joint_evidence"]["candidates"]
            .as_array()
            .unwrap()
            .len(),
        8
    );

    let event = run_event_from_live_emission(LiveSessionEmission {
        public_binding: None,
        value,
        authority_joint_evidence: Some(authority.clone()),
        diagnostic_identity: None,
        diagnostic_capture_fact: None,
    })
    .unwrap();
    let RunEventKind::FieldObservation { joint_evidence, .. } = event.kind else {
        panic!("expected field observation");
    };
    assert_eq!(joint_evidence, authority);
}

#[test]
fn accepted_resolution_includes_catalog_title_artist_and_evidence() {
    let catalog = catalog_from_records(&[
        tachi_record("song-1", "CATALOG TITLE", "CATALOG ARTIST"),
        tachi_record("song-2", "OTHER SONG", "OTHER ARTIST"),
    ]);
    let domain = CatalogCandidateDomain::from_catalog(&catalog).unwrap();
    let output = project_fields(
        &domain,
        ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text("CATALOG TITLE"),
            artist: text("CATALOG ARTIST"),
            clear_type: text("CLEAR"),
            difficulty: text("HYPER"),
            level: text("8"),
            notes: text("800"),
            current_score: text("1200"),
            ..Default::default()
        }),
    );
    let value = live_session_event_value(
        Some("invocation-session-1"),
        Some(1),
        GamescopeLiveSessionEvent::Observation {
            screen_episode_id: 0,
            sequence: 1,
            monotonic_start_ms: 10,
            monotonic_end_ms: 20,
            output: &output,
        },
    )
    .unwrap();
    let presentation = &value["song_resolution_presentation"];
    assert_eq!(presentation["status"], "accepted");
    assert_eq!(
        presentation["selected"]["display_titles"][0],
        "CATALOG TITLE"
    );
    assert_eq!(presentation["selected"]["artist"], "CATALOG ARTIST");
    assert!(presentation["selected"]["scorepeek_song_id"].is_string());
    assert!(
        presentation["evidence_summary"]
            .as_str()
            .unwrap()
            .contains("runner-up margin=")
    );
}

fn project_fields(
    domain: &CatalogCandidateDomain,
    fields: ScreenFieldObservations,
) -> RegisteredScreenFieldObservation {
    project_fields_with_catalog(domain, &Catalog::default(), fields)
}

fn project_fields_with_catalog(
    domain: &CatalogCandidateDomain,
    catalog: &Catalog,
    fields: ScreenFieldObservations,
) -> RegisteredScreenFieldObservation {
    let projected = scorepeek_core::model::session::ProjectedScreenFieldObservation::project(
        domain, catalog, fields, None,
    );
    let timing = scorepeek_core::model::session::RecognitionProcessingTiming::unmeasured(
        projected.catalog_evidence_us(),
    );
    projected.complete(None, timing)
}

fn catalog_from_records(records: &[SourceObservation]) -> Catalog {
    let policy = SourcePolicy::tachi();
    let mut field_authority = policy
        .field_authority
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    field_authority.sort();
    let snapshot = SourceSnapshot {
        policy: policy.clone(),
        evidence: SourceEvidence {
            source_id: SourceId::Tachi,
            lineage_id: LineageId::GameMdb,
            revision_strategy: RevisionStrategy::GitCommit,
            revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            content_sha256: "a".repeat(64),
            byte_size: records.len(),
            record_count: records.len(),
            parser_version: policy.parser_version.to_owned(),
            declared_scope: policy.declared_scope.to_owned(),
            completeness: policy.completeness,
            field_authority,
            freshness: policy.freshness.to_owned(),
            rights_and_provenance: policy.rights_and_provenance.to_owned(),
        },
        observations: records.to_vec(),
    };
    Catalog::default()
        .federate(FederationInput {
            tachi: Some(snapshot),
            ..FederationInput::default()
        })
        .catalog
}

fn tachi_record(id: &str, title: &str, artist: &str) -> SourceObservation {
    SourceObservation::Tachi(TachiObservation {
        source_song_id: id.to_owned(),
        title_variants: BTreeSet::from([SourceTitleObservation {
            value: title.to_owned(),
            kind: DisplayVariantKind::InGameDisplay,
        }]),
        artist: artist.to_owned(),
        version: "SYNTHETIC".to_owned(),
        charts: vec![SourceChartObservation {
            chart: Chart {
                key: ChartKey {
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Normal,
                },
                level: 1,
                notes: 1,
            },
            source_chart_id: "spn".to_owned(),
            product_versions: BTreeSet::from(["synthetic-v1".to_owned()]),
            primary: true,
        }],
        primary_infinitas: true,
    })
}

fn text(value: &str) -> DynamicTextObservation {
    DynamicTextObservation {
        input_width: 1,
        output_timesteps: 1,
        open_text: value.to_owned(),
        constrained_text: None,
    }
}
