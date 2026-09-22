//! Sequential registered field observation for private canonical replay.
//!
//! Core owns the OCR, numeric, catalog projection and domain types. This adapter owns only
//! resource lifetime and sequential calls; it has no runtime worker or diagnostic binding.

use scorepeek_core::model::session::{
    ProjectedScreenFieldObservation, RecognitionProcessingTiming, RegisteredScreenFieldObservation,
    TitleEvidenceObservation,
};
use scorepeek_core::recognition::music_select::{
    BestNumericObservation, MusicSelectScreenFieldObservations, observe_music_select_difficulty,
    observe_music_select_play_side, observe_music_select_play_type, resolve_music_select_best,
};
use scorepeek_core::recognition::result::numeric::RegisteredNumericRuntime;
use scorepeek_core::recognition::result::{
    PlayOptionsObservation, observe_play_options, observed_result_difficulty,
};
use scorepeek_core::recognition::screen::{
    ScreenFieldObservations, ScreenRgb8Crops, ScreenTextField, TitleEvidenceExtractor,
    observe_result_fields_with_numeric,
};
use scorepeek_core::recognition::shared::CatalogCandidateDomain;
use scorepeek_core::recognition::title::{DynamicTextObservation, normalized_title_key};
use scorepeek_resources::recognition::RegisteredRecognitionResources;

const TITLE_EVIDENCE_MANIFEST: &[u8] =
    include_bytes!("../../../models/manifests/title-evidence-runtime-v2.json");
const TITLE_EVIDENCE_MANIFEST_SHA256: &str =
    "1be327a2b18c3fa1274c167b2f267e9bb6a5c6b076a843818e49f3980db9642b";

pub(crate) struct RegisteredFieldObserver {
    resources: RegisteredRecognitionResources,
    numeric: RegisteredNumericRuntime,
    domain: CatalogCandidateDomain,
}

impl RegisteredFieldObserver {
    pub(crate) fn new(resources: RegisteredRecognitionResources) -> Result<Self, String> {
        if crate::resources::sha256(TITLE_EVIDENCE_MANIFEST) != TITLE_EVIDENCE_MANIFEST_SHA256 {
            return Err("registered title evidence manifest differs".into());
        }
        let domain = CatalogCandidateDomain::from_catalog(resources.catalog())
            .map_err(|error| error.to_string())?;
        let numeric =
            RegisteredNumericRuntime::load_embedded().map_err(|error| error.to_string())?;
        Ok(Self {
            resources,
            numeric,
            domain,
        })
    }

    fn text(
        &mut self,
        crop: &scorepeek_core::recognition::screen::Rgb8Crop,
    ) -> Result<DynamicTextObservation, String> {
        self.resources
            .title_runtime()
            .observe_open_text(crop)
            .map_err(|error| error.to_string())
    }

    #[allow(
        clippy::too_many_lines,
        reason = "each registered screen field is observed once in source order"
    )]
    pub(crate) fn observe(
        &mut self,
        crops: &ScreenRgb8Crops,
    ) -> Result<RegisteredScreenFieldObservation, String> {
        let (fields, numeric_batch, title_evidence) = match crops {
            ScreenRgb8Crops::Title(_) => {
                return Err("recorded game-version state replaces TITLE OCR in replay".into());
            }
            ScreenRgb8Crops::Result(result) => {
                let difficulty = self.text(&result.difficulty)?;
                let mut numeric = self
                    .numeric
                    .observe(result)
                    .map_err(|error| error.to_string())?;
                numeric
                    .join_level(observed_result_difficulty(&difficulty))
                    .map_err(|error| error.to_string())?;
                let mut fields =
                    observe_result_fields_with_numeric(result, &numeric, |field, crop| {
                        if field == ScreenTextField::ResultDifficulty {
                            Ok(difficulty.clone())
                        } else {
                            self.text(crop)
                        }
                    })
                    .map_err(|error| error.to_string())?;
                fields.play_options = self.text(&result.play_options).map_or_else(
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
                let full = self.text(&select.active_list_title)?;
                let foreground_observation = foreground
                    .as_ref()
                    .map(|(crop, _)| self.text(crop))
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
                let header = self.text(&select.best.header).map_or_else(
                    |error| {
                        failures.push(format!(
                            "{:?}: {error}",
                            ScreenTextField::MusicSelectBestHeader
                        ));
                        String::new()
                    },
                    |value| value.open_text,
                );
                let clear = self.text(&select.best.clear_type).map_or_else(
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
                    artist: self.text(&select.artist)?,
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
            self.resources.catalog(),
            fields,
            title_evidence,
        );
        Ok(projected.complete(numeric_batch, RecognitionProcessingTiming::unmeasured(0)))
    }
}
