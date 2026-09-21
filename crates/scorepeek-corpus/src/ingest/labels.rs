//! Complete replay-label schema and validation.

use crate::store::{
    COMPLETE_LABEL_SCHEMA, CorpusError, ErrorContext, ReplayFrame, valid_label_text,
    validate_opaque_id, validate_token,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenClass {
    Result,
    MusicSelect,
    Transition,
    Negative,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum LabelState<T> {
    Known { value: T },
    Unknown { reason: String },
    NotApplicable,
}

impl<T> LabelState<T> {
    fn validate(
        &self,
        field: &str,
        validate_known: impl FnOnce(&T) -> bool,
    ) -> Result<(), CorpusError> {
        match self {
            Self::Known { value } if validate_known(value) => Ok(()),
            Self::Known { .. } => Err(CorpusError::InvalidReplay(format!(
                "complete-label {field} has an invalid known value"
            ))),
            Self::Unknown { reason } if valid_label_text(reason) => Ok(()),
            Self::Unknown { .. } => Err(CorpusError::InvalidReplay(format!(
                "complete-label {field} has an invalid unknown reason"
            ))),
            Self::NotApplicable => Ok(()),
        }
    }

    fn require_applicable(&self, field: &str) -> Result<(), CorpusError> {
        if matches!(self, Self::NotApplicable) {
            return Err(CorpusError::InvalidReplay(format!(
                "complete-label {field} is mandatory for this shape"
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaySide {
    OnePlayer,
    TwoPlayer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMode {
    SinglePlay,
    DoublePlay,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayType {
    Single,
    Double,
    DoubleBattle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Beginner,
    Normal,
    Hyper,
    Another,
    Leggendaria,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompleteLabel {
    Result {
        schema: String,
        frame_id: String,
        annotation_revision: String,
        screen_state: LabelState<bool>,
        savable: LabelState<bool>,
        playside: LabelState<PlaySide>,
        play_mode: LabelState<PlayMode>,
        play_type: LabelState<PlayType>,
        song_id: LabelState<String>,
        difficulty: LabelState<Difficulty>,
        level: LabelState<u8>,
        notes: LabelState<u32>,
        current_score: LabelState<u32>,
    },
    MusicSelect {
        schema: String,
        frame_id: String,
        annotation_revision: String,
        screen_state: LabelState<bool>,
        play_mode: LabelState<PlayMode>,
        song_id: LabelState<String>,
        selected_difficulty: LabelState<Difficulty>,
        selected_level: LabelState<u8>,
    },
    NonRecognition {
        schema: String,
        frame_id: String,
        annotation_revision: String,
        screen_class: NonRecognitionClass,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelShape {
    Result,
    MusicSelect,
    NonRecognition,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompleteLabelSummary {
    pub schema: String,
    pub frame_id: String,
    pub annotation_revision: String,
    pub shape: LabelShape,
    pub labels_sha256: String,
    pub label_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NonRecognitionClass {
    Transition,
    Negative,
    Unknown,
}

impl CompleteLabel {
    pub(crate) fn summary_fields(&self) -> (&str, &str, LabelShape) {
        match self {
            Self::Result {
                frame_id,
                annotation_revision,
                ..
            } => (frame_id, annotation_revision, LabelShape::Result),
            Self::MusicSelect {
                frame_id,
                annotation_revision,
                ..
            } => (frame_id, annotation_revision, LabelShape::MusicSelect),
            Self::NonRecognition {
                frame_id,
                annotation_revision,
                ..
            } => (frame_id, annotation_revision, LabelShape::NonRecognition),
        }
    }

    pub(crate) fn validate_contents(&self) -> Result<(), CorpusError> {
        let (schema, frame_id, annotation_revision) = match self {
            Self::Result {
                schema,
                frame_id,
                annotation_revision,
                screen_state,
                savable,
                playside,
                play_mode,
                play_type,
                song_id,
                difficulty,
                level,
                notes,
                current_score,
            } => {
                validate_required_label(screen_state, "result.screen_state", |value| *value)?;
                validate_required_label(savable, "result.savable", |_| true)?;
                validate_required_label(playside, "result.playside", |_| true)?;
                validate_required_label(play_mode, "result.play_mode", |_| true)?;
                validate_required_label(play_type, "result.play_type", |_| true)?;
                validate_required_label(song_id, "result.song_id", |value| {
                    valid_label_text(value)
                })?;
                validate_required_label(difficulty, "result.difficulty", |_| true)?;
                validate_required_label(level, "result.level", |value| (1..=12).contains(value))?;
                validate_required_label(notes, "result.notes", |value| *value > 0)?;
                validate_required_label(current_score, "result.current_score", |_| true)?;
                validate_result_cross_fields(play_mode, play_type, notes, current_score)?;
                (schema, frame_id, annotation_revision)
            }
            Self::MusicSelect {
                schema,
                frame_id,
                annotation_revision,
                screen_state,
                play_mode,
                song_id,
                selected_difficulty,
                selected_level,
            } => {
                validate_required_label(screen_state, "music_select.screen_state", |value| *value)?;
                validate_required_label(play_mode, "music_select.play_mode", |_| true)?;
                validate_required_label(song_id, "music_select.song_id", |value| {
                    valid_label_text(value)
                })?;
                validate_required_label(
                    selected_difficulty,
                    "music_select.selected_difficulty",
                    |_| true,
                )?;
                validate_required_label(selected_level, "music_select.selected_level", |value| {
                    (1..=12).contains(value)
                })?;
                (schema, frame_id, annotation_revision)
            }
            Self::NonRecognition {
                schema,
                frame_id,
                annotation_revision,
                screen_class: _,
            } => (schema, frame_id, annotation_revision),
        };
        if schema != COMPLETE_LABEL_SCHEMA {
            return Err(CorpusError::InvalidReplay(format!(
                "complete-label schema must be {COMPLETE_LABEL_SCHEMA:?}"
            )));
        }
        validate_opaque_id(frame_id, "complete-label frame_id", ErrorContext::Replay)?;
        validate_token(
            annotation_revision,
            "complete-label annotation_revision",
            ErrorContext::Replay,
        )
    }

    pub(crate) fn validate_for(&self, frame: &ReplayFrame) -> Result<(), CorpusError> {
        self.validate_contents()?;
        let (frame_id, annotation_revision) = match self {
            Self::Result {
                frame_id,
                annotation_revision,
                ..
            } => {
                require_screen_class(frame, ScreenClass::Result)?;
                (frame_id, annotation_revision)
            }
            Self::MusicSelect {
                frame_id,
                annotation_revision,
                ..
            } => {
                require_screen_class(frame, ScreenClass::MusicSelect)?;
                (frame_id, annotation_revision)
            }
            Self::NonRecognition {
                frame_id,
                annotation_revision,
                screen_class,
                ..
            } => {
                let expected = match screen_class {
                    NonRecognitionClass::Transition => ScreenClass::Transition,
                    NonRecognitionClass::Negative => ScreenClass::Negative,
                    NonRecognitionClass::Unknown => ScreenClass::Unknown,
                };
                require_screen_class(frame, expected)?;
                (frame_id, annotation_revision)
            }
        };
        if frame_id != &frame.frame_id || annotation_revision != &frame.annotation_revision {
            return Err(CorpusError::InvalidReplay(
                "complete-label identity does not match its replay frame".to_owned(),
            ));
        }
        Ok(())
    }
}

pub(crate) fn validate_result_cross_fields(
    play_mode: &LabelState<PlayMode>,
    play_type: &LabelState<PlayType>,
    notes: &LabelState<u32>,
    current_score: &LabelState<u32>,
) -> Result<(), CorpusError> {
    if let (LabelState::Known { value: mode }, LabelState::Known { value: kind }) =
        (play_mode, play_type)
    {
        let compatible = matches!(
            (mode, kind),
            (PlayMode::SinglePlay, PlayType::Single)
                | (
                    PlayMode::DoublePlay,
                    PlayType::Double | PlayType::DoubleBattle
                )
        );
        if !compatible {
            return Err(CorpusError::InvalidReplay(
                "complete-label result play_mode and play_type are inconsistent".to_owned(),
            ));
        }
    }
    if let (LabelState::Known { value: note_count }, LabelState::Known { value: score }) =
        (notes, current_score)
        && u64::from(*score) > 2 * u64::from(*note_count)
    {
        return Err(CorpusError::InvalidReplay(
            "complete-label result current_score exceeds twice the note count".to_owned(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_required_label<T>(
    state: &LabelState<T>,
    field: &str,
    validate_known: impl FnOnce(&T) -> bool,
) -> Result<(), CorpusError> {
    state.validate(field, validate_known)?;
    state.require_applicable(field)
}

pub(crate) fn require_screen_class(
    frame: &ReplayFrame,
    expected: ScreenClass,
) -> Result<(), CorpusError> {
    if frame.screen_class != expected {
        return Err(CorpusError::InvalidReplay(
            "complete-label shape does not match screen_class".to_owned(),
        ));
    }
    Ok(())
}
