use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayAttemptScreen {
    MusicSelect,
    DecideTransition,
    Play,
    Result,
    Unknown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionSource {
    EvidenceAccepted,
    Stable,
    LastConfirmedHeld,
    RetryInherited,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayAttemptPhase {
    Decided,
    Playing,
    Result,
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
    NoStableSelection,
    DecideNotObserved,
    PlayNotObserved,
    ReturnedToSelect,
    SessionEnded,
    NoActiveAttempt,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the observation contract exposes independently observed path phases"
)]
pub struct PlayAttemptPath {
    pub select_observed: bool,
    pub decide_observed: bool,
    pub play_observed: bool,
    pub result_observed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlayAttempt<S> {
    pub attempt_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_attempt_id: Option<u64>,
    pub phase: PlayAttemptPhase,
    pub path: PlayAttemptPath,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection_source: Option<SelectionSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_song: Option<S>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_song: Option<S>,
    pub result_relation: PlayAttemptResultRelation,
    pub reasons: Vec<PlayAttemptReason>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcceptedPlayAttempt<'a, S> {
    pub attempt_id: u64,
    pub parent_attempt_id: Option<u64>,
    pub song: &'a S,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PlayAttemptState<S> {
    Idle,
    Armed {
        source_sequence: u64,
        selection_source: SelectionSource,
        selected_song: S,
    },
    Attempt {
        attempt: PlayAttempt<S>,
    },
    UnlinkedResult {
        source_sequence: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        result_song: Option<S>,
        reason: PlayAttemptReason,
    },
}

#[derive(Clone, Debug)]
struct SelectionHandoff<S> {
    song: S,
    source: SelectionSource,
}

#[derive(Clone, Debug)]
pub struct PlayAttemptReducer<S> {
    next_attempt_id: u64,
    selection_handoff: Option<SelectionHandoff<S>>,
    selection_screen_observed: bool,
    state: PlayAttemptState<S>,
}

impl<S> Default for PlayAttemptReducer<S> {
    fn default() -> Self {
        Self {
            next_attempt_id: 1,
            selection_handoff: None,
            selection_screen_observed: false,
            state: PlayAttemptState::Idle,
        }
    }
}

impl<S: Clone + Eq> PlayAttemptReducer<S> {
    #[must_use]
    pub const fn state(&self) -> &PlayAttemptState<S> {
        &self.state
    }

    pub fn reset_session(&mut self) {
        *self = Self::default();
    }

    pub fn observe_selection_screen(&mut self) {
        self.selection_screen_observed = true;
    }

    #[must_use]
    pub fn accepted_result(&self) -> Option<AcceptedPlayAttempt<'_, S>> {
        let PlayAttemptState::Attempt { attempt } = &self.state else {
            return None;
        };
        if attempt.phase != PlayAttemptPhase::Result
            || !attempt.path.play_observed
            || !attempt.path.result_observed
            || attempt.result_relation != PlayAttemptResultRelation::Confirmed
        {
            return None;
        }
        let selected_song = attempt.selected_song.as_ref()?;
        let result_song = attempt.result_song.as_ref()?;
        if selected_song != result_song {
            return None;
        }
        Some(AcceptedPlayAttempt {
            attempt_id: attempt.attempt_id,
            parent_attempt_id: attempt.parent_attempt_id,
            song: result_song,
        })
    }

    pub fn observe_selection(
        &mut self,
        song: Option<S>,
        source: Option<SelectionSource>,
        source_sequence: u64,
    ) -> Vec<PlayAttemptState<S>> {
        let mut updates = Vec::new();
        self.selection_handoff = song
            .zip(source)
            .map(|(song, source)| SelectionHandoff { song, source });
        if self.selection_handoff.is_some()
            && let PlayAttemptState::Attempt { attempt } = &mut self.state
            && matches!(
                attempt.phase,
                PlayAttemptPhase::Decided | PlayAttemptPhase::Playing
            )
        {
            attempt.phase = PlayAttemptPhase::Abandoned;
            push_reason(&mut attempt.reasons, PlayAttemptReason::ReturnedToSelect);
            updates.push(self.state.clone());
        }
        let previous = self.state.clone();
        match self.selection_handoff.as_ref() {
            Some(handoff) => {
                self.state = PlayAttemptState::Armed {
                    source_sequence,
                    selection_source: handoff.source,
                    selected_song: handoff.song.clone(),
                };
            }
            None if matches!(self.state, PlayAttemptState::Armed { .. }) => {
                self.state = PlayAttemptState::Idle;
            }
            None => {}
        }
        if self.state != previous {
            updates.push(self.state.clone());
        }
        updates
    }

    pub fn observe_screen(
        &mut self,
        screen: PlayAttemptScreen,
        source_sequence: u64,
    ) -> Option<PlayAttemptState<S>> {
        let previous = self.state.clone();
        match screen {
            PlayAttemptScreen::MusicSelect | PlayAttemptScreen::Unknown => {}
            PlayAttemptScreen::DecideTransition => self.begin_decide_attempt(),
            PlayAttemptScreen::Play => self.observe_play(),
            PlayAttemptScreen::Result => self.observe_result(source_sequence),
        }
        (self.state != previous).then(|| self.state.clone())
    }

    pub fn observe_selection_candidate(
        &mut self,
        candidate: Option<&S>,
    ) -> Option<PlayAttemptState<S>> {
        let previous = self.state.clone();
        if let PlayAttemptState::Armed { selected_song, .. } = &self.state
            && candidate.is_some_and(|candidate| candidate != selected_song)
        {
            self.selection_handoff = None;
            self.state = PlayAttemptState::Idle;
        }
        (self.state != previous).then(|| self.state.clone())
    }

    pub fn observe_stable_result(&mut self, song: S) -> Option<PlayAttemptState<S>> {
        let previous = self.state.clone();
        match &mut self.state {
            PlayAttemptState::Attempt { attempt } if attempt.phase == PlayAttemptPhase::Result => {
                if attempt.selected_song.is_none() && attempt.path.select_observed {
                    attempt.selected_song = Some(song.clone());
                    attempt.selection_source = Some(SelectionSource::EvidenceAccepted);
                }
                attempt.result_song = Some(song.clone());
                attempt.result_relation = match attempt.selected_song.as_ref() {
                    Some(selected) if *selected == song => PlayAttemptResultRelation::Confirmed,
                    Some(_) => PlayAttemptResultRelation::Conflict,
                    None => PlayAttemptResultRelation::Unlinked,
                };
            }
            PlayAttemptState::UnlinkedResult { result_song, .. } => {
                *result_song = Some(song);
            }
            PlayAttemptState::Idle
            | PlayAttemptState::Armed { .. }
            | PlayAttemptState::Attempt { .. } => {}
        }
        (self.state != previous).then(|| self.state.clone())
    }

    pub fn finish_session(&mut self) -> Option<PlayAttemptState<S>> {
        let previous = self.state.clone();
        if let PlayAttemptState::Attempt { attempt } = &mut self.state
            && matches!(
                attempt.phase,
                PlayAttemptPhase::Decided | PlayAttemptPhase::Playing
            )
        {
            attempt.phase = PlayAttemptPhase::Abandoned;
            push_reason(&mut attempt.reasons, PlayAttemptReason::SessionEnded);
        }
        self.selection_handoff = None;
        if matches!(self.state, PlayAttemptState::Armed { .. }) {
            self.state = PlayAttemptState::Idle;
        }
        (self.state != previous).then(|| self.state.clone())
    }

    fn begin_decide_attempt(&mut self) {
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
        let handoff = self.selection_handoff.take();
        let select_observed = self.selection_screen_observed || handoff.is_some();
        self.selection_screen_observed = false;
        let mut reasons = Vec::new();
        if handoff.is_none() {
            reasons.push(PlayAttemptReason::NoStableSelection);
        }
        self.state = PlayAttemptState::Attempt {
            attempt: PlayAttempt {
                attempt_id: self.allocate_attempt_id(),
                parent_attempt_id: None,
                phase: PlayAttemptPhase::Decided,
                path: PlayAttemptPath {
                    select_observed,
                    decide_observed: true,
                    ..PlayAttemptPath::default()
                },
                selection_source: handoff.as_ref().map(|value| value.source),
                selected_song: handoff.map(|value| value.song),
                result_song: None,
                result_relation: PlayAttemptResultRelation::NotObserved,
                reasons,
            },
        };
    }

    fn observe_play(&mut self) {
        let retry = match &self.state {
            _ if self.selection_screen_observed => None,
            PlayAttemptState::Attempt { attempt } if attempt.phase == PlayAttemptPhase::Result => {
                Some((
                    Some(attempt.attempt_id),
                    attempt
                        .result_song
                        .clone()
                        .or_else(|| attempt.selected_song.clone()),
                ))
            }
            PlayAttemptState::UnlinkedResult { result_song, .. } => {
                Some((None, result_song.clone()))
            }
            _ => None,
        };
        if let Some((parent_attempt_id, song)) = retry {
            let mut reasons = Vec::new();
            if song.is_none() {
                reasons.push(PlayAttemptReason::NoStableSelection);
            }
            self.state = PlayAttemptState::Attempt {
                attempt: PlayAttempt {
                    attempt_id: self.allocate_attempt_id(),
                    parent_attempt_id,
                    phase: PlayAttemptPhase::Playing,
                    path: PlayAttemptPath {
                        play_observed: true,
                        ..PlayAttemptPath::default()
                    },
                    selection_source: song.as_ref().map(|_| SelectionSource::RetryInherited),
                    selected_song: song,
                    result_song: None,
                    result_relation: PlayAttemptResultRelation::NotObserved,
                    reasons,
                },
            };
            return;
        }

        if let PlayAttemptState::Attempt { attempt } = &mut self.state
            && matches!(
                attempt.phase,
                PlayAttemptPhase::Decided | PlayAttemptPhase::Playing
            )
        {
            attempt.phase = PlayAttemptPhase::Playing;
            attempt.path.play_observed = true;
            return;
        }

        let handoff = self.selection_handoff.take();
        let select_observed = self.selection_screen_observed || handoff.is_some();
        self.selection_screen_observed = false;
        let mut reasons = vec![PlayAttemptReason::DecideNotObserved];
        if handoff.is_none() {
            reasons.push(PlayAttemptReason::NoStableSelection);
        }
        self.state = PlayAttemptState::Attempt {
            attempt: PlayAttempt {
                attempt_id: self.allocate_attempt_id(),
                parent_attempt_id: None,
                phase: PlayAttemptPhase::Playing,
                path: PlayAttemptPath {
                    select_observed,
                    play_observed: true,
                    ..PlayAttemptPath::default()
                },
                selection_source: handoff.as_ref().map(|value| value.source),
                selected_song: handoff.map(|value| value.song),
                result_song: None,
                result_relation: PlayAttemptResultRelation::NotObserved,
                reasons,
            },
        };
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
        if !matches!(
            self.state,
            PlayAttemptState::Attempt {
                attempt: PlayAttempt {
                    phase: PlayAttemptPhase::Result,
                    ..
                }
            }
        ) {
            self.state = PlayAttemptState::UnlinkedResult {
                source_sequence,
                result_song: None,
                reason: PlayAttemptReason::NoActiveAttempt,
            };
        }
    }

    fn allocate_attempt_id(&mut self) -> u64 {
        let attempt_id = self.next_attempt_id;
        self.next_attempt_id = self.next_attempt_id.saturating_add(1);
        attempt_id
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

    fn attempt(state: &PlayAttemptState<u8>) -> &PlayAttempt<u8> {
        let PlayAttemptState::Attempt { attempt } = state else {
            panic!("expected an attempt");
        };
        attempt
    }

    #[test]
    fn stable_selection_is_observable_before_decision_and_survives_unknown() {
        let mut reducer = PlayAttemptReducer::default();
        let updates = reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 9);
        let armed = updates.last().unwrap();
        assert_eq!(
            armed,
            &PlayAttemptState::Armed {
                source_sequence: 9,
                selection_source: SelectionSource::Stable,
                selected_song: 7,
            }
        );
        assert_eq!(reducer.observe_screen(PlayAttemptScreen::Unknown, 10), None);

        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 11);

        let attempt = attempt(reducer.state());
        assert_eq!(attempt.selected_song, Some(7));
        assert_eq!(attempt.selection_source, Some(SelectionSource::Stable));
        assert!(attempt.path.select_observed);
    }

    #[test]
    fn armed_selection_survives_a_transitional_music_select_false_positive() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 9);
        reducer.observe_screen(PlayAttemptScreen::Unknown, 10);
        assert_eq!(
            reducer.observe_screen(PlayAttemptScreen::MusicSelect, 11),
            None
        );
        reducer.observe_screen(PlayAttemptScreen::Unknown, 12);
        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 13);

        let attempt = attempt(reducer.state());
        assert_eq!(attempt.selected_song, Some(7));
        assert_eq!(attempt.selection_source, Some(SelectionSource::Stable));
        assert!(attempt.path.select_observed);
        assert!(
            !attempt
                .reasons
                .contains(&PlayAttemptReason::NoStableSelection)
        );
    }

    #[test]
    fn only_a_different_pending_selection_clears_an_armed_handoff() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 9);
        assert_eq!(reducer.observe_selection_candidate(Some(&7)), None);
        assert_eq!(reducer.observe_selection_candidate(None), None);
        assert_eq!(
            reducer.observe_selection_candidate(Some(&8)),
            Some(PlayAttemptState::Idle)
        );
    }

    #[test]
    fn normal_path_confirms_the_selected_song() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 9);
        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 10);
        reducer.observe_screen(PlayAttemptScreen::Play, 11);
        reducer.observe_screen(PlayAttemptScreen::Unknown, 12);
        assert_eq!(
            reducer.observe_screen(PlayAttemptScreen::MusicSelect, 13),
            None
        );
        reducer.observe_screen(PlayAttemptScreen::Unknown, 14);
        reducer.observe_screen(PlayAttemptScreen::Result, 15);
        reducer.observe_stable_result(7);

        let attempt = attempt(reducer.state());
        assert_eq!(attempt.attempt_id, 1);
        assert_eq!(attempt.phase, PlayAttemptPhase::Result);
        assert_eq!(
            attempt.result_relation,
            PlayAttemptResultRelation::Confirmed
        );
        assert_eq!(
            attempt.path,
            PlayAttemptPath {
                select_observed: true,
                decide_observed: true,
                play_observed: true,
                result_observed: true,
            }
        );
        assert_eq!(
            reducer.accepted_result(),
            Some(AcceptedPlayAttempt {
                attempt_id: 1,
                parent_attempt_id: None,
                song: &7,
            })
        );
    }

    #[test]
    fn missing_decide_still_accepts_play_then_matching_result() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 1);
        reducer.observe_screen(PlayAttemptScreen::Play, 2);
        reducer.observe_screen(PlayAttemptScreen::Result, 3);
        reducer.observe_stable_result(7);

        assert!(reducer.accepted_result().is_some());
    }

    #[test]
    fn observed_select_without_identity_is_linked_by_later_joint_result() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection_screen();
        reducer.observe_screen(PlayAttemptScreen::Play, 10);
        reducer.observe_screen(PlayAttemptScreen::Result, 20);
        reducer.observe_stable_result(7);

        let accepted = reducer.accepted_result().unwrap();
        assert_eq!(*accepted.song, 7);
        let PlayAttemptState::Attempt { attempt } = reducer.state() else {
            panic!("attempt must remain observable");
        };
        assert!(attempt.path.select_observed);
        assert_eq!(
            attempt.result_relation,
            PlayAttemptResultRelation::Confirmed
        );
    }

    #[test]
    fn missing_play_conflict_unlinked_and_abandoned_are_not_accepted() {
        let mut missing_play = PlayAttemptReducer::default();
        missing_play.observe_selection(Some(7), Some(SelectionSource::Stable), 1);
        missing_play.observe_screen(PlayAttemptScreen::DecideTransition, 2);
        missing_play.observe_screen(PlayAttemptScreen::Result, 3);
        missing_play.observe_stable_result(7);
        assert_eq!(missing_play.accepted_result(), None);

        let mut conflict = PlayAttemptReducer::default();
        conflict.observe_selection(Some(7), Some(SelectionSource::Stable), 1);
        conflict.observe_screen(PlayAttemptScreen::Play, 2);
        conflict.observe_screen(PlayAttemptScreen::Result, 3);
        conflict.observe_stable_result(8);
        assert_eq!(conflict.accepted_result(), None);

        let mut unlinked = PlayAttemptReducer::default();
        unlinked.observe_screen(PlayAttemptScreen::Result, 1);
        unlinked.observe_stable_result(7);
        assert_eq!(unlinked.accepted_result(), None);

        let mut abandoned = PlayAttemptReducer::default();
        abandoned.observe_selection(Some(7), Some(SelectionSource::Stable), 1);
        abandoned.observe_screen(PlayAttemptScreen::Play, 2);
        abandoned.observe_selection(Some(8), Some(SelectionSource::Stable), 3);
        assert_eq!(abandoned.accepted_result(), None);
    }

    #[test]
    fn held_selection_is_distinct_and_result_conflict_preserves_both_songs() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::LastConfirmedHeld), 9);
        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 10);
        reducer.observe_screen(PlayAttemptScreen::Play, 11);
        reducer.observe_screen(PlayAttemptScreen::Result, 12);
        reducer.observe_stable_result(8);

        let attempt = attempt(reducer.state());
        assert_eq!(
            attempt.selection_source,
            Some(SelectionSource::LastConfirmedHeld)
        );
        assert_eq!(attempt.selected_song, Some(7));
        assert_eq!(attempt.result_song, Some(8));
        assert_eq!(attempt.result_relation, PlayAttemptResultRelation::Conflict);
    }

    #[test]
    fn result_survives_select_and_pending_until_the_next_selection_is_armed() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 9);
        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 10);
        reducer.observe_screen(PlayAttemptScreen::Play, 11);
        reducer.observe_screen(PlayAttemptScreen::Result, 12);
        reducer.observe_stable_result(7);
        let result = reducer.state().clone();

        assert_eq!(
            reducer.observe_screen(PlayAttemptScreen::MusicSelect, 13),
            None
        );
        assert!(reducer.observe_selection(None, None, 14).is_empty());
        assert_eq!(reducer.state(), &result);

        let updates = reducer.observe_selection(Some(8), Some(SelectionSource::Stable), 15);
        let armed = updates.last().unwrap();
        assert_eq!(
            armed,
            &PlayAttemptState::Armed {
                source_sequence: 15,
                selection_source: SelectionSource::Stable,
                selected_song: 8,
            }
        );
    }

    #[test]
    fn missing_decide_and_result_only_stay_explicitly_incomplete() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 19);
        reducer.observe_screen(PlayAttemptScreen::Play, 20);
        let attempt = attempt(reducer.state());
        assert!(!attempt.path.decide_observed);
        assert!(
            attempt
                .reasons
                .contains(&PlayAttemptReason::DecideNotObserved)
        );

        reducer.reset_session();
        reducer.observe_screen(PlayAttemptScreen::Result, 30);
        assert_eq!(
            reducer.state(),
            &PlayAttemptState::UnlinkedResult {
                source_sequence: 30,
                result_song: None,
                reason: PlayAttemptReason::NoActiveAttempt,
            }
        );
    }

    #[test]
    fn retry_is_a_new_child_attempt_with_inherited_result_song() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 9);
        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 10);
        reducer.observe_screen(PlayAttemptScreen::Play, 11);
        reducer.observe_screen(PlayAttemptScreen::Result, 12);
        reducer.observe_stable_result(8);
        reducer.observe_screen(PlayAttemptScreen::Play, 13);

        let retry = attempt(reducer.state());
        assert_eq!(retry.attempt_id, 2);
        assert_eq!(retry.parent_attempt_id, Some(1));
        assert_eq!(retry.selected_song, Some(8));
        assert_eq!(
            retry.selection_source,
            Some(SelectionSource::RetryInherited)
        );
        assert!(!retry.path.select_observed);
        assert!(retry.path.play_observed);

        reducer.observe_screen(PlayAttemptScreen::Result, 14);
        reducer.observe_stable_result(8);
        assert_eq!(
            reducer.accepted_result(),
            Some(AcceptedPlayAttempt {
                attempt_id: 2,
                parent_attempt_id: Some(1),
                song: &8,
            })
        );
    }

    #[test]
    fn observed_select_after_result_starts_a_fresh_unidentified_attempt() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::EvidenceAccepted), 9);
        reducer.observe_screen(PlayAttemptScreen::Play, 10);
        reducer.observe_screen(PlayAttemptScreen::Result, 11);
        reducer.observe_stable_result(7);

        reducer.observe_selection_screen();
        reducer.observe_screen(PlayAttemptScreen::Play, 12);

        let fresh = attempt(reducer.state());
        assert_eq!(fresh.attempt_id, 2);
        assert_eq!(fresh.parent_attempt_id, None);
        assert_eq!(fresh.selected_song, None);
        assert_eq!(fresh.selection_source, None);
        assert!(fresh.path.select_observed);
        assert!(fresh.path.play_observed);
    }

    #[test]
    fn unknown_between_repeated_decide_does_not_replace_the_active_attempt() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 9);
        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 10);
        reducer.observe_screen(PlayAttemptScreen::Unknown, 11);
        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 12);
        let decided = attempt(reducer.state());
        assert_eq!(decided.attempt_id, 1);
        assert_eq!(decided.selected_song, Some(7));
        assert_eq!(decided.selection_source, Some(SelectionSource::Stable));

        reducer.observe_screen(PlayAttemptScreen::Play, 13);
        reducer.observe_screen(PlayAttemptScreen::Unknown, 14);
        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 15);
        let playing = attempt(reducer.state());
        assert_eq!(playing.attempt_id, 1);
        assert_eq!(playing.phase, PlayAttemptPhase::Playing);
        assert_eq!(playing.selected_song, Some(7));
    }

    #[test]
    fn temporal_selection_abandons_and_rearms_an_incomplete_attempt() {
        let mut reducer = PlayAttemptReducer::default();
        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 10);
        assert_eq!(
            reducer.observe_screen(PlayAttemptScreen::MusicSelect, 11),
            None
        );
        assert_eq!(attempt(reducer.state()).phase, PlayAttemptPhase::Decided);

        let updates = reducer.observe_selection(Some(7), Some(SelectionSource::Stable), 12);
        assert_eq!(updates.len(), 2);
        let abandoned = attempt(&updates[0]);
        assert_eq!(abandoned.phase, PlayAttemptPhase::Abandoned);
        assert!(
            abandoned
                .reasons
                .contains(&PlayAttemptReason::ReturnedToSelect)
        );
        assert_eq!(
            updates[1],
            PlayAttemptState::Armed {
                source_sequence: 12,
                selection_source: SelectionSource::Stable,
                selected_song: 7,
            }
        );

        reducer.observe_screen(PlayAttemptScreen::DecideTransition, 13);
        reducer.finish_session();
        let abandoned = attempt(reducer.state());
        assert_eq!(abandoned.phase, PlayAttemptPhase::Abandoned);
        assert!(abandoned.reasons.contains(&PlayAttemptReason::SessionEnded));
    }
}
