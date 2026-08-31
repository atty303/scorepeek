use scorepeek::catalog::Catalog;
use scorepeek::recognition::{
    CatalogCandidateDomain, CatalogCandidateDomainError, MusicSelectSongResolution,
    NumericBatchInference, OnnxParityError, ParsedResultFields, RegisteredNumericRuntime,
    RegisteredRecognitionResources, RegisteredResourceLoadError, ResultChartResolution,
    ResultPerformanceResolution, ResultSongResolution, ScreenCatalogCandidateObservations,
    ScreenFieldObservationError, ScreenFieldObservations, ScreenSongResolution,
    assist_unknown_result_song_with_chart, matching_single_play_songs,
    observe_result_fields_with_numeric, observe_screen_fields, observed_result_difficulty,
    resolve_clear_type, resolve_music_select_song, resolve_result_chart,
    resolve_result_performance, resolve_result_song,
};
use serde::Serialize;
use std::error::Error;
use std::fmt;

use super::field_observer::{FieldObserver, FieldObserverInput};

/// Production screen-field observer owning the exact resources for one immutable run.
pub struct RegisteredScreenFieldObserver {
    resources: RegisteredRecognitionResources,
    numeric_runtime: RegisteredNumericRuntime,
    candidate_domain: CatalogCandidateDomain,
}

#[derive(Debug)]
pub enum RegisteredScreenFieldObserverLoadError {
    Resources(RegisteredResourceLoadError),
    NumericModel(scorepeek::numeric_model_store::NumericModelStoreError),
    CandidateDomain(CatalogCandidateDomainError),
}

impl fmt::Display for RegisteredScreenFieldObserverLoadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resources(error) => error.fmt(formatter),
            Self::NumericModel(error) => error.fmt(formatter),
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

impl From<scorepeek::numeric_model_store::NumericModelStoreError>
    for RegisteredScreenFieldObserverLoadError
{
    fn from(error: scorepeek::numeric_model_store::NumericModelStoreError) -> Self {
        Self::NumericModel(error)
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
    ) -> Result<Self, CatalogCandidateDomainError> {
        let candidate_domain = CatalogCandidateDomain::from_catalog(resources.catalog())?;
        Ok(Self {
            resources,
            numeric_runtime,
            candidate_domain,
        })
    }
}

/// Complete registered field inference and full-catalog evidence for one classified screen.
#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredScreenFieldObservation {
    fields: ScreenFieldObservations,
    candidates: ScreenCatalogCandidateObservations,
    song_resolution: ScreenSongResolution,
    clear_type: Option<&'static str>,
    parsed_result_fields: Option<ParsedResultFields>,
    result_chart_resolution: Option<ResultChartResolution>,
    result_performance_resolution: Option<ResultPerformanceResolution>,
    current_score_ocr_resolution: Option<CurrentScoreOcrResolution>,
    numeric_batch: Option<NumericBatchInference>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentScoreOcrSelection {
    Primary,
    CyanRetry,
    CyanRetryTrailingEight,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CurrentScoreOcrResolution {
    pub primary: CurrentScoreOcrAttempt,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cyan_retry: Option<CurrentScoreOcrAttempt>,
    pub selection: CurrentScoreOcrSelection,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CurrentScoreOcrAttempt {
    pub input_width: usize,
    pub output_timesteps: usize,
    pub open_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub constrained_text: Option<String>,
}

impl From<&scorepeek::recognition::DynamicTextObservation> for CurrentScoreOcrAttempt {
    fn from(value: &scorepeek::recognition::DynamicTextObservation) -> Self {
        Self {
            input_width: value.input_width,
            output_timesteps: value.output_timesteps,
            open_text: value.open_text.clone(),
            constrained_text: value.constrained_text.clone(),
        }
    }
}

impl RegisteredScreenFieldObservation {
    #[cfg(test)]
    pub(crate) fn from_fields(
        candidate_domain: &CatalogCandidateDomain,
        fields: ScreenFieldObservations,
    ) -> Self {
        Self::from_fields_with_catalog(candidate_domain, &Catalog::default(), fields)
    }

    pub(crate) fn from_fields_with_catalog(
        candidate_domain: &CatalogCandidateDomain,
        catalog: &Catalog,
        fields: ScreenFieldObservations,
    ) -> Self {
        let candidates = candidate_domain.observe(&fields);
        let parsed_result_fields = match &fields {
            ScreenFieldObservations::Result(fields) => {
                Some(ParsedResultFields::from_observations(fields))
            }
            ScreenFieldObservations::MusicSelect(_) => None,
        };
        let song_resolution = match (&fields, &candidates) {
            (
                ScreenFieldObservations::Result(fields),
                ScreenCatalogCandidateObservations::Result { candidates, .. },
            ) => {
                let primary = resolve_result_song(
                    &fields.title.open_text,
                    &fields.artist.open_text,
                    candidates,
                );
                let matching_song_ids = parsed_result_fields
                    .as_ref()
                    .map_or_else(Vec::new, |parsed| {
                        matching_single_play_songs(catalog, parsed)
                    });
                ScreenSongResolution::Result(assist_unknown_result_song_with_chart(
                    primary,
                    &matching_song_ids,
                ))
            }
            (
                ScreenFieldObservations::MusicSelect(fields),
                ScreenCatalogCandidateObservations::MusicSelect { candidates, .. },
            ) => ScreenSongResolution::MusicSelect(resolve_music_select_song(
                &fields.central_title.open_text,
                &fields.artist.open_text,
                &fields.active_list_title.open_text,
                candidates,
            )),
            _ => unreachable!("field observations and candidates share one screen"),
        };
        let clear_type = match &fields {
            ScreenFieldObservations::Result(fields) => {
                resolve_clear_type(&fields.clear_type.open_text)
            }
            ScreenFieldObservations::MusicSelect(_) => None,
        };
        let result_chart_resolution = match (&song_resolution, &parsed_result_fields) {
            (ScreenSongResolution::Result(resolution), Some(parsed)) => resolution
                .accepted_song_id()
                .map(|song_id| resolve_result_chart(catalog, song_id, parsed)),
            _ => None,
        };
        let result_performance_resolution = match (
            result_chart_resolution.as_ref(),
            parsed_result_fields.as_ref(),
        ) {
            (
                Some(ResultChartResolution::Accepted {
                    chart,
                    current_score,
                    ..
                }),
                Some(parsed),
            ) => Some(resolve_result_performance(
                parsed,
                chart.notes,
                *current_score,
            )),
            _ => None,
        };
        Self {
            fields,
            candidates,
            song_resolution,
            clear_type,
            parsed_result_fields,
            result_chart_resolution,
            result_performance_resolution,
            current_score_ocr_resolution: None,
            numeric_batch: None,
        }
    }

    #[must_use]
    pub const fn fields(&self) -> &ScreenFieldObservations {
        &self.fields
    }

    #[must_use]
    pub const fn candidates(&self) -> &ScreenCatalogCandidateObservations {
        &self.candidates
    }

    #[must_use]
    pub const fn result_resolution(&self) -> Option<&ResultSongResolution> {
        match &self.song_resolution {
            ScreenSongResolution::Result(resolution) => Some(resolution),
            ScreenSongResolution::MusicSelect(_) => None,
        }
    }

    #[must_use]
    pub const fn music_select_resolution(&self) -> Option<&MusicSelectSongResolution> {
        match &self.song_resolution {
            ScreenSongResolution::Result(_) => None,
            ScreenSongResolution::MusicSelect(resolution) => Some(resolution),
        }
    }

    #[must_use]
    pub const fn song_resolution(&self) -> &ScreenSongResolution {
        &self.song_resolution
    }

    #[must_use]
    pub const fn clear_type(&self) -> Option<&'static str> {
        self.clear_type
    }

    #[must_use]
    pub const fn parsed_result_fields(&self) -> Option<&ParsedResultFields> {
        self.parsed_result_fields.as_ref()
    }

    #[must_use]
    pub const fn result_chart_resolution(&self) -> Option<&ResultChartResolution> {
        self.result_chart_resolution.as_ref()
    }

    #[must_use]
    pub const fn result_performance_resolution(&self) -> Option<&ResultPerformanceResolution> {
        self.result_performance_resolution.as_ref()
    }

    #[must_use]
    pub const fn current_score_ocr_resolution(&self) -> Option<&CurrentScoreOcrResolution> {
        self.current_score_ocr_resolution.as_ref()
    }

    #[must_use]
    pub const fn numeric_batch(&self) -> Option<&NumericBatchInference> {
        self.numeric_batch.as_ref()
    }
}

impl FieldObserver for RegisteredScreenFieldObserver {
    type Output =
        Result<RegisteredScreenFieldObservation, ScreenFieldObservationError<OnnxParityError>>;

    fn observe(&mut self, input: &FieldObserverInput) -> Self::Output {
        let (fields, numeric_batch) = match input.crops() {
            scorepeek::recognition::ScreenRgb8Crops::Result(crops) => {
                let difficulty = self
                    .resources
                    .title_runtime()
                    .observe_open_text(&crops.difficulty)
                    .map_err(|source| {
                        ScreenFieldObservationError::new(
                            scorepeek::recognition::ScreenTextField::ResultDifficulty,
                            source,
                        )
                    })?;
                let numeric = self
                    .numeric_runtime
                    .observe(crops, observed_result_difficulty(&difficulty))
                    .map_err(|source| {
                        ScreenFieldObservationError::new(
                            scorepeek::recognition::ScreenTextField::ResultNumericBatch,
                            source,
                        )
                    })?;
                let fields = observe_result_fields_with_numeric(crops, &numeric, |field, crop| {
                    if field == scorepeek::recognition::ScreenTextField::ResultDifficulty {
                        Ok(difficulty.clone())
                    } else {
                        self.resources.title_runtime().observe_open_text(crop)
                    }
                })?;
                (
                    scorepeek::recognition::ScreenFieldObservations::Result(fields),
                    Some(numeric),
                )
            }
            scorepeek::recognition::ScreenRgb8Crops::MusicSelect(_) => (
                observe_screen_fields(input.crops(), |_, crop| {
                    self.resources.title_runtime().observe_open_text(crop)
                })?,
                None,
            ),
        };
        let mut observation = RegisteredScreenFieldObservation::from_fields_with_catalog(
            &self.candidate_domain,
            self.resources.catalog(),
            fields,
        );
        observation.numeric_batch = numeric_batch;
        Ok(observation)
    }
}

#[cfg(test)]
mod tests {
    use scorepeek::catalog::Catalog;
    use scorepeek::recognition::{
        DynamicTextObservation, MusicSelectScreenFieldObservations, MusicSelectSongResolution,
        MusicSelectSongUnknownReason, ResultScreenFieldObservations, ResultSongResolution,
        ResultSongUnknownReason,
    };

    use super::*;

    #[test]
    fn clear_type_resolution_accepts_registered_values_and_display_aliases() {
        assert_eq!(resolve_clear_type("EXH-CLEAR"), Some("EXH-CLEAR"));
        assert_eq!(resolve_clear_type("XH-CLEAR"), Some("EXH-CLEAR"));
        assert_eq!(resolve_clear_type("A-CLEAR"), Some("ASSIST CLEAR"));
        assert_eq!(resolve_clear_type("F-COMBO"), Some("F-COMBO"));
        assert_eq!(resolve_clear_type(""), None);
        assert_eq!(resolve_clear_type("UNRELATED"), None);
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
        let output = RegisteredScreenFieldObservation::from_fields(&domain, fields.clone());

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
            central_title: text("texture"),
            artist: text("artist"),
            selected_chart: text("HYPER 8"),
            active_list_title: text("TITLE"),
        });
        let output = RegisteredScreenFieldObservation::from_fields(&domain, fields.clone());

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
}
