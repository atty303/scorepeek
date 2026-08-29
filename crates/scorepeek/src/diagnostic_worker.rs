use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Barrier, Mutex, OnceLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::diagnostic_recording::{
    CANONICAL_BYTES, DiagnosticErrorType, DiagnosticExternalDegradation, DiagnosticFact,
    DiagnosticFinishOutcome, DiagnosticFrameInput, DiagnosticPolicy, DiagnosticRecorder,
    DiagnosticRunDescriptor, DiagnosticRunStatus, DiagnosticSourceFrameInput,
};
use scorepeek::capture::{UncalibratedMemoryType, UncalibratedVideoContract};

pub const DEFAULT_DIAGNOSTIC_QUEUE_CAPACITY: usize = 2;
const DIAGNOSTIC_FACT_QUEUE_CAPACITY: usize = 256;
pub const DEFAULT_DIAGNOSTIC_FLUSH_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PENDING_QUEUE_DROPS: usize = 4_096;

#[derive(Default)]
struct DiagnosticWorkerHooks {
    start_gate: Option<Arc<Barrier>>,
    exit_gate: Option<Arc<Barrier>>,
    finished: Option<Arc<AtomicBool>>,
}

#[derive(Debug)]
pub struct DiagnosticOwnedFrame {
    pub sequence: u64,
    pub monotonic_start_ms: u64,
    pub monotonic_end_ms: u64,
    pub pixels: Arc<Box<[u8]>>,
    pub source: Option<DiagnosticOwnedSourceFrame>,
}

#[derive(Debug)]
pub struct DiagnosticOwnedSourceFrame {
    pub source_sequence: u64,
    pub contract: UncalibratedVideoContract,
    pub memory_type: UncalibratedMemoryType,
    pub stride: u32,
    pub received_monotonic_ns: u64,
    pub bytes: Arc<Box<[u8]>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticEnqueueOutcome {
    Enqueued,
    SkippedCadence,
    Rejected,
    Disabled,
    QueueFull,
    WorkerUnavailable,
}

enum DiagnosticWorkerMessage {
    Frame(DiagnosticOwnedFrame),
    Frames(Vec<DiagnosticOwnedFrame>),
    Fact(DiagnosticFact),
    Finish {
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
        degradations: Vec<DiagnosticExternalDegradation>,
        unbound_drops: Vec<(DiagnosticErrorType, u64, u64)>,
        last_error_type: Option<DiagnosticErrorType>,
        response: SyncSender<DiagnosticFinishOutcome>,
    },
}

enum DiagnosticWorkerState {
    Disabled,
    Unavailable,
    Active {
        frame_sender: SyncSender<DiagnosticWorkerMessage>,
        fact_sender: SyncSender<DiagnosticWorkerMessage>,
        worker: JoinHandle<()>,
        cancellation: Arc<AtomicBool>,
    },
}

struct DiagnosticFinishRequest {
    pending_degradations: Vec<DiagnosticExternalDegradation>,
    overflow_counts: [u64; DiagnosticErrorType::COUNT],
    overflow_entry_counts: [u64; DiagnosticErrorType::COUNT],
    last_external_error: Option<DiagnosticErrorType>,
    status: DiagnosticRunStatus,
    monotonic_end_ms: u64,
    timeout: Duration,
}

pub struct DiagnosticWorkerHandle {
    state: DiagnosticWorkerState,
    sample_interval_ms: u64,
    run_monotonic_start_ms: u64,
    last_sample_slot_ms: Option<u64>,
    last_offered_sequence: Option<u64>,
    last_offered_monotonic_ms: Option<u64>,
    last_offered_monotonic_end_ms: Option<u64>,
    pending_degradations: Vec<DiagnosticExternalDegradation>,
    overflow_counts: [u64; DiagnosticErrorType::COUNT],
    overflow_entry_counts: [u64; DiagnosticErrorType::COUNT],
    last_external_error: Option<DiagnosticErrorType>,
}

impl DiagnosticWorkerHandle {
    #[must_use]
    pub fn start(
        root: &std::path::Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
    ) -> Self {
        Self::start_inner(
            root.to_owned(),
            descriptor,
            policy,
            DEFAULT_DIAGNOSTIC_QUEUE_CAPACITY,
            Some(production_supervisor()),
            DiagnosticWorkerHooks::default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn start_for_test(
        root: &std::path::Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        capacity: usize,
    ) -> Self {
        Self::start_inner(
            root.to_owned(),
            descriptor,
            policy,
            capacity,
            None,
            DiagnosticWorkerHooks::default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn start_with_supervisor_for_test(
        root: &std::path::Path,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        supervisor: &Mutex<Weak<()>>,
    ) -> Self {
        Self::start_inner(
            root.to_owned(),
            descriptor,
            policy,
            DEFAULT_DIAGNOSTIC_QUEUE_CAPACITY,
            Some(supervisor),
            DiagnosticWorkerHooks::default(),
        )
    }

    fn start_inner(
        root: std::path::PathBuf,
        descriptor: DiagnosticRunDescriptor,
        policy: DiagnosticPolicy,
        capacity: usize,
        supervisor: Option<&Mutex<Weak<()>>>,
        hooks: DiagnosticWorkerHooks,
    ) -> Self {
        let run_monotonic_start_ms = descriptor.monotonic_start_ms;
        if !policy.enabled {
            return Self::with_state(
                DiagnosticWorkerState::Disabled,
                policy.sample_interval_ms,
                run_monotonic_start_ms,
            );
        }
        if capacity == 0 {
            return Self::with_state(
                DiagnosticWorkerState::Unavailable,
                policy.sample_interval_ms,
                run_monotonic_start_ms,
            );
        }
        let supervisor_token = if let Some(supervisor) = supervisor {
            let Some(token) = acquire_worker_token(supervisor) else {
                return Self::with_state(
                    DiagnosticWorkerState::Unavailable,
                    policy.sample_interval_ms,
                    run_monotonic_start_ms,
                );
            };
            Some(token)
        } else {
            None
        };
        let sample_interval_ms = policy.sample_interval_ms;
        let cancellation = Arc::new(AtomicBool::new(false));
        let worker_cancellation = Arc::clone(&cancellation);
        let (frame_sender, frame_receiver) = mpsc::sync_channel(capacity);
        let (fact_sender, fact_receiver) = mpsc::sync_channel(DIAGNOSTIC_FACT_QUEUE_CAPACITY);
        let worker = thread::Builder::new()
            .name("scorepeek-diagnostic-writer".to_owned())
            .spawn(move || {
                let mut supervisor_token = supervisor_token;
                if let Some(gate) = hooks.start_gate {
                    gate.wait();
                }
                run_worker(
                    &frame_receiver,
                    &fact_receiver,
                    &root,
                    &descriptor,
                    policy,
                    &worker_cancellation,
                    &mut supervisor_token,
                );
                if let Some(gate) = hooks.exit_gate {
                    gate.wait();
                }
                if let Some(finished) = hooks.finished {
                    finished.store(true, Ordering::Release);
                }
            });
        match worker {
            Ok(worker) => Self::with_state(
                DiagnosticWorkerState::Active {
                    frame_sender,
                    fact_sender,
                    worker,
                    cancellation,
                },
                sample_interval_ms,
                run_monotonic_start_ms,
            ),
            Err(_) => Self::with_state(
                DiagnosticWorkerState::Unavailable,
                sample_interval_ms,
                run_monotonic_start_ms,
            ),
        }
    }

    fn with_state(
        state: DiagnosticWorkerState,
        sample_interval_ms: u64,
        run_monotonic_start_ms: u64,
    ) -> Self {
        Self {
            state,
            sample_interval_ms,
            run_monotonic_start_ms,
            last_sample_slot_ms: None,
            last_offered_sequence: None,
            last_offered_monotonic_ms: None,
            last_offered_monotonic_end_ms: None,
            pending_degradations: Vec::new(),
            overflow_counts: [0; DiagnosticErrorType::COUNT],
            overflow_entry_counts: [0; DiagnosticErrorType::COUNT],
            last_external_error: None,
        }
    }

    pub fn try_record_frame(&mut self, frame: DiagnosticOwnedFrame) -> DiagnosticEnqueueOutcome {
        if !matches!(self.state, DiagnosticWorkerState::Active { .. }) {
            return self.inactive_outcome();
        }
        if !self.validate_frame_offer(&frame) {
            return DiagnosticEnqueueOutcome::Rejected;
        }
        if !self.claim_sample_slot(frame.monotonic_start_ms) {
            return DiagnosticEnqueueOutcome::SkippedCadence;
        }
        let sequence = frame.sequence;
        self.try_send(DiagnosticWorkerMessage::Frame(frame), sequence)
    }

    pub fn observe_frame(&mut self, frame: &DiagnosticOwnedFrame) -> DiagnosticEnqueueOutcome {
        if !matches!(self.state, DiagnosticWorkerState::Active { .. }) {
            return self.inactive_outcome();
        }
        if self.validate_frame_offer(frame) {
            DiagnosticEnqueueOutcome::SkippedCadence
        } else {
            DiagnosticEnqueueOutcome::Rejected
        }
    }

    pub fn try_record_observed_frames(
        &mut self,
        frames: Vec<DiagnosticOwnedFrame>,
    ) -> DiagnosticEnqueueOutcome {
        if frames.is_empty() {
            return DiagnosticEnqueueOutcome::SkippedCadence;
        }
        let valid = frames.windows(2).all(|pair| {
            pair[0].sequence < pair[1].sequence
                && pair[0].monotonic_start_ms < pair[1].monotonic_start_ms
        }) && frames.iter().all(|frame| {
            frame.pixels.len() == CANONICAL_BYTES
                && frame.monotonic_end_ms >= frame.monotonic_start_ms
                && frame.monotonic_start_ms >= self.run_monotonic_start_ms
                && self
                    .last_offered_sequence
                    .is_some_and(|offered| frame.sequence <= offered)
        });
        if !valid {
            for frame in &frames {
                self.record_queue_drop(DiagnosticErrorType::InvalidConfiguration, frame.sequence);
            }
            return DiagnosticEnqueueOutcome::Rejected;
        }
        let sequences = frames
            .iter()
            .map(|frame| frame.sequence)
            .collect::<Vec<_>>();
        self.try_send_batch(DiagnosticWorkerMessage::Frames(frames), &sequences)
    }

    pub fn try_record_fact(&mut self, fact: DiagnosticFact) -> DiagnosticEnqueueOutcome {
        let sequence = fact.sequence;
        self.try_send(DiagnosticWorkerMessage::Fact(fact), sequence)
    }

    pub fn record_recognition_busy_skip(
        &mut self,
        sequence: u64,
        monotonic_start_ms: u64,
        monotonic_end_ms: u64,
    ) -> DiagnosticEnqueueOutcome {
        if self
            .last_offered_sequence
            .is_some_and(|previous| sequence <= previous)
            || self
                .last_offered_monotonic_ms
                .is_some_and(|previous| monotonic_start_ms <= previous)
            || monotonic_end_ms < monotonic_start_ms
        {
            self.record_queue_drop(DiagnosticErrorType::TimingNonmonotonic, sequence);
            return DiagnosticEnqueueOutcome::Rejected;
        }
        if let Some(previous) = self.last_offered_sequence
            && sequence > previous.saturating_add(1)
        {
            self.record_sequence_gap(previous + 1, sequence - 1);
        }
        self.last_offered_sequence = Some(sequence);
        self.last_offered_monotonic_ms = Some(monotonic_start_ms);
        self.last_offered_monotonic_end_ms = Some(monotonic_end_ms);
        self.try_record_fact(DiagnosticFact {
            sequence,
            monotonic_start_ms,
            monotonic_end_ms,
            operation: crate::diagnostic_recording::DiagnosticOperation::SampleRecognition,
            status: crate::diagnostic_recording::DiagnosticOperationStatus::Success,
            error_type: None,
            detail: crate::diagnostic_recording::DiagnosticDetail::RecognitionBusySkip,
        })
    }

    pub fn record_external_error(&mut self, error_type: DiagnosticErrorType, sequence: u64) {
        self.record_queue_drop(error_type, sequence);
    }

    pub fn record_external_unbound_error(&mut self, error_type: DiagnosticErrorType, count: u64) {
        if count == 0 {
            return;
        }
        self.last_external_error = Some(error_type);
        self.overflow_counts[error_type.index()] =
            self.overflow_counts[error_type.index()].saturating_add(count);
        self.overflow_entry_counts[error_type.index()] =
            self.overflow_entry_counts[error_type.index()].saturating_add(1);
    }

    pub fn record_frame_until(
        &mut self,
        mut frame: DiagnosticOwnedFrame,
        deadline: Instant,
    ) -> DiagnosticEnqueueOutcome {
        if !matches!(self.state, DiagnosticWorkerState::Active { .. }) {
            return self.inactive_outcome();
        }
        if !self.validate_frame_offer(&frame) {
            return DiagnosticEnqueueOutcome::Rejected;
        }
        if !self.claim_sample_slot(frame.monotonic_start_ms) {
            return DiagnosticEnqueueOutcome::SkippedCadence;
        }
        loop {
            let sequence = frame.sequence;
            let DiagnosticWorkerState::Active { frame_sender, .. } = &self.state else {
                return self.inactive_outcome();
            };
            match frame_sender.try_send(DiagnosticWorkerMessage::Frame(frame)) {
                Ok(()) => return DiagnosticEnqueueOutcome::Enqueued,
                Err(TrySendError::Full(DiagnosticWorkerMessage::Frame(returned)))
                    if Instant::now() < deadline =>
                {
                    frame = returned;
                    thread::yield_now();
                }
                Err(TrySendError::Full(_)) => {
                    self.record_queue_drop(DiagnosticErrorType::QueueFull, sequence);
                    return DiagnosticEnqueueOutcome::QueueFull;
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.record_queue_drop(DiagnosticErrorType::WorkerUnavailable, sequence);
                    return DiagnosticEnqueueOutcome::WorkerUnavailable;
                }
            }
        }
    }

    #[must_use]
    pub fn finish(
        self,
        status: DiagnosticRunStatus,
        monotonic_end_ms: u64,
        timeout: Duration,
    ) -> DiagnosticFinishOutcome {
        let Self {
            state,
            sample_interval_ms: _,
            run_monotonic_start_ms: _,
            last_sample_slot_ms: _,
            last_offered_sequence: _,
            last_offered_monotonic_ms: _,
            last_offered_monotonic_end_ms: _,
            pending_degradations,
            overflow_counts,
            overflow_entry_counts,
            last_external_error,
        } = self;
        match state {
            DiagnosticWorkerState::Disabled => DiagnosticFinishOutcome {
                completeness: None,
                error_type: None,
                manifest_sha256: None,
            },
            DiagnosticWorkerState::Unavailable => unavailable_finish(),
            DiagnosticWorkerState::Active {
                frame_sender,
                fact_sender,
                worker,
                cancellation,
            } => finish_active(
                frame_sender,
                fact_sender,
                worker,
                &cancellation,
                DiagnosticFinishRequest {
                    pending_degradations,
                    overflow_counts,
                    overflow_entry_counts,
                    last_external_error,
                    status,
                    monotonic_end_ms,
                    timeout,
                },
            ),
        }
    }

    fn try_send(
        &mut self,
        message: DiagnosticWorkerMessage,
        sequence: u64,
    ) -> DiagnosticEnqueueOutcome {
        let DiagnosticWorkerState::Active {
            frame_sender,
            fact_sender,
            ..
        } = &self.state
        else {
            return self.inactive_outcome();
        };
        let sender = if matches!(message, DiagnosticWorkerMessage::Fact(_)) {
            fact_sender
        } else {
            frame_sender
        };
        match sender.try_send(message) {
            Ok(()) => DiagnosticEnqueueOutcome::Enqueued,
            Err(TrySendError::Full(_)) => {
                self.record_queue_drop(DiagnosticErrorType::QueueFull, sequence);
                DiagnosticEnqueueOutcome::QueueFull
            }
            Err(TrySendError::Disconnected(_)) => {
                self.record_queue_drop(DiagnosticErrorType::WorkerUnavailable, sequence);
                DiagnosticEnqueueOutcome::WorkerUnavailable
            }
        }
    }

    fn try_send_batch(
        &mut self,
        message: DiagnosticWorkerMessage,
        sequences: &[u64],
    ) -> DiagnosticEnqueueOutcome {
        let DiagnosticWorkerState::Active { frame_sender, .. } = &self.state else {
            return self.inactive_outcome();
        };
        match frame_sender.try_send(message) {
            Ok(()) => DiagnosticEnqueueOutcome::Enqueued,
            Err(TrySendError::Full(_)) => {
                for &sequence in sequences {
                    self.record_queue_drop(DiagnosticErrorType::QueueFull, sequence);
                }
                DiagnosticEnqueueOutcome::QueueFull
            }
            Err(TrySendError::Disconnected(_)) => {
                for &sequence in sequences {
                    self.record_queue_drop(DiagnosticErrorType::WorkerUnavailable, sequence);
                }
                DiagnosticEnqueueOutcome::WorkerUnavailable
            }
        }
    }

    fn inactive_outcome(&self) -> DiagnosticEnqueueOutcome {
        match self.state {
            DiagnosticWorkerState::Disabled => DiagnosticEnqueueOutcome::Disabled,
            DiagnosticWorkerState::Unavailable => DiagnosticEnqueueOutcome::WorkerUnavailable,
            DiagnosticWorkerState::Active { .. } => unreachable!("active state was matched"),
        }
    }

    fn claim_sample_slot(&mut self, monotonic_start_ms: u64) -> bool {
        match self.last_sample_slot_ms {
            Some(previous)
                if monotonic_start_ms > previous
                    && monotonic_start_ms - previous < self.sample_interval_ms =>
            {
                false
            }
            Some(previous) if monotonic_start_ms <= previous => true,
            _ => {
                self.last_sample_slot_ms = Some(monotonic_start_ms);
                true
            }
        }
    }

    fn validate_frame_offer(&mut self, frame: &DiagnosticOwnedFrame) -> bool {
        if self
            .last_offered_sequence
            .is_some_and(|previous| frame.sequence <= previous)
        {
            self.record_queue_drop(DiagnosticErrorType::SequenceNonmonotonic, frame.sequence);
            return false;
        }
        if let Some(previous) = self.last_offered_sequence
            && frame.sequence > previous.saturating_add(1)
        {
            self.record_sequence_gap(previous + 1, frame.sequence - 1);
        }
        self.last_offered_sequence = Some(frame.sequence);
        let invalid_configuration = frame.pixels.len() != CANONICAL_BYTES
            || frame.monotonic_end_ms < frame.monotonic_start_ms
            || frame.monotonic_start_ms < self.run_monotonic_start_ms;
        if invalid_configuration {
            self.record_queue_drop(DiagnosticErrorType::InvalidConfiguration, frame.sequence);
            return false;
        }
        if self
            .last_offered_monotonic_ms
            .is_some_and(|previous| frame.monotonic_start_ms <= previous)
            || self
                .last_offered_monotonic_end_ms
                .is_some_and(|previous| frame.monotonic_end_ms < previous)
        {
            self.record_queue_drop(DiagnosticErrorType::TimingNonmonotonic, frame.sequence);
            return false;
        }
        self.last_offered_monotonic_ms = Some(frame.monotonic_start_ms);
        self.last_offered_monotonic_end_ms = Some(frame.monotonic_end_ms);
        true
    }

    fn record_sequence_gap(&mut self, first: u64, last: u64) {
        self.last_external_error = Some(DiagnosticErrorType::CaptureSequenceGap);
        if self.pending_degradations.len() < MAX_PENDING_QUEUE_DROPS {
            self.pending_degradations
                .push(DiagnosticExternalDegradation::SequenceGap(first, last));
        } else {
            let count = last.saturating_sub(first).saturating_add(1);
            let index = DiagnosticErrorType::CaptureSequenceGap.index();
            self.overflow_counts[index] = self.overflow_counts[index].saturating_add(count);
            self.overflow_entry_counts[index] = self.overflow_entry_counts[index].saturating_add(1);
        }
    }

    fn record_queue_drop(&mut self, reason: DiagnosticErrorType, sequence: u64) {
        self.last_external_error = Some(reason);
        if self.pending_degradations.len() < MAX_PENDING_QUEUE_DROPS {
            self.pending_degradations
                .push(DiagnosticExternalDegradation::Drop(reason, sequence));
        } else {
            self.overflow_counts[reason.index()] =
                self.overflow_counts[reason.index()].saturating_add(1);
            self.overflow_entry_counts[reason.index()] =
                self.overflow_entry_counts[reason.index()].saturating_add(1);
        }
    }
}

fn finish_active(
    frame_sender: SyncSender<DiagnosticWorkerMessage>,
    fact_sender: SyncSender<DiagnosticWorkerMessage>,
    worker: JoinHandle<()>,
    cancellation: &AtomicBool,
    request: DiagnosticFinishRequest,
) -> DiagnosticFinishOutcome {
    let deadline = Instant::now() + request.timeout;
    let (response, receiver) = mpsc::sync_channel(1);
    let mut message = DiagnosticWorkerMessage::Finish {
        status: request.status,
        monotonic_end_ms: request.monotonic_end_ms,
        degradations: request.pending_degradations,
        unbound_drops: DiagnosticErrorType::ALL
            .into_iter()
            .filter_map(|reason| {
                let count = request.overflow_counts[reason.index()];
                let entries = request.overflow_entry_counts[reason.index()];
                (count > 0).then_some((reason, count, entries))
            })
            .collect(),
        last_error_type: request.last_external_error,
        response,
    };
    loop {
        match fact_sender.try_send(message) {
            Ok(()) => break,
            Err(TrySendError::Full(returned)) if Instant::now() < deadline => {
                message = returned;
                thread::yield_now();
            }
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                cancellation.store(true, Ordering::Release);
                drop(frame_sender);
                drop(fact_sender);
                drop(worker);
                return timeout_finish();
            }
        }
    }
    drop(frame_sender);
    drop(fact_sender);
    let remaining = deadline.saturating_duration_since(Instant::now());
    let Ok(outcome) = receiver.recv_timeout(remaining) else {
        cancellation.store(true, Ordering::Release);
        drop(worker);
        return timeout_finish();
    };
    drop(worker);
    outcome
}

fn run_worker(
    frame_receiver: &Receiver<DiagnosticWorkerMessage>,
    fact_receiver: &Receiver<DiagnosticWorkerMessage>,
    root: &std::path::Path,
    descriptor: &DiagnosticRunDescriptor,
    policy: DiagnosticPolicy,
    cancellation: &AtomicBool,
    supervisor_token: &mut Option<Arc<()>>,
) {
    let mut recorder = DiagnosticRecorder::start(root, descriptor, policy);
    loop {
        let message = match fact_receiver.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => {
                match frame_receiver.recv_timeout(Duration::from_millis(1)) {
                    Ok(message) => message,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        match fact_receiver.recv_timeout(Duration::from_millis(1)) {
                            Ok(message) => message,
                            Err(_) => return,
                        }
                    }
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => match frame_receiver.recv() {
                Ok(message) => message,
                Err(_) => return,
            },
        };
        match message {
            DiagnosticWorkerMessage::Frame(frame) => {
                let _ = recorder.record_sampled_frame(DiagnosticFrameInput {
                    sequence: frame.sequence,
                    monotonic_start_ms: frame.monotonic_start_ms,
                    monotonic_end_ms: frame.monotonic_end_ms,
                    pixels: &frame.pixels,
                    source: source_input(frame.source.as_ref()),
                });
            }
            DiagnosticWorkerMessage::Frames(frames) => {
                for frame in frames {
                    let _ = recorder.record_sampled_frame(DiagnosticFrameInput {
                        sequence: frame.sequence,
                        monotonic_start_ms: frame.monotonic_start_ms,
                        monotonic_end_ms: frame.monotonic_end_ms,
                        pixels: &frame.pixels,
                        source: source_input(frame.source.as_ref()),
                    });
                }
            }
            DiagnosticWorkerMessage::Fact(fact) => {
                let _ = recorder.record_fact(&fact);
            }
            DiagnosticWorkerMessage::Finish {
                status,
                monotonic_end_ms,
                degradations,
                unbound_drops,
                last_error_type,
                response,
            } => {
                for pending in frame_receiver.try_iter() {
                    record_worker_message(&mut recorder, pending);
                }
                recorder.record_external_degradations(
                    &degradations,
                    &unbound_drops,
                    last_error_type,
                );
                let outcome = recorder.finish_cancellable(status, monotonic_end_ms, cancellation);
                if !cancellation.load(Ordering::Acquire) {
                    drop(supervisor_token.take());
                }
                let _ = response.send(outcome);
                return;
            }
        }
    }
}

fn record_worker_message(recorder: &mut DiagnosticRecorder, message: DiagnosticWorkerMessage) {
    match message {
        DiagnosticWorkerMessage::Frame(frame) => {
            let _ = recorder.record_sampled_frame(DiagnosticFrameInput {
                sequence: frame.sequence,
                monotonic_start_ms: frame.monotonic_start_ms,
                monotonic_end_ms: frame.monotonic_end_ms,
                pixels: &frame.pixels,
                source: source_input(frame.source.as_ref()),
            });
        }
        DiagnosticWorkerMessage::Frames(frames) => {
            for frame in frames {
                record_worker_message(recorder, DiagnosticWorkerMessage::Frame(frame));
            }
        }
        DiagnosticWorkerMessage::Fact(fact) => {
            let _ = recorder.record_fact(&fact);
        }
        DiagnosticWorkerMessage::Finish { .. } => {}
    }
}

fn source_input(
    source: Option<&DiagnosticOwnedSourceFrame>,
) -> Option<DiagnosticSourceFrameInput<'_>> {
    source.map(|source| DiagnosticSourceFrameInput {
        source_sequence: source.source_sequence,
        contract: source.contract,
        memory_type: source.memory_type,
        stride: source.stride,
        received_monotonic_ns: source.received_monotonic_ns,
        bytes: &source.bytes,
    })
}

fn production_supervisor() -> &'static Mutex<Weak<()>> {
    static ACTIVE_WORKER: OnceLock<Mutex<Weak<()>>> = OnceLock::new();
    ACTIVE_WORKER.get_or_init(|| Mutex::new(Weak::new()))
}

fn acquire_worker_token(supervisor: &Mutex<Weak<()>>) -> Option<Arc<()>> {
    let mut active = supervisor
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if active.upgrade().is_some() {
        return None;
    }
    let token = Arc::new(());
    *active = Arc::downgrade(&token);
    Some(token)
}

fn timeout_finish() -> DiagnosticFinishOutcome {
    DiagnosticFinishOutcome {
        completeness: Some(crate::diagnostic_recording::DiagnosticCompleteness::Partial),
        error_type: Some(DiagnosticErrorType::FlushTimeout),
        manifest_sha256: None,
    }
}

fn unavailable_finish() -> DiagnosticFinishOutcome {
    DiagnosticFinishOutcome {
        completeness: Some(crate::diagnostic_recording::DiagnosticCompleteness::Dropped),
        error_type: Some(DiagnosticErrorType::WorkerUnavailable),
        manifest_sha256: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic_recording::{
        DiagnosticBinding, DiagnosticCompleteness, DiagnosticReplayBinding, DiagnosticResource,
    };
    use std::fs;

    fn descriptor(run_id: &str) -> DiagnosticRunDescriptor {
        DiagnosticRunDescriptor {
            run_id: run_id.to_owned(),
            monotonic_start_ms: 0,
            resource: DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "1".repeat(64),
            },
            binding: DiagnosticBinding {
                capture_generation: 1,
                capture_profile_sha256: "2".repeat(64),
                normalizer_sha256: "3".repeat(64),
                canonical_layout_sha256: "4".repeat(64),
                catalog_sha256: "5".repeat(64),
                model_sha256: "6".repeat(64),
                runtime_sha256: "7".repeat(64),
                replay: Some(DiagnosticReplayBinding {
                    request_sha256: "8".repeat(64),
                    extraction_sha256: "9".repeat(64),
                }),
            },
        }
    }

    fn frame(sequence: u64, time: u64) -> DiagnosticOwnedFrame {
        DiagnosticOwnedFrame {
            sequence,
            monotonic_start_ms: time,
            monotonic_end_ms: time + 16,
            pixels: Arc::new(
                vec![u8::try_from(sequence).unwrap(); 1_920 * 1_080 * 3].into_boxed_slice(),
            ),
            source: None,
        }
    }

    #[test]
    fn bounded_queue_drop_is_manifested_without_changing_the_caller_result() {
        let root = tempfile::tempdir().unwrap();
        let gate = Arc::new(Barrier::new(2));
        let mut worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("queue-drop-run"),
            DiagnosticPolicy::default(),
            1,
            None,
            DiagnosticWorkerHooks {
                start_gate: Some(Arc::clone(&gate)),
                ..DiagnosticWorkerHooks::default()
            },
        );
        assert_eq!(
            worker.try_record_frame(frame(1, 0)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        let caller_result = Result::<_, &'static str>::Ok("recognition-result");
        assert_eq!(
            worker.try_record_frame(frame(2, 50)),
            DiagnosticEnqueueOutcome::SkippedCadence
        );
        assert_eq!(
            worker.try_record_frame(frame(3, 1_000)),
            DiagnosticEnqueueOutcome::QueueFull
        );
        assert_eq!(caller_result, Ok("recognition-result"));
        gate.wait();
        let outcome = worker.finish(DiagnosticRunStatus::Success, 1_016, Duration::from_secs(5));
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Partial));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("queue-drop-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["last_error_type"], "queue_full");
        assert_eq!(manifest["degradations"][0]["affected_sequence"], 3);
    }

    #[test]
    fn frame_backpressure_does_not_consume_the_fact_queue() {
        let root = tempfile::tempdir().unwrap();
        let gate = Arc::new(Barrier::new(2));
        let mut worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("split-queue-run"),
            DiagnosticPolicy::default(),
            1,
            None,
            DiagnosticWorkerHooks {
                start_gate: Some(Arc::clone(&gate)),
                ..DiagnosticWorkerHooks::default()
            },
        );
        assert_eq!(
            worker.try_record_frame(frame(1, 0)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert_eq!(
            worker.try_record_fact(DiagnosticFact {
                sequence: 1,
                monotonic_start_ms: 0,
                monotonic_end_ms: 16,
                operation: crate::diagnostic_recording::DiagnosticOperation::SampleRecognition,
                status: crate::diagnostic_recording::DiagnosticOperationStatus::Success,
                error_type: None,
                detail: crate::diagnostic_recording::DiagnosticDetail::SamplingSummary {
                    processed_ticks: 1,
                    busy_skips: 0,
                    maximum_consecutive_busy_skips: 0,
                },
            }),
            DiagnosticEnqueueOutcome::Enqueued
        );
        gate.wait();
        let outcome = worker.finish(DiagnosticRunStatus::Success, 16, Duration::from_secs(5));
        let manifest =
            fs::read_to_string(root.path().join("split-queue-run/manifest.json")).unwrap();
        assert_eq!(
            outcome.completeness,
            Some(DiagnosticCompleteness::Complete),
            "{manifest}"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("split-queue-run/facts.ndjson"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    #[test]
    fn rejected_frame_batch_records_every_selected_sequence() {
        let root = tempfile::tempdir().unwrap();
        let gate = Arc::new(Barrier::new(2));
        let mut worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("batch-queue-drop-run"),
            DiagnosticPolicy::default(),
            1,
            None,
            DiagnosticWorkerHooks {
                start_gate: Some(Arc::clone(&gate)),
                ..DiagnosticWorkerHooks::default()
            },
        );
        assert_eq!(
            worker.try_record_frame(frame(1, 0)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        let frames = (2..=4)
            .map(|sequence| frame(sequence, (sequence - 1) * 1_000))
            .collect::<Vec<_>>();
        for frame in &frames {
            assert_eq!(
                worker.observe_frame(frame),
                DiagnosticEnqueueOutcome::SkippedCadence
            );
        }
        assert_eq!(
            worker.try_record_observed_frames(frames),
            DiagnosticEnqueueOutcome::QueueFull
        );
        gate.wait();
        let outcome = worker.finish(DiagnosticRunStatus::Success, 3_016, Duration::from_secs(5));
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Partial));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("batch-queue-drop-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["dropped_count"], 3);
        let sequences = manifest["degradations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["affected_sequence"].as_u64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![2, 3, 4]);
    }

    #[test]
    fn producer_degradations_preserve_gap_then_queue_drop_order() {
        let root = tempfile::tempdir().unwrap();
        let gate = Arc::new(Barrier::new(2));
        let mut worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("ordered-degradation-run"),
            DiagnosticPolicy::default(),
            1,
            None,
            DiagnosticWorkerHooks {
                start_gate: Some(Arc::clone(&gate)),
                ..DiagnosticWorkerHooks::default()
            },
        );
        assert_eq!(
            worker.try_record_frame(frame(1, 0)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert_eq!(
            worker.try_record_frame(frame(3, 1_000)),
            DiagnosticEnqueueOutcome::QueueFull
        );
        gate.wait();
        let outcome = worker.finish(DiagnosticRunStatus::Success, 1_016, Duration::from_secs(5));
        assert_eq!(outcome.error_type, Some(DiagnosticErrorType::QueueFull));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("ordered-degradation-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            manifest["degradations"][0]["reason"],
            "capture_sequence_gap"
        );
        assert_eq!(manifest["degradations"][1]["reason"], "queue_full");
        assert_eq!(manifest["last_error_type"], "queue_full");
    }

    #[test]
    fn producer_cadence_skip_does_not_create_a_capture_sequence_gap() {
        let root = tempfile::tempdir().unwrap();
        let mut worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("cadence-run"),
            DiagnosticPolicy::default(),
            2,
            None,
            DiagnosticWorkerHooks::default(),
        );
        assert_eq!(
            worker.try_record_frame(frame(1, 0)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert_eq!(
            worker.try_record_frame(frame(2, 50)),
            DiagnosticEnqueueOutcome::SkippedCadence
        );
        assert_eq!(
            worker.record_frame_until(frame(3, 1_000), Instant::now() + Duration::from_secs(1)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        let outcome = worker.finish(DiagnosticRunStatus::Success, 1_016, Duration::from_secs(5));
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Complete));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("cadence-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["dropped_count"], 0);
        assert_eq!(manifest["frames"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn recognition_busy_skip_is_explicit_and_does_not_create_a_capture_gap() {
        let root = tempfile::tempdir().unwrap();
        let mut worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("recognition-busy-run"),
            DiagnosticPolicy::default(),
            2,
            None,
            DiagnosticWorkerHooks::default(),
        );
        assert_eq!(
            worker.try_record_frame(frame(1, 0)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert_eq!(
            worker.record_recognition_busy_skip(2, 50, 66),
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert_eq!(
            worker.record_frame_until(frame(3, 1_000), Instant::now() + Duration::from_secs(1)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        let outcome = worker.finish(DiagnosticRunStatus::Success, 1_016, Duration::from_secs(5));
        let manifest_bytes =
            fs::read(root.path().join("recognition-busy-run/manifest.json")).unwrap();
        assert_eq!(
            outcome.completeness,
            Some(DiagnosticCompleteness::Complete),
            "{}",
            String::from_utf8_lossy(&manifest_bytes)
        );
        let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(manifest["dropped_count"], 0);
        let fact: serde_json::Value = serde_json::from_str(
            fs::read_to_string(root.path().join("recognition-busy-run/facts.ndjson"))
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(fact["fact"]["tick_sequence"], 2);
        assert_eq!(fact["fact"]["detail"]["kind"], "recognition_busy_skip");
    }

    #[test]
    fn producer_reports_a_true_capture_sequence_gap_once() {
        let root = tempfile::tempdir().unwrap();
        let mut worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("capture-gap-run"),
            DiagnosticPolicy::default(),
            2,
            None,
            DiagnosticWorkerHooks::default(),
        );
        assert_eq!(
            worker.try_record_frame(frame(1, 0)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        assert_eq!(
            worker.record_frame_until(frame(3, 1_000), Instant::now() + Duration::from_secs(1)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        let outcome = worker.finish(DiagnosticRunStatus::Success, 1_016, Duration::from_secs(5));
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Partial));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("capture-gap-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["dropped_count"], 1);
        assert_eq!(
            manifest["degradations"][0]["reason"],
            "capture_sequence_gap"
        );
        assert_eq!(manifest["degradations"][0]["first_missing_sequence"], 2);
        assert_eq!(manifest["degradations"][0]["last_missing_sequence"], 2);
    }

    #[test]
    fn flush_timeout_cancels_before_worker_finalization() {
        let root = tempfile::tempdir().unwrap();
        let start_gate = Arc::new(Barrier::new(2));
        let exit_gate = Arc::new(Barrier::new(2));
        let finished = Arc::new(AtomicBool::new(false));
        let supervisor = Mutex::new(Weak::new());
        let worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("flush-timeout-run"),
            DiagnosticPolicy::default(),
            1,
            Some(&supervisor),
            DiagnosticWorkerHooks {
                start_gate: Some(Arc::clone(&start_gate)),
                exit_gate: Some(Arc::clone(&exit_gate)),
                finished: Some(Arc::clone(&finished)),
            },
        );
        let outcome = worker.finish(DiagnosticRunStatus::Success, 0, Duration::from_millis(1));
        assert_eq!(outcome.error_type, Some(DiagnosticErrorType::FlushTimeout));
        assert!(acquire_worker_token(&supervisor).is_none());
        start_gate.wait();
        let run_file = root.path().join("flush-timeout-run/run.json");
        let deadline = Instant::now() + Duration::from_secs(1);
        while !run_file.is_file() && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(run_file.is_file());
        assert!(acquire_worker_token(&supervisor).is_none());
        exit_gate.wait();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !finished.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(finished.load(Ordering::Acquire));
        assert!(acquire_worker_token(&supervisor).is_some());
        assert!(!root.path().join("flush-timeout-run/manifest.json").exists());
    }

    #[test]
    fn completed_worker_releases_supervisor_before_the_bounded_response() {
        let root = tempfile::tempdir().unwrap();
        let exit_gate = Arc::new(Barrier::new(2));
        let finished = Arc::new(AtomicBool::new(false));
        let supervisor = Mutex::new(Weak::new());
        let worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("response-before-exit-run"),
            DiagnosticPolicy::default(),
            1,
            Some(&supervisor),
            DiagnosticWorkerHooks {
                exit_gate: Some(Arc::clone(&exit_gate)),
                finished: Some(Arc::clone(&finished)),
                ..DiagnosticWorkerHooks::default()
            },
        );
        let outcome = worker.finish(DiagnosticRunStatus::Success, 0, Duration::from_secs(1));
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Complete));
        assert!(!finished.load(Ordering::Acquire));
        let next_token = acquire_worker_token(&supervisor);
        assert!(next_token.is_some());
        exit_gate.wait();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !finished.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(finished.load(Ordering::Acquire));
    }

    #[test]
    fn rejected_offer_is_not_recounted_as_a_capture_gap() {
        let root = tempfile::tempdir().unwrap();
        let mut worker = DiagnosticWorkerHandle::start_inner(
            root.path().to_owned(),
            descriptor("rejected-offer-run"),
            DiagnosticPolicy::default(),
            2,
            None,
            DiagnosticWorkerHooks::default(),
        );
        assert_eq!(
            worker.try_record_frame(frame(1, 0)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        let mut rejected = frame(2, 500);
        rejected.pixels = Arc::new(Box::new([]));
        assert_eq!(
            worker.try_record_frame(rejected),
            DiagnosticEnqueueOutcome::Rejected
        );
        assert_eq!(
            worker.record_frame_until(frame(3, 1_000), Instant::now() + Duration::from_secs(1)),
            DiagnosticEnqueueOutcome::Enqueued
        );
        let outcome = worker.finish(DiagnosticRunStatus::Success, 1_016, Duration::from_secs(5));
        assert_eq!(outcome.completeness, Some(DiagnosticCompleteness::Partial));
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(root.path().join("rejected-offer-run/manifest.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["dropped_count"], 1);
        assert_eq!(
            manifest["degradation_reason_counts"][0]["reason"],
            "invalid_configuration"
        );
    }

    #[test]
    fn overflow_keeps_missing_count_separate_from_omitted_range_entries() {
        let mut worker =
            DiagnosticWorkerHandle::with_state(DiagnosticWorkerState::Unavailable, 1_000, 0);
        for sequence in 0..MAX_PENDING_QUEUE_DROPS {
            let first = u64::try_from(sequence).unwrap() * 2;
            worker.record_sequence_gap(first, first);
        }
        worker.record_sequence_gap(10_000, 10_999);
        let index = DiagnosticErrorType::CaptureSequenceGap.index();
        assert_eq!(worker.overflow_counts[index], 1_000);
        assert_eq!(worker.overflow_entry_counts[index], 1);
    }

    #[test]
    fn disabled_worker_is_a_noop() {
        let root = tempfile::tempdir().unwrap();
        let mut worker = DiagnosticWorkerHandle::start(
            root.path(),
            descriptor("disabled-worker-run"),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        );
        assert_eq!(
            worker.try_record_frame(frame(1, 0)),
            DiagnosticEnqueueOutcome::Disabled
        );
        assert_eq!(
            worker
                .finish(DiagnosticRunStatus::Success, 16, Duration::from_secs(1))
                .completeness,
            None
        );
        assert_eq!(root.path().read_dir().unwrap().count(), 0);
    }
}
