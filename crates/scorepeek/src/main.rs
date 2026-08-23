mod canonical_source;
mod capture_calibration;
mod capture_live;
pub mod diagnostic_control;
pub mod diagnostic_live;
pub mod diagnostic_recording;
pub mod diagnostic_replay;
pub mod diagnostic_worker;
mod inventory;
pub mod recognition_live;
mod recording_simulation;

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write as _;
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

fn run(args: &[OsString]) -> Result<(), String> {
    if let Some(result) = try_diagnostic_control(args)
        .or_else(|| try_diagnostic_replay(args))
        .or_else(|| try_recording_simulation(args))
        .or_else(|| try_capture_commands(args))
        .or_else(|| try_doctor(args))
        .or_else(|| try_provisional_title_candidates(args))
        .or_else(|| try_integrated_context_crop(args))
        .or_else(|| try_integrated_context_observe(args))
        .or_else(|| try_registered_resource_gate(args))
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

fn try_registered_resource_gate(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition_command,
        gate,
        catalog_flag,
        catalog_root,
        bundle_flag,
        bundle_root,
        catalog_digest_flag,
        catalog_digest,
        model_digest_flag,
        model_digest,
        runtime_digest_flag,
        runtime_digest,
    ] = args
    else {
        return None;
    };
    if recognition_command != "recognition"
        || gate != "field-resource-load-gate"
        || catalog_flag != "--catalog-store"
        || bundle_flag != "--bundle"
        || catalog_digest_flag != "--catalog-sha256"
        || model_digest_flag != "--model-sha256"
        || runtime_digest_flag != "--runtime-sha256"
    {
        return None;
    }
    Some(registered_resource_gate(
        catalog_root,
        bundle_root,
        catalog_digest,
        model_digest,
        runtime_digest,
    ))
}

fn registered_resource_gate(
    catalog_root: &OsStr,
    bundle_root: &OsStr,
    catalog_digest: &OsStr,
    model_digest: &OsStr,
    runtime_digest: &OsStr,
) -> Result<(), String> {
    let catalog_digest = parse_cli_sha256(catalog_digest, "catalog SHA-256")?;
    let model_digest = parse_cli_sha256(model_digest, "model SHA-256")?;
    let runtime_digest = parse_cli_sha256(runtime_digest, "runtime SHA-256")?;
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
                .load_registered_resources(Path::new(catalog_root), Path::new(bundle_root))
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

fn try_capture_commands(args: &[OsString]) -> Option<Result<(), String>> {
    try_capture_field_observation(args)
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
    "--model-sha256",
    "--runtime-sha256",
    "--recording",
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
];

const CAPTURE_FIELD_OBSERVATION_FLAGS: &[&str] = &[
    "--binding",
    "--binding-sha256",
    "--capture-generation",
    "--duration-ms",
    "--diagnostic-root",
    "--catalog-store",
    "--bundle",
    "--run-id",
    "--build-sha256",
    "--canonical-layout-sha256",
    "--catalog-sha256",
    "--model-sha256",
    "--runtime-sha256",
    "--recording",
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
];

fn try_capture_field_observation(args: &[OsString]) -> Option<Result<(), String>> {
    let values = capture_flag_values(
        args,
        "gamescope-field-observation-gate",
        CAPTURE_FIELD_OBSERVATION_FLAGS,
    )?;
    Some(run_capture_field_observation(&values))
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
    if args.first()? != "capture" || args.get(1)? != command || args.len() != 2 + flags.len() * 2 {
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
        model_digest,
        runtime_digest,
        recording,
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
    ] = values
    else {
        unreachable!("capture flag parser returns the exact value count");
    };
    let binding_digest = parse_cli_sha256(binding_digest, "binding SHA-256")?;
    let generation = parse_capture_generation(generation)?;
    let duration_ms = capture_live::parse_duration_ms(duration)?;
    let run_id = parse_diagnostic_run_id(run_id)?;
    let policy = parse_diagnostic_recording_policy(recording)?;
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
            model_sha256: parse_cli_sha256(model_digest, "model SHA-256")?,
            runtime_sha256: parse_cli_sha256(runtime_digest, "runtime SHA-256")?,
            replay: None,
        },
    };
    let config = capture_live::GamescopeDiagnosticHandoffGateConfig {
        binding_path: Path::new(binding),
        expected_binding_sha256: &binding_digest,
        session: configuration.capture_provenance()?,
        capture_generation: generation,
        descriptor,
        policy,
        duration_ms,
        diagnostic_root: Path::new(diagnostic_root),
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

fn run_capture_field_observation(values: &[&OsStr]) -> Result<(), String> {
    let [
        binding,
        binding_digest,
        generation,
        duration,
        diagnostic_root,
        catalog_root,
        bundle_root,
        run_id,
        build_digest,
        layout_digest,
        catalog_digest,
        model_digest,
        runtime_digest,
        recording,
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
    ] = values
    else {
        unreachable!("capture flag parser returns the exact value count");
    };
    let binding_digest = parse_cli_sha256(binding_digest, "binding SHA-256")?;
    let generation = parse_capture_generation(generation)?;
    let duration_ms = capture_live::parse_duration_ms(duration)?;
    let run_id = parse_diagnostic_run_id(run_id)?;
    let policy = parse_diagnostic_recording_policy(recording)?;
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
            model_sha256: parse_cli_sha256(model_digest, "model SHA-256")?,
            runtime_sha256: parse_cli_sha256(runtime_digest, "runtime SHA-256")?,
            replay: None,
        },
    };
    let handoff = capture_live::GamescopeDiagnosticHandoffGateConfig {
        binding_path: Path::new(binding),
        expected_binding_sha256: &binding_digest,
        session: configuration.capture_provenance()?,
        capture_generation: generation,
        descriptor,
        policy,
        duration_ms,
        diagnostic_root: Path::new(diagnostic_root),
    };
    let report = capture_live::run_gamescope_field_observation_gate(
        capture_live::GamescopeFieldObservationGateConfig {
            handoff,
            catalog_root: Path::new(catalog_root),
            bundle_root: Path::new(bundle_root),
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
            .unwrap_or("Gamescope field observation gate failed")
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
        && command == "gamescope-canonical-frame-gate"
        && binding_flag == "--binding"
        && binding_digest_flag == "--binding-sha256"
        && generation_flag == "--capture-generation"
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
            let report = capture_live::run_gamescope_canonical_frame_gate(
                Path::new(binding),
                expected_digest,
                configuration.capture_provenance()?,
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
        && command == "gamescope-binding-admission-gate"
        && binding_flag == "--binding"
        && binding_digest_flag == "--binding-sha256"
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
            let expected_digest = binding_digest
                .to_str()
                .ok_or_else(|| "binding digest must be UTF-8".to_owned())?;
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
            let report = capture_live::run_gamescope_binding_admission_gate(
                Path::new(binding),
                expected_digest,
                configuration.capture_provenance()?,
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

fn try_doctor(args: &[OsString]) -> Option<Result<(), String>> {
    matches!(args, [command] if command == "doctor").then(|| {
        println!("{}", inventory::collect().to_json());
        Ok(())
    })
}

fn try_recording_simulation(args: &[OsString]) -> Option<Result<(), String>> {
    try_recording_simulation_profile_author(args).or_else(|| try_recording_simulation_run(args))
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

fn try_recording_simulation_run(args: &[OsString]) -> Option<Result<(), String>> {
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
        bundle_flag,
        bundle,
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
        && bundle_flag == "--bundle"
        && run_id_flag == "--run-id"
        && build_digest_flag == "--build-sha256"
        && recording_flag == "--recording")
        .then(|| {
            let report = recording_simulation::run_recording_simulation(
                recording_simulation::RecordingSimulationRunConfig {
                    profile_path: Path::new(profile),
                    expected_profile_sha256: &parse_cli_sha256(profile_digest, "profile SHA-256")?,
                    extraction_directory: Path::new(extraction),
                    diagnostic_root: Path::new(diagnostic_root),
                    catalog_root: Path::new(catalog_store),
                    bundle_root: Path::new(bundle),
                    run_id: parse_diagnostic_run_id(run_id)?,
                    build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
                    policy: parse_diagnostic_recording_policy(recording)?,
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
        })
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

fn try_integrated_context_observe(args: &[OsString]) -> Option<Result<(), String>> {
    let [
        recognition,
        observe,
        crops_flag,
        crops,
        digest_flag,
        digest,
        model_id_flag,
        model_id,
        bundle_flag,
        bundle,
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
        && model_id_flag == "--model-id"
        && bundle_flag == "--bundle"
        && output_flag == "--output")
        .then(|| {
            let digest = digest
                .to_str()
                .ok_or_else(|| "crop artifact SHA-256 must be UTF-8".to_owned())?;
            let model_id = model_id
                .to_str()
                .ok_or_else(|| "model ID must be UTF-8".to_owned())?;
            let summary = recognition::observe_integrated_context(
                Path::new(crops),
                digest,
                model_id,
                Path::new(bundle),
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
    let metadata = parent.symlink_metadata().map_err(|error| {
        format!("provisional title candidate output parent inspection failed: {error}")
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
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
        .symlink_metadata()
        .map_err(|error| format!("model export requirements parent inspection failed: {error}"))?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
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
        "scorepeek {}\n\nUsage:\n  scorepeek doctor\n  scorepeek capture gamescope-live-gate --duration-ms MILLISECONDS [--consume-interval-ms MILLISECONDS]\n  scorepeek capture gamescope-lifecycle-gate --duration-ms MILLISECONDS --runs RUNS --consume-interval-ms MILLISECONDS\n  scorepeek capture gamescope-calibration-sample --output DIRECTORY --nested-width PIXELS --nested-height PIXELS --nested-refresh HZ --scaler SCALER --filter FILTER\n  scorepeek capture gamescope-calibration-session-sample --output DIRECTORY --environment-id ID --gamescope-version VERSION --backend BACKEND --output-width PIXELS --output-height PIXELS --nested-width PIXELS --nested-height PIXELS --nested-refresh HZ --scaler SCALER --filter FILTER\n  scorepeek capture gamescope-profile-binding-author --calibration DIRECTORY --calibration-sha256 SHA256 --output FILE --left-numerator N --left-denominator D --top-numerator N --top-denominator D --width-numerator N --width-denominator D --height-numerator N --height-denominator D\n  scorepeek capture gamescope-binding-admission-gate --binding FILE --binding-sha256 SHA256 --environment-id ID --gamescope-version VERSION --backend BACKEND --output-width PIXELS --output-height PIXELS --nested-width PIXELS --nested-height PIXELS --nested-refresh HZ --scaler SCALER --filter FILTER\n  scorepeek capture gamescope-canonical-frame-gate --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION --environment-id ID --gamescope-version VERSION --backend BACKEND --output-width PIXELS --output-height PIXELS --nested-width PIXELS --nested-height PIXELS --nested-refresh HZ --scaler SCALER --filter FILTER\n  scorepeek catalog sync\n  scorepeek diagnostic status --root DIRECTORY\n  scorepeek diagnostic list --root DIRECTORY\n  scorepeek diagnostic freeze --root DIRECTORY --run-id RUN_ID --run-sha256 SHA256 --manifest-sha256 SHA256_OR_NONE\n  scorepeek diagnostic delete --root DIRECTORY --run-id RUN_ID --run-sha256 SHA256 --manifest-sha256 SHA256_OR_NONE\n  scorepeek diagnostic export --root DIRECTORY --run-id RUN_ID --run-sha256 SHA256 --manifest-sha256 SHA256 --destination DIRECTORY\n  scorepeek diagnostic replay --request FILE --request-sha256 SHA256 --extraction DIRECTORY --output-root DIRECTORY\n  scorepeek recognition inspect --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID\n  scorepeek recognition crop --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID --output DIRECTORY\n  scorepeek recognition music-select-crop --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID --output DIRECTORY\n  scorepeek recognition integrated-context-crop --extraction DIRECTORY --extraction-sha256 SHA256 --frame-id FRAME_ID --output DIRECTORY\n  scorepeek recognition integrated-context-observe --crop-artifact DIRECTORY --crop-artifact-sha256 SHA256 --model-id MODEL_ID --bundle DIRECTORY --output DIRECTORY\n  scorepeek recognition provisional-title-candidates --catalog-store DIRECTORY --output FILE\n  scorepeek recognition title-dictionary-audit --catalog-store DIRECTORY --dictionary FILE\n  scorepeek recognition title-model-export-requirements --catalog-store DIRECTORY --baseline-dictionary FILE --output DIRECTORY\n  scorepeek recognition title-spike --catalog-store DIRECTORY --ocr-text TEXT --ocr-confidence SCORE\n  scorepeek recognition title-official-onnx-decode --model FILE --dictionary FILE --request FILE\n  scorepeek recognition title-official-dynamic-onnx-decode --model-id MODEL_ID --bundle DIRECTORY --request FILE\n  scorepeek recognition title-onnx-parity --model FILE --reference DIRECTORY --reference-sha256 SHA256 --crop-artifact DIRECTORY --catalog-store DIRECTORY --dictionary FILE --minimum-log-probability SCORE --minimum-runner-up-margin SCORE\n  scorepeek recognition title-model-contract-parity --model FILE --model-sha256 SHA256 --reference DIRECTORY --reference-sha256 SHA256 --dictionary FILE",
        env!("CARGO_PKG_VERSION")
    );
    println!(
        "  scorepeek recognition field-resource-load-gate --catalog-store DIRECTORY --bundle DIRECTORY --catalog-sha256 SHA256 --model-sha256 SHA256 --runtime-sha256 SHA256"
    );
    println!(
        "  scorepeek capture gamescope-diagnostic-handoff-gate --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION --duration-ms MILLISECONDS --diagnostic-root DIRECTORY --run-id RUN_ID --build-sha256 SHA256 --canonical-layout-sha256 SHA256 --catalog-sha256 SHA256 --model-sha256 SHA256 --runtime-sha256 SHA256 --recording enabled|disabled --environment-id ID --gamescope-version VERSION --backend BACKEND --output-width PIXELS --output-height PIXELS --nested-width PIXELS --nested-height PIXELS --nested-refresh HZ --scaler SCALER --filter FILTER"
    );
    println!(
        "  scorepeek capture gamescope-recognition-handoff-gate --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION --duration-ms MILLISECONDS --diagnostic-root DIRECTORY --run-id RUN_ID --build-sha256 SHA256 --canonical-layout-sha256 SHA256 --catalog-sha256 SHA256 --model-sha256 SHA256 --runtime-sha256 SHA256 --recording enabled|disabled --environment-id ID --gamescope-version VERSION --backend BACKEND --output-width PIXELS --output-height PIXELS --nested-width PIXELS --nested-height PIXELS --nested-refresh HZ --scaler SCALER --filter FILTER"
    );
    println!(
        "  scorepeek capture gamescope-field-observation-gate --binding FILE --binding-sha256 SHA256 --capture-generation GENERATION --duration-ms MILLISECONDS --diagnostic-root DIRECTORY --catalog-store DIRECTORY --bundle DIRECTORY --run-id RUN_ID --build-sha256 SHA256 --canonical-layout-sha256 SHA256 --catalog-sha256 SHA256 --model-sha256 SHA256 --runtime-sha256 SHA256 --recording enabled|disabled --environment-id ID --gamescope-version VERSION --backend BACKEND --output-width PIXELS --output-height PIXELS --nested-width PIXELS --nested-height PIXELS --nested-refresh HZ --scaler SCALER --filter FILTER"
    );
}

#[cfg(test)]
mod tests {
    use super::{
        PrivatePublicationPoint, catalog_paths, catalog_sync_error, publish_private_file,
        publish_private_file_with,
    };
    use scorepeek::catalog::{
        AdapterError, CatalogStoreError, CatalogSyncError, DqnAcquisitionError,
        TachiAcquisitionError, TachiResource, TextageAcquisitionError, TextageResource,
    };
    use std::ffi::OsStr;
    use std::fs;
    use std::path::PathBuf;

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
}
