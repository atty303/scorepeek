use super::*;

#[cfg(test)]
pub(super) fn run_capture_handoff(values: &[&OsStr], inspect_screen: bool) -> Result<(), String> {
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
    let descriptor = DiagnosticRunDescriptor {
        run_id,
        monotonic_start_ms: 0,
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
        },
        binding: DiagnosticBinding {
            capture_generation: generation.get(),
            capture_profile_sha256: String::new(),
            normalizer_sha256: String::new(),
            canonical_layout_sha256: parse_cli_sha256(layout_digest, "canonical layout SHA-256")?,
            catalog_sha256: parse_cli_sha256(catalog_digest, "catalog SHA-256")?,
            model_sha256: recognition_title::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: recognition_title::LIVE_RUNTIME_SHA256.to_owned(),
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

#[cfg(test)]
pub(super) fn run_capture_field_observation(
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
    let descriptor = DiagnosticRunDescriptor {
        run_id,
        monotonic_start_ms: 0,
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: parse_cli_sha256(build_digest, "build SHA-256")?,
        },
        binding: DiagnosticBinding {
            capture_generation: generation.get(),
            capture_profile_sha256: String::new(),
            normalizer_sha256: String::new(),
            canonical_layout_sha256: parse_cli_sha256(layout_digest, "canonical layout SHA-256")?,
            catalog_sha256: parse_cli_sha256(catalog_digest, "catalog SHA-256")?,
            model_sha256: recognition_title::LIVE_MODEL_SHA256.to_owned(),
            runtime_sha256: recognition_title::LIVE_RUNTIME_SHA256.to_owned(),
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
            canonical_recording_root: None,
            recognition_artifact_retention:
                recognition_artifact::RecognitionArtifactRetention::Complete,
            recording_memory_limit: RecordingMemoryLimit::default_limit(),
            recording_retention: RecordingRetention::Selective,
            runtime_capture: capture_live::RuntimeCaptureInput::LegacyGamescope {
                binding_path: Path::new(binding),
                expected_binding_sha256: &binding_digest,
                expected_source_node_id: None,
            },
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

#[cfg(test)]
pub(super) fn print_capture_handoff_report(
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

pub(super) fn parse_cli_sha256(value: &OsStr, label: &str) -> Result<String, String> {
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

pub(super) fn parse_capture_generation(
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

pub(super) fn parse_diagnostic_run_id(value: &OsStr) -> Result<String, String> {
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

pub(super) fn parse_diagnostic_recording_policy(value: &OsStr) -> Result<DiagnosticPolicy, String> {
    match value.to_str() {
        Some("enabled") => Ok(DiagnosticPolicy {
            retention: DiagnosticRetention::FactsOnly,
            ..DiagnosticPolicy::default()
        }),
        Some("disabled") => Ok(DiagnosticPolicy {
            enabled: false,
            ..DiagnosticPolicy::default()
        }),
        _ => Err("recording must be enabled or disabled".to_owned()),
    }
}

#[cfg(test)]
pub(super) fn try_capture_canonical_frame(args: &[OsString]) -> Option<Result<(), String>> {
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

#[cfg(test)]
pub(super) fn try_capture_binding_admission(args: &[OsString]) -> Option<Result<(), String>> {
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

#[cfg(test)]
pub(super) fn try_capture_live_gate(args: &[OsString]) -> Option<Result<(), String>> {
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

#[cfg(test)]
pub(super) fn print_capture_gate_report(
    report: &capture_live::GamescopeLiveGateReport,
) -> Result<(), String> {
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
