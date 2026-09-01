use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayAttemptScreen {
    MusicSelect,
    DecideTransition,
    Play,
    Result,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayAttemptPhase {
    Decided,
    Playing,
    Result,
    Completed,
    Abandoned,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayAttemptResultRelation {
    NotObserved,
    Pending,
    Confirmed,
    Conflict,
    Unlinked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayAttemptReason {
    NoSelectionLinkage,
    DecideNotObserved,
    PlayNotObserved,
    ReturnedToSelect,
    SessionEnded,
    NoActiveAttempt,
    JointIdentityUnresolved,
    ResultEvidenceUnresolved,
    LinkageConflict,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct PlayAttemptPath {
    pub select_observed: bool,
    pub decide_observed: bool,
    pub play_observed: bool,
    pub result_observed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayAttempt {
    pub attempt_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_attempt_id: Option<u64>,
    pub phase: PlayAttemptPhase,
    pub path: PlayAttemptPath,
    pub result_relation: PlayAttemptResultRelation,
    pub reasons: Vec<PlayAttemptReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcceptedPlayAttempt {
    pub attempt_id: u64,
    pub parent_attempt_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PlayAttemptState {
    Idle,
    Attempt {
        attempt: PlayAttempt,
    },
    UnlinkedResult {
        source_sequence: u64,
        reason: PlayAttemptReason,
    },
}

#[derive(Clone, Debug)]
pub struct PlayAttemptReducer {
    next_attempt_id: u64,
    selection_screen_observed: bool,
    state: PlayAttemptState,
}

impl Default for PlayAttemptReducer {
    fn default() -> Self {
        Self {
            next_attempt_id: 1,
            selection_screen_observed: false,
            state: PlayAttemptState::Idle,
        }
    }
}

impl PlayAttemptReducer {
    #[must_use]
    pub const fn state(&self) -> &PlayAttemptState {
        &self.state
    }
    pub fn reset_session(&mut self) {
        *self = Self::default();
    }

    pub fn observe_selection_screen(&mut self) -> Option<PlayAttemptState> {
        let previous = self.state.clone();
        self.selection_screen_observed = true;
        if let PlayAttemptState::Attempt { attempt } = &mut self.state
            && !matches!(
                attempt.phase,
                PlayAttemptPhase::Completed | PlayAttemptPhase::Abandoned
            )
        {
            attempt.phase = PlayAttemptPhase::Abandoned;
            push_reason(&mut attempt.reasons, PlayAttemptReason::ReturnedToSelect);
        }
        (self.state != previous).then(|| self.state.clone())
    }

    pub fn observe_screen(
        &mut self,
        screen: PlayAttemptScreen,
        sequence: u64,
    ) -> Option<PlayAttemptState> {
        let previous = self.state.clone();
        match screen {
            PlayAttemptScreen::MusicSelect => {}
            PlayAttemptScreen::DecideTransition => self.begin_attempt(true),
            PlayAttemptScreen::Play => self.observe_play(),
            PlayAttemptScreen::Result => self.observe_result(sequence),
        }
        (self.state != previous).then(|| self.state.clone())
    }

    pub fn resolve_result_with_reason(
        &mut self,
        rejection: Option<PlayAttemptReason>,
    ) -> Option<PlayAttemptState> {
        let previous = self.state.clone();
        if let PlayAttemptState::Attempt { attempt } = &mut self.state
            && attempt.phase == PlayAttemptPhase::Result
        {
            let rejection = rejection.or_else(|| {
                attempt.reasons.iter().copied().find(|reason| {
                    matches!(
                        reason,
                        PlayAttemptReason::NoSelectionLinkage | PlayAttemptReason::PlayNotObserved
                    )
                })
            });
            attempt.phase = PlayAttemptPhase::Completed;
            attempt.result_relation = if let Some(reason) = rejection {
                push_reason(&mut attempt.reasons, reason);
                PlayAttemptResultRelation::Conflict
            } else {
                PlayAttemptResultRelation::Confirmed
            };
        }
        (self.state != previous).then(|| self.state.clone())
    }

    #[must_use]
    pub fn accepted_result(&self) -> Option<AcceptedPlayAttempt> {
        let PlayAttemptState::Attempt { attempt } = &self.state else {
            return None;
        };
        let linked = attempt.path.select_observed || attempt.parent_attempt_id.is_some();
        (attempt.phase == PlayAttemptPhase::Completed
            && linked
            && attempt.path.play_observed
            && attempt.path.result_observed
            && attempt.result_relation == PlayAttemptResultRelation::Confirmed)
            .then_some(AcceptedPlayAttempt {
                attempt_id: attempt.attempt_id,
                parent_attempt_id: attempt.parent_attempt_id,
            })
    }

    pub fn finish_session(&mut self) -> Option<PlayAttemptState> {
        let previous = self.state.clone();
        if let PlayAttemptState::Attempt { attempt } = &mut self.state
            && !matches!(
                attempt.phase,
                PlayAttemptPhase::Completed | PlayAttemptPhase::Abandoned
            )
        {
            attempt.phase = PlayAttemptPhase::Abandoned;
            push_reason(&mut attempt.reasons, PlayAttemptReason::SessionEnded);
        }
        (self.state != previous).then(|| self.state.clone())
    }

    fn begin_attempt(&mut self, decide_observed: bool) {
        if matches!(
            self.state,
            PlayAttemptState::Attempt {
                attempt: PlayAttempt {
                    phase: PlayAttemptPhase::Decided | PlayAttemptPhase::Playing,
                    ..
                }
            }
        ) {
            return;
        }
        let select_observed = std::mem::take(&mut self.selection_screen_observed);
        let mut reasons = Vec::new();
        if !select_observed {
            reasons.push(PlayAttemptReason::NoSelectionLinkage);
        }
        let attempt_id = self.allocate_attempt_id();
        self.state = PlayAttemptState::Attempt {
            attempt: PlayAttempt {
                attempt_id,
                parent_attempt_id: None,
                phase: PlayAttemptPhase::Decided,
                path: PlayAttemptPath {
                    select_observed,
                    decide_observed,
                    ..PlayAttemptPath::default()
                },
                result_relation: PlayAttemptResultRelation::NotObserved,
                reasons,
            },
        };
    }

    fn observe_play(&mut self) {
        if let PlayAttemptState::Attempt { attempt } = &mut self.state {
            if matches!(
                attempt.phase,
                PlayAttemptPhase::Decided | PlayAttemptPhase::Playing
            ) {
                attempt.phase = PlayAttemptPhase::Playing;
                attempt.path.play_observed = true;
                return;
            }
            if matches!(
                attempt.phase,
                PlayAttemptPhase::Result | PlayAttemptPhase::Completed
            ) && !self.selection_screen_observed
            {
                let parent_attempt_id = Some(attempt.attempt_id);
                let attempt_id = self.allocate_attempt_id();
                self.state = PlayAttemptState::Attempt {
                    attempt: PlayAttempt {
                        attempt_id,
                        parent_attempt_id,
                        phase: PlayAttemptPhase::Playing,
                        path: PlayAttemptPath {
                            select_observed: true,
                            play_observed: true,
                            ..PlayAttemptPath::default()
                        },
                        result_relation: PlayAttemptResultRelation::NotObserved,
                        reasons: vec![PlayAttemptReason::DecideNotObserved],
                    },
                };
                return;
            }
        }
        self.begin_attempt(false);
        if let PlayAttemptState::Attempt { attempt } = &mut self.state {
            attempt.phase = PlayAttemptPhase::Playing;
            attempt.path.play_observed = true;
            push_reason(&mut attempt.reasons, PlayAttemptReason::DecideNotObserved);
        }
    }

    fn observe_result(&mut self, source_sequence: u64) {
        if let PlayAttemptState::Attempt { attempt } = &mut self.state
            && matches!(
                attempt.phase,
                PlayAttemptPhase::Decided | PlayAttemptPhase::Playing
            )
        {
            if !attempt.path.play_observed {
                push_reason(&mut attempt.reasons, PlayAttemptReason::PlayNotObserved);
            }
            attempt.phase = PlayAttemptPhase::Result;
            attempt.path.result_observed = true;
            attempt.result_relation = PlayAttemptResultRelation::Pending;
            return;
        }
        self.state = PlayAttemptState::UnlinkedResult {
            source_sequence,
            reason: PlayAttemptReason::NoActiveAttempt,
        };
    }

    fn allocate_attempt_id(&mut self) -> u64 {
        let id = self.next_attempt_id;
        self.next_attempt_id = self.next_attempt_id.saturating_add(1);
        id
    }
}

fn push_reason(reasons: &mut Vec<PlayAttemptReason>, reason: PlayAttemptReason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_confirmed_only_by_finalization() {
        let mut resolver = PlayAttemptReducer::default();
        resolver.observe_selection_screen();
        resolver.observe_screen(PlayAttemptScreen::Play, 2);
        resolver.observe_screen(PlayAttemptScreen::Result, 3);
        assert!(resolver.accepted_result().is_none());
        resolver.resolve_result_with_reason(None);
        assert_eq!(resolver.accepted_result().unwrap().attempt_id, 1);
    }

    #[test]
    fn direct_retry_inherits_linkage_once() {
        let mut resolver = PlayAttemptReducer::default();
        resolver.observe_selection_screen();
        resolver.observe_screen(PlayAttemptScreen::Play, 2);
        resolver.observe_screen(PlayAttemptScreen::Result, 3);
        resolver.resolve_result_with_reason(None);
        resolver.observe_screen(PlayAttemptScreen::Play, 4);
        resolver.observe_screen(PlayAttemptScreen::Result, 5);
        resolver.resolve_result_with_reason(None);
        assert_eq!(
            resolver.accepted_result().unwrap().parent_attempt_id,
            Some(1)
        );
    }

    #[test]
    fn missing_linkage_or_play_is_rejected() {
        let mut unlinked = PlayAttemptReducer::default();
        unlinked.observe_screen(PlayAttemptScreen::Play, 1);
        unlinked.observe_screen(PlayAttemptScreen::Result, 2);
        unlinked.resolve_result_with_reason(None);
        assert!(unlinked.accepted_result().is_none());
        assert!(matches!(
            unlinked.state(),
            PlayAttemptState::Attempt { attempt }
                if attempt.result_relation == PlayAttemptResultRelation::Conflict
                    && attempt.reasons.contains(&PlayAttemptReason::NoSelectionLinkage)
        ));
        let mut missing_play = PlayAttemptReducer::default();
        missing_play.observe_selection_screen();
        missing_play.observe_screen(PlayAttemptScreen::DecideTransition, 1);
        missing_play.observe_screen(PlayAttemptScreen::Result, 2);
        missing_play.resolve_result_with_reason(None);
        assert!(missing_play.accepted_result().is_none());
        assert!(matches!(
            missing_play.state(),
            PlayAttemptState::Attempt { attempt }
                if attempt.result_relation == PlayAttemptResultRelation::Conflict
                    && attempt.reasons.contains(&PlayAttemptReason::PlayNotObserved)
        ));
    }
}
