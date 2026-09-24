//! Portable state used while reducing recognition observations into domain events.

mod hypothesis;
mod lifecycle;
mod music_select_flow;
mod result;
mod result_flow;
mod selection;
#[cfg(test)]
mod semantic_tests;

pub use hypothesis::*;
pub use result::*;
pub use selection::*;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::catalog::{Chart, ChartKey, Difficulty, PlayType, ScorepeekSongId};
use crate::event::{
    BestChart, CurrentSelectionDifficulty, DomainTransitionKind, EvidenceContribution,
    MusicSelectResolverState, MusicSelectionState, MusicSelectionUnresolvedReason,
    NumericResultEventSuppressionReason, NumericResultTemporalState, NumericResultTransitionReason,
    ResolverHypothesisKey, ResolverResolutionState, ResolverScope, ResultDomainEvent,
    ResultPanelSideEpisodeState, ResultPanelSideTransitionReason, ResultRetractionReason,
    ResultState, SelectFrameIdentity, SelectIdentityStatus, SelectionDifficultyTarget,
    SelectionDifficultyTransitionReason, SongPresentation,
};
use crate::recognition::music_select::PlaySide;
use crate::recognition::result::{
    ParsedResultFields, PlayOption, PlayOptions, PlayOptionsObservation, PlayOptionsUnknownReason,
    ResultPerformanceResolution, resolve_result_performance,
};
use crate::recognition::screen::ResultPanelSide;
use crate::recognition::screen::ScreenClass;
use crate::recognition::shared::{
    EvidenceFamily, JointEvidenceCandidate, JointEvidenceObservation,
};
use crate::session::attempt::{
    AcceptedPlayAttempt, PlayAttemptReason, PlayAttemptReducer, PlayAttemptScreen, PlayAttemptState,
};
use crate::session::timeline::SemanticEpisodePhase;
use serde::Serialize;
use std::convert::Infallible;

pub const NUMERIC_REQUIRED_OBSERVATIONS: u8 = 2;
pub const EVIDENCE_FAMILY_CAP: u16 = 300;
pub const JOINT_ACCEPT_SUPPORT: u16 = 260;
pub const JOINT_ACCEPT_MARGIN: u16 = 50;
pub const SELECTION_CHANGE_MARGIN: u16 = 120;
const PLAY_OPTIONS_REQUIRED_OBSERVATIONS: u8 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResolverTransitionIdentity {
    state: ResolverResolutionState,
    select_play_type: Option<PlayType>,
    result_play_type: Option<PlayType>,
    top: Option<ResolverHypothesisKey>,
    runner_up: Option<ResolverHypothesisKey>,
    runner_song: Option<ResolverHypothesisKey>,
    runner_chart: Option<ResolverHypothesisKey>,
}

fn build_result_domain_event(
    attempt: AcceptedPlayAttempt,
    numeric: &NumericResultView,
    play_side: PlaySide,
    play_options: PlayOptions,
) -> ResultDomainEvent {
    let ResultPerformanceResolution::Accepted {
        judgments,
        miss_count,
        timing,
        combo_break,
        previous_best,
        ..
    } = &numeric.performance
    else {
        unreachable!("accepted numeric view stores accepted performance");
    };
    ResultDomainEvent {
        contract: "scorepeek-result-detected-v4".to_owned(),
        attempt_id: attempt.attempt_id,
        parent_attempt_id: attempt.parent_attempt_id,
        scorepeek_song_id: numeric.song_id,
        play_side,
        play_mode: match numeric.chart.key.play_type {
            PlayType::Single => "single_play",
            PlayType::Double => "double_play",
        }
        .to_owned(),
        play_type: numeric.chart.key.play_type,
        difficulty: numeric.chart.key.difficulty,
        level: numeric.chart.level,
        notes: numeric.chart.notes,
        current_score: numeric.current_score,
        clear_type: numeric.clear_type.clone(),
        judgments: judgments.clone(),
        miss_count: miss_count.clone(),
        timing: timing.clone(),
        combo_break: combo_break.clone(),
        previous_best: previous_best.clone(),
        play_options,
    }
}

const fn result_play_side(panel_side: Option<ResultPanelSide>) -> Option<PlaySide> {
    let Some(panel_side) = panel_side else {
        return None;
    };
    Some(match panel_side {
        ResultPanelSide::Left => PlaySide::OnePlayer,
        ResultPanelSide::Right => PlaySide::TwoPlayer,
    })
}

fn play_attempt_screen(screen: ScreenClass) -> Option<PlayAttemptScreen> {
    match screen {
        ScreenClass::MusicSelect => Some(PlayAttemptScreen::MusicSelect),
        ScreenClass::DecideTransition => Some(PlayAttemptScreen::DecideTransition),
        ScreenClass::Play => Some(PlayAttemptScreen::Play),
        ScreenClass::Result => Some(PlayAttemptScreen::Result),
        _ => None,
    }
}

fn result_chart_factor(fields: &ParsedResultFields) -> ResultChartFactor {
    ResultChartFactor {
        play_type: fields.play_type.known().copied(),
        difficulty: fields.difficulty.known().copied(),
        notes: fields.notes.known().copied(),
        level: fields.level.known().copied(),
    }
}

fn resolver_node(
    scope: ResolverScope,
    accumulator: &HypothesisAccumulator,
    summary: &HypothesisSummary,
) -> ResolverNodeSnapshot {
    ResolverNodeSnapshot {
        scope,
        started_ms: accumulator.first_observation_ms,
        last_observation_ms: accumulator.last_observation_ms,
        observations: accumulator.observation_count,
        top: summary.selected.clone(),
        runner_up: summary.runner_up.clone(),
        runner_song: summary.runner_song.clone(),
        runner_chart: summary.runner_chart.clone(),
        top_candidates: summary.top_candidates.clone(),
        support: summary.support,
        margin: summary.margin,
        song_margin: summary.song_margin,
        chart_margin: summary.chart_margin,
        select_play_type: summary.select_play_type,
        result_play_type: summary.result_play_type,
        play_type_mismatch: summary.select_play_type.is_some()
            && summary.result_play_type.is_some()
            && summary.select_play_type != summary.result_play_type,
        family_contributions: summary.selected_family_support.clone(),
        current_difficulty: accumulator.select_difficulty,
        state: summary.state,
    }
}

fn attempt_node(
    state: &PlayAttemptState,
    started_ms: Option<u64>,
    phase_started_ms: Option<u64>,
    select: &HypothesisSummary,
    result: &HypothesisSummary,
    joint: &HypothesisSummary,
) -> Option<AttemptNodeSnapshot> {
    if matches!(state, PlayAttemptState::Idle) {
        return None;
    }
    let attempt_id = match state {
        PlayAttemptState::Attempt { attempt } => Some(attempt.attempt_id),
        PlayAttemptState::Idle | PlayAttemptState::UnlinkedResult { .. } => None,
    };
    Some(AttemptNodeSnapshot {
        attempt_id,
        started_ms,
        phase_started_ms,
        attempt_state: state.clone(),
        select_top: select.selected.clone(),
        result_top: result.selected.clone(),
        joint_top: joint.selected.clone(),
        support: joint.support,
        margin: joint.margin,
        song_margin: joint.song_margin,
        chart_margin: joint.chart_margin,
        runner_song: joint.runner_song.clone(),
        runner_chart: joint.runner_chart.clone(),
        top_candidates: joint.top_candidates.clone(),
        family_contributions: joint.selected_family_support.clone(),
        state: joint.state,
    })
}

/// A transport-neutral action produced by [`DomainReducer`].
#[allow(
    clippy::large_enum_variant,
    reason = "effects retain owned domain values in one ordered queue without extra indirection"
)]
#[derive(Clone, Debug)]
pub enum DomainEffect {
    Event(DomainTransitionKind),
    SessionEnded,
    Snapshot(DomainSnapshot),
}

/// Ordered output from one reducer input.
#[derive(Clone, Debug, Default)]
pub struct ReducedDomainTransitions {
    effects: Vec<DomainEffect>,
}

pub type DomainReductionError = Infallible;

#[derive(Clone, Debug, Default, Serialize)]
pub struct DomainSnapshot {
    pub now_ms: u64,
    pub raw_screen: Option<ScreenClass>,
    pub screen: Option<ScreenClass>,
    pub suspended: bool,
    pub finalizing: bool,
    pub screen_episode_id: u64,
    pub screen_episode_started_ms: Option<u64>,
    pub source_sequence: Option<u64>,
    pub latest_field_sequence: Option<u64>,
    pub latest_field_ms: Option<u64>,
    pub selection_difficulty_target: Option<SelectionDifficultyTarget>,
    pub selection_difficulty: Option<CurrentSelectionDifficulty>,
    pub local: Option<ResolverNodeSnapshot>,
    pub successor: Option<ResolverNodeSnapshot>,
    pub attempt: Option<AttemptNodeSnapshot>,
    pub gate: GateDecision,
    pub gates: Vec<GateSnapshot>,
    pub play_options: Option<PlayOptionsDebugSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlayOptionsDebugSnapshot {
    pub latest: PlayOptionsObservation,
    pub observations: u8,
    pub conflicting: bool,
    pub resolved: PlayOptions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    Accepted,
    Pending,
    Failed,
    Inactive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    Link,
    Identity,
    Clear,
    Numeric,
    Drain,
    Emit,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDecision {
    ResultConfirmed,
    #[default]
    JointIdentityPending,
    NumericPending,
    LinkedAttemptPending,
    Ready,
}

#[derive(Clone, Debug, Serialize)]
pub struct GateSnapshot {
    pub kind: GateKind,
    pub state: GateState,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolverNodeSnapshot {
    pub scope: ResolverScope,
    pub started_ms: Option<u64>,
    pub last_observation_ms: Option<u64>,
    pub observations: u32,
    pub top: Option<JointEvidenceCandidate>,
    pub runner_up: Option<JointEvidenceCandidate>,
    pub runner_song: Option<JointEvidenceCandidate>,
    pub runner_chart: Option<JointEvidenceCandidate>,
    pub top_candidates: Vec<JointEvidenceCandidate>,
    pub support: u16,
    pub margin: u16,
    pub song_margin: u16,
    pub chart_margin: u16,
    pub select_play_type: Option<PlayType>,
    pub result_play_type: Option<PlayType>,
    pub play_type_mismatch: bool,
    pub family_contributions: BTreeMap<EvidenceFamily, EvidenceContribution>,
    pub current_difficulty: Option<CurrentSelectionDifficulty>,
    pub state: ResolverResolutionState,
}

#[derive(Clone, Debug, Serialize)]
pub struct AttemptNodeSnapshot {
    pub attempt_id: Option<u64>,
    pub started_ms: Option<u64>,
    pub phase_started_ms: Option<u64>,
    pub attempt_state: PlayAttemptState,
    pub select_top: Option<JointEvidenceCandidate>,
    pub result_top: Option<JointEvidenceCandidate>,
    pub joint_top: Option<JointEvidenceCandidate>,
    pub support: u16,
    pub margin: u16,
    pub song_margin: u16,
    pub chart_margin: u16,
    pub runner_song: Option<JointEvidenceCandidate>,
    pub runner_chart: Option<JointEvidenceCandidate>,
    pub top_candidates: Vec<JointEvidenceCandidate>,
    pub family_contributions: BTreeMap<EvidenceFamily, EvidenceContribution>,
    pub state: ResolverResolutionState,
}

impl ReducedDomainTransitions {
    #[must_use]
    pub fn effects(&self) -> &[DomainEffect] {
        &self.effects
    }

    #[must_use]
    pub fn into_effects(self) -> Vec<DomainEffect> {
        self.effects
    }
}

/// Portable semantic state for reducing observations into ordered domain events.
#[allow(
    clippy::struct_excessive_bools,
    reason = "these booleans are independent reducer facts rather than one state machine axis"
)]
#[derive(Clone, Default)]
pub(crate) struct DomainReducer {
    engine: ResolverEngine,
    pending_numeric_result: Option<PendingNumericResult>,
    pending_supplemental_result: Option<PendingSupplementalResult>,
    accepted_numeric_result: Option<NumericResultView>,
    active_provisional_result: Option<ActiveProvisionalResult>,
    music_selection_revision: u64,
    music_select_resolver: MusicSelectResolver,
    published_music_select_resolver: MusicSelectResolverState,
    active_music_selection: Option<MusicSelectionState>,
    music_selection_episode_active: bool,
    play_options: PlayOptionsEpisodeAccumulator,
    result_panel_side: ResultPanelSideAccumulator,
    result_select_context_detached: bool,
    numeric_evidence: VecDeque<RawNumericEvidence>,
    last_numeric_sequence: Option<u64>,
    last_numeric_monotonic_ms: Option<u64>,
    emitted_attempt_ids: BTreeSet<u64>,
    latest_screen_boundary_sequence: Option<u64>,
    screen_episode_id: u64,
    screen_episode_started_ms: Option<u64>,
    screen_episode_last_ms: Option<u64>,
    result_resolver_active: bool,
    result_episode_finalizing: bool,
    semantic_episode_suspended: bool,
    resolver_transitions: BTreeMap<ResolverScope, ResolverTransitionIdentity>,
    attempt_started_ms: Option<u64>,
    attempt_phase_started_ms: Option<u64>,
    active_session_id: Option<String>,
    current_screen: Option<ScreenClass>,
    raw_screen: Option<ScreenClass>,
    resolver_now_ms: u64,
    resolver_source_sequence: Option<u64>,
    latest_field_sequence: Option<u64>,
    latest_field_ms: Option<u64>,
    effects: Vec<DomainEffect>,
}

impl DomainReducer {
    #[cfg(test)]
    #[must_use]
    pub fn new() -> Self {
        Self {
            numeric_evidence: VecDeque::with_capacity(8),
            ..Self::default()
        }
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "event emission shares the reducer's uniform fallible pipeline contract"
    )]
    fn emit(&mut self, event: DomainTransitionKind) -> Result<(), DomainReductionError> {
        self.effects.push(DomainEffect::Event(event));
        Ok(())
    }

    pub(super) fn finish(&mut self) -> ReducedDomainTransitions {
        ReducedDomainTransitions {
            effects: std::mem::take(&mut self.effects),
        }
    }

    fn publish_one(&mut self, event: &DomainTransitionKind) -> Result<(), DomainReductionError> {
        self.emit(event.clone())
    }

    fn sync_music_select_resolver_state(&mut self) -> Result<(), DomainReductionError> {
        let active = self.current_screen == Some(ScreenClass::MusicSelect)
            && self.music_selection_episode_active;
        let difficulty = self
            .music_select_resolver
            .selection_epochs
            .active_difficulty_state();
        let best = &mut self.music_select_resolver.best;
        if !active {
            *best = MusicSelectResolverState::default();
        }
        if active {
            best.current_difficulty = difficulty.and_then(|(_, current)| current);
            best.difficulty_target = difficulty.map(|(target, _)| target);
        }
        best.active = active;
        best.suspended = active && self.semantic_episode_suspended;
        best.screen_episode_id = self.screen_episode_id;
        if best.same_notification(&self.published_music_select_resolver) {
            return Ok(());
        }
        let state = best.clone();
        self.published_music_select_resolver = state.clone();
        self.publish_one(&DomainTransitionKind::MusicSelectResolverChanged {
            session_id: self.active_session_id.clone(),
            state,
        })
    }

    fn sync_resolver_snapshot(
        &mut self,
        now_ms: u64,
        source_sequence: Option<u64>,
        field_observed: bool,
    ) -> Result<(), DomainReductionError> {
        self.screen_episode_last_ms = Some(now_ms);
        self.resolver_now_ms = now_ms;
        self.resolver_source_sequence = source_sequence;
        if field_observed {
            self.latest_field_sequence = source_sequence;
            self.latest_field_ms = Some(now_ms);
        }
        self.sync_music_select_resolver_state()?;
        self.effects.push(DomainEffect::Snapshot(self.snapshot()));
        Ok(())
    }

    /// Reduces one typed domain input and returns ordered domain decisions.
    ///
    /// # Errors
    /// This reducer is currently infallible. The result keeps the boundary explicit for future
    /// schema-level validation without coupling consumers to implementation state.
    pub(crate) fn reduce(
        &mut self,
        input: &crate::event::DomainInput,
    ) -> Result<ReducedDomainTransitions, DomainReductionError> {
        debug_assert!(self.effects.is_empty());
        self.publish_internal(input)?;
        Ok(self.finish())
    }

    #[allow(clippy::too_many_arguments)]
    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "the snapshot is an exhaustive projection of reducer authority"
    )]
    pub fn snapshot(&self) -> DomainSnapshot {
        let current_summary = self.engine.selection_epochs.incumbent.summary();
        let challenger_summary = self.engine.selection_epochs.successor.summary();
        let result_summary = self.engine.result_hypotheses.summary();
        let mut joint = self.engine.retained_select.clone();
        joint.add_from(&self.engine.result_hypotheses);
        let joint_summary = joint.summary();
        let local = match self.current_screen {
            Some(ScreenClass::MusicSelect) => Some(resolver_node(
                ResolverScope::SelectionIncumbent,
                &self.engine.selection_epochs.incumbent,
                &current_summary,
            )),
            Some(ScreenClass::Result) => Some(resolver_node(
                ResolverScope::Result,
                &self.engine.result_hypotheses,
                &result_summary,
            )),
            _ => None,
        };
        let successor = (self.current_screen == Some(ScreenClass::MusicSelect)
            && self.engine.selection_epochs.successor.observation_count > 0)
            .then(|| {
                resolver_node(
                    ResolverScope::SelectionSuccessor,
                    &self.engine.selection_epochs.successor,
                    &challenger_summary,
                )
            });
        let attempt = attempt_node(
            self.engine.play_attempt.state(),
            self.attempt_started_ms,
            self.attempt_phase_started_ms,
            &self.engine.retained_select.summary(),
            &result_summary,
            &joint_summary,
        );
        let emitted = self
            .engine
            .play_attempt
            .accepted_result()
            .is_some_and(|attempt| self.emitted_attempt_ids.contains(&attempt.attempt_id));
        let attempt_completed_rejected = matches!(
            self.engine.play_attempt.state(),
            PlayAttemptState::Attempt { attempt }
                if matches!(attempt.phase, crate::session::attempt::PlayAttemptPhase::Completed)
                    && !attempt.reasons.is_empty()
        );
        let linked = matches!(
            self.engine.play_attempt.state(),
            PlayAttemptState::Attempt { attempt }
                if attempt.path.select_observed
                    && attempt.path.play_observed
                    && attempt.path.result_observed
        );
        let gate = if emitted {
            GateDecision::ResultConfirmed
        } else if joint_summary.state != ResolverResolutionState::AcceptedJoint {
            GateDecision::JointIdentityPending
        } else if self.accepted_numeric_result.is_none() {
            GateDecision::NumericPending
        } else if self.engine.play_attempt.accepted_result().is_none() {
            GateDecision::LinkedAttemptPending
        } else {
            GateDecision::Ready
        };
        let gate_state = |accepted: bool| {
            if accepted {
                GateState::Accepted
            } else if attempt_completed_rejected {
                GateState::Failed
            } else {
                GateState::Pending
            }
        };
        let gates = vec![
            GateSnapshot {
                kind: GateKind::Link,
                state: gate_state(linked),
            },
            GateSnapshot {
                kind: GateKind::Identity,
                state: match joint_summary.state {
                    ResolverResolutionState::AcceptedJoint => GateState::Accepted,
                    ResolverResolutionState::Conflict => GateState::Failed,
                    _ if attempt_completed_rejected => GateState::Failed,
                    _ => GateState::Pending,
                },
            },
            GateSnapshot {
                kind: GateKind::Clear,
                state: gate_state(self.accepted_numeric_result.is_some()),
            },
            GateSnapshot {
                kind: GateKind::Numeric,
                state: gate_state(self.accepted_numeric_result.is_some()),
            },
            GateSnapshot {
                kind: GateKind::Drain,
                state: if self.result_episode_finalizing {
                    GateState::Pending
                } else if emitted || attempt_completed_rejected {
                    GateState::Accepted
                } else {
                    GateState::Inactive
                },
            },
            GateSnapshot {
                kind: GateKind::Emit,
                state: if emitted {
                    GateState::Accepted
                } else if attempt_completed_rejected {
                    GateState::Failed
                } else {
                    GateState::Inactive
                },
            },
        ];
        let selection_difficulty = self.engine.selection_epochs.active_difficulty_state();
        DomainSnapshot {
            now_ms: self.resolver_now_ms,
            raw_screen: self.raw_screen,
            screen: self.current_screen,
            suspended: self.semantic_episode_suspended,
            finalizing: self.result_episode_finalizing,
            screen_episode_id: self.screen_episode_id,
            screen_episode_started_ms: self.screen_episode_started_ms,
            source_sequence: self.resolver_source_sequence,
            latest_field_sequence: self.latest_field_sequence,
            latest_field_ms: self.latest_field_ms,
            selection_difficulty_target: selection_difficulty.map(|(target, _)| target),
            selection_difficulty: selection_difficulty.and_then(|(_, current)| current),
            local,
            successor,
            attempt,
            gate,
            gates,
            play_options: self.play_options.latest().cloned().map(|latest| {
                PlayOptionsDebugSnapshot {
                    latest,
                    observations: self.play_options.observations(),
                    conflicting: self.play_options.conflicting(),
                    resolved: self.play_options.resolved(),
                }
            }),
        }
    }
}

#[must_use]
pub fn candidate_song_presentation(candidate: &JointEvidenceCandidate) -> SongPresentation {
    SongPresentation {
        scorepeek_song_id: candidate.song_id,
        display_titles: candidate.display_titles.clone(),
        artist: candidate.artist.clone(),
    }
}

#[must_use]
pub fn resolver_hypothesis_key(candidate: &JointEvidenceCandidate) -> ResolverHypothesisKey {
    ResolverHypothesisKey::new(candidate.song_id, candidate.chart.key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_episode_preserves_event_and_snapshot_checkpoint_order() {
        let mut reducer = DomainReducer::new();
        let input = crate::event::DomainInput::SemanticScreenEpisodeChanged {
            session_id: Some("session-1".to_owned()),
            screen_episode_id: 7,
            sequence: 11,
            monotonic_end_ms: 1_100,
            screen: ScreenClass::MusicSelect,
            phase: SemanticEpisodePhase::Started,
        };

        let effects = reducer.reduce(&input).unwrap().into_effects();
        assert!(matches!(effects.first(), Some(DomainEffect::Event(_))));
        assert!(effects.iter().any(|effect| matches!(
            effect,
            DomainEffect::Snapshot(snapshot)
                if snapshot.screen_episode_id == 7
                    && snapshot.source_sequence == Some(11)
        )));
    }
}
