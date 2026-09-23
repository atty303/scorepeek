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
    BestChart, CurrentSelectionDifficulty, EvidenceContribution, MusicSelectResolverState,
    MusicSelectionState, MusicSelectionUnresolvedReason, NumericResultEventSuppressionReason,
    NumericResultTemporalState, NumericResultTransitionReason, ResolverHypothesisKey,
    ResolverResolutionState, ResolverScope, ResultDomainEvent, ResultPanelSideEpisodeState,
    ResultPanelSideTransitionReason, ResultRetractionReason, ResultState, RunEvent, RunEventKind,
    SelectFrameIdentity, SelectIdentityStatus, SelectionDifficultyTarget,
    SelectionDifficultyTransitionReason, SongPresentation, SongResolutionPresentation,
};
use crate::recognition::music_select::PlaySide;
use crate::recognition::result::{
    ParsedResultFields, PlayOption, PlayOptions, PlayOptionsObservation, PlayOptionsUnknownReason,
    ResultPerformanceResolution, resolve_result_performance,
};
use crate::recognition::screen::ResultPanelSide;
use crate::recognition::shared::{
    EvidenceFamily, JointEvidenceCandidate, JointEvidenceObservation,
};
use crate::session::attempt::{
    AcceptedPlayAttempt, PlayAttemptReason, PlayAttemptReducer, PlayAttemptScreen, PlayAttemptState,
};
use crate::session::timeline::SemanticEpisodePhase;
use serde::Serialize;
use serde_json::Value;
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

fn play_attempt_screen(screen: &str) -> Option<PlayAttemptScreen> {
    match screen {
        "music_select" => Some(PlayAttemptScreen::MusicSelect),
        "decide_transition" => Some(PlayAttemptScreen::DecideTransition),
        "play" => Some(PlayAttemptScreen::Play),
        "result" => Some(PlayAttemptScreen::Result),
        _ => None,
    }
}

fn selected_play_side(fields: &Value) -> Option<PlaySide> {
    let value = fields.pointer("/play_side/state")?.get("value")?.as_str()?;
    match value {
        "one_player" => Some(PlaySide::OnePlayer),
        "two_player" => Some(PlaySide::TwoPlayer),
        _ => None,
    }
}

fn result_panel_side(fields: &Value) -> Option<ResultPanelSide> {
    serde_json::from_value(fields.get("panel_side")?.clone()).ok()
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
    label: &'static str,
    accumulator: &HypothesisAccumulator,
    summary: &HypothesisSummary,
) -> ResolverNodeSnapshot {
    ResolverNodeSnapshot {
        label,
        started_ms: accumulator.first_observation_ms,
        last_observation_ms: accumulator.last_observation_ms,
        observations: accumulator.observation_count,
        top: summary.selected.as_ref().map(candidate_label),
        runner_up: summary.runner_up.as_ref().map(candidate_label),
        runner_song: summary.runner_song.as_ref().map(candidate_label),
        runner_chart: summary.runner_chart.as_ref().map(candidate_label),
        top_candidates: summary.top_candidates.iter().map(candidate_label).collect(),
        support: summary.support,
        margin: summary.margin,
        song_margin: summary.song_margin,
        chart_margin: summary.chart_margin,
        select_play_type: summary.select_play_type,
        result_play_type: summary.result_play_type,
        play_type_mismatch: summary.select_play_type.is_some()
            && summary.result_play_type.is_some()
            && summary.select_play_type != summary.result_play_type,
        family_contributions: family_contribution_labels(&summary.selected_family_support),
        current_difficulty: accumulator.select_difficulty,
        state: summary.state,
    }
}

fn candidate_label(candidate: &JointEvidenceCandidate) -> String {
    let title = candidate.display_titles.first().map_or("?", String::as_str);
    format!(
        "{} / {:?} {:?} Lv{} notes={}",
        title,
        play_type_label(candidate.chart.key.play_type),
        difficulty_label(candidate.chart.key.difficulty),
        candidate.chart.level,
        candidate.chart.notes,
    )
}

const fn play_type_label(play_type: PlayType) -> &'static str {
    match play_type {
        PlayType::Single => "SP",
        PlayType::Double => "DP",
    }
}

const fn difficulty_label(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Beginner => "BEGINNER",
        Difficulty::Normal => "NORMAL",
        Difficulty::Hyper => "HYPER",
        Difficulty::Another => "ANOTHER",
        Difficulty::Leggendaria => "LEGGENDARIA",
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
    let (attempt_id, phase, path) = match state {
        PlayAttemptState::Idle => return None,
        PlayAttemptState::UnlinkedResult { .. } => {
            (None, "unlinked_result".to_owned(), "R".to_owned())
        }
        PlayAttemptState::Attempt { attempt } => {
            let mut path = String::new();
            for (observed, label) in [
                (attempt.path.select_observed, 'S'),
                (attempt.path.decide_observed, 'D'),
                (attempt.path.play_observed, 'P'),
                (attempt.path.result_observed, 'R'),
            ] {
                if observed {
                    if !path.is_empty() {
                        path.push('-');
                    }
                    path.push(label);
                }
            }
            (
                Some(attempt.attempt_id),
                format!("{:?}", attempt.phase).to_ascii_lowercase(),
                path,
            )
        }
    };
    Some(AttemptNodeSnapshot {
        attempt_id,
        started_ms,
        phase_started_ms,
        phase,
        path,
        select_top: select.selected.as_ref().map(candidate_label),
        result_top: result.selected.as_ref().map(candidate_label),
        joint_top: joint.selected.as_ref().map(candidate_label),
        support: joint.support,
        margin: joint.margin,
        song_margin: joint.song_margin,
        chart_margin: joint.chart_margin,
        runner_song: joint.runner_song.as_ref().map(candidate_label),
        runner_chart: joint.runner_chart.as_ref().map(candidate_label),
        top_candidates: joint.top_candidates.iter().map(candidate_label).collect(),
        family_contributions: family_contribution_labels(&joint.selected_family_support),
        state: joint.state,
    })
}

fn family_contribution_labels(
    contributions: &BTreeMap<EvidenceFamily, EvidenceContribution>,
) -> Vec<String> {
    let mut values = contributions
        .iter()
        .map(|(family, contribution)| {
            format!(
                "{}={}",
                format!("{family:?}").to_ascii_lowercase(),
                contribution.normalized()
            )
        })
        .collect::<Vec<_>>();
    values.sort();
    values
}

#[allow(
    clippy::too_many_lines,
    reason = "the snapshot projection enumerates a fixed set of diagnostic fields"
)]
fn important_raw_fields(fields: &Value) -> Vec<(String, String)> {
    let marker = fields.get("selected_difficulty").and_then(|observation| {
        let state = observation.get("state")?;
        let status = state.get("status")?.as_str()?;
        let value = state.get("value").and_then(Value::as_str).unwrap_or("-");
        let winner = observation
            .get("winner_score_ppm")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let margin = observation
            .get("margin_ppm")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Some((
            "marker".to_owned(),
            format!("{status}:{value} score={winner} margin={margin}"),
        ))
    });
    let select_play_type = fields.get("play_type").and_then(|observation| {
        let state = observation.get("state")?;
        let status = state.get("status")?.as_str()?;
        let value = state.get("value").and_then(Value::as_str).unwrap_or("-");
        let single = observation
            .get("single_score_ppm")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let double = observation
            .get("double_score_ppm")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Some((
            "select_play_type".to_owned(),
            format!("{status}:{value} sp={single} dp={double}"),
        ))
    });
    let play_side = fields.get("play_side").and_then(|observation| {
        let state = observation.get("state")?;
        let status = state.get("status")?.as_str()?;
        let value = state.get("value").and_then(Value::as_str).unwrap_or("-");
        let winner = observation
            .get("winner_bright_pixels")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let margin = observation
            .get("margin")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        Some((
            "play_side".to_owned(),
            format!("{status}:{value} bright={winner} margin={margin}"),
        ))
    });
    let title_foreground = fields.get("title_evidence").map(|evidence| {
        let raw = evidence
            .pointer("/foreground/open_text")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let scalar_count = evidence
            .get("normalized_scalar_count")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let width = evidence
            .pointer("/geometry/occupancy_width_ppm")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let edge = evidence
            .get("geometry")
            .map(|geometry| {
                format!(
                    "{}{}",
                    if geometry["touches_left_edge"].as_bool().unwrap_or(false) {
                        "L"
                    } else {
                        ""
                    },
                    if geometry["touches_right_edge"].as_bool().unwrap_or(false) {
                        "R"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_default();
        (
            "title_fg".to_owned(),
            format!("{raw} chars={scalar_count} width_ppm={width} edge={edge}"),
        )
    });
    marker
        .into_iter()
        .chain(select_play_type)
        .chain(play_side)
        .chain(title_foreground)
        .chain(
            [
                "title",
                "central_title",
                "active_list_title",
                "artist",
                "play_type",
                "difficulty",
                "current_score",
                "pgreat",
                "great",
                "good",
                "bad",
                "poor",
            ]
            .into_iter()
            .filter_map(|key| {
                fields
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(|value| (key.to_owned(), value.to_owned()))
            }),
        )
        .take(8)
        .collect()
}

/// A transport-neutral action produced by [`RunEventReducer`].
#[allow(
    clippy::large_enum_variant,
    reason = "effects retain owned domain values in one ordered queue without extra indirection"
)]
#[derive(Clone, Debug)]
pub enum RunReducerEffect {
    Event(RunEvent),
    ClearFieldObservation,
    Snapshot(RunReducerSnapshot),
    Refresh,
    FinishScores,
}

/// Ordered output from one reducer input.
#[derive(Clone, Debug, Default)]
pub struct ReducedRunEvents {
    effects: Vec<RunReducerEffect>,
}

pub type RunEventReductionError = Infallible;

#[derive(Clone, Debug, Default, Serialize)]
pub struct RunReducerSnapshot {
    pub now_ms: u64,
    pub raw_screen: Option<String>,
    pub screen: Option<String>,
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
    pub gate: String,
    pub gates: Vec<GateSnapshot>,
    pub raw_fields: Vec<(String, String)>,
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

#[derive(Clone, Debug, Serialize)]
pub struct GateSnapshot {
    pub label: &'static str,
    pub state: GateState,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolverNodeSnapshot {
    pub label: &'static str,
    pub started_ms: Option<u64>,
    pub last_observation_ms: Option<u64>,
    pub observations: u32,
    pub top: Option<String>,
    pub runner_up: Option<String>,
    pub runner_song: Option<String>,
    pub runner_chart: Option<String>,
    pub top_candidates: Vec<String>,
    pub support: u16,
    pub margin: u16,
    pub song_margin: u16,
    pub chart_margin: u16,
    pub select_play_type: Option<PlayType>,
    pub result_play_type: Option<PlayType>,
    pub play_type_mismatch: bool,
    pub family_contributions: Vec<String>,
    pub current_difficulty: Option<CurrentSelectionDifficulty>,
    pub state: ResolverResolutionState,
}

#[derive(Clone, Debug, Serialize)]
pub struct AttemptNodeSnapshot {
    pub attempt_id: Option<u64>,
    pub started_ms: Option<u64>,
    pub phase_started_ms: Option<u64>,
    pub phase: String,
    pub path: String,
    pub select_top: Option<String>,
    pub result_top: Option<String>,
    pub joint_top: Option<String>,
    pub support: u16,
    pub margin: u16,
    pub song_margin: u16,
    pub chart_margin: u16,
    pub runner_song: Option<String>,
    pub runner_chart: Option<String>,
    pub top_candidates: Vec<String>,
    pub family_contributions: Vec<String>,
    pub state: ResolverResolutionState,
}

impl ReducedRunEvents {
    #[must_use]
    pub fn effects(&self) -> &[RunReducerEffect] {
        &self.effects
    }

    #[must_use]
    pub fn into_effects(self) -> Vec<RunReducerEffect> {
        self.effects
    }
}

/// Portable semantic state for reducing observations into ordered domain events.
#[allow(
    clippy::struct_excessive_bools,
    reason = "these booleans are independent reducer facts rather than one state machine axis"
)]
#[derive(Clone, Default)]
pub struct RunEventReducer {
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
    current_screen: Option<String>,
    raw_screen: Option<String>,
    resolver_now_ms: u64,
    resolver_source_sequence: Option<u64>,
    latest_field_sequence: Option<u64>,
    latest_field_ms: Option<u64>,
    raw_fields: Vec<(String, String)>,
    effects: Vec<RunReducerEffect>,
}

impl RunEventReducer {
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
    fn emit(&mut self, event: RunEvent) -> Result<(), RunEventReductionError> {
        match &event.kind {
            RunEventKind::CanonicalSessionStarted { session_id } => {
                self.active_session_id = Some(session_id.clone());
                self.current_screen = None;
                self.raw_screen = None;
            }
            RunEventKind::SessionStarted { session_id, .. } => {
                self.active_session_id.clone_from(session_id);
                self.current_screen = None;
                self.raw_screen = None;
            }
            RunEventKind::RawScreenObserved { screen, .. } => {
                self.raw_screen = Some(screen.clone());
            }
            RunEventKind::ScreenChanged { screen, .. } => {
                self.current_screen = Some(screen.clone());
            }
            RunEventKind::SemanticScreenEpisodeChanged { screen, phase, .. } => match phase {
                SemanticEpisodePhase::Started | SemanticEpisodePhase::Resumed => {
                    self.current_screen = Some(screen.clone());
                }
                SemanticEpisodePhase::Finalized => self.current_screen = None,
                SemanticEpisodePhase::Suspended | SemanticEpisodePhase::Closing => {}
            },
            RunEventKind::SessionFinished { .. }
            | RunEventKind::CanonicalSessionFinished { .. } => {
                self.active_session_id = None;
                self.current_screen = None;
            }
            _ => {}
        }
        self.effects.push(RunReducerEffect::Event(event));
        Ok(())
    }

    fn finish(&mut self) -> ReducedRunEvents {
        ReducedRunEvents {
            effects: std::mem::take(&mut self.effects),
        }
    }

    fn publish_one(&mut self, event: &RunEvent) -> Result<(), RunEventReductionError> {
        self.emit(event.clone())
    }

    #[allow(
        clippy::unnecessary_wraps,
        reason = "refresh effects share the reducer's uniform fallible pipeline contract"
    )]
    fn refresh(&mut self) -> Result<(), RunEventReductionError> {
        self.effects.push(RunReducerEffect::Refresh);
        Ok(())
    }

    fn sync_music_select_resolver_state(&mut self) -> Result<(), RunEventReductionError> {
        let active = self.current_screen.as_deref() == Some("music_select")
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
        self.publish_one(&RunEvent {
            schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::MusicSelectResolverChanged {
                session_id: self.active_session_id.clone(),
                state,
            },
        })
    }

    fn sync_resolver_snapshot(
        &mut self,
        now_ms: u64,
        source_sequence: Option<u64>,
        raw_fields: Option<&Value>,
    ) -> Result<(), RunEventReductionError> {
        self.screen_episode_last_ms = Some(now_ms);
        self.resolver_now_ms = now_ms;
        self.resolver_source_sequence = source_sequence;
        if let Some(fields) = raw_fields {
            self.latest_field_sequence = source_sequence;
            self.latest_field_ms = Some(now_ms);
            self.raw_fields = important_raw_fields(fields);
        }
        self.sync_music_select_resolver_state()?;
        self.effects
            .push(RunReducerEffect::Snapshot(self.snapshot()));
        Ok(())
    }

    /// Reduces one transport-neutral run event and returns all ordered semantic effects.
    ///
    /// # Errors
    /// This reducer is currently infallible. The result keeps the boundary explicit for future
    /// schema-level validation without coupling consumers to implementation state.
    pub fn reduce(&mut self, event: &RunEvent) -> Result<ReducedRunEvents, RunEventReductionError> {
        debug_assert!(self.effects.is_empty());
        self.publish_internal(event)?;
        Ok(self.finish())
    }

    #[must_use]
    #[allow(
        clippy::too_many_lines,
        reason = "the snapshot is an exhaustive projection of reducer authority"
    )]
    pub fn snapshot(&self) -> RunReducerSnapshot {
        let current_summary = self.engine.selection_epochs.incumbent.summary();
        let challenger_summary = self.engine.selection_epochs.successor.summary();
        let result_summary = self.engine.result_hypotheses.summary();
        let mut joint = self.engine.retained_select.clone();
        joint.add_from(&self.engine.result_hypotheses);
        let joint_summary = joint.summary();
        let local = match self.current_screen.as_deref() {
            Some("music_select") => Some(resolver_node(
                "MUSIC SELECT resolver",
                &self.engine.selection_epochs.incumbent,
                &current_summary,
            )),
            Some("result") => Some(resolver_node(
                "RESULT resolver",
                &self.engine.result_hypotheses,
                &result_summary,
            )),
            _ => None,
        };
        let successor = (self.current_screen.as_deref() == Some("music_select")
            && self.engine.selection_epochs.successor.observation_count > 0)
            .then(|| {
                resolver_node(
                    "successor",
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
            "accepted: result confirmed"
        } else if joint_summary.state != ResolverResolutionState::AcceptedJoint {
            "waiting: joint identity"
        } else if self.accepted_numeric_result.is_none() {
            "waiting: numeric performance"
        } else if self.engine.play_attempt.accepted_result().is_none() {
            "waiting: linked play attempt"
        } else {
            "ready: domain promotion"
        }
        .to_owned();
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
                label: "link",
                state: gate_state(linked),
            },
            GateSnapshot {
                label: "identity",
                state: match joint_summary.state {
                    ResolverResolutionState::AcceptedJoint => GateState::Accepted,
                    ResolverResolutionState::Conflict => GateState::Failed,
                    _ if attempt_completed_rejected => GateState::Failed,
                    _ => GateState::Pending,
                },
            },
            GateSnapshot {
                label: "clear",
                state: gate_state(self.accepted_numeric_result.is_some()),
            },
            GateSnapshot {
                label: "numeric",
                state: gate_state(self.accepted_numeric_result.is_some()),
            },
            GateSnapshot {
                label: "drain",
                state: if self.result_episode_finalizing {
                    GateState::Pending
                } else if emitted || attempt_completed_rejected {
                    GateState::Accepted
                } else {
                    GateState::Inactive
                },
            },
            GateSnapshot {
                label: "emit",
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
        RunReducerSnapshot {
            now_ms: self.resolver_now_ms,
            raw_screen: self.raw_screen.clone(),
            screen: self.current_screen.clone(),
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
            raw_fields: self.raw_fields.clone(),
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
    fn semantic_episode_preserves_event_then_snapshot_checkpoint_order() {
        let mut reducer = RunEventReducer::new();
        let input = RunEvent {
            schema: crate::event::RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::SemanticScreenEpisodeChanged {
                session_id: Some("session-1".to_owned()),
                screen_episode_id: 7,
                sequence: 11,
                monotonic_end_ms: 1_100,
                screen: "music_select".to_owned(),
                phase: SemanticEpisodePhase::Started,
            },
        };

        let effects = reducer.reduce(&input).unwrap().into_effects();
        assert!(matches!(
            effects.first(),
            Some(RunReducerEffect::Event(RunEvent {
                kind: RunEventKind::SemanticScreenEpisodeChanged { sequence: 11, .. },
                ..
            }))
        ));
        assert!(matches!(
            effects.get(1),
            Some(RunReducerEffect::ClearFieldObservation)
        ));
        assert!(matches!(effects.last(), Some(RunReducerEffect::Refresh)));
        assert!(effects.iter().rev().skip(1).any(|effect| matches!(
            effect,
            RunReducerEffect::Snapshot(snapshot)
                if snapshot.screen_episode_id == 7
                    && snapshot.source_sequence == Some(11)
        )));
    }
}
