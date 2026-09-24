#![allow(
    clippy::wildcard_imports,
    reason = "this file is an implementation partition of its parent reducer authority"
)]

use super::*;

impl RunEventReducer {
    pub(super) fn publish_internal(
        &mut self,
        event: &RunEvent,
    ) -> Result<(), RunEventReductionError> {
        match &event.kind {
            RunEventKind::CanonicalSessionStarted { session_id } => {
                self.reset_session();
                self.publish_one(event)?;
                self.publish_result_state(session_id.clone(), 0, ResultState::Inactive)?;
                Ok(())
            }
            RunEventKind::SessionStarted { session_id, .. } => {
                self.reset_session();
                self.publish_one(event)?;
                if let Some(session_id) = session_id.clone() {
                    self.publish_result_state(session_id, 0, ResultState::Inactive)?;
                }
                Ok(())
            }
            RunEventKind::FieldObservation { .. } => self.publish_field_observation(event),
            RunEventKind::RawScreenObserved {
                session_id,
                semantic_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
                result_presence,
                ..
            } => {
                self.publish_one(event)?;
                if screen == "result"
                    && let (Some(episode_id), Some(side)) = (
                        *semantic_episode_id,
                        result_presence
                            .as_ref()
                            .and_then(|evidence| evidence.panel_side.known()),
                    )
                {
                    self.observe_result_panel_side(
                        session_id.as_ref(),
                        episode_id,
                        *sequence,
                        side,
                    )?;
                }
                self.publish_screen_tick(*sequence, *monotonic_end_ms)
            }
            RunEventKind::SemanticScreenEpisodeChanged { .. } => {
                self.publish_semantic_screen_episode(event)
            }
            RunEventKind::ScreenChanged { .. } => self.publish_screen_change(event, true),
            RunEventKind::ScreenTick {
                sequence,
                monotonic_end_ms,
                ..
            } => self.publish_screen_tick(*sequence, *monotonic_end_ms),
            RunEventKind::SessionFinished { .. }
            | RunEventKind::CanonicalSessionFinished { .. } => self.publish_session_finished(event),
            RunEventKind::WatcherStarted { .. }
            | RunEventKind::GameVersionChanged { .. }
            | RunEventKind::RecordingHealthChanged { .. }
            | RunEventKind::RecordingFinalizing { .. }
            | RunEventKind::RecordingCompleted { .. }
            | RunEventKind::TemporalResultChanged { .. }
            | RunEventKind::TemporalMusicSelectChanged { .. }
            | RunEventKind::NumericResultChanged { .. }
            | RunEventKind::PlayAttemptChanged { .. }
            | RunEventKind::ResolverStateChanged { .. }
            | RunEventKind::SelectionDifficultyChanged { .. }
            | RunEventKind::MusicSelectionChanged { .. }
            | RunEventKind::MusicSelectBestObserved { .. }
            | RunEventKind::MusicSelectResolverChanged { .. }
            | RunEventKind::ResultChanged { .. }
            | RunEventKind::ResultPanelSideChanged { .. }
            | RunEventKind::ResultSelectContextMismatch { .. }
            | RunEventKind::OverlayObserved { .. } => self.publish_one(event),
            RunEventKind::WatcherStopped { .. } => unreachable!("watcher state is runtime-owned"),
        }
    }

    pub(super) fn observe_result_panel_side(
        &mut self,
        session_id: Option<&String>,
        episode_id: u64,
        sequence: u64,
        side: ResultPanelSide,
    ) -> Result<(), RunEventReductionError> {
        let Some((state, reason)) = self.result_panel_side.observe(episode_id, sequence, side)
        else {
            return Ok(());
        };
        self.publish_one(&RunEvent {
            schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::ResultPanelSideChanged {
                session_id: session_id.cloned(),
                screen_episode_id: episode_id,
                source_sequence: sequence,
                state,
                reason,
            },
        })?;
        if reason == ResultPanelSideTransitionReason::Conflict
            && let Some(session_id) = session_id.cloned()
        {
            self.withdraw_result_provisional(
                session_id,
                sequence,
                ResultRetractionReason::PanelSideConflict,
            )?;
        }
        Ok(())
    }

    pub(super) fn publish_semantic_screen_episode(
        &mut self,
        event: &RunEvent,
    ) -> Result<(), RunEventReductionError> {
        let RunEventKind::SemanticScreenEpisodeChanged {
            session_id,
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            phase,
        } = &event.kind
        else {
            unreachable!("semantic episode dispatcher preserves event kind");
        };
        self.publish_one(event)?;
        if screen == "music_select" && *phase != SemanticEpisodePhase::Started {
            self.music_select_resolver.best_minimum_sequence = *sequence;
            if matches!(
                phase,
                SemanticEpisodePhase::Closing | SemanticEpisodePhase::Finalized
            ) {
                self.music_select_resolver.best_closed = true;
            }
        }
        match phase {
            SemanticEpisodePhase::Started => {
                self.semantic_episode_suspended = false;
                self.clear_field_observation();
                let screen_change = RunEvent {
                    schema: event.schema.clone(),
                    kind: RunEventKind::ScreenChanged {
                        session_id: session_id.clone(),
                        screen_episode_id: *screen_episode_id,
                        sequence: *sequence,
                        monotonic_start_ms: *monotonic_end_ms,
                        monotonic_end_ms: *monotonic_end_ms,
                        screen: screen.clone(),
                    },
                };
                self.publish_screen_change(&screen_change, false)
            }
            SemanticEpisodePhase::Suspended => {
                self.semantic_episode_suspended = true;
                if screen == "music_select" {
                    self.music_select_resolver
                        .best
                        .hold(SelectIdentityStatus::AwaitingEvidence);
                }
                self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), false)?;
                Ok(())
            }
            SemanticEpisodePhase::Resumed => {
                self.semantic_episode_suspended = false;
                self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), false)?;
                Ok(())
            }
            SemanticEpisodePhase::Closing => {
                self.result_episode_finalizing = screen == "result";
                self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), false)?;
                Ok(())
            }
            SemanticEpisodePhase::Finalized => {
                if screen == "music_select" {
                    self.engine.retained_select = self.engine.selection_epochs.handoff();
                    self.publish_music_selection(
                        session_id.as_ref(),
                        *sequence,
                        MusicSelectionState::Unresolved {
                            reason: MusicSelectionUnresolvedReason::EpisodeEnded,
                        },
                    )?;
                    self.music_selection_episode_active = false;
                } else if screen == "result" {
                    self.finalize_result_attempt(session_id.clone(), *sequence)?;
                    self.result_panel_side.clear();
                    self.result_select_context_detached = false;
                }
                self.result_episode_finalizing = false;
                self.semantic_episode_suspended = false;
                self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), false)?;
                Ok(())
            }
        }
    }

    pub(super) fn publish_screen_tick(
        &mut self,
        sequence: u64,
        monotonic_end_ms: u64,
    ) -> Result<(), RunEventReductionError> {
        self.sync_resolver_snapshot(monotonic_end_ms, Some(sequence), false)?;
        Ok(())
    }

    pub(crate) fn finish_watcher(&mut self) -> Result<(), RunEventReductionError> {
        if let Some(state) = self.engine.play_attempt.finish_session() {
            self.publish_play_attempt_update(self.active_session_id.clone(), None, state)?;
        }
        Ok(())
    }

    pub(super) fn publish_field_observation(
        &mut self,
        event: &RunEvent,
    ) -> Result<(), RunEventReductionError> {
        let RunEventKind::FieldObservation {
            session_id,
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            fields,
            parsed_result_fields,
            joint_evidence,
            ..
        } = &event.kind
        else {
            unreachable!("field observation dispatcher preserves event kind");
        };
        self.publish_one(event)?;
        if self
            .latest_screen_boundary_sequence
            .is_some_and(|boundary| *sequence < boundary)
            || (*screen_episode_id != 0 && *screen_episode_id != self.screen_episode_id)
        {
            return Ok(());
        }
        if screen == "result" {
            let panel_side = result_panel_side(fields);
            if let Some(side) = panel_side {
                self.observe_result_panel_side(
                    session_id.as_ref(),
                    *screen_episode_id,
                    *sequence,
                    side,
                )?;
            }
            if panel_side != self.result_panel_side.stable() {
                return Ok(());
            }
        }
        match screen.as_str() {
            "result" => self.reduce_result_observation(
                session_id.as_ref(),
                *sequence,
                *monotonic_end_ms,
                fields,
                parsed_result_fields.as_ref(),
                joint_evidence,
            ),
            "music_select" => self.reduce_music_select_observation(
                session_id.as_ref(),
                *sequence,
                *monotonic_end_ms,
                fields,
                joint_evidence,
            ),
            _ => Ok(()),
        }
    }

    pub(super) fn clear_field_observation(&mut self) {
        self.latest_field_sequence = None;
        self.latest_field_ms = None;
    }

    pub(super) fn reset_numeric_result(&mut self) {
        self.pending_numeric_result = None;
        self.pending_supplemental_result = None;
        self.accepted_numeric_result = None;
        self.last_numeric_sequence = None;
        self.last_numeric_monotonic_ms = None;
    }

    pub(super) fn reset_session(&mut self) {
        self.engine.play_attempt.reset_session();
        self.reset_numeric_result();
        self.active_provisional_result = None;
        self.music_selection_revision = 0;
        self.music_select_resolver = MusicSelectResolver::default();
        self.published_music_select_resolver = MusicSelectResolverState::default();
        self.active_music_selection = None;
        self.music_selection_episode_active = false;
        self.emitted_attempt_ids.clear();
        self.latest_screen_boundary_sequence = None;
        self.screen_episode_id = 0;
        self.screen_episode_started_ms = None;
        self.screen_episode_last_ms = None;
        self.engine = ResolverEngine::default();
        self.result_resolver_active = false;
        self.result_episode_finalizing = false;
        self.semantic_episode_suspended = false;
        self.resolver_transitions.clear();
        self.attempt_started_ms = None;
        self.attempt_phase_started_ms = None;
        self.numeric_evidence.clear();
        self.play_options = PlayOptionsEpisodeAccumulator::default();
        self.result_panel_side.clear();
        self.result_select_context_detached = false;
        self.clear_field_observation();
    }

    pub(super) fn publish_resolver_transition(
        &mut self,
        session_id: Option<&String>,
        source_sequence: u64,
        scope: ResolverScope,
        summary: &HypothesisSummary,
        observation_count: u32,
    ) -> Result<(), RunEventReductionError> {
        let identity = ResolverTransitionIdentity {
            state: summary.state,
            select_play_type: summary.select_play_type,
            result_play_type: summary.result_play_type,
            top: summary.selected.as_ref().map(resolver_hypothesis_key),
            runner_up: summary.runner_up.as_ref().map(resolver_hypothesis_key),
            runner_song: summary.runner_song.as_ref().map(resolver_hypothesis_key),
            runner_chart: summary.runner_chart.as_ref().map(resolver_hypothesis_key),
        };
        if self.resolver_transitions.get(&scope) == Some(&identity) {
            return Ok(());
        }
        self.resolver_transitions.insert(scope, identity.clone());
        self.emit(RunEvent {
            schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::ResolverStateChanged {
                session_id: session_id.cloned(),
                screen_episode_id: self.screen_episode_id,
                source_sequence,
                scope,
                state: identity.state,
                select_play_type: identity.select_play_type,
                result_play_type: identity.result_play_type,
                play_type_mismatch: identity.select_play_type.is_some()
                    && identity.result_play_type.is_some()
                    && identity.select_play_type != identity.result_play_type,
                top: identity.top,
                runner_up: identity.runner_up,
                runner_song: identity.runner_song,
                runner_chart: identity.runner_chart,
                top_candidates: summary
                    .top_candidates
                    .iter()
                    .map(resolver_hypothesis_key)
                    .collect(),
                support: summary.support,
                margin: summary.margin,
                song_margin: summary.song_margin,
                chart_margin: summary.chart_margin,
                selected_family_support: summary.selected_family_support.clone(),
                runner_up_family_support: summary.runner_up_family_support.clone(),
                observation_count,
            },
        })
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one owner preserves ordered screen, attempt, reset, and diagnostic transitions"
    )]
    pub(super) fn publish_screen_change(
        &mut self,
        event: &RunEvent,
        publish_event: bool,
    ) -> Result<(), RunEventReductionError> {
        let RunEventKind::ScreenChanged {
            session_id,
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            ..
        } = &event.kind
        else {
            unreachable!("screen change dispatcher preserves event kind");
        };
        self.latest_screen_boundary_sequence = Some(*sequence);
        if screen != "music_select" && self.music_selection_episode_active {
            self.publish_music_selection(
                session_id.as_ref(),
                *sequence,
                MusicSelectionState::Unresolved {
                    reason: MusicSelectionUnresolvedReason::EpisodeEnded,
                },
            )?;
            self.music_selection_episode_active = false;
        }
        self.screen_episode_id = *screen_episode_id;
        self.screen_episode_started_ms = Some(*monotonic_end_ms);
        self.screen_episode_last_ms = Some(*monotonic_end_ms);
        let close_result_resolver = screen != "result" && self.result_resolver_active;
        let selection_difficulty_reset = (screen == "music_select")
            .then(|| self.engine.selection_epochs.active_difficulty_state())
            .flatten();
        if close_result_resolver {
            self.result_resolver_active = false;
            self.engine.result_hypotheses = HypothesisAccumulator::default();
            self.engine.provisional_joint = None;
            if screen != "play" {
                self.engine.retained_select = HypothesisAccumulator::default();
            }
            self.resolver_transitions.remove(&ResolverScope::Result);
            self.resolver_transitions
                .remove(&ResolverScope::AttemptJoint);
        }
        let mut selection_screen_attempt_update = None;
        if screen == "music_select" {
            self.music_selection_revision = 0;
            self.music_select_resolver = MusicSelectResolver::default();
            self.active_music_selection = None;
            self.music_selection_episode_active = true;
            self.engine.selection_epochs = SelectionEpochTracker::default();
            self.engine.retained_select = HypothesisAccumulator::default();
            self.resolver_transitions
                .remove(&ResolverScope::SelectionIncumbent);
            self.resolver_transitions
                .remove(&ResolverScope::SelectionSuccessor);
            selection_screen_attempt_update = self.engine.play_attempt.observe_selection_screen();
        }
        if screen == "result" {
            self.result_panel_side.start_episode(*screen_episode_id);
            self.result_select_context_detached = false;
            self.result_resolver_active = true;
            self.active_provisional_result = None;
            self.engine.result_hypotheses = HypothesisAccumulator::default();
            self.engine.provisional_joint = None;
            self.resolver_transitions.remove(&ResolverScope::Result);
            self.resolver_transitions
                .remove(&ResolverScope::AttemptJoint);
            self.numeric_evidence.clear();
            self.play_options = PlayOptionsEpisodeAccumulator::default();
        }
        if matches!(screen.as_str(), "decide_transition" | "play")
            && self.attempt_started_ms.is_none()
        {
            self.attempt_started_ms = Some(*monotonic_end_ms);
        }
        if matches!(screen.as_str(), "decide_transition" | "play" | "result") {
            self.attempt_phase_started_ms = Some(*monotonic_end_ms);
        }
        if publish_event {
            self.publish_one(event)?;
        }
        if let Some((target, _)) = selection_difficulty_reset {
            self.publish_one(&RunEvent {
                schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::SelectionDifficultyChanged {
                    session_id: session_id.clone(),
                    screen_episode_id: *screen_episode_id,
                    source_sequence: *sequence,
                    target,
                    reason: SelectionDifficultyTransitionReason::Reset,
                    current: None,
                },
            })?;
        }
        let unresolved = HypothesisAccumulator::default().summary();
        for scope in if close_result_resolver || screen == "result" {
            [
                Some(ResolverScope::Result),
                Some(ResolverScope::AttemptJoint),
            ]
        } else {
            [None, None]
        }
        .into_iter()
        .flatten()
        {
            self.publish_resolver_transition(
                session_id.as_ref(),
                *sequence,
                scope,
                &unresolved,
                0,
            )?;
        }
        if screen == "music_select" {
            for scope in [
                ResolverScope::SelectionIncumbent,
                ResolverScope::SelectionSuccessor,
            ] {
                self.publish_resolver_transition(
                    session_id.as_ref(),
                    *sequence,
                    scope,
                    &unresolved,
                    0,
                )?;
            }
        }
        if let Some(attempt_screen) = play_attempt_screen(screen)
            && let Some(state) = self
                .engine
                .play_attempt
                .observe_screen(attempt_screen, *sequence)
        {
            self.publish_play_attempt_update(session_id.clone(), Some(*sequence), state)?;
        }
        if screen == "play"
            && let Some(session_id) = session_id.clone()
        {
            self.active_provisional_result = None;
            self.publish_result_state(session_id, *sequence, ResultState::Inactive)?;
        }
        if let Some(state) = selection_screen_attempt_update {
            self.attempt_started_ms = None;
            self.attempt_phase_started_ms = None;
            self.publish_play_attempt_update(session_id.clone(), Some(*sequence), state)?;
        }
        if screen != "result" {
            self.result_panel_side.clear();
            self.result_select_context_detached = false;
            self.reset_numeric_result();
            self.numeric_evidence.clear();
            self.play_options = PlayOptionsEpisodeAccumulator::default();
        }
        self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), false)?;
        Ok(())
    }

    pub(super) fn publish_session_finished(
        &mut self,
        event: &RunEvent,
    ) -> Result<(), RunEventReductionError> {
        let (RunEventKind::SessionFinished { session_id, .. }
        | RunEventKind::CanonicalSessionFinished { session_id }) = &event.kind
        else {
            unreachable!("session-finished dispatcher preserves event kind")
        };
        if self.music_selection_episode_active {
            let source_sequence = self
                .resolver_source_sequence
                .or(self.latest_screen_boundary_sequence)
                .unwrap_or_default();
            self.publish_music_selection(
                Some(session_id),
                source_sequence,
                MusicSelectionState::Unresolved {
                    reason: MusicSelectionUnresolvedReason::EpisodeEnded,
                },
            )?;
            self.music_selection_episode_active = false;
        }
        self.result_panel_side.clear();
        self.result_select_context_detached = false;
        self.effects.push(RunReducerEffect::SessionEnded);
        self.active_session_id = None;
        self.current_screen = None;
        if let Some(state) = self.engine.play_attempt.finish_session() {
            self.publish_play_attempt_update(Some(session_id.clone()), None, state)?;
        }
        Ok(())
    }

    pub(super) fn publish_play_attempt_update(
        &mut self,
        session_id: Option<String>,
        source_sequence: Option<u64>,
        state: PlayAttemptState,
    ) -> Result<(), RunEventReductionError> {
        self.publish_one(&RunEvent {
            schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::PlayAttemptChanged {
                session_id: session_id.clone(),
                source_sequence,
                state,
            },
        })?;
        if let Some(sequence) = source_sequence {
            self.try_emit_result(session_id, sequence)?;
        }
        Ok(())
    }

    pub(super) fn finalize_result_attempt(
        &mut self,
        session_id: Option<String>,
        sequence: u64,
    ) -> Result<(), RunEventReductionError> {
        self.result_episode_finalizing = true;
        let rejection = if self.holds_stable_numeric_result() {
            None
        } else {
            match (
                self.engine.provisional_joint.as_ref(),
                self.accepted_numeric_result.as_ref(),
            ) {
                (None, _) => Some(PlayAttemptReason::JointIdentityUnresolved),
                (Some(_), None) => Some(PlayAttemptReason::ResultEvidenceUnresolved),
                (Some(joint), Some(numeric)) if !joint_matches_numeric(joint, numeric) => {
                    Some(PlayAttemptReason::LinkageConflict)
                }
                (Some(_), Some(_)) => None,
            }
        };
        if let Some(state) = self
            .engine
            .play_attempt
            .resolve_result_with_reason(rejection)
        {
            self.publish_play_attempt_update(session_id.clone(), Some(sequence), state)?;
        }
        self.try_emit_result(session_id.clone(), sequence)?;
        if self.engine.play_attempt.accepted_result().is_none()
            && let Some(session_id) = session_id
        {
            self.withdraw_result_provisional(
                session_id,
                sequence,
                ResultRetractionReason::AttemptRejected,
            )?;
        }
        Ok(())
    }
}
