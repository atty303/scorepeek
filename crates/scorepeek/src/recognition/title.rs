use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use serde::Serialize;
use unicode_normalization::UnicodeNormalization as _;

use crate::catalog::{
    Catalog, ChartKey, Difficulty, DisplayVariant, DisplayVariantKind, InfinitasStatus, PlayType,
    ScorepeekSongId, SourceEvidence,
};

pub const DIAGNOSTIC_TITLE_COMPARISON_KEY_ID: &str = "scorepeek-title-nfc-without-ascii-space-v1";
pub const DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE: f64 = 0.95;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProvisionalTitleCandidate {
    pub song_id: ScorepeekSongId,
    pub variants: Vec<DisplayVariant>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProvisionalTitleCandidateSet {
    pub comparison_key_id: &'static str,
    pub domain: ProvisionalTitleCandidateDomain,
    pub source_evidence: Vec<SourceEvidence>,
    pub candidates: Vec<ProvisionalTitleCandidate>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ProvisionalTitleCandidateDomain {
    pub play_type: PlayType,
    pub difficulty: Difficulty,
    pub infinitas_status: InfinitasStatus,
}

/// Exports the exact non-search title decision domain for private provisional labeling.
///
/// The caller must bind this value to the active catalog digest when it publishes an artifact.
#[must_use]
pub fn provisional_title_candidates(catalog: &Catalog) -> ProvisionalTitleCandidateSet {
    let domain = ProvisionalTitleCandidateDomain {
        play_type: PlayType::Single,
        difficulty: Difficulty::Hyper,
        infinitas_status: InfinitasStatus::ConfirmedPresent,
    };
    let chart_key = ChartKey {
        play_type: domain.play_type,
        difficulty: domain.difficulty,
    };
    let candidates = catalog
        .songs()
        .iter()
        .filter(|(_, song)| {
            song.infinitas_status() == domain.infinitas_status
                && song.charts().contains_key(&chart_key)
        })
        .filter_map(|(song_id, song)| {
            let variants: Vec<_> = song
                .title_variants()
                .iter()
                .filter(|variant| variant.kind != DisplayVariantKind::SearchTerm)
                .cloned()
                .collect();
            (!variants.is_empty()).then_some(ProvisionalTitleCandidate {
                song_id: *song_id,
                variants,
            })
        })
        .collect();
    ProvisionalTitleCandidateSet {
        comparison_key_id: DIAGNOSTIC_TITLE_COMPARISON_KEY_ID,
        domain,
        source_evidence: catalog.source_evidence().values().cloned().collect(),
        candidates,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticTitleUnknownReason {
    LowConfidence,
    NoCandidate,
    AmbiguousCandidates,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DiagnosticTitleCandidate {
    Unique {
        song_id: ScorepeekSongId,
    },
    Unknown {
        reason: DiagnosticTitleUnknownReason,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticTitleError;

impl fmt::Display for DiagnosticTitleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("diagnostic OCR title input is invalid")
    }
}

impl Error for DiagnosticTitleError {}

/// Produces a diagnostic catalog candidate from open-text OCR output.
///
/// This is not an accepted title value: it lacks CTC-logit scoring, runner-up margin,
/// temporal agreement, and independent screen context.
///
/// # Errors
///
/// Returns an error for empty OCR text or a non-finite confidence outside `[0, 1]`.
pub fn diagnostic_title_candidate(
    catalog: &Catalog,
    ocr_text: &str,
    confidence: f64,
) -> Result<DiagnosticTitleCandidate, DiagnosticTitleError> {
    if ocr_text.is_empty() || !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(DiagnosticTitleError);
    }
    if confidence < DIAGNOSTIC_TITLE_MINIMUM_CONFIDENCE {
        return Ok(DiagnosticTitleCandidate::Unknown {
            reason: DiagnosticTitleUnknownReason::LowConfidence,
        });
    }

    let candidates = catalog.songs().iter().flat_map(|(song_id, song)| {
        song.title_variants()
            .iter()
            .map(move |variant| (*song_id, variant.kind, variant.value.as_str()))
    });
    Ok(match unique_candidate(ocr_text, candidates) {
        CandidateMatch::None => DiagnosticTitleCandidate::Unknown {
            reason: DiagnosticTitleUnknownReason::NoCandidate,
        },
        CandidateMatch::Unique(song_id) => DiagnosticTitleCandidate::Unique { song_id },
        CandidateMatch::Ambiguous => DiagnosticTitleCandidate::Unknown {
            reason: DiagnosticTitleUnknownReason::AmbiguousCandidates,
        },
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateMatch<T> {
    None,
    Unique(T),
    Ambiguous,
}

fn unique_candidate<'a, T: Copy + Ord>(
    ocr_text: &str,
    candidates: impl IntoIterator<Item = (T, DisplayVariantKind, &'a str)>,
) -> CandidateMatch<T> {
    let observed_key = comparison_key(ocr_text);
    let matches: BTreeSet<_> = candidates
        .into_iter()
        .filter(|(_, kind, _)| *kind != DisplayVariantKind::SearchTerm)
        .filter_map(|(id, _, value)| (comparison_key(value) == observed_key).then_some(id))
        .collect();
    let mut matches = matches.into_iter();
    match (matches.next(), matches.next()) {
        (None, _) => CandidateMatch::None,
        (Some(id), None) => CandidateMatch::Unique(id),
        (Some(_), Some(_)) => CandidateMatch::Ambiguous,
    }
}

fn comparison_key(value: &str) -> String {
    value.nfc().filter(|character| *character != ' ').collect()
}

#[cfg(test)]
mod tests {
    use super::{
        CandidateMatch, DiagnosticTitleCandidate, DiagnosticTitleError,
        DiagnosticTitleUnknownReason, comparison_key, diagnostic_title_candidate,
        provisional_title_candidates, unique_candidate,
    };
    use crate::catalog::{Catalog, Difficulty, DisplayVariantKind, InfinitasStatus, PlayType};

    #[test]
    fn provisional_candidate_domain_is_explicit_even_when_empty() {
        let candidates = provisional_title_candidates(&Catalog::default());
        assert_eq!(candidates.domain.play_type, PlayType::Single);
        assert_eq!(candidates.domain.difficulty, Difficulty::Hyper);
        assert_eq!(
            candidates.domain.infinitas_status,
            InfinitasStatus::ConfirmedPresent
        );
        assert!(candidates.candidates.is_empty());
    }

    #[test]
    fn comparison_key_removes_only_ascii_space_after_nfc() {
        assert_eq!(comparison_key("ABSOLUTE EVIL"), "ABSOLUTEEVIL");
        assert_eq!(comparison_key("Cafe\u{301} Noir"), "Caf\u{e9}Noir");
        assert_eq!(comparison_key("Absolute\tEvil"), "Absolute\tEvil");
        assert_eq!(comparison_key("Absolute\u{a0}Evil"), "Absolute\u{a0}Evil");
        assert_ne!(
            comparison_key("ABSOLUTE EVIL"),
            comparison_key("Absolute Evil")
        );
        assert_ne!(comparison_key("A-B"), comparison_key("AB"));
    }

    #[test]
    fn candidate_match_is_unique_by_song_and_excludes_search_terms() {
        let candidates = [
            (1, DisplayVariantKind::InGameDisplay, "ABSOLUTE EVIL"),
            (1, DisplayVariantKind::OfficialDisplay, "ABSOLUTE  EVIL"),
            (2, DisplayVariantKind::SearchTerm, "ABSOLUTEEVIL"),
        ];
        assert_eq!(
            unique_candidate("ABSOLUTEEVIL", candidates),
            CandidateMatch::Unique(1)
        );
    }

    #[test]
    fn candidate_match_rejects_collisions_and_non_exact_changes() {
        let collision = [
            (1, DisplayVariantKind::InGameDisplay, "A B"),
            (2, DisplayVariantKind::AlternateDisplay, "AB"),
        ];
        assert_eq!(unique_candidate("AB", collision), CandidateMatch::Ambiguous);
        assert_eq!(
            unique_candidate("A-B", [(1, DisplayVariantKind::InGameDisplay, "AB")]),
            CandidateMatch::None
        );
    }

    #[test]
    fn diagnostic_input_and_confidence_fail_closed() {
        let catalog = Catalog::default();
        assert_eq!(
            diagnostic_title_candidate(&catalog, "ABSOLUTEEVIL", 0.94),
            Ok(DiagnosticTitleCandidate::Unknown {
                reason: DiagnosticTitleUnknownReason::LowConfidence,
            })
        );
        assert_eq!(
            diagnostic_title_candidate(&catalog, "ABSOLUTEEVIL", 0.95),
            Ok(DiagnosticTitleCandidate::Unknown {
                reason: DiagnosticTitleUnknownReason::NoCandidate,
            })
        );
        assert_eq!(
            diagnostic_title_candidate(&catalog, "", 1.0),
            Err(DiagnosticTitleError)
        );
        assert_eq!(
            diagnostic_title_candidate(&catalog, "ABSOLUTEEVIL", f64::NAN),
            Err(DiagnosticTitleError)
        );
    }
}
