use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::{
    FractionalLinearGeometry, FractionalRectangle, RationalCoordinate, UncalibratedMemoryType,
    UncalibratedVideoContract,
};

const BINDING_SCHEMA: &str = "scorepeek-gamescope-profile-binding-v1";
const LOCAL_BINDING_SCHEMA: &str = "scorepeek-gamescope-profile-binding-v2";
const MEASURED_BINDING_SCHEMA: &str = "scorepeek-gamescope-profile-binding-v3";
const MEASURED_PROFILE_SCHEMA: &str = "scorepeek-gamescope-capture-profile-v2";
const PROFILE_SCHEMA: &str = "scorepeek-gamescope-capture-profile-v1";
const NORMALIZER_SCHEMA: &str = "scorepeek-fractional-linear-normalizer-v1";
const CANONICAL_FRAME_CONTRACT_ID: &str = "scorepeek-canonical-rgb8-1920x1080-v1";
const NORMALIZER_IMPLEMENTATION: &str = "scorepeek-fractional-linear-half-pixel-q11-v1";
const MAX_ARTIFACT_BYTES: usize = 64 * 1024;
const MAX_FRAME_BYTES: u64 = 128 * 1024 * 1024;
const BGRX_BYTES_PER_PIXEL: u64 = 4;
const MAX_TOKEN_BYTES: usize = 128;
const MAX_WIDTH: u32 = 7_680;
const MAX_HEIGHT: u32 = 4_320;
const MAX_GAMESCOPE_ARGUMENTS: usize = 128;
const MAX_GAMESCOPE_ARGUMENT_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GamescopeProfileBindingError {
    ArtifactTooLarge,
    InvalidDigest,
    DigestMismatch,
    InvalidDocument,
    NonCanonicalDocument,
    UnsupportedSchema,
    InvalidProfile,
    InvalidObservedContract,
    InvalidNormalizer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservedContractMismatch {
    Video,
    MemoryType,
    Stride,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GamescopeSessionProvenanceMismatch {
    Environment,
    GamescopeVersion,
    Backend,
    OutputDimensions,
    NestedDimensions,
    NestedRefresh,
    Scaler,
    Filter,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GamescopeSessionProvenanceInput {
    pub environment_id: String,
    pub gamescope_version: String,
    pub backend_id: String,
    pub output_width: u32,
    pub output_height: u32,
    pub nested_width: u32,
    pub nested_height: u32,
    pub nested_refresh_hz: u32,
    pub scaler: String,
    pub filter: String,
}

/// Explicit operator-owned provenance for one newly acquired default-remote Gamescope session.
///
/// This value records the exact launch contract; it does not infer provenance from `PipeWire` caps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GamescopeSessionProvenance {
    provider: GamescopeProviderProvenance,
}

impl GamescopeSessionProvenance {
    /// Validates one bounded explicit session contract.
    ///
    /// # Errors
    /// Returns `InvalidProfile` for an invalid identifier, dimension, refresh, scaler, or filter.
    pub fn new(
        input: GamescopeSessionProvenanceInput,
    ) -> Result<Self, GamescopeProfileBindingError> {
        let provider = GamescopeProviderProvenance {
            source: ProviderSource::GamescopeDefaultRemote,
            environment_id: input.environment_id,
            gamescope_version: input.gamescope_version,
            backend_id: input.backend_id,
            scaling_configuration: ScalingConfiguration {
                output_width: input.output_width,
                output_height: input.output_height,
                nested_width: input.nested_width,
                nested_height: input.nested_height,
                nested_refresh_hz: input.nested_refresh_hz,
                scaler: parse_scaler(&input.scaler)?,
                filter: parse_filter(&input.filter)?,
            },
        };
        provider.validate()?;
        Ok(Self { provider })
    }
}

/// Trusted, operator-supplied inputs used to author one immutable Gamescope binding.
///
/// The calibration artifact reader owns filesystem validation. This pure boundary validates and
/// canonicalizes only the already verified provenance, observed contract, and explicit geometry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GamescopeProfileBindingAuthoringInput {
    pub calibration_evidence_sha256: String,
    pub environment_id: String,
    pub gamescope_version: String,
    pub backend_id: String,
    pub output_width: u32,
    pub output_height: u32,
    pub nested_width: u32,
    pub nested_height: u32,
    pub nested_refresh_hz: u32,
    pub scaler: String,
    pub filter: String,
    pub observed_video_contract: UncalibratedVideoContract,
    pub memory_type: UncalibratedMemoryType,
    pub stride: u32,
    pub geometry: FractionalRectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MeasuredGamescopeProfileBindingAuthoringInput {
    pub observed_width: u32,
    pub observed_height: u32,
    pub geometry: FractionalRectangle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoredGamescopeProfileBinding {
    pub bytes: Vec<u8>,
    pub artifact_sha256: String,
    pub capture_profile_sha256: String,
}

/// An immutable, digest-pinned Gamescope capture-profile and normalizer binding.
///
/// Construction requires canonical artifact bytes and their independently selected SHA-256. The
/// profile identity is the SHA-256 of the canonical capture-profile subdocument; it is never
/// derived from negotiated caps alone. Parsing does not read files or record diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GamescopeProfileBinding {
    capture_profile_sha256: String,
    normalizer_artifact_sha256: String,
    environment_id: String,
    gamescope_version: String,
    backend_id: String,
    scaling_configuration: ScalingConfiguration,
    observed: ObservedContract,
    geometry: FractionalLinearGeometry,
    gamescope_arguments: Option<Vec<String>>,
    measured: bool,
}

impl GamescopeProfileBinding {
    /// Authors the minimal machine-local profile produced by marker measurement.
    ///
    /// # Errors
    /// Returns a stable validation error when dimensions or geometry cannot form the fixed
    /// observed and canonical contracts.
    pub fn author_measured(
        input: MeasuredGamescopeProfileBindingAuthoringInput,
    ) -> Result<AuthoredGamescopeProfileBinding, GamescopeProfileBindingError> {
        let capture_profile = MeasuredCaptureProfileArtifact {
            schema: MEASURED_PROFILE_SCHEMA.to_owned(),
            source: ProviderSource::GamescopeDefaultRemote,
            pixel_format: PixelFormat::Bgrx,
            observed_width: input.observed_width,
            observed_height: input.observed_height,
        };
        capture_profile.validate()?;
        let capture_profile_bytes = canonical_json(&capture_profile)
            .map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
        let capture_profile_sha256 = sha256(&capture_profile_bytes);
        let artifact = MeasuredBindingArtifact {
            schema: MEASURED_BINDING_SCHEMA.to_owned(),
            capture_profile,
            normalizer: NormalizerArtifact {
                schema: NORMALIZER_SCHEMA.to_owned(),
                capture_profile_sha256: capture_profile_sha256.clone(),
                canonical_frame_contract_id: CANONICAL_FRAME_CONTRACT_ID.to_owned(),
                implementation: NORMALIZER_IMPLEMENTATION.to_owned(),
                source: FractionalRectangleArtifact::from_rectangle(input.geometry),
            },
        };
        artifact.validate()?;
        let bytes =
            canonical_json(&artifact).map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
        let artifact_sha256 = sha256(&bytes);
        Self::parse(&bytes, &artifact_sha256)?;
        Ok(AuthoredGamescopeProfileBinding {
            bytes,
            artifact_sha256,
            capture_profile_sha256,
        })
    }
    /// Authors canonical binding bytes from separately verified calibration evidence.
    ///
    /// # Errors
    /// Returns the same stable validation errors used by runtime parsing. No filesystem access or
    /// diagnostic recording occurs here.
    pub fn author(
        input: GamescopeProfileBindingAuthoringInput,
    ) -> Result<AuthoredGamescopeProfileBinding, GamescopeProfileBindingError> {
        Self::author_inner(input, None)
    }

    /// Authors a machine-local binding that also retains the exact Gamescope argument vector.
    ///
    /// # Errors
    /// Returns the standard binding validation errors, including `InvalidProfile` when the
    /// argument vector exceeds its bounded local configuration contract.
    pub fn author_local(
        input: GamescopeProfileBindingAuthoringInput,
        gamescope_arguments: Vec<String>,
    ) -> Result<AuthoredGamescopeProfileBinding, GamescopeProfileBindingError> {
        Self::author_inner(input, Some(gamescope_arguments))
    }

    fn author_inner(
        input: GamescopeProfileBindingAuthoringInput,
        gamescope_arguments: Option<Vec<String>>,
    ) -> Result<AuthoredGamescopeProfileBinding, GamescopeProfileBindingError> {
        let scaler = parse_scaler(&input.scaler)?;
        let filter = parse_filter(&input.filter)?;
        let capture_profile = CaptureProfileArtifact {
            schema: PROFILE_SCHEMA.to_owned(),
            provider: GamescopeProviderProvenance {
                source: ProviderSource::GamescopeDefaultRemote,
                environment_id: input.environment_id,
                gamescope_version: input.gamescope_version,
                backend_id: input.backend_id,
                scaling_configuration: ScalingConfiguration {
                    output_width: input.output_width,
                    output_height: input.output_height,
                    nested_width: input.nested_width,
                    nested_height: input.nested_height,
                    nested_refresh_hz: input.nested_refresh_hz,
                    scaler,
                    filter,
                },
            },
            observed: ObservedContract {
                pixel_format: PixelFormat::Bgrx,
                video: input.observed_video_contract,
                memory_type: input.memory_type,
                stride: input.stride,
            },
            calibration_evidence_sha256: input.calibration_evidence_sha256,
        };
        let capture_profile_bytes = canonical_json(&capture_profile)
            .map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
        let capture_profile_sha256 = sha256(&capture_profile_bytes);
        let source = FractionalRectangleArtifact::from_rectangle(input.geometry);
        let artifact = BindingArtifact {
            schema: if gamescope_arguments.is_some() {
                LOCAL_BINDING_SCHEMA.to_owned()
            } else {
                BINDING_SCHEMA.to_owned()
            },
            capture_profile,
            normalizer: NormalizerArtifact {
                schema: NORMALIZER_SCHEMA.to_owned(),
                capture_profile_sha256: capture_profile_sha256.clone(),
                canonical_frame_contract_id: CANONICAL_FRAME_CONTRACT_ID.to_owned(),
                implementation: NORMALIZER_IMPLEMENTATION.to_owned(),
                source,
            },
            gamescope_arguments,
        };
        artifact.validate()?;
        let bytes =
            canonical_json(&artifact).map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(GamescopeProfileBindingError::ArtifactTooLarge);
        }
        let artifact_sha256 = sha256(&bytes);
        Self::parse(&bytes, &artifact_sha256)?;
        Ok(AuthoredGamescopeProfileBinding {
            bytes,
            artifact_sha256,
            capture_profile_sha256,
        })
    }

    /// Parses one canonical immutable binding selected by its expected SHA-256.
    ///
    /// # Errors
    /// Returns a stable typed error for an over-capacity artifact, invalid digest, non-canonical or
    /// unsupported document, invalid profile provenance/contract, or invalid normalizer binding.
    pub fn parse(
        bytes: &[u8],
        expected_sha256: &str,
    ) -> Result<Self, GamescopeProfileBindingError> {
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(GamescopeProfileBindingError::ArtifactTooLarge);
        }
        if bytes.is_empty() {
            return Err(GamescopeProfileBindingError::InvalidDocument);
        }
        if !valid_sha256(expected_sha256) {
            return Err(GamescopeProfileBindingError::InvalidDigest);
        }
        if sha256(bytes) != expected_sha256 {
            return Err(GamescopeProfileBindingError::DigestMismatch);
        }
        let document: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
        if document.get("schema").and_then(serde_json::Value::as_str)
            == Some(MEASURED_BINDING_SCHEMA)
        {
            let artifact: MeasuredBindingArtifact = serde_json::from_value(document)
                .map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
            let canonical = canonical_json(&artifact)
                .map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
            if canonical != bytes {
                return Err(GamescopeProfileBindingError::NonCanonicalDocument);
            }
            artifact.validate()?;
            let profile_bytes = canonical_json(&artifact.capture_profile)
                .map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
            let capture_profile_sha256 = sha256(&profile_bytes);
            if artifact.normalizer.capture_profile_sha256 != capture_profile_sha256 {
                return Err(GamescopeProfileBindingError::InvalidNormalizer);
            }
            let observed = ObservedContract::measured(
                artifact.capture_profile.observed_width,
                artifact.capture_profile.observed_height,
            )?;
            let geometry = artifact.normalizer.geometry(&observed)?;
            return Ok(Self {
                capture_profile_sha256,
                normalizer_artifact_sha256: expected_sha256.to_owned(),
                environment_id: String::new(),
                gamescope_version: String::new(),
                backend_id: String::new(),
                scaling_configuration: ScalingConfiguration::measured(
                    observed.video.width,
                    observed.video.height,
                ),
                observed,
                geometry,
                gamescope_arguments: None,
                measured: true,
            });
        }
        let artifact: BindingArtifact = serde_json::from_value(document)
            .map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
        let canonical =
            canonical_json(&artifact).map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
        if canonical != bytes {
            return Err(GamescopeProfileBindingError::NonCanonicalDocument);
        }
        artifact.validate()?;

        let profile_bytes = canonical_json(&artifact.capture_profile)
            .map_err(|_| GamescopeProfileBindingError::InvalidDocument)?;
        let capture_profile_sha256 = sha256(&profile_bytes);
        if artifact.normalizer.capture_profile_sha256 != capture_profile_sha256 {
            return Err(GamescopeProfileBindingError::InvalidNormalizer);
        }
        let geometry = artifact
            .normalizer
            .geometry(&artifact.capture_profile.observed)?;

        Ok(Self {
            capture_profile_sha256,
            normalizer_artifact_sha256: expected_sha256.to_owned(),
            environment_id: artifact.capture_profile.provider.environment_id,
            gamescope_version: artifact.capture_profile.provider.gamescope_version,
            backend_id: artifact.capture_profile.provider.backend_id,
            scaling_configuration: artifact.capture_profile.provider.scaling_configuration,
            observed: artifact.capture_profile.observed,
            geometry,
            gamescope_arguments: artifact.gamescope_arguments,
            measured: false,
        })
    }

    #[must_use]
    pub fn capture_profile_sha256(&self) -> &str {
        &self.capture_profile_sha256
    }

    #[must_use]
    pub fn normalizer_artifact_sha256(&self) -> &str {
        &self.normalizer_artifact_sha256
    }

    #[must_use]
    pub fn environment_id(&self) -> &str {
        &self.environment_id
    }

    #[must_use]
    pub fn gamescope_version(&self) -> &str {
        &self.gamescope_version
    }

    #[must_use]
    pub fn backend_id(&self) -> &str {
        &self.backend_id
    }

    #[must_use]
    pub const fn geometry(&self) -> FractionalLinearGeometry {
        self.geometry
    }

    #[must_use]
    pub const fn is_measured(&self) -> bool {
        self.measured
    }

    #[must_use]
    pub const fn observed_width(&self) -> u32 {
        self.observed.video.width
    }

    #[must_use]
    pub const fn observed_height(&self) -> u32 {
        self.observed.video.height
    }

    #[must_use]
    pub const fn source_rectangle(&self) -> FractionalRectangle {
        self.geometry.source_rectangle()
    }

    /// Compares a newly negotiated receiver contract with every registered observed field.
    ///
    /// # Errors
    /// Returns the first stable mismatch category. No field is relaxed or inferred.
    pub fn verify_observed_contract(
        &self,
        video: UncalibratedVideoContract,
        memory_type: UncalibratedMemoryType,
        stride: u32,
    ) -> Result<(), ObservedContractMismatch> {
        if video.width != self.observed.video.width || video.height != self.observed.video.height {
            return Err(ObservedContractMismatch::Video);
        }
        let _ = (memory_type, stride);
        Ok(())
    }

    /// Compares every explicit property of a newly acquired Gamescope session with this profile.
    ///
    /// # Errors
    /// Returns the first stable mismatch category. `PipeWire` observations are not used to infer or
    /// repair session provenance.
    pub fn verify_session_provenance(
        &self,
        session: &GamescopeSessionProvenance,
    ) -> Result<(), GamescopeSessionProvenanceMismatch> {
        let expected = &self.scaling_configuration;
        let actual = &session.provider.scaling_configuration;
        if self.environment_id != session.provider.environment_id {
            return Err(GamescopeSessionProvenanceMismatch::Environment);
        }
        if self.gamescope_version != session.provider.gamescope_version {
            return Err(GamescopeSessionProvenanceMismatch::GamescopeVersion);
        }
        if self.backend_id != session.provider.backend_id {
            return Err(GamescopeSessionProvenanceMismatch::Backend);
        }
        if expected.output_width != actual.output_width
            || expected.output_height != actual.output_height
        {
            return Err(GamescopeSessionProvenanceMismatch::OutputDimensions);
        }
        if expected.nested_width != actual.nested_width
            || expected.nested_height != actual.nested_height
        {
            return Err(GamescopeSessionProvenanceMismatch::NestedDimensions);
        }
        if expected.nested_refresh_hz != actual.nested_refresh_hz {
            return Err(GamescopeSessionProvenanceMismatch::NestedRefresh);
        }
        if expected.scaler != actual.scaler {
            return Err(GamescopeSessionProvenanceMismatch::Scaler);
        }
        if expected.filter != actual.filter {
            return Err(GamescopeSessionProvenanceMismatch::Filter);
        }
        Ok(())
    }

    #[must_use]
    pub const fn nested_width(&self) -> u32 {
        self.scaling_configuration.nested_width
    }

    #[must_use]
    pub const fn nested_height(&self) -> u32 {
        self.scaling_configuration.nested_height
    }

    #[must_use]
    pub const fn nested_refresh_hz(&self) -> u32 {
        self.scaling_configuration.nested_refresh_hz
    }

    #[must_use]
    pub const fn output_width(&self) -> u32 {
        self.scaling_configuration.output_width
    }

    #[must_use]
    pub const fn output_height(&self) -> u32 {
        self.scaling_configuration.output_height
    }

    #[must_use]
    pub const fn scaler(&self) -> &'static str {
        match self.scaling_configuration.scaler {
            GamescopeScaler::Auto => "auto",
            GamescopeScaler::Integer => "integer",
            GamescopeScaler::Fit => "fit",
            GamescopeScaler::Fill => "fill",
            GamescopeScaler::Stretch => "stretch",
        }
    }

    #[must_use]
    pub const fn filter(&self) -> &'static str {
        match self.scaling_configuration.filter {
            GamescopeFilter::Linear => "linear",
            GamescopeFilter::Nearest => "nearest",
            GamescopeFilter::Fsr => "fsr",
            GamescopeFilter::Nis => "nis",
            GamescopeFilter::Pixel => "pixel",
        }
    }

    #[must_use]
    pub fn gamescope_arguments(&self) -> Option<&[String]> {
        self.gamescope_arguments.as_deref()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct MeasuredBindingArtifact {
    schema: String,
    capture_profile: MeasuredCaptureProfileArtifact,
    normalizer: NormalizerArtifact,
}

impl MeasuredBindingArtifact {
    fn validate(&self) -> Result<(), GamescopeProfileBindingError> {
        if self.schema != MEASURED_BINDING_SCHEMA {
            return Err(GamescopeProfileBindingError::UnsupportedSchema);
        }
        self.capture_profile.validate()?;
        self.normalizer.validate()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct MeasuredCaptureProfileArtifact {
    schema: String,
    source: ProviderSource,
    pixel_format: PixelFormat,
    observed_width: u32,
    observed_height: u32,
}

impl MeasuredCaptureProfileArtifact {
    fn validate(&self) -> Result<(), GamescopeProfileBindingError> {
        if self.schema != MEASURED_PROFILE_SCHEMA
            || self.source != ProviderSource::GamescopeDefaultRemote
            || self.pixel_format != PixelFormat::Bgrx
            || self.observed_width == 0
            || self.observed_width > MAX_WIDTH
            || self.observed_height == 0
            || self.observed_height > MAX_HEIGHT
        {
            return Err(GamescopeProfileBindingError::InvalidProfile);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BindingArtifact {
    schema: String,
    capture_profile: CaptureProfileArtifact,
    normalizer: NormalizerArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gamescope_arguments: Option<Vec<String>>,
}

impl BindingArtifact {
    fn validate(&self) -> Result<(), GamescopeProfileBindingError> {
        if self.schema != BINDING_SCHEMA && self.schema != LOCAL_BINDING_SCHEMA {
            return Err(GamescopeProfileBindingError::UnsupportedSchema);
        }
        match (&*self.schema, &self.gamescope_arguments) {
            (BINDING_SCHEMA, None) => {}
            (LOCAL_BINDING_SCHEMA, Some(arguments)) if valid_gamescope_arguments(arguments) => {}
            _ => return Err(GamescopeProfileBindingError::InvalidProfile),
        }
        self.capture_profile.validate()?;
        self.normalizer.validate()
    }
}

fn valid_gamescope_arguments(arguments: &[String]) -> bool {
    arguments.len() <= MAX_GAMESCOPE_ARGUMENTS
        && arguments
            .iter()
            .try_fold(0usize, |total, argument| {
                total.checked_add(argument.len()).filter(|next| {
                    *next <= MAX_GAMESCOPE_ARGUMENT_BYTES && !argument.as_bytes().contains(&0)
                })
            })
            .is_some()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CaptureProfileArtifact {
    schema: String,
    provider: GamescopeProviderProvenance,
    observed: ObservedContract,
    calibration_evidence_sha256: String,
}

impl CaptureProfileArtifact {
    fn validate(&self) -> Result<(), GamescopeProfileBindingError> {
        if self.schema != PROFILE_SCHEMA {
            return Err(GamescopeProfileBindingError::UnsupportedSchema);
        }
        self.provider.validate()?;
        if !valid_sha256(&self.calibration_evidence_sha256) {
            return Err(GamescopeProfileBindingError::InvalidProfile);
        }
        self.observed.validate()?;
        if self.provider.scaling_configuration.output_width != self.observed.video.width
            || self.provider.scaling_configuration.output_height != self.observed.video.height
        {
            return Err(GamescopeProfileBindingError::InvalidProfile);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct GamescopeProviderProvenance {
    source: ProviderSource,
    environment_id: String,
    gamescope_version: String,
    backend_id: String,
    scaling_configuration: ScalingConfiguration,
}

impl GamescopeProviderProvenance {
    fn validate(&self) -> Result<(), GamescopeProfileBindingError> {
        for value in [
            &self.environment_id,
            &self.gamescope_version,
            &self.backend_id,
        ] {
            if !valid_token(value) {
                return Err(GamescopeProfileBindingError::InvalidProfile);
            }
        }
        self.scaling_configuration.validate()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProviderSource {
    GamescopeDefaultRemote,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ScalingConfiguration {
    output_width: u32,
    output_height: u32,
    nested_width: u32,
    nested_height: u32,
    nested_refresh_hz: u32,
    scaler: GamescopeScaler,
    filter: GamescopeFilter,
}

impl ScalingConfiguration {
    const fn measured(width: u32, height: u32) -> Self {
        Self {
            output_width: width,
            output_height: height,
            nested_width: 1,
            nested_height: 1,
            nested_refresh_hz: 1,
            scaler: GamescopeScaler::Auto,
            filter: GamescopeFilter::Linear,
        }
    }

    fn validate(&self) -> Result<(), GamescopeProfileBindingError> {
        if self.output_width == 0
            || self.output_width > 7_680
            || self.output_height == 0
            || self.output_height > 4_320
            || self.nested_width == 0
            || self.nested_width > 7_680
            || self.nested_height == 0
            || self.nested_height > 4_320
            || self.nested_refresh_hz == 0
            || self.nested_refresh_hz > 1_000
        {
            return Err(GamescopeProfileBindingError::InvalidProfile);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum GamescopeScaler {
    Auto,
    Integer,
    Fit,
    Fill,
    Stretch,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum GamescopeFilter {
    Linear,
    Nearest,
    Fsr,
    Nis,
    Pixel,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct ObservedContract {
    pixel_format: PixelFormat,
    video: UncalibratedVideoContract,
    memory_type: UncalibratedMemoryType,
    stride: u32,
}

impl ObservedContract {
    fn measured(width: u32, height: u32) -> Result<Self, GamescopeProfileBindingError> {
        let stride = width
            .checked_mul(4)
            .ok_or(GamescopeProfileBindingError::InvalidObservedContract)?;
        let observed = Self {
            pixel_format: PixelFormat::Bgrx,
            video: UncalibratedVideoContract {
                width,
                height,
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
            },
            memory_type: UncalibratedMemoryType::MemoryFileDescriptor,
            stride,
        };
        observed.validate()?;
        Ok(observed)
    }

    fn validate(&self) -> Result<(), GamescopeProfileBindingError> {
        if self.video.width == 0
            || self.video.width > MAX_WIDTH
            || self.video.height == 0
            || self.video.height > MAX_HEIGHT
        {
            return Err(GamescopeProfileBindingError::InvalidObservedContract);
        }
        let minimum_stride = u64::from(self.video.width)
            .checked_mul(BGRX_BYTES_PER_PIXEL)
            .ok_or(GamescopeProfileBindingError::InvalidObservedContract)?;
        let byte_count = u64::from(self.stride)
            .checked_mul(u64::from(self.video.height))
            .ok_or(GamescopeProfileBindingError::InvalidObservedContract)?;
        if u64::from(self.stride) < minimum_stride || byte_count > MAX_FRAME_BYTES {
            return Err(GamescopeProfileBindingError::InvalidObservedContract);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum PixelFormat {
    #[serde(rename = "BGRx")]
    Bgrx,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct NormalizerArtifact {
    schema: String,
    capture_profile_sha256: String,
    canonical_frame_contract_id: String,
    implementation: String,
    source: FractionalRectangleArtifact,
}

impl NormalizerArtifact {
    fn validate(&self) -> Result<(), GamescopeProfileBindingError> {
        if self.schema != NORMALIZER_SCHEMA
            || self.canonical_frame_contract_id != CANONICAL_FRAME_CONTRACT_ID
            || self.implementation != NORMALIZER_IMPLEMENTATION
            || !valid_sha256(&self.capture_profile_sha256)
        {
            return Err(GamescopeProfileBindingError::InvalidNormalizer);
        }
        Ok(())
    }

    fn geometry(
        &self,
        observed: &ObservedContract,
    ) -> Result<FractionalLinearGeometry, GamescopeProfileBindingError> {
        let rectangle = self.source.rectangle()?;
        FractionalLinearGeometry::new(observed.video.width, observed.video.height, rectangle)
            .map_err(|_| GamescopeProfileBindingError::InvalidNormalizer)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FractionalRectangleArtifact {
    left: RationalArtifact,
    top: RationalArtifact,
    width: RationalArtifact,
    height: RationalArtifact,
}

impl FractionalRectangleArtifact {
    fn from_rectangle(rectangle: FractionalRectangle) -> Self {
        Self {
            left: RationalArtifact::from_coordinate(rectangle.left()),
            top: RationalArtifact::from_coordinate(rectangle.top()),
            width: RationalArtifact::from_coordinate(rectangle.width()),
            height: RationalArtifact::from_coordinate(rectangle.height()),
        }
    }

    fn rectangle(&self) -> Result<FractionalRectangle, GamescopeProfileBindingError> {
        Ok(FractionalRectangle::new(
            self.left.coordinate()?,
            self.top.coordinate()?,
            self.width.coordinate()?,
            self.height.coordinate()?,
        ))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RationalArtifact {
    numerator: i64,
    denominator: u32,
}

impl RationalArtifact {
    const fn from_coordinate(coordinate: RationalCoordinate) -> Self {
        Self {
            numerator: coordinate.numerator(),
            denominator: coordinate.denominator(),
        }
    }

    fn coordinate(self) -> Result<RationalCoordinate, GamescopeProfileBindingError> {
        RationalCoordinate::new(self.numerator, self.denominator)
            .map_err(|_| GamescopeProfileBindingError::InvalidNormalizer)
    }
}

fn parse_scaler(value: &str) -> Result<GamescopeScaler, GamescopeProfileBindingError> {
    match value {
        "auto" => Ok(GamescopeScaler::Auto),
        "integer" => Ok(GamescopeScaler::Integer),
        "fit" => Ok(GamescopeScaler::Fit),
        "fill" => Ok(GamescopeScaler::Fill),
        "stretch" => Ok(GamescopeScaler::Stretch),
        _ => Err(GamescopeProfileBindingError::InvalidProfile),
    }
}

fn parse_filter(value: &str) -> Result<GamescopeFilter, GamescopeProfileBindingError> {
    match value {
        "linear" => Ok(GamescopeFilter::Linear),
        "nearest" => Ok(GamescopeFilter::Nearest),
        "fsr" => Ok(GamescopeFilter::Fsr),
        "nis" => Ok(GamescopeFilter::Nis),
        "pixel" => Ok(GamescopeFilter::Pixel),
        _ => Err(GamescopeProfileBindingError::InvalidProfile),
    }
}

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>, serde_json::Error> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sha256(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOKEN_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_contract() -> UncalibratedVideoContract {
        UncalibratedVideoContract {
            width: 2_556,
            height: 1_428,
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

    fn artifact() -> BindingArtifact {
        let capture_profile = CaptureProfileArtifact {
            schema: PROFILE_SCHEMA.to_owned(),
            provider: GamescopeProviderProvenance {
                source: ProviderSource::GamescopeDefaultRemote,
                environment_id: "development-machine-v1".to_owned(),
                gamescope_version: "3.16.19-128-g7282613+".to_owned(),
                backend_id: "sdl".to_owned(),
                scaling_configuration: ScalingConfiguration {
                    output_width: 2_556,
                    output_height: 1_428,
                    nested_width: 1_920,
                    nested_height: 1_080,
                    nested_refresh_hz: 120,
                    scaler: GamescopeScaler::Auto,
                    filter: GamescopeFilter::Linear,
                },
            },
            observed: ObservedContract {
                pixel_format: PixelFormat::Bgrx,
                video: video_contract(),
                memory_type: UncalibratedMemoryType::MemoryFileDescriptor,
                stride: 10_224,
            },
            calibration_evidence_sha256: "1".repeat(64),
        };
        let capture_profile_sha256 = sha256(&canonical_json(&capture_profile).unwrap());
        BindingArtifact {
            schema: BINDING_SCHEMA.to_owned(),
            capture_profile,
            normalizer: NormalizerArtifact {
                schema: NORMALIZER_SCHEMA.to_owned(),
                capture_profile_sha256,
                canonical_frame_contract_id: CANONICAL_FRAME_CONTRACT_ID.to_owned(),
                implementation: NORMALIZER_IMPLEMENTATION.to_owned(),
                source: FractionalRectangleArtifact {
                    left: RationalArtifact {
                        numerator: 26,
                        denominator: 3,
                    },
                    top: RationalArtifact {
                        numerator: 0,
                        denominator: 1,
                    },
                    width: RationalArtifact {
                        numerator: 7_616,
                        denominator: 3,
                    },
                    height: RationalArtifact {
                        numerator: 1_428,
                        denominator: 1,
                    },
                },
            },
            gamescope_arguments: None,
        }
    }

    fn encoded_artifact(artifact: &BindingArtifact) -> (Vec<u8>, String) {
        let bytes = canonical_json(artifact).unwrap();
        let digest = sha256(&bytes);
        (bytes, digest)
    }

    fn session_provenance() -> GamescopeSessionProvenanceInput {
        GamescopeSessionProvenanceInput {
            environment_id: "development-machine-v1".to_owned(),
            gamescope_version: "3.16.19-128-g7282613+".to_owned(),
            backend_id: "sdl".to_owned(),
            output_width: 2_556,
            output_height: 1_428,
            nested_width: 1_920,
            nested_height: 1_080,
            nested_refresh_hz: 120,
            scaler: "auto".to_owned(),
            filter: "linear".to_owned(),
        }
    }

    #[test]
    fn canonical_artifact_binds_profile_normalizer_and_observed_contract() {
        let artifact = artifact();
        let expected_profile = artifact.normalizer.capture_profile_sha256.clone();
        let (bytes, digest) = encoded_artifact(&artifact);
        let binding = GamescopeProfileBinding::parse(&bytes, &digest).unwrap();

        assert_eq!(binding.capture_profile_sha256(), expected_profile);
        assert_eq!(binding.normalizer_artifact_sha256(), digest);
        assert_eq!(binding.environment_id(), "development-machine-v1");
        assert_eq!(binding.gamescope_version(), "3.16.19-128-g7282613+");
        assert_eq!(binding.backend_id(), "sdl");
        assert_eq!(binding.nested_width(), 1_920);
        assert_eq!(binding.nested_height(), 1_080);
        assert_eq!(binding.nested_refresh_hz(), 120);
        assert_eq!(
            binding.verify_observed_contract(
                video_contract(),
                UncalibratedMemoryType::MemoryFileDescriptor,
                10_224,
            ),
            Ok(())
        );
    }

    #[test]
    fn local_binding_retains_exact_bounded_gamescope_arguments() {
        let authored = GamescopeProfileBinding::author_local(
            GamescopeProfileBindingAuthoringInput {
                calibration_evidence_sha256: "1".repeat(64),
                environment_id: "local".to_owned(),
                gamescope_version: "3.16.19".to_owned(),
                backend_id: "wayland".to_owned(),
                output_width: 2_556,
                output_height: 1_428,
                nested_width: 1_920,
                nested_height: 1_080,
                nested_refresh_hz: 120,
                scaler: "auto".to_owned(),
                filter: "linear".to_owned(),
                observed_video_contract: video_contract(),
                memory_type: UncalibratedMemoryType::MemoryFileDescriptor,
                stride: 10_224,
                geometry: FractionalRectangle::new(
                    RationalCoordinate::new(26, 3).unwrap(),
                    RationalCoordinate::new(0, 1).unwrap(),
                    RationalCoordinate::new(7_616, 3).unwrap(),
                    RationalCoordinate::new(1_428, 1).unwrap(),
                ),
            },
            vec![
                "--backend".to_owned(),
                "wayland".to_owned(),
                "--hdr-enabled".to_owned(),
            ],
        )
        .unwrap();
        let binding =
            GamescopeProfileBinding::parse(&authored.bytes, &authored.artifact_sha256).unwrap();
        assert_eq!(
            binding.gamescope_arguments().unwrap(),
            ["--backend", "wayland", "--hdr-enabled"]
        );
    }

    #[test]
    fn digest_and_canonical_encoding_are_required() {
        let (mut bytes, digest) = encoded_artifact(&artifact());
        assert_eq!(
            GamescopeProfileBinding::parse(&bytes, &"f".repeat(64)).unwrap_err(),
            GamescopeProfileBindingError::DigestMismatch
        );
        bytes.pop();
        let noncanonical_digest = sha256(&bytes);
        assert_eq!(
            GamescopeProfileBinding::parse(&bytes, &noncanonical_digest).unwrap_err(),
            GamescopeProfileBindingError::NonCanonicalDocument
        );
        assert!(GamescopeProfileBinding::parse(&[], &digest).is_err());
    }

    #[test]
    fn profile_substitution_and_invalid_geometry_fail_closed() {
        let mut substituted = artifact();
        substituted.capture_profile.provider.environment_id = "other-machine".to_owned();
        let (bytes, digest) = encoded_artifact(&substituted);
        assert_eq!(
            GamescopeProfileBinding::parse(&bytes, &digest).unwrap_err(),
            GamescopeProfileBindingError::InvalidNormalizer
        );

        let mut invalid_geometry = artifact();
        invalid_geometry.normalizer.source.width.numerator = 7_700;
        let (bytes, digest) = encoded_artifact(&invalid_geometry);
        assert_eq!(
            GamescopeProfileBinding::parse(&bytes, &digest).unwrap_err(),
            GamescopeProfileBindingError::InvalidNormalizer
        );
    }

    #[test]
    fn output_dimensions_must_match_the_observed_video_contract() {
        let mut mismatched = artifact();
        mismatched
            .capture_profile
            .provider
            .scaling_configuration
            .output_width += 1;
        mismatched.normalizer.capture_profile_sha256 =
            sha256(&canonical_json(&mismatched.capture_profile).unwrap());
        let (bytes, digest) = encoded_artifact(&mismatched);

        assert_eq!(
            GamescopeProfileBinding::parse(&bytes, &digest).unwrap_err(),
            GamescopeProfileBindingError::InvalidProfile
        );
    }

    #[test]
    fn runtime_observed_contract_uses_dimensions_not_incidental_metadata() {
        let (bytes, digest) = encoded_artifact(&artifact());
        let binding = GamescopeProfileBinding::parse(&bytes, &digest).unwrap();
        let mut changed_video = video_contract();
        changed_video.color_primaries = 1;
        assert_eq!(
            binding.verify_observed_contract(
                changed_video,
                UncalibratedMemoryType::MemoryFileDescriptor,
                10_224,
            ),
            Ok(())
        );
        assert_eq!(
            binding.verify_observed_contract(
                video_contract(),
                UncalibratedMemoryType::DmaBuf,
                10_224,
            ),
            Ok(())
        );
        assert_eq!(
            binding.verify_observed_contract(
                video_contract(),
                UncalibratedMemoryType::MemoryFileDescriptor,
                10_220,
            ),
            Ok(())
        );
        let mut changed_dimensions = video_contract();
        changed_dimensions.width += 1;
        assert_eq!(
            binding.verify_observed_contract(
                changed_dimensions,
                UncalibratedMemoryType::MemoryPointer,
                20_000,
            ),
            Err(ObservedContractMismatch::Video)
        );
    }

    #[test]
    fn measured_profile_is_minimal_and_round_trips_geometry() {
        let rectangle = FractionalRectangle::new(
            RationalCoordinate::new(-1_024, 2_048).unwrap(),
            RationalCoordinate::new(-1_024, 2_048).unwrap(),
            RationalCoordinate::new(7_864_320, 2_048).unwrap(),
            RationalCoordinate::new(4_423_680, 2_048).unwrap(),
        );
        let authored = GamescopeProfileBinding::author_measured(
            MeasuredGamescopeProfileBindingAuthoringInput {
                observed_width: 3_840,
                observed_height: 2_160,
                geometry: rectangle,
            },
        )
        .unwrap();
        let binding =
            GamescopeProfileBinding::parse(&authored.bytes, &authored.artifact_sha256).unwrap();
        let encoded = String::from_utf8(authored.bytes).unwrap();
        assert!(binding.is_measured());
        assert_eq!(binding.source_rectangle(), rectangle);
        for forbidden in [
            "gamescope_version",
            "backend_id",
            "nested_refresh",
            "scaler",
            "filter",
            "gamescope_arguments",
            "stride",
            "memory_type",
        ] {
            assert!(!encoded.contains(forbidden), "unexpected field {forbidden}");
        }
    }

    #[test]
    fn measured_profile_retains_the_preexisting_unsigned_numerator_range() {
        let rectangle = FractionalRectangle::new(
            RationalCoordinate::new(3_000_000_000, 3_000_000_000).unwrap(),
            RationalCoordinate::new(0, 1).unwrap(),
            RationalCoordinate::new(3_000_000_000, 1_000_000).unwrap(),
            RationalCoordinate::new(2_160, 1).unwrap(),
        );
        let authored = GamescopeProfileBinding::author_measured(
            MeasuredGamescopeProfileBindingAuthoringInput {
                observed_width: 3_840,
                observed_height: 2_160,
                geometry: rectangle,
            },
        )
        .unwrap();
        let binding =
            GamescopeProfileBinding::parse(&authored.bytes, &authored.artifact_sha256).unwrap();
        assert_eq!(binding.source_rectangle(), rectangle);
    }

    #[test]
    fn every_session_provenance_field_is_exact() {
        let (bytes, digest) = encoded_artifact(&artifact());
        let binding = GamescopeProfileBinding::parse(&bytes, &digest).unwrap();
        let matching = GamescopeSessionProvenance::new(session_provenance()).unwrap();
        assert_eq!(binding.verify_session_provenance(&matching), Ok(()));

        let cases = [
            (
                GamescopeSessionProvenanceMismatch::Environment,
                GamescopeSessionProvenanceInput {
                    environment_id: "other-machine".to_owned(),
                    ..session_provenance()
                },
            ),
            (
                GamescopeSessionProvenanceMismatch::GamescopeVersion,
                GamescopeSessionProvenanceInput {
                    gamescope_version: "3.16.20".to_owned(),
                    ..session_provenance()
                },
            ),
            (
                GamescopeSessionProvenanceMismatch::Backend,
                GamescopeSessionProvenanceInput {
                    backend_id: "wayland".to_owned(),
                    ..session_provenance()
                },
            ),
            (
                GamescopeSessionProvenanceMismatch::OutputDimensions,
                GamescopeSessionProvenanceInput {
                    output_width: 2_555,
                    ..session_provenance()
                },
            ),
            (
                GamescopeSessionProvenanceMismatch::NestedDimensions,
                GamescopeSessionProvenanceInput {
                    nested_height: 1_079,
                    ..session_provenance()
                },
            ),
            (
                GamescopeSessionProvenanceMismatch::NestedRefresh,
                GamescopeSessionProvenanceInput {
                    nested_refresh_hz: 119,
                    ..session_provenance()
                },
            ),
            (
                GamescopeSessionProvenanceMismatch::Scaler,
                GamescopeSessionProvenanceInput {
                    scaler: "fit".to_owned(),
                    ..session_provenance()
                },
            ),
            (
                GamescopeSessionProvenanceMismatch::Filter,
                GamescopeSessionProvenanceInput {
                    filter: "nearest".to_owned(),
                    ..session_provenance()
                },
            ),
        ];
        for (expected, input) in cases {
            let actual = GamescopeSessionProvenance::new(input).unwrap();
            assert_eq!(binding.verify_session_provenance(&actual), Err(expected));
        }
    }
}
