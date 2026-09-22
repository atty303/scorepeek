use scorepeek_core::catalog::Catalog;
use scorepeek_core::recognition::music_select::{
    observe_music_select_difficulty, observe_music_select_play_side, observe_music_select_play_type,
};
use scorepeek_core::recognition::result::numeric::{
    NumericBatchInference, RegisteredNumericRuntime,
};
use scorepeek_core::recognition::result::observed_result_difficulty;
use scorepeek_core::recognition::screen::{
    ScreenFieldObservationError, ScreenFieldObservations, observe_result_fields_with_numeric,
};
use scorepeek_core::recognition::shared::{CatalogCandidateDomain, CatalogCandidateDomainError};
use scorepeek_core::recognition::title::{
    OnnxParityError, RegisteredRecognitionResources, RegisteredResourceLoadError,
};
use scorepeek_core::recognition::{
    music_select as music_select_recognition, result as result_recognition,
    screen as screen_recognition, title as title_recognition,
};
use sha2::{Digest as _, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use super::field_observer::{FieldObserver, FieldObserverAdmission, FieldObserverInput};
use scorepeek_core::model::session::{
    PendingTextRecognition, ProjectedScreenFieldObservation, RecognitionExecutionMode,
    RecognitionProcessingTiming, RegisteredScreenFieldObservation,
    RegisteredTextRecognitionSession, TextRecognitionResult, TitleEvidenceObservation,
};

const TITLE_EVIDENCE_RUNTIME_MANIFEST: &[u8] =
    include_bytes!("../../../../../../models/manifests/title-evidence-runtime-v2.json");
pub const TITLE_EVIDENCE_RUNTIME_MANIFEST_SHA256: &str =
    "1be327a2b18c3fa1274c167b2f267e9bb6a5c6b076a843818e49f3980db9642b";

/// Production screen-field observer owning the exact resources for one immutable run.
pub struct RegisteredScreenFieldObserver {
    catalog: Catalog,
    text_pool: Arc<RegisteredTextRecognitionSession>,
    numeric_worker: Arc<RegisteredNumericObserverWorker>,
    candidate_domain: CatalogCandidateDomain,
    prefetched_text: Arc<Mutex<BTreeMap<u64, PendingTextRecognition>>>,
    prefetched_numeric: Arc<Mutex<BTreeMap<u64, PendingNumericObservationBatch>>>,
    projection_cache: VecDeque<ProjectionCacheEntry>,
}

const PROJECTION_CACHE_ENTRIES: usize = 4;

struct ProjectionCacheEntry {
    fields: ScreenFieldObservations,
    title_evidence: Option<TitleEvidenceObservation>,
    observation: ProjectedScreenFieldObservation,
}

/// Run-independent registered OCR resources shared by offline replay sessions.
pub struct SharedRegisteredScreenFieldResources {
    catalog_sha256: String,
    model_sha256: String,
    runtime_sha256: String,
    catalog: Catalog,
    text_pool: Arc<RegisteredTextRecognitionSession>,
}

#[derive(Debug)]
pub enum RegisteredScreenFieldObserverLoadError {
    Resources(RegisteredResourceLoadError),
    NumericModel(OnnxParityError),
    CandidateDomain(CatalogCandidateDomainError),
    TextRuntime(OnnxParityError),
}

impl fmt::Display for RegisteredScreenFieldObserverLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resources(error) => error.fmt(formatter),
            Self::NumericModel(error) | Self::TextRuntime(error) => error.fmt(formatter),
            Self::CandidateDomain(error) => error.fmt(formatter),
        }
    }
}

impl Error for RegisteredScreenFieldObserverLoadError {}

impl From<RegisteredResourceLoadError> for RegisteredScreenFieldObserverLoadError {
    fn from(error: RegisteredResourceLoadError) -> Self {
        Self::Resources(error)
    }
}

impl From<CatalogCandidateDomainError> for RegisteredScreenFieldObserverLoadError {
    fn from(error: CatalogCandidateDomainError) -> Self {
        Self::CandidateDomain(error)
    }
}

impl From<OnnxParityError> for RegisteredScreenFieldObserverLoadError {
    fn from(error: OnnxParityError) -> Self {
        Self::TextRuntime(error)
    }
}

impl RegisteredScreenFieldObserver {
    /// Builds the immutable full-catalog comparison domain once for this observer lifetime.
    ///
    /// # Errors
    /// Returns the exact catalog-domain error when an active song has no scoreable title.
    pub fn new(
        resources: RegisteredRecognitionResources,
        numeric_runtime: RegisteredNumericRuntime,
        execution_mode: RecognitionExecutionMode,
    ) -> Result<Self, RegisteredScreenFieldObserverLoadError> {
        let mut manifest_sha256 = String::with_capacity(64);
        for byte in Sha256::digest(TITLE_EVIDENCE_RUNTIME_MANIFEST) {
            let _ = write!(manifest_sha256, "{byte:02x}");
        }
        if manifest_sha256 != TITLE_EVIDENCE_RUNTIME_MANIFEST_SHA256 {
            return Err(OnnxParityError::InvalidArtifact.into());
        }
        let (catalog, title_runtime) = resources.into_catalog_and_title_runtime();
        let candidate_domain = CatalogCandidateDomain::from_catalog(&catalog)?;
        let text_pool = Arc::new(RegisteredTextRecognitionSession::start(
            title_runtime,
            execution_mode,
        )?);
        Ok(Self {
            catalog,
            text_pool,
            numeric_worker: Arc::new(RegisteredNumericObserverWorker::start(numeric_runtime)?),
            candidate_domain,
            prefetched_text: Arc::new(Mutex::new(BTreeMap::new())),
            prefetched_numeric: Arc::new(Mutex::new(BTreeMap::new())),
            projection_cache: VecDeque::new(),
        })
    }

    fn from_shared(
        shared: &SharedRegisteredScreenFieldResources,
        numeric_runtime: RegisteredNumericRuntime,
    ) -> Result<Self, RegisteredScreenFieldObserverLoadError> {
        verify_title_evidence_manifest()?;
        let catalog = shared.catalog.clone();
        let candidate_domain = CatalogCandidateDomain::from_catalog(&catalog)?;
        Ok(Self {
            catalog,
            text_pool: Arc::clone(&shared.text_pool),
            numeric_worker: Arc::new(RegisteredNumericObserverWorker::start(numeric_runtime)?),
            candidate_domain,
            prefetched_text: Arc::new(Mutex::new(BTreeMap::new())),
            prefetched_numeric: Arc::new(Mutex::new(BTreeMap::new())),
            projection_cache: VecDeque::new(),
        })
    }

    pub(crate) fn prefetch_fields(
        &self,
        input: &FieldObserverInput,
    ) -> Result<(), ScreenFieldObservationError<OnnxParityError>> {
        submit_fields(
            &self.text_pool,
            &self.numeric_worker,
            &self.prefetched_text,
            &self.prefetched_numeric,
            input,
        )
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
        let observation = ProjectedScreenFieldObservation::project(
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
    sender: mpsc::Sender<NumericObserverMessage>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl RegisteredNumericObserverWorker {
    fn observe_best(
        &self,
        crops: &music_select_recognition::MusicSelectBestCrops,
    ) -> Result<music_select_recognition::BestNumericObservation, OnnxParityError> {
        let (response, receiver) = mpsc::channel();
        self.sender
            .send(NumericObserverMessage::SelectBest {
                crops: crops.clone(),
                response,
            })
            .map_err(|_| OnnxParityError::InvalidArtifact)?;
        receiver
            .recv()
            .map_err(|_| OnnxParityError::InvalidArtifact)?
    }
    fn start(mut runtime: RegisteredNumericRuntime) -> Result<Self, OnnxParityError> {
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("scorepeek-numeric-observer".to_owned())
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
            })?;
        Ok(Self {
            sender,
            worker: Mutex::new(Some(worker)),
        })
    }

    fn submit(
        &self,
        crops: &screen_recognition::ResultScreenRgb8Crops,
    ) -> Result<PendingNumericObservationBatch, OnnxParityError> {
        let (response, receiver) = mpsc::channel();
        self.sender
            .send(NumericObserverMessage::Observe(Box::new(
                NumericObservationJob {
                    crops: crops.clone(),
                    response,
                },
            )))
            .map_err(|_| OnnxParityError::InvalidArtifact)?;
        Ok(PendingNumericObservationBatch { receiver })
    }
}

impl Drop for RegisteredNumericObserverWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(NumericObserverMessage::Finish);
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
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
            .map_err(|_| OnnxParityError::InvalidArtifact)?
    }
}

fn submit_fields(
    text_pool: &RegisteredTextRecognitionSession,
    numeric_worker: &RegisteredNumericObserverWorker,
    prefetched_text: &Mutex<BTreeMap<u64, PendingTextRecognition>>,
    prefetched_numeric: &Mutex<BTreeMap<u64, PendingNumericObservationBatch>>,
    input: &FieldObserverInput,
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
    input: &FieldObserverInput,
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

impl SharedRegisteredScreenFieldResources {
    /// Loads the immutable catalog/model binding once and creates one global offline text pool.
    ///
    /// # Errors
    /// Returns the registered resource or text runtime failure before session replay starts.
    pub fn load(
        descriptor: &scorepeek_core::diagnostics::DiagnosticRunDescriptor,
        catalog_root: &Path,
        bundle_root: &Path,
        text_workers: usize,
    ) -> Result<Self, RegisteredScreenFieldObserverLoadError> {
        verify_title_evidence_manifest()?;
        let resources = RegisteredRecognitionResources::load(
            catalog_root,
            bundle_root,
            &descriptor.binding.catalog_sha256,
            &descriptor.binding.model_sha256,
            &descriptor.binding.runtime_sha256,
        )?;
        let (catalog, title_runtime) = resources.into_catalog_and_title_runtime();
        let text_pool = RegisteredTextRecognitionSession::start_with_worker_count(
            title_runtime,
            RecognitionExecutionMode::Offline,
            text_workers,
        )?;
        Ok(Self {
            catalog_sha256: descriptor.binding.catalog_sha256.clone(),
            model_sha256: descriptor.binding.model_sha256.clone(),
            runtime_sha256: descriptor.binding.runtime_sha256.clone(),
            catalog,
            text_pool: Arc::new(text_pool),
        })
    }

    /// Loads another immutable catalog while reusing an existing registered text pool.
    ///
    /// Offline corpus replay uses this when one suite contains sessions captured against
    /// different catalog generations. Text inference is catalog-independent, while candidate
    /// projection remains bound to the catalog recorded by each session.
    ///
    /// # Errors
    /// Returns the registered resource error when the descriptor binding or catalog generation
    /// cannot be loaded.
    pub fn load_sharing_text_pool(
        descriptor: &scorepeek_core::diagnostics::DiagnosticRunDescriptor,
        catalog_root: &Path,
        bundle_root: &Path,
        shared: &Self,
    ) -> Result<Self, RegisteredScreenFieldObserverLoadError> {
        if descriptor.binding.model_sha256 != shared.model_sha256 {
            return Err(RegisteredResourceLoadError::ModelBindingMismatch.into());
        }
        if descriptor.binding.runtime_sha256 != shared.runtime_sha256 {
            return Err(RegisteredResourceLoadError::RuntimeBindingMismatch.into());
        }
        verify_title_evidence_manifest()?;
        let resources = RegisteredRecognitionResources::load(
            catalog_root,
            bundle_root,
            &descriptor.binding.catalog_sha256,
            &descriptor.binding.model_sha256,
            &descriptor.binding.runtime_sha256,
        )?;
        let (catalog, _unused_title_runtime) = resources.into_catalog_and_title_runtime();
        Ok(Self {
            catalog_sha256: descriptor.binding.catalog_sha256.clone(),
            model_sha256: descriptor.binding.model_sha256.clone(),
            runtime_sha256: descriptor.binding.runtime_sha256.clone(),
            catalog,
            text_pool: Arc::clone(&shared.text_pool),
        })
    }

    #[must_use]
    pub fn text_workers(&self) -> usize {
        self.text_pool.worker_count()
    }

    pub(crate) fn observer(
        &self,
        binding: &super::field_observer::FieldObserverSessionBinding,
        numeric_runtime: RegisteredNumericRuntime,
    ) -> Result<RegisteredScreenFieldObserver, RegisteredScreenFieldObserverLoadError> {
        if binding.catalog_sha256() != self.catalog_sha256
            || binding.model_sha256() != self.model_sha256
            || binding.runtime_sha256() != self.runtime_sha256
        {
            return Err(RegisteredResourceLoadError::CatalogBindingMismatch.into());
        }
        RegisteredScreenFieldObserver::from_shared(self, numeric_runtime)
    }
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
        let normalized_text =
            scorepeek_core::recognition::title::normalized_title_key(&selected.open_text);
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

impl FieldObserver for RegisteredScreenFieldObserver {
    type Output =
        Result<RegisteredScreenFieldObservation, ScreenFieldObservationError<OnnxParityError>>;

    const PIPELINED_PREFETCH: bool = true;

    fn outer_worker_count(&self, maximum_outstanding: usize) -> usize {
        let maximum = match self.text_pool.execution_mode() {
            RecognitionExecutionMode::Live => 2,
            RecognitionExecutionMode::Offline => 4,
        };
        maximum_outstanding.min(maximum)
    }

    fn fork_outer_worker(&self) -> Option<Self> {
        Some(Self {
            catalog: self.catalog.clone(),
            text_pool: Arc::clone(&self.text_pool),
            numeric_worker: Arc::clone(&self.numeric_worker),
            candidate_domain: self.candidate_domain.clone(),
            prefetched_text: Arc::clone(&self.prefetched_text),
            prefetched_numeric: Arc::clone(&self.prefetched_numeric),
            projection_cache: VecDeque::new(),
        })
    }

    fn admission(&self) -> Option<FieldObserverAdmission<Self::Output>> {
        let text_pool = Arc::clone(&self.text_pool);
        let numeric_worker = Arc::clone(&self.numeric_worker);
        let prefetched_text = Arc::clone(&self.prefetched_text);
        let prefetched_numeric = Arc::clone(&self.prefetched_numeric);
        Some(Arc::new(move |input| {
            submit_fields(
                &text_pool,
                &numeric_worker,
                &prefetched_text,
                &prefetched_numeric,
                input,
            )
            .err()
            .map(Err)
        }))
    }

    fn prefetch(&mut self, input: &FieldObserverInput) -> Option<Self::Output> {
        self.prefetch_fields(input).err().map(Err)
    }

    fn observe(&mut self, input: &FieldObserverInput) -> Self::Output {
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
    use scorepeek_core::catalog::{Catalog, Difficulty};
    use scorepeek_core::recognition::music_select::{
        MusicSelectScreenFieldObservations, MusicSelectSongResolution, MusicSelectSongUnknownReason,
    };
    use scorepeek_core::recognition::result::{
        ResultSongResolution, ResultSongUnknownReason, resolve_clear_type,
    };
    use scorepeek_core::recognition::screen::ResultScreenFieldObservations;
    use scorepeek_core::recognition::title::DynamicTextObservation;

    use super::*;

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
        use scorepeek_core::recognition::music_select::{
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
