//! Portable run-domain values independent of runtime transport and presentation.

use crate::catalog::{Difficulty, PlayType, ScorepeekSongId};
use crate::recognition::music_select::{
    BestClearType, BestValue, MusicSelectBestValues, PlaySide, StableBestField,
};
use crate::recognition::result::{
    PlayOptions, PreviousBest, ResultJudgments, ResultTiming, SupplementalResultValue,
};
use crate::recognition::screen::ResultPanelSide;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct DomainEvent {
    pub event: String,
    #[serde(flatten)]
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SongPresentation {
    pub scorepeek_song_id: ScorepeekSongId,
    pub display_titles: Vec<String>,
    pub artist: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicSelectionUnresolvedReason {
    EvidenceUnresolved,
    EpisodeEnded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MusicSelectionState {
    Unresolved {
        reason: MusicSelectionUnresolvedReason,
    },
    Selected {
        scorepeek_song_id: ScorepeekSongId,
        play_side: PlaySide,
        play_type: PlayType,
        difficulty: Difficulty,
        level: u8,
        notes: u32,
        presentation: SongPresentation,
    },
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
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResultPanelSideEpisodeState {
    Pending,
    Stable { side: ResultPanelSide },
    Conflicted { stable_side: ResultPanelSide },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultPanelSideTransitionReason {
    CandidateStarted,
    CandidateRepeated,
    Accepted,
    OppositeObserved,
    Conflict,
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
    pub play_side: PlaySide,
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
    pub play_options: PlayOptions,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultRetractionReason {
    EvidenceUnresolved,
    PanelSideConflict,
    AttemptRejected,
    SessionEnded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResultState {
    Inactive,
    Provisional {
        #[serde(skip_serializing_if = "Option::is_none")]
        song: Option<SongPresentation>,
        result: Box<ResultDomainEvent>,
    },
    Retracted {
        #[serde(skip_serializing_if = "Option::is_none")]
        song: Option<SongPresentation>,
        result: Box<ResultDomainEvent>,
        reason: ResultRetractionReason,
    },
    Confirmed {
        #[serde(skip_serializing_if = "Option::is_none")]
        song: Option<SongPresentation>,
        result: Box<ResultDomainEvent>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolverResolutionState {
    Unresolved,
    SongProjected,
    JointCandidate,
    AcceptedJoint,
    Conflict,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceContribution {
    raw: u64,
    normalized: u16,
}

impl EvidenceContribution {
    #[must_use]
    pub const fn new(raw: u64, normalized: u16) -> Self {
        Self { raw, normalized }
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.raw
    }

    #[must_use]
    pub const fn normalized(self) -> u16 {
        self.normalized
    }
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
    chart: crate::catalog::ChartKey,
}

impl ResolverHypothesisKey {
    #[must_use]
    pub const fn new(song_id: ScorepeekSongId, chart: crate::catalog::ChartKey) -> Self {
        Self { song_id, chart }
    }

    #[must_use]
    pub const fn song_id(&self) -> ScorepeekSongId {
        self.song_id
    }

    #[must_use]
    pub const fn chart(&self) -> crate::catalog::ChartKey {
        self.chart
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SongResolutionPresentation {
    Accepted {
        reason: Option<serde_json::Value>,
        selected: SongPresentation,
        runner_up: SongPresentation,
        evidence_summary: String,
    },
    Unknown {
        reason: serde_json::Value,
        selected: Option<SongPresentation>,
        runner_up: Option<SongPresentation>,
        evidence_summary: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionDifficultyTarget {
    Pending,
    Incumbent,
    Successor,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionDifficultyTransitionReason {
    Changed,
    PendingApplied,
    TargetSwitch,
    Reset,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CurrentSelectionDifficulty {
    pub difficulty: Difficulty,
    pub consecutive_known: u32,
    first_sequence: u64,
    last_sequence: u64,
    first_monotonic_ms: u64,
    last_monotonic_ms: u64,
}

impl CurrentSelectionDifficulty {
    #[must_use]
    pub const fn observed(difficulty: Difficulty, sequence: u64, monotonic_ms: u64) -> Self {
        Self {
            difficulty,
            consecutive_known: 1,
            first_sequence: sequence,
            last_sequence: sequence,
            first_monotonic_ms: monotonic_ms,
            last_monotonic_ms: monotonic_ms,
        }
    }

    pub fn observe(&mut self, difficulty: Difficulty, sequence: u64, monotonic_ms: u64) -> bool {
        if sequence <= self.last_sequence || monotonic_ms < self.last_monotonic_ms {
            return false;
        }
        if self.difficulty != difficulty {
            *self = Self::observed(difficulty, sequence, monotonic_ms);
            return true;
        }
        self.consecutive_known = self.consecutive_known.saturating_add(1);
        self.last_sequence = sequence;
        self.last_monotonic_ms = monotonic_ms;
        false
    }

    #[must_use]
    pub fn support(self) -> u64 {
        u64::from(self.consecutive_known).saturating_mul(50)
    }

    #[must_use]
    pub const fn last_sequence(self) -> u64 {
        self.last_sequence
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BestChart {
    pub scorepeek_song_id: ScorepeekSongId,
    pub play_side: PlaySide,
    pub play_type: PlayType,
    pub difficulty: Difficulty,
    pub notes: u32,
    pub presentation: SongPresentation,
}

impl BestChart {
    #[must_use]
    pub fn from_selection(selection: MusicSelectionState) -> Option<Self> {
        let MusicSelectionState::Selected {
            scorepeek_song_id,
            play_side,
            play_type,
            difficulty,
            notes,
            presentation,
            ..
        } = selection
        else {
            return None;
        };
        Some(Self {
            scorepeek_song_id,
            play_side,
            play_type,
            difficulty,
            notes,
            presentation,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MusicSelectBestSnapshot {
    pub contract: String,
    pub source: String,
    pub layout: String,
    pub observation_id: String,
    pub session_id: String,
    pub screen_episode_id: u64,
    pub selection_interval: u64,
    pub source_sequence: u64,
    pub observed_monotonic_ms: u64,
    pub revision: u64,
    pub chart: BestChart,
    pub values: MusicSelectBestValues,
    pub derived_dj_rank: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BestOutputState {
    #[default]
    IdentityUnresolved,
    Stabilizing,
    Partial,
    Complete,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectIdentityStatus {
    #[default]
    AwaitingEvidence,
    AwaitingDifficulty,
    AwaitingPlayType,
    Stabilizing,
    CurrentFrameConflict,
    Resolved,
}

pub enum SelectFrameIdentity {
    Confirmed(BestChart),
    Missing(SelectIdentityStatus),
    Conflicting(SelectIdentityStatus),
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MusicSelectResolverState {
    pub active: bool,
    pub suspended: bool,
    pub screen_episode_id: u64,
    pub selection_interval: u64,
    pub chart: Option<BestChart>,
    pub identity_status: SelectIdentityStatus,
    pub current_difficulty: Option<CurrentSelectionDifficulty>,
    pub difficulty_target: Option<SelectionDifficultyTarget>,
    pub score: StableBestField<u32>,
    pub miss_count: StableBestField<u32>,
    pub clear_type: StableBestField<BestClearType>,
    pub output: BestOutputState,
    pub revision: u64,
    pub snapshot: Option<MusicSelectBestSnapshot>,
    #[serde(skip)]
    last_published: Option<MusicSelectBestSnapshot>,
}

impl MusicSelectResolverState {
    pub fn hold(&mut self, reason: SelectIdentityStatus) {
        self.score = StableBestField::default();
        self.miss_count = StableBestField::default();
        self.clear_type = StableBestField::default();
        self.identity_status = reason;
        self.output = BestOutputState::IdentityUnresolved;
        self.snapshot = self.last_published.clone();
    }

    pub fn end_interval(&mut self, reason: SelectIdentityStatus) {
        self.hold(reason);
        self.chart = None;
        self.snapshot = None;
        self.last_published = None;
        self.revision = 0;
    }

    pub fn observe_frame(&mut self, identity: SelectFrameIdentity, values: MusicSelectBestValues) {
        match identity {
            SelectFrameIdentity::Confirmed(chart) => self.observe(chart, values),
            SelectFrameIdentity::Missing(reason) => self.hold(reason),
            SelectFrameIdentity::Conflicting(reason) => self.end_interval(reason),
        }
    }

    pub fn observe(&mut self, chart: BestChart, values: MusicSelectBestValues) {
        let maximum_score = u64::from(chart.notes) * 2;
        if self.chart.as_ref() != Some(&chart) {
            self.end_interval(SelectIdentityStatus::Stabilizing);
            self.selection_interval = self.selection_interval.saturating_add(1);
            self.chart = Some(chart);
        }
        self.identity_status = SelectIdentityStatus::Resolved;
        let score = match values.score {
            BestValue::Known(score) if u64::from(score) > maximum_score => BestValue::Unknown,
            value => value,
        };
        self.score.observe(score);
        self.miss_count.observe(values.miss_count);
        self.clear_type.observe(values.clear_type);
        let values = self.values();
        self.output = if !values.has_observed_value() {
            BestOutputState::Stabilizing
        } else if values.score == BestValue::Unknown
            || values.miss_count == BestValue::Unknown
            || values.clear_type == BestValue::Unknown
        {
            BestOutputState::Partial
        } else {
            BestOutputState::Complete
        };
        if !values.has_observed_value() {
            self.snapshot = self.last_published.clone();
        }
    }

    #[must_use]
    pub fn same_notification(&self, previous: &Self) -> bool {
        let difficulty = |state: &Self| {
            state.current_difficulty.map(|current| {
                (
                    current.difficulty,
                    if state.chart.is_none() {
                        current.consecutive_known
                    } else {
                        0
                    },
                )
            })
        };
        self.active == previous.active
            && self.suspended == previous.suspended
            && self.screen_episode_id == previous.screen_episode_id
            && self.selection_interval == previous.selection_interval
            && self.chart == previous.chart
            && self.identity_status == previous.identity_status
            && difficulty(self) == difficulty(previous)
            && self.difficulty_target == previous.difficulty_target
            && self.score == previous.score
            && self.miss_count == previous.miss_count
            && self.clear_type == previous.clear_type
            && self.output == previous.output
            && self.revision == previous.revision
            && self.snapshot == previous.snapshot
    }

    fn values(&self) -> MusicSelectBestValues {
        MusicSelectBestValues {
            score: self.score.accepted(),
            miss_count: self.miss_count.accepted(),
            clear_type: self.clear_type.accepted(),
        }
    }

    pub fn publish_candidate(
        &mut self,
        session_id: &str,
        source_sequence: u64,
        observed_monotonic_ms: u64,
    ) -> Option<MusicSelectBestSnapshot> {
        if self.suspended || self.identity_status != SelectIdentityStatus::Resolved {
            return None;
        }
        let chart = self.chart.clone()?;
        let values = self.values();
        if !values.has_observed_value() {
            return None;
        }
        if let Some(previous) = &self.last_published
            && previous.values == values
        {
            self.snapshot = Some(previous.clone());
            return None;
        }
        self.revision = self.revision.saturating_add(1);
        let derived_dj_rank = match (&values.score, &values.clear_type) {
            (_, BestValue::Known(BestClearType::NoPlay)) => None,
            (BestValue::Known(score), _) => {
                crate::recognition::music_select::dj_rank(*score, chart.notes).map(str::to_owned)
            }
            _ => None,
        };
        let snapshot = MusicSelectBestSnapshot {
            contract: "scorepeek-music-select-best-snapshot-v4".to_owned(),
            source: "music_select".to_owned(),
            layout: "scorepeek-music-select-best-layout-v1".to_owned(),
            observation_id: format!(
                "{session_id}:{}:{}:{}",
                self.screen_episode_id, self.selection_interval, self.revision
            ),
            session_id: session_id.to_owned(),
            screen_episode_id: self.screen_episode_id,
            selection_interval: self.selection_interval,
            source_sequence,
            observed_monotonic_ms,
            revision: self.revision,
            chart,
            values,
            derived_dj_rank,
        };
        self.snapshot = Some(snapshot.clone());
        self.last_published = Some(snapshot.clone());
        Some(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(difficulty: Difficulty) -> BestChart {
        let song = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        BestChart {
            scorepeek_song_id: song,
            play_side: PlaySide::OnePlayer,
            play_type: PlayType::Single,
            difficulty,
            notes: 1000,
            presentation: SongPresentation {
                scorepeek_song_id: song,
                display_titles: vec!["TEST".into()],
                artist: "ARTIST".into(),
            },
        }
    }

    fn values(score: u32) -> MusicSelectBestValues {
        MusicSelectBestValues {
            score: BestValue::Known(score),
            miss_count: BestValue::Unknown,
            clear_type: BestValue::Known(BestClearType::Clear),
        }
    }

    #[test]
    fn partial_snapshots_deduplicate_and_revisit_has_a_new_identity() {
        let mut state = MusicSelectResolverState {
            active: true,
            screen_episode_id: 2,
            ..MusicSelectResolverState::default()
        };
        state.observe(selected(Difficulty::Hyper), values(1500));
        assert!(state.publish_candidate("session", 1, 100).is_none());
        state.observe(selected(Difficulty::Hyper), values(1500));
        let first = state.publish_candidate("session", 2, 200).unwrap();
        assert_eq!(first.derived_dj_rank.as_deref(), Some("A"));
        assert_eq!(state.output, BestOutputState::Partial);
        assert!(state.publish_candidate("session", 3, 300).is_none());
        state.observe(selected(Difficulty::Another), values(1600));
        assert!(state.snapshot.is_none());
        assert_eq!(state.score.consecutive, 1);
        state.end_interval(SelectIdentityStatus::CurrentFrameConflict);
        assert_eq!(state.output, BestOutputState::IdentityUnresolved);
        for _ in 0..2 {
            state.observe(selected(Difficulty::Hyper), values(1500));
        }
        let revisit = state.publish_candidate("session", 6, 600).unwrap();
        assert_ne!(first.observation_id, revisit.observation_id);
        assert_eq!(first.values, revisit.values);
    }

    #[test]
    fn unknown_gap_does_not_reemit_identical_content() {
        let mut state = MusicSelectResolverState::default();
        for _ in 0..2 {
            state.observe(selected(Difficulty::Hyper), values(1500));
        }
        let first = state.publish_candidate("session", 2, 200).unwrap();
        state.observe(
            selected(Difficulty::Hyper),
            MusicSelectBestValues::default(),
        );
        for _ in 0..2 {
            state.observe(selected(Difficulty::Hyper), values(1500));
        }
        assert!(state.publish_candidate("session", 5, 500).is_none());
        assert_eq!(state.snapshot.as_ref(), Some(&first));
    }

    #[test]
    fn missing_identity_retains_publication_but_restarts_fields() {
        let mut state = MusicSelectResolverState {
            active: true,
            ..MusicSelectResolverState::default()
        };
        for _ in 0..2 {
            state.observe(selected(Difficulty::Hyper), values(1500));
        }
        let first = state.publish_candidate("session", 2, 200).unwrap();
        for reason in [
            SelectIdentityStatus::AwaitingDifficulty,
            SelectIdentityStatus::AwaitingPlayType,
            SelectIdentityStatus::AwaitingEvidence,
        ] {
            state.observe_frame(SelectFrameIdentity::Missing(reason), values(1600));
            assert_eq!(state.selection_interval, first.selection_interval);
            assert_eq!(state.snapshot.as_ref(), Some(&first));
            assert_eq!(state.score.consecutive, 0);
            assert!(state.publish_candidate("session", 3, 300).is_none());
            state.observe(selected(Difficulty::Hyper), values(1500));
            assert!(state.publish_candidate("session", 4, 400).is_none());
            state.observe(selected(Difficulty::Hyper), values(1500));
            assert!(state.publish_candidate("session", 5, 500).is_none());
        }
        state.observe_frame(
            SelectFrameIdentity::Conflicting(SelectIdentityStatus::CurrentFrameConflict),
            values(1500),
        );
        assert!(state.chart.is_none() && state.snapshot.is_none());
        for _ in 0..2 {
            state.observe(selected(Difficulty::Hyper), values(1500));
        }
        let revisit = state.publish_candidate("session", 8, 800).unwrap();
        assert_eq!(revisit.revision, 1);
        assert_ne!(revisit.selection_interval, first.selection_interval);
    }

    #[test]
    fn invalid_score_fails_closed_without_fabricating_history() {
        let mut state = MusicSelectResolverState::default();
        for _ in 0..2 {
            state.observe(selected(Difficulty::Hyper), values(1500));
        }
        assert!(state.publish_candidate("session", 2, 200).is_some());
        state.observe(
            selected(Difficulty::Hyper),
            MusicSelectBestValues::default(),
        );
        assert!(state.snapshot.is_some());
        assert_eq!(state.score.accepted(), BestValue::Unknown);
        assert_eq!(state.output, BestOutputState::Stabilizing);
        for _ in 0..2 {
            state.observe(selected(Difficulty::Hyper), values(2001));
        }
        let partial = state.publish_candidate("session", 5, 500).unwrap();
        assert_eq!(partial.values.score, BestValue::Unknown);
        assert_eq!(
            partial.values.clear_type,
            BestValue::Known(BestClearType::Clear)
        );
    }
}
