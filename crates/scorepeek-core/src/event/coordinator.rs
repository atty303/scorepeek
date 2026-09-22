//! Explicit state transition and bounded output coordination for domain events.

use super::{ReducedRunEvents, RunEvent, RunEventReducer, RunReducerSnapshot};
use crate::game_version::GameVersionState;

#[derive(Clone, Copy, Debug)]
pub struct CoordinatorPolicy {
    pub maximum_effects_per_input: usize,
}

impl Default for CoordinatorPolicy {
    fn default() -> Self {
        Self {
            maximum_effects_per_input: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinatorError {
    InvalidPolicy,
    SequenceOrder,
    Finished,
    OutputLimit,
    VersionAlreadySet,
}

/// A product-state value. Cloning the previous state before reduction ensures an error leaves
/// the caller's state unchanged; ORT and scheduling state are deliberately outside this value.
#[derive(Clone, Default)]
pub struct DomainState {
    reducer: RunEventReducer,
    last_input_sequence: Option<u64>,
    finished: bool,
    game_version: Option<GameVersionState>,
}

impl DomainState {
    #[must_use]
    pub fn snapshot(&self) -> RunReducerSnapshot {
        self.reducer.snapshot()
    }
    #[must_use]
    pub const fn last_input_sequence(&self) -> Option<u64> {
        self.last_input_sequence
    }
    #[must_use]
    pub const fn is_finished(&self) -> bool {
        self.finished
    }
    #[must_use]
    pub const fn game_version(&self) -> Option<&GameVersionState> {
        self.game_version.as_ref()
    }
}

pub struct DomainTransition {
    pub next: DomainState,
    pub outputs: ReducedRunEvents,
}

/// Reduces one input from the supplied previous state. No state is hidden between calls.
///
/// # Errors
/// Rejects policy, order, finish, or output bound violations without changing `previous`.
pub fn reduce_transition(
    previous: &DomainState,
    input_sequence: u64,
    input: &RunEvent,
    policy: CoordinatorPolicy,
) -> Result<DomainTransition, CoordinatorError> {
    if policy.maximum_effects_per_input == 0 || policy.maximum_effects_per_input > 64 {
        return Err(CoordinatorError::InvalidPolicy);
    }
    if previous.finished {
        return Err(CoordinatorError::Finished);
    }
    if previous
        .last_input_sequence
        .is_some_and(|last| input_sequence <= last)
    {
        return Err(CoordinatorError::SequenceOrder);
    }
    let mut next = previous.clone();
    let outputs = match next.reducer.reduce(input) {
        Ok(value) => value,
        Err(never) => match never {},
    };
    if outputs.effects().len() > policy.maximum_effects_per_input {
        return Err(CoordinatorError::OutputLimit);
    }
    next.last_input_sequence = Some(input_sequence);
    if let super::RunEventKind::GameVersionChanged { version, .. } = &input.kind {
        next.game_version = Some(GameVersionState::Identified(version.clone()));
    }
    if matches!(
        input.kind,
        super::RunEventKind::WatcherStopped { .. }
            | super::RunEventKind::CanonicalSessionFinished { .. }
    ) {
        next.finished = true;
    }
    Ok(DomainTransition { next, outputs })
}

/// Applies the externally observed recording version as one explicit canonical input.
/// This preserves all four recorded states without rerunning title OCR or guessing a default.
///
/// # Errors
/// Rejects a duplicate version input, finished state, or unordered input without changing state.
pub fn reduce_canonical_game_version(
    previous: &DomainState,
    input_sequence: u64,
    game_version: &GameVersionState,
) -> Result<DomainTransition, CoordinatorError> {
    if previous.finished {
        return Err(CoordinatorError::Finished);
    }
    if previous.game_version.is_some() {
        return Err(CoordinatorError::VersionAlreadySet);
    }
    if previous
        .last_input_sequence
        .is_some_and(|last| input_sequence <= last)
    {
        return Err(CoordinatorError::SequenceOrder);
    }
    let mut next = previous.clone();
    next.game_version = Some(game_version.clone());
    next.last_input_sequence = Some(input_sequence);
    Ok(DomainTransition {
        next,
        outputs: ReducedRunEvents::default(),
    })
}

/// Thin live/replay coordinator holding only explicit domain state and policy.
pub struct DomainCoordinator {
    state: DomainState,
    policy: CoordinatorPolicy,
}

impl DomainCoordinator {
    /// # Errors
    /// Rejects a policy outside the bounded output contract.
    pub fn new(policy: CoordinatorPolicy) -> Result<Self, CoordinatorError> {
        if policy.maximum_effects_per_input == 0 || policy.maximum_effects_per_input > 64 {
            return Err(CoordinatorError::InvalidPolicy);
        }
        Ok(Self {
            state: DomainState::default(),
            policy,
        })
    }

    /// # Errors
    /// Preserves the previous state when order, finish, or output bounds fail.
    pub fn step(
        &mut self,
        input_sequence: u64,
        input: &RunEvent,
    ) -> Result<ReducedRunEvents, CoordinatorError> {
        let transition = reduce_transition(&self.state, input_sequence, input, self.policy)?;
        self.state = transition.next;
        Ok(transition.outputs)
    }

    /// # Errors
    /// Preserves state on duplicate or unordered canonical version input.
    pub fn step_canonical_game_version(
        &mut self,
        input_sequence: u64,
        game_version: &GameVersionState,
    ) -> Result<ReducedRunEvents, CoordinatorError> {
        let transition = reduce_canonical_game_version(&self.state, input_sequence, game_version)?;
        self.state = transition.next;
        Ok(transition.outputs)
    }

    #[must_use]
    pub const fn state(&self) -> &DomainState {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{RUN_EVENT_SCHEMA, RunEventKind};

    fn event(kind: RunEventKind) -> RunEvent {
        RunEvent {
            schema: RUN_EVENT_SCHEMA.into(),
            kind,
        }
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one ordered scenario covers coordinator conformance"
    )]
    fn pure_transition_and_coordinator_match_for_same_ordered_input() {
        let inputs = [
            event(RunEventKind::WatcherStarted {
                invocation_id: "synthetic".into(),
            }),
            event(RunEventKind::SessionStarted {
                session_id: Some("session-1".into()),
                capture_generation: 1,
                capture_profile_sha256: "a".repeat(64),
                normalizer_artifact_sha256: "b".repeat(64),
            }),
            event(RunEventKind::ScreenChanged {
                session_id: Some("session-1".into()),
                capture_generation: Some(1),
                screen_episode_id: 1,
                sequence: 3,
                monotonic_start_ms: 300,
                monotonic_end_ms: 300,
                screen: "music_select".into(),
            }),
            event(RunEventKind::ScreenTick {
                screen_episode_id: 1,
                sequence: 4,
                monotonic_end_ms: 400,
                screen: "music_select".into(),
            }),
            event(RunEventKind::SessionFinished {
                session_id: "session-1".into(),
                capture_generation: 1,
                outcome: "complete".into(),
                report: serde_json::Value::Null,
            }),
            event(RunEventKind::SessionStarted {
                session_id: Some("session-2".into()),
                capture_generation: 2,
                capture_profile_sha256: "a".repeat(64),
                normalizer_artifact_sha256: "b".repeat(64),
            }),
            event(RunEventKind::SessionFinished {
                session_id: "session-2".into(),
                capture_generation: 2,
                outcome: "complete".into(),
                report: serde_json::Value::Null,
            }),
            event(RunEventKind::WatcherStopped {
                invocation_id: "synthetic".into(),
                reason: "complete".into(),
            }),
        ];
        let policy = CoordinatorPolicy::default();
        let mut explicit = DomainState::default();
        let mut coordinator = DomainCoordinator::new(policy).unwrap();
        for (index, input) in inputs.iter().enumerate() {
            let sequence = index as u64 + 1;
            let transition = reduce_transition(&explicit, sequence, input, policy).unwrap();
            let coordinated = coordinator.step(sequence, input).unwrap();
            assert_eq!(
                serde_json::to_value(
                    transition
                        .outputs
                        .into_effects()
                        .iter()
                        .map(format_effect)
                        .collect::<Vec<_>>()
                )
                .unwrap(),
                serde_json::to_value(
                    coordinated
                        .into_effects()
                        .iter()
                        .map(format_effect)
                        .collect::<Vec<_>>()
                )
                .unwrap()
            );
            explicit = transition.next;
            assert_eq!(
                serde_json::to_value(explicit.snapshot()).unwrap(),
                serde_json::to_value(coordinator.state().snapshot()).unwrap()
            );
            if index == 2 {
                let unchanged = serde_json::to_value(coordinator.state().snapshot()).unwrap();
                assert!(matches!(
                    coordinator.step(sequence, input),
                    Err(CoordinatorError::SequenceOrder)
                ));
                assert_eq!(
                    serde_json::to_value(coordinator.state().snapshot()).unwrap(),
                    unchanged
                );
            }
        }
        assert!(explicit.is_finished());
        let before = serde_json::to_value(coordinator.state().snapshot()).unwrap();
        assert!(matches!(
            coordinator.step(9, &inputs[0]),
            Err(CoordinatorError::Finished)
        ));
        assert_eq!(
            serde_json::to_value(coordinator.state().snapshot()).unwrap(),
            before
        );
    }

    #[test]
    fn canonical_game_version_preserves_each_recorded_state_and_rejects_replacement() {
        for state in [
            GameVersionState::Identified("P2D:J:B:A:2026080500".into()),
            GameVersionState::NotObserved,
            GameVersionState::Ambiguous,
            GameVersionState::ObserverFailed,
        ] {
            let mut coordinator = DomainCoordinator::new(CoordinatorPolicy::default()).unwrap();
            let explicit =
                reduce_canonical_game_version(&DomainState::default(), 1, &state).unwrap();
            assert!(
                coordinator
                    .step_canonical_game_version(1, &state)
                    .unwrap()
                    .effects()
                    .is_empty()
            );
            assert_eq!(explicit.next.game_version(), Some(&state));
            assert_eq!(coordinator.state().game_version(), Some(&state));
            assert!(matches!(
                coordinator.step_canonical_game_version(2, &GameVersionState::NotObserved),
                Err(CoordinatorError::VersionAlreadySet)
            ));
            assert_eq!(coordinator.state().game_version(), Some(&state));
        }
    }

    #[test]
    fn canonical_session_boundaries_are_deterministic_without_runtime_binding() {
        let inputs = [
            event(RunEventKind::CanonicalSessionStarted {
                session_id: "canonical-1".into(),
            }),
            event(RunEventKind::CanonicalSessionFinished {
                session_id: "canonical-1".into(),
            }),
        ];
        let mut first = DomainCoordinator::new(CoordinatorPolicy::default()).unwrap();
        let mut second = DomainCoordinator::new(CoordinatorPolicy::default()).unwrap();
        for (index, input) in inputs.iter().enumerate() {
            let sequence = index as u64 + 1;
            let left = first.step(sequence, input).unwrap();
            let right = second.step(sequence, input).unwrap();
            assert_eq!(
                left.effects().iter().map(format_effect).collect::<Vec<_>>(),
                right
                    .effects()
                    .iter()
                    .map(format_effect)
                    .collect::<Vec<_>>()
            );
            assert!(left.effects().len() <= CoordinatorPolicy::default().maximum_effects_per_input);
        }
        assert!(first.state().is_finished());
        assert!(matches!(
            first.step(3, &inputs[0]),
            Err(CoordinatorError::Finished)
        ));
    }

    fn format_effect(effect: &super::super::RunReducerEffect) -> String {
        format!("{effect:?}")
    }
}
