#![allow(
    clippy::wildcard_imports,
    reason = "this file is an implementation partition of its parent reducer authority"
)]

use super::*;

impl RunEventReducer {
    pub(super) fn sync_music_selection(
        &mut self,
        session_id: Option<&String>,
        source_sequence: u64,
    ) -> Result<(), RunEventReductionError> {
        match self.music_select_resolver.selected() {
            Some(state) if self.active_music_selection.as_ref() != Some(&state) => {
                self.publish_music_selection(session_id, source_sequence, state)
            }
            None if matches!(
                self.active_music_selection,
                Some(MusicSelectionState::Selected { .. })
            ) =>
            {
                self.publish_music_selection(
                    session_id,
                    source_sequence,
                    MusicSelectionState::Unresolved {
                        reason: MusicSelectionUnresolvedReason::EvidenceUnresolved,
                    },
                )
            }
            _ => Ok(()),
        }
    }

    pub(super) fn publish_music_selection(
        &mut self,
        session_id: Option<&String>,
        source_sequence: u64,
        state: MusicSelectionState,
    ) -> Result<(), RunEventReductionError> {
        if self.active_music_selection.as_ref() == Some(&state) {
            return Ok(());
        }
        self.music_selection_revision = self.music_selection_revision.saturating_add(1);
        self.publish_one(&RunEvent {
            schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::MusicSelectionChanged {
                session_id: session_id.cloned(),
                screen_episode_id: self.screen_episode_id,
                source_sequence,
                revision: self.music_selection_revision,
                state: state.clone(),
            },
        })?;
        self.active_music_selection = Some(state);
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "one reducer keeps accumulator, diagnostic temporal state, attempt handoff, and output ordering together"
    )]
    pub(super) fn reduce_music_select_observation(
        &mut self,
        session_id: Option<&String>,
        sequence: u64,
        monotonic_end_ms: u64,
        fields: &Value,
        joint_evidence: &JointEvidenceObservation,
        _presentation: &SongResolutionPresentation,
    ) -> Result<(), RunEventReductionError> {
        self.music_select_resolver.observe(
            sequence,
            monotonic_end_ms,
            joint_evidence,
            selected_difficulty(fields),
            selected_play_type(fields),
            selected_play_side(fields),
        );
        let current_observation = !self.semantic_episode_suspended
            && self
                .music_select_resolver
                .accepts_best_observation(sequence);
        let difficulty_transitions = self.engine.selection_epochs.observe_at_with_play_type(
            sequence,
            monotonic_end_ms,
            joint_evidence,
            selected_difficulty(fields),
            selected_play_type(fields),
            selected_play_side(fields),
        );
        for transition in difficulty_transitions {
            self.publish_one(&RunEvent {
                schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::SelectionDifficultyChanged {
                    session_id: session_id.cloned(),
                    screen_episode_id: self.screen_episode_id,
                    source_sequence: sequence,
                    target: transition.target,
                    reason: transition.reason,
                    current: transition.current,
                },
            })?;
        }
        let current_summary = self.engine.selection_epochs.incumbent.summary();
        self.publish_resolver_transition(
            session_id,
            sequence,
            ResolverScope::SelectionIncumbent,
            &current_summary,
            self.engine.selection_epochs.incumbent.observation_count,
        )?;
        if self.engine.selection_epochs.successor.observation_count > 0 {
            let challenger_summary = self.engine.selection_epochs.successor.summary();
            self.publish_resolver_transition(
                session_id,
                sequence,
                ResolverScope::SelectionSuccessor,
                &challenger_summary,
                self.engine.selection_epochs.successor.observation_count,
            )?;
        }
        if current_observation && self.music_selection_episode_active {
            let identity = self
                .music_select_resolver
                .best_frame_identity(fields, joint_evidence);
            let best = serde_json::from_value::<
                crate::recognition::music_select::MusicSelectBestObservation,
            >(fields["best"].clone())
            .unwrap_or_default();
            self.music_select_resolver.best.screen_episode_id = self.screen_episode_id;
            self.music_select_resolver
                .best
                .observe_frame(identity, best.values);
            if let Some(session) = session_id
                && let Some(snapshot) = self.music_select_resolver.best.publish_candidate(
                    session,
                    sequence,
                    monotonic_end_ms,
                )
            {
                self.publish_one(&RunEvent {
                    schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
                    kind: RunEventKind::MusicSelectBestObserved {
                        session_id: session.clone(),
                        snapshot,
                    },
                })?;
            }
        }
        self.sync_music_selection(session_id, sequence)?;
        self.sync_resolver_snapshot(monotonic_end_ms, Some(sequence), Some(fields))?;
        self.refresh()
    }
}
