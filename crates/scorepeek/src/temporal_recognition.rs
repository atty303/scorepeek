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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MusicSelectTemporalPolicy {
    dwell: u64,
    unknown_grace: u64,
    maximum_gap: u64,
}

impl MusicSelectTemporalPolicy {
    /// Creates a bounded music-select temporal policy.
    ///
    /// # Errors
    /// Returns [`TemporalPolicyError`] when any duration is zero.
    pub const fn new(
        dwell_ms: u64,
        unknown_grace_ms: u64,
        maximum_gap_ms: u64,
    ) -> Result<Self, TemporalPolicyError> {
        if dwell_ms == 0 || unknown_grace_ms == 0 || maximum_gap_ms == 0 {
            return Err(TemporalPolicyError);
        }
        Ok(Self {
            dwell: dwell_ms,
            unknown_grace: unknown_grace_ms,
            maximum_gap: maximum_gap_ms,
        })
    }

    #[must_use]
    pub const fn dwell_ms(self) -> u64 {
        self.dwell
    }

    #[must_use]
    pub const fn unknown_grace_ms(self) -> u64 {
        self.unknown_grace
    }

    #[must_use]
    pub const fn maximum_gap_ms(self) -> u64 {
        self.maximum_gap
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MusicSelectTemporalEvidence {
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub first_monotonic_ms: u64,
    pub last_monotonic_ms: u64,
}

impl MusicSelectTemporalEvidence {
    #[must_use]
    pub const fn elapsed_ms(self) -> u64 {
        self.last_monotonic_ms
            .saturating_sub(self.first_monotonic_ms)
    }

    fn advance(&mut self, sequence: u64, monotonic_ms: u64) {
        self.last_sequence = sequence;
        self.last_monotonic_ms = monotonic_ms;
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MusicSelectTemporalState<T> {
    Empty,
    Pending {
        candidate: T,
        evidence: MusicSelectTemporalEvidence,
    },
    Stable {
        value: T,
        evidence: MusicSelectTemporalEvidence,
    },
    HeldUnknown {
        value: T,
        evidence: MusicSelectTemporalEvidence,
        unknown_since_sequence: u64,
        unknown_since_monotonic_ms: u64,
    },
    Changing {
        previous: T,
        previous_evidence: MusicSelectTemporalEvidence,
        candidate: T,
        candidate_evidence: MusicSelectTemporalEvidence,
    },
}

impl<T> MusicSelectTemporalState<T> {
    #[must_use]
    pub const fn confirmed_value(&self) -> Option<&T> {
        match self {
            Self::Stable { value, .. } => Some(value),
            Self::Empty
            | Self::Pending { .. }
            | Self::HeldUnknown { .. }
            | Self::Changing { .. } => None,
        }
    }

    #[must_use]
    pub const fn retained_value(&self) -> Option<&T> {
        match self {
            Self::Stable { value, .. } | Self::HeldUnknown { value, .. } => Some(value),
            Self::Changing { previous, .. } => Some(previous),
            Self::Empty | Self::Pending { .. } => None,
        }
    }

    #[must_use]
    pub const fn candidate_value(&self) -> Option<&T> {
        match self {
            Self::Pending { candidate, .. } | Self::Changing { candidate, .. } => Some(candidate),
            Self::Empty | Self::Stable { .. } | Self::HeldUnknown { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicSelectTemporalTransitionReason {
    PendingStarted,
    PendingAdvanced,
    PendingReplaced,
    PendingClearedByUnknown,
    Stabilized,
    UnknownHeld,
    UnknownGraceExpired,
    ChangePendingStarted,
    ChangePendingAdvanced,
    ChangePendingReplaced,
    ChangeCancelled,
    StableReplaced,
    ResetByGap,
    ResetByScreenChange,
    ResetBySessionBoundary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicSelectTemporalUpdate<T> {
    pub state: MusicSelectTemporalState<T>,
    pub reasons: Vec<MusicSelectTemporalTransitionReason>,
}

#[derive(Clone, Debug)]
pub struct MusicSelectTemporalReducer<T> {
    policy: MusicSelectTemporalPolicy,
    state: MusicSelectTemporalState<T>,
    last_observation_monotonic_ms: Option<u64>,
}

impl<T: Clone + Eq> MusicSelectTemporalReducer<T> {
    #[must_use]
    pub const fn new(policy: MusicSelectTemporalPolicy) -> Self {
        Self {
            policy,
            state: MusicSelectTemporalState::Empty,
            last_observation_monotonic_ms: None,
        }
    }

    #[must_use]
    pub const fn state(&self) -> &MusicSelectTemporalState<T> {
        &self.state
    }

    pub fn observe(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        observed: Option<T>,
    ) -> Option<MusicSelectTemporalUpdate<T>> {
        let mut reasons = Vec::new();
        if self.last_observation_monotonic_ms.is_some_and(|previous| {
            monotonic_ms < previous
                || monotonic_ms.saturating_sub(previous) > self.policy.maximum_gap
        }) && !matches!(self.state, MusicSelectTemporalState::Empty)
        {
            self.state = MusicSelectTemporalState::Empty;
            reasons.push(MusicSelectTemporalTransitionReason::ResetByGap);
        }
        self.last_observation_monotonic_ms = Some(monotonic_ms);
        reduce_music_select(
            &mut self.state,
            self.policy,
            sequence,
            monotonic_ms,
            observed,
            &mut reasons,
        );
        (!reasons.is_empty()).then(|| MusicSelectTemporalUpdate {
            state: self.state.clone(),
            reasons,
        })
    }

    pub fn reset(
        &mut self,
        reason: MusicSelectTemporalTransitionReason,
    ) -> Option<MusicSelectTemporalUpdate<T>> {
        debug_assert!(matches!(
            reason,
            MusicSelectTemporalTransitionReason::ResetByScreenChange
                | MusicSelectTemporalTransitionReason::ResetBySessionBoundary
        ));
        self.last_observation_monotonic_ms = None;
        if matches!(self.state, MusicSelectTemporalState::Empty) {
            return None;
        }
        self.state = MusicSelectTemporalState::Empty;
        Some(MusicSelectTemporalUpdate {
            state: self.state.clone(),
            reasons: vec![reason],
        })
    }
}

fn music_select_evidence(sequence: u64, monotonic_ms: u64) -> MusicSelectTemporalEvidence {
    MusicSelectTemporalEvidence {
        first_sequence: sequence,
        last_sequence: sequence,
        first_monotonic_ms: monotonic_ms,
        last_monotonic_ms: monotonic_ms,
    }
}

fn reduce_music_select<T: Clone + Eq>(
    state: &mut MusicSelectTemporalState<T>,
    policy: MusicSelectTemporalPolicy,
    sequence: u64,
    monotonic_ms: u64,
    observed: Option<T>,
    reasons: &mut Vec<MusicSelectTemporalTransitionReason>,
) {
    match observed {
        Some(observed) => {
            reduce_music_select_accepted(state, policy, sequence, monotonic_ms, observed, reasons);
        }
        None => reduce_music_select_unknown(state, policy, sequence, monotonic_ms, reasons),
    }
}

fn reduce_music_select_unknown<T: Clone>(
    state: &mut MusicSelectTemporalState<T>,
    policy: MusicSelectTemporalPolicy,
    sequence: u64,
    monotonic_ms: u64,
    reasons: &mut Vec<MusicSelectTemporalTransitionReason>,
) {
    match state {
        MusicSelectTemporalState::Pending { .. } => {
            *state = MusicSelectTemporalState::Empty;
            reasons.push(MusicSelectTemporalTransitionReason::PendingClearedByUnknown);
        }
        MusicSelectTemporalState::Stable { value, evidence } => {
            *state = MusicSelectTemporalState::HeldUnknown {
                value: value.clone(),
                evidence: *evidence,
                unknown_since_sequence: sequence,
                unknown_since_monotonic_ms: monotonic_ms,
            };
            reasons.push(MusicSelectTemporalTransitionReason::UnknownHeld);
        }
        MusicSelectTemporalState::HeldUnknown {
            unknown_since_monotonic_ms,
            ..
        } if monotonic_ms.saturating_sub(*unknown_since_monotonic_ms) >= policy.unknown_grace => {
            *state = MusicSelectTemporalState::Empty;
            reasons.push(MusicSelectTemporalTransitionReason::UnknownGraceExpired);
        }
        MusicSelectTemporalState::Changing {
            previous,
            previous_evidence,
            candidate_evidence,
            ..
        } => {
            let unknown_since_monotonic_ms = candidate_evidence.first_monotonic_ms;
            let unknown_since_sequence = candidate_evidence.first_sequence;
            if monotonic_ms.saturating_sub(unknown_since_monotonic_ms) >= policy.unknown_grace {
                *state = MusicSelectTemporalState::Empty;
                reasons.push(MusicSelectTemporalTransitionReason::UnknownGraceExpired);
            } else {
                *state = MusicSelectTemporalState::HeldUnknown {
                    value: previous.clone(),
                    evidence: *previous_evidence,
                    unknown_since_sequence,
                    unknown_since_monotonic_ms,
                };
                reasons.push(MusicSelectTemporalTransitionReason::UnknownHeld);
            }
        }
        MusicSelectTemporalState::Empty | MusicSelectTemporalState::HeldUnknown { .. } => {}
    }
}

fn reduce_music_select_accepted<T: Clone + Eq>(
    state: &mut MusicSelectTemporalState<T>,
    policy: MusicSelectTemporalPolicy,
    sequence: u64,
    monotonic_ms: u64,
    observed: T,
    reasons: &mut Vec<MusicSelectTemporalTransitionReason>,
) {
    match state {
        MusicSelectTemporalState::Empty => {
            *state = MusicSelectTemporalState::Pending {
                candidate: observed,
                evidence: music_select_evidence(sequence, monotonic_ms),
            };
            reasons.push(MusicSelectTemporalTransitionReason::PendingStarted);
        }
        MusicSelectTemporalState::Pending {
            candidate,
            evidence,
        } if *candidate == observed => {
            evidence.advance(sequence, monotonic_ms);
            if evidence.elapsed_ms() >= policy.dwell {
                *state = MusicSelectTemporalState::Stable {
                    value: candidate.clone(),
                    evidence: *evidence,
                };
                reasons.push(MusicSelectTemporalTransitionReason::Stabilized);
            } else {
                reasons.push(MusicSelectTemporalTransitionReason::PendingAdvanced);
            }
        }
        MusicSelectTemporalState::Pending { .. } => {
            *state = MusicSelectTemporalState::Pending {
                candidate: observed,
                evidence: music_select_evidence(sequence, monotonic_ms),
            };
            reasons.push(MusicSelectTemporalTransitionReason::PendingReplaced);
        }
        MusicSelectTemporalState::Stable { value, evidence } if *value == observed => {
            evidence.advance(sequence, monotonic_ms);
        }
        MusicSelectTemporalState::HeldUnknown {
            value, evidence, ..
        } if *value == observed => {
            evidence.advance(sequence, monotonic_ms);
            *state = MusicSelectTemporalState::Stable {
                value: value.clone(),
                evidence: *evidence,
            };
            reasons.push(MusicSelectTemporalTransitionReason::ChangeCancelled);
        }
        MusicSelectTemporalState::Stable { value, evidence }
        | MusicSelectTemporalState::HeldUnknown {
            value, evidence, ..
        } => {
            *state = MusicSelectTemporalState::Changing {
                previous: value.clone(),
                previous_evidence: *evidence,
                candidate: observed,
                candidate_evidence: music_select_evidence(sequence, monotonic_ms),
            };
            reasons.push(MusicSelectTemporalTransitionReason::ChangePendingStarted);
        }
        MusicSelectTemporalState::Changing {
            previous,
            previous_evidence,
            candidate,
            candidate_evidence,
        } if *previous == observed => {
            previous_evidence.advance(sequence, monotonic_ms);
            *state = MusicSelectTemporalState::Stable {
                value: previous.clone(),
                evidence: *previous_evidence,
            };
            reasons.push(MusicSelectTemporalTransitionReason::ChangeCancelled);
        }
        MusicSelectTemporalState::Changing {
            candidate,
            candidate_evidence,
            ..
        } if *candidate == observed => {
            candidate_evidence.advance(sequence, monotonic_ms);
            if candidate_evidence.elapsed_ms() >= policy.dwell {
                *state = MusicSelectTemporalState::Stable {
                    value: candidate.clone(),
                    evidence: *candidate_evidence,
                };
                reasons.push(MusicSelectTemporalTransitionReason::StableReplaced);
            } else {
                reasons.push(MusicSelectTemporalTransitionReason::ChangePendingAdvanced);
            }
        }
        MusicSelectTemporalState::Changing {
            previous,
            previous_evidence,
            ..
        } => {
            *state = MusicSelectTemporalState::Changing {
                previous: previous.clone(),
                previous_evidence: *previous_evidence,
                candidate: observed,
                candidate_evidence: music_select_evidence(sequence, monotonic_ms),
            };
            reasons.push(MusicSelectTemporalTransitionReason::ChangePendingReplaced);
        }
    }
}

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

    fn music_select_reducer() -> MusicSelectTemporalReducer<u8> {
        MusicSelectTemporalReducer::new(MusicSelectTemporalPolicy::new(200, 200, 250).unwrap())
    }

    fn stabilize_music_select(reducer: &mut MusicSelectTemporalReducer<u8>, value: u8) {
        reducer.observe(1, 100, Some(value));
        reducer.observe(2, 200, Some(value));
        reducer.observe(3, 300, Some(value));
        assert_eq!(reducer.state().confirmed_value(), Some(&value));
    }

    #[test]
    fn music_select_holds_one_unknown_and_recovers_without_reacquisition() {
        let mut reducer = music_select_reducer();
        stabilize_music_select(&mut reducer, 7);
        let held = reducer.observe(4, 400, None).unwrap();
        assert!(matches!(
            held.state,
            MusicSelectTemporalState::HeldUnknown { value: 7, .. }
        ));
        assert_eq!(held.state.confirmed_value(), None);
        assert_eq!(held.state.retained_value(), Some(&7));
        let recovered = reducer.observe(5, 500, Some(7)).unwrap();
        assert_eq!(recovered.state.confirmed_value(), Some(&7));
        assert_eq!(
            recovered.reasons,
            vec![MusicSelectTemporalTransitionReason::ChangeCancelled]
        );
    }

    #[test]
    fn music_select_unknown_clears_an_unconfirmed_candidate_without_grace() {
        let mut reducer = music_select_reducer();
        reducer.observe(1, 100, Some(7));
        let cleared = reducer.observe(2, 200, None).unwrap();
        assert_eq!(cleared.state, MusicSelectTemporalState::Empty);
        assert_eq!(
            cleared.reasons,
            vec![MusicSelectTemporalTransitionReason::PendingClearedByUnknown]
        );
    }

    #[test]
    fn music_select_replaces_only_after_the_new_identity_dwells() {
        let mut reducer = music_select_reducer();
        stabilize_music_select(&mut reducer, 7);
        let changing = reducer.observe(4, 400, Some(8)).unwrap();
        assert!(matches!(
            changing.state,
            MusicSelectTemporalState::Changing {
                previous: 7,
                candidate: 8,
                ..
            }
        ));
        assert_eq!(changing.state.confirmed_value(), None);
        assert_eq!(changing.state.retained_value(), Some(&7));
        reducer.observe(5, 500, Some(8));
        let replaced = reducer.observe(6, 600, Some(8)).unwrap();
        assert_eq!(replaced.state.confirmed_value(), Some(&8));
        assert_eq!(
            replaced.reasons,
            vec![MusicSelectTemporalTransitionReason::StableReplaced]
        );
    }

    #[test]
    fn music_select_cancels_a_short_change_and_bounds_unknown_retention() {
        let mut reducer = music_select_reducer();
        stabilize_music_select(&mut reducer, 7);
        reducer.observe(4, 400, Some(8));
        let cancelled = reducer.observe(5, 500, Some(7)).unwrap();
        assert_eq!(cancelled.state.confirmed_value(), Some(&7));
        reducer.observe(6, 600, None);
        let expired = reducer.observe(7, 800, None).unwrap();
        assert_eq!(expired.state, MusicSelectTemporalState::Empty);
        assert_eq!(
            expired.reasons,
            vec![MusicSelectTemporalTransitionReason::UnknownGraceExpired]
        );
    }

    #[test]
    fn music_select_gap_and_explicit_boundaries_reset_state() {
        let mut reducer = music_select_reducer();
        stabilize_music_select(&mut reducer, 7);
        let gap = reducer.observe(4, 551, Some(8)).unwrap();
        assert!(matches!(
            gap.state,
            MusicSelectTemporalState::Pending { candidate: 8, .. }
        ));
        assert_eq!(
            gap.reasons,
            vec![
                MusicSelectTemporalTransitionReason::ResetByGap,
                MusicSelectTemporalTransitionReason::PendingStarted,
            ]
        );
        let reset = reducer
            .reset(MusicSelectTemporalTransitionReason::ResetByScreenChange)
            .unwrap();
        assert_eq!(reset.state, MusicSelectTemporalState::Empty);
    }
}
