mod calibration_marker;
mod canonical_recording;
mod canonical_source;
mod capture_calibration;
mod capture_live;
pub mod diagnostic_control;
pub mod diagnostic_live;
pub mod diagnostic_recording;
mod diagnostic_reevaluation;
pub mod diagnostic_replay;
pub mod diagnostic_worker;
mod inventory;
mod live_control;
mod local_profiles;
mod play_attempt;
mod recognition_artifact;
pub mod recognition_live;
mod recording_simulation;
mod routine_output;
mod routine_watcher;
mod run_event_artifact;
mod session_artifact;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, BufWriter, Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use scorepeek::catalog::CatalogStore;
use scorepeek::catalog::{CatalogSync, CatalogSyncError};
use scorepeek::recognition::{
    self, CanonicalFrame, DIAGNOSTIC_TITLE_COMPARISON_KEY_ID, DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

#[allow(clippy::too_many_lines)]
fn run(args: &[OsString]) -> Result<(), String> {
    run_with_model_initializer(args, |override_bundle| {
        scorepeek::model_cache::ensure_small_model(override_bundle, |event| match event {
            scorepeek::model_cache::ModelCacheEvent::DownloadStarted => {
                eprintln!("scorepeek: downloading PP-OCRv6-small model...");
            }
            scorepeek::model_cache::ModelCacheEvent::DownloadCompleted => {
                eprintln!("scorepeek: PP-OCRv6-small model download complete");
            }
        })
        .map_err(|error| format!("scorepeek model initialization failed: {error}"))
    })
}

fn run_with_model_initializer(
    args: &[OsString],
    initialize: impl FnOnce(Option<&Path>) -> Result<PathBuf, String>,
) -> Result<(), String> {
    if let Some(result) = try_offline_program_information(args)
        .or_else(|| try_numeric_model_install(args))
        .or_else(|| try_doctor(args))
    {
        return result;
    }
    let (override_bundle, args) = parse_global_model_bundle(args)?;
    let bundle = initialize(override_bundle)?;
    run_command(args, &bundle)
}

fn parse_global_model_bundle(args: &[OsString]) -> Result<(Option<&Path>, &[OsString]), String> {
    match args {
        [flag, bundle, rest @ ..] if flag == "--model-bundle" => {
            if rest.is_empty() {
                return Err("--model-bundle requires a command".to_owned());
            }
            Ok((Some(Path::new(bundle)), rest))
        }
        _ => Ok((None, args)),
    }
}

#[allow(clippy::too_many_lines)]
fn run_command(args: &[OsString], bundle: &Path) -> Result<(), String> {
    if let Some(result) = local_profiles::try_command(args, bundle)
        .or_else(|| try_diagnostic_control(args))
        .or_else(|| try_diagnostic_reevaluation(args, bundle))
        .or_else(|| try_diagnostic_replay(args))
        .or_else(|| try_recording_simulation(args, bundle))
        .or_else(|| try_routine_live_session(args, bundle))
        .or_else(|| try_live_session(args, bundle))
        .or_else(|| try_capture_commands(args, bundle))
        .or_else(|| try_provisional_title_candidates(args))
        .or_else(|| try_integrated_context_crop(args))
        .or_else(|| try_integrated_context_observe(args, bundle))
        .or_else(|| try_registered_resource_gate(args, bundle))
        .or_else(|| try_dynamic_official_onnx_decode(args))
        .or_else(|| try_official_onnx_decode(args))
        .or_else(|| try_title_model_contract_parity(args))
        .or_else(|| try_title_onnx_parity(args))
        .or_else(|| try_title_dictionary_audit(args))
        .or_else(|| try_title_model_export_requirements(args))
        .or_else(|| try_program_information(args))
    {
        return result;
    }
    match args {
        [catalog, sync] if catalog == "catalog" && sync == "sync" => sync_catalog(),
        [
            recognition,
            inspect,
            extraction_flag,
            extraction,
            digest_flag,
            digest,
            frame_flag,
            frame_id,
        ] if recognition == "recognition"
            && inspect == "inspect"
            && extraction_flag == "--extraction"
            && digest_flag == "--extraction-sha256"
            && frame_flag == "--frame-id" =>
        {
            inspect_canonical_frame(extraction, digest, frame_id)
        }
        [recognition, inspect, frame_flag, frame, digest_flag, digest]
            if recognition == "recognition"
                && inspect == "inspect-diagnostic-qoi"
                && frame_flag == "--frame"
                && digest_flag == "--frame-sha256" =>
        {
            inspect_diagnostic_qoi(frame, digest)
        }
        [
            recognition,
            crop,
            extraction_flag,
            extraction,
            digest_flag,
            digest,
            frame_flag,
            frame_id,
            output_flag,
            output,
        ] if recognition == "recognition"
            && crop == "crop"
            && extraction_flag == "--extraction"
            && digest_flag == "--extraction-sha256"
            && frame_flag == "--frame-id"
            && output_flag == "--output" =>
        {
            crop_canonical_result(extraction, digest, frame_id, output)
        }
        [
            recognition,
            crop,
            extraction_flag,
            extraction,
            digest_flag,
            digest,
            frame_flag,
            frame_id,
            output_flag,
            output,
        ] if recognition == "recognition"
            && crop == "music-select-crop"
            && extraction_flag == "--extraction"
            && digest_flag == "--extraction-sha256"
            && frame_flag == "--frame-id"
            && output_flag == "--output" =>
        {
            crop_canonical_music_select(extraction, digest, frame_id, output)
        }
        [
            recognition,
            title_spike,
            store_flag,
            store,
            text_flag,
            text,
            confidence_flag,
            confidence,
        ] if recognition == "recognition"
            && title_spike == "title-spike"
            && store_flag == "--catalog-store"
            && text_flag == "--ocr-text"
            && confidence_flag == "--ocr-confidence" =>
        {
            diagnostic_title_spike(store, text, confidence)
        }
        _ => Err("usage: scorepeek --help".to_owned()),
    }
}

#[derive(Serialize)]
struct RegisteredResourceGateReport<'a> {
    schema: &'static str,
    status: &'static str,
    error_type: Option<RegisteredResourceGateErrorType>,
    catalog_sha256: &'a str,
    model_sha256: &'a str,
    runtime_sha256: &'a str,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum RegisteredResourceGateErrorType {
    InvalidBinding,
    WorkerUnavailable,
    FinishTimeout,
    InvalidLocation,
    ModelBindingMismatch,
    RuntimeBindingMismatch,
    CatalogUnavailable,
    CatalogBindingMismatch,
    CatalogLoadFailed,
    ModelBundleInvalid,
    RuntimeInitializationFailed,
}

struct RegisteredResourceOwner {
    _resources: recognition::RegisteredRecognitionResources,
}

impl recognition_live::field_observer::FieldObserver for RegisteredResourceOwner {
    type Output = ();

    fn observe(
        &mut self,
        _input: &recognition_live::field_observer::FieldObserverInput,
    ) -> Self::Output {
    }
}

impl From<recognition::RegisteredResourceLoadErrorType> for RegisteredResourceGateErrorType {
    fn from(error: recognition::RegisteredResourceLoadErrorType) -> Self {
        match error {
            recognition::RegisteredResourceLoadErrorType::InvalidLocation => Self::InvalidLocation,
            recognition::RegisteredResourceLoadErrorType::ModelBindingMismatch => {
                Self::ModelBindingMismatch
            }
            recognition::RegisteredResourceLoadErrorType::RuntimeBindingMismatch => {
                Self::RuntimeBindingMismatch
            }
            recognition::RegisteredResourceLoadErrorType::CatalogUnavailable => {
                Self::CatalogUnavailable
            }
            recognition::RegisteredResourceLoadErrorType::CatalogBindingMismatch => {
                Self::CatalogBindingMismatch
            }
            recognition::RegisteredResourceLoadErrorType::CatalogLoadFailed => {
                Self::CatalogLoadFailed
            }
            recognition::RegisteredResourceLoadErrorType::ModelBundleInvalid => {
                Self::ModelBundleInvalid
            }
            recognition::RegisteredResourceLoadErrorType::RuntimeInitializationFailed => {
                Self::RuntimeInitializationFailed
            }
        }
    }
}

fn try_registered_resource_gate(
    args: &[OsString],
    bundle_root: &Path,
) -> Option<Result<(), String>> {
    let [
        recognition_command,
        gate,
        catalog_flag,
        catalog_root,
        catalog_digest_flag,
        catalog_digest,
    ] = args
    else {
        return None;
    };
    if recognition_command != "recognition"
        || gate != "field-resource-load-gate"
        || catalog_flag != "--catalog-store"
        || catalog_digest_flag != "--catalog-sha256"
    {
        return None;
    }
    Some(registered_resource_gate(
        catalog_root,
        bundle_root,
        catalog_digest,
    ))
}

fn registered_resource_gate(
    catalog_root: &OsStr,
    bundle_root: &Path,
    catalog_digest: &OsStr,
) -> Result<(), String> {
    let catalog_digest = parse_cli_sha256(catalog_digest, "catalog SHA-256")?;
    let model_digest = recognition::LIVE_MODEL_SHA256.to_owned();
    let runtime_digest = recognition::LIVE_RUNTIME_SHA256.to_owned();
    let descriptor = diagnostic_recording::DiagnosticRunDescriptor {
        run_id: "field-resource-load-gate".to_owned(),
        monotonic_start_ms: 0,
        resource: diagnostic_recording::DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: "0".repeat(64),
        },
        binding: diagnostic_recording::DiagnosticBinding {
            capture_generation: 1,
            capture_profile_sha256: "0".repeat(64),
            normalizer_sha256: "0".repeat(64),
            canonical_layout_sha256: recognition::CanonicalLayout::sha256(),
            catalog_sha256: catalog_digest.clone(),
            model_sha256: model_digest.clone(),
            runtime_sha256: runtime_digest.clone(),
            replay: None,
        },
    };
    let worker =
        recognition_live::field_observer::FieldObserverWorker::start(&descriptor, |binding| {
            binding
                .load_registered_resources(Path::new(catalog_root), bundle_root)
                .map(|resources| RegisteredResourceOwner {
                    _resources: resources,
                })
        });
    match worker {
        Ok(worker) => {
            let outcome = worker
                .finish(recognition_live::field_observer::DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT);
            if outcome.status
                != recognition_live::field_observer::FieldObserverFinishStatus::Complete
            {
                let error_type = match outcome.status {
                    recognition_live::field_observer::FieldObserverFinishStatus::Timeout => {
                        RegisteredResourceGateErrorType::FinishTimeout
                    }
                    recognition_live::field_observer::FieldObserverFinishStatus::WorkerUnavailable => {
                        RegisteredResourceGateErrorType::WorkerUnavailable
                    }
                    recognition_live::field_observer::FieldObserverFinishStatus::Complete => {
                        unreachable!("complete outcome was handled above")
                    }
                };
                print_registered_resource_gate_report(
                    "error",
                    Some(error_type),
                    &catalog_digest,
                    &model_digest,
                    &runtime_digest,
                )?;
                return Err("registered resource worker did not finish cleanly".to_owned());
            }
            let report = RegisteredResourceGateReport {
                schema: "scorepeek-field-resource-load-gate-v1",
                status: "success",
                error_type: None,
                catalog_sha256: &catalog_digest,
                model_sha256: &model_digest,
                runtime_sha256: &runtime_digest,
            };
            println!(
                "{}",
                serde_json::to_string(&report)
                    .map_err(|_| "resource gate report serialization failed".to_owned())?
            );
            Ok(())
        }
        Err(error) => {
            let (error_type, message) = registered_resource_start_error(error);
            print_registered_resource_gate_report(
                "error",
                Some(error_type),
                &catalog_digest,
                &model_digest,
                &runtime_digest,
            )?;
            Err(message)
        }
    }
}

fn registered_resource_start_error(
    error: recognition_live::field_observer::FieldObserverStartError<
        recognition::RegisteredResourceLoadError,
    >,
) -> (RegisteredResourceGateErrorType, String) {
    use recognition_live::field_observer::FieldObserverStartError;
    match error {
        FieldObserverStartError::InvalidBinding => (
            RegisteredResourceGateErrorType::InvalidBinding,
            "registered resource worker binding is invalid".to_owned(),
        ),
        FieldObserverStartError::Load(error) => (error.error_type().into(), error.to_string()),
        FieldObserverStartError::WorkerUnavailable => (
            RegisteredResourceGateErrorType::WorkerUnavailable,
            "registered resource worker is unavailable".to_owned(),
        ),
    }
}

fn print_registered_resource_gate_report(
    status: &'static str,
    error_type: Option<RegisteredResourceGateErrorType>,
    catalog_sha256: &str,
    model_sha256: &str,
    runtime_sha256: &str,
) -> Result<(), String> {
    let report = RegisteredResourceGateReport {
        schema: "scorepeek-field-resource-load-gate-v1",
        status,
        error_type,
        catalog_sha256,
        model_sha256,
        runtime_sha256,
    };
    println!(
        "{}",
        serde_json::to_string(&report)
            .map_err(|_| "resource gate report serialization failed".to_owned())?
    );
    Ok(())
}

fn try_capture_commands(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    try_capture_result_recognition(args, bundle)
        .or_else(|| try_capture_field_observation(args, bundle))
        .or_else(|| try_capture_recognition_handoff(args))
        .or_else(|| try_capture_diagnostic_handoff(args))
        .or_else(|| try_capture_canonical_frame(args))
        .or_else(|| try_capture_binding_admission(args))
        .or_else(|| try_capture_session_calibration(args))
        .or_else(|| try_capture_binding_author(args))
        .or_else(|| try_capture_calibration(args))
        .or_else(|| try_capture_live_gate(args))
}

const CAPTURE_HANDOFF_FLAGS: &[&str] = &[
    "--binding",
    "--binding-sha256",
    "--capture-generation",
    "--duration-ms",
    "--diagnostic-root",
    "--run-id",
    "--build-sha256",
    "--canonical-layout-sha256",
    "--catalog-sha256",
    "--recording",
];

const CAPTURE_FIELD_OBSERVATION_FLAGS: &[&str] = &[
    "--binding",
    "--binding-sha256",
    "--capture-generation",
    "--duration-ms",
    "--diagnostic-root",
    "--catalog-store",
    "--run-id",
    "--build-sha256",
    "--canonical-layout-sha256",
    "--catalog-sha256",
    "--recording",
];

const CAPTURE_RESULT_RECOGNITION_FLAGS: &[&str] = &[
    "--binding",
    "--binding-sha256",
    "--capture-generation",
    "--duration-ms",
    "--diagnostic-root",
    "--catalog-store",
    "--run-id",
    "--build-sha256",
    "--canonical-layout-sha256",
    "--catalog-sha256",
    "--recording",
    "--recognition-artifact",
];

const LIVE_SESSION_FLAGS: &[&str] = &[
    "--binding",
    "--binding-sha256",
    "--capture-generation",
    "--diagnostic-root",
    "--catalog-store",
    "--run-id",
    "--build-sha256",
    "--canonical-layout-sha256",
    "--catalog-sha256",
    "--recording",
    "--recognition-artifact",
];

fn try_routine_live_session(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let [run, options @ ..] = args else {
        return None;
    };
    if run != "run" {
        return None;
    }
    let options = match parse_routine_run_options(options) {
        Ok(options) => options,
        Err(error) => return Some(Err(error)),
    };
    Some(run_routine_live_session(
        options.profile,
        if options.recording {
            "enabled"
        } else {
            "disabled"
        },
        options.recording_memory_limit,
        bundle,
    ))
}

struct RoutineRunOptions<'a> {
    profile: Option<&'a OsStr>,
    recording: bool,
    recording_memory_limit: canonical_recording::RecordingMemoryLimit,
}

fn parse_routine_run_options(options: &[OsString]) -> Result<RoutineRunOptions<'_>, String> {
    let mut profile = None;
    let mut recording = false;
    let mut recording_memory_mib = None;
    let mut index = 0;
    while index < options.len() {
        match options[index].to_str() {
            Some("--record") if !recording => recording = true,
            Some("--profile") if profile.is_none() => {
                index += 1;
                let Some(value) = options.get(index) else {
                    return Err("--profile requires a profile name".to_owned());
                };
                profile = Some(value.as_os_str());
            }
            Some("--record-memory-mib") if recording_memory_mib.is_none() => {
                index += 1;
                let Some(value) = options.get(index).and_then(|value| value.to_str()) else {
                    return Err("--record-memory-mib requires an integer MiB value".to_owned());
                };
                recording_memory_mib =
                    Some(value.parse::<usize>().map_err(|_| {
                        "--record-memory-mib requires an integer MiB value".to_owned()
                    })?);
            }
            Some(option) => return Err(format!("unknown or duplicate run option: {option}")),
            None => return Err("run option must be UTF-8".to_owned()),
        }
        index += 1;
    }
    if recording_memory_mib.is_some() && !recording {
        return Err("--record-memory-mib requires --record".to_owned());
    }
    let recording_memory_limit = canonical_recording::RecordingMemoryLimit::from_mib(
        recording_memory_mib.unwrap_or(canonical_recording::DEFAULT_RECORDING_MEMORY_MIB),
    )?;
    Ok(RoutineRunOptions {
        profile,
        recording,
        recording_memory_limit,
    })
}

#[allow(clippy::too_many_lines)]
fn run_routine_live_session(
    profile_name: Option<&OsStr>,
    recording: &str,
    recording_memory_limit: canonical_recording::RecordingMemoryLimit,
    bundle: &Path,
) -> Result<(), String> {
    let selected = local_profiles::select_for_run(profile_name)?;
    let (catalog_root, _) = catalog_paths(
        env::var_os("XDG_DATA_HOME").as_deref(),
        env::var_os("XDG_CACHE_HOME").as_deref(),
        env::var_os("HOME").as_deref(),
    )?;
    CatalogStore::new(&catalog_root)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| {
            format!(
                "catalog store {} has no active catalog; transfer or sync the catalog first",
                catalog_root.display()
            )
        })?;
    let elapsed = std::time::SystemTime::UNIX_EPOCH
        .elapsed()
        .map_err(|_| "system clock is before the Unix epoch".to_owned())?;
    let invocation_id = format!(
        "run-{}-{}-{}",
        elapsed.as_secs(),
        elapsed.subsec_nanos(),
        std::process::id()
    );
    let recording_enabled = recording == "enabled";
    if recording_enabled {
        canonical_recording::CanonicalRecordingWorker::preflight()?;
    }
    let state = local_profiles::state_paths(recording_enabled)?;
    let build_sha256 = current_executable_sha256()?;
    let monitor = live_control::SignalStopMonitor::start()?;
    let stop = monitor.stop_token();
    let mut output = routine_output::RoutineOutput::start(
        invocation_id.clone(),
        selected.binding.capture_profile_sha256().to_owned(),
        recording_enabled,
        state.recording_staging_store(),
    )?;
    output.publish(&routine_output::RunEvent {
        schema: "scorepeek-run-event-v9".to_owned(),
        kind: routine_output::RunEventKind::WatcherStarted {
            invocation_id: invocation_id.clone(),
            profile_sha256: selected.binding.capture_profile_sha256().to_owned(),
        },
    })?;

    let mut lifetimes = routine_watcher::SourceLifetimes::new();
    let mut announced = None;
    while !stop.load(std::sync::atomic::Ordering::Acquire) {
        let Ok(snapshot) =
            scorepeek::capture::snapshot_gamescope_sources(std::time::Duration::from_millis(500))
        else {
            announce_watcher_state(
                &mut announced,
                routine_watcher::WatcherState::RemoteUnavailable,
                "PipeWire is unavailable; scorepeek will keep waiting",
                &mut output,
            )?;
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        };
        if stop.load(std::sync::atomic::Ordering::Acquire) {
            break;
        }
        let decision = lifetimes.observe(snapshot);
        match decision {
            routine_watcher::WatchDecision::WaitAbsent
            | routine_watcher::WatchDecision::WaitConsumed => {
                announce_watcher_state(
                    &mut announced,
                    routine_watcher::WatcherState::WaitingForSource,
                    "waiting for a Gamescope PipeWire source",
                    &mut output,
                )?;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            routine_watcher::WatchDecision::WaitAmbiguous => {
                announce_watcher_state(
                    &mut announced,
                    routine_watcher::WatcherState::AmbiguousSources,
                    "multiple Gamescope sources are present; waiting for exactly one",
                    &mut output,
                )?;
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            routine_watcher::WatchDecision::Admit {
                node_id,
                generation,
            } => {
                let Ok(Some(active)) = CatalogStore::new(&catalog_root).load_active() else {
                    announce_watcher_state(
                        &mut announced,
                        routine_watcher::WatcherState::CatalogUnavailable,
                        "active catalog is temporarily unavailable; scorepeek will retry",
                        &mut output,
                    )?;
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    continue;
                };
                let session_id = format!("{invocation_id}-session-{generation}");
                let session_paths = match state.start_recording_session(&session_id) {
                    Ok(paths) => paths,
                    Err(error) => {
                        output.warning(format!(
                            "recording staging degraded for this session: {error}"
                        ))?;
                        None
                    }
                };
                let diagnostic_root = session_paths
                    .as_ref()
                    .map_or(Path::new("/"), |paths| paths.capture_root.as_path());
                let recognition_root = session_paths
                    .as_ref()
                    .map(|paths| paths.recognition_directory.as_path());
                let values = routine_live_values(
                    &selected,
                    generation,
                    diagnostic_root,
                    &catalog_root,
                    &session_id,
                    &build_sha256,
                    &active.digest,
                    recording,
                    recognition_root,
                );
                let references = values.iter().map(OsString::as_os_str).collect::<Vec<_>>();
                if stop.load(std::sync::atomic::Ordering::Acquire) {
                    if let Some(paths) = session_paths.as_ref()
                        && let Err(error) = paths.cleanup()
                    {
                        output.warning(error)?;
                    }
                    break;
                }
                let mut started = false;
                let mut emit = |emission: LiveSessionEmission| {
                    let output_started = std::time::Instant::now();
                    let event = run_event_from_live_emission(emission)?;
                    if matches!(
                        &event.kind,
                        routine_output::RunEventKind::SessionStarted { .. }
                    ) {
                        started = true;
                    }
                    let output_overhead_us =
                        u64::try_from(output_started.elapsed().as_micros()).unwrap_or(u64::MAX);
                    let timing = output.publish_timed(&event)?;
                    Ok(capture_live::LiveEventProcessingTiming {
                        screen_resolver_us: timing.screen_resolver_us,
                        attempt_resolver_us: timing.attempt_resolver_us,
                        output_us: Some(
                            timing
                                .output_us
                                .unwrap_or(0)
                                .saturating_add(output_overhead_us),
                        ),
                    })
                };
                let report = execute_live_session(
                    &references,
                    bundle,
                    recognition_root.is_some(),
                    recording_memory_limit,
                    Some(&session_id),
                    Some(node_id),
                    &stop,
                    &mut emit,
                )?;
                if report.output_failed() {
                    return Err("live result output failed".to_owned());
                }
                if started {
                    lifetimes.admitted(node_id);
                    announced = None;
                    let outcome = match report.stop_reason() {
                        Some(capture_live::LiveSessionStopReason::RequestedSignal) => "stopped",
                        Some(capture_live::LiveSessionStopReason::SourceEnded) => "source_ended",
                        _ => "error",
                    };
                    output.publish(&routine_output::RunEvent {
                        schema: "scorepeek-run-event-v9".to_owned(),
                        kind: routine_output::RunEventKind::SessionFinished {
                            session_id: session_id.clone(),
                            capture_generation: generation,
                            outcome: outcome.to_owned(),
                            report: serde_json::to_value(&report).map_err(|error| {
                                format!("live report serialization failed: {error}")
                            })?,
                        },
                    })?;
                    let event_artifact = output.take_completed_event_artifact();
                    let mut recording_published = false;
                    if let (
                        Some(session_paths),
                        Some(recognition_root),
                        Some(capture_manifest_sha256),
                        Some(recognition_manifest_sha256),
                        Some(event_artifact),
                    ) = (
                        session_paths.as_ref(),
                        recognition_root,
                        report.diagnostic_manifest_sha256(),
                        report.recognition_artifact_manifest_sha256(),
                        event_artifact.as_ref(),
                    ) && let Some(event_manifest_sha256) =
                        event_artifact.manifest_sha256.as_deref()
                    {
                        let (processed_ticks, busy_skips, maximum_consecutive_busy_skips) =
                            report.recognition_sampling();
                        let (
                            field_observation_busy_skips,
                            maximum_consecutive_field_observation_busy_skips,
                        ) = report.field_busy_sampling();
                        let completeness = if report.diagnostic_completeness_name() == "complete"
                            && event_artifact.complete
                            && report.canonical_recording_is_complete()
                        {
                            "complete"
                        } else {
                            "partial"
                        };
                        match session_artifact::publish(&session_artifact::PublishRequest {
                            root: &state.diagnostic_session_store,
                            session_id: &session_id,
                            capture_generation: generation,
                            profile_sha256: selected.binding.capture_profile_sha256(),
                            catalog_sha256: &active.digest,
                            processed_ticks,
                            busy_skips,
                            maximum_consecutive_busy_skips,
                            field_observation_busy_skips,
                            maximum_consecutive_field_observation_busy_skips,
                            completeness,
                            capture_directory: &session_paths.capture_directory,
                            capture_manifest_sha256,
                            recognition_directory: recognition_root,
                            recognition_manifest_sha256,
                            canonical_directory: &session_paths.root.join("canonical"),
                            event_directory: &event_artifact.root,
                            event_manifest_sha256,
                            profile_path: &selected.path,
                        }) {
                            Ok(published) => {
                                if let Err(error) = session_paths.cleanup() {
                                    output.warning(error)?;
                                }
                                if completeness == "complete" {
                                    recording_published = true;
                                    output.publish(&routine_output::RunEvent {
                                        schema: "scorepeek-run-event-v9".to_owned(),
                                        kind: routine_output::RunEventKind::RecordingReady {
                                            session_id: session_id.clone(),
                                            directory: published.directory.display().to_string(),
                                            manifest_sha256: published.manifest_sha256,
                                        },
                                    })?;
                                } else {
                                    output.status_recording_degraded()?;
                                    output.warning(format!(
                                        "partial session was published for diagnosis but is not importable: {}",
                                        published.directory.display()
                                    ))?;
                                }
                            }
                            Err(error) => {
                                output.status_recording_degraded()?;
                                output.warning(format!(
                                    "diagnostic session publication degraded: {error}"
                                ))?;
                            }
                        }
                    }
                    if state.recording_enabled {
                        if !recording_published {
                            output.status_recording_degraded()?;
                        }
                        if report.diagnostic_manifest_sha256().is_none() {
                            output.warning(
                                "diagnostic session was not published: capture component has no manifest",
                            )?;
                        }
                        if recognition_root.is_some()
                            && report.recognition_artifact_manifest_sha256().is_none()
                        {
                            output.warning(
                                "diagnostic session was not published: recognition component has no manifest",
                            )?;
                        }
                        if report.canonical_recording_manifest_sha256().is_none() {
                            output.warning(
                                "diagnostic session was not published: canonical recording component has no manifest",
                            )?;
                        }
                        match event_artifact.as_ref() {
                            None => output.warning(
                                "diagnostic session was not published: run-event component did not finish",
                            )?,
                            Some(artifact) if artifact.manifest_sha256.is_none() => output.warning(
                                format!(
                                    "diagnostic session was not published: run-event component has no manifest{}",
                                    artifact
                                        .error
                                        .as_deref()
                                        .map_or_else(String::new, |error| format!(": {error}"))
                                ),
                            )?,
                            Some(artifact) if !artifact.complete => output.warning(format!(
                                "diagnostic run-event recording is partial: {} events were dropped",
                                artifact.dropped
                            ))?,
                            Some(_) => {}
                        }
                    }
                } else {
                    if let Some(paths) = session_paths.as_ref()
                        && let Err(error) = paths.cleanup()
                    {
                        output.warning(error)?;
                    }
                    match report.startup_retry() {
                        Some(capture_live::LiveSessionStartupRetry::Admission) => {
                            announce_watcher_state(
                                &mut announced,
                                routine_watcher::WatcherState::AdmissionRejected,
                                "Gamescope source is not ready; scorepeek will keep waiting",
                                &mut output,
                            )?;
                        }
                        Some(capture_live::LiveSessionStartupRetry::Catalog) => {
                            announce_watcher_state(
                                &mut announced,
                                routine_watcher::WatcherState::CatalogUnavailable,
                                "active catalog changed or is temporarily unavailable; scorepeek will retry",
                                &mut output,
                            )?;
                        }
                        None => return Err(report.startup_failure_summary()),
                    }
                    std::thread::sleep(std::time::Duration::from_millis(500));
                }
            }
        }
    }
    output.publish(&routine_output::RunEvent {
        schema: "scorepeek-run-event-v9".to_owned(),
        kind: routine_output::RunEventKind::WatcherStopped {
            invocation_id,
            reason: "signal".to_owned(),
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn routine_live_values(
    selected: &local_profiles::SelectedProfile,
    generation: u64,
    diagnostic_root: &Path,
    catalog_root: &Path,
    session_id: &str,
    build_sha256: &str,
    catalog_sha256: &str,
    recording: &str,
    recognition_root: Option<&Path>,
) -> Vec<OsString> {
    vec![
        selected.path.clone().into_os_string(),
        selected.digest.clone().into(),
        generation.to_string().into(),
        diagnostic_root.as_os_str().to_owned(),
        catalog_root.as_os_str().to_owned(),
        session_id.into(),
        build_sha256.into(),
        recognition::CanonicalLayout::sha256().into(),
        catalog_sha256.into(),
        recording.into(),
        recognition_root
            .unwrap_or(Path::new("/"))
            .as_os_str()
            .to_owned(),
    ]
}

fn announce_watcher_state(
    announced: &mut Option<routine_watcher::WatcherState>,
    state: routine_watcher::WatcherState,
    message: &str,
    output: &mut routine_output::RoutineOutput,
) -> Result<(), String> {
    output.watcher_state(state.as_str(), None, None, message)?;
    if *announced != Some(state) {
        *announced = Some(state);
    }
    Ok(())
}

fn try_live_session(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let values = command_flag_values(args, "run", "gamescope", LIVE_SESSION_FLAGS)?;
    Some(run_live_session(&values, bundle, true))
}

fn try_capture_result_recognition(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let values = capture_flag_values(
        args,
        "gamescope-result-recognition-gate",
        CAPTURE_RESULT_RECOGNITION_FLAGS,
    )?;
    let (artifact, common) = values
        .split_last()
        .expect("result recognition flags are non-empty");
    Some(run_capture_field_observation(
        common,
        bundle,
        Some(Path::new(artifact)),
    ))
}

fn try_capture_field_observation(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let values = capture_flag_values(
        args,
        "gamescope-field-observation-gate",
        CAPTURE_FIELD_OBSERVATION_FLAGS,
    )?;
    Some(run_capture_field_observation(&values, bundle, None))
}

fn try_capture_recognition_handoff(args: &[OsString]) -> Option<Result<(), String>> {
    let values = capture_flag_values(
        args,
        "gamescope-recognition-handoff-gate",
        CAPTURE_HANDOFF_FLAGS,
    )?;
    Some(run_capture_handoff(&values, true))
}

fn try_capture_diagnostic_handoff(args: &[OsString]) -> Option<Result<(), String>> {
    let values = capture_flag_values(
        args,
        "gamescope-diagnostic-handoff-gate",
        CAPTURE_HANDOFF_FLAGS,
    )?;
    Some(run_capture_handoff(&values, false))
}

fn capture_flag_values<'a>(
    args: &'a [OsString],
    command: &str,
    flags: &[&str],
) -> Option<Vec<&'a OsStr>> {
    command_flag_values(args, "capture", command, flags)
}

fn command_flag_values<'a>(
    args: &'a [OsString],
    namespace: &str,
    command: &str,
    flags: &[&str],
) -> Option<Vec<&'a OsStr>> {
    if args.first()? != namespace || args.get(1)? != command || args.len() != 2 + flags.len() * 2 {
        return None;
    }
    let mut values = Vec::with_capacity(flags.len());
    for (pair, expected_flag) in args[2..].chunks_exact(2).zip(flags) {
        if pair[0] != *expected_flag {
            return None;
        }
        values.push(pair[1].as_os_str());
    }
    Some(values)
}

fn run_live_session(
    values: &[&OsStr],
    bundle_root: &Path,
    persist_recognition: bool,
) -> Result<(), String> {
    let monitor = live_control::SignalStopMonitor::start()?;
    let stop = monitor.stop_token();
    let stdout = io::stdout();
    let mut output = BufWriter::new(stdout.lock());
    let mut emit = |emission: LiveSessionEmission| {
        let started = std::time::Instant::now();
        write_ndjson(&mut output, &emission.value)?;
        Ok(capture_live::LiveEventProcessingTiming {
            screen_resolver_us: None,
            attempt_resolver_us: None,
            output_us: Some(u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)),
        })
    };
    let report = execute_live_session(
        values,
        bundle_root,
        persist_recognition,
        canonical_recording::RecordingMemoryLimit::default_limit(),
        None,
        None,
        &stop,
        &mut emit,
    )?;
    write_ndjson(&mut output, &report)?;
    report.succeeded().then_some(()).ok_or_else(|| {
        report
            .failure_detail()
            .unwrap_or("Gamescope live recognition session failed")
            .to_owned()
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn execute_live_session(
    values: &[&OsStr],
    bundle_root: &Path,
    persist_recognition: bool,
    recording_memory_limit: canonical_recording::RecordingMemoryLimit,
    session_id: Option<&str>,
    expected_source_node_id: Option<u32>,
    stop: &std::sync::atomic::AtomicBool,
    emit: &mut impl FnMut(
        LiveSessionEmission,
    ) -> Result<capture_live::LiveEventProcessingTiming, String>,
) -> Result<capture_live::GamescopeFieldObservationGateReport, String> {
    let [
        binding,
        binding_digest,
        generation,
        diagnostic_root,
        catalog_root,
        run_id,
        build_digest,
        layout_digest,
        catalog_digest,
        recording,
        recognition_artifact_root,
    ] = values
    else {
        unreachable!("live session flag parser returns the exact value count");
    };
    let binding_digest = parse_cli_sha256(binding_digest, "binding SHA-256")?;
    let generation = parse_capture_generation(generation)?;
    let descriptor = diagnostic_recording::DiagnosticRunDescriptor {
        run_id: parse_diagnostic_run_id(run_id)?,
        monotonic_start_ms: 0,
        resource: diagnostic_recording::DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
        },
        binding: diagnostic_recording::DiagnosticBinding {
            capture_generation: generation.get(),
            capture_profile_sha256: String::new(),
            normalizer_sha256: String::new(),
            canonical_layout_sha256: parse_cli_sha256(layout_digest, "canonical layout SHA-256")?,
            catalog_sha256: parse_cli_sha256(catalog_digest, "catalog SHA-256")?,
            model_sha256: recognition::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: recognition::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    };
    let mut policy = parse_diagnostic_recording_policy(recording)?;
    policy.retention = if session_id.is_some() {
        diagnostic_recording::DiagnosticRetention::FactsOnly
    } else {
        diagnostic_recording::DiagnosticRetention::ForegroundFailureWindowV1
    };
    let diagnostic_preflight = prepare_live_diagnostic_root(Path::new(diagnostic_root), &policy);
    if session_id.is_none() {
        emit(LiveSessionEmission {
            value: serde_json::to_value(&diagnostic_preflight)
                .map_err(|error| format!("live result serialization failed: {error}"))?,
            authority_joint_evidence: None,
        })?;
    }
    let report = capture_live::run_gamescope_live_session(
        capture_live::GamescopeFieldObservationGateConfig {
            handoff: capture_live::GamescopeDiagnosticHandoffGateConfig {
                binding_path: Path::new(binding),
                expected_binding_sha256: &binding_digest,
                capture_generation: generation,
                descriptor,
                policy,
                duration_ms: 0,
                diagnostic_root: Path::new(diagnostic_root),
                diagnostic_directory_name: session_id.map(|_| "capture"),
                expected_source_node_id,
            },
            catalog_root: Path::new(catalog_root),
            bundle_root,
            recognition_artifact_root: optional_recognition_root(
                persist_recognition,
                Path::new(recognition_artifact_root),
            ),
            recognition_artifact_retention:
                recognition_artifact::RecognitionArtifactRetention::Complete,
            recording_memory_limit,
        },
        stop,
        &mut |event| {
            let started = std::time::Instant::now();
            let authority_joint_evidence = if session_id.is_some() {
                match &event {
                    capture_live::GamescopeLiveSessionEvent::Observation { output, .. } => {
                        Some(output.joint_evidence().clone())
                    }
                    _ => None,
                }
            } else {
                None
            };
            let value =
                live_session_event_value(session_id, session_id.map(|_| generation.get()), event)?;
            let serialization_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let mut timing = emit(LiveSessionEmission {
                value,
                authority_joint_evidence,
            })?;
            timing.add(capture_live::LiveEventProcessingTiming {
                screen_resolver_us: None,
                attempt_resolver_us: None,
                output_us: Some(serialization_us),
            });
            Ok(timing)
        },
    );
    Ok(report)
}

struct LiveSessionEmission {
    value: serde_json::Value,
    authority_joint_evidence:
        Option<recognition_live::screen_field_observer::JointEvidenceObservation>,
}

fn run_event_from_live_emission(
    emission: LiveSessionEmission,
) -> Result<routine_output::RunEvent, String> {
    let mut event = routine_output::RunEvent::from_value(emission.value)?;
    if let Some(authority_joint_evidence) = emission.authority_joint_evidence {
        let routine_output::RunEventKind::FieldObservation { joint_evidence, .. } = &mut event.kind
        else {
            return Err("full joint evidence was attached to a non-field event".to_owned());
        };
        *joint_evidence = authority_joint_evidence;
    }
    Ok(event)
}

fn optional_recognition_root(enabled: bool, root: &Path) -> Option<&Path> {
    enabled.then_some(root)
}

fn current_executable_sha256() -> Result<String, String> {
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
struct LiveDiagnosticPreflight<'a> {
    schema: &'static str,
    event: &'static str,
    status: &'static str,
    root: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_type: Option<&'static str>,
}

fn prepare_live_diagnostic_root<'a>(
    root: &'a Path,
    policy: &diagnostic_recording::DiagnosticPolicy,
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

fn prepare_private_directory(path: &Path) -> bool {
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
fn live_session_event_value(
    session_id: Option<&str>,
    routine_generation: Option<u64>,
    event: capture_live::GamescopeLiveSessionEvent<'_>,
) -> Result<serde_json::Value, String> {
    let schema = if session_id.is_some() {
        "scorepeek-run-event-v9"
    } else {
        "scorepeek-live-session-event-v1"
    };
    let value = match event {
        capture_live::GamescopeLiveSessionEvent::Started {
            capture_generation,
            capture_profile_sha256,
            normalizer_artifact_sha256,
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
        capture_live::GamescopeLiveSessionEvent::RawScreenObserved {
            semantic_episode_id,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            screen,
        } => {
            let mut value = serde_json::json!({
                "schema": schema,
                "event": "raw_screen_observed",
                "semantic_episode_id": semantic_episode_id,
                "sequence": sequence,
                "monotonic_start_ms": monotonic_start_ms,
                "monotonic_end_ms": monotonic_end_ms,
                "screen": screen,
                "unknown_reason": (screen == scorepeek::recognition::ScreenClass::Unknown)
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
        capture_live::GamescopeLiveSessionEvent::Observation {
            screen_episode_id,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            output: observation,
        } => {
            let (screen, fields) = match observation.fields() {
                scorepeek::recognition::ScreenFieldObservations::Result(fields) => (
                    "result",
                    serde_json::json!({
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
                scorepeek::recognition::ScreenFieldObservations::MusicSelect(fields) => (
                    "music_select",
                    serde_json::json!({
                        "central_title": fields.central_title.open_text,
                        "artist": fields.artist.open_text,
                        "selected_difficulty": fields.selected_difficulty,
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

fn song_resolution_presentation(
    observation: &recognition_live::screen_field_observer::RegisteredScreenFieldObservation,
) -> Result<routine_output::SongResolutionPresentation, String> {
    use scorepeek::recognition::{MusicSelectSongResolution, ResultSongResolution};

    match observation.song_resolution() {
        scorepeek::recognition::ScreenSongResolution::Result(resolution) => match resolution {
            ResultSongResolution::Accepted {
                selected,
                runner_up,
                title_edit_margin,
                ..
            } => Ok(routine_output::SongResolutionPresentation::Accepted {
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
            } => Ok(routine_output::SongResolutionPresentation::Unknown {
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
        scorepeek::recognition::ScreenSongResolution::MusicSelect(resolution) => match resolution {
            MusicSelectSongResolution::Accepted {
                selected,
                runner_up,
                active_prefix_edit_margin,
                corroboration,
                ..
            } => Ok(routine_output::SongResolutionPresentation::Accepted {
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
            } => Ok(routine_output::SongResolutionPresentation::Unknown {
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

fn song_presentation(
    observation: &recognition_live::screen_field_observer::RegisteredScreenFieldObservation,
    song_id: scorepeek::catalog::ScorepeekSongId,
) -> Result<routine_output::SongPresentation, String> {
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
    Ok(routine_output::SongPresentation {
        scorepeek_song_id: song_id,
        display_titles: evidence.title.display.clone(),
        artist: artist.clone(),
    })
}

fn write_ndjson(output: &mut impl io::Write, value: &impl Serialize) -> Result<(), String> {
    serde_json::to_writer(&mut *output, value)
        .map_err(|error| format!("live result serialization failed: {error}"))?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|error| format!("live result output failed: {error}"))
}

fn run_capture_handoff(values: &[&OsStr], inspect_screen: bool) -> Result<(), String> {
    let [
        binding,
        binding_digest,
        generation,
        duration,
        diagnostic_root,
        run_id,
        build_digest,
        layout_digest,
        catalog_digest,
        recording,
    ] = values
    else {
        unreachable!("capture flag parser returns the exact value count");
    };
    let binding_digest = parse_cli_sha256(binding_digest, "binding SHA-256")?;
    let generation = parse_capture_generation(generation)?;
    let duration_ms = capture_live::parse_duration_ms(duration)?;
    let run_id = parse_diagnostic_run_id(run_id)?;
    let policy = parse_diagnostic_recording_policy(recording)?;
    let descriptor = diagnostic_recording::DiagnosticRunDescriptor {
        run_id,
        monotonic_start_ms: 0,
        resource: diagnostic_recording::DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
        },
        binding: diagnostic_recording::DiagnosticBinding {
            capture_generation: generation.get(),
            capture_profile_sha256: String::new(),
            normalizer_sha256: String::new(),
            canonical_layout_sha256: parse_cli_sha256(layout_digest, "canonical layout SHA-256")?,
            catalog_sha256: parse_cli_sha256(catalog_digest, "catalog SHA-256")?,
            model_sha256: recognition::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: recognition::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    };
    let config = capture_live::GamescopeDiagnosticHandoffGateConfig {
        binding_path: Path::new(binding),
        expected_binding_sha256: &binding_digest,
        capture_generation: generation,
        descriptor,
        policy,
        duration_ms,
        diagnostic_root: Path::new(diagnostic_root),
        diagnostic_directory_name: None,
        expected_source_node_id: None,
    };
    if inspect_screen {
        let report = capture_live::run_gamescope_recognition_handoff_gate(config);
        print_capture_handoff_report(
            &report,
            report.succeeded(),
            "Gamescope recognition handoff gate failed",
        )
    } else {
        let report = capture_live::run_gamescope_diagnostic_handoff_gate(config);
        print_capture_handoff_report(
            &report,
            report.succeeded(),
            "Gamescope diagnostic handoff gate failed",
        )
    }
}

fn run_capture_field_observation(
    values: &[&OsStr],
    bundle_root: &Path,
    recognition_artifact_root: Option<&Path>,
) -> Result<(), String> {
    let [
        binding,
        binding_digest,
        generation,
        duration,
        diagnostic_root,
        catalog_root,
        run_id,
        build_digest,
        layout_digest,
        catalog_digest,
        recording,
    ] = values
    else {
        unreachable!("capture flag parser returns the exact value count");
    };
    let binding_digest = parse_cli_sha256(binding_digest, "binding SHA-256")?;
    let generation = parse_capture_generation(generation)?;
    let duration_ms = capture_live::parse_duration_ms(duration)?;
    let run_id = parse_diagnostic_run_id(run_id)?;
    let policy = parse_diagnostic_recording_policy(recording)?;
    let descriptor = diagnostic_recording::DiagnosticRunDescriptor {
        run_id,
        monotonic_start_ms: 0,
        resource: diagnostic_recording::DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
        },
        binding: diagnostic_recording::DiagnosticBinding {
            capture_generation: generation.get(),
            capture_profile_sha256: String::new(),
            normalizer_sha256: String::new(),
            canonical_layout_sha256: parse_cli_sha256(layout_digest, "canonical layout SHA-256")?,
            catalog_sha256: parse_cli_sha256(catalog_digest, "catalog SHA-256")?,
            model_sha256: recognition::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: recognition::LIVE_RUNTIME_SHA256.to_owned(),
            replay: None,
        },
    };
    let handoff = capture_live::GamescopeDiagnosticHandoffGateConfig {
        binding_path: Path::new(binding),
        expected_binding_sha256: &binding_digest,
        capture_generation: generation,
        descriptor,
        policy,
        duration_ms,
        diagnostic_root: Path::new(diagnostic_root),
        diagnostic_directory_name: None,
        expected_source_node_id: None,
    };
    let report = capture_live::run_gamescope_field_observation_gate(
        capture_live::GamescopeFieldObservationGateConfig {
            handoff,
            catalog_root: Path::new(catalog_root),
            bundle_root,
            recognition_artifact_root,
            recognition_artifact_retention:
                recognition_artifact::RecognitionArtifactRetention::Complete,
            recording_memory_limit: canonical_recording::RecordingMemoryLimit::default_limit(),
        },
    );
    println!(
        "{}",
        serde_json::to_string(&report)
            .map_err(|_| "capture handoff gate report serialization failed".to_owned())?
    );
    report.succeeded().then_some(()).ok_or_else(|| {
        report
            .failure_detail()
            .unwrap_or("Gamescope field observation or recognition artifact gate failed")
            .to_owned()
    })
}

fn print_capture_handoff_report(
    report: &impl Serialize,
    succeeded: bool,
    failure: &str,
) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(report)
            .map_err(|_| "capture handoff gate report serialization failed".to_owned())?
    );
    succeeded.then_some(()).ok_or_else(|| failure.to_owned())
}

fn parse_cli_sha256(value: &OsStr, label: &str) -> Result<String, String> {
    let value = value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(format!("{label} must be lowercase hexadecimal"));
    }
    Ok(value.to_owned())
}

fn parse_capture_generation(
    value: &OsStr,
) -> Result<scorepeek::capture::CaptureGeneration, String> {
    let generation = value
        .to_str()
        .ok_or_else(|| "capture generation must be UTF-8".to_owned())?
        .parse::<u64>()
        .map_err(|_| "capture generation must be an integer".to_owned())?;
    scorepeek::capture::CaptureGeneration::new(generation)
        .map_err(|_| "capture generation must be nonzero".to_owned())
}

fn parse_diagnostic_run_id(value: &OsStr) -> Result<String, String> {
    let value = value
        .to_str()
        .ok_or_else(|| "diagnostic run ID must be UTF-8".to_owned())?;
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(
            "diagnostic run ID must be 1-64 lowercase ASCII letters, digits, or hyphens".to_owned(),
        );
    }
    Ok(value.to_owned())
}

fn parse_diagnostic_recording_policy(
    value: &OsStr,
) -> Result<diagnostic_recording::DiagnosticPolicy, String> {
    match value.to_str() {
        Some("enabled") => Ok(diagnostic_recording::DiagnosticPolicy::default()),
        Some("disabled") => Ok(diagnostic_recording::DiagnosticPolicy {
            enabled: false,
            ..diagnostic_recording::DiagnosticPolicy::default()
        }),
        _ => Err("recording must be enabled or disabled".to_owned()),
    }
}

fn try_capture_canonical_frame(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        capture,
        command,
        binding_flag,
        binding,
        binding_digest_flag,
        binding_digest,
        generation_flag,
        generation,
    ] = args
    else {
        return None;
    };
    (capture == "capture"
        && command == "gamescope-canonical-frame-gate"
        && binding_flag == "--binding"
        && binding_digest_flag == "--binding-sha256"
        && generation_flag == "--capture-generation")
        .then(|| {
            let expected_digest = binding_digest
                .to_str()
                .ok_or_else(|| "binding digest must be UTF-8".to_owned())?;
            let generation = generation
                .to_str()
                .ok_or_else(|| "capture generation must be UTF-8".to_owned())?
                .parse::<u64>()
                .map_err(|_| "capture generation must be an integer".to_owned())?;
            let generation = scorepeek::capture::CaptureGeneration::new(generation)
                .map_err(|_| "capture generation must be nonzero".to_owned())?;
            let report = capture_live::run_gamescope_canonical_frame_gate(
                Path::new(binding),
                expected_digest,
                generation,
            );
            println!(
                "{}",
                serde_json::to_string(&report)
                    .map_err(|_| "canonical frame gate report serialization failed".to_owned())?
            );
            report
                .succeeded()
                .then_some(())
                .ok_or_else(|| "Gamescope canonical frame gate failed".to_owned())
        })
}

fn try_capture_binding_admission(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        capture,
        command,
        binding_flag,
        binding,
        binding_digest_flag,
        binding_digest,
    ] = args
    else {
        return None;
    };
    (capture == "capture"
        && command == "gamescope-binding-admission-gate"
        && binding_flag == "--binding"
        && binding_digest_flag == "--binding-sha256")
        .then(|| {
            let expected_digest = binding_digest
                .to_str()
                .ok_or_else(|| "binding digest must be UTF-8".to_owned())?;
            let report = capture_live::run_gamescope_binding_admission_gate(
                Path::new(binding),
                expected_digest,
            );
            println!(
                "{}",
                serde_json::to_string(&report)
                    .map_err(|_| "binding admission report serialization failed".to_owned())?
            );
            report
                .succeeded()
                .then_some(())
                .ok_or_else(|| "Gamescope profile binding admission failed".to_owned())
        })
}

fn try_capture_binding_author(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        capture,
        command,
        calibration_flag,
        calibration,
        calibration_digest_flag,
        calibration_digest,
        output_flag,
        output,
        left_numerator_flag,
        left_numerator,
        left_denominator_flag,
        left_denominator,
        top_numerator_flag,
        top_numerator,
        top_denominator_flag,
        top_denominator,
        width_numerator_flag,
        width_numerator,
        width_denominator_flag,
        width_denominator,
        height_numerator_flag,
        height_numerator,
        height_denominator_flag,
        height_denominator,
    ] = args
    else {
        return None;
    };
    (capture == "capture"
        && command == "gamescope-profile-binding-author"
        && calibration_flag == "--calibration"
        && calibration_digest_flag == "--calibration-sha256"
        && output_flag == "--output"
        && left_numerator_flag == "--left-numerator"
        && left_denominator_flag == "--left-denominator"
        && top_numerator_flag == "--top-numerator"
        && top_denominator_flag == "--top-denominator"
        && width_numerator_flag == "--width-numerator"
        && width_denominator_flag == "--width-denominator"
        && height_numerator_flag == "--height-numerator"
        && height_denominator_flag == "--height-denominator")
        .then(|| {
            let expected_digest = calibration_digest
                .to_str()
                .ok_or_else(|| "calibration digest must be UTF-8".to_owned())?;
            let geometry = capture_calibration::parse_fractional_geometry(
                left_numerator,
                left_denominator,
                top_numerator,
                top_denominator,
                width_numerator,
                width_denominator,
                height_numerator,
                height_denominator,
            )?;
            let report = capture_calibration::author_gamescope_profile_binding(
                Path::new(calibration),
                expected_digest,
                Path::new(output),
                geometry,
            );
            println!(
                "{}",
                serde_json::to_string(&report)
                    .map_err(|_| "binding author report serialization failed".to_owned())?
            );
            report
                .succeeded()
                .then_some(())
                .ok_or_else(|| "Gamescope profile binding author failed".to_owned())
        })
}

fn try_capture_session_calibration(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        capture,
        command,
        output_flag,
        output,
        environment_flag,
        environment,
        version_flag,
        version,
        backend_flag,
        backend,
        output_width_flag,
        output_width,
        output_height_flag,
        output_height,
        width_flag,
        width,
        height_flag,
        height,
        refresh_flag,
        refresh,
        scaler_flag,
        scaler,
        filter_flag,
        filter,
    ] = args
    else {
        return None;
    };
    (capture == "capture"
        && command == "gamescope-calibration-session-sample"
        && output_flag == "--output"
        && environment_flag == "--environment-id"
        && version_flag == "--gamescope-version"
        && backend_flag == "--backend"
        && output_width_flag == "--output-width"
        && output_height_flag == "--output-height"
        && width_flag == "--nested-width"
        && height_flag == "--nested-height"
        && refresh_flag == "--nested-refresh"
        && scaler_flag == "--scaler"
        && filter_flag == "--filter")
        .then(|| {
            let configuration = capture_calibration::parse_session_configuration(
                environment,
                version,
                backend,
                output_width,
                output_height,
                width,
                height,
                refresh,
                scaler,
                filter,
            )?;
            let report = capture_calibration::capture_gamescope_calibration_session_sample(
                Path::new(output),
                &configuration,
            );
            println!(
                "{}",
                serde_json::to_string(&report).map_err(|_| {
                    "Gamescope calibration session report serialization failed".to_owned()
                })?
            );
            report
                .succeeded()
                .then_some(())
                .ok_or_else(|| "Gamescope calibration session sample failed".to_owned())
        })
}

fn try_capture_calibration(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        capture,
        command,
        output_flag,
        output,
        width_flag,
        width,
        height_flag,
        height,
        refresh_flag,
        refresh,
        scaler_flag,
        scaler,
        filter_flag,
        filter,
    ] = args
    else {
        return None;
    };
    (capture == "capture"
        && command == "gamescope-calibration-sample"
        && output_flag == "--output"
        && width_flag == "--nested-width"
        && height_flag == "--nested-height"
        && refresh_flag == "--nested-refresh"
        && scaler_flag == "--scaler"
        && filter_flag == "--filter")
        .then(|| {
            let configuration = capture_calibration::parse_scaling_configuration(
                width, height, refresh, scaler, filter,
            )?;
            let report = capture_calibration::capture_gamescope_calibration_sample(
                Path::new(output),
                configuration,
            );
            println!(
                "{}",
                serde_json::to_string(&report).map_err(|_| {
                    "Gamescope calibration sample report serialization failed".to_owned()
                })?
            );
            report
                .succeeded()
                .then_some(())
                .ok_or_else(|| "Gamescope calibration sample failed".to_owned())
        })
}

fn try_capture_live_gate(args: &[OsString]) -> Option<Result<(), String>> {
    match args {
        [capture, command, duration_flag, duration]
            if capture == "capture"
                && command == "gamescope-live-gate"
                && duration_flag == "--duration-ms" =>
        {
            Some((|| {
                let duration_ms = capture_live::parse_duration_ms(duration)?;
                let report = capture_live::run_gamescope_live_gate(duration_ms);
                print_capture_gate_report(&report)
            })())
        }
        [
            capture,
            command,
            duration_flag,
            duration,
            interval_flag,
            interval,
        ] if capture == "capture"
            && command == "gamescope-live-gate"
            && duration_flag == "--duration-ms"
            && interval_flag == "--consume-interval-ms" =>
        {
            Some((|| {
                let duration_ms = capture_live::parse_duration_ms(duration)?;
                let consumer_interval_ms = capture_live::parse_consumer_interval_ms(interval)?;
                let report = capture_live::run_gamescope_live_gate_with_interval(
                    duration_ms,
                    consumer_interval_ms,
                );
                print_capture_gate_report(&report)
            })())
        }
        [
            capture,
            command,
            duration_flag,
            duration,
            runs_flag,
            runs,
            interval_flag,
            interval,
        ] if capture == "capture"
            && command == "gamescope-lifecycle-gate"
            && duration_flag == "--duration-ms"
            && runs_flag == "--runs"
            && interval_flag == "--consume-interval-ms" =>
        {
            Some((|| {
                let duration_ms = capture_live::parse_duration_ms(duration)?;
                let runs = capture_live::parse_lifecycle_runs(runs)?;
                let consumer_interval_ms = capture_live::parse_consumer_interval_ms(interval)?;
                let report = capture_live::run_gamescope_lifecycle_gate(
                    duration_ms,
                    runs,
                    consumer_interval_ms,
                );
                println!(
                    "{}",
                    serde_json::to_string(&report).map_err(|_| {
                        "Gamescope lifecycle gate report serialization failed".to_owned()
                    })?
                );
                report
                    .succeeded()
                    .then_some(())
                    .ok_or_else(|| "Gamescope lifecycle capture gate failed".to_owned())
            })())
        }
        _ => None,
    }
}

fn print_capture_gate_report(report: &capture_live::GamescopeLiveGateReport) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(&report)
            .map_err(|_| "capture live gate report serialization failed".to_owned())?
    );
    report
        .succeeded()
        .then_some(())
        .ok_or_else(|| "Gamescope live capture gate failed".to_owned())
}

fn try_program_information(args: &[OsString]) -> Option<Result<(), String>> {
    match args {
        [flag] if flag == "--help" || flag == "-h" => {
            print_usage();
            Some(Ok(()))
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("scorepeek {}", env!("CARGO_PKG_VERSION"));
            Some(Ok(()))
        }
        _ => None,
    }
}

fn try_offline_program_information(args: &[OsString]) -> Option<Result<(), String>> {
    match args {
        [flag] if flag == "--help" => {
            print_usage();
            Some(Ok(()))
        }
        [flag] if flag == "--version" => {
            println!("scorepeek {}", env!("CARGO_PKG_VERSION"));
            Some(Ok(()))
        }
        _ => None,
    }
}

fn try_diagnostic_control(args: &[OsString]) -> Option<Result<(), String>> {
    match args {
        [diagnostic, command, root_flag, root]
            if diagnostic == "diagnostic" && root_flag == "--root" =>
        {
            match command.to_str() {
                Some("status") => Some(print_diagnostic_summary(
                    diagnostic_control::diagnostic_store_status(Path::new(root)),
                )),
                Some("list") => Some(print_diagnostic_summary(
                    diagnostic_control::diagnostic_run_list(Path::new(root)),
                )),
                _ => None,
            }
        }
        [
            diagnostic,
            command,
            root_flag,
            root,
            run_id_flag,
            run_id,
            run_digest_flag,
            run_digest,
            manifest_flag,
            manifest_digest,
        ] if diagnostic == "diagnostic"
            && (command == "freeze" || command == "delete")
            && root_flag == "--root"
            && run_id_flag == "--run-id"
            && run_digest_flag == "--run-sha256"
            && manifest_flag == "--manifest-sha256" =>
        {
            Some((|| {
                let run_id = utf8_control_value(run_id, "run ID")?;
                let run_digest = utf8_control_value(run_digest, "run digest")?;
                let manifest_digest = utf8_control_value(manifest_digest, "manifest digest")?;
                let manifest_digest = (manifest_digest != "none").then_some(manifest_digest);
                if command == "freeze" {
                    print_diagnostic_summary(diagnostic_control::diagnostic_freeze(
                        Path::new(root),
                        run_id,
                        run_digest,
                        manifest_digest,
                    ))
                } else {
                    print_diagnostic_summary(diagnostic_control::diagnostic_delete(
                        Path::new(root),
                        run_id,
                        run_digest,
                        manifest_digest,
                    ))
                }
            })())
        }
        [
            diagnostic,
            export,
            root_flag,
            root,
            run_id_flag,
            run_id,
            run_digest_flag,
            run_digest,
            manifest_flag,
            manifest_digest,
            destination_flag,
            destination,
        ] if diagnostic == "diagnostic"
            && export == "export"
            && root_flag == "--root"
            && run_id_flag == "--run-id"
            && run_digest_flag == "--run-sha256"
            && manifest_flag == "--manifest-sha256"
            && destination_flag == "--destination" =>
        {
            Some((|| {
                print_diagnostic_summary(diagnostic_control::diagnostic_export(
                    Path::new(root),
                    utf8_control_value(run_id, "run ID")?,
                    utf8_control_value(run_digest, "run digest")?,
                    utf8_control_value(manifest_digest, "manifest digest")?,
                    Path::new(destination),
                ))
            })())
        }
        _ => None,
    }
}

fn utf8_control_value<'a>(value: &'a OsStr, label: &str) -> Result<&'a str, String> {
    value
        .to_str()
        .ok_or_else(|| format!("diagnostic control {label} must be UTF-8"))
}

fn print_diagnostic_summary<T: Serialize>(summary: Result<T, String>) -> Result<(), String> {
    let summary = summary?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|_| "diagnostic control summary serialization failed".to_owned())?
    );
    Ok(())
}

fn try_diagnostic_replay(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        diagnostic,
        replay,
        request_flag,
        request,
        digest_flag,
        digest,
        extraction_flag,
        extraction,
        output_flag,
        output,
    ] = args
    else {
        return None;
    };
    (diagnostic == "diagnostic"
        && replay == "replay"
        && request_flag == "--request"
        && digest_flag == "--request-sha256"
        && extraction_flag == "--extraction"
        && output_flag == "--output-root")
        .then(|| {
            let digest = digest
                .to_str()
                .ok_or_else(|| "diagnostic replay request digest must be UTF-8".to_owned())?;
            let summary = diagnostic_replay::replay_diagnostic_run(
                Path::new(request),
                digest,
                Path::new(extraction),
                Path::new(output),
            )?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|_| "diagnostic replay summary serialization failed".to_owned())?
            );
            Ok(())
        })
}

fn try_diagnostic_reevaluation(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let [
        diagnostic,
        reevaluate,
        session_flag,
        session,
        digest_flag,
        digest,
        output_flag,
        output,
    ] = args
    else {
        return None;
    };
    (diagnostic == "diagnostic"
        && reevaluate == "reevaluate"
        && session_flag == "--session"
        && digest_flag == "--session-sha256"
        && output_flag == "--output")
        .then(|| {
            let digest = digest
                .to_str()
                .ok_or_else(|| "diagnostic session digest must be UTF-8".to_owned())?;
            let (catalog_root, _) = catalog_paths(
                env::var_os("XDG_DATA_HOME").as_deref(),
                env::var_os("XDG_CACHE_HOME").as_deref(),
                env::var_os("HOME").as_deref(),
            )?;
            let summary = diagnostic_reevaluation::reevaluate(
                Path::new(session),
                digest,
                Path::new(output),
                &catalog_root,
                bundle,
                &current_executable_sha256()?,
            )?;
            println!(
                "{}",
                serde_json::to_string(&summary).map_err(|_| {
                    "diagnostic reevaluation summary serialization failed".to_owned()
                })?
            );
            Ok(())
        })
}

fn try_doctor(args: &[OsString]) -> Option<Result<(), String>> {
    matches!(args, [command] if command == "doctor").then(|| {
        let target_inventory: serde_json::Value =
            serde_json::from_str(&inventory::collect().to_json())
                .map_err(|error| format!("doctor report serialization failed: {error}"))?;
        let numeric_model = match scorepeek::numeric_model_store::active_registered(
            recognition::NUMERIC_MODEL_MANIFEST_BYTES,
            recognition::NUMERIC_MODEL_MANIFEST_SHA256,
        ) {
            Ok(runtime) => serde_json::json!({
                "status": "active",
                "model_id": runtime.contract().model_id,
                "model_sha256": runtime.contract().model_sha256,
                "manifest_sha256": recognition::NUMERIC_MODEL_MANIFEST_SHA256,
                "preprocessor_id": runtime.contract().preprocessor_id,
            }),
            Err(error) => serde_json::json!({
                "status": "unavailable",
                "reason": error.to_string(),
                "registered_manifest_sha256": recognition::NUMERIC_MODEL_MANIFEST_SHA256,
            }),
        };
        let report = serde_json::json!({
            "schema": "scorepeek-doctor-v2",
            "target_inventory": target_inventory,
            "numeric_model": numeric_model,
        });
        println!(
            "{}",
            serde_json::to_string(&report)
                .map_err(|error| format!("doctor report serialization failed: {error}"))?
        );
        Ok(())
    })
}

fn try_numeric_model_install(args: &[OsString]) -> Option<Result<(), String>> {
    let [numeric_model, install, bundle_flag, bundle] = args else {
        return None;
    };
    if numeric_model != "numeric-model" || install != "install" || bundle_flag != "--bundle" {
        return None;
    }
    Some((|| {
        let source = Path::new(bundle)
            .canonicalize()
            .map_err(|error| format!("numeric model bundle cannot be resolved: {error}"))?;
        let installed = scorepeek::numeric_model_store::install_registered(
            &source,
            recognition::NUMERIC_MODEL_MANIFEST_BYTES,
            recognition::NUMERIC_MODEL_MANIFEST_SHA256,
        )
        .map_err(|error| format!("numeric model installation failed: {error}"))?;
        println!(
            "{}",
            serde_json::json!({
                "schema": "scorepeek-numeric-model-install-v1",
                "status": "active",
                "manifest_sha256": recognition::NUMERIC_MODEL_MANIFEST_SHA256,
                "object": installed,
            })
        );
        Ok(())
    })())
}

fn try_recording_simulation(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    try_recording_simulation_profile_author(args)
        .or_else(|| try_recording_recognition_evidence_run(args, bundle))
        .or_else(|| try_recording_simulation_run(args, bundle))
}

fn try_recording_recognition_evidence_run(
    args: &[OsString],
    bundle: &Path,
) -> Option<Result<(), String>> {
    let [
        recognition,
        simulate,
        profile_flag,
        profile,
        profile_digest_flag,
        profile_digest,
        extraction_flag,
        extraction,
        diagnostic_root_flag,
        diagnostic_root,
        catalog_store_flag,
        catalog_store,
        run_id_flag,
        run_id,
        build_digest_flag,
        build_digest,
        recording_flag,
        recording,
        artifact_flag,
        artifact,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && (simulate == "recording-recognition-evidence"
            || simulate == "recording-recognition-simulation")
        && profile_flag == "--profile"
        && profile_digest_flag == "--profile-sha256"
        && extraction_flag == "--extraction"
        && diagnostic_root_flag == "--diagnostic-root"
        && catalog_store_flag == "--catalog-store"
        && run_id_flag == "--run-id"
        && build_digest_flag == "--build-sha256"
        && recording_flag == "--recording"
        && artifact_flag == "--recognition-artifact")
        .then(|| {
            execute_recording_simulation(
                profile,
                profile_digest,
                extraction,
                diagnostic_root,
                catalog_store,
                bundle,
                run_id,
                build_digest,
                recording,
                Some(Path::new(artifact)),
                simulate == "recording-recognition-simulation",
            )
        })
}

fn try_recording_simulation_profile_author(args: &[OsString]) -> Option<Result<(), String>> {
    if let [
        recognition,
        author,
        candidate_flag,
        candidate,
        candidate_digest_flag,
        candidate_digest,
        recording_manifest_flag,
        recording_manifest,
        coverage_label_flag,
        coverage_label,
        extraction_flag,
        extraction,
        output_flag,
        output,
    ] = args
        && recognition == "recognition"
        && author == "recording-simulation-profile-author"
        && candidate_flag == "--candidate"
        && candidate_digest_flag == "--candidate-sha256"
        && recording_manifest_flag == "--recording-manifest"
        && coverage_label_flag == "--coverage-label"
        && extraction_flag == "--extraction"
        && output_flag == "--output"
    {
        return Some((|| {
            let candidate_digest = parse_cli_sha256(candidate_digest, "candidate SHA-256")?;
            let profile_digest = recording_simulation::author_recording_simulation_profile(
                Path::new(candidate),
                &candidate_digest,
                Path::new(recording_manifest),
                Path::new(extraction),
                Path::new(coverage_label),
                Path::new(output),
            )?;
            println!(
                "{}",
                serde_json::json!({
                    "schema": "scorepeek-recording-field-simulation-profile-author-report-v1",
                    "status": "success",
                    "profile_sha256": profile_digest,
                })
            );
            Ok(())
        })());
    }

    None
}

fn try_recording_simulation_run(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let [
        recognition,
        simulate,
        profile_flag,
        profile,
        profile_digest_flag,
        profile_digest,
        extraction_flag,
        extraction,
        diagnostic_root_flag,
        diagnostic_root,
        catalog_store_flag,
        catalog_store,
        run_id_flag,
        run_id,
        build_digest_flag,
        build_digest,
        recording_flag,
        recording,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && simulate == "recording-simulation"
        && profile_flag == "--profile"
        && profile_digest_flag == "--profile-sha256"
        && extraction_flag == "--extraction"
        && diagnostic_root_flag == "--diagnostic-root"
        && catalog_store_flag == "--catalog-store"
        && run_id_flag == "--run-id"
        && build_digest_flag == "--build-sha256"
        && recording_flag == "--recording")
        .then(|| {
            execute_recording_simulation(
                profile,
                profile_digest,
                extraction,
                diagnostic_root,
                catalog_store,
                bundle,
                run_id,
                build_digest,
                recording,
                None,
                false,
            )
        })
}

#[allow(clippy::too_many_arguments)]
fn execute_recording_simulation(
    profile: &OsStr,
    profile_digest: &OsStr,
    extraction: &OsStr,
    diagnostic_root: &OsStr,
    catalog_store: &OsStr,
    bundle: &Path,
    run_id: &OsStr,
    build_digest: &OsStr,
    recording: &OsStr,
    recognition_artifact_root: Option<&Path>,
    require_song_resolution: bool,
) -> Result<(), String> {
    let profile_digest = parse_cli_sha256(profile_digest, "profile SHA-256")?;
    let report = recording_simulation::run_recording_simulation(
        recording_simulation::RecordingSimulationRunConfig {
            profile_path: Path::new(profile),
            expected_profile_sha256: &profile_digest,
            extraction_directory: Path::new(extraction),
            diagnostic_root: Path::new(diagnostic_root),
            catalog_root: Path::new(catalog_store),
            bundle_root: bundle,
            run_id: parse_diagnostic_run_id(run_id)?,
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
            policy: parse_diagnostic_recording_policy(recording)?,
            recognition_artifact_root,
            require_song_resolution,
        },
    );
    let succeeded = report.succeeded();
    println!(
        "{}",
        serde_json::to_string(&report)
            .map_err(|_| "recording simulation report serialization failed".to_owned())?
    );
    succeeded
        .then_some(())
        .ok_or_else(|| "recording field simulation failed".to_owned())
}

fn try_integrated_context_crop(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        crop,
        extraction_flag,
        extraction,
        digest_flag,
        digest,
        frame_flag,
        frame_id,
        output_flag,
        output,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && crop == "integrated-context-crop"
        && extraction_flag == "--extraction"
        && digest_flag == "--extraction-sha256"
        && frame_flag == "--frame-id"
        && output_flag == "--output")
        .then(|| crop_integrated_context(extraction, digest, frame_id, output))
}

fn try_dynamic_official_onnx_decode(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        decode,
        model_id_flag,
        model_id,
        bundle_flag,
        bundle,
        request_flag,
        request,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && decode == "title-official-dynamic-onnx-decode"
        && model_id_flag == "--model-id"
        && bundle_flag == "--bundle"
        && request_flag == "--request")
        .then(|| {
            let summary = recognition::decode_dynamic_official_onnx_crops(
                &model_id.to_string_lossy(),
                Path::new(bundle),
                Path::new(request),
            )
            .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&summary).map_err(|error| format!(
                    "dynamic official ONNX decode summary failed: {error}"
                ))?
            );
            Ok(())
        })
}

fn try_integrated_context_observe(args: &[OsString], bundle: &Path) -> Option<Result<(), String>> {
    let [
        recognition,
        observe,
        crops_flag,
        crops,
        digest_flag,
        digest,
        output_flag,
        output,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && observe == "integrated-context-observe"
        && crops_flag == "--crop-artifact"
        && digest_flag == "--crop-artifact-sha256"
        && output_flag == "--output")
        .then(|| {
            let digest = digest
                .to_str()
                .ok_or_else(|| "crop artifact SHA-256 must be UTF-8".to_owned())?;
            let summary = recognition::observe_integrated_context(
                Path::new(crops),
                digest,
                recognition::LIVE_MODEL_ID,
                bundle,
                Path::new(output),
            )
            .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&summary).map_err(|error| format!(
                    "integrated context observation summary failed: {error}"
                ))?
            );
            Ok(())
        })
}

fn try_official_onnx_decode(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        decode,
        model_flag,
        model,
        dictionary_flag,
        dictionary,
        request_flag,
        request,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && decode == "title-official-onnx-decode"
        && model_flag == "--model"
        && dictionary_flag == "--dictionary"
        && request_flag == "--request")
        .then(|| {
            let summary = recognition::decode_official_onnx_crops(
                Path::new(model),
                Path::new(dictionary),
                Path::new(request),
            )
            .map_err(|error| error.to_string())?;
            println!(
                "{}",
                serde_json::to_string(&summary)
                    .map_err(|error| format!("official ONNX decode summary failed: {error}"))?
            );
            Ok(())
        })
}

fn try_provisional_title_candidates(args: &[OsString]) -> Option<Result<(), String>> {
    let [recognition, export, store_flag, store, output_flag, output] = args else {
        return None;
    };
    (recognition == "recognition"
        && export == "provisional-title-candidates"
        && store_flag == "--catalog-store"
        && output_flag == "--output")
        .then(|| provisional_title_candidates(store, output))
}

fn try_title_model_contract_parity(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        parity,
        model_flag,
        model,
        model_digest_flag,
        model_digest,
        reference_flag,
        reference,
        reference_digest_flag,
        reference_digest,
        dictionary_flag,
        dictionary,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && parity == "title-model-contract-parity"
        && model_flag == "--model"
        && model_digest_flag == "--model-sha256"
        && reference_flag == "--reference"
        && reference_digest_flag == "--reference-sha256"
        && dictionary_flag == "--dictionary")
        .then(|| {
            title_model_contract_parity([
                model,
                model_digest,
                reference,
                reference_digest,
                dictionary,
            ])
        })
}

fn try_title_model_export_requirements(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        export,
        store_flag,
        store,
        dictionary_flag,
        dictionary,
        output_flag,
        output,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && export == "title-model-export-requirements"
        && store_flag == "--catalog-store"
        && dictionary_flag == "--baseline-dictionary"
        && output_flag == "--output")
        .then(|| title_model_export_requirements(store, dictionary, output))
}

fn try_title_dictionary_audit(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        audit,
        store_flag,
        store,
        dictionary_flag,
        dictionary,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && audit == "title-dictionary-audit"
        && store_flag == "--catalog-store"
        && dictionary_flag == "--dictionary")
        .then(|| title_dictionary_audit(store, dictionary))
}

fn try_title_onnx_parity(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        parity,
        model_flag,
        model,
        reference_flag,
        reference,
        digest_flag,
        digest,
        crop_flag,
        crop,
        store_flag,
        store,
        dictionary_flag,
        dictionary,
        minimum_score_flag,
        minimum_score,
        minimum_margin_flag,
        minimum_margin,
    ] = args
    else {
        return None;
    };
    (recognition == "recognition"
        && parity == "title-onnx-parity"
        && model_flag == "--model"
        && reference_flag == "--reference"
        && digest_flag == "--reference-sha256"
        && crop_flag == "--crop-artifact"
        && store_flag == "--catalog-store"
        && dictionary_flag == "--dictionary"
        && minimum_score_flag == "--minimum-log-probability"
        && minimum_margin_flag == "--minimum-runner-up-margin")
        .then(|| {
            title_onnx_parity([
                model,
                reference,
                digest,
                crop,
                store,
                dictionary,
                minimum_score,
                minimum_margin,
            ])
        })
}

fn inspect_canonical_frame(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let snapshot = recognition::inspect(&frame).map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&snapshot)
            .map_err(|error| format!("recognition result encoding failed: {error}"))?
    );
    Ok(())
}

fn inspect_diagnostic_qoi(frame: &OsStr, expected_sha256: &OsStr) -> Result<(), String> {
    const MAX_DIAGNOSTIC_QOI_BYTES: u64 = 16 * 1024 * 1024;

    let path = Path::new(frame);
    let expected_sha256 = parse_cli_sha256(expected_sha256, "diagnostic QOI SHA-256")?;
    let metadata = path
        .metadata()
        .map_err(|_| "diagnostic QOI is unavailable".to_owned())?;
    if !path.is_absolute() || !metadata.is_file() || metadata.len() > MAX_DIAGNOSTIC_QOI_BYTES {
        return Err("diagnostic QOI must be a bounded absolute regular file".to_owned());
    }
    let encoded = fs::read(path).map_err(|_| "diagnostic QOI read failed".to_owned())?;
    if encode_sha256(&encoded) != expected_sha256 {
        return Err("diagnostic QOI digest mismatch".to_owned());
    }
    let (header, pixels) =
        qoi::decode_to_vec(encoded).map_err(|_| "diagnostic QOI decoding failed".to_owned())?;
    if header.width != 1_920 || header.height != 1_080 || pixels.len() != 1_920 * 1_080 * 3 {
        return Err("diagnostic QOI is not canonical RGB8 1920x1080".to_owned());
    }
    let observation =
        recognition::inspect_canonical_rgb8(&pixels).map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::json!({
            "schema": "scorepeek-diagnostic-qoi-recognition-inspection-v1",
            "frame_sha256": expected_sha256,
            "canonical_pixel_sha256": encode_sha256(&pixels),
            "canonical_layout_sha256": recognition::CanonicalLayout::sha256(),
            "observation": observation,
        })
    );
    Ok(())
}

fn crop_canonical_result(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let summary = recognition::export_result_crops(&frame, frame_id, output)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("crop export summary encoding failed: {error}"))?
    );
    Ok(())
}

fn crop_canonical_music_select(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let summary = recognition::export_music_select_crops(&frame, frame_id, output)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("crop export summary encoding failed: {error}"))?
    );
    Ok(())
}

fn crop_integrated_context(
    extraction: &OsStr,
    extraction_sha256: &OsStr,
    frame_id: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let extraction_sha256 = extraction_sha256
        .to_str()
        .ok_or_else(|| "canonical extraction SHA-256 must be UTF-8".to_owned())?;
    let frame_id = frame_id
        .to_str()
        .ok_or_else(|| "canonical frame ID must be UTF-8".to_owned())?;
    let frame = CanonicalFrame::read_extraction(extraction, frame_id, extraction_sha256)
        .map_err(|error| error.to_string())?;
    let summary = recognition::export_integrated_context_crops(&frame, frame_id, output)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("crop export summary encoding failed: {error}"))?
    );
    Ok(())
}

#[derive(Serialize)]
struct DiagnosticTitleSpikeSummary {
    schema: &'static str,
    catalog_sha256: String,
    comparison_key_id: &'static str,
    minimum_confidence: f64,
    candidate: recognition::DiagnosticTitleCandidate,
}

#[derive(Serialize)]
struct ProvisionalTitleCandidatesArtifact {
    schema: &'static str,
    catalog_sha256: String,
    #[serde(flatten)]
    candidates: recognition::ProvisionalTitleCandidateSet,
}

#[derive(Serialize)]
struct ProvisionalTitleCandidatesSummary {
    schema: &'static str,
    output: PathBuf,
    artifact_sha256: String,
    catalog_sha256: String,
    candidate_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrivatePublicationPoint {
    FileSynced,
    Linked,
    StagingRemoved,
}

fn publish_private_file(output: &Path, bytes: &[u8]) -> std::io::Result<()> {
    publish_private_file_with(output, bytes, |_| Ok(()))
}

fn publish_private_file_with(
    output: &Path,
    bytes: &[u8],
    mut checkpoint: impl FnMut(PrivatePublicationPoint) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = output.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no parent")
    })?;
    let mut staging = tempfile::Builder::new()
        .prefix(".scorepeek-private-staging-")
        .tempfile_in(parent)?;
    staging.as_file_mut().write_all(bytes)?;
    staging.as_file_mut().sync_all()?;
    checkpoint(PrivatePublicationPoint::FileSynced)?;

    let staging_path = staging.path().to_owned();
    let mut linked = false;
    let publication = (|| {
        fs::hard_link(&staging_path, output)?;
        linked = true;
        checkpoint(PrivatePublicationPoint::Linked)?;
        fs::remove_file(&staging_path)?;
        checkpoint(PrivatePublicationPoint::StagingRemoved)?;
        fs::File::open(parent)?.sync_all()
    })();
    if let Err(error) = publication {
        if linked {
            let _ = fs::remove_file(output);
        }
        let _ = fs::remove_file(&staging_path);
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
        return Err(error);
    }
    Ok(())
}

fn provisional_title_candidates(catalog_store: &OsStr, output: &OsStr) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let output = PathBuf::from(output);
    if !output.is_absolute() || output.as_os_str().is_empty() {
        return Err("provisional title candidate output must be an absolute path".to_owned());
    }
    let parent = output
        .parent()
        .ok_or_else(|| "provisional title candidate output must have a parent".to_owned())?;
    let metadata = parent.metadata().map_err(|error| {
        format!("provisional title candidate output parent inspection failed: {error}")
    })?;
    if !metadata.is_dir() {
        return Err(
            "provisional title candidate output parent must be a regular directory".to_owned(),
        );
    }
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let candidates = recognition::provisional_title_candidates(&active.catalog);
    let candidate_count = candidates.candidates.len();
    let artifact = ProvisionalTitleCandidatesArtifact {
        schema: "scorepeek-private-provisional-title-candidates-v1",
        catalog_sha256: active.digest.clone(),
        candidates,
    };
    let mut bytes = serde_json::to_vec(&artifact)
        .map_err(|error| format!("provisional title candidate encoding failed: {error}"))?;
    bytes.push(b'\n');
    publish_private_file(&output, &bytes)
        .map_err(|error| format!("provisional title candidate publication failed: {error}"))?;
    let summary = ProvisionalTitleCandidatesSummary {
        schema: "scorepeek-private-provisional-title-candidates-summary-v1",
        output,
        artifact_sha256: encode_sha256(&bytes),
        catalog_sha256: active.digest,
        candidate_count,
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("provisional title candidate summary failed: {error}"))?
    );
    Ok(())
}

fn diagnostic_title_spike(
    catalog_store: &OsStr,
    ocr_text: &OsStr,
    ocr_confidence: &OsStr,
) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let ocr_text = ocr_text
        .to_str()
        .ok_or_else(|| "diagnostic OCR text must be UTF-8".to_owned())?;
    let ocr_confidence = ocr_confidence
        .to_str()
        .ok_or_else(|| "diagnostic OCR confidence must be UTF-8".to_owned())?
        .parse::<f64>()
        .map_err(|_| "diagnostic OCR confidence must be a decimal number".to_owned())?;
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let candidate =
        recognition::diagnostic_title_candidate(&active.catalog, ocr_text, ocr_confidence)
            .map_err(|error| error.to_string())?;
    let summary = DiagnosticTitleSpikeSummary {
        schema: "scorepeek-diagnostic-title-spike-v1",
        catalog_sha256: active.digest,
        comparison_key_id: DIAGNOSTIC_TITLE_COMPARISON_KEY_ID,
        minimum_confidence: DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE,
        candidate,
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("title spike result encoding failed: {error}"))?
    );
    Ok(())
}

#[derive(Serialize)]
struct TitleDictionaryAuditSummary {
    schema: &'static str,
    catalog_sha256: String,
    audit: recognition::CatalogTitleDictionaryAudit,
}

fn title_dictionary_audit(catalog_store: &OsStr, dictionary: &OsStr) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let audit = recognition::audit_catalog_title_dictionary(&active.catalog, Path::new(dictionary))
        .map_err(|error| error.to_string())?;
    let summary = TitleDictionaryAuditSummary {
        schema: "scorepeek-catalog-title-dictionary-audit-v1",
        catalog_sha256: active.digest,
        audit,
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("title dictionary audit encoding failed: {error}"))?
    );
    Ok(())
}

#[derive(Serialize)]
struct TitleModelExportRequirementsArtifact {
    schema: &'static str,
    catalog_sha256: String,
    requirements: recognition::TitleModelExportRequirements,
}

#[derive(Serialize)]
struct TitleModelExportRequirementsSummary {
    schema: &'static str,
    output: PathBuf,
    manifest_sha256: String,
    catalog_sha256: String,
    output_timesteps: usize,
    output_classes: usize,
    non_search_variant_count: usize,
}

fn title_model_export_requirements(
    catalog_store: &OsStr,
    dictionary: &OsStr,
    output: &OsStr,
) -> Result<(), String> {
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let output = absolute_directory(PathBuf::from(output), "model export requirements output")?;
    let parent = output
        .parent()
        .ok_or_else(|| "model export requirements output must have a parent".to_owned())?;
    let parent_metadata = parent
        .metadata()
        .map_err(|error| format!("model export requirements parent inspection failed: {error}"))?;
    if !parent_metadata.is_dir() {
        return Err("model export requirements parent must be a regular directory".to_owned());
    }
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let requirements =
        recognition::title_model_export_requirements(&active.catalog, Path::new(dictionary))
            .map_err(|error| error.to_string())?;
    let summary = TitleModelExportRequirementsSummary {
        schema: "scorepeek-title-model-export-requirements-summary-v1",
        output: output.clone(),
        manifest_sha256: String::new(),
        catalog_sha256: active.digest.clone(),
        output_timesteps: requirements.output_timesteps,
        output_classes: requirements.output_classes,
        non_search_variant_count: requirements.non_search_variant_count,
    };
    let artifact = TitleModelExportRequirementsArtifact {
        schema: "scorepeek-private-title-model-export-requirements-v1",
        catalog_sha256: active.digest,
        requirements,
    };
    let mut bytes = serde_json::to_vec(&artifact)
        .map_err(|error| format!("model export requirements encoding failed: {error}"))?;
    bytes.push(b'\n');
    DirBuilder::new()
        .mode(0o700)
        .create(&output)
        .map_err(|error| format!("model export requirements output creation failed: {error}"))?;
    let publication = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(output.join("manifest.json"))
            .map_err(|error| format!("model export requirements publication failed: {error}"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("model export requirements publication failed: {error}"))?;
        fs::File::open(&output)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("model export requirements sync failed: {error}"))
    })();
    if let Err(error) = publication {
        fs::remove_dir_all(&output)
            .map_err(|cleanup| format!("{error}; failed to remove incomplete output: {cleanup}"))?;
        return Err(error);
    }
    let summary = TitleModelExportRequirementsSummary {
        manifest_sha256: encode_sha256(&bytes),
        ..summary
    };
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("model export requirements summary failed: {error}"))?
    );
    Ok(())
}

fn encode_sha256(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn title_onnx_parity(arguments: [&OsStr; 8]) -> Result<(), String> {
    let [
        model,
        reference,
        reference_sha256,
        crop,
        catalog_store,
        dictionary,
        minimum_log_probability,
        minimum_runner_up_margin,
    ] = arguments;
    let reference_sha256 = reference_sha256
        .to_str()
        .ok_or_else(|| "parity reference SHA-256 must be UTF-8".to_owned())?;
    let catalog_store = absolute_directory(PathBuf::from(catalog_store), "catalog store")?;
    let active = CatalogStore::new(catalog_store)
        .load_active()
        .map_err(|error| format!("active catalog load failed: {error}"))?
        .ok_or_else(|| "catalog store has no active catalog".to_owned())?;
    let minimum_log_probability =
        parse_f64(minimum_log_probability, "minimum title log probability")?;
    let minimum_runner_up_margin =
        parse_f64(minimum_runner_up_margin, "minimum title runner-up margin")?;
    let thresholds = recognition::DiagnosticTitleThresholds {
        minimum_log_probability,
        minimum_runner_up_margin,
    };
    let request = recognition::OnnxTitleDiagnosticRequest {
        model_path: Path::new(model),
        reference_directory: Path::new(reference),
        reference_sha256,
        crop_directory: Path::new(crop),
        catalog_sha256: &active.digest,
        inference_yml: Path::new(dictionary),
    };
    let summary = recognition::compare_paddle_onnx(request, &active.catalog, thresholds)
        .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("ONNX parity summary encoding failed: {error}"))?
    );
    Ok(())
}

fn title_model_contract_parity(arguments: [&OsStr; 5]) -> Result<(), String> {
    let [model, model_sha256, reference, reference_sha256, dictionary] = arguments;
    let model_sha256 = model_sha256
        .to_str()
        .ok_or_else(|| "model SHA-256 must be UTF-8".to_owned())?;
    let reference_sha256 = reference_sha256
        .to_str()
        .ok_or_else(|| "parity reference SHA-256 must be UTF-8".to_owned())?;
    let request = recognition::ExportContractParityRequest {
        model_path: Path::new(model),
        model_sha256,
        reference_directory: Path::new(reference),
        reference_sha256,
        inference_yml: Path::new(dictionary),
    };
    let summary =
        recognition::compare_export_contract(request).map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string(&summary)
            .map_err(|error| format!("export contract parity encoding failed: {error}"))?
    );
    Ok(())
}

fn parse_f64(value: &OsStr, label: &str) -> Result<f64, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?
        .parse::<f64>()
        .map_err(|_| format!("{label} must be a decimal number"))
}

fn sync_catalog() -> Result<(), String> {
    let xdg_data_home = env::var_os("XDG_DATA_HOME");
    let xdg_cache_home = env::var_os("XDG_CACHE_HOME");
    let home = env::var_os("HOME");
    let (store_root, cache_root) = catalog_paths(
        xdg_data_home.as_deref(),
        xdg_cache_home.as_deref(),
        home.as_deref(),
    )?;
    let result = CatalogSync::new(store_root, cache_root)
        .sync()
        .map_err(|error| catalog_sync_error(&error))?;
    println!(
        "{}",
        serde_json::to_string(&result.into_summary())
            .map_err(|error| format!("catalog sync result encoding failed: {error}"))?
    );
    Ok(())
}

fn catalog_sync_error(error: &CatalogSyncError) -> String {
    format!("scorepeek catalog sync failed: {error}")
}

fn catalog_paths(
    xdg_data_home: Option<&OsStr>,
    xdg_cache_home: Option<&OsStr>,
    home: Option<&OsStr>,
) -> Result<(PathBuf, PathBuf), String> {
    let data = xdg_base_directory(xdg_data_home, home, ".local/share")?;
    let cache = xdg_base_directory(xdg_cache_home, home, ".cache")?;
    Ok((
        data.join("scorepeek/catalog"),
        cache.join("scorepeek/catalog/sources"),
    ))
}

fn xdg_base_directory(
    configured: Option<&OsStr>,
    home: Option<&OsStr>,
    fallback: &str,
) -> Result<PathBuf, String> {
    if let Some(configured) = configured {
        let path = PathBuf::from(configured);
        return absolute_directory(path, "XDG base directory");
    }
    let home =
        home.ok_or_else(|| "HOME is required when an XDG base directory is unset".to_owned())?;
    let home = absolute_directory(PathBuf::from(home), "HOME")?;
    Ok(home.join(fallback))
}

fn absolute_directory(path: PathBuf, name: &str) -> Result<PathBuf, String> {
    if path.as_os_str().is_empty() || !Path::new(&path).is_absolute() {
        return Err(format!("{name} must be an absolute, non-empty path"));
    }
    Ok(path)
}

fn print_usage() {
    println!(
        "scorepeek {}\n\nUsage:\n  scorepeek --help\n  scorepeek --version\n  scorepeek doctor\n  scorepeek [--model-bundle DIRECTORY] COMMAND ...\n  scorepeek setup gamescope --profile NAME -- GAMESCOPE_ARGS...\n  scorepeek profile list\n  scorepeek run [--profile NAME] [--record [--record-memory-mib MIB]]\n  scorepeek capture gamescope-live-gate --duration-ms MILLISECONDS [--consume-interval-ms MILLISECONDS]\n  scorepeek capture gamescope-lifecycle-gate --duration-ms MILLISECONDS --runs RUNS --consume-interval-ms MILLISECONDS\n  scorepeek capture gamescope-calibration-sample --output DIRECTORY --nested-width PIXELS --nested-height PIXELS --nested-refresh HZ --scaler SCALER --filter FILTER\n  scorepeek capture gamescope-calibration-session-sample --output DIRECTORY --environment-id ID --gamescope-version VERSION --backend BACKEND --output-width PIXELS --output-height PIXELS --nested-width PIXELS --nested-height PIXELS --nested-refresh HZ --scaler SCALER --filter FILTER\n  scorepeek capture gamescope-profile-binding-author --calibration DIRECTORY --calibration-sha256 SHA256 --output FILE --left-numerator N --left-denominator D --top-numerator N --top-denominator D --width-numerator N --width-denominator D --height-numerator N --height-denominator D\n  scorepeek capture gamescope-binding-admission-gate --binding FILE --binding-sha256 SHA256\n  scorepeek capture gamescope-canonical-frame-gate --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION\n  scorepeek catalog sync\n  scorepeek diagnostic status --root DIRECTORY\n  scorepeek diagnostic list --root DIRECTORY\n  scorepeek diagnostic freeze --root DIRECTORY --run-id RUN_ID --run-sha256 SHA256 --manifest-sha256 SHA256_OR_NONE\n  scorepeek diagnostic delete --root DIRECTORY --run-id RUN_ID --run-sha256 SHA256 --manifest-sha256 SHA256_OR_NONE\n  scorepeek diagnostic export --root DIRECTORY --run-id RUN_ID --run-sha256 SHA256 --manifest-sha256 SHA256 --destination DIRECTORY\n  scorepeek diagnostic replay --request FILE --request-sha256 SHA256 --extraction DIRECTORY --output-root DIRECTORY\n  scorepeek recognition inspect --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID\n  scorepeek recognition inspect-diagnostic-qoi --frame FILE --frame-sha256 SHA256\n  scorepeek recognition crop --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID --output DIRECTORY\n  scorepeek recognition music-select-crop --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID --output DIRECTORY\n  scorepeek recognition integrated-context-crop --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID --output DIRECTORY\n  scorepeek recognition integrated-context-observe --crop-artifact DIRECTORY --crop-artifact-sha256 SHA256 --output DIRECTORY\n  scorepeek recognition provisional-title-candidates --catalog-store DIRECTORY --output FILE\n  scorepeek recognition title-dictionary-audit --catalog-store DIRECTORY --dictionary FILE\n  scorepeek recognition title-model-export-requirements --catalog-store DIRECTORY --baseline-dictionary FILE --output DIRECTORY\n  scorepeek recognition title-spike --catalog-store DIRECTORY --ocr-text TEXT --ocr-confidence SCORE\n  scorepeek recognition title-official-onnx-decode --model FILE --dictionary FILE --request FILE\n  scorepeek recognition title-official-dynamic-onnx-decode --model-id MODEL_ID --bundle DIRECTORY --request FILE\n  scorepeek recognition title-onnx-parity --model FILE --reference DIRECTORY --reference-sha256 SHA256 --crop-artifact DIRECTORY --catalog-store DIRECTORY --dictionary FILE --minimum-log-probability SCORE --minimum-runner-up-margin SCORE\n  scorepeek recognition title-model-contract-parity --model FILE --model-sha256 SHA256 --reference DIRECTORY --reference-sha256 SHA256 --dictionary FILE",
        env!("CARGO_PKG_VERSION")
    );
    println!(
        "  scorepeek recognition field-resource-load-gate --catalog-store DIRECTORY --catalog-sha256 SHA256"
    );
    println!(
        "  scorepeek diagnostic reevaluate --session DIRECTORY --session-sha256 SHA256 --output DIRECTORY"
    );
    println!(
        "  scorepeek capture gamescope-diagnostic-handoff-gate --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION --duration-ms MILLISECONDS --diagnostic-root DIRECTORY --run-id RUN_ID --build-sha256 SHA256 --canonical-layout-sha256 SHA256 --catalog-sha256 SHA256 --recording enabled|disabled"
    );
    println!(
        "  scorepeek capture gamescope-recognition-handoff-gate --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION --duration-ms MILLISECONDS --diagnostic-root DIRECTORY --run-id RUN_ID --build-sha256 SHA256 --canonical-layout-sha256 SHA256 --catalog-sha256 SHA256 --recording enabled|disabled"
    );
    println!(
        "  scorepeek capture gamescope-field-observation-gate --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION --duration-ms MILLISECONDS --diagnostic-root DIRECTORY --catalog-store DIRECTORY --run-id RUN_ID --build-sha256 SHA256 --canonical-layout-sha256 SHA256 --catalog-sha256 SHA256 --recording enabled|disabled"
    );
    println!(
        "  scorepeek capture gamescope-result-recognition-gate --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION --duration-ms MILLISECONDS --diagnostic-root DIRECTORY --catalog-store DIRECTORY --run-id RUN_ID --build-sha256 SHA256 --canonical-layout-sha256 SHA256 --catalog-sha256 SHA256 --recording enabled|disabled --recognition-artifact DIRECTORY"
    );
    println!(
        "  scorepeek run gamescope --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION --diagnostic-root DIRECTORY --catalog-store DIRECTORY --run-id RUN_ID --build-sha256 SHA256 --canonical-layout-sha256 SHA256 --catalog-sha256 SHA256 --recording enabled|disabled --recognition-artifact DIRECTORY"
    );
    println!("  scorepeek numeric-model install --bundle DIRECTORY");
}

#[cfg(test)]
mod tests {
    use super::{
        CAPTURE_FIELD_OBSERVATION_FLAGS, CAPTURE_HANDOFF_FLAGS, CAPTURE_RESULT_RECOGNITION_FLAGS,
        LIVE_SESSION_FLAGS, LiveSessionEmission, PrivatePublicationPoint, catalog_paths,
        catalog_sync_error, command_flag_values, live_session_event_value,
        optional_recognition_root, parse_routine_run_options, prepare_live_diagnostic_root,
        publish_private_file, publish_private_file_with, run_event_from_live_emission,
        run_with_model_initializer,
    };
    use crate::capture_live::GamescopeLiveSessionEvent;
    use crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation;
    use scorepeek::catalog::{
        AdapterError, Catalog, CatalogStoreError, CatalogSyncError, DqnAcquisitionError,
        FederationInput, SourceRevision, TachiAcquisitionError, TachiFixtureAdapter, TachiResource,
        TextageAcquisitionError, TextageResource,
    };
    use scorepeek::recognition::{
        CatalogCandidateDomain, DynamicTextObservation, ResultScreenFieldObservations,
        ScreenFieldObservations,
    };
    use std::cell::Cell;
    use std::ffi::{OsStr, OsString};
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn unrecorded_run_disables_the_recognition_artifact_root() {
        let root = Path::new("/tmp/recognition");
        assert_eq!(optional_recognition_root(false, root), None);
        assert_eq!(optional_recognition_root(true, root), Some(root));
    }

    #[test]
    fn ordinary_run_options_are_order_independent_and_record_is_opt_in() {
        let empty: [OsString; 0] = [];
        let parsed = parse_routine_run_options(&empty).unwrap();
        assert_eq!(parsed.profile, None);
        assert!(!parsed.recording);
        let record = [OsString::from("--record")];
        let parsed = parse_routine_run_options(&record).unwrap();
        assert_eq!(parsed.profile, None);
        assert!(parsed.recording);
        assert_eq!(
            parsed.recording_memory_limit.bytes(),
            1024_u64 * 1024 * 1024
        );
        let profile_then_record = [
            OsString::from("--profile"),
            OsString::from("target"),
            OsString::from("--record"),
        ];
        let parsed = parse_routine_run_options(&profile_then_record).unwrap();
        assert_eq!(parsed.profile, Some(OsStr::new("target")));
        assert!(parsed.recording);
        let record_then_profile = [
            OsString::from("--record"),
            OsString::from("--profile"),
            OsString::from("target"),
        ];
        assert!(parse_routine_run_options(&record_then_profile).is_ok());
        let configured = [
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
    }

    #[test]
    fn removed_or_duplicate_recording_options_are_rejected() {
        for options in [
            vec![OsString::from("--no-recording")],
            vec![OsString::from("--record-attempts")],
            vec![OsString::from("--record"), OsString::from("--record")],
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
    fn numeric_model_install_skips_text_model_initialization() {
        let initialized = Cell::new(false);
        let error = run_with_model_initializer(
            &[
                OsString::from("numeric-model"),
                OsString::from("install"),
                OsString::from("--bundle"),
                OsString::from("/definitely/missing/numeric-model-bundle"),
            ],
            |_| {
                initialized.set(true);
                Err("must not initialize".to_owned())
            },
        )
        .unwrap_err();
        assert!(!initialized.get());
        assert!(error.starts_with("numeric model bundle cannot be resolved:"));
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
        let paths =
            catalog_paths(Some(OsStr::new("/data")), Some(OsStr::new("/cache")), None).unwrap();
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
    fn catalog_sync_errors_retain_actionable_causes() {
        let adapter = catalog_sync_error(&CatalogSyncError::DqnAcquisition(
            DqnAcquisitionError::Adapter(AdapterError::DuplicateRecord(
                "SOURCE SENTINEL".to_owned(),
            )),
        ));
        assert_eq!(
            adapter,
            "scorepeek catalog sync failed: dqn response validation failed: duplicate source record SOURCE SENTINEL"
        );

        let tachi = catalog_sync_error(&CatalogSyncError::TachiAcquisition(
            TachiAcquisitionError::Transport(TachiResource::Songs, "TRANSPORT SENTINEL".to_owned()),
        ));
        assert_eq!(
            tachi,
            "scorepeek catalog sync failed: Tachi songs seed acquisition failed: TRANSPORT SENTINEL"
        );

        let textage = catalog_sync_error(&CatalogSyncError::TextageAcquisition(
            TextageAcquisitionError::Transport(
                TextageResource::Title,
                "TEXTAGE SENTINEL".to_owned(),
            ),
        ));
        assert_eq!(
            textage,
            "scorepeek catalog sync failed: Textage title table transport failed: TEXTAGE SENTINEL"
        );

        let store = catalog_sync_error(&CatalogSyncError::Store(
            CatalogStoreError::InvalidSnapshot("SNAPSHOT SENTINEL".to_owned()),
        ));
        assert_eq!(
            store,
            "scorepeek catalog sync failed: invalid catalog snapshot: SNAPSHOT SENTINEL"
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
        let preflight = prepare_live_diagnostic_root(
            &root,
            &crate::diagnostic_recording::DiagnosticPolicy::default(),
        );
        assert_eq!(preflight.status, "ready");
        assert_eq!(preflight.error_type, None);
        assert!(root.is_dir());
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
                screen: scorepeek::recognition::ScreenClass::Unknown,
            },
        )
        .unwrap();
        assert_eq!(value["schema"], "scorepeek-run-event-v9");
        assert_eq!(value["event"], "raw_screen_observed");
        assert_eq!(value["semantic_episode_id"], 1);
        assert_eq!(value["session_id"], "invocation-session-2");
        assert_eq!(value["capture_generation"], 2);
        assert_eq!(value["sequence"], 41);
        assert_eq!(value["screen"], "unknown");

        let mode = live_session_event_value(
            Some("invocation-session-2"),
            Some(2),
            GamescopeLiveSessionEvent::SemanticScreenEpisode {
                screen_episode_id: 1,
                sequence: 42,
                monotonic_end_ms: 150,
                screen: scorepeek::recognition::ScreenClass::ModeSelect,
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
        let output = RegisteredScreenFieldObservation::from_fields(
            &domain,
            ScreenFieldObservations::Result(ResultScreenFieldObservations {
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
            None,
            None,
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
        assert_eq!(value["fields"]["title"], "TITLE EXACT");
        assert_eq!(value["fields"]["artist"], "ARTIST EXACT");
        assert_eq!(value["fields"]["clear_type"], "FAILED");
        assert_eq!(value["fields"]["play_type"], "SP");
        assert_eq!(
            value["result_song_resolution"]["reason"],
            "no_catalog_candidates"
        );
    }

    #[test]
    fn routine_observation_binds_session_and_generation() {
        let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
        let output = RegisteredScreenFieldObservation::from_fields(
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
        assert_eq!(value["schema"], "scorepeek-run-event-v9");
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
        let output = RegisteredScreenFieldObservation::from_fields_with_catalog(
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
            value,
            authority_joint_evidence: Some(authority.clone()),
        })
        .unwrap();
        let crate::routine_output::RunEventKind::FieldObservation { joint_evidence, .. } =
            event.kind
        else {
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
        let output = RegisteredScreenFieldObservation::from_fields(
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

    fn catalog_from_records(records: &[serde_json::Value]) -> Catalog {
        let bytes = serde_json::to_vec(&serde_json::json!({
            "schema": "scorepeek-tachi-fixture-v1",
            "records": records,
        }))
        .unwrap();
        let snapshot = TachiFixtureAdapter::parse(
            &bytes,
            SourceRevision::git_commit("0123456789abcdef0123456789abcdef01234567").unwrap(),
        )
        .unwrap();
        Catalog::default()
            .federate(FederationInput {
                tachi: Some(snapshot),
                ..FederationInput::default()
            })
            .catalog
    }

    fn tachi_record(id: &str, title: &str, artist: &str) -> serde_json::Value {
        serde_json::json!({
            "source_song_id": id,
            "title": title,
            "title_kind": "in_game_display",
            "artist": artist,
            "version": "SYNTHETIC",
            "charts": [{
                "play_type": "single",
                "difficulty": "normal",
                "level": 1,
                "notes": 1,
                "source_chart_id": "spn",
                "product_versions": ["synthetic-v1"],
                "primary": true
            }],
            "primary_infinitas": true
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
}
