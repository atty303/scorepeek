use super::*;

pub(super) fn try_recording_simulation(
    args: &[OsString],
    bundle: &Path,
) -> Option<Result<(), String>> {
    try_recording_simulation_profile_author(args)
        .or_else(|| try_recording_recognition_evidence_run(args, bundle))
        .or_else(|| try_recording_simulation_run(args, bundle))
}

pub(super) fn try_recording_recognition_evidence_run(
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

pub(super) fn try_recording_simulation_profile_author(
    args: &[OsString],
) -> Option<Result<(), String>> {
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

pub(super) fn try_recording_simulation_run(
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
pub(super) fn execute_recording_simulation(
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
