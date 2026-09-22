#![allow(
    clippy::wildcard_imports,
    reason = "this file is an implementation partition of its parent reducer authority"
)]

use super::*;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveProvisionalResult {
    pub song: Option<SongPresentation>,
    pub result: ResultDomainEvent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NumericResultView {
    pub song_id: ScorepeekSongId,
    pub clear_type: String,
    pub chart: Chart,
    pub current_score: u32,
    pub performance: ResultPerformanceResolution,
    pub source_sequence: u64,
}

#[derive(Clone, Debug)]
pub struct PendingNumericResult {
    pub view: NumericResultView,
    pub observations: u8,
}

#[derive(Clone, Debug)]
pub struct PendingSupplementalResult {
    pub performance: ResultPerformanceResolution,
    pub source_sequence: u64,
    pub observations: u8,
}

#[derive(Clone, Debug)]
pub struct NumericResultTransition {
    pub state: NumericResultTemporalState,
    pub reason: NumericResultTransitionReason,
    pub replaced_accepted: bool,
}

#[derive(Clone, Debug)]
pub struct RawNumericEvidence {
    pub sequence: u64,
    pub monotonic_end_ms: u64,
    pub clear_type: String,
    pub parsed: ParsedResultFields,
}

#[must_use]
pub fn joint_matches_numeric(
    candidate: &JointEvidenceCandidate,
    numeric: &NumericResultView,
) -> bool {
    candidate.song_id == numeric.song_id && candidate.chart == numeric.chart
}

#[must_use]
pub fn same_numeric_tuple(left: &NumericResultView, right: &NumericResultView) -> bool {
    left.song_id == right.song_id
        && left.clear_type == right.clear_type
        && left.chart == right.chart
        && left.current_score == right.current_score
        && matches!(
            (&left.performance, &right.performance),
            (
                ResultPerformanceResolution::Accepted { judgments: left, .. },
                ResultPerformanceResolution::Accepted { judgments: right, .. }
            ) if left == right
        )
}

pub fn retain_supplemental_result(
    target: &mut ResultPerformanceResolution,
    retained: &ResultPerformanceResolution,
) {
    let (
        ResultPerformanceResolution::Accepted {
            miss_count,
            timing,
            combo_break,
            previous_best,
            ..
        },
        ResultPerformanceResolution::Accepted {
            miss_count: retained_miss_count,
            timing: retained_timing,
            combo_break: retained_combo_break,
            previous_best: retained_previous_best,
            ..
        },
    ) = (target, retained)
    else {
        return;
    };
    miss_count.clone_from(retained_miss_count);
    timing.clone_from(retained_timing);
    combo_break.clone_from(retained_combo_break);
    previous_best.clone_from(retained_previous_best);
}

#[derive(Clone, Debug, Default)]
pub struct PlayOptionsEpisodeAccumulator {
    candidate: Option<Vec<PlayOption>>,
    observations: u8,
    conflicting: bool,
    fallback_reason: Option<PlayOptionsUnknownReason>,
    latest: Option<PlayOptionsObservation>,
    last_sequence: Option<u64>,
}

impl PlayOptionsEpisodeAccumulator {
    pub fn observe(&mut self, sequence: u64, observation: PlayOptionsObservation) {
        if self
            .last_sequence
            .is_some_and(|previous| sequence <= previous)
        {
            return;
        }
        match &observation.parsed {
            PlayOptions::Known { values } => match &self.candidate {
                Some(candidate) if candidate == values => {
                    self.observations = self.observations.saturating_add(1);
                }
                Some(_) => self.conflicting = true,
                None => {
                    self.candidate = Some(values.clone());
                    self.observations = 1;
                }
            },
            PlayOptions::Unknown { reason } => self.fallback_reason = Some(*reason),
        }
        self.latest = Some(observation);
        self.last_sequence = Some(sequence);
    }

    #[must_use]
    pub fn resolved(&self) -> PlayOptions {
        if self.conflicting {
            return PlayOptions::Unknown {
                reason: PlayOptionsUnknownReason::ConflictingObservations,
            };
        }
        if let Some(values) = &self.candidate {
            return if self.observations >= PLAY_OPTIONS_REQUIRED_OBSERVATIONS {
                PlayOptions::Known {
                    values: values.clone(),
                }
            } else {
                PlayOptions::Unknown {
                    reason: PlayOptionsUnknownReason::InsufficientObservations,
                }
            };
        }
        PlayOptions::Unknown {
            reason: self
                .fallback_reason
                .unwrap_or(PlayOptionsUnknownReason::NotObserved),
        }
    }

    #[must_use]
    pub const fn latest(&self) -> Option<&PlayOptionsObservation> {
        self.latest.as_ref()
    }

    #[must_use]
    pub const fn observations(&self) -> u8 {
        self.observations
    }

    #[must_use]
    pub const fn conflicting(&self) -> bool {
        self.conflicting
    }
}

#[derive(Clone, Debug, Default)]
pub struct ResultPanelSideAccumulator {
    episode_id: Option<u64>,
    observations: BTreeMap<ResultPanelSide, u8>,
    stable: Option<ResultPanelSide>,
    opposite_observations: u8,
    last_sequence: Option<u64>,
    conflicted: bool,
}

impl ResultPanelSideAccumulator {
    pub fn start_episode(&mut self, episode_id: u64) {
        if self.episode_id != Some(episode_id) {
            *self = Self {
                episode_id: Some(episode_id),
                ..Self::default()
            };
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[must_use]
    pub const fn stable(&self) -> Option<ResultPanelSide> {
        if self.conflicted { None } else { self.stable }
    }

    pub fn observe(
        &mut self,
        episode_id: u64,
        sequence: u64,
        side: ResultPanelSide,
    ) -> Option<(ResultPanelSideEpisodeState, ResultPanelSideTransitionReason)> {
        self.start_episode(episode_id);
        if self.conflicted || self.last_sequence.is_some_and(|last| sequence <= last) {
            return None;
        }
        self.last_sequence = Some(sequence);
        if let Some(stable) = self.stable {
            if side == stable {
                return None;
            }
            self.opposite_observations = self.opposite_observations.saturating_add(1);
            if self.opposite_observations >= 2 {
                self.conflicted = true;
                return Some((
                    ResultPanelSideEpisodeState::Conflicted {
                        stable_side: stable,
                    },
                    ResultPanelSideTransitionReason::Conflict,
                ));
            }
            return Some((
                ResultPanelSideEpisodeState::Stable { side: stable },
                ResultPanelSideTransitionReason::OppositeObserved,
            ));
        }
        let count = self.observations.entry(side).or_default();
        *count = count.saturating_add(1);
        if *count >= 2 {
            self.stable = Some(side);
            self.observations.clear();
            return Some((
                ResultPanelSideEpisodeState::Stable { side },
                ResultPanelSideTransitionReason::Accepted,
            ));
        }
        Some((
            ResultPanelSideEpisodeState::Pending,
            if self.observations.len() == 1 {
                ResultPanelSideTransitionReason::CandidateStarted
            } else {
                ResultPanelSideTransitionReason::CandidateRepeated
            },
        ))
    }
}
