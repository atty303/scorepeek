use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use scorepeek::recognition::{
    DynamicTextObservation, OnnxParityError, RegisteredDynamicTitleRuntime, Rgb8Crop,
    ScreenTextField,
};

const MAX_TEXT_FIELDS_PER_FRAME: usize = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecognitionExecutionMode {
    Live,
    Offline,
}

impl RecognitionExecutionMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live_half_available_parallelism_capped_12_v2",
            Self::Offline => "offline_available_parallelism_minus_four_capped_12_v2",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextObserverPoolConfiguration {
    pub available_parallelism: usize,
    pub workers: usize,
    pub execution_mode: RecognitionExecutionMode,
}

#[must_use]
pub fn select_text_worker_count(
    execution_mode: RecognitionExecutionMode,
    available_parallelism: usize,
) -> usize {
    let available_parallelism = available_parallelism.max(1);
    let requested = match execution_mode {
        RecognitionExecutionMode::Live => available_parallelism / 2,
        RecognitionExecutionMode::Offline => available_parallelism.saturating_sub(4),
    };
    requested.clamp(1, 12)
}

/// Applies the production policy, with one explicit offline-only benchmark override.
#[must_use]
pub fn configured_text_worker_count(
    execution_mode: RecognitionExecutionMode,
    available_parallelism: usize,
) -> usize {
    if execution_mode == RecognitionExecutionMode::Offline
        && std::env::var_os("SCOREPEEK_INTERNAL_SINGLE_TEXT_WORKER").as_deref()
            == Some(std::ffi::OsStr::new("1"))
    {
        1
    } else {
        select_text_worker_count(execution_mode, available_parallelism)
    }
}

struct TextJob {
    field: ScreenTextField,
    crop: Rgb8Crop,
    dispatched: Instant,
    response: Sender<TextJobResult>,
}

struct TextJobResult {
    worker_id: usize,
    field: ScreenTextField,
    observation: Result<DynamicTextObservation, OnnxParityError>,
    completed_after_dispatch_us: u64,
    queue_wait_us: u64,
    inference_us: u64,
}

enum TextWorkerMessage {
    Observe(TextJob),
    Finish,
}

pub struct TextObservationBatch {
    pub observations: Vec<(
        ScreenTextField,
        Result<DynamicTextObservation, OnnxParityError>,
    )>,
    pub wall_us: u64,
    pub maximum_queue_wait_us: u64,
    pub maximum_worker_inference_us: u64,
    pub worker_busy_us: u64,
    pub worker_ids: Vec<usize>,
}

pub struct RegisteredTextObserverPool {
    senders: Vec<Sender<TextWorkerMessage>>,
    workers: Vec<JoinHandle<()>>,
    configuration: TextObserverPoolConfiguration,
    next_worker: AtomicUsize,
}

impl RegisteredTextObserverPool {
    /// Constructs every persistent registered PP-OCR session before frame admission.
    ///
    /// # Errors
    /// Returns an error when a registered runtime cannot load or a worker thread cannot start.
    pub fn start(
        first_runtime: RegisteredDynamicTitleRuntime,
        execution_mode: RecognitionExecutionMode,
    ) -> Result<Self, OnnxParityError> {
        let available_parallelism = thread::available_parallelism().map_or(1, usize::from);
        let worker_count = configured_text_worker_count(execution_mode, available_parallelism);
        Self::start_with_worker_count(first_runtime, execution_mode, worker_count)
    }

    /// Constructs an explicitly sized pool for offline replay and benchmarking.
    ///
    /// # Errors
    /// Returns an error for a zero/unsupported worker count or runtime/thread startup failure.
    pub fn start_with_worker_count(
        first_runtime: RegisteredDynamicTitleRuntime,
        execution_mode: RecognitionExecutionMode,
        worker_count: usize,
    ) -> Result<Self, OnnxParityError> {
        let available_parallelism = thread::available_parallelism().map_or(1, usize::from);
        if worker_count == 0 || worker_count > available_parallelism {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let mut runtimes = Vec::with_capacity(worker_count);
        for _ in 1..worker_count {
            runtimes.push(first_runtime.spawn_peer()?);
        }
        runtimes.push(first_runtime);

        let mut senders: Vec<Sender<TextWorkerMessage>> = Vec::with_capacity(worker_count);
        let mut workers: Vec<JoinHandle<()>> = Vec::with_capacity(worker_count);
        for (index, mut runtime) in runtimes.into_iter().enumerate() {
            let (sender, receiver) = mpsc::channel();
            let worker = match thread::Builder::new()
                .name(format!("scorepeek-text-observer-{index}"))
                .spawn(move || run_text_worker(index, &mut runtime, &receiver))
            {
                Ok(worker) => worker,
                Err(error) => {
                    for sender in &senders {
                        let _ = sender.send(TextWorkerMessage::Finish);
                    }
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(error.into());
                }
            };
            senders.push(sender);
            workers.push(worker);
        }
        Ok(Self {
            senders,
            workers,
            configuration: TextObserverPoolConfiguration {
                available_parallelism,
                workers: worker_count,
                execution_mode,
            },
            next_worker: AtomicUsize::new(0),
        })
    }

    #[must_use]
    pub const fn configuration(&self) -> TextObserverPoolConfiguration {
        self.configuration
    }

    /// Transfers one bounded frame's independent text jobs without waiting for recognition.
    ///
    /// # Errors
    /// Returns an error for an empty or oversized batch or an unavailable worker.
    pub fn submit(
        &self,
        jobs: Vec<(ScreenTextField, Rgb8Crop)>,
    ) -> Result<PendingTextObservationBatch, OnnxParityError> {
        if jobs.is_empty() || jobs.len() > MAX_TEXT_FIELDS_PER_FRAME {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let dispatched = Instant::now();
        let mut pending = Vec::with_capacity(jobs.len());
        let first_worker = self.next_worker.fetch_add(jobs.len(), Ordering::Relaxed);
        for (index, (field, crop)) in jobs.into_iter().enumerate() {
            let (response, receiver) = mpsc::channel();
            self.senders[(first_worker + index) % self.senders.len()]
                .send(TextWorkerMessage::Observe(TextJob {
                    field,
                    crop,
                    dispatched,
                    response,
                }))
                .map_err(|_| OnnxParityError::InvalidArtifact)?;
            pending.push((field, receiver));
        }
        Ok(PendingTextObservationBatch { pending })
    }
}

impl Drop for RegisteredTextObserverPool {
    fn drop(&mut self) {
        for sender in &self.senders {
            let _ = sender.send(TextWorkerMessage::Finish);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

pub struct PendingTextObservationBatch {
    pending: Vec<(ScreenTextField, Receiver<TextJobResult>)>,
}

impl PendingTextObservationBatch {
    /// Joins all submitted jobs in deterministic field order.
    ///
    /// # Errors
    /// Returns an error when a worker disappears or returns a mismatched field.
    pub fn join(self) -> Result<TextObservationBatch, OnnxParityError> {
        let mut observations = Vec::with_capacity(self.pending.len());
        let mut wall_us = 0;
        let mut maximum_queue_wait_us = 0;
        let mut maximum_worker_inference_us = 0;
        let mut worker_busy_us = 0_u64;
        let mut worker_ids = Vec::with_capacity(self.pending.len());
        for (expected_field, receiver) in self.pending {
            let result = receiver
                .recv()
                .map_err(|_| OnnxParityError::InvalidArtifact)?;
            if result.field != expected_field {
                return Err(OnnxParityError::InvalidArtifact);
            }
            wall_us = wall_us.max(result.completed_after_dispatch_us);
            maximum_queue_wait_us = maximum_queue_wait_us.max(result.queue_wait_us);
            maximum_worker_inference_us = maximum_worker_inference_us.max(result.inference_us);
            worker_busy_us = worker_busy_us.saturating_add(result.inference_us);
            if !worker_ids.contains(&result.worker_id) {
                worker_ids.push(result.worker_id);
            }
            observations.push((expected_field, result.observation));
        }
        Ok(TextObservationBatch {
            observations,
            wall_us,
            maximum_queue_wait_us,
            maximum_worker_inference_us,
            worker_busy_us,
            worker_ids,
        })
    }
}

fn run_text_worker(
    worker_id: usize,
    runtime: &mut RegisteredDynamicTitleRuntime,
    receiver: &Receiver<TextWorkerMessage>,
) {
    while let Ok(message) = receiver.recv() {
        match message {
            TextWorkerMessage::Observe(job) => {
                let inference_started = Instant::now();
                let queue_wait_us = duration_us(inference_started.duration_since(job.dispatched));
                let observation = runtime.observe_open_text(&job.crop);
                let inference_us = duration_us(inference_started.elapsed());
                let completed_after_dispatch_us = duration_us(job.dispatched.elapsed());
                let _ = job.response.send(TextJobResult {
                    worker_id,
                    field: job.field,
                    observation,
                    completed_after_dispatch_us,
                    queue_wait_us,
                    inference_us,
                });
            }
            TextWorkerMessage::Finish => return,
        }
    }
}

fn duration_us(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_count_follows_bounded_execution_policy() {
        assert_eq!(
            select_text_worker_count(RecognitionExecutionMode::Live, 1),
            1
        );
        assert_eq!(
            select_text_worker_count(RecognitionExecutionMode::Live, 4),
            2
        );
        assert_eq!(
            select_text_worker_count(RecognitionExecutionMode::Live, 32),
            12
        );
        assert_eq!(
            select_text_worker_count(RecognitionExecutionMode::Offline, 1),
            1
        );
        assert_eq!(
            select_text_worker_count(RecognitionExecutionMode::Offline, 4),
            1
        );
        assert_eq!(
            select_text_worker_count(RecognitionExecutionMode::Offline, 32),
            12
        );
    }
}
