use std::ffi::OsStr;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::time::SystemTime;

use scorepeek::capture::{
    AuthoredGamescopeProfileBinding, CaptureDiagnosticFact, CaptureDiagnosticOperation,
    CaptureDiagnosticSink, CaptureDiagnosticStatus, CaptureErrorType, CaptureSourceKind,
    FractionalLinearGeometry, FractionalRectangle, GamescopeProfileBinding,
    GamescopeProfileBindingAuthoringInput, MeasuredGamescopeProfileBindingAuthoringInput,
    RationalCoordinate, UncalibratedFrame, UncalibratedMemoryType, UncalibratedVideoContract,
    acquire_gamescope_source, start_uncalibrated_gamescope_receiver,
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

pub struct GuidedGamescopeProfileInput<'a> {
    pub expected_marker_rgb8: &'a [u8],
}

pub struct GuidedGamescopeProfile {
    pub binding: AuthoredGamescopeProfileBinding,
    pub observed_width: u32,
    pub observed_height: u32,
    pub geometry: FractionalRectangle,
    pub verified_fiducial_count: u32,
}

pub fn capture_guided_gamescope_profile(
    input: &GuidedGamescopeProfileInput<'_>,
) -> Result<GuidedGamescopeProfile, String> {
    let mut sink = BoundedDiagnosticSink::default();
    let lease = acquire_gamescope_source(DISCOVERY_TIMEOUT, &mut sink).map_err(|error| {
        format!(
            "Gamescope source acquisition failed: {:?}",
            error.error_type()
        )
    })?;
    let mut receiver =
        start_uncalibrated_gamescope_receiver(lease, RECEIVER_START_TIMEOUT, &mut sink)
            .map_err(|error| format!("Gamescope receiver failed: {:?}", error.error_type()))?;
    let frame = receiver.take_latest_frame();
    receiver.shutdown(&mut sink).map_err(|error| {
        format!(
            "Gamescope receiver shutdown failed: {:?}",
            error.error_type()
        )
    })?;
    let frame = frame.ok_or_else(|| "Gamescope marker frame was unavailable".to_owned())?;
    let contract = frame.contract();
    let geometry = measure_marker_geometry(&frame)?;
    let normalized = FractionalLinearGeometry::new(contract.width, contract.height, geometry)
        .map_err(|_| "Gamescope marker geometry was invalid".to_owned())?
        .normalize(&frame)
        .map_err(|_| "Gamescope marker normalization failed".to_owned())?;
    let verified_fiducial_count =
        validate_measured_marker(normalized.pixels(), input.expected_marker_rgb8)?;
    let binding =
        GamescopeProfileBinding::author_measured(MeasuredGamescopeProfileBindingAuthoringInput {
            observed_width: contract.width,
            observed_height: contract.height,
            geometry,
        })
        .map_err(|error| format!("Gamescope profile binding was invalid: {error:?}"))?;
    Ok(GuidedGamescopeProfile {
        binding,
        observed_width: contract.width,
        observed_height: contract.height,
        geometry,
        verified_fiducial_count,
    })
}

#[derive(Clone, Copy)]
struct FiducialObservation {
    count: u32,
    sum_x: f64,
    sum_y: f64,
    minimum_x: u32,
    minimum_y: u32,
    maximum_x: u32,
    maximum_y: u32,
}

impl FiducialObservation {
    const fn empty() -> Self {
        Self {
            count: 0,
            sum_x: 0.0,
            sum_y: 0.0,
            minimum_x: u32::MAX,
            minimum_y: u32::MAX,
            maximum_x: 0,
            maximum_y: 0,
        }
    }
}

fn measure_marker_geometry(frame: &UncalibratedFrame) -> Result<FractionalRectangle, String> {
    let contract = frame.contract();
    measure_marker_geometry_bgrx(
        contract.width,
        contract.height,
        frame.stride(),
        frame.bytes(),
    )
}

fn measure_marker_geometry_bgrx(
    width: u32,
    height: u32,
    stride: u32,
    bytes: &[u8],
) -> Result<FractionalRectangle, String> {
    let stride =
        usize::try_from(stride).map_err(|_| "Gamescope marker stride was invalid".to_owned())?;
    let required = stride
        .checked_mul(usize::try_from(height).expect("u32 fits usize"))
        .ok_or_else(|| "Gamescope marker byte length overflowed".to_owned())?;
    if bytes.len() != required || stride < usize::try_from(width).expect("u32 fits usize") * 4 {
        return Err("Gamescope marker byte layout was invalid".to_owned());
    }
    let mut observations = [FiducialObservation::empty(); 9];
    for y in 0..height {
        let row = usize::try_from(y)
            .ok()
            .and_then(|value| value.checked_mul(stride))
            .ok_or_else(|| "Gamescope marker frame offset overflowed".to_owned())?;
        for x in 0..width {
            let offset = row
                .checked_add(usize::try_from(x).expect("u32 fits usize") * 4)
                .ok_or_else(|| "Gamescope marker frame offset overflowed".to_owned())?;
            let bgr = &bytes[offset..offset + 3];
            for (index, &(_, _, rgb)) in crate::calibration_marker::fiducials().iter().enumerate() {
                if bgr[0].abs_diff(rgb[2]) <= 10
                    && bgr[1].abs_diff(rgb[1]) <= 10
                    && bgr[2].abs_diff(rgb[0]) <= 10
                {
                    let observation = &mut observations[index];
                    observation.count += 1;
                    observation.sum_x += f64::from(x);
                    observation.sum_y += f64::from(y);
                    observation.minimum_x = observation.minimum_x.min(x);
                    observation.minimum_y = observation.minimum_y.min(y);
                    observation.maximum_x = observation.maximum_x.max(x);
                    observation.maximum_y = observation.maximum_y.max(y);
                    break;
                }
            }
        }
    }
    let mut points = Vec::with_capacity(observations.len());
    for (observation, &(left, top, _)) in observations
        .iter()
        .zip(crate::calibration_marker::fiducials())
    {
        if observation.count < 16 {
            return Err("Gamescope marker has a missing or unreadable fiducial".to_owned());
        }
        points.push((
            f64::from(left + crate::calibration_marker::FIDUCIAL_SIZE / 2),
            f64::from(top + crate::calibration_marker::FIDUCIAL_SIZE / 2),
            observation.sum_x / f64::from(observation.count) + 0.5,
            observation.sum_y / f64::from(observation.count) + 0.5,
        ));
    }
    let (scale_x, left) = fit_axis(&points, 0, 2)?;
    let (scale_y, top) = fit_axis(&points, 1, 3)?;
    if scale_x <= 0.0 || scale_y <= 0.0 {
        return Err("Gamescope marker is mirrored or rotated".to_owned());
    }
    for &(canonical_x, canonical_y, observed_x, observed_y) in &points {
        if (scale_x.mul_add(canonical_x, left) - observed_x).abs() > 1.0
            || (scale_y.mul_add(canonical_y, top) - observed_y).abs() > 1.0
        {
            return Err(
                "Gamescope marker does not have one axis-aligned linear transform".to_owned(),
            );
        }
    }
    validate_fiducial_bounds(&observations, scale_x, scale_y, left, top)?;
    let rectangle_width = scale_x * f64::from(crate::calibration_marker::WIDTH);
    let rectangle_height = scale_y * f64::from(crate::calibration_marker::HEIGHT);
    finalize_measured_rectangle(width, height, left, top, rectangle_width, rectangle_height)
}

fn finalize_measured_rectangle(
    width: u32,
    height: u32,
    left: f64,
    top: f64,
    rectangle_width: f64,
    rectangle_height: f64,
) -> Result<FractionalRectangle, String> {
    let observed_width = f64::from(width);
    let observed_height = f64::from(height);
    if left < 0.0
        || top < 0.0
        || left + rectangle_width > observed_width
        || top + rectangle_height > observed_height
    {
        return Err(format!(
            "Gamescope marker is cropped and cannot be reconstructed: rectangle=({left:.3},{top:.3},{rectangle_width:.3},{rectangle_height:.3}) frame=({observed_width:.0},{observed_height:.0})"
        ));
    }
    let rectangle = FractionalRectangle::new(
        quantize_coordinate(left)?,
        quantize_coordinate(top)?,
        quantize_coordinate(rectangle_width)?,
        quantize_coordinate(rectangle_height)?,
    );
    FractionalLinearGeometry::new(width, height, rectangle).map_err(|_| {
        "Gamescope marker quantized rectangle is cropped and cannot be reconstructed".to_owned()
    })?;
    Ok(rectangle)
}

fn fit_axis(
    points: &[(f64, f64, f64, f64)],
    canonical_index: usize,
    observed_index: usize,
) -> Result<(f64, f64), String> {
    let value = |point: &(f64, f64, f64, f64), index| match index {
        0 => point.0,
        1 => point.1,
        2 => point.2,
        _ => point.3,
    };
    let count = 9.0;
    let canonical_mean = points
        .iter()
        .map(|point| value(point, canonical_index))
        .sum::<f64>()
        / count;
    let observed_mean = points
        .iter()
        .map(|point| value(point, observed_index))
        .sum::<f64>()
        / count;
    let numerator = points
        .iter()
        .map(|point| {
            (value(point, canonical_index) - canonical_mean)
                * (value(point, observed_index) - observed_mean)
        })
        .sum::<f64>();
    let denominator = points
        .iter()
        .map(|point| (value(point, canonical_index) - canonical_mean).powi(2))
        .sum::<f64>();
    if denominator == 0.0 {
        return Err("Gamescope marker fiducials were degenerate".to_owned());
    }
    let scale = numerator / denominator;
    Ok((scale, observed_mean - scale * canonical_mean))
}

fn validate_fiducial_bounds(
    observations: &[FiducialObservation; 9],
    scale_x: f64,
    scale_y: f64,
    left: f64,
    top: f64,
) -> Result<(), String> {
    for (observation, &(fiducial_left, fiducial_top, _)) in observations
        .iter()
        .zip(crate::calibration_marker::fiducials())
    {
        let expected_left = scale_x.mul_add(f64::from(fiducial_left), left);
        let expected_top = scale_y.mul_add(f64::from(fiducial_top), top);
        let expected_right = scale_x.mul_add(
            f64::from(fiducial_left + crate::calibration_marker::FIDUCIAL_SIZE),
            left,
        );
        let expected_bottom = scale_y.mul_add(
            f64::from(fiducial_top + crate::calibration_marker::FIDUCIAL_SIZE),
            top,
        );
        if (f64::from(observation.minimum_x) + 0.5 - expected_left).abs() > 6.0
            || (f64::from(observation.minimum_y) + 0.5 - expected_top).abs() > 6.0
            || (f64::from(observation.maximum_x) + 0.5 - expected_right).abs() > 6.0
            || (f64::from(observation.maximum_y) + 0.5 - expected_bottom).abs() > 6.0
        {
            return Err("Gamescope marker has a duplicated or malformed fiducial".to_owned());
        }
    }
    Ok(())
}

fn quantize_coordinate(value: f64) -> Result<RationalCoordinate, String> {
    if !value.is_finite() || value < 0.0 || value > f64::from(u32::MAX) / 2_048.0 {
        return Err("Gamescope marker geometry was invalid".to_owned());
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let numerator = (value * 2_048.0).round() as u32;
    RationalCoordinate::new(numerator, 2_048)
        .map_err(|_| "Gamescope marker geometry was invalid".to_owned())
}

fn validate_measured_marker(actual: &[u8], expected: &[u8]) -> Result<u32, String> {
    if actual.len() != expected.len() || actual.len() != 1_920 * 1_080 * 3 {
        return Err("Gamescope marker byte length did not match".to_owned());
    }
    let inset = crate::calibration_marker::FIDUCIAL_SIZE / 4;
    for &(left, top, _) in crate::calibration_marker::fiducials() {
        let mut matching = 0u32;
        let mut count = 0u32;
        for y in top + inset..top + crate::calibration_marker::FIDUCIAL_SIZE - inset {
            for x in left + inset..left + crate::calibration_marker::FIDUCIAL_SIZE - inset {
                let offset =
                    usize::try_from((y * 1_920 + x) * 3).expect("marker offset fits usize");
                count += 1;
                if actual[offset..offset + 3]
                    .iter()
                    .zip(&expected[offset..offset + 3])
                    .all(|(actual, expected)| actual.abs_diff(*expected) <= 16)
                {
                    matching += 1;
                }
            }
        }
        if matching * 10 < count * 9 {
            return Err(format!(
                "Gamescope marker fiducial at ({left},{top}) was not preserved"
            ));
        }
    }
    for (x, y) in [
        (180, 180),
        (660, 300),
        (1_260, 420),
        (1_740, 780),
        (780, 900),
    ] {
        let offset = usize::try_from((y * 1_920 + x) * 3).expect("marker offset fits usize");
        if actual[offset..offset + 3]
            .iter()
            .zip(&expected[offset..offset + 3])
            .any(|(actual, expected)| actual.abs_diff(*expected) > 20)
        {
            return Err(
                "Gamescope marker cell interiors or channel order were not preserved".to_owned(),
            );
        }
    }
    Ok(u32::try_from(crate::calibration_marker::fiducials().len())
        .expect("fiducial count fits u32"))
}

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
    let metadata = path.metadata().map_err(|_| ())?;
    if !metadata.is_dir() {
        return Err(());
    }
    path.canonicalize().map_err(|_| ())
}

fn read_bounded_regular_file(path: &Path, maximum: u64) -> Result<Vec<u8>, ()> {
    let before = path.metadata().map_err(|_| ())?;
    if !before.is_file() || before.len() > maximum {
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
        .metadata()
        .map_err(|_| BindingAuthorErrorType::OutputUnavailable)?;
    if !metadata.is_dir() {
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
    let metadata = parent.metadata().map_err(|_| ())?;
    if !metadata.is_dir() {
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
        finalize_measured_rectangle, measure_marker_geometry_bgrx, parse_fractional_geometry,
        parse_scaling_configuration, parse_session_configuration, publish_binding_with,
        publish_sample_with, resolve_output, validate_measured_marker,
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
    fn measured_geometry_accepts_correctable_axis_aligned_transforms() {
        for (width, height, left, top, scale_x, scale_y) in [
            (1_920, 1_080, 0.0, 0.0, 1.0, 1.0),
            (2_560, 1_440, 64.25, 22.75, 1.2, 1.25),
            (3_840, 2_160, 0.0, 0.0, 2.0, 2.0),
            (3_500, 2_000, 37.5, 81.25, 1.7, 1.6),
        ] {
            let bytes = transformed_marker(width, height, left, top, scale_x, scale_y);
            let rectangle = measure_marker_geometry_bgrx(width, height, width * 4, &bytes)
                .unwrap_or_else(|error| {
                    panic!("{width}x{height} {left},{top} {scale_x}x{scale_y}: {error}")
                });
            assert!((coordinate(rectangle.left()) - left).abs() <= 0.6);
            assert!((coordinate(rectangle.top()) - top).abs() <= 0.6);
            assert!((coordinate(rectangle.width()) - scale_x * 1_920.0).abs() <= 1.2);
            assert!((coordinate(rectangle.height()) - scale_y * 1_080.0).abs() <= 1.2);
        }
    }

    #[test]
    fn measured_geometry_rejects_crop_and_non_axis_aligned_mapping() {
        for (left, top, rectangle_width, rectangle_height) in [
            (-1.0 / 2_048.0, 0.0, 1_920.0, 1_080.0),
            (1.0 / 2_048.0, 0.0, 1_920.0, 1_080.0),
            (0.0, -1.0 / 2_048.0, 1_920.0, 1_080.0),
            (0.0, 1.0 / 2_048.0, 1_920.0, 1_080.0),
            (-1.0, 0.0, 1_920.0, 1_080.0),
            (1.0, 0.0, 1_920.0, 1_080.0),
            (0.0, -1.0, 1_920.0, 1_080.0),
            (0.0, 1.0, 1_920.0, 1_080.0),
        ] {
            assert!(
                finalize_measured_rectangle(
                    1_920,
                    1_080,
                    left,
                    top,
                    rectangle_width,
                    rectangle_height,
                )
                .unwrap_err()
                .contains("cropped")
            );
        }

        for (width, height, left, top) in [
            (1_920, 1_080, -8.0, 0.0),
            (1_920, 1_080, 8.0, 0.0),
            (1_920, 1_080, 0.0, -8.0),
            (1_920, 1_080, 0.0, 8.0),
        ] {
            let cropped = transformed_marker(width, height, left, top, 1.0, 1.0);
            assert!(
                measure_marker_geometry_bgrx(width, height, width * 4, &cropped)
                    .unwrap_err()
                    .contains("cropped")
            );
        }

        let mut sheared = transformed_marker(3_840, 2_200, 0.0, 0.0, 2.0, 2.0);
        for y in 1_100usize..2_200 {
            let row = y * 3_840 * 4;
            sheared[row..row + 3_840 * 4].rotate_right(16);
        }
        assert!(measure_marker_geometry_bgrx(3_840, 2_200, 3_840 * 4, &sheared).is_err());
    }

    #[test]
    fn marker_detection_tolerates_bounded_filter_edges_and_rejects_bad_fiducials() {
        let marker = crate::calibration_marker::rgb8();
        let softened = soften_marker_edges(&marker, 2);
        let filtered = transformed_marker_from(&softened, 3_840, 2_160, 0.0, 0.0, 2.0, 2.0);
        measure_marker_geometry_bgrx(3_840, 2_160, 3_840 * 4, &filtered).unwrap();
        assert_eq!(validate_measured_marker(&softened, &marker).unwrap(), 9);

        let mut missing = marker.to_vec();
        let &(left, top, _) = &crate::calibration_marker::fiducials()[0];
        for y in top..top + crate::calibration_marker::FIDUCIAL_SIZE {
            for x in left..left + crate::calibration_marker::FIDUCIAL_SIZE {
                let offset = ((y * 1_920 + x) * 3) as usize;
                missing[offset..offset + 3].fill(32);
            }
        }
        let missing = transformed_marker_from(&missing, 1_920, 1_080, 0.0, 0.0, 1.0, 1.0);
        assert!(measure_marker_geometry_bgrx(1_920, 1_080, 1_920 * 4, &missing).is_err());

        let mut duplicate = marker.to_vec();
        let duplicate_color = crate::calibration_marker::fiducials()[0].2;
        for y in 400u32..448 {
            for x in 400u32..448 {
                let offset = ((y * 1_920 + x) * 3) as usize;
                duplicate[offset..offset + 3].copy_from_slice(&duplicate_color);
            }
        }
        let duplicate = transformed_marker_from(&duplicate, 1_920, 1_080, 0.0, 0.0, 1.0, 1.0);
        assert!(measure_marker_geometry_bgrx(1_920, 1_080, 1_920 * 4, &duplicate).is_err());

        let mut wrong_channels = transformed_marker(1_920, 1_080, 0.0, 0.0, 1.0, 1.0);
        for pixel in wrong_channels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        assert!(measure_marker_geometry_bgrx(1_920, 1_080, 1_920 * 4, &wrong_channels).is_err());

        let mut mirrored = transformed_marker(1_920, 1_080, 0.0, 0.0, 1.0, 1.0);
        for row in mirrored.chunks_exact_mut(1_920 * 4) {
            for x in 0..960 {
                let opposite = 1_919 - x;
                for channel in 0..4 {
                    row.swap(x * 4 + channel, opposite * 4 + channel);
                }
            }
        }
        assert!(measure_marker_geometry_bgrx(1_920, 1_080, 1_920 * 4, &mirrored).is_err());
    }

    fn coordinate(value: scorepeek::capture::RationalCoordinate) -> f64 {
        f64::from(value.numerator()) / f64::from(value.denominator())
    }

    fn transformed_marker(
        width: u32,
        height: u32,
        left: f64,
        top: f64,
        scale_x: f64,
        scale_y: f64,
    ) -> Vec<u8> {
        let marker = crate::calibration_marker::rgb8();
        transformed_marker_from(&marker, width, height, left, top, scale_x, scale_y)
    }

    fn transformed_marker_from(
        marker: &[u8],
        width: u32,
        height: u32,
        left: f64,
        top: f64,
        scale_x: f64,
        scale_y: f64,
    ) -> Vec<u8> {
        let mut bytes = vec![0u8; (width * height * 4) as usize];
        for y in 0..height {
            for x in 0..width {
                let canonical_x = ((f64::from(x) + 0.5 - left) / scale_x - 0.5).round();
                let canonical_y = ((f64::from(y) + 0.5 - top) / scale_y - 0.5).round();
                let destination = ((y * width + x) * 4) as usize;
                if (0.0..1_920.0).contains(&canonical_x) && (0.0..1_080.0).contains(&canonical_y) {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let canonical_x = canonical_x as u32;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let canonical_y = canonical_y as u32;
                    let source = ((canonical_y * 1_920 + canonical_x) * 3) as usize;
                    bytes[destination..destination + 4].copy_from_slice(&[
                        marker[source + 2],
                        marker[source + 1],
                        marker[source],
                        0,
                    ]);
                }
            }
        }
        bytes
    }

    fn soften_marker_edges(marker: &[u8], radius: u32) -> Box<[u8]> {
        let mut softened = marker.to_vec();
        for y in radius..1_080 - radius {
            for x in radius..1_920 - radius {
                let center = ((y * 1_920 + x) * 3) as usize;
                let different_neighbor = [
                    (x - radius, y),
                    (x + radius, y),
                    (x, y - radius),
                    (x, y + radius),
                ]
                .into_iter()
                .any(|(neighbor_x, neighbor_y)| {
                    let neighbor = ((neighbor_y * 1_920 + neighbor_x) * 3) as usize;
                    marker[center..center + 3] != marker[neighbor..neighbor + 3]
                });
                if different_neighbor {
                    for channel in 0..3 {
                        let mut total = 0u32;
                        let mut count = 0u32;
                        for sample_y in y - radius..=y + radius {
                            for sample_x in x - radius..=x + radius {
                                let sample = ((sample_y * 1_920 + sample_x) * 3) as usize;
                                total += u32::from(marker[sample + channel]);
                                count += 1;
                            }
                        }
                        softened[center + channel] = u8::try_from(total / count).unwrap();
                    }
                }
            }
        }
        softened.into_boxed_slice()
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
