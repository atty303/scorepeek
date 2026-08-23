use std::ffi::OsStr;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::SystemTime;

use scorepeek::capture::{
    CaptureDiagnosticFact, CaptureDiagnosticOperation, CaptureDiagnosticSink,
    CaptureDiagnosticStatus, CaptureErrorType, CaptureSourceKind, FractionalRectangle,
    GamescopeProfileBinding, GamescopeProfileBindingAuthoringInput, GamescopeSessionProvenance,
    GamescopeSessionProvenanceInput, RationalCoordinate, UncalibratedFrame, UncalibratedMemoryType,
    UncalibratedVideoContract, acquire_gamescope_source, start_uncalibrated_gamescope_receiver,
};
use serde::{Deserialize, Serialize};
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
static BINDING_STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GamescopeScaler {
    Auto,
    Integer,
    Fit,
    Fill,
    Stretch,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GamescopeFilter {
    Linear,
    Nearest,
    Fsr,
    Nis,
    Pixel,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ScalingEvidenceKind {
    OperatorDeclared,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GamescopeScalingConfiguration {
    evidence_kind: ScalingEvidenceKind,
    nested_width: u32,
    nested_height: u32,
    nested_refresh_hz: u32,
    scaler: GamescopeScaler,
    filter: GamescopeFilter,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GamescopeSessionConfiguration {
    environment_id: String,
    gamescope_version: String,
    backend_id: String,
    output_width: u32,
    output_height: u32,
    scaling_configuration: GamescopeScalingConfiguration,
}

impl GamescopeSessionConfiguration {
    pub fn capture_provenance(&self) -> Result<GamescopeSessionProvenance, String> {
        GamescopeSessionProvenance::new(GamescopeSessionProvenanceInput {
            environment_id: self.environment_id.clone(),
            gamescope_version: self.gamescope_version.clone(),
            backend_id: self.backend_id.clone(),
            output_width: self.output_width,
            output_height: self.output_height,
            nested_width: self.scaling_configuration.nested_width,
            nested_height: self.scaling_configuration.nested_height,
            nested_refresh_hz: self.scaling_configuration.nested_refresh_hz,
            scaler: scaler_name(self.scaling_configuration.scaler).to_owned(),
            filter: filter_name(self.scaling_configuration.filter).to_owned(),
        })
        .map_err(|_| "Gamescope session provenance is invalid".to_owned())
    }
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
    SessionContractMismatch,
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

#[derive(Serialize)]
struct GamescopeCalibrationSessionSampleManifest<'a> {
    schema: &'static str,
    calibration_state: &'static str,
    source: CaptureSourceKind,
    session_configuration: &'a GamescopeSessionConfiguration,
    observed_video_contract: UncalibratedVideoContract,
    memory_type: UncalibratedMemoryType,
    stride: u32,
    receiver_sequence: u64,
    received_monotonic_ns: u64,
    frame: CalibrationFrameArtifact,
    diagnostic_facts: &'a [CaptureDiagnosticFact],
    dropped_diagnostic_facts: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCalibrationSessionManifest {
    schema: String,
    calibration_state: String,
    source: CaptureSourceKind,
    session_configuration: GamescopeSessionConfiguration,
    observed_video_contract: UncalibratedVideoContract,
    memory_type: UncalibratedMemoryType,
    stride: u32,
    receiver_sequence: u64,
    received_monotonic_ns: u64,
    frame: StoredCalibrationFrameArtifact,
    diagnostic_facts: Vec<StoredCaptureDiagnosticFact>,
    dropped_diagnostic_facts: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCaptureDiagnosticFact {
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    operation: CaptureDiagnosticOperation,
    status: CaptureDiagnosticStatus,
    error_type: Option<CaptureErrorType>,
    detail: StoredCaptureDiagnosticDetail,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StoredCaptureDiagnosticDetail {
    SourceAcquisition {
        source: CaptureSourceKind,
        candidate_count: u32,
        selected_node_id: Option<u32>,
    },
    RegistryDiscovery {
        global_count: u32,
        candidate_count: u32,
    },
    SourceLifetime {
        source: CaptureSourceKind,
        selected_node_id: u32,
        failure_origin: CaptureDiagnosticOperation,
    },
    StreamNegotiation {
        format: String,
        requested_framerate_num: u32,
        requested_framerate_denom: u32,
        width: u32,
        height: u32,
        framerate_num: u32,
        framerate_denom: u32,
        maximum_framerate_num: u32,
        maximum_framerate_denom: u32,
        pixel_aspect_num: u32,
        pixel_aspect_denom: u32,
        chroma_site: u32,
        color_range: u32,
        color_matrix: u32,
        transfer_function: u32,
        color_primaries: u32,
    },
    FirstFrame {
        memory_type: String,
        stride: u32,
        byte_count: u32,
    },
    ProfileBindingAdmission,
    FrameNormalization {
        source_sequence: u64,
    },
    SteadyReception {
        received_frames: u64,
        overwritten_frames: u64,
        last_sequence: Option<u64>,
        maximum_gap_ns: u64,
    },
    ReceiverShutdown {
        received_frames: u64,
        overwritten_frames: u64,
    },
    Shutdown {
        source: CaptureSourceKind,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCalibrationFrameArtifact {
    filename: String,
    byte_count: u64,
    sha256: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum BindingAuthorStatus {
    Success,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum BindingAuthorErrorType {
    CalibrationUnavailable,
    CalibrationDigestMismatch,
    CalibrationInvalid,
    FrameDigestMismatch,
    GeometryInvalid,
    BindingInvalid,
    OutputUnavailable,
    PublicationFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BindingPublicationPoint {
    TemporaryCreated,
    TemporarySynced,
    BindingPublished,
    ParentSynced,
    TemporaryRemoved,
    CleanupSynced,
}

#[derive(Debug, Serialize)]
pub struct GamescopeBindingAuthorReport {
    schema: &'static str,
    status: BindingAuthorStatus,
    error_type: Option<BindingAuthorErrorType>,
    binding_sha256: Option<String>,
    capture_profile_sha256: Option<String>,
}

impl GamescopeBindingAuthorReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, BindingAuthorStatus::Success)
    }
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

#[allow(clippy::too_many_arguments)]
pub fn parse_session_configuration(
    environment_id: &OsStr,
    gamescope_version: &OsStr,
    backend_id: &OsStr,
    output_width: &OsStr,
    output_height: &OsStr,
    nested_width: &OsStr,
    nested_height: &OsStr,
    nested_refresh: &OsStr,
    scaler: &OsStr,
    filter: &OsStr,
) -> Result<GamescopeSessionConfiguration, String> {
    Ok(GamescopeSessionConfiguration {
        environment_id: parse_token(environment_id, "environment ID")?,
        gamescope_version: parse_token(gamescope_version, "Gamescope version")?,
        backend_id: parse_token(backend_id, "Gamescope backend")?,
        output_width: parse_bounded_u32(output_width, "Gamescope output width", MAX_NESTED_WIDTH)?,
        output_height: parse_bounded_u32(
            output_height,
            "Gamescope output height",
            MAX_NESTED_HEIGHT,
        )?,
        scaling_configuration: parse_scaling_configuration(
            nested_width,
            nested_height,
            nested_refresh,
            scaler,
            filter,
        )?,
    })
}

fn parse_token(value: &OsStr, label: &str) -> Result<String, String> {
    let value = value
        .to_str()
        .ok_or_else(|| format!("{label} must be UTF-8"))?;
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
    {
        return Err(format!("{label} must be a bounded identifier"));
    }
    Ok(value.to_owned())
}

#[allow(clippy::too_many_arguments)]
pub fn parse_fractional_geometry(
    left_numerator: &OsStr,
    left_denominator: &OsStr,
    top_numerator: &OsStr,
    top_denominator: &OsStr,
    width_numerator: &OsStr,
    width_denominator: &OsStr,
    height_numerator: &OsStr,
    height_denominator: &OsStr,
) -> Result<FractionalRectangle, String> {
    fn coordinate(numerator: &OsStr, denominator: &OsStr) -> Result<RationalCoordinate, String> {
        let numerator = numerator
            .to_str()
            .ok_or_else(|| "geometry coordinate must be UTF-8".to_owned())?
            .parse::<u32>()
            .map_err(|_| "geometry coordinate must be an integer".to_owned())?;
        let denominator = denominator
            .to_str()
            .ok_or_else(|| "geometry denominator must be UTF-8".to_owned())?
            .parse::<u32>()
            .map_err(|_| "geometry denominator must be an integer".to_owned())?;
        RationalCoordinate::new(numerator, denominator)
            .map_err(|_| "geometry denominator must be non-zero".to_owned())
    }
    Ok(FractionalRectangle::new(
        coordinate(left_numerator, left_denominator)?,
        coordinate(top_numerator, top_denominator)?,
        coordinate(width_numerator, width_denominator)?,
        coordinate(height_numerator, height_denominator)?,
    ))
}

pub fn author_gamescope_profile_binding(
    calibration: &Path,
    expected_calibration_sha256: &str,
    output: &Path,
    geometry: FractionalRectangle,
) -> GamescopeBindingAuthorReport {
    match author_gamescope_profile_binding_inner(
        calibration,
        expected_calibration_sha256,
        output,
        geometry,
    ) {
        Ok((binding_sha256, capture_profile_sha256)) => GamescopeBindingAuthorReport {
            schema: "scorepeek-gamescope-profile-binding-author-report-v1",
            status: BindingAuthorStatus::Success,
            error_type: None,
            binding_sha256: Some(binding_sha256),
            capture_profile_sha256: Some(capture_profile_sha256),
        },
        Err(error_type) => GamescopeBindingAuthorReport {
            schema: "scorepeek-gamescope-profile-binding-author-report-v1",
            status: BindingAuthorStatus::Error,
            error_type: Some(error_type),
            binding_sha256: None,
            capture_profile_sha256: None,
        },
    }
}

fn author_gamescope_profile_binding_inner(
    calibration: &Path,
    expected_calibration_sha256: &str,
    output: &Path,
    geometry: FractionalRectangle,
) -> Result<(String, String), BindingAuthorErrorType> {
    if !valid_sha256(expected_calibration_sha256) {
        return Err(BindingAuthorErrorType::CalibrationDigestMismatch);
    }
    let calibration = resolve_existing_directory(calibration)
        .map_err(|()| BindingAuthorErrorType::CalibrationUnavailable)?;
    verify_complete_calibration_directory(&calibration)?;
    let manifest_bytes = read_bounded_regular_file(&calibration.join(MANIFEST_FILENAME), 64 * 1024)
        .map_err(|()| BindingAuthorErrorType::CalibrationUnavailable)?;
    if encode_sha256(&manifest_bytes) != expected_calibration_sha256 {
        return Err(BindingAuthorErrorType::CalibrationDigestMismatch);
    }
    let manifest: StoredCalibrationSessionManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| BindingAuthorErrorType::CalibrationInvalid)?;
    let mut canonical_manifest =
        serde_json::to_vec(&manifest).map_err(|_| BindingAuthorErrorType::CalibrationInvalid)?;
    canonical_manifest.push(b'\n');
    if canonical_manifest != manifest_bytes {
        return Err(BindingAuthorErrorType::CalibrationInvalid);
    }
    validate_stored_session_manifest(&manifest)?;
    let frame_path = calibration.join(FRAME_FILENAME);
    let frame_bytes = read_bounded_regular_file(&frame_path, 128 * 1024 * 1024)
        .map_err(|()| BindingAuthorErrorType::CalibrationUnavailable)?;
    if u64::try_from(frame_bytes.len()).ok() != Some(manifest.frame.byte_count)
        || encode_sha256(&frame_bytes) != manifest.frame.sha256
    {
        return Err(BindingAuthorErrorType::FrameDigestMismatch);
    }
    let session = manifest.session_configuration;
    let authored = GamescopeProfileBinding::author(GamescopeProfileBindingAuthoringInput {
        calibration_evidence_sha256: expected_calibration_sha256.to_owned(),
        environment_id: session.environment_id,
        gamescope_version: session.gamescope_version,
        backend_id: session.backend_id,
        output_width: session.output_width,
        output_height: session.output_height,
        nested_width: session.scaling_configuration.nested_width,
        nested_height: session.scaling_configuration.nested_height,
        nested_refresh_hz: session.scaling_configuration.nested_refresh_hz,
        scaler: scaler_name(session.scaling_configuration.scaler).to_owned(),
        filter: filter_name(session.scaling_configuration.filter).to_owned(),
        observed_video_contract: manifest.observed_video_contract,
        memory_type: manifest.memory_type,
        stride: manifest.stride,
        geometry,
    })
    .map_err(|error| match error {
        scorepeek::capture::GamescopeProfileBindingError::InvalidNormalizer => {
            BindingAuthorErrorType::GeometryInvalid
        }
        _ => BindingAuthorErrorType::BindingInvalid,
    })?;
    publish_binding(output, &authored.bytes)?;
    Ok((authored.artifact_sha256, authored.capture_profile_sha256))
}

fn validate_stored_session_manifest(
    manifest: &StoredCalibrationSessionManifest,
) -> Result<(), BindingAuthorErrorType> {
    let expected_bytes = u64::from(manifest.stride)
        .checked_mul(u64::from(manifest.observed_video_contract.height))
        .ok_or(BindingAuthorErrorType::CalibrationInvalid)?;
    if manifest.schema != "scorepeek-private-uncalibrated-gamescope-session-sample-v1"
        || manifest.calibration_state != "uncalibrated"
        || manifest.source != CaptureSourceKind::GamescopeDefaultRemote
        || manifest.frame.filename != FRAME_FILENAME
        || !valid_sha256(&manifest.frame.sha256)
        || manifest.frame.byte_count != expected_bytes
        || manifest.session_configuration.output_width != manifest.observed_video_contract.width
        || manifest.session_configuration.output_height != manifest.observed_video_contract.height
        || manifest.receiver_sequence == 0
        || manifest.received_monotonic_ns == 0
        || manifest.diagnostic_facts.len() > MAX_DIAGNOSTIC_FACTS
        || manifest
            .dropped_diagnostic_facts
            .checked_add(u64::try_from(manifest.diagnostic_facts.len()).unwrap_or(u64::MAX))
            .is_none()
    {
        return Err(BindingAuthorErrorType::CalibrationInvalid);
    }
    Ok(())
}

fn verify_complete_calibration_directory(path: &Path) -> Result<(), BindingAuthorErrorType> {
    let mut names = fs::read_dir(path)
        .map_err(|_| BindingAuthorErrorType::CalibrationUnavailable)?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name())
                .map_err(|_| BindingAuthorErrorType::CalibrationUnavailable)
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    let mut expected = vec![
        OsStr::new(FRAME_FILENAME).to_os_string(),
        OsStr::new(MANIFEST_FILENAME).to_os_string(),
        OsStr::new(OWNERSHIP_FILENAME).to_os_string(),
    ];
    expected.sort();
    if names != expected {
        return Err(BindingAuthorErrorType::CalibrationInvalid);
    }
    let ownership = read_bounded_regular_file(&path.join(OWNERSHIP_FILENAME), 128)
        .map_err(|()| BindingAuthorErrorType::CalibrationInvalid)?;
    if ownership != OWNERSHIP_BYTES {
        return Err(BindingAuthorErrorType::CalibrationInvalid);
    }
    Ok(())
}

fn resolve_existing_directory(path: &Path) -> Result<PathBuf, ()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(());
    }
    let metadata = path.symlink_metadata().map_err(|_| ())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(());
    }
    path.canonicalize().map_err(|_| ())
}

fn read_bounded_regular_file(path: &Path, maximum: u64) -> Result<Vec<u8>, ()> {
    let before = path.symlink_metadata().map_err(|_| ())?;
    if !before.is_file() || before.file_type().is_symlink() || before.len() > maximum {
        return Err(());
    }
    let mut file = File::open(path).map_err(|_| ())?;
    let opened = file.metadata().map_err(|_| ())?;
    if opened.dev() != before.dev() || opened.ino() != before.ino() {
        return Err(());
    }
    let mut bytes = Vec::with_capacity(usize::try_from(before.len()).map_err(|_| ())?);
    (&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| ())?;
    if u64::try_from(bytes.len()).map_err(|_| ())? != before.len() {
        return Err(());
    }
    let after = file.metadata().map_err(|_| ())?;
    if after.dev() != before.dev()
        || after.ino() != before.ino()
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
    {
        return Err(());
    }
    Ok(bytes)
}

fn publish_binding(output: &Path, bytes: &[u8]) -> Result<(), BindingAuthorErrorType> {
    publish_binding_with(output, bytes, |_| Ok(()))
}

fn publish_binding_with(
    output: &Path,
    bytes: &[u8],
    mut checkpoint: impl FnMut(BindingPublicationPoint) -> std::io::Result<()>,
) -> Result<(), BindingAuthorErrorType> {
    if !output.is_absolute()
        || output.file_name().is_none()
        || output
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(BindingAuthorErrorType::OutputUnavailable);
    }
    let parent = output
        .parent()
        .ok_or(BindingAuthorErrorType::OutputUnavailable)?;
    let metadata = parent
        .symlink_metadata()
        .map_err(|_| BindingAuthorErrorType::OutputUnavailable)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(BindingAuthorErrorType::OutputUnavailable);
    }
    let canonical_output = parent
        .canonicalize()
        .map_err(|_| BindingAuthorErrorType::OutputUnavailable)?
        .join(
            output
                .file_name()
                .ok_or(BindingAuthorErrorType::OutputUnavailable)?,
        );
    if canonical_output.symlink_metadata().is_ok() {
        let existing = read_bounded_regular_file(&canonical_output, 64 * 1024)
            .map_err(|()| BindingAuthorErrorType::OutputUnavailable)?;
        if existing != bytes {
            return Err(BindingAuthorErrorType::OutputUnavailable);
        }
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| BindingAuthorErrorType::PublicationFailed)?;
        return Ok(());
    }

    let (staging, mut file) = create_binding_staging(parent, &canonical_output)?;
    let staging_metadata = file
        .metadata()
        .map_err(|_| BindingAuthorErrorType::PublicationFailed)?;

    let mut published = false;
    let mut parent_synced = false;
    let publication = (|| {
        checkpoint(BindingPublicationPoint::TemporaryCreated)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        checkpoint(BindingPublicationPoint::TemporarySynced)?;
        fs::hard_link(&staging, &canonical_output)?;
        published = true;
        checkpoint(BindingPublicationPoint::BindingPublished)?;
        File::open(parent)?.sync_all()?;
        parent_synced = true;
        checkpoint(BindingPublicationPoint::ParentSynced)?;
        fs::remove_file(&staging)?;
        checkpoint(BindingPublicationPoint::TemporaryRemoved)?;
        File::open(parent)?.sync_all()?;
        checkpoint(BindingPublicationPoint::CleanupSynced)
    })();
    if publication.is_err() {
        cleanup_binding_publication(
            parent,
            &canonical_output,
            &staging,
            &staging_metadata,
            published,
            parent_synced,
        );
        return Err(BindingAuthorErrorType::PublicationFailed);
    }
    Ok(())
}

fn create_binding_staging(
    parent: &Path,
    output: &Path,
) -> Result<(PathBuf, File), BindingAuthorErrorType> {
    let filename = output
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or(BindingAuthorErrorType::OutputUnavailable)?;
    let process_id = std::process::id();
    let timestamp = SystemTime::UNIX_EPOCH
        .elapsed()
        .map_err(|_| BindingAuthorErrorType::PublicationFailed)?
        .as_nanos();
    for _ in 0..128 {
        let sequence = BINDING_STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let staging = parent.join(format!(
            ".{filename}.scorepeek-gamescope-binding-v1.{process_id}.{timestamp}.{sequence}.staging"
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)
        {
            Ok(file) => return Ok((staging, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(BindingAuthorErrorType::OutputUnavailable),
        }
    }
    Err(BindingAuthorErrorType::OutputUnavailable)
}

fn cleanup_binding_publication(
    parent: &Path,
    output: &Path,
    staging: &Path,
    staging_metadata: &fs::Metadata,
    published: bool,
    parent_synced: bool,
) {
    if published && !parent_synced {
        let _ = remove_matching_inode(output, staging_metadata);
    }
    let _ = remove_matching_inode(staging, staging_metadata);
    let _ = File::open(parent).and_then(|directory| directory.sync_all());
}

fn remove_matching_inode(path: &Path, expected: &fs::Metadata) -> Result<(), ()> {
    match path.symlink_metadata() {
        Ok(actual) => {
            if !actual.is_file()
                || actual.file_type().is_symlink()
                || actual.dev() != expected.dev()
                || actual.ino() != expected.ino()
            {
                return Err(());
            }
            fs::remove_file(path).map_err(|_| ())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

const fn scaler_name(value: GamescopeScaler) -> &'static str {
    match value {
        GamescopeScaler::Auto => "auto",
        GamescopeScaler::Integer => "integer",
        GamescopeScaler::Fit => "fit",
        GamescopeScaler::Fill => "fill",
        GamescopeScaler::Stretch => "stretch",
    }
}

const fn filter_name(value: GamescopeFilter) -> &'static str {
    match value {
        GamescopeFilter::Linear => "linear",
        GamescopeFilter::Nearest => "nearest",
        GamescopeFilter::Fsr => "fsr",
        GamescopeFilter::Nis => "nis",
        GamescopeFilter::Pixel => "pixel",
    }
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

pub fn capture_gamescope_calibration_session_sample(
    output: &Path,
    session_configuration: &GamescopeSessionConfiguration,
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
    if frame.contract().width != session_configuration.output_width
        || frame.contract().height != session_configuration.output_height
    {
        return error_report(
            CalibrationSampleErrorType::SessionContractMismatch,
            None,
            sink,
        );
    }
    publish_session_sample(&output, session_configuration, &frame, sink)
}

fn publish_session_sample(
    output: &Path,
    session_configuration: &GamescopeSessionConfiguration,
    frame: &UncalibratedFrame,
    sink: BoundedDiagnosticSink,
) -> GamescopeCalibrationSampleReport {
    let frame_sha256 = encode_sha256(frame.bytes());
    let manifest = GamescopeCalibrationSessionSampleManifest {
        schema: "scorepeek-private-uncalibrated-gamescope-session-sample-v1",
        calibration_state: "uncalibrated",
        source: CaptureSourceKind::GamescopeDefaultRemote,
        session_configuration,
        observed_video_contract: frame.contract(),
        memory_type: frame.memory_type(),
        stride: frame.stride(),
        receiver_sequence: frame.sequence(),
        received_monotonic_ns: frame.received_monotonic_ns(),
        frame: CalibrationFrameArtifact {
            filename: FRAME_FILENAME,
            byte_count: u64::try_from(frame.bytes().len()).unwrap_or(u64::MAX),
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
    use std::io::{self, Write as _};
    use std::os::unix::fs::symlink;

    use scorepeek::capture::{
        CaptureSourceKind, UncalibratedMemoryType, UncalibratedVideoContract,
    };

    use super::{
        BindingAuthorErrorType, BindingPublicationPoint, CalibrationFrameArtifact, FRAME_FILENAME,
        GamescopeCalibrationSampleManifest, GamescopeCalibrationSessionSampleManifest,
        MANIFEST_FILENAME, MANIFEST_STAGING_FILENAME, OWNERSHIP_BYTES, OWNERSHIP_FILENAME,
        PublicationPoint, StoredCalibrationSessionManifest, author_gamescope_profile_binding_inner,
        cleanup_binding_publication, create_binding_staging, encode_sha256,
        parse_fractional_geometry, parse_scaling_configuration, parse_session_configuration,
        publish_binding_with, publish_sample_with, resolve_output,
    };

    fn tiny_video_contract() -> UncalibratedVideoContract {
        UncalibratedVideoContract {
            width: 2,
            height: 2,
            framerate_num: 0,
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
        }
    }

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
    fn session_configuration_is_explicit_and_bounded() {
        let configuration = parse_session_configuration(
            OsStr::new("development-machine-v1"),
            OsStr::new("3.16.19-128-g7282613+"),
            OsStr::new("sdl"),
            OsStr::new("2556"),
            OsStr::new("1428"),
            OsStr::new("1920"),
            OsStr::new("1080"),
            OsStr::new("120"),
            OsStr::new("auto"),
            OsStr::new("linear"),
        )
        .unwrap();
        assert_eq!(configuration.backend_id, "sdl");
        assert_eq!(configuration.output_width, 2_556);
        assert_eq!(configuration.output_height, 1_428);
        assert!(
            parse_session_configuration(
                OsStr::new("bad value"),
                OsStr::new("3.16.19"),
                OsStr::new("sdl"),
                OsStr::new("2556"),
                OsStr::new("1428"),
                OsStr::new("1920"),
                OsStr::new("1080"),
                OsStr::new("120"),
                OsStr::new("auto"),
                OsStr::new("linear"),
            )
            .is_err()
        );
    }

    #[test]
    fn verified_session_sample_authors_create_only_binding() {
        let parent = tempfile::tempdir().unwrap();
        let calibration = parent.path().join("calibration");
        fs::create_dir(&calibration).unwrap();
        fs::write(calibration.join(OWNERSHIP_FILENAME), OWNERSHIP_BYTES).unwrap();
        let frame = vec![17_u8; 16];
        fs::write(calibration.join(FRAME_FILENAME), &frame).unwrap();
        let session = parse_session_configuration(
            OsStr::new("development-machine-v1"),
            OsStr::new("3.16.19-128-g7282613+"),
            OsStr::new("sdl"),
            OsStr::new("2"),
            OsStr::new("2"),
            OsStr::new("1920"),
            OsStr::new("1080"),
            OsStr::new("120"),
            OsStr::new("auto"),
            OsStr::new("linear"),
        )
        .unwrap();
        let manifest = GamescopeCalibrationSessionSampleManifest {
            schema: "scorepeek-private-uncalibrated-gamescope-session-sample-v1",
            calibration_state: "uncalibrated",
            source: CaptureSourceKind::GamescopeDefaultRemote,
            session_configuration: &session,
            observed_video_contract: tiny_video_contract(),
            memory_type: UncalibratedMemoryType::MemoryFileDescriptor,
            stride: 8,
            receiver_sequence: 1,
            received_monotonic_ns: 42,
            frame: CalibrationFrameArtifact {
                filename: FRAME_FILENAME,
                byte_count: 16,
                sha256: encode_sha256(&frame),
            },
            diagnostic_facts: &[],
            dropped_diagnostic_facts: 0,
        };
        let mut manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        manifest_bytes.push(b'\n');
        fs::write(calibration.join(MANIFEST_FILENAME), &manifest_bytes).unwrap();
        let manifest_digest = encode_sha256(&manifest_bytes);
        let output = parent.path().join("binding.json");
        let geometry = parse_fractional_geometry(
            OsStr::new("0"),
            OsStr::new("1"),
            OsStr::new("0"),
            OsStr::new("1"),
            OsStr::new("2"),
            OsStr::new("1"),
            OsStr::new("2"),
            OsStr::new("1"),
        )
        .unwrap();
        let (binding_digest, profile_digest) = author_gamescope_profile_binding_inner(
            &calibration,
            &manifest_digest,
            &output,
            geometry,
        )
        .unwrap();
        let bytes = fs::read(&output).unwrap();
        let binding =
            scorepeek::capture::GamescopeProfileBinding::parse(&bytes, &binding_digest).unwrap();
        assert_eq!(binding.capture_profile_sha256(), profile_digest);
        assert_eq!(binding.output_width(), 2);
        assert_eq!(binding.output_height(), 2);
        assert_eq!(
            author_gamescope_profile_binding_inner(
                &calibration,
                &manifest_digest,
                &output,
                geometry,
            )
            .unwrap(),
            (binding_digest, profile_digest)
        );

        fs::write(calibration.join(FRAME_FILENAME), vec![18_u8; 16]).unwrap();
        let second_output = parent.path().join("second.json");
        assert_eq!(
            author_gamescope_profile_binding_inner(
                &calibration,
                &manifest_digest,
                &second_output,
                geometry,
            )
            .unwrap_err(),
            BindingAuthorErrorType::FrameDigestMismatch
        );
        assert!(!second_output.exists());
    }

    #[test]
    fn stored_diagnostic_facts_reject_schema_drift() {
        let valid = serde_json::json!({
            "schema": "scorepeek-private-uncalibrated-gamescope-session-sample-v1",
            "calibration_state": "uncalibrated",
            "source": "gamescope_default_remote",
            "session_configuration": {
                "environment_id": "development-machine-v1",
                "gamescope_version": "3.16.19-128-g7282613+",
                "backend_id": "sdl",
                "output_width": 2,
                "output_height": 2,
                "scaling_configuration": {
                    "evidence_kind": "operator_declared",
                    "nested_width": 1920,
                    "nested_height": 1080,
                    "nested_refresh_hz": 120,
                    "scaler": "auto",
                    "filter": "linear"
                }
            },
            "observed_video_contract": tiny_video_contract(),
            "memory_type": "memory_file_descriptor",
            "stride": 8,
            "receiver_sequence": 1,
            "received_monotonic_ns": 42,
            "frame": {
                "filename": FRAME_FILENAME,
                "byte_count": 16,
                "sha256": "a".repeat(64)
            },
            "diagnostic_facts": [{
                "sequence": 1,
                "monotonic_start_ms": 0,
                "monotonic_end_ms": 1,
                "operation": "shutdown",
                "status": "success",
                "error_type": null,
                "detail": {
                    "kind": "shutdown",
                    "source": "gamescope_default_remote"
                }
            }],
            "dropped_diagnostic_facts": 0
        });
        assert!(serde_json::from_value::<StoredCalibrationSessionManifest>(valid.clone()).is_ok());

        let mut unknown_field = valid.clone();
        unknown_field["diagnostic_facts"][0]["detail"]["unexpected"] =
            serde_json::Value::Bool(true);
        let mut unknown_kind = valid.clone();
        unknown_kind["diagnostic_facts"][0]["detail"]["kind"] =
            serde_json::Value::String("unknown".to_owned());
        let mut missing_field = valid.clone();
        missing_field["diagnostic_facts"][0]
            .as_object_mut()
            .unwrap()
            .remove("sequence");
        let mut wrong_type = valid;
        wrong_type["diagnostic_facts"][0]["sequence"] = serde_json::Value::String("one".to_owned());

        for invalid in [unknown_field, unknown_kind, missing_field, wrong_type] {
            assert!(serde_json::from_value::<StoredCalibrationSessionManifest>(invalid).is_err());
        }
    }

    #[test]
    fn binding_publication_recovers_every_interruption_without_clobbering() {
        for failed in [
            BindingPublicationPoint::TemporaryCreated,
            BindingPublicationPoint::TemporarySynced,
            BindingPublicationPoint::BindingPublished,
            BindingPublicationPoint::ParentSynced,
            BindingPublicationPoint::TemporaryRemoved,
            BindingPublicationPoint::CleanupSynced,
        ] {
            let parent = tempfile::tempdir().unwrap();
            let output = parent.path().join("binding.json");
            let bytes = b"canonical binding\n";
            let result = publish_binding_with(&output, bytes, |point| {
                if point == failed {
                    Err(io::Error::other("injected"))
                } else {
                    Ok(())
                }
            });
            assert_eq!(
                result.unwrap_err(),
                BindingAuthorErrorType::PublicationFailed
            );
            if output.exists() {
                assert_eq!(fs::read(&output).unwrap(), bytes);
            }

            publish_binding_with(&output, bytes, |_| Ok(())).unwrap();
            assert_eq!(fs::read(&output).unwrap(), bytes);
            assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1);
            assert_eq!(
                publish_binding_with(&output, b"different\n", |_| Ok(())).unwrap_err(),
                BindingAuthorErrorType::OutputUnavailable
            );
            assert_eq!(fs::read(&output).unwrap(), bytes);
        }
    }

    #[test]
    fn partial_binding_temporary_files_do_not_block_retry() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("binding.json");
        let stale = parent
            .path()
            .join(".binding.json.scorepeek-gamescope-binding-v1.stale.staging");
        fs::write(&stale, b"partial from stopped process").unwrap();

        publish_binding_with(&output, b"canonical binding\n", |_| Ok(())).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"canonical binding\n");
        assert_eq!(fs::read(&stale).unwrap(), b"partial from stopped process");
    }

    #[test]
    fn current_partial_binding_temporary_file_is_cleaned_by_inode() {
        let parent = tempfile::tempdir().unwrap();
        let output = parent.path().join("binding.json");
        let (staging, mut file) = create_binding_staging(parent.path(), &output).unwrap();
        let metadata = file.metadata().unwrap();
        file.write_all(b"partial").unwrap();

        cleanup_binding_publication(parent.path(), &output, &staging, &metadata, false, false);
        assert!(!staging.exists());
        assert!(!output.exists());
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
