use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Read as _;
use std::path::Path;

use ort::session::Session;
use ort::value::Tensor;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::title_decoder::{
    CatalogTitleDecision, CatalogTitleDecoderError, DiagnosticTitleThresholds,
    TITLE_DICTIONARY_SHA256, load_dictionary_contract, score_catalog_titles,
};
use super::title_preprocessor::{
    TITLE_INPUT_SHAPE, TITLE_INPUT_VALUES, TITLE_PREPROCESSOR_ID, preprocess_title_crop,
};
use super::{RecognitionError, read_title_crop_artifact};
use crate::catalog::Catalog;

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

#[derive(Debug)]
pub enum OnnxParityError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Ort(ort::Error),
    Recognition(RecognitionError),
    CatalogDecoder(CatalogTitleDecoderError),
    InvalidArtifact,
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
        || !request.reference_directory.symlink_metadata()?.is_dir()
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
    let metadata = path.symlink_metadata()?;
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
    let metadata = path.symlink_metadata()?;
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

#[cfg(test)]
mod tests {
    use super::{argmax_tokens, ctc_log_probability};

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
}
