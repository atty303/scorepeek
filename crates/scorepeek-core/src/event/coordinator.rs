//! Explicit state transition and bounded output coordination for domain events.

use super::reducer::DomainReducer;
use super::{DomainInput, DomainSnapshot, ReducedDomainTransitions};
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
    reducer: DomainReducer,
    last_input_sequence: Option<u64>,
    finished: bool,
    game_version: Option<GameVersionState>,
}

impl DomainState {
    #[must_use]
    pub fn snapshot(&self) -> DomainSnapshot {
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
    pub outputs: ReducedDomainTransitions,
}

/// Reduces one input from the supplied previous state. No state is hidden between calls.
///
/// # Errors
/// Rejects policy, order, finish, or output bound violations without changing `previous`.
pub fn reduce_transition(
    previous: &DomainState,
    input_sequence: u64,
    input: &DomainInput,
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
    if matches!(input, DomainInput::SessionStarted { .. }) {
        next.game_version = None;
    }
    if let DomainInput::GameVersionState(game_version) = input {
        if next.game_version.is_some() {
            return Err(CoordinatorError::VersionAlreadySet);
        }
        next.game_version = Some(game_version.clone());
    }
    if matches!(input, DomainInput::WatcherFinished) {
        next.finished = true;
    }
    Ok(DomainTransition { next, outputs })
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
        input: &DomainInput,
    ) -> Result<ReducedDomainTransitions, CoordinatorError> {
        let transition = reduce_transition(&self.state, input_sequence, input, self.policy)?;
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

    #[test]
    fn live_and_replay_boundaries_use_the_same_input_contract() {
        let inputs = [
            DomainInput::SessionStarted {
                session_id: "one".into(),
            },
            DomainInput::SessionFinished {
                session_id: "one".into(),
            },
        ];
        let mut coordinator = DomainCoordinator::new(CoordinatorPolicy::default()).unwrap();
        coordinator.step(1, &inputs[0]).unwrap();
        assert!(coordinator.step(1, &inputs[1]).is_err());
        let outputs = coordinator.step(2, &inputs[1]).unwrap();
        assert!(
            outputs
                .effects()
                .iter()
                .any(|effect| matches!(effect, super::super::DomainEffect::SessionEnded))
        );
        coordinator.step(3, &inputs[0]).unwrap();
        coordinator.step(4, &DomainInput::WatcherFinished).unwrap();
        assert!(coordinator.state().is_finished());
        assert!(matches!(
            coordinator.step(5, &inputs[0]),
            Err(CoordinatorError::Finished)
        ));
    }

    #[test]
    fn all_recorded_game_version_states_survive_without_inference() {
        for state in [
            GameVersionState::Identified("P2D:J:B:A:2026080500".into()),
            GameVersionState::NotObserved,
            GameVersionState::Ambiguous,
            GameVersionState::ObserverFailed,
        ] {
            let mut coordinator = DomainCoordinator::new(CoordinatorPolicy::default()).unwrap();
            coordinator
                .step(
                    1,
                    &DomainInput::SessionStarted {
                        session_id: "one".into(),
                    },
                )
                .unwrap();
            coordinator
                .step(2, &DomainInput::GameVersionState(state.clone()))
                .unwrap();
            assert_eq!(coordinator.state().game_version(), Some(&state));
            assert!(matches!(
                coordinator.step(3, &DomainInput::GameVersionState(state)),
                Err(CoordinatorError::VersionAlreadySet)
            ));
        }
    }
}
