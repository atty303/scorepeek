use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use scorepeek::recognition::{CanonicalLayout, ScreenClass, ScreenRgb8Crops};

use super::LiveScreenRgb8Crops;
use crate::diagnostic_recording::DiagnosticRunDescriptor;

pub const DEFAULT_FIELD_OBSERVER_QUEUE_CAPACITY: usize = 2;
pub const DEFAULT_FIELD_OBSERVER_FINISH_TIMEOUT: Duration = Duration::from_secs(5);

/// Immutable recognition inputs loaded once for one field-observer worker lifetime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldObserverSessionBinding {
    run_id: String,
    identity_sha256: String,
    capture_generation: u64,
    capture_profile_sha256: String,
    normalizer_sha256: String,
    canonical_layout_sha256: String,
    catalog_sha256: String,
    model_sha256: String,
    runtime_sha256: String,
}

impl FieldObserverSessionBinding {
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    #[must_use]
    pub fn identity_sha256(&self) -> &str {
        &self.identity_sha256
    }

    #[must_use]
    pub const fn capture_generation(&self) -> u64 {
        self.capture_generation
    }

    #[must_use]
    pub fn catalog_sha256(&self) -> &str {
        &self.catalog_sha256
    }

    #[must_use]
    pub fn model_sha256(&self) -> &str {
        &self.model_sha256
    }

    #[must_use]
    pub fn runtime_sha256(&self) -> &str {
        &self.runtime_sha256
    }

    fn from_descriptor(descriptor: &DiagnosticRunDescriptor) -> Option<Self> {
        if !descriptor.is_valid()
            || descriptor.binding.replay.is_some()
            || descriptor.binding.canonical_layout_sha256 != CanonicalLayout::sha256()
        {
            return None;
        }
        let identity_sha256 = descriptor.binding.identity_sha256()?;
        Some(Self {
            run_id: descriptor.run_id.clone(),
            identity_sha256,
            capture_generation: descriptor.binding.capture_generation,
            capture_profile_sha256: descriptor.binding.capture_profile_sha256.clone(),
            normalizer_sha256: descriptor.binding.normalizer_sha256.clone(),
            canonical_layout_sha256: descriptor.binding.canonical_layout_sha256.clone(),
            catalog_sha256: descriptor.binding.catalog_sha256.clone(),
            model_sha256: descriptor.binding.model_sha256.clone(),
            runtime_sha256: descriptor.binding.runtime_sha256.clone(),
        })
    }
}

/// One worker-owned field-observer input derived only from an opaque admitted live crop owner.
#[derive(Debug)]
pub struct FieldObserverInput {
    binding: Arc<FieldObserverSessionBinding>,
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    crops: ScreenRgb8Crops,
}

impl FieldObserverInput {
    #[must_use]
    pub fn binding(&self) -> &FieldObserverSessionBinding {
        &self.binding
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub const fn monotonic_start_ms(&self) -> u64 {
        self.monotonic_start_ms
    }

    #[must_use]
    pub const fn monotonic_end_ms(&self) -> u64 {
        self.monotonic_end_ms
    }

    #[must_use]
    pub const fn screen(&self) -> ScreenClass {
        match &self.crops {
            ScreenRgb8Crops::Result(_) => ScreenClass::Result,
            ScreenRgb8Crops::MusicSelect(_) => ScreenClass::MusicSelect,
        }
    }

    #[must_use]
    pub const fn crops(&self) -> &ScreenRgb8Crops {
        &self.crops
    }
}

/// Application-provided model/catalog observer owned exclusively by one worker thread.
pub trait FieldObserver: Send + 'static {
    type Output: Send + 'static;

    fn observe(&mut self, input: &FieldObserverInput) -> Self::Output;
}

#[derive(Debug)]
pub enum FieldObserverStartError<E> {
    InvalidBinding,
    Load(E),
    WorkerUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldObserverOfferError {
    BindingMismatch,
    OutstandingLimit,
    QueueFull,
    WorkerUnavailable,
}

/// One result whose provenance is supplied by the worker rather than observer output.
#[derive(Debug)]
pub struct BoundFieldObservation<T> {
    binding: Arc<FieldObserverSessionBinding>,
    sequence: u64,
    monotonic_start_ms: u64,
    monotonic_end_ms: u64,
    screen: ScreenClass,
    output: T,
}

impl<T> BoundFieldObservation<T> {
    #[must_use]
    pub fn binding(&self) -> &FieldObserverSessionBinding {
        &self.binding
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    #[must_use]
    pub const fn monotonic_start_ms(&self) -> u64 {
        self.monotonic_start_ms
    }

    #[must_use]
    pub const fn monotonic_end_ms(&self) -> u64 {
        self.monotonic_end_ms
    }

    #[must_use]
    pub const fn screen(&self) -> ScreenClass {
        self.screen
    }

    #[must_use]
    pub const fn output(&self) -> &T {
        &self.output
    }

    #[must_use]
    pub fn into_output(self) -> T {
        self.output
    }
}

#[derive(Debug)]
pub struct PendingFieldObservation<T> {
    receiver: Receiver<BoundFieldObservation<T>>,
    delivery: PendingDelivery,
}

const DELIVERY_PENDING: u8 = 0;
const DELIVERY_CONSUMED: u8 = 1;
const DELIVERY_ABANDONED: u8 = 2;
const DELIVERY_OUTSTANDING_BITS: u32 = 8;
const DELIVERY_OUTSTANDING_MASK: u64 = (1 << DELIVERY_OUTSTANDING_BITS) - 1;
const DELIVERY_ABANDONED_INCREMENT: u64 = 1 << DELIVERY_OUTSTANDING_BITS;

#[derive(Debug)]
struct PendingDelivery {
    state: AtomicU8,
    counts: Arc<AtomicU64>,
}

impl PendingDelivery {
    fn consumed(&self) {
        if self
            .state
            .compare_exchange(
                DELIVERY_PENDING,
                DELIVERY_CONSUMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.counts.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Drop for PendingDelivery {
    fn drop(&mut self) {
        if self
            .state
            .compare_exchange(
                DELIVERY_PENDING,
                DELIVERY_ABANDONED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            self.counts
                .fetch_add(DELIVERY_ABANDONED_INCREMENT - 1, Ordering::AcqRel);
        }
    }
}

#[derive(Debug)]
pub enum FieldObservationPoll<T> {
    Pending,
    Ready(BoundFieldObservation<T>),
    WorkerUnavailable,
}

impl<T> PendingFieldObservation<T> {
    #[must_use]
    pub fn poll(&self) -> FieldObservationPoll<T> {
        match self.receiver.try_recv() {
            Ok(observation) => {
                self.delivery.consumed();
                FieldObservationPoll::Ready(observation)
            }
            Err(TryRecvError::Empty) => FieldObservationPoll::Pending,
            Err(TryRecvError::Disconnected) => FieldObservationPoll::WorkerUnavailable,
        }
    }

    /// Waits only for the caller-selected bound.
    #[must_use]
    pub fn wait(&self, timeout: Duration) -> FieldObservationPoll<T> {
        match self.receiver.recv_timeout(timeout) {
            Ok(observation) => {
                self.delivery.consumed();
                FieldObservationPoll::Ready(observation)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => FieldObservationPoll::Pending,
            Err(mpsc::RecvTimeoutError::Disconnected) => FieldObservationPoll::WorkerUnavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldObserverFinishStatus {
    Complete,
    Timeout,
    WorkerUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FieldObserverFinishOutcome {
    pub status: FieldObserverFinishStatus,
    pub submitted: u64,
    pub completed: Option<u64>,
    pub abandoned: Option<u64>,
}

enum FieldObserverMessage<T> {
    Observe {
        input: Box<FieldObserverInput>,
        response: mpsc::Sender<BoundFieldObservation<T>>,
    },
    Finish {
        response: SyncSender<u64>,
    },
}

pub struct FieldObserverWorker<O: FieldObserver> {
    binding: Arc<FieldObserverSessionBinding>,
    sender: SyncSender<FieldObserverMessage<O::Output>>,
    worker: JoinHandle<()>,
    submitted: u64,
    maximum_outstanding: u8,
    delivery_counts: Arc<AtomicU64>,
}

impl<O: FieldObserver> FieldObserverWorker<O> {
    /// Loads the immutable observer inputs exactly once before starting its exclusive worker.
    ///
    /// # Errors
    /// Returns a typed error for invalid binding, loader failure, an active production worker, or
    /// thread creation failure.
    pub fn start<E>(
        descriptor: &DiagnosticRunDescriptor,
        loader: impl FnOnce(&FieldObserverSessionBinding) -> Result<O, E>,
    ) -> Result<Self, FieldObserverStartError<E>> {
        Self::start_inner(
            descriptor,
            loader,
            DEFAULT_FIELD_OBSERVER_QUEUE_CAPACITY,
            Some(production_supervisor()),
        )
    }

    #[cfg(test)]
    fn start_for_test<E>(
        descriptor: &DiagnosticRunDescriptor,
        loader: impl FnOnce(&FieldObserverSessionBinding) -> Result<O, E>,
        capacity: usize,
    ) -> Result<Self, FieldObserverStartError<E>> {
        Self::start_inner(descriptor, loader, capacity, None)
    }

    fn start_inner<E>(
        descriptor: &DiagnosticRunDescriptor,
        loader: impl FnOnce(&FieldObserverSessionBinding) -> Result<O, E>,
        capacity: usize,
        supervisor: Option<&Mutex<Weak<()>>>,
    ) -> Result<Self, FieldObserverStartError<E>> {
        let binding = Arc::new(
            FieldObserverSessionBinding::from_descriptor(descriptor)
                .ok_or(FieldObserverStartError::InvalidBinding)?,
        );
        let maximum_outstanding = u8::try_from(capacity)
            .ok()
            .filter(|maximum| *maximum != 0)
            .ok_or(FieldObserverStartError::WorkerUnavailable)?;
        let supervisor_token = match supervisor {
            Some(supervisor) => acquire_worker_token(supervisor)
                .ok_or(FieldObserverStartError::WorkerUnavailable)?,
            None => Arc::new(()),
        };
        let observer = loader(&binding).map_err(FieldObserverStartError::Load)?;
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let worker_binding = Arc::clone(&binding);
        let delivery_counts = Arc::new(AtomicU64::new(0));
        let worker = thread::Builder::new()
            .name("scorepeek-field-observer".to_owned())
            .spawn(move || {
                let completion = run_worker(observer, &receiver, &worker_binding);
                drop(receiver);
                drop(worker_binding);
                drop(supervisor_token);
                if let Some((response, completed)) = completion {
                    let _ = response.send(completed);
                }
            })
            .map_err(|_| FieldObserverStartError::WorkerUnavailable)?;
        Ok(Self {
            binding,
            sender,
            worker,
            submitted: 0,
            maximum_outstanding,
            delivery_counts,
        })
    }

    /// Transfers one opaque live crop owner without waiting for inference or queue capacity.
    ///
    /// # Errors
    /// Returns a typed error for binding mismatch, exhausted outstanding-result capacity, a full
    /// queue, or a disconnected worker.
    pub fn try_observe(
        &mut self,
        live: LiveScreenRgb8Crops<'_>,
    ) -> Result<PendingFieldObservation<O::Output>, FieldObserverOfferError> {
        let frame = live.frame;
        let binding = &self.binding;
        if live.run_binding.run_id != binding.run_id
            || live.run_binding.binding_sha256 != binding.identity_sha256
            || frame.capture_generation() != binding.capture_generation
            || frame.capture_profile_sha256() != binding.capture_profile_sha256
            || frame.normalizer_sha256() != binding.normalizer_sha256
        {
            return Err(FieldObserverOfferError::BindingMismatch);
        }
        let layout_matches = match &live.crops {
            ScreenRgb8Crops::Result(crops) => {
                crops.canonical_layout_sha256 == binding.canonical_layout_sha256
            }
            ScreenRgb8Crops::MusicSelect(crops) => {
                crops.canonical_layout_sha256 == binding.canonical_layout_sha256
            }
        };
        if !layout_matches {
            return Err(FieldObserverOfferError::BindingMismatch);
        }
        if !claim_outstanding(&self.delivery_counts, self.maximum_outstanding) {
            return Err(FieldObserverOfferError::OutstandingLimit);
        }
        let input = FieldObserverInput {
            binding: Arc::clone(binding),
            sequence: frame.sequence(),
            monotonic_start_ms: frame.monotonic_start_ms(),
            monotonic_end_ms: frame.monotonic_end_ms(),
            crops: live.crops,
        };
        let (response, receiver) = mpsc::channel();
        match self.sender.try_send(FieldObserverMessage::Observe {
            input: Box::new(input),
            response,
        }) {
            Ok(()) => {
                self.submitted = self.submitted.saturating_add(1);
                Ok(PendingFieldObservation {
                    receiver,
                    delivery: PendingDelivery {
                        state: AtomicU8::new(DELIVERY_PENDING),
                        counts: Arc::clone(&self.delivery_counts),
                    },
                })
            }
            Err(TrySendError::Full(_)) => {
                self.delivery_counts.fetch_sub(1, Ordering::AcqRel);
                Err(FieldObserverOfferError::QueueFull)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.delivery_counts.fetch_sub(1, Ordering::AcqRel);
                Err(FieldObserverOfferError::WorkerUnavailable)
            }
        }
    }

    #[must_use]
    pub fn finish(self, timeout: Duration) -> FieldObserverFinishOutcome {
        let Self {
            binding: _,
            sender,
            worker,
            submitted,
            maximum_outstanding: _,
            delivery_counts,
        } = self;
        let deadline = Instant::now() + timeout;
        let (response, receiver) = mpsc::sync_channel(1);
        let mut message = FieldObserverMessage::Finish { response };
        loop {
            match sender.try_send(message) {
                Ok(()) => break,
                Err(TrySendError::Full(returned)) if Instant::now() < deadline => {
                    message = returned;
                    thread::yield_now();
                }
                Err(TrySendError::Full(_)) => {
                    drop(sender);
                    drop(worker);
                    return FieldObserverFinishOutcome {
                        status: FieldObserverFinishStatus::Timeout,
                        submitted,
                        completed: None,
                        abandoned: None,
                    };
                }
                Err(TrySendError::Disconnected(_)) => {
                    drop(sender);
                    let _ = worker.join();
                    return FieldObserverFinishOutcome {
                        status: FieldObserverFinishStatus::WorkerUnavailable,
                        submitted,
                        completed: None,
                        abandoned: None,
                    };
                }
            }
        }
        drop(sender);
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(completed) => {
                drop(worker);
                FieldObserverFinishOutcome {
                    status: FieldObserverFinishStatus::Complete,
                    submitted,
                    completed: Some(completed),
                    abandoned: Some(abandoned_at_finish(delivery_counts.load(Ordering::Acquire))),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drop(worker);
                FieldObserverFinishOutcome {
                    status: FieldObserverFinishStatus::Timeout,
                    submitted,
                    completed: None,
                    abandoned: None,
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = worker.join();
                FieldObserverFinishOutcome {
                    status: FieldObserverFinishStatus::WorkerUnavailable,
                    submitted,
                    completed: None,
                    abandoned: None,
                }
            }
        }
    }
}

fn run_worker<O: FieldObserver>(
    mut observer: O,
    receiver: &Receiver<FieldObserverMessage<O::Output>>,
    binding: &Arc<FieldObserverSessionBinding>,
) -> Option<(SyncSender<u64>, u64)> {
    let mut completed = 0_u64;
    while let Ok(message) = receiver.recv() {
        match message {
            FieldObserverMessage::Observe { input, response } => {
                let sequence = input.sequence;
                let monotonic_start_ms = input.monotonic_start_ms;
                let monotonic_end_ms = input.monotonic_end_ms;
                let screen = input.screen();
                let output = observer.observe(&input);
                completed = completed.saturating_add(1);
                let _ = response.send(BoundFieldObservation {
                    binding: Arc::clone(binding),
                    sequence,
                    monotonic_start_ms,
                    monotonic_end_ms,
                    screen,
                    output,
                });
            }
            FieldObserverMessage::Finish { response } => {
                return Some((response, completed));
            }
        }
    }
    None
}

fn claim_outstanding(counts: &AtomicU64, maximum: u8) -> bool {
    let mut current = counts.load(Ordering::Acquire);
    loop {
        if current & DELIVERY_OUTSTANDING_MASK >= u64::from(maximum) {
            return false;
        }
        match counts.compare_exchange_weak(
            current,
            current + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

const fn abandoned_at_finish(counts: u64) -> u64 {
    (counts >> DELIVERY_OUTSTANDING_BITS) + (counts & DELIVERY_OUTSTANDING_MASK)
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

#[cfg(test)]
mod tests {
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use scorepeek::recognition::{CanonicalLayout, ScreenRgb8Crops};

    use super::*;
    use crate::diagnostic_live::LiveCanonicalFrame;
    use crate::diagnostic_recording::{DiagnosticPolicy, DiagnosticResource, DiagnosticRunStatus};
    use crate::recognition_live::LiveRecognitionSession;

    fn descriptor(run_id: &str, generation: u64) -> DiagnosticRunDescriptor {
        DiagnosticRunDescriptor {
            run_id: run_id.to_owned(),
            monotonic_start_ms: 0,
            resource: DiagnosticResource {
                program: "scorepeek",
                version: env!("CARGO_PKG_VERSION"),
                build_sha256: "1".repeat(64),
            },
            binding: crate::diagnostic_recording::DiagnosticBinding {
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

    struct InspectingObserver {
        loader_thread: thread::ThreadId,
    }

    impl FieldObserver for InspectingObserver {
        type Output = (ScreenClass, [u8; 3], bool);

        fn observe(&mut self, input: &FieldObserverInput) -> Self::Output {
            let first_pixel = match input.crops() {
                ScreenRgb8Crops::Result(crops) => crops.title.pixels()[..3].try_into().unwrap(),
                ScreenRgb8Crops::MusicSelect(crops) => {
                    crops.central_title.pixels()[..3].try_into().unwrap()
                }
            };
            (
                input.screen(),
                first_pixel,
                thread::current().id() != self.loader_thread,
            )
        }
    }

    #[test]
    fn loader_runs_once_before_worker_and_output_retains_binding() {
        let descriptor = descriptor("field-observer", 1);
        let loads = Arc::new(AtomicUsize::new(0));
        let loader_thread = thread::current().id();
        let loads_for_loader = Arc::clone(&loads);
        let mut worker = FieldObserverWorker::start_for_test(
            &descriptor,
            move |binding| {
                loads_for_loader.fetch_add(1, Ordering::Relaxed);
                assert_eq!(binding.catalog_sha256(), "5".repeat(64));
                assert_eq!(binding.model_sha256(), "6".repeat(64));
                assert_eq!(binding.runtime_sha256(), "7".repeat(64));
                Ok::<_, ()>(InspectingObserver { loader_thread })
            },
            2,
        )
        .unwrap();
        assert_eq!(loads.load(Ordering::Relaxed), 1);

        let root = tempfile::tempdir().unwrap();
        let mut session = LiveRecognitionSession::start(
            root.path(),
            descriptor.clone(),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        let frame = solid_frame([200, 100, 20], 1, 1);
        let live = session.inspect(&frame).unwrap().field_inputs.unwrap();
        let pending = worker.try_observe(live).unwrap();
        let FieldObservationPoll::Ready(observation) = pending.wait(Duration::from_secs(1)) else {
            panic!("field observation did not complete");
        };
        assert_eq!(
            observation.binding().identity_sha256(),
            descriptor.binding.identity_sha256().unwrap()
        );
        assert_eq!(observation.sequence(), 1);
        assert_eq!(observation.screen(), ScreenClass::Result);
        assert_eq!(
            observation.output(),
            &(ScreenClass::Result, [200, 100, 20], true)
        );

        assert_eq!(
            worker.finish(Duration::from_secs(1)),
            FieldObserverFinishOutcome {
                status: FieldObserverFinishStatus::Complete,
                submitted: 1,
                completed: Some(1),
                abandoned: Some(0),
            }
        );
        let _ = session.finish(DiagnosticRunStatus::Success, 40);
    }

    struct NoopObserver;

    impl FieldObserver for NoopObserver {
        type Output = ();

        fn observe(&mut self, _input: &FieldObserverInput) {}
    }

    #[test]
    fn different_run_with_same_binding_is_rejected_before_queueing() {
        let worker_descriptor = descriptor("worker-binding", 1);
        let mut worker = FieldObserverWorker::start_for_test(
            &worker_descriptor,
            |_| Ok::<_, ()>(NoopObserver),
            1,
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut session = LiveRecognitionSession::start(
            root.path(),
            descriptor("frame-binding", 1),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        let frame = solid_frame([200, 100, 20], 1, 1);
        let live = session.inspect(&frame).unwrap().field_inputs.unwrap();
        assert!(matches!(
            worker.try_observe(live),
            Err(FieldObserverOfferError::BindingMismatch)
        ));

        let second_root = tempfile::tempdir().unwrap();
        let mut different_binding_session = LiveRecognitionSession::start(
            second_root.path(),
            descriptor("worker-binding", 2),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        let different_binding_frame = solid_frame([200, 100, 20], 2, 1);
        let different_binding_live = different_binding_session
            .inspect(&different_binding_frame)
            .unwrap()
            .field_inputs
            .unwrap();
        assert!(matches!(
            worker.try_observe(different_binding_live),
            Err(FieldObserverOfferError::BindingMismatch)
        ));
        assert_eq!(worker.finish(Duration::from_secs(1)).submitted, 0);
        let _ = session.finish(DiagnosticRunStatus::Success, 40);
        let _ = different_binding_session.finish(DiagnosticRunStatus::Success, 40);
    }

    struct BlockingObserver {
        started: Option<mpsc::Sender<()>>,
        release: Arc<(Mutex<bool>, Condvar)>,
        exited: Option<mpsc::Sender<()>>,
    }

    impl FieldObserver for BlockingObserver {
        type Output = ();

        fn observe(&mut self, _input: &FieldObserverInput) {
            if let Some(started) = self.started.take() {
                let _ = started.send(());
                let (lock, condition) = &*self.release;
                let mut released = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                while !*released {
                    released = condition
                        .wait(released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
        }
    }

    impl Drop for BlockingObserver {
        fn drop(&mut self) {
            if let Some(exited) = self.exited.take() {
                let _ = exited.send(());
            }
        }
    }

    #[test]
    fn queue_full_is_nonblocking_and_abandoned_results_are_bounded() {
        let descriptor = descriptor("bounded-observer", 1);
        let (started_sender, started_receiver) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let release_for_loader = Arc::clone(&release);
        let mut worker = FieldObserverWorker::start_for_test(
            &descriptor,
            move |_| {
                Ok::<_, ()>(BlockingObserver {
                    started: Some(started_sender),
                    release: release_for_loader,
                    exited: None,
                })
            },
            1,
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut session = LiveRecognitionSession::start(
            root.path(),
            descriptor.clone(),
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        let first_frame = solid_frame([200, 100, 20], 1, 1);
        let first = worker
            .try_observe(session.inspect(&first_frame).unwrap().field_inputs.unwrap())
            .unwrap();
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        drop(first);
        let second_frame = solid_frame([200, 100, 20], 1, 2);
        let second = worker
            .try_observe(
                session
                    .inspect(&second_frame)
                    .unwrap()
                    .field_inputs
                    .unwrap(),
            )
            .unwrap();
        drop(second);
        let third_frame = solid_frame([200, 100, 20], 1, 3);
        assert!(matches!(
            worker.try_observe(session.inspect(&third_frame).unwrap().field_inputs.unwrap()),
            Err(FieldObserverOfferError::QueueFull)
        ));
        let (lock, condition) = &*release;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condition.notify_all();
        assert_eq!(
            worker.finish(Duration::from_secs(1)),
            FieldObserverFinishOutcome {
                status: FieldObserverFinishStatus::Complete,
                submitted: 2,
                completed: Some(2),
                abandoned: Some(2),
            }
        );
        let _ = session.finish(DiagnosticRunStatus::Success, 80);
    }

    #[test]
    fn outstanding_limit_covers_unconsumed_results_and_finish_counts_them() {
        let descriptor = descriptor("outstanding-observer", 1);
        let mut worker =
            FieldObserverWorker::start_for_test(&descriptor, |_| Ok::<_, ()>(NoopObserver), 1)
                .unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut session = LiveRecognitionSession::start(
            root.path(),
            descriptor,
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        let first_frame = solid_frame([200, 100, 20], 1, 1);
        let first = worker
            .try_observe(session.inspect(&first_frame).unwrap().field_inputs.unwrap())
            .unwrap();
        let second_frame = solid_frame([200, 100, 20], 1, 2);
        assert!(matches!(
            worker.try_observe(
                session
                    .inspect(&second_frame)
                    .unwrap()
                    .field_inputs
                    .unwrap()
            ),
            Err(FieldObserverOfferError::OutstandingLimit)
        ));
        assert!(matches!(
            first.wait(Duration::from_secs(1)),
            FieldObservationPoll::Ready(_)
        ));

        let third_frame = solid_frame([200, 100, 20], 1, 3);
        let unconsumed = worker
            .try_observe(session.inspect(&third_frame).unwrap().field_inputs.unwrap())
            .unwrap();
        assert_eq!(
            worker.finish(Duration::from_secs(1)),
            FieldObserverFinishOutcome {
                status: FieldObserverFinishStatus::Complete,
                submitted: 2,
                completed: Some(2),
                abandoned: Some(1),
            }
        );
        drop(unconsumed);
        let _ = session.finish(DiagnosticRunStatus::Success, 80);
    }

    struct TeardownBlockingObserver {
        started: mpsc::Sender<()>,
        finished: Option<mpsc::Sender<()>>,
        release: Arc<(Mutex<bool>, Condvar)>,
    }

    impl FieldObserver for TeardownBlockingObserver {
        type Output = ();

        fn observe(&mut self, _input: &FieldObserverInput) {}
    }

    impl Drop for TeardownBlockingObserver {
        fn drop(&mut self) {
            let _ = self.started.send(());
            let (lock, condition) = &*self.release;
            let mut released = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !*released {
                released = condition
                    .wait(released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if let Some(finished) = self.finished.take() {
                let _ = finished.send(());
            }
        }
    }

    #[test]
    fn supervisor_token_is_held_through_observer_teardown() {
        let descriptor = descriptor("teardown-observer", 1);
        let supervisor = Arc::new(Mutex::new(Weak::new()));
        let (started_sender, started_receiver) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let release_for_loader = Arc::clone(&release);
        let worker = FieldObserverWorker::start_inner(
            &descriptor,
            move |_| {
                Ok::<_, ()>(TeardownBlockingObserver {
                    started: started_sender,
                    finished: None,
                    release: release_for_loader,
                })
            },
            1,
            Some(&supervisor),
        )
        .unwrap();
        let finish_thread = thread::spawn(move || worker.finish(Duration::from_secs(1)));
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        let loads = AtomicUsize::new(0);
        let unavailable = FieldObserverWorker::start_inner(
            &descriptor,
            |_| {
                loads.fetch_add(1, Ordering::Relaxed);
                Ok::<_, ()>(NoopObserver)
            },
            1,
            Some(&supervisor),
        );
        assert!(matches!(
            unavailable,
            Err(FieldObserverStartError::WorkerUnavailable)
        ));
        assert_eq!(loads.load(Ordering::Relaxed), 0);

        let (lock, condition) = &*release;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condition.notify_all();
        assert_eq!(
            finish_thread.join().unwrap().status,
            FieldObserverFinishStatus::Complete
        );

        let replacement = FieldObserverWorker::start_inner(
            &descriptor,
            |_| Ok::<_, ()>(NoopObserver),
            1,
            Some(&supervisor),
        )
        .unwrap();
        assert_eq!(
            replacement.finish(Duration::from_secs(1)).status,
            FieldObserverFinishStatus::Complete
        );
    }

    #[test]
    fn observer_teardown_is_inside_the_bounded_finish_wait() {
        let descriptor = descriptor("bounded-teardown-observer", 1);
        let supervisor = Arc::new(Mutex::new(Weak::new()));
        let (started_sender, started_receiver) = mpsc::channel();
        let (finished_sender, finished_receiver) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let release_for_loader = Arc::clone(&release);
        let worker = FieldObserverWorker::start_inner(
            &descriptor,
            move |_| {
                Ok::<_, ()>(TeardownBlockingObserver {
                    started: started_sender,
                    finished: Some(finished_sender),
                    release: release_for_loader,
                })
            },
            1,
            Some(&supervisor),
        )
        .unwrap();

        assert_eq!(
            worker.finish(Duration::from_millis(10)).status,
            FieldObserverFinishStatus::Timeout
        );
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(matches!(
            FieldObserverWorker::start_inner(
                &descriptor,
                |_| Ok::<_, ()>(NoopObserver),
                1,
                Some(&supervisor),
            ),
            Err(FieldObserverStartError::WorkerUnavailable)
        ));

        let (lock, condition) = &*release;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condition.notify_all();
        finished_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while supervisor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade()
            .is_some()
        {
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        let replacement = FieldObserverWorker::start_inner(
            &descriptor,
            |_| Ok::<_, ()>(NoopObserver),
            1,
            Some(&supervisor),
        )
        .unwrap();
        assert_eq!(
            replacement.finish(Duration::from_secs(1)).status,
            FieldObserverFinishStatus::Complete
        );
    }

    #[test]
    fn finish_timeout_does_not_claim_a_terminal_worker_state() {
        let descriptor = descriptor("observer-timeout", 1);
        let (started_sender, started_receiver) = mpsc::channel();
        let (exited_sender, exited_receiver) = mpsc::channel();
        let release = Arc::new((Mutex::new(false), Condvar::new()));
        let release_for_loader = Arc::clone(&release);
        let mut worker = FieldObserverWorker::start_for_test(
            &descriptor,
            move |_| {
                Ok::<_, ()>(BlockingObserver {
                    started: Some(started_sender),
                    release: release_for_loader,
                    exited: Some(exited_sender),
                })
            },
            1,
        )
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut session = LiveRecognitionSession::start(
            root.path(),
            descriptor,
            DiagnosticPolicy {
                enabled: false,
                ..DiagnosticPolicy::default()
            },
        )
        .unwrap();
        let frame = solid_frame([200, 100, 20], 1, 1);
        let pending = worker
            .try_observe(session.inspect(&frame).unwrap().field_inputs.unwrap())
            .unwrap();
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            worker.finish(Duration::from_millis(10)),
            FieldObserverFinishOutcome {
                status: FieldObserverFinishStatus::Timeout,
                submitted: 1,
                completed: None,
                abandoned: None,
            }
        );
        assert!(matches!(pending.poll(), FieldObservationPoll::Pending));
        let (lock, condition) = &*release;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        condition.notify_all();
        exited_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let _ = session.finish(DiagnosticRunStatus::Success, 40);
    }

    #[test]
    fn invalid_binding_fails_before_loading() {
        let mut invalid = descriptor("invalid-observer", 1);
        invalid.binding.replay = Some(crate::diagnostic_recording::DiagnosticReplayBinding {
            request_sha256: "8".repeat(64),
            extraction_sha256: "9".repeat(64),
        });
        let loads = AtomicUsize::new(0);
        let result = FieldObserverWorker::start_for_test(
            &invalid,
            |_| {
                loads.fetch_add(1, Ordering::Relaxed);
                Ok::<_, ()>(NoopObserver)
            },
            1,
        );
        assert!(matches!(
            result,
            Err(FieldObserverStartError::InvalidBinding)
        ));
        assert_eq!(loads.load(Ordering::Relaxed), 0);

        let mut obsolete_layout = descriptor("obsolete-layout-observer", 1);
        obsolete_layout.binding.canonical_layout_sha256 = "a".repeat(64);
        let result = FieldObserverWorker::start_for_test(
            &obsolete_layout,
            |_| {
                loads.fetch_add(1, Ordering::Relaxed);
                Ok::<_, ()>(NoopObserver)
            },
            1,
        );
        assert!(matches!(
            result,
            Err(FieldObserverStartError::InvalidBinding)
        ));
        assert_eq!(loads.load(Ordering::Relaxed), 0);
    }
}
