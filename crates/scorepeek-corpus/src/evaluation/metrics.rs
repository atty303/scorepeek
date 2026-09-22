//! Deterministic temporal evaluation metric primitives.

use scorepeek_core::replay::TemporalFieldState;
use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct TemporalOutcomeSummary {
    pub(super) stable_correct: usize,
    pub(super) stable_incorrect: usize,
    pub(super) conflict: usize,
    pub(super) unresolved: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct TransitionSummary {
    pub(super) gap_resets: usize,
    pub(super) conflicts: usize,
    pub(super) pending_replacements: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(super) struct DistributionSummary {
    pub(super) samples: usize,
    pub(super) minimum: Option<u64>,
    pub(super) p50: Option<u64>,
    pub(super) p95: Option<u64>,
    pub(super) maximum: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TemporalOutcome {
    StableCorrect,
    StableIncorrect,
    Conflict,
    Unresolved,
}

pub(super) fn field_outcome<T: Eq>(state: &TemporalFieldState<T>, expected: &T) -> TemporalOutcome {
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

pub(super) fn count_outcome(outcome: TemporalOutcome, summary: &mut TemporalOutcomeSummary) {
    match outcome {
        TemporalOutcome::StableCorrect => summary.stable_correct += 1,
        TemporalOutcome::StableIncorrect => summary.stable_incorrect += 1,
        TemporalOutcome::Conflict => summary.conflict += 1,
        TemporalOutcome::Unresolved => summary.unresolved += 1,
    }
}

pub(super) fn distribution(mut values: Vec<u64>) -> DistributionSummary {
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
