use std::cmp::Ordering;
use std::fmt::Write as _;
use std::sync::Arc;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::Tensor;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use super::decode::{CatalogTitleDecoderError, load_dictionary_contract};
use super::preprocess::{
    DYNAMIC_TITLE_INPUT_HEIGHT, DYNAMIC_TITLE_PREPROCESSOR_ID, preprocess_dynamic_title_image,
};
pub use crate::model::manifest::RegisteredLiveModelFile;
use crate::model::manifest::{DynamicBundleManifest, ManifestError};
pub use crate::model::text::DynamicTextObservation;
use crate::recognition::screen::{RecognitionError, Rgb8Crop};
use crate::recognition::shared::ctc::CtcSequenceTrie;

pub use crate::model::registry::{
    LIVE_MODEL_BUNDLE_MANIFEST_SHA256, LIVE_MODEL_SHA256, LIVE_RUNTIME_SHA256,
};
const LIVE_RUNTIME_MANIFEST_BYTES: &[u8] =
    include_bytes!("../../../../../models/manifests/pp-ocrv6-small-live-runtime-v5.json");
pub const LIVE_MODEL_ID: &str = "pp-ocrv6-small-rec-onnx-v1";

#[derive(Debug)]
pub enum OnnxParityError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Ort(ort::Error),
    Recognition(RecognitionError),
    CatalogDecoder(CatalogTitleDecoderError),
    InvalidArtifact,
    WorkerUnavailable,
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
            Self::Recognition(error) => write!(formatter, "screen recognition failed: {error}"),
            Self::CatalogDecoder(error) => {
                write!(formatter, "catalog title decoding failed: {error}")
            }
            Self::InvalidArtifact => formatter.write_str("ONNX parity artifact is invalid"),
            Self::WorkerUnavailable => formatter.write_str("recognition worker is unavailable"),
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

impl From<ManifestError> for OnnxParityError {
    fn from(error: ManifestError) -> Self {
        match error {
            ManifestError::Json(error) => Self::Json(error),
            ManifestError::InvalidArtifact => Self::InvalidArtifact,
        }
    }
}

/// Returns the verified download contract embedded for the live PP-OCRv6-small bundle.
///
/// # Errors
/// Returns an error if the embedded manifest no longer matches the compiled registration.
pub fn registered_live_model_files() -> Result<Vec<RegisteredLiveModelFile>, OnnxParityError> {
    crate::model::manifest::registered_live_model_files().map_err(Into::into)
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
    live_available_parallelism_divisor: usize,
    offline_reserved_parallelism: usize,
    default_maximum_text_workers: usize,
}

impl LiveRuntimeManifest {
    fn load_registered() -> Result<Self, OnnxParityError> {
        if encode_sha256(LIVE_RUNTIME_MANIFEST_BYTES) != LIVE_RUNTIME_SHA256 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let manifest: Self = serde_json::from_slice(LIVE_RUNTIME_MANIFEST_BYTES)?;
        if manifest.schema != "scorepeek-field-text-runtime-v5"
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
            || manifest.decoder_id != "scorepeek-ctc-open-greedy-numeric-exact-v1"
            || manifest.model_bundle_manifest_sha256 != LIVE_MODEL_BUNDLE_MANIFEST_SHA256
            || manifest.live_available_parallelism_divisor != 2
            || manifest.offline_reserved_parallelism != 4
            || manifest.default_maximum_text_workers != 12
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(manifest)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CtcCharacterSet {
    Digits,
    DigitsUpToTwo,
    DigitsAndDashes,
    DigitsAndDashesUpToThree,
}

impl CtcCharacterSet {
    const fn maximum_digits(self) -> usize {
        match self {
            Self::DigitsUpToTwo => 2,
            Self::DigitsAndDashesUpToThree => 3,
            Self::Digits | Self::DigitsAndDashes => 4,
        }
    }

    const fn includes_dashes(self) -> bool {
        matches!(self, Self::DigitsAndDashes | Self::DigitsAndDashesUpToThree)
    }
}

impl NumericCtcDecoder {
    fn new(dictionary: &[String], character_set: CtcCharacterSet) -> Result<Self, OnnxParityError> {
        let mut digit_tokens = [None; 10];
        let mut dash_tokens = Vec::new();
        for (index, token) in dictionary.iter().enumerate().skip(1) {
            let index = u32::try_from(index).map_err(|_| OnnxParityError::InvalidArtifact)?;
            if token.len() == 1 && token.as_bytes()[0].is_ascii_digit() {
                let digit = usize::from(token.as_bytes()[0] - b'0');
                if digit_tokens[digit].replace(index).is_some() {
                    return Err(OnnxParityError::InvalidArtifact);
                }
            } else if character_set.includes_dashes()
                && matches!(token.as_str(), "-" | "―" | "ー" | "—")
            {
                dash_tokens.push((index, token.clone()));
            }
        }
        let digit_tokens = digit_tokens
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or(OnnxParityError::InvalidArtifact)?;
        let mut candidates = CtcSequenceTrie::default();
        let mut sequences = vec![(String::new(), Vec::new())];
        for _ in 0..character_set.maximum_digits() {
            let mut next = Vec::with_capacity(sequences.len() * 10);
            for (prefix_text, prefix_tokens) in &sequences {
                for (digit, token) in digit_tokens.iter().copied().enumerate() {
                    let mut text = prefix_text.clone();
                    let digit =
                        u8::try_from(digit).map_err(|_| OnnxParityError::InvalidArtifact)?;
                    text.push(char::from(b'0' + digit));
                    let mut tokens = prefix_tokens.clone();
                    tokens.push(token);
                    if !candidates.insert(&tokens, text.clone()) {
                        return Err(OnnxParityError::InvalidArtifact);
                    }
                    next.push((text, tokens));
                }
            }
            sequences = next;
        }
        if character_set.includes_dashes() {
            if dash_tokens.is_empty() {
                return Err(OnnxParityError::InvalidArtifact);
            }
            for (first_token, first_text) in &dash_tokens {
                if !candidates.insert(&[*first_token], first_text.clone()) {
                    return Err(OnnxParityError::InvalidArtifact);
                }
                for (second_token, second_text) in &dash_tokens {
                    if !candidates.insert(
                        &[*first_token, *second_token],
                        format!("{first_text}{second_text}"),
                    ) {
                        return Err(OnnxParityError::InvalidArtifact);
                    }
                }
            }
        }
        if candidates.is_empty() {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(Self { candidates })
    }

    fn decode(&self, probabilities: &[f32], classes: usize) -> Result<String, OnnxParityError> {
        let scores = self
            .candidates
            .score(probabilities, classes)
            .ok_or(OnnxParityError::InvalidArtifact)?;
        let Some((text, score)) = scores.values.into_iter().max_by(
            |(left_text, left_score), (right_text, right_score)| {
                left_score
                    .total_cmp(right_score)
                    .then_with(|| right_text.cmp(left_text))
            },
        ) else {
            return Err(OnnxParityError::InvalidArtifact);
        };
        Ok(
            if score.total_cmp(&scores.blank_log_probability) == Ordering::Greater {
                text.clone()
            } else {
                String::new()
            },
        )
    }
}

/// The exact registered live text runtime, loaded once and owned by one observer worker.
pub struct RegisteredDynamicTitleRuntime {
    session: Session,
    model_bytes: Arc<[u8]>,
    dictionary: Vec<String>,
    output_classes: usize,
    numeric_digits: NumericCtcDecoder,
    numeric_digits_up_to_two: NumericCtcDecoder,
    numeric_digits_and_dashes: NumericCtcDecoder,
    numeric_digits_and_dashes_up_to_three: NumericCtcDecoder,
}

struct NumericCtcDecoder {
    candidates: CtcSequenceTrie<String>,
}

impl RegisteredDynamicTitleRuntime {
    /// Verifies the complete registered PP-OCRv6-small bundle and constructs its fixed CPU session.
    ///
    /// # Errors
    /// Returns an error for missing, changed, or malformed bundle bytes or runtime initialization
    /// failure. No runtime download or fallback is attempted.
    pub fn from_registered_bundle_bytes(files: &[(&str, &[u8])]) -> Result<Self, OnnxParityError> {
        let runtime = LiveRuntimeManifest::load_registered()?;
        let manifest = DynamicBundleManifest::load_registered(LIVE_MODEL_ID)?;
        let model_bytes: Arc<[u8]> = Arc::from(manifest.verified_model_bytes(files)?);
        let dictionary_file = manifest
            .file("inference.yml")
            .ok_or(OnnxParityError::InvalidArtifact)?;
        let dictionary_bytes = files
            .iter()
            .find_map(|(name, bytes)| (*name == "inference.yml").then_some(*bytes))
            .ok_or(OnnxParityError::InvalidArtifact)?;
        let dictionary = load_dictionary_contract(
            dictionary_bytes,
            dictionary_file.sha256(),
            manifest.output_classes(),
        )?;
        Self::from_verified(&runtime, model_bytes, dictionary, manifest.output_classes())
    }

    /// Constructs another independent session from the already-verified model and dictionary.
    ///
    /// # Errors
    /// Returns an error when the fixed registered runtime cannot initialize another session.
    pub fn spawn_peer(&self) -> Result<Self, OnnxParityError> {
        Self::from_verified(
            &LiveRuntimeManifest::load_registered()?,
            Arc::clone(&self.model_bytes),
            self.dictionary.clone(),
            self.output_classes,
        )
    }

    fn from_verified(
        runtime: &LiveRuntimeManifest,
        model_bytes: Arc<[u8]>,
        dictionary: Vec<String>,
        output_classes: usize,
    ) -> Result<Self, OnnxParityError> {
        let numeric_digits = NumericCtcDecoder::new(&dictionary, CtcCharacterSet::Digits)?;
        let numeric_digits_up_to_two =
            NumericCtcDecoder::new(&dictionary, CtcCharacterSet::DigitsUpToTwo)?;
        let numeric_digits_and_dashes =
            NumericCtcDecoder::new(&dictionary, CtcCharacterSet::DigitsAndDashes)?;
        let numeric_digits_and_dashes_up_to_three =
            NumericCtcDecoder::new(&dictionary, CtcCharacterSet::DigitsAndDashesUpToThree)?;
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
            model_bytes,
            dictionary,
            output_classes,
            numeric_digits,
            numeric_digits_up_to_two,
            numeric_digits_and_dashes,
            numeric_digits_and_dashes_up_to_three,
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
            None,
        )
        .map(|(observation, _)| observation)
    }

    /// Runs one inference and decodes both unrestricted text and a field-local character set.
    ///
    /// # Errors
    /// Returns an error for an invalid crop, dictionary, or runtime tensor contract.
    pub fn observe_constrained_text(
        &mut self,
        crop: &Rgb8Crop,
        character_set: CtcCharacterSet,
    ) -> Result<DynamicTextObservation, OnnxParityError> {
        let numeric_decoder = match character_set {
            CtcCharacterSet::Digits => &self.numeric_digits,
            CtcCharacterSet::DigitsUpToTwo => &self.numeric_digits_up_to_two,
            CtcCharacterSet::DigitsAndDashes => &self.numeric_digits_and_dashes,
            CtcCharacterSet::DigitsAndDashesUpToThree => {
                &self.numeric_digits_and_dashes_up_to_three
            }
        };
        observe_dynamic_rgb8(
            &mut self.session,
            &self.dictionary,
            self.output_classes,
            crop.pixels(),
            crop.roi.width as usize,
            crop.roi.height as usize,
            Some(numeric_decoder),
        )
        .map(|(observation, _)| observation)
    }
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

fn observe_dynamic_rgb8(
    session: &mut Session,
    dictionary: &[String],
    output_classes: usize,
    pixels: &[u8],
    source_width: usize,
    source_height: usize,
    numeric_decoder: Option<&NumericCtcDecoder>,
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
    let open_text = decode_dictionary_tokens(dictionary, &collapsed)?;
    let constrained_text = numeric_decoder
        .map(|decoder| decoder.decode(probabilities, output_classes))
        .transpose()?;
    Ok((
        DynamicTextObservation {
            input_width: input_shape[3],
            output_timesteps: timesteps,
            open_text,
            constrained_text,
        },
        input_tensor_sha256,
    ))
}

fn decode_dictionary_tokens(
    dictionary: &[String],
    tokens: &[u32],
) -> Result<String, OnnxParityError> {
    let mut text = String::new();
    for token in tokens {
        text.push_str(
            dictionary
                .get(usize::try_from(*token).map_err(|_| OnnxParityError::InvalidArtifact)?)
                .ok_or(OnnxParityError::InvalidArtifact)?,
        );
    }
    Ok(text)
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
        CtcCharacterSet, LiveRuntimeManifest, NumericCtcDecoder, argmax_tokens,
        validate_argmax_probability_rows,
    };

    #[test]
    fn registered_live_runtime_manifest_is_exact() {
        let manifest = LiveRuntimeManifest::load_registered().unwrap();
        assert_eq!(manifest.intra_threads, 1);
        assert_eq!(manifest.inter_threads, 1);
        assert!(!manifest.parallel_execution);
        assert_eq!(manifest.execution_provider, "CPUExecutionProvider");
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
    fn numeric_sequence_decode_sums_alignments_that_greedy_discards() {
        let dictionary = numeric_test_dictionary();
        let decoder = NumericCtcDecoder::new(&dictionary, CtcCharacterSet::Digits).unwrap();
        let classes = dictionary.len();
        let mut probabilities = vec![0.0_f32; classes * 2];
        probabilities[0] = 0.6;
        probabilities[1] = 0.4;
        probabilities[classes] = 0.6;
        probabilities[classes + 1] = 0.4;

        let (_, greedy) = argmax_tokens(&probabilities, 2, classes).unwrap();
        assert!(greedy.is_empty());
        assert_eq!(decoder.decode(&probabilities, classes).unwrap(), "0");
    }

    #[test]
    fn numeric_sequence_decode_preserves_repeated_digits_and_dash_sequences() {
        let dictionary = numeric_test_dictionary();
        let classes = dictionary.len();
        let digits = NumericCtcDecoder::new(&dictionary, CtcCharacterSet::Digits).unwrap();
        let dashes = NumericCtcDecoder::new(&dictionary, CtcCharacterSet::DigitsAndDashes).unwrap();
        let mut repeated_digit = vec![0.0_f32; classes * 3];
        repeated_digit[2] = 1.0;
        repeated_digit[classes] = 1.0;
        repeated_digit[classes * 2 + 2] = 1.0;
        assert_eq!(digits.decode(&repeated_digit, classes).unwrap(), "11");

        let dash_token = classes - 1;
        let mut repeated_dash = vec![0.0_f32; classes * 3];
        repeated_dash[dash_token] = 1.0;
        repeated_dash[classes] = 1.0;
        repeated_dash[classes * 2 + dash_token] = 1.0;
        assert_eq!(dashes.decode(&repeated_dash, classes).unwrap(), "--");
    }

    #[test]
    fn numeric_sequence_decode_keeps_all_blank_and_ties_empty() {
        let dictionary = numeric_test_dictionary();
        let classes = dictionary.len();
        let decoder = NumericCtcDecoder::new(&dictionary, CtcCharacterSet::Digits).unwrap();
        let mut blank_wins = vec![0.0_f32; classes];
        blank_wins[0] = 0.9;
        blank_wins[1] = 0.1;
        assert_eq!(decoder.decode(&blank_wins, classes).unwrap(), "");

        let mut tie = vec![0.0_f32; classes];
        tie[0] = 0.5;
        tie[1] = 0.5;
        assert_eq!(decoder.decode(&tie, classes).unwrap(), "");
    }

    #[test]
    fn numeric_sequence_decode_preserves_zero_padding_and_field_widths() {
        let dictionary = numeric_test_dictionary();
        let classes = dictionary.len();
        let digits = NumericCtcDecoder::new(&dictionary, CtcCharacterSet::Digits).unwrap();
        let mut padded = vec![0.0_f32; classes * 4];
        for (timestep, token) in [1_usize, 8, 7, 5].into_iter().enumerate() {
            padded[timestep * classes + token] = 1.0;
        }
        assert_eq!(digits.decode(&padded, classes).unwrap(), "0764");

        let level = NumericCtcDecoder::new(&dictionary, CtcCharacterSet::DigitsUpToTwo).unwrap();
        let mut three_digits = vec![0.0_f32; classes * 3];
        for (timestep, token) in [2_usize, 3, 4].into_iter().enumerate() {
            three_digits[timestep * classes + token] = 1.0;
        }
        assert_eq!(level.decode(&three_digits, classes).unwrap(), "");

        let combo =
            NumericCtcDecoder::new(&dictionary, CtcCharacterSet::DigitsAndDashesUpToThree).unwrap();
        let mut four_digits = vec![0.0_f32; classes * 7];
        for (timestep, token) in [2_usize, 0, 2, 0, 2, 0, 3].into_iter().enumerate() {
            four_digits[timestep * classes + token] = 1.0;
        }
        assert_eq!(combo.decode(&four_digits, classes).unwrap(), "");
    }

    fn numeric_test_dictionary() -> Vec<String> {
        std::iter::once(String::new())
            .chain((0..=9).map(|digit| digit.to_string()))
            .chain(std::iter::once("-".to_owned()))
            .collect()
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
