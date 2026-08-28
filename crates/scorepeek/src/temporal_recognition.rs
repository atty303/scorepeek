use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemporalPolicy {
    required_observations: u8,
    maximum_gap_ms: u64,
}

impl TemporalPolicy {
    /// Creates a bounded temporal-evidence policy.
    ///
    /// # Errors
    /// Returns [`TemporalPolicyError`] when fewer than two observations are required or the gap is
    /// zero.
    pub const fn new(
        required_observations: u8,
        maximum_gap_ms: u64,
    ) -> Result<Self, TemporalPolicyError> {
        if required_observations < 2 || maximum_gap_ms == 0 {
            return Err(TemporalPolicyError);
        }
        Ok(Self {
            required_observations,
            maximum_gap_ms,
        })
    }

    #[must_use]
    pub const fn required_observations(self) -> u8 {
        self.required_observations
    }

    #[must_use]
    pub const fn maximum_gap_ms(self) -> u64 {
        self.maximum_gap_ms
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TemporalPolicyError;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TemporalEvidence {
    pub count: u8,
    pub required: u8,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub first_monotonic_ms: u64,
    pub last_monotonic_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TemporalFieldState<T> {
    Empty,
    Pending {
        value: T,
        evidence: TemporalEvidence,
    },
    Stable {
        value: T,
        evidence: TemporalEvidence,
    },
    Conflict {
        stable: T,
        observed: T,
        sequence: u64,
        monotonic_ms: u64,
    },
}

impl<T> TemporalFieldState<T> {
    #[must_use]
    pub const fn stable_value(&self) -> Option<&T> {
        match self {
            Self::Stable { value, .. } => Some(value),
            Self::Empty | Self::Pending { .. } | Self::Conflict { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalTransitionReason {
    PendingStarted,
    PendingAdvanced,
    PendingReplaced,
    Stabilized,
    PendingClearedByUnknown,
    ResetByGap,
    ResetByScreenChange,
    ResetBySessionBoundary,
    Conflict,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TemporalFieldTransition {
    pub field: TemporalField,
    pub reason: TemporalTransitionReason,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalField {
    Song,
    ClearType,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultTemporalState<S> {
    pub song: TemporalFieldState<S>,
    pub clear_type: TemporalFieldState<String>,
}

impl<S> Default for ResultTemporalState<S> {
    fn default() -> Self {
        Self {
            song: TemporalFieldState::Empty,
            clear_type: TemporalFieldState::Empty,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResultTemporalUpdate<S> {
    pub state: ResultTemporalState<S>,
    pub transitions: Vec<TemporalFieldTransition>,
}

#[derive(Clone, Debug)]
pub struct ResultTemporalReducer<S> {
    policy: TemporalPolicy,
    state: ResultTemporalState<S>,
    last_observation_monotonic_ms: Option<u64>,
}

impl<S: Clone + Eq> ResultTemporalReducer<S> {
    #[must_use]
    pub fn new(policy: TemporalPolicy) -> Self {
        Self {
            policy,
            state: ResultTemporalState::default(),
            last_observation_monotonic_ms: None,
        }
    }

    #[must_use]
    pub const fn state(&self) -> &ResultTemporalState<S> {
        &self.state
    }

    pub fn observe_result(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        song: Option<S>,
        clear_type: Option<String>,
    ) -> Option<ResultTemporalUpdate<S>> {
        let mut transitions = Vec::new();
        if self.last_observation_monotonic_ms.is_some_and(|previous| {
            monotonic_ms < previous
                || monotonic_ms.saturating_sub(previous) > self.policy.maximum_gap_ms
        }) {
            reset_field(
                &mut self.state.song,
                TemporalField::Song,
                TemporalTransitionReason::ResetByGap,
                &mut transitions,
            );
            reset_field(
                &mut self.state.clear_type,
                TemporalField::ClearType,
                TemporalTransitionReason::ResetByGap,
                &mut transitions,
            );
        }
        self.last_observation_monotonic_ms = Some(monotonic_ms);
        reduce_field(
            &mut self.state.song,
            self.policy,
            sequence,
            monotonic_ms,
            song,
            TemporalField::Song,
            &mut transitions,
        );
        reduce_field(
            &mut self.state.clear_type,
            self.policy,
            sequence,
            monotonic_ms,
            clear_type,
            TemporalField::ClearType,
            &mut transitions,
        );
        (!transitions.is_empty()).then(|| ResultTemporalUpdate {
            state: self.state.clone(),
            transitions,
        })
    }

    pub fn reset(&mut self, reason: TemporalTransitionReason) -> Option<ResultTemporalUpdate<S>> {
        debug_assert!(matches!(
            reason,
            TemporalTransitionReason::ResetByScreenChange
                | TemporalTransitionReason::ResetBySessionBoundary
        ));
        let mut transitions = Vec::new();
        self.last_observation_monotonic_ms = None;
        reset_field(
            &mut self.state.song,
            TemporalField::Song,
            reason,
            &mut transitions,
        );
        reset_field(
            &mut self.state.clear_type,
            TemporalField::ClearType,
            reason,
            &mut transitions,
        );
        (!transitions.is_empty()).then(|| ResultTemporalUpdate {
            state: self.state.clone(),
            transitions,
        })
    }
}

fn reduce_field<T: Clone + Eq>(
    state: &mut TemporalFieldState<T>,
    policy: TemporalPolicy,
    sequence: u64,
    monotonic_ms: u64,
    observed: Option<T>,
    field: TemporalField,
    transitions: &mut Vec<TemporalFieldTransition>,
) {
    let Some(observed) = observed else {
        if matches!(state, TemporalFieldState::Pending { .. }) {
            *state = TemporalFieldState::Empty;
            transitions.push(TemporalFieldTransition {
                field,
                reason: TemporalTransitionReason::PendingClearedByUnknown,
            });
        }
        return;
    };

    match state {
        TemporalFieldState::Empty => {
            *state = TemporalFieldState::Pending {
                value: observed,
                evidence: TemporalEvidence {
                    count: 1,
                    required: policy.required_observations,
                    first_sequence: sequence,
                    last_sequence: sequence,
                    first_monotonic_ms: monotonic_ms,
                    last_monotonic_ms: monotonic_ms,
                },
            };
            transitions.push(TemporalFieldTransition {
                field,
                reason: TemporalTransitionReason::PendingStarted,
            });
        }
        TemporalFieldState::Pending { value, evidence } if *value == observed => {
            evidence.count = evidence.count.saturating_add(1);
            evidence.last_sequence = sequence;
            evidence.last_monotonic_ms = monotonic_ms;
            let reason = if evidence.count >= policy.required_observations {
                let value = value.clone();
                let evidence = *evidence;
                *state = TemporalFieldState::Stable { value, evidence };
                TemporalTransitionReason::Stabilized
            } else {
                TemporalTransitionReason::PendingAdvanced
            };
            transitions.push(TemporalFieldTransition { field, reason });
        }
        TemporalFieldState::Pending { .. } => {
            *state = TemporalFieldState::Pending {
                value: observed,
                evidence: TemporalEvidence {
                    count: 1,
                    required: policy.required_observations,
                    first_sequence: sequence,
                    last_sequence: sequence,
                    first_monotonic_ms: monotonic_ms,
                    last_monotonic_ms: monotonic_ms,
                },
            };
            transitions.push(TemporalFieldTransition {
                field,
                reason: TemporalTransitionReason::PendingReplaced,
            });
        }
        TemporalFieldState::Stable { value, .. } if *value == observed => {}
        TemporalFieldState::Stable { value, .. } => {
            *state = TemporalFieldState::Conflict {
                stable: value.clone(),
                observed,
                sequence,
                monotonic_ms,
            };
            transitions.push(TemporalFieldTransition {
                field,
                reason: TemporalTransitionReason::Conflict,
            });
        }
        TemporalFieldState::Conflict { .. } => {}
    }
}

fn reset_field<T>(
    state: &mut TemporalFieldState<T>,
    field: TemporalField,
    reason: TemporalTransitionReason,
    transitions: &mut Vec<TemporalFieldTransition>,
) {
    if !matches!(state, TemporalFieldState::Empty) {
        *state = TemporalFieldState::Empty;
        transitions.push(TemporalFieldTransition { field, reason });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reducer() -> ResultTemporalReducer<u8> {
        ResultTemporalReducer::new(TemporalPolicy::new(2, 250).unwrap())
    }

    #[test]
    fn repeated_result_values_stabilize_independently() {
        let mut reducer = reducer();
        reducer.observe_result(1, 100, Some(7), None);
        let update = reducer
            .observe_result(2, 200, Some(7), Some("CLEAR".to_owned()))
            .unwrap();
        assert!(matches!(
            update.state.song,
            TemporalFieldState::Stable { value: 7, .. }
        ));
        assert!(matches!(
            update.state.clear_type,
            TemporalFieldState::Pending { .. }
        ));
        let update = reducer
            .observe_result(3, 300, None, Some("CLEAR".to_owned()))
            .unwrap();
        assert!(matches!(
            update.state.song,
            TemporalFieldState::Stable { value: 7, .. }
        ));
        assert!(matches!(
            update.state.clear_type,
            TemporalFieldState::Stable { .. }
        ));
    }

    #[test]
    fn unknown_clears_pending_but_not_stable() {
        let mut reducer = reducer();
        reducer.observe_result(1, 100, Some(7), None);
        let update = reducer.observe_result(2, 200, None, None).unwrap();
        assert_eq!(update.state.song, TemporalFieldState::Empty);
        reducer.observe_result(3, 300, Some(7), None);
        reducer.observe_result(4, 400, Some(7), None);
        assert!(reducer.observe_result(5, 500, None, None).is_none());
        assert_eq!(reducer.state().song.stable_value(), Some(&7));
    }

    #[test]
    fn different_accepted_value_after_stable_is_a_conflict() {
        let mut reducer = reducer();
        reducer.observe_result(1, 100, Some(7), None);
        reducer.observe_result(2, 200, Some(7), None);
        let update = reducer.observe_result(3, 300, Some(8), None).unwrap();
        assert_eq!(
            update.state.song,
            TemporalFieldState::Conflict {
                stable: 7,
                observed: 8,
                sequence: 3,
                monotonic_ms: 300,
            }
        );
    }

    #[test]
    fn continuous_same_observations_keep_stable_state_past_the_evidence_window() {
        let mut reducer = reducer();
        reducer.observe_result(1, 100, Some(7), None);
        reducer.observe_result(2, 200, Some(7), None);
        for (sequence, time) in [(3, 300), (4, 400), (5, 500)] {
            assert!(
                reducer
                    .observe_result(sequence, time, Some(7), None)
                    .is_none()
            );
        }
        assert_eq!(reducer.state().song.stable_value(), Some(&7));
    }

    #[test]
    fn continuous_unknown_result_observations_keep_stable_state_and_gap_clock_current() {
        let mut reducer = reducer();
        reducer.observe_result(1, 100, Some(7), None);
        reducer.observe_result(2, 200, Some(7), None);
        assert!(reducer.observe_result(3, 300, None, None).is_none());
        assert!(reducer.observe_result(4, 400, None, None).is_none());
        assert!(reducer.observe_result(5, 500, Some(7), None).is_none());
        assert_eq!(reducer.state().song.stable_value(), Some(&7));
    }

    #[test]
    fn gap_or_reversed_time_resets_conflict_before_new_evidence() {
        for time in [551, 250] {
            let mut reducer = reducer();
            reducer.observe_result(1, 100, Some(7), None);
            reducer.observe_result(2, 200, Some(7), None);
            reducer.observe_result(3, 300, Some(8), None);
            let update = reducer.observe_result(4, time, Some(9), None).unwrap();
            assert!(matches!(
                update.state.song,
                TemporalFieldState::Pending {
                    value: 9,
                    evidence: TemporalEvidence { count: 1, .. },
                }
            ));
            assert!(update.transitions.iter().any(|transition| {
                transition.field == TemporalField::Song
                    && transition.reason == TemporalTransitionReason::ResetByGap
            }));
        }
    }

    #[test]
    fn excessive_or_reversed_time_gap_restarts_evidence() {
        for time in [351, 99] {
            let mut reducer = reducer();
            reducer.observe_result(1, 100, Some(7), None);
            let update = reducer.observe_result(2, time, Some(7), None).unwrap();
            assert!(matches!(
                update.state.song,
                TemporalFieldState::Pending {
                    evidence: TemporalEvidence { count: 1, .. },
                    ..
                }
            ));
            assert!(update.transitions.iter().any(|transition| {
                transition.field == TemporalField::Song
                    && transition.reason == TemporalTransitionReason::ResetByGap
            }));
        }
    }

    #[test]
    fn explicit_boundaries_clear_all_temporal_state() {
        let mut reducer = reducer();
        reducer.observe_result(1, 100, Some(7), Some("CLEAR".to_owned()));
        reducer.observe_result(2, 200, Some(7), Some("CLEAR".to_owned()));
        let update = reducer
            .reset(TemporalTransitionReason::ResetBySessionBoundary)
            .unwrap();
        assert_eq!(update.state, ResultTemporalState::default());
        assert_eq!(update.transitions.len(), 2);
    }

    #[test]
    fn invalid_policy_is_rejected() {
        assert_eq!(TemporalPolicy::new(1, 250), Err(TemporalPolicyError));
        assert_eq!(TemporalPolicy::new(2, 0), Err(TemporalPolicyError));
    }
}
