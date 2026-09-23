//! Registered field observation from resolved immutable recognition resources.

use crate::catalog::Catalog;
use crate::model::session::{
    ProjectedScreenFieldObservation, RecognitionProcessingTiming, RegisteredScreenFieldObservation,
    TitleEvidenceObservation,
};
use crate::recognition::music_select::{
    BestNumericObservation, MusicSelectScreenFieldObservations, observe_music_select_difficulty,
    observe_music_select_play_side, observe_music_select_play_type, resolve_music_select_best,
};
use crate::recognition::result::numeric::RegisteredNumericRuntime;
use crate::recognition::result::{
    PlayOptionsObservation, observe_play_options, observed_result_difficulty,
};
use crate::recognition::screen::{
    Rgb8Crop, ScreenFieldObservations, ScreenRgb8Crops, ScreenTextField, TitleEvidenceExtractor,
    observe_result_fields_with_numeric,
};
use crate::recognition::shared::CatalogCandidateDomain;
use crate::recognition::text_observer_pool::{
    RecognitionExecutionMode, RegisteredTextRecognitionSession, TextRecognitionResult,
};
use crate::recognition::title::RegisteredDynamicTitleRuntime;
use crate::recognition::title::{DynamicTextObservation, normalized_title_key};
use sha2::{Digest as _, Sha256};
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;

const TITLE_EVIDENCE_MANIFEST: &[u8] =
    include_bytes!("../../../../models/manifests/title-evidence-runtime-v2.json");
const TITLE_EVIDENCE_MANIFEST_SHA256: &str =
    "1be327a2b18c3fa1274c167b2f267e9bb6a5c6b076a843818e49f3980db9642b";

fn take_text(
    batch: &mut TextRecognitionResult,
    field: ScreenTextField,
) -> Result<DynamicTextObservation, String> {
    let index = batch
        .observations
        .iter()
        .position(|(candidate, _)| *candidate == field)
        .ok_or_else(|| format!("OCR worker returned no observation for {field:?}"))?;
    batch
        .observations
        .remove(index)
        .1
        .map_err(|error| error.to_string())
}

pub struct RegisteredFieldObserver {
    catalog: Catalog,
    text_pool: Arc<RegisteredTextRecognitionSession>,
    numeric: RegisteredNumericRuntime,
    domain: CatalogCandidateDomain,
}

impl RegisteredFieldObserver {
    /// Builds an observer from resolved catalog and a shared core text pool.
    ///
    /// # Errors
    /// Returns a manifest, catalog-domain, or numeric-model initialization error.
    pub fn new(
        catalog: Catalog,
        text_pool: Arc<RegisteredTextRecognitionSession>,
    ) -> Result<Self, String> {
        let mut digest = String::with_capacity(64);
        for byte in Sha256::digest(TITLE_EVIDENCE_MANIFEST) {
            write!(&mut digest, "{byte:02x}").expect("writing to String cannot fail");
        }
        if digest != TITLE_EVIDENCE_MANIFEST_SHA256 {
            return Err("registered title evidence manifest differs".into());
        }
        let domain =
            CatalogCandidateDomain::from_catalog(&catalog).map_err(|error| error.to_string())?;
        let numeric =
            RegisteredNumericRuntime::load_embedded().map_err(|error| error.to_string())?;
        Ok(Self {
            catalog,
            text_pool,
            numeric,
            domain,
        })
    }

    fn text_batch(
        &self,
        jobs: Vec<(ScreenTextField, Rgb8Crop)>,
    ) -> Result<TextRecognitionResult, String> {
        let pending = self
            .text_pool
            .submit(jobs)
            .map_err(|error| error.to_string())?;
        pending.join().map_err(|error| error.to_string())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "each registered screen field is observed once in source order"
    )]
    /// Observes one bounded set of screen crops.
    ///
    /// # Errors
    /// Returns the field or model observation failure.
    pub fn observe(
        &mut self,
        crops: &ScreenRgb8Crops,
    ) -> Result<RegisteredScreenFieldObservation, String> {
        let (fields, numeric_batch, title_evidence) = match crops {
            ScreenRgb8Crops::Title(_) => {
                return Err("recorded game-version state replaces TITLE OCR in replay".into());
            }
            ScreenRgb8Crops::Result(result) => {
                let mut text = self.text_batch(vec![
                    (ScreenTextField::ResultDifficulty, result.difficulty.clone()),
                    (ScreenTextField::ResultPlayType, result.play_type.clone()),
                    (ScreenTextField::ResultTitle, result.title.clone()),
                    (ScreenTextField::ResultArtist, result.artist.clone()),
                    (ScreenTextField::ResultClearType, result.clear_type.clone()),
                    (
                        ScreenTextField::ResultPreviousClearType,
                        result.previous_clear_type.clone(),
                    ),
                    (
                        ScreenTextField::ResultPlayOptions,
                        result.play_options.clone(),
                    ),
                ])?;
                let difficulty = take_text(&mut text, ScreenTextField::ResultDifficulty)?;
                let mut numeric = self
                    .numeric
                    .observe(result)
                    .map_err(|error| error.to_string())?;
                numeric
                    .join_level(observed_result_difficulty(&difficulty))
                    .map_err(|error| error.to_string())?;
                let mut fields =
                    observe_result_fields_with_numeric(result, &numeric, |field, _crop| {
                        if field == ScreenTextField::ResultDifficulty {
                            Ok(difficulty.clone())
                        } else {
                            take_text(&mut text, field)
                        }
                    })
                    .map_err(|error| error.to_string())?;
                fields.play_options = take_text(&mut text, ScreenTextField::ResultPlayOptions)
                    .map_or_else(
                        |_| PlayOptionsObservation::failed(&result.play_options),
                        |options| observe_play_options(&result.play_options, &options),
                    );
                (ScreenFieldObservations::Result(fields), Some(numeric), None)
            }
            ScreenRgb8Crops::MusicSelect(select) => {
                let play_type = observe_music_select_play_type(&select.play_type)
                    .map_err(|error| format!("music select play type: {error:?}"))?;
                let foreground =
                    TitleEvidenceExtractor::REGISTERED.extract(&select.active_list_title);
                let mut jobs = vec![
                    (
                        ScreenTextField::MusicSelectBestHeader,
                        select.best.header.clone(),
                    ),
                    (
                        ScreenTextField::MusicSelectBestClearType,
                        select.best.clear_type.clone(),
                    ),
                    (ScreenTextField::MusicSelectArtist, select.artist.clone()),
                    (
                        ScreenTextField::MusicSelectActiveListTitle,
                        select.active_list_title.clone(),
                    ),
                ];
                if let Some((crop, _)) = &foreground {
                    jobs.push((ScreenTextField::MusicSelectActiveListTitle, crop.clone()));
                }
                let mut text = self.text_batch(jobs)?;
                let full = take_text(&mut text, ScreenTextField::MusicSelectActiveListTitle)?;
                let foreground_observation = foreground
                    .as_ref()
                    .map(|_| take_text(&mut text, ScreenTextField::MusicSelectActiveListTitle))
                    .transpose()?;
                let selected = foreground_observation.clone().unwrap_or_default();
                let normalized_text = normalized_title_key(&selected.open_text);
                let title_evidence = TitleEvidenceObservation {
                    extractor_id: "scorepeek-active-title-gray80-bbox-x4-full-y-v1",
                    runtime_manifest_sha256: TITLE_EVIDENCE_MANIFEST_SHA256,
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
                let mut failures = Vec::new();
                let numeric = self
                    .numeric
                    .observe_music_select_best(&select.best)
                    .unwrap_or_else(|error| {
                        failures.push(format!("numeric: {error}"));
                        BestNumericObservation::default()
                    });
                let header = take_text(&mut text, ScreenTextField::MusicSelectBestHeader)
                    .map_or_else(
                        |error| {
                            failures.push(format!(
                                "{:?}: {error}",
                                ScreenTextField::MusicSelectBestHeader
                            ));
                            String::new()
                        },
                        |value| value.open_text,
                    );
                let clear = take_text(&mut text, ScreenTextField::MusicSelectBestClearType)
                    .map_or_else(
                        |error| {
                            failures.push(format!(
                                "{:?}: {error}",
                                ScreenTextField::MusicSelectBestClearType
                            ));
                            String::new()
                        },
                        |value| value.open_text,
                    );
                let mut best = resolve_music_select_best(header, clear, numeric);
                best.failures = failures;
                let fields = MusicSelectScreenFieldObservations {
                    best,
                    central_title: DynamicTextObservation::default(),
                    artist: take_text(&mut text, ScreenTextField::MusicSelectArtist)?,
                    play_type,
                    selected_difficulty: observe_music_select_difficulty(
                        &select.difficulty_markers,
                    ),
                    play_side: observe_music_select_play_side(&select.play_side),
                    active_list_title: selected,
                };
                (
                    ScreenFieldObservations::MusicSelect(fields),
                    None,
                    Some(title_evidence),
                )
            }
        };
        let projected = ProjectedScreenFieldObservation::project(
            &self.domain,
            &self.catalog,
            fields,
            title_evidence,
        );
        Ok(projected.complete(numeric_batch, RecognitionProcessingTiming::unmeasured(0)))
    }
}

const MAX_PENDING_FRAMES_PER_WORKER: usize = 8;

struct FieldJob {
    crops: ScreenRgb8Crops,
    response: SyncSender<Result<RegisteredScreenFieldObservation, String>>,
}

/// Bounded frame-level recognition scheduling over the same core OCR pool used live.
pub struct RegisteredFieldPool {
    senders: Vec<SyncSender<Option<FieldJob>>>,
    handles: Vec<thread::JoinHandle<()>>,
    cursor: usize,
}

impl RegisteredFieldPool {
    /// Starts bounded field workers from resolved immutable resources.
    ///
    /// # Errors
    /// Returns the OCR initialization or thread startup error.
    pub fn start(
        catalog: &Catalog,
        title_runtime: RegisteredDynamicTitleRuntime,
    ) -> Result<Self, String> {
        let text_pool = Arc::new(
            RegisteredTextRecognitionSession::start(
                title_runtime,
                RecognitionExecutionMode::Offline,
            )
            .map_err(|error| error.to_string())?,
        );
        let worker_count = thread::available_parallelism()
            .map_or(1, usize::from)
            .div_ceil(4)
            .clamp(1, 4);
        let mut senders: Vec<SyncSender<Option<FieldJob>>> = Vec::with_capacity(worker_count);
        let mut handles = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (sender, receiver) =
                mpsc::sync_channel::<Option<FieldJob>>(MAX_PENDING_FRAMES_PER_WORKER);
            let catalog = catalog.clone();
            let text_pool = Arc::clone(&text_pool);
            let worker = thread::Builder::new()
                .name(format!("scorepeek-field-observer-{index}"))
                .spawn(move || {
                    let mut observer = RegisteredFieldObserver::new(catalog, text_pool);
                    while let Ok(Some(job)) = receiver.recv() {
                        let result = observer
                            .as_mut()
                            .map_err(|error| error.clone())
                            .and_then(|observer| observer.observe(&job.crops));
                        let _ = job.response.send(result);
                    }
                })
                .map_err(|error| error.to_string())?;
            senders.push(sender);
            handles.push(worker);
        }
        Ok(Self {
            senders,
            handles,
            cursor: 0,
        })
    }

    /// Submits one frame. A full bounded queue applies backpressure.
    ///
    /// # Errors
    /// Returns an error if a worker has stopped.
    pub fn submit(&mut self, crops: ScreenRgb8Crops) -> Result<PendingFieldRecognition, String> {
        let (response, receiver) = mpsc::sync_channel(1);
        let index = self.cursor % self.senders.len();
        self.cursor += 1;
        self.senders[index]
            .send(Some(FieldJob { crops, response }))
            .map_err(|_| "field observer worker stopped".to_owned())?;
        Ok(PendingFieldRecognition { receiver })
    }
}

impl Drop for RegisteredFieldPool {
    fn drop(&mut self) {
        for sender in &self.senders {
            let _ = sender.send(None);
        }
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

/// One pending bounded field observation; joining preserves submission order at the caller.
pub struct PendingFieldRecognition {
    receiver: Receiver<Result<RegisteredScreenFieldObservation, String>>,
}

impl PendingFieldRecognition {
    /// Waits for the observation.
    ///
    /// # Errors
    /// Returns the recognition failure or worker failure.
    pub fn join(self) -> Result<RegisteredScreenFieldObservation, String> {
        self.receiver
            .recv()
            .map_err(|_| "field observer worker stopped".to_owned())?
    }
}
