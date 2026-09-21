//! Portable projection of recognized screen fields into product observations.

use std::collections::BTreeMap;
use std::ops::Deref;
use std::time::Instant;

use serde::Serialize;

use crate::catalog::{Catalog, DisplayVariantKind, ScorepeekSongId};
use crate::recognition::{
    CatalogCandidateDomain, EvidenceFamily, JointEvidenceCandidate, JointEvidenceObservation,
    MusicSelectSongResolution, NumericBatchInference, ParsedResultFields, ResultChartResolution,
    ResultPerformanceResolution, ResultSongResolution, ScreenCatalogCandidateObservations,
    ScreenFieldObservations, ScreenSongResolution, assist_unknown_result_song_with_chart,
    matching_observed_chart_songs, resolve_clear_type, resolve_music_select_song,
    resolve_result_chart, resolve_result_performance, resolve_result_song,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecognitionFrameTiming {
    pub screen_classification_us: u64,
    pub crop_prepare_us: Option<u64>,
    pub screen_resolver_us: Option<u64>,
    pub attempt_resolver_us: Option<u64>,
    pub output_us: Option<u64>,
    pub frame_processing_wall_us: u64,
}

fn duration_us(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
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
    processing_timing: RecognitionProcessingTiming,
    joint_evidence: JointEvidenceObservation,
    title_evidence: Option<TitleEvidenceObservation>,
}

/// Catalog projection awaiting worker processing metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedScreenFieldObservation {
    observation: RegisteredScreenFieldObservation,
}

/// Completed observation with end-to-end frame timing attached.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameTimedScreenFieldObservation {
    observation: RegisteredScreenFieldObservation,
}

impl Deref for FrameTimedScreenFieldObservation {
    type Target = RegisteredScreenFieldObservation;

    fn deref(&self) -> &Self::Target {
        &self.observation
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TitleEvidenceObservation {
    pub extractor_id: &'static str,
    pub runtime_manifest_sha256: &'static str,
    pub selected_view: &'static str,
    pub full: crate::recognition::DynamicTextObservation,
    pub foreground: Option<crate::recognition::DynamicTextObservation>,
    pub normalized_text: String,
    pub normalized_scalar_count: usize,
    pub geometry: Option<crate::recognition::TitleForegroundGeometry>,
    pub mask_absent: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecognitionProcessingTiming {
    pub execution_policy: &'static str,
    pub available_parallelism: usize,
    pub text_workers: usize,
    pub frame_total_us: u64,
    pub field_queue_wait_us: u64,
    pub text_batch_wall_us: u64,
    pub maximum_text_worker_queue_wait_us: u64,
    pub maximum_text_worker_inference_us: u64,
    pub text_worker_busy_us: u64,
    pub text_worker_ids: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numeric_recognition_us: Option<u64>,
    pub join_us: u64,
    pub catalog_evidence_us: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_classification_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crop_prepare_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_resolver_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_finalization_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_us: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_end_to_end_wall_us: Option<u64>,
}

impl RecognitionProcessingTiming {
    #[must_use]
    pub fn unmeasured(catalog_evidence_us: u64) -> Self {
        Self {
            execution_policy: "test",
            available_parallelism: 1,
            text_workers: 1,
            frame_total_us: 0,
            field_queue_wait_us: 0,
            text_batch_wall_us: 0,
            maximum_text_worker_queue_wait_us: 0,
            maximum_text_worker_inference_us: 0,
            text_worker_busy_us: 0,
            text_worker_ids: Vec::new(),
            numeric_recognition_us: None,
            join_us: 0,
            catalog_evidence_us,
            screen_classification_us: None,
            crop_prepare_us: None,
            screen_resolver_us: None,
            attempt_finalization_us: None,
            output_us: None,
            frame_end_to_end_wall_us: None,
        }
    }
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

impl From<&crate::recognition::DynamicTextObservation> for CurrentScoreOcrAttempt {
    fn from(value: &crate::recognition::DynamicTextObservation) -> Self {
        Self {
            input_width: value.input_width,
            output_timesteps: value.output_timesteps,
            open_text: value.open_text.clone(),
            constrained_text: value.constrained_text.clone(),
        }
    }
}

impl ProjectedScreenFieldObservation {
    #[must_use]
    pub fn project(
        candidate_domain: &CatalogCandidateDomain,
        catalog: &Catalog,
        fields: ScreenFieldObservations,
        title_evidence: Option<TitleEvidenceObservation>,
    ) -> Self {
        let catalog_started = Instant::now();
        let candidates = candidate_domain.observe(&fields);
        let catalog_evidence_us = duration_us(catalog_started.elapsed());
        let parsed_result_fields = match &fields {
            ScreenFieldObservations::Result(fields) => {
                Some(ParsedResultFields::from_observations(fields))
            }
            ScreenFieldObservations::Title(_) | ScreenFieldObservations::MusicSelect(_) => None,
        };
        let song_resolution = match (&fields, &candidates) {
            (
                ScreenFieldObservations::Title(_),
                ScreenCatalogCandidateObservations::Title { .. },
            ) => ScreenSongResolution::Title,
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
                        matching_observed_chart_songs(catalog, parsed)
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
            ScreenFieldObservations::Title(_) | ScreenFieldObservations::MusicSelect(_) => None,
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
        let joint_evidence = joint_evidence(catalog, &candidates, &fields, title_evidence.as_ref());
        Self {
            observation: RegisteredScreenFieldObservation {
                fields,
                candidates,
                song_resolution,
                clear_type,
                parsed_result_fields,
                result_chart_resolution,
                result_performance_resolution,
                current_score_ocr_resolution: None,
                numeric_batch: None,
                processing_timing: RecognitionProcessingTiming::unmeasured(catalog_evidence_us),
                joint_evidence,
                title_evidence,
            },
        }
    }

    #[must_use]
    pub const fn catalog_evidence_us(&self) -> u64 {
        self.observation.processing_timing.catalog_evidence_us
    }

    #[must_use]
    pub fn with_catalog_evidence_timing(mut self, catalog_evidence_us: u64) -> Self {
        self.observation.processing_timing =
            RecognitionProcessingTiming::unmeasured(catalog_evidence_us);
        self
    }

    #[must_use]
    pub fn complete(
        mut self,
        numeric_batch: Option<NumericBatchInference>,
        timing: RecognitionProcessingTiming,
    ) -> RegisteredScreenFieldObservation {
        self.observation.numeric_batch = numeric_batch;
        self.observation.processing_timing = timing;
        self.observation
    }
}

impl RegisteredScreenFieldObservation {
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
            ScreenSongResolution::Title | ScreenSongResolution::MusicSelect(_) => None,
        }
    }

    #[must_use]
    pub const fn music_select_resolution(&self) -> Option<&MusicSelectSongResolution> {
        match &self.song_resolution {
            ScreenSongResolution::Title | ScreenSongResolution::Result(_) => None,
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

    #[must_use]
    pub const fn processing_timing(&self) -> &RecognitionProcessingTiming {
        &self.processing_timing
    }

    #[must_use]
    pub fn with_frame_timing(
        mut self,
        timing: RecognitionFrameTiming,
    ) -> FrameTimedScreenFieldObservation {
        self.processing_timing.screen_classification_us = Some(timing.screen_classification_us);
        self.processing_timing.crop_prepare_us = timing.crop_prepare_us;
        self.processing_timing.screen_resolver_us = timing.screen_resolver_us;
        self.processing_timing.attempt_finalization_us = timing.attempt_resolver_us;
        self.processing_timing.output_us = timing.output_us;
        self.processing_timing.frame_end_to_end_wall_us = Some(timing.frame_processing_wall_us);
        FrameTimedScreenFieldObservation { observation: self }
    }

    #[must_use]
    pub const fn joint_evidence(&self) -> &JointEvidenceObservation {
        &self.joint_evidence
    }

    #[must_use]
    pub const fn title_evidence(&self) -> Option<&TitleEvidenceObservation> {
        self.title_evidence.as_ref()
    }
}

fn joint_evidence(
    catalog: &Catalog,
    candidates: &ScreenCatalogCandidateObservations,
    fields: &ScreenFieldObservations,
    title_evidence: Option<&TitleEvidenceObservation>,
) -> JointEvidenceObservation {
    let mut ranked = Vec::new();
    for (song_id, title_support, artist_support) in song_supports(candidates) {
        let Some(song) = catalog.songs().get(&song_id) else {
            continue;
        };
        for chart in song.charts().values() {
            let mut family_support = BTreeMap::new();
            match fields {
                ScreenFieldObservations::Title(_) => {}
                ScreenFieldObservations::Result(_) => {
                    family_support.insert(EvidenceFamily::ResultTitle, title_support);
                    family_support.insert(EvidenceFamily::ResultArtist, artist_support);
                }
                ScreenFieldObservations::MusicSelect(_) => {
                    family_support.insert(
                        EvidenceFamily::SelectTitle,
                        title_support.max(structural_title_support(song, title_evidence)),
                    );
                    family_support.insert(EvidenceFamily::SelectArtist, artist_support);
                }
            }
            let support = family_support.values().copied().sum();
            if support == 0 {
                continue;
            }
            ranked.push(JointEvidenceCandidate {
                song_id,
                chart: chart.clone(),
                display_titles: song
                    .title_variants()
                    .iter()
                    .map(|variant| variant.value.clone())
                    .collect(),
                artist: song.artist().to_owned(),
                family_support,
                support,
            });
        }
    }
    ranked.sort_by(|left, right| {
        right
            .support
            .cmp(&left.support)
            .then_with(|| left.song_id.cmp(&right.song_id))
            .then_with(|| left.chart.key.cmp(&right.chart.key))
    });
    JointEvidenceObservation {
        catalog_song_count: catalog.songs().len(),
        candidates: ranked,
    }
}

fn song_supports(
    candidates: &ScreenCatalogCandidateObservations,
) -> Vec<(ScorepeekSongId, u16, u16)> {
    match candidates {
        ScreenCatalogCandidateObservations::Title { .. } => Vec::new(),
        ScreenCatalogCandidateObservations::Result { candidates, .. } => candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.song_id,
                    text_support(candidate.title),
                    text_support(candidate.artist),
                )
            })
            .collect(),
        ScreenCatalogCandidateObservations::MusicSelect { candidates, .. } => candidates
            .iter()
            .map(|candidate| {
                let title = [
                    text_support(candidate.central_title),
                    text_support(candidate.active_list_title),
                    prefix_support(candidate.active_list_title_prefix),
                ]
                .into_iter()
                .max()
                .unwrap_or(0);
                (candidate.song_id, title, text_support(candidate.artist))
            })
            .collect(),
    }
}

fn structural_title_support(
    song: &crate::catalog::CatalogSong,
    evidence: Option<&TitleEvidenceObservation>,
) -> u16 {
    let Some(evidence) = evidence else { return 0 };
    let Some(geometry) = evidence.geometry else {
        return 0;
    };
    let count = evidence.normalized_scalar_count;
    if count == 0 || geometry.touches_left_edge || geometry.touches_right_edge {
        return 0;
    }
    let width_per_character =
        geometry.occupancy_width_ppm / u32::try_from(count).unwrap_or(u32::MAX).max(1);
    if !(5_000..=250_000).contains(&width_per_character) {
        return 0;
    }
    u16::from(song.title_variants().iter().any(|variant| {
        variant.kind != DisplayVariantKind::SearchTerm
            && crate::recognition::normalized_title_key(&variant.value)
                .chars()
                .count()
                == count
    })) * 60
}

fn text_support(score: crate::recognition::CatalogTextCandidateScore) -> u16 {
    similarity_support(
        score.minimum_edit_distance,
        score.maximum_normalized_similarity.matching_units,
        score.maximum_normalized_similarity.compared_units,
    )
}

fn prefix_support(score: crate::recognition::CatalogPrefixCandidateScore) -> u16 {
    similarity_support(
        score.minimum_edit_distance,
        score.maximum_normalized_similarity.matching_units,
        score.maximum_normalized_similarity.compared_units,
    )
}

fn similarity_support(edit: usize, matching: usize, compared: usize) -> u16 {
    if compared == 0 {
        return 0;
    }
    if edit == 0 {
        return 300;
    }
    let percentage = matching.saturating_mul(100) / compared;
    match percentage {
        90..=usize::MAX => 70,
        75..=89 => 35,
        60..=74 => 15,
        _ => 0,
    }
}
