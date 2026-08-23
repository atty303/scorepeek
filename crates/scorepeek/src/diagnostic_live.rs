use std::path::Path;
use std::sync::Arc;

use scorepeek::capture::NormalizedCanonicalFrame;

use crate::diagnostic_recording::{
    DiagnosticErrorType, DiagnosticFinishOutcome, DiagnosticPolicy, DiagnosticRunDescriptor,
    DiagnosticRunStatus,
};
use crate::diagnostic_worker::{
    DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT, DiagnosticEnqueueOutcome, DiagnosticOwnedFrame,
    DiagnosticWorkerHandle,
};

#[derive(Clone, Debug)]
pub struct LiveCanonicalFrame {
    capture_generation: u64,
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    capture_profile_sha256: String,
    normalizer_sha256: String,
    pixels: Arc<Box<[u8]>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveDiagnosticBridgeError {
    ReplayBindingNotAllowed,
}

impl LiveCanonicalFrame {
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub(crate) const fn sequence(&self) -> u64 {
        self.sequence
    }
}

impl From<NormalizedCanonicalFrame> for LiveCanonicalFrame {
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

pub struct LiveDiagnosticBridge {
    capture_generation: u64,
    capture_profile_sha256: String,
    normalizer_sha256: String,
    worker: DiagnosticWorkerHandle,
}

impl LiveDiagnosticBridge {
    /// Starts one application-owned diagnostic run for one immutable capture generation.
    ///
    /// # Errors
    /// Returns an error when a replay-bound descriptor is supplied to the live path.
    pub fn start(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Result<Self, LiveDiagnosticBridgeError> {
        if descriptor.binding.replay.is_some() {
            return Err(LiveDiagnosticBridgeError::ReplayBindingNotAllowed);
        }
        let worker = DiagnosticWorkerHandle::start(root, descriptor.clone(), policy);
        Ok(Self::with_worker(descriptor, worker))
    }

    /// Offers canonical evidence before recognition outcomes are known.
    ///
    /// The offer never waits for queue capacity or diagnostic I/O. A binding mismatch is recorded
    /// only as diagnostic degradation and cannot alter recognition or event results.
    pub fn offer(&mut self, frame: &LiveCanonicalFrame) -> DiagnosticEnqueueOutcome {
        if frame.capture_generation != self.capture_generation
            || frame.capture_profile_sha256 != self.capture_profile_sha256
            || frame.normalizer_sha256 != self.normalizer_sha256
        {
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
            worker,
        }
    }

    #[cfg(test)]
    fn start_for_test(
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
    fn start_with_supervisor_for_test(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_recording::{
        DiagnosticBinding, DiagnosticCompleteness, DiagnosticResource,
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
                canonical_layout_sha256: "4".repeat(64),
                catalog_sha256: "5".repeat(64),
                model_sha256: "6".repeat(64),
                runtime_sha256: "7".repeat(64),
                replay: None,
            },
        }
    }

    fn frame(generation: u64, sequence: u64, time: u64) -> LiveCanonicalFrame {
        LiveCanonicalFrame {
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

    #[test]
    fn offer_is_recognition_independent_and_reuses_owned_pixels() {
        let root = tempfile::tempdir().unwrap();
        let canonical = frame(1, 1, 0);
        let pixels = Arc::clone(&canonical.pixels);
        let mut bridge = LiveDiagnosticBridge::start_for_test(
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
    fn binding_change_is_rejected_and_makes_the_old_run_partial() {
        let root = tempfile::tempdir().unwrap();
        let mut bridge = LiveDiagnosticBridge::start_for_test(
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
    fn generation_rollover_creates_two_independent_runs() {
        let root = tempfile::tempdir().unwrap();
        let supervisor = std::sync::Mutex::new(std::sync::Weak::new());
        for generation in [1, 2] {
            let run_id = format!("generation-{generation}");
            let mut bridge = LiveDiagnosticBridge::start_with_supervisor_for_test(
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
        let mut bridge = LiveDiagnosticBridge::start_for_test(
            root.path(),
            descriptor("disabled-live", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
            2,
        );
        assert_eq!(
            bridge.offer(&frame(1, 1, 0)),
            DiagnosticEnqueueOutcome::Disabled
        );
        assert_eq!(
            bridge.finish(DiagnosticRunStatus::Success, 16).completeness,
            None
        );
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn worker_loss_is_diagnostic_only() {
        let root = tempfile::tempdir().unwrap();
        let mut bridge = LiveDiagnosticBridge::start_for_test(
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
