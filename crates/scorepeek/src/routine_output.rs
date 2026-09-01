use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs::{self, DirBuilder};
use std::io::{self, BufWriter, IsTerminal as _, Write as _};
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

use crate::play_attempt::{
    PlayAttemptReason, PlayAttemptReducer, PlayAttemptScreen, PlayAttemptState,
};
use crate::recognition_live::screen_field_observer::{
    EvidenceFamily, JointEvidenceCandidate, JointEvidenceObservation,
};
use crate::run_event_artifact::{FinishOutcome as RunEventArtifactOutcome, RunEventArtifactWorker};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use scorepeek::catalog::{Difficulty, PlayType, ScorepeekSongId};
use scorepeek::recognition::{
    ParsedResultFields, PreviousBest, PreviousBestValue, ResultChartResolution, ResultJudgments,
    ResultPerformanceResolution, ResultTiming, SupplementalResultValue, resolve_result_performance,
};
use scorepeek::temporal_recognition::{
    MusicSelectTemporalState, MusicSelectTemporalTransitionReason, ResultTemporalState,
    TemporalFieldTransition,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn duration_us(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

const MAX_CLIENTS: usize = 8;
const EVENT_QUEUE_CAPACITY: usize = 64;
const RESULT_HISTORY_CAPACITY: usize = 32;
const SOCKET_NAME: &str = "observations-v4.sock";
const RUN_EVENT_SCHEMA: &str = "scorepeek-run-event-v4";
const NUMERIC_REQUIRED_OBSERVATIONS: u8 = 2;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunEvent {
    pub schema: String,
    #[serde(flatten)]
    pub kind: RunEventKind,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "the event schema remains flat and values cross an already bounded queue"
)]
pub enum RunEventKind {
    WatcherStarted {
        invocation_id: String,
        profile_sha256: String,
    },
    SessionStarted {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        capture_generation: u64,
        capture_profile_sha256: String,
        normalizer_artifact_sha256: String,
    },
    RawScreenObserved {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        semantic_episode_id: Option<u64>,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        unknown_reason: Option<String>,
    },
    SemanticScreenEpisodeChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: String,
        phase: SemanticEpisodePhase,
    },
    ScreenChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        #[serde(default)]
        screen_episode_id: u64,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
    },
    ScreenTick {
        #[serde(default)]
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: String,
    },
    FieldObservation {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        #[serde(default)]
        screen_episode_id: u64,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: String,
        fields: Value,
        result_song_resolution: Value,
        music_select_song_resolution: Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        parsed_result_fields: Option<ParsedResultFields>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result_chart_resolution: Option<ResultChartResolution>,
        #[serde(skip_serializing_if = "Option::is_none")]
        result_performance_resolution: Option<ResultPerformanceResolution>,
        #[serde(skip_serializing_if = "Option::is_none")]
        current_score_ocr_resolution: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        numeric_batch: Option<Value>,
        joint_evidence: JointEvidenceObservation,
        processing_timing: Value,
        song_resolution_presentation: Box<SongResolutionPresentation>,
    },
    ResultDetected {
        session_id: String,
        capture_generation: u64,
        source_sequence: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        song: Option<SongPresentation>,
        result: ResultDomainEvent,
    },
    TemporalResultChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_sequence: Option<u64>,
        transitions: Vec<TemporalFieldTransition>,
        state: ResultTemporalState<scorepeek::catalog::ScorepeekSongId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stable_song: Option<SongPresentation>,
    },
    TemporalMusicSelectChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_sequence: Option<u64>,
        reasons: Vec<MusicSelectTemporalTransitionReason>,
        state: MusicSelectTemporalState<scorepeek::catalog::ScorepeekSongId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        retained_song: Option<SongPresentation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        candidate_song: Option<SongPresentation>,
    },
    NumericResultChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        source_sequence: u64,
        state: NumericResultTemporalState,
        reason: NumericResultTransitionReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        event_suppression_reason: Option<NumericResultEventSuppressionReason>,
    },
    PlayAttemptChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        source_sequence: Option<u64>,
        state: PlayAttemptState,
    },
    ResolverStateChanged {
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        capture_generation: Option<u64>,
        screen_episode_id: u64,
        source_sequence: u64,
        scope: ResolverScope,
        state: ResolverResolutionState,
        #[serde(skip_serializing_if = "Option::is_none")]
        top: Option<ResolverHypothesisKey>,
        #[serde(skip_serializing_if = "Option::is_none")]
        runner_up: Option<ResolverHypothesisKey>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runner_song: Option<ResolverHypothesisKey>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runner_chart: Option<ResolverHypothesisKey>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        top_candidates: Vec<ResolverHypothesisKey>,
        support: u16,
        margin: u16,
        #[serde(default)]
        song_margin: u16,
        #[serde(default)]
        chart_margin: u16,
        selected_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
        runner_up_family_support: BTreeMap<EvidenceFamily, EvidenceContribution>,
        observation_count: u32,
    },
    SessionFinished {
        session_id: String,
        capture_generation: u64,
        outcome: String,
        report: Value,
    },
    WatcherStopped {
        invocation_id: String,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticEpisodePhase {
    Started,
    Suspended,
    Resumed,
    Closing,
    Finalized,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NumericResultTemporalState {
    Unknown,
    Pending { observations: u8 },
    Accepted,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericResultTransitionReason {
    Incomplete,
    CandidateStarted,
    CandidateRepeated,
    Accepted,
    Conflict,
    ChronologyReset,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericResultEventSuppressionReason {
    NumericNotAccepted,
    SessionUnavailable,
    ResultSongNotStable,
    ClearTypeNotStable,
    PlayAttemptNotAccepted,
    LinkageConflict,
    AlreadyEmitted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResultDomainEvent {
    pub contract: String,
    pub attempt_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_attempt_id: Option<u64>,
    pub scorepeek_song_id: ScorepeekSongId,
    pub play_side: String,
    pub play_mode: String,
    pub play_type: PlayType,
    pub difficulty: Difficulty,
    pub level: u8,
    pub notes: u32,
    pub current_score: u32,
    pub clear_type: String,
    pub judgments: ResultJudgments,
    pub miss_count: SupplementalResultValue<u32>,
    pub timing: ResultTiming,
    pub combo_break: SupplementalResultValue<u32>,
    pub previous_best: PreviousBest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NumericResultView {
    song_id: ScorepeekSongId,
    clear_type: String,
    chart: scorepeek::catalog::Chart,
    current_score: u32,
    performance: ResultPerformanceResolution,
    source_sequence: u64,
}

#[derive(Clone, Debug)]
struct PendingNumericResult {
    view: NumericResultView,
    observations: u8,
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
        && left.performance == right.performance
}

fn candidate_song_presentation(candidate: &JointEvidenceCandidate) -> SongPresentation {
    SongPresentation {
        scorepeek_song_id: candidate.song_id,
        display_titles: candidate.display_titles.clone(),
        artist: candidate.artist.clone(),
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SongPresentation {
    pub scorepeek_song_id: scorepeek::catalog::ScorepeekSongId,
    pub display_titles: Vec<String>,
    pub artist: String,
}

const EVIDENCE_FAMILY_CAP: u16 = 300;
const JOINT_ACCEPT_SUPPORT: u16 = 260;
const JOINT_ACCEPT_MARGIN: u16 = 50;
const SELECTION_CHANGE_MARGIN: u16 = 120;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolverResolutionState {
    Unresolved,
    SongProjected,
    JointCandidate,
    AcceptedJoint,
    Conflict,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct JointKey {
    song_id: ScorepeekSongId,
    chart_key: scorepeek::catalog::ChartKey,
}

#[derive(Clone, Debug)]
struct AccumulatedHypothesis {
    candidate: JointEvidenceCandidate,
    family_support: BTreeMap<EvidenceFamily, u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceContribution {
    raw: u64,
    normalized: u16,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolverScope {
    SelectionIncumbent,
    SelectionSuccessor,
    Result,
    AttemptJoint,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ResolverHypothesisKey {
    song_id: ScorepeekSongId,
    chart: scorepeek::catalog::ChartKey,
}

impl ResolverHypothesisKey {
    fn from_candidate(candidate: &JointEvidenceCandidate) -> Self {
        Self {
            song_id: candidate.song_id,
            chart: candidate.chart.key,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResolverTransitionIdentity {
    state: ResolverResolutionState,
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
    select_difficulty_support: BTreeMap<Difficulty, u64>,
    result_chart_factors: BTreeMap<ResultChartFactor, u64>,
    first_observation_ms: Option<u64>,
    last_observation_ms: Option<u64>,
    observation_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ResultChartFactor {
    difficulty: Option<Difficulty>,
    notes: Option<u32>,
    level: Option<u8>,
}

#[derive(Clone, Debug)]
struct HypothesisSummary {
    state: ResolverResolutionState,
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
    fn observe(
        &mut self,
        monotonic_ms: u64,
        observation: &JointEvidenceObservation,
        select_difficulty: Option<Difficulty>,
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
            let value = self
                .select_difficulty_support
                .entry(difficulty)
                .or_default();
            *value = value.saturating_add(50);
        }
        if let Some(factor) = result_chart_factor {
            let value = self.result_chart_factors.entry(factor).or_default();
            *value = value.saturating_add(1);
        }
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
        for (difficulty, support) in &other.select_difficulty_support {
            let value = self
                .select_difficulty_support
                .entry(*difficulty)
                .or_default();
            *value = value.saturating_add(*support);
        }
        for (factor, observations) in &other.result_chart_factors {
            let value = self.result_chart_factors.entry(*factor).or_default();
            *value = value.saturating_add(*observations);
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "hierarchical projection keeps normalization and both runner dimensions together"
    )]
    fn summary(&self) -> HypothesisSummary {
        let mut expanded = self.candidates.clone();
        for accumulated in expanded.values_mut() {
            let select_chart = self
                .select_difficulty_support
                .get(&accumulated.candidate.chart.key.difficulty)
                .copied()
                .unwrap_or(0);
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
                        (
                            *family,
                            EvidenceContribution {
                                raw: *raw,
                                normalized,
                            },
                        )
                    })
                    .collect::<BTreeMap<_, _>>();
                let support = family_support
                    .values()
                    .fold(0_u16, |total, value| total.saturating_add(value.normalized));
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
        let chart_is_resolved = runner_chart.is_none() || chart_margin >= JOINT_ACCEPT_MARGIN;
        let state = if support >= JOINT_ACCEPT_SUPPORT && song_is_resolved && chart_is_resolved {
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
    pending_difficulty_support: BTreeMap<Difficulty, u64>,
}

impl SelectionEpochTracker {
    fn observe(
        &mut self,
        monotonic_ms: u64,
        evidence: &JointEvidenceObservation,
        difficulty: Option<Difficulty>,
    ) {
        let credible = credible_song_set(evidence);
        if credible.is_empty() {
            if let Some(difficulty) = difficulty {
                let support = self
                    .pending_difficulty_support
                    .entry(difficulty)
                    .or_default();
                *support = support.saturating_add(50);
            }
            return;
        }
        if self.incumbent.observation_count == 0 {
            merge_pending_difficulty(&mut self.incumbent, &mut self.pending_difficulty_support);
            self.incumbent
                .observe(monotonic_ms, evidence, difficulty, None);
            self.incumbent_songs.extend(credible);
            return;
        }
        if !self.incumbent_songs.is_disjoint(&credible) {
            merge_pending_difficulty(&mut self.incumbent, &mut self.pending_difficulty_support);
            self.incumbent
                .observe(monotonic_ms, evidence, difficulty, None);
            self.incumbent_songs.extend(credible);
            self.successor = HypothesisAccumulator::default();
            self.successor_songs.clear();
            return;
        }
        if !self.successor_songs.is_empty() && self.successor_songs.is_disjoint(&credible) {
            self.successor = HypothesisAccumulator::default();
            self.successor_songs.clear();
        }
        merge_pending_difficulty(&mut self.successor, &mut self.pending_difficulty_support);
        self.successor
            .observe(monotonic_ms, evidence, difficulty, None);
        self.successor_songs.extend(credible);
        if self.successor.summary().support >= SELECTION_CHANGE_MARGIN {
            self.incumbent = std::mem::take(&mut self.successor);
            self.incumbent_songs = std::mem::take(&mut self.successor_songs);
        }
    }

    fn handoff(&self) -> HypothesisAccumulator {
        if self.successor.observation_count > 0 {
            self.successor.clone()
        } else {
            self.incumbent.clone()
        }
    }
}

fn merge_pending_difficulty(
    accumulator: &mut HypothesisAccumulator,
    pending: &mut BTreeMap<Difficulty, u64>,
) {
    for (difficulty, support) in std::mem::take(pending) {
        let target = accumulator
            .select_difficulty_support
            .entry(difficulty)
            .or_default();
        *target = target.saturating_add(support);
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SongResolutionPresentation {
    Accepted {
        reason: Option<Value>,
        selected: SongPresentation,
        runner_up: SongPresentation,
        evidence_summary: String,
    },
    Unknown {
        reason: Value,
        selected: Option<SongPresentation>,
        runner_up: Option<SongPresentation>,
        evidence_summary: Option<String>,
    },
}

impl RunEvent {
    pub fn to_value(&self) -> Result<Value, String> {
        serde_json::to_value(self)
            .map_err(|error| format!("run event serialization failed: {error}"))
    }

    pub fn from_value(value: Value) -> Result<Self, String> {
        serde_json::from_value(value)
            .map_err(|error| format!("run event contract validation failed: {error}"))
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct RunViewState {
    invocation_id: String,
    profile_sha256: String,
    recording: &'static str,
    watcher_state: String,
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
    result_history: VecDeque<ResultHistoryEntry>,
    result_count: u64,
    #[serde(skip)]
    stable_result_song: Option<SongPresentation>,
    latest_report: Option<Value>,
    status_recording: &'static str,
    next_channel_sequence: u64,
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
    local: Option<ResolverNodeSnapshot>,
    successor: Option<ResolverNodeSnapshot>,
    attempt: Option<AttemptNodeSnapshot>,
    gate: String,
    gates: Vec<GateSnapshot>,
    raw_fields: Vec<(String, String)>,
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
    family_contributions: Vec<String>,
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
        Self {
            invocation_id,
            profile_sha256,
            recording: if recording_enabled {
                "enabled"
            } else {
                "disabled"
            },
            watcher_state: "starting".to_owned(),
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
            result_history: VecDeque::with_capacity(RESULT_HISTORY_CAPACITY),
            result_count: 0,
            stable_result_song: None,
            latest_report: None,
            status_recording: if recording_enabled {
                "ready"
            } else {
                "disabled"
            },
            next_channel_sequence: 1,
            message: "initializing".to_owned(),
            resolver: ResolverDebugSnapshot::default(),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn reduce(&mut self, event: &RunEvent, serialized: &Value) {
        match &event.kind {
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
                self.raw_screen = None;
                self.latest_observation = None;
                self.latest_stabilized_result = None;
                self.latest_temporal_music_select = None;
                self.latest_play_attempt = None;
                self.latest_numeric_result = None;
                self.stable_result_song = None;
                self.latest_report = None;
                "Gamescope session admitted".clone_into(&mut self.message);
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
            RunEventKind::ScreenTick { .. } | RunEventKind::ResolverStateChanged { .. } => {}
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
            RunEventKind::ResultDetected {
                session_id,
                capture_generation,
                source_sequence,
                song,
                result,
            } => {
                self.latest_result_detected = Some(serialized.clone());
                self.result_count = self.result_count.saturating_add(1);
                let song = song
                    .as_ref()
                    .filter(|song| song.scorepeek_song_id == result.scorepeek_song_id)
                    .cloned();
                if self.result_history.len() == RESULT_HISTORY_CAPACITY {
                    self.result_history.pop_front();
                }
                self.result_history.push_back(ResultHistoryEntry {
                    ordinal: self.result_count,
                    session_id: session_id.clone(),
                    capture_generation: *capture_generation,
                    source_sequence: *source_sequence,
                    song,
                    result: result.clone(),
                });
            }
            RunEventKind::SessionFinished {
                outcome, report, ..
            } => {
                "session_finished".clone_into(&mut self.watcher_state);
                self.active_session_id = None;
                self.capture_generation = None;
                self.current_screen = None;
                self.raw_screen = None;
                self.latest_report = Some(report.clone());
                self.message = format!("session finished: {outcome}");
            }
            RunEventKind::WatcherStopped { .. } => {
                "stopped".clone_into(&mut self.watcher_state);
                self.active_session_id = None;
                self.capture_generation = None;
                self.current_screen = None;
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
}

impl ChannelHealth {
    fn value(&self) -> Value {
        json!({
            "status": if self.server_failed.load(Ordering::Acquire) { "degraded" } else { "ready" },
            "connected_clients": self.connected_clients.load(Ordering::Acquire),
            "dropped_events": self.dropped_events.load(Ordering::Acquire),
            "disconnected_clients": self.disconnected_clients.load(Ordering::Acquire),
        })
    }
}

struct ObservationChannel {
    sender: SyncSender<Vec<u8>>,
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

impl ObservationChannel {
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
            .map_err(|error| format!("observation socket could not be bound: {error}"))?;
        let metadata = socket_path
            .symlink_metadata()
            .map_err(|error| format!("observation socket could not be inspected: {error}"))?;
        let socket_identity = (metadata.dev(), metadata.ino());
        let mut path_guard = SocketPathGuard::new(socket_path.clone(), socket_identity);
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("observation socket permissions could not be set: {error}"))?;
        listener.set_nonblocking(true).map_err(|error| {
            format!("observation socket could not be made nonblocking: {error}")
        })?;
        let (sender, receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(EVENT_QUEUE_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let health = Arc::new(ChannelHealth::default());
        let thread_stop = Arc::clone(&stop);
        let thread_health = Arc::clone(&health);
        let thread = thread::Builder::new()
            .name("scorepeek-observation-socket".to_owned())
            .spawn(move || {
                let mut clients = Vec::new();
                loop {
                    accept_clients(&listener, &state, &thread_health, &mut clients);
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(bytes) => broadcast(&bytes, &thread_health, &mut clients),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if thread_stop.load(Ordering::Acquire) {
                                break;
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                thread_health.connected_clients.store(0, Ordering::Release);
            })
            .map_err(|error| format!("observation socket worker could not start: {error}"))?;
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

    fn publish(&self, event: &Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(event)
            .map_err(|error| format!("run observation serialization failed: {error}"))?;
        bytes.push(b'\n');
        try_send_event(&self.sender, &self.health, bytes);
        Ok(())
    }
}

fn try_send_event(sender: &SyncSender<Vec<u8>>, health: &ChannelHealth, bytes: Vec<u8>) {
    match sender.try_send(bytes) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            health.dropped_events.fetch_add(1, Ordering::AcqRel);
        }
        Err(TrySendError::Disconnected(_)) => {
            health.server_failed.store(true, Ordering::Release);
        }
    }
}

impl Drop for ObservationChannel {
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
        Ok(_) => Err("observation socket directory is not a directory".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).map_err(|error| {
                format!("observation socket directory could not be created: {error}")
            })
        }
        Err(error) => Err(format!(
            "observation socket directory could not be inspected: {error}"
        )),
    }
}

fn remove_stale_socket(path: &Path) -> Result<(), String> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_socket() => match UnixStream::connect(path) {
            Ok(_) => Err("observation socket is already active".to_owned()),
            Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => fs::remove_file(path)
                .map_err(|error| format!("stale observation socket could not be removed: {error}")),
            Err(error) => Err(format!(
                "observation socket liveness could not be determined: {error}"
            )),
        },
        Ok(_) => Err("observation socket path contains a non-socket entry".to_owned()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "observation socket path could not be inspected: {error}"
        )),
    }
}

fn accept_clients(
    listener: &UnixListener,
    state: &Arc<Mutex<RunViewState>>,
    health: &ChannelHealth,
    clients: &mut Vec<UnixStream>,
) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if clients.len() >= MAX_CLIENTS || stream.set_nonblocking(true).is_err() {
                    health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
                clients.push(stream);
                health
                    .connected_clients
                    .store(clients.len(), Ordering::Release);
                let Some(snapshot) = snapshot_bytes(state, health) else {
                    clients.pop();
                    health
                        .connected_clients
                        .store(clients.len(), Ordering::Release);
                    health.server_failed.store(true, Ordering::Release);
                    continue;
                };
                if clients.last_mut().unwrap().write_all(&snapshot).is_err() {
                    clients.pop();
                    health
                        .connected_clients
                        .store(clients.len(), Ordering::Release);
                    health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => {
                health.server_failed.store(true, Ordering::Release);
                break;
            }
        }
    }
}

fn snapshot_bytes(state: &Arc<Mutex<RunViewState>>, health: &ChannelHealth) -> Option<Vec<u8>> {
    let state = state.lock().ok()?.clone();
    let mut bytes = serde_json::to_vec(&json!({
            "schema": "scorepeek-run-observation-snapshot-v4",
        "state": state,
        "channel": health.value(),
    }))
    .ok()?;
    bytes.push(b'\n');
    Some(bytes)
}

fn broadcast(bytes: &[u8], health: &ChannelHealth, clients: &mut Vec<UnixStream>) {
    clients.retain_mut(|client| {
        if client.write_all(bytes).is_ok() {
            true
        } else {
            health.disconnected_clients.fetch_add(1, Ordering::AcqRel);
            false
        }
    });
    health
        .connected_clients
        .store(clients.len(), Ordering::Release);
}

enum Display {
    Tui(TerminalGuard),
    Plain {
        output: BufWriter<io::Stdout>,
        last_line: Option<String>,
    },
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self, String> {
        let mut output = io::stdout();
        enter_alternate_screen(&mut output)?;
        let backend = CrosstermBackend::new(output);
        let terminal = Terminal::new(backend).map_err(|error| {
            let mut restore = io::stdout();
            let _ = restore.write_all(b"\x1b[?25h\x1b[?1049l");
            format!("terminal could not initialize TUI rendering: {error}")
        })?;
        Ok(Self { terminal })
    }

    fn draw(
        &mut self,
        state: &RunViewState,
        socket_path: &Path,
        health: &ChannelHealth,
    ) -> Result<(), String> {
        self.terminal
            .draw(|frame| render(frame, state, socket_path, health))
            .map(|_| ())
            .map_err(|error| format!("TUI output failed: {error}"))
    }
}

fn enter_alternate_screen(output: &mut impl io::Write) -> Result<(), String> {
    if let Err(error) = output
        .write_all(b"\x1b[?1049h\x1b[?25l")
        .and_then(|()| output.flush())
    {
        let _ = output.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = output.flush();
        return Err(format!("terminal could not enter TUI mode: {error}"));
    }
    Ok(())
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut output = io::stdout();
        let _ = output.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = output.flush();
    }
}

#[allow(clippy::struct_excessive_bools)]
pub struct RoutineOutput {
    state: Arc<Mutex<RunViewState>>,
    channel: ObservationChannel,
    display: Display,
    next_sequence: u64,
    engine: ResolverEngine,
    pending_numeric_result: Option<PendingNumericResult>,
    accepted_numeric_result: Option<NumericResultView>,
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
    event_store: Option<PathBuf>,
    event_worker: Option<RunEventArtifactWorker>,
    completed_event_artifact: Option<RunEventArtifactOutcome>,
    timing_active: bool,
    output_us: u64,
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
            top: summary
                .selected
                .as_ref()
                .map(ResolverHypothesisKey::from_candidate),
            runner_up: summary
                .runner_up
                .as_ref()
                .map(ResolverHypothesisKey::from_candidate),
            runner_song: summary
                .runner_song
                .as_ref()
                .map(ResolverHypothesisKey::from_candidate),
            runner_chart: summary
                .runner_chart
                .as_ref()
                .map(ResolverHypothesisKey::from_candidate),
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
                top: identity.top,
                runner_up: identity.runner_up,
                runner_song: identity.runner_song,
                runner_chart: identity.runner_chart,
                top_candidates: summary
                    .top_candidates
                    .iter()
                    .map(ResolverHypothesisKey::from_candidate)
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

    pub fn start(
        invocation_id: String,
        profile_sha256: String,
        recording_enabled: bool,
        event_store: Option<PathBuf>,
    ) -> Result<Self, String> {
        let state = Arc::new(Mutex::new(RunViewState::new(
            invocation_id,
            profile_sha256,
            recording_enabled,
        )));
        let channel = ObservationChannel::start(Arc::clone(&state))?;
        let display = if io::stdout().is_terminal() {
            Display::Tui(TerminalGuard::new()?)
        } else {
            Display::Plain {
                output: BufWriter::new(io::stdout()),
                last_line: None,
            }
        };
        let mut output = Self {
            state,
            channel,
            display,
            next_sequence: 1,
            engine: ResolverEngine::default(),
            resolver_transitions: BTreeMap::new(),
            pending_numeric_result: None,
            accepted_numeric_result: None,
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
            event_store,
            event_worker: None,
            completed_event_artifact: None,
            timing_active: false,
            output_us: 0,
        };
        output.refresh()?;
        Ok(output)
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
            RunEventKind::SessionStarted { session_id, .. } => {
                self.engine.play_attempt.reset_session();
                self.reset_numeric_result();
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
                self.completed_event_artifact = None;
                self.clear_resolver_field_observation()?;
                self.event_worker =
                    self.event_store.as_deref().zip(session_id.as_deref()).map(
                        |(store, session_id)| RunEventArtifactWorker::start(store, session_id),
                    );
                self.publish_one(event)
            }
            RunEventKind::WatcherStopped { .. } => self.publish_watcher_stopped(event),
            RunEventKind::FieldObservation { .. } => self.publish_field_observation(event),
            RunEventKind::RawScreenObserved {
                sequence,
                monotonic_end_ms,
                ..
            } => {
                self.publish_one(event)?;
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
            | RunEventKind::TemporalResultChanged { .. }
            | RunEventKind::TemporalMusicSelectChanged { .. }
            | RunEventKind::NumericResultChanged { .. }
            | RunEventKind::PlayAttemptChanged { .. }
            | RunEventKind::ResolverStateChanged { .. }
            | RunEventKind::ResultDetected { .. } => self.publish_one(event),
        }
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
        match phase {
            SemanticEpisodePhase::Started => {
                self.semantic_episode_suspended = false;
                self.clear_resolver_field_observation()?;
                let legacy = RunEvent {
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
                self.publish_screen_change(&legacy, false)
            }
            SemanticEpisodePhase::Suspended => {
                self.semantic_episode_suspended = true;
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
                } else if screen == "result" {
                    self.finalize_result_attempt(
                        session_id.clone(),
                        *capture_generation,
                        *sequence,
                    )?;
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
        let rejection = match (
            self.engine.provisional_joint.as_ref(),
            self.accepted_numeric_result.as_ref(),
        ) {
            (None, _) => Some(PlayAttemptReason::JointIdentityUnresolved),
            (Some(_), None) => Some(PlayAttemptReason::ResultEvidenceUnresolved),
            (Some(joint), Some(numeric)) if !joint_matches_numeric(joint, numeric) => {
                Some(PlayAttemptReason::LinkageConflict)
            }
            (Some(_), Some(_)) => None,
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
        self.try_emit_result(session_id, capture_generation, sequence)
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
        self.publish_one(event)?;
        self.completed_event_artifact =
            self.event_worker.take().map(RunEventArtifactWorker::finish);
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
        self.engine.result_hypotheses.observe(
            monotonic_end_ms,
            joint_evidence,
            None,
            parsed_result_fields.map(result_chart_factor),
        );
        let result_summary = self.engine.result_hypotheses.summary();
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
                if let Some((state, reason)) = self.observe_numeric_result(
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
                            state,
                            reason,
                            event_suppression_reason: self
                                .numeric_event_suppression_reason(session_id, capture_generation),
                        },
                    })?;
                }
            }
        }
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
    ) -> Option<(NumericResultTemporalState, NumericResultTransitionReason)> {
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
            return self.pending_numeric_result.take().map(|_| {
                (
                    NumericResultTemporalState::Unknown,
                    NumericResultTransitionReason::Incomplete,
                )
            });
        };
        let Some(current_score) = parsed.current_score.known().copied() else {
            return self.pending_numeric_result.take().map(|_| {
                (
                    NumericResultTemporalState::Unknown,
                    NumericResultTransitionReason::Incomplete,
                )
            });
        };
        let performance = resolve_result_performance(parsed, candidate.chart.notes, current_score);
        if !matches!(performance, ResultPerformanceResolution::Accepted { .. }) {
            return self.pending_numeric_result.take().map(|_| {
                (
                    NumericResultTemporalState::Unknown,
                    NumericResultTransitionReason::Incomplete,
                )
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
        if let Some(accepted) = &self.accepted_numeric_result {
            if same_numeric_tuple(accepted, &view) {
                return None;
            }
            self.accepted_numeric_result = None;
        }
        let had_conflict = self
            .pending_numeric_result
            .as_ref()
            .is_some_and(|pending| !same_numeric_tuple(&pending.view, &view));
        match &mut self.pending_numeric_result {
            Some(pending) if same_numeric_tuple(&pending.view, &view) => {
                pending.observations = pending.observations.saturating_add(1);
                pending.view.source_sequence = sequence;
                if pending.observations >= NUMERIC_REQUIRED_OBSERVATIONS {
                    self.accepted_numeric_result = Some(pending.view.clone());
                    self.pending_numeric_result = None;
                    Some((
                        NumericResultTemporalState::Accepted,
                        NumericResultTransitionReason::Accepted,
                    ))
                } else {
                    Some((
                        NumericResultTemporalState::Pending {
                            observations: pending.observations,
                        },
                        NumericResultTransitionReason::CandidateRepeated,
                    ))
                }
            }
            _ => {
                self.pending_numeric_result = Some(PendingNumericResult {
                    view,
                    observations: 1,
                });
                Some((
                    NumericResultTemporalState::Pending { observations: 1 },
                    if chronology_reset {
                        NumericResultTransitionReason::ChronologyReset
                    } else if had_conflict {
                        NumericResultTransitionReason::Conflict
                    } else {
                        NumericResultTransitionReason::CandidateStarted
                    },
                ))
            }
        }
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
        if !self
            .engine
            .provisional_joint
            .as_ref()
            .is_some_and(|candidate| joint_matches_numeric(candidate, numeric))
        {
            return Ok(());
        }
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
        let result = ResultDomainEvent {
            contract: "scorepeek-result-detected-v2".to_owned(),
            attempt_id: accepted_attempt.attempt_id,
            parent_attempt_id: accepted_attempt.parent_attempt_id,
            scorepeek_song_id: numeric.song_id,
            play_side: "one_player".to_owned(),
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
        };
        let source_sequence = numeric.source_sequence.max(fallback_sequence);
        let emitted_attempt_id = accepted_attempt.attempt_id;
        self.publish_one(&RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::ResultDetected {
                session_id,
                capture_generation,
                source_sequence,
                song: self
                    .engine
                    .provisional_joint
                    .as_ref()
                    .map(candidate_song_presentation),
                result,
            },
        })?;
        self.emitted_attempt_ids.insert(emitted_attempt_id);
        Ok(())
    }

    fn reset_numeric_result(&mut self) {
        self.pending_numeric_result = None;
        self.accepted_numeric_result = None;
        self.last_numeric_sequence = None;
        self.last_numeric_monotonic_ms = None;
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
        self.engine.selection_epochs.observe(
            monotonic_end_ms,
            joint_evidence,
            selected_difficulty(fields),
        );
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
        self.screen_episode_id = *screen_episode_id;
        self.screen_episode_started_ms = Some(*monotonic_end_ms);
        self.screen_episode_last_ms = Some(*monotonic_end_ms);
        let close_result_resolver = screen != "result" && self.result_resolver_active;
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
            self.engine.selection_epochs = SelectionEpochTracker::default();
            self.engine.retained_select = HypothesisAccumulator::default();
            self.resolver_transitions
                .remove(&ResolverScope::SelectionIncumbent);
            self.resolver_transitions
                .remove(&ResolverScope::SelectionSuccessor);
            selection_screen_attempt_update = self.engine.play_attempt.observe_selection_screen();
        }
        if screen == "result" {
            self.result_resolver_active = true;
            self.engine.result_hypotheses = HypothesisAccumulator::default();
            self.engine.provisional_joint = None;
            self.resolver_transitions.remove(&ResolverScope::Result);
            self.resolver_transitions
                .remove(&ResolverScope::AttemptJoint);
            self.numeric_evidence.clear();
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
            self.reset_numeric_result();
            self.numeric_evidence.clear();
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
        self.publish_one(event)?;
        if let Some(state) = self.engine.play_attempt.finish_session() {
            self.publish_play_attempt_update(
                Some(session_id.clone()),
                Some(*capture_generation),
                None,
                state,
            )?;
        }
        self.completed_event_artifact =
            self.event_worker.take().map(RunEventArtifactWorker::finish);
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
            "accepted: result_detected emitted"
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
                if matches!(attempt.phase, crate::play_attempt::PlayAttemptPhase::Completed)
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
        state.resolver = ResolverDebugSnapshot {
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
            local,
            successor,
            attempt,
            gate,
            gates,
            raw_fields: retained_raw,
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
        let output_started = Instant::now();
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        let mut value = bounded_run_event_value(event)?;
        if let Some(object) = value.as_object_mut() {
            object.insert("channel_sequence".to_owned(), sequence.into());
        }
        if let Some(worker) = &mut self.event_worker {
            worker.try_record(&value);
        }
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            state.reduce(event, &value);
            state.next_channel_sequence = self.next_sequence;
        }
        self.channel.publish(&value)?;
        if self.timing_active {
            self.output_us = self
                .output_us
                .saturating_add(duration_us(output_started.elapsed()));
        }
        self.refresh()
    }

    pub fn take_completed_event_artifact(&mut self) -> Option<RunEventArtifactOutcome> {
        self.completed_event_artifact.take()
    }

    pub fn watcher_state(
        &mut self,
        state_name: &str,
        session_id: Option<&str>,
        generation: Option<u64>,
        message: &str,
    ) -> Result<(), String> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "run view state lock was poisoned".to_owned())?;
            state_name.clone_into(&mut state.watcher_state);
            state.active_session_id = session_id.map(ToOwned::to_owned);
            state.capture_generation = generation;
            message.clone_into(&mut state.message);
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
        let state = self
            .state
            .lock()
            .map_err(|_| "run view state lock was poisoned".to_owned())?
            .clone();
        let result = match &mut self.display {
            Display::Tui(terminal) => {
                terminal.draw(&state, &self.channel.socket_path, &self.channel.health)
            }
            Display::Plain { output, last_line } => {
                let line = plain_status_line(&state, &self.channel.health);
                if last_line.as_deref() != Some(&line) {
                    writeln!(output, "{line}")
                        .and_then(|()| output.flush())
                        .map_err(|error| format!("plain run output failed: {error}"))?;
                    *last_line = Some(line);
                }
                Ok(())
            }
        };
        if self.timing_active {
            self.output_us = self
                .output_us
                .saturating_add(duration_us(output_started.elapsed()));
        }
        result
    }
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

fn result_chart_factor(fields: &ParsedResultFields) -> ResultChartFactor {
    ResultChartFactor {
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
        family_contributions: family_contribution_labels(&summary.selected_family_support),
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
                contribution.normalized
            )
        })
        .collect::<Vec<_>>();
    values.sort();
    values
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
    let keys = [
        "title",
        "central_title",
        "active_list_title",
        "artist",
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

fn elapsed_seconds(now_ms: u64, started_ms: Option<u64>) -> u64 {
    started_ms.map_or(0, |started| now_ms.saturating_sub(started) / 1_000)
}

fn plain_status_line(state: &RunViewState, health: &ChannelHealth) -> String {
    let channel = health.value();
    format!(
        "scorepeek: state={} sessions={} session={} generation={} channel={} clients={} dropped={} disconnected={} message={}",
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
        state.message,
    )
}

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
            Constraint::Length(9),
            Constraint::Min(0),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(fixed_watcher_lines(state, health)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(watcher_color(state, health)))
                .title("Watcher"),
        ),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new(fixed_domain_lines(state, available_width)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(
                    Style::default().fg(if state.latest_result_detected.is_some() {
                        Color::Green
                    } else {
                        Color::DarkGray
                    }),
                )
                .title("Latest domain"),
        ),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(resolver_lines(&state.resolver, available_width))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(resolver_color(&state.resolver)))
                    .title("Resolver"),
            ),
        rows[2],
    );
}

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
            "recording={}  channel={} clients={} drop={}  {}",
            state.status_recording,
            channel["status"].as_str().unwrap_or("degraded"),
            channel["connected_clients"].as_u64().unwrap_or(0),
            channel["dropped_events"].as_u64().unwrap_or(0),
            state.message,
        )),
    ]
}

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

const fn gate_color(state: GateState) -> Color {
    match state {
        GateState::Accepted => Color::Green,
        GateState::Pending => Color::Yellow,
        GateState::Failed => Color::Red,
        GateState::Inactive => Color::DarkGray,
    }
}

const fn gate_suffix(state: GateState) -> &'static str {
    match state {
        GateState::Accepted => "✓",
        GateState::Pending => "…",
        GateState::Failed => "✗",
        GateState::Inactive => "–",
    }
}

fn fixed_domain_lines(state: &RunViewState, available_width: usize) -> Vec<Line<'static>> {
    let Some(entry) = state.result_history.back() else {
        return vec![Line::from(Span::styled(
            "No accepted scorepeek-result-detected-v2 event yet",
            Style::default().fg(Color::DarkGray),
        ))];
    };
    let mut lines = expanded_result_history_entry_lines(entry, available_width);
    lines.truncate(7);
    lines
}

#[allow(
    clippy::too_many_lines,
    reason = "the fixed ten-line debug tree keeps semantic styles adjacent to their typed values"
)]
fn resolver_lines(snapshot: &ResolverDebugSnapshot, available_width: usize) -> Vec<Line<'static>> {
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
    lines.push(Line::from(fitted_value(
        "│  TYPED ",
        &raw.iter()
            .skip(3)
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join(" "),
        available_width,
    )));
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

fn expanded_result_history_entry_lines(
    entry: &ResultHistoryEntry,
    available_width: usize,
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
                format!("#{} {}", entry.ordinal, result.clear_type),
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
            entry.song.as_ref().map_or("-", |song| song.artist.as_str()),
            available_width,
        )),
        Line::from(format!(
            "attempt=#{} parent=#{}  notes={} side={} mode={} sequence={}",
            result.attempt_id,
            result
                .parent_attempt_id
                .map_or_else(|| "-".to_owned(), |value| format!("{value}")),
            grouped_u32(result.notes),
            result.play_side,
            result.play_mode,
            entry.source_sequence,
        )),
    ]
}

const fn clear_type_color(clear_type: &str) -> Color {
    match clear_type.as_bytes() {
        b"FAILED" => Color::Red,
        b"ASSIST CLEAR" | b"EASY CLEAR" => Color::Yellow,
        b"CLEAR" | b"HARD CLEAR" | b"EXH-CLEAR" | b"F-COMBO" => Color::Green,
        _ => Color::White,
    }
}

const fn difficulty_color(difficulty: Difficulty) -> Color {
    match difficulty {
        Difficulty::Beginner => Color::Green,
        Difficulty::Normal => Color::Blue,
        Difficulty::Hyper => Color::Yellow,
        Difficulty::Another => Color::Red,
        Difficulty::Leggendaria => Color::Magenta,
    }
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

fn grouped_u32(value: u32) -> String {
    grouped_u64(u64::from(value))
}

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

fn supplemental_u32(value: &SupplementalResultValue<u32>) -> String {
    match value {
        SupplementalResultValue::Known { value } => grouped_u32(*value),
        SupplementalResultValue::NotDisplayed => "--".to_owned(),
        SupplementalResultValue::Unknown { .. } => "?".to_owned(),
    }
}

fn previous_u32(value: &PreviousBestValue<u32>) -> String {
    match value {
        PreviousBestValue::Known { value } => grouped_u32(*value),
        PreviousBestValue::NotPlayed => "NO PLAY".to_owned(),
        PreviousBestValue::NotDisplayed => "--".to_owned(),
        PreviousBestValue::Unknown { .. } => "?".to_owned(),
    }
}

fn previous_text(value: &PreviousBestValue<String>) -> String {
    match value {
        PreviousBestValue::Known { value } => value.clone(),
        PreviousBestValue::NotPlayed => "NO PLAY".to_owned(),
        PreviousBestValue::NotDisplayed => "--".to_owned(),
        PreviousBestValue::Unknown { .. } => "?".to_owned(),
    }
}

fn fitted_value(prefix: &str, value: &str, available_width: usize) -> String {
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
mod tests {
    use std::io::{BufRead as _, BufReader, Write};
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    use ratatui::backend::TestBackend;
    use scorepeek::recognition::ResultFieldValue;

    use super::*;

    fn state() -> Arc<Mutex<RunViewState>> {
        Arc::new(Mutex::new(RunViewState::new(
            "invocation-1".to_owned(),
            "a".repeat(64),
            true,
        )))
    }

    fn test_output(state: Arc<Mutex<RunViewState>>, channel: ObservationChannel) -> RoutineOutput {
        RoutineOutput {
            state,
            channel,
            display: Display::Plain {
                output: BufWriter::new(io::stdout()),
                last_line: None,
            },
            next_sequence: 1,
            engine: ResolverEngine::default(),
            pending_numeric_result: None,
            accepted_numeric_result: None,
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
            resolver_transitions: BTreeMap::new(),
            attempt_started_ms: None,
            attempt_phase_started_ms: None,
            event_store: None,
            event_worker: None,
            completed_event_artifact: None,
            timing_active: false,
            output_us: 0,
        }
    }

    fn disconnected_test_channel() -> ObservationChannel {
        let (sender, receiver) = std::sync::mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        drop(receiver);
        ObservationChannel {
            sender,
            stop: Arc::new(AtomicBool::new(false)),
            health: Arc::new(ChannelHealth::default()),
            thread: None,
            socket_path: PathBuf::new(),
            socket_identity: (0, 0),
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the fixture intentionally spells out one complete accepted result contract"
    )]
    fn accepted_result_event(sequence: u64) -> RunEvent {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        RunEvent {
            schema: "scorepeek-run-event-v3".to_owned(),
            kind: RunEventKind::FieldObservation {
                session_id: Some("invocation-1-session-1".to_owned()),
                capture_generation: Some(1),
                screen_episode_id: 0,
                sequence,
                monotonic_start_ms: sequence.saturating_mul(100),
                monotonic_end_ms: sequence.saturating_mul(100).saturating_add(25),
                screen: "result".to_owned(),
                fields: json!({
                    "title": "OCR TITLE",
                    "artist": "OCR ARTIST",
                    "clear_type": "CLEAR",
                    "clear_type_ocr": "CLEAR"
                }),
                result_song_resolution: json!({ "status": "accepted" }),
                music_select_song_resolution: Value::Null,
                parsed_result_fields: Some(ParsedResultFields {
                    resolver_id: "test".to_owned(),
                    difficulty: ResultFieldValue::Known {
                        value: Difficulty::Hyper,
                    },
                    level: ResultFieldValue::Known { value: 8 },
                    notes: ResultFieldValue::Known { value: 764 },
                    current_score: ResultFieldValue::Known { value: 1_286 },
                    previous_clear_type: PreviousBestValue::Known {
                        value: "CLEAR".to_owned(),
                    },
                    previous_score: PreviousBestValue::Known { value: 1_200 },
                    previous_miss_count: PreviousBestValue::Known { value: 4 },
                    miss_count: SupplementalResultValue::Known { value: 3 },
                    pgreat: ResultFieldValue::Known { value: 600 },
                    great: ResultFieldValue::Known { value: 86 },
                    good: ResultFieldValue::Known { value: 10 },
                    bad: ResultFieldValue::Known { value: 5 },
                    poor: ResultFieldValue::Known { value: 3 },
                    fast: SupplementalResultValue::Known { value: 20 },
                    slow: SupplementalResultValue::Known { value: 21 },
                    combo_break: SupplementalResultValue::Known { value: 2 },
                }),
                result_chart_resolution: Some(ResultChartResolution::Accepted {
                    resolver_id: "scorepeek-result-fields-catalog-constrained-v5".to_owned(),
                    chart: scorepeek::catalog::Chart {
                        key: scorepeek::catalog::ChartKey {
                            play_type: PlayType::Single,
                            difficulty: Difficulty::Hyper,
                        },
                        level: 8,
                        notes: 764,
                    },
                    current_score: 1_286,
                }),
                result_performance_resolution: Some(ResultPerformanceResolution::Accepted {
                    resolver_id: "scorepeek-result-performance-v1".to_owned(),
                    judgments: ResultJudgments {
                        pgreat: 600,
                        great: 86,
                        good: 10,
                        bad: 5,
                        poor: 3,
                    },
                    miss_count: SupplementalResultValue::Known { value: 3 },
                    timing: ResultTiming {
                        fast: SupplementalResultValue::Known { value: 20 },
                        slow: SupplementalResultValue::Known { value: 21 },
                    },
                    combo_break: SupplementalResultValue::Known { value: 2 },
                    previous_best: PreviousBest {
                        clear_type: scorepeek::recognition::PreviousBestValue::Known {
                            value: "CLEAR".to_owned(),
                        },
                        score: scorepeek::recognition::PreviousBestValue::Known { value: 1_200 },
                        miss_count: scorepeek::recognition::PreviousBestValue::Known { value: 4 },
                    },
                }),
                current_score_ocr_resolution: None,
                numeric_batch: None,
                joint_evidence: JointEvidenceObservation {
                    catalog_song_count: 0,
                    candidates: vec![JointEvidenceCandidate {
                        song_id,
                        chart: scorepeek::catalog::Chart {
                            key: scorepeek::catalog::ChartKey {
                                play_type: PlayType::Single,
                                difficulty: Difficulty::Hyper,
                            },
                            level: 8,
                            notes: 764,
                        },
                        display_titles: vec!["CATALOG TITLE".to_owned()],
                        artist: "CATALOG ARTIST".to_owned(),
                        family_support: BTreeMap::from([
                            (EvidenceFamily::ResultTitle, 120),
                            (EvidenceFamily::ResultArtist, 120),
                            (EvidenceFamily::ResultChart, 160),
                        ]),
                        support: 400,
                    }],
                },
                processing_timing: Value::Null,
                song_resolution_presentation: Box::new(SongResolutionPresentation::Accepted {
                    reason: None,
                    selected: SongPresentation {
                        scorepeek_song_id: song_id,
                        display_titles: vec!["CATALOG TITLE".to_owned()],
                        artist: "CATALOG ARTIST".to_owned(),
                    },
                    runner_up: SongPresentation {
                        scorepeek_song_id: serde_json::from_str(
                            "\"00000000-0000-0000-0000-000000000002\"",
                        )
                        .unwrap(),
                        display_titles: vec!["RUNNER UP".to_owned()],
                        artist: "RUNNER ARTIST".to_owned(),
                    },
                    evidence_summary: "title edit=0; runner-up margin=4".to_owned(),
                }),
            },
        }
    }

    fn detected_result_event(
        session_id: &str,
        capture_generation: u64,
        source_sequence: u64,
        result: ResultDomainEvent,
    ) -> RunEvent {
        RunEvent {
            schema: "scorepeek-run-event-v3".to_owned(),
            kind: RunEventKind::ResultDetected {
                session_id: session_id.to_owned(),
                capture_generation,
                source_sequence,
                song: None,
                result,
            },
        }
    }

    fn prepare_accepted_attempt(output: &mut RoutineOutput) {
        output.engine.play_attempt.observe_selection_screen();
        output
            .engine
            .play_attempt
            .observe_screen(PlayAttemptScreen::Play, 0);
        output
            .engine
            .play_attempt
            .observe_screen(PlayAttemptScreen::Result, 0);
    }

    fn accepted_result_without_joint_identity(sequence: u64) -> RunEvent {
        let mut event = accepted_result_event(sequence);
        let RunEventKind::FieldObservation { joint_evidence, .. } = &mut event.kind else {
            unreachable!();
        };
        joint_evidence.candidates.clear();
        event
    }

    fn screen_event(sequence: u64, screen: &str) -> RunEvent {
        RunEvent {
            schema: "scorepeek-run-event-v3".to_owned(),
            kind: RunEventKind::ScreenChanged {
                session_id: Some("invocation-1-session-1".to_owned()),
                capture_generation: Some(1),
                screen_episode_id: sequence,
                sequence,
                monotonic_start_ms: sequence.saturating_mul(100),
                monotonic_end_ms: sequence.saturating_mul(100).saturating_add(25),
                screen: screen.to_owned(),
            },
        }
    }

    fn semantic_episode_event(
        sequence: u64,
        screen: &str,
        phase: SemanticEpisodePhase,
    ) -> RunEvent {
        RunEvent {
            schema: "scorepeek-run-event-v3".to_owned(),
            kind: RunEventKind::SemanticScreenEpisodeChanged {
                session_id: Some("invocation-1-session-1".to_owned()),
                capture_generation: Some(1),
                screen_episode_id: sequence,
                sequence,
                monotonic_end_ms: sequence.saturating_mul(100).saturating_add(25),
                screen: screen.to_owned(),
                phase,
            },
        }
    }

    fn failed_session_finished_event() -> RunEvent {
        RunEvent::from_value(json!({
            "schema": "scorepeek-run-event-v3",
            "event": "session_finished",
            "session_id": "invocation-1-session-1",
            "capture_generation": 1,
            "outcome": "error",
            "report": { "error_type": "field_observer_finish_failed" }
        }))
        .unwrap()
    }

    fn read_events(reader: &mut BufReader<UnixStream>, count: usize) -> Vec<Value> {
        let mut events = Vec::with_capacity(count);
        while events.len() < count {
            let event = read_raw_event(reader);
            if event["event"] != "resolver_state_changed" {
                events.push(event);
            }
        }
        events
    }

    fn read_raw_event(reader: &mut BufReader<UnixStream>) -> Value {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str::<Value>(&line).unwrap()
    }

    fn read_events_through(
        reader: &mut BufReader<UnixStream>,
        terminal_event: &str,
        maximum: usize,
    ) -> Vec<Value> {
        let mut events = Vec::new();
        while events.len() < maximum {
            let event = read_events(reader, 1).pop().unwrap();
            let complete = event["event"] == terminal_event;
            events.push(event);
            if complete {
                return events;
            }
        }
        panic!("event {terminal_event} was not observed within {maximum} events");
    }

    #[derive(Default)]
    struct FailFirstWrite {
        failed: bool,
        recovered_bytes: Vec<u8>,
    }

    impl Write for FailFirstWrite {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if !self.failed {
                self.failed = true;
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, "injected"));
            }
            self.recovered_bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn partial_terminal_entry_attempts_to_restore_screen_and_cursor() {
        let mut output = FailFirstWrite::default();
        assert!(enter_alternate_screen(&mut output).is_err());
        assert_eq!(output.recovered_bytes, b"\x1b[?25h\x1b[?1049l");
    }

    #[test]
    fn socket_sends_snapshot_before_live_events_and_removes_its_own_path() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let socket_path = channel.socket_path.clone();
        let stream = UnixStream::connect(&socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let snapshot: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(snapshot["schema"], "scorepeek-run-observation-snapshot-v4");
        assert_eq!(snapshot["state"]["invocation_id"], "invocation-1");
        assert_eq!(snapshot["state"]["next_channel_sequence"], 1);
        assert_eq!(snapshot["channel"]["connected_clients"], 1);

        channel
            .publish(&json!({
                "schema": "scorepeek-run-event-v3",
                "event": "watcher_started",
                "channel_sequence": 1,
            }))
            .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let event: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(event["event"], "watcher_started");
        drop(channel);
        assert!(!socket_path.exists());
    }

    #[test]
    fn numeric_before_joint_identity_emits_once_after_attempt_confirmation() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let stream = UnixStream::connect(&channel.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        let mut output = test_output(state, channel);
        prepare_accepted_attempt(&mut output);

        output
            .publish(&accepted_result_without_joint_identity(1))
            .unwrap();
        output
            .publish(&accepted_result_without_joint_identity(2))
            .unwrap();
        output.publish(&accepted_result_event(3)).unwrap();
        output
            .publish(&semantic_episode_event(
                4,
                "result",
                SemanticEpisodePhase::Closing,
            ))
            .unwrap();
        output
            .publish(&semantic_episode_event(
                4,
                "result",
                SemanticEpisodePhase::Finalized,
            ))
            .unwrap();
        let completed = read_events_through(&mut reader, "result_detected", 20);
        let confirmation = completed
            .iter()
            .position(|event| {
                event["event"] == "play_attempt_changed"
                    && event["state"]["attempt"]["result_relation"] == "confirmed"
            })
            .unwrap();
        let result = completed
            .iter()
            .position(|event| event["event"] == "result_detected")
            .unwrap();
        assert!(confirmation < result);
        assert!(completed.iter().any(|event| {
            event["event"] == "numeric_result_changed" && event["state"]["status"] == "accepted"
        }));
        assert_eq!(completed[result]["source_sequence"], 4);

        output.publish(&accepted_result_event(4)).unwrap();
        assert_eq!(read_events(&mut reader, 1)[0]["event"], "field_observation");
    }

    #[test]
    fn incomplete_numeric_finalizes_the_attempt_as_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let mut output = test_output(state, channel);
        prepare_accepted_attempt(&mut output);

        output.publish(&accepted_result_event(1)).unwrap();
        output
            .publish(&semantic_episode_event(
                2,
                "result",
                SemanticEpisodePhase::Finalized,
            ))
            .unwrap();

        assert!(output.emitted_attempt_ids.is_empty());
        assert!(matches!(
            output.engine.play_attempt.state(),
            PlayAttemptState::Attempt { attempt }
                if attempt.result_relation
                    == crate::play_attempt::PlayAttemptResultRelation::Conflict
                    && attempt.reasons.contains(&PlayAttemptReason::ResultEvidenceUnresolved)
        ));
    }

    #[test]
    fn stale_numeric_from_another_chart_cannot_confirm_the_attempt() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let mut output = test_output(state, channel);
        prepare_accepted_attempt(&mut output);

        output.publish(&accepted_result_event(1)).unwrap();
        output.publish(&accepted_result_event(2)).unwrap();
        output
            .engine
            .provisional_joint
            .as_mut()
            .unwrap()
            .chart
            .key
            .difficulty = Difficulty::Another;
        output
            .publish(&semantic_episode_event(
                3,
                "result",
                SemanticEpisodePhase::Finalized,
            ))
            .unwrap();

        assert!(output.emitted_attempt_ids.is_empty());
        assert!(matches!(
            output.engine.play_attempt.state(),
            PlayAttemptState::Attempt { attempt }
                if attempt.result_relation
                    == crate::play_attempt::PlayAttemptResultRelation::Conflict
                    && attempt.reasons.contains(&PlayAttemptReason::LinkageConflict)
        ));
    }

    #[test]
    fn failed_session_boundary_cannot_replace_semantic_result_finalization() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let mut output = test_output(state, channel);
        prepare_accepted_attempt(&mut output);

        output.publish(&accepted_result_event(1)).unwrap();
        output.publish(&accepted_result_event(2)).unwrap();
        output.publish(&failed_session_finished_event()).unwrap();

        assert!(output.emitted_attempt_ids.is_empty());
        assert!(matches!(
            output.engine.play_attempt.state(),
            PlayAttemptState::Attempt { attempt }
                if attempt.phase == crate::play_attempt::PlayAttemptPhase::Abandoned
                    && attempt.reasons.contains(&PlayAttemptReason::SessionEnded)
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn normalized_result_evidence_completes_an_attempt_despite_wrong_select_title() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let mut output = test_output(Arc::clone(&state), channel);
        let correct_song_id =
            serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        let wrong_song_id =
            serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
        let collision_song_id =
            serde_json::from_str("\"00000000-0000-0000-0000-000000000003\"").unwrap();
        let chart = scorepeek::catalog::Chart {
            key: scorepeek::catalog::ChartKey {
                play_type: PlayType::Single,
                difficulty: Difficulty::Hyper,
            },
            level: 8,
            notes: 764,
        };

        output.publish(&screen_event(1, "music_select")).unwrap();
        output.engine.retained_select.observe(
            100,
            &JointEvidenceObservation {
                catalog_song_count: 0,
                candidates: vec![
                    JointEvidenceCandidate {
                        song_id: wrong_song_id,
                        chart: chart.clone(),
                        display_titles: vec!["X".to_owned()],
                        artist: "D.J.Amuro".to_owned(),
                        family_support: BTreeMap::from([
                            (EvidenceFamily::SelectTitleLexical, 300),
                            (EvidenceFamily::SelectTitleStructural, 60),
                            (EvidenceFamily::SelectChart, 50),
                        ]),
                        support: 410,
                    },
                    JointEvidenceCandidate {
                        song_id: correct_song_id,
                        chart: chart.clone(),
                        display_titles: vec!["〆".to_owned()],
                        artist: "lapix".to_owned(),
                        family_support: BTreeMap::from([
                            (EvidenceFamily::SelectTitleStructural, 60),
                            (EvidenceFamily::SelectArtist, 300),
                            (EvidenceFamily::SelectChart, 50),
                        ]),
                        support: 410,
                    },
                ],
            },
            None,
            None,
        );
        output
            .publish(&screen_event(2, "decide_transition"))
            .unwrap();
        output.publish(&screen_event(3, "play")).unwrap();
        output.publish(&screen_event(4, "result")).unwrap();

        let result = |sequence| {
            let mut event = accepted_result_event(sequence);
            let RunEventKind::FieldObservation { joint_evidence, .. } = &mut event.kind else {
                unreachable!();
            };
            joint_evidence.candidates = vec![
                JointEvidenceCandidate {
                    song_id: correct_song_id,
                    chart: chart.clone(),
                    display_titles: vec!["〆".to_owned()],
                    artist: "lapix".to_owned(),
                    family_support: BTreeMap::from([
                        (EvidenceFamily::ResultArtist, 300),
                        (EvidenceFamily::ResultChart, 170),
                    ]),
                    support: 470,
                },
                JointEvidenceCandidate {
                    song_id: wrong_song_id,
                    chart: chart.clone(),
                    display_titles: vec!["WRONG SELECT".to_owned()],
                    artist: "WRONG ARTIST".to_owned(),
                    family_support: BTreeMap::from([(EvidenceFamily::ResultChart, 70)]),
                    support: 70,
                },
                JointEvidenceCandidate {
                    song_id: collision_song_id,
                    chart: chart.clone(),
                    display_titles: vec!["Flying Castle".to_owned()],
                    artist: "lapix".to_owned(),
                    family_support: BTreeMap::from([
                        (EvidenceFamily::ResultArtist, 300),
                        (EvidenceFamily::ResultChart, 170),
                    ]),
                    support: 470,
                },
            ];
            event
        };
        output.publish(&result(5)).unwrap();
        output.publish(&result(6)).unwrap();

        assert_eq!(output.emitted_attempt_ids.len(), 0);
        output
            .publish(&semantic_episode_event(
                7,
                "result",
                SemanticEpisodePhase::Closing,
            ))
            .unwrap();
        output
            .publish(&semantic_episode_event(
                7,
                "result",
                SemanticEpisodePhase::Finalized,
            ))
            .unwrap();
        assert_eq!(output.emitted_attempt_ids.len(), 1);
        assert_eq!(state.lock().unwrap().result_count, 1);
        assert_eq!(
            state
                .lock()
                .unwrap()
                .result_history
                .back()
                .unwrap()
                .song
                .as_ref()
                .unwrap()
                .display_titles[0],
            "〆"
        );
        assert!(matches!(
            output.engine.play_attempt.state(),
            PlayAttemptState::Attempt { attempt }
                if attempt.result_relation
                    == crate::play_attempt::PlayAttemptResultRelation::Confirmed
        ));
    }

    #[test]
    fn result_reentry_does_not_reemit_the_same_attempt() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let mut output = test_output(Arc::clone(&state), channel);
        prepare_accepted_attempt(&mut output);

        output.publish(&accepted_result_event(1)).unwrap();
        output.publish(&accepted_result_event(2)).unwrap();
        assert_eq!(state.lock().unwrap().result_count, 0);
        output
            .publish(&semantic_episode_event(
                3,
                "result",
                SemanticEpisodePhase::Closing,
            ))
            .unwrap();
        output
            .publish(&semantic_episode_event(
                3,
                "result",
                SemanticEpisodePhase::Finalized,
            ))
            .unwrap();
        assert_eq!(state.lock().unwrap().result_count, 1);

        output.publish(&screen_event(4, "result")).unwrap();
        output.publish(&accepted_result_event(5)).unwrap();
        output.publish(&accepted_result_event(6)).unwrap();

        assert_eq!(state.lock().unwrap().result_count, 1);
    }

    #[test]
    fn socket_broadcasts_one_live_event_to_multiple_clients() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let mut readers = (0..2)
            .map(|_| {
                let stream = UnixStream::connect(&channel.socket_path).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                BufReader::new(stream)
            })
            .collect::<Vec<_>>();
        for reader in &mut readers {
            let mut snapshot = String::new();
            reader.read_line(&mut snapshot).unwrap();
            assert!(snapshot.contains("scorepeek-run-observation-snapshot-v4"));
        }
        channel
            .publish(&json!({
                "schema": "scorepeek-run-event-v3",
                "event": "watcher_started",
                "channel_sequence": 1,
            }))
            .unwrap();
        for reader in &mut readers {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["channel_sequence"],
                1
            );
        }
    }

    #[test]
    fn publishing_without_clients_is_healthy() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        channel
            .publish(&json!({
                "schema": "scorepeek-run-event-v3",
                "event": "watcher_started",
                "channel_sequence": 1,
            }))
            .unwrap();
        std::thread::sleep(Duration::from_millis(40));
        assert!(!channel.health.server_failed.load(Ordering::Acquire));
        assert_eq!(channel.health.connected_clients.load(Ordering::Acquire), 0);
    }

    #[test]
    fn a_slow_client_is_disconnected_without_degrading_the_server() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let stream = UnixStream::connect(&channel.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        channel
            .publish(&json!({
                "schema": "scorepeek-run-event-v3",
                "event": "field_observation",
                "channel_sequence": 1,
                "payload": "x".repeat(2 * 1024 * 1024),
            }))
            .unwrap();
        for _ in 0..50 {
            if channel.health.disconnected_clients.load(Ordering::Acquire) > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(channel.health.connected_clients.load(Ordering::Acquire), 0);
        assert_eq!(
            channel.health.disconnected_clients.load(Ordering::Acquire),
            1
        );
        assert!(!channel.health.server_failed.load(Ordering::Acquire));
    }

    #[test]
    fn stale_socket_is_replaced_but_other_entries_are_preserved() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("scorepeek");
        fs::create_dir(&directory).unwrap();
        let socket_path = directory.join(SOCKET_NAME);
        let stale = UnixListener::bind(&socket_path).unwrap();
        drop(stale);
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        drop(channel);

        fs::write(&socket_path, b"owned by operator").unwrap();
        let Err(error) = ObservationChannel::start_at(temporary.path(), state()) else {
            panic!("non-socket entry must not be replaced");
        };
        assert!(error.contains("non-socket"));
        assert_eq!(fs::read(&socket_path).unwrap(), b"owned by operator");

        fs::remove_file(&socket_path).unwrap();
        let target = directory.join("target");
        fs::write(&target, b"target").unwrap();
        symlink(&target, &socket_path).unwrap();
        let Err(error) = ObservationChannel::start_at(temporary.path(), state()) else {
            panic!("symlink must not be replaced");
        };
        assert!(error.contains("non-socket"));
        assert!(
            socket_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn active_socket_is_not_unlinked_or_rebound() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("scorepeek");
        fs::create_dir(&directory).unwrap();
        let socket_path = directory.join(SOCKET_NAME);
        let listener = UnixListener::bind(&socket_path).unwrap();
        let Err(error) = ObservationChannel::start_at(temporary.path(), state()) else {
            panic!("active socket must not be replaced");
        };
        assert!(error.contains("already active"));
        assert!(
            socket_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_socket()
        );
        drop(listener);
    }

    #[test]
    fn initialization_guard_removes_only_the_socket_it_owns() {
        let temporary = tempfile::tempdir().unwrap();
        let socket_path = temporary.path().join("initializing.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let metadata = socket_path.symlink_metadata().unwrap();
        let identity = (metadata.dev(), metadata.ino());
        drop(SocketPathGuard::new(socket_path.clone(), identity));
        assert!(!socket_path.exists());
        drop(listener);
    }

    #[test]
    fn cleanup_preserves_an_entry_that_replaced_the_owned_socket() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let socket_path = channel.socket_path.clone();
        fs::remove_file(&socket_path).unwrap();
        fs::write(&socket_path, b"replacement").unwrap();
        drop(channel);
        assert_eq!(fs::read(&socket_path).unwrap(), b"replacement");
    }

    #[test]
    fn cleanup_preserves_a_socket_with_a_different_identity() {
        let temporary = tempfile::tempdir().unwrap();
        let channel = ObservationChannel::start_at(temporary.path(), state()).unwrap();
        let socket_path = channel.socket_path.clone();
        fs::remove_file(&socket_path).unwrap();
        let replacement = UnixListener::bind(&socket_path).unwrap();
        drop(channel);
        assert!(
            socket_path
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_socket()
        );
        drop(replacement);
    }

    #[test]
    fn full_event_queue_is_counted_without_blocking_the_producer() {
        let (sender, _receiver) = std::sync::mpsc::sync_channel::<Vec<u8>>(1);
        let health = ChannelHealth::default();
        try_send_event(&sender, &health, vec![1]);
        try_send_event(&sender, &health, vec![2]);
        assert_eq!(health.dropped_events.load(Ordering::Acquire), 1);
        assert!(!health.server_failed.load(Ordering::Acquire));
    }

    #[test]
    fn plain_status_does_not_change_for_a_field_observation() {
        let mut state = RunViewState::new("invocation-1".to_owned(), "e".repeat(64), true);
        let health = ChannelHealth::default();
        let before = plain_status_line(&state, &health);
        state.latest_observation = Some(json!({
            "event": "field_observation",
            "fields": { "title": "OCR VALUE" }
        }));
        assert_eq!(plain_status_line(&state, &health), before);
        assert!(!before.contains("OCR VALUE"));
    }

    #[test]
    fn typed_reducer_tracks_session_report_and_stop_transitions() {
        let mut state = RunViewState::new("invocation-1".to_owned(), "d".repeat(64), true);
        let started = RunEvent::from_value(json!({
            "schema": "scorepeek-run-event-v3",
            "event": "session_started",
            "session_id": "invocation-1-session-1",
            "capture_generation": 1,
            "capture_profile_sha256": "profile",
            "normalizer_artifact_sha256": "normalizer"
        }))
        .unwrap();
        state.reduce(&started, &started.to_value().unwrap());
        assert_eq!(state.watcher_state, "session_active");
        assert_eq!(state.session_count, 1);
        assert_eq!(
            state.active_session_id.as_deref(),
            Some("invocation-1-session-1")
        );

        let finished = RunEvent::from_value(json!({
            "schema": "scorepeek-run-event-v3",
            "event": "session_finished",
            "session_id": "invocation-1-session-1",
            "capture_generation": 1,
            "outcome": "source_ended",
            "report": { "recognition_ticks": 3 }
        }))
        .unwrap();
        state.reduce(&finished, &finished.to_value().unwrap());
        assert_eq!(state.watcher_state, "session_finished");
        assert_eq!(
            state.latest_report.as_ref().unwrap()["recognition_ticks"],
            3
        );

        state.latest_observation = Some(json!({ "sequence": 9 }));
        state.latest_stabilized_result = Some(json!({ "state": { "song": "stable" } }));
        state.latest_temporal_music_select = Some(json!({ "state": { "status": "changing" } }));
        let next_started = RunEvent::from_value(json!({
            "schema": "scorepeek-run-event-v3",
            "event": "session_started",
            "session_id": "invocation-1-session-2",
            "capture_generation": 2,
            "capture_profile_sha256": "profile",
            "normalizer_artifact_sha256": "normalizer"
        }))
        .unwrap();
        state.reduce(&next_started, &next_started.to_value().unwrap());
        assert_eq!(
            state.active_session_id.as_deref(),
            Some("invocation-1-session-2")
        );
        assert!(state.latest_observation.is_none());
        assert!(state.latest_report.is_none());

        let stopped = RunEvent::from_value(json!({
            "schema": "scorepeek-run-event-v3",
            "event": "watcher_stopped",
            "invocation_id": "invocation-1",
            "reason": "signal"
        }))
        .unwrap();
        state.reduce(&stopped, &stopped.to_value().unwrap());
        assert_eq!(state.watcher_state, "stopped");
        assert!(state.latest_observation.is_none());
        assert!(state.latest_stabilized_result.is_none());
        assert!(state.latest_temporal_music_select.is_none());
    }

    #[test]
    fn result_history_remains_bounded_and_survives_session_changes() {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        let mut state = RunViewState::new("invocation-1".to_owned(), "a".repeat(64), true);
        state.stable_result_song = Some(SongPresentation {
            scorepeek_song_id: song_id,
            display_titles: vec!["TITLE".to_owned()],
            artist: "ARTIST".to_owned(),
        });
        for source_sequence in 1..=(RESULT_HISTORY_CAPACITY as u64 + 1) {
            let result = detected_result_event(
                "session-1",
                1,
                source_sequence,
                ResultDomainEvent {
                    contract: "scorepeek-result-detected-v2".to_owned(),
                    attempt_id: source_sequence,
                    parent_attempt_id: None,
                    scorepeek_song_id: song_id,
                    play_side: "one_player".to_owned(),
                    play_mode: "single_play".to_owned(),
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Normal,
                    level: 5,
                    notes: 100,
                    current_score: 150,
                    clear_type: "CLEAR".to_owned(),
                    judgments: ResultJudgments {
                        pgreat: 70,
                        great: 10,
                        good: 5,
                        bad: 3,
                        poor: 2,
                    },
                    miss_count: SupplementalResultValue::Known { value: 2 },
                    timing: ResultTiming {
                        fast: SupplementalResultValue::Known { value: 4 },
                        slow: SupplementalResultValue::Known { value: 5 },
                    },
                    combo_break: SupplementalResultValue::Known { value: 1 },
                    previous_best: PreviousBest {
                        clear_type: scorepeek::recognition::PreviousBestValue::NotPlayed,
                        score: scorepeek::recognition::PreviousBestValue::NotPlayed,
                        miss_count: scorepeek::recognition::PreviousBestValue::NotPlayed,
                    },
                },
            );
            state.reduce(&result, &result.to_value().unwrap());
        }
        assert_eq!(state.result_count, RESULT_HISTORY_CAPACITY as u64 + 1);
        assert_eq!(state.result_history.len(), RESULT_HISTORY_CAPACITY);
        assert_eq!(state.result_history.front().unwrap().ordinal, 2);
        assert_eq!(
            state.result_history.back().unwrap().ordinal,
            RESULT_HISTORY_CAPACITY as u64 + 1
        );

        let next_session = RunEvent {
            schema: "scorepeek-run-event-v3".to_owned(),
            kind: RunEventKind::SessionStarted {
                session_id: Some("session-2".to_owned()),
                capture_generation: 2,
                capture_profile_sha256: "b".repeat(64),
                normalizer_artifact_sha256: "c".repeat(64),
            },
        };
        state.reduce(&next_session, &next_session.to_value().unwrap());
        assert_eq!(
            state.result_history.back().unwrap().ordinal,
            RESULT_HISTORY_CAPACITY as u64 + 1
        );
        assert_eq!(state.result_count, RESULT_HISTORY_CAPACITY as u64 + 1);
        assert!(state.stable_result_song.is_none());
    }

    #[test]
    fn result_value_labels_preserve_domain_states_without_debug_reasons() {
        use scorepeek::recognition::ResultFieldUnknownReason;

        assert_eq!(
            supplemental_u32(&SupplementalResultValue::Known { value: 1_234 }),
            "1,234"
        );
        assert_eq!(
            supplemental_u32(&SupplementalResultValue::NotDisplayed),
            "--"
        );
        assert_eq!(
            supplemental_u32(&SupplementalResultValue::Unknown {
                reason: ResultFieldUnknownReason::InvalidFormat,
            }),
            "?"
        );
        assert_eq!(previous_text(&PreviousBestValue::NotPlayed), "NO PLAY");
        assert_eq!(previous_u32(&PreviousBestValue::NotDisplayed), "--");
        assert_eq!(
            previous_u32(&PreviousBestValue::Unknown {
                reason: ResultFieldUnknownReason::OutOfRange,
            }),
            "?"
        );
    }

    #[test]
    fn fitted_song_text_uses_an_ellipsis_without_mutating_the_value() {
        let value = "非常に長い曲名を完全な状態で保持する";
        let rendered = fitted_value("Catalog title: ", value, 24);
        assert!(rendered.starts_with("Catalog title: "));
        assert!(rendered.ends_with('…'));
        assert!(Line::raw(rendered).width() <= 24);
        assert_eq!(value, "非常に長い曲名を完全な状態で保持する");
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the 80x25 fixture spells out every simultaneously visible resolver node and gate"
    )]
    fn fixed_three_pane_tui_renders_resolver_duration_and_clips_below_minimum() {
        let mut state = RunViewState::new("invocation-1".to_owned(), "a".repeat(64), true);
        state.watcher_state = "session_active".to_owned();
        state.capture_generation = Some(3);
        state.resolver = ResolverDebugSnapshot {
            now_ms: 14_900,
            raw_screen: Some("result".to_owned()),
            screen: Some("result".to_owned()),
            suspended: false,
            finalizing: false,
            screen_episode_id: 18,
            screen_episode_started_ms: Some(2_000),
            source_sequence: Some(1_240),
            latest_field_sequence: Some(1_238),
            latest_field_ms: Some(13_000),
            local: Some(ResolverNodeSnapshot {
                label: "RESULT resolver",
                started_ms: Some(3_000),
                last_observation_ms: Some(13_000),
                observations: 6,
                top: Some("TEST SONG / HYPER Lv8".to_owned()),
                runner_up: Some("OTHER SONG / HYPER Lv8".to_owned()),
                runner_song: Some("OTHER SONG / SP HYPER Lv8 notes=764".to_owned()),
                runner_chart: None,
                top_candidates: vec!["TEST SONG / SP HYPER Lv8 notes=764".to_owned()],
                support: 320,
                margin: 80,
                song_margin: 80,
                chart_margin: 320,
                family_contributions: vec!["result_title=300".to_owned()],
                state: ResolverResolutionState::AcceptedJoint,
            }),
            successor: Some(ResolverNodeSnapshot {
                label: "successor",
                started_ms: Some(13_000),
                last_observation_ms: Some(14_000),
                observations: 2,
                top: Some("NEXT / SP HYPER Lv9 notes=900".to_owned()),
                runner_up: None,
                runner_song: None,
                runner_chart: None,
                top_candidates: vec!["NEXT / SP HYPER Lv9 notes=900".to_owned()],
                support: 140,
                margin: 140,
                song_margin: 140,
                chart_margin: 140,
                family_contributions: vec!["select_title=140".to_owned()],
                state: ResolverResolutionState::JointCandidate,
            }),
            attempt: Some(AttemptNodeSnapshot {
                attempt_id: Some(14),
                started_ms: Some(1_000),
                phase_started_ms: Some(2_000),
                phase: "result".to_owned(),
                path: "S-D-P-R".to_owned(),
                select_top: Some("TEST SONG / HYPER Lv8".to_owned()),
                result_top: Some("TEST SONG / HYPER Lv8".to_owned()),
                joint_top: Some("TEST SONG / HYPER Lv8".to_owned()),
                support: 400,
                margin: 160,
                song_margin: 160,
                chart_margin: 400,
                runner_song: Some("OTHER SONG / SP HYPER Lv8 notes=764".to_owned()),
                runner_chart: None,
                top_candidates: vec!["TEST SONG / SP HYPER Lv8 notes=764".to_owned()],
                family_contributions: vec!["result_title=300".to_owned()],
                state: ResolverResolutionState::AcceptedJoint,
            }),
            gate: "waiting: numeric performance".to_owned(),
            gates: vec![
                GateSnapshot {
                    label: "link",
                    state: GateState::Accepted,
                },
                GateSnapshot {
                    label: "identity",
                    state: GateState::Accepted,
                },
                GateSnapshot {
                    label: "clear",
                    state: GateState::Accepted,
                },
                GateSnapshot {
                    label: "numeric",
                    state: GateState::Pending,
                },
                GateSnapshot {
                    label: "drain",
                    state: GateState::Inactive,
                },
                GateSnapshot {
                    label: "emit",
                    state: GateState::Inactive,
                },
            ],
            raw_fields: vec![("title".to_owned(), "OCR TITLE".to_owned())],
        };
        let health = ChannelHealth::default();
        for (width, height) in [(80, 25), (79, 24)] {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| render(frame, &state, Path::new("/run/scorepeek.sock"), &health))
                .unwrap();
            if width == 80 {
                let rendered = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>();
                assert!(rendered.contains("Watcher"));
                assert!(rendered.contains("Latest domain"));
                assert!(rendered.contains("Resolver"));
                assert!(rendered.contains("episode=#18"));
                assert!(rendered.contains("#18 12s"));
                assert!(rendered.contains("ATTEMPT #14"));
                assert!(rendered.contains("numeric…"));
                assert!(rendered.contains("link✓"));
                assert!(rendered.contains("identity✓"));
                assert!(rendered.contains("clear✓"));
                assert!(rendered.contains("drain–"));
                assert!(rendered.contains("emit–"));
                assert!(
                    terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .any(|cell| cell.fg == Color::Green)
                );
                assert!(
                    terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .any(|cell| cell.fg == Color::Yellow)
                );
            }
        }
    }

    #[test]
    fn semantic_palette_keeps_typed_state_and_domain_colors_consistent() {
        assert_eq!(
            resolution_color(ResolverResolutionState::AcceptedJoint),
            Color::Green
        );
        assert_eq!(
            resolution_color(ResolverResolutionState::JointCandidate),
            Color::Cyan
        );
        assert_eq!(
            resolution_color(ResolverResolutionState::Unresolved),
            Color::Yellow
        );
        assert_eq!(
            resolution_color(ResolverResolutionState::Conflict),
            Color::Red
        );
        assert_eq!(gate_color(GateState::Inactive), Color::DarkGray);
        assert_eq!(gate_suffix(GateState::Accepted), "✓");
        assert_eq!(gate_suffix(GateState::Pending), "…");
        assert_eq!(gate_suffix(GateState::Failed), "✗");
        assert_eq!(gate_suffix(GateState::Inactive), "–");
        assert_eq!(difficulty_color(Difficulty::Beginner), Color::Green);
        assert_eq!(difficulty_color(Difficulty::Normal), Color::Blue);
        assert_eq!(difficulty_color(Difficulty::Hyper), Color::Yellow);
        assert_eq!(difficulty_color(Difficulty::Another), Color::Red);
        assert_eq!(difficulty_color(Difficulty::Leggendaria), Color::Magenta);
        assert_eq!(clear_type_color("FAILED"), Color::Red);
        assert_eq!(clear_type_color("ASSIST CLEAR"), Color::Yellow);
        assert_eq!(clear_type_color("F-COMBO"), Color::Green);
    }

    #[test]
    fn episode_evidence_accumulates_by_family_caps_and_unknown_does_not_erase() {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        let candidate = JointEvidenceCandidate {
            song_id,
            chart: scorepeek::catalog::Chart {
                key: scorepeek::catalog::ChartKey {
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Hyper,
                },
                level: 8,
                notes: 764,
            },
            display_titles: vec!["TEST SONG".to_owned()],
            artist: "TEST ARTIST".to_owned(),
            family_support: BTreeMap::from([
                (EvidenceFamily::ResultTitle, 70),
                (EvidenceFamily::ResultArtist, 35),
                (EvidenceFamily::ResultChart, 50),
            ]),
            support: 155,
        };
        let observation = JointEvidenceObservation {
            catalog_song_count: 0,
            candidates: vec![candidate],
        };
        let mut accumulator = HypothesisAccumulator::default();
        let chart_factor = ResultChartFactor {
            difficulty: Some(Difficulty::Hyper),
            notes: Some(764),
            level: Some(8),
        };
        accumulator.observe(100, &observation, None, Some(chart_factor));
        assert_eq!(
            accumulator.summary().state,
            ResolverResolutionState::JointCandidate
        );
        accumulator.observe(
            200,
            &JointEvidenceObservation {
                catalog_song_count: 0,
                candidates: Vec::new(),
            },
            None,
            None,
        );
        accumulator.observe(300, &observation, None, Some(chart_factor));
        assert_eq!(
            accumulator.summary().state,
            ResolverResolutionState::AcceptedJoint
        );
        for tick in 0..20 {
            accumulator.observe(400 + tick, &observation, None, None);
        }
        let accepted = accumulator.summary().accepted().unwrap();
        let stored = &accumulator.candidates[&JointKey {
            song_id,
            chart_key: accepted.chart.key,
        }];
        assert!(
            stored
                .family_support
                .values()
                .any(|support| { *support > u64::from(EVIDENCE_FAMILY_CAP) })
        );
        assert!(
            accumulator
                .summary()
                .selected_family_support
                .values()
                .all(|support| support.normalized <= EVIDENCE_FAMILY_CAP)
        );
    }

    #[test]
    fn family_normalization_preserves_candidate_ratios_above_the_cap() {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        let make = |difficulty, support| JointEvidenceCandidate {
            song_id,
            chart: scorepeek::catalog::Chart {
                key: scorepeek::catalog::ChartKey {
                    play_type: PlayType::Single,
                    difficulty,
                },
                level: 8,
                notes: 764,
            },
            display_titles: vec!["TEST SONG".to_owned()],
            artist: "TEST ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, support)]),
            support,
        };
        let observation = JointEvidenceObservation {
            catalog_song_count: 0,
            candidates: vec![make(Difficulty::Hyper, 170), make(Difficulty::Another, 70)],
        };
        let mut accumulator = HypothesisAccumulator::default();
        for tick in 0..3 {
            accumulator.observe(100 + tick, &observation, None, None);
        }
        let summary = accumulator.summary();
        assert_eq!(summary.support, 300);
        assert_eq!(summary.support.saturating_sub(summary.margin), 123);
        assert_eq!(summary.margin, 177);
        assert_eq!(
            summary.selected_family_support[&EvidenceFamily::ResultArtist],
            EvidenceContribution {
                raw: 510,
                normalized: 300,
            }
        );
        assert_eq!(
            summary.runner_up_family_support[&EvidenceFamily::ResultArtist],
            EvidenceContribution {
                raw: 210,
                normalized: 123,
            }
        );
    }

    #[test]
    fn chart_factors_wait_for_song_evidence_and_apply_to_later_candidates() {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000011\"").unwrap();
        let mut accumulator = HypothesisAccumulator::default();
        accumulator.observe(
            100,
            &JointEvidenceObservation {
                catalog_song_count: 2,
                candidates: Vec::new(),
            },
            Some(Difficulty::Hyper),
            Some(ResultChartFactor {
                difficulty: Some(Difficulty::Hyper),
                notes: Some(1_136),
                level: Some(10),
            }),
        );
        assert_eq!(
            accumulator.summary().state,
            ResolverResolutionState::Unresolved
        );

        let candidate = |difficulty, notes| JointEvidenceCandidate {
            song_id,
            chart: scorepeek::catalog::Chart {
                key: scorepeek::catalog::ChartKey {
                    play_type: PlayType::Single,
                    difficulty,
                },
                level: 10,
                notes,
            },
            display_titles: vec!["∀".to_owned()],
            artist: "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, 220)]),
            support: 220,
        };
        accumulator.observe(
            200,
            &JointEvidenceObservation {
                catalog_song_count: 2,
                candidates: vec![
                    candidate(Difficulty::Hyper, 1_136),
                    candidate(Difficulty::Another, 1_500),
                ],
            },
            None,
            None,
        );
        let summary = accumulator.summary();
        assert_eq!(
            summary.selected.unwrap().chart.key.difficulty,
            Difficulty::Hyper
        );
        assert_eq!(summary.support, 370);
        assert_eq!(summary.chart_margin, 140);
    }

    #[test]
    fn selection_epoch_retains_difficulty_until_song_evidence_arrives() {
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000012\"").unwrap();
        let mut epochs = SelectionEpochTracker::default();
        epochs.observe(
            100,
            &JointEvidenceObservation {
                catalog_song_count: 2,
                candidates: Vec::new(),
            },
            Some(Difficulty::Hyper),
        );
        let candidate = |difficulty| JointEvidenceCandidate {
            song_id,
            chart: scorepeek::catalog::Chart {
                key: scorepeek::catalog::ChartKey {
                    play_type: PlayType::Single,
                    difficulty,
                },
                level: 10,
                notes: if difficulty == Difficulty::Hyper {
                    1_136
                } else {
                    1_500
                },
            },
            display_titles: vec!["∀".to_owned()],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::SelectTitle, 300)]),
            support: 300,
        };
        epochs.observe(
            200,
            &JointEvidenceObservation {
                catalog_song_count: 2,
                candidates: vec![candidate(Difficulty::Hyper), candidate(Difficulty::Another)],
            },
            None,
        );
        let summary = epochs.incumbent.summary();
        assert_eq!(
            summary.selected.unwrap().chart.key.difficulty,
            Difficulty::Hyper
        );
        assert_eq!(summary.chart_margin, 50);
        assert!(epochs.pending_difficulty_support.is_empty());
    }

    #[test]
    fn diagnostic_top_does_not_truncate_resolver_authority() {
        let first = serde_json::from_str("\"00000000-0000-0000-0000-000000000051\"").unwrap();
        let second = serde_json::from_str("\"00000000-0000-0000-0000-000000000052\"").unwrap();
        let keys = [
            (PlayType::Single, Difficulty::Beginner),
            (PlayType::Single, Difficulty::Normal),
            (PlayType::Single, Difficulty::Hyper),
            (PlayType::Single, Difficulty::Another),
            (PlayType::Single, Difficulty::Leggendaria),
            (PlayType::Double, Difficulty::Normal),
            (PlayType::Double, Difficulty::Hyper),
            (PlayType::Double, Difficulty::Another),
        ];
        let candidate = |song_id, play_type, difficulty, support| JointEvidenceCandidate {
            song_id,
            chart: scorepeek::catalog::Chart {
                key: scorepeek::catalog::ChartKey {
                    play_type,
                    difficulty,
                },
                level: 10,
                notes: 1_000,
            },
            display_titles: vec![format!("SONG-{song_id:?}")],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, support)]),
            support,
        };
        let mut candidates = keys
            .into_iter()
            .map(|(play_type, difficulty)| candidate(first, play_type, difficulty, 300))
            .collect::<Vec<_>>();
        candidates.push(candidate(second, PlayType::Single, Difficulty::Hyper, 270));
        let mut event = accepted_result_event(1);
        let RunEventKind::FieldObservation { joint_evidence, .. } = &mut event.kind else {
            unreachable!();
        };
        joint_evidence.candidates = candidates;
        joint_evidence.catalog_song_count = 2;

        let mut authority = HypothesisAccumulator::default();
        authority.observe(100, joint_evidence, None, None);
        assert_eq!(authority.summary().state, ResolverResolutionState::Conflict);
        assert_eq!(joint_evidence.candidates.len(), 9);
        let diagnostic = bounded_run_event_value(&event).unwrap();
        assert_eq!(
            diagnostic["joint_evidence"]["candidates"]
                .as_array()
                .unwrap()
                .len(),
            8
        );
        let RunEventKind::FieldObservation { joint_evidence, .. } = &event.kind else {
            unreachable!();
        };
        assert_eq!(joint_evidence.candidates.len(), 9);
    }

    #[test]
    fn runner_song_and_runner_chart_are_independent_hierarchical_competitors() {
        let first = serde_json::from_str("\"00000000-0000-0000-0000-000000000021\"").unwrap();
        let second = serde_json::from_str("\"00000000-0000-0000-0000-000000000022\"").unwrap();
        let candidate = |song_id, play_type, support| JointEvidenceCandidate {
            song_id,
            chart: scorepeek::catalog::Chart {
                key: scorepeek::catalog::ChartKey {
                    play_type,
                    difficulty: Difficulty::Hyper,
                },
                level: 10,
                notes: 1_136,
            },
            display_titles: vec![format!("SONG-{song_id:?}")],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(EvidenceFamily::ResultArtist, support)]),
            support,
        };
        let mut accumulator = HypothesisAccumulator::default();
        accumulator.observe(
            100,
            &JointEvidenceObservation {
                catalog_song_count: 2,
                candidates: vec![
                    candidate(first, PlayType::Single, 400),
                    candidate(first, PlayType::Double, 380),
                    candidate(second, PlayType::Single, 350),
                ],
            },
            None,
            None,
        );
        let summary = accumulator.summary();
        assert_eq!(summary.runner_up.as_ref().unwrap().song_id, first);
        assert_eq!(summary.runner_chart.as_ref().unwrap().song_id, first);
        assert_eq!(summary.runner_song.as_ref().unwrap().song_id, second);
        assert_eq!(summary.song_margin, 38);
        assert_eq!(summary.chart_margin, 15);
    }

    #[test]
    fn latest_failure_oracle_resolves_forall_from_cross_screen_factors() {
        let wrong = serde_json::from_str("\"00000000-0000-0000-0000-000000000031\"").unwrap();
        let expected = serde_json::from_str("\"00000000-0000-0000-0000-000000000032\"").unwrap();
        let chart = |song_id, play_type, notes, family, support| JointEvidenceCandidate {
            song_id,
            chart: scorepeek::catalog::Chart {
                key: scorepeek::catalog::ChartKey {
                    play_type,
                    difficulty: Difficulty::Hyper,
                },
                level: 10,
                notes,
            },
            display_titles: vec![if song_id == expected { "∀" } else { "A" }.to_owned()],
            artist: if song_id == expected {
                "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro"
            } else {
                "OTHER"
            }
            .to_owned(),
            family_support: BTreeMap::from([(family, support)]),
            support,
        };
        let mut select = HypothesisAccumulator::default();
        select.observe(
            100,
            &JointEvidenceObservation {
                catalog_song_count: 2,
                candidates: vec![chart(
                    wrong,
                    PlayType::Single,
                    1_000,
                    EvidenceFamily::SelectTitle,
                    300,
                )],
            },
            Some(Difficulty::Hyper),
            None,
        );
        let mut result = HypothesisAccumulator::default();
        result.observe(
            200,
            &JointEvidenceObservation {
                catalog_song_count: 2,
                candidates: vec![
                    chart(
                        expected,
                        PlayType::Single,
                        1_136,
                        EvidenceFamily::ResultArtist,
                        300,
                    ),
                    chart(
                        expected,
                        PlayType::Double,
                        1_500,
                        EvidenceFamily::ResultArtist,
                        300,
                    ),
                ],
            },
            None,
            Some(ResultChartFactor {
                difficulty: Some(Difficulty::Hyper),
                notes: Some(1_136),
                level: None,
            }),
        );
        select.add_from(&result);
        let summary = select.summary();
        let accepted = summary.accepted().expect("∀ SP HYPER should resolve");
        assert_eq!(accepted.song_id, expected);
        assert_eq!(accepted.chart.key.play_type, PlayType::Single);
        assert_eq!(accepted.chart.notes, 1_136);
        assert_eq!(summary.song_margin, 50);
        assert_eq!(summary.chart_margin, 50);
    }

    #[test]
    fn music_select_fields_update_the_typed_tui_snapshot() {
        let shared = state();
        shared.lock().unwrap().current_screen = Some("music_select".to_owned());
        let mut output = test_output(Arc::clone(&shared), disconnected_test_channel());
        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000041\"").unwrap();
        let fields = json!({
            "active_list_title": "A",
            "artist": "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro",
            "selected_difficulty": {
                "state": { "status": "known", "value": "hyper" },
                "winner_score_ppm": 500_000,
                "margin_ppm": 250_000
            },
            "title_evidence": {
                "foreground": { "open_text": "A" },
                "normalized_scalar_count": 1,
                "geometry": {
                    "occupancy_width_ppm": 42000,
                    "touches_left_edge": false,
                    "touches_right_edge": false
                }
            }
        });
        output
            .reduce_music_select_observation(
                Some(&"invocation-1-session-1".to_owned()),
                Some(1),
                42,
                4_200,
                &fields,
                &JointEvidenceObservation {
                    catalog_song_count: 2,
                    candidates: vec![JointEvidenceCandidate {
                        song_id,
                        chart: scorepeek::catalog::Chart {
                            key: scorepeek::catalog::ChartKey {
                                play_type: PlayType::Single,
                                difficulty: Difficulty::Hyper,
                            },
                            level: 10,
                            notes: 1_136,
                        },
                        display_titles: vec!["∀".to_owned()],
                        artist: "BEMANI Sound Team \"HuΣeR\" respect for D.J.Amuro".to_owned(),
                        family_support: BTreeMap::from([
                            (EvidenceFamily::SelectTitle, 180),
                            (EvidenceFamily::SelectArtist, 300),
                        ]),
                        support: 480,
                    }],
                },
                &SongResolutionPresentation::Unknown {
                    reason: json!("test"),
                    selected: None,
                    runner_up: None,
                    evidence_summary: None,
                },
            )
            .unwrap();
        let snapshot = shared.lock().unwrap().resolver.clone();
        assert_eq!(snapshot.latest_field_sequence, Some(42));
        assert!(
            snapshot
                .raw_fields
                .iter()
                .any(|(key, value)| { key == "artist" && value.contains("HuΣeR") })
        );
        assert!(
            snapshot
                .raw_fields
                .iter()
                .any(|(key, value)| { key == "marker" && value.contains("known:hyper") })
        );
        assert_eq!(snapshot.local.unwrap().top_candidates.len(), 1);
        output
            .publish(&RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::SemanticScreenEpisodeChanged {
                    session_id: Some("invocation-1-session-1".to_owned()),
                    capture_generation: Some(1),
                    screen_episode_id: 43,
                    sequence: 43,
                    monotonic_end_ms: 4_300,
                    screen: "play".to_owned(),
                    phase: SemanticEpisodePhase::Started,
                },
            })
            .unwrap();
        let snapshot = shared.lock().unwrap().resolver.clone();
        assert!(snapshot.raw_fields.is_empty());
        assert_eq!(snapshot.latest_field_sequence, None);
        assert_eq!(snapshot.latest_field_ms, None);
    }

    #[test]
    fn music_select_handoff_waits_for_admitted_field_drain() {
        let shared = state();
        let mut output = test_output(Arc::clone(&shared), disconnected_test_channel());
        let semantic = |sequence, phase| RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind: RunEventKind::SemanticScreenEpisodeChanged {
                session_id: Some("invocation-1-session-1".to_owned()),
                capture_generation: Some(1),
                screen_episode_id: 7,
                sequence,
                monotonic_end_ms: sequence * 100,
                screen: "music_select".to_owned(),
                phase,
            },
        };
        output
            .publish(&semantic(70, SemanticEpisodePhase::Started))
            .unwrap();
        output
            .publish(&semantic(71, SemanticEpisodePhase::Closing))
            .unwrap();
        assert_eq!(output.engine.retained_select.observation_count, 0);

        let song_id = serde_json::from_str("\"00000000-0000-0000-0000-000000000061\"").unwrap();
        output
            .publish(&RunEvent {
                schema: RUN_EVENT_SCHEMA.to_owned(),
                kind: RunEventKind::FieldObservation {
                    session_id: Some("invocation-1-session-1".to_owned()),
                    capture_generation: Some(1),
                    screen_episode_id: 7,
                    sequence: 70,
                    monotonic_start_ms: 6_900,
                    monotonic_end_ms: 7_050,
                    screen: "music_select".to_owned(),
                    fields: json!({
                        "active_list_title": "A",
                        "artist": "ARTIST",
                        "selected_difficulty": { "state": { "status": "known", "value": "hyper" } }
                    }),
                    result_song_resolution: Value::Null,
                    music_select_song_resolution: Value::Null,
                    parsed_result_fields: None,
                    result_chart_resolution: None,
                    result_performance_resolution: None,
                    current_score_ocr_resolution: None,
                    numeric_batch: None,
                    joint_evidence: JointEvidenceObservation {
                        catalog_song_count: 2,
                        candidates: vec![JointEvidenceCandidate {
                            song_id,
                            chart: scorepeek::catalog::Chart {
                                key: scorepeek::catalog::ChartKey {
                                    play_type: PlayType::Single,
                                    difficulty: Difficulty::Hyper,
                                },
                                level: 10,
                                notes: 1_136,
                            },
                            display_titles: vec!["∀".to_owned()],
                            artist: "ARTIST".to_owned(),
                            family_support: BTreeMap::from([(EvidenceFamily::SelectArtist, 300)]),
                            support: 300,
                        }],
                    },
                    processing_timing: Value::Null,
                    song_resolution_presentation: Box::new(SongResolutionPresentation::Unknown {
                        reason: json!("test"),
                        selected: None,
                        runner_up: None,
                        evidence_summary: None,
                    }),
                },
            })
            .unwrap();
        assert_eq!(
            output.engine.selection_epochs.incumbent.observation_count,
            1
        );
        assert_eq!(output.engine.retained_select.observation_count, 0);
        output
            .publish(&semantic(72, SemanticEpisodePhase::Finalized))
            .unwrap();
        assert_eq!(output.engine.retained_select.observation_count, 1);
        assert_eq!(
            output
                .engine
                .retained_select
                .summary()
                .selected
                .unwrap()
                .song_id,
            song_id
        );
    }

    #[test]
    fn selection_epoch_hands_off_only_the_latest_unfinished_successor() {
        let song = |suffix: u8| {
            serde_json::from_str(&format!("\"00000000-0000-0000-0000-{suffix:012}\"")).unwrap()
        };
        let observation = |song_id, support| JointEvidenceObservation {
            catalog_song_count: 100,
            candidates: vec![JointEvidenceCandidate {
                song_id,
                chart: scorepeek::catalog::Chart {
                    key: scorepeek::catalog::ChartKey {
                        play_type: PlayType::Single,
                        difficulty: Difficulty::Hyper,
                    },
                    level: 8,
                    notes: 764,
                },
                display_titles: vec![format!("SONG {song_id:?}")],
                artist: "ARTIST".to_owned(),
                family_support: BTreeMap::from([(EvidenceFamily::SelectTitleLexical, support)]),
                support,
            }],
        };
        let incumbent = song(1);
        let successor = song(2);
        let mut epochs = SelectionEpochTracker::default();
        epochs.observe(100, &observation(incumbent, 300), None);
        epochs.observe(200, &observation(successor, 70), None);
        assert_eq!(
            epochs.handoff().summary().selected.unwrap().song_id,
            successor
        );

        epochs.observe(300, &observation(incumbent, 300), None);
        assert!(epochs.successor.candidates.is_empty());
        assert_eq!(
            epochs.handoff().summary().selected.unwrap().song_id,
            incumbent
        );
    }

    #[test]
    fn accepted_hypothesis_can_return_to_conflict_on_new_contradictory_evidence() {
        let first_song = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        let second_song = serde_json::from_str("\"00000000-0000-0000-0000-000000000002\"").unwrap();
        let candidate = |song_id, family| JointEvidenceCandidate {
            song_id,
            chart: scorepeek::catalog::Chart {
                key: scorepeek::catalog::ChartKey {
                    play_type: PlayType::Single,
                    difficulty: Difficulty::Hyper,
                },
                level: 8,
                notes: 764,
            },
            display_titles: vec!["TEST".to_owned()],
            artist: "ARTIST".to_owned(),
            family_support: BTreeMap::from([(family, 300)]),
            support: 300,
        };
        let mut accumulator = HypothesisAccumulator::default();
        accumulator.observe(
            100,
            &JointEvidenceObservation {
                catalog_song_count: 0,
                candidates: vec![candidate(first_song, EvidenceFamily::ResultTitle)],
            },
            None,
            None,
        );
        assert_eq!(
            accumulator.summary().state,
            ResolverResolutionState::AcceptedJoint
        );
        accumulator.observe(
            200,
            &JointEvidenceObservation {
                catalog_song_count: 0,
                candidates: vec![candidate(second_song, EvidenceFamily::ResultArtist)],
            },
            None,
            None,
        );
        assert_eq!(
            accumulator.summary().state,
            ResolverResolutionState::Conflict
        );
    }

    #[test]
    fn resolver_transition_records_raw_and_normalized_family_contributions() {
        let temporary = tempfile::tempdir().unwrap();
        let state = state();
        let channel = ObservationChannel::start_at(temporary.path(), Arc::clone(&state)).unwrap();
        let stream = UnixStream::connect(&channel.socket_path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut snapshot = String::new();
        reader.read_line(&mut snapshot).unwrap();
        let mut output = test_output(state, channel);

        output.publish(&accepted_result_event(1)).unwrap();
        assert_eq!(read_raw_event(&mut reader)["event"], "field_observation");
        let transition = read_raw_event(&mut reader);
        assert_eq!(transition["event"], "resolver_state_changed");
        assert_eq!(transition["scope"], "result");
        assert_eq!(transition["state"], "accepted_joint");
        assert_eq!(
            transition["selected_family_support"]["result_title"]["raw"],
            120
        );
        assert_eq!(
            transition["selected_family_support"]["result_title"]["normalized"],
            120
        );
        assert_eq!(transition["observation_count"], 1);
    }
}
