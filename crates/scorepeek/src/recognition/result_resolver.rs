use serde::Serialize;

use crate::catalog::ScorepeekSongId;

use super::{CatalogTextCandidateScore, ResultSongCandidateObservation};

pub const RESULT_SONG_RESOLVER_ID: &str =
    "scorepeek-result-song-title-primary-artist-corroborated-v2";
pub const RESULT_SONG_CHART_ASSISTED_RESOLVER_ID: &str =
    "scorepeek-result-song-title-primary-chart-assisted-v1";

const MAXIMUM_TITLE_EDIT_DISTANCE: usize = 3;
const MINIMUM_TITLE_MATCHING_UNITS: usize = 3;
const MINIMUM_TITLE_COMPARED_UNITS: usize = 4;
const MINIMUM_TITLE_EDIT_MARGIN: usize = 2;
const MINIMUM_ARTIST_MATCHING_UNITS: usize = 2;
const MINIMUM_ARTIST_COMPARED_UNITS: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSongUnknownReason {
    EmptyTitle,
    EmptyArtist,
    NoCatalogCandidates,
    RunnerUpMissing,
    TitleEditDistanceExceeded,
    TitleSimilarityTooLow,
    TitleEditMarginTooSmall,
    ArtistSimilarityTooLow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RankedResultSongCandidate {
    pub song_id: ScorepeekSongId,
    pub title: CatalogTextCandidateScore,
    pub artist: CatalogTextCandidateScore,
}

impl From<&ResultSongCandidateObservation> for RankedResultSongCandidate {
    fn from(candidate: &ResultSongCandidateObservation) -> Self {
        Self {
            song_id: candidate.song_id,
            title: candidate.title,
            artist: candidate.artist,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResultSongResolution {
    Accepted {
        resolver_id: &'static str,
        selected: RankedResultSongCandidate,
        runner_up: RankedResultSongCandidate,
        title_edit_margin: usize,
    },
    Unknown {
        resolver_id: &'static str,
        reason: ResultSongUnknownReason,
        selected: Option<RankedResultSongCandidate>,
        runner_up: Option<RankedResultSongCandidate>,
        title_edit_margin: Option<usize>,
    },
}

impl ResultSongResolution {
    #[must_use]
    pub const fn accepted_song_id(&self) -> Option<ScorepeekSongId> {
        match self {
            Self::Accepted { selected, .. } => Some(selected.song_id),
            Self::Unknown { .. } => None,
        }
    }
}

#[must_use]
pub fn resolve_result_song(
    observed_title: &str,
    observed_artist: &str,
    candidates: &[ResultSongCandidateObservation],
) -> ResultSongResolution {
    if observed_title.is_empty() {
        return unknown(ResultSongUnknownReason::EmptyTitle, None, None, None);
    }
    if observed_artist.is_empty() {
        return unknown(ResultSongUnknownReason::EmptyArtist, None, None, None);
    }
    if candidates.is_empty() {
        return unknown(
            ResultSongUnknownReason::NoCatalogCandidates,
            None,
            None,
            None,
        );
    }
    if candidates.len() == 1 {
        return unknown(
            ResultSongUnknownReason::RunnerUpMissing,
            candidates.first().map(Into::into),
            None,
            None,
        );
    }
    let mut ranked: Vec<_> = candidates.iter().collect();
    ranked.sort_by(|left, right| {
        left.title
            .minimum_edit_distance
            .cmp(&right.title.minimum_edit_distance)
            .then_with(|| left.song_id.cmp(&right.song_id))
    });
    let selected = RankedResultSongCandidate::from(ranked[0]);
    let runner_up = RankedResultSongCandidate::from(ranked[1]);
    let margin = runner_up
        .title
        .minimum_edit_distance
        .saturating_sub(selected.title.minimum_edit_distance);
    if selected.title.minimum_edit_distance > MAXIMUM_TITLE_EDIT_DISTANCE {
        return unknown(
            ResultSongUnknownReason::TitleEditDistanceExceeded,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    }
    if !ratio_at_least(
        selected.title.maximum_normalized_similarity.matching_units,
        selected.title.maximum_normalized_similarity.compared_units,
        MINIMUM_TITLE_MATCHING_UNITS,
        MINIMUM_TITLE_COMPARED_UNITS,
    ) {
        return unknown(
            ResultSongUnknownReason::TitleSimilarityTooLow,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    }
    if margin < MINIMUM_TITLE_EDIT_MARGIN {
        return unknown(
            ResultSongUnknownReason::TitleEditMarginTooSmall,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    }
    if !ratio_at_least(
        selected.artist.maximum_normalized_similarity.matching_units,
        selected.artist.maximum_normalized_similarity.compared_units,
        MINIMUM_ARTIST_MATCHING_UNITS,
        MINIMUM_ARTIST_COMPARED_UNITS,
    ) {
        return unknown(
            ResultSongUnknownReason::ArtistSimilarityTooLow,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    }
    ResultSongResolution::Accepted {
        resolver_id: RESULT_SONG_RESOLVER_ID,
        selected,
        runner_up,
        title_edit_margin: margin,
    }
}

/// Uses one catalog-unique SP chart only to complete an otherwise unknown primary decision.
/// An already accepted primary decision is returned unchanged, even when chart evidence conflicts.
#[must_use]
pub fn assist_unknown_result_song_with_chart(
    primary: ResultSongResolution,
    matching_song_ids: &[ScorepeekSongId],
) -> ResultSongResolution {
    let ResultSongResolution::Unknown {
        reason,
        selected: Some(selected),
        runner_up: Some(runner_up),
        title_edit_margin: Some(title_edit_margin),
        ..
    } = &primary
    else {
        return primary;
    };
    if !matches!(
        reason,
        ResultSongUnknownReason::TitleEditMarginTooSmall
            | ResultSongUnknownReason::ArtistSimilarityTooLow
    ) || matching_song_ids != [selected.song_id]
    {
        return primary;
    }
    ResultSongResolution::Accepted {
        resolver_id: RESULT_SONG_CHART_ASSISTED_RESOLVER_ID,
        selected: *selected,
        runner_up: *runner_up,
        title_edit_margin: *title_edit_margin,
    }
}

fn unknown(
    reason: ResultSongUnknownReason,
    selected: Option<RankedResultSongCandidate>,
    runner_up: Option<RankedResultSongCandidate>,
    title_edit_margin: Option<usize>,
) -> ResultSongResolution {
    ResultSongResolution::Unknown {
        resolver_id: RESULT_SONG_RESOLVER_ID,
        reason,
        selected,
        runner_up,
        title_edit_margin,
    }
}

fn ratio_at_least(
    matching_units: usize,
    compared_units: usize,
    required_matching_units: usize,
    required_compared_units: usize,
) -> bool {
    compared_units > 0
        && matching_units.saturating_mul(required_compared_units)
            >= compared_units.saturating_mul(required_matching_units)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognition::CatalogNormalizedSimilarity;

    fn id(value: u128) -> ScorepeekSongId {
        serde_json::from_str(&format!("\"{value:032x}\""))
            .expect("UUID deserialization accepts a simple UUID string")
    }

    fn score(edit: usize, matching: usize, compared: usize) -> CatalogTextCandidateScore {
        CatalogTextCandidateScore {
            minimum_edit_distance: edit,
            maximum_normalized_similarity: CatalogNormalizedSimilarity {
                matching_units: matching,
                compared_units: compared,
            },
        }
    }

    fn candidate(
        song_id: ScorepeekSongId,
        title: CatalogTextCandidateScore,
        artist: CatalogTextCandidateScore,
    ) -> ResultSongCandidateObservation {
        ResultSongCandidateObservation {
            song_id,
            title,
            artist,
        }
    }

    #[test]
    fn accepts_exact_and_one_edit_titles_with_artist_corroboration() {
        let exact = resolve_result_song(
            "ABSOLUTEEVIL",
            "Yuta Imai",
            &[
                candidate(id(1), score(0, 12, 12), score(0, 9, 9)),
                candidate(id(2), score(4, 8, 12), score(8, 1, 9)),
            ],
        );
        assert_eq!(exact.accepted_song_id(), Some(id(1)));

        let one_edit = resolve_result_song(
            "ANEMON",
            "d Team HuΣeR X Yvya",
            &[
                candidate(id(1), score(1, 6, 7), score(20, 20, 43)),
                candidate(id(2), score(3, 4, 7), score(17, 17, 35)),
            ],
        );
        assert_eq!(one_edit.accepted_song_id(), Some(id(1)));

        let measured_result_ocr = resolve_result_song(
            "Miracle Sumpho",
            "US",
            &[
                candidate(id(1), score(3, 11, 14), score(2, 2, 4)),
                candidate(id(2), score(8, 5, 13), score(9, 0, 11)),
            ],
        );
        assert_eq!(measured_result_ocr.accepted_song_id(), Some(id(1)));
    }

    #[test]
    fn chart_assistance_completes_only_unknown_primary_decisions() {
        let candidates = [
            candidate(id(1), score(0, 8, 8), score(8, 1, 8)),
            candidate(id(2), score(1, 7, 8), score(8, 1, 8)),
        ];
        let primary = resolve_result_song("TITLE", "artist", &candidates);
        assert!(matches!(primary, ResultSongResolution::Unknown { .. }));
        let assisted = assist_unknown_result_song_with_chart(primary, &[id(1)]);
        assert_eq!(assisted.accepted_song_id(), Some(id(1)));
        assert!(matches!(
            assisted,
            ResultSongResolution::Accepted {
                resolver_id: RESULT_SONG_CHART_ASSISTED_RESOLVER_ID,
                ..
            }
        ));

        let accepted = ResultSongResolution::Accepted {
            resolver_id: RESULT_SONG_RESOLVER_ID,
            selected: RankedResultSongCandidate::from(&candidates[0]),
            runner_up: RankedResultSongCandidate::from(&candidates[1]),
            title_edit_margin: 2,
        };
        let retained = assist_unknown_result_song_with_chart(accepted.clone(), &[id(2)]);
        assert_eq!(retained, accepted);
    }

    #[test]
    fn fails_closed_for_blank_ambiguous_or_uncorroborated_observations() {
        assert_eq!(
            resolve_result_song("", "", &[]),
            unknown(ResultSongUnknownReason::EmptyTitle, None, None, None)
        );
        let ambiguous = resolve_result_song(
            "ANEMON",
            "artist",
            &[
                candidate(id(1), score(1, 6, 7), score(1, 1, 1)),
                candidate(id(2), score(2, 6, 7), score(1, 1, 1)),
            ],
        );
        assert!(matches!(
            ambiguous,
            ResultSongResolution::Unknown {
                reason: ResultSongUnknownReason::TitleEditMarginTooSmall,
                ..
            }
        ));
        let uncorroborated = resolve_result_song(
            "ANEMON",
            "noise",
            &[
                candidate(id(1), score(1, 6, 7), score(20, 1, 43)),
                candidate(id(2), score(3, 4, 7), score(17, 1, 35)),
            ],
        );
        assert!(matches!(
            uncorroborated,
            ResultSongResolution::Unknown {
                reason: ResultSongUnknownReason::ArtistSimilarityTooLow,
                ..
            }
        ));
    }

    #[test]
    fn distinguishes_empty_catalog_from_missing_runner_up() {
        assert!(matches!(
            resolve_result_song("TITLE", "ARTIST", &[]),
            ResultSongResolution::Unknown {
                reason: ResultSongUnknownReason::NoCatalogCandidates,
                selected: None,
                ..
            }
        ));
        assert!(matches!(
            resolve_result_song(
                "TITLE",
                "ARTIST",
                &[candidate(id(1), score(0, 5, 5), score(0, 6, 6))],
            ),
            ResultSongResolution::Unknown {
                reason: ResultSongUnknownReason::RunnerUpMissing,
                selected: Some(_),
                runner_up: None,
                ..
            }
        ));
    }
}
