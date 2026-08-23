use std::fmt;
use std::path::Path;
use std::sync::Arc;

use scorepeek::capture::NormalizedCanonicalFrame;
use scorepeek::recognition::{
    CanonicalFrame, ScreenClass, ScreenFieldObservationError, ScreenFieldObservations,
    ScreenTextField,
};

use crate::diagnostic_recording::{
    DiagnosticDetail, DiagnosticErrorType, DiagnosticFact, DiagnosticFactErrorType,
    DiagnosticFinishOutcome, DiagnosticOperation, DiagnosticOperationStatus, DiagnosticPolicy,
    DiagnosticRunDescriptor, DiagnosticRunStatus, DiagnosticScreen, DiagnosticTextField,
};
use crate::diagnostic_worker::{
    DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT, DiagnosticEnqueueOutcome, DiagnosticOwnedFrame,
    DiagnosticWorkerHandle,
};
use crate::recognition_live::RecognitionObservation;

#[derive(Clone)]
pub struct BoundCanonicalFrame {
    capture_generation: u64,
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    capture_profile_sha256: String,
    normalizer_sha256: String,
    pixels: Arc<Box<[u8]>>,
}

impl fmt::Debug for BoundCanonicalFrame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BoundCanonicalFrame")
            .field("capture_generation", &self.capture_generation)
            .field("sequence", &self.sequence)
            .field("monotonic_start_ms", &self.monotonic_start_ms)
            .field("monotonic_end_ms", &self.monotonic_end_ms)
            .field("capture_profile_sha256", &self.capture_profile_sha256)
            .field("normalizer_sha256", &self.normalizer_sha256)
            .finish_non_exhaustive()
    }
}

impl BoundCanonicalFrame {
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub(crate) const fn capture_generation(&self) -> u64 {
        self.capture_generation
    }

    #[must_use]
    pub(crate) fn capture_profile_sha256(&self) -> &str {
        &self.capture_profile_sha256
    }

    #[must_use]
    pub(crate) fn normalizer_sha256(&self) -> &str {
        &self.normalizer_sha256
    }

    #[must_use]
    pub(crate) const fn monotonic_start_ms(&self) -> u64 {
        self.monotonic_start_ms
    }

    #[must_use]
    pub(crate) const fn monotonic_end_ms(&self) -> u64 {
        self.monotonic_end_ms
    }

    pub(crate) fn from_extraction(
        frame: CanonicalFrame,
        capture_generation: u64,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
    ) -> Self {
        Self {
            capture_generation,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            capture_profile_sha256: frame.capture_profile_id().to_owned(),
            normalizer_sha256: frame.normalizer_artifact_sha256().to_owned(),
            pixels: Arc::new(frame.into_pixels()),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test(generation: u64, sequence: u64, time: u64) -> Self {
        Self::for_test_pixels(
            generation,
            sequence,
            time,
            vec![7; crate::diagnostic_recording::CANONICAL_BYTES].into_boxed_slice(),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_test_pixels(
        generation: u64,
        sequence: u64,
        time: u64,
        pixels: Box<[u8]>,
    ) -> Self {
        Self {
            capture_generation: generation,
            sequence,
            monotonic_start_ms: time,
            monotonic_end_ms: time + 16,
            capture_profile_sha256: "2".repeat(64),
            normalizer_sha256: "3".repeat(64),
            pixels: Arc::new(pixels),
        }
    }
}

impl From<NormalizedCanonicalFrame> for BoundCanonicalFrame {
    fn from(frame: NormalizedCanonicalFrame) -> Self {
        let received_monotonic_ms = frame.received_monotonic_ns() / 1_000_000;
        let pixel_address = frame.pixels().as_ptr();
        let live = Self {
            capture_generation: frame.capture_generation().get(),
            sequence: frame.source_sequence(),
            monotonic_start_ms: received_monotonic_ms,
            monotonic_end_ms: received_monotonic_ms,
            capture_profile_sha256: frame.capture_profile_sha256().to_owned(),
            normalizer_sha256: frame.normalizer_artifact_sha256().to_owned(),
            pixels: Arc::new(frame.into_pixels()),
        };
        debug_assert_eq!(
            live.pixels.len(),
            crate::diagnostic_recording::CANONICAL_BYTES
        );
        debug_assert_eq!(live.pixels.as_ptr(), pixel_address);
        live
    }
}

pub struct DiagnosticBridge {
    capture_generation: u64,
    capture_profile_sha256: String,
    normalizer_sha256: String,
    canonical_layout_sha256: String,
    worker: DiagnosticWorkerHandle,
}

impl DiagnosticBridge {
    /// Starts one application-owned diagnostic run for one immutable source generation.
    #[must_use]
    pub fn start(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Self {
        let worker = DiagnosticWorkerHandle::start(root, descriptor.clone(), policy);
        Self::with_worker(descriptor, worker)
    }

    /// Offers canonical evidence before recognition outcomes are known.
    ///
    /// The offer never waits for queue capacity or diagnostic I/O. A binding mismatch is recorded
    /// only as diagnostic degradation and cannot alter recognition or event results.
    pub fn offer(&mut self, frame: &BoundCanonicalFrame) -> DiagnosticEnqueueOutcome {
        if !self.matches_frame(frame) {
            self.worker
                .record_external_error(DiagnosticErrorType::InvalidConfiguration, frame.sequence);
            return DiagnosticEnqueueOutcome::Rejected;
        }
        self.worker.try_record_frame(DiagnosticOwnedFrame {
            sequence: frame.sequence,
            monotonic_start_ms: frame.monotonic_start_ms,
            monotonic_end_ms: frame.monotonic_end_ms,
            pixels: Arc::clone(&frame.pixels),
        })
    }

    /// Records one screen-predicate result against the same immutable run and live-frame binding.
    ///
    /// Queueing is non-blocking and diagnostic failure does not change the recognition observation.
    pub fn record_screen_observation(
        &mut self,
        observation: &RecognitionObservation<'_>,
    ) -> DiagnosticEnqueueOutcome {
        let frame = observation.frame();
        if !self.matches_frame(frame)
            || observation.canonical_layout_sha256() != self.canonical_layout_sha256
        {
            self.worker
                .record_external_error(DiagnosticErrorType::InvalidConfiguration, frame.sequence);
            return DiagnosticEnqueueOutcome::Rejected;
        }
        let screen = match observation.screen() {
            scorepeek::recognition::ScreenClass::Result => DiagnosticScreen::Result,
            scorepeek::recognition::ScreenClass::MusicSelect => DiagnosticScreen::MusicSelection,
            scorepeek::recognition::ScreenClass::Unknown => DiagnosticScreen::Unknown,
        };
        self.worker.try_record_fact(DiagnosticFact {
            sequence: frame.sequence,
            monotonic_start_ms: frame.monotonic_start_ms,
            monotonic_end_ms: frame.monotonic_end_ms,
            operation: DiagnosticOperation::InspectRecognition,
            status: DiagnosticOperationStatus::Success,
            error_type: None,
            detail: DiagnosticDetail::ScreenObservation { screen },
        })
    }

    /// Records a typed screen-inspection failure without replacing its application error.
    pub fn record_recognition_failure(
        &mut self,
        frame: &BoundCanonicalFrame,
    ) -> DiagnosticEnqueueOutcome {
        if !self.matches_frame(frame) {
            self.worker
                .record_external_error(DiagnosticErrorType::InvalidConfiguration, frame.sequence);
            return DiagnosticEnqueueOutcome::Rejected;
        }
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
            ScreenClass::Result => DiagnosticScreen::Result,
            ScreenClass::MusicSelect => DiagnosticScreen::MusicSelection,
            ScreenClass::Unknown => {
                self.worker
                    .record_external_error(DiagnosticErrorType::InvalidConfiguration, sequence);
                return DiagnosticEnqueueOutcome::Rejected;
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
                self.worker
                    .record_external_error(DiagnosticErrorType::InvalidConfiguration, sequence);
                return DiagnosticEnqueueOutcome::Rejected;
            }
            Err(failed_field) => {
                let Some(field) = diagnostic_text_field(screen, failed_field) else {
                    self.worker
                        .record_external_error(DiagnosticErrorType::InvalidConfiguration, sequence);
                    return DiagnosticEnqueueOutcome::Rejected;
                };
                (
                    DiagnosticOperationStatus::Error,
                    Some(DiagnosticFactErrorType::FieldObservationFailed),
                    0,
                    match screen {
                        ScreenClass::Result => 4,
                        ScreenClass::MusicSelect => 1,
                        ScreenClass::Unknown => unreachable!("unknown screen was rejected above"),
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

    pub(crate) fn reject_field_observation(&mut self, sequence: u64) -> DiagnosticEnqueueOutcome {
        self.worker
            .record_external_error(DiagnosticErrorType::InvalidConfiguration, sequence);
        DiagnosticEnqueueOutcome::Rejected
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
        self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
    ) -> DiagnosticFinishOutcome {
        self.worker
            .finish(status, monotonic_end_ms, DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT)
    }

    fn with_worker(descriptor: DiagnosticRunDescriptor, worker: DiagnosticWorkerHandle) -> Self {
        Self {
            capture_generation: descriptor.binding.capture_generation,
            capture_profile_sha256: descriptor.binding.capture_profile_sha256,
            normalizer_sha256: descriptor.binding.normalizer_sha256,
            canonical_layout_sha256: descriptor.binding.canonical_layout_sha256,
            worker,
        }
    }

    pub(crate) fn matches_frame(&self, frame: &BoundCanonicalFrame) -> bool {
        frame.capture_generation == self.capture_generation
            && frame.capture_profile_sha256 == self.capture_profile_sha256
            && frame.normalizer_sha256 == self.normalizer_sha256
    }

    #[cfg(test)]
    pub(crate) fn start_for_test(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        capacity: usize,
    ) -> Self {
        let worker =
            DiagnosticWorkerHandle::start_for_test(root, descriptor.clone(), policy, capacity);
        Self::with_worker(descriptor, worker)
    }

    #[cfg(test)]
    pub(crate) fn start_with_supervisor_for_test(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        supervisor: &std::sync::Mutex<std::sync::Weak<()>>,
    ) -> Self {
        let worker = DiagnosticWorkerHandle::start_with_supervisor_for_test(
            root,
            descriptor.clone(),
            policy,
            supervisor,
        );
        Self::with_worker(descriptor, worker)
    }
}

fn diagnostic_text_field(
    screen: ScreenClass,
    field: ScreenTextField,
) -> Option<DiagnosticTextField> {
    Some(match (screen, field) {
        (ScreenClass::Result, ScreenTextField::ResultTitle) => DiagnosticTextField::ResultTitle,
        (ScreenClass::Result, ScreenTextField::ResultArtist) => DiagnosticTextField::ResultArtist,
        (ScreenClass::Result, ScreenTextField::ResultClearType) => {
            DiagnosticTextField::ResultClearType
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
    use crate::diagnostic_recording::{
        DiagnosticBinding, DiagnosticCompleteness, DiagnosticResource,
    };
    use crate::recognition_live::RecognitionObservation;
    use scorepeek::recognition::{
        CanonicalLayout, DynamicTextObservation, FieldNotObserved, FieldNotObservedReason,
        ResultScreenFieldObservations, ScreenClass, ScreenFieldObservationError,
        ScreenFieldObservations, ScreenTextField,
    };
    use std::fs;

    fn descriptor(run_id: &str, generation: u64) -> DiagnosticRunDescriptor {
        DiagnosticRunDescriptor {
            run_id: run_id.to_owned(),
            monotonic_start_ms: 0,
            resource: DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "1".repeat(64),
            },
            binding: DiagnosticBinding {
                capture_generation: generation,
                capture_profile_sha256: "2".repeat(64),
                normalizer_sha256: "3".repeat(64),
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
            capture_generation: generation,
            sequence,
            monotonic_start_ms: time,
            monotonic_end_ms: time + 16,
            capture_profile_sha256: "2".repeat(64),
            normalizer_sha256: "3".repeat(64),
            pixels: Arc::new(
                vec![7; crate::diagnostic_recording::CANONICAL_BYTES].into_boxed_slice(),
            ),
        }
    }

    fn result_fields(text: &str) -> ScreenFieldObservations {
        let text = || DynamicTextObservation {
            input_width: 320,
            output_timesteps: 20,
            open_text: text.to_owned(),
        };
        let unimplemented = FieldNotObserved {
            reason: FieldNotObservedReason::ObserverNotImplemented,
        };
        ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text(),
            artist: text(),
            clear_type: text(),
            difficulty: unimplemented,
            level: unimplemented,
            notes: unimplemented,
            current_score: unimplemented,
        })
    }

    #[test]
    fn offer_is_recognition_independent_and_reuses_owned_pixels() {
        let root = tempfile::tempdir().unwrap();
        let canonical = frame(1, 1, 0);
        let pixels = Arc::clone(&canonical.pixels);
        let mut bridge = DiagnosticBridge::start_for_test(
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
        let canonical = frame(1, 1, 17);
        let pixels = Arc::clone(&canonical.pixels);
        let observation = RecognitionObservation::inspect(&canonical).unwrap();
        assert_eq!(observation.screen(), ScreenClass::Unknown);
        let mut bridge = DiagnosticBridge::start_for_test(
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
        assert_eq!(manifest["facts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn binding_change_is_rejected_and_makes_the_old_run_partial() {
        let root = tempfile::tempdir().unwrap();
        let mut bridge = DiagnosticBridge::start_for_test(
            root.path(),
            descriptor("old-generation", 1),
            DiagnosticPolicy::default(),
            2,
        );
        assert_eq!(
            bridge.offer(&frame(2, 1, 0)),
            DiagnosticEnqueueOutcome::Rejected
        );
        let outcome = bridge.finish(DiagnosticRunStatus::Success, 16);
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Partial));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("old-generation/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["last_error_type"], "invalid_configuration");
        assert_eq!(manifest["frames"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn screen_observation_rejects_a_different_layout_binding() {
        let root = tempfile::tempdir().unwrap();
        let canonical = frame(1, 1, 17);
        let observation = RecognitionObservation::inspect(&canonical).unwrap();
        let mut mismatched = descriptor("layout-mismatch", 1);
        mismatched.binding.canonical_layout_sha256 = "4".repeat(64);
        let mut bridge = DiagnosticBridge::start_for_test(
            root.path(),
            mismatched,
            DiagnosticPolicy::default(),
            2,
        );
        assert_eq!(
            bridge.record_screen_observation(&observation),
            DiagnosticEnqueueOutcome::Rejected
        );
        assert_eq!(
            bridge.finish(DiagnosticRunStatus::Success, 33).completeness,
            Some(DiagnosticCompleteness::Partial)
        );
    }

    #[test]
    fn generation_rollover_creates_two_independent_runs() {
        let root = tempfile::tempdir().unwrap();
        let supervisor = std::sync::Mutex::new(std::sync::Weak::new());
        for generation in [1, 2] {
            let run_id = format!("generation-{generation}");
            let mut bridge = DiagnosticBridge::start_with_supervisor_for_test(
                root.path(),
                descriptor(&run_id, generation),
                DiagnosticPolicy::default(),
                &supervisor,
            );
            assert_eq!(
                bridge.offer(&frame(generation, 1, 0)),
                DiagnosticEnqueueOutcome::Enqueued
            );
            assert_eq!(
                bridge.finish(DiagnosticRunStatus::Success, 16).completeness,
                Some(DiagnosticCompleteness::Complete)
            );
        }
        assert!(root.path().join("generation-1/manifest.json").is_file());
        assert!(root.path().join("generation-2/manifest.json").is_file());
    }

    #[test]
    fn opt_out_preserves_live_result_and_writes_nothing() {
        let root = tempfile::tempdir().unwrap();
        let canonical = frame(1, 1, 0);
        let observation = RecognitionObservation::inspect(&canonical).unwrap();
        let mut bridge = DiagnosticBridge::start_for_test(
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
        let mut bridge = DiagnosticBridge::start_for_test(
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
        let filename = manifest["facts"][0]["filename"].as_str().unwrap();
        let fact_bytes = fs::read(run.join(filename)).unwrap();
        let fact: serde_json::Value = serde_json::from_slice(&fact_bytes).unwrap();
        assert_eq!(fact["fact"]["operation"], "observe_fields");
        assert_eq!(fact["fact"]["detail"]["kind"], "field_observation");
        assert_eq!(fact["fact"]["detail"]["observed_fields"], 3);
        assert_eq!(fact["fact"]["detail"]["unimplemented_fields"], 4);
        assert!(
            !String::from_utf8(fact_bytes)
                .unwrap()
                .contains("OCR CONTENT SENTINEL")
        );

        let disabled_root = tempfile::tempdir().unwrap();
        let disabled_output = Ok::<_, ScreenFieldObservationError<&'static str>>(result_fields(
            "OCR CONTENT SENTINEL",
        ));
        let mut disabled = DiagnosticBridge::start_for_test(
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
        let mut bridge = DiagnosticBridge::start_for_test(
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
        let filename = manifest["facts"][0]["filename"].as_str().unwrap();
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
        let mut bridge = DiagnosticBridge::start_for_test(
            root.path(),
            descriptor("worker-loss", 1),
            DiagnosticPolicy::default(),
            0,
        );
        assert_eq!(
            bridge.offer(&frame(1, 1, 0)),
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
