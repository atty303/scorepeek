use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const SCHEMA: &str = "scorepeek-private-play-attempt-scenario-v1";
const MAX_SCENARIO_BYTES: usize = 1024 * 1024;
const MAX_SEGMENTS: usize = 8;
const MAX_OBSERVATIONS: usize = 4_096;
const MAX_EPISODES: usize = 1_024;
const MAX_ATTEMPTS: usize = 512;
const MAX_SEGMENT_DURATION_MS: u64 = 15 * 60 * 1_000;
const MAX_ARTIFACT_BYTES_PER_SEGMENT: u64 = 32 * 1024 * 1024 * 1024;
const MAX_RETAINED_BYTES: u64 = 128 * 1024 * 1024 * 1024;
const CANONICAL_FRAME_FILE_BYTES: u64 = 6_220_817;
const SYNTHETIC_TARGET_INTERVAL_MS: u64 = 250;
const SYNTHETIC_MAXIMUM_OBSERVATION_GAP_MS: u64 = 500;
const SYNTHETIC_STABLE_SELECTION_OBSERVATIONS: usize = 2;
const SYNTHETIC_STABLE_SELECTION_DWELL_MS: u64 = 250;
const SYNTHETIC_MINIMUM_RESULT_DWELL_MS: u64 = 1_000;

#[derive(Debug)]
pub enum PlayAttemptScenarioError {
    Json(serde_json::Error),
    InvalidContract,
    TimelineMismatch,
}

impl std::fmt::Display for PlayAttemptScenarioError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "play-attempt scenario JSON failed: {error}"),
            Self::InvalidContract => formatter.write_str("play-attempt scenario is invalid"),
            Self::TimelineMismatch => formatter
                .write_str("recording-derived timeline does not match the scenario proposal"),
        }
    }
}

impl std::error::Error for PlayAttemptScenarioError {}

impl From<serde_json::Error> for PlayAttemptScenarioError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissAccountingScope {
    SyntheticScenario,
    CalibratedProfile,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlayAttemptScenarioSummary {
    pub schema: &'static str,
    pub scenario_sha256: String,
    pub segment_count: usize,
    pub observation_count: usize,
    pub episode_count: usize,
    pub attempt_count: usize,
    pub result_episode_count: usize,
    pub absent_result_event_count: usize,
    pub miss_accounting_scope: MissAccountingScope,
    pub timeline_review_state: TimelineReviewState,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    schema: String,
    #[serde(rename = "scenario_id")]
    id: String,
    resource: Resource,
    policy: DiagnosticPolicy,
    timeline_review: TimelineReview,
    bindings: Vec<BindingSet>,
    segments: Vec<Segment>,
    proposed_episodes: Vec<Episode>,
    proposed_attempts: Vec<ProposedAttempt>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineReview {
    inference_source: TimelineInferenceSource,
    state: TimelineReviewState,
    operator_notes_applied: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum TimelineInferenceSource {
    RecordingOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineReviewState {
    NeedsOperatorReview,
    Confirmed,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Resource {
    service_name: String,
    service_version: String,
    runtime_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiagnosticPolicy {
    recording_default: RecordingDefault,
    recording_opt_out_supported: bool,
    target_interval_ms: u64,
    maximum_observation_gap_ms: u64,
    minimum_stable_selection_observations: usize,
    minimum_stable_selection_dwell_ms: u64,
    maximum_segment_duration_ms: u64,
    maximum_observations_per_segment: usize,
    maximum_artifact_bytes_per_segment: u64,
    full_frame_retention: FullFrameRetention,
    retention: RetentionPolicy,
    remote_export: RemoteExport,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum RecordingDefault {
    Enabled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum FullFrameRetention {
    UntilRoiContractStabilizes,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum RemoteExport {
    Disabled,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionPolicy {
    #[serde(rename = "maximum_normal_runs")]
    normal_runs: usize,
    #[serde(rename = "maximum_priority_runs")]
    priority_runs: usize,
    #[serde(rename = "maximum_total_bytes")]
    total_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingSet {
    binding_id: String,
    capture_generation: u64,
    capture_profile_sha256: String,
    normalizer_sha256: String,
    canonical_layout_sha256: String,
    catalog_sha256: String,
    model_sha256: String,
    runtime_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Segment {
    #[serde(rename = "segment_id")]
    id: String,
    binding_id: String,
    completeness: RecordingCompleteness,
    observations: Vec<Observation>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum RecordingCompleteness {
    Complete {
        maximum_observation_gap_ms: u64,
        minimum_result_dwell_ms: u64,
        dwell_evidence: DwellEvidence,
    },
    Partial {
        reason: PartialReason,
    },
    Dropped {
        reason: DroppedReason,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
enum DwellEvidence {
    SyntheticScenario,
    CalibratedProfile { calibration_id: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum PartialReason {
    CaptureGap,
    ArtifactUnavailable,
    RecordingFailure,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum DroppedReason {
    CapacityExceeded,
    RecordingDisabled,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    sequence: u64,
    monotonic_ms: u64,
    canonical_frame_sha256: String,
    artifact: FrameArtifact,
    timeline_evidence: TimelineEvidence,
    selection_evidence: SelectionEvidence,
    #[serde(rename = "screen_observation")]
    screen: ScreenObservation,
    song_decision: SongDecision,
    event_outcome: EventOutcome,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum SelectionEvidence {
    Observed { fingerprint_sha256: String },
    Unknown { reason: SelectionUnknownReason },
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SelectionUnknownReason {
    Ambiguous,
    Uncovered,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum TimelineEvidence {
    Observed { screen: ScreenKind },
    Unknown { reason: TimelineUnknownReason },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum TimelineUnknownReason {
    RecordingAmbiguous,
    UncoveredInterval,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrameArtifact {
    kind: ArtifactKind,
    file_sha256: String,
    bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ArtifactKind {
    CanonicalRgb8Ppm,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum ScreenObservation {
    NotRun { reason: NotRunReason },
    Unknown { reason: ScreenUnknownReason },
    Observed { screen: ScreenKind },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum NotRunReason {
    SparseDiagnosticCadence,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ScreenUnknownReason {
    DetectorUnknown,
    Transition,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "snake_case")]
enum ScreenKind {
    MusicSelect,
    Gameplay,
    Result,
    Other,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum SongDecision {
    NotRun { reason: NotRunReason },
    NotApplicable,
    Unknown { reason: SongUnknownReason },
    Rejected { reason: SongRejectionReason },
    Accepted { song_id: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SongUnknownReason {
    InsufficientEvidence,
    DetectorUnknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SongRejectionReason {
    ContextConflict,
    BindingChanged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum EventOutcome {
    Absent,
    Suppressed { reason: SuppressionReason },
    Emitted { event_id: String },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum SuppressionReason {
    Deduplicated,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct Episode {
    #[serde(rename = "episode_id")]
    id: String,
    segment_id: String,
    kind: ScreenKind,
    first_sequence: u64,
    last_sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct ProposedAttempt {
    #[serde(rename = "attempt_id")]
    id: String,
    #[serde(rename = "selection_episode_id")]
    selection: String,
    #[serde(rename = "gameplay_episode_id")]
    gameplay: String,
    #[serde(rename = "result_episode_id")]
    result: String,
}

type ObservationMap<'a> = BTreeMap<(&'a str, u64), &'a Observation>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ProposedResultEvent {
    Absent,
    Emitted,
}

/// Validates a bounded, replay-labelled play-attempt scenario without executing recognition.
///
/// The scenario keeps recorded detector/decision outcomes separate from the recording-derived
/// timeline proposal. A proposed result episode can therefore remain enumerable even when detection
/// was not run and no public event was emitted, while still requiring operator review.
///
/// # Errors
/// Returns an error for malformed or over-limit input, binding drift, non-monotonic observations,
/// invalid completeness evidence, overlapping episodes, an impossible play-attempt ordering, or a
/// committed proposal that differs from the recording-only replay.
pub fn validate_play_attempt_scenario(
    bytes: &[u8],
) -> Result<PlayAttemptScenarioSummary, PlayAttemptScenarioError> {
    if bytes.is_empty() || bytes.len() > MAX_SCENARIO_BYTES {
        return Err(PlayAttemptScenarioError::InvalidContract);
    }
    let scenario: Scenario = serde_json::from_slice(bytes)?;
    validate_header(&scenario)?;
    let bindings = validate_bindings(&scenario)?;
    let (observations, miss_accounting_scope) = validate_segments(&scenario, &bindings)?;
    let episodes = validate_episodes(&scenario, &observations)?;
    validate_attempts(&scenario, &episodes)?;
    if derive_episodes(&scenario) != scenario.proposed_episodes
        || derive_attempts(&scenario, &scenario.proposed_episodes) != scenario.proposed_attempts
    {
        return Err(PlayAttemptScenarioError::TimelineMismatch);
    }
    let (result_episode_count, absent_result_event_count) =
        result_episode_counts(&scenario, &observations);
    Ok(PlayAttemptScenarioSummary {
        schema: SCHEMA,
        scenario_sha256: encode_sha256(bytes),
        segment_count: scenario.segments.len(),
        observation_count: observations.len(),
        episode_count: scenario.proposed_episodes.len(),
        attempt_count: scenario.proposed_attempts.len(),
        result_episode_count,
        absent_result_event_count,
        miss_accounting_scope,
        timeline_review_state: scenario.timeline_review.state,
    })
}

fn validate_header(scenario: &Scenario) -> Result<(), PlayAttemptScenarioError> {
    let policy = &scenario.policy;
    if scenario.schema != SCHEMA
        || !valid_id(&scenario.id)
        || scenario.resource.service_name != "scorepeek"
        || scenario.resource.service_version != env!("CARGO_PKG_VERSION")
        || !valid_sha256(&scenario.resource.runtime_sha256)
        || scenario.timeline_review.inference_source != TimelineInferenceSource::RecordingOnly
        || scenario.timeline_review.operator_notes_applied
            != (scenario.timeline_review.state == TimelineReviewState::Confirmed)
        || policy.recording_default != RecordingDefault::Enabled
        || !policy.recording_opt_out_supported
        || policy.target_interval_ms != SYNTHETIC_TARGET_INTERVAL_MS
        || policy.maximum_observation_gap_ms != SYNTHETIC_MAXIMUM_OBSERVATION_GAP_MS
        || policy.minimum_stable_selection_observations != SYNTHETIC_STABLE_SELECTION_OBSERVATIONS
        || policy.minimum_stable_selection_dwell_ms != SYNTHETIC_STABLE_SELECTION_DWELL_MS
        || policy.maximum_segment_duration_ms == 0
        || policy.maximum_segment_duration_ms > MAX_SEGMENT_DURATION_MS
        || policy.maximum_observations_per_segment == 0
        || policy.maximum_observations_per_segment > MAX_OBSERVATIONS
        || policy.maximum_artifact_bytes_per_segment == 0
        || policy.maximum_artifact_bytes_per_segment > MAX_ARTIFACT_BYTES_PER_SEGMENT
        || policy.full_frame_retention != FullFrameRetention::UntilRoiContractStabilizes
        || policy.remote_export != RemoteExport::Disabled
        || policy.retention.normal_runs > 2
        || policy.retention.priority_runs == 0
        || policy.retention.priority_runs > 6
        || policy.retention.normal_runs + policy.retention.priority_runs > MAX_SEGMENTS
        || policy.retention.total_bytes == 0
        || policy.retention.total_bytes > MAX_RETAINED_BYTES
        || scenario.segments.is_empty()
        || scenario.segments.len() > MAX_SEGMENTS
        || scenario.proposed_episodes.len() > MAX_EPISODES
        || scenario.proposed_attempts.len() > MAX_ATTEMPTS
    {
        return Err(PlayAttemptScenarioError::InvalidContract);
    }
    Ok(())
}

fn validate_bindings(
    scenario: &Scenario,
) -> Result<BTreeMap<&str, &BindingSet>, PlayAttemptScenarioError> {
    if scenario.bindings.is_empty() || scenario.bindings.len() > MAX_SEGMENTS {
        return Err(PlayAttemptScenarioError::InvalidContract);
    }
    let mut bindings = BTreeMap::new();
    for binding in &scenario.bindings {
        if !valid_id(&binding.binding_id)
            || binding.capture_generation == 0
            || !valid_sha256(&binding.capture_profile_sha256)
            || !valid_sha256(&binding.normalizer_sha256)
            || !valid_sha256(&binding.canonical_layout_sha256)
            || !valid_sha256(&binding.catalog_sha256)
            || !valid_sha256(&binding.model_sha256)
            || binding.runtime_sha256 != scenario.resource.runtime_sha256
            || bindings
                .insert(binding.binding_id.as_str(), binding)
                .is_some()
        {
            return Err(PlayAttemptScenarioError::InvalidContract);
        }
    }
    Ok(bindings)
}

fn validate_segments<'a>(
    scenario: &'a Scenario,
    bindings: &BTreeMap<&str, &BindingSet>,
) -> Result<(ObservationMap<'a>, MissAccountingScope), PlayAttemptScenarioError> {
    let mut segment_ids = BTreeSet::new();
    let mut observations = BTreeMap::new();
    let mut total_artifact_bytes = 0_u64;
    for segment in &scenario.segments {
        if !valid_id(&segment.id)
            || !segment_ids.insert(segment.id.as_str())
            || !bindings.contains_key(segment.binding_id.as_str())
            || segment.observations.is_empty()
            || segment.observations.len() > scenario.policy.maximum_observations_per_segment
        {
            return Err(PlayAttemptScenarioError::InvalidContract);
        }
        let mut maximum_gap = 0;
        let mut artifact_bytes = 0_u64;
        for (index, observation) in segment.observations.iter().enumerate() {
            validate_observation(observation)?;
            artifact_bytes = artifact_bytes
                .checked_add(observation.artifact.bytes)
                .ok_or(PlayAttemptScenarioError::InvalidContract)?;
            if let Some(previous) = index
                .checked_sub(1)
                .and_then(|previous| segment.observations.get(previous))
            {
                if previous.sequence.checked_add(1) != Some(observation.sequence)
                    || observation.monotonic_ms <= previous.monotonic_ms
                {
                    return Err(PlayAttemptScenarioError::InvalidContract);
                }
                maximum_gap = maximum_gap.max(observation.monotonic_ms - previous.monotonic_ms);
            }
            if observations
                .insert((segment.id.as_str(), observation.sequence), observation)
                .is_some()
            {
                return Err(PlayAttemptScenarioError::InvalidContract);
            }
        }
        let duration = segment
            .observations
            .last()
            .expect("non-empty checked")
            .monotonic_ms
            - segment
                .observations
                .first()
                .expect("non-empty checked")
                .monotonic_ms;
        if duration > scenario.policy.maximum_segment_duration_ms
            || artifact_bytes > scenario.policy.maximum_artifact_bytes_per_segment
        {
            return Err(PlayAttemptScenarioError::InvalidContract);
        }
        total_artifact_bytes = total_artifact_bytes
            .checked_add(artifact_bytes)
            .ok_or(PlayAttemptScenarioError::InvalidContract)?;
        match &segment.completeness {
            RecordingCompleteness::Complete {
                maximum_observation_gap_ms,
                minimum_result_dwell_ms,
                dwell_evidence,
            } => {
                if let DwellEvidence::CalibratedProfile { calibration_id } = dwell_evidence {
                    let _ = calibration_id;
                    return Err(PlayAttemptScenarioError::InvalidContract);
                }
                if segment.observations.len() < 2
                    || *maximum_observation_gap_ms != maximum_gap
                    || maximum_gap > scenario.policy.maximum_observation_gap_ms
                    || maximum_gap >= *minimum_result_dwell_ms
                    || *minimum_result_dwell_ms != SYNTHETIC_MINIMUM_RESULT_DWELL_MS
                {
                    return Err(PlayAttemptScenarioError::InvalidContract);
                }
            }
            RecordingCompleteness::Partial { reason } => {
                let _ = reason;
            }
            RecordingCompleteness::Dropped { reason } => {
                let _ = reason;
            }
        }
    }
    if total_artifact_bytes > scenario.policy.retention.total_bytes {
        return Err(PlayAttemptScenarioError::InvalidContract);
    }
    Ok((observations, MissAccountingScope::SyntheticScenario))
}

fn validate_observation(observation: &Observation) -> Result<(), PlayAttemptScenarioError> {
    if !valid_sha256(&observation.canonical_frame_sha256)
        || observation.artifact.kind != ArtifactKind::CanonicalRgb8Ppm
        || !valid_sha256(&observation.artifact.file_sha256)
        || observation.artifact.bytes != CANONICAL_FRAME_FILE_BYTES
    {
        return Err(PlayAttemptScenarioError::InvalidContract);
    }
    match &observation.timeline_evidence {
        TimelineEvidence::Observed { screen } => {
            let _ = screen;
        }
        TimelineEvidence::Unknown { reason } => {
            let _ = reason;
        }
    }
    match (
        &observation.timeline_evidence,
        &observation.selection_evidence,
    ) {
        (
            TimelineEvidence::Observed {
                screen: ScreenKind::MusicSelect,
            },
            SelectionEvidence::Observed { fingerprint_sha256 },
        ) if valid_sha256(fingerprint_sha256) => {}
        (
            TimelineEvidence::Observed {
                screen: ScreenKind::MusicSelect,
            }
            | TimelineEvidence::Unknown { .. },
            SelectionEvidence::Unknown { reason },
        ) => {
            let _ = reason;
        }
        (
            TimelineEvidence::Observed {
                screen: ScreenKind::Gameplay | ScreenKind::Result | ScreenKind::Other,
            }
            | TimelineEvidence::Unknown { .. },
            SelectionEvidence::NotApplicable,
        ) => {}
        _ => return Err(PlayAttemptScenarioError::InvalidContract),
    }
    match &observation.screen {
        ScreenObservation::NotRun { reason } => {
            let _ = reason;
        }
        ScreenObservation::Unknown { reason } => {
            let _ = reason;
        }
        ScreenObservation::Observed { screen } => {
            let _ = screen;
        }
    }
    match &observation.song_decision {
        SongDecision::NotRun { reason } => {
            let _ = reason;
        }
        SongDecision::NotApplicable => {}
        SongDecision::Unknown { reason } => {
            let _ = reason;
        }
        SongDecision::Rejected { reason } => {
            let _ = reason;
        }
        SongDecision::Accepted { song_id } if valid_id(song_id) => {}
        SongDecision::Accepted { .. } => return Err(PlayAttemptScenarioError::InvalidContract),
    }
    match &observation.event_outcome {
        EventOutcome::Absent => {}
        EventOutcome::Suppressed { reason } => {
            let _ = reason;
        }
        EventOutcome::Emitted { event_id } if valid_id(event_id) => {}
        EventOutcome::Emitted { .. } => return Err(PlayAttemptScenarioError::InvalidContract),
    }
    Ok(())
}

fn validate_episodes<'a>(
    scenario: &'a Scenario,
    observations: &BTreeMap<(&str, u64), &Observation>,
) -> Result<BTreeMap<&'a str, &'a Episode>, PlayAttemptScenarioError> {
    let mut episodes = BTreeMap::new();
    let mut occupied = BTreeSet::new();
    for episode in &scenario.proposed_episodes {
        if !valid_id(&episode.id)
            || episode.first_sequence > episode.last_sequence
            || episodes.insert(episode.id.as_str(), episode).is_some()
        {
            return Err(PlayAttemptScenarioError::InvalidContract);
        }
        for sequence in episode.first_sequence..=episode.last_sequence {
            if !observations.contains_key(&(episode.segment_id.as_str(), sequence))
                || !occupied.insert((episode.segment_id.as_str(), sequence))
            {
                return Err(PlayAttemptScenarioError::InvalidContract);
            }
        }
    }
    Ok(episodes)
}

fn validate_attempts(
    scenario: &Scenario,
    episodes: &BTreeMap<&str, &Episode>,
) -> Result<(), PlayAttemptScenarioError> {
    let mut attempt_ids = BTreeSet::new();
    let mut used_episodes = BTreeSet::new();
    for attempt in &scenario.proposed_attempts {
        let selection = episodes
            .get(attempt.selection.as_str())
            .ok_or(PlayAttemptScenarioError::InvalidContract)?;
        let gameplay = episodes
            .get(attempt.gameplay.as_str())
            .ok_or(PlayAttemptScenarioError::InvalidContract)?;
        let result = episodes
            .get(attempt.result.as_str())
            .ok_or(PlayAttemptScenarioError::InvalidContract)?;
        if !valid_id(&attempt.id)
            || !attempt_ids.insert(attempt.id.as_str())
            || selection.kind != ScreenKind::MusicSelect
            || gameplay.kind != ScreenKind::Gameplay
            || result.kind != ScreenKind::Result
            || selection.segment_id != gameplay.segment_id
            || gameplay.segment_id != result.segment_id
            || selection.last_sequence >= gameplay.first_sequence
            || gameplay.last_sequence >= result.first_sequence
            || !used_episodes.insert(selection.id.as_str())
            || !used_episodes.insert(gameplay.id.as_str())
            || !used_episodes.insert(result.id.as_str())
        {
            return Err(PlayAttemptScenarioError::InvalidContract);
        }
    }
    Ok(())
}

fn result_episode_counts(scenario: &Scenario, observations: &ObservationMap<'_>) -> (usize, usize) {
    let results: Vec<_> = scenario
        .proposed_episodes
        .iter()
        .filter(|episode| episode.kind == ScreenKind::Result)
        .collect();
    let absent = results
        .iter()
        .filter(|episode| segment_is_complete(scenario, &episode.segment_id))
        .filter(|episode| {
            result_event_from_outcomes((episode.first_sequence..=episode.last_sequence).map(
                |sequence| {
                    &observations
                        .get(&(episode.segment_id.as_str(), sequence))
                        .expect("validated episode observation")
                        .event_outcome
                },
            )) == ProposedResultEvent::Absent
        })
        .count();
    (results.len(), absent)
}

/// Replays recording-only timeline evidence and renders the proposal for operator review.
///
/// This function is pure: it does not inspect frame bytes, read operator notes, publish events, or
/// confirm the proposal. The scenario's proposed episodes and attempts are replay oracles and must
/// exactly match the derived composition.
///
/// # Errors
/// Returns an error when the scenario contract is invalid or its proposal differs from replay.
pub fn render_timeline_proposal_report(bytes: &[u8]) -> Result<String, PlayAttemptScenarioError> {
    let summary = validate_play_attempt_scenario(bytes)?;
    let scenario: Scenario = serde_json::from_slice(bytes)?;
    let episodes = derive_episodes(&scenario);
    let attempts = derive_attempts(&scenario, &episodes);
    Ok(render_report(&scenario, &summary, &episodes, &attempts))
}

fn derive_episodes(scenario: &Scenario) -> Vec<Episode> {
    let mut episodes = Vec::new();
    let mut counts = BTreeMap::new();
    for segment in &scenario.segments {
        let mut current: Option<Episode> = None;
        for observation in &segment.observations {
            let observed = match observation.timeline_evidence {
                TimelineEvidence::Observed { screen } => Some(screen),
                TimelineEvidence::Unknown { .. } => None,
            };
            match (&mut current, observed) {
                (Some(episode), Some(screen)) if episode.kind == screen => {
                    episode.last_sequence = observation.sequence;
                }
                (slot, Some(screen)) => {
                    if let Some(completed) = slot.take() {
                        episodes.push(completed);
                    }
                    let count = counts.entry(screen).or_insert(0_u64);
                    *count += 1;
                    *slot = Some(Episode {
                        id: format!("{}-{count:03}", episode_prefix(screen)),
                        segment_id: segment.id.clone(),
                        kind: screen,
                        first_sequence: observation.sequence,
                        last_sequence: observation.sequence,
                    });
                }
                (slot, None) => {
                    if let Some(completed) = slot.take() {
                        episodes.push(completed);
                    }
                }
            }
        }
        if let Some(completed) = current {
            episodes.push(completed);
        }
    }
    episodes
}

fn derive_attempts(scenario: &Scenario, episodes: &[Episode]) -> Vec<ProposedAttempt> {
    let mut attempts = Vec::new();
    let mut current_segment = None;
    let mut selection: Option<&Episode> = None;
    let mut gameplay: Option<(&Episode, &Episode)> = None;
    for episode in episodes {
        if current_segment != Some(episode.segment_id.as_str()) {
            current_segment = Some(episode.segment_id.as_str());
            selection = None;
            gameplay = None;
        }
        if !segment_is_complete(scenario, &episode.segment_id) {
            selection = None;
            gameplay = None;
            continue;
        }
        match episode.kind {
            ScreenKind::MusicSelect => {
                selection = selection_is_stable(scenario, episode).then_some(episode);
                gameplay = None;
            }
            ScreenKind::Gameplay => {
                gameplay = selection
                    .filter(|selected| {
                        selected.last_sequence.checked_add(1) == Some(episode.first_sequence)
                    })
                    .map(|selected| (selected, episode));
                if gameplay.is_none() {
                    selection = None;
                }
            }
            ScreenKind::Result => {
                if let Some((selected, played)) = gameplay.take().filter(|(_, played)| {
                    played.last_sequence.checked_add(1) == Some(episode.first_sequence)
                }) {
                    attempts.push(ProposedAttempt {
                        id: format!("attempt-{:03}", attempts.len() + 1),
                        selection: selected.id.clone(),
                        gameplay: played.id.clone(),
                        result: episode.id.clone(),
                    });
                }
                selection = None;
            }
            ScreenKind::Other => {
                selection = None;
                gameplay = None;
            }
        }
    }
    attempts
}

fn segment_is_complete(scenario: &Scenario, segment_id: &str) -> bool {
    scenario.segments.iter().any(|segment| {
        segment.id == segment_id
            && matches!(segment.completeness, RecordingCompleteness::Complete { .. })
    })
}

fn selection_is_stable(scenario: &Scenario, episode: &Episode) -> bool {
    let segment = scenario
        .segments
        .iter()
        .find(|segment| segment.id == episode.segment_id)
        .expect("validated episode segment");
    let observations: Vec<_> = segment
        .observations
        .iter()
        .filter(|observation| {
            (episode.first_sequence..=episode.last_sequence).contains(&observation.sequence)
        })
        .collect();
    let Some(SelectionEvidence::Observed {
        fingerprint_sha256: latest,
    }) = observations
        .last()
        .map(|observation| &observation.selection_evidence)
    else {
        return false;
    };
    let stable_suffix: Vec<_> = observations
        .iter()
        .rev()
        .take_while(|observation| {
            matches!(
                &observation.selection_evidence,
                SelectionEvidence::Observed { fingerprint_sha256 }
                    if fingerprint_sha256 == latest
            )
        })
        .copied()
        .collect();
    stable_suffix.len() >= scenario.policy.minimum_stable_selection_observations
        && stable_suffix
            .first()
            .zip(stable_suffix.last())
            .is_some_and(|(last, first)| {
                last.monotonic_ms - first.monotonic_ms
                    >= scenario.policy.minimum_stable_selection_dwell_ms
            })
}

fn result_event_for_episode(scenario: &Scenario, episode: &Episode) -> ProposedResultEvent {
    let observations = scenario
        .segments
        .iter()
        .find(|segment| segment.id == episode.segment_id)
        .expect("validated episode segment")
        .observations
        .iter()
        .filter(|observation| {
            (episode.first_sequence..=episode.last_sequence).contains(&observation.sequence)
        });
    result_event_from_outcomes(observations.map(|observation| &observation.event_outcome))
}

fn result_event_from_outcomes<'a>(
    outcomes: impl Iterator<Item = &'a EventOutcome>,
) -> ProposedResultEvent {
    if outcomes
        .into_iter()
        .any(|outcome| matches!(outcome, EventOutcome::Emitted { .. }))
    {
        ProposedResultEvent::Emitted
    } else {
        ProposedResultEvent::Absent
    }
}

fn render_report(
    scenario: &Scenario,
    summary: &PlayAttemptScenarioSummary,
    episodes: &[Episode],
    attempts: &[ProposedAttempt],
) -> String {
    let mut report = String::new();
    writeln!(report, "# Timeline proposal: {}", scenario.id).expect("String write");
    writeln!(report).expect("String write");
    writeln!(report, "- Scenario SHA-256: `{}`", summary.scenario_sha256).expect("String write");
    writeln!(
        report,
        "- Review state: `{}`",
        review_state_name(summary.timeline_review_state)
    )
    .expect("String write");
    writeln!(report, "- Inference source: `recording_only`").expect("String write");

    render_recording_structure(&mut report, scenario, episodes, attempts);

    writeln!(report, "\n## Gaps and discrepancies").expect("String write");
    render_discrepancies(&mut report, scenario, episodes, attempts);

    writeln!(report, "\n## Operator review questions").expect("String write");
    writeln!(report, "\n- Are the inferred episode boundaries correct?").expect("String write");
    writeln!(
        report,
        "- Does each proposed selection → gameplay → result link describe one play?"
    )
    .expect("String write");
    writeln!(
        report,
        "- Are there recording-external exceptions or missing facts to apply?"
    )
    .expect("String write");
    report
}

fn render_recording_structure(
    report: &mut String,
    scenario: &Scenario,
    episodes: &[Episode],
    attempts: &[ProposedAttempt],
) {
    writeln!(report, "\n## Bindings").expect("String write");
    for binding in &scenario.bindings {
        writeln!(
            report,
            "\n- `{}`: generation {}; capture `{}`; normalizer `{}`; layout `{}`; catalog `{}`; model `{}`; runtime `{}`",
            binding.binding_id,
            binding.capture_generation,
            binding.capture_profile_sha256,
            binding.normalizer_sha256,
            binding.canonical_layout_sha256,
            binding.catalog_sha256,
            binding.model_sha256,
            binding.runtime_sha256
        )
        .expect("String write");
    }

    writeln!(report, "\n## Segments").expect("String write");
    for segment in &scenario.segments {
        let first = segment
            .observations
            .first()
            .expect("validated observations");
        let last = segment.observations.last().expect("validated observations");
        writeln!(
            report,
            "\n- `{}`: binding `{}`, sequences {}–{}, monotonic {}–{} ms, {}",
            segment.id,
            segment.binding_id,
            first.sequence,
            last.sequence,
            first.monotonic_ms,
            last.monotonic_ms,
            completeness_description(&segment.completeness)
        )
        .expect("String write");
    }

    writeln!(report, "\n## Inferred episodes").expect("String write");
    if episodes.is_empty() {
        writeln!(report, "\n- None inferred from the recording evidence.").expect("String write");
    }
    for episode in episodes {
        write!(
            report,
            "\n- `{}`: `{}` sequences {}–{} in `{}`",
            episode.id,
            screen_name(episode.kind),
            episode.first_sequence,
            episode.last_sequence,
            episode.segment_id
        )
        .expect("String write");
        if episode.kind == ScreenKind::MusicSelect {
            let fingerprints = selection_fingerprints(scenario, episode)
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            write!(
                report,
                "; recording selection fingerprints [{fingerprints}], stable `{}`",
                selection_is_stable(scenario, episode)
            )
            .expect("String write");
        }
        writeln!(report).expect("String write");
    }

    writeln!(report, "\n## Proposed play attempts").expect("String write");
    if attempts.is_empty() {
        writeln!(
            report,
            "\n- None; no complete unbroken stable selection → gameplay → result link was inferred."
        )
        .expect("String write");
    }
    for attempt in attempts {
        writeln!(
            report,
            "\n- `{}`: `{}` → `{}` → `{}`",
            attempt.id, attempt.selection, attempt.gameplay, attempt.result
        )
        .expect("String write");
    }
}

fn render_discrepancies(
    report: &mut String,
    scenario: &Scenario,
    episodes: &[Episode],
    attempts: &[ProposedAttempt],
) {
    let mut count = render_observation_discrepancies(report, scenario);
    count += render_episode_discrepancies(report, scenario, episodes);
    count += render_attempt_discrepancies(report, scenario, episodes, attempts);
    if count == 0 {
        writeln!(report, "\n- None in the recorded evidence.").expect("String write");
    }
}

fn render_observation_discrepancies(report: &mut String, scenario: &Scenario) -> usize {
    let mut count = 0;
    for segment in &scenario.segments {
        count += render_segment_discrepancy(report, segment);
        for observation in &segment.observations {
            count += render_covered_live_evidence(report, &segment.id, observation);
            count += render_screen_discrepancy(report, &segment.id, observation);
            count += render_song_discrepancy(report, &segment.id, observation);
            count += render_event_discrepancy(report, &segment.id, observation);
        }
    }
    count
}

fn render_covered_live_evidence(
    report: &mut String,
    segment_id: &str,
    observation: &Observation,
) -> usize {
    if !matches!(
        observation.timeline_evidence,
        TimelineEvidence::Observed { .. }
    ) {
        return 0;
    }
    writeln!(
        report,
        "\n- `{segment_id}` sequence {} live evidence: {}",
        observation.sequence,
        live_observation_summary(observation)
    )
    .expect("String write");
    1
}

fn render_segment_discrepancy(report: &mut String, segment: &Segment) -> usize {
    if matches!(segment.completeness, RecordingCompleteness::Complete { .. }) {
        return 0;
    }
    writeln!(
        report,
        "\n- `{}`: {} recording; gap location is not fully known, so attempt linkage is disabled for the segment",
        segment.id,
        completeness_state_name(&segment.completeness)
    )
    .expect("String write");
    1
}

fn render_screen_discrepancy(
    report: &mut String,
    segment_id: &str,
    observation: &Observation,
) -> usize {
    match (&observation.timeline_evidence, &observation.screen) {
        (
            TimelineEvidence::Observed { screen: inferred },
            ScreenObservation::Observed { screen: detected },
        ) if inferred != detected => {
            writeln!(
                report,
                "\n- `{segment_id}` sequence {}: recording replay `{}` conflicts with live detector `{}`",
                observation.sequence,
                screen_name(*inferred),
                screen_name(*detected)
            )
            .expect("String write");
            1
        }
        (
            TimelineEvidence::Observed { screen: inferred },
            ScreenObservation::Unknown { .. } | ScreenObservation::NotRun { .. },
        ) => {
            writeln!(
                report,
                "\n- `{segment_id}` sequence {}: recording replay inferred `{}` while the live detector was `{}`",
                observation.sequence,
                screen_name(*inferred),
                detector_state_name(&observation.screen)
            )
            .expect("String write");
            1
        }
        (TimelineEvidence::Unknown { reason }, _) => {
            writeln!(
                report,
                "\n- `{segment_id}` sequence {}: timeline uncovered (`{}`); {}",
                observation.sequence,
                timeline_unknown_name(*reason),
                live_observation_summary(observation)
            )
            .expect("String write");
            1
        }
        _ => 0,
    }
}

fn render_song_discrepancy(
    report: &mut String,
    segment_id: &str,
    observation: &Observation,
) -> usize {
    match (&observation.timeline_evidence, &observation.song_decision) {
        (
            TimelineEvidence::Observed {
                screen: ScreenKind::MusicSelect | ScreenKind::Result,
            },
            decision,
        ) if !matches!(decision, SongDecision::Accepted { .. }) => {
            writeln!(
                report,
                "\n- `{segment_id}` sequence {}: `{}` timeline has song decision `{}`",
                observation.sequence,
                timeline_screen_name(&observation.timeline_evidence),
                song_decision_state_name(decision)
            )
            .expect("String write");
            1
        }
        (
            TimelineEvidence::Observed {
                screen: ScreenKind::Gameplay | ScreenKind::Other,
            },
            SongDecision::Accepted { .. },
        ) => {
            writeln!(
                report,
                "\n- `{segment_id}` sequence {}: `{}` timeline unexpectedly has an accepted song decision",
                observation.sequence,
                timeline_screen_name(&observation.timeline_evidence)
            )
            .expect("String write");
            1
        }
        _ => 0,
    }
}

fn render_event_discrepancy(
    report: &mut String,
    segment_id: &str,
    observation: &Observation,
) -> usize {
    match (&observation.timeline_evidence, &observation.event_outcome) {
        (
            TimelineEvidence::Observed {
                screen: ScreenKind::Gameplay | ScreenKind::Other,
            },
            EventOutcome::Emitted { .. },
        ) => {
            writeln!(
                report,
                "\n- `{segment_id}` sequence {}: `{}` timeline unexpectedly emitted a public event",
                observation.sequence,
                timeline_screen_name(&observation.timeline_evidence)
            )
            .expect("String write");
            1
        }
        (
            TimelineEvidence::Observed {
                screen: ScreenKind::Result,
            },
            EventOutcome::Suppressed { reason },
        ) => {
            writeln!(
                report,
                "\n- `{segment_id}` sequence {}: result event was suppressed (`{}`)",
                observation.sequence,
                suppression_reason_name(*reason)
            )
            .expect("String write");
            1
        }
        _ => 0,
    }
}

fn render_episode_discrepancies(
    report: &mut String,
    scenario: &Scenario,
    episodes: &[Episode],
) -> usize {
    let mut count = 0;
    for episode in episodes {
        count += 1;
        writeln!(
            report,
            "\n- `{}` live observations: {}",
            episode.id,
            episode_live_outcome_summary(scenario, episode)
        )
        .expect("String write");
    }
    for episode in episodes.iter().filter(|episode| {
        episode.kind == ScreenKind::Result && segment_is_complete(scenario, &episode.segment_id)
    }) {
        if result_event_for_episode(scenario, episode) == ProposedResultEvent::Absent {
            count += 1;
            writeln!(
                report,
                "\n- `{}`: result episode inferred but no result event was emitted",
                episode.id
            )
            .expect("String write");
        }
        let emitted = episode_observations(scenario, episode)
            .filter(|observation| matches!(observation.event_outcome, EventOutcome::Emitted { .. }))
            .count();
        if emitted > 1 {
            count += 1;
            writeln!(
                report,
                "\n- `{}`: result episode emitted {emitted} public events",
                episode.id
            )
            .expect("String write");
        }
    }
    for episode in episodes {
        let accepted = accepted_song_ids(scenario, episode);
        if accepted.len() > 1 {
            count += 1;
            writeln!(
                report,
                "\n- `{}`: conflicting accepted song IDs [{}]",
                episode.id,
                accepted.into_iter().collect::<Vec<_>>().join(", ")
            )
            .expect("String write");
        }
        if episode.kind == ScreenKind::MusicSelect {
            let fingerprints = selection_fingerprints(scenario, episode);
            if fingerprints.len() > 1 {
                count += 1;
                writeln!(
                    report,
                    "\n- `{}`: recording selection fingerprint changed [{}]",
                    episode.id,
                    fingerprints.into_iter().collect::<Vec<_>>().join(", ")
                )
                .expect("String write");
            }
            let unknown = selection_unknown_count(scenario, episode);
            if unknown > 0 {
                count += 1;
                writeln!(
                    report,
                    "\n- `{}`: recording selection continuity is unknown for {unknown} observations",
                    episode.id
                )
                .expect("String write");
            }
        }
    }
    count
}

fn render_attempt_discrepancies(
    report: &mut String,
    scenario: &Scenario,
    episodes: &[Episode],
    attempts: &[ProposedAttempt],
) -> usize {
    let mut count = 0;
    for attempt in attempts {
        let selection = episodes
            .iter()
            .find(|episode| episode.id == attempt.selection)
            .expect("derived selection episode");
        let result = episodes
            .iter()
            .find(|episode| episode.id == attempt.result)
            .expect("derived result episode");
        let selection_ids = accepted_song_ids(scenario, selection);
        let result_ids = accepted_song_ids(scenario, result);
        if selection_ids.len() == 1 && result_ids.len() == 1 && selection_ids != result_ids {
            count += 1;
            writeln!(
                report,
                "\n- `{}`: selection accepted [{}] but result accepted [{}]",
                attempt.id,
                selection_ids.into_iter().collect::<Vec<_>>().join(", "),
                result_ids.into_iter().collect::<Vec<_>>().join(", ")
            )
            .expect("String write");
        }
    }
    count
}

fn episode_live_outcome_summary(scenario: &Scenario, episode: &Episode) -> String {
    let observations = scenario
        .segments
        .iter()
        .find(|segment| segment.id == episode.segment_id)
        .expect("validated episode segment")
        .observations
        .iter()
        .filter(|observation| {
            (episode.first_sequence..=episode.last_sequence).contains(&observation.sequence)
        });
    let mut detected = 0;
    let mut detector_unknown = 0;
    let mut detector_not_run = 0;
    let mut song_accepted = 0;
    let mut song_other = 0;
    let mut event_emitted = 0;
    let mut event_suppressed = 0;
    let mut event_absent = 0;
    for observation in observations {
        match observation.screen {
            ScreenObservation::Observed { .. } => detected += 1,
            ScreenObservation::Unknown { .. } => detector_unknown += 1,
            ScreenObservation::NotRun { .. } => detector_not_run += 1,
        }
        match observation.song_decision {
            SongDecision::Accepted { .. } => song_accepted += 1,
            _ => song_other += 1,
        }
        match observation.event_outcome {
            EventOutcome::Emitted { .. } => event_emitted += 1,
            EventOutcome::Suppressed { .. } => event_suppressed += 1,
            EventOutcome::Absent => event_absent += 1,
        }
    }
    let accepted = accepted_song_ids(scenario, episode)
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    let fingerprints = selection_fingerprints(scenario, episode)
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "selection fingerprints [{fingerprints}]; live detector observed/unknown/not-run {detected}/{detector_unknown}/{detector_not_run}; song accepted/other {song_accepted}/{song_other}, IDs [{accepted}]; events emitted/suppressed/absent {event_emitted}/{event_suppressed}/{event_absent}"
    )
}

fn accepted_song_ids<'a>(scenario: &'a Scenario, episode: &Episode) -> BTreeSet<&'a str> {
    episode_observations(scenario, episode)
        .filter_map(|observation| match &observation.song_decision {
            SongDecision::Accepted { song_id } => Some(song_id.as_str()),
            _ => None,
        })
        .collect()
}

fn selection_fingerprints<'a>(scenario: &'a Scenario, episode: &Episode) -> BTreeSet<&'a str> {
    episode_observations(scenario, episode)
        .filter_map(|observation| match &observation.selection_evidence {
            SelectionEvidence::Observed { fingerprint_sha256 } => Some(fingerprint_sha256.as_str()),
            SelectionEvidence::Unknown { .. } | SelectionEvidence::NotApplicable => None,
        })
        .collect()
}

fn selection_unknown_count(scenario: &Scenario, episode: &Episode) -> usize {
    episode_observations(scenario, episode)
        .filter(|observation| {
            matches!(
                observation.selection_evidence,
                SelectionEvidence::Unknown { .. }
            )
        })
        .count()
}

fn episode_observations<'a>(
    scenario: &'a Scenario,
    episode: &Episode,
) -> impl Iterator<Item = &'a Observation> {
    let first_sequence = episode.first_sequence;
    let last_sequence = episode.last_sequence;
    scenario
        .segments
        .iter()
        .find(|segment| segment.id == episode.segment_id)
        .expect("validated episode segment")
        .observations
        .iter()
        .filter(move |observation| (first_sequence..=last_sequence).contains(&observation.sequence))
}

fn episode_prefix(screen: ScreenKind) -> &'static str {
    match screen {
        ScreenKind::MusicSelect => "selection",
        ScreenKind::Gameplay => "gameplay",
        ScreenKind::Result => "result",
        ScreenKind::Other => "other",
    }
}

fn screen_name(screen: ScreenKind) -> &'static str {
    match screen {
        ScreenKind::MusicSelect => "music_select",
        ScreenKind::Gameplay => "gameplay",
        ScreenKind::Result => "result",
        ScreenKind::Other => "other",
    }
}

fn review_state_name(state: TimelineReviewState) -> &'static str {
    match state {
        TimelineReviewState::NeedsOperatorReview => "needs_operator_review",
        TimelineReviewState::Confirmed => "confirmed",
    }
}

fn detector_state_name(observation: &ScreenObservation) -> &'static str {
    match observation {
        ScreenObservation::NotRun { .. } => "not_run",
        ScreenObservation::Unknown { .. } => "unknown",
        ScreenObservation::Observed { .. } => "observed",
    }
}

fn timeline_screen_name(evidence: &TimelineEvidence) -> &'static str {
    match evidence {
        TimelineEvidence::Observed { screen } => screen_name(*screen),
        TimelineEvidence::Unknown { .. } => "unknown",
    }
}

fn song_decision_state_name(decision: &SongDecision) -> &'static str {
    match decision {
        SongDecision::NotRun { .. } => "not_run",
        SongDecision::NotApplicable => "not_applicable",
        SongDecision::Unknown { .. } => "unknown",
        SongDecision::Rejected { .. } => "rejected",
        SongDecision::Accepted { .. } => "accepted",
    }
}

fn suppression_reason_name(reason: SuppressionReason) -> &'static str {
    match reason {
        SuppressionReason::Deduplicated => "deduplicated",
        SuppressionReason::Rejected => "rejected",
    }
}

fn timeline_unknown_name(reason: TimelineUnknownReason) -> &'static str {
    match reason {
        TimelineUnknownReason::RecordingAmbiguous => "recording_ambiguous",
        TimelineUnknownReason::UncoveredInterval => "uncovered_interval",
    }
}

fn live_observation_summary(observation: &Observation) -> String {
    format!(
        "live detector {}; song {}; event {}",
        live_screen_description(&observation.screen),
        live_song_description(&observation.song_decision),
        live_event_description(&observation.event_outcome)
    )
}

fn live_screen_description(observation: &ScreenObservation) -> String {
    match observation {
        ScreenObservation::Observed { screen } => format!("observed({})", screen_name(*screen)),
        ScreenObservation::NotRun { .. } => "not_run(sparse_diagnostic_cadence)".to_owned(),
        ScreenObservation::Unknown { reason } => format!(
            "unknown({})",
            match reason {
                ScreenUnknownReason::DetectorUnknown => "detector_unknown",
                ScreenUnknownReason::Transition => "transition",
            }
        ),
    }
}

fn live_song_description(decision: &SongDecision) -> String {
    match decision {
        SongDecision::Accepted { song_id } => format!("accepted({song_id})"),
        SongDecision::NotRun { .. } => "not_run(sparse_diagnostic_cadence)".to_owned(),
        SongDecision::NotApplicable => "not_applicable".to_owned(),
        SongDecision::Unknown { reason } => format!(
            "unknown({})",
            match reason {
                SongUnknownReason::InsufficientEvidence => "insufficient_evidence",
                SongUnknownReason::DetectorUnknown => "detector_unknown",
            }
        ),
        SongDecision::Rejected { reason } => format!(
            "rejected({})",
            match reason {
                SongRejectionReason::ContextConflict => "context_conflict",
                SongRejectionReason::BindingChanged => "binding_changed",
            }
        ),
    }
}

fn live_event_description(outcome: &EventOutcome) -> String {
    match outcome {
        EventOutcome::Absent => "absent".to_owned(),
        EventOutcome::Suppressed { reason } => {
            format!("suppressed({})", suppression_reason_name(*reason))
        }
        EventOutcome::Emitted { event_id } => format!("emitted({event_id})"),
    }
}

fn completeness_description(completeness: &RecordingCompleteness) -> String {
    match completeness {
        RecordingCompleteness::Complete {
            maximum_observation_gap_ms,
            minimum_result_dwell_ms,
            dwell_evidence,
        } => format!(
            "complete; maximum gap {maximum_observation_gap_ms} ms; minimum result dwell {minimum_result_dwell_ms} ms; evidence `{}`",
            match dwell_evidence {
                DwellEvidence::SyntheticScenario => "synthetic_scenario",
                DwellEvidence::CalibratedProfile { .. } => "calibrated_profile",
            }
        ),
        RecordingCompleteness::Partial { reason } => format!(
            "partial; reason `{}`",
            match reason {
                PartialReason::CaptureGap => "capture_gap",
                PartialReason::ArtifactUnavailable => "artifact_unavailable",
                PartialReason::RecordingFailure => "recording_failure",
            }
        ),
        RecordingCompleteness::Dropped { reason } => format!(
            "dropped; reason `{}`",
            match reason {
                DroppedReason::CapacityExceeded => "capacity_exceeded",
                DroppedReason::RecordingDisabled => "recording_disabled",
            }
        ),
    }
}

fn completeness_state_name(completeness: &RecordingCompleteness) -> &'static str {
    match completeness {
        RecordingCompleteness::Complete { .. } => "complete",
        RecordingCompleteness::Partial { .. } => "partial",
        RecordingCompleteness::Dropped { .. } => "dropped",
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
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
    use super::*;

    const SCENARIO: &[u8] = include_bytes!("play-attempt-scenario-v1.json");

    #[test]
    fn missed_result_remains_enumerable_without_recognition_or_event() {
        let summary = validate_play_attempt_scenario(SCENARIO).unwrap();
        assert_eq!(summary.segment_count, 1);
        assert_eq!(summary.observation_count, 6);
        assert_eq!(summary.episode_count, 3);
        assert_eq!(summary.attempt_count, 1);
        assert_eq!(summary.result_episode_count, 1);
        assert_eq!(summary.absent_result_event_count, 1);
        assert_eq!(
            summary.miss_accounting_scope,
            MissAccountingScope::SyntheticScenario
        );
        assert_eq!(
            summary.timeline_review_state,
            TimelineReviewState::NeedsOperatorReview
        );
    }

    #[test]
    fn complete_segment_rejects_a_gap_at_the_result_dwell_boundary() {
        let text = std::str::from_utf8(SCENARIO).unwrap();
        let changed = text
            .replace(
                "\"maximum_observation_gap_ms\": 250",
                "\"maximum_observation_gap_ms\": 1000",
            )
            .replace("\"monotonic_ms\": 1250", "\"monotonic_ms\": 2000");
        assert!(validate_play_attempt_scenario(changed.as_bytes()).is_err());
    }

    #[test]
    fn episode_rejects_an_unknown_segment() {
        let text = std::str::from_utf8(SCENARIO).unwrap();
        let changed = text.replace(
            "{ \"episode_id\": \"result-001\", \"segment_id\": \"segment-001\"",
            "{ \"episode_id\": \"result-001\", \"segment_id\": \"segment-other\"",
        );
        assert!(validate_play_attempt_scenario(changed.as_bytes()).is_err());
    }

    #[test]
    fn scenario_document_and_runtime_collections_are_bounded() {
        assert!(validate_play_attempt_scenario(&vec![b' '; MAX_SCENARIO_BYTES + 1]).is_err());
        let text = std::str::from_utf8(SCENARIO).unwrap();
        let changed = text.replace(
            "\"maximum_observations_per_segment\": 4096",
            "\"maximum_observations_per_segment\": 4097",
        );
        assert!(validate_play_attempt_scenario(changed.as_bytes()).is_err());
    }

    #[test]
    fn v1_rejects_variable_cadence_and_calibrated_scope() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["policy"]["target_interval_ms"] = serde_json::json!(1);
        document["policy"]["maximum_observation_gap_ms"] = serde_json::json!(1);
        document["policy"]["minimum_stable_selection_dwell_ms"] = serde_json::json!(1);
        assert!(validate_play_attempt_scenario(&serde_json::to_vec(&document).unwrap()).is_err());

        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["segments"][0]["completeness"]["dwell_evidence"] = serde_json::json!({
            "scope": "calibrated_profile",
            "calibration_id": "calibration-001"
        });
        assert!(validate_play_attempt_scenario(&serde_json::to_vec(&document).unwrap()).is_err());
    }

    #[test]
    fn confirmation_requires_explicit_operator_notes() {
        let text = std::str::from_utf8(SCENARIO).unwrap();
        let unreviewed_confirmation = text.replace(
            "\"state\": \"needs_operator_review\"",
            "\"state\": \"confirmed\"",
        );
        assert!(validate_play_attempt_scenario(unreviewed_confirmation.as_bytes()).is_err());

        let reviewed_confirmation = unreviewed_confirmation.replace(
            "\"operator_notes_applied\": false",
            "\"operator_notes_applied\": true",
        );
        assert!(validate_play_attempt_scenario(reviewed_confirmation.as_bytes()).is_ok());
    }

    #[test]
    fn replay_renders_recording_proposal_and_live_discrepancies() {
        let report = render_timeline_proposal_report(SCENARIO).unwrap();
        assert!(report.contains("`selection-001`: `music_select` sequences 1–2"));
        assert!(report.contains("`attempt-001`: `selection-001` → `gameplay-001` → `result-001`"));
        assert!(report.contains(
            "`segment-001` sequence 5: recording replay inferred `result` while the live detector was `not_run`"
        ));
        assert!(report.contains(
            "`segment-001` sequence 1 live evidence: live detector observed(music_select); song accepted(song-001); event emitted(event-music-001)"
        ));
        assert!(report.contains(
            "`segment-001` sequence 2 live evidence: live detector observed(music_select); song accepted(song-001); event suppressed(deduplicated)"
        ));
        assert!(
            report
                .contains("`result-001`: result episode inferred but no result event was emitted")
        );
        assert!(
            report.contains("Are there recording-external exceptions or missing facts to apply?")
        );
    }

    #[test]
    fn replay_rejects_a_proposal_not_derived_from_recording() {
        let text = std::str::from_utf8(SCENARIO).unwrap();
        let changed = text.replacen(
            "\"timeline_evidence\": { \"state\": \"observed\", \"screen\": \"result\" }",
            "\"timeline_evidence\": { \"state\": \"observed\", \"screen\": \"gameplay\" }",
            1,
        );
        assert!(matches!(
            render_timeline_proposal_report(changed.as_bytes()),
            Err(PlayAttemptScenarioError::TimelineMismatch)
        ));
        assert!(matches!(
            validate_play_attempt_scenario(changed.as_bytes()),
            Err(PlayAttemptScenarioError::TimelineMismatch)
        ));
    }

    #[test]
    fn unknown_recording_transition_breaks_attempt_linkage() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        for observation in document["segments"][0]["observations"]
            .as_array_mut()
            .unwrap()
        {
            observation["timeline_evidence"] =
                serde_json::json!({"state": "unknown", "reason": "recording_ambiguous"});
            observation["selection_evidence"] =
                serde_json::json!({"state": "unknown", "reason": "ambiguous"});
        }
        document["proposed_episodes"] = serde_json::json!([]);
        document["proposed_attempts"] = serde_json::json!([]);
        let report =
            render_timeline_proposal_report(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(report.contains("None inferred from the recording evidence."));
        assert!(report.contains("None; no complete unbroken stable selection"));
        assert!(report.contains("timeline uncovered (`recording_ambiguous`)"));
        assert!(report.contains(
            "`segment-001` sequence 1: timeline uncovered (`recording_ambiguous`); live detector observed(music_select); song accepted(song-001); event emitted(event-music-001)"
        ));
        assert!(report.contains(
            "`segment-001` sequence 2: timeline uncovered (`recording_ambiguous`); live detector observed(music_select); song accepted(song-001); event suppressed(deduplicated)"
        ));
    }

    #[test]
    fn partial_segment_disables_attempt_linkage() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["segments"][0]["completeness"] =
            serde_json::json!({"state": "partial", "reason": "capture_gap"});
        document["proposed_attempts"] = serde_json::json!([]);
        let bytes = serde_json::to_vec(&document).unwrap();
        let summary = validate_play_attempt_scenario(&bytes).unwrap();
        assert_eq!(summary.result_episode_count, 1);
        assert_eq!(summary.absent_result_event_count, 0);
        let report = render_timeline_proposal_report(&bytes).unwrap();
        assert!(report.contains("partial recording; gap location is not fully known"));
        assert!(report.contains("attempt linkage is disabled"));
        assert!(!report.contains("result episode inferred but no result event was emitted"));
    }

    #[test]
    fn validator_rejects_attempts_retained_on_a_partial_segment() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["segments"][0]["completeness"] =
            serde_json::json!({"state": "partial", "reason": "capture_gap"});
        assert!(matches!(
            validate_play_attempt_scenario(&serde_json::to_vec(&document).unwrap()),
            Err(PlayAttemptScenarioError::TimelineMismatch)
        ));
    }

    #[test]
    fn observed_other_screen_remains_in_the_recording_composition() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["segments"][0]["observations"][2]["timeline_evidence"] =
            serde_json::json!({"state": "observed", "screen": "other"});
        document["proposed_episodes"] = serde_json::json!([
            {"episode_id": "selection-001", "segment_id": "segment-001", "kind": "music_select", "first_sequence": 1, "last_sequence": 2},
            {"episode_id": "other-001", "segment_id": "segment-001", "kind": "other", "first_sequence": 3, "last_sequence": 3},
            {"episode_id": "gameplay-001", "segment_id": "segment-001", "kind": "gameplay", "first_sequence": 4, "last_sequence": 4},
            {"episode_id": "result-001", "segment_id": "segment-001", "kind": "result", "first_sequence": 5, "last_sequence": 6}
        ]);
        document["proposed_attempts"] = serde_json::json!([]);
        let report =
            render_timeline_proposal_report(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(report.contains("`other-001`: `other` sequences 3–3"));
        assert!(report.contains("None; no complete unbroken stable selection"));
    }

    #[test]
    fn one_selection_sample_is_reported_but_not_linked() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["segments"][0]["observations"][1]["timeline_evidence"] =
            serde_json::json!({"state": "observed", "screen": "gameplay"});
        document["segments"][0]["observations"][1]["selection_evidence"] =
            serde_json::json!({"state": "not_applicable"});
        document["proposed_episodes"] = serde_json::json!([
            {"episode_id": "selection-001", "segment_id": "segment-001", "kind": "music_select", "first_sequence": 1, "last_sequence": 1},
            {"episode_id": "gameplay-001", "segment_id": "segment-001", "kind": "gameplay", "first_sequence": 2, "last_sequence": 4},
            {"episode_id": "result-001", "segment_id": "segment-001", "kind": "result", "first_sequence": 5, "last_sequence": 6}
        ]);
        document["proposed_attempts"] = serde_json::json!([]);
        let bytes = serde_json::to_vec(&document).unwrap();
        let summary = validate_play_attempt_scenario(&bytes).unwrap();
        assert_eq!(summary.result_episode_count, 1);
        assert_eq!(summary.absent_result_event_count, 1);
        let report = render_timeline_proposal_report(&bytes).unwrap();
        assert!(report.contains("`selection-001`: `music_select` sequences 1–1"));
        assert!(report.contains("None; no complete unbroken stable selection"));
    }

    #[test]
    fn report_exposes_accepted_song_conflicts() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["segments"][0]["observations"][1]["song_decision"] =
            serde_json::json!({"state": "accepted", "song_id": "song-002"});
        let report =
            render_timeline_proposal_report(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(
            report.contains("`selection-001`: conflicting accepted song IDs [song-001, song-002]")
        );
    }

    #[test]
    fn report_exposes_cross_screen_song_conflict() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        for index in [4, 5] {
            document["segments"][0]["observations"][index]["song_decision"] =
                serde_json::json!({"state": "accepted", "song_id": "song-002"});
        }
        let report =
            render_timeline_proposal_report(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(report.contains(
            "`attempt-001`: selection accepted [song-001] but result accepted [song-002]"
        ));
    }

    #[test]
    fn changed_selection_fingerprint_is_not_linked() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["segments"][0]["observations"][1]["selection_evidence"] = serde_json::json!({
            "state": "observed",
            "fingerprint_sha256": "8888888888888888888888888888888888888888888888888888888888888888"
        });
        document["proposed_attempts"] = serde_json::json!([]);
        let report =
            render_timeline_proposal_report(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(report.contains("None; no complete unbroken stable selection"));
    }

    #[test]
    fn suppressed_result_event_remains_reportable_as_not_emitted() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["segments"][0]["observations"][4]["event_outcome"] =
            serde_json::json!({"state": "suppressed", "reason": "rejected"});
        let report =
            render_timeline_proposal_report(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(
            report.contains("`segment-001` sequence 5: result event was suppressed (`rejected`)")
        );
        assert!(report.contains("events emitted/suppressed/absent 0/1/1"));
        assert!(!report.contains("`result-001`; result event"));
    }

    #[test]
    fn gameplay_song_and_event_outcomes_are_reported_as_discrepancies() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        document["segments"][0]["observations"][2]["song_decision"] =
            serde_json::json!({"state": "accepted", "song_id": "song-001"});
        document["segments"][0]["observations"][2]["event_outcome"] =
            serde_json::json!({"state": "emitted", "event_id": "event-gameplay-001"});
        let report =
            render_timeline_proposal_report(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(report.contains(
            "`segment-001` sequence 3: `gameplay` timeline unexpectedly has an accepted song decision"
        ));
        assert!(report.contains(
            "`segment-001` sequence 3: `gameplay` timeline unexpectedly emitted a public event"
        ));
    }

    #[test]
    fn observation_discrepancies_identify_segment_local_sequences() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        for observation in document["segments"][0]["observations"]
            .as_array_mut()
            .unwrap()
        {
            observation["timeline_evidence"] =
                serde_json::json!({"state": "unknown", "reason": "recording_ambiguous"});
            observation["selection_evidence"] =
                serde_json::json!({"state": "unknown", "reason": "ambiguous"});
        }
        let mut second = document["segments"][0].clone();
        second["segment_id"] = serde_json::json!("segment-002");
        document["segments"].as_array_mut().unwrap().push(second);
        document["proposed_episodes"] = serde_json::json!([]);
        document["proposed_attempts"] = serde_json::json!([]);

        let report =
            render_timeline_proposal_report(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(report.contains("`segment-001` sequence 1: timeline uncovered"));
        assert!(report.contains("`segment-002` sequence 1: timeline uncovered"));
    }

    #[test]
    fn duplicate_result_emissions_are_reported() {
        let mut document: serde_json::Value = serde_json::from_slice(SCENARIO).unwrap();
        for (index, event_id) in [(4, "event-result-001"), (5, "event-result-002")] {
            document["segments"][0]["observations"][index]["event_outcome"] =
                serde_json::json!({"state": "emitted", "event_id": event_id});
        }
        let report =
            render_timeline_proposal_report(&serde_json::to_vec(&document).unwrap()).unwrap();
        assert!(report.contains("`result-001`: result episode emitted 2 public events"));
    }
}
