use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use scorepeek::recognition::{
    CanonicalLayout, MusicSelectScreenRgb8Crops, RecognitionError, ResultScreenRgb8Crops,
    ScreenClass, ScreenFieldObservationError, ScreenFieldObservations, ScreenPredicateObservation,
    ScreenRgb8Crops, inspect_canonical_rgb8, route_screen_rgb8_crops,
};

use crate::diagnostic_live::{BoundCanonicalFrame, DiagnosticBridge};
use crate::diagnostic_recording::{
    DiagnosticErrorType, DiagnosticFinishOutcome, DiagnosticPolicy, DiagnosticRunDescriptor,
    DiagnosticRunStatus,
};
use crate::diagnostic_worker::DiagnosticEnqueueOutcome;
use crate::recognition_live::field_observer::{
    BoundFieldObservation, FieldObserverFinishOutcome, FieldObserverFinishStatus,
    FieldObserverOfferError,
};
use crate::recognition_live::screen_field_observer::RegisteredScreenFieldObservation;

pub mod field_observer;
pub mod field_session;
pub mod screen_field_observer;
pub mod text_observer_pool;

fn duration_us(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FieldInputPolicy {
    Route,
    SkipBusy,
}

/// One screen-predicate result that borrows its immutable live capture evidence.
///
/// The result cannot outlive or detach from the profile- and generation-bearing frame that was
/// inspected. It carries no accepted field or event authority.
#[derive(Debug)]
pub struct RecognitionObservation<'a> {
    frame: &'a BoundCanonicalFrame,
    canonical_layout_sha256: String,
    predicate: ScreenPredicateObservation,
}

/// Owned, pure classification and crop preparation for one canonical RGB8 frame.
///
/// Replay may prepare this value on a shared worker before the session-local ordered timeline
/// consumes it. Construction performs no diagnostic, episode, attempt, or event mutation.
#[derive(Debug)]
pub struct PreparedRecognitionFrame {
    started: Instant,
    pixel_address: usize,
    pixel_length: usize,
    predicate: ScreenPredicateObservation,
    field_inputs: Option<ScreenRgb8Crops>,
    screen_classification_us: u64,
    crop_prepare_us: Option<u64>,
}

impl PreparedRecognitionFrame {
    /// Classifies and prepares all applicable field crops from one canonical RGB8 frame.
    ///
    /// # Errors
    /// Returns an error when the canonical pixels or embedded layouts are invalid.
    pub fn prepare(pixels: &[u8]) -> Result<Self, RecognitionError> {
        Self::prepare_since(pixels, Instant::now())
    }

    /// Classifies and prepares one frame while retaining an earlier scheduler-admission origin.
    ///
    /// # Errors
    /// Returns an error when the canonical pixels or embedded layouts are invalid.
    pub fn prepare_since(pixels: &[u8], started: Instant) -> Result<Self, RecognitionError> {
        let classification_started = Instant::now();
        let predicate = inspect_canonical_rgb8(pixels)?;
        let screen_classification_us = duration_us(classification_started.elapsed());
        let crop_started = Instant::now();
        let field_inputs = match predicate.screen {
            ScreenClass::Result | ScreenClass::MusicSelect => {
                Some(route_screen_rgb8_crops(pixels, predicate.screen)?)
            }
            ScreenClass::ModeSelect
            | ScreenClass::DecideTransition
            | ScreenClass::Play
            | ScreenClass::Unknown => None,
        };
        let crop_prepare_us = field_inputs
            .as_ref()
            .map(|_| duration_us(crop_started.elapsed()));
        Ok(Self {
            started,
            pixel_address: pixels.as_ptr() as usize,
            pixel_length: pixels.len(),
            predicate,
            field_inputs,
            screen_classification_us,
            crop_prepare_us,
        })
    }

    #[must_use]
    pub const fn screen_classification_us(&self) -> u64 {
        self.screen_classification_us
    }

    #[must_use]
    pub const fn crop_prepare_us(&self) -> Option<u64> {
        self.crop_prepare_us
    }
}

impl<'a> RecognitionObservation<'a> {
    /// Applies the embedded screen predicate to one admitted live canonical owner.
    ///
    /// # Errors
    /// Returns an error when the fixed canonical pixel or embedded layout contract is invalid.
    pub fn inspect(frame: &'a BoundCanonicalFrame) -> Result<Self, RecognitionError> {
        Ok(Self {
            frame,
            canonical_layout_sha256: CanonicalLayout::sha256(),
            predicate: inspect_canonical_rgb8(frame.pixels())?,
        })
    }

    #[must_use]
    pub const fn screen(&self) -> ScreenClass {
        self.predicate.screen
    }

    #[must_use]
    pub(crate) const fn frame(&self) -> &BoundCanonicalFrame {
        self.frame
    }

    #[must_use]
    pub(crate) fn canonical_layout_sha256(&self) -> &str {
        &self.canonical_layout_sha256
    }

    pub(crate) const fn predicate(&self) -> &ScreenPredicateObservation {
        &self.predicate
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecognitionSessionError {
    InvalidBinding,
    CanonicalLayoutMismatch,
    BindingUnchanged,
    FrameBindingMismatch,
    RecognitionFailed,
}

/// Recognition and diagnostic outcomes for one frame under one immutable live binding.
///
/// Diagnostic queue state is reported separately and never changes the recognition observation.
#[derive(Debug)]
pub struct RecognitionFrameResult<'a> {
    pub observation: RecognitionObservation<'a>,
    pub field_inputs: Option<BoundScreenRgb8Crops<'a>>,
    pub diagnostic_frame: DiagnosticEnqueueOutcome,
    pub diagnostic_fact: DiagnosticEnqueueOutcome,
    pub timing: FrameProcessingTiming,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub struct FrameProcessingTiming {
    #[serde(skip)]
    pub(crate) frame_started: Instant,
    pub source_sequence: u64,
    pub monotonic_start_ms: u64,
    pub monotonic_end_ms: u64,
    pub screen: ScreenClass,
    pub screen_classification_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop_prepare_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_resolver_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_resolver_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_us: Option<u64>,
    pub frame_processing_wall_us: u64,
}

impl FrameProcessingTiming {
    pub(crate) fn add_live_processing(&mut self, timing: LiveEventProcessingTiming) {
        add_optional_duration(&mut self.screen_resolver_us, timing.screen_resolver_us);
        add_optional_duration(&mut self.attempt_resolver_us, timing.attempt_resolver_us);
        add_optional_duration(&mut self.output_us, timing.output_us);
    }

    pub(crate) fn finish_wall(&mut self) {
        self.frame_processing_wall_us = duration_us(self.frame_started.elapsed());
    }
}

fn add_optional_duration(total: &mut Option<u64>, value: Option<u64>) {
    if let Some(value) = value {
        *total = Some(total.unwrap_or(0).saturating_add(value));
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LiveEventProcessingTiming {
    pub screen_resolver_us: Option<u64>,
    pub attempt_resolver_us: Option<u64>,
    pub output_us: Option<u64>,
}

impl LiveEventProcessingTiming {
    pub(crate) fn add(&mut self, other: Self) {
        add_optional_duration(&mut self.screen_resolver_us, other.screen_resolver_us);
        add_optional_duration(&mut self.attempt_resolver_us, other.attempt_resolver_us);
        add_optional_duration(&mut self.output_us, other.output_us);
    }
}

/// Screen-local field inputs that remain attached to their admitted live frame owner.
///
/// These crops are observer inputs only. They carry no accepted field, song, or event authority.
#[derive(Debug)]
pub struct BoundScreenRgb8Crops<'a> {
    frame: &'a BoundCanonicalFrame,
    run_binding: Arc<RecognitionRunBinding>,
    crops: ScreenRgb8Crops,
}

#[derive(Debug)]
struct RecognitionRunBinding {
    run_id: String,
    binding_sha256: String,
}

/// A borrowed view of one opaque live screen-crop owner.
#[derive(Clone, Copy, Debug)]
pub enum BoundScreenRgb8CropsRef<'a> {
    Result(&'a ResultScreenRgb8Crops),
    MusicSelect(&'a MusicSelectScreenRgb8Crops),
}

impl BoundScreenRgb8Crops<'_> {
    #[must_use]
    pub const fn screen(&self) -> ScreenClass {
        match &self.crops {
            ScreenRgb8Crops::Result(_) => ScreenClass::Result,
            ScreenRgb8Crops::MusicSelect(_) => ScreenClass::MusicSelect,
        }
    }

    #[must_use]
    pub const fn crops(&self) -> BoundScreenRgb8CropsRef<'_> {
        match &self.crops {
            ScreenRgb8Crops::Result(crops) => BoundScreenRgb8CropsRef::Result(crops),
            ScreenRgb8Crops::MusicSelect(crops) => BoundScreenRgb8CropsRef::MusicSelect(crops),
        }
    }

    #[must_use]
    pub const fn frame(&self) -> &BoundCanonicalFrame {
        self.frame
    }
}

pub struct RecognitionTransition {
    pub finished: DiagnosticFinishOutcome,
    pub binding_change_diagnostic: DiagnosticEnqueueOutcome,
    pub next: RecognitionSession,
}

/// Application-owned recognition lifetime for one immutable diagnostic binding.
///
/// This is a resource boundary, not an inferred game session. A different capture generation or
/// recognition input is rejected; `transition` records the explicit change, finishes the old run,
/// and only then starts the replacement session.
pub struct RecognitionSession {
    run_binding: Arc<RecognitionRunBinding>,
    bridge: DiagnosticBridge,
    last_sequence: Option<u64>,
}

impl RecognitionSession {
    /// Starts a source-bound session only for the embedded canonical layout.
    ///
    /// # Errors
    /// Returns a typed error for an invalid or noncanonical descriptor.
    pub fn start(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Result<Self, RecognitionSessionError> {
        let binding_sha256 = validate_descriptor(&descriptor)?;
        let run_id = descriptor.run_id.clone();
        let bridge = DiagnosticBridge::start(root, descriptor, policy);
        Ok(Self {
            run_binding: Arc::new(RecognitionRunBinding {
                run_id,
                binding_sha256,
            }),
            bridge,
            last_sequence: None,
        })
    }

    pub(crate) fn start_named(
        root: &Path,
        directory_name: &str,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Result<Self, RecognitionSessionError> {
        let binding_sha256 = validate_descriptor(&descriptor)?;
        let run_id = descriptor.run_id.clone();
        let bridge = DiagnosticBridge::start_named(root, directory_name, descriptor, policy);
        Ok(Self {
            run_binding: Arc::new(RecognitionRunBinding {
                run_id,
                binding_sha256,
            }),
            bridge,
            last_sequence: None,
        })
    }

    /// Inspects one frame after the independent diagnostic sampler sees the same owner.
    ///
    /// # Errors
    /// Returns a typed error before acceptance when the frame binding or recognition fails.
    pub fn inspect<'a>(
        &mut self,
        frame: &'a BoundCanonicalFrame,
    ) -> Result<RecognitionFrameResult<'a>, RecognitionSessionError> {
        self.inspect_with_field_policy(frame, FieldInputPolicy::Route)
    }

    pub(crate) fn inspect_with_field_policy<'a>(
        &mut self,
        frame: &'a BoundCanonicalFrame,
        field_policy: FieldInputPolicy,
    ) -> Result<RecognitionFrameResult<'a>, RecognitionSessionError> {
        let frame_started = Instant::now();
        if !self.bridge.matches_frame(frame) {
            return Err(RecognitionSessionError::FrameBindingMismatch);
        }
        self.last_sequence = Some(frame.sequence());
        let classification_started = Instant::now();
        let Ok(observation) = RecognitionObservation::inspect(frame) else {
            let _ = self.bridge.offer(frame);
            let _ = self.bridge.record_recognition_failure(frame);
            return Err(RecognitionSessionError::RecognitionFailed);
        };
        let screen_classification_us = duration_us(classification_started.elapsed());
        let diagnostic_frame = self.bridge.record_frame_for_observation(&observation);
        let crop_started = Instant::now();
        let field_inputs = match observation.screen() {
            ScreenClass::ModeSelect
            | ScreenClass::DecideTransition
            | ScreenClass::Play
            | ScreenClass::Unknown => None,
            ScreenClass::Result | ScreenClass::MusicSelect
                if field_policy == FieldInputPolicy::SkipBusy =>
            {
                None
            }
            screen @ (ScreenClass::Result | ScreenClass::MusicSelect) => {
                let Ok(routed) = route_screen_rgb8_crops(frame.pixels(), screen) else {
                    let _ = self.bridge.record_recognition_failure(frame);
                    return Err(RecognitionSessionError::RecognitionFailed);
                };
                Some(BoundScreenRgb8Crops {
                    frame,
                    run_binding: Arc::clone(&self.run_binding),
                    crops: routed,
                })
            }
        };
        let crop_prepare_us = field_inputs
            .as_ref()
            .map(|_| duration_us(crop_started.elapsed()));
        let diagnostic_fact = self.bridge.record_screen_observation(&observation);
        let timing = FrameProcessingTiming {
            frame_started,
            source_sequence: frame.sequence(),
            monotonic_start_ms: frame.monotonic_start_ms(),
            monotonic_end_ms: frame.monotonic_end_ms(),
            screen: observation.screen(),
            screen_classification_us,
            crop_prepare_us,
            screen_resolver_us: None,
            attempt_resolver_us: None,
            output_us: None,
            frame_processing_wall_us: duration_us(frame_started.elapsed()),
        };
        Ok(RecognitionFrameResult {
            observation,
            field_inputs,
            diagnostic_frame,
            diagnostic_fact,
            timing,
        })
    }

    /// Admits one already prepared replay frame into this session's ordered recognition path.
    ///
    /// Preparation is pure and may complete out of order. Callers must invoke this method in
    /// source-sequence order; all diagnostic and semantic authority remains here.
    ///
    /// # Errors
    /// Returns an error when the prepared frame does not match this session's immutable binding.
    pub fn inspect_prepared<'a>(
        &mut self,
        frame: &'a BoundCanonicalFrame,
        prepared: PreparedRecognitionFrame,
    ) -> Result<RecognitionFrameResult<'a>, RecognitionSessionError> {
        if !self.bridge.matches_frame(frame) {
            return Err(RecognitionSessionError::FrameBindingMismatch);
        }
        if prepared.pixel_address != frame.pixels().as_ptr() as usize
            || prepared.pixel_length != frame.pixels().len()
        {
            let _ = self.bridge.offer(frame);
            let _ = self.bridge.record_recognition_failure(frame);
            return Err(RecognitionSessionError::RecognitionFailed);
        }
        self.last_sequence = Some(frame.sequence());
        let observation = RecognitionObservation {
            frame,
            canonical_layout_sha256: CanonicalLayout::sha256(),
            predicate: prepared.predicate,
        };
        let diagnostic_frame = self.bridge.record_frame_for_observation(&observation);
        let field_inputs = prepared.field_inputs.map(|crops| BoundScreenRgb8Crops {
            frame,
            run_binding: Arc::clone(&self.run_binding),
            crops,
        });
        let diagnostic_fact = self.bridge.record_screen_observation(&observation);
        let timing = FrameProcessingTiming {
            frame_started: prepared.started,
            source_sequence: frame.sequence(),
            monotonic_start_ms: frame.monotonic_start_ms(),
            monotonic_end_ms: frame.monotonic_end_ms(),
            screen: observation.screen(),
            screen_classification_us: prepared.screen_classification_us,
            crop_prepare_us: prepared.crop_prepare_us,
            screen_resolver_us: None,
            attempt_resolver_us: None,
            output_us: None,
            frame_processing_wall_us: duration_us(prepared.started.elapsed()),
        };
        Ok(RecognitionFrameResult {
            observation,
            field_inputs,
            diagnostic_frame,
            diagnostic_fact,
            timing,
        })
    }

    pub fn record_frame_processing_timing(
        &mut self,
        timing: FrameProcessingTiming,
        field_status: crate::diagnostic_recording::FrameFieldStatus,
        field_timing: Option<&screen_field_observer::RecognitionProcessingTiming>,
    ) -> DiagnosticEnqueueOutcome {
        self.bridge
            .record_frame_processing_timing(timing, field_status, field_timing)
    }

    /// Records value-free diagnostics for one worker-bound field result.
    ///
    /// The returned diagnostic enqueue outcome is independent of and cannot mutate `observation`.
    /// # Panics
    ///
    /// Panics if the field observation belongs to another run binding.
    pub fn record_field_observation<T, E>(
        &mut self,
        observation: &BoundFieldObservation<Result<T, ScreenFieldObservationError<E>>>,
    ) -> DiagnosticEnqueueOutcome
    where
        T: DiagnosticScreenFieldObservation,
    {
        assert_eq!(observation.binding().run_id(), self.run_binding.run_id);
        assert_eq!(
            observation.binding().identity_sha256(),
            self.run_binding.binding_sha256
        );
        self.bridge.record_field_observation_summary(
            observation.sequence(),
            observation.monotonic_start_ms(),
            observation.monotonic_end_ms(),
            observation.screen(),
            observation
                .output()
                .as_ref()
                .map(DiagnosticScreenFieldObservation::diagnostic_fields)
                .map_err(|error| error.field),
        )
    }

    pub(crate) fn record_field_observer_offer_failure(
        &mut self,
        sequence: u64,
        error: FieldObserverOfferError,
    ) {
        let error_type = match error {
            FieldObserverOfferError::BindingMismatch => {
                unreachable!("field job must match its run binding")
            }
            FieldObserverOfferError::OutstandingLimit => {
                DiagnosticErrorType::FieldObserverOutstandingLimit
            }
            FieldObserverOfferError::QueueFull => DiagnosticErrorType::FieldObserverQueueFull,
            FieldObserverOfferError::WorkerUnavailable => {
                DiagnosticErrorType::FieldObserverUnavailable
            }
        };
        self.bridge
            .record_field_observer_degradation(error_type, sequence);
    }

    pub(crate) fn record_field_observer_unavailable(&mut self, sequence: u64) {
        self.bridge.record_field_observer_degradation(
            DiagnosticErrorType::FieldObserverUnavailable,
            sequence,
        );
    }

    pub(crate) fn record_sampling_summary(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        summary: crate::diagnostic_recording::RecognitionSamplingSummary,
    ) {
        let _ = self
            .bridge
            .record_sampling_summary(sequence, monotonic_ms, summary);
    }

    pub(crate) fn record_recognition_busy_skip(
        &mut self,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
    ) -> DiagnosticEnqueueOutcome {
        self.bridge
            .record_recognition_busy_skip(sequence, monotonic_start_ms, monotonic_end_ms)
    }

    pub(crate) fn record_field_observation_busy_skip(
        &mut self,
        frame: &BoundCanonicalFrame,
        screen: ScreenClass,
    ) -> DiagnosticEnqueueOutcome {
        self.bridge.record_field_observation_busy_skip(
            frame.sequence(),
            frame.monotonic_start_ms(),
            frame.monotonic_end_ms(),
            screen,
        )
    }

    pub(crate) fn record_abandoned_field_observation(&mut self, sequence: u64) {
        self.bridge.record_field_observer_degradation(
            DiagnosticErrorType::FieldObservationAbandoned,
            sequence,
        );
    }

    pub(crate) fn record_field_observer_finish(&mut self, outcome: FieldObserverFinishOutcome) {
        let terminal_error = match outcome.status {
            FieldObserverFinishStatus::Timeout => {
                Some(DiagnosticErrorType::FieldObserverFinishTimeout)
            }
            FieldObserverFinishStatus::WorkerUnavailable => {
                Some(DiagnosticErrorType::FieldObserverUnavailable)
            }
            FieldObserverFinishStatus::Complete => None,
        };
        if let Some(error_type) = terminal_error {
            self.bridge
                .record_unbound_field_observer_degradation(error_type, 1);
        }
    }

    #[must_use]
    pub fn finish(
        self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
    ) -> DiagnosticFinishOutcome {
        self.bridge.finish(status, monotonic_end_ms)
    }

    /// Replaces this session after the old run has recorded and finished its binding change.
    ///
    /// # Errors
    /// Returns a typed error without rotating for an invalid or unchanged next binding.
    pub fn transition(
        mut self,
        root: &Path,
        next_descriptor: DiagnosticRunDescriptor,
        next_policy: DiagnosticPolicy,
        monotonic_ms: u64,
    ) -> Result<RecognitionTransition, RecognitionSessionError> {
        let next_binding_sha256 = validate_descriptor(&next_descriptor)?;
        if next_binding_sha256 == self.run_binding.binding_sha256 {
            return Err(RecognitionSessionError::BindingUnchanged);
        }
        let binding_change_diagnostic = self.bridge.record_binding_change(
            self.last_sequence.unwrap_or(0),
            monotonic_ms,
            next_binding_sha256,
        );
        let finished = self
            .bridge
            .finish(DiagnosticRunStatus::Success, monotonic_ms);
        let next = Self::start(root, next_descriptor, next_policy)?;
        Ok(RecognitionTransition {
            finished,
            binding_change_diagnostic,
            next,
        })
    }

    #[cfg(test)]
    fn start_with_supervisor_for_test(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        supervisor: &std::sync::Mutex<std::sync::Weak<()>>,
    ) -> Result<Self, RecognitionSessionError> {
        let binding_sha256 = validate_descriptor(&descriptor)?;
        let run_id = descriptor.run_id.clone();
        let bridge =
            DiagnosticBridge::start_with_supervisor_for_test(root, descriptor, policy, supervisor);
        Ok(Self {
            run_binding: Arc::new(RecognitionRunBinding {
                run_id,
                binding_sha256,
            }),
            bridge,
            last_sequence: None,
        })
    }
}

#[doc(hidden)]
pub trait DiagnosticScreenFieldObservation {
    fn diagnostic_fields(&self) -> &ScreenFieldObservations;
}

impl DiagnosticScreenFieldObservation for ScreenFieldObservations {
    fn diagnostic_fields(&self) -> &ScreenFieldObservations {
        self
    }
}

impl DiagnosticScreenFieldObservation for RegisteredScreenFieldObservation {
    fn diagnostic_fields(&self) -> &ScreenFieldObservations {
        self.fields()
    }
}

fn validate_descriptor(
    descriptor: &DiagnosticRunDescriptor,
) -> Result<String, RecognitionSessionError> {
    let binding = &descriptor.binding;
    if binding.canonical_layout_sha256 != CanonicalLayout::sha256() {
        return Err(RecognitionSessionError::CanonicalLayoutMismatch);
    }
    if !descriptor.is_valid() {
        return Err(RecognitionSessionError::InvalidBinding);
    }
    binding
        .identity_sha256()
        .ok_or(RecognitionSessionError::InvalidBinding)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use scorepeek::recognition::{CanonicalLayout, ScreenClass};

    use super::*;
    use crate::diagnostic_recording::{DiagnosticBinding, DiagnosticResource};

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

    fn solid_frame(color: [u8; 3], sequence: u64) -> BoundCanonicalFrame {
        let mut pixels = Vec::with_capacity(crate::diagnostic_recording::CANONICAL_BYTES);
        for _ in 0..crate::diagnostic_recording::CANONICAL_BYTES / 3 {
            pixels.extend_from_slice(&color);
        }
        if color == [200, 100, 20] {
            for y in [451, 655] {
                for x in 0..518 {
                    pixels[(y * 1920 + x) * 3..][..3].copy_from_slice(&[0, 0, 0]);
                }
            }
        } else if color == [0, 180, 220] {
            let label = CanonicalLayout::load().unwrap().music_select.label;
            for y in label.y..label.y + label.height {
                for x in label.x..label.x + label.width {
                    pixels[(y as usize * 1920 + x as usize) * 3..][..3]
                        .copy_from_slice(&[220, 220, 220]);
                }
            }
            let encoded = include_bytes!("../assets/screen-references-v1/music-select.qoi");
            let (header, reference) = qoi::decode_to_vec(encoded).unwrap();
            for y in 0..header.height as usize {
                let source_start = y * header.width as usize * 3;
                let target_start = ((50 + y) * 1920 + 50) * 3;
                pixels[target_start..target_start + header.width as usize * 3].copy_from_slice(
                    &reference[source_start..source_start + header.width as usize * 3],
                );
            }
        }
        BoundCanonicalFrame::for_test_pixels(1, sequence, 0, pixels.into_boxed_slice())
    }

    #[test]
    fn diagnostic_opt_out_does_not_change_recognition() {
        let root = tempfile::tempdir().unwrap();
        let frame = BoundCanonicalFrame::for_test(1, 1, 0);
        let mut session = RecognitionSession::start(
            root.path(),
            descriptor("disabled-session", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        let result = session.inspect(&frame).unwrap();
        assert_eq!(result.observation.screen(), ScreenClass::Unknown);
        assert!(result.field_inputs.is_none());
        assert_eq!(result.diagnostic_frame, DiagnosticEnqueueOutcome::Disabled);
        assert_eq!(result.diagnostic_fact, DiagnosticEnqueueOutcome::Disabled);
        assert_eq!(
            session
                .finish(DiagnosticRunStatus::Success, 16)
                .completeness,
            None
        );
    }

    #[test]
    fn frame_wall_timer_keeps_the_inspection_origin_until_output_finishes() {
        let root = tempfile::tempdir().unwrap();
        let frame = BoundCanonicalFrame::for_test(1, 1, 0);
        let mut session = RecognitionSession::start(
            root.path(),
            descriptor("frame-wall-origin", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        let mut timing = session.inspect(&frame).unwrap().timing;
        let inspection_wall_us = timing.frame_processing_wall_us;
        std::thread::sleep(std::time::Duration::from_millis(1));
        timing.finish_wall();
        assert!(timing.frame_processing_wall_us > inspection_wall_us);
    }

    #[test]
    fn supported_screens_route_only_their_measured_field_inputs() {
        let root = tempfile::tempdir().unwrap();
        let policy = DiagnosticPolicy {
            enabled: false,
            ..DiagnosticPolicy::default()
        };

        let result_frame = solid_frame([200, 100, 20], 1);
        let mut result_session =
            RecognitionSession::start(root.path(), descriptor("result-fields", 1), policy.clone())
                .unwrap();
        let result = result_session.inspect(&result_frame).unwrap();
        assert_eq!(result.timing.source_sequence, result_frame.sequence());
        assert_eq!(result.timing.screen, ScreenClass::Result);
        assert!(result.timing.crop_prepare_us.is_some());
        assert!(result.timing.frame_processing_wall_us >= result.timing.screen_classification_us);
        let result_fields = result.field_inputs.unwrap();
        assert_eq!(result_fields.screen(), ScreenClass::Result);
        assert!(std::ptr::eq(result_fields.frame(), &raw const result_frame));
        let BoundScreenRgb8CropsRef::Result(crops) = result_fields.crops() else {
            panic!("result screen routed to music-select crops");
        };
        let layout = CanonicalLayout::load().unwrap();
        assert_eq!(crops.title.roi, layout.result.title);
        assert_eq!(crops.artist.roi, layout.result.artist);
        assert_eq!(crops.difficulty.roi, layout.result.difficulty);
        assert_eq!(crops.level.roi, layout.result.level);
        assert_eq!(crops.notes.roi, layout.result.notes);
        assert_eq!(crops.current_score.roi, layout.result.current_score);
        assert_eq!(crops.title.pixels()[..3], [200, 100, 20]);

        let music_frame = solid_frame([0, 180, 220], 1);
        let mut music_session =
            RecognitionSession::start(root.path(), descriptor("music-fields", 1), policy).unwrap();
        let music = music_session.inspect(&music_frame).unwrap();
        assert_eq!(music.timing.screen, ScreenClass::MusicSelect);
        assert!(music.timing.crop_prepare_us.is_some());
        let music_fields = music.field_inputs.unwrap();
        assert_eq!(music_fields.screen(), ScreenClass::MusicSelect);
        assert!(std::ptr::eq(music_fields.frame(), &raw const music_frame));
        let BoundScreenRgb8CropsRef::MusicSelect(crops) = music_fields.crops() else {
            panic!("music-select screen routed to result crops");
        };
        assert_eq!(crops.central_title.roi, layout.music_select.selected_title);
        assert_eq!(crops.central_title.pixels()[..3], [0, 180, 220]);
        assert!(!crops.artist.pixels().is_empty());
        assert!(
            crops
                .difficulty_markers
                .as_slots()
                .into_iter()
                .all(|(_, crop)| !crop.pixels().is_empty())
        );
        assert!(!crops.active_list_title.pixels().is_empty());
    }

    #[test]
    fn prepared_replay_inspection_matches_the_direct_recognition_path() {
        let root = tempfile::tempdir().unwrap();
        let frame = solid_frame([200, 100, 20], 7);
        let policy = DiagnosticPolicy {
            enabled: false,
            ..DiagnosticPolicy::default()
        };
        let mut direct_session = RecognitionSession::start(
            root.path(),
            descriptor("direct-preparation", 1),
            policy.clone(),
        )
        .unwrap();
        let direct = direct_session.inspect(&frame).unwrap();
        let direct_screen = direct.observation.screen();
        let direct_crops = direct.field_inputs.unwrap();

        let prepared = PreparedRecognitionFrame::prepare(frame.pixels()).unwrap();
        let mut prepared_session =
            RecognitionSession::start(root.path(), descriptor("parallel-preparation", 1), policy)
                .unwrap();
        let replayed = prepared_session.inspect_prepared(&frame, prepared).unwrap();
        assert_eq!(replayed.observation.screen(), direct_screen);
        let replayed_crops = replayed.field_inputs.unwrap();
        match (direct_crops.crops(), replayed_crops.crops()) {
            (BoundScreenRgb8CropsRef::Result(left), BoundScreenRgb8CropsRef::Result(right)) => {
                assert_eq!(left, right);
            }
            _ => panic!("prepared recognition changed the routed screen"),
        }
    }

    #[test]
    fn prepared_replay_inspection_rejects_another_pixel_owner() {
        let root = tempfile::tempdir().unwrap();
        let prepared_frame = solid_frame([200, 100, 20], 7);
        let another_frame = solid_frame([200, 100, 20], 8);
        let prepared = PreparedRecognitionFrame::prepare(prepared_frame.pixels()).unwrap();
        let mut session = RecognitionSession::start(
            root.path(),
            descriptor("mismatched-preparation", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        assert!(matches!(
            session.inspect_prepared(&another_frame, prepared),
            Err(RecognitionSessionError::RecognitionFailed)
        ));
    }

    #[test]
    fn mismatched_generation_stops_before_recognition() {
        let root = tempfile::tempdir().unwrap();
        let mut session = RecognitionSession::start(
            root.path(),
            descriptor("mismatched-session", 1),
            DiagnosticPolicy::default(),
        )
        .unwrap();
        let frame = BoundCanonicalFrame::for_test(2, 1, 0);
        assert!(matches!(
            session.inspect(&frame),
            Err(RecognitionSessionError::FrameBindingMismatch)
        ));
        let prepared = PreparedRecognitionFrame::prepare(frame.pixels()).unwrap();
        assert!(matches!(
            session.inspect_prepared(&frame, prepared),
            Err(RecognitionSessionError::FrameBindingMismatch)
        ));
        assert_eq!(
            session
                .finish(DiagnosticRunStatus::Success, 16)
                .completeness,
            Some(crate::diagnostic_recording::DiagnosticCompleteness::Complete)
        );
    }

    #[test]
    fn transition_records_next_binding_and_finishes_old_run_first() {
        let root = tempfile::tempdir().unwrap();
        let supervisor = std::sync::Mutex::new(std::sync::Weak::new());
        let old_descriptor = descriptor("old-session", 1);
        let mut old = RecognitionSession::start_with_supervisor_for_test(
            root.path(),
            old_descriptor,
            DiagnosticPolicy::default(),
            &supervisor,
        )
        .unwrap();
        old.inspect(&BoundCanonicalFrame::for_test(1, 1, 0))
            .unwrap();
        let first_fact = root.path().join("old-session/facts.ndjson");
        let deadline = Instant::now() + Duration::from_secs(1);
        while first_fact.metadata().map_or(0, |metadata| metadata.len()) == 0
            && Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert!(first_fact.metadata().unwrap().len() > 0);
        let next_descriptor = descriptor("next-session", 2);
        let expected_next_binding = next_descriptor.binding.identity_sha256().unwrap();
        let transition = old
            .transition(
                root.path(),
                next_descriptor,
                DiagnosticPolicy {
                    enabled: false,
                    ..DiagnosticPolicy::default()
                },
                16,
            )
            .unwrap();
        assert!(transition.finished.manifest_sha256.is_some());
        assert_eq!(
            transition.binding_change_diagnostic,
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert_eq!(
            transition
                .next
                .finish(DiagnosticRunStatus::Success, 32)
                .completeness,
            None
        );

        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("old-session/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["facts"]["record_count"], 2);
        let facts = fs::read_to_string(root.path().join("old-session/facts.ndjson")).unwrap();
        let binding_fact: serde_json::Value =
            serde_json::from_str(facts.lines().nth(1).unwrap()).unwrap();
        assert_eq!(binding_fact["fact"]["operation"], "change_binding");
        assert_eq!(
            binding_fact["fact"]["detail"]["next_binding_sha256"],
            expected_next_binding
        );
    }

    #[test]
    fn unchanged_or_noncanonical_binding_cannot_rotate() {
        let root = tempfile::tempdir().unwrap();
        let session = RecognitionSession::start(
            root.path(),
            descriptor("same-binding", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        assert!(matches!(
            session.transition(
                root.path(),
                descriptor("different-run", 1),
                DiagnosticPolicy::default(),
                0,
            ),
            Err(RecognitionSessionError::BindingUnchanged)
        ));

        let mut invalid = descriptor("invalid-layout", 1);
        invalid.binding.canonical_layout_sha256 = "4".repeat(64);
        assert!(matches!(
            RecognitionSession::start(root.path(), invalid, DiagnosticPolicy::default()),
            Err(RecognitionSessionError::CanonicalLayoutMismatch)
        ));
    }
}
