use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::sync::Arc;

use crate::catalog::{Catalog, DisplayVariantKind, ScorepeekSongId};
use serde::Serialize;

use super::title::{
    DIAGNOSTIC_TITLE_COMPARISON_KEY_ID, exact_comparison_key, folded_comparison_key,
};
use super::{
    MusicSelectScreenFieldObservations, ResultScreenFieldObservations, ScreenFieldObservations,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogNormalizedSimilarity {
    pub matching_units: usize,
    pub compared_units: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogTextCandidateScore {
    pub minimum_edit_distance: usize,
    pub maximum_normalized_similarity: CatalogNormalizedSimilarity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogPrefixCandidateScore {
    pub minimum_edit_distance: usize,
    pub maximum_normalized_similarity: CatalogNormalizedSimilarity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResultSongCandidateObservation {
    pub song_id: ScorepeekSongId,
    pub title: CatalogTextCandidateScore,
    pub artist: CatalogTextCandidateScore,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectSongCandidateObservation {
    pub song_id: ScorepeekSongId,
    pub central_title: CatalogTextCandidateScore,
    pub artist: CatalogTextCandidateScore,
    pub active_list_title: CatalogTextCandidateScore,
    pub active_list_title_prefix: CatalogPrefixCandidateScore,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScreenCatalogCandidateObservations {
    Result {
        comparison_key_id: &'static str,
        catalog: Arc<CatalogCandidateEvidenceTable>,
        candidates: Vec<ResultSongCandidateObservation>,
    },
    MusicSelect {
        comparison_key_id: &'static str,
        catalog: Arc<CatalogCandidateEvidenceTable>,
        candidates: Vec<MusicSelectSongCandidateObservation>,
    },
}

impl ScreenCatalogCandidateObservations {
    #[must_use]
    pub const fn comparison_key_id(&self) -> &'static str {
        match self {
            Self::Result {
                comparison_key_id, ..
            }
            | Self::MusicSelect {
                comparison_key_id, ..
            } => comparison_key_id,
        }
    }

    #[must_use]
    pub fn candidate_count(&self) -> usize {
        match self {
            Self::Result { candidates, .. } => candidates.len(),
            Self::MusicSelect { candidates, .. } => candidates.len(),
        }
    }

    #[must_use]
    pub fn catalog_evidence(&self) -> &CatalogCandidateEvidenceTable {
        match self {
            Self::Result { catalog, .. } | Self::MusicSelect { catalog, .. } => catalog,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogCandidateTextEvidence {
    pub display: Vec<String>,
    pub exact: Vec<String>,
    pub folded: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogCandidateSongEvidence {
    pub song_id: ScorepeekSongId,
    pub title: CatalogCandidateTextEvidence,
    pub artist: CatalogCandidateTextEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CatalogCandidateEvidenceTable {
    pub comparison_key_id: &'static str,
    pub songs: Vec<CatalogCandidateSongEvidence>,
}

#[derive(Clone, Debug)]
struct TextCandidateDomain {
    raw_and_exact: Vec<Vec<char>>,
    folded: Vec<Vec<char>>,
    evidence: CatalogCandidateTextEvidence,
}

#[derive(Clone, Debug)]
struct SongCandidateDomain {
    song_id: ScorepeekSongId,
    title: TextCandidateDomain,
    artist: TextCandidateDomain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CatalogCandidateDomainError {
    pub song_id: ScorepeekSongId,
}

impl fmt::Display for CatalogCandidateDomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("catalog song has no non-search title variant")
    }
}

impl Error for CatalogCandidateDomainError {}

/// Immutable full-catalog text comparison domain for screen-local observations.
///
/// This domain computes evidence for every active catalog song. It does not rank candidates,
/// choose a song, accept a field, apply a threshold, or own temporal state.
#[derive(Clone, Debug)]
pub struct CatalogCandidateDomain {
    songs: Vec<SongCandidateDomain>,
    evidence: Arc<CatalogCandidateEvidenceTable>,
}

impl CatalogCandidateDomain {
    /// Builds the complete deterministic comparison domain.
    ///
    /// # Errors
    /// Returns the exact song ID when an admitted catalog song has no non-search title variant.
    pub fn from_catalog(catalog: &Catalog) -> Result<Self, CatalogCandidateDomainError> {
        let title_domains =
            text_candidate_domains(catalog.songs().iter().flat_map(|(song_id, song)| {
                song.title_variants()
                    .iter()
                    .map(move |variant| (*song_id, variant.kind, variant.value.as_str()))
            }));
        let artist_domains =
            text_candidate_domains(catalog.songs().iter().map(|(song_id, song)| {
                (*song_id, DisplayVariantKind::OfficialDisplay, song.artist())
            }));
        let songs: Result<Vec<_>, _> = catalog
            .songs()
            .keys()
            .map(|song_id| {
                let title = title_domains
                    .get(song_id)
                    .cloned()
                    .ok_or(CatalogCandidateDomainError { song_id: *song_id })?;
                Ok(SongCandidateDomain {
                    song_id: *song_id,
                    title,
                    artist: artist_domains[song_id].clone(),
                })
            })
            .collect();
        let songs = songs?;
        let evidence = Arc::new(CatalogCandidateEvidenceTable {
            comparison_key_id: DIAGNOSTIC_TITLE_COMPARISON_KEY_ID,
            songs: songs
                .iter()
                .map(|song| CatalogCandidateSongEvidence {
                    song_id: song.song_id,
                    title: song.title.evidence.clone(),
                    artist: song.artist.evidence.clone(),
                })
                .collect(),
        });
        Ok(Self { songs, evidence })
    }

    /// Scores every song independently for each observed text field.
    ///
    /// The two music-select title presentations remain separate observations and are not counted
    /// as independent votes or reduced to one decision.
    #[must_use]
    pub fn observe(
        &self,
        observations: &ScreenFieldObservations,
    ) -> ScreenCatalogCandidateObservations {
        match observations {
            ScreenFieldObservations::Result(observations) => self.observe_result(observations),
            ScreenFieldObservations::MusicSelect(observations) => {
                self.observe_music_select(observations)
            }
        }
    }

    fn observe_result(
        &self,
        observations: &ResultScreenFieldObservations,
    ) -> ScreenCatalogCandidateObservations {
        let title = observation_forms(&observations.title.open_text);
        let artist = observation_forms(&observations.artist.open_text);
        let candidates = self
            .songs
            .iter()
            .map(|song| ResultSongCandidateObservation {
                song_id: song.song_id,
                title: score_text(&title, &song.title),
                artist: score_text(&artist, &song.artist),
            })
            .collect();
        ScreenCatalogCandidateObservations::Result {
            comparison_key_id: DIAGNOSTIC_TITLE_COMPARISON_KEY_ID,
            catalog: Arc::clone(&self.evidence),
            candidates,
        }
    }

    fn observe_music_select(
        &self,
        observations: &MusicSelectScreenFieldObservations,
    ) -> ScreenCatalogCandidateObservations {
        let central_title = observation_forms(&observations.central_title.open_text);
        let artist = observation_forms(&observations.artist.open_text);
        let active_list_title = observation_forms(&observations.active_list_title.open_text);
        let candidates = self
            .songs
            .iter()
            .map(|song| MusicSelectSongCandidateObservation {
                song_id: song.song_id,
                central_title: score_text(&central_title, &song.title),
                artist: score_text(&artist, &song.artist),
                active_list_title: score_text(&active_list_title, &song.title),
                active_list_title_prefix: score_prefix(&active_list_title, &song.title),
            })
            .collect();
        ScreenCatalogCandidateObservations::MusicSelect {
            comparison_key_id: DIAGNOSTIC_TITLE_COMPARISON_KEY_ID,
            catalog: Arc::clone(&self.evidence),
            candidates,
        }
    }
}

fn text_candidate_domains<'a, T: Copy + Ord>(
    candidates: impl IntoIterator<Item = (T, DisplayVariantKind, &'a str)>,
) -> BTreeMap<T, TextCandidateDomain> {
    let variants: Vec<_> = candidates
        .into_iter()
        .filter(|(_, kind, _)| *kind != DisplayVariantKind::SearchTerm)
        .map(|(song_id, _, value)| {
            (
                song_id,
                value.to_owned(),
                exact_comparison_key(value),
                folded_comparison_key(value),
            )
        })
        .collect();
    let mut folded_songs = BTreeMap::<String, BTreeSet<T>>::new();
    for (song_id, _, _, folded) in &variants {
        if !folded.is_empty() {
            folded_songs
                .entry(folded.clone())
                .or_default()
                .insert(*song_id);
        }
    }
    let mut display = BTreeMap::<T, BTreeSet<String>>::new();
    let mut exact_values = BTreeMap::<T, BTreeSet<String>>::new();
    let mut folded = BTreeMap::<T, BTreeSet<String>>::new();
    for (song_id, raw, exact, folded_key) in variants {
        display.entry(song_id).or_default().insert(raw);
        if !exact.is_empty() {
            exact_values.entry(song_id).or_default().insert(exact);
        }
        if folded_songs
            .get(&folded_key)
            .is_some_and(|songs| songs.len() == 1)
        {
            folded.entry(song_id).or_default().insert(folded_key);
        }
    }
    display
        .into_iter()
        .map(|(song_id, display)| {
            let exact = exact_values.remove(&song_id).unwrap_or_default();
            let folded = folded.remove(&song_id).unwrap_or_default();
            let evidence = CatalogCandidateTextEvidence {
                display: display.into_iter().collect(),
                exact: exact.into_iter().collect(),
                folded: folded.into_iter().collect(),
            };
            let raw_and_exact = evidence
                .display
                .iter()
                .chain(&evidence.exact)
                .map(|value| value.chars().collect())
                .collect();
            (
                song_id,
                TextCandidateDomain {
                    raw_and_exact,
                    folded: evidence
                        .folded
                        .iter()
                        .map(|value| value.chars().collect())
                        .collect(),
                    evidence,
                },
            )
        })
        .collect()
}

struct ObservationForms {
    raw_and_exact: Vec<Vec<char>>,
    folded: Vec<char>,
}

fn observation_forms(value: &str) -> ObservationForms {
    ObservationForms {
        raw_and_exact: BTreeSet::from([
            value.chars().collect(),
            exact_comparison_key(value).chars().collect(),
        ])
        .into_iter()
        .collect(),
        folded: folded_comparison_key(value).chars().collect(),
    }
}

fn score_text(
    observations: &ObservationForms,
    candidates: &TextCandidateDomain,
) -> CatalogTextCandidateScore {
    let mut minimum_edit_distance = usize::MAX;
    let mut maximum_normalized_similarity = None;
    for (observation, candidate) in observations
        .raw_and_exact
        .iter()
        .flat_map(|observation| {
            candidates
                .raw_and_exact
                .iter()
                .map(move |candidate| (observation.as_slice(), candidate.as_slice()))
        })
        .chain(
            candidates
                .folded
                .iter()
                .map(|candidate| (observations.folded.as_slice(), candidate.as_slice())),
        )
    {
        let distance = levenshtein_distance(observation, candidate);
        minimum_edit_distance = minimum_edit_distance.min(distance);
        let compared_units = observation.len().max(candidate.len()).max(1);
        let similarity = CatalogNormalizedSimilarity {
            matching_units: compared_units - distance,
            compared_units,
        };
        if maximum_normalized_similarity
            .is_none_or(|current| similarity_is_better(similarity, current))
        {
            maximum_normalized_similarity = Some(similarity);
        }
    }
    CatalogTextCandidateScore {
        minimum_edit_distance,
        maximum_normalized_similarity: maximum_normalized_similarity
            .expect("catalog candidate sequences are non-empty"),
    }
}

fn score_prefix(
    observations: &ObservationForms,
    candidates: &TextCandidateDomain,
) -> CatalogPrefixCandidateScore {
    let mut minimum_edit_distance = usize::MAX;
    let mut maximum_normalized_similarity = None;
    for (observation, candidate) in observations
        .raw_and_exact
        .iter()
        .flat_map(|observation| {
            candidates
                .raw_and_exact
                .iter()
                .map(move |candidate| (observation.as_slice(), candidate.as_slice()))
        })
        .chain(
            candidates
                .folded
                .iter()
                .map(|candidate| (observations.folded.as_slice(), candidate.as_slice())),
        )
    {
        let prefix = &candidate[..candidate.len().min(observation.len())];
        let distance = levenshtein_distance(observation, prefix);
        minimum_edit_distance = minimum_edit_distance.min(distance);
        let compared_units = observation.len().max(prefix.len()).max(1);
        let similarity = CatalogNormalizedSimilarity {
            matching_units: compared_units - distance,
            compared_units,
        };
        if maximum_normalized_similarity
            .is_none_or(|current| similarity_is_better(similarity, current))
        {
            maximum_normalized_similarity = Some(similarity);
        }
    }
    CatalogPrefixCandidateScore {
        minimum_edit_distance,
        maximum_normalized_similarity: maximum_normalized_similarity
            .expect("catalog candidate sequences are non-empty"),
    }
}

fn similarity_is_better(
    left: CatalogNormalizedSimilarity,
    right: CatalogNormalizedSimilarity,
) -> bool {
    let left_scaled = (left.matching_units as u128) * (right.compared_units as u128);
    let right_scaled = (right.matching_units as u128) * (left.compared_units as u128);
    left_scaled > right_scaled
        || (left_scaled == right_scaled && left.compared_units > right.compared_units)
}

fn levenshtein_distance(left: &[char], right: &[char]) -> usize {
    if left.len() > right.len() {
        return levenshtein_distance(right, left);
    }
    let mut previous: Vec<_> = (0..=left.len()).collect();
    let mut current = vec![0; left.len() + 1];
    for (right_index, right_character) in right.iter().enumerate() {
        current[0] = right_index + 1;
        for (left_index, left_character) in left.iter().enumerate() {
            current[left_index + 1] = (previous[left_index + 1] + 1)
                .min(current[left_index] + 1)
                .min(previous[left_index] + usize::from(left_character != right_character));
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[left.len()]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::catalog::{FederationInput, SourceRevision, TachiFixtureAdapter};
    use crate::recognition::{
        DynamicTextObservation, MusicSelectScreenFieldObservations, ResultScreenFieldObservations,
    };

    fn catalog() -> Catalog {
        catalog_from_records(&[
            tachi_record("song-cat", "CAT", "ALPHA"),
            tachi_record("song-bat", "BAT", "BETA"),
        ])
    }

    fn catalog_from_records(records: &[serde_json::Value]) -> Catalog {
        let bytes = serde_json::to_vec(&json!({
            "schema": "scorepeek-tachi-fixture-v1",
            "records": records,
        }))
        .unwrap();
        let snapshot = TachiFixtureAdapter::parse(
            &bytes,
            SourceRevision::git_commit("0123456789abcdef0123456789abcdef01234567").unwrap(),
        )
        .unwrap();
        Catalog::default()
            .federate(FederationInput {
                tachi: Some(snapshot),
                ..FederationInput::default()
            })
            .catalog
    }

    #[test]
    fn candidate_evidence_retains_exact_catalog_strings_once_per_domain() {
        let domain = CatalogCandidateDomain::from_catalog(&catalog()).unwrap();
        assert_eq!(domain.evidence.songs.len(), 2);
        let cat = domain
            .evidence
            .songs
            .iter()
            .find(|song| song.title.display == ["CAT"])
            .unwrap();
        assert_eq!(cat.title.exact, ["CAT"]);
        assert_eq!(cat.title.folded, ["CAT"]);
        assert_eq!(cat.artist.display, ["ALPHA"]);
        assert_eq!(cat.artist.exact, ["ALPHA"]);
    }

    fn tachi_record(id: &str, title: &str, artist: &str) -> serde_json::Value {
        json!({
            "source_song_id": id,
            "title": title,
            "title_kind": "in_game_display",
            "artist": artist,
            "version": "SYNTHETIC",
            "charts": [{
                "play_type": "single",
                "difficulty": "normal",
                "level": 1,
                "notes": 1,
                "source_chart_id": "spn",
                "product_versions": ["synthetic-v1"],
                "primary": true
            }],
            "primary_infinitas": true
        })
    }

    fn text(value: &str) -> DynamicTextObservation {
        DynamicTextObservation {
            input_width: 100,
            output_timesteps: value.chars().count(),
            open_text: value.to_owned(),
        }
    }

    #[test]
    fn result_keeps_every_song_and_does_not_reduce_conflicting_field_evidence() {
        let domain = CatalogCandidateDomain::from_catalog(&catalog()).unwrap();
        let observations = ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text("CAT"),
            artist: text("BETA"),
            clear_type: text("FAILED"),
            difficulty: text("HYPER"),
            level: text("8"),
            notes: text("800"),
            current_score: text("1200"),
        });
        let ScreenCatalogCandidateObservations::Result { candidates, .. } =
            domain.observe(&observations)
        else {
            panic!("result observations changed screen");
        };
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.title.minimum_edit_distance == 0)
                .count(),
            1
        );
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.artist.minimum_edit_distance == 0)
                .count(),
            1
        );
        assert_ne!(
            candidates
                .iter()
                .find(|candidate| candidate.title.minimum_edit_distance == 0)
                .unwrap()
                .song_id,
            candidates
                .iter()
                .find(|candidate| candidate.artist.minimum_edit_distance == 0)
                .unwrap()
                .song_id
        );
    }

    #[test]
    fn music_select_preserves_both_title_presentations_as_separate_evidence() {
        let domain = CatalogCandidateDomain::from_catalog(&catalog()).unwrap();
        let observations =
            ScreenFieldObservations::MusicSelect(MusicSelectScreenFieldObservations {
                central_title: text("CAT"),
                artist: text("ALPHA"),
                selected_chart: text("HYPER 8"),
                active_list_title: text("BAT"),
            });
        let ScreenCatalogCandidateObservations::MusicSelect { candidates, .. } =
            domain.observe(&observations)
        else {
            panic!("music-select observations changed screen");
        };
        assert_eq!(candidates.len(), 2);
        let central = candidates
            .iter()
            .find(|candidate| candidate.central_title.minimum_edit_distance == 0)
            .unwrap();
        let active = candidates
            .iter()
            .find(|candidate| candidate.active_list_title.minimum_edit_distance == 0)
            .unwrap();
        assert_ne!(central.song_id, active.song_id);
        assert_eq!(central.artist.minimum_edit_distance, 0);
    }

    #[test]
    fn comparison_forms_and_distance_metrics_are_exact_and_integer_only() {
        let width_folded = TextCandidateDomain {
            raw_and_exact: vec!["ＰＡＳＴＥＬＩＳＭ".chars().collect()],
            folded: vec!["PASTELISM".chars().collect()],
            evidence: CatalogCandidateTextEvidence {
                display: vec!["ＰＡＳＴＥＬＩＳＭ".to_owned()],
                exact: Vec::new(),
                folded: vec!["PASTELISM".to_owned()],
            },
        };
        let score = score_text(&observation_forms("PASTELISM"), &width_folded);
        assert_eq!(score.minimum_edit_distance, 0);
        assert_eq!(
            score.maximum_normalized_similarity,
            CatalogNormalizedSimilarity {
                matching_units: 9,
                compared_units: 9,
            }
        );

        assert_eq!(levenshtein_distance(&[], &['A', 'B']), 2);
        assert_eq!(levenshtein_distance(&['A', 'B'], &['A', 'C']), 1);
        assert_eq!(levenshtein_distance(&['猫'], &['犬']), 1);

        let prefix = score_prefix(
            &observation_forms("ASIAN VIRTUAL REALITIES (MELTING TOGETHE"),
            &TextCandidateDomain {
                raw_and_exact: vec![
                    "ASIAN VIRTUAL REALITIES (MELTING TOGETHER IN DAZZLING DARKNESS)"
                        .chars()
                        .collect(),
                ],
                folded: Vec::new(),
                evidence: CatalogCandidateTextEvidence {
                    display: Vec::new(),
                    exact: Vec::new(),
                    folded: Vec::new(),
                },
            },
        );
        assert_eq!(prefix.minimum_edit_distance, 0);
        assert_eq!(
            prefix.maximum_normalized_similarity,
            CatalogNormalizedSimilarity {
                matching_units: 40,
                compared_units: 40,
            }
        );

        let independent_metrics = score_text(
            &ObservationForms {
                raw_and_exact: vec!["abcdefghij".chars().collect()],
                folded: Vec::new(),
            },
            &TextCandidateDomain {
                raw_and_exact: vec![
                    "abcdef".chars().collect(),
                    "abcdefghijxxxxx".chars().collect(),
                ],
                folded: Vec::new(),
                evidence: CatalogCandidateTextEvidence {
                    display: Vec::new(),
                    exact: Vec::new(),
                    folded: Vec::new(),
                },
            },
        );
        assert_eq!(independent_metrics.minimum_edit_distance, 4);
        assert_eq!(
            independent_metrics.maximum_normalized_similarity,
            CatalogNormalizedSimilarity {
                matching_units: 10,
                compared_units: 15,
            }
        );
    }

    #[test]
    fn empty_catalog_is_an_explicit_zero_candidate_observation() {
        let domain = CatalogCandidateDomain::from_catalog(&Catalog::default()).unwrap();
        let observations = ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text("CAT"),
            artist: text("ALPHA"),
            clear_type: text("FAILED"),
            difficulty: text("HYPER"),
            level: text("8"),
            notes: text("800"),
            current_score: text("1200"),
        });
        let candidate_observations = domain.observe(&observations);
        assert_eq!(candidate_observations.candidate_count(), 0);
        assert_eq!(
            candidate_observations.comparison_key_id(),
            DIAGNOSTIC_TITLE_COMPARISON_KEY_ID
        );
    }

    #[test]
    fn search_term_only_song_fails_domain_construction_without_panicking() {
        let mut record = tachi_record("search-only", "SEARCH ALIAS", "ARTIST");
        record["title_kind"] = json!("search_term");
        let catalog = catalog_from_records(&[record]);
        let song_id = *catalog.songs().keys().next().unwrap();
        assert_eq!(
            CatalogCandidateDomain::from_catalog(&catalog).unwrap_err(),
            CatalogCandidateDomainError { song_id }
        );
    }

    #[test]
    fn cross_song_folded_collision_does_not_reappear_from_the_observation_form() {
        let catalog = catalog_from_records(&[
            tachi_record("fullwidth", "Ａ", "ARTIST A"),
            tachi_record("ascii", "A", "ARTIST B"),
        ]);
        let fullwidth_song_id = catalog
            .songs()
            .iter()
            .find(|(_, song)| {
                song.title_variants()
                    .iter()
                    .any(|variant| variant.value == "Ａ")
            })
            .map(|(song_id, _)| *song_id)
            .unwrap();
        let domain = CatalogCandidateDomain::from_catalog(&catalog).unwrap();
        let observations = ScreenFieldObservations::Result(ResultScreenFieldObservations {
            title: text("Ａ"),
            artist: text("ARTIST A"),
            clear_type: text("CLEAR"),
            difficulty: text("HYPER"),
            level: text("8"),
            notes: text("800"),
            current_score: text("1200"),
        });
        let ScreenCatalogCandidateObservations::Result { candidates, .. } =
            domain.observe(&observations)
        else {
            panic!("result observations changed screen");
        };
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.title.minimum_edit_distance == 0)
                .map(|candidate| candidate.song_id)
                .collect::<Vec<_>>(),
            [fullwidth_song_id]
        );
    }
}
