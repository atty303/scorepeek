use std::path::Path;

use scorepeek::recognition::{
    CanonicalLayout, RecognitionError, ScreenClass, ScreenPredicateObservation,
    inspect_canonical_rgb8,
};

use crate::diagnostic_live::{LiveCanonicalFrame, LiveDiagnosticBridge, LiveDiagnosticBridgeError};
use crate::diagnostic_recording::{
    DiagnosticFinishOutcome, DiagnosticPolicy, DiagnosticRunDescriptor, DiagnosticRunStatus,
};
use crate::diagnostic_worker::DiagnosticEnqueueOutcome;

/// One screen-predicate result that borrows its immutable live capture evidence.
///
/// The result cannot outlive or detach from the profile- and generation-bearing frame that was
/// inspected. It carries no accepted field or event authority.
#[derive(Debug)]
pub struct LiveRecognitionObservation<'a> {
    frame: &'a LiveCanonicalFrame,
    canonical_layout_sha256: String,
    predicate: ScreenPredicateObservation,
}

impl<'a> LiveRecognitionObservation<'a> {
    /// Applies the embedded screen predicate to one admitted live canonical owner.
    ///
    /// # Errors
    /// Returns an error when the fixed canonical pixel or embedded layout contract is invalid.
    pub fn inspect(frame: &'a LiveCanonicalFrame) -> Result<Self, RecognitionError> {
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
    pub(crate) const fn frame(&self) -> &LiveCanonicalFrame {
        self.frame
    }

    #[must_use]
    pub(crate) fn canonical_layout_sha256(&self) -> &str {
        &self.canonical_layout_sha256
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveRecognitionSessionError {
    InvalidBinding,
    ReplayBindingNotAllowed,
    CanonicalLayoutMismatch,
    BindingUnchanged,
    FrameBindingMismatch,
    RecognitionFailed,
}

impl From<LiveDiagnosticBridgeError> for LiveRecognitionSessionError {
    fn from(error: LiveDiagnosticBridgeError) -> Self {
        match error {
            LiveDiagnosticBridgeError::ReplayBindingNotAllowed => Self::ReplayBindingNotAllowed,
        }
    }
}

/// Recognition and diagnostic outcomes for one frame under one immutable live binding.
///
/// Diagnostic queue state is reported separately and never changes the recognition observation.
#[derive(Debug)]
pub struct LiveRecognitionFrameResult<'a> {
    pub observation: LiveRecognitionObservation<'a>,
    pub diagnostic_frame: DiagnosticEnqueueOutcome,
    pub diagnostic_fact: DiagnosticEnqueueOutcome,
}

pub struct LiveRecognitionTransition {
    pub finished: DiagnosticFinishOutcome,
    pub binding_change_diagnostic: DiagnosticEnqueueOutcome,
    pub next: LiveRecognitionSession,
}

/// Application-owned recognition lifetime for one immutable diagnostic binding.
///
/// This is a resource boundary, not an inferred game session. A different capture generation or
/// recognition input is rejected; `transition` records the explicit change, finishes the old run,
/// and only then starts the replacement session.
pub struct LiveRecognitionSession {
    binding_sha256: String,
    bridge: LiveDiagnosticBridge,
    last_sequence: Option<u64>,
}

impl LiveRecognitionSession {
    /// Starts a live session only for the embedded canonical layout and a non-replay binding.
    ///
    /// # Errors
    /// Returns a typed error for an invalid, replay-bound, or noncanonical descriptor.
    pub fn start(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Result<Self, LiveRecognitionSessionError> {
        let binding_sha256 = validate_live_descriptor(&descriptor)?;
        let bridge = LiveDiagnosticBridge::start(root, descriptor, policy)?;
        Ok(Self {
            binding_sha256,
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
        frame: &'a LiveCanonicalFrame,
    ) -> Result<LiveRecognitionFrameResult<'a>, LiveRecognitionSessionError> {
        if !self.bridge.matches_frame(frame) {
            let _ = self.bridge.offer(frame);
            return Err(LiveRecognitionSessionError::FrameBindingMismatch);
        }
        let diagnostic_frame = self.bridge.offer(frame);
        self.last_sequence = Some(frame.sequence());
        let Ok(observation) = LiveRecognitionObservation::inspect(frame) else {
            let _ = self.bridge.record_recognition_failure(frame);
            return Err(LiveRecognitionSessionError::RecognitionFailed);
        };
        let diagnostic_fact = self.bridge.record_screen_observation(&observation);
        Ok(LiveRecognitionFrameResult {
            observation,
            diagnostic_frame,
            diagnostic_fact,
        })
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
    ) -> Result<LiveRecognitionTransition, LiveRecognitionSessionError> {
        let next_binding_sha256 = validate_live_descriptor(&next_descriptor)?;
        if next_binding_sha256 == self.binding_sha256 {
            return Err(LiveRecognitionSessionError::BindingUnchanged);
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
        Ok(LiveRecognitionTransition {
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
    ) -> Result<Self, LiveRecognitionSessionError> {
        let binding_sha256 = validate_live_descriptor(&descriptor)?;
        let bridge = LiveDiagnosticBridge::start_with_supervisor_for_test(
            root, descriptor, policy, supervisor,
        );
        Ok(Self {
            binding_sha256,
            bridge,
            last_sequence: None,
        })
    }
}

fn validate_live_descriptor(
    descriptor: &DiagnosticRunDescriptor,
) -> Result<String, LiveRecognitionSessionError> {
    let binding = &descriptor.binding;
    if binding.replay.is_some() {
        return Err(LiveRecognitionSessionError::ReplayBindingNotAllowed);
    }
    if binding.canonical_layout_sha256 != CanonicalLayout::sha256() {
        return Err(LiveRecognitionSessionError::CanonicalLayoutMismatch);
    }
    if !descriptor.is_valid() {
        return Err(LiveRecognitionSessionError::InvalidBinding);
    }
    binding
        .identity_sha256()
        .ok_or(LiveRecognitionSessionError::InvalidBinding)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use scorepeek::recognition::{CanonicalLayout, ScreenClass};

    use super::*;
    use crate::diagnostic_recording::{
        DiagnosticBinding, DiagnosticCompleteness, DiagnosticResource,
    };

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

    #[test]
    fn diagnostic_opt_out_does_not_change_recognition() {
        let root = tempfile::tempdir().unwrap();
        let frame = LiveCanonicalFrame::for_test(1, 1, 0);
        let mut session = LiveRecognitionSession::start(
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
    fn mismatched_generation_stops_before_recognition() {
        let root = tempfile::tempdir().unwrap();
        let mut session = LiveRecognitionSession::start(
            root.path(),
            descriptor("mismatched-session", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        assert!(matches!(
            session.inspect(&LiveCanonicalFrame::for_test(2, 1, 0)),
            Err(LiveRecognitionSessionError::FrameBindingMismatch)
        ));
    }

    #[test]
    fn diagnostic_sequence_rejection_does_not_change_recognition() {
        let root = tempfile::tempdir().unwrap();
        let supervisor = std::sync::Mutex::new(std::sync::Weak::new());
        let frame = LiveCanonicalFrame::for_test(1, 1, 0);
        let mut session = LiveRecognitionSession::start_with_supervisor_for_test(
            root.path(),
            descriptor("sequence-rejection", 1),
            DiagnosticPolicy::default(),
            &supervisor,
        )
        .unwrap();
        let first = session.inspect(&frame).unwrap();
        assert_eq!(first.observation.screen(), ScreenClass::Unknown);
        assert_eq!(first.diagnostic_frame, DiagnosticEnqueueOutcome::Enqueued);
        let rejected = session.inspect(&frame).unwrap();
        assert_eq!(rejected.observation.screen(), ScreenClass::Unknown);
        assert_eq!(
            rejected.diagnostic_frame,
            DiagnosticEnqueueOutcome::Rejected
        );
        assert_eq!(
            session
                .finish(DiagnosticRunStatus::Success, 16)
                .completeness,
            Some(DiagnosticCompleteness::Partial)
        );
    }

    #[test]
    fn transition_records_next_binding_and_finishes_old_run_first() {
        let root = tempfile::tempdir().unwrap();
        let supervisor = std::sync::Mutex::new(std::sync::Weak::new());
        let old_descriptor = descriptor("old-session", 1);
        let mut old = LiveRecognitionSession::start_with_supervisor_for_test(
            root.path(),
            old_descriptor,
            DiagnosticPolicy::default(),
            &supervisor,
        )
        .unwrap();
        old.inspect(&LiveCanonicalFrame::for_test(1, 1, 0)).unwrap();
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
        let facts = manifest["facts"].as_array().unwrap();
        assert_eq!(facts.len(), 2);
        let binding_fact: serde_json::Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("old-session")
                    .join(facts[1]["filename"].as_str().unwrap()),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(binding_fact["fact"]["operation"], "change_binding");
        assert_eq!(
            binding_fact["fact"]["detail"]["next_binding_sha256"],
            expected_next_binding
        );
    }

    #[test]
    fn unchanged_or_noncanonical_binding_cannot_rotate() {
        let root = tempfile::tempdir().unwrap();
        let session = LiveRecognitionSession::start(
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
            Err(LiveRecognitionSessionError::BindingUnchanged)
        ));

        let mut invalid = descriptor("invalid-layout", 1);
        invalid.binding.canonical_layout_sha256 = "4".repeat(64);
        assert!(matches!(
            LiveRecognitionSession::start(root.path(), invalid, DiagnosticPolicy::default()),
            Err(LiveRecognitionSessionError::CanonicalLayoutMismatch)
        ));
    }
}
