use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::catalog::Difficulty;
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{Tensor, TensorElementType, ValueType};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::numeric_fixed_slot::{
    FIXED_SLOT_FEATURE_DIMENSIONS, FIXED_SLOT_PREPROCESSOR_ID, extract_fixed_slot_fields,
    fixed_not_displayed_fields, fixed_slot_feature,
};
use super::numeric_specialist::{
    FIXED_SLOT_CLASS_COUNT, FIXED_SLOT_CLASSES, NumericCalibration, NumericField,
    NumericFieldInference, ScoreBreakdownDecision, rank_fixed_slot_logits, select_score_breakdown,
};
use super::title_onnx::OnnxParityError;
use super::{
    CanonicalLayout, DynamicTextObservation, ResultNumericCharacterLayout, ResultScreenRgb8Crops,
};

pub const NUMERIC_PREPROCESSOR_ID: &str = FIXED_SLOT_PREPROCESSOR_ID;
pub const NUMERIC_MODEL_MANIFEST_BYTES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../models/manifests/numeric-fixed-slot-hog-mlp-runtime-v3.json"
));
pub const NUMERIC_MODEL_MANIFEST_SHA256: &str =
    "3427a05c5360880b6facca83e565e6426aaf38c917494b2ae90f982da1fdfd91";
const MAX_NUMERIC_MODEL_BYTES: u64 = 32 * 1024 * 1024;
const NUMERIC_FEATURE_DIMENSIONS_I64: i64 = 2_244;
const NUMERIC_OUTPUT_CLASSES_I64: i64 = 11;

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
    pub classes: String,
    pub preprocessor_id: String,
    pub feature_dimensions: usize,
    pub hidden_dimensions: usize,
    pub output_classes: usize,
    pub numeric_character_layout_sha256: String,
    pub canonical_layout_sha256: String,
    pub dataset_sha256: String,
    pub evaluation_manifest_sha256: String,
    pub final_training_sha256: String,
    pub license_id: String,
    pub calibrations: NumericModelCalibrations,
}

/// Historical CTC manifest shape retained for diagnostic and migration readers.
///
/// A legacy contract can be inspected, but it is never accepted by the active fixed-slot
/// runtime or its create-only installer.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyNumericModelContract {
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ReadableNumericModelContract {
    FixedSlot(NumericModelContract),
    LegacyCtc(LegacyNumericModelContract),
}

/// Parses the current fixed-slot manifest or the immutable historical CTC manifest.
///
/// # Errors
/// Returns a JSON error for any unknown manifest generation or malformed contract.
pub fn read_numeric_model_contract(
    bytes: &[u8],
) -> Result<ReadableNumericModelContract, serde_json::Error> {
    serde_json::from_slice(bytes)
}

impl NumericModelContract {
    fn validate(&self) -> bool {
        self.schema == "scorepeek-private-numeric-model-runtime-v2"
            && !self.model_id.is_empty()
            && self.model_filename == "inference.onnx"
            && valid_sha256(&self.model_sha256)
            && (1..=MAX_NUMERIC_MODEL_BYTES).contains(&self.model_bytes)
            && self.candidate == "shared_hog_mlp"
            && self.classes == FIXED_SLOT_CLASSES
            && self.preprocessor_id == NUMERIC_PREPROCESSOR_ID
            && self.feature_dimensions == FIXED_SLOT_FEATURE_DIMENSIONS
            && self.hidden_dimensions == 64
            && self.output_classes == FIXED_SLOT_CLASS_COUNT
            && self.numeric_character_layout_sha256 == ResultNumericCharacterLayout::sha256()
            && self.canonical_layout_sha256 == CanonicalLayout::sha256()
            && valid_sha256(&self.dataset_sha256)
            && valid_sha256(&self.evaluation_manifest_sha256)
            && valid_sha256(&self.final_training_sha256)
            && self.license_id == "LicenseRef-Scorepeek-Private-Trained-Weights"
            && self.calibrations.is_valid()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NumericCellCandidate {
    pub class: char,
    pub probability: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NumericCellInference {
    pub field: NumericField,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level_difficulty: Option<Difficulty>,
    pub slot: usize,
    pub candidates: Vec<NumericCellCandidate>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NumericBatchInference {
    pub model_id: String,
    pub model_sha256: String,
    pub preprocessor_id: String,
    pub elapsed_us: u64,
    pub input_cells: usize,
    pub input_tensor_sha256: String,
    pub output_tensor_sha256: String,
    pub cells: Vec<NumericCellInference>,
    pub fields: Vec<NumericFieldInference>,
    pub level_variants: Vec<NumericLevelVariantInference>,
    pub not_displayed_fields: Vec<NumericField>,
    pub score_breakdown: Option<ScoreBreakdownDecision>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NumericLevelVariantInference {
    pub difficulty: Difficulty,
    pub inference: NumericFieldInference,
}

impl NumericBatchInference {
    /// Selects the already-recognized level variant corresponding to the independent difficulty
    /// observation.
    ///
    /// # Errors
    /// Returns an error when the numeric batch lacks its required level field.
    pub fn join_level(&mut self, difficulty: Option<Difficulty>) -> Result<(), OnnxParityError> {
        let calibration = self
            .fields
            .iter()
            .find(|field| field.field == NumericField::Level)
            .map(|field| field.calibration)
            .ok_or(OnnxParityError::InvalidArtifact)?;
        let selected = difficulty.and_then(|difficulty| {
            self.level_variants
                .iter()
                .filter(|variant| variant.difficulty == difficulty)
                .map(|variant| &variant.inference)
                .max_by(|left, right| {
                    left.candidates
                        .first()
                        .map_or(f32::NEG_INFINITY, |candidate| {
                            candidate.calibrated_probability
                        })
                        .partial_cmp(
                            &right
                                .candidates
                                .first()
                                .map_or(f32::NEG_INFINITY, |candidate| {
                                    candidate.calibrated_probability
                                }),
                        )
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .cloned()
        });
        let level = selected.unwrap_or_else(|| unavailable_field(NumericField::Level, calibration));
        if let Some(current) = self
            .fields
            .iter_mut()
            .find(|field| field.field == NumericField::Level)
        {
            *current = level;
            Ok(())
        } else {
            Err(OnnxParityError::InvalidArtifact)
        }
    }

    #[must_use]
    pub fn field(&self, field: NumericField) -> Option<&NumericFieldInference> {
        self.fields
            .iter()
            .find(|inference| inference.field == field)
    }

    #[must_use]
    pub fn accepted_text(&self, field: NumericField) -> Option<String> {
        if self.not_displayed_fields.contains(&field) {
            return Some("--".to_owned());
        }
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
            input_width: 24,
            output_timesteps: field.maximum_digits(),
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
                    if shape.as_ref() == [-1, NUMERIC_FEATURE_DIMENSIONS_I64]
            )
            || !matches!(
                session.outputs()[0].dtype(),
                ValueType::Tensor { ty: TensorElementType::Float32, shape, .. }
                    if shape.as_ref() == [-1, NUMERIC_OUTPUT_CLASSES_I64]
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
        let not_displayed_fields = fixed_not_displayed_fields(crops);
        let registered = extract_fixed_slot_fields(crops)?;
        let cell_count = registered
            .iter()
            .map(|field| field.cells.len())
            .sum::<usize>();
        let mut input = Vec::with_capacity(cell_count * FIXED_SLOT_FEATURE_DIMENSIONS);
        for field in &registered {
            for cell in &field.cells {
                input.extend(fixed_slot_feature(
                    &cell.pixels,
                    cell.width,
                    cell.height,
                    field.field,
                )?);
            }
        }
        if input.is_empty() {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let input_tensor_sha256 = encode_f32_sha256(&input);
        let outputs = self.session.run(ort::inputs![Tensor::from_array((
            [cell_count, FIXED_SLOT_FEATURE_DIMENSIONS],
            input,
        ))?])?;
        let (shape, logits) = outputs[0].try_extract_tensor::<f32>()?;
        let [batch, classes] = shape.as_ref() else {
            return Err(OnnxParityError::InvalidArtifact);
        };
        if usize::try_from(*batch).ok() != Some(cell_count)
            || *classes != NUMERIC_OUTPUT_CLASSES_I64
            || logits.len() != cell_count * self.contract.output_classes
        {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let output_tensor_sha256 = encode_f32_sha256(logits);
        let mut offset = 0;
        let mut cells = Vec::with_capacity(cell_count);
        let mut decoded = Vec::with_capacity(registered.len());
        for field in registered {
            let length = field.cells.len() * self.contract.output_classes;
            for (slot, row) in logits[offset..offset + length]
                .chunks_exact(self.contract.output_classes)
                .enumerate()
            {
                cells.push(cell_inference(
                    field.field,
                    field.level_difficulty,
                    slot,
                    row,
                ));
            }
            decoded.push((
                field.level_difficulty,
                rank_fixed_slot_logits(
                    field.field,
                    &logits[offset..offset + length],
                    field.cells.len(),
                    self.contract.calibrations.for_field(field.field),
                )
                .map_err(|_| OnnxParityError::InvalidArtifact)?,
            ));
            offset += length;
        }
        let (fields, level_variants) = select_numeric_fields(&decoded, &self.contract.calibrations);
        let score_breakdown = select_batch_score_breakdown(&fields, &self.contract.calibrations)?;
        Ok(NumericBatchInference {
            model_id: self.contract.model_id.clone(),
            model_sha256: self.contract.model_sha256.clone(),
            preprocessor_id: self.contract.preprocessor_id.clone(),
            elapsed_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            input_cells: cell_count,
            input_tensor_sha256,
            output_tensor_sha256,
            cells,
            fields,
            level_variants,
            not_displayed_fields,
            score_breakdown,
        })
    }

    #[must_use]
    pub const fn contract(&self) -> &NumericModelContract {
        &self.contract
    }

    /// Runs the SELECT-specific measured cells through the registered numeric model.
    /// # Errors
    /// Rejects invalid crops, nonfinite output, or inference failure.
    pub fn observe_music_select_best(
        &mut self,
        crops: &super::MusicSelectBestCrops,
    ) -> Result<super::BestNumericObservation, OnnxParityError> {
        let layout = super::MusicSelectBestLayout::load()?;
        let cells = crops.numeric_cells()?;
        let mut input = Vec::with_capacity(8 * FIXED_SLOT_FEATURE_DIMENSIONS);
        for cell in &cells {
            input.extend(fixed_slot_feature(
                cell.pixels(),
                cell.roi.width as usize,
                cell.roi.height as usize,
                NumericField::PreviousScore,
            )?);
        }
        let outputs = self.session.run(ort::inputs![Tensor::from_array((
            [8, FIXED_SLOT_FEATURE_DIMENSIONS],
            input
        ))?])?;
        let (shape, logits) = outputs[0].try_extract_tensor::<f32>()?;
        if shape.as_ref() != [8, 11] || logits.iter().any(|v| !v.is_finite()) {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let mut observation = super::BestNumericObservation::default();
        let mut values = Vec::new();
        for field in logits.chunks_exact(44) {
            let mut text = String::new();
            let mut minimum_margin = f32::INFINITY;
            for row in field.chunks_exact(11) {
                let mut ranked: Vec<_> = row.iter().copied().enumerate().collect();
                ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
                text.push(char::from(b"_0123456789"[ranked[0].0]));
                minimum_margin = minimum_margin.min(ranked[0].1 - ranked[1].1);
            }
            let margin = format!("{:.0}", minimum_margin * 1000.0)
                .parse::<i32>()
                .map_err(|_| OnnxParityError::InvalidArtifact)?;
            // Dim leading zero placeholders can alternate between blank and zero, but blanks
            // after a significant digit cannot be interpreted as digits.
            let significant = text.trim_start_matches(['_', '0']);
            let value = if margin >= layout.minimum_logit_margin_milli && !significant.contains('_')
            {
                if significant.is_empty() {
                    text.ends_with('0').then_some(0)
                } else {
                    significant.parse().ok()
                }
            } else {
                None
            };
            values.push(value.map_or(super::BestValue::Unknown, super::BestValue::Known));
            observation.cell_classes.push(text);
            observation.minimum_margins_milli.push(margin);
        }
        observation.score = values.remove(0);
        observation.miss_count = if crops.miss_dashes() {
            super::BestValue::NoRecord
        } else {
            values.remove(0)
        };
        Ok(observation)
    }
}

fn select_numeric_fields(
    decoded: &[(Option<Difficulty>, NumericFieldInference)],
    calibrations: &NumericModelCalibrations,
) -> (
    Vec<NumericFieldInference>,
    Vec<NumericLevelVariantInference>,
) {
    let level_variants = decoded
        .iter()
        .filter_map(|(difficulty, inference)| {
            difficulty.map(|difficulty| NumericLevelVariantInference {
                difficulty,
                inference: inference.clone(),
            })
        })
        .collect::<Vec<_>>();
    let fields = NumericField::ALL
        .into_iter()
        .map(|field| {
            decoded
                .iter()
                .filter(|(difficulty, inference)| difficulty.is_none() && inference.field == field)
                .map(|(_, inference)| inference)
                .max_by(|left, right| {
                    left.candidates
                        .first()
                        .map_or(f32::NEG_INFINITY, |candidate| {
                            candidate.calibrated_probability
                        })
                        .partial_cmp(
                            &right
                                .candidates
                                .first()
                                .map_or(f32::NEG_INFINITY, |candidate| {
                                    candidate.calibrated_probability
                                }),
                        )
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .cloned()
                .unwrap_or_else(|| unavailable_field(field, calibrations.for_field(field)))
        })
        .collect();
    (fields, level_variants)
}

fn cell_inference(
    field: NumericField,
    level_difficulty: Option<Difficulty>,
    slot: usize,
    logits: &[f32],
) -> NumericCellInference {
    let maximum = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let normalizer = logits
        .iter()
        .map(|value| (*value - maximum).exp())
        .sum::<f32>();
    let mut candidates = logits
        .iter()
        .copied()
        .enumerate()
        .map(|(index, value)| NumericCellCandidate {
            class: char::from(FIXED_SLOT_CLASSES.as_bytes()[index]),
            probability: (value - maximum).exp() / normalizer,
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .probability
            .partial_cmp(&left.probability)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.class.cmp(&right.class))
    });
    candidates.truncate(3);
    NumericCellInference {
        field,
        level_difficulty,
        slot,
        candidates,
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

fn unavailable_field(
    field: NumericField,
    calibration: NumericCalibration,
) -> NumericFieldInference {
    NumericFieldInference {
        field,
        calibration,
        accepted: false,
        raw_text: String::new(),
        candidates: Vec::new(),
        all_blank_log_probability: 0.0,
        runner_up_margin: None,
    }
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
            elapsed_us: 1,
            input_cells: 6,
            input_tensor_sha256: "1".repeat(64),
            output_tensor_sha256: "2".repeat(64),
            cells: Vec::new(),
            fields: vec![bad],
            level_variants: Vec::new(),
            not_displayed_fields: Vec::new(),
            score_breakdown: None,
        };
        let observation = batch.text_observation(NumericField::Bad);
        assert_eq!(observation.open_text, "07-");
        assert_eq!(observation.constrained_text.as_deref(), Some("0"));
    }

    #[test]
    fn level_recognition_is_joined_with_difficulty_after_inference() {
        let calibration = NumericCalibration {
            enabled: true,
            temperature: 1.0,
            minimum_probability: 0.0,
            minimum_runner_up_margin: 0.0,
        };
        let mut batch = NumericBatchInference {
            model_id: "model".to_owned(),
            model_sha256: "0".repeat(64),
            preprocessor_id: NUMERIC_PREPROCESSOR_ID.to_owned(),
            elapsed_us: 1,
            input_cells: 4,
            input_tensor_sha256: "1".repeat(64),
            output_tensor_sha256: "2".repeat(64),
            cells: Vec::new(),
            fields: vec![unavailable_field(NumericField::Level, calibration)],
            level_variants: vec![
                NumericLevelVariantInference {
                    difficulty: Difficulty::Hyper,
                    inference: inference(NumericField::Level, "11", 2.0),
                },
                NumericLevelVariantInference {
                    difficulty: Difficulty::Another,
                    inference: inference(NumericField::Level, "12", 3.0),
                },
            ],
            not_displayed_fields: Vec::new(),
            score_breakdown: None,
        };

        batch.join_level(Some(Difficulty::Another)).unwrap();
        assert_eq!(
            batch.accepted_text(NumericField::Level).as_deref(),
            Some("12")
        );

        batch.join_level(None).unwrap();
        assert_eq!(batch.accepted_text(NumericField::Level), None);
    }

    #[test]
    fn current_and_legacy_numeric_manifests_remain_readable() {
        assert!(matches!(
            read_numeric_model_contract(NUMERIC_MODEL_MANIFEST_BYTES).unwrap(),
            ReadableNumericModelContract::FixedSlot(_)
        ));
        assert!(matches!(
            read_numeric_model_contract(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../models/manifests/numeric-mobile-ctc-runtime-v1.json"
            )))
            .unwrap(),
            ReadableNumericModelContract::LegacyCtc(_)
        ));
    }

    #[test]
    fn runtime_rejects_manifest_bound_to_another_layout() {
        let mut contract: NumericModelContract =
            serde_json::from_slice(NUMERIC_MODEL_MANIFEST_BYTES).unwrap();
        contract.numeric_character_layout_sha256 = "0".repeat(64);
        let mut bytes = serde_json::to_vec(&contract).unwrap();
        bytes.push(b'\n');
        let digest = encode_sha256(&bytes);
        let bundle = tempfile::tempdir().unwrap();
        assert!(matches!(
            RegisteredNumericRuntime::load(bundle.path(), &bytes, &digest),
            Err(OnnxParityError::InvalidArtifact)
        ));

        contract = serde_json::from_slice(NUMERIC_MODEL_MANIFEST_BYTES).unwrap();
        contract.canonical_layout_sha256 = "0".repeat(64);
        bytes = serde_json::to_vec(&contract).unwrap();
        bytes.push(b'\n');
        let digest = encode_sha256(&bytes);
        assert!(matches!(
            RegisteredNumericRuntime::load(bundle.path(), &bytes, &digest),
            Err(OnnxParityError::InvalidArtifact)
        ));
    }

    #[test]
    fn cell_evidence_retains_ranked_blank_and_digit_classes() {
        let mut logits = vec![0.0; FIXED_SLOT_CLASS_COUNT];
        logits[0] = 3.0;
        logits[8] = 2.0;
        logits[2] = 1.0;
        let cell = cell_inference(NumericField::Pgreat, None, 2, &logits);
        assert_eq!(cell.field, NumericField::Pgreat);
        assert_eq!(cell.level_difficulty, None);
        assert_eq!(cell.slot, 2);
        assert_eq!(cell.candidates.len(), 3);
        assert_eq!(cell.candidates[0].class, '_');
        assert_eq!(cell.candidates[1].class, '7');
        assert_eq!(cell.candidates[2].class, '1');

        let level = cell_inference(NumericField::Level, Some(Difficulty::Another), 0, &logits);
        assert_eq!(level.level_difficulty, Some(Difficulty::Another));
    }
}
