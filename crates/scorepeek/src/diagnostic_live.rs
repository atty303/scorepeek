use std::path::Path;
use std::sync::Arc;

use crate::diagnostic_recording::{
    CANONICAL_BYTES, DiagnosticErrorType, DiagnosticFinishOutcome, DiagnosticPolicy,
    DiagnosticRunDescriptor, DiagnosticRunStatus,
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
    pixels: Arc<[u8]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveCanonicalFrameError {
    InvalidContract,
}

impl LiveCanonicalFrame {
    /// Creates the application handoff emitted by a validated live normalizer.
    ///
    /// This boundary does not normalize an observed frame. It accepts only already-canonical RGB8
    /// pixels and immutable capture-generation/profile/normalizer evidence.
    ///
    /// # Errors
    /// Returns an error when geometry, timing, sequence, or binding identities are invalid.
    pub fn new(
        capture_generation: u64,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
        capture_profile_sha256: String,
        normalizer_sha256: String,
        pixels: Arc<[u8]>,
    ) -> Result<Self, LiveCanonicalFrameError> {
        if capture_generation == 0
            || sequence == 0
            || monotonic_end_ms < monotonic_start_ms
            || pixels.len() != CANONICAL_BYTES
            || !valid_sha256(&capture_profile_sha256)
            || !valid_sha256(&normalizer_sha256)
        {
            return Err(LiveCanonicalFrameError::InvalidContract);
        }
        Ok(Self {
            capture_generation,
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            capture_profile_sha256,
            normalizer_sha256,
            pixels,
        })
    }

    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
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
    ) -> Result<Self, LiveCanonicalFrameError> {
        if descriptor.binding.replay.is_some() {
            return Err(LiveCanonicalFrameError::InvalidContract);
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

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
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
        LiveCanonicalFrame::new(
            generation,
            sequence,
            time,
            time + 16,
            "2".repeat(64),
            "3".repeat(64),
            Arc::from(vec![7; CANONICAL_BYTES]),
        )
        .unwrap()
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
