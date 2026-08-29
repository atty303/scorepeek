use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write as _};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use scorepeek::recognition::{CanonicalLayout, MusicSelectMotionRegions, Roi, ScreenClass};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{CorpusError, ErrorContext, digest_bytes, read_bounded_regular};

const ACTIVE_SCHEMA: &str = "scorepeek-private-regression-suite-active-v1";
const SUITE_SCHEMA: &str = "scorepeek-private-regression-suite-v1";
const SESSION_SCHEMA: &str = "scorepeek-private-capture-session-v1";
const OBSERVATION_SCHEMA: &str = "scorepeek-recognition-observation-v5";
const DRAFT_SCHEMA: &str = "scorepeek-private-music-select-motion-review-draft-v1";
const SUMMARY_SCHEMA: &str = "scorepeek-private-music-select-motion-review-summary-v1";
const DECISIONS_SCHEMA: &str = "scorepeek-private-music-select-motion-review-decisions-v2";
const REVIEWED_SCHEMA: &str = "scorepeek-private-music-select-motion-reviewed-v2";
const APPLY_SUMMARY_SCHEMA: &str = "scorepeek-private-music-select-motion-review-apply-summary-v2";
const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_OBSERVATION_BYTES: u64 = 512 * 1024 * 1024;
const MAX_OBSERVATION_RECORD_BYTES: usize = 1024 * 1024;
const MAX_OBSERVATIONS: usize = 250_000;
const MAX_REVIEW_SAMPLES: usize = 10_000;
const MAX_DECODE_GAP_FRAMES: usize = 60;
const MAX_DECODE_SEGMENT_SAMPLES: usize = 256;
const MAX_VIDEO_PACKETS: usize = 250_000;
const MAX_PROBE_STDOUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_PROCESS_STDERR_BYTES: usize = 512 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const DECODE_SEGMENT_TIMEOUT: Duration = Duration::from_mins(3);
const REVIEW_PADDING_MS: u64 = 500;
const MAX_CONTIGUOUS_GAP_MS: u64 = 250;

#[derive(Debug, Serialize)]
pub struct MusicSelectMotionReviewSummary {
    schema: &'static str,
    output: PathBuf,
    draft_sha256: String,
    active_suite_sha256: String,
    session_sha256: String,
    span_count: usize,
    sample_count: usize,
    motion_pair_count: usize,
    authority: &'static str,
}

#[derive(Debug, Serialize)]
pub struct MusicSelectMotionReviewApplySummary {
    schema: &'static str,
    output: PathBuf,
    reviewed_sha256: String,
    source_draft_sha256: String,
    decision_interval_count: usize,
    reviewed_motion_pair_count: usize,
    operator_context_pair_count: usize,
    remaining_review_pair_count: usize,
    predicate_context_pair_count: usize,
    complete: bool,
    authority: &'static str,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MotionReviewDraft {
    schema: String,
    active_suite_sha256: String,
    session_sha256: String,
    source_session_id: String,
    video_sha256: String,
    capture_profile_sha256: String,
    normalizer_artifact_sha256: String,
    canonical_layout_sha256: String,
    sampling_interval_ms: u64,
    review_padding_ms: u64,
    regions: MusicSelectMotionRegions,
    spans: Vec<MotionSpan>,
    allowed_review_states: [String; 4],
    authority: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MotionSpan {
    span_id: String,
    observed_first_sequence: u64,
    observed_last_sequence: u64,
    observed_first_timestamp_ms: u64,
    observed_last_timestamp_ms: u64,
    review_first_timestamp_ms: u64,
    review_last_timestamp_ms: u64,
    review_state: ReviewState,
    samples: Vec<MotionSample>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReviewState {
    state: String,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MotionSample {
    sequence: u64,
    source_timestamp_ms: u64,
    source_frame_index: usize,
    screen: ScreenClass,
    motion_from_previous: Option<MotionEvidence>,
}

struct MotionRegionPixels {
    list_titles: Box<[u8]>,
    active_list_title: Box<[u8]>,
    central_title: Box<[u8]>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MotionEvidence {
    gap_ms: u64,
    list_titles: RegionMotion,
    active_list_title: RegionMotion,
    central_title: RegionMotion,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegionMotion {
    rgb_l1: u64,
    changed_pixels: u64,
    compared_pixels: u64,
    normalized_l1_ppm: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MotionReviewDecisions {
    schema: String,
    source_draft_sha256: String,
    decisions: Vec<MotionReviewDecision>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MotionReviewDecision {
    span_id: String,
    first_sequence: u64,
    last_sequence: u64,
    state: OperatorReviewState,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum OperatorReviewState {
    Stationary,
    Scrolling,
    SelectionChange,
    ScreenContext,
}

impl OperatorReviewState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Stationary => "stationary",
            Self::Scrolling => "scrolling",
            Self::SelectionChange => "selection_change",
            Self::ScreenContext => "screen_context",
        }
    }
}

#[derive(Debug, Serialize)]
struct ReviewedMotionSet {
    schema: &'static str,
    source_draft_sha256: String,
    active_suite_sha256: String,
    session_sha256: String,
    source_session_id: String,
    video_sha256: String,
    capture_profile_sha256: String,
    normalizer_artifact_sha256: String,
    canonical_layout_sha256: String,
    sampling_interval_ms: u64,
    review_padding_ms: u64,
    regions: MusicSelectMotionRegions,
    spans: Vec<ReviewedMotionSpan>,
    completeness: ReviewCompleteness,
    authority: &'static str,
}

#[derive(Debug, Serialize)]
struct ReviewedMotionSpan {
    span_id: String,
    observed_first_sequence: u64,
    observed_last_sequence: u64,
    observed_first_timestamp_ms: u64,
    observed_last_timestamp_ms: u64,
    review_first_timestamp_ms: u64,
    review_last_timestamp_ms: u64,
    pairs: Vec<ReviewedMotionPair>,
}

#[derive(Debug, Serialize)]
struct ReviewedMotionPair {
    previous_sequence: u64,
    sequence: u64,
    previous_timestamp_ms: u64,
    source_timestamp_ms: u64,
    previous_screen: ScreenClass,
    screen: ScreenClass,
    source_frame_index: usize,
    motion: MotionEvidence,
    review_state: ReviewState,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct ReviewCompleteness {
    decision_interval_count: usize,
    reviewed_motion_pair_count: usize,
    operator_context_pair_count: usize,
    remaining_review_pair_count: usize,
    predicate_context_pair_count: usize,
    complete: bool,
}

struct AppliedMotionReview {
    spans: Vec<ReviewedMotionSpan>,
    reviewed_motion_pair_count: usize,
    operator_context_pair_count: usize,
    remaining_review_pair_count: usize,
    predicate_context_pair_count: usize,
}

#[derive(Debug, Deserialize)]
struct ActiveSuite {
    schema: String,
    generation_sha256: String,
}

#[derive(Debug, Deserialize)]
struct RegressionSuite {
    schema: String,
    entries: Vec<SuiteEntry>,
}

#[derive(Debug, Deserialize)]
struct SuiteEntry {
    session_sha256: String,
}

#[derive(Debug, Deserialize)]
struct CaptureSession {
    schema: String,
    source_kind: String,
    source_session_id: String,
    profile_sha256: String,
    recognition_interval_ms: u64,
    artifacts: Vec<CorpusArtifact>,
}

#[derive(Debug, Deserialize)]
struct CaptureRun {
    schema: String,
    run_id: String,
    binding: CaptureRunBinding,
    source: CaptureRunSource,
}

#[derive(Debug, Deserialize)]
#[allow(clippy::struct_field_names)]
struct CaptureRunBinding {
    capture_profile_sha256: String,
    normalizer_sha256: String,
    canonical_layout_sha256: String,
}

#[derive(Debug, Deserialize)]
struct CaptureRunSource {
    kind: String,
    video_sha256: String,
}

#[derive(Debug, Deserialize)]
struct CorpusArtifact {
    source_path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Clone, Copy, Debug)]
struct ObservationRecord {
    sequence: u64,
    timestamp_ms: u64,
    screen: ScreenClass,
}

#[derive(Clone, Copy, Debug)]
struct ReviewWindow {
    observed_first_sequence: u64,
    observed_last_sequence: u64,
    observed_first_timestamp_ms: u64,
    observed_last_timestamp_ms: u64,
    review_first_timestamp_ms: u64,
    review_last_timestamp_ms: u64,
}

#[derive(Debug, Deserialize)]
struct VideoProbe {
    streams: Vec<VideoStream>,
    packets: Vec<VideoPacket>,
}

#[derive(Debug, Deserialize)]
struct VideoStream {
    codec_name: String,
    width: u32,
    height: u32,
    has_b_frames: u32,
}

#[derive(Debug, Deserialize)]
struct VideoPacket {
    pts_time: Option<String>,
    flags: String,
}

struct VideoInventory {
    timestamps: Vec<u64>,
    keyframes: Vec<usize>,
}

#[derive(Debug)]
struct BoundedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VideoIdentity {
    device: u64,
    inode: u64,
}

struct ProcessCompletion {
    status: Option<std::process::ExitStatus>,
    cleanup: String,
    failure: Option<String>,
}

/// Creates an immutable, unlabeled review draft from active-suite observations and their bound
/// source video. Motion is measured independently in the list, active-row, and central-title
/// regions; no measurement is converted into a stationary or scrolling label.
///
/// # Errors
/// Returns an error when any selected corpus object or video binding differs, the deterministic
/// 10 Hz replay cannot reproduce the retained observation timestamps, or the create-only output
/// cannot be published.
#[allow(clippy::too_many_lines)]
pub fn plan_music_select_motion_review(
    store: &Path,
    session_sha256: &str,
    video: &Path,
    output: &Path,
) -> Result<MusicSelectMotionReviewSummary, CorpusError> {
    if !store.is_absolute()
        || !video.is_absolute()
        || !video.is_file()
        || !output.is_absolute()
        || output.exists()
        || !valid_sha256(session_sha256)
    {
        return invalid("music-select motion review requires absolute inputs and absent output");
    }
    let (active, session) = load_bound_session(store, session_sha256)?;
    if session.source_kind != "video_replay" || session.recognition_interval_ms != 100 {
        return invalid("music-select motion review requires a 10 Hz video-replay session");
    }
    let mut video_file = File::open(video)?;
    let video_metadata = video_file.metadata()?;
    if !video_metadata.is_file() {
        return invalid("music-select motion review video is not a regular file");
    }
    let video_identity = VideoIdentity {
        device: video_metadata.dev(),
        inode: video_metadata.ino(),
    };
    let video_sha256 = digest_open_file(&mut video_file)?;
    if session.source_session_id != format!("video-{}", &video_sha256[..24]) {
        return invalid("music-select motion review video identity differs");
    }
    let profile_artifact = bound_artifact(&session, "capture/profile.json")?;
    let profile_bytes = read_bound_object(store, profile_artifact, MAX_DOCUMENT_BYTES as u64)?;
    let profile = scorepeek::capture::GamescopeProfileBinding::parse(
        &profile_bytes,
        &profile_artifact.sha256,
    )
    .map_err(|_| CorpusError::InvalidReplay("motion review profile is invalid".to_owned()))?;
    if profile.capture_profile_sha256() != session.profile_sha256 {
        return invalid("music-select motion review profile binding differs");
    }
    let run_artifact = bound_artifact(&session, "capture/run.json")?;
    let run: CaptureRun = serde_json::from_slice(&read_bound_object(
        store,
        run_artifact,
        MAX_DOCUMENT_BYTES as u64,
    )?)?;
    if run.schema != "scorepeek-private-diagnostic-capture-start-v3"
        || run.run_id != session.source_session_id
        || run.source.kind != "video_replay"
        || run.source.video_sha256 != video_sha256
        || run.binding.capture_profile_sha256 != session.profile_sha256
        || run.binding.normalizer_sha256 != profile.normalizer_artifact_sha256()
        || run.binding.canonical_layout_sha256 != CanonicalLayout::sha256()
    {
        return invalid("music-select motion review run binding differs");
    }
    let observations = read_observations(store, &session)?;
    let windows = review_windows(&observations);
    if windows.is_empty() {
        return invalid("selected session has no music-select review span");
    }
    let regions = CanonicalLayout::music_select_motion_regions()
        .map_err(|_| CorpusError::InvalidReplay("motion review layout is invalid".to_owned()))?;
    let mut spans = windows
        .iter()
        .enumerate()
        .map(|(index, window)| MotionSpan {
            span_id: format!("music-select-span-{:04}", index + 1),
            observed_first_sequence: window.observed_first_sequence,
            observed_last_sequence: window.observed_last_sequence,
            observed_first_timestamp_ms: window.observed_first_timestamp_ms,
            observed_last_timestamp_ms: window.observed_last_timestamp_ms,
            review_first_timestamp_ms: window.review_first_timestamp_ms,
            review_last_timestamp_ms: window.review_last_timestamp_ms,
            review_state: ReviewState {
                state: "unknown".to_owned(),
                reason: "operator_review_required".to_owned(),
            },
            samples: Vec::new(),
        })
        .collect::<Vec<_>>();
    decode_motion_samples(
        &video_file,
        &profile,
        &observations,
        &windows,
        &regions,
        &mut spans,
    )?;
    verify_video_unchanged(video, video_identity, &video_sha256)?;
    if spans.iter().any(|span| span.samples.is_empty()) {
        return invalid("music-select motion review span has no decoded sample");
    }
    let draft = MotionReviewDraft {
        schema: DRAFT_SCHEMA.to_owned(),
        active_suite_sha256: active.generation_sha256.clone(),
        session_sha256: session_sha256.to_owned(),
        source_session_id: session.source_session_id,
        video_sha256,
        capture_profile_sha256: session.profile_sha256,
        normalizer_artifact_sha256: profile.normalizer_artifact_sha256().to_owned(),
        canonical_layout_sha256: CanonicalLayout::sha256(),
        sampling_interval_ms: 100,
        review_padding_ms: REVIEW_PADDING_MS,
        regions,
        spans,
        allowed_review_states: [
            "stationary".to_owned(),
            "scrolling".to_owned(),
            "selection_change".to_owned(),
            "unknown".to_owned(),
        ],
        authority: "operator_review_required".to_owned(),
    };
    let mut bytes = serde_json::to_vec(&draft)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return invalid("music-select motion review draft exceeds its bound");
    }
    let draft_sha256 = digest_bytes(&bytes);
    publish_create_only(output, &bytes)?;
    let sample_count = draft.spans.iter().map(|span| span.samples.len()).sum();
    let motion_pair_count = draft
        .spans
        .iter()
        .flat_map(|span| &span.samples)
        .filter(|sample| sample.motion_from_previous.is_some())
        .count();
    Ok(MusicSelectMotionReviewSummary {
        schema: SUMMARY_SCHEMA,
        output: output.to_owned(),
        draft_sha256,
        active_suite_sha256: active.generation_sha256,
        session_sha256: session_sha256.to_owned(),
        span_count: draft.spans.len(),
        sample_count,
        motion_pair_count,
        authority: "operator_review_required",
    })
}

/// Applies explicit operator-reviewed sequence intervals to one immutable motion draft.
///
/// Decisions bind adjacent pairs whose previous and current predicates are both `music_select`.
/// An operator may classify such a false-positive pair as screen context instead of motion.
/// Omitted eligible pairs remain typed unknown; predicate-context pairs cannot receive a decision.
///
/// # Errors
/// Returns an error when the draft or decisions are non-canonical, their digest binding differs,
/// an interval is empty, overlapping, unbounded, or names a predicate-context pair, or the create-only
/// output cannot be published.
pub fn apply_music_select_motion_review(
    draft_path: &Path,
    decisions_path: &Path,
    output_path: &Path,
) -> Result<MusicSelectMotionReviewApplySummary, CorpusError> {
    if !draft_path.is_absolute() || !decisions_path.is_absolute() || !output_path.is_absolute() {
        return invalid("music-select motion review apply paths must be absolute");
    }
    let draft_bytes = read_bounded_regular(draft_path, MAX_DOCUMENT_BYTES, ErrorContext::Replay)?;
    let draft: MotionReviewDraft = serde_json::from_slice(&draft_bytes)?;
    validate_motion_review_draft(&draft, &draft_bytes)?;
    let source_draft_sha256 = digest_bytes(&draft_bytes);

    let decisions = read_motion_review_decisions(decisions_path, &source_draft_sha256)?;
    let labels = expand_motion_review_decisions(&draft, &decisions)?;
    let applied = apply_pair_labels(&draft, labels)?;
    let complete = applied.remaining_review_pair_count == 0;
    let completeness = ReviewCompleteness {
        decision_interval_count: decisions.decisions.len(),
        reviewed_motion_pair_count: applied.reviewed_motion_pair_count,
        operator_context_pair_count: applied.operator_context_pair_count,
        remaining_review_pair_count: applied.remaining_review_pair_count,
        predicate_context_pair_count: applied.predicate_context_pair_count,
        complete,
    };
    let reviewed = ReviewedMotionSet {
        schema: REVIEWED_SCHEMA,
        source_draft_sha256: source_draft_sha256.clone(),
        active_suite_sha256: draft.active_suite_sha256,
        session_sha256: draft.session_sha256,
        source_session_id: draft.source_session_id,
        video_sha256: draft.video_sha256,
        capture_profile_sha256: draft.capture_profile_sha256,
        normalizer_artifact_sha256: draft.normalizer_artifact_sha256,
        canonical_layout_sha256: draft.canonical_layout_sha256,
        sampling_interval_ms: draft.sampling_interval_ms,
        review_padding_ms: draft.review_padding_ms,
        regions: draft.regions,
        spans: applied.spans,
        completeness,
        authority: "operator_review",
    };
    let bytes = canonical_line(&reviewed)?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return invalid("music-select motion reviewed set exceeds its bound");
    }
    let reviewed_sha256 = digest_bytes(&bytes);
    publish_create_only(output_path, &bytes)?;
    Ok(MusicSelectMotionReviewApplySummary {
        schema: APPLY_SUMMARY_SCHEMA,
        output: output_path.to_owned(),
        reviewed_sha256,
        source_draft_sha256,
        decision_interval_count: decisions.decisions.len(),
        reviewed_motion_pair_count: completeness.reviewed_motion_pair_count,
        operator_context_pair_count: completeness.operator_context_pair_count,
        remaining_review_pair_count: completeness.remaining_review_pair_count,
        predicate_context_pair_count: completeness.predicate_context_pair_count,
        complete,
        authority: "operator_review",
    })
}

fn read_motion_review_decisions(
    path: &Path,
    source_draft_sha256: &str,
) -> Result<MotionReviewDecisions, CorpusError> {
    let bytes = read_bounded_regular(path, MAX_DOCUMENT_BYTES, ErrorContext::Replay)?;
    let decisions: MotionReviewDecisions = serde_json::from_slice(&bytes)?;
    if canonical_line(&decisions)? != bytes
        || decisions.schema != DECISIONS_SCHEMA
        || decisions.source_draft_sha256 != source_draft_sha256
        || decisions.decisions.len() > MAX_REVIEW_SAMPLES
    {
        return invalid("music-select motion decisions are not canonical and draft-bound");
    }
    Ok(decisions)
}

fn expand_motion_review_decisions(
    draft: &MotionReviewDraft,
    decisions: &MotionReviewDecisions,
) -> Result<BTreeMap<(String, u64), OperatorReviewState>, CorpusError> {
    let eligible_pairs = draft
        .spans
        .iter()
        .flat_map(|span| {
            span.samples
                .windows(2)
                .filter(|samples| {
                    samples[0].screen == ScreenClass::MusicSelect
                        && samples[1].screen == ScreenClass::MusicSelect
                })
                .map(|samples| (span.span_id.clone(), samples[1].sequence))
        })
        .collect::<BTreeSet<_>>();
    let mut labels = BTreeMap::new();
    for decision in &decisions.decisions {
        let interval_len = decision
            .last_sequence
            .checked_sub(decision.first_sequence)
            .and_then(|difference| difference.checked_add(1))
            .and_then(|count| usize::try_from(count).ok())
            .filter(|count| *count <= MAX_REVIEW_SAMPLES)
            .ok_or_else(|| {
                CorpusError::InvalidReplay(
                    "music-select motion decision interval is invalid or unbounded".to_owned(),
                )
            })?;
        if decision.span_id.is_empty() {
            return invalid("music-select motion decision span is empty");
        }
        for offset in 0..interval_len {
            let sequence = decision.first_sequence + u64::try_from(offset).unwrap_or(u64::MAX);
            let key = (decision.span_id.clone(), sequence);
            if !eligible_pairs.contains(&key) {
                return invalid(
                    "music-select motion decision names a predicate-context or absent pair",
                );
            }
            if labels.insert(key, decision.state).is_some() {
                return invalid("music-select motion decision intervals overlap");
            }
        }
    }
    Ok(labels)
}

fn apply_pair_labels(
    draft: &MotionReviewDraft,
    mut labels: BTreeMap<(String, u64), OperatorReviewState>,
) -> Result<AppliedMotionReview, CorpusError> {
    let mut result = AppliedMotionReview {
        spans: Vec::with_capacity(draft.spans.len()),
        reviewed_motion_pair_count: 0,
        operator_context_pair_count: 0,
        remaining_review_pair_count: 0,
        predicate_context_pair_count: 0,
    };
    for span in &draft.spans {
        let mut pairs = Vec::with_capacity(span.samples.len().saturating_sub(1));
        for samples in span.samples.windows(2) {
            pairs.push(review_motion_pair(span, samples, &mut labels, &mut result)?);
        }
        result.spans.push(ReviewedMotionSpan {
            span_id: span.span_id.clone(),
            observed_first_sequence: span.observed_first_sequence,
            observed_last_sequence: span.observed_last_sequence,
            observed_first_timestamp_ms: span.observed_first_timestamp_ms,
            observed_last_timestamp_ms: span.observed_last_timestamp_ms,
            review_first_timestamp_ms: span.review_first_timestamp_ms,
            review_last_timestamp_ms: span.review_last_timestamp_ms,
            pairs,
        });
    }
    if !labels.is_empty() {
        return invalid("music-select motion decisions were not consumed exactly once");
    }
    Ok(result)
}

fn review_motion_pair(
    span: &MotionSpan,
    samples: &[MotionSample],
    labels: &mut BTreeMap<(String, u64), OperatorReviewState>,
    result: &mut AppliedMotionReview,
) -> Result<ReviewedMotionPair, CorpusError> {
    let previous = &samples[0];
    let current = &samples[1];
    let motion = current.motion_from_previous.clone().ok_or_else(|| {
        CorpusError::InvalidReplay(
            "music-select motion draft pair lacks motion evidence".to_owned(),
        )
    })?;
    let eligible =
        previous.screen == ScreenClass::MusicSelect && current.screen == ScreenClass::MusicSelect;
    let review_state = if eligible {
        if let Some(state) = labels.remove(&(span.span_id.clone(), current.sequence)) {
            if matches!(state, OperatorReviewState::ScreenContext) {
                result.operator_context_pair_count += 1;
                ReviewState {
                    state: "unknown".to_owned(),
                    reason: "operator_screen_context".to_owned(),
                }
            } else {
                result.reviewed_motion_pair_count += 1;
                ReviewState {
                    state: state.as_str().to_owned(),
                    reason: "operator_reviewed".to_owned(),
                }
            }
        } else {
            result.remaining_review_pair_count += 1;
            ReviewState {
                state: "unknown".to_owned(),
                reason: "operator_review_required".to_owned(),
            }
        }
    } else {
        result.predicate_context_pair_count += 1;
        ReviewState {
            state: "unknown".to_owned(),
            reason: "predicate_screen_context".to_owned(),
        }
    };
    Ok(ReviewedMotionPair {
        previous_sequence: previous.sequence,
        sequence: current.sequence,
        previous_timestamp_ms: previous.source_timestamp_ms,
        source_timestamp_ms: current.source_timestamp_ms,
        previous_screen: previous.screen,
        screen: current.screen,
        source_frame_index: current.source_frame_index,
        motion,
        review_state,
    })
}

fn validate_motion_review_draft(
    draft: &MotionReviewDraft,
    bytes: &[u8],
) -> Result<(), CorpusError> {
    let expected_states = ["stationary", "scrolling", "selection_change", "unknown"];
    if canonical_line(draft)? != bytes
        || draft.schema != DRAFT_SCHEMA
        || draft.allowed_review_states.each_ref().map(String::as_str) != expected_states
        || draft.authority != "operator_review_required"
        || draft.sampling_interval_ms != 100
        || draft.review_padding_ms != REVIEW_PADDING_MS
        || draft.spans.is_empty()
    {
        return invalid("music-select motion draft is not canonical and versioned");
    }
    let mut span_ids = BTreeSet::new();
    let mut sample_count = 0_usize;
    for span in &draft.spans {
        if !span_ids.insert(&span.span_id)
            || span.samples.is_empty()
            || span.review_state.state != "unknown"
            || span.review_state.reason != "operator_review_required"
            || span.samples[0].motion_from_previous.is_some()
            || span.samples[1..]
                .iter()
                .any(|sample| sample.motion_from_previous.is_none())
            || span.samples.windows(2).any(|samples| {
                samples[0].sequence >= samples[1].sequence
                    || samples[0].source_timestamp_ms >= samples[1].source_timestamp_ms
                    || samples[0].source_frame_index > samples[1].source_frame_index
            })
        {
            return invalid("music-select motion draft span is invalid");
        }
        sample_count = sample_count.saturating_add(span.samples.len());
    }
    if sample_count > MAX_REVIEW_SAMPLES {
        return invalid("music-select motion draft sample count exceeds its bound");
    }
    Ok(())
}

fn canonical_line(value: &impl Serialize) -> Result<Vec<u8>, CorpusError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn load_bound_session(
    store: &Path,
    session_sha256: &str,
) -> Result<(ActiveSuite, CaptureSession), CorpusError> {
    let active: ActiveSuite = read_json(&store.join("active-suite.json"))?;
    if active.schema != ACTIVE_SCHEMA || !valid_sha256(&active.generation_sha256) {
        return invalid("active motion-review suite is invalid");
    }
    let suite: RegressionSuite = read_bound_json(
        &store
            .join("suites")
            .join(format!("{}.json", active.generation_sha256)),
        &active.generation_sha256,
    )?;
    if suite.schema != SUITE_SCHEMA
        || !suite
            .entries
            .iter()
            .any(|entry| entry.session_sha256 == session_sha256)
    {
        return invalid("motion-review session is not in the active suite");
    }
    let session: CaptureSession = read_bound_json(
        &store
            .join("sessions")
            .join(format!("{session_sha256}.json")),
        session_sha256,
    )?;
    if session.schema != SESSION_SCHEMA {
        return invalid("motion-review session schema differs");
    }
    Ok((active, session))
}

fn read_observations(
    store: &Path,
    session: &CaptureSession,
) -> Result<Vec<ObservationRecord>, CorpusError> {
    let artifact = bound_artifact(session, "recognition/observations.ndjson")?;
    let bytes = read_bound_object(store, artifact, MAX_OBSERVATION_BYTES)?;
    let mut records = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.len() > MAX_OBSERVATION_RECORD_BYTES || line.last() != Some(&b'\n') {
            return invalid("motion-review observation record exceeds its bound");
        }
        if records.len() >= MAX_OBSERVATIONS {
            return invalid("motion-review observation count exceeds its bound");
        }
        let value: Value = serde_json::from_slice(line)?;
        if value["schema"] != OBSERVATION_SCHEMA {
            return invalid("motion-review observation schema differs");
        }
        let sequence = value["tick_sequence"].as_u64().ok_or_else(|| {
            CorpusError::InvalidReplay("motion-review sequence is invalid".to_owned())
        })?;
        let timestamp_ms = value["source_timestamp_ms"].as_u64().ok_or_else(|| {
            CorpusError::InvalidReplay("motion-review timestamp is invalid".to_owned())
        })?;
        let screen = match value["screen"]
            .as_str()
            .or_else(|| value.pointer("/decision/screen").and_then(Value::as_str))
            .or_else(|| value.pointer("/fields/screen").and_then(Value::as_str))
        {
            Some("result") => ScreenClass::Result,
            Some("music_select") => ScreenClass::MusicSelect,
            Some("unknown") => ScreenClass::Unknown,
            _ => return invalid("motion-review screen is invalid"),
        };
        records.push(ObservationRecord {
            sequence,
            timestamp_ms,
            screen,
        });
    }
    if records.is_empty()
        || records.windows(2).any(|pair| {
            pair[0].sequence >= pair[1].sequence || pair[0].timestamp_ms >= pair[1].timestamp_ms
        })
    {
        return invalid("motion-review observations are unordered");
    }
    Ok(records)
}

fn review_windows(records: &[ObservationRecord]) -> Vec<ReviewWindow> {
    let mut windows = Vec::<ReviewWindow>::new();
    for record in records
        .iter()
        .filter(|record| record.screen == ScreenClass::MusicSelect)
    {
        let starts_new = windows.last().is_none_or(|window| {
            record
                .timestamp_ms
                .saturating_sub(window.observed_last_timestamp_ms)
                > MAX_CONTIGUOUS_GAP_MS
        });
        if starts_new {
            windows.push(ReviewWindow {
                observed_first_sequence: record.sequence,
                observed_last_sequence: record.sequence,
                observed_first_timestamp_ms: record.timestamp_ms,
                observed_last_timestamp_ms: record.timestamp_ms,
                review_first_timestamp_ms: record.timestamp_ms.saturating_sub(REVIEW_PADDING_MS),
                review_last_timestamp_ms: record.timestamp_ms.saturating_add(REVIEW_PADDING_MS),
            });
        } else if let Some(window) = windows.last_mut() {
            window.observed_last_sequence = record.sequence;
            window.observed_last_timestamp_ms = record.timestamp_ms;
            window.review_last_timestamp_ms = record.timestamp_ms.saturating_add(REVIEW_PADDING_MS);
        }
    }
    let mut merged = Vec::<ReviewWindow>::new();
    for window in windows {
        if let Some(previous) = merged.last_mut()
            && window.review_first_timestamp_ms <= previous.review_last_timestamp_ms
        {
            previous.observed_last_sequence = window.observed_last_sequence;
            previous.observed_last_timestamp_ms = window.observed_last_timestamp_ms;
            previous.review_last_timestamp_ms = window.review_last_timestamp_ms;
        } else {
            merged.push(window);
        }
    }
    merged
}

fn decode_motion_samples(
    video: &File,
    profile: &scorepeek::capture::GamescopeProfileBinding,
    observations: &[ObservationRecord],
    windows: &[ReviewWindow],
    regions: &MusicSelectMotionRegions,
    spans: &mut [MotionSpan],
) -> Result<(), CorpusError> {
    let mut targets = BTreeMap::<u64, (usize, ObservationRecord)>::new();
    for (span_index, window) in windows.iter().enumerate() {
        for record in observations.iter().filter(|record| {
            window.review_first_timestamp_ms <= record.timestamp_ms
                && record.timestamp_ms <= window.review_last_timestamp_ms
        }) {
            targets.insert(record.sequence, (span_index, *record));
        }
    }
    let maximum_timestamp_ms = observations
        .last()
        .and_then(|record| record.timestamp_ms.checked_add(1_000))
        .ok_or_else(|| {
            CorpusError::InvalidReplay("motion-review timestamp bound overflows".to_owned())
        })?;
    let inventory = probe_video(
        video,
        profile.observed_width(),
        profile.observed_height(),
        maximum_timestamp_ms,
    )?;
    let selected = selected_frame_targets(&inventory.timestamps, &targets)?;
    if selected.values().map(Vec::len).sum::<usize>() > MAX_REVIEW_SAMPLES {
        return invalid("music-select motion review sample count exceeds its bound");
    }
    let frame_bytes = usize::try_from(profile.observed_width())
        .ok()
        .and_then(|width| {
            usize::try_from(profile.observed_height())
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| {
            CorpusError::InvalidReplay("motion-review frame size overflows".to_owned())
        })?;
    let stride = profile
        .observed_width()
        .checked_mul(4)
        .ok_or_else(|| CorpusError::InvalidReplay("motion-review stride overflows".to_owned()))?;
    let mut previous = None::<(usize, u64, MotionRegionPixels)>;
    let mut segments = Vec::<Vec<(usize, Vec<(usize, ObservationRecord)>)>>::new();
    for item in selected {
        let starts_new = segments
            .last()
            .and_then(|segment| segment.last())
            .is_none_or(|(previous_index, _)| {
                item.0.saturating_sub(*previous_index) > MAX_DECODE_GAP_FRAMES
            })
            || segments
                .last()
                .is_some_and(|segment| segment.len() >= MAX_DECODE_SEGMENT_SAMPLES);
        if starts_new {
            segments.push(Vec::new());
        }
        segments.last_mut().expect("segment exists").push(item);
    }
    for segment in segments {
        decode_motion_segment(
            video,
            profile,
            &inventory,
            regions,
            spans,
            &mut previous,
            frame_bytes,
            stride,
            segment,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn decode_motion_segment(
    video: &File,
    profile: &scorepeek::capture::GamescopeProfileBinding,
    inventory: &VideoInventory,
    regions: &MusicSelectMotionRegions,
    spans: &mut [MotionSpan],
    previous: &mut Option<(usize, u64, MotionRegionPixels)>,
    frame_bytes: usize,
    stride: u32,
    segment: Vec<(usize, Vec<(usize, ObservationRecord)>)>,
) -> Result<(), CorpusError> {
    let first_index = segment[0].0;
    let keyframe_index = inventory
        .keyframes
        .iter()
        .copied()
        .take_while(|index| *index <= first_index)
        .last()
        .ok_or_else(|| CorpusError::InvalidReplay("motion-review keyframe is absent".to_owned()))?;
    let select = select_expression(segment.iter().map(|(index, _)| index - keyframe_index));
    let seek_ms = inventory.timestamps[keyframe_index];
    let seek = format!("{}.{:03}", seek_ms / 1_000, seek_ms % 1_000);
    let expected_pts = segment
        .iter()
        .map(|(index, _)| inventory.timestamps[*index])
        .collect::<Vec<_>>();
    let ffmpeg = super::media::find_executable("ffmpeg")?;
    let expected_frames = expected_pts.len();
    let started = Instant::now();
    let mut child = Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-nostdin",
            "-loglevel",
            "info",
            "-copyts",
            "-ss",
            &seek,
            "-i",
        ])
        .arg("/proc/self/fd/0")
        .args([
            "-map",
            "0:v:0",
            "-vf",
            &format!("select={select},showinfo"),
            "-fps_mode",
            "passthrough",
            "-frames:v",
            &expected_frames.to_string(),
            "-pix_fmt",
            "bgr0",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .stdin(Stdio::from(reopen_video(video)?))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let Some(stdout) = child.stdout.take() else {
        let completion = finish_decoder(&mut child, true, started, DECODE_SEGMENT_TIMEOUT);
        return invalid(&format!(
            "motion-review decoder stdout is unavailable (status {:?}; cleanup {}; failure {:?})",
            completion.status, completion.cleanup, completion.failure
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        drop(stdout);
        let completion = finish_decoder(&mut child, true, started, DECODE_SEGMENT_TIMEOUT);
        return invalid(&format!(
            "motion-review decoder stderr is unavailable (status {:?}; cleanup {}; failure {:?})",
            completion.status, completion.cleanup, completion.failure
        ));
    };
    let stderr_reader =
        thread::spawn(move || read_bounded_stream(stderr, MAX_PROCESS_STDERR_BYTES));
    let (frame_sender, frame_receiver) = sync_channel(1);
    let frame_reader = thread::spawn(move || {
        let mut stdout = stdout;
        for ordinal in 0..expected_frames {
            let mut frame = vec![0_u8; frame_bytes].into_boxed_slice();
            if let Err(error) = stdout.read_exact(&mut frame) {
                let _ = frame_sender.send(Err(format!(
                    "selected frame {ordinal} of {expected_frames} read failed: {error}"
                )));
                return;
            }
            if frame_sender.send(Ok(Some(frame))).is_err() {
                return;
            }
        }
        let mut extra = [0_u8; 1];
        let result = stdout
            .read(&mut extra)
            .map_err(|error| format!("decoder completion read failed: {error}"))
            .and_then(|read| {
                if read == 0 {
                    Ok(None)
                } else {
                    Err("decoder returned extra frames".to_owned())
                }
            });
        let _ = frame_sender.send(result);
    });
    let processing =
        (|| -> Result<(), CorpusError> {
            for (ordinal, (frame_index, records)) in segment.into_iter().enumerate() {
                let raw = receive_frame(&frame_receiver, started, DECODE_SEGMENT_TIMEOUT)?
                .ok_or_else(|| CorpusError::InvalidReplay(format!(
                    "decoder stopped before selected frame {frame_index} at sample {ordinal}"
                )))?;
                let pixels = MotionRegionPixels {
                    list_titles: normalize_region(profile, &raw, stride, regions.list_titles)?,
                    active_list_title: normalize_region(
                        profile,
                        &raw,
                        stride,
                        regions.active_list_title,
                    )?,
                    central_title: normalize_region(profile, &raw, stride, regions.central_title)?,
                };
                for (span_index, record) in records {
                    let timestamp_ms = inventory.timestamps[frame_index];
                    if timestamp_ms != record.timestamp_ms {
                        return invalid(
                            "motion-review replay timestamp differs from retained observation",
                        );
                    }
                    if previous
                        .as_ref()
                        .is_some_and(|(previous_span, _, _)| *previous_span != span_index)
                    {
                        *previous = None;
                    }
                    let motion_from_previous =
                        previous
                            .as_ref()
                            .map(|(_, previous_ms, previous_pixels)| MotionEvidence {
                                gap_ms: timestamp_ms.saturating_sub(*previous_ms),
                                list_titles: region_motion_packed(
                                    &previous_pixels.list_titles,
                                    &pixels.list_titles,
                                    regions.list_titles,
                                ),
                                active_list_title: region_motion_packed(
                                    &previous_pixels.active_list_title,
                                    &pixels.active_list_title,
                                    regions.active_list_title,
                                ),
                                central_title: region_motion_packed(
                                    &previous_pixels.central_title,
                                    &pixels.central_title,
                                    regions.central_title,
                                ),
                            });
                    *previous = Some((
                        span_index,
                        timestamp_ms,
                        MotionRegionPixels {
                            list_titles: pixels.list_titles.clone(),
                            active_list_title: pixels.active_list_title.clone(),
                            central_title: pixels.central_title.clone(),
                        },
                    ));
                    spans[span_index].samples.push(MotionSample {
                        sequence: record.sequence,
                        source_timestamp_ms: timestamp_ms,
                        source_frame_index: frame_index,
                        screen: record.screen,
                        motion_from_previous,
                    });
                }
            }
            if receive_frame(&frame_receiver, started, DECODE_SEGMENT_TIMEOUT)?.is_some() {
                return invalid("motion-review decoder returned extra frame payload");
            }
            Ok(())
        })();
    drop(frame_receiver);
    let completion = finish_decoder(
        &mut child,
        processing.is_err(),
        started,
        DECODE_SEGMENT_TIMEOUT,
    );
    let frame_result = frame_reader
        .join()
        .map_err(|_| CorpusError::InvalidReplay("decoder frame reader panicked".to_owned()));
    let stderr_result = join_stream_reader(stderr_reader);
    let stderr_detail = stderr_result
        .as_ref()
        .map_or_else(ToString::to_string, process_detail);
    if let Some(failure) = completion.failure {
        return invalid(&format!(
            "motion-review decoder supervision failed ({failure}; cleanup {}; stderr {stderr_detail})",
            completion.cleanup
        ));
    }
    frame_result?;
    let stderr = stderr_result?;
    let status = completion.status.ok_or_else(|| {
        CorpusError::InvalidReplay("motion-review decoder status is unavailable".to_owned())
    })?;
    if let Err(error) = processing {
        return invalid(&format!(
            "motion-review video decode failed during processing ({error}; status {status}; cleanup {}; stderr {})",
            completion.cleanup,
            process_detail(&stderr)
        ));
    }
    if !status.success() || stderr.truncated {
        return invalid(&format!(
            "motion-review video decode failed (status {status}; stderr_truncated {}; stderr {})",
            stderr.truncated,
            process_detail(&stderr)
        ));
    }
    let decoded_pts = parse_showinfo_pts(&stderr.bytes)?;
    if decoded_pts != expected_pts {
        return invalid("motion-review decoded PTS differs from selected packet PTS");
    }
    Ok(())
}

fn finish_decoder(
    child: &mut std::process::Child,
    terminate: bool,
    started: Instant,
    timeout: Duration,
) -> ProcessCompletion {
    let mut cleanup = "natural_exit".to_owned();
    let mut failure = None;
    if terminate {
        match child.try_wait() {
            Ok(Some(status)) => {
                return ProcessCompletion {
                    status: Some(status),
                    cleanup,
                    failure,
                };
            }
            Ok(None) => match child.kill() {
                Ok(()) => "killed_after_processing_failure".clone_into(&mut cleanup),
                Err(error) => cleanup = format!("kill_failed:{error}"),
            },
            Err(error) => {
                failure = Some(format!("try_wait_failed:{error}"));
                let _ = child.kill();
            }
        }
    }
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return ProcessCompletion {
                    status: Some(status),
                    cleanup,
                    failure,
                };
            }
            Ok(None) => {}
            Err(error) => {
                failure.get_or_insert_with(|| format!("try_wait_failed:{error}"));
                let _ = child.kill();
                return ProcessCompletion {
                    status: child.wait().ok(),
                    cleanup: "killed_after_wait_failure".to_owned(),
                    failure,
                };
            }
        }
        if started.elapsed() >= timeout {
            failure.get_or_insert_with(|| "execution_timeout".to_owned());
            let kill = child.kill();
            return ProcessCompletion {
                status: child.wait().ok(),
                cleanup: kill.map_or_else(
                    |error| format!("timeout_kill_failed:{error}"),
                    |()| "killed_after_timeout".to_owned(),
                ),
                failure,
            };
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn receive_frame(
    receiver: &Receiver<Result<Option<Box<[u8]>>, String>>,
    started: Instant,
    timeout: Duration,
) -> Result<Option<Box<[u8]>>, CorpusError> {
    let remaining = timeout.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        return invalid("motion-review decoder exceeded its timeout");
    }
    match receiver.recv_timeout(remaining) {
        Ok(Ok(frame)) => Ok(frame),
        Ok(Err(error)) => invalid(&format!("motion-review decoder output failed: {error}")),
        Err(RecvTimeoutError::Timeout) => invalid("motion-review decoder exceeded its timeout"),
        Err(RecvTimeoutError::Disconnected) => invalid("motion-review decoder output disconnected"),
    }
}

fn parse_showinfo_pts(stderr: &[u8]) -> Result<Vec<u64>, CorpusError> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter(|line| line.contains("showinfo"))
        .filter_map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("pts_time:"))
        })
        .map(parse_timestamp_ms)
        .collect()
}

fn normalize_region(
    profile: &scorepeek::capture::GamescopeProfileBinding,
    raw: &[u8],
    stride: u32,
    roi: Roi,
) -> Result<Box<[u8]>, CorpusError> {
    profile
        .geometry()
        .normalize_bgrx_region(
            raw,
            profile.observed_width(),
            profile.observed_height(),
            stride,
            scorepeek::capture::CanonicalRegion {
                left: roi.x,
                top: roi.y,
                width: roi.width,
                height: roi.height,
            },
        )
        .map_err(|_| CorpusError::InvalidReplay("motion-review normalization failed".to_owned()))
}

fn selected_frame_targets(
    timestamps: &[u64],
    targets: &BTreeMap<u64, (usize, ObservationRecord)>,
) -> Result<BTreeMap<usize, Vec<(usize, ObservationRecord)>>, CorpusError> {
    let mut selected = BTreeMap::<usize, Vec<(usize, ObservationRecord)>>::new();
    for (sequence, target) in targets {
        let tick_ms = sequence.checked_mul(100).ok_or_else(|| {
            CorpusError::InvalidReplay("motion-review tick timestamp overflows".to_owned())
        })?;
        let end = timestamps.partition_point(|timestamp| *timestamp <= tick_ms);
        let Some(frame_index) = end.checked_sub(1) else {
            return invalid("motion-review observation lies before the source video");
        };
        if timestamps[frame_index] != target.1.timestamp_ms {
            return invalid("motion-review replay timestamp differs from retained observation");
        }
        selected.entry(frame_index).or_default().push(*target);
    }
    Ok(selected)
}

fn select_expression(indices: impl IntoIterator<Item = usize>) -> String {
    let indices = indices.into_iter().collect::<Vec<_>>();
    let mut terms = Vec::new();
    let mut start = 0;
    while start < indices.len() {
        let stride = indices.get(start + 1).map(|next| next - indices[start]);
        let mut end = start + usize::from(stride.is_some());
        while end + 1 < indices.len() && Some(indices[end + 1] - indices[end]) == stride {
            end += 1;
        }
        if end.saturating_sub(start) >= 2 {
            let first = indices[start];
            let last = indices[end];
            let stride = stride.expect("three indices have a stride");
            terms.push(format!(
                "between(n\\,{first}\\,{last})*not(mod(n-{first}\\,{stride}))"
            ));
        } else {
            terms.extend(
                indices[start..=end]
                    .iter()
                    .map(|index| format!("eq(n\\,{index})")),
            );
        }
        start = end + 1;
    }
    terms.join("+")
}

fn probe_video(
    video: &File,
    width: u32,
    height: u32,
    maximum_timestamp_ms: u64,
) -> Result<VideoInventory, CorpusError> {
    let ffprobe = super::media::find_executable("ffprobe")?;
    let mut command = Command::new(ffprobe);
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_streams",
            "-show_packets",
            "-show_entries",
            "stream=codec_name,width,height,has_b_frames:packet=pts_time,flags",
            "-of",
            "json",
        ])
        .arg("/proc/self/fd/0")
        .stdin(Stdio::from(reopen_video(video)?));
    let (status, stdout, stderr) = run_bounded_output(
        &mut command,
        MAX_PROBE_STDOUT_BYTES,
        MAX_PROCESS_STDERR_BYTES,
        PROBE_TIMEOUT,
    )?;
    if !status.success() || stdout.truncated || stderr.truncated {
        return invalid(&format!(
            "motion-review video metadata failed (status {status}, stdout_truncated {}, stderr {})",
            stdout.truncated,
            process_detail(&stderr)
        ));
    }
    let probe: VideoProbe = serde_json::from_slice(&stdout.bytes)?;
    if probe.streams.len() != 1
        || probe.streams[0].codec_name != "ffv1"
        || probe.streams[0].width != width
        || probe.streams[0].height != height
        || probe.streams[0].has_b_frames != 0
        || probe.packets.is_empty()
        || probe.packets.len() > MAX_VIDEO_PACKETS
    {
        return invalid("motion-review video dimensions differ from the bound profile");
    }
    let mut timestamps = Vec::with_capacity(probe.packets.len());
    let mut keyframes = Vec::new();
    for (index, packet) in probe.packets.into_iter().enumerate() {
        if packet.flags.contains('K') {
            keyframes.push(index);
        }
        let timestamp_ms = packet
            .pts_time
            .as_deref()
            .ok_or_else(|| CorpusError::InvalidReplay("video packet PTS is absent".to_owned()))
            .and_then(parse_timestamp_ms)?;
        if timestamp_ms > maximum_timestamp_ms {
            return invalid("motion-review video timestamp exceeds the session bound");
        }
        timestamps.push(timestamp_ms);
    }
    if keyframes.first() != Some(&0) || timestamps.windows(2).any(|pair| pair[0] >= pair[1]) {
        return invalid("motion-review video packet PTS is not strictly increasing");
    }
    Ok(VideoInventory {
        timestamps,
        keyframes,
    })
}

fn run_bounded_output(
    command: &mut Command,
    stdout_limit: usize,
    stderr_limit: usize,
    timeout: Duration,
) -> Result<(std::process::ExitStatus, BoundedStream, BoundedStream), CorpusError> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let started = Instant::now();
    let Some(stdout) = child.stdout.take() else {
        let completion = finish_decoder(&mut child, true, started, timeout);
        return invalid(&format!(
            "process stdout is unavailable (status {:?}; cleanup {}; failure {:?})",
            completion.status, completion.cleanup, completion.failure
        ));
    };
    let Some(stderr) = child.stderr.take() else {
        drop(stdout);
        let completion = finish_decoder(&mut child, true, started, timeout);
        return invalid(&format!(
            "process stderr is unavailable (status {:?}; cleanup {}; failure {:?})",
            completion.status, completion.cleanup, completion.failure
        ));
    };
    let stdout_reader = thread::spawn(move || read_bounded_stream(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_bounded_stream(stderr, stderr_limit));
    let completion = finish_decoder(&mut child, false, started, timeout);
    let stdout_result = join_stream_reader(stdout_reader);
    let stderr_result = join_stream_reader(stderr_reader);
    if let Some(failure) = completion.failure {
        let stderr = stderr_result
            .as_ref()
            .map_or_else(ToString::to_string, process_detail);
        return invalid(&format!(
            "process supervision failed ({failure}; cleanup {}; stderr {stderr})",
            completion.cleanup
        ));
    }
    let stdout = stdout_result?;
    let stderr = stderr_result?;
    let status = completion.status.ok_or_else(|| {
        CorpusError::InvalidReplay("process status is unavailable after supervision".to_owned())
    })?;
    Ok((status, stdout, stderr))
}

fn read_bounded_stream(
    mut reader: impl Read,
    limit: usize,
) -> Result<BoundedStream, std::io::Error> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut truncated = false;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(bytes.len());
        bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        truncated |= read > remaining;
    }
    Ok(BoundedStream { bytes, truncated })
}

fn join_stream_reader(
    handle: thread::JoinHandle<Result<BoundedStream, std::io::Error>>,
) -> Result<BoundedStream, CorpusError> {
    handle
        .join()
        .map_err(|_| CorpusError::InvalidReplay("process stream reader panicked".to_owned()))?
        .map_err(CorpusError::Io)
}

fn process_detail(stderr: &BoundedStream) -> String {
    let retained = &stderr.bytes[..stderr.bytes.len().min(4_096)];
    let detail = String::from_utf8_lossy(retained)
        .trim()
        .replace(['\r', '\n'], " ");
    if detail.is_empty() {
        "empty".to_owned()
    } else {
        detail
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn parse_timestamp_ms(value: &str) -> Result<u64, CorpusError> {
    let seconds = value.parse::<f64>().map_err(|_| {
        CorpusError::InvalidReplay("motion-review video timestamp is invalid".to_owned())
    })?;
    if !seconds.is_finite() || seconds < 0.0 || seconds > u64::MAX as f64 / 1_000.0 {
        return invalid("motion-review video timestamp is outside its bound");
    }
    Ok((seconds * 1_000.0).round() as u64)
}

fn region_motion_packed(previous: &[u8], current: &[u8], roi: Roi) -> RegionMotion {
    let mut rgb_l1 = 0_u64;
    let mut changed_pixels = 0_u64;
    for (before, after) in previous.chunks_exact(3).zip(current.chunks_exact(3)) {
        if before != after {
            changed_pixels += 1;
        }
        rgb_l1 += before
            .iter()
            .zip(after)
            .map(|(left, right)| u64::from(left.abs_diff(*right)))
            .sum::<u64>();
    }
    let compared_pixels = u64::from(roi.width) * u64::from(roi.height);
    let maximum = compared_pixels.saturating_mul(3).saturating_mul(255);
    RegionMotion {
        rgb_l1,
        changed_pixels,
        compared_pixels,
        normalized_l1_ppm: rgb_l1.saturating_mul(1_000_000) / maximum.max(1),
    }
}

fn bound_artifact<'a>(
    session: &'a CaptureSession,
    source_path: &str,
) -> Result<&'a CorpusArtifact, CorpusError> {
    session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == source_path)
        .ok_or_else(|| CorpusError::InvalidReplay(format!("{source_path} is unavailable")))
}

fn read_bound_object(
    store: &Path,
    artifact: &CorpusArtifact,
    maximum_bytes: u64,
) -> Result<Vec<u8>, CorpusError> {
    if artifact.bytes == 0 || artifact.bytes > maximum_bytes || !valid_sha256(&artifact.sha256) {
        return invalid("motion-review artifact binding is invalid");
    }
    let bytes = read_bounded_regular(
        &store.join("objects").join(&artifact.sha256),
        usize::try_from(maximum_bytes).unwrap_or(usize::MAX),
        ErrorContext::Replay,
    )?;
    if bytes.len() as u64 != artifact.bytes || digest_bytes(&bytes) != artifact.sha256 {
        return invalid("motion-review artifact identity differs");
    }
    Ok(bytes)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, CorpusError> {
    let bytes = read_bounded_regular(path, MAX_DOCUMENT_BYTES, ErrorContext::Replay)?;
    serde_json::from_slice(&bytes).map_err(CorpusError::Json)
}

fn read_bound_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
    expected_sha256: &str,
) -> Result<T, CorpusError> {
    let bytes = read_bounded_regular(path, MAX_DOCUMENT_BYTES, ErrorContext::Replay)?;
    if !valid_sha256(expected_sha256) || digest_bytes(&bytes) != expected_sha256 {
        return invalid("motion-review document identity differs");
    }
    serde_json::from_slice(&bytes).map_err(CorpusError::Json)
}

fn digest_open_file(file: &mut File) -> Result<String, CorpusError> {
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(&mut *file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let mut encoded = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    drop(reader);
    file.seek(SeekFrom::Start(0))?;
    Ok(encoded)
}

fn reopen_video(video: &File) -> Result<File, CorpusError> {
    File::open(format!("/proc/self/fd/{}", video.as_raw_fd())).map_err(CorpusError::Io)
}

fn verify_video_unchanged(
    path: &Path,
    expected_identity: VideoIdentity,
    expected_sha256: &str,
) -> Result<(), CorpusError> {
    let mut current = File::open(path)?;
    let metadata = current.metadata()?;
    let identity = VideoIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    if identity != expected_identity || digest_open_file(&mut current)? != expected_sha256 {
        return invalid("music-select motion review video changed during decoding");
    }
    Ok(())
}

fn publish_create_only(path: &Path, bytes: &[u8]) -> Result<(), CorpusError> {
    let parent = path.parent().ok_or_else(|| {
        CorpusError::InvalidRequest("motion-review output has no parent".to_owned())
    })?;
    if !parent.is_dir() {
        return invalid("motion-review output parent is unavailable");
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".scorepeek-motion-review-")
        .tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| CorpusError::Io(error.error))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn invalid<T>(detail: &str) -> Result<T, CorpusError> {
    Err(CorpusError::InvalidReplay(detail.to_owned()))
}

#[cfg(test)]
mod tests {
    use scorepeek::capture::{
        FractionalRectangle, GamescopeProfileBinding,
        MeasuredGamescopeProfileBindingAuthoringInput, RationalCoordinate,
    };
    use scorepeek::recognition::{CanonicalLayout, Roi, ScreenClass};

    use std::collections::BTreeMap;
    use std::fs::{self, File};
    use std::os::unix::fs::MetadataExt as _;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    use serde_json::json;
    use tempfile::TempDir;

    use super::{
        MAX_PROCESS_STDERR_BYTES, MotionReviewDecision, MotionReviewDecisions, ObservationRecord,
        OperatorReviewState, VideoIdentity, apply_music_select_motion_review, canonical_line,
        digest_bytes, parse_showinfo_pts, plan_music_select_motion_review, read_bounded_stream,
        region_motion_packed, review_windows, run_bounded_output, select_expression,
        selected_frame_targets, verify_video_unchanged,
    };

    #[test]
    fn review_windows_merge_overlapping_padding_around_fast_screen_flicker() {
        let records = [
            observation(1, 100, ScreenClass::Unknown),
            observation(2, 200, ScreenClass::MusicSelect),
            observation(3, 300, ScreenClass::MusicSelect),
            observation(4, 400, ScreenClass::Unknown),
            observation(8, 800, ScreenClass::MusicSelect),
        ];
        let windows = review_windows(&records);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].observed_first_sequence, 2);
        assert_eq!(windows[0].observed_last_sequence, 8);
        assert_eq!(windows[0].review_first_timestamp_ms, 0);
        assert_eq!(windows[0].review_last_timestamp_ms, 1_300);
    }

    #[test]
    fn packed_region_motion_counts_only_supplied_pixels() {
        let mut previous = vec![0_u8; 2 * 3];
        let mut current = previous.clone();
        let roi = Roi {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
        };
        previous[0] = 10;
        current[0] = 20;
        let motion = region_motion_packed(&previous, &current, roi);
        assert_eq!(motion.changed_pixels, 1);
        assert_eq!(motion.rgb_l1, 10);
        assert_eq!(motion.compared_pixels, 2);
    }

    #[test]
    fn selected_frames_reproduce_latest_frame_sampling_at_each_tick() {
        let target = observation(1, 99, ScreenClass::MusicSelect);
        let targets = BTreeMap::from([(1, (0, target))]);
        let selected = selected_frame_targets(&[0, 16, 99, 101], &targets).unwrap();
        assert_eq!(selected.keys().copied().collect::<Vec<_>>(), vec![2]);
        assert_eq!(selected[&2][0].1.timestamp_ms, 99);
    }

    #[test]
    fn select_expression_compacts_regular_frame_runs_without_crossing_gaps() {
        assert_eq!(
            select_expression([6, 12, 18, 30, 36, 50]),
            "between(n\\,6\\,18)*not(mod(n-6\\,6))+eq(n\\,30)+eq(n\\,36)+eq(n\\,50)"
        );
    }

    #[test]
    fn showinfo_pts_is_independent_bounded_decode_evidence() {
        let stderr = b"[Parsed_showinfo_1] n: 0 pts: 89800 pts_time:89.8 duration:16\n\
            [Parsed_showinfo_1] n: 1 pts: 89900 pts_time:89.9 duration:16\n";
        assert_eq!(parse_showinfo_pts(stderr).unwrap(), vec![89_800, 89_900]);
    }

    #[test]
    fn process_stream_is_drained_but_retained_bytes_are_bounded() {
        let output = read_bounded_stream(&b"abcdef"[..], 4).unwrap();
        assert_eq!(output.bytes, b"abcd");
        assert!(output.truncated);
    }

    #[test]
    fn bounded_process_timeout_reaps_the_child() {
        let mut command = Command::new("sleep");
        command.arg("10").stdin(Stdio::null());
        let error = run_bounded_output(
            &mut command,
            16,
            MAX_PROCESS_STDERR_BYTES,
            Duration::from_millis(10),
        )
        .unwrap_err();
        assert!(error.to_string().contains("execution_timeout"));
    }

    #[test]
    fn final_video_check_rejects_path_replacement_even_with_equal_bytes() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("video.mkv");
        fs::write(&path, b"same bytes").unwrap();
        let original = File::open(&path).unwrap();
        let metadata = original.metadata().unwrap();
        let identity = VideoIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        let digest = digest_bytes(b"same bytes");
        fs::rename(&path, temporary.path().join("old.mkv")).unwrap();
        fs::write(&path, b"same bytes").unwrap();
        assert!(verify_video_unchanged(&path, identity, &digest).is_err());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn public_review_plan_verifies_bindings_decodes_and_publishes_create_only() {
        let temporary = TempDir::new().unwrap();
        let store = temporary.path().join("store");
        fs::create_dir_all(store.join("objects")).unwrap();
        fs::create_dir_all(store.join("sessions")).unwrap();
        fs::create_dir_all(store.join("suites")).unwrap();
        let video = temporary.path().join("source.mkv");
        let status = Command::new(super::super::media::find_executable("ffmpeg").unwrap())
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=black:s=1920x1080:r=10:d=0.4",
                "-c:v",
                "ffv1",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&video)
            .status()
            .unwrap();
        assert!(status.success());
        let video_sha256 = digest_bytes(&fs::read(&video).unwrap());
        let source_session_id = format!("video-{}", &video_sha256[..24]);
        let coordinate = |value| RationalCoordinate::new(value, 1).unwrap();
        let authored = GamescopeProfileBinding::author_measured(
            MeasuredGamescopeProfileBindingAuthoringInput {
                observed_width: 1_920,
                observed_height: 1_080,
                geometry: FractionalRectangle::new(
                    coordinate(0),
                    coordinate(0),
                    coordinate(1_920),
                    coordinate(1_080),
                ),
            },
        )
        .unwrap();
        let profile =
            GamescopeProfileBinding::parse(&authored.bytes, &authored.artifact_sha256).unwrap();
        let profile_ref = write_object(&store, &authored.bytes);
        let run = serde_json::to_vec(&json!({
            "schema": "scorepeek-private-diagnostic-capture-start-v3",
            "run_id": source_session_id,
            "binding": {
                "capture_profile_sha256": authored.capture_profile_sha256,
                "normalizer_sha256": profile.normalizer_artifact_sha256(),
                "canonical_layout_sha256": CanonicalLayout::sha256(),
            },
            "source": {"kind": "video_replay", "video_sha256": video_sha256},
        }))
        .unwrap();
        let run_ref = write_object(&store, &run);
        let observations = [
            json!({"schema": super::OBSERVATION_SCHEMA, "tick_sequence": 1, "source_timestamp_ms": 100, "screen": "music_select"}),
            json!({"schema": super::OBSERVATION_SCHEMA, "tick_sequence": 2, "source_timestamp_ms": 200, "screen": "music_select"}),
            json!({"schema": super::OBSERVATION_SCHEMA, "tick_sequence": 3, "source_timestamp_ms": 300, "screen": "unknown"}),
        ]
        .into_iter()
        .flat_map(|value| {
            let mut bytes = serde_json::to_vec(&value).unwrap();
            bytes.push(b'\n');
            bytes
        })
        .collect::<Vec<_>>();
        let observations_ref = write_object(&store, &observations);
        let session = serde_json::to_vec(&json!({
            "schema": super::SESSION_SCHEMA,
            "source_kind": "video_replay",
            "source_session_id": source_session_id,
            "profile_sha256": profile.capture_profile_sha256(),
            "recognition_interval_ms": 100,
            "artifacts": [
                artifact("capture/profile.json", &profile_ref),
                artifact("capture/run.json", &run_ref),
                artifact("recognition/observations.ndjson", &observations_ref),
            ],
        }))
        .unwrap();
        let session_sha256 = digest_bytes(&session);
        fs::write(
            store
                .join("sessions")
                .join(format!("{session_sha256}.json")),
            &session,
        )
        .unwrap();
        let suite = serde_json::to_vec(&json!({
            "schema": super::SUITE_SCHEMA,
            "entries": [{"session_sha256": session_sha256}],
        }))
        .unwrap();
        let suite_sha256 = digest_bytes(&suite);
        fs::write(
            store.join("suites").join(format!("{suite_sha256}.json")),
            suite,
        )
        .unwrap();
        fs::write(
            store.join("active-suite.json"),
            serde_json::to_vec(&json!({
                "schema": super::ACTIVE_SCHEMA,
                "generation_sha256": suite_sha256,
            }))
            .unwrap(),
        )
        .unwrap();
        let output = temporary.path().join("review.json");
        let summary =
            plan_music_select_motion_review(&store, &session_sha256, &video, &output).unwrap();
        assert_eq!(summary.span_count, 1);
        assert_eq!(summary.sample_count, 3);
        assert_eq!(summary.motion_pair_count, 2);
        assert!(output.is_file());
        let decisions = MotionReviewDecisions {
            schema: super::DECISIONS_SCHEMA.to_owned(),
            source_draft_sha256: digest_bytes(&fs::read(&output).unwrap()),
            decisions: vec![MotionReviewDecision {
                span_id: "music-select-span-0001".to_owned(),
                first_sequence: 2,
                last_sequence: 2,
                state: OperatorReviewState::Stationary,
            }],
        };
        let decisions_path = temporary.path().join("decisions.json");
        fs::write(&decisions_path, canonical_line(&decisions).unwrap()).unwrap();
        let reviewed_path = temporary.path().join("reviewed.json");
        let applied =
            apply_music_select_motion_review(&output, &decisions_path, &reviewed_path).unwrap();
        assert_eq!(applied.decision_interval_count, 1);
        assert_eq!(applied.reviewed_motion_pair_count, 1);
        assert_eq!(applied.operator_context_pair_count, 0);
        assert_eq!(applied.remaining_review_pair_count, 0);
        assert_eq!(applied.predicate_context_pair_count, 1);
        assert!(applied.complete);
        let reviewed: serde_json::Value =
            serde_json::from_slice(&fs::read(&reviewed_path).unwrap()).unwrap();
        assert_eq!(reviewed["schema"], super::REVIEWED_SCHEMA);
        assert_eq!(
            reviewed["spans"][0]["pairs"][0]["review_state"]["state"],
            "stationary"
        );
        assert_eq!(
            reviewed["spans"][0]["pairs"][1]["review_state"]["reason"],
            "predicate_screen_context"
        );
        assert!(
            apply_music_select_motion_review(&output, &decisions_path, &reviewed_path).is_err(),
            "review application must not replace an existing set"
        );
        let operator_context = MotionReviewDecisions {
            schema: super::DECISIONS_SCHEMA.to_owned(),
            source_draft_sha256: digest_bytes(&fs::read(&output).unwrap()),
            decisions: vec![MotionReviewDecision {
                span_id: "music-select-span-0001".to_owned(),
                first_sequence: 2,
                last_sequence: 2,
                state: OperatorReviewState::ScreenContext,
            }],
        };
        let operator_context_path = temporary.path().join("operator-context.json");
        fs::write(
            &operator_context_path,
            canonical_line(&operator_context).unwrap(),
        )
        .unwrap();
        let operator_context_reviewed = temporary.path().join("operator-context-reviewed.json");
        let applied = apply_music_select_motion_review(
            &output,
            &operator_context_path,
            &operator_context_reviewed,
        )
        .unwrap();
        assert_eq!(applied.reviewed_motion_pair_count, 0);
        assert_eq!(applied.operator_context_pair_count, 1);
        assert_eq!(applied.remaining_review_pair_count, 0);
        assert_eq!(applied.predicate_context_pair_count, 1);
        assert!(applied.complete);
        let reviewed: serde_json::Value =
            serde_json::from_slice(&fs::read(operator_context_reviewed).unwrap()).unwrap();
        assert_eq!(
            reviewed["spans"][0]["pairs"][0]["review_state"]["reason"],
            "operator_screen_context"
        );
        let invalid_context = MotionReviewDecisions {
            schema: super::DECISIONS_SCHEMA.to_owned(),
            source_draft_sha256: digest_bytes(&fs::read(&output).unwrap()),
            decisions: vec![MotionReviewDecision {
                span_id: "music-select-span-0001".to_owned(),
                first_sequence: 3,
                last_sequence: 3,
                state: OperatorReviewState::SelectionChange,
            }],
        };
        let invalid_path = temporary.path().join("invalid-context.json");
        fs::write(&invalid_path, canonical_line(&invalid_context).unwrap()).unwrap();
        assert!(
            apply_music_select_motion_review(
                &output,
                &invalid_path,
                &temporary.path().join("invalid-reviewed.json")
            )
            .is_err(),
            "context pairs cannot receive an operator decision"
        );
        let overlapping = MotionReviewDecisions {
            schema: super::DECISIONS_SCHEMA.to_owned(),
            source_draft_sha256: digest_bytes(&fs::read(&output).unwrap()),
            decisions: vec![
                MotionReviewDecision {
                    span_id: "music-select-span-0001".to_owned(),
                    first_sequence: 2,
                    last_sequence: 2,
                    state: OperatorReviewState::Stationary,
                },
                MotionReviewDecision {
                    span_id: "music-select-span-0001".to_owned(),
                    first_sequence: 2,
                    last_sequence: 2,
                    state: OperatorReviewState::Scrolling,
                },
            ],
        };
        let overlapping_path = temporary.path().join("overlapping.json");
        fs::write(&overlapping_path, canonical_line(&overlapping).unwrap()).unwrap();
        assert!(
            apply_music_select_motion_review(
                &output,
                &overlapping_path,
                &temporary.path().join("overlapping-reviewed.json")
            )
            .is_err(),
            "overlapping decision intervals must fail closed"
        );
        assert!(plan_music_select_motion_review(&store, &session_sha256, &video, &output).is_err());
    }

    fn write_object(store: &std::path::Path, bytes: &[u8]) -> (String, usize) {
        let sha256 = digest_bytes(bytes);
        fs::write(store.join("objects").join(&sha256), bytes).unwrap();
        (sha256, bytes.len())
    }

    fn artifact(path: &str, object: &(String, usize)) -> serde_json::Value {
        json!({"source_path": path, "sha256": object.0, "bytes": object.1})
    }

    const fn observation(
        sequence: u64,
        timestamp_ms: u64,
        screen: ScreenClass,
    ) -> ObservationRecord {
        ObservationRecord {
            sequence,
            timestamp_ms,
            screen,
        }
    }
}
