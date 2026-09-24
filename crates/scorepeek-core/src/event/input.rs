//! Inputs that can change portable domain state.

use serde_json::{Map, Value};

use crate::game_version::GameVersionState;
use crate::recognition::result::ParsedResultFields;
use crate::recognition::screen::ResultPanelSide;
use crate::recognition::shared::JointEvidenceObservation;
use crate::session::timeline::SemanticEpisodePhase;

use super::{RUN_EVENT_SCHEMA, RunEvent, RunEventKind, SongResolutionPresentation};

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
        screen: String,
        result_panel_side: Option<ResultPanelSide>,
    },
    SemanticScreenEpisodeChanged {
        session_id: Option<String>,
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: String,
        phase: SemanticEpisodePhase,
    },
    ScreenChanged {
        session_id: Option<String>,
        screen_episode_id: u64,
        sequence: u64,
        monotonic_end_ms: u64,
        screen: String,
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
        screen: String,
        fields: Value,
        parsed_result_fields: Option<Box<ParsedResultFields>>,
        joint_evidence: JointEvidenceObservation,
    },
}

impl DomainInput {
    /// Selects only semantic input fields from the diagnostic and output event contract.
    #[must_use]
    pub fn from_run_event(event: &RunEvent) -> Option<Self> {
        Some(match &event.kind {
            RunEventKind::SessionStarted {
                session_id: Some(session_id),
            }
            | RunEventKind::CanonicalSessionStarted { session_id } => Self::SessionStarted {
                session_id: session_id.clone(),
            },
            RunEventKind::SessionFinished { session_id, .. }
            | RunEventKind::CanonicalSessionFinished { session_id } => Self::SessionFinished {
                session_id: session_id.clone(),
            },
            RunEventKind::WatcherStopped { .. } => Self::WatcherFinished,
            RunEventKind::GameVersionChanged { version, .. } => {
                Self::GameVersionState(GameVersionState::Identified(version.clone()))
            }
            RunEventKind::RawScreenObserved {
                session_id,
                semantic_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
                result_presence,
                ..
            } => Self::RawScreenObserved {
                session_id: session_id.clone(),
                semantic_episode_id: *semantic_episode_id,
                sequence: *sequence,
                monotonic_end_ms: *monotonic_end_ms,
                screen: screen.clone(),
                result_panel_side: result_presence
                    .as_ref()
                    .and_then(|presence| presence.panel_side.known()),
            },
            RunEventKind::SemanticScreenEpisodeChanged {
                session_id,
                screen_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
                phase,
            } => Self::SemanticScreenEpisodeChanged {
                session_id: session_id.clone(),
                screen_episode_id: *screen_episode_id,
                sequence: *sequence,
                monotonic_end_ms: *monotonic_end_ms,
                screen: screen.clone(),
                phase: *phase,
            },
            RunEventKind::ScreenChanged {
                session_id,
                screen_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
                ..
            } => Self::ScreenChanged {
                session_id: session_id.clone(),
                screen_episode_id: *screen_episode_id,
                sequence: *sequence,
                monotonic_end_ms: *monotonic_end_ms,
                screen: screen.clone(),
            },
            RunEventKind::ScreenTick {
                sequence,
                monotonic_end_ms,
                ..
            } => Self::ScreenTick {
                sequence: *sequence,
                monotonic_end_ms: *monotonic_end_ms,
            },
            RunEventKind::FieldObservation {
                session_id,
                screen_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
                fields,
                parsed_result_fields,
                joint_evidence,
                ..
            } => Self::FieldObservation {
                session_id: session_id.clone(),
                screen_episode_id: *screen_episode_id,
                sequence: *sequence,
                monotonic_end_ms: *monotonic_end_ms,
                screen: screen.clone(),
                fields: domain_fields(screen, fields),
                parsed_result_fields: parsed_result_fields.clone().map(Box::new),
                joint_evidence: joint_evidence.clone(),
            },
            _ => return None,
        })
    }

    pub(super) fn as_reducer_event(&self) -> Option<RunEvent> {
        let kind = match self {
            Self::SessionStarted { session_id } => RunEventKind::SessionStarted {
                session_id: Some(session_id.clone()),
            },
            Self::SessionFinished { session_id } => RunEventKind::CanonicalSessionFinished {
                session_id: session_id.clone(),
            },
            Self::WatcherFinished | Self::GameVersionState(_) => return None,
            Self::RawScreenObserved {
                session_id,
                semantic_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
                ..
            } => RunEventKind::RawScreenObserved {
                session_id: session_id.clone(),
                semantic_episode_id: *semantic_episode_id,
                sequence: *sequence,
                monotonic_start_ms: *monotonic_end_ms,
                monotonic_end_ms: *monotonic_end_ms,
                screen: screen.clone(),
                result_presence: None,
                play_presence: None,
                unknown_reason: None,
            },
            Self::SemanticScreenEpisodeChanged {
                session_id,
                screen_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
                phase,
            } => RunEventKind::SemanticScreenEpisodeChanged {
                session_id: session_id.clone(),
                screen_episode_id: *screen_episode_id,
                sequence: *sequence,
                monotonic_end_ms: *monotonic_end_ms,
                screen: screen.clone(),
                phase: *phase,
            },
            Self::ScreenChanged {
                session_id,
                screen_episode_id,
                sequence,
                monotonic_end_ms,
                screen,
            } => RunEventKind::ScreenChanged {
                session_id: session_id.clone(),
                screen_episode_id: *screen_episode_id,
                sequence: *sequence,
                monotonic_start_ms: *monotonic_end_ms,
                monotonic_end_ms: *monotonic_end_ms,
                screen: screen.clone(),
            },
            Self::ScreenTick {
                sequence,
                monotonic_end_ms,
            } => RunEventKind::ScreenTick {
                screen_episode_id: 0,
                sequence: *sequence,
                monotonic_end_ms: *monotonic_end_ms,
                screen: String::new(),
            },
            Self::FieldObservation { .. } => self.field_reducer_kind()?,
        };
        Some(RunEvent {
            schema: RUN_EVENT_SCHEMA.to_owned(),
            kind,
        })
    }

    fn field_reducer_kind(&self) -> Option<RunEventKind> {
        let Self::FieldObservation {
            session_id,
            screen_episode_id,
            sequence,
            monotonic_end_ms,
            screen,
            fields,
            parsed_result_fields,
            joint_evidence,
        } = self
        else {
            return None;
        };
        Some(RunEventKind::FieldObservation {
            session_id: session_id.clone(),
            screen_episode_id: *screen_episode_id,
            sequence: *sequence,
            monotonic_start_ms: *monotonic_end_ms,
            monotonic_end_ms: *monotonic_end_ms,
            screen: screen.clone(),
            fields: fields.clone(),
            result_song_resolution: Value::Null,
            music_select_song_resolution: Value::Null,
            parsed_result_fields: parsed_result_fields
                .as_ref()
                .map(|fields| fields.as_ref().clone()),
            result_chart_resolution: None,
            result_performance_resolution: None,
            current_score_ocr_resolution: None,
            numeric_batch: None,
            joint_evidence: joint_evidence.clone(),
            processing_timing: Value::Null,
            song_resolution_presentation: Box::new(SongResolutionPresentation::Unknown {
                reason: Value::Null,
                selected: None,
                runner_up: None,
                evidence_summary: None,
            }),
        })
    }
}

fn domain_fields(screen: &str, fields: &Value) -> Value {
    let keys: &[&str] = match screen {
        "result" => &["panel_side", "play_options", "clear_type"],
        "music_select" => &["selected_difficulty", "play_type", "play_side", "best"],
        _ => &[],
    };
    Value::Object(
        keys.iter()
            .filter_map(|key| {
                fields
                    .get(*key)
                    .map(|value| ((*key).to_owned(), value.clone()))
            })
            .collect::<Map<String, Value>>(),
    )
}
