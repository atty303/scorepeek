use std::fmt::Write as _;

use super::{
    Deserialize, Difficulty, Line, MusicSelectionState, PlayType, ScorepeekSongId, Serialize,
    SongPresentation, difficulty_label, fitted_value, play_type_label,
};
use scorepeek::recognition::{BestClearType, BestValue, MusicSelectBestValues, StableBestField};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BestChart {
    pub scorepeek_song_id: ScorepeekSongId,
    pub play_type: PlayType,
    pub difficulty: Difficulty,
    pub notes: u32,
    pub presentation: SongPresentation,
}

impl BestChart {
    fn from_selection(selection: MusicSelectionState) -> Option<Self> {
        let MusicSelectionState::Selected {
            scorepeek_song_id,
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
    pub capture_generation: u64,
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

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MusicSelectResolverState {
    pub active: bool,
    pub suspended: bool,
    pub screen_episode_id: u64,
    pub selection_interval: u64,
    pub chart: Option<BestChart>,
    pub identity_status: SelectIdentityStatus,
    pub current_difficulty: Option<super::CurrentSelectionDifficulty>,
    pub difficulty_target: Option<super::SelectionDifficultyTarget>,
    pub score: StableBestField<u32>,
    pub miss_count: StableBestField<u32>,
    pub clear_type: StableBestField<BestClearType>,
    pub output: BestOutputState,
    pub revision: u64,
    pub snapshot: Option<MusicSelectBestSnapshot>,
    #[serde(skip)]
    pub(super) last_published: Option<MusicSelectBestSnapshot>,
}

impl MusicSelectResolverState {
    pub fn observe(
        &mut self,
        selection: Option<MusicSelectionState>,
        values: MusicSelectBestValues,
    ) {
        let chart = selection.and_then(BestChart::from_selection);
        if self.chart != chart {
            self.selection_interval = self.selection_interval.saturating_add(1);
            self.score = StableBestField::default();
            self.miss_count = StableBestField::default();
            self.clear_type = StableBestField::default();
            self.snapshot = None;
            self.last_published = None;
            self.revision = 0;
            self.chart = chart;
        }
        self.identity_status = if self.chart.is_some() {
            SelectIdentityStatus::Resolved
        } else {
            SelectIdentityStatus::AwaitingEvidence
        };
        let Some(chart) = self.chart.as_ref() else {
            self.output = BestOutputState::IdentityUnresolved;
            return;
        };
        let score = match values.score {
            BestValue::Known(score) if u64::from(score) > u64::from(chart.notes) * 2 => {
                BestValue::Unknown
            }
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
            self.snapshot = None;
        }
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
        capture_generation: u64,
        source_sequence: u64,
        observed_monotonic_ms: u64,
    ) -> Option<MusicSelectBestSnapshot> {
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
                scorepeek::recognition::dj_rank(*score, chart.notes).map(str::to_owned)
            }
            _ => None,
        };
        let snapshot = MusicSelectBestSnapshot {
            contract: "scorepeek-music-select-best-snapshot-v1".to_owned(),
            source: "music_select".to_owned(),
            layout: "scorepeek-music-select-best-layout-v1".to_owned(),
            observation_id: format!(
                "{session_id}:{capture_generation}:{}:{}:{}",
                self.screen_episode_id, self.selection_interval, self.revision
            ),
            session_id: session_id.to_owned(),
            capture_generation,
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

fn field_label<T>(field: &StableBestField<T>, show: impl FnOnce(&T) -> String) -> String {
    let value = match &field.observed {
        BestValue::Known(value) => show(value),
        BestValue::NoRecord => "no record".to_owned(),
        BestValue::NotDisplayed => "not displayed".to_owned(),
        BestValue::Unknown => if field.observed_once {
            "unknown"
        } else {
            "waiting"
        }
        .to_owned(),
    };
    if field.consecutive == 1 {
        format!("{value} (1/2)")
    } else {
        value
    }
}

pub fn lines(state: &MusicSelectResolverState, width: usize) -> Vec<Line<'static>> {
    if !state.active {
        return vec![Line::from("inactive")];
    }
    let phase = if state.suspended {
        "suspended"
    } else {
        "active"
    };
    let mut heading = format!(
        "{phase} ep#{} selection#{}",
        state.screen_episode_id, state.selection_interval
    );
    if state.chart.is_none()
        && let Some(current) = state.current_difficulty
    {
        let _ = write!(
            heading,
            " {} streak={} target={}",
            difficulty_label(current.difficulty),
            current.consecutive_known,
            match state.difficulty_target {
                Some(super::SelectionDifficultyTarget::Pending) => "pending",
                Some(super::SelectionDifficultyTarget::Incumbent) => "incumbent",
                Some(super::SelectionDifficultyTarget::Successor) => "successor",
                None => "-",
            }
        );
    }
    let chart = state.chart.as_ref().map_or_else(
        || format!("identity unresolved: {:?}", state.identity_status),
        |c| {
            fitted_value(
                &format!(
                    "{} {} / ",
                    play_type_label(c.play_type),
                    difficulty_label(c.difficulty)
                ),
                c.presentation
                    .display_titles
                    .first()
                    .map_or("?", String::as_str),
                width,
            )
        },
    );
    let values = format!(
        "SCORE {}  MISS {}",
        field_label(&state.score, ToString::to_string),
        field_label(&state.miss_count, ToString::to_string)
    );
    let clear = field_label(&state.clear_type, |c| format!("{c:?}"));
    let rank = state
        .snapshot
        .as_ref()
        .and_then(|s| s.derived_dj_rank.as_deref())
        .unwrap_or("?");
    let output = match state.output {
        BestOutputState::IdentityUnresolved => "waiting: identity",
        BestOutputState::Stabilizing => "waiting: values",
        BestOutputState::Partial => "partial snapshot emitted",
        BestOutputState::Complete => "snapshot emitted",
    };
    vec![
        Line::from(heading),
        Line::from(chart),
        Line::from(values),
        Line::from(format!("{clear}  DJ {rank} (derived)")),
        Line::from(format!("{output} revision={}", state.revision)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selected(difficulty: Difficulty) -> MusicSelectionState {
        let song = serde_json::from_str("\"00000000-0000-0000-0000-000000000001\"").unwrap();
        MusicSelectionState::Selected {
            scorepeek_song_id: song,
            play_type: PlayType::Single,
            difficulty,
            level: 10,
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
            ..Default::default()
        };
        state.observe(Some(selected(Difficulty::Hyper)), values(1500));
        assert!(state.publish_candidate("session", 1, 1, 100).is_none());
        state.observe(Some(selected(Difficulty::Hyper)), values(1500));
        let first = state.publish_candidate("session", 1, 2, 200).unwrap();
        assert_eq!(first.derived_dj_rank.as_deref(), Some("A"));
        assert_eq!(state.output, BestOutputState::Partial);
        assert!(state.publish_candidate("session", 1, 3, 300).is_none());
        state.observe(Some(selected(Difficulty::Another)), values(1600));
        assert!(state.snapshot.is_none());
        assert_eq!(state.score.consecutive, 1);
        state.observe(None, values(1600));
        assert_eq!(state.output, BestOutputState::IdentityUnresolved);
        for _ in 0..2 {
            state.observe(Some(selected(Difficulty::Hyper)), values(1500));
        }
        let revisit = state.publish_candidate("session", 1, 6, 600).unwrap();
        assert_ne!(first.observation_id, revisit.observation_id);
        assert_eq!(first.values, revisit.values);
    }
    #[test]
    fn unknown_gap_does_not_reemit_identical_content() {
        let mut state = MusicSelectResolverState::default();
        for _ in 0..2 {
            state.observe(Some(selected(Difficulty::Hyper)), values(1500));
        }
        let first = state.publish_candidate("session", 1, 2, 200).unwrap();
        state.observe(
            Some(selected(Difficulty::Hyper)),
            MusicSelectBestValues::default(),
        );
        for _ in 0..2 {
            state.observe(Some(selected(Difficulty::Hyper)), values(1500));
        }
        assert!(state.publish_candidate("session", 1, 5, 500).is_none());
        assert_eq!(state.snapshot.as_ref(), Some(&first));
    }

    #[test]
    fn pane_distinguishes_wait_unknown_pending_and_explicit_absence() {
        let mut field = StableBestField::default();
        assert_eq!(field_label(&field, u32::to_string), "waiting");
        field.observe(BestValue::Unknown);
        assert_eq!(field_label(&field, u32::to_string), "unknown");
        field.observe(BestValue::Known(12));
        assert_eq!(field_label(&field, u32::to_string), "12 (1/2)");
        field.observe(BestValue::Known(12));
        assert_eq!(field_label(&field, u32::to_string), "12");
        for _ in 0..2 {
            field.observe(BestValue::NoRecord);
        }
        assert_eq!(field_label(&field, u32::to_string), "no record");
        for _ in 0..2 {
            field.observe(BestValue::NotDisplayed);
        }
        assert_eq!(field_label(&field, u32::to_string), "not displayed");
    }

    #[test]
    fn unknown_clears_current_values_without_fabricating_a_history_result() {
        let mut state = MusicSelectResolverState::default();
        for _ in 0..2 {
            state.observe(Some(selected(Difficulty::Hyper)), values(1500));
        }
        assert!(state.publish_candidate("session", 1, 2, 200).is_some());
        state.observe(
            Some(selected(Difficulty::Hyper)),
            MusicSelectBestValues::default(),
        );
        assert!(state.snapshot.is_none());
        assert_eq!(state.output, BestOutputState::Stabilizing);
        for _ in 0..2 {
            state.observe(Some(selected(Difficulty::Hyper)), values(2001));
        }
        let partial = state.publish_candidate("session", 1, 5, 500).unwrap();
        assert_eq!(partial.values.score, BestValue::Unknown);
        assert_eq!(
            partial.values.clear_type,
            BestValue::Known(BestClearType::Clear)
        );
    }
}
