use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::diagnostic_recording::{
    DEFAULT_SAMPLE_INTERVAL_MS, DiagnosticBinding, DiagnosticCompleteness, DiagnosticFinishOutcome,
    DiagnosticPolicy, DiagnosticReplayBinding, DiagnosticResource, DiagnosticRunDescriptor,
    DiagnosticRunStatus,
};
use crate::diagnostic_worker::{
    DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT, DiagnosticEnqueueOutcome, DiagnosticOwnedFrame,
    DiagnosticWorkerHandle,
};
use crate::recognition::CanonicalFrame;

const MAX_REPLAY_REQUEST_BYTES: u64 = 1024 * 1024;
const MAX_REPLAY_FRAMES: usize = 8_192;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticReplayRequest {
    schema: String,
    run_id: String,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    build_sha256: String,
    capture_generation: u64,
    capture_profile_sha256: String,
    normalizer_sha256: String,
    canonical_layout_sha256: String,
    catalog_sha256: String,
    model_sha256: String,
    runtime_sha256: String,
    extraction_sha256: String,
    frames: Vec<DiagnosticReplayFrame>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticReplayFrame {
    sequence: u64,
    frame_id: String,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
}

#[derive(Serialize)]
pub struct DiagnosticReplaySummary {
    schema: &'static str,
    run_id: String,
    request_sha256: String,
    offered_frames: usize,
    enqueued_frames: usize,
    completeness: Option<DiagnosticCompleteness>,
    error_type: Option<crate::diagnostic_recording::DiagnosticErrorType>,
    manifest_sha256: Option<String>,
}

/// Replays strict canonical extraction frames through the bounded application worker.
///
/// # Errors
/// Returns a value-free error when paths, request binding, canonical evidence, queue delivery, or
/// diagnostic completion fail. Private paths and frame contents are never included in the error.
pub fn replay_diagnostic_run(
    request_path: &Path,
    expected_request_sha256: &str,
    extraction_directory: &Path,
    output_root: &Path,
) -> Result<DiagnosticReplaySummary, String> {
    validate_directory(extraction_directory, "canonical extraction")?;
    validate_directory(output_root, "diagnostic output root")?;
    let request_bytes = read_bounded_regular(request_path, MAX_REPLAY_REQUEST_BYTES)?;
    if !valid_sha256(expected_request_sha256)
        || encode_sha256(&request_bytes) != expected_request_sha256
    {
        return Err("diagnostic replay request digest mismatch".to_owned());
    }
    let request: DiagnosticReplayRequest = serde_json::from_slice(&request_bytes)
        .map_err(|_| "diagnostic replay request schema is invalid".to_owned())?;
    if !valid_request(&request) {
        return Err("diagnostic replay request contract is invalid".to_owned());
    }
    let descriptor = descriptor(&request, expected_request_sha256);
    let mut worker =
        DiagnosticWorkerHandle::start(output_root, descriptor, DiagnosticPolicy::default());
    let mut enqueued_frames = 0;
    let mut previous_decode_index = None;
    for requested in &request.frames {
        let Ok(frame) = CanonicalFrame::read_extraction(
            extraction_directory,
            &requested.frame_id,
            &request.extraction_sha256,
        ) else {
            worker.record_external_error(
                crate::diagnostic_recording::DiagnosticErrorType::InvalidConfiguration,
                requested.sequence,
            );
            let _ = worker.finish(
                DiagnosticRunStatus::Error,
                requested.monotonic_start_ms,
                DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT,
            );
            return Err("diagnostic replay canonical frame is invalid".to_owned());
        };
        if !frame_matches_request(&frame, requested, &request, previous_decode_index) {
            worker.record_external_error(
                crate::diagnostic_recording::DiagnosticErrorType::InvalidConfiguration,
                requested.sequence,
            );
            let _ = worker.finish(
                DiagnosticRunStatus::Error,
                requested.monotonic_start_ms,
                DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT,
            );
            return Err("diagnostic replay canonical binding mismatch".to_owned());
        }
        previous_decode_index = Some(frame.decode_index());
        let outcome = worker.record_frame_until(
            DiagnosticOwnedFrame {
                sequence: requested.sequence,
                monotonic_start_ms: requested.monotonic_start_ms,
                monotonic_end_ms: requested.monotonic_end_ms,
                pixels: frame.pixels().to_vec().into_boxed_slice(),
            },
            Instant::now() + DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT,
        );
        if outcome != DiagnosticEnqueueOutcome::Enqueued {
            let _ = worker.finish(
                DiagnosticRunStatus::Error,
                requested.monotonic_end_ms,
                DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT,
            );
            return Err("diagnostic replay queue delivery failed".to_owned());
        }
        enqueued_frames += 1;
    }
    let finish = worker.finish(
        DiagnosticRunStatus::Success,
        request.monotonic_end_ms,
        DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT,
    );
    require_complete(&finish)?;
    Ok(DiagnosticReplaySummary {
        schema: "scorepeek-diagnostic-replay-summary-v1",
        run_id: request.run_id,
        request_sha256: expected_request_sha256.to_owned(),
        offered_frames: request.frames.len(),
        enqueued_frames,
        completeness: finish.completeness,
        error_type: finish.error_type,
        manifest_sha256: finish.manifest_sha256,
    })
}

fn frame_matches_request(
    frame: &CanonicalFrame,
    requested: &DiagnosticReplayFrame,
    request: &DiagnosticReplayRequest,
    previous_decode_index: Option<u64>,
) -> bool {
    frame.capture_profile_id() == request.capture_profile_sha256
        && frame.normalizer_artifact_sha256() == request.normalizer_sha256
        && frame.frame_extraction_sha256() == request.extraction_sha256
        && u64::try_from(frame.source_pts_ms()).ok() == Some(requested.monotonic_start_ms)
        && requested.monotonic_end_ms == requested.monotonic_start_ms
        && previous_decode_index.is_none_or(|previous| frame.decode_index() > previous)
}

fn require_complete(finish: &DiagnosticFinishOutcome) -> Result<(), String> {
    if finish.completeness == Some(DiagnosticCompleteness::Complete)
        && finish.error_type.is_none()
        && finish.manifest_sha256.is_some()
    {
        Ok(())
    } else {
        Err("diagnostic replay completion failed".to_owned())
    }
}

fn descriptor(request: &DiagnosticReplayRequest, request_sha256: &str) -> DiagnosticRunDescriptor {
    DiagnosticRunDescriptor {
        run_id: request.run_id.clone(),
        monotonic_start_ms: request.monotonic_start_ms,
        resource: DiagnosticResource {
            program: "scorepeek",
            version: env!("CARGO_PKG_VERSION"),
            build_sha256: request.build_sha256.clone(),
        },
        binding: DiagnosticBinding {
            capture_generation: request.capture_generation,
            capture_profile_sha256: request.capture_profile_sha256.clone(),
            normalizer_sha256: request.normalizer_sha256.clone(),
            canonical_layout_sha256: request.canonical_layout_sha256.clone(),
            catalog_sha256: request.catalog_sha256.clone(),
            model_sha256: request.model_sha256.clone(),
            runtime_sha256: request.runtime_sha256.clone(),
            replay: Some(DiagnosticReplayBinding {
                request_sha256: request_sha256.to_owned(),
                extraction_sha256: request.extraction_sha256.clone(),
            }),
        },
    }
}

fn valid_request(request: &DiagnosticReplayRequest) -> bool {
    let mut frame_ids = BTreeSet::new();
    request.schema == "scorepeek-diagnostic-replay-request-v1"
        && !request.run_id.is_empty()
        && request.run_id.len() <= 64
        && request
            .run_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && request.capture_generation > 0
        && request.monotonic_end_ms >= request.monotonic_start_ms
        && !request.frames.is_empty()
        && request.frames.len() <= MAX_REPLAY_FRAMES
        && [
            &request.build_sha256,
            &request.capture_profile_sha256,
            &request.normalizer_sha256,
            &request.canonical_layout_sha256,
            &request.catalog_sha256,
            &request.model_sha256,
            &request.runtime_sha256,
            &request.extraction_sha256,
        ]
        .into_iter()
        .all(|digest| valid_sha256(digest))
        && request.frames.iter().enumerate().all(|(index, frame)| {
            !frame.frame_id.is_empty()
                && frame_ids.insert(frame.frame_id.as_str())
                && frame.frame_id.len() <= 128
                && !frame.frame_id.chars().any(char::is_control)
                && frame.monotonic_start_ms >= request.monotonic_start_ms
                && frame.monotonic_end_ms >= frame.monotonic_start_ms
                && frame.monotonic_end_ms == frame.monotonic_start_ms
                && frame.monotonic_end_ms <= request.monotonic_end_ms
                && index.checked_sub(1).is_none_or(|previous| {
                    let prior = &request.frames[previous];
                    frame.sequence > prior.sequence
                        && frame.monotonic_start_ms > prior.monotonic_start_ms
                        && frame.monotonic_start_ms - prior.monotonic_start_ms
                            >= DEFAULT_SAMPLE_INTERVAL_MS
                        && frame.monotonic_end_ms >= prior.monotonic_end_ms
                })
        })
}

fn validate_directory(path: &Path, name: &str) -> Result<(), String> {
    let metadata = path
        .symlink_metadata()
        .map_err(|_| format!("{name} is unavailable"))?;
    if !path.is_absolute() || !metadata.is_dir() || metadata.is_symlink() {
        return Err(format!("{name} must be an absolute regular directory"));
    }
    Ok(())
}

fn read_bounded_regular(path: &Path, maximum: u64) -> Result<Vec<u8>, String> {
    let metadata = path
        .symlink_metadata()
        .map_err(|_| "diagnostic replay request is unavailable".to_owned())?;
    if !path.is_absolute()
        || !metadata.is_file()
        || metadata.is_symlink()
        || metadata.len() > maximum
    {
        return Err("diagnostic replay request file is invalid".to_owned());
    }
    let bytes = fs::read(path).map_err(|_| "diagnostic replay request read failed".to_owned())?;
    if bytes.len() as u64 != metadata.len() {
        return Err("diagnostic replay request changed while reading".to_owned());
    }
    Ok(bytes)
}

#[cfg(test)]
fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(value)
        .map_err(|_| "diagnostic replay request serialization failed".to_owned())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn encode_sha256(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    const CAPTURE_PROFILE: &str =
        "d5809dc9b2acc19837260053f4df59a454c9178ae2ac6a0602982effc9da4704";
    const FFMPEG_SHA256: &str = "9eac5b2b5076db5ff853a6fa0dcd6b8de7d0cac8481eadda6c47cd935825f1ee";
    const PPM_HEADER: &[u8] = b"P6\n1920 1080\n255\n";

    fn replay_guard() -> MutexGuard<'static, ()> {
        static REPLAY_TEST: OnceLock<Mutex<()>> = OnceLock::new();
        REPLAY_TEST
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    struct TestCanonicalEvidence {
        extraction: String,
        capture_profile: String,
        normalizer: String,
    }

    #[derive(Clone, Copy, Serialize)]
    struct TestTimeBase {
        numerator: i64,
        denominator: i64,
    }

    #[derive(Serialize)]
    struct TestObserved {
        input_format: &'static str,
        codec_name: &'static str,
        pixel_format: &'static str,
        width: u32,
        height: u32,
        source_time_base: TestTimeBase,
        color_range: Option<&'static str>,
        color_space: Option<&'static str>,
        color_transfer: Option<&'static str>,
        color_primaries: Option<&'static str>,
    }

    #[derive(Serialize)]
    struct TestNormalizer {
        schema: &'static str,
        capture_profile_id: &'static str,
        observed: TestObserved,
        canonical_frame_contract_id: &'static str,
        implementation: &'static str,
        ffmpeg_sha256: &'static str,
        filter: &'static str,
    }

    #[derive(Serialize)]
    struct TestExtractor {
        tool_id: &'static str,
        tool_version: &'static str,
        extractor_manifest_sha256: String,
        parameters_sha256: String,
    }

    #[derive(Serialize)]
    struct TestFrame {
        frame_id: String,
        source_pts: i64,
        decode_index: u64,
        filename: String,
        frame_sha256: String,
        file_sha256: String,
        bytes: u64,
    }

    #[derive(Serialize)]
    struct TestExtraction {
        schema: &'static str,
        fixture_id: &'static str,
        source_manifest_sha256: String,
        media_probe_sha256: String,
        capture_profile_id: &'static str,
        normalizer_artifact_sha256: String,
        canonical_frame_contract_id: &'static str,
        extractor: TestExtractor,
        source_time_base: TestTimeBase,
        video_stream_index: u32,
        frames: Vec<TestFrame>,
    }

    fn request(evidence: &TestCanonicalEvidence) -> DiagnosticReplayRequest {
        DiagnosticReplayRequest {
            schema: "scorepeek-diagnostic-replay-request-v1".to_owned(),
            run_id: "ordinary-session-replay".to_owned(),
            monotonic_start_ms: 0,
            monotonic_end_ms: 1_016,
            build_sha256: "1".repeat(64),
            capture_generation: 1,
            capture_profile_sha256: evidence.capture_profile.clone(),
            normalizer_sha256: evidence.normalizer.clone(),
            canonical_layout_sha256: "4".repeat(64),
            catalog_sha256: "5".repeat(64),
            model_sha256: "6".repeat(64),
            runtime_sha256: "7".repeat(64),
            extraction_sha256: evidence.extraction.clone(),
            frames: vec![
                DiagnosticReplayFrame {
                    sequence: 1,
                    frame_id: "ordinary-000".to_owned(),
                    monotonic_start_ms: 0,
                    monotonic_end_ms: 0,
                },
                DiagnosticReplayFrame {
                    sequence: 2,
                    frame_id: "ordinary-001".to_owned(),
                    monotonic_start_ms: 1_000,
                    monotonic_end_ms: 1_000,
                },
            ],
        }
    }

    fn write_test_canonical_extraction(
        directory: &Path,
        frame_ids: &[&str],
    ) -> TestCanonicalEvidence {
        let time_base = TestTimeBase {
            numerator: 1,
            denominator: 1_000,
        };
        let normalizer = TestNormalizer {
            schema: "scorepeek-domain-normalizer-artifact-v1",
            capture_profile_id: CAPTURE_PROFILE,
            observed: TestObserved {
                input_format: "matroska",
                codec_name: "ffv1",
                pixel_format: "yuv420p",
                width: 1_920,
                height: 1_080,
                source_time_base: time_base,
                color_range: Some("tv"),
                color_space: Some("bt709"),
                color_transfer: Some("bt709"),
                color_primaries: Some("bt709"),
            },
            canonical_frame_contract_id: "scorepeek-canonical-rgb8-1920x1080-v1",
            implementation: "ffmpeg-swscale-bt709-limited-to-rgb24-v1",
            ffmpeg_sha256: FFMPEG_SHA256,
            filter: "scale=1920:1080:flags=bitexact:in_color_matrix=bt709:out_color_matrix=bt709:in_range=tv:out_range=pc,format=rgb24",
        };
        let normalizer_bytes = canonical_json(&normalizer).unwrap();
        fs::write(directory.join("normalizer.json"), &normalizer_bytes).unwrap();
        let frames = frame_ids
            .iter()
            .enumerate()
            .map(|(index, frame_id)| {
                let pixels = vec![u8::try_from(index).unwrap(); 1_920 * 1_080 * 3];
                let mut ppm = PPM_HEADER.to_vec();
                ppm.extend_from_slice(&pixels);
                let filename = format!("frame-{index:06}.ppm");
                fs::write(directory.join(&filename), &ppm).unwrap();
                TestFrame {
                    frame_id: (*frame_id).to_owned(),
                    source_pts: i64::try_from(index * 1_000).unwrap(),
                    decode_index: index as u64,
                    filename,
                    frame_sha256: encode_sha256(&pixels),
                    file_sha256: encode_sha256(&ppm),
                    bytes: ppm.len() as u64,
                }
            })
            .collect();
        let normalizer_sha256 = encode_sha256(&normalizer_bytes);
        let manifest = TestExtraction {
            schema: "scorepeek-private-canonical-frame-extraction-v1",
            fixture_id: "fixture-diagnostic-replay",
            source_manifest_sha256: "3".repeat(64),
            media_probe_sha256: "4".repeat(64),
            capture_profile_id: CAPTURE_PROFILE,
            normalizer_artifact_sha256: normalizer_sha256.clone(),
            canonical_frame_contract_id: "scorepeek-canonical-rgb8-1920x1080-v1",
            extractor: TestExtractor {
                tool_id: "ffmpeg",
                tool_version: "8.1.2",
                extractor_manifest_sha256: "4".repeat(64),
                parameters_sha256: "5".repeat(64),
            },
            source_time_base: time_base,
            video_stream_index: 0,
            frames,
        };
        let manifest_bytes = canonical_json(&manifest).unwrap();
        fs::write(directory.join("manifest.json"), &manifest_bytes).unwrap();
        TestCanonicalEvidence {
            extraction: encode_sha256(&manifest_bytes),
            capture_profile: CAPTURE_PROFILE.to_owned(),
            normalizer: normalizer_sha256,
        }
    }

    #[test]
    fn replay_uses_the_worker_and_digest_binds_private_canonical_evidence() {
        let _guard = replay_guard();
        let extraction = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let request_directory = tempfile::tempdir().unwrap();
        let evidence =
            write_test_canonical_extraction(extraction.path(), &["ordinary-000", "ordinary-001"]);
        let bytes = canonical_json(&request(&evidence)).unwrap();
        let digest = encode_sha256(&bytes);
        let request_path = request_directory.path().join("request.json");
        fs::write(&request_path, bytes).unwrap();

        let summary =
            replay_diagnostic_run(&request_path, &digest, extraction.path(), output.path())
                .unwrap();
        assert_eq!(summary.offered_frames, 2);
        assert_eq!(summary.enqueued_frames, 2);
        assert_eq!(summary.completeness, Some(DiagnosticCompleteness::Complete));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(output.path().join("ordinary-session-replay/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["frames"].as_array().unwrap().len(), 2);
        let start: serde_json::Value = serde_json::from_slice(
            &fs::read(output.path().join("ordinary-session-replay/run.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(start["binding"]["replay"]["request_sha256"], digest);
        assert_eq!(
            start["binding"]["replay"]["extraction_sha256"],
            evidence.extraction
        );
    }

    #[test]
    fn missing_requested_frame_is_a_partial_error_manifest() {
        let _guard = replay_guard();
        let extraction = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let request_directory = tempfile::tempdir().unwrap();
        let evidence = write_test_canonical_extraction(extraction.path(), &["ordinary-000"]);
        let bytes = canonical_json(&request(&evidence)).unwrap();
        let digest = encode_sha256(&bytes);
        let request_path = request_directory.path().join("request.json");
        fs::write(&request_path, bytes).unwrap();
        assert!(
            replay_diagnostic_run(&request_path, &digest, extraction.path(), output.path())
                .is_err()
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(output.path().join("ordinary-session-replay/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["status"], "error");
        assert_eq!(manifest["completeness"], "partial");
        assert_eq!(manifest["dropped_count"], 1);
        assert_eq!(manifest["degradations"][0]["affected_sequence"], 2);
        assert_eq!(
            manifest["degradations"][0]["reason"],
            "invalid_configuration"
        );
    }

    #[test]
    fn request_digest_mismatch_fails_before_creating_a_run() {
        let extraction = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let request_directory = tempfile::tempdir().unwrap();
        let evidence = write_test_canonical_extraction(extraction.path(), &["ordinary-000"]);
        let request_path = request_directory.path().join("request.json");
        fs::write(&request_path, canonical_json(&request(&evidence)).unwrap()).unwrap();
        assert!(
            replay_diagnostic_run(
                &request_path,
                &"0".repeat(64),
                extraction.path(),
                output.path(),
            )
            .is_err()
        );
        assert_eq!(output.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn replay_request_rejects_frames_denser_than_the_fixed_cadence() {
        let evidence = TestCanonicalEvidence {
            extraction: "8".repeat(64),
            capture_profile: "2".repeat(64),
            normalizer: "3".repeat(64),
        };
        let mut request = request(&evidence);
        request.frames[1].monotonic_start_ms = 999;
        assert!(!valid_request(&request));
    }

    #[test]
    fn replay_rejects_invalid_descriptor_fields_and_incomplete_finish() {
        let evidence = TestCanonicalEvidence {
            extraction: "8".repeat(64),
            capture_profile: "2".repeat(64),
            normalizer: "3".repeat(64),
        };
        let mut invalid_run = request(&evidence);
        invalid_run.run_id = "INVALID/RUN".to_owned();
        assert!(!valid_request(&invalid_run));
        let mut invalid_generation = request(&evidence);
        invalid_generation.capture_generation = 0;
        assert!(!valid_request(&invalid_generation));
        assert!(
            require_complete(&DiagnosticFinishOutcome {
                completeness: Some(DiagnosticCompleteness::Partial),
                error_type: Some(crate::diagnostic_recording::DiagnosticErrorType::FlushTimeout),
                manifest_sha256: None,
            })
            .is_err()
        );
    }

    #[test]
    fn replay_binding_uses_extraction_pts_order_and_unique_frame_ids() {
        let extraction = tempfile::tempdir().unwrap();
        let evidence =
            write_test_canonical_extraction(extraction.path(), &["ordinary-000", "ordinary-001"]);
        let mut request = request(&evidence);
        let frame = CanonicalFrame::read_extraction(
            extraction.path(),
            "ordinary-001",
            &evidence.extraction,
        )
        .unwrap();
        assert!(frame_matches_request(
            &frame,
            &request.frames[1],
            &request,
            Some(0)
        ));
        request.frames[1].monotonic_start_ms = 2_000;
        request.frames[1].monotonic_end_ms = 2_000;
        assert!(!frame_matches_request(
            &frame,
            &request.frames[1],
            &request,
            Some(0)
        ));
        request.frames[1].frame_id = request.frames[0].frame_id.clone();
        assert!(!valid_request(&request));
    }
}
