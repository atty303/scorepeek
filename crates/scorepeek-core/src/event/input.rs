//! Typed observations admitted by the domain coordinator.

use crate::catalog::{Difficulty, PlayType};
use crate::game_version::GameVersionState;
use crate::model::session::RegisteredScreenFieldObservation;
use crate::recognition::music_select::{MusicSelectBestObservation, PlaySide};
use crate::recognition::result::{ParsedResultFields, PlayOptionsObservation};
use crate::recognition::screen::{ResultPanelSide, ScreenClass, ScreenFieldObservations};
use crate::recognition::shared::JointEvidenceObservation;
use crate::session::timeline::SemanticEpisodePhase;

#[derive(Clone, Debug)]
pub enum DomainFieldObservation {
    Title,
    Result {
        panel_side: Option<ResultPanelSide>,
        play_options: Option<PlayOptionsObservation>,
        clear_type: Option<String>,
        parsed_fields: Option<Box<ParsedResultFields>>,
    },
    MusicSelect {
        selected_difficulty: Option<Difficulty>,
        play_type: Option<PlayType>,
        play_side: Option<PlaySide>,
        best: MusicSelectBestObservation,
    },
}

impl DomainFieldObservation {
    #[must_use]
    pub fn from_registered(observation: &RegisteredScreenFieldObservation) -> Self {
        match observation.fields() {
            ScreenFieldObservations::Title(_) => Self::Title,
            ScreenFieldObservations::Result(fields) => Self::Result {
                panel_side: Some(fields.panel_side),
                play_options: Some(fields.play_options.clone()),
                clear_type: observation.clear_type().map(str::to_owned),
                parsed_fields: observation.parsed_result_fields().cloned().map(Box::new),
            },
            ScreenFieldObservations::MusicSelect(fields) => Self::MusicSelect {
                selected_difficulty: fields.selected_difficulty.known(),
                play_type: fields.play_type.known(),
                play_side: fields.play_side.known(),
                best: fields.best.clone(),
            },
        }
    }
}

#[derive(Clone, Debug)]
pub enum DomainInput {
    SessionStarted {
        session_id: String,
    },
    SessionFinished {
        session_id: String,
    },
    WatcherFinished,
    GameVersionState(GameVersionState),
    RawScreenObserved {
        session_id: Option<String>,
        semantic_episode_id: Option<u64>,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
        result_panel_side: Option<ResultPanelSide>,
    },
    SemanticScreenEpisodeChanged {
        session_id: Option<String>,
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
        phase: SemanticEpisodePhase,
    },
    ScreenChanged {
        session_id: Option<String>,
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
    },
    ScreenTick {
        sequence: u64,
        monotonic_end_ms: u64,
    },
    FieldObservation {
        session_id: Option<String>,
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        observation: DomainFieldObservation,
        joint_evidence: JointEvidenceObservation,
    },
}

impl DomainInput {
    #[must_use]
    pub fn from_registered_field(
        session_id: &str,
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        observation: &RegisteredScreenFieldObservation,
    ) -> Self {
        Self::FieldObservation {
            session_id: Some(session_id.to_owned()),
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            observation: DomainFieldObservation::from_registered(observation),
            joint_evidence: observation.joint_evidence().clone(),
        }
    }
}
