use crate::catalog::Catalog;
use crate::recognition::music_select::{
    observe_music_select_difficulty, observe_music_select_play_side, observe_music_select_play_type,
};
use crate::recognition::result::numeric::{NumericBatchInference, RegisteredNumericRuntime};
use crate::recognition::result::observed_result_difficulty;
use crate::recognition::screen::{
    ScreenFieldObservationError, ScreenFieldObservations, observe_result_fields_with_numeric,
};
use crate::recognition::shared::{CatalogCandidateDomain, CatalogCandidateDomainError};
use crate::recognition::title::OnnxParityError;
use crate::recognition::{
    music_select as music_select_recognition, result as result_recognition,
    screen as screen_recognition, title as title_recognition,
};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::model::session::{
    ProjectedScreenFieldObservation, RecognitionProcessingTiming, RegisteredScreenFieldObservation,
    TitleEvidenceObservation,
};
use crate::recognition::candidate_execution::ParallelCandidateExecution;
use crate::recognition::text_observer_pool::{
    PendingTextRecognition, RecognitionExecutionMode, RegisteredTextRecognitionSession,
    TextRecognitionResult,
};

const TITLE_EVIDENCE_RUNTIME_MANIFEST: &[u8] =
    include_bytes!("../../../../models/manifests/title-evidence-runtime-v2.json");
pub const TITLE_EVIDENCE_RUNTIME_MANIFEST_SHA256: &str =
    "1be327a2b18c3fa1274c167b2f267e9bb6a5c6b076a843818e49f3980db9642b";

/// Borrowed canonical crops and session-local sequence for one admitted field input.
pub struct FieldRecognitionInput<'a> {
    sequence: u64,
    crops: &'a screen_recognition::ScreenRgb8Crops,
    field_queue_wait_us: u64,
}

impl<'a> FieldRecognitionInput<'a> {
    #[must_use]
    pub const fn new(
        sequence: u64,
        crops: &'a screen_recognition::ScreenRgb8Crops,
        field_queue_wait_us: u64,
    ) -> Self {
        Self {
            sequence,
            crops,
            field_queue_wait_us,
        }
    }

    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
    #[must_use]
    pub const fn crops(&self) -> &'a screen_recognition::ScreenRgb8Crops {
        self.crops
    }
    #[must_use]
    pub const fn field_queue_wait_us(&self) -> u64 {
        self.field_queue_wait_us
    }
}

/// Core-owned field engine with shared OCR workers and session-local pending input.
pub struct RegisteredScreenFieldObserver {
    catalog: Arc<Catalog>,
    text_pool: Arc<RegisteredTextRecognitionSession>,
    numeric_worker: Arc<RegisteredNumericObserverWorker>,
    candidate_domain: Arc<CatalogCandidateDomain>,
    prefetched_text: Arc<Mutex<BTreeMap<u64, PendingTextRecognition>>>,
    prefetched_numeric: Arc<Mutex<BTreeMap<u64, PendingNumericObservationBatch>>>,
    pending_slots: Arc<PendingSlots>,
    projection_cache: VecDeque<ProjectionCacheEntry>,
}

const PROJECTION_CACHE_ENTRIES: usize = 4;
const MAX_PREFETCHED_FRAMES: usize = 8;

#[derive(Default)]
struct PendingSlots {
    sequences: Mutex<BTreeSet<u64>>,
    available: Condvar,
}

impl PendingSlots {
    fn acquire(&self, sequence: u64) -> Result<(), OnnxParityError> {
        let mut sequences = self
            .sequences
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if sequences.contains(&sequence) {
            return Err(OnnxParityError::InvalidArtifact);
        }
        while sequences.len() == MAX_PREFETCHED_FRAMES {
            sequences = self
                .available
                .wait(sequences)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if sequences.contains(&sequence) {
                return Err(OnnxParityError::InvalidArtifact);
            }
        }
        sequences.insert(sequence);
        Ok(())
    }

    fn release(&self, sequence: u64) {
        if self
            .sequences
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&sequence)
        {
            self.available.notify_one();
        }
    }
}

struct ProjectionCacheEntry {
    fields: ScreenFieldObservations,
    title_evidence: Option<TitleEvidenceObservation>,
    observation: ProjectedScreenFieldObservation,
}

#[derive(Debug)]
pub enum RegisteredFieldLoadError {
    Manifest,
    NumericModel(OnnxParityError),
    CandidateDomain(CatalogCandidateDomainError),
    TextRuntime(OnnxParityError),
}

impl fmt::Display for RegisteredFieldLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest => write!(formatter, "registered title evidence manifest differs"),
            Self::NumericModel(error) | Self::TextRuntime(error) => error.fmt(formatter),
            Self::CandidateDomain(error) => error.fmt(formatter),
        }
    }
}

impl Error for RegisteredFieldLoadError {}

/// Verified repository-registered text model bytes, resolved by the caller without I/O in core.
pub struct RegisteredTextBundleBytes {
    files: Vec<(String, Vec<u8>)>,
}

impl RegisteredTextBundleBytes {
    /// Validates the immutable registered bundle before worker construction.
    ///
    /// # Errors
    /// Returns a registered manifest, digest, or bundle shape error.
    pub fn from_files(files: Vec<(String, Vec<u8>)>) -> Result<Self, OnnxParityError> {
        let borrowed = files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect::<Vec<_>>();
        crate::model::manifest::verify_registered_live_model_bundle_bytes(&borrowed)?;
        Ok(Self { files })
    }

    fn start_runtime(
        &self,
    ) -> Result<title_recognition::RegisteredDynamicTitleRuntime, OnnxParityError> {
        let borrowed = self
            .files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .collect::<Vec<_>>();
        title_recognition::RegisteredDynamicTitleRuntime::from_registered_bundle_bytes(&borrowed)
    }
}

/// Model execution resources shared by independently bound field sessions.
pub struct RegisteredFieldEngine {
    text_pool: Arc<RegisteredTextRecognitionSession>,
    numeric_worker: Arc<RegisteredNumericObserverWorker>,
}

impl RegisteredFieldEngine {
    /// Starts bounded text and numeric workers without binding a catalog or domain state.
    ///
    /// # Errors
    /// Returns model validation or worker startup failure.
    pub fn start(
        text_bundle: &RegisteredTextBundleBytes,
        execution_mode: RecognitionExecutionMode,
    ) -> Result<Self, RegisteredFieldLoadError> {
        verify_title_evidence_manifest().map_err(|_| RegisteredFieldLoadError::Manifest)?;
        let title_runtime = text_bundle
            .start_runtime()
            .map_err(RegisteredFieldLoadError::TextRuntime)?;
        let text_pool = Arc::new(
            RegisteredTextRecognitionSession::start(title_runtime, execution_mode)
                .map_err(RegisteredFieldLoadError::TextRuntime)?,
        );
        let numeric_runtime = RegisteredNumericRuntime::load_embedded()
            .map_err(RegisteredFieldLoadError::NumericModel)?;
        let numeric_worker = Arc::new(
            RegisteredNumericObserverWorker::start(numeric_runtime, execution_mode)
                .map_err(RegisteredFieldLoadError::NumericModel)?,
        );
        Ok(Self {
            text_pool,
            numeric_worker,
        })
    }

    /// Binds one immutable catalog to independent session-local pending inputs.
    ///
    /// # Errors
    /// Returns a candidate-domain error when a catalog song lacks a scoreable title.
    pub fn bind(
        &self,
        catalog: Catalog,
    ) -> Result<RegisteredScreenFieldObserver, RegisteredFieldLoadError> {
        let candidate_domain = CatalogCandidateDomain::from_catalog(&catalog)
            .map_err(RegisteredFieldLoadError::CandidateDomain)?;
        Ok(RegisteredScreenFieldObserver {
            catalog: Arc::new(catalog),
            text_pool: Arc::clone(&self.text_pool),
            numeric_worker: Arc::clone(&self.numeric_worker),
            candidate_domain: Arc::new(candidate_domain),
            prefetched_text: Arc::new(Mutex::new(BTreeMap::new())),
            prefetched_numeric: Arc::new(Mutex::new(BTreeMap::new())),
            pending_slots: Arc::new(PendingSlots::default()),
            projection_cache: VecDeque::new(),
        })
    }
}

impl RegisteredScreenFieldObserver {
    /// Builds the immutable full-catalog comparison domain once for this observer lifetime.
    ///
    /// # Errors
    /// Returns the exact catalog-domain error when an active song has no scoreable title.
    pub fn new(
        catalog: Catalog,
        text_bundle: &RegisteredTextBundleBytes,
        execution_mode: RecognitionExecutionMode,
    ) -> Result<Self, RegisteredFieldLoadError> {
        RegisteredFieldEngine::start(text_bundle, execution_mode)?.bind(catalog)
    }

    /// Shares model workers and immutable catalog evidence with a separate replay session.
    #[must_use]
    pub fn fork_session(&self) -> Self {
        Self {
            catalog: Arc::clone(&self.catalog),
            text_pool: Arc::clone(&self.text_pool),
            numeric_worker: Arc::clone(&self.numeric_worker),
            candidate_domain: Arc::clone(&self.candidate_domain),
            prefetched_text: Arc::new(Mutex::new(BTreeMap::new())),
            prefetched_numeric: Arc::new(Mutex::new(BTreeMap::new())),
            pending_slots: Arc::new(PendingSlots::default()),
            projection_cache: VecDeque::new(),
        }
    }

    /// Shares one session's pending inputs across live outer workers.
    #[must_use]
    pub fn fork_worker(&self) -> Self {
        Self {
            catalog: Arc::clone(&self.catalog),
            text_pool: Arc::clone(&self.text_pool),
            numeric_worker: Arc::clone(&self.numeric_worker),
            candidate_domain: Arc::clone(&self.candidate_domain),
            prefetched_text: Arc::clone(&self.prefetched_text),
            prefetched_numeric: Arc::clone(&self.prefetched_numeric),
            pending_slots: Arc::clone(&self.pending_slots),
            projection_cache: VecDeque::new(),
        }
    }

    #[must_use]
    pub fn outer_worker_count(&self, maximum_outstanding: usize) -> usize {
        let maximum = match self.text_pool.execution_mode() {
            RecognitionExecutionMode::Live => 2,
            RecognitionExecutionMode::Offline => 4,
        };
        maximum_outstanding.min(maximum)
    }

    /// Starts independent text and numeric work before ordered completion.
    ///
    /// # Errors
    /// Returns the failed field and model or worker cause; failed submission releases its slot.
    pub fn prefetch_fields(
        &self,
        input: &FieldRecognitionInput<'_>,
    ) -> Result<(), ScreenFieldObservationError<OnnxParityError>> {
        let sequence = input.sequence();
        let field = first_text_field(input.crops());
        self.pending_slots
            .acquire(sequence)
            .map_err(|error| ScreenFieldObservationError::new(field, error))?;
        let result = submit_fields(
            &self.text_pool,
            &self.numeric_worker,
            &self.prefetched_text,
            &self.prefetched_numeric,
            input,
        );
        if result.is_err() {
            self.clear_pending(sequence);
        }
        result
    }

    fn clear_pending(&self, sequence: u64) {
        self.prefetched_text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&sequence);
        self.prefetched_numeric
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&sequence);
        self.pending_slots.release(sequence);
    }

    fn project_fields(
        &mut self,
        fields: ScreenFieldObservations,
        title_evidence: Option<TitleEvidenceObservation>,
    ) -> ProjectedScreenFieldObservation {
        let lookup_started = Instant::now();
        if let Some(index) = self
            .projection_cache
            .iter()
            .position(|entry| entry.fields == fields && entry.title_evidence == title_evidence)
        {
            let entry = self
                .projection_cache
                .remove(index)
                .expect("projection cache index exists");
            let observation = entry
                .observation
                .clone()
                .with_catalog_evidence_timing(duration_us(lookup_started.elapsed()));
            self.projection_cache.push_front(entry);
            return observation;
        }
        let observation = ProjectedScreenFieldObservation::project_with::<ParallelCandidateExecution>(
            &self.candidate_domain,
            &self.catalog,
            fields.clone(),
            title_evidence.clone(),
        );
        self.projection_cache.push_front(ProjectionCacheEntry {
            fields,
            title_evidence,
            observation: observation.clone(),
        });
        self.projection_cache.truncate(PROJECTION_CACHE_ENTRIES);
        observation
    }
}

struct NumericObservationJob {
    crops: screen_recognition::ResultScreenRgb8Crops,
    response: mpsc::Sender<Result<NumericBatchInference, OnnxParityError>>,
}

enum NumericObserverMessage {
    Observe(Box<NumericObservationJob>),
    SelectBest {
        crops: music_select_recognition::MusicSelectBestCrops,
        response:
            mpsc::Sender<Result<music_select_recognition::BestNumericObservation, OnnxParityError>>,
    },
    Finish,
}

struct RegisteredNumericObserverWorker {
    senders: Vec<mpsc::SyncSender<NumericObserverMessage>>,
    workers: Vec<JoinHandle<()>>,
    next_worker: AtomicUsize,
}

impl RegisteredNumericObserverWorker {
    fn observe_best(
        &self,
        crops: &music_select_recognition::MusicSelectBestCrops,
    ) -> Result<music_select_recognition::BestNumericObservation, OnnxParityError> {
        let (response, receiver) = mpsc::channel();
        self.senders[self.next_worker.fetch_add(1, Ordering::Relaxed) % self.senders.len()]
            .send(NumericObserverMessage::SelectBest {
                crops: crops.clone(),
                response,
            })
            .map_err(|_| OnnxParityError::WorkerUnavailable)?;
        receiver
            .recv()
            .map_err(|_| OnnxParityError::WorkerUnavailable)?
    }
    fn start(
        first_runtime: RegisteredNumericRuntime,
        execution_mode: RecognitionExecutionMode,
    ) -> Result<Self, OnnxParityError> {
        let count = match execution_mode {
            RecognitionExecutionMode::Live => 1,
            RecognitionExecutionMode::Offline => std::thread::available_parallelism()
                .map_or(1, usize::from)
                .div_ceil(4)
                .clamp(1, 4),
        };
        let mut senders: Vec<mpsc::SyncSender<NumericObserverMessage>> = Vec::with_capacity(count);
        let mut workers: Vec<JoinHandle<()>> = Vec::with_capacity(count);
        let mut runtimes = vec![first_runtime];
        for _ in 1..count {
            runtimes.push(RegisteredNumericRuntime::load_embedded()?);
        }
        for (index, mut runtime) in runtimes.into_iter().enumerate() {
            let (sender, receiver) = mpsc::sync_channel(8);
            let worker = std::thread::Builder::new()
                .name(format!("scorepeek-numeric-observer-{index}"))
                .spawn(move || {
                    while let Ok(message) = receiver.recv() {
                        match message {
                            NumericObserverMessage::Observe(job) => {
                                let _ = job.response.send(runtime.observe(&job.crops));
                            }
                            NumericObserverMessage::SelectBest { crops, response } => {
                                let _ = response.send(runtime.observe_music_select_best(&crops));
                            }
                            NumericObserverMessage::Finish => return,
                        }
                    }
                });
            let worker = match worker {
                Ok(worker) => worker,
                Err(error) => {
                    for sender in &senders {
                        let _ = sender.send(NumericObserverMessage::Finish);
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
            next_worker: AtomicUsize::new(0),
        })
    }

    fn submit(
        &self,
        crops: &screen_recognition::ResultScreenRgb8Crops,
    ) -> Result<PendingNumericObservationBatch, OnnxParityError> {
        let (response, receiver) = mpsc::channel();
        self.senders[self.next_worker.fetch_add(1, Ordering::Relaxed) % self.senders.len()]
            .send(NumericObserverMessage::Observe(Box::new(
                NumericObservationJob {
                    crops: crops.clone(),
                    response,
                },
            )))
            .map_err(|_| OnnxParityError::WorkerUnavailable)?;
        Ok(PendingNumericObservationBatch { receiver })
    }
}

impl Drop for RegisteredNumericObserverWorker {
    fn drop(&mut self) {
        for sender in &self.senders {
            let _ = sender.send(NumericObserverMessage::Finish);
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

struct PendingNumericObservationBatch {
    receiver: mpsc::Receiver<Result<NumericBatchInference, OnnxParityError>>,
}

impl PendingNumericObservationBatch {
    fn join(self) -> Result<NumericBatchInference, OnnxParityError> {
        self.receiver
            .recv()
            .map_err(|_| OnnxParityError::WorkerUnavailable)?
    }
}

fn submit_fields(
    text_pool: &RegisteredTextRecognitionSession,
    numeric_worker: &RegisteredNumericObserverWorker,
    prefetched_text: &Mutex<BTreeMap<u64, PendingTextRecognition>>,
    prefetched_numeric: &Mutex<BTreeMap<u64, PendingNumericObservationBatch>>,
    input: &FieldRecognitionInput<'_>,
) -> Result<(), ScreenFieldObservationError<OnnxParityError>> {
    submit_text_fields(text_pool, prefetched_text, input)?;
    if let screen_recognition::ScreenRgb8Crops::Result(crops) = input.crops() {
        let pending = numeric_worker.submit(crops).map_err(|source| {
            ScreenFieldObservationError::new(
                screen_recognition::ScreenTextField::ResultNumericBatch,
                source,
            )
        })?;
        if prefetched_numeric
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(input.sequence(), pending)
            .is_some()
        {
            return Err(ScreenFieldObservationError::new(
                screen_recognition::ScreenTextField::ResultNumericBatch,
                OnnxParityError::InvalidArtifact,
            ));
        }
    }
    Ok(())
}

fn first_text_field(
    crops: &screen_recognition::ScreenRgb8Crops,
) -> screen_recognition::ScreenTextField {
    use screen_recognition::{ScreenRgb8Crops, ScreenTextField};
    match crops {
        ScreenRgb8Crops::Title(_) => ScreenTextField::TitleGameVersion,
        ScreenRgb8Crops::Result(_) => ScreenTextField::ResultDifficulty,
        ScreenRgb8Crops::MusicSelect(_) => ScreenTextField::MusicSelectBestHeader,
    }
}

fn music_select_text_jobs(
    crops: &music_select_recognition::MusicSelectScreenRgb8Crops,
) -> Vec<(
    screen_recognition::ScreenTextField,
    screen_recognition::Rgb8Crop,
)> {
    use screen_recognition::ScreenTextField;
    let foreground =
        screen_recognition::TitleEvidenceExtractor::REGISTERED.extract(&crops.active_list_title);
    let mut jobs = vec![
        (
            ScreenTextField::MusicSelectBestHeader,
            crops.best.header.clone(),
        ),
        (
            ScreenTextField::MusicSelectBestClearType,
            crops.best.clear_type.clone(),
        ),
        (ScreenTextField::MusicSelectArtist, crops.artist.clone()),
        (
            ScreenTextField::MusicSelectActiveListTitle,
            crops.active_list_title.clone(),
        ),
    ];
    if let Some((crop, _)) = foreground {
        jobs.push((ScreenTextField::MusicSelectActiveListTitle, crop));
    }
    jobs
}

fn submit_text_fields(
    text_pool: &RegisteredTextRecognitionSession,
    prefetched_text: &Mutex<BTreeMap<u64, PendingTextRecognition>>,
    input: &FieldRecognitionInput<'_>,
) -> Result<(), ScreenFieldObservationError<OnnxParityError>> {
    use screen_recognition::ScreenTextField;
    let jobs = match input.crops() {
        screen_recognition::ScreenRgb8Crops::Title(crops) => vec![(
            ScreenTextField::TitleGameVersion,
            crops.game_version.clone(),
        )],
        screen_recognition::ScreenRgb8Crops::Result(crops) => vec![
            (ScreenTextField::ResultDifficulty, crops.difficulty.clone()),
            (ScreenTextField::ResultPlayType, crops.play_type.clone()),
            (ScreenTextField::ResultTitle, crops.title.clone()),
            (ScreenTextField::ResultArtist, crops.artist.clone()),
            (ScreenTextField::ResultClearType, crops.clear_type.clone()),
            (
                ScreenTextField::ResultPreviousClearType,
                crops.previous_clear_type.clone(),
            ),
            (
                ScreenTextField::ResultPlayOptions,
                crops.play_options.clone(),
            ),
        ],
        screen_recognition::ScreenRgb8Crops::MusicSelect(crops) => music_select_text_jobs(crops),
    };
    let error_field = jobs[0].0;
    let pending = text_pool
        .submit(jobs)
        .map_err(|source| ScreenFieldObservationError::new(error_field, source))?;
    if prefetched_text
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(input.sequence(), pending)
        .is_some()
    {
        return Err(ScreenFieldObservationError::new(
            error_field,
            OnnxParityError::InvalidArtifact,
        ));
    }
    Ok(())
}

fn verify_title_evidence_manifest() -> Result<(), OnnxParityError> {
    let mut manifest_sha256 = String::with_capacity(64);
    for byte in Sha256::digest(TITLE_EVIDENCE_RUNTIME_MANIFEST) {
        let _ = write!(manifest_sha256, "{byte:02x}");
    }
    if manifest_sha256 == TITLE_EVIDENCE_RUNTIME_MANIFEST_SHA256 {
        Ok(())
    } else {
        Err(OnnxParityError::InvalidArtifact)
    }
}

struct ObservedFrameFields {
    fields: ScreenFieldObservations,
    numeric_batch: Option<NumericBatchInference>,
    text_batch_wall_us: u64,
    maximum_text_worker_queue_wait_us: u64,
    maximum_text_worker_inference_us: u64,
    text_worker_busy_us: u64,
    text_worker_ids: Vec<usize>,
    join_started: Instant,
    title_evidence: Option<TitleEvidenceObservation>,
}

impl RegisteredScreenFieldObserver {
    fn observe_title(
        &mut self,
        sequence: u64,
        crops: &screen_recognition::TitleScreenRgb8Crops,
    ) -> Result<ObservedFrameFields, ScreenFieldObservationError<OnnxParityError>> {
        use screen_recognition::ScreenTextField;
        let pending = if let Some(pending) = self
            .prefetched_text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&sequence)
        {
            pending
        } else {
            self.text_pool
                .submit(vec![(
                    ScreenTextField::TitleGameVersion,
                    crops.game_version.clone(),
                )])
                .map_err(|source| {
                    ScreenFieldObservationError::new(ScreenTextField::TitleGameVersion, source)
                })?
        };
        let mut text = pending.join().map_err(|source| {
            ScreenFieldObservationError::new(ScreenTextField::TitleGameVersion, source)
        })?;
        let join_started = Instant::now();
        let game_version =
            take_text(&mut text, ScreenTextField::TitleGameVersion).map_err(|source| {
                ScreenFieldObservationError::new(ScreenTextField::TitleGameVersion, source)
            })?;
        Ok(ObservedFrameFields {
            fields: ScreenFieldObservations::Title(
                screen_recognition::TitleScreenFieldObservations { game_version },
            ),
            numeric_batch: None,
            text_batch_wall_us: text.wall_us,
            maximum_text_worker_queue_wait_us: text.maximum_queue_wait_us,
            maximum_text_worker_inference_us: text.maximum_worker_inference_us,
            text_worker_busy_us: text.worker_busy_us,
            text_worker_ids: text.worker_ids,
            join_started,
            title_evidence: None,
        })
    }

    fn observe_result(
        &mut self,
        sequence: u64,
        crops: &screen_recognition::ResultScreenRgb8Crops,
    ) -> Result<ObservedFrameFields, ScreenFieldObservationError<OnnxParityError>> {
        use screen_recognition::ScreenTextField;
        let pending = if let Some(pending) = self
            .prefetched_text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&sequence)
        {
            pending
        } else {
            self.text_pool
                .submit(vec![
                    (ScreenTextField::ResultDifficulty, crops.difficulty.clone()),
                    (ScreenTextField::ResultPlayType, crops.play_type.clone()),
                    (ScreenTextField::ResultTitle, crops.title.clone()),
                    (ScreenTextField::ResultArtist, crops.artist.clone()),
                    (ScreenTextField::ResultClearType, crops.clear_type.clone()),
                    (
                        ScreenTextField::ResultPreviousClearType,
                        crops.previous_clear_type.clone(),
                    ),
                    (
                        ScreenTextField::ResultPlayOptions,
                        crops.play_options.clone(),
                    ),
                ])
                .map_err(|source| {
                    ScreenFieldObservationError::new(ScreenTextField::ResultDifficulty, source)
                })?
        };
        let numeric = if let Some(pending) = self
            .prefetched_numeric
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&sequence)
        {
            pending.join()
        } else {
            self.numeric_worker
                .submit(crops)
                .and_then(PendingNumericObservationBatch::join)
        };
        let mut text = pending.join().map_err(|source| {
            ScreenFieldObservationError::new(ScreenTextField::ResultDifficulty, source)
        })?;
        let join_started = Instant::now();
        let difficulty =
            take_text(&mut text, ScreenTextField::ResultDifficulty).map_err(|source| {
                ScreenFieldObservationError::new(ScreenTextField::ResultDifficulty, source)
            })?;
        let mut numeric = numeric.map_err(|source| {
            ScreenFieldObservationError::new(ScreenTextField::ResultNumericBatch, source)
        })?;
        numeric
            .join_level(observed_result_difficulty(&difficulty))
            .map_err(|source| {
                ScreenFieldObservationError::new(ScreenTextField::ResultNumericBatch, source)
            })?;
        let mut fields = observe_result_fields_with_numeric(crops, &numeric, |field, _| {
            if field == ScreenTextField::ResultDifficulty {
                Ok(difficulty.clone())
            } else {
                take_text(&mut text, field)
            }
        })?;
        fields.play_options = match take_text_result(&mut text, ScreenTextField::ResultPlayOptions)
            .map_err(|source| {
                ScreenFieldObservationError::new(ScreenTextField::ResultPlayOptions, source)
            })? {
            Ok(observation) => {
                result_recognition::observe_play_options(&crops.play_options, &observation)
            }
            Err(_) => result_recognition::PlayOptionsObservation::failed(&crops.play_options),
        };
        Ok(ObservedFrameFields {
            fields: ScreenFieldObservations::Result(fields),
            numeric_batch: Some(numeric),
            text_batch_wall_us: text.wall_us,
            maximum_text_worker_queue_wait_us: text.maximum_queue_wait_us,
            maximum_text_worker_inference_us: text.maximum_worker_inference_us,
            text_worker_busy_us: text.worker_busy_us,
            text_worker_ids: text.worker_ids,
            join_started,
            title_evidence: None,
        })
    }

    fn observe_music_select_best(
        &self,
        crops: &music_select_recognition::MusicSelectScreenRgb8Crops,
        text: &mut TextRecognitionResult,
    ) -> music_select_recognition::MusicSelectBestObservation {
        use screen_recognition::ScreenTextField;
        let mut failures = Vec::new();
        let numeric = self
            .numeric_worker
            .observe_best(&crops.best)
            .unwrap_or_else(|error| {
                failures.push(format!("numeric: {error}"));
                music_select_recognition::BestNumericObservation::default()
            });
        let mut read = |field| {
            take_text(text, field).map_or_else(
                |error| {
                    failures.push(format!("{field:?}: {error}"));
                    String::new()
                },
                |value| value.open_text,
            )
        };
        let header = read(ScreenTextField::MusicSelectBestHeader);
        let clear = read(ScreenTextField::MusicSelectBestClearType);
        let mut best = music_select_recognition::resolve_music_select_best(header, clear, numeric);
        best.failures = failures;
        best
    }

    fn observe_music_select(
        &mut self,
        sequence: u64,
        crops: &music_select_recognition::MusicSelectScreenRgb8Crops,
    ) -> Result<ObservedFrameFields, ScreenFieldObservationError<OnnxParityError>> {
        use screen_recognition::ScreenTextField;
        let selected_difficulty = observe_music_select_difficulty(&crops.difficulty_markers);
        let play_side = observe_music_select_play_side(&crops.play_side);
        let play_type = observe_music_select_play_type(&crops.play_type).map_err(|_| {
            ScreenFieldObservationError::new(
                ScreenTextField::MusicSelectArtist,
                OnnxParityError::InvalidArtifact,
            )
        })?;
        let foreground = screen_recognition::TitleEvidenceExtractor::REGISTERED
            .extract(&crops.active_list_title);
        let jobs = music_select_text_jobs(crops);
        let pending = if let Some(pending) = self
            .prefetched_text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&sequence)
        {
            pending
        } else {
            self.text_pool.submit(jobs).map_err(|source| {
                ScreenFieldObservationError::new(ScreenTextField::MusicSelectArtist, source)
            })?
        };
        let mut text = pending.join().map_err(|source| {
            ScreenFieldObservationError::new(ScreenTextField::MusicSelectArtist, source)
        })?;
        let join_started = Instant::now();
        let full = take_text(&mut text, ScreenTextField::MusicSelectActiveListTitle).map_err(
            |source| {
                ScreenFieldObservationError::new(
                    ScreenTextField::MusicSelectActiveListTitle,
                    source,
                )
            },
        )?;
        let foreground_observation = if foreground.is_some() {
            Some(
                take_text(&mut text, ScreenTextField::MusicSelectActiveListTitle).map_err(
                    |source| {
                        ScreenFieldObservationError::new(
                            ScreenTextField::MusicSelectActiveListTitle,
                            source,
                        )
                    },
                )?,
            )
        } else {
            None
        };
        let selected = foreground_observation.clone().unwrap_or_default();
        let normalized_text = crate::recognition::title::normalized_title_key(&selected.open_text);
        let title_evidence = TitleEvidenceObservation {
            extractor_id: "scorepeek-active-title-gray80-bbox-x4-full-y-v1",
            runtime_manifest_sha256: TITLE_EVIDENCE_RUNTIME_MANIFEST_SHA256,
            selected_view: if foreground_observation.is_some() {
                "foreground"
            } else {
                "foreground_mask_absent"
            },
            full,
            foreground: foreground_observation,
            normalized_scalar_count: normalized_text.chars().count(),
            normalized_text,
            geometry: foreground.as_ref().map(|(_, geometry)| *geometry),
            mask_absent: foreground.is_none(),
        };
        let best = self.observe_music_select_best(crops, &mut text);
        let fields = ScreenFieldObservations::MusicSelect(
            music_select_recognition::MusicSelectScreenFieldObservations {
                best,
                central_title: title_recognition::DynamicTextObservation::default(),
                artist: take_text(&mut text, ScreenTextField::MusicSelectArtist).map_err(
                    |source| {
                        ScreenFieldObservationError::new(ScreenTextField::MusicSelectArtist, source)
                    },
                )?,
                play_type,
                selected_difficulty,
                play_side,
                active_list_title: selected,
            },
        );
        Ok(ObservedFrameFields {
            fields,
            numeric_batch: None,
            text_batch_wall_us: text.wall_us,
            maximum_text_worker_queue_wait_us: text.maximum_queue_wait_us,
            maximum_text_worker_inference_us: text.maximum_worker_inference_us,
            text_worker_busy_us: text.worker_busy_us,
            text_worker_ids: text.worker_ids,
            join_started,
            title_evidence: Some(title_evidence),
        })
    }
}

impl RegisteredScreenFieldObserver {
    /// Completes one field input in its caller's canonical sequence.
    ///
    /// # Errors
    /// Returns the failed field and cause without producing a partial observation.
    pub fn observe(
        &mut self,
        input: &FieldRecognitionInput<'_>,
    ) -> Result<RegisteredScreenFieldObservation, ScreenFieldObservationError<OnnxParityError>>
    {
        let outcome = (|| {
            let frame_started = Instant::now();
            let observed = match input.crops() {
                screen_recognition::ScreenRgb8Crops::Title(crops) => {
                    self.observe_title(input.sequence(), crops)?
                }
                screen_recognition::ScreenRgb8Crops::Result(crops) => {
                    self.observe_result(input.sequence(), crops)?
                }
                screen_recognition::ScreenRgb8Crops::MusicSelect(crops) => {
                    self.observe_music_select(input.sequence(), crops)?
                }
            };
            let observation = self.project_fields(observed.fields, observed.title_evidence);
            let numeric_recognition_us = observed
                .numeric_batch
                .as_ref()
                .map(|batch| batch.elapsed_us);
            let catalog_evidence_us = observation.catalog_evidence_us();
            let timing = RecognitionProcessingTiming {
                execution_policy: self.text_pool.execution_mode().as_str(),
                available_parallelism: self.text_pool.available_parallelism(),
                text_workers: self.text_pool.worker_count(),
                frame_total_us: duration_us(frame_started.elapsed()),
                field_queue_wait_us: input.field_queue_wait_us(),
                text_batch_wall_us: observed.text_batch_wall_us,
                maximum_text_worker_queue_wait_us: observed.maximum_text_worker_queue_wait_us,
                maximum_text_worker_inference_us: observed.maximum_text_worker_inference_us,
                text_worker_busy_us: observed.text_worker_busy_us,
                text_worker_ids: observed.text_worker_ids,
                numeric_recognition_us,
                join_us: duration_us(observed.join_started.elapsed()),
                catalog_evidence_us,
                screen_classification_us: None,
                crop_prepare_us: None,
                screen_resolver_us: None,
                attempt_finalization_us: None,
                output_us: None,
                frame_end_to_end_wall_us: None,
            };
            Ok(observation.complete(observed.numeric_batch, timing))
        })();
        self.clear_pending(input.sequence());
        outcome
    }
}

fn take_text(
    batch: &mut TextRecognitionResult,
    field: screen_recognition::ScreenTextField,
) -> Result<title_recognition::DynamicTextObservation, OnnxParityError> {
    take_text_result(batch, field)?
}

fn take_text_result(
    batch: &mut TextRecognitionResult,
    field: screen_recognition::ScreenTextField,
) -> Result<Result<title_recognition::DynamicTextObservation, OnnxParityError>, OnnxParityError> {
    let index = batch
        .observations
        .iter()
        .position(|(candidate, _)| *candidate == field)
        .ok_or(OnnxParityError::InvalidArtifact)?;
    Ok(batch.observations.remove(index).1)
}

fn duration_us(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use crate::catalog::{Catalog, Difficulty};
    use crate::event::DomainInput;
    use crate::event::coordinator::{CoordinatorPolicy, DomainCoordinator};
    use crate::recognition::music_select::{
        MusicSelectScreenFieldObservations, MusicSelectSongResolution, MusicSelectSongUnknownReason,
    };
    use crate::recognition::result::{
        ResultSongResolution, ResultSongUnknownReason, resolve_clear_type,
    };
    use crate::recognition::screen::ResultScreenFieldObservations;
    use crate::recognition::title::DynamicTextObservation;

    use super::*;

    #[test]
    fn session_pending_capacity_waits_for_completion_and_releases_on_teardown() {
        let slots = Arc::new(PendingSlots::default());
        for sequence in 0..MAX_PREFETCHED_FRAMES as u64 {
            slots.acquire(sequence).unwrap();
        }
        let waiting = Arc::clone(&slots);
        let (started, started_rx) = std::sync::mpsc::channel();
        let (completed, completed_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            started.send(()).unwrap();
            waiting.acquire(100).unwrap();
            completed.send(()).unwrap();
            waiting.release(100);
        });
        started_rx.recv().unwrap();
        assert!(
            completed_rx
                .recv_timeout(std::time::Duration::from_millis(20))
                .is_err()
        );
        slots.release(0);
        completed_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        worker.join().unwrap();
        for sequence in 1..MAX_PREFETCHED_FRAMES as u64 {
            slots.release(sequence);
        }
        assert!(slots.sequences.lock().unwrap().is_empty());
    }

    #[test]
    fn field_timing_and_worker_counts_do_not_change_domain_state_or_output() {
        let catalog = Catalog::default();
        let domain = CatalogCandidateDomain::from_catalog(&catalog).unwrap();
        let fields = ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: DynamicTextObservation {
                open_text: "TITLE".into(),
                ..Default::default()
            },
            artist: DynamicTextObservation {
                open_text: "ARTIST".into(),
                ..Default::default()
            },
            current_score: DynamicTextObservation {
                open_text: "1200".into(),
                ..Default::default()
            },
            ..Default::default()
        });
        let mut results = Vec::new();
        for (workers, elapsed) in [(2, 11), (12, 99)] {
            let observation = ProjectedScreenFieldObservation::project_with::<
                ParallelCandidateExecution,
            >(&domain, &catalog, fields.clone(), None);
            let mut timing =
                RecognitionProcessingTiming::unmeasured(observation.catalog_evidence_us());
            timing.text_workers = workers;
            timing.frame_total_us = elapsed;
            timing.text_worker_ids = (0..workers).collect();
            let observation = observation.complete(None, timing);
            let mut coordinator = DomainCoordinator::new(CoordinatorPolicy::default()).unwrap();
            let start = DomainInput::SessionStarted {
                session_id: "session".into(),
            };
            let screen = DomainInput::ScreenChanged {
                session_id: Some("session".into()),
                screen_episode_id: 1,
                sequence: 1,
                monotonic_end_ms: 100,
                screen: crate::recognition::screen::ScreenClass::Result,
            };
            coordinator.step(1, &start).unwrap();
            coordinator.step(2, &screen).unwrap();
            let input = DomainInput::from_registered_field("session", 1, 2, 200, &observation);
            let outputs = coordinator.step(3, &input).unwrap();
            let semantic_outputs = outputs
                .effects()
                .iter()
                .map(|effect| format!("{effect:?}"))
                .collect::<Vec<_>>();
            results.push((
                observation.fields().clone(),
                observation.candidates().clone(),
                format!("{:?}", coordinator.state().snapshot()),
                semantic_outputs,
            ));
        }
        assert_eq!(results[0], results[1]);
    }

    fn project_fields(
        domain: &CatalogCandidateDomain,
        fields: ScreenFieldObservations,
    ) -> RegisteredScreenFieldObservation {
        let projected =
            ProjectedScreenFieldObservation::project(domain, &Catalog::default(), fields, None);
        let timing = RecognitionProcessingTiming::unmeasured(projected.catalog_evidence_us());
        projected.complete(None, timing)
    }

    #[test]
    fn clear_type_resolution_accepts_registered_values_and_display_aliases() {
        assert_eq!(resolve_clear_type("EXH-CLEAR"), Some("EXH-CLEAR"));
        assert_eq!(resolve_clear_type("XH-CLEAR"), Some("EXH-CLEAR"));
        assert_eq!(resolve_clear_type("A-CLEAR"), Some("ASSIST CLEAR"));
        assert_eq!(resolve_clear_type("H-CLEAR"), Some("HARD CLEAR"));
        assert_eq!(resolve_clear_type("F-COMBO"), Some("F-COMBO"));
        assert_eq!(resolve_clear_type(""), None);
        assert_eq!(resolve_clear_type("UNRELATED"), None);
    }

    #[test]
    fn initial_blank_result_clear_type_remains_absent_evidence() {
        let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
        let fields = ScreenFieldObservations::Result(ResultScreenFieldObservations::default());
        let output = project_fields(&domain, fields);
        assert_eq!(output.clear_type(), None);
    }

    #[test]
    fn registered_output_keeps_fields_and_full_catalog_evidence_together() {
        let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
        let fields = ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "title".to_owned(),
                constrained_text: None,
            },
            artist: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "artist".to_owned(),
                constrained_text: None,
            },
            clear_type: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "FAILED".to_owned(),
                constrained_text: None,
            },
            difficulty: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "HYPER".to_owned(),
                constrained_text: None,
            },
            level: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "8".to_owned(),
                constrained_text: None,
            },
            notes: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "127".to_owned(),
                constrained_text: None,
            },
            current_score: DynamicTextObservation {
                input_width: 1,
                output_timesteps: 1,
                open_text: "1".to_owned(),
                constrained_text: None,
            },
            ..Default::default()
        });
        let output = project_fields(&domain, fields.clone());

        assert_eq!(output.fields(), &fields);
        assert_eq!(output.candidates().candidate_count(), 0);
        assert!(matches!(
            output.result_resolution(),
            Some(ResultSongResolution::Unknown {
                reason: ResultSongUnknownReason::NoCatalogCandidates,
                ..
            })
        ));
    }

    #[test]
    fn registered_output_resolves_the_matching_music_select_screen_shape() {
        let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
        let text = |value: &str| DynamicTextObservation {
            input_width: 1,
            output_timesteps: 1,
            open_text: value.to_owned(),
            constrained_text: None,
        };
        let fields = ScreenFieldObservations::MusicSelect(MusicSelectScreenFieldObservations {
            best: music_select_recognition::MusicSelectBestObservation::default(),
            central_title: text("texture"),
            artist: text("artist"),
            play_type: music_select_recognition::MusicSelectPlayTypeObservation::default(),
            selected_difficulty: music_select_difficulty(Difficulty::Hyper),
            play_side: music_select_recognition::test_music_select_play_side(None),
            active_list_title: text("TITLE"),
        });
        let output = project_fields(&domain, fields.clone());

        assert_eq!(output.fields(), &fields);
        assert!(output.result_resolution().is_none());
        assert!(matches!(
            output.music_select_resolution(),
            Some(MusicSelectSongResolution::Unknown {
                reason: MusicSelectSongUnknownReason::NoCatalogCandidates,
                ..
            })
        ));
    }

    fn music_select_difficulty(
        selected: Difficulty,
    ) -> music_select_recognition::MusicSelectDifficultyObservation {
        use crate::recognition::music_select::{
            MusicSelectDifficultyMarkerEvidence, MusicSelectDifficultyObservation,
            MusicSelectDifficultyState,
        };

        MusicSelectDifficultyObservation {
            predicate_id: "scorepeek-player-marker-outline-v2",
            state: MusicSelectDifficultyState::Known(selected),
            winner_score_ppm: 500_000,
            runner_up_score_ppm: 0,
            margin_ppm: 500_000,
            slots: [
                Difficulty::Beginner,
                Difficulty::Normal,
                Difficulty::Hyper,
                Difficulty::Another,
                Difficulty::Leggendaria,
            ]
            .map(|difficulty| MusicSelectDifficultyMarkerEvidence {
                difficulty,
                top_edge_ppm: u32::from(difficulty == selected) * 500_000,
                bottom_edge_ppm: u32::from(difficulty == selected) * 500_000,
                score_ppm: u32::from(difficulty == selected) * 500_000,
                qualifies: difficulty == selected,
            }),
        }
    }
}
