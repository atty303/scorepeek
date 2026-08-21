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

#[derive(Debug)]
pub enum PlayAttemptScenarioError {
    Json(serde_json::Error),
    InvalidContract,
}

impl std::fmt::Display for PlayAttemptScenarioError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(formatter, "play-attempt scenario JSON failed: {error}"),
            Self::InvalidContract => formatter.write_str("play-attempt scenario is invalid"),
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
    #[serde(rename = "screen_observation")]
    screen: ScreenObservation,
    song_decision: SongDecision,
    event_outcome: EventOutcome,
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Episode {
    #[serde(rename = "episode_id")]
    id: String,
    segment_id: String,
    kind: ScreenKind,
    first_sequence: u64,
    last_sequence: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProposedAttempt {
    attempt_id: String,
    selection_episode_id: String,
    gameplay_episode_id: String,
    result_episode_id: String,
    proposed_result_event: ProposedResultEvent,
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
/// invalid completeness evidence, overlapping episodes, or an impossible play-attempt ordering.
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
    let (result_episode_count, absent_result_event_count) =
        validate_attempts(&scenario, &episodes, &observations)?;
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
        || policy.target_interval_ms == 0
        || policy.target_interval_ms > policy.maximum_observation_gap_ms
        || policy.maximum_observation_gap_ms == 0
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
        || scenario.proposed_episodes.is_empty()
        || scenario.proposed_episodes.len() > MAX_EPISODES
        || scenario.proposed_attempts.is_empty()
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
    let mut calibrated = true;
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
                if observation.sequence != previous.sequence + 1
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
                if segment.observations.len() < 2
                    || *maximum_observation_gap_ms != maximum_gap
                    || maximum_gap > scenario.policy.maximum_observation_gap_ms
                    || maximum_gap >= *minimum_result_dwell_ms
                {
                    return Err(PlayAttemptScenarioError::InvalidContract);
                }
                match dwell_evidence {
                    DwellEvidence::SyntheticScenario => calibrated = false,
                    DwellEvidence::CalibratedProfile { calibration_id } => {
                        if !valid_id(calibration_id) {
                            return Err(PlayAttemptScenarioError::InvalidContract);
                        }
                    }
                }
            }
            RecordingCompleteness::Partial { reason } => {
                let _ = reason;
                calibrated = false;
            }
            RecordingCompleteness::Dropped { reason } => {
                let _ = reason;
                calibrated = false;
            }
        }
    }
    if total_artifact_bytes > scenario.policy.retention.total_bytes {
        return Err(PlayAttemptScenarioError::InvalidContract);
    }
    Ok((
        observations,
        if calibrated {
            MissAccountingScope::CalibratedProfile
        } else {
            MissAccountingScope::SyntheticScenario
        },
    ))
}

fn validate_observation(observation: &Observation) -> Result<(), PlayAttemptScenarioError> {
    if !valid_sha256(&observation.canonical_frame_sha256)
        || observation.artifact.kind != ArtifactKind::CanonicalRgb8Ppm
        || !valid_sha256(&observation.artifact.file_sha256)
        || observation.artifact.bytes != CANONICAL_FRAME_FILE_BYTES
    {
        return Err(PlayAttemptScenarioError::InvalidContract);
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
    observations: &BTreeMap<(&str, u64), &Observation>,
) -> Result<(usize, usize), PlayAttemptScenarioError> {
    let mut attempt_ids = BTreeSet::new();
    let mut used_episodes = BTreeSet::new();
    let mut absent_result_event_count = 0;
    for attempt in &scenario.proposed_attempts {
        let selection = episodes
            .get(attempt.selection_episode_id.as_str())
            .ok_or(PlayAttemptScenarioError::InvalidContract)?;
        let gameplay = episodes
            .get(attempt.gameplay_episode_id.as_str())
            .ok_or(PlayAttemptScenarioError::InvalidContract)?;
        let result = episodes
            .get(attempt.result_episode_id.as_str())
            .ok_or(PlayAttemptScenarioError::InvalidContract)?;
        if !valid_id(&attempt.attempt_id)
            || !attempt_ids.insert(attempt.attempt_id.as_str())
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
        let result_event_absent = (result.first_sequence..=result.last_sequence).all(|sequence| {
            observations
                .get(&(result.segment_id.as_str(), sequence))
                .is_some_and(|observation| {
                    matches!(&observation.event_outcome, EventOutcome::Absent)
                })
        });
        let result_event_emitted = (result.first_sequence..=result.last_sequence).any(|sequence| {
            observations
                .get(&(result.segment_id.as_str(), sequence))
                .is_some_and(|observation| {
                    matches!(&observation.event_outcome, EventOutcome::Emitted { .. })
                })
        });
        match attempt.proposed_result_event {
            ProposedResultEvent::Absent if result_event_absent => absent_result_event_count += 1,
            ProposedResultEvent::Emitted if result_event_emitted => {}
            _ => return Err(PlayAttemptScenarioError::InvalidContract),
        }
    }
    Ok((
        scenario
            .proposed_episodes
            .iter()
            .filter(|episode| episode.kind == ScreenKind::Result)
            .count(),
        absent_result_event_count,
    ))
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
}
