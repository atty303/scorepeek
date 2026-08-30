use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read as _;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::title_decoder::{
    CatalogTitleDecision, CatalogTitleDecoderError, DiagnosticTitleThresholds,
    TITLE_DICTIONARY_SHA256, load_dictionary_contract, score_catalog_titles,
};
use super::title_preprocessor::{
    DYNAMIC_TITLE_INPUT_HEIGHT, DYNAMIC_TITLE_PREPROCESSOR_ID, TITLE_INPUT_SHAPE,
    TITLE_INPUT_VALUES, TITLE_PREPROCESSOR_ID, preprocess_dynamic_title_image,
    preprocess_title_crop, preprocess_title_image,
};
use super::{RecognitionError, Rgb8Crop, read_title_crop_artifact};
use crate::catalog::{Catalog, CatalogStore, CatalogStoreError};

const MODEL_MANIFEST_BYTES: &[u8] =
    include_bytes!("../../../../models/manifests/pp-ocrv6-small-rec-onnx-v1.json");
const MODEL_MANIFEST_SHA256: &str =
    "48cc68b16e785c4b2a0fa2a7764bb1ac6e87e9199065f5bea090a94fca97ee6c";
const MODEL_BYTES: u64 = 21_159_378;
const OUTPUT_SHAPE: [usize; 3] = [1, 40, 18_710];
const OUTPUT_CLASSES: u32 = 18_710;
const INPUT_BYTES: u64 = TITLE_INPUT_VALUES as u64 * 4;
const OUTPUT_BYTES: u64 = 40 * 18_710 * 4;
const MAX_REFERENCE_MANIFEST_BYTES: u64 = 256 * 1024;
const MAX_TENSOR_ABSOLUTE_ERROR: f32 = 2e-5;
const MAX_INPUT_ABSOLUTE_ERROR: f32 = 1e-6;
const MAX_CANDIDATE_LOG_PROBABILITY_ERROR: f64 = 1e-3;
const IDENTITY_PRESENTATION_TRANSFORM_ID: &str = "scorepeek-title-rgb-identity-v1";
const CHANNEL_MAX_PRESENTATION_TRANSFORM_ID: &str = "scorepeek-title-channel-max-rgb-v1";
const MAX_BATCH_REQUEST_BYTES: u64 = 32 * 1024 * 1024;
const MAX_BATCH_CROP_BYTES: u64 = 8 * 1024 * 1024;
const MAX_BATCH_ROWS: usize = 4_096;
const CENSUS_BATCH_SIZE: usize = 8;
const SMALL_BUNDLE_MANIFEST_BYTES: &[u8] =
    include_bytes!("../../../../models/manifests/pp-ocrv6-small-rec-onnx-bundle-v1.json");
pub const LIVE_MODEL_BUNDLE_MANIFEST_SHA256: &str =
    "4064dfa4124ada63613fe39fe2dee92f6ce6cae898e2830b302f5ae593f60672";
const TINY_BUNDLE_MANIFEST_BYTES: &[u8] =
    include_bytes!("../../../../models/manifests/pp-ocrv6-tiny-rec-onnx-bundle-v1.json");
const TINY_BUNDLE_MANIFEST_SHA256: &str =
    "d24f1ec10098065efd24216b23b405bb2af5feabbb815bc499ba0a5735b8bfd0";
const MEDIUM_BUNDLE_MANIFEST_BYTES: &[u8] =
    include_bytes!("../../../../models/manifests/pp-ocrv6-medium-rec-onnx-bundle-v1.json");
const MEDIUM_BUNDLE_MANIFEST_SHA256: &str =
    "f794d77fb6d9860e2aadedd1ef575bd67c044b83fe2821243867b66c9a7c5abe";
const V5_MOBILE_BUNDLE_MANIFEST_BYTES: &[u8] =
    include_bytes!("../../../../models/manifests/pp-ocrv5-mobile-rec-onnx-bundle-v1.json");
const V5_MOBILE_BUNDLE_MANIFEST_SHA256: &str =
    "ebbd34d2c0e360b1cf55199fc1400886e7bfbb4d6917c7d86a994b79c2256971";
const V5_SERVER_BUNDLE_MANIFEST_BYTES: &[u8] =
    include_bytes!("../../../../models/manifests/pp-ocrv5-server-rec-onnx-bundle-v1.json");
const V5_SERVER_BUNDLE_MANIFEST_SHA256: &str =
    "4fe22f41508ed31b86e86caa88d433a20702d0a6e95cea07bcaca577441594fe";
const LIVE_RUNTIME_MANIFEST_BYTES: &[u8] =
    include_bytes!("../../../../models/manifests/pp-ocrv6-small-live-runtime-v1.json");
pub const LIVE_RUNTIME_SHA256: &str =
    "4864f57937b6d57510e82234325f611df31521ff508767011de137bebdf531dc";
pub const LIVE_MODEL_ID: &str = "pp-ocrv6-small-rec-onnx-v1";
pub const LIVE_MODEL_SHA256: &str =
    "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634";

#[derive(Debug)]
pub enum OnnxParityError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Ort(ort::Error),
    Recognition(RecognitionError),
    CatalogDecoder(CatalogTitleDecoderError),
    InvalidArtifact,
    NonFiniteProbability,
    NegativeProbability,
    ProbabilityRowSum { sum: f64 },
    TensorMismatch,
    TokenOrderMismatch,
    CandidateRankingMismatch,
}

impl std::fmt::Display for OnnxParityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "ONNX parity I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "ONNX parity JSON failed: {error}"),
            Self::Ort(error) => write!(formatter, "ONNX Runtime failed: {error}"),
            Self::Recognition(error) => write!(formatter, "title crop validation failed: {error}"),
            Self::CatalogDecoder(error) => {
                write!(formatter, "catalog title scoring failed: {error}")
            }
            Self::InvalidArtifact => formatter.write_str("ONNX parity artifact is invalid"),
            Self::NonFiniteProbability => {
                formatter.write_str("ONNX output contains a non-finite probability")
            }
            Self::NegativeProbability => {
                formatter.write_str("ONNX output contains a negative probability")
            }
            Self::ProbabilityRowSum { sum } => {
                write!(
                    formatter,
                    "ONNX output probability row does not sum to one: {sum:.9}"
                )
            }
            Self::TensorMismatch => formatter.write_str("Paddle and ONNX tensors differ"),
            Self::TokenOrderMismatch => formatter.write_str("Paddle and ONNX token order differs"),
            Self::CandidateRankingMismatch => {
                formatter.write_str("Paddle and ONNX candidate ranking differs")
            }
        }
    }
}

impl std::error::Error for OnnxParityError {}

impl From<std::io::Error> for OnnxParityError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for OnnxParityError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<ort::Error> for OnnxParityError {
    fn from(error: ort::Error) -> Self {
        Self::Ort(error)
    }
}

impl From<RecognitionError> for OnnxParityError {
    fn from(error: RecognitionError) -> Self {
        Self::Recognition(error)
    }
}

impl From<CatalogTitleDecoderError> for OnnxParityError {
    fn from(error: CatalogTitleDecoderError) -> Self {
        Self::CatalogDecoder(error)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnnxModelManifest {
    schema: String,
    model_id: String,
    model_name: String,
    source_repository: String,
    source_revision: String,
    source_url: String,
    sha256: String,
    bytes: u64,
    license_id: String,
    license_url: String,
    paddle_model_id: String,
    paddle_inference_json_sha256: String,
    paddle_inference_yml_sha256: String,
}

impl OnnxModelManifest {
    fn load_registered() -> Result<Self, OnnxParityError> {
        if encode_sha256(MODEL_MANIFEST_BYTES) != MODEL_MANIFEST_SHA256 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let manifest: Self = serde_json::from_slice(MODEL_MANIFEST_BYTES)?;
        let revision = "3d2d345e6a299891174f1397a72cdd81331359c7";
        if manifest.schema != "scorepeek-ocr-onnx-model-source-v1"
            || manifest.model_id != "pp-ocrv6-small-rec-onnx-v1"
            || manifest.model_name != "PP-OCRv6_small_rec"
            || manifest.source_repository != "PaddlePaddle/PP-OCRv6_small_rec_onnx"
            || manifest.source_revision != revision
            || manifest.source_url
                != format!(
                    "https://huggingface.co/PaddlePaddle/PP-OCRv6_small_rec_onnx/resolve/{revision}/inference.onnx"
                )
            || manifest.sha256 != "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634"
            || manifest.bytes != MODEL_BYTES
            || manifest.license_id != "Apache-2.0"
            || !manifest
                .license_url
                .starts_with("https://huggingface.co/PaddlePaddle/PP-OCRv6_small_rec_onnx/blob/")
            || manifest.paddle_model_id != "pp-ocrv6-small-rec-v1"
            || manifest.paddle_inference_json_sha256
                != "f0bf53c853937a917affdd74467472167727f8ab0f0f7bded01c4a16c27e46e6"
            || manifest.paddle_inference_yml_sha256
                != "ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1"
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(manifest)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DynamicBundleManifest {
    schema: String,
    model_id: String,
    model_name: String,
    source_repository: String,
    source_revision: String,
    license_id: String,
    license_url: String,
    native_contract: DynamicNativeContract,
    files: Vec<DynamicBundleFile>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DynamicNativeContract {
    input_layout: String,
    input_color_order: String,
    input_channels: usize,
    input_height: usize,
    preprocessor_minimum_width: usize,
    preprocessor_maximum_width: usize,
    output_classes: usize,
    ctc_blank_token: usize,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DynamicBundleFile {
    filename: String,
    source_url: String,
    sha256: String,
    bytes: u64,
}

/// One immutable file registered for the live PP-OCRv6-small bundle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredLiveModelFile {
    pub filename: String,
    pub source_url: String,
    pub sha256: String,
    pub bytes: u64,
}

/// Returns the verified download contract embedded for the live PP-OCRv6-small bundle.
///
/// # Errors
/// Returns an error if the embedded manifest no longer matches the compiled registration.
pub fn registered_live_model_files() -> Result<Vec<RegisteredLiveModelFile>, OnnxParityError> {
    let manifest = DynamicBundleManifest::load_registered(LIVE_MODEL_ID)?;
    Ok(manifest
        .files
        .into_iter()
        .map(|file| RegisteredLiveModelFile {
            filename: file.filename,
            source_url: file.source_url,
            sha256: file.sha256,
            bytes: file.bytes,
        })
        .collect())
}

/// Verifies the complete registered live bundle without constructing an ONNX session.
///
/// # Errors
/// Returns an error for missing, changed, non-regular, or malformed bundle files.
pub fn verify_registered_live_model_bundle(bundle: &Path) -> Result<(), OnnxParityError> {
    DynamicBundleManifest::load_registered(LIVE_MODEL_ID)?
        .verified_model_bytes(bundle)
        .map(|_| ())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveRuntimeManifest {
    schema: String,
    implementation_id: String,
    ort_crate_version: String,
    ort_api: u32,
    execution_provider: String,
    cpu_arena: bool,
    intra_threads: usize,
    inter_threads: usize,
    parallel_execution: bool,
    graph_optimization: String,
    preprocessor_id: String,
    decoder_id: String,
    model_bundle_manifest_sha256: String,
}

impl LiveRuntimeManifest {
    fn load_registered() -> Result<Self, OnnxParityError> {
        if encode_sha256(LIVE_RUNTIME_MANIFEST_BYTES) != LIVE_RUNTIME_SHA256 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let manifest: Self = serde_json::from_slice(LIVE_RUNTIME_MANIFEST_BYTES)?;
        if manifest.schema != "scorepeek-field-text-runtime-v1"
            || manifest.implementation_id != "scorepeek-pp-ocrv6-small-native-dynamic-cpu-v1"
            || manifest.ort_crate_version != "2.0.0-rc.13"
            || manifest.ort_api != 27
            || manifest.execution_provider != "CPUExecutionProvider"
            || manifest.cpu_arena
            || manifest.intra_threads != 1
            || manifest.inter_threads != 1
            || manifest.parallel_execution
            || manifest.graph_optimization != "all"
            || manifest.preprocessor_id != DYNAMIC_TITLE_PREPROCESSOR_ID
            || manifest.decoder_id != "scorepeek-ctc-greedy-collapse-v1"
            || manifest.model_bundle_manifest_sha256 != LIVE_MODEL_BUNDLE_MANIFEST_SHA256
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(manifest)
    }
}

/// One open-text observation produced without granting field or song authority.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DynamicTextObservation {
    pub input_width: usize,
    pub output_timesteps: usize,
    pub open_text: String,
}

/// The exact registered live text runtime, loaded once and owned by one observer worker.
pub struct RegisteredDynamicTitleRuntime {
    session: Session,
    dictionary: Vec<String>,
    output_classes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisteredResourceLoadErrorType {
    InvalidLocation,
    ModelBindingMismatch,
    RuntimeBindingMismatch,
    CatalogUnavailable,
    CatalogBindingMismatch,
    CatalogLoadFailed,
    ModelBundleInvalid,
    RuntimeInitializationFailed,
}

#[derive(Debug)]
pub enum RegisteredResourceLoadError {
    InvalidLocation {
        role: &'static str,
        source: Option<std::io::Error>,
    },
    ModelBindingMismatch,
    RuntimeBindingMismatch,
    CatalogUnavailable,
    CatalogBindingMismatch,
    Catalog(CatalogStoreError),
    Runtime(OnnxParityError),
}

impl RegisteredResourceLoadError {
    #[must_use]
    pub const fn error_type(&self) -> RegisteredResourceLoadErrorType {
        match self {
            Self::InvalidLocation { .. } => RegisteredResourceLoadErrorType::InvalidLocation,
            Self::ModelBindingMismatch => RegisteredResourceLoadErrorType::ModelBindingMismatch,
            Self::RuntimeBindingMismatch => RegisteredResourceLoadErrorType::RuntimeBindingMismatch,
            Self::CatalogUnavailable => RegisteredResourceLoadErrorType::CatalogUnavailable,
            Self::CatalogBindingMismatch => RegisteredResourceLoadErrorType::CatalogBindingMismatch,
            Self::Catalog(_) => RegisteredResourceLoadErrorType::CatalogLoadFailed,
            Self::Runtime(OnnxParityError::Ort(_)) => {
                RegisteredResourceLoadErrorType::RuntimeInitializationFailed
            }
            Self::Runtime(_) => RegisteredResourceLoadErrorType::ModelBundleInvalid,
        }
    }
}

impl std::fmt::Display for RegisteredResourceLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLocation { role, source } => {
                if let Some(source) = source {
                    write!(formatter, "registered {role} metadata failed: {source}")
                } else {
                    write!(formatter, "registered {role} must be an absolute directory")
                }
            }
            Self::ModelBindingMismatch => {
                formatter.write_str("recognition binding does not select the registered model")
            }
            Self::RuntimeBindingMismatch => {
                formatter.write_str("recognition binding does not select the registered runtime")
            }
            Self::CatalogUnavailable => formatter.write_str("active catalog is unavailable"),
            Self::CatalogBindingMismatch => {
                formatter.write_str("active catalog does not match the recognition binding")
            }
            Self::Catalog(error) => write!(formatter, "active catalog load failed: {error}"),
            Self::Runtime(error) => {
                write!(formatter, "registered text runtime load failed: {error}")
            }
        }
    }
}

impl std::error::Error for RegisteredResourceLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidLocation {
                source: Some(error),
                ..
            } => Some(error),
            Self::Catalog(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::InvalidLocation { source: None, .. }
            | Self::ModelBindingMismatch
            | Self::RuntimeBindingMismatch
            | Self::CatalogUnavailable
            | Self::CatalogBindingMismatch => None,
        }
    }
}

/// Exact catalog and text-runtime inputs retained for one immutable recognition run.
pub struct RegisteredRecognitionResources {
    catalog_digest: String,
    catalog: Catalog,
    title_runtime: RegisteredDynamicTitleRuntime,
}

impl RegisteredRecognitionResources {
    /// Loads and digest-checks the active catalog and registered runtime exactly once.
    ///
    /// # Errors
    /// Returns a stable typed failure for location, binding, catalog, bundle, or runtime errors.
    /// No download, fallback, or active-state mutation is attempted.
    pub fn load(
        catalog_root: &Path,
        bundle_root: &Path,
        expected_catalog_sha256: &str,
        expected_model_sha256: &str,
        expected_runtime_sha256: &str,
    ) -> Result<Self, RegisteredResourceLoadError> {
        if expected_model_sha256 != LIVE_MODEL_SHA256 {
            return Err(RegisteredResourceLoadError::ModelBindingMismatch);
        }
        if expected_runtime_sha256 != LIVE_RUNTIME_SHA256 {
            return Err(RegisteredResourceLoadError::RuntimeBindingMismatch);
        }
        validate_registered_resource_directory(catalog_root, "catalog store")?;
        validate_registered_resource_directory(bundle_root, "model bundle")?;
        let active = CatalogStore::new(catalog_root)
            .load_active()
            .map_err(RegisteredResourceLoadError::Catalog)?
            .ok_or(RegisteredResourceLoadError::CatalogUnavailable)?;
        if active.digest != expected_catalog_sha256 {
            return Err(RegisteredResourceLoadError::CatalogBindingMismatch);
        }
        let title_runtime = RegisteredDynamicTitleRuntime::load(bundle_root)
            .map_err(RegisteredResourceLoadError::Runtime)?;
        Ok(Self {
            catalog_digest: active.digest,
            catalog: active.catalog,
            title_runtime,
        })
    }

    #[must_use]
    pub fn catalog_sha256(&self) -> &str {
        &self.catalog_digest
    }

    #[must_use]
    pub const fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    #[must_use]
    pub const fn title_runtime(&mut self) -> &mut RegisteredDynamicTitleRuntime {
        &mut self.title_runtime
    }
}

fn validate_registered_resource_directory(
    path: &Path,
    role: &'static str,
) -> Result<(), RegisteredResourceLoadError> {
    if !path.is_absolute() {
        return Err(RegisteredResourceLoadError::InvalidLocation { role, source: None });
    }
    let metadata =
        path.metadata()
            .map_err(|source| RegisteredResourceLoadError::InvalidLocation {
                role,
                source: Some(source),
            })?;
    if !metadata.is_dir() {
        return Err(RegisteredResourceLoadError::InvalidLocation { role, source: None });
    }
    Ok(())
}

impl RegisteredDynamicTitleRuntime {
    /// Verifies the complete registered PP-OCRv6-small bundle and constructs its fixed CPU session.
    ///
    /// # Errors
    /// Returns an error for missing, changed, or malformed bundle bytes or runtime initialization
    /// failure. No runtime download or fallback is attempted.
    pub fn load(bundle: &Path) -> Result<Self, OnnxParityError> {
        let runtime = LiveRuntimeManifest::load_registered()?;
        let manifest = DynamicBundleManifest::load_registered(LIVE_MODEL_ID)?;
        let model_bytes = manifest.verified_model_bytes(bundle)?;
        let dictionary_file = manifest
            .files
            .iter()
            .find(|file| file.filename == "inference.yml")
            .ok_or(OnnxParityError::InvalidArtifact)?;
        let dictionary = load_dictionary_contract(
            &bundle.join("inference.yml"),
            &dictionary_file.sha256,
            manifest.native_contract.output_classes,
        )?;
        let session = Session::builder()?
            .with_execution_providers([ort::ep::CPU::default()
                .with_arena_allocator(runtime.cpu_arena)
                .build()])
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .with_intra_threads(runtime.intra_threads)
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .with_inter_threads(runtime.inter_threads)
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .with_parallel_execution(runtime.parallel_execution)
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .commit_from_memory(&model_bytes)?;
        if session.inputs().len() != 1 || session.outputs().len() != 1 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(Self {
            session,
            dictionary,
            output_classes: manifest.native_contract.output_classes,
        })
    }

    /// Runs the already-loaded runtime against one bounded RGB8 crop.
    ///
    /// # Errors
    /// Returns an error for an invalid crop or unexpected runtime tensor contract.
    pub fn observe_open_text(
        &mut self,
        crop: &Rgb8Crop,
    ) -> Result<DynamicTextObservation, OnnxParityError> {
        observe_dynamic_rgb8(
            &mut self.session,
            &self.dictionary,
            self.output_classes,
            crop.pixels(),
            crop.roi.width as usize,
            crop.roi.height as usize,
        )
        .map(|(observation, _)| observation)
    }
}

type RegisteredDynamicBundle = (
    &'static [u8],
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    usize,
    &'static [(&'static str, &'static str, u64)],
);

const SMALL_BUNDLE_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
        21_159_378,
    ),
    (
        "inference.json",
        "f0bf53c853937a917affdd74467472167727f8ab0f0f7bded01c4a16c27e46e6",
        208_004,
    ),
    (
        "inference.yml",
        "ab078671bb49f06228eadccd34f1bb501e157f7a047095ffb943ba81512c77d1",
        150_579,
    ),
];
const TINY_BUNDLE_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "9ef676d6ed3c88256a2d92c640c44f25b0c40947e111b14b8be8f594091563e6",
        4_462_639,
    ),
    (
        "inference.json",
        "b5b14770c7dcf092781e92f4278a2ae5f95048f08b4b8a04140e88cb2745f147",
        108_959,
    ),
    (
        "inference.yml",
        "66170210bad538e83fff3c4a3867e547d6bf20b50d64b20347c4b913f3034ea1",
        55_571,
    ),
];
const MEDIUM_BUNDLE_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "9c09abf0957f7968c7586464b7397b84ad2387a0497a351af40e9acc71b673ba",
        76_554_979,
    ),
    (
        "inference.json",
        "0b2e25e990bd072f1bf77d59d67d508bce6c4bd44af6624e0fb27d6da2cd00e8",
        221_814,
    ),
    (
        "inference.yml",
        "991b700facf5b50a7de193468207d5f4255b538dde0d312ae3b7c7a9b6873129",
        150_580,
    ),
];
const V5_MOBILE_BUNDLE_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "da72dc72ca4dc220df0dfde68c1dedc31c58d3e76a25871122e5056227d50092",
        16_534_782,
    ),
    (
        "inference.yml",
        "5dfeb2777f6d0db8177d8128a8acfcf6e6276dc4ac73ea3bf0dc06d6a5e85d8e",
        148_345,
    ),
];
const V5_SERVER_BUNDLE_FILES: &[(&str, &str, u64)] = &[
    (
        "inference.onnx",
        "d9dc333c9c7b042c6dffb8e33d72b6f65c9c1d463d0a3c2f78174fea55e94752",
        84_503_027,
    ),
    (
        "inference.yml",
        "2c719dba044c4e2228aef8ff92f5f575394d75d24c16de096a33b7cfd902f66d",
        148_345,
    ),
];

fn registered_dynamic_bundle(model_id: &str) -> Result<RegisteredDynamicBundle, OnnxParityError> {
    match model_id {
        "pp-ocrv6-small-rec-onnx-v1" => Ok((
            SMALL_BUNDLE_MANIFEST_BYTES,
            LIVE_MODEL_BUNDLE_MANIFEST_SHA256,
            "PP-OCRv6_small_rec",
            "PaddlePaddle/PP-OCRv6_small_rec_onnx",
            "b8f84f0b80c529de40b4fbb3544b84fa7233a513",
            18_710,
            SMALL_BUNDLE_FILES,
        )),
        "pp-ocrv6-tiny-rec-onnx-v1" => Ok((
            TINY_BUNDLE_MANIFEST_BYTES,
            TINY_BUNDLE_MANIFEST_SHA256,
            "PP-OCRv6_tiny_rec",
            "PaddlePaddle/PP-OCRv6_tiny_rec_onnx",
            "2612ab37152ae0a677521bae4e1e3d4fb4cf7c30",
            6_906,
            TINY_BUNDLE_FILES,
        )),
        "pp-ocrv6-medium-rec-onnx-v1" => Ok((
            MEDIUM_BUNDLE_MANIFEST_BYTES,
            MEDIUM_BUNDLE_MANIFEST_SHA256,
            "PP-OCRv6_medium_rec",
            "PaddlePaddle/PP-OCRv6_medium_rec_onnx",
            "50c7eacafc52fa7bcf4194e8cd08e46f8558504b",
            18_710,
            MEDIUM_BUNDLE_FILES,
        )),
        "pp-ocrv5-mobile-rec-onnx-v1" => Ok((
            V5_MOBILE_BUNDLE_MANIFEST_BYTES,
            V5_MOBILE_BUNDLE_MANIFEST_SHA256,
            "PP-OCRv5_mobile_rec",
            "PaddlePaddle/PP-OCRv5_mobile_rec_onnx",
            "ed152b8b495f84de93cda5709d768548a9127622",
            18_385,
            V5_MOBILE_BUNDLE_FILES,
        )),
        "pp-ocrv5-server-rec-onnx-v1" => Ok((
            V5_SERVER_BUNDLE_MANIFEST_BYTES,
            V5_SERVER_BUNDLE_MANIFEST_SHA256,
            "PP-OCRv5_server_rec",
            "PaddlePaddle/PP-OCRv5_server_rec_onnx",
            "b70df217f4fd99d14f970bad092cebe7d74cc4d1",
            18_385,
            V5_SERVER_BUNDLE_FILES,
        )),
        _ => Err(OnnxParityError::InvalidArtifact),
    }
}

impl DynamicBundleManifest {
    fn load_registered(model_id: &str) -> Result<Self, OnnxParityError> {
        let (bytes, digest, model_name, repository, revision, output_classes, expected_files) =
            registered_dynamic_bundle(model_id)?;
        if encode_sha256(bytes) != digest {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let manifest: Self = serde_json::from_slice(bytes)?;
        if manifest.schema != "scorepeek-ocr-onnx-model-bundle-v1"
            || manifest.model_id != model_id
            || manifest.model_name != model_name
            || manifest.source_repository != repository
            || manifest.source_revision != revision
            || manifest.license_id != "Apache-2.0"
            || manifest.license_url
                != format!("https://huggingface.co/{repository}/blob/{revision}/README.md")
            || manifest.native_contract.input_layout != "NCHW"
            || manifest.native_contract.input_color_order != "BGR"
            || manifest.native_contract.input_channels != 3
            || manifest.native_contract.input_height != DYNAMIC_TITLE_INPUT_HEIGHT
            || manifest.native_contract.preprocessor_minimum_width != 320
            || manifest.native_contract.preprocessor_maximum_width != 3_200
            || manifest.native_contract.output_classes != output_classes
            || manifest.native_contract.ctc_blank_token != 0
            || manifest.files.len() != expected_files.len()
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        for (file, (filename, sha256, bytes)) in
            manifest.files.iter().zip(expected_files.iter().copied())
        {
            if file.filename != filename
                || file.sha256 != sha256
                || file.bytes != bytes
                || file.source_url
                    != format!("https://huggingface.co/{repository}/resolve/{revision}/{filename}")
            {
                return Err(OnnxParityError::InvalidArtifact);
            }
        }
        Ok(manifest)
    }

    fn verified_file(&self, bundle: &Path, filename: &str) -> Result<Vec<u8>, OnnxParityError> {
        let file = self
            .files
            .iter()
            .find(|file| file.filename == filename)
            .ok_or(OnnxParityError::InvalidArtifact)?;
        let bytes = read_exact_regular(&bundle.join(filename), file.bytes)?;
        if encode_sha256(&bytes) != file.sha256 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(bytes)
    }

    fn verified_model_bytes(&self, bundle: &Path) -> Result<Vec<u8>, OnnxParityError> {
        let mut model = None;
        for file in &self.files {
            let bytes = self.verified_file(bundle, &file.filename)?;
            if file.filename == "inference.onnx" {
                model = Some(bytes);
            }
        }
        model.ok_or(OnnxParityError::InvalidArtifact)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParityReference {
    schema: String,
    frame_extraction_sha256: String,
    crop_manifest_sha256: String,
    title_crop_file_sha256: String,
    candidate_source_sha256: String,
    paddle_model_id: String,
    paddle_model_archive_sha256: String,
    onnx_model_id: String,
    onnx_model_sha256: String,
    paddle_inference_json_sha256: String,
    paddle_inference_yml_sha256: String,
    preprocessor_id: String,
    input: TensorArtifact,
    paddle_output: TensorArtifact,
    ctc_blank_token: u32,
    argmax_token_order: Vec<u32>,
    collapsed_token_order: Vec<u32>,
    candidate_ranking: Vec<CandidateScore>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TensorArtifact {
    filename: String,
    sha256: String,
    bytes: u64,
    shape: Vec<usize>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateScore {
    song_id: String,
    title: String,
    tokens: Vec<u32>,
    paddle_log_probability: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct OnnxParitySummary {
    pub schema: &'static str,
    pub reference_manifest_sha256: String,
    pub onnx_model_sha256: String,
    pub catalog_sha256: String,
    pub dictionary_sha256: &'static str,
    pub preprocessor_id: &'static str,
    pub thresholds: DiagnosticTitleThresholds,
    pub maximum_input_absolute_error: f32,
    pub maximum_tensor_absolute_error: f32,
    pub maximum_candidate_log_probability_error: f64,
    pub argmax_token_order_matches: bool,
    pub collapsed_token_order_matches: bool,
    pub candidate_ranking_matches: bool,
    pub top_candidate_song_id: String,
    pub catalog_title_decision: CatalogTitleDecision,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OfficialOnnxDecodeRequest {
    schema: String,
    rows: Vec<OfficialOnnxDecodeRequestRow>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OfficialOnnxDecodeRequestRow {
    path: PathBuf,
    file_sha256: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct OfficialOnnxDecodeSummary {
    pub schema: &'static str,
    pub model_id: String,
    pub model_sha256: String,
    pub dictionary_sha256: &'static str,
    pub preprocessor_id: &'static str,
    pub elapsed_ms: u128,
    pub decoded_text: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DynamicOfficialOnnxDecodeSummary {
    pub schema: &'static str,
    pub request_sha256: String,
    pub model_id: String,
    pub model_sha256: String,
    pub dictionary_sha256: String,
    pub preprocessor_id: &'static str,
    pub elapsed_ms: u128,
    pub input_widths: Vec<usize>,
    pub input_tensor_sha256s: Vec<String>,
    pub output_timesteps: Vec<usize>,
    pub decoded_text: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct OnnxTitleDiagnosticRequest<'a> {
    pub model_path: &'a Path,
    pub reference_directory: &'a Path,
    pub reference_sha256: &'a str,
    pub crop_directory: &'a Path,
    pub catalog_sha256: &'a str,
    pub inference_yml: &'a Path,
}

struct VerifiedTitleInputs {
    model_manifest: OnnxModelManifest,
    model_bytes: Vec<u8>,
    reference: ParityReference,
    rust_input: Vec<f32>,
    paddle_output: Vec<f32>,
    maximum_input_absolute_error: f32,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportContractReference {
    schema: String,
    training_preparation_sha256: String,
    validation_list_sha256: String,
    dictionary_sha256: String,
    validation_row_index: usize,
    crop_file_sha256: String,
    export_manifest_sha256: String,
    presentation_transform_id: String,
    onnx_model_sha256: String,
    inference_config_sha256: String,
    input: TensorArtifact,
    paddle_output: TensorArtifact,
    ctc_blank_token: u32,
    argmax_token_order: Vec<u32>,
    collapsed_token_order: Vec<u32>,
}

#[derive(Clone, Copy, Debug)]
pub struct ExportContractParityRequest<'a> {
    pub model_path: &'a Path,
    pub model_sha256: &'a str,
    pub reference_directory: &'a Path,
    pub reference_sha256: &'a str,
    pub inference_yml: &'a Path,
}

#[derive(Clone, Debug, Serialize)]
pub struct ExportContractParitySummary {
    pub schema: &'static str,
    pub reference_manifest_sha256: String,
    pub onnx_model_sha256: String,
    pub inference_config_sha256: String,
    pub presentation_transform_id: String,
    pub input_shape: Vec<usize>,
    pub output_shape: Vec<usize>,
    pub maximum_tensor_absolute_error: f32,
    pub argmax_token_order_matches: bool,
    pub collapsed_token_order_matches: bool,
}

/// Compares a scorepeek-owned Paddle tensor reference with its ONNX export.
///
/// This boundary verifies exact model, dictionary, tensor shape, probability, and token-order
/// evidence. It does not recognize a title or calibrate an acceptance threshold.
///
/// # Errors
/// Returns an error for invalid digest-bound evidence or any Paddle/ONNX contract mismatch.
pub fn compare_export_contract(
    request: ExportContractParityRequest<'_>,
) -> Result<ExportContractParitySummary, OnnxParityError> {
    if !valid_sha256(request.model_sha256) || !valid_sha256(request.reference_sha256) {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let manifest_bytes = read_bounded_regular(
        &request.reference_directory.join("manifest.json"),
        MAX_REFERENCE_MANIFEST_BYTES,
    )?;
    if encode_sha256(&manifest_bytes) != request.reference_sha256 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let reference: ExportContractReference = serde_json::from_slice(&manifest_bytes)?;
    reference.validate(request.model_sha256)?;
    let model_bytes = read_bounded_regular(request.model_path, 512 * 1024 * 1024)?;
    if encode_sha256(&model_bytes) != request.model_sha256 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let input = decode_f32(&reference.input.read(request.reference_directory)?)?;
    let paddle_output = decode_f32(&reference.paddle_output.read(request.reference_directory)?)?;
    let [batch, channels, height, width] = reference.input.shape.as_slice() else {
        return Err(OnnxParityError::InvalidArtifact);
    };
    let [output_batch, timesteps, classes] = reference.paddle_output.shape.as_slice() else {
        return Err(OnnxParityError::InvalidArtifact);
    };
    if (*batch, *channels, *height) != (1, 3, 48)
        || *output_batch != 1
        || *width
            != timesteps
                .checked_mul(8)
                .ok_or(OnnxParityError::InvalidArtifact)?
        || input.len() != batch * channels * height * width
        || paddle_output.len() != output_batch * timesteps * classes
    {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let dictionary = load_dictionary_contract(
        request.inference_yml,
        &reference.inference_config_sha256,
        *classes,
    )?;
    if dictionary.len() != *classes {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let dictionary_bytes = dictionary[1..dictionary.len() - 1]
        .iter()
        .flat_map(|token| token.bytes().chain(std::iter::once(b'\n')))
        .collect::<Vec<_>>();
    if encode_sha256(&dictionary_bytes) != reference.dictionary_sha256 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    validate_probability_rows(&paddle_output, *classes)?;

    let mut session = Session::builder()?.commit_from_memory(&model_bytes)?;
    if session.inputs().len() != 1 || session.outputs().len() != 1 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let input_shape = [*batch, *channels, *height, *width];
    let outputs = session.run(ort::inputs![Tensor::from_array((input_shape, input))?])?;
    let (output_shape, onnx_output) = outputs[0].try_extract_tensor::<f32>()?;
    let expected_output_shape = [
        i64::try_from(*output_batch).map_err(|_| OnnxParityError::InvalidArtifact)?,
        i64::try_from(*timesteps).map_err(|_| OnnxParityError::InvalidArtifact)?,
        i64::try_from(*classes).map_err(|_| OnnxParityError::InvalidArtifact)?,
    ];
    if output_shape.as_ref() != expected_output_shape {
        return Err(OnnxParityError::InvalidArtifact);
    }
    validate_probability_rows(onnx_output, *classes)?;
    let maximum_tensor_absolute_error = onnx_output
        .iter()
        .zip(&paddle_output)
        .map(|(onnx, paddle)| (onnx - paddle).abs())
        .fold(0.0_f32, f32::max);
    if maximum_tensor_absolute_error > MAX_TENSOR_ABSOLUTE_ERROR {
        return Err(OnnxParityError::TensorMismatch);
    }
    let (argmax, collapsed) = argmax_tokens(onnx_output, *timesteps, *classes)?;
    if argmax != reference.argmax_token_order || collapsed != reference.collapsed_token_order {
        return Err(OnnxParityError::TokenOrderMismatch);
    }
    Ok(ExportContractParitySummary {
        schema: "scorepeek-title-model-export-contract-parity-v1",
        reference_manifest_sha256: request.reference_sha256.to_owned(),
        onnx_model_sha256: request.model_sha256.to_owned(),
        inference_config_sha256: reference.inference_config_sha256,
        presentation_transform_id: reference.presentation_transform_id,
        input_shape: reference.input.shape,
        output_shape: reference.paddle_output.shape,
        maximum_tensor_absolute_error,
        argmax_token_order_matches: true,
        collapsed_token_order_matches: true,
    })
}

fn validate_probability_rows(probabilities: &[f32], classes: usize) -> Result<(), OnnxParityError> {
    if classes == 0 || !probabilities.len().is_multiple_of(classes) {
        return Err(OnnxParityError::InvalidArtifact);
    }
    for row in probabilities.chunks_exact(classes) {
        let sum: f64 = row.iter().map(|value| f64::from(*value)).sum();
        if row.iter().any(|value| !value.is_finite() || *value <= 0.0) || (sum - 1.0).abs() > 2e-5 {
            return Err(OnnxParityError::InvalidArtifact);
        }
    }
    Ok(())
}

fn validate_argmax_probability_rows(
    probabilities: &[f32],
    classes: usize,
) -> Result<(), OnnxParityError> {
    const SUM_TOLERANCE: f64 = 1e-4;

    if classes == 0 || !probabilities.len().is_multiple_of(classes) {
        return Err(OnnxParityError::InvalidArtifact);
    }
    for row in probabilities.chunks_exact(classes) {
        let sum: f64 = row.iter().map(|value| f64::from(*value)).sum();
        if row.iter().any(|value| !value.is_finite()) {
            return Err(OnnxParityError::NonFiniteProbability);
        }
        if row.iter().any(|value| *value < 0.0) {
            return Err(OnnxParityError::NegativeProbability);
        }
        if (sum - 1.0).abs() > SUM_TOLERANCE {
            return Err(OnnxParityError::ProbabilityRowSum { sum });
        }
    }
    Ok(())
}

/// Runs the registered Rust preprocessor and ONNX graph against one verified title crop.
///
/// The Paddle reference remains an independent parity oracle for the complete input and output
/// tensors. Exact registered-dictionary titles from the identified active catalog are then scored
/// through a shared CTC trie. This diagnostic does not create an accepted recognition result.
///
/// # Errors
/// Returns an error for unregistered model bytes, invalid reference evidence, tensor drift,
/// token-order drift, or candidate-ranking drift.
pub fn compare_paddle_onnx(
    request: OnnxTitleDiagnosticRequest<'_>,
    catalog: &Catalog,
    thresholds: DiagnosticTitleThresholds,
) -> Result<OnnxParitySummary, OnnxParityError> {
    let verified = load_verified_title_inputs(&request)?;

    let mut session = Session::builder()?.commit_from_memory(&verified.model_bytes)?;
    if session.inputs().len() != 1 || session.outputs().len() != 1 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let input_tensor = Tensor::from_array((TITLE_INPUT_SHAPE, verified.rust_input))?;
    let outputs = session.run(ort::inputs![input_tensor])?;
    let (output_shape, onnx_output) = outputs[0].try_extract_tensor::<f32>()?;
    if output_shape.as_ref() != [1_i64, 40, 18_710]
        || onnx_output.len() != verified.paddle_output.len()
    {
        return Err(OnnxParityError::InvalidArtifact);
    }
    if onnx_output
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return Err(OnnxParityError::InvalidArtifact);
    }

    let maximum_tensor_absolute_error = onnx_output
        .iter()
        .zip(&verified.paddle_output)
        .map(|(onnx, paddle)| (onnx - paddle).abs())
        .fold(0.0_f32, f32::max);
    if maximum_tensor_absolute_error > MAX_TENSOR_ABSOLUTE_ERROR {
        return Err(OnnxParityError::TensorMismatch);
    }

    let (argmax, collapsed) = argmax_tokens(onnx_output, OUTPUT_SHAPE[1], OUTPUT_SHAPE[2])?;
    if argmax != verified.reference.argmax_token_order {
        return Err(OnnxParityError::TokenOrderMismatch);
    }
    if collapsed != verified.reference.collapsed_token_order {
        return Err(OnnxParityError::TokenOrderMismatch);
    }

    let mut ranked = Vec::with_capacity(verified.reference.candidate_ranking.len());
    let mut maximum_candidate_error = 0.0_f64;
    for candidate in &verified.reference.candidate_ranking {
        let score = ctc_log_probability(
            onnx_output,
            OUTPUT_SHAPE[1],
            OUTPUT_SHAPE[2],
            &candidate.tokens,
        )?;
        maximum_candidate_error =
            maximum_candidate_error.max((score - candidate.paddle_log_probability).abs());
        ranked.push((candidate.song_id.as_str(), score));
    }
    if maximum_candidate_error > MAX_CANDIDATE_LOG_PROBABILITY_ERROR {
        return Err(OnnxParityError::CandidateRankingMismatch);
    }
    ranked.sort_by(|left, right| right.1.total_cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    let expected: Vec<_> = verified
        .reference
        .candidate_ranking
        .iter()
        .map(|candidate| candidate.song_id.as_str())
        .collect();
    let actual: Vec<_> = ranked.iter().map(|(song_id, _)| *song_id).collect();
    if actual != expected {
        return Err(OnnxParityError::CandidateRankingMismatch);
    }
    let catalog_title_decision =
        score_catalog_titles(onnx_output, catalog, request.inference_yml, thresholds)?;

    Ok(OnnxParitySummary {
        schema: "scorepeek-ocr-onnx-title-diagnostic-v1",
        reference_manifest_sha256: request.reference_sha256.to_owned(),
        onnx_model_sha256: verified.model_manifest.sha256,
        catalog_sha256: request.catalog_sha256.to_owned(),
        dictionary_sha256: TITLE_DICTIONARY_SHA256,
        preprocessor_id: TITLE_PREPROCESSOR_ID,
        thresholds,
        maximum_input_absolute_error: verified.maximum_input_absolute_error,
        maximum_tensor_absolute_error,
        maximum_candidate_log_probability_error: maximum_candidate_error,
        argmax_token_order_matches: true,
        collapsed_token_order_matches: true,
        candidate_ranking_matches: true,
        top_candidate_song_id: actual[0].to_owned(),
        catalog_title_decision,
    })
}

/// Runs the registered official ONNX recognizer over a digest-bound batch of strict P6 crops.
///
/// This boundary returns only the model's collapsed open-text observations. Song matching remains
/// a separate offline evaluation concern so a model's dictionary or timestep limit cannot remove
/// catalog songs from the comparison domain.
///
/// # Errors
/// Returns an error for unregistered model or dictionary bytes, malformed crop evidence, or an
/// unexpected ONNX tensor contract.
pub fn decode_official_onnx_crops(
    model_path: &Path,
    inference_yml: &Path,
    request_path: &Path,
) -> Result<OfficialOnnxDecodeSummary, OnnxParityError> {
    let manifest = OnnxModelManifest::load_registered()?;
    let model_bytes = read_exact_regular(model_path, MODEL_BYTES)?;
    if encode_sha256(&model_bytes) != manifest.sha256 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let dictionary =
        load_dictionary_contract(inference_yml, TITLE_DICTIONARY_SHA256, OUTPUT_SHAPE[2])?;
    let request_bytes = read_bounded_regular(request_path, MAX_BATCH_REQUEST_BYTES)?;
    let request: OfficialOnnxDecodeRequest = serde_json::from_slice(&request_bytes)?;
    if request.schema != "scorepeek-private-official-onnx-decode-request-v1"
        || request.rows.is_empty()
        || request.rows.len() > MAX_BATCH_ROWS
    {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let mut crops = Vec::with_capacity(request.rows.len());
    for row in &request.rows {
        if !row.path.is_absolute() || !valid_sha256(&row.file_sha256) {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let bytes = read_bounded_regular(&row.path, MAX_BATCH_CROP_BYTES)?;
        if encode_sha256(&bytes) != row.file_sha256 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let (width, height, pixels) = strict_p6(&bytes)?;
        crops.push(preprocess_title_image(pixels, width, height)?);
    }

    let started = Instant::now();
    let mut session = Session::builder()?.commit_from_memory(&model_bytes)?;
    if session.inputs().len() != 1 || session.outputs().len() != 1 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let mut decoded_text = Vec::with_capacity(crops.len());
    for batch in crops.chunks(CENSUS_BATCH_SIZE) {
        let batch_size = batch.len();
        let input: Vec<_> = batch.iter().flatten().copied().collect();
        let input_shape = [batch_size, 3, TITLE_INPUT_SHAPE[2], TITLE_INPUT_SHAPE[3]];
        let outputs = session.run(ort::inputs![Tensor::from_array((input_shape, input))?])?;
        let (shape, probabilities) = outputs[0].try_extract_tensor::<f32>()?;
        let expected_shape = [
            i64::try_from(batch_size).map_err(|_| OnnxParityError::InvalidArtifact)?,
            i64::try_from(OUTPUT_SHAPE[1]).map_err(|_| OnnxParityError::InvalidArtifact)?,
            i64::try_from(OUTPUT_SHAPE[2]).map_err(|_| OnnxParityError::InvalidArtifact)?,
        ];
        if shape.as_ref() != expected_shape
            || probabilities.len() != batch_size * OUTPUT_SHAPE[1] * OUTPUT_SHAPE[2]
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        validate_argmax_probability_rows(probabilities, OUTPUT_SHAPE[2])?;
        for output in probabilities.chunks_exact(OUTPUT_SHAPE[1] * OUTPUT_SHAPE[2]) {
            let (_, collapsed) = argmax_tokens(output, OUTPUT_SHAPE[1], OUTPUT_SHAPE[2])?;
            let mut text = String::new();
            for token in collapsed {
                text.push_str(
                    dictionary
                        .get(usize::try_from(token).map_err(|_| OnnxParityError::InvalidArtifact)?)
                        .ok_or(OnnxParityError::InvalidArtifact)?,
                );
            }
            decoded_text.push(text);
        }
    }
    Ok(OfficialOnnxDecodeSummary {
        schema: "scorepeek-official-onnx-open-text-batch-v1",
        model_id: manifest.model_id,
        model_sha256: manifest.sha256,
        dictionary_sha256: TITLE_DICTIONARY_SHA256,
        preprocessor_id: TITLE_PREPROCESSOR_ID,
        elapsed_ms: started.elapsed().as_millis(),
        decoded_text,
    })
}

/// Runs a registered dynamic recognizer over digest-bound strict P6 crops without retaining tensors.
///
/// Each crop is preprocessed and executed before the next crop is read. This keeps the dynamic
/// width contract bounded even for the maximum request row count.
///
/// # Errors
/// Returns an error for incomplete registered bundle bytes, malformed crop evidence, or an
/// unexpected dynamic ONNX tensor contract.
pub fn decode_dynamic_official_onnx_crops(
    model_id: &str,
    bundle_path: &Path,
    request_path: &Path,
) -> Result<DynamicOfficialOnnxDecodeSummary, OnnxParityError> {
    let manifest = DynamicBundleManifest::load_registered(model_id)?;
    let output_classes = manifest.native_contract.output_classes;
    let model_bytes = manifest.verified_model_bytes(bundle_path)?;
    let dictionary_file = manifest
        .files
        .iter()
        .find(|file| file.filename == "inference.yml")
        .ok_or(OnnxParityError::InvalidArtifact)?;
    let dictionary = load_dictionary_contract(
        &bundle_path.join("inference.yml"),
        &dictionary_file.sha256,
        output_classes,
    )?;
    let model_file = manifest
        .files
        .iter()
        .find(|file| file.filename == "inference.onnx")
        .ok_or(OnnxParityError::InvalidArtifact)?;
    let request_bytes = read_bounded_regular(request_path, MAX_BATCH_REQUEST_BYTES)?;
    let request_sha256 = encode_sha256(&request_bytes);
    let request: OfficialOnnxDecodeRequest = serde_json::from_slice(&request_bytes)?;
    if request.schema != "scorepeek-private-official-onnx-decode-request-v1"
        || request.rows.is_empty()
        || request.rows.len() > MAX_BATCH_ROWS
    {
        return Err(OnnxParityError::InvalidArtifact);
    }

    let started = Instant::now();
    let mut session = Session::builder()?.commit_from_memory(&model_bytes)?;
    if session.inputs().len() != 1 || session.outputs().len() != 1 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let mut input_widths = Vec::with_capacity(request.rows.len());
    let mut input_tensor_sha256s = Vec::with_capacity(request.rows.len());
    let mut output_timesteps = Vec::with_capacity(request.rows.len());
    let mut decoded_text = Vec::with_capacity(request.rows.len());
    for row in &request.rows {
        if !row.path.is_absolute() || !valid_sha256(&row.file_sha256) {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let crop_bytes = read_bounded_regular(&row.path, MAX_BATCH_CROP_BYTES)?;
        if encode_sha256(&crop_bytes) != row.file_sha256 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let (source_width, source_height, pixels) = strict_p6(&crop_bytes)?;
        let (observation, input_tensor_sha256) = observe_dynamic_rgb8(
            &mut session,
            &dictionary,
            output_classes,
            pixels,
            source_width,
            source_height,
        )?;
        input_widths.push(observation.input_width);
        input_tensor_sha256s.push(input_tensor_sha256);
        output_timesteps.push(observation.output_timesteps);
        decoded_text.push(observation.open_text);
    }
    Ok(DynamicOfficialOnnxDecodeSummary {
        schema: "scorepeek-official-onnx-dynamic-open-text-batch-v1",
        request_sha256,
        model_id: manifest.model_id,
        model_sha256: model_file.sha256.clone(),
        dictionary_sha256: dictionary_file.sha256.clone(),
        preprocessor_id: DYNAMIC_TITLE_PREPROCESSOR_ID,
        elapsed_ms: started.elapsed().as_millis(),
        input_widths,
        input_tensor_sha256s,
        output_timesteps,
        decoded_text,
    })
}

fn observe_dynamic_rgb8(
    session: &mut Session,
    dictionary: &[String],
    output_classes: usize,
    pixels: &[u8],
    source_width: usize,
    source_height: usize,
) -> Result<(DynamicTextObservation, String), OnnxParityError> {
    let input = preprocess_dynamic_title_image(pixels, source_width, source_height)?;
    let input_tensor_sha256 = encode_f32_sha256(&input.values);
    let input_shape = [1, 3, DYNAMIC_TITLE_INPUT_HEIGHT, input.width];
    let outputs = session.run(ort::inputs![Tensor::from_array((
        input_shape,
        input.values
    ))?])?;
    let (shape, probabilities) = outputs[0].try_extract_tensor::<f32>()?;
    let [batch, timesteps, classes] = shape.as_ref() else {
        return Err(OnnxParityError::InvalidArtifact);
    };
    let timesteps = usize::try_from(*timesteps).map_err(|_| OnnxParityError::InvalidArtifact)?;
    if *batch != 1
        || timesteps == 0
        || usize::try_from(*classes).map_err(|_| OnnxParityError::InvalidArtifact)?
            != output_classes
        || probabilities.len() != timesteps * output_classes
    {
        return Err(OnnxParityError::InvalidArtifact);
    }
    validate_argmax_probability_rows(probabilities, output_classes)?;
    let (_, collapsed) = argmax_tokens(probabilities, timesteps, output_classes)?;
    let mut open_text = String::new();
    for token in collapsed {
        open_text.push_str(
            dictionary
                .get(usize::try_from(token).map_err(|_| OnnxParityError::InvalidArtifact)?)
                .ok_or(OnnxParityError::InvalidArtifact)?,
        );
    }
    Ok((
        DynamicTextObservation {
            input_width: input_shape[3],
            output_timesteps: timesteps,
            open_text,
        },
        input_tensor_sha256,
    ))
}

fn strict_p6(bytes: &[u8]) -> Result<(usize, usize, &[u8]), OnnxParityError> {
    let mut parts = bytes.splitn(4, |byte| *byte == b'\n');
    let magic = parts.next().ok_or(OnnxParityError::InvalidArtifact)?;
    let dimensions = parts.next().ok_or(OnnxParityError::InvalidArtifact)?;
    let maximum = parts.next().ok_or(OnnxParityError::InvalidArtifact)?;
    let pixels = parts.next().ok_or(OnnxParityError::InvalidArtifact)?;
    let dimensions = std::str::from_utf8(dimensions)
        .map_err(|_| OnnxParityError::InvalidArtifact)?
        .split_whitespace()
        .map(str::parse::<usize>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| OnnxParityError::InvalidArtifact)?;
    let [width, height] = dimensions.as_slice() else {
        return Err(OnnxParityError::InvalidArtifact);
    };
    if magic != b"P6"
        || maximum != b"255"
        || *width == 0
        || *height == 0
        || *width > 4_096
        || *height > 4_096
        || pixels.len() != width * height * 3
    {
        return Err(OnnxParityError::InvalidArtifact);
    }
    Ok((*width, *height, pixels))
}

fn load_verified_title_inputs(
    request: &OnnxTitleDiagnosticRequest<'_>,
) -> Result<VerifiedTitleInputs, OnnxParityError> {
    if !valid_sha256(request.reference_sha256) || !valid_sha256(request.catalog_sha256) {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let model_manifest = OnnxModelManifest::load_registered()?;
    let model_bytes = read_exact_regular(request.model_path, MODEL_BYTES)?;
    if encode_sha256(&model_bytes) != model_manifest.sha256 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    if !request.reference_directory.is_absolute()
        || !request.reference_directory.metadata()?.is_dir()
    {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let manifest_bytes = read_bounded_regular(
        &request.reference_directory.join("manifest.json"),
        MAX_REFERENCE_MANIFEST_BYTES,
    )?;
    if encode_sha256(&manifest_bytes) != request.reference_sha256 {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let reference: ParityReference = serde_json::from_slice(&manifest_bytes)?;
    reference.validate(&model_manifest)?;
    let input = decode_f32(&reference.input.read(request.reference_directory)?)?;
    let paddle_output = decode_f32(&reference.paddle_output.read(request.reference_directory)?)?;
    let (title_roi, title_pixels) =
        read_title_crop_artifact(request.crop_directory, &reference.crop_manifest_sha256)?;
    let rust_input = preprocess_title_crop(&title_pixels, title_roi)?;
    let maximum_input_absolute_error = rust_input
        .iter()
        .zip(&input)
        .map(|(rust, paddle)| (rust - paddle).abs())
        .fold(0.0_f32, f32::max);
    if maximum_input_absolute_error > MAX_INPUT_ABSOLUTE_ERROR {
        return Err(OnnxParityError::TensorMismatch);
    }
    Ok(VerifiedTitleInputs {
        model_manifest,
        model_bytes,
        reference,
        rust_input,
        paddle_output,
        maximum_input_absolute_error,
    })
}

impl ParityReference {
    fn validate(&self, model: &OnnxModelManifest) -> Result<(), OnnxParityError> {
        if self.schema != "scorepeek-ocr-paddle-parity-reference-v1"
            || !valid_sha256(&self.frame_extraction_sha256)
            || !valid_sha256(&self.crop_manifest_sha256)
            || !valid_sha256(&self.title_crop_file_sha256)
            || !valid_sha256(&self.candidate_source_sha256)
            || self.paddle_model_id != model.paddle_model_id
            || self.paddle_model_archive_sha256
                != "da460f968ce9f88325ac3a34fa302077d6e9b0dcefb16ba3137cd7796f879d06"
            || self.onnx_model_id != model.model_id
            || self.onnx_model_sha256 != model.sha256
            || self.paddle_inference_json_sha256 != model.paddle_inference_json_sha256
            || self.paddle_inference_yml_sha256 != model.paddle_inference_yml_sha256
            || self.preprocessor_id != TITLE_PREPROCESSOR_ID
            || self.input.filename != "input.f32le"
            || self.input.bytes != INPUT_BYTES
            || self.input.shape != TITLE_INPUT_SHAPE
            || self.paddle_output.filename != "paddle-output.f32le"
            || self.paddle_output.bytes != OUTPUT_BYTES
            || self.paddle_output.shape != OUTPUT_SHAPE
            || self.ctc_blank_token != 0
            || self.argmax_token_order.len() != OUTPUT_SHAPE[1]
            || self
                .argmax_token_order
                .iter()
                .any(|token| *token >= OUTPUT_CLASSES)
            || self
                .collapsed_token_order
                .iter()
                .any(|token| *token == 0 || *token >= OUTPUT_CLASSES)
            || self.candidate_ranking.len() < 2
            || self.candidate_ranking.len() > 1_024
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let mut previous: Option<(&str, f64)> = None;
        let mut song_ids = BTreeSet::new();
        for candidate in &self.candidate_ranking {
            let Ok(song_id) = uuid::Uuid::parse_str(&candidate.song_id) else {
                return Err(OnnxParityError::InvalidArtifact);
            };
            if song_id.to_string() != candidate.song_id
                || !song_ids.insert(candidate.song_id.as_str())
                || candidate.title.is_empty()
                || candidate.title.len() > 1_024
                || candidate.title.chars().any(char::is_control)
                || candidate.tokens.is_empty()
                || candidate.tokens.len() > 256
                || candidate
                    .tokens
                    .iter()
                    .any(|token| *token == 0 || *token >= OUTPUT_CLASSES)
                || !candidate.paddle_log_probability.is_finite()
            {
                return Err(OnnxParityError::InvalidArtifact);
            }
            if let Some((previous_id, previous_score)) = previous {
                let order = previous_score
                    .total_cmp(&candidate.paddle_log_probability)
                    .reverse()
                    .then_with(|| previous_id.cmp(&candidate.song_id));
                if order == Ordering::Greater {
                    return Err(OnnxParityError::InvalidArtifact);
                }
            }
            previous = Some((&candidate.song_id, candidate.paddle_log_probability));
        }
        Ok(())
    }
}

impl ExportContractReference {
    fn validate(&self, model_sha256: &str) -> Result<(), OnnxParityError> {
        let hashes = [
            &self.training_preparation_sha256,
            &self.validation_list_sha256,
            &self.dictionary_sha256,
            &self.crop_file_sha256,
            &self.export_manifest_sha256,
            &self.onnx_model_sha256,
            &self.inference_config_sha256,
        ];
        let output_classes = self.paddle_output.shape.get(2).copied().unwrap_or(0);
        if self.schema != "scorepeek-private-title-model-export-parity-reference-v1"
            || hashes.into_iter().any(|hash| !valid_sha256(hash))
            || self.validation_row_index != 0
            || self.onnx_model_sha256 != model_sha256
            || !valid_presentation_transform_id(&self.presentation_transform_id)
            || self.input.filename != "input.f32le"
            || self.paddle_output.filename != "paddle-output.f32le"
            || self.input.bytes != self.input.shape.iter().product::<usize>() as u64 * 4
            || self.paddle_output.bytes
                != self.paddle_output.shape.iter().product::<usize>() as u64 * 4
            || self.ctc_blank_token != 0
            || self.argmax_token_order.len()
                != self.paddle_output.shape.get(1).copied().unwrap_or(0)
            || self
                .argmax_token_order
                .iter()
                .any(|token| usize::try_from(*token).map_or(true, |token| token >= output_classes))
            || self.collapsed_token_order.iter().any(|token| {
                *token == 0 || usize::try_from(*token).map_or(true, |token| token >= output_classes)
            })
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(())
    }
}

fn valid_presentation_transform_id(value: &str) -> bool {
    matches!(
        value,
        IDENTITY_PRESENTATION_TRANSFORM_ID | CHANNEL_MAX_PRESENTATION_TRANSFORM_ID
    )
}

impl TensorArtifact {
    fn read(&self, directory: &Path) -> Result<Vec<u8>, OnnxParityError> {
        if !valid_sha256(&self.sha256) {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let bytes = read_exact_regular(&directory.join(&self.filename), self.bytes)?;
        if encode_sha256(&bytes) != self.sha256 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(bytes)
    }
}

fn read_exact_regular(path: &Path, exact: u64) -> Result<Vec<u8>, OnnxParityError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() != exact {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let capacity = usize::try_from(exact).map_err(|_| OnnxParityError::InvalidArtifact)?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)?.take(exact + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != exact {
        return Err(OnnxParityError::InvalidArtifact);
    }
    Ok(bytes)
}

fn read_bounded_regular(path: &Path, maximum: u64) -> Result<Vec<u8>, OnnxParityError> {
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| OnnxParityError::InvalidArtifact)?;
    let mut bytes = Vec::with_capacity(capacity);
    File::open(path)?
        .take(maximum + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(OnnxParityError::InvalidArtifact);
    }
    Ok(bytes)
}

fn decode_f32(bytes: &[u8]) -> Result<Vec<f32>, OnnxParityError> {
    if !bytes.len().is_multiple_of(4) {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let values: Vec<_> = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes(chunk.try_into().expect("chunk size is fixed")))
        .collect();
    if values.iter().any(|value| !value.is_finite()) {
        return Err(OnnxParityError::InvalidArtifact);
    }
    Ok(values)
}

fn argmax_tokens(
    probabilities: &[f32],
    timesteps: usize,
    classes: usize,
) -> Result<(Vec<u32>, Vec<u32>), OnnxParityError> {
    if probabilities.len() != timesteps * classes {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let mut raw = Vec::with_capacity(timesteps);
    for row in probabilities.chunks_exact(classes) {
        let mut token = 0_usize;
        for (index, value) in row.iter().enumerate().skip(1) {
            if value.total_cmp(&row[token]) == Ordering::Greater {
                token = index;
            }
        }
        raw.push(u32::try_from(token).map_err(|_| OnnxParityError::InvalidArtifact)?);
    }
    let mut collapsed = Vec::new();
    let mut previous = None;
    for token in &raw {
        if *token != 0 && Some(*token) != previous {
            collapsed.push(*token);
        }
        previous = Some(*token);
    }
    Ok((raw, collapsed))
}

fn ctc_log_probability(
    probabilities: &[f32],
    timesteps: usize,
    classes: usize,
    tokens: &[u32],
) -> Result<f64, OnnxParityError> {
    if probabilities.len() != timesteps * classes
        || tokens.is_empty()
        || tokens.iter().any(|token| {
            *token == 0 || usize::try_from(*token).map_or(true, |token| token >= classes)
        })
    {
        return Err(OnnxParityError::InvalidArtifact);
    }
    let mut labels = Vec::with_capacity(tokens.len() * 2 + 1);
    labels.push(0_u32);
    for token in tokens {
        labels.push(*token);
        labels.push(0);
    }
    let probability = |time: usize, token: u32| -> Result<f64, OnnxParityError> {
        let token = usize::try_from(token).map_err(|_| OnnxParityError::InvalidArtifact)?;
        let value = probabilities[time * classes + token];
        if !value.is_finite() || value <= 0.0 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(f64::from(value).ln())
    };
    let mut previous = vec![f64::NEG_INFINITY; labels.len()];
    previous[0] = probability(0, 0)?;
    previous[1] = probability(0, labels[1])?;
    for timestep in 1..timesteps {
        let mut current = vec![f64::NEG_INFINITY; labels.len()];
        for (state, token) in labels.iter().copied().enumerate() {
            let mut sources = [f64::NEG_INFINITY; 3];
            sources[0] = previous[state];
            if state > 0 {
                sources[1] = previous[state - 1];
            }
            if state > 1 && token != 0 && token != labels[state - 2] {
                sources[2] = previous[state - 2];
            }
            current[state] = logsumexp(&sources) + probability(timestep, token)?;
        }
        previous = current;
    }
    Ok(logsumexp(&previous[previous.len() - 2..]))
}

fn logsumexp(values: &[f64]) -> f64 {
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if maximum == f64::NEG_INFINITY {
        maximum
    } else {
        maximum
            + values
                .iter()
                .map(|value| (value - maximum).exp())
                .sum::<f64>()
                .ln()
    }
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
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn encode_f32_sha256(values: &[f32]) -> String {
    let mut digest = Sha256::new();
    for value in values {
        digest.update(value.to_le_bytes());
    }
    let mut encoded = String::with_capacity(64);
    for byte in digest.finalize() {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        DynamicBundleManifest, LIVE_MODEL_SHA256, LIVE_RUNTIME_SHA256, LiveRuntimeManifest,
        RegisteredRecognitionResources, RegisteredResourceLoadError,
        RegisteredResourceLoadErrorType, argmax_tokens, ctc_log_probability, strict_p6,
        valid_presentation_transform_id, validate_argmax_probability_rows,
    };
    use crate::catalog::{Catalog, CatalogStore};

    fn load_error(
        result: Result<RegisteredRecognitionResources, RegisteredResourceLoadError>,
    ) -> RegisteredResourceLoadError {
        match result {
            Ok(_) => panic!("resource load unexpectedly succeeded"),
            Err(error) => error,
        }
    }

    #[test]
    fn registered_live_runtime_manifest_is_exact() {
        let manifest = LiveRuntimeManifest::load_registered().unwrap();
        assert_eq!(manifest.intra_threads, 1);
        assert_eq!(manifest.inter_threads, 1);
        assert!(!manifest.parallel_execution);
        assert_eq!(manifest.execution_provider, "CPUExecutionProvider");
    }

    #[test]
    fn registered_resources_reject_binding_and_catalog_failures_before_runtime_loading() {
        let missing = tempfile::tempdir().unwrap();
        let bundle = tempfile::tempdir().unwrap();
        let model_mismatch = load_error(RegisteredRecognitionResources::load(
            missing.path(),
            bundle.path(),
            &"1".repeat(64),
            &"2".repeat(64),
            LIVE_RUNTIME_SHA256,
        ));
        assert_eq!(
            model_mismatch.error_type(),
            RegisteredResourceLoadErrorType::ModelBindingMismatch
        );
        let runtime_mismatch = load_error(RegisteredRecognitionResources::load(
            missing.path(),
            bundle.path(),
            &"1".repeat(64),
            LIVE_MODEL_SHA256,
            &"2".repeat(64),
        ));
        assert_eq!(
            runtime_mismatch.error_type(),
            RegisteredResourceLoadErrorType::RuntimeBindingMismatch
        );
        let unavailable = load_error(RegisteredRecognitionResources::load(
            missing.path(),
            bundle.path(),
            &"1".repeat(64),
            LIVE_MODEL_SHA256,
            LIVE_RUNTIME_SHA256,
        ));
        assert!(matches!(
            unavailable,
            RegisteredResourceLoadError::CatalogUnavailable
        ));

        let catalog_root = tempfile::tempdir().unwrap();
        let active = CatalogStore::new(catalog_root.path())
            .begin_update()
            .unwrap()
            .publish(&Catalog::default())
            .unwrap();
        let mismatch = load_error(RegisteredRecognitionResources::load(
            catalog_root.path(),
            bundle.path(),
            &"3".repeat(64),
            LIVE_MODEL_SHA256,
            LIVE_RUNTIME_SHA256,
        ));
        assert_ne!(active.digest, "3".repeat(64));
        assert_eq!(
            mismatch.error_type(),
            RegisteredResourceLoadErrorType::CatalogBindingMismatch
        );
    }

    #[test]
    fn registered_resource_location_errors_retain_role_and_io_source() {
        let root = tempfile::tempdir().unwrap();
        let bundle = tempfile::tempdir().unwrap();
        let missing_catalog = root.path().join("missing-catalog");
        let catalog_error = load_error(RegisteredRecognitionResources::load(
            &missing_catalog,
            bundle.path(),
            &"1".repeat(64),
            LIVE_MODEL_SHA256,
            LIVE_RUNTIME_SHA256,
        ));
        assert_eq!(
            catalog_error.error_type(),
            RegisteredResourceLoadErrorType::InvalidLocation
        );
        assert!(
            catalog_error
                .to_string()
                .contains("catalog store metadata failed")
        );
        assert!(std::error::Error::source(&catalog_error).is_some());

        let missing_bundle = root.path().join("missing-bundle");
        let bundle_error = load_error(RegisteredRecognitionResources::load(
            root.path(),
            &missing_bundle,
            &"1".repeat(64),
            LIVE_MODEL_SHA256,
            LIVE_RUNTIME_SHA256,
        ));
        assert!(
            bundle_error
                .to_string()
                .contains("model bundle metadata failed")
        );
        assert!(std::error::Error::source(&bundle_error).is_some());
    }

    #[test]
    fn registered_tiny_bundle_manifest_is_exact() {
        let manifest = DynamicBundleManifest::load_registered("pp-ocrv6-tiny-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv6-tiny-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 6_906);
        assert_eq!(manifest.files.len(), 3);
    }

    #[test]
    fn registered_small_bundle_manifest_is_exact() {
        let manifest =
            DynamicBundleManifest::load_registered("pp-ocrv6-small-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv6-small-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 18_710);
        assert_eq!(manifest.files.len(), 3);
    }

    #[test]
    fn registered_medium_bundle_manifest_is_exact() {
        let manifest =
            DynamicBundleManifest::load_registered("pp-ocrv6-medium-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv6-medium-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 18_710);
        assert_eq!(manifest.files.len(), 3);
    }

    #[test]
    fn registered_v5_mobile_bundle_manifest_is_exact() {
        let manifest =
            DynamicBundleManifest::load_registered("pp-ocrv5-mobile-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv5-mobile-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 18_385);
        assert_eq!(manifest.files.len(), 2);
    }

    #[test]
    fn registered_v5_server_bundle_manifest_is_exact() {
        let manifest =
            DynamicBundleManifest::load_registered("pp-ocrv5-server-rec-onnx-v1").unwrap();
        assert_eq!(manifest.model_id, "pp-ocrv5-server-rec-onnx-v1");
        assert_eq!(manifest.native_contract.output_classes, 18_385);
        assert_eq!(manifest.files.len(), 2);
    }

    #[test]
    fn export_contract_accepts_only_registered_presentation_transforms() {
        assert!(valid_presentation_transform_id(
            "scorepeek-title-rgb-identity-v1"
        ));
        assert!(valid_presentation_transform_id(
            "scorepeek-title-channel-max-rgb-v1"
        ));
        assert!(!valid_presentation_transform_id("unknown"));
    }

    #[test]
    fn ctc_score_sums_blank_repeat_and_direct_alignments() {
        let probabilities = [
            0.6_f32, 0.4, 0.0, // blank or A
            0.2, 0.7, 0.1, // A
            0.5, 0.4, 0.1, // blank or A
        ];
        let score = ctc_log_probability(&probabilities, 3, 3, &[1]).unwrap();
        let expected = 0.6 * 0.7 * 0.5
            + 0.4 * 0.7 * 0.5
            + 0.6 * 0.7 * 0.4
            + 0.4 * 0.7 * 0.4
            + 0.4 * 0.2 * 0.5
            + 0.6 * 0.2 * 0.4;
        assert!((score.exp() - expected).abs() < 1e-6);
    }

    #[test]
    fn argmax_order_collapses_repeats_across_blanks() {
        let probabilities = [
            0.9_f32, 0.1, 0.0, // blank
            0.1, 0.8, 0.1, // A
            0.1, 0.7, 0.2, // repeated A
            0.8, 0.1, 0.1, // blank
            0.1, 0.7, 0.2, // A again
        ];
        let (raw, collapsed) = argmax_tokens(&probabilities, 5, 3).unwrap();
        assert_eq!(raw, [0, 1, 1, 0, 1]);
        assert_eq!(collapsed, [1, 1]);
    }

    #[test]
    fn argmax_order_accepts_an_all_blank_collapse() {
        let probabilities = [0.9_f32, 0.1, 0.8, 0.2, 0.7, 0.3];
        let (raw, collapsed) = argmax_tokens(&probabilities, 3, 2).unwrap();
        assert_eq!(raw, [0, 0, 0]);
        assert!(collapsed.is_empty());
    }

    #[test]
    fn batch_crop_reader_accepts_only_strict_complete_p6() {
        let bytes = b"P6\n2 1\n255\n\x00\x01\x02\x03\x04\x05";
        assert_eq!(strict_p6(bytes).unwrap(), (2, 1, &bytes[11..]));
        assert!(strict_p6(b"P6\n2 1\n255\n\x00").is_err());
        assert!(strict_p6(b"P3\n2 1\n255\n\x00\x01\x02\x03\x04\x05").is_err());
    }

    #[test]
    fn batch_decode_rejects_invalid_probability_rows() {
        assert!(validate_argmax_probability_rows(&[0.0, 1.0], 2).is_ok());
        assert!(validate_argmax_probability_rows(&[0.000_05, 1.0], 2).is_ok());
        assert!(validate_argmax_probability_rows(&[f32::NAN, 1.0], 2).is_err());
        assert!(validate_argmax_probability_rows(&[-0.25, 1.25], 2).is_err());
        assert!(validate_argmax_probability_rows(&[0.25, 0.25], 2).is_err());
        assert!(validate_argmax_probability_rows(&[0.001, 1.0], 2).is_err());
    }
}
