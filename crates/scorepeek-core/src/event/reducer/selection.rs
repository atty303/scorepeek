use super::*;

#[derive(Clone, Debug, Default)]
pub struct SelectionEpochTracker {
    pub incumbent: HypothesisAccumulator,
    pub successor: HypothesisAccumulator,
    pub incumbent_songs: BTreeSet<ScorepeekSongId>,
    pub successor_songs: BTreeSet<ScorepeekSongId>,
    pub pending_difficulty: Option<CurrentSelectionDifficulty>,
    pub play_types: BTreeMap<PlayType, u64>,
    pub play_sides: BTreeMap<PlaySide, u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectionDifficultyTransition {
    pub target: SelectionDifficultyTarget,
    pub reason: SelectionDifficultyTransitionReason,
    pub current: Option<CurrentSelectionDifficulty>,
}

#[derive(Default)]
pub struct MusicSelectResolver {
    pub best: MusicSelectResolverState,
    pub best_last_sequence: Option<u64>,
    pub best_minimum_sequence: u64,
    pub best_closed: bool,
    pub selection_epochs: SelectionEpochTracker,
}

#[derive(Clone, Debug, Default)]
pub struct ResolverEngine {
    pub play_attempt: PlayAttemptReducer,
    pub selection_epochs: SelectionEpochTracker,
    pub retained_select: HypothesisAccumulator,
    pub result_hypotheses: HypothesisAccumulator,
    pub provisional_joint: Option<JointEvidenceCandidate>,
}

impl MusicSelectResolver {
    pub fn observe(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        evidence: &JointEvidenceObservation,
        difficulty: Option<Difficulty>,
        play_type: Option<PlayType>,
        play_side: Option<PlaySide>,
    ) {
        self.selection_epochs.observe_at_with_play_type(
            sequence,
            monotonic_ms,
            evidence,
            difficulty,
            play_type,
            play_side,
        );
    }

    #[must_use]
    pub fn best_frame_identity(
        &self,
        fields: &Value,
        evidence: &JointEvidenceObservation,
    ) -> SelectFrameIdentity {
        let credible = credible_song_set(evidence);
        let difficulty = selected_difficulty(fields);
        let play_type = selected_play_type(fields);
        let selected = self.selected().and_then(BestChart::from_selection);
        if let Some(chart) = selected.as_ref()
            && credible == BTreeSet::from([chart.scorepeek_song_id])
            && difficulty == Some(chart.difficulty)
            && play_type == Some(chart.play_type)
        {
            return SelectFrameIdentity::Confirmed(chart.clone());
        }
        let conflicting = self.best.chart.as_ref().is_some_and(|chart| {
            credible.iter().any(|song| *song != chart.scorepeek_song_id)
                || difficulty.is_some_and(|value| value != chart.difficulty)
                || play_type.is_some_and(|value| value != chart.play_type)
        });
        if conflicting {
            return SelectFrameIdentity::Conflicting(SelectIdentityStatus::CurrentFrameConflict);
        }
        let reason = if difficulty.is_none() {
            SelectIdentityStatus::AwaitingDifficulty
        } else if play_type.is_none() {
            SelectIdentityStatus::AwaitingPlayType
        } else if credible.is_empty() {
            SelectIdentityStatus::AwaitingEvidence
        } else if selected.is_some() {
            SelectIdentityStatus::CurrentFrameConflict
        } else {
            SelectIdentityStatus::Stabilizing
        };
        SelectFrameIdentity::Missing(reason)
    }

    pub fn accepts_best_observation(&mut self, sequence: u64) -> bool {
        if self.best_closed
            || sequence < self.best_minimum_sequence
            || self.best_last_sequence.is_some_and(|last| sequence <= last)
        {
            return false;
        }
        self.best_last_sequence = Some(sequence);
        true
    }

    #[must_use]
    pub fn selected(&self) -> Option<MusicSelectionState> {
        let accumulator = if self.selection_epochs.successor.observation_count > 0 {
            &self.selection_epochs.successor
        } else {
            &self.selection_epochs.incumbent
        };
        let summary = accumulator.summary();
        let selected = summary.selected.as_ref()?;
        let play_type = summary.select_play_type?;
        let play_side = accumulator.resolved_select_play_side()?;
        let difficulty = accumulator.select_difficulty?.difficulty;
        if summary.support < JOINT_ACCEPT_SUPPORT
            || summary.song_margin < JOINT_ACCEPT_MARGIN
            || summary.chart_margin < JOINT_ACCEPT_MARGIN
            || selected.chart.key.play_type != play_type
            || selected.chart.key.difficulty != difficulty
        {
            return None;
        }
        Some(MusicSelectionState::Selected {
            scorepeek_song_id: selected.song_id,
            play_side,
            play_type,
            difficulty,
            level: selected.chart.level,
            notes: selected.chart.notes,
            presentation: candidate_song_presentation(selected),
        })
    }
}

#[must_use]
pub fn selected_difficulty(fields: &Value) -> Option<Difficulty> {
    let value = fields
        .pointer("/selected_difficulty/state")?
        .get("value")?
        .as_str()?;
    match value {
        "beginner" => Some(Difficulty::Beginner),
        "normal" => Some(Difficulty::Normal),
        "hyper" => Some(Difficulty::Hyper),
        "another" => Some(Difficulty::Another),
        "leggendaria" => Some(Difficulty::Leggendaria),
        _ => None,
    }
}

#[must_use]
pub fn selected_play_type(fields: &Value) -> Option<PlayType> {
    let value = fields.pointer("/play_type/state")?.get("value")?.as_str()?;
    match value {
        "single" => Some(PlayType::Single),
        "double" => Some(PlayType::Double),
        _ => None,
    }
}

fn observe_current_difficulty(
    current: &mut Option<CurrentSelectionDifficulty>,
    difficulty: Difficulty,
    sequence: u64,
    monotonic_ms: u64,
) -> bool {
    if let Some(current) = current {
        current.observe(difficulty, sequence, monotonic_ms)
    } else {
        *current = Some(CurrentSelectionDifficulty::observed(
            difficulty,
            sequence,
            monotonic_ms,
        ));
        true
    }
}

fn push_difficulty_change(
    transitions: &mut Vec<SelectionDifficultyTransition>,
    target: SelectionDifficultyTarget,
    previous: Option<CurrentSelectionDifficulty>,
    current: Option<CurrentSelectionDifficulty>,
) {
    if previous.map(|value| value.difficulty) != current.map(|value| value.difficulty) {
        transitions.push(SelectionDifficultyTransition {
            target,
            reason: SelectionDifficultyTransitionReason::Changed,
            current,
        });
    }
}

fn push_target_switch(
    transitions: &mut Vec<SelectionDifficultyTransition>,
    previous: Option<(
        SelectionDifficultyTarget,
        Option<CurrentSelectionDifficulty>,
    )>,
    current: Option<(
        SelectionDifficultyTarget,
        Option<CurrentSelectionDifficulty>,
    )>,
) {
    let Some((target, current)) = current else {
        return;
    };
    if previous.map(|(target, _)| target) != Some(target)
        && !transitions.iter().any(|transition| {
            transition.target == target
                && matches!(
                    transition.reason,
                    SelectionDifficultyTransitionReason::Changed
                        | SelectionDifficultyTransitionReason::PendingApplied
                )
        })
    {
        transitions.push(SelectionDifficultyTransition {
            target,
            reason: SelectionDifficultyTransitionReason::TargetSwitch,
            current,
        });
    }
}
impl SelectionEpochTracker {
    pub fn active_difficulty_state(
        &self,
    ) -> Option<(
        SelectionDifficultyTarget,
        Option<CurrentSelectionDifficulty>,
    )> {
        if self.successor.observation_count > 0 {
            Some((
                SelectionDifficultyTarget::Successor,
                self.successor.select_difficulty,
            ))
        } else if self.incumbent.observation_count > 0 {
            Some((
                SelectionDifficultyTarget::Incumbent,
                self.incumbent.select_difficulty,
            ))
        } else {
            self.pending_difficulty
                .map(|current| (SelectionDifficultyTarget::Pending, Some(current)))
        }
    }

    pub fn observe_at_with_play_type(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        evidence: &JointEvidenceObservation,
        difficulty: Option<Difficulty>,
        play_type: Option<PlayType>,
        play_side: Option<PlaySide>,
    ) -> Vec<SelectionDifficultyTransition> {
        if let Some(play_type) = play_type {
            let count = self.play_types.entry(play_type).or_default();
            *count = count.saturating_add(1);
        }
        if let Some(play_side) = play_side {
            let count = self.play_sides.entry(play_side).or_default();
            *count = count.saturating_add(1);
        }
        let transitions = self.observe_selection_at(sequence, monotonic_ms, evidence, difficulty);
        self.incumbent
            .select_play_types
            .clone_from(&self.play_types);
        self.successor
            .select_play_types
            .clone_from(&self.play_types);
        self.incumbent
            .select_play_sides
            .clone_from(&self.play_sides);
        self.successor
            .select_play_sides
            .clone_from(&self.play_sides);
        transitions
    }
}
impl SelectionEpochTracker {
    #[cfg(feature = "reducer-test-support")]
    pub fn observe_at(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        evidence: &JointEvidenceObservation,
        difficulty: Option<Difficulty>,
    ) -> Vec<SelectionDifficultyTransition> {
        self.observe_at_with_play_type(sequence, monotonic_ms, evidence, difficulty, None, None)
    }

    pub fn observe_difficulty_only(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        difficulty: Difficulty,
    ) -> Vec<SelectionDifficultyTransition> {
        let (target, changed, current) = if self.successor.observation_count > 0 {
            let changed =
                self.successor
                    .observe_select_difficulty(difficulty, sequence, monotonic_ms);
            (
                SelectionDifficultyTarget::Successor,
                changed,
                self.successor.select_difficulty,
            )
        } else if self.incumbent.observation_count > 0 {
            let changed =
                self.incumbent
                    .observe_select_difficulty(difficulty, sequence, monotonic_ms);
            (
                SelectionDifficultyTarget::Incumbent,
                changed,
                self.incumbent.select_difficulty,
            )
        } else {
            let changed = observe_current_difficulty(
                &mut self.pending_difficulty,
                difficulty,
                sequence,
                monotonic_ms,
            );
            (
                SelectionDifficultyTarget::Pending,
                changed,
                self.pending_difficulty,
            )
        };
        if changed {
            vec![SelectionDifficultyTransition {
                target,
                reason: SelectionDifficultyTransitionReason::Changed,
                current,
            }]
        } else {
            Vec::new()
        }
    }

    #[cfg(feature = "reducer-test-support")]
    pub fn observe(
        &mut self,
        monotonic_ms: u64,
        evidence: &JointEvidenceObservation,
        difficulty: Option<Difficulty>,
    ) -> Vec<SelectionDifficultyTransition> {
        self.observe_at(monotonic_ms, monotonic_ms, evidence, difficulty)
    }

    pub fn handoff(&self) -> HypothesisAccumulator {
        if self.successor.observation_count > 0 {
            self.successor.clone()
        } else {
            self.incumbent.clone()
        }
    }
}

impl SelectionEpochTracker {
    pub fn observe_selection_at(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        evidence: &JointEvidenceObservation,
        difficulty: Option<Difficulty>,
    ) -> Vec<SelectionDifficultyTransition> {
        let previous_target = self.active_difficulty_state();
        let mut transitions = Vec::new();
        let credible = credible_song_set(evidence);
        if credible.is_empty() {
            let mut transitions = difficulty.map_or_else(Vec::new, |difficulty| {
                self.observe_difficulty_only(sequence, monotonic_ms, difficulty)
            });
            push_target_switch(
                &mut transitions,
                previous_target,
                self.active_difficulty_state(),
            );
            return transitions;
        }
        if self.incumbent.observation_count == 0 {
            if let Some(pending) = self.pending_difficulty.take() {
                self.incumbent.select_difficulty = Some(pending);
                transitions.push(SelectionDifficultyTransition {
                    target: SelectionDifficultyTarget::Incumbent,
                    reason: SelectionDifficultyTransitionReason::PendingApplied,
                    current: Some(pending),
                });
            }
            let previous = self.incumbent.select_difficulty;
            self.incumbent
                .observe_at(sequence, monotonic_ms, evidence, difficulty, None, None);
            push_difficulty_change(
                &mut transitions,
                SelectionDifficultyTarget::Incumbent,
                previous,
                self.incumbent.select_difficulty,
            );
            self.incumbent_songs.extend(credible);
            push_target_switch(
                &mut transitions,
                previous_target,
                self.active_difficulty_state(),
            );
            return transitions;
        }
        if !self.incumbent_songs.is_disjoint(&credible) {
            let previous = self.incumbent.select_difficulty;
            self.incumbent
                .observe_at(sequence, monotonic_ms, evidence, difficulty, None, None);
            push_difficulty_change(
                &mut transitions,
                SelectionDifficultyTarget::Incumbent,
                previous,
                self.incumbent.select_difficulty,
            );
            self.incumbent_songs.extend(credible);
            if self.successor.select_difficulty.is_some() {
                transitions.push(SelectionDifficultyTransition {
                    target: SelectionDifficultyTarget::Successor,
                    reason: SelectionDifficultyTransitionReason::Reset,
                    current: None,
                });
            }
            self.successor = HypothesisAccumulator::default();
            self.successor_songs.clear();
            push_target_switch(
                &mut transitions,
                previous_target,
                self.active_difficulty_state(),
            );
            return transitions;
        }
        if !self.successor_songs.is_empty() && self.successor_songs.is_disjoint(&credible) {
            if self.successor.select_difficulty.is_some() {
                transitions.push(SelectionDifficultyTransition {
                    target: SelectionDifficultyTarget::Successor,
                    reason: SelectionDifficultyTransitionReason::Reset,
                    current: None,
                });
            }
            self.successor = HypothesisAccumulator::default();
            self.successor_songs.clear();
        }
        let previous = self.successor.select_difficulty;
        self.successor
            .observe_at(sequence, monotonic_ms, evidence, difficulty, None, None);
        push_difficulty_change(
            &mut transitions,
            SelectionDifficultyTarget::Successor,
            previous,
            self.successor.select_difficulty,
        );
        self.successor_songs.extend(credible);
        if self.successor.summary().support >= SELECTION_CHANGE_MARGIN {
            self.incumbent = std::mem::take(&mut self.successor);
            self.incumbent_songs = std::mem::take(&mut self.successor_songs);
        }
        push_target_switch(
            &mut transitions,
            previous_target,
            self.active_difficulty_state(),
        );
        transitions
    }
}
