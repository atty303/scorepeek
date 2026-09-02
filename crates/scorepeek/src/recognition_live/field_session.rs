use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use scorepeek::recognition::ScreenFieldObservationError;
use scorepeek::recognition::{NUMERIC_MODEL_MANIFEST_BYTES, NUMERIC_MODEL_MANIFEST_SHA256};

use super::DiagnosticScreenFieldObservation;
use super::field_observer::{
    BoundFieldObservation, FieldObservationPoll, FieldObserver, FieldObserverFinishOutcome,
    FieldObserverOfferError, FieldObserverStartError, FieldObserverWorker, PendingFieldObservation,
};
use super::screen_field_observer::{
    RegisteredScreenFieldObserver, RegisteredScreenFieldObserverLoadError,
    SharedRegisteredScreenFieldResources,
};
use super::text_observer_pool::RecognitionExecutionMode;
use super::{
    FieldInputPolicy, PreparedRecognitionFrame, RecognitionFrameResult, RecognitionObservation,
    RecognitionSession, RecognitionSessionError,
};
use crate::diagnostic_live::BoundCanonicalFrame;
use crate::diagnostic_recording::{
    DiagnosticFinishOutcome, DiagnosticPolicy, DiagnosticRunDescriptor, DiagnosticRunStatus,
};
use crate::diagnostic_worker::DiagnosticEnqueueOutcome;

#[derive(Debug)]
pub enum FieldObservationStartError<E> {
    FieldObserver(FieldObserverStartError<E>),
    Recognition {
        error: RecognitionSessionError,
        field_observer_finish: FieldObserverFinishOutcome,
    },
}

#[derive(Debug)]
pub enum FieldObservationSubmission<T> {
    NotApplicable,
    BusySkipped,
    Submitted(PendingSessionFieldObservation<T>),
    Rejected(FieldObserverOfferError),
}

#[derive(Debug)]
pub struct PendingSessionFieldObservation<T> {
    pending: PendingFieldObservation<T>,
    owner: Arc<()>,
    identity: Arc<()>,
    timing: super::FrameProcessingTiming,
    screen_episode_id: u64,
}

impl<T> PendingSessionFieldObservation<T> {
    pub fn bind_screen_episode(&mut self, screen_episode_id: u64) {
        self.screen_episode_id = screen_episode_id;
    }

    pub fn add_live_processing(&mut self, timing: super::LiveEventProcessingTiming) {
        self.timing.add_live_processing(timing);
    }
}

#[derive(Debug)]
pub struct FieldObservationFrameResult<'a, T> {
    pub observation: RecognitionObservation<'a>,
    pub field_submission: FieldObservationSubmission<T>,
    pub diagnostic_frame: DiagnosticEnqueueOutcome,
    pub diagnostic_screen_fact: DiagnosticEnqueueOutcome,
    pub timing: super::FrameProcessingTiming,
}

#[derive(Debug)]
pub enum FieldObservationSessionPoll<T> {
    Pending,
    Ready {
        observation: BoundFieldObservation<T>,
        diagnostic_field_fact: DiagnosticEnqueueOutcome,
        timing: super::FrameProcessingTiming,
        screen_episode_id: u64,
    },
    Consumed,
    BindingMismatch,
    Terminal,
    WorkerUnavailable,
}

#[derive(Debug, Eq, PartialEq)]
pub struct FieldObservationFinishOutcome {
    pub field_observer: FieldObserverFinishOutcome,
    pub diagnostic: DiagnosticFinishOutcome,
}

/// One application owner for a live recognition run and its exact field observer.
pub struct FieldObservationSession<O: FieldObserver> {
    recognition: RecognitionSession,
    field_observer: FieldObserverWorker<O>,
    owner: Arc<()>,
    outstanding: Vec<(Arc<()>, u64)>,
}

impl<O: FieldObserver> FieldObservationSession<O> {
    pub(crate) fn record_sampling_summary(
        &mut self,
        sequence: u64,
        monotonic_ms: u64,
        summary: crate::diagnostic_recording::RecognitionSamplingSummary,
    ) {
        self.recognition
            .record_sampling_summary(sequence, monotonic_ms, summary);
    }

    /// Loads the observer before opening the matching diagnostic-backed recognition run.
    ///
    /// # Errors
    /// Returns the field-observer start error, or the recognition start error together with the
    /// bounded observer cleanup outcome.
    pub fn start<E>(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        loader: impl FnOnce(&super::field_observer::FieldObserverSessionBinding) -> Result<O, E>,
    ) -> Result<Self, FieldObservationStartError<E>> {
        let field_observer = FieldObserverWorker::start(&descriptor, loader)
            .map_err(FieldObservationStartError::FieldObserver)?;
        let recognition = match RecognitionSession::start(root, descriptor, policy) {
            Ok(recognition) => recognition,
            Err(error) => {
                return Err(FieldObservationStartError::Recognition {
                    error,
                    field_observer_finish: field_observer
                        .finish(super::field_observer::DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT),
                });
            }
        };
        Ok(Self {
            recognition,
            field_observer,
            owner: Arc::new(()),
            outstanding: Vec::new(),
        })
    }

    fn start_with_capacity<E>(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        capacity: usize,
        loader: impl FnOnce(&super::field_observer::FieldObserverSessionBinding) -> Result<O, E>,
    ) -> Result<Self, FieldObservationStartError<E>> {
        let field_observer =
            FieldObserverWorker::start_with_capacity(&descriptor, loader, capacity)
                .map_err(FieldObservationStartError::FieldObserver)?;
        let recognition = match RecognitionSession::start(root, descriptor, policy) {
            Ok(recognition) => recognition,
            Err(error) => {
                return Err(FieldObservationStartError::Recognition {
                    error,
                    field_observer_finish: field_observer
                        .finish(super::field_observer::DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT),
                });
            }
        };
        Ok(Self {
            recognition,
            field_observer,
            owner: Arc::new(()),
            outstanding: Vec::new(),
        })
    }

    fn start_unmanaged_with_capacity<E>(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        capacity: usize,
        loader: impl FnOnce(&super::field_observer::FieldObserverSessionBinding) -> Result<O, E>,
    ) -> Result<Self, FieldObservationStartError<E>> {
        let field_observer =
            FieldObserverWorker::start_unmanaged_with_capacity(&descriptor, loader, capacity)
                .map_err(FieldObservationStartError::FieldObserver)?;
        let recognition = match RecognitionSession::start(root, descriptor, policy) {
            Ok(recognition) => recognition,
            Err(error) => {
                return Err(FieldObservationStartError::Recognition {
                    error,
                    field_observer_finish: field_observer
                        .finish(super::field_observer::DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT),
                });
            }
        };
        Ok(Self {
            recognition,
            field_observer,
            owner: Arc::new(()),
            outstanding: Vec::new(),
        })
    }

    /// Inspects and non-blockingly submits one current-run complete crop set when applicable.
    ///
    /// Field-worker rejection is returned separately and cannot replace the screen observation.
    ///
    /// # Errors
    /// Returns the recognition-session error before field submission.
    pub fn inspect<'a>(
        &mut self,
        frame: &'a BoundCanonicalFrame,
    ) -> Result<FieldObservationFrameResult<'a, O::Output>, RecognitionSessionError> {
        self.inspect_with_field_policy(frame, FieldInputPolicy::Route)
    }

    /// Submits one source-ordered frame whose pure classification and crops were prepared by a
    /// shared replay worker.
    ///
    /// # Errors
    /// Returns the recognition-session error before field submission.
    pub fn inspect_prepared<'a>(
        &mut self,
        frame: &'a BoundCanonicalFrame,
        prepared: PreparedRecognitionFrame,
    ) -> Result<FieldObservationFrameResult<'a, O::Output>, RecognitionSessionError> {
        let RecognitionFrameResult {
            observation,
            field_inputs,
            diagnostic_frame,
            diagnostic_fact,
            timing,
        } = self.recognition.inspect_prepared(frame, prepared)?;
        let field_submission = match field_inputs {
            None => FieldObservationSubmission::NotApplicable,
            Some(inputs) => match self.field_observer.try_observe(inputs) {
                Ok(pending) => {
                    let identity = Arc::new(());
                    self.outstanding
                        .push((Arc::clone(&identity), frame.sequence()));
                    FieldObservationSubmission::Submitted(PendingSessionFieldObservation {
                        pending,
                        owner: Arc::clone(&self.owner),
                        identity,
                        timing,
                        screen_episode_id: 0,
                    })
                }
                Err(error) => {
                    self.recognition
                        .record_field_observer_offer_failure(frame.sequence(), error);
                    FieldObservationSubmission::Rejected(error)
                }
            },
        };
        Ok(FieldObservationFrameResult {
            observation,
            field_submission,
            diagnostic_frame,
            diagnostic_screen_fact: diagnostic_fact,
            timing,
        })
    }

    pub(crate) fn inspect_while_field_busy<'a>(
        &mut self,
        frame: &'a BoundCanonicalFrame,
    ) -> Result<FieldObservationFrameResult<'a, O::Output>, RecognitionSessionError> {
        self.inspect_with_field_policy(frame, FieldInputPolicy::SkipBusy)
    }

    fn inspect_with_field_policy<'a>(
        &mut self,
        frame: &'a BoundCanonicalFrame,
        field_policy: FieldInputPolicy,
    ) -> Result<FieldObservationFrameResult<'a, O::Output>, RecognitionSessionError> {
        let RecognitionFrameResult {
            observation,
            field_inputs,
            diagnostic_frame,
            diagnostic_fact,
            timing,
        } = self
            .recognition
            .inspect_with_field_policy(frame, field_policy)?;
        let field_submission = match (field_policy, field_inputs) {
            (FieldInputPolicy::SkipBusy, None)
                if matches!(
                    observation.screen(),
                    scorepeek::recognition::ScreenClass::Result
                        | scorepeek::recognition::ScreenClass::MusicSelect
                ) =>
            {
                let _ = self
                    .recognition
                    .record_field_observation_busy_skip(frame, observation.screen());
                FieldObservationSubmission::BusySkipped
            }
            (_, None) => FieldObservationSubmission::NotApplicable,
            (_, Some(inputs)) => match self.field_observer.try_observe(inputs) {
                Ok(pending) => {
                    let identity = Arc::new(());
                    self.outstanding
                        .push((Arc::clone(&identity), frame.sequence()));
                    FieldObservationSubmission::Submitted(PendingSessionFieldObservation {
                        pending,
                        owner: Arc::clone(&self.owner),
                        identity,
                        timing,
                        screen_episode_id: 0,
                    })
                }
                Err(error) => {
                    self.recognition
                        .record_field_observer_offer_failure(frame.sequence(), error);
                    FieldObservationSubmission::Rejected(error)
                }
            },
        };
        Ok(FieldObservationFrameResult {
            observation,
            field_submission,
            diagnostic_frame,
            diagnostic_screen_fact: diagnostic_fact,
            timing,
        })
    }

    #[must_use]
    pub fn finish(
        mut self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
        field_observer_timeout: Duration,
    ) -> FieldObservationFinishOutcome {
        let field_observer = self.field_observer.finish(field_observer_timeout);
        for (_, sequence) in self.outstanding {
            self.recognition
                .record_abandoned_field_observation(sequence);
        }
        self.recognition
            .record_field_observer_finish(field_observer);
        let diagnostic = self.recognition.finish(status, monotonic_end_ms);
        FieldObservationFinishOutcome {
            field_observer,
            diagnostic,
        }
    }

    /// Offline failure teardown retains the session until its admitted field worker has exited.
    #[must_use]
    pub fn finish_offline(
        mut self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
    ) -> FieldObservationFinishOutcome {
        let field_observer = self.field_observer.finish_joining();
        for (_, sequence) in self.outstanding {
            self.recognition
                .record_abandoned_field_observation(sequence);
        }
        self.recognition
            .record_field_observer_finish(field_observer);
        let diagnostic = self.recognition.finish(status, monotonic_end_ms);
        FieldObservationFinishOutcome {
            field_observer,
            diagnostic,
        }
    }

    /// Finishes after capture teardown while extending the shared monotonic run bound through the
    /// field-worker shutdown performed by this call.
    #[must_use]
    pub fn finish_after_capture(
        mut self,
        status: DiagnosticRunStatus,
        capture_end_ms: u64,
        elapsed_after_capture: Duration,
        field_observer_timeout: Duration,
    ) -> FieldObservationFinishOutcome {
        let finish_started = Instant::now();
        let field_observer = self.field_observer.finish(field_observer_timeout);
        for (_, sequence) in self.outstanding {
            self.recognition
                .record_abandoned_field_observation(sequence);
        }
        self.recognition
            .record_field_observer_finish(field_observer);
        let elapsed_ms = duration_millis_saturating(
            elapsed_after_capture.saturating_add(finish_started.elapsed()),
        );
        let diagnostic_status = if field_observer.status
            == super::field_observer::FieldObserverFinishStatus::Complete
        {
            status
        } else {
            DiagnosticRunStatus::Error
        };
        let diagnostic = self
            .recognition
            .finish(diagnostic_status, capture_end_ms.saturating_add(elapsed_ms));
        FieldObservationFinishOutcome {
            field_observer,
            diagnostic,
        }
    }

    #[cfg(test)]
    fn start_for_test<E>(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        loader: impl FnOnce(&super::field_observer::FieldObserverSessionBinding) -> Result<O, E>,
        capacity: usize,
    ) -> Result<Self, FieldObservationStartError<E>> {
        let field_observer = FieldObserverWorker::start_for_test(&descriptor, loader, capacity)
            .map_err(FieldObservationStartError::FieldObserver)?;
        let supervisor = std::sync::Mutex::new(std::sync::Weak::new());
        let recognition = match RecognitionSession::start_with_supervisor_for_test(
            root,
            descriptor,
            policy,
            &supervisor,
        ) {
            Ok(recognition) => recognition,
            Err(error) => {
                return Err(FieldObservationStartError::Recognition {
                    error,
                    field_observer_finish: field_observer
                        .finish(super::field_observer::DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT),
                });
            }
        };
        Ok(Self {
            recognition,
            field_observer,
            owner: Arc::new(()),
            outstanding: Vec::new(),
        })
    }
}

fn duration_millis_saturating(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

impl FieldObservationSession<RegisteredScreenFieldObserver> {
    /// Starts the production session with the exact registered catalog, model, and runtime.
    ///
    /// # Errors
    /// Returns a typed descriptor, resource-loading, worker, or recognition-session error.
    pub fn start_registered(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        catalog_root: &Path,
        bundle_root: &Path,
        execution_mode: RecognitionExecutionMode,
    ) -> Result<Self, FieldObservationStartError<RegisteredScreenFieldObserverLoadError>> {
        let available_parallelism = std::thread::available_parallelism().map_or(1, usize::from);
        let workers = super::text_observer_pool::select_text_worker_count(
            execution_mode,
            available_parallelism,
        );
        let capacity = match execution_mode {
            RecognitionExecutionMode::Live => 2,
            RecognitionExecutionMode::Offline => workers.saturating_mul(2),
        };
        Self::start_with_capacity(root, descriptor, policy, capacity, |binding| {
            let resources = binding.load_registered_resources(catalog_root, bundle_root)?;
            let numeric_runtime = scorepeek::numeric_model_store::active_registered(
                NUMERIC_MODEL_MANIFEST_BYTES,
                NUMERIC_MODEL_MANIFEST_SHA256,
            )?;
            RegisteredScreenFieldObserver::new(resources, numeric_runtime, execution_mode)
        })
    }

    /// Starts one offline session using the corpus-wide registered text pool.
    ///
    /// # Errors
    /// Returns a typed descriptor, numeric runtime, shared binding, worker, or recognition error.
    pub fn start_registered_shared(
        root: &Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        shared: Arc<SharedRegisteredScreenFieldResources>,
    ) -> Result<Self, FieldObservationStartError<RegisteredScreenFieldObserverLoadError>> {
        let capacity = shared.text_workers().saturating_mul(2);
        Self::start_unmanaged_with_capacity(root, descriptor, policy, capacity, move |binding| {
            let numeric_runtime = scorepeek::numeric_model_store::active_registered(
                NUMERIC_MODEL_MANIFEST_BYTES,
                NUMERIC_MODEL_MANIFEST_SHA256,
            )?;
            shared.observer(binding, numeric_runtime)
        })
    }
}

impl<O: FieldObserver> FieldObservationSession<O> {
    #[must_use]
    pub fn poll_field_observation<T, E>(
        &mut self,
        pending: &PendingSessionFieldObservation<O::Output>,
    ) -> FieldObservationSessionPoll<O::Output>
    where
        O: FieldObserver<Output = Result<T, ScreenFieldObservationError<E>>>,
        T: DiagnosticScreenFieldObservation + Send + 'static,
        E: Send + 'static,
    {
        self.poll_owned_field_observation(pending, None)
    }

    /// Waits only for the caller-selected bound and records a completed value-free fact.
    #[must_use]
    pub fn wait_field_observation<T, E>(
        &mut self,
        pending: &PendingSessionFieldObservation<O::Output>,
        timeout: Duration,
    ) -> FieldObservationSessionPoll<O::Output>
    where
        O: FieldObserver<Output = Result<T, ScreenFieldObservationError<E>>>,
        T: DiagnosticScreenFieldObservation + Send + 'static,
        E: Send + 'static,
    {
        self.poll_owned_field_observation(pending, Some(timeout))
    }

    pub fn record_recognition_busy_skip(
        &mut self,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
    ) -> DiagnosticEnqueueOutcome {
        self.recognition.record_recognition_busy_skip(
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
        )
    }

    pub fn record_frame_processing_timing(
        &mut self,
        mut timing: super::FrameProcessingTiming,
        field_status: crate::diagnostic_recording::FrameFieldStatus,
        field_timing: Option<&super::screen_field_observer::RecognitionProcessingTiming>,
    ) -> DiagnosticEnqueueOutcome {
        timing.finish_wall();
        self.recognition
            .record_frame_processing_timing(timing, field_status, field_timing)
    }

    fn poll_owned_field_observation<T, E>(
        &mut self,
        pending: &PendingSessionFieldObservation<O::Output>,
        timeout: Option<Duration>,
    ) -> FieldObservationSessionPoll<O::Output>
    where
        O: FieldObserver<Output = Result<T, ScreenFieldObservationError<E>>>,
        T: DiagnosticScreenFieldObservation + Send + 'static,
        E: Send + 'static,
    {
        if !Arc::ptr_eq(&pending.owner, &self.owner) {
            self.recognition.reject_pending_field_observation();
            return FieldObservationSessionPoll::BindingMismatch;
        }
        let Some(index) = self
            .outstanding
            .iter()
            .position(|(identity, _)| Arc::ptr_eq(identity, &pending.identity))
        else {
            return match pending.pending.poll() {
                FieldObservationPoll::Consumed => FieldObservationSessionPoll::Consumed,
                FieldObservationPoll::Terminal => FieldObservationSessionPoll::Terminal,
                FieldObservationPoll::Pending
                | FieldObservationPoll::Ready(_)
                | FieldObservationPoll::WorkerUnavailable => {
                    FieldObservationSessionPoll::BindingMismatch
                }
            };
        };
        let sequence = self.outstanding[index].1;
        let poll = timeout.map_or_else(
            || pending.pending.poll(),
            |timeout| pending.pending.wait(timeout),
        );
        match poll {
            FieldObservationPoll::Pending => FieldObservationSessionPoll::Pending,
            FieldObservationPoll::Ready(observation) => {
                self.outstanding.swap_remove(index);
                let diagnostic_field_fact = self.recognition.record_field_observation(&observation);
                let mut timing = pending.timing;
                timing.finish_wall();
                FieldObservationSessionPoll::Ready {
                    observation,
                    diagnostic_field_fact,
                    timing,
                    screen_episode_id: pending.screen_episode_id,
                }
            }
            FieldObservationPoll::Consumed => {
                self.outstanding.swap_remove(index);
                FieldObservationSessionPoll::Consumed
            }
            FieldObservationPoll::Terminal => {
                self.outstanding.swap_remove(index);
                FieldObservationSessionPoll::Terminal
            }
            FieldObservationPoll::WorkerUnavailable => {
                self.outstanding.swap_remove(index);
                self.recognition.record_field_observer_unavailable(sequence);
                FieldObservationSessionPoll::WorkerUnavailable
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use scorepeek::recognition::{
        CanonicalLayout, DynamicTextObservation, ScreenClass, ScreenFieldObservationError,
        ScreenFieldObservations, observe_screen_fields,
    };

    use super::*;
    use crate::diagnostic_recording::{
        DiagnosticBinding, DiagnosticCompleteness, DiagnosticResource,
    };
    use crate::recognition_live::field_observer::{FieldObserverFinishStatus, FieldObserverInput};

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

    fn solid_frame(color: [u8; 3], generation: u64, sequence: u64) -> BoundCanonicalFrame {
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
        }
        BoundCanonicalFrame::for_test_pixels(
            generation,
            sequence,
            sequence * 20,
            pixels.into_boxed_slice(),
        )
    }

    struct CompleteObserver;

    impl FieldObserver for CompleteObserver {
        type Output = Result<ScreenFieldObservations, ScreenFieldObservationError<&'static str>>;

        fn observe(&mut self, input: &FieldObserverInput) -> Self::Output {
            observe_screen_fields(input.crops(), |_, crop| {
                Ok(DynamicTextObservation {
                    input_width: crop.roi.width as usize,
                    output_timesteps: 1,
                    open_text: "imperfect observation".to_owned(),
                    constrained_text: None,
                })
            })
        }
    }

    struct PanickingObserver;

    impl FieldObserver for PanickingObserver {
        type Output = Result<ScreenFieldObservations, ScreenFieldObservationError<&'static str>>;

        fn observe(&mut self, _input: &FieldObserverInput) -> Self::Output {
            panic!("observer failed");
        }
    }

    #[test]
    fn integrated_session_submits_and_records_one_current_run_complete_output() {
        let root = tempfile::tempdir().unwrap();
        let mut session = FieldObservationSession::start_for_test(
            root.path(),
            descriptor("integrated-field", 1),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(CompleteObserver),
            2,
        )
        .unwrap();
        let frame = solid_frame([200, 100, 20], 1, 1);
        let result = session.inspect(&frame).unwrap();
        assert_eq!(result.observation.screen(), ScreenClass::Result);
        let FieldObservationSubmission::Submitted(pending) = result.field_submission else {
            panic!("result screen did not submit complete field inputs");
        };
        let FieldObservationSessionPoll::Ready {
            observation,
            diagnostic_field_fact,
            ..
        } = session.wait_field_observation(&pending, Duration::from_secs(1))
        else {
            panic!("field observation did not complete");
        };
        assert_eq!(observation.sequence(), 1);
        assert_eq!(observation.screen(), ScreenClass::Result);
        assert_eq!(
            observation.output().as_ref().unwrap().screen(),
            ScreenClass::Result
        );
        assert_eq!(diagnostic_field_fact, DiagnosticEnqueueOutcome::Enqueued);
        assert!(matches!(
            session.poll_field_observation(&pending),
            FieldObservationSessionPoll::Consumed
        ));

        let finished = session.finish(DiagnosticRunStatus::Success, 40, Duration::from_secs(1));
        assert_eq!(
            finished.field_observer.status,
            FieldObserverFinishStatus::Complete
        );
        assert_eq!(finished.field_observer.abandoned, Some(0));
        assert_eq!(
            finished.diagnostic.completeness,
            Some(DiagnosticCompleteness::Complete)
        );
    }

    #[test]
    fn busy_field_policy_keeps_screen_classification_and_skips_only_field_submission() {
        let root = tempfile::tempdir().unwrap();
        let mut session = FieldObservationSession::start_for_test(
            root.path(),
            descriptor("busy-field-screen-tick", 1),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(CompleteObserver),
            1,
        )
        .unwrap();

        let result_frame = solid_frame([200, 100, 20], 1, 1);
        let result = session.inspect_while_field_busy(&result_frame).unwrap();
        assert_eq!(result.observation.screen(), ScreenClass::Result);
        assert!(matches!(
            result.field_submission,
            FieldObservationSubmission::BusySkipped
        ));

        let unknown_frame = solid_frame([0, 0, 0], 1, 2);
        let unknown = session.inspect_while_field_busy(&unknown_frame).unwrap();
        assert_eq!(unknown.observation.screen(), ScreenClass::Unknown);
        assert!(matches!(
            unknown.field_submission,
            FieldObservationSubmission::NotApplicable
        ));

        let finished = session.finish(DiagnosticRunStatus::Success, 60, Duration::from_secs(1));
        assert_eq!(finished.field_observer.submitted, 0);
    }

    #[test]
    fn integrated_opt_out_keeps_the_same_complete_field_output_without_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let mut session = FieldObservationSession::start_for_test(
            root.path(),
            descriptor("integrated-field-disabled", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
            |_| Ok::<_, ()>(CompleteObserver),
            2,
        )
        .unwrap();
        let frame = solid_frame([200, 100, 20], 1, 1);
        let result = session.inspect(&frame).unwrap();
        let FieldObservationSubmission::Submitted(pending) = result.field_submission else {
            panic!("result screen did not submit complete field inputs");
        };
        let FieldObservationSessionPoll::Ready {
            observation,
            diagnostic_field_fact,
            ..
        } = session.wait_field_observation(&pending, Duration::from_secs(1))
        else {
            panic!("field observation did not complete");
        };
        assert_eq!(
            observation.output().as_ref().unwrap().screen(),
            ScreenClass::Result
        );
        assert_eq!(diagnostic_field_fact, DiagnosticEnqueueOutcome::Disabled);

        let finished = session.finish(DiagnosticRunStatus::Success, 40, Duration::from_secs(1));
        assert_eq!(
            finished.field_observer.status,
            FieldObserverFinishStatus::Complete
        );
        assert_eq!(finished.diagnostic.completeness, None);
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }

    #[test]
    fn field_capacity_loss_is_diagnostic_only_and_makes_the_run_partial() {
        let root = tempfile::tempdir().unwrap();
        let mut session = FieldObservationSession::start_for_test(
            root.path(),
            descriptor("integrated-field-capacity", 1),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(CompleteObserver),
            1,
        )
        .unwrap();
        let first = solid_frame([200, 100, 20], 1, 1);
        let first_result = session.inspect(&first).unwrap();
        let FieldObservationSubmission::Submitted(pending) = first_result.field_submission else {
            panic!("first result was not submitted");
        };
        let second = solid_frame([200, 100, 20], 1, 2);
        let second_result = session.inspect(&second).unwrap();
        assert_eq!(second_result.observation.screen(), ScreenClass::Result);
        assert!(matches!(
            second_result.field_submission,
            FieldObservationSubmission::Rejected(FieldObserverOfferError::OutstandingLimit)
        ));

        let finished = session.finish(DiagnosticRunStatus::Success, 60, Duration::from_secs(1));
        assert_eq!(
            finished.field_observer.status,
            FieldObserverFinishStatus::Complete
        );
        assert_eq!(finished.field_observer.abandoned, Some(1));
        assert_eq!(
            finished.diagnostic.completeness,
            Some(DiagnosticCompleteness::Partial)
        );
        drop(pending);

        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("integrated-field-capacity/manifest.json")).unwrap(),
        )
        .unwrap();
        let reasons: Vec<_> = manifest["degradations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["reason"].as_str().unwrap())
            .collect();
        assert!(reasons.contains(&"field_observer_outstanding_limit"));
        assert!(reasons.contains(&"field_observation_abandoned"));
        let outstanding = manifest["degradations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["reason"] == "field_observer_outstanding_limit")
            .unwrap();
        let abandoned = manifest["degradations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["reason"] == "field_observation_abandoned")
            .unwrap();
        assert_eq!(outstanding["affected_sequence"], 2);
        assert_eq!(abandoned["affected_sequence"], 1);
    }

    #[test]
    fn another_run_rejects_a_pending_before_consuming_its_output() {
        let first_root = tempfile::tempdir().unwrap();
        let mut first_session = FieldObservationSession::start_for_test(
            first_root.path(),
            descriptor("integrated-field-first", 1),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(CompleteObserver),
            1,
        )
        .unwrap();
        let frame = solid_frame([200, 100, 20], 1, 1);
        let first_result = first_session.inspect(&frame).unwrap();
        let FieldObservationSubmission::Submitted(first_pending) = first_result.field_submission
        else {
            panic!("first result was not submitted");
        };
        let _ = first_session.finish(DiagnosticRunStatus::Success, 40, Duration::from_secs(1));

        let second_root = tempfile::tempdir().unwrap();
        let mut second_session = FieldObservationSession::start_for_test(
            second_root.path(),
            descriptor("integrated-field-second", 2),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(CompleteObserver),
            1,
        )
        .unwrap();
        assert!(matches!(
            second_session.wait_field_observation(&first_pending, Duration::from_secs(1)),
            FieldObservationSessionPoll::BindingMismatch
        ));
        let finished =
            second_session.finish(DiagnosticRunStatus::Success, 40, Duration::from_secs(1));
        assert_eq!(
            finished.diagnostic.completeness,
            Some(DiagnosticCompleteness::Partial)
        );
    }

    #[test]
    fn disconnected_pending_becomes_terminal_without_repeating_its_sequence_degradation() {
        let root = tempfile::tempdir().unwrap();
        let mut session = FieldObservationSession::start_for_test(
            root.path(),
            descriptor("integrated-field-disconnected", 1),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(PanickingObserver),
            1,
        )
        .unwrap();
        let frame = solid_frame([200, 100, 20], 1, 1);
        let result = session.inspect(&frame).unwrap();
        let FieldObservationSubmission::Submitted(pending) = result.field_submission else {
            panic!("result screen did not submit complete field inputs");
        };

        assert!(matches!(
            session.wait_field_observation(&pending, Duration::from_secs(1)),
            FieldObservationSessionPoll::WorkerUnavailable
        ));
        assert!(matches!(
            session.poll_field_observation(&pending),
            FieldObservationSessionPoll::Terminal
        ));
        assert!(matches!(
            session.poll_field_observation(&pending),
            FieldObservationSessionPoll::Terminal
        ));

        let finished = session.finish(DiagnosticRunStatus::Success, 40, Duration::from_secs(1));
        assert_eq!(
            finished.field_observer.status,
            FieldObserverFinishStatus::WorkerUnavailable
        );
        assert_eq!(
            finished.diagnostic.completeness,
            Some(DiagnosticCompleteness::Partial)
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(
                root.path()
                    .join("integrated-field-disconnected/manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let sequence_degradations: Vec<_> = manifest["degradations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| {
                item["reason"] == "field_observer_unavailable" && item["affected_sequence"] == 1
            })
            .collect();
        assert_eq!(sequence_degradations.len(), 1);
    }
}
