use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead as _, BufReader, Read as _};
use std::path::Path;

use scorepeek::catalog::ScorepeekSongId;
use scorepeek::recognition::ScreenClass;
use scorepeek::temporal_recognition::{
    ResultTemporalReducer, TemporalFieldState, TemporalPolicy, TemporalTransitionReason,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{CorpusError, ErrorContext, digest_bytes, encode_digest, read_bounded_regular};

const ACTIVE_SCHEMA: &str = "scorepeek-private-regression-suite-active-v1";
const SUITE_SCHEMA: &str = "scorepeek-private-regression-suite-v1";
const SESSION_SCHEMA: &str = "scorepeek-private-capture-session-v1";
const LABEL_SCHEMA: &str = "scorepeek-private-session-regression-label-v1";
const OBSERVATION_SCHEMA: &str = "scorepeek-recognition-observation-v5";
const SUMMARY_SCHEMA: &str = "scorepeek-private-temporal-evaluation-v1";
const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const MAX_OBSERVATION_BYTES: u64 = 512 * 1024 * 1024;
const MAX_OBSERVATION_RECORD_BYTES: usize = 1024 * 1024;
const MAX_OBSERVATIONS: usize = 250_000;
const MAX_POLICIES: usize = 16;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TemporalEvaluationPolicy {
    pub required_observations: u8,
    pub maximum_gap_ms: u64,
}

impl TemporalEvaluationPolicy {
    /// Constructs one bounded offline policy.
    ///
    /// # Errors
    ///
    /// Returns [`CorpusError::InvalidRequest`] when fewer than two observations are required, the
    /// gap is zero, or either value exceeds the offline evaluator bound.
    pub fn new(required_observations: u8, maximum_gap_ms: u64) -> Result<Self, CorpusError> {
        TemporalPolicy::new(required_observations, maximum_gap_ms).map_err(|_| {
            CorpusError::InvalidRequest(
                "temporal policy requires at least two observations and a nonzero gap".to_owned(),
            )
        })?;
        if required_observations > 16 || maximum_gap_ms > 60_000 {
            return Err(CorpusError::InvalidRequest(
                "temporal policy exceeds the offline evaluation bound".to_owned(),
            ));
        }
        Ok(Self {
            required_observations,
            maximum_gap_ms,
        })
    }

    fn reducer(self) -> ResultTemporalReducer<ScorepeekSongId> {
        ResultTemporalReducer::new(
            TemporalPolicy::new(self.required_observations, self.maximum_gap_ms)
                .expect("validated temporal evaluation policy remains valid"),
        )
    }
}

#[derive(Debug, Serialize)]
pub struct TemporalEvaluationSummary {
    schema: &'static str,
    generation_sha256: String,
    session_count: usize,
    labeled_episode_count: usize,
    analyzable_episode_count: usize,
    excluded_episodes: Vec<ExcludedEpisode>,
    raw_observations: RawObservationSummary,
    policies: Vec<TemporalPolicySummary>,
    authority: &'static str,
}

#[derive(Debug, Serialize)]
struct ExcludedEpisode {
    episode_id: String,
    reason: ExclusionReason,
    available_result_observations: usize,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ExclusionReason {
    ResultIntervalUnavailable,
    AmbiguousResultInterval,
    InsufficientTemporalObservations,
}

#[derive(Clone, Debug, Default, Serialize)]
struct RawObservationSummary {
    observations: usize,
    song: RawFieldSummary,
    clear_type: RawFieldSummary,
}

#[derive(Clone, Debug, Default, Serialize)]
struct RawFieldSummary {
    correct: usize,
    incorrect: usize,
    unknown: usize,
}

#[derive(Debug, Serialize)]
struct TemporalPolicySummary {
    policy: TemporalEvaluationPolicy,
    episode_count: usize,
    song: TemporalOutcomeSummary,
    clear_type: TemporalOutcomeSummary,
    joint_stable_correct: usize,
    transitions: TransitionSummary,
    joint_stabilization_ms: DistributionSummary,
    joint_stabilization_observations: DistributionSummary,
    episodes: Vec<EpisodePolicyResult>,
}

#[derive(Clone, Debug, Default, Serialize)]
struct TemporalOutcomeSummary {
    stable_correct: usize,
    stable_incorrect: usize,
    conflict: usize,
    unresolved: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
struct TransitionSummary {
    gap_resets: usize,
    conflicts: usize,
    pending_replacements: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
struct DistributionSummary {
    samples: usize,
    minimum: Option<u64>,
    p50: Option<u64>,
    p95: Option<u64>,
    maximum: Option<u64>,
}

#[derive(Debug, Serialize)]
struct EpisodePolicyResult {
    episode_id: String,
    observation_count: usize,
    first_sequence: u64,
    last_sequence: u64,
    song: TemporalOutcome,
    clear_type: TemporalOutcome,
    joint_stable_correct: bool,
    joint_stabilization_ms: Option<u64>,
    joint_stabilization_observations: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum TemporalOutcome {
    StableCorrect,
    StableIncorrect,
    Conflict,
    Unresolved,
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
    label_sha256: String,
}

#[derive(Debug, Deserialize)]
struct CaptureSession {
    schema: String,
    canonical_frames: Vec<CanonicalFrame>,
    artifacts: Vec<CorpusArtifact>,
}

#[derive(Debug, Deserialize)]
struct CanonicalFrame {
    sequence: u64,
}

#[derive(Debug, Deserialize)]
struct CorpusArtifact {
    source_path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Deserialize)]
struct RegressionLabel {
    schema: String,
    session_sha256: String,
    disposition: LabelDisposition,
    episodes: Vec<RegressionEpisode>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LabelDisposition {
    Include,
    Exclude,
}

#[derive(Clone, Debug, Deserialize)]
struct RegressionEpisode {
    episode_id: String,
    expected_song_id: String,
    expected_clear_type: String,
    stable_sequences: Vec<u64>,
}

#[derive(Clone, Debug)]
struct TemporalRecord {
    sequence: u64,
    timestamp_ms: u64,
    screen: ScreenClass,
    song: Option<ScorepeekSongId>,
    clear_type: Option<String>,
    has_result_observation: bool,
}

#[derive(Clone, Debug)]
struct AnalyzableEpisode {
    label: RegressionEpisode,
    expected_song: ScorepeekSongId,
    observations: Vec<TemporalRecord>,
}

#[derive(Clone, Debug)]
struct ResultInterval {
    observations: Vec<TemporalRecord>,
    first_sequence: u64,
    exclusive_end_sequence: Option<u64>,
}

/// Evaluates the production result reducer over the active private corpus suite.
///
/// # Errors
///
/// Returns an error when a policy is invalid, a bound corpus document or observation object is
/// unavailable or differs from its declared identity, or an observation cannot be decoded into the
/// supported temporal input.
pub fn evaluate_temporal_corpus(
    store: &Path,
    policies: &[TemporalEvaluationPolicy],
) -> Result<TemporalEvaluationSummary, CorpusError> {
    let policies = validate_policies(policies)?;
    let active: ActiveSuite = read_json(&store.join("active-suite.json"))?;
    if active.schema != ACTIVE_SCHEMA || !valid_sha256(&active.generation_sha256) {
        return invalid("active temporal evaluation suite is invalid");
    }
    let suite_path = store
        .join("suites")
        .join(format!("{}.json", active.generation_sha256));
    let suite_bytes = read_bounded_regular(&suite_path, MAX_DOCUMENT_BYTES, ErrorContext::Replay)?;
    if digest_bytes(&suite_bytes) != active.generation_sha256 {
        return invalid("active temporal evaluation suite digest differs");
    }
    let suite: RegressionSuite = serde_json::from_slice(&suite_bytes)?;
    if suite.schema != SUITE_SCHEMA {
        return invalid("active temporal evaluation suite schema differs");
    }

    let mut labeled_episode_count = 0;
    let mut analyzable = Vec::new();
    let mut excluded = Vec::new();
    for entry in &suite.entries {
        let session: CaptureSession = read_bound_json(
            &store
                .join("sessions")
                .join(format!("{}.json", entry.session_sha256)),
            &entry.session_sha256,
        )?;
        let label: RegressionLabel = read_bound_json(
            &store
                .join("labels")
                .join(format!("{}.json", entry.label_sha256)),
            &entry.label_sha256,
        )?;
        if session.schema != SESSION_SCHEMA || label.schema != LABEL_SCHEMA {
            return invalid("temporal evaluation suite entry schema differs");
        }
        validate_entry_binding(entry, &session, &label)?;
        labeled_episode_count += label.episodes.len();
        let records = read_observations(store, &session)?;
        bind_episodes(&records, label.episodes, &mut analyzable, &mut excluded)?;
    }

    let raw_observations = summarize_raw(&analyzable);
    let policy_summaries = policies
        .into_iter()
        .map(|policy| evaluate_policy(policy, &analyzable))
        .collect();
    Ok(TemporalEvaluationSummary {
        schema: SUMMARY_SCHEMA,
        generation_sha256: active.generation_sha256,
        session_count: suite.entries.len(),
        labeled_episode_count,
        analyzable_episode_count: analyzable.len(),
        excluded_episodes: excluded,
        raw_observations,
        policies: policy_summaries,
        authority: "offline_descriptive_only",
    })
}

fn validate_entry_binding(
    entry: &SuiteEntry,
    session: &CaptureSession,
    label: &RegressionLabel,
) -> Result<(), CorpusError> {
    let available_sequences = session
        .canonical_frames
        .iter()
        .map(|frame| frame.sequence)
        .collect::<BTreeSet<_>>();
    if label.session_sha256 != entry.session_sha256
        || label.disposition != LabelDisposition::Include
        || label.episodes.iter().any(|episode| {
            episode
                .stable_sequences
                .iter()
                .any(|sequence| !available_sequences.contains(sequence))
        })
    {
        return invalid("temporal evaluation suite entry binding differs");
    }
    Ok(())
}

fn validate_policies(
    policies: &[TemporalEvaluationPolicy],
) -> Result<Vec<TemporalEvaluationPolicy>, CorpusError> {
    if policies.is_empty() || policies.len() > MAX_POLICIES {
        return Err(CorpusError::InvalidRequest(
            "temporal evaluation requires between one and sixteen policies".to_owned(),
        ));
    }
    let unique = policies.iter().copied().collect::<BTreeSet<_>>();
    if unique.len() != policies.len() {
        return Err(CorpusError::InvalidRequest(
            "temporal evaluation policies must be unique".to_owned(),
        ));
    }
    Ok(policies.to_vec())
}

fn bind_episodes(
    records: &[TemporalRecord],
    labels: Vec<RegressionEpisode>,
    analyzable: &mut Vec<AnalyzableEpisode>,
    excluded: &mut Vec<ExcludedEpisode>,
) -> Result<(), CorpusError> {
    let intervals = result_intervals(records);
    let mut claimed = BTreeMap::<usize, String>::new();
    for label in labels {
        if label.stable_sequences.is_empty() {
            return invalid("temporal evaluation label has no stable sequence anchor");
        }
        let expected_song: ScorepeekSongId = serde_json::from_value(Value::String(
            label.expected_song_id.clone(),
        ))
        .map_err(|_| CorpusError::InvalidReplay("expected song ID is invalid".to_owned()))?;
        let matching = intervals
            .iter()
            .enumerate()
            .filter(|(_, interval)| {
                label.stable_sequences.iter().all(|sequence| {
                    interval.first_sequence <= *sequence
                        && interval
                            .exclusive_end_sequence
                            .is_none_or(|end| *sequence < end)
                })
            })
            .collect::<Vec<_>>();
        if matching.len() != 1 {
            excluded.push(ExcludedEpisode {
                episode_id: label.episode_id,
                reason: if matching.is_empty() {
                    ExclusionReason::ResultIntervalUnavailable
                } else {
                    ExclusionReason::AmbiguousResultInterval
                },
                available_result_observations: 0,
            });
            continue;
        }
        let (interval_index, interval) = matching[0];
        if claimed
            .insert(interval_index, label.episode_id.clone())
            .is_some()
        {
            return invalid("multiple labeled episodes claim one result interval");
        }
        let observations = interval
            .observations
            .iter()
            .filter(|record| record.has_result_observation)
            .cloned()
            .collect::<Vec<_>>();
        if observations.len() < 2 {
            excluded.push(ExcludedEpisode {
                episode_id: label.episode_id,
                reason: ExclusionReason::InsufficientTemporalObservations,
                available_result_observations: observations.len(),
            });
            continue;
        }
        analyzable.push(AnalyzableEpisode {
            label,
            expected_song,
            observations,
        });
    }
    Ok(())
}

fn result_intervals(records: &[TemporalRecord]) -> Vec<ResultInterval> {
    let mut intervals = Vec::new();
    let mut current = Vec::new();
    for record in records {
        if record.screen == ScreenClass::Result {
            current.push(record.clone());
        } else if !current.is_empty() {
            intervals.push(ResultInterval {
                first_sequence: current[0].sequence,
                observations: std::mem::take(&mut current),
                exclusive_end_sequence: Some(record.sequence),
            });
        }
    }
    if !current.is_empty() {
        intervals.push(ResultInterval {
            first_sequence: current[0].sequence,
            observations: current,
            exclusive_end_sequence: None,
        });
    }
    intervals
}

fn summarize_raw(episodes: &[AnalyzableEpisode]) -> RawObservationSummary {
    let mut summary = RawObservationSummary::default();
    for episode in episodes {
        for observation in &episode.observations {
            summary.observations += 1;
            count_raw(
                observation.song.as_ref(),
                &episode.expected_song,
                &mut summary.song,
            );
            count_raw(
                observation.clear_type.as_ref(),
                &episode.label.expected_clear_type,
                &mut summary.clear_type,
            );
        }
    }
    summary
}

fn count_raw<T: Eq>(observed: Option<&T>, expected: &T, summary: &mut RawFieldSummary) {
    match observed {
        Some(value) if value == expected => summary.correct += 1,
        Some(_) => summary.incorrect += 1,
        None => summary.unknown += 1,
    }
}

fn evaluate_policy(
    policy: TemporalEvaluationPolicy,
    episodes: &[AnalyzableEpisode],
) -> TemporalPolicySummary {
    let mut summary = TemporalPolicySummary {
        policy,
        episode_count: episodes.len(),
        song: TemporalOutcomeSummary::default(),
        clear_type: TemporalOutcomeSummary::default(),
        joint_stable_correct: 0,
        transitions: TransitionSummary::default(),
        joint_stabilization_ms: DistributionSummary::default(),
        joint_stabilization_observations: DistributionSummary::default(),
        episodes: Vec::with_capacity(episodes.len()),
    };
    let mut stabilization_ms = Vec::new();
    let mut stabilization_observations = Vec::new();
    for episode in episodes {
        let result = evaluate_episode(policy, episode, &mut summary.transitions);
        count_outcome(result.song, &mut summary.song);
        count_outcome(result.clear_type, &mut summary.clear_type);
        if result.joint_stable_correct {
            summary.joint_stable_correct += 1;
            stabilization_ms.extend(result.joint_stabilization_ms);
            stabilization_observations.extend(result.joint_stabilization_observations);
        }
        summary.episodes.push(result);
    }
    summary.joint_stabilization_ms = distribution(stabilization_ms);
    summary.joint_stabilization_observations = distribution(stabilization_observations);
    summary
}

fn evaluate_episode(
    policy: TemporalEvaluationPolicy,
    episode: &AnalyzableEpisode,
    transitions: &mut TransitionSummary,
) -> EpisodePolicyResult {
    let mut reducer = policy.reducer();
    let first_timestamp = episode.observations[0].timestamp_ms;
    let mut first_joint_stable = None;
    for (index, observation) in episode.observations.iter().enumerate() {
        if let Some(update) = reducer.observe_result(
            observation.sequence,
            observation.timestamp_ms,
            observation.song,
            observation.clear_type.clone(),
        ) {
            for transition in update.transitions {
                match transition.reason {
                    TemporalTransitionReason::ResetByGap => transitions.gap_resets += 1,
                    TemporalTransitionReason::Conflict => transitions.conflicts += 1,
                    TemporalTransitionReason::PendingReplaced => {
                        transitions.pending_replacements += 1;
                    }
                    TemporalTransitionReason::PendingStarted
                    | TemporalTransitionReason::PendingAdvanced
                    | TemporalTransitionReason::Stabilized
                    | TemporalTransitionReason::PendingClearedByUnknown
                    | TemporalTransitionReason::ResetByScreenChange
                    | TemporalTransitionReason::ResetBySessionBoundary => {}
                }
            }
        }
        if first_joint_stable.is_none()
            && reducer.state().song.stable_value() == Some(&episode.expected_song)
            && reducer.state().clear_type.stable_value() == Some(&episode.label.expected_clear_type)
        {
            first_joint_stable = Some((
                observation.timestamp_ms.saturating_sub(first_timestamp),
                u64::try_from(index + 1).unwrap_or(u64::MAX),
            ));
        }
    }
    let song = field_outcome(&reducer.state().song, &episode.expected_song);
    let clear_type = field_outcome(
        &reducer.state().clear_type,
        &episode.label.expected_clear_type,
    );
    let joint_stable_correct =
        song == TemporalOutcome::StableCorrect && clear_type == TemporalOutcome::StableCorrect;
    let (joint_stabilization_ms, joint_stabilization_observations) = if joint_stable_correct {
        first_joint_stable.map_or((None, None), |(milliseconds, observations)| {
            (Some(milliseconds), Some(observations))
        })
    } else {
        (None, None)
    };
    let first_sequence = episode.observations[0].sequence;
    let last_sequence = episode.observations[episode.observations.len() - 1].sequence;
    EpisodePolicyResult {
        episode_id: episode.label.episode_id.clone(),
        observation_count: episode.observations.len(),
        first_sequence,
        last_sequence,
        song,
        clear_type,
        joint_stable_correct,
        joint_stabilization_ms,
        joint_stabilization_observations,
    }
}

fn field_outcome<T: Eq>(state: &TemporalFieldState<T>, expected: &T) -> TemporalOutcome {
    match state {
        TemporalFieldState::Stable { value, .. } if value == expected => {
            TemporalOutcome::StableCorrect
        }
        TemporalFieldState::Stable { .. } => TemporalOutcome::StableIncorrect,
        TemporalFieldState::Conflict { .. } => TemporalOutcome::Conflict,
        TemporalFieldState::Empty | TemporalFieldState::Pending { .. } => {
            TemporalOutcome::Unresolved
        }
    }
}

fn count_outcome(outcome: TemporalOutcome, summary: &mut TemporalOutcomeSummary) {
    match outcome {
        TemporalOutcome::StableCorrect => summary.stable_correct += 1,
        TemporalOutcome::StableIncorrect => summary.stable_incorrect += 1,
        TemporalOutcome::Conflict => summary.conflict += 1,
        TemporalOutcome::Unresolved => summary.unresolved += 1,
    }
}

fn distribution(mut values: Vec<u64>) -> DistributionSummary {
    if values.is_empty() {
        return DistributionSummary::default();
    }
    values.sort_unstable();
    DistributionSummary {
        samples: values.len(),
        minimum: values.first().copied(),
        p50: percentile(&values, 50),
        p95: percentile(&values, 95),
        maximum: values.last().copied(),
    }
}

fn percentile(values: &[u64], percentile: usize) -> Option<u64> {
    let rank = values.len().saturating_mul(percentile).saturating_add(99) / 100;
    values.get(rank.saturating_sub(1)).copied()
}

fn read_observations(
    store: &Path,
    session: &CaptureSession,
) -> Result<Vec<TemporalRecord>, CorpusError> {
    let artifact = session
        .artifacts
        .iter()
        .find(|artifact| artifact.source_path == "recognition/observations.ndjson")
        .ok_or_else(|| {
            CorpusError::InvalidReplay(
                "temporal evaluation observation artifact is unavailable".to_owned(),
            )
        })?;
    if artifact.bytes == 0
        || artifact.bytes > MAX_OBSERVATION_BYTES
        || !valid_sha256(&artifact.sha256)
    {
        return invalid("temporal evaluation observation artifact binding is invalid");
    }
    let path = store.join("objects").join(&artifact.sha256);
    let metadata = path.metadata()?;
    if !metadata.is_file() || metadata.len() != artifact.bytes {
        return invalid("temporal evaluation observation artifact size differs");
    }
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = Vec::new();
    let mut records = Vec::new();
    let mut hasher = Sha256::new();
    while records.len() < MAX_OBSERVATIONS && read_line(&mut reader, &mut line)? {
        hasher.update(&line);
        let value: Value = serde_json::from_slice(&line)?;
        records.push(parse_record(&value)?);
    }
    if read_line(&mut reader, &mut line)? {
        return invalid("temporal evaluation observation count exceeds its bound");
    }
    if encode_digest(hasher.finalize()) != artifact.sha256 {
        return invalid("temporal evaluation observation artifact digest differs");
    }
    if records.is_empty()
        || records
            .windows(2)
            .any(|pair| pair[0].sequence >= pair[1].sequence)
    {
        return invalid("temporal evaluation observation order is invalid");
    }
    Ok(records)
}

fn read_line(reader: &mut BufReader<File>, line: &mut Vec<u8>) -> Result<bool, CorpusError> {
    line.clear();
    let read = reader
        .take(u64::try_from(MAX_OBSERVATION_RECORD_BYTES).unwrap_or(u64::MAX) + 1)
        .read_until(b'\n', line)?;
    if read == 0 {
        return Ok(false);
    }
    if read > MAX_OBSERVATION_RECORD_BYTES || line.last() != Some(&b'\n') {
        return invalid("temporal evaluation observation record exceeds its bound");
    }
    Ok(true)
}

fn parse_record(value: &Value) -> Result<TemporalRecord, CorpusError> {
    if value["schema"] != OBSERVATION_SCHEMA {
        return invalid("temporal evaluation observation schema differs");
    }
    let sequence = value["tick_sequence"].as_u64().ok_or_else(|| {
        CorpusError::InvalidReplay("temporal observation sequence is invalid".to_owned())
    })?;
    let timestamp_ms = value["source_timestamp_ms"].as_u64().ok_or_else(|| {
        CorpusError::InvalidReplay("temporal observation timestamp is invalid".to_owned())
    })?;
    let screen_name = value["screen"]
        .as_str()
        .or_else(|| value.pointer("/decision/screen").and_then(Value::as_str))
        .or_else(|| value.pointer("/fields/screen").and_then(Value::as_str))
        .ok_or_else(|| {
            CorpusError::InvalidReplay("temporal observation screen is invalid".to_owned())
        })?;
    let screen = match screen_name {
        "result" => ScreenClass::Result,
        "music_select" => ScreenClass::MusicSelect,
        "decide_transition" => ScreenClass::DecideTransition,
        "play" => ScreenClass::Play,
        "unknown" => ScreenClass::Unknown,
        _ => return invalid("temporal observation screen is unsupported"),
    };
    let decision_screen = value.pointer("/decision/screen").and_then(Value::as_str);
    let has_result_observation = screen == ScreenClass::Result
        && (decision_screen == Some("result")
            || value.get("song_id").and_then(Value::as_str).is_some()
            || value.pointer("/fields/clear_type").is_some());
    let song = if has_result_observation {
        accepted_song(value)?
    } else {
        None
    };
    let clear_type = if has_result_observation {
        observed_clear_type(value)
    } else {
        None
    };
    Ok(TemporalRecord {
        sequence,
        timestamp_ms,
        screen,
        song,
        clear_type,
        has_result_observation,
    })
}

fn accepted_song(value: &Value) -> Result<Option<ScorepeekSongId>, CorpusError> {
    let accepted = value
        .pointer("/decision/resolution/status")
        .and_then(Value::as_str);
    let song = if accepted == Some("accepted") {
        value.pointer("/decision/resolution/selected/song_id")
    } else if value.get("decision").is_none() {
        value.get("song_id")
    } else {
        None
    };
    let Some(song) = song.and_then(Value::as_str) else {
        return Ok(None);
    };
    serde_json::from_value(Value::String(song.to_owned()))
        .map(Some)
        .map_err(|_| CorpusError::InvalidReplay("observed song ID is invalid".to_owned()))
}

fn observed_clear_type(value: &Value) -> Option<String> {
    value
        .pointer("/fields/clear_type")
        .and_then(|clear| {
            clear
                .as_str()
                .or_else(|| clear.get("open_text").and_then(Value::as_str))
        })
        .and_then(scorepeek::recognition_live::screen_field_observer::resolve_clear_type)
        .map(ToOwned::to_owned)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, CorpusError> {
    let bytes = read_bounded_regular(path, MAX_DOCUMENT_BYTES, ErrorContext::Replay)?;
    serde_json::from_slice(&bytes).map_err(CorpusError::Json)
}

fn read_bound_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
    expected_sha256: &str,
) -> Result<T, CorpusError> {
    if !valid_sha256(expected_sha256) {
        return invalid("temporal evaluation document digest is invalid");
    }
    let bytes = read_bounded_regular(path, MAX_DOCUMENT_BYTES, ErrorContext::Replay)?;
    if digest_bytes(&bytes) != expected_sha256 {
        return invalid("temporal evaluation document digest differs");
    }
    serde_json::from_slice(&bytes).map_err(CorpusError::Json)
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
    use std::fs;

    use super::*;

    fn policy(required_observations: u8, maximum_gap_ms: u64) -> TemporalEvaluationPolicy {
        TemporalEvaluationPolicy::new(required_observations, maximum_gap_ms).unwrap()
    }

    fn song(value: &str) -> ScorepeekSongId {
        serde_json::from_value(Value::String(value.to_owned())).unwrap()
    }

    fn observation(
        sequence: u64,
        timestamp_ms: u64,
        song: Option<ScorepeekSongId>,
        clear_type: Option<&str>,
    ) -> TemporalRecord {
        TemporalRecord {
            sequence,
            timestamp_ms,
            screen: ScreenClass::Result,
            song,
            clear_type: clear_type.map(ToOwned::to_owned),
            has_result_observation: true,
        }
    }

    #[test]
    fn policy_comparison_reports_latency_coverage_and_wrong_stability() {
        let expected = song("00000000-0000-0000-0000-000000000001");
        let wrong = song("00000000-0000-0000-0000-000000000002");
        let episodes = vec![
            AnalyzableEpisode {
                label: RegressionEpisode {
                    episode_id: "correct".to_owned(),
                    expected_song_id: expected.as_uuid().to_string(),
                    expected_clear_type: "CLEAR".to_owned(),
                    stable_sequences: vec![1],
                },
                expected_song: expected,
                observations: vec![
                    observation(1, 100, Some(expected), Some("CLEAR")),
                    observation(2, 200, Some(expected), Some("CLEAR")),
                    observation(3, 300, Some(expected), Some("CLEAR")),
                ],
            },
            AnalyzableEpisode {
                label: RegressionEpisode {
                    episode_id: "wrong".to_owned(),
                    expected_song_id: expected.as_uuid().to_string(),
                    expected_clear_type: "CLEAR".to_owned(),
                    stable_sequences: vec![4],
                },
                expected_song: expected,
                observations: vec![
                    observation(4, 400, Some(wrong), Some("CLEAR")),
                    observation(5, 500, Some(wrong), Some("CLEAR")),
                    observation(6, 600, Some(expected), Some("CLEAR")),
                ],
            },
        ];
        let two = evaluate_policy(policy(2, 250), &episodes);
        assert_eq!(two.song.stable_correct, 1);
        assert_eq!(two.song.conflict, 1);
        assert_eq!(two.joint_stable_correct, 1);
        assert_eq!(two.joint_stabilization_ms.p50, Some(100));
        assert_eq!(two.joint_stabilization_observations.p50, Some(2));
        assert_eq!(two.transitions.conflicts, 1);

        let three = evaluate_policy(policy(3, 250), &episodes);
        assert_eq!(three.song.stable_correct, 1);
        assert_eq!(three.song.unresolved, 1);
        assert_eq!(three.joint_stable_correct, 1);
    }

    #[test]
    fn result_intervals_are_split_by_raw_screen_boundaries() {
        let expected = song("00000000-0000-0000-0000-000000000001");
        let mut records = vec![
            observation(1, 100, Some(expected), Some("CLEAR")),
            observation(2, 200, Some(expected), Some("CLEAR")),
        ];
        records.push(TemporalRecord {
            sequence: 3,
            timestamp_ms: 300,
            screen: ScreenClass::Unknown,
            song: None,
            clear_type: None,
            has_result_observation: false,
        });
        records.extend([
            observation(4, 400, Some(expected), Some("CLEAR")),
            observation(5, 500, Some(expected), Some("CLEAR")),
        ]);
        let intervals = result_intervals(&records);
        assert_eq!(intervals.len(), 2);
        assert_eq!(intervals[0].observations.len(), 2);
        assert_eq!(intervals[0].exclusive_end_sequence, Some(3));
        assert_eq!(intervals[1].observations.len(), 2);
        assert_eq!(intervals[1].exclusive_end_sequence, None);
    }

    #[test]
    fn duplicate_or_unbounded_policies_are_rejected() {
        assert!(validate_policies(&[]).is_err());
        assert!(validate_policies(&[policy(2, 250), policy(2, 250)]).is_err());
        assert!(TemporalEvaluationPolicy::new(17, 250).is_err());
        assert!(TemporalEvaluationPolicy::new(2, 60_001).is_err());
    }

    #[test]
    fn mixed_observation_v5_shapes_preserve_only_accepted_result_values() {
        let expected = "00000000-0000-0000-0000-000000000001";
        let current = serde_json::json!({
            "schema": OBSERVATION_SCHEMA,
            "tick_sequence": 7,
            "source_timestamp_ms": 700,
            "screen": "result",
            "fields": {"clear_type": {"open_text": "EXH-CLEAR"}},
            "decision": {
                "screen": "result",
                "resolution": {
                    "status": "accepted",
                    "selected": {"song_id": expected}
                }
            }
        });
        let parsed = parse_record(&current).unwrap();
        assert_eq!(parsed.song, Some(song(expected)));
        assert_eq!(parsed.clear_type.as_deref(), Some("EXH-CLEAR"));

        let unknown = serde_json::json!({
            "schema": OBSERVATION_SCHEMA,
            "tick_sequence": 8,
            "source_timestamp_ms": 800,
            "fields": {"screen": "result", "clear_type": {"open_text": ""}},
            "decision": {"screen": "result", "resolution": {"status": "unknown"}}
        });
        let parsed = parse_record(&unknown).unwrap();
        assert_eq!(parsed.song, None);
        assert_eq!(parsed.clear_type, None);

        let legacy = serde_json::json!({
            "schema": OBSERVATION_SCHEMA,
            "tick_sequence": 9,
            "source_timestamp_ms": 900,
            "screen": "result",
            "fields": {"clear_type": "CLEAR"},
            "song_id": expected
        });
        let parsed = parse_record(&legacy).unwrap();
        assert_eq!(parsed.song, Some(song(expected)));
        assert_eq!(parsed.clear_type.as_deref(), Some("CLEAR"));

        let placeholder = serde_json::json!({
            "schema": OBSERVATION_SCHEMA,
            "tick_sequence": 10,
            "source_timestamp_ms": 1_000,
            "screen": "result",
            "fields": null,
            "song_id": null
        });
        assert!(!parse_record(&placeholder).unwrap().has_result_observation);
    }

    #[test]
    fn path_screens_are_non_result_interval_boundaries() {
        for (sequence, screen, expected) in [
            (1, "decide_transition", ScreenClass::DecideTransition),
            (2, "play", ScreenClass::Play),
        ] {
            let parsed = parse_record(&serde_json::json!({
                "schema": OBSERVATION_SCHEMA,
                "tick_sequence": sequence,
                "source_timestamp_ms": sequence * 100,
                "screen": screen,
                "fields": null,
                "song_id": null
            }))
            .unwrap();
            assert_eq!(parsed.screen, expected);
            assert!(!parsed.has_result_observation);
        }
    }

    #[test]
    fn predicate_placeholder_does_not_hide_a_production_gap_reset() {
        let expected = song("00000000-0000-0000-0000-000000000001");
        let actual = |sequence, timestamp_ms| {
            observation(sequence, timestamp_ms, Some(expected), Some("CLEAR"))
        };
        let placeholder = parse_record(&serde_json::json!({
            "schema": OBSERVATION_SCHEMA,
            "tick_sequence": 3,
            "source_timestamp_ms": 300,
            "screen": "result",
            "fields": null,
            "song_id": null
        }))
        .unwrap();
        let observations = [actual(1, 100), actual(2, 200), placeholder, actual(4, 500)]
            .into_iter()
            .filter(|record| record.has_result_observation)
            .collect();
        let episode = AnalyzableEpisode {
            label: RegressionEpisode {
                episode_id: "gap".to_owned(),
                expected_song_id: expected.as_uuid().to_string(),
                expected_clear_type: "CLEAR".to_owned(),
                stable_sequences: vec![1],
            },
            expected_song: expected,
            observations,
        };
        let summary = evaluate_policy(policy(2, 250), &[episode]);
        assert_eq!(summary.song.unresolved, 1);
        assert_eq!(summary.clear_type.unresolved, 1);
        assert_eq!(summary.transitions.gap_resets, 2);
    }

    #[test]
    fn suite_entry_binding_requires_include_label_and_available_stable_frames() {
        let entry = SuiteEntry {
            session_sha256: "a".repeat(64),
            label_sha256: "b".repeat(64),
        };
        let session = CaptureSession {
            schema: SESSION_SCHEMA.to_owned(),
            canonical_frames: vec![CanonicalFrame { sequence: 7 }],
            artifacts: Vec::new(),
        };
        let label = |session_sha256: String, disposition, stable_sequences| RegressionLabel {
            schema: LABEL_SCHEMA.to_owned(),
            session_sha256,
            disposition,
            episodes: vec![RegressionEpisode {
                episode_id: "episode".to_owned(),
                expected_song_id: "00000000-0000-0000-0000-000000000001".to_owned(),
                expected_clear_type: "CLEAR".to_owned(),
                stable_sequences,
            }],
        };
        assert!(
            validate_entry_binding(
                &entry,
                &session,
                &label(
                    entry.session_sha256.clone(),
                    LabelDisposition::Include,
                    vec![7]
                ),
            )
            .is_ok()
        );
        assert!(
            validate_entry_binding(
                &entry,
                &session,
                &label("c".repeat(64), LabelDisposition::Include, vec![7]),
            )
            .is_err()
        );
        assert!(
            validate_entry_binding(
                &entry,
                &session,
                &label(
                    entry.session_sha256.clone(),
                    LabelDisposition::Exclude,
                    vec![7]
                ),
            )
            .is_err()
        );
        assert!(
            validate_entry_binding(
                &entry,
                &session,
                &label(
                    entry.session_sha256.clone(),
                    LabelDisposition::Include,
                    vec![8]
                ),
            )
            .is_err()
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn active_suite_boundary_is_digest_bound_and_evaluated() {
        let root = tempfile::tempdir().unwrap();
        for directory in ["objects", "sessions", "labels", "suites"] {
            fs::create_dir(root.path().join(directory)).unwrap();
        }
        let expected = "00000000-0000-0000-0000-000000000001";
        let mut observations = Vec::new();
        for sequence in 1..=3 {
            let record = serde_json::json!({
                "schema": OBSERVATION_SCHEMA,
                "tick_sequence": sequence,
                "source_timestamp_ms": sequence * 100,
                "screen": "result",
                "fields": {"clear_type": {"open_text": "CLEAR"}},
                "decision": {
                    "screen": "result",
                    "resolution": {
                        "status": "accepted",
                        "selected": {"song_id": expected}
                    }
                }
            });
            observations.extend(serde_json::to_vec(&record).unwrap());
            observations.push(b'\n');
        }
        let observation_sha256 = digest_bytes(&observations);
        fs::write(
            root.path().join("objects").join(&observation_sha256),
            &observations,
        )
        .unwrap();

        let session = serde_json::json!({
            "schema": SESSION_SCHEMA,
            "canonical_frames": [{"sequence": 2}],
            "artifacts": [{
                "source_path": "recognition/observations.ndjson",
                "sha256": observation_sha256,
                "bytes": observations.len()
            }]
        });
        let session_bytes = serde_json::to_vec(&session).unwrap();
        let session_sha256 = digest_bytes(&session_bytes);
        fs::write(
            root.path()
                .join("sessions")
                .join(format!("{session_sha256}.json")),
            session_bytes,
        )
        .unwrap();

        let label = serde_json::json!({
            "schema": LABEL_SCHEMA,
            "session_sha256": session_sha256,
            "disposition": "include",
            "episodes": [{
                "episode_id": "episode-1",
                "expected_song_id": expected,
                "expected_clear_type": "CLEAR",
                "stable_sequences": [2]
            }]
        });
        let label_bytes = serde_json::to_vec(&label).unwrap();
        let label_sha256 = digest_bytes(&label_bytes);
        fs::write(
            root.path()
                .join("labels")
                .join(format!("{label_sha256}.json")),
            label_bytes,
        )
        .unwrap();

        let suite = serde_json::json!({
            "schema": SUITE_SCHEMA,
            "entries": [{
                "session_sha256": session_sha256,
                "label_sha256": label_sha256
            }]
        });
        let suite_bytes = serde_json::to_vec(&suite).unwrap();
        let generation_sha256 = digest_bytes(&suite_bytes);
        fs::write(
            root.path()
                .join("suites")
                .join(format!("{generation_sha256}.json")),
            suite_bytes,
        )
        .unwrap();
        fs::write(
            root.path().join("active-suite.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": ACTIVE_SCHEMA,
                "generation_sha256": generation_sha256
            }))
            .unwrap(),
        )
        .unwrap();

        let summary = evaluate_temporal_corpus(root.path(), &[policy(2, 250)]).unwrap();
        assert_eq!(summary.session_count, 1);
        assert_eq!(summary.analyzable_episode_count, 1);
        assert_eq!(summary.raw_observations.observations, 3);
        assert_eq!(summary.policies[0].joint_stable_correct, 1);
        assert_eq!(summary.policies[0].joint_stabilization_ms.p50, Some(100));

        fs::write(
            root.path().join("objects").join(observation_sha256),
            b"changed\n",
        )
        .unwrap();
        assert!(evaluate_temporal_corpus(root.path(), &[policy(2, 250)]).is_err());
    }
}
