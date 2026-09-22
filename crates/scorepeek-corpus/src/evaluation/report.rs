//! Temporal evaluation report serialization contracts.

use serde::Serialize;

use super::metrics::{
    DistributionSummary, TemporalOutcome, TemporalOutcomeSummary, TransitionSummary,
};
use super::session::TemporalEvaluationPolicy;

#[derive(Debug, Serialize)]
pub struct TemporalEvaluationSummary {
    pub(super) schema: &'static str,
    pub(super) generation_sha256: String,
    pub(super) session_count: usize,
    pub(super) labeled_episode_count: usize,
    pub(super) analyzable_episode_count: usize,
    pub(super) excluded_episodes: Vec<ExcludedEpisode>,
    pub(super) raw_observations: RawObservationSummary,
    pub(super) policies: Vec<TemporalPolicySummary>,
    pub(super) authority: &'static str,
}

#[derive(Debug, Serialize)]
pub(super) struct ExcludedEpisode {
    pub(super) episode_id: String,
    pub(super) reason: ExclusionReason,
    pub(super) available_result_observations: usize,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExclusionReason {
    ResultIntervalUnavailable,
    AmbiguousResultInterval,
    InsufficientTemporalObservations,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct RawObservationSummary {
    pub(super) observations: usize,
    pub(super) song: RawFieldSummary,
    pub(super) clear_type: RawFieldSummary,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct RawFieldSummary {
    pub(super) correct: usize,
    pub(super) incorrect: usize,
    pub(super) unknown: usize,
}

#[derive(Debug, Serialize)]
pub(super) struct TemporalPolicySummary {
    pub(super) policy: TemporalEvaluationPolicy,
    pub(super) episode_count: usize,
    pub(super) song: TemporalOutcomeSummary,
    pub(super) clear_type: TemporalOutcomeSummary,
    pub(super) joint_stable_correct: usize,
    pub(super) transitions: TransitionSummary,
    pub(super) joint_stabilization_ms: DistributionSummary,
    pub(super) joint_stabilization_observations: DistributionSummary,
    pub(super) episodes: Vec<EpisodePolicyResult>,
}

#[derive(Debug, Serialize)]
pub(super) struct EpisodePolicyResult {
    pub(super) episode_id: String,
    pub(super) observation_count: usize,
    pub(super) first_sequence: u64,
    pub(super) last_sequence: u64,
    pub(super) song: TemporalOutcome,
    pub(super) clear_type: TemporalOutcome,
    pub(super) joint_stable_correct: bool,
    pub(super) joint_stabilization_ms: Option<u64>,
    pub(super) joint_stabilization_observations: Option<u64>,
}
