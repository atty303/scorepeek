use super::*;

pub(super) struct GamescopeCanonicalFrameSource<'a> {
    pub(super) lease: &'a mut CaptureLease,
    pub(super) counters: &'a mut FieldObservationCounters,
    pub(super) sink: &'a mut BoundedDiagnosticSink,
    pub(super) normalizer: NormalizationWorker,
}

pub(super) type NormalizationResult = Result<
    (NormalizedCanonicalFrame, CalibratedSourceFrameEvidence),
    scorepeek::capture::CaptureError,
>;

pub(super) struct NormalizationCompletion {
    pub(super) source_sequence: u64,
    pub(super) result: NormalizationResult,
}

pub(super) struct NormalizationWorker {
    pub(super) input: Option<SyncSender<scorepeek::capture::ObservedFrame>>,
    pub(super) output: Receiver<NormalizationCompletion>,
    pub(super) worker: Option<JoinHandle<()>>,
    pub(super) pending: bool,
}

impl NormalizationWorker {
    pub(super) fn start(
        normalizer: AdmittedFrameNormalizer,
    ) -> Result<Self, (FieldObservationGateErrorType, Option<CaptureErrorType>)> {
        let (input, frames) = mpsc::sync_channel::<scorepeek::capture::ObservedFrame>(1);
        let (results, output) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("scorepeek-capture-normalizer".to_owned())
            .spawn(move || {
                while let Ok(frame) = frames.recv() {
                    let source_sequence = frame.source_sequence();
                    if results
                        .send(NormalizationCompletion {
                            source_sequence,
                            result: normalizer.normalize_with_source(frame),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|_| {
                (
                    FieldObservationGateErrorType::NormalizationFailed,
                    Some(CaptureErrorType::ReceiverFailed),
                )
            })?;
        Ok(Self {
            input: Some(input),
            output,
            worker: Some(worker),
            pending: false,
        })
    }

    pub(super) fn take_ready(&mut self) -> Option<NormalizationCompletion> {
        match self.output.try_recv() {
            Ok(result) => {
                self.pending = false;
                Some(result)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    pub(super) fn submit(&mut self, frame: scorepeek::capture::ObservedFrame) -> Result<(), ()> {
        if self.pending {
            return Err(());
        }
        self.input
            .as_ref()
            .ok_or(())?
            .try_send(frame)
            .map_err(|_| ())?;
        self.pending = true;
        Ok(())
    }

    pub(super) fn wait_ready(&mut self, timeout: Duration) -> Option<NormalizationCompletion> {
        if !self.pending {
            return None;
        }
        match self.output.recv_timeout(timeout) {
            Ok(result) => {
                self.pending = false;
                Some(result)
            }
            Err(_) => None,
        }
    }
}

impl Drop for NormalizationWorker {
    fn drop(&mut self) {
        self.input.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl CanonicalFrameSource for GamescopeCanonicalFrameSource<'_> {
    type Error = (FieldObservationGateErrorType, Option<CaptureErrorType>);

    fn next_frame(
        &mut self,
        maximum_wait: Duration,
    ) -> Result<Option<BoundCanonicalFrame>, Self::Error> {
        if let Some(completion) = self.normalizer.take_ready() {
            let error_type = completion
                .result
                .as_ref()
                .err()
                .map(scorepeek::capture::CaptureError::error_type);
            self.lease.record_worker_normalization(
                completion.source_sequence,
                error_type,
                self.sink,
            );
            let (normalized, source) = completion.result.map_err(|error| {
                (
                    FieldObservationGateErrorType::NormalizationFailed,
                    Some(error.error_type()),
                )
            })?;
            self.counters.normalized_frames = self.counters.normalized_frames.saturating_add(1);
            return Ok(Some(BoundCanonicalFrame::from_normalized_with_source(
                normalized, source,
            )));
        }
        if !self.normalizer.pending
            && let Some(observed) = self.lease.take_latest_observed_frame()
        {
            self.counters.observed_frames = self.counters.observed_frames.saturating_add(1);
            self.normalizer.submit(observed).map_err(|()| {
                (
                    FieldObservationGateErrorType::NormalizationFailed,
                    Some(CaptureErrorType::ReceiverFailed),
                )
            })?;
        }
        if let Some(completion) = self.normalizer.wait_ready(maximum_wait) {
            let error_type = completion
                .result
                .as_ref()
                .err()
                .map(scorepeek::capture::CaptureError::error_type);
            self.lease.record_worker_normalization(
                completion.source_sequence,
                error_type,
                self.sink,
            );
            let (normalized, source) = completion.result.map_err(|error| {
                (
                    FieldObservationGateErrorType::NormalizationFailed,
                    Some(error.error_type()),
                )
            })?;
            self.counters.normalized_frames = self.counters.normalized_frames.saturating_add(1);
            return Ok(Some(BoundCanonicalFrame::from_normalized_with_source(
                normalized, source,
            )));
        }
        let poll_timeout = if self.normalizer.pending {
            Duration::ZERO
        } else {
            maximum_wait
        };
        self.lease.poll(poll_timeout, self.sink).map_err(|error| {
            (
                FieldObservationGateErrorType::CaptureFailed,
                Some(error.error_type()),
            )
        })?;
        Ok(None)
    }
}
