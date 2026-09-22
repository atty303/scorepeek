use super::*;

impl RunEventReducer {
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the reducer keeps ordered temporal, attempt, and domain emission in one path"
    )]
    pub(super) fn reduce_result_observation(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_end_ms: u64,
        fields: &Value,
        parsed_result_fields: Option<&ParsedResultFields>,
        joint_evidence: &JointEvidenceObservation,
        _song_resolution_presentation: &SongResolutionPresentation,
    ) -> Result<(), RunEventReductionError> {
        self.engine.result_hypotheses.observe_at(
            sequence,
            monotonic_end_ms,
            joint_evidence,
            None,
            None,
            parsed_result_fields.map(result_chart_factor),
        );
        if let Some(observation) = fields
            .get("play_options")
            .cloned()
            .and_then(|value| serde_json::from_value::<PlayOptionsObservation>(value).ok())
        {
            self.play_options.observe(sequence, observation);
        }
        let result_summary = self.engine.result_hypotheses.summary();
        if !self.result_select_context_detached
            && let Some(result_side) = result_play_side(self.result_panel_side.stable())
            && let Some(select_side) = self.engine.retained_select.resolved_select_play_side()
            && select_side != result_side
        {
            self.result_select_context_detached = true;
            self.engine.retained_select = HypothesisAccumulator::default();
            self.engine.provisional_joint = None;
            self.publish_one(&RunEvent {
                schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::ResultSelectContextMismatch {
                    session_id: session_id.cloned(),
                    capture_generation,
                    screen_episode_id: self.screen_episode_id,
                    source_sequence: sequence,
                    select_play_side: select_side,
                    result_play_side: result_side,
                },
            })?;
            if let Some(state) = self.engine.play_attempt.detach_selection_linkage() {
                self.publish_play_attempt_update(
                    session_id.cloned(),
                    capture_generation,
                    Some(sequence),
                    state,
                )?;
            }
        }
        self.publish_resolver_transition(
            session_id,
            capture_generation,
            sequence,
            ResolverScope::Result,
            &result_summary,
            self.engine.result_hypotheses.observation_count,
        )?;
        let mut joint = self.engine.retained_select.clone();
        joint.add_from(&self.engine.result_hypotheses);
        let joint_summary = joint.summary();
        self.publish_resolver_transition(
            session_id,
            capture_generation,
            sequence,
            ResolverScope::AttemptJoint,
            &joint_summary,
            self.engine
                .retained_select
                .observation_count
                .saturating_add(self.engine.result_hypotheses.observation_count),
        )?;
        let accepted_joint = joint_summary.accepted();
        self.engine.provisional_joint.clone_from(&accepted_joint);
        let observed_clear_type = fields
            .get("clear_type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if let (Some(clear_type), Some(parsed)) =
            (observed_clear_type.clone(), parsed_result_fields.cloned())
        {
            if self.numeric_evidence.len() == 8 {
                self.numeric_evidence.pop_front();
            }
            self.numeric_evidence.push_back(RawNumericEvidence {
                sequence,
                monotonic_end_ms,
                clear_type,
                parsed,
            });
        }
        if let Some(candidate) = accepted_joint.as_ref() {
            let pending: Vec<_> = self
                .numeric_evidence
                .iter()
                .filter(|evidence| {
                    self.last_numeric_sequence
                        .is_none_or(|last| evidence.sequence > last)
                })
                .cloned()
                .collect();
            for evidence in pending {
                if let Some(transition) = self.observe_numeric_result(
                    evidence.sequence,
                    evidence.monotonic_end_ms,
                    Some(candidate),
                    Some(evidence.clear_type),
                    Some(&evidence.parsed),
                ) {
                    self.publish_one(&RunEvent {
                        schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
                        kind: RunEventKind::NumericResultChanged {
                            session_id: session_id.cloned(),
                            capture_generation,
                            source_sequence: evidence.sequence,
                            state: transition.state,
                            reason: transition.reason,
                            event_suppression_reason: self
                                .numeric_event_suppression_reason(session_id, capture_generation),
                        },
                    })?;
                    if transition.replaced_accepted
                        && let (Some(session_id), Some(capture_generation)) =
                            (session_id.cloned(), capture_generation)
                    {
                        self.withdraw_result_provisional(
                            session_id,
                            capture_generation,
                            evidence.sequence,
                            ResultRetractionReason::EvidenceUnresolved,
                        )?;
                    }
                }
            }
        }
        self.sync_result_provisional(session_id.cloned(), capture_generation, sequence)?;
        self.try_emit_result(session_id.cloned(), capture_generation, sequence)?;
        self.sync_resolver_snapshot(monotonic_end_ms, Some(sequence), Some(fields))?;
        self.refresh()
    }

    pub(super) fn observe_numeric_result(
        &mut self,
        sequence: u64,
        monotonic_end_ms: u64,
        accepted_joint: Option<&JointEvidenceCandidate>,
        observed_clear_type: Option<String>,
        parsed_result_fields: Option<&ParsedResultFields>,
    ) -> Option<NumericResultTransition> {
        let chronology_reset = self
            .last_numeric_sequence
            .is_some_and(|last| sequence <= last)
            || self
                .last_numeric_monotonic_ms
                .is_some_and(|last| monotonic_end_ms < last);
        if chronology_reset {
            self.reset_numeric_result();
        }
        self.last_numeric_sequence = Some(sequence);
        self.last_numeric_monotonic_ms = Some(monotonic_end_ms);
        let (Some(candidate), Some(clear_type), Some(parsed)) =
            (accepted_joint, observed_clear_type, parsed_result_fields)
        else {
            return self
                .pending_numeric_result
                .take()
                .map(|_| NumericResultTransition {
                    state: NumericResultTemporalState::Unknown,
                    reason: NumericResultTransitionReason::Incomplete,
                    replaced_accepted: false,
                });
        };
        let Some(current_score) = parsed.current_score.known().copied() else {
            return self
                .pending_numeric_result
                .take()
                .map(|_| NumericResultTransition {
                    state: NumericResultTemporalState::Unknown,
                    reason: NumericResultTransitionReason::Incomplete,
                    replaced_accepted: false,
                });
        };
        let performance = resolve_result_performance(parsed, candidate.chart.notes, current_score);
        if !matches!(performance, ResultPerformanceResolution::Accepted { .. }) {
            return self
                .pending_numeric_result
                .take()
                .map(|_| NumericResultTransition {
                    state: NumericResultTemporalState::Unknown,
                    reason: NumericResultTransitionReason::Incomplete,
                    replaced_accepted: false,
                });
        }
        self.stabilize_numeric_result(
            NumericResultView {
                song_id: candidate.song_id,
                clear_type,
                chart: candidate.chart.clone(),
                current_score,
                performance,
                source_sequence: sequence,
            },
            chronology_reset,
        )
    }

    pub(super) fn stabilize_numeric_result(
        &mut self,
        view: NumericResultView,
        chronology_reset: bool,
    ) -> Option<NumericResultTransition> {
        let replaces_accepted = self
            .accepted_numeric_result
            .as_ref()
            .is_some_and(|accepted| !same_numeric_tuple(accepted, &view));
        if let Some(accepted) = &self.accepted_numeric_result
            && same_numeric_tuple(accepted, &view)
            && accepted.performance == view.performance
        {
            self.pending_numeric_result = None;
            self.pending_supplemental_result = None;
            return None;
        }
        if self
            .accepted_numeric_result
            .as_ref()
            .is_some_and(|accepted| same_numeric_tuple(accepted, &view))
        {
            self.pending_numeric_result = None;
            return self.stabilize_supplemental_result(view);
        }
        let had_conflict = self
            .pending_numeric_result
            .as_ref()
            .is_some_and(|pending| !same_numeric_tuple(&pending.view, &view));
        let transition = match &mut self.pending_numeric_result {
            Some(pending) if same_numeric_tuple(&pending.view, &view) => {
                pending.observations = pending.observations.saturating_add(1);
                let supplemental_stable = pending.view.performance == view.performance;
                pending.view = view;
                if pending.observations >= NUMERIC_REQUIRED_OBSERVATIONS {
                    let mut accepted = pending.view.clone();
                    if replaces_accepted
                        && !supplemental_stable
                        && let Some(previous) = &self.accepted_numeric_result
                    {
                        retain_supplemental_result(
                            &mut accepted.performance,
                            &previous.performance,
                        );
                        self.pending_supplemental_result = Some(PendingSupplementalResult {
                            performance: pending.view.performance.clone(),
                            source_sequence: pending.view.source_sequence,
                            observations: 1,
                        });
                    } else {
                        self.pending_supplemental_result = None;
                    }
                    self.accepted_numeric_result = Some(accepted);
                    self.pending_numeric_result = None;
                    Some(NumericResultTransition {
                        state: NumericResultTemporalState::Accepted,
                        reason: NumericResultTransitionReason::Accepted,
                        replaced_accepted: replaces_accepted,
                    })
                } else {
                    Some(NumericResultTransition {
                        state: NumericResultTemporalState::Pending {
                            observations: pending.observations,
                        },
                        reason: NumericResultTransitionReason::CandidateRepeated,
                        replaced_accepted: false,
                    })
                }
            }
            _ => {
                self.pending_supplemental_result = None;
                self.pending_numeric_result = Some(PendingNumericResult {
                    view,
                    observations: 1,
                });
                Some(NumericResultTransition {
                    state: NumericResultTemporalState::Pending { observations: 1 },
                    reason: if chronology_reset {
                        NumericResultTransitionReason::ChronologyReset
                    } else if had_conflict {
                        NumericResultTransitionReason::Conflict
                    } else {
                        NumericResultTransitionReason::CandidateStarted
                    },
                    replaced_accepted: false,
                })
            }
        };
        if self.accepted_numeric_result.is_some() && self.pending_numeric_result.is_some() {
            None
        } else {
            transition
        }
    }

    pub(super) fn stabilize_supplemental_result(
        &mut self,
        view: NumericResultView,
    ) -> Option<NumericResultTransition> {
        let repeated = self
            .pending_supplemental_result
            .as_ref()
            .is_some_and(|pending| pending.performance == view.performance);
        if repeated {
            let pending = self
                .pending_supplemental_result
                .as_mut()
                .expect("repeated supplemental candidate exists");
            pending.observations = pending.observations.saturating_add(1);
            pending.source_sequence = view.source_sequence;
            if pending.observations >= NUMERIC_REQUIRED_OBSERVATIONS {
                let pending = self
                    .pending_supplemental_result
                    .take()
                    .expect("accepted supplemental candidate exists");
                let accepted = self
                    .accepted_numeric_result
                    .as_mut()
                    .expect("supplemental result requires accepted numeric result");
                accepted.performance = pending.performance;
                accepted.source_sequence = pending.source_sequence;
                return Some(NumericResultTransition {
                    state: NumericResultTemporalState::Accepted,
                    reason: NumericResultTransitionReason::Accepted,
                    replaced_accepted: false,
                });
            }
        } else {
            self.pending_supplemental_result = Some(PendingSupplementalResult {
                performance: view.performance,
                source_sequence: view.source_sequence,
                observations: 1,
            });
        }
        None
    }

    pub(super) fn numeric_event_suppression_reason(
        &self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
    ) -> Option<NumericResultEventSuppressionReason> {
        if self
            .engine
            .play_attempt
            .accepted_result()
            .is_some_and(|attempt| self.emitted_attempt_ids.contains(&attempt.attempt_id))
        {
            return Some(NumericResultEventSuppressionReason::AlreadyEmitted);
        }
        if session_id.is_none() || capture_generation.is_none() {
            return Some(NumericResultEventSuppressionReason::SessionUnavailable);
        }
        let Some(numeric) = self.accepted_numeric_result.as_ref() else {
            return Some(NumericResultEventSuppressionReason::NumericNotAccepted);
        };
        if self.engine.play_attempt.accepted_result().is_none() {
            return Some(NumericResultEventSuppressionReason::PlayAttemptNotAccepted);
        }
        (!self
            .engine
            .provisional_joint
            .as_ref()
            .is_some_and(|candidate| joint_matches_numeric(candidate, numeric)))
        .then_some(NumericResultEventSuppressionReason::LinkageConflict)
    }

    pub(super) fn try_emit_result(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        fallback_sequence: u64,
    ) -> Result<(), RunEventReductionError> {
        if !self.result_episode_finalizing {
            return Ok(());
        }
        let (Some(session_id), Some(capture_generation)) = (session_id, capture_generation) else {
            return Ok(());
        };
        let Some(numeric) = self.accepted_numeric_result.as_ref() else {
            return Ok(());
        };
        let Some(accepted_attempt) = self.engine.play_attempt.accepted_result() else {
            return Ok(());
        };
        if self
            .emitted_attempt_ids
            .contains(&accepted_attempt.attempt_id)
        {
            return Ok(());
        }
        if self.holds_stable_numeric_result() {
            let Some(candidate) = self
                .active_provisional_result
                .clone()
                .filter(|candidate| candidate.result.attempt_id == accepted_attempt.attempt_id)
            else {
                return Ok(());
            };
            let source_sequence = numeric.source_sequence.max(fallback_sequence);
            let attempt_id = accepted_attempt.attempt_id;
            self.publish_result_state(
                session_id,
                capture_generation,
                source_sequence,
                ResultState::Confirmed {
                    song: candidate.song,
                    result: Box::new(candidate.result),
                },
            )?;
            self.active_provisional_result = None;
            self.emitted_attempt_ids.insert(attempt_id);
            return Ok(());
        }
        if !self
            .engine
            .provisional_joint
            .as_ref()
            .is_some_and(|candidate| joint_matches_numeric(candidate, numeric))
        {
            return Ok(());
        }
        let Some(play_side) = result_play_side(self.result_panel_side.stable()) else {
            return Ok(());
        };
        let result = build_result_domain_event(
            accepted_attempt,
            numeric,
            play_side,
            self.play_options.resolved(),
        );
        let source_sequence = numeric.source_sequence.max(fallback_sequence);
        let emitted_attempt_id = accepted_attempt.attempt_id;
        let song = self
            .engine
            .provisional_joint
            .as_ref()
            .map(candidate_song_presentation);
        let candidate = ActiveProvisionalResult { song, result };
        if self.active_provisional_result.as_ref() != Some(&candidate) {
            self.publish_result_state(
                session_id.clone(),
                capture_generation,
                source_sequence,
                ResultState::Provisional {
                    song: candidate.song.clone(),
                    result: Box::new(candidate.result.clone()),
                },
            )?;
        }
        self.publish_result_state(
            session_id,
            capture_generation,
            source_sequence,
            ResultState::Confirmed {
                song: candidate.song.clone(),
                result: Box::new(candidate.result.clone()),
            },
        )?;
        self.active_provisional_result = None;
        self.emitted_attempt_ids.insert(emitted_attempt_id);
        Ok(())
    }

    pub(super) fn sync_result_provisional(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        fallback_sequence: u64,
    ) -> Result<(), RunEventReductionError> {
        if self.holds_stable_numeric_result() {
            return Ok(());
        }
        let candidate = self
            .engine
            .provisional_joint
            .as_ref()
            .zip(self.accepted_numeric_result.as_ref())
            .filter(|(joint, numeric)| joint_matches_numeric(joint, numeric))
            .zip(self.engine.play_attempt.active_result())
            .and_then(|((joint, numeric), attempt)| {
                let play_side = result_play_side(self.result_panel_side.stable())?;
                let song = Some(candidate_song_presentation(joint));
                let result = build_result_domain_event(
                    attempt,
                    numeric,
                    play_side,
                    self.play_options.resolved(),
                );
                Some((
                    ActiveProvisionalResult { song, result },
                    numeric.source_sequence.max(fallback_sequence),
                ))
            });
        let (Some(session_id), Some(capture_generation)) = (session_id, capture_generation) else {
            return Ok(());
        };
        match candidate {
            Some((candidate, source_sequence))
                if self.active_provisional_result.as_ref() != Some(&candidate) =>
            {
                self.publish_result_state(
                    session_id,
                    capture_generation,
                    source_sequence,
                    ResultState::Provisional {
                        song: candidate.song.clone(),
                        result: Box::new(candidate.result.clone()),
                    },
                )?;
                self.active_provisional_result = Some(candidate);
            }
            None if self.active_provisional_result.is_some() => {
                self.withdraw_result_provisional(
                    session_id,
                    capture_generation,
                    fallback_sequence,
                    ResultRetractionReason::EvidenceUnresolved,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn holds_stable_numeric_result(&self) -> bool {
        let (Some(active), Some(accepted), Some(joint)) = (
            self.active_provisional_result.as_ref(),
            self.accepted_numeric_result.as_ref(),
            self.engine.provisional_joint.as_ref(),
        ) else {
            return false;
        };
        (self.pending_numeric_result.is_some() || self.pending_supplemental_result.is_some())
            && active.result.scorepeek_song_id == accepted.song_id
            && joint_matches_numeric(joint, accepted)
    }

    pub(super) fn withdraw_result_provisional(
        &mut self,
        session_id: String,
        capture_generation: u64,
        source_sequence: u64,
        reason: ResultRetractionReason,
    ) -> Result<(), RunEventReductionError> {
        let Some(candidate) = self.active_provisional_result.take() else {
            return Ok(());
        };
        self.publish_result_state(
            session_id,
            capture_generation,
            source_sequence,
            ResultState::Retracted {
                song: candidate.song,
                result: Box::new(candidate.result),
                reason,
            },
        )
    }

    pub(super) fn publish_result_state(
        &mut self,
        session_id: String,
        capture_generation: u64,
        source_sequence: u64,
        state: ResultState,
    ) -> Result<(), RunEventReductionError> {
        self.publish_one(&RunEvent {
            schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::ResultChanged {
                session_id,
                capture_generation,
                source_sequence,
                state,
            },
        })
    }
}
