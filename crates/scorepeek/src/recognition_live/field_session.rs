use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use scorepeek::recognition::{
    RegisteredResourceLoadError, ScreenFieldObservationError, ScreenFieldObservations,
};

use super::field_observer::{
    BoundFieldObservation, FieldObservationPoll, FieldObserver, FieldObserverFinishOutcome,
    FieldObserverOfferError, FieldObserverStartError, FieldObserverWorker, PendingFieldObservation,
};
use super::screen_field_observer::RegisteredScreenFieldObserver;
use super::{
    LiveRecognitionFrameResult, LiveRecognitionObservation, LiveRecognitionSession,
    LiveRecognitionSessionError,
};
use crate::diagnostic_live::LiveCanonicalFrame;
use crate::diagnostic_recording::{
    DiagnosticFinishOutcome, DiagnosticPolicy, DiagnosticRunDescriptor, DiagnosticRunStatus,
};
use crate::diagnostic_worker::DiagnosticEnqueueOutcome;

#[derive(Debug)]
pub enum LiveFieldObservationStartError<E> {
    FieldObserver(FieldObserverStartError<E>),
    Recognition {
        error: LiveRecognitionSessionError,
        field_observer_finish: FieldObserverFinishOutcome,
    },
}

#[derive(Debug)]
pub enum LiveFieldObservationSubmission<T> {
    NotApplicable,
    Submitted(LivePendingFieldObservation<T>),
    Rejected(FieldObserverOfferError),
}

#[derive(Debug)]
pub struct LivePendingFieldObservation<T> {
    pending: PendingFieldObservation<T>,
    owner: Arc<()>,
    identity: Arc<()>,
}

#[derive(Debug)]
pub struct LiveFieldObservationFrameResult<'a, T> {
    pub observation: LiveRecognitionObservation<'a>,
    pub field_submission: LiveFieldObservationSubmission<T>,
    pub diagnostic_frame: DiagnosticEnqueueOutcome,
    pub diagnostic_screen_fact: DiagnosticEnqueueOutcome,
}

#[derive(Debug)]
pub enum LiveFieldObservationPoll<T> {
    Pending,
    Ready {
        observation: BoundFieldObservation<T>,
        diagnostic_field_fact: DiagnosticEnqueueOutcome,
    },
    Consumed,
    BindingMismatch,
    Terminal,
    WorkerUnavailable,
}

#[derive(Debug, Eq, PartialEq)]
pub struct LiveFieldObservationFinishOutcome {
    pub field_observer: FieldObserverFinishOutcome,
    pub diagnostic: DiagnosticFinishOutcome,
}

/// One application owner for a live recognition run and its exact field observer.
pub struct LiveFieldObservationSession<O: FieldObserver> {
    recognition: LiveRecognitionSession,
    field_observer: FieldObserverWorker<O>,
    owner: Arc<()>,
    outstanding: Vec<(Arc<()>, u64)>,
}

impl<O: FieldObserver> LiveFieldObservationSession<O> {
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
    ) -> Result<Self, LiveFieldObservationStartError<E>> {
        let field_observer = FieldObserverWorker::start(&descriptor, loader)
            .map_err(LiveFieldObservationStartError::FieldObserver)?;
        let recognition = match LiveRecognitionSession::start(root, descriptor, policy) {
            Ok(recognition) => recognition,
            Err(error) => {
                return Err(LiveFieldObservationStartError::Recognition {
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
        frame: &'a LiveCanonicalFrame,
    ) -> Result<LiveFieldObservationFrameResult<'a, O::Output>, LiveRecognitionSessionError> {
        let LiveRecognitionFrameResult {
            observation,
            field_inputs,
            diagnostic_frame,
            diagnostic_fact,
        } = self.recognition.inspect(frame)?;
        let field_submission = match field_inputs {
            None => LiveFieldObservationSubmission::NotApplicable,
            Some(inputs) => match self.field_observer.try_observe(inputs) {
                Ok(pending) => {
                    let identity = Arc::new(());
                    self.outstanding
                        .push((Arc::clone(&identity), frame.sequence()));
                    LiveFieldObservationSubmission::Submitted(LivePendingFieldObservation {
                        pending,
                        owner: Arc::clone(&self.owner),
                        identity,
                    })
                }
                Err(error) => {
                    self.recognition
                        .record_field_observer_offer_failure(frame.sequence(), error);
                    LiveFieldObservationSubmission::Rejected(error)
                }
            },
        };
        Ok(LiveFieldObservationFrameResult {
            observation,
            field_submission,
            diagnostic_frame,
            diagnostic_screen_fact: diagnostic_fact,
        })
    }

    #[must_use]
    pub fn finish(
        mut self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
        field_observer_timeout: Duration,
    ) -> LiveFieldObservationFinishOutcome {
        let field_observer = self.field_observer.finish(field_observer_timeout);
        for (_, sequence) in self.outstanding {
            self.recognition
                .record_abandoned_field_observation(sequence);
        }
        self.recognition
            .record_field_observer_finish(field_observer);
        let diagnostic = self.recognition.finish(status, monotonic_end_ms);
        LiveFieldObservationFinishOutcome {
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
    ) -> Result<Self, LiveFieldObservationStartError<E>> {
        let field_observer = FieldObserverWorker::start_for_test(&descriptor, loader, capacity)
            .map_err(LiveFieldObservationStartError::FieldObserver)?;
        let supervisor = std::sync::Mutex::new(std::sync::Weak::new());
        let recognition = match LiveRecognitionSession::start_with_supervisor_for_test(
            root,
            descriptor,
            policy,
            &supervisor,
        ) {
            Ok(recognition) => recognition,
            Err(error) => {
                return Err(LiveFieldObservationStartError::Recognition {
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

impl LiveFieldObservationSession<RegisteredScreenFieldObserver> {
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
    ) -> Result<Self, LiveFieldObservationStartError<RegisteredResourceLoadError>> {
        Self::start(root, descriptor, policy, |binding| {
            binding
                .load_registered_resources(catalog_root, bundle_root)
                .map(RegisteredScreenFieldObserver::new)
        })
    }
}

impl<O, E> LiveFieldObservationSession<O>
where
    O: FieldObserver<Output = Result<ScreenFieldObservations, ScreenFieldObservationError<E>>>,
    E: Send + 'static,
{
    #[must_use]
    pub fn poll_field_observation(
        &mut self,
        pending: &LivePendingFieldObservation<O::Output>,
    ) -> LiveFieldObservationPoll<O::Output> {
        self.poll_owned_field_observation(pending, None)
    }

    /// Waits only for the caller-selected bound and records a completed value-free fact.
    #[must_use]
    pub fn wait_field_observation(
        &mut self,
        pending: &LivePendingFieldObservation<O::Output>,
        timeout: Duration,
    ) -> LiveFieldObservationPoll<O::Output> {
        self.poll_owned_field_observation(pending, Some(timeout))
    }

    fn poll_owned_field_observation(
        &mut self,
        pending: &LivePendingFieldObservation<O::Output>,
        timeout: Option<Duration>,
    ) -> LiveFieldObservationPoll<O::Output> {
        if !Arc::ptr_eq(&pending.owner, &self.owner) {
            self.recognition.reject_pending_field_observation();
            return LiveFieldObservationPoll::BindingMismatch;
        }
        let Some(index) = self
            .outstanding
            .iter()
            .position(|(identity, _)| Arc::ptr_eq(identity, &pending.identity))
        else {
            return match pending.pending.poll() {
                FieldObservationPoll::Consumed => LiveFieldObservationPoll::Consumed,
                FieldObservationPoll::Terminal => LiveFieldObservationPoll::Terminal,
                FieldObservationPoll::Pending
                | FieldObservationPoll::Ready(_)
                | FieldObservationPoll::WorkerUnavailable => {
                    LiveFieldObservationPoll::BindingMismatch
                }
            };
        };
        let sequence = self.outstanding[index].1;
        let poll = timeout.map_or_else(
            || pending.pending.poll(),
            |timeout| pending.pending.wait(timeout),
        );
        match poll {
            FieldObservationPoll::Pending => LiveFieldObservationPoll::Pending,
            FieldObservationPoll::Ready(observation) => {
                self.outstanding.swap_remove(index);
                let diagnostic_field_fact = self.recognition.record_field_observation(&observation);
                LiveFieldObservationPoll::Ready {
                    observation,
                    diagnostic_field_fact,
                }
            }
            FieldObservationPoll::Consumed => {
                self.outstanding.swap_remove(index);
                LiveFieldObservationPoll::Consumed
            }
            FieldObservationPoll::Terminal => {
                self.outstanding.swap_remove(index);
                LiveFieldObservationPoll::Terminal
            }
            FieldObservationPoll::WorkerUnavailable => {
                self.outstanding.swap_remove(index);
                self.recognition.record_field_observer_unavailable(sequence);
                LiveFieldObservationPoll::WorkerUnavailable
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

    fn solid_frame(color: [u8; 3], generation: u64, sequence: u64) -> LiveCanonicalFrame {
        let mut pixels = Vec::with_capacity(crate::diagnostic_recording::CANONICAL_BYTES);
        for _ in 0..crate::diagnostic_recording::CANONICAL_BYTES / 3 {
            pixels.extend_from_slice(&color);
        }
        LiveCanonicalFrame::for_test_pixels(
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
            observe_screen_fields(input.crops(), |crop| {
                Ok(DynamicTextObservation {
                    input_width: crop.roi.width as usize,
                    output_timesteps: 1,
                    open_text: "imperfect observation".to_owned(),
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
        let mut session = LiveFieldObservationSession::start_for_test(
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
        let LiveFieldObservationSubmission::Submitted(pending) = result.field_submission else {
            panic!("result screen did not submit complete field inputs");
        };
        let LiveFieldObservationPoll::Ready {
            observation,
            diagnostic_field_fact,
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
            LiveFieldObservationPoll::Consumed
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
    fn integrated_opt_out_keeps_the_same_complete_field_output_without_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let mut session = LiveFieldObservationSession::start_for_test(
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
        let LiveFieldObservationSubmission::Submitted(pending) = result.field_submission else {
            panic!("result screen did not submit complete field inputs");
        };
        let LiveFieldObservationPoll::Ready {
            observation,
            diagnostic_field_fact,
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
        let mut session = LiveFieldObservationSession::start_for_test(
            root.path(),
            descriptor("integrated-field-capacity", 1),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(CompleteObserver),
            1,
        )
        .unwrap();
        let first = solid_frame([200, 100, 20], 1, 1);
        let first_result = session.inspect(&first).unwrap();
        let LiveFieldObservationSubmission::Submitted(pending) = first_result.field_submission
        else {
            panic!("first result was not submitted");
        };
        let second = solid_frame([200, 100, 20], 1, 2);
        let second_result = session.inspect(&second).unwrap();
        assert_eq!(second_result.observation.screen(), ScreenClass::Result);
        assert!(matches!(
            second_result.field_submission,
            LiveFieldObservationSubmission::Rejected(FieldObserverOfferError::OutstandingLimit)
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
        let mut first_session = LiveFieldObservationSession::start_for_test(
            first_root.path(),
            descriptor("integrated-field-first", 1),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(CompleteObserver),
            1,
        )
        .unwrap();
        let frame = solid_frame([200, 100, 20], 1, 1);
        let first_result = first_session.inspect(&frame).unwrap();
        let LiveFieldObservationSubmission::Submitted(first_pending) =
            first_result.field_submission
        else {
            panic!("first result was not submitted");
        };
        let _ = first_session.finish(DiagnosticRunStatus::Success, 40, Duration::from_secs(1));

        let second_root = tempfile::tempdir().unwrap();
        let mut second_session = LiveFieldObservationSession::start_for_test(
            second_root.path(),
            descriptor("integrated-field-second", 2),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(CompleteObserver),
            1,
        )
        .unwrap();
        assert!(matches!(
            second_session.wait_field_observation(&first_pending, Duration::from_secs(1)),
            LiveFieldObservationPoll::BindingMismatch
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
        let mut session = LiveFieldObservationSession::start_for_test(
            root.path(),
            descriptor("integrated-field-disconnected", 1),
            DiagnosticPolicy::default(),
            |_| Ok::<_, ()>(PanickingObserver),
            1,
        )
        .unwrap();
        let frame = solid_frame([200, 100, 20], 1, 1);
        let result = session.inspect(&frame).unwrap();
        let LiveFieldObservationSubmission::Submitted(pending) = result.field_submission else {
            panic!("result screen did not submit complete field inputs");
        };

        assert!(matches!(
            session.wait_field_observation(&pending, Duration::from_secs(1)),
            LiveFieldObservationPoll::WorkerUnavailable
        ));
        assert!(matches!(
            session.poll_field_observation(&pending),
            LiveFieldObservationPoll::Terminal
        ));
        assert!(matches!(
            session.poll_field_observation(&pending),
            LiveFieldObservationPoll::Terminal
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
