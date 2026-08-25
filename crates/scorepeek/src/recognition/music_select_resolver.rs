use std::collections::BTreeSet;

use serde::Serialize;

use crate::catalog::ScorepeekSongId;

use super::title::folded_comparison_key;
use super::{
    CatalogPrefixCandidateScore, CatalogTextCandidateScore, MusicSelectSongCandidateObservation,
};

pub const MUSIC_SELECT_SONG_RESOLVER_ID: &str =
    "scorepeek-music-select-active-prefix-corroborated-v1";

const MINIMUM_ACTIVE_PREFIX_UNITS: usize = 5;
const MAXIMUM_ACTIVE_PREFIX_EDIT_DISTANCE: usize = 1;
const MINIMUM_ACTIVE_PREFIX_MATCHING_UNITS: usize = 6;
const MINIMUM_ACTIVE_PREFIX_COMPARED_UNITS: usize = 7;
const MAXIMUM_CORROBORATION_EDIT_DISTANCE: usize = 1;
const MINIMUM_CORROBORATION_MATCHING_UNITS: usize = 4;
const MINIMUM_CORROBORATION_COMPARED_UNITS: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MusicSelectSongUnknownReason {
    EmptyActiveListTitle,
    ActiveListTitleTooShort,
    NoCatalogCandidates,
    RunnerUpMissing,
    ActivePrefixEditDistanceExceeded,
    ActivePrefixSimilarityTooLow,
    CentralTitleConflict,
    ArtistConflict,
    CorroborationConflict,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct RankedMusicSelectSongCandidate {
    pub song_id: ScorepeekSongId,
    pub central_title: CatalogTextCandidateScore,
    pub artist: CatalogTextCandidateScore,
    pub active_list_title: CatalogTextCandidateScore,
    pub active_list_title_prefix: CatalogPrefixCandidateScore,
}

impl From<&MusicSelectSongCandidateObservation> for RankedMusicSelectSongCandidate {
    fn from(candidate: &MusicSelectSongCandidateObservation) -> Self {
        Self {
            song_id: candidate.song_id,
            central_title: candidate.central_title,
            artist: candidate.artist,
            active_list_title: candidate.active_list_title,
            active_list_title_prefix: candidate.active_list_title_prefix,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct MusicSelectCorroboration {
    pub central_title: bool,
    pub artist: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MusicSelectSongResolution {
    Accepted {
        resolver_id: &'static str,
        selected: RankedMusicSelectSongCandidate,
        runner_up: RankedMusicSelectSongCandidate,
        active_prefix_edit_margin: usize,
        corroboration: MusicSelectCorroboration,
    },
    Unknown {
        resolver_id: &'static str,
        reason: MusicSelectSongUnknownReason,
        selected: Option<RankedMusicSelectSongCandidate>,
        runner_up: Option<RankedMusicSelectSongCandidate>,
        active_prefix_edit_margin: Option<usize>,
    },
}

impl MusicSelectSongResolution {
    #[must_use]
    pub const fn accepted_song_id(&self) -> Option<ScorepeekSongId> {
        match self {
            Self::Accepted { selected, .. } => Some(selected.song_id),
            Self::Unknown { .. } => None,
        }
    }
}

#[must_use]
pub fn resolve_music_select_song(
    observed_central_title: &str,
    observed_artist: &str,
    observed_active_list_title: &str,
    candidates: &[MusicSelectSongCandidateObservation],
) -> MusicSelectSongResolution {
    if observed_active_list_title.is_empty() {
        return unknown(
            MusicSelectSongUnknownReason::EmptyActiveListTitle,
            None,
            None,
            None,
        );
    }
    if folded_comparison_key(observed_active_list_title)
        .chars()
        .count()
        < MINIMUM_ACTIVE_PREFIX_UNITS
    {
        return unknown(
            MusicSelectSongUnknownReason::ActiveListTitleTooShort,
            None,
            None,
            None,
        );
    }
    if candidates.is_empty() {
        return unknown(
            MusicSelectSongUnknownReason::NoCatalogCandidates,
            None,
            None,
            None,
        );
    }
    if candidates.len() == 1 {
        return unknown(
            MusicSelectSongUnknownReason::RunnerUpMissing,
            candidates.first().map(Into::into),
            None,
            None,
        );
    }

    let ranked = rank_candidates(candidates);
    resolve_ranked(observed_central_title, observed_artist, candidates, &ranked)
}

fn rank_candidates(
    candidates: &[MusicSelectSongCandidateObservation],
) -> Vec<&MusicSelectSongCandidateObservation> {
    let mut ranked: Vec<_> = candidates.iter().collect();
    ranked.sort_by(|left, right| {
        left.active_list_title_prefix
            .minimum_edit_distance
            .cmp(&right.active_list_title_prefix.minimum_edit_distance)
            .then_with(|| {
                compare_similarity(
                    right.active_list_title_prefix.maximum_normalized_similarity,
                    left.active_list_title_prefix.maximum_normalized_similarity,
                )
            })
            .then_with(|| left.song_id.cmp(&right.song_id))
    });
    ranked
}

fn resolve_ranked(
    observed_central_title: &str,
    observed_artist: &str,
    candidates: &[MusicSelectSongCandidateObservation],
    ranked: &[&MusicSelectSongCandidateObservation],
) -> MusicSelectSongResolution {
    let [selected, runner_up, ..] = ranked else {
        return unknown(
            MusicSelectSongUnknownReason::RunnerUpMissing,
            ranked.first().map(|candidate| (*candidate).into()),
            None,
            None,
        );
    };
    let selected = RankedMusicSelectSongCandidate::from(*selected);
    let runner_up = RankedMusicSelectSongCandidate::from(*runner_up);
    let margin = runner_up
        .active_list_title_prefix
        .minimum_edit_distance
        .saturating_sub(selected.active_list_title_prefix.minimum_edit_distance);
    if selected.active_list_title_prefix.minimum_edit_distance > MAXIMUM_ACTIVE_PREFIX_EDIT_DISTANCE
    {
        return unknown(
            MusicSelectSongUnknownReason::ActivePrefixEditDistanceExceeded,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    }
    if !score_ratio_at_least(
        selected.active_list_title_prefix,
        MINIMUM_ACTIVE_PREFIX_MATCHING_UNITS,
        MINIMUM_ACTIVE_PREFIX_COMPARED_UNITS,
    ) {
        return unknown(
            MusicSelectSongUnknownReason::ActivePrefixSimilarityTooLow,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    }

    let mut survivors = active_survivors(ranked, selected.active_list_title_prefix);
    let central = strong_text_candidates(observed_central_title, candidates, |candidate| {
        candidate.central_title
    });
    let artist = strong_text_candidates(observed_artist, candidates, |candidate| candidate.artist);
    resolve_corroboration(
        &mut survivors,
        central.as_ref(),
        artist.as_ref(),
        &ResolutionContext {
            selected,
            runner_up,
            margin,
            ranked,
        },
    )
}

fn active_survivors(
    ranked: &[&MusicSelectSongCandidateObservation],
    selected: CatalogPrefixCandidateScore,
) -> BTreeSet<ScorepeekSongId> {
    ranked
        .iter()
        .take_while(|candidate| {
            candidate.active_list_title_prefix.minimum_edit_distance
                == selected.minimum_edit_distance
                && score_ratio_at_least(
                    candidate.active_list_title_prefix,
                    MINIMUM_ACTIVE_PREFIX_MATCHING_UNITS,
                    MINIMUM_ACTIVE_PREFIX_COMPARED_UNITS,
                )
        })
        .map(|candidate| candidate.song_id)
        .collect()
}

struct ResolutionContext<'a> {
    selected: RankedMusicSelectSongCandidate,
    runner_up: RankedMusicSelectSongCandidate,
    margin: usize,
    ranked: &'a [&'a MusicSelectSongCandidateObservation],
}

fn resolve_corroboration(
    survivors: &mut BTreeSet<ScorepeekSongId>,
    central: Option<&BTreeSet<ScorepeekSongId>>,
    artist: Option<&BTreeSet<ScorepeekSongId>>,
    context: &ResolutionContext<'_>,
) -> MusicSelectSongResolution {
    let selected = context.selected;
    let runner_up = context.runner_up;
    let margin = context.margin;
    let ranked = context.ranked;
    if survivors.len() == 1 {
        let Some(song_id) = survivors.first().copied() else {
            return unknown(
                MusicSelectSongUnknownReason::Ambiguous,
                Some(selected),
                Some(runner_up),
                Some(margin),
            );
        };
        return resolve_unique_survivor(song_id, central, artist, context);
    }

    let mut used_central = false;
    let mut used_artist = false;
    if let Some(central) = central {
        survivors.retain(|song_id| central.contains(song_id));
        used_central = true;
    }
    if let Some(artist) = artist {
        survivors.retain(|song_id| artist.contains(song_id));
        used_artist = true;
    }
    if survivors.is_empty() && (used_central || used_artist) {
        return unknown(
            MusicSelectSongUnknownReason::CorroborationConflict,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    }
    if survivors.len() != 1 {
        return unknown(
            MusicSelectSongUnknownReason::Ambiguous,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    }
    let Some(song_id) = survivors.first().copied() else {
        return unknown(
            MusicSelectSongUnknownReason::Ambiguous,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    };
    let Some(selected) = ranked
        .iter()
        .find(|candidate| candidate.song_id == song_id)
        .map(|candidate| RankedMusicSelectSongCandidate::from(*candidate))
    else {
        return unknown(
            MusicSelectSongUnknownReason::CorroborationConflict,
            Some(selected),
            Some(runner_up),
            Some(margin),
        );
    };
    let Some(runner_up) = ranked
        .iter()
        .find(|candidate| candidate.song_id != song_id)
        .map(|candidate| RankedMusicSelectSongCandidate::from(*candidate))
    else {
        return unknown(
            MusicSelectSongUnknownReason::RunnerUpMissing,
            Some(selected),
            None,
            None,
        );
    };
    let margin = runner_up
        .active_list_title_prefix
        .minimum_edit_distance
        .saturating_sub(selected.active_list_title_prefix.minimum_edit_distance);
    accepted(
        selected,
        runner_up,
        margin,
        MusicSelectCorroboration {
            central_title: used_central,
            artist: used_artist,
        },
    )
}

fn resolve_unique_survivor(
    song_id: ScorepeekSongId,
    central: Option<&BTreeSet<ScorepeekSongId>>,
    artist: Option<&BTreeSet<ScorepeekSongId>>,
    context: &ResolutionContext<'_>,
) -> MusicSelectSongResolution {
    if central.is_some_and(|set| !set.contains(&song_id)) {
        return unknown(
            MusicSelectSongUnknownReason::CentralTitleConflict,
            Some(context.selected),
            Some(context.runner_up),
            Some(context.margin),
        );
    }
    if artist.is_some_and(|set| !set.contains(&song_id)) {
        return unknown(
            MusicSelectSongUnknownReason::ArtistConflict,
            Some(context.selected),
            Some(context.runner_up),
            Some(context.margin),
        );
    }
    accepted(
        context.selected,
        context.runner_up,
        context.margin,
        MusicSelectCorroboration {
            central_title: central.is_some(),
            artist: artist.is_some(),
        },
    )
}

fn strong_text_candidates(
    observed: &str,
    candidates: &[MusicSelectSongCandidateObservation],
    score: impl Fn(&MusicSelectSongCandidateObservation) -> CatalogTextCandidateScore,
) -> Option<BTreeSet<ScorepeekSongId>> {
    if observed.is_empty() {
        return None;
    }
    let strong: BTreeSet<_> = candidates
        .iter()
        .filter(|candidate| {
            let score = score(candidate);
            score.minimum_edit_distance <= MAXIMUM_CORROBORATION_EDIT_DISTANCE
                && text_ratio_at_least(
                    score,
                    MINIMUM_CORROBORATION_MATCHING_UNITS,
                    MINIMUM_CORROBORATION_COMPARED_UNITS,
                )
        })
        .map(|candidate| candidate.song_id)
        .collect();
    (!strong.is_empty()).then_some(strong)
}

fn accepted(
    selected: RankedMusicSelectSongCandidate,
    runner_up: RankedMusicSelectSongCandidate,
    active_prefix_edit_margin: usize,
    corroboration: MusicSelectCorroboration,
) -> MusicSelectSongResolution {
    MusicSelectSongResolution::Accepted {
        resolver_id: MUSIC_SELECT_SONG_RESOLVER_ID,
        selected,
        runner_up,
        active_prefix_edit_margin,
        corroboration,
    }
}

fn unknown(
    reason: MusicSelectSongUnknownReason,
    selected: Option<RankedMusicSelectSongCandidate>,
    runner_up: Option<RankedMusicSelectSongCandidate>,
    active_prefix_edit_margin: Option<usize>,
) -> MusicSelectSongResolution {
    MusicSelectSongResolution::Unknown {
        resolver_id: MUSIC_SELECT_SONG_RESOLVER_ID,
        reason,
        selected,
        runner_up,
        active_prefix_edit_margin,
    }
}

fn score_ratio_at_least(
    score: CatalogPrefixCandidateScore,
    required_matching_units: usize,
    required_compared_units: usize,
) -> bool {
    ratio_at_least(
        score.maximum_normalized_similarity.matching_units,
        score.maximum_normalized_similarity.compared_units,
        required_matching_units,
        required_compared_units,
    )
}

fn text_ratio_at_least(
    score: CatalogTextCandidateScore,
    required_matching_units: usize,
    required_compared_units: usize,
) -> bool {
    ratio_at_least(
        score.maximum_normalized_similarity.matching_units,
        score.maximum_normalized_similarity.compared_units,
        required_matching_units,
        required_compared_units,
    )
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

fn compare_similarity(
    left: super::CatalogNormalizedSimilarity,
    right: super::CatalogNormalizedSimilarity,
) -> std::cmp::Ordering {
    let left_scaled = (left.matching_units as u128) * (right.compared_units as u128);
    let right_scaled = (right.matching_units as u128) * (left.compared_units as u128);
    left_scaled
        .cmp(&right_scaled)
        .then_with(|| left.compared_units.cmp(&right.compared_units))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognition::CatalogNormalizedSimilarity;

    fn id(value: u128) -> ScorepeekSongId {
        serde_json::from_str(&format!("\"{value:032x}\""))
            .expect("UUID deserialization accepts a simple UUID string")
    }

    const fn text(edit: usize, matching: usize, compared: usize) -> CatalogTextCandidateScore {
        CatalogTextCandidateScore {
            minimum_edit_distance: edit,
            maximum_normalized_similarity: CatalogNormalizedSimilarity {
                matching_units: matching,
                compared_units: compared,
            },
        }
    }

    const fn prefix(edit: usize, matching: usize, compared: usize) -> CatalogPrefixCandidateScore {
        CatalogPrefixCandidateScore {
            minimum_edit_distance: edit,
            maximum_normalized_similarity: CatalogNormalizedSimilarity {
                matching_units: matching,
                compared_units: compared,
            },
        }
    }

    fn candidate(
        song_id: ScorepeekSongId,
        central_title: CatalogTextCandidateScore,
        artist: CatalogTextCandidateScore,
        active_prefix: CatalogPrefixCandidateScore,
    ) -> MusicSelectSongCandidateObservation {
        MusicSelectSongCandidateObservation {
            song_id,
            central_title,
            artist,
            active_list_title: text(20, 20, 40),
            active_list_title_prefix: active_prefix,
        }
    }

    #[test]
    fn unique_active_prefix_accepts_without_texture_or_artist_weighting() {
        let resolution = resolve_music_select_song(
            "ASIANDRTVALREALIES",
            "かあ",
            "ASIAN VIRTUAL REALITIES (MELTING TOGETHE",
            &[
                candidate(id(1), text(20, 20, 63), text(2, 2, 4), prefix(0, 43, 43)),
                candidate(id(2), text(30, 10, 40), text(8, 0, 8), prefix(15, 28, 43)),
            ],
        );
        assert_eq!(resolution.accepted_song_id(), Some(id(1)));
        assert!(matches!(
            resolution,
            MusicSelectSongResolution::Accepted {
                corroboration: MusicSelectCorroboration {
                    central_title: false,
                    artist: false,
                },
                ..
            }
        ));
    }

    #[test]
    fn strong_corroboration_resolves_a_shared_active_prefix() {
        let resolution = resolve_music_select_song(
            "ALPHA LONG VERSION",
            "noise",
            "ALPHA",
            &[
                candidate(id(1), text(0, 16, 16), text(8, 0, 8), prefix(0, 5, 5)),
                candidate(id(2), text(9, 7, 16), text(8, 0, 8), prefix(0, 5, 5)),
            ],
        );
        assert_eq!(resolution.accepted_song_id(), Some(id(1)));
        assert!(matches!(
            resolution,
            MusicSelectSongResolution::Accepted {
                corroboration: MusicSelectCorroboration {
                    central_title: true,
                    artist: false,
                },
                ..
            }
        ));
    }

    #[test]
    fn corroboration_reselects_a_distinct_runner_up_after_an_active_tie() {
        for expected in [id(2), id(3)] {
            let resolution = resolve_music_select_song(
                "STRONG TITLE",
                "noise",
                "ALPHA",
                &[
                    candidate(id(1), text(9, 3, 12), text(8, 0, 8), prefix(0, 5, 5)),
                    candidate(
                        id(2),
                        if expected == id(2) {
                            text(0, 11, 11)
                        } else {
                            text(9, 3, 12)
                        },
                        text(8, 0, 8),
                        prefix(0, 5, 5),
                    ),
                    candidate(
                        id(3),
                        if expected == id(3) {
                            text(0, 11, 11)
                        } else {
                            text(9, 3, 12)
                        },
                        text(8, 0, 8),
                        prefix(0, 5, 5),
                    ),
                ],
            );
            assert!(matches!(
                resolution,
                MusicSelectSongResolution::Accepted {
                    selected,
                    runner_up,
                    active_prefix_edit_margin: 0,
                    ..
                } if selected.song_id == expected
                    && runner_up.song_id == id(1)
                    && runner_up.song_id != selected.song_id
            ));
        }
    }

    #[test]
    fn strong_disjoint_texture_fails_closed_instead_of_outvoting_active_title() {
        let resolution = resolve_music_select_song(
            "BETA",
            "noise",
            "ALPHA",
            &[
                candidate(id(1), text(4, 0, 4), text(8, 0, 8), prefix(0, 5, 5)),
                candidate(id(2), text(0, 4, 4), text(8, 0, 8), prefix(4, 1, 5)),
            ],
        );
        assert!(matches!(
            resolution,
            MusicSelectSongResolution::Unknown {
                reason: MusicSelectSongUnknownReason::CentralTitleConflict,
                ..
            }
        ));
    }

    #[test]
    fn short_or_ambiguous_active_evidence_remains_unknown() {
        for active_title in [
            "ABCD",
            "X    ",
            "X\u{3000}\u{3000}\u{3000}\u{3000}",
            "     ",
        ] {
            assert!(matches!(
                resolve_music_select_song(
                    "",
                    "",
                    active_title,
                    &[
                        candidate(id(1), text(8, 0, 8), text(8, 0, 8), prefix(0, 5, 5)),
                        candidate(id(2), text(8, 0, 8), text(8, 0, 8), prefix(1, 4, 5)),
                    ],
                ),
                MusicSelectSongResolution::Unknown {
                    reason: MusicSelectSongUnknownReason::ActiveListTitleTooShort,
                    ..
                }
            ));
        }
        let ambiguous = resolve_music_select_song(
            "noise",
            "noise",
            "ALPHA",
            &[
                candidate(id(1), text(5, 0, 5), text(5, 0, 5), prefix(0, 5, 5)),
                candidate(id(2), text(5, 0, 5), text(5, 0, 5), prefix(0, 5, 5)),
            ],
        );
        assert!(matches!(
            ambiguous,
            MusicSelectSongResolution::Unknown {
                reason: MusicSelectSongUnknownReason::Ambiguous,
                ..
            }
        ));
    }
}
