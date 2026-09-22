//! Portable diagnostic status and fact records.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRunStatus {
    Success,
    Error,
    Cancel,
    Timeout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCompleteness {
    Complete,
    Partial,
    Dropped,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticErrorType {
    InvalidConfiguration,
    StoreUnavailable,
    SequenceNonmonotonic,
    TimingNonmonotonic,
    CaptureSequenceGap,
    CapacityExceeded,
    FrameLimitExceeded,
    FactLimitExceeded,
    EncodeFailed,
    WriteFailed,
    FinalizeFailed,
    QueueFull,
    WorkerUnavailable,
    FlushTimeout,
    FieldObserverOutstandingLimit,
    FieldObserverQueueFull,
    FieldObserverUnavailable,
    FieldObserverFinishTimeout,
    FieldObservationAbandoned,
}

impl DiagnosticErrorType {
    pub const ALL: [Self; 19] = [
        Self::InvalidConfiguration,
        Self::StoreUnavailable,
        Self::SequenceNonmonotonic,
        Self::TimingNonmonotonic,
        Self::CaptureSequenceGap,
        Self::CapacityExceeded,
        Self::FrameLimitExceeded,
        Self::FactLimitExceeded,
        Self::EncodeFailed,
        Self::WriteFailed,
        Self::FinalizeFailed,
        Self::QueueFull,
        Self::WorkerUnavailable,
        Self::FlushTimeout,
        Self::FieldObserverOutstandingLimit,
        Self::FieldObserverQueueFull,
        Self::FieldObserverUnavailable,
        Self::FieldObserverFinishTimeout,
        Self::FieldObservationAbandoned,
    ];
    pub const COUNT: usize = Self::ALL.len();

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOperation {
    CaptureFrame,
    NormalizeFrame,
    SampleRecognition,
    InspectRecognition,
    ObserveFields,
    ReduceSongContext,
    DeliverEvent,
    ChangeBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticOperationStatus {
    Success,
    Error,
    Cancel,
    Timeout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticFactErrorType {
    CaptureUnavailable,
    NormalizeFailed,
    RecognitionFailed,
    FieldObservationFailed,
    SelectionConflict,
    EventDeliveryFailed,
    ConsumerUnavailable,
    OperationTimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticScreen {
    Unknown,
    Title,
    MusicSelection,
    ModeSelection,
    DecideTransition,
    Gameplay,
    Result,
    ConfirmedNonState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticContextChange {
    Replaced,
    Preserved,
    Cleared,
    AlreadyEmpty,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticDecisionDomain {
    MusicSelection,
    Result,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticDecisionOutcome {
    Accepted,
    Unknown,
    Suppressed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticTextField {
    TitleGameVersion,
    ResultTitle,
    ResultArtist,
    ResultClearType,
    ResultDifficulty,
    ResultPlayType,
    ResultLevel,
    ResultNotes,
    ResultCurrentScore,
    ResultPreviousClearType,
    ResultPreviousScore,
    ResultPreviousMissCount,
    ResultMissCount,
    ResultPgreat,
    ResultGreat,
    ResultGood,
    ResultBad,
    ResultPoor,
    ResultFast,
    ResultSlow,
    ResultComboBreak,
    MusicSelectCentralTitle,
    MusicSelectArtist,
    MusicSelectActiveListTitle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticEventKind {
    MusicSelectDetected,
    ResultDetected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticEventOutcome {
    Emitted,
    Suppressed,
    NotApplicable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameFieldStatus {
    Completed,
    BusySkip,
    NotApplicable,
    Failed,
    LateEpisode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecognitionSamplingSummary {
    pub processed_ticks: u64,
    pub busy_skips: u64,
    pub maximum_consecutive_busy_skips: u64,
    pub field_observation_busy_skips: u64,
    pub maximum_consecutive_field_observation_busy_skips: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiagnosticDetail {
    Operation,
    SamplingSummary {
        processed_ticks: u64,
        busy_skips: u64,
        maximum_consecutive_busy_skips: u64,
        field_observation_busy_skips: u64,
        maximum_consecutive_field_observation_busy_skips: u64,
    },
    RecognitionBusySkip,
    FieldObservationBusySkip {
        screen: DiagnosticScreen,
    },
    FrameProcessingTiming {
        screen: DiagnosticScreen,
        screen_classification_us: u64,
        crop_prepare_us: Option<u64>,
        field_queue_wait_us: Option<u64>,
        text_batch_wall_us: Option<u64>,
        maximum_text_worker_queue_wait_us: Option<u64>,
        maximum_text_worker_inference_us: Option<u64>,
        text_worker_busy_us: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_worker_ids: Option<Vec<usize>>,
        numeric_ocr_us: Option<u64>,
        field_join_us: Option<u64>,
        catalog_evidence_us: Option<u64>,
        screen_resolver_us: Option<u64>,
        attempt_resolver_us: Option<u64>,
        output_us: Option<u64>,
        frame_processing_wall_us: u64,
        field_status: FrameFieldStatus,
    },
    ScreenObservation {
        screen: DiagnosticScreen,
    },
    ScreenPredicateObservation {
        screen: DiagnosticScreen,
        screen_path_layout_sha256: String,
        title_bright_bbox: Option<crate::frame::Roi>,
        title_bright_channel_min: u8,
        title_qualifies: bool,
        result_warm_pixels: u32,
        result_warm_pixels_min: u32,
        result_panel_side: crate::recognition::screen::ResultPanelSideState,
        result_panels: [crate::recognition::screen::ResultPanelPresenceEvidence; 2],
        result_horizontal_edge_pixels_min: u32,
        music_select_cyan_header_pixels: u32,
        music_select_cyan_header_pixels_min: u32,
        music_select_colored_level_pixels: u32,
        music_select_colored_level_pixels_min: u32,
        music_select_bright_label_pixels: u32,
        music_select_bright_label_pixels_min: u32,
        music_select_reference_evaluated: bool,
        music_select_music_reference_score_ppm: u32,
        music_select_mode_reference_score_ppm: u32,
        music_select_reference_score_min_ppm: u32,
        music_select_reference_winner_margin_min_ppm: u32,
        decide_transition_cyan_pixels: u32,
        decide_transition_cyan_pixels_min: u32,
        decide_transition_bright_pixels: u32,
        decide_transition_bright_pixels_min: u32,
        decide_transition_saturated_pixels: u32,
        decide_transition_saturated_pixels_min: u32,
        play_presence: crate::recognition::screen::PlayPresenceEvidence,
    },
    FieldObservation {
        screen: DiagnosticScreen,
        observed_fields: u8,
        unimplemented_fields: u8,
        failed_field: Option<DiagnosticTextField>,
    },
    SongContextObservation {
        change: DiagnosticContextChange,
        candidate_set_sha256: Option<String>,
    },
    SongDecision {
        domain: DiagnosticDecisionDomain,
        outcome: DiagnosticDecisionOutcome,
        song_id: Option<String>,
    },
    EventDelivery {
        event: DiagnosticEventKind,
        outcome: DiagnosticEventOutcome,
    },
    BindingChange {
        next_binding_sha256: String,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct DiagnosticFact {
    #[serde(rename = "tick_sequence")]
    pub sequence: u64,
    pub monotonic_start_ms: u64,
    pub monotonic_end_ms: u64,
    pub operation: DiagnosticOperation,
    pub status: DiagnosticOperationStatus,
    pub error_type: Option<DiagnosticFactErrorType>,
    pub detail: DiagnosticDetail,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_serialization_preserves_the_existing_tagged_schema() {
        let fact = DiagnosticFact {
            sequence: 7,
            monotonic_start_ms: 10,
            monotonic_end_ms: 11,
            operation: DiagnosticOperation::ObserveFields,
            status: DiagnosticOperationStatus::Error,
            error_type: Some(DiagnosticFactErrorType::FieldObservationFailed),
            detail: DiagnosticDetail::FieldObservation {
                screen: DiagnosticScreen::Result,
                observed_fields: 3,
                unimplemented_fields: 1,
                failed_field: Some(DiagnosticTextField::ResultTitle),
            },
        };
        assert_eq!(
            serde_json::to_value(fact).unwrap(),
            serde_json::json!({
                "tick_sequence": 7,
                "monotonic_start_ms": 10,
                "monotonic_end_ms": 11,
                "operation": "observe_fields",
                "status": "error",
                "error_type": "field_observation_failed",
                "detail": {
                    "kind": "field_observation",
                    "screen": "result",
                    "observed_fields": 3,
                    "unimplemented_fields": 1,
                    "failed_field": "result_title"
                }
            })
        );
    }

    #[test]
    fn degradation_reason_index_covers_every_wire_reason_once() {
        assert_eq!(DiagnosticErrorType::ALL.len(), DiagnosticErrorType::COUNT);
        for (index, reason) in DiagnosticErrorType::ALL.into_iter().enumerate() {
            assert_eq!(reason.index(), index);
        }
    }
}
