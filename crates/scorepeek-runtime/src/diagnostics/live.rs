use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use crate::diagnostics::contract::{
    DiagnosticDetail, DiagnosticErrorType, DiagnosticFact, DiagnosticFactErrorType,
    DiagnosticOperation, DiagnosticOperationStatus, DiagnosticPolicy, DiagnosticRetention,
    DiagnosticRunDescriptor, DiagnosticRunStatus, DiagnosticScreen, DiagnosticTextField,
};
use scorepeek_core::recognition::screen::{
    ScreenClass, ScreenFieldObservationError, ScreenFieldObservations, ScreenTextField,
};

use crate::diagnostics::ring::{
    DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT, DiagnosticEnqueueOutcome, DiagnosticOwnedFrame,
    DiagnosticWorkerHandle,
};
use crate::diagnostics::writer::DiagnosticFinishOutcome;
use crate::service::session::recognition::{BoundCanonicalFrame, RecognitionObservation};

const FOREGROUND_RING_INTERVAL_MS: u64 = 1_000;
const FOREGROUND_RING_FRAMES: usize = 12;
const FOREGROUND_RESULT_INTERVAL_MS: u64 = 1_000;
const FOREGROUND_BASELINE_INTERVAL_MS: u64 = 5 * 60 * 1_000;

pub struct RecognitionDiagnosticRecorder {
    session_id: String,
    canonical_layout_sha256: String,
    worker: DiagnosticWorkerHandle,
    retention: DiagnosticRetention,
    foreground_ring: VecDeque<DiagnosticOwnedFrame>,
    foreground_last_ring_ms: Option<u64>,
    foreground_last_recorded_ms: Option<u64>,
    foreground_last_screen: Option<ScreenClass>,
}

impl RecognitionDiagnosticRecorder {
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Starts one application-owned diagnostic run for one immutable source generation.
    #[must_use]
    pub fn start(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Self {
        let retention = policy.retention;
        let worker = DiagnosticWorkerHandle::start(root, descriptor.clone(), policy);
        Self::with_worker(descriptor, worker, retention)
    }

    #[must_use]
    pub(crate) fn start_named(
        root: &Path,
        directory_name: &str,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Self {
        let retention = policy.retention;
        let worker =
            DiagnosticWorkerHandle::start_named(root, directory_name, descriptor.clone(), policy);
        Self::with_worker(descriptor, worker, retention)
    }

    /// Offers canonical evidence before recognition outcomes are known.
    ///
    /// The offer never waits for queue capacity or diagnostic I/O.
    /// # Panics
    ///
    /// Panics if the frame belongs to another run binding.
    pub fn offer(&mut self, frame: &BoundCanonicalFrame) -> DiagnosticEnqueueOutcome {
        assert!(
            self.matches_frame(frame),
            "diagnostic frame binding must match its run"
        );
        if self.retention == DiagnosticRetention::FactsOnly {
            return DiagnosticEnqueueOutcome::Disabled;
        }
        self.worker.try_record_frame(DiagnosticOwnedFrame {
            sequence: frame.sequence,
            monotonic_start_ms: frame.monotonic_start_ms,
            monotonic_end_ms: frame.monotonic_end_ms,
            pixels: Arc::clone(&frame.pixels),
            source: None,
        })
    }

    /// # Panics
    ///
    /// Panics if the observation belongs to another run binding.
    pub fn record_frame_for_observation(
        &mut self,
        observation: &RecognitionObservation<'_>,
    ) -> DiagnosticEnqueueOutcome {
        if self.retention == DiagnosticRetention::FactsOnly {
            return DiagnosticEnqueueOutcome::Disabled;
        }
        if self.retention == DiagnosticRetention::CompleteCadence {
            return self.offer(observation.frame());
        }
        let frame = observation.frame();
        assert!(
            self.matches_frame(frame),
            "diagnostic frame binding must match its run"
        );
        let screen = observation.screen();
        let predicate = observation.predicate();
        let partial_result = screen == ScreenClass::Unknown
            && predicate.result_presence.warm_pixels >= predicate.result_presence.warm_pixels_min;
        let owned = owned_frame(frame);
        let observed = self.worker.observe_frame(&owned);
        if matches!(
            observed,
            DiagnosticEnqueueOutcome::Rejected
                | DiagnosticEnqueueOutcome::Disabled
                | DiagnosticEnqueueOutcome::WorkerUnavailable
        ) {
            return observed;
        }
        let ring_due = self.foreground_last_ring_ms.is_none_or(|previous| {
            frame.monotonic_start_ms.saturating_sub(previous) >= FOREGROUND_RING_INTERVAL_MS
        });
        if screen == ScreenClass::Unknown && ring_due {
            if self.foreground_ring.len() == FOREGROUND_RING_FRAMES {
                self.foreground_ring.pop_front();
            }
            self.foreground_ring.push_back(owned);
            self.foreground_last_ring_ms = Some(frame.monotonic_start_ms);
        }

        let transitioned_from_unknown = self.foreground_last_screen == Some(ScreenClass::Unknown)
            && screen != ScreenClass::Unknown;
        let interval = if screen == ScreenClass::Result {
            FOREGROUND_RESULT_INTERVAL_MS
        } else {
            FOREGROUND_BASELINE_INTERVAL_MS
        };
        let known_due = screen != ScreenClass::Unknown
            && self.foreground_last_recorded_ms.is_none_or(|previous| {
                frame.monotonic_start_ms.saturating_sub(previous) >= interval
            });
        let mut retained = Vec::new();
        if partial_result || transitioned_from_unknown {
            retained.extend(self.foreground_ring.drain(..));
        }
        if screen != ScreenClass::Unknown
            && (transitioned_from_unknown || known_due)
            && retained
                .last()
                .is_none_or(|saved| saved.sequence != frame.sequence)
        {
            retained.push(owned_frame(frame));
        }
        self.foreground_last_screen = Some(screen);
        if retained.is_empty() {
            return DiagnosticEnqueueOutcome::SkippedCadence;
        }
        self.foreground_last_recorded_ms = retained.last().map(|saved| saved.monotonic_start_ms);
        self.worker.try_record_observed_frames(retained)
    }

    /// Records one screen-predicate result against the same immutable run and live-frame binding.
    ///
    /// Queueing is non-blocking and diagnostic failure does not change the recognition observation.
    #[allow(
        clippy::too_many_lines,
        reason = "one screen-predicate diagnostic preserves every bounded raw measurement and threshold"
    )]
    /// # Panics
    ///
    /// Panics if the observation belongs to another run or layout.
    pub fn record_screen_observation(
        &mut self,
        observation: &RecognitionObservation<'_>,
    ) -> DiagnosticEnqueueOutcome {
        let frame = observation.frame();
        assert!(self.matches_frame(frame));
        assert_eq!(
            observation.canonical_layout_sha256(),
            self.canonical_layout_sha256
        );
        let screen = match observation.screen() {
            scorepeek_core::recognition::screen::ScreenClass::Title => DiagnosticScreen::Title,
            scorepeek_core::recognition::screen::ScreenClass::Result => DiagnosticScreen::Result,
            scorepeek_core::recognition::screen::ScreenClass::MusicSelect => {
                DiagnosticScreen::MusicSelection
            }
            scorepeek_core::recognition::screen::ScreenClass::ModeSelect => {
                DiagnosticScreen::ModeSelection
            }
            scorepeek_core::recognition::screen::ScreenClass::DecideTransition => {
                DiagnosticScreen::DecideTransition
            }
            scorepeek_core::recognition::screen::ScreenClass::Play => DiagnosticScreen::Gameplay,
            scorepeek_core::recognition::screen::ScreenClass::Unknown => DiagnosticScreen::Unknown,
        };
        let predicate = observation.predicate();
        self.worker.try_record_fact(DiagnosticFact {
            sequence: frame.sequence,
            monotonic_start_ms: frame.monotonic_start_ms,
            monotonic_end_ms: frame.monotonic_end_ms,
            operation: DiagnosticOperation::InspectRecognition,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::ScreenPredicateObservation {
                screen,
                screen_path_layout_sha256: predicate.screen_path_layout_sha256.clone(),
                title_bright_bbox: predicate.title_presence.bright_bbox,
                title_bright_channel_min: predicate.title_presence.bright_channel_min,
                title_qualifies: predicate.title_presence.qualifies,
                result_warm_pixels: predicate.result_presence.warm_pixels,
                result_warm_pixels_min: predicate.result_presence.warm_pixels_min,
                result_panel_side: predicate.result_presence.panel_side,
                result_panels: predicate.result_presence.panels,
                result_horizontal_edge_pixels_min: predicate
                    .result_presence
                    .horizontal_edge_pixels_min,
                music_select_cyan_header_pixels: predicate.music_select_presence.cyan_header_pixels,
                music_select_cyan_header_pixels_min: predicate
                    .music_select_presence
                    .cyan_header_pixels_min,
                music_select_colored_level_pixels: predicate
                    .music_select_presence
                    .colored_level_pixels,
                music_select_colored_level_pixels_min: predicate
                    .music_select_presence
                    .colored_level_pixels_min,
                music_select_bright_label_pixels: predicate
                    .music_select_presence
                    .bright_label_pixels,
                music_select_bright_label_pixels_min: predicate
                    .music_select_presence
                    .bright_label_pixels_min,
                music_select_reference_evaluated: predicate
                    .music_select_presence
                    .reference_evaluated,
                music_select_music_reference_score_ppm: predicate
                    .music_select_presence
                    .music_reference_score_ppm,
                music_select_mode_reference_score_ppm: predicate
                    .music_select_presence
                    .mode_select_reference_score_ppm,
                music_select_reference_score_min_ppm: predicate
                    .music_select_presence
                    .reference_score_min_ppm,
                music_select_reference_winner_margin_min_ppm: predicate
                    .music_select_presence
                    .reference_winner_margin_min_ppm,
                decide_transition_cyan_pixels: predicate.decide_transition_presence.cyan_pixels,
                decide_transition_cyan_pixels_min: predicate
                    .decide_transition_presence
                    .cyan_pixels_min,
                decide_transition_bright_pixels: predicate.decide_transition_presence.bright_pixels,
                decide_transition_bright_pixels_min: predicate
                    .decide_transition_presence
                    .bright_pixels_min,
                decide_transition_saturated_pixels: predicate
                    .decide_transition_presence
                    .saturated_pixels,
                decide_transition_saturated_pixels_min: predicate
                    .decide_transition_presence
                    .saturated_pixels_min,
                play_presence: predicate.play_presence,
            },
        })
    }

    pub fn record_frame_processing_timing(
        &mut self,
        timing: crate::service::session::recognition::FrameProcessingTiming,
        field_status: crate::diagnostics::contract::FrameFieldStatus,
        field_timing: Option<&scorepeek_core::model::session::RecognitionProcessingTiming>,
    ) -> DiagnosticEnqueueOutcome {
        let screen = match timing.screen {
            ScreenClass::Title => DiagnosticScreen::Title,
            ScreenClass::Result => DiagnosticScreen::Result,
            ScreenClass::MusicSelect => DiagnosticScreen::MusicSelection,
            ScreenClass::ModeSelect => DiagnosticScreen::ModeSelection,
            ScreenClass::DecideTransition => DiagnosticScreen::DecideTransition,
            ScreenClass::Play => DiagnosticScreen::Gameplay,
            ScreenClass::Unknown => DiagnosticScreen::Unknown,
        };
        self.worker.try_record_fact(DiagnosticFact {
            sequence: timing.source_sequence,
            monotonic_start_ms: timing.monotonic_start_ms,
            monotonic_end_ms: timing.monotonic_end_ms,
            operation: DiagnosticOperation::InspectRecognition,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::FrameProcessingTiming {
                screen,
                screen_classification_us: timing.screen_classification_us,
                crop_prepare_us: timing.crop_prepare_us,
                field_queue_wait_us: field_timing.map(|value| value.field_queue_wait_us),
                text_batch_wall_us: field_timing.map(|value| value.text_batch_wall_us),
                maximum_text_worker_queue_wait_us: field_timing
                    .map(|value| value.maximum_text_worker_queue_wait_us),
                maximum_text_worker_inference_us: field_timing
                    .map(|value| value.maximum_text_worker_inference_us),
                text_worker_busy_us: field_timing.map(|value| value.text_worker_busy_us),
                text_worker_ids: field_timing.map(|value| value.text_worker_ids.clone()),
                numeric_ocr_us: field_timing.and_then(|value| value.numeric_recognition_us),
                field_join_us: field_timing.map(|value| value.join_us),
                catalog_evidence_us: field_timing.map(|value| value.catalog_evidence_us),
                screen_resolver_us: timing.screen_resolver_us,
                attempt_resolver_us: timing.attempt_resolver_us,
                output_us: timing.output_us,
                frame_processing_wall_us: timing.frame_processing_wall_us,
                field_status,
            },
        })
    }

    /// Records a typed screen-inspection failure without replacing its application error.
    /// # Panics
    ///
    /// Panics if the frame belongs to another run binding.
    pub fn record_recognition_failure(
        &mut self,
        frame: &BoundCanonicalFrame,
    ) -> DiagnosticEnqueueOutcome {
        assert!(
            self.matches_frame(frame),
            "diagnostic frame binding must match its run"
        );
        self.worker.try_record_fact(DiagnosticFact {
            sequence: frame.sequence,
            monotonic_start_ms: frame.monotonic_start_ms,
            monotonic_end_ms: frame.monotonic_end_ms,
            operation: DiagnosticOperation::InspectRecognition,
            status: DiagnosticOperationStatus::Error,
            error_type: Some(DiagnosticFactErrorType::RecognitionFailed),
            detail: DiagnosticDetail::ScreenObservation {
                screen: DiagnosticScreen::Unknown,
            },
        })
    }

    /// Records only value-free field-observer status after the bound result is available.
    ///
    /// Diagnostic queueing remains non-blocking and cannot replace or mutate the worker output.
    pub fn record_field_observation<E>(
        &mut self,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
        output: &Result<ScreenFieldObservations, ScreenFieldObservationError<E>>,
    ) -> DiagnosticEnqueueOutcome {
        self.record_field_observation_summary(
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            screen,
            output.as_ref().map_err(|error| error.field),
        )
    }

    pub(crate) fn record_field_observation_summary(
        &mut self,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
        output: Result<&ScreenFieldObservations, ScreenTextField>,
    ) -> DiagnosticEnqueueOutcome {
        let diagnostic_screen = match screen {
            ScreenClass::Title => DiagnosticScreen::Title,
            ScreenClass::Result => DiagnosticScreen::Result,
            ScreenClass::MusicSelect => DiagnosticScreen::MusicSelection,
            ScreenClass::ModeSelect
            | ScreenClass::DecideTransition
            | ScreenClass::Play
            | ScreenClass::Unknown => {
                unreachable!("field observation must belong to a field screen");
            }
        };
        let (status, error_type, observed_fields, unimplemented_fields, failed_field) = match output
        {
            Ok(fields) if fields.screen() == screen => {
                let (observed, unimplemented) = fields.diagnostic_field_counts();
                (
                    DiagnosticOperationStatus::Success,
                    None,
                    observed,
                    unimplemented,
                    None,
                )
            }
            Ok(_) => {
                unreachable!("field output must match its screen");
            }
            Err(failed_field) => {
                let Some(field) = diagnostic_text_field(screen, failed_field) else {
                    unreachable!("failed field must belong to its screen");
                };
                (
                    DiagnosticOperationStatus::Error,
                    Some(DiagnosticFactErrorType::FieldObservationFailed),
                    0,
                    match screen {
                        ScreenClass::Title | ScreenClass::Result => 0,
                        ScreenClass::MusicSelect => 1,
                        ScreenClass::ModeSelect
                        | ScreenClass::DecideTransition
                        | ScreenClass::Play
                        | ScreenClass::Unknown => {
                            unreachable!("non-field screen was rejected above")
                        }
                    },
                    Some(field),
                )
            }
        };
        self.worker.try_record_fact(DiagnosticFact {
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            operation: DiagnosticOperation::ObserveFields,
            status,
            error_type,
            detail: DiagnosticDetail::FieldObservation {
                screen: diagnostic_screen,
                observed_fields,
                unimplemented_fields,
                failed_field,
            },
        })
    }

    pub(crate) fn record_field_observation_busy_skip(
        &mut self,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        screen: ScreenClass,
    ) -> DiagnosticEnqueueOutcome {
        let screen = match screen {
            ScreenClass::Title => DiagnosticScreen::Title,
            ScreenClass::Result => DiagnosticScreen::Result,
            ScreenClass::MusicSelect => DiagnosticScreen::MusicSelection,
            ScreenClass::ModeSelect
            | ScreenClass::DecideTransition
            | ScreenClass::Play
            | ScreenClass::Unknown => return DiagnosticEnqueueOutcome::Rejected,
        };
        self.worker.try_record_fact(DiagnosticFact {
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            operation: DiagnosticOperation::ObserveFields,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::FieldObservationBusySkip { screen },
        })
    }

    pub(crate) fn record_field_observer_degradation(
        &mut self,
        error_type: DiagnosticErrorType,
        sequence: u64,
    ) {
        self.worker.record_external_error(error_type, sequence);
    }

    pub(crate) fn record_unbound_field_observer_degradation(
        &mut self,
        error_type: DiagnosticErrorType,
        count: u64,
    ) {
        self.worker.record_external_unbound_error(error_type, count);
    }

    pub(crate) fn record_sampling_summary(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        summary: crate::diagnostics::contract::RecognitionSamplingSummary,
    ) -> DiagnosticEnqueueOutcome {
        self.worker.try_record_fact(DiagnosticFact {
            sequence,
            monotonic_start_ms: monotonic_ms,
            monotonic_end_ms: monotonic_ms,
            operation: DiagnosticOperation::SampleRecognition,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::SamplingSummary {
                processed_ticks: summary.processed_ticks,
                busy_skips: summary.busy_skips,
                maximum_consecutive_busy_skips: summary.maximum_consecutive_busy_skips,
                field_observation_busy_skips: summary.field_observation_busy_skips,
                maximum_consecutive_field_observation_busy_skips: summary
                    .maximum_consecutive_field_observation_busy_skips,
            },
        })
    }

    pub(crate) fn record_recognition_busy_skip(
        &mut self,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
    ) -> DiagnosticEnqueueOutcome {
        self.worker
            .record_recognition_busy_skip(sequence, monotonic_start_ms, monotonic_end_ms)
    }

    /// Records the explicit end of this immutable binding before the application starts another.
    pub fn record_binding_change(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        next_binding_sha256: String,
    ) -> DiagnosticEnqueueOutcome {
        self.worker.try_record_fact(DiagnosticFact {
            sequence,
            monotonic_start_ms: monotonic_ms,
            monotonic_end_ms: monotonic_ms,
            operation: DiagnosticOperation::ChangeBinding,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::BindingChange {
                next_binding_sha256,
            },
        })
    }

    /// Finishes the run with the fixed bounded application flush timeout.
    #[must_use]
    pub fn finish(
        mut self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
    ) -> DiagnosticFinishOutcome {
        if self.retention == DiagnosticRetention::ForegroundFailureWindowV1
            && !self.foreground_ring.is_empty()
        {
            let retained = self.foreground_ring.drain(..).collect();
            let _ = self.worker.try_record_observed_frames(retained);
        }
        self.worker
            .finish(status, monotonic_end_ms, DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT)
    }

    fn with_worker(
        descriptor: DiagnosticRunDescriptor,
        worker: DiagnosticWorkerHandle,
        retention: DiagnosticRetention,
    ) -> Self {
        Self {
            session_id: descriptor.run_id,
            canonical_layout_sha256: descriptor.binding.canonical_layout_sha256,
            worker,
            retention,
            foreground_ring: VecDeque::new(),
            foreground_last_ring_ms: None,
            foreground_last_recorded_ms: None,
            foreground_last_screen: None,
        }
    }

    pub(crate) fn matches_frame(&self, frame: &BoundCanonicalFrame) -> bool {
        frame.session_id() == self.session_id
    }

    #[cfg(test)]
    pub(crate) fn start_for_test(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        capacity: usize,
    ) -> Self {
        let retention = policy.retention;
        let worker =
            DiagnosticWorkerHandle::start_for_test(root, descriptor.clone(), policy, capacity);
        Self::with_worker(descriptor, worker, retention)
    }

    #[cfg(test)]
    pub(crate) fn start_with_supervisor_for_test(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        supervisor: &std::sync::Mutex<std::sync::Weak<()>>,
    ) -> Self {
        let retention = policy.retention;
        let worker = DiagnosticWorkerHandle::start_with_supervisor_for_test(
            root,
            descriptor.clone(),
            policy,
            supervisor,
        );
        Self::with_worker(descriptor, worker, retention)
    }
}

fn owned_frame(frame: &BoundCanonicalFrame) -> DiagnosticOwnedFrame {
    DiagnosticOwnedFrame {
        sequence: frame.sequence,
        monotonic_start_ms: frame.monotonic_start_ms,
        monotonic_end_ms: frame.monotonic_end_ms,
        pixels: Arc::clone(&frame.pixels),
        source: None,
    }
}

fn diagnostic_text_field(
    screen: ScreenClass,
    field: ScreenTextField,
) -> Option<DiagnosticTextField> {
    Some(match (screen, field) {
        (ScreenClass::Title, ScreenTextField::TitleGameVersion) => {
            DiagnosticTextField::TitleGameVersion
        }
        (ScreenClass::Result, ScreenTextField::ResultTitle) => DiagnosticTextField::ResultTitle,
        (ScreenClass::Result, ScreenTextField::ResultArtist) => DiagnosticTextField::ResultArtist,
        (ScreenClass::Result, ScreenTextField::ResultClearType) => {
            DiagnosticTextField::ResultClearType
        }
        (ScreenClass::Result, ScreenTextField::ResultDifficulty) => {
            DiagnosticTextField::ResultDifficulty
        }
        (ScreenClass::Result, ScreenTextField::ResultPlayType) => {
            DiagnosticTextField::ResultPlayType
        }
        (ScreenClass::Result, ScreenTextField::ResultLevel) => DiagnosticTextField::ResultLevel,
        (ScreenClass::Result, ScreenTextField::ResultNotes) => DiagnosticTextField::ResultNotes,
        (ScreenClass::Result, ScreenTextField::ResultCurrentScore) => {
            DiagnosticTextField::ResultCurrentScore
        }
        (ScreenClass::Result, ScreenTextField::ResultPreviousClearType) => {
            DiagnosticTextField::ResultPreviousClearType
        }
        (ScreenClass::Result, ScreenTextField::ResultPreviousScore) => {
            DiagnosticTextField::ResultPreviousScore
        }
        (ScreenClass::Result, ScreenTextField::ResultPreviousMissCount) => {
            DiagnosticTextField::ResultPreviousMissCount
        }
        (ScreenClass::Result, ScreenTextField::ResultMissCount) => {
            DiagnosticTextField::ResultMissCount
        }
        (ScreenClass::Result, ScreenTextField::ResultPgreat) => DiagnosticTextField::ResultPgreat,
        (ScreenClass::Result, ScreenTextField::ResultGreat) => DiagnosticTextField::ResultGreat,
        (ScreenClass::Result, ScreenTextField::ResultGood) => DiagnosticTextField::ResultGood,
        (ScreenClass::Result, ScreenTextField::ResultBad) => DiagnosticTextField::ResultBad,
        (ScreenClass::Result, ScreenTextField::ResultPoor) => DiagnosticTextField::ResultPoor,
        (ScreenClass::Result, ScreenTextField::ResultFast) => DiagnosticTextField::ResultFast,
        (ScreenClass::Result, ScreenTextField::ResultSlow) => DiagnosticTextField::ResultSlow,
        (ScreenClass::Result, ScreenTextField::ResultComboBreak) => {
            DiagnosticTextField::ResultComboBreak
        }
        (ScreenClass::MusicSelect, ScreenTextField::MusicSelectCentralTitle) => {
            DiagnosticTextField::MusicSelectCentralTitle
        }
        (ScreenClass::MusicSelect, ScreenTextField::MusicSelectArtist) => {
            DiagnosticTextField::MusicSelectArtist
        }
        (ScreenClass::MusicSelect, ScreenTextField::MusicSelectActiveListTitle) => {
            DiagnosticTextField::MusicSelectActiveListTitle
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::contract::DiagnosticCompleteness;
    use crate::diagnostics::contract::{DiagnosticBinding, DiagnosticResource};
    use crate::service::session::recognition::{BoundCanonicalFrame, RecognitionObservation};
    use scorepeek_core::frame::CanonicalLayout;
    use scorepeek_core::recognition::screen::{
        ResultScreenFieldObservations, ScreenClass, ScreenFieldObservationError,
        ScreenFieldObservations, ScreenTextField,
    };
    use scorepeek_core::recognition::title::DynamicTextObservation;
    use std::fs;

    fn descriptor(run_id: &str, _generation: u64) -> DiagnosticRunDescriptor {
        DiagnosticRunDescriptor {
            run_id: run_id.to_owned(),
            monotonic_start_ms: 0,
            resource: DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "1".repeat(64),
            },
            binding: DiagnosticBinding {
                canonical_layout_sha256: CanonicalLayout::sha256(),
                catalog_sha256: "5".repeat(64),
                model_sha256: "6".repeat(64),
                runtime_sha256: "7".repeat(64),
                replay: None,
            },
        }
    }

    fn frame(generation: u64, sequence: u64, time: u64) -> BoundCanonicalFrame {
        BoundCanonicalFrame {
            session_id: Arc::from(format!("session-{generation}")),
            source_sequence: sequence,
            sequence,
            monotonic_start_ms: time,
            monotonic_end_ms: time + 16,
            pixels: Arc::new(
                vec![7; crate::diagnostics::writer::CANONICAL_BYTES].into_boxed_slice(),
            ),
        }
    }

    fn result_fields(text: &str) -> ScreenFieldObservations {
        let text = || DynamicTextObservation {
            input_width: 320,
            output_timesteps: 20,
            open_text: text.to_owned(),
            constrained_text: None,
        };
        ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text(),
            artist: text(),
            clear_type: text(),
            difficulty: text(),
            level: text(),
            notes: text(),
            current_score: text(),
            ..Default::default()
        })
    }

    #[test]
    fn offer_is_recognition_independent_and_reuses_owned_pixels() {
        let root = tempfile::tempdir().unwrap();
        let canonical = frame(1, 1, 0).for_test_session("live-run");
        let pixels = Arc::clone(&canonical.pixels);
        let mut bridge = RecognitionDiagnosticRecorder::start_for_test(
            root.path(),
            descriptor("live-run", 1),
            DiagnosticPolicy::default(),
            2,
        );
        assert_eq!(bridge.offer(&canonical), DiagnosticEnqueueOutcome::Enqueued);
        assert!(Arc::ptr_eq(&pixels, &canonical.pixels));
        let recognition_result = Result::<_, &'static str>::Ok("unchanged");
        let outcome = bridge.finish(DiagnosticRunStatus::Success, 16);
        assert_eq!(recognition_result, Ok("unchanged"));
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Complete));
    }

    #[test]
    fn screen_observation_retains_live_binding_and_shared_pixels() {
        let root = tempfile::tempdir().unwrap();
        let canonical = frame(1, 1, 17).for_test_session("screen-observation");
        let pixels = Arc::clone(&canonical.pixels);
        let observation = RecognitionObservation::inspect(&canonical).unwrap();
        assert_eq!(observation.screen(), ScreenClass::Unknown);
        let mut bridge = RecognitionDiagnosticRecorder::start_for_test(
            root.path(),
            descriptor("screen-observation", 1),
            DiagnosticPolicy::default(),
            2,
        );
        assert_eq!(bridge.offer(&canonical), DiagnosticEnqueueOutcome::Enqueued);
        assert_eq!(
            bridge.record_screen_observation(&observation),
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert!(Arc::ptr_eq(&pixels, &canonical.pixels));
        assert_eq!(
            bridge.finish(DiagnosticRunStatus::Success, 33).completeness,
            Some(DiagnosticCompleteness::Complete)
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("screen-observation/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["frames"].as_array().unwrap().len(), 1);
        assert_eq!(manifest["facts"]["record_count"], 1);
        let fact: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("screen-observation/facts.ndjson")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            fact["fact"]["detail"]["kind"],
            "screen_predicate_observation"
        );
        assert_eq!(fact["fact"]["detail"]["screen"], "unknown");
        assert_eq!(fact["fact"]["detail"]["result_warm_pixels_min"], 3_000);
        assert_eq!(
            fact["fact"]["detail"]["result_horizontal_edge_pixels_min"],
            490
        );
        assert_eq!(
            fact["fact"]["detail"]["music_select_cyan_header_pixels_min"],
            7_000
        );
        assert_eq!(
            fact["fact"]["detail"]["music_select_bright_label_pixels"],
            0
        );
        assert_eq!(
            fact["fact"]["detail"]["music_select_bright_label_pixels_min"],
            4_000
        );
    }

    #[test]
    fn frame_timing_retains_each_immediate_field_status_once() {
        let root = tempfile::tempdir().unwrap();
        let mut bridge = RecognitionDiagnosticRecorder::start_for_test(
            root.path(),
            descriptor("frame-field-status", 1),
            DiagnosticPolicy::default(),
            8,
        );
        for (sequence, field_status) in [
            (1, crate::diagnostics::contract::FrameFieldStatus::BusySkip),
            (
                2,
                crate::diagnostics::contract::FrameFieldStatus::NotApplicable,
            ),
            (3, crate::diagnostics::contract::FrameFieldStatus::Failed),
        ] {
            assert_eq!(
                bridge.offer(
                    &frame(1, sequence, sequence * 100).for_test_session("frame-field-status")
                ),
                DiagnosticEnqueueOutcome::Enqueued
            );
            assert_eq!(
                bridge.record_frame_processing_timing(
                    crate::service::session::recognition::FrameProcessingTiming {
                        frame_started: std::time::Instant::now(),
                        source_sequence: sequence,
                        monotonic_start_ms: sequence * 100,
                        monotonic_end_ms: sequence * 100 + 16,
                        screen: ScreenClass::Unknown,
                        screen_classification_us: 5,
                        crop_prepare_us: None,
                        screen_resolver_us: Some(2),
                        attempt_resolver_us: None,
                        output_us: Some(1),
                        frame_processing_wall_us: 7,
                    },
                    field_status,
                    None,
                ),
                DiagnosticEnqueueOutcome::Enqueued
            );
        }
        assert_eq!(
            bridge
                .finish(DiagnosticRunStatus::Success, 400)
                .completeness,
            Some(DiagnosticCompleteness::Complete)
        );
        let facts = fs::read_to_string(root.path().join("frame-field-status/facts.ndjson"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        let manifest =
            fs::read_to_string(root.path().join("frame-field-status/manifest.json")).unwrap();
        assert_eq!(facts.len(), 3, "{manifest}");
        assert_eq!(facts[0]["fact"]["detail"]["field_status"], "busy_skip");
        assert_eq!(facts[0]["fact"]["detail"]["screen_resolver_us"], 2);
        assert_eq!(
            facts[0]["fact"]["detail"]["attempt_resolver_us"],
            serde_json::Value::Null
        );
        assert_eq!(facts[0]["fact"]["detail"]["output_us"], 1);
        assert_eq!(facts[0]["fact"]["detail"]["frame_processing_wall_us"], 7);
        assert_eq!(facts[1]["fact"]["detail"]["field_status"], "not_applicable");
        assert_eq!(facts[2]["fact"]["detail"]["field_status"], "failed");
    }

    #[test]
    fn foreground_retention_records_every_tick_and_keeps_only_the_frame_tail() {
        let root = tempfile::tempdir().unwrap();
        let policy = DiagnosticPolicy {
            retention: DiagnosticRetention::ForegroundFailureWindowV1,
            ..DiagnosticPolicy::default()
        };
        let mut bridge = RecognitionDiagnosticRecorder::start_for_test(
            root.path(),
            descriptor("foreground-tail", 1),
            policy,
            64,
        );

        for sequence in 1..=20 {
            let canonical =
                frame(1, sequence, (sequence - 1) * 1_000).for_test_session("foreground-tail");
            let observation = RecognitionObservation::inspect(&canonical).unwrap();
            assert_eq!(observation.screen(), ScreenClass::Unknown);
            assert_eq!(
                bridge.record_frame_for_observation(&observation),
                DiagnosticEnqueueOutcome::SkippedCadence
            );
            assert_eq!(
                bridge.record_screen_observation(&observation),
                DiagnosticEnqueueOutcome::Enqueued
            );
        }
        let outcome = bridge.finish(DiagnosticRunStatus::Success, 20_000);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Complete));

        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("foreground-tail/manifest.json")).unwrap(),
        )
        .unwrap();
        let frames = manifest["frames"].as_array().unwrap();
        assert_eq!(frames.len(), 12);
        assert_eq!(frames.first().unwrap()["sequence"], 9);
        assert_eq!(frames.last().unwrap()["sequence"], 20);
        assert_eq!(manifest["facts"]["record_count"], 20);
    }

    #[test]
    fn facts_only_retention_never_materializes_qoi_frames() {
        let root = tempfile::tempdir().unwrap();
        let policy = DiagnosticPolicy {
            retention: DiagnosticRetention::FactsOnly,
            ..DiagnosticPolicy::default()
        };
        let mut bridge = RecognitionDiagnosticRecorder::start_for_test(
            root.path(),
            descriptor("facts-only", 1),
            policy,
            8,
        );
        let canonical = frame(1, 1, 0).for_test_session("facts-only");
        let observation = RecognitionObservation::inspect(&canonical).unwrap();
        assert_eq!(
            bridge.record_frame_for_observation(&observation),
            DiagnosticEnqueueOutcome::Disabled
        );
        assert_eq!(bridge.offer(&canonical), DiagnosticEnqueueOutcome::Disabled);
        assert_eq!(
            bridge.record_screen_observation(&observation),
            DiagnosticEnqueueOutcome::Enqueued
        );
        let outcome = bridge.finish(DiagnosticRunStatus::Success, 100);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Complete));
        let directory = root.path().join("facts-only");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["frames"].as_array().unwrap().len(), 0);
        assert_eq!(manifest["facts"]["record_count"], 1);
        assert!(fs::read_dir(directory).unwrap().flatten().all(|entry| {
            entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "qoi")
        }));
    }

    #[test]
    fn generation_rollover_creates_two_independent_runs() {
        let root = tempfile::tempdir().unwrap();
        let supervisor = std::sync::Mutex::new(std::sync::Weak::new());
        for generation in [1, 2] {
            let run_id = format!("generation-{generation}");
            let mut bridge = RecognitionDiagnosticRecorder::start_with_supervisor_for_test(
                root.path(),
                descriptor(&run_id, generation),
                DiagnosticPolicy::default(),
                &supervisor,
            );
            assert_eq!(
                bridge.offer(&frame(generation, 1, 0).for_test_session(&run_id)),
                DiagnosticEnqueueOutcome::Enqueued
            );
            assert_eq!(
                bridge.finish(DiagnosticRunStatus::Success, 16).completeness,
                Some(DiagnosticCompleteness::Complete),
                "generation {generation} must release the diagnostic worker"
            );
        }
        assert!(root.path().join("generation-1/manifest.json").is_file());
        assert!(root.path().join("generation-2/manifest.json").is_file());
    }

    #[test]
    fn opt_out_preserves_live_result_and_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let canonical = frame(1, 1, 0).for_test_session("disabled-live");
        let observation = RecognitionObservation::inspect(&canonical).unwrap();
        let mut bridge = RecognitionDiagnosticRecorder::start_for_test(
            root.path(),
            descriptor("disabled-live", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
            2,
        );
        assert_eq!(bridge.offer(&canonical), DiagnosticEnqueueOutcome::Disabled);
        assert_eq!(
            bridge.record_screen_observation(&observation),
            DiagnosticEnqueueOutcome::Disabled
        );
        assert_eq!(observation.screen(), ScreenClass::Unknown);
        assert_eq!(
            bridge.finish(DiagnosticRunStatus::Success, 16).completeness,
            None
        );
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn field_observation_diagnostics_are_value_free_and_non_interfering() {
        let root = tempfile::tempdir().unwrap();
        let output = Ok::<_, ScreenFieldObservationError<&'static str>>(result_fields(
            "OCR CONTENT SENTINEL",
        ));
        let mut bridge = RecognitionDiagnosticRecorder::start_for_test(
            root.path(),
            descriptor("field-observation", 1),
            DiagnosticPolicy::default(),
            2,
        );
        assert_eq!(
            bridge.record_field_observation(7, 20, 36, ScreenClass::Result, &output),
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert_eq!(
            output.as_ref().unwrap().screen(),
            ScreenClass::Result,
            "diagnostic enqueue must not change the observer output"
        );
        assert_eq!(
            bridge.finish(DiagnosticRunStatus::Success, 40).completeness,
            Some(DiagnosticCompleteness::Complete)
        );
        let run = root.path().join("field-observation");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(run.join("manifest.json")).unwrap()).unwrap();
        let filename = manifest["facts"]["filename"].as_str().unwrap();
        let fact_bytes = fs::read(run.join(filename)).unwrap();
        let fact: serde_json::Value = serde_json::from_slice(&fact_bytes).unwrap();
        assert_eq!(fact["fact"]["operation"], "observe_fields");
        assert_eq!(fact["fact"]["detail"]["kind"], "field_observation");
        assert_eq!(fact["fact"]["detail"]["observed_fields"], 20);
        assert_eq!(fact["fact"]["detail"]["unimplemented_fields"], 0);
        assert!(
            !String::from_utf8(fact_bytes)
                .unwrap()
                .contains("OCR CONTENT SENTINEL")
        );

        let disabled_root = tempfile::tempdir().unwrap();
        let disabled_output = Ok::<_, ScreenFieldObservationError<&'static str>>(result_fields(
            "OCR CONTENT SENTINEL",
        ));
        let mut disabled = RecognitionDiagnosticRecorder::start_for_test(
            disabled_root.path(),
            descriptor("field-observation-disabled", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
            2,
        );
        assert_eq!(
            disabled.record_field_observation(7, 20, 36, ScreenClass::Result, &disabled_output,),
            DiagnosticEnqueueOutcome::Disabled
        );
        assert!(matches!(
            disabled_output,
            Ok(fields) if fields.screen() == ScreenClass::Result
        ));
        assert_eq!(
            disabled
                .finish(DiagnosticRunStatus::Success, 40)
                .completeness,
            None
        );
        assert_eq!(disabled_root.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn field_observation_failure_records_only_typed_field_and_error() {
        let root = tempfile::tempdir().unwrap();
        let output = Err(ScreenFieldObservationError::new(
            ScreenTextField::ResultArtist,
            "RUNTIME CAUSE SENTINEL",
        ));
        let mut bridge = RecognitionDiagnosticRecorder::start_for_test(
            root.path(),
            descriptor("field-observation-error", 1),
            DiagnosticPolicy::default(),
            1,
        );
        assert_eq!(
            bridge.record_field_observation(8, 40, 56, ScreenClass::Result, &output),
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert_eq!(
            output.as_ref().unwrap_err().source_error(),
            &"RUNTIME CAUSE SENTINEL"
        );
        assert_eq!(
            bridge.finish(DiagnosticRunStatus::Success, 60).completeness,
            Some(DiagnosticCompleteness::Complete)
        );
        let run = root.path().join("field-observation-error");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(run.join("manifest.json")).unwrap()).unwrap();
        let filename = manifest["facts"]["filename"].as_str().unwrap();
        let fact_bytes = fs::read(run.join(filename)).unwrap();
        let fact: serde_json::Value = serde_json::from_slice(&fact_bytes).unwrap();
        assert_eq!(fact["fact"]["status"], "error");
        assert_eq!(fact["fact"]["error_type"], "field_observation_failed");
        assert_eq!(fact["fact"]["detail"]["failed_field"], "result_artist");
        assert!(
            !String::from_utf8(fact_bytes)
                .unwrap()
                .contains("RUNTIME CAUSE SENTINEL")
        );
    }

    #[test]
    fn worker_loss_is_diagnostic_only() {
        let root = tempfile::tempdir().unwrap();
        let mut bridge = RecognitionDiagnosticRecorder::start_for_test(
            root.path(),
            descriptor("worker-loss", 1),
            DiagnosticPolicy::default(),
            0,
        );
        assert_eq!(
            bridge.offer(&frame(1, 1, 0).for_test_session("worker-loss")),
            DiagnosticEnqueueOutcome::WorkerUnavailable
        );
        let recognition_result = Result::<_, &'static str>::Ok("unchanged");
        let outcome = bridge.finish(DiagnosticRunStatus::Success, 16);
        assert_eq!(recognition_result, Ok("unchanged"));
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Dropped));
        assert_eq!(
            outcome.error_type,
            Some(DiagnosticErrorType::WorkerUnavailable)
        );
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }
}
