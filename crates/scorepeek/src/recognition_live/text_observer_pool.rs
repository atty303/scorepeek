use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use scorepeek::recognition::{
    DynamicTextObservation, OnnxParityError, RegisteredDynamicTitleRuntime, Rgb8Crop,
    ScreenTextField,
};

const MAX_TEXT_JOBS_PER_FRAME: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecognitionExecutionMode {
    Live,
    Offline,
}

impl RecognitionExecutionMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live_half_available_parallelism_v1",
            Self::Offline => "offline_available_parallelism_minus_one_v1",
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
        RecognitionExecutionMode::Offline => available_parallelism.saturating_sub(1),
    };
    requested.clamp(1, MAX_TEXT_JOBS_PER_FRAME)
}

struct TextJob {
    field: ScreenTextField,
    crop: Rgb8Crop,
    dispatched: Instant,
    response: Sender<TextJobResult>,
}

struct TextJobResult {
    field: ScreenTextField,
    observation: Result<DynamicTextObservation, OnnxParityError>,
    completed_after_dispatch_us: u64,
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
}

pub struct RegisteredTextObserverPool {
    senders: Vec<Sender<TextWorkerMessage>>,
    workers: Vec<JoinHandle<()>>,
    configuration: TextObserverPoolConfiguration,
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
        let worker_count = select_text_worker_count(execution_mode, available_parallelism);
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
                .spawn(move || run_text_worker(&mut runtime, &receiver))
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
        if jobs.is_empty() || jobs.len() > MAX_TEXT_JOBS_PER_FRAME {
            return Err(OnnxParityError::InvalidArtifact);
        }
        let dispatched = Instant::now();
        let mut pending = Vec::with_capacity(jobs.len());
        for (index, (field, crop)) in jobs.into_iter().enumerate() {
            let (response, receiver) = mpsc::channel();
            self.senders[index % self.senders.len()]
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
        for (expected_field, receiver) in self.pending {
            let result = receiver
                .recv()
                .map_err(|_| OnnxParityError::InvalidArtifact)?;
            if result.field != expected_field {
                return Err(OnnxParityError::InvalidArtifact);
            }
            wall_us = wall_us.max(result.completed_after_dispatch_us);
            observations.push((expected_field, result.observation));
        }
        Ok(TextObservationBatch {
            observations,
            wall_us,
        })
    }
}

fn run_text_worker(
    runtime: &mut RegisteredDynamicTitleRuntime,
    receiver: &Receiver<TextWorkerMessage>,
) {
    while let Ok(message) = receiver.recv() {
        match message {
            TextWorkerMessage::Observe(job) => {
                let observation = runtime.observe_open_text(&job.crop);
                let completed_after_dispatch_us = duration_us(job.dispatched.elapsed());
                let _ = job.response.send(TextJobResult {
                    field: job.field,
                    observation,
                    completed_after_dispatch_us,
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
            5
        );
        assert_eq!(
            select_text_worker_count(RecognitionExecutionMode::Offline, 1),
            1
        );
        assert_eq!(
            select_text_worker_count(RecognitionExecutionMode::Offline, 4),
            3
        );
        assert_eq!(
            select_text_worker_count(RecognitionExecutionMode::Offline, 32),
            5
        );
    }
}
