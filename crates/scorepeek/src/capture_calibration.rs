use std::ffi::OsStr;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use scorepeek::capture::{
    CaptureDiagnosticFact, CaptureDiagnosticSink, CaptureErrorType, CaptureSourceKind,
    UncalibratedFrame, UncalibratedMemoryType, UncalibratedVideoContract, acquire_gamescope_source,
    start_uncalibrated_gamescope_receiver,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(2);
const RECEIVER_START_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_DIAGNOSTIC_FACTS: usize = 32;
const MAX_NESTED_WIDTH: u32 = 7_680;
const MAX_NESTED_HEIGHT: u32 = 4_320;
const MAX_NESTED_REFRESH: u32 = 1_000;
const FRAME_FILENAME: &str = "frame.bgrx";
const MANIFEST_FILENAME: &str = "manifest.json";
const MANIFEST_STAGING_FILENAME: &str = ".manifest.json.staging";
const OWNERSHIP_FILENAME: &str = ".scorepeek-uncalibrated-calibration-v1";
const OWNERSHIP_BYTES: &[u8] = b"scorepeek-owned-uncalibrated-calibration-v1\n";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GamescopeScaler {
    Auto,
    Integer,
    Fit,
    Fill,
    Stretch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GamescopeFilter {
    Linear,
    Nearest,
    Fsr,
    Nis,
    Pixel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ScalingEvidenceKind {
    OperatorDeclared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct GamescopeScalingConfiguration {
    evidence_kind: ScalingEvidenceKind,
    nested_width: u32,
    nested_height: u32,
    nested_refresh_hz: u32,
    scaler: GamescopeScaler,
    filter: GamescopeFilter,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CalibrationSampleStatus {
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum CalibrationSampleErrorType {
    OutputUnavailable,
    CaptureFailed,
    FrameUnavailable,
    ReceiverShutdownFailed,
    ManifestEncodingFailed,
    PublicationFailed,
}

#[derive(Debug, Serialize)]
pub struct GamescopeCalibrationSampleReport {
    schema: &'static str,
    status: CalibrationSampleStatus,
    error_type: Option<CalibrationSampleErrorType>,
    capture_error_type: Option<CaptureErrorType>,
    manifest_sha256: Option<String>,
    frame_sha256: Option<String>,
    diagnostic_facts: Vec<CaptureDiagnosticFact>,
    dropped_diagnostic_facts: u64,
}

impl GamescopeCalibrationSampleReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, CalibrationSampleStatus::Success)
    }
}

#[derive(Serialize)]
struct CalibrationFrameArtifact {
    filename: &'static str,
    byte_count: u64,
    sha256: String,
}

#[derive(Serialize)]
struct GamescopeCalibrationSampleManifest<'a> {
    schema: &'static str,
    calibration_state: &'static str,
    source: CaptureSourceKind,
    scaling_configuration: GamescopeScalingConfiguration,
    observed_video_contract: UncalibratedVideoContract,
    memory_type: UncalibratedMemoryType,
    stride: u32,
    receiver_sequence: u64,
    received_monotonic_ns: u64,
    frame: CalibrationFrameArtifact,
    diagnostic_facts: &'a [CaptureDiagnosticFact],
    dropped_diagnostic_facts: u64,
}

#[derive(Default)]
struct BoundedDiagnosticSink {
    facts: Vec<CaptureDiagnosticFact>,
    dropped: u64,
}

impl CaptureDiagnosticSink for BoundedDiagnosticSink {
    fn record(&mut self, fact: CaptureDiagnosticFact) {
        if self.facts.len() < MAX_DIAGNOSTIC_FACTS {
            self.facts.push(fact);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicationPoint {
    DirectoryCreated,
    OwnershipSynced,
    FrameSynced,
    ManifestSynced,
    ManifestPublished,
    DirectorySynced,
}

pub fn parse_scaling_configuration(
    nested_width: &OsStr,
    nested_height: &OsStr,
    nested_refresh: &OsStr,
    scaler: &OsStr,
    filter: &OsStr,
) -> Result<GamescopeScalingConfiguration, String> {
    Ok(GamescopeScalingConfiguration {
        evidence_kind: ScalingEvidenceKind::OperatorDeclared,
        nested_width: parse_bounded_u32(nested_width, "Gamescope nested width", MAX_NESTED_WIDTH)?,
        nested_height: parse_bounded_u32(
            nested_height,
            "Gamescope nested height",
            MAX_NESTED_HEIGHT,
        )?,
        nested_refresh_hz: parse_bounded_u32(
            nested_refresh,
            "Gamescope nested refresh",
            MAX_NESTED_REFRESH,
        )?,
        scaler: parse_scaler(scaler)?,
        filter: parse_filter(filter)?,
    })
}

fn parse_bounded_u32(value: &OsStr, label: &str, maximum: u32) -> Result<u32, String> {
    let value = value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?;
    let value = value
        .parse::<u32>()
        .map_err(|_| format!("{label} must be an integer"))?;
    if !(1..=maximum).contains(&value) {
        return Err(format!("{label} must be between 1 and {maximum}"));
    }
    Ok(value)
}

fn parse_scaler(value: &OsStr) -> Result<GamescopeScaler, String> {
    match value.to_str() {
        Some("auto") => Ok(GamescopeScaler::Auto),
        Some("integer") => Ok(GamescopeScaler::Integer),
        Some("fit") => Ok(GamescopeScaler::Fit),
        Some("fill") => Ok(GamescopeScaler::Fill),
        Some("stretch") => Ok(GamescopeScaler::Stretch),
        _ => Err("Gamescope scaler must be auto, integer, fit, fill, or stretch".to_owned()),
    }
}

fn parse_filter(value: &OsStr) -> Result<GamescopeFilter, String> {
    match value.to_str() {
        Some("linear") => Ok(GamescopeFilter::Linear),
        Some("nearest") => Ok(GamescopeFilter::Nearest),
        Some("fsr") => Ok(GamescopeFilter::Fsr),
        Some("nis") => Ok(GamescopeFilter::Nis),
        Some("pixel") => Ok(GamescopeFilter::Pixel),
        _ => Err("Gamescope filter must be linear, nearest, fsr, nis, or pixel".to_owned()),
    }
}

pub fn capture_gamescope_calibration_sample(
    output: &Path,
    scaling_configuration: GamescopeScalingConfiguration,
) -> GamescopeCalibrationSampleReport {
    let Ok(output) = resolve_output(output) else {
        return error_report(
            CalibrationSampleErrorType::OutputUnavailable,
            None,
            BoundedDiagnosticSink::default(),
        );
    };
    let mut sink = BoundedDiagnosticSink::default();
    let lease = match acquire_gamescope_source(DISCOVERY_TIMEOUT, &mut sink) {
        Ok(lease) => lease,
        Err(error) => {
            return error_report(
                CalibrationSampleErrorType::CaptureFailed,
                Some(error.error_type()),
                sink,
            );
        }
    };
    let mut receiver =
        match start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, &mut sink) {
            Ok(receiver) => receiver,
            Err(error) => {
                return error_report(
                    CalibrationSampleErrorType::CaptureFailed,
                    Some(error.error_type()),
                    sink,
                );
            }
        };
    let frame = receiver.take_latest_frame();
    if let Err(error) = receiver.shutdown(&mut sink) {
        return error_report(
            CalibrationSampleErrorType::ReceiverShutdownFailed,
            Some(error.error_type()),
            sink,
        );
    }
    let Some(frame) = frame else {
        return error_report(CalibrationSampleErrorType::FrameUnavailable, None, sink);
    };
    publish_sample(&output, scaling_configuration, &frame, sink)
}

fn publish_sample(
    output: &Path,
    scaling_configuration: GamescopeScalingConfiguration,
    frame: &UncalibratedFrame,
    sink: BoundedDiagnosticSink,
) -> GamescopeCalibrationSampleReport {
    let frame_sha256 = encode_sha256(frame.bytes());
    let frame_byte_count = u64::try_from(frame.bytes().len()).unwrap_or(u64::MAX);
    let manifest = GamescopeCalibrationSampleManifest {
        schema: "scorepeek-private-uncalibrated-gamescope-sample-v1",
        calibration_state: "uncalibrated",
        source: CaptureSourceKind::GamescopeDefaultRemote,
        scaling_configuration,
        observed_video_contract: frame.contract(),
        memory_type: frame.memory_type(),
        stride: frame.stride(),
        receiver_sequence: frame.sequence(),
        received_monotonic_ns: frame.received_monotonic_ns(),
        frame: CalibrationFrameArtifact {
            filename: FRAME_FILENAME,
            byte_count: frame_byte_count,
            sha256: frame_sha256.clone(),
        },
        diagnostic_facts: &sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    };
    let Ok(mut manifest_bytes) = serde_json::to_vec(&manifest) else {
        return error_report(
            CalibrationSampleErrorType::ManifestEncodingFailed,
            None,
            sink,
        );
    };
    manifest_bytes.push(b'\n');
    let manifest_sha256 = encode_sha256(&manifest_bytes);
    if publish_sample_with(output, frame.bytes(), &manifest_bytes, |_| Ok(())).is_err() {
        return error_report(CalibrationSampleErrorType::PublicationFailed, None, sink);
    }
    GamescopeCalibrationSampleReport {
        schema: "scorepeek-gamescope-calibration-sample-report-v1",
        status: CalibrationSampleStatus::Success,
        error_type: None,
        capture_error_type: None,
        manifest_sha256: Some(manifest_sha256),
        frame_sha256: Some(frame_sha256),
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}

fn resolve_output(output: &Path) -> Result<PathBuf, ()> {
    if !output.is_absolute()
        || output.file_name().is_none()
        || output
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(());
    }
    let parent = output.parent().ok_or(())?;
    let metadata = parent.symlink_metadata().map_err(|_| ())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(());
    }
    let output = parent
        .canonicalize()
        .map_err(|_| ())?
        .join(output.file_name().ok_or(())?);
    if output.symlink_metadata().is_ok() {
        recover_owned_incomplete(&output)?;
    }
    if output.symlink_metadata().is_ok() {
        return Err(());
    }
    Ok(output)
}

fn recover_owned_incomplete(output: &Path) -> Result<(), ()> {
    let metadata = output.symlink_metadata().map_err(|_| ())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(());
    }
    if output.join(MANIFEST_FILENAME).symlink_metadata().is_ok() {
        return Err(());
    }
    remove_owned_output(output, false).map_err(|_| ())
}

fn publish_sample_with(
    output: &Path,
    frame: &[u8],
    manifest: &[u8],
    mut checkpoint: impl FnMut(PublicationPoint) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let parent = output.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no parent")
    })?;
    DirBuilder::new().mode(0o700).create(output)?;
    let mut ownership_synced = false;
    let publication = (|| {
        checkpoint(PublicationPoint::DirectoryCreated)?;
        write_private_file(&output.join(OWNERSHIP_FILENAME), OWNERSHIP_BYTES)?;
        ownership_synced = true;
        checkpoint(PublicationPoint::OwnershipSynced)?;
        write_private_file(&output.join(FRAME_FILENAME), frame)?;
        checkpoint(PublicationPoint::FrameSynced)?;
        write_private_file(&output.join(MANIFEST_STAGING_FILENAME), manifest)?;
        checkpoint(PublicationPoint::ManifestSynced)?;
        fs::hard_link(
            output.join(MANIFEST_STAGING_FILENAME),
            output.join(MANIFEST_FILENAME),
        )?;
        checkpoint(PublicationPoint::ManifestPublished)?;
        fs::remove_file(output.join(MANIFEST_STAGING_FILENAME))?;
        File::open(output)?.sync_all()?;
        checkpoint(PublicationPoint::DirectorySynced)?;
        File::open(parent)?.sync_all()
    })();
    if let Err(error) = publication {
        let _ = cleanup_created_output(output, ownership_synced);
        return Err(error);
    }
    Ok(())
}

fn cleanup_created_output(output: &Path, ownership_synced: bool) -> std::io::Result<()> {
    if ownership_synced {
        return remove_owned_output(output, true);
    }
    for entry in fs::read_dir(output)? {
        if entry?.file_name() != OWNERSHIP_FILENAME {
            return Err(std::io::Error::other(
                "new calibration output has an unknown entry",
            ));
        }
    }
    if output.join(OWNERSHIP_FILENAME).symlink_metadata().is_ok() {
        fs::remove_file(output.join(OWNERSHIP_FILENAME))?;
    }
    fs::remove_dir(output)?;
    File::open(output.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no parent")
    })?)?
    .sync_all()
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn remove_owned_output(output: &Path, allow_manifest: bool) -> std::io::Result<()> {
    verify_regular_file(&output.join(OWNERSHIP_FILENAME), Some(OWNERSHIP_BYTES))?;
    for entry in fs::read_dir(output)? {
        let name = entry?.file_name();
        let allowed = name == OWNERSHIP_FILENAME
            || name == FRAME_FILENAME
            || name == MANIFEST_STAGING_FILENAME
            || (allow_manifest && name == MANIFEST_FILENAME);
        if !allowed {
            return Err(std::io::Error::other(
                "calibration output has an unknown entry",
            ));
        }
    }
    for name in [FRAME_FILENAME, MANIFEST_STAGING_FILENAME, MANIFEST_FILENAME] {
        let path = output.join(name);
        match path.symlink_metadata() {
            Ok(_) if name == MANIFEST_FILENAME && !allow_manifest => {
                return Err(std::io::Error::other(
                    "incomplete calibration output has a manifest",
                ));
            }
            Ok(_) => {
                verify_regular_file(&path, None)?;
                fs::remove_file(path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    fs::remove_file(output.join(OWNERSHIP_FILENAME))?;
    fs::remove_dir(output)?;
    File::open(output.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "output has no parent")
    })?)?
    .sync_all()
}

fn verify_regular_file(path: &Path, expected: Option<&[u8]>) -> std::io::Result<()> {
    let path_metadata = path.symlink_metadata()?;
    if !path_metadata.is_file() || path_metadata.file_type().is_symlink() {
        return Err(std::io::Error::other(
            "calibration output entry is not a regular file",
        ));
    }
    let mut file = File::open(path)?;
    let file_metadata = file.metadata()?;
    if !file_metadata.is_file()
        || path_metadata.dev() != file_metadata.dev()
        || path_metadata.ino() != file_metadata.ino()
    {
        return Err(std::io::Error::other(
            "calibration output entry changed during validation",
        ));
    }
    if let Some(expected) = expected {
        if file_metadata.len() != u64::try_from(expected.len()).unwrap_or(u64::MAX) {
            return Err(std::io::Error::other("calibration ownership mismatch"));
        }
        let mut actual = vec![0; expected.len()];
        file.read_exact(&mut actual)?;
        if actual != expected {
            return Err(std::io::Error::other("calibration ownership mismatch"));
        }
    }
    Ok(())
}

fn error_report(
    error_type: CalibrationSampleErrorType,
    capture_error_type: Option<CaptureErrorType>,
    sink: BoundedDiagnosticSink,
) -> GamescopeCalibrationSampleReport {
    GamescopeCalibrationSampleReport {
        schema: "scorepeek-gamescope-calibration-sample-report-v1",
        status: CalibrationSampleStatus::Error,
        error_type: Some(error_type),
        capture_error_type,
        manifest_sha256: None,
        frame_sha256: None,
        diagnostic_facts: sink.facts,
        dropped_diagnostic_facts: sink.dropped,
    }
}

fn encode_sha256(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;
    use std::io;
    use std::os::unix::fs::symlink;

    use scorepeek::capture::{
        CaptureSourceKind, UncalibratedMemoryType, UncalibratedVideoContract,
    };

    use super::{
        CalibrationFrameArtifact, FRAME_FILENAME, GamescopeCalibrationSampleManifest,
        MANIFEST_FILENAME, MANIFEST_STAGING_FILENAME, OWNERSHIP_BYTES, OWNERSHIP_FILENAME,
        PublicationPoint, parse_scaling_configuration, publish_sample_with, resolve_output,
    };

    #[test]
    fn scaling_configuration_is_strict_and_bounded() {
        let configuration = parse_scaling_configuration(
            OsStr::new("1920"),
            OsStr::new("1080"),
            OsStr::new("120"),
            OsStr::new("auto"),
            OsStr::new("linear"),
        )
        .unwrap();
        assert_eq!(configuration.nested_width, 1920);
        assert_eq!(configuration.nested_height, 1080);
        assert_eq!(configuration.nested_refresh_hz, 120);
        assert!(
            parse_scaling_configuration(
                OsStr::new("0"),
                OsStr::new("1080"),
                OsStr::new("120"),
                OsStr::new("auto"),
                OsStr::new("linear"),
            )
            .is_err()
        );
        assert!(
            parse_scaling_configuration(
                OsStr::new("1920"),
                OsStr::new("1080"),
                OsStr::new("120"),
                OsStr::new("auto"),
                OsStr::new("unknown"),
            )
            .is_err()
        );
    }

    #[test]
    fn manifest_binds_declared_scaling_and_raw_frame_digest() {
        let scaling_configuration = parse_scaling_configuration(
            OsStr::new("1920"),
            OsStr::new("1080"),
            OsStr::new("120"),
            OsStr::new("auto"),
            OsStr::new("linear"),
        )
        .unwrap();
        let manifest = GamescopeCalibrationSampleManifest {
            schema: "scorepeek-private-uncalibrated-gamescope-sample-v1",
            calibration_state: "uncalibrated",
            source: CaptureSourceKind::GamescopeDefaultRemote,
            scaling_configuration,
            observed_video_contract: UncalibratedVideoContract {
                width: 2_556,
                height: 1_428,
                framerate_num: 0,
                framerate_denom: 1,
                maximum_framerate_num: 0,
                maximum_framerate_denom: 1,
                pixel_aspect_num: 1,
                pixel_aspect_denom: 1,
                chroma_site: 0,
                color_range: 0,
                color_matrix: 0,
                transfer_function: 0,
                color_primaries: 0,
            },
            memory_type: UncalibratedMemoryType::MemoryFileDescriptor,
            stride: 10_224,
            receiver_sequence: 1,
            received_monotonic_ns: 42,
            frame: CalibrationFrameArtifact {
                filename: FRAME_FILENAME,
                byte_count: 14_599_872,
                sha256: "a".repeat(64),
            },
            diagnostic_facts: &[],
            dropped_diagnostic_facts: 0,
        };
        let value = serde_json::to_value(manifest).unwrap();
        assert_eq!(value["calibration_state"], "uncalibrated");
        assert_eq!(value["scaling_configuration"]["filter"], "linear");
        assert_eq!(value["observed_video_contract"]["width"], 2_556);
        assert_eq!(value["frame"]["sha256"], "a".repeat(64));
    }

    #[test]
    fn publication_is_create_only_and_manifest_last() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("sample");
        let mut points = Vec::new();
        publish_sample_with(&output, b"pixels", b"manifest\n", |point| {
            if point == PublicationPoint::ManifestSynced {
                assert!(!output.join(MANIFEST_FILENAME).exists());
                assert!(output.join(MANIFEST_STAGING_FILENAME).is_file());
            }
            points.push(point);
            Ok(())
        })
        .unwrap();
        assert_eq!(fs::read(output.join(FRAME_FILENAME)).unwrap(), b"pixels");
        assert_eq!(
            fs::read(output.join(MANIFEST_FILENAME)).unwrap(),
            b"manifest\n"
        );
        assert_eq!(
            fs::read(output.join(OWNERSHIP_FILENAME)).unwrap(),
            OWNERSHIP_BYTES
        );
        assert_eq!(points.last(), Some(&PublicationPoint::DirectorySynced));
        assert!(publish_sample_with(&output, b"new", b"new", |_| Ok(())).is_err());
        assert_eq!(fs::read(output.join(FRAME_FILENAME)).unwrap(), b"pixels");
    }

    #[test]
    fn every_publication_failure_cleans_owned_output() {
        for failed in [
            PublicationPoint::DirectoryCreated,
            PublicationPoint::OwnershipSynced,
            PublicationPoint::FrameSynced,
            PublicationPoint::ManifestSynced,
            PublicationPoint::ManifestPublished,
            PublicationPoint::DirectorySynced,
        ] {
            let parent = tempfile::tempdir().unwrap();
            let output = parent.path().join("sample");
            let result = publish_sample_with(&output, b"pixels", b"manifest\n", |point| {
                if point == failed {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err());
            assert!(!output.exists());
        }
    }

    #[test]
    fn resolver_recovers_only_owned_incomplete_output() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("sample");
        fs::create_dir(&output).unwrap();
        fs::write(output.join(OWNERSHIP_FILENAME), OWNERSHIP_BYTES).unwrap();
        fs::write(output.join(FRAME_FILENAME), b"partial").unwrap();
        assert_eq!(resolve_output(&output).unwrap(), output);
        assert!(!output.exists());

        fs::create_dir(&output).unwrap();
        fs::write(output.join(OWNERSHIP_FILENAME), OWNERSHIP_BYTES).unwrap();
        fs::write(output.join("unknown"), b"preserve").unwrap();
        assert!(resolve_output(&output).is_err());
        assert_eq!(fs::read(output.join("unknown")).unwrap(), b"preserve");
    }

    #[test]
    fn resolver_preserves_symlinked_owned_entry_names() {
        let parent = tempfile::tempdir().unwrap();
        let external = parent.path().join("external");
        fs::write(&external, OWNERSHIP_BYTES).unwrap();
        let output = parent.path().join("sample");
        fs::create_dir(&output).unwrap();
        symlink(&external, output.join(OWNERSHIP_FILENAME)).unwrap();
        assert!(resolve_output(&output).is_err());
        assert!(output.exists());
        assert_eq!(fs::read(&external).unwrap(), OWNERSHIP_BYTES);

        fs::remove_file(output.join(OWNERSHIP_FILENAME)).unwrap();
        fs::write(output.join(OWNERSHIP_FILENAME), OWNERSHIP_BYTES).unwrap();
        symlink(&external, output.join(FRAME_FILENAME)).unwrap();
        assert!(resolve_output(&output).is_err());
        assert!(output.exists());
        assert_eq!(fs::read(&external).unwrap(), OWNERSHIP_BYTES);
    }

    #[test]
    fn resolver_rejects_relative_and_complete_outputs() {
        assert!(resolve_output(std::path::Path::new("relative")).is_err());
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("sample");
        fs::create_dir(&output).unwrap();
        fs::write(output.join(OWNERSHIP_FILENAME), OWNERSHIP_BYTES).unwrap();
        fs::write(output.join(MANIFEST_FILENAME), b"complete").unwrap();
        assert!(resolve_output(&output).is_err());
        assert!(output.exists());
    }
}
