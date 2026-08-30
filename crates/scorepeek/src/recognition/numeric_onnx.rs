use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::Instant;

use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{Tensor, TensorElementType, ValueType};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::numeric_specialist::{
    NUMERIC_BLANK_INDEX, NUMERIC_DICTIONARY, NumericCalibration, NumericField,
    NumericFieldInference, ScoreBreakdownDecision, rank_numeric_probabilities,
    select_score_breakdown,
};
use super::title_onnx::OnnxParityError;
use super::title_preprocessor::{
    NUMERIC_INPUT_HEIGHT, NUMERIC_INPUT_VALUES, NUMERIC_INPUT_WIDTH, preprocess_numeric_image,
};
use super::{DynamicTextObservation, ResultScreenRgb8Crops, Rgb8Crop};

pub const NUMERIC_PREPROCESSOR_ID: &str = "paddleocr-3.7.0-bgr-rec-resize-3x32x320-v1";
pub const NUMERIC_MODEL_MANIFEST_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../models/manifests/numeric-mobile-ctc-runtime-v1.json"
));
pub const NUMERIC_MODEL_MANIFEST_SHA256: &str =
    "7badce6d463a2d795e513b67979c9eceb53718adbcc7fa3b6afe4cbd12e1ba2a";
const MAX_NUMERIC_MODEL_BYTES: u64 = 32 * 1024 * 1024;
const NUMERIC_BATCH_I64: i64 = 14;
const NUMERIC_INPUT_HEIGHT_I64: i64 = 32;
const NUMERIC_INPUT_WIDTH_I64: i64 = 320;
const NUMERIC_OUTPUT_CLASSES_I64: i64 = 12;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NumericModelCalibrations {
    pub level: NumericCalibration,
    pub notes: NumericCalibration,
    pub score: NumericCalibration,
    pub judgment: NumericCalibration,
    pub supplemental: NumericCalibration,
    pub joint_minimum_runner_up_margin: f32,
}

impl NumericModelCalibrations {
    fn for_field(&self, field: NumericField) -> NumericCalibration {
        match field {
            NumericField::Level => self.level,
            NumericField::Notes => self.notes,
            NumericField::CurrentScore | NumericField::PreviousScore => self.score,
            NumericField::Pgreat
            | NumericField::Great
            | NumericField::Good
            | NumericField::Bad
            | NumericField::Poor => self.judgment,
            NumericField::PreviousMissCount
            | NumericField::MissCount
            | NumericField::Fast
            | NumericField::Slow
            | NumericField::ComboBreak => self.supplemental,
        }
    }

    fn is_valid(&self) -> bool {
        let calibrations = [
            self.level,
            self.notes,
            self.score,
            self.judgment,
            self.supplemental,
        ];
        calibrations.into_iter().all(|calibration| {
            calibration.temperature.is_finite()
                && calibration.temperature > 0.0
                && calibration.minimum_probability.is_finite()
                && (0.0..=1.0).contains(&calibration.minimum_probability)
                && calibration.minimum_runner_up_margin.is_finite()
                && calibration.minimum_runner_up_margin >= 0.0
        }) && self.joint_minimum_runner_up_margin.is_finite()
            && self.joint_minimum_runner_up_margin >= 0.0
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NumericModelContract {
    pub schema: String,
    pub model_id: String,
    pub model_filename: String,
    pub model_sha256: String,
    pub model_bytes: u64,
    pub candidate: String,
    pub dictionary: String,
    pub preprocessor_id: String,
    pub input_shape: [usize; 3],
    pub output_classes: usize,
    pub dataset_sha256: String,
    pub preparation_sha256: String,
    pub evaluation_manifest_sha256: String,
    pub final_training_manifest_sha256: String,
    pub initializer_manifest_sha256: String,
    pub initializer_checkpoint_sha256: String,
    pub training_source_commit: String,
    pub export_manifest_sha256: String,
    pub paddle_graph_sha256: String,
    pub paddle_parameters_sha256: String,
    pub license_id: String,
    pub calibrations: NumericModelCalibrations,
}

impl NumericModelContract {
    fn validate(&self) -> bool {
        self.schema == "scorepeek-private-numeric-model-runtime-v1"
            && !self.model_id.is_empty()
            && self.model_filename == "inference.onnx"
            && valid_sha256(&self.model_sha256)
            && (1..=MAX_NUMERIC_MODEL_BYTES).contains(&self.model_bytes)
            && self.candidate == "mobile"
            && self.dictionary == NUMERIC_DICTIONARY
            && self.preprocessor_id == NUMERIC_PREPROCESSOR_ID
            && self.input_shape == [3, NUMERIC_INPUT_HEIGHT, NUMERIC_INPUT_WIDTH]
            && self.output_classes == NUMERIC_BLANK_INDEX + 1
            && valid_sha256(&self.dataset_sha256)
            && valid_sha256(&self.preparation_sha256)
            && valid_sha256(&self.evaluation_manifest_sha256)
            && valid_sha256(&self.final_training_manifest_sha256)
            && valid_sha256(&self.initializer_manifest_sha256)
            && valid_sha256(&self.initializer_checkpoint_sha256)
            && self.training_source_commit.len() == 40
            && self
                .training_source_commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            && valid_sha256(&self.export_manifest_sha256)
            && valid_sha256(&self.paddle_graph_sha256)
            && valid_sha256(&self.paddle_parameters_sha256)
            && self.license_id == "Apache-2.0"
            && self.calibrations.is_valid()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NumericBatchInference {
    pub model_id: String,
    pub model_sha256: String,
    pub preprocessor_id: String,
    pub elapsed_ms: u64,
    pub output_timesteps: usize,
    pub input_tensor_sha256: String,
    pub output_tensor_sha256: String,
    pub fields: Vec<NumericFieldInference>,
    pub score_breakdown: Option<ScoreBreakdownDecision>,
}

impl NumericBatchInference {
    #[must_use]
    pub fn field(&self, field: NumericField) -> Option<&NumericFieldInference> {
        self.fields
            .iter()
            .find(|inference| inference.field == field)
    }

    #[must_use]
    pub fn accepted_text(&self, field: NumericField) -> Option<String> {
        if let Some(joint) = self
            .score_breakdown
            .as_ref()
            .and_then(|decision| decision.accepted.as_ref())
        {
            match field {
                NumericField::CurrentScore => return Some(joint.current_score.to_string()),
                NumericField::Pgreat => return Some(joint.pgreat.to_string()),
                NumericField::Great => return Some(joint.great.to_string()),
                _ => {}
            }
        }
        if matches!(
            field,
            NumericField::CurrentScore | NumericField::Pgreat | NumericField::Great
        ) {
            return None;
        }
        let inference = self.field(field)?;
        if !inference.accepted {
            return None;
        }
        Some(inference.candidates.first()?.text.clone())
    }

    #[must_use]
    pub fn text_observation(&self, field: NumericField) -> DynamicTextObservation {
        DynamicTextObservation {
            input_width: NUMERIC_INPUT_WIDTH,
            output_timesteps: self.output_timesteps,
            open_text: self
                .field(field)
                .map_or_else(String::new, |value| value.raw_text.clone()),
            constrained_text: self.accepted_text(field),
        }
    }
}

fn select_batch_score_breakdown(
    fields: &[NumericFieldInference],
    calibrations: &NumericModelCalibrations,
) -> Result<Option<ScoreBreakdownDecision>, OnnxParityError> {
    let field = |wanted| {
        fields
            .iter()
            .find(|inference| inference.field == wanted)
            .ok_or(OnnxParityError::InvalidArtifact)
    };
    let notes = accepted_value(field(NumericField::Notes)?, calibrations.notes);
    let score_fields = [
        field(NumericField::CurrentScore)?,
        field(NumericField::Pgreat)?,
        field(NumericField::Great)?,
    ];
    if score_fields.iter().any(|inference| {
        inference.candidates.first().is_none_or(|candidate| {
            candidate.log_probability <= inference.all_blank_log_probability
        })
    }) {
        return Ok(None);
    }
    if score_fields[1..]
        .iter()
        .any(|inference| !calibrations.judgment.accepts(inference))
    {
        return Ok(None);
    }
    Ok(Some(select_score_breakdown(
        notes,
        score_fields[0],
        score_fields[1],
        score_fields[2],
        calibrations.joint_minimum_runner_up_margin,
    )))
}

pub struct RegisteredNumericRuntime {
    session: Session,
    contract: NumericModelContract,
}

impl RegisteredNumericRuntime {
    /// Loads one manifest-bound private numeric model without download or fallback.
    ///
    /// # Errors
    ///
    /// Returns an error when the manifest, model bytes, or ONNX tensor contract is invalid.
    pub fn load(
        bundle: &Path,
        manifest_bytes: &[u8],
        expected_manifest_sha256: &str,
    ) -> Result<Self, OnnxParityError> {
        if !bundle.is_absolute()
            || encode_sha256(manifest_bytes) != expected_manifest_sha256
            || manifest_bytes.last() != Some(&b'\n')
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let contract: NumericModelContract = serde_json::from_slice(manifest_bytes)?;
        if !contract.validate() {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let model_path = bundle.join(&contract.model_filename);
        let metadata = model_path.metadata()?;
        if !metadata.is_file() || metadata.len() != contract.model_bytes {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let model_bytes = fs::read(&model_path)?;
        if encode_sha256(&model_bytes) != contract.model_sha256 {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let session = Session::builder()?
            .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .with_intra_threads(1)
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .with_inter_threads(1)
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .with_parallel_execution(false)
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .with_optimization_level(GraphOptimizationLevel::All)
            .map_err(|error| OnnxParityError::Ort(error.into()))?
            .commit_from_memory(&model_bytes)?;
        if session.inputs().len() != 1
            || session.outputs().len() != 1
            || !matches!(
                session.inputs()[0].dtype(),
                ValueType::Tensor { ty: TensorElementType::Float32, shape, .. }
                    if shape.as_ref() == [-1, 3, NUMERIC_INPUT_HEIGHT_I64, NUMERIC_INPUT_WIDTH_I64]
            )
            || !matches!(
                session.outputs()[0].dtype(),
                ValueType::Tensor { ty: TensorElementType::Float32, shape, .. }
                    if shape.len() == 3
                        && shape[0] == -1
                        && shape[1] > 0
                        && shape[2] == NUMERIC_OUTPUT_CLASSES_I64
            )
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        Ok(Self { session, contract })
    }

    /// Runs all fourteen fixed numeric fields in one ONNX batch.
    ///
    /// # Errors
    ///
    /// Returns an error when preprocessing, inference, or numeric decoding fails its contract.
    pub fn observe(
        &mut self,
        crops: &ResultScreenRgb8Crops,
    ) -> Result<NumericBatchInference, OnnxParityError> {
        let started = Instant::now();
        let registered = numeric_crops(crops);
        let mut input = Vec::with_capacity(NumericField::ALL.len() * NUMERIC_INPUT_VALUES);
        for (_, crop) in &registered {
            input.extend(preprocess_numeric_image(
                crop.pixels(),
                crop.roi.width as usize,
                crop.roi.height as usize,
            )?);
        }
        let input_tensor_sha256 = encode_f32_sha256(&input);
        let outputs = self.session.run(ort::inputs![Tensor::from_array((
            [
                NumericField::ALL.len(),
                3,
                NUMERIC_INPUT_HEIGHT,
                NUMERIC_INPUT_WIDTH,
            ],
            input,
        ))?])?;
        let (shape, probabilities) = outputs[0].try_extract_tensor::<f32>()?;
        let [batch, timesteps, classes] = shape.as_ref() else {
            return Err(OnnxParityError::InvalidArtifact);
        };
        let timesteps =
            usize::try_from(*timesteps).map_err(|_| OnnxParityError::InvalidArtifact)?;
        if *batch != NUMERIC_BATCH_I64
            || timesteps == 0
            || *classes != NUMERIC_OUTPUT_CLASSES_I64
            || probabilities.len()
                != NumericField::ALL.len() * timesteps * self.contract.output_classes
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let stride = timesteps * self.contract.output_classes;
        let output_tensor_sha256 = encode_f32_sha256(probabilities);
        let mut fields = Vec::with_capacity(NumericField::ALL.len());
        for (index, field) in NumericField::ALL.into_iter().enumerate() {
            fields.push(
                rank_numeric_probabilities(
                    field,
                    &probabilities[index * stride..(index + 1) * stride],
                    timesteps,
                    self.contract.calibrations.for_field(field),
                )
                .map_err(|_| OnnxParityError::InvalidArtifact)?,
            );
        }
        let score_breakdown = select_batch_score_breakdown(&fields, &self.contract.calibrations)?;
        Ok(NumericBatchInference {
            model_id: self.contract.model_id.clone(),
            model_sha256: self.contract.model_sha256.clone(),
            preprocessor_id: self.contract.preprocessor_id.clone(),
            elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            output_timesteps: timesteps,
            input_tensor_sha256,
            output_tensor_sha256,
            fields,
            score_breakdown,
        })
    }

    #[must_use]
    pub const fn contract(&self) -> &NumericModelContract {
        &self.contract
    }
}

fn accepted_value(
    inference: &NumericFieldInference,
    calibration: NumericCalibration,
) -> Option<u32> {
    calibration
        .accepts(inference)
        .then(|| inference.candidates.first()?.text.parse().ok())
        .flatten()
}

fn numeric_crops(crops: &ResultScreenRgb8Crops) -> [(NumericField, &Rgb8Crop); 14] {
    [
        (NumericField::Level, &crops.level),
        (NumericField::Notes, &crops.notes),
        (NumericField::CurrentScore, &crops.current_score),
        (NumericField::PreviousScore, &crops.previous_score),
        (NumericField::PreviousMissCount, &crops.previous_miss_count),
        (NumericField::MissCount, &crops.miss_count),
        (NumericField::Pgreat, &crops.pgreat),
        (NumericField::Great, &crops.great),
        (NumericField::Good, &crops.good),
        (NumericField::Bad, &crops.bad),
        (NumericField::Poor, &crops.poor),
        (NumericField::Fast, &crops.fast),
        (NumericField::Slow, &crops.slow),
        (NumericField::ComboBreak, &crops.combo_break),
    ]
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn encode_sha256(bytes: &[u8]) -> String {
    encode_digest(&Sha256::digest(bytes))
}

fn encode_f32_sha256(values: &[f32]) -> String {
    let mut hasher = Sha256::new();
    for value in values {
        hasher.update(value.to_le_bytes());
    }
    encode_digest(&hasher.finalize())
}

fn encode_digest(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut output, byte| {
            write!(output, "{byte:02x}").expect("writing to a String cannot fail");
            output
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognition::numeric_specialist::NumericCandidate;

    fn calibration(minimum_runner_up_margin: f32) -> NumericCalibration {
        NumericCalibration {
            enabled: true,
            temperature: 1.0,
            minimum_probability: 0.0,
            minimum_runner_up_margin,
        }
    }

    fn inference(field: NumericField, text: &str, margin: f32) -> NumericFieldInference {
        NumericFieldInference {
            field,
            calibration: calibration(0.0),
            accepted: true,
            raw_text: text.to_owned(),
            candidates: vec![NumericCandidate {
                text: text.to_owned(),
                log_probability: -0.1,
                calibrated_probability: 0.9,
            }],
            all_blank_log_probability: -10.0,
            runner_up_margin: Some(margin),
        }
    }

    #[test]
    fn score_joint_requires_judgment_calibration_for_pgreat_and_great() {
        let calibrations = NumericModelCalibrations {
            level: calibration(0.0),
            notes: NumericCalibration {
                enabled: false,
                ..calibration(0.0)
            },
            score: calibration(0.0),
            judgment: calibration(1.0),
            supplemental: calibration(0.0),
            joint_minimum_runner_up_margin: 0.0,
        };
        let mut fields = vec![
            inference(NumericField::Notes, "764", 2.0),
            inference(NumericField::CurrentScore, "1383", 2.0),
            inference(NumericField::Pgreat, "630", 0.2),
            inference(NumericField::Great, "123", 2.0),
        ];
        assert!(
            select_batch_score_breakdown(&fields, &calibrations)
                .unwrap()
                .is_none()
        );
        fields[2].runner_up_margin = Some(2.0);
        assert!(
            select_batch_score_breakdown(&fields, &calibrations)
                .unwrap()
                .and_then(|decision| decision.accepted)
                .is_some()
        );
    }

    #[test]
    fn text_observation_keeps_unrestricted_raw_separate_from_accepted_text() {
        let mut bad = inference(NumericField::Bad, "0", 2.0);
        bad.raw_text = "07-".to_owned();
        let batch = NumericBatchInference {
            model_id: "model".to_owned(),
            model_sha256: "0".repeat(64),
            preprocessor_id: NUMERIC_PREPROCESSOR_ID.to_owned(),
            elapsed_ms: 1,
            output_timesteps: 6,
            input_tensor_sha256: "1".repeat(64),
            output_tensor_sha256: "2".repeat(64),
            fields: vec![bad],
            score_breakdown: None,
        };
        let observation = batch.text_observation(NumericField::Bad);
        assert_eq!(observation.open_text, "07-");
        assert_eq!(observation.constrained_text.as_deref(), Some("0"));
    }
}
