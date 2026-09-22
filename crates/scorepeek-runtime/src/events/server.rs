#![allow(
    clippy::missing_errors_doc,
    reason = "internal event service methods retain their existing operation-specific errors"
)]

#[cfg(test)]
use super::music_select_best;
use super::snapshot as event_api;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs::{self, DirBuilder};
use std::io::{self, Write as _};
use std::os::unix::fs::{
    DirBuilderExt as _, FileTypeExt as _, MetadataExt as _, PermissionsExt as _,
};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::diagnostics::inspect::{DiagnosticSink, RunDiagnostics};
#[cfg(test)]
use ratatui::layout::{Constraint, Direction, Layout};
#[cfg(test)]
use ratatui::style::{Color, Modifier, Style};
#[cfg(test)]
use ratatui::text::{Line, Span};
#[cfg(test)]
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use scorepeek_core::catalog::{Difficulty, PlayType, ScorepeekSongId};
use scorepeek_core::event::{
    BestChart, CurrentSelectionDifficulty, EvidenceContribution, MusicSelectResolverState,
    MusicSelectionState, MusicSelectionUnresolvedReason, NumericResultEventSuppressionReason,
    NumericResultTemporalState, NumericResultTransitionReason, RUN_EVENT_SCHEMA,
    ResolverHypothesisKey, ResolverResolutionState, ResolverScope, ResultDomainEvent,
    ResultPanelSideEpisodeState, ResultPanelSideTransitionReason, ResultRetractionReason,
    ResultState, RunEvent, RunEventKind, SelectFrameIdentity, SelectIdentityStatus,
    SelectionDifficultyTarget, SelectionDifficultyTransitionReason, SongPresentation,
    SongResolutionPresentation,
};
use scorepeek_core::recognition::music_select::PlaySide;
use scorepeek_core::recognition::result::{
    ParsedResultFields, PlayOption, PlayOptions, PlayOptionsObservation, PlayOptionsUnknownReason,
    ResultPerformanceResolution, resolve_result_performance,
};
#[cfg(test)]
use scorepeek_core::recognition::result::{
    PreviousBest, PreviousBestValue, ResultChartResolution, ResultJudgments, ResultTiming,
    SupplementalResultValue,
};
use scorepeek_core::recognition::screen::ResultPanelSide;
use scorepeek_core::recognition::shared::{
    EvidenceFamily, JointEvidenceCandidate, JointEvidenceObservation,
};
use scorepeek_core::session::attempt::{
    AcceptedPlayAttempt, PlayAttemptReason, PlayAttemptReducer, PlayAttemptScreen, PlayAttemptState,
};
use scorepeek_core::session::timeline::SemanticEpisodePhase;
use serde::Serialize;
use serde_json::{Value, json};

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

const MAX_CLIENTS: usize = 8;
const EVENT_QUEUE_CAPACITY: usize = 64;
const RESULT_HISTORY_CAPACITY: usize = 32;
const SOCKET_NAME: &str = "events.sock";
const NUMERIC_REQUIRED_OBSERVATIONS: u8 = 2;
const PLAY_OPTIONS_REQUIRED_OBSERVATIONS: u8 = 2;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveProvisionalResult {
    song: Option<SongPresentation>,
    result: ResultDomainEvent,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NumericResultView {
    song_id: ScorepeekSongId,
    clear_type: String,
    chart: scorepeek_core::catalog::Chart,
    current_score: u32,
    performance: ResultPerformanceResolution,
    source_sequence: u64,
}

#[derive(Clone, Debug)]
struct PendingNumericResult {
    view: NumericResultView,
    observations: u8,
}

#[derive(Clone, Debug)]
struct PendingSupplementalResult {
    performance: ResultPerformanceResolution,
    source_sequence: u64,
    observations: u8,
}

#[derive(Clone, Debug)]
struct NumericResultTransition {
    state: NumericResultTemporalState,
    reason: NumericResultTransitionReason,
    replaced_accepted: bool,
}

#[derive(Clone, Debug, Default)]
struct PlayOptionsEpisodeAccumulator {
    candidate: Option<Vec<PlayOption>>,
    observations: u8,
    conflicting: bool,
    fallback_reason: Option<PlayOptionsUnknownReason>,
    latest: Option<PlayOptionsObservation>,
    last_sequence: Option<u64>,
}

impl PlayOptionsEpisodeAccumulator {
    fn observe(&mut self, sequence: u64, observation: PlayOptionsObservation) {
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

    fn resolved(&self) -> PlayOptions {
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
}

#[derive(Clone, Debug, Default)]
struct ResultPanelSideAccumulator {
    episode_id: Option<u64>,
    observations: BTreeMap<ResultPanelSide, u8>,
    stable: Option<ResultPanelSide>,
    opposite_observations: u8,
    last_sequence: Option<u64>,
    conflicted: bool,
}

impl ResultPanelSideAccumulator {
    fn start_episode(&mut self, episode_id: u64) {
        if self.episode_id != Some(episode_id) {
            *self = Self {
                episode_id: Some(episode_id),
                ..Self::default()
            };
        }
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    const fn stable(&self) -> Option<ResultPanelSide> {
        if self.conflicted { None } else { self.stable }
    }

    fn observe(
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

fn joint_matches_numeric(candidate: &JointEvidenceCandidate, numeric: &NumericResultView) -> bool {
    candidate.song_id == numeric.song_id && candidate.chart == numeric.chart
}

#[derive(Clone, Debug)]
struct RawNumericEvidence {
    sequence: u64,
    monotonic_end_ms: u64,
    clear_type: String,
    parsed: ParsedResultFields,
}

fn same_numeric_tuple(left: &NumericResultView, right: &NumericResultView) -> bool {
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

fn retain_supplemental_result(
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

fn candidate_song_presentation(candidate: &JointEvidenceCandidate) -> SongPresentation {
    SongPresentation {
        scorepeek_song_id: candidate.song_id,
        display_titles: candidate.display_titles.clone(),
        artist: candidate.artist.clone(),
    }
}

fn resolver_hypothesis_key(candidate: &JointEvidenceCandidate) -> ResolverHypothesisKey {
    ResolverHypothesisKey::new(candidate.song_id, candidate.chart.key)
}

const EVIDENCE_FAMILY_CAP: u16 = 300;
const JOINT_ACCEPT_SUPPORT: u16 = 260;
const JOINT_ACCEPT_MARGIN: u16 = 50;
const SELECTION_CHANGE_MARGIN: u16 = 120;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct JointKey {
    song_id: ScorepeekSongId,
    chart_key: scorepeek_core::catalog::ChartKey,
}

#[derive(Clone, Debug)]
struct AccumulatedHypothesis {
    candidate: JointEvidenceCandidate,
    family_support: BTreeMap<EvidenceFamily, u64>,
}

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

#[derive(Clone, Debug)]
struct RankedHypothesis<'a> {
    accumulated: &'a AccumulatedHypothesis,
    family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
    support: u16,
}

#[derive(Clone, Debug, Default)]
struct HypothesisAccumulator {
    candidates: BTreeMap<JointKey, AccumulatedHypothesis>,
    select_difficulty: Option<CurrentSelectionDifficulty>,
    select_play_types: BTreeMap<PlayType, u64>,
    select_play_sides: BTreeMap<PlaySide, u64>,
    result_chart_factors: BTreeMap<ResultChartFactor, u64>,
    has_result_evidence: bool,
    first_observation_ms: Option<u64>,
    last_observation_ms: Option<u64>,
    observation_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ResultChartFactor {
    play_type: Option<PlayType>,
    difficulty: Option<Difficulty>,
    notes: Option<u32>,
    level: Option<u8>,
}

#[derive(Clone, Debug)]
struct HypothesisSummary {
    state: ResolverResolutionState,
    select_play_type: Option<PlayType>,
    result_play_type: Option<PlayType>,
    selected: Option<JointEvidenceCandidate>,
    runner_up: Option<JointEvidenceCandidate>,
    runner_song: Option<JointEvidenceCandidate>,
    runner_chart: Option<JointEvidenceCandidate>,
    top_candidates: Vec<JointEvidenceCandidate>,
    support: u16,
    margin: u16,
    song_margin: u16,
    chart_margin: u16,
    selected_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
    runner_up_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
}

impl HypothesisSummary {
    fn accepted(&self) -> Option<JointEvidenceCandidate> {
        (self.state == ResolverResolutionState::AcceptedJoint)
            .then(|| self.selected.clone())
            .flatten()
    }
}

impl HypothesisAccumulator {
    fn observe_at(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        observation: &JointEvidenceObservation,
        select_difficulty: Option<Difficulty>,
        select_play_type: Option<PlayType>,
        result_chart_factor: Option<ResultChartFactor>,
    ) {
        self.first_observation_ms.get_or_insert(monotonic_ms);
        self.last_observation_ms = Some(monotonic_ms);
        self.observation_count = self.observation_count.saturating_add(1);
        for candidate in &observation.candidates {
            let key = JointKey {
                song_id: candidate.song_id,
                chart_key: candidate.chart.key,
            };
            let accumulated = self
                .candidates
                .entry(key)
                .or_insert_with(|| AccumulatedHypothesis {
                    candidate: candidate.clone(),
                    family_support: BTreeMap::new(),
                });
            for (family, delta) in &candidate.family_support {
                if matches!(
                    family,
                    EvidenceFamily::SelectChart | EvidenceFamily::ResultChart
                ) {
                    continue;
                }
                let value = accumulated.family_support.entry(*family).or_default();
                *value = value.saturating_add(u64::from(*delta));
            }
        }
        if let Some(difficulty) = select_difficulty {
            self.observe_select_difficulty(difficulty, sequence, monotonic_ms);
        }
        if let Some(play_type) = select_play_type {
            let count = self.select_play_types.entry(play_type).or_default();
            *count = count.saturating_add(1);
        }
        if let Some(factor) = result_chart_factor {
            let value = self.result_chart_factors.entry(factor).or_default();
            *value = value.saturating_add(1);
            self.has_result_evidence |= factor.play_type.is_some()
                || factor.difficulty.is_some()
                || factor.notes.is_some()
                || factor.level.is_some();
        }
        self.has_result_evidence |= observation.candidates.iter().any(|candidate| {
            candidate.family_support.iter().any(|(family, support)| {
                *support > 0
                    && matches!(
                        family,
                        EvidenceFamily::ResultTitle
                            | EvidenceFamily::ResultArtist
                            | EvidenceFamily::ResultChart
                            | EvidenceFamily::ResultPlayType
                    )
            })
        });
    }

    #[cfg(test)]
    fn observe(
        &mut self,
        monotonic_ms: u64,
        observation: &JointEvidenceObservation,
        select_difficulty: Option<Difficulty>,
        result_chart_factor: Option<ResultChartFactor>,
    ) {
        self.observe_at(
            monotonic_ms,
            monotonic_ms,
            observation,
            select_difficulty,
            None,
            result_chart_factor,
        );
    }

    fn add_from(&mut self, other: &Self) {
        for accumulated in other.candidates.values() {
            let key = JointKey {
                song_id: accumulated.candidate.song_id,
                chart_key: accumulated.candidate.chart.key,
            };
            let target = self
                .candidates
                .entry(key)
                .or_insert_with(|| AccumulatedHypothesis {
                    candidate: accumulated.candidate.clone(),
                    family_support: BTreeMap::new(),
                });
            for (family, support) in &accumulated.family_support {
                let value = target.family_support.entry(*family).or_default();
                *value = value.saturating_add(*support);
            }
        }
        self.adopt_newer_select_difficulty(other.select_difficulty);
        for (play_type, observations) in &other.select_play_types {
            let value = self.select_play_types.entry(*play_type).or_default();
            *value = value.saturating_add(*observations);
        }
        for (factor, observations) in &other.result_chart_factors {
            let value = self.result_chart_factors.entry(*factor).or_default();
            *value = value.saturating_add(*observations);
        }
        self.has_result_evidence |= other.has_result_evidence;
    }

    #[allow(
        clippy::too_many_lines,
        reason = "hierarchical projection keeps normalization and both runner dimensions together"
    )]
    fn summary(&self) -> HypothesisSummary {
        let mut expanded = self.candidates.clone();
        let select_play_type = self.resolved_select_play_type();
        let result_play_type = self.resolved_result_play_type();
        for accumulated in expanded.values_mut() {
            let select_chart = self.select_difficulty.map_or(0, |current| {
                u64::from(current.difficulty == accumulated.candidate.chart.key.difficulty)
                    * current.support()
            });
            if select_chart > 0 {
                accumulated
                    .family_support
                    .insert(EvidenceFamily::SelectChart, select_chart);
            }
            let result_chart = self
                .result_chart_factors
                .iter()
                .map(|(factor, observations)| {
                    let difficulty = u64::from(
                        factor.difficulty == Some(accumulated.candidate.chart.key.difficulty),
                    ) * 50;
                    let notes =
                        u64::from(factor.notes == Some(accumulated.candidate.chart.notes)) * 100;
                    let level =
                        u64::from(factor.level == Some(accumulated.candidate.chart.level)) * 10;
                    difficulty.max(notes).max(level) * observations
                })
                .sum();
            if result_chart > 0 {
                accumulated
                    .family_support
                    .insert(EvidenceFamily::ResultChart, result_chart);
            }
            if result_play_type == Some(accumulated.candidate.chart.key.play_type) {
                accumulated
                    .family_support
                    .insert(EvidenceFamily::ResultPlayType, 100);
            }
            if select_play_type == Some(accumulated.candidate.chart.key.play_type) {
                accumulated
                    .family_support
                    .insert(EvidenceFamily::SelectPlayType, 100);
            }
        }
        let mut family_maxima = BTreeMap::<EvidenceFamily, u64>::new();
        for candidate in expanded.values() {
            for (family, raw) in &candidate.family_support {
                let maximum = family_maxima.entry(*family).or_default();
                *maximum = (*maximum).max(*raw);
            }
        }
        let mut ranked = expanded
            .values()
            .map(|accumulated| {
                let family_support = accumulated
                    .family_support
                    .iter()
                    .map(|(family, raw)| {
                        let maximum = family_maxima[family];
                        let normalized = normalize_family_support(*raw, maximum);
                        (*family, EvidenceContribution::new(*raw, normalized))
                    })
                    .collect::<BTreeMap<_, _>>();
                let support = family_support.values().fold(0_u16, |total, value| {
                    total.saturating_add(value.normalized())
                });
                RankedHypothesis {
                    accumulated,
                    family_support,
                    support,
                }
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|left, right| {
            right
                .support
                .cmp(&left.support)
                .then_with(|| {
                    left.accumulated
                        .candidate
                        .song_id
                        .cmp(&right.accumulated.candidate.song_id)
                })
                .then_with(|| {
                    left.accumulated
                        .candidate
                        .chart
                        .key
                        .cmp(&right.accumulated.candidate.chart.key)
                })
        });
        let Some(selected) = ranked.first() else {
            return HypothesisSummary {
                state: ResolverResolutionState::Unresolved,
                select_play_type,
                result_play_type,
                selected: None,
                runner_up: None,
                runner_song: None,
                runner_chart: None,
                top_candidates: Vec::new(),
                support: 0,
                margin: 0,
                song_margin: 0,
                chart_margin: 0,
                selected_family_support: BTreeMap::new(),
                runner_up_family_support: BTreeMap::new(),
            };
        };
        let support = selected.support;
        let runner_up = ranked.get(1);
        let runner_support = runner_up.map_or(0, |candidate| candidate.support);
        let margin = support.saturating_sub(runner_support);
        let runner_song = ranked.iter().find(|candidate| {
            candidate.accumulated.candidate.song_id != selected.accumulated.candidate.song_id
        });
        let runner_chart = ranked.iter().find(|candidate| {
            candidate.accumulated.candidate.song_id == selected.accumulated.candidate.song_id
                && candidate.accumulated.candidate.chart.key
                    != selected.accumulated.candidate.chart.key
        });
        let song_margin = support.saturating_sub(runner_song.map_or(0, |value| value.support));
        let chart_margin = support.saturating_sub(runner_chart.map_or(0, |value| value.support));
        let song_is_resolved = runner_song.is_none() || song_margin >= JOINT_ACCEPT_MARGIN;
        let chart_is_resolved = select_play_type.is_some() || result_play_type.is_some();
        let chart_is_resolved =
            chart_is_resolved && (runner_chart.is_none() || chart_margin >= JOINT_ACCEPT_MARGIN);
        let state = if self.has_result_evidence
            && support >= JOINT_ACCEPT_SUPPORT
            && song_is_resolved
            && chart_is_resolved
        {
            ResolverResolutionState::AcceptedJoint
        } else if support >= JOINT_ACCEPT_SUPPORT && song_is_resolved {
            ResolverResolutionState::SongProjected
        } else if support >= JOINT_ACCEPT_SUPPORT {
            ResolverResolutionState::Conflict
        } else {
            ResolverResolutionState::JointCandidate
        };
        HypothesisSummary {
            state,
            select_play_type,
            result_play_type,
            selected: Some(selected.accumulated.candidate.clone()),
            runner_up: runner_up.map(|value| value.accumulated.candidate.clone()),
            runner_song: runner_song.map(|value| value.accumulated.candidate.clone()),
            runner_chart: runner_chart.map(|value| value.accumulated.candidate.clone()),
            top_candidates: ranked
                .iter()
                .take(3)
                .map(|value| value.accumulated.candidate.clone())
                .collect(),
            support,
            margin,
            song_margin,
            chart_margin,
            selected_family_support: selected.family_support.clone(),
            runner_up_family_support: runner_up
                .map_or_else(BTreeMap::new, |value| value.family_support.clone()),
        }
    }

    fn resolved_result_play_type(&self) -> Option<PlayType> {
        let mut observed = BTreeMap::<PlayType, u64>::new();
        for (factor, observations) in &self.result_chart_factors {
            if let Some(play_type) = factor.play_type {
                let count = observed.entry(play_type).or_default();
                *count = count.saturating_add(*observations);
            }
        }
        if observed.len() != 1 {
            return None;
        }
        observed
            .into_iter()
            .next()
            .and_then(|(play_type, observations)| (observations >= 2).then_some(play_type))
    }

    fn resolved_select_play_type(&self) -> Option<PlayType> {
        if self.select_play_types.len() != 1 {
            return None;
        }
        self.select_play_types
            .iter()
            .next()
            .and_then(|(play_type, observations)| (*observations >= 2).then_some(*play_type))
    }

    fn resolved_select_play_side(&self) -> Option<PlaySide> {
        if self.select_play_sides.len() != 1 {
            return None;
        }
        self.select_play_sides
            .iter()
            .next()
            .and_then(|(play_side, observations)| (*observations >= 2).then_some(*play_side))
    }

    fn observe_select_difficulty(
        &mut self,
        difficulty: Difficulty,
        sequence: u64,
        monotonic_ms: u64,
    ) -> bool {
        if let Some(current) = &mut self.select_difficulty {
            current.observe(difficulty, sequence, monotonic_ms)
        } else {
            self.select_difficulty = Some(CurrentSelectionDifficulty::observed(
                difficulty,
                sequence,
                monotonic_ms,
            ));
            true
        }
    }

    fn adopt_newer_select_difficulty(&mut self, incoming: Option<CurrentSelectionDifficulty>) {
        if incoming.is_some_and(|value| {
            self.select_difficulty
                .is_none_or(|current| value.last_sequence() > current.last_sequence())
        }) {
            self.select_difficulty = incoming;
        }
    }
}

fn normalize_family_support(raw: u64, maximum: u64) -> u16 {
    if maximum <= u64::from(EVIDENCE_FAMILY_CAP) {
        return u16::try_from(raw).unwrap_or(EVIDENCE_FAMILY_CAP);
    }
    let scaled = u128::from(raw) * u128::from(EVIDENCE_FAMILY_CAP) / u128::from(maximum);
    u16::try_from(scaled).unwrap_or(EVIDENCE_FAMILY_CAP)
}

fn credible_song_set(observation: &JointEvidenceObservation) -> BTreeSet<ScorepeekSongId> {
    let mut support_by_song = BTreeMap::<ScorepeekSongId, u16>::new();
    for candidate in &observation.candidates {
        let support = candidate
            .family_support
            .iter()
            .filter(|(family, _)| {
                matches!(
                    family,
                    EvidenceFamily::SelectTitle
                        | EvidenceFamily::SelectTitleLexical
                        | EvidenceFamily::SelectTitleStructural
                        | EvidenceFamily::SelectArtist
                )
            })
            .fold(0_u16, |total, (_, value)| total.saturating_add(*value));
        let current = support_by_song.entry(candidate.song_id).or_default();
        *current = (*current).max(support);
    }
    let maximum = support_by_song.values().copied().max().unwrap_or(0);
    if maximum == 0 {
        return BTreeSet::new();
    }
    let credible = support_by_song
        .iter()
        .filter_map(|(song, support)| (*support == maximum).then_some(*song))
        .collect::<BTreeSet<_>>();
    if observation.catalog_song_count > 0 && credible.len() == observation.catalog_song_count {
        BTreeSet::new()
    } else {
        credible
    }
}

/// Pure MUSIC SELECT epoch state. It owns evidence handoff, never attempt or output authority.
#[derive(Clone, Debug, Default)]
struct SelectionEpochTracker {
    incumbent: HypothesisAccumulator,
    successor: HypothesisAccumulator,
    incumbent_songs: BTreeSet<ScorepeekSongId>,
    successor_songs: BTreeSet<ScorepeekSongId>,
    pending_difficulty: Option<CurrentSelectionDifficulty>,
    play_types: BTreeMap<PlayType, u64>,
    play_sides: BTreeMap<PlaySide, u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectionDifficultyTransition {
    target: SelectionDifficultyTarget,
    reason: SelectionDifficultyTransitionReason,
    current: Option<CurrentSelectionDifficulty>,
}

impl SelectionEpochTracker {
    fn active_difficulty_state(
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

    fn observe_at_with_play_type(
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

    fn observe_selection_at(
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

    #[cfg(test)]
    fn observe_at(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        evidence: &JointEvidenceObservation,
        difficulty: Option<Difficulty>,
    ) -> Vec<SelectionDifficultyTransition> {
        self.observe_at_with_play_type(sequence, monotonic_ms, evidence, difficulty, None, None)
    }

    fn observe_difficulty_only(
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

    #[cfg(test)]
    fn observe(
        &mut self,
        monotonic_ms: u64,
        evidence: &JointEvidenceObservation,
        difficulty: Option<Difficulty>,
    ) -> Vec<SelectionDifficultyTransition> {
        self.observe_at(monotonic_ms, monotonic_ms, evidence, difficulty)
    }

    fn handoff(&self) -> HypothesisAccumulator {
        if self.successor.observation_count > 0 {
            self.successor.clone()
        } else {
            self.incumbent.clone()
        }
    }
}

#[derive(Default)]
struct MusicSelectResolver {
    best: MusicSelectResolverState,
    best_last_sequence: Option<u64>,
    best_minimum_sequence: u64,
    best_closed: bool,
    selection_epochs: SelectionEpochTracker,
}

impl MusicSelectResolver {
    fn observe(
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

    fn best_frame_identity(
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

    fn accepts_best_observation(&mut self, sequence: u64) -> bool {
        if self.best_closed
            || sequence < self.best_minimum_sequence
            || self.best_last_sequence.is_some_and(|last| sequence <= last)
        {
            return false;
        }
        self.best_last_sequence = Some(sequence);
        true
    }

    fn selected(&self) -> Option<MusicSelectionState> {
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

/// Pure ordered resolver state. Output sinks consume its typed state but do not own identity.
#[derive(Clone, Debug, Default)]
struct ResolverEngine {
    play_attempt: PlayAttemptReducer,
    selection_epochs: SelectionEpochTracker,
    retained_select: HypothesisAccumulator,
    result_hypotheses: HypothesisAccumulator,
    provisional_joint: Option<JointEvidenceCandidate>,
}

#[derive(Clone, Debug, Serialize)]
struct ResultHistoryEntry {
    ordinal: u64,
    session_id: String,
    capture_generation: u64,
    source_sequence: u64,
    song: Option<SongPresentation>,
    result: ResultDomainEvent,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunViewState {
    #[serde(skip)]
    public: event_api::PublicState,
    invocation_id: String,
    profile_sha256: String,
    recording: &'static str,
    watcher_state: String,
    scores_summary: Option<String>,
    channel_start_failure: Option<String>,
    session_count: u64,
    active_session_id: Option<String>,
    capture_generation: Option<u64>,
    current_screen: Option<String>,
    raw_screen: Option<String>,
    #[serde(skip)]
    latest_observation: Option<Value>,
    #[serde(skip)]
    latest_stabilized_result: Option<Value>,
    #[serde(skip)]
    latest_temporal_music_select: Option<Value>,
    #[serde(skip)]
    latest_play_attempt: Option<Value>,
    #[serde(skip)]
    latest_numeric_result: Option<Value>,
    latest_result_detected: Option<Value>,
    latest_provisional_result: Option<ResultHistoryEntry>,
    latest_result_label: Option<&'static str>,
    latest_music_selection: Option<MusicSelectionState>,
    music_select: MusicSelectResolverState,
    result_history: VecDeque<ResultHistoryEntry>,
    result_count: u64,
    #[serde(skip)]
    stable_result_song: Option<SongPresentation>,
    latest_report: Option<Value>,
    status_recording: &'static str,
    recording_memory_limit_bytes: u64,
    recording_memory_used_bytes: u64,
    recording_memory_high_water_bytes: u64,
    recording_dropped_frames: u64,
    next_channel_sequence: u64,
    #[serde(skip)]
    overlay_summary: String,
    message: String,
    resolver: ResolverDebugSnapshot,
}

#[derive(Clone, Debug, Default, Serialize)]
struct ResolverDebugSnapshot {
    now_ms: u64,
    raw_screen: Option<String>,
    screen: Option<String>,
    suspended: bool,
    finalizing: bool,
    screen_episode_id: u64,
    screen_episode_started_ms: Option<u64>,
    source_sequence: Option<u64>,
    latest_field_sequence: Option<u64>,
    latest_field_ms: Option<u64>,
    selection_difficulty_target: Option<SelectionDifficultyTarget>,
    selection_difficulty: Option<CurrentSelectionDifficulty>,
    local: Option<ResolverNodeSnapshot>,
    successor: Option<ResolverNodeSnapshot>,
    attempt: Option<AttemptNodeSnapshot>,
    gate: String,
    gates: Vec<GateSnapshot>,
    raw_fields: Vec<(String, String)>,
    play_options: Option<PlayOptionsDebugSnapshot>,
}

#[derive(Clone, Debug, Serialize)]
struct PlayOptionsDebugSnapshot {
    latest: PlayOptionsObservation,
    observations: u8,
    conflicting: bool,
    resolved: PlayOptions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum GateState {
    Accepted,
    Pending,
    Failed,
    Inactive,
}

#[derive(Clone, Debug, Serialize)]
struct GateSnapshot {
    label: &'static str,
    state: GateState,
}

#[derive(Clone, Debug, Serialize)]
struct ResolverNodeSnapshot {
    label: &'static str,
    started_ms: Option<u64>,
    last_observation_ms: Option<u64>,
    observations: u32,
    top: Option<String>,
    runner_up: Option<String>,
    runner_song: Option<String>,
    runner_chart: Option<String>,
    top_candidates: Vec<String>,
    support: u16,
    margin: u16,
    song_margin: u16,
    chart_margin: u16,
    select_play_type: Option<PlayType>,
    result_play_type: Option<PlayType>,
    play_type_mismatch: bool,
    family_contributions: Vec<String>,
    current_difficulty: Option<CurrentSelectionDifficulty>,
    state: ResolverResolutionState,
}

#[derive(Clone, Debug, Serialize)]
struct AttemptNodeSnapshot {
    attempt_id: Option<u64>,
    started_ms: Option<u64>,
    phase_started_ms: Option<u64>,
    phase: String,
    path: String,
    select_top: Option<String>,
    result_top: Option<String>,
    joint_top: Option<String>,
    support: u16,
    margin: u16,
    song_margin: u16,
    chart_margin: u16,
    runner_song: Option<String>,
    runner_chart: Option<String>,
    top_candidates: Vec<String>,
    family_contributions: Vec<String>,
    state: ResolverResolutionState,
}

impl RunViewState {
    fn new(invocation_id: String, profile_sha256: String, recording_enabled: bool) -> Self {
        let mut public = event_api::PublicState::new(invocation_id.clone());
        if recording_enabled {
            public.enable_recording();
        }
        Self {
            public,
            invocation_id,
            profile_sha256,
            recording: if recording_enabled {
                "enabled"
            } else {
                "disabled"
            },
            watcher_state: "starting".to_owned(),
            scores_summary: None,
            overlay_summary: String::new(),
            channel_start_failure: None,
            session_count: 0,
            active_session_id: None,
            capture_generation: None,
            current_screen: None,
            raw_screen: None,
            latest_observation: None,
            latest_stabilized_result: None,
            latest_temporal_music_select: None,
            latest_play_attempt: None,
            latest_numeric_result: None,
            latest_result_detected: None,
            latest_provisional_result: None,
            latest_result_label: None,
            latest_music_selection: None,
            music_select: MusicSelectResolverState::default(),
            result_history: VecDeque::with_capacity(RESULT_HISTORY_CAPACITY),
            result_count: 0,
            stable_result_song: None,
            latest_report: None,
            status_recording: if recording_enabled {
                "armed"
            } else {
                "disabled"
            },
            recording_memory_limit_bytes: 0,
            recording_memory_used_bytes: 0,
            recording_memory_high_water_bytes: 0,
            recording_dropped_frames: 0,
            next_channel_sequence: 1,
            message: "initializing".to_owned(),
            resolver: ResolverDebugSnapshot::default(),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn reduce(&mut self, event: &RunEvent, serialized: &Value) {
        match &event.kind {
            RunEventKind::MusicSelectResolverChanged { state, .. } => {
                self.music_select = state.clone();
            }
            RunEventKind::WatcherStarted { .. } => "starting".clone_into(&mut self.watcher_state),
            RunEventKind::SessionStarted {
                session_id,
                capture_generation,
                ..
            } => {
                "session_active".clone_into(&mut self.watcher_state);
                self.session_count = self.session_count.saturating_add(1);
                self.active_session_id.clone_from(session_id);
                self.capture_generation = Some(*capture_generation);
                self.current_screen = None;
                self.music_select = MusicSelectResolverState::default();
                self.raw_screen = None;
                self.latest_observation = None;
                self.latest_stabilized_result = None;
                self.latest_temporal_music_select = None;
                self.latest_play_attempt = None;
                self.latest_numeric_result = None;
                self.latest_provisional_result = None;
                self.latest_result_label = None;
                self.latest_music_selection = None;
                self.music_select = MusicSelectResolverState::default();
                self.stable_result_song = None;
                self.latest_report = None;
                if self.recording == "enabled" && self.status_recording != "degraded" {
                    self.status_recording = "armed";
                }
                "capture session admitted".clone_into(&mut self.message);
            }
            RunEventKind::RecordingHealthChanged {
                state,
                memory_limit_bytes,
                memory_used_bytes,
                memory_high_water_bytes,
                dropped_frames,
                ..
            } => {
                self.status_recording = match state.as_str() {
                    "active" => "active",
                    "pressured" => "pressured",
                    _ => "degraded",
                };
                self.recording_memory_limit_bytes = *memory_limit_bytes;
                self.recording_memory_used_bytes = *memory_used_bytes;
                self.recording_memory_high_water_bytes = *memory_high_water_bytes;
                self.recording_dropped_frames = *dropped_frames;
            }
            RunEventKind::RecordingFinalizing { .. } => {
                if self.status_recording != "degraded" {
                    self.status_recording = "finalizing";
                }
                "session recording finalizing".clone_into(&mut self.message);
            }
            RunEventKind::RecordingCompleted { session_id, .. } => {
                self.status_recording = "ready";
                self.message = format!("session recording ready: {session_id}");
            }
            RunEventKind::ScreenChanged { screen, .. } => {
                if screen == "result" {
                    self.latest_numeric_result = None;
                }
                self.current_screen = Some(screen.clone());
            }
            RunEventKind::RawScreenObserved { screen, .. } => {
                self.raw_screen = Some(screen.clone());
            }
            RunEventKind::SemanticScreenEpisodeChanged { screen, phase, .. } => match phase {
                SemanticEpisodePhase::Started | SemanticEpisodePhase::Resumed => {
                    self.current_screen = Some(screen.clone());
                }
                SemanticEpisodePhase::Finalized => self.current_screen = None,
                SemanticEpisodePhase::Suspended | SemanticEpisodePhase::Closing => {}
            },
            RunEventKind::GameVersionChanged { .. }
            | RunEventKind::OverlayObserved { .. }
            | RunEventKind::MusicSelectBestObserved { .. }
            | RunEventKind::ScreenTick { .. }
            | RunEventKind::ResolverStateChanged { .. }
            | RunEventKind::SelectionDifficultyChanged { .. }
            | RunEventKind::ResultPanelSideChanged { .. }
            | RunEventKind::ResultSelectContextMismatch { .. } => {}
            RunEventKind::MusicSelectionChanged { state, .. } => {
                self.latest_music_selection = Some(state.clone());
            }
            RunEventKind::FieldObservation { .. } => {
                self.latest_observation = Some(serialized.clone());
            }
            RunEventKind::TemporalResultChanged { stable_song, .. } => {
                self.latest_stabilized_result = Some(serialized.clone());
                self.stable_result_song.clone_from(stable_song);
            }
            RunEventKind::TemporalMusicSelectChanged { .. } => {
                self.latest_temporal_music_select = Some(serialized.clone());
            }
            RunEventKind::NumericResultChanged { .. } => {
                self.latest_numeric_result = Some(serialized.clone());
            }
            RunEventKind::PlayAttemptChanged { .. } => {
                self.latest_play_attempt = Some(serialized.clone());
            }
            RunEventKind::ResultChanged {
                session_id,
                capture_generation,
                source_sequence,
                state,
            } => match state {
                ResultState::Inactive => {
                    self.latest_provisional_result = None;
                    self.latest_result_label = Some("INACTIVE");
                }
                ResultState::Provisional { song, result } => {
                    self.latest_result_label = Some("PROVISIONAL");
                    self.latest_provisional_result = Some(ResultHistoryEntry {
                        ordinal: self.result_count.saturating_add(1),
                        session_id: session_id.clone(),
                        capture_generation: *capture_generation,
                        source_sequence: *source_sequence,
                        song: song
                            .as_ref()
                            .filter(|song| song.scorepeek_song_id == result.scorepeek_song_id)
                            .cloned(),
                        result: result.as_ref().clone(),
                    });
                }
                ResultState::Retracted { song, result, .. } => {
                    self.latest_result_label = Some("RETRACTED");
                    self.latest_provisional_result = Some(ResultHistoryEntry {
                        ordinal: self.result_count.saturating_add(1),
                        session_id: session_id.clone(),
                        capture_generation: *capture_generation,
                        source_sequence: *source_sequence,
                        song: song.clone(),
                        result: result.as_ref().clone(),
                    });
                }
                ResultState::Confirmed { song, result } => {
                    self.latest_provisional_result = None;
                    self.latest_result_label = None;
                    self.latest_result_detected = Some(serialized.clone());
                    self.result_count = self.result_count.saturating_add(1);
                    if self.result_history.len() == RESULT_HISTORY_CAPACITY {
                        self.result_history.pop_front();
                    }
                    self.result_history.push_back(ResultHistoryEntry {
                        ordinal: self.result_count,
                        session_id: session_id.clone(),
                        capture_generation: *capture_generation,
                        source_sequence: *source_sequence,
                        song: song.clone(),
                        result: result.as_ref().clone(),
                    });
                }
            },
            RunEventKind::SessionFinished {
                outcome, report, ..
            } => {
                "session_finished".clone_into(&mut self.watcher_state);
                self.active_session_id = None;
                self.capture_generation = None;
                self.current_screen = None;
                self.music_select = MusicSelectResolverState::default();
                self.raw_screen = None;
                self.latest_report = Some(report.clone());
                self.message = format!("session finished: {outcome}");
                if self.recording != "disabled" && self.status_recording != "degraded" {
                    self.status_recording = "finalizing";
                }
            }
            RunEventKind::WatcherStopped { .. } => {
                "stopped".clone_into(&mut self.watcher_state);
                self.active_session_id = None;
                self.capture_generation = None;
                self.current_screen = None;
                self.music_select = MusicSelectResolverState::default();
                self.raw_screen = None;
                self.latest_observation = None;
                self.latest_stabilized_result = None;
                self.latest_temporal_music_select = None;
                self.latest_play_attempt = None;
                self.latest_numeric_result = None;
                self.stable_result_song = None;
                "scorepeek stopped by signal".clone_into(&mut self.message);
            }
        }
    }
}

#[derive(Default)]
struct ChannelHealth {
    connected_clients: AtomicUsize,
    dropped_events: AtomicU64,
    disconnected_clients: AtomicU64,
    server_failed: AtomicBool,
    oversized_records: AtomicU64,
}

impl ChannelHealth {
    fn value(&self) -> Value {
        json!({
            "status": if self.server_failed.load(Ordering::Acquire) { "degraded" } else { "ready" },
            "connected_clients": self.connected_clients.load(Ordering::Acquire),
            "dropped_events": self.dropped_events.load(Ordering::Acquire),
            "oversized_records": self.oversized_records.load(Ordering::Acquire),
            "error_type": if self.oversized_records.load(Ordering::Acquire) > 0 { Some("record_too_large") } else if self.server_failed.load(Ordering::Acquire) { Some("worker_unavailable") } else if self.dropped_events.load(Ordering::Acquire) > 0 { Some("queue_overflow") } else { None },
            "disconnected_clients": self.disconnected_clients.load(Ordering::Acquire),
        })
    }
}

struct EventChannel {
    sender: SyncSender<QueuedEvent>,
    stop: Arc<AtomicBool>,
    health: Arc<ChannelHealth>,
    thread: Option<JoinHandle<()>>,
    socket_path: PathBuf,
    socket_identity: (u64, u64),
}

struct SocketPathGuard {
    path: PathBuf,
    identity: (u64, u64),
    armed: bool,
}

impl SocketPathGuard {
    fn new(path: PathBuf, identity: (u64, u64)) -> Self {
        Self {
            path,
            identity,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for SocketPathGuard {
    fn drop(&mut self) {
        if self.armed {
            remove_owned_socket(&self.path, self.identity);
        }
    }
}

impl EventChannel {
    fn start(state: Arc<Mutex<RunViewState>>) -> Result<Self, String> {
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && !path.as_os_str().is_empty())
            .ok_or_else(|| {
                "XDG_RUNTIME_DIR must be absolute and non-empty for scorepeek run".to_owned()
            })?;
        Self::start_at(&runtime, state)
    }

    fn start_at(runtime: &Path, state: Arc<Mutex<RunViewState>>) -> Result<Self, String> {
        let directory = runtime.join("scorepeek");
        ensure_private_directory(&directory)?;
        let socket_path = directory.join(SOCKET_NAME);
        remove_stale_socket(&socket_path)?;
        let listener = UnixListener::bind(&socket_path)
            .map_err(|error| format!("event socket could not be bound: {error}"))?;
        let metadata = socket_path
            .symlink_metadata()
            .map_err(|error| format!("event socket could not be inspected: {error}"))?;
        let socket_identity = (metadata.dev(), metadata.ino());
        let mut path_guard = SocketPathGuard::new(socket_path.clone(), socket_identity);
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("event socket permissions could not be set: {error}"))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("event socket could not be made nonblocking: {error}"))?;
        let (sender, receiver) = std::sync::mpsc::sync_channel::<QueuedEvent>(EVENT_QUEUE_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let health = Arc::new(ChannelHealth::default());
        let thread_stop = Arc::clone(&stop);
        let thread_health = Arc::clone(&health);
        let thread = thread::Builder::new()
            .name("scorepeek-event-socket".to_owned())
            .spawn(move || {
                let mut clients = Vec::new();
                loop {
                    if thread_health.server_failed.load(Ordering::Acquire) {
                        break;
                    }
                    prune_clients(&thread_health, &mut clients);
                    accept_clients(&listener, &state, &thread_health, &mut clients);
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(record) => broadcast(&record, &thread_health, &mut clients),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if thread_stop.load(Ordering::Acquire) {
                                break;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                retain_clients(&thread_health, &mut clients, |_| false);
            })
            .map_err(|error| format!("event socket worker could not start: {error}"))?;
        let channel = Self {
            sender,
            stop,
            health,
            thread: Some(thread),
            socket_path,
            socket_identity,
        };
        path_guard.disarm();
        Ok(channel)
    }

    fn publish(&self, event: QueuedEvent) -> &'static str {
        try_send_event(&self.sender, &self.health, event)
    }
}

struct QueuedEvent {
    sequence: u64,
    bytes: Vec<u8>,
}

fn try_send_event(
    sender: &SyncSender<QueuedEvent>,
    health: &ChannelHealth,
    event: QueuedEvent,
) -> &'static str {
    match sender.try_send(event) {
        Ok(()) => "enqueued",
        Err(TrySendError::Full(_)) => {
            health.dropped_events.fetch_add(1, Ordering::AcqRel);
            "queue_full"
        }
        Err(TrySendError::Disconnected(_)) => {
            health.server_failed.store(true, Ordering::Release);
            "worker_unavailable"
        }
    }
}

impl Drop for EventChannel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            self.health.server_failed.store(true, Ordering::Release);
        }
        remove_owned_socket(&self.socket_path, self.socket_identity);
    }
}

fn remove_owned_socket(path: &Path, identity: (u64, u64)) {
    if let Ok(metadata) = path.symlink_metadata()
        && metadata.file_type().is_socket()
        && (metadata.dev(), metadata.ino()) == identity
    {
        let _ = fs::remove_file(path);
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err("event socket directory is not a directory".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(path)
                .map_err(|error| format!("event socket directory could not be created: {error}"))
        }
        Err(error) => Err(format!(
            "event socket directory could not be inspected: {error}"
        )),
    }
}

fn remove_stale_socket(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_socket() => match UnixStream::connect(path) {
            Ok(_) => Err("event socket is already active".to_owned()),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path)
                .map_err(|error| format!("stale event socket could not be removed: {error}")),
            Err(error) => Err(format!(
                "event socket liveness could not be determined: {error}"
            )),
        },
        Ok(_) => Err("event socket path contains a non-socket entry".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("event socket path could not be inspected: {error}")),
    }
}

struct EventClient {
    stream: UnixStream,
    next_sequence: u64,
    epoch: u64,
}

fn prune_clients(health: &ChannelHealth, clients: &mut Vec<EventClient>) {
    let epoch = health.dropped_events.load(Ordering::Acquire);
    retain_clients(health, clients, |client| {
        // On Linux AF_UNIX, a zero-byte send detects a closed peer without consuming requests
        // or mistaking a client's write-half shutdown for loss of its receiving side.
        client.epoch == epoch
            && match client.stream.write(&[]) {
                Ok(_) => true,
                Err(error) => matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ),
            }
    });
}

fn retain_clients(
    health: &ChannelHealth,
    clients: &mut Vec<EventClient>,
    mut keep: impl FnMut(&mut EventClient) -> bool,
) {
    let before = clients.len();
    clients.retain_mut(|client| keep(client));
    health
        .disconnected_clients
        .fetch_add((before - clients.len()) as u64, Ordering::AcqRel);
    health
        .connected_clients
        .store(clients.len(), Ordering::Release);
}

fn accept_clients(
    listener: &UnixListener,
    state: &Arc<Mutex<RunViewState>>,
    health: &ChannelHealth,
    clients: &mut Vec<EventClient>,
) {
    // Bound admission work as well as established clients so a reconnect loop cannot starve delivery.
    for _ in 0..MAX_CLIENTS {
        match listener.accept() {
            Ok((mut stream, _)) => {
                prune_clients(health, clients);
                if clients.len() >= MAX_CLIENTS || stream.set_nonblocking(true).is_err() {
                    health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
                let Ok(state) = state.lock() else {
                    health.server_failed.store(true, Ordering::Release);
                    return;
                };
                if health.server_failed.load(Ordering::Acquire) {
                    return;
                }
                let Ok(bytes) = event_api::encode(&state.public) else {
                    health.oversized_records.fetch_add(1, Ordering::AcqRel);
                    health.server_failed.store(true, Ordering::Release);
                    return;
                };
                let next_sequence = state.public.next_sequence;
                let epoch = health.dropped_events.load(Ordering::Acquire);
                if stream.write_all(&bytes).is_err() {
                    health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
                clients.push(EventClient {
                    stream,
                    next_sequence,
                    epoch,
                });
                health
                    .connected_clients
                    .store(clients.len(), Ordering::Release);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => {
                health.server_failed.store(true, Ordering::Release);
                break;
            }
        }
    }
}

#[cfg(test)]
fn snapshot_bytes(state: &Arc<Mutex<RunViewState>>, _health: &ChannelHealth) -> Option<Vec<u8>> {
    event_api::encode(&state.lock().ok()?.public).ok()
}

fn broadcast(record: &QueuedEvent, health: &ChannelHealth, clients: &mut Vec<EventClient>) {
    let epoch = health.dropped_events.load(Ordering::Acquire);
    retain_clients(health, clients, |client| {
        if client.epoch != epoch {
            return false;
        }
        if record.sequence < client.next_sequence {
            return true;
        }
        if record.sequence != client.next_sequence {
            return false;
        }
        if client.stream.write_all(&record.bytes).is_err() {
            return false;
        }
        client.next_sequence += 1;
        true
    });
}

fn commit_public_projection(
    current: &mut event_api::PublicState,
    projected: event_api::PublicState,
    events: &[event_api::PublicRecord],
    channel: Option<&EventChannel>,
    scores: Option<&crate::scores::Worker>,
) -> Vec<Value> {
    if events.is_empty() {
        return Vec::new();
    }
    *current = projected;
    let records = events
        .iter()
        .map(event_api::encode)
        .collect::<Result<Vec<_>, _>>();
    if let Some(scores) = scores {
        match &records {
            Ok(records) => {
                for bytes in records {
                    scores.offer(bytes);
                }
            }
            Err(error) => scores.reject("event_encoding", error),
        }
    }
    let mut observations = Vec::with_capacity(events.len());
    let Some(channel) = channel else {
        for event in events {
            observations.push(json!({"public_event":event, "public_event_sequence":event.sequence, "enqueue":"channel_unavailable"}));
        }
        return observations;
    };
    if channel.health.server_failed.load(Ordering::Acquire) {
        for event in events {
            observations.push(json!({"public_event":event, "public_event_sequence":event.sequence, "enqueue":"worker_unavailable"}));
        }
        return observations;
    }
    if let Ok(records) = records.and_then(|records| event_api::encode(current).map(|_| records)) {
        for (event, bytes) in events.iter().zip(records) {
            let enqueue = channel.publish(QueuedEvent {
                sequence: event.sequence,
                bytes,
            });
            observations.push(json!({"public_event":event, "public_event_sequence":event.sequence, "enqueue":enqueue}));
        }
    } else {
        channel
            .health
            .oversized_records
            .fetch_add(1, Ordering::AcqRel);
        channel.health.server_failed.store(true, Ordering::Release);
        for event in events {
            observations.push(json!({"public_event":event, "public_event_sequence":event.sequence, "enqueue":"encoding_failed"}));
        }
    }
    observations
}

#[allow(clippy::struct_excessive_bools)]
pub struct RoutineOutput {
    state: Arc<Mutex<RunViewState>>,
    channel: Option<EventChannel>,
    scores: Option<crate::scores::Worker>,
    publish_frontend_snapshots: bool,
    next_sequence: u64,
    engine: ResolverEngine,
    pending_numeric_result: Option<PendingNumericResult>,
    pending_supplemental_result: Option<PendingSupplementalResult>,
    accepted_numeric_result: Option<NumericResultView>,
    active_provisional_result: Option<ActiveProvisionalResult>,
    music_selection_revision: u64,
    music_select_resolver: MusicSelectResolver,
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
    timing_active: bool,
    output_us: u64,
    #[cfg(test)]
    headless_events: Vec<RunEvent>,
    diagnostics: Option<RunDiagnostics>,
}

pub struct RoutineOutputStartError {
    message: String,
    diagnostics: RunDiagnostics,
}

impl RoutineOutputStartError {
    #[must_use]
    pub fn into_parts(self) -> (String, RunDiagnostics) {
        (self.message, self.diagnostics)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(
    clippy::struct_field_names,
    reason = "the unit suffix is part of the explicit frame-timing contract"
)]
pub struct RoutineEventProcessingTiming {
    pub screen_resolver_us: Option<u64>,
    pub attempt_resolver_us: Option<u64>,
    pub output_us: Option<u64>,
}

impl RoutineOutput {
    pub fn refresh_overlays(
        &mut self,
        children: &mut crate::overlay::supervisor::Children,
        controller: Option<&crate::config::control::Controller>,
    ) -> Result<(), String> {
        for message in children.poll() {
            self.warning(message)?;
        }
        self.state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .overlay_summary = children.summary();
        for observation in children.take_observations() {
            self.publish_overlay_observation(observation)?;
        }
        if let Some(controller) = controller {
            for record in controller.take_observations() {
                self.publish_overlay_observation(serde_json::json!({
                    "source":"controller", "record":record
                }))?;
            }
        }
        Ok(())
    }

    fn publish_overlay_observation(&mut self, observation: Value) -> Result<(), String> {
        self.publish_one_inner(
            &RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::OverlayObserved { observation },
            },
            false,
        )
    }
    #[must_use]
    pub fn event_socket_path(&self) -> Option<&Path> {
        self.channel
            .as_ref()
            .map(|channel| channel.socket_path.as_path())
    }

    pub fn diagnostic_run_root(&self) -> Option<&Path> {
        self.diagnostics.as_ref().and_then(RunDiagnostics::run_root)
    }

    pub fn finish_diagnostics(&mut self, operation_status: &str) {
        if let Some(diagnostics) = &mut self.diagnostics {
            diagnostics.finish(operation_status);
        }
    }

    pub fn record_diagnostic(&self, resource: &str, detail: &Value, critical: bool) {
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.sink().record(resource, detail, critical);
        }
    }

    fn publish_resolver_transition(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        source_sequence: u64,
        scope: ResolverScope,
        summary: &HypothesisSummary,
        observation_count: u32,
    ) -> Result<(), String> {
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
        self.publish_one(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::ResolverStateChanged {
                session_id: session_id.cloned(),
                capture_generation,
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

    /// Starts the public run output while retaining diagnostic ownership on failure.
    ///
    /// # Panics
    /// Panics only if the internal constructor loses diagnostics that this entry point supplied.
    pub fn start(
        invocation_id: String,
        profile_sha256: String,
        recording_enabled: bool,
        diagnostics: RunDiagnostics,
    ) -> Result<Self, RoutineOutputStartError> {
        let state = Arc::new(Mutex::new(RunViewState::new(
            invocation_id,
            profile_sha256,
            recording_enabled,
        )));
        let channel = EventChannel::start(Arc::clone(&state));
        Self::from_channel(state, channel, true, Some(diagnostics)).map_err(
            |(message, diagnostics)| RoutineOutputStartError {
                message,
                diagnostics: diagnostics.expect("start supplied diagnostic ownership"),
            },
        )
    }

    fn from_channel(
        state: Arc<Mutex<RunViewState>>,
        channel: Result<EventChannel, String>,
        publish_frontend_snapshots: bool,
        diagnostics: Option<RunDiagnostics>,
    ) -> Result<Self, (String, Option<RunDiagnostics>)> {
        let channel = match channel {
            Ok(channel) => Some(channel),
            Err(error) => {
                let Ok(mut state) = state.lock() else {
                    return Err(("run view state lock was poisoned".to_owned(), diagnostics));
                };
                state.channel_start_failure = Some(error);
                None
            }
        };
        let mut output = Self {
            state,
            channel,
            scores: None,
            publish_frontend_snapshots,
            next_sequence: 1,
            engine: ResolverEngine::default(),
            resolver_transitions: BTreeMap::new(),
            pending_numeric_result: None,
            pending_supplemental_result: None,
            accepted_numeric_result: None,
            active_provisional_result: None,
            music_selection_revision: 0,
            music_select_resolver: MusicSelectResolver::default(),
            active_music_selection: None,
            music_selection_episode_active: false,
            play_options: PlayOptionsEpisodeAccumulator::default(),
            result_panel_side: ResultPanelSideAccumulator::default(),
            result_select_context_detached: false,
            numeric_evidence: VecDeque::with_capacity(8),
            last_numeric_sequence: None,
            last_numeric_monotonic_ms: None,
            emitted_attempt_ids: BTreeSet::new(),
            latest_screen_boundary_sequence: None,
            screen_episode_id: 0,
            screen_episode_started_ms: None,
            screen_episode_last_ms: None,
            result_resolver_active: false,
            result_episode_finalizing: false,
            semantic_episode_suspended: false,
            attempt_started_ms: None,
            attempt_phase_started_ms: None,
            timing_active: false,
            output_us: 0,
            #[cfg(test)]
            headless_events: Vec::new(),
            diagnostics,
        };
        match output.refresh() {
            Ok(()) => Ok(output),
            Err(message) => {
                let diagnostics = output.diagnostics.take();
                Err((message, diagnostics))
            }
        }
    }

    #[must_use]
    #[cfg(test)]
    pub fn start_headless(invocation_id: String, profile_sha256: String) -> Self {
        Self::start_headless_inner(invocation_id, profile_sha256, None)
    }

    #[must_use]
    #[allow(
        dead_code,
        reason = "the library entry point is consumed by corpus replay"
    )]
    pub fn start_headless_with_diagnostics(
        invocation_id: String,
        profile_sha256: String,
        diagnostics: RunDiagnostics,
    ) -> Self {
        Self::start_headless_inner(invocation_id, profile_sha256, Some(diagnostics))
    }

    #[allow(
        dead_code,
        reason = "the library entry point is consumed by corpus replay"
    )]
    fn start_headless_inner(
        invocation_id: String,
        profile_sha256: String,
        diagnostics: Option<RunDiagnostics>,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(RunViewState::new(
                invocation_id,
                profile_sha256,
                false,
            ))),
            channel: None,
            scores: None,
            publish_frontend_snapshots: false,
            next_sequence: 1,
            engine: ResolverEngine::default(),
            resolver_transitions: BTreeMap::new(),
            pending_numeric_result: None,
            pending_supplemental_result: None,
            accepted_numeric_result: None,
            active_provisional_result: None,
            music_selection_revision: 0,
            music_select_resolver: MusicSelectResolver::default(),
            active_music_selection: None,
            music_selection_episode_active: false,
            play_options: PlayOptionsEpisodeAccumulator::default(),
            result_panel_side: ResultPanelSideAccumulator::default(),
            result_select_context_detached: false,
            numeric_evidence: VecDeque::with_capacity(8),
            last_numeric_sequence: None,
            last_numeric_monotonic_ms: None,
            emitted_attempt_ids: BTreeSet::new(),
            latest_screen_boundary_sequence: None,
            screen_episode_id: 0,
            screen_episode_started_ms: None,
            screen_episode_last_ms: None,
            result_resolver_active: false,
            result_episode_finalizing: false,
            semantic_episode_suspended: false,
            attempt_started_ms: None,
            attempt_phase_started_ms: None,
            timing_active: false,
            output_us: 0,
            #[cfg(test)]
            headless_events: Vec::new(),
            diagnostics,
        }
    }

    #[cfg(test)]
    pub fn take_headless_events(&mut self) -> Vec<RunEvent> {
        std::mem::take(&mut self.headless_events)
    }

    pub fn refresh_scores(&mut self) -> Result<(), String> {
        let diagnostic_sink = self.diagnostics.as_ref().map(RunDiagnostics::sink);
        let completions = self
            .scores
            .as_ref()
            .map(crate::scores::Worker::take_completions)
            .unwrap_or_default();
        for completion in completions {
            let persisted = completion.outcome == crate::scores::CompletionOutcome::Persisted;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            let mut projected = state.public.clone();
            let mut events = Vec::new();
            if persisted && let Some(chart) = completion.chart {
                events.push(projected.score_store_changed(chart));
            }
            let observations = commit_public_projection(
                &mut state.public,
                projected,
                &events,
                self.channel.as_ref(),
                None,
            );
            if let Some(sink) = &diagnostic_sink {
                for observation in observations {
                    sink.record("public_event", &observation, false);
                }
            }
        }
        let persistence_failed = self
            .scores_health()
            .is_some_and(|health| health.failure.is_some());
        let mut state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?;
        let mut projected = state.public.clone();
        let health_events = projected
            .scores_health(!persistence_failed)
            .into_iter()
            .collect::<Vec<_>>();
        let observations = commit_public_projection(
            &mut state.public,
            projected,
            &health_events,
            self.channel.as_ref(),
            None,
        );
        drop(state);
        if let Some(sink) = &diagnostic_sink {
            for observation in observations {
                sink.record("public_event", &observation, false);
            }
        }
        self.refresh()
    }

    pub fn enable_scores(&mut self, path: &Path) -> Result<(), String> {
        self.scores = Some(crate::scores::Worker::start(path));
        let mut state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?;
        state.scores_summary = Some(format!("{}", path.display()));
        state.public.enable_scores();
        drop(state);
        self.refresh()
    }

    pub fn scores_health(&self) -> Option<crate::scores::Health> {
        self.scores.as_ref().map(crate::scores::Worker::health)
    }

    pub fn bind_public_session(&mut self, binding: event_api::Binding) {
        if let Ok(mut state) = self.state.lock() {
            state.public.pending_binding = Some(binding);
        }
    }

    pub fn publish(&mut self, event: &RunEvent) -> Result<(), String> {
        self.publish_timed(event).map(|_| ())
    }

    pub fn publish_timed(
        &mut self,
        event: &RunEvent,
    ) -> Result<RoutineEventProcessingTiming, String> {
        self.timing_active = true;
        self.output_us = 0;
        let started = Instant::now();
        let result = self.publish_internal(event);
        let total_us = duration_us(started.elapsed());
        self.timing_active = false;
        let output_us = self.output_us.min(total_us);
        let resolver_us = total_us.saturating_sub(output_us);
        result?;
        let (screen_resolver_us, attempt_resolver_us) = match &event.kind {
            RunEventKind::RawScreenObserved { .. }
            | RunEventKind::SemanticScreenEpisodeChanged { .. }
            | RunEventKind::ScreenChanged { .. }
            | RunEventKind::ScreenTick { .. } => (Some(resolver_us), None),
            RunEventKind::FieldObservation { .. } => (None, Some(resolver_us)),
            _ => (None, None),
        };
        Ok(RoutineEventProcessingTiming {
            screen_resolver_us,
            attempt_resolver_us,
            output_us: Some(output_us),
        })
    }

    fn publish_internal(&mut self, event: &RunEvent) -> Result<(), String> {
        match &event.kind {
            RunEventKind::SessionStarted {
                session_id,
                capture_generation,
                ..
            } => {
                self.engine.play_attempt.reset_session();
                self.reset_numeric_result();
                self.active_provisional_result = None;
                self.music_selection_revision = 0;
                self.music_select_resolver = MusicSelectResolver::default();
                self.active_music_selection = None;
                self.music_selection_episode_active = false;
                self.emitted_attempt_ids.clear();
                self.latest_screen_boundary_sequence = None;
                self.screen_episode_id = 0;
                self.screen_episode_started_ms = None;
                self.screen_episode_last_ms = None;
                self.engine.selection_epochs = SelectionEpochTracker::default();
                self.engine.retained_select = HypothesisAccumulator::default();
                self.engine.result_hypotheses = HypothesisAccumulator::default();
                self.engine.provisional_joint = None;
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
                self.clear_resolver_field_observation()?;
                self.publish_one(event)?;
                if let Some(session_id) = session_id.clone() {
                    self.publish_result_state(
                        session_id,
                        *capture_generation,
                        0,
                        ResultState::Inactive,
                    )?;
                }
                Ok(())
            }
            RunEventKind::WatcherStopped { .. } => self.publish_watcher_stopped(event),
            RunEventKind::FieldObservation { .. } => self.publish_field_observation(event),
            RunEventKind::RawScreenObserved {
                session_id,
                capture_generation,
                semantic_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
                result_presence,
                ..
            } => {
                self.publish_one(event)?;
                if screen == "result"
                    && let (Some(episode_id), Some(side)) =
                        (*semantic_episode_id, result_presence.panel_side.known())
                {
                    self.observe_result_panel_side(
                        session_id.as_ref(),
                        *capture_generation,
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
            RunEventKind::SessionFinished { .. } => self.publish_session_finished(event),
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
        }
    }

    fn observe_result_panel_side(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        episode_id: u64,
        sequence: u64,
        side: ResultPanelSide,
    ) -> Result<(), String> {
        let Some((state, reason)) = self.result_panel_side.observe(episode_id, sequence, side)
        else {
            return Ok(());
        };
        self.publish_one(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::ResultPanelSideChanged {
                session_id: session_id.cloned(),
                capture_generation,
                screen_episode_id: episode_id,
                source_sequence: sequence,
                state,
                reason,
            },
        })?;
        if reason == ResultPanelSideTransitionReason::Conflict
            && let (Some(session_id), Some(capture_generation)) =
                (session_id.cloned(), capture_generation)
        {
            self.withdraw_result_provisional(
                session_id,
                capture_generation,
                sequence,
                ResultRetractionReason::PanelSideConflict,
            )?;
        }
        Ok(())
    }

    fn publish_semantic_screen_episode(&mut self, event: &RunEvent) -> Result<(), String> {
        let RunEventKind::SemanticScreenEpisodeChanged {
            session_id,
            capture_generation,
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
                self.clear_resolver_field_observation()?;
                let screen_change = RunEvent {
                    schema: event.schema.clone(),
                    kind: RunEventKind::ScreenChanged {
                        session_id: session_id.clone(),
                        capture_generation: *capture_generation,
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
                self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), None)?;
                self.refresh()
            }
            SemanticEpisodePhase::Resumed => {
                self.semantic_episode_suspended = false;
                self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), None)?;
                self.refresh()
            }
            SemanticEpisodePhase::Closing => {
                self.result_episode_finalizing = screen == "result";
                self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), None)?;
                self.refresh()
            }
            SemanticEpisodePhase::Finalized => {
                if screen == "music_select" {
                    self.engine.retained_select = self.engine.selection_epochs.handoff();
                    self.publish_music_selection(
                        session_id.as_ref(),
                        *capture_generation,
                        *sequence,
                        MusicSelectionState::Unresolved {
                            reason: MusicSelectionUnresolvedReason::EpisodeEnded,
                        },
                    )?;
                    self.music_selection_episode_active = false;
                } else if screen == "result" {
                    self.finalize_result_attempt(
                        session_id.clone(),
                        *capture_generation,
                        *sequence,
                    )?;
                    self.result_panel_side.clear();
                    self.result_select_context_detached = false;
                }
                self.result_episode_finalizing = false;
                self.semantic_episode_suspended = false;
                self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), None)?;
                self.refresh()
            }
        }
    }

    fn finalize_result_attempt(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        sequence: u64,
    ) -> Result<(), String> {
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
            self.publish_play_attempt_update(
                session_id.clone(),
                capture_generation,
                Some(sequence),
                state,
            )?;
        }
        self.try_emit_result(session_id.clone(), capture_generation, sequence)?;
        if self.engine.play_attempt.accepted_result().is_none()
            && let (Some(session_id), Some(capture_generation)) = (session_id, capture_generation)
        {
            self.withdraw_result_provisional(
                session_id,
                capture_generation,
                sequence,
                ResultRetractionReason::AttemptRejected,
            )?;
        }
        Ok(())
    }

    fn publish_screen_tick(&mut self, sequence: u64, monotonic_end_ms: u64) -> Result<(), String> {
        let previous_second = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .resolver
            .now_ms
            / 1_000;
        self.sync_resolver_snapshot(monotonic_end_ms, Some(sequence), None)?;
        if monotonic_end_ms / 1_000 != previous_second {
            self.refresh()?;
        }
        Ok(())
    }

    fn publish_watcher_stopped(&mut self, event: &RunEvent) -> Result<(), String> {
        let (session_id, capture_generation) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            (state.active_session_id.clone(), state.capture_generation)
        };
        if let Some(state) = self.engine.play_attempt.finish_session() {
            self.publish_play_attempt_update(session_id.clone(), capture_generation, None, state)?;
        }
        if let Some(scores) = &mut self.scores {
            scores.finish();
        }
        self.refresh_scores()?;
        self.publish_one(event)?;
        Ok(())
    }

    fn publish_field_observation(&mut self, event: &RunEvent) -> Result<(), String> {
        let RunEventKind::FieldObservation {
            session_id,
            capture_generation,
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            fields,
            parsed_result_fields,
            joint_evidence,
            song_resolution_presentation,
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
                    *capture_generation,
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
                *capture_generation,
                *sequence,
                *monotonic_end_ms,
                fields,
                parsed_result_fields.as_ref(),
                joint_evidence,
                song_resolution_presentation,
            ),
            "music_select" => self.reduce_music_select_observation(
                session_id.as_ref(),
                *capture_generation,
                *sequence,
                *monotonic_end_ms,
                fields,
                joint_evidence,
                song_resolution_presentation,
            ),
            _ => Ok(()),
        }
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the reducer keeps ordered temporal, attempt, and domain emission in one path"
    )]
    fn reduce_result_observation(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_end_ms: u64,
        fields: &Value,
        parsed_result_fields: Option<&ParsedResultFields>,
        joint_evidence: &JointEvidenceObservation,
        _song_resolution_presentation: &SongResolutionPresentation,
    ) -> Result<(), String> {
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
                schema: RUN_EVENT_SCHEMA.to_owned(),
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
                        schema: RUN_EVENT_SCHEMA.to_owned(),
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
        self.refresh()?;
        Ok(())
    }

    fn observe_numeric_result(
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
        let view = NumericResultView {
            song_id: candidate.song_id,
            clear_type,
            chart: candidate.chart.clone(),
            current_score,
            performance,
            source_sequence: sequence,
        };
        self.stabilize_numeric_result(view, chronology_reset)
    }

    fn stabilize_numeric_result(
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

    fn stabilize_supplemental_result(
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

    fn numeric_event_suppression_reason(
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
        let Some(accepted_attempt) = self.engine.play_attempt.accepted_result() else {
            return Some(NumericResultEventSuppressionReason::PlayAttemptNotAccepted);
        };
        let _ = accepted_attempt;
        (!self
            .engine
            .provisional_joint
            .as_ref()
            .is_some_and(|candidate| joint_matches_numeric(candidate, numeric)))
        .then_some(NumericResultEventSuppressionReason::LinkageConflict)
    }

    fn try_emit_result(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        fallback_sequence: u64,
    ) -> Result<(), String> {
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
            self.emitted_attempt_ids.insert(accepted_attempt.attempt_id);
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

    fn sync_result_provisional(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        fallback_sequence: u64,
    ) -> Result<(), String> {
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

    fn holds_stable_numeric_result(&self) -> bool {
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

    fn withdraw_result_provisional(
        &mut self,
        session_id: String,
        capture_generation: u64,
        source_sequence: u64,
        reason: ResultRetractionReason,
    ) -> Result<(), String> {
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

    fn publish_result_state(
        &mut self,
        session_id: String,
        capture_generation: u64,
        source_sequence: u64,
        state: ResultState,
    ) -> Result<(), String> {
        self.publish_one(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::ResultChanged {
                session_id,
                capture_generation,
                source_sequence,
                state,
            },
        })
    }

    fn reset_numeric_result(&mut self) {
        self.pending_numeric_result = None;
        self.pending_supplemental_result = None;
        self.accepted_numeric_result = None;
        self.last_numeric_sequence = None;
        self.last_numeric_monotonic_ms = None;
    }

    fn sync_music_selection(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        source_sequence: u64,
    ) -> Result<(), String> {
        match self.music_select_resolver.selected() {
            Some(state) if self.active_music_selection.as_ref() != Some(&state) => {
                self.publish_music_selection(session_id, capture_generation, source_sequence, state)
            }
            None if matches!(
                self.active_music_selection,
                Some(MusicSelectionState::Selected { .. })
            ) =>
            {
                self.publish_music_selection(
                    session_id,
                    capture_generation,
                    source_sequence,
                    MusicSelectionState::Unresolved {
                        reason: MusicSelectionUnresolvedReason::EvidenceUnresolved,
                    },
                )
            }
            _ => Ok(()),
        }
    }

    fn publish_music_selection(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        source_sequence: u64,
        state: MusicSelectionState,
    ) -> Result<(), String> {
        if self.active_music_selection.as_ref() == Some(&state) {
            return Ok(());
        }
        self.music_selection_revision = self.music_selection_revision.saturating_add(1);
        self.publish_one(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::MusicSelectionChanged {
                session_id: session_id.cloned(),
                capture_generation,
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
    fn reduce_music_select_observation(
        &mut self,
        session_id: Option<&String>,
        capture_generation: Option<u64>,
        sequence: u64,
        monotonic_end_ms: u64,
        fields: &Value,
        joint_evidence: &JointEvidenceObservation,
        _presentation: &SongResolutionPresentation,
    ) -> Result<(), String> {
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
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::SelectionDifficultyChanged {
                    session_id: session_id.cloned(),
                    capture_generation,
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
            capture_generation,
            sequence,
            ResolverScope::SelectionIncumbent,
            &current_summary,
            self.engine.selection_epochs.incumbent.observation_count,
        )?;
        if self.engine.selection_epochs.successor.observation_count > 0 {
            let challenger_summary = self.engine.selection_epochs.successor.summary();
            self.publish_resolver_transition(
                session_id,
                capture_generation,
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
                scorepeek_core::recognition::music_select::MusicSelectBestObservation,
            >(fields["best"].clone())
            .unwrap_or_default();
            self.music_select_resolver.best.screen_episode_id = self.screen_episode_id;
            self.music_select_resolver
                .best
                .observe_frame(identity, best.values);
            if let (Some(session), Some(generation)) = (session_id, capture_generation)
                && let Some(snapshot) = self.music_select_resolver.best.publish_candidate(
                    session,
                    generation,
                    sequence,
                    monotonic_end_ms,
                )
            {
                self.publish_one(&RunEvent {
                    schema: RUN_EVENT_SCHEMA.to_owned(),
                    kind: RunEventKind::MusicSelectBestObserved {
                        session_id: session.clone(),
                        capture_generation: generation,
                        snapshot,
                    },
                })?;
            }
        }
        self.sync_music_selection(session_id, capture_generation, sequence)?;
        self.sync_resolver_snapshot(monotonic_end_ms, Some(sequence), Some(fields))?;
        self.refresh()?;
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one owner preserves ordered screen, attempt, reset, and diagnostic transitions"
    )]
    fn publish_screen_change(
        &mut self,
        event: &RunEvent,
        publish_event: bool,
    ) -> Result<(), String> {
        let RunEventKind::ScreenChanged {
            session_id,
            capture_generation,
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
                *capture_generation,
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
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::SelectionDifficultyChanged {
                    session_id: session_id.clone(),
                    capture_generation: *capture_generation,
                    screen_episode_id: *screen_episode_id,
                    source_sequence: *sequence,
                    target,
                    reason: SelectionDifficultyTransitionReason::Reset,
                    current: None,
                },
            })?;
        }
        let unresolved = HypothesisAccumulator::default().summary();
        if close_result_resolver {
            self.publish_resolver_transition(
                session_id.as_ref(),
                *capture_generation,
                *sequence,
                ResolverScope::Result,
                &unresolved,
                0,
            )?;
            self.publish_resolver_transition(
                session_id.as_ref(),
                *capture_generation,
                *sequence,
                ResolverScope::AttemptJoint,
                &unresolved,
                0,
            )?;
        }
        if screen == "music_select" {
            self.publish_resolver_transition(
                session_id.as_ref(),
                *capture_generation,
                *sequence,
                ResolverScope::SelectionIncumbent,
                &unresolved,
                0,
            )?;
            self.publish_resolver_transition(
                session_id.as_ref(),
                *capture_generation,
                *sequence,
                ResolverScope::SelectionSuccessor,
                &unresolved,
                0,
            )?;
        }
        if screen == "result" {
            self.publish_resolver_transition(
                session_id.as_ref(),
                *capture_generation,
                *sequence,
                ResolverScope::Result,
                &unresolved,
                0,
            )?;
            self.publish_resolver_transition(
                session_id.as_ref(),
                *capture_generation,
                *sequence,
                ResolverScope::AttemptJoint,
                &unresolved,
                0,
            )?;
        }
        if let Some(attempt_screen) = play_attempt_screen(screen)
            && let Some(state) = self
                .engine
                .play_attempt
                .observe_screen(attempt_screen, *sequence)
        {
            self.publish_play_attempt_update(
                session_id.clone(),
                *capture_generation,
                Some(*sequence),
                state,
            )?;
        }
        if screen == "play"
            && let (Some(session_id), Some(capture_generation)) =
                (session_id.clone(), *capture_generation)
        {
            self.active_provisional_result = None;
            self.publish_result_state(
                session_id,
                capture_generation,
                *sequence,
                ResultState::Inactive,
            )?;
        }
        if let Some(state) = selection_screen_attempt_update {
            self.attempt_started_ms = None;
            self.attempt_phase_started_ms = None;
            self.publish_play_attempt_update(
                session_id.clone(),
                *capture_generation,
                Some(*sequence),
                state,
            )?;
        }
        if screen != "result" {
            self.result_panel_side.clear();
            self.result_select_context_detached = false;
            self.reset_numeric_result();
            self.numeric_evidence.clear();
            self.play_options = PlayOptionsEpisodeAccumulator::default();
        }
        self.sync_resolver_snapshot(*monotonic_end_ms, Some(*sequence), None)?;
        self.refresh()?;
        Ok(())
    }

    fn publish_session_finished(&mut self, event: &RunEvent) -> Result<(), String> {
        let RunEventKind::SessionFinished {
            session_id,
            capture_generation,
            ..
        } = &event.kind
        else {
            unreachable!("session-finished dispatcher preserves event kind");
        };
        if self.music_selection_episode_active {
            let source_sequence = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?
                .resolver
                .source_sequence
                .or(self.latest_screen_boundary_sequence)
                .unwrap_or_default();
            self.publish_music_selection(
                Some(session_id),
                Some(*capture_generation),
                source_sequence,
                MusicSelectionState::Unresolved {
                    reason: MusicSelectionUnresolvedReason::EpisodeEnded,
                },
            )?;
            self.music_selection_episode_active = false;
        }
        self.result_panel_side.clear();
        self.result_select_context_detached = false;
        self.publish_one(event)?;
        if let Some(state) = self.engine.play_attempt.finish_session() {
            self.publish_play_attempt_update(
                Some(session_id.clone()),
                Some(*capture_generation),
                None,
                state,
            )?;
        }
        Ok(())
    }

    fn publish_play_attempt_update(
        &mut self,
        session_id: Option<String>,
        capture_generation: Option<u64>,
        source_sequence: Option<u64>,
        state: PlayAttemptState,
    ) -> Result<(), String> {
        self.publish_one(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::PlayAttemptChanged {
                session_id: session_id.clone(),
                capture_generation,
                source_sequence,
                state,
            },
        })?;
        if let Some(sequence) = source_sequence {
            self.try_emit_result(session_id, capture_generation, sequence)?;
        }
        Ok(())
    }

    fn sync_music_select_resolver_state(&mut self) -> Result<(), String> {
        let (active, previous, session_id, capture_generation) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            (
                state.current_screen.as_deref() == Some("music_select")
                    && self.music_selection_episode_active,
                state.music_select.clone(),
                state.active_session_id.clone(),
                state.capture_generation,
            )
        };
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
        if !best.same_notification(&previous) {
            let state = best.clone();
            self.publish_one(&RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::MusicSelectResolverChanged {
                    session_id,
                    capture_generation,
                    state,
                },
            })?;
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one typed snapshot derives the complete fixed resolver tree and promotion gates"
    )]
    fn sync_resolver_snapshot(
        &mut self,
        now_ms: u64,
        source_sequence: Option<u64>,
        raw_fields: Option<&Value>,
    ) -> Result<(), String> {
        self.screen_episode_last_ms = Some(now_ms);
        self.sync_music_select_resolver_state()?;
        let current_screen = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .current_screen
            .clone();
        let current_summary = self.engine.selection_epochs.incumbent.summary();
        let challenger_summary = self.engine.selection_epochs.successor.summary();
        let result_summary = self.engine.result_hypotheses.summary();
        let mut joint = self.engine.retained_select.clone();
        joint.add_from(&self.engine.result_hypotheses);
        let joint_summary = joint.summary();
        let local = match current_screen.as_deref() {
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
        let successor = (current_screen.as_deref() == Some("music_select")
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
        let gate = if self
            .engine
            .play_attempt
            .accepted_result()
            .is_some_and(|attempt| self.emitted_attempt_ids.contains(&attempt.attempt_id))
        {
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
        let emitted = self
            .engine
            .play_attempt
            .accepted_result()
            .is_some_and(|attempt| self.emitted_attempt_ids.contains(&attempt.attempt_id));
        let attempt_completed_rejected = matches!(
            self.engine.play_attempt.state(),
            PlayAttemptState::Attempt { attempt }
                if matches!(attempt.phase, scorepeek_core::session::attempt::PlayAttemptPhase::Completed)
                    && !attempt.reasons.is_empty()
        );
        let linked = matches!(
            self.engine.play_attempt.state(),
            PlayAttemptState::Attempt { attempt }
                if attempt.path.select_observed
                    && attempt.path.play_observed
                    && attempt.path.result_observed
        );
        let gates = vec![
            GateSnapshot {
                label: "link",
                state: if linked {
                    GateState::Accepted
                } else if attempt_completed_rejected {
                    GateState::Failed
                } else {
                    GateState::Pending
                },
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
                state: if self.accepted_numeric_result.is_some() {
                    GateState::Accepted
                } else if attempt_completed_rejected {
                    GateState::Failed
                } else {
                    GateState::Pending
                },
            },
            GateSnapshot {
                label: "numeric",
                state: if self.accepted_numeric_result.is_some() {
                    GateState::Accepted
                } else if attempt_completed_rejected {
                    GateState::Failed
                } else {
                    GateState::Pending
                },
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
        let mut state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?;
        let retained_raw =
            raw_fields.map_or_else(|| state.resolver.raw_fields.clone(), important_raw_fields);
        let latest_field_sequence = raw_fields
            .and(source_sequence)
            .or(state.resolver.latest_field_sequence);
        let latest_field_ms = raw_fields
            .map(|_| now_ms)
            .or(state.resolver.latest_field_ms);
        let selection_difficulty = self.engine.selection_epochs.active_difficulty_state();
        state.resolver =
            ResolverDebugSnapshot {
                now_ms,
                raw_screen: state.raw_screen.clone(),
                screen: current_screen,
                suspended: self.semantic_episode_suspended,
                finalizing: self.result_episode_finalizing,
                screen_episode_id: self.screen_episode_id,
                screen_episode_started_ms: self.screen_episode_started_ms,
                source_sequence,
                latest_field_sequence,
                latest_field_ms,
                selection_difficulty_target: selection_difficulty.map(|(target, _)| target),
                selection_difficulty: selection_difficulty.and_then(|(_, current)| current),
                local,
                successor,
                attempt,
                gate,
                gates,
                raw_fields: retained_raw,
                play_options: self.play_options.latest.clone().map(|latest| {
                    PlayOptionsDebugSnapshot {
                        latest,
                        observations: self.play_options.observations,
                        conflicting: self.play_options.conflicting,
                        resolved: self.play_options.resolved(),
                    }
                }),
            };
        Ok(())
    }

    fn clear_resolver_field_observation(&mut self) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?;
        state.resolver.raw_fields.clear();
        state.resolver.latest_field_sequence = None;
        state.resolver.latest_field_ms = None;
        Ok(())
    }

    fn publish_one(&mut self, event: &RunEvent) -> Result<(), String> {
        self.publish_one_inner(event, true)
    }

    fn publish_one_inner(&mut self, event: &RunEvent, refresh: bool) -> Result<(), String> {
        let output_started = Instant::now();
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let mut value = bounded_run_event_value(event)?;
        if let Some(object) = value.as_object_mut() {
            object.insert("channel_sequence".to_owned(), sequence.into());
        }
        let mut public_observations = Vec::new();
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            state.reduce(event, &value);
            state.next_channel_sequence = self.next_sequence;
            if event_api::PublicState::observes(event) {
                let mut projected = state.public.clone();
                let events = projected.project(event);
                public_observations = commit_public_projection(
                    &mut state.public,
                    projected,
                    &events,
                    self.channel.as_ref(),
                    self.scores.as_ref(),
                );
            }
        }
        if let Some(channel) = &self.channel {
            value["event_api_health"] = channel.health.value();
            if let Ok(state) = self.state.lock() {
                value["event_api_health"]["invocation_id"] = state.invocation_id.clone().into();
            }
        }
        if self.channel.is_none()
            && let Ok(state) = self.state.lock()
            && let Some(cause) = &state.channel_start_failure
        {
            value["event_api_health"] = json!({"status":"degraded", "error_type":"startup_failed", "cause":cause, "invocation_id":state.invocation_id});
        }
        if let Some(health) = self.scores_health() {
            value["scores_health"] = serde_json::to_value(health).unwrap_or(Value::Null);
        }
        if let Some(diagnostics) = &self.diagnostics {
            let sink: DiagnosticSink = diagnostics.sink();
            for observation in public_observations {
                sink.record("public_event", &observation, false);
            }
            value["diagnostic_health"] = sink.health();
            sink.record("run_event", &value, important_run_event(event));
        }
        #[cfg(test)]
        self.headless_events.push(event.clone());
        if self.timing_active {
            self.output_us = self
                .output_us
                .saturating_add(duration_us(output_started.elapsed()));
        }
        if refresh { self.refresh() } else { Ok(()) }
    }

    pub fn watcher_state(
        &mut self,
        state_name: &str,
        session_id: Option<&str>,
        generation: Option<u64>,
        message: &str,
    ) -> Result<(), String> {
        let diagnostic_sink = self.diagnostics.as_ref().map(RunDiagnostics::sink);
        let observations = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            state_name.clone_into(&mut state.watcher_state);
            state.active_session_id = session_id.map(ToOwned::to_owned);
            state.capture_generation = generation;
            message.clone_into(&mut state.message);
            let mut projected = state.public.clone();
            let events = projected
                .watcher(
                    event_api::WatcherStatus::from_internal(state_name)
                        .ok_or_else(|| "unknown public watcher state".to_owned())?,
                )
                .into_iter()
                .collect::<Vec<_>>();
            commit_public_projection(
                &mut state.public,
                projected,
                &events,
                self.channel.as_ref(),
                self.scores.as_ref(),
            )
        };
        if let Some(sink) = &diagnostic_sink {
            for observation in observations {
                sink.record("public_event", &observation, false);
            }
        }
        self.refresh()
    }

    pub fn warning(&mut self, message: impl Into<String>) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .message = message.into();
        self.refresh()
    }

    pub fn status_recording_degraded(&mut self) -> Result<(), String> {
        self.state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .status_recording = "degraded";
        self.refresh()
    }

    fn refresh(&mut self) -> Result<(), String> {
        let output_started = Instant::now();
        let mut state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .clone();
        if let Some(health) = self.scores_health()
            && let Some(path) = &state.scores_summary
        {
            state.scores_summary = Some(format!(
                "scores={} unsaved={} db={path}",
                if health.failure.is_some() {
                    "degraded"
                } else if health.flush.is_some() {
                    "saved"
                } else {
                    "active"
                },
                health.pending + health.rejected
            ));
        }
        if !self.publish_frontend_snapshots {
            return Ok(());
        }
        let unavailable = ChannelHealth {
            server_failed: AtomicBool::new(true),
            ..ChannelHealth::default()
        };
        let health = self
            .channel
            .as_ref()
            .map_or(&unavailable, |channel| channel.health.as_ref());
        let channel = health.value();
        let snapshot = scorepeek_frontend_api::ApplicationSnapshot {
            revision: scorepeek_frontend_api::Revision(state.next_channel_sequence),
            running: state.watcher_state != "stopped",
            diagnostic_run_id: self
                .diagnostics
                .as_ref()
                .and_then(RunDiagnostics::run_root)
                .and_then(Path::file_name)
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_owned),
            run: Some(scorepeek_frontend_api::RunSnapshot {
                watcher_state: state.watcher_state.clone(),
                session_count: state.session_count,
                active_session_id: state.active_session_id.clone(),
                capture_generation: state.capture_generation,
                raw_screen: state.raw_screen.clone(),
                semantic_screen: state.current_screen.clone(),
                recording_status: state.status_recording.to_owned(),
                recording_memory_used_bytes: state.recording_memory_used_bytes,
                recording_memory_limit_bytes: state.recording_memory_limit_bytes,
                recording_dropped_frames: state.recording_dropped_frames,
                event_stream_status: channel["status"].as_str().unwrap_or("degraded").to_owned(),
                connected_clients: channel["connected_clients"].as_u64().unwrap_or(0),
                dropped_events: channel["dropped_events"].as_u64().unwrap_or(0),
                disconnected_clients: channel["disconnected_clients"].as_u64().unwrap_or(0),
                result_count: state.result_count,
                latest_result_label: state.latest_result_label.map(str::to_owned),
                scores_summary: state.scores_summary.clone(),
                overlay_summary: state.overlay_summary.clone(),
                message: state.message.clone(),
            }),
        };
        let _ = crate::service::dispatch::frontend_event(
            scorepeek_frontend_api::FrontendEvent::Snapshot {
                snapshot: Box::new(snapshot),
            },
        );
        let result = Ok(());
        if self.timing_active {
            self.output_us = self
                .output_us
                .saturating_add(duration_us(output_started.elapsed()));
        }
        result
    }
}

fn important_run_event(event: &RunEvent) -> bool {
    matches!(
        event.kind,
        RunEventKind::WatcherStarted { .. }
            | RunEventKind::WatcherStopped { .. }
            | RunEventKind::SessionStarted { .. }
            | RunEventKind::SessionFinished { .. }
            | RunEventKind::RecordingHealthChanged { .. }
            | RunEventKind::RecordingFinalizing { .. }
            | RunEventKind::RecordingCompleted { .. }
    )
}

impl Drop for RoutineOutput {
    fn drop(&mut self) {
        if let Some(scores) = &mut self.scores {
            scores.finish();
            let _ = self.refresh();
        }
    }
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

fn bounded_run_event_value(event: &RunEvent) -> Result<Value, String> {
    let RunEventKind::FieldObservation {
        session_id,
        capture_generation,
        screen_episode_id,
        sequence,
        monotonic_start_ms,
        monotonic_end_ms,
        screen,
        fields,
        result_song_resolution,
        music_select_song_resolution,
        parsed_result_fields,
        result_chart_resolution,
        result_performance_resolution,
        current_score_ocr_resolution,
        numeric_batch,
        joint_evidence,
        processing_timing,
        song_resolution_presentation,
    } = &event.kind
    else {
        return event.to_value();
    };
    RunEvent {
        schema: event.schema.clone(),
        kind: RunEventKind::FieldObservation {
            session_id: session_id.clone(),
            capture_generation: *capture_generation,
            screen_episode_id: *screen_episode_id,
            sequence: *sequence,
            monotonic_start_ms: *monotonic_start_ms,
            monotonic_end_ms: *monotonic_end_ms,
            screen: screen.clone(),
            fields: fields.clone(),
            result_song_resolution: result_song_resolution.clone(),
            music_select_song_resolution: music_select_song_resolution.clone(),
            parsed_result_fields: parsed_result_fields.clone(),
            result_chart_resolution: result_chart_resolution.clone(),
            result_performance_resolution: result_performance_resolution.clone(),
            current_score_ocr_resolution: current_score_ocr_resolution.clone(),
            numeric_batch: numeric_batch.clone(),
            joint_evidence: joint_evidence.diagnostic_top(),
            processing_timing: processing_timing.clone(),
            song_resolution_presentation: song_resolution_presentation.clone(),
        },
    }
    .to_value()
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

fn selected_difficulty(fields: &Value) -> Option<Difficulty> {
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

fn selected_play_type(fields: &Value) -> Option<PlayType> {
    let value = fields.pointer("/play_type/state")?.get("value")?.as_str()?;
    match value {
        "single" => Some(PlayType::Single),
        "double" => Some(PlayType::Double),
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
        "{} / {} {} Lv{} notes={}",
        title,
        play_type_label(candidate.chart.key.play_type),
        difficulty_label(candidate.chart.key.difficulty),
        candidate.chart.level,
        candidate.chart.notes,
    )
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

fn important_play_side(fields: &Value) -> Option<(String, String)> {
    let observation = fields.get("play_side")?;
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
}

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
    let play_side = important_play_side(fields);
    let keys = [
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
    ];
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
        .chain(keys.into_iter().filter_map(|key| {
            fields
                .get(key)
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| (key.to_owned(), value.to_owned()))
        }))
        .take(8)
        .collect()
}

#[cfg(test)]
fn elapsed_seconds(now_ms: u64, started_ms: Option<u64>) -> u64 {
    started_ms.map_or(0, |started| now_ms.saturating_sub(started) / 1_000)
}

#[cfg(test)]
fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!(
            "{}.{:02}GiB",
            bytes / GIB,
            (bytes % GIB).saturating_mul(100) / GIB
        )
    } else if bytes >= MIB {
        format!(
            "{}.{}MiB",
            bytes / MIB,
            (bytes % MIB).saturating_mul(10) / MIB
        )
    } else if bytes >= KIB {
        format!(
            "{}.{}KiB",
            bytes / KIB,
            (bytes % KIB).saturating_mul(10) / KIB
        )
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
fn plain_status_line(state: &RunViewState, health: &ChannelHealth) -> String {
    let channel = health.value();
    format!(
        "scorepeek: state={} sessions={} session={} generation={} channel={} clients={} dropped={} disconnected={} message={} {}{}",
        state.watcher_state,
        state.session_count,
        state.active_session_id.as_deref().unwrap_or("-"),
        state
            .capture_generation
            .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        channel["status"].as_str().unwrap_or("degraded"),
        channel["connected_clients"].as_u64().unwrap_or(0),
        channel["dropped_events"].as_u64().unwrap_or(0),
        channel["disconnected_clients"].as_u64().unwrap_or(0),
        state
            .channel_start_failure
            .as_deref()
            .unwrap_or(&state.message),
        state.scores_summary.as_deref().unwrap_or("scores=disabled"),
        state.overlay_summary,
    )
}

#[cfg(test)]
fn render(
    frame: &mut ratatui::Frame<'_>,
    state: &RunViewState,
    _socket_path: &Path,
    health: &ChannelHealth,
) {
    let area = frame.area();
    let available_width = area.width.saturating_sub(2) as usize;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(fixed_watcher_lines(state, health)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(watcher_color(state, health)))
                .title(state.scores_summary.as_ref().map_or_else(
                    || "Watcher".to_owned(),
                    |scores| format!("Watcher {scores}"),
                )),
        ),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new(fixed_domain_lines(state, available_width)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(match state.latest_result_label {
                    Some("RETRACTED") => Color::Red,
                    Some("PROVISIONAL") => Color::Yellow,
                    Some("INACTIVE") => Color::DarkGray,
                    _ if state.latest_result_detected.is_some() => Color::Green,
                    _ => Color::DarkGray,
                }))
                .title("Latest result"),
        ),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(if rows[3].height < 12 {
            compact_resolver_lines(&state.resolver, available_width)
        } else {
            resolver_lines(&state.resolver, available_width)
        })
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(resolver_color(&state.resolver)))
                .title("Resolver"),
        ),
        rows[3],
    );
    frame.render_widget(
        Paragraph::new(music_select_best::lines(
            &state.music_select,
            available_width,
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Music Select Resolver")
                .border_style(Style::default().fg(if state.music_select.suspended {
                    Color::Yellow
                } else if state.music_select.chart.is_some() {
                    Color::Cyan
                } else {
                    Color::DarkGray
                })),
        ),
        rows[2],
    );
}

#[cfg(test)]
fn fixed_watcher_lines(state: &RunViewState, health: &ChannelHealth) -> Vec<Line<'static>> {
    let channel = health.value();
    let resolver = &state.resolver;
    vec![
        Line::from(vec![
            Span::styled(
                state.watcher_state.clone(),
                Style::default().fg(watcher_color(state, health)),
            ),
            Span::raw(format!(
                "  raw={} semantic=",
                resolver.raw_screen.as_deref().unwrap_or("-"),
            )),
            Span::styled(
                resolver.screen.clone().unwrap_or_else(|| "-".to_owned()),
                Style::default().fg(if resolver.suspended {
                    Color::Yellow
                } else {
                    Color::Cyan
                }),
            ),
            Span::raw(format!(
                "{} episode=#{} {}s  sessions={} gen={}",
                if resolver.suspended { " suspended" } else { "" },
                resolver.screen_episode_id,
                elapsed_seconds(resolver.now_ms, resolver.screen_episode_started_ms),
                state.session_count,
                state
                    .capture_generation
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            )),
        ]),
        Line::from(format!(
            "recording={} mem={}/{} high={} frame_drop={}  channel={} clients={} drop={}{}  {}",
            state.status_recording,
            human_bytes(state.recording_memory_used_bytes),
            human_bytes(state.recording_memory_limit_bytes),
            human_bytes(state.recording_memory_high_water_bytes),
            state.recording_dropped_frames,
            channel["status"].as_str().unwrap_or("degraded"),
            channel["connected_clients"].as_u64().unwrap_or(0),
            channel["dropped_events"].as_u64().unwrap_or(0),
            state.overlay_summary,
            state.message,
        )),
    ]
}

#[cfg(test)]
fn watcher_color(state: &RunViewState, health: &ChannelHealth) -> Color {
    let channel = health.value();
    if channel["status"].as_str() == Some("degraded")
        || channel["dropped_events"].as_u64().unwrap_or(0) > 0
        || state.status_recording == "degraded"
    {
        Color::Red
    } else if state.watcher_state == "stopped" {
        Color::DarkGray
    } else if state.watcher_state == "starting" || state.resolver.suspended {
        Color::Yellow
    } else {
        Color::Green
    }
}

#[cfg(test)]
fn resolution_color(state: ResolverResolutionState) -> Color {
    match state {
        ResolverResolutionState::AcceptedJoint => Color::Green,
        ResolverResolutionState::Conflict => Color::Red,
        ResolverResolutionState::SongProjected | ResolverResolutionState::JointCandidate => {
            Color::Cyan
        }
        ResolverResolutionState::Unresolved => Color::Yellow,
    }
}

#[cfg(test)]
fn resolver_color(snapshot: &ResolverDebugSnapshot) -> Color {
    if snapshot
        .gates
        .iter()
        .any(|gate| gate.state == GateState::Failed)
    {
        Color::Red
    } else if snapshot
        .gates
        .iter()
        .any(|gate| gate.label == "emit" && gate.state == GateState::Accepted)
    {
        Color::Green
    } else if snapshot.suspended || snapshot.finalizing {
        Color::Yellow
    } else if snapshot.local.is_some() {
        Color::Cyan
    } else {
        Color::DarkGray
    }
}

#[cfg(test)]
const fn gate_color(state: GateState) -> Color {
    match state {
        GateState::Accepted => Color::Green,
        GateState::Pending => Color::Yellow,
        GateState::Failed => Color::Red,
        GateState::Inactive => Color::DarkGray,
    }
}

#[cfg(test)]
const fn gate_suffix(state: GateState) -> &'static str {
    match state {
        GateState::Accepted => "✓",
        GateState::Pending => "…",
        GateState::Failed => "✗",
        GateState::Inactive => "–",
    }
}

#[cfg(test)]
fn fixed_domain_lines(state: &RunViewState, available_width: usize) -> Vec<Line<'static>> {
    if state.latest_result_label == Some("INACTIVE") {
        return vec![Line::from(Span::styled(
            "No active result",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    if let Some(entry) = state.latest_provisional_result.as_ref() {
        let mut lines = expanded_result_history_entry_lines(
            entry,
            available_width,
            state.latest_result_label.unwrap_or("PROVISIONAL"),
        );
        lines.truncate(6);
        return lines;
    }
    let Some(entry) = state.result_history.back() else {
        return vec![Line::from(Span::styled(
            "No result payload yet",
            Style::default().fg(Color::DarkGray),
        ))];
    };
    let mut lines = expanded_result_history_entry_lines(
        entry,
        available_width,
        &format!("CONFIRMED #{}", entry.ordinal),
    );
    lines.truncate(6);
    lines
}

#[cfg(test)]
fn compact_resolver_lines(snapshot: &ResolverDebugSnapshot, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if snapshot.screen.as_deref() == Some("result")
        && let Some(local) = &snapshot.local
    {
        lines.push(Line::from(fitted_value(
            &format!(
                "{:?} support={} songΔ={} chartΔ={} ",
                local.state, local.support, local.song_margin, local.chart_margin
            ),
            local.top.as_deref().unwrap_or("-"),
            width,
        )));
    } else {
        lines.push(Line::from("No active RESULT resolution"));
    }
    lines.push(Line::from(snapshot.attempt.as_ref().map_or_else(
        || "ATTEMPT -".to_owned(),
        |a| {
            format!(
                "ATTEMPT #{} {} {}",
                a.attempt_id
                    .map_or_else(|| "-".to_owned(), |id| id.to_string()),
                a.phase,
                a.path
            )
        },
    )));
    lines.push(Line::from(fitted_value("", &snapshot.gate, width)));
    lines.push(Line::from(
        snapshot
            .gates
            .iter()
            .map(|g| {
                Span::styled(
                    format!("{}{} ", g.label, gate_suffix(g.state)),
                    Style::default().fg(gate_color(g.state)),
                )
            })
            .collect::<Vec<_>>(),
    ));
    lines
}

#[allow(
    clippy::too_many_lines,
    reason = "the fixed ten-line debug tree keeps semantic styles adjacent to their typed values"
)]
#[cfg(test)]
fn resolver_lines(snapshot: &ResolverDebugSnapshot, available_width: usize) -> Vec<Line<'static>> {
    if snapshot.screen.as_deref() != Some("result") {
        return compact_resolver_lines(snapshot, available_width);
    }
    let semantic_color = if snapshot.suspended {
        Color::Yellow
    } else {
        Color::Cyan
    };
    let mut lines = vec![Line::from(vec![
        Span::raw(format!(
            "SCREEN raw={} semantic=",
            snapshot.raw_screen.as_deref().unwrap_or("-")
        )),
        Span::styled(
            snapshot.screen.clone().unwrap_or_else(|| "-".to_owned()),
            Style::default()
                .fg(semantic_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if snapshot.suspended { " suspended" } else { "" },
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(format!(
            "  episode=#{} {}s seq={}",
            snapshot.screen_episode_id,
            elapsed_seconds(snapshot.now_ms, snapshot.screen_episode_started_ms),
            snapshot
                .source_sequence
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        )),
    ])];
    lines.push(Line::from(vec![
        Span::styled("├─ FIELD ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            if snapshot.finalizing {
                "draining"
            } else {
                "completed"
            },
            Style::default().fg(if snapshot.finalizing {
                Color::Yellow
            } else if snapshot.latest_field_sequence.is_some() {
                Color::Green
            } else {
                Color::DarkGray
            }),
        ),
        Span::raw(format!(
            " seq={} age={}s",
            snapshot
                .latest_field_sequence
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            elapsed_seconds(snapshot.now_ms, snapshot.latest_field_ms),
        )),
    ]));
    let raw = snapshot
        .raw_fields
        .iter()
        .map(|(key, value)| format!("{key}=\"{value}\""))
        .collect::<Vec<_>>();
    lines.push(Line::from(Span::styled(
        fitted_value(
            "│  OCR ",
            &raw.iter().take(3).cloned().collect::<Vec<_>>().join(" "),
            available_width,
        ),
        Style::default().fg(Color::White),
    )));
    if snapshot.screen.as_deref() == Some("result") {
        let (text, color) = snapshot.play_options.as_ref().map_or_else(
            || {
                (
                    "raw=- marker=- typed=unknown(not_observed)".to_owned(),
                    Color::DarkGray,
                )
            },
            |options| {
                let raw = options.latest.raw_text.as_deref().unwrap_or("-");
                let marker = format!(
                    "{:?}:{}",
                    options.latest.marker.state, options.latest.marker.orange_pixels
                )
                .to_ascii_lowercase();
                let typed = play_options_text(&options.resolved);
                let color = match options.resolved {
                    PlayOptions::Known { .. } => Color::Green,
                    PlayOptions::Unknown {
                        reason: PlayOptionsUnknownReason::ConflictingObservations,
                    } => Color::Red,
                    PlayOptions::Unknown { .. } => Color::Yellow,
                };
                (
                    format!(
                        "raw=\"{raw}\" marker={marker} typed={typed} stable={}{}",
                        options.observations,
                        if options.conflicting { " conflict" } else { "" },
                    ),
                    color,
                )
            },
        );
        lines.push(Line::from(Span::styled(
            fitted_value("│  OPT ", &text, available_width),
            Style::default().fg(color),
        )));
    }
    if let Some(local) = &snapshot.local {
        lines.push(Line::from(vec![
            Span::styled("├─ LOCAL ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:?}", local.state),
                Style::default().fg(resolution_color(local.state)),
            ),
            Span::raw(format!(
                " obs={} age={}s support={} songΔ={} chartΔ={}",
                local.observations,
                elapsed_seconds(snapshot.now_ms, local.last_observation_ms),
                local.support,
                local.song_margin,
                local.chart_margin,
            )),
        ]));
        lines.push(Line::from(Span::styled(
            fitted_value(
                "│  TOP ",
                local.top.as_deref().unwrap_or("-"),
                available_width,
            ),
            Style::default().fg(resolution_color(local.state)),
        )));
        lines.push(Line::from(fitted_value(
            "│  RUN song=",
            &format!(
                "{} chart={} {}={}",
                local.runner_song.as_deref().unwrap_or("-"),
                local.runner_chart.as_deref().unwrap_or("-"),
                if snapshot.successor.is_some() {
                    "successor"
                } else {
                    "#3"
                },
                snapshot
                    .successor
                    .as_ref()
                    .and_then(|value| value.top.as_deref())
                    .unwrap_or_else(|| local.top_candidates.get(2).map_or("-", String::as_str)),
            ),
            available_width,
        )));
    }
    if let Some(attempt) = &snapshot.attempt {
        lines.push(Line::from(vec![
            Span::styled("└─ ATTEMPT ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "{} phase={} path={}",
                    attempt
                        .attempt_id
                        .map_or_else(|| "-".to_owned(), |value| format!("#{value}")),
                    attempt.phase,
                    attempt.path,
                ),
                Style::default().fg(Color::Cyan),
            ),
            Span::raw(format!(
                " {}s",
                elapsed_seconds(snapshot.now_ms, attempt.started_ms)
            )),
        ]));
        lines.push(Line::from(fitted_value(
            "   FACTORS ",
            &attempt.family_contributions.join(" "),
            available_width,
        )));
    } else if let Some(local) = &snapshot.local {
        lines.push(Line::from(fitted_value(
            "│  FACTORS ",
            &local.family_contributions.join(" "),
            available_width,
        )));
    }
    let mut gate_spans = vec![Span::styled(
        "   GATES ",
        Style::default().fg(Color::DarkGray),
    )];
    for gate in &snapshot.gates {
        gate_spans.push(Span::styled(
            format!("{}{} ", gate.label, gate_suffix(gate.state)),
            Style::default().fg(gate_color(gate.state)),
        ));
    }
    lines.push(Line::from(gate_spans));
    lines
}

#[cfg(test)]
fn expanded_result_history_entry_lines(
    entry: &ResultHistoryEntry,
    available_width: usize,
    authority: &str,
) -> Vec<Line<'static>> {
    let result = &entry.result;
    let maximum_score = u64::from(result.notes) * 2;
    let percentage_tenths = u64::from(result.current_score)
        .checked_mul(1_000)
        .and_then(|value| value.checked_div(maximum_score))
        .unwrap_or(0);
    let title = entry
        .song
        .as_ref()
        .and_then(|song| song.display_titles.first())
        .map_or_else(
            || result.scorepeek_song_id.as_uuid().to_string(),
            ToOwned::to_owned,
        );
    vec![
        Line::from(vec![
            Span::styled(
                format!("{authority} {}", result.clear_type),
                Style::default()
                    .fg(clear_type_color(&result.clear_type))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  {} ", play_type_label(result.play_type))),
            Span::styled(
                difficulty_label(result.difficulty),
                Style::default().fg(difficulty_color(result.difficulty)),
            ),
            Span::raw(format!(
                " Lv{}  EX SCORE {} / {} ({}.{:01}%)",
                result.level,
                grouped_u32(result.current_score),
                grouped_u64(maximum_score),
                percentage_tenths / 10,
                percentage_tenths % 10,
            )),
        ]),
        Line::from(fitted_value("Title: ", &title, available_width)),
        Line::from(format!(
            "PGREAT {}  GREAT {}  GOOD {}  BAD {}  POOR {}",
            grouped_u32(result.judgments.pgreat),
            grouped_u32(result.judgments.great),
            grouped_u32(result.judgments.good),
            grouped_u32(result.judgments.bad),
            grouped_u32(result.judgments.poor),
        )),
        Line::from(format!(
            "MISS {}  FAST {}  SLOW {}  COMBO BREAK {}",
            supplemental_u32(&result.miss_count),
            supplemental_u32(&result.timing.fast),
            supplemental_u32(&result.timing.slow),
            supplemental_u32(&result.combo_break),
        )),
        Line::from(format!(
            "Previous: clear={}  EX SCORE {}  MISS {}",
            previous_text(&result.previous_best.clear_type),
            previous_u32(&result.previous_best.score),
            previous_u32(&result.previous_best.miss_count),
        )),
        Line::from(fitted_value(
            "Artist: ",
            &format!(
                "{}  Options: {}",
                entry.song.as_ref().map_or("-", |song| song.artist.as_str()),
                play_options_text(&result.play_options),
            ),
            available_width,
        )),
        Line::from(format!(
            "attempt=#{} parent=#{}  notes={} side={} mode={} sequence={}",
            result.attempt_id,
            result
                .parent_attempt_id
                .map_or_else(|| "-".to_owned(), |value| format!("{value}")),
            grouped_u32(result.notes),
            play_side_label(result.play_side),
            result.play_mode,
            entry.source_sequence,
        )),
    ]
}

#[cfg(test)]
const fn clear_type_color(clear_type: &str) -> Color {
    match clear_type.as_bytes() {
        b"FAILED" => Color::Red,
        b"ASSIST CLEAR" | b"EASY CLEAR" => Color::Yellow,
        b"CLEAR" | b"HARD CLEAR" | b"EXH-CLEAR" | b"F-COMBO" => Color::Green,
        _ => Color::White,
    }
}

#[cfg(test)]
const fn difficulty_color(difficulty: Difficulty) -> Color {
    match difficulty {
        Difficulty::Beginner => Color::Green,
        Difficulty::Normal => Color::Blue,
        Difficulty::Hyper => Color::Yellow,
        Difficulty::Another => Color::Red,
        Difficulty::Leggendaria => Color::Magenta,
    }
}

pub(super) const fn play_type_label(play_type: PlayType) -> &'static str {
    match play_type {
        PlayType::Single => "SP",
        PlayType::Double => "DP",
    }
}

#[cfg(test)]
const fn play_side_label(play_side: PlaySide) -> &'static str {
    match play_side {
        PlaySide::OnePlayer => "1P",
        PlaySide::TwoPlayer => "2P",
    }
}

pub(super) const fn difficulty_label(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Beginner => "BEGINNER",
        Difficulty::Normal => "NORMAL",
        Difficulty::Hyper => "HYPER",
        Difficulty::Another => "ANOTHER",
        Difficulty::Leggendaria => "LEGGENDARIA",
    }
}

#[cfg(test)]
fn grouped_u32(value: u32) -> String {
    grouped_u64(u64::from(value))
}

#[cfg(test)]
fn grouped_u64(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
fn supplemental_u32(value: &SupplementalResultValue<u32>) -> String {
    match value {
        SupplementalResultValue::Known { value } => grouped_u32(*value),
        SupplementalResultValue::NotDisplayed => "--".to_owned(),
        SupplementalResultValue::Unknown { .. } => "?".to_owned(),
    }
}

#[cfg(test)]
fn play_options_text(value: &PlayOptions) -> String {
    match value {
        PlayOptions::Known { values } if values.is_empty() => "none".to_owned(),
        PlayOptions::Known { values } => values
            .iter()
            .map(|value| match value {
                PlayOption::Random => "RANDOM",
                PlayOption::RRandom => "R-RANDOM",
                PlayOption::SRandom => "S-RANDOM",
                PlayOption::Mirror => "MIRROR",
                PlayOption::AutoScratch => "A-SCR",
                PlayOption::Legacy => "LEGACY",
            })
            .collect::<Vec<_>>()
            .join(","),
        PlayOptions::Unknown { reason } => {
            format!("unknown({})", format!("{reason:?}").to_ascii_lowercase())
        }
    }
}

#[cfg(test)]
fn previous_u32(value: &PreviousBestValue<u32>) -> String {
    match value {
        PreviousBestValue::Known { value } => grouped_u32(*value),
        PreviousBestValue::NotPlayed => "NO PLAY".to_owned(),
        PreviousBestValue::NotDisplayed => "--".to_owned(),
        PreviousBestValue::Unknown { .. } => "?".to_owned(),
    }
}

#[cfg(test)]
fn previous_text(value: &PreviousBestValue<String>) -> String {
    match value {
        PreviousBestValue::Known { value } => value.clone(),
        PreviousBestValue::NotPlayed => "NO PLAY".to_owned(),
        PreviousBestValue::NotDisplayed => "--".to_owned(),
        PreviousBestValue::Unknown { .. } => "?".to_owned(),
    }
}

#[cfg(test)]
pub(super) fn fitted_value(prefix: &str, value: &str, available_width: usize) -> String {
    let prefix_width = Line::raw(prefix).width();
    if prefix_width >= available_width {
        return prefix.to_owned();
    }
    let maximum = available_width - prefix_width;
    if Line::raw(value).width() <= maximum {
        return format!("{prefix}{value}");
    }
    let ellipsis_width = Line::raw("…").width();
    let mut truncated = String::new();
    for character in value.chars() {
        let mut candidate = truncated.clone();
        candidate.push(character);
        if Line::raw(&candidate).width().saturating_add(ellipsis_width) > maximum {
            break;
        }
        truncated.push(character);
    }
    format!("{prefix}{truncated}…")
}

#[cfg(test)]
#[path = "server/tests.rs"]
mod tests;
