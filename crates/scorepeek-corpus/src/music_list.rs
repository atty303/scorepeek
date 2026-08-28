use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    CorpusError, canonical_json, digest_bytes, is_sha256, validate_opaque_id, validate_sha256,
    validate_token,
};

const OBSERVATION_SCHEMA: &str = "scorepeek-private-music-list-row-observation-draft-v2";
const MOTION_REQUEST_SCHEMA: &str = "scorepeek-private-music-list-motion-request-v1";
const MOTION_ARTIFACT_SCHEMA: &str = "scorepeek-private-music-list-motion-artifact-v1";
const MOTION_REVIEW_PLAN_SCHEMA: &str = "scorepeek-private-music-list-motion-review-plan-v1";
const MOTION_REVIEW_DECISIONS_SCHEMA: &str =
    "scorepeek-private-music-list-motion-review-decisions-v1";
const MAX_DOCUMENT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_OBSERVATIONS: usize = 250_000;
const MUSIC_LIST_SLOTS: u8 = 20;
const MUSIC_LIST_ROW_RGB_VALUES: u64 = 475 * 45 * 3;
const CANONICAL_FRAME_RGB_VALUES: usize = 1_920 * 1_080 * 3;
const CANONICAL_LAYOUT_BYTES: &[u8] =
    include_bytes!("../../scorepeek/src/canonical-layout-v1.json");
const CALIBRATED_CAPTURE_PROFILE_SHA256: &str =
    "d5809dc9b2acc19837260053f4df59a454c9178ae2ac6a0602982effc9da4704";
const CALIBRATED_FFMPEG_SHA256: &str =
    "9eac5b2b5076db5ff853a6fa0dcd6b8de7d0cac8481eadda6c47cd935825f1ee";
const NORMALIZER_FILTER: &str = "scale=1920:1080:flags=bitexact:in_color_matrix=bt709:out_color_matrix=bt709:in_range=tv:out_range=pc,format=rgb24";

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListRowObservationDocument {
    schema: String,
    catalog_sha256: String,
    source_manifest_sha256: String,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    canonical_layout_sha256: String,
    observations: Vec<MusicListRowObservation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListRowObservation {
    observation_id: String,
    slot: u8,
    frame: MusicListRowFrame,
    annotation: MusicListRowAnnotation,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListRowFrame {
    frame_extraction_directory: PathBuf,
    frame_extraction_sha256: String,
    crop_directory: PathBuf,
    crop_manifest_sha256: String,
    frame_id: String,
    source_pts: i64,
    decode_index: u64,
    crop_file_sha256: String,
    crop_pixel_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum MusicListRowAnnotation {
    Stationary {
        adjacent_frame: MusicListRowFrame,
        reported_rgb_l1_sum: u64,
        reported_compared_rgb_values: u64,
        presentation: TitlePresentation,
    },
    Scrolling {
        adjacent_frame: MusicListRowFrame,
        reported_rgb_l1_sum: u64,
        reported_compared_rgb_values: u64,
        presentation: TitlePresentation,
    },
    Selected,
    Clipped {
        edge: ClippedEdge,
    },
    NonTitle {
        kind: NonTitleKind,
    },
    Unknown {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ClippedEdge {
    Left,
    Right,
    Both,
    Obscured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum NonTitleKind {
    Empty,
    Separator,
    UnlockCondition,
    Overlay,
    OtherUi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TitlePresentation {
    availability: TitleAvailability,
    color_domain: TitleColorDomain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TitleAvailability {
    Available,
    LockedDimmed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum TitleColorDomain {
    Standard,
    InfinitasBlue,
    LeggendariaPurple,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicListRowObservationSummary {
    pub schema: &'static str,
    pub evidence_verified: bool,
    pub catalog_sha256: String,
    pub observation_count: usize,
    pub stationary_count: usize,
    pub scrolling_count: usize,
    pub selected_count: usize,
    pub clipped_count: usize,
    pub non_title_count: usize,
    pub unknown_count: usize,
    pub locked_dimmed_count: usize,
    pub infinitas_blue_count: usize,
    pub leggendaria_purple_count: usize,
    pub unlock_condition_count: usize,
    pub stationary_rgb_l1_min: Option<u64>,
    pub stationary_rgb_l1_max: Option<u64>,
    pub scrolling_rgb_l1_min: Option<u64>,
    pub scrolling_rgb_l1_max: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListMotionRequest {
    schema: String,
    catalog_sha256: String,
    source_manifest_sha256: String,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    canonical_layout_sha256: String,
    pairs: Vec<MusicListMotionPairRequest>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListMotionPairRequest {
    pair_id: String,
    first_frame: MusicListPairFrame,
    second_frame: MusicListPairFrame,
    motion: PairMotion,
    first_rows: [PairRowAnnotation; 20],
    second_rows: [PairRowAnnotation; 20],
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListPairFrame {
    frame_extraction_directory: PathBuf,
    frame_extraction_sha256: String,
    crop_directory: PathBuf,
    crop_manifest_sha256: String,
    frame_id: String,
    source_pts: i64,
    decode_index: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum PairMotion {
    Stationary,
    Scrolling,
    Unknown { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(tag = "content", rename_all = "snake_case", deny_unknown_fields)]
enum PairRowAnnotation {
    Title { presentation: TitlePresentation },
    Selected,
    Clipped { edge: ClippedEdge },
    NonTitle { kind: NonTitleKind },
    Unknown { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListMotionArtifact {
    schema: String,
    catalog_sha256: String,
    source_manifest_sha256: String,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    canonical_layout_sha256: String,
    pairs: Vec<MusicListMotionPair>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListMotionPair {
    pair_id: String,
    first_frame: MusicListPairFrame,
    second_frame: MusicListPairFrame,
    motion: PairMotion,
    first_rows: [PairRowAnnotation; 20],
    second_rows: [PairRowAnnotation; 20],
    row_rgb_l1_sums: [u64; 20],
    aggregate_rgb_l1_sum: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicListMotionSummary {
    pub schema: &'static str,
    pub evidence_verified: bool,
    pub pair_count: usize,
    pub stationary_count: usize,
    pub scrolling_count: usize,
    pub unknown_count: usize,
    pub aggregate_rgb_l1_min: Option<u64>,
    pub aggregate_rgb_l1_max: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicListMotionReviewPlanSummary {
    pub schema: &'static str,
    pub source_artifact_bound: bool,
    pub pair_count: usize,
    pub observation_count: usize,
    pub unique_crop_count: usize,
    pub duplicate_group_count: usize,
    pub exact_duplicate_savings_count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicListMotionReviewApplySummary {
    pub schema: &'static str,
    pub source_artifact_bound: bool,
    pub decision_count: usize,
    pub applied_occurrence_count: usize,
    pub remaining_unknown_occurrence_count: usize,
}

#[derive(Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListMotionReviewPlan {
    schema: String,
    source_artifact_sha256: String,
    catalog_sha256: String,
    source_manifest_sha256: String,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    canonical_layout_sha256: String,
    groups: Vec<MusicListMotionReviewGroup>,
}

#[derive(Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListMotionReviewGroup {
    crop_pixel_sha256: String,
    occurrences: Vec<MusicListMotionReviewOccurrence>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum MusicListMotionFrameRole {
    First,
    Second,
}

#[derive(Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListMotionReviewOccurrence {
    pair_id: String,
    frame_role: MusicListMotionFrameRole,
    frame_id: String,
    source_pts: i64,
    decode_index: u64,
    slot: u8,
    pair_motion: PairMotion,
    current_annotation: PairRowAnnotation,
    crop_path: PathBuf,
    crop_file_sha256: String,
}

#[derive(Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListMotionReviewDecisions {
    schema: String,
    source_review_plan_sha256: String,
    decisions: Vec<MusicListMotionReviewDecision>,
}

#[derive(Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicListMotionReviewDecision {
    crop_pixel_sha256: String,
    annotation: PairRowAnnotation,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalExtractionManifest {
    schema: String,
    fixture_id: String,
    source_manifest_sha256: String,
    media_probe_sha256: String,
    capture_profile_id: String,
    normalizer_artifact_sha256: String,
    canonical_frame_contract_id: String,
    extractor: ExtractionIdentity,
    source_time_base: TimeBase,
    video_stream_index: u32,
    frames: Vec<CanonicalExtractionFrame>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CanonicalExtractionFrame {
    frame_id: String,
    source_pts: i64,
    decode_index: u64,
    filename: String,
    frame_sha256: String,
    file_sha256: String,
    bytes: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExtractionIdentity {
    tool_id: String,
    tool_version: String,
    extractor_manifest_sha256: String,
    parameters_sha256: String,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct TimeBase {
    numerator: u32,
    denominator: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DomainNormalizerManifest {
    schema: String,
    capture_profile_id: String,
    observed: ObservedMediaContract,
    canonical_frame_contract_id: String,
    implementation: String,
    ffmpeg_sha256: String,
    filter: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ObservedMediaContract {
    input_format: String,
    codec_name: String,
    pixel_format: String,
    width: u32,
    height: u32,
    source_time_base: TimeBase,
    color_range: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicSelectCropManifest {
    schema: String,
    frame_id: String,
    frame_extraction_sha256: String,
    canonical_frame_sha256: String,
    normalizer_artifact_sha256: String,
    canonical_layout_sha256: String,
    crops: Vec<MusicSelectCrop>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MusicSelectCrop {
    field: String,
    filename: String,
    roi: CropRoi,
    pixel_sha256: String,
    file_sha256: String,
    bytes: u64,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CropRoi {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Inspects the shape of a private music-list row observation draft.
///
/// This boundary validates only canonical JSON, identifiers, state shape, and reported measurement
/// ranges. It does not read the referenced canonical extraction or crop artifacts and therefore
/// never promotes the draft into verified calibration or label evidence.
///
/// # Errors
/// Returns an error for a non-canonical document, an invalid binding, duplicate observation IDs,
/// or temporal evidence that does not compare adjacent decoded frames.
pub fn inspect_music_list_row_observation_draft(
    path: impl AsRef<Path>,
) -> Result<MusicListRowObservationSummary, CorpusError> {
    let path = path.as_ref();
    if !path.is_absolute() {
        return Err(invalid("observation document path must be absolute"));
    }
    let bytes = read_bounded_regular(path)?;
    let document: MusicListRowObservationDocument = serde_json::from_slice(&bytes)?;
    let mut canonical = serde_json::to_vec(&document)?;
    canonical.push(b'\n');
    if canonical != bytes {
        return Err(invalid("observation document must be canonical JSON"));
    }
    document.validate()
}

/// Verifies every referenced canonical extraction and crop artifact and recomputes temporal L1.
///
/// # Errors
/// Returns an error when any artifact binding, byte hash, crop geometry, or reported measurement
/// differs from the scorepeek-produced canonical artifacts.
pub fn verify_music_list_row_observation_draft(
    path: impl AsRef<Path>,
) -> Result<MusicListRowObservationSummary, CorpusError> {
    let path = path.as_ref();
    if !path.is_absolute() {
        return Err(invalid("observation document path must be absolute"));
    }
    let bytes = read_bounded_regular(path)?;
    let document: MusicListRowObservationDocument = serde_json::from_slice(&bytes)?;
    let mut canonical = serde_json::to_vec(&document)?;
    canonical.push(b'\n');
    if canonical != bytes {
        return Err(invalid("observation document must be canonical JSON"));
    }
    let mut summary = document.clone().validate()?;
    let mut stationary = Vec::new();
    let mut scrolling = Vec::new();
    for observation in &document.observations {
        let primary = verify_frame_artifacts(&document, observation.slot, &observation.frame)?;
        match &observation.annotation {
            MusicListRowAnnotation::Stationary {
                adjacent_frame,
                reported_rgb_l1_sum,
                ..
            } => {
                let adjacent = verify_frame_artifacts(&document, observation.slot, adjacent_frame)?;
                let computed = rgb_l1_sum(&primary, &adjacent)?;
                if computed != *reported_rgb_l1_sum {
                    return Err(invalid(
                        "reported stationary RGB L1 does not match crop bytes",
                    ));
                }
                stationary.push(computed);
            }
            MusicListRowAnnotation::Scrolling {
                adjacent_frame,
                reported_rgb_l1_sum,
                ..
            } => {
                let adjacent = verify_frame_artifacts(&document, observation.slot, adjacent_frame)?;
                let computed = rgb_l1_sum(&primary, &adjacent)?;
                if computed != *reported_rgb_l1_sum {
                    return Err(invalid(
                        "reported scrolling RGB L1 does not match crop bytes",
                    ));
                }
                scrolling.push(computed);
            }
            MusicListRowAnnotation::Selected
            | MusicListRowAnnotation::Clipped { .. }
            | MusicListRowAnnotation::NonTitle { .. }
            | MusicListRowAnnotation::Unknown { .. } => {}
        }
    }
    summary.evidence_verified = true;
    (summary.stationary_rgb_l1_min, summary.stationary_rgb_l1_max) = min_max(&stationary);
    (summary.scrolling_rgb_l1_min, summary.scrolling_rgb_l1_max) = min_max(&scrolling);
    Ok(summary)
}

/// Measures every row of each annotated adjacent-frame pair and writes a canonical artifact.
///
/// The request supplies only human semantic annotations and artifact identities. Motion labels are
/// never inferred from the measured values. The output is created without replacing an existing
/// file.
///
/// # Errors
/// Returns an error for a non-canonical request, an invalid or incomplete artifact binding, a
/// non-adjacent pair, or an existing/unsafe output path.
pub fn measure_music_list_motion(
    request_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
) -> Result<MusicListMotionSummary, CorpusError> {
    let request_path = request_path.as_ref();
    let output_path = output_path.as_ref();
    if !request_path.is_absolute() || !output_path.is_absolute() {
        return Err(invalid("motion request and output paths must be absolute"));
    }
    let request_bytes = read_bounded_regular(request_path)?;
    let request: MusicListMotionRequest = serde_json::from_slice(&request_bytes)?;
    if canonical_json(&request)? != request_bytes {
        return Err(invalid("motion request must be canonical JSON"));
    }
    validate_motion_request(&request)?;
    let domain = motion_domain(
        &request.catalog_sha256,
        &request.source_manifest_sha256,
        &request.capture_profile_id,
        &request.normalizer_artifact_sha256,
        &request.canonical_layout_sha256,
    );
    let pairs = request
        .pairs
        .into_iter()
        .map(|pair| measure_pair(&domain, pair))
        .collect::<Result<Vec<_>, _>>()?;
    let artifact = MusicListMotionArtifact {
        schema: MOTION_ARTIFACT_SCHEMA.to_owned(),
        catalog_sha256: request.catalog_sha256,
        source_manifest_sha256: request.source_manifest_sha256,
        capture_profile_id: request.capture_profile_id,
        normalizer_artifact_sha256: request.normalizer_artifact_sha256,
        canonical_layout_sha256: request.canonical_layout_sha256,
        pairs,
    };
    let summary = summarize_motion(&artifact, true);
    write_new_canonical(output_path, &artifact)?;
    Ok(summary)
}

/// Rehashes every canonical frame and all 21 crops behind a complete-pair motion artifact.
///
/// # Errors
/// Returns an error when the document is non-canonical, an identity or semantic annotation is
/// invalid, any pair is non-adjacent, or any reported row/aggregate measurement differs.
pub fn verify_music_list_motion(
    path: impl AsRef<Path>,
) -> Result<MusicListMotionSummary, CorpusError> {
    let path = path.as_ref();
    if !path.is_absolute() {
        return Err(invalid("motion artifact path must be absolute"));
    }
    let bytes = read_bounded_regular(path)?;
    let artifact: MusicListMotionArtifact = serde_json::from_slice(&bytes)?;
    if canonical_json(&artifact)? != bytes || artifact.schema != MOTION_ARTIFACT_SCHEMA {
        return Err(invalid("motion artifact must be canonical and versioned"));
    }
    validate_motion_metadata(
        &artifact.catalog_sha256,
        &artifact.source_manifest_sha256,
        &artifact.capture_profile_id,
        &artifact.normalizer_artifact_sha256,
        &artifact.canonical_layout_sha256,
        artifact.pairs.len(),
    )?;
    let domain = motion_domain(
        &artifact.catalog_sha256,
        &artifact.source_manifest_sha256,
        &artifact.capture_profile_id,
        &artifact.normalizer_artifact_sha256,
        &artifact.canonical_layout_sha256,
    );
    let mut ids = BTreeSet::new();
    let mut frame_pairs = BTreeSet::new();
    for pair in &artifact.pairs {
        validate_pair_identity(pair, &mut ids, &mut frame_pairs)?;
        validate_row_annotations(&pair.first_rows)?;
        validate_row_annotations(&pair.second_rows)?;
        verify_motion_pair_artifacts(&domain, pair)?;
    }
    Ok(summarize_motion(&artifact, true))
}

/// Builds a create-only review plan that groups digest-declared identical row crops.
///
/// Every one of the forty row occurrences per pair remains explicit. Grouping uses only the
/// scorepeek-written crop-manifest SHA-256 and never rereads crop pixels, remeasures motion, or
/// derives a semantic annotation.
///
/// # Errors
/// Returns an error when either path is not absolute, a selected manifest binding is invalid, the
/// row-observation bound is exceeded, or the output already exists.
pub fn plan_music_list_motion_review(
    artifact_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
) -> Result<MusicListMotionReviewPlanSummary, CorpusError> {
    let artifact_path = artifact_path.as_ref();
    let output_path = output_path.as_ref();
    if !artifact_path.is_absolute() || !output_path.is_absolute() {
        return Err(invalid(
            "motion artifact and review-plan paths must be absolute",
        ));
    }
    let bytes = read_bounded_regular(artifact_path)?;
    let artifact = read_motion_artifact(&bytes)?;
    let observation_count = artifact
        .pairs
        .len()
        .checked_mul(usize::from(MUSIC_LIST_SLOTS) * 2)
        .filter(|count| *count <= MAX_OBSERVATIONS)
        .ok_or(CorpusError::CapacityExceeded)?;
    let domain = motion_domain(
        &artifact.catalog_sha256,
        &artifact.source_manifest_sha256,
        &artifact.capture_profile_id,
        &artifact.normalizer_artifact_sha256,
        &artifact.canonical_layout_sha256,
    );
    let mut ids = BTreeSet::new();
    let mut frame_pairs = BTreeSet::new();
    let mut groups = BTreeMap::<String, Vec<MusicListMotionReviewOccurrence>>::new();
    for pair in &artifact.pairs {
        validate_pair_identity(pair, &mut ids, &mut frame_pairs)?;
        validate_row_annotations(&pair.first_rows)?;
        validate_row_annotations(&pair.second_rows)?;
        let first = read_review_crop_manifest(&domain, &pair.first_frame)?;
        let second = read_review_crop_manifest(&domain, &pair.second_frame)?;
        add_review_manifest(
            pair,
            MusicListMotionFrameRole::First,
            &pair.first_frame,
            &pair.first_rows,
            &first,
            &mut groups,
        )?;
        add_review_manifest(
            pair,
            MusicListMotionFrameRole::Second,
            &pair.second_frame,
            &pair.second_rows,
            &second,
            &mut groups,
        )?;
    }
    let duplicate_group_count = groups
        .values()
        .filter(|occurrences| occurrences.len() > 1)
        .count();
    let unique_crop_count = groups.len();
    let summary = MusicListMotionReviewPlanSummary {
        schema: "scorepeek-music-list-motion-review-plan-summary-v1",
        source_artifact_bound: true,
        pair_count: artifact.pairs.len(),
        observation_count,
        unique_crop_count,
        duplicate_group_count,
        exact_duplicate_savings_count: observation_count - unique_crop_count,
    };
    let plan = MusicListMotionReviewPlan {
        schema: MOTION_REVIEW_PLAN_SCHEMA.to_owned(),
        source_artifact_sha256: digest_bytes(&bytes),
        catalog_sha256: artifact.catalog_sha256,
        source_manifest_sha256: artifact.source_manifest_sha256,
        capture_profile_id: artifact.capture_profile_id,
        normalizer_artifact_sha256: artifact.normalizer_artifact_sha256,
        canonical_layout_sha256: artifact.canonical_layout_sha256,
        groups: groups
            .into_iter()
            .map(
                |(crop_pixel_sha256, occurrences)| MusicListMotionReviewGroup {
                    crop_pixel_sha256,
                    occurrences,
                },
            )
            .collect(),
    };
    write_new_canonical(output_path, &plan)?;
    Ok(summary)
}

/// Applies explicit, partial human review decisions to a digest-bound motion artifact.
///
/// The selected plan must name the exact source artifact and every occurrence it changes. Crop
/// bytes and prior measurements are not re-read. Omitted groups remain unchanged, so callers can
/// quarantine ambiguity as `unknown` and accumulate reviewed batches without guessing.
///
/// # Errors
/// Returns an error when an input is non-canonical, its digest binding is stale, a decision is
/// duplicated or does not name an exact crop group, a required binding fails, or the create-only
/// output already exists.
pub fn apply_music_list_motion_review(
    artifact_path: impl AsRef<Path>,
    plan_path: impl AsRef<Path>,
    decisions_path: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
) -> Result<MusicListMotionReviewApplySummary, CorpusError> {
    let artifact_path = artifact_path.as_ref();
    let plan_path = plan_path.as_ref();
    let decisions_path = decisions_path.as_ref();
    let output_path = output_path.as_ref();
    if !artifact_path.is_absolute()
        || !plan_path.is_absolute()
        || !decisions_path.is_absolute()
        || !output_path.is_absolute()
    {
        return Err(invalid("motion review paths must be absolute"));
    }

    let artifact_bytes = read_bounded_regular(artifact_path)?;
    let artifact = read_motion_artifact(&artifact_bytes)?;

    let plan_bytes = read_bounded_regular(plan_path)?;
    let plan: MusicListMotionReviewPlan = serde_json::from_slice(&plan_bytes)?;
    if canonical_json(&plan)? != plan_bytes || plan.schema != MOTION_REVIEW_PLAN_SCHEMA {
        return Err(invalid("motion review plan is not canonical and versioned"));
    }
    validate_review_plan_against_artifact(&plan, &artifact, &artifact_bytes)?;

    let by_hash = read_motion_review_decisions(decisions_path, &plan_bytes, &plan)?;

    let mut pairs: Vec<MusicListMotionPairRequest> = artifact
        .pairs
        .iter()
        .map(|pair| MusicListMotionPairRequest {
            pair_id: pair.pair_id.clone(),
            first_frame: pair.first_frame.clone(),
            second_frame: pair.second_frame.clone(),
            motion: pair.motion.clone(),
            first_rows: pair.first_rows.clone(),
            second_rows: pair.second_rows.clone(),
        })
        .collect();
    let pair_indexes: BTreeMap<_, _> = pairs
        .iter()
        .enumerate()
        .map(|(index, pair)| (pair.pair_id.clone(), index))
        .collect();
    let mut applied_occurrence_count = 0_usize;
    for group in &plan.groups {
        let Some(annotation) = by_hash.get(&group.crop_pixel_sha256) else {
            continue;
        };
        for occurrence in &group.occurrences {
            let pair = pair_indexes
                .get(&occurrence.pair_id)
                .and_then(|index| pairs.get_mut(*index))
                .ok_or_else(|| invalid("verified review occurrence pair is absent"))?;
            let rows = match occurrence.frame_role {
                MusicListMotionFrameRole::First => &mut pair.first_rows,
                MusicListMotionFrameRole::Second => &mut pair.second_rows,
            };
            rows[usize::from(occurrence.slot)] = annotation.clone();
            applied_occurrence_count += 1;
        }
    }
    let remaining_unknown_occurrence_count = pairs
        .iter()
        .flat_map(|pair| pair.first_rows.iter().chain(pair.second_rows.iter()))
        .filter(|annotation| matches!(annotation, PairRowAnnotation::Unknown { .. }))
        .count();
    let request = MusicListMotionRequest {
        schema: MOTION_REQUEST_SCHEMA.to_owned(),
        catalog_sha256: artifact.catalog_sha256,
        source_manifest_sha256: artifact.source_manifest_sha256,
        capture_profile_id: artifact.capture_profile_id,
        normalizer_artifact_sha256: artifact.normalizer_artifact_sha256,
        canonical_layout_sha256: artifact.canonical_layout_sha256,
        pairs,
    };
    validate_motion_request(&request)?;
    write_new_canonical(output_path, &request)?;
    Ok(MusicListMotionReviewApplySummary {
        schema: "scorepeek-music-list-motion-review-apply-summary-v1",
        source_artifact_bound: true,
        decision_count: by_hash.len(),
        applied_occurrence_count,
        remaining_unknown_occurrence_count,
    })
}

fn read_motion_review_decisions(
    decisions_path: &Path,
    plan_bytes: &[u8],
    plan: &MusicListMotionReviewPlan,
) -> Result<BTreeMap<String, PairRowAnnotation>, CorpusError> {
    let decisions_bytes = read_bounded_regular(decisions_path)?;
    let decisions: MusicListMotionReviewDecisions = serde_json::from_slice(&decisions_bytes)?;
    if canonical_json(&decisions)? != decisions_bytes
        || decisions.schema != MOTION_REVIEW_DECISIONS_SCHEMA
        || decisions.source_review_plan_sha256 != digest_bytes(plan_bytes)
    {
        return Err(invalid(
            "motion review decisions must be canonical and bind the review plan",
        ));
    }
    let known_groups: BTreeSet<_> = plan
        .groups
        .iter()
        .map(|group| group.crop_pixel_sha256.as_str())
        .collect();
    let mut by_hash = BTreeMap::new();
    for decision in decisions.decisions {
        validate_sha256(
            &decision.crop_pixel_sha256,
            "crop_pixel_sha256",
            crate::ErrorContext::Replay,
        )?;
        if !known_groups.contains(decision.crop_pixel_sha256.as_str()) {
            return Err(invalid(
                "motion review decision names an unknown crop group",
            ));
        }
        if matches!(decision.annotation, PairRowAnnotation::Unknown { .. }) {
            return Err(invalid(
                "omit unresolved crop groups instead of deciding unknown",
            ));
        }
        if by_hash
            .insert(decision.crop_pixel_sha256, decision.annotation)
            .is_some()
        {
            return Err(invalid("motion review decision crop group is duplicated"));
        }
    }
    Ok(by_hash)
}

fn read_motion_artifact(artifact_bytes: &[u8]) -> Result<MusicListMotionArtifact, CorpusError> {
    let artifact: MusicListMotionArtifact = serde_json::from_slice(artifact_bytes)?;
    if canonical_json(&artifact)? != artifact_bytes || artifact.schema != MOTION_ARTIFACT_SCHEMA {
        return Err(invalid("motion artifact must be canonical and versioned"));
    }
    validate_motion_metadata(
        &artifact.catalog_sha256,
        &artifact.source_manifest_sha256,
        &artifact.capture_profile_id,
        &artifact.normalizer_artifact_sha256,
        &artifact.canonical_layout_sha256,
        artifact.pairs.len(),
    )?;
    let mut ids = BTreeSet::new();
    let mut frame_pairs = BTreeSet::new();
    for pair in &artifact.pairs {
        validate_pair_identity(pair, &mut ids, &mut frame_pairs)?;
        validate_row_annotations(&pair.first_rows)?;
        validate_row_annotations(&pair.second_rows)?;
    }
    Ok(artifact)
}

fn read_review_crop_manifest(
    document: &MusicListRowObservationDocument,
    frame: &MusicListPairFrame,
) -> Result<MusicSelectCropManifest, CorpusError> {
    frame.validate()?;
    verify_directory(&frame.crop_directory)?;
    let bytes = read_bounded_regular(&frame.crop_directory.join("manifest.json"))?;
    if digest_bytes(&bytes) != frame.crop_manifest_sha256 {
        return Err(invalid("music-list crop manifest digest differs"));
    }
    let manifest: MusicSelectCropManifest = serde_json::from_slice(&bytes)?;
    if canonical_json(&manifest)? != bytes
        || manifest.schema != "scorepeek-private-canonical-music-select-crops-v1"
        || manifest.frame_id != frame.frame_id
        || manifest.frame_extraction_sha256 != frame.frame_extraction_sha256
        || manifest.normalizer_artifact_sha256 != document.normalizer_artifact_sha256
        || manifest.canonical_layout_sha256 != document.canonical_layout_sha256
        || manifest.canonical_layout_sha256 != digest_bytes(CANONICAL_LAYOUT_BYTES)
        || manifest.crops.len() != usize::from(MUSIC_LIST_SLOTS) + 1
    {
        return Err(invalid("music-list crop manifest binding is invalid"));
    }
    validate_sha256(
        &manifest.canonical_frame_sha256,
        "canonical_frame_sha256",
        crate::ErrorContext::Replay,
    )?;
    for (index, crop) in manifest.crops.iter().enumerate() {
        let (field, filename, roi) = expected_crop(index)?;
        if crop.field != field
            || crop.filename != filename
            || crop.roi != roi
            || crop.bytes != ppm_file_bytes(roi)?
        {
            return Err(invalid("music-list crop manifest is not layout-bound"));
        }
        validate_sha256(
            &crop.pixel_sha256,
            "crop pixel_sha256",
            crate::ErrorContext::Replay,
        )?;
        validate_sha256(
            &crop.file_sha256,
            "crop file_sha256",
            crate::ErrorContext::Replay,
        )?;
    }
    Ok(manifest)
}

fn read_measurement_rows(
    document: &MusicListRowObservationDocument,
    frame: &MusicListPairFrame,
) -> Result<Vec<Vec<u8>>, CorpusError> {
    let manifest = read_review_crop_manifest(document, frame)?;
    manifest
        .crops
        .iter()
        .skip(1)
        .map(|crop| read_validated_crop_pixels(&frame.crop_directory, crop))
        .collect()
}

fn validate_review_plan_against_artifact(
    plan: &MusicListMotionReviewPlan,
    artifact: &MusicListMotionArtifact,
    artifact_bytes: &[u8],
) -> Result<(), CorpusError> {
    if plan.source_artifact_sha256 != digest_bytes(artifact_bytes)
        || plan.catalog_sha256 != artifact.catalog_sha256
        || plan.source_manifest_sha256 != artifact.source_manifest_sha256
        || plan.capture_profile_id != artifact.capture_profile_id
        || plan.normalizer_artifact_sha256 != artifact.normalizer_artifact_sha256
        || plan.canonical_layout_sha256 != artifact.canonical_layout_sha256
    {
        return Err(invalid(
            "motion review plan does not bind the selected artifact",
        ));
    }
    let pairs: BTreeMap<_, _> = artifact
        .pairs
        .iter()
        .map(|pair| (pair.pair_id.as_str(), pair))
        .collect();
    let expected_occurrences = artifact
        .pairs
        .len()
        .checked_mul(usize::from(MUSIC_LIST_SLOTS) * 2)
        .ok_or(CorpusError::CapacityExceeded)?;
    let mut group_digests = BTreeSet::new();
    let mut occurrences = BTreeSet::new();
    for group in &plan.groups {
        validate_sha256(
            &group.crop_pixel_sha256,
            "crop_pixel_sha256",
            crate::ErrorContext::Replay,
        )?;
        if !group_digests.insert(group.crop_pixel_sha256.as_str()) || group.occurrences.is_empty() {
            return Err(invalid("motion review plan has an invalid crop group"));
        }
        for occurrence in &group.occurrences {
            let pair = pairs
                .get(occurrence.pair_id.as_str())
                .ok_or_else(|| invalid("motion review occurrence pair is absent"))?;
            let slot = usize::from(occurrence.slot);
            let (frame, annotations) = match occurrence.frame_role {
                MusicListMotionFrameRole::First => (&pair.first_frame, &pair.first_rows),
                MusicListMotionFrameRole::Second => (&pair.second_frame, &pair.second_rows),
            };
            if slot >= usize::from(MUSIC_LIST_SLOTS)
                || occurrence.frame_id != frame.frame_id
                || occurrence.source_pts != frame.source_pts
                || occurrence.decode_index != frame.decode_index
                || occurrence.pair_motion != pair.motion
                || occurrence.current_annotation != annotations[slot]
                || !occurrence.crop_path.is_absolute()
                || !is_sha256(&occurrence.crop_file_sha256)
                || !occurrences.insert((
                    occurrence.pair_id.as_str(),
                    occurrence.frame_role,
                    occurrence.slot,
                ))
            {
                return Err(invalid("motion review occurrence binding is invalid"));
            }
        }
    }
    if occurrences.len() != expected_occurrences {
        return Err(invalid(
            "motion review plan does not cover every row occurrence",
        ));
    }
    Ok(())
}

fn add_review_manifest(
    pair: &MusicListMotionPair,
    frame_role: MusicListMotionFrameRole,
    frame: &MusicListPairFrame,
    annotations: &[PairRowAnnotation; 20],
    manifest: &MusicSelectCropManifest,
    groups: &mut BTreeMap<String, Vec<MusicListMotionReviewOccurrence>>,
) -> Result<(), CorpusError> {
    for (slot, annotation) in annotations.iter().enumerate() {
        let crop = manifest
            .crops
            .get(slot + 1)
            .ok_or_else(|| invalid("music-list slot crop is absent"))?;
        groups.entry(crop.pixel_sha256.clone()).or_default().push(
            MusicListMotionReviewOccurrence {
                pair_id: pair.pair_id.clone(),
                frame_role,
                frame_id: frame.frame_id.clone(),
                source_pts: frame.source_pts,
                decode_index: frame.decode_index,
                slot: u8::try_from(slot).map_err(|_| CorpusError::CapacityExceeded)?,
                pair_motion: pair.motion.clone(),
                current_annotation: annotation.clone(),
                crop_path: frame.crop_directory.join(&crop.filename),
                crop_file_sha256: crop.file_sha256.clone(),
            },
        );
    }
    Ok(())
}

type VerifiedFrameArtifacts = (MusicSelectCropManifest, Vec<Vec<u8>>);

fn verify_motion_pair_artifacts(
    domain: &MusicListRowObservationDocument,
    pair: &MusicListMotionPair,
) -> Result<(VerifiedFrameArtifacts, VerifiedFrameArtifacts), CorpusError> {
    let first = verify_complete_frame_artifacts(domain, &pair.first_frame)?;
    let second = verify_complete_frame_artifacts(domain, &pair.second_frame)?;
    let (rows, aggregate) = measure_rows(&first.1[1..], &second.1[1..])?;
    if rows != pair.row_rgb_l1_sums || aggregate != pair.aggregate_rgb_l1_sum {
        return Err(invalid("motion artifact RGB L1 measurements changed"));
    }
    Ok((first, second))
}

fn validate_motion_request(request: &MusicListMotionRequest) -> Result<(), CorpusError> {
    if request.schema != MOTION_REQUEST_SCHEMA {
        return Err(invalid("unsupported motion request schema"));
    }
    validate_motion_metadata(
        &request.catalog_sha256,
        &request.source_manifest_sha256,
        &request.capture_profile_id,
        &request.normalizer_artifact_sha256,
        &request.canonical_layout_sha256,
        request.pairs.len(),
    )?;
    let mut ids = BTreeSet::new();
    let mut frame_pairs = BTreeSet::new();
    for pair in &request.pairs {
        validate_pair_identity(pair, &mut ids, &mut frame_pairs)?;
        validate_row_annotations(&pair.first_rows)?;
        validate_row_annotations(&pair.second_rows)?;
    }
    Ok(())
}

fn validate_motion_metadata(
    catalog_sha256: &str,
    source_manifest_sha256: &str,
    capture_profile_id: &str,
    normalizer_artifact_sha256: &str,
    canonical_layout_sha256: &str,
    pair_count: usize,
) -> Result<(), CorpusError> {
    for (value, field) in [
        (catalog_sha256, "catalog_sha256"),
        (source_manifest_sha256, "source_manifest_sha256"),
        (normalizer_artifact_sha256, "normalizer_artifact_sha256"),
        (canonical_layout_sha256, "canonical_layout_sha256"),
    ] {
        validate_sha256(value, field, crate::ErrorContext::Replay)?;
    }
    validate_token(
        capture_profile_id,
        "capture_profile_id",
        crate::ErrorContext::Replay,
    )?;
    if capture_profile_id != CALIBRATED_CAPTURE_PROFILE_SHA256
        || canonical_layout_sha256 != digest_bytes(CANONICAL_LAYOUT_BYTES)
        || pair_count == 0
        || pair_count > MAX_OBSERVATIONS
    {
        return Err(invalid("motion artifact domain or pair count is invalid"));
    }
    Ok(())
}

fn validate_pair_identity(
    pair: &impl MotionPairIdentity,
    ids: &mut BTreeSet<String>,
    frame_pairs: &mut BTreeSet<(String, u64, u64)>,
) -> Result<(), CorpusError> {
    validate_opaque_id(pair.pair_id(), "pair_id", crate::ErrorContext::Replay)?;
    pair.first_frame().validate()?;
    pair.second_frame().validate()?;
    validate_pair_motion(pair.motion())?;
    if !ids.insert(pair.pair_id().to_owned())
        || !frame_pairs.insert((
            pair.first_frame().frame_extraction_sha256.clone(),
            pair.first_frame().decode_index,
            pair.second_frame().decode_index,
        ))
        || pair.first_frame().frame_extraction_sha256 != pair.second_frame().frame_extraction_sha256
        || pair.second_frame().decode_index != pair.first_frame().decode_index.saturating_add(1)
    {
        return Err(invalid("motion pairs must be unique adjacent frames"));
    }
    Ok(())
}

trait MotionPairIdentity {
    fn pair_id(&self) -> &str;
    fn first_frame(&self) -> &MusicListPairFrame;
    fn second_frame(&self) -> &MusicListPairFrame;
    fn motion(&self) -> &PairMotion;
}

macro_rules! impl_motion_pair_identity {
    ($type:ty) => {
        impl MotionPairIdentity for $type {
            fn pair_id(&self) -> &str {
                &self.pair_id
            }
            fn first_frame(&self) -> &MusicListPairFrame {
                &self.first_frame
            }
            fn second_frame(&self) -> &MusicListPairFrame {
                &self.second_frame
            }
            fn motion(&self) -> &PairMotion {
                &self.motion
            }
        }
    };
}

impl_motion_pair_identity!(MusicListMotionPairRequest);
impl_motion_pair_identity!(MusicListMotionPair);

fn validate_pair_motion(motion: &PairMotion) -> Result<(), CorpusError> {
    if let PairMotion::Unknown { reason } = motion {
        validate_token(reason, "unknown motion reason", crate::ErrorContext::Replay)?;
    }
    Ok(())
}

fn validate_row_annotations(rows: &[PairRowAnnotation; 20]) -> Result<(), CorpusError> {
    for row in rows {
        if let PairRowAnnotation::Unknown { reason } = row {
            validate_token(reason, "unknown row reason", crate::ErrorContext::Replay)?;
        }
    }
    Ok(())
}

fn motion_domain(
    catalog_sha256: &str,
    source_manifest_sha256: &str,
    capture_profile_id: &str,
    normalizer_artifact_sha256: &str,
    canonical_layout_sha256: &str,
) -> MusicListRowObservationDocument {
    MusicListRowObservationDocument {
        schema: OBSERVATION_SCHEMA.to_owned(),
        catalog_sha256: catalog_sha256.to_owned(),
        source_manifest_sha256: source_manifest_sha256.to_owned(),
        capture_profile_id: capture_profile_id.to_owned(),
        normalizer_artifact_sha256: normalizer_artifact_sha256.to_owned(),
        canonical_layout_sha256: canonical_layout_sha256.to_owned(),
        observations: Vec::new(),
    }
}

fn measure_pair(
    domain: &MusicListRowObservationDocument,
    pair: MusicListMotionPairRequest,
) -> Result<MusicListMotionPair, CorpusError> {
    let first = read_measurement_rows(domain, &pair.first_frame)?;
    let second = read_measurement_rows(domain, &pair.second_frame)?;
    let (row_rgb_l1_sums, aggregate_rgb_l1_sum) = measure_rows(&first, &second)?;
    Ok(MusicListMotionPair {
        pair_id: pair.pair_id,
        first_frame: pair.first_frame,
        second_frame: pair.second_frame,
        motion: pair.motion,
        first_rows: pair.first_rows,
        second_rows: pair.second_rows,
        row_rgb_l1_sums,
        aggregate_rgb_l1_sum,
    })
}

fn measure_rows(first: &[Vec<u8>], second: &[Vec<u8>]) -> Result<([u64; 20], u64), CorpusError> {
    if first.len() != usize::from(MUSIC_LIST_SLOTS) || second.len() != usize::from(MUSIC_LIST_SLOTS)
    {
        return Err(invalid(
            "complete-pair measurement requires all twenty rows",
        ));
    }
    let mut rows = [0_u64; 20];
    for (index, value) in rows.iter_mut().enumerate() {
        *value = rgb_l1_sum(&first[index], &second[index])?;
    }
    let aggregate = rows
        .iter()
        .try_fold(0_u64, |sum, value| sum.checked_add(*value))
        .ok_or(CorpusError::CapacityExceeded)?;
    Ok((rows, aggregate))
}

fn summarize_motion(
    artifact: &MusicListMotionArtifact,
    evidence_verified: bool,
) -> MusicListMotionSummary {
    let mut counts = [0_usize; 3];
    let mut aggregates = Vec::with_capacity(artifact.pairs.len());
    for pair in &artifact.pairs {
        match pair.motion {
            PairMotion::Stationary => counts[0] += 1,
            PairMotion::Scrolling => counts[1] += 1,
            PairMotion::Unknown { .. } => counts[2] += 1,
        }
        aggregates.push(pair.aggregate_rgb_l1_sum);
    }
    let (aggregate_rgb_l1_min, aggregate_rgb_l1_max) = min_max(&aggregates);
    MusicListMotionSummary {
        schema: "scorepeek-music-list-motion-summary-v1",
        evidence_verified,
        pair_count: artifact.pairs.len(),
        stationary_count: counts[0],
        scrolling_count: counts[1],
        unknown_count: counts[2],
        aggregate_rgb_l1_min,
        aggregate_rgb_l1_max,
    }
}

fn write_new_canonical(path: &Path, value: &impl Serialize) -> Result<(), CorpusError> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("motion artifact output has no parent"))?;
    verify_directory(parent)?;
    let bytes = canonical_json(value)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn read_bounded_regular(path: &Path) -> Result<Vec<u8>, CorpusError> {
    read_bounded_regular_after_metadata(path, || Ok(()))
}

fn read_bounded_regular_after_metadata(
    path: &Path,
    after_metadata: impl FnOnce() -> Result<(), CorpusError>,
) -> Result<Vec<u8>, CorpusError> {
    let path_metadata = path.metadata()?;
    if !path_metadata.is_file() {
        return Err(invalid("observation document is not a regular file"));
    }
    after_metadata()?;
    let mut file = File::open(path)?;
    let opened_metadata = file.metadata()?;
    if !opened_metadata.is_file()
        || opened_metadata.dev() != path_metadata.dev()
        || opened_metadata.ino() != path_metadata.ino()
        || opened_metadata.len() == 0
        || opened_metadata.len() > MAX_DOCUMENT_BYTES
    {
        return Err(invalid(
            "observation document is not a bounded stable regular file",
        ));
    }
    let capacity = usize::try_from(opened_metadata.len())
        .map_err(|_| invalid("observation document length is not addressable"))?;
    let mut bytes = Vec::with_capacity(capacity);
    std::io::Read::by_ref(&mut file)
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let final_metadata = file.metadata()?;
    if bytes.len() as u64 != opened_metadata.len()
        || bytes.len() as u64 != final_metadata.len()
        || bytes.len() as u64 > MAX_DOCUMENT_BYTES
        || final_metadata.dev() != opened_metadata.dev()
        || final_metadata.ino() != opened_metadata.ino()
    {
        return Err(invalid("observation document changed while reading"));
    }
    Ok(bytes)
}

impl MusicListRowObservationDocument {
    fn validate(self) -> Result<MusicListRowObservationSummary, CorpusError> {
        if self.schema != OBSERVATION_SCHEMA {
            return Err(invalid("unsupported observation schema"));
        }
        for (value, field) in [
            (&self.catalog_sha256, "catalog_sha256"),
            (&self.source_manifest_sha256, "source_manifest_sha256"),
            (
                &self.normalizer_artifact_sha256,
                "normalizer_artifact_sha256",
            ),
            (&self.canonical_layout_sha256, "canonical_layout_sha256"),
        ] {
            validate_sha256(value, field, crate::ErrorContext::Replay)?;
        }
        validate_token(
            &self.capture_profile_id,
            "capture_profile_id",
            crate::ErrorContext::Replay,
        )?;
        if self.observations.is_empty() || self.observations.len() > MAX_OBSERVATIONS {
            return Err(invalid("observation count is outside bounds"));
        }
        let mut ids = BTreeSet::new();
        let mut rows = BTreeSet::new();
        let mut counts = [0_usize; 6];
        let mut presentation_counts = [0_usize; 4];
        for observation in self.observations {
            validate_opaque_id(
                &observation.observation_id,
                "observation_id",
                crate::ErrorContext::Replay,
            )?;
            if !ids.insert(observation.observation_id) {
                return Err(invalid("observation IDs must be unique"));
            }
            if !rows.insert((
                observation.frame.frame_extraction_sha256.clone(),
                observation.frame.frame_id.clone(),
                observation.slot,
            )) {
                return Err(invalid("each geometric row may be annotated only once"));
            }
            if observation.slot >= MUSIC_LIST_SLOTS {
                return Err(invalid("music-list slot is outside the shared layout"));
            }
            validate_annotation(
                &observation.frame,
                observation.annotation,
                &mut counts,
                &mut presentation_counts,
            )?;
        }
        Ok(MusicListRowObservationSummary {
            schema: "scorepeek-music-list-row-observation-draft-inspection-v1",
            evidence_verified: false,
            catalog_sha256: self.catalog_sha256,
            observation_count: counts.iter().sum(),
            stationary_count: counts[0],
            scrolling_count: counts[1],
            selected_count: counts[2],
            clipped_count: counts[3],
            non_title_count: counts[4],
            unknown_count: counts[5],
            locked_dimmed_count: presentation_counts[0],
            infinitas_blue_count: presentation_counts[1],
            leggendaria_purple_count: presentation_counts[2],
            unlock_condition_count: presentation_counts[3],
            stationary_rgb_l1_min: None,
            stationary_rgb_l1_max: None,
            scrolling_rgb_l1_min: None,
            scrolling_rgb_l1_max: None,
        })
    }
}

fn validate_annotation(
    frame: &MusicListRowFrame,
    annotation: MusicListRowAnnotation,
    counts: &mut [usize; 6],
    presentation_counts: &mut [usize; 4],
) -> Result<(), CorpusError> {
    frame.validate()?;
    match annotation {
        MusicListRowAnnotation::Stationary {
            adjacent_frame,
            reported_rgb_l1_sum,
            reported_compared_rgb_values,
            presentation,
        } => {
            validate_motion(
                frame,
                &adjacent_frame,
                reported_rgb_l1_sum,
                reported_compared_rgb_values,
            )?;
            count_presentation(presentation, presentation_counts);
            counts[0] += 1;
        }
        MusicListRowAnnotation::Scrolling {
            adjacent_frame,
            reported_rgb_l1_sum,
            reported_compared_rgb_values,
            presentation,
        } => {
            validate_motion(
                frame,
                &adjacent_frame,
                reported_rgb_l1_sum,
                reported_compared_rgb_values,
            )?;
            count_presentation(presentation, presentation_counts);
            counts[1] += 1;
        }
        MusicListRowAnnotation::Selected => counts[2] += 1,
        MusicListRowAnnotation::Clipped { .. } => counts[3] += 1,
        MusicListRowAnnotation::NonTitle { kind } => {
            if kind == NonTitleKind::UnlockCondition {
                presentation_counts[3] += 1;
            }
            counts[4] += 1;
        }
        MusicListRowAnnotation::Unknown { reason } => {
            validate_token(&reason, "unknown reason", crate::ErrorContext::Replay)?;
            counts[5] += 1;
        }
    }
    Ok(())
}

fn count_presentation(presentation: TitlePresentation, counts: &mut [usize; 4]) {
    if presentation.availability == TitleAvailability::LockedDimmed {
        counts[0] += 1;
    }
    match presentation.color_domain {
        TitleColorDomain::Standard => {}
        TitleColorDomain::InfinitasBlue => counts[1] += 1,
        TitleColorDomain::LeggendariaPurple => counts[2] += 1,
    }
}

fn verify_frame_artifacts(
    document: &MusicListRowObservationDocument,
    slot: u8,
    frame: &MusicListRowFrame,
) -> Result<Vec<u8>, CorpusError> {
    let pair_frame = MusicListPairFrame {
        frame_extraction_directory: frame.frame_extraction_directory.clone(),
        frame_extraction_sha256: frame.frame_extraction_sha256.clone(),
        crop_directory: frame.crop_directory.clone(),
        crop_manifest_sha256: frame.crop_manifest_sha256.clone(),
        frame_id: frame.frame_id.clone(),
        source_pts: frame.source_pts,
        decode_index: frame.decode_index,
    };
    let (crop_manifest, crop_pixels) = verify_complete_frame_artifacts(document, &pair_frame)?;
    let crop_index = usize::from(slot) + 1;
    let crop = crop_manifest
        .crops
        .get(crop_index)
        .ok_or_else(|| invalid("music-list slot crop is absent"))?;
    if crop.file_sha256 != frame.crop_file_sha256 || crop.pixel_sha256 != frame.crop_pixel_sha256 {
        return Err(invalid("music-list slot crop identity is invalid"));
    }
    crop_pixels
        .into_iter()
        .nth(crop_index)
        .ok_or_else(|| invalid("music-list slot crop is absent"))
}

fn verify_complete_frame_artifacts(
    document: &MusicListRowObservationDocument,
    frame: &MusicListPairFrame,
) -> Result<(MusicSelectCropManifest, Vec<Vec<u8>>), CorpusError> {
    frame.validate()?;
    verify_directory(&frame.frame_extraction_directory)?;
    let extraction_bytes =
        read_bounded_regular(&frame.frame_extraction_directory.join("manifest.json"))?;
    if digest_bytes(&extraction_bytes) != frame.frame_extraction_sha256 {
        return Err(invalid("canonical extraction manifest hash changed"));
    }
    let extraction: CanonicalExtractionManifest = serde_json::from_slice(&extraction_bytes)?;
    validate_extraction_manifest(&extraction, &extraction_bytes, document)?;
    validate_normalizer_manifest(document, &extraction, &frame.frame_extraction_directory)?;
    let extracted = extraction
        .frames
        .iter()
        .find(|candidate| candidate.frame_id == frame.frame_id)
        .ok_or_else(|| invalid("observation frame is absent from canonical extraction"))?;
    if extracted.source_pts != frame.source_pts
        || extracted.decode_index != frame.decode_index
        || extracted.filename.contains('/')
        || extracted.filename.contains('\\')
    {
        return Err(invalid(
            "observation frame identity does not match extraction",
        ));
    }
    let frame_file =
        read_bounded_regular(&frame.frame_extraction_directory.join(&extracted.filename))?;
    if frame_file.len() as u64 != extracted.bytes
        || digest_bytes(&frame_file) != extracted.file_sha256
    {
        return Err(invalid("canonical frame file hash changed"));
    }
    let canonical_pixels = parse_ppm(&frame_file, 1_920, 1_080)?;
    if canonical_pixels.len() != CANONICAL_FRAME_RGB_VALUES
        || digest_bytes(canonical_pixels) != extracted.frame_sha256
    {
        return Err(invalid("canonical frame pixels do not match extraction"));
    }

    verify_directory(&frame.crop_directory)?;
    let crop_manifest_bytes = read_bounded_regular(&frame.crop_directory.join("manifest.json"))?;
    if digest_bytes(&crop_manifest_bytes) != frame.crop_manifest_sha256 {
        return Err(invalid("music-list crop manifest hash changed"));
    }
    let crop_manifest: MusicSelectCropManifest = serde_json::from_slice(&crop_manifest_bytes)?;
    if canonical_json(&crop_manifest)? != crop_manifest_bytes
        || crop_manifest.schema != "scorepeek-private-canonical-music-select-crops-v1"
        || crop_manifest.frame_id != frame.frame_id
        || crop_manifest.frame_extraction_sha256 != frame.frame_extraction_sha256
        || crop_manifest.canonical_frame_sha256 != extracted.frame_sha256
        || crop_manifest.normalizer_artifact_sha256 != document.normalizer_artifact_sha256
        || crop_manifest.canonical_layout_sha256 != document.canonical_layout_sha256
        || crop_manifest.canonical_layout_sha256 != digest_bytes(CANONICAL_LAYOUT_BYTES)
    {
        return Err(invalid("music-list crop manifest binding is invalid"));
    }
    let crop_pixels =
        validate_complete_crop_artifact(&frame.crop_directory, &crop_manifest, canonical_pixels)?;
    Ok((crop_manifest, crop_pixels))
}

fn validate_extraction_manifest(
    extraction: &CanonicalExtractionManifest,
    bytes: &[u8],
    document: &MusicListRowObservationDocument,
) -> Result<(), CorpusError> {
    if canonical_json(extraction)? != bytes
        || extraction.schema != "scorepeek-private-canonical-frame-extraction-v1"
        || extraction.source_manifest_sha256 != document.source_manifest_sha256
        || extraction.capture_profile_id != document.capture_profile_id
        || extraction.capture_profile_id != CALIBRATED_CAPTURE_PROFILE_SHA256
        || extraction.normalizer_artifact_sha256 != document.normalizer_artifact_sha256
        || extraction.canonical_frame_contract_id != "scorepeek-canonical-rgb8-1920x1080-v1"
        || extraction.fixture_id.is_empty()
        || extraction.extractor.tool_id != "ffmpeg"
        || extraction.extractor.tool_version != "8.1.2"
        || extraction.extractor.extractor_manifest_sha256 != extraction.media_probe_sha256
        || extraction.source_time_base
            != (TimeBase {
                numerator: 1,
                denominator: 1_000,
            })
        || extraction.video_stream_index > 255
        || extraction.frames.is_empty()
        || extraction.frames.len() > 512
    {
        return Err(invalid("canonical extraction binding is invalid"));
    }
    for (value, field) in [
        (&extraction.source_manifest_sha256, "source_manifest_sha256"),
        (&extraction.media_probe_sha256, "media_probe_sha256"),
        (&extraction.capture_profile_id, "capture_profile_id"),
        (
            &extraction.normalizer_artifact_sha256,
            "normalizer_artifact_sha256",
        ),
        (
            &extraction.extractor.parameters_sha256,
            "extractor parameters_sha256",
        ),
    ] {
        validate_sha256(value, field, crate::ErrorContext::Replay)?;
    }
    let mut ids = BTreeSet::new();
    let mut previous = None;
    for (index, frame) in extraction.frames.iter().enumerate() {
        validate_opaque_id(&frame.frame_id, "frame_id", crate::ErrorContext::Replay)?;
        validate_sha256(
            &frame.frame_sha256,
            "frame_sha256",
            crate::ErrorContext::Replay,
        )?;
        validate_sha256(
            &frame.file_sha256,
            "file_sha256",
            crate::ErrorContext::Replay,
        )?;
        if frame.filename != format!("frame-{index:06}.ppm")
            || frame.bytes != 6_220_817
            || !ids.insert(&frame.frame_id)
            || previous.is_some_and(|value| value >= frame.decode_index)
        {
            return Err(invalid("canonical extraction frame set is invalid"));
        }
        previous = Some(frame.decode_index);
    }
    Ok(())
}

fn validate_normalizer_manifest(
    document: &MusicListRowObservationDocument,
    extraction: &CanonicalExtractionManifest,
    directory: &Path,
) -> Result<(), CorpusError> {
    let bytes = read_bounded_regular(&directory.join("normalizer.json"))?;
    if digest_bytes(&bytes) != document.normalizer_artifact_sha256 {
        return Err(invalid("canonical normalizer hash changed"));
    }
    let normalizer: DomainNormalizerManifest = serde_json::from_slice(&bytes)?;
    if canonical_json(&normalizer)? != bytes
        || normalizer.schema != "scorepeek-domain-normalizer-artifact-v1"
        || normalizer.capture_profile_id != document.capture_profile_id
        || normalizer.capture_profile_id != CALIBRATED_CAPTURE_PROFILE_SHA256
        || normalizer.canonical_frame_contract_id != "scorepeek-canonical-rgb8-1920x1080-v1"
        || normalizer.implementation != "ffmpeg-swscale-bt709-limited-to-rgb24-v1"
        || normalizer.ffmpeg_sha256 != CALIBRATED_FFMPEG_SHA256
        || normalizer.filter != NORMALIZER_FILTER
        || normalizer.observed.source_time_base != extraction.source_time_base
        || !normalizer.observed.is_supported()
    {
        return Err(invalid("canonical normalizer binding is invalid"));
    }
    Ok(())
}

impl ObservedMediaContract {
    fn is_supported(&self) -> bool {
        self.input_format == "matroska"
            && self.codec_name == "ffv1"
            && self.pixel_format == "yuv420p"
            && self.width == 1_920
            && self.height == 1_080
            && self.source_time_base
                == (TimeBase {
                    numerator: 1,
                    denominator: 1_000,
                })
            && self.color_range.as_deref() == Some("tv")
            && self.color_space.as_deref() == Some("bt709")
            && self.color_transfer.as_deref() == Some("bt709")
            && self.color_primaries.as_deref() == Some("bt709")
    }
}

fn validate_complete_crop_artifact(
    directory: &Path,
    manifest: &MusicSelectCropManifest,
    canonical_pixels: &[u8],
) -> Result<Vec<Vec<u8>>, CorpusError> {
    if manifest.crops.len() != usize::from(MUSIC_LIST_SLOTS) + 1 {
        return Err(invalid("music-list crop set is incomplete"));
    }
    let mut verified_pixels = Vec::with_capacity(manifest.crops.len());
    for (index, crop) in manifest.crops.iter().enumerate() {
        let (field, filename, roi) = expected_crop(index)?;
        let expected_bytes = ppm_file_bytes(roi)?;
        validate_sha256(
            &crop.pixel_sha256,
            "crop pixel_sha256",
            crate::ErrorContext::Replay,
        )?;
        validate_sha256(
            &crop.file_sha256,
            "crop file_sha256",
            crate::ErrorContext::Replay,
        )?;
        if crop.field != field
            || crop.filename != filename
            || crop.roi != roi
            || crop.bytes != expected_bytes
        {
            return Err(invalid("music-list crop set is not layout-bound"));
        }
        let pixels = read_validated_crop_pixels(directory, crop)?;
        if pixels != crop_from_canonical(canonical_pixels, roi)? {
            return Err(invalid(
                "music-list crop pixels do not match canonical frame",
            ));
        }
        verified_pixels.push(pixels);
    }
    Ok(verified_pixels)
}

fn expected_crop(index: usize) -> Result<(String, String, CropRoi), CorpusError> {
    if index == 0 {
        return Ok((
            "selected_title".to_owned(),
            "selected-title.ppm".to_owned(),
            CropRoi {
                x: 140,
                y: 315,
                width: 820,
                height: 100,
            },
        ));
    }
    let slot = u32::try_from(index - 1).map_err(|_| invalid("music-list slot is invalid"))?;
    Ok((
        format!("list_title_{slot:02}"),
        format!("list-title-{slot:02}.ppm"),
        CropRoi {
            x: 1_335,
            y: 20 + slot * 50,
            width: 475,
            height: 45,
        },
    ))
}

fn ppm_file_bytes(roi: CropRoi) -> Result<u64, CorpusError> {
    let header = format!("P6\n{} {}\n255\n", roi.width, roi.height);
    u64::from(roi.width)
        .checked_mul(u64::from(roi.height))
        .and_then(|pixels| pixels.checked_mul(3))
        .and_then(|pixels| pixels.checked_add(header.len() as u64))
        .ok_or(CorpusError::CapacityExceeded)
}

fn read_validated_crop_pixels(
    directory: &Path,
    crop: &MusicSelectCrop,
) -> Result<Vec<u8>, CorpusError> {
    let crop_file = read_bounded_regular(&directory.join(&crop.filename))?;
    if crop_file.len() as u64 != crop.bytes || digest_bytes(&crop_file) != crop.file_sha256 {
        return Err(invalid("music-list crop file hash changed"));
    }
    let pixels = parse_ppm(&crop_file, crop.roi.width, crop.roi.height)?;
    if digest_bytes(pixels) != crop.pixel_sha256 {
        return Err(invalid("music-list crop pixel hash changed"));
    }
    Ok(pixels.to_vec())
}

fn verify_directory(path: &Path) -> Result<(), CorpusError> {
    let metadata = path.metadata()?;
    if !metadata.is_dir() {
        return Err(invalid("artifact directory is not a directory"));
    }
    Ok(())
}

fn parse_ppm(bytes: &[u8], width: u32, height: u32) -> Result<&[u8], CorpusError> {
    let header = format!("P6\n{width} {height}\n255\n");
    let pixels = bytes
        .strip_prefix(header.as_bytes())
        .ok_or_else(|| invalid("artifact is not a canonical binary PPM"))?;
    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| invalid("PPM dimensions overflow"))?;
    if pixels.len() != expected {
        return Err(invalid("PPM pixel length is invalid"));
    }
    Ok(pixels)
}

fn crop_from_canonical(pixels: &[u8], roi: CropRoi) -> Result<Vec<u8>, CorpusError> {
    let row_bytes = usize::try_from(roi.width)
        .ok()
        .and_then(|width| width.checked_mul(3))
        .ok_or_else(|| invalid("crop row width overflow"))?;
    let mut crop = Vec::with_capacity(row_bytes * usize::try_from(roi.height).unwrap_or(0));
    for y in roi.y..roi.y + roi.height {
        let start = (usize::try_from(y).map_err(|_| invalid("crop y is invalid"))? * 1_920
            + usize::try_from(roi.x).map_err(|_| invalid("crop x is invalid"))?)
            * 3;
        let end = start + row_bytes;
        crop.extend_from_slice(
            pixels
                .get(start..end)
                .ok_or_else(|| invalid("crop is outside canonical frame"))?,
        );
    }
    Ok(crop)
}

fn rgb_l1_sum(left: &[u8], right: &[u8]) -> Result<u64, CorpusError> {
    if left.len() as u64 != MUSIC_LIST_ROW_RGB_VALUES || left.len() != right.len() {
        return Err(invalid("RGB L1 inputs have different geometry"));
    }
    Ok(left
        .iter()
        .zip(right)
        .map(|(left, right)| u64::from(left.abs_diff(*right)))
        .sum())
}

fn min_max(values: &[u64]) -> (Option<u64>, Option<u64>) {
    (values.iter().min().copied(), values.iter().max().copied())
}

impl MusicListRowFrame {
    fn validate(&self) -> Result<(), CorpusError> {
        if !self.frame_extraction_directory.is_absolute() || !self.crop_directory.is_absolute() {
            return Err(invalid("music-list artifact directories must be absolute"));
        }
        for (value, field) in [
            (&self.frame_extraction_sha256, "frame_extraction_sha256"),
            (&self.crop_manifest_sha256, "crop_manifest_sha256"),
            (&self.crop_file_sha256, "crop_file_sha256"),
            (&self.crop_pixel_sha256, "crop_pixel_sha256"),
        ] {
            validate_sha256(value, field, crate::ErrorContext::Replay)?;
        }
        validate_opaque_id(&self.frame_id, "frame_id", crate::ErrorContext::Replay)
    }
}

impl MusicListPairFrame {
    fn validate(&self) -> Result<(), CorpusError> {
        if !self.frame_extraction_directory.is_absolute() || !self.crop_directory.is_absolute() {
            return Err(invalid("music-list artifact directories must be absolute"));
        }
        for (value, field) in [
            (&self.frame_extraction_sha256, "frame_extraction_sha256"),
            (&self.crop_manifest_sha256, "crop_manifest_sha256"),
        ] {
            validate_sha256(value, field, crate::ErrorContext::Replay)?;
        }
        validate_opaque_id(&self.frame_id, "frame_id", crate::ErrorContext::Replay)
    }
}

fn validate_motion(
    frame: &MusicListRowFrame,
    adjacent: &MusicListRowFrame,
    reported_rgb_l1_sum: u64,
    reported_compared_rgb_values: u64,
) -> Result<(), CorpusError> {
    adjacent.validate()?;
    if frame.frame_extraction_sha256 != adjacent.frame_extraction_sha256
        || frame.frame_id == adjacent.frame_id
        || frame.decode_index.abs_diff(adjacent.decode_index) != 1
        || frame.source_pts == adjacent.source_pts
        || reported_compared_rgb_values != MUSIC_LIST_ROW_RGB_VALUES
        || reported_rgb_l1_sum > MUSIC_LIST_ROW_RGB_VALUES * 255
    {
        return Err(invalid(
            "stationary and scrolling states require a valid adjacent-frame RGB comparison",
        ));
    }
    Ok(())
}

fn invalid(detail: &str) -> CorpusError {
    CorpusError::InvalidReplay(detail.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, OpenOptions};
    use std::io::{Seek as _, SeekFrom, Write as _};

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;

    fn canonical(value: &serde_json::Value) -> Vec<u8> {
        let document: MusicListRowObservationDocument =
            serde_json::from_value(value.clone()).unwrap();
        let mut bytes = serde_json::to_vec(&document).unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn frame(index: u64) -> serde_json::Value {
        json!({
            "frame_extraction_directory": "/private/extraction",
            "frame_extraction_sha256": "1".repeat(64),
            "crop_directory": format!("/private/crops/{index}"),
            "crop_manifest_sha256": format!("{index:064x}"),
            "frame_id": format!("frame-{index}"),
            "source_pts": i64::try_from(index).unwrap() * 1_000,
            "decode_index": index,
            "crop_file_sha256": "a".repeat(64),
            "crop_pixel_sha256": "b".repeat(64)
        })
    }

    fn document(annotation: &serde_json::Value) -> serde_json::Value {
        json!({
            "schema": OBSERVATION_SCHEMA,
            "catalog_sha256": "c".repeat(64),
            "source_manifest_sha256": "d".repeat(64),
            "capture_profile_id": "profile-v1",
            "normalizer_artifact_sha256": "e".repeat(64),
            "canonical_layout_sha256": "f".repeat(64),
            "observations": [{
                "observation_id": "observation-1",
                "slot": 3,
                "frame": frame(10),
                "annotation": annotation
            }]
        })
    }

    fn standard_presentation() -> serde_json::Value {
        json!({"availability": "available", "color_domain": "standard"})
    }

    fn ppm(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
        bytes.extend_from_slice(pixels);
        bytes
    }

    fn pair_frame(value: &serde_json::Value) -> serde_json::Value {
        let mut value = value.clone();
        let object = value.as_object_mut().unwrap();
        object.remove("crop_file_sha256");
        object.remove("crop_pixel_sha256");
        value
    }

    fn motion_pair(
        pair_id: &str,
        extraction_sha256: &str,
        first_pts: i64,
        second_pts: i64,
    ) -> MusicListMotionPairRequest {
        let frame = |decode_index, source_pts| MusicListPairFrame {
            frame_extraction_directory: PathBuf::from("/private/extraction"),
            frame_extraction_sha256: extraction_sha256.to_owned(),
            crop_directory: PathBuf::from(format!("/private/crops/{decode_index}")),
            crop_manifest_sha256: format!("{decode_index:064x}"),
            frame_id: format!("frame-{decode_index}"),
            source_pts,
            decode_index,
        };
        MusicListMotionPairRequest {
            pair_id: pair_id.to_owned(),
            first_frame: frame(10, first_pts),
            second_frame: frame(11, second_pts),
            motion: PairMotion::Unknown {
                reason: "pending-review".to_owned(),
            },
            first_rows: std::array::from_fn(|_| PairRowAnnotation::Unknown {
                reason: "pending-review".to_owned(),
            }),
            second_rows: std::array::from_fn(|_| PairRowAnnotation::Unknown {
                reason: "pending-review".to_owned(),
            }),
        }
    }

    #[test]
    fn stationary_and_scrolling_bind_adjacent_frame_measurements() {
        for state in ["stationary", "scrolling"] {
            let value = document(&json!({
                "state": state,
                "adjacent_frame": frame(11),
                "reported_rgb_l1_sum": 1234,
                "reported_compared_rgb_values": MUSIC_LIST_ROW_RGB_VALUES,
                "presentation": standard_presentation()
            }));
            let directory = tempdir().unwrap();
            let path = directory.path().join(format!("{state}.json"));
            fs::write(&path, canonical(&value)).unwrap();
            let summary = inspect_music_list_row_observation_draft(&path).unwrap();
            assert!(!summary.evidence_verified);
            assert_eq!(summary.observation_count, 1);
            assert_eq!(summary.stationary_count + summary.scrolling_count, 1);
        }
    }

    #[test]
    fn every_non_training_state_is_explicit_and_value_free() {
        for classification in [
            json!({"state": "selected"}),
            json!({"state": "clipped", "edge": "left"}),
            json!({"state": "non_title", "kind": "separator"}),
            json!({"state": "unknown", "reason": "unobservable"}),
        ] {
            let value = document(&classification);
            let directory = tempdir().unwrap();
            let path = directory.path().join("observations.json");
            fs::write(&path, canonical(&value)).unwrap();
            assert_eq!(
                inspect_music_list_row_observation_draft(&path)
                    .unwrap()
                    .observation_count,
                1
            );
        }
    }

    #[test]
    fn title_presentation_and_unlock_condition_are_orthogonal_counts() {
        let locked = document(&json!({
            "state": "stationary",
            "adjacent_frame": frame(11),
            "reported_rgb_l1_sum": 1234,
            "reported_compared_rgb_values": MUSIC_LIST_ROW_RGB_VALUES,
            "presentation": {
                "availability": "locked_dimmed",
                "color_domain": "infinitas_blue"
            }
        }));
        let directory = tempdir().unwrap();
        let path = directory.path().join("locked.json");
        fs::write(&path, canonical(&locked)).unwrap();
        let summary = inspect_music_list_row_observation_draft(&path).unwrap();
        assert_eq!(summary.locked_dimmed_count, 1);
        assert_eq!(summary.infinitas_blue_count, 1);

        let unlock = document(&json!({"state": "non_title", "kind": "unlock_condition"}));
        fs::write(&path, canonical(&unlock)).unwrap();
        let summary = inspect_music_list_row_observation_draft(&path).unwrap();
        assert_eq!(summary.unlock_condition_count, 1);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn explicit_verify_recomputes_l1_but_review_planning_trusts_the_artifact() {
        let directory = tempdir().unwrap();
        let extraction = directory.path().join("extraction");
        let first_crop_directory = directory.path().join("crop-0");
        let second_crop_directory = directory.path().join("crop-1");
        fs::create_dir(&extraction).unwrap();
        fs::create_dir(&first_crop_directory).unwrap();
        fs::create_dir(&second_crop_directory).unwrap();
        let layout = digest_bytes(CANONICAL_LAYOUT_BYTES);
        let source = "d".repeat(64);
        let probe = "a".repeat(64);
        let normalizer_manifest = DomainNormalizerManifest {
            schema: "scorepeek-domain-normalizer-artifact-v1".to_owned(),
            capture_profile_id: CALIBRATED_CAPTURE_PROFILE_SHA256.to_owned(),
            observed: ObservedMediaContract {
                input_format: "matroska".to_owned(),
                codec_name: "ffv1".to_owned(),
                pixel_format: "yuv420p".to_owned(),
                width: 1_920,
                height: 1_080,
                source_time_base: TimeBase {
                    numerator: 1,
                    denominator: 1_000,
                },
                color_range: Some("tv".to_owned()),
                color_space: Some("bt709".to_owned()),
                color_transfer: Some("bt709".to_owned()),
                color_primaries: Some("bt709".to_owned()),
            },
            canonical_frame_contract_id: "scorepeek-canonical-rgb8-1920x1080-v1".to_owned(),
            implementation: "ffmpeg-swscale-bt709-limited-to-rgb24-v1".to_owned(),
            ffmpeg_sha256: CALIBRATED_FFMPEG_SHA256.to_owned(),
            filter: NORMALIZER_FILTER.to_owned(),
        };
        let normalizer_bytes = canonical_json(&normalizer_manifest).unwrap();
        let normalizer = digest_bytes(&normalizer_bytes);
        fs::write(extraction.join("normalizer.json"), normalizer_bytes).unwrap();
        let mut extracted_frames = Vec::new();
        let mut canonical_frames = Vec::new();
        for (index, value, crop_directory) in [
            (0_u64, 0_u8, &first_crop_directory),
            (1, 1, &second_crop_directory),
        ] {
            let frame_pixels = vec![value; CANONICAL_FRAME_RGB_VALUES];
            let frame_bytes = ppm(1_920, 1_080, &frame_pixels);
            let filename = format!("frame-{index:06}.ppm");
            fs::write(extraction.join(&filename), &frame_bytes).unwrap();
            extracted_frames.push(json!({
                "frame_id": format!("frame-{index}"),
                "source_pts": i64::try_from(index).unwrap() * 17,
                "decode_index": index,
                "filename": filename,
                "frame_sha256": digest_bytes(&frame_pixels),
                "file_sha256": digest_bytes(&frame_bytes),
                "bytes": frame_bytes.len()
            }));
            canonical_frames.push((crop_directory.clone(), frame_pixels));
        }
        let extraction_manifest: CanonicalExtractionManifest = serde_json::from_value(json!({
            "schema": "scorepeek-private-canonical-frame-extraction-v1",
            "fixture_id": "fixture-1",
            "source_manifest_sha256": source,
            "media_probe_sha256": probe,
            "capture_profile_id": CALIBRATED_CAPTURE_PROFILE_SHA256,
            "normalizer_artifact_sha256": normalizer,
            "canonical_frame_contract_id": "scorepeek-canonical-rgb8-1920x1080-v1",
            "extractor": {
                "tool_id": "ffmpeg",
                "tool_version": "8.1.2",
                "extractor_manifest_sha256": probe,
                "parameters_sha256": "b".repeat(64)
            },
            "source_time_base": {"numerator": 1, "denominator": 1000},
            "video_stream_index": 0,
            "frames": extracted_frames
        }))
        .unwrap();
        let extraction_bytes = canonical_json(&extraction_manifest).unwrap();
        fs::write(extraction.join("manifest.json"), &extraction_bytes).unwrap();
        let extraction_sha256 = digest_bytes(&extraction_bytes);

        let mut references = Vec::new();
        for (index, (crop_directory, frame_pixels)) in canonical_frames.into_iter().enumerate() {
            let mut crops = Vec::new();
            let mut observed_crop = None;
            for crop_index in 0..=usize::from(MUSIC_LIST_SLOTS) {
                let (field, filename, roi) = expected_crop(crop_index).unwrap();
                let crop_pixels = crop_from_canonical(&frame_pixels, roi).unwrap();
                let crop_bytes = ppm(roi.width, roi.height, &crop_pixels);
                fs::write(crop_directory.join(&filename), &crop_bytes).unwrap();
                let crop = MusicSelectCrop {
                    field,
                    filename,
                    roi,
                    pixel_sha256: digest_bytes(&crop_pixels),
                    file_sha256: digest_bytes(&crop_bytes),
                    bytes: crop_bytes.len() as u64,
                };
                if crop_index == 4 {
                    observed_crop = Some((crop.file_sha256.clone(), crop.pixel_sha256.clone()));
                }
                crops.push(crop);
            }
            let crop_manifest = MusicSelectCropManifest {
                schema: "scorepeek-private-canonical-music-select-crops-v1".to_owned(),
                frame_id: format!("frame-{index}"),
                frame_extraction_sha256: extraction_sha256.clone(),
                canonical_frame_sha256: digest_bytes(&frame_pixels),
                normalizer_artifact_sha256: normalizer.clone(),
                canonical_layout_sha256: layout.clone(),
                crops,
            };
            let crop_manifest_bytes = canonical_json(&crop_manifest).unwrap();
            fs::write(crop_directory.join("manifest.json"), &crop_manifest_bytes).unwrap();
            let (crop_file_sha256, crop_pixel_sha256) = observed_crop.unwrap();
            references.push(json!({
                "frame_extraction_directory": extraction,
                "frame_extraction_sha256": extraction_sha256,
                "crop_directory": crop_directory,
                "crop_manifest_sha256": digest_bytes(&crop_manifest_bytes),
                "frame_id": format!("frame-{index}"),
                "source_pts": i64::try_from(index).unwrap() * 17,
                "decode_index": index,
                "crop_file_sha256": crop_file_sha256,
                "crop_pixel_sha256": crop_pixel_sha256
            }));
        }
        let mut value = json!({
            "schema": OBSERVATION_SCHEMA,
            "catalog_sha256": "c".repeat(64),
            "source_manifest_sha256": source,
            "capture_profile_id": CALIBRATED_CAPTURE_PROFILE_SHA256,
            "normalizer_artifact_sha256": normalizer,
            "canonical_layout_sha256": layout,
            "observations": [{
                "observation_id": "observation-1",
                "slot": 3,
                "frame": references[0],
                "annotation": {
                    "state": "scrolling",
                    "adjacent_frame": references[1],
                    "reported_rgb_l1_sum": MUSIC_LIST_ROW_RGB_VALUES,
                    "reported_compared_rgb_values": MUSIC_LIST_ROW_RGB_VALUES,
                    "presentation": standard_presentation()
                }
            }]
        });
        let path = directory.path().join("observations.json");
        fs::write(&path, canonical(&value)).unwrap();
        let summary = verify_music_list_row_observation_draft(&path).unwrap();
        assert!(summary.evidence_verified);
        assert_eq!(
            summary.scrolling_rgb_l1_min,
            Some(MUSIC_LIST_ROW_RGB_VALUES)
        );

        let complete_manifest_bytes =
            fs::read(second_crop_directory.join("manifest.json")).unwrap();
        let mut incomplete_manifest: MusicSelectCropManifest =
            serde_json::from_slice(&complete_manifest_bytes).unwrap();
        incomplete_manifest.crops.pop();
        let incomplete_manifest_bytes = canonical_json(&incomplete_manifest).unwrap();
        fs::write(
            second_crop_directory.join("manifest.json"),
            &incomplete_manifest_bytes,
        )
        .unwrap();
        value["observations"][0]["annotation"]["adjacent_frame"]["crop_manifest_sha256"] =
            json!(digest_bytes(&incomplete_manifest_bytes));
        fs::write(&path, canonical(&value)).unwrap();
        assert!(verify_music_list_row_observation_draft(&path).is_err());

        fs::write(
            second_crop_directory.join("manifest.json"),
            &complete_manifest_bytes,
        )
        .unwrap();
        value["observations"][0]["annotation"]["adjacent_frame"]["crop_manifest_sha256"] =
            json!(digest_bytes(&complete_manifest_bytes));
        fs::write(&path, canonical(&value)).unwrap();

        let unknown_rows = vec![json!({"content": "unknown", "reason": "pending-review"}); 20];
        let motion_request: MusicListMotionRequest = serde_json::from_value(json!({
            "schema": MOTION_REQUEST_SCHEMA,
            "catalog_sha256": "c".repeat(64),
            "source_manifest_sha256": source,
            "capture_profile_id": CALIBRATED_CAPTURE_PROFILE_SHA256,
            "normalizer_artifact_sha256": normalizer,
            "canonical_layout_sha256": layout,
            "pairs": [{
                "pair_id": "pair-1",
                "first_frame": pair_frame(&references[0]),
                "second_frame": pair_frame(&references[1]),
                "motion": {"state": "unknown", "reason": "pending-review"},
                "first_rows": unknown_rows,
                "second_rows": unknown_rows
            }]
        }))
        .unwrap();
        let motion_request_path = directory.path().join("motion-request.json");
        let motion_artifact_path = directory.path().join("motion-artifact.json");
        fs::write(
            &motion_request_path,
            canonical_json(&motion_request).unwrap(),
        )
        .unwrap();
        let motion_summary =
            measure_music_list_motion(&motion_request_path, &motion_artifact_path).unwrap();
        assert_eq!(motion_summary.pair_count, 1);
        assert_eq!(motion_summary.unknown_count, 1);
        assert_eq!(
            motion_summary.aggregate_rgb_l1_min,
            Some(MUSIC_LIST_ROW_RGB_VALUES * u64::from(MUSIC_LIST_SLOTS))
        );
        assert!(
            verify_music_list_motion(&motion_artifact_path)
                .unwrap()
                .evidence_verified
        );
        let review_plan_path = directory.path().join("motion-review-plan.json");
        let review_summary =
            plan_music_list_motion_review(&motion_artifact_path, &review_plan_path).unwrap();
        assert!(review_summary.source_artifact_bound);
        assert_eq!(review_summary.pair_count, 1);
        assert_eq!(review_summary.observation_count, 40);
        assert_eq!(review_summary.unique_crop_count, 2);
        assert_eq!(review_summary.duplicate_group_count, 2);
        assert_eq!(review_summary.exact_duplicate_savings_count, 38);
        let review_plan: serde_json::Value =
            serde_json::from_slice(&fs::read(&review_plan_path).unwrap()).unwrap();
        assert_eq!(review_plan["schema"], MOTION_REVIEW_PLAN_SCHEMA);
        assert_eq!(
            review_plan["source_artifact_sha256"],
            digest_bytes(&fs::read(&motion_artifact_path).unwrap())
        );
        assert_eq!(review_plan["groups"].as_array().unwrap().len(), 2);
        let reviewed_group = review_plan["groups"][0]["crop_pixel_sha256"]
            .as_str()
            .unwrap()
            .to_owned();
        let decisions = MusicListMotionReviewDecisions {
            schema: MOTION_REVIEW_DECISIONS_SCHEMA.to_owned(),
            source_review_plan_sha256: digest_bytes(&fs::read(&review_plan_path).unwrap()),
            decisions: vec![MusicListMotionReviewDecision {
                crop_pixel_sha256: reviewed_group,
                annotation: PairRowAnnotation::Selected,
            }],
        };
        let decisions_path = directory.path().join("motion-review-decisions.json");
        fs::write(&decisions_path, canonical_json(&decisions).unwrap()).unwrap();
        let reviewed_request_path = directory.path().join("reviewed-motion-request.json");
        let apply_summary = apply_music_list_motion_review(
            &motion_artifact_path,
            &review_plan_path,
            &decisions_path,
            &reviewed_request_path,
        )
        .unwrap();
        assert!(apply_summary.source_artifact_bound);
        assert_eq!(apply_summary.decision_count, 1);
        assert_eq!(apply_summary.applied_occurrence_count, 20);
        assert_eq!(apply_summary.remaining_unknown_occurrence_count, 20);
        let reviewed_request: MusicListMotionRequest =
            serde_json::from_slice(&fs::read(&reviewed_request_path).unwrap()).unwrap();
        assert_eq!(
            reviewed_request.pairs[0]
                .first_rows
                .iter()
                .chain(reviewed_request.pairs[0].second_rows.iter())
                .filter(|annotation| matches!(annotation, PairRowAnnotation::Selected))
                .count(),
            20
        );
        assert!(
            apply_music_list_motion_review(
                &motion_artifact_path,
                &review_plan_path,
                &decisions_path,
                &reviewed_request_path,
            )
            .is_err(),
            "review application must not replace an existing request"
        );
        assert!(
            plan_music_list_motion_review(&motion_artifact_path, &review_plan_path).is_err(),
            "review planning must not replace an existing plan"
        );
        assert!(
            measure_music_list_motion(&motion_request_path, &motion_artifact_path).is_err(),
            "measurement must not replace an existing artifact"
        );
        let mut changed_artifact: MusicListMotionArtifact =
            serde_json::from_slice(&fs::read(&motion_artifact_path).unwrap()).unwrap();
        changed_artifact.pairs[0].aggregate_rgb_l1_sum += 1;
        fs::write(
            &motion_artifact_path,
            canonical_json(&changed_artifact).unwrap(),
        )
        .unwrap();
        assert!(verify_music_list_motion(&motion_artifact_path).is_err());
        let aggregate_tamper_plan = directory.path().join("aggregate-tamper-review-plan.json");
        assert!(
            plan_music_list_motion_review(&motion_artifact_path, &aggregate_tamper_plan).is_ok()
        );
        assert!(aggregate_tamper_plan.exists());

        changed_artifact.pairs[0].aggregate_rgb_l1_sum -= 1;
        changed_artifact.pairs[0].row_rgb_l1_sums[0] += 1;
        fs::write(
            &motion_artifact_path,
            canonical_json(&changed_artifact).unwrap(),
        )
        .unwrap();
        let row_tamper_plan = directory.path().join("row-tamper-review-plan.json");
        assert!(plan_music_list_motion_review(&motion_artifact_path, &row_tamper_plan).is_ok());
        assert!(row_tamper_plan.exists());

        let mut tampered = fs::read(second_crop_directory.join("list-title-03.ppm")).unwrap();
        *tampered.last_mut().unwrap() = 2;
        fs::write(second_crop_directory.join("list-title-03.ppm"), tampered).unwrap();
        let crop_tamper_plan = directory.path().join("crop-tamper-review-plan.json");
        assert!(plan_music_list_motion_review(&motion_artifact_path, &crop_tamper_plan).is_ok());
        assert!(verify_music_list_row_observation_draft(&path).is_err());
    }

    #[test]
    fn temporal_states_reject_non_adjacent_or_noncanonical_evidence() {
        let value = document(&json!({
            "state": "stationary",
            "adjacent_frame": frame(12),
            "reported_rgb_l1_sum": 1,
            "reported_compared_rgb_values": MUSIC_LIST_ROW_RGB_VALUES,
            "presentation": standard_presentation()
        }));
        let directory = tempdir().unwrap();
        let path = directory.path().join("observations.json");
        fs::write(&path, canonical(&value)).unwrap();
        assert!(inspect_music_list_row_observation_draft(&path).is_err());

        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        assert!(inspect_music_list_row_observation_draft(&path).is_err());
    }

    #[test]
    fn motion_pair_identity_is_scoped_to_extraction_and_decode_order() {
        let first = motion_pair("pair-1", &"1".repeat(64), 100, 100);
        let second = motion_pair("pair-2", &"2".repeat(64), 100, 99);
        let duplicate = motion_pair("pair-3", &"1".repeat(64), 200, 201);
        let mut ids = BTreeSet::new();
        let mut frame_pairs = BTreeSet::new();
        validate_pair_identity(&first, &mut ids, &mut frame_pairs).unwrap();
        validate_pair_identity(&second, &mut ids, &mut frame_pairs).unwrap();
        assert!(validate_pair_identity(&duplicate, &mut ids, &mut frame_pairs).is_err());
    }

    #[test]
    fn one_geometric_row_cannot_have_conflicting_annotations() {
        let mut value = document(&json!({"state": "selected"}));
        let duplicate = json!({
            "observation_id": "observation-2",
            "slot": 3,
            "frame": frame(10),
            "annotation": {"state": "clipped", "edge": "left"}
        });
        value["observations"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        let directory = tempdir().unwrap();
        let path = directory.path().join("observations.json");
        fs::write(&path, canonical(&value)).unwrap();
        assert!(inspect_music_list_row_observation_draft(&path).is_err());
    }

    #[test]
    fn bounded_reader_rejects_growth_beyond_the_document_limit() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("oversized.json");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.set_len(MAX_DOCUMENT_BYTES + 1).unwrap();
        file.seek(SeekFrom::Start(MAX_DOCUMENT_BYTES)).unwrap();
        file.write_all(b"x").unwrap();
        drop(file);
        assert!(read_bounded_regular(&path).is_err());
    }

    #[test]
    fn bounded_reader_rejects_path_replacement_after_metadata() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("draft.json");
        let replacement = directory.path().join("replacement.json");
        fs::write(&path, b"{}\n").unwrap();
        let file = File::create(&replacement).unwrap();
        file.set_len(MAX_DOCUMENT_BYTES + 1).unwrap();
        drop(file);
        let result = read_bounded_regular_after_metadata(&path, || {
            fs::rename(&replacement, &path)?;
            Ok(())
        });
        assert!(result.is_err());
    }
}
